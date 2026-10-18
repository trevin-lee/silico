//! M3.5 `/break-it`, profiles to faces through OCCT: hostile sketches (corner arcs, arcs
//! and short chords, small circles, holes near their outline, a kilometre out, turned, on
//! every plane) whose profiles the core makes must build as valid faces of their area in
//! closed form, with the history the fake kernel claims; neighbours the tolerance decision
//! lets run within 1e-7 of each other near their shared point (a spike, a crack); and
//! regions the core would never send (a hole outside its outline, or across it), which the
//! kernel says it leaves to OCCT's check.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/profile.rs"]
mod draw;
#[path = "../../silico-core/tests/common/hostile_profiles.rs"]
mod hostile;

use std::f64::consts::PI;
use std::sync::OnceLock;
use std::time::Duration;

use draw::{Draw, feature};
use hostile::{Piece, hostile};
use proptest::prelude::*;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Kernel, KernelRequest};
use silico_core::profile::{Profile, profile};
use silico_kernel::{Curve, FaceRegions, Region, SketchPlane, make_faces, summarize};

/// The profile, if the gate takes the sketch and the core makes one.
fn made(d: &Draw) -> Option<Profile> {
    let checked = d.checked().ok()?;
    profile(&checked, &feature(), &d.solution()).ok()
}

/// The regions the core sends for `d`'s profile, through the request's check.
fn regions(p: &Profile) -> FaceRegions {
    let regions = p.faces();
    KernelRequest::make_faces(&regions).unwrap();
    regions
}

/// The faces' area, which OCCT must find valid.
fn area(regions: &FaceRegions) -> Result<f64, String> {
    let built = make_faces(regions).map_err(|e| format!("{e:?}"))?;
    let s = summarize(&built.shape).unwrap();
    if !s.valid {
        return Err("built, but not valid".to_owned());
    }
    Ok(s.area)
}

fn shape<K: Kernel>(kernel: &K, regions: &FaceRegions) -> silico_core::kernel::ShapeData {
    kernel
        .submit(
            kernel.generation(),
            KernelRequest::make_faces(regions).unwrap(),
        )
        .wait_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("no outcome within a minute"))
        .done()
        .unwrap()
        .unwrap()
        .shape
}

fn pool() -> silico_core::pool::KernelPool {
    silico_core::pool::KernelPool::start_workers(
        std::path::Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")),
        1,
    )
    .unwrap()
}

fn fake() -> &'static FakeKernel {
    static FAKE: OnceLock<FakeKernel> = OnceLock::new();
    FAKE.get_or_init(|| FakeKernel::new(1).unwrap())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Every hostile sketch whose profile the core makes and sends (the request's own
    /// disagreements, tests/adversarial_profile_faces.rs in silico-core, left out) builds
    /// valid faces of its area in closed form, with the history the fake kernel gives.
    #[test]
    fn a_profile_the_core_sends_builds_valid_faces_of_its_area(h in hostile()) {
        let known = h.pieces.iter().any(|p| match *p {
            Piece::Rounded { r, .. } => r.iter().any(|&r| r > 0.0 && r <= 1.01e-7),
            Piece::D { r, half, .. } => 2.0 * r * libm::sin(half) <= 1.5e-7 || r <= 1.01e-7,
            _ => false,
        });
        if known {
            return Ok(());
        }
        let d = h.draw();
        let Some(p) = made(&d) else {
            return Ok(());
        };
        let sent = p.faces();
        if KernelRequest::make_faces(&sent).is_err() {
            // The core's own seam (the strip at the sliver boundary): its test's.
            return Ok(());
        }
        let built = make_faces(&sent);
        prop_assert!(built.is_ok(), "{:?}", built.err());
        let built = built.unwrap();
        let s = summarize(&built.shape).unwrap();
        prop_assert!(s.valid, "not valid");
        prop_assert!(
            (s.area - h.area()).abs() <= h.area_tolerance().max(1e-9 * h.area()),
            "OCCT's area {} against {} (±{})", s.area, h.area(), h.area_tolerance()
        );
        let faked = fake()
            .submit(fake().generation(), KernelRequest::make_faces(&sent).unwrap())
            .wait_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("no outcome within a minute"))
            .done()
            .unwrap()
            .unwrap();
        prop_assert_eq!(&faked.history, &built.history);
    }
}

#[test]
fn a_spike_whose_sides_run_within_the_tolerance_near_its_tip_builds_its_area() {
    // A 10 × 10 square with a spike 20 long out of its lower right, its sides 2e-7 apart
    // at its base, so within 1e-7 of each other for the 10 mm nearest its tip (the
    // decision's neighbours, which only an intersection away from the tip would refuse).
    // Shoelace: (0,0), (30,0), (10,h), (10,10), (0,10) is 100 + 10h.
    let h = 2e-7;
    let mut d = Draw::new();
    d.polygon(&[
        (0.0, 0.0),
        (30.0, 0.0),
        (10.0, h),
        (10.0, 10.0),
        (0.0, 10.0),
    ]);
    let p = made(&d).expect("the decision takes neighbours near their shared point");
    let got = area(&regions(&p));
    assert!(
        got.as_ref()
            .is_ok_and(|a| (a - (100.0 + 10.0 * h)).abs() < 1e-12),
        "{got:?}"
    );
}

#[test]
fn a_crack_whose_sides_run_within_the_tolerance_near_its_tip_builds_its_area() {
    // The same cut into a 10 × 10 square from the middle of its top: a slit 9.5 deep and
    // 2e-7 wide at the top. 100 − 9.5h/2.
    let h = 2e-7;
    let mut d = Draw::new();
    d.polygon(&[
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (5.0 + h, 10.0),
        (5.0, 0.5),
        (5.0, 10.0),
        (0.0, 10.0),
    ]);
    let p = made(&d).expect("the decision takes neighbours near their shared point");
    let got = area(&regions(&p));
    assert!(
        got.as_ref()
            .is_ok_and(|a| (a - (100.0 - 9.5 * h / 2.0)).abs() < 1e-12),
        "{got:?}"
    );
}

#[test]
fn two_arcs_from_one_point_on_circles_3e_7_apart_build_their_area() {
    // Arcs of radius 10 from (10, 0) a quarter turn round, one about the origin and one
    // about (0, −3e-7) (through (10, 0) too), the inner one a hundredth of a radian
    // further, closed through (−20, 20): within 1e-7 of each other for the 3.4 mm nearest
    // the shared point and 3e-7 apart at the top, crossing nowhere else on both (their
    // circles meet again at (−10, 0)). If the core makes it, OCCT must build it, valid,
    // with the area the profile works out.
    let mut d = Draw::new();
    let s = d.point(10.0, 0.0);
    let o1 = d.point(0.0, 0.0);
    let e1 = d.point(0.0, 10.0);
    let c2 = [0.0, -3e-7];
    let r2 = (100.0f64 + 9e-14).sqrt();
    let o2 = d.point(c2[0], c2[1]);
    let turn = PI / 2.0 + 0.01;
    let e2 = d.point(c2[0] + r2 * libm::cos(turn), c2[1] + r2 * libm::sin(turn));
    d.arc(&o1, &s, &e1);
    d.arc(&o2, &s, &e2);
    let far = d.point(-20.0, 20.0);
    d.line(&e1, &far);
    d.line(&far, &e2);
    let p = made(&d).expect("the decision takes neighbours near their shared point");
    let regions = regions(&p);
    let profile_area: f64 = p.regions.iter().map(|r| r.area()).sum();
    let got = area(&regions);
    assert!(
        got.as_ref()
            .is_ok_and(|a| (a - profile_area).abs() < 1e-9 * profile_area.abs().max(1.0)),
        "{got:?}, the profile's {profile_area}"
    );
}

fn square(x0: f64, y0: f64, side: f64) -> Vec<Curve> {
    let p = [
        [x0, y0],
        [x0 + side, y0],
        [x0 + side, y0 + side],
        [x0, y0 + side],
    ];
    (0..4)
        .map(|i| Curve::Line {
            start: p[i],
            end: p[(i + 1) % 4],
        })
        .collect()
}

fn clockwise(curves: Vec<Curve>) -> Vec<Curve> {
    curves
        .into_iter()
        .rev()
        .map(|c| match c {
            Curve::Line { start, end } => Curve::Line {
                start: end,
                end: start,
            },
            other => other,
        })
        .collect()
}

#[test]
fn a_hole_outside_its_outline_is_not_built_as_a_face() {
    // A unit square with a 1 × 1 hole 4 mm away: no face has that hole. The kernel leaves
    // nesting to the core and says OCCT checks the faces it makes; it must not hand back a
    // "valid" face of area 1 − 1 = 0, or 1.
    let regions = FaceRegions {
        plane: SketchPlane::Xy,
        regions: vec![Region {
            outer: square(0.0, 0.0, 1.0),
            holes: vec![clockwise(square(5.0, 5.0, 1.0))],
        }],
    };
    regions.check().unwrap();
    let got = area(&regions);
    assert!(got.is_err(), "built a face of area {got:?}");
}

#[test]
fn a_hole_across_its_outline_is_not_built_as_a_face() {
    // A unit square with a 1 × 1 hole across its corner.
    let regions = FaceRegions {
        plane: SketchPlane::Xy,
        regions: vec![Region {
            outer: square(0.0, 0.0, 1.0),
            holes: vec![clockwise(square(0.5, 0.5, 1.0))],
        }],
    };
    regions.check().unwrap();
    let got = area(&regions);
    assert!(got.is_err(), "built a face of area {got:?}");
}

#[test]
fn a_circle_hole_outside_its_outline_is_not_built_as_a_face() {
    let regions = FaceRegions {
        plane: SketchPlane::Xz,
        regions: vec![Region {
            outer: square(0.0, 0.0, 1.0),
            holes: vec![vec![Curve::Circle {
                center: [5.0, 5.0],
                radius: 0.25,
            }]],
        }],
    };
    regions.check().unwrap();
    let got = area(&regions);
    assert!(
        got.is_err(),
        "built a face of area {got:?}, 1 − π/16 = {}",
        1.0 - PI / 16.0
    );
}

#[test]
fn a_circle_with_itself_as_its_hole_is_not_built_as_a_face() {
    // The outline and its hole one circle: no points, so the request's check (which
    // compares points) can't see it; a face of no area must not come back valid.
    let circle = Curve::Circle {
        center: [1.0, 2.0],
        radius: 3.0,
    };
    let regions = FaceRegions {
        plane: SketchPlane::Yz,
        regions: vec![Region {
            outer: vec![circle],
            holes: vec![vec![circle]],
        }],
    };
    regions.check().unwrap();
    let got = area(&regions);
    assert!(got.is_err(), "built a face of area {got:?}");
}

#[test]
fn a_hole_whose_side_lies_along_its_outlines_is_not_built_as_a_face() {
    // A 2 × 2 hole sharing the outline's bottom side's line from x = 1 to 3, its corners
    // off the outline's points (so the check, which compares points, passes it).
    let regions = FaceRegions {
        plane: SketchPlane::Xy,
        regions: vec![Region {
            outer: square(0.0, 0.0, 4.0),
            holes: vec![clockwise(square(1.0, 0.0, 2.0))],
        }],
    };
    regions.check().unwrap();
    let got = area(&regions);
    assert!(
        got.is_err(),
        "built a face of area {got:?} (16 − 4 = 12 if it ignored the touch)"
    );
}

#[test]
fn faces_spelled_with_negative_zeros_are_one_shape_to_the_fake_as_to_the_kernel() {
    // The unit square from the origin, its zeros +0 or −0 (a solve can leave −0): the
    // kernel places each point with its zeros as 0, so the two are one set of bytes;
    // the fake must agree, or its caches and fingerprints see two shapes where the real
    // kernel sees one.
    let regions = |z: f64| FaceRegions {
        plane: SketchPlane::Xy,
        regions: vec![Region {
            outer: square(z, z, 1.0)
                .into_iter()
                .map(|c| match c {
                    Curve::Line { start, end } => Curve::Line {
                        start: start.map(|v| if v == 0.0 { z } else { v }),
                        end: end.map(|v| if v == 0.0 { z } else { v }),
                    },
                    other => other,
                })
                .collect(),
            holes: vec![],
        }],
    };
    let pool = pool();
    let real = shape(&pool, &regions(0.0)) == shape(&pool, &regions(-0.0));
    let faked = shape(fake(), &regions(0.0)) == shape(fake(), &regions(-0.0));
    assert!(real, "the kernel's bytes differ by the sign of a zero");
    // BREAKS: the fake writes each value as Rust prints it, "-0" for −0, so its bytes
    // differ where the real kernel's (each point placed with its zeros as 0) don't.
    assert_eq!(faked, real, "the fake's bytes differ by the sign of a zero");
}

#[test]
fn an_arc_starting_at_its_centres_left_is_one_shape_whatever_the_sign_of_its_zero() {
    // The lower half of the unit circle, from (−1, 0) round to (1, 0), and its chord: drawn
    // with its start at y = +0 or y = −0 (a solve can leave either), its start angle is
    // atan2(±0, −1) = ±π, so the request differs by a turn while the arc doesn't. The
    // kernel places points with their zeros as 0 so a shape doesn't depend on a zero's
    // sign; the arc's bytes must not either.
    // BREAKS: the core sends start angle π (end 2π) for +0 and −π (end 0) for −0, and the
    // kernel builds the circle's range from them as sent: one arc, two sets of bytes (two
    // cache entries, two shapes to anything comparing bytes). A document keeps −0 apart
    // from 0 (`Coordinate`), and a solve can leave either.
    let shape_of = |zero: f64| {
        let mut d = Draw::new();
        let o = d.point(0.0, 0.0);
        let s = d.point(-1.0, zero);
        let e = d.point(1.0, 0.0);
        d.arc(&o, &s, &e);
        d.line(&e, &s);
        let p = made(&d).unwrap();
        let sent = regions(&p);
        (sent.clone(), shape(&pool(), &sent))
    };
    let ((plus, a), (minus, b)) = (shape_of(0.0), shape_of(-0.0));
    assert!(
        a == b,
        "one arc, two shapes: sent {:?} and {:?}",
        plus.regions[0].outer[0],
        minus.regions[0].outer[0]
    );
}
