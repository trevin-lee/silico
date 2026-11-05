//! /break-it on `status.rebuild.features` (issue #367): each feature's state in a rebuild,
//! under a document of 50,000 features, a cancel landing while steps settle, a cancel
//! during a sketch's solve, and random timelines with random refusals. Expected values come
//! from hand counts and from a naive evaluator written here (each feature in timeline
//! order: a refused primitive failed or faulted, a boolean using one not built blocked,
//! anything else built), never from the recompute under test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::model;
use super::recompute;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use model::WAIT;
use proptest::prelude::*;
use recompute::{document, engine};
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{Progress, Rebuild, RebuildState, Revision};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Op, Primitive};
use silico_core::recompute::{
    FeatureStates, Recompute, RunOutcome, StepCount, StepCounter, StepState,
};
use silico_core::{Error, ErrorCode};

use StepState::{Blocked, Built, Failed, Faulted, Queued, Running};

/// Polls `progress` until `done` holds of it, for at most [`WAIT`].
#[track_caller]
fn until(progress: &StepCounter, done: impl Fn(&silico_core::recompute::StepCount) -> bool) {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let now = progress.now();
        if done(&now) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{now:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn states_of(features: &FeatureStates) -> Vec<StepState> {
    features.iter().map(|(_, state)| state).collect()
}

/// Whether a feature read `was` in one copy of a count may read `is` in a later one, by
/// hand from the decision: unsettled (queued, or running: a feature goes back to queued
/// between its steps) may become anything; built may yet fail its check, fault, or be
/// blocked; failed, faulted, and blocked are final. Copies skip states between them, so
/// this is the moves' closure.
fn may_follow(was: StepState, is: StepState) -> bool {
    was == is
        || matches!(was, Queued | Running)
        || (was == Built && matches!(is, Failed | Faulted | Blocked))
}

/// Over `copies` of a count, in the order they were taken: no feature goes back
/// ([`may_follow`]), and nothing reads running in a copy of a count that has ended.
fn never_back(copies: &[StepCount]) -> Result<(), String> {
    for pair in copies.windows(2) {
        let [before, after] = pair else { continue };
        for ((feature, was), (_, is)) in before.features.iter().zip(after.features.iter()) {
            if !may_follow(was, is) {
                return Err(format!("{feature} went from {was:?} to {is:?}"));
            }
        }
    }
    for copy in copies.iter().filter(|copy| copy.ended) {
        if let Some((feature, _)) = copy.features.iter().find(|(_, s)| *s == Running) {
            return Err(format!("{feature} running once its rebuild ended"));
        }
    }
    Ok(())
}

/// Copies of `progress`'s count, taken as fast as a reader can until it ends, the last
/// once it has: what `status` reads, read while the rebuild runs.
fn copied_until_ended(progress: &StepCounter) -> std::thread::JoinHandle<Vec<StepCount>> {
    let progress = progress.clone();
    std::thread::spawn(move || {
        let mut copies = Vec::new();
        loop {
            let now = progress.now();
            let ended = now.ended;
            copies.push(now);
            if ended {
                return copies;
            }
            std::thread::yield_now();
        }
    })
}

/// `n` unit boxes at the origin, `b0000000` on: one step, shared by all of them.
fn boxes(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            format!(
                r#"{{"id": "b{i:07}", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#
            )
        })
        .collect()
}

#[test]
fn fifty_thousand_features_are_each_listed_running_and_written_at_a_fixed_size_each() {
    // 50,000 identical boxes: one step, its key owned by all of them. Held, every one is
    // running; written, each is one line of the list, the same size for every feature
    // (8-character IDs, one state word), so a status grows linearly: by hand 49 bytes a
    // feature as the canonical layout lays it out at this depth (four spaces, the entry's 43
    // characters with "running", a comma and a newline; the last has no comma), about
    // 2.4 MB for the whole list (not the 50 KB the decision's residual guessed). Then
    // stopped: every one queued again, none left running.
    const N: usize = 50_000;
    let features = boxes(N);
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let engine = engine(2);
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let run = engine.start(document(&refs));
    let progress = run.progress();
    until(&progress, |count| {
        count.features.len() == N && count.features.iter().all(|(_, s)| s == Running)
    });
    let count = progress.now();
    assert_eq!((count.done, count.steps), (0, 1));
    let listed: Vec<&str> = count.features.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!((listed[0], listed[N - 1]), ("b0000000", "b0049999"));
    let rebuild = Rebuild {
        revision: Revision::OPENED,
        state: RebuildState::Running,
        progress: Some(Progress::from(&count)),
    };
    let text = write(&rebuild.to_wire(), Layout::Canonical).unwrap();
    let empty = Rebuild {
        revision: Revision::OPENED,
        state: RebuildState::Running,
        progress: Some(Progress {
            features: FeatureStates::default(),
            ..Progress::from(&count)
        }),
    };
    let none = write(&empty.to_wire(), Layout::Canonical).unwrap();
    assert!(
        text.contains("\n    {\"feature\": \"b0000000\", \"state\": \"running\"},\n"),
        "{}",
        &text[..300]
    );
    // An empty list is written `[]`, the rebuild on one line; a full one lays the rebuild
    // out a member to a line, by hand: `{\n` (2), `  "revision": 1,\n` (17),
    // `  "state": "running",\n` (22), `  "done": 0,\n` (13), `  "steps": 1,\n` (14),
    // `  "features": [\n` (16), the entries (49 each, the last with no comma), `  ]\n` (4),
    // and `}` (1): 88 bytes and 49 a feature.
    assert_eq!(
        none,
        r#"{"revision": 1, "state": "running", "done": 0, "steps": 1, "features": []}"#
    );
    assert_eq!(text.len(), 88 + N * 49);
    engine.cancel();
    assert!(matches!(run.wait(), Ok(RunOutcome::Superseded)));
    hold.release();
    let stopped = progress.now();
    assert!(stopped.ended);
    assert!(
        stopped.features.iter().all(|(_, s)| s == Queued),
        "{:?}",
        stopped.features.iter().find(|(_, s)| *s != Queued)
    );
}

/// `n` boxes, each its own step (its length `1 + i`), `k0000000` on.
fn distinct_boxes(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            format!(
                r#"{{"id": "k{i:07}", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": {}, "width": 1, "height": 1}}"#,
                i + 1
            )
        })
        .collect()
}

#[test]
fn a_cancel_landing_while_steps_settle_leaves_only_built_and_queued_and_built_is_true() {
    // 200 independent boxes, each its own step, on a fake of two threads; the rebuild
    // cancelled once at least `at` have settled, while the rest are settling. Whatever the
    // timing: nothing is left running, nothing is failed, faulted, or blocked by the
    // cancel (a superseded step isn't a failure), and every box said built was made by the
    // kernel (its length asked). The next rebuild of the same document builds them all,
    // asking again only for those not built.
    const N: usize = 200;
    let texts = distinct_boxes(N);
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    for at in [1, 37, 120, 199] {
        let engine = engine(2);
        let run = engine.start(document(&refs));
        let progress = run.progress();
        until(&progress, |count| count.done >= at || count.ended);
        engine.cancel();
        let outcome = run.wait();
        let stopped = progress.now();
        assert!(stopped.ended);
        let made: std::collections::BTreeSet<u64> = engine
            .kernel()
            .calls()
            .iter()
            .filter_map(|call| match &call.op {
                Op::MakePrimitive(Primitive::Box { dx, .. }) => Some(*dx as u64),
                _ => None,
            })
            .collect();
        for (i, (feature, state)) in stopped.features.iter().enumerate() {
            match state {
                Queued => {}
                Built => assert!(
                    made.contains(&(i as u64 + 1)),
                    "{feature} built, never made"
                ),
                other => panic!("{feature} {other:?} after a cancel at {at}"),
            }
        }
        if matches!(outcome, Ok(RunOutcome::Published(_))) {
            // The cancel came after the end: all built.
            assert!(stopped.features.iter().all(|(_, s)| s == Built));
        }
        let built_before = stopped.features.iter().filter(|(_, s)| *s == Built).count();
        let asked = engine.kernel().calls().len();
        let again = engine.start(document(&refs));
        let progress = again.progress();
        assert!(matches!(again.wait(), Ok(RunOutcome::Published(_))));
        assert!(progress.now().features.iter().all(|(_, s)| s == Built));
        let boxes_again = engine.kernel().calls()[asked..]
            .iter()
            .filter(|call| matches!(call.op, Op::MakePrimitive(Primitive::Box { .. })))
            .count();
        assert!(boxes_again <= N - built_before, "{boxes_again} asked again");
    }
}

#[test]
fn a_cancel_during_a_sketchs_solve_leaves_it_queued_not_running() {
    // A square sketch nothing sweeps, its solve held, and a box beside it. Cancelled: the
    // sketch isn't left running (its solve never settled), and the next rebuild solves it.
    let square = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Square", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 10, "y": 0, "construction": false}, {"id": "pa000003", "x": 10, "y": 10, "construction": false}, {"id": "pa000004", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
    let block = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;
    let sketches = FakeSketches::new();
    sketches.hold();
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(sketches.clone());
    let run = engine.start(document(&[square, block]));
    let progress = run.progress();
    assert!(sketches.wait_held(WAIT));
    until(&progress, |count| {
        states_of(&count.features) == [Running, Built]
    });
    engine.cancel();
    sketches.release();
    assert!(matches!(run.wait(), Ok(RunOutcome::Superseded)));
    assert_eq!(states_of(&progress.now().features), [Queued, Built]);
    let again = engine.start(document(&[square, block]));
    let progress = again.progress();
    assert!(matches!(again.wait(), Ok(RunOutcome::Published(_))));
    assert_eq!(states_of(&progress.now().features), [Built, Built]);
}

#[test]
fn a_status_read_on_another_thread_while_steps_settle_never_waits_or_tears() {
    // 200 boxes settling on two threads, and a reader copying the count as fast as it can:
    // every copy lists every box, in order, and `built` never goes back to `queued` or
    // `running` for a box, copy to copy (a box's one step, once made, stays made).
    const N: usize = 200;
    let texts = distinct_boxes(N);
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let engine = engine(2);
    let run = engine.start(document(&refs));
    let progress = run.progress();
    let reads = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut last = vec![Queued; N];
            loop {
                let now = progress.now();
                let states = states_of(&now.features);
                assert_eq!(states.len(), N);
                for (i, (was, is)) in last.iter().zip(&states).enumerate() {
                    assert!(
                        !(*was == Built && *is != Built),
                        "k{i:07} went from built to {is:?}"
                    );
                }
                last = states;
                reads.fetch_add(1, Ordering::Relaxed);
                if now.ended {
                    break;
                }
            }
        });
    });
    assert!(matches!(run.wait(), Ok(RunOutcome::Published(_))));
    assert!(reads.load(Ordering::Relaxed) >= 1);
    assert!(progress.now().features.iter().all(|(_, s)| s == Built));
}

/// A feature of a random timeline: a box (refused, crashed, or made), or a fuse or cut of
/// two earlier features.
#[derive(Debug, Clone)]
enum Made {
    Box(Fate),
    Boolean { cut: bool, base: usize, tool: usize },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Fate {
    Made,
    Refused,
    Crashed,
}

fn timeline() -> impl Strategy<Value = Vec<Made>> {
    let fate = prop_oneof![
        6 => Just(Fate::Made),
        1 => Just(Fate::Refused),
        1 => Just(Fate::Crashed)
    ];
    (
        2usize..9,
        proptest::collection::vec((fate, any::<bool>(), any::<u16>(), any::<u16>(), 0u8..3), 9),
    )
        .prop_map(|(n, picks)| {
            let mut made = Vec::with_capacity(n);
            for (i, (fate, cut, a, b, kind)) in picks.into_iter().take(n).enumerate() {
                if i >= 2 && kind == 0 {
                    let base = usize::from(a) % i;
                    let mut tool = usize::from(b) % i;
                    if tool == base {
                        tool = (base + 1) % i;
                    }
                    made.push(Made::Boolean { cut, base, tool });
                } else {
                    made.push(Made::Box(fate));
                }
            }
            made
        })
}

/// The naive evaluator: each feature in order, a box as its fate, a boolean blocked if
/// either input isn't built.
fn expected(timeline: &[Made]) -> Vec<StepState> {
    let mut states: Vec<StepState> = Vec::new();
    for made in timeline {
        let state = match made {
            Made::Box(Fate::Made) => Built,
            Made::Box(Fate::Refused) => Failed,
            Made::Box(Fate::Crashed) => Faulted,
            Made::Boolean { base, tool, .. } => {
                if states[*base] == Built && states[*tool] == Built {
                    Built
                } else {
                    Blocked
                }
            }
        };
        states.push(state);
    }
    states
}

fn texts(timeline: &[Made]) -> Vec<String> {
    timeline
        .iter()
        .enumerate()
        .map(|(i, made)| match made {
            // Each box its own height, so its fate is found by it; apart along x, so no
            // two touch.
            Made::Box(_) => format!(
                r#"{{"id": "f{i:07}", "type": "box", "v": 1, "name": "B", "position": [{}, 0, 0], "length": 1, "width": 1, "height": {}}}"#,
                i * 10,
                i + 1
            ),
            Made::Boolean { cut, base, tool } => format!(
                r#"{{"id": "f{i:07}", "type": "{}", "v": 1, "name": "O", "base": "f{base:07}", "tool": "f{tool:07}"}}"#,
                if *cut { "cut" } else { "fuse" }
            ),
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    #[test]
    fn a_finished_rebuilds_states_are_the_naive_evaluators(timeline in timeline()) {
        // Each feature consumed at most once is what the gate asks (a solid used twice is
        // refused): such timelines are skipped, not built.
        let texts = texts(&timeline);
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let text = format!(
            "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}]}}",
            refs.join(", ")
        );
        let Ok(read) = silico_core::document::read(text.as_bytes()) else {
            return Err(TestCaseError::reject("unread"));
        };
        let Ok(checked) = silico_core::document::Checked::new(read) else {
            return Err(TestCaseError::reject("gate"));
        };
        let fates: Vec<(u64, Fate)> = timeline
            .iter()
            .enumerate()
            .filter_map(|(i, made)| match made {
                Made::Box(fate) => Some((i as u64 + 1, *fate)),
                Made::Boolean { .. } => None,
            })
            .collect();
        let fates = Arc::new(fates);
        let kernel = FakeKernel::new(2).unwrap();
        let by_height = Arc::clone(&fates);
        kernel.refuse(move |op| {
            let Op::MakePrimitive(Primitive::Box { dz, .. }) = op else {
                return None;
            };
            match by_height.iter().find(|(h, _)| *h as f64 == *dz)?.1 {
                Fate::Made => None,
                Fate::Refused => Some(Error::new(ErrorCode::KernelFailed, "MakeBox failed")),
                Fate::Crashed => Some(Error::new(ErrorCode::WorkerCrashed, "crashed")),
            }
        });
        let engine = Recompute::new(kernel);
        let run = engine.start(checked);
        let progress = run.progress();
        let copies = copied_until_ended(&progress);
        prop_assert!(matches!(run.wait(), Ok(RunOutcome::Published(_))));
        let copies = copies.join().unwrap();
        prop_assert!(copies.last().is_some_and(|last| last.ended));
        // Every copy read on the way, not only the end (which the model's outcomes set):
        // nothing went back, and nothing ran once it ended.
        if let Err(why) = never_back(&copies) {
            return Err(TestCaseError::fail(why));
        }
        let now = progress.now();
        let listed: Vec<String> = now.features.iter().map(|(f, _)| f.as_str().to_owned()).collect();
        let ids: Vec<String> = (0..timeline.len()).map(|i| format!("f{i:07}")).collect();
        prop_assert_eq!(listed, ids);
        prop_assert_eq!(states_of(&now.features), expected(&timeline));
    }
}

/// An engine on the fake kernel and the fake sketch solver.
fn faked() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

/// A kernel whose submit panics on spheres: a bug under a recompute's step.
struct PanicsOnSpheres(FakeKernel);

impl silico_core::kernel::Kernel for PanicsOnSpheres {
    fn identity(&self) -> &str {
        self.0.identity()
    }

    fn generation(&self) -> silico_core::kernel::Generation {
        self.0.generation()
    }

    fn advance(&self, generation: silico_core::kernel::Generation) {
        self.0.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: silico_core::kernel::Generation,
        request: silico_core::kernel::KernelRequest<T>,
    ) -> silico_core::pool::Pending<T> {
        if request.name() == "MakeSphere" {
            panic!("a bug under a recompute step");
        }
        self.0.submit(generation, request)
    }
}

const BLOCK: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;
const BALL: &str = r#"{"id": "q0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}"#;
const DENT: &str = r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "Dent", "base": "k7f2qx3m", "tool": "q0000001"}"#;
const SQUARE: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Square", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 10, "y": 0, "construction": false}, {"id": "pa000003", "x": 10, "y": 10, "construction": false}, {"id": "pa000004", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
const PAD: &str = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;

#[test]
fn a_step_that_panics_is_faulted_and_what_uses_it_blocked_not_left_queued() {
    // The ball's step panics (a bug): the rebuild ends faulted, with the panic's error. The
    // ball is faulted, the cut that uses it blocked, as for a worker's crash; not left
    // queued under a faulted rebuild, as though Rebuild could resume them. The block, on a
    // branch of its own, is built or (stopped before it settled) queued; never running.
    let engine = Recompute::new(PanicsOnSpheres(FakeKernel::new(2).unwrap()));
    let run = engine.start(document(&[BLOCK, BALL, DENT]));
    let progress = run.progress();
    let error = run.wait_timeout(WAIT).ok().unwrap().unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal, "{error}");
    let ended = progress.now();
    assert!(ended.ended);
    let states = states_of(&ended.features);
    assert!(matches!(states[0], Built | Queued), "{states:?}");
    assert_eq!(states[1..], [Faulted, Blocked]);
}

/// A sketch solver with a bug: every solve panics.
struct SolvePanics;

impl silico_core::sketch::SketchSolver for SolvePanics {
    fn solve(
        &mut self,
        _: silico_protocol::SketchSystem,
    ) -> Result<silico_protocol::Solved, Error> {
        panic!("a bug in the solver")
    }
}

#[test]
fn a_sketchs_solve_that_panics_is_faulted_and_what_sweeps_it_blocked() {
    // Padded, the square is solved before any plan; alone, beside the block's step. Either
    // way its solve panics (a bug): the rebuild ends faulted; the square is faulted, not
    // left queued, and the pad that sweeps it blocked; the block, alone, built or queued.
    let swept = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(SolvePanics);
    let run = swept.start(document(&[SQUARE, PAD, BLOCK]));
    let progress = run.progress();
    let error = run.wait_timeout(WAIT).ok().unwrap().unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal, "{error}");
    assert_eq!(
        states_of(&progress.now().features),
        [Faulted, Blocked, Queued]
    );
    let beside = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(SolvePanics);
    let run = beside.start(document(&[SQUARE, BLOCK]));
    let progress = run.progress();
    let error = run.wait_timeout(WAIT).ok().unwrap().unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal, "{error}");
    let states = states_of(&progress.now().features);
    assert_eq!(states[0], Faulted, "{states:?}");
    assert!(matches!(states[1], Built | Queued), "{states:?}");
}

#[test]
fn a_pad_whose_bases_solid_cant_be_measured_is_failed_while_the_rebuild_runs() {
    // The square padded onto the block, and a ball on its own branch, held, keeping the
    // rebuild running. Only the block's measure is refused: the pad's own solid is
    // measured, its base's isn't, so its check can't be made, and it fails (as the model
    // will say, with the base's error), while the ball still runs; at the end, the same.
    let on_block = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "base": "k7f2qx3m", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let engine = faked();
    engine.kernel().refuse(|op| match op {
        Op::Summarize { shape }
            if FakeKernel::describe(shape).is_some_and(|made| made.starts_with("box(")) =>
        {
            Some(Error::new(
                ErrorCode::KernelFailed,
                "the base's Summarize failed",
            ))
        }
        _ => None,
    });
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
    let run = engine.start(document(&[SQUARE, BLOCK, on_block, BALL]));
    let progress = run.progress();
    until(&progress, |count| {
        states_of(&count.features) == [Built, Built, Failed, Running]
    });
    assert!(!progress.now().ended);
    hold.release();
    let Ok(RunOutcome::Published(model)) = run.wait() else {
        panic!("not published");
    };
    let pad = silico_core::document::FeatureId::new("p0000001").unwrap();
    let Some(silico_core::recompute::FeatureOutcome::Failed(error)) = model.outcome(&pad) else {
        panic!("{:?}", model.outcome(&pad));
    };
    assert!(
        error.message().contains("the base's Summarize failed"),
        "{error:?}"
    );
    assert_eq!(
        states_of(&progress.now().features),
        [Built, Built, Failed, Built]
    );
}

#[test]
fn a_pad_refused_before_any_step_waits_queued_for_its_base_then_fails_or_is_blocked() {
    // A segment, which makes no profile, padded onto the block: the pad is refused at its
    // plan, before any step, but the model says blocked if its base doesn't build, failed
    // if it does. So while the block is held the pad is queued, not failed (it could yet be
    // blocked); the block built, the pad fails at once; the block refused, the pad is
    // blocked, never failed on the way. A ball on its own branch, held, keeps the rebuild
    // running while that's read.
    let segment = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Segment", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
    let on_block = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "base": "k7f2qx3m", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    for block_builds in [true, false] {
        let engine = faked();
        if !block_builds {
            engine.kernel().refuse(|op| {
                matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
                    .then(|| Error::new(ErrorCode::KernelFailed, "MakeBox failed"))
            });
        }
        let block = engine
            .kernel()
            .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
        let ball = engine
            .kernel()
            .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
        let run = engine.start(document(&[segment, BLOCK, on_block, BALL]));
        let progress = run.progress();
        let copies = copied_until_ended(&progress);
        until(&progress, |count| {
            states_of(&count.features) == [Built, Running, Queued, Running]
        });
        block.release();
        let (pad, want) = if block_builds {
            (Failed, [Built, Built, Failed, Built])
        } else {
            (Blocked, [Built, Failed, Blocked, Built])
        };
        until(&progress, |count| {
            states_of(&count.features) == [Built, want[1], pad, Running]
        });
        ball.release();
        let Ok(RunOutcome::Published(model)) = run.wait() else {
            panic!("not published");
        };
        never_back(&copies.join().unwrap()).unwrap();
        let id = silico_core::document::FeatureId::new("p0000001").unwrap();
        let said = model.outcome(&id);
        if block_builds {
            assert!(
                matches!(
                    said,
                    Some(silico_core::recompute::FeatureOutcome::Failed(_))
                ),
                "{said:?}"
            );
        } else {
            assert!(
                matches!(
                    said,
                    Some(silico_core::recompute::FeatureOutcome::Blocked { .. })
                ),
                "{said:?}"
            );
        }
        assert_eq!(states_of(&progress.now().features), want);
    }
}

#[test]
fn a_cut_whose_step_ran_on_after_its_base_failed_its_check_stays_blocked() {
    // The square padded onto the block, the pad cut by a pin, and a ball on its own branch,
    // held, keeping the rebuild running. The block's measure is refused, so the pad, once
    // its own solid is measured, fails its check. The cut's step starts as soon as the
    // pad's solid is made, before that: held there, then the pad's measure let go, the pad
    // fails and the cut, running, is blocked; then the cut's step let go, it's made, and
    // the cut stays blocked (what it's made from failed), never built on the way.
    let on_block = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "base": "k7f2qx3m", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let pin = r#"{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 20}"#;
    let hole = r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "Hole", "base": "p0000001", "tool": "c0000001"}"#;
    let made = |op: &Op, prefix: &str| match op {
        Op::Summarize { shape } => {
            FakeKernel::describe(shape).is_some_and(|d| d.starts_with(prefix))
        }
        _ => false,
    };
    // Four workers: the ball and the cut each hold one, and the measures need theirs.
    let engine = Recompute::new(FakeKernel::new(4).unwrap()).with_sketches(FakeSketches::new());
    engine.kernel().refuse(move |op| {
        made(op, "box(").then(|| Error::new(ErrorCode::KernelFailed, "the base's Summarize failed"))
    });
    let measure = engine.kernel().hold(move |op| made(op, "fuse("));
    let cut = engine.kernel().hold(|op| {
        matches!(
            op,
            Op::Boolean {
                op: silico_core::kernel::BooleanOp::Cut,
                ..
            }
        )
    });
    let ball = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
    let run = engine.start(document(&[SQUARE, BLOCK, on_block, pin, hole, BALL]));
    let progress = run.progress();
    let copies = copied_until_ended(&progress);
    // The pad made, its measure held; the cut's step held; the ball held.
    until(&progress, |count| {
        states_of(&count.features) == [Built, Built, Built, Built, Running, Running]
    });
    let deadline = std::time::Instant::now() + WAIT;
    while !engine
        .kernel()
        .calls()
        .iter()
        .any(|call| made(&call.op, "fuse("))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the pad's measure never started"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    measure.release();
    until(&progress, |count| {
        states_of(&count.features) == [Built, Built, Failed, Built, Blocked, Running]
    });
    cut.release();
    // Every step settled but the ball's: the cut's made.
    until(&progress, |count| {
        count.steps > 0 && count.done + 1 == count.steps
    });
    assert_eq!(
        states_of(&progress.now().features),
        [Built, Built, Failed, Built, Blocked, Running]
    );
    ball.release();
    assert!(matches!(run.wait(), Ok(RunOutcome::Published(_))));
    never_back(&copies.join().unwrap()).unwrap();
    assert_eq!(
        states_of(&progress.now().features),
        [Built, Built, Failed, Built, Blocked, Built]
    );
}

/// Whether `op` measures the pad's solid: the fake's fuse of its prism onto the block.
fn measures_the_pad(op: &Op) -> bool {
    match op {
        Op::Summarize { shape } => {
            FakeKernel::describe(shape).is_some_and(|d| d.starts_with("fuse("))
        }
        _ => false,
    }
}

/// Waits until `engine`'s kernel has been asked to measure the pad's solid.
#[track_caller]
fn until_the_pad_is_measured(engine: &Recompute<FakeKernel>) {
    let deadline = std::time::Instant::now() + WAIT;
    while !engine
        .kernel()
        .calls()
        .iter()
        .any(|call| measures_the_pad(&call.op))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the pad's measure never started"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

const ON_BLOCK: &str = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "base": "k7f2qx3m", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;

#[test]
fn a_pad_refused_by_its_plan_waits_queued_until_its_bases_check_is_decided() {
    // The square padded onto the block (p1), whose check needs its solid's measure, held;
    // a segment, which makes no profile, padded onto p1 (p2): refused by its plan. The
    // model says p2 is blocked if p1 fails its check, failed if it passes. So once p1 is
    // built, its measure held, p2 is queued, not failed (it could yet be blocked); then
    // p1's measure refused, p1 fails and p2 is blocked, never failed on the way; or p1's
    // measure let go, its check passes and p2 fails. Before, p2 failed as p1 was built. A
    // ball on its own branch, held, keeps the rebuild running while the outcome is read.
    let segment = r#"{"id": "s0000002", "type": "sketch", "v": 1, "name": "Segment", "plane": "xy", "points": [{"id": "pb000001", "x": 0, "y": 0, "construction": false}, {"id": "pb000002", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
    let refused = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "Refused", "base": "p0000001", "sketch": "s0000002", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    for check_fails in [true, false] {
        let engine = faked();
        if check_fails {
            engine.kernel().refuse(|op| {
                measures_the_pad(op)
                    .then(|| Error::new(ErrorCode::KernelFailed, "the pad's Summarize failed"))
            });
        }
        let measure = engine.kernel().hold(measures_the_pad);
        let ball = engine
            .kernel()
            .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
        let run = engine.start(document(&[SQUARE, BLOCK, ON_BLOCK, segment, refused, BALL]));
        let progress = run.progress();
        let copies = copied_until_ended(&progress);
        until(&progress, |count| states_of(&count.features)[2] == Built);
        until_the_pad_is_measured(&engine);
        assert_eq!(
            states_of(&progress.now().features),
            [Built, Built, Built, Built, Queued, Running]
        );
        measure.release();
        // Said while the rebuild runs, not only by its model at the end.
        let mid = if check_fails {
            [Built, Built, Failed, Built, Blocked, Running]
        } else {
            [Built, Built, Built, Built, Failed, Running]
        };
        until(&progress, |count| states_of(&count.features) == mid);
        assert!(!progress.now().ended);
        ball.release();
        let Ok(RunOutcome::Published(model)) = run.wait() else {
            panic!("not published");
        };
        let copies = copies.join().unwrap();
        never_back(&copies).unwrap();
        let id = silico_core::document::FeatureId::new("p0000002").unwrap();
        let said = model.outcome(&id);
        let want = if check_fails {
            assert!(
                matches!(
                    said,
                    Some(silico_core::recompute::FeatureOutcome::Blocked { .. })
                ),
                "{said:?}"
            );
            assert!(
                copies.iter().all(|c| c.features.get(&id) != Some(Failed)),
                "the refused pad read failed on its way to blocked"
            );
            [Built, Built, Failed, Built, Blocked, Built]
        } else {
            assert!(
                matches!(
                    said,
                    Some(silico_core::recompute::FeatureOutcome::Failed(_))
                ),
                "{said:?}"
            );
            [Built, Built, Built, Built, Failed, Built]
        };
        assert_eq!(states_of(&progress.now().features), want);
    }
}

#[test]
fn a_cut_whose_own_step_fails_waits_queued_until_its_bases_check_is_decided() {
    // The square padded onto the block (p1), its measure held, and a pin cut from p1, the
    // cut's step refused by the kernel. The cut's step runs as soon as p1's solid is made,
    // before p1's check is decided, and fails. The model says the cut is blocked if p1 fails
    // its check, whatever the cut's own failure; failed if p1 passes. So with every step
    // settled and p1's measure held, the cut is queued, its failure held; then p1's measure
    // refused, p1 fails and the cut is blocked, never failed on the way; or p1's measure let
    // go, the cut fails. Before, the cut read failed, and was failed until the model said
    // blocked at the end. A ball on its own branch, held, keeps the rebuild running while
    // the outcome is read.
    let pin = r#"{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 20}"#;
    let hole = r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "Hole", "base": "p0000001", "tool": "c0000001"}"#;
    for check_fails in [true, false] {
        let engine = faked();
        engine.kernel().refuse(move |op| {
            if check_fails && measures_the_pad(op) {
                return Some(Error::new(
                    ErrorCode::KernelFailed,
                    "the pad's Summarize failed",
                ));
            }
            matches!(
                op,
                Op::Boolean {
                    op: silico_core::kernel::BooleanOp::Cut,
                    ..
                }
            )
            .then(|| Error::new(ErrorCode::KernelFailed, "the cut failed"))
        });
        let measure = engine.kernel().hold(measures_the_pad);
        let ball = engine
            .kernel()
            .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
        let run = engine.start(document(&[SQUARE, BLOCK, ON_BLOCK, pin, hole, BALL]));
        let progress = run.progress();
        let copies = copied_until_ended(&progress);
        // Every step settled but the ball's, the cut's refused among them; the pad's measure
        // held.
        until(&progress, |count| {
            count.steps > 0 && count.done + 1 == count.steps
        });
        until_the_pad_is_measured(&engine);
        assert_eq!(
            states_of(&progress.now().features),
            [Built, Built, Built, Built, Queued, Running]
        );
        measure.release();
        // Said while the rebuild runs, not only by its model at the end.
        let mid = if check_fails {
            [Built, Built, Failed, Built, Blocked, Running]
        } else {
            [Built, Built, Built, Built, Failed, Running]
        };
        until(&progress, |count| states_of(&count.features) == mid);
        assert!(!progress.now().ended);
        ball.release();
        let Ok(RunOutcome::Published(model)) = run.wait() else {
            panic!("not published");
        };
        let copies = copies.join().unwrap();
        never_back(&copies).unwrap();
        let id = silico_core::document::FeatureId::new("m0000001").unwrap();
        let said = model.outcome(&id);
        let want = if check_fails {
            assert!(
                matches!(
                    said,
                    Some(silico_core::recompute::FeatureOutcome::Blocked { .. })
                ),
                "{said:?}"
            );
            assert!(
                copies.iter().all(|c| c.features.get(&id) != Some(Failed)),
                "the cut read failed on its way to blocked"
            );
            [Built, Built, Failed, Built, Blocked, Built]
        } else {
            assert!(
                matches!(
                    said,
                    Some(silico_core::recompute::FeatureOutcome::Failed(_))
                ),
                "{said:?}"
            );
            [Built, Built, Built, Built, Failed, Built]
        };
        assert_eq!(states_of(&progress.now().features), want);
    }
}

/// A kernel whose submit panics on cuts: a bug under a recompute's step.
struct PanicsOnCuts(FakeKernel);

impl silico_core::kernel::Kernel for PanicsOnCuts {
    fn identity(&self) -> &str {
        self.0.identity()
    }

    fn generation(&self) -> silico_core::kernel::Generation {
        self.0.generation()
    }

    fn advance(&self, generation: silico_core::kernel::Generation) {
        self.0.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: silico_core::kernel::Generation,
        request: silico_core::kernel::KernelRequest<T>,
    ) -> silico_core::pool::Pending<T> {
        if request.name() == "Cut" {
            panic!("a bug under a recompute step");
        }
        self.0.submit(generation, request)
    }
}

#[test]
fn a_step_that_panics_on_a_base_whose_check_is_undecided_is_faulted_at_once() {
    // A failure is held while what it uses may yet fail its check; a panic's isn't: the
    // recompute ends with the panic's error, and no model says otherwise. The square padded
    // onto the block, its measure held, and a pin cut from the pad, the cut's step
    // panicking: the cut is faulted as it panics, its base's check undecided, not left
    // queued under a faulted rebuild.
    let pin = r#"{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 20}"#;
    let hole = r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "Hole", "base": "p0000001", "tool": "c0000001"}"#;
    let fake = FakeKernel::new(2).unwrap();
    let measure = fake.hold(measures_the_pad);
    let engine = Recompute::new(PanicsOnCuts(fake)).with_sketches(FakeSketches::new());
    let run = engine.start(document(&[SQUARE, BLOCK, ON_BLOCK, pin, hole]));
    let progress = run.progress();
    until(&progress, |count| states_of(&count.features)[4] == Faulted);
    assert_eq!(
        states_of(&progress.now().features),
        [Built, Built, Built, Built, Faulted]
    );
    measure.release();
    let error = run.wait_timeout(WAIT).ok().unwrap().unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal, "{error}");
    assert_eq!(states_of(&progress.now().features)[4], Faulted);
}

/// Whether `op` is a boolean cut.
fn is_cut(op: &Op) -> bool {
    matches!(
        op,
        Op::Boolean {
            op: silico_core::kernel::BooleanOp::Cut,
            ..
        }
    )
}

/// How many of `engine`'s kernel's requests so far `picks` picks.
fn asked(engine: &Recompute<FakeKernel>, picks: impl Fn(&Op) -> bool) -> usize {
    engine
        .kernel()
        .calls()
        .iter()
        .filter(|call| picks(&call.op))
        .count()
}

/// Waits until `engine`'s kernel has been asked more than `before` requests `picks` picks.
#[track_caller]
fn until_asked(engine: &Recompute<FakeKernel>, before: usize, picks: impl Fn(&Op) -> bool) {
    let deadline = std::time::Instant::now() + WAIT;
    while asked(engine, &picks) <= before {
        assert!(std::time::Instant::now() < deadline, "never asked");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_failure_kept_from_the_last_rebuild_on_firm_inputs_is_said_at_the_plan() {
    // The block fused with a pin (the fuse's check, one solid, decided by its result), and
    // a second pin cut from the fuse, the kernel refusing every cut. Rebuilt once: the cut
    // failed, its failure kept (a kernel refusal follows from its values). Rebuilt again
    // with a ball on its own branch, held: everything else is kept, so the plan finds every
    // state at once. By hand: the block and both pins built, with nothing they use, so
    // firm; the fuse built, its check passed by its kept result (one solid), all it uses
    // firm, so firm; the cut's own failure, all it uses firm, is said at the plan: failed,
    // not queued (it can't be blocked: nothing it uses can fail now), while the ball runs.
    let pin = r#"{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 20}"#;
    let fuse = r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Fused", "base": "k7f2qx3m", "tool": "c0000001"}"#;
    let pin2 = r#"{"id": "c0000002", "type": "cylinder", "v": 1, "name": "Pin2", "position": [5, 5, -1], "radius": 1, "height": 20}"#;
    let hole = r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "Hole", "base": "f0000001", "tool": "c0000002"}"#;
    let engine = Recompute::new(FakeKernel::new(4).unwrap()).with_sketches(FakeSketches::new());
    engine
        .kernel()
        .refuse(|op| is_cut(op).then(|| Error::new(ErrorCode::KernelFailed, "the cut failed")));
    let first = engine.start(document(&[BLOCK, pin, fuse, pin2, hole]));
    let first_progress = first.progress();
    assert!(matches!(first.wait(), Ok(RunOutcome::Published(_))));
    assert_eq!(
        states_of(&first_progress.now().features),
        [Built, Built, Built, Built, Failed]
    );
    assert_eq!(asked(&engine, is_cut), 1);
    let ball = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
    let run = engine.start(document(&[BLOCK, pin, fuse, pin2, hole, BALL]));
    let progress = run.progress();
    let copies = copied_until_ended(&progress);
    until(&progress, |count| states_of(&count.features)[5] == Running);
    assert_eq!(
        states_of(&progress.now().features),
        [Built, Built, Built, Built, Failed, Running]
    );
    assert!(!progress.now().ended);
    ball.release();
    let Ok(RunOutcome::Published(model)) = run.wait() else {
        panic!("not published");
    };
    never_back(&copies.join().unwrap()).unwrap();
    // The cut's failure was kept, not asked again.
    assert_eq!(asked(&engine, is_cut), 1);
    let id = silico_core::document::FeatureId::new("m0000001").unwrap();
    assert!(
        matches!(
            model.outcome(&id),
            Some(silico_core::recompute::FeatureOutcome::Failed(_))
        ),
        "{:?}",
        model.outcome(&id)
    );
    assert_eq!(
        states_of(&progress.now().features),
        [Built, Built, Built, Built, Failed, Built]
    );
}

#[test]
fn a_refused_pad_on_a_cut_waits_queued_while_what_the_cut_uses_is_unchecked() {
    // The square padded onto the block (p1), its measure held; a pin cut from p1 (m1),
    // which the kernel makes; and a segment, which makes no profile, padded onto m1 (p2):
    // refused by its plan. The model says p2 is blocked if anything it's made from fails
    // its check, failed if not. m1's step and its own check (one solid) pass while p1's
    // check waits on p1's measure: m1 is built, but not firm, since what it uses isn't,
    // so p2 is queued, not failed. Then p1's measure let go, p1's check passes, m1 is
    // firm, and p2 fails, while a ball on its own branch, held, keeps the rebuild running.
    let pin = r#"{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 20}"#;
    let hole = r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "Hole", "base": "p0000001", "tool": "c0000001"}"#;
    let segment = r#"{"id": "s0000002", "type": "sketch", "v": 1, "name": "Segment", "plane": "xy", "points": [{"id": "pb000001", "x": 0, "y": 0, "construction": false}, {"id": "pb000002", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
    let refused = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "Refused", "base": "m0000001", "sketch": "s0000002", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    // Four workers: the ball and p1's measure each hold one, and the rest need theirs.
    let engine = Recompute::new(FakeKernel::new(4).unwrap()).with_sketches(FakeSketches::new());
    let measure = engine.kernel().hold(measures_the_pad);
    let ball = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
    let run = engine.start(document(&[
        SQUARE, BLOCK, ON_BLOCK, pin, hole, segment, refused, BALL,
    ]));
    let progress = run.progress();
    let copies = copied_until_ended(&progress);
    // m1's own measure is asked once its check is judged: by then, all m1 says is said.
    let measures_the_cut = |op: &Op| match op {
        Op::Summarize { shape } => {
            FakeKernel::describe(shape).is_some_and(|d| d.starts_with("cut("))
        }
        _ => false,
    };
    until_asked(&engine, 0, measures_the_cut);
    until_the_pad_is_measured(&engine);
    assert_eq!(
        states_of(&progress.now().features),
        [Built, Built, Built, Built, Built, Built, Queued, Running]
    );
    measure.release();
    until(&progress, |count| {
        states_of(&count.features) == [Built, Built, Built, Built, Built, Built, Failed, Running]
    });
    assert!(!progress.now().ended);
    ball.release();
    let Ok(RunOutcome::Published(model)) = run.wait() else {
        panic!("not published");
    };
    let copies = copies.join().unwrap();
    never_back(&copies).unwrap();
    let id = silico_core::document::FeatureId::new("p0000002").unwrap();
    assert!(
        matches!(
            model.outcome(&id),
            Some(silico_core::recompute::FeatureOutcome::Failed(_))
        ),
        "{:?}",
        model.outcome(&id)
    );
    assert_eq!(
        states_of(&progress.now().features),
        [Built, Built, Built, Built, Built, Built, Failed, Built]
    );
}

#[test]
fn a_step_doomed_by_a_kept_failure_still_held_is_blocked_at_the_plan() {
    // The square padded onto the block (p1); a pin cut from p1 (m1), the kernel refusing
    // every cut; and a second pin cut from m1 (m2). Rebuilt once with p1's measure crashing
    // its worker: m1's failure is kept (a kernel refusal follows from its values), m2 never
    // ran, and p1's solid is kept unmeasured (a crash doesn't follow from them). Rebuilt
    // again, p1's measure held: the plan finds p1 built but its check undecided, so m1's
    // kept failure is held, m1 queued (the model would say blocked if p1 failed its check).
    // m2's step can't run, since m1's failed: by hand, m2 is blocked at the plan, whatever
    // m1 comes to, and never queued. Let go, p1's check passes, m1 fails, m2 stays blocked.
    let pin = r#"{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 20}"#;
    let hole = r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "Hole", "base": "p0000001", "tool": "c0000001"}"#;
    let pin2 = r#"{"id": "c0000002", "type": "cylinder", "v": 1, "name": "Pin2", "position": [5, 5, -1], "radius": 1, "height": 20}"#;
    let second = r#"{"id": "m0000002", "type": "cut", "v": 1, "name": "Second", "base": "m0000001", "tool": "c0000002"}"#;
    let features = [SQUARE, BLOCK, ON_BLOCK, pin, hole, pin2, second];
    let crashing = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let engine = Recompute::new(FakeKernel::new(4).unwrap()).with_sketches(FakeSketches::new());
    let crashes = Arc::clone(&crashing);
    engine.kernel().refuse(move |op| {
        if measures_the_pad(op) && crashes.load(Ordering::SeqCst) {
            return Some(Error::new(
                ErrorCode::WorkerCrashed,
                "the pad's Summarize crashed",
            ));
        }
        is_cut(op).then(|| Error::new(ErrorCode::KernelFailed, "the cut failed"))
    });
    let first = engine.start(document(&features));
    assert!(matches!(first.wait(), Ok(RunOutcome::Published(_))));
    assert_eq!(asked(&engine, is_cut), 1);
    crashing.store(false, Ordering::SeqCst);
    let measured_before = asked(&engine, measures_the_pad);
    let measure = engine.kernel().hold(measures_the_pad);
    let run = engine.start(document(&features));
    let progress = run.progress();
    let copies = copied_until_ended(&progress);
    until_asked(&engine, measured_before, measures_the_pad);
    assert_eq!(
        states_of(&progress.now().features),
        [Built, Built, Built, Built, Queued, Built, Blocked]
    );
    assert!(!progress.now().ended);
    measure.release();
    let Ok(RunOutcome::Published(model)) = run.wait() else {
        panic!("not published");
    };
    never_back(&copies.join().unwrap()).unwrap();
    // m1's failure was kept, not asked again; m2's step never ran.
    assert_eq!(asked(&engine, is_cut), 1);
    let (m1, m2) = (
        silico_core::document::FeatureId::new("m0000001").unwrap(),
        silico_core::document::FeatureId::new("m0000002").unwrap(),
    );
    assert!(
        matches!(
            model.outcome(&m1),
            Some(silico_core::recompute::FeatureOutcome::Failed(_))
        ),
        "{:?}",
        model.outcome(&m1)
    );
    assert!(
        matches!(
            model.outcome(&m2),
            Some(silico_core::recompute::FeatureOutcome::Blocked { on }) if *on == m1
        ),
        "{:?}",
        model.outcome(&m2)
    );
    assert_eq!(
        states_of(&progress.now().features),
        [Built, Built, Built, Built, Failed, Built, Blocked]
    );
}
