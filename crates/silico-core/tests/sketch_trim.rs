//! Trimming a curve (docs/ports/sketch-edit.md, §4 and §9's tests; decisions 2, 5, 6, 8):
//! the M5.0 design's worked example word for word, FreeCAD's trim tests, each ported as
//! drawn and, where decision 5 departs from FreeCAD's 5e-5 mm join, with the cutter's end
//! held, and Silico's own. Each result goes through the validation gate; each expected
//! sketch and list is worked by hand.

use silico_core::Error;
use silico_core::document::sketch_edit::{Trim, trim};
use silico_core::document::{
    Direction, Document, ElementId, Form, Operation, Reference, Sketch, read, validate,
};

use crate::fixture::{arc, circle, circle_as, document, fields, line, point};

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

/// The trim's sketch in place of the document's, checked by the gate.
#[track_caller]
fn gated(document: &Document, trim: &Trim) -> Sketch {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(trim.edited.sketch.clone());
    if let Err(invalid) = validate(&document) {
        panic!("the gate refused the trim: {:?}", invalid.errors());
    }
    sketch_of(&document).clone()
}

#[track_caller]
fn form<'a>(sketch: &'a Sketch, name: &str) -> &'a Form {
    &sketch
        .constraints
        .iter()
        .find(|c| c.id == id(name))
        .unwrap_or_else(|| panic!("no constraint {name}"))
        .form
}

fn has(sketch: &Sketch, name: &str) -> bool {
    sketch.elements().any(|(e, _)| *e == id(name))
}

#[track_caller]
fn ends(sketch: &Sketch, name: &str) -> (String, String) {
    let line = sketch.lines.iter().find(|l| l.id == id(name)).unwrap();
    (line.from.to_string(), line.to.to_string())
}

#[track_caller]
fn arc_points(sketch: &Sketch, name: &str) -> [String; 3] {
    let arc = sketch.arcs.iter().find(|a| a.id == id(name)).unwrap();
    [
        arc.center.to_string(),
        arc.start.to_string(),
        arc.end.to_string(),
    ]
}

#[track_caller]
fn at(sketch: &Sketch, name: &str) -> (f64, f64, bool) {
    let p = sketch.points.iter().find(|p| p.id == id(name)).unwrap();
    (p.x.mm(), p.y.mm(), p.construction)
}

#[track_caller]
fn near(sketch: &Sketch, name: &str, expected: [f64; 2]) {
    let (x, y, _) = at(sketch, name);
    assert!(
        (x - expected[0]).abs() < 1e-12 && (y - expected[1]).abs() < 1e-12,
        "{name} at ({x}, {y}), not {expected:?}"
    );
}

fn said(error: &Error) -> (&str, Option<&str>, Vec<&str>) {
    (
        error.message(),
        error.parameter(),
        error.elements().iter().map(String::as_str).collect(),
    )
}

fn on(point: &str, curve: &str) -> Form {
    Form::On {
        point: element(point),
        curve: element(curve),
    }
}

// ---- the design's worked example

/// The M5.0 design's `sketch_trim` example (the port note's §4 worked example, 1.1.3), as
/// sketch members; `more` adds constraints.
fn worked_example(more: &str) -> Document {
    let points = [
        point("pd000001", 0.0, 0.0),
        point("pd000002", 10.0, 0.0),
        point("pd000003", 3.0, -2.0),
        point("pd000004", 3.0, 2.0),
        point("pd000005", 7.0, 0.0),
        point("pd000006", 7.0, 3.0),
        point("pd000007", 5.0, 0.0),
        point("pd000008", 9.0, 0.0),
        point("pd00000a", 0.0, 5.0),
        point("pd00000b", 0.0, 10.0),
        point("pd00000c", 10.0, 10.0),
        point("pd00000d", 0.0, -10.0),
    ]
    .join(", ");
    let lines = [
        line("nd000001", "pd000001", "pd000002"),
        line("nd000002", "pd000003", "pd000004"),
        line("nd000003", "pd000005", "pd000006"),
        line("nd000004", "pd00000b", "pd00000c"),
        line("nd000005", "pd000001", "pd00000a"),
    ]
    .join(", ");
    let constraints = format!(
        r#"{{"id": "kd000001", "type": "horizontal", "line": "nd000001"}}, {{"id": "kd000002", "type": "length", "line": "nd000001", "value": 10, "driving": true}}, {{"id": "kd000003", "type": "equal", "a": "nd000001", "b": "nd000004"}}, {{"id": "kd000004", "type": "on", "point": "pd000005", "curve": "nd000001"}}, {{"id": "kd000005", "type": "on", "point": "pd000007", "curve": "nd000001"}}, {{"id": "kd000006", "type": "on", "point": "pd000008", "curve": "nd000001"}}{more}"#
    );
    doc(&points, &lines, "", "", &constraints)
}

#[test]
fn the_designs_worked_example_word_for_word() {
    let before = worked_example("");
    let done = trim(
        sketch_of(&before),
        &id("nd000001"),
        [5.0, 0.0],
        &ids(&["pd000009", "nd000006", "kd00000a", "kd00000b"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    // nd000001 ends at the new point (3, 0), on the cutter nd000002 (kd00000b).
    assert_eq!(
        ends(&sketch, "nd000001"),
        ("pd000001".to_owned(), "pd000009".to_owned())
    );
    assert_eq!(at(&sketch, "pd000009"), (3.0, 0.0, false));
    assert_eq!(form(&sketch, "kd00000b"), &on("pd000009", "nd000002"));
    // nd000006 runs from the cutter's start, shared since kd000004 held it on the line.
    assert_eq!(
        ends(&sketch, "nd000006"),
        ("pd000005".to_owned(), "pd000002".to_owned())
    );
    assert_eq!(
        form(&sketch, "kd00000a"),
        &Form::Horizontal {
            line: element("nd000006")
        }
    );
    assert_eq!(
        form(&sketch, "kd000001"),
        &Form::Horizontal {
            line: element("nd000001")
        }
    );
    match form(&sketch, "kd000002") {
        Form::Distance {
            a,
            b,
            value,
            driving,
        } => {
            assert_eq!((a, b), (&element("pd000001"), &element("pd000002")));
            assert_eq!((value.to_string(), *driving), ("10".to_owned(), true));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(form(&sketch, "kd000006"), &on("pd000008", "nd000006"));
    for gone in ["kd000003", "kd000004", "kd000005"] {
        assert!(!has(&sketch, gone), "{gone}");
    }
    // The neighbour keeps pd000001; the lone point pd000007 stays, now free.
    assert_eq!(
        ends(&sketch, "nd000005"),
        ("pd000001".to_owned(), "pd00000a".to_owned())
    );
    assert!(has(&sketch, "pd000007"));
    assert_eq!(done.pieces, ids(&["nd000001", "nd000006"]));
    assert_eq!(
        done.edited.added,
        ids(&["pd000009", "nd000006", "kd00000a", "kd00000b"])
    );
    assert_eq!(
        done.edited.modified,
        ids(&["nd000001", "kd000002", "kd000006"])
    );
    assert_eq!(
        done.edited.removed,
        ids(&["kd000003", "kd000004", "kd000005"])
    );
}

#[test]
fn the_designs_symmetry_about_the_trimmed_line_is_refused_word_for_word() {
    let before = worked_example(
        r#", {"id": "kd000007", "type": "symmetric", "a": "pd00000b", "b": "pd00000d", "about": "nd000001"}"#,
    );
    let error = trim(
        sketch_of(&before),
        &id("nd000001"),
        [5.0, 0.0],
        &ids(&["pd000009", "nd000006", "kd00000a", "kd00000b"]),
    )
    .unwrap_err();
    assert_eq!(
        said(&error),
        (
            "trimming line nd000001 there would leave constraint kd000007, a symmetry about it, on no piece of it",
            Some("pick"),
            vec!["kd000007", "nd000001"]
        )
    );
    assert_eq!(error.elements_total(), 2);
    assert_eq!(
        error.cause(),
        Some("the trim's pieces are separate lines, and the symmetry is about the whole one")
    );
    assert_eq!(
        error.suggestion(),
        Some("remove the constraint first, or make the points symmetric about another line")
    );
}

// ---- FreeCAD's: lines (SketcherTestHelpers' line, (1, 2) to (3, 4))

/// The line from (1, 2) to (3, 4), `a0000000`, beside `points`, `lines`, `constraints`.
fn freecads_line(points: &[String], lines: &[String], constraints: &str) -> Document {
    let points = [point("p0000001", 1.0, 2.0), point("p0000002", 3.0, 4.0)]
        .into_iter()
        .chain(points.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    let lines = std::iter::once(line("a0000000", "p0000001", "p0000002"))
        .chain(lines.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    doc(&points, &lines, "", "", constraints)
}

/// The point at `t` of the way along (1, 2) to (3, 4).
fn along_line(t: f64) -> [f64; 2] {
    [1.0 + 2.0 * t, 2.0 + 2.0 * t]
}

#[test]
fn freecads_trim_without_intersection_removes_the_line() {
    // testTrimWithoutIntersection: no cut, the line goes, with its points.
    let before = freecads_line(&[], &[], "");
    let error = trim(
        sketch_of(&before),
        &id("a0000000"),
        [2.0, 3.1],
        &ids(&["n0000000"]),
    )
    .unwrap_err();
    assert_eq!(
        said(&error),
        ("the trim needs 0 new IDs, got 1", Some("ids"), vec![])
    );
    let done = trim(sketch_of(&before), &id("a0000000"), [2.0, 3.1], &[]).unwrap();
    let sketch = gated(&before, &done);
    assert!(sketch.lines.is_empty() && sketch.points.is_empty());
    assert!(done.pieces.is_empty());
    assert_eq!(
        done.edited.removed,
        ids(&["p0000001", "p0000002", "a0000000"])
    );
}

/// testTrimLineSegmentEnd's cutter: from the line's middle, (2, 3), 0.1 on along it, to
/// (2.1, 3.1), overlapping it there.
fn cutter_at_the_middle() -> (Vec<String>, Vec<String>) {
    let (start, end) = (along_line(0.5), [2.1, 3.1]);
    (
        vec![
            point("c0000001", start[0], start[1]),
            point("c0000002", end[0], end[1]),
        ],
        vec![line("e0000000", "c0000001", "c0000002")],
    )
}

#[test]
fn freecads_trim_of_a_line_end_shortens_it_to_the_cutter_as_drawn_or_held() {
    // testTrimLineSegmentEnd, picked at 0.2: the cut is the cutter's start, (2, 3), and
    // FreeCAD joins the new end to it (1 Coincident, through its 5e-5 mm test). As drawn,
    // Silico gives the line a new start there, on the cutter (decision 5); held on the
    // line by an on, the cutter's start is the line's, and the on goes.
    let (points, lines) = cutter_at_the_middle();
    let pick = along_line(0.2);
    let drawn = freecads_line(&points, &lines, "");
    let done = trim(
        sketch_of(&drawn),
        &id("a0000000"),
        pick,
        &ids(&["n0000000", "k0000009"]),
    )
    .unwrap();
    let sketch = gated(&drawn, &done);
    assert_eq!(at(&sketch, "n0000000"), (2.0, 3.0, false));
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("n0000000".to_owned(), "p0000002".to_owned())
    );
    assert_eq!(form(&sketch, "k0000009"), &on("n0000000", "e0000000"));
    assert!(!has(&sketch, "p0000001"));
    assert_eq!(done.pieces, ids(&["a0000000"]));
    assert_eq!(done.edited.added, ids(&["n0000000", "k0000009"]));
    assert_eq!(done.edited.modified, ids(&["a0000000"]));
    assert_eq!(done.edited.removed, ids(&["p0000001"]));
    let held = freecads_line(
        &points,
        &lines,
        r#"{"id": "k0000000", "type": "on", "point": "c0000001", "curve": "a0000000"}"#,
    );
    let done = trim(sketch_of(&held), &id("a0000000"), pick, &[]).unwrap();
    let sketch = gated(&held, &done);
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("c0000001".to_owned(), "p0000002".to_owned())
    );
    assert!(sketch.constraints.is_empty());
    assert_eq!(done.edited.removed, ids(&["p0000001", "k0000000"]));
}

#[test]
fn freecads_trim_of_a_lines_middle_leaves_two_pieces_as_drawn_or_held() {
    // testTrimLineSegmentMid, picked at 0.5: cutter 1 overlaps the line from 0.3 to 0.35,
    // so the cut before the pick is its end, at 0.35; cutter 2 crosses it at 0.7. FreeCAD:
    // 2 curves, 1 Coincident (cutter 1's end, within 5e-5 mm), 1 PointOnObject. As drawn,
    // Silico: two new points, each on its cutter; held, cutter 1's end is shared.
    let (c1, c2) = (
        along_line(0.3),
        [along_line(0.3)[0] + 0.1, along_line(0.3)[1] + 0.1],
    );
    let mid = along_line(0.7);
    let points = vec![
        point("c0000001", c1[0], c1[1]),
        point("c0000002", c2[0], c2[1]),
        point("c0000003", mid[0] - 0.1, mid[1] + 0.1),
        point("c0000004", mid[0] + 0.1, mid[1] - 0.1),
    ];
    let lines = vec![
        line("e0000000", "c0000001", "c0000002"),
        line("f0000000", "c0000003", "c0000004"),
    ];
    let drawn = freecads_line(&points, &lines, "");
    let done = trim(
        sketch_of(&drawn),
        &id("a0000000"),
        along_line(0.5),
        &ids(&["n0000000", "n0000001", "b1000000", "k0000008", "k0000009"]),
    )
    .unwrap();
    let sketch = gated(&drawn, &done);
    near(&sketch, "n0000000", c2);
    near(&sketch, "n0000001", mid);
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("p0000001".to_owned(), "n0000000".to_owned())
    );
    assert_eq!(
        ends(&sketch, "b1000000"),
        ("n0000001".to_owned(), "p0000002".to_owned())
    );
    assert_eq!(form(&sketch, "k0000008"), &on("n0000000", "e0000000"));
    assert_eq!(form(&sketch, "k0000009"), &on("n0000001", "f0000000"));
    assert_eq!(done.pieces, ids(&["a0000000", "b1000000"]));
    let held = freecads_line(
        &points,
        &lines,
        r#"{"id": "k0000000", "type": "on", "point": "c0000002", "curve": "a0000000"}"#,
    );
    let done = trim(
        sketch_of(&held),
        &id("a0000000"),
        along_line(0.5),
        &ids(&["n0000001", "b1000000", "k0000009"]),
    )
    .unwrap();
    let sketch = gated(&held, &done);
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("p0000001".to_owned(), "c0000002".to_owned())
    );
    assert_eq!(
        done.edited.added,
        ids(&["n0000001", "b1000000", "k0000009"])
    );
    assert_eq!(done.edited.removed, ids(&["k0000000"]));
}

#[test]
fn freecads_full_length_and_end_symmetry_constraints_go_with_the_end_trimmed_away() {
    // testTrimEndEffectOnFullLengthConstraints and testTrimEndEffectOnSymmetricConstraints:
    // trimmed at 0.2 back to the cutter at the middle, the line's start goes, and its
    // length (one piece left, `2779f0b29f`) and the symmetry of its ends about the cutter's
    // start (`f0d30cb570`) with it.
    let (points, lines) = cutter_at_the_middle();
    let before = freecads_line(
        &points,
        &lines,
        r#"{"id": "k0000000", "type": "length", "line": "a0000000", "value": 2.828427, "driving": false}, {"id": "k0000001", "type": "symmetric", "a": "p0000001", "b": "p0000002", "about": "c0000001"}"#,
    );
    let done = trim(
        sketch_of(&before),
        &id("a0000000"),
        along_line(0.2),
        &ids(&["n0000000", "k0000009"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert!(!has(&sketch, "k0000000") && !has(&sketch, "k0000001"));
    assert_eq!(
        done.edited.removed,
        ids(&["p0000001", "k0000000", "k0000001"])
    );
}

#[test]
fn freecads_unrelated_tangent_stays_edge_to_edge() {
    // testTrimEndEffectOnUnrelatedTangent (AstoCAD #24, 08a66afbc8): x = 1 from y = −2 to 2,
    // tangent to a circle of radius 1 about the origin, cut by one of radius 1.5 at
    // y = ±√1.25, picked at 0.2 (y = −1.2). The cut after the pick is the outer circle's;
    // the inner's contact at (1, 0) is a cut too, but not one the trim uses, so its tangency
    // stays an edge tangency.
    let before = doc(
        &[
            point("p0000001", 1.0, -2.0),
            point("p0000002", 1.0, 2.0),
            point("z0000000", 0.0, 0.0),
        ]
        .join(", "),
        &line("a0000000", "p0000001", "p0000002"),
        &[
            circle("c0000001", "z0000000", 1.0),
            circle("c0000002", "z0000000", 1.5),
        ]
        .join(", "),
        "",
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "c0000001", "side": "left"}"#,
    );
    let done = trim(
        sketch_of(&before),
        &id("a0000000"),
        [1.0, -1.2],
        &ids(&["n0000000", "k0000009"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    near(&sketch, "n0000000", [1.0, -(1.25_f64.sqrt())]);
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("n0000000".to_owned(), "p0000002".to_owned())
    );
    assert_eq!(
        form(&sketch, "k0000000"),
        form(sketch_of(&before), "k0000000")
    );
    assert_eq!(form(&sketch, "k0000009"), &on("n0000000", "c0000002"));
}

// ---- FreeCAD's: circles and arcs (about (1, 2), radius 3)

/// The point at `angle` on the circle of radius 3 about (1, 2).
fn on_the_circle(angle: f64) -> [f64; 2] {
    [1.0 + 3.0 * libm::cos(angle), 2.0 + 3.0 * libm::sin(angle)]
}

/// testTrimCircleMid's cutters: one starting on the circle at `first` and running 0.1 by
/// 0.1 outward, one crossing it at `second` (from 0.1 by 0.1 before it to as far after).
fn two_cutters(first: f64, second: f64) -> (Vec<String>, Vec<String>) {
    let (p1, p3) = (on_the_circle(first), on_the_circle(second));
    (
        vec![
            point("c0000001", p1[0], p1[1]),
            point("c0000002", p1[0] + 0.1, p1[1] + 0.1),
            point("c0000003", p3[0] - 0.1, p3[1] - 0.1),
            point("c0000004", p3[0] + 0.1, p3[1] + 0.1),
        ],
        vec![
            line("e0000000", "c0000001", "c0000002"),
            line("f0000000", "c0000003", "c0000004"),
        ],
    )
}

#[test]
fn freecads_circle_trimmed_with_one_cut_is_removed() {
    // testTrimCircleEnd: one cutter, starting on the circle at π; picked at 0.2 of a turn.
    let start = on_the_circle(std::f64::consts::PI);
    let before = doc(
        &[
            point("z0000000", 1.0, 2.0),
            point("c0000001", start[0], start[1]),
            point("c0000002", start[0] + 0.1, start[1] + 0.1),
        ]
        .join(", "),
        &line("e0000000", "c0000001", "c0000002"),
        &circle("c0000000", "z0000000", 3.0),
        "",
        "",
    );
    let done = trim(
        sketch_of(&before),
        &id("c0000000"),
        on_the_circle(0.2 * std::f64::consts::TAU),
        &[],
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert!(sketch.circles.is_empty() && sketch.arcs.is_empty());
    assert_eq!(done.edited.removed, ids(&["z0000000", "c0000000"]));
}

#[test]
fn freecads_circle_trimmed_between_two_cuts_is_the_arc_from_the_one_after_to_the_one_before() {
    // testTrimCircleMid, and testTrimEffectOnConstruction on a construction circle: cuts at
    // 0.3 and 0.7 of a turn, picked at 0.5. FreeCAD: one arc, 1 Coincident (cutter 1's
    // start), 1 PointOnObject. As drawn, Silico: the arc counter-clockwise from 0.7 to 0.3,
    // keeping c0's ID, two new points each on its cutter, construction kept; held, the
    // first cutter's start is the arc's end.
    let tau = std::f64::consts::TAU;
    let (points, lines) = two_cutters(0.3 * tau, 0.7 * tau);
    let all = |more: &[String]| {
        std::iter::once(point("z0000000", 1.0, 2.0))
            .chain(points.iter().cloned())
            .chain(more.iter().cloned())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let drawn = doc(
        &all(&[]),
        &lines.join(", "),
        &circle_as("c0000000", "z0000000", 3.0, true),
        "",
        "",
    );
    let done = trim(
        sketch_of(&drawn),
        &id("c0000000"),
        on_the_circle(0.5 * tau),
        &ids(&["n0000000", "n0000001", "k0000008", "k0000009"]),
    )
    .unwrap();
    let sketch = gated(&drawn, &done);
    assert!(sketch.circles.is_empty());
    assert_eq!(
        arc_points(&sketch, "c0000000"),
        ["z0000000", "n0000000", "n0000001"]
    );
    assert!(sketch.arcs[0].construction);
    near(&sketch, "n0000000", on_the_circle(0.7 * tau));
    near(&sketch, "n0000001", on_the_circle(0.3 * tau));
    assert!(at(&sketch, "n0000000").2 && at(&sketch, "n0000001").2);
    assert_eq!(form(&sketch, "k0000008"), &on("n0000000", "f0000000"));
    assert_eq!(form(&sketch, "k0000009"), &on("n0000001", "e0000000"));
    assert_eq!(done.pieces, ids(&["c0000000"]));
    let held = doc(
        &all(&[]),
        &lines.join(", "),
        &circle("c0000000", "z0000000", 3.0),
        "",
        r#"{"id": "k0000000", "type": "on", "point": "c0000001", "curve": "c0000000"}"#,
    );
    let done = trim(
        sketch_of(&held),
        &id("c0000000"),
        on_the_circle(0.5 * tau),
        &ids(&["n0000000", "k0000008"]),
    )
    .unwrap();
    let sketch = gated(&held, &done);
    assert_eq!(
        arc_points(&sketch, "c0000000"),
        ["z0000000", "n0000000", "c0000001"]
    );
    assert_eq!(done.edited.removed, ids(&["k0000000"]));
}

/// testTrimArcOfCircle's arc: about (1, 2), radius 3, from π/3 to 3π/2.
fn the_arc(points: &[String], lines: &[String], constraints: &str) -> Document {
    let (start, end) = (
        on_the_circle(std::f64::consts::FRAC_PI_3),
        on_the_circle(1.5 * std::f64::consts::PI),
    );
    let all = [
        point("z0000000", 1.0, 2.0),
        point("r0000000", start[0], start[1]),
        point("r0000001", end[0], end[1]),
    ]
    .into_iter()
    .chain(points.iter().cloned())
    .collect::<Vec<_>>()
    .join(", ");
    doc(
        &all,
        &lines.join(", "),
        "",
        &arc("a0000000", "z0000000", "r0000000", "r0000001"),
        constraints,
    )
}

/// The angle `t` of the way along testTrimArcOfCircle's arc.
fn along_arc(t: f64) -> f64 {
    std::f64::consts::FRAC_PI_3 + t * (1.5 * std::f64::consts::PI - std::f64::consts::FRAC_PI_3)
}

#[test]
fn freecads_arc_trimmed_at_an_end_and_in_its_middle() {
    // testTrimArcOfCircleEnd: a cutter starting on the arc at 0.5, picked at 0.2: the arc
    // starts there now (FreeCAD: 1 Coincident; Silico, as drawn: a new point on the cutter).
    // testTrimArcOfCircleMid: cuts at 0.3 and 0.7, picked at 0.5: two arcs sharing the
    // centre (FreeCAD's second Coincident), each end new, on its cutter.
    let half = on_the_circle(along_arc(0.5));
    let one = the_arc(
        &[
            point("c0000001", half[0], half[1]),
            point("c0000002", half[0] + 0.1, half[1] + 0.1),
        ],
        &[line("e0000000", "c0000001", "c0000002")],
        "",
    );
    let done = trim(
        sketch_of(&one),
        &id("a0000000"),
        on_the_circle(along_arc(0.2)),
        &ids(&["n0000000", "k0000009"]),
    )
    .unwrap();
    let sketch = gated(&one, &done);
    assert_eq!(
        arc_points(&sketch, "a0000000"),
        ["z0000000", "n0000000", "r0000001"]
    );
    near(&sketch, "n0000000", half);
    assert!(!has(&sketch, "r0000000"));
    let (points, lines) = two_cutters(along_arc(0.3), along_arc(0.7));
    let mid = the_arc(&points, &lines, "");
    let done = trim(
        sketch_of(&mid),
        &id("a0000000"),
        on_the_circle(along_arc(0.5)),
        &ids(&["n0000000", "n0000001", "b1000000", "k0000008", "k0000009"]),
    )
    .unwrap();
    let sketch = gated(&mid, &done);
    assert_eq!(
        arc_points(&sketch, "a0000000"),
        ["z0000000", "r0000000", "n0000000"]
    );
    assert_eq!(
        arc_points(&sketch, "b1000000"),
        ["z0000000", "n0000001", "r0000001"]
    );
    near(&sketch, "n0000000", on_the_circle(along_arc(0.3)));
    near(&sketch, "n0000001", on_the_circle(along_arc(0.7)));
    assert_eq!(form(&sketch, "k0000008"), &on("n0000000", "e0000000"));
    assert_eq!(form(&sketch, "k0000009"), &on("n0000001", "f0000000"));
}

// ---- Silico's own

/// Line a0000000 from p0 (0, 0) to p4 (32, 0), beside `points`, `lines`, and the rest.
fn a_line(points: &[String], lines: &[String], circles: &str, constraints: &str) -> Document {
    let points = [point("p0000000", 0.0, 0.0), point("p0000004", 32.0, 0.0)]
        .into_iter()
        .chain(points.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    let lines = std::iter::once(line("a0000000", "p0000000", "p0000004"))
        .chain(lines.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    doc(&points, &lines, circles, "", constraints)
}

/// A vertical cutter `{name}0000000` at `x`, from `{name}0000001` at y = −5 to
/// `{name}0000002` at y = 5.
fn upright(name: char, x: f64) -> (Vec<String>, String) {
    let (low, high) = (format!("{name}0000001"), format!("{name}0000002"));
    (
        vec![point(&low, x, -5.0), point(&high, x, 5.0)],
        line(&format!("{name}0000000"), &low, &high),
    )
}

#[test]
fn a_lines_direction_constraints_go_to_both_pieces_which_nothing_joins() {
    // §4, item 9 (assumeTangency = false; 1.1.3 puts a perpendicular to a line it doesn't
    // touch on both pieces): cutters at 8 and 24, picked at 16. The copies come after the
    // new piece, by the IDs of those they copy, then the new ends' ons.
    let (mut points, d) = upright('d', 8.0);
    let (more, e) = upright('e', 24.0);
    points.extend(more);
    points.push(point("f0000001", 40.0, 10.0));
    points.push(point("f0000002", 40.0, 20.0));
    let before = a_line(
        &points,
        &[d, e, line("f0000000", "f0000001", "f0000002")],
        "",
        r#"{"id": "k0000000", "type": "perpendicular", "a": "a0000000", "b": "f0000000"}, {"id": "k0000001", "type": "horizontal", "line": "a0000000"}"#,
    );
    let done = trim(
        sketch_of(&before),
        &id("a0000000"),
        [16.0, 0.0],
        &ids(&[
            "n0000000", "n0000001", "b1000000", "k0000002", "k0000003", "k0000004", "k0000005",
        ]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(at(&sketch, "n0000000"), (8.0, 0.0, false));
    assert_eq!(at(&sketch, "n0000001"), (24.0, 0.0, false));
    assert_eq!(
        form(&sketch, "k0000002"),
        &Form::Perpendicular {
            a: element("b1000000"),
            b: element("f0000000")
        }
    );
    assert_eq!(
        form(&sketch, "k0000003"),
        &Form::Horizontal {
            line: element("b1000000")
        }
    );
    assert_eq!(
        form(&sketch, "k0000000"),
        form(sketch_of(&before), "k0000000")
    );
    assert_eq!(form(&sketch, "k0000004"), &on("n0000000", "d0000000"));
    assert_eq!(form(&sketch, "k0000005"), &on("n0000001", "e0000000"));
    // No tangent_at joins the pieces.
    assert!(
        !sketch
            .constraints
            .iter()
            .any(|c| matches!(c.form, Form::TangentAt { .. }))
    );
    assert_eq!(
        done.edited.added,
        ids(&[
            "n0000000", "n0000001", "b1000000", "k0000002", "k0000003", "k0000004", "k0000005"
        ])
    );
    assert_eq!(done.edited.modified, ids(&["a0000000"]));
    assert!(done.edited.removed.is_empty());
}

#[test]
fn a_tangent_line_cuts_a_circle_and_one_more_than_1e_7_off_doesnt() {
    // Decision 6: a gap of at most 1e-7 mm is contact. y = 10 + δ from x = −20 to 20 over
    // the circle of radius 10 about the origin, picked at x = −10: at δ = 5e-8 the contact,
    // (0, 10.00000005), cuts it; at δ = 2e-7 nothing does, and the line goes whole.
    for (rise, cuts) in [(0.00000005, true), (0.0000002, false)] {
        let y = 10.0 + rise;
        let before = doc(
            &[
                point("p0000000", -20.0, y),
                point("p0000004", 20.0, y),
                point("z0000000", 0.0, 0.0),
            ]
            .join(", "),
            &line("a0000000", "p0000000", "p0000004"),
            &circle("c0000000", "z0000000", 10.0),
            "",
            "",
        );
        let picked = [-10.0, y];
        if cuts {
            let done = trim(
                sketch_of(&before),
                &id("a0000000"),
                picked,
                &ids(&["n0000000", "k0000009"]),
            )
            .unwrap();
            let sketch = gated(&before, &done);
            assert_eq!(at(&sketch, "n0000000"), (0.0, y, false), "{rise}");
            assert_eq!(form(&sketch, "k0000009"), &on("n0000000", "c0000000"));
        } else {
            let done = trim(sketch_of(&before), &id("a0000000"), picked, &[]).unwrap();
            assert!(done.pieces.is_empty(), "{rise}");
        }
    }
}

#[test]
fn a_cutters_end_within_1e_7_mm_of_the_line_cuts_it_and_one_farther_doesnt() {
    // e0000000 runs up x = 16 from 5e-8 above the line (an end on it, `e10068a648`): a cut;
    // from 2e-7 above, neither its end nor its line's crossing, which it doesn't reach, is.
    for (gap, cuts) in [(0.00000005, true), (0.0000002, false)] {
        let before = a_line(
            &[point("e0000001", 16.0, gap), point("e0000002", 16.0, 5.0)],
            &[line("e0000000", "e0000001", "e0000002")],
            "",
            "",
        );
        let result = trim(
            sketch_of(&before),
            &id("a0000000"),
            [8.0, 0.0],
            &ids(&["n0000000", "k0000009"]),
        );
        if cuts {
            let done = result.unwrap();
            let sketch = gated(&before, &done);
            assert_eq!(at(&sketch, "n0000000"), (16.0, 0.0, false), "{gap}");
        } else {
            assert_eq!(
                result.unwrap_err().message(),
                "the trim needs 0 new IDs, got 2",
                "{gap}"
            );
        }
    }
}

#[test]
fn collinear_overlapping_lines_cut_at_the_overlaps_ends() {
    // `3cbe309330`: b lies along a from 8 to 16; picked at 12, between its ends.
    let before = a_line(
        &[point("b0000001", 8.0, 0.0), point("b0000002", 16.0, 0.0)],
        &[line("b0000000", "b0000001", "b0000002")],
        "",
        "",
    );
    let done = trim(
        sketch_of(&before),
        &id("a0000000"),
        [12.0, 0.0],
        &ids(&["n0000000", "n0000001", "b1000000", "k0000008", "k0000009"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(at(&sketch, "n0000000"), (8.0, 0.0, false));
    assert_eq!(at(&sketch, "n0000001"), (16.0, 0.0, false));
    assert_eq!(form(&sketch, "k0000008"), &on("n0000000", "b0000000"));
    assert_eq!(form(&sketch, "k0000009"), &on("n0000001", "b0000000"));
}

#[test]
fn a_sketch_point_doesnt_cut_and_an_axis_doesnt_either() {
    // s0 is held on a at 16, and a runs across the y axis: neither cuts, so a goes whole,
    // with the on.
    let before = doc(
        &[
            point("p0000000", -16.0, 0.0),
            point("p0000004", 16.0, 0.0),
            point("s0000000", 8.0, 0.0),
        ]
        .join(", "),
        &line("a0000000", "p0000000", "p0000004"),
        "",
        "",
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "a0000000"}"#,
    );
    let done = trim(sketch_of(&before), &id("a0000000"), [4.0, 0.0], &[]).unwrap();
    let sketch = gated(&before, &done);
    assert!(sketch.lines.is_empty());
    assert!(has(&sketch, "s0000000"));
    assert_eq!(
        done.edited.removed,
        ids(&["p0000000", "p0000004", "a0000000", "k0000000"])
    );
}

#[test]
fn a_neighbour_sharing_the_end_doesnt_cut_and_keeps_the_end_when_its_trimmed_away() {
    // c starts at a's end p4 (no cut within 1e-7 mm of a's own end, 226d24792d), d crosses
    // a at 16. Picked at 24, a is trimmed back to 16; p4 stays, c's; the tangent_at of a and
    // c there goes, a no longer ending at p4.
    let (points, d) = upright('d', 16.0);
    let mut points = points;
    points.push(point("c0000001", 32.0, 10.0));
    let before = a_line(
        &points,
        &[d, line("c0000000", "p0000004", "c0000001")],
        "",
        r#"{"id": "k0000000", "type": "tangent_at", "a": "a0000000", "b": "c0000000", "at": "p0000004", "direction": "same"}"#,
    );
    let done = trim(
        sketch_of(&before),
        &id("a0000000"),
        [24.0, 0.0],
        &ids(&["n0000000", "k0000009"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("p0000000".to_owned(), "n0000000".to_owned())
    );
    assert_eq!(
        ends(&sketch, "c0000000"),
        ("p0000004".to_owned(), "c0000001".to_owned())
    );
    assert_eq!(done.edited.removed, ids(&["k0000000"]));
}

#[test]
fn two_cuts_within_1e_7_mm_of_each_other_remove_the_line_whole() {
    // FreeCAD's 5e-5 mm here is 1e-7 (the design): cutters at 16 and 16 + 5e-8, picked
    // between them.
    let (mut points, d) = upright('d', 16.0);
    let (more, e) = upright('e', 16.00000005);
    points.extend(more);
    let before = a_line(&points, &[d, e], "", "");
    let done = trim(
        sketch_of(&before),
        &id("a0000000"),
        [16.000000025, 0.0],
        &[],
    )
    .unwrap();
    assert!(done.pieces.is_empty());
    assert!(!has(&gated(&before, &done), "a0000000"));
}

#[test]
fn a_tangency_with_the_cutter_becomes_an_end_tangency_as_the_drawing_shows_it() {
    // y = 10 from x = −20 to 20 rests on the circle of radius 10 about the origin (k0, the
    // circle on its right); picked at x = −10, it's trimmed back to the contact (0, 10), and
    // k0 becomes a tangent_on of its new start on the circle (decision 8): the line leaves
    // along +x, the circle runs counter-clockwise there along −x: opposite. No on is added.
    let before = doc(
        &[
            point("p0000000", -20.0, 10.0),
            point("p0000004", 20.0, 10.0),
            point("z0000000", 0.0, 0.0),
        ]
        .join(", "),
        &line("a0000000", "p0000000", "p0000004"),
        &circle("c0000000", "z0000000", 10.0),
        "",
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "c0000000", "side": "right"}"#,
    );
    let done = trim(
        sketch_of(&before),
        &id("a0000000"),
        [-10.0, 10.0],
        &ids(&["n0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(at(&sketch, "n0000000"), (0.0, 10.0, false));
    assert_eq!(
        form(&sketch, "k0000000"),
        &Form::TangentOn {
            a: element("a0000000"),
            at: element("n0000000"),
            b: element("c0000000"),
            direction: Direction::Opposite,
        }
    );
    assert_eq!(sketch.constraints.len(), 1);
    assert_eq!(done.edited.added, ids(&["n0000000"]));
    assert_eq!(done.edited.modified, ids(&["a0000000", "k0000000"]));
    assert_eq!(done.edited.removed, ids(&["p0000000"]));
}

#[test]
fn an_arcs_edge_tangency_no_piece_touches_is_refused_naming_it() {
    // Decision 2: the upper half of the circle of radius 10 about the origin, cut at
    // x = ±5 and picked at the top, where the line t, along y = 10 from x = 5 to 20 (so not
    // reaching the contact, and no cut), touches it.
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("r0000000", 10.0, 0.0),
            point("r0000001", -10.0, 0.0),
            point("t0000001", 5.0, 10.0),
            point("t0000002", 20.0, 10.0),
            point("d0000001", 5.0, 0.0),
            point("d0000002", 5.0, 20.0),
            point("e0000001", -5.0, 0.0),
            point("e0000002", -5.0, 20.0),
        ]
        .join(", "),
        &[
            line("t0000000", "t0000001", "t0000002"),
            line("d0000000", "d0000001", "d0000002"),
            line("e0000000", "e0000001", "e0000002"),
        ]
        .join(", "),
        "",
        &arc("r1000000", "z0000000", "r0000000", "r0000001"),
        r#"{"id": "k0000000", "type": "tangent", "a": "r1000000", "b": "t0000000", "side": "right"}"#,
    );
    let error = trim(
        sketch_of(&before),
        &id("r1000000"),
        [0.0, 10.0],
        &ids(&["n0000000", "n0000001", "b1000000", "k0000008", "k0000009"]),
    )
    .unwrap_err();
    assert_eq!(
        said(&error),
        (
            "trimming arc r1000000 there would leave constraint k0000000, an edge tangency with t0000000, on no piece of it",
            Some("pick"),
            vec!["k0000000", "r1000000"]
        )
    );
    assert_eq!(error.cause(), Some("no piece of the arc touches t0000000"));
}

#[test]
fn a_member_that_isnt_a_curve_a_pick_not_finite_and_bad_ids_are_refused() {
    let (points, d) = upright('d', 16.0);
    let before = a_line(
        &points,
        &[d],
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}"#,
    );
    let sketch = sketch_of(&before);
    let two = ids(&["n0000000", "k0000009"]);
    for (curve, message) in [
        (
            "p0000000",
            "point p0000000 isn't a curve: a line, an arc, or a circle trims",
        ),
        (
            "k0000000",
            "constraint k0000000 isn't a curve: a line, an arc, or a circle trims",
        ),
    ] {
        let error = trim(sketch, &id(curve), [8.0, 0.0], &two).unwrap_err();
        assert_eq!(said(&error), (message, Some("curve"), vec![curve]));
    }
    assert_eq!(
        trim(sketch, &id("zzzzzzzz"), [8.0, 0.0], &two)
            .unwrap_err()
            .message(),
        "the sketch has no curve zzzzzzzz"
    );
    assert_eq!(
        said(&trim(sketch, &id("a0000000"), [f64::NAN, 0.0], &two).unwrap_err()),
        (
            "the pick isn't a finite position",
            Some("pick"),
            vec!["a0000000"]
        )
    );
    assert_eq!(
        said(&trim(sketch, &id("a0000000"), [8.0, 0.0], &two[..1]).unwrap_err()),
        ("the trim needs 2 new IDs, got 1", Some("ids"), vec![])
    );
    assert_eq!(
        said(
            &trim(
                sketch,
                &id("a0000000"),
                [8.0, 0.0],
                &ids(&["n0000000", "d0000001"])
            )
            .unwrap_err()
        ),
        (
            "the sketch already has a member d0000001",
            Some("ids.1"),
            vec![]
        )
    );
    assert_eq!(
        said(
            &trim(
                sketch,
                &id("a0000000"),
                [8.0, 0.0],
                &ids(&["n0000000", "n0000000"])
            )
            .unwrap_err()
        ),
        ("ID n0000000 is given twice", Some("ids.1"), vec![])
    );
    assert_eq!(sketch, sketch_of(&before));
}
