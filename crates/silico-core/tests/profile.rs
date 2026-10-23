//! M3.5: a solved sketch's profile (docs/design/m3.5-profile.md, docs/ports/profile.md).
//! Every row of the port note's *Edge cases*, FreeCAD's FaceMakerBullseye and face-making
//! tests that port, and the neighbour cases of the tolerance decision: the refusals, every
//! element named, or the regions, their depth, direction, and area. Areas are worked out by
//! hand (rectangles, circles πr², half discs, circular segments (r²/2)(θ − sin θ)), never
//! by the code under test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/profile.rs"]
mod draw;

use std::f64::consts::PI;

use draw::{Draw, FEATURE, close, feature, named};
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::document::{Coordinate, ElementId, Plane};
use silico_core::profile::{CurveKind, Edge, Loop, Profile, Region, profile};
use silico_protocol::Curve;

const EPS: f64 = 1e-12;

/// The rule every "apart" and point message ends with.
const RULE: &str =
    "a profile's curves must be more than 1e-7 mm apart except at a point they share";

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

/// A line walked from point `from`, at `start`, to point `to`, at `end`.
fn line(
    curve: &str,
    (from, start): (&str, [f64; 2]),
    (to, end): (&str, [f64; 2]),
    reversed: bool,
) -> Edge {
    Edge {
        curve: id(curve),
        kind: CurveKind::Line,
        center: None,
        ends: Some([id(from), id(to)]),
        reversed,
        sent: Curve::Line { start, end },
    }
}

/// Every loop walks closed from its smallest curve ID, each curve starting where the one
/// before ends; outer loops counter-clockwise at even depth, holes clockwise.
fn check_walks(p: &Profile) {
    let check = |l: &Loop| {
        let first = &l.curves[0].curve;
        assert!(l.curves.iter().all(|e| &e.curve >= first), "{l:#?}");
        let n = l.curves.len();
        for i in 0..n {
            if let (Some([_, end]), Some([start, _])) =
                (&l.curves[i].ends, &l.curves[(i + 1) % n].ends)
            {
                assert_eq!(end, start, "{l:#?}");
            } else {
                assert_eq!(n, 1, "only a circle has no ends: {l:#?}");
                assert_eq!(l.curves[0].kind, CurveKind::Circle);
            }
        }
    };
    for r in &p.regions {
        assert!(r.depth.is_multiple_of(2));
        assert!(r.outer.area > 0.0, "{r:#?}");
        check(&r.outer);
        for h in &r.holes {
            assert!(h.area < 0.0, "{r:#?}");
            check(h);
        }
    }
}

fn made(d: &Draw) -> Profile {
    let p = d.made();
    check_walks(&p);
    p
}

/// The regions' areas and depths, in the profile's order.
fn regions(p: &Profile) -> Vec<(f64, usize)> {
    p.regions.iter().map(|r| (r.area(), r.depth)).collect()
}

fn assert_areas(p: &Profile, expected: &[(f64, usize)]) {
    let got = regions(p);
    assert_eq!(got.len(), expected.len(), "{got:?}");
    for ((a, da), (b, db)) in got.iter().zip(expected) {
        assert!(close(*a, *b, EPS), "{got:?} against {expected:?}");
        assert_eq!(da, db, "{got:?} against {expected:?}");
    }
}

// FaceMakerBullseye.cpp (FreeCAD main, 88c80b3ed8) and the port note's accepted rows.

#[test]
fn a_4_by_3_rectangle_either_way_round_is_one_region_of_12_walked_counter_clockwise() {
    let mut d = Draw::new();
    let n = d.rectangle(0.0, 0.0, 4.0, 3.0);
    let p = made(&d);
    assert_areas(&p, &[(12.0, 0)]);
    assert!(close(p.regions[0].outer.perimeter, 14.0, EPS));
    assert!(p.regions[0].holes.is_empty());
    assert_eq!(
        p.regions[0].outer.curves,
        vec![
            line(
                &n[0],
                ("p0000001", [0.0, 0.0]),
                ("p0000002", [4.0, 0.0]),
                false
            ),
            line(
                &n[1],
                ("p0000002", [4.0, 0.0]),
                ("p0000003", [4.0, 3.0]),
                false
            ),
            line(
                &n[2],
                ("p0000003", [4.0, 3.0]),
                ("p0000004", [0.0, 3.0]),
                false
            ),
            line(
                &n[3],
                ("p0000004", [0.0, 3.0]),
                ("p0000001", [0.0, 0.0]),
                false
            ),
        ]
    );

    // Drawn clockwise: (0,0) up to (0,3), across, down, back. Walked from its first line,
    // reversed, then the others back the way they were drawn.
    let mut d = Draw::new();
    let n = d.polygon(&[(0.0, 0.0), (0.0, 3.0), (4.0, 3.0), (4.0, 0.0)]);
    let p = made(&d);
    assert_areas(&p, &[(12.0, 0)]);
    assert_eq!(
        p.regions[0].outer.curves,
        vec![
            line(
                &n[0],
                ("p0000002", [0.0, 3.0]),
                ("p0000001", [0.0, 0.0]),
                true
            ),
            line(
                &n[3],
                ("p0000001", [0.0, 0.0]),
                ("p0000004", [4.0, 0.0]),
                true
            ),
            line(
                &n[2],
                ("p0000004", [4.0, 0.0]),
                ("p0000003", [4.0, 3.0]),
                true
            ),
            line(
                &n[1],
                ("p0000003", [4.0, 3.0]),
                ("p0000002", [0.0, 3.0]),
                true
            ),
        ]
    );
}

#[test]
fn a_loop_walks_from_its_smallest_curve_id_whatever_the_order_and_direction_drawn() {
    // A (0,0), B (4,0), C (4,3), D (0,3); the smallest ID is C→B's, drawn down the right
    // side. Walked from it as drawn, C→B→A→D→C is clockwise, so the outer loop runs it the
    // other way from the same line: B→C (n0000003, reversed), C→D (n0000007 as drawn),
    // D→A (n0000005, reversed), A→B (n0000009 as drawn).
    let mut d = Draw::new();
    let (a, b, c, e) = ("p000000a", "p000000b", "p000000c", "p000000d");
    d.point_named(a, 0.0, 0.0);
    d.point_named(b, 4.0, 0.0);
    d.point_named(c, 4.0, 3.0);
    d.point_named(e, 0.0, 3.0);
    d.line_named("n0000009", a, b);
    d.line_named("n0000003", c, b);
    d.line_named("n0000007", c, e);
    d.line_named("n0000005", a, e);
    let p = made(&d);
    assert_areas(&p, &[(12.0, 0)]);
    assert_eq!(
        p.regions[0].outer.curves,
        vec![
            line("n0000003", (b, [4.0, 0.0]), (c, [4.0, 3.0]), true),
            line("n0000007", (c, [4.0, 3.0]), (e, [0.0, 3.0]), false),
            line("n0000005", (e, [0.0, 3.0]), (a, [0.0, 0.0]), true),
            line("n0000009", (a, [0.0, 0.0]), (b, [4.0, 0.0]), false),
        ]
    );
}

#[test]
fn a_circle_is_a_loop_by_itself_counter_clockwise_outside_and_clockwise_as_a_hole() {
    // getWireDirection{CCW,CW}Circle: a circle r 5.
    let mut d = Draw::new();
    let c = d.circle(0.0, 0.0, 5.0);
    let p = made(&d);
    assert_areas(&p, &[(25.0 * PI, 0)]);
    assert!(close(p.regions[0].outer.perimeter, 10.0 * PI, EPS));
    assert_eq!(
        p.regions[0].outer.curves,
        vec![Edge {
            curve: id(&c),
            kind: CurveKind::Circle,
            center: Some(id("p0000001")),
            ends: None,
            reversed: false,
            sent: Curve::Circle {
                center: [0.0, 0.0],
                radius: 5.0,
            },
        }]
    );

    // The port note's row: a circle r 2, 4π.
    let mut d = Draw::new();
    d.circle(1.0, -3.0, 2.0);
    assert_areas(&made(&d), &[(4.0 * PI, 0)]);

    // As a hole of a 20 × 20 square: walked clockwise, its area negative.
    let mut d = Draw::new();
    d.rectangle(-10.0, -10.0, 10.0, 10.0);
    let c = d.circle(0.0, 0.0, 5.0);
    let p = made(&d);
    assert_areas(&p, &[(400.0 - 25.0 * PI, 0)]);
    let hole = &p.regions[0].holes[0];
    assert_eq!(hole.curves[0].curve, id(&c));
    assert!(hole.curves[0].reversed);
    assert!(close(hole.area, -25.0 * PI, EPS));
}

#[test]
fn a_3_by_2_rectangle_drawn_either_way_is_counter_clockwise_and_a_circle_r_1_is_pi() {
    // getWireDirection{CCW,CW}Rectangle and makeElementFace{Simple,ClosedWire}.
    for corners in [
        [(0.0, 0.0), (3.0, 0.0), (3.0, 2.0), (0.0, 2.0)],
        [(0.0, 0.0), (0.0, 2.0), (3.0, 2.0), (3.0, 0.0)],
    ] {
        let mut d = Draw::new();
        d.polygon(&corners);
        let p = made(&d);
        assert_areas(&p, &[(6.0, 0)]);
        let clockwise = corners[1].0 == 0.0;
        assert!(
            p.regions[0]
                .outer
                .curves
                .iter()
                .all(|e| e.reversed == clockwise)
        );
    }
    let mut d = Draw::new();
    d.circle(0.0, 0.0, 1.0);
    assert_areas(&made(&d), &[(PI, 0)]);
}

#[test]
fn an_arc_and_its_chord_enclose_the_half_disc() {
    // The port note's row: an arc r 2 and its chord, 2π.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(2.0, 0.0);
    let e = d.point(-2.0, 0.0);
    let arc = d.arc(&o, &s, &e);
    let chord = d.line(&e, &s);
    let p = made(&d);
    assert_areas(&p, &[(2.0 * PI, 0)]);
    assert!(close(p.regions[0].outer.perimeter, 2.0 * PI + 4.0, EPS));
    assert_eq!(
        p.regions[0].outer.curves,
        vec![
            Edge {
                curve: id(&arc),
                kind: CurveKind::Arc,
                center: Some(id(&o)),
                ends: Some([id(&s), id(&e)]),
                reversed: false,
                // From angle 0 round to π.
                sent: Curve::Arc {
                    center: [0.0, 0.0],
                    start: [2.0, 0.0],
                    end: [-2.0, 0.0],
                    radius: 2.0,
                    start_angle: 0.0,
                    end_angle: PI,
                },
            },
            line(&chord, (&e, [-2.0, 0.0]), (&s, [2.0, 0.0]), false),
        ]
    );

    // The chord drawn from the arc's start to its end: walked back, end to start.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(2.0, 0.0);
    let e = d.point(-2.0, 0.0);
    d.arc(&o, &s, &e);
    d.line(&s, &e);
    assert_areas(&made(&d), &[(2.0 * PI, 0)]);
}

/// The slot of the port note's row: half circles r 1 joined by lines 4 long, each line
/// tangent to both arcs at the points it shares with them.
fn slot(d: &mut Draw, x: f64, y: f64) {
    let c1 = d.point(x, y);
    let c2 = d.point(x + 4.0, y);
    let a = d.point(x, y - 1.0);
    let b = d.point(x + 4.0, y - 1.0);
    let c = d.point(x + 4.0, y + 1.0);
    let e = d.point(x, y + 1.0);
    d.line(&a, &b);
    d.arc(&c2, &b, &c);
    d.line(&c, &e);
    d.arc(&c1, &e, &a);
}

#[test]
fn a_slot_of_tangent_lines_and_arcs_is_8_plus_pi() {
    let mut d = Draw::new();
    slot(&mut d, 0.0, 0.0);
    let p = made(&d);
    assert_areas(&p, &[(8.0 + PI, 0)]);
    assert!(close(p.regions[0].outer.perimeter, 8.0 + 2.0 * PI, EPS));
}

#[test]
fn a_rounded_rectangle_of_tangent_quarter_arcs_is_its_rectangle_less_four_corners() {
    // 10 × 6, corners r 1: 60 − (4 − π).
    let mut d = Draw::new();
    let p = |d: &mut Draw, x, y| d.point(x, y);
    let (b0, b1) = (p(&mut d, 1.0, 0.0), p(&mut d, 9.0, 0.0));
    let (r0, r1) = (p(&mut d, 10.0, 1.0), p(&mut d, 10.0, 5.0));
    let (t0, t1) = (p(&mut d, 9.0, 6.0), p(&mut d, 1.0, 6.0));
    let (l0, l1) = (p(&mut d, 0.0, 5.0), p(&mut d, 0.0, 1.0));
    let corners = [
        p(&mut d, 9.0, 1.0),
        p(&mut d, 9.0, 5.0),
        p(&mut d, 1.0, 5.0),
        p(&mut d, 1.0, 1.0),
    ];
    d.line(&b0, &b1);
    d.arc(&corners[0], &b1, &r0);
    d.line(&r0, &r1);
    d.arc(&corners[1], &r1, &t0);
    d.line(&t0, &t1);
    d.arc(&corners[2], &t1, &l0);
    d.line(&l0, &l1);
    d.arc(&corners[3], &l1, &b0);
    assert_areas(&made(&d), &[(56.0 + PI, 0)]);
}

#[test]
fn arcs_tangent_inside_and_outside_and_meeting_in_a_cusp_are_a_profile() {
    // Half a yin-yang: the top half of a circle r 2, the bottom half of the circle r 1 about
    // (−1, 0), and the top half of the circle r 1 about (1, 0), walked back. The big arc
    // and each small one are tangent inside at (±2, 0) (a cusp at (2, 0), where both leave
    // upward), the small ones outside at the origin: 2π − π/2 + π/2.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let left = d.point(-1.0, 0.0);
    let right = d.point(1.0, 0.0);
    let (e, w, m) = (
        d.point(2.0, 0.0),
        d.point(-2.0, 0.0),
        d.point_named("p000000z", 0.0, 0.0),
    );
    d.arc(&o, &e, &w);
    d.arc(&left, &w, &m);
    d.arc(&right, &e, &m);
    assert_areas(&made(&d), &[(2.0 * PI, 0)]);
}

#[test]
fn a_circle_drawn_as_two_half_arcs_is_the_circle() {
    let mut d = Draw::new();
    let o = d.point(3.0, 4.0);
    let s = d.point(5.0, 4.0);
    let e = d.point(1.0, 4.0);
    d.arc(&o, &s, &e);
    d.arc(&o, &e, &s);
    let p = made(&d);
    assert_areas(&p, &[(4.0 * PI, 0)]);
    assert!(close(p.regions[0].outer.perimeter, 4.0 * PI, EPS));
}

#[test]
fn a_line_that_meets_its_arcs_circle_again_beyond_the_arc_is_a_profile() {
    // The arc r 1 from (1, 0) to (0, 1), a line from (0, 1) down through the circle at
    // (0, −1), off the arc, to (0, −2), and back to (1, 0): the triangle (1.5) and the
    // arc's segment (π/4 − 1/2).
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let p0 = d.point(1.0, 0.0);
    let e = d.point(0.0, 1.0);
    let r = d.point(0.0, -2.0);
    d.arc(&o, &p0, &e);
    d.line(&e, &r);
    d.line(&r, &p0);
    assert_areas(&made(&d), &[(1.0 + PI / 4.0, 0)]);
}

#[test]
fn a_cusp_with_a_long_tangent_line_is_a_profile() {
    // The arc r 1 from P (1, 0) to E (0, 1); from P a line straight up, along the arc's
    // tangent, to Q (1, 0.5), so the line and the arc both leave P upward; E to R (2, 2)
    // and R to Q close it outside the circle. The polygon P E R Q, 1.25, less the arc's
    // segment, π/4 − 1/2.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let p0 = d.point(1.0, 0.0);
    let e = d.point(0.0, 1.0);
    let r = d.point(2.0, 2.0);
    let q = d.point(1.0, 0.5);
    d.arc(&o, &p0, &e);
    d.line(&e, &r);
    d.line(&r, &q);
    d.line(&q, &p0);
    assert_areas(&made(&d), &[(1.75 - PI / 4.0, 0)]);
}

#[test]
fn a_hole_r_half_in_a_4_by_3_rectangle_is_12_less_pi_over_4() {
    // faceDrillerWithOneHole, faceDrillerReversedHoleWire, buildEssenceOuterWithHole: the
    // rectangle either way round.
    for clockwise in [false, true] {
        let mut d = Draw::new();
        if clockwise {
            d.polygon(&[(0.0, 0.0), (0.0, 3.0), (4.0, 3.0), (4.0, 0.0)]);
        } else {
            d.rectangle(0.0, 0.0, 4.0, 3.0);
        }
        d.circle(2.0, 1.5, 0.5);
        let p = made(&d);
        assert_areas(&p, &[(12.0 - PI / 4.0, 0)]);
        assert_eq!(p.regions[0].holes.len(), 1);
        assert!(close(p.regions[0].holes[0].area, -PI / 4.0, EPS));
        assert!(close(p.regions[0].perimeter(), 14.0 + PI, EPS));
    }
}

#[test]
fn the_profile_is_the_same_on_every_plane() {
    // faceDrillerFlippedPlane, getWireDirectionFlippedPlane: the profile is in the
    // sketch's own coordinates, so its direction is about the sketch's normal, whichever
    // way that points in the model.
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 4.0, 3.0);
    d.circle(2.0, 1.5, 0.5);
    let on_xy = made(&d);
    assert_eq!(on_xy.plane, Plane::Xy);
    assert_eq!(on_xy.faces().plane, silico_protocol::SketchPlane::Xy);
    for plane in [Plane::Xz, Plane::Yz] {
        d.sketch.plane = plane;
        let on = made(&d);
        // The same regions, on the sketch's plane.
        assert_eq!(on.regions, on_xy.regions);
        assert_eq!(on.plane, plane);
        assert_eq!(on.faces().regions, on_xy.faces().regions);
        assert_eq!(on.faces().plane, silico_protocol::SketchPlane::from(plane));
    }
}

#[test]
fn a_10_by_6_rectangle_with_three_holes_is_60_less_1_8125_pi() {
    // faceDrillerMultipleHoles: holes r 0.5, 1, 0.75.
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 10.0, 6.0);
    let holes = [
        d.circle(2.0, 3.0, 0.5),
        d.circle(5.0, 3.0, 1.0),
        d.circle(8.0, 3.0, 0.75),
    ];
    let p = made(&d);
    assert_areas(&p, &[(60.0 - 1.8125 * PI, 0)]);
    let got: Vec<&ElementId> = p.regions[0]
        .holes
        .iter()
        .map(|h| &h.curves[0].curve)
        .collect();
    let expected: Vec<ElementId> = holes.iter().map(|h| id(h)).collect();
    assert_eq!(got, expected.iter().collect::<Vec<_>>());
}

#[test]
fn nested_islands_are_two_regions() {
    // buildEssenceNestedIslands: a 10 × 10 square, circles r 4 and 2 about its centre: the
    // square less the r 4 circle, and the r 2 island, 100 − 12π in all. Regions are in
    // their outer loops' smallest IDs' order: the island's circle (c…) before the square's
    // lines (n…).
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 10.0, 10.0);
    d.circle(5.0, 5.0, 4.0);
    d.circle(5.0, 5.0, 2.0);
    let p = made(&d);
    assert_areas(&p, &[(4.0 * PI, 2), (100.0 - 16.0 * PI, 0)]);
    let total: f64 = p.regions.iter().map(Region::area).sum();
    assert!(close(total, 100.0 - 12.0 * PI, EPS));
}

#[test]
fn a_100_square_with_25_holes_is_10000_less_25_pi() {
    // buildEssenceManyWires.
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 100.0, 100.0);
    for i in 0..5 {
        for j in 0..5 {
            d.circle(10.0 + 20.0 * f64::from(i), 10.0 + 20.0 * f64::from(j), 1.0);
        }
    }
    let p = made(&d);
    assert_areas(&p, &[(10_000.0 - 25.0 * PI, 0)]);
    assert_eq!(p.regions[0].holes.len(), 25);
}

#[test]
fn face_maker_unified_nesting_cases_have_their_areas() {
    // main's TestFaceMakerUnifiedPlanar.py (7186d30f6e), its nesting cases, and
    // TestPartApp.py's PartExtrusionTests face.
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 40.0, 40.0);
    d.circle(20.0, 20.0, 5.0);
    assert_areas(&made(&d), &[(1600.0 - 25.0 * PI, 0)]);

    let mut d = Draw::new();
    d.circle(0.0, 0.0, 10.0);
    d.circle(0.0, 0.0, 5.0);
    assert_areas(&made(&d), &[(75.0 * PI, 0)]);

    let mut d = Draw::new();
    d.rectangle(-15.0, -15.0, 15.0, 15.0);
    d.rectangle(-10.0, -10.0, 10.0, 10.0);
    d.rectangle(-5.0, -5.0, 5.0, 5.0);
    let p = made(&d);
    assert_areas(&p, &[(500.0, 0), (100.0, 2)]);
    assert!(close(p.regions.iter().map(Region::area).sum(), 600.0, EPS));

    let mut d = Draw::new();
    for r in [25.8, 15.4, 6.4, 2.5] {
        d.circle(0.0, 0.0, r);
    }
    let p = made(&d);
    assert_areas(
        &p,
        &[
            (PI * (25.8 * 25.8 - 15.4 * 15.4), 0),
            (PI * (6.4 * 6.4 - 2.5 * 2.5), 2),
        ],
    );

    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 10.0, 10.0);
    d.rectangle(2.0, 2.0, 8.0, 8.0);
    assert_areas(&made(&d), &[(64.0, 0)]);
}

#[test]
fn a_square_and_circles_about_its_centre_nest_to_any_depth_in_any_order() {
    // A 100 × 100 square, circles r 40, 20, 10 about its centre: 10000 − 1600π, and the
    // r 20 island less the r 10 hole, 300π. Drawn outer first or inner first.
    for inner_first in [false, true] {
        let mut d = Draw::new();
        if inner_first {
            for r in [10.0, 20.0, 40.0] {
                d.circle(50.0, 50.0, r);
            }
            d.rectangle(0.0, 0.0, 100.0, 100.0);
        } else {
            d.rectangle(0.0, 0.0, 100.0, 100.0);
            for r in [40.0, 20.0, 10.0] {
                d.circle(50.0, 50.0, r);
            }
        }
        let p = made(&d);
        let mut got = regions(&p);
        got.sort_by_key(|&(_, depth)| depth);
        assert_eq!(got.len(), 2);
        assert!(close(got[0].0, 10_000.0 - 1600.0 * PI, EPS) && got[0].1 == 0);
        assert!(close(got[1].0, 300.0 * PI, EPS) && got[1].1 == 2);
    }

    // Squares 100, 80, and 10, nested: 3600 and 100.
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 100.0, 100.0);
    d.rectangle(10.0, 10.0, 90.0, 90.0);
    d.rectangle(45.0, 45.0, 55.0, 55.0);
    assert_areas(&made(&d), &[(3600.0, 0), (100.0, 2)]);
}

/// Each region by its outer loop's first curve: its depth, its holes' first curves, and its
/// area.
fn structure(p: &Profile) -> Vec<(String, usize, Vec<String>, f64)> {
    p.regions
        .iter()
        .map(|r| {
            let first = |l: &Loop| l.curves[0].curve.to_string();
            let holes = r.holes.iter().map(first).collect();
            (first(&r.outer), r.depth, holes, r.area())
        })
        .collect()
}

#[test]
fn each_hole_belongs_to_the_loop_just_around_it_at_every_depth() {
    // Squares, about (10, 10): A 20 (0 to 20), B 14 (3 to 17), C 8 (6 to 14), D 2 (9 to
    // 11), nested; E, 2 x 2 at (17.5, 0.5), in A beside B; F, 1 x 1 at (7, 7), in C beside
    // D. Depths: A 0, B and E 1, C 2, D and F 3. So A's region has holes B and E, and
    // C's has D and F: 400 - 196 - 4 = 200, and 64 - 4 - 1 = 59. A hole given to any
    // loop around it but the innermost (A for D, say) moves it to another region. Drawn
    // outermost first and innermost first, so the loops around one come in either order.
    for innermost_first in [false, true] {
        let mut d = Draw::new();
        let mut squares = vec![
            ("A", (0.0, 0.0, 20.0, 20.0)),
            ("B", (3.0, 3.0, 17.0, 17.0)),
            ("E", (17.5, 0.5, 19.5, 2.5)),
            ("C", (6.0, 6.0, 14.0, 14.0)),
            ("F", (7.0, 7.0, 8.0, 8.0)),
            ("D", (9.0, 9.0, 11.0, 11.0)),
        ];
        if innermost_first {
            squares.reverse();
        }
        let mut first = std::collections::BTreeMap::new();
        for (name, (x0, y0, x1, y1)) in squares {
            first.insert(name, d.rectangle(x0, y0, x1, y1)[0].clone());
        }
        let p = made(&d);
        let mut got = structure(&p);
        got.sort_by_key(|r| r.1);
        let holes = |a: &str, b: &str| {
            let mut h = vec![first[a].clone(), first[b].clone()];
            h.sort();
            h
        };
        let want = [
            (first["A"].clone(), 0, holes("B", "E"), 200.0),
            (first["C"].clone(), 2, holes("D", "F"), 59.0),
        ];
        assert_eq!(got.len(), 2, "{got:?}");
        for (g, w) in got.iter().zip(&want) {
            assert_eq!((&g.0, g.1, &g.2), (&w.0, w.1, &w.2), "{got:?}");
            assert!(close(g.3, w.3, EPS), "{got:?}");
        }
    }
}

#[test]
fn circles_nested_four_deep_beside_a_square_s_islands_belong_where_they_lie() {
    // A 100 x 100 square from the origin; in it, circles r 40, 30, 20, 10 about (50, 50)
    // (depths 1 to 4) and a circle r 4 about (95, 5), beside them (depth 1); in the r 30
    // island, a circle r 3 about (50, 25), between the r 30 and r 20 circles (depth 3).
    // Regions: the square less the r 40 and r 4 circles, 10000 - 1616 pi; the r 30
    // island less the r 20 and r 3 circles, 900 pi - 400 pi - 9 pi = 491 pi; the r 10
    // island, 100 pi. Each hole's loop just around it is also the least in area of the loops
    // around it, so the depth rule and the area rule it replaced agree here.
    let mut d = Draw::new();
    let square = d.rectangle(0.0, 0.0, 100.0, 100.0)[0].clone();
    let r40 = d.circle(50.0, 50.0, 40.0);
    let r30 = d.circle(50.0, 50.0, 30.0);
    let r20 = d.circle(50.0, 50.0, 20.0);
    let r10 = d.circle(50.0, 50.0, 10.0);
    let r4 = d.circle(95.0, 5.0, 4.0);
    let r3 = d.circle(50.0, 25.0, 3.0);
    let p = made(&d);
    let mut got = structure(&p);
    got.sort_by_key(|r| r.1);
    let want = [
        (square, 0, vec![r40, r4], 10_000.0 - 1616.0 * PI),
        (r30, 2, vec![r20, r3], 491.0 * PI),
        (r10, 4, vec![], 100.0 * PI),
    ];
    assert_eq!(got.len(), 3, "{got:?}");
    for (g, w) in got.iter().zip(&want) {
        assert_eq!((&g.0, g.1, &g.2), (&w.0, w.1, &w.2), "{got:?}");
        assert!(close(g.3, w.3, EPS), "{got:?}");
    }
}

#[test]
fn each_hole_belongs_to_the_loop_around_it_not_another_at_its_depth() {
    // Two 10 x 10 squares, from (0, 0) and (20, 0), each with a circle r 2 at its centre:
    // two regions at depth 0, each 100 - 4 pi with its own circle. Drawn in either order.
    // Any loop one shallower than a hole would give both circles to one square.
    for right_first in [false, true] {
        let mut d = Draw::new();
        let mut parts = vec![
            (0.0, String::new(), String::new()),
            (20.0, String::new(), String::new()),
        ];
        if right_first {
            parts.reverse();
        }
        for (x, square, circle) in &mut parts {
            *square = d.rectangle(*x, 0.0, *x + 10.0, 10.0)[0].clone();
            *circle = d.circle(*x + 5.0, 5.0, 2.0);
        }
        let p = made(&d);
        let mut got = structure(&p);
        got.sort_by(|a, b| a.0.cmp(&b.0));
        let mut want: Vec<_> = parts
            .into_iter()
            .map(|(_, square, circle)| (square, 0, vec![circle], 100.0 - 4.0 * PI))
            .collect();
        want.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got.len(), 2, "{got:?}");
        for (g, w) in got.iter().zip(&want) {
            assert_eq!((&g.0, g.1, &g.2), (&w.0, w.1, &w.2), "{got:?}");
            assert!(close(g.3, w.3, EPS), "{got:?}");
        }
    }

    // A 100 x 100 square with a hole from 10 to 90 holding two islands, 20 to 40 and 60 to
    // 80, each with a circle r 5 at its centre: the square less its hole, 3600, and each
    // island less its own circle, 400 - 25 pi, at depth 2.
    let mut d = Draw::new();
    let outer = d.rectangle(0.0, 0.0, 100.0, 100.0)[0].clone();
    let hole = d.rectangle(10.0, 10.0, 90.0, 90.0)[0].clone();
    let low = d.rectangle(20.0, 20.0, 40.0, 40.0)[0].clone();
    let high = d.rectangle(60.0, 60.0, 80.0, 80.0)[0].clone();
    let low_circle = d.circle(30.0, 30.0, 5.0);
    let high_circle = d.circle(70.0, 70.0, 5.0);
    let p = made(&d);
    let mut got = structure(&p);
    got.sort_by(|a, b| a.0.cmp(&b.0));
    let mut want = vec![
        (outer, 0, vec![hole], 3600.0),
        (low, 2, vec![low_circle], 400.0 - 25.0 * PI),
        (high, 2, vec![high_circle], 400.0 - 25.0 * PI),
    ];
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(got.len(), 3, "{got:?}");
    for (g, w) in got.iter().zip(&want) {
        assert_eq!((&g.0, g.1, &g.2), (&w.0, w.1, &w.2), "{got:?}");
        assert!(close(g.3, w.3, EPS), "{got:?}");
    }
}

#[test]
fn separate_outlines_are_separate_regions() {
    // Two squares; four circles r 15 at (±20, ±20) (testPD_TNPSketchPadMultipleSolids).
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 1.0, 1.0);
    d.rectangle(5.0, 0.0, 7.0, 2.0);
    assert_areas(&made(&d), &[(1.0, 0), (4.0, 0)]);

    let mut d = Draw::new();
    for (x, y) in [(20.0, 20.0), (-20.0, 20.0), (-20.0, -20.0), (20.0, -20.0)] {
        d.circle(x, y, 15.0);
    }
    assert_areas(&made(&d), &[(225.0 * PI, 0); 4]);
}

#[test]
fn points_on_no_curve_and_construction_geometry_are_left_out() {
    let mut d = Draw::new();
    let n = d.rectangle(0.0, 0.0, 4.0, 3.0);
    d.point(10.0, 10.0);
    d.construction_line("p0000001", "p0000003");
    let c = d.circle(2.0, 1.5, 0.5);
    d.sketch.circles[0].construction = true;
    let p = made(&d);
    assert_areas(&p, &[(12.0, 0)]);
    assert!(
        p.regions[0]
            .outer
            .curves
            .iter()
            .all(|e| n.contains(&e.curve.to_string()) && e.curve.as_str() != c)
    );
}

#[test]
fn the_solve_s_coordinates_are_used_not_the_stored_ones() {
    // Stored as a bow-tie (which crosses), solved as the 4 × 3 rectangle.
    let mut d = Draw::new();
    d.polygon(&[(0.0, 0.0), (4.0, 3.0), (4.0, 0.0), (0.0, 3.0)]);
    let mut solution = d.solution();
    let at = |x: f64, y: f64| [Coordinate::new(x).unwrap(), Coordinate::new(y).unwrap()];
    solution.points.insert(id("p0000002"), at(4.0, 0.0));
    solution.points.insert(id("p0000003"), at(4.0, 3.0));
    let p = d.profile_from(&solution).unwrap();
    assert_areas(&p, &[(12.0, 0)]);

    // And a circle's radius from the solve.
    let mut d = Draw::new();
    let c = d.circle(0.0, 0.0, 1.0);
    let mut solution = d.solution();
    solution.radii.insert(id(&c), Coordinate::new(3.0).unwrap());
    let p = d.profile_from(&solution).unwrap();
    assert_areas(&p, &[(9.0 * PI, 0)]);
}

#[test]
fn a_solution_of_another_sketch_is_a_bug_named() {
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 4.0, 3.0);
    let mut solution = d.solution();
    solution.points.remove(&id("p0000003"));
    let r = d.profile_from(&solution).unwrap_err();
    assert_eq!(r.total(), 1);
    let e = &r.errors()[0];
    assert_eq!(e.code(), ErrorCode::Internal);
    assert_eq!(e.feature(), Some(FEATURE));
    assert_eq!(e.elements(), ["p0000003", "n0000006"]);
}

#[test]
fn the_order_the_sketch_lists_its_geometry_changes_nothing() {
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 100.0, 100.0);
    d.circle(30.0, 30.0, 10.0);
    slot(&mut d, 60.0, 60.0);
    d.rectangle(60.0, 10.0, 90.0, 40.0);
    d.circle(75.0, 25.0, 5.0);
    let forward = made(&d);
    d.sketch.points.reverse();
    d.sketch.lines.reverse();
    d.sketch.circles.reverse();
    d.sketch.arcs.reverse();
    assert_eq!(made(&d), forward);
    assert_eq!(forward.regions.len(), 2);
    assert!(close(
        forward.regions.iter().map(Region::area).sum(),
        10_000.0 - 100.0 * PI - (8.0 + PI) - 900.0 + 25.0 * PI,
        EPS
    ));
}

// The refusals: the port note's Edge cases rows, and the messages, each spelt once.

#[test]
fn a_sketch_with_no_profile_curves_is_refused() {
    let expected = vec![(
        "the sketch has no curves to make a profile of".to_owned(),
        Vec::<String>::new(),
    )];
    // An empty sketch; only a point; only construction geometry.
    let d = Draw::new();
    assert_eq!(named(&d.refused()), expected);
    let mut d = Draw::new();
    d.point(1.0, 2.0);
    assert_eq!(named(&d.refused()), expected);
    let mut d = Draw::new();
    let a = d.point(0.0, 0.0);
    let b = d.point(1.0, 0.0);
    d.construction_line(&a, &b);
    d.circle(0.0, 0.0, 1.0);
    d.sketch.circles[0].construction = true;
    let e = d.refused();
    assert_eq!(named(&e), expected);
    assert_eq!(e[0].elements_total(), 0);
}

#[test]
fn three_sides_of_a_rectangle_are_open_at_both_ends() {
    let mut d = Draw::new();
    let a = d.point(0.0, 0.0);
    let b = d.point(4.0, 0.0);
    let c = d.point(4.0, 3.0);
    let e = d.point(0.0, 3.0);
    let ab = d.line(&a, &b);
    d.line(&b, &c);
    let ce = d.line(&c, &e);
    assert_eq!(
        named(&d.refused()),
        vec![
            (
                format!(
                    "point {a} ends line {ab} and no other profile curve, so the profile is open there"
                ),
                vec![a.clone(), ab.clone()]
            ),
            (
                format!(
                    "point {e} ends line {ce} and no other profile curve, so the profile is open there"
                ),
                vec![e.clone(), ce.clone()]
            ),
        ]
    );
}

/// The 4 × 3 rectangle with its bottom split at (2, 0): its points and lines.
fn split_rectangle(d: &mut Draw) -> (Vec<String>, Vec<String>) {
    let p = vec![
        d.point(0.0, 0.0),
        d.point(2.0, 0.0),
        d.point(4.0, 0.0),
        d.point(4.0, 3.0),
        d.point(0.0, 3.0),
    ];
    let n = (0..5)
        .map(|i| d.line(&p[i].clone(), &p[(i + 1) % 5].clone()))
        .collect();
    (p, n)
}

#[test]
fn a_line_into_the_rectangle_from_a_point_of_its_outline_is_open_and_three_curves_meet() {
    let mut d = Draw::new();
    let (p, n) = split_rectangle(&mut d);
    let free = d.point(2.0, 1.5);
    let stem = d.line(&p[1], &free);
    assert_eq!(
        named(&d.refused()),
        vec![
            (
                format!(
                    "point {free} ends line {stem} and no other profile curve, so the profile is open there"
                ),
                vec![free.clone(), stem.clone()]
            ),
            (
                format!(
                    "3 curve ends meet at point {}: a profile's points each join two curves",
                    p[1]
                ),
                vec![p[1].clone(), n[0].clone(), n[1].clone(), stem.clone()]
            ),
        ]
    );
}

#[test]
fn a_line_across_the_rectangle_from_side_to_side_is_open_and_its_ends_are_on_the_sides() {
    let mut d = Draw::new();
    let n = d.rectangle(0.0, 0.0, 4.0, 3.0);
    let (e, f) = (d.point(1.0, 0.0), d.point(1.0, 3.0));
    let across = d.line(&e, &f);
    let open = |p: &str| {
        (
            format!(
                "point {p} ends line {across} and no other profile curve, so the profile is open there"
            ),
            vec![p.to_owned(), across.clone()],
        )
    };
    let on = |p: &str, side: &str| {
        (
            format!("point {p}, the end of line {across}, is 0 mm from line {side}: {RULE}"),
            vec![across.clone(), side.to_owned(), p.to_owned()],
        )
    };
    let errors = d.refused();
    assert_eq!(
        named(&errors),
        vec![open(&e), open(&f), on(&e, &n[0]), on(&f, &n[2])]
    );
    assert_eq!(errors[2].elements_total(), 3);
    assert_eq!(
        errors[2].suggestion(),
        Some(
            format!(
                "move point {e} more than 1e-7 mm from {}, or redraw so they share a point",
                n[0]
            )
            .as_str()
        )
    );
}

#[test]
fn two_loops_sharing_a_line_have_two_points_where_three_curves_meet() {
    // θ: the rectangle split at (2, 0) and (2, 3), joined by a line.
    let mut d = Draw::new();
    let p = [
        d.point(0.0, 0.0),
        d.point(2.0, 0.0),
        d.point(4.0, 0.0),
        d.point(4.0, 3.0),
        d.point(2.0, 3.0),
        d.point(0.0, 3.0),
    ];
    let n: Vec<String> = (0..6).map(|i| d.line(&p[i], &p[(i + 1) % 6])).collect();
    let middle = d.line(&p[1], &p[4]);
    assert_eq!(
        named(&d.refused()),
        vec![
            (
                format!(
                    "3 curve ends meet at point {}: a profile's points each join two curves",
                    p[1]
                ),
                vec![p[1].clone(), n[0].clone(), n[1].clone(), middle.clone()]
            ),
            (
                format!(
                    "3 curve ends meet at point {}: a profile's points each join two curves",
                    p[4]
                ),
                vec![p[4].clone(), n[3].clone(), n[4].clone(), middle.clone()]
            ),
        ]
    );
}

#[test]
fn outlines_meeting_at_a_point_are_refused_there() {
    // A figure 8 (two triangles at a point), two squares at a corner, a triangular hole at
    // the square's corner, two square holes sharing a corner: four curves at one point,
    // and nothing else.
    let figure_8 = |d: &mut Draw| {
        let w = d.point(0.0, 0.0);
        let t = [
            d.point(2.0, -1.0),
            d.point(2.0, 1.0),
            d.point(-2.0, 1.0),
            d.point(-2.0, -1.0),
        ];
        let n = [
            d.line(&w, &t[0]),
            d.line(&t[0], &t[1]),
            d.line(&t[1], &w),
            d.line(&w, &t[2]),
            d.line(&t[2], &t[3]),
            d.line(&t[3], &w),
        ];
        (
            w,
            vec![n[0].clone(), n[2].clone(), n[3].clone(), n[5].clone()],
        )
    };
    let corners = |d: &mut Draw| {
        let w = d.point(1.0, 1.0);
        let a = [d.point(0.0, 0.0), d.point(1.0, 0.0), d.point(0.0, 1.0)];
        let b = [d.point(2.0, 1.0), d.point(2.0, 2.0), d.point(1.0, 2.0)];
        let n = [
            d.line(&a[0], &a[1]),
            d.line(&a[1], &w),
            d.line(&w, &a[2]),
            d.line(&a[2], &a[0]),
            d.line(&w, &b[0]),
            d.line(&b[0], &b[1]),
            d.line(&b[1], &b[2]),
            d.line(&b[2], &w),
        ];
        (
            w,
            vec![n[1].clone(), n[2].clone(), n[4].clone(), n[7].clone()],
        )
    };
    let hole_at_corner = |d: &mut Draw| {
        let w = d.point(0.0, 0.0);
        let s = [d.point(10.0, 0.0), d.point(10.0, 10.0), d.point(0.0, 10.0)];
        let t = [d.point(5.0, 1.0), d.point(1.0, 5.0)];
        let n = [
            d.line(&w, &s[0]),
            d.line(&s[0], &s[1]),
            d.line(&s[1], &s[2]),
            d.line(&s[2], &w),
            d.line(&w, &t[0]),
            d.line(&t[0], &t[1]),
            d.line(&t[1], &w),
        ];
        (
            w,
            vec![n[0].clone(), n[3].clone(), n[4].clone(), n[6].clone()],
        )
    };
    let holes_at_corner = |d: &mut Draw| {
        d.rectangle(0.0, 0.0, 20.0, 20.0);
        let w = d.point(10.0, 10.0);
        let a = [d.point(2.0, 2.0), d.point(10.0, 2.0), d.point(2.0, 10.0)];
        let b = [
            d.point(18.0, 10.0),
            d.point(18.0, 18.0),
            d.point(10.0, 18.0),
        ];
        let n = [
            d.line(&a[0], &a[1]),
            d.line(&a[1], &w),
            d.line(&w, &a[2]),
            d.line(&a[2], &a[0]),
            d.line(&w, &b[0]),
            d.line(&b[0], &b[1]),
            d.line(&b[1], &b[2]),
            d.line(&b[2], &w),
        ];
        (
            w,
            vec![n[1].clone(), n[2].clone(), n[4].clone(), n[7].clone()],
        )
    };
    type Case = fn(&mut Draw) -> (String, Vec<String>);
    let cases: [Case; 4] = [figure_8, corners, hole_at_corner, holes_at_corner];
    for case in cases {
        let mut d = Draw::new();
        let (w, mut curves) = case(&mut d);
        curves.sort();
        assert_eq!(
            named(&d.refused()),
            vec![(
                format!("4 curve ends meet at point {w}: a profile's points each join two curves"),
                std::iter::once(w.clone()).chain(curves).collect()
            )]
        );
    }
}

#[test]
fn a_bow_tie_s_crossing_sides_are_refused() {
    let mut d = Draw::new();
    let n = d.polygon(&[(0.0, 0.0), (2.0, 2.0), (2.0, 0.0), (0.0, 2.0)]);
    let errors = d.refused();
    assert_eq!(
        named(&errors),
        vec![(
            format!("line {} and line {} cross", n[0], n[2]),
            vec![n[0].clone(), n[2].clone()]
        )]
    );
    assert_eq!(
        errors[0].suggestion(),
        Some(
            "move or remove one of them: a profile's curves meet only at the points they share, and each point joins two curves"
        )
    );
}

/// The refusals of kind "cross", as the pairs they name.
fn crossings(errors: &[silico_core::Error]) -> Vec<Vec<String>> {
    errors
        .iter()
        .filter(|e| e.message().ends_with(" cross"))
        .map(|e| e.elements().to_vec())
        .collect()
}

#[test]
fn overlapping_rectangles_are_refused_in_either_order() {
    // (0,0)–(20,10) and (10,5)–(30,15): R1's top crosses R2's left at (10, 10), R1's right
    // R2's bottom at (20, 5). FreeCAD's face depends on the order; Silico refuses both.
    for first in [true, false] {
        let mut d = Draw::new();
        let (r1, r2) = if first {
            let r1 = d.rectangle(0.0, 0.0, 20.0, 10.0);
            (r1, d.rectangle(10.0, 5.0, 30.0, 15.0))
        } else {
            let r2 = d.rectangle(10.0, 5.0, 30.0, 15.0);
            (d.rectangle(0.0, 0.0, 20.0, 10.0), r2)
        };
        let errors = d.refused();
        let mut expected = vec![
            vec![r1[2].clone(), r2[3].clone()],
            vec![r1[1].clone(), r2[0].clone()],
        ];
        for pair in &mut expected {
            pair.sort();
        }
        expected.sort();
        assert_eq!(errors.len(), 2, "{:?}", named(&errors));
        assert_eq!(crossings(&errors), expected);
    }
}

#[test]
fn a_bar_across_the_side_of_a_square_is_refused_wherever_it_starts() {
    for start_inside in [true, false] {
        let mut d = Draw::new();
        let square = d.rectangle(0.0, 0.0, 10.0, 10.0);
        let bar = if start_inside {
            d.rectangle(5.0, 4.0, 15.0, 6.0)
        } else {
            d.polygon(&[(15.0, 4.0), (15.0, 6.0), (5.0, 6.0), (5.0, 4.0)])
        };
        let (bottom, top) = if start_inside {
            (&bar[0], &bar[2])
        } else {
            (&bar[3], &bar[1])
        };
        let mut expected = vec![
            vec![square[1].clone(), bottom.clone()],
            vec![square[1].clone(), top.clone()],
        ];
        for pair in &mut expected {
            pair.sort();
        }
        expected.sort();
        let errors = d.refused();
        assert_eq!(crossings(&errors), expected);
        assert_eq!(errors.len(), 2);
    }
}

#[test]
fn crossing_circles_are_refused() {
    let mut d = Draw::new();
    let a = d.circle(0.0, 0.0, 10.0);
    let b = d.circle(12.0, 0.0, 5.0);
    assert_eq!(
        named(&d.refused()),
        vec![(
            format!("circle {a} and circle {b} cross"),
            vec![a.clone(), b.clone()]
        )]
    );

    // testPD_TNPSketchPadOverlapping: four circles r 25 at (±20, ±20), each crossing its
    // two neighbours (40 apart), not the one across (56.6 apart).
    let mut d = Draw::new();
    let c: Vec<String> = [(20.0, 20.0), (-20.0, 20.0), (-20.0, -20.0), (20.0, -20.0)]
        .into_iter()
        .map(|(x, y)| d.circle(x, y, 25.0))
        .collect();
    let errors = d.refused();
    assert_eq!(
        crossings(&errors),
        vec![
            vec![c[0].clone(), c[1].clone()],
            vec![c[0].clone(), c[3].clone()],
            vec![c[1].clone(), c[2].clone()],
            vec![c[2].clone(), c[3].clone()],
        ]
    );
    assert_eq!(errors.len(), 4);
}

#[test]
fn tangent_curves_that_share_no_point_are_0_mm_apart() {
    // makeElementFaceNull/FromFace: a 3 × 2 rectangle, a hole r 1 at its centre, tangent
    // to both long sides.
    let mut d = Draw::new();
    let n = d.rectangle(0.0, 0.0, 3.0, 2.0);
    let c = d.circle(1.5, 1.0, 1.0);
    let errors = d.refused();
    let apart = |side: &str| {
        (
            format!("circle {c} and line {side} are 0 mm apart: {RULE}"),
            vec![c.clone(), side.to_owned()],
        )
    };
    assert_eq!(named(&errors), vec![apart(&n[0]), apart(&n[2])]);
    assert_eq!(
        errors[0].suggestion(),
        Some("move them more than 1e-7 mm apart")
    );

    // A circle r 5 in a 10 × 10 square, touching all four sides.
    let mut d = Draw::new();
    let n = d.rectangle(0.0, 0.0, 10.0, 10.0);
    let c = d.circle(5.0, 5.0, 5.0);
    let expected: Vec<Vec<String>> = n.iter().map(|side| vec![c.clone(), side.clone()]).collect();
    let errors = d.refused();
    assert_eq!(
        errors
            .iter()
            .map(|e| e.elements().to_vec())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(
        errors
            .iter()
            .all(|e| e.message().contains(" are 0 mm apart: "))
    );

    // testPD_TNPSketchPadTouching: four circles r 20 at (±20, ±20), each touching two.
    let mut d = Draw::new();
    for (x, y) in [(20.0, 20.0), (-20.0, 20.0), (-20.0, -20.0), (20.0, -20.0)] {
        d.circle(x, y, 20.0);
    }
    let errors = d.refused();
    assert_eq!(errors.len(), 4);
    assert!(
        errors
            .iter()
            .all(|e| e.message().contains(" are 0 mm apart: "))
    );

    // A circle in a circular hole, tangent to it inside at (25, 15).
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 30.0, 30.0);
    let hole = d.circle(15.0, 15.0, 10.0);
    let island = d.circle(20.0, 15.0, 5.0);
    assert_eq!(
        named(&d.refused()),
        vec![(
            format!("circle {hole} and circle {island} are 0 mm apart: {RULE}"),
            vec![hole.clone(), island.clone()]
        )]
    );
}

#[test]
fn a_hole_with_a_point_on_a_side_is_refused_naming_the_point() {
    // A triangular hole whose first point, then whose last, is on the square's bottom; and
    // one whose point is on another hole's side.
    for corners in [
        [(5.0, 0.0), (7.0, 3.0), (3.0, 3.0)],
        [(7.0, 3.0), (3.0, 3.0), (5.0, 0.0)],
    ] {
        let mut d = Draw::new();
        let square = d.rectangle(0.0, 0.0, 10.0, 10.0);
        let tri = d.polygon(&corners);
        let on = d
            .sketch
            .points
            .iter()
            .find(|p| p.y.mm() == 0.0 && p.x.mm() == 5.0)
            .unwrap()
            .id
            .to_string();
        let ending: Vec<&String> = tri
            .iter()
            .filter(|l| {
                d.sketch.lines.iter().any(|x| {
                    x.id.as_str() == l.as_str() && (x.from.as_str() == on || x.to.as_str() == on)
                })
            })
            .collect();
        let errors = d.refused();
        let expected: Vec<(String, Vec<String>)> = ending
            .iter()
            .map(|l| {
                (
                    format!(
                        "point {on}, the end of line {l}, is 0 mm from line {}: {RULE}",
                        square[0]
                    ),
                    vec![(*l).clone(), square[0].clone(), on.clone()],
                )
            })
            .collect();
        assert_eq!(named(&errors), expected);
        assert!(errors.iter().all(|e| e.elements_total() == 3));
    }

    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 20.0, 20.0);
    let hole = d.rectangle(2.0, 2.0, 10.0, 10.0);
    let tri = d.polygon(&[(10.0, 5.0), (15.0, 3.0), (15.0, 8.0)]);
    let errors = d.refused();
    assert_eq!(
        errors
            .iter()
            .map(|e| e.elements().to_vec())
            .collect::<Vec<_>>(),
        vec![
            vec![tri[0].clone(), hole[1].clone(), "p0000017".to_owned()],
            vec![tri[2].clone(), hole[1].clone(), "p0000017".to_owned()],
        ]
    );
}

#[test]
fn curves_along_each_other_overlap() {
    // A circle drawn twice, about one point and about two at one place.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let a = d.circle_about(&o, 3.0);
    let b = d.circle_about(&o, 3.0);
    let errors = d.refused();
    assert_eq!(
        named(&errors),
        vec![(
            format!("circle {a} and circle {b} overlap"),
            vec![a.clone(), b.clone()]
        )]
    );
    let mut d = Draw::new();
    d.circle(1.0, 1.0, 3.0);
    d.circle(1.0, 1.0, 3.0);
    assert!(d.refused()[0].message().ends_with(" overlap"));

    // Two lines between the same two points, alone and inside a 10 × 10 square.
    for in_square in [false, true] {
        let mut d = Draw::new();
        if in_square {
            d.rectangle(-5.0, -5.0, 5.0, 5.0);
        }
        let p = d.point(0.0, 0.0);
        let q = d.point(1.0, 2.0);
        let a = d.line(&p, &q);
        let b = d.line(&q, &p);
        assert_eq!(
            named(&d.refused()),
            vec![(
                format!("line {a} and line {b} overlap"),
                vec![a.clone(), b.clone()]
            )]
        );
    }

    // Two unit squares side by side, each with its own line along x = 1.
    let mut d = Draw::new();
    let left = d.rectangle(0.0, 0.0, 1.0, 1.0);
    let right = d.rectangle(1.0, 0.0, 2.0, 1.0);
    let errors = d.refused();
    assert!(named(&errors).contains(&(
        format!("line {} and line {} overlap", left[1], right[3]),
        vec![left[1].clone(), right[3].clone()]
    )));

    // Two arcs of one circle from one point, both counter-clockwise: the half from (2, 0)
    // to (−2, 0) and the quarter from (2, 0) to (0, 2), closed by a line from (−2, 0) to
    // (0, 2) (whose end is then on the half).
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(2.0, 0.0);
    let e = d.point(-2.0, 0.0);
    let q = d.point(0.0, 2.0);
    let half = d.arc(&o, &s, &e);
    let quarter = d.arc(&o, &s, &q);
    d.line(&e, &q);
    assert!(named(&d.refused()).contains(&(
        format!("arc {half} and arc {quarter} overlap"),
        vec![half.clone(), quarter.clone()]
    )));
}

#[test]
fn a_triangle_over_half_a_square_sharing_three_corners_is_refused_every_way() {
    let mut d = Draw::new();
    let p = [
        d.point(0.0, 0.0),
        d.point(10.0, 0.0),
        d.point(10.0, 10.0),
        d.point(0.0, 10.0),
    ];
    let sq: Vec<String> = (0..4).map(|i| d.line(&p[i], &p[(i + 1) % 4])).collect();
    let tri = [
        d.line(&p[0], &p[1]),
        d.line(&p[1], &p[2]),
        d.line(&p[2], &p[0]),
    ];
    let errors = d.refused();
    let got = named(&errors);
    assert_eq!(got.len(), 5, "{got:#?}");
    assert_eq!(
        got[0],
        (
            format!(
                "4 curve ends meet at point {}: a profile's points each join two curves",
                p[0]
            ),
            vec![
                p[0].clone(),
                sq[0].clone(),
                sq[3].clone(),
                tri[0].clone(),
                tri[2].clone()
            ]
        )
    );
    assert!(
        got[1]
            .0
            .starts_with(&format!("4 curve ends meet at point {}", p[1]))
    );
    assert!(
        got[2]
            .0
            .starts_with(&format!("4 curve ends meet at point {}", p[2]))
    );
    assert_eq!(
        got[3].0,
        format!("line {} and line {} overlap", sq[0], tri[0])
    );
    assert_eq!(
        got[4].0,
        format!("line {} and line {} overlap", sq[1], tri[1])
    );
}

#[test]
fn a_line_crossing_its_arcs_circle_again_within_the_arc_crosses_it() {
    // The arc r 1 from (1, 0) to (0, 1); a line from (0, 1) to (2, 0.2) meets the circle
    // again at about (0.69, 0.72), 46° round, on the arc.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let p0 = d.point(1.0, 0.0);
    let e = d.point(0.0, 1.0);
    let q = d.point(2.0, 0.2);
    let arc = d.arc(&o, &p0, &e);
    let through = d.line(&e, &q);
    d.line(&q, &p0);
    assert_eq!(
        named(&d.refused()),
        vec![(
            format!("arc {arc} and line {through} cross"),
            vec![arc.clone(), through.clone()]
        )]
    );
}

#[test]
fn a_cusp_whose_tangent_line_is_too_short_is_refused_naming_its_far_end() {
    // The cusp of a_cusp_with_a_long_tangent_line_is_a_profile with the line 1e-4 long,
    // shorter than √(2r × 1e-7) (4.5e-4): its far end, Q (1, 1e-4), is
    // √(1 + 1e-8) − 1 ≈ 5e-9 off the circle, within 1e-7 of the arc.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let p0 = d.point(1.0, 0.0);
    let e = d.point(0.0, 1.0);
    let r = d.point(2.0, 2.0);
    let q = d.point(1.0, 1e-4);
    let arc = d.arc(&o, &p0, &e);
    d.line(&e, &r);
    let rq = d.line(&r, &q);
    let qp = d.line(&q, &p0);
    let errors = d.refused();
    assert_eq!(
        errors
            .iter()
            .map(|e| e.elements().to_vec())
            .collect::<Vec<_>>(),
        vec![
            vec![rq.clone(), arc.clone(), q.clone()],
            vec![qp.clone(), arc.clone(), q.clone()]
        ]
    );
    for e in &errors {
        assert!(
            e.message()
                .starts_with(&format!("point {q}, the end of line ")),
            "{e}"
        );
        assert!(e.message().contains(" mm from arc "), "{e}");
    }
}

#[test]
fn a_loop_with_no_area_is_refused() {
    // A rectangle 10 × 1.5e-7, whose long sides are more than 1e-7 apart: 1.5e-6 mm² is
    // at most 1e-7 × its perimeter, 20 + 3e-7.
    let mut d = Draw::new();
    let n = d.rectangle(0.0, 0.0, 10.0, 1.5e-7);
    let w = 1.5e-7;
    assert!(10.0 * w <= 1e-7 * (20.0 + 2.0 * w));
    let errors = d.refused();
    assert_eq!(errors.len(), 1);
    let e = &errors[0];
    assert!(
        e.message().starts_with(&format!(
            "loop {}, {}, {}, {} encloses no area to speak of (1.5",
            n[0], n[1], n[2], n[3]
        )),
        "{e}"
    );
    assert!(
        e.message()
            .contains(" mm², against a perimeter of 20.0000003"),
        "{e}"
    );
    assert_eq!(e.elements(), n.as_slice());
    assert_eq!(
        e.suggestion(),
        Some("give it some width: the kernel builds nothing thinner than 1e-7 mm")
    );
}

#[test]
fn every_refusal_is_listed_up_to_a_hundred_and_the_rest_counted() {
    // 15 bars across, 15 down: each pair crosses four times, 900 crossings.
    let mut d = Draw::new();
    for i in 0..15 {
        let at = 10.0 * f64::from(i);
        d.rectangle(-5.0, at, 200.0, at + 1.0);
        d.rectangle(at, -5.0, at + 1.0, 200.0);
    }
    let r = d.profile().unwrap_err();
    assert_eq!(r.total(), 900);
    assert_eq!(r.errors().len(), silico_core::document::PROBLEMS_LISTED);
    // The first hundred by the pair's IDs.
    let pairs: Vec<Vec<String>> = r.errors().iter().map(|e| e.elements().to_vec()).collect();
    let mut sorted = pairs.clone();
    sorted.sort();
    assert_eq!(pairs, sorted);
    assert_eq!(pairs[0], vec!["n0000005".to_owned(), "n0000014".to_owned()]);
}

// Generated: squares and circles nested about a centre, alternately, each well inside the
// one around it, drawn either way round, a circle sometimes as two half arcs. Regions are
// every other ring: the area of shape 2k less shape 2k + 1, in closed form.

#[derive(Debug, Clone, Copy)]
enum Shape {
    Square(f64),
    Circle(f64),
}

impl Shape {
    fn area(self) -> f64 {
        match self {
            Shape::Square(s) => s * s,
            Shape::Circle(r) => PI * r * r,
        }
    }
}

fn nested(scales: &[f64]) -> Vec<Shape> {
    let mut shapes = vec![Shape::Square(100.0)];
    for (i, &f) in scales.iter().enumerate() {
        let last = shapes[i];
        shapes.push(match last {
            // A circle inside a square of side s: r < s/2.
            Shape::Square(s) => Shape::Circle(s / 2.0 * f),
            // A square inside a circle of radius r: side < r√2.
            Shape::Circle(r) => Shape::Square(r * std::f64::consts::SQRT_2 * f),
        });
    }
    shapes
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn nested_squares_and_circles_are_their_rings(
        scales in prop::collection::vec(0.3f64..0.9, 0..7),
        cx in -500.0f64..500.0,
        cy in -500.0f64..500.0,
        clockwise in prop::collection::vec(any::<bool>(), 8),
        halves in prop::collection::vec(any::<bool>(), 8),
    ) {
        let shapes = nested(&scales);
        let mut d = Draw::new();
        for (i, shape) in shapes.iter().enumerate() {
            match *shape {
                Shape::Square(s) => {
                    let h = s / 2.0;
                    if clockwise[i] {
                        d.polygon(&[(cx - h, cy - h), (cx - h, cy + h), (cx + h, cy + h), (cx + h, cy - h)]);
                    } else {
                        d.rectangle(cx - h, cy - h, cx + h, cy + h);
                    }
                }
                Shape::Circle(r) => {
                    if halves[i] {
                        let o = d.point(cx, cy);
                        let a = d.point(cx + r, cy);
                        let b = d.point(cx - r, cy);
                        d.arc(&o, &a, &b);
                        d.arc(&o, &b, &a);
                    } else {
                        d.circle(cx, cy, r);
                    }
                }
            }
        }
        let p = made(&d);
        let mut got = regions(&p);
        got.sort_by_key(|&(_, depth)| depth);
        let expected: Vec<(f64, usize)> = shapes
            .chunks(2)
            .enumerate()
            .map(|(k, pair)| {
                let hole = pair.get(1).map_or(0.0, |s| s.area());
                (pair[0].area() - hole, 2 * k)
            })
            .collect();
        prop_assert_eq!(got.len(), expected.len());
        for ((a, da), (b, db)) in got.iter().zip(&expected) {
            // Coordinates up to 550 from the origin: the shoelace sum's rounding is about
            // 1e-16 of 550², relative to an area of at least 0.04 or so.
            prop_assert!(close(*a, *b, 1e-9), "{} against {}", a, b);
            prop_assert_eq!(da, db);
        }
        for r in &p.regions {
            prop_assert!(r.holes.len() <= 1);
        }
    }
}

#[test]
fn a_feature_that_isnt_a_sketch_of_the_document_has_no_profile() {
    let d = Draw::new();
    let checked = d.checked().unwrap();
    let other = silico_core::document::FeatureId::new("zzzzzzzz").unwrap();
    let r = profile(&checked, &other, &d.solution()).unwrap_err();
    assert_eq!(r.errors()[0].code(), ErrorCode::InvalidInput);
    assert_eq!(
        r.errors()[0].message(),
        "the document has no feature zzzzzzzz"
    );

    let mut document = checked.into_document();
    document.features[0].operation = silico_core::document::Operation::Sphere {
        position: [silico_core::document::Length::from_nm(0); 3],
        radius: silico_core::document::Length::from_nm(1_000_000),
    };
    let checked = silico_core::document::Checked::new(document).unwrap();
    let r = profile(&checked, &feature(), &d.solution()).unwrap_err();
    assert_eq!(
        r.errors()[0].message(),
        format!("feature {FEATURE} isn't a sketch")
    );
    assert_eq!(r.errors()[0].feature(), Some(FEATURE));
}

#[test]
fn a_lone_line_just_longer_than_the_tolerance_is_open_at_each_end_not_told_to_share_them() {
    // Its length by hypot (the request's measure) is just above 1e-7, by a plain square
    // root of the sum of squares just at or below it: measured both ways, the line passed
    // the size rule and then its own ends were told to share the point, the line named
    // twice. Measured one way, its ends are two open ends.
    let (x, y) = (
        f64::from_bits(0x3e77c10b900b8100),
        f64::from_bits(0x3e69016c51facb34),
    );
    assert!(libm::hypot(x, y) > 1e-7);
    assert!((x * x + y * y).sqrt() <= 1e-7);
    let mut d = Draw::new();
    let p = d.point(0.0, 0.0);
    let q = d.point(x, y);
    let l = d.line(&p, &q);
    let open = |end: &str| {
        (
            format!(
                "point {end} ends line {l} and no other profile curve, so the profile is open there"
            ),
            vec![end.to_owned(), l.clone()],
        )
    };
    let errors = d.refused();
    assert_eq!(named(&errors), vec![open(&p), open(&q)]);
}

/// A 10 × 10 square whose lower right corner is rounded by an arc of radius 1 about
/// (9, 1), from (9, 0) to its end at (10 + `off`, 1): `off` from its circle, outwards.
fn corner_arc_ending_off_its_circle(d: &mut Draw, off: f64) -> [String; 4] {
    let a = d.point(0.0, 0.0);
    let b = d.point(9.0, 0.0);
    let c = d.point(10.0 + off, 1.0);
    let top = d.point(10.0, 10.0);
    let left = d.point(0.0, 10.0);
    let o = d.point(9.0, 1.0);
    d.line(&a, &b);
    let arc = d.arc(&o, &b, &c);
    d.line(&c, &top);
    d.line(&top, &left);
    d.line(&left, &a);
    [arc, o, b, c]
}

#[test]
fn an_arc_whose_end_is_off_its_circle_is_refused_naming_it_its_centre_and_its_ends() {
    // The request's check refuses an arc whose end is more than 1e-7 from where its angle
    // and radius (from its centre to its start) put it; the profile names it first. The
    // end's angle is 0, so it's off by (10 + off) − 10.
    for off in [2e-7, 1e-6, 1e-5] {
        let mut d = Draw::new();
        let named_elements = corner_arc_ending_off_its_circle(&mut d, off);
        let errors = d.refused();
        assert_eq!(errors.len(), 1, "{:#?}", named(&errors));
        let e = &errors[0];
        assert_eq!(e.elements(), named_elements.as_slice());
        assert_eq!(e.elements_total(), 4);
        assert_eq!(
            e.message(),
            format!(
                "arc {}'s end is {} mm from its circle (the circle about its centre through its start): a profile's arcs end within 1e-7 mm of their circles",
                named_elements[0],
                silico_protocol::format_number((10.0 + off) - 10.0)
            )
        );
        assert_eq!(
            e.suggestion(),
            Some("solve the sketch again, or put the arc's end on its circle")
        );
    }
    // 5e-8 off is within the tolerance: made, and the request takes it.
    let mut d = Draw::new();
    corner_arc_ending_off_its_circle(&mut d, 5e-8);
    let p = made(&d);
    silico_core::kernel::KernelRequest::make_faces(&p.faces()).unwrap();
}

#[test]
fn overlapping_arcs_across_the_circles_seam_at_pi_overlap() {
    // Two arcs of the circle of radius 25 about one centre, their points integers on it
    // (so both radii are exactly 25): from (−20, 15), at 143°, round past π to
    // (−20, −15), at 217°; and from (−24, −7), at −163.7° (its start angle just past −π),
    // to (−15, −20), at −126.9°. They share 196.3° to 217°.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let (s1, e1) = (d.point(-20.0, 15.0), d.point(-20.0, -15.0));
    let (s2, e2) = (d.point(-24.0, -7.0), d.point(-15.0, -20.0));
    let a1 = d.arc(&o, &s1, &e1);
    let a2 = d.arc(&o, &s2, &e2);
    let got = named(&d.refused());
    assert!(
        got.contains(&(
            format!("arc {a1} and arc {a2} overlap"),
            vec![a1.clone(), a2.clone()]
        )),
        "{got:#?}"
    );
}

#[test]
fn a_sliver_loop_among_a_regions_holes_is_refused_as_a_loop() {
    // A 20 × 20 square with a circle hole, and a 6 × 1.5e-7 rectangle hole: the
    // rectangle is the sliver, named by its lines; the region isn't refused as well.
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 20.0, 20.0);
    d.circle(5.0, 5.0, 2.0);
    let thin = d.rectangle(12.0, 10.0, 18.0, 10.0 + 1.5e-7);
    let errors = d.refused();
    assert_eq!(errors.len(), 1, "{:#?}", named(&errors));
    assert!(errors[0].message().starts_with("loop "), "{}", errors[0]);
    let mut got = errors[0].elements().to_vec();
    got.sort();
    assert_eq!(got, thin);
}

#[test]
fn open_ends_grouped_with_both_ends_of_one_line_name_each_curve_once_and_say_lengthen_it() {
    // L1 from p (0, 0) to q (1.5e-7, 0), longer than the tolerance; L2 from r (0.75e-7, 0)
    // to (5, 5). r is within 1e-7 of both p and q, so the three open ends are one group,
    // though p and q aren't within it of each other. L1 has both its ends in the group:
    // sharing one point would shrink it to nothing, so the advice is to lengthen it.
    let mut d = Draw::new();
    let p = d.point(0.0, 0.0);
    let r = d.point(0.75e-7, 0.0);
    let q = d.point(1.5e-7, 0.0);
    let s = d.point(5.0, 5.0);
    let l1 = d.line(&p, &q);
    let l2 = d.line(&r, &s);
    let errors = d.refused();
    assert_eq!(
        named(&errors),
        vec![
            (
                format!(
                    "points {p}, {r}, {q} end curves within 1e-7 mm of each other, but aren't one point, so the profile is open there"
                ),
                vec![p.clone(), r.clone(), q.clone(), l1.clone(), l2.clone()]
            ),
            (
                format!(
                    "point {s} ends line {l2} and no other profile curve, so the profile is open there"
                ),
                vec![s.clone(), l2.clone()]
            ),
        ]
    );
    assert_eq!(errors[0].elements_total(), 5);
    assert_eq!(
        errors[0].suggestion(),
        Some(format!("lengthen line {l1}, or end the curves at one point").as_str())
    );
}

#[test]
fn an_arc_with_both_ends_in_an_open_group_is_to_be_lengthened() {
    // An arc of radius 10 about the origin, counter-clockwise from s (10, 0) round to e at
    // the angle −1.5e-8, so its ends are 1.5e-7 apart; a line from r (10, −0.75e-7),
    // within 1e-7 of both ends, out to (20, 20). The arc has both its ends in the group,
    // so it's the arc that's to be lengthened, as a line would be.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let delta: f64 = 1.5e-8;
    let s = d.point(10.0, 0.0);
    let e = d.point(10.0 * libm::cos(delta), -10.0 * libm::sin(delta));
    let r = d.point(10.0, -0.75e-7);
    let far = d.point(20.0, 20.0);
    let a = d.arc(&o, &s, &e);
    let l = d.line(&r, &far);
    let errors = d.refused();
    assert_eq!(
        named(&errors),
        vec![
            (
                format!(
                    "points {s}, {e}, {r} end curves within 1e-7 mm of each other, but aren't one point, so the profile is open there"
                ),
                vec![s.clone(), e.clone(), r.clone(), a.clone(), l.clone()]
            ),
            (
                format!(
                    "point {far} ends line {l} and no other profile curve, so the profile is open there"
                ),
                vec![far.clone(), l.clone()]
            ),
        ]
    );
    assert_eq!(
        errors[0].suggestion(),
        Some(format!("lengthen arc {a}, or end the curves at one point").as_str())
    );
}

#[test]
fn two_lines_with_both_ends_in_one_open_group_are_both_to_be_lengthened() {
    // L1 from p (0, 0) to q (1.5e-7, 0), and L3 0.9e-7 above it, from a (0, 0.9e-7) to
    // b (1.5e-7, 0.9e-7); L2 from r (0.75e-7, 0.45e-7), within 1e-7 of all four, out to
    // (−5, 5). The five ends are one group, and L1 and L3 each have both ends in it: both
    // are named, once each. L1 and L3 are also too close.
    let mut d = Draw::new();
    let p = d.point(0.0, 0.0);
    let q = d.point(1.5e-7, 0.0);
    let a = d.point(0.0, 0.9e-7);
    let b = d.point(1.5e-7, 0.9e-7);
    let r = d.point(0.75e-7, 0.45e-7);
    let far = d.point(-5.0, 5.0);
    let l1 = d.line(&p, &q);
    let l2 = d.line(&r, &far);
    let l3 = d.line(&a, &b);
    let errors = d.refused();
    assert_eq!(
        named(&errors),
        vec![
            (
                format!(
                    "points {p}, {q}, {a}, {b}, {r} end curves within 1e-7 mm of each other, but aren't one point, so the profile is open there"
                ),
                vec![
                    p.clone(),
                    q.clone(),
                    a.clone(),
                    b.clone(),
                    r.clone(),
                    l1.clone(),
                    l3.clone(),
                    l2.clone()
                ]
            ),
            (
                format!(
                    "point {far} ends line {l2} and no other profile curve, so the profile is open there"
                ),
                vec![far.clone(), l2.clone()]
            ),
            (
                format!(
                    "line {l1} and line {l3} are 9e-8 mm apart: a profile's curves must be more than 1e-7 mm apart except at a point they share"
                ),
                vec![l1.clone(), l3.clone()]
            ),
        ]
    );
    assert_eq!(
        errors[0].suggestion(),
        Some(format!("lengthen line {l1} and line {l3}, or end the curves at one point").as_str())
    );
}
