//! D2's letter for M2's booleans (Fable's decision, 2026-10-18; docs/design/
//! m3.6-sketch-features.md, S5): a `fuse`, `cut`, or `common` feature whose result holds more
//! than one solid is refused, `kernel_failed`, naming itself, and what uses it is blocked;
//! decided per feature from its solid's stored history, the same fresh, warm in memory, and
//! from the disk cache. Each case's count is worked out by hand from where the boxes are.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/sweeps.rs"]
mod sweeps;

use std::path::Path;
use std::sync::Arc;

use silico_core::cache::Cache;
use silico_core::document::{Checked, read};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use sweeps::*;

fn pool() -> KernelPool {
    KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2).unwrap()
}

fn publish(engine: &Recompute<KernelPool>, features: &[&str]) -> Arc<Model> {
    let document = Checked::new(read(document(features).as_bytes()).unwrap()).unwrap();
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => model,
        other => panic!("{other:?}"),
    }
}

fn boolean(id: &str, op: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "{op}", "v": 1, "name": "B", "base": "{base}", "tool": "{tool}"}}"#
    )
}

fn fillet_of(base: &str) -> String {
    format!(
        r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "{base}", "edges": ["b0000001.box.Edge1"], "radius": 0.5}}"#
    )
}

fn refused(model: &Model, feature: &str) -> Error {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Failed(error)) => error.clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

#[test]
fn two_boxes_apart_fused_are_refused_naming_the_fuse_fresh_warm_and_from_the_cache() {
    // 10 mm cubes from the origin and from x = 20: 10 mm apart, two solids. The fuse is
    // refused, and a fillet of it blocked on it, each time alike.
    let features = [
        box_at("b0000001", [0.0; 3], 10.0, 10.0, 10.0),
        box_at("b0000002", [20.0, 0.0, 0.0], 10.0, 10.0, 10.0),
        boolean("f0000001", "fuse", "b0000001", "b0000002"),
        fillet_of("f0000001"),
    ];
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let dir = std::env::temp_dir().join(format!("silico-boolean-solids-{}", std::process::id()));
    let secret = std::env::temp_dir().join(format!(
        "silico-boolean-solids-{}.secret",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let check = |model: &Model| {
        let error = refused(model, "f0000001");
        assert_eq!(error.code(), ErrorCode::KernelFailed);
        assert_eq!(error.feature(), Some("f0000001"));
        assert_eq!(
            error.message(),
            "fuse f0000001 makes 2 solids: its base b0000001 and tool b0000002 don't touch, or meet only along an edge or at a point"
        );
        assert_eq!(
            model.outcome(&id("ffffffff")),
            Some(&FeatureOutcome::Blocked { on: id("f0000001") })
        );
        assert_eq!(model.errors().count(), 1);
        // Neither box is consumed: both are bodies, as a failed feature leaves its inputs.
        assert_eq!(model.bodies(), [id("b0000001"), id("b0000002")]);
    };
    let first = Recompute::with_cache(pool(), Cache::with_secret(&dir, &secret));
    check(&publish(&first, &refs));
    check(&publish(&first, &refs));
    drop(first);
    let second = Recompute::with_cache(pool(), Cache::with_secret(&dir, &secret));
    check(&publish(&second, &refs));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&secret);
}

#[test]
fn two_boxes_touching_fused_are_one_solid_and_built() {
    // The second cube from x = 10: they share a face, one solid of 2000 mm³.
    let engine = Recompute::new(pool());
    let model = publish(
        &engine,
        &[
            &box_at("b0000001", [0.0; 3], 10.0, 10.0, 10.0),
            &box_at("b0000002", [10.0, 0.0, 0.0], 10.0, 10.0, 10.0),
            &boolean("f0000001", "fuse", "b0000001", "b0000002"),
        ],
    );
    let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&id("f0000001")) else {
        panic!("{:?}", model.outcome(&id("f0000001")));
    };
    assert_eq!(built.history.as_ref().unwrap().result.solids, 1);
    assert!((built.summary().unwrap().volume - 2000.0).abs() <= 1e-9);
}

#[test]
fn a_box_cut_through_by_a_slab_is_refused_as_cut_in_two() {
    // The 10 mm cube less a slab across it, z 4 to 6, wider than it: two pieces.
    let engine = Recompute::new(pool());
    let model = publish(
        &engine,
        &[
            &box_at("b0000001", [0.0; 3], 10.0, 10.0, 10.0),
            &box_at("b0000002", [-1.0, -1.0, 4.0], 12.0, 12.0, 2.0),
            &boolean("c0000001", "cut", "b0000001", "b0000002"),
        ],
    );
    let error = refused(&model, "c0000001");
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    assert_eq!(
        error.message(),
        "cut c0000001 cuts its base b0000001 in two: the result is 2 solids"
    );
    assert_eq!(
        error.suggestion(),
        Some("shorten or move the tool so the base stays one piece")
    );
}

#[test]
fn a_common_of_a_slotted_box_and_a_slab_across_its_slot_is_refused_as_two_pieces() {
    // The cube with a slot from its top (x 4 to 6, z 5 to 10, through in y): one U-shaped
    // solid. In common with a slab across the slot (z 6 to 9): the slab's part in each arm,
    // two pieces.
    let engine = Recompute::new(pool());
    let model = publish(
        &engine,
        &[
            &box_at("b0000001", [0.0; 3], 10.0, 10.0, 10.0),
            &box_at("b0000002", [4.0, -1.0, 5.0], 2.0, 12.0, 6.0),
            &boolean("c0000001", "cut", "b0000001", "b0000002"),
            &box_at("b0000003", [-1.0, -1.0, 6.0], 12.0, 12.0, 3.0),
            &boolean("m0000001", "common", "c0000001", "b0000003"),
        ],
    );
    assert!(matches!(
        model.outcome(&id("c0000001")),
        Some(FeatureOutcome::Built { .. })
    ));
    let error = refused(&model, "m0000001");
    assert_eq!(
        error.message(),
        "common m0000001 makes 2 solids of base c0000001 and tool b0000003: they overlap in two places"
    );
}
