//! /break-it on M2.4 recompute: hostile documents, kernels, and timing. Expected values come
//! from hand calculation or from a naive sequential evaluator written here (each feature in
//! timeline order, each step asked of a separate fake kernel one at a time), never from the
//! recompute under test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;
#[path = "common/recompute.rs"]
mod recompute;

use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use example::EXAMPLE;
use proptest::prelude::*;
use recompute::{WAIT, calls_since, checked, described, engine, id, published};
use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{BooleanOp, Generation, Kernel, KernelRequest, Op, Primitive, ShapeData};
use silico_core::pool::Pending;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};

/// A document of these features, each a JSON object's text.
fn document(features: &[String]) -> Checked {
    let features: Vec<&str> = features.iter().map(String::as_str).collect();
    recompute::document(&features)
}

fn fid(i: usize) -> String {
    format!("f{i:07}")
}

fn boxed(i: &str, position: [f64; 3], size: [f64; 3]) -> String {
    format!(
        r#"{{"id": "{i}", "type": "box", "v": 1, "name": "B", "position": [{}, {}, {}], "length": {}, "width": {}, "height": {}}}"#,
        position[0], position[1], position[2], size[0], size[1], size[2]
    )
}

fn sphere(i: &str, position: [f64; 3], radius: f64) -> String {
    format!(
        r#"{{"id": "{i}", "type": "sphere", "v": 1, "name": "S", "position": [{}, {}, {}], "radius": {radius}}}"#,
        position[0], position[1], position[2]
    )
}

fn cylinder(i: &str, position: [f64; 3], radius: f64, height: f64) -> String {
    format!(
        r#"{{"id": "{i}", "type": "cylinder", "v": 1, "name": "C", "position": [{}, {}, {}], "radius": {radius}, "height": {height}}}"#,
        position[0], position[1], position[2]
    )
}

fn boolean(i: &str, kind: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{i}", "type": "{kind}", "v": 1, "name": "X", "base": "{base}", "tool": "{tool}"}}"#
    )
}

fn refuse_spheres_of(engine: &Recompute<FakeKernel>, radius: f64, code: ErrorCode) {
    engine.kernel().refuse(move |op| {
        matches!(op, Op::MakePrimitive(Primitive::Sphere { radius: r }) if *r == radius)
            .then(|| Error::new(code, "MakeSphere failed"))
    });
}

// ---------------------------------------------------------------------------------------
// Generations at their end.

#[test]
fn a_kernel_at_the_last_generation_doesnt_panic_when_a_recompute_starts() {
    // Once: `Recompute::start` computes the next generation as `max(..) + 1`, which
    // overflows when the kernel's generation is u64::MAX: a panic in the caller's thread
    // (debug), or in release a wrap to generation 0, whose every request the kernel
    // supersedes, so the recompute is Superseded forever and nothing says why.
    let kernel = FakeKernel::new(1).unwrap();
    kernel.advance(Generation(u64::MAX));
    let engine = Recompute::new(kernel);
    let started = std::panic::catch_unwind(AssertUnwindSafe(|| engine.start(checked(EXAMPLE))));
    let run = started.expect("start panicked");
    match run.wait_timeout(WAIT).ok() {
        Some(Ok(RunOutcome::Published(_))) => {}
        Some(Err(error)) => assert_ne!(error.code(), ErrorCode::Internal, "{error}"),
        other => panic!("neither built nor refused with a reason: {other:?}"),
    }
}

#[test]
fn cancelling_at_the_last_generation_still_supersedes_the_running_recompute() {
    // Once: `cancel` saturates at u64::MAX, so the generation doesn't change, the kernel
    // isn't advanced, and the "cancelled" recompute runs on and publishes.
    let kernel = FakeKernel::new(1).unwrap();
    kernel.advance(Generation(u64::MAX - 1));
    let engine = Recompute::new(kernel);
    let first = engine.published();
    let hold = engine.kernel().hold(|_| true);
    let run = engine.start(checked(EXAMPLE));
    assert!(engine.kernel().wait_running(1, WAIT));
    engine.cancel();
    hold.release();
    match run.wait_timeout(WAIT).ok() {
        Some(Ok(RunOutcome::Superseded)) => {}
        other => panic!("a cancelled recompute came to {other:?}"),
    }
    assert_eq!(engine.published(), first);
}

// ---------------------------------------------------------------------------------------
// A kernel that panics.

/// A kernel whose submit panics on spheres: a bug anywhere under a recompute's step.
struct Panicky(FakeKernel);

impl Kernel for Panicky {
    fn generation(&self) -> Generation {
        self.0.generation()
    }

    fn advance(&self, generation: Generation) {
        self.0.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        if request.name() == "MakeSphere" {
            panic!("a bug under a recompute step");
        }
        if request.name() == "MakeCylinder" {
            // A formatted message: a `String` payload, not a `&str`.
            let n = 2;
            panic!("bug number {n} under a recompute step");
        }
        self.0.submit(generation, request)
    }
}

#[test]
fn a_panic_under_one_step_ends_the_recompute_with_an_error_not_a_hang() {
    // Once: the panicking step's thread had counted itself running and never uncounts,
    // so every other thread of the recompute waits for it forever (`ready` empty,
    // `running` 1), `thread::scope` never returns, and `Run::wait` hangs: the handling of a
    // dead recompute thread (`stopped()`) is never reached. Up to 64 threads leak per panic.
    let engine = Recompute::new(Panicky(FakeKernel::new(2).unwrap()));
    let run = engine.start(document(&[
        boxed("aaaaaaaa", [0.0; 3], [1.0, 1.0, 1.0]),
        sphere("bbbbbbbb", [0.0; 3], 1.0),
    ]));
    match run.wait_timeout(Duration::from_secs(5)) {
        Ok(Err(error)) => {
            assert_eq!(error.code(), ErrorCode::Internal, "{error}");
            // Which feature's step, and the panic's own words.
            assert_eq!(error.feature(), Some("bbbbbbbb"));
            assert_eq!(error.cause(), Some("a bug under a recompute step"));
        }
        Ok(Ok(outcome)) => panic!("a panicking step came to {outcome:?}"),
        Err(_) => panic!("the recompute hung after a step panicked"),
    }
}

#[test]
fn a_panic_with_a_formatted_message_is_named_with_it() {
    let engine = Recompute::new(Panicky(FakeKernel::new(2).unwrap()));
    let run = engine.start(document(&[
        r#"{"id": "cccccccc", "type": "cylinder", "v": 1, "name": "C", "position": [0, 0, 0], "radius": 1, "height": 1}"#.to_owned(),
    ]));
    let error = run
        .wait_timeout(Duration::from_secs(5))
        .ok()
        .expect("hung")
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(error.feature(), Some("cccccccc"));
    assert_eq!(error.cause(), Some("bug number 2 under a recompute step"));
}

#[test]
fn after_a_step_panics_the_next_recompute_still_publishes() {
    let engine = Recompute::new(Panicky(FakeKernel::new(2).unwrap()));
    let _hung = engine.start(document(&[
        boxed("aaaaaaaa", [0.0; 3], [1.0, 1.0, 1.0]),
        sphere("bbbbbbbb", [0.0; 3], 1.0),
    ]));
    std::thread::sleep(Duration::from_millis(200));
    let model = published(
        &engine,
        document(&[boxed("aaaaaaaa", [0.0; 3], [2.0, 1.0, 1.0])]),
    );
    assert_eq!(described(&model, "aaaaaaaa"), "box(2, 1, 1)");
}

// ---------------------------------------------------------------------------------------
// Waiting.

#[test]
fn waiting_forever_with_a_timeout_of_duration_max_works() {
    let engine = engine(2);
    let run = engine.start(checked(EXAMPLE));
    assert!(matches!(
        run.wait_timeout(Duration::MAX),
        Ok(Ok(RunOutcome::Published(_)))
    ));
}

#[test]
fn polling_a_run_until_it_finishes_gives_the_same_model_as_waiting() {
    let engine = engine(2);
    let mut run = engine.start(checked(EXAMPLE));
    let deadline = Instant::now() + WAIT;
    let outcome = loop {
        match run.try_take() {
            Ok(outcome) => break outcome.unwrap(),
            Err(again) => run = again,
        }
        assert!(Instant::now() < deadline, "never finished");
        std::thread::yield_now();
    };
    let RunOutcome::Published(model) = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(engine.published(), Some(model));
}

#[test]
fn dropping_the_engine_mid_recompute_returns_at_once_and_the_run_is_superseded() {
    let engine = engine(2);
    let hold = engine.kernel().hold(|_| true);
    let run = engine.start(checked(EXAMPLE));
    assert!(engine.kernel().wait_running(1, WAIT));
    let at = Instant::now();
    drop(engine);
    assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
    match run.wait_timeout(WAIT).ok() {
        Some(Ok(RunOutcome::Superseded)) => {}
        other => panic!("{other:?}"),
    }
    drop(hold);
}

#[test]
fn a_dropped_run_still_publishes() {
    let engine = engine(2);
    drop(engine.start(checked(EXAMPLE)));
    let deadline = Instant::now() + WAIT;
    while engine.published().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        described(&engine.published().unwrap(), "m3tq8v0z"),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
}

// ---------------------------------------------------------------------------------------
// Supersession storms and concurrent callers.

#[test]
fn two_hundred_starts_in_a_row_publish_only_the_last() {
    let engine = engine(4);
    let hold = engine.kernel().hold(|_| true);
    let runs: Vec<_> = (1..=200)
        .map(|n| {
            engine.start(document(&[
                boxed("aaaaaaaa", [1.0, 0.0, 0.0], [f64::from(n), 1.0, 1.0]),
                sphere("bbbbbbbb", [0.0; 3], 1.0),
                boolean("cccccccc", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ]))
        })
        .collect();
    hold.release();
    let outcomes: Vec<_> = runs
        .into_iter()
        .map(|run| run.wait_timeout(WAIT).ok().expect("hung").unwrap())
        .collect();
    let (last, older) = outcomes.split_last().unwrap();
    for outcome in older {
        assert_eq!(outcome, &RunOutcome::Superseded);
    }
    let RunOutcome::Published(model) = last else {
        panic!("{last:?}")
    };
    assert_eq!(
        described(model, "cccccccc"),
        "fuse(translate(box(200, 1, 1), 1, 0, 0), sphere(1))"
    );
    assert_eq!(engine.published().as_ref(), Some(model));
}

#[test]
fn starts_from_many_threads_at_once_publish_the_newest_and_never_mix_documents() {
    let engine = Arc::new(engine(4));
    let threads = 8;
    let each = 25;
    let barrier = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let engine = Arc::clone(&engine);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let runs: Vec<(f64, _)> = (0..each)
                    .map(|n| {
                        let length = (t * each + n + 1) as f64;
                        let run = engine.start(document(&[
                            boxed("aaaaaaaa", [0.0, 0.0, 1.0], [length, 1.0, 1.0]),
                            cylinder("bbbbbbbb", [0.0; 3], 1.0, length),
                            boolean("cccccccc", "cut", "aaaaaaaa", "bbbbbbbb"),
                        ]));
                        (length, run)
                    })
                    .collect();
                runs.into_iter()
                    .map(|(length, run)| {
                        (length, run.wait_timeout(WAIT).ok().expect("hung").unwrap())
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut newest: Option<Arc<Model>> = None;
    for handle in handles {
        for (length, outcome) in handle.join().unwrap() {
            if let RunOutcome::Published(model) = outcome {
                // A published model is its own document's, whole.
                assert_eq!(
                    described(&model, "cccccccc"),
                    format!("cut(translate(box({length}, 1, 1), 0, 0, 1), cylinder(1, {length}))")
                );
                if newest
                    .as_ref()
                    .is_none_or(|n| n.generation() < model.generation())
                {
                    newest = Some(model);
                }
            }
        }
    }
    // The newest recompute can't be superseded, so it published, and its model stands.
    let newest = newest.expect("nothing published");
    assert_eq!(engine.published(), Some(newest));
}

#[test]
fn an_edit_that_shares_a_step_the_superseded_run_had_in_flight_runs_it_again() {
    let engine = engine(2);
    let hold = engine.kernel().hold(|_| true);
    let older = engine.start(checked(EXAMPLE));
    assert!(engine.kernel().wait_running(1, WAIT));
    let renamed = EXAMPLE.replace("\"Pin\"", "\"Dowel\"");
    let newer = engine.start(checked(&renamed));
    hold.release();
    assert_eq!(older.wait().unwrap(), RunOutcome::Superseded);
    let RunOutcome::Published(model) = newer.wait().unwrap() else {
        panic!()
    };
    assert_eq!(
        described(&model, "m3tq8v0z"),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
    let newest = engine.kernel().generation();
    let mut ran: Vec<_> = engine
        .kernel()
        .calls()
        .iter()
        .filter(|c| c.generation == newest)
        .map(|c| c.op.name())
        .collect();
    ran.sort_unstable();
    // Nothing of the older run finished, so the newer one runs every step.
    assert_eq!(ran, ["Cut", "MakeBox", "MakeCylinder", "Translate"]);
}

#[test]
fn steps_a_cancelled_recompute_finished_arent_run_again() {
    // docs/ports/recompute.md, open question 3: kept by key.
    let engine = engine(2);
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. })));
    let run = engine.start(checked(EXAMPLE));
    // Both started, and only the held cylinder still running: the box was made.
    let deadline = Instant::now() + WAIT;
    while engine.kernel().calls().len() < 2
        || engine.kernel().wait_running(2, Duration::from_millis(1))
    {
        assert!(Instant::now() < deadline, "the box never finished");
    }
    let mut started = calls_since(&engine, 0);
    started.sort_unstable();
    assert_eq!(started, ["MakeBox", "MakeCylinder"]);
    engine.cancel();
    assert_eq!(run.wait().unwrap(), RunOutcome::Superseded);
    drop(hold);
    let before = engine.kernel().calls().len();
    published(&engine, checked(EXAMPLE));
    assert_eq!(
        calls_since(&engine, before),
        ["MakeCylinder", "Translate", "Cut"]
    );
}

// ---------------------------------------------------------------------------------------
// Failures: which are kept, and who is blamed.

#[test]
fn only_refusals_that_follow_from_the_values_are_kept() {
    let all = [
        (ErrorCode::InvalidInput, true),
        (ErrorCode::KernelFailed, true),
        (ErrorCode::InvalidResult, true),
        (ErrorCode::WorkerCrashed, false),
        (ErrorCode::WorkerTimeout, false),
        (ErrorCode::ProtocolError, false),
        (ErrorCode::Internal, false),
        (ErrorCode::ResourceExhausted, false),
    ];
    for (code, kept) in all {
        let engine = engine(2);
        engine.kernel().refuse(move |op| {
            matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
                .then(|| Error::new(code, "no"))
        });
        let model = published(&engine, checked(EXAMPLE));
        match model.outcome(&id("a91cw0de")) {
            Some(FeatureOutcome::Failed(e)) => {
                assert_eq!(e.code(), code);
                assert_eq!(e.feature(), Some("a91cw0de"));
            }
            other => panic!("{code}: {other:?}"),
        }
        assert_eq!(
            model.outcome(&id("m3tq8v0z")),
            Some(&FeatureOutcome::Blocked { on: id("a91cw0de") }),
            "{code}"
        );
        let before = engine.kernel().calls().len();
        published(&engine, checked(EXAMPLE));
        let again = calls_since(&engine, before);
        if kept {
            assert!(again.is_empty(), "{code}: {again:?}");
        } else {
            assert_eq!(again, ["MakeCylinder"], "{code}");
        }
    }
}

#[test]
fn a_crash_that_doesnt_happen_again_builds_the_whole_model_next_time() {
    let engine = engine(2);
    let crashing = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&crashing);
    engine.kernel().refuse(move |op| {
        (flag.load(Ordering::SeqCst) && matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. })))
            .then(|| Error::new(ErrorCode::WorkerCrashed, "crashed"))
    });
    published(&engine, checked(EXAMPLE));
    crashing.store(false, Ordering::SeqCst);
    let model = published(&engine, checked(EXAMPLE));
    assert_eq!(model.errors().count(), 0);
    assert_eq!(
        described(&model, "m3tq8v0z"),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
}

#[test]
fn blocked_on_a_blocked_base_and_a_failed_tool_names_the_base_root() {
    let engine = engine(4);
    refuse_spheres_of(&engine, 2.0, ErrorCode::KernelFailed);
    let model = published(
        &engine,
        document(&[
            sphere("s0000001", [0.0; 3], 2.0),
            boxed("b0000001", [0.0; 3], [1.0, 1.0, 1.0]),
            boolean("c0000001", "cut", "b0000001", "s0000001"),
            sphere("s0000002", [1.0, 0.0, 0.0], 2.0),
            boolean("f0000001", "fuse", "c0000001", "s0000002"),
            // The tool blocked, the base failed.
            sphere("s0000003", [2.0, 0.0, 0.0], 2.0),
            boxed("b0000002", [0.0; 3], [2.0, 1.0, 1.0]),
            sphere("s0000004", [3.0, 0.0, 0.0], 2.0),
            boolean("c0000002", "common", "b0000002", "s0000004"),
            boolean("f0000002", "fuse", "s0000003", "c0000002"),
        ]),
    );
    assert_eq!(
        model.outcome(&id("f0000001")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
    assert_eq!(
        model.outcome(&id("f0000002")),
        Some(&FeatureOutcome::Blocked { on: id("s0000003") })
    );
    let failed: Vec<_> = model.errors().map(|e| e.feature().unwrap()).collect();
    assert_eq!(failed, ["s0000001", "s0000002", "s0000003", "s0000004"]);
    assert_eq!(model.bodies(), [id("b0000001"), id("b0000002")]);
}

#[test]
fn a_refused_move_fails_its_feature_and_the_make_is_kept_for_a_move_to_the_origin() {
    let engine = engine(2);
    engine.kernel().refuse(|op| {
        matches!(op, Op::Translate { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "Translate failed").with_cause("gp_Trsf"))
    });
    let doc = |at: [f64; 3]| {
        document(&[
            boxed("aaaaaaaa", at, [1.0, 1.0, 1.0]),
            sphere("bbbbbbbb", [0.0; 3], 1.0),
            boolean("cccccccc", "cut", "aaaaaaaa", "bbbbbbbb"),
        ])
    };
    let model = published(&engine, doc([5.0, 0.0, 0.0]));
    match model.outcome(&id("aaaaaaaa")) {
        Some(FeatureOutcome::Failed(e)) => {
            assert_eq!(e.code(), ErrorCode::KernelFailed);
            assert_eq!(e.feature(), Some("aaaaaaaa"));
            assert_eq!(e.cause(), Some("gp_Trsf"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        model.outcome(&id("cccccccc")),
        Some(&FeatureOutcome::Blocked { on: id("aaaaaaaa") })
    );
    assert_eq!(model.bodies(), [id("bbbbbbbb")]);
    let before = engine.kernel().calls().len();
    let model = published(&engine, doc([0.0; 3]));
    assert_eq!(calls_since(&engine, before), ["Cut"]);
    assert_eq!(
        described(&model, "cccccccc"),
        "cut(box(1, 1, 1), sphere(1))"
    );
    // Back to the document before, one publish later: its results, the move's refusal too,
    // are kept (undo runs nothing)...
    let before = engine.kernel().calls().len();
    published(&engine, doc([5.0, 0.0, 0.0]));
    assert!(calls_since(&engine, before).is_empty());
    // ...until two other documents have been published since: then asked again.
    published(&engine, doc([0.0; 3]));
    published(&engine, doc([0.0, 0.0, 7.0]));
    let before = engine.kernel().calls().len();
    published(&engine, doc([5.0, 0.0, 0.0]));
    assert_eq!(calls_since(&engine, before), ["Translate"]);
}

#[test]
fn two_identical_features_refused_are_two_errors_each_naming_its_own_feature() {
    let engine = engine(2);
    refuse_spheres_of(&engine, 2.0, ErrorCode::KernelFailed);
    let model = published(
        &engine,
        document(&[
            sphere("aaaaaaaa", [0.0; 3], 2.0),
            sphere("bbbbbbbb", [0.0; 3], 2.0),
        ]),
    );
    let failed: Vec<_> = model.errors().map(|e| e.feature().unwrap()).collect();
    assert_eq!(failed, ["aaaaaaaa", "bbbbbbbb"]);
    assert_eq!(calls_since(&engine, 0), ["MakeSphere"]);
    assert!(model.bodies().is_empty());
}

// ---------------------------------------------------------------------------------------
// Shapes of the graph.

#[test]
fn a_boolean_of_two_identical_solids_waits_for_both_inputs_once() {
    for size in [1, 2, 8] {
        let engine = engine(size);
        let hold = engine
            .kernel()
            .hold(|op| matches!(op, Op::MakePrimitive(_)));
        let run = engine.start(document(&[
            boxed("aaaaaaaa", [1.0, 2.0, 3.0], [1.0, 1.0, 1.0]),
            boxed("bbbbbbbb", [1.0, 2.0, 3.0], [1.0, 1.0, 1.0]),
            boolean("cccccccc", "cut", "aaaaaaaa", "bbbbbbbb"),
        ]));
        assert!(engine.kernel().wait_running(1, WAIT));
        hold.release();
        let RunOutcome::Published(model) = run.wait_timeout(WAIT).ok().expect("hung").unwrap()
        else {
            panic!()
        };
        assert_eq!(
            described(&model, "cccccccc"),
            "cut(translate(box(1, 1, 1), 1, 2, 3), translate(box(1, 1, 1), 1, 2, 3))"
        );
        assert_eq!(calls_since(&engine, 0), ["MakeBox", "Translate", "Cut"]);
        assert_eq!(model.bodies(), [id("cccccccc")]);
    }
}

#[test]
fn a_document_of_no_features_publishes_an_empty_model() {
    let Ok(document) =
        read(br#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": []}"#)
    else {
        return;
    };
    let engine = engine(1);
    let model = published(&engine, Checked::new(document).unwrap());
    assert!(model.features().is_empty());
    assert!(model.bodies().is_empty());
    assert!(engine.kernel().calls().is_empty());
}

#[test]
fn a_primitive_placed_at_a_kilometre_builds() {
    let engine = engine(1);
    let model = published(
        &engine,
        document(&[boxed("aaaaaaaa", [1e6, -1e6, 0.000001], [1.0, 1.0, 1.0])]),
    );
    assert_eq!(
        described(&model, "aaaaaaaa"),
        "translate(box(1, 1, 1), 1000000, -1000000, 0.000001)"
    );
}

#[test]
fn a_position_one_nanometre_from_the_origin_is_moved_and_keyed_apart() {
    let engine = engine(1);
    let model = published(
        &engine,
        document(&[
            boxed("aaaaaaaa", [0.0; 3], [1.0, 1.0, 1.0]),
            boxed("bbbbbbbb", [0.0, 0.0, 0.000001], [1.0, 1.0, 1.0]),
            boxed("cccccccc", [0.0, 0.0, -0.000001], [1.0, 1.0, 1.0]),
        ]),
    );
    let key = |f: &str| match model.outcome(&id(f)) {
        Some(FeatureOutcome::Built { key, .. }) => *key,
        other => panic!("{other:?}"),
    };
    assert_ne!(key("aaaaaaaa"), key("bbbbbbbb"));
    assert_ne!(key("bbbbbbbb"), key("cccccccc"));
    assert_eq!(
        calls_since(&engine, 0),
        ["MakeBox", "Translate", "Translate"]
    );
}

#[test]
fn keys_are_the_same_on_another_engine_and_another_kernel_size() {
    let doc = || checked(EXAMPLE);
    let a = published(&engine(1), doc());
    let b = published(&engine(8), doc());
    let keys = |m: &Model| -> Vec<String> {
        m.features()
            .iter()
            .map(|(_, o)| match o {
                FeatureOutcome::Built { key, .. } => key.to_string(),
                other => panic!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(keys(&a), keys(&b));
    for key in keys(&a) {
        assert_eq!(key.len(), 64);
        assert!(
            key.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}

#[test]
fn a_chain_of_four_hundred_booleans_finishes_promptly() {
    // The fake's own shapes grow with the chain (a thousand take it 3 s alone, in sequence),
    // so the chain is kept short enough that only recompute's overhead could slow it.
    let n = 400;
    let mut features = vec![boxed(&fid(0), [0.0; 3], [1.0, 1.0, 1.0])];
    let mut last = fid(0);
    for i in 1..n {
        let p = fid(2 * i);
        let b = fid(2 * i + 1);
        features.push(sphere(&p, [0.0; 3], i as f64));
        features.push(boolean(&b, "fuse", &last, &p));
        last = b;
    }
    let engine = engine(8);
    let at = Instant::now();
    let model = published(&engine, document(&features));
    let took = at.elapsed();
    assert_eq!(model.bodies(), [id(&last)]);
    assert_eq!(engine.kernel().calls().len(), 1 + 2 * (n - 1));
    assert!(took < Duration::from_secs(5), "took {took:?}");
}

#[test]
fn five_thousand_independent_features_finish_promptly() {
    let n = 5000;
    let features: Vec<String> = (0..n)
        .map(|i| boxed(&fid(i), [0.0; 3], [(i + 1) as f64, 1.0, 1.0]))
        .collect();
    let engine = engine(8);
    let at = Instant::now();
    let model = published(&engine, document(&features));
    let took = at.elapsed();
    assert_eq!(model.bodies().len(), n);
    assert!(engine.kernel().most_running() <= 8);
    assert!(took < Duration::from_secs(5), "took {took:?}");
    // And again, unchanged: nothing runs, and it's quick.
    let at = Instant::now();
    published(&engine, document(&features));
    assert_eq!(engine.kernel().calls().len(), n);
    assert!(at.elapsed() < Duration::from_secs(2), "{:?}", at.elapsed());
}

// ---------------------------------------------------------------------------------------
// Property: recompute agrees with a naive sequential evaluator, edit after edit.

#[derive(Debug, Clone)]
enum Shape {
    Box([u8; 3]),
    Sphere(u8),
    Cylinder(u8, u8),
}

#[derive(Debug, Clone)]
struct Values {
    shape: Shape,
    position: [u8; 3],
    op: u8,
}

#[derive(Debug, Clone)]
enum Kind {
    Primitive,
    Boolean { base: usize, tool: usize },
}

fn values() -> impl Strategy<Value = Values> {
    let small = 1u8..=2;
    let shape = prop_oneof![
        prop::array::uniform3(small.clone()).prop_map(Shape::Box),
        small.clone().prop_map(Shape::Sphere),
        (small.clone(), small).prop_map(|(r, h)| Shape::Cylinder(r, h)),
    ];
    let coordinate = prop_oneof![3 => Just(0u8), 1 => Just(1u8)];
    (shape, prop::array::uniform3(coordinate), 0u8..3).prop_map(|(shape, position, op)| Values {
        shape,
        position,
        op,
    })
}

/// A timeline's structure: which features are booleans of which earlier solids, each used
/// once.
fn structure(choices: &[(bool, usize, usize)]) -> Vec<Kind> {
    let mut free: Vec<usize> = Vec::new();
    let mut kinds = Vec::new();
    for (i, &(boolean, x, y)) in choices.iter().enumerate() {
        if boolean && free.len() >= 2 {
            let base = free.remove(x % free.len());
            let tool = free.remove(y % free.len());
            kinds.push(Kind::Boolean { base, tool });
        } else {
            kinds.push(Kind::Primitive);
        }
        free.push(i);
    }
    kinds
}

fn op_of(n: u8) -> (BooleanOp, &'static str) {
    match n % 3 {
        0 => (BooleanOp::Fuse, "fuse"),
        1 => (BooleanOp::Cut, "cut"),
        _ => (BooleanOp::Common, "common"),
    }
}

fn primitive_of(shape: &Shape) -> Primitive {
    match *shape {
        Shape::Box([a, b, c]) => Primitive::Box {
            dx: f64::from(a),
            dy: f64::from(b),
            dz: f64::from(c),
        },
        Shape::Sphere(r) => Primitive::Sphere {
            radius: f64::from(r),
        },
        Shape::Cylinder(r, h) => Primitive::Cylinder {
            radius: f64::from(r),
            height: f64::from(h),
        },
    }
}

fn text_of(kinds: &[Kind], values: &[Values]) -> Checked {
    let features: Vec<String> = kinds
        .iter()
        .zip(values)
        .enumerate()
        .map(|(i, (kind, v))| match kind {
            Kind::Boolean { base, tool } => {
                boolean(&fid(i), op_of(v.op).1, &fid(*base), &fid(*tool))
            }
            Kind::Primitive => {
                let p = v.position.map(f64::from);
                match v.shape {
                    Shape::Box(s) => boxed(&fid(i), p, s.map(f64::from)),
                    Shape::Sphere(r) => sphere(&fid(i), p, f64::from(r)),
                    Shape::Cylinder(r, h) => cylinder(&fid(i), p, f64::from(r), f64::from(h)),
                }
            }
        })
        .collect();
    document(&features)
}

/// The rules both kernels refuse by: a sphere of radius 2, a move by (1, 1, 1), and every
/// common fail as the kernel would (kept); a cylinder of radius 2 and height 1 crashes its
/// worker (not kept).
fn refusals(kernel: &FakeKernel) {
    kernel.refuse(|op| match op {
        Op::MakePrimitive(Primitive::Sphere { radius }) if *radius == 2.0 => {
            Some(Error::new(ErrorCode::KernelFailed, "sphere"))
        }
        Op::MakePrimitive(Primitive::Cylinder { radius, height })
            if *radius == 2.0 && *height == 1.0 =>
        {
            Some(Error::new(ErrorCode::WorkerCrashed, "crash"))
        }
        Op::Translate { offset, .. } if *offset == [1.0, 1.0, 1.0] => {
            Some(Error::new(ErrorCode::KernelFailed, "move"))
        }
        Op::Boolean {
            op: BooleanOp::Common,
            ..
        } => Some(Error::new(ErrorCode::KernelFailed, "common")),
        _ => None,
    });
}

#[derive(Debug, Clone, PartialEq)]
enum Expected {
    Built { step: String, shape: ShapeData },
    Failed(ErrorCode),
    Blocked(usize),
}

/// A step, whether its inputs all built (so it runs, or has a result), and its error's code.
type Evaluated = (RefStep, bool, Option<ErrorCode>);

struct Reference {
    kernel: FakeKernel,
    results: BTreeMap<String, Result<ShapeData, ErrorCode>>,
}

/// One step, as a step's identity and its operation's name.
struct RefStep {
    identity: String,
    name: &'static str,
}

impl Reference {
    fn new() -> Self {
        let kernel = FakeKernel::new(1).unwrap();
        refusals(&kernel);
        Self {
            kernel,
            results: BTreeMap::new(),
        }
    }

    fn result(
        &mut self,
        identity: &str,
        ask: impl FnOnce(&FakeKernel) -> Result<ShapeData, Error>,
    ) -> Result<ShapeData, ErrorCode> {
        if let Some(result) = self.results.get(identity) {
            return result.clone();
        }
        let result = ask(&self.kernel).map_err(|e| e.code());
        self.results.insert(identity.to_owned(), result.clone());
        result
    }

    /// Each feature's outcome, and every step the document has, with whether its inputs
    /// all built (so it runs, or has a result) and its own result's code.
    fn evaluate(&mut self, kinds: &[Kind], values: &[Values]) -> (Vec<Expected>, Vec<Evaluated>) {
        let g = Generation(0);
        let mut outcomes: Vec<Expected> = Vec::new();
        let mut steps: Vec<Evaluated> = Vec::new();
        let mut last: Vec<String> = Vec::new();
        for (kind, v) in kinds.iter().zip(values) {
            match kind {
                Kind::Primitive => {
                    let primitive = primitive_of(&v.shape);
                    let make = format!("{primitive:?}");
                    let made = self.result(&make, |k| {
                        k.submit(g, KernelRequest::make(primitive).unwrap())
                            .wait()
                            .done()
                            .unwrap()
                    });
                    steps.push((
                        RefStep {
                            identity: make.clone(),
                            name: Op::MakePrimitive(primitive).name(),
                        },
                        true,
                        made.as_ref().err().copied(),
                    ));
                    let mut identity = make.clone();
                    let mut outcome = made.clone();
                    if v.position != [0, 0, 0] {
                        let offset = v.position.map(f64::from);
                        let moved = format!("T[{make}]{offset:?}");
                        let ran = made.is_ok();
                        let result = match &made {
                            Ok(shape) => {
                                let shape = shape.clone();
                                Some(self.result(&moved, |k| {
                                    k.submit(g, KernelRequest::translate(&shape, offset).unwrap())
                                        .wait()
                                        .done()
                                        .unwrap()
                                        .map(|b| b.shape)
                                }))
                            }
                            Err(_) => None,
                        };
                        steps.push((
                            RefStep {
                                identity: moved.clone(),
                                name: "Translate",
                            },
                            ran,
                            result.as_ref().and_then(|r| r.as_ref().err().copied()),
                        ));
                        if let Some(result) = result {
                            outcome = result;
                        }
                        identity = moved;
                    }
                    outcomes.push(match outcome {
                        Ok(shape) => Expected::Built {
                            step: identity.clone(),
                            shape,
                        },
                        Err(code) => Expected::Failed(code),
                    });
                    last.push(identity);
                }
                Kind::Boolean { base, tool } => {
                    let (op, _) = op_of(v.op);
                    let identity = format!("{op:?}[{}][{}]", last[*base], last[*tool]);
                    let root = |u: usize| match &outcomes[u] {
                        Expected::Failed(_) => Some(u),
                        Expected::Blocked(on) => Some(*on),
                        Expected::Built { .. } => None,
                    };
                    let blocked = root(*base).or_else(|| root(*tool));
                    let inputs_built = matches!(
                        (&outcomes[*base], &outcomes[*tool]),
                        (Expected::Built { .. }, Expected::Built { .. })
                    );
                    let mut code = None;
                    let outcome = match (&outcomes[*base], &outcomes[*tool], blocked) {
                        (
                            Expected::Built { shape: a, .. },
                            Expected::Built { shape: b, .. },
                            None,
                        ) => {
                            let (a, b) = (a.clone(), b.clone());
                            match self.result(&identity, |k| {
                                k.submit(g, KernelRequest::boolean(op, &a, &b))
                                    .wait()
                                    .done()
                                    .unwrap()
                                    .map(|b| b.shape)
                            }) {
                                Ok(shape) => Expected::Built {
                                    step: identity.clone(),
                                    shape,
                                },
                                Err(c) => {
                                    code = Some(c);
                                    Expected::Failed(c)
                                }
                            }
                        }
                        (_, _, Some(on)) => Expected::Blocked(on),
                        _ => panic!("inputs built but blocked"),
                    };
                    steps.push((
                        RefStep {
                            identity: identity.clone(),
                            name: op.name(),
                        },
                        inputs_built,
                        code,
                    ));
                    outcomes.push(outcome);
                    last.push(identity);
                }
            }
        }
        (outcomes, steps)
    }
}

fn kept_code(code: Option<ErrorCode>) -> bool {
    match code {
        None => true,
        Some(c) => matches!(
            c,
            ErrorCode::InvalidInput | ErrorCode::KernelFailed | ErrorCode::InvalidResult
        ),
    }
}

fn check_model(model: &Model, kinds: &[Kind], expected: &[Expected]) -> Result<(), TestCaseError> {
    prop_assert_eq!(model.features().len(), expected.len());
    for (i, ((fid_, outcome), want)) in model.features().iter().zip(expected).enumerate() {
        prop_assert_eq!(fid_, &id(&fid(i)));
        match (outcome, want) {
            (FeatureOutcome::Built { built, .. }, Expected::Built { shape, .. }) => {
                prop_assert_eq!(&built.shape, shape, "feature {}", i);
            }
            (FeatureOutcome::Failed(e), Expected::Failed(code)) => {
                prop_assert_eq!(e.code(), *code, "feature {}", i);
                let named = fid(i);
                prop_assert_eq!(e.feature(), Some(named.as_str()));
            }
            (FeatureOutcome::Blocked { on }, Expected::Blocked(root)) => {
                prop_assert_eq!(on, &id(&fid(*root)), "feature {}", i);
            }
            (got, want) => prop_assert!(false, "feature {}: got {:?}, want {:?}", i, got, want),
        }
    }
    // Keys match exactly when the work does.
    let built: Vec<(usize, silico_core::recompute::Key, &String)> = model
        .features()
        .iter()
        .zip(expected)
        .enumerate()
        .filter_map(|(i, ((_, o), w))| match (o, w) {
            (FeatureOutcome::Built { key, .. }, Expected::Built { step, .. }) => {
                Some((i, *key, step))
            }
            _ => None,
        })
        .collect();
    for a in &built {
        for b in &built {
            prop_assert_eq!(a.1 == b.1, a.2 == b.2, "features {} and {}", a.0, b.0);
        }
    }
    // Bodies: built, and not consumed by a built boolean.
    let mut consumed = vec![false; kinds.len()];
    for (kind, want) in kinds.iter().zip(expected) {
        if let (Kind::Boolean { base, tool }, Expected::Built { .. }) = (kind, want) {
            consumed[*base] = true;
            consumed[*tool] = true;
        }
    }
    let bodies: Vec<FeatureId> = expected
        .iter()
        .enumerate()
        .filter(|(i, w)| matches!(w, Expected::Built { .. }) && !consumed[*i])
        .map(|(i, _)| id(&fid(i)))
        .collect();
    prop_assert_eq!(model.bodies(), bodies.as_slice());
    let failures = expected
        .iter()
        .filter(|w| matches!(w, Expected::Failed(_)))
        .count();
    prop_assert_eq!(model.errors().count(), failures);
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    #[test]
    fn recompute_matches_a_sequential_evaluator_and_runs_exactly_the_new_steps(
        choices in prop::collection::vec((prop::bool::weighted(0.45), 0usize..16, 0usize..16), 1..12),
        edits in prop::collection::vec(prop::collection::vec(values(), 12), 3),
        size in prop_oneof![Just(1usize), Just(2), Just(8)],
    ) {
        let kinds = structure(&choices);
        let engine = engine(size);
        refusals(engine.kernel());
        let mut reference = Reference::new();
        // The steps with a result kept after the last publish: those kept since, while the
        // last two published plans use them.
        let mut known: BTreeSet<String> = BTreeSet::new();
        let mut previous_plan: BTreeSet<String> = BTreeSet::new();
        for values in &edits {
            let values = &values[..kinds.len()];
            let (expected, steps) = reference.evaluate(&kinds, values);
            let before = engine.kernel().calls().len();
            let model = published(&engine, text_of(&kinds, values));
            check_model(&model, &kinds, &expected)?;
            // The calls: each step whose inputs built and that has no result kept, once.
            let mut want: Vec<&str> = Vec::new();
            let mut seen = BTreeSet::new();
            for (step, runs, _) in &steps {
                if *runs && !known.contains(&step.identity) && seen.insert(step.identity.clone()) {
                    want.push(step.name);
                }
            }
            want.sort_unstable();
            let mut got = calls_since(&engine, before);
            got.sort_unstable();
            prop_assert_eq!(&got, &want, "calls");
            let plan: BTreeSet<String> = steps.iter().map(|(s, _, _)| s.identity.clone()).collect();
            known.extend(
                steps
                    .iter()
                    .filter(|(_, runs, code)| *runs && kept_code(*code))
                    .map(|(s, _, _)| s.identity.clone()),
            );
            known.retain(|identity| plan.contains(identity) || previous_plan.contains(identity));
            previous_plan = plan;
        }
    }
}
