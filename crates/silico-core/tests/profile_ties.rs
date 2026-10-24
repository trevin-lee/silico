//! M3.5's profile where its rules meet a tie, an exact tangency, or rounding's edge (the
//! M3 milestone's mutants run): the end named of two as near another curve, which line two
//! of one length along one line are measured from, a line exactly tangent to an arc's
//! circle off the arc, a short line ending just past a long one's end, and an arc and a
//! circle on one centre. All but two arcs of one circle failed on a mutant of the code
//! before they were added; expected values are by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/profile.rs"]
mod draw;

use std::f64::consts::PI;

use draw::{Draw, close};

#[test]
fn of_two_ends_as_near_another_curve_the_first_by_id_is_named() {
    // The upper half of the circle r 5 about (5, 5e-8), from (10, 5e-8) counter-clockwise
    // to (0, 5e-8): both its ends 5e-8 above the line (-1, 0) to (11, 0), exactly (each
    // the same sum: 12 · 5e-8 / 12). Of two ends as near, the meeting names the first by
    // ID: the start, p0000001.
    let mut d = Draw::new();
    let start = d.point(10.0, 5e-8);
    let end = d.point(0.0, 5e-8);
    let center = d.point(5.0, 5e-8);
    let arc = d.arc(&center, &start, &end);
    let (left, right) = (d.point(-1.0, 0.0), d.point(11.0, 0.0));
    let line = d.line(&left, &right);
    let met: Vec<String> = d
        .refused()
        .iter()
        .map(|e| e.message().to_owned())
        .filter(|m| m.contains(&format!("the end of arc {arc}")))
        .collect();
    assert_eq!(met.len(), 1, "{met:#?}");
    assert!(
        met[0].starts_with(&format!("point {start}, the end of arc {arc}, is "))
            && met[0].contains(&format!(" mm from line {line}")),
        "{met:#?}"
    );
    assert!(!met[0].contains(&end), "{met:#?}");
}

#[test]
fn two_lines_of_one_length_along_one_line_are_measured_from_the_first() {
    // (0, 0) to (10, 0), then (5, 0) to (15, 5e-8): both 10 long (the second's 5e-8 rise
    // is under an ulp of its length), running along each other from x = 5 to 10. Measured
    // from the first's line, the second's far end is 5e-8 off it: they're 5e-8 apart. From
    // the second's line, the first's ends would be 2.5e-8 off.
    let mut d = Draw::new();
    let (p, q) = (d.point(0.0, 0.0), d.point(10.0, 0.0));
    let first = d.line(&p, &q);
    let (r, s) = (d.point(5.0, 0.0), d.point(15.0, 5e-8));
    let second = d.line(&r, &s);
    let apart: Vec<String> = d
        .refused()
        .iter()
        .map(|e| e.message().to_owned())
        .filter(|m| m.starts_with(&format!("line {first} and line {second} are ")))
        .collect();
    assert_eq!(apart.len(), 1, "{apart:#?}");
    assert!(
        apart[0].starts_with(&format!(
            "line {first} and line {second} are 5e-8 mm apart:"
        )),
        "{apart:#?}"
    );
}

#[test]
fn a_line_tangent_to_an_arcs_circle_off_the_arc_is_apart_from_it() {
    // A D: the upper half of the circle r 5 about the origin, closed by its chord; and a
    // rectangle (-2, -6) to (2, -5), its top exactly tangent to the circle at (0, -5),
    // which isn't on the arc, and 5 from the D everywhere else. Two regions, the D's area
    // 25π/2 and the rectangle's 4.
    let mut d = Draw::new();
    let center = d.point(0.0, 0.0);
    let (right, left) = (d.point(5.0, 0.0), d.point(-5.0, 0.0));
    d.arc(&center, &right, &left);
    d.line(&left, &right);
    d.rectangle(-2.0, -6.0, 2.0, -5.0);
    let p = d.made();
    let mut areas: Vec<f64> = p.regions.iter().map(|r| r.area()).collect();
    areas.sort_by(f64::total_cmp);
    assert_eq!(areas.len(), 2, "{areas:?}");
    assert!(close(areas[0], 4.0, 1e-12), "{areas:?}");
    assert!(close(areas[1], 25.0 * PI / 2.0, 1e-12), "{areas:?}");
}

#[test]
fn two_arcs_of_one_circle_that_share_no_stretch_are_apart() {
    // Two regions on the circle r 5 about the origin, one centre: the upper half closed by
    // its chord, and the arc from (-3, -4) counter-clockwise through the bottom to (3, -4),
    // closed by its chord. The arcs are of one circle but share none of it, so they don't
    // meet: 4 apart at their nearest. Areas by hand: 25π/2, and the segment
    // (r²/2)(θ - sin θ) with θ = 2 atan2(3, 4) and sin θ = 2 (3/5)(4/5).
    let mut d = Draw::new();
    let center = d.point(0.0, 0.0);
    let (right, left) = (d.point(5.0, 0.0), d.point(-5.0, 0.0));
    d.arc(&center, &right, &left);
    d.line(&left, &right);
    let (low_left, low_right) = (d.point(-3.0, -4.0), d.point(3.0, -4.0));
    d.arc(&center, &low_left, &low_right);
    d.line(&low_right, &low_left);
    let p = d.made();
    let mut areas: Vec<f64> = p.regions.iter().map(|r| r.area()).collect();
    areas.sort_by(f64::total_cmp);
    let theta = 2.0 * libm::atan2(3.0, 4.0);
    assert_eq!(areas.len(), 2, "{areas:?}");
    assert!(close(areas[0], 12.5 * (theta - 0.96), 1e-12), "{areas:?}");
    assert!(close(areas[1], 25.0 * PI / 2.0, 1e-12), "{areas:?}");
}

/// The ends of a long line, (0, 0) to (65536, 65536), and of a short one from just past its
/// end: r is 1.77e-7 from q, within rounding of p–q's line, and r–s, kinked toward p–q's
/// line, crosses it 1.4e-3 short of q, but stops at r. Nothing comes within 1e-7 of the
/// other (by hand): no meeting.
fn joint(d: &mut Draw) -> [String; 4] {
    [
        d.point(0.0, 0.0),
        d.point(65536.0, 65536.0),
        d.point(65536.000000125, 65536.00000012502),
        d.point(65537.000000125, 65537.00000013957),
    ]
}

/// The refusals naming both `a` and `b`.
fn naming_both(d: &Draw, a: &str, b: &str) -> Vec<String> {
    d.refused()
        .iter()
        .map(|e| e.message().to_owned())
        .filter(|m| m.contains(a) && m.contains(b))
        .collect()
}

#[test]
fn a_short_line_just_past_a_long_lines_end_kinked_toward_its_line_doesnt_cross_it() {
    // The short line's end r is on the long line's line to rounding (its side 0), and the
    // long line's ends are surely on either side of the short one's line. Both must be
    // surely across for a crossing (the M3 review's #95, refuting #78's argument).
    let mut d = Draw::new();
    let [p, q, r, s] = joint(&mut d);
    let long = d.line(&p, &q);
    let short = d.line(&r, &s);
    let met = naming_both(&d, &long, &short);
    assert!(met.is_empty(), "{met:#?}");
}

#[test]
fn a_short_line_drawn_first_ending_just_past_a_long_lines_end_doesnt_cross_it() {
    // The same, the short line drawn first from s to r, so the end whose side can't be
    // told is its second: the other order of the crossing's test.
    let mut d = Draw::new();
    let [p, q, r, s] = joint(&mut d);
    let short = d.line(&s, &r);
    let long = d.line(&p, &q);
    let met = naming_both(&d, &short, &long);
    assert!(met.is_empty(), "{met:#?}");
}

#[test]
fn an_arc_and_a_circle_on_one_centre_the_tolerance_apart_meet_though_its_ends_measure_over() {
    // An arc r 2.5e-7 about the origin from (2.0742859626077817e-7, 1.3954704387153133e-7),
    // whose squared length gives 2.5e-7 but whose hypot gives 2.5000000000000004e-7, to
    // (1.0203970485069385e-7, 2.2822773414724007e-7), 0.56 rad round, drawn where the
    // request's check puts its end, bit for bit (and the kernel, to an ulp); closed by its
    // chord. A circle r 1.5e-7 about the same centre: exactly 1e-7 apart all along the arc,
    // which touches (within 1e-7, inclusive), though each of the arc's ends measures
    // 1.0000000000000005e-7 from the circle, where it's drawn and where the kernel ends it
    // (the M3 review's #95).
    let (start, end) = (
        [2.0742859626077817e-7, 1.3954704387153133e-7],
        [1.0203970485069385e-7, 2.2822773414724007e-7],
    );
    // The premises, as the request measures them.
    let [r, from, to] = silico_protocol::arc_from_points([0.0; 2], start, end).unwrap();
    assert_eq!(r - 1.5e-7, 1e-7);
    assert_eq!(
        silico_protocol::on_circle([0.0; 2], r, to),
        end,
        "drawn where it ends"
    );
    for p in [
        start,
        end,
        silico_protocol::on_circle([0.0; 2], r, from),
        silico_protocol::on_circle([0.0; 2], r, to),
    ] {
        assert_eq!(
            silico_protocol::distance([0.0; 2], p) - 1.5e-7,
            1.0000000000000005e-7,
            "{p:?}"
        );
    }
    let mut d = Draw::new();
    let center = d.point(0.0, 0.0);
    let start = d.point(start[0], start[1]);
    let end = d.point(end[0], end[1]);
    let arc = d.arc(&center, &start, &end);
    d.line(&end, &start);
    let circle = d.circle_about(&center, 1.5e-7);
    let met = naming_both(&d, &arc, &circle);
    assert_eq!(met.len(), 1, "{met:#?}");
    assert!(
        met[0].starts_with(&format!("arc {arc} and circle {circle} are 1e-7 mm apart")),
        "{met:#?}"
    );
}
