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
    // PlaneGCS finds free values from the constraints' Jacobian: with no constraints there
    // is none, and it lists nothing.
    assert!(solved.free.is_empty());
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
    // rectangle starts with its bottom off horizontal, so the solve is only `Converged`,
    // and its values stay as they were.
    assert_eq!(solved.status, SolveStatus::Converged);
    assert_eq!(solved.values, system.values);

    // Started where the constraints already hold, the dropped one holds too.
    let mut started_solved = system.clone();
    started_solved.values = silico_sketch::solve(&rectangle_40_by_20().b.system(Algorithm::DogLeg))
        .unwrap()
        .values;
    let solved = silico_sketch::solve(&started_solved).unwrap();
    assert_eq!(solved.redundant, vec![again]);
    assert_eq!(solved.status, SolveStatus::Success);
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
