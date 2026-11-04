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
use silico_core::recompute::{FeatureStates, Recompute, RunOutcome, StepCounter, StepState};
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
        prop_assert!(matches!(run.wait(), Ok(RunOutcome::Published(_))));
        let now = progress.now();
        let listed: Vec<String> = now.features.iter().map(|(f, _)| f.as_str().to_owned()).collect();
        let ids: Vec<String> = (0..timeline.len()).map(|i| format!("f{i:07}")).collect();
        prop_assert_eq!(listed, ids);
        prop_assert_eq!(states_of(&now.features), expected(&timeline));
    }
}
