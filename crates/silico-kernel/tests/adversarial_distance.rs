//! break-it: hostile and degenerate inputs for the least distance between two elements
//! (`silico_kernel::distance`, `Op::Distance`, `silico_protocol::witness`; M4.5,
//! docs/ports/measure.md): element numbers at and past their ends, an empty compound, each
//! kind against each kind, degenerated edges (a sphere's poles, a cone's apex) and seams,
//! points every direction from a surface, curved pairs whose least distance is met along a
//! circle or a whole surface, parts far from the origin, tiny, and a nanometre apart, the
//! same request in fresh workers and with its elements swapped, garbage sent to a worker,
//! the fake's refusals against a worker's, and the witness's order over hostile coordinates.
//! Every expected value is a closed form, by hand, named where it's used.

use super::kernel;

use std::path::Path;
use std::time::Duration;

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Kernel, KernelRequest};
use silico_core::pool::Outcome;
use silico_kernel::{
    Primitive, Shape, distance, from_text_brep, make, test_ops, to_brep, translate,
};
use silico_protocol::{Blob, Distance, ElementKind, ElementRef, Refusal, ShapeData, witness};

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/brep")
        .join(name);
    std::fs::read(path).unwrap()
}

fn cube(side: f64) -> Shape {
    make(&Primitive::Box {
        dx: side,
        dy: side,
        dz: side,
    })
    .unwrap()
}

fn sphere(radius: f64) -> Shape {
    make(&Primitive::Sphere { radius }).unwrap()
}

fn cylinder(radius: f64, height: f64) -> Shape {
    make(&Primitive::Cylinder { radius, height }).unwrap()
}

fn torus() -> Shape {
    make(&Primitive::Torus {
        major_radius: 10.0,
        minor_radius: 2.0,
    })
    .unwrap()
}

fn moved(shape: &Shape, by: [f64; 3]) -> Shape {
    translate(shape, by).unwrap().shape
}

fn face(index: u32) -> ElementRef {
    ElementRef::face(index)
}

fn edge(index: u32) -> ElementRef {
    ElementRef::edge(index)
}

fn vertex(index: u32) -> ElementRef {
    ElementRef {
        kind: ElementKind::Vertex,
        index,
    }
}

fn length(p: [f64; 3]) -> f64 {
    libm::sqrt(p[0] * p[0] + p[1] * p[1] + p[2] * p[2])
}

fn apart(p: [f64; 3], q: [f64; 3]) -> f64 {
    length([q[0] - p[0], q[1] - p[1], q[2] - p[2]])
}

fn assert_near(what: &str, got: f64, want: f64, tolerance: f64) {
    assert!(
        (got - want).abs() <= tolerance,
        "{what}: got {got:e}, want {want:e} (off {:e})",
        (got - want).abs()
    );
}

fn assert_point(what: &str, got: [f64; 3], want: [f64; 3], tolerance: f64) {
    let off = (0..3).map(|i| (got[i] - want[i]).abs()).fold(0.0, f64::max);
    assert!(off <= tolerance, "{what}: got {got:?}, want {want:?}");
}

/// A distance found, and its witness pair as far apart as it says, to OCCT's precision
/// (1e-7, `Precision::Confusion`, which `BRepExtrema_DistShapeShape` keeps solutions within).
fn measured(
    what: &str,
    shape: &Shape,
    first: ElementRef,
    other: Option<&Shape>,
    second: ElementRef,
) -> Distance {
    let d = distance(shape, first, other, second)
        .unwrap_or_else(|e| panic!("{what}: refused: {:?} ({:?})", e.refusal(), e.cause()));
    assert!(d.solutions >= 1, "{what}: {d:?}");
    assert_near(
        &format!("{what}: the witness's length against the distance"),
        apart(d.from, d.to),
        d.distance,
        1e-7,
    );
    d
}

fn refused(shape: &Shape, first: ElementRef, other: Option<&Shape>, second: ElementRef) -> Refusal {
    distance(shape, first, other, second)
        .unwrap_err()
        .refusal()
        .clone()
}

fn missing(parameter: &str, element: ElementRef, count: u32) -> Refusal {
    Refusal::ElementMissing {
        parameter: parameter.to_owned(),
        element,
        count,
    }
}

// ---- Element numbers ----

#[test]
fn elements_numbered_past_the_count_or_u32_max_are_refused_naming_which() {
    // A cube: 6 faces, 12 edges, 8 vertices. The last of each is measured; one past it,
    // and u32::MAX (which the bridge casts to an int only after the range check), refused.
    let shape = cube(10.0);
    // Face6 (z = 10) holds Edge12 ((0, 10, 10) to (10, 10, 10)); Vertex8 (10, 10, 0) is 10
    // below that edge's end.
    assert_eq!(
        measured("Face6", &shape, face(6), None, edge(12)).distance,
        0.0
    );
    assert_eq!(
        measured("Vertex8", &shape, vertex(8), None, edge(12)).distance,
        10.0
    );
    for (first, second, want) in [
        (face(7), face(1), missing("first", face(7), 6)),
        (face(1), edge(13), missing("second", edge(13), 12)),
        (vertex(9), vertex(1), missing("first", vertex(9), 8)),
        (face(u32::MAX), face(1), missing("first", face(u32::MAX), 6)),
        (
            face(1),
            vertex(u32::MAX),
            missing("second", vertex(u32::MAX), 8),
        ),
        // Both out of range: the first is named.
        (face(7), edge(13), missing("first", face(7), 6)),
    ] {
        assert_eq!(
            refused(&shape, first, None, second),
            want,
            "{first} {second}"
        );
    }
    // Counted on the other shape when there is one: a sphere has 3 edges (its seam and
    // its two poles), so Edge12 is the cube's but not the sphere's.
    let ball = sphere(1.0);
    assert_eq!(
        refused(&shape, face(1), Some(&ball), edge(12)),
        missing("second", edge(12), 3)
    );
}

#[test]
fn a_compound_of_nothing_has_no_element_to_measure() {
    // An empty compound has 0 of every kind, as either shape.
    let empty = test_ops::empty_shape().unwrap();
    let shape = cube(10.0);
    assert_eq!(
        refused(&shape, face(1), Some(&empty), face(1)),
        missing("second", face(1), 0)
    );
    assert_eq!(
        refused(&empty, vertex(1), Some(&shape), face(1)),
        missing("first", vertex(1), 0)
    );
    assert_eq!(
        refused(&empty, edge(1), None, edge(1)),
        missing("first", edge(1), 0)
    );
}

// ---- Each kind against each kind, by hand ----

#[test]
fn each_kind_against_each_kind_is_measured_between_the_bounded_elements() {
    // A 10 mm cube A at the origin and B, A moved 20 along x (B's Face1 is x = 20, its
    // Edge1 (20, 0, 0) to (20, 0, 10), its Vertex2 (20, 0, 0)).
    let a = cube(10.0);
    let b = moved(&a, [20.0, 0.0, 0.0]);
    // A's Vertex5, (10, 0, 10), to B's Face1: 10, straight across.
    let d = measured("vertex to face", &a, vertex(5), Some(&b), face(1));
    assert_eq!(d.distance, 10.0);
    assert_eq!((d.from, d.to), ([10.0, 0.0, 10.0], [20.0, 0.0, 10.0]));
    // A's Edge5, (10, 0, 0) to (10, 0, 10), to B's Vertex2, (20, 0, 0): 10, from its end.
    let d = measured("edge to vertex", &a, edge(5), Some(&b), vertex(2));
    assert_eq!(d.distance, 10.0);
    assert_eq!((d.from, d.to), ([10.0, 0.0, 0.0], [20.0, 0.0, 0.0]));
    // A's Face2 (x = 10) to B's Edge1, parallel 10 away: met all along it; the witness is
    // the pair first by coordinates, at its lower end.
    let d = measured("face to edge", &a, face(2), Some(&b), edge(1));
    assert_eq!(d.distance, 10.0);
    assert_eq!((d.from, d.to), ([10.0, 0.0, 0.0], [20.0, 0.0, 0.0]));
    // A's Edge1 (x = y = 0, z from 0 to 10) and C's Edge9 (C is A moved (5, 5, 0): y = 5,
    // z = 0, x from 5 to 15): their lines are 5 apart, at (0, 0, 0) and (0, 5, 0), but
    // (0, 5, 0) isn't on the edge, which starts at x = 5: bounded, √50 = 7.0710678118654755,
    // from (0, 0, 0) to the edge's start (5, 5, 0).
    let c = moved(&a, [5.0, 5.0, 0.0]);
    let d = measured("skew edges", &a, edge(1), Some(&c), edge(9));
    assert_near("skew edges", d.distance, 50f64.sqrt(), 1e-12);
    assert_eq!((d.from, d.to), ([0.0, 0.0, 0.0], [5.0, 5.0, 0.0]));
}

#[test]
fn crossing_touching_and_overlapping_elements_are_0_apart_where_they_meet() {
    let a = cube(10.0);
    // A's top (z = 10) and D's Face1 (D is A moved (5, 5, 5): x = 5, y and z from 5 to 15)
    // cross along x = 5, z = 10, y from 5 to 10; the first point of it is (5, 5, 10).
    let d = moved(&a, [5.0, 5.0, 5.0]);
    let crossing = measured("crossing faces", &a, face(6), Some(&d), face(1));
    assert_near("crossing faces", crossing.distance, 0.0, 1e-12);
    assert_point("crossing faces", crossing.from, [5.0, 5.0, 10.0], 1e-9);
    assert_point("crossing faces", crossing.to, [5.0, 5.0, 10.0], 1e-9);
    // Cubes corner to corner at (10, 10, 10): A's top and E's bottom touch at that point
    // only, and so do the vertices there (A's Vertex7, E's Vertex2) and the edges (A's
    // Edge12, y = z = 10; E's Edge4, x = 10, z = 10).
    let e = moved(&a, [10.0, 10.0, 10.0]);
    for (first, second) in [
        (face(6), face(5)),
        (vertex(7), vertex(2)),
        (edge(12), edge(4)),
    ] {
        let touch = measured("corner to corner", &a, first, Some(&e), second);
        assert_eq!(touch.distance, 0.0, "{first} {second}");
        assert_eq!(touch.from, [10.0, 10.0, 10.0], "{first} {second}");
        assert_eq!(touch.to, [10.0, 10.0, 10.0], "{first} {second}");
    }
    // Two bottoms overlapping in a 5 × 5 square, in one plane: 0, first met at (5, 5, 0),
    // the overlap's corner nearest the origin.
    let side = moved(&a, [5.0, 5.0, 0.0]);
    let overlap = measured("overlapping faces", &a, face(5), Some(&side), face(5));
    assert_eq!(overlap.distance, 0.0);
    assert_eq!(
        (overlap.from, overlap.to),
        ([5.0, 5.0, 0.0], [5.0, 5.0, 0.0])
    );
    // An edge, a vertex, and a seam each 0 from a face it bounds.
    let in_face = measured("edge in its face", &a, edge(1), None, face(1));
    assert_eq!(in_face.distance, 0.0);
    assert_eq!(in_face.from, in_face.to);
    let corner = measured("vertex in its face", &a, vertex(1), None, face(1));
    assert_eq!(corner.distance, 0.0);
    assert_eq!(
        (corner.from, corner.to),
        ([0.0, 0.0, 10.0], [0.0, 0.0, 10.0])
    );
    let cyl = cylinder(2.0, 10.0);
    let seam = measured("seam in its face", &cyl, edge(1), None, face(1));
    assert_eq!(seam.distance, 0.0);
    assert_point("seam in its face", seam.from, [2.0, 0.0, 0.0], 1e-12);
}

#[test]
fn every_element_is_0_from_itself_at_one_point() {
    // The same element twice, each of every kind of a box, a sphere (its poles are
    // degenerated edges), a cylinder (its seam), a pointed cone (its apex), and a torus.
    let shapes = [
        ("box", cube(10.0), (6, 12, 8)),
        ("sphere", sphere(5.0), (1, 3, 2)),
        ("cylinder", cylinder(2.0, 10.0), (3, 3, 2)),
        (
            "cone",
            make(&Primitive::Cone {
                radius1: 5.0,
                radius2: 0.0,
                height: 10.0,
            })
            .unwrap(),
            (2, 3, 2),
        ),
        ("torus", torus(), (1, 2, 1)),
    ];
    for (name, shape, (faces, edges, vertices)) in &shapes {
        for (kind, count) in [
            (ElementKind::Face, faces),
            (ElementKind::Edge, edges),
            (ElementKind::Vertex, vertices),
        ] {
            for index in 1..=*count {
                let element = ElementRef { kind, index };
                let what = format!("{name} {element}");
                let d = measured(&what, shape, element, None, element);
                assert!(d.distance.abs() <= 1e-12, "{what}: {d:?}");
                assert_point(&what, d.from, d.to, 1e-12);
            }
        }
    }
}

// ---- Degenerated edges and singular points ----

#[test]
fn a_spheres_poles_and_seam_are_measured_as_what_they_are() {
    // A sphere of radius 5: Edge1 is its north pole (0, 0, 5), a degenerated edge, Edge3
    // its south pole (0, 0, -5), Edge2 its seam, the half circle in y = 0 with x ≥ 0. A
    // 10 mm cube moved 20 along x: its Face1 is x = 20 (y, z from 0 to 10), its Vertex1
    // (20, 0, 10).
    let ball = sphere(5.0);
    let wall = moved(&cube(10.0), [20.0, 0.0, 0.0]);
    // The north pole to the face: 20, to (20, 0, 5).
    let d = measured("north pole to face", &ball, edge(1), Some(&wall), face(1));
    assert_near("north pole to face", d.distance, 20.0, 1e-12);
    assert_point("north pole", d.from, [0.0, 0.0, 5.0], 1e-12);
    assert_point("north pole", d.to, [20.0, 0.0, 5.0], 1e-12);
    // The north pole to (20, 0, 10): √(400 + 25) = √425; the south pole: √(400 + 225) = 25.
    let d = measured(
        "north pole to vertex",
        &ball,
        edge(1),
        Some(&wall),
        vertex(1),
    );
    assert_near("north pole to vertex", d.distance, 425f64.sqrt(), 1e-12);
    let d = measured(
        "south pole to vertex",
        &ball,
        edge(3),
        Some(&wall),
        vertex(1),
    );
    assert_near("south pole to vertex", d.distance, 25.0, 1e-12);
    // The seam's nearest point to x = 20 is its middle, (5, 0, 0): 15.
    let d = measured("seam to face", &ball, edge(2), Some(&wall), face(1));
    assert_near("seam to face", d.distance, 15.0, 1e-12);
    assert_point("seam to face", d.from, [5.0, 0.0, 0.0], 1e-9);
    assert_point("seam to face", d.to, [20.0, 0.0, 0.0], 1e-9);
}

#[test]
fn a_point_above_a_cones_apex_is_its_height_above_it() {
    // A cone of radius 5 at z = 0 to a point at z = 10 (its Edge1, degenerated), and a
    // point on its axis at (0, 0, 20): the apex is nearest, 10 below, from both the cone's
    // side (its slope is 1:2, so no point of the side is nearer) and its apex edge.
    let cone = make(&Primitive::Cone {
        radius1: 5.0,
        radius2: 0.0,
        height: 10.0,
    })
    .unwrap();
    let above = moved(&cube(10.0), [0.0, 0.0, 20.0]);
    for element in [face(1), edge(1)] {
        let d = measured("above the apex", &above, vertex(2), Some(&cone), element);
        assert_near("above the apex", d.distance, 10.0, 1e-12);
        assert_point("above the apex", d.to, [0.0, 0.0, 10.0], 1e-12);
    }
}

#[test]
fn a_point_every_direction_from_a_surface_is_its_radius_away() {
    // A point every point of a surface or curve is equally far from (a sphere's, a
    // circle's, a torus's centre; a point on a cylinder's axis): OCCT's extrema there are
    // infinite in number; the distance is the radius, met at a point of the element.
    let origin = cube(10.0); // its Vertex2 is (0, 0, 0)
    let ball = sphere(5.0);
    let d = measured("sphere's centre", &origin, vertex(2), Some(&ball), face(1));
    assert_near("sphere's centre", d.distance, 5.0, 1e-12);
    assert_near("sphere's centre: on it", length(d.to), 5.0, 1e-12);
    // A torus of radii 10 and 2: its tube's inner circle is 8 from its centre, as its face
    // and its seam circle (Edge1, in y = 0) are.
    for element in [face(1), edge(1)] {
        let d = measured(
            "torus's centre",
            &origin,
            vertex(2),
            Some(&torus()),
            element,
        );
        assert_near("torus's centre", d.distance, 8.0, 1e-12);
        assert_near("torus's centre: its inner circle", length(d.to), 8.0, 1e-12);
    }
    // A cylinder of radius 2: its bottom rim (Edge2) from the rim's centre, its side from a
    // point on its axis (0, 0, 5): 2, at the point's height.
    let cyl = cylinder(2.0, 10.0);
    let d = measured("rim's centre", &origin, vertex(2), Some(&cyl), edge(2));
    assert_near("rim's centre", d.distance, 2.0, 1e-12);
    let on_axis = moved(&origin, [0.0, 0.0, 5.0]);
    let d = measured("cylinder's axis", &on_axis, vertex(2), Some(&cyl), face(1));
    assert_near("cylinder's axis", d.distance, 2.0, 1e-12);
    assert_near("cylinder's axis: at its height", d.to[2], 5.0, 1e-12);
}

// ---- Curved pairs ----

#[test]
fn curved_pairs_are_their_closed_forms_apart() {
    let ball = sphere(5.0);
    // Spheres of radius 5 with centres √178 apart ((12, 5, -3)): √178 − 10, met on the
    // line of centres, 5 from each.
    let c = [12.0, 5.0, -3.0];
    let gap = 178f64.sqrt();
    let skew = measured(
        "skew spheres",
        &ball,
        face(1),
        Some(&moved(&ball, c)),
        face(1),
    );
    assert_near("skew spheres", skew.distance, gap - 10.0, 1e-9);
    let unit = c.map(|x| x / gap);
    assert_point("skew spheres", skew.from, unit.map(|u| 5.0 * u), 1e-6);
    assert_point(
        "skew spheres",
        skew.to,
        [0, 1, 2].map(|i| c[i] - 5.0 * unit[i]),
        1e-6,
    );
    // 20 apart along x: 10, met at (5, 0, 0) and (15, 0, 0). OCCT's surface extrema land
    // 5e-5 off that line (its witness is (5, 5e-5, 5e-5), 8.7e-10 longer than its
    // distance): the distance is flat to second order there, so a point 5e-5 off is still
    // within 1e-9 of it.
    let along = measured(
        "spheres along x",
        &ball,
        face(1),
        Some(&moved(&ball, [20.0, 0.0, 0.0])),
        face(1),
    );
    assert_near("spheres along x", along.distance, 10.0, 1e-9);
    assert_point("spheres along x", along.from, [5.0, 0.0, 0.0], 1e-4);
    assert_point("spheres along x", along.to, [15.0, 0.0, 0.0], 1e-4);
    // Concentric spheres of radii 5 and 10: 5, met everywhere.
    let d = measured(
        "concentric spheres",
        &ball,
        face(1),
        Some(&sphere(10.0)),
        face(1),
    );
    assert_near("concentric spheres", d.distance, 5.0, 1e-12);
    assert_near("concentric: on the inner", length(d.from), 5.0, 1e-12);
    assert_near("concentric: on the outer", length(d.to), 10.0, 1e-12);
    // Coaxial cylinders of radii 2 and 5, both 10 high: 3, met along every radius.
    let d = measured(
        "coaxial cylinders",
        &cylinder(2.0, 10.0),
        face(1),
        Some(&cylinder(5.0, 10.0)),
        face(1),
    );
    assert_near("coaxial cylinders", d.distance, 3.0, 1e-12);
    let radius = |p: [f64; 3]| libm::hypot(p[0], p[1]);
    assert_near("coaxial: on the inner", radius(d.from), 2.0, 1e-12);
    assert_near("coaxial: on the outer", radius(d.to), 5.0, 1e-12);
    assert_near("coaxial: at one height", d.from[2], d.to[2], 1e-12);
    // Parallel cylinders of radius 2, axes 10 apart along x: 6, from x = 2 to x = 8.
    let cyl = cylinder(2.0, 10.0);
    let d = measured(
        "parallel cylinders",
        &cyl,
        face(1),
        Some(&moved(&cyl, [10.0, 0.0, 0.0])),
        face(1),
    );
    assert_near("parallel cylinders", d.distance, 6.0, 1e-12);
    assert_near("parallel cylinders: from x = 2", d.from[0], 2.0, 1e-9);
    assert_near("parallel cylinders: to x = 8", d.to[0], 8.0, 1e-9);
    assert_near("parallel cylinders: in y = 0", d.from[1], 0.0, 1e-9);
    // A torus of radii 10 and 2 under a plane at z = 5 (a slab's bottom): 3, met all
    // around the tube's top circle (radius 10 at z = 2).
    let slab = moved(
        &make(&Primitive::Box {
            dx: 40.0,
            dy: 40.0,
            dz: 1.0,
        })
        .unwrap(),
        [-20.0, -20.0, 5.0],
    );
    let d = measured(
        "torus under a plane",
        &torus(),
        face(1),
        Some(&slab),
        face(5),
    );
    assert_near("torus under a plane", d.distance, 3.0, 1e-9);
    assert_near("torus: its top", d.from[2], 2.0, 1e-9);
    assert_near("torus: its top circle", radius(d.from), 10.0, 1e-6);
    // A sphere of radius 5 in the torus's hole: the tube's inner circle is radius 8, 3 off;
    // one of radius 8 touches the tube all around it: 0.
    let d = measured(
        "sphere in the hole",
        &ball,
        face(1),
        Some(&torus()),
        face(1),
    );
    assert_near("sphere in the hole", d.distance, 3.0, 1e-9);
    let d = measured(
        "sphere in the ring",
        &sphere(8.0),
        face(1),
        Some(&torus()),
        face(1),
    );
    assert_near("sphere in the ring", d.distance, 0.0, 1e-9);
    // A sphere of radius 5 at (20, 0, 5) beside the cylinder of radius 2: 20 − 5 − 2 = 13.
    let d = measured(
        "sphere beside a cylinder",
        &moved(&ball, [20.0, 0.0, 5.0]),
        face(1),
        Some(&cyl),
        face(1),
    );
    assert_near("sphere beside a cylinder", d.distance, 13.0, 1e-9);
    assert_point("sphere beside a cylinder", d.from, [15.0, 0.0, 5.0], 1e-6);
    assert_point("sphere beside a cylinder", d.to, [2.0, 0.0, 5.0], 1e-6);
    // Two such tori, the second moved (30, 0, 1): their tubes' nearest circles are centred
    // at (10, 0, 0) and (20, 0, 1), √101 apart, so √101 − 4.
    let d = measured(
        "tori side by side",
        &torus(),
        face(1),
        Some(&moved(&torus(), [30.0, 0.0, 1.0])),
        face(1),
    );
    assert_near("tori side by side", d.distance, 101f64.sqrt() - 4.0, 1e-9);
}

// ---- Scale ----

#[test]
fn parts_far_from_the_origin_tiny_or_a_nanometre_apart_are_measured_exactly() {
    // Two 10 mm cubes a kilometre out on every axis, 0.1 apart: the first's top at
    // (1e6 − 20.1) + 10, the second's bottom at 1e6 − 10; at 1e6 an ulp is 1.2e-10.
    let a = moved(&cube(10.0), [1e6 - 10.0, 1e6 - 10.0, 1e6 - 20.1]);
    let b = moved(&cube(10.0), [1e6 - 10.0, 1e6 - 10.0, 1e6 - 10.0]);
    let d = measured("a kilometre out", &a, face(6), Some(&b), face(5));
    assert_near("a kilometre out", d.distance, 0.1, 1e-9);
    assert_point(
        "a kilometre out",
        d.from,
        [999_990.0, 999_990.0, 999_989.9],
        1e-9,
    );
    assert_point(
        "a kilometre out",
        d.to,
        [999_990.0, 999_990.0, 999_990.0],
        1e-9,
    );
    // A cube a micrometre on a side: its bottom and top 1e-6 apart, from its first corner.
    let tiny = cube(1e-6);
    let d = measured("a micrometre cube", &tiny, face(5), None, face(6));
    assert_eq!(d.distance, 1e-6);
    assert_eq!((d.from, d.to), ([0.0, 0.0, 0.0], [0.0, 0.0, 1e-6]));
    // Two 10 mm cubes a nanometre (1e-6 mm) apart, and 5e-8 apart, under OCCT's
    // confusion tolerance: each the gap as placed, (10 + g) − 10, never snapped to 0.
    for gap in [1e-6, 5e-8] {
        let above = moved(&cube(10.0), [0.0, 0.0, 10.0 + gap]);
        let d = measured("a hair apart", &cube(10.0), face(6), Some(&above), face(5));
        let placed = (10.0 + gap) - 10.0;
        assert!(d.distance > 0.0, "{gap}: {d:?}");
        assert_near(&format!("{gap} apart"), d.distance, placed, 1e-15);
        assert_eq!(d.from, [0.0, 0.0, 10.0], "{gap}");
    }
}

// ---- The same request again, and swapped ----

fn data(shape: &Shape) -> ShapeData {
    ShapeData::checked(Blob::inline(to_brep(shape).unwrap()), true)
}

#[test]
fn the_same_request_gives_the_same_bits_here_and_in_fresh_workers() {
    // Pairs OCCT measures by iterating (two spheres, two conics of offsets_and_conics.brep,
    // two faces of NIST's CTC 01): the same bits in this process, and in each of two fresh
    // worker processes.
    let ball = sphere(5.0);
    let skew = moved(&ball, [12.0, 5.0, -3.0]);
    let conics = from_text_brep(&fixture("offsets_and_conics.brep")).unwrap();
    let part = from_text_brep(&fixture("nist_ctc_01_asme1_ap242-e1.brep")).unwrap();
    let cases: [(&str, &Shape, ElementRef, Option<&Shape>, ElementRef); 3] = [
        ("spheres", &ball, face(1), Some(&skew), face(1)),
        ("conics", &conics, edge(5), None, edge(6)),
        ("CTC 01", &part, face(25), None, face(39)),
    ];
    let bits = |d: &Distance| {
        (
            d.distance.to_bits(),
            d.from.map(f64::to_bits),
            d.to.map(f64::to_bits),
            d.solutions,
        )
    };
    let mut workers = [kernel::kernel(), kernel::kernel()];
    for (what, shape, first, other, second) in cases {
        let here = distance(shape, first, other, second).unwrap();
        let again = distance(shape, first, other, second).unwrap();
        assert_eq!(bits(&again), bits(&here), "{what}: again");
        for worker in &mut workers {
            let request =
                KernelRequest::distance(&data(shape), first, other.map(data).as_ref(), second)
                    .unwrap();
            let through = worker.run(request).unwrap();
            assert_eq!(bits(&through), bits(&here), "{what}: through a worker");
        }
    }
}

#[test]
fn swapping_the_elements_swaps_the_witness_and_nothing_else() {
    // docs/ports/measure.md and `witness`'s doc: "swapping the elements swaps its points
    // and nothing else". Each pair below, measured with its elements one way and the other.
    //
    // OCCT's BRepExtrema_DistShapeShape isn't symmetric in its two shapes where it iterates
    // (surface and curve extrema): given them the other way round, two skew spheres'
    // witness moved by 3e-8; two conics of offsets_and_conics.brep (Edge5, Edge6) gave
    // distances an ulp apart and 2 solutions one way, 1 the other; and two faces of NIST's
    // CTC 01 (Face25, Face39, 507.5 apart) gave witnesses 47 mm apart, as each way found
    // other pairs. So `distance` gives OCCT the elements in one order, whatever order they
    // came in (by their shapes' B-reps, then kind and number), and swaps its answer back.
    let ball = sphere(5.0);
    let skew = moved(&ball, [12.0, 5.0, -3.0]);
    let conics = from_text_brep(&fixture("offsets_and_conics.brep")).unwrap();
    let part = from_text_brep(&fixture("nist_ctc_01_asme1_ap242-e1.brep")).unwrap();
    let cube_a = cube(10.0);
    let cube_b = moved(&cube_a, [3.0, 4.0, 20.0]);
    let tor = torus();
    let cyl = cylinder(2.0, 10.0);
    let wide = cylinder(5.0, 10.0);
    // (what, shape, first, other, second); without another shape, both are the first's.
    let cases: [(&str, &Shape, ElementRef, Option<&Shape>, ElementRef); 7] = [
        ("boxes", &cube_a, face(6), Some(&cube_b), face(5)),
        ("coaxial cylinders", &cyl, face(1), Some(&wide), face(1)),
        ("sphere in a torus", &ball, face(1), Some(&tor), face(1)),
        ("skew spheres", &ball, face(1), Some(&skew), face(1)),
        ("conics", &conics, edge(5), None, edge(6)),
        ("CTC 01 faces 11 and 95", &part, face(11), None, face(95)),
        ("CTC 01 faces 25 and 39", &part, face(25), None, face(39)),
    ];
    let mut differ = Vec::new();
    for (what, shape, first, other, second) in cases {
        let one = distance(shape, first, other, second).unwrap();
        let (back_shape, back_other) = match other {
            Some(other) => (other, Some(shape)),
            None => (shape, None),
        };
        let other_way = distance(back_shape, second, back_other, first).unwrap();
        let swapped = Distance {
            distance: other_way.distance,
            from: other_way.to,
            to: other_way.from,
            solutions: other_way.solutions,
        };
        if swapped != one || swapped.distance.to_bits() != one.distance.to_bits() {
            differ.push(format!("{what}:\n  {one:?}\n  {swapped:?} (swapped back)"));
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
}

// ---- Garbage to a worker, and the fake against it ----

#[test]
fn a_worker_asked_for_a_distance_on_garbage_says_so_and_goes_on() {
    // Bytes that aren't a B-rep, none, and a B-rep cut short, as the shape and as the other
    // shape: each a structured refusal, and the worker then measures a box as it should.
    let mut worker = kernel::kernel();
    let good = data(&cube(10.0));
    let mut cut_short = good.brep().as_bytes().to_vec();
    cut_short.truncate(cut_short.len() / 2);
    for (what, bytes) in [
        ("garbage", b"not a brep at all".to_vec()),
        ("nothing", Vec::new()),
        ("cut short", cut_short),
    ] {
        let bad = ShapeData::unchecked(Blob::inline(bytes));
        for (place, shape, other) in [("shape", &bad, &good), ("other", &good, &bad)] {
            let request = KernelRequest::distance(shape, face(1), Some(other), face(2)).unwrap();
            let err = worker.run(request).unwrap_err();
            assert!(
                matches!(
                    err.code(),
                    ErrorCode::InvalidInput | ErrorCode::KernelFailed
                ),
                "{what} as the {place}: {err} ({:?})",
                err.code()
            );
            let request = KernelRequest::distance(&good, face(5), None, face(6)).unwrap();
            assert_eq!(worker.run(request).unwrap().distance, 10.0, "{what}");
        }
    }
    // An empty compound as the other shape: the worker names `second`, and goes on.
    let empty = data(&test_ops::empty_shape().unwrap());
    let request = KernelRequest::distance(&good, face(1), Some(&empty), face(1)).unwrap();
    let err = worker.run(request).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput);
    assert_eq!(err.parameter(), Some("second"));
    assert_eq!(
        err.message(),
        "second, face 1, doesn't exist: its shape has no faces"
    );
    // (0, 0, 0) to (10, 10, 0): √200.
    let request = KernelRequest::distance(&good, vertex(2), None, vertex(8)).unwrap();
    assert_near(
        "after the empty compound",
        worker.run(request).unwrap().distance,
        200f64.sqrt(),
        1e-12,
    );
}

/// `request`'s result on the fake kernel, within 5 s.
fn on_fake<T: Send + 'static>(
    fake: &FakeKernel,
    request: KernelRequest<T>,
) -> Result<T, silico_core::Error> {
    match fake
        .submit(fake.generation(), request)
        .wait_timeout(Duration::from_secs(5))
    {
        Ok(Outcome::Done(result)) => result,
        Ok(Outcome::Superseded) => panic!("superseded"),
        Err(_) => panic!("no outcome within 5 s (the fake hung)"),
    }
}

#[test]
fn the_fakes_refusals_read_as_a_workers() {
    // The same element a box hasn't, asked of the fake kernel and of a worker: the same
    // code, parameter, message, cause, suggestion, and elements, so a core test on the fake
    // sees what an agent would.
    //
    // The worker client words a worker's InvalidParameter refusal with the suggestion
    // "change the value the message names" (`worker::failure_error`); the fake's distance
    // builds its refusal as the worker's `Failure` and words it with `failure_error`, as
    // `FakeKernel::refuse_as_worker` does, so the two are one.
    let fake = FakeKernel::new(1).unwrap();
    let block = Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    };
    let fake_box = on_fake(&fake, KernelRequest::make(block).unwrap()).unwrap();
    let mut worker = kernel::kernel();
    let real_box = worker.make(block).unwrap();
    let parts = |e: &silico_core::Error| {
        (
            e.code(),
            e.parameter().map(str::to_owned),
            e.message().to_owned(),
            e.cause().map(str::to_owned),
            e.suggestion().map(str::to_owned),
            e.elements().to_vec(),
        )
    };
    for (first, second, other) in [
        (face(1), face(7), false),
        (edge(u32::MAX), edge(1), false),
        (vertex(1), vertex(9), true),
    ] {
        let ask = |shape: &ShapeData| {
            KernelRequest::distance(shape, first, other.then_some(shape), second).unwrap()
        };
        let from_fake = on_fake(&fake, ask(&fake_box)).unwrap_err();
        let from_worker = worker.run(ask(&real_box)).unwrap_err();
        assert_eq!(parts(&from_fake), parts(&from_worker), "{first} {second}");
    }
    // A shape with one element of the kind: a sphere's one face, its second asked for.
    let ball = Primitive::Sphere { radius: 1.0 };
    let fake_ball = on_fake(&fake, KernelRequest::make(ball).unwrap()).unwrap();
    let real_ball = worker.make(ball).unwrap();
    let from_fake = on_fake(
        &fake,
        KernelRequest::distance(&fake_box, face(1), Some(&fake_ball), face(2)).unwrap(),
    )
    .unwrap_err();
    let from_worker = worker
        .run(KernelRequest::distance(&real_box, face(1), Some(&real_ball), face(2)).unwrap())
        .unwrap_err();
    assert_eq!(parts(&from_fake), parts(&from_worker));
    assert_eq!(
        from_worker.message(),
        "second, face 2, doesn't exist: its shape has faces 1 to 1"
    );
}

// ---- The witness's order ----

/// A coordinate from values where an order goes wrong: both zeros, ±1, the smallest
/// subnormal, a tiny normal, a kilometre, the infinities, and NaN.
fn coordinate() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(1.0),
        Just(-1.0),
        Just(5e-324),
        Just(1e-300),
        Just(1e6),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
        Just(f64::NAN),
        -2.0..2.0f64,
    ]
}

fn point() -> impl Strategy<Value = [f64; 3]> {
    [coordinate(), coordinate(), coordinate()]
}

type Pair = [[f64; 3]; 2];

fn pair_bits(pair: &Pair) -> [[u64; 3]; 2] {
    pair.map(|p| p.map(f64::to_bits))
}

/// The witness by its definition, written apart from `witness`: each pair's two points
/// sorted by (x, y, z) under `total_cmp`'s order, the pairs sorted by those, the first
/// taken.
fn first_by_definition(pairs: &[Pair]) -> Option<Pair> {
    // total_cmp's order as an integer one: every bit of a negative flipped, and the sign
    // bit of a positive set.
    let key = |p: &[f64; 3]| {
        p.map(|c| {
            let b = c.to_bits();
            if b >> 63 == 1 { !b } else { b | (1 << 63) }
        })
    };
    let mut keyed: Vec<_> = pairs
        .iter()
        .map(|pair| {
            let (a, b) = (key(&pair[0]), key(&pair[1]));
            (if a <= b { (a, b) } else { (b, a) }, *pair)
        })
        .collect();
    keyed.sort_by_key(|x| x.0);
    keyed.first().map(|(_, pair)| *pair)
}

/// Whether two pairs are the same two points the other way round (and not one point
/// twice): no choice between them can both ignore the pairs' order and swap with the
/// elements, so the property below leaves them out.
fn reversed(a: &Pair, b: &Pair) -> bool {
    pair_bits(a) == pair_bits(&[b[1], b[0]]) && pair_bits(a) != pair_bits(b)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn the_witness_is_the_first_pair_whatever_their_order_and_swaps_with_them(
        (pairs, shuffled) in prop::collection::vec([point(), point()], 1..8)
            .prop_flat_map(|pairs| (Just(pairs.clone()), Just(pairs).prop_shuffle()))
    ) {
        for (i, a) in pairs.iter().enumerate() {
            for b in &pairs[i + 1..] {
                prop_assume!(!reversed(a, b));
            }
        }
        let chosen = witness(&pairs).unwrap();
        let want = first_by_definition(&pairs).unwrap();
        prop_assert_eq!(pair_bits(&chosen), pair_bits(&want));
        // Whatever order the pairs come in.
        prop_assert_eq!(pair_bits(&witness(&shuffled).unwrap()), pair_bits(&chosen));
        // Each pair swapped: the witness swapped.
        let swapped: Vec<Pair> = pairs.iter().map(|[a, b]| [*b, *a]).collect();
        prop_assert_eq!(
            pair_bits(&witness(&swapped).unwrap()),
            pair_bits(&[chosen[1], chosen[0]])
        );
    }
}

/// A vertex at (`x`, 0, 0), as FreeCAD writes one (`Part.Vertex(x, 0, 0)`).
fn vertex_at(x: &str) -> Shape {
    let text = format!(
        "\nCASCADE Topology V1, (c) Matra-Datavision\nLocations 0\nCurve2ds 0\nCurves 0\n\
         Polygon3D 0\nPolygonOnTriangulations 0\nSurfaces 0\nTriangulations 0\n\nTShapes 1\n\
         Ve\n1e-07\n{x} 0 0\n0 0\n\n1101101\n*\n\n+1 0 \n"
    );
    from_text_brep(text.as_bytes()).unwrap()
}

#[test]
fn a_distance_past_what_a_number_holds_is_not_done_and_the_worker_lives_on() {
    // Two vertices at x = 1e308 and x = -1e308, each a finite point: 2e308 apart, which no
    // f64 holds. OCCT keeps only solutions less than its running least distance, which
    // starts at the largest finite number, so it keeps none and isn't done: an infinite
    // distance is never sent, and the request is refused as OCCT's (kernel_failed), with
    // its cause, never a worker's lie (protocol_error).
    let (far, other) = (vertex_at("1e+308"), vertex_at("-1e+308"));
    let here = distance(&far, vertex(1), Some(&other), vertex(1)).unwrap_err();
    assert_eq!(here.refusal(), &Refusal::DistanceNotDone);
    let mut worker = kernel::kernel();
    let request =
        KernelRequest::distance(&data(&far), vertex(1), Some(&data(&other)), vertex(1)).unwrap();
    let error = worker.run(request).unwrap_err();
    assert_eq!(error.code(), ErrorCode::KernelFailed, "{error}");
    assert_eq!(error.message(), "Distance failed");
    assert_eq!(error.cause(), Some("BRepExtrema_DistShapeShape: not done"));
    assert_eq!(
        error.suggestion(),
        Some("report it with the document: the kernel couldn't measure this pair")
    );
    // The worker lives on.
    let (three, seven) = (vertex_at("3"), vertex_at("7"));
    let request =
        KernelRequest::distance(&data(&three), vertex(1), Some(&data(&seven)), vertex(1)).unwrap();
    assert_eq!(worker.run(request).unwrap().distance, 4.0);
}
