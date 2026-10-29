//! `silico_core::extend::next_crossing` and `By::To` on the fake sketch solver (docs/ports/
//! sketch-edit.md, §7; docs/design/m5.0-app.md, open item 12c): the nearest crossing past an
//! extended end, and the end put at a point exactly. Expected values by hand: each sketch's
//! crossings land on exact binary values (a line through (0, 0) and (10, 0) meets x = 15 at
//! t = 1.5; a circle about (0, 0) of radius 10 meets x = ±6 at y = ±8).

use silico_core::document::sketch_edit::Edited;
use silico_core::document::{Coordinate, Document, ElementId, Operation, Sketch, read, validate};
use silico_core::extend::{By, Crossing, End, extend, next_crossing};
use silico_core::fake::{DragCall, FakeSketches};
use silico_core::{Error, ErrorCode};
use silico_protocol::{DragBegin, Moved, PointValues, SketchGeometry, SolveStatus};

use crate::fixture::{arc, circle, document, fields, line, line_as, point};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn doc(points: &[String], lines: &[String], circles: &str, arcs: &str) -> Document {
    read(
        document(&fields(
            &points.join(", "),
            &lines.join(", "),
            circles,
            arcs,
            "",
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

fn xy(x: f64, y: f64) -> [Coordinate; 2] {
    [Coordinate::new(x).unwrap(), Coordinate::new(y).unwrap()]
}

fn crossing(curve: &str, x: f64, y: f64) -> Option<Crossing> {
    Some(Crossing {
        curve: id(curve),
        at: xy(x, y),
    })
}

/// A frame with the dragged point at `xy`, everything else where it began.
fn frame_at(begin: &DragBegin, xy: [f64; 2]) -> Moved {
    let mut values = begin.system.values.clone();
    match begin.system.geometry[begin.point.geometry().0 as usize] {
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

/// A fake whose one move puts the dragged point exactly where it was sent.
fn exact() -> FakeSketches {
    let fake = FakeSketches::new();
    fake.answer_moves(|begin, to| Ok(frame_at(begin, to)));
    fake
}

fn said(error: &Error) -> (ErrorCode, &str, Option<&str>, Vec<&str>, Option<&str>) {
    (
        error.code(),
        error.message(),
        error.parameter(),
        error.elements().iter().map(String::as_str).collect(),
        error.suggestion(),
    )
}

/// Line a0 from p0 at (0, 0) to p1 at (10, 0), and `others`, with their points `more`.
fn a_line_and(more: &[String], others: &[String], circles: &str) -> Document {
    let mut points = vec![point("p0000000", 0.0, 0.0), point("p0000001", 10.0, 0.0)];
    points.extend(more.iter().cloned());
    let mut lines = vec![line("a0000000", "p0000000", "p0000001")];
    lines.extend(others.iter().cloned());
    doc(&points, &lines, circles, "")
}

#[test]
fn a_lines_end_meets_the_nearest_curve_past_it_on_that_curve() {
    // x = 15 and x = 20 past the end, x = −5 past the start; a short line at x = 12 whose
    // own span (y from 1 to 5) the extension misses. The end meets x = 15, the start x = −5.
    let before = a_line_and(
        &[
            point("q0000000", 15.0, -5.0),
            point("q0000001", 15.0, 5.0),
            point("q0000002", 20.0, -5.0),
            point("q0000003", 20.0, 5.0),
            point("q0000004", -5.0, -5.0),
            point("q0000005", -5.0, 5.0),
            point("q0000006", 12.0, 1.0),
            point("q0000007", 12.0, 5.0),
        ],
        &[
            line("b0000000", "q0000000", "q0000001"),
            line("b0000001", "q0000002", "q0000003"),
            line("b0000002", "q0000004", "q0000005"),
            line("b0000003", "q0000006", "q0000007"),
        ],
        "",
    );
    let sketch = sketch_of(&before);
    assert_eq!(
        next_crossing(sketch, &id("a0000000"), End::End).unwrap(),
        crossing("b0000000", 15.0, 0.0)
    );
    assert_eq!(
        next_crossing(sketch, &id("a0000000"), End::Start).unwrap(),
        crossing("b0000002", -5.0, 0.0)
    );
}

#[test]
fn a_line_meets_a_circles_nearer_side_and_a_nearer_construction_line_first() {
    // A circle about (30, 0) of radius 5: the extension meets it at (25, 0), then (35, 0).
    let circled = || circle("c0000000", "z0000000", 5.0);
    let before = a_line_and(&[point("z0000000", 30.0, 0.0)], &[], &circled());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap(),
        crossing("c0000000", 25.0, 0.0)
    );
    // A construction line at x = 20 comes first: construction counts, as trim cuts at it.
    let before = a_line_and(
        &[
            point("z0000000", 30.0, 0.0),
            point("q0000000", 20.0, -1.0),
            point("q0000001", 20.0, 1.0),
        ],
        &[line_as("b0000000", "q0000000", "q0000001", true)],
        &circled(),
    );
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap(),
        crossing("b0000000", 20.0, 0.0)
    );
    // Nothing past the start.
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::Start).unwrap(),
        None
    );
}

#[test]
fn an_end_of_another_curve_within_the_floor_of_the_line_is_met_where_it_is_stored() {
    // b0 ends 3e-8 mm above the extension, at (15, 3e-8): that point, as stored, not the
    // carriers' crossing (15, 0), so the extended end lands on it exactly; and the end goes
    // there exactly, its move sent to it.
    let before = a_line_and(
        &[point("q0000000", 15.0, 3e-8), point("q0000001", 15.0, 5.0)],
        &[line("b0000000", "q0000000", "q0000001")],
        "",
    );
    let sketch = sketch_of(&before);
    let met = next_crossing(sketch, &id("a0000000"), End::End).unwrap();
    assert_eq!(met, crossing("b0000000", 15.0, 3e-8));
    let mut fake = exact();
    let edited = extend(
        sketch,
        &id("a0000000"),
        End::End,
        By::To(met.unwrap().at),
        &mut fake,
    )
    .unwrap();
    assert_eq!(
        fake.drag_calls()[1..],
        [DragCall::Move([15.0, 3e-8]), DragCall::End]
    );
    assert_eq!(at(&gated(&before, &edited), "p0000001"), [15.0, 3e-8]);
    assert_eq!(edited.modified, [id("p0000001")]);
}

#[test]
fn of_curves_met_at_one_place_the_least_id_wins() {
    // x = 15 (b0000001) and a diagonal through (15, 0) (b0000000), both met at (15, 0).
    let before = a_line_and(
        &[
            point("q0000000", 15.0, -5.0),
            point("q0000001", 15.0, 5.0),
            point("q0000002", 14.0, -1.0),
            point("q0000003", 16.0, 1.0),
        ],
        &[
            line("b0000001", "q0000000", "q0000001"),
            line("b0000000", "q0000002", "q0000003"),
        ],
        "",
    );
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap(),
        crossing("b0000000", 15.0, 0.0)
    );
}

/// Arc r0 about z0 at (0, 0), radius 10, from s0 at (10, 0) to e0 at (0, 10), and the
/// vertical lines x = −6 (b0) and x = 6 (b1), from y = −20 to 20.
fn an_arc_and_two_lines() -> Document {
    doc(
        &[
            point("z0000000", 0.0, 0.0),
            point("s0000000", 10.0, 0.0),
            point("e0000000", 0.0, 10.0),
            point("q0000000", -6.0, -20.0),
            point("q0000001", -6.0, 20.0),
            point("q0000002", 6.0, -20.0),
            point("q0000003", 6.0, 20.0),
        ],
        &[
            line("b0000000", "q0000000", "q0000001"),
            line("b0000001", "q0000002", "q0000003"),
        ],
        "",
        &arc("r0000000", "z0000000", "s0000000", "e0000000"),
    )
}

#[test]
fn an_arcs_end_meets_the_nearest_curve_around_its_circle_past_it_not_on_it() {
    // The circle meets x = −6 at (−6, ±8), 126.87° and 233.13°, and x = 6 at (6, ±8),
    // 53.13° and 306.87°. On from the end (90°): (6, 8) is on the arc, not past it; (−6, 8)
    // is 36.87° on, nearest. Back from the start (0°): (6, −8) is 53.13° back, before
    // (−6, −8) at 126.87°.
    let before = an_arc_and_two_lines();
    let sketch = sketch_of(&before);
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::End).unwrap(),
        crossing("b0000000", -6.0, 8.0)
    );
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::Start).unwrap(),
        crossing("b0000001", 6.0, -8.0)
    );
}

#[test]
fn an_arcs_end_goes_to_the_crossing_exactly() {
    // To (−6, 8): the end put there, bit for bit, the arc now 126.87° about the same centre.
    let before = an_arc_and_two_lines();
    let edited = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::End,
        By::To(xy(-6.0, 8.0)),
        &mut FakeSketches::new(),
    )
    .unwrap();
    let sketch = gated(&before, &edited);
    assert_eq!(at(&sketch, "e0000000"), [-6.0, 8.0]);
    assert_eq!(at(&sketch, "s0000000"), [10.0, 0.0]);
    assert_eq!(at(&sketch, "z0000000"), [0.0, 0.0]);
    assert_eq!(edited.modified, [id("e0000000")]);
    // The start back to (6, −8).
    let edited = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::Start,
        By::To(xy(6.0, -8.0)),
        &mut FakeSketches::new(),
    )
    .unwrap();
    assert_eq!(at(&gated(&before, &edited), "s0000000"), [6.0, -8.0]);
    assert_eq!(edited.modified, [id("s0000000")]);
}

#[test]
fn a_lone_line_meets_nothing_and_a_circle_or_a_missing_member_is_refused() {
    let before = a_line_and(
        &[point("z0000000", 30.0, 0.0)],
        &[],
        &circle("c0000000", "z0000000", 5.0),
    );
    let sketch = sketch_of(&before);
    let lone = a_line_and(&[], &[], "");
    assert_eq!(
        next_crossing(sketch_of(&lone), &id("a0000000"), End::End).unwrap(),
        None
    );
    let e = next_crossing(sketch, &id("c0000000"), End::End).unwrap_err();
    assert_eq!(
        said(&e),
        (
            ErrorCode::InvalidInput,
            "circle c0000000 can't be extended: only a line or an arc can",
            Some("curve"),
            vec!["c0000000"],
            Some("name a line or an arc"),
        )
    );
    let e = next_crossing(sketch, &id("x0000000"), End::End).unwrap_err();
    assert_eq!(said(&e).1, "the sketch has no member x0000000");
}

#[test]
fn a_point_off_the_curves_carrier_its_own_end_or_behind_its_other_end_is_refused() {
    let before = a_line_and(&[], &[], "");
    let sketch = sketch_of(&before);
    let to = |x: f64, y: f64| {
        extend(
            sketch,
            &id("a0000000"),
            End::End,
            By::To(xy(x, y)),
            &mut exact(),
        )
    };
    // 2e-7 mm off the line through its ends.
    assert_eq!(
        said(&to(15.0, 2e-7).unwrap_err()),
        (
            ErrorCode::InvalidInput,
            "line a0000000 can't extend to (15, 0.0000002): the point is 0.0000002 mm from its line, farther than 1e-7 mm",
            Some("by"),
            vec!["a0000000"],
            Some("give a point on its line, such as where it meets another curve"),
        )
    );
    // Its own end: nothing.
    assert_eq!(
        said(&to(10.0, 0.0).unwrap_err()),
        (
            ErrorCode::InvalidInput,
            "extending a0000000 to where its end is changes nothing",
            Some("by"),
            vec!["a0000000"],
            Some("give a point other than its end"),
        )
    );
    // Behind its other end: flipped, 5 mm long.
    assert_eq!(
        said(&to(-5.0, 0.0).unwrap_err()),
        (
            ErrorCode::InvalidInput,
            "extending line a0000000 that far would leave it 5 mm long or flipped: a line must be longer than 1e-7 mm and keep its direction",
            Some("by"),
            vec!["a0000000"],
            Some("give a point farther from its other end, the way the line runs"),
        )
    );
    // Within 1e-7 of the line, but not on it: the end goes to the point as given.
    let mut fake = exact();
    let edited = extend(
        sketch,
        &id("a0000000"),
        End::End,
        By::To(xy(15.0, 1e-7)),
        &mut fake,
    )
    .unwrap();
    assert_eq!(
        fake.drag_calls()[1..],
        [DragCall::Move([15.0, 1e-7]), DragCall::End]
    );
    assert_eq!(at(&edited.sketch, "p0000001"), [15.0, 1e-7]);
}

#[test]
fn an_arcs_point_off_its_circle_or_at_its_other_end_is_refused() {
    let before = an_arc_and_two_lines();
    let sketch = sketch_of(&before);
    let to = |x: f64, y: f64| {
        extend(
            sketch,
            &id("r0000000"),
            End::End,
            By::To(xy(x, y)),
            &mut FakeSketches::new(),
        )
    };
    // (−6, 8.5) is 10.2956 − 10 mm off the circle.
    let off = libm::hypot(6.0, 8.5) - 10.0;
    assert_eq!(
        said(&to(-6.0, 8.5).unwrap_err()),
        (
            ErrorCode::InvalidInput,
            format!("arc r0000000 can't extend to (-6, 8.5): the point is {off} mm from its circle, farther than 1e-7 mm").as_str(),
            Some("by"),
            vec!["r0000000"],
            Some("give a point on its circle, such as where it meets another curve"),
        )
    );
    // Its start: no sweep left.
    assert_eq!(
        said(&to(10.0, 0.0).unwrap_err()),
        (
            ErrorCode::InvalidInput,
            "extending arc r0000000 that far would leave it 0 mm long or flipped: an arc must be longer than 1e-7 mm and keep its direction",
            Some("by"),
            vec!["r0000000"],
            Some("give a point farther around its circle from its other end"),
        )
    );
}
