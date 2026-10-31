//! M5.9, hostile face frames (`/break-it`): `FaceFrame::check` at and either side of its
//! bounds, frames built the ways the core will build them (a rotation's columns, or a
//! face's normal and a stored axis projected onto its plane, as docs/ports/attacher.md's
//! F1 and F2 derive them), and `SketchPlane::place` on them. Every expected value is a
//! hand calculation or a property of a rotation.

use proptest::prelude::*;
use silico_protocol::{FACE_FRAME_TOLERANCE, FaceFrame, FaceRegions, MAX_LENGTH, SketchPlane};

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

/// The rotation by `angle` about the unit `k` (Rodrigues), as a frame: its columns x, y,
/// and n, from `origin`.
fn rotation(k: [f64; 3], angle: f64, origin: [f64; 3]) -> FaceFrame {
    let (s, c) = (libm::sin(angle), libm::cos(angle));
    let column = |e: [f64; 3]| -> [f64; 3] {
        // R e = e cos + (k × e) sin + k (k·e)(1 − cos)
        let kxe = cross(k, e);
        let ke = dot(k, e);
        [0, 1, 2].map(|i| e[i] * c + kxe[i] * s + k[i] * ke * (1.0 - c))
    };
    FaceFrame {
        origin,
        x: column([1.0, 0.0, 0.0]),
        y: column([0.0, 1.0, 0.0]),
        normal: column([0.0, 0.0, 1.0]),
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
        normal: [0.0, 0.0, 1.0],
    }
}

#[test]
fn a_frames_origin_exactly_a_kilometre_out_passes_and_a_hair_past_is_refused() {
    for k in 0..3 {
        for sign in [1.0, -1.0] {
            let mut at = xy();
            at.origin[k] = sign * MAX_LENGTH;
            assert_eq!(at.check(), Ok(()), "{at:?}");
            let mut past = xy();
            past.origin[k] = sign * f64::from_bits(MAX_LENGTH.to_bits() + 1);
            refused(past, "the face frame's origin must be within 1e6 mm");
        }
    }
}

#[test]
fn a_frame_of_negative_zeros_and_subnormals_is_sound_and_places_its_origin_as_plus_zero() {
    let frame = FaceFrame {
        origin: [-0.0, 5e-324, -5e-324],
        x: [1.0, -0.0, 5e-324],
        y: [-0.0, 1.0, -0.0],
        normal: [-5e-324, 0.0, 1.0],
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
            normal: [f64::MAX, f64::MAX, 0.0],
            ..xy()
        },
        "the face frame's normal must be a unit vector",
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
fn a_frame_of_the_xy_plane_with_x_and_y_swapped_is_left_handed() {
    refused(
        FaceFrame {
            x: [0.0, 1.0, 0.0],
            y: [1.0, 0.0, 0.0],
            ..xy()
        },
        "must be x × y",
    );
    // Swapped and the normal reversed: right-handed again, the XY plane seen from below.
    assert_eq!(
        FaceFrame {
            x: [0.0, 1.0, 0.0],
            y: [1.0, 0.0, 0.0],
            normal: [0.0, 0.0, -1.0],
            ..xy()
        }
        .check(),
        Ok(())
    );
}

#[test]
fn a_bad_frame_is_refused_before_regions_whatever_they_hold() {
    // A frame that fails says so even when the regions are nonsense too: the plane's
    // refusal names the plane.
    let regions = FaceRegions {
        plane: SketchPlane::Face(FaceFrame {
            normal: [0.0, 0.0, f64::NAN],
            ..xy()
        }),
        regions: vec![],
    };
    let err = regions.check().unwrap_err();
    assert_eq!(err.parameter, "plane", "{}", err.message);
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

fn origin() -> impl Strategy<Value = [f64; 3]> {
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

    /// Every rotation's columns, from anywhere within a kilometre, are a sound frame; the
    /// same with its normal reversed is left-handed, and refused.
    #[test]
    fn every_rotation_is_a_sound_frame_and_its_mirror_is_not(
        k in axis(), a in angle(), o in origin()
    ) {
        let frame = rotation(k, a, o);
        prop_assert_eq!(frame.check(), Ok(()), "{:?}", frame);
        let mirrored = FaceFrame { normal: frame.normal.map(|c| -c), ..frame };
        let err = mirrored.check().unwrap_err();
        prop_assert!(err.message.contains("x × y"), "{}", err.message);
    }

    /// A frame as F1 and F2 derive it: a face's unit normal n, a stored axis a projected onto
    /// its plane and made a unit, x, and y = n × x. Whatever n is, and whatever axis is
    /// stored, so long as it's at least 0.01 rad (about half a degree) from n, the frame
    /// passes: the check's tolerance must admit what a few roundings make.
    #[test]
    fn a_frame_of_a_normal_and_a_projected_stored_axis_passes(
        n in axis(),
        stored in prop_oneof![
            Just([1.0, 0.0, 0.0]), Just([-1.0, 0.0, 0.0]),
            Just([0.0, 1.0, 0.0]), Just([0.0, -1.0, 0.0]),
            Just([0.0, 0.0, 1.0]), Just([0.0, 0.0, -1.0]),
        ],
        o in origin(),
    ) {
        let along = dot(stored, n);
        prop_assume!(libm::acos(along.abs().min(1.0)) >= 0.01);
        let x = unit([0, 1, 2].map(|i| stored[i] - n[i] * along));
        let y = cross(n, x);
        let frame = FaceFrame { origin: o, x, y, normal: n };
        prop_assert_eq!(frame.check(), Ok(()), "{:?}", frame);
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
        let p = SketchPlane::Face(frame).place([u, v]);
        let size = libm::sqrt(u * u + v * v);
        let got = libm::sqrt(dot(p, p));
        prop_assert!((got - size).abs() <= 1e-14 * size.max(1.0), "{} vs {}", got, size);
        prop_assert!(dot(p, frame.normal).abs() <= 1e-14 * size.max(1.0), "{:?}", p);
    }
}

#[test]
fn a_frames_tolerance_times_a_sketchs_reach_is_off_its_plane_by_more_than_occts() {
    // The check admits a normal leaning 0.9e-12 off x × y; a sketch's point may be 1e6 from
    // the sketch's origin. Placed, the point is off the plane through the origin with that
    // normal by 1e6 · 0.9e-12 = 9e-7 mm: nine times OCCT's tolerance (1e-7), which every
    // vertex and edge of the faces must be on their plane to. So the kernel builds the faces'
    // plane spanned by x and y, not from the normal (silico-kernel adversarial_face_frames,
    // the same frame).
    let frame = FaceFrame {
        normal: [0.9e-12, 0.0, 1.0],
        ..xy()
    };
    assert_eq!(frame.check(), Ok(()));
    const { assert!(FACE_FRAME_TOLERANCE * MAX_LENGTH > silico_protocol::LENGTH_TOLERANCE) };
    let p = SketchPlane::Face(frame).place([MAX_LENGTH, 0.0]);
    let off = dot(p, unit(frame.normal)).abs();
    assert!(off > 8.9e-7 && off < 9.1e-7, "{off}");
}
