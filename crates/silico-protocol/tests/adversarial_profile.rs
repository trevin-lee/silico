//! break-it: hostile and degenerate values for the profile, extrusion, and revolve checks
//! (docs/ports/extrude-revolve.md). Expected answers come from exact integer geometry, hand
//! calculations, or OCCT's documented limits, never from the code under test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use silico_protocol::{
    Axis, InvalidParameter, MAX_LENGTH, Profile, ProfilePlane, check_revolve, extrusion_vector,
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
        Ok(revolution) => assert_eq!(revolution.direction, [0.0, 1.0, 0.0]),
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
fn a_leaning_axis_snaps_and_moves_by_true_distance() {
    // A 3e-7 × 10 sliver beside an axis along x leaning (0.9, 0, 0.4): its far corners are
    // 10 from the axis, so 6e-7 degrees short of a turn leaves 1.047e-7 there, over the
    // tolerance (not snapped), and 6e-7 degrees moves them that far (enough). Measured
    // without the lean, they'd read 9% short, and both would go the other way.
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
    assert_eq!(
        check_revolve(&plane, &p.points, &a, 360.0 - 6e-7)
            .unwrap()
            .angle,
        360.0 - 6e-7
    );
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
