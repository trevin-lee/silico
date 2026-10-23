//! M3.5, hostile regions for the kernel's faces (`/break-it`): arcs at OCCT's parametric
//! tolerance and a hair short of a turn, ends off their circle by up to the tolerance,
//! angles a turn on, thousands of sides and holes, and a kilometre across. Each area is
//! worked out in closed form.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::close;
use super::imports;

use std::f64::consts::{PI, TAU};

use close::assert_close;
use silico_kernel::{Curve, FaceRegions, Region, SketchPlane, make_faces, summarize};

fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
    Curve::Line { start, end }
}

fn one(plane: SketchPlane, outer: Vec<Curve>, holes: Vec<Vec<Curve>>) -> FaceRegions {
    FaceRegions {
        plane,
        regions: vec![Region { outer, holes }],
    }
}

/// The point at `angle` on the circle of `radius` about the origin.
fn at(radius: f64, angle: f64) -> [f64; 2] {
    [radius * libm::cos(angle), radius * libm::sin(angle)]
}

/// A sector of the circle of `radius` about the origin, from angle 0 turning `turn`
/// counter-clockwise, closed by its two radii: r²θ/2.
fn sector(radius: f64, turn: f64) -> Vec<Curve> {
    let (start, end) = (at(radius, 0.0), at(radius, turn));
    vec![
        Curve::Arc {
            center: [0.0, 0.0],
            start,
            end,
            radius,
            start_angle: 0.0,
            end_angle: turn,
        },
        line(end, [0.0, 0.0]),
        line([0.0, 0.0], start),
    ]
}

fn area(regions: &FaceRegions) -> f64 {
    let built = make_faces(regions).unwrap();
    let s = summarize(&built.shape).unwrap();
    assert!(s.valid);
    s.area
}

#[test]
fn an_arc_turning_less_than_occts_parametric_tolerance_stays_that_short() {
    // OCCT's edge maker would make an arc turning less than 1e-9 radians a turn longer
    // (ElCLib::AdjustPeriodic): a 0.05 mm arc of radius 100 m, a whole circle, its face
    // valid at 3.1e10 mm². Built with the range as sent, it's the sliver it says.
    let radius = 1e5;
    for turn in [5e-10, 1e-9, 2e-9, 1e-6] {
        for plane in SketchPlane::ALL {
            let regions = one(plane, sector(radius, turn), vec![]);
            assert_close(
                &format!("a turn of {turn} on {plane:?}"),
                area(&regions),
                radius * radius * turn / 2.0,
                1e-6,
            );
        }
    }
}

#[test]
fn an_arc_a_hair_short_of_a_turn_closes_by_its_chord() {
    // From angle 0 round to 1e-9 short of a turn, r 1 m: a chord 1e-6 long closes it, the
    // circle less a sliver, (r²/2)(θ − sin θ).
    let (radius, turn) = (1e3, TAU - 1e-9);
    let (start, end) = (at(radius, 0.0), at(radius, turn));
    let outer = vec![
        Curve::Arc {
            center: [0.0, 0.0],
            start,
            end,
            radius,
            start_angle: 0.0,
            end_angle: turn,
        },
        line(end, start),
    ];
    let want = radius * radius / 2.0 * (turn - libm::sin(turn));
    assert_close(
        "nearly a circle",
        area(&one(SketchPlane::Xz, outer, vec![])),
        want,
        1e-9,
    );
}

#[test]
fn ends_off_their_circle_within_the_tolerance_still_share_their_vertex() {
    // A half disc of r 2, its arc's end 9e-8 off where its angle puts it (the solve's
    // convergence is about 1e-10; this is the tolerance's edge), and its chord from there.
    for plane in SketchPlane::ALL {
        let end = [-2.0, 9e-8];
        let outer = vec![
            Curve::Arc {
                center: [0.0, 0.0],
                start: [2.0, 0.0],
                end,
                radius: 2.0,
                start_angle: 0.0,
                end_angle: PI,
            },
            line(end, [2.0, 0.0]),
        ];
        let built = make_faces(&one(plane, outer, vec![])).unwrap();
        let s = summarize(&built.shape).unwrap();
        assert!(s.valid, "{plane:?}");
        assert_eq!((s.edges, s.vertices), (2, 2), "{plane:?}");
        assert_close("half disc", s.area, 2.0 * PI, 1e-7);
    }
}

#[test]
fn an_arcs_angles_a_turn_on_or_back_make_the_same_face() {
    // The quarter disc of r 3, its arc's angles as sent, a turn on, and a turn back.
    let quarter = |shift: f64| {
        vec![
            Curve::Arc {
                center: [0.0, 0.0],
                start: [3.0, 0.0],
                end: at(3.0, PI / 2.0),
                radius: 3.0,
                start_angle: shift,
                end_angle: PI / 2.0 + shift,
            },
            line(at(3.0, PI / 2.0), [0.0, 0.0]),
            line([0.0, 0.0], [3.0, 0.0]),
        ]
    };
    for shift in [0.0, TAU, -TAU] {
        assert_close(
            &format!("shifted {shift}"),
            area(&one(SketchPlane::Yz, quarter(shift), vec![])),
            9.0 * PI / 4.0,
            1e-12,
        );
    }
}

#[test]
fn thousands_of_sides_and_hundreds_of_holes_are_built_and_measured() {
    // A regular 2000-gon of circumradius 100: (n/2) r² sin(2π/n).
    let n = 2000;
    let corner = |i: usize| at(100.0, TAU * i as f64 / n as f64);
    let polygon: Vec<Curve> = (0..n)
        .map(|i| line(corner(i), corner((i + 1) % n)))
        .collect();
    let want = n as f64 / 2.0 * 1e4 * libm::sin(TAU / n as f64);
    assert_close(
        "2000-gon",
        area(&one(SketchPlane::Xy, polygon, vec![])),
        want,
        1e-12,
    );
    // A 1000 × 1000 square with 500 holes of r 1, a 10 mm grid.
    let p = [[0.0, 0.0], [1000.0, 0.0], [1000.0, 1000.0], [0.0, 1000.0]];
    let square: Vec<Curve> = (0..4).map(|i| line(p[i], p[(i + 1) % 4])).collect();
    let holes: Vec<Vec<Curve>> = (0..500)
        .map(|i| {
            let (x, y) = ((i % 50) as f64, (i / 50) as f64);
            vec![Curve::Circle {
                center: [5.0 + 10.0 * x, 5.0 + 10.0 * y],
                radius: 1.0,
            }]
        })
        .collect();
    let regions = one(SketchPlane::Xz, square, holes);
    let built = make_faces(&regions).unwrap();
    let s = summarize(&built.shape).unwrap();
    assert_eq!((s.faces, s.edges, s.vertices), (1, 504, 504));
    assert_close("holed square", s.area, 1e6 - 500.0 * PI, 1e-12);
}

#[test]
fn a_profile_a_kilometre_across_is_built() {
    let p = [[-1e6, -1e6], [1e6, -1e6], [1e6, 1e6], [-1e6, 1e6]];
    let square: Vec<Curve> = (0..4).map(|i| line(p[i], p[(i + 1) % 4])).collect();
    let hole = vec![Curve::Circle {
        center: [9.99e5, 0.0],
        radius: 1e-3,
    }];
    assert_close(
        "two kilometres square",
        area(&one(SketchPlane::Xy, square, vec![hole])),
        4e12 - PI * 1e-6,
        1e-12,
    );
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}
