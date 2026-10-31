//! Joining a split's two pieces (docs/design/m5.0-app.md, open item 12b and `sketch_join`;
//! docs/ports/sketch-edit.md, §3, *Join*): split's exact inverse. The defining tests are the
//! round trips: a split then a join at the split point gives the document before the split
//! byte for byte, as the canonical writer writes it, for a line, an arc, and a circle with
//! constraints on the curve; and a join then a split at the same place gives the document
//! before the join. Each other expected value is worked by hand: which piece keeps its ID,
//! what's removed, what's re-pointed, and each refusal's words.

use silico_core::Error;
use silico_core::document::sketch_edit::{Joined, join, split};
use silico_core::document::{
    Direction, Document, ElementId, Form, Length, Operation, Reference, Sketch, read, validate,
    write,
};

use crate::fixture::{arc, circle, document, fields, line, line_as, point};

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
    let document =
        read(document(&fields(points, lines, circles, arcs, constraints)).as_bytes()).unwrap();
    if let Err(invalid) = validate(&document) {
        panic!("the gate refused the fixture: {:?}", invalid.errors());
    }
    document
}

fn sketch_of(document: &Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

/// `document` with `sketch` in place of its sketch, checked by the gate.
#[track_caller]
fn with(document: &Document, sketch: &Sketch) -> Document {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(sketch.clone());
    if let Err(invalid) = validate(&document) {
        panic!("the gate refused the edit: {:?}", invalid.errors());
    }
    document
}

/// The document as the canonical writer writes it: the bytes a save puts in the file.
fn text(document: &Document) -> String {
    write(document).unwrap()
}

/// `curve` of `before` split at `at` with `new`, then joined at `point`: the split's
/// document, the join's, and the join.
#[track_caller]
fn split_then_join(
    before: &Document,
    curve: &str,
    at: &[[f64; 2]],
    new: &[&str],
    point: &str,
) -> (Document, Document, Joined) {
    let done = split(sketch_of(before), &id(curve), at, &ids(new)).unwrap();
    let split_doc = with(before, &done.edited.sketch);
    let joined = join(sketch_of(&split_doc), &id(point)).unwrap();
    let joined_doc = with(&split_doc, &joined.edited.sketch);
    (split_doc, joined_doc, joined)
}

/// The refusal's message, parameter, elements, cause, and suggestion.
fn said(error: &Error) -> (&str, Option<&str>, Vec<&str>, Option<&str>, Option<&str>) {
    (
        error.message(),
        error.parameter(),
        error.elements().iter().map(String::as_str).collect(),
        error.cause(),
        error.suggestion(),
    )
}

#[track_caller]
fn refused(document: &Document, point: &str) -> Error {
    join(sketch_of(document), &id(point)).unwrap_err()
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

// ---- lines

/// Line a0 from p0 (0, 0) to p4 (32, 0), a line e0 from q0 (10, 10) to q2 (20, 10), a point
/// s1 at (24, 0) and d0 at (25, 3), beside `more` points; every split position here at
/// x = 8, whose projection is exact in binary.
const LINE_POINTS: &str = r#"{"id": "p0000000", "x": 0, "y": 0, "construction": false}, {"id": "p0000004", "x": 32, "y": 0, "construction": false}, {"id": "q0000000", "x": 10, "y": 10, "construction": false}, {"id": "q0000002", "x": 20, "y": 10, "construction": false}, {"id": "s0000001", "x": 24, "y": 0, "construction": false}, {"id": "d0000000", "x": 25, "y": 3, "construction": false}"#;

fn a_line(more_points: &str, more_lines: &str, constraints: &str) -> Document {
    let points = [LINE_POINTS, more_points]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let lines = [
        line("a0000000", "p0000000", "p0000004"),
        line("e0000000", "q0000000", "q0000002"),
        more_lines.to_owned(),
    ]
    .into_iter()
    .filter(|l| !l.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    doc(&points, &lines, "", "", constraints)
}

/// The constraints on a0 the line round trips carry: a horizontal and an angle to e0
/// (directions, which stay on the start-side piece), a point s1 held on it past the split
/// and a point d0 held 3 mm left of it past the split (each moved to the second piece and
/// back), and the origin's place.
const LINE_CONSTRAINTS: &str = r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}, {"id": "k0000001", "type": "angle", "a": "a0000000", "b": "e0000000", "value": 0, "driving": false}, {"id": "k0000002", "type": "on", "point": "s0000001", "curve": "a0000000"}, {"id": "k0000003", "type": "point_line_distance", "point": "d0000000", "line": "a0000000", "value": 3, "side": "left", "driving": true}, {"id": "k0000004", "type": "distance_x", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#;

#[test]
fn a_line_split_then_joined_at_the_split_point_is_the_document_before_byte_for_byte() {
    // Its outer ends dimensioned by a distance of 32, as a split leaves a length: the split
    // at (8, 0) moves k2 and k3 to the second piece and joins the pieces by j0; the join
    // at n0 takes them back, and removes n0, b1, and j0.
    let before = a_line(
        "",
        "",
        &format!(
            r#"{LINE_CONSTRAINTS}, {{"id": "k0000005", "type": "distance", "a": "p0000000", "b": "p0000004", "value": 32, "driving": true}}"#
        ),
    );
    let (split_doc, joined_doc, joined) = split_then_join(
        &before,
        "a0000000",
        &[[8.0, 0.0]],
        &["n0000000", "b1000000", "j0000000"],
        "n0000000",
    );
    assert_ne!(text(&split_doc), text(&before));
    assert_eq!(text(&joined_doc), text(&before));
    assert_eq!(joined.pieces, [id("a0000000"), id("b1000000")]);
    assert!(joined.edited.added.is_empty());
    assert_eq!(
        joined.edited.modified,
        ids(&["a0000000", "k0000002", "k0000003"])
    );
    assert_eq!(
        joined.edited.removed,
        ids(&["n0000000", "b1000000", "j0000000"])
    );
}

#[test]
fn a_lines_length_split_then_joined_is_the_distance_of_its_outer_ends_nothing_retyped() {
    // A split makes a length the distance of the outer ends (4d6b1f3eb8), and the join
    // retypes nothing (open item 12b): the document before, but for k5, which is the
    // distance of p0 and p4 of 32 under its ID, as the split wrote it.
    let length =
        r#"{"id": "k0000005", "type": "length", "line": "a0000000", "value": 32, "driving": true}"#;
    let distance = r#"{"id": "k0000005", "type": "distance", "a": "p0000000", "b": "p0000004", "value": 32, "driving": true}"#;
    let before = a_line("", "", &format!("{LINE_CONSTRAINTS}, {length}"));
    let (_, joined_doc, joined) = split_then_join(
        &before,
        "a0000000",
        &[[8.0, 0.0]],
        &["n0000000", "b1000000", "j0000000"],
        "n0000000",
    );
    let expected = a_line("", "", &format!("{LINE_CONSTRAINTS}, {distance}"));
    assert_eq!(text(&joined_doc), text(&expected));
    // k5 isn't the join's: the split changed it, and the join leaves it.
    assert!(!joined.edited.modified.contains(&id("k0000005")));
}

#[test]
fn a_line_joined_then_split_at_the_same_place_is_the_document_before_byte_for_byte() {
    // Two pieces as a split leaves them: a0 from p0 to n0 (8, 0), b1 from n0 to p4, held by
    // j0, with s1 and d0 held on the second piece and the horizontal on the first. Joined
    // at n0, then split at n0's place with n0, b1, and j0 again.
    let before = doc(
        &format!(r#"{LINE_POINTS}, {}"#, point("n0000000", 8.0, 0.0)),
        &[
            line("a0000000", "p0000000", "n0000000"),
            line("e0000000", "q0000000", "q0000002"),
            line("b1000000", "n0000000", "p0000004"),
        ]
        .join(", "),
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}, {"id": "k0000001", "type": "angle", "a": "a0000000", "b": "e0000000", "value": 0, "driving": false}, {"id": "k0000002", "type": "on", "point": "s0000001", "curve": "b1000000"}, {"id": "k0000003", "type": "point_line_distance", "point": "d0000000", "line": "b1000000", "value": 3, "side": "left", "driving": true}, {"id": "j0000000", "type": "tangent_at", "a": "a0000000", "b": "b1000000", "at": "n0000000", "direction": "same"}"#,
    );
    let joined = join(sketch_of(&before), &id("n0000000")).unwrap();
    let joined_doc = with(&before, &joined.edited.sketch);
    let line_ends: Vec<_> = sketch_of(&joined_doc)
        .lines
        .iter()
        .map(|l| (l.id.to_string(), l.from.to_string(), l.to.to_string()))
        .collect();
    assert_eq!(
        line_ends,
        [
            ("a0000000", "p0000000", "p0000004"),
            ("e0000000", "q0000000", "q0000002")
        ]
        .map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.to_owned()))
    );
    let again = split(
        sketch_of(&joined_doc),
        &id("a0000000"),
        &[[8.0, 0.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    assert_eq!(
        text(&with(&joined_doc, &again.edited.sketch)),
        text(&before)
    );
}

#[test]
fn a_direction_on_the_second_piece_goes_to_the_joined_line_which_a_split_keeps_on_the_first() {
    // Join then split isn't byte for byte where the second piece holds a direction: the
    // join puts it on the joined line, and a split keeps directions on its start-side
    // piece (decision 1). A horizontal on b1, joined, is a0's; split again, it stays a0's.
    let before = doc(
        &format!(r#"{LINE_POINTS}, {}"#, point("n0000000", 8.0, 0.0)),
        &[
            line("a0000000", "p0000000", "n0000000"),
            line("e0000000", "q0000000", "q0000002"),
            line("b1000000", "n0000000", "p0000004"),
        ]
        .join(", "),
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "b1000000"}, {"id": "j0000000", "type": "tangent_at", "a": "b1000000", "b": "a0000000", "at": "n0000000", "direction": "same"}"#,
    );
    // The tangent_at named the other way round holds them on one line too.
    let joined = join(sketch_of(&before), &id("n0000000")).unwrap();
    let joined_doc = with(&before, &joined.edited.sketch);
    assert_eq!(
        form(sketch_of(&joined_doc), "k0000000"),
        &Form::Horizontal {
            line: element("a0000000")
        }
    );
    assert_eq!(joined.edited.modified, ids(&["a0000000", "k0000000"]));
    let again = split(
        sketch_of(&joined_doc),
        &id("a0000000"),
        &[[8.0, 0.0]],
        &ids(&["n0000000", "b1000000", "j0000000"]),
    )
    .unwrap();
    assert_eq!(
        form(&again.edited.sketch, "k0000000"),
        &Form::Horizontal {
            line: element("a0000000")
        }
    );
}

/// Pieces a0 (p0 to n0) and b1 (n0 to p4) as a split leaves them, joined by j0, with
/// `constraints` after it.
fn pieces(more_points: &str, more_lines: &str, constraints: &str) -> Document {
    let points = [
        LINE_POINTS.to_owned(),
        point("n0000000", 8.0, 0.0),
        more_points.to_owned(),
    ]
    .into_iter()
    .filter(|p| !p.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    let lines = [
        line("a0000000", "p0000000", "n0000000"),
        line("e0000000", "q0000000", "q0000002"),
        line("b1000000", "n0000000", "p0000004"),
        more_lines.to_owned(),
    ]
    .into_iter()
    .filter(|l| !l.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    let join = r#"{"id": "j0000000", "type": "tangent_at", "a": "a0000000", "b": "b1000000", "at": "n0000000", "direction": "same"}"#;
    let constraints = if constraints.is_empty() {
        join.to_owned()
    } else {
        format!("{join}, {constraints}")
    };
    doc(&points, &lines, "", "", &constraints)
}

#[test]
fn constraints_on_the_dropped_piece_are_re_pointed_and_repeats_dropped_and_listed() {
    // k0 horizontal a0 and k1 horizontal b1: one horizontal, k0, kept; k1 repeats it,
    // removed. k2 perpendicular a0, e0... e0 is horizontal here, so an angle: k2 angle of
    // a0 to e0 and k3 angle of b1 to e0, both 0, k3 removed. k4 parallel e0, b1 and k5
    // parallel a0, e0: the same pair the other way round, k4 removed. k6 parallel a0, b1
    // and k7 equal a0, b1 name the joined line twice: removed. k8 holds s1 on b1:
    // re-pointed, modified. k9 holds p4, b1's far end, on a0: the joined line's own end
    // then, implied, removed. kA distance of n0 from q0: names the point removed, removed.
    let before = pieces(
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}, {"id": "k0000001", "type": "horizontal", "line": "b1000000"}, {"id": "k0000002", "type": "angle", "a": "a0000000", "b": "e0000000", "value": 0, "driving": false}, {"id": "k0000003", "type": "angle", "a": "b1000000", "b": "e0000000", "value": 0, "driving": false}, {"id": "k0000004", "type": "parallel", "a": "e0000000", "b": "b1000000"}, {"id": "k0000005", "type": "parallel", "a": "a0000000", "b": "e0000000"}, {"id": "k0000006", "type": "parallel", "a": "a0000000", "b": "b1000000"}, {"id": "k0000007", "type": "equal", "a": "a0000000", "b": "b1000000"}, {"id": "k0000008", "type": "on", "point": "s0000001", "curve": "b1000000"}, {"id": "k0000009", "type": "on", "point": "p0000004", "curve": "a0000000"}, {"id": "k000000a", "type": "distance", "a": "q0000000", "b": "n0000000", "value": 12, "driving": true}"#,
    );
    let joined = join(sketch_of(&before), &id("n0000000")).unwrap();
    let after = with(&before, &joined.edited.sketch);
    let kept: Vec<String> = sketch_of(&after)
        .constraints
        .iter()
        .map(|c| c.id.to_string())
        .collect();
    assert_eq!(kept, ["k0000000", "k0000002", "k0000005", "k0000008"]);
    assert_eq!(
        form(sketch_of(&after), "k0000008"),
        &Form::On {
            point: element("s0000001"),
            curve: element("a0000000"),
        }
    );
    assert_eq!(joined.edited.modified, ids(&["a0000000", "k0000008"]));
    assert_eq!(
        joined.edited.removed,
        ids(&[
            "n0000000", "b1000000", "j0000000", "k0000001", "k0000003", "k0000004", "k0000006",
            "k0000007", "k0000009", "k000000a"
        ])
    );
    let line_ends: Vec<_> = sketch_of(&after)
        .lines
        .iter()
        .map(|l| {
            (
                l.id.as_str().to_owned(),
                l.from.as_str().to_owned(),
                l.to.as_str().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        line_ends,
        [
            ("a0000000", "p0000000", "p0000004"),
            ("e0000000", "q0000000", "q0000002")
        ]
        .map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.to_owned()))
    );
    // No coordinate moves: the points left are the points as they were.
    let before_points: Vec<_> = sketch_of(&before)
        .points
        .iter()
        .filter(|p| p.id != id("n0000000"))
        .cloned()
        .collect();
    assert_eq!(sketch_of(&after).points, before_points);
}

#[test]
fn a_constraint_on_the_far_end_goes_with_it_to_the_joined_line() {
    // A line g0 from p4 on to (40, 8), tangent at p4 to b1, which ends there: the join
    // makes it a0's, whose end p4 is then.
    let before = pieces(
        &point("g0000001", 40.0, 8.0),
        &line("g0000000", "p0000004", "g0000001"),
        r#"{"id": "k0000000", "type": "tangent_at", "a": "b1000000", "b": "g0000000", "at": "p0000004", "direction": "opposite"}"#,
    );
    let joined = join(sketch_of(&before), &id("n0000000")).unwrap();
    let after = with(&before, &joined.edited.sketch);
    assert_eq!(
        form(sketch_of(&after), "k0000000"),
        &Form::TangentAt {
            a: element("a0000000"),
            b: element("g0000000"),
            at: element("p0000004"),
            direction: Direction::Opposite,
        }
    );
}

// ---- arcs

/// An arc a0 about c0 (1, 2), radius 5, from r0 (4, 6) counter-clockwise to r1 (1, −3),
/// with a line e0 from g0 (8, 3) into r0 and f0 from r1 on to g1 (5, −3), a point s0 at
/// (4, −2), at about 307° on its circle, past the arc's end (it runs from about 53° to
/// 270°) and nearer r1 than r0, and s2 at (1, 7), at 90°: split at (−4, 2), at π, exact in
/// binary.
fn the_arc(constraints: &str) -> Document {
    let points = [
        point("c0000000", 1.0, 2.0),
        point("r0000000", 4.0, 6.0),
        point("r0000001", 1.0, -3.0),
        point("g0000000", 8.0, 3.0),
        point("g0000001", 5.0, -3.0),
        point("s0000000", 4.0, -2.0),
        point("s0000002", 1.0, 7.0),
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
fn an_arc_split_then_joined_at_the_split_point_is_the_document_before_byte_for_byte() {
    // The radius and the tangent_at at the start stay on a0; the tangent_at at the end and
    // s0, held on the circle past the end nearer r1, go to b1 with the split; s2, held at
    // π/2, stays on a0. The join takes each back.
    let before = the_arc(
        r#"{"id": "k0000000", "type": "radius", "curve": "a0000000", "value": 5, "driving": true}, {"id": "k0000001", "type": "tangent_at", "a": "e0000000", "b": "a0000000", "at": "r0000000", "direction": "same"}, {"id": "k0000002", "type": "tangent_at", "a": "a0000000", "b": "f0000000", "at": "r0000001", "direction": "same"}, {"id": "k0000003", "type": "on", "point": "s0000000", "curve": "a0000000"}, {"id": "k0000004", "type": "on", "point": "s0000002", "curve": "a0000000"}"#,
    );
    let (split_doc, joined_doc, joined) = split_then_join(
        &before,
        "a0000000",
        &[[-4.0, 2.0]],
        &["n0000000", "b1000000"],
        "n0000000",
    );
    assert_eq!(
        form(sketch_of(&split_doc), "k0000002"),
        &Form::TangentAt {
            a: element("b1000000"),
            b: element("f0000000"),
            at: element("r0000001"),
            direction: Direction::Same,
        }
    );
    assert_eq!(text(&joined_doc), text(&before));
    assert_eq!(joined.pieces, [id("a0000000"), id("b1000000")]);
    assert_eq!(
        joined.edited.modified,
        ids(&["a0000000", "k0000002", "k0000003"])
    );
    assert_eq!(joined.edited.removed, ids(&["n0000000", "b1000000"]));
}

#[test]
fn an_arc_joined_then_split_at_the_same_place_is_the_document_before_byte_for_byte() {
    let before = {
        let points = [
            point("c0000000", 1.0, 2.0),
            point("r0000000", 4.0, 6.0),
            point("r0000001", 1.0, -3.0),
            point("g0000000", 8.0, 3.0),
            point("g0000001", 5.0, -3.0),
            point("s0000000", 4.0, -2.0),
            point("n0000000", -4.0, 2.0),
        ]
        .join(", ");
        let lines = [
            line("e0000000", "g0000000", "r0000000"),
            line("f0000000", "r0000001", "g0000001"),
        ]
        .join(", ");
        let arcs = [
            arc("a0000000", "c0000000", "r0000000", "n0000000"),
            arc("b1000000", "c0000000", "n0000000", "r0000001"),
        ]
        .join(", ");
        doc(
            &points,
            &lines,
            "",
            &arcs,
            r#"{"id": "k0000000", "type": "radius", "curve": "a0000000", "value": 5, "driving": true}, {"id": "k0000002", "type": "tangent_at", "a": "b1000000", "b": "f0000000", "at": "r0000001", "direction": "same"}, {"id": "k0000003", "type": "on", "point": "s0000000", "curve": "b1000000"}"#,
        )
    };
    let joined = join(sketch_of(&before), &id("n0000000")).unwrap();
    let joined_doc = with(&before, &joined.edited.sketch);
    let arc_now = &sketch_of(&joined_doc).arcs;
    assert_eq!(arc_now.len(), 1);
    assert_eq!(
        (
            arc_now[0].id.as_str(),
            arc_now[0].start.as_str(),
            arc_now[0].end.as_str()
        ),
        ("a0000000", "r0000000", "r0000001")
    );
    let again = split(
        sketch_of(&joined_doc),
        &id("a0000000"),
        &[[-4.0, 2.0]],
        &ids(&["n0000000", "b1000000"]),
    )
    .unwrap();
    assert_eq!(
        text(&with(&joined_doc, &again.edited.sketch)),
        text(&before)
    );
}

// ---- circles

/// Circle c0 about z0 (1, 2), radius 3, with a radius, an equal to circle c2 of radius 3
/// about z2 (20, 2), a point t0 (1, 5) held on it at π/2 and t1 (1, −1) at 3π/2, and a line
/// h0 from (5, −1) to (−5, −1) tangent below it, its contact at 3π/2: split at (4, 2) and
/// (−2, 2), at 0 and π, exact in binary, c0 keeps the arc over the top, and t1 and the
/// tangent go to the arc under it.
fn the_circle() -> Document {
    let points = [
        point("z0000000", 1.0, 2.0),
        point("z0000002", 20.0, 2.0),
        point("t0000000", 1.0, 5.0),
        point("t0000001", 1.0, -1.0),
        point("h0000001", 5.0, -1.0),
        point("h0000002", -5.0, -1.0),
    ]
    .join(", ");
    let circles = [
        circle("c0000000", "z0000000", 3.0),
        circle("c0000002", "z0000002", 3.0),
    ]
    .join(", ");
    doc(
        &points,
        &line("h0000000", "h0000001", "h0000002"),
        &circles,
        "",
        r#"{"id": "k0000000", "type": "radius", "curve": "c0000000", "value": 3, "driving": true}, {"id": "k0000001", "type": "equal", "a": "c0000000", "b": "c0000002"}, {"id": "k0000002", "type": "on", "point": "t0000000", "curve": "c0000000"}, {"id": "k0000003", "type": "on", "point": "t0000001", "curve": "c0000000"}, {"id": "k0000004", "type": "tangent", "a": "h0000000", "b": "c0000000", "side": "right"}"#,
    )
}

#[test]
fn a_circle_split_then_joined_at_its_second_split_point_is_the_document_before_byte_for_byte() {
    // Split at (4, 2), n0, and (−2, 2), n1: c0 runs from n0 to n1, c1 from n1 to n0. Joined
    // at n1, the arc that ends there, c0, keeps its ID and is the circle again, of its
    // radius, 3, from z0 to n0; n0, n1, and c1 go.
    let before = the_circle();
    let (split_doc, joined_doc, joined) = split_then_join(
        &before,
        "c0000000",
        &[[4.0, 2.0], [-2.0, 2.0]],
        &["n0000000", "n0000001", "c1000000"],
        "n0000001",
    );
    assert!(sketch_of(&split_doc).circles.len() == 1);
    assert_eq!(
        form(sketch_of(&split_doc), "k0000003"),
        &Form::On {
            point: element("t0000001"),
            curve: element("c1000000"),
        }
    );
    assert_eq!(text(&joined_doc), text(&before));
    assert_eq!(joined.pieces, [id("c0000000"), id("c1000000")]);
    assert_eq!(
        joined.edited.modified,
        ids(&["c0000000", "k0000003", "k0000004"])
    );
    assert_eq!(
        joined.edited.removed,
        ids(&["n0000000", "n0000001", "c1000000"])
    );
}

#[test]
fn a_split_circle_joined_at_its_first_split_point_keeps_the_arc_that_ends_there() {
    // The same rule at n0: the arc that ends there is c1, which keeps its ID; the circle
    // is c1, and c0's constraints name it.
    let before = the_circle();
    let (_, joined_doc, joined) = split_then_join(
        &before,
        "c0000000",
        &[[4.0, 2.0], [-2.0, 2.0]],
        &["n0000000", "n0000001", "c1000000"],
        "n0000000",
    );
    assert_eq!(joined.pieces, [id("c1000000"), id("c0000000")]);
    let sketch = sketch_of(&joined_doc);
    let circles: Vec<_> = sketch
        .circles
        .iter()
        .map(|c| (c.id.to_string(), c.center.to_string(), c.radius.mm()))
        .collect();
    assert_eq!(
        circles,
        [
            ("c0000002".to_owned(), "z0000002".to_owned(), 3.0),
            ("c1000000".to_owned(), "z0000000".to_owned(), 3.0)
        ]
    );
    assert!(sketch.arcs.is_empty());
    assert_eq!(
        form(sketch, "k0000000"),
        &Form::Radius {
            curve: element("c1000000"),
            value: Length::parse("3").unwrap(),
            driving: true,
        }
    );
    assert_eq!(
        joined.edited.modified,
        ids(&["c1000000", "k0000000", "k0000001", "k0000002"])
    );
    assert_eq!(
        joined.edited.removed,
        ids(&["n0000000", "n0000001", "c0000000"])
    );
}

#[test]
fn a_circles_two_arcs_joined_then_split_at_the_same_places_is_the_document_before() {
    let points = [
        point("z0000000", 1.0, 2.0),
        point("t0000001", 1.0, -1.0),
        point("n0000000", 4.0, 2.0),
        point("n0000001", -2.0, 2.0),
    ]
    .join(", ");
    let arcs = [
        arc("c0000000", "z0000000", "n0000000", "n0000001"),
        arc("c1000000", "z0000000", "n0000001", "n0000000"),
    ]
    .join(", ");
    let before = doc(
        &points,
        "",
        "",
        &arcs,
        r#"{"id": "k0000000", "type": "radius", "curve": "c0000000", "value": 3, "driving": true}, {"id": "k0000003", "type": "on", "point": "t0000001", "curve": "c1000000"}"#,
    );
    let joined = join(sketch_of(&before), &id("n0000001")).unwrap();
    let joined_doc = with(&before, &joined.edited.sketch);
    let again = split(
        sketch_of(&joined_doc),
        &id("c0000000"),
        &[[4.0, 2.0], [-2.0, 2.0]],
        &ids(&["n0000000", "n0000001", "c1000000"]),
    )
    .unwrap();
    assert_eq!(
        text(&with(&joined_doc, &again.edited.sketch)),
        text(&before)
    );
}

// ---- refusals

#[test]
fn a_member_that_isnt_a_point_or_a_point_that_isnt_two_pieces_is_refused_naming_it() {
    let before = pieces(
        &[point("w0000000", 50.0, 50.0), point("w0000001", 60.0, 50.0)].join(", "),
        &line("w1000000", "n0000000", "w0000001"),
        "",
    );
    let error = refused(&before, "zz000000");
    assert_eq!(
        said(&error),
        (
            "the sketch has no point zz000000",
            Some("point"),
            vec![],
            None,
            Some("name a point of this sketch by its ID")
        )
    );
    let error = refused(&before, "a0000000");
    assert_eq!(
        said(&error),
        (
            "line a0000000 isn't a point: a join takes the point two pieces share",
            Some("point"),
            vec!["a0000000"],
            None,
            Some("name the point a split left between two pieces")
        )
    );
    let error = refused(&before, "w0000000");
    assert_eq!(
        said(&error),
        (
            "point w0000000 is used by no curve, so there's nothing to join",
            Some("point"),
            vec!["w0000000"],
            None,
            Some("name a point two pieces share")
        )
    );
    let error = refused(&before, "q0000000");
    assert_eq!(
        said(&error),
        (
            "point q0000000 is used by no curve but line e0000000, so there's nothing to join",
            Some("point"),
            vec!["q0000000", "e0000000"],
            None,
            Some("name a point two pieces share")
        )
    );
    let error = refused(&before, "n0000000");
    assert_eq!(
        said(&error),
        (
            "point n0000000 is shared by line a0000000, line b1000000 and line w1000000: a join takes a point only its two pieces use",
            Some("point"),
            vec!["n0000000", "a0000000", "b1000000", "w1000000"],
            None,
            Some("unjoin the other curves from it first")
        )
    );
}

#[test]
fn a_centre_a_line_and_an_arc_and_construction_with_not_are_refused_naming_them() {
    // A line y0 from (0, 20) to the centre z0 of circle c0: the point is the circle's
    // centre. A line v0 from (40, 0) ends at r0 (45, 0), where arc m0 about (45, 5) starts:
    // a line and an arc. Construction x1 and x2, held smooth at x2 (70, 0): one of each.
    let points = [
        point("y0000001", 0.0, 20.0),
        point("z0000000", 0.0, 30.0),
        point("v0000001", 40.0, 0.0),
        point("r0000000", 45.0, 0.0),
        point("m0000001", 45.0, 5.0),
        point("m0000002", 50.0, 5.0),
        point("x0000001", 60.0, 0.0),
        point("x0000002", 70.0, 0.0),
        point("x0000003", 80.0, 0.0),
    ]
    .join(", ");
    let lines = [
        line("y0000000", "y0000001", "z0000000"),
        line("v0000000", "v0000001", "r0000000"),
        line_as("x1000000", "x0000001", "x0000002", true),
        line("x2000000", "x0000002", "x0000003"),
    ]
    .join(", ");
    let before = doc(
        &points,
        &lines,
        &circle("c0000000", "z0000000", 3.0),
        &arc("m0000000", "m0000001", "r0000000", "m0000002"),
        r#"{"id": "k0000000", "type": "tangent_at", "a": "x1000000", "b": "x2000000", "at": "x0000002", "direction": "same"}"#,
    );
    let error = refused(&before, "z0000000");
    assert_eq!(
        said(&error),
        (
            "point z0000000 is circle c0000000's centre: a join takes a point two pieces end at",
            Some("point"),
            vec!["z0000000", "c0000000"],
            None,
            Some("name the point a split left between two pieces")
        )
    );
    let error = refused(&before, "r0000000");
    assert_eq!(
        said(&error),
        (
            "line v0000000 and arc m0000000 meet at point r0000000, but a line and an arc are never one curve",
            Some("point"),
            vec!["r0000000", "v0000000", "m0000000"],
            None,
            Some("join two lines held on one line, or two arcs about one centre")
        )
    );
    let error = refused(&before, "x0000002");
    assert_eq!(
        said(&error),
        (
            "line x1000000 is construction and line x2000000 isn't: a join makes one curve, construction or not",
            Some("point"),
            vec!["x0000002", "x1000000", "x2000000"],
            None,
            Some("make both pieces construction, or neither, then join")
        )
    );
}

#[test]
fn lines_not_held_on_one_line_or_not_running_on_through_the_point_are_refused() {
    // The bracket's corner: two lines meeting square, no tangent_at.
    let corner = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000002", 10.0, 10.0),
        ]
        .join(", "),
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "p0000001", "p0000002"),
        ]
        .join(", "),
        "",
        "",
        "",
    );
    let error = refused(&corner, "p0000001");
    assert_eq!(
        said(&error),
        (
            "lines a0000000 and b0000000 meet at point p0000001, but no tangent_at (same) there holds them on one line",
            Some("point"),
            vec!["p0000001", "a0000000", "b0000000"],
            Some(
                "a split holds a line's two pieces on one line by a tangent_at (same) at the point between them, which a join undoes"
            ),
            Some("add a tangent_at (same) of the two at the point, then join")
        )
    );
    // Held `opposite`, a cusp: not one line either.
    let cusp = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("n0000000", 8.0, 0.0),
            point("p0000004", 32.0, 0.0),
        ]
        .join(", "),
        &[
            line("a0000000", "p0000000", "n0000000"),
            line("b1000000", "n0000000", "p0000004"),
        ]
        .join(", "),
        "",
        "",
        r#"{"id": "j0000000", "type": "tangent_at", "a": "a0000000", "b": "b1000000", "at": "n0000000", "direction": "opposite"}"#,
    );
    assert_eq!(
        refused(&cusp, "n0000000").message(),
        "lines a0000000 and b1000000 meet at point n0000000, but no tangent_at (same) there holds them on one line"
    );
    // Both starting at the point, held smooth: no piece ends there.
    let both = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("n0000000", 8.0, 0.0),
            point("p0000004", 32.0, 0.0),
        ]
        .join(", "),
        &[
            line("a0000000", "n0000000", "p0000000"),
            line("b1000000", "n0000000", "p0000004"),
        ]
        .join(", "),
        "",
        "",
        r#"{"id": "j0000000", "type": "tangent_at", "a": "a0000000", "b": "b1000000", "at": "n0000000", "direction": "same"}"#,
    );
    assert_eq!(
        said(&refused(&both, "n0000000")),
        (
            "lines a0000000 and b1000000 both start at point n0000000: a join takes a piece that ends there and one that starts there",
            Some("point"),
            vec!["n0000000", "a0000000", "b1000000"],
            None,
            Some("join pieces that run on through the point, as a split leaves them")
        )
    );
}

#[test]
fn lines_whose_join_would_end_at_one_point_at_both_ends_are_refused() {
    // A curve's own two ends (FreeCAD's join refuses it): a0 from p0 to n0, b1 from n0 back
    // to p0, held smooth at n0 (one runs back over the other).
    let back = doc(
        &[point("p0000000", 0.0, 0.0), point("n0000000", 8.0, 0.0)].join(", "),
        &[
            line("a0000000", "p0000000", "n0000000"),
            line("b1000000", "n0000000", "p0000000"),
        ]
        .join(", "),
        "",
        "",
        r#"{"id": "j0000000", "type": "tangent_at", "a": "a0000000", "b": "b1000000", "at": "n0000000", "direction": "same"}"#,
    );
    assert_eq!(
        said(&refused(&back, "n0000000")),
        (
            "joining lines a0000000 and b1000000 at point n0000000 would give one line both of whose ends are point p0000000",
            Some("point"),
            vec!["n0000000", "a0000000", "b1000000", "p0000000"],
            None,
            Some("remove one of the lines instead")
        )
    );
}

#[test]
fn a_pieces_length_midpoint_or_equal_with_another_line_is_refused_naming_it() {
    let cause = "a piece's length, midpoint, or equal measures the piece, not the line a join makes: a split makes its curve's the outer ends' distance or symmetry, which the joined line keeps";
    let suggestion = "remove the constraint first, or dimension the outer ends instead";
    for (constraint, message, line) in [
        (
            r#"{"id": "k0000000", "type": "length", "line": "b1000000", "value": 24, "driving": true}"#,
            "joining lines a0000000 and b1000000 at point n0000000 would put constraint k0000000, line b1000000's length, on the whole joined line",
            "b1000000",
        ),
        (
            r#"{"id": "k0000000", "type": "length", "line": "a0000000", "value": 8, "driving": true}"#,
            "joining lines a0000000 and b1000000 at point n0000000 would put constraint k0000000, line a0000000's length, on the whole joined line",
            "a0000000",
        ),
        (
            r#"{"id": "k0000000", "type": "midpoint", "point": "s0000001", "line": "a0000000"}"#,
            "joining lines a0000000 and b1000000 at point n0000000 would put constraint k0000000, the midpoint of line a0000000, on the whole joined line",
            "a0000000",
        ),
        (
            r#"{"id": "k0000000", "type": "equal", "a": "e0000000", "b": "b1000000"}"#,
            "joining lines a0000000 and b1000000 at point n0000000 would put constraint k0000000, an equal of line b1000000 and e0000000, on the whole joined line",
            "b1000000",
        ),
    ] {
        let before = pieces("", "", constraint);
        assert_eq!(
            said(&refused(&before, "n0000000")),
            (
                message,
                Some("point"),
                vec!["k0000000", line],
                Some(cause),
                Some(suggestion)
            ),
            "{constraint}"
        );
    }
}

#[test]
fn arcs_with_no_centre_in_common_or_sweeping_a_turn_between_them_are_refused() {
    // a0 about c0 (0, 0) from (5, 0) to (0, 5); b0 about c1, another point at (0, 0), from
    // (0, 5) on: two centres at one place are two points, not one carrier.
    let two_centres = doc(
        &[
            point("c0000000", 0.0, 0.0),
            point("c0000001", 0.0, 0.0),
            point("r0000000", 5.0, 0.0),
            point("n0000000", 0.0, 5.0),
            point("r0000001", -5.0, 0.0),
        ]
        .join(", "),
        "",
        "",
        &[
            arc("a0000000", "c0000000", "r0000000", "n0000000"),
            arc("b0000000", "c0000001", "n0000000", "r0000001"),
        ]
        .join(", "),
        "",
    );
    assert_eq!(
        said(&refused(&two_centres, "n0000000")),
        (
            "arcs a0000000 and b0000000 meet at point n0000000, but have no centre in common, so they aren't held on one circle",
            Some("point"),
            vec!["n0000000", "a0000000", "b0000000"],
            None,
            Some("merge their centres first, then join")
        )
    );
    // a0 from (5, 0) round 270° to (0, −5), b0 on 180° to (0, 5): 450° between them.
    let over = doc(
        &[
            point("c0000000", 0.0, 0.0),
            point("r0000000", 5.0, 0.0),
            point("n0000000", 0.0, -5.0),
            point("r0000001", 0.0, 5.0),
        ]
        .join(", "),
        "",
        "",
        &[
            arc("a0000000", "c0000000", "r0000000", "n0000000"),
            arc("b0000000", "c0000000", "n0000000", "r0000001"),
        ]
        .join(", "),
        "",
    );
    assert_eq!(
        said(&refused(&over, "n0000000")),
        (
            "arcs a0000000 and b0000000 sweep a whole turn or more between them, so one arc can't join them at point n0000000",
            Some("point"),
            vec!["n0000000", "a0000000", "b0000000"],
            None,
            Some("trim one of them first, then join")
        )
    );
    // Both ending at the point: they overlap, and neither runs on.
    let both = doc(
        &[
            point("c0000000", 0.0, 0.0),
            point("r0000000", 5.0, 0.0),
            point("n0000000", 0.0, 5.0),
            point("r0000001", -5.0, 0.0),
        ]
        .join(", "),
        "",
        "",
        &[
            arc("a0000000", "c0000000", "r0000000", "n0000000"),
            arc("b0000000", "c0000000", "r0000001", "n0000000"),
        ]
        .join(", "),
        "",
    );
    assert_eq!(
        refused(&both, "n0000000").message(),
        "arcs a0000000 and b0000000 both end at point n0000000: a join takes a piece that ends there and one that starts there"
    );
}
