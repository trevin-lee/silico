//! break-it: hostile and degenerate values for the profile, extrusion, and revolve checks
//! (docs/ports/extrude-revolve.md). Expected answers come from exact integer geometry, hand
//! calculations, or OCCT's documented limits, never from the code under test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use silico_protocol::{
    Axis, InvalidParameter, MAX_LENGTH, Profile, ProfilePlane, check_revolve, extrusion_vector,
    side_of_line,
};

fn profile(points: &[[f64; 3]]) -> Profile {
    Profile {
        points: points.to_vec(),
    }
}

fn z(x: f64, y: f64) -> [f64; 3] {
    [x, y, 0.0]
}

fn unit_square_at(x0: f64) -> Profile {
    profile(&[z(x0, 0.0), z(x0 + 1.0, 0.0), z(x0 + 1.0, 1.0), z(x0, 1.0)])
}

fn refused<T: std::fmt::Debug>(result: Result<T, InvalidParameter>) -> InvalidParameter {
    match result {
        Ok(v) => panic!("accepted: {v:?}"),
        Err(e) => e,
    }
}

// ---- Vectors whose length overflows ----

#[test]
fn extrusion_direction_with_huge_finite_components_is_not_called_in_plane() {
    // (1e300, 0, 1e300) points 45° out of the XY plane, but its length overflows
    // to infinity in check_direction, so the "unit" vector is (0, 0, 0) and the error says
    // the direction lies in the profile's plane. Either scale it (it's a fine direction:
    // the vector is (1, 0, 1) × 5) or refuse it as too large; never call it in-plane.
    let plane = unit_square_at(0.0).check().unwrap();
    match extrusion_vector(&plane, 5.0, Some([1e300, 0.0, 1e300])) {
        Ok(v) => {
            for (got, want) in v.into_iter().zip([5.0, 0.0, 5.0]) {
                assert!((got - want).abs() < 1e-12, "{v:?}");
            }
        }
        Err(e) => {
            assert_eq!(e.parameter, "direction", "{}", e.message);
            assert!(
                !e.message.contains("lies in the profile's plane"),
                "a 45° direction was called in-plane: {}",
                e.message
            );
        }
    }
}

#[test]
fn revolve_axis_with_huge_finite_components_is_not_blamed_on_the_angle() {
    // an axis direction of (0, 1e300, 0) is the y axis, but its length overflows,
    // the unit direction becomes (0, 0, 0), every corner is 0 from the "axis", and the
    // error blames the angle ("moves the profile less than 1e-7 mm") for a 90° revolve of
    // a profile 1 to 2 from the axis. Expected: the revolve's unit axis (0, 1, 0), or a
    // refusal naming the axis.
    let p = unit_square_at(1.0);
    let plane = p.check().unwrap();
    let axis = Axis {
        origin: [0.0; 3],
        direction: [0.0, 1e300, 0.0],
    };
    match check_revolve(&plane, &p.points, &axis, 90.0) {
        Ok(unit) => assert_eq!(unit, [0.0, 1.0, 0.0]),
        Err(e) => assert_eq!(e.parameter, "axis", "{}", e.message),
    }
}

#[test]
fn vectors_whose_length_underflows_are_refused_as_having_no_direction() {
    // Lengths 1.4e-200 and 0 (subnormal components squared underflow): no direction.
    let plane = unit_square_at(0.0).check().unwrap();
    for v in [[1e-200, 0.0, 1e-200], [f64::MIN_POSITIVE, 0.0, 5e-324]] {
        let e = refused(extrusion_vector(&plane, 1.0, Some(v)));
        assert_eq!(e.parameter, "direction");
        assert!(e.message.contains("no direction"), "{}", e.message);
    }
}

#[test]
fn a_direction_exactly_at_freecads_limit_out_of_the_plane_is_extruded() {
    // FreeCAD's Pad refuses a direction only when |direction · normal| < Confusion (1e-7,
    // FeatureExtrude.cpp at 876271d660, line 504), so one exactly at it is extruded. Here
    // (1, 0, z) has factor z/√(1 + z²) on the xy square, which by hand needs z = 1e-7 ×
    // (1 + 5e-15) to be 1e-7; this z, within rounding of that, is the smallest whose factor
    // rounds to 1e-7 exactly. It rises 0.05 mm along the normal by sweeping 0.05 / 1e-7 =
    // 5e5 mm, under a kilometre. The baseline's surviving mutant, `factor <= ...`, refused it.
    let plane = unit_square_at(0.0).check().unwrap();
    let v = extrusion_vector(&plane, 0.05, Some([1.0, 0.0, 1.0000000000000049e-7])).unwrap();
    assert!((v[0] - 5e5).abs() < 1e-6, "{v:?}");
    assert_eq!(v[1], 0.0);
    assert!((v[2] - 0.05).abs() < 1e-15, "{v:?}");
    // z = 1e-7 itself has factor 1e-7/√(1 + 1e-14), just under the limit: in the plane.
    let e = refused(extrusion_vector(&plane, 0.05, Some([1.0, 0.0, 1e-7])));
    assert_eq!(e.parameter, "direction");
    assert!(
        e.message.contains("lies in the profile's plane"),
        "{}",
        e.message
    );
}

// ---- Self-crossing that cancellation hides ----

/// A bow tie whose sides 1 and 3 cross a third of the way along, at an angle of about
/// 3h / l radians: (0, 0) → (l, 2h) → (l, 0) → (0, h). Side 1 is y = 2hx/l and side 3 is
/// y = h(1 − x/l): they meet at x = l/3. Its lobes don't cancel (net area hl/2), so only
/// the crossing check can refuse it.
fn shallow_bow_tie(l: f64, h: f64) -> Profile {
    profile(&[z(0.0, 0.0), z(l, 2.0 * h), z(l, 0.0), z(0.0, h)])
}

#[test]
fn a_bow_tie_crossing_at_a_shallow_angle_is_refused() {
    // segment_distance computes uu·vv − uv² for two long, nearly antiparallel sides,
    // and the difference cancels to rounding noise (uu·vv ≈ 1e8 has an ulp of 1.5e-8, the
    // true value is 2.25e-8 for l = 100, h = 5e-7), so the closest points are wrong and the
    // crossing is missed. OCCT then builds an invalid face, refused only afterwards as
    // "the extrusion's result isn't a valid solid", naming nothing. Every case here
    // crosses (by construction, at x = l/3), so each must be refused as crossing itself.
    for (l, h) in [
        (100.0, 5e-7),
        (1e3, 1e-6),
        (1e3, 1e-5),
        (1e4, 1e-4),
        (1e5, 1e-4),
        (1e6, 1e-6),
    ] {
        let e = refused(shallow_bow_tie(l, h).check());
        assert_eq!(e.parameter, "points", "l = {l}, h = {h}: {}", e.message);
        assert!(
            e.message.contains("crosses or touches itself"),
            "l = {l}, h = {h}: {}",
            e.message
        );
    }
}

#[test]
fn a_bow_tie_crossing_at_an_ordinary_angle_is_refused() {
    for (l, h) in [(10.0, 1e-6), (10.0, 1e-4), (100.0, 1e-5), (1e3, 1e-4)] {
        let e = refused(shallow_bow_tie(l, h).check());
        assert!(
            e.message.contains("crosses or touches itself"),
            "{}",
            e.message
        );
    }
}

// ---- Values exactly at, and one step either side of, each limit ----

#[test]
fn a_point_is_left_or_right_of_a_line_only_where_rounding_cant_have_chosen() {
    // By hand: (0, 1) is left of the x axis run from (0, 0) to (1, 0), (0, -1) right, and
    // (5, 0) on it.
    let (o, x) = ([0.0, 0.0], [1.0, 0.0]);
    assert_eq!(side_of_line(o, x, [0.0, 1.0]), 1);
    assert_eq!(side_of_line(o, x, [0.0, -1.0]), -1);
    assert_eq!(side_of_line(o, x, [5.0, 0.0]), 0);
    // Corner 1 of the notch below, on the line from corner 5 to corner 6 within 1e-22 mm
    // (exact rationals): its orientation is rounding's, so it's on the line. 1e-6 mm off it
    // it's surely on a side: the line runs down and to the left, so its left is (2, -1)/√5.
    let (c5, c6) = ([73.70000015, 145.7000003], [61.10000015, 120.5000003]);
    assert_eq!(side_of_line(c5, c6, [61.1, 120.5]), 0);
    // Corner 2, by hand 7e-15 mm off the line: its orientation rounds to -1.1e-13, inside
    // the bound (4.2e-13) but not 0, so only the bound says it's on the line (#78's tests
    // review).
    assert_eq!(side_of_line(c5, c6, [48.5, 95.3]), 0);
    // The same three corners scaled by 2^-7, which is exact, so corner 2 is still 7e-15 / 128
    // mm off the line: by hand its orientation, -6.9e-18, is within the bound times the
    // products' sum (2.6e-17) but over it times their product (5e-19): a bound taken on the
    // product would trust rounding's sign near the origin (#44's surviving mutant).
    let near = |p: [f64; 2]| [p[0] / 128.0, p[1] / 128.0];
    assert_eq!(side_of_line(near(c5), near(c6), near([48.5, 95.3])), 0);
    // Past the bound, rounding's sign is the exact one, and side_of_line says so: this point
    // near the line from corner 5 to corner 6 is left of it by exact rational arithmetic,
    // its orientation 1.76 times the bound, so a bound two to four times as wide would call
    // it on the line (#78's mutants: the bound's `*` as `+`, its `/` as `%` or `*`).
    assert_eq!(
        side_of_line(c5, c6, [56.35393626839515, 111.00787253679026]),
        1
    );
    // And just inside the bound, on the line. From (0, 0) to (1, 1 - 3ε), (1, 1) has products
    // 1 and 1 - 3ε, both exact, and an orientation of exactly 3ε (their difference is exact):
    // left of the line, but within the bound times the products' sum, 2 - 3ε, which by hand
    // is 3ε + 3.5ε² - 12ε³ (rounded, 6.661338147750941e-16 against 3ε's ...939e-16). The
    // bound with 3 - 8ε in place of 3 + 8ε gives 3ε - 12.5ε², under it, and calls the point
    // left (#78's mutant: the bound's `+` as `-`).
    let e = f64::EPSILON;
    assert_eq!(
        side_of_line([0.0, 0.0], [1.0, 1.0 - 3.0 * e], [1.0, 1.0]),
        0
    );
    let off = 1e-6 / 5f64.sqrt();
    assert_eq!(side_of_line(c5, c6, [61.1 - 2.0 * off, 120.5 + off]), -1);
    assert_eq!(side_of_line(c5, c6, [61.1 + 2.0 * off, 120.5 - off]), 1);
}

#[test]
fn a_profile_crossing_itself_in_its_plane_is_refused_though_its_corners_are_off_it() {
    // Found by #78's robustness review: crossings are decided in the profile's plane, but
    // were measured as touches in space, where corners within the tolerance of the plane
    // can be twice that apart along its normal. Here the side from (500, 1000) comes down
    // through (500, 0, h), the corner B, onto side D to E along y = 0, and carries on to
    // (500, -10): in the plane, B is on side D to E (by hand, their x and y), a crossing;
    // in space it's 2h = 1.2e-7 from it, over the tolerance, D and E being h below it.
    let h = 6e-8;
    let refusal = refused(
        profile(&[
            [0.0, 0.0, 0.0],
            [490.0, 0.0, -h],
            [510.0, 0.0, -h],
            [1000.0, 0.0, 0.0],
            [1000.0, 1000.0, 0.0],
            [500.0, 1000.0, 0.0],
            [500.0, 0.0, h],
            [500.0, -10.0, 0.0],
            [0.0, -10.0, 0.0],
        ])
        .check(),
    );
    assert_eq!(refusal.parameter, "points");
    assert!(
        refusal.message.contains("crosses or touches itself"),
        "{}",
        refusal.message
    );
}

#[test]
fn corners_apart_in_space_but_one_in_the_profiles_plane_are_the_same_point() {
    // #78's tests review: corners 3 and 4 are 1.2e-7 mm apart, along the normal only, so
    // in the face they're one point (by hand: both at (10, 10)).
    let refusal = refused(
        profile(&[
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, -6e-8],
            [10.0, 10.0, 6e-8],
            [0.0, 10.0, 0.0],
        ])
        .check(),
    );
    assert!(
        refusal
            .message
            .contains("corners 3 and 4 are the same point in the profile's plane"),
        "{}",
        refusal.message
    );
}

/// A profile with a notch cut in it at a slant: the first side runs from `a` back along `u`
/// for `l`, the notch goes `3` mm to the right (`m`) and up along `u` past where it began,
/// back to the line, and down along it to `gap` short of `a`, then out to the left and back
/// to `a`. Sides 1 (corners 1 to 2) and 5 (corners 5 to 6) are on one line, `gap` apart.
fn notched(a: [f64; 2], u: [f64; 2], l: f64, gap: f64) -> Profile {
    let m = [u[1], -u[0]];
    let at = |along: f64, right: f64| {
        z(
            a[0] + along * u[0] + right * m[0],
            a[1] + along * u[1] + right * m[1],
        )
    };
    profile(&[
        at(0.0, 0.0),
        at(-l, 0.0),
        at(-l, 3.0),
        at(l + gap, 3.0),
        at(l + gap, 0.0),
        at(gap, 0.0),
        at(gap, -3.0),
    ])
}

#[test]
fn two_sides_on_one_line_apart_by_more_than_the_tolerance_are_a_profile() {
    // Found by M3.5's review: two sides on one slanted line, 3.35e-7 mm apart along it,
    // were called crossing, each side's ends measuring as on opposite sides of the other's
    // line by the rounding of products of nothing. By hand (exact rationals): sides 1 and 5
    // are on the line y = 2x - 1.7 within 7e-15 mm, corner 6 is 3.354e-7 mm from corner 1,
    // more than the tolerance, and no two sides meet; the corners turn counterclockwise
    // round 169.0467 mm², so the profile faces +z.
    let plane = profile(&[
        z(61.1, 120.5),
        z(48.5, 95.3),
        z(51.183281572999746, 93.95835921350012),
        z(76.38328172299974, 144.35835951350012),
        z(73.70000015, 145.7000003),
        z(61.10000015, 120.5000003),
        z(58.41671850200025, 121.84164093649989),
    ])
    .check()
    .unwrap();
    assert_eq!(plane.normal, [0.0, 0.0, 1.0]);
    // The same notch at other slants, places, and gaps: sides 1 and 5 are `gap` apart by
    // construction, over the tolerance in every case, so every one is a profile.
    let mut admitted_not = Vec::new();
    for degrees in [
        0.0,
        7.0,
        30.0,
        45.0,
        63.434948822922,
        90.0,
        121.0,
        200.0,
        333.0,
    ] {
        let t = f64::to_radians(degrees);
        let u = [libm::cos(t), libm::sin(t)];
        for a in [
            [0.0, 0.0],
            [61.1, 120.5],
            [-997.5, 512.25],
            [123456.789, -98765.4321],
        ] {
            for l in [1.0, 28.174456516497354, 500.0] {
                for gap in [1.5e-7, 2e-7, 3.354101926004954e-7, 1e-6] {
                    if let Err(e) = notched(a, u, l, gap).check() {
                        admitted_not.push(format!(
                            "{degrees}° at {a:?}, {l} long, {gap:e} apart: {e:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(
        admitted_not.is_empty(),
        "{} of 432 refused:\n{}",
        admitted_not.len(),
        admitted_not.join("\n")
    );
    // And the same notch with sides 1 and 5 overlapping along the line, or apart by less
    // than the tolerance, touches itself, wherever it's drawn.
    for degrees in [0.0, 30.0, 63.434948822922, 121.0] {
        let t = f64::to_radians(degrees);
        let u = [libm::cos(t), libm::sin(t)];
        for a in [[0.0, 0.0], [-997.5, 512.25], [123456.789, -98765.4321]] {
            for gap in [-1e-6, 5e-8] {
                let refusal = refused(notched(a, u, 28.174456516497354, gap).check());
                assert!(
                    refusal.message.contains("crosses or touches itself"),
                    "{degrees}° at {a:?}, {gap:e} apart: {}",
                    refusal.message
                );
            }
        }
    }
}

/// A square of side `s` with a spike from its top right corner whose tip, near the middle
/// of its first side's line, is `off` across it (positive) or short of it (negative).
fn spiked(at: [f64; 2], s: f64, off: f64) -> Profile {
    let across = [1.0 / 5f64.sqrt(), -2.0 / 5f64.sqrt()];
    let p = |x: f64, y: f64| z(at[0] + x, at[1] + y);
    profile(&[
        p(0.0, 0.0),
        p(s, s / 2.0),
        p(s, s),
        p(s / 2.0 + off * across[0], s / 4.0 + off * across[1]),
        p(0.0, s),
    ])
}

#[test]
fn a_spike_across_a_side_by_twice_the_tolerance_is_a_crossing_at_any_size() {
    // #78's tests review: no test held the orientation bound from being made too lenient.
    // The spike's tip is 2e-7 mm across side 1 (the line y = x / 2, from (0, 0) to
    // (s, s / 2)), by hand; it's further than the tolerance from every side, so only the
    // crossing can refuse it, and sides 3 and 4 both cross side 1. At a side of 1e6 mm the
    // tip's orientation is within ten thousand times the bound: a bound that much looser
    // lets it through.
    for (at, s) in [([0.0, 0.0], 1e6), ([0.0, 0.0], 1e3), ([-5e5, -5e5], 1e6)] {
        let refusal = refused(spiked(at, s, 2e-7).check());
        assert!(
            refusal
                .message
                .contains("side 1 (corners 1 to 2) meets side 3 (corners 3 to 4)"),
            "{s} at {at:?}: {}",
            refusal.message
        );
        // Its tip 1e-6 short of the line, nothing crosses.
        assert!(spiked(at, s, -1e-6).check().is_ok(), "{s} at {at:?}");
    }
}

#[test]
fn coordinates_at_a_kilometre_are_accepted_and_one_step_past_refused() {
    let at = profile(&[
        [MAX_LENGTH, MAX_LENGTH, -MAX_LENGTH],
        [MAX_LENGTH - 1.0, MAX_LENGTH, -MAX_LENGTH],
        [MAX_LENGTH, MAX_LENGTH - 1.0, -MAX_LENGTH],
    ]);
    assert!(at.check().is_ok());
    let mut past = at.clone();
    past.points[0][0] = MAX_LENGTH.next_up();
    let e = refused(past.check());
    assert!(e.message.contains("corner 1"), "{}", e.message);
}

#[test]
fn consecutive_corners_exactly_the_tolerance_apart_are_the_same_and_just_over_are_not() {
    // A unit square with an extra corner d along the bottom from the origin (exact: d − 0
    // is d). At 1e-7 the two are one point (OCCT's Precision::Confusion()); one step
    // further they're two.
    let with_gap = |d: f64| {
        profile(&[
            z(0.0, 0.0),
            z(d, 0.0),
            z(1.0, 0.0),
            z(1.0, 1.0),
            z(0.0, 1.0),
        ])
    };
    let e = refused(with_gap(1e-7).check());
    assert!(
        e.message.contains("corners 1 and 2 are the same point"),
        "{}",
        e.message
    );
    assert!(with_gap(1e-7f64.next_up()).check().is_ok());
}

#[test]
fn a_profile_with_every_coordinate_negative_zero_or_subnormal_has_no_area() {
    let e = refused(profile(&[[-0.0; 3], [5e-324, 0.0, 0.0], [0.0, 5e-324, -0.0]]).check());
    assert_eq!(e.parameter, "points");
    let e = refused(profile(&[[-0.0; 3], [1.0, -0.0, -0.0], [0.0, 1e-12, 0.0]]).check());
    assert_eq!(e.parameter, "points", "{}", e.message);
}

#[test]
fn an_axis_leaning_out_of_the_plane_at_the_limit_is_accepted_and_past_it_refused() {
    // An axis is in the plane while it's within the tolerance of it wherever it passes a
    // corner: through the origin along y, leaning `lean`, it passes the square's far
    // corners (y = 1) `lean` off the plane.
    let p = unit_square_at(1.0);
    let plane = p.check().unwrap();
    let leaning = |lean: f64| Axis {
        origin: [0.0; 3],
        direction: [0.0, 1.0, lean],
    };
    assert!(check_revolve(&plane, &p.points, &leaning(0.99e-7), 90.0).is_ok());
    let e = refused(check_revolve(&plane, &p.points, &leaning(1.01e-7), 90.0));
    assert_eq!(e.parameter, "axis");
    // Leaning a lot, it passes the far corners far off the plane: refused by distance.
    let e = refused(check_revolve(&plane, &p.points, &leaning(1e6), 90.0));
    assert_eq!(e.parameter, "axis");
    // Its point exactly the tolerance off the plane, and one step further.
    let off = |dz: f64| Axis {
        origin: [0.0, 0.0, dz],
        direction: [0.0, 1.0, 0.0],
    };
    assert!(check_revolve(&plane, &p.points, &off(1e-7), 90.0).is_ok());
    assert!(check_revolve(&plane, &p.points, &off(1e-7f64.next_up()), 90.0).is_err());
}

#[test]
fn a_profile_crossing_the_axis_by_the_tolerance_is_touching_and_by_more_is_crossing() {
    let plane_of = |p: &Profile| p.check().unwrap();
    let y = Axis {
        origin: [0.0; 3],
        direction: [0.0, 1.0, 0.0],
    };
    let at = unit_square_at(-1e-7);
    assert!(check_revolve(&plane_of(&at), &at.points, &y, 360.0).is_ok());
    let over = unit_square_at(-2e-7);
    let e = refused(check_revolve(&plane_of(&over), &over.points, &y, 360.0));
    assert!(e.message.contains("crosses the profile"), "{}", e.message);
}

#[test]
fn a_revolve_angle_below_occts_angular_precision_is_refused_as_the_angle() {
    // a profile 9e5 to 1e6 from the axis, by 5e-11 degrees (8.7e-13 rad): its far
    // corner moves 8.7e-7 mm, past the motion check, but OCCT refuses any revolve of
    // Precision::Angular() (1e-12 rad) or less (BRepSweep_Revol.cxx:35, throwing
    // "Standard_ConstructionError: BRepSweep_Rotation::Constructor"), and FreeCAD's
    // PartDesign refuses it first ("Angle of revolution too small", FeatureRevolved.cpp:251).
    let far = profile(&[z(9e5, 0.0), z(1e6, 0.0), z(1e6, 1.0), z(9e5, 1.0)]);
    let plane = far.check().unwrap();
    let y = Axis {
        origin: [0.0; 3],
        direction: [0.0, 1.0, 0.0],
    };
    for angle in [5e-11, 3e-11, 1e-12f64.to_degrees()] {
        let e = refused(check_revolve(&plane, &far.points, &y, angle));
        assert_eq!(e.parameter, "angle", "{angle}: {}", e.message);
    }
}

#[test]
fn a_revolve_short_of_a_whole_turn_by_the_tolerance_is_refused_and_a_nanodegree_more_turns() {
    // A unit square from x = 9 about y: its far corners are 10 off the axis. 360° less k
    // nanodegrees stops them 10 · k · 1e-9 · π/180 mm short of where they started: within
    // 1e-7 mm for k <= 1e-7 · 180 / (10π · 1e-9) = 1800/π = 572.96. So 572 short is refused,
    // its ends one place (FreeCAD 1.1.3 builds it, and a cut across it cuts itself), and
    // 573 short is the turn it says. 360 is a whole turn.
    let p = unit_square_at(9.0);
    let plane = p.check().unwrap();
    let y = Axis {
        origin: [0.0; 3],
        direction: [0.0, 1.0, 0.0],
    };
    for angle in [360.0 - 572e-9, 360.0 - 1e-9, 359.999_999_999_9] {
        let e = refused(check_revolve(&plane, &p.points, &y, angle));
        assert_eq!(e.parameter, "angle", "{angle}: {}", e.message);
        assert!(
            e.message.contains("within 1e-7 mm of where it started")
                && e.message.contains("use 360 for a whole turn"),
            "{angle}: {}",
            e.message
        );
    }
    assert_eq!(
        refused(check_revolve(&plane, &p.points, &y, 360.0 - 572e-9)).message,
        "the revolve's angle of 359.999999428 degrees stops the profile's farthest corner, 10 mm from the axis, within 1e-7 mm of where it started, short of a whole turn: its ends would be one place; use 360 for a whole turn, or less for ends apart"
    );
    for angle in [360.0 - 573e-9, 360.0] {
        assert_eq!(
            check_revolve(&plane, &p.points, &y, angle),
            Ok([0.0, 1.0, 0.0]),
            "{angle}"
        );
    }
}

#[test]
fn an_extrusion_too_long_at_an_ordinary_slant_blames_its_length_not_a_flat_direction() {
    // a kilometre's rise along (1, 0, 1), 45° out of the plane, sweeps
    // 1.414e6 mm and is refused, but the message says the direction is "so nearly in the
    // profile's plane", which a 45° direction isn't: an agent would turn the direction up
    // rather than shorten the length.
    let plane = unit_square_at(0.0).check().unwrap();
    let e = refused(extrusion_vector(&plane, MAX_LENGTH, Some([1.0, 0.0, 1.0])));
    assert_eq!(e.parameter, "length", "{}", e.message);
    assert!(e.message.contains("shorten it"), "{}", e.message);
}

#[test]
fn a_plainly_twisted_profile_is_refused_as_not_flat() {
    // Far off any plane, it's said to be not flat, not taken for one that crosses itself
    // (which, flattened onto the plane through its three furthest corners, it does).
    for points in [
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, -1.0],
        ],
        vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [0.0, 1.0, 1.0],
            [2.0, 1.0, -1.0],
        ],
        vec![
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 0.0],
            [1.0, 1.0, 3.0],
            [3.0, 1.0, -3.0],
            [2.0, -2.0, 0.0],
        ],
    ] {
        let e = profile(&points).check().unwrap_err();
        assert!(
            e.message.contains("isn't flat"),
            "{points:?}: {}",
            e.message
        );
    }
}

#[test]
fn an_axis_point_past_a_kilometre_is_refused() {
    let p = unit_square_at(1.0);
    let plane = p.check().unwrap();
    for origin in [[0.0, 2e6, 0.0], [0.0, MAX_LENGTH.next_up(), 0.0]] {
        let a = Axis {
            origin,
            direction: [0.0, 1.0, 0.0],
        };
        let e = refused(check_revolve(&plane, &p.points, &a, 90.0));
        assert_eq!(e.parameter, "axis", "{}", e.message);
        assert!(e.message.contains("a kilometre"), "{}", e.message);
    }
    let a = Axis {
        origin: [0.0, MAX_LENGTH, 0.0],
        direction: [0.0, 1.0, 0.0],
    };
    assert!(check_revolve(&plane, &p.points, &a, 90.0).is_ok());
}

#[test]
fn extrusion_limits_hold_exactly_at_the_limit() {
    let plane = unit_square_at(0.0).check().unwrap();
    // A length of exactly the tolerance is no length; a hair over it is one.
    assert_eq!(
        refused(extrusion_vector(&plane, 1e-7, None)).parameter,
        "length"
    );
    assert!(extrusion_vector(&plane, 1e-7f64.next_up(), None).is_ok());
    // A kilometre is the longest, straight up or along a direction.
    assert!(extrusion_vector(&plane, MAX_LENGTH, None).is_ok());
    assert!(extrusion_vector(&plane, MAX_LENGTH, Some([0.0, 0.0, 1.0])).is_ok());
    assert_eq!(
        refused(extrusion_vector(&plane, MAX_LENGTH.next_up(), None)).parameter,
        "length"
    );
    // Slanted, the sweep along the direction is what's limited: 3-4-5, a rise of 4 sweeps
    // 5, so a rise of 8e5 sweeps a kilometre (to rounding): just under it is allowed, just
    // over it isn't.
    assert!(extrusion_vector(&plane, 7.999_999e5, Some([3.0, 0.0, 4.0])).is_ok());
    assert_eq!(
        refused(extrusion_vector(&plane, 8.000_001e5, Some([3.0, 0.0, 4.0]))).parameter,
        "length"
    );
}

#[test]
fn the_most_slanted_direction_allowed_and_one_step_flatter() {
    // |direction · normal| of 1e-7 is allowed (the guard is `<`) when the sweep
    // stays within a kilometre: a rise of 1e-4 then travels 1e3.
    let plane = unit_square_at(0.0).check().unwrap();
    // (1, 0, 1e-7) normalizes to just under 1e-7, so take the next value up.
    let d = [1.0, 0.0, 1.000_000_1e-7];
    let v = extrusion_vector(&plane, 1e-4, Some(d)).unwrap();
    assert!((v[2] - 1e-4).abs() < 1e-16, "{v:?}");
    let e = refused(extrusion_vector(&plane, 1e-4, Some([1.0, 0.0, 9e-8])));
    assert!(
        e.message.contains("lies in the profile's plane"),
        "{}",
        e.message
    );
}

#[test]
fn a_thousand_corners_check_quickly() {
    // The crossing check is quadratic: 1000 corners is half a million side pairs. The
    // corners are on the unit circle, spaced evenly round the diamond |x| + |y| = 1 and
    // scaled out onto it (Silico's lints keep trigonometry out).
    let n = 1000u32;
    let circle: Vec<[f64; 3]> = (0..n)
        .map(|i| {
            let u = 4.0 * f64::from(i) / f64::from(n);
            let (x, y) = match u {
                u if u < 1.0 => (1.0 - u, u),
                u if u < 2.0 => (1.0 - u, 2.0 - u),
                u if u < 3.0 => (u - 3.0, 2.0 - u),
                u => (u - 3.0, u - 4.0),
            };
            let r = (x * x + y * y).sqrt();
            [x / r, y / r, 0.0]
        })
        .collect();
    let start = std::time::Instant::now();
    assert!(profile(&circle).check().is_ok());
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    // The same with its last corner pulled across the circle: refused.
    let mut crossed = circle;
    crossed[n as usize - 1] = [-1.5, 0.0, 0.0];
    assert!(profile(&crossed).check().is_err());
}

// ---- Property: the check agrees with exact integer geometry ----

fn orient(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> i64 {
    ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).signum()
}

fn on_segment(p: (i64, i64), a: (i64, i64), b: (i64, i64)) -> bool {
    orient(a, b, p) == 0
        && p.0 >= a.0.min(b.0)
        && p.0 <= a.0.max(b.0)
        && p.1 >= a.1.min(b.1)
        && p.1 <= a.1.max(b.1)
}

fn segments_touch(a: (i64, i64), b: (i64, i64), c: (i64, i64), d: (i64, i64)) -> bool {
    let (o1, o2, o3, o4) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    (o1 * o2 < 0 && o3 * o4 < 0)
        || on_segment(c, a, b)
        || on_segment(d, a, b)
        || on_segment(a, c, d)
        || on_segment(b, c, d)
}

/// Whether the integer polygon is simple, exactly: distinct consecutive corners, no two
/// sides touching except neighbours at their shared corner, and no side folding back onto
/// its neighbour. (A simple polygon on the integer grid has area at least 1/2.)
fn simple(points: &[(i64, i64)]) -> bool {
    let n = points.len();
    let next = |i: usize| points[(i + 1) % n];
    if (0..n).any(|i| points[i] == next(i)) {
        return false;
    }
    for (i, &a) in points.iter().enumerate() {
        let b = next(i);
        for (j, &c) in points.iter().enumerate().skip(i + 1) {
            let d = next(j);
            let touches = if j == i + 1 {
                on_segment(d, a, b) || on_segment(a, c, d)
            } else if i == 0 && j == n - 1 {
                on_segment(c, a, b) || on_segment(b, c, d)
            } else {
                segments_touch(a, b, c, d)
            };
            if touches {
                return false;
            }
        }
    }
    true
}

fn twice_signed_area(points: &[(i64, i64)]) -> i64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum()
}

/// The rotation a quaternion of any length but zero gives, applied to `p`.
fn rotate(p: [f64; 3], q: [f64; 4]) -> [f64; 3] {
    let n = q.iter().map(|c| c * c).sum::<f64>().sqrt();
    let [w, x, y, z] = q.map(|c| c / n);
    let row = |a: f64, b: f64, c: f64| a * p[0] + b * p[1] + c * p[2];
    [
        row(
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ),
        row(
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ),
        row(
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ),
    ]
}

fn grid_polygon() -> impl Strategy<Value = Vec<(i64, i64)>> {
    prop::collection::vec((0i64..=4, 0i64..=4), 3..=8)
}

fn placement() -> impl Strategy<Value = (f64, [f64; 3], Option<[f64; 4]>)> {
    // Scales from a hundredth of a micron to 100 m, offsets out to 1e5, and either no
    // rotation (exact) or a general one.
    let scale = prop::sample::select(vec![1e-4, 1e-3, 0.37, 1.0, 1e3, 1e5]);
    let offset = prop::sample::select(vec![0.0, -3.5, 1e3, 1e5]);
    let angles = prop::option::of(
        prop::array::uniform4(-1.0f64..1.0).prop_filter("a quaternion of some length", |q| {
            q.iter().map(|c| c * c).sum::<f64>() > 0.01
        }),
    );
    (scale, prop::array::uniform3(offset), angles)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 4000,
        failure_persistence: None,
        // Fixed, so a red run is red every time.
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5111_c0b0),
        ..ProptestConfig::default()
    })]

    #[test]
    fn the_check_accepts_exactly_the_simple_polygons(
        grid in grid_polygon(),
        (scale, offset, angles) in placement(),
    ) {
        let points: Vec<[f64; 3]> = grid
            .iter()
            .map(|&(x, y)| {
                let p = [x as f64 * scale, y as f64 * scale, 0.0];
                let p = angles.map_or(p, |a| rotate(p, a));
                [p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]]
            })
            .collect();
        let checked = profile(&points).check();
        let want = simple(&grid);
        prop_assert_eq!(checked.is_ok(), want, "{:?} → {:?}", grid, checked.as_ref().err());
        if let Ok(ProfilePlane { normal, .. }) = checked {
            // The normal is the plane's +z or −z (as the corners turn), rotated. Rotated
            // corners far from the origin are rounded (to about 1e-16 of the offset), which
            // tilts a small polygon's plane by up to that over its size.
            let sign = twice_signed_area(&grid).signum() as f64;
            let up = angles.map_or([0.0, 0.0, sign], |a| rotate([0.0, 0.0, sign], a));
            let far = offset.iter().fold(1.0f64, |m, c| m.max(c.abs()));
            let tolerance = 1e-12 + 1e-14 * far / scale;
            for k in 0..3 {
                prop_assert!((normal[k] - up[k]).abs() < tolerance, "{:?} vs {:?}", normal, up);
            }
        }
    }

    #[test]
    fn an_extrusion_vector_always_rises_its_length_along_the_normal(
        length in prop_oneof![Just(1.0000001e-7), Just(1.0), Just(MAX_LENGTH), 1e-6f64..1e5],
        direction in prop::array::uniform3(prop_oneof![Just(0.0), Just(-1.0), Just(1e-7), -1e3f64..1e3]),
    ) {
        let plane = unit_square_at(0.0).check().unwrap();
        if let Ok(v) = extrusion_vector(&plane, length, Some(direction)) {
            prop_assert!(v.iter().all(|c| c.is_finite()));
            prop_assert!((v[2].abs() - length).abs() <= 1e-9 * length, "{:?} for {}", v, length);
            let travel = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            prop_assert!(travel <= MAX_LENGTH * (1.0 + 1e-15), "{}", travel);
            // Along the direction given, the same way.
            let d = direction.iter().map(|c| c * c).sum::<f64>().sqrt();
            for k in 0..3 {
                prop_assert!((v[k] / travel - direction[k] / d).abs() < 1e-9, "{:?} vs {:?}", v, direction);
            }
        }
    }
}

#[test]
fn a_leaning_axis_that_crosses_the_profile_past_the_tolerance_is_refused() {
    // A sliver 3e-7 wide whose bottom corners are 1.08e-7 to 1.24e-7 across an axis along
    // x that leans (0.9, 0, 0.4): within the tolerance of the plane at every corner, so in
    // it, but crossing the profile by more than the tolerance. Measured across the axis
    // without accounting for its lean, they read 9.9e-8: touching.
    let y0 = -1.08e-7;
    let p = profile(&[
        [-1.5e-7, y0, 0.0],
        [1.5e-7, y0, 0.0],
        [1.5e-7, 10.0, 0.0],
        [-1.5e-7, 10.0, 0.0],
    ]);
    let plane = p.check().unwrap();
    let a = Axis {
        origin: [0.0; 3],
        direction: [0.9, 0.0, 0.4],
    };
    let e = refused(check_revolve(&plane, &p.points, &a, 90.0));
    assert_eq!(e.parameter, "axis", "{}", e.message);
    assert!(e.message.contains("crosses the profile"), "{}", e.message);
}

#[test]
fn a_leaning_axis_moves_the_profile_by_true_distance() {
    // A 3e-7 × 10 sliver beside an axis along x leaning (0.9, 0, 0.4): its far corners are
    // 10 from the axis, so 6e-7 degrees moves them 1.047e-7, enough. Measured without the
    // lean, they'd read 9% short, and it would be refused.
    let p = profile(&[
        [-1.5e-7, 0.0, 0.0],
        [1.5e-7, 0.0, 0.0],
        [1.5e-7, 10.0, 0.0],
        [-1.5e-7, 10.0, 0.0],
    ]);
    let plane = p.check().unwrap();
    let a = Axis {
        origin: [0.0; 3],
        direction: [0.9, 0.0, 0.4],
    };
    assert!(check_revolve(&plane, &p.points, &a, 6e-7).is_ok());
}

#[test]
fn a_steep_axis_past_a_sliver_is_refused_by_its_lean_alone() {
    // A 1 × 2.5e-7 sliver, with the axis through the middle of its height: the axis passes
    // every corner within 6.25e-8 of the plane however much it leans, so only the lean
    // limit (half, as a cosine to the normal) refuses a steep one.
    let h = 2.5e-7;
    let p = profile(&[
        [1.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, h, 0.0],
        [1.0, h, 0.0],
    ]);
    let plane = p.check().unwrap();
    let leaning = |lean: f64| Axis {
        origin: [0.0, h / 2.0, 0.0],
        direction: [0.0, (1.0 - lean * lean).sqrt(), lean],
    };
    assert!(check_revolve(&plane, &p.points, &leaning(0.49), 90.0).is_ok());
    for lean in [0.51, 0.9, 0.999] {
        let e = refused(check_revolve(&plane, &p.points, &leaning(lean), 90.0));
        assert_eq!(e.parameter, "axis", "{lean}: {}", e.message);
        assert!(
            e.message.contains("must lie in the profile's plane"),
            "{lean}: {}",
            e.message
        );
    }
}

#[test]
fn only_a_negative_turn_is_told_to_reverse_the_axis() {
    let p = unit_square_at(1.0);
    let plane = p.check().unwrap();
    let y = Axis {
        origin: [0.0; 3],
        direction: [0.0, 1.0, 0.0],
    };
    for angle in [-90.0, -360.0, -1e-3] {
        let e = refused(check_revolve(&plane, &p.points, &y, angle));
        assert!(
            e.message.contains("reverse the axis's direction"),
            "{angle}: {}",
            e.message
        );
    }
    for angle in [0.0, 400.0, -400.0, f64::NAN, f64::NEG_INFINITY] {
        let e = refused(check_revolve(&plane, &p.points, &y, angle));
        assert!(!e.message.contains("reverse"), "{angle}: {}", e.message);
    }
}
