//! M3.5's profile where its rules meet a tie or an exact tangency (the M3 milestone's
//! mutants run): the end named of two as near another curve, which line two of one length
//! along one line are measured from, and a line exactly tangent to an arc's circle off the
//! arc. Each failed on a mutant of the code before it was added; expected values are by
//! hand.
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
