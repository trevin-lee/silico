//! M3.6: the kernel's sweeps of a profile's faces (docs/design/m3.6-sketch-features.md, S3):
//! a prism from a start offset along a vector, through all of a base, and a revolution about
//! an axis from a start angle. Every extent form the design builds on each of D6's planes,
//! against analytic volumes and boxes; the history the design's rule asks for, pinned per
//! element kind against the geometry; and the values and replies the core refuses.
//! Needs the FreeCAD oracle (`pixi install -e oracle`): the shapes no profile's faces make
//! are written by FreeCAD.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::close;
use super::imports;
use super::oracle;
use super::planes;

use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::path::Path;
use std::time::Duration;

use close::assert_close;
use silico_core::ErrorCode;
use silico_core::kernel::{KernelRequest, KernelWorker};
use silico_core::worker::Worker;
use silico_kernel::{
    Axis, BooleanOp, Built, Curve, ElementKind, FaceRegions, History, KernelErrorKind, Region,
    Shape, ShapeSummary, SketchPlane, boolean, make, make_faces, prism, prism_through_all,
    revolution, summarize, tessellate, to_brep,
};
use silico_protocol::{
    Blob, DebugOp, Generation, Misbehavior, Op, Primitive, ShapeData, SweepKind,
    THROUGH_ALL_FACTOR, radians,
};

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

/// One region on `plane`.
fn one(plane: SketchPlane, outer: Vec<Curve>, holes: Vec<Vec<Curve>>) -> FaceRegions {
    FaceRegions {
        plane,
        regions: vec![Region { outer, holes }],
    }
}

/// The faces OCCT makes of `regions`, and their history.
fn faces(regions: &FaceRegions) -> Built {
    make_faces(regions).unwrap()
}

fn summary(built: &Built) -> ShapeSummary {
    summarize(&built.shape).unwrap()
}

fn scale(v: [f64; 3], k: f64) -> [f64; 3] {
    v.map(|c| c * k)
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The point at sketch coordinates `(x, y)` and `t` along the normal, in model space.
fn at(plane: SketchPlane, [x, y]: [f64; 2], t: f64) -> [f64; 3] {
    add(
        plane.origin(),
        add(
            add(scale(plane.x_axis(), x), scale(plane.y_axis(), y)),
            scale(plane.normal(), t),
        ),
    )
}

/// The box around sketch coordinates `xs` × `ys` from `t0` to `t1` along the normal: the
/// box of its eight corners.
fn box_of(plane: SketchPlane, xs: [f64; 2], ys: [f64; 2], ts: [f64; 2]) -> [[f64; 3]; 2] {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for x in xs {
        for y in ys {
            for t in ts {
                let p = at(plane, [x, y], t);
                for k in 0..3 {
                    low[k] = low[k].min(p[k]);
                    high[k] = high[k].max(p[k]);
                }
            }
        }
    }
    [low, high]
}

fn assert_box(what: &str, got: Option<[[f64; 3]; 2]>, want: [[f64; 3]; 2], tolerance: f64) {
    let got = got.unwrap();
    for c in 0..2 {
        for k in 0..3 {
            assert!(
                (got[c][k] - want[c][k]).abs() <= tolerance,
                "{what}: box {got:?}, want {want:?}"
            );
        }
    }
}

/// A sweep's history, checked by the core's rule and the protocol's.
fn assert_history(built: &Built, kind: SweepKind) {
    assert_eq!(built.history.validate(1), Ok(()));
    assert_eq!(built.history.sweep_fault(kind), None);
    assert!(built.history.ends.start.is_empty() && built.history.ends.end.is_empty());
}

/// The extent forms of a pad (design S1), as the core will turn them into a prism: the
/// start offset and vector along the direction `d` (the plane's normal), and where along
/// it the prism runs, worked out by hand.
#[derive(Debug, Clone, Copy)]
enum Extent {
    OneSide { length: f64, reversed: bool },
    Symmetric { length: f64 },
    TwoSides { length: f64, length2: f64 },
}

impl Extent {
    const ALL: [Extent; 4] = [
        Extent::OneSide {
            length: 10.0,
            reversed: false,
        },
        Extent::OneSide {
            length: 10.0,
            reversed: true,
        },
        Extent::Symmetric { length: 10.0 },
        Extent::TwoSides {
            length: 10.0,
            length2: 5.0,
        },
    ];

    /// The start, the vector, and the range along the normal it covers.
    fn sweep(self, normal: [f64; 3]) -> ([f64; 3], [f64; 3], [f64; 2]) {
        match self {
            Extent::OneSide {
                length,
                reversed: false,
            } => ([0.0; 3], scale(normal, length), [0.0, length]),
            Extent::OneSide {
                length,
                reversed: true,
            } => ([0.0; 3], scale(normal, -length), [-length, 0.0]),
            Extent::Symmetric { length } => (
                scale(normal, -length / 2.0),
                scale(normal, length),
                [-length / 2.0, length / 2.0],
            ),
            Extent::TwoSides { length, length2 } => (
                scale(normal, -length2),
                scale(normal, length + length2),
                [-length2, length],
            ),
        }
    }
}

#[test]
fn a_rectangle_padded_every_way_on_every_plane_is_its_box() {
    // 4 × 3 from (1, 2): 12 × the length, from where the form says to where it says.
    for plane in planes::every() {
        let made = faces(&one(plane, rectangle(1.0, 2.0, 4.0, 3.0), vec![]));
        for extent in Extent::ALL {
            let (start, vector, [a, b]) = extent.sweep(plane.normal());
            let built = prism(&made.shape, start, vector).unwrap();
            let s = summary(&built);
            let what = format!("{plane:?} {extent:?}");
            assert!(s.valid, "{what}");
            assert_close(&what, s.volume, 12.0 * (b - a), 1e-12);
            assert_close(&what, s.area, 2.0 * 12.0 + 14.0 * (b - a), 1e-12);
            assert_box(
                &what,
                s.bounds,
                box_of(plane, [1.0, 5.0], [2.0, 5.0], [a, b]),
                1e-12,
            );
            let center = at(plane, [3.0, 3.5], (a + b) / 2.0);
            for (got, want) in s.center.iter().zip(center) {
                assert!((got - want).abs() < 1e-9, "{what}: {:?}", s.center);
            }
            assert_eq!((s.faces, s.edges, s.vertices), (6, 12, 8), "{what}");
            assert_history(&built, SweepKind::Prism);
        }
    }
}

#[test]
fn a_circle_a_ring_and_the_slot_plate_padded_are_their_areas_times_the_length() {
    let apex = 80.0 / libm::tan(radians(50.0));
    // FreeCAD's slot plate (testBoxCase), its side at 50°: a trapezoid and a half disc,
    // counter-clockwise from the origin.
    let plate = vec![
        line([0.0, 0.0], [200.0, 0.0]),
        Curve::Arc {
            center: [200.0, 40.0],
            start: [200.0, 0.0],
            end: [200.0, 80.0],
            radius: 40.0,
            start_angle: -FRAC_PI_2,
            end_angle: FRAC_PI_2,
        },
        line([200.0, 80.0], [apex, 80.0]),
        line([apex, 80.0], [0.0, 0.0]),
    ];
    let reversed_circle = |c: [f64; 2], r: f64| vec![circle(c, r)];
    // Each case: its name, its outer loop and holes, its area, and its prism's faces,
    // edges, and vertices.
    type Case = (
        &'static str,
        Vec<Curve>,
        Vec<Vec<Curve>>,
        f64,
        (u32, u32, u32),
    );
    let cases: [Case; 3] = [
        (
            "circle r 2",
            vec![circle([3.0, -1.0], 2.0)],
            vec![],
            4.0 * PI,
            (3, 3, 2),
        ),
        (
            "ring 3 and 1",
            vec![circle([0.0, 0.0], 3.0)],
            vec![reversed_circle([0.0, 0.0], 1.0)],
            8.0 * PI,
            (4, 6, 4),
        ),
        (
            "slot plate",
            plate,
            vec![],
            80.0 * (400.0 - apex) / 2.0 + 800.0 * PI,
            (6, 12, 8),
        ),
    ];
    // testBoxCase: 6 faces, and 10 × (16000 − 3200 · cot 50° + 800π) = 158281.553 by hand.
    assert!((10.0 * cases[2].3 - 158281.553).abs() < 1e-3);
    for plane in planes::every() {
        for (name, outer, holes, area, counts) in cases.clone() {
            let made = faces(&one(plane, outer, holes));
            let built = prism(&made.shape, [0.0; 3], scale(plane.normal(), 10.0)).unwrap();
            let s = summary(&built);
            let what = format!("{plane:?} {name}");
            assert!(s.valid, "{what}");
            assert_close(&what, s.volume, 10.0 * area, 1e-9);
            assert_eq!((s.faces, s.edges, s.vertices), counts, "{what}");
            assert_history(&built, SweepKind::Prism);
        }
    }
}

#[test]
fn through_all_is_sized_past_the_base_by_freecads_rule_and_cuts_through_it() {
    // A 10 mm cube, and a 2 × 2 square on XY inside its bottom: the box around both is the
    // cube's, √300 across, so the prism is 2.02 √300 long, up from the plane; cut from the
    // cube, it takes 2 × 2 × 10.
    let cube = make(&Primitive::Box {
        dx: 10.0,
        dy: 10.0,
        dz: 10.0,
    })
    .unwrap();
    let made = faces(&one(SketchPlane::Xy, rectangle(4.0, 4.0, 2.0, 2.0), vec![]));
    let built = prism_through_all(&made.shape, &cube, [0.0, 0.0, 1.0]).unwrap();
    let length = THROUGH_ALL_FACTOR * 300f64.sqrt();
    let s = summary(&built);
    assert_close("through all", s.volume, 4.0 * length, 1e-12);
    assert_box(
        "through all",
        s.bounds,
        [[4.0, 4.0, 0.0], [6.0, 6.0, length]],
        1e-12,
    );
    assert!(length > 10.0);
    assert_history(&built, SweepKind::Prism);
    let cut = boolean(BooleanOp::Cut, &cube, &built.shape).unwrap();
    assert_close(
        "cut through",
        summarize(&cut.shape).unwrap().volume,
        960.0,
        1e-12,
    );
    // The profile's box counts too: a square far from the cube lengthens it, on XZ, along
    // +Y (against the plane's normal, as a pocket goes).
    let far = faces(&one(
        SketchPlane::Xz,
        rectangle(30.0, 0.0, 10.0, 10.0),
        vec![],
    ));
    let built = prism_through_all(&far.shape, &cube, [0.0, 1.0, 0.0]).unwrap();
    // The box from (0, 0, 0) to (40, 10, 10): √1800.
    let length = THROUGH_ALL_FACTOR * 1800f64.sqrt();
    let s = summary(&built);
    assert_close("far", s.volume, 100.0 * length, 1e-12);
    assert_box(
        "far",
        s.bounds,
        [[30.0, 0.0, 0.0], [40.0, length, 10.0]],
        1e-12,
    );
    // Past a kilometre: refused, naming the base.
    let huge = make(&Primitive::Box {
        dx: 6e5,
        dy: 1.0,
        dz: 1.0,
    })
    .unwrap();
    let err = prism_through_all(&made.shape, &huge, [0.0, 0.0, 1.0])
        .err()
        .unwrap();
    assert_eq!(err.kind(), KernelErrorKind::InvalidInput);
    assert!(
        err.message()
            .contains("a sweep must be longer than 1e-7 mm and at most 1e6 mm"),
        "{}",
        err.message()
    );
}

/// A circle of radius `r` at `big` along the sketch's x, turned about the sketch's y axis.
fn torus_section(plane: SketchPlane, big: f64, r: f64) -> Shape {
    faces(&one(plane, vec![circle([big, 0.0], r)], vec![])).shape
}

#[test]
fn a_circle_turned_about_the_sketchs_y_axis_is_a_torus_segment_turning_the_right_way() {
    // Main's Revolution.cpp: a circle r at R from the axis, turned θ: 2π²·R·r²·θ/360. The
    // box pins the sense: right-handed about +y takes the sketch's +x towards −normal
    // (FreeCAD: a profile at +X about +Y turns toward −Z).
    let (big, r) = (30.0, 10.0);
    let reach = big + r;
    for plane in planes::square() {
        let section = torus_section(plane, big, r);
        let y = plane.y_axis();
        let up = Axis {
            origin: plane.origin(),
            direction: y,
        };
        let down = Axis {
            origin: plane.origin(),
            direction: scale(y, -1.0),
        };
        let s45 = reach * libm::sin(radians(45.0));
        // (axis, start, angle, θ in degrees, x range, normal range)
        let cases = [
            (up, 0.0, 90.0, [0.0, reach], [-reach, 0.0], SweepKind::Turn),
            (down, 0.0, 90.0, [0.0, reach], [0.0, reach], SweepKind::Turn),
            (
                up,
                0.0,
                180.0,
                [-reach, reach],
                [-reach, 0.0],
                SweepKind::Turn,
            ),
            (
                up,
                -45.0,
                90.0,
                [(big - r) * libm::cos(radians(45.0)), reach],
                [-s45, s45],
                SweepKind::Turn,
            ),
            (
                up,
                -90.0,
                180.0,
                [0.0, reach],
                [-reach, reach],
                SweepKind::Turn,
            ),
            (
                up,
                0.0,
                360.0,
                [-reach, reach],
                [-reach, reach],
                SweepKind::WholeTurn,
            ),
            (
                up,
                -180.0,
                360.0,
                [-reach, reach],
                [-reach, reach],
                SweepKind::WholeTurn,
            ),
        ];
        for (axis, start, angle, xs, ns, kind) in cases {
            let what = format!("{plane:?} {start}° + {angle}° about {:?}", axis.direction);
            let built = revolution(&section, &axis, radians(start), radians(angle)).unwrap();
            let s = summary(&built);
            assert!(s.valid, "{what}");
            assert_close(
                &what,
                s.volume,
                2.0 * PI * PI * big * r * r * angle / 360.0,
                1e-9,
            );
            assert_box(&what, s.bounds, box_of(plane, xs, [-r, r], ns), 1e-6);
            assert_history(&built, kind);
        }
    }
}

#[test]
fn a_rectangle_turned_about_its_own_side_is_a_cylinder() {
    // π·r²·h, and for θ, that times θ/360; its side on the axis.
    let (r, h) = (3.0, 7.0);
    for plane in planes::square() {
        let made = faces(&one(plane, rectangle(0.0, 0.0, r, h), vec![]));
        let axis = Axis {
            origin: plane.origin(),
            direction: plane.y_axis(),
        };
        for (start, angle, kind) in [
            (0.0, 360.0, SweepKind::WholeTurn),
            (0.0, 90.0, SweepKind::Turn),
            (-60.0, 120.0, SweepKind::Turn),
        ] {
            let what = format!("{plane:?} {start}° + {angle}°");
            let built = revolution(&made.shape, &axis, radians(start), radians(angle)).unwrap();
            let s = summary(&built);
            assert!(s.valid, "{what}");
            assert_close(&what, s.volume, PI * r * r * h * angle / 360.0, 1e-12);
            assert_history(&built, kind);
        }
        let whole = summary(&revolution(&made.shape, &axis, 0.0, TAU).unwrap());
        assert_box(
            &format!("{plane:?} whole"),
            whole.bounds,
            box_of(plane, [-r, r], [0.0, h], [-r, r]),
            1e-9,
        );
        assert_eq!((whole.faces, whole.edges, whole.vertices), (3, 3, 2));
    }
}

/// Which of the result's elements each input element became, by the faces' history: the
/// faces' own numbers of the regions' curve `n` (from 1).
fn curve_edge(made: &Built, n: usize) -> u32 {
    made.history.inputs[0].edges[n - 1].kept.unwrap()
}

/// Points, element by element.
type Points = Vec<Vec<[f64; 3]>>;

/// The points of each face of `shape`'s mesh, and of each edge's polyline.
fn mesh_points(shape: &Shape) -> (Points, Points) {
    let mesh = tessellate(shape, 0.01).unwrap();
    let mut faces = Vec::new();
    let mut first = 0usize;
    for &count in &mesh.face_triangles {
        let triangles = &mesh.triangles[first..first + count as usize];
        faces.push(
            triangles
                .iter()
                .flat_map(|t| t.iter().map(|&i| mesh.positions[i as usize]))
                .collect(),
        );
        first += count as usize;
    }
    let edges = mesh.edges().map(<[[f64; 3]]>::to_vec).collect();
    (faces, edges)
}

fn the(list: &[silico_protocol::Reported], kind: ElementKind) -> u32 {
    let found: Vec<u32> = list
        .iter()
        .filter(|r| r.element.kind == kind)
        .map(|r| r.element.index)
        .collect();
    assert_eq!(found.len(), 1, "{list:?}");
    found[0]
}

#[test]
fn each_elements_start_is_modified_its_end_generated_of_its_kind_and_its_sides_generated() {
    // A 4 × 3 rectangle on XZ (normal −Y), padded two sides 10 and 5: from 5 behind the
    // plane (+Y) to 10 in front (−Y). Each start counterpart lies at t = −5, each end at
    // t = 10, each side spans both.
    let plane = SketchPlane::Xz;
    let corners = [[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]];
    let made = faces(&one(plane, rectangle(0.0, 0.0, 4.0, 3.0), vec![]));
    let (start, vector, [a, b]) = Extent::TwoSides {
        length: 10.0,
        length2: 5.0,
    }
    .sweep(plane.normal());
    let built = prism(&made.shape, start, vector).unwrap();
    let h: &History = &built.history;
    assert_eq!(
        (
            h.result.faces,
            h.result.edges,
            h.result.vertices,
            h.result.solids
        ),
        (6, 12, 8, 1)
    );
    let (face_points, edge_points) = mesh_points(&built.shape);
    let along = |p: [f64; 3]| dot(p, plane.normal());
    let input = &h.inputs[0];
    assert_eq!(
        (input.faces.len(), input.edges.len(), input.vertices.len()),
        (1, 4, 4)
    );
    // The face: its start the bottom, its end the top, its side the solid.
    let face = &input.faces[0];
    assert_eq!(face.kept, None);
    assert_eq!(face.modified.len(), 1);
    let bottom = the(&face.modified, ElementKind::Face);
    let top = the(&face.generated, ElementKind::Face);
    assert_eq!(the(&face.generated, ElementKind::Solid), 1);
    assert_eq!(face.generated.len(), 2);
    for p in &face_points[bottom as usize - 1] {
        assert!((along(*p) - a).abs() < 1e-12, "{p:?}");
    }
    for p in &face_points[top as usize - 1] {
        assert!((along(*p) - b).abs() < 1e-12, "{p:?}");
    }
    // Each curve: its start edge where the curve is at t = a, its end at t = b, its side the
    // face over both.
    for n in 1..=4 {
        let fate = &input.edges[curve_edge(&made, n) as usize - 1];
        assert_eq!(
            (fate.kept, fate.modified.len(), fate.generated.len()),
            (None, 1, 2)
        );
        let (from, to) = (corners[n - 1], corners[n % 4]);
        let start_edge = &edge_points[the(&fate.modified, ElementKind::Edge) as usize - 1];
        let end_edge = &edge_points[the(&fate.generated, ElementKind::Edge) as usize - 1];
        for (polyline, t) in [(start_edge, a), (end_edge, b)] {
            let ends = [polyline[0], polyline[polyline.len() - 1]];
            let want = [at(plane, from, t), at(plane, to, t)];
            let matches = |x: [[f64; 3]; 2], y: [[f64; 3]; 2]| {
                x.iter()
                    .zip(&y)
                    .all(|(p, q)| (0..3).all(|k| (p[k] - q[k]).abs() < 1e-12))
            };
            assert!(
                matches(ends, want) || matches([ends[1], ends[0]], want),
                "curve {n} at {t}: {ends:?}, want {want:?}"
            );
        }
        let side = &face_points[the(&fate.generated, ElementKind::Face) as usize - 1];
        let (lo, hi) = side
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(along(*p)), hi.max(along(*p)))
            });
        assert!(
            (lo - a).abs() < 1e-12 && (hi - b).abs() < 1e-12,
            "curve {n}: {lo}..{hi}"
        );
    }
    // Each point: its side edge runs from it at t = a to it at t = b.
    for (k, corner) in corners.iter().enumerate() {
        let vertex = made.history.inputs[0].vertices[k].kept.unwrap();
        let fate = &input.vertices[vertex as usize - 1];
        assert_eq!(
            (fate.kept, fate.modified.len(), fate.generated.len()),
            (None, 1, 2)
        );
        the(&fate.modified, ElementKind::Vertex);
        the(&fate.generated, ElementKind::Vertex);
        let side = &edge_points[the(&fate.generated, ElementKind::Edge) as usize - 1];
        let mut ends = [along(side[0]), along(side[side.len() - 1])];
        ends.sort_by(f64::total_cmp);
        assert_eq!(ends, [a, b], "point {k}");
        let foot = at(plane, *corner, 0.0);
        for p in side {
            let off = add(*p, scale(plane.normal(), -along(*p)));
            assert!(
                (0..3).all(|i| (off[i] - foot[i]).abs() < 1e-12),
                "point {k}: {p:?}"
            );
        }
    }
}

#[test]
fn a_ring_and_a_circle_report_every_element_and_their_counts() {
    // A rectangle with a round hole: 1 face, 5 edges (the circle one), 5 vertices (the
    // circle's seam one); padded, 7 faces, 15 edges, 10 vertices, each element reported
    // once as a start, once as an end, once as a side.
    let made = faces(&one(
        SketchPlane::Yz,
        rectangle(0.0, 0.0, 10.0, 10.0),
        vec![vec![circle([5.0, 5.0], 2.0)]],
    ));
    let built = prism(&made.shape, [0.0; 3], [5.0, 0.0, 0.0]).unwrap();
    let h = &built.history;
    assert_eq!(
        (h.result.faces, h.result.edges, h.result.vertices),
        (7, 15, 10)
    );
    let input = &h.inputs[0];
    for (kind, side, fates) in [
        (ElementKind::Face, ElementKind::Solid, &input.faces),
        (ElementKind::Edge, ElementKind::Face, &input.edges),
        (ElementKind::Vertex, ElementKind::Edge, &input.vertices),
    ] {
        for fate in fates {
            assert_eq!(fate.kept, None);
            the(&fate.modified, kind);
            the(&fate.generated, kind);
            the(&fate.generated, side);
        }
    }
    assert_close(
        "ring",
        summary(&built).volume,
        5.0 * (100.0 - 4.0 * PI),
        1e-9,
    );
}

#[test]
fn a_turn_reports_an_axis_side_once_and_a_whole_turn_no_ends() {
    // A 3 × 4 rectangle on XZ against the z axis (its left side on it), turned about +Z.
    // Curves: 1 the bottom, 2 the right side, 3 the top, 4 the left side, on the axis;
    // points 1 (0, 0) and 4 (0, 4) on the axis.
    let made = faces(&one(SketchPlane::Xz, rectangle(0.0, 0.0, 3.0, 4.0), vec![]));
    let z = Axis {
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
    };
    let edge = |built: &Built, n: usize| {
        built.history.inputs[0].edges[curve_edge(&made, n) as usize - 1].clone()
    };
    let vertex = |built: &Built, k: usize| {
        let v = made.history.inputs[0].vertices[k - 1].kept.unwrap();
        built.history.inputs[0].vertices[v as usize - 1].clone()
    };
    let quarter = revolution(&made.shape, &z, 0.0, FRAC_PI_2).unwrap();
    let r = quarter.history.result;
    // Two flat ends, the cylinder's side, the two quarter discs: 5 faces.
    assert_eq!((r.faces, r.edges, r.vertices), (5, 9, 6));
    // The side on the axis: its start, which is its end, only; it sweeps nothing.
    let axis_side = edge(&quarter, 4);
    assert_eq!(
        (axis_side.modified.len(), axis_side.generated.len()),
        (1, 0)
    );
    for k in [1, 4] {
        let on = vertex(&quarter, k);
        assert_eq!((on.modified.len(), on.generated.len()), (1, 0), "point {k}");
    }
    // The others: a start, an end, a side.
    for n in 1..=3 {
        let fate = edge(&quarter, n);
        the(&fate.modified, ElementKind::Edge);
        the(&fate.generated, ElementKind::Edge);
        the(&fate.generated, ElementKind::Face);
    }
    for k in [2, 3] {
        let fate = vertex(&quarter, k);
        the(&fate.modified, ElementKind::Vertex);
        the(&fate.generated, ElementKind::Vertex);
        the(&fate.generated, ElementKind::Edge);
    }
    assert_history(&quarter, SweepKind::Turn);
    // A whole turn: no ends at all. The face is reported nothing; the top and bottom sweep
    // flat discs, with no start (no seam); the side on the axis and its points aren't in
    // the result; the right side starts at its cylinder's seam, and sweeps the cylinder.
    let whole = revolution(&made.shape, &z, 0.0, TAU).unwrap();
    let r = whole.history.result;
    assert_eq!((r.faces, r.edges, r.vertices), (3, 3, 2));
    assert_eq!(whole.history.inputs[0].faces[0], Default::default());
    for n in [1, 3] {
        let disc = edge(&whole, n);
        assert!(disc.modified.is_empty(), "curve {n}");
        the(&disc.generated, ElementKind::Face);
        assert_eq!(disc.generated.len(), 1);
    }
    assert_eq!(edge(&whole, 4), Default::default());
    let seam = edge(&whole, 2);
    the(&seam.modified, ElementKind::Edge);
    the(&seam.generated, ElementKind::Face);
    assert_eq!(seam.generated.len(), 1);
    for k in [1, 4] {
        assert_eq!(vertex(&whole, k), Default::default(), "point {k}");
    }
    for k in [2, 3] {
        let fate = vertex(&whole, k);
        the(&fate.modified, ElementKind::Vertex);
        the(&fate.generated, ElementKind::Edge);
        assert_eq!(fate.generated.len(), 1);
    }
    assert_history(&whole, SweepKind::WholeTurn);
    // Off the axis, a whole turn's slanted side sweeps a cone and starts at its seam too.
    let trapezoid = faces(&one(
        SketchPlane::Xz,
        vec![
            line([2.0, 0.0], [5.0, 0.0]),
            line([5.0, 0.0], [4.0, 4.0]),
            line([4.0, 4.0], [2.0, 4.0]),
            line([2.0, 4.0], [2.0, 0.0]),
        ],
        vec![],
    ));
    let cone = revolution(&trapezoid.shape, &z, 0.0, TAU).unwrap();
    let slant = &cone.history.inputs[0].edges[curve_edge(&trapezoid, 2) as usize - 1];
    the(&slant.modified, ElementKind::Edge);
    the(&slant.generated, ElementKind::Face);
    // Pappus: the trapezoid (2 to 4 wide, 4 high, area 10, its centroid at x 31/10 from
    // the axis... worked out as the rectangle 2..4 (8, at 3) and the triangle 4..5 (2, at
    // 13/3)): 2π × (8 × 3 + 2 × 13/3).
    assert_close(
        "cone",
        summary(&cone).volume,
        TAU * (8.0 * 3.0 + 2.0 * 13.0 / 3.0),
        1e-9,
    );
    assert_history(&cone, SweepKind::WholeTurn);
}

#[test]
fn faces_moved_to_the_start_sweep_as_from_where_they_are() {
    // A start offset and a start angle move the faces as a whole: the same counts, and the
    // same history, element for element, as the sweep from where they are.
    let made = faces(&one(SketchPlane::Xy, rectangle(1.0, 1.0, 2.0, 2.0), vec![]));
    let moved = prism(&made.shape, [0.0, 0.0, -7.0], [0.0, 0.0, 3.0]).unwrap();
    let here = prism(&made.shape, [0.0; 3], [0.0, 0.0, 3.0]).unwrap();
    assert_eq!(moved.history, here.history);
    assert_box(
        "moved",
        summary(&moved).bounds,
        [[1.0, 1.0, -7.0], [3.0, 3.0, -4.0]],
        1e-12,
    );
    let turned = revolution(
        &made.shape,
        &Axis {
            origin: [0.0; 3],
            direction: [0.0, 1.0, 0.0],
        },
        radians(-30.0),
        radians(60.0),
    )
    .unwrap();
    let from_here = revolution(
        &made.shape,
        &Axis {
            origin: [0.0; 3],
            direction: [0.0, 1.0, 0.0],
        },
        0.0,
        radians(60.0),
    )
    .unwrap();
    assert_eq!(turned.history, from_here.history);
    assert_history(&turned, SweepKind::Turn);
    // Turned back 30° about +y, the square at x 1..3 reaches z = 3 sin 30° = 1.5 above the
    // plane, and turned on by 60°, 3 sin 30° below it.
    let [low, high] = summary(&turned).bounds.unwrap();
    assert!(
        (high[2] - 1.5).abs() < 1e-9 && (low[2] + 1.5).abs() < 1e-9,
        "{low:?} {high:?}"
    );
}

#[test]
fn a_turn_whose_axis_crosses_the_faces_is_occts_failure_not_a_crash() {
    // The core refuses a crossing axis first (design S6); the kernel, given one anyway,
    // fails as OCCT does (MakeRevol's own check), and goes on.
    let made = faces(&one(
        SketchPlane::Xz,
        rectangle(-1.0, 0.0, 4.0, 3.0),
        vec![],
    ));
    let z = Axis {
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
    };
    let err = revolution(&made.shape, &z, 0.0, FRAC_PI_2).err().unwrap();
    assert_eq!(err.kind(), KernelErrorKind::Failed, "{err}");
    assert!(err.suggestion().unwrap().contains("revolve"), "{err:?}");
    let clear = faces(&one(SketchPlane::Xz, rectangle(1.0, 0.0, 4.0, 3.0), vec![]));
    assert!(revolution(&clear.shape, &z, 0.0, FRAC_PI_2).is_ok());
}

// --- Through the worker, and refusals ------------------------------------------------------

fn kernel() -> KernelWorker {
    KernelWorker::start_worker(Path::new(WORKER)).unwrap()
}

fn data(shape: &Shape) -> ShapeData {
    ShapeData::checked(Blob::inline(to_brep(shape).unwrap()), true)
}

#[test]
fn the_sweeps_run_through_the_worker_with_their_histories_checked() {
    let mut k = kernel();
    let regions = one(SketchPlane::Xy, rectangle(0.0, 0.0, 4.0, 3.0), vec![]);
    let made = k.make_faces(&regions).unwrap();
    let built = k
        .prism(&made.shape, [0.0, 0.0, -1.0], [0.0, 0.0, 2.0])
        .unwrap();
    assert_close(
        "prism",
        k.summarize(&built.shape).unwrap().volume,
        24.0,
        1e-12,
    );
    let cube = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 10.0,
            dz: 10.0,
        })
        .unwrap();
    let through = k
        .prism_through_all(&made.shape, &cube, [0.0, 0.0, 1.0])
        .unwrap();
    assert_close(
        "through all",
        k.summarize(&through.shape).unwrap().volume,
        12.0 * THROUGH_ALL_FACTOR * 300f64.sqrt(),
        1e-12,
    );
    let axis = Axis {
        origin: [-1.0, 0.0, 0.0],
        direction: [0.0, 1.0, 0.0],
    };
    let turned = k.revolution(&made.shape, axis, 0.0, TAU).unwrap();
    // Pappus: 12 × 2π × 3 (the centroid's x, 2, from the axis at −1).
    assert_close(
        "revolution",
        k.summarize(&turned.shape).unwrap().volume,
        12.0 * TAU * 3.0,
        1e-9,
    );
}

#[test]
fn values_a_sweep_cant_take_are_refused_before_sending_naming_them() {
    let faces = ShapeData::checked(Blob::inline(vec![]), true);
    let z = [0.0, 0.0, 1.0];
    let cases: [(Result<KernelRequest<_>, silico_core::Error>, &str); 6] = [
        (
            KernelRequest::prism(&faces, [f64::NAN, 0.0, 0.0], z),
            "start",
        ),
        (KernelRequest::prism(&faces, [0.0; 3], [0.0; 3]), "vector"),
        (
            KernelRequest::prism(&faces, [0.0; 3], [0.0, 0.0, 2e6]),
            "vector",
        ),
        (
            KernelRequest::prism_through_all(&faces, &faces, [0.0, 0.0, 2.0]),
            "direction",
        ),
        (
            KernelRequest::revolution(
                &faces,
                Axis {
                    origin: [0.0; 3],
                    direction: z,
                },
                0.0,
                0.0,
            ),
            "angle",
        ),
        (
            KernelRequest::revolution(
                &faces,
                Axis {
                    origin: [0.0; 3],
                    direction: [0.0, 0.0, 0.5],
                },
                0.0,
                1.0,
            ),
            "axis",
        ),
    ];
    for (request, parameter) in cases {
        let error = request.err().unwrap();
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
        assert_eq!(error.parameter(), Some(parameter), "{error:?}");
        assert!(error.suggestion().is_some());
    }
}

/// Shapes FreeCAD makes that a profile's faces never are, as its text B-reps: a box's
/// shell (six faces on three planes, and no solid), and a solid of one flat face's open
/// shell (one face, on one plane, but a solid).
const NOT_FACES: &str = r#"import json, Part
shell = Part.makeBox(1, 2, 3).Shells[0]
solid = Part.Solid(Part.Shell([Part.makePlane(2, 3)]))
assert (len(shell.Faces), len(shell.Solids)) == (6, 0)
assert (len(solid.Faces), len(solid.Solids)) == (1, 1)
print("SILICO_ORACLE " + json.dumps([shell.exportBrepToString(), solid.exportBrepToString()]))
"#;

#[test]
fn faces_on_planes_apart_or_a_solid_of_flat_faces_arent_swept() {
    // Each refused for its own reason: the shell's faces aren't on one plane, though it has
    // no solid; the solid's one face is on one plane (z = 0, which the prism leaves), but
    // it's a solid.
    let [shell, solid]: [String; 2] = serde_json::from_value(oracle::freecad(NOT_FACES)).unwrap();
    for (text, got) in [
        (shell, "6 faces and 0 solids, not on one plane"),
        (solid, "1 faces and 1 solids"),
    ] {
        let shape = silico_kernel::from_text_brep(text.as_bytes()).unwrap();
        let err = prism(&shape, [0.0; 3], [0.0, 0.0, 1.0]).err().unwrap();
        assert_eq!(
            err.refusal(),
            &silico_kernel::Refusal::InvalidParameter {
                parameter: "faces".to_owned(),
                message: format!(
                    "a sweep takes a profile's faces, on one plane, got a shape of {got}"
                ),
            },
            "{got}"
        );
    }
}

#[test]
fn the_worker_refuses_what_isnt_faces_and_bad_values_and_goes_on() {
    let timeout = Duration::from_secs(60);
    let mut worker = Worker::spawn(Path::new(WORKER)).unwrap();
    let cube = data(
        &make(&Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        })
        .unwrap(),
    );
    let refusal =
        |op: Op, worker: &mut Worker| worker.call(Generation(1), op, timeout).unwrap_err();
    // A solid isn't faces.
    let error = refusal(
        Op::Prism {
            faces: cube.clone(),
            start: [0.0; 3],
            vector: [0.0, 0.0, 1.0],
        },
        &mut worker,
    );
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.parameter(), Some("faces"));
    assert!(
        error.message().contains(
            "a sweep takes a profile's faces, on one plane, got a shape of 6 faces and 1 solids"
        ),
        "{error:?}"
    );
    // Values the core would refuse, refused again by the worker.
    let made = data(&faces(&one(SketchPlane::Xy, rectangle(0.0, 0.0, 1.0, 1.0), vec![])).shape);
    let error = refusal(
        Op::Revolution {
            faces: made.clone(),
            axis: Axis {
                origin: [0.0; 3],
                direction: [0.0, 1.0, 0.0],
            },
            start: 0.0,
            angle: 7.0,
        },
        &mut worker,
    );
    assert_eq!(error.parameter(), Some("angle"), "{error:?}");
    let error = refusal(
        Op::PrismThroughAll {
            faces: made.clone(),
            base: cube.clone(),
            direction: [0.0, 0.0, 0.9],
        },
        &mut worker,
    );
    assert_eq!(error.parameter(), Some("direction"), "{error:?}");
    // A sweep along the faces' own plane, or about their own normal: a flat "solid" OCCT
    // would call valid, refused naming the field, with what to do instead.
    for (op, parameter, says) in [
        (
            Op::Prism {
                faces: made.clone(),
                start: [0.0; 3],
                vector: [1.0, 0.0, 0.0],
            },
            "vector",
            "sweep the faces along a vector out of their plane",
        ),
        (
            Op::PrismThroughAll {
                faces: made.clone(),
                base: cube.clone(),
                direction: [0.0, 1.0, 0.0],
            },
            "direction",
            "sweep the faces along a vector out of their plane",
        ),
        (
            Op::Revolution {
                faces: made.clone(),
                axis: Axis {
                    origin: [5.0, 0.0, 0.0],
                    direction: [0.0, 0.0, 1.0],
                },
                start: 0.0,
                angle: 1.0,
            },
            "axis",
            "turn the faces about a line in their plane",
        ),
    ] {
        let error = refusal(op, &mut worker);
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
        assert_eq!(error.parameter(), Some(parameter), "{error:?}");
        assert_eq!(error.suggestion(), Some(says));
    }
    // And it goes on.
    let reply = worker
        .call(
            Generation(1),
            Op::Prism {
                faces: made,
                start: [0.0; 3],
                vector: [0.0, 0.0, 1.0],
            },
            timeout,
        )
        .unwrap();
    assert!(matches!(reply, silico_protocol::Reply::Built(_)));
}

#[test]
fn a_worker_whose_sweep_history_breaks_the_rule_is_refused_as_a_protocol_error() {
    let worker = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
    let mut k = KernelWorker::from_worker(worker);
    let made = k
        .make_faces(&one(SketchPlane::Xy, rectangle(0.0, 0.0, 1.0, 1.0), vec![]))
        .unwrap();
    k.run(KernelRequest::debug(DebugOp::Misbehave(
        Misbehavior::BadHistory,
    )))
    .unwrap();
    let error = k.prism(&made.shape, [0.0; 3], [0.0, 0.0, 1.0]).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ProtocolError, "{error:?}");
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}
