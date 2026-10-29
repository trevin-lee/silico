//! Splitting a curve (docs/ports/sketch-edit.md, §3's table and §9's tests; decisions 1 to
//! 4): FreeCAD's `testSplitLineSegment`, `testSplitCircle`, and `testSplitArcOfCircle`, and
//! Silico's own. Each result goes through the validation gate; each expected sketch, list,
//! and coordinate is worked by hand, on positions whose projections are exact in binary.

use silico_core::Error;
use silico_core::document::sketch_edit::{Split, split};
use silico_core::document::{
    Direction, Document, ElementId, Form, Operation, Reference, Sketch, TangentSide, read, validate,
};

use crate::fixture::{arc, circle, circle_as, document, fields, line, line_as, point};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn ids(texts: &[&str]) -> Vec<ElementId> {
    texts.iter().map(|t| id(t)).collect()
}

fn element(text: &str) -> Reference {
    Reference::Element(id(text))
}

fn doc(points: &str, lines: &str, circles: &str, arcs: &str, constraints: &str) -> Document {
    read(document(&fields(points, lines, circles, arcs, constraints)).as_bytes()).unwrap()
}

fn sketch_of(document: &Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

/// The split's sketch in place of the document's, checked by the gate.
#[track_caller]
fn gated(document: &Document, split: &Split) -> Sketch {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(split.edited.sketch.clone());
    if let Err(invalid) = validate(&document) {
        panic!("the gate refused the split: {:?}", invalid.errors());
    }
    sketch_of(&document).clone()
}

/// The constraint `name`'s form.
#[track_caller]
fn form<'a>(sketch: &'a Sketch, name: &str) -> &'a Form {
    &sketch
        .constraints
        .iter()
        .find(|c| c.id == id(name))
        .unwrap_or_else(|| panic!("no constraint {name}"))
        .form
}

/// A line's ends, by ID.
#[track_caller]
fn ends(sketch: &Sketch, name: &str) -> (String, String) {
    let line = sketch.lines.iter().find(|l| l.id == id(name)).unwrap();
    (line.from.to_string(), line.to.to_string())
}

/// An arc's centre, start, and end, by ID.
#[track_caller]
fn arc_points(sketch: &Sketch, name: &str) -> [String; 3] {
    let arc = sketch.arcs.iter().find(|a| a.id == id(name)).unwrap();
    [
        arc.center.to_string(),
        arc.start.to_string(),
        arc.end.to_string(),
    ]
}

/// A point's coordinates and `construction`.
#[track_caller]
fn at(sketch: &Sketch, name: &str) -> (f64, f64, bool) {
    let p = sketch.points.iter().find(|p| p.id == id(name)).unwrap();
    (p.x.mm(), p.y.mm(), p.construction)
}

/// The refusal's message, parameter, and elements.
fn said(error: &Error) -> (&str, Option<&str>, Vec<&str>) {
    (
        error.message(),
        error.parameter(),
        error.elements().iter().map(String::as_str).collect(),
    )
}

/// Line a0000000 from p0 (0, 0) to p4 (32, 0) along x, beside the points the tests use: a
/// length that's a power of two, so every projection onto it is exact.
const POINTS: &str = r#"{"id": "p0000000", "x": 0, "y": 0, "construction": false}, {"id": "p0000004", "x": 32, "y": 0, "construction": false}, {"id": "q0000000", "x": 10, "y": 10, "construction": false}, {"id": "q0000002", "x": 20, "y": 10, "construction": false}"#;

fn a_line(points: &str, lines: &str, circles: &str, arcs: &str, constraints: &str) -> Document {
    let points = if points.is_empty() {
        POINTS.to_owned()
    } else {
        format!("{POINTS}, {points}")
    };
    let lines = std::iter::once(line("a0000000", "p0000000", "p0000004"))
        .chain((!lines.is_empty()).then(|| lines.to_owned()))
        .collect::<Vec<_>>()
        .join(", ");
    doc(&points, &lines, circles, arcs, constraints)
}

// ---- lines

#[test]
fn a_line_splits_at_the_positions_projection_into_pieces_joined_collinear() {
    // testSplitLineSegment, at a position 5e-8 mm off the line (within 1e-7 mm): the new
    // point at its projection, (10, 0), 10/32 of the way; the start-side piece keeps
    // a's ID; the pieces joined by a tangent_at (same), FreeCAD main's endpoint Tangent
    // (6d06b61c7e). The horizontal and a parallel stay on the start-side piece (decision
    // 1), so nothing is redundant, where 1.1.3 copies them to both.
    let before = a_line(
        "",
        &line("e0000000", "q0000000", "q0000002"),
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}, {"id": "k0000001", "type": "parallel", "a": "e0000000", "b": "a0000000"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.00000005]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(at(&sketch, "n0000000"), (10.0, 0.0, false));
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("p0000000".to_owned(), "n0000000".to_owned())
    );
    assert_eq!(
        ends(&sketch, "b1000000"),
        ("n0000000".to_owned(), "p0000004".to_owned())
    );
    assert_eq!(
        form(&sketch, "j0000000"),
        &Form::TangentAt {
            a: element("a0000000"),
            b: element("b1000000"),
            at: element("n0000000"),
            direction: Direction::Same,
        }
    );
    assert_eq!(
        form(&sketch, "k0000000"),
        &Form::Horizontal {
            line: element("a0000000")
        }
    );
    assert_eq!(
        form(&sketch, "k0000001"),
        &Form::Parallel {
            a: element("e0000000"),
            b: element("a0000000")
        }
    );
    assert_eq!(sketch.constraints.len(), 3);
    assert_eq!(done.pieces, [id("a0000000"), id("b1000000")]);
    assert_eq!(
        done.edited.added,
        ids(&["n0000000", "b1000000", "j0000000"])
    );
    assert_eq!(done.edited.modified, ids(&["a0000000"]));
    assert!(done.edited.removed.is_empty());
}

#[test]
fn freecads_split_position_off_the_line_is_refused_and_its_projection_splits() {
    // testSplitLineSegment's line, (1, 2) to (3, 4), and its split point (2, 3.1), which
    // FreeCAD projects: 0.1/√2 = 0.0707106781186547… mm off, so refused (decision 4). At
    // (2, 3), the line's midpoint, it splits there.
    let before = doc(
        r#"{"id": "p0000001", "x": 1, "y": 2, "construction": false}, {"id": "p0000002", "x": 3, "y": 4, "construction": false}"#,
        &line("a0000000", "p0000001", "p0000002"),
        "",
        "",
        "",
    );
    let error = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[2.0, 3.1]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap_err();
    let off: f64 = error
        .message()
        .strip_prefix("the split position is ")
        .and_then(|m| m.strip_suffix(" mm from line a0000000, farther than 1e-7 mm"))
        .unwrap_or_else(|| panic!("{}", error.message()))
        .parse()
        .unwrap();
    assert!((off - 0.1 / 2.0_f64.sqrt()).abs() < 1e-15, "{off}");
    assert_eq!(error.parameter(), Some("at.0"));
    assert_eq!(error.elements(), ["a0000000"]);
    assert_eq!(error.suggestion(), Some("give a position on the curve"));
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[2.0, 3.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(at(&sketch, "n0000000"), (2.0, 3.0, false));
    // One curve added, and one tangency joining them (FreeCAD's counts).
    assert_eq!(sketch.lines.len(), 2);
    assert_eq!(sketch.constraints.len(), 1);
}

#[test]
fn a_lines_length_and_midpoint_become_its_outer_ends_and_its_equal_is_dropped() {
    // §3's table: a length, a distance of the outer ends (4d6b1f3eb8); a midpoint, a
    // symmetric of them about the point; an equal on a line, dropped (857190a20a, #25008);
    // an angle and a perpendicular to a line neither piece touches stay on the start-side
    // piece (decision 1; main drops the perpendicular, a bug). Another line's length stays
    // as it is, and an equal naming a second, dropped as well.
    let points = r#"{"id": "m0000000", "x": 16, "y": 0, "construction": false}, {"id": "r0000000", "x": 30, "y": 5, "construction": false}, {"id": "r0000001", "x": 30, "y": 15, "construction": false}"#;
    let lines = [
        line("e0000000", "q0000000", "q0000002"),
        line("f0000000", "r0000000", "r0000001"),
    ]
    .join(", ");
    let before = a_line(
        points,
        &lines,
        "",
        "",
        r#"{"id": "k0000000", "type": "length", "line": "a0000000", "value": 32, "driving": true}, {"id": "k0000001", "type": "midpoint", "point": "m0000000", "line": "a0000000"}, {"id": "k0000002", "type": "equal", "a": "a0000000", "b": "e0000000"}, {"id": "k0000003", "type": "angle", "a": "a0000000", "b": "e0000000", "value": 0, "driving": false}, {"id": "k0000004", "type": "perpendicular", "a": "a0000000", "b": "f0000000"}, {"id": "k0000005", "type": "length", "line": "e0000000", "value": 10, "driving": true}, {"id": "k0000006", "type": "equal", "a": "f0000000", "b": "a0000000"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    match form(&sketch, "k0000000") {
        Form::Distance {
            a,
            b,
            value,
            driving,
        } => {
            assert_eq!((a, b), (&element("p0000000"), &element("p0000004")));
            assert_eq!((value.to_string(), *driving), ("32".to_owned(), true));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        form(&sketch, "k0000001"),
        &Form::Symmetric {
            a: element("p0000000"),
            b: element("p0000004"),
            about: element("m0000000"),
        }
    );
    assert!(!sketch.constraints.iter().any(|c| c.id == id("k0000002")));
    assert_eq!(
        form(&sketch, "k0000003"),
        form(sketch_of(&before), "k0000003")
    );
    assert_eq!(
        form(&sketch, "k0000004"),
        &Form::Perpendicular {
            a: element("a0000000"),
            b: element("f0000000")
        }
    );
    assert_eq!(
        form(&sketch, "k0000005"),
        form(sketch_of(&before), "k0000005")
    );
    assert!(!sketch.constraints.iter().any(|c| c.id == id("k0000006")));
    assert_eq!(
        done.edited.modified,
        ids(&["a0000000", "k0000000", "k0000001"])
    );
    assert_eq!(done.edited.removed, ids(&["k0000002", "k0000006"]));
}

#[test]
fn a_point_held_on_the_line_goes_to_the_piece_whose_span_holds_it_or_the_nearer_ends() {
    // FreeCAD bbbff1f8f1 (#28240: 1.1.3 drops these): s0 at 5 on the first piece, s1 at 30
    // on the second, s2 at 50 past p4 (at 32) and s3 at -5 past p0 (closestParameter's nearer end),
    // and d0 held 3 mm left of the line above 25, on the second.
    let points = r#"{"id": "s0000000", "x": 5, "y": 0, "construction": false}, {"id": "s0000001", "x": 30, "y": 0, "construction": false}, {"id": "s0000002", "x": 50, "y": 0, "construction": false}, {"id": "s0000003", "x": -5, "y": 0, "construction": false}, {"id": "d0000000", "x": 25, "y": 3, "construction": false}"#;
    let before = a_line(
        points,
        "",
        "",
        "",
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "a0000000"}, {"id": "k0000001", "type": "on", "point": "s0000001", "curve": "a0000000"}, {"id": "k0000002", "type": "on", "point": "s0000002", "curve": "a0000000"}, {"id": "k0000003", "type": "on", "point": "s0000003", "curve": "a0000000"}, {"id": "k0000004", "type": "point_line_distance", "point": "d0000000", "line": "a0000000", "value": 3, "side": "left", "driving": true}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    let on = |name: &str| match form(&sketch, name) {
        Form::On { curve, .. } | Form::PointLineDistance { line: curve, .. } => curve.to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        ["k0000000", "k0000001", "k0000002", "k0000003", "k0000004"].map(on),
        ["a0000000", "b1000000", "b1000000", "a0000000", "b1000000"]
    );
    assert_eq!(
        done.edited.modified,
        ids(&["a0000000", "k0000001", "k0000002", "k0000004"])
    );
}

#[test]
fn a_point_held_on_the_line_at_the_split_by_an_on_is_shared_and_the_on_goes() {
    // s0 at (10, 0) is held on a (k0). Split at s0's place, the pieces share s0, no point
    // is made, and the on goes as implied.
    let before = a_line(
        r#"{"id": "s0000000", "x": 10, "y": 0, "construction": false}"#,
        "",
        "",
        "",
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "a0000000"}"#,
    );
    // Two new IDs, not three: the piece and the join.
    let error = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap_err();
    assert_eq!(
        said(&error),
        ("the split needs 2 new IDs, got 3", Some("ids"), vec![])
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("p0000000".to_owned(), "s0000000".to_owned())
    );
    assert_eq!(
        ends(&sketch, "b1000000"),
        ("s0000000".to_owned(), "p0000004".to_owned())
    );
    assert_eq!(sketch.points.len(), sketch_of(&before).points.len());
    assert_eq!(done.edited.added, ids(&["b1000000", "j0000000"]));
    assert_eq!(done.edited.modified, ids(&["a0000000"]));
    assert_eq!(done.edited.removed, ids(&["k0000000"]));
}

#[test]
fn a_point_held_on_the_line_at_the_split_by_a_tangent_on_is_shared_and_it_becomes_a_tangent_at() {
    // Arc c about z0 (10, 10) starts at s0 (10, 0) tangent on a (k1: leaving its start
    // along +x, as a runs: same), which holds s0 on a as an on would (decision 5). Split at
    // s0's place, the pieces share s0, and the tangent_on becomes a tangent_at of c and the
    // start-side piece, whose own direction at s0 is a's: same.
    let points = r#"{"id": "s0000000", "x": 10, "y": 0, "construction": false}, {"id": "z0000000", "x": 10, "y": 10, "construction": false}, {"id": "z0000001", "x": 20, "y": 10, "construction": false}"#;
    let before = a_line(
        points,
        "",
        "",
        &arc("c0000000", "z0000000", "s0000000", "z0000001"),
        r#"{"id": "k0000001", "type": "tangent_on", "a": "c0000000", "at": "s0000000", "b": "a0000000", "direction": "same"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        ends(&sketch, "b1000000"),
        ("s0000000".to_owned(), "p0000004".to_owned())
    );
    assert_eq!(
        form(&sketch, "k0000001"),
        &Form::TangentAt {
            a: element("c0000000"),
            b: element("a0000000"),
            at: element("s0000000"),
            direction: Direction::Same,
        }
    );
    assert_eq!(sketch.points.len(), sketch_of(&before).points.len());
    assert_eq!(done.edited.added, ids(&["b1000000", "j0000000"]));
    assert_eq!(done.edited.modified, ids(&["a0000000", "k0000001"]));
    assert!(done.edited.removed.is_empty());
}

#[test]
fn a_tangent_on_whose_point_lies_on_the_second_piece_moves_to_it() {
    // Arc c about z0 (30, 10) starts at t0 (30, 0), on a's second half, tangent on a.
    let points = r#"{"id": "t0000000", "x": 30, "y": 0, "construction": false}, {"id": "z0000000", "x": 30, "y": 10, "construction": false}, {"id": "z0000001", "x": 40, "y": 10, "construction": false}"#;
    let before = a_line(
        points,
        "",
        "",
        &arc("c0000000", "z0000000", "t0000000", "z0000001"),
        r#"{"id": "k0000000", "type": "tangent_on", "a": "c0000000", "at": "t0000000", "b": "a0000000", "direction": "same"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        form(&sketch, "k0000000"),
        &Form::TangentOn {
            a: element("c0000000"),
            at: element("t0000000"),
            b: element("b1000000"),
            direction: Direction::Same,
        }
    );
}

#[test]
fn an_edge_tangent_goes_to_the_piece_whose_span_holds_the_contact() {
    // Circles of radius 10 resting on a: c0 about (30, 10) touches at 30, the second
    // piece; c1 about (5, 10), listed as a, at 5, the first; c2 about (60, 10) at 60, past
    // p4 (at 32), the nearer end's piece (decision 2); and arc r about q2 (20, 10), from q0
    // (10, 10) round under it to z0 (30, 10), listed as a, at 20, the second.
    let points = r#"{"id": "z0000000", "x": 30, "y": 10, "construction": false}, {"id": "z0000001", "x": 5, "y": 10, "construction": false}, {"id": "z0000002", "x": 60, "y": 10, "construction": false}"#;
    let circles = [
        circle("c0000000", "z0000000", 10.0),
        circle("c1000000", "z0000001", 10.0),
        circle("c2000000", "z0000002", 10.0),
    ]
    .join(", ");
    let before = a_line(
        points,
        "",
        &circles,
        &arc("r0000000", "q0000002", "q0000000", "z0000000"),
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "c0000000", "side": "left"}, {"id": "k0000001", "type": "tangent", "a": "c1000000", "b": "a0000000", "side": "left"}, {"id": "k0000002", "type": "tangent", "a": "a0000000", "b": "c2000000", "side": "left"}, {"id": "k0000003", "type": "tangent", "a": "r0000000", "b": "a0000000", "side": "left"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    let pair = |name: &str| match form(&sketch, name) {
        Form::Tangent { a, b, .. } => (a.to_string(), b.to_string()),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        ["k0000000", "k0000001", "k0000002", "k0000003"].map(pair),
        [
            ("b1000000".to_owned(), "c0000000".to_owned()),
            ("c1000000".to_owned(), "a0000000".to_owned()),
            ("b1000000".to_owned(), "c2000000".to_owned()),
            ("r0000000".to_owned(), "b1000000".to_owned()),
        ]
    );
    assert_eq!(
        done.edited.modified,
        ids(&["a0000000", "k0000000", "k0000002", "k0000003"])
    );
}

#[test]
fn a_construction_lines_pieces_and_point_are_construction_and_no_other_point_moves() {
    // 23ecf94ab6 (#12715); and c6c084f22c (#23711): no solve midway, so every point the
    // split doesn't make keeps its bits (0.1 + 0.2 isn't 0.3).
    let before = doc(
        r#"{"id": "p0000000", "x": 0.1, "y": 0.30000000000000004, "construction": false}, {"id": "p0000004", "x": 40.1, "y": 0.30000000000000004, "construction": false}, {"id": "q0000000", "x": -7.000000000000001, "y": 3, "construction": false}"#,
        &line_as("a0000000", "p0000000", "p0000004", true),
        "",
        "",
        "",
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[20.1, 0.30000000000000004]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert!(sketch.lines.iter().all(|l| l.construction));
    assert!(at(&sketch, "n0000000").2);
    for original in &sketch_of(&before).points {
        let kept = sketch.points.iter().find(|p| p.id == original.id).unwrap();
        assert_eq!(kept, original);
    }
}

#[test]
fn a_split_at_past_within_1e_7_mm_of_or_off_a_lines_ends_is_refused_naming_it() {
    let before = a_line("", "", "", "", "");
    let refused = |position: [f64; 2]| {
        split(
            sketch_of(&before),
            &id("a0000000"),
            &[position],
            &ids(&["n0000000", "b1000000", "j0000000"]),
        )
        .unwrap_err()
    };
    // 2^-24 (5.960464477539063e-8) from an end, exactly: within 1e-7 mm; 1e-7 from one,
    // the floor itself: within too.
    let near = 1.0 / 16_777_216.0;
    assert_eq!(near, 5.960464477539063e-8);
    for (position, message, end) in [
        (
            [0.0, 0.0],
            "the split position is at line a0000000's end p0000000",
            "p0000000",
        ),
        (
            [32.0, 0.0],
            "the split position is at line a0000000's end p0000004",
            "p0000004",
        ),
        (
            [-1.0, 0.0],
            "the split position is past line a0000000's end p0000000",
            "p0000000",
        ),
        (
            [33.0, 0.0],
            "the split position is past line a0000000's end p0000004",
            "p0000004",
        ),
        (
            [0.0000001, 0.0],
            "the split position is 1e-7 mm from line a0000000's end p0000000, within 1e-7 mm",
            "p0000000",
        ),
        (
            [near, 0.0],
            "the split position is 5.960464477539063e-8 mm from line a0000000's end p0000000, within 1e-7 mm",
            "p0000000",
        ),
        (
            [32.0 - near, 0.0],
            "the split position is 5.960464477539063e-8 mm from line a0000000's end p0000004, within 1e-7 mm",
            "p0000004",
        ),
        // Past an end by less than 1e-7 mm: within it, said so.
        (
            [-near, 0.0],
            "the split position is 5.960464477539063e-8 mm from line a0000000's end p0000000, within 1e-7 mm",
            "p0000000",
        ),
    ] {
        let error = refused(position);
        assert_eq!(
            said(&error),
            (message, Some("at.0"), vec!["a0000000", end]),
            "{position:?}"
        );
        assert_eq!(
            error.suggestion(),
            Some("give a position between the curve's ends")
        );
    }
    // Just past the floor from p0: it splits.
    split(
        sketch_of(&before),
        &id("a0000000"),
        &[[0.0000002, 0.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    assert_eq!(
        said(&refused([10.0, 1.0])),
        (
            "the split position is 1 mm from line a0000000, farther than 1e-7 mm",
            Some("at.0"),
            vec!["a0000000"]
        )
    );
    // 2e-7 off: farther.
    assert_eq!(
        refused([10.0, 0.0000002]).message(),
        "the split position is 2e-7 mm from line a0000000, farther than 1e-7 mm"
    );
    assert_eq!(
        said(&refused([f64::NAN, 0.0])),
        (
            "the split position isn't a finite position",
            Some("at.0"),
            vec!["a0000000"]
        )
    );
}

#[test]
fn a_member_that_isnt_a_curve_the_wrong_count_of_positions_and_bad_ids_are_refused() {
    let before = a_line(
        "",
        "",
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}"#,
    );
    let sketch = sketch_of(&before);
    let three = ids(&["n0000000", "b1000000", "j0000000"]);
    let at = [[10.0, 0.0]];
    for (curve, message) in [
        (
            "p0000000",
            "point p0000000 isn't a curve: a line, an arc, or a circle splits",
        ),
        (
            "k0000000",
            "constraint k0000000 isn't a curve: a line, an arc, or a circle splits",
        ),
    ] {
        let error = split(sketch, &id(curve), &at, &three).unwrap_err();
        assert_eq!(said(&error), (message, Some("curve"), vec![curve]));
    }
    let error = split(sketch, &id("zzzzzzzz"), &at, &three).unwrap_err();
    assert_eq!(
        (error.message(), error.parameter()),
        ("the sketch has no curve zzzzzzzz", Some("curve"))
    );
    for positions in [&[][..], &[[10.0, 0.0], [20.0, 0.0]]] {
        let error = split(sketch, &id("a0000000"), positions, &three).unwrap_err();
        assert_eq!(
            said(&error),
            (
                &*format!("a line splits at one position, got {}", positions.len()),
                Some("at"),
                vec!["a0000000"]
            )
        );
    }
    let error = split(sketch, &id("a0000000"), &at, &three[..2]).unwrap_err();
    assert_eq!(
        said(&error),
        ("the split needs 3 new IDs, got 2", Some("ids"), vec![])
    );
    let error = split(
        sketch,
        &id("a0000000"),
        &at,
        &ids(&["n0000000", "p0000004", "j0000000"]),
    )
    .unwrap_err();
    assert_eq!(
        said(&error),
        (
            "the sketch already has a member p0000004",
            Some("ids.1"),
            vec![]
        )
    );
    let error = split(
        sketch,
        &id("a0000000"),
        &at,
        &ids(&["n0000000", "b1000000", "n0000000"]),
    )
    .unwrap_err();
    assert_eq!(
        said(&error),
        ("ID n0000000 is given twice", Some("ids.2"), vec![])
    );
    // The sketch it was given is as it was.
    assert_eq!(sketch, sketch_of(&before));
}

// ---- arcs

/// testSplitArcOfCircle's arc, about (1, 2), here of radius 5 so its points are exact:
/// counter-clockwise from r0 (4, 6), at atan(4/3) (FreeCAD's starts at π/3), to r1 (1, −3),
/// at 3π/2 (FreeCAD's too); a line e ending at r0 along the arc's direction there, (−4, 3),
/// and a line f starting at r1 along it there, (1, 0).
fn the_arc(constraints: &str) -> Document {
    let points = [
        point("c0000000", 1.0, 2.0),
        point("r0000000", 4.0, 6.0),
        point("r0000001", 1.0, -3.0),
        point("g0000000", 8.0, 3.0),
        point("g0000001", 5.0, -3.0),
    ]
    .join(", ");
    let lines = [
        line("e0000000", "g0000000", "r0000000"),
        line("f0000000", "r0000001", "g0000001"),
    ]
    .join(", ");
    doc(
        &points,
        &lines,
        "",
        &arc("a0000000", "c0000000", "r0000000", "r0000001"),
        constraints,
    )
}

#[test]
fn an_arc_splits_into_two_sharing_its_centre_and_the_tangencies_at_its_ends_stay_there() {
    // testSplitArcOfCircle at (−4, 2), at π on the arc: the pieces share the new point and
    // the centre, so no constraint joins them (FreeCAD's two Coincidents); the radius stays
    // on the start-side piece; the tangent_at at the start stays on it, the one at the end
    // goes with that end to the new piece.
    let before = the_arc(
        r#"{"id": "k0000000", "type": "radius", "curve": "a0000000", "value": 5, "driving": true}, {"id": "k0000001", "type": "tangent_at", "a": "e0000000", "b": "a0000000", "at": "r0000000", "direction": "same"}, {"id": "k0000002", "type": "tangent_at", "a": "a0000000", "b": "f0000000", "at": "r0000001", "direction": "same"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[-4.0, 2.0]],
        &ids(&["n0000000", "b1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(at(&sketch, "n0000000"), (-4.0, 2.0, false));
    assert_eq!(
        arc_points(&sketch, "a0000000"),
        ["c0000000", "r0000000", "n0000000"]
    );
    assert_eq!(
        arc_points(&sketch, "b1000000"),
        ["c0000000", "n0000000", "r0000001"]
    );
    assert_eq!(
        form(&sketch, "k0000000"),
        form(sketch_of(&before), "k0000000")
    );
    assert_eq!(
        form(&sketch, "k0000001"),
        form(sketch_of(&before), "k0000001")
    );
    assert_eq!(
        form(&sketch, "k0000002"),
        &Form::TangentAt {
            a: element("b1000000"),
            b: element("f0000000"),
            at: element("r0000001"),
            direction: Direction::Same,
        }
    );
    assert_eq!(sketch.constraints.len(), 3);
    assert_eq!(done.pieces, [id("a0000000"), id("b1000000")]);
    assert_eq!(done.edited.added, ids(&["n0000000", "b1000000"]));
    assert_eq!(done.edited.modified, ids(&["a0000000", "k0000002"]));
}

#[test]
fn freecads_arc_split_position_off_the_circle_and_one_past_its_ends_are_refused() {
    let before = the_arc("");
    let refused = |position: [f64; 2]| {
        split(
            sketch_of(&before),
            &id("a0000000"),
            &[position],
            &ids(&["n0000000", "b1000000"]),
        )
        .unwrap_err()
    };
    // testSplitArcOfCircle's (−2, 3.1): 5 − √10.21 = 1.80469… mm off the circle.
    let error = refused([-2.0, 3.1]);
    let off: f64 = error
        .message()
        .strip_prefix("the split position is ")
        .and_then(|m| m.strip_suffix(" mm from arc a0000000, farther than 1e-7 mm"))
        .unwrap_or_else(|| panic!("{}", error.message()))
        .parse()
        .unwrap();
    assert!((off - (5.0 - 10.21_f64.sqrt())).abs() < 1e-14, "{off}");
    // (6, 2), at 0, outside the sweep from atan(4/3) to 3π/2: past it, nearer its start
    // (√20 mm from r0, √50 from r1); (3.5, 2 − 2.5√3), at 5π/3, nearer its end (√8.25... :
    // 2.59 mm from r1, 8.35 from r0).
    assert_eq!(
        said(&refused([6.0, 2.0])),
        (
            "the split position is past arc a0000000's end r0000000",
            Some("at.0"),
            vec!["a0000000", "r0000000"]
        )
    );
    assert_eq!(
        said(&refused([3.5, 2.0 - 2.5 * 3.0_f64.sqrt()])),
        (
            "the split position is past arc a0000000's end r0000001",
            Some("at.0"),
            vec!["a0000000", "r0000001"]
        )
    );
    assert_eq!(
        refused([1.0, -3.0]).message(),
        "the split position is at arc a0000000's end r0000001"
    );
    assert_eq!(
        refused([4.0, 6.0]).message(),
        "the split position is at arc a0000000's end r0000000"
    );
    // The centre: every point of the circle is as near, 5 mm.
    assert_eq!(
        refused([1.0, 2.0]).message(),
        "the split position is 5 mm from arc a0000000, farther than 1e-7 mm"
    );
}

// ---- circles

#[test]
fn a_circle_splits_at_two_positions_into_two_arcs_and_at_one_is_refused() {
    // testSplitCircle: FreeCAD makes one arc whose ends are one place, which the gate
    // refuses (decision 3). About (1, 2), radius 3: at (4, 2) and (1, 5), angles 0 and
    // π/2, c0's ID goes to the arc counter-clockwise from the first to the second, the new
    // one runs on round; the radius stays on c0; a point held on the circle at π goes to
    // the new arc, one at π/4 to c0.
    // cos π/4 = sin π/4 = 1/√2.
    let (s, c) = (
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
    );
    let points = [
        point("z0000000", 1.0, 2.0),
        point("s0000000", -2.0, 2.0),
        point("s0000001", 1.0 + 3.0 * c, 2.0 + 3.0 * s),
    ]
    .join(", ");
    let before = doc(
        &points,
        "",
        &circle("c0000000", "z0000000", 3.0),
        "",
        r#"{"id": "k0000000", "type": "radius", "curve": "c0000000", "value": 3, "driving": true}, {"id": "k0000001", "type": "on", "point": "s0000000", "curve": "c0000000"}, {"id": "k0000002", "type": "on", "point": "s0000001", "curve": "c0000000"}"#,
    );
    let error = split(
        sketch_of(&before),
        &id("c0000000"),
        &[[2.0, 3.1]],
        &ids(&["n0000000", "c1000000"]),
    )
    .unwrap_err();
    assert_eq!(
        said(&error),
        (
            "a circle splits at two positions into two arcs, got 1",
            Some("at"),
            vec!["c0000000"]
        )
    );
    assert_eq!(
        error.cause(),
        Some("an arc whose start and end are one point can't be built")
    );
    assert_eq!(error.suggestion(), Some("give two positions on the circle"));
    let done = split(
        sketch_of(&before),
        &id("c0000000"),
        &[[4.0, 2.0], [1.0, 5.0]],
        &ids(&["n0000000", "n0000001", "c1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert!(sketch.circles.is_empty());
    assert_eq!(at(&sketch, "n0000000"), (4.0, 2.0, false));
    assert_eq!(at(&sketch, "n0000001"), (1.0, 5.0, false));
    assert_eq!(
        arc_points(&sketch, "c0000000"),
        ["z0000000", "n0000000", "n0000001"]
    );
    assert_eq!(
        arc_points(&sketch, "c1000000"),
        ["z0000000", "n0000001", "n0000000"]
    );
    assert_eq!(
        form(&sketch, "k0000000"),
        form(sketch_of(&before), "k0000000")
    );
    let on = |name: &str| match form(&sketch, name) {
        Form::On { curve, .. } => curve.to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(["k0000001", "k0000002"].map(on), ["c1000000", "c0000000"]);
    assert_eq!(done.pieces, [id("c0000000"), id("c1000000")]);
    assert_eq!(
        done.edited.added,
        ids(&["n0000000", "n0000001", "c1000000"])
    );
    assert_eq!(done.edited.modified, ids(&["c0000000", "k0000001"]));
}

#[test]
fn a_circles_two_positions_within_1e_7_mm_or_three_are_refused() {
    let before = doc(
        &point("z0000000", 1.0, 2.0),
        "",
        &circle("c0000000", "z0000000", 3.0),
        "",
        "",
    );
    let three = ids(&["n0000000", "n0000001", "c1000000"]);
    let error = split(
        sketch_of(&before),
        &id("c0000000"),
        &[[4.0, 2.0], [4.0, 2.0]],
        &three,
    )
    .unwrap_err();
    assert_eq!(
        said(&error),
        (
            "the two split positions are 0 mm apart on circle c0000000, within 1e-7 mm",
            Some("at.1"),
            vec!["c0000000"]
        )
    );
    let error = split(
        sketch_of(&before),
        &id("c0000000"),
        &[[4.0, 2.0], [1.0, 5.0], [-2.0, 2.0]],
        &three,
    )
    .unwrap_err();
    assert_eq!(
        (error.message(), error.cause()),
        (
            "a circle splits at two positions into two arcs, got 3",
            None
        )
    );
}

#[test]
fn an_edge_tangent_with_a_curve_ending_at_the_split_point_is_refused_naming_it() {
    // As the split property found (seed 20261021, the mutants run's): arc c about z0
    // (10, 10) starts at s0 (10, 0), held tangent on a there (k1) and edge tangent to a too
    // (k0, c's centre left of a). Split at s0, the pieces share it, so c and the start-side
    // piece, which holds the contact, share an end: there an edge tangency is a tangent_at,
    // as the merge refuses (open item 13 of docs/design/m5.0-app.md).
    let points = r#"{"id": "s0000000", "x": 10, "y": 0, "construction": false}, {"id": "z0000000", "x": 10, "y": 10, "construction": false}, {"id": "z0000001", "x": 20, "y": 10, "construction": false}"#;
    let before = a_line(
        points,
        "",
        "",
        &arc("c0000000", "z0000000", "s0000000", "z0000001"),
        r#"{"id": "k0000000", "type": "tangent", "a": "c0000000", "b": "a0000000", "side": "left"}, {"id": "k0000001", "type": "tangent_on", "a": "c0000000", "at": "s0000000", "b": "a0000000", "direction": "same"}"#,
    );
    let error = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["b1000000", "j0000000"]),
    )
    .unwrap_err();
    assert_eq!(
        said(&error),
        (
            "the split would make constraint k0000000 an edge tangency of c0000000 and a0000000, which would then share an end, s0000000: where curves meet, a tangency is a tangent_at",
            Some("at.0"),
            vec!["k0000000", "c0000000", "a0000000", "s0000000"]
        )
    );
    assert_eq!(
        error.suggestion(),
        Some("remove the constraint first, or split elsewhere")
    );
}

#[test]
fn a_constraint_the_shared_point_would_make_a_pieces_own_is_refused_naming_it() {
    // s0 at (10, 0) held on a, and measured from a by a reference distance of 0: split
    // there, s0 is the pieces' end, and the distance would hold a piece's own point.
    let before = a_line(
        r#"{"id": "s0000000", "x": 10, "y": 0, "construction": false}"#,
        "",
        "",
        "",
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "a0000000"}, {"id": "k0000001", "type": "point_line_distance", "point": "s0000000", "line": "a0000000", "value": 0, "side": "left", "driving": false}"#,
    );
    let error = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["b1000000", "j0000000"]),
    )
    .unwrap_err();
    assert_eq!(
        said(&error),
        (
            "the split would make constraint k0000001 hold s0000000, a point of a0000000, on a0000000 itself",
            Some("at.0"),
            vec!["k0000001", "s0000000", "a0000000"]
        )
    );
    assert_eq!(
        error.suggestion(),
        Some("remove the constraint first, or split elsewhere")
    );
}

// ---- the tests review's: rounds' pieces, contacts, and floors

/// Arc a0000000 about z0 (0, 0), radius 25, counter-clockwise from e0 (25, 0), at 0, to e1
/// (−15, −20), at 233.13°; beside `points`.
fn an_arc(points: &[String], lines: &str, circles: &str, constraints: &str) -> Document {
    let all = [
        point("z0000000", 0.0, 0.0),
        point("e0000000", 25.0, 0.0),
        point("e0000001", -15.0, -20.0),
    ]
    .into_iter()
    .chain(points.iter().cloned())
    .collect::<Vec<_>>()
    .join(", ");
    doc(
        &all,
        lines,
        circles,
        &arc("a0000000", "z0000000", "e0000000", "e0000001"),
        constraints,
    )
}

#[test]
fn points_on_an_arc_go_to_the_piece_holding_them_or_past_its_ends_the_nearer_ends() {
    // Split at (0, 25), 90°: s0 (20, 15) at 36.9° is on a; s1 (−25, 0) at 180° on b; s2
    // (24, −7) at 343.7°, past both, nearer e0 (7.07 against 41.0): a; s3 (0, −25) at 270°,
    // past, nearer e1 (15.8 against 35.4): b.
    let points = [
        point("s0000000", 20.0, 15.0),
        point("s0000001", -25.0, 0.0),
        point("s0000002", 24.0, -7.0),
        point("s0000003", 0.0, -25.0),
    ];
    let constraints = (0..4)
        .map(|i| {
            format!(r#"{{"id": "k000000{i}", "type": "on", "point": "s000000{i}", "curve": "a0000000"}}"#)
        })
        .collect::<Vec<_>>()
        .join(", ");
    let before = an_arc(&points, "", "", &constraints);
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[0.0, 25.0]],
        &ids(&["n0000000", "b1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    let on = |name: &str| match form(&sketch, name) {
        Form::On { curve, .. } => curve.to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        ["k0000000", "k0000001", "k0000002", "k0000003"].map(on),
        ["a0000000", "b1000000", "a0000000", "b1000000"]
    );
}

#[test]
fn an_arcs_tangencies_go_to_the_piece_touching_the_contact() {
    // Split at (0, 25): the line m, x = −25, touches a at (−25, 0), 180°, on b; the circle c
    // about (25, 0) of radius 50 holds a inside it, touching at (−25, 0) too (the side away
    // from c's centre), on b.
    let points = [
        point("q0000000", -25.0, -30.0),
        point("q0000001", -25.0, 30.0),
        point("y0000000", 25.0, 0.0),
    ];
    let before = an_arc(
        &points,
        &line("m0000000", "q0000000", "q0000001"),
        &circle("c0000000", "y0000000", 50.0),
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "m0000000", "side": "right"}, {"id": "k0000001", "type": "tangent", "a": "a0000000", "b": "c0000000", "side": "inside"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[0.0, 25.0]],
        &ids(&["n0000000", "b1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        form(&sketch, "k0000000"),
        &Form::Tangent {
            a: element("b1000000"),
            b: element("m0000000"),
            side: TangentSide::Right,
        }
    );
    assert_eq!(
        form(&sketch, "k0000001"),
        &Form::Tangent {
            a: element("b1000000"),
            b: element("c0000000"),
            side: TangentSide::Inside,
        }
    );
    assert_eq!(
        done.edited.modified,
        ids(&["a0000000", "k0000000", "k0000001"])
    );
}

#[test]
fn an_arcs_equal_and_radius_stay_on_the_start_side_piece() {
    let before = an_arc(
        &[point("y0000000", 100.0, 0.0)],
        "",
        &circle("c0000000", "y0000000", 25.0),
        r#"{"id": "k0000000", "type": "equal", "a": "a0000000", "b": "c0000000"}, {"id": "k0000001", "type": "radius", "curve": "a0000000", "value": 25, "driving": true}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[0.0, 25.0]],
        &ids(&["n0000000", "b1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        form(&sketch, "k0000000"),
        &Form::Equal {
            a: element("a0000000"),
            b: element("c0000000")
        }
    );
    assert_eq!(
        form(&sketch, "k0000001"),
        form(sketch_of(&before), "k0000001")
    );
    assert_eq!(done.edited.modified, ids(&["a0000000"]));
    assert!(done.edited.removed.is_empty());
}

#[test]
fn a_tangent_on_at_a_lines_start_stays_and_at_its_far_end_goes_to_the_new_piece() {
    // a's start p0 is tangent on circle c0 about (0, −10), its end p4 on c1 about (32, 10).
    let before = a_line(
        r#"{"id": "y0000000", "x": 0, "y": -10, "construction": false}, {"id": "y0000001", "x": 32, "y": 10, "construction": false}"#,
        "",
        &[
            circle("c0000000", "y0000000", 10.0),
            circle("c1000000", "y0000001", 10.0),
        ]
        .join(", "),
        "",
        r#"{"id": "k0000000", "type": "tangent_on", "a": "a0000000", "at": "p0000000", "b": "c0000000", "direction": "same"}, {"id": "k0000001", "type": "tangent_on", "a": "a0000000", "at": "p0000004", "b": "c1000000", "direction": "same"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        form(&sketch, "k0000000"),
        form(sketch_of(&before), "k0000000")
    );
    assert_eq!(
        form(&sketch, "k0000001"),
        &Form::TangentOn {
            a: element("b1000000"),
            at: element("p0000004"),
            b: element("c1000000"),
            direction: Direction::Same,
        }
    );
    assert_eq!(done.edited.modified, ids(&["a0000000", "k0000001"]));
}

#[test]
fn a_construction_circles_and_arcs_pieces_and_points_are_construction() {
    let before = doc(
        &point("z0000000", 0.0, 0.0),
        "",
        &circle_as("c0000000", "z0000000", 25.0, true),
        "",
        "",
    );
    let done = split(
        sketch_of(&before),
        &id("c0000000"),
        &[[25.0, 0.0], [0.0, 25.0]],
        &ids(&["n0000000", "n0000001", "c1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(sketch.arcs.len(), 2);
    assert!(sketch.arcs.iter().all(|a| a.construction));
    assert!(at(&sketch, "n0000000").2 && at(&sketch, "n0000001").2);
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("e0000000", 25.0, 0.0),
            point("e0000001", -15.0, -20.0),
        ]
        .join(", "),
        "",
        "",
        r#"{"id": "a0000000", "center": "z0000000", "start": "e0000000", "end": "e0000001", "construction": true}"#,
        "",
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[0.0, 25.0]],
        &ids(&["n0000000", "b1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert!(sketch.arcs.iter().all(|a| a.construction));
    assert!(at(&sketch, "n0000000").2);
}

/// The point on the circle of radius 25 about the origin whose chord from (25, 0) is
/// `chord` long.
fn chord_from_the_start(chord: f64) -> [f64; 2] {
    let angle = 2.0 * libm::asin(chord / 50.0);
    [25.0 * libm::cos(angle), 25.0 * libm::sin(angle)]
}

#[test]
fn a_rounds_split_within_1e_7_mm_of_an_end_or_of_its_other_position_is_refused_and_past_it_splits()
{
    // 5e-8 from the arc's start (chord), refused naming e0; 5e-7, split. A circle's two
    // positions 5e-8 apart, refused at at.1; 5e-7, split.
    let before = an_arc(&[], "", "", "");
    let error = split(
        sketch_of(&before),
        &id("a0000000"),
        &[chord_from_the_start(0.00000005)],
        &ids(&["n0000000", "b1000000"]),
    )
    .unwrap_err();
    let (message, parameter, elements) = said(&error);
    assert!(
        message.starts_with("the split position is ")
            && message.ends_with(" mm from arc a0000000's end e0000000, within 1e-7 mm"),
        "{message}"
    );
    assert_eq!(
        (parameter, elements),
        (Some("at.0"), vec!["a0000000", "e0000000"])
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[chord_from_the_start(0.0000005)],
        &ids(&["n0000000", "b1000000"]),
    )
    .unwrap();
    gated(&before, &done);
    let circle_doc = doc(
        &point("z0000000", 0.0, 0.0),
        "",
        &circle("c0000000", "z0000000", 25.0),
        "",
        "",
    );
    let three = ids(&["n0000000", "n0000001", "c1000000"]);
    let error = split(
        sketch_of(&circle_doc),
        &id("c0000000"),
        &[[25.0, 0.0], chord_from_the_start(0.00000005)],
        &three,
    )
    .unwrap_err();
    assert_eq!(error.parameter(), Some("at.1"));
    assert!(
        error.message().starts_with("the two split positions are "),
        "{}",
        error.message()
    );
    let done = split(
        sketch_of(&circle_doc),
        &id("c0000000"),
        &[[25.0, 0.0], chord_from_the_start(0.0000005)],
        &three,
    )
    .unwrap();
    gated(&circle_doc, &done);
}

#[test]
fn of_two_points_held_at_the_split_the_least_id_is_shared() {
    // t0 at (10, 0) and s0 5e-8 above it, both held on a: s0 is shared, t0 left beside it.
    let before = a_line(
        r#"{"id": "t0000000", "x": 10, "y": 0, "construction": false}, {"id": "s0000000", "x": 10, "y": 0.00000005, "construction": false}"#,
        "",
        "",
        "",
        r#"{"id": "k0000000", "type": "on", "point": "t0000000", "curve": "a0000000"}, {"id": "k0000001", "type": "on", "point": "s0000000", "curve": "a0000000"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("p0000000".to_owned(), "s0000000".to_owned())
    );
    assert_eq!(done.edited.removed, ids(&["k0000001"]));
}

// ---- the mutants': points held elsewhere, rounds off the origin, and the ends' feet

#[test]
fn a_point_held_at_the_split_on_another_curve_isnt_the_split_point() {
    // The vertical line e (x = 10) crosses a at (10, 0): s0 there is held on e (k0), and
    // arc x about (5, 0) starts at t0 there, tangent on e (k1: leaving its start along +y,
    // as e runs). Neither is held on a, so a split of a at (10, 0) makes a new point, and
    // k0 and k1 stay as they were.
    let points = [
        point("s0000000", 10.0, 0.0),
        point("t0000000", 10.0, 0.0),
        point("v0000000", 10.0, -5.0),
        point("v0000001", 10.0, 5.0),
        point("z0000000", 5.0, 0.0),
        point("w0000000", 5.0, 5.0),
    ]
    .join(", ");
    let before = a_line(
        &points,
        &line("e0000000", "v0000000", "v0000001"),
        "",
        &arc("x0000000", "z0000000", "t0000000", "w0000000"),
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "e0000000"}, {"id": "k0000001", "type": "tangent_on", "a": "x0000000", "at": "t0000000", "b": "e0000000", "direction": "same"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[10.0, 0.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(at(&sketch, "n0000000"), (10.0, 0.0, false));
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("p0000000".to_owned(), "n0000000".to_owned())
    );
    assert_eq!(
        ends(&sketch, "b1000000"),
        ("n0000000".to_owned(), "p0000004".to_owned())
    );
    for name in ["k0000000", "k0000001"] {
        assert_eq!(
            form(&sketch, name),
            form(sketch_of(&before), name),
            "{name}"
        );
    }
    assert_eq!(
        done.edited.added,
        ids(&["n0000000", "b1000000", "j0000000"])
    );
    assert_eq!(done.edited.modified, ids(&["a0000000"]));
    assert!(done.edited.removed.is_empty());
}

#[test]
fn a_circle_split_at_two_points_held_on_it_shares_them_both() {
    // s0 (10, 0) and s1 (−10, 0) held on c, split at their places: each is a split point,
    // so the split makes no point, only the second arc, and both ons go as implied.
    let points = [
        point("z0000000", 0.0, 0.0),
        point("s0000000", 10.0, 0.0),
        point("s0000001", -10.0, 0.0),
    ]
    .join(", ");
    let before = doc(
        &points,
        "",
        &circle("c0000000", "z0000000", 10.0),
        "",
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "c0000000"}, {"id": "k0000001", "type": "on", "point": "s0000001", "curve": "c0000000"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("c0000000"),
        &[[10.0, 0.0], [-10.0, 0.0]],
        &ids(&["c1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        arc_points(&sketch, "c0000000"),
        ["z0000000", "s0000000", "s0000001"]
    );
    assert_eq!(
        arc_points(&sketch, "c1000000"),
        ["z0000000", "s0000001", "s0000000"]
    );
    assert_eq!(sketch.points.len(), 3);
    assert!(sketch.constraints.is_empty());
    assert_eq!(done.edited.added, ids(&["c1000000"]));
    assert_eq!(done.edited.modified, ids(&["c0000000"]));
    assert_eq!(done.edited.removed, ids(&["k0000000", "k0000001"]));
}

#[test]
fn a_point_held_at_a_lines_ends_foot_farther_than_1e_7_mm_from_it_isnt_past_it() {
    // q = 2^-33, an ulp of the coordinates here. Line a from p0 (700000, 600000) to p4
    // (700005.25, 600107.25), along (42, 858)/8; s0 at p0 + q(−858, 42), held on a, is
    // square to it from p0 ((−858, 42)·(42, 858) = 0, every product exact), so its
    // parameter along a is exactly 0, and it's q√737928 = 1.0000395e-7 mm from p0. The
    // position p0 + (0, q) projects onto itself (exactly, p0 + q(0.0488, 0.9977), rounded
    // to the ulp), q√(858² + 41²) = q√737845 = 9.99983e-8 mm from s0: s0 is the split point.
    // At p0's foot, s0 isn't past p0, and it's farther than 1e-7 from it: the split takes
    // it, the start-side piece 1.0000395e-7 mm long. So with a drawn the other way, s0 at
    // parameter exactly 1, p0 its far end.
    let q = 1.0 / 8_589_934_592.0;
    let s0 = [700_000.0 - 858.0 * q, 600_000.0 + 42.0 * q];
    assert_eq!(s0, [699_999.999_999_900_1, 600_000.000_000_004_9]);
    let points = [
        point("p0000000", 700_000.0, 600_000.0),
        point("p0000004", 700_005.25, 600_107.25),
        point("s0000000", s0[0], s0[1]),
    ]
    .join(", ");
    for (from, to) in [("p0000000", "p0000004"), ("p0000004", "p0000000")] {
        let before = doc(
            &points,
            &line("a0000000", from, to),
            "",
            "",
            r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "a0000000"}"#,
        );
        let done = split(
            sketch_of(&before),
            &id("a0000000"),
            &[[700_000.0, 600_000.0 + q]],
            &ids(&["b1000000", "j0000000"]),
        )
        .unwrap_or_else(|e| panic!("{from} to {to}: {e:?}"));
        let sketch = gated(&before, &done);
        assert_eq!(
            [ends(&sketch, "a0000000"), ends(&sketch, "b1000000")],
            [
                (from.to_owned(), "s0000000".to_owned()),
                ("s0000000".to_owned(), to.to_owned())
            ]
        );
        assert_eq!(done.edited.added, ids(&["b1000000", "j0000000"]));
        assert_eq!(done.edited.removed, ids(&["k0000000"]));
    }
}

#[test]
fn points_on_an_arc_about_a_centre_off_the_origin_go_to_the_piece_holding_them() {
    // Arc a about z0 (40, 30), radius 25, counter-clockwise from e0 (55, 50), at 53.13°, to
    // e1 (64, 23), at 343.74°; split at (25, 10), 233.13°. s0 (33, 6), at 253.74°, is on b;
    // s1 (15, 30), at 180°, on a.
    let points = [
        point("z0000000", 40.0, 30.0),
        point("e0000000", 55.0, 50.0),
        point("e0000001", 64.0, 23.0),
        point("s0000000", 33.0, 6.0),
        point("s0000001", 15.0, 30.0),
    ]
    .join(", ");
    let before = doc(
        &points,
        "",
        "",
        &arc("a0000000", "z0000000", "e0000000", "e0000001"),
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "a0000000"}, {"id": "k0000001", "type": "on", "point": "s0000001", "curve": "a0000000"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[25.0, 10.0]],
        &ids(&["n0000000", "b1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    let on = |name: &str| match form(&sketch, name) {
        Form::On { curve, .. } => curve.to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(["k0000000", "k0000001"].map(on), ["b1000000", "a0000000"]);
    assert_eq!(done.edited.modified, ids(&["a0000000", "k0000000"]));
}

#[test]
fn points_on_a_circle_off_the_origin_and_a_lines_tangency_go_to_the_piece_holding_them() {
    // Circle c about z0 (40, 30), radius 5, split at (44, 33), 36.87°, and (36, 33),
    // 143.13°: c keeps the arc between them through 90°, the new one the rest. Held on c:
    // s1 (40, 35) at 90°, s6 (43, 34) at 53.13°, and s7 (37, 34) at 126.87° on c; s2 (45, 30)
    // at 0°, s3 (35, 30) at 180°, s4 (37, 26) at 233.13°, and s5 (44, 27) at 323.13° on the
    // new one. The line m, y = 35, touches c at its centre's foot, (40, 35), 90°: on c.
    let points = [
        point("z0000000", 40.0, 30.0),
        point("s0000001", 40.0, 35.0),
        point("s0000002", 45.0, 30.0),
        point("s0000003", 35.0, 30.0),
        point("s0000004", 37.0, 26.0),
        point("s0000005", 44.0, 27.0),
        point("s0000006", 43.0, 34.0),
        point("s0000007", 37.0, 34.0),
        point("y0000000", 30.0, 35.0),
        point("y0000001", 50.0, 35.0),
    ]
    .join(", ");
    let constraints = (1..8)
        .map(|i| {
            format!(r#"{{"id": "k000000{i}", "type": "on", "point": "s000000{i}", "curve": "c0000000"}}"#)
        })
        .chain(std::iter::once(
            r#"{"id": "k0000000", "type": "tangent", "a": "c0000000", "b": "m0000000", "side": "right"}"#.to_owned(),
        ))
        .collect::<Vec<_>>()
        .join(", ");
    let before = doc(
        &points,
        &line("m0000000", "y0000000", "y0000001"),
        &circle("c0000000", "z0000000", 5.0),
        "",
        &constraints,
    );
    let done = split(
        sketch_of(&before),
        &id("c0000000"),
        &[[44.0, 33.0], [36.0, 33.0]],
        &ids(&["n0000000", "n0000001", "c1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    let on = |i: usize| match form(&sketch, &format!("k000000{i}")) {
        Form::On { curve, .. } => curve.to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        [1, 2, 3, 4, 5, 6, 7].map(on),
        [
            "c0000000", "c1000000", "c1000000", "c1000000", "c1000000", "c0000000", "c0000000"
        ]
    );
    assert_eq!(
        form(&sketch, "k0000000"),
        form(sketch_of(&before), "k0000000")
    );
    assert_eq!(
        done.edited.modified,
        ids(&["c0000000", "k0000002", "k0000003", "k0000004", "k0000005"])
    );
}

#[test]
fn a_circles_tangencies_with_circles_go_to_the_piece_touching_them() {
    // Circle c about z0 (40, 30), radius 25, split at (47, 54), 73.74°, then (60, 45),
    // 36.87°: the new arc runs between them, through 53.13°, where c touches each circle
    // here, at (55, 50) = z0 + (15, 20), so each tangency goes to it. With s = 2^-30: g1
    // about (43, 34), 5 from z0 along (3, 4), radius 20, inside c; g2 about z0 + s(3, 4),
    // radius 25 − 5s, inside c too; g3 about z0 − s(3, 4), radius 25 + 5s, holding c inside
    // it, so the contact is on c's side away from g3's centre; and g4 about (46, 38), radius
    // 25, as large as c, drawn 10 from it: neither is inside the other, so the contact is
    // toward g4's centre. Each tangency is inside. (g2's and g3's centres are so near z0
    // that a contact scaled by s², not by 1/s, would round onto z0, at 0°, on c.)
    let s = 1.0 / 1_073_741_824.0;
    let points = [
        point("z0000000", 40.0, 30.0),
        point("y0000001", 43.0, 34.0),
        point("y0000002", 40.0 + 3.0 * s, 30.0 + 4.0 * s),
        point("y0000003", 40.0 - 3.0 * s, 30.0 - 4.0 * s),
        point("y0000004", 46.0, 38.0),
    ]
    .join(", ");
    let circles = [
        circle("c0000000", "z0000000", 25.0),
        circle("g0000001", "y0000001", 20.0),
        circle("g0000002", "y0000002", 25.0 - 5.0 * s),
        circle("g0000003", "y0000003", 25.0 + 5.0 * s),
        circle("g0000004", "y0000004", 25.0),
    ]
    .join(", ");
    let constraints = (1..5)
        .map(|i| {
            format!(r#"{{"id": "k000000{i}", "type": "tangent", "a": "c0000000", "b": "g000000{i}", "side": "inside"}}"#)
        })
        .collect::<Vec<_>>()
        .join(", ");
    let before = doc(&points, "", &circles, "", &constraints);
    let done = split(
        sketch_of(&before),
        &id("c0000000"),
        &[[47.0, 54.0], [60.0, 45.0]],
        &ids(&["n0000000", "n0000001", "c1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    for i in 1..5 {
        assert_eq!(
            form(&sketch, &format!("k000000{i}")),
            &Form::Tangent {
                a: element("c1000000"),
                b: element(&format!("g000000{i}")),
                side: TangentSide::Inside,
            },
            "k000000{i}"
        );
    }
    assert_eq!(
        done.edited.modified,
        ids(&["c0000000", "k0000001", "k0000002", "k0000003", "k0000004"])
    );
}

#[test]
fn an_arcs_tangent_at_at_its_far_end_goes_to_the_new_piece_named_second_too() {
    // testSplitArcOfCircle's arc at (−4, 2) again, its tangent_ats naming it second: the one
    // at its start r0 stays, the one at its end r1 goes with that end to the new piece.
    let before = the_arc(
        r#"{"id": "k0000001", "type": "tangent_at", "a": "e0000000", "b": "a0000000", "at": "r0000000", "direction": "same"}, {"id": "k0000002", "type": "tangent_at", "a": "f0000000", "b": "a0000000", "at": "r0000001", "direction": "same"}"#,
    );
    let done = split(
        sketch_of(&before),
        &id("a0000000"),
        &[[-4.0, 2.0]],
        &ids(&["n0000000", "b1000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        form(&sketch, "k0000001"),
        form(sketch_of(&before), "k0000001")
    );
    assert_eq!(
        form(&sketch, "k0000002"),
        &Form::TangentAt {
            a: element("f0000000"),
            b: element("b1000000"),
            at: element("r0000001"),
            direction: Direction::Same,
        }
    );
    assert_eq!(done.edited.modified, ids(&["a0000000", "k0000002"]));
}
