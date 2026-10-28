//! M3.7: a drag session (docs/ports/sketcher.md, Dragging), in this process through
//! `silico_sketch::Drag` and through the worker by the core's client. Where a dragged
//! point lands is worked out by hand: the solver keeps the sketch's constraints and pulls
//! the point as near the cursor as they let it (least squares), so each case's answer is a
//! projection or a weighted mean.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::systems;
use super::wait::WAIT;

use std::f64::consts::{FRAC_PI_2, PI};
use std::path::Path;

use silico_core::ErrorCode;
use silico_core::worker::{Worker, WorkerKind};
use silico_protocol::{
    Algorithm, Blob, DragBegin, Generation, Geometry, Moved, Op, PointOf, PointValues, Refusal,
    Reply, SketchGeometry, SketchSystem, SolveStatus, SolverConstraint, Tag,
};
use silico_sketch::Drag;
use silico_sketch::worker::State;
use systems::Builder;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// The solver stops when its step is under 1e-10: a point lands within this of where it
/// should.
const CLOSE: f64 = 1e-9;

fn assert_near(got: [f64; 2], want: [f64; 2], what: &str) {
    let off = libm::hypot(got[0] - want[0], got[1] - want[1]);
    assert!(
        off <= CLOSE,
        "{what}: got {got:?}, want {want:?} ({off:e} off)"
    );
}

fn at(values: &[f64], p: PointValues) -> [f64; 2] {
    [values[p.x.0 as usize], values[p.y.0 as usize]]
}

fn begin(system: SketchSystem, point: PointOf) -> Drag {
    let start = Drag::begin(&DragBegin { system, point }).unwrap();
    assert!(start.conflicting.is_empty(), "{:?}", start.conflicting);
    start.drag.unwrap()
}

/// A move that succeeded: its values.
fn moved(drag: &mut Drag, to: [f64; 2]) -> Vec<f64> {
    let moved = drag.move_to(to).unwrap();
    assert_eq!(moved.status, SolveStatus::Success, "to {to:?}");
    moved.values.unwrap()
}

/// A line from a fixed (3, -2) whose length is fixed at 10, drawn along +x: one degree of
/// freedom, its angle. Returns the system and its end's values.
fn fixed_length_line() -> (SketchSystem, Geometry, PointValues) {
    let mut b = Builder::default();
    let start = PointValues {
        x: b.fixed(3.0),
        y: b.fixed(-2.0),
    };
    let end = b.point_values(13.0, -2.0);
    let line = b.add(SketchGeometry::Line { start, end });
    let length = b.fixed(10.0);
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Start(line),
        b: PointOf::End(line),
        value: length,
    });
    (b.system(Algorithm::DogLeg), line, end)
}

#[test]
fn a_lines_end_dragged_around_its_fixed_start_lands_on_its_circle_toward_the_cursor() {
    let (system, line, end) = fixed_length_line();
    let start = Drag::begin(&DragBegin {
        system,
        point: PointOf::End(line),
    })
    .unwrap();
    // Two unknowns, one equation.
    assert_eq!((start.dofs, start.conflicting.as_slice()), (1, &[][..]));
    let mut drag = start.drag.unwrap();
    // The nearest point of the circle of radius 10 about (3, -2) to the cursor: the start
    // plus 10 times the unit vector toward the cursor. Past the circle, inside it, on it,
    // and nearly square to where the end started.
    for cursor in [
        [20.0, 5.0],
        [4.0, -1.0],
        [12.0, -2.0],
        [3.5, -30.0],
        [5.0, 30.0],
    ] {
        let values = moved(&mut drag, cursor);
        let (dx, dy) = (cursor[0] - 3.0, cursor[1] + 2.0);
        let d = libm::hypot(dx, dy);
        let want = [3.0 + 10.0 * dx / d, -2.0 + 10.0 * dy / d];
        assert_near(at(&values, end), want, &format!("cursor {cursor:?}"));
        // The fixed start stays exactly where it was.
        assert_eq!(&values[..2], &[3.0, -2.0]);
    }
    // A cursor well behind the start, 149° round from where the end started: solving from
    // the drag's start, the solver stops at the other point where the circle's normal
    // meets the cursor, the farthest, opposite the nearest. FreeCAD 1.1.3 does the same:
    // its `moveGeometry` of a 10 long line's end about the origin to (-10, 6) gives
    // (8.574929257125444, -5.144957554275264), and here the start is at (3, -2).
    let values = moved(&mut drag, [-7.0, 4.0]);
    let d = 136.0_f64.sqrt();
    assert_near(
        at(&values, end),
        [3.0 + 100.0 / d, -2.0 - 60.0 / d],
        "behind the start",
    );
}

#[test]
fn a_point_on_a_fixed_horizontal_line_keeps_its_height_and_follows_the_cursor_across() {
    let mut b = Builder::default();
    let start = PointValues {
        x: b.fixed(0.0),
        y: b.fixed(1.0),
    };
    let end = PointValues {
        x: b.fixed(10.0),
        y: b.fixed(1.0),
    };
    let line = b.add(SketchGeometry::Line { start, end });
    let p = b.point_values(4.0, 1.0);
    let point = b.add(SketchGeometry::Point(p));
    b.constrain(|tag| SolverConstraint::PointOnLine {
        tag,
        point: PointOf::Point(point),
        line,
    });
    let mut drag = begin(b.system(Algorithm::DogLeg), PointOf::Point(point));
    // On the line extended past its ends too: (x, 1) for every cursor (x, y).
    for cursor in [[7.5, 3.0], [-20.0, -6.25], [123.0, 1.0], [4.0, 1e3]] {
        let values = moved(&mut drag, cursor);
        assert_near(at(&values, p), [cursor[0], 1.0], &format!("{cursor:?}"));
    }
}

#[test]
fn a_free_point_lands_exactly_on_the_cursor() {
    let mut b = Builder::default();
    let p = b.point_values(1.0, 2.0);
    let point = b.add(SketchGeometry::Point(p));
    let start = Drag::begin(&DragBegin {
        system: b.system(Algorithm::DogLeg),
        point: PointOf::Point(point),
    })
    .unwrap();
    assert_eq!(start.dofs, 2);
    let mut drag = start.drag.unwrap();
    // To the last bit or two: DogLeg adds its step to where the point started.
    for cursor in [[-3.25, 8.5], [1e6, -1e6], [0.1, 0.2], [1.0, 2.0]] {
        let got = moved(&mut drag, cursor);
        for (g, w) in got.iter().zip(cursor) {
            assert!(
                (g - w).abs() <= 4.0 * f64::EPSILON * w.abs().max(1.0),
                "{cursor:?}: got {got:?}"
            );
        }
    }
}

/// An arc about (1, 2) of radius 5 from 0 to π/2, every value of it free, with its rules;
/// its radius fixed at 5 if `fixed_radius`. Returns the system, the arc, its centre, and
/// its end.
fn quarter_arc(fixed_radius: bool) -> (SketchSystem, Geometry, PointValues, PointValues) {
    let mut b = Builder::default();
    let center = b.point_values(1.0, 2.0);
    let radius = b.unknown(5.0);
    let start_angle = b.unknown(0.0);
    let end_angle = b.unknown(std::f64::consts::FRAC_PI_2);
    let start = b.point_values(6.0, 2.0);
    let end = b.point_values(1.0, 7.0);
    let arc = b.add(SketchGeometry::Arc {
        center,
        radius,
        start_angle,
        end_angle,
        start,
        end,
    });
    b.constraints.push(SolverConstraint::ArcRules { arc });
    if fixed_radius {
        let five = b.fixed(5.0);
        b.constrain(|tag| SolverConstraint::Radius {
            tag,
            circle: arc,
            value: five,
        });
    }
    (b.system(Algorithm::DogLeg), arc, center, end)
}

#[test]
fn an_arcs_end_moves_before_its_centre() {
    // The end is pinned to the cursor, and the centre to where it started a hundred times
    // more weakly (FreeCAD 4d7a9435c1). With the radius free, the end reaches the cursor by
    // changing the radius and angle alone: the centre doesn't move.
    let (system, arc, center, end) = quarter_arc(false);
    let mut drag = begin(system, PointOf::End(arc));
    for cursor in [[-6.0, 9.0], [1.0, 2.5], [1.0, 17.0]] {
        let values = moved(&mut drag, cursor);
        assert_near(at(&values, end), cursor, "end");
        assert_near(at(&values, center), [1.0, 2.0], "centre");
        let radius = values[2];
        let want = libm::hypot(cursor[0] - 1.0, cursor[1] - 2.0);
        assert!(
            (radius - want).abs() <= CLOSE,
            "radius {radius}, want {want}"
        );
    }
}

#[test]
fn an_arc_of_fixed_radius_follows_its_end_by_the_weight_of_its_centres_pin() {
    // The end on the circle of radius 5 about the centre; the pins' squared misses, the
    // centre's weighted by 0.01² = 1e-4, least in sum. A cursor d from the centre's start
    // along u: the centre moves δ along u, the end lands at the centre plus 5u, d - 5 - δ
    // short of the cursor, and (d - 5 - δ)² + 1e-4 δ² is least at δ = (d - 5) / (1 + 1e-4).
    // With the pins weighted alike it would be (d - 5) / 2.
    let (system, arc, center, end) = quarter_arc(true);
    let mut drag = begin(system, PointOf::End(arc));
    for cursor in [[1.0, 17.0], [-11.0, -7.0], [4.0, 6.0], [1.0, 3.0]] {
        let values = moved(&mut drag, cursor);
        let (dx, dy) = (cursor[0] - 1.0, cursor[1] - 2.0);
        let d = libm::hypot(dx, dy);
        let u = [dx / d, dy / d];
        let delta = (d - 5.0) / (1.0 + 1e-4);
        let c = [1.0 + delta * u[0], 2.0 + delta * u[1]];
        assert_near(
            at(&values, center),
            c,
            &format!("centre, cursor {cursor:?}"),
        );
        assert_near(
            at(&values, end),
            [c[0] + 5.0 * u[0], c[1] + 5.0 * u[1]],
            &format!("end, cursor {cursor:?}"),
        );
    }
}

#[test]
fn a_sketch_point_an_arc_ends_at_pins_the_arcs_centre_as_the_arcs_end_does() {
    // The core drags a sketch's point (M3.2's D2), whose values the arc's end shares, not
    // the arc's end: the fixed-radius quarter arc, dragged by such a point, lands where its
    // end dragged does (the case above, δ = (d - 5) / (1 + 1e-4)). Were the centre not
    // pinned, it would follow freely, δ = d - 5, and the end would reach the cursor.
    let (mut system, _, center, end) = quarter_arc(true);
    system.geometry.push(SketchGeometry::Point(end));
    let point = Geometry(system.geometry.len() as u32 - 1);
    let mut drag = begin(system, PointOf::Point(point));
    for cursor in [[1.0, 17.0], [-11.0, -7.0], [4.0, 6.0]] {
        let values = moved(&mut drag, cursor);
        let (dx, dy) = (cursor[0] - 1.0, cursor[1] - 2.0);
        let d = libm::hypot(dx, dy);
        let u = [dx / d, dy / d];
        let delta = (d - 5.0) / (1.0 + 1e-4);
        let c = [1.0 + delta * u[0], 2.0 + delta * u[1]];
        assert_near(
            at(&values, center),
            c,
            &format!("centre, cursor {cursor:?}"),
        );
        assert_near(
            at(&values, end),
            [c[0] + 5.0 * u[0], c[1] + 5.0 * u[1]],
            &format!("end, cursor {cursor:?}"),
        );
    }
}

#[test]
fn a_point_two_arcs_end_at_pins_both_their_centres() {
    // P at (0, 0) is the start of an arc about (-5, 0) and the end of one about (5, 0),
    // each of radius 5 held, mirror images in x = 0. Dragged to (0, d), P stays on x = 0,
    // at (0, y); each centre goes to the point 5 from P nearest where it started, s - 5 from
    // there, s = √(25 + y²); and y makes (y - d)² + 2 · 1e-4 · (s - 5)² least, the two
    // centres' pins each weighted 0.01² (found here by Newton's method). With one centre
    // pinned, the weight would be 1e-4, and P would land about half as far short: 5.5e-4
    // mm nearer the cursor at d = 10, 4.3e-5 at d = 3. A centre is held along its circle
    // about P by its 1e-4-weighted pin alone, so the solver stops with it less close than
    // P: within 1e-8, still far finer than those.
    let mut b = Builder::default();
    let p = b.point_values(0.0, 0.0);
    let arc = |b: &mut Builder, center: (f64, f64), start, end, angles: [f64; 2]| {
        let center = b.point_values(center.0, center.1);
        let radius = b.unknown(5.0);
        let start_angle = b.unknown(angles[0]);
        let end_angle = b.unknown(angles[1]);
        let arc = b.add(SketchGeometry::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            start,
            end,
        });
        b.constraints.push(SolverConstraint::ArcRules { arc });
        let five = b.fixed(5.0);
        b.constrain(|tag| SolverConstraint::Radius {
            tag,
            circle: arc,
            value: five,
        });
        center
    };
    let left_end = b.point_values(-5.0, 5.0);
    let left = arc(&mut b, (-5.0, 0.0), p, left_end, [0.0, FRAC_PI_2]);
    let right_start = b.point_values(5.0, 5.0);
    let right = arc(&mut b, (5.0, 0.0), right_start, p, [FRAC_PI_2, PI]);
    let point = b.add(SketchGeometry::Point(p));
    let mut drag = begin(b.system(Algorithm::DogLeg), PointOf::Point(point));
    for d in [10.0, -8.0, 3.0] {
        let mut y: f64 = d;
        for _ in 0..50 {
            let s = libm::hypot(5.0, y);
            let f = (y - d) + 2e-4 * (s - 5.0) * y / s;
            let df = 1.0 + 2e-4 * ((y / s).powi(2) + (s - 5.0) * 25.0 / s.powi(3));
            y -= f / df;
        }
        let s = libm::hypot(5.0, y);
        let values = moved(&mut drag, [0.0, d]);
        assert_near(at(&values, p), [0.0, y], &format!("P, cursor (0, {d})"));
        let rise = y - 5.0 * y / s;
        for (centre, want) in [(left, [-25.0 / s, rise]), (right, [25.0 / s, rise])] {
            let got = at(&values, centre);
            let off = libm::hypot(got[0] - want[0], got[1] - want[1]);
            assert!(
                off <= 1e-8,
                "centre, cursor (0, {d}): got {got:?}, want {want:?} ({off:e} off)"
            );
        }
    }
}

#[test]
fn a_move_the_solver_fails_gives_no_values_and_the_next_move_starts_afresh() {
    // The free arc's end dragged to (30, -40) or (-11, -7): from the drag's start, SQP
    // can't meet the arc's rules, and the move fails, as FreeCAD 1.1.3's does ("Not able
    // to move point with the id and type: (0, 2)", for the same arc and cursors).
    let (system, arc, center, end) = quarter_arc(false);
    let mut drag = begin(system, PointOf::End(arc));
    for cursor in [[30.0, -40.0], [-11.0, -7.0]] {
        let failed = drag.move_to(cursor).unwrap();
        assert_ne!(failed.status, SolveStatus::Success, "{cursor:?}");
        assert_eq!(failed.values, None, "{cursor:?}");
        // The next move solves from the drag's start, as if the failed one never was.
        let values = moved(&mut drag, [-6.0, 9.0]);
        assert_near(at(&values, end), [-6.0, 9.0], "end");
        assert_near(at(&values, center), [1.0, 2.0], "centre");
    }
}

#[test]
fn a_solution_out_of_reach_is_refused_and_the_drag_goes_on() {
    // Two free points, the second 10 right of the first: the first dragged to x = 1e6 puts
    // the second at 1e6 + 10, past a kilometre, so its x, value 2, is out of reach.
    let mut b = Builder::default();
    let p = b.point_values(0.0, 0.0);
    let q = b.point_values(10.0, 0.0);
    let first = b.add(SketchGeometry::Point(p));
    let second = b.add(SketchGeometry::Point(q));
    let ten = b.fixed(10.0);
    b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Point(first),
        b: PointOf::Point(second),
        value: ten,
    });
    let system = b.system(Algorithm::DogLeg);
    let mut state = State::default();
    assert!(matches!(
        state.handle(Op::DragBegin(DragBegin {
            system,
            point: PointOf::Point(first)
        })),
        Ok(Reply::DragStarted { .. })
    ));
    let failure = state.handle(Op::DragMove { to: [1e6, 0.0] }).unwrap_err();
    assert_eq!(failure.refusal, Refusal::SolutionOutOfReach { value: 2 });
    assert!(state.dragging());
    match state.handle(Op::DragMove { to: [-6.0, 9.0] }).unwrap() {
        Reply::Moved(Moved {
            status: SolveStatus::Success,
            values: Some(values),
        }) => {
            assert_near(at(&values, p), [-6.0, 9.0], "first");
            // 10 right of the first; its height, which nothing ties, where it was.
            assert_near(at(&values, q), [4.0, 0.0], "second");
        }
        other => panic!("DragMove replied {other:?}"),
    }
}

#[test]
fn a_centre_dragged_moves_a_circle_of_fixed_radius_whole() {
    // A circle's centre pinned alone: it goes to the cursor, and the radius stays.
    let mut b = Builder::default();
    let center = b.point_values(0.0, 0.0);
    let radius = b.unknown(2.0);
    let circle = b.add(SketchGeometry::Circle { center, radius });
    let two = b.fixed(2.0);
    b.constrain(|tag| SolverConstraint::Radius {
        tag,
        circle,
        value: two,
    });
    let mut drag = begin(b.system(Algorithm::DogLeg), PointOf::Center(circle));
    let values = moved(&mut drag, [5.5, -1.25]);
    assert_near(at(&values, center), [5.5, -1.25], "centre");
    assert!((values[2] - 2.0).abs() <= CLOSE, "radius {}", values[2]);
}

/// A chain of `links` lines, each 10 long, joined end to start from a fixed point at the
/// origin, every other one at 30° to the one before; drawn along +x, off its solution.
/// Returns the system and the last line.
fn chain(links: usize) -> (SketchSystem, Geometry) {
    let mut b = Builder::default();
    let origin = b.fixed_point(0.0, 0.0);
    let ten = b.fixed(10.0);
    let angle = b.fixed(std::f64::consts::FRAC_PI_6);
    let mut lines: Vec<Geometry> = Vec::new();
    for i in 0..links {
        let x = 10.0 * i as f64;
        let line = b.line((x + 0.1, 0.2), (x + 9.7, -0.3));
        b.constrain(|tag| SolverConstraint::Distance {
            tag,
            a: PointOf::Start(line),
            b: PointOf::End(line),
            value: ten,
        });
        match lines.last() {
            None => {
                b.constrain(|tag| SolverConstraint::Coincident {
                    tag,
                    a: PointOf::Start(line),
                    b: PointOf::Point(origin),
                });
            }
            Some(&before) => {
                b.constrain(|tag| SolverConstraint::Coincident {
                    tag,
                    a: PointOf::End(before),
                    b: PointOf::Start(line),
                });
                if i % 2 == 1 {
                    b.constrain(|tag| SolverConstraint::Angle {
                        tag,
                        a: before,
                        b: line,
                        value: angle,
                    });
                }
            }
        }
        lines.push(line);
    }
    let last = *lines.last().unwrap();
    (solved(b.system(Algorithm::DogLeg)), last)
}

/// `system`, its values moved to its solution, as a drag starts from a solved sketch.
fn solved(mut system: SketchSystem) -> SketchSystem {
    let solved = silico_sketch::solve(&system).unwrap();
    assert_eq!(solved.status, SolveStatus::Success);
    system.values = solved.values;
    system
}

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|v| v.to_bits()).collect()
}

#[test]
fn a_moves_result_depends_on_the_start_and_the_cursor_alone() {
    let (system, last) = chain(6);
    let point = PointOf::End(last);
    let path = [[40.0, 10.0], [35.0, 20.0], [-10.0, 30.0], [20.0, 25.0]];
    let to = [22.5, 27.5];
    let mut walked = begin(system.clone(), point);
    for p in path {
        moved(&mut walked, p);
    }
    let after_walk = moved(&mut walked, to);
    let mut straight = begin(system.clone(), point);
    let direct = moved(&mut straight, to);
    assert_eq!(bits(&after_walk), bits(&direct));
    // And again on the same drag, after moves elsewhere, and after a move that came back
    // out of reach of the chain.
    moved(&mut straight, [-55.0, 0.0]);
    moved(&mut straight, [1000.0, 1000.0]);
    assert_eq!(bits(&moved(&mut straight, to)), bits(&direct));
}

#[test]
fn a_drag_of_a_sketch_with_constraints_in_conflict_starts_no_session() {
    // A line horizontal and 5 high at once: both conflict. Four unknowns less one
    // independent equation leaves three degrees of freedom.
    let mut b = Builder::default();
    let line = b.line((0.0, 0.0), (10.0, 0.5));
    b.constrain(|tag| SolverConstraint::Horizontal { tag, line });
    let five = b.fixed(5.0);
    b.constrain(|tag| SolverConstraint::DistanceY {
        tag,
        a: PointOf::Start(line),
        b: PointOf::End(line),
        value: five,
    });
    let begin = DragBegin {
        system: b.system(Algorithm::DogLeg),
        point: PointOf::End(line),
    };
    let start = Drag::begin(&begin).unwrap();
    assert_eq!(start.conflicting, [Tag(1), Tag(2)]);
    assert_eq!(start.dofs, 3);
    assert!(start.drag.is_none());

    // The worker's state says so, and refuses a move.
    let mut state = State::default();
    match state.handle(Op::DragBegin(begin)).unwrap() {
        Reply::DragStarted { dofs, conflicting } => {
            assert_eq!((dofs, conflicting), (3, vec![Tag(1), Tag(2)]));
        }
        other => panic!("DragBegin replied {other:?}"),
    }
    assert!(!state.dragging());
    let refused = state.handle(Op::DragMove { to: [1.0, 1.0] }).unwrap_err();
    assert_eq!(
        refused.refusal,
        Refusal::NoDragSession {
            op: "DragMove".to_owned()
        }
    );
}

#[test]
fn a_target_that_isnt_finite_or_is_out_of_reach_is_refused_and_the_drag_goes_on() {
    let (system, line, end) = fixed_length_line();
    let mut drag = begin(system, PointOf::End(line));
    for to in [
        [f64::NAN, 0.0],
        [0.0, f64::INFINITY],
        [f64::NEG_INFINITY, 1.0],
        [1.000_000_1e6, 0.0],
        [0.0, -2e6],
    ] {
        let failure = drag.move_to(to).unwrap_err();
        match failure.refusal {
            Refusal::InvalidParameter { parameter, message } => {
                assert_eq!(parameter, "to");
                assert!(message.starts_with("the drag's target's "), "{message}");
            }
            other => panic!("{to:?}: {other:?}"),
        }
    }
    let values = moved(&mut drag, [3.0, 18.0]);
    assert_near(at(&values, end), [3.0, 8.0], "end");
}

#[test]
fn a_begin_naming_geometry_there_isnt_or_a_point_it_lacks_is_refused() {
    let (system, ..) = fixed_length_line();
    for (point, words) in [
        (
            PointOf::End(Geometry(1)),
            "the drag names geometry 1, but there are 1",
        ),
        (
            PointOf::Center(Geometry(0)),
            "the drag names the centre of geometry 0, but geometry 0 is a line",
        ),
        (
            PointOf::Point(Geometry(0)),
            "the drag names point 0, but geometry 0 is a line",
        ),
    ] {
        let begin = DragBegin {
            system: system.clone(),
            point,
        };
        let refused = |failure: silico_protocol::Failure| {
            assert_eq!(
                failure.refusal,
                Refusal::InvalidParameter {
                    parameter: "point".to_owned(),
                    message: words.to_owned()
                }
            );
        };
        match Drag::begin(&begin) {
            Err(failure) => refused(failure),
            Ok(_) => panic!("{point:?} began a drag"),
        }
        refused(State::default().handle(Op::DragBegin(begin)).unwrap_err());
    }
}

fn worker() -> Worker {
    Worker::spawn_as(Path::new(WORKER), WorkerKind::Sketch).unwrap()
}

fn call(w: &mut Worker, op: Op) -> Result<Reply, silico_core::Error> {
    w.call(Generation(1), op, WAIT)
}

fn moved_there(w: &mut Worker, to: [f64; 2]) -> Moved {
    match call(w, Op::DragMove { to }).unwrap() {
        Reply::Moved(moved) => moved,
        other => panic!("DragMove replied {other:?}"),
    }
}

fn assert_no_drag(w: &mut Worker, op: Op) {
    let name = op.name();
    let err = call(w, op).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert_eq!(
        err.message(),
        format!("{name} needs a drag under way, and there's none")
    );
    assert_eq!(
        err.suggestion(),
        Some(
            "begin the drag again: a drag ends at its end, at a solve, or when the sketch worker restarts"
        )
    );
}

#[test]
fn a_drag_through_the_worker_begins_moves_and_ends_as_in_this_process() {
    let (system, last) = chain(8);
    let point = PointOf::End(last);
    let SketchGeometry::Line { end, .. } = system.geometry[last.0 as usize] else {
        panic!("the chain's last link isn't a line");
    };
    let [x, y] = at(&system.values, end);
    let path: Vec<[f64; 2]> = (1..=20)
        .map(|i| [x - 0.25 * i as f64, y + 0.125 * i as f64])
        .collect();
    let mut here = begin(system.clone(), point);
    let mut w = worker();
    match call(&mut w, Op::DragBegin(DragBegin { system, point })).unwrap() {
        // 32 unknowns; 8 lengths, 7 joins and the pin to the origin (2 each), 4 angles.
        Reply::DragStarted { dofs, conflicting } => {
            assert_eq!((dofs, conflicting.as_slice()), (32 - 8 - 16 - 4, &[][..]));
        }
        other => panic!("DragBegin replied {other:?}"),
    }
    // Each move as in this process, bit for bit, whether it solved or not (one of these,
    // the twelfth, doesn't: SQP from the drag's start fails there).
    let mut solved = 0;
    for to in path {
        let there = moved_there(&mut w, to);
        let here = here.move_to(to).unwrap();
        assert_eq!(there.status, here.status, "{to:?}");
        assert_eq!(
            there.values.as_deref().map(bits),
            here.values.as_deref().map(bits),
            "{to:?}"
        );
        solved += usize::from(there.status == SolveStatus::Success);
    }
    assert_eq!(solved, 19);
    // An echo leaves the drag alone.
    assert!(matches!(
        call(
            &mut w,
            Op::Echo {
                data: Blob::inline(vec![1])
            }
        ),
        Ok(Reply::Echo { .. })
    ));
    assert_eq!(moved_there(&mut w, [x, y]).status, SolveStatus::Success);
    assert!(matches!(call(&mut w, Op::DragEnd), Ok(Reply::Done)));
    assert_no_drag(&mut w, Op::DragMove { to: [x, y] });
    assert_no_drag(&mut w, Op::DragEnd);
}

#[test]
fn moving_or_ending_with_no_drag_is_refused_and_a_solve_ends_a_drag() {
    let mut w = worker();
    assert_no_drag(&mut w, Op::DragMove { to: [0.0, 0.0] });
    assert_no_drag(&mut w, Op::DragEnd);

    let (system, line, _) = fixed_length_line();
    let point = PointOf::End(line);
    let start = Op::DragBegin(DragBegin {
        system: system.clone(),
        point,
    });
    assert!(matches!(
        call(&mut w, start.clone()),
        Ok(Reply::DragStarted { .. })
    ));
    assert!(matches!(
        call(&mut w, Op::Solve(system.clone())),
        Ok(Reply::Solved(_))
    ));
    assert_no_drag(&mut w, Op::DragMove { to: [0.0, 0.0] });

    // A solve the worker refuses ends the drag too.
    assert!(matches!(
        call(&mut w, start.clone()),
        Ok(Reply::DragStarted { .. })
    ));
    let mut broken = system.clone();
    broken.values[0] = f64::NAN;
    assert_eq!(
        call(&mut w, Op::Solve(broken)).unwrap_err().code(),
        ErrorCode::InvalidInput
    );
    assert_no_drag(&mut w, Op::DragEnd);

    // A begin the worker refuses ends the drag under way.
    assert!(matches!(
        call(&mut w, start.clone()),
        Ok(Reply::DragStarted { .. })
    ));
    let err = call(
        &mut w,
        Op::DragBegin(DragBegin {
            system: system.clone(),
            point: PointOf::Center(line),
        }),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert_eq!(err.parameter(), Some("point"), "{err}");
    assert_no_drag(&mut w, Op::DragMove { to: [0.0, 0.0] });

    // A second begin replaces the first: moves go to the second's point. The start of a
    // free line, its end fixed at (0, 9).
    assert!(matches!(call(&mut w, start), Ok(Reply::DragStarted { .. })));
    let mut b = Builder::default();
    let s = b.point_values(0.0, 0.0);
    let e = PointValues {
        x: b.fixed(0.0),
        y: b.fixed(9.0),
    };
    let free = b.add(SketchGeometry::Line { start: s, end: e });
    assert!(matches!(
        call(
            &mut w,
            Op::DragBegin(DragBegin {
                system: b.system(Algorithm::DogLeg),
                point: PointOf::Start(free),
            })
        ),
        Ok(Reply::DragStarted { dofs: 2, .. })
    ));
    let values = moved_there(&mut w, [-4.0, 2.5]).values.unwrap();
    assert_eq!(values, [-4.0, 2.5, 0.0, 9.0]);
    assert!(matches!(call(&mut w, Op::DragEnd), Ok(Reply::Done)));
}

#[test]
fn a_bad_target_through_the_worker_is_invalid_input_and_the_drag_goes_on() {
    let mut w = worker();
    let (system, line, end) = fixed_length_line();
    assert!(matches!(
        call(
            &mut w,
            Op::DragBegin(DragBegin {
                system,
                point: PointOf::End(line)
            })
        ),
        Ok(Reply::DragStarted { .. })
    ));
    for to in [[f64::NAN, 1.0], [2e6, 0.0], [0.0, f64::NEG_INFINITY]] {
        let err = call(&mut w, Op::DragMove { to }).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
        assert_eq!(err.parameter(), Some("to"), "{err}");
    }
    let values = moved_there(&mut w, [3.0, -22.0]).values.unwrap();
    assert_near(at(&values, end), [3.0, -12.0], "end");
}

#[test]
fn the_cores_drag_client_drags_through_the_worker_and_ends_with_the_whole_sketch_solved() {
    // M5's core client (`silico_core::drag`) on the sketch worker: a 10 mm line's end, its
    // start fixed at (3, -2), moved toward (20, 5), then (5, 30), and ended toward (3, 20).
    // Each frame lands on the circle of radius 10 about the start toward its cursor, and
    // the end's closing solve keeps the end where the final move put it: (3, 8).
    use silico_core::drag::{Begun, Kept, MoveOutcome};
    use silico_core::recompute::SketchSolvers;
    use silico_core::sketch::{SketchSolver, SketchWorker};
    let solvers = SketchSolvers::starting(|| {
        SketchWorker::start_worker(Path::new(WORKER)).map(|w| Box::new(w) as Box<dyn SketchSolver>)
    });
    let (system, line, end) = fixed_length_line();
    let Begun::Open { session, dofs } = silico_core::drag::begin(
        &solvers,
        DragBegin {
            system,
            point: PointOf::End(line),
        },
    )
    .unwrap() else {
        panic!("conflicting")
    };
    assert_eq!(dofs, 1);
    for cursor in [[20.0, 5.0], [5.0, 30.0]] {
        let MoveOutcome::Solved { moved, .. } = session.move_to(cursor).unwrap().wait() else {
            panic!("{cursor:?}")
        };
        let values = moved.values.unwrap();
        let (dx, dy) = (cursor[0] - 3.0, cursor[1] + 2.0);
        let d = libm::hypot(dx, dy);
        assert_near(
            at(&values, end),
            [3.0 + 10.0 * dx / d, -2.0 + 10.0 * dy / d],
            &format!("cursor {cursor:?}"),
        );
    }
    let ended = session.end(Some([3.0, 20.0])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor);
    let solved = ended.solved.unwrap();
    assert_near(at(&solved.values, end), [3.0, 8.0], "the end");
    assert_eq!(&solved.values[..2], &[3.0, -2.0]);
}
