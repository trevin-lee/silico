//! break-it, M3.1: hostile sketches for PlaneGCS through the bridge and the sketch worker.
//! Expected values are worked out by hand (lengths, positions, degrees of freedom) or are
//! the bridge's own promises checked independently: a constraint's residual recomputed
//! here from the solved values, `Solved`'s documented orderings, and the worker's limits.
//!
//! Each "Found by break-it" note says what the test found before the code was fixed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/generate.rs"]
mod generate;
#[path = "common/systems.rs"]
mod systems;

use std::path::Path;
use std::time::{Duration, Instant};

use generate::{generate, residuals};
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::worker::{Worker, WorkerKind};
use silico_protocol::{
    Algorithm, DebugOp, Generation, Geometry, Op, PointOf, PointValues, Refusal, Reply,
    SketchGeometry, SketchSystem, SolveStatus, Solved, SolverConstraint, Tag, Value,
};
use systems::{Builder, Rng, process_exists, rectangle_40_by_20};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-test-worker");

fn solve(system: &SketchSystem) -> Solved {
    system.check().unwrap();
    silico_sketch::solve(system).unwrap()
}

fn strictly_increasing<T: Ord + std::fmt::Debug>(v: &[T]) -> bool {
    v.windows(2).all(|w| w[0] < w[1])
}

// ---------------------------------------------------------------------------------------
// Tags the solver can't hold.

/// The two disagreeing widths of `two_widths_that_disagree_conflict_and_both_are_named`
/// (solve.rs), the second tagged `tag`: 40 and 50 can't both hold, so the diagnosis names
/// the first width (tag 10) and the second, and 16 unknowns less 17 equations leave -1.
fn rectangle_with_a_second_width_tagged(tag: u32) -> SketchSystem {
    let mut r = rectangle_40_by_20();
    let bottom = r.sides[0];
    let other = r.b.fixed(50.0);
    r.b.constraints.push(SolverConstraint::DistanceX {
        tag: Tag(tag),
        a: PointOf::Start(bottom),
        b: PointOf::End(bottom),
        value: other,
    });
    r.b.system(Algorithm::DogLeg)
}

#[test]
fn a_conflict_tagged_past_i32_max_is_named_or_refused() {
    // The control: a small tag is named, as solve.rs shows.
    let solved = solve(&rectangle_with_a_second_width_tagged(12));
    assert_eq!(solved.conflicting, vec![Tag(10), Tag(12)]);
    assert_eq!(solved.dofs, -1);

    // Found by break-it, now refused by the check: the check took any tag from 1, but the
    // bridge hands PlaneGCS an `int`, so
    // 2^31 and u32::MAX arrive negative; PlaneGCS leaves negatively tagged constraints out
    // of the diagnosis (GCS.cpp, addConstraint and diagnose: "negatively tagged constraints
    // have no impact"). The conflict goes unreported and the solve says Success with the
    // width at 40, not 50: dofs 0, conflicting [], status Success.
    for big in [0x8000_0000u32, u32::MAX] {
        let system = rectangle_with_a_second_width_tagged(big);
        match system.check() {
            Err(e) => {
                assert_eq!(e.parameter, "constraints");
                assert!(e.message.contains(&big.to_string()), "{}", e.message);
            }
            Ok(()) => {
                let solved = silico_sketch::solve(&system).unwrap();
                assert_eq!(
                    solved.conflicting,
                    vec![Tag(10), Tag(big)],
                    "tag {big}: {solved:?}"
                );
                assert_eq!(solved.dofs, -1, "tag {big}");
                assert_ne!(solved.status, SolveStatus::Success, "tag {big}");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Constraints on values the solver may not change.

/// Two fixed points 3 apart, and a distance of 5 between them: it can't hold.
fn fixed_points_3_apart_dimensioned_5(b: &mut Builder) {
    let p = b.fixed_point(0.0, 0.0);
    let q = b.fixed_point(3.0, 0.0);
    let five = b.fixed(5.0);
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Point(p),
        b: PointOf::Point(q),
        value: five,
    });
}

#[test]
fn a_violated_distance_between_fixed_points_alone_is_reported_by_its_residual() {
    // Found by break-it: with no unknowns at all, PlaneGCS's diagnosis stops at an empty
    // parameter list (dofs 0, nothing named) and its solve returns Success, so nothing said
    // |pq| = 3 against a distance of 5. PlaneGCS solves only what has unknowns; the
    // constraint's residual, |pq| − 5 = −2, now says it doesn't hold.
    let mut b = Builder::default();
    fixed_points_3_apart_dimensioned_5(&mut b);
    let solved = solve(&b.system(Algorithm::DogLeg));
    assert_eq!(solved.residuals, vec![(Tag(1), Some(-2.0))], "{solved:?}");
    assert_eq!(solved.dofs, 0);
}

#[test]
fn a_violated_distance_between_fixed_points_beside_a_solved_sketch_is_named_and_reported() {
    // Found by break-it: the diagnosis names the distance (tag 12) as conflicting, and the
    // status is Success, PlaneGCS's for the parts it solved (only those with unknowns). The
    // distance's residual says it doesn't hold; every other constraint's is 0 to the
    // solver's convergence.
    let mut r = rectangle_40_by_20();
    fixed_points_3_apart_dimensioned_5(&mut r.b);
    let solved = solve(&r.b.system(Algorithm::DogLeg));
    assert_eq!(solved.conflicting, vec![Tag(12)]);
    assert_eq!(solved.status, SolveStatus::Success);
    let (last, rest) = solved.residuals.split_last().unwrap();
    assert_eq!(*last, (Tag(12), Some(-2.0)));
    assert_eq!(rest.len(), 11);
    for (tag, r) in rest {
        assert!(r.unwrap().abs() <= 1e-10, "tag {}: {r:?}", tag.0);
    }
}

#[test]
fn a_constraint_of_two_equations_is_reported_by_their_root_mean_square() {
    // A coincidence is two equations, x and y: between fixed points at (0, 0) and (3, 4)
    // their errors are −3 and −4, and the residual is √((9 + 16) / 2) = √12.5.
    let mut b = Builder::default();
    let p = b.fixed_point(0.0, 0.0);
    let q = b.fixed_point(3.0, 4.0);
    b.constrain(|tag| SolverConstraint::Coincident {
        tag,
        a: PointOf::Point(p),
        b: PointOf::Point(q),
    });
    let solved = solve(&b.system(Algorithm::DogLeg));
    assert_eq!(solved.residuals, vec![(Tag(1), Some(12.5_f64.sqrt()))]);
}

#[test]
fn a_residual_that_isnt_a_number_is_reported_as_none() {
    // Found by break-it (seed 0xca6ae0f8e83e36dc): a line whose ends are made coincident
    // collapses, and a point on it then measures 0/0. Its residual is reported as none, not
    // NaN, so the core can't mistake it for a number.
    // That seed's system: a line from (−2, 0) to (23.44, 8.46), its start's x fixed, made
    // perpendicular to itself and its ends coincident, so it collapses to a point; a point
    // on it (its own start, twice) then measures 0/0, and the rest measure 0.
    let line = Geometry(0);
    let system = SketchSystem {
        values: vec![-2.0, 0.0, 23.440289617918054, 8.464919169626306],
        unknowns: vec![Value(1), Value(2), Value(3)],
        geometry: vec![SketchGeometry::Line {
            start: PointValues {
                x: Value(0),
                y: Value(1),
            },
            end: PointValues {
                x: Value(2),
                y: Value(3),
            },
        }],
        constraints: vec![
            SolverConstraint::PointOnLine {
                tag: Tag(1),
                point: PointOf::Start(line),
                line,
            },
            SolverConstraint::Perpendicular {
                tag: Tag(2),
                a: line,
                b: line,
            },
            SolverConstraint::PointOnLine {
                tag: Tag(3),
                point: PointOf::Start(line),
                line,
            },
            SolverConstraint::EqualLength {
                tag: Tag(4),
                a: line,
                b: line,
            },
            SolverConstraint::Parallel {
                tag: Tag(5),
                a: line,
                b: line,
            },
            SolverConstraint::Coincident {
                tag: Tag(6),
                a: PointOf::Start(line),
                b: PointOf::End(line),
            },
        ],
        algorithms: vec![Algorithm::DogLeg],
    };
    let solved = solve(&system);
    // Equations repeat (the point on the line, stated twice, among them), so some are
    // dropped. DogLeg solves what's left, but on a line of no length, which FreeCAD can't
    // build and undoes (`fc128d357d`): solved invalid, with that solution, so its residuals
    // are read on it.
    assert!(!solved.redundant.is_empty(), "{solved:?}");
    assert_eq!(solved.status, SolveStatus::SolvedInvalid);
    assert_eq!(solved.algorithm, Some(Algorithm::DogLeg));
    let v = &solved.values;
    assert_eq!((v[0], v[1]), (v[2], v[3]), "the line collapsed");
    assert_eq!(
        solved.residuals,
        vec![
            (Tag(1), None),
            (Tag(2), Some(0.0)),
            (Tag(3), None),
            (Tag(4), Some(0.0)),
            (Tag(5), Some(0.0)),
            (Tag(6), Some(0.0)),
        ]
    );
}

// ---------------------------------------------------------------------------------------
// What `Solved` promises about its lists.

#[test]
fn free_values_are_each_listed_once() {
    // Found by break-it, now fixed: PlaneGCS lists a dependent parameter once per group it's
    // in; the bridge
    // sorts the list but keeps the repeats. A free line dimensioned to length 0 comes back
    // with free [0, 1, 1, 1, 2, 3]. `Solved::free` is "in increasing order", as unknowns are.
    let mut b = Builder::default();
    let l = b.line((1.0, 1.0), (4.0, 5.0));
    let zero = b.fixed(0.0);
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Start(l),
        b: PointOf::End(l),
        value: zero,
    });
    let solved = solve(&b.system(Algorithm::DogLeg));
    assert!(strictly_increasing(&solved.free), "{:?}", solved.free);
}

#[test]
fn a_line_with_no_constraints_is_free_as_it_is_beside_a_constrained_sketch() {
    // Beside a fully constrained rectangle, an unconstrained line's four values are free.
    let mut r = rectangle_40_by_20();
    r.b.line((100.0, 100.0), (110.0, 105.0));
    let solved = solve(&r.b.system(Algorithm::DogLeg));
    assert_eq!(solved.dofs, 4);
    assert_eq!(solved.free, (20..24).map(Value).collect::<Vec<_>>());

    // Found by break-it: alone, with no constraint in the sketch, the same line was listed
    // as fixed, with dofs 4: PlaneGCS's diagnosis has no equations and lists no dependent
    // value. FreeCAD reads that as "everything free" (Sketch::calculateDependentParameters-
    // Elements, `isEmptyDiagnoseMatrix()`); the bridge now says so, and the core decides.
    let mut b = Builder::default();
    b.line((100.0, 100.0), (110.0, 105.0));
    let solved = solve(&b.system(Algorithm::DogLeg));
    assert_eq!(solved.dofs, 4);
    assert!(solved.free.is_empty());
    assert!(solved.no_equations);
    // And a sketch with equations says it has them.
    let mut r = rectangle_40_by_20();
    r.b.line((100.0, 100.0), (110.0, 105.0));
    assert!(!solve(&r.b.system(Algorithm::DogLeg)).no_equations);
}

// ---------------------------------------------------------------------------------------
// Geometry drawn with no size: gradients of 0/0.

/// A system whose only constraint holds for some placement of its free values, with the
/// unknowns and independent equations it has: the diagnosis must not call it conflicting.
fn assert_satisfiable_diagnosis(name: &str, system: &SketchSystem, unknowns: i32, rows: i32) {
    system.check().unwrap();
    match silico_sketch::solve(system) {
        Ok(solved) => {
            assert!(
                solved.conflicting.is_empty() && solved.redundant.is_empty(),
                "{name}: a satisfiable constraint diagnosed as {solved:?}"
            );
            assert_eq!(solved.dofs, unknowns - rows, "{name}: {solved:?}");
        }
        // A refusal is acceptable only if it says what's wrong, as data.
        Err(failure) => panic!("{name}: refused as {failure:?}"),
    }
}

#[test]
fn a_line_drawn_with_no_length_is_refused_by_the_check() {
    // Found by break-it: each constraint's gradient divides by a length that starts at 0 (P2PDistance,
    // PointOnLine, L2LAngle), or its scale does (Parallel: 1/(|l1||l2|) at construction), so
    // the Jacobian row is NaN. The bridge returns Ok with the constraint named conflicting,
    // dofs as if it weren't there, and status Failed: e.g. a length of 5 on a free line drawn
    // from (1, 1) to (1, 1) gives dofs 4, conflicting [1]. Each holds for some placement
    // (a line of length 5; a point on a line through it; a line parallel to another, or at an
    // angle of 1 to it), so the diagnosis is wrong; FreeCAD can't build the input (a
    // Part.LineSegment with equal ends throws), so it never meets this.
    let mut b = Builder::default();
    let l = b.line((1.0, 1.0), (1.0, 1.0));
    let five = b.fixed(5.0);
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Start(l),
        b: PointOf::End(l),
        value: five,
    });
    let distance = b.system(Algorithm::DogLeg);

    let mut b = Builder::default();
    let l = b.line((0.0, 0.0), (0.0, 0.0));
    let p = b.point_values(3.0, 4.0);
    let pt = b.add(SketchGeometry::Point(p));
    b.constrain(|tag| SolverConstraint::PointOnLine {
        tag,
        point: PointOf::Point(pt),
        line: l,
    });
    let on_line = b.system(Algorithm::DogLeg);

    let mut b = Builder::default();
    let l = b.line((0.0, 0.0), (0.0, 0.0));
    let m = b.line((0.0, 1.0), (5.0, 2.0));
    b.constrain(|tag| SolverConstraint::Parallel { tag, a: l, b: m });
    let parallel = b.system(Algorithm::DogLeg);

    let mut b = Builder::default();
    let l = b.line((0.0, 0.0), (0.0, 0.0));
    let m = b.line((0.0, 1.0), (5.0, 2.0));
    let one = b.fixed(1.0);
    b.constrain(|tag| SolverConstraint::Angle {
        tag,
        a: l,
        b: m,
        value: one,
    });
    let angle = b.system(Algorithm::DogLeg);

    // The check now refuses a line drawn with no length, naming it, so the solver never sees
    // one: FreeCAD can't draw one either.
    for (name, system, line) in [
        ("distance", &distance, 0),
        ("point on line", &on_line, 0),
        ("parallel", &parallel, 0),
        ("angle", &angle, 0),
    ] {
        let e = system.check().unwrap_err();
        assert_eq!(e.parameter, "geometry", "{name}");
        assert!(
            e.message
                .starts_with(&format!("geometry {line} is a line drawn with no length")),
            "{name}: {}",
            e.message
        );
    }
    // Drawn with any length at all, the same constraints are diagnosed as satisfiable.
    let mut b = Builder::default();
    let l = b.line((1.0, 1.0), (1.0, 1.0 + 1e-9));
    let five = b.fixed(5.0);
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Start(l),
        b: PointOf::End(l),
        value: five,
    });
    assert_satisfiable_diagnosis(
        "distance, drawn 1e-9 long",
        &b.system(Algorithm::DogLeg),
        4,
        1,
    );
}

#[test]
fn a_distance_of_zero_brings_the_points_together() {
    let mut b = Builder::default();
    let l = b.line((1.0, 1.0), (4.0, 5.0));
    let zero = b.fixed(0.0);
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Start(l),
        b: PointOf::End(l),
        value: zero,
    });
    let system = b.system(Algorithm::DogLeg);
    let solved = solve(&system);
    assert_eq!(solved.status, SolveStatus::Success);
    // Four unknowns, one equation.
    assert_eq!(solved.dofs, 3);
    let [sx, sy, ex, ey] = systems::line_values(&system, &solved.values, l);
    assert!(libm::hypot(sx - ex, sy - ey) <= 1e-9, "{:?}", solved.values);
}

#[test]
fn a_distance_from_a_point_to_itself_conflicts() {
    // 0 = 5 can't hold, and its gradient is 0: rank 0, so dofs 4 and the distance conflicts.
    let mut b = Builder::default();
    let l = b.line((1.0, 1.0), (4.0, 5.0));
    let five = b.fixed(5.0);
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Start(l),
        b: PointOf::Start(l),
        value: five,
    });
    let system = b.system(Algorithm::DogLeg);
    let solved = solve(&system);
    assert_eq!(solved.conflicting, vec![Tag(1)]);
    assert_eq!(solved.dofs, 4);
    assert_ne!(solved.status, SolveStatus::Success);
    assert_eq!(solved.values, system.values);
}

#[test]
fn a_line_parallel_to_itself_is_redundant() {
    let mut b = Builder::default();
    let l = b.line((0.0, 0.0), (3.0, 4.0));
    b.constrain(|tag| SolverConstraint::Parallel { tag, a: l, b: l });
    let solved = solve(&b.system(Algorithm::DogLeg));
    assert_eq!(solved.redundant, vec![Tag(1)]);
    assert_eq!(solved.dofs, 4);
    assert_eq!(solved.status, SolveStatus::SolvedExceptRedundant);
    assert_eq!(solved.values, vec![0.0, 0.0, 3.0, 4.0]);
}

#[test]
fn a_negative_distance_never_succeeds() {
    // |pq| = -5 has no solution.
    for algorithm in [
        Algorithm::DogLeg,
        Algorithm::LevenbergMarquardt,
        Algorithm::Bfgs,
    ] {
        let mut b = Builder::default();
        let l = b.line((1.0, 1.0), (4.0, 5.0));
        let v = b.fixed(-5.0);
        b.constrain(|tag| SolverConstraint::Distance {
            tag,
            a: PointOf::Start(l),
            b: PointOf::End(l),
            value: v,
        });
        let system = b.system(algorithm);
        let solved = solve(&system);
        assert_ne!(solved.status, SolveStatus::Success, "{algorithm:?}");
        assert_eq!(solved.values, system.values, "{algorithm:?}");
    }
}

#[test]
fn an_arc_drawn_with_no_radius_takes_its_radius_and_angles() {
    // Centre fixed at the origin, from 0 to 90°, drawn with radius 0 and both ends at the
    // centre: a radius of 5 puts its start at (5, 0) and its end at (0, 5).
    let mut b = Builder::default();
    let center = PointValues {
        x: b.fixed(0.0),
        y: b.fixed(0.0),
    };
    let radius = b.unknown(0.0);
    let start_angle = b.fixed(0.0);
    let end_angle = b.fixed(std::f64::consts::FRAC_PI_2);
    let start = b.point_values(0.0, 0.0);
    let end = b.point_values(0.0, 0.0);
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
    let solved = solve(&b.system(Algorithm::DogLeg));
    assert_eq!(solved.status, SolveStatus::Success);
    assert_eq!(solved.dofs, 0);
    let got = [start.x, start.y, end.x, end.y].map(|v| solved.values[v.0 as usize]);
    for (g, w) in got.iter().zip([5.0, 0.0, 0.0, 5.0]) {
        assert!((g - w).abs() <= 1e-9, "{got:?}");
    }
}

// ---------------------------------------------------------------------------------------
// Errors.

#[test]
fn a_check_refusal_says_which_constraint() {
    // Found by break-it: among many constraints, the refusal named the geometry but not the
    // constraint (by tag or place): "a constraint names geometry 7, but there are 1".
    let mut b = Builder::default();
    let line = b.line((0.0, 0.0), (1.0, 0.0));
    for _ in 0..20 {
        b.constrain(|tag| SolverConstraint::Horizontal { tag, line });
    }
    b.constrain(|tag| SolverConstraint::Parallel {
        tag,
        a: line,
        b: Geometry(7),
    });
    let e = b.system(Algorithm::DogLeg).check().unwrap_err();
    assert_eq!(e.parameter, "constraints");
    assert!(
        e.message.contains("21") || e.message.contains("20"),
        "doesn't say which constraint: {}",
        e.message
    );
}

// ---------------------------------------------------------------------------------------
// Size: what the check admits must be answered.

/// `n` free lines, each parallel to the next, drawn off parallel: 4n unknowns in one
/// component, n - 1 equations.
fn parallel_chain(n: usize, algorithm: Algorithm) -> SketchSystem {
    let mut b = Builder::default();
    let lines: Vec<Geometry> = (0..n)
        .map(|i| b.line((i as f64, 0.0), (i as f64 + 10.0, 1.0 + (i % 3) as f64)))
        .collect();
    for w in lines.windows(2) {
        let (a, c) = (w[0], w[1]);
        b.constrain(|tag| SolverConstraint::Parallel { tag, a, b: c });
    }
    b.system(algorithm)
}

#[test]
fn the_largest_system_the_check_admits_is_answered() {
    // Found by break-it: 2,000 lines (8,000 unknowns) chained by 1,999 parallels passed the
    // check, half its Jacobian limit, and ran for many minutes: Levenberg–Marquardt forms
    // JᵀJ, unknowns × unknowns, and factors it in O(unknowns³) (measured in release: 500
    // lines 13 s, 1,000 lines about 100 s). The check now refuses more than
    // MAX_SKETCH_UNKNOWNS, and the most it admits is answered: 512 free lines, each
    // parallel to the next, are 2,048 unknowns less 511 independent equations.
    assert!(
        parallel_chain(2000, Algorithm::LevenbergMarquardt)
            .check()
            .is_err()
    );
    let lines = silico_protocol::MAX_SKETCH_UNKNOWNS / 4;
    let system = parallel_chain(lines, Algorithm::LevenbergMarquardt);
    system.check().unwrap();
    let mut w = Worker::spawn_as(Path::new(WORKER), WorkerKind::Sketch).unwrap();
    match w.call(Generation(1), Op::Solve(system), Duration::from_secs(60)) {
        Ok(Reply::Solved(solved)) => {
            assert_eq!(solved.status, SolveStatus::Success);
            assert_eq!(solved.dofs, 2048 - 511);
            assert!(solved.conflicting.is_empty() && solved.redundant.is_empty());
        }
        other => panic!("the largest admitted system wasn't solved: {other:?}"),
    }
}

#[test]
fn a_heavily_conflicting_system_at_the_limits_is_answered_or_timed_out_and_reaped() {
    // Found by the robustness review: thousands of dimensions that conflict make
    // PlaneGCS's conflict analysis run for minutes (2,048 unknowns and 4,000 distances:
    // over 11 minutes). The bound on a solve is the request's timeout: the worker is
    // killed and reaped when it passes, and the error says so.
    let mut rng = Rng(0x00c0_ffee);
    let mut b = Builder::default();
    let lines: Vec<Geometry> = (0..silico_protocol::MAX_SKETCH_UNKNOWNS / 4)
        .map(|_| {
            let (x, y) = (rng.range(-100.0, 100.0), rng.range(-100.0, 100.0));
            b.line((x, y), (x + rng.range(1.0, 10.0), y + rng.range(1.0, 10.0)))
        })
        .collect();
    for _ in 0..4000 {
        let a = lines[rng.below(lines.len() as u64) as usize];
        let c = lines[rng.below(lines.len() as u64) as usize];
        let value = b.fixed(rng.range(1.0, 50.0));
        b.constrain(|tag| SolverConstraint::Distance {
            tag,
            a: PointOf::Start(a),
            b: PointOf::End(c),
            value,
        });
    }
    let system = b.system(Algorithm::DogLeg);
    system.check().unwrap();
    let mut w = Worker::spawn_as(Path::new(WORKER), WorkerKind::Sketch).unwrap();
    let pid = w.pid();
    let start = Instant::now();
    match w.call(Generation(1), Op::Solve(system), Duration::from_secs(5)) {
        Ok(Reply::Solved(solved)) => {
            assert!(!solved.conflicting.is_empty(), "{solved:?}");
            assert_ne!(solved.status, SolveStatus::Success);
        }
        Ok(other) => panic!("replied {other:?}"),
        Err(e) => {
            assert_eq!(e.code(), ErrorCode::WorkerTimeout, "{e}");
            assert!(w.is_stopped());
            assert!(!process_exists(pid), "worker {pid} still exists");
        }
    }
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn a_checked_system_has_a_bounded_amount_of_geometry() {
    // Found by break-it: the check bounded values (10,000), constraints (10,000), and the
    // Jacobian, but not geometry, which may share values: a million points on values 0 and 1
    // passed.
    // Each costs 3 bytes on the wire and about 260 in the solve (the bridge's flattened
    // encoding and its `Geo`, which holds every kind of PlaneGCS curve): measured, 5 million
    // points (a 15 MB frame) peak at 1.29 GB resident in one solve, so a frame at the
    // 512 MiB limit (about 170 million points) asks the worker for tens of gigabytes.
    let points = 1_000_000;
    let system = SketchSystem {
        values: vec![1.0, 2.0],
        unknowns: vec![Value(0), Value(1)],
        geometry: vec![
            SketchGeometry::Point(PointValues {
                x: Value(0),
                y: Value(1),
            });
            points
        ],
        constraints: Vec::new(),
        algorithms: vec![Algorithm::DogLeg],
    };
    let e = system.check().unwrap_err();
    assert_eq!(e.parameter, "geometry", "{}", e.message);
}

#[test]
fn a_solution_is_one_the_next_solve_accepts() {
    // A fixed point at x = 1e6 (within reach), and a free point 1e6 to its right: the
    // solution puts it at x = 2e6, past the check's reach (1e6).
    // Found by break-it: the worker returned it as a Success, and the next solve, started
    // from it (a drag, an edit), was refused: "value 2 must be finite and within 1e6 of 0,
    // got 2000000". The worker refused such a solution itself, as out of reach; since M3.3
    // it's a solution that can't be built, passed over for the next algorithm and given as
    // solved invalid, with its values, so the core can name the point that went there.
    let mut b = Builder::default();
    let p = b.fixed_point(1e6, 0.0);
    let q = b.point_values(1e6 - 1.0, 0.5);
    let q = b.add(SketchGeometry::Point(q));
    let dx = b.fixed(1e6);
    let dy = b.fixed(0.0);
    b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Point(p),
        b: PointOf::Point(q),
        value: dx,
    });
    b.constrain(|tag| SolverConstraint::DistanceY {
        tag,
        a: PointOf::Point(p),
        b: PointOf::Point(q),
        value: dy,
    });
    let system = b.system(Algorithm::DogLeg);
    system.check().unwrap();
    let solved = silico_sketch::solve(&system).unwrap();
    if solved.status == SolveStatus::Success {
        let mut next = system.clone();
        next.values = solved.values;
        if let Err(e) = next.check() {
            panic!("solved, then refused: {}", e.message);
        }
    } else {
        assert_eq!(solved.status, SolveStatus::SolvedInvalid);
        assert_eq!(solved.values[2], 2e6, "{solved:?}");
    }
}

#[test]
fn a_sketch_workers_errors_dont_call_it_the_kernel_worker() {
    // Found by break-it: the core's worker client worded every failure for the kernel worker:
    // a sketch worker that crashed was "the kernel worker crashed during DebugSegfault". A
    // worker started as a sketch worker is now named as one.
    let mut w = Worker::spawn_as(Path::new(TEST_WORKER), WorkerKind::Sketch).unwrap();
    let err = w
        .call(
            Generation(1),
            Op::Debug(DebugOp::Segfault),
            Duration::from_secs(30),
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(!err.message().contains("kernel"), "{err}");
    assert_eq!(
        err.message(),
        "the sketch worker crashed during DebugSegfault"
    );
}

// ---------------------------------------------------------------------------------------
// Many threads at once, in one process.

#[test]
fn solves_on_many_threads_at_once_agree_bit_for_bit() {
    let system = rectangle_40_by_20().b.system(Algorithm::LevenbergMarquardt);
    let alone = solve(&system);
    let bits = |s: &Solved| s.values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    (0..20)
                        .map(|_| silico_sketch::solve(&system).unwrap())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            for s in h.join().unwrap() {
                assert_eq!(bits(&s), bits(&alone));
                assert_eq!(s, alone);
            }
        }
    });
}

// ---------------------------------------------------------------------------------------
// Generated sketches, biased to the degenerate: coincident points, zero lengths and radii,
// zero and negative dimensions, constraints on fixed values, and now and then a reference
// the check must refuse. Whatever passes the check, the bridge must build (no exception),
// and what it returns must keep `Solved`'s promises, checked here from the values alone.

/// The values `c` reads: its geometry's and its dimension's.
fn constraint_values(s: &SketchSystem, c: &SolverConstraint, out: &mut Vec<Value>) {
    let geometry = |g: Geometry, out: &mut Vec<Value>| {
        out.extend(match s.geometry[g.0 as usize] {
            SketchGeometry::Point(p) => vec![p.x, p.y],
            SketchGeometry::Line { start, end } => vec![start.x, start.y, end.x, end.y],
            SketchGeometry::Circle { center, radius } => vec![center.x, center.y, radius],
            SketchGeometry::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                start,
                end,
            } => vec![
                center.x,
                center.y,
                radius,
                start_angle,
                end_angle,
                start.x,
                start.y,
                end.x,
                end.y,
            ],
        });
    };
    // A point's own two values, as PlaneGCS's constraint holds them.
    let point = |p: PointOf, out: &mut Vec<Value>| {
        let pv = match (
            p,
            s.geometry[match p {
                PointOf::Point(g) | PointOf::Start(g) | PointOf::End(g) | PointOf::Center(g) => g,
            }
            .0 as usize],
        ) {
            (PointOf::Point(_), SketchGeometry::Point(pv)) => pv,
            (PointOf::Start(_), SketchGeometry::Line { start, .. })
            | (PointOf::Start(_), SketchGeometry::Arc { start, .. }) => start,
            (PointOf::End(_), SketchGeometry::Line { end, .. })
            | (PointOf::End(_), SketchGeometry::Arc { end, .. }) => end,
            (PointOf::Center(_), SketchGeometry::Circle { center, .. })
            | (PointOf::Center(_), SketchGeometry::Arc { center, .. }) => center,
            (p, g) => panic!("{p:?} of {g:?}"),
        };
        out.extend([pv.x, pv.y]);
    };
    match *c {
        SolverConstraint::ArcRules { arc } => geometry(arc, out),
        SolverConstraint::Coincident { a, b, .. } => {
            point(a, out);
            point(b, out);
        }
        SolverConstraint::Horizontal { line, .. } | SolverConstraint::Vertical { line, .. } => {
            geometry(line, out)
        }
        SolverConstraint::DistanceX { a, b, value, .. }
        | SolverConstraint::DistanceY { a, b, value, .. }
        | SolverConstraint::Distance { a, b, value, .. } => {
            point(a, out);
            point(b, out);
            out.push(value);
        }
        SolverConstraint::PointOnLine { point: p, line, .. } => {
            point(p, out);
            geometry(line, out);
        }
        SolverConstraint::Parallel { a, b, .. }
        | SolverConstraint::Perpendicular { a, b, .. }
        | SolverConstraint::EqualLength { a, b, .. } => {
            geometry(a, out);
            geometry(b, out);
        }
        SolverConstraint::Angle { a, b, value, .. } => {
            geometry(a, out);
            geometry(b, out);
            out.push(value);
        }
        SolverConstraint::Radius { circle, value, .. } => {
            out.push(match s.geometry[circle.0 as usize] {
                SketchGeometry::Circle { radius, .. } | SketchGeometry::Arc { radius, .. } => {
                    radius
                }
                ref g => panic!("radius of {g:?}"),
            });
            out.push(value);
        }
        // Not generated here: M3.3's forms have their own tests.
        other => panic!("not a form this test generates: {other:?}"),
    }
}

/// Whether geometry at `values` can be built, written out here: every value is finite and
/// within a kilometre, every line has length (dx² + dy² isn't 0), every circle and arc a
/// radius above 0, and every arc its ends at two places.
fn can_build(system: &SketchSystem, values: &[f64]) -> bool {
    if values.iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
        return false;
    }
    let apart = |a: PointValues, b: PointValues| {
        let dx = values[b.x.0 as usize] - values[a.x.0 as usize];
        let dy = values[b.y.0 as usize] - values[a.y.0 as usize];
        dx * dx + dy * dy != 0.0
    };
    system.geometry.iter().all(|g| match *g {
        SketchGeometry::Point(_) => true,
        SketchGeometry::Line { start, end } => apart(start, end),
        SketchGeometry::Circle { radius, .. } => values[radius.0 as usize] > 0.0,
        SketchGeometry::Arc {
            radius, start, end, ..
        } => values[radius.0 as usize] > 0.0 && apart(start, end),
    })
}

/// Every promise `Solved` makes about `system`, checked independently of the solver.
fn check_promises(system: &SketchSystem) -> Result<(), String> {
    if system.check().is_err() {
        return Ok(());
    }
    let solved = match silico_sketch::solve(system) {
        Ok(solved) => solved,
        // A solution that isn't finite is refused as data; anything else means the bridge
        // couldn't read what the check passed.
        Err(f) if matches!(f.refusal, Refusal::SolutionOutOfReach { .. }) => return Ok(()),
        Err(f) => return Err(format!("the bridge refused a checked system: {f:?}")),
    };
    if solved.values.len() != system.values.len() {
        return Err("values of another length".into());
    }
    let unknown = |i: usize| system.unknowns.binary_search(&Value(i as u32)).is_ok();
    for (i, (a, b)) in system.values.iter().zip(&solved.values).enumerate() {
        if !unknown(i) && a.to_bits() != b.to_bits() {
            return Err(format!("fixed value {i} moved from {a} to {b}"));
        }
    }
    let tags: Vec<Tag> = system.constraints.iter().filter_map(|c| c.tag()).collect();
    for (name, list) in [
        ("conflicting", &solved.conflicting),
        ("redundant", &solved.redundant),
        ("partially redundant", &solved.partially_redundant),
    ] {
        if !strictly_increasing(list) || list.iter().any(|t| !tags.contains(t)) {
            return Err(format!("{name} {list:?} with tags {tags:?}"));
        }
    }
    if !strictly_increasing(&solved.free) || solved.free.iter().any(|&v| !unknown(v.0 as usize)) {
        return Err(format!("free {:?}", solved.free));
    }
    if solved.dofs > system.unknowns.len() as i32 {
        return Err(format!(
            "dofs {} > {} unknowns",
            solved.dofs,
            system.unknowns.len()
        ));
    }
    // Every tagged constraint has a residual, by tag; one that isn't a number is reported as
    // none, and a reported one agrees with this test's own measure where the constraint is
    // one equation (several are reported as their root mean square).
    let reported: Vec<Tag> = solved.residuals.iter().map(|(t, _)| *t).collect();
    let mut sorted_tags = tags.clone();
    sorted_tags.sort_unstable();
    if reported != sorted_tags {
        return Err(format!("residuals for {reported:?}, tags {sorted_tags:?}"));
    }
    let residual_of = |t: Tag| {
        solved
            .residuals
            .iter()
            .find(|(r, _)| *r == t)
            .and_then(|(_, r)| *r)
    };
    for c in &system.constraints {
        let mine = residuals(system, &solved.values, c);
        let (Some(t), &[mine]) = (c.tag(), mine.as_slice()) else {
            continue;
        };
        match residual_of(t) {
            None if mine.is_finite() => {
                return Err(format!("{c:?}'s residual {mine:e} reported as none"));
            }
            Some(r) if (r.abs() - mine.abs()).abs() > 1e-9 * (1.0 + mine.abs()) => {
                return Err(format!(
                    "{c:?}'s residual reported {r:e}, measured {mine:e}"
                ));
            }
            _ => {}
        }
    }
    let solved_it = matches!(
        solved.status,
        SolveStatus::Success | SolveStatus::SolvedExceptRedundant
    );
    let invalid = solved.status == SolveStatus::SolvedInvalid;
    if (solved_it || invalid) != solved.algorithm.is_some() {
        return Err(format!(
            "{:?} with algorithm {:?}",
            solved.status, solved.algorithm
        ));
    }
    // A solution stands only on geometry that can be built; one that can't is given as
    // solved invalid, with that solution (M3.3, FreeCAD's `fc128d357d`).
    if (solved_it || invalid) && can_build(system, &solved.values) != solved_it {
        return Err(format!(
            "{:?} on geometry that {} be built",
            solved.status,
            if solved_it { "can't" } else { "can" }
        ));
    }
    if invalid {
        return Ok(());
    }
    if !solved_it {
        if solved.values != system.values {
            return Err(format!("{:?}, yet the values moved", solved.status));
        }
        return Ok(());
    }
    // A status means something only without conflicts (SolveStatus): PlaneGCS can say
    // Success for a conflicting sketch with its conflicting constraints unmet (seed
    // 0x5b1f6d2f2cde3691), and FreeCAD never solves one.
    if !solved.conflicting.is_empty() {
        return Ok(());
    }
    // Solved: every constraint the solver solved (one with an unknown) is met, to its
    // convergence (1e-10 on each residual; a redundant constraint is checked to 1e-5, err² ≤
    // 1e-10), or its residual is reported as none (geometry the solve left of no size). A
    // constraint among fixed values alone isn't solved: its residual says whether it holds.
    let touches_an_unknown = |c: &SolverConstraint| {
        let mut b = Vec::new();
        constraint_values(system, c, &mut b);
        b.iter().any(|v| system.unknowns.binary_search(v).is_ok())
    };
    for c in system.constraints.iter().filter(|c| touches_an_unknown(c)) {
        let loose = c.tag().is_some_and(|t| {
            solved.redundant.contains(&t) || solved.partially_redundant.contains(&t)
        });
        if c.tag().is_some_and(|t| residual_of(t).is_none()) {
            continue;
        }
        // Solved except the dropped equations: those are the core's to judge.
        if loose && solved.status == SolveStatus::SolvedExceptRedundant {
            continue;
        }
        let tolerance = if loose { 1e-4 } else { 1e-8 };
        for r in residuals(system, &solved.values, c) {
            if r.is_nan() || r.abs() > tolerance {
                return Err(format!("solved, but {c:?} is off by {r:e}: {solved:?}"));
            }
        }
    }
    // The same input gives the same bits.
    let again = silico_sketch::solve(system).map_err(|f| format!("again: {f:?}"))?;
    let bits = |s: &Solved| s.values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    if again != solved || bits(&again) != bits(&solved) {
        return Err("solved twice, two answers".into());
    }
    Ok(())
}

// Found by break-it: the first promise to fail was `free`'s order (a value listed more than
// once, as in free_values_are_each_listed_once); past it, Success with a constraint on fixed
// values unmet (the fixed-points tests above), and Success with a constraint left undefined
// on a line the solve collapsed (seed 0xca6ae0f8e83e36dc: a line perpendicular to itself, its
// ends coincident, its start on it; the point-on-line residual is 0/0, and PlaneGCS's check
// of redundant constraints after the solve, `err * err > convergence`, is false for NaN).
// Free values are now listed once, and every constraint's residual is reported, none where
// it isn't a number, so the core sees each of these.
proptest! {
    #![proptest_config(ProptestConfig { cases: 4000, ..ProptestConfig::default() })]

    #[test]
    fn generated_sketches_keep_every_promise(seed in any::<u64>()) {
        let system = generate(seed, true, None);
        if let Err(problem) = check_promises(&system) {
            prop_assert!(false, "seed {seed:#x}: {problem}\n{system:?}");
        }
    }
}
