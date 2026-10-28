//! `silico_core::extend` on the fake sketch solver (docs/ports/sketch-edit.md, §7, decision
//! 11): a line's end through one drag from the sketch as stored, an arc's end turned about its
//! centre, and the refusals. Expected values by hand: the fake answers a drag's move with the
//! dragged point at the cursor nudged by a thousandth of where it began; an arc's turned end
//! is its rotation about the centre.

use std::time::Duration;

use silico_core::document::sketch_edit::Edited;
use silico_core::document::{
    Angle, Document, ElementId, Length, Operation, Sketch, read, validate,
};
use silico_core::extend::{By, End, extend};
use silico_core::fake::{DragCall, FakeSketches};
use silico_core::{Error, ErrorCode};
use silico_protocol::{DragBegin, Moved, PointValues, SketchGeometry, SolveStatus, Tag};

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

/// The edit's sketch in `document`, checked by the gate.
fn gated(document: &Document, edited: &Edited) -> Sketch {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(edited.sketch.clone());
    if let Err(invalid) = validate(&document) {
        panic!("the gate refused the edit: {:?}", invalid.errors());
    }
    edited.sketch.clone()
}

fn at(sketch: &Sketch, name: &str) -> [f64; 2] {
    let p = sketch
        .points
        .iter()
        .find(|p| p.id.as_str() == name)
        .unwrap();
    [p.x.mm(), p.y.mm()]
}

fn near(got: [f64; 2], want: [f64; 2]) {
    assert!(
        (got[0] - want[0]).abs() <= 1e-12 && (got[1] - want[1]).abs() <= 1e-12,
        "{got:?} against {want:?}"
    );
}

fn said(error: &Error) -> (&str, Option<&str>, Vec<&str>) {
    (
        error.message(),
        error.parameter(),
        error.elements().iter().map(String::as_str).collect(),
    )
}

/// The begin state with the dragged point at `xy`: a frame the fake answers instead.
fn frame_at(begin: &DragBegin, xy: [f64; 2]) -> Moved {
    let mut values = begin.system.values.clone();
    let geometry = begin.point.geometry();
    match begin.system.geometry[geometry.0 as usize] {
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

fn mm(value: f64) -> By {
    By::Length(Length::rounded(value).unwrap())
}

fn degrees(value: f64) -> By {
    By::Angle(Angle::rounded(value).unwrap())
}

/// Line a0 from p0 (0, 0) to p1 (10, 0), with `constraints`.
fn a_line(constraints: &str) -> Document {
    doc(
        &[point("p0000000", 0.0, 0.0), point("p0000001", 10.0, 0.0)],
        &[line("a0000000", "p0000000", "p0000001")],
        "",
        "",
        constraints,
    )
}

/// Arc r0 about z0 (0, 0), radius 10, from s0 (10, 0) to e0 (0, 10): a quarter turn.
fn an_arc() -> Document {
    doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("s0000000", 10.0, 0.0),
            point("e0000000", 0.0, 10.0),
        ],
        &[],
        "",
        &arc("r0000000", "z0000000", "s0000000", "e0000000"),
        "",
    )
}

#[test]
fn a_lines_end_is_dragged_once_from_the_stored_sketch_to_the_new_length() {
    // FreeCAD's target: from p0 along the line, 10 + 5 = 15 mm: (15, 0). The fake's frame
    // puts p1 there nudged by its begin (10, 0) / 1000: (15.01, 0).
    let before = a_line("");
    let mut fake = FakeSketches::new();
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap();
    let sketch = gated(&before, &edited);
    assert_eq!(at(&sketch, "p0000001"), [15.01, 0.0]);
    assert_eq!(at(&sketch, "p0000000"), [0.0, 0.0]);
    assert_eq!(edited.modified, [id("p0000001")]);
    assert!(edited.added.is_empty() && edited.removed.is_empty());
    let calls = fake.drag_calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(matches!(calls[0], DragCall::Begin(_)));
    assert_eq!(calls[1..], [DragCall::Move([15.0, 0.0]), DragCall::End]);
    // One move under the solver's own time, not a drag's: an extend isn't a drag.
    assert_eq!(fake.move_timeouts(), [Duration::MAX]);
    // No whole-sketch solve: the edit's solve, after it, is the command's.
    assert!(fake.requests().is_empty());
}

#[test]
fn a_lines_start_and_a_tilted_lines_end_go_along_the_line() {
    // The start of a0, back 5: (−5, 0), nudged by (0, 0) / 1000.
    let before = a_line("");
    let mut fake = FakeSketches::new();
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::Start,
        mm(5.0),
        &mut fake,
    )
    .unwrap();
    assert_eq!(at(&gated(&before, &edited), "p0000000"), [-5.0, 0.0]);
    assert_eq!(edited.modified, [id("p0000000")]);
    // A 3-4-5 line from (0, 0) to (3, 4), its end on by 5: (6, 8), nudged by (3, 4) / 1000.
    let before = doc(
        &[point("p0000000", 0.0, 0.0), point("p0000001", 3.0, 4.0)],
        &[line("a0000000", "p0000000", "p0000001")],
        "",
        "",
        "",
    );
    let mut fake = FakeSketches::new();
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap();
    assert_eq!(fake.drag_calls()[1], DragCall::Move([6.0, 8.0]));
    assert_eq!(at(&gated(&before, &edited), "p0000001"), [6.003, 8.004]);
    // Shortened by 2: (1.8, 2.4), nudged likewise.
    let mut fake = FakeSketches::new();
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(-2.0),
        &mut fake,
    )
    .unwrap();
    let got = at(&gated(&before, &edited), "p0000001");
    near(got, [1.8 + 0.003, 2.4 + 0.004]);
}

#[test]
fn a_line_shortened_to_nothing_or_past_its_other_end_is_refused_before_any_drag() {
    // 10 − 10 is nothing and 10 − 15 flips it: each refused, naming `by`, with nothing
    // sent. A length is whole nanometres, so the shortest a 10 mm line is left is 1e-6 mm,
    // past the 1e-7 mm floor: made, the frame at the cursor exactly.
    let before = a_line("");
    for by in [-10.0, -15.0] {
        let mut fake = FakeSketches::new();
        let e = extend(
            sketch_of(&before),
            &id("a0000000"),
            End::End,
            mm(by),
            &mut fake,
        )
        .unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{by}");
        assert_eq!(e.parameter(), Some("by"), "{by}");
        assert_eq!(e.elements(), ["a0000000"], "{by}");
        assert!(fake.drag_calls().is_empty(), "{by}");
    }
    let mut fake = FakeSketches::new();
    fake.answer_moves(|begin, to| Ok(frame_at(begin, to)));
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(-9.999999),
        &mut fake,
    )
    .unwrap();
    near(at(&gated(&before, &edited), "p0000001"), [1e-6, 0.0]);
}

#[test]
fn a_drag_that_flips_or_collapses_the_line_is_refused_after_it() {
    // The frame puts the end behind the start, onto it, or 1e-7 mm from it (the floor):
    // refused as the result, naming `by`, the worker's drag ended.
    let before = a_line("");
    for end_at in [[-3.0, 0.0], [0.0, 0.0], [1e-7, 0.0]] {
        let mut fake = FakeSketches::new();
        fake.answer_moves(move |begin, _| Ok(frame_at(begin, end_at)));
        let e = extend(
            sketch_of(&before),
            &id("a0000000"),
            End::End,
            mm(5.0),
            &mut fake,
        )
        .unwrap_err();
        assert_eq!(e.parameter(), Some("by"), "{end_at:?}");
        assert!(e.message().contains("flipped"), "{e}");
        assert_eq!(fake.drag_calls().last(), Some(&DragCall::End));
    }
    // 2e-7 mm from it: made.
    let mut fake = FakeSketches::new();
    fake.answer_moves(|begin, _| Ok(frame_at(begin, [2e-7, 0.0])));
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap();
    assert_eq!(at(&edited.sketch, "p0000001"), [2e-7, 0.0]);
}

#[test]
fn a_drag_whose_frame_isnt_a_success_is_refused_and_ended() {
    let before = a_line("");
    let mut fake = FakeSketches::new();
    fake.answer_moves(|_, _| {
        Ok(Moved {
            status: SolveStatus::Failed,
            values: None,
        })
    });
    let e = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(
        said(&e),
        (
            "line a0000000's end p0000001 can't move by 5 mm: the drag's solve ended Failed",
            Some("by"),
            vec!["a0000000", "p0000001"]
        )
    );
    assert_eq!(e.code(), ErrorCode::InvalidResult);
    assert_eq!(fake.drag_calls().last(), Some(&DragCall::End));
}

#[test]
fn a_frame_that_cant_be_the_sketchs_is_the_workers_fault() {
    // One value for a four-value sketch: the drag client's own check.
    let before = a_line("");
    let mut fake = FakeSketches::new();
    fake.answer_moves(|_, _| {
        Ok(Moved {
            status: SolveStatus::Success,
            values: Some(vec![1.0]),
        })
    });
    let e = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::ProtocolError);
    assert!(
        e.message()
            .starts_with("the sketch worker's drag frame has 1 values for "),
        "{e}"
    );
}

#[test]
fn a_worker_that_fails_the_move_fails_the_edit_with_its_error() {
    let before = a_line("");
    let mut fake = FakeSketches::new();
    fake.answer_moves(|_, _| Err(Error::new(ErrorCode::WorkerCrashed, "the fake crashed")));
    let e = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::WorkerCrashed);
    assert_eq!(e.message(), "the fake crashed");
}

#[test]
fn a_sketch_whose_constraints_conflict_is_refused_naming_them() {
    // The begin finds tag 1, the sketch's one constraint, k0000000, in conflict.
    let before = a_line(r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}"#);
    let mut fake = FakeSketches::new();
    fake.conflicting_drags(vec![Tag(1)]);
    let e = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained);
    assert_eq!(e.elements(), ["k0000000"]);
    // The begin found the conflict, so no move was made, and no drag was left to end.
    assert!(matches!(fake.drag_calls()[..], [DragCall::Begin(_)]));
    // A tag it wasn't sent: the worker's fault.
    let mut fake = FakeSketches::new();
    fake.conflicting_drags(vec![Tag(9)]);
    let e = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::ProtocolError);
}

#[test]
fn an_arcs_end_turns_on_and_its_start_back_about_the_centre() {
    // The quarter arc's end, on by 45°: (10 cos 135°, 10 sin 135°). Its start, back by 45°:
    // (10 cos −45°, 10 sin −45°). Nothing is sent to the solver.
    let before = an_arc();
    let mut fake = FakeSketches::new();
    let edited = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::End,
        degrees(45.0),
        &mut fake,
    )
    .unwrap();
    let h = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
    near(at(&gated(&before, &edited), "e0000000"), [-h, h]);
    assert_eq!(edited.modified, [id("e0000000")]);
    let edited = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::Start,
        degrees(45.0),
        &mut fake,
    )
    .unwrap();
    near(at(&gated(&before, &edited), "s0000000"), [h, -h]);
    assert_eq!(edited.modified, [id("s0000000")]);
    // Shortened by 30° at its end: (10 cos 60°, 10 sin 60°).
    let edited = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::End,
        degrees(-30.0),
        &mut fake,
    )
    .unwrap();
    near(
        at(&gated(&before, &edited), "e0000000"),
        [5.0, 10.0 * 3f64.sqrt() / 2.0],
    );
    assert!(fake.drag_calls().is_empty() && fake.requests().is_empty());
}

#[test]
fn an_arc_turned_to_nothing_past_its_start_or_a_whole_turn_is_refused() {
    // 90° − 90° is nothing, 90° − 120° flips it, 90° + 270° is a whole turn, 90° + 300° past.
    let before = an_arc();
    for by in [-90.0, -120.0, 270.0, 300.0] {
        let mut fake = FakeSketches::new();
        let e = extend(
            sketch_of(&before),
            &id("r0000000"),
            End::End,
            degrees(by),
            &mut fake,
        )
        .unwrap_err();
        assert_eq!(e.parameter(), Some("by"), "{by}");
        assert_eq!(e.elements(), ["r0000000"], "{by}");
    }
    // Just short of a whole turn: made, the ends apart.
    let mut fake = FakeSketches::new();
    let edited = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::End,
        degrees(269.0),
        &mut fake,
    )
    .unwrap();
    let a = 359f64.to_radians();
    near(
        at(&gated(&before, &edited), "e0000000"),
        [10.0 * libm::cos(a), 10.0 * libm::sin(a)],
    );
}

#[test]
fn the_wrong_increment_another_kind_of_member_and_nothing_are_refused() {
    let line_doc = a_line("");
    let mut fake = FakeSketches::new();
    let e = extend(
        sketch_of(&line_doc),
        &id("a0000000"),
        End::End,
        degrees(5.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(
        said(&e),
        (
            "line a0000000 extends by a length, not an angle",
            Some("by"),
            vec!["a0000000"]
        )
    );
    let e = extend(
        sketch_of(&line_doc),
        &id("a0000000"),
        End::End,
        mm(0.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(
        said(&e),
        (
            "extending a0000000 by 0 changes nothing",
            Some("by"),
            vec!["a0000000"]
        )
    );
    let arc_doc = an_arc();
    let e = extend(
        sketch_of(&arc_doc),
        &id("r0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(
        e.message(),
        "arc r0000000 extends by an angle, not a length"
    );
    let e = extend(
        sketch_of(&arc_doc),
        &id("r0000000"),
        End::End,
        degrees(0.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(e.message(), "extending r0000000 by 0 changes nothing");
    let circle_doc = doc(
        &[point("z0000000", 0.0, 0.0)],
        &[],
        &circle("c0000000", "z0000000", 5.0),
        "",
        "",
    );
    let e = extend(
        sketch_of(&circle_doc),
        &id("c0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(
        said(&e),
        (
            "circle c0000000 can't be extended: only a line or an arc can",
            Some("curve"),
            vec!["c0000000"]
        )
    );
    let e = extend(
        sketch_of(&circle_doc),
        &id("z0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(
        e.message(),
        "point z0000000 can't be extended: only a line or an arc can"
    );
    let e = extend(
        sketch_of(&circle_doc),
        &id("q0000000"),
        End::End,
        mm(5.0),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(
        said(&e),
        ("the sketch has no member q0000000", Some("curve"), vec![])
    );
    assert!(fake.drag_calls().is_empty());
}
