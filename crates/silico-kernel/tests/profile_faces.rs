//! M3.5 end to end: a document's sketch solved, its profile found (`silico_core::profile`),
//! sent as the kernel's faces request (`Profile::faces`), and built by OCCT, each face's
//! area against its analytic value. The sketches are drawn where they solve, and solved by
//! the fake sketch solver (which answers with the values it's sent), so these test the
//! profile and the faces, not the solver; the faces' own cases are tests/faces.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture;

use fixture::{arc, circle, line, point};
use std::f64::consts::PI;

use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeSketches;
use silico_core::kernel::KernelRequest;
use silico_core::profile::profile;
use silico_core::sketch::solve;
use silico_kernel::{make_faces, summarize};

const FEATURE: &str = "s0000001";

/// A document of one sketch on `plane` with these lists, no constraints.
fn document(
    plane: &str,
    points: &[String],
    lines: &[String],
    circles: &[String],
    arcs: &[String],
) -> Checked {
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"{FEATURE}\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", \"plane\": \"{plane}\", \"points\": [{}], \"lines\": [{}], \"circles\": [{}], \"arcs\": [{}], \"constraints\": []}}]}}",
        points.join(", "),
        lines.join(", "),
        circles.join(", "),
        arcs.join(", ")
    );
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

/// The faces' total area, and their count ([`built`]).
fn faces(doc: &Checked) -> (f64, usize) {
    let (summary, areas, _) = built(doc);
    (summary.area, areas.len())
}

fn assert_area(got: f64, want: f64) {
    assert!(
        (got - want).abs() <= 1e-9 * want.abs().max(1.0),
        "area {got}, want {want}"
    );
}

/// A 4 × 3 rectangle at the origin, its lines drawn each way round: p1 → p2 along the
/// bottom, then up, back along the top, and down (or the other way, `reversed`).
fn rectangle(reversed: bool) -> (Vec<String>, Vec<String>) {
    let points = vec![
        point("p0000001", 0.0, 0.0),
        point("p0000002", 4.0, 0.0),
        point("p0000003", 4.0, 3.0),
        point("p0000004", 0.0, 3.0),
    ];
    let ends = [
        ("p0000001", "p0000002"),
        ("p0000002", "p0000003"),
        ("p0000003", "p0000004"),
        ("p0000004", "p0000001"),
    ];
    let lines = ends
        .iter()
        .enumerate()
        .map(|(i, &(a, b))| {
            let id = format!("n000000{}", i + 1);
            if reversed {
                line(&id, b, a)
            } else {
                line(&id, a, b)
            }
        })
        .collect();
    (points, lines)
}

#[test]
fn a_rectangle_drawn_either_way_round_is_one_face_of_its_area() {
    // FreeCAD's faceDrillerSimpleRectangle and faceDrillerReversedOuterWire: 12.
    for reversed in [false, true] {
        let (points, lines) = rectangle(reversed);
        let (area, n) = faces(&document("xy", &points, &lines, &[], &[]));
        assert_eq!(n, 1);
        assert_area(area, 12.0);
    }
}

#[test]
fn a_rectangle_with_a_circular_hole_is_its_area_less_the_holes() {
    // faceDrillerWithOneHole: a hole r 0.5 at (2, 1.5): 12 − π/4.
    let (mut points, lines) = rectangle(false);
    points.push(point("p0000005", 2.0, 1.5));
    let (area, n) = faces(&document(
        "xy",
        &points,
        &lines,
        &[circle("c0000001", "p0000005", 0.5)],
        &[],
    ));
    assert_eq!(n, 1);
    assert_area(area, 12.0 - PI / 4.0);
}

#[test]
fn circles_nested_in_a_square_are_an_island_in_a_hole() {
    // buildEssenceNestedIslands: 10 × 10, circles r 4 and 2 about its centre: the square
    // less the r 4 hole, and the r 2 island: 100 − 16π + 4π = 100 − 12π, in two faces.
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", 10.0, 0.0),
        point("p0000003", 10.0, 10.0),
        point("p0000004", 0.0, 10.0),
        point("p0000005", 5.0, 5.0),
    ];
    let lines = [
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000002", "p0000003"),
        line("n0000003", "p0000003", "p0000004"),
        line("n0000004", "p0000004", "p0000001"),
    ];
    let circles = [
        circle("c0000001", "p0000005", 4.0),
        circle("c0000002", "p0000005", 2.0),
    ];
    let (area, n) = faces(&document("xy", &points, &lines, &circles, &[]));
    assert_eq!(n, 2);
    assert_area(area, 100.0 - 12.0 * PI);
}

/// A slot: lines along y = 0 and y = 2 from x = 0 to 4, closed by half circles r 1 about
/// (4, 1) and (0, 1), each drawn counter-clockwise: 4 × 2 + π.
fn slot(dx: f64, dy: f64) -> (Vec<String>, Vec<String>, Vec<String>) {
    let p = |id: &str, x: f64, y: f64| point(id, x + dx, y + dy);
    let points = vec![
        p("q0000001", 0.0, 0.0),
        p("q0000002", 4.0, 0.0),
        p("q0000003", 4.0, 2.0),
        p("q0000004", 0.0, 2.0),
        p("q0000005", 4.0, 1.0),
        p("q0000006", 0.0, 1.0),
    ];
    let lines = vec![
        line("m0000001", "q0000001", "q0000002"),
        line("m0000002", "q0000003", "q0000004"),
    ];
    let arcs = vec![
        arc("b0000001", "q0000005", "q0000002", "q0000003"),
        arc("b0000002", "q0000006", "q0000004", "q0000001"),
    ];
    (points, lines, arcs)
}

#[test]
fn a_slot_of_lines_and_arcs_is_its_area() {
    let (points, lines, arcs) = slot(0.0, 0.0);
    let (area, n) = faces(&document("xy", &points, &lines, &[], &arcs));
    assert_eq!(n, 1);
    assert_area(area, 8.0 + PI);
}

#[test]
fn a_slot_as_a_hole_walks_its_arcs_against_their_drawing() {
    // A hole runs clockwise, so its arcs, drawn counter-clockwise, are sent end first with
    // their angles swapped: 10 × 10 less the slot, 100 − 8 − π.
    let (mut points, mut lines, arcs) = slot(3.0, 4.0);
    points.extend([
        point("p0000001", 0.0, 0.0),
        point("p0000002", 10.0, 0.0),
        point("p0000003", 10.0, 10.0),
        point("p0000004", 0.0, 10.0),
    ]);
    lines.extend([
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000002", "p0000003"),
        line("n0000003", "p0000003", "p0000004"),
        line("n0000004", "p0000004", "p0000001"),
    ]);
    let (area, n) = faces(&document("xy", &points, &lines, &[], &arcs));
    assert_eq!(n, 1);
    assert_area(area, 100.0 - 8.0 - PI);
}

#[test]
fn a_rectangle_on_each_plane_is_its_area() {
    // The 4 × 3 rectangle from the origin, its x and y along the plane's (D6): on XY +X
    // and +Y, on XZ +X and +Z, on YZ +Y and +Z.
    for (plane, far) in [
        ("xy", [4.0, 3.0, 0.0]),
        ("xz", [4.0, 0.0, 3.0]),
        ("yz", [0.0, 4.0, 3.0]),
    ] {
        let (points, lines) = rectangle(false);
        let (summary, ..) = built(&document(plane, &points, &lines, &[], &[]));
        assert_area(summary.area, 12.0);
        let [low, high] = summary.bounds.unwrap();
        for (got, want) in low.iter().zip([0.0; 3]).chain(high.iter().zip(far)) {
            assert!((got - want).abs() <= 1e-9, "{plane}: {low:?} to {high:?}");
        }
    }
}

/// FreeCAD's slot plate (TestSketcherSolver.py's `CreateSlotPlateSet`), where it solves
/// (silico-sketch's `freecads_slot_plate_rises_to_its_apex`): a bottom from (200, 0) to the
/// origin, a side at 50° up to the apex (80 / tan 50°, 80), a top to (200, 80), and an arc
/// of radius 40 about (200, 40) back down to the start.
fn slot_plate() -> (Vec<String>, Vec<String>, Vec<String>) {
    let apex = 80.0 / libm::tan(50f64.to_radians());
    (
        vec![
            point("p0000000", 200.0, 0.0),
            point("p0000001", 0.0, 0.0),
            point("p0000002", apex, 80.0),
            point("p0000003", 200.0, 80.0),
            point("p0000004", 200.0, 40.0),
        ],
        vec![
            line("n0000000", "p0000000", "p0000001"),
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000002", "p0000003"),
        ],
        vec![arc("a0000003", "p0000004", "p0000000", "p0000003")],
    )
}

/// The document's sketch solved, its profile found, sent through the core's request
/// check, and built: the faces' summary, and the profile's regions' areas and holes.
fn built(doc: &Checked) -> (silico_protocol::ShapeSummary, Vec<f64>, Vec<usize>) {
    let feature = FeatureId::new(FEATURE).unwrap();
    let solution = solve(doc, &feature, &mut FakeSketches::new()).unwrap();
    let found = profile(doc, &feature, &solution).unwrap_or_else(|r| panic!("{:?}", r.errors()));
    let regions = found.faces();
    KernelRequest::make_faces(&regions).unwrap();
    let shape = make_faces(&regions).unwrap().shape;
    (
        summarize(&shape).unwrap(),
        found.regions.iter().map(|r| r.area()).collect(),
        found.regions.iter().map(|r| r.holes.len()).collect(),
    )
}

#[test]
fn freecads_slot_plate_is_one_face_of_four_edges_and_its_area() {
    // testSlotCase: the plate's shape has 4 edges. Its area by hand: the trapezoid under
    // the top, 80 × (200 + (200 − apex)) / 2, and the arc's half disc, 800π. FreeCAD
    // measures 15828.157343, its angle datum 0.872665 rad where Silico's is 50° exactly:
    // 40 × the apex's difference, 2.04e-3, apart.
    let (points, lines, arcs) = slot_plate();
    let (summary, areas, holes) = built(&document("xy", &points, &lines, &[], &arcs));
    let apex = 80.0 / libm::tan(50f64.to_radians());
    let want = 80.0 * (400.0 - apex) / 2.0 + 800.0 * PI;
    assert_eq!((summary.faces, summary.edges), (1, 4));
    assert_area(summary.area, want);
    assert_eq!(holes, [0]);
    assert!((areas[0] - want).abs() <= 1e-9 * want);
    let freecad_apex = 80.0 / libm::tan(0.872665);
    assert!((15828.157343 - (80.0 * (400.0 - freecad_apex) / 2.0 + 800.0 * PI)).abs() < 1e-6);
}

#[test]
fn freecads_slot_plate_with_its_inner_set_is_one_face_with_two_holes() {
    // testSlotCase, then `CreateSlotPlateInnerSet`: a circle, and a hole of three lines and
    // an arc; the shape has 9 edges, one face with two holes. The inner set has 9 degrees
    // of freedom, so where it solves depends on where the solve starts: FreeCAD's face is
    // 8607.71344, and Silico's solve from FreeCAD's drawing, its shared points starting
    // where the lines end, puts the hole as drawn here.
    let (mut points, mut lines, mut arcs) = slot_plate();
    points.extend([
        point("p0000010", 195.055893, 39.562252),
        point("p0000011", 150.16951703902004, 13.050452947874792),
        point("p0000012", 36.70045615522816, 13.050452947874792),
        point("p0000013", 79.11862023739202, 63.602452384044234),
        point("p0000014", 148.15210260767645, 63.602452384044234),
        point("p0000015", 192.5722413533035, 40.05890339848491),
    ]);
    lines.extend([
        line("n0000005", "p0000011", "p0000012"),
        line("n0000006", "p0000012", "p0000013"),
        line("n0000007", "p0000013", "p0000014"),
    ]);
    arcs.push(arc("a0000008", "p0000015", "p0000014", "p0000011"));
    let circles = [circle("c0000004", "p0000010", 29.846098)];
    let (summary, areas, holes) = built(&document("xy", &points, &lines, &circles, &arcs));
    assert_eq!((summary.faces, summary.edges), (1, 9));
    assert_eq!(holes, [2]);
    // By hand: the plate, less the circle, less the hole of lines and arc: the trapezoid
    // between its two horizontal sides, less the arc's segment, which bulges into it from
    // its right side. 15828.155303 − 2798.497956 − (4612.934232 − 233.608346).
    let apex = 80.0 / libm::tan(50f64.to_radians());
    let plate = 80.0 * (400.0 - apex) / 2.0 + 800.0 * PI;
    let circle = PI * 29.846098 * 29.846098;
    let (bottom, top) = (
        150.16951703902004 - 36.70045615522816,
        148.15210260767645 - 79.11862023739202,
    );
    let trapezoid = (63.602452384044234 - 13.050452947874792) * (bottom + top) / 2.0;
    let c = [192.5722413533035, 40.05890339848491];
    let (start, end) = (
        [148.15210260767645 - c[0], 63.602452384044234 - c[1]],
        [150.16951703902004 - c[0], 13.050452947874792 - c[1]],
    );
    let r = libm::hypot(start[0], start[1]);
    let theta = libm::atan2(end[1], end[0]) + 2.0 * PI - libm::atan2(start[1], start[0]);
    let segment = r * r / 2.0 * (theta - libm::sin(theta));
    let want = plate - circle - (trapezoid - segment);
    assert!((want - 8650.331462).abs() < 1e-6, "{want}");
    assert_area(summary.area, want);
    // And the profile's own measure agrees with OCCT's.
    assert_area(summary.area, areas[0]);
}
