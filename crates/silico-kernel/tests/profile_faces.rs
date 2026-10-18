//! M3.5 end to end: a document's sketch solved, its profile found (`silico_core::profile`),
//! sent as the kernel's faces request (`Profile::faces`), and built by OCCT, each face's
//! area against its analytic value. The sketches are drawn where they solve, and solved by
//! the fake sketch solver (which answers with the values it's sent), so these test the
//! profile and the faces, not the solver; the faces' own cases are tests/faces.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::PI;

use silico_core::document::{Checked, FeatureId, Operation, read};
use silico_core::fake::FakeSketches;
use silico_core::kernel::KernelRequest;
use silico_core::profile::profile;
use silico_core::sketch::solve;
use silico_kernel::{make_faces, summarize};

const FEATURE: &str = "s0000001";

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

/// The faces' total area, and their count, from the document's sketch: solved, its
/// profile found, sent through the core's request check, and built.
fn faces(doc: &Checked) -> (f64, usize) {
    let feature = FeatureId::new(FEATURE).unwrap();
    let solution = solve(doc, &feature, &mut FakeSketches::new()).unwrap();
    let found = profile(doc, &feature, &solution).unwrap_or_else(|r| panic!("{:?}", r.errors()));
    let Operation::Sketch(sketch) = &doc.document().features[0].operation else {
        panic!()
    };
    let regions = found.faces(sketch.plane, &solution).unwrap();
    KernelRequest::make_faces(&regions).unwrap();
    let built = make_faces(&regions).unwrap();
    (summarize(&built.shape).unwrap().area, regions.regions.len())
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
    for plane in ["xy", "xz", "yz"] {
        let (points, lines) = rectangle(false);
        let (area, _) = faces(&document(plane, &points, &lines, &[], &[]));
        assert_area(area, 12.0);
    }
}
