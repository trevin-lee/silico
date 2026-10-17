//! M3.1: PlaneGCS through the bridge, in this process. Expected values are worked out by
//! hand: a rectangle's corners, an arc's end points, and the degrees of freedom of each
//! sketch (unknowns less independent equations).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/systems.rs"]
mod systems;

use silico_protocol::{
    Algorithm, PointOf, PointValues, Refusal, SketchGeometry, SolveStatus, SolverConstraint, Tag,
};
use systems::{Builder, line_values, rectangle, rectangle_40_by_20};

/// The solver meets its constraints to its convergence, 1e-10 on each residual: corners
/// land within that of their places.
const CLOSE: f64 = 1e-9;

fn assert_close(got: [f64; 4], want: [f64; 4], what: &str) {
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() <= CLOSE, "{what}: got {got:?}, want {want:?}");
    }
}

#[test]
fn a_constrained_rectangle_solves_to_its_corners() {
    for algorithm in [
        Algorithm::DogLeg,
        Algorithm::LevenbergMarquardt,
        Algorithm::Bfgs,
    ] {
        let r = rectangle_40_by_20();
        let [bottom, right, top, left] = r.sides;
        let system = r.b.system(algorithm);
        system.check().unwrap();
        let solved = silico_sketch::solve(&system).unwrap();
        assert_eq!(solved.status, SolveStatus::Success, "{algorithm:?}");
        // 16 unknowns; 4 coincidences of 2 equations, 4 directions, the corner pinned (2),
        // and the two lengths: 16 equations, all independent.
        assert_eq!(solved.dofs, 0, "{algorithm:?}");
        assert!(solved.conflicting.is_empty() && solved.redundant.is_empty());
        assert!(solved.partially_redundant.is_empty() && solved.free.is_empty());
        let v = &solved.values;
        assert_close(
            line_values(&system, v, bottom),
            [0.0, 0.0, 40.0, 0.0],
            "bottom",
        );
        assert_close(
            line_values(&system, v, right),
            [40.0, 0.0, 40.0, 20.0],
            "right",
        );
        assert_close(line_values(&system, v, top), [40.0, 20.0, 0.0, 20.0], "top");
        assert_close(line_values(&system, v, left), [0.0, 20.0, 0.0, 0.0], "left");
        // Fixed values are left as they were.
        assert_eq!(&v[..2], &[0.0, 0.0]);
        assert_eq!(&v[18..], &[40.0, 20.0]);
    }
}

#[test]
fn a_rectangle_without_its_lengths_has_two_degrees_of_freedom_its_width_and_height() {
    let system = rectangle().b.system(Algorithm::DogLeg);
    let solved = silico_sketch::solve(&system).unwrap();
    assert_eq!(solved.status, SolveStatus::Success);
    assert_eq!(solved.dofs, 2);
    assert!(solved.conflicting.is_empty() && solved.redundant.is_empty());
    // The pinned corner and the directions fix every coordinate but the width and the
    // height: the free values are the four x coordinates on the right side (the bottom's
    // end, the right side's ends, the top's start: values 4, 6, 8, 10) and the four y
    // coordinates on the top (the right side's end, the top's ends, the left side's start:
    // 9, 11, 13, 15). Values 0 and 1 are the fixed origin.
    let free: Vec<u32> = solved.free.iter().map(|v| v.0).collect();
    assert_eq!(free, vec![4, 6, 8, 9, 10, 11, 13, 15]);
}

#[test]
fn a_free_line_has_four_degrees_of_freedom_and_stays_where_it_is() {
    let mut b = Builder::default();
    b.line((1.0, 2.0), (3.0, 5.0));
    let system = b.system(Algorithm::DogLeg);
    let solved = silico_sketch::solve(&system).unwrap();
    assert_eq!(solved.dofs, 4);
    assert_eq!(solved.status, SolveStatus::Success);
    assert_eq!(solved.values, vec![1.0, 2.0, 3.0, 5.0]);
    // With no equations PlaneGCS lists nothing free, and says there were none, which
    // FreeCAD reads as every unknown free.
    assert!(solved.free.is_empty());
    assert!(solved.no_equations);
    assert!(solved.residuals.is_empty());
}

#[test]
fn a_horizontal_stated_twice_is_redundant_and_the_later_is_named() {
    // FreeCAD's TestCoincidentCommandGui fixture: the popularity contest drops the later of
    // two equal constraints (the higher tag), and the rest still fix every coordinate.
    let mut r = rectangle_40_by_20();
    let bottom = r.sides[0];
    let again =
        r.b.constrain(|tag| SolverConstraint::Horizontal { tag, line: bottom });
    let system = r.b.system(Algorithm::DogLeg);
    let solved = silico_sketch::solve(&system).unwrap();
    assert_eq!(solved.redundant, vec![again]);
    assert!(solved.conflicting.is_empty() && solved.partially_redundant.is_empty());
    assert_eq!(solved.dofs, 0);
    // PlaneGCS solves without the dropped constraint, then checks it on the values it
    // started from (GCS.cpp's System::solve, before anything is written back): this
    // rectangle starts with its bottom off horizontal, so PlaneGCS says Converged, which
    // from DogLeg can only mean that check. The bridge says so, with the solution: the
    // corners, and the dropped constraint's residual there, for the core to judge.
    assert_eq!(solved.status, SolveStatus::SolvedExceptRedundant);
    assert_eq!(solved.algorithm, Some(Algorithm::DogLeg));
    assert_close(
        line_values(&system, &solved.values, bottom),
        [0.0, 0.0, 40.0, 0.0],
        "bottom",
    );
    let (tag, residual) = solved.residuals[solved.residuals.len() - 1];
    assert_eq!(tag, again);
    assert!(residual.unwrap().abs() <= 1e-10, "{residual:?}");

    // Started where the constraints already hold, PlaneGCS's check passes and it says
    // Success; the bridge says the same as before, since the status mustn't depend on where
    // the sketch was drawn.
    let mut started_solved = system.clone();
    started_solved.values = silico_sketch::solve(&rectangle_40_by_20().b.system(Algorithm::DogLeg))
        .unwrap()
        .values;
    let solved = silico_sketch::solve(&started_solved).unwrap();
    assert_eq!(solved.redundant, vec![again]);
    assert_eq!(solved.status, SolveStatus::SolvedExceptRedundant);
    assert_eq!(solved.algorithm, Some(Algorithm::DogLeg));

    // BFGS can end Converged of its own accord, so its Converged can't be read as that
    // check, and stays unsolved, as FreeCAD reads it (Sketch::internalSolve takes only
    // Success): drawn off horizontal, the values come back as given; drawn solved, BFGS
    // says Success, so the solve stands, except the dropped constraint.
    let mut bfgs = system.clone();
    bfgs.algorithms = vec![Algorithm::Bfgs];
    let solved = silico_sketch::solve(&bfgs).unwrap();
    assert_eq!(solved.status, SolveStatus::Converged);
    assert_eq!(solved.algorithm, None);
    assert_eq!(solved.values, system.values);
    bfgs.values = started_solved.values;
    let solved = silico_sketch::solve(&bfgs).unwrap();
    assert_eq!(solved.status, SolveStatus::SolvedExceptRedundant);
    assert_eq!(solved.algorithm, Some(Algorithm::Bfgs));
}

/// A point 5 from the origin in x, and 5.000001 too, and 2 in y. The diagnosis drops the
/// second x distance as redundant (it repeats the first's equation to within the QR's
/// tolerance), and the solve meets the rest exactly.
fn disagreeing_widths_drawn_at(x: f64, y: f64) -> silico_protocol::Solved {
    let mut b = Builder::default();
    let origin = b.fixed_point(0.0, 0.0);
    let v = b.point_values(x, y);
    let p = b.add(SketchGeometry::Point(v));
    let (five, five_ish, two) = (b.fixed(5.0), b.fixed(5.000_001), b.fixed(2.0));
    for (value, x) in [(five, true), (five_ish, true), (two, false)] {
        b.constrain(|tag| {
            let (a, b) = (PointOf::Point(origin), PointOf::Point(p));
            if x {
                SolverConstraint::DistanceX { tag, a, b, value }
            } else {
                SolverConstraint::DistanceY { tag, a, b, value }
            }
        });
    }
    let system = b.system_trying(vec![
        Algorithm::DogLeg,
        Algorithm::LevenbergMarquardt,
        Algorithm::Bfgs,
    ]);
    system.check().unwrap();
    silico_sketch::solve(&system).unwrap()
}

#[test]
fn a_solve_with_equations_dropped_says_so_wherever_the_sketch_was_drawn() {
    // Found by the port review: PlaneGCS checks the dropped distance on the values it
    // started from, within its convergence (an error of 1e-5), so drawn between the two
    // widths it says Success, and drawn elsewhere Converged. The bridge says the same of
    // both: solved, except the dropped distance, whose residual is the 1e-6 it misses by.
    for (x, y) in [(5.000_000_5, 2.0), (7.0, 3.0)] {
        let solved = disagreeing_widths_drawn_at(x, y);
        assert_eq!(
            (solved.redundant.as_slice(), solved.status, solved.algorithm),
            (
                [Tag(2)].as_slice(),
                SolveStatus::SolvedExceptRedundant,
                Some(Algorithm::DogLeg)
            ),
            "drawn at ({x}, {y}): {solved:?}"
        );
        // Values: the origin (0, 1), the point (2, 3).
        for (v, want) in [(2, 5.0), (3, 2.0)] {
            assert!((solved.values[v] - want).abs() <= CLOSE, "{solved:?}");
        }
        let residual = solved.residuals[1].1.unwrap();
        assert!((residual.abs() - 1e-6).abs() <= 1e-12, "{residual}");
    }
}

#[test]
fn two_widths_that_disagree_conflict_and_both_are_named() {
    let mut r = rectangle_40_by_20();
    let bottom = r.sides[0];
    let other = r.b.fixed(50.0);
    let second = r.b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Start(bottom),
        b: PointOf::End(bottom),
        value: other,
    });
    let system = r.b.system(Algorithm::DogLeg);
    let solved = silico_sketch::solve(&system).unwrap();
    // The first width is tag 10 (four coincidences, four directions, the pin, then it).
    assert_eq!(solved.conflicting, vec![Tag(10), second]);
    assert!(solved.redundant.is_empty());
    // Over-constrained: 16 unknowns less 17 equations, none of them redundant.
    assert_eq!(solved.dofs, -1);
    assert_ne!(solved.status, SolveStatus::Success);
}

#[test]
fn an_arcs_end_points_go_to_its_angles() {
    // A fixed circle of radius 5 at (1, 2), from 30° to 120°: its rules put its start at
    // (1 + 5 cos 30°, 2 + 5 sin 30°) and its end at (1 + 5 cos 120°, 2 + 5 sin 120°).
    let mut b = Builder::default();
    let center = PointValues {
        x: b.fixed(1.0),
        y: b.fixed(2.0),
    };
    let radius = b.fixed(5.0);
    let start_angle = b.fixed(std::f64::consts::FRAC_PI_6);
    let end_angle = b.fixed(2.0 * std::f64::consts::FRAC_PI_3);
    let start = b.point_values(4.0, 3.0);
    let end = b.point_values(-1.0, 6.0);
    let arc = b.add(SketchGeometry::Arc {
        center,
        radius,
        start_angle,
        end_angle,
        start,
        end,
    });
    b.constraints.push(SolverConstraint::ArcRules { arc });
    let system = b.system(Algorithm::DogLeg);
    system.check().unwrap();
    let solved = silico_sketch::solve(&system).unwrap();
    assert_eq!(solved.status, SolveStatus::Success);
    assert_eq!(solved.dofs, 0);
    let half_root3 = 3.0_f64.sqrt() / 2.0;
    let want = [
        1.0 + 5.0 * half_root3,
        2.0 + 2.5,
        1.0 - 2.5,
        2.0 + 5.0 * half_root3,
    ];
    let got = [start.x, start.y, end.x, end.y].map(|v| solved.values[v.0 as usize]);
    assert_close(got, want, "arc ends");
}

#[test]
fn the_same_sketch_solves_to_the_same_bits() {
    let solve = || {
        let system = rectangle_40_by_20().b.system(Algorithm::LevenbergMarquardt);
        silico_sketch::solve(&system).unwrap()
    };
    let first = solve();
    for _ in 0..5 {
        let again = solve();
        assert_eq!(
            again.values.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            first.values.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!(again, first);
    }
}

#[test]
fn a_system_the_bridge_cant_read_is_its_exception_not_undefined_behavior() {
    // Unchecked on purpose: a constraint naming geometry that isn't there, which
    // SketchSystem::check refuses and the bridge checks again.
    let mut b = Builder::default();
    let line = b.line((0.0, 0.0), (1.0, 0.0));
    b.constraints.push(SolverConstraint::Parallel {
        tag: Tag(1),
        a: line,
        b: silico_protocol::Geometry(7),
    });
    let system = b.system(Algorithm::DogLeg);
    assert!(system.check().is_err());
    let failure = silico_sketch::solve(&system).unwrap_err();
    assert_eq!(failure.refusal, Refusal::SolverThrew);
    let cause = failure.cause.unwrap();
    assert!(cause.contains("geometry 7"), "{cause}");
}

#[test]
fn a_cpp_exception_reaches_rust_as_an_error() {
    let e = silico_sketch::test_ops::throw_runtime_error("from the test").unwrap_err();
    assert_eq!(e, "from the test");
}

#[test]
fn a_partly_redundant_sketch_is_solved_except_its_dropped_equation() {
    // Found by the port review, and FreeCAD 1.1.3 fails it (solve() -1): a point drawn at
    // (1, 2), made coincident with a fixed origin, and given x(origin) − x(point) = 0 and
    // x(point) − x(origin) = 0. PlaneGCS drops the coincidence's x equation (tag 1, so
    // partially redundant), solves the rest, and checks the dropped equation on the values
    // it started from (x = 1): Converged. The solution is the origin.
    let mut b = Builder::default();
    let o = b.fixed_point(0.0, 0.0);
    let v = b.point_values(1.0, 2.0);
    let p = b.add(SketchGeometry::Point(v));
    let zero = b.fixed(0.0);
    b.constrain(|tag| SolverConstraint::Coincident {
        tag,
        a: PointOf::Point(p),
        b: PointOf::Point(o),
    });
    b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Point(o),
        b: PointOf::Point(p),
        value: zero,
    });
    b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Point(p),
        b: PointOf::Point(o),
        value: zero,
    });
    let system = b.system_trying(vec![
        Algorithm::DogLeg,
        Algorithm::LevenbergMarquardt,
        Algorithm::Bfgs,
    ]);
    let solved = silico_sketch::solve(&system).unwrap();
    assert!(solved.redundant.is_empty());
    assert_eq!(solved.partially_redundant, vec![Tag(1)]);
    assert_eq!(solved.status, SolveStatus::SolvedExceptRedundant);
    assert_eq!(solved.algorithm, Some(Algorithm::DogLeg));
    assert!(solved.values[2].abs() <= 1e-10 && solved.values[3].abs() <= 1e-10);
    for (tag, r) in &solved.residuals {
        assert!(r.unwrap().abs() <= 1e-10, "tag {}: {r:?}", tag.0);
    }
}

#[test]
fn chained_coincidences_onto_a_half_fixed_point_solve_in_every_order() {
    // Found by the robustness review: a fixed point a, two free points q and r, and a point
    // b whose x is fixed at 0 and y free, chained a = q = r = b. One coincidence's x
    // equation repeats the others (partially redundant), in whatever order they come. Each
    // order solves to q = r = a and b's y = 0.
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let mut b = Builder::default();
        let a = b.fixed_point(0.0, 0.0);
        let qv = b.point_values(1.0, 2.0);
        let q = b.add(SketchGeometry::Point(qv));
        let rv = b.point_values(3.0, 5.0);
        let r = b.add(SketchGeometry::Point(rv));
        let bv = PointValues {
            x: b.fixed(0.0),
            y: b.unknown(9.0),
        };
        let bp = b.add(SketchGeometry::Point(bv));
        for i in order {
            let (x, y) = [(a, q), (q, r), (r, bp)][i];
            b.constrain(|tag| SolverConstraint::Coincident {
                tag,
                a: PointOf::Point(x),
                b: PointOf::Point(y),
            });
        }
        let system = b.system_trying(vec![Algorithm::DogLeg, Algorithm::LevenbergMarquardt]);
        system.check().unwrap();
        let solved = silico_sketch::solve(&system).unwrap();
        assert_eq!(
            solved.status,
            SolveStatus::SolvedExceptRedundant,
            "{order:?}: {solved:?}"
        );
        assert!(solved.redundant.is_empty(), "{order:?}: {solved:?}");
        assert_eq!(solved.partially_redundant.len(), 1, "{order:?}: {solved:?}");
        // Values: a (0, 1), q (2, 3), r (4, 5), b's x (6) and y (7).
        for v in [2, 3, 4, 5, 7] {
            assert!(solved.values[v].abs() <= 1e-10, "{order:?}: {solved:?}");
        }
        assert_eq!(solved.residuals.len(), 3, "{order:?}: {solved:?}");
        for (tag, residual) in &solved.residuals {
            assert!(
                residual.is_some_and(|r| r.abs() <= 1e-10),
                "{order:?}, {tag:?}: {solved:?}"
            );
        }
    }
}

#[test]
fn every_algorithm_failing_reports_the_last_and_gives_the_values_back() {
    let mut r = rectangle_40_by_20();
    let bottom = r.sides[0];
    let other = r.b.fixed(50.0);
    r.b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Start(bottom),
        b: PointOf::End(bottom),
        value: other,
    });
    let system = r.b.system_trying(vec![
        Algorithm::DogLeg,
        Algorithm::LevenbergMarquardt,
        Algorithm::Bfgs,
    ]);
    let solved = silico_sketch::solve(&system).unwrap();
    let mut bfgs = system.clone();
    bfgs.algorithms = vec![Algorithm::Bfgs];
    let last = silico_sketch::solve(&bfgs).unwrap();
    assert!(!matches!(
        last.status,
        SolveStatus::Success | SolveStatus::SolvedExceptRedundant
    ));
    assert_eq!(solved.status, last.status);
    assert_eq!(solved.algorithm, None);
    assert_eq!(solved.values, system.values);
}
