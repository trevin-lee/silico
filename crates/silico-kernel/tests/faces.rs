//! M3.5: faces from a profile's regions (docs/design/m3.5-profile.md, the kernel's half).
//! FreeCAD's FaceMakerBullseye tests that port, with their analytic areas, on each of D6's
//! planes, the face's normal the plane's; each curve's edge where the curve is, so the
//! history's curve-to-edge map is checked against the geometry; and refusals, through the
//! worker and in it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::close;
use super::imports;
use super::planes;
use crate::wait::WAIT;

use std::f64::consts::{FRAC_PI_2, PI};
use std::path::Path;

use close::assert_close;
use silico_core::ErrorCode;
use silico_core::kernel::KernelWorker;
use silico_core::worker::Worker;
use silico_kernel::{
    Built, Curve, FaceRegions, KernelErrorKind, Region, SketchPlane, make_faces, summarize,
    tessellate, to_brep,
};
use silico_protocol::{DebugOp, Generation, Misbehavior, Op};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
    Curve::Line { start, end }
}

fn circle(center: [f64; 2], radius: f64) -> Curve {
    Curve::Circle { center, radius }
}

/// The `w` × `h` rectangle from `(x, y)`, counter-clockwise from its lowest corner.
fn rectangle(x: f64, y: f64, w: f64, h: f64) -> Vec<Curve> {
    let p = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    (0..4).map(|i| line(p[i], p[(i + 1) % 4])).collect()
}

/// The loop walked the other way: each curve's ends and angles swapped.
fn reversed(curves: &[Curve]) -> Vec<Curve> {
    curves
        .iter()
        .rev()
        .map(|c| match *c {
            Curve::Line { start, end } => line(end, start),
            Curve::Arc {
                center,
                start,
                end,
                radius,
                start_angle,
                end_angle,
            } => Curve::Arc {
                center,
                start: end,
                end: start,
                radius,
                start_angle: end_angle,
                end_angle: start_angle,
            },
            circle => circle,
        })
        .collect()
}

/// A slot: half circles of radius 1 about (0, 0) and (4, 0) joined by lines 4 long,
/// counter-clockwise from (0, -1): 8 + π.
fn slot() -> Vec<Curve> {
    vec![
        line([0.0, -1.0], [4.0, -1.0]),
        Curve::Arc {
            center: [4.0, 0.0],
            start: [4.0, -1.0],
            end: [4.0, 1.0],
            radius: 1.0,
            start_angle: -FRAC_PI_2,
            end_angle: FRAC_PI_2,
        },
        line([4.0, 1.0], [0.0, 1.0]),
        Curve::Arc {
            center: [0.0, 0.0],
            start: [0.0, 1.0],
            end: [0.0, -1.0],
            radius: 1.0,
            start_angle: FRAC_PI_2,
            end_angle: 3.0 * FRAC_PI_2,
        },
    ]
}

/// The loop moved `dx` along the sketch's x.
fn moved(curves: &[Curve], dx: f64) -> Vec<Curve> {
    let by = |p: [f64; 2]| [p[0] + dx, p[1]];
    curves
        .iter()
        .map(|c| match *c {
            Curve::Line { start, end } => line(by(start), by(end)),
            Curve::Arc {
                center,
                start,
                end,
                radius,
                start_angle,
                end_angle,
            } => Curve::Arc {
                center: by(center),
                start: by(start),
                end: by(end),
                radius,
                start_angle,
                end_angle,
            },
            Curve::Circle { center, radius } => circle(by(center), radius),
        })
        .collect()
}

fn region(outer: Vec<Curve>, holes: Vec<Vec<Curve>>) -> Region {
    Region { outer, holes }
}

fn on(plane: SketchPlane, regions: Vec<Region>) -> FaceRegions {
    FaceRegions { plane, regions }
}

/// A case: what it is, its regions, its area, and its faces, edges, and vertices.
type Case = (&'static str, Vec<Region>, f64, (u32, u32, u32));

/// FreeCAD's FaceMakerBullseye cases that port (tests/src/Mod/Part/App/FaceMakerBullseye.cpp
/// on main, 88c80b3ed8), and the slot, each with its area worked out by hand and its
/// faces, edges, and vertices: a line or arc has a vertex at its start, a circle one at its
/// seam.
fn bullseye_cases() -> Vec<Case> {
    let grid: Vec<Vec<Curve>> = (0..25)
        .map(|i| {
            let (row, col) = (f64::from(i / 5), f64::from(i % 5));
            vec![circle([10.0 + 20.0 * col, 10.0 + 20.0 * row], 1.0)]
        })
        .collect();
    vec![
        // faceDrillerSimpleRectangle.
        (
            "4 × 3 rectangle",
            vec![region(rectangle(0.0, 0.0, 4.0, 3.0), vec![])],
            12.0,
            (1, 4, 4),
        ),
        // faceDrillerReversedOuterWire: the same rectangle drawn from its far corner,
        // which the core orients; it starts where its smallest curve ID is.
        (
            "4 × 3 rectangle from its far corner",
            vec![region(
                vec![
                    line([4.0, 3.0], [0.0, 3.0]),
                    line([0.0, 3.0], [0.0, 0.0]),
                    line([0.0, 0.0], [4.0, 0.0]),
                    line([4.0, 0.0], [4.0, 3.0]),
                ],
                vec![],
            )],
            12.0,
            (1, 4, 4),
        ),
        // faceDrillerWithOneHole, faceDrillerReversedHoleWire, buildEssenceOuterWithHole.
        (
            "4 × 3 with a hole",
            vec![region(
                rectangle(0.0, 0.0, 4.0, 3.0),
                vec![vec![circle([2.0, 1.5], 0.5)]],
            )],
            12.0 - PI / 4.0,
            (1, 5, 5),
        ),
        // faceDrillerMultipleHoles.
        (
            "10 × 6 with three holes",
            vec![region(
                rectangle(0.0, 0.0, 10.0, 6.0),
                vec![
                    vec![circle([2.0, 3.0], 0.5)],
                    vec![circle([5.0, 3.0], 1.0)],
                    vec![circle([8.0, 3.0], 0.75)],
                ],
            )],
            60.0 - 1.8125 * PI,
            (1, 7, 7),
        ),
        // buildEssenceNestedIslands: the island inside the hole is a region of its own.
        (
            "nested islands",
            vec![
                region(
                    rectangle(0.0, 0.0, 10.0, 10.0),
                    vec![vec![circle([5.0, 5.0], 4.0)]],
                ),
                region(vec![circle([5.0, 5.0], 2.0)], vec![]),
            ],
            100.0 - 12.0 * PI,
            (2, 6, 6),
        ),
        // buildEssenceManyWires.
        (
            "100 × 100 with 25 holes",
            vec![region(rectangle(0.0, 0.0, 100.0, 100.0), grid)],
            10000.0 - 25.0 * PI,
            (1, 29, 29),
        ),
        // makeElementFaceClosedWire's circle (TopoShapeExpansion.cpp).
        (
            "circle r 1",
            vec![region(vec![circle([0.0, 0.0], 1.0)], vec![])],
            PI,
            (1, 1, 1),
        ),
        // The slot, and the slot as a hole (walked clockwise, its arcs turning clockwise)
        // in a 10 × 6 rectangle.
        ("slot", vec![region(slot(), vec![])], 8.0 + PI, (1, 4, 4)),
        (
            "slot hole",
            vec![region(
                rectangle(-3.0, -3.0, 10.0, 6.0),
                vec![reversed(&slot())],
            )],
            60.0 - (8.0 + PI),
            (1, 8, 8),
        ),
    ]
}

/// The box around the sketch's points `low` to `high`, placed on `plane`, whose axes are
/// model space's (one either way along each): its two corners placed, each coordinate's
/// least and greatest.
fn placed_box(plane: SketchPlane, low: [f64; 2], high: [f64; 2]) -> [[f64; 3]; 2] {
    let (a, b) = (plane.place(low), plane.place(high));
    [
        std::array::from_fn(|k| a[k].min(b[k])),
        std::array::from_fn(|k| a[k].max(b[k])),
    ]
}

/// Every normal of a fine mesh of `built`'s faces is `plane`'s.
fn assert_faces_face(what: &str, built: &Built, plane: SketchPlane) {
    let [low, high] = summarize(&built.shape).unwrap().bounds.unwrap();
    let mesh = tessellate(&built.shape, 1e-3 * distance(low, high)).unwrap();
    assert!(!mesh.normals.is_empty(), "{what}");
    let normal = plane.normal();
    for n in &mesh.normals {
        for k in 0..3 {
            assert!(
                (f64::from(n[k]) - normal[k]).abs() < 1e-6,
                "{what} on {plane:?}: a normal {n:?}, want {normal:?}"
            );
        }
    }
}

#[test]
fn freecads_bullseye_cases_have_their_areas_on_every_plane_facing_its_normal() {
    for (what, regions, area, (faces, edges, vertices)) in bullseye_cases() {
        for plane in planes::every() {
            let built = make_faces(&on(plane, regions.clone())).unwrap();
            let s = summarize(&built.shape).unwrap();
            assert_close(&format!("{what} on {plane:?}"), s.area, area, 1e-9);
            assert_eq!(
                (s.faces, s.edges, s.vertices),
                (faces, edges, vertices),
                "{what} on {plane:?}"
            );
            // Faces enclose nothing: OCCT's volume of them is 0 exactly on a plane whose axes
            // are model space's, and 0 to rounding on the tilted frame, whose points are
            // placed to rounding (−4.5e-16 for the 4 × 3 rectangle's 12 mm²).
            if planes::square().contains(&plane) {
                assert_eq!(s.volume, 0.0, "{what} on {plane:?}");
            } else {
                assert!(
                    s.volume.abs() <= 1e-12 * area,
                    "{what} on {plane:?}: {}",
                    s.volume
                );
            }
            assert!(s.valid, "{what} on {plane:?}");
            assert_faces_face(what, &built, plane);
        }
    }
}

#[test]
fn the_faces_lie_on_the_plane_where_the_sketch_puts_them() {
    // The rectangle with a hole, and the slot (its arcs reach x = -1 and x = 5), boxed
    // where each plane puts the sketch's x and y: each origin plane, and the reversed top,
    // whose x runs along −X from z = 7.
    let cases = [
        (
            region(
                rectangle(1.0, 2.0, 4.0, 3.0),
                vec![vec![circle([3.0, 3.5], 0.5)]],
            ),
            [1.0, 2.0],
            [5.0, 5.0],
        ),
        (region(slot(), vec![]), [-1.0, -1.0], [5.0, 1.0]),
    ];
    for (r, low, high) in cases {
        for plane in planes::square() {
            let built = make_faces(&on(plane, vec![r.clone()])).unwrap();
            let bounds = summarize(&built.shape).unwrap().bounds.unwrap();
            let want = placed_box(plane, low, high);
            for (got, want) in bounds.iter().flatten().zip(want.iter().flatten()) {
                assert!(
                    (got - want).abs() < 1e-9,
                    "{plane:?}: {bounds:?}, want {want:?}"
                );
            }
        }
    }
}

/// The distance from `p` to the segment `a`–`b`.
fn to_segment(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let d: Vec<f64> = (0..3).map(|k| b[k] - a[k]).collect();
    let w: Vec<f64> = (0..3).map(|k| p[k] - a[k]).collect();
    let dd: f64 = d.iter().map(|x| x * x).sum();
    let t = (w.iter().zip(&d).map(|(x, y)| x * y).sum::<f64>() / dd).clamp(0.0, 1.0);
    (0..3)
        .map(|k| (w[k] - t * d[k]) * (w[k] - t * d[k]))
        .sum::<f64>()
        .sqrt()
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3)
        .map(|k| (a[k] - b[k]) * (a[k] - b[k]))
        .sum::<f64>()
        .sqrt()
}

#[test]
fn each_curve_is_kept_as_the_edge_where_it_is_and_each_point_as_its_vertex() {
    // Two regions, the first with lines, an arc each way, and a circle hole; the second a
    // circle: every kind of curve, in an outer loop and a hole.
    let regions = vec![
        region(
            rectangle(-3.0, -3.0, 10.0, 6.0),
            vec![reversed(&slot()), vec![circle([6.0, 2.0], 0.5)]],
        ),
        region(vec![circle([20.0, 0.0], 2.0)], vec![]),
        region(moved(&slot(), 30.0), vec![]),
    ];
    for plane in planes::every() {
        let sent = on(plane, regions.clone());
        let built = make_faces(&sent).unwrap();
        let h = &built.history;
        assert_eq!(h.validate(1), Ok(()));
        let input = &h.inputs[0];
        let curves: Vec<&Curve> = sent.curves().collect();
        assert_eq!(
            (input.faces.len(), input.edges.len(), input.vertices.len()),
            (0, curves.len(), sent.points().count())
        );
        let mesh = tessellate(&built.shape, 0.005).unwrap();
        let polylines: Vec<&[[f64; 3]]> = mesh.edges().collect();
        // Each point's vertex, by where the edge starting there begins.
        let mut point = 0usize;
        for (n, (curve, fate)) in curves.iter().zip(&input.edges).enumerate() {
            assert!(
                fate.modified.is_empty() && fate.generated.is_empty(),
                "{fate:?}"
            );
            let edge = fate.kept.unwrap();
            let points = polylines[edge as usize - 1];
            let on_curve = |p: [f64; 3]| match **curve {
                Curve::Line { start, end } => to_segment(p, plane.place(start), plane.place(end)),
                Curve::Arc { center, radius, .. } | Curve::Circle { center, radius } => {
                    (distance(p, plane.place(center)) - radius).abs()
                }
            };
            for &p in points {
                assert!(
                    on_curve(p) < 1e-9,
                    "{plane:?}: curve {} is Edge{edge}, whose point {p:?} isn't on it",
                    n + 1
                );
            }
            if let Some((start, end)) = curve.ends() {
                let (first, last) = (points[0], points[points.len() - 1]);
                assert!(
                    distance(first, plane.place(start)) < 1e-9,
                    "curve {}",
                    n + 1
                );
                assert!(distance(last, plane.place(end)) < 1e-9, "curve {}", n + 1);
                // An arc passes through its middle angle, not round the other way (whose
                // points, its ends included, are at least r√2 from it).
                if let Curve::Arc {
                    center,
                    radius,
                    start_angle,
                    end_angle,
                    ..
                } = **curve
                {
                    let middle = (start_angle + end_angle) / 2.0;
                    let want = plane.place([
                        center[0] + radius * libm::cos(middle),
                        center[1] + radius * libm::sin(middle),
                    ]);
                    assert!(
                        points.iter().any(|&p| distance(p, want) < radius / 2.0),
                        "{plane:?}: curve {} doesn't pass {want:?}",
                        n + 1
                    );
                }
                // Its start is the point's vertex.
                let vertex = input.vertices[point].kept.unwrap();
                assert_eq!(
                    h.topology.edge_vertices[edge as usize - 1][0],
                    vertex,
                    "{plane:?}: point {}",
                    point + 1
                );
                point += 1;
            }
        }
        // Face n is region n: its outer edges its outer loop's.
        let mut next = 0u32;
        for (n, r) in regions.iter().enumerate() {
            let outer: Vec<u32> = (next + 1..=next + r.outer.len() as u32)
                .map(|c| input.edges[c as usize - 1].kept.unwrap())
                .collect();
            let mut sorted = outer.clone();
            sorted.sort_unstable();
            assert_eq!(
                h.topology.outer_edges[n],
                sorted,
                "{plane:?}: Face{}",
                n + 1
            );
            next += r.loops().map(|l| l.len() as u32).sum::<u32>();
        }
    }
}

#[test]
fn neighbouring_curves_share_their_vertex() {
    // The slot's four curves meet at four vertices, each the end of one and the start of
    // the next.
    let built = make_faces(&on(SketchPlane::Xy, vec![region(slot(), vec![])])).unwrap();
    let h = &built.history;
    let edges: Vec<u32> = h.inputs[0].edges.iter().map(|f| f.kept.unwrap()).collect();
    for i in 0..4 {
        let this = &h.topology.edge_vertices[edges[i] as usize - 1];
        let next = &h.topology.edge_vertices[edges[(i + 1) % 4] as usize - 1];
        assert_eq!(this.len(), 2);
        assert_eq!(this[1], next[0], "curves {} and {}", i + 1, (i + 1) % 4 + 1);
    }
}

#[test]
fn faces_through_the_worker_match_in_process_and_refusals_never_reach_it() {
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    for (what, regions, area, _) in bullseye_cases() {
        let sent = on(SketchPlane::Xz, regions);
        let built = k.make_faces(&sent).unwrap();
        let here = make_faces(&sent).unwrap();
        assert_eq!(
            built.shape.brep().as_bytes(),
            to_brep(&here.shape).unwrap().as_slice(),
            "{what}"
        );
        assert_eq!(built.history, here.history, "{what}");
        assert_eq!(built.shape.valid(), Some(true));
        assert_close(what, k.summarize(&built.shape).unwrap().area, area, 1e-9);
    }

    let sent = k.requests_sent();
    let rect = || rectangle(0.0, 0.0, 4.0, 3.0);
    let mut open = rect();
    open.pop();
    let mut nan = rect();
    nan[1] = line([4.0, 0.0], [f64::NAN, 3.0]);
    let bad = [
        on(SketchPlane::Xy, vec![]),
        on(SketchPlane::Xy, vec![region(open, vec![])]),
        on(SketchPlane::Xy, vec![region(nan, vec![])]),
        on(SketchPlane::Xy, vec![region(reversed(&rect()), vec![])]),
    ];
    for regions in &bad {
        let err = k.make_faces(regions).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
        assert_eq!(err.parameter(), Some("regions"), "{err}");
        assert!(err.suggestion().is_some(), "{err}");
    }
    assert_eq!(k.requests_sent(), sent);
}

#[test]
fn the_worker_refuses_bad_regions_itself_and_goes_on() {
    // Sent straight to the worker, past the core's checks.
    let mut w = Worker::spawn(Path::new(WORKER)).unwrap();
    let timeout = WAIT;
    let rect = || rectangle(0.0, 0.0, 4.0, 3.0);
    let mut open = rect();
    open[3] = line([0.0, 3.0], [0.0, 1e-9]);
    let mut far = rect();
    far[2] = line([4.0, 3.0], [-2e6, 3.0]);
    let bad = [
        on(SketchPlane::Yz, vec![]),
        on(SketchPlane::Yz, vec![region(open, vec![])]),
        on(SketchPlane::Yz, vec![region(far, vec![])]),
        on(
            SketchPlane::Yz,
            vec![region(
                rect(),
                vec![vec![circle([2.0, 1.5], f64::INFINITY)]],
            )],
        ),
    ];
    for regions in bad {
        let err = w
            .call(Generation(1), Op::MakeFaces(regions), timeout)
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
        assert_eq!(err.parameter(), Some("regions"), "{err}");
        let good = Op::MakeFaces(on(SketchPlane::Yz, vec![region(rect(), vec![])]));
        assert!(w.call(Generation(1), good, timeout).is_ok());
    }
}

#[test]
fn faces_occt_finds_invalid_are_an_invalid_result() {
    // A hole crossing its region's side, which the core refuses before sending and the
    // worker leaves to OCCT: the face is refused, not fixed.
    let crossing = on(
        SketchPlane::Xy,
        vec![region(
            rectangle(0.0, 0.0, 10.0, 10.0),
            vec![vec![circle([10.0, 5.0], 2.0)]],
        )],
    );
    let err = make_faces(&crossing).err().unwrap();
    assert_eq!(err.kind(), KernelErrorKind::InvalidResult, "{err}");
    assert_eq!(err.message(), "the profile's faces aren't valid");
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let err = k.make_faces(&crossing).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidResult, "{err}");
    assert!(
        err.suggestion().unwrap().contains("crossing or touching"),
        "{err}"
    );
    // The worker goes on.
    let fine = on(
        SketchPlane::Xy,
        vec![region(rectangle(0.0, 0.0, 1.0, 1.0), vec![])],
    );
    assert!(k.make_faces(&fine).is_ok());
}

#[test]
fn the_same_faces_twice_give_the_same_bytes_and_history_on_two_workers() {
    let regions = on(
        SketchPlane::Xz,
        vec![
            region(
                rectangle(-3.0, -3.0, 10.0, 6.0),
                vec![reversed(&slot()), vec![circle([6.0, 2.0], 0.5)]],
            ),
            region(vec![circle([20.0, 0.0], 2.0)], vec![]),
        ],
    );
    let mut a = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let mut b = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    assert_ne!(a.pid(), b.pid());
    let (one, two) = (
        a.make_faces(&regions).unwrap(),
        b.make_faces(&regions).unwrap(),
    );
    assert_eq!(one, two);
    assert_eq!(one, a.make_faces(&regions).unwrap());
}

#[test]
fn a_faces_history_that_doesnt_fit_the_regions_is_refused() {
    let mut w = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
    w.call(
        Generation(0),
        Op::Debug(DebugOp::Misbehave(Misbehavior::BadHistory)),
        WAIT,
    )
    .unwrap();
    let mut k = KernelWorker::from_worker(w);
    let regions = on(
        SketchPlane::Xy,
        vec![region(rectangle(0.0, 0.0, 1.0, 1.0), vec![])],
    );
    let err = k.make_faces(&regions).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}
