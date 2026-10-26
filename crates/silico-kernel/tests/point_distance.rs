//! `distance`'s `center_to_element`, its kernel half (`silico_kernel::point_distance`,
//! `Op::PointDistance`): a point's least distance from an element as bounded, as FreeCAD's
//! QuickMeasure measures a circle's centre against a face or an edge
//! (`Measurement::circleCenterDistance`: the centre made a vertex, and
//! `BRepExtrema_DistShapeShape` from it to the element). A point over a face, past its edge,
//! off an edge's end, beside its middle, and at a vertex; each refusal, on the bridge and
//! through a worker. Every expected value is by hand.

use super::cube_elements::{cube, edge_between, vertex_at};

use std::path::Path;
use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::kernel::{Blob, KernelRequest, KernelWorker, ShapeData};
use silico_core::worker::Worker;
use silico_kernel::{point_distance, to_brep};
use silico_protocol::{ElementKind, ElementRef, Generation, Op, Refusal, Reply};

fn close(what: &str, got: f64, want: f64) {
    assert!((got - want).abs() < 1e-12, "{what}: got {got}, want {want}");
}

fn assert_point(what: &str, got: [f64; 3], want: [f64; 3]) {
    let off = (0..3).map(|i| (got[i] - want[i]).abs()).fold(0.0, f64::max);
    assert!(off < 1e-9, "{what}: got {got:?}, want {want:?}");
}

#[test]
fn a_point_over_a_face_is_its_height_above_it_met_at_its_foot() {
    // (3, 4, 25) over the top (z = 10): 15 above it, its foot (3, 4, 10) inside the face.
    let d = point_distance(&cube(10.0), ElementRef::face(6), [3.0, 4.0, 25.0]).unwrap();
    close("distance", d.distance, 15.0);
    assert_eq!(d.from, [3.0, 4.0, 25.0]);
    assert_point("to", d.to, [3.0, 4.0, 10.0]);
    assert_eq!(d.solutions, 1);
}

#[test]
fn a_point_past_a_faces_edge_is_measured_to_the_face_as_bounded_not_its_plane() {
    // (13, 4, 14): 4 above the top's plane, but its foot (13, 4, 10) is 3 past the top's
    // edge at x = 10. As bounded, the nearest point of the face is on that edge, (10, 4,
    // 10): √(3² + 4²) = 5 away, not 4.
    let d = point_distance(&cube(10.0), ElementRef::face(6), [13.0, 4.0, 14.0]).unwrap();
    close("distance", d.distance, 5.0);
    assert_eq!(d.from, [13.0, 4.0, 14.0]);
    assert_point("to", d.to, [10.0, 4.0, 10.0]);
}

#[test]
fn a_point_off_an_edges_end_is_measured_to_its_end_and_one_beside_it_to_its_middle() {
    let shape = cube(10.0);
    // The edge along x from (0, 0, 0) to (10, 0, 0).
    let along_x = edge_between(&shape, [0.0, 0.0, 0.0], [10.0, 0.0, 0.0]);
    // (−3, −4, 0) is 3 past its end at the origin along it and 4 off it: its nearest point
    // is that end, √(3² + 4²) = 5 away, where its line, unbounded, would be 4.
    let end = point_distance(&shape, along_x, [-3.0, -4.0, 0.0]).unwrap();
    close("off the end", end.distance, 5.0);
    assert_point("its end", end.to, [0.0, 0.0, 0.0]);
    // (5, 3, 4), beside its middle: its foot (5, 0, 0), √(3² + 4²) = 5 away.
    let middle = point_distance(&shape, along_x, [5.0, 3.0, 4.0]).unwrap();
    close("beside it", middle.distance, 5.0);
    assert_eq!(middle.from, [5.0, 3.0, 4.0]);
    assert_point("its foot", middle.to, [5.0, 0.0, 0.0]);
}

#[test]
fn a_point_is_its_straight_line_from_a_vertex() {
    // The corner at (10, 10, 10) and (12, 13, 16): √(2² + 3² + 6²) = √49 = 7.
    let shape = cube(10.0);
    let corner = vertex_at(&shape, [10.0, 10.0, 10.0]);
    let d = point_distance(&shape, corner, [12.0, 13.0, 16.0]).unwrap();
    close("distance", d.distance, 7.0);
    assert_eq!(d.to, [10.0, 10.0, 10.0]);
    assert_eq!(d.solutions, 1);
    // A point on the face is 0 from it, met at itself.
    let on = point_distance(&shape, ElementRef::face(6), [5.0, 5.0, 10.0]).unwrap();
    close("on the face", on.distance, 0.0);
    assert_point("on the face", on.to, [5.0, 5.0, 10.0]);
}

#[test]
fn an_element_its_shape_hasnt_a_solid_a_0_or_a_point_not_finite_is_refused_naming_it() {
    let shape = cube(10.0);
    let missing = point_distance(&shape, ElementRef::face(7), [0.0; 3]).unwrap_err();
    assert_eq!(
        missing.refusal(),
        &Refusal::ElementMissing {
            parameter: "element".to_owned(),
            element: ElementRef::face(7),
            count: 6,
        }
    );
    // A cube has 12 edges: the 13th is refused, counted.
    let edges = point_distance(&shape, ElementRef::edge(13), [0.0; 3]).unwrap_err();
    assert_eq!(
        edges.refusal(),
        &Refusal::ElementMissing {
            parameter: "element".to_owned(),
            element: ElementRef::edge(13),
            count: 12,
        }
    );
    let solid = ElementRef {
        kind: ElementKind::Solid,
        index: 1,
    };
    let refused = point_distance(&shape, solid, [0.0; 3]).unwrap_err();
    assert_eq!(
        refused.refusal(),
        &Refusal::InvalidParameter {
            parameter: "element".to_owned(),
            message: "element must be a face, an edge, or a vertex, not a solid".to_owned(),
        }
    );
    let zero = point_distance(&shape, ElementRef::edge(0), [0.0; 3]).unwrap_err();
    assert_eq!(
        zero.refusal(),
        &Refusal::InvalidParameter {
            parameter: "element".to_owned(),
            message: "elements are numbered from 1, so element can't be 0".to_owned(),
        }
    );
    let nan = point_distance(&shape, ElementRef::face(1), [0.0, f64::NAN, 0.0]).unwrap_err();
    assert_eq!(
        nan.refusal(),
        &Refusal::InvalidParameter {
            parameter: "point".to_owned(),
            message: "point [0.0, NaN, 0.0] must be finite and within 1000000000000 mm of the \
                      origin"
                .to_owned(),
        }
    );
}

#[test]
fn through_a_worker_the_distance_is_the_bridges_and_a_refusal_is_data() {
    let worker_path = Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"));
    let mut worker = KernelWorker::start_worker(worker_path).unwrap();
    let shape = cube(10.0);
    let data = ShapeData::checked(Blob::inline(to_brep(&shape).unwrap()), true);
    let request =
        KernelRequest::point_distance(&data, ElementRef::face(6), [13.0, 4.0, 14.0]).unwrap();
    let through = worker.run(request).unwrap();
    let here = point_distance(&shape, ElementRef::face(6), [13.0, 4.0, 14.0]).unwrap();
    assert_eq!(through, here);
    close("through the worker", through.distance, 5.0);
    // A face the shape hasn't: the worker's refusal, as the core's error, naming it.
    let request = KernelRequest::point_distance(&data, ElementRef::face(9), [0.0; 3]).unwrap();
    let error = worker.run(request).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.parameter(), Some("element"));
    assert_eq!(
        error.message(),
        "element, face 9, doesn't exist: its shape has faces 1 to 6"
    );
    // The worker lives on.
    let request =
        KernelRequest::point_distance(&data, ElementRef::face(5), [1.0, 1.0, -2.0]).unwrap();
    close("after", worker.run(request).unwrap().distance, 2.0);
}

#[test]
fn the_worker_refuses_a_point_the_core_would_never_send() {
    // The core refuses a point that isn't finite before it asks; the worker refuses one
    // sent to it raw too, as data, naming the point, and goes on.
    let mut w = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap();
    let timeout = Duration::from_secs(30);
    let shape = ShapeData::checked(Blob::inline(to_brep(&cube(10.0)).unwrap()), true);
    for point in [[f64::INFINITY, 0.0, 0.0], [0.0, 0.0, 2e12]] {
        let err = w
            .call(
                Generation(1),
                Op::PointDistance {
                    shape: shape.clone(),
                    element: ElementRef::face(1),
                    point,
                },
                timeout,
            )
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
        assert_eq!(err.parameter(), Some("point"), "{err}");
    }
    match w
        .call(
            Generation(1),
            Op::PointDistance {
                shape,
                element: ElementRef::face(1),
                point: [-4.0, 5.0, 5.0],
            },
            timeout,
        )
        .unwrap()
    {
        // 4 out from the face x = 0, its foot (0, 5, 5) inside it.
        Reply::Distance(d) => {
            close("x = 0", d.distance, 4.0);
            assert_point("foot", d.to, [0.0, 5.0, 5.0]);
        }
        other => panic!("PointDistance replied {other:?}"),
    }
}
