//! Hostile and large profiles: nesting 50 deep, thousands of loops, loops at the kilometre
//! limit, arcs that nearly close, solutions that collapse a curve, and the same sketch in
//! any list order. Expected values are closed form.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::draw;

use std::f64::consts::PI;

use draw::{Draw, close, named};
use proptest::prelude::*;
use silico_core::document::{Coordinate, ElementId};
use silico_core::profile::Region;
use silico_protocol::MAX_LENGTH;

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

#[test]
fn squares_nested_50_deep_are_25_rings() {
    // Squares of side 100, 98, …, 2 about the origin: rings 100² − 98², 96² − 94², ….
    let mut d = Draw::new();
    for k in 0..50 {
        let h = 50.0 - f64::from(k);
        if k % 2 == 0 {
            d.rectangle(-h, -h, h, h);
        } else {
            d.polygon(&[(-h, -h), (-h, h), (h, h), (h, -h)]);
        }
    }
    let p = d.made();
    assert_eq!(p.regions.len(), 25);
    let mut got: Vec<(usize, f64)> = p.regions.iter().map(|r| (r.depth, r.area())).collect();
    got.sort_by_key(|&(depth, _)| depth);
    for (k, (depth, area)) in got.into_iter().enumerate() {
        let outer = 100.0 - 4.0 * k as f64;
        let inner = outer - 2.0;
        assert_eq!(depth, 2 * k);
        assert!(
            close(area, outer * outer - inner * inner, 1e-12),
            "{k}: {area}"
        );
    }
    for r in &p.regions {
        assert_eq!(r.holes.len(), 1);
        assert!(r.outer.area > 0.0 && r.holes[0].area < 0.0);
    }
}

#[test]
fn the_gate_bounds_a_profile_at_341_loops() {
    // A sketch the gate takes has at most 1,024 unknowns (2 a point, 1 a circle's radius,
    // 3 an arc's), and the cheapest loop, a circle, costs 3: 341 loops (1,023), never 342
    // (1,026). That keeps a face's holes far below what OCCT's face check, quadratic in
    // them, takes long over (1,000 holes 3.7 s, 3,000 holes 24 s, optimized).
    let circles = |n: u32| {
        let mut d = Draw::new();
        for k in 0..n {
            d.circle(5.0 * f64::from(k % 20), 5.0 * f64::from(k / 20), 2.0);
        }
        d
    };
    let d = circles(341);
    let p = d.made();
    assert_eq!(p.regions.len(), 341);
    assert!(
        p.regions
            .iter()
            .all(|r| r.depth == 0 && close(r.area(), 4.0 * PI, 1e-12))
    );

    let invalid = circles(342).checked().unwrap_err();
    assert!(
        invalid
            .errors()
            .iter()
            .any(|e| e.message().contains("1026")),
        "{:#?}",
        invalid.errors()
    );

    // As holes: a square (4 points, 8 unknowns) and 338 circles (1,014): 1,022.
    let mut d = Draw::new();
    d.rectangle(-10.0, -10.0, 100.0, 100.0);
    for k in 0..338 {
        d.circle(5.0 * f64::from(k % 18), 5.0 * f64::from(k / 18), 2.0);
    }
    let p = d.made();
    assert_eq!(p.regions[0].holes.len(), 338);
    assert!(close(
        p.regions[0].area(),
        110.0 * 110.0 - 338.0 * 4.0 * PI,
        1e-12
    ));
}

#[test]
fn as_many_lines_as_a_sketch_may_have_crossing_each_other_are_counted() {
    // 512 points (the most the gate's unknowns allow), 256 up each side of a 100 wide
    // strip, and lines from each on the left to the 37 on the right from its own height
    // up (wrapping): 9,472 lines, 9,984 pieces of geometry. Every point is where 37 lines
    // meet, and two lines that share no point cross exactly when their left ends and their
    // right ends are in opposite orders, which is counted here on the indices alone.
    let mut d = Draw::new();
    let left: Vec<String> = (0..256).map(|i| d.point(0.0, f64::from(i))).collect();
    let right: Vec<String> = (0..256).map(|j| d.point(100.0, f64::from(j))).collect();
    let mut pairs = Vec::new();
    for (i, from) in left.iter().enumerate() {
        for k in 0..37 {
            let j = (i + k) % 256;
            d.line(from, &right[j]);
            pairs.push((i as i64, j as i64));
        }
    }
    assert!(d.sketch.points.len() + d.sketch.lines.len() <= silico_protocol::MAX_SKETCH_GEOMETRY);
    let mut crossings = 0u64;
    for (a, &(i, j)) in pairs.iter().enumerate() {
        for &(k, l) in &pairs[a + 1..] {
            if (i - k) * (j - l) < 0 {
                crossings += 1;
            }
        }
    }
    // A few seconds unoptimized: the pairs are tested by a sweep over boxes, which here
    // overlap almost all of them.
    let r = d.profile().unwrap_err();
    assert_eq!(r.total(), 512 + crossings);
    assert_eq!(r.errors().len(), silico_core::document::PROBLEMS_LISTED);
    assert!(
        r.errors()[0]
            .message()
            .starts_with("37 curve ends meet at point ")
    );
}

#[test]
fn loops_at_the_kilometre_limit_keep_their_areas() {
    // The square as far out as a point may be, a hole r 1 a millimetre in from its corner,
    // and one at its centre.
    let mut d = Draw::new();
    d.rectangle(-MAX_LENGTH, -MAX_LENGTH, MAX_LENGTH, MAX_LENGTH);
    d.circle(MAX_LENGTH - 2.0, MAX_LENGTH - 2.0, 1.0);
    d.circle(0.0, 0.0, 1.0);
    let p = d.made();
    assert_eq!(p.regions[0].holes.len(), 2);
    assert!(close(p.regions[0].area(), 4e12 - 2.0 * PI, 1e-15));

    // A small square out at the limit: its area from coordinates a kilometre out.
    let mut d = Draw::new();
    let x = MAX_LENGTH - 10.0;
    d.rectangle(x, x, x + 3.0, x + 4.0);
    assert!(close(d.made().regions[0].area(), 12.0, 1e-9));
}

#[test]
fn an_arc_of_nearly_a_whole_turn_and_its_short_chord() {
    // An arc r 2 from 1e-3 rad round to −1e-3 rad, closed by the chord between its ends,
    // about 4e-3 long: the segment (r²/2)(θ − sin θ), θ = 2π − 2e-3.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(2.0 * libm::cos(1e-3), 2.0 * libm::sin(1e-3));
    let e = d.point(2.0 * libm::cos(1e-3), -2.0 * libm::sin(1e-3));
    d.arc(&o, &s, &e);
    d.line(&e, &s);
    let theta = 2.0 * PI - 2e-3;
    let area = 2.0 * (theta - libm::sin(theta));
    assert!(close(d.made().regions[0].area(), area, 1e-9));
}

#[test]
fn two_arcs_of_one_circle_end_to_end_at_unshared_points_should_share_them() {
    // The half circles from (2, 0) to (−2, 0) and back, each with its own ends: the ends
    // are at one place, so each pair should be one point; nothing else is wrong.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let (s, e) = (d.point(2.0, 0.0), d.point(-2.0, 0.0));
    let (e2, s2) = (d.point(-2.0, 0.0), d.point(2.0, 0.0));
    d.arc(&o, &s, &e);
    d.arc(&o, &e2, &s2);
    let errors = d.refused();
    assert_eq!(errors.len(), 2, "{:#?}", named(&errors));
    assert!(
        errors
            .iter()
            .all(|e| e.suggestion() == Some("share the point: end the curves at one point"))
    );
    assert_eq!(errors[0].elements()[..2], [s.clone(), s2.clone()]);
    assert_eq!(errors[1].elements()[..2], [e.clone(), e2.clone()]);
}

#[test]
fn a_line_through_a_circle_off_an_arc_of_it_is_apart_from_the_arc() {
    // The arc r 1 from (0, 1) round to (0, −1) by the left (x ≤ 0), and well away a
    // triangle whose side crosses the arc's circle at x = 0.5, off the arc: the triangle
    // and the arc and its chord are two regions.
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let top = d.point(0.0, 1.0);
    let bottom = d.point(0.0, -1.0);
    d.arc(&o, &top, &bottom);
    d.line(&bottom, &top);
    d.polygon(&[(0.5, -3.0), (0.5, 3.0), (3.0, 0.0)]);
    let p = d.made();
    let mut areas: Vec<f64> = p.regions.iter().map(Region::area).collect();
    areas.sort_by(f64::total_cmp);
    assert!(close(areas[0], PI / 2.0, 1e-12));
    assert!(close(areas[1], 7.5, 1e-12));
}

#[test]
fn a_solution_that_collapses_a_curve_is_refused_naming_it() {
    // A line whose ends the solve puts at one place, and a circle solved to radius 0.
    let mut d = Draw::new();
    let n = d.rectangle(0.0, 0.0, 4.0, 3.0);
    let c = d.circle(2.0, 1.5, 0.5);
    let mut solution = d.solution();
    let at = |x: f64, y: f64| [Coordinate::new(x).unwrap(), Coordinate::new(y).unwrap()];
    solution.points.insert(id("p0000002"), at(0.0, 0.0));
    solution.radii.insert(id(&c), Coordinate::new(0.0).unwrap());
    let r = d.profile_from(&solution).unwrap_err();
    // Too small to build, each named with its points, by curve ID (the circle's c… first).
    assert_eq!(
        named(r.errors()),
        vec![
            (
                format!(
                    "circle {c}'s radius is 0 mm: a profile's arcs and circles have a radius above 1e-7 mm"
                ),
                vec![c.clone(), "p0000009".to_owned()]
            ),
            (
                format!(
                    "line {} is 0 mm long: a profile's lines and arcs are longer than 1e-7 mm between their ends",
                    n[0]
                ),
                vec![n[0].clone(), "p0000001".to_owned(), "p0000002".to_owned()]
            ),
        ]
    );
    assert_eq!(
        r.errors()[0].suggestion(),
        Some("draw it larger, or remove it")
    );
    assert_eq!(
        r.errors()[1].suggestion(),
        Some("lengthen it, or remove it and end its neighbours at one point")
    );

    // An arc whose end the solve puts on its start (and so its chord's ends too).
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let s = d.point(1.0, 0.0);
    let e = d.point(0.0, 1.0);
    let a = d.arc(&o, &s, &e);
    let chord = d.line(&e, &s);
    let mut solution = d.solution();
    solution.points.insert(id(&e), at(1.0, 0.0));
    let r = d.profile_from(&solution).unwrap_err();
    let got: Vec<Vec<String>> = r.errors().iter().map(|e| e.elements().to_vec()).collect();
    assert_eq!(
        got,
        vec![vec![a, o, s.clone(), e.clone()], vec![chord, e, s]]
    );
    assert!(
        r.errors()[0]
            .message()
            .contains("sweeps nothing, or a whole turn")
    );
    assert!(r.errors()[1].message().contains(" is 0 mm long: "));
}

#[test]
fn the_same_profile_from_a_far_off_origin() {
    // A slot with a hole, moved half a kilometre: the same areas.
    let shape = |dx: f64| {
        let mut d = Draw::new();
        d.rectangle(dx, dx, dx + 20.0, dx + 10.0);
        d.circle(dx + 5.0, dx + 5.0, 2.0);
        let o = d.point(dx + 14.0, dx + 5.0);
        let s = d.point(dx + 16.0, dx + 5.0);
        let e = d.point(dx + 12.0, dx + 5.0);
        d.arc(&o, &s, &e);
        d.line(&e, &s);
        d.made()
    };
    let near: f64 = shape(0.0).regions.iter().map(Region::area).sum();
    let far: f64 = shape(5e5).regions.iter().map(Region::area).sum();
    assert!(close(near, 200.0 - 4.0 * PI - 2.0 * PI, 1e-12));
    assert!(close(far, near, 1e-9));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn any_list_order_gives_the_same_profile(seed in any::<u64>()) {
        let mut d = Draw::new();
        d.rectangle(0.0, 0.0, 100.0, 100.0);
        d.circle(20.0, 20.0, 5.0);
        d.rectangle(50.0, 50.0, 90.0, 90.0);
        d.circle(70.0, 70.0, 10.0);
        d.polygon(&[(10.0, 60.0), (30.0, 60.0), (20.0, 80.0)]);
        let o = d.point(70.0, 20.0);
        let s = d.point(80.0, 20.0);
        let e = d.point(60.0, 20.0);
        d.arc(&o, &s, &e);
        d.arc(&o, &e, &s);
        let expected = d.made();
        // A shuffle by the seed: each list rotated and maybe reversed.
        let shuffle = |n: usize, k: u64| -> Vec<usize> {
            let mut v: Vec<usize> = (0..n).collect();
            v.rotate_left((k as usize) % n.max(1));
            if k & 1 == 1 {
                v.reverse();
            }
            v
        };
        let s = &mut d.sketch;
        let order = shuffle(s.points.len(), seed);
        s.points = order.iter().map(|&i| s.points[i].clone()).collect();
        let order = shuffle(s.lines.len(), seed >> 8);
        s.lines = order.iter().map(|&i| s.lines[i].clone()).collect();
        let order = shuffle(s.circles.len(), seed >> 16);
        s.circles = order.iter().map(|&i| s.circles[i].clone()).collect();
        let order = shuffle(s.arcs.len(), seed >> 24);
        s.arcs = order.iter().map(|&i| s.arcs[i].clone()).collect();
        prop_assert_eq!(d.made(), expected);
    }
}
