//! M5.9, hostile face frames (`/break-it`): `FaceFrame::check` at and either side of its
//! bounds, frames built the ways the core will build them (a rotation's columns, or
//! `FaceFrame::from_normal`, a face's normal and a stored axis projected onto its plane, as
//! docs/ports/attacher.md's F1 and F2 derive them, against the note's worked tables), and
//! `SketchPlane::place` on them. Every expected value is a hand calculation or a property of
//! a rotation.

use proptest::prelude::*;
use silico_protocol::{
    FACE_FRAME_SQUARENESS, FaceFrame, FaceFrameError, FaceRegions, LENGTH_TOLERANCE,
    MAX_FRAME_ORIGIN, MAX_LENGTH, SketchPlane,
};

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let l = libm::sqrt(dot(v, v));
    v.map(|c| c / l)
}

/// The rotation by `angle` about the unit `k` (Rodrigues) applied to `e`:
/// R e = e cos + (k × e) sin + k (k·e)(1 − cos).
fn rotated(k: [f64; 3], angle: f64, e: [f64; 3]) -> [f64; 3] {
    let (s, c) = (libm::sin(angle), libm::cos(angle));
    let kxe = cross(k, e);
    let ke = dot(k, e);
    [0, 1, 2].map(|i| e[i] * c + kxe[i] * s + k[i] * ke * (1.0 - c))
}

/// The rotation's frame: its first two columns x and y, from `origin`. Its third column,
/// R z, is its normal.
fn rotation(k: [f64; 3], angle: f64, origin: [f64; 3]) -> FaceFrame {
    FaceFrame {
        origin,
        x: rotated(k, angle, [1.0, 0.0, 0.0]),
        y: rotated(k, angle, [0.0, 1.0, 0.0]),
    }
}

fn refused(frame: FaceFrame, says: &str) {
    let err = frame.check().unwrap_err();
    assert_eq!(err.parameter, "plane", "{}", err.message);
    assert!(err.message.contains(says), "{}", err.message);
}

fn xy() -> FaceFrame {
    FaceFrame {
        origin: [0.0; 3],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
    }
}

fn off(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f64::max)
}

#[test]
fn a_frames_origin_exactly_at_its_reach_passes_and_a_hair_past_is_refused() {
    // Along an axis, the origin's length is its one coordinate exactly (√(c²) = |c| in
    // binary floating point), so the bound is exact there.
    for k in 0..3 {
        for sign in [1.0, -1.0] {
            let mut at = xy();
            at.origin[k] = sign * MAX_FRAME_ORIGIN;
            assert_eq!(at.check(), Ok(()), "{at:?}");
            let mut past = xy();
            past.origin[k] = sign * f64::from_bits(MAX_FRAME_ORIGIN.to_bits() + 1);
            refused(past, "the face frame's origin must be within √3 · 1e6 mm");
        }
    }
    // √3 · 1e6 and its rounding: just over 1.732e6, under 1.7321e6.
    const { assert!(MAX_FRAME_ORIGIN > 1.732_050_807e6 && MAX_FRAME_ORIGIN < 1.7321e6) };
}

#[test]
fn a_frame_of_negative_zeros_and_subnormals_is_sound_and_places_its_origin_as_plus_zero() {
    let frame = FaceFrame {
        origin: [-0.0, 5e-324, -5e-324],
        x: [1.0, -0.0, 5e-324],
        y: [-0.0, 1.0, -0.0],
    };
    assert_eq!(frame.check(), Ok(()));
    // (0, 0) is the origin, its −0 as +0: origin + 0·x + 0·y + 0.
    let placed = SketchPlane::Face(frame).place([0.0, -0.0]);
    assert_eq!(
        placed.map(f64::to_bits),
        [0.0, 5e-324, -5e-324].map(f64::to_bits)
    );
}

#[test]
fn an_axis_so_long_its_length_overflows_is_refused_as_not_a_unit_vector() {
    // 1e155² overflows to inf: refused as a length, not passed as NaN comparisons would.
    refused(
        FaceFrame {
            x: [1e155, 0.0, 0.0],
            ..xy()
        },
        "the face frame's x axis must be a unit vector",
    );
    refused(
        FaceFrame {
            y: [f64::MAX, f64::MAX, 0.0],
            ..xy()
        },
        "the face frame's y axis must be a unit vector",
    );
    // And one too short to square (each component a subnormal): length 0.
    refused(
        FaceFrame {
            y: [5e-324, 5e-324, 5e-324],
            ..xy()
        },
        "the face frame's y axis must be a unit vector",
    );
}

#[test]
fn a_frame_of_the_xy_plane_with_x_and_y_swapped_is_the_xy_plane_seen_from_below() {
    // No frame is left-handed: its normal is x × y, so swapping x and y turns it over.
    let swapped = FaceFrame {
        x: [0.0, 1.0, 0.0],
        y: [1.0, 0.0, 0.0],
        ..xy()
    };
    assert_eq!(swapped.check(), Ok(()));
    assert_eq!(SketchPlane::Face(swapped).normal(), [0.0, 0.0, -1.0]);
    assert_eq!(SketchPlane::Face(xy()).normal(), [0.0, 0.0, 1.0]);
}

#[test]
fn a_bad_frame_is_refused_before_regions_whatever_they_hold() {
    // A frame that fails says so even when the regions are nonsense too: the plane's
    // refusal names the plane.
    let regions = FaceRegions {
        plane: SketchPlane::Face(FaceFrame {
            y: [0.0, f64::NAN, 0.0],
            ..xy()
        }),
        regions: vec![],
    };
    let err = regions.check().unwrap_err();
    assert_eq!(err.parameter, "plane", "{}", err.message);
}

#[test]
fn a_frames_squareness_times_a_sketchs_reach_is_within_occts_tolerance() {
    // OCCT builds the faces' plane from the normal and x, and makes its own y, n × x̂. A
    // frame whose y leans s = 16ε (2⁻⁴⁸ ≈ 3.6e-15) towards x: x = +X, y = (s, √(1 − s²), 0) = (s, 1, 0)
    // in floating point, n = x × y / |x × y| = +Z, OCCT's y = Z × X = +Y. The sketch's
    // (0, 1e6) is placed at (1e6 · s, 1e6, 0) ≈ (3.6e-9, 1e6, 0); OCCT's is (0, 1e6, 0):
    // 3.6e-9 apart, the frame's share beside the 1e-7 an arc's end may be off its circle.
    let s = FACE_FRAME_SQUARENESS;
    let frame = FaceFrame {
        y: [s, libm::sqrt(1.0 - s * s), 0.0],
        ..xy()
    };
    assert_eq!(frame.y[1], 1.0);
    assert_eq!(frame.check(), Ok(()));
    let plane = SketchPlane::Face(frame);
    assert_eq!(plane.normal(), [0.0, 0.0, 1.0]);
    let placed = plane.place([0.0, MAX_LENGTH]);
    let gap = off(placed, [0.0, MAX_LENGTH, 0.0]);
    assert_eq!(FACE_FRAME_SQUARENESS, libm::ldexp(1.0, -48));
    assert!(gap > 3.5e-9 && gap < 3.6e-9, "{gap}");
    const { assert!(FACE_FRAME_SQUARENESS * MAX_LENGTH * 10.0 <= LENGTH_TOLERANCE) };
    // Twice as far off square is refused.
    refused(
        FaceFrame {
            y: [2.0 * s, 1.0, 0.0],
            ..xy()
        },
        "must be perpendicular, to 16ε (3.6e-15)",
    );
}

/// A frame from `from_normal`, which must be made.
fn framed(p: [f64; 3], n: [f64; 3], a: [f64; 3]) -> FaceFrame {
    FaceFrame::from_normal(p, n, a).unwrap_or_else(|e| panic!("{n:?}, {a:?}: {e:?}"))
}

#[test]
fn a_boxs_six_faces_frames_are_the_worked_tables_exactly() {
    // docs/ports/attacher.md, *Worked frames*: a box with its corner at (1, 2, 3), 10 × 20
    // × 30, each face's point the corner or the opposite corner (11, 22, 33). Every axis is
    // model space's, so every value is exact, each zero +0 (so `neg` writes +0s too).
    const X: [f64; 3] = [1.0, 0.0, 0.0];
    const Y: [f64; 3] = [0.0, 1.0, 0.0];
    const Z: [f64; 3] = [0.0, 0.0, 1.0];
    let neg = |v: [f64; 3]| v.map(|c| -c + 0.0);
    let (low, high) = ([1.0, 2.0, 3.0], [11.0, 22.0, 33.0]);
    // (point, n, the default token's axis, x, y, o).
    let faces = [
        ("Face1", low, neg(X), neg(Y), neg(Y), Z, [1.0, 0.0, 0.0]),
        ("Face2", high, X, Y, Y, Z, [11.0, 0.0, 0.0]),
        ("Face3", low, neg(Y), X, X, Z, [0.0, 2.0, 0.0]),
        ("Face4", high, Y, neg(X), neg(X), Z, [0.0, 22.0, 0.0]),
        ("Face5", low, neg(Z), X, X, neg(Y), [0.0, 0.0, 3.0]),
        ("Face6", high, Z, X, X, Y, [0.0, 0.0, 33.0]),
    ];
    let bits = |v: [f64; 3]| v.map(f64::to_bits);
    for (face, p, n, a, x, y, o) in faces {
        let frame = framed(p, n, a);
        assert_eq!(bits(frame.x), bits(x), "{face}'s x");
        assert_eq!(bits(frame.y), bits(y), "{face}'s y");
        assert_eq!(bits(frame.origin), bits(o), "{face}'s origin");
        assert_eq!(SketchPlane::Face(frame).normal(), n, "{face}'s normal");
    }
    // Face6 reversed, the frame of −n and −a: n −Z, x −X, y +Y, the same origin.
    let reversed = framed(high, neg(Z), neg(X));
    assert_eq!(bits(reversed.x), bits(neg(X)));
    assert_eq!(bits(reversed.y), bits(Y));
    assert_eq!(bits(reversed.origin), bits([0.0, 0.0, 33.0]));
    assert_eq!(SketchPlane::Face(reversed).normal(), neg(Z));
    // A point (u, v) lands at o + u·x + v·y: (2, 5) on Face2 is (11, 2, 5).
    assert_eq!(
        SketchPlane::Face(framed(high, X, Y)).place([2.0, 5.0]),
        [11.0, 2.0, 5.0]
    );
}

#[test]
fn the_45_degree_ties_frames_are_the_worked_tables_to_rounding() {
    use std::f64::consts::FRAC_1_SQRT_2 as H;
    let t = 1.0 / 3f64.sqrt();
    let (r6, r2) = (1.0 / 6f64.sqrt(), H);
    // (n, the token's axis, x, y), from the note's table of ties, each by hand.
    let ties = [
        ([H, H, 0.0], [0.0, 1.0, 0.0], [-H, H, 0.0], [0.0, 0.0, 1.0]),
        ([H, -H, 0.0], [0.0, 1.0, 0.0], [H, H, 0.0], [0.0, 0.0, 1.0]),
        // The X–Y boundary's other winners give the same x.
        ([H, H, 0.0], [-1.0, 0.0, 0.0], [-H, H, 0.0], [0.0, 0.0, 1.0]),
        ([H, -H, 0.0], [1.0, 0.0, 0.0], [H, H, 0.0], [0.0, 0.0, 1.0]),
        ([H, 0.0, H], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [-H, 0.0, H]),
        // X = Z with `+x` instead: turned 90°.
        ([H, 0.0, H], [1.0, 0.0, 0.0], [H, 0.0, -H], [0.0, 1.0, 0.0]),
        (
            [0.0, H, H],
            [-1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, -H, H],
        ),
        // Y = Z with `+x` instead: turned 180°.
        ([0.0, H, H], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, H, -H]),
        ([0.0, -H, H], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, H, H]),
        (
            [t, t, t],
            [0.0, 1.0, 0.0],
            [-r6, 2.0 * r6, -r6],
            [-r2, 0.0, r2],
        ),
    ];
    for (n, a, x, y) in ties {
        let frame = framed([0.0; 3], n, a);
        assert!(
            off(frame.x, x) <= 4.0 * f64::EPSILON,
            "{n:?}: x {:?}",
            frame.x
        );
        assert!(
            off(frame.y, y) <= 4.0 * f64::EPSILON,
            "{n:?}: y {:?}",
            frame.y
        );
        let normal = SketchPlane::Face(frame).normal();
        assert!(off(normal, n) <= 4.0 * f64::EPSILON, "{n:?}: {normal:?}");
    }
    // Face8 of testPartDesignTNPChamfer: the box's corner at (10, 0) chamfered by 1, the
    // face through (9, 0) and (10, 1), n·p = 9/√2, so o = (4.5, −4.5, 0).
    let face8 = framed([10.0, 1.0, 5.0], [H, -H, 0.0], [0.0, 1.0, 0.0]);
    assert!(
        off(face8.origin, [4.5, -4.5, 0.0]) <= 1e-14,
        "{:?}",
        face8.origin
    );
}

#[test]
fn an_axis_along_the_normal_is_refused_and_one_just_off_it_is_taken() {
    // Face2 (token `+y`) turned 90° about Z: n = +Y, so a − n(n·a) = 0.
    assert_eq!(
        FaceFrame::from_normal([0.0; 3], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
        Err(FaceFrameError::AxisAlongNormal(0.0))
    );
    // As floating point turns it: n = (cos 90°, sin 90°, 0) = (6.1e-17, 1, 0), its
    // projection about 6e-17 long, under 1e-12.
    let n = [libm::cos(std::f64::consts::FRAC_PI_2), 1.0, 0.0];
    let Err(FaceFrameError::AxisAlongNormal(length)) =
        FaceFrame::from_normal([0.0; 3], n, [0.0, 1.0, 0.0])
    else {
        panic!("taken")
    };
    assert!(length > 0.0 && length < 1e-16, "{length}");
    // At 89.99999999°: |a − n(n·a)| = cos θ ≈ 1.745e-10, taken, x ≈ −X (the note's
    // *Parallel*).
    let theta = 89.999_999_99f64.to_radians();
    let n = [libm::cos(theta), libm::sin(theta), 0.0];
    let frame = framed([0.0; 3], n, [0.0, 1.0, 0.0]);
    assert!(off(frame.x, [-1.0, 0.0, 0.0]) <= 1e-9, "{:?}", frame.x);
    assert_eq!(frame.check(), Ok(()));
}

#[test]
fn from_normal_refuses_inputs_that_arent_finite_and_an_origin_past_its_reach() {
    for bad in [f64::NAN, f64::INFINITY] {
        for (p, n, a) in [
            ([bad, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            ([0.0; 3], [0.0, bad, 1.0], [1.0, 0.0, 0.0]),
            ([0.0; 3], [0.0, 0.0, 1.0], [bad, 0.0, 0.0]),
        ] {
            let Err(FaceFrameError::Invalid(e)) = FaceFrame::from_normal(p, n, a) else {
                panic!("{p:?} {n:?} {a:?} taken")
            };
            assert_eq!(e.parameter, "plane");
            assert!(e.message.contains("must be finite"), "{}", e.message);
        }
    }
    // A plane 2e6 out along its normal: its origin is out of reach.
    let Err(FaceFrameError::Invalid(e)) =
        FaceFrame::from_normal([2e6, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    else {
        panic!("taken")
    };
    assert!(e.message.contains("origin must be within"), "{}", e.message);
}

#[test]
fn a_normal_a_hair_off_a_unit_gives_the_frame_of_the_unit_normal() {
    // `describe` takes a normal 1e-6 off a unit. n = (1, 2, 2)/3 on a plane 1e6 out along
    // it (p = 1e6 · n), and the same n made 1 + 1e-7 long: the same frame, to 4ε (the
    // origin to 4ε of its 1e6), its origin on the plane. Taken as it came, the long n would
    // put o = n(n·p) (1 + 1e-7)² · 1e6 out, 0.2 mm off the plane.
    let n = [1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0];
    let p = n.map(|c| c * 1e6);
    let long = n.map(|c| c * (1.0 + 1e-7));
    for a in [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0]] {
        let unit = framed(p, n, a);
        let near = framed(p, long, a);
        assert!(off(near.x, unit.x) <= 4.0 * f64::EPSILON, "{:?}", near.x);
        assert!(off(near.y, unit.y) <= 4.0 * f64::EPSILON, "{:?}", near.y);
        assert!(
            off(near.origin, unit.origin) <= 4.0 * f64::EPSILON * 1e6,
            "{:?} vs {:?}",
            near.origin,
            unit.origin
        );
        // On the plane: n · o is the plane's offset, 1e6, to a few roundings of it.
        assert!(
            (dot(n, near.origin) - 1e6).abs() <= 1e-8,
            "{:?}",
            near.origin
        );
    }
}

#[test]
fn a_normal_of_no_length_or_none_to_measure_is_refused_as_invalid_naming_the_normal() {
    // Zero, not a number, or too long to square (|n|² overflows): refused as invalid,
    // naming the face's normal, never as an axis along it.
    for n in [
        [0.0, 0.0, 0.0],
        [-0.0, 0.0, -0.0],
        [f64::NAN, 0.0, 1.0],
        [0.0, f64::INFINITY, 0.0],
        [1e200, 0.0, 0.0],
        [5e-324, 0.0, 0.0],
    ] {
        match FaceFrame::from_normal([0.0; 3], n, [1.0, 0.0, 0.0]) {
            Err(FaceFrameError::Invalid(e)) => {
                assert_eq!(e.parameter, "plane", "{n:?}");
                assert!(
                    e.message.contains("the face's normal"),
                    "{n:?}: {}",
                    e.message
                );
            }
            other => panic!("{n:?}: {other:?}"),
        }
    }
}

#[test]
fn a_normal_off_a_unit_by_more_than_describes_tolerance_is_refused_saying_its_true_length() {
    // `describe` gives a plane's normal a unit to 1e-6 (NORMAL_LENGTH_TOLERANCE): one off
    // it by more is a fault in what described the face, refused with its length as it is.
    // 1e-160 long squares into the subnormals (1e-320, partial underflow), 1e200 long past
    // f64's range (inf): each said as its own length, not a rounded or infinite one.
    for (n, said) in [
        (
            [1e-160, 0.0, 0.0],
            "the face's normal must be a unit vector to 1e-6, got [1e-160, 0.0, 0.0], 1e-160 long",
        ),
        (
            [1e200, 0.0, 0.0],
            "the face's normal must be a unit vector to 1e-6, got [1e200, 0.0, 0.0], 1e200 long",
        ),
        (
            [0.0, 0.0, 1.0 + 2e-6],
            "the face's normal must be a unit vector to 1e-6, got [0.0, 0.0, 1.000002], 1.000002 long",
        ),
        (
            [0.0, 0.0, 0.0],
            "the face's normal must be a unit vector to 1e-6, got [0.0, 0.0, 0.0], 0 long",
        ),
    ] {
        match FaceFrame::from_normal([0.0; 3], n, [1.0, 0.0, 0.0]) {
            Err(FaceFrameError::Invalid(e)) => {
                assert_eq!(e.parameter, "plane", "{n:?}");
                assert_eq!(e.message, said, "{n:?}");
            }
            other => panic!("{n:?}: {other:?}"),
        }
    }
    // (0, 0, 1 + 1e-6), at the tolerance, on a face at z = 1000: taken, made a unit, so
    // o = n̂(n̂·p) = (0, 0, 1000) to 4ε of it. Unnormalized, n(n·p) would put it 2e-3 up.
    let n = [0.0, 0.0, 1.0 + 1e-6];
    let frame = FaceFrame::from_normal([3.0, -4.0, 1000.0], n, [1.0, 0.0, 0.0]).unwrap();
    assert!(
        (frame.origin[2] - 1000.0).abs() <= 4.0 * f64::EPSILON * 1000.0,
        "{:?}",
        frame.origin
    );
    assert_eq!((frame.origin[0], frame.origin[1]), (0.0, 0.0));
    assert_eq!(frame.x, [1.0, 0.0, 0.0]);
    assert_eq!(frame.y, [0.0, 1.0, 0.0]);
}

#[test]
fn a_box_corners_diagonal_origin_needs_the_bounds_rounding() {
    // The far corner of a box a kilometre out on every axis, on a face whose normal is
    // the diagonal: o = n(n·p) is the corner itself, √3 · 1e6 out, but worked out it comes
    // to 1732050.8075688777, past the bare √3 · 1e6 (1732050.8075688772), so the bound
    // carries the rounding (MAX_FRAME_ORIGIN).
    let n = unit([1.0, 1.0, 1.0]);
    for p in [[1e6; 3], [-1e6; 3]] {
        let frame = framed(p, n, [1.0, 0.0, 0.0]);
        let reach = libm::sqrt(dot(frame.origin, frame.origin));
        assert!(reach > 1.732_050_807_568_877_2e6, "{p:?}: {reach}");
        assert!(reach <= MAX_FRAME_ORIGIN, "{p:?}: {reach}");
    }
}

#[test]
fn a_frames_axes_and_origin_reach_the_kernel_as_plus_zeros() {
    // A frame of −0s keys as the one of +0s (the faces step's line takes each + 0), so
    // what's sent must be the same: its origin and axes each + 0.
    let frame = FaceFrame {
        origin: [-0.0, -0.0, 7.0],
        x: [-1.0, -0.0, -0.0],
        y: [-0.0, 1.0, -0.0],
    };
    let plane = SketchPlane::Face(frame);
    let bits = |v: [f64; 3]| v.map(f64::to_bits);
    assert_eq!(bits(plane.origin()), bits([0.0, 0.0, 7.0]));
    assert_eq!(bits(plane.x_axis()), bits([-1.0, 0.0, 0.0]));
    assert_eq!(bits(plane.y_axis()), bits([0.0, 1.0, 0.0]));
}

/// Angles biased to the edges: none, a hair, the quarter turns and their neighbours, 45°,
/// and anything.
fn angle() -> impl Strategy<Value = f64> {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};
    prop_oneof![
        Just(0.0),
        Just(1e-15),
        Just(1e-9),
        Just(FRAC_PI_2),
        Just(PI),
        Just(-FRAC_PI_2),
        Just(FRAC_PI_4),
        (-1e-6..1e-6f64).prop_map(|d| FRAC_PI_2 + d),
        -PI..PI,
    ]
}

/// A unit axis biased to model space's axes and their diagonals, and any direction.
fn axis() -> impl Strategy<Value = [f64; 3]> {
    prop_oneof![
        Just([1.0, 0.0, 0.0]),
        Just([0.0, 1.0, 0.0]),
        Just([0.0, 0.0, -1.0]),
        Just(unit([1.0, 1.0, 1.0])),
        Just(unit([1.0, -1.0, 0.0])),
        (-1.0..1.0f64, -1.0..1.0f64, -1.0..1.0f64)
            .prop_filter("not tiny", |(a, b, c)| a * a + b * b + c * c > 1e-6)
            .prop_map(|(a, b, c)| unit([a, b, c])),
    ]
}

/// The six tokens' axes.
fn token() -> impl Strategy<Value = [f64; 3]> {
    prop_oneof![
        Just([1.0, 0.0, 0.0]),
        Just([-1.0, 0.0, 0.0]),
        Just([0.0, 1.0, 0.0]),
        Just([0.0, -1.0, 0.0]),
        Just([0.0, 0.0, 1.0]),
        Just([0.0, 0.0, -1.0]),
    ]
}

/// A face's unit normal: any direction, or one leaning off a token's axis by anything
/// from 1e-12 to a radian (a face nearly parallel to the stored axis), made a unit as
/// `describe` gives it, to rounding.
fn normal() -> impl Strategy<Value = [f64; 3]> {
    prop_oneof![
        axis(),
        (token(), axis(), -12.0..0.0f64).prop_map(|(a, lean, exponent)| {
            let size = libm::pow(10.0, exponent);
            unit([0, 1, 2].map(|k| a[k] + size * lean[k]))
        }),
    ]
}

/// A point within a kilometre on each axis, biased to the box's corners.
fn point() -> impl Strategy<Value = [f64; 3]> {
    let c = prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(MAX_LENGTH),
        Just(-MAX_LENGTH),
        Just(1e-300),
        -MAX_LENGTH..MAX_LENGTH,
    ];
    [c.clone(), c.clone(), c]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4000))]

    /// Every rotation's columns, from anywhere within reach, are a sound frame, whose normal
    /// is the rotation's third column: a frame is right-handed by construction.
    #[test]
    fn every_rotation_is_a_sound_frame_whose_normal_is_its_third_column(
        k in axis(), a in angle(), o in point()
    ) {
        let frame = rotation(k, a, o);
        prop_assert_eq!(frame.check(), Ok(()), "{:?}", frame);
        let n = SketchPlane::Face(frame).normal();
        let third = rotated(k, a, [0.0, 0.0, 1.0]);
        prop_assert!(off(n, third) <= 1e-15, "{:?} vs {:?}", n, third);
    }

    /// A frame as the core will make it (`FaceFrame::from_normal`), from any unit normal n,
    /// any of the six tokens a standing off it by at least 1e-12, and any point within a
    /// kilometre: made, and it passes the check (the check's tolerances admit what the
    /// construction's roundings make); an axis standing off it by less is refused. Its x is a's projection onto the plane made a unit,
    /// its normal n, and its origin n(n·p), each to rounding.
    #[test]
    fn a_frame_from_any_normal_and_token_passes_the_check(
        n in normal(), a in token(), p in point(),
    ) {
        let along = dot(a, n);
        let projected = [0, 1, 2].map(|k| a[k] - n[k] * along);
        let standing = libm::sqrt(dot(projected, projected));
        if standing < 1e-12 {
            // Along the normal: refused, saying how far it stands off.
            prop_assert_eq!(
                FaceFrame::from_normal(p, n, a),
                Err(FaceFrameError::AxisAlongNormal(standing))
            );
            return Ok(());
        }
        let frame = match FaceFrame::from_normal(p, n, a) {
            Ok(frame) => frame,
            Err(e) => return Err(TestCaseError::fail(format!("{n:?} {a:?} {p:?}: {e:?}"))),
        };
        prop_assert_eq!(frame.check(), Ok(()));
        prop_assert!((libm::sqrt(dot(frame.x, frame.x)) - 1.0).abs() <= 4.0 * f64::EPSILON);
        prop_assert!((libm::sqrt(dot(frame.y, frame.y)) - 1.0).abs() <= 4.0 * f64::EPSILON);
        prop_assert!(dot(frame.x, frame.y).abs() <= FACE_FRAME_SQUARENESS);
        // x lies in the plane, towards a; y is n × x; the normal is n.
        prop_assert!(dot(frame.x, n).abs() <= 4.0 * f64::EPSILON, "{:?}", frame.x);
        prop_assert!(dot(frame.x, a) > 0.0);
        let normal = SketchPlane::Face(frame).normal();
        prop_assert!(off(normal, n) <= 8.0 * f64::EPSILON, "{:?} vs {:?}", normal, n);
        let offset = dot(n, p);
        let o = n.map(|c| c * offset);
        prop_assert!(off(frame.origin, o) <= 4.0 * f64::EPSILON * MAX_LENGTH, "{:?}", frame.origin);
        // The reversed sketch's frame, of −n and −a: the same origin and y, x negated.
        let reversed = FaceFrame::from_normal(p, n.map(|c| -c), a.map(|c| -c));
        prop_assert_eq!(
            reversed.map(|r| (r.origin, r.x.map(|c| -c + 0.0), r.y)),
            Ok((frame.origin, frame.x, frame.y))
        );
    }

    /// A point placed on a rotation's frame is as far from the origin as the sketch's point
    /// is from (0, 0), and on the plane: its offset along n at most a few roundings of
    /// its size.
    #[test]
    fn a_placed_point_keeps_its_distance_and_stays_on_the_plane(
        k in axis(), a in angle(),
        u in -MAX_LENGTH..MAX_LENGTH, v in -MAX_LENGTH..MAX_LENGTH,
    ) {
        let frame = rotation(k, a, [0.0; 3]);
        let n = rotated(k, a, [0.0, 0.0, 1.0]);
        let p = SketchPlane::Face(frame).place([u, v]);
        let size = libm::sqrt(u * u + v * v);
        let got = libm::sqrt(dot(p, p));
        prop_assert!((got - size).abs() <= 1e-14 * size.max(1.0), "{} vs {}", got, size);
        prop_assert!(dot(p, n).abs() <= 1e-14 * size.max(1.0), "{:?}", p);
    }
}
