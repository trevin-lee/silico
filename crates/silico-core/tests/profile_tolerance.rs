//! M3.5's tolerance (the maintainer's decision of 2026-10-18 on docs/design/m3.5-profile.md's
//! open question 1): two profile curves within `LENGTH_TOLERANCE`, 1e-7 mm, of each other
//! away from a point they share are refused, inclusive; so are open ends that close
//! (share the point) and loops and regions whose area is at most 1e-7 × their perimeter.
//! Each boundary a step either side: values whose distance is exactly 1e-7 in f64 (checked
//! here, since `sqrt(t·t) == t` and scaling by a power of two are exact), the next f64 above,
//! and 0.99e-7 against 1.01e-7 where the distance is a difference of rounded values.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/profile.rs"]
mod draw;

use std::f64::consts::PI;

use draw::{Draw, close, named};
use silico_core::profile::Region;
use silico_protocol::LENGTH_TOLERANCE;

const RULE: &str =
    "a profile's curves must be more than 1e-7 mm apart except at a point they share";

/// The next f64 above 1e-7.
fn above() -> f64 {
    LENGTH_TOLERANCE.next_up()
}

#[test]
fn the_tolerance_is_occt_s_and_its_boundary_values_are_exact() {
    assert_eq!(LENGTH_TOLERANCE, 1e-7);
    assert_eq!(above(), 1.0000000000000001e-7);
    for t in [LENGTH_TOLERANCE, above(), 5e-8] {
        assert_eq!((t * t).sqrt(), t);
        assert_eq!(8.0 * t / 8.0, t);
    }
}

/// A rectangle (0, −5)–(8, 0), and above it one (2, gap)–(6, 5): A's top and B's bottom
/// are parallel, `gap` apart, and B's bottom corners `gap` above A's top. A's top is 8 long,
/// so the gap is measured off it exactly.
fn stacked(gap: f64) -> (Draw, Vec<String>, Vec<String>) {
    let mut d = Draw::new();
    let a = d.rectangle(0.0, -5.0, 8.0, 0.0);
    let b = d.rectangle(2.0, gap, 6.0, 5.0);
    (d, a, b)
}

#[test]
fn parallel_lines_exactly_1e_7_apart_are_refused_and_the_next_f64_passes() {
    let (d, a, b) = stacked(LENGTH_TOLERANCE);
    let top = &a[2];
    let errors = d.refused();
    assert_eq!(
        named(&errors),
        vec![
            (
                format!("line {top} and line {} are 1e-7 mm apart: {RULE}", b[0]),
                vec![top.clone(), b[0].clone()]
            ),
            (
                format!(
                    "point p0000010, the end of line {}, is 1e-7 mm from line {top}: {RULE}",
                    b[1]
                ),
                vec![b[1].clone(), top.clone(), "p0000010".to_owned()]
            ),
            (
                format!(
                    "point p0000009, the end of line {}, is 1e-7 mm from line {top}: {RULE}",
                    b[3]
                ),
                vec![b[3].clone(), top.clone(), "p0000009".to_owned()]
            ),
        ]
    );

    let (d, ..) = stacked(above());
    let p = d.made();
    assert_eq!(p.regions.len(), 2);
}

#[test]
fn near_lines_name_the_gap_as_measured() {
    // 5e-8 apart, printed as the file prints numbers.
    let (d, a, b) = stacked(5e-8);
    let errors = d.refused();
    assert_eq!(
        errors[0].message(),
        format!("line {} and line {} are 5e-8 mm apart: {RULE}", a[2], b[0])
    );
    assert_eq!(
        errors[0].suggestion(),
        Some("move them more than 1e-7 mm apart")
    );
    assert_eq!(errors[0].elements_total(), 2);
}

#[test]
fn line_line_0_99e_7_apart_is_refused_and_1_01e_7_passes() {
    assert!(stacked(0.99e-7).0.profile().is_err());
    assert_eq!(stacked(1.01e-7).0.made().regions.len(), 2);
}

#[test]
fn two_holes_0_99e_7_apart_are_refused_and_1_01e_7_apart_pass() {
    // Holes (−5, −2)–(0, 2) and (g, −2)–(5, 2) in a 20 × 20 square: their facing sides,
    // 4 long, are g apart.
    let holes = |g: f64| {
        let mut d = Draw::new();
        d.rectangle(-10.0, -10.0, 10.0, 10.0);
        let a = d.rectangle(-5.0, -2.0, 0.0, 2.0);
        let b = d.rectangle(g, -2.0, 5.0, 2.0);
        (d, a, b)
    };
    let (d, a, b) = holes(0.99e-7);
    let errors = d.refused();
    assert!(
        named(&errors).iter().any(|(m, e)| {
            m.starts_with(&format!("line {} and line {} are 9.9", a[1], b[3]))
                && m.ends_with(&format!("e-8 mm apart: {RULE}"))
                && e == &vec![a[1].clone(), b[3].clone()]
        }),
        "{:#?}",
        named(&errors)
    );
    let g = 1.01e-7;
    let p = holes(g).0.made();
    assert_eq!(p.regions.len(), 1);
    assert_eq!(p.regions[0].holes.len(), 2);
    assert!(close(
        p.regions[0].area(),
        400.0 - 20.0 - 4.0 * (5.0 - g),
        1e-12
    ));
}

#[test]
fn a_circular_hole_0_99e_7_from_a_side_is_refused_and_1_01e_7_passes() {
    // A circle about (10, 5) of radius 5 − g in a 20 × 20 square: g from its bottom (to
    // rounding of 5 − g, about 1e-15).
    let near = |g: f64| {
        let mut d = Draw::new();
        let n = d.rectangle(0.0, 0.0, 20.0, 20.0);
        let c = d.circle(10.0, 5.0, 5.0 - g);
        (d, n, c)
    };
    let (d, n, c) = near(0.99e-7);
    let errors = d.refused();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].elements(), [c.clone(), n[0].clone()]);
    assert!(
        errors[0]
            .message()
            .starts_with(&format!("circle {c} and line {} are 9.9", n[0]))
    );
    let g = 1.01e-7;
    let p = near(g).0.made();
    assert!(close(
        p.regions[0].area(),
        400.0 - PI * (5.0 - g) * (5.0 - g),
        1e-12
    ));
}

#[test]
fn concentric_circles_0_99e_7_apart_are_refused_as_near_and_1_01e_7_apart_as_a_ring() {
    let ring = |outer: f64| {
        let mut d = Draw::new();
        let a = d.circle(0.0, 0.0, 1.0);
        let b = d.circle(0.0, 0.0, outer);
        (d, a, b)
    };
    let (d, a, b) = ring(1.0 + 0.99e-7);
    let errors = d.refused();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].elements(), [a.clone(), b.clone()]);
    // The gap is (1 + 0.99e-7) − 1, 0.99e-7 to rounding (about 1e-16).
    let m = errors[0].message();
    assert!(
        m.starts_with(&format!("circle {a} and circle {b} are 9.")),
        "{m}"
    );
    assert!(m.ends_with(&format!("e-8 mm apart: {RULE}")), "{m}");

    // 1.01e-7 apart they don't meet, but the ring between them, 2π × 1e-7 or so wide in
    // all, is a region with no area to speak of (the next test's rule).
    let (d, ..) = ring(1.0 + 1.01e-7);
    let errors = d.refused();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message().starts_with("the region inside loop "));

    // 1.0000001 − 1.0 is 1.00000000058e-7, past 1e-7: the circles don't meet (the ring is
    // refused as above).
    let outer = 1.0000001;
    assert!(outer - 1.0 > LENGTH_TOLERANCE);
    let (d, ..) = ring(outer);
    assert!(
        d.refused()[0]
            .message()
            .starts_with("the region inside loop ")
    );
}

#[test]
fn open_ends_exactly_1e_7_apart_share_the_point_and_the_next_f64_is_open() {
    // Three sides of the 4 × 3 rectangle and the fourth, up the left side, ending `gap`
    // above the corner it should end at.
    let short = |gap: f64| {
        let mut d = Draw::new();
        let p = [
            d.point(0.0, 0.0),
            d.point(4.0, 0.0),
            d.point(4.0, 3.0),
            d.point(0.0, 3.0),
            d.point(0.0, gap),
        ];
        let n = [
            d.line(&p[0], &p[1]),
            d.line(&p[1], &p[2]),
            d.line(&p[2], &p[3]),
            d.line(&p[3], &p[4]),
        ];
        (d, p, n)
    };
    for gap in [LENGTH_TOLERANCE, 0.99e-7] {
        let (d, p, n) = short(gap);
        let errors = d.refused();
        assert_eq!(
            named(&errors),
            vec![(
                format!(
                    "points {}, {} end curves within 1e-7 mm of each other, but aren't one point, so the profile is open there",
                    p[0], p[4]
                ),
                vec![p[0].clone(), p[4].clone(), n[0].clone(), n[3].clone()]
            )]
        );
        assert_eq!(
            errors[0].suggestion(),
            Some("share the point: end the curves at one point")
        );
    }
    for gap in [above(), 1.01e-7, 1.5e-7] {
        let (d, p, n) = short(gap);
        assert_eq!(
            named(&d.refused()),
            vec![
                (
                    format!(
                        "point {} ends line {} and no other profile curve, so the profile is open there",
                        p[0], n[0]
                    ),
                    vec![p[0].clone(), n[0].clone()]
                ),
                (
                    format!(
                        "point {} ends line {} and no other profile curve, so the profile is open there",
                        p[4], n[3]
                    ),
                    vec![p[4].clone(), n[3].clone()]
                ),
            ],
            "{gap}"
        );
    }
}

#[test]
fn thin_rectangles_are_slivers_up_to_twice_the_tolerance_wide() {
    // 10 × w: 10w against 1e-7 × (20 + 2w), so a sliver up to w ≈ 2e-7.
    let thin = |w: f64| {
        let mut d = Draw::new();
        d.rectangle(0.0, 0.0, 10.0, w);
        d
    };
    // The rule by hand: 10w against 1e-7 × (20 + 2w).
    let by_hand = |w: f64| 10.0 * w <= 1e-7 * (20.0 + 2.0 * w);
    assert!(by_hand(1.9e-7));
    let errors = thin(1.9e-7).refused();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message().starts_with("loop "));
    assert!(
        errors[0]
            .message()
            .contains(" encloses no area to speak of (")
    );

    assert!(!by_hand(2.1e-7));
    let p = thin(2.1e-7).made();
    assert!(close(p.regions[0].area(), 2.1e-6, 1e-9));
}

#[test]
fn a_ring_whose_loops_each_have_area_is_a_sliver_region() {
    // Circles r 1 and 1 + g: 2πg or so against 1e-7 × 2π(2 + g), so a sliver up to
    // g ≈ 2e-7.
    let ring = |g: f64| {
        let mut d = Draw::new();
        let outer = d.circle(0.0, 0.0, 1.0 + g);
        let hole = d.circle(0.0, 0.0, 1.0);
        (d, outer, hole)
    };
    let (d, outer, hole) = ring(1.5e-7);
    let errors = d.refused();
    assert_eq!(errors.len(), 1);
    let e = &errors[0];
    assert!(
        e.message().starts_with(&format!(
            "the region inside loop {outer} less its holes encloses no area to speak of ("
        )),
        "{e}"
    );
    assert!(
        e.message().contains(" mm², against a perimeter of 12.566"),
        "{e}"
    );
    assert_eq!(e.elements(), [outer.clone(), hole.clone()]);
    assert_eq!(e.elements_total(), 2);
    assert_eq!(
        e.suggestion(),
        Some("give it some width: the kernel builds nothing thinner than 1e-7 mm")
    );

    let g = 3e-7;
    let p = ring(g).0.made();
    let area: f64 = p.regions.iter().map(Region::area).sum();
    assert!(close(area, PI * ((1.0 + g) * (1.0 + g) - 1.0), 1e-12));
}

#[test]
fn a_circle_whose_area_is_exactly_1e_7_times_its_perimeter_is_a_sliver() {
    // r = 2e-7: πr² (as π·r·r) and 1e-7 × 2πr (as 1e-7 × (2π·r)) are the same f64, so the
    // rule's ≤ decides it; the next f64 up is past it.
    let r = 2e-7;
    assert_eq!(PI * r * r, 1e-7 * (2.0 * PI * r));
    let mut d = Draw::new();
    let c = d.circle(0.0, 0.0, r);
    let errors = d.refused();
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0]
            .message()
            .starts_with(&format!("loop {c} encloses no area to speak of ("))
    );

    let r = r.next_up();
    assert!(PI * r * r > 1e-7 * (2.0 * PI * r));
    let mut d = Draw::new();
    d.circle(0.0, 0.0, r);
    assert!(close(d.made().regions[0].area(), PI * r * r, 1e-15));
}
