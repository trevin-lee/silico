//! M2.4: recompute on the in-memory kernel. Expected values are written by hand: which
//! kernel calls an edit makes (a feature's steps and those below it, nothing else), what
//! each feature comes to, the bodies, and the shapes, as the fake describes what made them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;

use std::sync::Arc;
use std::time::Duration;

use example::EXAMPLE;
use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Kernel, Op, Primitive};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};

const WAIT: Duration = Duration::from_secs(10);

fn checked(text: &str) -> Checked {
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

/// A document of these features, each a JSON object's text.
fn document(features: &[&str]) -> Checked {
    checked(&format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}]}}",
        features.join(", ")
    ))
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn engine(size: usize) -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(size).unwrap())
}

/// Recomputes `document` and returns the model it published.
fn published(engine: &Recompute<FakeKernel>, document: Checked) -> Arc<Model> {
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => model,
        RunOutcome::Superseded => panic!("superseded"),
    }
}

/// The names of the kernel calls made since `from`.
fn calls_since(engine: &Recompute<FakeKernel>, from: usize) -> Vec<&'static str> {
    engine.kernel().calls()[from..]
        .iter()
        .map(|call| call.op.name())
        .collect()
}

fn described(model: &Model, feature: &str) -> String {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => FakeKernel::describe(&built.shape).unwrap(),
        other => panic!("{feature}: {other:?}"),
    }
}

fn ids(texts: &[&str]) -> Vec<FeatureId> {
    texts.iter().map(|t| id(t)).collect()
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
    assert!(engine.kernel().wait_running(2, WAIT));
    hold.release();
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    assert_eq!(engine.kernel().most_running(), 2);
}

#[test]
fn a_chain_runs_one_step_at_a_time() {
    let engine = engine(4);
    published(
        &engine,
        document(&[
            r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Moved", "position": [5, 0, 0], "length": 1, "width": 1, "height": 1}"#,
        ]),
    );
    assert_eq!(calls_since(&engine, 0), ["MakeBox", "Translate"]);
    assert_eq!(engine.kernel().most_running(), 1);
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
