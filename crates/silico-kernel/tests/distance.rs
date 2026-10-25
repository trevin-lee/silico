//! M4.5: the least distance between two elements (`silico_kernel::distance`,
//! `Op::Distance`; docs/ports/measure.md), on OCCT's `BRepExtrema_DistShapeShape`: boxes'
//! faces, apart, touching, and across shapes; corners; a sphere against a plane (FreeCAD's
//! `testCurvedFaceDistance`); a cylinder's rims; the witness pair chosen by coordinates and
//! swapped with the elements; and each refusal. Every expected value is by hand.

use std::path::Path;

use silico_core::ErrorCode;
use silico_core::kernel::{Blob, KernelRequest, KernelWorker, ShapeData};
use silico_kernel::{Primitive, Shape, describe_whole, distance, make, to_brep, translate};
use silico_protocol::{EdgeCurve, ElementKind, ElementRef, Refusal};

fn cube(side: f64) -> Shape {
    make(&Primitive::Box {
        dx: side,
        dy: side,
        dz: side,
    })
    .unwrap()
}

fn moved(shape: &Shape, by: [f64; 3]) -> Shape {
    translate(shape, by).unwrap().shape
}

fn vertex(index: u32) -> ElementRef {
    ElementRef {
        kind: ElementKind::Vertex,
        index,
    }
}

fn edge(index: u32) -> ElementRef {
    ElementRef::edge(index)
}

/// The number of `shape`'s vertex at `at`.
fn vertex_at(shape: &Shape, at: [f64; 3]) -> u32 {
    let d = describe_whole(shape).unwrap();
    let i = d.vertices.iter().position(|v| *v == at).unwrap();
    u32::try_from(i + 1).unwrap()
}

fn assert_point(what: &str, got: [f64; 3], want: [f64; 3]) {
    let off = (0..3).map(|i| (got[i] - want[i]).abs()).fold(0.0, f64::max);
    assert!(off < 1e-9, "{what}: got {got:?}, want {want:?}");
}

#[test]
fn a_boxs_bottom_and_top_are_its_height_apart_their_witness_its_first_corners() {
    // A 10 × 20 × 30 box: Face5 is z = 0, Face6 z = 30. OCCT finds the pairs at the
    // corners; the witness is the pair whose lesser point is least, (0, 0, 0) to (0, 0, 30).
    let shape = make(&Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    })
    .unwrap();
    let d = distance(&shape, ElementRef::face(5), None, ElementRef::face(6)).unwrap();
    assert_eq!(d.distance, 30.0);
    assert_eq!(d.from, [0.0, 0.0, 0.0]);
    assert_eq!(d.to, [0.0, 0.0, 30.0]);
    assert!(d.solutions >= 1);
    // Swapped: the points swap, and nothing else.
    let back = distance(&shape, ElementRef::face(6), None, ElementRef::face(5)).unwrap();
    assert_eq!(back.distance, 30.0);
    assert_eq!((back.from, back.to), (d.to, d.from));
    assert_eq!(back.solutions, d.solutions);
    // The same face twice is 0 apart.
    let same = distance(&shape, ElementRef::face(5), None, ElementRef::face(5)).unwrap();
    assert_eq!(same.distance, 0.0);
    assert_eq!(same.from, same.to);
}

#[test]
fn faces_that_meet_are_0_apart_where_they_meet() {
    // Face1 (x = 0) and Face5 (z = 0) of a 10 mm cube share the edge along y at x = z = 0.
    let shape = cube(10.0);
    let d = distance(&shape, ElementRef::face(1), None, ElementRef::face(5)).unwrap();
    assert!(d.distance.abs() < 1e-12, "{}", d.distance);
    assert_point("from", d.from, d.to);
    assert!(
        d.from[0].abs() < 1e-12 && d.from[2].abs() < 1e-12,
        "{:?}",
        d.from
    );
}

#[test]
fn corners_are_their_diagonal_apart() {
    // (0, 0, 0) and (10, 20, 30): √(100 + 400 + 900) = √1400 = 37.416573867739413.
    let shape = make(&Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    })
    .unwrap();
    let low = vertex_at(&shape, [0.0, 0.0, 0.0]);
    let high = vertex_at(&shape, [10.0, 20.0, 30.0]);
    let d = distance(&shape, vertex(low), None, vertex(high)).unwrap();
    assert!(
        (d.distance - 1400f64.sqrt()).abs() < 1e-12,
        "{}",
        d.distance
    );
    assert_eq!(d.from, [0.0, 0.0, 0.0]);
    assert_eq!(d.to, [10.0, 20.0, 30.0]);
    assert_eq!(d.solutions, 1);
}

#[test]
fn elements_of_two_shapes_are_measured_each_on_its_own() {
    // A 10 mm cube, and another 40 up: the first's top (z = 10) and the second's bottom
    // (z = 40) are 30 apart; the first's bottom and the second's top (z = 50) 50.
    let a = cube(10.0);
    let b = moved(&a, [0.0, 0.0, 40.0]);
    let d = distance(&a, ElementRef::face(6), Some(&b), ElementRef::face(5)).unwrap();
    assert_eq!(d.distance, 30.0);
    assert_eq!(d.from, [0.0, 0.0, 10.0]);
    assert_eq!(d.to, [0.0, 0.0, 40.0]);
    let d = distance(&a, ElementRef::face(5), Some(&b), ElementRef::face(6)).unwrap();
    assert_eq!(d.distance, 50.0);
    // Without the other shape, the second element is the first shape's own.
    let own = distance(&a, ElementRef::face(5), None, ElementRef::face(6)).unwrap();
    assert_eq!(own.distance, 10.0);
}

#[test]
fn faces_apart_both_across_and_up_are_the_hypotenuse_apart() {
    // A 10 mm cube's top (z = 10, x from 0 to 10) and another's bottom moved 15 along x and
    // 20 up (z = 20, x from 15 to 25): 5 across and 10 up, √125 = 11.180339887498949, met
    // along the near edges.
    let a = cube(10.0);
    let b = moved(&a, [15.0, 0.0, 20.0]);
    let d = distance(&a, ElementRef::face(6), Some(&b), ElementRef::face(5)).unwrap();
    assert!((d.distance - 125f64.sqrt()).abs() < 1e-9, "{}", d.distance);
    assert_point("from", d.from, [10.0, 0.0, 10.0]);
    assert_point("to", d.to, [15.0, 0.0, 20.0]);
}

#[test]
fn a_spheres_face_is_its_gap_from_a_plane_as_freecad_measures_it() {
    // FreeCAD's testCurvedFaceDistance: a sphere of radius 5 at the origin, and a box's face
    // at x = 20: 15, met at (5, 0, 0) and (20, 0, 0).
    let sphere = make(&Primitive::Sphere { radius: 5.0 }).unwrap();
    let wall = moved(&cube(10.0), [20.0, -5.0, -5.0]);
    let d = distance(
        &sphere,
        ElementRef::face(1),
        Some(&wall),
        ElementRef::face(1),
    )
    .unwrap();
    assert!((d.distance - 15.0).abs() < 1e-7, "{}", d.distance);
    let off = |p: [f64; 3], q: [f64; 3]| (0..3).map(|i| (p[i] - q[i]).abs()).fold(0.0, f64::max);
    assert!(off(d.from, [5.0, 0.0, 0.0]) < 1e-6, "{:?}", d.from);
    assert!(off(d.to, [20.0, 0.0, 0.0]) < 1e-6, "{:?}", d.to);
}

#[test]
fn a_cylinders_rims_are_its_height_apart() {
    // A cylinder of radius 2.5 and height 10: its two circle edges, at z = 0 and z = 10.
    let cylinder = make(&Primitive::Cylinder {
        radius: 2.5,
        height: 10.0,
    })
    .unwrap();
    let d = describe_whole(&cylinder).unwrap();
    let rims: Vec<u32> = d
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.curve, EdgeCurve::Circle { .. }))
        .map(|(i, _)| u32::try_from(i + 1).unwrap())
        .collect();
    assert_eq!(rims.len(), 2);
    let apart = distance(&cylinder, edge(rims[0]), None, edge(rims[1])).unwrap();
    assert!((apart.distance - 10.0).abs() < 1e-9, "{}", apart.distance);
    // The witness is one of the rims' points straight above the other.
    assert!((apart.from[0] - apart.to[0]).abs() < 1e-9, "{apart:?}");
    assert!((apart.from[1] - apart.to[1]).abs() < 1e-9, "{apart:?}");
}

#[test]
fn an_element_its_shape_hasnt_or_a_solid_is_refused_naming_which() {
    let shape = cube(10.0);
    let missing = distance(&shape, ElementRef::face(1), None, ElementRef::face(7)).unwrap_err();
    assert_eq!(
        missing.refusal(),
        &Refusal::InvalidParameter {
            parameter: "second".to_owned(),
            message: "second is Face7, and its shape has 6 faces".to_owned(),
        }
    );
    let zero = distance(&shape, edge(0), None, edge(1)).unwrap_err();
    assert_eq!(
        zero.refusal(),
        &Refusal::InvalidParameter {
            parameter: "first".to_owned(),
            message: "first is Edge0, and its shape has 12 edges".to_owned(),
        }
    );
    // Of the other shape, counted on it: a sphere has one face.
    let sphere = make(&Primitive::Sphere { radius: 1.0 }).unwrap();
    let other = distance(
        &shape,
        ElementRef::face(6),
        Some(&sphere),
        ElementRef::face(2),
    )
    .unwrap_err();
    assert_eq!(
        other.refusal(),
        &Refusal::InvalidParameter {
            parameter: "second".to_owned(),
            message: "second is Face2, and its shape has 1 face".to_owned(),
        }
    );
    let solid = ElementRef {
        kind: ElementKind::Solid,
        index: 1,
    };
    let refused = distance(&shape, solid, None, ElementRef::face(1)).unwrap_err();
    assert_eq!(
        refused.refusal(),
        &Refusal::InvalidParameter {
            parameter: "first".to_owned(),
            message: "first must be a face, an edge, or a vertex, not a solid".to_owned(),
        }
    );
}

#[test]
fn through_a_worker_the_distance_is_the_bridges_and_a_refusal_is_data() {
    let mut worker =
        KernelWorker::start_worker(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap();
    let a = cube(10.0);
    let b = moved(&a, [0.0, 0.0, 40.0]);
    let data = |shape: &Shape| ShapeData::checked(Blob::inline(to_brep(shape).unwrap()), true);
    let request = KernelRequest::distance(
        &data(&a),
        ElementRef::face(6),
        Some(&data(&b)),
        ElementRef::face(5),
    )
    .unwrap();
    let through = worker.run(request).unwrap();
    let here = distance(&a, ElementRef::face(6), Some(&b), ElementRef::face(5)).unwrap();
    assert_eq!(through, here);
    // An element the shape hasn't: the worker's refusal, as the core's error, naming it.
    let request =
        KernelRequest::distance(&data(&a), ElementRef::face(1), None, ElementRef::face(9)).unwrap();
    let error = worker.run(request).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.parameter(), Some("second"));
    assert_eq!(
        error.message(),
        "second is Face9, and its shape has 6 faces"
    );
    // The worker lives on.
    let request =
        KernelRequest::distance(&data(&a), ElementRef::face(5), None, ElementRef::face(6)).unwrap();
    assert_eq!(worker.run(request).unwrap().distance, 10.0);
}
