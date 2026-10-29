//! break-it, M3.7: hostile drags, through `silico_sketch::Drag`, the worker's `State`, and
//! the sketch worker. Expected values are worked out by hand (projections, weighted means),
//! taken from FreeCAD 1.1.3's `Sketch::internalSolve` and `updateGeometry` (Sketch.cpp
//! 4993-5045, 4713-4796: a drag's frame whose geometry can't be built is undone), or are
//! the drag's own promises checked independently: every constraint met on a `Success`
//! (residuals recomputed here), fixed values untouched, values the next solve accepts, and
//! a move's result a function of the start and the cursor alone.
//!
//! Tests marked BREAKS were bug reports, each since fixed: they say what broke.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::generate;
use super::systems;
use super::wait::WAIT;

use std::path::Path;
use std::time::{Duration, Instant};

use generate::{dir, generate, points_of, residuals, xy};
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::worker::{Worker, WorkerKind};
use silico_protocol::{
    Algorithm, DebugOp, DragBegin, Generation, Geometry, Moved, Op, PointOf, PointValues, Refusal,
    Reply, SketchGeometry, SketchSystem, SolveStatus, SolverConstraint, Tag, Value,
};
use silico_sketch::Drag;
use silico_sketch::worker::State;
use systems::{Builder, Rng};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-test-worker");
/// What the largest admitted drag's move may cost: the bound its test holds it to (a move
/// answered in time), measured as its wall time over how much slower than a quiet machine
/// this one ran it (`Share`); the call itself is allowed [`WAIT`].
const TIMEOUT: Duration = Duration::from_secs(30);
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
    let start = Drag::begin(&DragBegin {
        system,
        dragged: silico_protocol::Dragged::Point(point),
    })
    .unwrap();
    assert!(start.conflicting.is_empty(), "{:?}", start.conflicting);
    start.drag.unwrap()
}

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// `system` with `values`, as the next solve or drag would get it.
fn with_values(system: &SketchSystem, values: Vec<f64>) -> SketchSystem {
    SketchSystem {
        values,
        ..system.clone()
    }
}

// ---------------------------------------------------------------------------------------
// Frames whose geometry isn't geometry.

/// A line from a fixed (0, 0) to a free (5, 5), horizontal if `horizontal` (then drawn
/// from (0, 0) to (5, 0)).
fn line_from_the_origin(horizontal: bool) -> (SketchSystem, Geometry) {
    let mut b = Builder::default();
    let start = PointValues {
        x: b.fixed(0.0),
        y: b.fixed(0.0),
    };
    let end = b.point_values(5.0, if horizontal { 0.0 } else { 5.0 });
    let line = b.add(SketchGeometry::Line { start, end });
    if horizontal {
        b.constrain(|tag| SolverConstraint::Horizontal { tag, line });
    }
    (b.system(Algorithm::DogLeg), line)
}

#[test]
fn a_lines_end_dragged_onto_its_fixed_start_gives_no_line_the_next_solve_refuses() {
    // BREAKS: the end lands exactly on the start, (0, 0), and the move is a Success with
    // values [0, 0, 0, 0]: a line of no length, which the next solve or drag (the core's
    // closing solve after DragEnd, or the next DragBegin) refuses: "geometry 0 is a line
    // drawn with no length, both ends at (0, 0)". FreeCAD's drag builds each frame's
    // geometry and undoes a frame it can't build (Sketch.cpp 5028-5036; GeomLineSegment::
    // setPoints throws "Both points are equal"), so it never shows such a line. Expected:
    // no values (not a Success), or values the next check accepts.
    let (system, line) = line_from_the_origin(false);
    let mut drag = begin(system.clone(), PointOf::End(line));
    let moved = drag.move_to([0.0, 0.0]).unwrap();
    // Fixed: solved, but undone, as FreeCAD undoes it.
    assert_eq!(
        (moved.status, &moved.values),
        (SolveStatus::SolvedInvalid, &None)
    );
    if let Some(values) = moved.values
        && let Err(e) = with_values(&system, values.clone()).check()
    {
        panic!(
            "the move was a {:?} with {values:?}, which the next solve refuses: {}",
            moved.status, e.message
        );
    }
    // The drag goes on.
    let values = drag.move_to([-3.0, 4.0]).unwrap().values.unwrap();
    assert_eq!(values, [0.0, 0.0, -3.0, 4.0]);
}

#[test]
fn a_horizontal_lines_end_dragged_square_above_its_start_gives_no_line_the_next_solve_refuses() {
    // BREAKS: a cursor at x = 0 (a grid snap, or any cursor crossing over the start) lands a
    // horizontal line's free end on its fixed start: the projection of (0, 7) on y = 0 is
    // (0, 0). Success, with a line of no length the next solve refuses.
    let (system, line) = line_from_the_origin(true);
    let mut drag = begin(system.clone(), PointOf::End(line));
    // Either side of the start, the end follows the cursor's x.
    for x in [2.5, -2.5] {
        let values = drag.move_to([x, 7.0]).unwrap().values.unwrap();
        assert_eq!(&values[2..], &[x, 0.0], "cursor ({x}, 7)");
    }
    let moved = drag.move_to([0.0, 7.0]).unwrap();
    assert_eq!(
        (moved.status, &moved.values),
        (SolveStatus::SolvedInvalid, &None)
    );
    if let Some(values) = moved.values
        && let Err(e) = with_values(&system, values.clone()).check()
    {
        panic!(
            "the move was a {:?} with {values:?}, which the next solve refuses: {}",
            moved.status, e.message
        );
    }
}

#[test]
fn a_drag_ended_on_a_line_of_no_length_leaves_a_sketch_the_closing_solve_refuses() {
    // BREAKS: the flow the protocol documents (Op::DragEnd: "the core then solves the sketch
    // where the last move left it"), through the worker: begin, a move that lands the end on
    // the start, end, then the closing solve of those values, which the worker refuses as
    // invalid input naming geometry 0. The user's last frame can't be kept.
    let (system, line) = line_from_the_origin(false);
    let mut w = Worker::spawn_as(Path::new(WORKER), WorkerKind::Sketch).unwrap();
    let call = |w: &mut Worker, op| w.call(Generation(1), op, WAIT);
    assert!(matches!(
        call(
            &mut w,
            Op::DragBegin(DragBegin {
                system: system.clone(),
                dragged: silico_protocol::Dragged::Point(PointOf::End(line)),
            })
        ),
        Ok(Reply::DragStarted { dofs: 2, .. })
    ));
    let last = match call(&mut w, Op::DragMove { to: [0.0, 0.0] }).unwrap() {
        Reply::Moved(Moved {
            status: SolveStatus::Success,
            values: Some(values),
        }) => values,
        // Fixed: not a frame; the core keeps the one before.
        Reply::Moved(Moved {
            status: SolveStatus::SolvedInvalid,
            values: None,
        }) => return,
        other => panic!("DragMove replied {other:?}"),
    };
    assert!(matches!(call(&mut w, Op::DragEnd), Ok(Reply::Done)));
    match call(&mut w, Op::Solve(with_values(&system, last.clone()))) {
        Ok(Reply::Solved(_)) => {}
        Ok(other) => panic!("Solve replied {other:?}"),
        Err(e) => panic!(
            "the drag's last frame {last:?} was a Success, and its closing solve was refused: \
             {e} ({:?})",
            e.code()
        ),
    }
}

/// An arc about (1, 2) of radius 5 from 0 to π/2, every value free, with its rules.
/// Returns the system, the arc, its radius, its centre, its start, and its end.
fn quarter_arc(
    fixed_radius: bool,
) -> (
    SketchSystem,
    Geometry,
    Value,
    PointValues,
    PointValues,
    PointValues,
) {
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
    (b.system(Algorithm::DogLeg), arc, radius, center, start, end)
}

#[test]
fn an_arcs_end_dragged_onto_or_through_its_centre_never_gives_a_negative_radius() {
    // BREAKS: the free arc's end dragged onto its centre, (1, 2): Success, with the radius
    // -8.061022372090546e-13. FreeCAD 1.1.3 builds each frame's arc and undoes a frame it
    // can't build: for the same arc and cursor its `moveGeometry` logs "Updating geometry:
    // Error build geometry(0): gp_Circ::SetRadius() - radius should be positive number",
    // "Invalid solution from DogLeg solver", and leaves the arc at radius 5, its end at
    // (1, 7). An arc of negative radius is no arc; the check lets it through to the next
    // solve. Swept over cursors near and across the centre too.
    let (system, arc, radius, ..) = quarter_arc(false);
    let mut drag = begin(system, PointOf::End(arc));
    let mut cursors = vec![
        [1.0, 2.0],
        [1.0, -3.0],
        [1.0, 1.0],
        [1.5, -3.0],
        [0.5, -8.0],
    ];
    for i in -8..=8 {
        for j in -8..=8 {
            cursors.push([1.0 + 0.25 * f64::from(i), 2.0 + 0.25 * f64::from(j)]);
        }
    }
    // Fixed: onto the centre, solved but undone.
    let onto = drag.move_to([1.0, 2.0]).unwrap();
    assert_eq!(
        (onto.status, &onto.values),
        (SolveStatus::SolvedInvalid, &None)
    );
    let mut negative = Vec::new();
    for cursor in cursors {
        let moved = drag.move_to(cursor).unwrap();
        if let Some(values) = moved.values {
            let r = values[radius.0 as usize];
            if r < 0.0 {
                negative.push((cursor, r));
            }
        }
    }
    assert!(
        negative.is_empty(),
        "Success with a negative radius: {negative:?}"
    );
}

#[test]
fn a_free_arcs_end_dragged_across_its_centre_lands_where_freecads_does() {
    // FreeCAD 1.1.3's `moveGeometry` of the same quarter arc's end (Sketch.moveGeometry(0,
    // 2, cursor), each from the solved arc): to (1, -3), radius 4.99999999898352; to (1, 1),
    // 0.9999999999980447; to (1.5, -3), 5.024937810560022 (the end on the cursor each
    // time); to (0.5, -8), not moved (status 2). The radii are those of a centre that
    // barely moves (the end's pin 1e4 times the centre's), so the radius is the cursor's
    // distance from (1, 2), to within the solver's stop.
    let (system, arc, radius, _, _, end) = quarter_arc(false);
    for (cursor, want) in [
        ([1.0, -3.0], Some(4.99999999898352)),
        ([1.0, 1.0], Some(0.9999999999980447)),
        ([1.5, -3.0], Some(5.024937810560022)),
        ([0.5, -8.0], None),
    ] {
        let mut drag = begin(system.clone(), PointOf::End(arc));
        let moved = drag.move_to(cursor).unwrap();
        match (want, moved.values) {
            (Some(r), Some(values)) => {
                let got = values[radius.0 as usize];
                assert!(
                    (got - r).abs() <= 1e-6,
                    "{cursor:?}: radius {got}, FreeCAD {r}"
                );
                let e = at(&values, end);
                assert!(
                    libm::hypot(e[0] - cursor[0], e[1] - cursor[1]) <= 1e-6,
                    "{cursor:?}: end at {e:?}"
                );
            }
            (None, None) => {}
            (want, got) => panic!(
                "{cursor:?}: FreeCAD {want:?}, got {:?} {got:?}",
                moved.status
            ),
        }
    }
}

#[test]
fn an_arcs_start_moves_before_its_centre_as_its_end_does() {
    // The start's pin and the centre's weak pin (FreeCAD 4d7a9435c1) go through the same
    // branch as the end's, untested for the start. With the radius fixed at 5 and the
    // centre pinned 0.01 as hard, a cursor d from the centre along u moves the centre
    // δ = (d - 5) / (1 + 1e-4) along u (drag.rs works the end's case out), and the start
    // lands at the centre plus 5u. The centre's pin is weak, so SQP stops short of that
    // optimum by up to 1e-6: FreeCAD 1.1.3, for the cursor (7, -4), puts the centre at
    // (3.4642203391501472, -0.46421903106136109), 5.6e-7 off the δ above (Silico 6.7e-7).
    // drag.rs's 1e-9 ("a point lands within this of where it should") holds only for the
    // cursors it picked.
    let (system, arc, _, center, start, _) = quarter_arc(true);
    let near = |got: [f64; 2], want: [f64; 2], what: &str| {
        let off = libm::hypot(got[0] - want[0], got[1] - want[1]);
        assert!(
            off <= 1e-6,
            "{what}: got {got:?}, want {want:?} ({off:e} off)"
        );
    };
    let mut drag = begin(system, PointOf::Start(arc));
    for cursor in [[13.0, 2.0], [7.0, -4.0], [4.0, 6.0], [1.5, 2.0]] {
        let values = drag.move_to(cursor).unwrap();
        assert_eq!(values.status, SolveStatus::Success, "{cursor:?}");
        let values = values.values.unwrap();
        let (dx, dy) = (cursor[0] - 1.0, cursor[1] - 2.0);
        let d = libm::hypot(dx, dy);
        let u = [dx / d, dy / d];
        let delta = (d - 5.0) / (1.0 + 1e-4);
        let c = [1.0 + delta * u[0], 2.0 + delta * u[1]];
        near(at(&values, center), c, &format!("centre, {cursor:?}"));
        near(
            at(&values, start),
            [c[0] + 5.0 * u[0], c[1] + 5.0 * u[1]],
            &format!("start, {cursor:?}"),
        );
    }
}

#[test]
fn an_arcs_centre_dragged_goes_to_the_cursor_and_keeps_its_rules() {
    // The centre alone is pinned: it lands on the cursor, and the arc's start and end stay
    // on it at its radius and angles. Which way the angles went doesn't matter (the arc is
    // the counter-clockwise one from its start to its end): dragged to (100, −0.5), the
    // solve leaves the end angle a turn below where it reads, as FreeCAD draws it too.
    let (system, arc, radius, center, start, end) = quarter_arc(true);
    let mut drag = begin(system, PointOf::Center(arc));
    for cursor in [[-20.0, 30.0], [1.0, 2.0], [100.0, -0.5]] {
        let values = drag.move_to(cursor).unwrap().values.unwrap();
        assert_near(at(&values, center), cursor, "centre");
        let r = values[radius.0 as usize];
        assert!((r - 5.0).abs() <= CLOSE, "radius {r}");
        for (p, a) in [(start, values[3]), (end, values[4])] {
            assert_near(
                at(&values, p),
                [cursor[0] + r * libm::cos(a), cursor[1] + r * libm::sin(a)],
                "an end on the arc",
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Redundancy, read where the drag started.

#[test]
fn a_redundant_constraint_unmet_where_the_drag_starts_stalls_every_move_as_in_freecad() {
    // Spec question, pinned to FreeCAD: a line drawn off horizontal, constrained horizontal
    // twice over (Horizontal, and its ends 0 apart in y): one of the two is dropped as
    // redundant. PlaneGCS checks the dropped one after a solve on the values it started
    // from, before the solution is written back (GCS.cpp System::solve, 1901-1913), so with
    // the drawing off horizontal every move ends without values, though the kept constraint
    // solves exactly. The bridge's own solve reads this as depending "on where the sketch
    // was drawn, not on the solution" and reports SolvedExceptRedundant; the drag doesn't.
    // FreeCAD 1.1.3 does the same: `moveGeometry(0, 2, (7, 3))` of this sketch, unsolved,
    // returns 2 and leaves the end at (10, 1) (solved first, it returns 1, unmoved too).
    let mut b = Builder::default();
    let start = PointValues {
        x: b.fixed(0.0),
        y: b.fixed(0.0),
    };
    let end = b.point_values(10.0, 1.0);
    let line = b.add(SketchGeometry::Line { start, end });
    b.constrain(|tag| SolverConstraint::Horizontal { tag, line });
    let zero = b.fixed(0.0);
    b.constrain(|tag| SolverConstraint::DistanceY {
        tag,
        a: PointOf::Start(line),
        b: PointOf::End(line),
        value: zero,
    });
    let system = b.system(Algorithm::DogLeg);
    let solved = silico_sketch::solve(&system).unwrap();
    assert_eq!(solved.status, SolveStatus::SolvedExceptRedundant);
    assert_eq!(&solved.values[2..4], &[10.0, 0.0]);

    let mut drag = begin(system, PointOf::End(line));
    let moved = drag.move_to([7.0, 3.0]).unwrap();
    assert_ne!(moved.status, SolveStatus::Success, "{moved:?}");
    assert_eq!(moved.values, None);
}

#[test]
fn a_constraint_redundant_only_where_the_drag_starts_goes_unmet_as_in_freecad() {
    // Spec question, pinned to FreeCAD: a point 5 from a fixed centre and 5 right of it.
    // Where it's drawn, at (5, 0), both rows of the Jacobian are (1, 0), so the diagnosis
    // drops the second as redundant, though together the two hold the point at (5, 0)
    // alone. Dragged, the point goes round the circle as if the dropped one weren't there,
    // and each move is a Success that misses it by up to 10: FreeCAD 1.1.3's `moveGeometry`
    // gives the same, to the bit for (0, 10): (-4.748917405881524e-24, 5),
    // (4.975185951049946, -0.4975185951049943), and (3, 3.9999999999999996). A drag's
    // `Moved` carries no residuals (a solve's does), so nothing tells the core.
    let mut b = Builder::default();
    let c = b.fixed_point(0.0, 0.0);
    let p = b.point_values(5.0, 0.0);
    let point = b.add(SketchGeometry::Point(p));
    let five = b.fixed(5.0);
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Point(c),
        b: PointOf::Point(point),
        value: five,
    });
    b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Point(c),
        b: PointOf::Point(point),
        value: five,
    });
    let system = b.system(Algorithm::DogLeg);
    let mut drag = begin(system, PointOf::Point(point));
    for (cursor, freecad) in [
        ([0.0, 10.0], [-4.748917405881524e-24, 5.0]),
        ([-5.0, 0.5], [4.975185951049946, -0.4975185951049943]),
        ([3.0, 4.0], [3.0, 3.9999999999999996]),
    ] {
        let moved = drag.move_to(cursor).unwrap();
        assert_eq!(moved.status, SolveStatus::Success, "{cursor:?}");
        assert_near(
            at(&moved.values.unwrap(), p),
            freecad,
            &format!("{cursor:?}"),
        );
    }
}

#[test]
fn a_violated_dimension_between_fixed_points_beside_a_drag_is_named_or_refused() {
    // Two fixed points 3 apart, dimensioned 5, beside a free point that's dragged. PlaneGCS
    // solves only components with unknowns, so a move can say Success while the dimension
    // stays unmet; a solve shows it by its residual, a drag's Moved has none. Either the
    // begin names the dimension in conflict (and starts no drag, as FreeCAD's drag refuses
    // a sketch with conflicts), or each Success comes with a sketch whose dimension holds.
    let mut b = Builder::default();
    let p = b.fixed_point(0.0, 0.0);
    let q = b.fixed_point(3.0, 0.0);
    let five = b.fixed(5.0);
    let dimension = b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Point(p),
        b: PointOf::Point(q),
        value: five,
    });
    let free = b.point_values(1.0, 1.0);
    let free = b.add(SketchGeometry::Point(free));
    let system = b.system(Algorithm::DogLeg);
    let start = Drag::begin(&DragBegin {
        system,
        dragged: silico_protocol::Dragged::Point(PointOf::Point(free)),
    })
    .unwrap();
    match start.drag {
        None => assert_eq!(start.conflicting, [dimension]),
        Some(mut drag) => {
            let moved = drag.move_to([2.0, 2.0]).unwrap();
            assert_ne!(
                moved.status,
                SolveStatus::Success,
                "a drag of a sketch whose fixed points miss their dimension by 2: {moved:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Points that can't move.

#[test]
fn a_fixed_point_dragged_stays_and_the_rest_of_the_sketch_stays_where_it_began() {
    // The pin has no unknown, so the drag's part is empty: the point stays, and so does a
    // free line beside it, whose end is tied 3 right of its start though drawn 1 right: the
    // drag solves only its own part (FreeCAD's solves every part each move, which a solved
    // sketch leaves where it is anyway; the closing solve solves the rest).
    let mut b = Builder::default();
    let fixed = b.fixed_point(4.0, -1.0);
    let line = b.line((0.0, 0.0), (1.0, 1.0));
    let three = b.fixed(3.0);
    b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Start(line),
        b: PointOf::End(line),
        value: three,
    });
    let system = b.system(Algorithm::DogLeg);
    let mut drag = begin(system.clone(), PointOf::Point(fixed));
    let moved = drag.move_to([50.0, 50.0]).unwrap();
    assert_eq!(moved.status, SolveStatus::Success);
    let values = moved.values.unwrap();
    assert_eq!(values, system.values);
}

#[test]
fn a_sketch_with_no_unknowns_says_what_any_fixed_point_says() {
    // The same fixed point dragged in a sketch with nothing free: PlaneGCS's initSolution
    // returns before it diagnoses when there are no unknowns, so the degrees of freedom and
    // the moves' status must come out as they do beside free geometry: 0, and a Success
    // that leaves the point where it is.
    let mut b = Builder::default();
    let fixed = b.fixed_point(4.0, -1.0);
    let system = b.system(Algorithm::DogLeg);
    let start = Drag::begin(&DragBegin {
        system: system.clone(),
        dragged: silico_protocol::Dragged::Point(PointOf::Point(fixed)),
    })
    .unwrap();
    let solved = silico_sketch::solve(&system).unwrap();
    assert_eq!(
        start.dofs, solved.dofs,
        "the drag's diagnosis and the solve's disagree"
    );
    assert_eq!(start.dofs, 0, "nothing to move: no degrees of freedom");
    let mut drag = start.drag.unwrap();
    let moved = drag.move_to([1.0, 1.0]).unwrap();
    assert_eq!(
        (moved.status, moved.values),
        (SolveStatus::Success, Some(vec![4.0, -1.0])),
        "a fixed point stays, as it does beside free geometry"
    );
}

// ---------------------------------------------------------------------------------------
// Sessions side by side, threads, the worker's life.

#[test]
fn two_drags_interleaved_in_one_process_agree_with_each_alone() {
    let (a_system, a_line) = line_from_the_origin(true);
    let (b_system, b_arc, ..) = quarter_arc(true);
    let a_path = [[3.0, 1.0], [-7.0, 2.0], [9.5, -4.0]];
    let b_path = [[1.0, 17.0], [-11.0, -7.0], [4.0, 6.0]];
    let alone = |system: &SketchSystem, point, path: &[[f64; 2]]| {
        let mut d = begin(system.clone(), point);
        path.iter()
            .map(|&c| {
                let m = d.move_to(c).unwrap();
                (m.status, m.values.as_deref().map(bits))
            })
            .collect::<Vec<_>>()
    };
    let a_alone = alone(&a_system, PointOf::End(a_line), &a_path);
    let b_alone = alone(&b_system, PointOf::End(b_arc), &b_path);
    let mut a = begin(a_system, PointOf::End(a_line));
    let mut b = begin(b_system, PointOf::End(b_arc));
    for i in 0..3 {
        let ma = a.move_to(a_path[i]).unwrap();
        let mb = b.move_to(b_path[i]).unwrap();
        assert_eq!((ma.status, ma.values.as_deref().map(bits)), a_alone[i]);
        assert_eq!((mb.status, mb.values.as_deref().map(bits)), b_alone[i]);
    }
}

#[test]
fn drags_on_many_threads_at_once_agree_bit_for_bit() {
    let (system, arc, ..) = quarter_arc(true);
    let path: Vec<[f64; 2]> = (0..40)
        .map(|i| [1.0 + 0.3 * i as f64, 17.0 - 0.5 * i as f64])
        .collect();
    let run = || {
        let mut d = begin(system.clone(), PointOf::End(arc));
        path.iter()
            .map(|&c| {
                let m = d.move_to(c).unwrap();
                (m.status, m.values.as_deref().map(bits))
            })
            .collect::<Vec<_>>()
    };
    let alone = run();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8).map(|_| scope.spawn(run)).collect();
        for h in handles {
            assert_eq!(h.join().unwrap(), alone);
        }
    });
}

#[test]
fn a_thousand_begins_in_one_state_each_replace_the_last() {
    // Each begin ends the drag before it: after many, a move goes to the last one's point.
    let (line_system, line) = line_from_the_origin(false);
    let (arc_system, arc, ..) = quarter_arc(false);
    let mut state = State::default();
    for i in 0..1000 {
        let op = if i % 2 == 0 {
            Op::DragBegin(DragBegin {
                system: line_system.clone(),
                dragged: silico_protocol::Dragged::Point(PointOf::End(line)),
            })
        } else {
            Op::DragBegin(DragBegin {
                system: arc_system.clone(),
                dragged: silico_protocol::Dragged::Point(PointOf::End(arc)),
            })
        };
        assert!(matches!(state.handle(op), Ok(Reply::DragStarted { .. })));
    }
    // The last was the arc's: its values, 13 of them.
    match state.handle(Op::DragMove { to: [-6.0, 9.0] }).unwrap() {
        Reply::Moved(Moved {
            status: SolveStatus::Success,
            values: Some(values),
        }) => {
            assert_eq!(values.len(), arc_system.values.len());
            assert_near([values[7], values[8]], [-6.0, 9.0], "the arc's end");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_debug_op_between_moves_leaves_the_drag_alone_and_a_crash_ends_it() {
    // The test worker answers debug operations beside the drag: a sleep leaves the drag
    // under way; a crash takes it with the process, and the next worker has none, refused
    // as the refusal's suggestion says ("when the sketch worker restarts").
    let (system, line) = line_from_the_origin(false);
    let mut w = Worker::spawn_as(Path::new(TEST_WORKER), WorkerKind::Sketch).unwrap();
    let call = |w: &mut Worker, op| w.call(Generation(1), op, WAIT);
    let begin = Op::DragBegin(DragBegin {
        system,
        dragged: silico_protocol::Dragged::Point(PointOf::End(line)),
    });
    assert!(matches!(
        call(&mut w, begin.clone()),
        Ok(Reply::DragStarted { .. })
    ));
    assert!(matches!(
        call(&mut w, Op::Debug(DebugOp::Sleep { millis: 1 })),
        Ok(Reply::Done)
    ));
    match call(&mut w, Op::DragMove { to: [1.0, 2.0] }).unwrap() {
        Reply::Moved(Moved {
            status: SolveStatus::Success,
            values: Some(values),
        }) => assert_eq!(values, [0.0, 0.0, 1.0, 2.0]),
        other => panic!("{other:?}"),
    }
    let err = call(&mut w, Op::Debug(DebugOp::Segfault)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let mut w = Worker::spawn_as(Path::new(TEST_WORKER), WorkerKind::Sketch).unwrap();
    let err = call(&mut w, Op::DragMove { to: [1.0, 2.0] }).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert_eq!(
        err.message(),
        "DragMove needs a drag under way, and there's none"
    );
    // And a begin there works.
    assert!(matches!(call(&mut w, begin), Ok(Reply::DragStarted { .. })));
}

#[test]
fn a_worker_killed_mid_drag_is_reported_and_its_successor_has_no_drag() {
    let (system, line) = line_from_the_origin(false);
    let mut w = Worker::spawn_as(Path::new(WORKER), WorkerKind::Sketch).unwrap();
    assert!(matches!(
        w.call(
            Generation(1),
            Op::DragBegin(DragBegin {
                system,
                dragged: silico_protocol::Dragged::Point(PointOf::End(line)),
            }),
            WAIT
        ),
        Ok(Reply::DragStarted { .. })
    ));
    let pid = w.pid();
    let status = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let err = w
        .call(Generation(1), Op::DragMove { to: [1.0, 1.0] }, WAIT)
        .unwrap_err();
    assert!(
        matches!(
            err.code(),
            ErrorCode::WorkerCrashed | ErrorCode::ProtocolError
        ),
        "{err} ({:?})",
        err.code()
    );
    let mut w = Worker::spawn_as(Path::new(WORKER), WorkerKind::Sketch).unwrap();
    let err = w.call(Generation(1), Op::DragEnd, WAIT).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
}

// ---------------------------------------------------------------------------------------
// Size: what the check admits must be answered, one frame at a time.

#[test]
fn a_move_of_the_largest_part_a_drag_admits_is_answered_in_time() {
    // Was a BREAK (twice): a pinned part goes to SQP (GCS.cpp 4502), dense, up to 100
    // iterations, so a move costs more than a solve. A chain linked end to end and pulled
    // far from where it began is the worst found (the review's): a move takes 0.76 s at 256
    // unknowns on a loaded machine, 6 s at 512, 64 s at 1,024, past the request's time,
    // which killed the worker mid-drag. Fixed: a drag takes at most MAX_DRAG_UNKNOWNS (256)
    // in the part it moves, and a move of that worst case at the limit comes back in time.
    let links = silico_protocol::MAX_DRAG_UNKNOWNS / 4;
    let (system, last) = linked_chain(links);
    assert_eq!(system.part(&[PointOf::End(last)]).unknowns.len(), 256);
    let mut w = Worker::spawn_as(Path::new(WORKER), WorkerKind::Sketch).unwrap();
    match w.call(
        Generation(1),
        Op::DragBegin(DragBegin {
            system,
            dragged: silico_protocol::Dragged::Point(PointOf::End(last)),
        }),
        WAIT,
    ) {
        Ok(Reply::DragStarted { dofs, conflicting }) => {
            // 4 unknowns a link, 3 equations a link (its length, and the two joining it to
            // the one before, or to the origin): one degree of freedom a link.
            assert_eq!((dofs, conflicting.as_slice()), (64, &[][..]));
        }
        other => panic!("begin: {other:?}"),
    }
    let share = crate::share::Share::start();
    let t = Instant::now();
    let moved = w.call(
        Generation(1),
        Op::DragMove {
            to: [-(links as f64) / 2.0, 10.0],
        },
        WAIT,
    );
    let (took, slowdown) = (t.elapsed(), share.finish().slowdown);
    match moved {
        Ok(Reply::Moved(_)) => {}
        other => panic!("a move of the largest admitted drag, after {took:?}: {other:?}"),
    }
    assert!(
        took.div_f64(slowdown) < TIMEOUT,
        "a move of the largest admitted drag took {took:?}, the machine {slowdown:.2} times \
         slower than a quiet one"
    );
}

/// `n` unit links joined end to end in a zigzag, the first start held at the origin: one
/// part of 4n unknowns, n degrees of freedom.
fn linked_chain(n: usize) -> (SketchSystem, Geometry) {
    let mut b = Builder::default();
    let (zero, one) = (b.fixed(0.0), b.fixed(1.0));
    let origin = b.fixed_point(0.0, 0.0);
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let mut lines = Vec::new();
    for i in 0..n {
        let rise = if i % 2 == 0 { 1.0 } else { -1.0 } * 0.75f64.sqrt();
        let (nx, ny) = (x + 0.5, y + rise);
        lines.push(b.line((x, y), (nx, ny)));
        (x, y) = (nx, ny);
    }
    let start = PointOf::Start(lines[0]);
    b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Point(origin),
        b: start,
        value: zero,
    });
    b.constrain(|tag| SolverConstraint::DistanceY {
        tag,
        a: PointOf::Point(origin),
        b: start,
        value: zero,
    });
    for &l in &lines {
        b.constrain(|tag| SolverConstraint::Distance {
            tag,
            a: PointOf::Start(l),
            b: PointOf::End(l),
            value: one,
        });
    }
    for w in lines.windows(2) {
        let (a, c) = (PointOf::End(w[0]), PointOf::Start(w[1]));
        b.constrain(|tag| SolverConstraint::DistanceX {
            tag,
            a,
            b: c,
            value: zero,
        });
        b.constrain(|tag| SolverConstraint::DistanceY {
            tag,
            a,
            b: c,
            value: zero,
        });
    }
    (b.system(Algorithm::DogLeg), lines[n - 1])
}

#[test]
fn a_drag_is_bounded_by_the_part_it_moves_not_the_whole_sketch() {
    // 257 unknowns in the dragged part: one past the limit, named.
    let (system, last) = linked_chain(64);
    let mut b = system.clone();
    let extra = b.values.len() as u32;
    b.values.push(0.5);
    b.unknowns.push(Value(extra));
    // The extra unknown joined to the chain's last end through a DistanceX.
    let value = Value(b.values.len() as u32);
    b.values.push(0.0);
    let spare = Geometry(b.geometry.len() as u32);
    let p = PointValues {
        x: Value(extra),
        y: Value(0),
    };
    b.geometry.push(SketchGeometry::Point(p));
    let tag = Tag(b.constraints.len() as u32 + 1);
    b.constraints.push(SolverConstraint::DistanceX {
        tag,
        a: PointOf::End(last),
        b: PointOf::Point(spare),
        value,
    });
    b.check().unwrap();
    assert_eq!(b.part(&[PointOf::End(last)]).unknowns.len(), 257);
    let err = DragBegin {
        system: b,
        dragged: silico_protocol::Dragged::Point(PointOf::End(last)),
    }
    .check()
    .unwrap_err();
    assert_eq!(err.parameter, "point");
    assert_eq!(
        err.message,
        "the part of the sketch the drag moves has 257 unknowns, and a drag takes at most 256 there: each move of a larger part takes too long"
    );
    // At the limit: admitted.
    DragBegin {
        system,
        dragged: silico_protocol::Dragged::Point(PointOf::End(last)),
    }
    .check()
    .unwrap();
    // A small part of a large sketch: a free point beside a chain of 1,020 unknowns, 1,022
    // in all, within MAX_SKETCH_UNKNOWNS.
    let (mut big, _) = linked_chain(255);
    let at = big.values.len() as u32;
    big.values.extend([3.0, 4.0]);
    big.unknowns.extend([Value(at), Value(at + 1)]);
    let lone = Geometry(big.geometry.len() as u32);
    big.geometry.push(SketchGeometry::Point(PointValues {
        x: Value(at),
        y: Value(at + 1),
    }));
    assert_eq!(big.part(&[PointOf::Point(lone)]).unknowns.len(), 2);
    DragBegin {
        system: big,
        dragged: silico_protocol::Dragged::Point(PointOf::Point(lone)),
    }
    .check()
    .unwrap();
}

/// `n` free lines of length 1 in a row, each end joined to the next start by a coincidence.
fn coincident_chain_into(b: &mut Builder, n: usize) -> Vec<Geometry> {
    let lines: Vec<Geometry> = (0..n)
        .map(|i| b.line((i as f64, 0.0), (i as f64 + 0.5, 1.0)))
        .collect();
    let one = b.fixed(1.0);
    for &l in &lines {
        b.constrain(|tag| SolverConstraint::Distance {
            tag,
            a: PointOf::Start(l),
            b: PointOf::End(l),
            value: one,
        });
    }
    for w in lines.windows(2) {
        let (a, c) = (PointOf::End(w[0]), PointOf::Start(w[1]));
        b.constrain(|tag| SolverConstraint::Coincident { tag, a, b: c });
    }
    lines
}

#[test]
fn a_drag_counts_every_part_its_pins_hold() {
    // A chain joined by coincidences, as sketches join lines: 65 lines, 260 unknowns, one
    // part, past the limit.
    let mut b = Builder::default();
    let lines = coincident_chain_into(&mut b, 65);
    let last = *lines.last().unwrap();
    let system = b.system(Algorithm::DogLeg);
    system.check().unwrap();
    assert_eq!(system.part(&[PointOf::End(last)]).unknowns.len(), 260);
    let err = DragBegin {
        system,
        dragged: silico_protocol::Dragged::Point(PointOf::End(last)),
    }
    .check()
    .unwrap_err();
    assert_eq!(err.parameter, "point");
    // An arc whose centre is joined to that chain's end (without its rules, which the
    // protocol allows), its end dragged: the end's own part is 2 unknowns, but its centre is
    // pinned too, so the drag holds the chain's part: 260 + the centre's.
    let mut b = Builder::default();
    let lines = coincident_chain_into(&mut b, 64);
    let last = *lines.last().unwrap();
    let (cx, cy) = (40.0, 3.0);
    let center = b.point_values(cx, cy);
    let radius = b.unknown(5.0);
    let start_angle = b.unknown(0.0);
    let end_angle = b.unknown(std::f64::consts::FRAC_PI_2);
    let start = b.point_values(cx + 5.0, cy);
    let end = b.point_values(cx, cy + 5.0);
    let arc = b.add(SketchGeometry::Arc {
        center,
        radius,
        start_angle,
        end_angle,
        start,
        end,
    });
    b.constrain(|tag| SolverConstraint::Coincident {
        tag,
        a: PointOf::End(last),
        b: PointOf::Center(arc),
    });
    let system = b.system(Algorithm::DogLeg);
    system.check().unwrap();
    assert_eq!(system.part(&[PointOf::End(arc)]).unknowns.len(), 2);
    let begin = DragBegin {
        system,
        dragged: silico_protocol::Dragged::Point(PointOf::End(arc)),
    };
    // The end's 2, and the centre's with the chain's: 2 + 256 + 2.
    assert_eq!(begin.part().unknowns.len(), 260);
    assert_eq!(begin.check().unwrap_err().parameter, "point");
}

#[test]
fn a_move_solves_only_the_part_it_moves_and_leaves_the_rest_where_it_was() {
    // A lone free point beside a chain that can't be closed (its last end held 800 from the
    // origin by 64 unit links, 256 unknowns): the whole sketch would fail every frame,
    // solved anew each move (FreeCAD's System::solve solves every part). The drag holds only
    // the point's part, so each move solves the point, and the chain stays as it was drawn.
    let (mut chain, last) = linked_chain(64);
    let far = Value(chain.values.len() as u32);
    chain.values.push(800.0);
    let origin = PointOf::Point(Geometry(0));
    chain.constraints.push(SolverConstraint::Distance {
        tag: Tag(chain.constraints.len() as u32 + 1),
        a: origin,
        b: PointOf::End(last),
        value: far,
    });
    let at = chain.values.len() as u32;
    chain.values.extend([3.0, 4.0]);
    chain.unknowns.extend([Value(at), Value(at + 1)]);
    let lone = Geometry(chain.geometry.len() as u32);
    chain.geometry.push(SketchGeometry::Point(PointValues {
        x: Value(at),
        y: Value(at + 1),
    }));
    chain.check().unwrap();
    let before = chain.values.clone();
    let mut drag = begin(chain, PointOf::Point(lone));
    for cursor in [[3.05, 4.0], [3.1, 4.1], [5.0, -2.0]] {
        let m = drag.move_to(cursor).unwrap();
        assert_eq!(m.status, SolveStatus::Success, "{cursor:?}");
        let values = m.values.unwrap();
        assert_near(
            [values[at as usize], values[at as usize + 1]],
            cursor,
            "the point",
        );
        for (i, (a, b)) in before.iter().zip(&values).enumerate() {
            if i != at as usize && i != at as usize + 1 {
                assert_eq!(a.to_bits(), b.to_bits(), "value {i} moved");
            }
        }
    }
}

#[test]
fn an_arc_of_negative_radius_or_a_line_too_short_to_measure_is_no_frame() {
    // An arc held at a radius of -1 (its rules join the radius to its centre), its centre
    // dragged: solved, but not an arc. (Held at 0, PlaneGCS lands on 7.7e-34: an arc.)
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
    let value = b.fixed(-1.0);
    b.constrain(|tag| SolverConstraint::Radius {
        tag,
        circle: arc,
        value,
    });
    let system = b.system(Algorithm::DogLeg);
    system.check().unwrap();
    let mut drag = begin(system, PointOf::Center(arc));
    let m = drag.move_to([3.0, 4.0]).unwrap();
    assert_eq!(
        (m.status, &m.values),
        (SolveStatus::SolvedInvalid, &None),
        "radius -1"
    );
    // A circle held at a radius of -1, its centre dragged: its radius isn't in the part the
    // drag moves (nothing joins it to the centre), so the circle moves as it was, radius 2.
    let mut b = Builder::default();
    let center = b.point_values(0.0, 0.0);
    let radius = b.unknown(2.0);
    let circle = b.add(SketchGeometry::Circle { center, radius });
    let value = b.fixed(-1.0);
    b.constrain(|tag| SolverConstraint::Radius { tag, circle, value });
    let system = b.system(Algorithm::DogLeg);
    system.check().unwrap();
    let mut drag = begin(system, PointOf::Center(circle));
    let m = drag.move_to([3.0, 4.0]).unwrap();
    assert_eq!(m.status, SolveStatus::Success);
    assert_eq!(m.values.as_deref().map(|v| v[2]), Some(2.0));
    // A line whose end lands 1e-200 from its start: no length to the solver.
    let mut b = Builder::default();
    let start = PointValues {
        x: b.fixed(0.0),
        y: b.fixed(0.0),
    };
    let end = b.point_values(1e-200, 1.0);
    let line = b.add(SketchGeometry::Line { start, end });
    let system = b.system(Algorithm::DogLeg);
    system.check().unwrap();
    let mut drag = begin(system, PointOf::End(line));
    let m = drag.move_to([1e-200, 0.0]).unwrap();
    assert_eq!(
        (m.status, &m.values),
        (SolveStatus::SolvedInvalid, &None),
        "{m:?}"
    );
}

// ---------------------------------------------------------------------------------------
// Generated sketches: every promise of every move.

fn point_values(s: &SketchSystem, p: PointOf) -> PointValues {
    let (g, which) = match p {
        PointOf::Point(g) => (g, 0),
        PointOf::Start(g) => (g, 1),
        PointOf::End(g) => (g, 2),
        PointOf::Center(g) => (g, 3),
    };
    match (s.geometry[g.0 as usize], which) {
        (SketchGeometry::Point(p), 0) => p,
        (SketchGeometry::Line { start, .. } | SketchGeometry::Arc { start, .. }, 1) => start,
        (SketchGeometry::Line { end, .. } | SketchGeometry::Arc { end, .. }, 2) => end,
        (SketchGeometry::Circle { center, .. } | SketchGeometry::Arc { center, .. }, 3) => center,
        other => panic!("{other:?}"),
    }
}

/// Whether `c` reads an unknown (else PlaneGCS leaves it to its residual).
fn touches_an_unknown(s: &SketchSystem, c: &SolverConstraint) -> bool {
    let unknown = |v: Value| s.unknowns.binary_search(&v).is_ok();
    let geometry = |g: Geometry| s.geometry[g.0 as usize].values().into_iter().any(unknown);
    let point = |p: PointOf| {
        let pv = point_values(s, p);
        unknown(pv.x) || unknown(pv.y)
    };
    match *c {
        SolverConstraint::ArcRules { arc } => geometry(arc),
        SolverConstraint::Coincident { a, b, .. }
        | SolverConstraint::DistanceX { a, b, .. }
        | SolverConstraint::DistanceY { a, b, .. }
        | SolverConstraint::Distance { a, b, .. } => point(a) || point(b),
        SolverConstraint::Horizontal { line, .. } | SolverConstraint::Vertical { line, .. } => {
            geometry(line)
        }
        SolverConstraint::PointOnLine { point: p, line, .. } => point(p) || geometry(line),
        SolverConstraint::Parallel { a, b, .. }
        | SolverConstraint::Perpendicular { a, b, .. }
        | SolverConstraint::EqualLength { a, b, .. }
        | SolverConstraint::Angle { a, b, .. } => geometry(a) || geometry(b),
        SolverConstraint::Radius { circle, .. } => geometry(circle),
        // Not generated here: M3.3's forms have their own tests.
        other => panic!("not a form this test generates: {other:?}"),
    }
}

/// Whether `c` measures by a line the move left shorter than 1e-6, whose direction is
/// noise: those lines are NextRefused's (exactly 0) or ill-conditioned, not unmet.
fn on_a_point_line(s: &SketchSystem, v: &[f64], c: &SolverConstraint) -> bool {
    let short = |l: Geometry| {
        let d = dir(s, v, l);
        libm::hypot(d.0, d.1) < 1e-6
    };
    match *c {
        SolverConstraint::PointOnLine { line, .. } => short(line),
        SolverConstraint::Parallel { a, b, .. }
        | SolverConstraint::Perpendicular { a, b, .. }
        | SolverConstraint::Angle { a, b, .. } => short(a) || short(b),
        _ => false,
    }
}

/// What may go wrong with one generated drag, by kind, so failures that share a cause can
/// be told apart. Over 8,000 seeds, at the time of writing: NextRefused 135 (lines of no
/// length), nothing else once redundant constraints are left to the spec question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Broken {
    /// The begin refused a system its check passed, or the bridge failed.
    Refused,
    /// The drag's diagnosis isn't the solve's of the same system.
    Diagnosis,
    /// A session with conflicts, or none without.
    Session,
    /// A move answered with something other than a frame or "out of reach".
    MoveFailed,
    /// Status and values disagree, or values of another length.
    Shape,
    /// A value outside the part the drag moves changed.
    FixedMoved,
    /// A Success with a constraint the solver solved unmet, or not a number.
    Unmet,
    /// A Success the next solve's check refuses.
    NextRefused,
    /// A move from a fresh begin straight to the cursor gives other bits.
    Nondeterministic,
}

fn check_drag(seed: u64) -> Result<(), (Broken, String)> {
    let mut system = generate(seed, false, Some(Algorithm::DogLeg));
    if system.check().is_err() {
        return Ok(());
    }
    let mut rng = Rng(seed ^ 0x00d1_a600_d1a6);
    // Half the time from the solve's solution, as a drag usually starts.
    let Ok(first) = silico_sketch::solve(&system) else {
        return Ok(());
    };
    if rng.below(2) == 0 && first.status == SolveStatus::Success && first.conflicting.is_empty() {
        system.values = first.values;
        if system.check().is_err() {
            return Ok(());
        }
    }
    let Ok(diagnosis) = silico_sketch::solve(&system) else {
        return Ok(());
    };
    let points: Vec<PointOf> = system
        .geometry
        .iter()
        .enumerate()
        .flat_map(|(i, &g)| points_of(g, Geometry(i as u32)))
        .collect();
    let point = points[rng.below(points.len() as u64) as usize];
    let begin = DragBegin {
        system: system.clone(),
        dragged: silico_protocol::Dragged::Point(point),
    };
    let start = Drag::begin(&begin).map_err(|f| (Broken::Refused, format!("{f:?}")))?;
    if (start.dofs, &start.conflicting) != (diagnosis.dofs, &diagnosis.conflicting) {
        return Err((
            Broken::Diagnosis,
            format!(
                "drag ({}, {:?}), solve ({}, {:?})",
                start.dofs, start.conflicting, diagnosis.dofs, diagnosis.conflicting
            ),
        ));
    }
    let Some(mut drag) = start.drag else {
        return if start.conflicting.is_empty() {
            Err((Broken::Session, "no session, no conflicts".into()))
        } else {
            Ok(())
        };
    };
    if !start.conflicting.is_empty() {
        return Err((Broken::Session, "a session with conflicts".into()));
    }
    let loose: Vec<Tag> = diagnosis
        .redundant
        .iter()
        .chain(&diagnosis.partially_redundant)
        .copied()
        .collect();
    // Cursors: anywhere, on another point (coincidence), on the dragged point's own other
    // points, where it is, and a hair off where it is.
    let here = xy(&system, &system.values, point);
    let mut cursors = Vec::new();
    for _ in 0..6 {
        let c = match rng.below(5) {
            0 | 1 => [rng.range(-60.0, 60.0), rng.range(-60.0, 60.0)],
            2 => {
                let other = points[rng.below(points.len() as u64) as usize];
                let (x, y) = xy(&system, &system.values, other);
                [x, y]
            }
            3 => [here.0, here.1],
            _ => [here.0 + 1e-9, here.1 - 1e-9],
        };
        cursors.push(c);
    }
    let unknown = |i: usize| system.unknowns.binary_search(&Value(i as u32)).is_ok();
    let part = begin.part();
    for c in cursors {
        let moved = match drag.move_to(c) {
            Ok(m) => m,
            Err(f) if matches!(f.refusal, Refusal::SolutionOutOfReach { .. }) => continue,
            Err(f) => return Err((Broken::MoveFailed, format!("to {c:?}: {f:?}"))),
        };
        let values = match (moved.status, moved.values.clone()) {
            (SolveStatus::Success, Some(v)) if v.len() == system.values.len() => v,
            (SolveStatus::Converged | SolveStatus::Failed | SolveStatus::SolvedInvalid, None) => {
                continue;
            }
            other => return Err((Broken::Shape, format!("to {c:?}: {other:?}"))),
        };
        // Only the part the drag moves is solved; every other value (fixed, or an unknown of
        // another part) stays where the drag began, bit for bit, so the part is held to what
        // a move does, not only to its own list.
        for (i, (a, b)) in system.values.iter().zip(&values).enumerate() {
            if part.unknowns.binary_search(&Value(i as u32)).is_err() && a.to_bits() != b.to_bits()
            {
                let what = if unknown(i) {
                    "an unknown outside the part"
                } else {
                    "fixed"
                };
                return Err((
                    Broken::FixedMoved,
                    format!("to {c:?}: value {i}, {what}, {a} -> {b}"),
                ));
            }
        }
        for con in part.constraints.iter().filter(|con| {
            touches_an_unknown(&system, con) && !on_a_point_line(&system, &values, con)
        }) {
            // Dropped as redundant where the drag starts: never looked at again, so a move
            // may leave it unmet, as FreeCAD's does (the spec question pinned by
            // a_constraint_redundant_only_where_the_drag_starts_goes_unmet_as_in_freecad;
            // seed 0x4c6a32c90cf09fd0, an arc's end coincident with its centre, drawn at
            // radius 0, is one: the coincidence is dropped, and the arc opens to 5.6).
            if con.tag().is_some_and(|t| loose.contains(&t)) {
                continue;
            }
            let tolerance = 1e-7;
            for r in residuals(&system, &values, con) {
                if r.is_nan() || r.abs() > tolerance {
                    return Err((
                        Broken::Unmet,
                        format!("to {c:?}: Success, but {con:?} is off by {r:e}; {values:?}"),
                    ));
                }
            }
        }
        if let Err(e) = with_values(&system, values.clone()).check() {
            return Err((
                Broken::NextRefused,
                format!("to {c:?}: Success, then refused: {}", e.message),
            ));
        }
        let mut fresh = Drag::begin(&begin)
            .map_err(|f| (Broken::Refused, format!("{f:?}")))?
            .drag
            .ok_or((Broken::Session, "the second begin has no session".into()))?;
        let again = fresh
            .move_to(c)
            .map_err(|f| (Broken::Nondeterministic, format!("{f:?}")))?;
        if again.status != moved.status || again.values.as_deref().map(bits) != Some(bits(&values))
        {
            return Err((
                Broken::Nondeterministic,
                format!("to {c:?}: walked {moved:?}, straight {again:?}"),
            ));
        }
    }
    Ok(())
}

// BREAKS: NextRefused, a Success with a line of no length (minimized by proptest to a free
// line with no constraints, its start dragged onto its end; seed 0xed579d2002cd9ad9), the
// root cause of the targeted line tests above.
proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn generated_drags_keep_every_promise(seed in any::<u64>()) {
        if let Err((kind, problem)) = check_drag(seed) {
            prop_assert!(false, "seed {seed:#x}: {kind:?}: {problem}\n{:?}", generate(seed, false, Some(Algorithm::DogLeg)));
        }
    }
}

#[test]
fn a_circle_drawn_at_radius_zero_dragged_is_no_frame() {
    // Its radius is fixed at 0 and its centre free: the frame is solved, but no circle.
    let mut b = Builder::default();
    let center = b.point_values(0.0, 0.0);
    let radius = b.fixed(0.0);
    let circle = b.add(SketchGeometry::Circle { center, radius });
    let system = b.system(Algorithm::DogLeg);
    system.check().unwrap();
    let mut drag = begin(system, PointOf::Center(circle));
    let m = drag.move_to([3.0, 4.0]).unwrap();
    assert_eq!((m.status, &m.values), (SolveStatus::SolvedInvalid, &None));
}

#[test]
fn geometry_outside_the_part_doesnt_void_a_frame() {
    // A circle drawn at radius 0, away from a lone point dragged: the drag doesn't move the
    // circle, so the frame stands.
    let mut b = Builder::default();
    let center = b.point_values(10.0, 10.0);
    let radius = b.fixed(0.0);
    b.add(SketchGeometry::Circle { center, radius });
    let p = b.point_values(1.0, 1.0);
    let lone = b.add(SketchGeometry::Point(p));
    let system = b.system(Algorithm::DogLeg);
    system.check().unwrap();
    let before = system.values.clone();
    let mut drag = begin(system, PointOf::Point(lone));
    let m = drag.move_to([3.0, 4.0]).unwrap();
    assert_eq!(m.status, SolveStatus::Success, "{m:?}");
    // The circle (values 0..3) as drawn, the point (3, 4) on the cursor.
    let values = m.values.unwrap();
    assert_eq!(bits(&values[..3]), bits(&before[..3]));
    assert_near([values[3], values[4]], [3.0, 4.0], "the point");
}
