//! break-it: N8 and N9 on the real kernel. A boolean retyped, or its operands swapped, must
//! never take a reference to an element for another. Volumes worked out by hand: a fillet
//! of a straight edge `l` long, radius r, between faces at a right angle changes the volume
//! by (1 − π/4) r² l, taken away where the edge is convex, added where it's concave. (The
//! void's ring is silico-kernel's tests/references.rs.)
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::PI;
use std::path::Path;
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::document::{Checked, FeatureId, read};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

const HEAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": ["#;

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn engine() -> Recompute<KernelPool> {
    Recompute::new(
        KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
            .unwrap(),
    )
}

fn publish(engine: &Recompute<KernelPool>, text: &str) -> Arc<Model> {
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => model,
        other => panic!("{other:?}"),
    }
}

fn volume(model: &Model, feature: &str) -> f64 {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => built.summary().unwrap().volume,
        other => panic!("{feature}: {other:?}"),
    }
}

/// `a` (`aaaaaaaa`) and `b` (`bbbbbbbb`), their boolean `cccccccc` of type `op` (of `base`
/// and `tool`), and a fillet `ffffffff` of `edge`, radius `r`.
fn boolean(a: &str, b: &str, op: &str, base: &str, tool: &str, edge: &str, r: f64) -> String {
    format!(
        r#"{HEAD}
{a},
{b},
{{"id": "cccccccc", "type": "{op}", "v": 1, "name": "Op", "base": "{base}", "tool": "{tool}"}},
{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "cccccccc", "edges": [{edge:?}], "radius": {r}}}
]}}"#
    )
}

/// A 10 mm cube at the origin.
const BLOCK: &str = r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#;

fn outcome_word(model: &Model, feature: &str) -> String {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { .. }) => "built".to_owned(),
        Some(FeatureOutcome::Broken { errors, .. }) => format!("broken: {errors:?}"),
        other => format!("{other:?}"),
    }
}

/// A 2 mm cube at (4, 4, 4): wholly inside a 10 mm block.
const CUBE_INSIDE: &str = r#"{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Cube", "position": [4, 4, 4], "length": 2, "width": 2, "height": 2}"#;

#[test]
fn a_common_made_a_cut_of_its_operands_swapped_never_takes_the_cubes_edge_for_the_voids() {
    // Found by break-it (round 2's R2B1, swapped), closed by N9. The common of a small cube
    // and the block around it is the cube, every element the common's own, `I`: its Edge1
    // (along z at x = y = 4, 2 long), kept whole, is `cccccccc.common.Edge(I
    // bbbbbbbb.box.Edge1)`, convex, a fillet takes material away. The block cut by the cube
    // (the common made a cut, base and tool swapped) leaves a cubic void whose edge there is
    // the cut's own, `cccccccc.cut.Edge(I ...)`, concave: the common's name breaks, the cut's
    // its candidate, and by it the fillet adds what the other took. The cube's own name
    // finds neither.
    let engine = engine();
    let r = 0.2;
    let straight = (1.0 - PI / 4.0) * r * r * 2.0;
    let of = |op: &str| format!("cccccccc.{op}.Edge(I bbbbbbbb.box.Edge1)");
    let common = |edge: &str| {
        boolean(
            BLOCK,
            CUBE_INSIDE,
            "common",
            "bbbbbbbb",
            "aaaaaaaa",
            edge,
            r,
        )
    };
    let cut = |edge: &str| boolean(BLOCK, CUBE_INSIDE, "cut", "aaaaaaaa", "bbbbbbbb", edge, r);
    let model = publish(&engine, &common(&of("common")));
    assert!(
        model.errors().next().is_none(),
        "{:?}",
        model.errors().collect::<Vec<_>>()
    );
    let taken = volume(&model, "cccccccc") - volume(&model, "ffffffff");
    assert!(
        (taken - straight).abs() < 1e-6,
        "the cube's edge: {taken} against {straight}"
    );
    let model = publish(&engine, &cut(&of("common")));
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("ffffffff")) else {
        panic!("{}", outcome_word(&model, "ffffffff"));
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    assert_eq!(
        errors[0].candidates().first(),
        Some(&of("cut")),
        "{errors:?}"
    );
    let model = publish(&engine, &cut(&of("cut")));
    assert!(
        model.errors().next().is_none(),
        "{:?}",
        model.errors().collect::<Vec<_>>()
    );
    let added = volume(&model, "ffffffff") - volume(&model, "cccccccc");
    assert!(
        (added - straight).abs() < 1e-6,
        "the void's edge: {added} against {straight}"
    );
    for (text, op) in [
        (common("bbbbbbbb.box.Edge1"), "common"),
        (cut("bbbbbbbb.box.Edge1"), "cut"),
    ] {
        let model = publish(&engine, &text);
        let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("ffffffff")) else {
            panic!("{op}: {}", outcome_word(&model, "ffffffff"));
        };
        assert_eq!(
            errors[0].candidates().first(),
            Some(&of(op)),
            "{op}: {errors:?}"
        );
    }
}

#[test]
fn a_rim_whose_tool_was_retyped_says_which_feature_changed_type() {
    // Found by break-it (wording): a hole's rim names the pin's side
    // (`bbbbbbbb.cylinder.Face1`). The pin made a cone under its ID breaks the reference, as
    // it must, with the cone's rim the first candidate; the error says the type change,
    // though the feature that changed is one the name holds inside, not its top.
    let engine = engine();
    let pin = |kind: &str| {
        let values = match kind {
            "cylinder" => r#""radius": 2, "height": 12"#,
            _ => r#""radius1": 2, "radius2": 1.5, "height": 12"#,
        };
        format!(
            r#"{{"id": "bbbbbbbb", "type": "{kind}", "v": 1, "name": "Pin", "position": [5, 5, -1], {values}}}"#
        )
    };
    let rim =
        |kind: &str| format!("cccccccc.cut.Edge(G aaaaaaaa.box.Face6, G bbbbbbbb.{kind}.Face1)");
    let as_cylinder = publish(
        &engine,
        &boolean(
            BLOCK,
            &pin("cylinder"),
            "cut",
            "aaaaaaaa",
            "bbbbbbbb",
            &rim("cylinder"),
            0.5,
        ),
    );
    assert!(
        as_cylinder.errors().next().is_none(),
        "{:?}",
        as_cylinder.errors().collect::<Vec<_>>()
    );
    let as_cone = publish(
        &engine,
        &boolean(
            BLOCK,
            &pin("cone"),
            "cut",
            "aaaaaaaa",
            "bbbbbbbb",
            &rim("cylinder"),
            0.5,
        ),
    );
    let Some(FeatureOutcome::Broken { errors, .. }) = as_cone.outcome(&id("ffffffff")) else {
        panic!("{}", outcome_word(&as_cone, "ffffffff"));
    };
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    assert_eq!(errors[0].candidates().first(), Some(&rim("cone")));
    assert!(
        errors[0]
            .message()
            .contains("names an element a cylinder made, but bbbbbbbb is a cone now"),
        "{}",
        errors[0]
    );
}
