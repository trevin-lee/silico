//! M4.5: the least distance between two elements (`silico_kernel::distance`,
//! `Op::Distance`; docs/ports/measure.md), on OCCT's `BRepExtrema_DistShapeShape`: boxes'
//! faces, apart, touching, and across shapes; corners; a sphere against a plane (FreeCAD's
//! `testCurvedFaceDistance`); a cylinder's rims; the witness pair chosen by coordinates and
//! swapped with the elements, never a pair OCCT's filter leaves past its precision; and each
//! refusal. Every expected value is by hand.

use super::cube_elements::{cube, edge_between, vertex_at};

use std::path::Path;

use silico_core::ErrorCode;
use silico_core::kernel::{Blob, KernelRequest, KernelWorker, ShapeData};
use silico_kernel::{
    Primitive, Shape, describe_whole, distance, extrude, make, to_brep, translate,
};
use silico_protocol::{EdgeCurve, ElementKind, ElementRef, Profile, Refusal};

fn moved(shape: &Shape, by: [f64; 3]) -> Shape {
    translate(shape, by).unwrap().shape
}

fn edge(index: u32) -> ElementRef {
    ElementRef::edge(index)
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
    // A pair at each of the four corners.
    assert_eq!(d.solutions, 4);
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
    let d = distance(&shape, low, None, high).unwrap();
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
        &Refusal::ElementMissing {
            parameter: "second".to_owned(),
            element: ElementRef::face(7),
            count: 6,
        }
    );
    let zero = distance(&shape, edge(0), None, edge(1)).unwrap_err();
    assert_eq!(
        zero.refusal(),
        &Refusal::InvalidParameter {
            parameter: "first".to_owned(),
            message: "elements are numbered from 1, so first can't be 0".to_owned(),
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
        &Refusal::ElementMissing {
            parameter: "second".to_owned(),
            element: ElementRef::face(2),
            count: 1,
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
        "second, face 9, doesn't exist: its shape has faces 1 to 6"
    );
    // The worker lives on.
    let request =
        KernelRequest::distance(&data(&a), ElementRef::face(5), None, ElementRef::face(6)).unwrap();
    assert_eq!(worker.run(request).unwrap().distance, 10.0);
}

#[test]
fn circles_centres_apart_are_their_gap_apart_as_freecads_circle_test_has_them() {
    // FreeCAD's testCircleCircle: circles of radius 1 centred at (0, 0, 0) and (3, 4, 0),
    // centres 5 apart, so 5 − 1 − 1 = 3 apart where they're nearest, on the line between
    // the centres: (0.6, 0.8, 0) and (2.4, 3.2, 0). Each the bottom rim of a cylinder.
    let rim = |at: [f64; 3]| {
        let cylinder = moved(
            &make(&Primitive::Cylinder {
                radius: 1.0,
                height: 1.0,
            })
            .unwrap(),
            at,
        );
        let d = describe_whole(&cylinder).unwrap();
        let bottom = d
            .edges
            .iter()
            .position(|e| matches!(e.curve, EdgeCurve::Circle { .. }) && e.start[2] == 0.0)
            .unwrap();
        (cylinder, edge(u32::try_from(bottom + 1).unwrap()))
    };
    let (a, a_rim) = rim([0.0, 0.0, 0.0]);
    let (b, b_rim) = rim([3.0, 4.0, 0.0]);
    let d = distance(&a, a_rim, Some(&b), b_rim).unwrap();
    assert!((d.distance - 3.0).abs() < 1e-9, "{}", d.distance);
    assert_point("from", d.from, [0.6, 0.8, 0.0]);
    assert_point("to", d.to, [2.4, 3.2, 0.0]);
}

#[test]
fn corners_of_two_shapes_are_measured_across_them() {
    // FreeCAD's testTwoBoxesMovedByContainers, without containers: a 10 mm cube, and
    // another moved 50 along x; the first's corner at the origin and the second's at
    // (50, 0, 0) are 50 apart.
    let a = cube(10.0);
    let b = moved(&a, [50.0, 0.0, 0.0]);
    let origin = vertex_at(&a, [0.0, 0.0, 0.0]);
    let there = vertex_at(&b, [50.0, 0.0, 0.0]);
    let d = distance(&a, origin, Some(&b), there).unwrap();
    assert_eq!(d.distance, 50.0);
    assert_eq!(d.from, [0.0, 0.0, 0.0]);
    assert_eq!(d.to, [50.0, 0.0, 0.0]);
}

#[test]
fn a_pair_occts_filter_leaves_past_its_precision_is_never_the_witness() {
    // OCCT's last step (BRepExtrema_DistShapeShape::Perform) removes each pair further than
    // its precision (1e-7) from the distance, but its loop steps on after a removal, so the
    // pair after each removed one is never looked at. Two edges 1250 mm apart along x,
    // crossed: a, in x = 0, from A' = (0, −0.02, 0) to A = (0, 0, 0); b, in x = 1250, from
    // B' = (1250, 0, 0.014) to B = (1250, 0, 0). A and B are nearest, 1250 apart; each
    // other pair's excess over 1250 is its offset across, squared, over 2 × 1250 (to 1e-15):
    // A'B' (0.02² + 0.014²) / 2500 = 2.384e-7, A'B 0.02² / 2500 = 1.6e-7, AB'
    // 0.014² / 2500 = 0.784e-7. OCCT pairs the vertices first, a's first, each from its
    // first vertex (A', B'), keeping a pair within 1e-7 of the least so far and lowering
    // it: A'B', A'B (0.784e-7 below), AB' (0.816e-7 below), AB (0.784e-7 below), so its
    // reference ends at 1250, every step within its precision. Its filter removes A'B'
    // (2.384e-7 over), steps past A'B, which moved into its place, and keeps AB' and AB:
    // three pairs, A'B 1.6e-7 over (each element's other searches find their least at the
    // vertices, which these pairs have). By the witness's order, A'B comes first (its lesser
    // point, A', is below A by y): unfiltered, the witness would be a pair 1.6e-7 further
    // apart than the distance. Within the precision, AB' and AB, of which AB is first (B is
    // below B' by z): the nearest pair itself.
    let prism = |points: Vec<[f64; 3]>| extrude(&Profile { points }, 1.0, None).unwrap().shape;
    let (a_far, a_near) = ([0.0, -0.02, 0.0], [0.0, 0.0, 0.0]);
    let (b_far, b_near) = ([1250.0, 0.0, 0.014], [1250.0, 0.0, 0.0]);
    // Each edge the first of its triangle's profile, so it runs from the far end to the
    // near one, as the walk above has it.
    let a = prism(vec![a_far, a_near, [0.0, -0.01, -1.0]]);
    let b = prism(vec![b_far, b_near, [1250.0, 1.0, 0.007]]);
    let (ea, eb) = (
        edge_between(&a, a_far, a_near),
        edge_between(&b, b_far, b_near),
    );
    let runs = |shape: &Shape, e: ElementRef| {
        let edge = &describe_whole(shape).unwrap().edges[e.index as usize - 1];
        (edge.start, edge.end)
    };
    assert_eq!(runs(&a, ea), (a_far, a_near));
    assert_eq!(runs(&b, eb), (b_far, b_near));
    let d = distance(&a, ea, Some(&b), eb).unwrap();
    assert_eq!(d.distance, 1250.0);
    assert_eq!((d.from, d.to), (a_near, b_near));
    assert_eq!(d.solutions, 3);
    // Swapped: the same pair, its points swapped.
    let back = distance(&b, eb, Some(&a), ea).unwrap();
    assert_eq!(
        (back.distance, back.from, back.to),
        (1250.0, b_near, a_near)
    );
    assert_eq!(back.solutions, 3);
}
