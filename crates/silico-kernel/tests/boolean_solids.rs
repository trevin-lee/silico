//! D2's letter for M2's booleans (Fable's decision, 2026-10-18; docs/design/
//! m3.6-sketch-features.md, S5): a `fuse`, `cut`, or `common` feature whose result holds more
//! than one solid is refused, `kernel_failed`, naming itself, and what uses it is blocked;
//! decided per feature from its solid's stored history, the same fresh, warm in memory, and
//! from the disk cache; and the same for a pad or pocket, end to end. Each case's count is
//! worked out by hand from where the boxes and sketches are.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/cached.rs"]
mod cached;
#[path = "../../silico-core/tests/common/sweeps.rs"]
mod sweeps;

use std::path::Path;
use std::sync::Arc;

use cached::{Counted, Scratch};
use silico_core::document::{Checked, read};
use silico_core::fake::FakeSketches;
use silico_core::kernel::Kernel;
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use sweeps::*;

fn pool() -> KernelPool {
    KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2).unwrap()
}

fn publish<K: Kernel + 'static>(engine: &Recompute<K>, features: &[&str]) -> Arc<Model> {
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
    let scratch = Scratch::new("boolean-solids");
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
    let counted = || Counted::new(env!("CARGO_BIN_EXE_silico-kernel-worker"), 2);
    let first = Recompute::with_cache(counted(), scratch.cache());
    check(&publish(&first, &refs));
    let asked = first.kernel().asked();
    check(&publish(&first, &refs));
    assert_eq!(
        first.kernel().asked(),
        asked,
        "warm, the kernel was asked again"
    );
    drop(first);
    // From the cache: every step read back, measured, nothing asked of the kernel.
    let second = Recompute::with_cache(counted(), scratch.cache());
    check(&publish(&second, &refs));
    assert_eq!(second.kernel().asked(), 0);
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

/// A counted pool over `scratch`'s cache, sketches solved as stored.
fn swept(scratch: &Scratch) -> Recompute<Counted> {
    Recompute::with_cache(
        Counted::new(env!("CARGO_BIN_EXE_silico-kernel-worker"), 2),
        scratch.cache(),
    )
    .with_sketches(FakeSketches::new())
}

#[test]
fn a_sweep_feature_leaving_more_than_one_solid_is_refused_fresh_and_from_the_cache_and_nothing_is_built_on_it()
 {
    // Design S5 end to end on the real kernel: a slot (x −5..15, y 3..5) pocketed through
    // all of the 10 mm cube under the XY plane, across its whole x span: two pieces; and a
    // pad of a rectangle beside the cube (x 20..24), missing it: two solids. Each refused in
    // its words, fresh and from the cache (nothing asked then), and a fillet on it blocked,
    // its step never asked for: the document with the fillet asks the kernel no more than
    // the one without.
    let cube = box_at("bbbbbbbb", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0);
    let all = r#"{"type": "through_all", "reversed": false}"#;
    let cases = [
        (
            rectangle("ssssssss", "xy", [-5.0, 3.0], 20.0, 2.0),
            pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", all),
            "pocket qqqqqqqq cuts its base bbbbbbbb in two: the result is 2 solids",
        ),
        (
            rectangle("ssssssss", "xy", [20.0, 0.0], 4.0, 2.0),
            pad("qqqqqqqq", Some("bbbbbbbb"), "ssssssss", TEN),
            "pad qqqqqqqq doesn't reach its base bbbbbbbb: the result is 2 solids",
        ),
    ];
    for (n, (sketch, feature, said)) in cases.into_iter().enumerate() {
        let fillet = r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "qqqqqqqq", "edges": ["bbbbbbbb.box.Edge1"], "radius": 0.5}"#;
        let without = [cube.as_str(), sketch.as_str(), feature.as_str()];
        let with = [cube.as_str(), sketch.as_str(), feature.as_str(), fillet];
        let check = |model: &Model, filleted: bool| {
            let error = refused(model, "qqqqqqqq");
            assert_eq!(error.code(), ErrorCode::KernelFailed);
            assert_eq!(error.message(), said);
            if filleted {
                assert_eq!(
                    model.outcome(&id("ffffffff")),
                    Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
                );
            }
        };
        let scratch = Scratch::new(&format!("swept-solids-{n}"));
        let plain = swept(&scratch);
        check(&publish(&plain, &without), false);
        let alone = plain.kernel().asked();
        drop(plain);
        let scratch = Scratch::new(&format!("swept-solids-{n}-again"));
        let first = swept(&scratch);
        check(&publish(&first, &with), true);
        assert_eq!(
            first.kernel().asked(),
            alone,
            "the fillet's step was asked for"
        );
        drop(first);
        let second = swept(&scratch);
        check(&publish(&second, &with), true);
        assert_eq!(second.kernel().asked(), 0);
    }
}
