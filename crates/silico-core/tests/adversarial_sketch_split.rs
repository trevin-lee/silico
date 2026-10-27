//! Hostile inputs for splitting a sketch's curve (`document::sketch_edit::split`), from
//! break-it. The split's contract: a refusal as data naming the curve and the parameter at
//! fault, or a sketch the validation gate takes, whose constraints mean what they meant.
//! Expected values are worked by hand (exact binary positions, a circle's tangent
//! directions), or, in the property tests, checked by the gate and by the solver's sent
//! angles before and after (each piece's own direction is the curve's, so a carried
//! tangency's angle can't change).

use std::sync::mpsc;
use std::time::Duration;

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use silico_core::Error;
use silico_core::document::sketch_edit::{Split, split};
use silico_core::document::{Document, ElementId, Form, Operation, Sketch, read, validate};
use silico_core::sketch::Translation;

use crate::fixture::{arc, circle, document, fields, line, point};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn ids(texts: &[&str]) -> Vec<ElementId> {
    texts.iter().map(|t| id(t)).collect()
}

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

/// IDs for new members, none of which the tests' sketches hold.
const FRESH: [&str; 4] = ["w0000000", "w0000001", "w0000002", "w0000003"];

/// The split with as many fresh IDs as it asks for: tried with one to four, a count the
/// split refuses as the wrong count passed over.
fn split_any(sketch: &Sketch, curve: &str, at: &[[f64; 2]]) -> Result<Split, Error> {
    let mut last = None;
    for n in 1..=FRESH.len() {
        match split(sketch, &id(curve), at, &ids(&FRESH[..n])) {
            Err(e)
                if e.parameter() == Some("ids") && e.message().starts_with("the split needs") =>
            {
                last = Some(e);
            }
            other => return other,
        }
    }
    Err(last.unwrap())
}

/// The split's contract for `curve` at `at`: a refusal naming the curve at an `at`
/// parameter, or a sketch the gate takes. The split's result, if any.
#[track_caller]
fn refused_or_gated(before: &Document, curve: &str, at: &[[f64; 2]]) -> Option<Split> {
    match split_any(sketch_of(before), curve, at) {
        Err(e) => {
            assert!(
                e.parameter().is_some_and(|p| p.starts_with("at")),
                "refused at the wrong parameter: {e:?}"
            );
            assert!(
                e.elements().iter().any(|x| x == curve),
                "the refusal doesn't name {curve}: {e:?}"
            );
            assert!(e.suggestion().is_some(), "no suggestion: {e:?}");
            None
        }
        Ok(done) => {
            assert_eq!(
                gate(before, &done.edited.sketch),
                Vec::<String>::new(),
                "the split returned a sketch the gate refuses"
            );
            Some(done)
        }
    }
}

/// The angle sent to the solver for the constraint `name` (a tangent_on's or tangent_at's:
/// 0 or π, between the two curves' own directions at the point).
#[track_caller]
fn sent_angle(sketch: &Sketch, name: &str) -> f64 {
    let translation = Translation::new(sketch).unwrap_or_else(|e| panic!("{e:?}"));
    let mut sorted: Vec<&ElementId> = sketch.constraints.iter().map(|c| &c.id).collect();
    sorted.sort();
    let i = sorted.iter().position(|c| c.as_str() == name).unwrap();
    translation.sent_angle(i).unwrap()
}

fn form<'a>(sketch: &'a Sketch, name: &str) -> &'a Form {
    &sketch
        .constraints
        .iter()
        .find(|c| c.id == id(name))
        .unwrap_or_else(|| panic!("no constraint {name}"))
        .form
}

// ------------------------------------------------------------------ tangencies carried

/// Circle c about z (0, 0), radius 10; s (10, 0) held on it; line x from s up to (10, 10),
/// tangent on c at s, leaving s along +y, which is c's own (counter-clockwise) direction
/// there: `same`, sent as an angle of 0.
fn circle_with_a_tangent_line_at_s() -> Document {
    let points = [
        point("z0000000", 0.0, 0.0),
        point("s0000000", 10.0, 0.0),
        point("t0000000", 10.0, 10.0),
    ]
    .join(", ");
    doc(
        &points,
        &line("x0000000", "s0000000", "t0000000"),
        &circle("c0000000", "z0000000", 10.0),
        "",
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "c0000000"}, {"id": "k0000001", "type": "tangent_on", "a": "x0000000", "at": "s0000000", "b": "c0000000", "direction": "same"}"#,
    )
}

#[test]
fn a_circle_split_first_at_a_tangent_on_point_keeps_the_tangency_unflipped() {
    // Broke, before the fix: split carries the tangent_on's `same` to the tangent_at as stored, but the
    // start-side arc c *starts* at s (it runs counter-clockwise from at.0 to at.1), and x
    // starts there too: both start at s, so a tangent_at `same` (a smooth path along x into
    // s and on along c) is sent as π, where the drawing has the two running the same way,
    // angle 0. A solve then turns x round or refuses a flip. By hand: x leaves s along +y;
    // c's own direction at angle 0 is (−sin 0, cos 0) = +y; the angle between is 0, so the
    // carried constraint must be sent as 0 (`opposite`, both starting at s).
    let before = circle_with_a_tangent_line_at_s();
    assert_eq!(sent_angle(sketch_of(&before), "k0000001"), 0.0);
    let done = refused_or_gated(&before, "c0000000", &[[10.0, 0.0], [-10.0, 0.0]]).unwrap();
    let after = &done.edited.sketch;
    assert!(matches!(form(after, "k0000001"), Form::TangentAt { .. }));
    assert_eq!(
        sent_angle(after, "k0000001"),
        0.0,
        "the carried tangency was flipped: {:?}",
        form(after, "k0000001")
    );
}

#[test]
fn a_circle_split_second_at_a_tangent_on_point_keeps_the_tangency() {
    // The same circle split with s second: the start-side arc ends at s, arriving along
    // +y, and x starts there along +y: `same` is sent as 0, as before.
    let before = circle_with_a_tangent_line_at_s();
    let done = refused_or_gated(&before, "c0000000", &[[-10.0, 0.0], [10.0, 0.0]]).unwrap();
    assert_eq!(sent_angle(&done.edited.sketch, "k0000001"), 0.0);
}

#[test]
fn an_arc_split_at_a_tangent_on_point_keeps_the_tangency() {
    // Arc r about z (0, 0) from e0 (10, 0) to e1 (−10, 0) through (0, 10); s (0, 10) on it;
    // line x from s to (−10, 10), leaving s along −x, r's own direction at 90°: same.
    let points = [
        point("z0000000", 0.0, 0.0),
        point("e0000000", 10.0, 0.0),
        point("e0000001", -10.0, 0.0),
        point("s0000000", 0.0, 10.0),
        point("t0000000", -10.0, 10.0),
    ]
    .join(", ");
    let before = doc(
        &points,
        &line("x0000000", "s0000000", "t0000000"),
        "",
        &arc("r0000000", "z0000000", "e0000000", "e0000001"),
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "r0000000"}, {"id": "k0000001", "type": "tangent_on", "a": "x0000000", "at": "s0000000", "b": "r0000000", "direction": "same"}"#,
    );
    assert_eq!(sent_angle(sketch_of(&before), "k0000001"), 0.0);
    let done = refused_or_gated(&before, "r0000000", &[[0.0, 10.0]]).unwrap();
    assert_eq!(sent_angle(&done.edited.sketch, "k0000001"), 0.0);
}

#[test]
fn a_tangent_at_on_a_lines_far_end_and_one_on_its_near_end_keep_their_angles() {
    // Line a from p0 (0, 0) to p4 (32, 0); arcs b0 about (0, 10) ending at p0 and b1 about
    // (32, 10) starting at p4, each tangent_at a, smooth: the far end's goes to the new
    // piece, which ends at p4 as a did, so neither angle changes.
    let points = [
        point("p0000000", 0.0, 0.0),
        point("p0000004", 32.0, 0.0),
        point("z0000000", 0.0, 10.0),
        point("z0000001", -10.0, 10.0),
        point("z0000002", 32.0, 10.0),
        point("z0000003", 42.0, 10.0),
    ]
    .join(", ");
    let arcs = [
        arc("b0000000", "z0000000", "z0000001", "p0000000"),
        arc("b0000001", "z0000002", "p0000004", "z0000003"),
    ]
    .join(", ");
    let before = doc(
        &points,
        &line("a0000000", "p0000000", "p0000004"),
        "",
        &arcs,
        r#"{"id": "k0000000", "type": "tangent_at", "a": "b0000000", "b": "a0000000", "at": "p0000000", "direction": "same"}, {"id": "k0000001", "type": "tangent_at", "a": "a0000000", "b": "b0000001", "at": "p0000004", "direction": "same"}"#,
    );
    let angles = |s: &Sketch| [sent_angle(s, "k0000000"), sent_angle(s, "k0000001")];
    let was = angles(sketch_of(&before));
    let done = refused_or_gated(&before, "a0000000", &[[8.0, 0.0]]).unwrap();
    assert_eq!(angles(&done.edited.sketch), was);
}

// ------------------------------------------------------------------ pieces of no size

#[test]
fn a_point_on_the_line_within_1e_7_of_its_end_is_shared_by_a_cut_farther_off() {
    // Broke, before the fix: the cut, (32 − 1.5e-7, 0), is 1.5e-7 mm from a's end p4, so it passes the
    // end check; s, held on a at 32 − 6e-8, is within 1e-7 of the cut, so it's shared in
    // the cut's place. The new piece then runs from s to p4, 6e-8 mm: the gate refuses it
    // (lines are longer than 1e-7 mm between their ends). The end check must be of the
    // point the pieces end at, shared or new.
    let points = [
        point("p0000000", 0.0, 0.0),
        point("p0000004", 32.0, 0.0),
        point("s0000000", 31.99999994, 0.0),
    ]
    .join(", ");
    let before = doc(
        &points,
        &line("a0000000", "p0000000", "p0000004"),
        "",
        "",
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "a0000000"}"#,
    );
    refused_or_gated(&before, "a0000000", &[[31.99999985, 0.0]]);
}

#[test]
fn a_circles_cuts_1_5e_7_apart_with_a_point_held_between_them() {
    // Broke, before the fix: the cuts (10, 0) and (10, 1.5e-7) are farther apart than 1e-7, so they pass;
    // s, held on c at (10, 7.5e-8), is within 1e-7 of the first and is shared there, and
    // the second gets a new point. The arcs between s and it are 7.5e-8 mm end to end: the
    // gate refuses them.
    let points = [
        point("z0000000", 0.0, 0.0),
        point("s0000000", 10.0, 0.000000075),
    ]
    .join(", ");
    let before = doc(
        &points,
        "",
        &circle("c0000000", "z0000000", 10.0),
        "",
        r#"{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "c0000000"}"#,
    );
    refused_or_gated(&before, "c0000000", &[[10.0, 0.0], [10.0, 0.00000015]]);
}

#[test]
fn an_arc_whose_end_is_drawn_off_its_circle_split_beside_that_end() {
    // Arc r about z (0, 0) from (10, 0) to e1 drawn 9e-8 mm outside its circle at
    // (0, 10.00000009), which the gate leaves to the solve. The cut, (5e-8, 10) on the
    // circle, is hypot(5e-8, 9e-8) ≈ 1.03e-7 mm from e1 as stored, so it passes the end
    // check; but the kernel ends the new piece on the circle at e1's angle, (0, 10), 5e-8
    // mm from the cut, and the gate measures it so.
    let points = [
        point("z0000000", 0.0, 0.0),
        point("e0000000", 10.0, 0.0),
        point("e0000001", 0.0, 10.00000009),
    ]
    .join(", ");
    let before = doc(
        &points,
        "",
        "",
        &arc("r0000000", "z0000000", "e0000000", "e0000001"),
        "",
    );
    refused_or_gated(&before, "r0000000", &[[0.00000005, 10.0]]);
}

/// Line a from p0 (0, 0) to p4 (32, 0); s (10, 0) held on it, q (10, 5) off it; and
/// `constraints` beside the `on`.
fn a_line_with_s_held_on_it(constraints: &str) -> Document {
    let points = [
        point("p0000000", 0.0, 0.0),
        point("p0000004", 32.0, 0.0),
        point("s0000000", 10.0, 0.0),
        point("q0000000", 10.0, 5.0),
    ]
    .join(", ");
    doc(
        &points,
        &line("a0000000", "p0000000", "p0000004"),
        "",
        "",
        &format!(
            r#"{{"id": "k0000000", "type": "on", "point": "s0000000", "curve": "a0000000"}}, {constraints}"#
        ),
    )
}

#[test]
fn a_reference_distance_of_the_shared_point_from_the_split_line() {
    // Broke, before the fix: s is held on a, and a reference point_line_distance measures it from a: 0,
    // which a reference dimension may be. Split at s, s becomes the pieces' shared end, and
    // the point_line_distance goes to the start-side piece, whose own point s now is: the
    // gate refuses it ("a point_line_distance's point, s0000000, is part of a0000000
    // itself"). It's implied, as the `on` is, and goes with it, or the split refuses it.
    let before = a_line_with_s_held_on_it(
        r#"{"id": "k0000001", "type": "point_line_distance", "point": "s0000000", "line": "a0000000", "value": 0, "side": "left", "driving": false}"#,
    );
    refused_or_gated(&before, "a0000000", &[[10.0, 0.0]]);
}

#[test]
fn a_midpoint_at_the_shared_point_becomes_a_symmetric_of_the_outer_ends() {
    // s, held on a, is also a's midpoint (drawn at 10, where a solve moves it to 16): split
    // at s, the midpoint becomes the outer ends' symmetric about s, which the gate takes.
    let before = a_line_with_s_held_on_it(
        r#"{"id": "k0000001", "type": "midpoint", "point": "s0000000", "line": "a0000000"}"#,
    );
    let done = refused_or_gated(&before, "a0000000", &[[10.0, 0.0]]).unwrap();
    assert!(matches!(
        form(&done.edited.sketch, "k0000001"),
        Form::Symmetric { .. }
    ));
}

#[test]
fn a_symmetric_about_the_split_line_of_the_shared_point() {
    // Broke, before the fix: s, held on a, and q mirrored about a (the solve puts q on s; the gate takes
    // it, s being no end of a). After the split s is the start-side piece's end and the
    // symmetric stays about that piece: the gate refuses it ("a symmetric's s0000000 is an
    // end of a0000000, so it mirrors onto itself"). Refused, or gated.
    let before = a_line_with_s_held_on_it(
        r#"{"id": "k0000001", "type": "symmetric", "a": "s0000000", "b": "q0000000", "about": "a0000000"}"#,
    );
    refused_or_gated(&before, "a0000000", &[[10.0, 0.0]]);
}

#[test]
fn a_line_cut_each_ulp_either_side_of_1e_7_from_its_end() {
    // 32 − x is exact for x in [16, 64] (Sterbenz), so each cut's distance from p4 is known
    // exactly: within 1e-7 refused, naming p4, else a split the gate takes.
    let before = doc(
        &[point("p0000000", 0.0, 0.0), point("p0000004", 32.0, 0.0)].join(", "),
        &line("a0000000", "p0000000", "p0000004"),
        "",
        "",
        "",
    );
    let mut x: f64 = 32.0 - 1e-7;
    for _ in 0..8 {
        x = f64::from_bits(x.to_bits() - 1);
    }
    for _ in 0..16 {
        let result = split_any(sketch_of(&before), "a0000000", &[[x, 0.0]]);
        if 32.0 - x <= 1e-7 {
            let e = result.unwrap_err();
            assert_eq!(e.parameter(), Some("at.0"), "{x}: {e:?}");
            assert_eq!(e.elements(), ["a0000000", "p0000004"], "{x}");
        } else {
            let done = result.unwrap_or_else(|e| panic!("{x}: {e:?}"));
            assert_eq!(
                gate(&before, &done.edited.sketch),
                Vec::<String>::new(),
                "{x}"
            );
        }
        x = f64::from_bits(x.to_bits() + 1);
    }
}

#[test]
fn an_arc_cut_where_its_chord_to_the_end_crosses_1e_7() {
    // Arc r about (0, 0), radius 10, from (10, 0) to (0, 10). Cuts at angles stepped across
    // the one whose chord to (0, 10) is 1e-7 (2·10·sin(δ/2) = 1e-7): each refused, or a
    // split the gate takes, as it measures the pieces (the kernel's ends on the circle).
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("e0000000", 10.0, 0.0),
            point("e0000001", 0.0, 10.0),
        ]
        .join(", "),
        "",
        "",
        &arc("r0000000", "z0000000", "e0000000", "e0000001"),
        "",
    );
    let threshold = 2.0 * libm::asin(1e-7 / 20.0);
    for step in -40i32..=40 {
        let delta = threshold * (1.0 + f64::from(step) * 1e-9);
        let angle = std::f64::consts::FRAC_PI_2 - delta;
        let at = [10.0 * libm::cos(angle), 10.0 * libm::sin(angle)];
        refused_or_gated(&before, "r0000000", &[at]);
    }
}

#[test]
fn a_circles_cuts_whose_chord_crosses_1e_7() {
    // Circle c about (0, 0), radius 10: (10, 0) and a second cut stepped across a chord of
    // 1e-7 from it, each refused or gated.
    let before = doc(
        &point("z0000000", 0.0, 0.0),
        "",
        &circle("c0000000", "z0000000", 10.0),
        "",
        "",
    );
    let threshold = 2.0 * libm::asin(1e-7 / 20.0);
    for step in -40i32..=40 {
        let delta = threshold * (1.0 + f64::from(step) * 1e-9);
        let at = [10.0 * libm::cos(delta), 10.0 * libm::sin(delta)];
        refused_or_gated(&before, "c0000000", &[[10.0, 0.0], at]);
    }
}

#[test]
fn a_kilometre_circle_split_at_its_diagonals() {
    // A circle of radius 1e6 mm, the most the gate takes, split where the projection
    // rounds: each arc's radius, measured from its start, may come out an ulp past 1e6.
    let before = doc(
        &point("z0000000", 0.0, 0.0),
        "",
        &circle("c0000000", "z0000000", 1_000_000.0),
        "",
        "",
    );
    let h = 1_000_000.0 * std::f64::consts::FRAC_1_SQRT_2;
    for at in [
        [[h, h], [-h, -h]],
        [[h, -h], [-h, h]],
        [[1e6, 1.0], [-1e6, 3.0]],
        [[123_456.789, 992_349.0], [-700_000.0, -714_142.842_854_285]],
    ] {
        refused_or_gated(&before, "c0000000", &at);
    }
}

#[test]
fn a_kilometre_line_split_near_its_ends_and_middle() {
    let before = doc(
        &[
            point("p0000000", -1_000_000.0, -1_000_000.0),
            point("p0000004", 1_000_000.0, 999_999.0),
        ]
        .join(", "),
        &line("a0000000", "p0000000", "p0000004"),
        "",
        "",
        "",
    );
    for t in [
        1e-13,
        1e-12,
        1e-10,
        0.25,
        1.0 / 3.0,
        0.5,
        1.0 - 1e-12,
        1.0 - 1e-13,
    ] {
        let at = [
            -1_000_000.0 + t * 2_000_000.0,
            -1_000_000.0 + t * 1_999_999.0,
        ];
        refused_or_gated(&before, "a0000000", &[at]);
    }
}

// ------------------------------------------------------------------ hostile numbers

fn the_x_line() -> Document {
    doc(
        &[point("p0000000", 0.0, 0.0), point("p0000004", 32.0, 32.0)].join(", "),
        &line("a0000000", "p0000000", "p0000004"),
        "",
        "",
        "",
    )
}

#[test]
fn a_finite_position_whose_projection_overflows_to_not_a_number() {
    // Broke, before the fix: (1e308, −1e308) is finite, but its projection onto a diagonal line takes
    // 1e308·32 − 1e308·32 = inf − inf, not a number; the distance off the line is then not
    // a number too, which `off > 1e-7` lets past, and the refusal comes from writing the
    // point: "the split point isn't a finite position" at `at`, naming nothing. The
    // position is 1.41e308 mm off the line: refused as off the curve, at at.0, naming a.
    // So for (−f64::MAX, f64::MAX), and the like.
    let before = the_x_line();
    for at in [[1e308, -1e308], [-f64::MAX, f64::MAX], [f64::MAX, -1e308]] {
        let e = split_any(sketch_of(&before), "a0000000", &[at]).unwrap_err();
        assert_eq!(e.parameter(), Some("at.0"), "{at:?}: {e:?}");
        assert_eq!(e.elements(), ["a0000000"], "{at:?}: {e:?}");
        assert!(
            e.message().contains("farther than 1e-7 mm"),
            "{at:?}: {e:?}"
        );
    }
}

#[test]
fn positions_not_finite_far_or_at_a_rounds_centre_are_refused_naming_the_curve() {
    let line_doc = the_x_line();
    for at in [
        [f64::NAN, 1.0],
        [1.0, f64::INFINITY],
        [f64::NEG_INFINITY, 0.0],
        [f64::MAX, f64::MAX],
        [1e300, 1e300],
        [1e300, -1e300],
        [f64::MIN_POSITIVE, -f64::MIN_POSITIVE],
        [5e-324, 5e-324],
        [-0.0, -0.0],
    ] {
        let e = split_any(sketch_of(&line_doc), "a0000000", &[at]).unwrap_err();
        assert!(e.parameter().is_some_and(|p| p == "at.0"), "{at:?}: {e:?}");
        assert!(
            e.elements().iter().any(|x| x == "a0000000"),
            "{at:?}: {e:?}"
        );
        assert!(!e.message().contains("NaN"), "{at:?}: {e:?}");
    }
    // A circle at its centre, at overflowing distances, and with one position bad.
    let circle_doc = doc(
        &point("z0000000", 5.0, 5.0),
        "",
        &circle("c0000000", "z0000000", 10.0),
        "",
        "",
    );
    for at in [
        [[5.0, 5.0], [15.0, 5.0]],
        [[15.0, 5.0], [5.0, 5.0]],
        [[f64::MAX, f64::MAX], [15.0, 5.0]],
        [[15.0, 5.0], [-f64::MAX, f64::MAX]],
        [[15.0, 5.0], [f64::NAN, 5.0]],
    ] {
        let e = split_any(sketch_of(&circle_doc), "c0000000", &at).unwrap_err();
        assert!(
            e.parameter().is_some_and(|p| p == "at.0" || p == "at.1"),
            "{at:?}: {e:?}"
        );
        assert!(
            e.elements().iter().any(|x| x == "c0000000"),
            "{at:?}: {e:?}"
        );
        assert!(!e.message().contains("NaN"), "{at:?}: {e:?}");
    }
}

#[test]
fn a_circle_split_at_two_positions_exactly_1e_7_apart_and_its_mirror_image() {
    // (10, 0) and (10, 1e-7) project 1e-7 apart (to an ulp) on the circle of radius 10;
    // the refusal names at.1. Positions given in either order are the same refusal.
    let before = doc(
        &point("z0000000", 0.0, 0.0),
        "",
        &circle("c0000000", "z0000000", 10.0),
        "",
        "",
    );
    for at in [
        [[10.0, 0.0], [10.0, 0.0]],
        [[10.0, 0.0], [10.0, 0.00000005]],
        [[10.0, 0.00000005], [10.0, 0.0]],
        [[0.0, 10.0], [0.0, 10.0]],
        [[-10.0, 0.0], [-10.0, -0.0]],
    ] {
        let e = split_any(sketch_of(&before), "c0000000", &at).unwrap_err();
        assert_eq!(e.parameter(), Some("at.1"), "{at:?}: {e:?}");
        assert_eq!(e.elements(), ["c0000000"]);
    }
}

#[test]
fn a_circle_split_either_side_of_its_far_side_where_atan2_wraps() {
    // Cuts just above and below (−10, 0), where atan2 jumps from π to −π: two arcs, one
    // nearly a whole turn, the other 2e-6 rad, each built and gated.
    let before = doc(
        &point("z0000000", 0.0, 0.0),
        "",
        &circle("c0000000", "z0000000", 10.0),
        "",
        "",
    );
    for at in [
        [[-10.0, 0.00001], [-10.0, -0.00001]],
        [[-10.0, -0.00001], [-10.0, 0.00001]],
        [[-10.0, 0.0], [-10.0, -0.00001]],
        [[-10.0, -0.00001], [-10.0, 0.0]],
    ] {
        let done = refused_or_gated(&before, "c0000000", &at).unwrap();
        assert_eq!(done.edited.sketch.arcs.len(), 2);
        assert!(done.edited.sketch.circles.is_empty());
    }
}

// ------------------------------------------------------------------ hostile IDs and members

#[test]
fn ids_already_a_constraints_the_curves_or_an_axis_name_are_refused() {
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000004", 32.0, 0.0),
            point("q0000000", 5.0, 5.0),
        ]
        .join(", "),
        &line("a0000000", "p0000000", "p0000004"),
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}"#,
    );
    let sketch = sketch_of(&before);
    for (given, place) in [
        (["k0000000", "w0000001", "w0000002"], 0),
        (["w0000000", "a0000000", "w0000002"], 1),
        (["w0000000", "w0000001", "p0000004"], 2),
        (["w0000000", "w0000001", "w0000000"], 2),
    ] {
        let e = split(sketch, &id("a0000000"), &[[10.0, 0.0]], &ids(&given)).unwrap_err();
        assert_eq!(
            e.parameter(),
            Some(format!("ids.{place}").as_str()),
            "{given:?}"
        );
    }
    // The curve named by a constraint's, a point's, or no member's ID.
    for (curve, says) in [
        ("k0000000", "constraint k0000000 isn't a curve"),
        ("q0000000", "point q0000000 isn't a curve"),
        ("zzzzzzzz", "the sketch has no curve zzzzzzzz"),
    ] {
        let e = split(sketch, &id(curve), &[[10.0, 0.0]], &ids(&FRESH[..3])).unwrap_err();
        assert!(e.message().starts_with(says), "{curve}: {e:?}");
        assert_eq!(e.parameter(), Some("curve"));
    }
}

#[test]
fn a_refused_split_leaves_the_sketch_and_the_next_split_works() {
    let before = the_x_line();
    let sketch = sketch_of(&before).clone();
    let _ = split(&sketch, &id("a0000000"), &[[33.0, 33.0]], &ids(&FRESH[..3])).unwrap_err();
    assert_eq!(&sketch, sketch_of(&before));
    let done = refused_or_gated(&before, "a0000000", &[[8.0, 8.0]]).unwrap();
    // Splitting a piece again, then the other piece: three lines, each gated.
    let mut document = before.clone();
    document.features[0].operation = Operation::Sketch(done.edited.sketch.clone());
    let again = split(
        &done.edited.sketch,
        &id("a0000000"),
        &[[4.0, 4.0]],
        &ids(&["v0000000", "v0000001", "v0000002"]),
    )
    .unwrap();
    assert_eq!(gate(&document, &again.edited.sketch), Vec::<String>::new());
    let third = split(
        &again.edited.sketch,
        &done.pieces[1],
        &[[20.0, 20.0]],
        &ids(&["t0000000", "t0000001", "t0000002"]),
    )
    .unwrap();
    assert_eq!(gate(&document, &third.edited.sketch), Vec::<String>::new());
    assert_eq!(third.edited.sketch.lines.len(), 4);
}

// ------------------------------------------------------------------ points held otherwise

#[test]
fn a_tangent_on_point_at_the_cut_without_an_on_is_the_split_point() {
    // A tangent_on holds its `at` on `b` as an `on` does (decision 5): x's end s, tangent on
    // a at (8, 0), is the split point, and the tangent_on a tangent_at. Broke, before the
    // fix: only an `on` made a point the split point, so s was left beside a new point at
    // its place, unjoined (the port review's medium).
    let points = [
        point("p0000000", 0.0, 0.0),
        point("p0000004", 32.0, 0.0),
        point("z0000000", 8.0, 10.0),
        point("s0000000", 8.0, 0.0),
        point("t0000000", 18.0, 10.0),
    ]
    .join(", ");
    let before = doc(
        &points,
        &line("a0000000", "p0000000", "p0000004"),
        "",
        &arc("x0000000", "z0000000", "s0000000", "t0000000"),
        r#"{"id": "k0000000", "type": "tangent_on", "a": "x0000000", "at": "s0000000", "b": "a0000000", "direction": "same"}"#,
    );
    let done = refused_or_gated(&before, "a0000000", &[[8.0, 0.0]]).unwrap();
    assert_eq!(done.edited.added.len(), 2, "{:?}", done.edited.added);
    assert!(matches!(
        form(&done.edited.sketch, "k0000000"),
        Form::TangentAt { .. }
    ));
}

#[test]
fn a_split_of_a_line_with_the_most_points_on_it_the_gate_takes() {
    // 510 points held on a line, the most the solver's 1,024 unknowns leave room for
    // beside its ends: each goes to the piece holding it.
    let n: u32 = 510;
    let mut points = vec![point("p0000000", 0.0, 0.0), point("p0000004", 8192.0, 0.0)];
    let mut constraints = Vec::new();
    for i in 0..n {
        points.push(point(&format!("s{i:07}"), f64::from(i) * 16.0 + 1.0, 0.0));
        constraints.push(format!(
            r#"{{"id": "k{i:07}", "type": "on", "point": "s{i:07}", "curve": "a0000000"}}"#
        ));
    }
    let before = doc(
        &points.join(", "),
        &line("a0000000", "p0000000", "p0000004"),
        "",
        "",
        &constraints.join(", "),
    );
    let sketch = sketch_of(&before).clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(split(
            &sketch,
            &id("a0000000"),
            &[[4000.0, 0.0]],
            &ids(&FRESH[..3]),
        ));
    });
    let done = rx
        .recv_timeout(Duration::from_secs(120))
        .expect("the split hung")
        .unwrap();
    let on_second = done
        .edited
        .sketch
        .constraints
        .iter()
        .filter(|c| matches!(&c.form, Form::On { curve, .. } if *curve == silico_core::document::Reference::Element(done.pieces[1].clone())))
        .count();
    // s(i) at 16i + 1 is past 4000 for i ≥ 250.
    assert_eq!(on_second, 260);
}

// ------------------------------------------------------------------ property tests

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x5b17_c0de),
        ..ProptestConfig::default()
    }
}

/// The curve split, and the places on it (exact, or nearly) points can be drawn at.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    /// From (0, 0) to (40, 0).
    Line,
    /// From (0, 0) to (30, 40).
    Tilted,
    /// About (0, 0), radius 25, from (25, 0) to (−15, −20).
    Arc,
    /// About (0, 0), radius 25.
    Circle,
}

/// Points on the circle of radius 25 with whole coordinates, counter-clockwise from 0°.
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

impl Target {
    /// The places along the curve (and, for an arc, beyond its ends on its circle).
    fn stations(self) -> Vec<[f64; 2]> {
        match self {
            Target::Line => (1..8).map(|k| [5.0 * f64::from(k), 0.0]).collect(),
            Target::Tilted => (1..10)
                .map(|k| [3.0 * f64::from(k), 4.0 * f64::from(k)])
                .collect(),
            Target::Arc | Target::Circle => ROUND.iter().map(|&(x, y)| [x, y]).collect(),
        }
    }

    /// The curve's ends (a circle's none).
    fn ends(self) -> Vec<[f64; 2]> {
        match self {
            Target::Line => vec![[0.0, 0.0], [40.0, 0.0]],
            Target::Tilted => vec![[0.0, 0.0], [30.0, 40.0]],
            Target::Arc => vec![[25.0, 0.0], [-15.0, -20.0]],
            Target::Circle => vec![],
        }
    }

    /// The unit direction along the curve at `p`, the way its parameter runs.
    fn along(self, p: [f64; 2]) -> [f64; 2] {
        match self {
            Target::Line => [1.0, 0.0],
            Target::Tilted => [0.6, 0.8],
            Target::Arc | Target::Circle => {
                let r = libm::hypot(p[0], p[1]);
                [-p[1] / r, p[0] / r]
            }
        }
    }

    /// The unit normal at `p`.
    fn normal(self, p: [f64; 2]) -> [f64; 2] {
        let a = self.along(p);
        [a[1], -a[0]]
    }
}

/// Small offsets at and either side of the tolerance, and a few larger.
const JITTER: [f64; 10] = [
    0.0,
    3e-8,
    5e-8,
    9.9e-8,
    1e-7,
    1.000_000_1e-7,
    1.5e-7,
    2e-7,
    1e-6,
    1e-3,
];

/// How a split position (or a point's place) is chosen.
#[derive(Debug, Clone, Copy)]
struct Place {
    kind: u8,
    index: usize,
    along: usize,
    off: usize,
    sign: bool,
}

fn place() -> impl Strategy<Value = Place> {
    (
        0u8..6,
        0usize..32,
        0usize..JITTER.len(),
        0usize..JITTER.len(),
        any::<bool>(),
    )
        .prop_map(|(kind, index, along, off, sign)| Place {
            kind,
            index,
            along,
            off,
            sign,
        })
}

impl Place {
    /// The place: for a split position (`points` given), mostly within the tolerance of
    /// one of the drawn points, an end, or a station.
    fn at(&self, target: Target, points: &[[f64; 2]]) -> [f64; 2] {
        let s = if self.sign { -1.0 } else { 1.0 };
        let cut = !points.is_empty();
        let (along, off) = if cut && self.kind != 5 {
            (self.along % 6, self.off % 6)
        } else {
            (self.along, self.off)
        };
        let base = match self.kind {
            // A station.
            0 => {
                let st = target.stations();
                st[self.index % st.len()]
            }
            // An end, or for a circle a station.
            1 if !target.ends().is_empty() => {
                let e = target.ends();
                e[self.index % e.len()]
            }
            // One of the drawn points, which may be held on the curve.
            2..=4 if cut => points[self.index % points.len()],
            _ => {
                let st = target.stations();
                st[(self.index * 7) % st.len()]
            }
        };
        let (a, n) = (target.along(base), target.normal(base));
        let side = if self.index.is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        let (da, dn) = (s * JITTER[along], side * JITTER[off]);
        let mut p = [
            base[0] + a[0] * da + n[0] * dn,
            base[1] + a[1] * da + n[1] * dn,
        ];
        // On a round, back onto the circle now and then, as a solve would leave it.
        if matches!(target, Target::Arc | Target::Circle) && off == 0 {
            let r = libm::hypot(p[0], p[1]);
            p = [p[0] * 25.0 / r, p[1] * 25.0 / r];
        }
        p
    }
}

#[derive(Debug, Clone)]
struct Recipe {
    target: Target,
    /// Points drawn near the curve, each held on it or not.
    near: Vec<(Place, bool)>,
    /// Lines between drawn points (indices into every point).
    lines: Vec<(usize, usize)>,
    /// Arcs centred off the curve (a grid place), from one drawn point to another.
    arcs: Vec<(usize, usize, usize)>,
    /// A form, three references, and a choice.
    constraints: Vec<(u8, usize, usize, usize, u8)>,
    /// Lines from a drawn point out to a grid place, tangent on the split curve there in
    /// one direction or the other.
    joints: Vec<(usize, usize, bool)>,
    cuts: [Place; 2],
}

fn recipe() -> impl Strategy<Value = Recipe> {
    (
        prop_oneof![
            Just(Target::Line),
            Just(Target::Tilted),
            Just(Target::Arc),
            Just(Target::Circle)
        ],
        prop::collection::vec((place(), any::<bool>()), 0..=4),
        prop::collection::vec((0usize..16, 0usize..16), 0..=2),
        prop::collection::vec((0usize..4, 0usize..16, 0usize..16), 0..=2),
        prop::collection::vec((0u8..14, 0usize..32, 0usize..32, 0usize..32, 0u8..4), 0..=8),
        prop::collection::vec((0usize..4, 0usize..4, any::<bool>()), 0..=2),
        [place(), place()],
    )
        .prop_map(
            |(target, near, lines, arcs, constraints, joints, cuts)| Recipe {
                target,
                near,
                lines,
                arcs,
                constraints,
                joints,
                cuts,
            },
        )
}

const GRID: [(f64, f64); 4] = [(10.0, 30.0), (-10.0, -40.0), (50.0, 10.0), (35.0, -5.0)];

/// The recipe's sketch with every constraint the gate refuses left out, and the drawn
/// points' places; none if its geometry is refused.
fn build(recipe: &Recipe) -> Option<(Document, Vec<[f64; 2]>)> {
    let t = recipe.target;
    let mut points = Vec::new();
    let mut places: Vec<[f64; 2]> = Vec::new();
    let (target_json, target_points): (String, Vec<&str>) = match t {
        Target::Line | Target::Tilted => {
            let e = t.ends();
            points.push(point("e0000000", e[0][0], e[0][1]));
            points.push(point("e0000001", e[1][0], e[1][1]));
            (
                line("a0000000", "e0000000", "e0000001"),
                vec!["e0000000", "e0000001"],
            )
        }
        Target::Arc => {
            points.push(point("e0000000", 25.0, 0.0));
            points.push(point("e0000001", -15.0, -20.0));
            points.push(point("e0000002", 0.0, 0.0));
            (
                arc("a0000000", "e0000002", "e0000000", "e0000001"),
                vec!["e0000000", "e0000001", "e0000002"],
            )
        }
        Target::Circle => {
            points.push(point("e0000002", 0.0, 0.0));
            (circle("a0000000", "e0000002", 25.0), vec!["e0000002"])
        }
    };
    let mut on = Vec::new();
    for (i, (p, held)) in recipe.near.iter().enumerate() {
        // A point held on the curve is on it, as a solve leaves it, most of the time.
        let p = if *held && p.off % 4 != 0 {
            Place { off: 0, ..*p }
        } else {
            *p
        };
        let xy = p.at(t, &[]);
        let pid = format!("s{i:07}");
        points.push(point(&pid, xy[0], xy[1]));
        places.push(xy);
        if *held {
            on.push(format!(
                r#"{{"id": "h{i:07}", "type": "on", "point": "{pid}", "curve": "a0000000"}}"#
            ));
        }
    }
    for (i, &(x, y)) in GRID.iter().enumerate() {
        points.push(point(&format!("g{i:07}"), x, y));
    }
    let pool: Vec<String> = target_points
        .iter()
        .map(|s| (*s).to_owned())
        .chain((0..recipe.near.len()).map(|i| format!("s{i:07}")))
        .chain((0..GRID.len()).map(|i| format!("g{i:07}")))
        .collect();
    let p = |i: usize| pool[i % pool.len()].clone();
    let mut lines = vec![];
    let mut circles = vec![];
    let mut arcs = vec![];
    let mut curves = vec!["a0000000".to_owned()];
    let mut ends: Vec<(String, String)> = vec![];
    match t {
        Target::Line | Target::Tilted => lines.push(target_json),
        Target::Arc => arcs.push(target_json),
        Target::Circle => circles.push(target_json),
    }
    // Other curves start, mostly, at a point drawn near the split one, as a T or a
    // tangency there would.
    let from = |i: usize| {
        if recipe.near.is_empty() || i % 4 == 3 {
            p(i)
        } else {
            format!("s{:07}", i % recipe.near.len())
        }
    };
    for (i, &(a, b)) in recipe.lines.iter().enumerate() {
        if from(a) != p(b) {
            let id = format!("m{i:07}");
            lines.push(line(&id, &from(a), &p(b)));
            ends.push((id.clone(), from(a)));
            ends.push((id.clone(), p(b)));
            curves.push(id);
        }
    }
    for (i, &(c, s, e)) in recipe.arcs.iter().enumerate() {
        let center = format!("g{:07}", c % GRID.len());
        let s = s * 4 + usize::from(s % 2 == 0);
        if from(s) != p(e) && from(s) != center && p(e) != center {
            let id = format!("r{i:07}");
            arcs.push(arc(&id, &center, &from(s), &p(e)));
            ends.push((id.clone(), from(s)));
            ends.push((id.clone(), p(e)));
            curves.push(id);
        }
    }
    let mut joints = Vec::new();
    for (i, &(s, g, same)) in recipe.joints.iter().enumerate() {
        if recipe.near.is_empty() {
            break;
        }
        let id = format!("j{i:07}");
        let at = format!("s{:07}", s % recipe.near.len());
        lines.push(line(&id, &at, &format!("g{g:07}")));
        let dir = if same { "same" } else { "opposite" };
        joints.push(format!(
            r#"{{"id": "t{i:07}", "type": "tangent_on", "a": "{id}", "at": "{at}", "b": "a0000000", "direction": "{dir}"}}"#
        ));
        curves.push(id);
    }
    // A second circle about a grid point, for tangents and equals.
    circles.push(circle("c0000009", "g0000002", 5.0));
    curves.push("c0000009".to_owned());
    let mut curve_pool = curves.clone();
    curve_pool.extend(["x_axis".to_owned(), "y_axis".to_owned()]);
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
    let mut kept: Vec<String> = Vec::new();
    for json in on.into_iter().chain(joints) {
        let mut trial = kept.clone();
        trial.push(json);
        if read(text(&trial).as_bytes()).is_ok_and(|d| validate(&d).is_ok()) {
            kept = trial;
        }
    }
    for (k, &(f, r1, r2, r3, choice)) in recipe.constraints.iter().enumerate() {
        // Half the time the split curve is the constraint's curve.
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
            5 => format!(
                r#""type": "radius", "curve": "{}", "value": 25, "driving": true"#,
                c(r1)
            ),
            6 => format!(r#""type": "equal", "a": "{}", "b": "{}""#, c(r1), c(r2)),
            7 => format!(
                r#""type": "tangent", "a": "{}", "b": "{}", "side": "{tside}""#,
                c(r1),
                c(r2)
            ),
            8 if !ends.is_empty() => {
                let (a, at) = &ends[r1 % ends.len()];
                format!(
                    r#""type": "tangent_on", "a": "{a}", "at": "{at}", "b": "{}", "direction": "{dir}""#,
                    c(r3)
                )
            }
            9 if !ends.is_empty() => {
                // At a target end, mostly: the curve and one of the others sharing it.
                let (b, at) = &ends[r1 % ends.len()];
                format!(
                    r#""type": "tangent_at", "a": "a0000000", "b": "{b}", "at": "{at}", "direction": "{dir}""#
                )
            }
            10 => format!(
                r#""type": "symmetric", "a": "{}", "b": "{}", "about": "{}""#,
                p(r1),
                p(r2),
                c(r3)
            ),
            11 => format!(
                r#""type": "midpoint", "point": "{}", "line": "{}""#,
                p(r1),
                c(r2)
            ),
            12 => format!(
                r#""type": "angle", "a": "{}", "b": "{}", "value": 30, "driving": false"#,
                c(r1),
                c(r2)
            ),
            _ => format!(
                r#""type": "diameter", "curve": "{}", "value": 50, "driving": true"#,
                c(r1)
            ),
        };
        let mut trial = kept.clone();
        trial.push(format!(r#"{{"id": "k{k:07}", {body}}}"#));
        if read(text(&trial).as_bytes()).is_ok_and(|d| validate(&d).is_ok()) {
            kept = trial;
        }
    }
    let document = read(text(&kept).as_bytes()).ok()?;
    Some((document, places))
}

/// Each tangent_on's and tangent_at's sent angle, by ID.
fn angles(sketch: &Sketch) -> Vec<(String, f64)> {
    let Ok(translation) = Translation::new(sketch) else {
        return Vec::new();
    };
    let mut sorted: Vec<&ElementId> = sketch.constraints.iter().map(|c| &c.id).collect();
    sorted.sort();
    sorted
        .iter()
        .enumerate()
        .filter_map(|(i, c)| translation.sent_angle(i).map(|a| (c.to_string(), a)))
        .collect()
}

/// A recipe's sketch, its split positions, and its split.
type Splitting = (Document, Vec<[f64; 2]>, Result<Split, Error>);

/// The recipe's sketch and its split, if the recipe builds.
fn split_of(recipe: &Recipe) -> Option<Splitting> {
    let (before, places) = build(recipe)?;
    let n = if recipe.target == Target::Circle {
        2
    } else {
        1
    };
    let at: Vec<[f64; 2]> = recipe.cuts[..n]
        .iter()
        .map(|c| c.at(recipe.target, &places))
        .collect();
    let result = split_any(sketch_of(&before), "a0000000", &at);
    Some((before, at, result))
}

proptest! {
    #![proptest_config(config(3000))]

    /// Any split of a gated sketch's curve: refused naming the curve and its parameter, or
    /// a sketch the gate takes.
    // Broke, before the fix: a point held on the curve within 1e-7 of the cut but within 1e-7 of an end
    // is shared, and the piece to that end is too short (the targeted
    // a_point_on_the_line_within_1e_7_of_its_end_is_shared_by_a_cut_farther_off and
    // a_circles_cuts_1_5e_7_apart_with_a_point_held_between_them are its minimal cases);
    // and a point_line_distance of the shared point from the split line lands on the
    // piece's own point (a_reference_distance_of_the_shared_point_from_the_split_line).
    #[test]
    fn any_split_is_refused_or_gated(recipe in recipe()) {
        let Some((before, at, result)) = split_of(&recipe) else { return Ok(()) };
        match result {
            Err(e) => {
                prop_assert!(
                    matches!(e.parameter(), Some("at.0" | "at.1")),
                    "{e:?}"
                );
                prop_assert!(e.elements().iter().any(|x| x == "a0000000"), "{e:?}");
                prop_assert!(e.suggestion().is_some(), "{e:?}");
            }
            Ok(done) => {
                prop_assert_eq!(gate(&before, &done.edited.sketch), Vec::<String>::new(), "at {:?}", at);
            }
        }
    }

    /// Any split moves no point drawn before, and is the same split each time.
    #[test]
    fn any_split_moves_no_point_and_is_the_same_each_time(recipe in recipe()) {
        let Some((before, at, result)) = split_of(&recipe) else { return Ok(()) };
        let sketch = sketch_of(&before);
        let again = split_any(sketch, "a0000000", &at);
        prop_assert_eq!(&again, &result);
        if let Ok(done) = result {
            for p in &sketch.points {
                let kept = done.edited.sketch.points.iter().find(|q| q.id == p.id);
                prop_assert_eq!(kept, Some(p));
            }
        }
    }

    /// Every tangency a split carries is sent at the angle it was: each piece's own
    /// direction along it is the curve's.
    // Broke, before the fix: a tangent_on at a circle's first cut becomes a tangent_at with the start-side
    // arc, which starts there, at its stored direction, turned half a turn (the targeted
    // a_circle_split_first_at_a_tangent_on_point_keeps_the_tangency_unflipped).
    #[test]
    fn any_split_keeps_its_tangencies(recipe in recipe()) {
        let Some((before, _, result)) = split_of(&recipe) else { return Ok(()) };
        let sketch = sketch_of(&before);
        if let Ok(done) = result {
            let after = &done.edited.sketch;
            let was: std::collections::BTreeMap<String, f64> = angles(sketch).into_iter().collect();
            for (c, angle) in angles(after) {
                if let Some(old) = was.get(&c) {
                    prop_assert_eq!(*old, angle, "{} turned: {:?} to {:?}", c,
                        form(sketch, &c), form(after, &c));
                }
            }
        }
    }
}
