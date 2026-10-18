//! M3.3: a document's sketch solved through the sketch worker (silico_core::sketch), from
//! the file to the solution as data. FreeCAD's own solver tests, ported to Silico's forms
//! (shared points for its coincidences), and the cases FreeCAD lacks; expected values are
//! worked out by hand, each where it's asserted.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/systems.rs"]
mod systems;

use std::path::Path;

use silico_core::ErrorCode;
use silico_core::document::{Checked, ElementId, FeatureId, read};
use silico_core::sketch::{SketchSolution, SketchWorker, solve};
use silico_protocol::Algorithm;
use systems::Rng;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
const FEATURE: &str = "k7f2qx3m";

/// The solver meets each constraint to 1e-10; points land within this of where they must.
const CLOSE: f64 = 1e-8;

fn worker() -> SketchWorker {
    SketchWorker::start_worker(Path::new(WORKER)).unwrap()
}

fn point(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
}

fn line(id: &str, from: &str, to: &str) -> String {
    format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#)
}

fn circle(id: &str, center: &str, radius: f64) -> String {
    format!(r#"{{"id": "{id}", "center": "{center}", "radius": {radius}, "construction": false}}"#)
}

fn arc(id: &str, center: &str, start: &str, end: &str) -> String {
    format!(
        r#"{{"id": "{id}", "center": "{center}", "start": "{start}", "end": "{end}", "construction": false}}"#
    )
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

fn solved(doc: &Checked, w: &mut SketchWorker) -> Result<SketchSolution, silico_core::Error> {
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
    // distance of 30, so the line lands at y 30, 10 from the circle. And a line at y 10
    // across the circle, held there, reads back a reference distance of 10.
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
            point("p0000002", -20.0, 10.0),
            point("p0000003", 20.0, 10.0),
        ],
        &[line("n0000001", "p0000002", "p0000003")],
        &[circle("c0000001", "p0000001", 20.0)],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            r#"{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": -20, "driving": true}"#,
            r#"{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 10, "driving": true}"#,
            r#"{"id": "k0000005", "type": "distance_x", "a": "origin", "b": "p0000003", "value": 20, "driving": true}"#,
            r#"{"id": "k0000006", "type": "distance_y", "a": "origin", "b": "p0000003", "value": 10, "driving": true}"#,
            r#"{"id": "k0000007", "type": "point_line_distance", "point": "p0000001", "line": "n0000001", "value": 0, "side": "left", "driving": false}"#,
        ],
    );
    let s = solved(&doc, &mut worker()).unwrap();
    // The centre is right of a line running +x at y 10.
    assert_eq!(
        s.references[&id("k0000007")],
        silico_core::sketch::Measured::FromLine {
            distance: 10.0,
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
    // it only when it's met on the solution within 1e-8 mm.
    for x0 in [0.0, 999_980.0] {
        let s = solved(&twice_ten(x0, "10"), &mut worker()).unwrap();
        assert_eq!(s.redundant.len(), 1, "{x0}: {s:?}");
        assert_eq!(s.dofs, 0);
        let e = solved(&twice_ten(x0, "10.000001"), &mut worker()).unwrap_err();
        assert_eq!(e.code(), ErrorCode::OverConstrained, "{x0}: {e}");
        assert_eq!(e.elements().len(), 1, "{e}");
    }
}

#[test]
fn a_thirty_sixty_ninety_triangle_from_two_hundred_starts_keeps_its_implied_angle() {
    // The oracle's case for decision 5: a triangle with its corner angles 30°, 60°, and 90°
    // (the third implied by the other two), its base horizontal, 10 long, its first corner
    // at the origin. Drawn from 200 starts about the solution, every solve keeps the
    // redundant angle: PlaneGCS leaves it up to about 1.3e-10 rad off, inside 1e-9.
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
        assert_at(&s, "p0000003", c);
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
}

#[test]
fn a_solution_that_cant_be_built_is_passed_over_for_the_next_algorithms() {
    // TestCoincidentCommandGui.py's collapsed line: FreeCAD's coincidences make its two ends
    // one, exactly, so no algorithm can build it. In Silico a line's ends are two points
    // (the gate refuses one point twice), and the nearest is both held at (5, 5) by
    // dimensions. DogLeg solves that exactly, to a line of no length, which FreeCAD's
    // GeomLineSegment refuses ("Both points are equal": gp::Resolution, so only exactly);
    // its solution is passed over (fc128d357d), and Levenberg–Marquardt's, its ends 1e-10
    // apart, is a line FreeCAD builds too.
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
    let s = solved(&doc, &mut worker()).unwrap();
    assert_eq!(s.algorithm, Algorithm::LevenbergMarquardt);
    let (a, b) = (
        s.point(&id("p0000001")).unwrap(),
        s.point(&id("p0000002")).unwrap(),
    );
    assert!(a != b, "{a:?} {b:?}");
    assert!(
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9,
        "{a:?} {b:?}"
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
