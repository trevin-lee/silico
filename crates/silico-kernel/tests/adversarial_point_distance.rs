//! break-it: hostile and degenerate inputs for a point's least distance from an element
//! (`silico_kernel::point_distance`, `Op::PointDistance`; `distance`'s `center_to_element`,
//! as FreeCAD's QuickMeasure measures a circle's centre against a face or an edge): points
//! at and far past the origin's bound, on the element, on its boundary, on its line past its
//! end, in a face's hole and a nanometre-scale step either side of the hole's rim, every
//! direction from a surface or a circle (OCCT's extrema infinite in number), a sphere's and a
//! cone's degenerated edges, elements numbered past the shape and of an empty compound,
//! garbage shapes, the fake against a worker, the same request in fresh workers, and a
//! property test over a cube's every element against the closed form. Every expected value
//! is by hand, named where it's used.

use super::kernel;

use std::path::Path;
use std::time::Duration;

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Kernel, KernelRequest};
use silico_core::pool::Outcome;
use silico_core::worker::Worker;
use silico_kernel::{
    Primitive, Shape, boolean, describe_whole, from_text_brep, make, point_distance, test_ops,
    to_brep, translate,
};
use silico_protocol::{
    Blob, BooleanOp, Distance, ElementKind, ElementRef, Generation, MAX_POINT_COORDINATE, Op,
    Reply, ShapeData, Surface,
};

fn cube(side: f64) -> Shape {
    make(&Primitive::Box {
        dx: side,
        dy: side,
        dz: side,
    })
    .unwrap()
}

fn data(shape: &Shape) -> ShapeData {
    ShapeData::checked(Blob::inline(to_brep(shape).unwrap()), true)
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

fn apart(p: [f64; 3], q: [f64; 3]) -> f64 {
    let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    libm::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
}

fn assert_near(what: &str, got: f64, want: f64, tolerance: f64) {
    assert!(
        (got - want).abs() <= tolerance,
        "{what}: got {got:e}, want {want:e} (off {:e})",
        (got - want).abs()
    );
}

/// The worker's answer to a point distance, within 30 s, its witness checked: `from` the
/// point itself, and `to` as far from it as the distance, to OCCT's precision.
fn through(worker: &mut Worker, shape: &ShapeData, element: ElementRef, p: [f64; 3]) -> Distance {
    match worker
        .call(
            Generation(1),
            Op::PointDistance {
                shape: shape.clone(),
                element,
                point: p,
            },
            Duration::from_secs(30),
        )
        .unwrap_or_else(|e| panic!("{element} from {p:?}: {e} ({:?})", e.code()))
    {
        Reply::Distance(d) => {
            assert_eq!(d.from, p, "{element} from {p:?}: from");
            let scale = 1.0 + p.iter().fold(0.0f64, |m, c| m.max(c.abs()));
            assert_near(
                &format!("{element} from {p:?}: its witness against its distance"),
                apart(d.from, d.to),
                d.distance,
                1e-7 + 1e-14 * scale,
            );
            assert!(d.solutions >= 1, "{element} from {p:?}: {d:?}");
            d
        }
        other => panic!("{element} from {p:?}: replied {other:?}"),
    }
}

fn worker() -> Worker {
    Worker::spawn(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap()
}

/// M2.1's plate: a 40 × 20 × 10 block with a hole of radius 2.5 through it at (20, 10).
fn plate() -> Shape {
    let block = make(&Primitive::Box {
        dx: 40.0,
        dy: 20.0,
        dz: 10.0,
    })
    .unwrap();
    let pin = make(&Primitive::Cylinder {
        radius: 2.5,
        height: 12.0,
    })
    .unwrap();
    let pin = translate(&pin, [20.0, 10.0, -1.0]).unwrap().shape;
    boolean(BooleanOp::Cut, &block, &pin).unwrap().shape
}

/// The number of `shape`'s planar face whose outward normal is `normal` and whose centre of
/// area has `z` as its height.
fn planar_face(shape: &Shape, normal: [f64; 3], z: f64) -> ElementRef {
    let d = describe_whole(shape).unwrap();
    let found: Vec<usize> = d
        .faces
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            matches!(f.surface, Surface::Plane { normal: n } if n == normal)
                && (f.centroid[2] - z).abs() < 1e-9
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(found.len(), 1, "{normal:?} at z = {z}: {found:?}");
    face(u32::try_from(found[0] + 1).unwrap())
}

// ---- Far out, at the bound ----

#[test]
fn a_point_at_the_bound_is_its_distance_from_a_face_an_edge_and_a_vertex_through_a_worker() {
    // A 10 mm cube: Face1 x = 0, Face2 x = 10. From (−1e12, 5, 5), Face1 is 1e12 away, its
    // foot (0, 5, 5) inside it; Face2 1e12 + 10. From (1e12, 1e12, 1e12), the corner
    // (10, 10, 10) is √3 · (1e12 − 10) away. The edge along x from the origin to (10, 0, 0),
    // from (1e12, 5, 5): its end (10, 0, 0), √((1e12 − 10)² + 50). Each with traps on, as a
    // worker runs, to a relative 1e-12 (a double's spacing at 1e12 is 1.2e-4).
    let mut w = worker();
    let shape = cube(10.0);
    let d = data(&shape);
    let most = MAX_POINT_COORDINATE;
    let near = |what: &str, got: f64, want: f64| assert_near(what, got, want, want * 1e-12);
    // (Its witness may be any point of the face within OCCT's precision of the distance,
    // which, 1e12 out, is anywhere on it: a witness, not an identity.)
    let x0 = through(&mut w, &d, face(1), [-most, 5.0, 5.0]);
    near("x = 0", x0.distance, 1e12);
    assert_near("x = 0: on it", x0.to[0], 0.0, 1e-9);
    near(
        "x = 10",
        through(&mut w, &d, face(2), [-most, 5.0, 5.0]).distance,
        1e12 + 10.0,
    );
    let corner = d_vertex(&shape, [10.0, 10.0, 10.0]);
    near(
        "corner",
        through(&mut w, &d, corner, [most; 3]).distance,
        3f64.sqrt() * (1e12 - 10.0),
    );
    let along_x = edge_between(&shape, [0.0; 3], [10.0, 0.0, 0.0]);
    near(
        "edge's end",
        through(&mut w, &d, along_x, [most, 5.0, 5.0]).distance,
        libm::sqrt((1e12f64 - 10.0) * (1e12 - 10.0) + 50.0),
    );
    // Every corner of the bound against every element: answered, the worker alive.
    for element in (1..=6)
        .map(face)
        .chain((1..=12).map(edge))
        .chain((1..=8).map(vertex))
    {
        for signs in [[1.0, 1.0, 1.0], [-1.0, 1.0, -1.0], [-1.0, -1.0, -1.0]] {
            let p = signs.map(|s| s * most);
            let got = through(&mut w, &d, element, p).distance;
            // No point of the cube is nearer than √3 · (1e12 − 10) or farther than √3 · 1e12
            // + √300 from a corner of the bound.
            assert!(
                got >= 3f64.sqrt() * (1e12 - 10.0) * (1.0 - 1e-12)
                    && got <= 3f64.sqrt() * 1e12 + 300f64.sqrt(),
                "{element} from {p:?}: {got}"
            );
        }
    }
}

fn d_vertex(shape: &Shape, at: [f64; 3]) -> ElementRef {
    let d = describe_whole(shape).unwrap();
    vertex(u32::try_from(d.vertices.iter().position(|v| *v == at).unwrap() + 1).unwrap())
}

fn edge_between(shape: &Shape, a: [f64; 3], b: [f64; 3]) -> ElementRef {
    let d = describe_whole(shape).unwrap();
    let i = d
        .edges
        .iter()
        .position(|e| (e.start == a && e.end == b) || (e.start == b && e.end == a))
        .unwrap();
    edge(u32::try_from(i + 1).unwrap())
}

#[test]
fn a_point_at_the_bound_from_curved_elements_answers_or_refuses_in_time() {
    // A sphere of radius 5 about the origin: from (1e12, 0, 0), 1e12 − 5; from the bound's
    // corner, √3 · 1e12 − 5. A torus of radii 10 and 2 about z: from (1e12, 0, 0), its
    // outer equator's (12, 0, 0), 1e12 − 12. A cylinder of radius 2, z from 0 to 10: from
    // (1e12, 0, 5), 1e12 − 2. Each, through a worker with its traps on, is its closed form
    // to a relative 1e-12, or a refusal as data, the worker alive after it; never a crash
    // or a hang.
    let mut w = worker();
    let most = MAX_POINT_COORDINATE;
    let ball = data(&make(&Primitive::Sphere { radius: 5.0 }).unwrap());
    let ring = data(
        &make(&Primitive::Torus {
            major_radius: 10.0,
            minor_radius: 2.0,
        })
        .unwrap(),
    );
    let can = data(
        &make(&Primitive::Cylinder {
            radius: 2.0,
            height: 10.0,
        })
        .unwrap(),
    );
    for (what, shape, element, p, want) in [
        ("sphere", &ball, face(1), [most, 0.0, 0.0], 1e12 - 5.0),
        (
            "sphere, corner",
            &ball,
            face(1),
            [most; 3],
            3f64.sqrt() * 1e12 - 5.0,
        ),
        ("torus", &ring, face(1), [most, 0.0, 0.0], 1e12 - 12.0),
        ("cylinder", &can, face(1), [most, 0.0, 5.0], 1e12 - 2.0),
    ] {
        let got = through(&mut w, shape, element, p).distance;
        assert_near(what, got, want, want * 1e-12);
    }
    // A real part's B-spline and offset faces, every one, from the bound's corner: each
    // answered in time (the point is farther from every face than 1e12) or refused as data.
    let part = from_text_brep(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/brep/offsets_and_conics.brep"),
        )
        .unwrap(),
    )
    .unwrap();
    let faces = describe_whole(&part).unwrap().faces.len();
    let part = data(&part);
    for n in 1..=u32::try_from(faces).unwrap() {
        match w.call(
            Generation(1),
            Op::PointDistance {
                shape: part.clone(),
                element: face(n),
                point: [most, -most, most],
            },
            Duration::from_secs(30),
        ) {
            Ok(Reply::Distance(d)) => assert!(d.distance > 1e12, "face {n}: {d:?}"),
            Ok(other) => panic!("face {n}: {other:?}"),
            Err(e) => assert!(
                matches!(e.code(), ErrorCode::KernelFailed | ErrorCode::InvalidResult),
                "face {n}: {e} ({:?})",
                e.code()
            ),
        }
    }
    let after = through(&mut w, &ball, face(1), [0.0, 0.0, 12.0]);
    assert_near("after", after.distance, 7.0, 1e-12);
}

// ---- On the element, and on its line past its end ----

#[test]
fn a_point_on_the_element_or_its_boundary_is_0_from_it() {
    // The cube's corner (0, 0, 0) as a point, with −0.0s: 0 from the corner, the edges and
    // faces it bounds. A point inside the bottom edge, (3, 0, 0): 0 from it, from the bottom
    // (z = 0) and the front (y = 0). (5, 5, 10), inside the top: 0.
    let mut w = worker();
    let shape = cube(10.0);
    let d = data(&shape);
    let origin = d_vertex(&shape, [0.0; 3]);
    let along_x = edge_between(&shape, [0.0; 3], [10.0, 0.0, 0.0]);
    let bottom = planar_face(&shape, [0.0, 0.0, -1.0], 0.0);
    let top = planar_face(&shape, [0.0, 0.0, 1.0], 10.0);
    for (what, element, p) in [
        ("the corner itself", origin, [-0.0, -0.0, -0.0]),
        ("the edge's end", along_x, [0.0, -0.0, 0.0]),
        ("the bottom's corner", bottom, [-0.0, 0.0, -0.0]),
        ("inside the edge", along_x, [3.0, 0.0, 0.0]),
        ("the edge on the bottom", bottom, [3.0, -0.0, 0.0]),
        ("inside the top", top, [5.0, 5.0, 10.0]),
        ("the top's edge", top, [10.0, 5.0, 10.0]),
    ] {
        let got = through(&mut w, &d, element, p);
        assert_near(what, got.distance, 0.0, 1e-12);
        assert_near(what, apart(got.to, p), 0.0, 1e-12);
    }
}

#[test]
fn a_point_on_an_edges_line_past_its_end_is_its_distance_from_that_end() {
    // The edge from the origin to (10, 0, 0), from points on its line: (15, 0, 0) is 5 past
    // its end, (−1e-6, 0, 0) a nanometre before its start, (1e6, 0, 0) a kilometre on.
    let mut w = worker();
    let shape = cube(10.0);
    let d = data(&shape);
    let along_x = edge_between(&shape, [0.0; 3], [10.0, 0.0, 0.0]);
    for (p, want, end) in [
        ([15.0, 0.0, 0.0], 5.0, [10.0, 0.0, 0.0]),
        ([-1e-6, 0.0, 0.0], 1e-6, [0.0; 3]),
        ([1e6, 0.0, 0.0], 1e6 - 10.0, [10.0, 0.0, 0.0]),
    ] {
        let got = through(&mut w, &d, along_x, p);
        assert_near(&format!("{p:?}"), got.distance, want, 1e-9);
        assert_near(&format!("{p:?}: at its end"), apart(got.to, end), 0.0, 1e-9);
    }
}

// ---- A face's hole ----

#[test]
fn a_point_in_a_faces_hole_is_measured_to_the_holes_rim() {
    // The plate's top (z = 10), the hole of radius 2.5 at (20, 10) cut from it: from its
    // rim's centre (20, 10, 10), every point of the rim is 2.5 away and nothing of the face
    // nearer; from (20, 10, 13), √(3² + 2.5²) = √15.25; from (20, 10, 5), inside the hole,
    // √(5² + 2.5²) = √31.25; from (21, 10, 10), 1 off the centre toward +x, 1.5 (the rim's
    // (22.5, 10, 10)). The bottom (z = 0) from the top rim's centre: √(10² + 2.5²).
    let mut w = worker();
    let shape = plate();
    let d = data(&shape);
    let top = planar_face(&shape, [0.0, 0.0, 1.0], 10.0);
    let bottom = planar_face(&shape, [0.0, 0.0, -1.0], 0.0);
    for (what, element, p, want) in [
        ("rim's centre", top, [20.0, 10.0, 10.0], 2.5),
        ("above the hole", top, [20.0, 10.0, 13.0], 15.25f64.sqrt()),
        ("in the hole", top, [20.0, 10.0, 5.0], 31.25f64.sqrt()),
        ("off the centre", top, [21.0, 10.0, 10.0], 1.5),
        ("the bottom", bottom, [20.0, 10.0, 10.0], 106.25f64.sqrt()),
    ] {
        let got = through(&mut w, &d, element, p);
        assert_near(what, got.distance, want, 1e-9);
        // Its witness is on the rim: 2.5 from the hole's axis.
        assert_near(
            &format!("{what}: on the rim"),
            apart([got.to[0], got.to[1], 0.0], [20.0, 10.0, 0.0]),
            2.5,
            1e-9,
        );
    }
    // At the rim's centre the rim is everywhere 2.5 away: the same pair from fresh workers.
    let first = through(&mut worker(), &d, top, [20.0, 10.0, 10.0]);
    let again = through(&mut worker(), &d, top, [20.0, 10.0, 10.0]);
    assert_eq!(first.distance.to_bits(), again.distance.to_bits());
    assert_eq!(first.to.map(f64::to_bits), again.to.map(f64::to_bits));
    assert_eq!(first.solutions, again.solutions);
}

#[test]
fn a_point_a_few_nanometres_either_side_of_a_holes_rim_is_that_far_or_0() {
    // In the top's plane along +x from the hole's centre: 10 nm short of the rim, in the
    // hole, the face is 10 nm away (1e-5 mm); 1 µm short, 1e-3; on the face 10 nm past the
    // rim, 0. The nanometre is what the core rounds to, so a face classifier's tolerance
    // that swallowed 10 nm would show.
    let mut w = worker();
    let shape = plate();
    let d = data(&shape);
    let top = planar_face(&shape, [0.0, 0.0, 1.0], 10.0);
    for (p, want) in [
        ([22.5 - 1e-5, 10.0, 10.0], 1e-5),
        ([22.5 - 1e-3, 10.0, 10.0], 1e-3),
        ([22.5 + 1e-5, 10.0, 10.0], 0.0),
        ([22.5, 10.0, 10.0], 0.0),
    ] {
        let got = through(&mut w, &d, top, p);
        assert_near(&format!("{p:?}"), got.distance, want, 1e-9);
    }
}

// ---- Every direction from the element: extrema infinite in number ----

#[test]
fn a_point_every_direction_from_an_element_is_its_radius_away_the_same_from_fresh_workers() {
    // A sphere's centre to its face: 5. A cylinder's bottom rim's centre (the origin) to the
    // rim, Edge2: 2; a point on its axis, (0, 0, 5), to its wall: 2. A torus's centre to its
    // face: its inner equator, 8. Each the same bits from two fresh workers.
    let ball = data(&make(&Primitive::Sphere { radius: 5.0 }).unwrap());
    let can = data(
        &make(&Primitive::Cylinder {
            radius: 2.0,
            height: 10.0,
        })
        .unwrap(),
    );
    let ring = data(
        &make(&Primitive::Torus {
            major_radius: 10.0,
            minor_radius: 2.0,
        })
        .unwrap(),
    );
    for (what, shape, element, p, want) in [
        ("sphere's centre", &ball, face(1), [0.0; 3], 5.0),
        ("rim's centre", &can, edge(2), [0.0; 3], 2.0),
        ("on the axis", &can, face(1), [0.0, 0.0, 5.0], 2.0),
        ("torus's centre", &ring, face(1), [0.0; 3], 8.0),
    ] {
        let one = through(&mut worker(), shape, element, p);
        let two = through(&mut worker(), shape, element, p);
        assert_near(what, one.distance, want, 1e-9);
        assert_eq!(one.distance.to_bits(), two.distance.to_bits(), "{what}");
        assert_eq!(one.to.map(f64::to_bits), two.to.map(f64::to_bits), "{what}");
    }
}

// ---- Degenerated edges ----

#[test]
fn a_point_to_a_degenerated_edge_is_its_distance_from_the_pole_or_apex() {
    // A sphere of radius 5: Edge1 its north pole (0, 0, 5), Edge3 its south pole (0, 0,
    // −5), both degenerated. From (3, 4, 5): 5 from the north pole, √(25 + 100) from the
    // south. A pointed cone of radius 5, 10 high: Edge1 its apex (0, 0, 10), degenerated;
    // from (0, 0, 20), 10.
    let mut w = worker();
    let ball = data(&make(&Primitive::Sphere { radius: 5.0 }).unwrap());
    let cone = data(
        &make(&Primitive::Cone {
            radius1: 5.0,
            radius2: 0.0,
            height: 10.0,
        })
        .unwrap(),
    );
    for (what, shape, element, p, want, at) in [
        (
            "north pole",
            &ball,
            edge(1),
            [3.0, 4.0, 5.0],
            5.0,
            [0.0, 0.0, 5.0],
        ),
        (
            "south pole",
            &ball,
            edge(3),
            [3.0, 4.0, 5.0],
            125f64.sqrt(),
            [0.0, 0.0, -5.0],
        ),
        (
            "apex",
            &cone,
            edge(1),
            [0.0, 0.0, 20.0],
            10.0,
            [0.0, 0.0, 10.0],
        ),
    ] {
        let got = through(&mut w, shape, element, p);
        assert_near(what, got.distance, want, 1e-9);
        assert_near(&format!("{what}: at it"), apart(got.to, at), 0.0, 1e-9);
    }
}

// ---- Elements the shape hasn't, garbage ----

#[test]
fn elements_past_the_shape_or_of_an_empty_compound_are_refused_counted_through_a_worker() {
    let mut w = kernel::kernel();
    let cube = data(&cube(10.0));
    let empty = data(&test_ops::empty_shape().unwrap());
    for (shape, element, message) in [
        (
            &cube,
            face(u32::MAX),
            "element, face 4294967295, doesn't exist: its shape has faces 1 to 6",
        ),
        (
            &cube,
            edge(13),
            "element, edge 13, doesn't exist: its shape has edges 1 to 12",
        ),
        (
            &cube,
            vertex(9),
            "element, vertex 9, doesn't exist: its shape has vertices 1 to 8",
        ),
        (
            &empty,
            face(1),
            "element, face 1, doesn't exist: its shape has no faces",
        ),
        (
            &empty,
            vertex(1),
            "element, vertex 1, doesn't exist: its shape has no vertices",
        ),
    ] {
        let request = KernelRequest::point_distance(shape, element, [1.0, 2.0, 3.0]).unwrap();
        let err = w.run(request).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{element}: {err}");
        assert_eq!(err.parameter(), Some("element"), "{element}");
        assert_eq!(err.message(), message);
    }
    // The worker goes on: (5, 5, −3) is 3 below the bottom.
    let request = KernelRequest::point_distance(&cube, face(5), [5.0, 5.0, -3.0]).unwrap();
    assert_near("after", w.run(request).unwrap().distance, 3.0, 1e-12);
}

#[test]
fn a_point_distance_on_garbage_says_so_and_the_worker_goes_on() {
    let mut w = kernel::kernel();
    let good = data(&cube(10.0));
    let mut cut_short = good.brep().as_bytes().to_vec();
    cut_short.truncate(cut_short.len() / 2);
    for (what, bytes) in [
        ("garbage", b"not a brep at all".to_vec()),
        ("nothing", Vec::new()),
        ("cut short", cut_short),
    ] {
        let bad = ShapeData::unchecked(Blob::inline(bytes));
        let request = KernelRequest::point_distance(&bad, face(1), [0.0; 3]).unwrap();
        let err = w.run(request).unwrap_err();
        assert!(
            matches!(
                err.code(),
                ErrorCode::InvalidInput | ErrorCode::KernelFailed
            ),
            "{what}: {err} ({:?})",
            err.code()
        );
        let request = KernelRequest::point_distance(&good, face(6), [5.0, 5.0, 12.0]).unwrap();
        assert_near(what, w.run(request).unwrap().distance, 2.0, 1e-12);
    }
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
fn the_fakes_point_distance_refusals_read_as_a_workers() {
    // The same element a shape hasn't, asked of the fake and of a worker: one code,
    // parameter, message, cause, suggestion, and elements, so a core test on the fake sees
    // what an agent would. A box has 6 faces, 12 edges, 8 vertices; a sphere 1, 3, 2.
    let fake = FakeKernel::new(1).unwrap();
    let mut w = kernel::kernel();
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
    for primitive in [
        Primitive::Box {
            dx: 1.0,
            dy: 2.0,
            dz: 3.0,
        },
        Primitive::Sphere { radius: 1.0 },
    ] {
        let fake_shape = on_fake(&fake, KernelRequest::make(primitive).unwrap()).unwrap();
        let real_shape = w.make(primitive).unwrap();
        for element in [
            face(7),
            face(2),
            edge(u32::MAX),
            edge(4),
            vertex(9),
            vertex(3),
        ] {
            let fake_said = on_fake(
                &fake,
                KernelRequest::point_distance(&fake_shape, element, [0.0; 3]).unwrap(),
            );
            let worker_said =
                w.run(KernelRequest::point_distance(&real_shape, element, [0.0; 3]).unwrap());
            match (fake_said, worker_said) {
                (Err(f), Err(r)) => assert_eq!(parts(&f), parts(&r), "{primitive:?} {element}"),
                (Ok(_), Ok(_)) => {}
                (f, r) => panic!("{primitive:?} {element}: the fake {f:?}, a worker {r:?}"),
            }
        }
    }
}

// ---- The request's own refusals ----

#[test]
fn the_core_refuses_a_point_past_the_bound_naming_it_before_any_worker_is_asked() {
    // Just past the bound in each coordinate, a NaN, and both infinities: refused by the
    // request, never sent; a solid or a 0 named before the point.
    let shape = data(&cube(1.0));
    let past = f64::from_bits(MAX_POINT_COORDINATE.to_bits() + 1);
    for p in [
        [past, 0.0, 0.0],
        [0.0, -past, 0.0],
        [0.0, 0.0, past],
        [f64::NAN, 0.0, 0.0],
        [0.0, f64::INFINITY, 0.0],
        [0.0, 0.0, f64::NEG_INFINITY],
    ] {
        let Err(err) = KernelRequest::point_distance(&shape, face(1), p) else {
            panic!("{p:?} asked");
        };
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{p:?}");
        assert_eq!(err.parameter(), Some("point"), "{p:?}");
    }
    let solid = ElementRef {
        kind: ElementKind::Solid,
        index: 1,
    };
    let Err(err) = KernelRequest::point_distance(&shape, solid, [f64::NAN; 3]) else {
        panic!("a solid asked");
    };
    assert_eq!(err.parameter(), Some("element"));
    // The worker, sent one raw, refuses it as data and goes on.
    let mut w = worker();
    let err = w
        .call(
            Generation(1),
            Op::PointDistance {
                shape: shape.clone(),
                element: face(1),
                point: [0.0, 0.0, past],
            },
            Duration::from_secs(30),
        )
        .unwrap_err();
    assert_eq!(err.parameter(), Some("point"), "{err}");
    assert_near(
        "after",
        through(&mut w, &shape, face(2), [3.0, 0.5, 0.5]).distance,
        2.0,
        1e-12,
    );
}

// ---- A property: a cube's every element, against the closed form ----

/// A coordinate biased toward where a point's foot changes which part of an element is
/// nearest: the cube's faces (0, 10, ±0.0), a nanometre and OCCT's precision either side of
/// them, its middle, a kilometre and the bound out, and any value between.
fn coordinate() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(10.0),
        Just(5.0),
        Just(1e-6),
        Just(-1e-6),
        Just(10.0 + 1e-6),
        Just(10.0 - 1e-6),
        Just(1e-7),
        Just(-1e-7),
        Just(1e6),
        Just(-1e6),
        Just(MAX_POINT_COORDINATE),
        Just(-MAX_POINT_COORDINATE),
        -20.0..30.0f64,
    ]
}

/// A cube element's extent, an axis-aligned box: its least and greatest corner.
fn extents(shape: &Shape) -> Vec<(ElementRef, [f64; 3], [f64; 3])> {
    let d = describe_whole(shape).unwrap();
    let mut all = Vec::new();
    for (i, f) in d.faces.iter().enumerate() {
        let Surface::Plane { normal } = f.surface else {
            panic!("a cube's face isn't flat")
        };
        let axis = normal.iter().position(|c| c.abs() == 1.0).unwrap();
        let (mut lo, mut hi) = ([0.0; 3], [10.0; 3]);
        lo[axis] = f.centroid[axis];
        hi[axis] = f.centroid[axis];
        all.push((face(u32::try_from(i + 1).unwrap()), lo, hi));
    }
    for (i, e) in d.edges.iter().enumerate() {
        let lo = [0, 1, 2].map(|k| e.start[k].min(e.end[k]));
        let hi = [0, 1, 2].map(|k| e.start[k].max(e.end[k]));
        all.push((edge(u32::try_from(i + 1).unwrap()), lo, hi));
    }
    for (i, v) in d.vertices.iter().enumerate() {
        all.push((vertex(u32::try_from(i + 1).unwrap()), *v, *v));
    }
    all
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn a_points_distance_from_a_cubes_element_is_its_distance_from_the_elements_box(
        p in [coordinate(), coordinate(), coordinate()],
        pick in 0usize..26,
    ) {
        // A face, an edge, or a vertex of an axis-aligned cube is an axis-aligned box, flat
        // in one, two, or three of its axes: the least distance from a point is the length
        // of how far the point is outside it along each axis.
        let shape = cube(10.0);
        let all = extents(&shape);
        let (element, lo, hi) = all[pick];
        let out = [0, 1, 2].map(|k| (lo[k] - p[k]).max(p[k] - hi[k]).max(0.0));
        let want = libm::sqrt(out[0] * out[0] + out[1] * out[1] + out[2] * out[2]);
        let d = point_distance(&shape, element, p).unwrap();
        let scale = 1.0 + p.iter().fold(0.0f64, |m, c| m.max(c.abs()));
        prop_assert!(
            (d.distance - want).abs() <= 1e-7 + 1e-14 * scale,
            "{element} from {p:?}: got {}, want {want}", d.distance
        );
        prop_assert_eq!(d.from, p);
        // Its witness is on the element, within OCCT's precision.
        for k in 0..3 {
            prop_assert!(
                d.to[k] >= lo[k] - 1e-7 - 1e-15 * scale && d.to[k] <= hi[k] + 1e-7 + 1e-15 * scale,
                "{element} from {p:?}: its witness {:?} is off it", d.to
            );
        }
    }
}

// ---- A real part: every hole's centre against every flat face, against a mesh ----

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The point of the triangle `a b c` nearest `p` (Ericson, Real-Time Collision Detection,
/// 5.1.5: by the Voronoi region of the triangle `p` is in).
fn nearest_on_triangle(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f64; 3] {
    let lerp =
        |o: [f64; 3], d: [f64; 3], t: f64| [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]];
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return lerp(a, ab, d1 / (d1 - d3));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return lerp(a, ac, d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return lerp(b, sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    [
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ]
}

#[test]
fn every_holes_centre_on_a_real_part_is_its_meshed_distance_from_every_flat_face() {
    // NIST's CTC 01: every circle's or arc's centre (its holes', bosses', rounds') against
    // every planar face, as bounded, against the least distance from the centre to that
    // face's triangles: a planar face's mesh lies in its plane and covers it but for its
    // curved boundaries' chords, each within the mesh's deflection of the boundary, so the
    // two agree to the deflection. A face measured as its plane, a hole ignored, or a
    // boundary missed shows as millimetres.
    let part = from_text_brep(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/brep/nist_ctc_01_asme1_ap242-e1.brep"),
        )
        .unwrap(),
    )
    .unwrap();
    let described = describe_whole(&part).unwrap();
    let meshed = silico_kernel::tessellate_at_least_the_floor(&part, 0.01).unwrap();
    let mesh = &meshed.mesh;
    let mut first = 0usize;
    let mut triangles_of = Vec::new();
    for &count in &mesh.face_triangles {
        let count = count as usize;
        triangles_of.push(first..first + count);
        first += count;
    }
    let centres: Vec<[f64; 3]> = described
        .edges
        .iter()
        .filter_map(|e| match &e.curve {
            silico_protocol::EdgeCurve::Circle { axis, .. } => Some(axis.origin),
            _ => None,
        })
        .collect();
    assert!(centres.len() > 10, "{} circles", centres.len());
    let mut checked = 0;
    let mut worst = (0.0f64, String::new());
    for (i, f) in described.faces.iter().enumerate() {
        if !matches!(f.surface, Surface::Plane { .. }) || triangles_of[i].is_empty() {
            continue;
        }
        let element = face(u32::try_from(i + 1).unwrap());
        for &c in &centres {
            let exact = point_distance(&part, element, c).unwrap().distance;
            let meshed = mesh.triangles[triangles_of[i].clone()]
                .iter()
                .map(|t| {
                    let [a, b, cc] = t.map(|k| mesh.positions[k as usize]);
                    apart(c, nearest_on_triangle(c, a, b, cc))
                })
                .fold(f64::INFINITY, f64::min);
            let off = (exact - meshed).abs();
            if off > worst.0 {
                worst = (
                    off,
                    format!("{element} from {c:?}: exact {exact}, meshed {meshed}"),
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 100, "{checked} pairs");
    assert!(
        worst.0 <= 1.5 * meshed.deflection + 1e-6,
        "worst of {checked} (deflection {}): {}",
        meshed.deflection,
        worst.1
    );
}
