#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/profile.rs"]
mod draw;

use std::f64::consts::PI;

use draw::{Draw, close};

#[test]
fn a_hole_inside_an_arc_past_both_its_top_and_bottom_is_a_hole() {
    // A D: the arc r 10 about the origin from (6, 8) counter-clockwise past the top, the
    // left, and the bottom to (6, -8), closed by the chord x = 6; a circle r 0.5 about
    // (0, -9), inside it, below the chord's lower end. Area by hand: the circle's segment
    // beyond the chord, (r^2/2)(theta - sin theta) with theta = 2pi - 2 atan2(8, 6) and
    // sin theta = -2(0.8)(0.6), less the hole.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(6.0, 8.0);
    let e = d.point(6.0, -8.0);
    d.arc(&o, &s, &e);
    d.line(&e, &s);
    d.circle(0.0, -9.0, 0.5);
    let p = d.made();
    assert_eq!(p.regions.len(), 1, "{p:#?}");
    assert_eq!(p.regions[0].holes.len(), 1);
    let theta = 2.0 * PI - 2.0 * libm::atan2(8.0, 6.0);
    let want = 50.0 * (theta + 0.96) - PI / 4.0;
    assert!(
        close(p.regions[0].area(), want, 1e-12),
        "{}",
        p.regions[0].area()
    );
}

#[test]
fn a_line_drawn_towards_the_point_it_shares_with_an_arc_still_crosses_it() {
    // a_line_crossing_its_arcs_circle_again_within_the_arc_crosses_it with the line drawn
    // the other way, from (2, 0.2) to the shared (0, 1): it meets the circle again at
    // about (0.69, 0.72), on the arc.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let p0 = d.point(1.0, 0.0);
    let e = d.point(0.0, 1.0);
    let q = d.point(2.0, 0.2);
    let arc = d.arc(&o, &p0, &e);
    let through = d.line(&q, &e);
    d.line(&q, &p0);
    let errors = d.refused();
    assert!(
        draw::named(&errors).contains(&(
            format!("arc {arc} and line {through} cross"),
            vec![arc.clone(), through.clone()]
        )),
        "{:#?}",
        draw::named(&errors)
    );
}

#[test]
fn two_arcs_from_one_point_that_cross_again_are_refused() {
    // Arc A about (0, 0), r 5, from (0, -5) to (3, 4); arc B about (6, 0), r 5, from
    // (3, 4) to (6, -5); the line (6, -5)-(0, -5), tangent to both circles at its ends,
    // closes the loop. The circles meet again at (3, -4), at -53.13 deg on A (within
    // -90..53.13) and 233.13 deg on B (within 126.87..270): the arcs cross there.
    let mut d = Draw::new();
    let c1 = d.point(0.0, 0.0);
    let c2 = d.point(6.0, 0.0);
    let a0 = d.point(0.0, -5.0);
    let p = d.point(3.0, 4.0);
    let b1 = d.point(6.0, -5.0);
    let a = d.arc(&c1, &a0, &p);
    let b = d.arc(&c2, &p, &b1);
    d.line(&b1, &a0);
    let errors = d.refused();
    assert_eq!(
        draw::named(&errors),
        vec![(
            format!("arc {a} and arc {b} cross"),
            vec![a.clone(), b.clone()]
        )]
    );
}

#[test]
fn a_circle_tangent_to_a_sides_line_beyond_the_side_is_apart_from_it() {
    // A 10 x 1 bar, and a circle r 5 about (12, 6): tangent to the line y = 1 of the bar's
    // top at (12, 1), 2 past the side's end; its nearest to the bar is the corner (10, 1),
    // sqrt(29) - 5 = 0.385 away. Their boxes overlap, so the pair is compared.
    let mut d = Draw::new();
    d.rectangle(0.0, 0.0, 10.0, 1.0);
    d.circle(12.0, 6.0, 5.0);
    let p = d.made();
    assert_eq!(p.regions.len(), 2);
}

#[test]
fn which_hundred_crossings_are_listed_doesnt_depend_on_where_the_curves_are() {
    // every_refusal_is_listed_up_to_a_hundred_and_the_rest_counted's 900 crossings, and
    // the same sketch mirrored in x (the same IDs, drawn in the same order): the first
    // hundred by ID are the same pairs, wherever the sweep meets them.
    let grid = |sign: f64| {
        let mut d = Draw::new();
        for i in 0..15 {
            let at = 10.0 * f64::from(i);
            d.rectangle(sign * -5.0, at, sign * 200.0, at + 1.0);
            d.rectangle(sign * at, -5.0, sign * (at + 1.0), 200.0);
        }
        let r = d.profile().unwrap_err();
        assert_eq!(r.total(), 900);
        r.errors()
            .iter()
            .map(|e| e.elements().to_vec())
            .collect::<Vec<_>>()
    };
    assert_eq!(grid(1.0), grid(-1.0));
}

#[test]
fn open_ends_within_the_tolerance_across_x_share_the_point() {
    // open_ends_exactly_1e_7_apart_share_the_point's rectangle with its fourth side ending
    // 5e-8 to the left of the corner instead of above it.
    let mut d = Draw::new();
    let p = [
        d.point(0.0, 0.0),
        d.point(4.0, 0.0),
        d.point(4.0, 3.0),
        d.point(0.0, 3.0),
        d.point(-5e-8, 0.0),
    ];
    let n = [
        d.line(&p[0], &p[1]),
        d.line(&p[1], &p[2]),
        d.line(&p[2], &p[3]),
        d.line(&p[3], &p[4]),
    ];
    assert_eq!(
        draw::named(&d.refused()),
        vec![(
            format!(
                "points {}, {} end curves within 1e-7 mm of each other, but aren't one point, so the profile is open there",
                p[0], p[4]
            ),
            vec![p[0].clone(), p[4].clone(), n[0].clone(), n[3].clone()]
        )]
    );
}

#[test]
fn arcs_that_cross_at_one_of_their_circles_two_crossings_are_refused() {
    // D1: the lower half of the circle r 5 about (0, 0), from (-5, 0) to (5, 0), and its
    // chord. D2: the quarter of the circle r 5 about (6, -0.5) from (1, -0.5) to
    // (6, -5.5), and its chord. The circles meet near (3.3, -3.8), on both arcs, and near
    // (2.7, 4.2), on neither.
    let mut d = Draw::new();
    let o1 = d.point(0.0, 0.0);
    let w = d.point(-5.0, 0.0);
    let e = d.point(5.0, 0.0);
    let a1 = d.arc(&o1, &w, &e);
    d.line(&e, &w);
    let o2 = d.point(6.0, -0.5);
    let s2 = d.point(1.0, -0.5);
    let e2 = d.point(6.0, -5.5);
    let a2 = d.arc(&o2, &s2, &e2);
    d.line(&e2, &s2);
    let errors = d.refused();
    assert!(
        draw::named(&errors).contains(&(
            format!("arc {a1} and arc {a2} cross"),
            vec![a1.clone(), a2.clone()]
        )),
        "{:#?}",
        draw::named(&errors)
    );
}

#[test]
fn a_line_that_crosses_a_circle_once_from_outside_is_refused() {
    // A circle r 5 about the origin; a triangle (0, 8), (0, 3), (2, 8): its first side
    // runs from outside in, crossing at (0, 5), its second from inside out.
    let mut d = Draw::new();
    let c = d.circle(0.0, 0.0, 5.0);
    let n = d.polygon(&[(0.0, 8.0), (0.0, 3.0), (2.0, 8.0)]);
    let errors = d.refused();
    let mut pairs: Vec<Vec<String>> = errors.iter().map(|e| e.elements().to_vec()).collect();
    pairs.sort();
    let mut want = vec![vec![c.clone(), n[0].clone()], vec![c.clone(), n[1].clone()]];
    for p in &mut want {
        p.sort();
    }
    want.sort();
    assert_eq!(pairs, want, "{:#?}", draw::named(&errors));
}
