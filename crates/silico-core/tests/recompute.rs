//! M2.4: recompute on the in-memory kernel. Expected values are written by hand: which
//! kernel calls an edit makes (a feature's steps and those below it, nothing else), what
//! each feature comes to, the bodies, and the shapes, as the fake describes what made them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;
#[path = "common/model.rs"]
mod model;
#[path = "common/recompute.rs"]
mod recompute;

use std::sync::Arc;
use std::time::Duration;

use StepState::{Blocked, Built, Failed, Queued, Running};
use example::EXAMPLE;
use model::{WAIT, checked, described, id, published};
use recompute::{calls_since, document, engine};
use silico_core::document::{Checked, FeatureId, Measure};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    BooleanOp, BuiltData, Clients, ElementCounts, ElementFate, Generation, History, InputHistory,
    Kernel, KernelRequest, Op, Primitive, Topology,
};
use silico_core::pool::Pending;
use silico_core::recompute::{
    FeatureOutcome, FeatureStates, Model, Recompute, RunOutcome, StepCount, StepState,
};
use silico_core::{Error, ErrorCode};
use silico_protocol::Reply;

fn ids(texts: &[&str]) -> Vec<FeatureId> {
    texts.iter().map(|t| id(t)).collect()
}

/// Each feature's state in a rebuild, as its count lists them: `pairs`, by ID, in order.
fn states(pairs: &[(&str, StepState)]) -> FeatureStates {
    pairs
        .iter()
        .map(|(feature, state)| (id(feature), *state))
        .collect()
}

/// The design's example with a different pin or block.
fn example_with(block: &str, pin: &str) -> Checked {
    document(&[
        &format!(r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", {block}}}"#),
        &format!(r#"{{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", {pin}}}"#),
        r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de", "note": "Clearance for an M5 bolt"}"#,
    ])
}

const BLOCK: &str = r#""position": [0, 0, 0], "length": 40, "width": 20, "height": 10"#;
const PIN: &str = r#""position": [20, 10, -1], "radius": 2.5, "height": 12"#;

#[test]
fn the_example_builds_as_written() {
    let engine = engine(2);
    let model = published(&engine, checked(EXAMPLE));
    assert_eq!(
        described(&model, "m3tq8v0z"),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
    assert_eq!(described(&model, "k7f2qx3m"), "box(40, 20, 10)");
    assert_eq!(model.bodies(), ids(&["m3tq8v0z"]));
    assert_eq!(model.errors().count(), 0);
    // The block is at the origin, so it isn't moved.
    let mut calls = calls_since(&engine, 0);
    calls.sort_unstable();
    assert_eq!(calls, ["Cut", "MakeBox", "MakeCylinder", "Translate"]);
    // A cut's history is its two inputs'.
    let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&id("m3tq8v0z")) else {
        panic!()
    };
    assert_eq!(built.history.as_ref().unwrap().inputs.len(), 2);
    assert_eq!(engine.published(), Some(model));
}

#[test]
fn every_request_is_made_after_the_requests_it_uses() {
    let engine = engine(4);
    published(&engine, checked(EXAMPLE));
    let calls = engine.kernel().calls();
    let at = |name: &str| calls.iter().position(|c| c.op.name() == name).unwrap();
    assert!(at("MakeCylinder") < at("Translate"));
    assert!(at("Translate") < at("Cut"));
    assert!(at("MakeBox") < at("Cut"));
}

#[test]
fn an_unchanged_renamed_renoted_or_reordered_document_runs_nothing() {
    let engine = engine(2);
    published(&engine, checked(EXAMPLE));
    let before = engine.kernel().calls().len();
    let same = published(&engine, checked(EXAMPLE));
    let renamed = EXAMPLE
        .replace("\"Pin\"", "\"Dowel\"")
        .replace("Clearance for an M5 bolt", "Clearance for an M6 bolt");
    let renamed = published(&engine, checked(&renamed));
    // The pin before the block: the same features, in another valid order.
    let reordered = document(&[
        &format!(r#"{{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", {PIN}}}"#),
        &format!(r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", {BLOCK}}}"#),
        r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}"#,
    ]);
    let reordered = published(&engine, reordered);
    assert!(calls_since(&engine, before).is_empty());
    for model in [&same, &renamed, &reordered] {
        assert_eq!(
            described(model, "m3tq8v0z"),
            "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
        );
    }
}

#[test]
fn a_value_edit_reruns_that_feature_and_those_below_it_only() {
    let engine = engine(2);
    published(&engine, example_with(BLOCK, PIN));
    let before = engine.kernel().calls().len();
    let thicker = r#""position": [20, 10, -1], "radius": 3, "height": 12"#;
    let model = published(&engine, example_with(BLOCK, thicker));
    assert_eq!(
        calls_since(&engine, before),
        ["MakeCylinder", "Translate", "Cut"]
    );
    assert_eq!(
        described(&model, "m3tq8v0z"),
        "cut(box(40, 20, 10), translate(cylinder(3, 12), 20, 10, -1))"
    );
}

#[test]
fn a_position_edit_reruns_the_move_not_the_make() {
    let engine = engine(2);
    published(&engine, example_with(BLOCK, PIN));
    let before = engine.kernel().calls().len();
    let moved = r#""position": [21, 10, -1], "radius": 2.5, "height": 12"#;
    published(&engine, example_with(BLOCK, moved));
    assert_eq!(calls_since(&engine, before), ["Translate", "Cut"]);
}

#[test]
fn identical_steps_run_once() {
    let engine = engine(4);
    let cube = r#""type": "box", "v": 1, "name": "Cube", "position": [1, 2, 3], "length": 1, "width": 1, "height": 1"#;
    let model = published(
        &engine,
        document(&[
            &format!(r#"{{"id": "aaaaaaaa", {cube}}}"#),
            &format!(r#"{{"id": "bbbbbbbb", {cube}}}"#),
        ]),
    );
    let mut calls = calls_since(&engine, 0);
    calls.sort_unstable();
    assert_eq!(calls, ["MakeBox", "Translate"]);
    let key = |feature: &str| match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { key, .. }) => *key,
        other => panic!("{other:?}"),
    };
    assert_eq!(key("aaaaaaaa"), key("bbbbbbbb"));
    assert_eq!(model.bodies(), ids(&["aaaaaaaa", "bbbbbbbb"]));
}

fn refuse_cylinders(engine: &Recompute<FakeKernel>, code: ErrorCode) {
    engine.kernel().refuse(move |op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| Error::new(code, "MakeCylinder failed").with_cause("StdFail_NotDone"))
    });
}

#[test]
fn a_failed_feature_blocks_those_using_it_and_every_other_builds() {
    let engine = engine(2);
    refuse_cylinders(&engine, ErrorCode::KernelFailed);
    let model = published(&engine, checked(EXAMPLE));
    let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("a91cw0de")) else {
        panic!("{model:?}")
    };
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    assert_eq!(error.feature(), Some("a91cw0de"));
    assert_eq!(error.cause(), Some("StdFail_NotDone"));
    assert_eq!(
        model.outcome(&id("m3tq8v0z")),
        Some(&FeatureOutcome::Blocked { on: id("a91cw0de") })
    );
    assert_eq!(described(&model, "k7f2qx3m"), "box(40, 20, 10)");
    // The cut consumed nothing, so the block is a body.
    assert_eq!(model.bodies(), ids(&["k7f2qx3m"]));
    let errors: Vec<_> = model.errors().collect();
    assert_eq!(errors, [error]);
    // The cut never ran.
    assert!(!calls_since(&engine, 0).contains(&"Cut"));
}

#[test]
fn a_feature_blocked_on_a_blocked_one_names_the_failure_at_the_root() {
    let engine = engine(2);
    refuse_cylinders(&engine, ErrorCode::KernelFailed);
    let model = published(
        &engine,
        document(&[
            &format!(r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", {BLOCK}}}"#),
            &format!(r#"{{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", {PIN}}}"#),
            r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}"#,
            r#"{"id": "s0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 1}"#,
            r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "s0000001", "tool": "m3tq8v0z"}"#,
        ]),
    );
    assert_eq!(
        model.outcome(&id("f0000001")),
        Some(&FeatureOutcome::Blocked { on: id("a91cw0de") })
    );
    assert_eq!(model.bodies(), ids(&["k7f2qx3m", "s0000001"]));
    assert_eq!(model.errors().count(), 1);
}

#[test]
fn a_refusal_is_kept_with_its_key_and_an_edit_tries_again() {
    let engine = engine(2);
    refuse_cylinders(&engine, ErrorCode::KernelFailed);
    published(&engine, checked(EXAMPLE));
    let before = engine.kernel().calls().len();
    // The same values fail the same way: not asked again.
    let again = published(&engine, checked(EXAMPLE));
    assert!(calls_since(&engine, before).is_empty());
    assert!(matches!(
        again.outcome(&id("a91cw0de")),
        Some(FeatureOutcome::Failed(_))
    ));
    // Different values are a different step.
    let thicker = r#""position": [20, 10, -1], "radius": 3, "height": 12"#;
    published(&engine, example_with(BLOCK, thicker));
    assert_eq!(calls_since(&engine, before), ["MakeCylinder"]);
}

#[test]
fn a_crash_isnt_kept_so_the_next_recompute_tries_again() {
    let engine = engine(2);
    refuse_cylinders(&engine, ErrorCode::WorkerCrashed);
    let model = published(&engine, checked(EXAMPLE));
    assert!(matches!(
        model.outcome(&id("a91cw0de")),
        Some(FeatureOutcome::Failed(e)) if e.code() == ErrorCode::WorkerCrashed
    ));
    let before = engine.kernel().calls().len();
    published(&engine, checked(EXAMPLE));
    assert_eq!(calls_since(&engine, before), ["MakeCylinder"]);
}

#[test]
fn independent_branches_run_at_once() {
    let engine = engine(4);
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(_)));
    let run = engine.start(checked(EXAMPLE));
    // The two primitives, held, run at once: neither branch waits on the other. Counted
    // while they're held, where nothing else can run (each solid's measure, from M2.6,
    // waits for its solid), not over the whole run, where a measure and a move can overlap
    // however the scheduler lets them.
    assert!(engine.kernel().wait_running(2, WAIT));
    assert_eq!(engine.kernel().most_running(), 2);
    let mut started = calls_since(&engine, 0);
    started.sort_unstable();
    assert_eq!(started, ["MakeBox", "MakeCylinder"]);
    hold.release();
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
}

/// `count` but its clock, which no test can pin: its `elapsed` zero.
fn clockless(count: StepCount) -> StepCount {
    StepCount {
        elapsed: Duration::ZERO,
        ..count
    }
}

/// Polls `progress` until its whole count but its clock ([`clockless`]) is `want`, for at
/// most [`WAIT`].
#[track_caller]
fn until_counted(progress: &silico_core::recompute::StepCounter, want: &StepCount) {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let now = clockless(progress.now());
        if now == *want {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{now:?}, not {want:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The example's cylinders held until the hold is dropped.
fn hold_cylinders(engine: &Recompute<FakeKernel>) -> silico_core::fake::Hold {
    engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. })))
}

#[test]
fn a_recompute_counts_its_steps_names_the_one_running_and_ends_with_all_done() {
    // The example's steps, by hand: the block at the origin (one), the pin at (20, 10, -1)
    // (its cylinder and its move: two), and the hole cut (one): four. The pin's cylinder
    // held: the block's step settles; the move and the cut wait on the cylinder.
    let engine = engine(2);
    let hold = hold_cylinders(&engine);
    let run = engine.start(checked(EXAMPLE));
    let progress = run.progress();
    until_counted(
        &progress,
        &StepCount {
            done: 1,
            steps: 4,
            running: Some("a91cw0de cylinder".to_owned()),
            ended: false,
            elapsed: Duration::ZERO,
            features: states(&[
                ("k7f2qx3m", Built),
                ("a91cw0de", Running),
                ("m3tq8v0z", Queued),
            ]),
        },
    );
    hold.release();
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    let all_done = StepCount {
        done: 4,
        steps: 4,
        running: None,
        ended: true,
        elapsed: Duration::ZERO,
        features: states(&[
            ("k7f2qx3m", Built),
            ("a91cw0de", Built),
            ("m3tq8v0z", Built),
        ]),
    };
    assert_eq!(clockless(progress.now()), all_done);
    // Again, every step kept: all four settled before any runs.
    let again = engine.start(checked(EXAMPLE));
    let progress = again.progress();
    assert!(matches!(again.wait().unwrap(), RunOutcome::Published(_)));
    assert_eq!(clockless(progress.now()), all_done);
    // A thicker pin, its cylinder held: the block's step is kept, settled before any runs,
    // so one is done while the cylinder is held, though the block never ran.
    let before = engine.kernel().calls().len();
    let hold = hold_cylinders(&engine);
    let thicker = engine.start(example_with(
        BLOCK,
        r#""position": [20, 10, -1], "radius": 3, "height": 12"#,
    ));
    let progress = thicker.progress();
    until_counted(
        &progress,
        &StepCount {
            done: 1,
            steps: 4,
            running: Some("a91cw0de cylinder".to_owned()),
            ended: false,
            elapsed: Duration::ZERO,
            features: states(&[
                ("k7f2qx3m", Built),
                ("a91cw0de", Running),
                ("m3tq8v0z", Queued),
            ]),
        },
    );
    hold.release();
    assert!(matches!(thicker.wait().unwrap(), RunOutcome::Published(_)));
    assert_eq!(clockless(progress.now()), all_done);
    // Kept, not run: the pin's cylinder, its move, and the cut ran, one after the other,
    // and no box was made.
    assert_eq!(
        calls_since(&engine, before),
        ["MakeCylinder", "Translate", "Cut"]
    );
}

#[test]
fn a_second_rebuild_of_the_same_document_has_its_kept_features_built_at_its_first_plan() {
    // The example and a ball on a branch of its own. The first rebuild: the ball's worker
    // crashes, a fault, which isn't kept; the block, the pin, and the cut are. The second,
    // of the same document, the ball's sphere held: the three kept settle with the plan,
    // never run, and are built then, the ball running. Steps by hand: the block (1), the
    // pin's cylinder and move (2), the cut (1), the ball at the origin (1): five, four
    // kept.
    let engine = engine(2);
    let crashing = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let crashes = Arc::clone(&crashing);
    engine.kernel().refuse(move |op| {
        (crashes.load(std::sync::atomic::Ordering::SeqCst)
            && matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })))
        .then(|| Error::new(ErrorCode::WorkerCrashed, "crashed"))
    });
    let ball = r#"{"id": "q0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}"#;
    let block = format!(r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", {BLOCK}}}"#);
    let pin = format!(r#"{{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", {PIN}}}"#);
    let hole = r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}"#;
    let parts = [block.as_str(), pin.as_str(), hole, ball];
    let first = engine.start(document(&parts));
    let progress = first.progress();
    assert!(matches!(first.wait().unwrap(), RunOutcome::Published(_)));
    assert_eq!(
        progress.now().features,
        states(&[
            ("k7f2qx3m", Built),
            ("a91cw0de", Built),
            ("m3tq8v0z", Built),
            ("q0000001", StepState::Faulted),
        ])
    );
    crashing.store(false, std::sync::atomic::Ordering::SeqCst);
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
    let before = engine.kernel().calls().len();
    let second = engine.start(document(&parts));
    let progress = second.progress();
    until_counted(
        &progress,
        &StepCount {
            done: 4,
            steps: 5,
            running: Some("q0000001 sphere".to_owned()),
            ended: false,
            elapsed: Duration::ZERO,
            features: states(&[
                ("k7f2qx3m", Built),
                ("a91cw0de", Built),
                ("m3tq8v0z", Built),
                ("q0000001", Running),
            ]),
        },
    );
    hold.release();
    assert!(matches!(second.wait().unwrap(), RunOutcome::Published(_)));
    // Only the ball ran again.
    assert_eq!(calls_since(&engine, before), ["MakeSphere"]);
    assert_eq!(
        progress.now().features,
        states(&[
            ("k7f2qx3m", Built),
            ("a91cw0de", Built),
            ("m3tq8v0z", Built),
            ("q0000001", Built),
        ])
    );
}

#[test]
fn steps_that_cant_run_are_counted_done_before_any_runs() {
    // The pin's cylinder refused (kept), then the block made 41 long, its box held: the
    // cylinder's step is kept, failed, and the move and the cut, which use it, can't run.
    // All three are settled while the block's box is held: three of four done.
    let engine = engine(2);
    refuse_cylinders(&engine, ErrorCode::KernelFailed);
    published(&engine, checked(EXAMPLE));
    let before = engine.kernel().calls().len();
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let run = engine.start(example_with(
        r#""position": [0, 0, 0], "length": 41, "width": 20, "height": 10"#,
        PIN,
    ));
    let progress = run.progress();
    until_counted(
        &progress,
        &StepCount {
            done: 3,
            steps: 4,
            running: Some("k7f2qx3m box".to_owned()),
            ended: false,
            elapsed: Duration::ZERO,
            features: states(&[
                ("k7f2qx3m", Running),
                ("a91cw0de", Failed),
                ("m3tq8v0z", Blocked),
            ]),
        },
    );
    hold.release();
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    assert_eq!(
        clockless(progress.now()),
        StepCount {
            done: 4,
            steps: 4,
            running: None,
            ended: true,
            elapsed: Duration::ZERO,
            features: states(&[
                ("k7f2qx3m", Built),
                ("a91cw0de", Failed),
                ("m3tq8v0z", Blocked)
            ]),
        }
    );
    // Kept, not run: the block's box alone was made, and no cylinder was asked for again.
    assert_eq!(calls_since(&engine, before), ["MakeBox"]);
}

#[test]
fn a_step_only_a_feature_refused_mid_run_needs_is_counted_done_unrun_while_another_branch_is_held()
{
    // Two unit boxes 5 mm apart, fused: solids that don't touch fuse to two solids, as
    // OCCT's fuse gives them (the fake can't model a split, so its fuse is answered so by
    // hand). The fuse's own check (one solid at most: design S5) refuses it once its step
    // is made, mid-run. A 2 mm box cut from the fuse: the cut's step serves the cut alone,
    // made from the refused fuse, so it comes off the ready queue after the fuse is judged
    // (its last input, made in the same lock) and is settled without running. A cylinder,
    // held, on a branch of its own, keeps the run going meanwhile. Its steps, by hand: the
    // unit box (1: the first's, at the origin, and the second's are the same step, as
    // `identical_steps_run_once` has it), the second's move (1), the fuse (1), the 2 mm box
    // (1), the cut (1), and the cylinder (1): six. While the cylinder is held, the other
    // five are settled, the cut's among them: four ran, one never will.
    let engine = engine(2);
    let fates = |first: u32, n: u32| -> Vec<ElementFate> {
        (first..first + n)
            .map(|k| ElementFate {
                kept: Some(k),
                ..ElementFate::default()
            })
            .collect()
    };
    // Each box's 6 faces, 12 edges, and 8 vertices kept, the second's numbered after the
    // first's: 12, 24, and 16 in two solids.
    let split = History {
        inputs: vec![
            InputHistory {
                faces: fates(1, 6),
                edges: fates(1, 12),
                vertices: fates(1, 8),
            },
            InputHistory {
                faces: fates(7, 6),
                edges: fates(13, 12),
                vertices: fates(9, 8),
            },
        ],
        result: ElementCounts {
            faces: 12,
            edges: 24,
            vertices: 16,
            solids: 2,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![]; 12],
            outer_edges: vec![vec![]; 12],
            edge_vertices: vec![vec![]; 24],
            solid_faces: vec![(1..=6).collect(), (7..=12).collect()],
        },
    };
    split.validate(2).unwrap();
    engine.kernel().answer_instead(move |op| match op {
        Op::Boolean {
            op: BooleanOp::Fuse,
            base,
            ..
        } => Some(Reply::Built(BuiltData {
            shape: base.clone(),
            history: split.clone(),
        })),
        _ => None,
    });
    let hold = hold_cylinders(&engine);
    let run = engine.start(document(&[
        r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "First", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Second", "position": [5, 0, 0], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "ffffffff", "type": "fuse", "v": 1, "name": "Apart", "base": "aaaaaaaa", "tool": "bbbbbbbb"}"#,
        r#"{"id": "cccccccc", "type": "box", "v": 1, "name": "Cutter", "position": [0, 0, 0], "length": 2, "width": 2, "height": 2}"#,
        r#"{"id": "dddddddd", "type": "cut", "v": 1, "name": "Cut", "base": "ffffffff", "tool": "cccccccc"}"#,
        r#"{"id": "pppppppp", "type": "cylinder", "v": 1, "name": "Pin", "position": [0, 0, 0], "radius": 1, "height": 1}"#,
    ]));
    let progress = run.progress();
    until_counted(
        &progress,
        &StepCount {
            done: 5,
            steps: 6,
            running: Some("pppppppp cylinder".to_owned()),
            ended: false,
            elapsed: Duration::ZERO,
            features: states(&[
                ("aaaaaaaa", Built),
                ("bbbbbbbb", Built),
                ("ffffffff", Failed),
                ("cccccccc", Built),
                ("dddddddd", Blocked),
                ("pppppppp", Running),
            ]),
        },
    );
    // Counted while held: the cut was never asked for, so it was counted as it was skipped,
    // not by a run.
    assert!(!calls_since(&engine, 0).contains(&"Cut"));
    hold.release();
    let Ok(RunOutcome::Published(model)) = run.wait() else {
        panic!("not published")
    };
    assert_eq!(
        clockless(progress.now()),
        StepCount {
            done: 6,
            steps: 6,
            running: None,
            ended: true,
            elapsed: Duration::ZERO,
            features: states(&[
                ("aaaaaaaa", Built),
                ("bbbbbbbb", Built),
                ("ffffffff", Failed),
                ("cccccccc", Built),
                ("dddddddd", Blocked),
                ("pppppppp", Built)
            ]),
        }
    );
    // The fuse refused in its own words (design S5, as tests/sweep_refusals.rs pins them),
    // the cut blocked on it, and never run.
    let Some(FeatureOutcome::Failed(refused)) = model.outcome(&id("ffffffff")) else {
        panic!("{model:?}")
    };
    assert_eq!(refused.code(), ErrorCode::KernelFailed);
    assert_eq!(
        refused.message(),
        "fuse ffffffff makes 2 solids: its base aaaaaaaa and tool bbbbbbbb don't touch, or meet only along an edge or at a point"
    );
    assert_eq!(
        model.outcome(&id("dddddddd")),
        Some(&FeatureOutcome::Blocked { on: id("ffffffff") })
    );
    let mut calls = calls_since(&engine, 0);
    calls.sort_unstable();
    assert_eq!(
        calls,
        ["Fuse", "MakeBox", "MakeBox", "MakeCylinder", "Translate"]
    );
}

#[test]
fn a_chain_runs_one_step_at_a_time() {
    // Counted once the solid's summary has started, held, where its steps have all run and
    // its description's batches (sent at once, three of them: M4.4) haven't yet.
    let engine = engine(4);
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::Summarize { .. }));
    let run = engine.start(document(&[
        r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Moved", "position": [5, 0, 0], "length": 1, "width": 1, "height": 1}"#,
    ]));
    let deadline = std::time::Instant::now() + WAIT;
    while !engine
        .kernel()
        .calls()
        .iter()
        .any(|call| matches!(call.op, Op::Summarize { .. }))
    {
        assert!(std::time::Instant::now() < deadline, "never summarized");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(engine.kernel().most_running(), 1);
    hold.release();
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    assert_eq!(calls_since(&engine, 0), ["MakeBox", "Translate"]);
}

#[test]
fn a_superseded_recompute_publishes_nothing_and_the_newer_one_does() {
    let engine = engine(2);
    let first = published(&engine, checked(EXAMPLE));
    let hold = engine.kernel().hold(|_| true);
    let thicker = r#""position": [20, 10, -1], "radius": 3, "height": 12"#;
    let older = engine.start(example_with(BLOCK, thicker));
    assert!(engine.kernel().wait_running(1, WAIT));
    let wider = r#""position": [0, 0, 0], "length": 50, "width": 20, "height": 10"#;
    let newer = engine.start(example_with(wider, PIN));
    assert_eq!(older.wait().unwrap(), RunOutcome::Superseded);
    // Until the newer one finishes, the published model is the one before.
    assert_eq!(engine.published(), Some(Arc::clone(&first)));
    hold.release();
    let RunOutcome::Published(model) = newer.wait().unwrap() else {
        panic!("superseded")
    };
    assert_eq!(
        described(&model, "m3tq8v0z"),
        "cut(box(50, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
    assert_eq!(engine.published(), Some(model));
}

#[test]
fn a_cancelled_recompute_leaves_the_published_model_as_it_was() {
    let engine = engine(2);
    let first = published(&engine, checked(EXAMPLE));
    let hold = engine.kernel().hold(|_| true);
    let thicker = r#""position": [20, 10, -1], "radius": 3, "height": 12"#;
    let run = engine.start(example_with(BLOCK, thicker));
    assert!(engine.kernel().wait_running(1, WAIT));
    let at_cancel = engine.kernel().calls().len();
    let cancelled = engine.kernel().generation();
    engine.cancel();
    assert_eq!(run.wait().unwrap(), RunOutcome::Superseded);
    drop(hold);
    assert_eq!(engine.published(), Some(first));
    // The held make was stopped, and nothing after it (its move, the cut) was started.
    let late: Vec<_> = engine.kernel().calls()[at_cancel..]
        .iter()
        .filter(|c| c.generation == cancelled)
        .map(|c| c.op.name())
        .collect();
    assert!(late.is_empty(), "{late:?}");
}

#[test]
fn the_same_document_makes_the_same_model_on_any_kernel_size() {
    let doc = || {
        document(&[
            &format!(r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", {BLOCK}}}"#),
            &format!(r#"{{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", {PIN}}}"#),
            r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}"#,
            r#"{"id": "s0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [3, 0, 0], "radius": 1}"#,
            r#"{"id": "c0000001", "type": "cone", "v": 1, "name": "Tip", "position": [0, 3, 0], "radius1": 1, "radius2": 0, "height": 2}"#,
            r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "s0000001", "tool": "c0000001"}"#,
            r#"{"id": "f0000002", "type": "common", "v": 1, "name": "All", "base": "m3tq8v0z", "tool": "f0000001"}"#,
        ])
    };
    let models: Vec<Arc<Model>> = [1, 2, 8]
        .into_iter()
        .map(|size| published(&engine(size), doc()))
        .collect();
    for model in &models[1..] {
        assert_eq!(model.features(), models[0].features());
        assert_eq!(model.bodies(), models[0].bodies());
    }
    assert_eq!(models[0].bodies(), ids(&["f0000002"]));
}

#[test]
fn a_step_two_features_share_runs_once_and_each_feature_above_it_runs_once() {
    // FreeCAD's testRecompute's diamond: one input used along two paths. Two identical
    // spheres are one step, used by two booleans.
    let engine = engine(4);
    let doc = |radius: &str| {
        let ball = |id: &str| {
            format!(
                r#"{{"id": "{id}", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": {radius}}}"#
            )
        };
        document(&[
            &ball("s0000001"),
            &ball("s0000002"),
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
            r#"{"id": "b0000002", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 2, "width": 2, "height": 2}"#,
            r#"{"id": "c0000001", "type": "cut", "v": 1, "name": "One", "base": "b0000001", "tool": "s0000001"}"#,
            r#"{"id": "c0000002", "type": "cut", "v": 1, "name": "Two", "base": "b0000002", "tool": "s0000002"}"#,
        ])
    };
    published(&engine, doc("1"));
    let mut calls = calls_since(&engine, 0);
    calls.sort_unstable();
    assert_eq!(calls, ["Cut", "Cut", "MakeBox", "MakeBox", "MakeSphere"]);
    let before = engine.kernel().calls().len();
    published(&engine, doc("2"));
    let mut calls = calls_since(&engine, before);
    calls.sort_unstable();
    assert_eq!(calls, ["Cut", "Cut", "MakeSphere"]);
}

#[test]
fn the_order_steps_finish_in_changes_nothing() {
    let doc = || {
        document(&[
            &format!(r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", {BLOCK}}}"#),
            &format!(r#"{{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", {PIN}}}"#),
            r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}"#,
            r#"{"id": "s0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [3, 0, 0], "radius": 1}"#,
            r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "m3tq8v0z", "tool": "s0000001"}"#,
        ])
    };
    // Each primitive held on its own, then released in one order and in the reverse.
    let kinds: [fn(&Op) -> bool; 3] = [
        |op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })),
        |op| matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. })),
        |op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })),
    ];
    let mut models = Vec::new();
    for reverse in [false, true] {
        let engine = engine(4);
        let holds: Vec<_> = kinds
            .iter()
            .map(|picks| engine.kernel().hold(*picks))
            .collect();
        let run = engine.start(doc());
        assert!(engine.kernel().wait_running(3, WAIT));
        let order: Vec<usize> = if reverse {
            vec![2, 1, 0]
        } else {
            vec![0, 1, 2]
        };
        for i in order {
            holds[i].release();
            std::thread::sleep(Duration::from_millis(20));
        }
        let RunOutcome::Published(model) = run.wait().unwrap() else {
            panic!("superseded")
        };
        models.push(model);
    }
    assert_eq!(models[0].features(), models[1].features());
    assert_eq!(models[0].bodies(), models[1].bodies());
}

#[test]
fn superseded_recomputes_keep_only_the_newest_ones_steps_and_an_undo_runs_nothing() {
    let engine = engine(2);
    let pin =
        |radius: u32| format!(r#""position": [20, 10, -1], "radius": {radius}, "height": 12"#);
    published(&engine, example_with(BLOCK, &pin(1)));
    // Three edits, each superseded with its cut held: each makes its cylinder first.
    let hold = engine.kernel().hold(|op| matches!(op, Op::Boolean { .. }));
    for radius in [2, 3, 4] {
        let cuts = engine
            .kernel()
            .calls()
            .iter()
            .filter(|c| c.op.name() == "Cut")
            .count();
        let run = engine.start(example_with(BLOCK, &pin(radius)));
        let deadline = std::time::Instant::now() + WAIT;
        while engine
            .kernel()
            .calls()
            .iter()
            .filter(|c| c.op.name() == "Cut")
            .count()
            == cuts
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        engine.cancel();
        assert_eq!(run.wait().unwrap(), RunOutcome::Superseded);
    }
    drop(hold);
    // The newest superseded one's steps are kept; the older ones' aren't.
    let before = engine.kernel().calls().len();
    published(&engine, example_with(BLOCK, &pin(4)));
    assert_eq!(calls_since(&engine, before), ["Cut"]);
    let before = engine.kernel().calls().len();
    published(&engine, example_with(BLOCK, &pin(2)));
    assert_eq!(
        calls_since(&engine, before),
        ["MakeCylinder", "Translate", "Cut"]
    );
    // Back to the model published before: an undo runs nothing.
    let before = engine.kernel().calls().len();
    published(&engine, example_with(BLOCK, &pin(4)));
    assert!(calls_since(&engine, before).is_empty());
}

// The key's contents: each part of a step changes it, so an edit to it never reuses
// another step's result.

const A_BOX: &str = r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 4, "width": 4, "height": 4}"#;
const A_SPHERE: &str = r#"{"id": "bbbbbbbb", "type": "sphere", "v": 1, "name": "B", "position": [0, 0, 0], "radius": 1}"#;

#[test]
fn changing_a_cut_to_a_fuse_fuses() {
    let engine = engine(2);
    let cut = r#"{"id": "cccccccc", "type": "cut", "v": 1, "name": "C", "base": "aaaaaaaa", "tool": "bbbbbbbb"}"#;
    published(&engine, document(&[A_BOX, A_SPHERE, cut]));
    let fuse = cut.replace("\"cut\"", "\"fuse\"");
    let model = published(&engine, document(&[A_BOX, A_SPHERE, &fuse]));
    assert_eq!(
        described(&model, "cccccccc"),
        "fuse(box(4, 4, 4), sphere(1))"
    );
}

#[test]
fn swapping_a_cuts_base_and_tool_cuts_the_other_way() {
    let engine = engine(2);
    let cut = |base: &str, tool: &str| {
        format!(
            r#"{{"id": "cccccccc", "type": "cut", "v": 1, "name": "C", "base": "{base}", "tool": "{tool}"}}"#
        )
    };
    published(
        &engine,
        document(&[A_BOX, A_SPHERE, &cut("aaaaaaaa", "bbbbbbbb")]),
    );
    let model = published(
        &engine,
        document(&[A_BOX, A_SPHERE, &cut("bbbbbbbb", "aaaaaaaa")]),
    );
    assert_eq!(
        described(&model, "cccccccc"),
        "cut(sphere(1), box(4, 4, 4))"
    );
}

#[test]
fn a_box_and_a_cone_of_the_same_numbers_are_different_steps() {
    let model = published(
        &engine(2),
        document(&[
            r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 3, "width": 2, "height": 5}"#,
            r#"{"id": "bbbbbbbb", "type": "cone", "v": 1, "name": "B", "position": [0, 0, 0], "radius1": 3, "radius2": 2, "height": 5}"#,
        ]),
    );
    assert_eq!(described(&model, "aaaaaaaa"), "box(3, 2, 5)");
    assert_eq!(described(&model, "bbbbbbbb"), "cone(3, 2, 5)");
}

#[test]
fn editing_any_value_of_a_cone_or_a_torus_rebuilds_it() {
    let engine = engine(2);
    let doc = |cone: [u32; 3], torus: [u32; 2]| {
        let cone = format!(
            r#"{{"id": "aaaaaaaa", "type": "cone", "v": 1, "name": "A", "position": [0, 0, 0], "radius1": {}, "radius2": {}, "height": {}}}"#,
            cone[0], cone[1], cone[2]
        );
        let torus = format!(
            r#"{{"id": "bbbbbbbb", "type": "torus", "v": 1, "name": "B", "position": [0, 0, 0], "radius1": {}, "radius2": {}}}"#,
            torus[0], torus[1]
        );
        document(&[&cone, &torus])
    };
    published(&engine, doc([3, 1, 5], [3, 1]));
    for (cone, torus, want) in [
        ([4, 1, 5], [3, 1], ("cone(4, 1, 5)", "torus(3, 1)")),
        ([4, 2, 5], [3, 1], ("cone(4, 2, 5)", "torus(3, 1)")),
        ([4, 2, 6], [4, 1], ("cone(4, 2, 6)", "torus(4, 1)")),
        ([4, 2, 6], [4, 2], ("cone(4, 2, 6)", "torus(4, 2)")),
    ] {
        let model = published(&engine, doc(cone, torus));
        assert_eq!(described(&model, "aaaaaaaa"), want.0);
        assert_eq!(described(&model, "bbbbbbbb"), want.1);
    }
}

/// A fake kernel that, for one generation, holds its last job (the measure of a feature's
/// solid) after the fake has run it and before handing it back, until let go: a recompute
/// superseded after its last job ran.
struct Gate {
    inner: FakeKernel,
    held: Generation,
    /// Set once the fake has sent the held step's result.
    delivered: std::sync::atomic::AtomicBool,
    open: std::sync::Mutex<bool>,
    opened: std::sync::Condvar,
}

impl Kernel for Gate {
    fn identity(&self) -> &str {
        self.inner.identity()
    }

    fn generation(&self) -> Generation {
        self.inner.generation()
    }

    fn advance(&self, generation: Generation) {
        self.inner.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        let before = self.inner.calls().len();
        // The held generation's last job, the measure of its feature's solid, once: so the
        // older recompute has run every job when the newer one supersedes it, and only the
        // check at publishing can stop it.
        let last = request.name() == "Summarize";
        let pending = self.inner.submit(generation, request);
        if generation == self.held
            && last
            && !self.delivered.load(std::sync::atomic::Ordering::SeqCst)
        {
            let deadline = std::time::Instant::now() + WAIT;
            while self.inner.calls().len() == before && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            // Until the fake has sent the step's result: it stops counting a step as running
            // only once its outcome is sent.
            while self.inner.wait_running(1, Duration::ZERO) && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                !self.inner.wait_running(1, Duration::ZERO),
                "the step wasn't delivered in time"
            );
            self.delivered
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.opened.wait(open).unwrap();
            }
        }
        pending
    }
}

#[test]
fn a_recompute_superseded_after_its_last_step_ran_doesnt_publish_over_the_newer_one() {
    let inner = FakeKernel::new(2).unwrap();
    let held = Generation(inner.generation().0 + 1);
    let engine = Recompute::new(Gate {
        inner,
        held,
        delivered: std::sync::atomic::AtomicBool::new(false),
        open: std::sync::Mutex::new(false),
        opened: std::sync::Condvar::new(),
    });
    let older = engine.start(document(&[
        r#"{"id": "aaaaaaaa", "type": "sphere", "v": 1, "name": "A", "position": [0, 0, 0], "radius": 7}"#,
    ]));
    // Until the fake has delivered the older recompute's last job, the measure of its solid:
    // only then does the newer one start, so the older one is superseded after every job
    // ran, and only publishing can tell.
    let deadline = std::time::Instant::now() + WAIT;
    while !engine
        .kernel()
        .delivered
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the step was never held"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // The newer recompute runs and publishes while the older one holds its finished step.
    let newer = published(&engine, checked(EXAMPLE));
    *engine.kernel().open.lock().unwrap() = true;
    engine.kernel().opened.notify_all();
    let older = older.wait_timeout(WAIT).ok().expect("hung").unwrap();
    assert_eq!(older, RunOutcome::Superseded);
    assert_eq!(engine.published(), Some(newer));
}

/// The fake kernel, noting the thread each step is asked from.
struct Threads {
    inner: FakeKernel,
    names: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

impl Kernel for Threads {
    fn identity(&self) -> &str {
        self.inner.identity()
    }

    fn generation(&self) -> Generation {
        self.inner.generation()
    }

    fn advance(&self, generation: Generation) {
        self.inner.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        let name = std::thread::current().name().unwrap_or("").to_owned();
        self.names.lock().unwrap().insert(name);
        self.inner.submit(generation, request)
    }
}

#[test]
fn a_recompute_waits_on_at_most_sixty_four_steps_at_once() {
    let engine = Recompute::new(Threads {
        inner: FakeKernel::new(4).unwrap(),
        names: std::sync::Mutex::default(),
    });
    let hold = engine.kernel().inner.hold(|_| true);
    // 200 primitives at the origin: 200 steps, none waiting on another.
    let features: Vec<String> = (0..200)
        .map(|n| {
            format!(
                r#"{{"id": "a{n:07}", "type": "sphere", "v": 1, "name": "S", "position": [0, 0, 0], "radius": {}}}"#,
                n + 1
            )
        })
        .collect();
    let features: Vec<&str> = features.iter().map(String::as_str).collect();
    let run = engine.start(document(&features));
    let deadline = std::time::Instant::now() + WAIT;
    while engine.kernel().names.lock().unwrap().len() < 64 {
        assert!(
            std::time::Instant::now() < deadline,
            "fewer than 64 threads asked"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(engine.kernel().names.lock().unwrap().len(), 64);
    drop(hold);
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    assert_eq!(engine.kernel().names.lock().unwrap().len(), 64);
}

#[test]
fn superseded_recomputes_hold_no_more_than_the_newest_ones_results_before_any_publish() {
    let engine = engine(2);
    let pin =
        |radius: u32| format!(r#""position": [20, 10, -1], "radius": {radius}, "height": 12"#);
    published(&engine, example_with(BLOCK, &pin(1)));
    let hold = engine.kernel().hold(|op| matches!(op, Op::Boolean { .. }));
    for radius in [2, 3] {
        let before = engine.kernel().calls().len();
        let run = engine.start(example_with(BLOCK, &pin(radius)));
        let deadline = std::time::Instant::now() + WAIT;
        while !calls_since(&engine, before).contains(&"Cut") {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        engine.cancel();
        assert_eq!(run.wait().unwrap(), RunOutcome::Superseded);
    }
    drop(hold);
    // The older superseded one's steps went when the newer one kept its own, before any
    // publish.
    let before = engine.kernel().calls().len();
    published(&engine, example_with(BLOCK, &pin(2)));
    assert_eq!(
        calls_since(&engine, before),
        ["MakeCylinder", "Translate", "Cut"]
    );
}

#[test]
fn steps_one_result_makes_ready_run_at_once() {
    // One make (two identical boxes), used by two moves: when it's made, both moves are
    // ready, and both run at once.
    let engine = engine(4);
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::Translate { .. }));
    let run = engine.start(document(&[
        r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [1, 0, 0], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "B", "position": [0, 1, 0], "length": 1, "width": 1, "height": 1}"#,
    ]));
    let together = engine.kernel().wait_running(2, WAIT);
    hold.release();
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    assert!(together, "the two moves didn't run at once");
}

#[test]
fn a_solid_made_is_measured_at_once_while_what_uses_it_runs() {
    // Found by the milestone's mutation run: the measure a make makes ready didn't wake an
    // idle thread (it was counted as one fewer job, not one more), so it waited for the
    // thread that made it, which passed every test. A block and a tool are made (the
    // block's make held until the tool is measured), then the cut that uses them is held:
    // the block is still measured, by another thread, while the cut runs.
    let engine = engine(4);
    let block = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx == 2.0));
    let cut = engine.kernel().hold(|op| op.name() == "Cut");
    let run = engine.start(document(&[
        r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 2, "width": 1, "height": 1}"#,
        r#"{"id": "tttttttt", "type": "box", "v": 1, "name": "T", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "bbbbbbbb", "type": "cut", "v": 1, "name": "B", "base": "aaaaaaaa", "tool": "tttttttt"}"#,
    ]));
    let summaries = || {
        engine
            .kernel()
            .calls()
            .iter()
            .filter(|call| call.op.name() == "Summarize")
            .count()
    };
    let until = |what: &str, done: &dyn Fn() -> bool| {
        let deadline = std::time::Instant::now() + WAIT;
        while !done() {
            assert!(std::time::Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    until("the tool was never measured", &|| summaries() == 1);
    block.release();
    until("the cut never ran", &|| {
        engine.kernel().wait_running(1, Duration::ZERO)
    });
    until("the block wasn't measured while the cut ran", &|| {
        summaries() == 2
    });
    cut.release();
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
}

/// The fake kernel, with the first recompute's `advance` held until let go: so a newer
/// recompute can start, and get ahead, before the older one has.
struct SlowAdvance {
    fake: FakeKernel,
    /// Set when the first recompute's `advance` is entered.
    entered: std::sync::atomic::AtomicBool,
    open: std::sync::Mutex<bool>,
    opened: std::sync::Condvar,
}

impl Kernel for SlowAdvance {
    fn identity(&self) -> &str {
        self.fake.identity()
    }

    fn generation(&self) -> Generation {
        self.fake.generation()
    }

    fn advance(&self, generation: Generation) {
        if generation == Generation(1) {
            self.entered
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.opened.wait(open).unwrap();
            }
        }
        self.fake.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        self.fake.submit(generation, request)
    }
}

#[test]
fn an_older_recompute_finishing_late_doesnt_make_the_newer_ones_results_forgotten() {
    let engine = Arc::new(Recompute::new(SlowAdvance {
        fake: FakeKernel::new(2).unwrap(),
        entered: std::sync::atomic::AtomicBool::new(false),
        open: std::sync::Mutex::new(false),
        opened: std::sync::Condvar::new(),
    }));
    let one_box = |size: u32| {
        document(&[&format!(
            r#"{{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": {size}, "width": 1, "height": 1}}"#
        )])
    };
    // The older one starts, and waits in its `advance`.
    let older = {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || engine.start(one_box(1)).wait())
    };
    // Until the older one has taken its generation and waits in its `advance`.
    let deadline = std::time::Instant::now() + WAIT;
    while !engine
        .kernel()
        .entered
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the older recompute never started"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // The newer one starts and runs, its make held.
    let hold = engine
        .kernel()
        .fake
        .hold(|op| matches!(op, Op::MakePrimitive(_)));
    let newer = engine.start(one_box(2));
    assert!(engine.kernel().fake.wait_running(1, WAIT));
    // Now the older one goes on, and finds itself superseded.
    *engine.kernel().open.lock().unwrap() = true;
    engine.kernel().opened.notify_all();
    assert_eq!(older.join().unwrap().unwrap(), RunOutcome::Superseded);
    drop(hold);
    assert!(matches!(newer.wait().unwrap(), RunOutcome::Published(_)));
    // The newer one's results are kept: its document again runs nothing.
    let before = engine.kernel().fake.calls().len();
    published(&engine, one_box(2));
    assert_eq!(engine.kernel().fake.calls().len(), before);
}

#[test]
fn three_moves_on_one_make_all_run_at_once() {
    // Waking one thread for a make's three moves would leave two waiting for it.
    let engine = engine(4);
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::Translate { .. }));
    let make = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(_)));
    let run = engine.start(document(&[
        r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [1, 0, 0], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "B", "position": [0, 1, 0], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "cccccccc", "type": "box", "v": 1, "name": "C", "position": [0, 0, 1], "length": 1, "width": 1, "height": 1}"#,
    ]));
    assert!(engine.kernel().wait_running(1, WAIT));
    // Let the recompute's other threads find nothing ready and wait, while the make is
    // held (a pause can only let a wrong wake-up pass, never fail a right one).
    std::thread::sleep(Duration::from_millis(100));
    make.release();
    let together = engine.kernel().wait_running(3, WAIT);
    hold.release();
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    assert!(together, "the three moves didn't run at once");
}

#[test]
fn models_are_equal_exactly_when_theyre_of_the_same_document_and_generation_and_came_to_the_same() {
    // Found by the milestone's mutation run: no test compared two models, so equality that
    // looked only at the document, or only at the features, passed every test. Two engines'
    // first models, of documents that differ only in a name and a note: every feature comes
    // to the same, in the same generation, but they're not of the same document. And one
    // engine's two models of one document, built in two generations: not equal either.
    let renamed = EXAMPLE
        .replace("\"Pin\"", "\"Dowel\"")
        .replace("Clearance for an M5 bolt", "Clearance for an M6 bolt");
    let first = published(&engine(2), checked(EXAMPLE));
    let same = published(&engine(2), checked(EXAMPLE));
    let other = published(&engine(2), checked(&renamed));
    assert_eq!(*same, *first);
    assert_eq!(other.generation(), first.generation());
    assert_eq!(other.features(), first.features());
    assert_eq!(other.bodies(), first.bodies());
    assert_eq!(other.errors().count(), 0);
    assert_ne!(*other, *first);
    let engine = engine(2);
    let earlier = published(&engine, checked(EXAMPLE));
    let later = published(&engine, checked(EXAMPLE));
    assert_ne!(earlier.generation(), later.generation());
    assert_eq!(later.features(), earlier.features());
    assert_ne!(*later, *earlier);
}

#[test]
fn a_key_debugs_as_its_hex() {
    // Found by the milestone's mutation run: a key's `Debug` (in every logged outcome) was
    // never read. It's `Key(...)` around its 64 hex digits, as it displays.
    let model = published(&engine(2), checked(EXAMPLE));
    let Some(FeatureOutcome::Built { key, .. }) = model.outcome(&id("k7f2qx3m")) else {
        panic!(
            "k7f2qx3m didn't build: {:?}",
            model.outcome(&id("k7f2qx3m"))
        )
    };
    let hex = key.to_string();
    assert_eq!(hex.len(), 64);
    assert!(
        hex.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(format!("{key:?}"), format!("Key({hex})"));
}

#[test]
fn engines_on_two_clients_of_one_kernel_at_once_never_supersede_each_other() {
    // M4.2 (MA2): two documents' engines rebuilding at once, each on a client of one kernel.
    // `B`'s five rebuilds advance only its own client, so `A`'s held box, of its first
    // generation, survives them all. On one client (or global generations), `B`'s first
    // rebuild supersedes `A`.
    let kernel = FakeKernel::new(2).unwrap();
    let first = Recompute::new(kernel.client());
    let second = Recompute::new(kernel.client());
    let hold = kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let a = first.start(checked(EXAMPLE));
    // The box is held from the moment it's in the call record.
    let deadline = std::time::Instant::now() + WAIT;
    while !kernel.calls().iter().any(|c| c.op.name() == "MakeBox") {
        assert!(
            std::time::Instant::now() < deadline,
            "the box never started"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let runs: Vec<_> = (1..=5)
        .map(|radius| {
            second.start(document(&[&format!(
                r#"{{"id": "s0000000", "type": "sphere", "v": 1, "name": "S", "position": [0, 0, 0], "radius": {radius}}}"#
            )]))
        })
        .collect();
    hold.release();
    let Ok(Ok(RunOutcome::Published(model))) = a.wait_timeout(WAIT) else {
        panic!("A wasn't published")
    };
    // 40 × 20 × 10 = 8000, in millionths.
    let block = model.fingerprint(&id("k7f2qx3m")).unwrap();
    assert_eq!(block.volume, Measure::from_millionths(8_000_000_000));
    assert_eq!(
        described(&model, "m3tq8v0z"),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
    // Each engine counts its own client's generations from 0: A's one rebuild is 1, B's
    // fifth is 5.
    assert_eq!(model.generation(), Generation(1));
    let mut outcomes: Vec<RunOutcome> = runs
        .into_iter()
        .map(|run| run.wait_timeout(WAIT).ok().unwrap().unwrap())
        .collect();
    let Some(RunOutcome::Published(last)) = outcomes.pop() else {
        panic!("B's last rebuild wasn't published")
    };
    assert_eq!(described(&last, "s0000000"), "sphere(5)");
    assert_eq!(last.generation(), Generation(5));
    // Its earlier ones were each published, or superseded by its own next.
    assert!(
        outcomes
            .iter()
            .all(|o| matches!(o, RunOutcome::Published(_) | RunOutcome::Superseded))
    );
}
