//! Hostile inputs for `silico_core::extend` on the fake sketch solver, from break-it: the
//! increments at the ends of their types, the reach, a near whole turn, an arc too far
//! from the origin to move, a drag that moves more than points, and neighbours the edit
//! collapses. The contract: a refusal naming `by` (or `curve`) and the curve, or a sketch
//! the gate takes whose `modified` says what changed. Expected values by hand, or, in the
//! property tests, from the stored points and the increment (a rotation, a length along
//! the line).

use std::f64::consts::TAU;

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use silico_core::document::sketch_edit::Edited;
use silico_core::document::{
    Angle, Document, ElementId, Length, Operation, Sketch, read, validate,
};
use silico_core::extend::{By, End, extend};
use silico_core::fake::{DragCall, FakeSketches};
use silico_core::{Error, ErrorCode};
use silico_protocol::{DragBegin, Moved, PointValues, SketchGeometry, SolveStatus};

use crate::fixture::{arc, circle, document, fields, line, point};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn doc(
    points: &[String],
    lines: &[String],
    circles: &str,
    arcs: &str,
    constraints: &str,
) -> Document {
    read(
        document(&fields(
            &points.join(", "),
            &lines.join(", "),
            circles,
            arcs,
            constraints,
        ))
        .as_bytes(),
    )
    .unwrap()
}

fn sketch_of(document: &Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

/// The gate's refusals of `sketch` in `document`'s place, each with its elements.
fn gate(document: &Document, sketch: &Sketch) -> Vec<(String, Vec<String>)> {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(sketch.clone());
    match validate(&document) {
        Ok(()) => Vec::new(),
        Err(invalid) => invalid
            .errors()
            .iter()
            .map(|e| (e.message().to_owned(), e.elements().to_vec()))
            .collect(),
    }
}

fn at(sketch: &Sketch, name: &str) -> [f64; 2] {
    let p = sketch
        .points
        .iter()
        .find(|p| p.id.as_str() == name)
        .unwrap();
    [p.x.mm(), p.y.mm()]
}

/// The begin state with the dragged point at `xy`, every other value as it began.
fn frame_at(begin: &DragBegin, xy: [f64; 2]) -> Moved {
    let mut values = begin.system.values.clone();
    match begin.system.geometry[dragged_geometry(begin).0 as usize] {
        SketchGeometry::Point(PointValues { x, y }) => {
            (values[x.0 as usize], values[y.0 as usize]) = (xy[0], xy[1]);
        }
        ref other => panic!("dragged a point of {other:?}"),
    }
    Moved {
        status: SolveStatus::Success,
        values: Some(values),
    }
}

/// An error's code, message, parameter, elements, and suggestion, to compare whole.
fn said(error: &Error) -> (ErrorCode, &str, Option<&str>, Vec<&str>, Option<&str>) {
    (
        error.code(),
        error.message(),
        error.parameter(),
        error.elements().iter().map(String::as_str).collect(),
        error.suggestion(),
    )
}

/// A refusal of the input: InvalidInput, naming `parameter` and `curve`.
#[track_caller]
fn refused(result: Result<Edited, Error>, parameter: &str, curve: &str) -> Error {
    let e = result.unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
    assert_eq!(e.parameter(), Some(parameter), "{e}");
    assert_eq!(e.elements(), [curve], "{e}");
    e
}

/// Line a0 from p0 to p1.
fn a_line_from(p: [f64; 2], q: [f64; 2]) -> Document {
    doc(
        &[point("p0000000", p[0], p[1]), point("p0000001", q[0], q[1])],
        &[line("a0000000", "p0000000", "p0000001")],
        "",
        "",
        "",
    )
}

/// Arc r0 about z0 at `c`, from s0 to e0, the points as given.
fn an_arc_at(c: [f64; 2], s: [f64; 2], e: [f64; 2]) -> Document {
    doc(
        &[
            point("z0000000", c[0], c[1]),
            point("s0000000", s[0], s[1]),
            point("e0000000", e[0], e[1]),
        ],
        &[],
        "",
        &arc("r0000000", "z0000000", "s0000000", "e0000000"),
        "",
    )
}

#[test]
fn increments_at_the_ends_of_their_types_are_refused_naming_by() {
    // i64::MIN and i64::MAX nanometres and nanodegrees, which only code makes: each a
    // refusal of `by`, nothing dragged, and the same solver then extends a line as asked.
    let line_doc = a_line_from([0.0, 0.0], [10.0, 0.0]);
    let arc_doc = an_arc_at([0.0, 0.0], [10.0, 0.0], [0.0, 10.0]);
    let mut fake = FakeSketches::new();
    for nm in [i64::MIN, i64::MIN + 1, i64::MAX] {
        for end in [End::Start, End::End] {
            let got = extend(
                sketch_of(&line_doc),
                &id("a0000000"),
                end,
                By::Length(Length::from_nm(nm)),
                &mut fake,
            );
            refused(got, "by", "a0000000");
        }
    }
    for ndeg in [i64::MIN, i64::MIN + 1, i64::MAX] {
        for end in [End::Start, End::End] {
            let got = extend(
                sketch_of(&arc_doc),
                &id("r0000000"),
                end,
                By::Angle(Angle::from_ndeg(ndeg)),
                &mut fake,
            );
            refused(got, "by", "r0000000");
        }
    }
    assert!(fake.drag_calls().is_empty());
    let edited = extend(
        sketch_of(&line_doc),
        &id("a0000000"),
        End::End,
        By::Length(Length::from_nm(1)),
        &mut fake,
    )
    .unwrap();
    assert_eq!(edited.modified, [id("p0000001")]);
}

#[test]
fn a_line_extended_exactly_to_the_reach_is_made() {
    // From (0, 0) to (999,000, 0), on by 1,000 mm: the end at (1e6, 0) exactly, the reach
    // itself (`within_reach` takes |v| <= 1e6), as a point there is. The frame puts the end
    // at the cursor.
    // BREAKS: the target is worked as q + (p − q)·(longer / length), 999,000 × (1e6 /
    // 999,000) = 1e6 + 1 ulp, so the edit is refused as "past the sketch's reach" though
    // the line it asks for ends on it. 311 of 1,284 such integer lines overshoot.
    let before = a_line_from([0.0, 0.0], [999_000.0, 0.0]);
    let mut fake = FakeSketches::new();
    fake.answer_moves(|begin, to| Ok(frame_at(begin, to)));
    let got = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::Length(Length::rounded(1000.0).unwrap()),
        &mut fake,
    );
    let edited = got.unwrap_or_else(|e| panic!("refused: {e}"));
    assert_eq!(at(&edited.sketch, "p0000001"), [1e6, 0.0]);
    assert!(gate(&before, &edited.sketch).is_empty());
}

#[test]
fn a_line_extended_a_nanometre_past_the_reach_is_refused_naming_by() {
    let before = a_line_from([0.0, 0.0], [999_000.0, 0.0]);
    let mut fake = FakeSketches::new();
    let e = refused(
        extend(
            sketch_of(&before),
            &id("a0000000"),
            End::End,
            By::Length(Length::rounded(1000.000001).unwrap()),
            &mut fake,
        ),
        "by",
        "a0000000",
    );
    assert!(e.message().contains("past the sketch's reach"), "{e}");
    assert!(fake.drag_calls().is_empty());
}

#[test]
fn an_arc_turned_past_the_reach_is_refused_naming_by() {
    // About (999,992, 0), radius 10, from 90° to 180°; its start back by 60° is at 30°:
    // x = 999,992 + 10 cos 30° = 1,000,000.66, past the reach.
    let c = [999_992.0, 0.0];
    let before = an_arc_at(c, [c[0], 10.0], [c[0] - 10.0, 0.0]);
    assert!(gate(&before, sketch_of(&before)).is_empty());
    let mut fake = FakeSketches::new();
    let e = refused(
        extend(
            sketch_of(&before),
            &id("r0000000"),
            End::Start,
            By::Angle(Angle::rounded(60.0).unwrap()),
            &mut fake,
        ),
        "by",
        "r0000000",
    );
    assert!(e.message().contains("past the sketch's reach"), "{e}");
    // 40° back is at 50°: x = 999,992 + 6.43, within it: made, on the circle.
    let edited = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::Start,
        By::Angle(Angle::rounded(40.0).unwrap()),
        &mut fake,
    )
    .unwrap();
    let s = at(&edited.sketch, "s0000000");
    let a = 50f64.to_radians();
    assert!((s[0] - (c[0] + 10.0 * libm::cos(a))).abs() < 1e-9, "{s:?}");
    assert!((s[1] - 10.0 * libm::sin(a)).abs() < 1e-9, "{s:?}");
    assert!(gate(&before, &edited.sketch).is_empty());
}

#[test]
fn a_major_arc_extends_and_shortens_by_the_angle_given() {
    // About (0, 0), radius 10, from 0° to 270°: on by 80° at its end is 350°; on by 90° is
    // a whole turn, refused; its start on by 89.999° (back) is at −89.999°.
    let before = an_arc_at([0.0, 0.0], [10.0, 0.0], [0.0, -10.0]);
    let mut fake = FakeSketches::new();
    let edited = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::End,
        By::Angle(Angle::rounded(80.0).unwrap()),
        &mut fake,
    )
    .unwrap();
    let e = at(&edited.sketch, "e0000000");
    let a = 350f64.to_radians();
    assert!(
        (e[0] - 10.0 * libm::cos(a)).abs() < 1e-12 && (e[1] - 10.0 * libm::sin(a)).abs() < 1e-12
    );
    assert!(gate(&before, &edited.sketch).is_empty());
    let e = refused(
        extend(
            sketch_of(&before),
            &id("r0000000"),
            End::End,
            By::Angle(Angle::rounded(90.0).unwrap()),
            &mut fake,
        ),
        "by",
        "r0000000",
    );
    assert!(e.message().contains("a whole turn"), "{e}");
    let edited = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::Start,
        By::Angle(Angle::from_ndeg(89_999_000_000)),
        &mut fake,
    )
    .unwrap();
    assert!(gate(&before, &edited.sketch).is_empty());
    let s = at(&edited.sketch, "s0000000");
    // The start a thousandth of a degree short of the end, 1.7e-4 mm from it.
    let a = (-89.999f64).to_radians();
    assert!(
        (s[0] - 10.0 * libm::cos(a)).abs() < 1e-12 && (s[1] - 10.0 * libm::sin(a)).abs() < 1e-12
    );
}

#[test]
fn an_arc_nearly_a_whole_turn_with_its_ends_within_the_floor_is_refused_as_a_whole_turn() {
    // The quarter arc of radius 10 on by 269.9999995°: swept 2π − 8.7e-9 rad, its ends
    // 8.7e-8 mm apart, within the floor. Refused, rightly, naming `by`.
    // BREAKS: the refusal is the "too short" one: "would leave it 0 mm long or flipped",
    // and its suggestion is "shorten it by less", for an arc 62.8 mm long that was
    // lengthened to nearly a whole turn. Extending it by less is what helps.
    let before = an_arc_at([0.0, 0.0], [10.0, 0.0], [0.0, 10.0]);
    let mut fake = FakeSketches::new();
    let e = refused(
        extend(
            sketch_of(&before),
            &id("r0000000"),
            End::End,
            By::Angle(Angle::from_ndeg(269_999_999_500)),
            &mut fake,
        ),
        "by",
        "r0000000",
    );
    assert!(!e.message().contains("0 mm long"), "{e}");
    assert_eq!(
        e.suggestion(),
        Some("extend it by less, or draw a circle"),
        "{e}"
    );
}

#[test]
fn a_smallest_arc_far_out_turned_keeps_a_radius_the_gate_takes() {
    // An arc of radius 1.0000001e-7 (the gate takes it: above 1e-7) about (550,278.73, 0),
    // its start turned back by −68.335715835° (on, so the arc shortens), from 274.4° to
    // 206.1°: its chord is 1.8e-7 mm, and the arc stays one the gate takes, as before.
    // (Found by `any_arc_extend_is_refused_or_gated_at_the_angle_given`.)
    // BREAKS: Ok, but the turned start, rounded at 5.5e5 mm (an ulp is 1.2e-10, a thousandth
    // of the radius), lands 9.999999999993508e-8 mm from the centre: the gate refuses the
    // arc the edit returned, "arcs and circles have a radius above 1e-7 mm". The edit checks
    // the arc's sweep and its ends, never the radius its rotation leaves.
    let c = [550_278.728_671_871, 0.0];
    let r = 1.000_000_1e-7;
    let a0 = 0.709_053_592_832_840_3;
    let a1 = a0 + 274.423_087_901_379_63f64.to_radians();
    let s = [c[0] + r * libm::cos(a0), c[1] + r * libm::sin(a0)];
    let e = [c[0] + r * libm::cos(a1), c[1] + r * libm::sin(a1)];
    let before = an_arc_at(c, s, e);
    assert!(gate(&before, sketch_of(&before)).is_empty());
    let mut fake = FakeSketches::new();
    let got = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::Start,
        By::Angle(Angle::from_ndeg(-68_335_715_835)),
        &mut fake,
    );
    match got {
        Ok(edited) => {
            let refusals = gate(&before, &edited.sketch);
            assert!(refusals.is_empty(), "{refusals:?}");
        }
        Err(e) => {
            assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
            assert_eq!(e.parameter(), Some("by"), "{e}");
        }
    }
}

#[test]
fn a_lines_drag_refused_after_a_lengthening_suggests_what_helps() {
    // Lengthened by 5, the frame puts the end behind its start (constraints can pull it
    // there): refused naming `by`, rightly.
    // BREAKS: the suggestion is "shorten it by less", for an increment that lengthened the
    // line; nothing the caller can do by the increment's size follows from it.
    let before = a_line_from([0.0, 0.0], [10.0, 0.0]);
    let mut fake = FakeSketches::new();
    fake.answer_moves(|begin, _| Ok(frame_at(begin, [-3.0, 0.0])));
    let e = refused(
        extend(
            sketch_of(&before),
            &id("a0000000"),
            End::End,
            By::Length(Length::rounded(5.0).unwrap()),
            &mut fake,
        ),
        "by",
        "a0000000",
    );
    assert_ne!(e.suggestion(), Some("shorten it by less"), "{e}");
}

#[test]
fn an_arc_turned_by_a_nanodegree_where_it_cant_move_is_refused_as_nothing() {
    // A quarter arc of radius 2e-7 about (10, −30) (the gate's smallest is above 1e-7), and
    // one of radius 1 about (999,999, 999,999). Each end on by 1 nanodegree moves r × 1.7e-11
    // mm, under half an ulp of its coordinates: the point stays where it is, bit for bit.
    // (Found by `any_arc_extend_is_refused_or_gated_at_the_angle_given`.)
    // BREAKS: Ok, with `modified` empty and the sketch unchanged: an increment that changes
    // nothing, which the edit refuses when it's 0, is taken here. On the small arc it's so
    // up to about 100 nanodegrees.
    for (c, r) in [([10.0, -30.0], 2e-7), ([999_999.0, 999_999.0], 1.0)] {
        let before = an_arc_at(c, [c[0] + r, c[1]], [c[0], c[1] + r]);
        assert!(gate(&before, sketch_of(&before)).is_empty());
        let mut fake = FakeSketches::new();
        let got = extend(
            sketch_of(&before),
            &id("r0000000"),
            End::End,
            By::Angle(Angle::from_ndeg(1)),
            &mut fake,
        );
        nothing_refused(got, &before);
    }
}

#[track_caller]
fn nothing_refused(got: Result<Edited, Error>, before: &Document) {
    match got {
        Ok(edited) => panic!(
            "Ok, modified {:?}, the sketch {}",
            edited.modified,
            if &edited.sketch == sketch_of(before) {
                "unchanged"
            } else {
                "changed"
            }
        ),
        Err(e) => {
            assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
            assert_eq!(e.parameter(), Some("by"), "{e}");
        }
    }
}

#[test]
fn a_drag_that_moves_a_circles_radius_lists_the_circle_modified() {
    // Line a0's end p1 on circle c0 (about z0 at (5, 0), radius 5): the drag that pulls p1
    // out grows the circle (its radius is an unknown the frame moves), as PlaneGCS's does.
    // The edited sketch's circle has the frame's radius, 10.
    // BREAKS: `modified` is the points moved alone, [p1]: circle c0, changed in place, is
    // missing, so a client that redraws or reports what `modified` names misses it.
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("z0000000", 5.0, 0.0),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &circle("c0000000", "z0000000", 5.0),
        "",
        r#"{"id": "k0000000", "type": "on", "point": "p0000001", "curve": "c0000000"}"#,
    );
    let mut fake = FakeSketches::new();
    fake.answer_moves(|begin, to| {
        let mut moved = frame_at(begin, to);
        let values = moved.values.as_mut().unwrap();
        for g in &begin.system.geometry {
            if let SketchGeometry::Circle { radius, .. } = g {
                values[radius.0 as usize] = 10.0;
            }
        }
        Ok(moved)
    });
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::Length(Length::rounded(5.0).unwrap()),
        &mut fake,
    )
    .unwrap();
    assert_eq!(edited.sketch.circles[0].radius.mm(), 10.0);
    // By kind, as `Edited` lists them: the point, then the circle.
    assert_eq!(edited.modified, [id("p0000001"), id("c0000000")]);
}

#[test]
fn a_drag_onto_a_neighbours_far_end_is_refused_naming_it_as_the_gate_would() {
    // Lines a0 (0, 0)–(10, 0) and b0 (10, 0)–(12, 0), sharing p1; a0 on by 2 takes p1 onto
    // p2, so b0 has no length: refused as the gate would refuse it, naming b0.
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000002", 12.0, 0.0),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "p0000001", "p0000002"),
        ],
        "",
        "",
        "",
    );
    let mut fake = FakeSketches::new();
    fake.answer_moves(|begin, to| Ok(frame_at(begin, to)));
    let e = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::Length(Length::rounded(2.0).unwrap()),
        &mut fake,
    )
    .unwrap_err();
    // Refused as the edit's: the curve extended first, then what the gate names (b0's ends,
    // then b0), with the edit's suggestion. The fake puts p1 at (12, 0) exactly.
    assert_eq!(
        said(&e),
        (
            ErrorCode::InvalidInput,
            "extending a0000000 there would leave a sketch the gate refuses: line b0000000 is 0 mm long, from point p0000001 at (12, 0) to p0000002 at (12, 0): lines and arcs are longer than 1e-7 mm between their ends, as the kernel measures them",
            Some("by"),
            vec!["a0000000", "p0000001", "p0000002", "b0000000"],
            Some("extend it by less, or change what moves with its end"),
        )
    );
}

#[test]
fn an_arc_turned_onto_a_neighbours_far_end_is_refused_naming_it_as_the_gate_would() {
    // The quarter arc's end e0 (0, 10) shared with line b0 to q0 at 135° on its circle;
    // on by 45° the end lands on q0 (to rounding), so b0 has no length.
    let h = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
    let before = doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("s0000000", 10.0, 0.0),
            point("e0000000", 0.0, 10.0),
            point("q0000000", -h, h),
        ],
        &[line("b0000000", "e0000000", "q0000000")],
        "",
        &arc("r0000000", "z0000000", "s0000000", "e0000000"),
        "",
    );
    let mut fake = FakeSketches::new();
    let e = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::End,
        By::Angle(Angle::rounded(45.0).unwrap()),
        &mut fake,
    )
    .unwrap_err();
    // Refused as the edit's: the curve extended first, then what the gate names (b0's ends,
    // then b0), with the edit's suggestion. b0's length is rounding's, so the message is
    // matched up to it.
    let (code, message, parameter, elements, suggestion) = said(&e);
    assert_eq!(
        (code, parameter, elements, suggestion),
        (
            ErrorCode::InvalidInput,
            Some("by"),
            vec!["r0000000", "e0000000", "q0000000", "b0000000"],
            Some("extend it by less, or change what moves with its end"),
        )
    );
    assert!(
        message.starts_with(
            "extending r0000000 there would leave a sketch the gate refuses: line b0000000 is "
        ),
        "{message}"
    );
}

#[test]
fn a_constraint_is_no_curve_to_extend() {
    // (The fixed references, origin and the axes, aren't IDs: `ElementId::new` refuses them.)
    assert!(ElementId::new("x_axis").is_none());
    let before = doc(
        &[point("p0000000", 0.0, 0.0), point("p0000001", 10.0, 0.0)],
        &[line("a0000000", "p0000000", "p0000001")],
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}"#,
    );
    let mut fake = FakeSketches::new();
    let e = extend(
        sketch_of(&before),
        &id("k0000000"),
        End::End,
        By::Length(Length::rounded(5.0).unwrap()),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(
        e.message(),
        "constraint k0000000 can't be extended: only a line or an arc can"
    );
    assert_eq!(e.elements(), ["k0000000"]);
    assert!(fake.drag_calls().is_empty());
}

#[test]
fn a_line_whose_end_is_shared_by_a_closed_loop_drags_once_and_gates() {
    // A 10 × 5 rectangle; its bottom on by 3 at its end: one begin, one move to (13, 0),
    // one end; the frame at the cursor leaves a quadrilateral the gate takes.
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000002", 10.0, 5.0),
            point("p0000003", 0.0, 5.0),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("a0000001", "p0000001", "p0000002"),
            line("a0000002", "p0000002", "p0000003"),
            line("a0000003", "p0000003", "p0000000"),
        ],
        "",
        "",
        "",
    );
    let mut fake = FakeSketches::new();
    fake.answer_moves(|begin, to| Ok(frame_at(begin, to)));
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::Length(Length::rounded(3.0).unwrap()),
        &mut fake,
    )
    .unwrap();
    assert_eq!(
        fake.drag_calls()[1..],
        [DragCall::Move([13.0, 0.0]), DragCall::End]
    );
    assert_eq!(at(&edited.sketch, "p0000001"), [13.0, 0.0]);
    assert_eq!(edited.modified, [id("p0000001")]);
    assert!(gate(&before, &edited.sketch).is_empty());
}

/// The line's refusal as too short or flipped, word for word, with `size` its length.
fn line_too_short(size: f64, suggestion: &str) -> (String, Option<String>) {
    (
        format!(
            "extending line a0000000 that far would leave it {size} mm long or flipped: a line must be longer than 1e-7 mm and keep its direction"
        ),
        Some(suggestion.to_string()),
    )
}

#[test]
fn a_lines_drag_that_leaves_it_at_the_floor_running_its_way_is_refused_with_its_length() {
    // From (2, 3) to (10, 3), the end dragged to 2 + 2⁻²⁵ (exact in binary): 2⁻²⁵ mm long,
    // under the 1e-7 floor, though it runs the way it did. Refused with that length, the
    // suggestion following the increment's sign: lengthened by 5, the constraints pulled
    // it back; shortened by 5, it's shortened too far.
    let tiny = 1.0 / 33_554_432.0; // 2⁻²⁵, exactly
    for (by, suggestion) in [
        (
            5.0,
            "change the constraints that pull its end back, or extend it by less",
        ),
        (-5.0, "shorten it by less"),
    ] {
        let before = a_line_from([2.0, 3.0], [10.0, 3.0]);
        let mut fake = FakeSketches::new();
        fake.answer_moves(move |begin, _| Ok(frame_at(begin, [2.0 + tiny, 3.0])));
        let e = refused(
            extend(
                sketch_of(&before),
                &id("a0000000"),
                End::End,
                By::Length(Length::rounded(by).unwrap()),
                &mut fake,
            ),
            "by",
            "a0000000",
        );
        assert_eq!(
            (e.message().to_string(), e.suggestion().map(str::to_string)),
            line_too_short(tiny, suggestion),
            "by {by}"
        );
    }
}

#[test]
fn a_lines_drag_that_turns_it_a_right_angle_is_refused_as_not_running_its_way() {
    // From (2, 3) to (10, 3), lengthened by 5, the end dragged to (2, 8): 5 mm long, at
    // right angles to how it ran (its direction's dot product exactly 0), so not its way.
    let before = a_line_from([2.0, 3.0], [10.0, 3.0]);
    let mut fake = FakeSketches::new();
    fake.answer_moves(|begin, _| Ok(frame_at(begin, [2.0, 8.0])));
    let e = refused(
        extend(
            sketch_of(&before),
            &id("a0000000"),
            End::End,
            By::Length(Length::rounded(5.0).unwrap()),
            &mut fake,
        ),
        "by",
        "a0000000",
    );
    assert_eq!(
        (e.message().to_string(), e.suggestion().map(str::to_string)),
        line_too_short(
            5.0,
            "change the constraints that pull its end back, or extend it by less"
        )
    );
}

#[test]
fn a_vertical_line_extends_along_itself() {
    // From (3, 0) to (3, 10), its end on by 5 to (3, 15): its x unchanged, so the
    // direction's x part is 0 before and after, and it still runs its way.
    let before = a_line_from([3.0, 0.0], [3.0, 10.0]);
    let mut fake = FakeSketches::new();
    fake.answer_moves(|begin, _| Ok(frame_at(begin, [3.0, 15.0])));
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::Length(Length::rounded(5.0).unwrap()),
        &mut fake,
    )
    .unwrap();
    assert_eq!(at(&edited.sketch, "p0000001"), [3.0, 15.0]);
    assert_eq!(edited.modified, [id("p0000001")]);
}

/// The size an arc's refusal as too short gives, its words checked around it.
#[track_caller]
fn arc_too_short(e: &Error) -> f64 {
    let size = e
        .message()
        .strip_prefix("extending arc r0000000 that far would leave it ")
        .and_then(|rest| {
            rest.strip_suffix(
                " mm long or flipped: an arc must be longer than 1e-7 mm and keep its direction",
            )
        })
        .unwrap_or_else(|| panic!("{e}"));
    assert_eq!(e.suggestion(), Some("shorten it by less"), "{e}");
    size.parse().unwrap()
}

#[test]
fn an_arc_shortened_to_under_the_floor_long_is_refused_with_its_length() {
    // About (5, 5), radius 2, a quarter turn from (7, 5) to (5, 7), shortened by
    // 89.999999°: 1e-6° left, 2 × 1e-6 × π/180 = 3.490658503988659e-8 mm long, under the
    // floor. Refused with that length (to the rounding of the sweep, 1e-8 of it).
    let before = an_arc_at([5.0, 5.0], [7.0, 5.0], [5.0, 7.0]);
    let e = refused(
        extend(
            sketch_of(&before),
            &id("r0000000"),
            End::End,
            By::Angle(Angle::rounded(-89.999_999).unwrap()),
            &mut FakeSketches::new(),
        ),
        "by",
        "r0000000",
    );
    let size = arc_too_short(&e);
    assert!(
        (size - 3.490_658_503_988_659e-8).abs() <= 1e-6 * 3.5e-8,
        "{e}"
    );
}

#[test]
fn an_arc_longer_than_the_floor_with_its_ends_within_it_is_refused_with_its_length() {
    // About (0, 0), radius 1e-6, a quarter turn from (1e-6, 0) to (0, 1e-6), shortened by
    // 84.26926°: 5.73074° left, 0.10002028 rad, so 1.0002028157573983e-7 mm long, just
    // over the floor, but its ends 2 × 1e-6 × sin(0.05001014) = 9.9978595e-8 mm apart,
    // under it. Refused by the ends, with the arc's length.
    let before = an_arc_at([0.0, 0.0], [1e-6, 0.0], [0.0, 1e-6]);
    let e = refused(
        extend(
            sketch_of(&before),
            &id("r0000000"),
            End::End,
            By::Angle(Angle::rounded(-84.269_26).unwrap()),
            &mut FakeSketches::new(),
        ),
        "by",
        "r0000000",
    );
    let size = arc_too_short(&e);
    assert!(
        (size - 1.000_202_815_757_398_3e-7).abs() <= 1e-9 * 1e-7,
        "{e}"
    );
}

#[test]
fn an_arcs_start_turned_to_under_the_floor_long_short_of_an_end_off_its_circle_is_refused() {
    // About (0, 0), from (10, 0), radius 10, a quarter turn to (0, 10.5), 0.5 mm off its
    // circle, as the gate leaves it to the solve. The start turned back by 89.999999999°:
    // 1e-9° left, 10 × 1e-9 × π/180 = 1.7453292519943295e-10 mm long, under the floor,
    // though its ends are 0.5 mm apart. Refused with that length (to the rounding of the
    // sweep, 1e-4 of it, from π/2 less nearly π/2).
    let before = an_arc_at([0.0, 0.0], [10.0, 0.0], [0.0, 10.5]);
    assert_eq!(gate(&before, sketch_of(&before)), Vec::new());
    let e = refused(
        extend(
            sketch_of(&before),
            &id("r0000000"),
            End::Start,
            By::Angle(Angle::rounded(-89.999_999_999).unwrap()),
            &mut FakeSketches::new(),
        ),
        "by",
        "r0000000",
    );
    let size = arc_too_short(&e);
    assert!(
        (size - 1.745_329_251_994_329_5e-10).abs() <= 1e-4 * 1.75e-10,
        "{e}"
    );
}

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0xe7e9_d001),
        ..ProptestConfig::default()
    }
}

/// A coordinate, biased to the origin, the reach, and its edge.
fn coordinate() -> impl Strategy<Value = f64> {
    prop_oneof![
        -100.0..100.0f64,
        -1e6..1e6f64,
        (999_000.0..1e6f64),
        (-1e6..-999_000.0f64),
        Just(0.0),
        Just(-0.0),
    ]
}

/// A size, biased to the floor and tiny and huge ones.
fn size() -> impl Strategy<Value = f64> {
    prop_oneof![
        1e-3..100.0f64,
        2e-7..1e-5f64,
        Just(1.000_000_1e-7),
        100.0..2e5f64,
    ]
}

/// The line's increment, in nanometres, biased to the line's own length and to nothing.
fn line_increment(length: f64) -> impl Strategy<Value = i64> {
    let back = -(length * 1e6).round() as i64;
    prop_oneof![
        -1_000_000_000i64..1_000_000_000,
        (back - 3)..(back + 3),
        -3i64..3,
        Just(back - 1_000_000),
    ]
}

/// The arc's turn, in nanodegrees, biased to undoing its sweep and to a whole turn.
fn arc_turn(sweep_degrees: f64) -> impl Strategy<Value = i64> {
    let back = -(sweep_degrees * 1e9).round() as i64;
    let full = ((360.0 - sweep_degrees) * 1e9).round() as i64;
    prop_oneof![
        -360_000_000_000i64..360_000_000_000,
        (back - 2000)..(back + 2000),
        (full - 2000)..(full + 2000),
        -5i64..5,
    ]
}

proptest! {
    #![proptest_config(config(1500))]

    /// A line extended: refused naming `by`, the line too short or its end past the
    /// reach; or a sketch the gate takes, its end dragged once to the point along the line
    /// at the new length from the other end.
    #[test]
    fn any_line_extend_is_refused_or_gated_along_the_line(
        q in (coordinate(), coordinate()),
        (length, angle) in (size(), 0.0..TAU),
        start in any::<bool>(),
        nm in size().prop_flat_map(line_increment),
    ) {
        let q = [q.0, q.1];
        let p = [q[0] + length * libm::cos(angle), q[1] + length * libm::sin(angle)];
        let (from, to) = if start { (p, q) } else { (q, p) };
        let before = a_line_from(from, to);
        if !gate(&before, sketch_of(&before)).is_empty() {
            return Ok(());
        }
        let stored = libm::hypot(p[0] - q[0], p[1] - q[1]);
        let by = Length::from_nm(nm);
        let mut fake = FakeSketches::new();
        fake.answer_moves(|begin, to| Ok(frame_at(begin, to)));
        let end = if start { End::Start } else { End::End };
        let got = extend(sketch_of(&before), &id("a0000000"), end, By::Length(by), &mut fake);
        let longer = stored + by.mm();
        match got {
            Err(e) => {
                prop_assert_eq!(e.code(), ErrorCode::InvalidInput, "{}", e);
                prop_assert_eq!(e.parameter(), Some("by"), "{}", e);
                prop_assert_eq!(e.elements(), ["a0000000"], "{}", e);
                // Refused only for a reason: nothing, too short, or past the reach.
                let target = [q[0] + (p[0] - q[0]) / stored * longer, q[1] + (p[1] - q[1]) / stored * longer];
                let reach = target.iter().any(|v| v.abs() > 1e6 - 1e-6);
                prop_assert!(nm == 0 || longer <= 1e-7 * (1.0 + 1e-9) || reach, "{}", e);
            }
            Ok(edited) => {
                prop_assert!(nm != 0 && longer > 1e-7);
                let refusals = gate(&before, &edited.sketch);
                prop_assert!(refusals.is_empty(), "{:?}", refusals);
                let moved = if start { "p0000000" } else { "p0000001" };
                let other = if start { "p0000001" } else { "p0000000" };
                let m = at(&edited.sketch, moved);
                prop_assert_eq!(at(&edited.sketch, other), q);
                // At the new length from the other end, along the line.
                let now = libm::hypot(m[0] - q[0], m[1] - q[1]);
                let scale = 1e-12 * (1.0 + q[0].abs().max(q[1].abs()) + longer);
                prop_assert!((now - longer).abs() <= 4.0 * scale, "{} against {}", now, longer);
                let cross = (m[0] - q[0]) * (p[1] - q[1]) - (m[1] - q[1]) * (p[0] - q[0]);
                prop_assert!(cross.abs() <= 4.0 * scale * stored.max(1.0) * longer.max(1.0), "{}", cross);
                let calls = fake.drag_calls();
                prop_assert_eq!(calls.len(), 3);
                prop_assert_eq!(&calls[2], &DragCall::End);
            }
        }
    }

    /// An arc extended: refused naming `by`, nothing, flipped, too short, a whole turn, its
    /// ends (their chord) within the floor, or past the reach; or a sketch the gate takes, the moved end on its circle at the
    /// sweep plus the turn.
    // BREAKS: an arc of radius just above the floor far from the origin, its start turned,
    // comes back with a radius the gate refuses (as in
    // `a_smallest_arc_far_out_turned_keeps_a_radius_the_gate_takes`).
    #[test]
    fn any_arc_extend_is_refused_or_gated_at_the_angle_given(
        c in (coordinate(), coordinate()),
        radius in size(),
        a0 in 0.0..TAU,
        sweep_degrees in prop_oneof![1e-6..359.999f64, 0.1..180.0f64, 359.0..359.999_999f64],
        at_start in any::<bool>(),
        ndeg in prop_oneof![1e-6..359.999f64, 0.1..180.0f64, 359.0..359.999_999f64].prop_flat_map(arc_turn),
    ) {
        let c = [c.0, c.1];
        let a1 = a0 + sweep_degrees.to_radians();
        let s = [c[0] + radius * libm::cos(a0), c[1] + radius * libm::sin(a0)];
        let e = [c[0] + radius * libm::cos(a1), c[1] + radius * libm::sin(a1)];
        let before = an_arc_at(c, s, e);
        if !gate(&before, sketch_of(&before)).is_empty() {
            return Ok(());
        }
        // The stored sweep, from the stored points.
        let angle = |p: [f64; 2]| libm::atan2(p[1] - c[1], p[0] - c[0]);
        let stored = (angle(e) - angle(s)).rem_euclid(TAU);
        let by = Angle::from_ndeg(ndeg);
        let turn = by.degrees().to_radians();
        let mut fake = FakeSketches::new();
        let end = if at_start { End::Start } else { End::End };
        let got = extend(sketch_of(&before), &id("r0000000"), end, By::Angle(by), &mut fake);
        let swept = stored + turn;
        match got {
            Err(err) => {
                prop_assert_eq!(err.code(), ErrorCode::InvalidInput, "{}", err);
                prop_assert_eq!(err.parameter(), Some("by"), "{}", err);
                prop_assert_eq!(err.elements(), ["r0000000"], "{}", err);
                let r = libm::hypot(s[0] - c[0], s[1] - c[1]).max(libm::hypot(e[0] - c[0], e[1] - c[1]));
                let reach = c.iter().any(|v| v.abs() + r > 1e6 - 1e-6);
                // A turn under a few ulps of the point moves it nowhere, bit for bit:
                // nothing changes (`an_arc_turned_by_a_nanodegree_where_it_cant_move_is_refused_as_nothing`).
                let ulp = 4e-16 * (1.0 + c[0].abs().max(c[1].abs()) + r);
                prop_assert!(
                    ndeg == 0
                        || (err.message().ends_with("changes nothing") && r * turn.abs() <= 8.0 * ulp)
                        // A radius at the floor, which rounding the turned end can take under it.
                        || (err.message().contains("from its centre") && r <= 1e-7 + 8.0 * ulp)
                        || r * swept <= 1e-7 * (1.0 + 1e-6)
                        || 2.0 * r * libm::sin(swept / 2.0).abs() <= 1e-7 * (1.0 + 1e-6)
                        || swept >= TAU
                        || reach,
                    "{} (swept {})", err, swept
                );
            }
            Ok(edited) => {
                prop_assert!(ndeg != 0 && swept > 0.0 && swept < TAU, "swept {}", swept);
                let refusals = gate(&before, &edited.sketch);
                prop_assert!(refusals.is_empty(), "{:?}", refusals);
                // (An Ok that moved nothing is
                // `an_arc_turned_by_a_nanodegree_where_it_cant_move_is_refused_as_nothing`'s.)
                let (moved, from) = if at_start { ("s0000000", s) } else { ("e0000000", e) };
                let m = at(&edited.sketch, moved);
                let r0 = libm::hypot(from[0] - c[0], from[1] - c[1]);
                let r1 = libm::hypot(m[0] - c[0], m[1] - c[1]);
                let ulp = 4e-16 * (1.0 + c[0].abs().max(c[1].abs()) + r0);
                prop_assert!((r1 - r0).abs() <= 8.0 * ulp, "{} against {}", r1, r0);
                // The moved end at the stored angle turned: back for the start, on for the end.
                let want = if at_start { -turn } else { turn };
                let got = (angle(m) - angle(from) + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI;
                let wrapped = (want + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI;
                prop_assert!((got - wrapped).abs() <= 8.0 * ulp / r0 + 1e-12, "{} against {}", got, wrapped);
            }
        }
    }
}

/// The geometry an extend's one move drags: its point's.
fn dragged_geometry(begin: &silico_protocol::DragBegin) -> silico_protocol::Geometry {
    match begin.dragged {
        silico_protocol::Dragged::Point(point) => point.geometry(),
        silico_protocol::Dragged::Edge { curve, .. } => curve,
    }
}
