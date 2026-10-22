//! M3.5 `/break-it`, the seam between the core's profile and the kernel's faces request: a
//! profile the core makes must be one the request takes (`FaceRegions::check`), or the user
//! is told "this is a bug in the profile" about a sketch whose curves it never named. Corner
//! arcs at OCCT's tolerance, an arc and its chord with the chord within it, slivers at the
//! boundary each side computes its own way, and a property over hostile sketches (each
//! area in closed form). No kernel worker: the request's check runs in the core.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/profile.rs"]
mod draw;
#[path = "common/hostile_profiles.rs"]
mod hostile;

use std::f64::consts::PI;

use draw::{Draw, feature, named};
use hostile::{Hostile, hostile};
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::kernel::KernelRequest;
use silico_core::profile::{Profile, Region, profile};

/// The sketch's profile, or `None` where the gate refuses the sketch's structure (a case
/// the profile never sees). Its sizes are the solution's: the gate's floor on drawn sizes
/// aside, so the profile's own rules meet hostile sizes.
fn profile_of(d: &Draw) -> Option<Result<Profile, silico_core::profile::Refused>> {
    let checked = d.structure().ok()?;
    Some(profile(&checked, &feature(), &d.solution()))
}

/// The kernel request for the profile's faces, as the core would make it: the error it
/// refuses them with, if it does.
fn request(p: &Profile) -> Result<(), silico_core::Error> {
    let regions = p.faces();
    KernelRequest::make_faces(&regions).map(|_| ())
}

/// That a profile the core made is one the kernel request takes; or, where the core
/// refuses the sketch, that it names the curve at fault.
fn made_and_sent_or_refused_naming(d: &Draw, curve: &str) {
    match d.profile() {
        Ok(p) => {
            if let Err(e) = request(&p) {
                panic!(
                    "the profile was made, but the kernel request refuses it: {} ({:?}; suggestion {:?})",
                    e.message(),
                    e.parameter(),
                    e.suggestion()
                );
            }
        }
        Err(r) => {
            assert!(
                r.errors()
                    .iter()
                    .any(|e| e.code() == ErrorCode::InvalidInput
                        && e.elements().iter().any(|n| n == curve)),
                "refused without naming {curve}: {:#?}",
                named(r.errors())
            );
        }
    }
}

/// A 10 × 10 square whose lower right corner is rounded by an arc of radius `r`: its arc's
/// ID.
fn square_with_a_rounded_corner(d: &mut Draw, r: f64) -> String {
    let a = d.point(0.0, 0.0);
    let b = d.point(10.0 - r, 0.0);
    let c = d.point(10.0, r);
    let top = d.point(10.0, 10.0);
    let left = d.point(0.0, 10.0);
    let o = d.point(10.0 - r, r);
    d.line(&a, &b);
    let arc = d.arc(&o, &b, &c);
    d.line(&c, &top);
    d.line(&top, &left);
    d.line(&left, &a);
    arc
}

#[test]
fn a_corner_rounded_by_an_arc_of_radius_1e_7_is_made_and_sent_or_refused_naming_the_arc() {
    // BREAKS: the profile takes a corner arc of radius r in (1e-7/√2, 1e-7] (its ends are
    // r√2 apart, more than the tolerance, so no neighbour comes within it), but
    // `FaceRegions::check` refuses any radius not above 1e-7: the user gets "curve 2 (curve
    // 2 of region 1's outer loop)'s radius must be a length above 1e-7 mm ... this is a bug
    // in the profile", naming no element of the sketch. Analytic: the ends are r√2 apart,
    // 1.41e-7 at r = 1e-7.
    for r in [1e-7, 9e-8, 7.1e-8] {
        let mut d = Draw::new();
        let arc = square_with_a_rounded_corner(&mut d, r);
        made_and_sent_or_refused_naming(&d, &arc);
    }
}

#[test]
fn a_corner_rounded_by_an_arc_just_above_the_tolerance_is_made_and_sent() {
    // The next radius the request takes: its area is the square's less (1 − π/4) r².
    let r = 1.0000000000000001e-7;
    let mut d = Draw::new();
    square_with_a_rounded_corner(&mut d, r);
    let p = d.made();
    request(&p).unwrap();
    let area: f64 = p.regions.iter().map(Region::area).sum();
    assert!(
        (area - (100.0 - (1.0 - PI / 4.0) * r * r)).abs() < 1e-12,
        "{area}"
    );
}

#[test]
fn a_corner_arc_whose_ends_are_within_the_tolerance_is_refused_naming_it() {
    // r = 7e-8: the arc's ends are 9.9e-8 apart, so the sides it joins come within the
    // tolerance of each other at their ends.
    let mut d = Draw::new();
    let arc = square_with_a_rounded_corner(&mut d, 7e-8);
    let errors = d.refused();
    assert!(
        errors.iter().all(|e| e.code() == ErrorCode::InvalidInput),
        "{:#?}",
        named(&errors)
    );
    // The two sides are named; the arc between them needn't be.
    assert!(!errors.is_empty(), "{arc}");
}

/// An arc of radius `r` about the origin the long way round from +`half` to −`half`, closed
/// by its chord: the arc's and the chord's IDs.
fn arc_and_short_chord(d: &mut Draw, r: f64, half: f64) -> (String, String) {
    let o = d.point(0.0, 0.0);
    let s = d.point(r * libm::cos(half), r * libm::sin(half));
    let e = d.point(r * libm::cos(half), -r * libm::sin(half));
    let arc = d.arc(&o, &s, &e);
    let chord = d.line(&e, &s);
    (arc, chord)
}

#[test]
fn an_arc_closed_by_a_chord_within_the_tolerance_is_made_and_sent_or_refused_naming_the_chord() {
    // BREAKS: a loop of two curves whose shared points are 5e-8 apart (a line and an arc
    // of nearly a whole turn) has no end that isn't shared, so no neighbour test fires and
    // the profile is made; `FaceRegions::check` then refuses the 5e-8 mm line ("curve 2
    // (curve 2 of region 1's outer loop)'s ends are 5e-8 mm apart ... this is a bug in the
    // profile"). In any loop of three or more such a line is refused, named, as its far
    // end within its length of its other neighbour. Analytic: the chord is 2 sin(2.5e-8) =
    // 5e-8.
    for half in [2.5e-8, 5e-8] {
        let mut d = Draw::new();
        let (_, chord) = arc_and_short_chord(&mut d, 1.0, half);
        made_and_sent_or_refused_naming(&d, &chord);
    }
}

#[test]
fn two_arcs_whose_shared_ends_are_within_the_tolerance_are_made_and_sent_or_refused() {
    // BREAKS: the same with the chord an arc of another circle (radius 1, about (2, 0),
    // through both ends): two arcs sharing both points 5e-8 apart. The profile is made;
    // the request refuses the short arc ("its ends are 5e-8 mm apart").
    let half: f64 = 2.5e-8;
    let mut d = Draw::new();
    let o = d.point(0.0, 0.0);
    let (x, y) = (libm::cos(half), libm::sin(half));
    let s = d.point(x, y);
    let e = d.point(x, -y);
    d.arc(&o, &s, &e);
    // The other circle about (2x, 0), through both ends: counter-clockwise from s (at
    // π − 2.5e-8 about it) to e (at −π + 2.5e-8) is its short arc, bulging 1e-15 inwards.
    let o2 = d.point(2.0 * x, 0.0);
    let small = d.arc(&o2, &s, &e);
    made_and_sent_or_refused_naming(&d, &small);
}

#[test]
fn an_arc_closed_by_a_chord_just_above_the_tolerance_is_made_and_sent() {
    // The chord 2 sin(half) just above 1e-7: both halves take it.
    let mut d = Draw::new();
    arc_and_short_chord(&mut d, 1.0, 5.0000001e-8);
    let p = d.made();
    request(&p).unwrap();
    let theta = 2.0 * PI - 2.0 * 5.0000001e-8;
    let area = (theta - libm::sin(theta)) / 2.0;
    let got: f64 = p.regions.iter().map(Region::area).sum();
    assert!((got - area).abs() < 1e-12, "{got} {area}");
}

/// A `length` × `width` rectangle turned by `angle` about (x0, y0), drawn counter-clockwise
/// (or clockwise, `cw`).
fn turned_strip(d: &mut Draw, x0: f64, y0: f64, length: f64, width: f64, angle: f64, cw: bool) {
    let (c, s) = (libm::cos(angle), libm::sin(angle));
    let at = |x: f64, y: f64| (x0 + c * x - s * y, y0 + s * x + c * y);
    let mut corners = vec![
        at(0.0, 0.0),
        at(length, 0.0),
        at(length, width),
        at(0.0, width),
    ];
    if cw {
        corners.reverse();
    }
    d.polygon(&corners);
}

#[test]
fn a_strip_at_the_sliver_boundary_the_profile_takes_is_one_the_request_takes() {
    // Bisect each turned strip's width to the last the profile refuses as a sliver and the
    // first it takes; the request's check must take that one too. The two sides work out
    // the area about different first points (a loop walked against its drawing starts at
    // the other end of its first curve) and the perimeter by sqrt and by hypot, so they
    // can disagree exactly at the boundary.
    // BREAKS: on turned strips 12 to 15 long, the first width the profile takes (about
    // 2.00000003e-7) the request refuses as "region 1's outer loop encloses no area to
    // speak of (2.800000038632769e-6 mm², against a perimeter of 28.00000040000001 mm)",
    // "a bug in the profile": the shoelace sums' rounding (products of coordinates ~10
    // cancelling to ~1e-6) is ~1e-8 relative, and each side takes its own first point.
    let mut disagree = Vec::new();
    for k in 0..120 {
        let angle = 0.05 + f64::from(k) * 0.0517;
        let (x0, y0) = if k % 3 == 0 {
            (0.0, 0.0)
        } else if k % 3 == 1 {
            (1234.5, -987.25)
        } else {
            (9.1e5, 3.3e5)
        };
        let cw = k % 2 == 1;
        let length = 10.0 + f64::from(k % 7);
        let takes = |width: f64| {
            let mut d = Draw::new();
            turned_strip(&mut d, x0, y0, length, width, angle, cw);
            match profile_of(&d) {
                Some(Ok(p)) => Some((d, p)),
                _ => None,
            }
        };
        let (mut lo, mut hi) = (1e-7, 4e-7);
        if takes(lo).is_some() || takes(hi).is_none() {
            continue;
        }
        for _ in 0..80 {
            let mid = (lo + hi) / 2.0;
            if mid == lo || mid == hi {
                break;
            }
            if takes(mid).is_some() {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let (_, p) = takes(hi).unwrap();
        if let Err(e) = request(&p) {
            disagree.push((k, hi, e.message().to_owned()));
        }
    }
    assert!(disagree.is_empty(), "{disagree:#?}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    /// Every hostile sketch the profile takes, the kernel request takes too, with its area
    /// in closed form; every one it refuses, it refuses as the user's to fix (invalid
    /// input naming the sketch, with a suggestion), never as a bug.
    #[test]
    fn a_profile_the_core_makes_is_one_the_kernel_request_takes(h in hostile()) {
        let d = h.draw();
        let Some(made) = profile_of(&d) else {
            return Ok(());
        };
        match made {
            Ok(p) => {
                // BREAKS: shrinks to a corner arc of radius 7.1e-8 to 1e-7, or an arc and its
                // chord 5e-8 long (the targeted tests above).
                if let Err(e) = request(&p) {
                    prop_assert!(false, "made, but the request refuses it: {}", e.message());
                }
                let area: f64 = p.regions.iter().map(Region::area).sum();
                prop_assert!(
                    (area - h.area()).abs() <= h.area_tolerance(),
                    "area {} against {} (±{})", area, h.area(), h.area_tolerance()
                );
            }
            Err(r) => {
                for e in r.errors() {
                    prop_assert_eq!(e.code(), ErrorCode::InvalidInput, "{}", e);
                    prop_assert_eq!(e.feature(), Some(draw::FEATURE));
                    prop_assert!(!e.elements().is_empty(), "{}", e);
                    prop_assert!(e.suggestion().is_some(), "{}", e);
                }
            }
        }
    }

    /// The same, with the request's two known disagreements left out (corner arcs of
    /// radius about 1e-7 or less, and chords 1e-7 or shorter): what else the seam lets
    /// through.
    #[test]
    fn apart_from_the_known_disagreements_the_profile_and_request_agree(h in hostile()) {
        use hostile::Piece;
        let known = h.pieces.iter().any(|p| match *p {
            // (1e-7 and the next f64 above it, at most, as rounding a kilometre out
            // takes them below.)
            Piece::Rounded { r, .. } => r.iter().any(|&r| r > 0.0 && r <= 1.01e-7),
            Piece::D { r, half, .. } => 2.0 * r * libm::sin(half) <= 1.5e-7 || r <= 1.01e-7,
            _ => false,
        });
        if known {
            return Ok(());
        }
        let d = h.draw();
        let Some(Ok(p)) = profile_of(&d) else {
            return Ok(());
        };
        if let Err(e) = request(&p) {
            prop_assert!(false, "made, but the request refuses it: {}", e.message());
        }
        let area: f64 = p.regions.iter().map(Region::area).sum();
        prop_assert!(
            (area - h.area()).abs() <= h.area_tolerance(),
            "area {} against {} (±{})", area, h.area(), h.area_tolerance()
        );
    }
}

#[test]
fn hostile_sketches_mostly_make_profiles() {
    // That the generator reaches both sides: of a fixed sample, some are made and some
    // refused (else the properties above test nothing).
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let (mut made, mut refused) = (0, 0);
    for _ in 0..300 {
        let h: Hostile = hostile().new_tree(&mut runner).unwrap().current();
        match profile_of(&h.draw()) {
            Some(Ok(_)) => made += 1,
            Some(Err(_)) => refused += 1,
            None => {}
        }
    }
    assert!(made > 50 && refused > 20, "{made} made, {refused} refused");
}

#[test]
fn a_curve_whose_own_ends_are_within_the_tolerance_is_not_told_to_share_the_point() {
    // Was a break: an arc of radius 1 from 2.5e-8 rad round to −2.5e-8 (its ends 5e-8
    // apart, nothing else drawn), or a lone line 5e-8 long, had both its ends open and
    // within the tolerance, so the refusal was "points p, q end curves within 1e-7 mm of
    // each other, but aren't one point", advising "share the point: end the curves at one
    // point". Followed, that's an arc that starts and ends at one point or a line from a
    // point to itself, which the gate refuses: a dead end (the arc wants to be a circle,
    // the line to be longer). It also named the curve twice ([p, q, a, a]).
    //
    // Now (the coordinator's decision 2, 2026-10-18: a curve whose own ends are within
    // the tolerance is too small to build, the request's own rule) it's refused as that,
    // before open ends are looked at, naming the curve and its points once each, with
    // advice that leads somewhere, and nothing says "share the point". The test looked for
    // the "aren't one point" refusal; that refusal no longer exists for one curve's ends,
    // so it checks the size refusal instead, keeping both of its checks.
    let half: f64 = 2.5e-8;
    let mut arc = Draw::new();
    let o = arc.point(0.0, 0.0);
    let s = arc.point(libm::cos(half), libm::sin(half));
    let e = arc.point(libm::cos(half), -libm::sin(half));
    let a = arc.arc(&o, &s, &e);
    let mut line = Draw::new();
    let p = line.point(0.0, 0.0);
    let q = line.point(5e-8, 0.0);
    let l = line.line(&p, &q);
    let cases = [
        (
            &arc,
            vec![a.clone(), o, s, e],
            format!("arc {a}'s ends are "),
            "draw a circle instead, or open the arc",
        ),
        (
            &line,
            vec![l.clone(), p, q],
            format!("line {l} is 5e-8 mm long: "),
            "lengthen it, or remove it and end its neighbours at one point",
        ),
    ];
    for (d, elements, starts, suggestion) in cases {
        let errors = d.refused();
        assert_eq!(errors.len(), 1, "{:#?}", named(&errors));
        let e = &errors[0];
        assert_eq!(e.elements(), elements.as_slice());
        assert_eq!(e.elements_total(), elements.len() as u64);
        let mut unique = elements.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), elements.len(), "an element named twice");
        assert!(e.message().starts_with(&starts), "{e}");
        assert!(
            e.message().ends_with(
                " a profile's lines and arcs are longer than 1e-7 mm between their ends"
            ),
            "{e}"
        );
        assert_eq!(e.suggestion(), Some(suggestion));
        assert!(
            errors.iter().all(|e| !e
                .suggestion()
                .is_some_and(|s| s.contains("share the point"))),
            "{:#?}",
            named(&errors)
        );
    }
}

/// Every number a faces request holds: each curve's values, as `Curve` holds them.
fn sent_values(p: &Profile) -> Vec<f64> {
    use silico_protocol::Curve;
    p.faces()
        .curves()
        .flat_map(|c| match *c {
            Curve::Line { start, end } => vec![start[0], start[1], end[0], end[1]],
            Curve::Arc {
                center,
                start,
                end,
                radius,
                start_angle,
                end_angle,
            } => vec![
                center[0],
                center[1],
                start[0],
                start[1],
                end[0],
                end[1],
                radius,
                start_angle,
                end_angle,
            ],
            Curve::Circle { center, radius } => vec![center[0], center[1], radius],
        })
        .collect()
}

#[test]
fn a_profile_drawn_on_minus_zeros_sends_none() {
    // M3.6, design S3: the faces step's key takes the request's bits as sent, so the
    // request must never hold a −0 (a solve can leave one): two requests of one profile would
    // then be two steps, and an arc's angle from atan2(±0, −1) ±π. A half disc and a
    // square drawn with every zero a −0, the half disc's arc from (1, −0) about (0, −0) to
    // (−1, −0), whose angles would be −0 and −π from the −0s, π and 0 from +0s.
    let mut d = Draw::new();
    let (east, west, center) = (d.point(1.0, -0.0), d.point(-1.0, -0.0), d.point(-0.0, -0.0));
    d.arc(&center, &east, &west);
    d.line(&west, &east);
    d.polygon(&[(-0.0, 3.0), (2.0, 3.0), (2.0, 5.0), (-0.0, 5.0)]);
    let p = profile_of(&d).unwrap().unwrap();
    let values = sent_values(&p);
    assert!(values.contains(&0.0));
    for v in &values {
        assert!(*v != 0.0 || v.is_sign_positive(), "a −0 sent: {values:?}");
    }
    // The arc's angles are +0's: 0 and π, counter-clockwise.
    assert!(values.contains(&std::f64::consts::PI), "{values:?}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Every hostile sketch's profile sends no −0 (design S3).
    #[test]
    fn no_profile_sends_a_minus_zero(h in hostile()) {
        let d = h.draw();
        let Some(Ok(p)) = profile_of(&d) else {
            return Ok(());
        };
        for v in sent_values(&p) {
            prop_assert!(v != 0.0 || v.is_sign_positive(), "a −0 sent");
        }
    }
}
