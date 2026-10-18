//! M3.3: each constraint form M3.3 adds, through the bridge, in this process. Expected
//! values are worked out by hand: where each form puts a free point, from a start the
//! solver's first step takes straight there or close, and the degrees of freedom (unknowns
//! less independent equations).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/systems.rs"]
mod systems;

use std::f64::consts::{FRAC_PI_2, PI};

use silico_protocol::{
    Algorithm, Geometry, PointOf, PointValues, SketchGeometry, SketchSystem, SolveStatus, Solved,
    SolverConstraint, Value,
};
use systems::Builder;

/// The solver meets each constraint to 1e-10 (its convergence); a point lands within a few
/// times that of where it must be.
const CLOSE: f64 = 1e-9;

fn solve(system: &SketchSystem) -> Solved {
    system.check().unwrap();
    silico_sketch::solve(system).unwrap()
}

/// Where `p` is at the values `v`.
fn at(s: &SketchSystem, v: &[f64], p: PointValues) -> [f64; 2] {
    let _ = s;
    [v[p.x.0 as usize], v[p.y.0 as usize]]
}

fn assert_at(got: [f64; 2], want: [f64; 2], tolerance: f64, what: &str) {
    assert!(
        (got[0] - want[0]).abs() <= tolerance && (got[1] - want[1]).abs() <= tolerance,
        "{what}: got {got:?}, want {want:?}"
    );
}

fn fixed_point_values(b: &mut Builder, x: f64, y: f64) -> PointValues {
    PointValues {
        x: b.fixed(x),
        y: b.fixed(y),
    }
}

/// A line the solver keeps, from `a` to `b`.
fn fixed_line(b: &mut Builder, from: (f64, f64), to: (f64, f64)) -> Geometry {
    let start = fixed_point_values(b, from.0, from.1);
    let end = fixed_point_values(b, to.0, to.1);
    b.add(SketchGeometry::Line { start, end })
}

/// A circle the solver keeps.
fn fixed_circle(b: &mut Builder, c: (f64, f64), r: f64) -> Geometry {
    let center = fixed_point_values(b, c.0, c.1);
    let radius = b.fixed(r);
    b.add(SketchGeometry::Circle { center, radius })
}

/// A free point, and its values.
fn free_point(b: &mut Builder, x: f64, y: f64) -> (Geometry, PointValues) {
    let p = b.point_values(x, y);
    (b.add(SketchGeometry::Point(p)), p)
}

fn solved_once(system: &SketchSystem) -> Solved {
    let solved = solve(system);
    assert_eq!(solved.status, SolveStatus::Success, "{solved:?}");
    assert!(solved.conflicting.is_empty() && solved.redundant.is_empty());
    solved
}

#[test]
fn two_points_made_level_and_one_above_the_other() {
    for level in [true, false] {
        let mut b = Builder::default();
        let a = b.fixed_point(1.0, 2.0);
        let (q, qv) = free_point(&mut b, 5.0, 7.0);
        let (pa, pq) = (PointOf::Point(a), PointOf::Point(q));
        b.constrain(|tag| {
            if level {
                SolverConstraint::HorizontalPoints { tag, a: pa, b: pq }
            } else {
                SolverConstraint::VerticalPoints { tag, a: pa, b: pq }
            }
        });
        let s = b.system(Algorithm::DogLeg);
        let solved = solved_once(&s);
        // One equation on two unknowns; the step moves only the coordinate it reads.
        assert_eq!(solved.dofs, 1);
        let want = if level { [5.0, 2.0] } else { [1.0, 7.0] };
        assert_at(at(&s, &solved.values, qv), want, CLOSE, "the free point");
    }
}

#[test]
fn a_points_distance_from_a_line_keeps_the_side_it_starts_on() {
    // 1.1.3's distance is unsigned: from above the line the point lands 2 above it, from
    // below 2 below (the core checks the stored side after the solve).
    for (from, want) in [(5.0, 2.0), (-5.0, -2.0)] {
        let mut b = Builder::default();
        let line = fixed_line(&mut b, (0.0, 0.0), (10.0, 0.0));
        let (p, pv) = free_point(&mut b, 3.0, from);
        let two = b.fixed(2.0);
        b.constrain(|tag| SolverConstraint::PointLineDistance {
            tag,
            point: PointOf::Point(p),
            line,
            value: two,
        });
        let s = b.system(Algorithm::DogLeg);
        let solved = solved_once(&s);
        assert_eq!(solved.dofs, 1);
        assert_at(at(&s, &solved.values, pv), [3.0, want], CLOSE, "the point");
    }
}

#[test]
fn a_point_on_a_circle_and_on_an_arcs_whole_circle() {
    // A point at (6, 8) put on a circle of radius 5 about the origin lands on it; one at
    // (−6, −8) on a quarter arc from 0° to 90° lands on the arc's circle but not the arc, in
    // the opposite quarter, as FreeCAD's PointOnObject puts it. (Where on the circle is
    // DogLeg's: its trust region doesn't take the first, radial step whole.)
    let mut b = Builder::default();
    let circle = fixed_circle(&mut b, (0.0, 0.0), 5.0);
    let center = fixed_point_values(&mut b, 0.0, 0.0);
    let arc = {
        let radius = b.fixed(5.0);
        let start_angle = b.fixed(0.0);
        let end_angle = b.fixed(FRAC_PI_2);
        let start = fixed_point_values(&mut b, 5.0, 0.0);
        let end = fixed_point_values(&mut b, 0.0, 5.0);
        b.add(SketchGeometry::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            start,
            end,
        })
    };
    let (p, pv) = free_point(&mut b, 6.0, 8.0);
    let (q, qv) = free_point(&mut b, -6.0, -8.0);
    b.constrain(|tag| SolverConstraint::PointOnCircle {
        tag,
        point: PointOf::Point(p),
        circle,
    });
    b.constrain(|tag| SolverConstraint::PointOnCircle {
        tag,
        point: PointOf::Point(q),
        circle: arc,
    });
    let s = b.system(Algorithm::DogLeg);
    let solved = solved_once(&s);
    assert_eq!(solved.dofs, 2);
    let radius = |p: [f64; 2]| (p[0] * p[0] + p[1] * p[1]).sqrt();
    let (p, q) = (at(&s, &solved.values, pv), at(&s, &solved.values, qv));
    assert!((radius(p) - 5.0).abs() <= CLOSE, "on the circle: {p:?}");
    assert!(
        (radius(q) - 5.0).abs() <= CLOSE,
        "on the arc's circle: {q:?}"
    );
    assert!(q[0] < 0.0 && q[1] < 0.0, "off the arc: {q:?}");
}

/// An arc about the origin the solver may change, from (r, 0) to (0, r), with its rules.
fn free_quarter_arc(b: &mut Builder, r: f64) -> (Geometry, Value) {
    let center = fixed_point_values(b, 0.0, 0.0);
    let radius = b.unknown(r);
    let start_angle = b.unknown(0.0);
    let end_angle = b.unknown(FRAC_PI_2);
    let start = b.point_values(r, 0.0);
    let end = b.point_values(0.0, r);
    let arc = b.add(SketchGeometry::Arc {
        center,
        radius,
        start_angle,
        end_angle,
        start,
        end,
    });
    b.constraints.push(SolverConstraint::ArcRules { arc });
    (arc, radius)
}

#[test]
fn equal_radii_of_circles_and_arcs_in_either_order() {
    // A circle of radius 5 the solver keeps, and a circle and two arcs of radius 3 each
    // made equal to it, or to each other, named in each order: all come to 5.
    let mut b = Builder::default();
    let five = fixed_circle(&mut b, (20.0, 0.0), 5.0);
    let center = fixed_point_values(&mut b, -20.0, 0.0);
    let r = b.unknown(3.0);
    let circle = b.add(SketchGeometry::Circle { center, radius: r });
    let (arc, ar) = free_quarter_arc(&mut b, 3.0);
    let (other, or) = {
        let (g, v) = free_quarter_arc(&mut b, 3.0);
        (g, v)
    };
    for (a, c) in [(five, circle), (arc, five), (arc, other)] {
        b.constrain(|tag| SolverConstraint::EqualRadius { tag, a, b: c });
    }
    let s = b.system(Algorithm::DogLeg);
    let solved = solved_once(&s);
    for v in [r, ar, or] {
        assert!(
            (solved.values[v.0 as usize] - 5.0).abs() <= CLOSE,
            "{solved:?}"
        );
    }
}

#[test]
fn a_line_tangent_to_a_circle_keeps_the_side_the_circle_is_on() {
    for (from, want) in [(3.0, 2.0), (-3.0, -2.0)] {
        let mut b = Builder::default();
        let line = fixed_line(&mut b, (0.0, 0.0), (10.0, 0.0));
        let center = b.point_values(5.0, from);
        let radius = b.fixed(2.0);
        let circle = b.add(SketchGeometry::Circle { center, radius });
        b.constrain(|tag| SolverConstraint::TangentLine { tag, line, circle });
        let s = b.system(Algorithm::DogLeg);
        let solved = solved_once(&s);
        assert_eq!(solved.dofs, 1);
        assert_at(
            at(&s, &solved.values, center),
            [5.0, want],
            CLOSE,
            "the centre",
        );
    }
}

#[test]
fn circles_tangent_outside_and_inside() {
    // A circle of radius 3 at the origin, and one of radius 2 whose centre starts at (10, 0):
    // outside, the centres end 5 apart; inside, 1.
    for (internal, want) in [(false, 5.0), (true, 1.0)] {
        let mut b = Builder::default();
        let a = fixed_circle(&mut b, (0.0, 0.0), 3.0);
        let center = b.point_values(10.0, 0.0);
        let radius = b.fixed(2.0);
        let c = b.add(SketchGeometry::Circle { center, radius });
        b.constrain(|tag| SolverConstraint::TangentCircles {
            tag,
            a,
            b: c,
            internal,
        });
        let s = b.system(Algorithm::DogLeg);
        let solved = solved_once(&s);
        assert_eq!(solved.dofs, 1);
        assert_at(
            at(&s, &solved.values, center),
            [want, 0.0],
            CLOSE,
            "the centre",
        );
    }
}

/// The points where lines from `p` touch a circle of radius `r` about the origin: those `t`
/// with |t| = r and t·(p − t) = 0, so t·p = r².
fn tangent_points(p: [f64; 2], r: f64) -> [[f64; 2]; 2] {
    let pp = p[0] * p[0] + p[1] * p[1];
    let (k, h) = (r * r / pp, r * (pp - r * r).sqrt() / pp);
    let perp = [-p[1], p[0]];
    [
        [k * p[0] + h * perp[0], k * p[1] + h * perp[1]],
        [k * p[0] - h * perp[0], k * p[1] - h * perp[1]],
    ]
}

#[test]
fn a_line_ending_tangent_on_a_circle_takes_the_tangent_its_direction_says() {
    // A line from (−20, 10.5), held there, ending on a circle of radius 10 about the origin
    // and tangent to it there: the roadmap's oracle case. With the angle π the line's own
    // direction is the opposite of the circle's (counter-clockwise) where they touch: the
    // upper tangent point, (0.248380314661, 9.996914885068); with 0 the same, the lower.
    let p = [-20.0, 10.5];
    let [lower, upper] = tangent_points(p, 10.0);
    assert!((upper[0] - 0.248_380_314_661).abs() < 1e-12, "{upper:?}");
    assert!((upper[1] - 9.996_914_885_068).abs() < 1e-12, "{upper:?}");
    for (angle, drawn, want) in [(PI, [0.5, 10.2], upper), (0.0, [-8.0, -6.0], lower)] {
        let mut b = Builder::default();
        let circle = fixed_circle(&mut b, (0.0, 0.0), 10.0);
        let start = fixed_point_values(&mut b, p[0], p[1]);
        let end = b.point_values(drawn[0], drawn[1]);
        let line = b.add(SketchGeometry::Line { start, end });
        let angle = b.fixed(angle);
        b.constrain(|tag| SolverConstraint::TangentOn {
            tag,
            a: line,
            point: PointOf::End(line),
            b: circle,
            angle,
        });
        let s = b.system(Algorithm::DogLeg);
        let solved = solved_once(&s);
        // The end's two unknowns, and two equations under one tag.
        assert_eq!(solved.dofs, 0);
        assert_at(
            at(&s, &solved.values, end),
            want,
            CLOSE,
            "the tangent point",
        );
    }
}

#[test]
fn a_line_and_an_arc_meeting_tangent_at_their_shared_point() {
    // A line ending at the origin, where an arc about (0, 5) starts (so the arc runs
    // counter-clockwise from there, along +x): held smooth (0) from a line coming from the
    // left, or a cusp (π) from one coming from the right, the line's free start comes level
    // with the origin.
    for (from, angle) in [(-10.0, 0.0), (10.0, PI)] {
        let mut b = Builder::default();
        let shared = fixed_point_values(&mut b, 0.0, 0.0);
        let start = PointValues {
            x: b.fixed(from),
            y: b.unknown(1.0),
        };
        let line = b.add(SketchGeometry::Line { start, end: shared });
        let center = fixed_point_values(&mut b, 0.0, 5.0);
        let radius = b.unknown(5.0);
        let start_angle = b.unknown(-FRAC_PI_2);
        let end_angle = b.unknown(0.0);
        let end = b.point_values(5.0, 5.0);
        let arc = b.add(SketchGeometry::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            start: shared,
            end,
        });
        b.constraints.push(SolverConstraint::ArcRules { arc });
        let angle = b.fixed(angle);
        b.constrain(|tag| SolverConstraint::AngleViaPoint {
            tag,
            a: line,
            b: arc,
            point: PointOf::End(line),
            angle,
        });
        let s = b.system(Algorithm::DogLeg);
        let solved = solved_once(&s);
        let y = solved.values[start.y.0 as usize];
        // An angle met to 1e-10 rad over a line 10 long: its start within 1e-9.
        assert!(y.abs() <= 1e-8, "the line's start at y = {y}");
    }
}

#[test]
fn points_symmetric_about_a_line_and_about_a_point() {
    // (1, 2) mirrored about the y axis is (−1, 2); about (3, 3), (5, 4).
    let mut b = Builder::default();
    let axis = fixed_line(&mut b, (0.0, 0.0), (0.0, 1.0));
    let a = b.fixed_point(1.0, 2.0);
    let m = b.fixed_point(3.0, 3.0);
    let (q, qv) = free_point(&mut b, -3.0, 5.0);
    let (r, rv) = free_point(&mut b, 4.0, 6.0);
    b.constrain(|tag| SolverConstraint::SymmetricLine {
        tag,
        a: PointOf::Point(a),
        b: PointOf::Point(q),
        line: axis,
    });
    b.constrain(|tag| SolverConstraint::SymmetricPoint {
        tag,
        a: PointOf::Point(a),
        b: PointOf::Point(r),
        center: PointOf::Point(m),
    });
    let s = b.system(Algorithm::DogLeg);
    let solved = solved_once(&s);
    // Two equations each, on two points' four unknowns.
    assert_eq!(solved.dofs, 0);
    assert_at(
        at(&s, &solved.values, qv),
        [-1.0, 2.0],
        CLOSE,
        "about the line",
    );
    assert_at(
        at(&s, &solved.values, rv),
        [5.0, 4.0],
        CLOSE,
        "about the point",
    );
}

#[test]
fn a_midpoint_on_a_line_is_one_equation() {
    // The middle of (1, 2) and a free point on the y axis: the free point's x comes to −1,
    // and its y, which the one equation doesn't read, stays.
    let mut b = Builder::default();
    let axis = fixed_line(&mut b, (0.0, 0.0), (0.0, 1.0));
    let a = b.fixed_point(1.0, 2.0);
    let (q, qv) = free_point(&mut b, -3.0, 5.0);
    b.constrain(|tag| SolverConstraint::MidpointOnLine {
        tag,
        a: PointOf::Point(a),
        b: PointOf::Point(q),
        line: axis,
    });
    let s = b.system(Algorithm::DogLeg);
    let solved = solved_once(&s);
    assert_eq!(solved.dofs, 1);
    assert_at(
        at(&s, &solved.values, qv),
        [-1.0, 5.0],
        CLOSE,
        "the free point",
    );
}

#[test]
fn anchored_sqp_meets_the_constraints_as_near_the_start_as_they_let_it() {
    // FreeCAD's last resort, alone: a point drawn at (3, 4), 10 from the origin. Anchored
    // where it starts, the solve keeps the distance and moves it no further than it must:
    // straight out, to (6, 8).
    let mut b = Builder::default();
    let origin = b.fixed_point(0.0, 0.0);
    let (p, pv) = free_point(&mut b, 3.0, 4.0);
    let ten = b.fixed(10.0);
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Point(origin),
        b: PointOf::Point(p),
        value: ten,
    });
    let s = b.system(Algorithm::AnchoredSqp);
    let solved = solved_once(&s);
    assert_eq!(solved.algorithm, Some(Algorithm::AnchoredSqp));
    assert_at(at(&s, &solved.values, pv), [6.0, 8.0], 1e-7, "the point");
}
