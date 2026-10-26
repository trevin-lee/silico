//! `classify`: where a point is against a shape (inside, on its boundary, or outside), by
//! OCCT's solid classifier (M4.6: a camera's eye inside a body), against what each shape is
//! by hand.

use silico_kernel::{
    BooleanOp, PointPlace, Primitive, Shape, boolean, classify, make, make_box, translate,
};
use silico_protocol::Refusal;

/// Whether `point` is inside `shape`, on its boundary counted as inside, as the core counts
/// it (`KernelRequest::classify`).
fn inside(shape: &Shape, point: [f64; 3]) -> Result<bool, silico_kernel::KernelError> {
    classify(shape, point).map(|place| place != PointPlace::Outside)
}

fn sphere(radius: f64) -> Shape {
    make(&Primitive::Sphere { radius }).unwrap()
}

#[test]
fn a_boxs_inside_and_its_boundary_are_inside_and_past_it_isnt() {
    // The box from the origin to (40, 20, 10).
    let b = make_box(40.0, 20.0, 10.0).unwrap();
    for (point, want) in [
        ([20.0, 10.0, 5.0], true),
        ([1e-3, 1e-3, 1e-3], true),
        // On a face, an edge, a vertex: on the boundary counts as inside.
        ([20.0, 0.0, 5.0], true),
        ([40.0, 20.0, 5.0], true),
        ([40.0, 20.0, 10.0], true),
        ([0.0, 0.0, 0.0], true),
        // Within 1e-8 of a face: on it. OCCT's classifier counts a point on a face to about
        // the box's tolerance (1e-7), its band varying by face (to 5e-8 on y = 0, to 9e-8 on
        // x = 40), so nothing between is pinned.
        ([20.0, -1e-8, 5.0], true),
        ([40.000_000_01, 10.0, 5.0], true),
        ([20.0, 10.0, 10.000_000_01], true),
        // Past it, by twice the tolerance or more.
        ([20.0, -2e-7, 5.0], false),
        ([40.000_000_2, 10.0, 5.0], false),
        ([20.0, 10.0, 10.000_000_2], false),
        ([-1.0, -1.0, -1.0], false),
        ([1e9, 1e9, 1e9], false),
        ([20.0, 10.0, 10.5], false),
    ] {
        assert_eq!(inside(&b, point).unwrap(), want, "{point:?}");
    }
}

#[test]
fn a_sphere_holds_what_its_radius_does() {
    let s = sphere(10.0);
    for (point, want) in [
        ([0.0, 0.0, 0.0], true),
        ([9.999, 0.0, 0.0], true),
        ([10.0, 0.0, 0.0], true),
        ([6.0, 8.0, 0.0], true),
        // 5e-8 past the radius, within the tolerance: on it; 5e-6 past, outside.
        ([6.0, 8.0, 1e-3], true),
        ([6.0, 8.0, 1e-2], false),
        ([7.1, 7.1, 0.0], false),
        ([0.0, 0.0, -10.001], false),
    ] {
        assert_eq!(inside(&s, point).unwrap(), want, "{point:?}");
    }
}

#[test]
fn a_toruss_hole_and_its_tube() {
    // Major radius 10, minor 2: the tube's centre circle has radius 10 in z = 0.
    let t = make(&Primitive::Torus {
        major_radius: 10.0,
        minor_radius: 2.0,
    })
    .unwrap();
    for (point, want) in [
        ([0.0, 0.0, 0.0], false),
        ([10.0, 0.0, 0.0], true),
        ([0.0, -11.9, 0.0], true),
        ([0.0, -12.1, 0.0], false),
        ([7.9, 0.0, 0.0], false),
        ([10.0, 0.0, 1.9], true),
        ([10.0, 0.0, 2.1], false),
    ] {
        assert_eq!(inside(&t, point).unwrap(), want, "{point:?}");
    }
}

#[test]
fn either_of_two_bodies_holds_a_point_and_the_gap_between_them_doesnt() {
    // Two unit cubes, from the origin and from (3, 0, 0), as one shape.
    let a = make_box(1.0, 1.0, 1.0).unwrap();
    let b = translate(&a, [3.0, 0.0, 0.0]).unwrap().shape;
    let both = boolean(BooleanOp::Fuse, &a, &b).unwrap().shape;
    for (point, want) in [
        ([0.5, 0.5, 0.5], true),
        ([3.5, 0.5, 0.5], true),
        ([2.0, 0.5, 0.5], false),
        ([1.0, 0.5, 0.5], true),
    ] {
        assert_eq!(inside(&both, point).unwrap(), want, "{point:?}");
    }
}

#[test]
fn a_point_that_isnt_finite_is_refused_naming_it() {
    let b = make_box(1.0, 1.0, 1.0).unwrap();
    for point in [
        [f64::NAN, 0.0, 0.0],
        [0.0, f64::INFINITY, 0.0],
        [0.0, 0.0, f64::NEG_INFINITY],
    ] {
        let e = inside(&b, point).unwrap_err();
        assert!(
            matches!(e.refusal(), Refusal::InvalidParameter { parameter, .. } if parameter == "point"),
            "{point:?}: {e:?}"
        );
    }
}

#[test]
fn the_classifier_tells_inside_from_on_the_boundary_from_outside() {
    // The box from the origin to (40, 20, 10): its centre, a face's, an edge's middle, a
    // corner, and a point past it.
    let b = make_box(40.0, 20.0, 10.0).unwrap();
    for (point, place) in [
        ([20.0, 10.0, 5.0], PointPlace::Inside),
        ([20.0, 0.0, 5.0], PointPlace::OnBoundary),
        ([40.0, 20.0, 5.0], PointPlace::OnBoundary),
        ([0.0, 0.0, 0.0], PointPlace::OnBoundary),
        ([20.0, -1.0, 5.0], PointPlace::Outside),
    ] {
        assert_eq!(classify(&b, point).unwrap(), place, "{point:?}");
    }
}

#[test]
fn a_point_as_far_as_may_be_is_classified_and_one_past_it_refused() {
    // 1e12 mm, either way along each axis, outside the box; an ulp past, refused.
    let b = make_box(40.0, 20.0, 10.0).unwrap();
    for point in [[1e12, 10.0, 5.0], [-1e12, 10.0, 5.0], [20.0, 1e12, -1e12]] {
        assert_eq!(
            classify(&b, point).unwrap(),
            PointPlace::Outside,
            "{point:?}"
        );
    }
    for point in [
        [1e12f64.next_up(), 10.0, 5.0],
        [20.0, -1e12f64.next_up(), 5.0],
        [20.0, 10.0, 1e12f64.next_up()],
    ] {
        let e = classify(&b, point).unwrap_err();
        assert!(
            matches!(e.refusal(), Refusal::InvalidParameter { parameter, .. } if parameter == "point"),
            "{point:?}: {e:?}"
        );
    }
}
