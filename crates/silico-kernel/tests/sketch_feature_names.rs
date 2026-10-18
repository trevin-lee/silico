//! M3.6 on the real kernel: the names users store of a pad's, pocket's, revolve's, and
//! groove's elements (docs/design/m3.6-sketch-features.md, S4 and S9), each pinned to where
//! its element is. Every expected name is the design's table written out for the case, and
//! every expected place worked out by hand from the sketch, the extent, and the base: each
//! element is found by its name in the feature's element map, and its place read from the
//! kernel's mesh of the solid (an edge's points, a face's triangles), so a name that named
//! another element would be caught. Sketches are solved as stored (no constraints), by the
//! fake sketch solver.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/sweeps.rs"]
mod sweeps;

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;
use std::path::Path;
use std::sync::Arc;

use silico_core::document::{Checked, read};
use silico_core::fake::FakeSketches;
use silico_core::kernel::{ElementKind, KernelWorker};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use sweeps::*;

fn engine() -> Recompute<KernelPool> {
    Recompute::new(
        KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
            .unwrap(),
    )
    .with_sketches(FakeSketches::new())
}

fn publish(engine: &Recompute<KernelPool>, features: &[&str]) -> Arc<Model> {
    let document = Checked::new(read(document(features).as_bytes()).unwrap()).unwrap();
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => model,
        other => panic!("{other:?}"),
    }
}

/// A box, `[low, high]`: where an element's points are.
type Bounds = [[f64; 3]; 2];

/// Every element of `feature`'s solid by its own name, with where its points lie (the
/// kernel's mesh: an edge's polyline, a face's triangles; a vertex has none, and a
/// degenerate edge none either: `None`). Every name is stable (a reference may take it),
/// and none is a sketch's: a sketch's elements are in no solid.
fn named(model: &Model, feature: &str) -> BTreeMap<String, Option<Bounds>> {
    let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&id(feature)) else {
        panic!("{feature}: {:?}", model.outcome(&id(feature)));
    };
    let mut kernel =
        KernelWorker::start_worker(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap();
    let mesh = kernel.tessellate(&built.shape, 0.05).unwrap();
    let map = model.element_map(&id(feature)).unwrap();
    let bounds = |points: &mut dyn Iterator<Item = [f64; 3]>| -> Option<Bounds> {
        let mut found: Option<Bounds> = None;
        for p in points {
            let [low, high] = found.get_or_insert([p, p]);
            for k in 0..3 {
                low[k] = low[k].min(p[k]);
                high[k] = high[k].max(p[k]);
            }
        }
        found
    };
    let mut all = BTreeMap::new();
    for (element, names) in map.iter() {
        let name = names.first().unwrap();
        assert!(!map.is_unstable(name), "{feature}: {name} is unstable");
        let text = name.to_string();
        // Its type word, after the feature's ID, is never a sketch's.
        assert_ne!(
            text.split('.').nth(1),
            Some("sketch"),
            "{feature}: {text} is a sketch's own"
        );
        let n = element.index as usize - 1;
        let place = match element.kind {
            ElementKind::Edge => {
                let start: usize = mesh.edge_point_counts[..n]
                    .iter()
                    .map(|&c| c as usize)
                    .sum();
                let count = mesh.edge_point_counts[n] as usize;
                bounds(&mut mesh.edge_points[start..start + count].iter().copied())
            }
            ElementKind::Face => {
                let start: usize = mesh.face_triangles[..n].iter().map(|&c| c as usize).sum();
                let count = mesh.face_triangles[n] as usize;
                bounds(
                    &mut mesh.triangles[start..start + count]
                        .iter()
                        .flatten()
                        .map(|&i| mesh.positions[i as usize]),
                )
            }
            ElementKind::Vertex | ElementKind::Solid => None,
        };
        assert!(all.insert(text, place).is_none());
    }
    all
}

/// That `names` has `name`, its points spanning `low` to `high` (to 1e-9: a plane's or a
/// line's points lie on it exactly, to rounding).
#[track_caller]
fn assert_at(names: &BTreeMap<String, Option<Bounds>>, name: &str, low: [f64; 3], high: [f64; 3]) {
    let Some(place) = names.get(name) else {
        panic!(
            "no {name} among:\n{}",
            names.keys().cloned().collect::<Vec<_>>().join("\n")
        );
    };
    let Some([got_low, got_high]) = place else {
        panic!("{name} has no points");
    };
    for k in 0..3 {
        assert!(
            (got_low[k] - low[k]).abs() <= 1e-9 && (got_high[k] - high[k]).abs() <= 1e-9,
            "{name}: {got_low:?}..{got_high:?}, not {low:?}..{high:?}"
        );
    }
}

fn volume(model: &Model, feature: &str) -> f64 {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => built.summary().unwrap().volume,
        other => panic!("{feature}: {other:?}"),
    }
}

#[track_caller]
fn assert_close(got: f64, want: f64, what: &str) {
    assert!(
        (got - want).abs() <= 1e-9 * want.abs().max(1.0),
        "{what}: {got} against {want}"
    );
}

/// The model's errors, as `(code, feature, message)`.
fn errors(model: &Model) -> Vec<(ErrorCode, String, String)> {
    model
        .errors()
        .map(|e: &Error| {
            (
                e.code(),
                e.feature().unwrap_or_default().to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

/// A 10 mm cube under the XY plane, so the plane is its top face (its Face6).
fn cube_below() -> String {
    box_at("bbbbbbbb", [0.0, 0.0, -10.0], 10.0, 10.0, 10.0)
}

/// The 2 mm square from (4, 4) on XY, its corners pa000001 to pa000004 counter-clockwise
/// and its sides na000001 (y = 4), na000002 (x = 6), na000003 (y = 6), na000004 (x = 4).
fn square() -> String {
    rectangle("ssssssss", "xy", [4.0, 4.0], 2.0, 2.0)
}

const THROUGH: &str = r#"{"type": "through_all", "reversed": false}"#;

/// Where the square's sides and corners are, at height `z`: each side's ID and its ends,
/// each corner's ID and where it is.
fn sides(z: f64) -> [(&'static str, [f64; 3], [f64; 3]); 4] {
    [
        ("na000001", [4.0, 4.0, z], [6.0, 4.0, z]),
        ("na000002", [6.0, 4.0, z], [6.0, 6.0, z]),
        ("na000003", [4.0, 6.0, z], [6.0, 6.0, z]),
        ("na000004", [4.0, 4.0, z], [4.0, 6.0, z]),
    ]
}

fn corners() -> [(&'static str, f64, f64); 4] {
    [
        ("pa000001", 4.0, 4.0),
        ("pa000002", 6.0, 4.0),
        ("pa000003", 6.0, 6.0),
        ("pa000004", 4.0, 6.0),
    ]
}

#[test]
fn a_pad_and_a_blind_pocket_in_a_boxs_top_name_their_elements_as_the_design_says() {
    // Design S9: the top stays b.box.Face6; walls Face(G Edge.c); the pad's top and the
    // pocket's floor Face(G Face1); rims Edge(M Edge.c); vertical edges Edge(G Vertex.v); the
    // pad's top edges and the pocket's floor edges Edge(G Edge.c).
    let engine = engine();
    for (feature, kind, end) in [("pppppppp", "pad", 5.0f64), ("qqqqqqqq", "pocket", -5.0)] {
        let sweep = if kind == "pad" {
            pad(feature, Some("bbbbbbbb"), "ssssssss", &one_side("5"))
        } else {
            pocket(feature, "bbbbbbbb", "ssssssss", &one_side("5"))
        };
        let model = publish(&engine, &[&cube_below(), &square(), &sweep]);
        assert_eq!(errors(&model), []);
        let names = named(&model, feature);
        // The cube's top keeps its name, with the square's outline in it.
        assert_at(
            &names,
            "bbbbbbbb.box.Face6",
            [0.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
        );
        assert_at(
            &names,
            "bbbbbbbb.box.Face5",
            [0.0, 0.0, -10.0],
            [10.0, 10.0, -10.0],
        );
        let (low, high) = (end.min(0.0), end.max(0.0));
        for (side, from, to) in sides(0.0) {
            let wall = format!("{feature}.{kind}.Face(G ssssssss.sketch.Edge.{side})");
            assert_at(&names, &wall, [from[0], from[1], low], [to[0], to[1], high]);
            let rim = format!("{feature}.{kind}.Edge(M ssssssss.sketch.Edge.{side})");
            assert_at(&names, &rim, from, to);
            let far = format!("{feature}.{kind}.Edge(G ssssssss.sketch.Edge.{side})");
            assert_at(&names, &far, [from[0], from[1], end], [to[0], to[1], end]);
        }
        for (corner, x, y) in corners() {
            let edge = format!("{feature}.{kind}.Edge(G ssssssss.sketch.Vertex.{corner})");
            assert_at(&names, &edge, [x, y, low], [x, y, high]);
            for letter in ["M", "G"] {
                let vertex =
                    format!("{feature}.{kind}.Vertex({letter} ssssssss.sketch.Vertex.{corner})");
                assert!(names.contains_key(&vertex), "{vertex}");
            }
        }
        let end_face = format!("{feature}.{kind}.Face(G ssssssss.sketch.Face1)");
        assert_at(&names, &end_face, [4.0, 4.0, end], [6.0, 6.0, end]);
        // The cube's 6 faces, 12 edges, and 8 vertices, and the sweep's 5, 12, and 8: no
        // other element.
        assert_eq!(names.len(), 26 + 25, "{names:#?}");
    }
}

#[test]
fn a_pocket_made_through_keeps_its_walls_names_and_its_rims_through_the_bottom_are_sections() {
    // Design S9: the same pocket made through: the same wall names, the floor gone, the
    // bottom b.box.Face5 keeping its name, and the rim there a section edge, generated by
    // the cut from the bottom and the wall.
    let engine = engine();
    let model = publish(
        &engine,
        &[
            &cube_below(),
            &square(),
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", THROUGH),
        ],
    );
    assert_eq!(errors(&model), []);
    let names = named(&model, "qqqqqqqq");
    assert_at(
        &names,
        "bbbbbbbb.box.Face5",
        [0.0, 0.0, -10.0],
        [10.0, 10.0, -10.0],
    );
    assert_at(
        &names,
        "bbbbbbbb.box.Face6",
        [0.0, 0.0, 0.0],
        [10.0, 10.0, 0.0],
    );
    assert!(!names.contains_key("qqqqqqqq.pocket.Face(G ssssssss.sketch.Face1)"));
    for (side, from, to) in sides(0.0) {
        let wall = format!("qqqqqqqq.pocket.Face(G ssssssss.sketch.Edge.{side})");
        assert_at(
            &names,
            &wall,
            [from[0], from[1], -10.0],
            [to[0], to[1], 0.0],
        );
        let rim = format!("qqqqqqqq.pocket.Edge(M ssssssss.sketch.Edge.{side})");
        assert_at(&names, &rim, from, to);
        let section = format!(
            "qqqqqqqq.pocket.Edge(G bbbbbbbb.box.Face5, G qqqqqqqq.pocket.Face(G ssssssss.sketch.Edge.{side}))"
        );
        assert_at(
            &names,
            &section,
            [from[0], from[1], -10.0],
            [to[0], to[1], -10.0],
        );
        // The floor's edges are gone.
        assert!(!names.contains_key(&format!(
            "qqqqqqqq.pocket.Edge(G ssssssss.sketch.Edge.{side})"
        )));
    }
    for (corner, x, y) in corners() {
        let edge = format!("qqqqqqqq.pocket.Edge(G ssssssss.sketch.Vertex.{corner})");
        assert_at(&names, &edge, [x, y, -10.0], [x, y, 0.0]);
    }
    // A chamfer of the blind pocket's floor edge, the pocket then made through: the edge is
    // gone, and the reference breaks, loudly, its section rim among the candidates.
    let floor_edge = "qqqqqqqq.pocket.Edge(G ssssssss.sketch.Edge.na000001)";
    let chamfer = format!(
        r#"{{"id": "cccccccc", "type": "chamfer", "v": 1, "name": "C", "base": "qqqqqqqq", "edges": ["{floor_edge}"], "size": {{"distance": 0.25}}}}"#
    );
    let blind = pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", &one_side("5"));
    let model = publish(&engine, &[&cube_below(), &square(), &blind, &chamfer]);
    assert_eq!(errors(&model), []);
    // A chamfer of an edge 2 long between two faces at a right angle, by d: d²/2 × 2, on
    // the outside of the pocket's floor (a concave edge: added).
    let pocketed = 1000.0 - 2.0 * 2.0 * 5.0;
    assert_close(
        volume(&model, "cccccccc"),
        pocketed + 0.25 * 0.25 / 2.0 * 2.0,
        "chamfer",
    );
    let through = pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", THROUGH);
    let model = publish(&engine, &[&cube_below(), &square(), &through, &chamfer]);
    let Some(FeatureOutcome::Broken { errors, total }) = model.outcome(&id("cccccccc")) else {
        panic!("{:?}", model.outcome(&id("cccccccc")));
    };
    assert_eq!(*total, 1);
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    let section = "qqqqqqqq.pocket.Edge(G bbbbbbbb.box.Face5, G qqqqqqqq.pocket.Face(G ssssssss.sketch.Edge.na000001))";
    assert!(
        errors[0].candidates().iter().any(|c| c == section),
        "{:?}",
        errors[0].candidates()
    );
}

#[test]
fn a_pocket_through_a_slot_ranks_its_split_walls_and_a_bare_reference_breaks_with_both() {
    // Design S9: the cube with a tunnel along x through it (y 2 to 8, z −6 to −4), and a
    // 4 mm square from (3, 3) pocketed through: each wall and each vertical edge is split
    // by the tunnel, into a piece above it (z −4 to 0, ranked 1 by the cut's list) and one
    // below (z −10 to −6, 2).
    let engine = engine();
    let tunnel = box_at("tttttttt", [-1.0, 2.0, -6.0], 12.0, 6.0, 2.0);
    let slotted = r#"{"id": "dddddddd", "type": "cut", "v": 1, "name": "Slot", "base": "bbbbbbbb", "tool": "tttttttt"}"#;
    let wide = rectangle("ssssssss", "xy", [3.0, 3.0], 4.0, 4.0);
    let features = |extra: &str| {
        let mut list = vec![
            cube_below(),
            tunnel.clone(),
            slotted.to_owned(),
            wide.clone(),
            pocket("qqqqqqqq", "dddddddd", "ssssssss", THROUGH),
        ];
        if !extra.is_empty() {
            list.push(extra.to_owned());
        }
        list
    };
    let list = features("");
    let refs: Vec<&str> = list.iter().map(String::as_str).collect();
    let model = publish(&engine, &refs);
    assert_eq!(errors(&model), []);
    let names = named(&model, "qqqqqqqq");
    let walls = [
        ("na000001", [3.0, 3.0], [7.0, 3.0]),
        ("na000002", [7.0, 3.0], [7.0, 7.0]),
        ("na000003", [3.0, 7.0], [7.0, 7.0]),
        ("na000004", [3.0, 3.0], [3.0, 7.0]),
    ];
    for (side, [x0, y0], [x1, y1]) in walls {
        let wall = format!("qqqqqqqq.pocket.Face(G ssssssss.sketch.Edge.{side})");
        assert!(!names.contains_key(&wall), "{wall} isn't one face now");
        for (rank, low, high) in [("1/2", -4.0, 0.0), ("2/2", -10.0, -6.0)] {
            let piece = format!("qqqqqqqq.pocket.Face(M{rank} {wall})");
            assert_at(&names, &piece, [x0, y0, low], [x1, y1, high]);
        }
    }
    for (corner, x, y) in [
        ("pa000001", 3.0, 3.0),
        ("pa000002", 7.0, 3.0),
        ("pa000003", 7.0, 7.0),
        ("pa000004", 3.0, 7.0),
    ] {
        let edge = format!("qqqqqqqq.pocket.Edge(G ssssssss.sketch.Vertex.{corner})");
        for (rank, low, high) in [("1/2", -4.0, 0.0), ("2/2", -10.0, -6.0)] {
            let piece = format!("qqqqqqqq.pocket.Edge(M{rank} {edge})");
            assert_at(&names, &piece, [x, y, low], [x, y, high]);
        }
    }
    // A fillet of the vertical edge by its one-piece name: broken, with both pieces as
    // candidates, and nothing taken for it.
    let edge = "qqqqqqqq.pocket.Edge(G ssssssss.sketch.Vertex.pa000001)";
    let fillet = format!(
        r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "qqqqqqqq", "edges": ["{edge}"], "radius": 0.5}}"#
    );
    let list = features(&fillet);
    let refs: Vec<&str> = list.iter().map(String::as_str).collect();
    let model = publish(&engine, &refs);
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("ffffffff")) else {
        panic!("{:?}", model.outcome(&id("ffffffff")));
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    let candidates: BTreeSet<&str> = errors[0].candidates().iter().map(String::as_str).collect();
    for rank in ["1/2", "2/2"] {
        let piece = format!("qqqqqqqq.pocket.Edge(M{rank} {edge})");
        assert!(
            candidates.contains(piece.as_str()),
            "{piece}: {candidates:?}"
        );
    }
}

#[test]
fn a_pocket_from_below_through_a_block_cuts_section_rims_on_both_its_faces() {
    // Design S9: a profile whose sweep passes through faces of the base. The cube from
    // z = 2 to 12, the square pocketed through from the XY plane, reversed (up): both rims
    // are section edges, of the bottom (Face5, z = 2) and the top (Face6, z = 12).
    let engine = engine();
    let above = box_at("bbbbbbbb", [0.0, 0.0, 2.0], 10.0, 10.0, 10.0);
    let up = r#"{"type": "through_all", "reversed": true}"#;
    let model = publish(
        &engine,
        &[
            &above,
            &square(),
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", up),
        ],
    );
    assert_eq!(errors(&model), []);
    assert_close(
        volume(&model, "qqqqqqqq"),
        1000.0 - 4.0 * 10.0,
        "the pocket",
    );
    let names = named(&model, "qqqqqqqq");
    for (side, from, to) in sides(0.0) {
        for (face, z) in [("Face5", 2.0), ("Face6", 12.0)] {
            let section = format!(
                "qqqqqqqq.pocket.Edge(G bbbbbbbb.box.{face}, G qqqqqqqq.pocket.Face(G ssssssss.sketch.Edge.{side}))"
            );
            assert_at(&names, &section, [from[0], from[1], z], [to[0], to[1], z]);
        }
        let wall = format!("qqqqqqqq.pocket.Face(G ssssssss.sketch.Edge.{side})");
        assert_at(&names, &wall, [from[0], from[1], 2.0], [to[0], to[1], 12.0]);
    }
}

#[test]
fn a_whole_turns_and_a_360_grooves_seams_are_named_and_keep_their_names() {
    // Design S9: every seam of a full revolve and of a 360° groove named. A whole turn's
    // edge that sweeps a face of revolution starts at that face's seam (OCCT's FirstShape,
    // docs/ports/history.md), so the seam is named from its curve, M, by history. The
    // revolve: the rectangle x 2 to 4, y 0 to 3 on XZ, about the sketch's y axis (the
    // model's z): a tube, its outer seam at x = 4 from na000002, its inner at x = 2 from
    // na000004, its four circles from the corners, its ends from the square sides.
    let engine = engine();
    let ring = rectangle("ssssssss", "xz", [2.0, 0.0], 2.0, 3.0);
    let revolve = turned("rrrrrrrr", "revolve", None, "ssssssss", "y_axis", TURN);
    let model = publish(&engine, &[&ring, &revolve]);
    assert_eq!(errors(&model), []);
    assert_close(
        volume(&model, "rrrrrrrr"),
        PI * (16.0 - 4.0) * 3.0,
        "the tube",
    );
    let names = named(&model, "rrrrrrrr");
    assert_at(
        &names,
        "rrrrrrrr.revolve.Edge(M ssssssss.sketch.Edge.na000002)",
        [4.0, 0.0, 0.0],
        [4.0, 0.0, 3.0],
    );
    assert_at(
        &names,
        "rrrrrrrr.revolve.Edge(M ssssssss.sketch.Edge.na000004)",
        [2.0, 0.0, 0.0],
        [2.0, 0.0, 3.0],
    );
    for (corner, radius, z) in [
        ("pa000001", 2.0, 0.0),
        ("pa000002", 4.0, 0.0),
        ("pa000003", 4.0, 3.0),
        ("pa000004", 2.0, 3.0),
    ] {
        let circle = format!("rrrrrrrr.revolve.Edge(G ssssssss.sketch.Vertex.{corner})");
        let Some(Some([low, high])) = names.get(&circle) else {
            panic!("{circle}");
        };
        // A circle about the z axis, of the corner's radius, at its height (its mesh's
        // points are on it, so its box reaches the radius at x = r).
        assert!(
            (high[0] - radius).abs() <= 1e-9 && low[2] == z && high[2] == z,
            "{circle}"
        );
        let vertex = format!("rrrrrrrr.revolve.Vertex(M ssssssss.sketch.Vertex.{corner})");
        assert!(names.contains_key(&vertex), "{vertex}");
    }
    for side in ["na000001", "na000002", "na000003", "na000004"] {
        let face = format!("rrrrrrrr.revolve.Face(G ssssssss.sketch.Edge.{side})");
        assert!(names.contains_key(&face), "{face}");
    }
    // 4 faces, 6 edges (4 circles, 2 seams), 4 vertices: no other element.
    assert_eq!(names.len(), 14, "{names:#?}");
    // The same document rebuilt by another engine names alike.
    let other = self::engine();
    assert_eq!(
        named(&publish(&other, &[&ring, &revolve]), "rrrrrrrr"),
        names
    );

    // The groove: a cylinder r 10, 20 high, grooved 360° by the 4 mm square from (8, 8) on
    // XZ about its axis: the groove's inner seam at x = 8 from na000004, the cylinder's seam
    // cut in two (above and below the groove), the side in two.
    let groove = |height: &str| {
        let cylinder = format!(
            r#"{{"id": "yyyyyyyy", "type": "cylinder", "v": 1, "name": "C", "position": [0, 0, 0], "radius": 10, "height": {height}}}"#
        );
        let profile = rectangle("ssssssss", "xz", [8.0, 8.0], 4.0, 4.0);
        let groove = turned(
            "gggggggg",
            "groove",
            Some("yyyyyyyy"),
            "ssssssss",
            "y_axis",
            TURN,
        );
        publish(&engine, &[&cylinder, &profile, &groove])
    };
    let model = groove("20");
    assert_eq!(errors(&model), []);
    // The ring removed: π (10² − 8²) × 4.
    assert_close(
        volume(&model, "gggggggg"),
        PI * 100.0 * 20.0 - PI * 36.0 * 4.0,
        "the groove",
    );
    let names = named(&model, "gggggggg");
    assert_at(
        &names,
        "gggggggg.groove.Edge(M ssssssss.sketch.Edge.na000004)",
        [8.0, 0.0, 8.0],
        [8.0, 0.0, 12.0],
    );
    assert_at(
        &names,
        "gggggggg.groove.Edge(M1/2 yyyyyyyy.cylinder.Edge1)",
        [10.0, 0.0, 0.0],
        [10.0, 0.0, 8.0],
    );
    assert_at(
        &names,
        "gggggggg.groove.Edge(M2/2 yyyyyyyy.cylinder.Edge1)",
        [10.0, 0.0, 12.0],
        [10.0, 0.0, 20.0],
    );
    for (section, z) in [("na000001", 8.0), ("na000003", 12.0)] {
        let rim = format!(
            "gggggggg.groove.Edge(G yyyyyyyy.cylinder.Face1, G gggggggg.groove.Face(G ssssssss.sketch.Edge.{section}))"
        );
        let Some(Some([low, high])) = names.get(&rim) else {
            panic!("{rim}");
        };
        assert!(
            low[2] == z && high[2] == z && (high[0] - 10.0).abs() <= 1e-9,
            "{rim}"
        );
    }
    // Rebuilt with the cylinder taller, an edit elsewhere: the groove's own elements keep
    // their names, and the seam above the groove its rank.
    let taller = named(&groove("30"), "gggggggg");
    let own = |names: &BTreeMap<String, Option<Bounds>>| -> BTreeSet<String> {
        names
            .keys()
            .filter(|n| n.starts_with("gggggggg."))
            .cloned()
            .collect()
    };
    assert_eq!(own(&taller), own(&names));
    assert_at(
        &taller,
        "gggggggg.groove.Edge(M2/2 yyyyyyyy.cylinder.Edge1)",
        [10.0, 0.0, 12.0],
        [10.0, 0.0, 30.0],
    );
}

#[test]
fn a_cones_apex_is_a_degenerate_edge_generated_from_its_point() {
    // Design S4: where a face of revolution closes on the axis, MakeRevol makes a
    // degenerate edge, generated from the profile's point there, as any point's side edge
    // is. The triangle (0, 0), (3, 0), (0, 4) on XZ about the sketch's y axis: a cone, its
    // apex pa000003 at (0, 0, 4), half a turn and a whole one.
    let engine = engine();
    let triangle = polygon("ssssssss", "xz", &[[0.0, 0.0], [3.0, 0.0], [0.0, 4.0]]);
    for (angle_text, fraction) in [("180", 0.5), ("360", 1.0)] {
        let revolve = turned(
            "rrrrrrrr",
            "revolve",
            None,
            "ssssssss",
            "y_axis",
            &angle(angle_text),
        );
        let model = publish(&engine, &[&triangle, &revolve]);
        assert_eq!(errors(&model), []);
        assert_close(
            volume(&model, "rrrrrrrr"),
            PI * 9.0 * 4.0 / 3.0 * fraction,
            "the cone",
        );
        let names = named(&model, "rrrrrrrr");
        assert_eq!(
            names.get("rrrrrrrr.revolve.Edge(G ssssssss.sketch.Vertex.pa000003)"),
            Some(&None),
            "the apex's edge has no length, so no points: {names:#?}"
        );
        assert!(names.contains_key("rrrrrrrr.revolve.Vertex(M ssssssss.sketch.Vertex.pa000003)"));
        // The slanted side's face, and the base's.
        assert!(names.contains_key("rrrrrrrr.revolve.Face(G ssssssss.sketch.Edge.na000002)"));
        assert!(names.contains_key("rrrrrrrr.revolve.Face(G ssssssss.sketch.Edge.na000001)"));
        if fraction < 1.0 {
            // Half a turn: its start and end faces, and the axis's edge, its start.
            assert_at(
                &names,
                "rrrrrrrr.revolve.Face(M ssssssss.sketch.Face1)",
                [0.0, 0.0, 0.0],
                [3.0, 0.0, 4.0],
            );
            assert_at(
                &names,
                "rrrrrrrr.revolve.Face(G ssssssss.sketch.Face1)",
                [-3.0, 0.0, 0.0],
                [0.0, 0.0, 4.0],
            );
            assert_at(
                &names,
                "rrrrrrrr.revolve.Edge(M ssssssss.sketch.Edge.na000003)",
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 4.0],
            );
        }
    }
}

/// testConstructionToggleTNP's sketch (FreeCAD's `TestSketcherSolver.py`, bug 15484): the
/// 30 mm square from the origin, its corners pa000001 (0, 0) to pa000004 (0, 30), its sides
/// na000001 (bottom), na000002 (right), na000003 (top), na000004 (left); and the zigzag
/// beside its left side, from (0, 0) through pz000001 (−10, 10), pz000002 (−5, 20), and
/// pz000003 (−10, 25) to (0, 30), lines nz000001 to nz000004. `toggled` makes the zigzag
/// the profile and the left side construction.
fn zigzag(toggled: bool) -> String {
    let point = |id: &str, x: f64, y: f64| {
        format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
    };
    let line = |id: &str, from: &str, to: &str, construction: bool| {
        format!(
            r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": {construction}}}"#
        )
    };
    let points = [
        point("pa000001", 0.0, 0.0),
        point("pa000002", 30.0, 0.0),
        point("pa000003", 30.0, 30.0),
        point("pa000004", 0.0, 30.0),
        point("pz000001", -10.0, 10.0),
        point("pz000002", -5.0, 20.0),
        point("pz000003", -10.0, 25.0),
    ];
    let lines = [
        line("na000001", "pa000001", "pa000002", false),
        line("na000002", "pa000002", "pa000003", false),
        line("na000003", "pa000003", "pa000004", false),
        line("na000004", "pa000004", "pa000001", toggled),
        line("nz000001", "pa000001", "pz000001", !toggled),
        line("nz000002", "pz000001", "pz000002", !toggled),
        line("nz000003", "pz000002", "pz000003", !toggled),
        line("nz000004", "pz000003", "pa000004", !toggled),
    ];
    format!(
        r#"{{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        lines.join(", ")
    )
}

#[test]
fn toggling_construction_lines_keeps_a_chamfer_on_the_pads_top_edge() {
    // testConstructionToggleTNP's essence (design S9): the square padded 10 (9000); a
    // chamfer of size 1 on the top edge over the square's top side; the top face and the
    // right side's face by name. Then the zigzag made profile and the left side
    // construction: the pad has 9 faces and 9000 + 187.5 × 10 = 10875 (the zigzag's area
    // by the shoelace formula, worked by hand: 187.5), the chamfer still on the top edge,
    // removing 1²/2 × 30 and the 1/3 its slanted end adds (below), and the top face and the
    // right side's face are still named, at z = 10 and x = 30.
    // The left side's face is gone, and a reference to its top edge breaks, never moved.
    let engine = engine();
    let top_edge = "pppppppp.pad.Edge(G ssssssss.sketch.Edge.na000003)";
    let chamfer = |edge: &str| {
        format!(
            r#"{{"id": "cccccccc", "type": "chamfer", "v": 1, "name": "C", "base": "pppppppp", "edges": ["{edge}"], "size": {{"distance": 1}}}}"#
        )
    };
    let pad = pad("pppppppp", None, "ssssssss", TEN);
    // Toggled, the top edge's end at (0, 30) meets the zigzag's last side, from (−10, 25),
    // not a square side: the chamfer runs on to that side's plane, x = 2 (y − 30), past
    // x = 0 by 2 (30 − y) across its section (u = 30 − y, v = 10 − z, u + v ≤ 1), so it
    // takes ∫ 2u dA = 2 × 1/6 = 1/3 more (worked by hand), as FreeCAD's chamfer does.
    for (toggled, area, faces, beyond) in [(false, 900.0, 6, 0.0), (true, 1087.5, 9, 1.0 / 3.0)] {
        let sketch = zigzag(toggled);
        let model = publish(&engine, &[&sketch, &pad, &chamfer(top_edge)]);
        assert_eq!(errors(&model), [], "toggled {toggled}");
        let padded = area * 10.0;
        assert_close(volume(&model, "pppppppp"), padded, "the pad");
        assert_close(
            volume(&model, "cccccccc"),
            padded - 0.5 * 30.0 - beyond,
            "the chamfer",
        );
        let names = named(&model, "pppppppp");
        assert_eq!(names.keys().filter(|n| n.contains(".Face")).count(), faces);
        assert_at(&names, top_edge, [0.0, 30.0, 10.0], [30.0, 30.0, 10.0]);
        let top = "pppppppp.pad.Face(G ssssssss.sketch.Face1)";
        let low_x = if toggled { -10.0 } else { 0.0 };
        assert_at(&names, top, [low_x, 0.0, 10.0], [30.0, 30.0, 10.0]);
        let right = "pppppppp.pad.Face(G ssssssss.sketch.Edge.na000002)";
        assert_at(&names, right, [30.0, 0.0, 0.0], [30.0, 30.0, 10.0]);
        let left = "pppppppp.pad.Face(G ssssssss.sketch.Edge.na000004)";
        assert_eq!(names.contains_key(left), !toggled);
    }
    // The left side's top edge, chamfered, then toggled away: broken, not moved.
    let left_edge = "pppppppp.pad.Edge(G ssssssss.sketch.Edge.na000004)";
    let model = publish(&engine, &[&zigzag(false), &pad, &chamfer(left_edge)]);
    assert_eq!(errors(&model), []);
    let model = publish(&engine, &[&zigzag(true), &pad, &chamfer(left_edge)]);
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("cccccccc")) else {
        panic!("{:?}", model.outcome(&id("cccccccc")));
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
}

#[test]
fn a_pocket_made_a_pad_under_its_id_breaks_references_by_its_type_word() {
    // N8's rule for sweep features (design S4): every type word is the document's type, so
    // a pocket made a pad under its ID breaks every reference to its elements, loudly, with
    // the pad's names as candidates.
    let engine = engine();
    let floor_edge = "qqqqqqqq.pocket.Edge(G ssssssss.sketch.Edge.na000001)";
    let chamfer = format!(
        r#"{{"id": "cccccccc", "type": "chamfer", "v": 1, "name": "C", "base": "qqqqqqqq", "edges": ["{floor_edge}"], "size": {{"distance": 0.25}}}}"#
    );
    let model = publish(
        &engine,
        &[
            &cube_below(),
            &square(),
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", &one_side("5")),
            &chamfer,
        ],
    );
    assert_eq!(errors(&model), []);
    let model = publish(
        &engine,
        &[
            &cube_below(),
            &square(),
            &pad("qqqqqqqq", Some("bbbbbbbb"), "ssssssss", &one_side("5")),
            &chamfer,
        ],
    );
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("cccccccc")) else {
        panic!("{:?}", model.outcome(&id("cccccccc")));
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    assert!(
        errors[0]
            .message()
            .contains("names an element a pocket made, but qqqqqqqq is a pad now"),
        "{}",
        errors[0].message()
    );
    assert!(
        errors[0]
            .candidates()
            .iter()
            .any(|c| c == "qqqqqqqq.pad.Edge(G ssssssss.sketch.Edge.na000001)"),
        "{:?}",
        errors[0].candidates()
    );
}

#[test]
fn a_fillet_of_a_pads_top_edge_takes_its_share_and_follows_the_sketch() {
    // Design S9: a fillet of a pad's top edge, named p.pad.Edge(G s.sketch.Edge.c), removes
    // (1 − π/4) r² ℓ, ℓ the edge's length (a square corner's area less the quarter disc,
    // along the edge), and keeps doing so when the sketch's points move to another length.
    // A pad of no base: the rectangle w by 10 on XY, padded 5, its top edge over the bottom
    // side (na000001, w long), r = 1.
    let engine = engine();
    let top_edge = "pppppppp.pad.Edge(G ssssssss.sketch.Edge.na000001)";
    let fillet = format!(
        r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "pppppppp", "edges": ["{top_edge}"], "radius": 1}}"#
    );
    for w in [20.0, 30.0] {
        let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], w, 10.0);
        let model = publish(
            &engine,
            &[
                &sketch,
                &pad("pppppppp", None, "ssssssss", &one_side("5")),
                &fillet,
            ],
        );
        assert_eq!(errors(&model), []);
        let padded = w * 10.0 * 5.0;
        assert_close(volume(&model, "pppppppp"), padded, "the pad");
        assert_close(
            volume(&model, "ffffffff"),
            padded - (1.0 - PI / 4.0) * w,
            "the fillet",
        );
    }
}

#[test]
fn a_fillet_of_a_flush_pads_rim_adds_the_concave_share() {
    // Design S9: the rim where a pad meets the face its sketch lies in is the sweep's start
    // edge, p.pad.Edge(M s.sketch.Edge.c), and a fillet of it adds (1 − π/4) r² ℓ: the
    // square corner's area less the quarter disc, along the edge's length (2), as the
    // fillet fills the concave corner. The 2 mm square padded 5 onto the cube, r = 0.5.
    let engine = engine();
    let rim = "pppppppp.pad.Edge(M ssssssss.sketch.Edge.na000001)";
    let fillet = format!(
        r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "pppppppp", "edges": ["{rim}"], "radius": 0.5}}"#
    );
    let model = publish(
        &engine,
        &[
            &cube_below(),
            &square(),
            &pad("pppppppp", Some("bbbbbbbb"), "ssssssss", &one_side("5")),
            &fillet,
        ],
    );
    assert_eq!(errors(&model), []);
    let padded = 1000.0 + 2.0 * 2.0 * 5.0;
    assert_close(volume(&model, "pppppppp"), padded, "the pad");
    assert_close(
        volume(&model, "ffffffff"),
        padded + (1.0 - PI / 4.0) * 0.25 * 2.0,
        "the fillet",
    );
}

/// The area a fillet of radius `r` takes from the corner where a plane through a cylinder's
/// axis meets the cylinder (radius `big`) square: worked by hand. The fillet's circle is
/// tangent to the plane at distance `c` = √((R − r)² − r²) from the axis and to the
/// cylinder at angle θ = atan(r / c), along its centre's direction; the corner's region
/// is the cylinder's sector to θ less the triangle (axis, tangent point on the plane,
/// centre) and the fillet's sector from the plane round to the cylinder (π/2 + θ):
/// R²θ/2 − c·r/2 − r²(π/2 + θ)/2. (A square corner's (1 − π/4) r² is its limit as R
/// grows; at R = 4, r = 0.5 it's 0.0665 against 0.0537.)
fn rim_on_a_cylinder(big: f64, r: f64) -> f64 {
    let c = ((big - r) * (big - r) - r * r).sqrt();
    let theta = libm::atan2(r, c);
    big * big * theta / 2.0 - c * r / 2.0 - r * r * (PI / 2.0 + theta) / 2.0
}

#[test]
fn a_whole_turns_seam_is_the_start_edge_a_turn_short_of_whole_names_alike() {
    // Fable's decision (design S4): a whole turn's seam is `Edge(M c)`, OCCT's FirstShape of
    // the curve, the same object a partial turn's start edge is. One document, the tube of
    // the whole-turn test, at 360°, 359°, and 360° again, one side: the outer side's seam
    // and the 359° turn's start edge are one name at x = 4, y = 0, z 0..3; the end edge and
    // the caps exist only short of a whole turn. A fillet of that edge (r 0.5) builds at
    // 359°, taking the cap-and-cylinder corner's area along its 3 mm (`rim_on_a_cylinder`),
    // and at 360° is refused as the kernel refuses a seam, naming it; fillets of the end
    // edge and of an edge square to the axis break, as their edges are gone.
    let engine = engine();
    let ring = rectangle("ssssssss", "xz", [2.0, 0.0], 2.0, 3.0);
    let start = "rrrrrrrr.revolve.Edge(M ssssssss.sketch.Edge.na000002)";
    let end = "rrrrrrrr.revolve.Edge(G ssssssss.sketch.Edge.na000002)";
    let square = "rrrrrrrr.revolve.Edge(M ssssssss.sketch.Edge.na000001)";
    let fillet = |edge: &str| {
        format!(
            r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "rrrrrrrr", "edges": ["{edge}"], "radius": 0.5}}"#
        )
    };
    let revolve = |angle_text: &str| {
        turned(
            "rrrrrrrr",
            "revolve",
            None,
            "ssssssss",
            "y_axis",
            &angle(angle_text),
        )
    };
    let mut whole_names = None;
    for angle_text in ["360", "359", "360"] {
        let turn = revolve(angle_text);
        let model = publish(&engine, &[&ring, &turn]);
        assert_eq!(errors(&model), []);
        let names = named(&model, "rrrrrrrr");
        assert_at(&names, start, [4.0, 0.0, 0.0], [4.0, 0.0, 3.0]);
        let whole = angle_text == "360";
        for partial in [
            end,
            "rrrrrrrr.revolve.Face(M ssssssss.sketch.Face1)",
            "rrrrrrrr.revolve.Face(G ssssssss.sketch.Face1)",
            square,
        ] {
            assert_eq!(
                names.contains_key(partial),
                !whole,
                "{partial} at {angle_text}°"
            );
        }
        if whole {
            // The same names, element for element, both times.
            match &whole_names {
                None => whole_names = Some(names),
                Some(before) => assert_eq!(&names, before),
            }
            let model = publish(&engine, &[&ring, &turn, &fillet(start)]);
            let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("ffffffff")) else {
                panic!("{:?}", model.outcome(&id("ffffffff")));
            };
            // As the kernel refuses a seam (`Trouble::Smooth`): its input, naming the edge
            // by its name, never a broken reference.
            assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
            assert_eq!(error.elements(), [start]);
            assert!(error.message().contains("at a seam"), "{error:?}");
            for gone in [end, square] {
                let model = publish(&engine, &[&ring, &turn, &fillet(gone)]);
                let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("ffffffff"))
                else {
                    panic!("{gone}: {:?}", model.outcome(&id("ffffffff")));
                };
                assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
            }
        } else {
            let model = publish(&engine, &[&ring, &turn, &fillet(start)]);
            assert_eq!(errors(&model), []);
            let turned = PI * (16.0 - 4.0) * 3.0 * 359.0 / 360.0;
            assert_close(volume(&model, "rrrrrrrr"), turned, "the turn");
            assert_close(
                volume(&model, "ffffffff"),
                turned - rim_on_a_cylinder(4.0, 0.5) * 3.0,
                "the fillet of the start edge",
            );
        }
    }
}

#[test]
fn a_symmetric_whole_turns_seam_and_its_start_edge_short_of_whole_name_alike() {
    // The same, symmetric: the turn starts at minus half its angle, so at 360° the seam is
    // at −180° (x = −4) and at 359° the start edge at −179.5°, both `Edge(M c)`.
    let engine = engine();
    let ring = rectangle("ssssssss", "xz", [2.0, 0.0], 2.0, 3.0);
    let start = "rrrrrrrr.revolve.Edge(M ssssssss.sketch.Edge.na000002)";
    for degrees in [360.0f64, 359.0] {
        let extent = format!(r#"{{"type": "symmetric", "angle": {degrees}}}"#);
        let turn = turned("rrrrrrrr", "revolve", None, "ssssssss", "y_axis", &extent);
        let model = publish(&engine, &[&ring, &turn]);
        assert_eq!(errors(&model), []);
        let names = named(&model, "rrrrrrrr");
        let at = -(degrees * (PI / 180.0)) / 2.0;
        let (x, y) = (4.0 * libm::cos(at), 4.0 * libm::sin(at));
        assert_at(&names, start, [x, y, 0.0], [x, y, 3.0]);
    }
}

#[test]
fn a_groove_whose_regions_lie_either_side_of_its_axis_cuts_both() {
    // Council round 1, proven against FreeCAD 1.1.3, which checks each face's outer wire
    // alone: a pad of a circle r 20 by 20, grooved a quarter turn about the sketch's y axis
    // by two rectangles on XZ, x 15..25 and x −25..−15, both z 5..10. Each turns through
    // the ring 15 to 20 of the cylinder, a quarter of it, 5 high: 2 × (π/4)(20² − 15²) × 5,
    // 1374.446786, FreeCAD's measure too.
    let engine = engine();
    let two = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xz", "points": [{"id": "pa000001", "x": 15, "y": 5, "construction": false}, {"id": "pa000002", "x": 25, "y": 5, "construction": false}, {"id": "pa000003", "x": 25, "y": 10, "construction": false}, {"id": "pa000004", "x": 15, "y": 10, "construction": false}, {"id": "pb000001", "x": -25, "y": 5, "construction": false}, {"id": "pb000002", "x": -15, "y": 5, "construction": false}, {"id": "pb000003", "x": -15, "y": 10, "construction": false}, {"id": "pb000004", "x": -25, "y": 10, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}, {"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": false}, {"id": "nb000002", "from": "pb000002", "to": "pb000003", "construction": false}, {"id": "nb000003", "from": "pb000003", "to": "pb000004", "construction": false}, {"id": "nb000004", "from": "pb000004", "to": "pb000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
    let model = publish(
        &engine,
        &[
            &circles("cccccccc", "xy", &[([0.0, 0.0], 20.0)]),
            &pad("pppppppp", None, "cccccccc", &one_side("20")),
            two,
            &turned(
                "gggggggg",
                "groove",
                Some("pppppppp"),
                "ssssssss",
                "y_axis",
                &angle("90"),
            ),
        ],
    );
    assert_eq!(errors(&model), []);
    let cylinder = PI * 400.0 * 20.0;
    let removed = 2.0 * (PI / 4.0) * (400.0 - 225.0) * 5.0;
    assert!((removed - 1374.446786).abs() < 1e-6);
    assert_close(volume(&model, "gggggggg"), cylinder - removed, "the groove");
}

#[test]
fn a_groove_of_regions_either_side_of_its_axis_turned_whole_cuts_what_both_sweep() {
    // Council round 2's critical: a pad of a circle r 20 by 20, grooved a whole turn by
    // rectangles A (x 15..25, z 5..10) and B (x −25..−15, z 7..12) about the sketch's y
    // axis. Their swept solids overlap (z 7..10), and cut as one compound tool OCCT removed
    // 2748.893572. Cut as separate tools, as FreeCAD's makeElementBoolean expands a compound
    // tool (docs/ports/sketch-features.md), the groove removes the union, the ring 15..20
    // from z 5 to 12: π (20² − 15²) × 7, leaving 21284.290228, FreeCAD 1.1.3's measure.
    // And two regions on one side (x 15..25, z 5..10 and z 12..15), whose solids don't
    // overlap: the rings 15..20 from z 5 to 10 and 12 to 15, π (20² − 15²) × 8.
    let engine = engine();
    let groove = |rects: &[[f64; 4]]| {
        let model = publish(
            &engine,
            &[
                &circles("cccccccc", "xy", &[([0.0, 0.0], 20.0)]),
                &pad("pppppppp", None, "cccccccc", &one_side("20")),
                &rectangles("ssssssss", "xz", rects),
                &turned(
                    "gggggggg",
                    "groove",
                    Some("pppppppp"),
                    "ssssssss",
                    "y_axis",
                    TURN,
                ),
            ],
        );
        assert_eq!(errors(&model), []);
        volume(&model, "gggggggg")
    };
    let cylinder = PI * 400.0 * 20.0;
    let want = cylinder - PI * 175.0 * 7.0;
    assert!((want - 21284.290228).abs() < 1e-6);
    assert_close(
        groove(&[[15.0, 5.0, 25.0, 10.0], [-25.0, 7.0, -15.0, 12.0]]),
        want,
        "the whole turn",
    );
    assert_close(
        groove(&[[15.0, 5.0, 25.0, 10.0], [15.0, 12.0, 25.0, 15.0]]),
        cylinder - PI * 175.0 * 8.0,
        "the whole turn of two regions on one side",
    );
}

#[test]
fn a_two_region_grooves_names_dont_depend_on_the_order_of_its_tools() {
    // The cut takes each region's solid as a tool of its own, in the regions' order (their
    // smallest curve IDs). Renaming B's curves so it comes first changes that order, and
    // every name of A's elements finds the same place: names come from each tool's own
    // elements, never from where OCCT lists the tools.
    let engine = engine();
    let groove = |b: &str| {
        let sketch = rectangles(
            "ssssssss",
            "xz",
            &[[15.0, 5.0, 25.0, 10.0], [-25.0, 7.0, -15.0, 12.0]],
        )
        .replace("nb0", b);
        let model = publish(
            &engine,
            &[
                &circles("cccccccc", "xy", &[([0.0, 0.0], 20.0)]),
                &pad("pppppppp", None, "cccccccc", &one_side("20")),
                &sketch,
                &turned(
                    "gggggggg",
                    "groove",
                    Some("pppppppp"),
                    "ssssssss",
                    "y_axis",
                    TURN,
                ),
            ],
        );
        assert_eq!(errors(&model), []);
        named(&model, "gggggggg")
    };
    // B's lines nb0000n1.., or n00000n (before A's na0000n).
    let (after, before) = (groove("nb0"), groove("n00"));
    let of_a = |names: &BTreeMap<String, Option<Bounds>>| -> BTreeMap<String, Option<Bounds>> {
        names
            .iter()
            .filter(|(n, _)| {
                n.contains("sketch.Edge.na")
                    && !n.contains("sketch.Edge.n00")
                    && !n.contains("sketch.Edge.nb")
            })
            .map(|(n, b)| (n.clone(), *b))
            .collect()
    };
    assert!(!of_a(&after).is_empty());
    assert_eq!(of_a(&after), of_a(&before));
}
