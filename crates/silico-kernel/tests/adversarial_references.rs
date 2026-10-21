//! break-it, M2.8, on the real kernel: fillets and chamfers of edges named in the document,
//! through upstream edits that change a primitive's topology, and of edges OCCT can't round
//! (a sphere's poles and seam, a cone's apex, an edge ending where two solids touch at a
//! corner). Expected values are worked out by hand from the geometry: where the material a
//! fillet takes lies (within its radius of the edge it rounds), and that a refusal is a
//! structured error naming the feature, after which the next document still builds.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
        KernelPool::start_on_demand(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
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

/// (volume, center) of a feature that built.
fn measures(model: &Model, feature: &str) -> (f64, [f64; 3]) {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => {
            let summary = built.summary().unwrap();
            (summary.volume, summary.center)
        }
        other => panic!("{feature}: {other:?}"),
    }
}

/// A cone (`kkkkkkkk`: radius 5 at z = 0, `top` at z = 10) with a hole of radius 1 down its
/// axis (pin `pppppppp` from z = -1 to 11, cut `hhhhhhhh`), and a fillet `ffffffff` of
/// radius 0.2 of `edge` of the hole.
fn holed_cone(top: f64, edge: &str) -> String {
    format!(
        r#"{HEAD}
{{"id": "kkkkkkkk", "type": "cone", "v": 1, "name": "Cone", "position": [0, 0, 0], "radius1": 5, "radius2": {top}, "height": 10}},
{{"id": "pppppppp", "type": "cylinder", "v": 1, "name": "Pin", "position": [0, 0, -1], "radius": 1, "height": 12}},
{{"id": "hhhhhhhh", "type": "cut", "v": 1, "name": "Hole", "base": "kkkkkkkk", "tool": "pppppppp"}},
{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "hhhhhhhh", "edges": [{edge:?}], "radius": 0.2}}
]}}"#
    )
}

/// The hole's rim where the cone's `face` meets the pin's side, as M2.7 names it.
fn rim(face: u32) -> String {
    format!("hhhhhhhh.cut.Edge(G kkkkkkkk.cone.Face{face}, G pppppppp.cylinder.Face1)")
}

/// The height of the material a fillet took from the hole: the centroid of what the hole
/// had and the fillet hasn't.
fn taken_at(model: &Model) -> f64 {
    let (hole, at) = measures(model, "hhhhhhhh");
    let (round, round_at) = measures(model, "ffffffff");
    let taken = hole - round;
    assert!(
        taken > 0.0,
        "the fillet took nothing: {hole} against {round}"
    );
    (hole * at[2] - round * round_at[2]) / taken
}

#[test]
fn cone_top_rim_named_before_the_top_shrinks_to_a_point_is_never_the_bottom_rim() {
    // The cone's top cap is its Face2, the base its Face3, while it has a top; a cone that
    // comes to a point has no top cap, and its base is Face2. The hole's top rim is named
    // from the cone's Face2; the fillet of it takes material within 0.2 of z = 10.
    let engine = engine();
    let top_rim = rim(2);
    let model = publish(&engine, &holed_cone(3.0, &top_rim));
    let errors: Vec<_> = model.errors().collect();
    assert!(errors.is_empty(), "{errors:?}");
    let map = model.element_map(&id("hhhhhhhh")).unwrap();
    let names: Vec<String> = map.iter().map(|(_, n)| n[0].to_string()).collect();
    assert!(names.contains(&top_rim), "{names:#?}");
    assert!(names.contains(&rim(3)), "{names:#?}");
    let z = taken_at(&model);
    assert!(
        (9.8..=10.0).contains(&z),
        "the top rim's fillet took material at z = {z}"
    );
    // The edit: the cone comes to a point. Its top rim is gone (the pin's side meets the
    // cone's side at z = 8 instead), so the reference to it is broken, and the base rim,
    // the only rim left on a cap, must not be taken for it.
    let model = publish(&engine, &holed_cone(0.0, &top_rim));
    let outcome = model.outcome(&id("ffffffff")).cloned();
    if let Some(FeatureOutcome::Built { .. }) = outcome {
        let z = taken_at(&model);
        // BREAKS: a primitive's elements are named by their numbers (M2.7), and OCCT numbers
        // a pointed cone's base Face2: the hole's base rim is now named exactly as its top
        // rim was, `hhhhhhhh.cut.Edge(G kkkkkkkk.cone.Face2, G pppppppp.cylinder.Face1)`, so the reference
        // resolves, silently, to the base rim, and the fillet rounds the bottom of the hole
        // (material taken at z = 0.04). FreeCAD 1.1.3, the same edit: the top rim was
        // `Face2;:G2(Face1;K-1;...)`, the base rim is now `Face2;:G2(Face1;K-2;...)` (its
        // place among the pin face's generated edges), so its fillet reports "Missing edge
        // link" and fails.
        panic!("the reference re-pointed: the fillet took material at z = {z}, not near 10");
    }
    let errors: Vec<ErrorCode> = model.errors().map(|e| e.code()).collect();
    assert_eq!(errors, [ErrorCode::ReferenceBroken]);
}

/// A document of one primitive `a` (`aaaaaaaa`), then `dress`, a fillet or chamfer of it.
fn dressed(primitive: &str, dress: &str) -> String {
    format!("{HEAD}\n{primitive},\n{dress}\n]}}")
}

const SPHERE: &str = r#"{"id": "aaaaaaaa", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}"#;
const POINTED: &str = r#"{"id": "aaaaaaaa", "type": "cone", "v": 1, "name": "Cone", "position": [0, 0, 0], "radius1": 5, "radius2": 0, "height": 10}"#;
const CYLINDER: &str = r#"{"id": "aaaaaaaa", "type": "cylinder", "v": 1, "name": "Can", "position": [0, 0, 0], "radius": 5, "height": 10}"#;
const TORUS: &str = r#"{"id": "aaaaaaaa", "type": "torus", "v": 1, "name": "Ring", "position": [0, 0, 0], "radius1": 10, "radius2": 2}"#;

fn fillet_of(edge: &str, radius: f64) -> String {
    format!(
        r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "aaaaaaaa", "edges": [{edge:?}], "radius": {radius}}}"#
    )
}

fn chamfer_of(edge: &str, distance: f64) -> String {
    format!(
        r#"{{"id": "ffffffff", "type": "chamfer", "v": 1, "name": "Bevel", "base": "aaaaaaaa", "edges": [{edge:?}], "size": {{"distance": {distance}}}}}"#
    )
}

#[test]
fn degenerate_and_seam_edges_named_in_a_document_are_refused_naming_the_feature() {
    // FreeCAD d214306d1b: a chamfer of a degenerated edge crashed OCCT. A sphere's poles
    // (Edge1, Edge3) and a pointed cone's apex are degenerate (its Edge4, at the top: a
    // cone's edges are numbered as in its fullest form); a sphere's,
    // cylinder's, cone's, and torus's seams have one face on both sides. Each is refused,
    // as a structured error naming the fillet, and the engine builds the next document.
    let engine = engine();
    let cases = [
        (SPHERE, "aaaaaaaa.sphere.Edge1"),
        (SPHERE, "aaaaaaaa.sphere.Edge3"),
        (SPHERE, "aaaaaaaa.sphere.Edge2"),
        (POINTED, "aaaaaaaa.cone.Edge4"),
        (POINTED, "aaaaaaaa.cone.Edge2"),
        (CYLINDER, "aaaaaaaa.cylinder.Edge1"),
        (TORUS, "aaaaaaaa.torus.Edge1"),
        (TORUS, "aaaaaaaa.torus.Edge2"),
    ];
    for (primitive, edge) in cases {
        for dress in [fillet_of(edge, 0.5), chamfer_of(edge, 0.5)] {
            let model = publish(&engine, &dressed(primitive, &dress));
            match model.outcome(&id("ffffffff")) {
                Some(FeatureOutcome::Failed(error)) => {
                    assert!(
                        matches!(
                            error.code(),
                            ErrorCode::KernelFailed
                                | ErrorCode::InvalidResult
                                | ErrorCode::InvalidInput
                        ),
                        "{dress}: {error:?}"
                    );
                    assert_eq!(error.feature(), Some("ffffffff"), "{dress}: {error:?}");
                    assert!(error.suggestion().is_some(), "{dress}: {error:?}");
                }
                Some(FeatureOutcome::Built { built, .. }) => {
                    // Built: then it must be a valid solid that differs from the primitive.
                    let summary = built.summary().unwrap();
                    assert!(summary.valid, "{dress}: an invalid solid, {summary:?}");
                    panic!("{dress}: rounded an edge with one face, or none: {summary:?}");
                }
                other => panic!("{dress}: {other:?}"),
            }
        }
        // The next document, a box's fillet, still builds.
        let model = publish(
            &engine,
            &dressed(
                r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
                &fillet_of("aaaaaaaa.box.Edge1", 1.0),
            ),
        );
        assert!(model.errors().next().is_none(), "after {edge}");
    }
}

#[test]
fn fillet_of_an_edge_ending_where_two_boxes_touch_at_a_corner_fails_the_feature_cleanly() {
    // Two cubes fused corner to corner at (10, 10, 10): the first box's Edge7, from
    // (10, 10, 0) to (10, 10, 10), ends there. OCCT segfaults filleting it, as FreeCAD
    // does; named in a document, it fails the fillet with a structured error, and the
    // engine rebuilds the next document.
    let engine = engine();
    let text = format!(
        r#"{HEAD}
{{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}},
{{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "B", "position": [10, 10, 10], "length": 10, "width": 10, "height": 10}},
{{"id": "rrrrrrrr", "type": "fuse", "v": 1, "name": "Fusion", "base": "aaaaaaaa", "tool": "bbbbbbbb"}},
{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "rrrrrrrr", "edges": ["aaaaaaaa.box.Edge7"], "radius": 1}}
]}}"#
    );
    let model = publish(&engine, &text);
    match model.outcome(&id("ffffffff")) {
        Some(FeatureOutcome::Failed(error)) => {
            assert_eq!(error.feature(), Some("ffffffff"), "{error:?}");
            assert_ne!(error.code(), ErrorCode::Internal, "{error:?}");
            assert!(error.suggestion().is_some(), "{error:?}");
        }
        other => panic!("{other:?}"),
    }
    let model = publish(
        &engine,
        &dressed(
            r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
            &fillet_of("aaaaaaaa.box.Edge7", 1.0),
        ),
    );
    assert!(
        model.errors().next().is_none(),
        "{:?}",
        model.errors().collect::<Vec<_>>()
    );
}
