//! Hostile inputs for trimming a sketch's curve (`document::sketch_edit::trim`), from
//! break-it. The trim's contract (docs/ports/sketch-edit.md §4, decisions 2, 5, 6, and 8;
//! docs/design/m5.0-app.md, `sketch_trim`): a refusal as data naming the constraint and the
//! pick, or a sketch the validation gate takes in which the piece holding the pick's
//! projection is gone, cut at the nearest cuts either side. Expected values are worked by
//! hand (whole-number points on circles of radius 10 and 25), from FreeCAD 1.1.3 run
//! headless on the same drawing (cited where used), or, in the property tests, from a model
//! of the cuts written here (vertical cutters across a horizontal line, chords and radius
//! lines across a circle), the gate, and the solver's sent angles before and after.

use std::collections::BTreeMap;

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use silico_core::Error;
use silico_core::document::sketch_edit::{Trim, trim};
use silico_core::document::{
    Direction, Document, ElementId, Form, Operation, Reference, Sketch, read, validate,
};
use silico_core::sketch::Translation;

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

/// A document of one sketch, which the gate takes.
#[track_caller]
fn doc(points: &str, lines: &str, circles: &str, arcs: &str, constraints: &str) -> Document {
    let document = read(document(&fields(points, lines, circles, arcs, constraints)).as_bytes())
        .unwrap_or_else(|e| panic!("{e:?}"));
    if let Err(invalid) = validate(&document) {
        panic!("the input isn't gated: {:?}", invalid.errors());
    }
    document
}

fn sketch_of(document: &Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

/// The gate's refusals of `sketch` in place of the document's, as text (none if it passes).
fn gate(document: &Document, sketch: &Sketch) -> Vec<String> {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(sketch.clone());
    match validate(&document) {
        Ok(_) => Vec::new(),
        Err(invalid) => invalid.errors().iter().map(|e| e.to_string()).collect(),
    }
}

/// The trim's sketch, which the gate must take.
#[track_caller]
fn gated(before: &Document, done: &Trim) -> Sketch {
    let refused = gate(before, &done.edited.sketch);
    assert!(refused.is_empty(), "the gate refused the trim: {refused:?}");
    done.edited.sketch.clone()
}

/// `n` IDs for new members, none of which the tests' sketches hold.
fn fresh(n: usize) -> Vec<ElementId> {
    (0..n).map(|i| id(&format!("w{i:07}"))).collect()
}

/// The trim with as many fresh IDs as it asks for (up to 16): the count it refuses passed
/// over.
fn trim_any(sketch: &Sketch, curve: &str, pick: [f64; 2]) -> Result<Trim, Error> {
    let mut last = None;
    for n in 0..=16 {
        match trim(sketch, &id(curve), pick, &fresh(n)) {
            Err(e) if e.parameter() == Some("ids") && e.message().starts_with("the trim needs") => {
                last = Some(e);
            }
            other => return other,
        }
    }
    Err(last.unwrap())
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

#[track_caller]
fn xy(sketch: &Sketch, name: &str) -> [f64; 2] {
    let p = sketch
        .points
        .iter()
        .find(|p| p.id == id(name))
        .unwrap_or_else(|| panic!("no point {name}"));
    [p.x.mm(), p.y.mm()]
}

#[track_caller]
fn near(sketch: &Sketch, name: &str, expected: [f64; 2]) {
    let [x, y] = xy(sketch, name);
    assert!(
        (x - expected[0]).abs() < 1e-12 && (y - expected[1]).abs() < 1e-12,
        "{name} at ({x}, {y}), not {expected:?}"
    );
}

#[track_caller]
fn ends(sketch: &Sketch, name: &str) -> (String, String) {
    let line = sketch.lines.iter().find(|l| l.id == id(name)).unwrap();
    (line.from.to_string(), line.to.to_string())
}

#[track_caller]
fn arc_points(sketch: &Sketch, name: &str) -> [String; 3] {
    let arc = sketch
        .arcs
        .iter()
        .find(|a| a.id == id(name))
        .unwrap_or_else(|| panic!("no arc {name}"));
    [
        arc.center.to_string(),
        arc.start.to_string(),
        arc.end.to_string(),
    ]
}

fn on(point: &str, curve: &str) -> Form {
    Form::On {
        point: element(point),
        curve: element(curve),
    }
}

/// The angle sent to the solver for the constraint `name` (a tangent_on's or tangent_at's).
#[track_caller]
fn sent_angle(sketch: &Sketch, name: &str) -> f64 {
    let translation = Translation::new(sketch).unwrap_or_else(|e| panic!("{e:?}"));
    let mut sorted: Vec<&ElementId> = sketch.constraints.iter().map(|c| &c.id).collect();
    sorted.sort();
    let i = sorted.iter().position(|c| c.as_str() == name).unwrap();
    translation.sent_angle(i).unwrap()
}

/// A point on the circle of radius 10 about the origin at `degrees`.
fn on_ten(degrees: f64) -> [f64; 2] {
    let a = degrees.to_radians();
    [10.0 * libm::cos(a), 10.0 * libm::sin(a)]
}

// ------------------------------------------------------------------ cuts at a round's centre

/// Circle c (radius 10 about z, the origin); a radius line r from z up to (0, 20), crossing
/// the circle at (0, 10); and the chord d along x = 6, crossing it at (6, ±8).
fn circle_with_a_radius_line() -> Document {
    doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("r0000001", 0.0, 20.0),
            point("d0000001", 6.0, -20.0),
            point("d0000002", 6.0, 20.0),
        ]
        .join(", "),
        &[
            line("r0000000", "z0000000", "r0000001"),
            line("d0000000", "d0000001", "d0000002"),
        ]
        .join(", "),
        &circle("c0000000", "z0000000", 10.0),
        "",
        "",
    )
}

// Broke, before the fix: the radius line's end at the circle's centre is taken for "another curve's end on
// the curve": a round projects its centre onto itself (`Geo::project` keeps it where it is),
// so the distance is 0, and a circle spans every point, so a cut is made at the centre, at
// an angle of atan2(0, 0) = 0. Picked at (10, −1), just below angle 0, that cut is the
// nearest ahead, and the arc left starts at a new point at the centre, which the gate
// refuses ("arc c0000000's start ... is 0 mm from its center"). FreeCAD 1.1.3, the same
// drawing with r's start coincident with the centre, trimmed at (10, −1): the arc from
// 53.13° to 306.87°, (6, 8) to (6, −8).
#[test]
fn a_radius_line_from_a_circles_centre_cuts_it_only_at_the_rim() {
    let before = circle_with_a_radius_line();
    let done = trim(
        sketch_of(&before),
        &id("c0000000"),
        [10.0, -1.0],
        &ids(&["n0000000", "n0000001", "k0000000", "k0000001"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        arc_points(&sketch, "c0000000"),
        ["z0000000", "n0000000", "n0000001"]
    );
    near(&sketch, "n0000000", [6.0, 8.0]);
    near(&sketch, "n0000001", [6.0, -8.0]);
    assert_eq!(form(&sketch, "k0000000"), &on("n0000000", "d0000000"));
    assert_eq!(form(&sketch, "k0000001"), &on("n0000001", "d0000000"));
}

// Broke, before the fix: a pie slice. The two radius lines from the arc's centre each make a cut at the
// centre (as above), at the arc's start angle, which is behind the pick at 20°: the arc is
// cut in two, its first piece from (10, 0) to a new point at the centre, and the trim asks
// for 5 IDs. FreeCAD 1.1.3, the same drawing trimmed at 20°: the arc from 53.13° to 90°.
#[test]
fn a_pie_slices_radius_lines_dont_cut_its_arc_at_the_centre() {
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("s0000001", 10.0, 0.0),
            point("s0000002", 0.0, 10.0),
            point("d0000001", 6.0, 5.0),
            point("d0000002", 6.0, 12.0),
        ]
        .join(", "),
        &[
            line("m0000000", "z0000000", "s0000001"),
            line("m0000001", "z0000000", "s0000002"),
            line("d0000000", "d0000001", "d0000002"),
        ]
        .join(", "),
        "",
        &arc("a0000000", "z0000000", "s0000001", "s0000002"),
        "",
    );
    let done = trim(
        sketch_of(&before),
        &id("a0000000"),
        on_ten(20.0),
        &ids(&["n0000000", "k0000000"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        arc_points(&sketch, "a0000000"),
        ["z0000000", "n0000000", "s0000002"]
    );
    near(&sketch, "n0000000", [6.0, 8.0]);
    assert_eq!(form(&sketch, "k0000000"), &on("n0000000", "d0000000"));
    assert_eq!(done.pieces, ids(&["a0000000"]));
}

// ------------------------------------------------------------------ the pick at a cut

// Broke, before the fix: picked exactly at one of a circle's two cuts, the nearest cut behind and the
// nearest ahead are the same cut (both 0 away), so the circle "has fewer than two distinct
// cuts" and is removed whole. FreeCAD 1.1.3, a circle of radius 10 cut by x = 6, trimmed at
// (6, 8): the arc from 306.87° to 413.13°, (6, −8) to (6, 8), the part counter-clockwise
// after the pick removed (and so at (−6, 8) and (6, −8) with chords at x = ±6). A line
// picked at a cut keeps a piece too.
#[test]
fn a_circle_picked_exactly_at_a_cut_keeps_the_arc_behind_it_as_freecad_does() {
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("d0000001", 6.0, -20.0),
            point("d0000002", 6.0, 20.0),
        ]
        .join(", "),
        &line("d0000000", "d0000001", "d0000002"),
        &circle("c0000000", "z0000000", 10.0),
        "",
        "",
    );
    let done = trim(
        sketch_of(&before),
        &id("c0000000"),
        [6.0, 8.0],
        &ids(&["n0000000", "n0000001", "k0000000", "k0000001"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        arc_points(&sketch, "c0000000"),
        ["z0000000", "n0000000", "n0000001"]
    );
    near(&sketch, "n0000000", [6.0, -8.0]);
    near(&sketch, "n0000001", [6.0, 8.0]);
}

// ------------------------------------------------------------------ picks that overflow

/// Line a from (0, 0) to (32, 32), cut by d along x = 16 at (16, 16).
fn the_diagonal() -> Document {
    doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000004", 32.0, 32.0),
            point("d0000001", 16.0, 0.0),
            point("d0000002", 16.0, 32.0),
        ]
        .join(", "),
        &[
            line("a0000000", "p0000000", "p0000004"),
            line("d0000000", "d0000001", "d0000002"),
        ]
        .join(", "),
        "",
        "",
        "",
    )
}

// Broke, before the fix: a finite pick whose projection overflows: (1e308, −1e308) is square to the line
// at its start, so it projects onto (0, 0), and the start's piece goes; but the projection's
// dot product is 3.2e309 − 3.2e309, ∞ − ∞, so the pick's place along the line is NaN, no cut
// is behind it or ahead of it, and the line is "removed whole": refused as "the trim needs 0
// new IDs, got 2" (naming `ids`), and with none given, Ok with the line gone. Expected: the
// start trimmed back to (16, 16), or a refusal naming the pick (as split refuses one).
#[test]
fn a_finite_pick_whose_projection_overflows_is_trimmed_or_refused_naming_the_pick() {
    let before = the_diagonal();
    let sketch = sketch_of(&before);
    for pick in [[1e308, -1e308], [-f64::MAX, f64::MAX]] {
        match trim(
            sketch,
            &id("a0000000"),
            pick,
            &ids(&["n0000000", "k0000000"]),
        ) {
            Ok(done) => {
                let after = gated(&before, &done);
                assert_eq!(
                    ends(&after, "a0000000"),
                    ("n0000000".to_owned(), "p0000004".to_owned()),
                    "{pick:?}"
                );
                near(&after, "n0000000", [16.0, 16.0]);
            }
            Err(e) => {
                assert_eq!(e.parameter(), Some("pick"), "{pick:?}: {e:?}");
                assert!(e.suggestion().is_some(), "{e:?}");
            }
        }
        // With no IDs, it mustn't remove the line whole either.
        if let Ok(done) = trim(sketch, &id("a0000000"), pick, &[]) {
            panic!("{pick:?}: Ok with pieces {:?}", done.pieces);
        }
    }
}

// Broke, before the fix (low): a pick at (f64::MAX, f64::MAX), 45° from the circle's centre: its distance
// from the centre, hypot, overflows to ∞, the projection scales by r / ∞ = 0 and lands on
// the centre, and the trim takes the pick at angle 0, removing the piece about (25, 0)
// instead of the one about 45°. Expected: the piece between 16.26° and 53.13° removed, or the
// pick refused naming it.
#[test]
fn a_pick_whose_distance_from_the_centre_overflows_is_taken_at_its_own_angle_or_refused() {
    // Circle of radius 25; chords x = 24 at (24, ±7), ±16.26°, and x = 15 at (15, ±20),
    // ±53.13°.
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("d0000001", 24.0, -30.0),
            point("d0000002", 24.0, 30.0),
            point("e0000001", 15.0, -30.0),
            point("e0000002", 15.0, 30.0),
        ]
        .join(", "),
        &[
            line("d0000000", "d0000001", "d0000002"),
            line("e0000000", "e0000001", "e0000002"),
        ]
        .join(", "),
        &circle("c0000000", "z0000000", 25.0),
        "",
        "",
    );
    match trim(
        sketch_of(&before),
        &id("c0000000"),
        [f64::MAX, f64::MAX],
        &ids(&["n0000000", "n0000001", "k0000000", "k0000001"]),
    ) {
        Ok(done) => {
            let sketch = gated(&before, &done);
            assert_eq!(
                arc_points(&sketch, "c0000000"),
                ["z0000000", "n0000000", "n0000001"]
            );
            near(&sketch, "n0000000", [15.0, 20.0]);
            near(&sketch, "n0000001", [24.0, 7.0]);
        }
        Err(e) => assert_eq!(e.parameter(), Some("pick"), "{e:?}"),
    }
}

// ------------------------------------------------------------------ decision 5's shared end

/// Line a from (0, 0) to (32, 0); d crosses it at x = 16; e starts on it there, at e1
/// (16, 0), held on it by k0, and runs up to (26, 10). Two cuts at one place: d's crossing
/// and e's held end, d's ID first.
fn a_held_end_beside_a_crossing() -> Document {
    doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000004", 32.0, 0.0),
            point("d0000001", 16.0, -5.0),
            point("d0000002", 16.0, 5.0),
            point("e0000001", 16.0, 0.0),
            point("e0000002", 26.0, 10.0),
        ]
        .join(", "),
        &[
            line("a0000000", "p0000000", "p0000004"),
            line("d0000000", "d0000001", "d0000002"),
            line("e0000000", "e0000001", "e0000002"),
        ]
        .join(", "),
        "",
        "",
        r#"{"id": "k0000000", "type": "on", "point": "e0000001", "curve": "a0000000"}"#,
    )
}

#[test]
fn a_held_end_beside_a_crossing_is_shared_when_the_end_side_is_trimmed() {
    // Picked at 24: the cut behind is the last at 16, e's (cuts at one place go by cutter
    // ID); a ends at e1 and k0 goes as implied.
    let before = a_held_end_beside_a_crossing();
    let done = trim(sketch_of(&before), &id("a0000000"), [24.0, 0.0], &[]).unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("p0000000".to_owned(), "e0000001".to_owned())
    );
    assert_eq!(done.edited.removed, ids(&["p0000004", "k0000000"]));
}

// Broke, before the fix: the mirror image. Picked at 8, the cut ahead is the first at 16, d's crossing (d
// sorts before e), so a gets a new start point on d, and e1, which k0 holds on a exactly
// there, isn't shared (k0 stays, holding e1 on a's carrier at its new start): decision 5's
// share depends on which cutter's ID sorts first, and the trim asks for 2 IDs. Expected, as
// at 24: a from e1 to p4, k0 removed as implied, nothing added.
#[test]
fn a_held_end_beside_a_crossing_is_shared_when_the_start_side_is_trimmed() {
    let before = a_held_end_beside_a_crossing();
    let done = trim(sketch_of(&before), &id("a0000000"), [8.0, 0.0], &[]).unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("e0000001".to_owned(), "p0000004".to_owned())
    );
    assert_eq!(done.edited.removed, ids(&["p0000000", "k0000000"]));
}

#[test]
fn a_chords_ends_held_on_a_circle_are_the_arcs_ends_when_its_trimmed() {
    // The D: chord d from (6, −8) to (6, 8), both ends held on the circle; picked at (10, 0),
    // the minor arc goes and the arc left runs from d2 to d1, sharing both, the ons implied.
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("d0000001", 6.0, -8.0),
            point("d0000002", 6.0, 8.0),
        ]
        .join(", "),
        &line("d0000000", "d0000001", "d0000002"),
        &circle("c0000000", "z0000000", 10.0),
        "",
        r#"{"id": "k0000000", "type": "on", "point": "d0000001", "curve": "c0000000"}, {"id": "k0000001", "type": "on", "point": "d0000002", "curve": "c0000000"}"#,
    );
    let done = trim(sketch_of(&before), &id("c0000000"), [10.0, 0.0], &[]).unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        arc_points(&sketch, "c0000000"),
        ["z0000000", "d0000002", "d0000001"]
    );
    assert!(sketch.constraints.is_empty());
    assert_eq!(done.edited.removed, ids(&["k0000000", "k0000001"]));
    assert_eq!(done.edited.modified, ids(&["c0000000"]));
}

// ------------------------------------------------------------------ tangencies

// Broke, before the fix (low: the drawing doesn't meet its own tangent): a line through a circle with an
// edge tangent to it. Picked at the centre, both cuts are the circle's, and each join takes
// the tangency over; the first piece's end gets it as a tangent_on, and the second piece's
// new start is left on nothing at all (no on is made for a join with a tangency). Expected:
// every new end held on the cutter (an on or the tangent_on), or a refusal naming k0.
#[test]
fn a_tangent_cutter_crossing_twice_leaves_no_new_end_unheld() {
    let before = doc(
        &[
            point("p0000000", -20.0, 0.0),
            point("p0000004", 20.0, 0.0),
            point("z0000000", 0.0, 1.0),
        ]
        .join(", "),
        &line("a0000000", "p0000000", "p0000004"),
        &circle("c0000000", "z0000000", 10.0),
        "",
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "c0000000", "side": "left"}"#,
    );
    match trim_any(sketch_of(&before), "a0000000", [0.0, 0.0]) {
        Ok(done) => {
            let sketch = gated(&before, &done);
            let new_points: Vec<&ElementId> = done
                .edited
                .added
                .iter()
                .filter(|a| sketch.points.iter().any(|p| p.id == **a))
                .collect();
            assert_eq!(new_points.len(), 2);
            for p in new_points {
                let (point, cutter) = (Reference::Element(p.clone()), element("c0000000"));
                let held = sketch.constraints.iter().any(|c| match &c.form {
                    Form::On { point: q, curve } => *q == point && *curve == cutter,
                    Form::TangentOn { at, b, .. } => *at == point && *b == cutter,
                    _ => false,
                });
                assert!(
                    held,
                    "new end {p} is held on nothing: {:?}",
                    sketch.constraints
                );
            }
        }
        Err(e) => {
            assert_eq!(e.parameter(), Some("pick"), "{e:?}");
            assert!(e.elements().iter().any(|x| x == "k0000000"), "{e:?}");
        }
    }
}

#[test]
fn a_circle_trimmed_back_to_an_inner_circle_tangent_to_it_keeps_the_tangency_as_drawn() {
    // C, radius 10 about the origin; D, radius 5 about (5, 0), inside it and tangent at
    // (10, 0) (k0); the chord e along x = −6 at (−6, ±8). Picked at (0, 10): the arc left
    // runs from (−6, 8) round to the contact (10, 0), where k0 becomes a tangent_on of its
    // end on D. There the arc runs counter-clockwise, up, so it leaves its end downward,
    // against D's own direction (up): opposite, sent as an angle of 0 (both own directions
    // up).
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("y0000000", 5.0, 0.0),
            point("e0000001", -6.0, -20.0),
            point("e0000002", -6.0, 20.0),
        ]
        .join(", "),
        &line("e0000000", "e0000001", "e0000002"),
        &[
            circle("c0000000", "z0000000", 10.0),
            circle("d0000000", "y0000000", 5.0),
        ]
        .join(", "),
        "",
        r#"{"id": "k0000000", "type": "tangent", "a": "c0000000", "b": "d0000000", "side": "inside"}"#,
    );
    let done = trim(
        sketch_of(&before),
        &id("c0000000"),
        [0.0, 10.0],
        &ids(&["n0000000", "n0000001", "k0000001"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        arc_points(&sketch, "c0000000"),
        ["z0000000", "n0000000", "n0000001"]
    );
    near(&sketch, "n0000000", [-6.0, 8.0]);
    near(&sketch, "n0000001", [10.0, 0.0]);
    assert_eq!(
        form(&sketch, "k0000000"),
        &Form::TangentOn {
            a: element("c0000000"),
            at: element("n0000001"),
            b: element("d0000000"),
            direction: Direction::Opposite,
        }
    );
    assert_eq!(sent_angle(&sketch, "k0000000"), 0.0);
    assert_eq!(form(&sketch, "k0000001"), &on("n0000000", "e0000000"));
}

#[test]
fn a_cutters_end_held_tangent_on_the_line_keeps_its_angle_as_a_tangent_at_either_side() {
    // Arc r about (16, 10), radius 10, from r1 (16, 0) to (26, 10): it leaves r1 along +x,
    // the line's own direction, tangent_on `same` (sent as 0). Picked at 24 the line ends at
    // r1, at 8 it starts there; either way the tangency's angle is what it was.
    for (pick, piece_ends) in [
        ([24.0, 0.0], ("p0000000", "r0000001")),
        ([8.0, 0.0], ("r0000001", "p0000004")),
    ] {
        let before = doc(
            &[
                point("p0000000", 0.0, 0.0),
                point("p0000004", 32.0, 0.0),
                point("q0000000", 16.0, 10.0),
                point("r0000001", 16.0, 0.0),
                point("r0000002", 26.0, 10.0),
            ]
            .join(", "),
            &line("a0000000", "p0000000", "p0000004"),
            "",
            &arc("r0000000", "q0000000", "r0000001", "r0000002"),
            r#"{"id": "k0000000", "type": "tangent_on", "a": "r0000000", "at": "r0000001", "b": "a0000000", "direction": "same"}"#,
        );
        let was = sent_angle(sketch_of(&before), "k0000000");
        let done = trim(sketch_of(&before), &id("a0000000"), pick, &[]).unwrap();
        let sketch = gated(&before, &done);
        assert_eq!(
            ends(&sketch, "a0000000"),
            (piece_ends.0.to_owned(), piece_ends.1.to_owned())
        );
        assert!(matches!(form(&sketch, "k0000000"), Form::TangentAt { .. }));
        assert_eq!(sent_angle(&sketch, "k0000000"), was, "{pick:?}");
    }
}

// ------------------------------------------------------------------ what cuts

#[test]
fn a_construction_cutter_cuts_and_a_construction_lines_pieces_are_construction() {
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000004", 32.0, 0.0),
            point("d0000001", 8.0, -5.0),
            point("d0000002", 8.0, 5.0),
            point("e0000001", 24.0, -5.0),
            point("e0000002", 24.0, 5.0),
        ]
        .join(", "),
        &[
            line_as("a0000000", "p0000000", "p0000004", true),
            line_as("d0000000", "d0000001", "d0000002", true),
            line("e0000000", "e0000001", "e0000002"),
        ]
        .join(", "),
        "",
        "",
        "",
    );
    let done = trim(
        sketch_of(&before),
        &id("a0000000"),
        [16.0, 0.0],
        &ids(&["n0000000", "n0000001", "b0000000", "k0000000", "k0000001"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    near(&sketch, "n0000000", [8.0, 0.0]);
    near(&sketch, "n0000001", [24.0, 0.0]);
    assert!(
        sketch
            .lines
            .iter()
            .find(|l| l.id == id("b0000000"))
            .unwrap()
            .construction
    );
    assert!(
        sketch
            .points
            .iter()
            .filter(|p| p.id.as_str().starts_with('n'))
            .all(|p| p.construction)
    );
}

#[test]
fn an_arc_on_the_same_circle_cuts_at_its_ends() {
    // The upper half of the circle of radius 25, and a cutter arc about the same centre
    // from (20, 15) to (−20, 15): an overlap, cut at its ends (`3cbe309330`); picked at the
    // top, two arcs are left, sharing the centre.
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("e0000000", 25.0, 0.0),
            point("e0000001", -25.0, 0.0),
            point("f0000001", 20.0, 15.0),
            point("f0000002", -20.0, 15.0),
        ]
        .join(", "),
        "",
        "",
        &[
            arc("a0000000", "z0000000", "e0000000", "e0000001"),
            arc("f0000000", "z0000000", "f0000001", "f0000002"),
        ]
        .join(", "),
        "",
    );
    let done = trim(
        sketch_of(&before),
        &id("a0000000"),
        [0.0, 25.0],
        &ids(&["n0000000", "n0000001", "b0000000", "k0000000", "k0000001"]),
    )
    .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        arc_points(&sketch, "a0000000"),
        ["z0000000", "e0000000", "n0000000"]
    );
    assert_eq!(
        arc_points(&sketch, "b0000000"),
        ["z0000000", "n0000001", "e0000001"]
    );
    near(&sketch, "n0000000", [20.0, 15.0]);
    near(&sketch, "n0000001", [-20.0, 15.0]);
}

#[test]
fn a_half_kilometre_line_cut_1_5e_7_from_its_end_leaves_that_sliver() {
    // At 5e5 mm a coordinate's ulp is 6e-11: a cut 1.5e-7 from the end is one, and the
    // piece left after trimming the rest is 1.5e-7 long; one 5e-8 from it isn't.
    for (gap, cuts) in [(1.5e-7, true), (5e-8, false)] {
        let x = 5e5 - gap;
        let before = doc(
            &[
                point("p0000000", 0.0, 0.0),
                point("p0000004", 5e5, 0.0),
                point("d0000001", x, -5.0),
                point("d0000002", x, 5.0),
            ]
            .join(", "),
            &[
                line("a0000000", "p0000000", "p0000004"),
                line("d0000000", "d0000001", "d0000002"),
            ]
            .join(", "),
            "",
            "",
            "",
        );
        let done = trim_any(sketch_of(&before), "a0000000", [1e5, 0.0]).unwrap();
        let sketch = gated(&before, &done);
        if cuts {
            let (from, to) = ends(&sketch, "a0000000");
            assert_eq!(to, "p0000004");
            let [fx, fy] = xy(&sketch, &from);
            assert!((fx - x).abs() < 1e-9 && fy == 0.0, "({fx}, {fy})");
        } else {
            assert!(sketch.lines.iter().all(|l| l.id != id("a0000000")));
        }
    }
}

// Broke, before the fix: the cut at (0, 25) is 2e-7 from the stored end, so it isn't "the curve's own
// end", but it's at the end's angle: the arc left runs from (0, 25) to (0, 25.0000002), a
// sweep of 0 (and the gate takes it: it refuses an arc's ends at one place, not at one
// angle). FreeCAD's own-end test is in the curve's parameter (`226d24792d`, 1e-8), which
// this cut is at.
#[test]
fn an_arc_whose_end_is_drawn_off_its_circle_trimmed_back_to_a_cut_at_the_ends_angle() {
    // The gate takes an arc whose end isn't on its circle (the solve mends it): a about the
    // origin from (25, 0) to (0, 25.0000002), 2e-7 outside its radius of 25, at 90°. The
    // cutter x = 0 crosses the circle at (0, 25), at the end's angle, 2e-7 from the end
    // (more than the 1e-7 that makes it the end). Picked at 45°, the arc is trimmed back to
    // that cut; what's left must be an arc the gate takes, or the trim refused naming the
    // pick, not an arc from (0, 25) to (0, 25.0000002) with no sweep.
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("e0000000", 25.0, 0.0),
            point("e0000001", 0.0, 25.0000002),
            point("d0000001", 0.0, 20.0),
            point("d0000002", 0.0, 30.0),
        ]
        .join(", "),
        &line("d0000000", "d0000001", "d0000002"),
        "",
        &arc("a0000000", "z0000000", "e0000000", "e0000001"),
        "",
    );
    match trim_any(sketch_of(&before), "a0000000", [17.0, 17.0]) {
        Ok(done) => {
            let sketch = gated(&before, &done);
            for a in &sketch.arcs {
                let [c, s, e] = [
                    xy(&sketch, a.center.as_str()),
                    xy(&sketch, a.start.as_str()),
                    xy(&sketch, a.end.as_str()),
                ];
                let sweep = (angle_of([e[0] - c[0], e[1] - c[1]])
                    - angle_of([s[0] - c[0], s[1] - c[1]]))
                .rem_euclid(std::f64::consts::TAU);
                assert!(
                    sweep > 1e-9,
                    "arc {} from {s:?} to {e:?} has a sweep of {sweep}",
                    a.id
                );
            }
        }
        Err(e) => assert_eq!(e.parameter(), Some("pick"), "{e:?}"),
    }
}

#[test]
fn a_trim_of_a_line_with_ten_thousand_constraints_is_prompt() {
    // 4,999 edge tangents of the line with a circle resting on it, and 4,998 ons of a lone
    // point on the cutter: each tangent is checked against every constraint for an end held
    // on the other (the gate's tangent_on rule). Picked at 8, the start goes back to d at 16
    // (the circle's contact at 20 is ahead of it), one new point and its on: 9,999.
    let mut constraints = Vec::new();
    for i in 0..4999 {
        constraints.push(format!(
            r#"{{"id": "t{i:07}", "type": "tangent", "a": "a0000000", "b": "c0000000", "side": "left"}}"#
        ));
    }
    for i in 0..4998 {
        constraints.push(format!(
            r#"{{"id": "k{i:07}", "type": "on", "point": "s0000000", "curve": "d0000000"}}"#
        ));
    }
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000004", 32.0, 0.0),
            point("d0000001", 16.0, -5.0),
            point("d0000002", 16.0, 5.0),
            point("q0000000", 20.0, 5.0),
            point("s0000000", 16.0, 3.0),
        ]
        .join(", "),
        &[
            line("a0000000", "p0000000", "p0000004"),
            line("d0000000", "d0000001", "d0000002"),
        ]
        .join(", "),
        &circle("c0000000", "q0000000", 5.0),
        "",
        &constraints.join(", "),
    );
    let sketch = sketch_of(&before).clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(trim(
            &sketch,
            &id("a0000000"),
            [8.0, 0.0],
            &ids(&["n0000000", "n0000001"]),
        ));
    });
    let done = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the trim took more than a minute")
        .unwrap();
    let sketch = gated(&before, &done);
    assert_eq!(
        ends(&sketch, "a0000000"),
        ("n0000000".to_owned(), "p0000004".to_owned())
    );
    assert_eq!(sketch.constraints.len(), 9998);
}

// ------------------------------------------------------------------ property tests

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x7_21_3b),
        ..ProptestConfig::default()
    }
}

/// Offsets at and either side of the 1e-7 mm contact tolerance, and a few clear of it.
const GAPS: [f64; 9] = [0.0, 5e-8, -5e-8, 1.5e-7, -1.5e-7, 2e-7, -2e-7, 3.0, -3.0];

/// A vertical cutter across the line y = 0 from (0, 0) to (40, 0): its x, and its y range.
#[derive(Debug, Clone, Copy)]
struct Upright {
    x: f64,
    low: f64,
    high: f64,
}

fn upright() -> impl Strategy<Value = Upright> {
    (
        prop_oneof![
            (1u32..8, 0usize..5)
                .prop_map(|(k, g)| f64::from(k) * 5.0 + [0.0, 3e-8, -4e-8, 1.5e-7, 0.25][g]),
            (0usize..4).prop_map(|g| [5e-8, 1.5e-7, 40.0 - 5e-8, 40.0 - 1.5e-7][g]),
        ],
        0usize..GAPS.len(),
        0usize..3,
        any::<bool>(),
    )
        .prop_map(|(x, g, h, up)| {
            // An end at y = gap, the other 5 or 10 away, up or down.
            let (end, other) = (GAPS[g], GAPS[g] + [5.0, 10.0, -5.0][h]);
            let (low, high) = if up { (end, other) } else { (other, end) };
            Upright {
                x,
                low: low.min(high),
                high: low.max(high),
            }
        })
}

/// Where the model says an upright cuts the line: where it crosses y = 0, or where an end
/// is within 1e-7 of it; none within 1e-7 of the line's ends.
fn model_cut(u: &Upright) -> Option<f64> {
    let tol = 1e-7;
    let reaches = (u.low <= 0.0 && 0.0 <= u.high) || u.low.abs() <= tol || u.high.abs() <= tol;
    (reaches && u.x > tol && u.x < 40.0 - tol).then_some(u.x)
}

/// The line and its uprights, as a document, and the uprights' IDs.
fn the_line_with(uprights: &[Upright]) -> Option<Document> {
    let mut points = vec![point("e0000000", 0.0, 0.0), point("e0000001", 40.0, 0.0)];
    let mut lines = vec![line("a0000000", "e0000000", "e0000001")];
    for (i, u) in uprights.iter().enumerate() {
        points.push(point(&format!("v{i:07}"), u.x, u.low));
        points.push(point(&format!("y{i:07}"), u.x, u.high));
        lines.push(line(
            &format!("m{i:07}"),
            &format!("v{i:07}"),
            &format!("y{i:07}"),
        ));
    }
    let d = read(document(&fields(&points.join(", "), &lines.join(", "), "", "", "")).as_bytes())
        .ok()?;
    validate(&d).ok()?;
    Some(d)
}

/// A circle of radius 25 about the origin, and its cutters: chords along x = k (whole-number
/// points (k, ±√(625 − k²))), each over a y range, and lines from the centre out through a
/// whole-number point of the circle, or short of it.
#[derive(Debug, Clone, Copy)]
enum RoundCutter {
    Chord { k: usize, low: f64, high: f64 },
    Spoke { at: usize, out: bool },
}

/// Whole-number points of the circle of radius 25, counter-clockwise from 0°.
const ROUND: [(f64, f64); 20] = [
    (25.0, 0.0),
    (24.0, 7.0),
    (20.0, 15.0),
    (15.0, 20.0),
    (7.0, 24.0),
    (0.0, 25.0),
    (-7.0, 24.0),
    (-15.0, 20.0),
    (-20.0, 15.0),
    (-24.0, 7.0),
    (-25.0, 0.0),
    (-24.0, -7.0),
    (-20.0, -15.0),
    (-15.0, -20.0),
    (-7.0, -24.0),
    (0.0, -25.0),
    (7.0, -24.0),
    (15.0, -20.0),
    (20.0, -15.0),
    (24.0, -7.0),
];

/// Chords' x: each meets the circle at two of [`ROUND`]'s points.
const CHORDS: [f64; 6] = [24.0, 20.0, 7.0, -7.0, -15.0, -24.0];

fn round_cutter() -> impl Strategy<Value = RoundCutter> {
    prop_oneof![
        (0usize..CHORDS.len(), 0usize..4, 0usize..4).prop_map(|(k, a, b)| {
            let ys: [f64; 4] = [-30.0, -10.0, 10.0, 30.0];
            let (low, high) = (ys[a].min(ys[b]), ys[a].max(ys[b]));
            RoundCutter::Chord {
                k,
                low,
                high: if low == high { high + 50.0 } else { high },
            }
        }),
        (0usize..ROUND.len(), any::<bool>()).prop_map(|(at, out)| RoundCutter::Spoke { at, out }),
    ]
}

/// The model's cuts of a round cutter on the circle: the [`ROUND`] indices it crosses at.
fn round_cuts(c: &RoundCutter) -> Vec<usize> {
    match *c {
        RoundCutter::Chord { k, low, high } => ROUND
            .iter()
            .enumerate()
            .filter(|(_, (x, y))| *x == CHORDS[k] && low <= *y && *y <= high)
            .map(|(i, _)| i)
            .collect(),
        RoundCutter::Spoke { at, out } => {
            if out {
                vec![at]
            } else {
                vec![]
            }
        }
    }
}

fn the_circle_with(cutters: &[RoundCutter]) -> Option<Document> {
    let mut points = vec![point("z0000000", 0.0, 0.0)];
    let mut lines = vec![];
    for (i, c) in cutters.iter().enumerate() {
        let (from, to) = (format!("v{i:07}"), format!("y{i:07}"));
        match *c {
            RoundCutter::Chord { k, low, high } => {
                points.push(point(&from, CHORDS[k], low));
                points.push(point(&to, CHORDS[k], high));
                lines.push(line(&format!("m{i:07}"), &from, &to));
            }
            RoundCutter::Spoke { at, out } => {
                let (x, y) = ROUND[at];
                let s = if out { 1.2 } else { 0.6 };
                points.push(point(&to, x * s, y * s));
                lines.push(line(&format!("m{i:07}"), "z0000000", &to));
            }
        }
    }
    let d = read(
        document(&fields(
            &points.join(", "),
            &lines.join(", "),
            &circle("c0000000", "z0000000", 25.0),
            "",
            "",
        ))
        .as_bytes(),
    )
    .ok()?;
    validate(&d).ok()?;
    Some(d)
}

fn angle_of(p: [f64; 2]) -> f64 {
    libm::atan2(p[1], p[0]).rem_euclid(std::f64::consts::TAU)
}

/// Each tangent_on's and tangent_at's sent angle, by ID.
fn angles(sketch: &Sketch) -> BTreeMap<String, f64> {
    let Ok(translation) = Translation::new(sketch) else {
        return BTreeMap::new();
    };
    let mut sorted: Vec<&ElementId> = sketch.constraints.iter().map(|c| &c.id).collect();
    sorted.sort();
    sorted
        .iter()
        .enumerate()
        .filter_map(|(i, c)| translation.sent_angle(i).map(|a| (c.to_string(), a)))
        .collect()
}

proptest! {
    #![proptest_config(config(1500))]

    /// Uprights across a line: the line left is what the model says, each new end on its
    /// cutter, and the gate takes it.
    #[test]
    fn a_line_cut_by_uprights_keeps_what_the_model_says(
        uprights in prop::collection::vec(upright(), 0..=5),
        pick_k in 0u32..9,
        pick_off in 0usize..3,
    ) {
        let Some(before) = the_line_with(&uprights) else { return Ok(()) };
        let pick = f64::from(pick_k) * 5.0 + [2.5, 0.1, -0.1][pick_off];
        let mut cuts: Vec<f64> = uprights.iter().filter_map(model_cut).collect();
        cuts.sort_by(f64::total_cmp);
        let behind = cuts.iter().rev().find(|c| **c <= pick).copied();
        let ahead = cuts.iter().find(|c| **c > pick).copied();
        let done = trim_any(sketch_of(&before), "a0000000", [pick, 0.0]);
        let done = match done {
            Ok(done) => done,
            Err(e) => return Err(TestCaseError::fail(format!("refused: {e:?}"))),
        };
        prop_assert_eq!(gate(&before, &done.edited.sketch), Vec::<String>::new());
        let sketch = &done.edited.sketch;
        let mut left: Vec<(f64, f64)> = done.pieces.iter().map(|p| {
            let l = sketch.lines.iter().find(|l| l.id == *p).unwrap();
            (xy(sketch, l.from.as_str())[0], xy(sketch, l.to.as_str())[0])
        }).collect();
        left.sort_by(|a, b| a.0.total_cmp(&b.0));
        let expected: Vec<(f64, f64)> = match (behind, ahead) {
            (None, None) => vec![],
            (Some(b), Some(a)) if a - b <= 1e-7 => vec![],
            (b, a) => b.map(|b| (0.0, b)).into_iter().chain(a.map(|a| (a, 40.0))).collect(),
        };
        prop_assert_eq!(left.len(), expected.len(), "cuts {:?}, pick {}: left {:?}", cuts, pick, left);
        for (got, want) in left.iter().zip(&expected) {
            prop_assert!((got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9,
                "cuts {:?}, pick {}: left {:?}", cuts, pick, left);
        }
    }

    /// Chords and spokes across a circle: the arc left is what the model says.
    // Broke, before the fix: a spoke from the centre makes a cut at the centre, at angle 0, so with one rim
    // cut beside it the circle isn't removed whole but left as an arc ending at its centre
    // (the targeted a_radius_line_from_a_circles_centre_cuts_it_only_at_the_rim is the same).
    #[test]
    fn a_circle_cut_by_chords_and_spokes_keeps_what_the_model_says(
        cutters in prop::collection::vec(round_cutter(), 0..=4),
        pick_at in 0usize..ROUND.len(),
    ) {
        circle_as_the_model_says(&cutters, pick_at)?;
    }

    /// The same with chords alone: no line from the centre.
    #[test]
    fn a_circle_cut_by_chords_keeps_what_the_model_says(
        cutters in prop::collection::vec(round_cutter(), 0..=4)
            .prop_map(|c| c.into_iter().filter(|c| matches!(c, RoundCutter::Chord { .. })).collect::<Vec<_>>()),
        pick_at in 0usize..ROUND.len(),
    ) {
        circle_as_the_model_says(&cutters, pick_at)?;
    }
}

/// A circle of radius 25 cut by `cutters`, picked 0.05 rad past one of [`ROUND`]'s points:
/// the arc left runs from the nearest cut ahead of the pick round to the nearest behind it,
/// as the model says; with fewer than two cuts, the circle goes whole.
fn circle_as_the_model_says(cutters: &[RoundCutter], pick_at: usize) -> Result<(), TestCaseError> {
    let Some(before) = the_circle_with(cutters) else {
        return Ok(());
    };
    let (px, py) = ROUND[pick_at];
    let pick_angle = angle_of([px, py]) + 0.05;
    let pick = [25.0 * libm::cos(pick_angle), 25.0 * libm::sin(pick_angle)];
    let mut cut_at: Vec<usize> = cutters.iter().flat_map(round_cuts).collect();
    cut_at.sort_unstable();
    cut_at.dedup();
    let done = match trim_any(sketch_of(&before), "c0000000", pick) {
        Ok(done) => done,
        Err(e) => return Err(TestCaseError::fail(format!("refused: {e:?}"))),
    };
    prop_assert_eq!(gate(&before, &done.edited.sketch), Vec::<String>::new());
    let sketch = &done.edited.sketch;
    if cut_at.len() < 2 {
        prop_assert!(
            done.pieces.is_empty(),
            "cuts {:?}: {:?}",
            cut_at,
            done.pieces
        );
        return Ok(());
    }
    let angle = |i: usize| angle_of([ROUND[i].0, ROUND[i].1]);
    let behind = |i: &usize| (pick_angle - angle(*i)).rem_euclid(std::f64::consts::TAU);
    let ahead = |i: &usize| (angle(*i) - pick_angle).rem_euclid(std::f64::consts::TAU);
    let before_cut = *cut_at
        .iter()
        .min_by(|a, b| behind(a).total_cmp(&behind(b)))
        .unwrap();
    let after_cut = *cut_at
        .iter()
        .min_by(|a, b| ahead(a).total_cmp(&ahead(b)))
        .unwrap();
    let [_, start, end] = arc_points(sketch, "c0000000");
    let (s, e) = (xy(sketch, &start), xy(sketch, &end));
    let close = |p: [f64; 2], i: usize| {
        (p[0] - ROUND[i].0).abs() < 1e-9 && (p[1] - ROUND[i].1).abs() < 1e-9
    };
    prop_assert!(
        close(s, after_cut) && close(e, before_cut),
        "cuts at {:?}, pick {:.3} rad: arc from {:?} to {:?}, not {:?} to {:?}",
        cut_at,
        pick_angle,
        s,
        e,
        ROUND[after_cut],
        ROUND[before_cut]
    );
    Ok(())
}

// ------------------------------------------------------------------ any gated sketch

/// The curve trimmed: from (0, 0) to (40, 0), from (0, 0) to (30, 40), the arc about
/// (0, 0) of radius 25 from (25, 0) to (−15, −20), or the circle of radius 25.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Line,
    Tilted,
    Arc,
    Circle,
}

impl Target {
    fn stations(self) -> Vec<[f64; 2]> {
        match self {
            Target::Line => (1..8).map(|k| [5.0 * f64::from(k), 0.0]).collect(),
            Target::Tilted => (1..10)
                .map(|k| [3.0 * f64::from(k), 4.0 * f64::from(k)])
                .collect(),
            Target::Arc | Target::Circle => ROUND.iter().map(|&(x, y)| [x, y]).collect(),
        }
    }

    fn normal(self, p: [f64; 2]) -> [f64; 2] {
        match self {
            Target::Line => [0.0, 1.0],
            Target::Tilted => [0.8, -0.6],
            Target::Arc | Target::Circle => {
                let r = libm::hypot(p[0], p[1]);
                [p[0] / r, p[1] / r]
            }
        }
    }

    /// How far `p` is from the carrier: the line, or the circle.
    fn off(self, p: [f64; 2]) -> f64 {
        match self {
            Target::Line => p[1].abs(),
            Target::Tilted => (0.8 * p[0] - 0.6 * p[1]).abs(),
            Target::Arc | Target::Circle => (libm::hypot(p[0], p[1]) - 25.0).abs(),
        }
    }

    /// Where `p` projects onto the carrier, as a length along it (a line's from its
    /// start, a round's as an arc length counter-clockwise from 0°).
    fn along(self, p: [f64; 2]) -> f64 {
        match self {
            Target::Line => p[0],
            Target::Tilted => 0.6 * p[0] + 0.8 * p[1],
            Target::Arc | Target::Circle => 25.0 * angle_of(p),
        }
    }

    fn round(self) -> bool {
        matches!(self, Target::Arc | Target::Circle)
    }
}

/// Places near the trimmed curve: a station, moved along and off it by a gap.
const NUDGE: [f64; 8] = [0.0, 5e-8, 1e-7, 1.5e-7, 2e-7, 1e-3, 0.5, 3.0];

/// Points off the curve for cutters to run to, and centres for arcs and circles.
const GRID: [(f64, f64); 6] = [
    (10.0, 30.0),
    (-10.0, -40.0),
    (50.0, 10.0),
    (35.0, -5.0),
    (20.0, -20.0),
    (5.0, 20.0),
];

#[derive(Debug, Clone)]
struct Recipe {
    target: Target,
    /// Points near the curve: a station, a nudge along, a nudge off, a side; held on it?
    near: Vec<(usize, usize, usize, bool, bool)>,
    /// Lines between pool points.
    lines: Vec<(usize, usize)>,
    /// Arcs about a centre (the target's, or a grid point), between pool points.
    arcs: Vec<(usize, usize, usize)>,
    /// Circles about a centre, of a radius.
    circles: Vec<(usize, usize)>,
    /// A form, three references, a choice.
    constraints: Vec<(u8, usize, usize, usize, u8)>,
    /// The pick: a station, a nudge along, a nudge off, a side.
    pick: (usize, usize, usize, bool),
}

fn recipe() -> impl Strategy<Value = Recipe> {
    (
        prop_oneof![
            Just(Target::Line),
            Just(Target::Tilted),
            Just(Target::Arc),
            Just(Target::Circle)
        ],
        prop::collection::vec(
            (
                0usize..20,
                0usize..NUDGE.len(),
                0usize..NUDGE.len(),
                any::<bool>(),
                any::<bool>(),
            ),
            0..=4,
        ),
        prop::collection::vec((0usize..16, 0usize..16), 0..=4),
        prop::collection::vec((0usize..8, 0usize..16, 0usize..16), 0..=2),
        prop::collection::vec((0usize..8, 0usize..6), 0..=2),
        prop::collection::vec((0u8..12, 0usize..32, 0usize..32, 0usize..32, 0u8..4), 0..=8),
        (
            0usize..20,
            0usize..NUDGE.len(),
            0usize..NUDGE.len(),
            any::<bool>(),
        ),
    )
        .prop_map(
            |(target, near, lines, arcs, circles, constraints, pick)| Recipe {
                target,
                near,
                lines,
                arcs,
                circles,
                constraints,
                pick,
            },
        )
}

fn nudged(t: Target, station: usize, along: usize, off: usize, side: bool) -> [f64; 2] {
    let st = t.stations();
    let base = st[station % st.len()];
    let n = t.normal(base);
    let a = [-n[1], n[0]];
    let s = if side { 1.0 } else { -1.0 };
    let (da, dn) = (s * NUDGE[along], s * NUDGE[off]);
    [
        base[0] + a[0] * da + n[0] * dn,
        base[1] + a[1] * da + n[1] * dn,
    ]
}

/// The recipe's sketch, every constraint the gate refuses left out; none if its geometry
/// is refused.
fn build(recipe: &Recipe) -> Option<Document> {
    let t = recipe.target;
    let mut points = vec![];
    let mut lines = vec![];
    let mut circles = vec![];
    let mut arcs = vec![];
    // The centre, for rounds and as a point lines may start at.
    points.push(point("e0000002", 0.0, 0.0));
    let mut pool: Vec<String> = vec!["e0000002".to_owned()];
    match t {
        Target::Line | Target::Tilted => {
            let end = if t == Target::Line {
                [40.0, 0.0]
            } else {
                [30.0, 40.0]
            };
            points.push(point("e0000001", end[0], end[1]));
            lines.push(line("a0000000", "e0000002", "e0000001"));
            pool.push("e0000001".to_owned());
        }
        Target::Arc => {
            points.push(point("e0000000", 25.0, 0.0));
            points.push(point("e0000001", -15.0, -20.0));
            arcs.push(arc("a0000000", "e0000002", "e0000000", "e0000001"));
            pool.extend(["e0000000".to_owned(), "e0000001".to_owned()]);
        }
        Target::Circle => circles.push(circle("a0000000", "e0000002", 25.0)),
    }
    let mut held = vec![];
    for (i, &(st, along, off, side, hold)) in recipe.near.iter().enumerate() {
        let p = nudged(t, st, along, off, side);
        let pid = format!("s{i:07}");
        points.push(point(&pid, p[0], p[1]));
        pool.push(pid.clone());
        if hold {
            held.push(format!(
                r#"{{"id": "h{i:07}", "type": "on", "point": "{pid}", "curve": "a0000000"}}"#
            ));
        }
    }
    for (i, &(x, y)) in GRID.iter().enumerate() {
        let g = format!("g{i:07}");
        points.push(point(&g, x, y));
        pool.push(g);
    }
    let p = |i: usize| pool[i % pool.len()].clone();
    let mut curves = vec!["a0000000".to_owned()];
    let mut ends: Vec<(String, String)> = vec![];
    for (i, &(a, b)) in recipe.lines.iter().enumerate() {
        // Mostly from a point near the curve (or the centre) out to another.
        let from = if recipe.near.is_empty() || a % 3 == 2 {
            p(a)
        } else {
            format!("s{:07}", a % recipe.near.len())
        };
        let to = p(b);
        // A line from a round's centre makes a cut there (the targeted
        // a_radius_line_from_a_circles_centre_cuts_it_only_at_the_rim): left out, so the
        // property tests look past it.
        if from != to && !(t.round() && (from == "e0000002" || to == "e0000002")) {
            let lid = format!("m{i:07}");
            lines.push(line(&lid, &from, &to));
            ends.push((lid.clone(), from));
            ends.push((lid.clone(), to));
            curves.push(lid);
        }
    }
    for (i, &(c, s, e)) in recipe.arcs.iter().enumerate() {
        let center = if c == 0 {
            "e0000002".to_owned()
        } else {
            format!("g{:07}", c % GRID.len())
        };
        let (s, e) = (p(s), p(e));
        if s != e
            && s != center
            && e != center
            && !(t.round() && (s == "e0000002" || e == "e0000002"))
        {
            let aid = format!("r{i:07}");
            arcs.push(arc(&aid, &center, &s, &e));
            ends.push((aid.clone(), s));
            ends.push((aid.clone(), e));
            curves.push(aid);
        }
    }
    // Circles: about a grid point, or placed tangent to the curve (within a nudge).
    let tangent_centres: [(f64, f64); 2] = match t {
        Target::Line => [(20.0, 5.0), (10.0, -5.0)],
        Target::Tilted => [(19.0, 17.0), (5.0, 12.0)],
        Target::Arc | Target::Circle => [(30.0, 0.0), (0.0, 20.0)],
    };
    for (i, &(c, r)) in recipe.circles.iter().enumerate() {
        let cid = format!("c{i:07}");
        let (center, radius) = if c < 2 {
            let pid = format!("q{i:07}");
            let (x, y) = tangent_centres[c];
            points.push(point(&pid, x, y));
            (pid, 5.0 + [0.0, 5e-8, -5e-8, 2e-7, -2e-7, 1.0][r])
        } else {
            (
                format!("g{:07}", c % GRID.len()),
                [5.0, 10.0, 25.0, 30.0, 12.5, 40.0][r],
            )
        };
        circles.push(circle(&cid, &center, radius));
        curves.push(cid);
    }
    let text = |constraints: &[String]| {
        document(&fields(
            &points.join(", "),
            &lines.join(", "),
            &circles.join(", "),
            &arcs.join(", "),
            &constraints.join(", "),
        ))
    };
    let geometry = read(text(&[]).as_bytes()).ok()?;
    validate(&geometry).ok()?;
    let mut kept: Vec<String> = vec![];
    let try_add = |json: String, kept: &mut Vec<String>| {
        let mut trial = kept.clone();
        trial.push(json);
        if read(text(&trial).as_bytes()).is_ok_and(|d| validate(&d).is_ok()) {
            *kept = trial;
        }
    };
    for json in held {
        try_add(json, &mut kept);
    }
    let mut curve_pool = curves.clone();
    curve_pool.extend(["x_axis".to_owned(), "y_axis".to_owned()]);
    for (k, &(f, r1, r2, r3, choice)) in recipe.constraints.iter().enumerate() {
        let c = |r: usize| {
            if r.is_multiple_of(2) {
                "a0000000".to_owned()
            } else {
                curve_pool[r / 2 % curve_pool.len()].clone()
            }
        };
        let side = ["left", "right"][usize::from(choice % 2)];
        let tside = ["left", "right", "outside", "inside"][usize::from(choice % 4)];
        let dir = ["same", "opposite"][usize::from(choice % 2)];
        let body = match f {
            0 => format!(r#""type": "horizontal", "line": "{}""#, c(r1)),
            1 => format!(
                r#""type": "length", "line": "{}", "value": 40, "driving": true"#,
                c(r1)
            ),
            2 => format!(
                r#""type": "point_line_distance", "point": "{}", "line": "{}", "value": 5, "side": "{side}", "driving": true"#,
                p(r1),
                c(r2)
            ),
            3 => format!(
                r#""type": "on", "point": "{}", "curve": "{}""#,
                p(r1),
                c(r2)
            ),
            4 => format!(r#""type": "parallel", "a": "{}", "b": "{}""#, c(r1), c(r2)),
            5 => format!(r#""type": "equal", "a": "{}", "b": "{}""#, c(r1), c(r2)),
            6 => format!(
                r#""type": "tangent", "a": "{}", "b": "{}", "side": "{tside}""#,
                c(r1),
                c(r2)
            ),
            7 if !ends.is_empty() => {
                // A cutter's end held tangent on the trimmed curve.
                let (a, at) = &ends[r1 % ends.len()];
                format!(
                    r#""type": "tangent_on", "a": "{a}", "at": "{at}", "b": "{}", "direction": "{dir}""#,
                    c(r3)
                )
            }
            8 if !ends.is_empty() => {
                let (b, at) = &ends[r1 % ends.len()];
                format!(
                    r#""type": "tangent_at", "a": "a0000000", "b": "{b}", "at": "{at}", "direction": "{dir}""#
                )
            }
            9 => format!(
                r#""type": "symmetric", "a": "{}", "b": "{}", "about": "{}""#,
                p(r1),
                p(r2),
                c(r3)
            ),
            10 => format!(
                r#""type": "midpoint", "point": "{}", "line": "{}""#,
                p(r1),
                c(r2)
            ),
            _ => format!(
                r#""type": "distance", "a": "{}", "b": "{}", "value": 7, "driving": true"#,
                p(r1),
                p(r2)
            ),
        };
        try_add(format!(r#"{{"id": "k{k:07}", {body}}}"#), &mut kept);
    }
    read(text(&kept).as_bytes()).ok()
}

/// A recipe's sketch, its pick, and its trim.
fn trim_of(recipe: &Recipe) -> Option<(Document, [f64; 2], Result<Trim, Error>)> {
    let before = build(recipe)?;
    let (st, along, off, side) = recipe.pick;
    let pick = nudged(recipe.target, st, along, off, side);
    let result = trim_any(sketch_of(&before), "a0000000", pick);
    Some((before, pick, result))
}

/// The pieces of the trimmed curve left: lines or arcs (or the circle), by ID.
fn pieces_left(sketch: &Sketch, done: &Trim) -> Vec<([f64; 2], [f64; 2])> {
    done.pieces
        .iter()
        .filter_map(|p| {
            if let Some(l) = sketch.lines.iter().find(|l| l.id == *p) {
                return Some((xy(sketch, l.from.as_str()), xy(sketch, l.to.as_str())));
            }
            sketch
                .arcs
                .iter()
                .find(|a| a.id == *p)
                .map(|a| (xy(sketch, a.start.as_str()), xy(sketch, a.end.as_str())))
        })
        .collect()
}

/// How far `at` is from the nearer end of line or arc `curve` (infinite for a circle).
fn off_ends(sketch: &Sketch, at: [f64; 2], curve: &ElementId) -> f64 {
    let ends: Vec<&str> = match sketch.lines.iter().find(|l| l.id == *curve) {
        Some(l) => vec![l.from.as_str(), l.to.as_str()],
        None => sketch
            .arcs
            .iter()
            .find(|a| a.id == *curve)
            .map_or(vec![], |a| vec![a.start.as_str(), a.end.as_str()]),
    };
    ends.into_iter()
        .map(|e| {
            let p = xy(sketch, e);
            libm::hypot(at[0] - p[0], at[1] - p[1])
        })
        .fold(f64::INFINITY, f64::min)
}

/// How far `at` is from curve `curve`'s carrier: a line's through its ends, a round's circle
/// (an arc's radius its start's distance from its centre), worked here, not by the trim.
fn off_curve(sketch: &Sketch, at: [f64; 2], curve: &ElementId) -> f64 {
    if let Some(l) = sketch.lines.iter().find(|l| l.id == *curve) {
        let (a, b) = (xy(sketch, l.from.as_str()), xy(sketch, l.to.as_str()));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        return ((at[0] - a[0]) * dy - (at[1] - a[1]) * dx).abs() / libm::hypot(dx, dy);
    }
    let (center, radius) = match sketch.circles.iter().find(|c| c.id == *curve) {
        Some(c) => (xy(sketch, c.center.as_str()), c.radius.mm()),
        None => {
            let a = sketch.arcs.iter().find(|a| a.id == *curve).unwrap();
            let center = xy(sketch, a.center.as_str());
            let start = xy(sketch, a.start.as_str());
            (
                center,
                libm::hypot(start[0] - center[0], start[1] - center[1]),
            )
        }
    };
    (libm::hypot(at[0] - center[0], at[1] - center[1]) - radius).abs()
}

proptest! {
    #![proptest_config(config(2000))]

    /// Any trim of a gated sketch's curve: refused naming the pick, or a sketch the gate
    /// takes, which moves no point it keeps, and is the same each time.
    #[test]
    fn any_trim_is_refused_or_gated_and_the_same_each_time(recipe in recipe()) {
        let Some((before, pick, result)) = trim_of(&recipe) else { return Ok(()) };
        let sketch = sketch_of(&before);
        prop_assert_eq!(&trim_any(sketch, "a0000000", pick), &result);
        match result {
            Err(e) => {
                prop_assert_eq!(e.parameter(), Some("pick"), "{:?}", e);
                prop_assert!(!e.elements().is_empty(), "{:?}", e);
                prop_assert!(e.suggestion().is_some(), "{:?}", e);
                // Only decision 2's refusals, a constraint a shared end would make a piece's
                // own (as split's), and a pick at a round's centre: anything else is a trim
                // refusing what it should make.
                prop_assert!(
                    e.message().ends_with("on no piece of it")
                        || e.message().starts_with("the trim would make constraint ")
                        || e.message().starts_with("the pick is at "),
                    "{:?}", e
                );
            }
            Ok(done) => {
                prop_assert_eq!(gate(&before, &done.edited.sketch), Vec::<String>::new(), "pick {:?}", pick);
                for p in &sketch.points {
                    if let Some(kept) = done.edited.sketch.points.iter().find(|q| q.id == p.id) {
                        prop_assert_eq!(kept, p);
                    }
                }
            }
        }
    }

    /// Every piece left lies on the curve, between the curve's ends, each new end on its
    /// carrier; and no piece holds the pick's projection more than 2e-7 mm from its ends.
    #[test]
    fn any_trim_removes_the_piece_holding_the_pick_and_keeps_the_rest_on_the_curve(recipe in recipe()) {
        let Some((_, pick, result)) = trim_of(&recipe) else { return Ok(()) };
        let Ok(done) = result else { return Ok(()) };
        let t = recipe.target;
        let sketch = &done.edited.sketch;
        for a in &done.edited.added {
            if let Some(p) = sketch.points.iter().find(|p| p.id == *a) {
                let at = [p.x.mm(), p.y.mm()];
                prop_assert!(t.off(at) <= 1e-7 + 1e-12, "new point {} at {:?} is {} off the curve", a, at, t.off(at));
            }
        }
        // Each new end held on its cutter (an on or a tangent_on) lies on that cutter, or at
        // one of its ends (an arc's end the recipe draws off its own circle cuts where it
        // is, and the solve settles it).
        for c in &sketch.constraints {
            let (point, cutter) = match &c.form {
                Form::On { point, curve } => (point, curve),
                Form::TangentOn { at, b, .. } => (at, b),
                _ => continue,
            };
            let (Some(p), Some(k)) = (point.element(), cutter.element()) else { continue };
            if !done.edited.added.contains(p) {
                continue;
            }
            let at = xy(sketch, p.as_str());
            let off = off_curve(sketch, at, k).min(off_ends(sketch, at, k));
            prop_assert!(off <= 1e-7 + 1e-12, "new end {} at {:?} is {} off its cutter {}", p, at, off, k);
        }
        let projected = t.along(pick);
        let length = |s: f64, e: f64| if t.round() { (e - s).rem_euclid(25.0 * std::f64::consts::TAU) } else { e - s };
        for (s, e) in pieces_left(sketch, &done) {
            prop_assert!(t.off(s) <= 1e-7 + 1e-12 && t.off(e) <= 1e-7 + 1e-12, "a piece from {:?} to {:?} off the curve", s, e);
            let (s, e) = (t.along(s), t.along(e));
            let into = length(s, projected);
            prop_assert!(!(into > 2e-7 && into < length(s, e) - 2e-7),
                "the piece from {} to {} (along) holds the pick at {}", s, e, projected);
        }
    }

    /// Every tangency the trim keeps is sent at the angle it was.
    #[test]
    fn any_trim_keeps_its_tangencies_angles(recipe in recipe()) {
        let Some((before, _, result)) = trim_of(&recipe) else { return Ok(()) };
        let Ok(done) = result else { return Ok(()) };
        let was = angles(sketch_of(&before));
        for (c, angle) in angles(&done.edited.sketch) {
            if let Some(old) = was.get(&c) {
                prop_assert_eq!(*old, angle, "{} turned: {:?} to {:?}", c,
                    form(sketch_of(&before), &c), form(&done.edited.sketch, &c));
            }
        }
    }
}
