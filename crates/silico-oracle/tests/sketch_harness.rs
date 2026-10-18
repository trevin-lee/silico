//! The sketch oracle's harness (M3.3): the FreeCAD model it builds from a Silico sketch, and
//! that it catches a wrong answer from either side. The models expected are written out by
//! hand from FreeCAD's `Sketcher.Constraint` arguments.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::{FRAC_PI_2, PI};

use silico_core::document::{Operation, Sketch, read};
use silico_core::sketch::{SketchWorker, Translation};
use silico_oracle::sketch::{
    FreecadConstraint, FreecadGeometry, FreecadSketch, SketchCase, compare, freecad_sketch,
    freecad_solve, silico_solve,
};

/// A document of one sketch with these fields (the JSON after the feature's name).
fn sketch(fields: &str) -> Sketch {
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"s0000001\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", \"plane\": \"xy\", {fields}}}]}}"
    );
    match &read(text.as_bytes()).unwrap().features[0].operation {
        Operation::Sketch(s) => s.clone(),
        other => panic!("{other:?}"),
    }
}

/// Two lines from p1 (0, 0): n1 to p2 (10, 0) and n2 to p3 (0, 8); n1 horizontal, 10 long,
/// p1 at the origin, and these constraints.
fn corner(more: &str) -> Sketch {
    sketch(&format!(
        r#""points": [{{"id": "p0000001", "x": 0, "y": 0, "construction": false}}, {{"id": "p0000002", "x": 10, "y": 0, "construction": false}}, {{"id": "p0000003", "x": 0, "y": 8, "construction": false}}], "lines": [{{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}}, {{"id": "n0000002", "from": "p0000001", "to": "p0000003", "construction": false}}], "circles": [], "arcs": [], "constraints": [{{"id": "k0000001", "type": "horizontal", "line": "n0000001"}}, {{"id": "k0000002", "type": "length", "line": "n0000001", "value": 10, "driving": true}}, {{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}}{more}]"#
    ))
}

fn model(s: &Sketch) -> FreecadSketch {
    freecad_sketch(s, &Translation::new(s).unwrap()).unwrap()
}

fn constraint(
    kind: &'static str,
    ints: Vec<i32>,
    value: Option<f64>,
    id: &str,
) -> FreecadConstraint {
    FreecadConstraint {
        kind,
        ints,
        value,
        driving: true,
        id: Some(id.to_owned()),
        locked: None,
    }
}

#[test]
fn a_shared_point_is_freecads_coincident_ends_after_silicos_constraints() {
    let m = model(&corner(""));
    assert_eq!(
        m.geometry,
        [
            FreecadGeometry::Line {
                start: [0.0, 0.0],
                end: [10.0, 0.0]
            },
            FreecadGeometry::Line {
                start: [0.0, 0.0],
                end: [0.0, 8.0]
            },
        ]
    );
    assert_eq!(
        m.constraints,
        [
            constraint("Horizontal", vec![0], None, "k0000001"),
            constraint("Distance", vec![0], Some(10.0), "k0000002"),
            // The origin is FreeCAD's root point, (−1, 1).
            constraint("DistanceX", vec![-1, 1, 0, 1], Some(0.0), "k0000003"),
            // p1 is both lines' start: one coincidence joins them.
            FreecadConstraint {
                kind: "Coincident",
                ints: vec![0, 1, 1, 1],
                value: None,
                driving: true,
                id: None,
                locked: None,
            },
        ]
    );
    assert_eq!(m.points["p0000001"], (0, 1));
    assert_eq!(m.points["p0000003"], (1, 2));
}

#[test]
fn a_tangency_where_curves_meet_is_freecads_own_coincidence() {
    // FreeCAD's endpoint-to-endpoint tangency joins the two ends itself: no coincidence is
    // added for them. Silico's θ of π (n1 starts at p1, n2 starts at p1: smooth means their
    // own directions opposite) is FreeCAD's locked value π − π/2.
    let m = model(&corner(
        r#", {"id": "k0000004", "type": "tangent_at", "a": "n0000001", "b": "n0000002", "at": "p0000001", "direction": "same"}"#,
    ));
    assert!(
        m.constraints.iter().all(|c| c.kind != "Coincident"),
        "{m:?}"
    );
    let tangent = m.constraints.iter().find(|c| c.kind == "Tangent").unwrap();
    assert_eq!(tangent.ints, [0, 1, 1, 1]);
    assert_eq!(tangent.locked, Some(PI - FRAC_PI_2));
}

#[test]
fn every_form_goes_to_freecads_constraint_of_it() {
    // A circle c1 about p4 and an arc a1 about p4 from p2 to p5 beside the corner, and one
    // constraint of each further form; FreeCAD's arguments by hand. Geometry: n1 (0), n2
    // (1), c1 (2), a1 (3).
    let s = sketch(
        r#""points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000003", "x": 0, "y": 8, "construction": false}, {"id": "p0000004", "x": 10, "y": 8, "construction": false}, {"id": "p0000005", "x": 2, "y": 8, "construction": false}],
        "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000002", "from": "p0000001", "to": "p0000003", "construction": false}],
        "circles": [{"id": "c0000001", "center": "p0000004", "radius": 3, "construction": false}],
        "arcs": [{"id": "a0000001", "center": "p0000004", "start": "p0000002", "end": "p0000005", "construction": false}],
        "constraints": [
          {"id": "k0000001", "type": "vertical_points", "a": "p0000003", "b": "origin"},
          {"id": "k0000002", "type": "point_line_distance", "point": "p0000004", "line": "n0000001", "value": 8, "side": "left", "driving": true},
          {"id": "k0000003", "type": "on", "point": "p0000005", "curve": "c0000001"},
          {"id": "k0000004", "type": "angle", "a": "x_axis", "b": "n0000002", "value": 90, "driving": false},
          {"id": "k0000005", "type": "diameter", "curve": "c0000001", "value": 6, "driving": true},
          {"id": "k0000006", "type": "equal", "a": "c0000001", "b": "a0000001"},
          {"id": "k0000007", "type": "tangent_on", "a": "n0000001", "at": "p0000002", "b": "c0000001", "direction": "opposite"},
          {"id": "k0000008", "type": "symmetric", "a": "p0000003", "b": "p0000002", "about": "p0000001"},
          {"id": "k0000009", "type": "midpoint", "point": "p0000005", "line": "n0000002"}
        ]"#,
    );
    let m = model(&s);
    let ours: Vec<_> = m.constraints.iter().filter(|c| c.id.is_some()).collect();
    let want = [
        ("Vertical", vec![1, 2, -1, 1], None),
        ("Distance", vec![2, 3, 0], Some(8.0)),
        ("PointOnObject", vec![3, 2, 2], None),
        ("Angle", vec![-1, 1], Some(FRAC_PI_2)),
        ("Diameter", vec![2], Some(6.0)),
        ("Equal", vec![2, 3], None),
        ("Tangent", vec![0, 2, 2], None),
        ("Symmetric", vec![1, 2, 0, 2, 0, 1], None),
        ("Symmetric", vec![1, 1, 1, 2, 3, 2], None),
    ];
    assert_eq!(ours.len(), want.len());
    for (c, (kind, ints, value)) in ours.iter().zip(want) {
        assert_eq!((c.kind, &c.ints, c.value), (kind, &ints, value), "{c:?}");
    }
    // The reference angle isn't driving; the tangency at n1's end, opposite, is θ = 0,
    // locked at −π/2.
    assert!(!ours[3].driving);
    assert_eq!(ours[6].locked, Some(-FRAC_PI_2));
    // The arc: radius √68 from (10, 8) to (10, 0), from atan2(−8, 0) to atan2(0, −8) = π.
    match m.geometry[3] {
        FreecadGeometry::Arc {
            radius, from, to, ..
        } => {
            assert_eq!(radius, 8.0);
            assert_eq!((from, to), (-FRAC_PI_2, PI));
        }
        ref other => panic!("{other:?}"),
    }
}

fn case(s: Sketch) -> SketchCase {
    SketchCase {
        name: "corner".to_owned(),
        note: String::new(),
        sketch: s,
    }
}

#[test]
fn the_harness_catches_a_wrong_answer_from_either_side() {
    // The corner held fully, solved by both sides: they agree. FreeCAD given a length of 11
    // instead of 10 (a wrong model) puts p2 elsewhere, and the harness says so; FreeCAD's
    // lists made to name a constraint redundant that Silico's don't, and it says so.
    let s = corner(
        r#", {"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000005", "type": "vertical", "line": "n0000002"}, {"id": "k0000006", "type": "length", "line": "n0000002", "value": 8, "driving": true}"#,
    );
    // The sketch worker beside this test, as the oracle's run finds it.
    let mut worker = SketchWorker::start().unwrap();
    let ours = silico_solve(&case(s.clone()), &mut worker).unwrap();
    let good = model(&s);
    let mut wrong = good.clone();
    wrong.constraints[1].value = Some(11.0);
    let theirs = freecad_solve(&[good.clone(), wrong.clone()]).unwrap();
    assert!(
        compare(&good, &ours, &theirs[0]).is_empty(),
        "{:?}",
        compare(&good, &ours, &theirs[0])
    );
    let how = compare(&wrong, &ours, &theirs[1]);
    assert!(
        how.iter().any(|h| h.starts_with("point p0000002")),
        "{how:?}"
    );
    let mut listed = theirs[0].clone();
    listed.redundant = vec![2];
    let how = compare(&good, &ours, &listed);
    assert!(how.iter().any(|h| h.starts_with("redundant")), "{how:?}");
}
