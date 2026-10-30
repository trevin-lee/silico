//! Hostile inputs for `silico_core::extend::next_crossing` and `By::To` on the fake sketch
//! solver, from break-it. The contract: `next_crossing`'s answer is a point `By::To` takes as
//! it is ("[`By::To`] takes it as it is"), past the end by more than 1e-7 mm, on the nearest
//! other curve; `By::To` puts the end there exactly or refuses naming `by` and the curve.
//! Expected values by hand (crossings at exact binary values), or from the stored points.

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use silico_core::document::sketch_edit::Edited;
use silico_core::document::{Coordinate, Document, ElementId, Operation, Sketch, read, validate};
use silico_core::extend::{By, Crossing, End, extend, next_crossing};
use silico_core::fake::FakeSketches;
use silico_core::{Error, ErrorCode};
use silico_protocol::{DragBegin, Moved, PointValues, SketchGeometry, SolveStatus};

use crate::fixture::{arc, circle, document, fields, line, point_debug};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn doc(points: &[String], lines: &[String], circles: &[String], arcs: &[String]) -> Document {
    read(
        document(&fields(
            &points.join(", "),
            &lines.join(", "),
            &circles.join(", "),
            &arcs.join(", "),
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

/// The gate's refusals of `document`, as messages.
fn gate_of(document: &Document) -> Vec<String> {
    match validate(document) {
        Ok(()) => Vec::new(),
        Err(invalid) => invalid
            .errors()
            .iter()
            .map(|e| e.message().to_owned())
            .collect(),
    }
}

/// `document` with the edit's sketch in place, refused by the gate or not.
fn gate_edit(document: &Document, edited: &Edited) -> Vec<String> {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(edited.sketch.clone());
    gate_of(&document)
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

fn mm(c: [Coordinate; 2]) -> [f64; 2] {
    [c[0].mm(), c[1].mm()]
}

fn p(name: &str, at: [f64; 2]) -> String {
    point_debug(name, at[0], at[1])
}

/// A frame with the dragged point at `xy`, everything else where it began.
fn frame_at(begin: &DragBegin, xy: [f64; 2]) -> Moved {
    let mut values = begin.system.values.clone();
    let silico_protocol::Dragged::Point(point) = begin.dragged else {
        panic!("an extend drags a point")
    };
    match begin.system.geometry[point.geometry().0 as usize] {
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

/// `next_crossing`'s answer for `curve`'s `end`, taken by `By::To` as it is: the edit, or
/// its refusal. Fails the test if there's no crossing.
fn extended_to_next(
    document: &Document,
    curve: &str,
    end: End,
) -> (Crossing, Result<Edited, Error>) {
    let sketch = sketch_of(document);
    let met = next_crossing(sketch, &id(curve), end)
        .unwrap()
        .expect("a crossing");
    let edit = extend(sketch, &id(curve), end, By::To(met.at), &mut exact());
    (met, edit)
}

// ---------------------------------------------------------------------------------------
// next_crossing's answer refused by By::To.

#[test]
fn an_arcs_end_off_its_start_radius_is_sent_a_crossing_by_to_refuses() {
    // Arc r0 about (0, 0) from (10, 0) to (0, 10.5): the gate leaves an end off its circle to
    // the solve. next_crossing measures the arc's circle from its start (radius 10), and
    // finds x = −6 at (−6, 8); By::To measures it from the end that moves (radius 10.5), so
    // the point next_crossing gave is 0.5 mm off "its circle".
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [10.0, 0.0]),
            p("e0000000", [0.0, 10.5]),
            p("q0000000", [-6.0, -20.0]),
            p("q0000001", [-6.0, 20.0]),
        ],
        &[line("b0000000", "q0000000", "q0000001")],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    let (met, edit) = extended_to_next(&before, "r0000000", End::End);
    assert_eq!(met.curve, id("b0000000"));
    assert_eq!(mm(met.at), [-6.0, 8.0]);
    // Before its fix: refused "arc r0000000 can't extend to (-6, 8): the point is 0.5 mm from its
    // circle, farther than 1e-7 mm" for the very point next_crossing gave.
    let edited = edit.unwrap();
    assert_eq!(at(&edited.sketch, "e0000000"), [-6.0, 8.0]);
}

#[test]
fn an_arcs_end_an_ulp_inside_its_start_radius_misses_a_tangent_line_next_crossing_found() {
    // Arc r0 about (0, 0) from (10, 0) to an end at the first whole degree whose point
    // (10 cos, 10 sin) is an ulp or so inside radius 10, as a solved arc's end is off its
    // start's radius. The line y = Y, Y the largest double with Y − 10 ≤ 1e-7, touches the
    // circle within the tolerance at its foot (0, Y): next_crossing's tangent contact, 1e-7
    // mm from radius 10. By::To measures (0, Y) from the end's own radius: over 1e-7.
    let end = (1..90)
        .map(|k| {
            let a = f64::from(k).to_radians();
            [10.0 * libm::cos(a), 10.0 * libm::sin(a)]
        })
        .find(|e| libm::hypot(e[0], e[1]) < 10.0)
        .expect("an end inside radius 10");
    let mut y: f64 = 10.0 + 1e-7;
    while y - 10.0 > 1e-7 {
        y = y.next_down();
    }
    while (y.next_up() - 10.0) <= 1e-7 {
        y = y.next_up();
    }
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [10.0, 0.0]),
            p("e0000000", end),
            p("q0000000", [-20.0, y]),
            p("q0000001", [20.0, y]),
        ],
        &[line("b0000000", "q0000000", "q0000001")],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    let (met, edit) = extended_to_next(&before, "r0000000", End::End);
    assert_eq!(met.curve, id("b0000000"));
    assert_eq!(mm(met.at), [0.0, y]);
    // Before its fix: refused as off its circle by 1e-7 mm and an ulp or so.
    let edited = edit.unwrap();
    assert_eq!(at(&edited.sketch, "e0000000"), [0.0, y]);
}

// ---------------------------------------------------------------------------------------
// Tangent continuations: the curve an end already runs into.

/// Line a0 from (−10, g) to the circle about (0, 10) of radius 10, where y = g meets it on
/// the left (x = −√(20g − g²)), and arc r0 from there, counter-clockwise, to (10, 10): the
/// line runs into the arc at its start, its contact within g of tangent. And a line x = 20
/// (b0) ahead of both.
fn line_into_an_arc(g: f64) -> Document {
    let x = -libm::sqrt(20.0 * g - g * g);
    doc(
        &[
            p("p0000000", [-10.0, g]),
            p("p0000001", [x, g]),
            p("z0000000", [0.0, 10.0]),
            p("e0000000", [10.0, 10.0]),
            p("q0000000", [20.0, -5.0]),
            p("q0000001", [20.0, 5.0]),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "q0000000", "q0000001"),
        ],
        &[],
        &[arc("r0000000", "z0000000", "p0000001", "e0000000")],
    )
}

#[test]
fn a_line_running_tangent_into_an_arc_meets_the_curve_ahead_not_the_arc_it_joins() {
    // g = 1e-12, a solve's tangency: the line and the arc share p1 and run within 1e-12 mm
    // of each other for √(20 · 1e-12) = 4.5e-6 mm past it. They touch at the end; the next
    // curve the extension meets is x = 20, at (20, g).
    let before = line_into_an_arc(1e-12);
    assert_eq!(gate_of(&before), Vec::<String>::new());
    let met = next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap();
    // Before its fix: the arc it already joins, at the tangent contact's foot (0, 1e-12), 4.5e-6 mm
    // past the end: an extension of 4.5 µm "to the next curve".
    assert_eq!(
        met,
        Some(Crossing {
            curve: id("b0000000"),
            at: xy(20.0, 1e-12),
        })
    );
}

/// Arc r0 about Z = (123.4, −56.7), radius 1,000, from its point at 0° counter-clockwise to
/// its point at `angle`, and line b0 from there along the tangent, 300 mm on, built as a
/// solve leaves them (cos and sin, so tangent to an ulp of a thousand, not exactly). Nothing
/// else: the arc's circle meets no other curve.
fn arc_into_a_line(angle: f64) -> Document {
    let z = [123.4, -56.7];
    let (s, c) = (libm::sin(angle), libm::cos(angle));
    let end = [z[0] + 1000.0 * c, z[1] + 1000.0 * s];
    let far = [end[0] - 300.0 * s, end[1] + 300.0 * c];
    doc(
        &[
            p("z0000000", z),
            p("s0000000", [z[0] + 1000.0, z[1]]),
            p("e0000000", end),
            p("q0000000", far),
        ],
        &[line("b0000000", "e0000000", "q0000000")],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    )
}

#[test]
fn an_arc_running_tangent_into_a_line_meets_nothing_but_the_line_it_joins() {
    // At 40 angles between 20° and 80°, the arc's end runs tangent into line b0, which
    // shares its end point. Nothing is ahead but b0's touching at the end, which is no
    // crossing past it: None. Any answer is a micro-extension along the line it joins.
    let mut wrong = Vec::new();
    for k in 0..40 {
        let angle = (20.0 + 1.5 * f64::from(k)).to_radians();
        let before = arc_into_a_line(angle);
        assert_eq!(gate_of(&before), Vec::<String>::new());
        let met = next_crossing(sketch_of(&before), &id("r0000000"), End::End).unwrap();
        if let Some(met) = met {
            let e = at(sketch_of(&before), "e0000000");
            let far = libm::hypot(mm(met.at)[0] - e[0], mm(met.at)[1] - e[1]);
            wrong.push((k, met.curve.to_string(), far));
        }
    }
    // Passes: an exact tangency's foot is within an ulp of a thousand of the end.
    assert_eq!(wrong, Vec::<(i32, String, f64)>::new());
}

#[test]
fn a_line_running_tangent_into_an_arc_as_a_solve_leaves_it_meets_the_curve_ahead() {
    // The line a0 tangent to the circle about Z = (123.4, −56.7) of radius 1,000 at its
    // point at `angle`, coming from 300 mm back along the tangent, and the arc r0 from that
    // point on, counter-clockwise, 0.2 rad. Built with cos and sin, so tangent to an ulp of
    // a thousand. Ahead along the tangent, 500 mm past the point, a long line across it
    // (b0): the next curve.
    let z = [123.4, -56.7];
    let mut wrong = Vec::new();
    for k in 0..40 {
        let angle = (20.0 + 1.5 * f64::from(k)).to_radians();
        let (s, c) = (libm::sin(angle), libm::cos(angle));
        let on = [z[0] + 1000.0 * c, z[1] + 1000.0 * s];
        let back = [on[0] + 300.0 * s, on[1] - 300.0 * c];
        let end = [
            z[0] + 1000.0 * libm::cos(angle + 0.2),
            z[1] + 1000.0 * libm::sin(angle + 0.2),
        ];
        let ahead = [on[0] - 500.0 * s, on[1] + 500.0 * c];
        let before = doc(
            &[
                p("z0000000", z),
                p("p0000000", back),
                p("p0000001", on),
                p("e0000000", end),
                p("q0000000", [ahead[0] - 200.0 * c, ahead[1] - 200.0 * s]),
                p("q0000001", [ahead[0] + 200.0 * c, ahead[1] + 200.0 * s]),
            ],
            &[
                line("a0000000", "p0000000", "p0000001"),
                line("b0000000", "q0000000", "q0000001"),
            ],
            &[],
            &[arc("r0000000", "z0000000", "p0000001", "e0000000")],
        );
        assert_eq!(gate_of(&before), Vec::<String>::new());
        let met = next_crossing(sketch_of(&before), &id("a0000000"), End::End)
            .unwrap()
            .unwrap();
        if met.curve != id("b0000000") {
            let far = libm::hypot(mm(met.at)[0] - on[0], mm(met.at)[1] - on[1]);
            wrong.push((k, met.curve.to_string(), far));
        }
    }
    // Passes: an exact tangency's foot is within an ulp of a thousand of the end.
    assert_eq!(wrong, Vec::<(i32, String, f64)>::new());
}

// ---------------------------------------------------------------------------------------
// Overlaps: a collinear neighbour.

#[test]
fn a_line_chained_to_a_collinear_line_is_sent_through_it_to_a_place_the_edit_refuses() {
    // a0 from (0, 0) to (10, 0), a1 from (10, 0) on to (20, 0), sharing p1: a straight chain.
    // next_crossing calls a1's far end (20, 0) a crossing; By::To drags p1 there, taking a1's
    // start with it, so a1 has no length.
    let before = doc(
        &[
            p("p0000000", [0.0, 0.0]),
            p("p0000001", [10.0, 0.0]),
            p("p0000002", [20.0, 0.0]),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("a0000001", "p0000001", "p0000002"),
        ],
        &[],
        &[],
    );
    let sketch = sketch_of(&before);
    let met = next_crossing(sketch, &id("a0000000"), End::End).unwrap();
    // Before its fix: Some(a0000001 at (20, 0)), an end the extension overlaps the whole of a1 to
    // reach, which no edit can take. A collinear neighbour's far end isn't a crossing past
    // the end: the extension runs along it from the end.
    assert_eq!(met, None);
}

#[test]
fn a_line_overlapping_a_collinear_line_meets_nothing_along_it() {
    // b0 from (5, 0) to (20, 0) overlaps a0 (0 to 10) from 5. The extension would run along
    // b0 to its far end (20, 0), so b0 isn't met there, and nothing else is.
    let before = doc(
        &[
            p("p0000000", [0.0, 0.0]),
            p("p0000001", [10.0, 0.0]),
            p("q0000000", [5.0, 0.0]),
            p("q0000001", [20.0, 0.0]),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "q0000000", "q0000001"),
        ],
        &[],
        &[],
    );
    let met = next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap();
    assert_eq!(met, None);
}

// ---------------------------------------------------------------------------------------
// Members the gate takes that next_crossing can't read.

#[test]
fn an_arc_whose_ends_are_at_one_angle_is_called_no_arc() {
    // Arc r0 about (0, 0) from (10, 0) to (20, 0): its ends at one angle, different radii.
    // The gate leaves it to the solve; next_crossing can't make an arc of it, and says it
    // isn't one.
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [10.0, 0.0]),
            p("e0000000", [20.0, 0.0]),
        ],
        &[],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    // Before its fix: "arc r0000000 can't be extended: only a line or an arc can" — it is an
    // arc; the message says its ends make no sweep about its centre. By::To refuses it alike.
    let want = (
        ErrorCode::InvalidInput,
        "arc r0000000's start and end are at one angle from its centre, so it has no circle to extend around",
        Some("curve"),
        vec!["r0000000"],
        Some("move one of its ends around its centre"),
    );
    let e = next_crossing(sketch_of(&before), &id("r0000000"), End::End).unwrap_err();
    assert_eq!(said(&e), want);
    let e = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::End,
        By::To(xy(0.0, 10.0)),
        &mut FakeSketches::new(),
    )
    .unwrap_err();
    assert_eq!(said(&e), want);
}

// ---------------------------------------------------------------------------------------
// Signed zeros and ulps at the end.

#[test]
fn a_line_sent_to_its_own_end_written_with_a_negative_zero_changes_nothing() {
    // The end at (10, 0); the target (10, −0): the same point.
    let before = doc(
        &[p("p0000000", [0.0, 0.0]), p("p0000001", [10.0, 0.0])],
        &[line("a0000000", "p0000000", "p0000001")],
        &[],
        &[],
    );
    let edit = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::To(xy(10.0, -0.0)),
        &mut exact(),
    );
    // Before its fix: Ok, p0000001 "modified" to (10, -0), which the file writes as "-0".
    let e = edit.unwrap_err();
    assert_eq!(
        said(&e).1,
        "extending a0000000 to where its end is changes nothing"
    );
}

#[test]
fn an_arc_sent_to_its_own_end_written_with_a_negative_zero_changes_nothing() {
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [10.0, 0.0]),
            p("e0000000", [0.0, 10.0]),
        ],
        &[],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    let edit = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::End,
        By::To(xy(-0.0, 10.0)),
        &mut FakeSketches::new(),
    );
    // Before its fix: Ok, e0000000 "modified" to (-0, 10).
    let e = edit.unwrap_err();
    assert_eq!(
        said(&e).1,
        "extending r0000000 to where its end is changes nothing"
    );
}

#[test]
fn a_line_sent_an_ulp_past_its_end_moves_it_that_ulp() {
    // Exactly as given: (10 + ulp, 0) is a point other than the end.
    let before = doc(
        &[p("p0000000", [0.0, 0.0]), p("p0000001", [10.0, 0.0])],
        &[line("a0000000", "p0000000", "p0000001")],
        &[],
        &[],
    );
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::To(xy(10f64.next_up(), 0.0)),
        &mut exact(),
    )
    .unwrap();
    assert_eq!(at(&edited.sketch, "p0000001"), [10f64.next_up(), 0.0]);
    assert_eq!(edited.modified, [id("p0000001")]);
    assert_eq!(gate_edit(&before, &edited), Vec::<String>::new());
}

// ---------------------------------------------------------------------------------------
// Huge and far targets.

#[test]
fn a_point_on_the_line_past_the_reach_is_refused_for_the_reach() {
    let before = doc(
        &[p("p0000000", [0.0, 0.0]), p("p0000001", [10.0, 10.0])],
        &[line("a0000000", "p0000000", "p0000001")],
        &[],
        &[],
    );
    let to = |x: f64, y: f64| {
        extend(
            sketch_of(&before),
            &id("a0000000"),
            End::End,
            By::To(xy(x, y)),
            &mut exact(),
        )
        .unwrap_err()
    };
    // (2e6, 2e6) is on the line, 1e6 past the reach.
    let e = to(2e6, 2e6);
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.parameter(), Some("by"));
    assert!(
        e.message().contains("past the sketch's reach"),
        "{:?}",
        said(&e)
    );
    // (f64::MAX, f64::MAX) is on the line too.
    let e = to(f64::MAX, f64::MAX);
    // Before its fix: "the point is inf mm from its line" (its projection overflows), though the
    // point is on the line: the reach is what's wrong with it.
    assert!(
        e.message().contains("past the sketch's reach"),
        "{:?}",
        said(&e)
    );
}

#[test]
fn a_crossing_past_the_reach_isnt_offered_and_by_to_there_is_refused_for_the_reach() {
    // a0 from (9.5e5, 0) to (9.9e5, 0), inside the circle about (9e5, 0) of radius 5e5,
    // which the extension leaves at (1.4e6, 0), past the reach: not offered, and the end
    // sent there is refused for the reach.
    let before = doc(
        &[
            p("p0000000", [9.5e5, 0.0]),
            p("p0000001", [9.9e5, 0.0]),
            p("z0000000", [9e5, 0.0]),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &[circle("c0000000", "z0000000", 5e5)],
        &[],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap(),
        None
    );
    let e = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::To(xy(1.4e6, 0.0)),
        &mut FakeSketches::new(),
    )
    .unwrap_err();
    assert_eq!(e.parameter(), Some("by"));
    assert!(
        e.message().contains("past the sketch's reach"),
        "{:?}",
        said(&e)
    );
}

// ---------------------------------------------------------------------------------------
// Ties and order.

#[test]
fn two_curves_met_within_the_floor_of_each_other() {
    // b0 crosses at x = 15 + 5e-8 and b1 at x = 15: one place as the gate and trim count it.
    // The nearer wins: only at the same distance does the least ID.
    let before = doc(
        &[
            p("p0000000", [0.0, 0.0]),
            p("p0000001", [10.0, 0.0]),
            p("q0000000", [15.0 + 5e-8, -5.0]),
            p("q0000001", [15.0 + 5e-8, 5.0]),
            p("q0000002", [15.0, -5.0]),
            p("q0000003", [15.0, 5.0]),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "q0000000", "q0000001"),
            line("b0000001", "q0000002", "q0000003"),
        ],
        &[],
        &[],
    );
    let met = next_crossing(sketch_of(&before), &id("a0000000"), End::End)
        .unwrap()
        .unwrap();
    // The nearer, b1.
    assert_eq!(met.curve, id("b0000001"));
}

#[test]
fn the_answer_is_the_same_whatever_order_the_file_lists_the_curves_in() {
    // Two crossings at one point by two lines, listed both ways, and a circle and an arc.
    let pts = [
        p("p0000000", [0.0, 0.0]),
        p("p0000001", [10.0, 0.0]),
        p("q0000000", [15.0, -5.0]),
        p("q0000001", [15.0, 5.0]),
        p("q0000002", [14.0, -1.0]),
        p("q0000003", [16.0, 1.0]),
        p("z0000000", [40.0, 0.0]),
    ];
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000002", "q0000000", "q0000001"),
        line("b0000001", "q0000002", "q0000003"),
    ];
    let circles = [circle("c0000000", "z0000000", 30.0)];
    let one = doc(&pts, &lines, &circles, &[]);
    let mut reversed = lines.to_vec();
    reversed.reverse();
    let mut pts_rev = pts.to_vec();
    pts_rev.reverse();
    let two = doc(&pts_rev, &reversed, &circles, &[]);
    let a = next_crossing(sketch_of(&one), &id("a0000000"), End::End).unwrap();
    let b = next_crossing(sketch_of(&two), &id("a0000000"), End::End).unwrap();
    assert_eq!(a, b);
    // The circle meets the line at (10, 0), a0's own end: not past it.
    assert_eq!(
        a,
        Some(Crossing {
            curve: id("b0000001"),
            at: xy(15.0, 0.0),
        })
    );
}

// ---------------------------------------------------------------------------------------
// An arc at the edge of a whole turn.

#[test]
fn an_arcs_crossing_just_short_of_its_other_end_extends_it_to_nearly_a_turn() {
    // Arc r0 about (0, 0), radius 10, from (10, 0) to (0, 10) (a quarter). A line through
    // (10, −d) for d small, vertical: it meets the circle at (√(100 − d²), −d), just short of
    // the start going round from the end, and runs on to the right, away from the circle.
    // Extending the end there sweeps the arc to nearly a turn, its ends ~d apart. For
    // d = 2e-7, past the floor: Ok, gated; for d = 5e-8, within: nothing is offered.
    for (d, offered) in [(2e-7, true), (5e-8, false)] {
        let x = libm::sqrt(100.0 - d * d);
        let before = doc(
            &[
                p("z0000000", [0.0, 0.0]),
                p("s0000000", [10.0, 0.0]),
                p("e0000000", [0.0, 10.0]),
                p("q0000000", [x, -d]),
                p("q0000001", [x + 30.0, -d]),
            ],
            &[line("b0000000", "q0000000", "q0000001")],
            &[],
            &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
        );
        let sketch = sketch_of(&before);
        let met = next_crossing(sketch, &id("r0000000"), End::End).unwrap();
        if !offered {
            assert_eq!(met, None);
            continue;
        }
        let met = met.unwrap();
        // b0's own end, as stored, 270° on less a hair.
        assert_eq!(mm(met.at), [x, -d]);
        let edited = extend(
            sketch,
            &id("r0000000"),
            End::End,
            By::To(met.at),
            &mut exact(),
        )
        .unwrap();
        assert_eq!(gate_edit(&before, &edited), Vec::<String>::new());
    }
}

// ---------------------------------------------------------------------------------------
// Arcs of one circle, and a drag that lands short.

#[test]
fn an_arc_meets_the_next_arc_of_its_own_circle_at_its_end() {
    // Arcs A (r0, 0° to 90°) and B (r1, 180° to 270°) of the circle about (0, 0), radius 10:
    // their carriers overlap, so no crossing; B's ends are on A's circle. A's end, going on
    // from 90°, meets B's start (−10, 0) at 180°; A's start, going back from 0°, meets B's end
    // (0, −10) at 270°. And A goes there exactly, gated.
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [10.0, 0.0]),
            p("e0000000", [0.0, 10.0]),
            p("s0000001", [-10.0, 0.0]),
            p("e0000001", [0.0, -10.0]),
        ],
        &[],
        &[],
        &[
            arc("r0000000", "z0000000", "s0000000", "e0000000"),
            arc("r0000001", "z0000000", "s0000001", "e0000001"),
        ],
    );
    let sketch = sketch_of(&before);
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::End).unwrap(),
        Some(Crossing {
            curve: id("r0000001"),
            at: xy(-10.0, 0.0),
        })
    );
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::Start).unwrap(),
        Some(Crossing {
            curve: id("r0000001"),
            at: xy(0.0, -10.0),
        })
    );
    let (_, edit) = extended_to_next(&before, "r0000000", End::End);
    let edited = edit.unwrap();
    assert_eq!(at(&edited.sketch, "e0000000"), [-10.0, 0.0]);
    assert_eq!(gate_edit(&before, &edited), Vec::<String>::new());
}

#[test]
fn a_line_whose_drag_lands_short_of_the_point_is_written_at_the_point() {
    // The worker's move puts the end halfway to (15, 0), at (12.5, 0), as constraints that
    // win would. The edit writes what its request named (Fable, 2026-10-29: an edit writes
    // exactly what it names, the drag's frame seeding the rest): the end at (15, 0), on the
    // curve it was sent to; the command's solve then moves it back where constraints pull.
    let before = doc(
        &[
            p("p0000000", [0.0, 0.0]),
            p("p0000001", [10.0, 0.0]),
            p("q0000000", [15.0, -5.0]),
            p("q0000001", [15.0, 5.0]),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "q0000000", "q0000001"),
        ],
        &[],
        &[],
    );
    let mut fake = FakeSketches::new();
    fake.answer_moves(|begin, to| Ok(frame_at(begin, [(10.0 + to[0]) / 2.0, to[1] / 2.0])));
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::To(xy(15.0, 0.0)),
        &mut fake,
    )
    .unwrap();
    assert_eq!(at(&edited.sketch, "p0000001"), [15.0, 0.0]);
}

// ---------------------------------------------------------------------------------------
// The composition, over random sketches biased to the floor.

/// A gap within a few ulps of 1e-7, or well under or over it.
fn gap() -> impl Strategy<Value = f64> {
    prop_oneof![
        (-6i32..=6).prop_map(|k| {
            let mut g = 1e-7f64;
            for _ in 0..k.unsigned_abs() {
                g = if k > 0 { g.next_up() } else { g.next_down() };
            }
            g
        }),
        (0.0..1e-7f64),
        Just(0.0),
        (1e-12..1e-9f64),
    ]
}

fn scale() -> impl Strategy<Value = f64> {
    prop_oneof![Just(1.0), Just(1e3), Just(1e5), Just(4e5), (1.0..5e5f64)]
}

/// The extension of `curve`'s `end` (its moving point `moving`) to `next_crossing`'s answer:
/// none, or taken by By::To and landed on exactly, or refused for the reach or by the gate.
/// Anything else (By::To calling the point off the carrier, or nothing, or flipped) is a
/// point next_crossing shouldn't have offered.
fn composes(before: &Document, curve: &str, moving: &str, end: End) -> Result<(), String> {
    let sketch = sketch_of(before);
    let Some(met) = next_crossing(sketch, &id(curve), end).unwrap() else {
        return Ok(());
    };
    match extend(sketch, &id(curve), end, By::To(met.at), &mut exact()) {
        Ok(edited) if at(&edited.sketch, moving) == mm(met.at) => Ok(()),
        Ok(edited) => Err(format!("{:?} for {met:?}", at(&edited.sketch, moving))),
        Err(e)
            if e.message().contains("past the sketch's reach")
                || e.message().contains("the gate refuses") =>
        {
            Ok(())
        }
        Err(e) => Err(format!("{:?} for {met:?}", said(&e))),
    }
}

/// Line a0 from O (`origin` × `scale`) at `theta`, `length` long, and line b0 from a point
/// `side` × `g` off a0's line, `past` beyond the moving end (a0's start if `start`), 10 mm
/// on at `tilt` to a0.
#[derive(Debug, Clone, Copy)]
struct LineNear {
    origin: (f64, f64),
    scale: f64,
    theta: f64,
    length: f64,
    past: f64,
    g: f64,
    side: f64,
    start: bool,
    tilt: f64,
}

fn line_near_a_line(at: LineNear) -> Document {
    let LineNear {
        origin,
        scale,
        theta,
        length,
        past,
        g,
        side,
        start,
        tilt,
    } = at;
    let o = [origin.0 * scale, origin.1 * scale];
    let (s, c) = (libm::sin(theta), libm::cos(theta));
    let q = [o[0] + length * c, o[1] + length * s];
    // Past the moving end: before o for the start, after q for the end.
    let along = if start { -past } else { length + past };
    let foot = [o[0] + along * c, o[1] + along * s];
    let r = [foot[0] - side * g * s, foot[1] + side * g * c];
    let far = [
        r[0] + 10.0 * libm::cos(theta + tilt),
        r[1] + 10.0 * libm::sin(theta + tilt),
    ];
    doc(
        &[
            p("p0000000", o),
            p("p0000001", q),
            p("q0000000", r),
            p("q0000001", far),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "q0000000", "q0000001"),
        ],
        &[],
        &[],
    )
}

/// Arc r0 about C (`center` × `scale`), `radius`, from `a0` for `sweep` radians, built with
/// cos and sin; line b0 radially out from a point `side` × `g` off its circle, `on` of the
/// way round its gap from the moving end (its start if `start`).
#[derive(Debug, Clone, Copy)]
struct ArcNear {
    center: (f64, f64),
    scale: f64,
    radius: f64,
    a0: f64,
    sweep: f64,
    on: f64,
    g: f64,
    side: f64,
    start: bool,
}

fn arc_near_a_line(at: ArcNear) -> Document {
    let ArcNear {
        center,
        scale,
        radius,
        a0,
        sweep,
        on,
        g,
        side,
        start,
    } = at;
    let c = [center.0 * scale, center.1 * scale];
    let at_angle = |a: f64, r: f64| [c[0] + r * libm::cos(a), c[1] + r * libm::sin(a)];
    let (sa, ea) = (a0, a0 + sweep);
    let gap_angle = std::f64::consts::TAU - sweep;
    let theta = if start {
        sa - on * gap_angle
    } else {
        ea + on * gap_angle
    };
    doc(
        &[
            p("z0000000", c),
            p("s0000000", at_angle(sa, radius)),
            p("e0000000", at_angle(ea, radius)),
            p("q0000000", at_angle(theta, radius + side * g)),
            p("q0000001", at_angle(theta, radius * 2.0 + 1.0)),
        ],
        &[line("b0000000", "q0000000", "q0000001")],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    )
}

#[test]
fn a_lines_start_sent_to_another_lines_end_1e_7_off_its_line_is_refused_as_off_it() {
    // proptest's minimal case: a0 from (−0.2496, 0) at 1.42 rad, 92 mm; b0's end 1e-7 mm
    // off a0's line, 26.5 mm before its start. next_crossing measures b0's end off the line
    // from a0's `from` to its `to` (≤ 1e-7, offered, as stored); By::To at the start
    // measures it from the `to` back to the `from`: 1.0000000255e-7, refused.
    let before = line_near_a_line(LineNear {
        origin: (-0.249_586_492_213_154_32, 0.0),
        scale: 1.0,
        theta: 1.420_153_096_502_498_2,
        length: 91.995_627_621_390_2,
        past: 26.459_145_373_842_8,
        g: 1e-7,
        side: 1.0,
        start: true,
        tilt: 0.1,
    });
    assert_eq!(gate_of(&before), Vec::<String>::new());
    // Before its fix: "line a0000000 can't extend to (-4.220419258614121, -26.159488929553333): the
    // point is 0.00000010000000255337113 mm from its line, farther than 1e-7 mm".
    assert_eq!(
        composes(&before, "a0000000", "p0000000", End::Start),
        Ok(())
    );
}

#[test]
fn an_arcs_end_sent_to_a_lines_end_1e_7_inside_its_circle_is_refused_as_off_it() {
    // proptest's minimal case: arc r0 of radius 0.001 about (−0.9129, 0.7519), 0.1 rad from
    // 5.61 rad; b0's end 1e-7 inside the circle, round the gap from the arc's end.
    // next_crossing measures it from the start's radius (≤ 1e-7); By::To from the end's,
    // cos and sin leave an ulp-level different: 1.00000000025e-7, refused.
    let before = arc_near_a_line(ArcNear {
        center: (-0.912_905_045_989_102_6, 0.751_914_836_881_388),
        scale: 1.0,
        radius: 0.001,
        a0: 5.610_923_718_473_643,
        sweep: 0.1,
        on: 0.05,
        g: 1e-7,
        side: -1.0,
        start: false,
    });
    assert_eq!(gate_of(&before), Vec::<String>::new());
    // Before its fix: "arc r0000000 can't extend to (-0.9119395547659281, 0.7516547855249733): the
    // point is 0.00000010000000002455961 mm from its circle, farther than 1e-7 mm".
    assert_eq!(composes(&before, "r0000000", "e0000000", End::End), Ok(()));
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2000,
        rng_seed: RngSeed::Fixed(0x5ee7_0e11),
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// A line a0 at any angle and place, and another line b0 whose end sits `gap` off a0's
    /// line, past either end of a0: if next_crossing offers a point, By::To takes it, and
    /// the end lands on it exactly.
    #[test]
    fn a_lines_next_crossing_with_an_end_near_its_line_is_taken_by_by_to(
        origin in (-1.0..1.0f64, -1.0..1.0f64),
        scale in scale(),
        theta in 0.0..std::f64::consts::TAU,
        length in prop_oneof![Just(1e-6), (0.001..100.0f64)],
        past in prop_oneof![(1e-6..1e-3f64), (0.01..100.0f64)],
        g in gap(),
        side in prop_oneof![Just(1.0), Just(-1.0)],
        start in any::<bool>(),
        tilt in 0.1..3.0f64,
    ) {
        let before = line_near_a_line(LineNear { origin, scale, theta, length, past, g, side, start, tilt });
        prop_assume!(gate_of(&before).is_empty());
        let (moving, end) = if start { ("p0000000", End::Start) } else { ("p0000001", End::End) };
        // Before its fix: at the start, By::To measures off the line the other way round, and an
        // end next_crossing offers at 1e-7 is refused an ulp past it.
        prop_assert_eq!(composes(&before, "a0000000", moving, end), Ok(()));
    }

    /// As above, at the line's `to` end only, where both measure from `from` to `to`.
    #[test]
    fn a_lines_next_crossing_at_its_end_with_an_end_near_its_line_is_taken_by_by_to(
        origin in (-1.0..1.0f64, -1.0..1.0f64),
        scale in scale(),
        theta in 0.0..std::f64::consts::TAU,
        length in prop_oneof![Just(1e-6), (0.001..100.0f64)],
        past in prop_oneof![(1e-6..1e-3f64), (0.01..100.0f64)],
        g in gap(),
        side in prop_oneof![Just(1.0), Just(-1.0)],
        tilt in 0.1..3.0f64,
    ) {
        let before = line_near_a_line(LineNear { origin, scale, theta, length, past, g, side, start: false, tilt });
        prop_assume!(gate_of(&before).is_empty());
        prop_assert_eq!(composes(&before, "a0000000", "p0000001", End::End), Ok(()));
    }

    /// An arc at any place, scale, and sweep, built with cos and sin as a solve leaves it, and
    /// a line whose end sits `gap` off the arc's circle (measured from its centre), in the
    /// gap past the moving end: if next_crossing offers a point, By::To takes it.
    #[test]
    fn an_arcs_next_crossing_with_an_end_near_its_circle_is_taken_by_by_to(
        center in (-1.0..1.0f64, -1.0..1.0f64),
        scale in scale(),
        radius in prop_oneof![Just(1e-3), (0.01..1000.0f64)],
        a0 in 0.0..std::f64::consts::TAU,
        sweep in 0.1..5.0f64,
        on in 0.05..0.95f64,
        g in gap(),
        side in prop_oneof![Just(1.0), Just(-1.0)],
        start in any::<bool>(),
    ) {
        let before = arc_near_a_line(ArcNear { center, scale, radius, a0, sweep, on, g, side, start });
        prop_assume!(gate_of(&before).is_empty());
        let (moving, end) = if start { ("s0000000", End::Start) } else { ("e0000000", End::End) };
        // Before its fix: at the end, By::To measures the circle from the end's radius, an ulp or so
        // off the start's that next_crossing measures from.
        prop_assert_eq!(composes(&before, "r0000000", moving, end), Ok(()));
    }

    /// As above, at the arc's start only, where both measure from the start, though one way
    /// each: next_crossing's radius is √(dx² + dy²), By::To's `hypot`, an ulp apart.
    /// Before its fix: center (0, −996.6), radius 565.118, 4.01 rad from 3.88, a line's end 1e-7
    /// inside: refused at 1.0000000793e-7 mm off.
    #[test]
    fn an_arcs_next_crossing_at_its_start_with_an_end_near_its_circle_is_taken_by_by_to(
        center in (-1.0..1.0f64, -1.0..1.0f64),
        scale in scale(),
        radius in prop_oneof![Just(1e-3), (0.01..1000.0f64)],
        a0 in 0.0..std::f64::consts::TAU,
        sweep in 0.1..5.0f64,
        on in 0.05..0.95f64,
        g in gap(),
        side in prop_oneof![Just(1.0), Just(-1.0)],
    ) {
        let before = arc_near_a_line(ArcNear { center, scale, radius, a0, sweep, on, g, side, start: true });
        prop_assume!(gate_of(&before).is_empty());
        prop_assert_eq!(composes(&before, "r0000000", "s0000000", End::Start), Ok(()));
    }

    /// A line along y = 0 from (0, 0) to (10, 0) and up to six vertical segments x = k_i,
    /// y in [lo_i, hi_i], k_i whole or half millimetres: the answer, by hand, is the least
    /// k_i > 10 whose span holds 0 (an end at y = 0 counts, within 1e-7 of the line), the
    /// least ID of those at one k.
    #[test]
    fn a_lines_next_crossing_among_vertical_segments_is_the_least_x_past_its_end(
        segments in prop::collection::vec((-40i32..80, -10i32..10, 0i32..10), 0..6),
    ) {
        let mut pts = vec![p("p0000000", [0.0, 0.0]), p("p0000001", [10.0, 0.0])];
        let mut lines = vec![line("a0000000", "p0000000", "p0000001")];
        let mut want: Option<(f64, Vec<String>)> = None;
        for (i, (k2, lo, len)) in segments.iter().enumerate() {
            let x = f64::from(*k2) / 2.0;
            let (lo, hi) = (f64::from(*lo), f64::from(*lo + *len + 1));
            let name = format!("b{i:07}");
            let (from, to) = (format!("q{:07}", 2 * i), format!("q{:07}", 2 * i + 1));
            pts.push(p(&from, [x, lo]));
            pts.push(p(&to, [x, hi]));
            lines.push(line(&name, &from, &to));
            let spans = lo <= 0.0 && 0.0 <= hi;
            // Past the end means more than 1e-7 from (10, 0); x = 10 sits on it.
            if spans && x > 10.0 && want.as_ref().is_none_or(|(bx, _)| x < *bx) {
                want = Some((x, Vec::new()));
            }
            if spans
                && let Some((bx, names)) = want.as_mut()
                && x == *bx
            {
                names.push(name);
            }
        }
        let before = doc(&pts, &lines, &[], &[]);
        prop_assume!(gate_of(&before).is_empty());
        let got = next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap();
        match (got, want) {
            (None, None) => {}
            (Some(got), Some((x, names))) => {
                // Any of the segments at the least x (their crossings round alike or by an
                // ulp, and the nearer wins), at (x, 0) to 1e-12.
                prop_assert!(names.contains(&got.curve.to_string()), "{got:?} not of {names:?}");
                prop_assert!((got.at[0].mm() - x).abs() <= 1e-12 && got.at[1].mm() == 0.0, "{got:?}");
            }
            (got, want) => prop_assert!(false, "got {got:?}, want {want:?}"),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Curves built on the moving end: they move with it, so they aren't met.

#[test]
fn a_line_joined_to_an_arc_meets_the_curve_ahead_not_the_arc_it_drags() {
    // a0 from (−20, 0) to p1 at (−10, 0); r0 about (0, 0) from p1 round to (10, 0); b0 is
    // x = 20. The extension crosses r0's circle at (10, 0), r0's own end, but r0 starts at
    // the moving end and goes with it: b0 at (20, 0) is next.
    let before = doc(
        &[
            p("p0000000", [-20.0, 0.0]),
            p("p0000001", [-10.0, 0.0]),
            p("z0000000", [0.0, 0.0]),
            p("e0000000", [10.0, 0.0]),
            p("q0000000", [20.0, -5.0]),
            p("q0000001", [20.0, 5.0]),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "q0000000", "q0000001"),
        ],
        &[],
        &[arc("r0000000", "z0000000", "p0000001", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    // Before its fix: r0000000 at (10, 0), which By::To then refused, r0's ends both there.
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap(),
        Some(Crossing {
            curve: id("b0000000"),
            at: xy(20.0, 0.0),
        })
    );
}

#[test]
fn a_line_joined_to_a_wide_arc_is_not_sent_into_it() {
    // r0 about (0, 0) from p1 at (−10, 0) round 270° to (0, 10): the extension meets its
    // circle at (10, 0), inside the arc. Before its fix: offered, and By::To took it,
    // leaving r0 a quarter of what it was, the gate passing it.
    let before = doc(
        &[
            p("p0000000", [-20.0, 0.0]),
            p("p0000001", [-10.0, 0.0]),
            p("z0000000", [0.0, 0.0]),
            p("e0000000", [0.0, 10.0]),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &[],
        &[arc("r0000000", "z0000000", "p0000001", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap(),
        None
    );
}

#[test]
fn an_arc_joined_to_a_line_or_a_circle_about_its_end_meets_neither() {
    // r0 about (0, 0) from (10, 0) to e0 at (0, 10). b0 runs from e0 through (−20, −10),
    // crossing the circle again at (−10, 0), 90° on; c0 is about e0, radius 3, crossing the
    // circle about 17.25° on. Both are built on e0 and move with it: nothing is met.
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [10.0, 0.0]),
            p("e0000000", [0.0, 10.0]),
            p("q0000000", [-20.0, -10.0]),
        ],
        &[line("b0000000", "e0000000", "q0000000")],
        &[circle("c0000000", "e0000000", 3.0)],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("r0000000"), End::End).unwrap(),
        None
    );
    // Its start isn't e0: going back from (10, 0), b0 is met at (−10, 0), 180° back (to
    // rounding: b0 is diagonal).
    let met = next_crossing(sketch_of(&before), &id("r0000000"), End::Start)
        .unwrap()
        .unwrap();
    assert_eq!(met.curve, id("b0000000"));
    let [x, y] = mm(met.at);
    assert!((x + 10.0).abs() <= 1e-12 && y.abs() <= 1e-12, "{x}, {y}");
}

// ---------------------------------------------------------------------------------------
// An arc's crossings: on its own span, along an overlapping arc, and the nearest by angle.

/// The point at `deg` degrees on the circle about (0, 0) of radius 10.
fn on_circle(deg: f64) -> [f64; 2] {
    let a = deg.to_radians();
    [10.0 * libm::cos(a), 10.0 * libm::sin(a)]
}

#[test]
fn a_crossing_on_the_arcs_own_span_is_past_neither_end() {
    // r0 about (0, 0), radius 10, from (10, 0) round 270° to (0, −10). x = −6 from y = 0 to
    // 20 crosses its circle at (−6, 8), 126.87°: on the arc itself. Nothing is past either
    // end.
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [10.0, 0.0]),
            p("e0000000", [0.0, -10.0]),
            p("q0000000", [-6.0, 0.0]),
            p("q0000001", [-6.0, 20.0]),
        ],
        &[line("b0000000", "q0000000", "q0000001")],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    let sketch = sketch_of(&before);
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::End).unwrap(),
        None
    );
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::Start).unwrap(),
        None
    );
}

#[test]
fn an_arc_overlapping_another_of_its_circle_past_its_end_meets_nothing_along_it() {
    // r0 from 0° to 90°. r1 from 300° to 20° overlaps r0's start: going back from 0°, the
    // extension runs along r1 to its start at 300°, so r1 isn't met there. r2 from 70° to
    // 200° overlaps r0's end: going on from 90°, it runs along r2 to 200°.
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", on_circle(0.0)),
            p("e0000000", on_circle(90.0)),
            p("s0000001", on_circle(300.0)),
            p("e0000001", on_circle(20.0)),
        ],
        &[],
        &[],
        &[
            arc("r0000000", "z0000000", "s0000000", "e0000000"),
            arc("r0000001", "z0000000", "s0000001", "e0000001"),
        ],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("r0000000"), End::Start).unwrap(),
        None
    );
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", on_circle(0.0)),
            p("e0000000", on_circle(90.0)),
            p("s0000002", on_circle(70.0)),
            p("e0000002", on_circle(200.0)),
        ],
        &[],
        &[],
        &[
            arc("r0000000", "z0000000", "s0000000", "e0000000"),
            arc("r0000002", "z0000000", "s0000002", "e0000002"),
        ],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("r0000000"), End::End).unwrap(),
        None
    );
}

#[test]
fn an_arcs_nearest_crossing_is_by_angle_not_by_chord() {
    // r0 about (0, 0), radius 10, from 0° to 30°. b0 (y = 0, x from −15 to −5) meets the
    // circle at (−10, 0), 150° on from the end; b1 (y = −5, x from 5 to 15) at (√75, −5),
    // 330°: 300° on from the end, but only 10 mm from it in a straight line. The end goes
    // round to b0 first; the start, back 30°, to b1.
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", on_circle(0.0)),
            p("e0000000", on_circle(30.0)),
            p("q0000000", [-15.0, 0.0]),
            p("q0000001", [-5.0, 0.0]),
            p("q0000002", [5.0, -5.0]),
            p("q0000003", [15.0, -5.0]),
        ],
        &[
            line("b0000000", "q0000000", "q0000001"),
            line("b0000001", "q0000002", "q0000003"),
        ],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    let sketch = sketch_of(&before);
    let end = next_crossing(sketch, &id("r0000000"), End::End)
        .unwrap()
        .unwrap();
    assert_eq!(end.curve, id("b0000000"));
    assert_eq!(mm(end.at), [-10.0, 0.0]);
    let start = next_crossing(sketch, &id("r0000000"), End::Start)
        .unwrap()
        .unwrap();
    assert_eq!(start.curve, id("b0000001"));
    let [x, y] = mm(start.at);
    assert_eq!(y, -5.0);
    assert!((x - 75f64.sqrt()).abs() <= 1e-12, "{x}");
}

// ---------------------------------------------------------------------------------------
// By::To within 1e-7 mm of the other end.

#[test]
fn an_arcs_start_sent_within_the_floor_of_its_end_is_too_short() {
    // r0 from (10, 0) to (0, 10); its start sent to (5e-8, 10), 5e-8 mm round the circle
    // short of its end: what's left is too short, not a whole turn.
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [10.0, 0.0]),
            p("e0000000", [0.0, 10.0]),
        ],
        &[],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    let e = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::Start,
        By::To(xy(5e-8, 10.0)),
        &mut FakeSketches::new(),
    )
    .unwrap_err();
    // Refused before anything is solved, as 10 mm × 5e-9 rad long (to rounding).
    let (code, message, parameter, elements, suggestion) = said(&e);
    assert_eq!(
        (code, parameter, elements, suggestion),
        (
            ErrorCode::InvalidInput,
            Some("by"),
            vec!["r0000000"],
            Some("give a point farther around its circle from its other end"),
        )
    );
    let size = message
        .strip_prefix("extending arc r0000000 that far would leave it ")
        .and_then(|m| {
            m.strip_suffix(
                " mm long or flipped: an arc must be longer than 1e-7 mm and keep its direction",
            )
        })
        .and_then(|m| m.parse::<f64>().ok());
    assert!(size.is_some_and(|s| (s - 5e-8).abs() <= 1e-15), "{message}");
}

#[test]
fn a_line_sent_within_the_floor_of_its_other_end_is_too_short_before_any_drag() {
    let before = doc(
        &[p("p0000000", [0.0, 0.0]), p("p0000001", [10.0, 0.0])],
        &[line("a0000000", "p0000000", "p0000001")],
        &[],
        &[],
    );
    let mut fake = exact();
    let e = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::To(xy(5e-8, 0.0)),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(
        e.suggestion(),
        Some("give a point farther from its other end, the way the line runs"),
        "{:?}",
        said(&e)
    );
    assert_eq!(fake.drag_calls(), []);
}

#[test]
fn a_line_ending_at_an_arcs_moving_end_is_not_met() {
    // b0 runs from (−20, −10) to e0, r0's end at (0, 10), crossing r0's circle again at
    // (−10, 0), 90° on; it ends at e0, so it moves with it.
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [10.0, 0.0]),
            p("e0000000", [0.0, 10.0]),
            p("q0000000", [-20.0, -10.0]),
        ],
        &[line("b0000000", "q0000000", "e0000000")],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("r0000000"), End::End).unwrap(),
        None
    );
}

#[test]
fn an_arc_about_a_lines_moving_end_is_not_met() {
    // r0 about p1 at (−10, 0), radius 5, from (−10, −5) round to (−10, 5): it crosses the
    // extension at (−5, 0), but it's centred on p1 and moves with it.
    let before = doc(
        &[
            p("p0000000", [-20.0, 0.0]),
            p("p0000001", [-10.0, 0.0]),
            p("s0000000", [-10.0, -5.0]),
            p("e0000000", [-10.0, 5.0]),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &[],
        &[arc("r0000000", "p0000001", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap(),
        None
    );
}

#[test]
fn a_line_joined_to_an_arcs_end_meets_the_curve_ahead_not_the_arc() {
    // The first joined test with r0 the other way round: from (10, 0) round to p1 at
    // (−10, 0), ending at the moving end. The extension meets r0's circle at (10, 0), r0's
    // start; b0 at (20, 0) is next.
    let before = doc(
        &[
            p("p0000000", [-20.0, 0.0]),
            p("p0000001", [-10.0, 0.0]),
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [10.0, 0.0]),
            p("q0000000", [20.0, -5.0]),
            p("q0000001", [20.0, 5.0]),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "q0000000", "q0000001"),
        ],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "p0000001")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap(),
        Some(Crossing {
            curve: id("b0000000"),
            at: xy(20.0, 0.0),
        })
    );
}

// ---------------------------------------------------------------------------------------
// Off the axes: a line's y and an arc's centre count.

#[test]
fn a_vertical_line_meets_the_curve_ahead_of_each_end() {
    // a0 from (3, −10) up to (3, −5); y = −2 (b0) ahead of its end, y = −15 (b1) behind
    // its start. Every test along y = 0 left the y terms of the line's direction and of
    // its halfway place untested.
    let before = doc(
        &[
            p("p0000000", [3.0, -10.0]),
            p("p0000001", [3.0, -5.0]),
            p("q0000000", [0.0, -2.0]),
            p("q0000001", [6.0, -2.0]),
            p("q0000002", [0.0, -15.0]),
            p("q0000003", [6.0, -15.0]),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "q0000000", "q0000001"),
            line("b0000001", "q0000002", "q0000003"),
        ],
        &[],
        &[],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    let sketch = sketch_of(&before);
    assert_eq!(
        next_crossing(sketch, &id("a0000000"), End::End).unwrap(),
        Some(Crossing {
            curve: id("b0000000"),
            at: xy(3.0, -2.0),
        })
    );
    assert_eq!(
        next_crossing(sketch, &id("a0000000"), End::Start).unwrap(),
        Some(Crossing {
            curve: id("b0000001"),
            at: xy(3.0, -15.0),
        })
    );
}

#[test]
fn a_vertical_line_overlapping_a_collinear_one_meets_nothing_along_it() {
    // a0 from (3, 0) up to (3, 5); a1 from (3, 4) to (3, 9) overlaps its last millimetre.
    // a1's far end (3, 9) is ahead, but halfway there, (3, 7), is on a1: the extension
    // runs along it, so it isn't met.
    let before = doc(
        &[
            p("p0000000", [3.0, 0.0]),
            p("p0000001", [3.0, 5.0]),
            p("q0000000", [3.0, 4.0]),
            p("q0000001", [3.0, 9.0]),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("a0000001", "q0000000", "q0000001"),
        ],
        &[],
        &[],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap(),
        None
    );
}

#[test]
fn an_arc_about_a_centre_off_the_origin_meets_the_nearest_curve_around_it() {
    // sketch_extend_to's arc and two lines moved by (100, −50): r0 about (100, −50), radius
    // 10, from (110, −50) round to (100, −40); x = 94 (b0) and x = 106 (b1). The circle
    // meets x = 94 at (94, −50 ± 8) and x = 106 at (106, −50 ± 8). On from the end (90°),
    // (94, −42) is 36.87° on, nearest; back from the start (0°), (106, −58) is 53.13°
    // back. Arcs centred at the origin left the centre's offset in the angle untested.
    let before = doc(
        &[
            p("z0000000", [100.0, -50.0]),
            p("s0000000", [110.0, -50.0]),
            p("e0000000", [100.0, -40.0]),
            p("q0000000", [94.0, -70.0]),
            p("q0000001", [94.0, -30.0]),
            p("q0000002", [106.0, -70.0]),
            p("q0000003", [106.0, -30.0]),
        ],
        &[
            line("b0000000", "q0000000", "q0000001"),
            line("b0000001", "q0000002", "q0000003"),
        ],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    let sketch = sketch_of(&before);
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::End).unwrap(),
        Some(Crossing {
            curve: id("b0000000"),
            at: xy(94.0, -42.0),
        })
    );
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::Start).unwrap(),
        Some(Crossing {
            curve: id("b0000001"),
            at: xy(106.0, -58.0),
        })
    );
}

#[test]
fn an_arc_starting_off_zero_meets_a_curve_in_the_rest_of_its_circle() {
    // r0 about (0, 0), radius 10, from (0, 10) at 90° to (−10, 0) at 180°: the rest of its
    // circle, 270°, is where its ends can go. x = 6 (b0, y from −20 to 0) meets the circle
    // at (6, −8), 306.87°: 126.87° on from the end, 143.13° back from the start, both
    // within the 270°. Arcs starting at 0° left the start's angle in that window untested.
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [0.0, 10.0]),
            p("e0000000", [-10.0, 0.0]),
            p("q0000000", [6.0, -20.0]),
            p("q0000001", [6.0, 0.0]),
        ],
        &[line("b0000000", "q0000000", "q0000001")],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    let sketch = sketch_of(&before);
    let want = Some(Crossing {
        curve: id("b0000000"),
        at: xy(6.0, -8.0),
    });
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::End).unwrap(),
        want
    );
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::Start).unwrap(),
        want
    );
}

#[test]
fn an_arc_overlapping_another_of_its_circle_by_a_sliver_meets_nothing_along_it() {
    // r0 from 30° to 120°, off zero at both ends. r1 from 10° to 32° overlaps its start by
    // 2°: back from 30°, the extension runs along r1 to its start, 20° back, halfway there
    // at 20°, on r1, so r1 isn't met. r2 from 118° to 200° overlaps its end by 2°: on from
    // 120°, it runs along r2 to its end, 80° on, halfway at 160°, on r2. The halfway place
    // is the end's angle less or more half the way: 43° or 83.8°, off r1 and r2, would
    // have them met.
    let with = |other: &str, from: f64, to: f64| {
        let (start, end) = (format!("s{}", &other[1..]), format!("e{}", &other[1..]));
        doc(
            &[
                p("z0000000", [0.0, 0.0]),
                p("s0000000", on_circle(30.0)),
                p("e0000000", on_circle(120.0)),
                p(&start, on_circle(from)),
                p(&end, on_circle(to)),
            ],
            &[],
            &[],
            &[
                arc("r0000000", "z0000000", "s0000000", "e0000000"),
                arc(other, "z0000000", &start, &end),
            ],
        )
    };
    let before = with("r0000001", 10.0, 32.0);
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("r0000000"), End::Start).unwrap(),
        None
    );
    let before = with("r0000002", 118.0, 200.0);
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("r0000000"), End::End).unwrap(),
        None
    );
}

/// Arc r0 about z0 at (0, 0) from s0 at (10, 0), radius 10, to e0 at (0, 10.5), 0.5 mm off
/// its circle, as the gate leaves it to the solve; and `more`.
fn an_arc_ending_off_its_circle(points: &[String], arcs: &[String]) -> Document {
    let mut all = vec![
        p("z0000000", [0.0, 0.0]),
        p("s0000000", [10.0, 0.0]),
        p("e0000000", [0.0, 10.5]),
    ];
    all.extend(points.iter().cloned());
    let mut rounds = vec![arc("r0000000", "z0000000", "s0000000", "e0000000")];
    rounds.extend(arcs.iter().cloned());
    doc(&all, &[], &[], &rounds)
}

#[test]
fn a_point_of_the_circle_at_either_ends_angle_is_past_neither() {
    // r1 about (0, 20) from (5, 20), radius 5, round 270° to (0, 10), 5 mm off its own
    // circle, which meets r0's nowhere (20 apart, radii 10 and 5). Its end (0, 10) is on
    // r0's circle at r0's end's angle, 90°: 0° on from the end, though 0.5 mm from it; and
    // 270° back from the start, at the other end's angle. Where the arc ends, not past it
    // either way, so nothing is met.
    let before = an_arc_ending_off_its_circle(
        &[
            p("z0000001", [0.0, 20.0]),
            p("s0000001", [5.0, 20.0]),
            p("e0000001", [0.0, 10.0]),
        ],
        &[arc("r0000001", "z0000001", "s0000001", "e0000001")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    let sketch = sketch_of(&before);
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::End).unwrap(),
        None
    );
    assert_eq!(
        next_crossing(sketch, &id("r0000000"), End::Start).unwrap(),
        None
    );
}

#[test]
fn an_arcs_start_sent_to_its_circle_at_its_ends_angle_is_too_short() {
    // (0, 10) is on r0's circle at its end's angle: the start there leaves a sweep of 0,
    // though the end, off the circle, is 0.5 mm away. Refused as 0 mm long.
    let before = an_arc_ending_off_its_circle(&[], &[]);
    let e = extend(
        sketch_of(&before),
        &id("r0000000"),
        End::Start,
        By::To(xy(0.0, 10.0)),
        &mut FakeSketches::new(),
    )
    .unwrap_err();
    assert_eq!(
        said(&e),
        (
            ErrorCode::InvalidInput,
            "extending arc r0000000 that far would leave it 0 mm long or flipped: an arc must be longer than 1e-7 mm and keep its direction",
            Some("by"),
            vec!["r0000000"],
            Some("give a point farther around its circle from its other end"),
        )
    );
}

#[test]
fn an_arcs_end_goes_to_a_point_sharing_a_coordinate_with_it_or_mirroring_its_start() {
    // r0 about (0, 0), radius 10, from (8, 6) at 36.87° to (0, 10). (0, −10) shares the
    // end's x; (8, −6) is the start mirrored, at −36.87°, so the sweep from the start is
    // 360° − 73.74°, not the 0 the two angles add to. Each is put there exactly.
    let before = doc(
        &[
            p("z0000000", [0.0, 0.0]),
            p("s0000000", [8.0, 6.0]),
            p("e0000000", [0.0, 10.0]),
        ],
        &[],
        &[],
        &[arc("r0000000", "z0000000", "s0000000", "e0000000")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    for to in [[0.0, -10.0], [8.0, -6.0]] {
        let edited = extend(
            sketch_of(&before),
            &id("r0000000"),
            End::End,
            By::To(xy(to[0], to[1])),
            &mut FakeSketches::new(),
        )
        .unwrap();
        assert_eq!(gate_edit(&before, &edited), Vec::<String>::new());
        assert_eq!(at(&edited.sketch, "e0000000"), to);
        assert_eq!(at(&edited.sketch, "s0000000"), [8.0, 6.0]);
        assert_eq!(edited.modified, [id("e0000000")]);
    }
}

// ---------------------------------------------------------------------------------------
// A line's By::To off the axes.

#[test]
fn a_line_off_the_axes_sent_behind_its_other_end_is_refused_with_its_length() {
    // a0 from (2, 1) to (10, 7), 10 mm along (0.8, 0.6). (−2, −2) is on its line, 5 mm
    // behind its start: flipped, 5 mm long.
    let before = doc(
        &[p("p0000000", [2.0, 1.0]), p("p0000001", [10.0, 7.0])],
        &[line("a0000000", "p0000000", "p0000001")],
        &[],
        &[],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    let mut fake = exact();
    let e = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::To(xy(-2.0, -2.0)),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(
        said(&e),
        (
            ErrorCode::InvalidInput,
            "extending line a0000000 that far would leave it 5 mm long or flipped: a line must be longer than 1e-7 mm and keep its direction",
            Some("by"),
            vec!["a0000000"],
            Some("give a point farther from its other end, the way the line runs"),
        )
    );
    assert_eq!(fake.drag_calls(), []);
}

#[test]
fn a_vertical_line_goes_to_a_point_ahead_on_it() {
    // a0 from (3, −10) up to (3, −5); (3, −2) is 3 mm on. Its x runs nowhere: the end
    // goes there.
    let before = doc(
        &[p("p0000000", [3.0, -10.0]), p("p0000001", [3.0, -5.0])],
        &[line("a0000000", "p0000000", "p0000001")],
        &[],
        &[],
    );
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        By::To(xy(3.0, -2.0)),
        &mut exact(),
    )
    .unwrap();
    assert_eq!(gate_edit(&before, &edited), Vec::<String>::new());
    assert_eq!(at(&edited.sketch, "p0000001"), [3.0, -2.0]);
    assert_eq!(edited.modified, [id("p0000001")]);
}

#[test]
fn a_point_square_to_a_lines_end_isnt_past_it_though_within_the_floor_of_its_line() {
    // a0 from F = (1 − 134814, 1 − 173732) to T = (1, 1), 219,903.67 mm long. X = T +
    // (−173732, 134814) · 2^-41 is square to it at T, the dot product exactly 0, and
    // 219,903.67 · 2^-41 = 1.0000061e-7 mm from T: over the floor. Yet the line's
    // projection of X, from F, rounds to within 1e-7 mm of X, so X is within the floor of
    // the line. X is r1's end, 9 mm off r1's own circle, which meets nothing: beside the
    // end, not past it, so the end meets nothing; and the start, sent to X, is refused as
    // turning the line, before any drag.
    let s = 1.0 / 2_199_023_255_552.0; // 2^-41, exactly
    let t = [1.0, 1.0];
    let f = [1.0 - 134_814.0, 1.0 - 173_732.0];
    let x = [1.0 - 173_732.0 * s, 1.0 + 134_814.0 * s];
    assert_eq!(
        (x[0] - t[0]) * (t[0] - f[0]) + (x[1] - t[1]) * (t[1] - f[1]),
        0.0
    );
    assert!(libm::hypot(x[0] - t[0], x[1] - t[1]) > 1e-7);
    let before = doc(
        &[
            p("p0000000", f),
            p("p0000001", t),
            p("z0000001", [-10.0, 10.0]),
            p("s0000001", [-5.0, 10.0]),
            p("e0000001", x),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &[],
        &[arc("r0000001", "z0000001", "s0000001", "e0000001")],
    );
    assert_eq!(gate_of(&before), Vec::<String>::new());
    assert_eq!(
        next_crossing(sketch_of(&before), &id("a0000000"), End::End).unwrap(),
        None
    );
    let mut fake = exact();
    let e = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::Start,
        By::To(xy(x[0], x[1])),
        &mut fake,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
    assert_eq!(e.parameter(), Some("by"), "{e}");
    assert_eq!(e.elements(), ["a0000000"], "{e}");
    assert_eq!(
        e.suggestion(),
        Some("give a point farther from its other end, the way the line runs"),
        "{e}"
    );
    let size = e
        .message()
        .strip_prefix("extending line a0000000 that far would leave it ")
        .and_then(|rest| {
            rest.strip_suffix(
                " mm long or flipped: a line must be longer than 1e-7 mm and keep its direction",
            )
        })
        .unwrap_or_else(|| panic!("{e}"));
    let size: f64 = size.parse().unwrap();
    assert!((size - 1.000_006_096_668_7e-7).abs() <= 1e-20, "{e}");
    assert_eq!(fake.drag_calls(), []);
}
