//! M3.3: a document's sketch solved through the sketch worker (silico_core::sketch), from
//! the file to the solution as data. FreeCAD's own solver tests, ported to Silico's forms
//! (shared points for its coincidences), and the cases FreeCAD lacks; expected values are
//! worked out by hand, each where it's asserted.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::systems;
use super::wait::WAIT;

use super::fixture;

use fixture::{arc, circle, line, point};
use std::path::Path;

use silico_core::ErrorCode;
use silico_core::document::{Checked, ElementId, FeatureId, read};
use silico_core::sketch::{SketchSolution, SketchSolver, SketchWorker, solve};
use silico_protocol::Algorithm;
use silico_protocol::{SketchSystem, Solved};
use systems::Rng;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
const FEATURE: &str = "k7f2qx3m";

/// The solver meets each constraint to 1e-10; points land within this of where they must.
const CLOSE: f64 = 1e-8;

fn worker() -> SketchWorker {
    SketchWorker::start_worker(Path::new(WORKER)).unwrap()
}

/// A document of one sketch with these lists.
fn document(
    points: &[String],
    lines: &[String],
    circles: &[String],
    arcs: &[String],
    constraints: &[&str],
) -> Checked {
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"{FEATURE}\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", \"plane\": \"xy\", \"points\": [{}], \"lines\": [{}], \"circles\": [{}], \"arcs\": [{}], \"constraints\": [{}]}}]}}",
        points.join(", "),
        lines.join(", "),
        circles.join(", "),
        arcs.join(", "),
        constraints.join(", ")
    );
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

fn solved(doc: &Checked, w: &mut dyn SketchSolver) -> Result<SketchSolution, silico_core::Error> {
    solve(doc, &FeatureId::new(FEATURE).unwrap(), w)
}

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn assert_at(solution: &SketchSolution, p: &str, want: [f64; 2]) {
    let got = solution.point(&id(p)).unwrap();
    assert!(
        (got[0] - want[0]).abs() <= CLOSE && (got[1] - want[1]).abs() <= CLOSE,
        "{p}: got {got:?}, want {want:?}"
    );
}

#[test]
fn freecads_rectangle_is_fully_constrained_at_its_corners() {
    // TestSketcherSolver.py's CreateRectangleSketch, as the M3.2 design's example has it: a
    // 40 × 20 rectangle with its lower-left corner at the origin, drawn off true; its
    // coincidences are its shared corners.
    let doc = document(
        &[
            point("0a1b2c3d", 0.3, -0.2),
            point("2p3q4r5s", -0.5, 19.0),
            point("4e5f6g7h", 41.0, 0.5),
            point("8j9k0m1n", 39.0, 21.0),
        ],
        &[
            line("6t7v8w9x", "0a1b2c3d", "4e5f6g7h"),
            line("a0b1c2d3", "4e5f6g7h", "8j9k0m1n"),
            line("j8k9m0n1", "8j9k0m1n", "2p3q4r5s"),
            line("p2q3r4s5", "2p3q4r5s", "0a1b2c3d"),
        ],
        &[],
        &[],
        &[
            r#"{"id": "c0d1e2f3", "type": "horizontal", "line": "6t7v8w9x"}"#,
            r#"{"id": "g4h5j6k7", "type": "length", "line": "6t7v8w9x", "value": 40, "driving": true}"#,
            r#"{"id": "m8n9p0q1", "type": "distance_x", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true}"#,
            r#"{"id": "r2s3t4v5", "type": "vertical", "line": "a0b1c2d3"}"#,
            r#"{"id": "w6x7y8z9", "type": "length", "line": "a0b1c2d3", "value": 20, "driving": true}"#,
            r#"{"id": "x0y1z2a3", "type": "horizontal", "line": "j8k9m0n1"}"#,
            r#"{"id": "y4z5a6b7", "type": "distance_y", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true}"#,
            r#"{"id": "z8a9b0c1", "type": "vertical", "line": "p2q3r4s5"}"#,
        ],
    );
    let s = solved(&doc, &mut worker()).unwrap();
    // Eight unknowns, eight independent equations.
    assert_eq!(s.dofs, 0);
    assert!(s.free.is_empty() && s.redundant.is_empty(), "{s:?}");
    for (p, want) in [
        ("0a1b2c3d", [0.0, 0.0]),
        ("4e5f6g7h", [40.0, 0.0]),
        ("8j9k0m1n", [40.0, 20.0]),
        ("2p3q4r5s", [0.0, 20.0]),
    ] {
        assert_at(&s, p, want);
    }
}

#[test]
fn a_midpoint_whose_equation_along_y_repeats_two_others_is_partially_redundant_as_in_freecad() {
    // The port review's sketch (check's warnings, decision 17): line na000001 from (−5, 0)
    // to (5, 0), its ends held by distances from the origin, and point pa000003 at (0, 0)
    // held at the line's midpoint (k0000001), at y 0 (k0000002), and on the line
    // (k0000003). FreeCAD 1.1.3, the same sketch, its constraints in the same order:
    // solve() 0, RedundantConstraints [], PartiallyRedundantConstraints [1], fully
    // constrained, the point at (0, 0). So here: the midpoint partially redundant, kept
    // apart from the redundant (of which there are none), nothing free.
    let doc = document(
        &[
            point("pa000001", -5.0, 0.0),
            point("pa000002", 5.0, 0.0),
            point("pa000003", 0.0, 0.0),
        ],
        &[line("na000001", "pa000001", "pa000002")],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "midpoint", "point": "pa000003", "line": "na000001"}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "pa000003", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "on", "point": "pa000003", "curve": "na000001"}"#,
            r#"{"id": "k0000004", "type": "distance_x", "a": "origin", "b": "pa000001", "value": -5, "driving": true}"#,
            r#"{"id": "k0000005", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000006", "type": "distance_x", "a": "origin", "b": "pa000002", "value": 5, "driving": true}"#,
            r#"{"id": "k0000007", "type": "distance_y", "a": "origin", "b": "pa000002", "value": 0, "driving": true}"#,
        ],
    );
    let s = solved(&doc, &mut worker()).unwrap();
    assert!(s.redundant.is_empty(), "{s:?}");
    assert_eq!(
        s.partially_redundant.keys().collect::<Vec<_>>(),
        [&id("k0000001")],
        "{s:?}"
    );
    assert_eq!(s.dofs, 0);
    assert!(s.free.is_empty(), "{s:?}");
    assert_at(&s, "pa000003", [0.0, 0.0]);
}

#[test]
fn freecads_box_takes_its_corners_from_two_datums() {
    // TestSketcherSolver.py's testBoxCase: a box drawn at FreeCAD's coordinates, its top
    // and bottom horizontal, its sides vertical, its right side 81.370787 and top 187.573036
    // long, and then its bottom-right corner put at x 90, y −50. By hand: bottom-right
    // (90, −50), top-right (90, 31.370787), top-left (−97.573036, 31.370787), bottom-left
    // (−97.573036, −50).
    let doc = document(
        &[
            point("p0000001", -99.230339, 36.960674),
            point("p0000002", 69.432587, 36.960674),
            point("p0000003", 69.432587, -53.196629),
            point("p0000004", -99.230339, -53.196629),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000002", "p0000003"),
            line("n0000003", "p0000003", "p0000004"),
            line("n0000004", "p0000004", "p0000001"),
        ],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "horizontal", "line": "n0000001"}"#,
            r#"{"id": "k0000002", "type": "horizontal", "line": "n0000003"}"#,
            r#"{"id": "k0000003", "type": "vertical", "line": "n0000002"}"#,
            r#"{"id": "k0000004", "type": "vertical", "line": "n0000004"}"#,
            r#"{"id": "k0000005", "type": "length", "line": "n0000002", "value": 81.370787, "driving": true}"#,
            r#"{"id": "k0000006", "type": "length", "line": "n0000001", "value": 187.573036, "driving": true}"#,
            r#"{"id": "k0000007", "type": "distance_x", "a": "origin", "b": "p0000003", "value": 90, "driving": true}"#,
            r#"{"id": "k0000008", "type": "distance_y", "a": "origin", "b": "p0000003", "value": -50, "driving": true}"#,
        ],
    );
    let s = solved(&doc, &mut worker()).unwrap();
    assert_eq!(s.dofs, 0);
    assert_at(&s, "p0000003", [90.0, -50.0]);
    assert_at(&s, "p0000002", [90.0, 31.370787]);
    assert_at(&s, "p0000001", [-97.573036, 31.370787]);
    assert_at(&s, "p0000004", [-97.573036, -50.0]);
}

#[test]
fn freecads_slot_plate_rises_to_its_apex() {
    // TestSketcherSolver.py's testSlotCase, its outer slot (CreateSlotPlateSet) with its
    // datums as set: a bottom line 200 long ending at the origin, a side rising from there at
    // 50° to a horizontal top, and a 40-radius arc tangent to both, back at the start. By
    // hand: the arc's centre (200, 40), the top at y 80, so the apex is (80 / tan 50°, 80)
    // = (67.1279195..., 80), and the arc ends at (200, 80). FreeCAD's angle is between the
    // bottom line at its end and the side at its start, 50° from +x; Silico's is from the
    // bottom's direction (−x, from its start at the right) to the side's: −130°.
    let doc = document(
        &[
            point("p0000000", 60.029362, -30.279360),
            point("p0000001", -120.376335, -30.279360),
            point("p0000002", -70.193062, 38.113884),
            point("p0000003", 60.241116, 37.478645),
            point("p0000004", 60.039921, 3.811391),
        ],
        &[
            line("n0000000", "p0000000", "p0000001"),
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000002", "p0000003"),
        ],
        &[],
        &[arc("a0000003", "p0000004", "p0000000", "p0000003")],
        &[
            r#"{"id": "k0000001", "type": "horizontal", "line": "n0000000"}"#,
            r#"{"id": "k0000002", "type": "horizontal", "line": "n0000002"}"#,
            r#"{"id": "k0000003", "type": "tangent_at", "a": "n0000002", "b": "a0000003", "at": "p0000003", "direction": "same"}"#,
            r#"{"id": "k0000004", "type": "tangent_at", "a": "n0000000", "b": "a0000003", "at": "p0000000", "direction": "same"}"#,
            r#"{"id": "k0000005", "type": "angle", "a": "n0000000", "b": "n0000001", "value": -130, "driving": true}"#,
            r#"{"id": "k0000006", "type": "length", "line": "n0000000", "value": 200, "driving": true}"#,
            r#"{"id": "k0000007", "type": "radius", "curve": "a0000003", "value": 40, "driving": true}"#,
            r#"{"id": "k0000008", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000009", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
        ],
    );
    let s = solved(&doc, &mut worker()).unwrap();
    // Five points (10 unknowns) and the arc's radius and angles (3), less 4 arc rules and
    // 9 equations.
    assert_eq!(s.dofs, 0);
    // (FreeCAD's datum is 0.872665 rad, a little over 50°: its apex is 67.1279195.)
    let apex = 80.0 / libm::tan(50f64.to_radians());
    assert_at(&s, "p0000001", [0.0, 0.0]);
    assert_at(&s, "p0000000", [200.0, 0.0]);
    assert_at(&s, "p0000002", [apex, 80.0]);
    assert_at(&s, "p0000003", [200.0, 80.0]);
    assert_at(&s, "p0000004", [200.0, 40.0]);
}

#[test]
fn freecads_circle_to_line_distance_and_its_reference() {
    // TestSketcherSolver.py's testCircleToLineDistance_Driving_Passant and _Reference_Secant,
    // in Silico's forms: a circle of radius 20 about the origin, held there, and a
    // horizontal line drawn at y 40. Silico has no circle-to-line distance; its essence is
    // the centre's distance from the line, the radius plus the gap: a gap of 10 is a
    // distance of 30, so the line lands at y 30, 10 from the circle. And a line at y 7
    // across the circle, held there, reads back a reference distance of 7 (not half the
    // radius, so it can't pass by FreeCAD's circle-to-line distance by coincidence).
    let doc = document(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", -20.0, 40.0),
            point("p0000003", 20.0, 40.0),
        ],
        &[line("n0000001", "p0000002", "p0000003")],
        &[circle("c0000001", "p0000001", 20.0)],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "radius", "curve": "c0000001", "value": 20, "driving": true}"#,
            r#"{"id": "k0000004", "type": "horizontal", "line": "n0000001"}"#,
            r#"{"id": "k0000005", "type": "point_line_distance", "point": "p0000001", "line": "n0000001", "value": 30, "side": "right", "driving": true}"#,
        ],
    );
    let s = solved(&doc, &mut worker()).unwrap();
    for p in ["p0000002", "p0000003"] {
        let y = s.point(&id(p)).unwrap()[1];
        assert!((y - 30.0).abs() <= CLOSE, "{p} at y {y}");
    }
    let doc = document(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", -20.0, 7.0),
            point("p0000003", 20.0, 7.0),
        ],
        &[line("n0000001", "p0000002", "p0000003")],
        &[circle("c0000001", "p0000001", 20.0)],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": -20, "driving": true}"#,
            r#"{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 7, "driving": true}"#,
            r#"{"id": "k0000005", "type": "distance_x", "a": "origin", "b": "p0000003", "value": 20, "driving": true}"#,
            r#"{"id": "k0000006", "type": "distance_y", "a": "origin", "b": "p0000003", "value": 7, "driving": true}"#,
            r#"{"id": "k0000007", "type": "point_line_distance", "point": "p0000001", "line": "n0000001", "value": 0, "side": "left", "driving": false}"#,
        ],
    );
    let s = solved(&doc, &mut worker()).unwrap();
    // The centre is right of a line running +x at y 7.
    assert_eq!(
        s.references[&id("k0000007")],
        silico_core::sketch::Measured::FromLine {
            distance: 7.0,
            side: silico_core::document::LineSide::Right
        }
    );
}

/// A line from p1 to p2, free, with these constraints.
fn free_line(constraints: &[&str]) -> Checked {
    document(
        &[point("p0000001", 1.0, 2.0), point("p0000002", 11.0, 3.0)],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        constraints,
    )
}

#[test]
fn a_free_line_has_four_degrees_of_freedom_and_its_points_are_free() {
    let s = solved(&free_line(&[]), &mut worker()).unwrap();
    assert_eq!(s.dofs, 4);
    assert_eq!(
        s.free.into_iter().collect::<Vec<_>>(),
        [id("p0000001"), id("p0000002")]
    );
}

#[test]
fn a_dimension_too_many_on_a_fixed_line_is_over_constrained_naming_it() {
    // The line held at both ends, then given a length it doesn't have: conflicting, and the
    // added length is among those named.
    let doc = free_line(&[
        r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 1, "driving": true}"#,
        r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 2, "driving": true}"#,
        r#"{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 11, "driving": true}"#,
        r#"{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 3, "driving": true}"#,
        r#"{"id": "k0000005", "type": "length", "line": "n0000001", "value": 12, "driving": true}"#,
    ]);
    let e = solved(&doc, &mut worker()).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert!(e.elements().contains(&"k0000005".to_owned()), "{e}");
}

/// Two lines from p1: n1 to p2 along +x, n2 to p3 up, drawn true; n1 horizontal and n2
/// vertical, and these constraints. PlaneGCS, like FreeCAD, diagnoses where the sketch is
/// drawn: drawn off true, the angle's equation is independent there of the other two, and
/// nothing is redundant or conflicting until a solve has put it true.
fn corner(constraints: &[&str]) -> Checked {
    let mut all = vec![
        r#"{"id": "k0000001", "type": "horizontal", "line": "n0000001"}"#,
        r#"{"id": "k0000002", "type": "vertical", "line": "n0000002"}"#,
    ];
    all.extend(constraints);
    document(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 0.0, 8.0),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000001", "p0000003"),
        ],
        &[],
        &[],
        &all,
    )
}

#[test]
fn a_right_angle_a_horizontal_and_a_vertical_already_make_is_redundant_and_kept() {
    let s = solved(
        &corner(&[
            r#"{"id": "k0000003", "type": "angle", "a": "n0000001", "b": "n0000002", "value": 90, "driving": true}"#,
        ]),
        &mut worker(),
    )
    .unwrap();
    assert_eq!(s.redundant.len(), 1, "{s:?}");
    // Three equations, two independent, on six unknowns.
    assert_eq!(s.dofs, 4);
}

#[test]
fn an_eighty_degree_angle_against_a_horizontal_and_a_vertical_conflicts() {
    let e = solved(
        &corner(&[
            r#"{"id": "k0000003", "type": "angle", "a": "n0000001", "b": "n0000002", "value": 80, "driving": true}"#,
        ]),
        &mut worker(),
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert!(e.elements().contains(&"k0000003".to_owned()), "{e}");
}

/// A line from p1 to p2 drawn at `x0` along x, held at p1, horizontal, 10 long, and a
/// horizontal distance from p1 to p2 of `dx`: two ways of giving its length, one of them
/// redundant to PlaneGCS, which calls them redundant if they're within 1e-5.
fn twice_ten(x0: f64, dx: &str) -> Checked {
    let dx_c = format!(
        r#"{{"id": "k0000006", "type": "distance_x", "a": "p0000001", "b": "p0000002", "value": {dx}, "driving": true}}"#
    );
    let x0s = format!(
        r#"{{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": {x0}, "driving": true}}"#
    );
    document(
        &[point("p0000001", x0, 0.0), point("p0000002", x0 + 9.0, 1.0)],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &[
            &x0s,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#,
            r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#,
            &dx_c,
        ],
    )
}

#[test]
fn two_lengths_a_nanometre_apart_conflict_and_equal_ones_agree_here_and_a_kilometre_out() {
    // Decision 5: PlaneGCS calls the horizontal distance redundant either way; Silico keeps
    // it only when it's met on the solution within 1e-8 mm (its part's extent is its 10
    // mm line, wherever the line is: position never counts).
    for x0 in [0.0, 999_980.0] {
        let s = solved(&twice_ten(x0, "10"), &mut worker()).unwrap();
        assert_eq!(s.redundant.len(), 1, "{x0}: {s:?}");
        assert_eq!(s.dofs, 0);
        let e = solved(&twice_ten(x0, "10.000001"), &mut worker()).unwrap_err();
        assert_eq!(e.code(), ErrorCode::OverConstrained, "{x0}: {e}");
        assert_eq!(e.elements().len(), 1, "{e}");
    }
}

/// A line from p1, held at the origin, horizontal, `len` long, and a horizontal distance
/// from the origin to its end of `dx`: its length given twice.
fn length_twice(len: f64, dx: &str) -> Checked {
    let length = format!(
        r#"{{"id": "k0000004", "type": "length", "line": "n0000001", "value": {len}, "driving": true}}"#
    );
    let dx = format!(
        r#"{{"id": "k0000005", "type": "distance_x", "a": "origin", "b": "p0000002", "value": {dx}, "driving": true}}"#
    );
    document(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", len - 1.0, 1.0),
        ],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#,
            &length,
            &dx,
        ],
    )
}

#[test]
fn a_nanometre_conflicts_under_a_metre_and_is_kept_over_one() {
    // Decision 5 (Fable, 2026-10-18; its scale, 2026-10-20): a sketch's tolerance is the
    // larger of 1e-8 mm and 1e-9 times its extent, the diagonal of the box around its
    // points, here the line itself. A line 999 mm long given again as 999.000001: a gap of
    // 1e-6 past 9.99e-7, over_constrained, the message giving both. At 1001 against 1001.000001
    // the tolerance is 1.001e-6: kept, its gap of 1e-6 reported.
    let e = solved(&length_twice(999.0, "999.000001"), &mut worker()).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert_eq!(e.elements().len(), 1, "{e}");
    assert!(e.message().contains("past the 9.99"), "{e}");
    assert!(
        e.suggestion().unwrap_or("").contains("driving: false"),
        "{e}"
    );
    let s = solved(&length_twice(1001.0, "1001.000001"), &mut worker()).unwrap();
    let gaps: Vec<f64> = s.redundant.values().copied().collect();
    assert_eq!(gaps.len(), 1, "{s:?}");
    assert!((0.99e-6..=1.01e-6).contains(&gaps[0]), "{gaps:?}");
}

/// A horizontal line n1 and a vertical line n2, both 10 long from p1 at the origin, the
/// angle between them given as `angle` degrees; and, if `far`, a point p5 at (1100, 100),
/// dimensioned from p1 (k8, k9), so it's in the lines' part of the sketch.
fn right_angle_given(angle: &str, far: bool) -> Checked {
    let angle = format!(
        r#"{{"id": "k0000007", "type": "angle", "a": "n0000001", "b": "n0000002", "value": {angle}, "driving": true}}"#
    );
    let mut points = vec![
        point("p0000001", 0.0, 0.0),
        point("p0000002", 9.0, 0.5),
        point("p0000003", 0.5, 11.0),
    ];
    let lines = [
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000001", "p0000003"),
    ];
    let mut constraints = vec![
        r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
        r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
        r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#,
        r#"{"id": "k0000004", "type": "vertical", "line": "n0000002"}"#,
        r#"{"id": "k0000005", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#,
        r#"{"id": "k0000006", "type": "length", "line": "n0000002", "value": 10, "driving": true}"#,
        &angle,
    ];
    if far {
        points.push(point("p0000005", 1100.0, 100.0));
        constraints.extend([
            r#"{"id": "k0000008", "type": "distance_x", "a": "p0000001", "b": "p0000005", "value": 1100, "driving": true}"#,
            r#"{"id": "k0000009", "type": "distance_y", "a": "p0000001", "b": "p0000005", "value": 100, "driving": true}"#,
        ]);
    }
    document(&points, &lines, &[], &[], &constraints)
}

#[test]
fn an_angle_off_is_measured_as_the_distance_it_moves_a_lines_end() {
    // Decision 5's angular conversion: a right angle given as 90.000002865°, 5.0e-8 rad off
    // what the horizontal and vertical make. Carried by a 10 mm line, a gap of 5.0e-7 mm
    // (hand: 2.865e-6° × π/180 × 10): past the 1.41e-8 of the lines alone (their points
    // span 10 × 10), over_constrained. With a point p5 at (1100, 100) dimensioned from p1,
    // in the lines' part, the part's points span 1,100 × 100: an extent of √(1100² + 100²)
    // = 1,104.5 and a tolerance of 1.1045e-6: kept, the gap reported. Whichever of the three
    // PlaneGCS drops, the gap is the same distance.
    let e = solved(&right_angle_given("90.000002865", false), &mut worker()).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    let s = solved(&right_angle_given("90.000002865", true), &mut worker()).unwrap();
    let gaps: Vec<f64> = s.redundant.values().copied().collect();
    assert_eq!(gaps.len(), 1, "{s:?}");
    assert!((4.9e-7..=5.1e-7).contains(&gaps[0]), "{gaps:?}");
}

#[test]
fn a_thirty_sixty_ninety_triangle_from_two_hundred_starts_keeps_its_implied_angle() {
    // The oracle's case for decision 5: a triangle with its corner angles 30°, 60°, and 90°
    // (the third implied by the other two), its base horizontal, 10 long, its first corner
    // at the origin. Drawn from 200 starts about the solution, every solve keeps the
    // redundant angle: it's 180° less the two kept angles, each met to 1e-10 rad, so it's
    // at most 2e-10 rad off (PlaneGCS leaves it up to 1.32e-10 here), which carried by its
    // shorter line (BC, 5 mm) is a gap of at most 1e-9 mm, inside the sketch's 1e-8.
    let mut rng = Rng(0x30_60_90);
    let mut w = worker();
    let s3 = 3f64.sqrt();
    // Corners: A (0, 0), B (10, 0), C, with the right angle at C: AC = 10 cos 30°.
    let c = [7.5, 2.5 * s3];
    for start in 0..200 {
        let j = |rng: &mut Rng| rng.range(-1.5, 1.5);
        let (bx, by, cx, cy) = (
            10.0 + j(&mut rng),
            j(&mut rng),
            c[0] + j(&mut rng),
            c[1] + j(&mut rng),
        );
        let doc = document(
            &[
                point("p0000001", 0.0, 0.0),
                point("p0000002", bx, by),
                point("p0000003", cx, cy),
            ],
            &[
                line("n0000001", "p0000001", "p0000002"),
                line("n0000002", "p0000002", "p0000003"),
                line("n0000003", "p0000003", "p0000001"),
            ],
            &[],
            &[],
            &[
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
                r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#,
                r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#,
                // At A, from AB (+x) to AC: 30° counter-clockwise; Silico's angles are
                // between the lines' own directions: n1 runs A→B, n3 runs C→A.
                r#"{"id": "k0000005", "type": "angle", "a": "n0000001", "b": "n0000003", "value": -150, "driving": true}"#,
                // At B, from BA to BC, 60° clockwise: n1 A→B against n2 B→C.
                r#"{"id": "k0000006", "type": "angle", "a": "n0000001", "b": "n0000002", "value": 120, "driving": true}"#,
                // At C, a right angle: n2 B→C against n3 C→A.
                r#"{"id": "k0000007", "type": "angle", "a": "n0000002", "b": "n0000003", "value": 90, "driving": true}"#,
            ],
        );
        let s = solved(&doc, &mut w).unwrap_or_else(|e| panic!("start {start}: {e}"));
        assert_eq!(s.redundant.len(), 1, "start {start}: {s:?}");
        assert!(
            s.redundant.values().all(|&gap| gap <= 1e-9),
            "start {start}: {s:?}"
        );
        assert_at(&s, "p0000003", c);
    }
}

#[test]
fn a_point_held_at_a_lines_middle_and_on_it_lands_at_the_middle() {
    // The oracle's on_repeats_a_midpoint, whose FreeCAD solve the oracle can't compare
    // (FreeCAD writes no geometry back for a redundant sketch): a line held at (0, 0) and
    // (10, 0), and a point at its middle that an on constraint also holds on it. The on
    // repeats one of the midpoint's two equations, so one is redundant and met exactly; the
    // point, from 50 starts about it, lands at the middle, (5, 0), by hand.
    let mut rng = Rng(0x500);
    let mut w = worker();
    for start in 0..50 {
        let (x, y) = (5.0 + rng.range(-3.0, 3.0), rng.range(-3.0, 3.0));
        let doc = document(
            &[
                point("p0000001", 0.0, 0.0),
                point("p0000002", 10.0, 0.0),
                point("p0000003", x, y),
            ],
            &[line("n0000001", "p0000001", "p0000002")],
            &[],
            &[],
            &[
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
                r#"{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 10, "driving": true}"#,
                r#"{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
                r#"{"id": "k0000005", "type": "midpoint", "point": "p0000003", "line": "n0000001"}"#,
                r#"{"id": "k0000006", "type": "on", "point": "p0000003", "curve": "n0000001"}"#,
            ],
        );
        let s = solved(&doc, &mut w).unwrap_or_else(|e| panic!("start {start}: {e}"));
        assert_eq!(s.redundant.len(), 1, "start {start}: {s:?}");
        assert!(
            s.redundant.values().all(|&gap| gap <= 1e-9),
            "start {start}: {s:?}"
        );
        assert_at(&s, "p0000003", [5.0, 0.0]);
    }
}

#[test]
fn a_line_ending_tangent_on_a_circle_lands_at_the_analytic_point() {
    // The roadmap's oracle case: a line from (−20, 10.5), held there, ending on a circle of
    // radius 10 about the origin, tangent, the line's direction leaving its end the same as
    // the circle's (counter-clockwise) there: the upper tangent point, where t·p = r² and
    // |t| = r: (0.248380314661, 9.996914885068) to 12 places.
    let doc = document(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", -20.0, 10.5),
            point("p0000003", 0.5, 10.2),
        ],
        &[line("n0000001", "p0000002", "p0000003")],
        &[circle("c0000001", "p0000001", 10.0)],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "radius", "curve": "c0000001", "value": 10, "driving": true}"#,
            r#"{"id": "k0000004", "type": "distance_x", "a": "origin", "b": "p0000002", "value": -20, "driving": true}"#,
            r#"{"id": "k0000005", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 10.5, "driving": true}"#,
            r#"{"id": "k0000006", "type": "tangent_on", "a": "n0000001", "at": "p0000003", "b": "c0000001", "direction": "same"}"#,
        ],
    );
    let s = solved(&doc, &mut worker()).unwrap();
    assert_eq!(s.dofs, 0);
    let t = s.point(&id("p0000003")).unwrap();
    assert!((t[0] - 0.248_380_314_661).abs() < 1e-11, "{t:?}");
    assert!((t[1] - 9.996_914_885_068).abs() < 1e-11, "{t:?}");
}

#[test]
fn a_point_two_lines_share_held_in_two_places_conflicts() {
    // TestCoincidentCommandGui.py's conflicting coincidence, in Silico's terms: two lines
    // whose ends FreeCAD holds at x 10 and x 20 and then makes coincident are, with shared
    // points, two lines through one point held at both: conflicting, both named.
    let doc = document(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 30.0, 0.0),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000002", "p0000003"),
        ],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 10, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 20, "driving": true}"#,
        ],
    );
    let e = solved(&doc, &mut worker()).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert_eq!(e.elements(), ["k0000001", "k0000002"]);
    // Without the second hold, the joined lines solve: 3 points' 6 values less the one held,
    // 5 degrees of freedom (by hand). (FreeCAD's fixture solves before its coincidence too,
    // as two lines apart: 8 values less two, 6.)
    let doc = document(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 30.0, 0.0),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000002", "p0000003"),
        ],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 10, "driving": true}"#,
        ],
    );
    assert_eq!(solved(&doc, &mut worker()).unwrap().dofs, 5);
}

/// TestCoincidentCommandGui.py's tangent substitution sketch: a line from (−10, 0), 20 long
/// and horizontal, and an arc of radius 5 about (0, 5), its centre's x and its ends' x held,
/// the line's end at `line_end`, and `tangency` between them. FreeCAD's arc runs from 0.5
/// rad to 2.5 rad; here from (4, 8) to (−4, 8) (3² + 4² = 5²), so every value is a whole
/// nanometre, as a document's are.
fn line_and_arc(line_end: &str, arc_start: &str, tangency: &str) -> Checked {
    let mut points = vec![
        point("p0000001", -10.0, 0.0),
        point("p0000003", 0.0, 5.0),
        point("p0000005", -4.0, 8.0),
    ];
    points.push(point(line_end, 10.0, 0.0));
    if arc_start != line_end {
        points.push(point(arc_start, 4.0, 8.0));
    }
    document(
        &points,
        &[line("n0000001", "p0000001", line_end)],
        &[],
        &[arc("a0000002", "p0000003", arc_start, "p0000005")],
        &[
            r#"{"id": "k0000001", "type": "horizontal", "line": "n0000001"}"#,
            r#"{"id": "k0000002", "type": "distance_x", "a": "origin", "b": "p0000001", "value": -10, "driving": true}"#,
            r#"{"id": "k0000003", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 20, "driving": true}"#,
            r#"{"id": "k0000005", "type": "distance_x", "a": "origin", "b": "p0000003", "value": 0, "driving": true}"#,
            r#"{"id": "k0000006", "type": "radius", "curve": "a0000002", "value": 5, "driving": true}"#,
            &format!(
                r#"{{"id": "k0000007", "type": "distance_x", "a": "origin", "b": "{arc_start}", "value": 4, "driving": true}}"#
            ),
            r#"{"id": "k0000008", "type": "distance_x", "a": "origin", "b": "p0000005", "value": -4, "driving": true}"#,
            tangency,
        ],
    )
}

#[test]
fn a_tangency_of_the_curves_holds_where_one_at_shared_ends_cant() {
    // TestCoincidentCommandGui.py's failed tangent substitution, its solver half. The line
    // tangent to the arc's circle solves where drawn (FreeCAD's solve() 0): the line on
    // y = 0, the centre 5 above it, every value held, 0 degrees of freedom by hand (11
    // values: 4 the line's, 7 the arc's centre, ends, and radius; 11 equations: 4 on the
    // line, the arc's 2 rules, its radius, 3 x's held, and the tangency). The tangency at a
    // shared end FreeCAD substitutes can't hold, because of the joint it implies, as in
    // FreeCAD: the line's end, held at x 10 by its length, is then the arc's start, held at
    // x 4 (FreeCAD's solve() nonzero).
    let curves = line_and_arc(
        "p0000002",
        "p0000004",
        r#"{"id": "k0000009", "type": "tangent", "a": "n0000001", "b": "a0000002", "side": "left"}"#,
    );
    let s = solved(&curves, &mut worker()).unwrap();
    assert_eq!(s.dofs, 0);
    let c = s.point(&id("p0000003")).unwrap();
    assert!(c[0].abs() < 1e-9 && (c[1] - 5.0).abs() < 1e-9, "{c:?}");
    let at_end = line_and_arc(
        "p0000002",
        "p0000002",
        r#"{"id": "k0000009", "type": "tangent_at", "a": "n0000001", "b": "a0000002", "at": "p0000002", "direction": "same"}"#,
    );
    let e = solved(&at_end, &mut worker()).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert!(e.elements().iter().any(|c| c == "k0000007"), "{e}");
}

/// The worker, recording the algorithm each solve it answers was solved by.
struct Counting {
    inner: SketchWorker,
    replies: Vec<Solved>,
}

impl Counting {
    fn new(inner: SketchWorker) -> Self {
        Counting {
            inner,
            replies: Vec::new(),
        }
    }

    fn algorithms(&self) -> Vec<Option<Algorithm>> {
        self.replies.iter().map(|r| r.algorithm).collect()
    }
}

impl SketchSolver for Counting {
    fn solve(&mut self, system: SketchSystem) -> Result<Solved, silico_core::Error> {
        let r = self.inner.solve(system)?;
        self.replies.push(r.clone());
        Ok(r)
    }
}

#[test]
fn a_line_collapsed_by_its_constraints_is_passed_over_by_dogleg_and_refused_under_the_floor() {
    // TestCoincidentCommandGui.py's collapsed line: FreeCAD's coincidences make its two ends
    // one, exactly, so no algorithm can build it. In Silico a line's ends are two points
    // (the gate refuses one point twice), and the nearest is both held at (5, 5) by
    // dimensions. DogLeg solves that exactly, to a line of no length, which FreeCAD's
    // GeomLineSegment refuses ("Both points are equal": gp::Resolution, so only exactly);
    // its solution is passed over (fc128d357d), and Levenberg–Marquardt's, its ends 1e-10
    // apart, is a line FreeCAD builds too. Silico doesn't keep it: a line 1e-7 mm long or
    // less is no size to the kernel, the gate's floor (the M3 review, round 1), so the solve
    // is an invalid result naming the line, with the gate's advice.
    let doc = free_line(&[
        r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 5, "driving": true}"#,
        r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 5, "driving": true}"#,
        r#"{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 5, "driving": true}"#,
        r#"{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 5, "driving": true}"#,
    ]);
    let silico_core::document::Operation::Sketch(sketch) = &doc.document().features[0].operation
    else {
        panic!()
    };
    let mut alone = silico_core::sketch::Translation::new(sketch)
        .unwrap()
        .system;
    alone.algorithms = vec![Algorithm::DogLeg];
    let by_dogleg = silico_sketch::solve(&alone).unwrap();
    assert_eq!(
        by_dogleg.status,
        silico_protocol::SolveStatus::SolvedInvalid
    );
    let mut w = Counting::new(worker());
    let e = solved(&doc, &mut w).unwrap_err();
    assert_eq!(w.algorithms(), [Some(Algorithm::LevenbergMarquardt)]);
    assert_eq!(e.code(), silico_core::ErrorCode::InvalidResult, "{e}");
    assert_eq!(e.elements(), ["n0000001", "p0000001", "p0000002"], "{e}");
    assert!(
        e.message()
            .starts_with("the sketch solves to geometry its file can't hold: line n0000001 is "),
        "{e}"
    );
    assert_eq!(
        e.suggestion(),
        Some("change the constraints that draw it this small: draw the line's ends further apart")
    );
}

#[test]
fn a_point_held_on_the_other_side_of_its_line_is_an_invalid_result() {
    // A point drawn left of a fixed line, held 3 to its right: 1.1.3's distance has no side,
    // so the solve keeps it left, 3 away, and that's refused, naming the constraint.
    let doc = document(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 4.0, 5.0),
        ],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 10, "driving": true}"#,
            r#"{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
            r#"{"id": "k0000005", "type": "point_line_distance", "point": "p0000003", "line": "n0000001", "value": 3, "side": "right", "driving": true}"#,
        ],
    );
    let e = solved(&doc, &mut worker()).unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidResult, "{e}");
    assert_eq!(e.elements(), ["k0000005"]);
}

#[test]
fn every_algorithm_is_offered_in_freecads_order() {
    assert_eq!(
        silico_core::sketch::ALGORITHMS,
        [
            Algorithm::DogLeg,
            Algorithm::LevenbergMarquardt,
            Algorithm::Bfgs,
            Algorithm::AnchoredSqp
        ]
    );
}

#[test]
fn a_symmetry_its_constraints_collapse_is_an_invalid_result_naming_it() {
    // The robustness review's case: a held on the x axis and mirrored about it, so b's only
    // place is a's, (5, 0): two points at one place, which no later solve could start from.
    let doc = document(
        &[point("p0000001", 5.0, 3.0), point("p0000002", 5.0, -4.0)],
        &[],
        &[],
        &[],
        &[
            r#"{"id": "c0000001", "type": "on", "point": "p0000001", "curve": "x_axis"}"#,
            r#"{"id": "c0000002", "type": "symmetric", "a": "p0000001", "b": "p0000002", "about": "x_axis"}"#,
            r#"{"id": "c0000003", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 5, "driving": true}"#,
        ],
    );
    let e = solved(&doc, &mut worker()).unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidResult, "{e}");
    assert_eq!(e.elements(), ["c0000002"]);
}

/// A line held from the origin to (10, 0), a circle of radius 2 whose centre is drawn at
/// (5, `y`), tangent to the line on `side`.
fn tangent_to_a_line(y: f64, side: &str) -> Checked {
    let c = format!(
        r#"{{"id": "k0000006", "type": "tangent", "a": "n0000001", "b": "c0000001", "side": "{side}"}}"#
    );
    document(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 5.0, y),
        ],
        &[line("n0000001", "p0000001", "p0000002")],
        &[circle("c0000001", "p0000003", 2.0)],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 10, "driving": true}"#,
            r#"{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
            r#"{"id": "k0000005", "type": "radius", "curve": "c0000001", "value": 2, "driving": true}"#,
            &c,
        ],
    )
}

#[test]
fn a_circle_tangent_to_a_line_lands_on_the_side_given() {
    // Left of the line (running +x) is +y: drawn above, the centre lands 2 above; drawn
    // below and held right, 2 below. Drawn below but held left, 1.1.3's unsigned tangency
    // keeps it below, which is refused, naming the tangency.
    for (y, side, want) in [(3.0, "left", 2.0), (-3.0, "right", -2.0)] {
        let s = solved(&tangent_to_a_line(y, side), &mut worker()).unwrap();
        assert_at(&s, "p0000003", [5.0, want]);
    }
    let e = solved(&tangent_to_a_line(-3.0, "left"), &mut worker()).unwrap_err();
    assert_eq!(
        (e.code(), e.elements()),
        (ErrorCode::InvalidResult, &["k0000006".to_owned()][..])
    );
}

#[test]
fn two_circles_tangent_outside_and_inside_land_their_radii_apart() {
    // A circle of radius 3 held at the origin, and one of radius 2 whose centre is drawn at
    // (10, 0.5) and held on the x axis: outside, the centres end 5 apart; inside, 1.
    for (side, want) in [("outside", 5.0), ("inside", 1.0)] {
        let c = format!(
            r#"{{"id": "k0000006", "type": "tangent", "a": "c0000001", "b": "c0000002", "side": "{side}"}}"#
        );
        let doc = document(
            &[point("p0000001", 0.0, 0.0), point("p0000002", 10.0, 0.5)],
            &[],
            &[
                circle("c0000001", "p0000001", 3.0),
                circle("c0000002", "p0000002", 2.0),
            ],
            &[],
            &[
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
                r#"{"id": "k0000003", "type": "radius", "curve": "c0000001", "value": 3, "driving": true}"#,
                r#"{"id": "k0000004", "type": "radius", "curve": "c0000002", "value": 2, "driving": true}"#,
                r#"{"id": "k0000005", "type": "on", "point": "p0000002", "curve": "x_axis"}"#,
                &c,
            ],
        );
        let s = solved(&doc, &mut worker()).unwrap();
        let p = s.point(&id("p0000002")).unwrap();
        assert!(
            (p[0].abs() - want).abs() <= CLOSE && p[1].abs() <= CLOSE,
            "{side}: {p:?}"
        );
    }
}

/// c1 and c2 both about p1, held at (1, 1); c1's radius held 10, c2's drawn 12 and held
/// `r2` if given; c1 and c2 tangent on `side` (k0000004).
fn on_one_centre(side: &str, r2: Option<f64>) -> Checked {
    let tangent = format!(
        r#"{{"id": "k0000004", "type": "tangent", "a": "c0000001", "b": "c0000002", "side": "{side}"}}"#
    );
    let radius = r2.map(|r| {
        format!(r#"{{"id": "k0000005", "type": "radius", "curve": "c0000002", "value": {r}, "driving": true}}"#)
    });
    let mut constraints = vec![
        r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 1, "driving": true}"#,
        r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 1, "driving": true}"#,
        r#"{"id": "k0000003", "type": "radius", "curve": "c0000001", "value": 10, "driving": true}"#,
        &tangent,
    ];
    constraints.extend(radius.as_deref());
    document(
        &[point("p0000001", 1.0, 1.0)],
        &[],
        &[
            circle("c0000001", "p0000001", 10.0),
            circle("c0000002", "p0000001", 12.0),
        ],
        &[],
        &constraints,
    )
}

#[test]
fn an_outside_tangency_between_circles_on_one_centre_is_an_invalid_result() {
    // The port reviewer's case: circles on one centre touch outside only at radius 0, but
    // 1.1.3's PlaneGCS measures a tangency between concentric circles as r1 − r2 whatever
    // its side, so the solve gave r2 = 10, two identical circles, the tangency 20 mm off
    // (d = 0, r1 + r2 = 20), and no redundancy. Refused, naming the tangency and both
    // circles, with c2's radius free and with it held 10 (which PlaneGCS called redundant).
    for r2 in [None, Some(10.0)] {
        let e = solved(&on_one_centre("outside", r2), &mut worker()).unwrap_err();
        assert_eq!(
            (e.code(), e.elements()),
            (
                ErrorCode::InvalidResult,
                &[
                    "k0000004".to_owned(),
                    "c0000001".to_owned(),
                    "c0000002".to_owned()
                ][..]
            ),
            "{r2:?}: {e}"
        );
    }
}

#[test]
fn an_outside_tangency_drawn_apart_and_driven_onto_one_centre_is_an_invalid_result() {
    // The route FreeCAD 1.1.3 takes too, and accepts (the port reviewer ran it on the
    // oracle): c1 r 10 held about the origin, c2 drawn 30 to its right, r 12, tangent
    // outside, and c2's centre dimensioned onto the origin. The centres cross under 1e-7
    // during the solve, where PlaneGCS measures r1 − r2 whatever the side, and it lands on
    // two equal circles on one centre. Silico refuses that on purpose, naming the tangency
    // and both circles.
    let doc = document(
        &[point("p0000001", 0.0, 0.0), point("p0000002", 30.0, 0.0)],
        &[],
        &[
            circle("c0000001", "p0000001", 10.0),
            circle("c0000002", "p0000002", 12.0),
        ],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "radius", "curve": "c0000001", "value": 10, "driving": true}"#,
            r#"{"id": "k0000004", "type": "tangent", "a": "c0000001", "b": "c0000002", "side": "outside"}"#,
            r#"{"id": "k0000005", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
            r#"{"id": "k0000006", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
        ],
    );
    let e = solved(&doc, &mut worker()).unwrap_err();
    assert_eq!(
        (e.code(), e.elements()),
        (
            ErrorCode::InvalidResult,
            &[
                "k0000004".to_owned(),
                "c0000001".to_owned(),
                "c0000002".to_owned()
            ][..]
        ),
        "{e}"
    );
}

#[test]
fn an_inside_tangency_between_circles_on_one_centre_makes_them_equal() {
    // Inside, PlaneGCS's r1 − r2 is the tangency itself: c2's free radius solves to c1's,
    // 10, about (1, 1).
    let s = solved(&on_one_centre("inside", None), &mut worker()).unwrap();
    assert_at(&s, "p0000001", [1.0, 1.0]);
    let r = s.radius(&id("c0000002")).unwrap();
    assert!((r - 10.0).abs() <= CLOSE, "{r}");
}

#[test]
fn an_outside_tangency_between_circles_apart_takes_the_gap_as_the_free_radius() {
    // c1 r 10 held about the origin, c2 about (25, 0), its radius free, drawn 12: tangent
    // outside, r2 = 25 − 10 = 15.
    let doc = document(
        &[point("p0000001", 0.0, 0.0), point("p0000002", 25.0, 0.0)],
        &[],
        &[
            circle("c0000001", "p0000001", 10.0),
            circle("c0000002", "p0000002", 12.0),
        ],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 25, "driving": true}"#,
            r#"{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
            r#"{"id": "k0000005", "type": "radius", "curve": "c0000001", "value": 10, "driving": true}"#,
            r#"{"id": "k0000006", "type": "tangent", "a": "c0000001", "b": "c0000002", "side": "outside"}"#,
        ],
    );
    let s = solved(&doc, &mut worker()).unwrap();
    assert_at(&s, "p0000002", [25.0, 0.0]);
    let r = s.radius(&id("c0000002")).unwrap();
    assert!((r - 15.0).abs() <= CLOSE, "{r}");
}

#[test]
fn a_sketch_worker_waits_its_own_time_and_the_time_it_is_given() {
    // The council's tests lens: SKETCH_TIMEOUT was guarded only by a test bounded by the
    // wall clock. The default is it, and a time given is the one used: a chain of 256 unit
    // links held 1,000 apart, which every algorithm takes seconds to fail, given 100 ms,
    // times out.
    use std::time::Duration;
    assert_eq!(worker().timeout(), silico_core::sketch::SKETCH_TIMEOUT);
    let short = Duration::from_millis(100);
    let mut w = worker().with_timeout(short);
    assert_eq!(w.timeout(), short);
    let n = 256;
    let points: Vec<String> = (0..n)
        .map(|i| point(&format!("p{i:07}"), i as f64 * 0.7, (i % 2) as f64 * 0.5))
        .collect();
    let mut constraints = vec![
        r#"{"id": "a0000001", "type": "distance_x", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#.to_owned(),
        r#"{"id": "a0000002", "type": "distance_y", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#.to_owned(),
        format!(r#"{{"id": "a0000003", "type": "distance_x", "a": "origin", "b": "p{:07}", "value": 1000, "driving": true}}"#, n - 1),
    ];
    for i in 1..n {
        constraints.push(format!(
            r#"{{"id": "d{i:07}", "type": "distance", "a": "p{:07}", "b": "p{i:07}", "value": 1, "driving": true}}"#,
            i - 1
        ));
    }
    let refs: Vec<&str> = constraints.iter().map(String::as_str).collect();
    let doc = document(&points, &[], &[], &[], &refs);
    let e = solved(&doc, &mut w).unwrap_err();
    assert_eq!(e.code(), ErrorCode::WorkerTimeout, "{e}");
    assert_eq!(e.feature(), Some(FEATURE));
    // Stopped for that, the worker is started again for the next solve (round 3's
    // robustness review: it stayed stopped, and every later sketch failed).
    let stopped = w.pid();
    w.set_timeout(silico_core::sketch::SKETCH_TIMEOUT);
    assert_eq!(w.timeout(), silico_core::sketch::SKETCH_TIMEOUT);
    let s = solved(&free_line(&[]), &mut w).unwrap();
    assert_eq!(s.dofs, 4);
    let restarted = w.pid();
    assert_ne!(restarted, stopped, "a new worker");
    // A worker that answered is kept: no new process for each solve (round 4's tests lens).
    solved(&free_line(&[]), &mut w).unwrap();
    assert_eq!(w.pid(), restarted);
}

#[test]
fn a_distrusted_sketch_worker_is_stopped_and_the_next_solve_starts_another() {
    // Round 3's tests lens: SketchWorker::distrust had no test.
    use silico_core::sketch::SketchSolver;
    let mut w = worker();
    let first = w.pid();
    assert!(systems::process_exists(first));
    w.distrust();
    let deadline = std::time::Instant::now() + WAIT;
    while systems::process_exists(first) {
        assert!(
            std::time::Instant::now() < deadline,
            "the distrusted worker still runs"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let s = solved(&free_line(&[]), &mut w).unwrap();
    assert_eq!(s.dofs, 4);
    assert_ne!(w.pid(), first);
}

#[test]
fn a_sketch_worker_killed_while_idle_is_started_again_for_the_next_solve() {
    // Round 4's robustness review: one killed from outside between solves failed the next
    // solve once before it was replaced.
    let mut w = worker();
    solved(&free_line(&[]), &mut w).unwrap();
    let first = w.pid();
    std::process::Command::new("kill")
        .args(["-9", &first.to_string()])
        .status()
        .unwrap();
    // Dead: gone, or a zombie waiting for its parent to reap it.
    let dead = || {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &first.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        stat.is_empty() || stat.starts_with('Z')
    };
    let deadline = std::time::Instant::now() + WAIT;
    while !dead() {
        assert!(
            std::time::Instant::now() < deadline,
            "the worker still runs"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let s = solved(&free_line(&[]), &mut w).unwrap();
    assert_eq!(s.dofs, 4);
    assert_ne!(w.pid(), first);
}
