//! break-it: hostile, degenerate, and boundary inputs for the chamfer (docs/ports/chamfer.md)
//! and the edge rule it shares with the fillet. Every expected value is a hand calculation:
//! a chamfer of a straight edge between flat faces removes (or, on a concave edge, adds) a
//! prism whose section is the triangle its two reaches span, and a chamfer of a circular
//! rim a ring of that triangle, by Pappus. Which face takes `distance` is found from each
//! shape's mesh or history (the face's number), not from the code under test. Angles are
//! ones whose sines and cosines are square roots, so no platform trigonometry is needed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/chamfer_sizes.rs"]
mod chamfer_sizes;
#[path = "common/close.rs"]
mod close;
#[path = "common/kernel.rs"]
mod kernel;
#[path = "common/numbers.rs"]
mod numbers;

use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::time::{Duration, Instant};

use chamfer_sizes::{angled, equal, two};
use close::assert_close;
use kernel::kernel;
use numbers::relative_error;
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_kernel::{
    Axis, BooleanOp, Built, ChamferSize, KernelError, KernelErrorKind, Primitive, Profile, Shape,
    ShapeSummary, boolean, chamfer, extrude, fillet, make, make_box, revolve, summarize,
    tessellate, to_brep, translate,
};

fn unit_cube() -> Primitive {
    Primitive::Box {
        dx: 1.0,
        dy: 1.0,
        dz: 1.0,
    }
}

/// Angles (degrees) with their sines and cosines, exact in square roots.
fn exact_angles() -> [(f64, f64, f64); 14] {
    let (r2, r3, r6) = (2f64.sqrt(), 3f64.sqrt(), 6f64.sqrt());
    // sin and cos of 15° and of 22.5°.
    let (s15, c15) = ((r6 - r2) / 4.0, (r6 + r2) / 4.0);
    let (s22, c22) = ((2.0 - r2).sqrt() / 2.0, (2.0 + r2).sqrt() / 2.0);
    [
        (15.0, s15, c15),
        (22.5, s22, c22),
        (30.0, 0.5, r3 / 2.0),
        (45.0, r2 / 2.0, r2 / 2.0),
        (60.0, r3 / 2.0, 0.5),
        (67.5, c22, s22),
        (75.0, c15, s15),
        (90.0, 1.0, 0.0),
        (105.0, c15, -s15),
        (112.5, c22, -s22),
        (120.0, r3 / 2.0, -0.5),
        (135.0, r2 / 2.0, -r2 / 2.0),
        (150.0, 0.5, -r3 / 2.0),
        (165.0, s15, -c15),
    ]
}

/// tan of an angle in [`exact_angles`] below 90°.
fn tan(degrees: f64) -> f64 {
    let (_, s, c) = exact_angles().into_iter().find(|a| a.0 == degrees).unwrap();
    s / c
}

/// How far a chamfer of `size` reaches along its reference face and along the other, on
/// an edge whose faces meet at θ (given by sin θ and cos θ), the material between them;
/// None when the faces and a chamfer face at that angle make no triangle. A
/// distance-and-angle's angle must be one of [`exact_angles`].
fn reaches(size: ChamferSize, sin_t: f64, cos_t: f64) -> Option<(f64, f64)> {
    match size {
        ChamferSize::Equal { distance } => Some((distance, distance)),
        ChamferSize::TwoDistances { distance, other } => Some((distance, other)),
        ChamferSize::DistanceAngle { distance, angle } => {
            // The triangle (edge, reference point, other point) has angles θ at the edge and
            // α at the reference point; by the law of sines the other side is
            // d sin α / sin(θ + α).
            let (_, s, c) = exact_angles().into_iter().find(|a| a.0 == angle).unwrap();
            let sin_sum = sin_t * c + cos_t * s;
            (sin_sum > 1e-9).then(|| (distance, distance * s / sin_sum))
        }
    }
}

/// The result's summary, after checking it's a valid solid with a well-formed history.
fn sound(what: &str, result: Result<Built, KernelError>) -> (Built, ShapeSummary) {
    let built = result.unwrap_or_else(|e| panic!("{what}: refused: {e} ({:?})", e.kind()));
    let s = summarize(&built.shape).unwrap();
    assert!(s.valid, "{what}: not valid: {s:?}");
    assert!(s.volume > 0.0, "{what}: volume {}", s.volume);
    assert_eq!(built.history.validate(1), Ok(()), "{what}");
    (built, s)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// How far `p` is from the line through `(x, y)` along z.
fn from_axis(p: [f64; 3], x: f64, y: f64) -> f64 {
    ((p[0] - x) * (p[0] - x) + (p[1] - y) * (p[1] - y)).sqrt()
}

/// Each edge's two ends, from the mesh (None for a degenerate edge, which has no points).
fn edge_ends(mesh: &silico_kernel::Mesh) -> Vec<Option<([f64; 3], [f64; 3])>> {
    mesh.edges()
        .map(|points| match points {
            [first, .., last] => Some((*first, *last)),
            _ => None,
        })
        .collect()
}

/// The edges (numbered from 1) with an end at `point`.
fn edges_ending_at(mesh: &silico_kernel::Mesh, point: [f64; 3]) -> Vec<u32> {
    edge_ends(mesh)
        .iter()
        .enumerate()
        .filter(|(_, ends)| {
            ends.is_some_and(|(a, b)| distance(a, point) < 1e-9 || distance(b, point) < 1e-9)
        })
        .map(|(i, _)| i as u32 + 1)
        .collect()
}

/// The number of the one face whose mesh points all satisfy `on`.
fn face_where(shape: &Shape, on: impl Fn([f64; 3]) -> bool) -> u32 {
    let mesh = tessellate(shape, 0.05).unwrap();
    let mut triangles = mesh.triangles.iter();
    let mut found = Vec::new();
    for (i, &count) in mesh.face_triangles.iter().enumerate() {
        let mut all = true;
        for t in triangles.by_ref().take(count as usize) {
            all &= t.iter().all(|&v| on(mesh.positions[v as usize]));
        }
        if all {
            found.push(i as u32 + 1);
        }
    }
    assert_eq!(found.len(), 1, "faces matching: {found:?}");
    found[0]
}

/// The points of face `face`'s mesh: on a flat face, its corners (and any points along its
/// edges), where the B-rep puts them. None when OCCT can't mesh the shape, as it can't a
/// face whose sides are 1e7 times its width (a box 1e-6 thick, or a box 1e-3 thick with
/// all but 1e-6 of a face chamfered away; the catalog's thin-solid entry).
fn face_points(shape: &Shape, face: u32) -> Option<Vec<[f64; 3]>> {
    let mesh = tessellate(shape, 1e9).ok()?;
    let mut triangles = mesh.triangles.iter();
    let mut points = Vec::new();
    for (i, &count) in mesh.face_triangles.iter().enumerate() {
        for t in triangles.by_ref().take(count as usize) {
            if i as u32 + 1 == face {
                points.extend(t.iter().map(|&v| mesh.positions[v as usize]));
            }
        }
    }
    Some(points)
}

/// A polygon's signed area, by the shoelace formula.
fn polygon_area(points: &[[f64; 2]]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

/// A 10 × 10 square whose bottom side dips `sag` at its middle, extruded 10; and the
/// number of the edge at the dip. The faces there meet at θ with sin θ = 10 sag / (25 +
/// sag²), turning by 2 atan(sag / 5).
fn kinked_prism(sag: f64) -> (Shape, u32) {
    let p = Profile {
        points: vec![
            [0.0, 0.0, 0.0],
            [5.0, -sag, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
        ],
    };
    let built = extrude(&p, 10.0, None).unwrap();
    let edge = built.history.inputs[0].vertices[1].generated[0]
        .element
        .index;
    (built.shape, edge)
}

// ---- Breaks ----

#[test]
fn an_edge_ending_where_two_solids_touch_at_a_corner_is_refused_not_a_crash() {
    // Two unit cubes touching only at (1, 1, 1), fused: two solids sharing that vertex.
    // Chamfering or filleting any of the six edges that end there (Edge8, 9, 11, 13, 16,
    // 17) segfaulted OCCT (FreeCAD 1.1.3's makeChamfer too, exit 139; it chamfers Edge7,
    // 1.995). Each joins two faces, so the first guard (an edge joining more than two
    // faces) let them through; now an edge ending at such a corner is never given to
    // OCCT. Wanted: the chamfer or fillet (a square edge 1 long: 2 − 0.1²/2, or
    // 2 − 0.1² (1 − π/4)), or a refusal before OCCT naming the edge, with a suggestion.
    let mut k = kernel();
    let a = k.make(unit_cube()).unwrap();
    let b = k.translate(&a, [1.0, 1.0, 1.0]).unwrap().shape;
    let touching = k.boolean(BooleanOp::Fuse, &a, &b).unwrap().shape;
    let at_corner = edges_ending_at(&k.tessellate(&touching, 0.1).unwrap(), [1.0; 3]);
    assert_eq!(at_corner, [8, 9, 11, 13, 16, 17]);
    let mut crashed = Vec::new();
    for &edge in &at_corner {
        for operation in ["chamfer", "fillet"] {
            let (result, removed) = if operation == "chamfer" {
                (k.chamfer(&touching, &[edge], equal(0.1), false), 0.005)
            } else {
                (k.fillet(&touching, &[edge], 0.1), 0.01 * (1.0 - PI / 4.0))
            };
            match result {
                Ok(built) => {
                    let s = k.summarize(&built.shape).unwrap();
                    assert!(s.valid, "{operation} of Edge{edge}");
                    assert_close(
                        &format!("{operation} of Edge{edge}"),
                        s.volume,
                        2.0 - removed,
                        1e-9,
                    );
                }
                Err(err) if err.code() == ErrorCode::WorkerCrashed => {
                    crashed.push(format!("{operation} of Edge{edge}: {err}"));
                    k = kernel();
                }
                Err(err) => {
                    assert_eq!(err.elements(), [format!("Edge{edge}")], "{err}");
                    assert!(err.suggestion().is_some(), "{err}");
                }
            }
        }
    }
    assert_eq!(crashed, Vec::<String>::new(), "the worker crashed");
    // Refused before OCCT, so safe in this process too: as invalid input, saying why.
    let a = make(&unit_cube()).unwrap();
    let b = translate(&a, [1.0, 1.0, 1.0]).unwrap().shape;
    let here = boolean(BooleanOp::Fuse, &a, &b).unwrap().shape;
    for &edge in &at_corner {
        for (operation, result) in [
            ("chamfer", chamfer(&here, &[edge], equal(0.1), false)),
            ("fillet", fillet(&here, &[edge], 0.1)),
        ] {
            let err = result.err().unwrap();
            assert_eq!(
                err.kind(),
                KernelErrorKind::InvalidInput,
                "{operation} Edge{edge}: {err}"
            );
            assert!(
                err.message()
                    .contains("touches itself or another solid at a point"),
                "{operation} Edge{edge}: {err}"
            );
        }
    }
    // And the worker goes on: Edge7 doesn't reach the corner.
    let built = k.chamfer(&touching, &[7], equal(0.1), false).unwrap();
    let after = k.summarize(&built.shape).unwrap();
    assert_close("Edge7", after.volume, 1.995, 1e-12);
}

#[test]
fn a_tangent_chain_that_reaches_a_shared_corner_is_refused_not_a_crash() {
    // OCCT chamfers the whole tangent chain the given edge starts, so the guard must look
    // at every corner of the chain, not only the edge's own. A 10 × 20 × 30 box with Edge1
    // filleted at r = 2 has a smooth chain round its top, from the edge along x (y = 0)
    // round the fillet's arc to the edge along y (x = 0), which ends at (0, 20, 30), where
    // a unit cube at (−1, 20, 30) touches the box at that one point. Chamfering or
    // filleting the edge along x, whose own corners are nowhere near it, segfaulted OCCT.
    // The refusal names the chain's edge that reaches the corner, and only the chain:
    // listed with a sound edge numbered below it or above it, the sound edge isn't named.
    let mut k = kernel();
    let a = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let a = k.fillet(&a, &[1], 2.0).unwrap().shape;
    let b = k.make(unit_cube()).unwrap();
    let b = k.translate(&b, [-1.0, 20.0, 30.0]).unwrap().shape;
    let fused = k.boolean(BooleanOp::Fuse, &a, &b).unwrap().shape;
    let mesh = k.tessellate(&fused, 0.05).unwrap();
    let along_x = edges_between(&mesh, [2.0, 0.0, 30.0], [10.0, 0.0, 30.0]);
    let along_y = edges_between(&mesh, [0.0, 2.0, 30.0], [0.0, 20.0, 30.0]);
    // Sound edges on the bottom, far from the chain and the cube: one numbered below the
    // edge along x and one above, found from the mesh, each chamfering alone.
    let bottom = [
        edges_between(&mesh, [2.0, 0.0, 0.0], [10.0, 0.0, 0.0]),
        edges_between(&mesh, [0.0, 2.0, 0.0], [0.0, 20.0, 0.0]),
        edges_between(&mesh, [10.0, 0.0, 0.0], [10.0, 20.0, 0.0]),
        edges_between(&mesh, [0.0, 20.0, 0.0], [10.0, 20.0, 0.0]),
    ];
    let below = bottom.iter().copied().filter(|&e| e < along_x).min();
    let above = bottom.iter().copied().filter(|&e| e > along_x).max();
    let (Some(below), Some(above)) = (below, above) else {
        panic!("no sound edge either side of Edge{along_x}: {bottom:?}");
    };
    for sound in [below, above] {
        assert!(
            k.chamfer(&fused, &[sound], equal(0.5), false).is_ok(),
            "Edge{sound}"
        );
    }
    let says = format!(
        "Edge{along_x} continues smoothly into Edge{along_y}, which ends at a corner where the shape touches itself or another solid at a point"
    );
    for edges in [vec![along_x], vec![below, along_x], vec![along_x, above]] {
        for operation in ["chamfer", "fillet"] {
            let result = if operation == "chamfer" {
                k.chamfer(&fused, &edges, equal(0.5), false)
            } else {
                k.fillet(&fused, &edges, 0.5)
            };
            let what = format!("{operation} {edges:?}");
            let err = result.err().unwrap_or_else(|| panic!("{what}: built"));
            assert_eq!(err.code(), ErrorCode::InvalidInput, "{what}: {err}");
            assert_eq!(err.elements(), [format!("Edge{along_x}")], "{what}");
            assert!(err.message().starts_with(&says), "{what}: {err}");
            for sound in [below, above] {
                assert!(
                    !err.message().contains(&format!("Edge{sound} "))
                        && !err.message().ends_with(&format!("Edge{sound}")),
                    "{what}: {err}"
                );
            }
            assert_eq!(
                err.suggestion(),
                Some(format!("leave Edge{along_x} out of the {operation}").as_str()),
                "{what}"
            );
        }
    }
    // The far edge is the chain's edge that reaches the corner wherever the chain starts:
    // with the cube at the other end of the chain, (10, −1, 30), the edge along y is
    // refused for the edge along x; and from the middle of the chain, the fillet's arc,
    // the far edge is the one at the corner, whichever end that is.
    let arc_ends = ([2.0, 0.0, 30.0], [0.0, 2.0, 30.0]);
    let x_ends = ([2.0, 0.0, 30.0], [10.0, 0.0, 30.0]);
    let y_ends = ([0.0, 2.0, 30.0], [0.0, 20.0, 30.0]);
    for (at, given, far) in [
        ([-1.0, 20.0, 30.0], x_ends, y_ends),
        ([10.0, -1.0, 30.0], y_ends, x_ends),
    ] {
        let b = k.make(unit_cube()).unwrap();
        let b = k.translate(&b, at).unwrap().shape;
        let fused = k.boolean(BooleanOp::Fuse, &a, &b).unwrap().shape;
        let mesh = k.tessellate(&fused, 0.05).unwrap();
        let far = edges_between(&mesh, far.0, far.1);
        for given in [given, arc_ends] {
            let given = edges_between(&mesh, given.0, given.1);
            let err = k
                .chamfer(&fused, &[given], equal(0.5), false)
                .err()
                .unwrap();
            assert!(
                err.message()
                    .starts_with(&format!("Edge{given} continues smoothly into Edge{far},")),
                "cube at {at:?}: {err}"
            );
        }
    }
    // The same chain without the cube chamfers.
    let alone = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let alone = k.fillet(&alone, &[1], 2.0).unwrap().shape;
    let edge = edges_between(
        &k.tessellate(&alone, 0.05).unwrap(),
        [2.0, 0.0, 30.0],
        [10.0, 0.0, 30.0],
    );
    assert!(k.chamfer(&alone, &[edge], equal(0.5), false).is_ok());
}

#[test]
fn the_edge_occt_throws_on_is_blamed_and_every_refused_edge_named() {
    // OCCT throws "only 2 faces" taking an edge that ends on one joining more than two
    // faces. Two boxes fused along the edge x = 10, y = 20: the top edge along x at y = 20
    // ends on it; listed with a sound edge numbered above it, it's that edge that's blamed,
    // not the last listed.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let two = boolean(
        BooleanOp::Fuse,
        &b,
        &translate(&b, [10.0, 20.0, 0.0]).unwrap().shape,
    )
    .unwrap()
    .shape;
    let mesh = tessellate(&two, 0.1).unwrap();
    let thrower = edges_between(&mesh, [0.0, 20.0, 30.0], [10.0, 20.0, 30.0]);
    let count = summarize(&two).unwrap().edges;
    let sound = (thrower + 1..=count)
        .find(|&e| chamfer(&two, &[e], equal(0.5), false).is_ok())
        .unwrap();
    for (operation, result) in [
        (
            "chamfer",
            chamfer(&two, &[thrower, sound], equal(0.5), false),
        ),
        ("fillet", fillet(&two, &[sound, thrower], 0.5)),
    ] {
        let err = result.err().unwrap();
        assert_eq!(err.kind(), KernelErrorKind::Failed, "{operation}: {err}");
        assert_eq!(
            err.element(),
            Some(format!("Edge{thrower}").as_str()),
            "{operation}: {err}"
        );
        assert_eq!(
            err.suggestion(),
            Some(format!(
                "Edge{thrower}, or an edge it continues smoothly into, ends where more than two faces meet (where solids touch along an edge), and OCCT can't {operation} it there: leave it out"
            ).as_str()),
            "{operation}: {err}"
        );
    }
    // A third box at (−10, 20, 0) makes a second edge joining four faces, x = 0, y = 20.
    // Listed with an edge OCCT throws on, both are refused, named in one refusal.
    let three = boolean(
        BooleanOp::Fuse,
        &two,
        &translate(&b, [-10.0, 20.0, 0.0]).unwrap().shape,
    )
    .unwrap()
    .shape;
    let mesh = tessellate(&three, 0.1).unwrap();
    let joins = [
        edges_between(&mesh, [10.0, 20.0, 0.0], [10.0, 20.0, 30.0]),
        edges_between(&mesh, [0.0, 20.0, 0.0], [0.0, 20.0, 30.0]),
    ];
    let thrower = edges_between(&mesh, [0.0, 20.0, 30.0], [10.0, 20.0, 30.0]);
    let (low, high) = (joins[0].min(joins[1]), joins[0].max(joins[1]));
    for (operation, result) in [
        (
            "chamfer",
            chamfer(&three, &[thrower, joins[0], joins[1]], equal(0.5), false),
        ),
        (
            "fillet",
            fillet(&three, &[joins[1], thrower, joins[0]], 0.5),
        ),
    ] {
        let err = result.err().unwrap();
        assert_eq!(
            err.kind(),
            KernelErrorKind::InvalidInput,
            "{operation}: {err}"
        );
        assert_eq!(err.element(), Some(format!("Edge{low}").as_str()));
        assert_eq!(
            err.suggestion(),
            Some(format!("leave Edge{low} and Edge{high} out of the {operation}").as_str()),
            "{operation}: {err}"
        );
    }
}

/// The edge from `p` to `q`, found from the mesh.
fn edges_between(mesh: &silico_kernel::Mesh, p: [f64; 3], q: [f64; 3]) -> u32 {
    edge_ends(mesh)
        .iter()
        .position(|e| {
            e.is_some_and(|(a, b)| {
                (distance(a, p) < 1e-6 && distance(b, q) < 1e-6)
                    || (distance(a, q) < 1e-6 && distance(b, p) < 1e-6)
            })
        })
        .map(|i| i as u32 + 1)
        .unwrap()
}

#[test]
fn every_edge_refused_is_named_in_one_refusal() {
    // One refusal names every edge at fault, so an agent following its suggestion needs
    // one call, not one per refused edge. Every side edge of a regular 64-gon
    // prism turns by 360°/64 = 5.6°, under OCCT's 0.1 radian, so all 64 are refused; the
    // rims are fine. One refusal must say to leave out all 64, and leaving them out builds.
    let n = 64;
    let points = (0..n)
        .map(|i| {
            let (s, c) = exact_polygon_corner(i, n);
            [10.0 * c, 10.0 * s, 0.0]
        })
        .collect();
    let shape = extrude(&Profile { points }, 5.0, None).unwrap().shape;
    let all: Vec<u32> = (1..=summarize(&shape).unwrap().edges).collect();
    for (operation, result) in [
        ("chamfer", chamfer(&shape, &all, equal(0.1), false)),
        ("fillet", fillet(&shape, &all, 0.1)),
    ] {
        let err = result.err().unwrap();
        assert_eq!(
            err.kind(),
            KernelErrorKind::InvalidInput,
            "{operation}: {err}"
        );
        let named: Vec<u32> = err
            .suggestion()
            .unwrap()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter_map(|w| w.strip_prefix("Edge")?.parse().ok())
            .collect();
        assert_eq!(named.len(), 64, "{operation}: {:?}", err.suggestion());
        let rest: Vec<u32> = all.iter().copied().filter(|e| !named.contains(e)).collect();
        assert_eq!(rest.len(), 128);
        let built = if operation == "chamfer" {
            chamfer(&shape, &rest, equal(0.1), false)
        } else {
            fillet(&shape, &rest, 0.1)
        };
        assert!(built.is_ok(), "{operation} without them");
    }
}

/// The sine and cosine of corner `i` of a regular `n`-gon, by doubling from exact values
/// (n a power of two), so no platform trigonometry is needed.
fn exact_polygon_corner(i: u32, n: u32) -> (f64, f64) {
    // cos and sin of 2π/n by halving the angle from a quarter turn: cos(θ/2) =
    // √((1 + cos θ)/2), sin(θ/2) = √((1 − cos θ)/2).
    let (mut s, mut c) = (1.0f64, 0.0f64);
    let mut m = 4;
    while m < n {
        (s, c) = (((1.0 - c) / 2.0).sqrt(), ((1.0 + c) / 2.0).sqrt());
        m *= 2;
    }
    // Rotate i times: (s_i, c_i) = (s_{i-1} c + c_{i-1} s, c_{i-1} c − s_{i-1} s).
    let (mut si, mut ci) = (0.0, 1.0);
    for _ in 0..i {
        (si, ci) = (si * c + ci * s, ci * c - si * s);
    }
    (si, ci)
}

#[test]
fn a_chamfer_that_fails_for_being_too_small_doesnt_say_to_make_it_smaller() {
    // Every OCCT failure of a one-edge chamfer got "try a smaller size" ("smaller
    // distances", "a smaller distance or angle"), including failures a smaller size can't
    // fix, so an agent that followed it never got there:
    // (a) The 10 × 20 × 30 box's Edge1 at distance 1 and an angle of 1e-9° (or the
    //     smallest subnormal, 5e-324°): the cut would reach 1 · tan(1e-9°) = 1.7e-11 along
    //     Face3, under OCCT's tolerance, and OCCT failed. Now the angle is refused.
    // (b) A quarter turn about y of the diamond (0, 0), (1, −1), (2, 0), (1, 1): its
    //     Edge6, from (1, −1, 0) to (2, 0, 0), chamfers at 0.1 and 0.01 and fails at
    //     0.001 (FreeCAD 1.1.3 the same).
    // (c) Its Edge3, from the apex on the axis to (1, −1, 0), fails at every size tried,
    //     0.1 to 0.001 (FreeCAD the same: "BRep_API: command not done").
    // OCCT doesn't say why it failed, so the suggestion mustn't point only one way: it
    // offers a smaller size, another size, or leaving the edge out.
    let says_smaller = |err: &KernelError| {
        err.suggestion().is_some_and(|s| {
            (s.contains("smaller") || s.contains("Smaller"))
                && !(s.contains("another") && s.contains("leave the edge out"))
        })
    };
    let refused = |err: &KernelError| {
        err.kind() == KernelErrorKind::InvalidInput && err.message().contains("angle")
    };
    let mut wrong = Vec::new();
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    sound("30°", chamfer(&b, &[1], angled(1.0, 30.0), false));
    for angle in [1e-9, 5e-324] {
        if let Err(err) = chamfer(&b, &[1], angled(1.0, angle), false)
            && (says_smaller(&err) || !refused(&err))
        {
            wrong.push(format!("(a) {angle}°: {err} {:?}", err.suggestion()));
        }
    }

    let diamond = Profile {
        points: vec![
            [0.0, 0.0, 0.0],
            [1.0, -1.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
        ],
    };
    let y = Axis {
        origin: [0.0; 3],
        direction: [0.0, 1.0, 0.0],
    };
    let quarter = revolve(&diamond, &y, 90.0).unwrap().shape;
    let ends = edge_ends(&tessellate(&quarter, 0.05).unwrap());
    let (s6, e6) = ends[5].unwrap();
    assert!(distance(s6, [1.0, -1.0, 0.0]) < 1e-9 && distance(e6, [2.0, 0.0, 0.0]) < 1e-9);
    let (s3, e3) = ends[2].unwrap();
    assert!(distance(s3, [0.0; 3]) < 1e-9 && distance(e3, [1.0, -1.0, 0.0]) < 1e-9);
    sound("(b) 0.01", chamfer(&quarter, &[6], equal(0.01), false));
    if let Err(err) = chamfer(&quarter, &[6], equal(0.001), false)
        && says_smaller(&err)
    {
        wrong.push(format!(
            "(b) 0.001, where 0.01 works: {:?}",
            err.suggestion()
        ));
    }
    let failures: Vec<KernelError> = [0.1, 0.01, 0.001]
        .into_iter()
        .filter_map(|d| chamfer(&quarter, &[3], equal(d), false).err())
        .collect();
    if failures.len() == 3 && failures.iter().any(says_smaller) {
        wrong.push(format!(
            "(c) fails at 0.1, 0.01, and 0.001: {:?}",
            failures[0].suggestion()
        ));
    }
    assert_eq!(wrong, Vec::<String>::new());
}

#[test]
fn an_edge_of_a_shape_with_no_edges_is_refused_saying_so() {
    // A shape with nothing in it (an empty compound, as an operation that made nothing
    // hands on) got "Edge1 doesn't exist: the shape has edges 1 to 0", for the chamfer and
    // the fillet: a range that doesn't exist. Now it says the shape has no edges.
    let empty = silico_kernel::test_ops::empty_shape().unwrap();
    for (operation, result) in [
        ("chamfer", chamfer(&empty, &[1], equal(1.0), false)),
        ("fillet", fillet(&empty, &[1], 1.0)),
    ] {
        let err = result.err().unwrap();
        assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{operation}");
        assert!(
            !err.message().contains("edges 1 to 0"),
            "{operation}: {}",
            err.message()
        );
    }
}

#[test]
fn a_corner_of_a_few_degrees_isnt_called_smooth() {
    // OCCT takes no edge whose faces' normals differ by less than 0.1 rad (5.7°: the G1
    // tolerance ChFi3d::IsTangentFaces passes to LocalAnalysis_SurfaceContinuity, OCCT's
    // src/ChFi3d/ChFi3d.cxx). A prism whose side turns by 3° (a dip of 0.13) or 5.70°
    // (0.249) at an edge has a visible corner there, as has every side edge of a regular
    // 100-gon prism (3.6°); the refusal said the faces "meet smoothly", and now says they
    // turn by less than OCCT's 0.1 radian (FreeCAD says only "There are no suitable
    // edges").
    for sag in [0.13, 0.249] {
        let (prism, edge) = kinked_prism(sag);
        for (operation, result) in [
            ("chamfer", chamfer(&prism, &[edge], equal(1.0), false)),
            ("fillet", fillet(&prism, &[edge], 1.0)),
        ] {
            let err = result.err().unwrap();
            assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{sag}: {err}");
            assert!(
                !err.message().contains("meet smoothly"),
                "{operation} of the corner a dip of {sag} makes: {}",
                err.message()
            );
        }
    }
}

// ---- Survived: geometry ----

#[test]
fn a_corner_just_over_occts_tangency_limit_chamfers_exactly() {
    // A dip of 0.251 turns the side by 0.1003 rad, just over OCCT's limit: a chamfer of 1
    // along each face removes the triangle with those sides at θ, sin θ / 2, over the
    // height 10.
    for sag in [0.251, 0.26, 0.5, 2.5] {
        let (prism, edge) = kinked_prism(sag);
        let before = summarize(&prism).unwrap().volume;
        let (_, s) = sound(
            &format!("{sag}"),
            chamfer(&prism, &[edge], equal(1.0), false),
        );
        let sin_t = 10.0 * sag / (25.0 + sag * sag);
        assert_close(&format!("{sag}"), before - s.volume, 5.0 * sin_t, 1e-9);
    }
}

#[test]
fn a_concave_edge_gets_the_triangle_added() {
    // An L (a 10 square less its 5 × 5 corner) extruded 10. The inner corner's edge is
    // concave, its faces at 90° outside the material: a chamfer fills the triangle with
    // reaches r1 (along the lower-numbered face) and r2, adding r1 r2 / 2 · 10.
    let l = Profile {
        points: vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 5.0, 0.0],
            [5.0, 5.0, 0.0],
            [5.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
        ],
    };
    let built = extrude(&l, 10.0, None).unwrap();
    let h = &built.history.inputs[0];
    // The corner (5, 5) is the profile's fourth point, between its third side (along
    // y = 5, the face toward +x) and its fourth (along x = 5, toward +y).
    let edge = h.vertices[3].generated[0].element.index;
    let toward_x = h.edges[2].generated[0].element.index;
    let toward_y = h.edges[3].generated[0].element.index;
    let base = summarize(&built.shape).unwrap();
    let (a, cx, cy) = (75.0, base.center[0], base.center[1]);
    // `distance` on the lower-numbered face: (reach along +x, reach along +y).
    let first_x = toward_x < toward_y;
    let reaches = |r_ref: f64, r_other: f64| {
        if first_x {
            (r_ref, r_other)
        } else {
            (r_other, r_ref)
        }
    };
    let cases = [
        (equal(1.0), false, (1.0, 1.0)),
        (two(1.0, 3.0), false, reaches(1.0, 3.0)),
        (two(1.0, 3.0), true, reaches(3.0, 1.0)),
        (angled(1.0, 30.0), false, reaches(1.0, tan(30.0))),
        (angled(1.0, 30.0), true, reaches(tan(30.0), 1.0)),
        (angled(2.0, 60.0), false, reaches(2.0, 2.0 * tan(60.0))),
    ];
    for (size, flip, (rx, ry)) in cases {
        let what = format!("{size:?} flip {flip}");
        let (_, s) = sound(&what, chamfer(&built.shape, &[edge], size, flip));
        let added = rx * ry / 2.0;
        assert_close(&what, s.volume, 750.0 + 10.0 * added, 1e-12);
        // The added triangle (5, 5), (5 + rx, 5), (5, 5 + ry), centroid a third along each.
        let want = [
            (a * cx + added * (5.0 + rx / 3.0)) / (a + added),
            (a * cy + added * (5.0 + ry / 3.0)) / (a + added),
        ];
        for (axis, want) in want.into_iter().enumerate() {
            assert!(
                (s.center[axis] - want).abs() <= 1e-9,
                "{what}: center {axis}: {} vs {want}",
                s.center[axis]
            );
        }
    }
}

#[test]
fn a_hole_rim_chamfers_into_a_countersink() {
    // A 20 × 20 × 10 block less a radius-3 cylinder through it at (10, 10). The top rim's
    // chamfer removes the triangle (3, 10), (3 + a, 10), (3, 10 − b) turned about the hole's
    // axis: by Pappus, 2π (3 + a/3) a b / 2, with a along the top face, b down the hole.
    let block = make_box(20.0, 20.0, 10.0).unwrap();
    let hole = translate(
        &make(&Primitive::Cylinder {
            radius: 3.0,
            height: 10.0,
        })
        .unwrap(),
        [10.0, 10.0, 0.0],
    )
    .unwrap()
    .shape;
    let holed = boolean(BooleanOp::Cut, &block, &hole).unwrap().shape;
    let before = summarize(&holed).unwrap().volume;
    let mesh = tessellate(&holed, 0.01).unwrap();
    let rim = edge_ends(&mesh)
        .iter()
        .position(|e| e.is_some_and(|(a, b)| distance(a, b) < 1e-9 && (a[2] - 10.0).abs() < 1e-9))
        .unwrap() as u32
        + 1;
    let top = face_where(&holed, |p| (p[2] - 10.0).abs() < 1e-9);
    let wall = face_where(&holed, |p| (from_axis(p, 10.0, 10.0) - 3.0).abs() < 1e-6);
    let top_first = top < wall;
    let cases = [
        (two(1.0, 2.0), false, (1.0, 2.0)),
        (two(1.0, 2.0), true, (1.0, 2.0)),
        (angled(1.0, 60.0), false, (1.0, tan(60.0))),
        (angled(1.0, 60.0), true, (1.0, tan(60.0))),
        (equal(2.5), false, (2.5, 2.5)),
    ];
    for (size, flip, (r_ref, r_other)) in cases {
        let (a, b) = if top_first != flip {
            (r_ref, r_other)
        } else {
            (r_other, r_ref)
        };
        let what = format!("Edge{rim} {size:?} flip {flip}");
        let (_, s) = sound(&what, chamfer(&holed, &[rim], size, flip));
        let removed = TAU * (3.0 + a / 3.0) * a * b / 2.0;
        assert_close(&what, before - s.volume, removed, 1e-8);
    }
}

#[test]
fn a_distance_and_angle_of_exactly_90_degrees_on_an_acute_corner() {
    // A 30° wedge (the triangle (0, 0), (10, 0), (5√3, 5) extruded 10), its sharp edge
    // chamfered at d = 1 with the chamfer face square to the reference face: the cut
    // reaches d / cos 30° along the other face, removing 1 · (2/√3) · sin 30° / 2 · 10.
    // At 89.99° and 90.01° OCCT builds it either way round; at exactly 90° measured from
    // the lower-numbered face (no flip) its solid is invalid (FreeCAD 1.1.3 returns it:
    // volume 247.1132, isValid False), and Silico refuses it, naming the edge, as it
    // refuses every invalid result (docs/ports/chamfer.md). The flipped one is valid.
    let r3 = 3f64.sqrt();
    let wedge = Profile {
        points: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [5.0 * r3, 5.0, 0.0]],
    };
    let built = extrude(&wedge, 10.0, None).unwrap();
    let edge = built.history.inputs[0].vertices[0].generated[0]
        .element
        .index;
    let want = 250.0 - 10.0 * (2.0 / r3) * 0.5 / 2.0;
    let (_, s) = sound(
        "flipped",
        chamfer(&built.shape, &[edge], angled(1.0, 90.0), true),
    );
    assert_close("flipped", s.volume, want, 1e-12);
    match chamfer(&built.shape, &[edge], angled(1.0, 90.0), false) {
        Ok(chamfered) => {
            let s = summarize(&chamfered.shape).unwrap();
            assert!(s.valid);
            assert_close("not flipped", s.volume, want, 1e-12);
        }
        Err(err) => {
            assert_eq!(err.kind(), KernelErrorKind::InvalidResult, "{err}");
            assert_eq!(err.element(), Some(format!("Edge{edge}").as_str()));
        }
    }
}

#[test]
fn a_cylinders_rim_at_a_distance_and_angle_is_a_cone() {
    // The r = 5, h = 10 cylinder's bottom rim (Edge2): `distance` d down its side (Face1)
    // and d tan α along the bottom, or the other way flipped. By Pappus the triangle
    // (5, 0), (5 − a, 0), (5, b) removes 2π (5 − a/3) a b / 2. Reaching the axis exactly
    // (a = 5) leaves a cone's tip, the bottom face gone.
    let cylinder = make(&Primitive::Cylinder {
        radius: 5.0,
        height: 10.0,
    })
    .unwrap();
    let full = 250.0 * PI;
    // (size, flip, d, the other reach)
    let cases = [
        (angled(1.0, 30.0), false, 1.0, tan(30.0)),
        (angled(1.0, 30.0), true, 1.0, tan(30.0)),
        (angled(2.0, 60.0), false, 2.0, 2.0 * tan(60.0)),
        (angled(4.0, 67.5), true, 4.0, 4.0 * tan(67.5)),
        (angled(3.0, 15.0), false, 3.0, 3.0 * tan(15.0)),
        (angled(5.0, 45.0), false, 5.0, 5.0),
        (equal(5.0), false, 5.0, 5.0),
    ];
    for (size, flip, d, other) in cases {
        let (b, a) = if flip { (other, d) } else { (d, other) };
        let what = format!("{size:?} flip {flip}");
        let (_, s) = sound(&what, chamfer(&cylinder, &[2], size, flip));
        assert_close(
            &what,
            s.volume,
            full - TAU * (5.0 - a / 3.0) * a * b / 2.0,
            1e-9,
        );
    }
    // Past the axis: no chamfer.
    for size in [equal(5.0000001), two(1.0, 5.0000001), angled(1.0, 80.0)] {
        let err = chamfer(&cylinder, &[2], size, false).err().unwrap();
        assert_eq!(err.element(), Some("Edge2"), "{size:?}: {err}");
    }
}

#[test]
fn a_chamfer_of_a_chamfers_own_edges() {
    // The 10 × 20 × 30 box with Edge1 cut at 2 (its chamfer face on x + y = 2). The edges
    // where the chamfer face meets Face1 and Face3 are 30 long with the faces at 135°: a
    // second chamfer of 0.5 removes 30 · 0.5² sin 135° / 2. The edges where it meets the
    // end caps are 2√2 long at 90°, and the cut's ends are slanted by Face1 and Face3: at
    // depth u into the chamfer face the cut is 2√2 + 2u long and ½ − u high, so it removes
    // ∫₀^½ (½ − u)(2√2 + 2u) du = √2/4 + 1/24.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let first = chamfer(&b, &[1], equal(2.0), false).unwrap().shape;
    let mesh = tessellate(&first, 0.05).unwrap();
    let (mut long, mut short) = (Vec::new(), Vec::new());
    for (i, ends) in edge_ends(&mesh).iter().enumerate() {
        let (p, q) = ends.unwrap();
        let on_chamfer = |v: [f64; 3]| (v[0] + v[1] - 2.0).abs() < 1e-9;
        if on_chamfer(p) && on_chamfer(q) {
            if (p[2] - q[2]).abs() > 1.0 {
                long.push(i as u32 + 1);
            } else {
                short.push(i as u32 + 1);
            }
        }
    }
    assert_eq!((long.len(), short.len()), (2, 2), "{long:?} {short:?}");
    let r2 = 2f64.sqrt();
    for (edges, removed) in [
        (long, 30.0 * 0.25 * (r2 / 2.0) / 2.0),
        (short, r2 / 4.0 + 1.0 / 24.0),
    ] {
        for edge in edges {
            let what = format!("Edge{edge}");
            let (_, s) = sound(&what, chamfer(&first, &[edge], equal(0.5), false));
            assert_close(&what, s.volume, 5940.0 - removed, 1e-12);
        }
    }
}

#[test]
fn two_edges_meeting_at_a_corner_up_to_the_face_width_match_the_formula() {
    // FreeCAD's testOther formula, L c²/2 + H c²/2 − c³/3, holds until c reaches the
    // 10-wide face both edges share.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    for c in [9.0, 9.9, 9.999] {
        let (_, s) = sound(&format!("{c}"), chamfer(&b, &[1, 2], equal(c), false));
        let removed = 30.0 * c * c / 2.0 + 20.0 * c * c / 2.0 - c * c * c / 3.0;
        assert_close(&format!("{c}"), s.volume, 6000.0 - removed, 1e-12);
    }
}

/// A box's volume with every edge chamfered by c: each edge loses a prism c²/2 in
/// section along its length less 2c, each corner 5c³/6 more (FreeCAD's octahedron test).
fn box_every_edge(dims: [f64; 3], c: f64) -> f64 {
    let [x, y, z] = dims;
    let lengths = 4.0 * ((x - 2.0 * c) + (y - 2.0 * c) + (z - 2.0 * c));
    x * y * z - c * c / 2.0 * lengths - 8.0 * 5.0 / 6.0 * c * c * c
}

#[test]
fn every_edge_at_and_past_half_the_narrowest_face() {
    // Clear of half the narrowest face (leaving strips 0.02 and 2e-4 wide), every edge
    // chamfers by the formula, whatever the order the edges are listed in. (Nearer, at
    // 4.999999, OCCT builds the 10 × 20 × 30 box's listed 1 to 12 and fails it listed 12 to
    // 1, as FreeCAD 1.1.3's makeChamfer and PartDesign Chamfer do.) At half or more the
    // chamfers would cross and OCCT fails, cleanly, blaming none of the twelve.
    let all: Vec<u32> = (1..=12).collect();
    let backwards: Vec<u32> = (1..=12).rev().collect();
    let mixed = [7, 2, 11, 5, 1, 12, 3, 9, 6, 10, 4, 8];
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    // The edges are a set: given to OCCT in increasing order, so at 4.999999 every order
    // builds, the same bytes and history as listed 1 to 12.
    let first = chamfer(&b, &all, equal(4.999999), false).unwrap();
    for order in [&backwards[..], &mixed] {
        let again = chamfer(&b, order, equal(4.999999), false).unwrap();
        assert!(
            to_brep(&again.shape).unwrap() == to_brep(&first.shape).unwrap(),
            "{order:?}"
        );
        assert_eq!(again.history, first.history, "{order:?}");
        let again = fillet(&b, order, 4.999999).map(|r| to_brep(&r.shape).unwrap());
        let first = fillet(&b, &all, 4.999999).map(|r| to_brep(&r.shape).unwrap());
        assert_eq!(again.ok(), first.ok(), "fillet {order:?}");
    }
    for c in [4.99, 4.9999] {
        let want = box_every_edge([10.0, 20.0, 30.0], c);
        for order in [&all[..], &backwards, &mixed] {
            let (_, s) = sound(&format!("{c}"), chamfer(&b, order, equal(c), false));
            assert_close(&format!("{c} {order:?}"), s.volume, want, 1e-9);
            assert_eq!(s.faces, 26);
        }
    }
    let cube = make_box(10.0, 10.0, 10.0).unwrap();
    for c in [5.0, 5.0 + 1e-9, 5.000001, 5.5, 7.5, 9.99] {
        for shape in [&cube, &b] {
            let err = chamfer(shape, &all, equal(c), false).err().unwrap();
            assert!(
                matches!(
                    err.kind(),
                    KernelErrorKind::Failed | KernelErrorKind::InvalidResult
                ),
                "{c}: {err}"
            );
            assert_eq!(err.element(), None, "{c}");
            assert!(err.suggestion().unwrap().contains("one at a time"), "{c}");
        }
    }
}

#[test]
fn sizes_at_the_tolerance_and_at_a_kilometre() {
    // Just above OCCT's tolerance (the smallest distance allowed), on a box and every edge
    // of it; at a kilometre's scale, half and nearly all of the face.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let all: Vec<u32> = (1..=12).collect();
    for d in [1e-7f64.next_up(), 1.0000001e-7, 2e-7] {
        let (_, s) = sound(&format!("{d}"), chamfer(&b, &[1], equal(d), false));
        assert_eq!((s.faces, s.edges, s.vertices), (7, 15, 10), "{d}");
        let (_, s) = sound(&format!("{d} all"), chamfer(&b, &all, equal(d), false));
        assert_eq!((s.faces, s.edges, s.vertices), (26, 48, 24), "{d} all");
        let (_, s) = sound(&format!("{d}, 5"), chamfer(&b, &[1], two(d, 5.0), false));
        assert_close(&format!("{d}, 5"), s.volume, 6000.0 - 15.0 * d * 5.0, 1e-12);
    }
    let km = make_box(1e6, 1e6, 1e6).unwrap();
    for d in [5e5, 999_999.0] {
        let (_, s) = sound(&format!("{d}"), chamfer(&km, &[1], equal(d), false));
        assert_close(&format!("{d}"), s.volume, 1e18 - 1e6 * d * d / 2.0, 1e-12);
    }
    let (_, s) = sound(
        "1e6 at 1.0000001e-7",
        chamfer(&km, &[1], equal(1.0000001e-7), false),
    );
    assert_eq!((s.faces, s.edges, s.vertices), (7, 15, 10));
}

#[test]
fn a_chamfer_a_kilometre_from_the_origin_is_as_wide_as_asked() {
    // The box moved a kilometre each way: the chamfer face of d on Edge1 is 30 by d√2,
    // measured from the mesh (exact for a flat face).
    let far = translate(&make_box(10.0, 20.0, 30.0).unwrap(), [1e6; 3])
        .unwrap()
        .shape;
    for d in [1e-3, 1e-5] {
        let (built, s) = sound(&format!("{d}"), chamfer(&far, &[1], equal(d), false));
        assert_eq!((s.faces, s.edges, s.vertices), (7, 15, 10));
        let face = built.history.inputs[0].edges[0].generated[0].element.index;
        let mesh = tessellate(&built.shape, 1.0).unwrap();
        let mut triangles = mesh.triangles.iter();
        let mut area = 0.0;
        for (i, &count) in mesh.face_triangles.iter().enumerate() {
            for t in triangles.by_ref().take(count as usize) {
                if i as u32 + 1 != face {
                    continue;
                }
                let [p, q, r] = t.map(|v| mesh.positions[v as usize]);
                let (u, v) = (
                    [q[0] - p[0], q[1] - p[1], q[2] - p[2]],
                    [r[0] - p[0], r[1] - p[1], r[2] - p[2]],
                );
                let n = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                area += (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0;
            }
        }
        // The mesh's points are a kilometre out, so each difference is good to about
        // 1e-10 absolute: relative to d, 1e-9 / d.
        assert_close(&format!("{d}"), area, 30.0 * d * 2f64.sqrt(), 1e-9 / d);
    }
}

#[test]
fn a_chamfer_along_a_tangent_chain_follows_it_from_the_edge_given() {
    // A 10 × 20 × 30 box with Edge1 filleted at r = 2. Its top outline runs smoothly from
    // the straight edge on x = 0 (18 long) round the fillet's quarter arc (center (2, 2))
    // to the straight edge on y = 0 (8 long): one contour, which a chamfer of either
    // straight edge or the arc follows all the way. At two(0.5, 1), the face `distance`
    // goes on is the given edge's lower-numbered face, and the whole chain follows it.
    // With 0.5 down the sides and 1 in along the top: 0.25 over the 26 straight, and on
    // the arc the triangle (2, 30), (2, 29.5), (1, 30) a quarter turn about the fillet's
    // axis: (π/2)(5/3)(0.25). Swapped: (π/2)(11/6)(0.25).
    let filleted = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0)
        .unwrap()
        .shape;
    let before = summarize(&filleted).unwrap().volume;
    let mesh = tessellate(&filleted, 0.01).unwrap();
    let ends = edge_ends(&mesh);
    let find = |a: [f64; 3], b: [f64; 3]| {
        ends.iter()
            .position(|e| {
                e.is_some_and(|(p, q)| {
                    (distance(p, a) < 1e-9 && distance(q, b) < 1e-9)
                        || (distance(p, b) < 1e-9 && distance(q, a) < 1e-9)
                })
            })
            .unwrap() as u32
            + 1
    };
    let side = find([0.0, 2.0, 30.0], [0.0, 20.0, 30.0]);
    let arc = find([0.0, 2.0, 30.0], [2.0, 0.0, 30.0]);
    let top = face_where(&filleted, |p| (p[2] - 30.0).abs() < 1e-9);
    let x0 = face_where(&filleted, |p| p[0].abs() < 1e-9);
    let round = face_where(&filleted, |p| (from_axis(p, 2.0, 2.0) - 2.0).abs() < 1e-6);
    let down_the_sides = 6.5 + FRAC_PI_2 * (5.0 / 3.0) * 0.25;
    let in_along_the_top = 6.5 + FRAC_PI_2 * (11.0 / 6.0) * 0.25;
    // Listed together, either way round, the lower-numbered of the two decides.
    let (lower, sides_first) = if side < arc {
        (side, x0 < top)
    } else {
        (arc, round < top)
    };
    for edges in [[side, arc], [arc, side]] {
        let what = format!("{edges:?}");
        let (built, s) = sound(&what, chamfer(&filleted, &edges, two(0.5, 1.0), false));
        let want = if sides_first {
            down_the_sides
        } else {
            in_along_the_top
        };
        assert_close(&what, before - s.volume, want, 1e-9);
        let alone = chamfer(&filleted, &[lower], two(0.5, 1.0), false).unwrap();
        assert!(
            to_brep(&built.shape).unwrap() == to_brep(&alone.shape).unwrap(),
            "{what}"
        );
    }
    for (edge, sides_first) in [(side, x0 < top), (arc, round < top)] {
        for flip in [false, true] {
            let what = format!("Edge{edge} flip {flip}");
            let (_, s) = sound(&what, chamfer(&filleted, &[edge], two(0.5, 1.0), flip));
            let want = if sides_first != flip {
                down_the_sides
            } else {
                in_along_the_top
            };
            assert_close(&what, before - s.volume, want, 1e-9);
            assert_eq!(s.faces, 10, "{what}");
        }
    }
}

// ---- Survived: edges OCCT won't take, and other shapes ----

#[test]
fn every_curved_primitives_seams_and_poles_are_refused_naming_them() {
    let torus = make(&Primitive::Torus {
        major_radius: 5.0,
        minor_radius: 1.0,
    })
    .unwrap();
    let sphere = make(&Primitive::Sphere { radius: 5.0 }).unwrap();
    let cases: [(&str, &Shape, &[u32], &str, &str); 3] = [
        ("torus", &torus, &[1, 2], "Edge1", "Edge2"),
        ("sphere", &sphere, &[1, 2, 3], "Edge1", "Edge2, Edge3"),
        // Edges are a set: listed seam first, they're still named in order.
        ("sphere seam first", &sphere, &[2, 1], "Edge1", "Edge2"),
    ];
    for (what, shape, edges, first, others) in cases {
        for (operation, result) in [
            ("chamfer", chamfer(shape, edges, equal(0.1), false)),
            ("fillet", fillet(shape, edges, 0.1)),
        ] {
            let err = result.err().unwrap();
            assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{what}: {err}");
            assert_eq!(err.element(), Some(first), "{what}: {err}");
            assert!(
                err.message()
                    .ends_with(&format!("; OCCT won't {operation} {others} either")),
                "{what}: {err}"
            );
        }
    }
}

#[test]
fn edges_of_two_separate_solids_in_one_shape_chamfer_together() {
    // Two unit cubes 4 apart, fused into one shape of two solids: an edge of each, each
    // losing 0.1²/2; then every edge of both.
    let a = make(&unit_cube()).unwrap();
    let b = translate(&a, [5.0, 0.0, 0.0]).unwrap().shape;
    let apart = boolean(BooleanOp::Fuse, &a, &b).unwrap().shape;
    assert_eq!(summarize(&apart).unwrap().edges, 24);
    let (_, s) = sound("1 and 13", chamfer(&apart, &[1, 13], equal(0.1), false));
    assert_close("1 and 13", s.volume, 2.0 - 2.0 * 0.005, 1e-12);
    let every: Vec<u32> = (1..=24).collect();
    let (_, s) = sound("all", chamfer(&apart, &every, equal(0.1), false));
    assert_close("all", s.volume, 2.0 * box_every_edge([1.0; 3], 0.1), 1e-12);
}

#[test]
fn every_edge_of_two_boxes_sharing_an_edge_chamfers_or_is_refused_never_crashes() {
    // Two 10 × 20 × 30 boxes fused along the edge x = 10, y = 20: every edge, through a
    // worker. Each is a square edge: its chamfer at 0.5 removes 0.125 per unit length. The
    // shared edge, and those ending on it, are refused or fail naming the edge.
    let mut k = kernel();
    let a = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let b = k.translate(&a, [10.0, 20.0, 0.0]).unwrap().shape;
    let fused = k.boolean(BooleanOp::Fuse, &a, &b).unwrap().shape;
    let mesh = k.tessellate(&fused, 0.1).unwrap();
    let (mut built, mut refused) = (0, 0);
    for (i, ends) in edge_ends(&mesh).iter().enumerate() {
        let edge = i as u32 + 1;
        let (p, q) = ends.unwrap();
        match k.chamfer(&fused, &[edge], equal(0.5), false) {
            Ok(result) => {
                let s = k.summarize(&result.shape).unwrap();
                assert!(s.valid, "Edge{edge}");
                assert_close(
                    &format!("Edge{edge}"),
                    s.volume,
                    12000.0 - 0.125 * distance(p, q),
                    1e-12,
                );
                built += 1;
            }
            Err(err) => {
                assert_ne!(err.code(), ErrorCode::WorkerCrashed, "Edge{edge}: {err}");
                assert_eq!(err.elements(), [format!("Edge{edge}")], "{err}");
                assert!(err.suggestion().is_some(), "{err}");
                refused += 1;
            }
        }
    }
    assert_eq!((built, refused), (14, 9));
}

// ---- Survived: requests ----

#[test]
fn a_million_edges_is_refused_quickly_and_the_worker_goes_on() {
    let mut k = kernel();
    let b = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let start = Instant::now();
    let repeated = vec![1u32; 1_000_000];
    let err = k.chamfer(&b, &repeated, equal(1.0), false).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert_eq!(err.elements(), ["Edge1"]);
    let counting: Vec<u32> = (1..=1_000_000).collect();
    // An edge the box doesn't have isn't one of its elements: named by its number, the
    // edges at fault.
    let err = k.chamfer(&b, &counting, equal(1.0), false).unwrap_err();
    assert!(err.elements().is_empty(), "{err}");
    assert_eq!(err.parameter(), Some("edges"), "{err}");
    assert_eq!(
        err.message(),
        "edge 13 doesn't exist: the shape has edges 1 to 12"
    );
    let err = k.chamfer(&b, &[u32::MAX], equal(1.0), false).unwrap_err();
    assert!(err.elements().is_empty(), "{err}");
    assert_eq!(
        err.message(),
        "edge 4294967295 doesn't exist: the shape has edges 1 to 12"
    );
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
    let after = k.chamfer(&b, &[1], equal(2.0), false).unwrap().shape;
    let s = k.summarize(&after).unwrap();
    assert_close("after", s.volume, 5940.0, 1e-12);
}

#[test]
fn the_same_chamfer_twice_is_the_same_bytes() {
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let cylinder = make(&Primitive::Cylinder {
        radius: 5.0,
        height: 10.0,
    })
    .unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let cases: [(&Shape, &[u32], ChamferSize, bool); 4] = [
        (&b, &[1, 2, 9], angled(1.5, 35.0), true),
        (&b, &[3, 5, 7, 11], two(0.7, 2.1), false),
        (&cylinder, &[2, 3], angled(1.0, 20.0), true),
        (&b, &all, equal(4.99), false),
    ];
    for (shape, edges, size, flip) in cases {
        let first = chamfer(shape, edges, size, flip).unwrap();
        let again = chamfer(shape, edges, size, flip).unwrap();
        assert!(
            to_brep(&first.shape).unwrap() == to_brep(&again.shape).unwrap(),
            "{edges:?} {size:?}"
        );
        assert_eq!(first.history, again.history, "{edges:?} {size:?}");
    }
}

// ---- Property tests ----

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        // Fixed, so a red run is red every time.
        rng_seed: proptest::test_runner::RngSeed::Fixed(0xc4a3_f0e5),
        ..ProptestConfig::default()
    }
}

/// A length biased toward the sizes the rule turns on: the tolerance and a ULP either
/// side, a kilometre and past it, zeros, and values that aren't numbers.
fn hostile_length() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(1e-7),
        Just(1e-7f64.next_up()),
        Just(1e-7f64.next_down()),
        Just(1e6),
        Just(1e6f64.next_up()),
        Just(0.0),
        Just(-0.0),
        Just(f64::NAN),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
        Just(f64::MIN_POSITIVE),
        Just(5e-324),
        Just(f64::MAX),
        -10.0..10.0f64,
        0.1..15.0f64,
    ]
}

fn hostile_angle() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(180.0),
        Just(180f64.next_down()),
        Just(5e-324),
        Just(90.0),
        Just(f64::NAN),
        Just(f64::INFINITY),
        -360.0..360.0f64,
        1.0..89.0f64,
    ]
}

fn length_allowed(v: f64) -> bool {
    v > 1e-7 && v <= 1e6
}

/// A box's faces (the axis each is square to, and where along it) and edges (the axis
/// each runs along, and its ends), from the mesh.
type Layout = (Vec<(usize, f64)>, Vec<(usize, [f64; 3])>);

fn box_layout(shape: &Shape) -> Layout {
    let mesh = tessellate(shape, 1e9).unwrap();
    let mut triangles = mesh.triangles.iter();
    let faces = mesh
        .face_triangles
        .iter()
        .map(|&count| {
            let tris: Vec<_> = triangles.by_ref().take(count as usize).collect();
            let p = tris[0].map(|v| mesh.positions[v as usize]);
            let axis = (0..3)
                .find(|&i| p[0][i] == p[1][i] && p[1][i] == p[2][i])
                .unwrap();
            (axis, p[0][axis])
        })
        .collect();
    let edges = edge_ends(&mesh)
        .into_iter()
        .map(|ends| {
            let (a, b) = ends.unwrap();
            let axis = (0..3).find(|&i| a[i] != b[i]).unwrap();
            (axis, a)
        })
        .collect();
    (faces, edges)
}

fn fraction() -> impl Strategy<Value = f64> {
    // Fractions of the room on a face: mostly inside, some at the edges of fitting, some
    // well past.
    prop_oneof![
        4 => 0.01..0.98f64,
        1 => 1e-4..1e-2f64,
        1 => Just(0.999),
        1 => 1.02..3.0f64,
    ]
}

proptest! {
    #![proptest_config(config())]

    /// A chamfer of any box edge, by any kind and either way round, cuts the triangle its
    /// reaches span, `distance` on the edge's lower-numbered face; or, when a reach doesn't
    /// fit its face (or the angle leaves no triangle), fails naming the edge. Twice gives
    /// the same bytes.
    #[test]
    fn box_chamfers_cut_the_triangle_the_sizes_say(
        dims in proptest::array::uniform3(prop_oneof![
            0.5..2.0f64, 5.0..50.0f64, 100.0..2000.0f64, Just(1e-3), Just(1e4)
        ]),
        edge in 1u32..=12,
        kind in 0u8..3,
        f_ref in fraction(),
        f_other in fraction(),
        angle in 0usize..14,
        flip in any::<bool>(),
    ) {
        let shape = make_box(dims[0], dims[1], dims[2]).unwrap();
        let (faces, edges) = box_layout(&shape);
        let (axis, start) = edges[edge as usize - 1];
        // The edge's two faces, lower-numbered first; the reference face is the first,
        // or the second when flipped.
        let mut own: Vec<usize> = (0..6)
            .filter(|&f| faces[f].0 != axis && faces[f].1 == start[faces[f].0])
            .collect();
        own.sort_unstable();
        prop_assert_eq!(own.len(), 2);
        let (reference, other) = if flip { (own[1], own[0]) } else { (own[0], own[1]) };
        // `distance` runs along the reference face, square to the other face's axis.
        let (along_ref, along_other) = (faces[other].0, faces[reference].0);
        let (room_ref, room_other) = (dims[along_ref], dims[along_other]);
        let d = f_ref * room_ref;
        let size = match kind {
            0 => equal(d),
            1 => two(d, f_other * room_other),
            _ => angled(d, exact_angles()[angle].0),
        };
        let reach = reaches(size, 1.0, 0.0);
        if let Some((r_ref, r_other)) = reach {
            prop_assume!(length_allowed(r_ref) && r_other > 1e-6);
        } else {
            prop_assume!(length_allowed(d));
        }
        let past = reach.is_none_or(|(r_ref, r_other)| {
            r_ref > 1.01 * room_ref || r_other > 1.01 * room_other
        });
        let result = chamfer(&shape, &[edge], size, flip);
        if past {
            let err = result.err().unwrap();
            prop_assert!(
                matches!(err.kind(), KernelErrorKind::Failed | KernelErrorKind::InvalidResult),
                "{err}"
            );
            let named = format!("Edge{edge}");
            prop_assert_eq!(err.element(), Some(named.as_str()));
            return Ok(());
        }
        let (r_ref, r_other) = reach.unwrap();
        prop_assume!(r_ref < 0.9995 * room_ref && r_other < 0.9995 * room_other);
        let built = result.unwrap();
        let s = summarize(&built.shape).unwrap();
        prop_assert!(s.valid);
        prop_assert_eq!((s.faces, s.edges, s.vertices), (7, 15, 10));
        prop_assert_eq!(built.history.validate(1), Ok(()));
        let fate = &built.history.inputs[0].edges[edge as usize - 1];
        prop_assert!(fate.is_deleted() && fate.generated.len() == 1);
        // The chamfer face's corners, where the B-rep puts them: at each end of the edge,
        // r_ref in along the reference face and r_other in along the other. OCCT places them
        // to a few ULPs of the box's size (9e-14 off on a box 1074 tall), far inside its
        // tolerance; a face swapped, or a reach wrong, is out by the reach.
        let tolerance = 1e-11 * dims.iter().copied().fold(1.0, f64::max);
        let on_face = face_points(&built.shape, fate.generated[0].element.index)
            .unwrap_or_default();
        let inward = |a: usize| if start[a] == 0.0 { 1.0 } else { -1.0 };
        for end in [0.0, dims[axis]] {
            let (mut on_ref, mut on_other) = (start, start);
            on_ref[axis] = end;
            on_other[axis] = end;
            on_ref[along_ref] += inward(along_ref) * r_ref;
            on_other[along_other] += inward(along_other) * r_other;
            for corner in [on_ref, on_other] {
                prop_assert!(
                    on_face.is_empty()
                        || on_face.iter().any(|&p| distance(p, corner) <= tolerance),
                    "no corner at {:?}: {:?}", corner, on_face
                );
            }
        }
        // And the volume, to the integration's 1e-9.
        let volume = dims[0] * dims[1] * dims[2];
        let removed = dims[axis] * r_ref * r_other / 2.0;
        prop_assert!(
            relative_error(s.volume, volume - removed) <= 1e-9,
            "volume {} want {}", s.volume, volume - removed
        );
        let again = chamfer(&shape, &[edge], size, flip).unwrap();
        prop_assert!(to_brep(&again.shape).unwrap() == to_brep(&built.shape).unwrap());
    }

    /// A chamfer of a side edge of a convex prism, its corner any angle: the triangle the
    /// reaches span at the interior angle θ (r1 r2 sin θ / 2 over the height), `distance`
    /// on the lower-numbered side face, including angles past 90° on acute corners.
    #[test]
    fn prism_side_edges_cut_the_triangle_the_sizes_say(
        n in 3usize..=7,
        gaps in proptest::collection::vec(0.05..1.0f64, 7),
        squash in prop_oneof![0.05..0.3f64, 0.3..1.0f64],
        scale in prop_oneof![0.5..5.0f64, 5.0..200.0f64],
        height in prop_oneof![0.2..2.0f64, 2.0..100.0f64],
        corner in 0usize..7,
        kind in 0u8..3,
        f_ref in 0.01..0.97f64,
        f_other in 0.01..0.97f64,
        angle in 0usize..14,
        flip in any::<bool>(),
    ) {
        // Points on an ellipse, in order round it (t from −10 to 10 sweeps the angle
        // 2 atan t from −169° to 169°): a convex polygon, counterclockwise.
        let total: f64 = gaps[..n].iter().sum();
        let mut t = -10.0;
        let points: Vec<[f64; 2]> = gaps[..n]
            .iter()
            .map(|g| {
                let p = [scale * (1.0 - t * t) / (1.0 + t * t), scale * squash * 2.0 * t / (1.0 + t * t)];
                t += 20.0 * g / total;
                p
            })
            .collect();
        let k = corner % n;
        let (prev, here, next) = (points[(k + n - 1) % n], points[k], points[(k + 1) % n]);
        let to_prev = [prev[0] - here[0], prev[1] - here[1]];
        let to_next = [next[0] - here[0], next[1] - here[1]];
        let len_prev = (to_prev[0] * to_prev[0] + to_prev[1] * to_prev[1]).sqrt();
        let len_next = (to_next[0] * to_next[0] + to_next[1] * to_next[1]).sqrt();
        let cos_t = (to_prev[0] * to_next[0] + to_prev[1] * to_next[1]) / (len_prev * len_next);
        let sin_t = (to_prev[0] * to_next[1] - to_prev[1] * to_next[0]).abs() / (len_prev * len_next);
        // OCCT takes no corner turning less than 0.1 rad: keep to θ < π − 0.12 (cos θ >
        // −cos 0.12) and, clear of slivers, θ > 0.02 (cos θ < cos 0.02).
        prop_assume!(cos_t > -0.992_808_635_853_866_3 && cos_t < 0.999_800_006_666_577_8);
        prop_assume!(len_prev > 1e-3 * scale && len_next > 1e-3 * scale);
        let profile = Profile {
            points: points.iter().map(|p| [p[0], p[1], 0.0]).collect(),
        };
        let built = match extrude(&profile, height, None) {
            Ok(built) => built,
            // A profile the rules refuse (corners too close) isn't this test's business.
            Err(e) => {
                prop_assume!(e.kind() != KernelErrorKind::InvalidInput);
                panic!("{e}");
            }
        };
        let h = &built.history.inputs[0];
        let edge = h.vertices[k].generated[0].element.index;
        let face_prev = h.edges[(k + n - 1) % n].generated[0].element.index;
        let face_next = h.edges[k].generated[0].element.index;
        // `distance` on the lower-numbered face, or the other when flipped.
        let ref_is_prev = (face_prev < face_next) != flip;
        let (room_ref, room_other) = if ref_is_prev {
            (len_prev, len_next)
        } else {
            (len_next, len_prev)
        };
        let size = match kind {
            0 => equal((f_ref * room_ref).min(f_other * room_other)),
            1 => two(f_ref * room_ref, f_other * room_other),
            _ => {
                // Any angle that makes a triangle at this corner, its distance cut down so
                // the other reach fits too.
                let fitting: Vec<_> = exact_angles()
                    .into_iter()
                    // Not exactly 90°, where OCCT's result can be invalid (see
                    // a_distance_and_angle_of_exactly_90_degrees_on_an_acute_corner).
                    .filter(|&(degrees, s, c)| degrees != 90.0 && sin_t * c + cos_t * s > 0.05)
                    .collect();
                prop_assume!(!fitting.is_empty());
                let (degrees, s, c) = fitting[angle % fitting.len()];
                let ratio = s / (sin_t * c + cos_t * s);
                angled((f_ref * room_ref).min(f_other * room_other / ratio), degrees)
            }
        };
        let Some((r_ref, r_other)) = reaches(size, sin_t, cos_t) else {
            return Err(TestCaseError::reject("no triangle"));
        };
        prop_assume!(r_ref < 0.98 * room_ref && r_other < 0.98 * room_other);
        prop_assume!(r_ref > 1e-4 * scale && r_other > 1e-4 * scale);
        let result = chamfer(&built.shape, &[edge], size, flip);
        let chamfered = result.unwrap_or_else(|e| panic!("{size:?} at cos θ {cos_t}: {e}"));
        let s = summarize(&chamfered.shape).unwrap();
        prop_assert!(s.valid);
        prop_assert_eq!(chamfered.history.validate(1), Ok(()));
        prop_assert_eq!(s.faces, built.history.result.faces + 1);
        let (r_prev, r_next) = if ref_is_prev { (r_ref, r_other) } else { (r_other, r_ref) };
        // In plan, the chamfer face runs from r_prev along the side toward the previous
        // corner to r_next along the side toward the next: every point of it is over one
        // of those two, and each has some.
        let q_prev = [here[0] + r_prev * to_prev[0] / len_prev, here[1] + r_prev * to_prev[1] / len_prev];
        let q_next = [here[0] + r_next * to_next[0] / len_next, here[1] + r_next * to_next[1] / len_next];
        let tolerance = 1e-11 * scale.max(height).max(1.0);
        let face = chamfered.history.inputs[0].edges[edge as usize - 1].generated[0].element.index;
        let on_face = face_points(&chamfered.shape, face).unwrap();
        let over = |p: [f64; 3], q: [f64; 2]| {
            ((p[0] - q[0]) * (p[0] - q[0]) + (p[1] - q[1]) * (p[1] - q[1])).sqrt() <= tolerance
        };
        prop_assert!(
            on_face.iter().all(|&p| over(p, q_prev) || over(p, q_next)),
            "cos θ {}: {:?} not all over {:?} or {:?}", cos_t, on_face, q_prev, q_next
        );
        prop_assert!(on_face.iter().any(|&p| over(p, q_prev)) && on_face.iter().any(|&p| over(p, q_next)));
        // And the volume, to the integration's 1e-9: the prism less the triangle.
        let want = (polygon_area(&points) - r_prev * r_next * sin_t / 2.0) * height;
        prop_assert!(
            relative_error(s.volume, want) <= 1e-9,
            "volume {} want {} (cos θ {})", s.volume, want, cos_t
        );
    }

    /// A size is refused, naming its parameter, exactly where the rule says: each distance
    /// above 1e-7 and at most 1e6, an angle above 0° and below 180° whose cut (the distance
    /// times the angle in radians) is deeper than 1e-7; and only then.
    #[test]
    fn sizes_are_refused_exactly_where_the_rule_says(
        kind in 0u8..3,
        a in hostile_length(),
        b in hostile_length(),
        angle in hostile_angle(),
    ) {
        let size = match kind {
            0 => equal(a),
            1 => two(a, b),
            _ => angled(a, angle),
        };
        let bad = match kind {
            0 => (!length_allowed(a)).then_some("distance"),
            1 => (!length_allowed(a))
                .then_some("distance")
                .or((!length_allowed(b)).then_some("other")),
            _ => (!length_allowed(a)).then_some("distance").or((!(angle > 0.0
                && angle < 180.0
                && a * (angle * (std::f64::consts::PI / 180.0)) > 1e-7))
                .then_some("angle")),
        };
        prop_assert_eq!(size.check().err().map(|e| e.parameter), bad);
        let result = chamfer(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], size, false);
        match (bad, result) {
            (Some(_), Err(err)) => prop_assert_eq!(err.kind(), KernelErrorKind::InvalidInput),
            (Some(parameter), Ok(_)) => prop_assert!(false, "{} not refused", parameter),
            (None, Err(err)) => prop_assert_ne!(err.kind(), KernelErrorKind::InvalidInput, "{}", err),
            (None, Ok(_)) => {}
        }
    }
}
