//! M3.5's geometric predicates at their harder cases (round 1 of the review's proof tests):
//! a hole nested inside an arc that passes both its circle's top and bottom, lines and arcs
//! from a shared point that cross again, arcs crossing at one of their circles' two
//! crossings, a line crossing a circle once from outside, a circle tangent to a side's line
//! beyond the side, open ends grouped across x, and which hundred crossings are listed.
//! Each failed on a mutant of the code before it was added; expected values are by hand.
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

// Round 1 of the CI mutants: each test below failed on a surviving mutant of the code.

/// The rule every "apart" and end message ends with.
const RULE: &str =
    "a profile's curves must be more than 1e-7 mm apart except at a point they share";

/// The circle r 1 about the origin, and a line inside it along the x axis from `from` to
/// `to`, its points drawn in the order `drawn` gives (`true`: `to`'s point first). An end
/// at (±x, 0) is 1 − x from the circle. The line's ends each end it alone (open), so
/// they're refused too; the pair's meeting is the one end nearest the circle.
fn chord(from: f64, to: f64, to_first: bool) -> (Vec<(String, Vec<String>)>, [String; 4]) {
    let mut d = Draw::new();
    let c = d.circle(0.0, 0.0, 1.0);
    let (p, q) = if to_first {
        let q = d.point(to, 0.0);
        (d.point(from, 0.0), q)
    } else {
        let p = d.point(from, 0.0);
        (p, d.point(to, 0.0))
    };
    let n = d.line(&p, &q);
    (draw::named(&d.refused()), [c, n, p, q])
}

/// The one refusal naming the circle and the line, of [`chord`]'s.
fn end_named<'a>(
    errors: &'a [(String, Vec<String>)],
    [c, n, ..]: &[String; 4],
) -> &'a (String, Vec<String>) {
    let mut of_pair = errors
        .iter()
        .filter(|(_, elements)| elements.contains(c) && elements.contains(n));
    let found = of_pair.next().unwrap();
    assert!(of_pair.next().is_none(), "{errors:#?}");
    found
}

#[test]
fn of_two_ends_near_a_curve_the_nearer_is_named_though_found_second() {
    // The line from (−(1 − 2⁻²⁴), 0) to (1 − 2⁻²⁵, 0): its start 2⁻²⁴ (6e-8) from the
    // circle, its end 2⁻²⁵ (3e-8), both within 1e-7. The end is the nearer.
    let near = 1.0 - libm::exp2(-25.0);
    let far = 1.0 - libm::exp2(-24.0);
    let (errors, names) = chord(-far, near, false);
    let [c, n, _, q] = &names;
    let (message, elements) = end_named(&errors, &names);
    assert!(
        message.starts_with(&format!("point {q}, the end of line {n}, is 2.98")),
        "{message}"
    );
    assert!(message.ends_with(&format!("e-8 mm from circle {c}: {RULE}")));
    assert_eq!(elements, &vec![n.clone(), c.clone(), q.clone()]);
}

#[test]
fn of_two_ends_near_a_curve_a_farther_one_first_by_id_isnt_named() {
    // The line from (−(1 − 2⁻²⁵), 0), 3e-8 from the circle, to (1 − 2⁻²⁴, 0), 6e-8, its
    // end's point drawn (so numbered) first: the nearer, its start, is named.
    let near = 1.0 - libm::exp2(-25.0);
    let far = 1.0 - libm::exp2(-24.0);
    let (errors, names) = chord(-near, far, true);
    let [c, n, p, _] = &names;
    let (message, elements) = end_named(&errors, &names);
    assert!(
        message.starts_with(&format!("point {p}, the end of line {n}, is 2.98")),
        "{message}"
    );
    assert_eq!(elements, &vec![n.clone(), c.clone(), p.clone()]);
}

#[test]
fn of_two_ends_as_near_a_curve_the_first_by_id_is_named() {
    // The line from (−(1 − 2⁻²⁴), 0) to (1 − 2⁻²⁴, 0): both ends exactly 2⁻²⁴ from the
    // circle (|±x| is x). The first by ID is named, whichever end is found first.
    let x = 1.0 - libm::exp2(-24.0);
    for to_first in [false, true] {
        let (errors, names) = chord(-x, x, to_first);
        let [c, n, p, q] = &names;
        let first = if to_first { q } else { p };
        let (message, elements) = end_named(&errors, &names);
        assert!(
            message.starts_with(&format!("point {first}, the end of line {n}, is 5.96")),
            "{to_first}: {message}"
        );
        assert_eq!(elements, &vec![n.clone(), c.clone(), first.clone()]);
    }
}

#[test]
fn a_point_on_an_arcs_circle_beyond_its_end_is_apart_from_it() {
    // The arc r 5 about the origin from (5, 0) counter-clockwise to (−3, 4), 126.87°, and
    // its chord; a triangle (−4, 3), (−4, 6), (−8, 3) outside the circle, its corner
    // (−4, 3) on the circle at 143.13°, past the arc's end: √2 from it, so the two are
    // apart. Areas by hand: the segment (r²/2)(θ − sin θ), θ = atan2(4, −3), sin θ = 0.8;
    // the triangle ½·3·4.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(5.0, 0.0);
    let e = d.point(-3.0, 4.0);
    d.arc(&o, &s, &e);
    d.line(&e, &s);
    d.polygon(&[(-4.0, 3.0), (-4.0, 6.0), (-8.0, 3.0)]);
    let p = d.made();
    let mut areas: Vec<f64> = p.regions.iter().map(|r| r.area()).collect();
    areas.sort_by(f64::total_cmp);
    let theta = libm::atan2(4.0, -3.0);
    let segment = 12.5 * (theta - 0.8);
    assert_eq!(areas.len(), 2, "{p:#?}");
    assert!(close(areas[0], 6.0, 1e-12), "{areas:?}");
    assert!(
        close(areas[1], segment, 1e-12),
        "{areas:?} against {segment}"
    );
}

#[test]
fn a_point_on_a_major_arc_where_it_turns_back_towards_its_start_is_on_it() {
    // The arc r 5 about the origin from (5, 0) counter-clockwise to (−3, −4), 233.13°, and
    // its chord; a triangle (−4, −3), (−8, −3), (−4, −6) outside the circle, its corner
    // (−4, −3) on the arc at 216.87°: 0 from it, past the half turn from the arc's start.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(5.0, 0.0);
    let e = d.point(-3.0, -4.0);
    let a = d.arc(&o, &s, &e);
    d.line(&e, &s);
    let corner = d.point(-4.0, -3.0);
    let (left, low) = (d.point(-8.0, -3.0), d.point(-4.0, -6.0));
    let sides = [
        d.line(&corner, &left),
        d.line(&left, &low),
        d.line(&low, &corner),
    ];
    let errors = draw::named(&d.refused());
    for side in [&sides[0], &sides[2]] {
        assert!(
            errors.contains(&(
                format!("point {corner}, the end of line {side}, is 0 mm from arc {a}: {RULE}"),
                vec![side.clone(), a.clone(), corner.to_owned()]
            )),
            "{errors:#?}"
        );
    }
}

#[test]
fn a_point_at_an_arcs_centre_is_as_far_from_it_as_its_nearer_end() {
    // A tiny arc about the origin from (1.5e-7, 0) to (−7.5e-8, 0): r 1.5e-7, from its
    // start, its end 7.5e-8 off its circle (within 1e-7, so the arc is built), so its end
    // is 7.5e-8 from its centre. A line from the centre's point up to (0, 5), and one from
    // the arc's end down to (−7.5e-8, −5), so neither point is open. The centre is 7.5e-8
    // from the arc, at its end, not its radius away: the line's end there is named, as
    // near as the arc's end is to the line, and first by ID.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(1.5e-7, 0.0);
    let e = d.point(-7.5e-8, 0.0);
    let a = d.arc(&o, &s, &e);
    let up = d.point(0.0, 5.0);
    let n = d.line(&o, &up);
    let down = d.point(-7.5e-8, -5.0);
    d.line(&e, &down);
    let errors = draw::named(&d.refused());
    assert!(
        errors.contains(&(
            format!("point {o}, the end of line {n}, is 7.5e-8 mm from arc {a}: {RULE}"),
            vec![n.clone(), a.clone(), o.clone()]
        )),
        "{errors:#?}"
    );
}

#[test]
fn a_short_line_near_a_long_one_is_measured_off_the_long_ones_line() {
    // A 2-long line from (2, 2e-8) to (4, 6e-8), the bottom of a quadrilateral up to y = 5,
    // drawn first; the 8 × 5 rectangle (0, −5)–(8, 0) below, its top from (8, 0) to (0, 0). Off
    // the long top's line the short line's ends are 2e-8 and 6e-8: they lie along each
    // other, 6e-8 apart. (Off the short line's own line, the top's end (8, 0) is 1.4e-7.)
    let mut d = Draw::new();
    let quad = d.polygon(&[(2.0, 2e-8), (4.0, 6e-8), (4.0, 5.0), (2.0, 5.0)]);
    let rect = d.rectangle(0.0, -5.0, 8.0, 0.0);
    let (short, top) = (&quad[0], &rect[2]);
    let errors = draw::named(&d.refused());
    let of_pair: Vec<_> = errors
        .iter()
        .filter(|(_, e)| e.contains(short) && e.contains(top))
        .collect();
    assert_eq!(
        of_pair,
        vec![&(
            format!("line {short} and line {top} are 6e-8 mm apart: {RULE}"),
            vec![short.clone(), top.clone()]
        )],
        "{errors:#?}"
    );
}

#[test]
fn a_solves_negative_zeros_are_sent_as_positive_zeros() {
    // The lower half disc r 1 about the origin: the arc from (−1, −0) counter-clockwise
    // to (1, 0), and the line back. As +0, the start is at π from the centre (atan2(+0, −1)),
    // the end a turn on from 0, at 2π; as −0 it would be at −π, a turn earlier.
    let mut d = Draw::new();
    let o = d.point(0.0, -0.0);
    let s = d.point(-1.0, -0.0);
    let e = d.point(1.0, 0.0);
    d.arc(&o, &s, &e);
    d.line(&e, &s);
    let p = d.made();
    assert_eq!(p.regions.len(), 1);
    assert!(close(p.regions[0].area(), PI / 2.0, 1e-15));
    let sent = p.regions[0].outer.sent();
    assert_eq!(
        sent,
        vec![
            silico_protocol::Curve::Arc {
                center: [0.0, 0.0],
                start: [-1.0, 0.0],
                end: [1.0, 0.0],
                radius: 1.0,
                start_angle: PI,
                end_angle: 2.0 * PI,
            },
            silico_protocol::Curve::Line {
                start: [1.0, 0.0],
                end: [-1.0, 0.0],
            },
        ]
    );
    // == takes −0 for +0: each zero's sign, as sent.
    for curve in &sent {
        let points: Vec<[f64; 2]> = match *curve {
            silico_protocol::Curve::Arc {
                center, start, end, ..
            } => vec![center, start, end],
            silico_protocol::Curve::Line { start, end } => vec![start, end],
            silico_protocol::Curve::Circle { center, .. } => vec![center],
        };
        for v in points.into_iter().flatten() {
            assert!(v != 0.0 || v.is_sign_positive(), "{curve:?}");
        }
    }
}

// Round 2 of the CI mutants: each test below failed on a surviving mutant of the code.

#[test]
fn a_solves_negative_zero_xs_are_sent_as_positive_zeros() {
    // The right half disc r 1 about (−0, 0): the arc from (−0, −1) counter-clockwise to
    // (−0, 1), at −π/2 and π/2 from its centre (atan2(±1, +0)), and the line back. Every x
    // is a zero, and each is sent as +0, as a_solves_negative_zeros_are_sent_as_positive_zeros
    // has each y. Area π/2.
    let mut d = Draw::new();
    let o = d.point(-0.0, 0.0);
    let s = d.point(-0.0, -1.0);
    let e = d.point(-0.0, 1.0);
    d.arc(&o, &s, &e);
    d.line(&e, &s);
    let p = d.made();
    assert_eq!(p.regions.len(), 1);
    assert!(close(p.regions[0].area(), PI / 2.0, 1e-15));
    let sent = p.regions[0].outer.sent();
    assert_eq!(
        sent,
        vec![
            silico_protocol::Curve::Arc {
                center: [0.0, 0.0],
                start: [0.0, -1.0],
                end: [0.0, 1.0],
                radius: 1.0,
                start_angle: -PI / 2.0,
                end_angle: PI / 2.0,
            },
            silico_protocol::Curve::Line {
                start: [0.0, 1.0],
                end: [0.0, -1.0],
            },
        ]
    );
    // == takes −0 for +0: each x's sign, as sent.
    for curve in &sent {
        let points: Vec<[f64; 2]> = match *curve {
            silico_protocol::Curve::Arc {
                center, start, end, ..
            } => vec![center, start, end],
            silico_protocol::Curve::Line { start, end } => vec![start, end],
            silico_protocol::Curve::Circle { center, .. } => vec![center],
        };
        for [x, _] in points {
            assert!(x.is_sign_positive(), "{curve:?}");
        }
    }
}

#[test]
fn a_point_on_a_major_arc_before_its_half_turn_is_on_it() {
    // The arc r 5 about the origin from (5, 0) counter-clockwise to (−3, −4), 233.13°, and
    // its chord; a triangle (4, 3), (8, 3), (4, 6) outside the circle, its corner (4, 3) on
    // the arc at 36.87° (|(4, 3)| = 5): 0 from it, within the arc's first half turn (and
    // more than a half turn before its end). Its sides leave the circle there: (1, 0) and
    // (0, 1) each point out of it at (4, 3) (dot (4, 3) > 0), so each meets it only there.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(5.0, 0.0);
    let e = d.point(-3.0, -4.0);
    let a = d.arc(&o, &s, &e);
    d.line(&e, &s);
    let corner = d.point(4.0, 3.0);
    let (right, high) = (d.point(8.0, 3.0), d.point(4.0, 6.0));
    let sides = [
        d.line(&corner, &right),
        d.line(&right, &high),
        d.line(&high, &corner),
    ];
    assert_eq!(
        draw::named(&d.refused()),
        [&sides[0], &sides[2]]
            .map(|side| (
                format!("point {corner}, the end of line {side}, is 0 mm from arc {a}: {RULE}"),
                vec![side.clone(), a.clone(), corner.clone()]
            ))
            .to_vec()
    );
}

#[test]
fn a_point_beyond_a_major_arcs_end_is_measured_off_its_circle_not_from_its_end() {
    // The arc r 5 about the origin from (5, 0) counter-clockwise to (0, −(5 − 2⁻²⁵)), three
    // quarters of a turn, its end 2⁻²⁵ (3e-8) inside its circle, and its chord; a triangle
    // (0, −(5 + 3·2⁻²⁵)), (3, −9), (−3, −9) below it, its corner straight below the arc's
    // end, on the ray of the end's angle: 3·2⁻²⁵ (8.94e-8) outside the circle, so that far
    // from the arc there, though 2⁻²³ (1.19e-7) from the end's point. Its sides leave the
    // corner downwards, away from the arc's end, so the end is 2⁻²³ from each.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(5.0, 0.0);
    let e = d.point(0.0, -(5.0 - libm::exp2(-25.0)));
    let a = d.arc(&o, &s, &e);
    d.line(&e, &s);
    let corner = d.point(0.0, -(5.0 + 3.0 * libm::exp2(-25.0)));
    let (right, left) = (d.point(3.0, -9.0), d.point(-3.0, -9.0));
    let sides = [
        d.line(&corner, &right),
        d.line(&right, &left),
        d.line(&left, &corner),
    ];
    assert_eq!(
        draw::named(&d.refused()),
        [&sides[0], &sides[2]]
            .map(|side| (
                format!(
                    "point {corner}, the end of line {side}, is 8.940696716308594e-8 mm from arc {a}: {RULE}"
                ),
                vec![side.clone(), a.clone(), corner.clone()]
            ))
            .to_vec()
    );
}

#[test]
fn a_point_beyond_a_half_turns_end_is_on_it_by_its_start_alone() {
    // The half turn r 5 about the origin from (3, 4) counter-clockwise to
    // −(1 − 2⁻²⁶)·(3, 4) = (−3 + 3·2⁻²⁶, −4 + 4·2⁻²⁶): its end 5·2⁻²⁶ (7.45e-8) inside
    // its circle, exactly opposite its start (3·e.y − 4·e.x is exactly 0), and its chord
    // through the centre. A triangle beyond the end: its corner (−3.000000024000001,
    // −4.000000032000002), −(3, 4) scaled by 1 + 8e-9 to a few ulps, so 5 × 8e-9 = 4e-8
    // outside the circle and 4e-8 + 7.45e-8 = 1.145e-7 from the end's point. The corner is
    // on the arc's last ray to the last bit: its cross with the start rounds to 0 (the
    // start's left, inclusive) though its cross with the end rounds to −1.8e-15. A half
    // turn is the start's left, so the corner is 4e-8 from the arc. The triangle's other
    // corners are (−7, −4) and (−3, −8): its sides leave the circle at the corner, along
    // −x and −y, away from the arc's end.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(3.0, 4.0);
    let e = d.point(
        -3.0 + 3.0 * libm::exp2(-26.0),
        -4.0 + 4.0 * libm::exp2(-26.0),
    );
    let a = d.arc(&o, &s, &e);
    d.line(&e, &s);
    let (sxy, exy, cxy) = (
        [3.0, 4.0],
        [
            -3.0 + 3.0 * libm::exp2(-26.0),
            -4.0 + 4.0 * libm::exp2(-26.0),
        ],
        [-3.000000024000001, -4.000000032000002],
    );
    // The premises, as on_arc computes them about the origin.
    let cross = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
    assert_eq!(cross(sxy, exy), 0.0, "the ends are opposite, as computed");
    assert!(sxy[0] * exy[0] + sxy[1] * exy[1] < 0.0);
    assert_eq!(
        cross(sxy, cxy),
        0.0,
        "the corner is on the start's line, as computed"
    );
    assert!(cross(cxy, exy) < 0.0, "but to the end's right, as computed");
    let corner = d.point(cxy[0], cxy[1]);
    let (left, low) = (d.point(-7.0, -4.0), d.point(-3.0, -8.0));
    let sides = [
        d.line(&corner, &left),
        d.line(&left, &low),
        d.line(&low, &corner),
    ];
    let errors = draw::named(&d.refused());
    assert_eq!(errors.len(), 2, "{errors:#?}");
    for ((message, elements), side) in errors.iter().zip([&sides[0], &sides[2]]) {
        let start = format!("point {corner}, the end of line {side}, is ");
        let end = format!(" mm from arc {a}: {RULE}");
        let gap: f64 = message
            .strip_prefix(&start)
            .and_then(|m| m.strip_suffix(&end))
            .unwrap_or_else(|| panic!("{message}"))
            .parse()
            .unwrap();
        assert!((gap - 4e-8).abs() < 1e-14, "{gap}");
        assert_eq!(elements, &vec![side.clone(), a.clone(), corner.clone()]);
    }
}

#[test]
fn a_point_just_right_of_a_half_turns_start_line_is_off_it_though_its_cross_with_the_end_is_0() {
    // a_point_beyond_a_half_turns_end_is_on_it_by_its_start_alone's half turn and triangle,
    // the corner moved out to (−3.0000000300000003, −4.000000040000001): −(3, 4) scaled by
    // 1 + 1e-8, so 5 × 1e-8 = 5e-8 outside the circle and 5e-8 + 7.45e-8 = 1.245e-7 from the
    // end's point. As computed, its cross with the start rounds to −1.8e-15 (the start's
    // right) and its cross with the end to 0. A half turn is the start's left, so the corner
    // is off the arc: 1.245e-7 from it, at its end, more than 1e-7. Nothing meets; the
    // regions are the half disc, 12.5π (the line back runs along the start's own diameter,
    // through the centre, so it adds no area), and the triangle, ½ × 4 × 4 = 8 less 1.4e-7
    // for the corner's move: twice its area is (4 − 3e-8)(4 − 4e-8) − 4e-8 × 3e-8 =
    // 16 − 2.8e-7.
    let (sxy, exy, cxy) = (
        [3.0, 4.0],
        [
            -3.0 + 3.0 * libm::exp2(-26.0),
            -4.0 + 4.0 * libm::exp2(-26.0),
        ],
        [-3.0000000300000003, -4.000000040000001],
    );
    // The premises, as on_arc computes them about the origin.
    let cross = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
    assert_eq!(cross(sxy, exy), 0.0, "the ends are opposite, as computed");
    assert!(sxy[0] * exy[0] + sxy[1] * exy[1] < 0.0);
    assert!(
        cross(sxy, cxy) < 0.0,
        "the corner is to the start's right, as computed"
    );
    assert_eq!(cross(cxy, exy), 0.0, "and on the end's line, as computed");
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(sxy[0], sxy[1]);
    let e = d.point(exy[0], exy[1]);
    d.arc(&o, &s, &e);
    d.line(&e, &s);
    d.polygon(&[(cxy[0], cxy[1]), (-7.0, -4.0), (-3.0, -8.0)]);
    let p = d.made();
    let areas: Vec<f64> = p.regions.iter().map(|r| r.area()).collect();
    assert_eq!(areas.len(), 2, "{p:#?}");
    assert!(close(areas[0], 12.5 * PI, 1e-14), "{areas:?}");
    assert!(close(areas[1], 8.0 - 1.4e-7, 1e-12), "{areas:?}");
}

#[test]
fn a_point_on_a_major_arcs_circle_in_its_gap_is_apart_from_it() {
    // The arc r 5 about the origin from (5, 0) counter-clockwise to (−3, −4), 233.13°, and
    // its chord; a triangle (3, −4), (6, −4), (3, −8) outside the circle, its corner (3, −4)
    // on the circle at 306.87°, in the arc's gap (233.13° to 360°): its nearest end (5, 0)
    // is √(2² + 4²) = 4.47 away, so the two are apart. The triangle's sides leave the
    // circle at the corner ((1, 0) and (0, −1) each point out, dot (3, −4) > 0), its third
    // side 7.2 from the centre; the chord is 2.68 from the corner. Areas by hand: the
    // segment (r²/2)(θ − sin θ), θ = π + atan2(4, 3), sin θ = −0.8; the triangle ½·3·4.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(5.0, 0.0);
    let e = d.point(-3.0, -4.0);
    d.arc(&o, &s, &e);
    d.line(&e, &s);
    d.polygon(&[(3.0, -4.0), (6.0, -4.0), (3.0, -8.0)]);
    let p = d.made();
    let mut areas: Vec<f64> = p.regions.iter().map(|r| r.area()).collect();
    areas.sort_by(f64::total_cmp);
    let segment = 12.5 * (PI + libm::atan2(4.0, 3.0) + 0.8);
    assert_eq!(areas.len(), 2, "{p:#?}");
    assert!(close(areas[0], 6.0, 1e-12), "{areas:?}");
    assert!(
        close(areas[1], segment, 1e-12),
        "{areas:?} against {segment}"
    );
}

#[test]
fn an_arc_whose_ends_point_one_way_sweeping_an_ulp_holds_no_point_a_quarter_turn_round() {
    // An arc r 10⁴ about the origin (|start| is 10⁴, as computed) whose end is its start
    // moved out along its radius and a few ulps round. Exactly, the ends are 1.00000017e-7
    // apart (1.0000001714e-7 as computed), nearly all of it along the radius. As computed,
    // the start's and end's cross product is 0 and their dot product positive (one
    // direction), but atan2 puts the end an ulp (4.4e-16 rad) counter-clockwise of the
    // start, so the arc sweeps 4.4e-16 rad. It builds only through rounding at 10⁴, where an
    // ulp is 1.8e-12: its ends are 1.7e-14 more than 1e-7 apart, and its end comes out
    // 9.99995e-8 from its circle as computed (by the rounded radius and the angle's cos and
    // sin; |end| − |start| as computed is 9.99989e-8), 4.5e-13 within 1e-7, though exactly
    // it's about as far out as the ends are apart. Its ends are open. A triangle outside the circle has its corner a quarter turn
    // clockwise round it, (s.y, −s.x), exactly the radius from the centre: nowhere near
    // the arc, which is all within 1e-7 of its start. Only the open ends are refused.
    let (sx, sy) = (-7671.596045639507, -6414.562659490378);
    let (ex, ey) = (-7671.596045716223, -6414.562659554524);
    assert_eq!(sx * ey, sy * ex, "the ends point one way, as computed");
    assert!(libm::atan2(ey, ex) > libm::atan2(sy, sx));
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(sx, sy);
    let e = d.point(ex, ey);
    let a = d.arc(&o, &s, &e);
    // The corner, then out along its radius, and across.
    let corner = d.point(sy, -sx);
    let out = d.point(-9600.0, 11500.0);
    let across = d.point(-6400.0, 11500.0);
    d.line(&corner, &out);
    d.line(&out, &across);
    d.line(&across, &corner);
    assert_eq!(
        draw::named(&d.refused()),
        [&s, &e]
            .map(|p| (
                format!(
                    "point {p} ends arc {a} and no other profile curve, so the profile is open there"
                ),
                vec![p.clone(), a.clone()]
            ))
            .to_vec()
    );
}

// Arcs whose ends point one way from their centres: by their angles one turns almost a
// whole circle, and one almost nothing.

/// An arc about the origin from `from` to `to`, closed by the line back, and the rectangle
/// (5000, −1000)–(20000, 1000): the arc's and line's names and the rectangle's lines
/// (bottom, right, top, left). `from` and `to` point one way from the centre, as computed:
/// `from.x·to.y` and `from.y·to.x` are equal (their cross product is 0), their dot product
/// positive.
fn one_way_arc_and_rectangle(from: [f64; 2], to: [f64; 2]) -> (Draw, [String; 2], Vec<String>) {
    assert_eq!(
        from[0] * to[1],
        from[1] * to[0],
        "the ends point one way, as computed"
    );
    assert!(from[0] * to[0] + from[1] * to[1] > 0.0);
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let start = d.point(from[0], from[1]);
    let end = d.point(to[0], to[1]);
    let a = d.arc(&o, &start, &end);
    let back = d.line(&end, &start);
    let rectangle = d.rectangle(5000.0, -1000.0, 20000.0, 1000.0);
    (d, [a, back], rectangle)
}

#[test]
fn an_arc_whose_ends_point_one_way_turning_almost_a_whole_circle_is_crossed_where_its_circle_is() {
    // The arc r 10⁴ about the origin (|start| is 10⁴, as computed) from its start at
    // −124.7° to its end, out along the start's radius and a few ulps round: exactly
    // 1.000002e-7 from the start (1.0000019980e-7 as computed, 2.0e-13 over 1e-7), and as
    // computed 9.99986e-8 from its circle (1.4e-12 within 1e-7), so it builds only through
    // rounding at 10⁴, as the ulp-sweep arc does. It's closed by the line back,
    // 1.0000019980e-7 long. atan2 puts the end a few ulps
    // clockwise of the start, so the arc turns counter-clockwise a whole circle less a few
    // ulps (6.283185307179585 rad): all of the circle but the few ulps at −124.7°. The
    // rectangle's bottom (y = −1000) and top (y = 1000) cross the circle at
    // x = √(10⁸ − 10⁶) = 3000√11 = 9949.87 (within 5000..20000), at ∓5.74°, on the arc;
    // its left side (x = 5000, |y| ≤ 1000) is inside the circle (5000² + 1000² < 10⁸) and
    // its right side (x = 20000) outside. The arc crosses the bottom and the top, as any
    // arc crossing a line does.
    let (s, e) = (
        [-5692.484703417905, -8221.655423414019],
        [-5692.48470347483, -8221.655423496235],
    );
    assert!(libm::atan2(e[1], e[0]) < libm::atan2(s[1], s[0]));
    let (d, [a, _], rectangle) = one_way_arc_and_rectangle(s, e);
    let (bottom, top) = (&rectangle[0], &rectangle[2]);
    let errors = d.refused();
    assert!(
        errors
            .iter()
            .all(|e| e.code() == silico_core::ErrorCode::InvalidInput)
    );
    assert_eq!(
        draw::named(&errors),
        [bottom, top]
            .map(|n| (
                format!("arc {a} and line {n} cross"),
                vec![a.clone(), n.clone()]
            ))
            .to_vec()
    );
}

#[test]
fn an_arc_turning_almost_a_whole_circle_holds_the_point_opposite_its_start() {
    // an_arc_whose_ends_point_one_way_turning_almost_a_whole_circle_is_crossed_where_its_
    // circle_is's arc and line back, and a triangle outside the circle cornered at
    // (5692.484703417854, 8221.655423413944): the start negated and moved a few dozen ulps,
    // opposite the start, half a turn round the arc, so on it; 9.09e-11 inside the circle
    // (|corner| − 10⁴ as computed). As computed, its cross with the start is 0 (on the
    // start's line) and with the end −7.45e-9. Its sides leave the circle there, to
    // (8000, 11500) and (5692, 12000) (each dot the corner > 0); the third side is 12,900
    // from the centre. The corner is 9.09e-11 from the arc, from each side's end.
    let (s, e) = (
        [-5692.484703417905, -8221.655423414019],
        [-5692.48470347483, -8221.655423496235],
    );
    let cxy = [5692.484703417854, 8221.655423413944];
    let cross = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
    assert_eq!(
        cross(s, cxy),
        0.0,
        "the corner is on the start's line, as computed"
    );
    assert!(cross(cxy, e) < 0.0);
    assert!(libm::atan2(e[1], e[0]) < libm::atan2(s[1], s[0]));
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let start = d.point(s[0], s[1]);
    let end = d.point(e[0], e[1]);
    let a = d.arc(&o, &start, &end);
    d.line(&end, &start);
    let corner = d.point(cxy[0], cxy[1]);
    let (out, up) = (d.point(8000.0, 11500.0), d.point(5692.0, 12000.0));
    let sides = [
        d.line(&corner, &out),
        d.line(&out, &up),
        d.line(&up, &corner),
    ];
    let errors = draw::named(&d.refused());
    assert_eq!(errors.len(), 2, "{errors:#?}");
    for ((message, elements), side) in errors.iter().zip([&sides[0], &sides[2]]) {
        let start = format!("point {corner}, the end of line {side}, is ");
        let end = format!(" mm from arc {a}: {RULE}");
        let gap: f64 = message
            .strip_prefix(&start)
            .and_then(|m| m.strip_suffix(&end))
            .unwrap_or_else(|| panic!("{message}"))
            .parse()
            .unwrap();
        // To two ulps of 10⁴, as hypot and a square root may round.
        assert!((gap - 9.09e-11).abs() < 4e-12, "{gap}");
        assert_eq!(elements, &vec![side.clone(), a.clone(), corner.clone()]);
    }
}

#[test]
fn an_arc_whose_ends_point_one_way_turning_almost_nothing_is_a_sliver_apart_from_the_rectangle() {
    // The same shape turning almost nothing. (The first test's ends swapped won't do: its
    // start would then be the outer point, and its end, the inner, 1.0000014e-7 off the
    // circle through the start, too far to build.) The ulp-sweep test's arc: r 10⁴
    // (|start| is 10⁴, as computed), from its start at −140.1° to its end out along its
    // radius, 1.0000001714e-7 from it as computed, which atan2 puts an ulp
    // counter-clockwise of the start: it turns 4.4e-16 rad, its
    // length 10⁴ × 4.4e-16 = 4.4e-12, far from the rectangle (whose nearest corner,
    // (5000, −1000), is at −11.3°). With the line back, 1.0000001714e-7 long, it's a loop of
    // no area (under 1e-15 mm²) and a perimeter of 1.0000001714e-7 + 4.4e-12 = 1.00004e-7:
    // a sliver, refused alone. Nothing crosses the rectangle, which is a region.
    let (s, e) = (
        [-7671.596045639507, -6414.562659490378],
        [-7671.596045716223, -6414.562659554524],
    );
    assert!(libm::atan2(e[1], e[0]) > libm::atan2(s[1], s[0]));
    let (d, [a, back], _) = one_way_arc_and_rectangle(s, e);
    let errors = d.refused();
    assert_eq!(errors.len(), 1, "{:#?}", draw::named(&errors));
    let message = errors[0].message();
    let numbers = message
        .strip_prefix(&format!("loop {a}, {back} encloses no area to speak of ("))
        .and_then(|m| m.strip_suffix(" mm)"))
        .unwrap_or_else(|| panic!("{message}"));
    let (area, perimeter) = numbers
        .split_once(" mm², against a perimeter of ")
        .unwrap_or_else(|| panic!("{message}"));
    let (area, perimeter): (f64, f64) = (area.parse().unwrap(), perimeter.parse().unwrap());
    assert!(area < 1e-15, "{message}");
    assert!((perimeter - 1.00004e-7).abs() < 1e-12, "{message}");
    assert_eq!(errors[0].elements(), [a.clone(), back.clone()]);
}

#[test]
fn sides_on_one_tilted_line_just_over_the_tolerance_apart_dont_cross() {
    // Two triangles, each with a side on the line y = 0.8x + 0.9: (3.6, 3.78) to
    // (6.5, 6.1), and (8.8, 7.94) to (6.50000015, 6.10000012). The sides' near ends are
    // sqrt(1.5^2 + 1.2^2) e-7 = 1.92e-7 mm apart, more than the tolerance, along the line;
    // their boxes are within the sweep's 2e-7 margin, so the pair is compared. Every corner
    // is the decimal nearest the line, so each side's ends are off the other's line by
    // rounding only. Nothing meets. Areas by hand: the first triangle's side x = 6.5 from
    // y 4 to 6.1 and apex at x 3.6, |3.6 (6.1 - 4) + 6.5 (4 - 3.78) + 6.5 (3.78 - 6.1)| / 2
    // = 3.045; the second's side x = 8.8 from y 6 to 7.94 and apex 2.29999985 from it,
    // 1.94 (2.29999985) / 2 = 2.2309998545.
    let mut d = Draw::new();
    d.polygon(&[(3.6, 3.78), (6.5, 6.1), (6.5, 4.0)]);
    d.polygon(&[(8.8, 7.94), (6.50000015, 6.10000012), (8.8, 6.0)]);
    let p = d.made();
    assert_eq!(p.regions.len(), 2, "{p:#?}");
    assert!(
        close(p.regions[0].area(), 3.045, 1e-12),
        "{}",
        p.regions[0].area()
    );
    assert!(
        close(p.regions[1].area(), 2.2309998545, 1e-12),
        "{}",
        p.regions[1].area()
    );
}
