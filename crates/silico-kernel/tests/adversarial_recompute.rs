//! /break-it on M2.4 recompute, on the real worker pool (OCCT): the same document gives
//! the same bytes whatever the pool's size and whatever recomputes superseded it first, and
//! a superseded or dropped recompute leaves nothing behind.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::document::{Checked, FeatureId, read};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

const WAIT: Duration = Duration::from_secs(60);

fn pool(size: usize) -> KernelPool {
    KernelPool::start_on_demand(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), size)
        .unwrap()
}

fn checked(features: &[String]) -> Checked {
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}]}}",
        features.join(", ")
    );
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

/// A part with every primitive and boolean, the pin's radius `pin`: one solid, every piece
/// meeting the block (the ring's tube from z 9.5, so the cap sits on the block's top; it
/// was at z 12, the cap floating apart, two solids, which D2 at M2's booleans refuses since
/// Fable's decision of 2026-10-18).
fn part(pin: f64) -> Checked {
    checked(&[
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#.to_owned(),
        format!(r#"{{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": {pin}, "height": 12}}"#),
        r#"{"id": "h0000001", "type": "cut", "v": 1, "name": "Hole", "base": "b0000001", "tool": "c0000001"}"#.to_owned(),
        r#"{"id": "s0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [40, 20, 10], "radius": 4}"#.to_owned(),
        r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Knob", "base": "h0000001", "tool": "s0000001"}"#.to_owned(),
        r#"{"id": "k0000001", "type": "cone", "v": 1, "name": "Tip", "position": [5, 5, 10], "radius1": 3, "radius2": 0, "height": 6}"#.to_owned(),
        r#"{"id": "t0000001", "type": "torus", "v": 1, "name": "Ring", "position": [5, 5, 10.5], "radius1": 3, "radius2": 1}"#.to_owned(),
        r#"{"id": "m0000001", "type": "common", "v": 1, "name": "Cap", "base": "k0000001", "tool": "t0000001"}"#.to_owned(),
        r#"{"id": "f0000002", "type": "fuse", "v": 1, "name": "All", "base": "f0000001", "tool": "m0000001"}"#.to_owned(),
    ])
}

fn published(engine: &Recompute<KernelPool>, document: Checked) -> Arc<Model> {
    match engine.start(document).wait_timeout(WAIT).ok() {
        Some(Ok(RunOutcome::Published(model))) => model,
        other => panic!("{other:?}"),
    }
}

fn built(model: &Model) {
    for (id, outcome) in model.features() {
        let FeatureOutcome::Built { built, .. } = outcome else {
            panic!("{id}: {outcome:?}")
        };
        assert_eq!(built.shape.valid(), Some(true), "{id}");
    }
}

#[test]
fn a_part_builds_to_the_same_bytes_on_one_worker_and_on_eight() {
    let one = published(&Recompute::new(pool(1)), part(2.5));
    let eight = published(&Recompute::new(pool(8)), part(2.5));
    built(&one);
    assert_eq!(one.features(), eight.features());
    assert_eq!(one.bodies(), [FeatureId::new("f0000002").unwrap()]);
}

#[test]
fn after_a_storm_of_superseded_edits_the_last_builds_to_a_fresh_builds_bytes() {
    let engine = Recompute::new(pool(4));
    let runs: Vec<_> = (0..20)
        .map(|n| engine.start(part(1.0 + f64::from(n) * 0.125)))
        .collect();
    let mut outcomes: Vec<_> = runs
        .into_iter()
        .map(|run| run.wait_timeout(WAIT).ok().expect("hung").unwrap())
        .collect();
    let last = outcomes.pop().unwrap();
    for outcome in &outcomes {
        assert_eq!(outcome, &RunOutcome::Superseded);
    }
    let RunOutcome::Published(model) = last else {
        panic!("{last:?}")
    };
    built(&model);
    let fresh = published(&Recompute::new(pool(2)), part(1.0 + 19.0 * 0.125));
    assert_eq!(model.features(), fresh.features());
    // And steps kept from the superseded recomputes rebuild an earlier edit to its fresh
    // bytes too.
    let again = published(&engine, part(1.5));
    let fresh = published(&Recompute::new(pool(2)), part(1.5));
    assert_eq!(again.features(), fresh.features());
}

#[test]
fn dropping_the_engine_mid_recompute_returns_promptly() {
    // The run may have finished by the drop, so either outcome; what's tested is that the
    // drop doesn't wait on it.
    let engine = Recompute::new(pool(2));
    let run = engine.start(part(2.5));
    let lateness = crate::lateness::Lateness::start();
    let at = Instant::now();
    drop(engine);
    let bound = crate::lateness::allowance(Duration::from_secs(1), lateness.most());
    assert!(
        at.elapsed() < bound,
        "{:?}, the bound {bound:?}",
        at.elapsed()
    );
    match run.wait_timeout(WAIT).ok() {
        Some(Ok(RunOutcome::Superseded | RunOutcome::Published(_))) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn booleans_occt_finds_empty_fail_their_feature_and_block_on_the_base_first() {
    // By hand: a sphere 100 mm from a 1 mm box has nothing in common with it, and a box cut
    // by the same box leaves nothing. Both are the kernel's refusals, naming the feature;
    // the fuse of the two is blocked on its base.
    let engine = Recompute::new(pool(2));
    let model = published(
        &engine,
        checked(&[
            r#"{"id": "a0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
            r#"{"id": "a0000002", "type": "sphere", "v": 1, "name": "B", "position": [100, 0, 0], "radius": 1}"#.to_owned(),
            r#"{"id": "a0000003", "type": "common", "v": 1, "name": "C", "base": "a0000001", "tool": "a0000002"}"#.to_owned(),
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "D", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
            r#"{"id": "b0000002", "type": "box", "v": 1, "name": "E", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
            r#"{"id": "b0000003", "type": "cut", "v": 1, "name": "F", "base": "b0000001", "tool": "b0000002"}"#.to_owned(),
            r#"{"id": "c0000001", "type": "fuse", "v": 1, "name": "G", "base": "a0000003", "tool": "b0000003"}"#.to_owned(),
        ]),
    );
    let id = |s: &str| FeatureId::new(s).unwrap();
    for failed in ["a0000003", "b0000003"] {
        match model.outcome(&id(failed)) {
            Some(FeatureOutcome::Failed(e)) => {
                assert_eq!(e.code(), silico_core::ErrorCode::KernelFailed, "{e}");
                assert_eq!(e.feature(), Some(failed));
                assert!(e.suggestion().is_some(), "{e}");
            }
            other => panic!("{failed}: {other:?}"),
        }
    }
    assert_eq!(
        model.outcome(&id("c0000001")),
        Some(&FeatureOutcome::Blocked { on: id("a0000003") })
    );
    assert_eq!(
        model.bodies(),
        [
            id("a0000001"),
            id("a0000002"),
            id("b0000001"),
            id("b0000002")
        ]
    );
    assert_eq!(model.errors().count(), 2);
}

// break-it: recompute's parallel branches on a pool whose slots start on demand.

fn on_demand(size: usize) -> KernelPool {
    KernelPool::start_on_demand(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), size)
        .unwrap()
}

#[test]
fn a_part_builds_to_the_same_bytes_on_a_pool_started_on_demand() {
    // The part's three branches (block and pin, ball, cone and torus) run at once on a
    // pool that starts its slots as they're needed: the same bytes as one worker's, and no
    // more workers than the pool's size.
    let one = published(&Recompute::new(pool(1)), part(2.5));
    for size in [1, 2, 8] {
        let engine = Recompute::new(on_demand(size));
        let model = published(&engine, part(2.5));
        built(&model);
        assert_eq!(model.features(), one.features(), "size {size}");
        let held = engine.kernel().pids().len();
        assert!((1..=size).contains(&held), "size {size}: {held} workers");
    }
}

#[test]
fn a_storm_of_superseded_edits_on_a_pool_started_on_demand_ends_in_a_fresh_builds_bytes() {
    // Twenty edits at once on a pool with no slot started, its superseded requests stopped
    // at once: slots start, are condemned, and replace their workers while others start.
    let engine = Recompute::new(on_demand(4));
    engine.kernel().set_stop_after(Duration::ZERO);
    let runs: Vec<_> = (0..20)
        .map(|n| engine.start(part(1.0 + f64::from(n) * 0.125)))
        .collect();
    let mut outcomes: Vec<_> = runs
        .into_iter()
        .map(|run| run.wait_timeout(WAIT).ok().expect("hung").unwrap())
        .collect();
    let last = outcomes.pop().unwrap();
    for outcome in &outcomes {
        assert_eq!(outcome, &RunOutcome::Superseded);
    }
    let RunOutcome::Published(model) = last else {
        panic!("{last:?}")
    };
    built(&model);
    let fresh = published(&Recompute::new(pool(1)), part(1.0 + 19.0 * 0.125));
    assert_eq!(model.features(), fresh.features());
    assert!(engine.kernel().pids().len() <= 4);
}

#[test]
fn dropping_the_engine_while_its_on_demand_slots_start_returns_promptly() {
    for _ in 0..20 {
        let engine = Recompute::new(on_demand(8));
        let run = engine.start(part(2.5));
        let lateness = crate::lateness::Lateness::start();
        let at = Instant::now();
        drop(engine);
        let bound = crate::lateness::allowance(Duration::from_secs(2), lateness.most());
        assert!(
            at.elapsed() < bound,
            "{:?}, the bound {bound:?}",
            at.elapsed()
        );
        match run.wait_timeout(WAIT).ok() {
            Some(Ok(RunOutcome::Superseded | RunOutcome::Published(_))) => {}
            other => panic!("{other:?}"),
        }
    }
}
