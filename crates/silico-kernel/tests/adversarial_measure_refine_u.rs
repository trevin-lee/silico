//! /break-it on OCCT patch 0004 (`BRepGProp_Gauss::Compute` refines U: `EpsU` the asked
//! precision of each strip's magnitude, none where the initial spans reach the cap) and on
//! `measure` integrating a shape moved by minus its tight box's centre (bridge.cpp). Every
//! value here is by hand (analytic, the shoelace formula, Rodrigues' rotation, Gauss–Legendre
//! on Cox–de Boor), and every measure goes through a kernel worker.
//!
//! Attacked: faces whose strips integrate to about nothing (a plane through the point the
//! volume is integrated about, where a tolerance relative to the strip's value is 0 or
//! round-off and U could only stop at its cap), B-spline faces of many spans (the cap's
//! cost, past `SM` spans, rough ones that reach the cap while refining, and rough ones
//! trimmed so that narrow strips follow wide ones), integrands kinked inside one U span
//! (an extruded polyline), located and rotated shapes far out, shapes centred exactly at
//! the origin, compounds whose box centre is nowhere near their parts, an empty compound,
//! and the worker's 60 s timeout.

use super::geometry::{apply, cross, dot, norm, rotation};
use super::kernel::kernel;
use super::oracle::{self, freecad_shape, freecad_text, from_freecad};
use super::quadrature::{
    basis, basis_derivative, integral, misses, misses_by, right_or_known_wrong,
    right_or_known_wrong_by,
};

use std::cell::RefCell;
use std::f64::consts::PI;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use silico_core::kernel::KernelWorker;
use silico_protocol::{ShapeData, ShapeSummary};

/// `shape` summarized through `k`, and how long that took.
fn timed(k: &mut KernelWorker, shape: &ShapeData) -> (ShapeSummary, Duration) {
    let start = Instant::now();
    let s = k.summarize(shape).unwrap();
    (s, start.elapsed())
}

fn centre_cases(what: &str, got: [f64; 3], want: [f64; 3]) -> Vec<(String, f64, f64)> {
    (0..3)
        .map(|i| {
            (
                format!("{what}: centre {}", ["x", "y", "z"][i]),
                got[i],
                want[i],
            )
        })
        .collect()
}

// ---- Strips that integrate to about nothing: U's tolerance 0 or round-off ----

/// An equal-legged L (legs 2t, thickness t) in XZ, extruded d along y: the mean of its
/// vertices, which OCCT integrates the volume about, is its inner corner, (t, d/2, t), so
/// its two inner faces' planes hold that point, and so does the box's centre the measure
/// moves to the origin: every strip of those faces integrates to round-off. Rotated by
/// its location so the round-off isn't exactly 0.
#[test]
fn an_l_brackets_inner_faces_through_its_vertex_mean_measure_by_hand_in_time() {
    let (t, s, d) = (0.1_f64, 0.2_f64, 0.3_f64);
    let (axis, angle, at) = ([1.0, 2.0, 3.0], 37.0, [0.1, 0.2, 0.3]);
    let shape = freecad_shape(&format!(
        "t, s, d = {t:?}, {s:?}, {d:?}\n\
         pts = [V(0,0,0), V(s,0,0), V(s,0,t), V(t,0,t), V(t,0,s), V(0,0,s), V(0,0,0)]\n\
         shape = Part.Face(Part.makePolygon(pts)).extrude(V(0, d, 0))\n\
         shape.Placement = App.Placement(V({}, {}, {}), App.Rotation(V(1, 2, 3), {angle:?}))\n",
        at[0], at[1], at[2]
    ));
    let mut k = kernel();
    let (m, took) = timed(&mut k, &shape);
    assert!(m.valid);
    let area_l = s * s - (s - t) * (s - t);
    let moment = (s * s * s / 2.0 - (s - t) * (s - t) * (s + t) / 2.0) / area_l;
    let centre = apply(&rotation(axis, angle), [moment, d / 2.0, moment], at);
    let missed = misses(
        &[
            ("volume".to_owned(), m.volume, area_l * d),
            ("area".to_owned(), m.area, 2.0 * area_l + 4.0 * s * d),
        ],
        1e-12,
    );
    let off = misses_by(&centre_cases("L", m.center, centre), 1e-12);
    assert!(missed.is_empty() && off.is_empty(), "{missed:?} {off:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

/// A lone square face, tilted: the volume OCCT integrates for it is the cone from the
/// mean of its vertices, which lies in its plane, so every strip is round-off. Its area is
/// 1, and the volume of a face about a point in its plane is 0.
#[test]
fn a_lone_tilted_square_whose_plane_holds_its_vertex_mean_measures_in_time() {
    let shape = freecad_shape(
        "shape = Part.makePlane(1, 1)\n\
         shape.Placement = App.Placement(V(0.3, 0.2, 0.1), App.Rotation(V(1, 1, 1), 33))\n",
    );
    let mut k = kernel();
    let (m, took) = timed(&mut k, &shape);
    assert!((m.area - 1.0).abs() <= 1e-12, "{m:?}");
    assert!(m.volume.abs() <= 1e-12, "{m:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

/// Python: `shape`, the unit square as a degree-1 B-spline surface of `nu` × `nv` spans
/// (poles on the knots, so the parametrization is the plane's own), tilted by its location.
fn planar_bspline(nu: usize, nv: usize) -> String {
    format!(
        "nu, nv = {nu}, {nv}\n\
         poles = [[V(i / nu, j / nv, 0) for j in range(nv + 1)] for i in range(nu + 1)]\n\
         mu = [2] + [1] * (nu - 1) + [2]\n\
         mv = [2] + [1] * (nv - 1) + [2]\n\
         s = Part.BSplineSurface()\n\
         s.buildFromPolesMultsKnots(poles, mu, mv, [i / nu for i in range(nu + 1)], \
             [j / nv for j in range(nv + 1)], False, False, 1, 1)\n\
         shape = s.toShape()\n\
         shape.Placement = App.Placement(V(0.3, 0.2, 0.1), App.Rotation(V(1, 1, 1), 33))\n"
    )
}

/// The same square as a B-spline surface of 60 × 60 spans: U's cap is 32 × 60 + 1
/// subdivisions, and L's Gauss order grows with V's spans, so a strip that never meets its
/// tolerance costs U's whole cap at every L point.
#[test]
fn a_tilted_planar_bspline_face_of_sixty_by_sixty_spans_measures_within_the_timeout() {
    let shape = freecad_shape(&planar_bspline(60, 60));
    let mut k = kernel();
    let (m, took) = timed(&mut k, &shape);
    assert!((m.area - 1.0).abs() <= 1e-12, "{m:?}");
    assert!(m.volume.abs() <= 1e-12, "{m:?}");
    // Measured through the worker, 0.30 s (0.27 s without patch 0004). Every strip's value
    // is round-off, and U's tolerance is the asked precision of the strip's magnitude, the
    // sum of its terms' absolute values, which is round-off too where every term is (the
    // face's plane holds the vertex mean), so U stops after its first pass. As first
    // proposed, relative to the strip's value, it was 59.96 s, past the worker's 60 s
    // timeout: U refined to its cap (1921 subdivisions) at every L point.
    assert!(took < Duration::from_secs(10), "{took:?}");
}

/// Python: `shape`, the equal-legged L of the first test (legs 0.2, thickness 0.1, depth
/// 0.3), its two inner faces (whose planes hold its vertex mean) remade as degree-1 B-spline
/// rectangles of `n` × `n` spans and sewn back, turned and moved as before. Valid, and the
/// same solid: an import whose CAD wrote its planes as B-spline surfaces.
fn l_with_bspline_inner_faces(n: usize) -> String {
    format!(
        "t, s, d, n = 0.1, 0.2, 0.3, {n}\n\
         pts = [V(0,0,0), V(s,0,0), V(s,0,t), V(t,0,t), V(t,0,s), V(0,0,s), V(0,0,0)]\n\
         L = Part.Face(Part.makePolygon(pts)).extrude(V(0, d, 0))\n\
         def rect(o, a, b):\n\
         \x20   poles = [[o + a * (i / n) + b * (j / n) for j in range(n + 1)] for i in range(n + 1)]\n\
         \x20   m = [2] + [1] * (n - 1) + [2]\n\
         \x20   k = [i / n for i in range(n + 1)]\n\
         \x20   srf = Part.BSplineSurface()\n\
         \x20   srf.buildFromPolesMultsKnots(poles, m, m, k, k, False, False, 1, 1)\n\
         \x20   return srf.toShape()\n\
         faces = []\n\
         for f in L.Faces:\n\
         \x20   bb = f.BoundBox\n\
         \x20   if abs(bb.XMin - t) < 1e-12 and abs(bb.XMax - t) < 1e-12:\n\
         \x20       faces.append(rect(V(t, 0, t), V(0, 0, s - t), V(0, d, 0)))\n\
         \x20   elif abs(bb.ZMin - t) < 1e-12 and abs(bb.ZMax - t) < 1e-12:\n\
         \x20       faces.append(rect(V(t, 0, t), V(s - t, 0, 0), V(0, d, 0)))\n\
         \x20   else:\n\
         \x20       faces.append(f)\n\
         sewed = Part.makeShell(faces)\n\
         sewed.sewShape()\n\
         shape = Part.Solid(sewed)\n\
         shape.fix(1e-7, 1e-7, 1e-7)\n\
         if shape.Volume < 0: shape.reverse()\n\
         assert shape.isValid()\n\
         assert sum(1 for f in shape.Faces if f.Surface.TypeId == 'Part::GeomBSplineSurface') == 2\n\
         shape.Placement = App.Placement(V(0.1, 0.2, 0.3), App.Rotation(V(1, 2, 3), 37))\n"
    )
}

/// The L bracket with its inner faces as B-spline rectangles of 20 × 20 and 60 × 60 spans:
/// a valid solid, measured by hand as the plain L is (0.009, 0.3).
#[test]
fn an_l_bracket_whose_inner_faces_are_many_span_bsplines_measures_within_the_timeout() {
    let mut k = kernel();
    for n in [20, 60] {
        let shape = freecad_shape(&l_with_bspline_inner_faces(n));
        let start = Instant::now();
        let got = k.summarize(&shape);
        let took = start.elapsed();
        // Measured through the worker: 20 × 20 0.20 s, 60 × 60 0.53 s (0.19 s and 0.48 s
        // without patch 0004). The faces' strips are round-off about the vertex mean, as
        // the plain L's, and so is their magnitude, so U stops after its first pass. As
        // first proposed (U's tolerance relative to the strip's value), 29.6 s and 122.8 s:
        // past the worker's 60 s, the solid's summary, its fingerprint, and any feature on
        // it failed.
        let m = got.unwrap();
        assert!(m.valid);
        let missed = misses(
            &[
                (format!("{n}: volume"), m.volume, 0.009),
                (format!("{n}: area"), m.area, 0.3),
            ],
            1e-12,
        );
        assert!(missed.is_empty(), "{missed:?}");
        assert!(took < Duration::from_secs(10), "{n}: {took:?}");
    }
}

/// A square B-spline face of 2000 U spans, past `SM` (32 × 61 + 1 = 1953): U's cap is
/// below its spans, so OCCT's first pass zeroes the span at the cap's index (`JU ==
/// UMaxSubs` while `kUEnd == 1`), in U and in L alike, with or without patch 0004, and the
/// patch gives such a face no U tolerance (`EpsU = 0`), so it's integrated as before it.
/// By hand its area is 1 and its volume (the cone from its vertex mean, in its plane) 0;
/// OCCT's known wrong area is 1 − 1/2000, span 1953 of 2000 equal spans left out
/// (docs/ports/measure.md, Known wrong results): either is taken, so a fixed OCCT passes,
/// and any other value fails.
#[test]
fn a_planar_bspline_face_of_two_thousand_u_spans_measures_its_area() {
    let shape = freecad_shape(&planar_bspline(2000, 1));
    let mut k = kernel();
    // Before the amendment (patch 0004 as first proposed): kernel_failed
    // "Standard_OutOfRange: NCollection_Array1::ChangeValue" after 8.9 s, U refined past
    // its arrays (the cap `JU == UMaxSubs` was passed before `JU >= iUSubEnd` started
    // refining, so it never ended it).
    let m = k.summarize(&shape).unwrap();
    let missed = right_or_known_wrong(
        &[("area".to_owned(), m.area, 1.0)],
        1e-12,
        &[("area", 1.0 - 1.0 / 2000.0)],
    );
    assert!(missed.is_empty(), "{missed:?}");
    assert!(m.volume.abs() <= 1e-12, "{m:?}");
}

/// The worker is well after a 2000-span face (which once made it fail).
#[test]
fn the_worker_measures_a_box_after_a_two_thousand_span_face() {
    let mut k = kernel();
    let _ = k.summarize(&freecad_shape(&planar_bspline(2000, 1)));
    let again = freecad_shape("shape = Part.makeBox(1, 2, 3)\n");
    let m = k.summarize(&again).unwrap();
    assert!(
        (m.volume - 6.0).abs() <= 1e-12 && (m.area - 22.0).abs() <= 1e-12,
        "{m:?}"
    );
}

/// That face (unit square, 2000 U spans) extruded 1: a unit cube, valid, 1 and 6 by hand.
/// OCCT's known wrong answer (not patch 0004's: the same without it, to the bit) leaves out
/// span 1953 of the bottom and the top, each 1/2000 of their area: 6 − 2/2000 = 5.999 in
/// area, and in volume, which each of the two adds a third of (∮ (p − c)·n / 3 about the
/// vertex mean, half the height away), 1 − 2 × (1/2000) × (1/2) / 3 = 1 − 1/6000. The
/// sides are surfaces of extrusion, one U span each, whole.
#[test]
fn a_prism_of_a_two_thousand_span_bspline_face_is_a_unit_cube() {
    let shape = freecad_shape(&format!(
        "{}shape.Placement = App.Placement()\nshape = shape.extrude(V(0, 0, 1))\nassert shape.isValid()\n",
        planar_bspline(2000, 1)
    ));
    let mut k = kernel();
    let m = k.summarize(&shape).unwrap();
    assert!(m.valid);
    let missed = right_or_known_wrong(
        &[
            ("volume".to_owned(), m.volume, 1.0),
            ("area".to_owned(), m.area, 6.0),
        ],
        1e-12,
        &[("volume", 1.0 - 1.0 / 6000.0), ("area", 6.0 - 2.0 / 2000.0)],
    );
    assert!(missed.is_empty(), "{missed:?}");
}

// ---- Rough faces: U refined to its cap, or past it not at all ----

/// ∫ f over [a, b], given `whole`, its eight-point Gauss–Legendre estimate there: the two
/// halves' estimates, each bisected again until the halves agree with their whole to
/// `tolerance` (absolute, the same at every depth, so round-off can't make it bisect
/// without end), at most `depth` times.
fn bisected(
    a: f64,
    b: f64,
    whole: f64,
    tolerance: f64,
    depth: u32,
    f: &impl Fn(f64) -> f64,
) -> f64 {
    let mid = (a + b) / 2.0;
    let (left, right) = (integral(a, mid, 1, f), integral(mid, b, 1, f));
    if depth == 0 || (left + right - whole).abs() <= tolerance {
        return left + right;
    }
    bisected(a, mid, left, tolerance, depth - 1, f)
        + bisected(mid, b, right, tolerance, depth - 1, f)
}

/// Where a rough face is trimmed in its (u, v): the whole unit square, or the triangle
/// (0, 0), (1, 0), (1, 1), v ≤ u, whose U strip at each point of its diagonal runs from 0
/// to the point, so the strips there narrow from all of U's spans to none, after its side
/// u = 1's strips, which each cross every span.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Trim {
    Square,
    Triangle,
}

/// rough.py's faces (seed 7): cubic in U over `n` equal spans, linear in V, poles (i/n, j,
/// z) for i in 0..n + 3 and j in 0..2, each z = 0.004 × uniform(−1, 1) drawn in that order
/// (the 200-span face drawn first, then the 2000-span one, then any other), so the poles
/// jump by about a span's width and no span's integrand is smooth enough for one Gauss
/// pass. Trimmed to the square or the triangle (its edges `Geom2d` segments in (u, v) on
/// the surface), tilted by its location. The face as FreeCAD makes it, each pole's z as
/// drawn, and the surface's point at a few (u, v), to check the evaluation by hand against
/// OCCT's.
struct Rough {
    shape: ShapeData,
    trim: Trim,
    z: Vec<[f64; 2]>,
    points: Vec<([f64; 2], [f64; 3])>,
}

fn rough(n: usize, trim: Trim) -> Rough {
    let triangle = if trim == Trim::Triangle {
        "True"
    } else {
        "False"
    };
    let script = format!(
        "import json, random, Part, FreeCAD as App\n\
         V = App.Vector\n\
         random.seed(7)\n\
         def rough(n):\n\
         \x20   z = [[0.004 * random.uniform(-1, 1) for j in range(2)] for i in range(n + 3)]\n\
         \x20   poles = [[V(i / n, j, z[i][j]) for j in range(2)] for i in range(n + 3)]\n\
         \x20   s = Part.BSplineSurface()\n\
         \x20   s.buildFromPolesMultsKnots(poles, [4] + [1] * (n - 1) + [4], [2, 2], \
             [i / n for i in range(n + 1)], [0.0, 1.0], False, False, 3, 1)\n\
         \x20   return s, z\n\
         drawn = {{n: rough(n) for n in (200, 2000)}}\n\
         if {n} not in drawn: drawn[{n}] = rough({n})\n\
         s, z = drawn[{n}]\n\
         if {triangle}:\n\
         \x20   P = App.Base.Vector2d\n\
         \x20   c = [P(0, 0), P(1, 0), P(1, 1)]\n\
         \x20   edges = [Part.Geom2d.Line2dSegment(c[i], c[(i + 1) % 3]).toShape(s) for i in range(3)]\n\
         \x20   shape = Part.Face(s, Part.Wire(edges))\n\
         else:\n\
         \x20   shape = s.toShape()\n\
         shape.Placement = App.Placement(V(0.3, 0.2, 0.1), App.Rotation(V(1, 1, 1), 33))\n\
         assert shape.isValid()\n\
         at = [(0.0, 0.0), (0.123456, 0.25), (0.5, 0.5), (0.97653, 0.9), (0.999, 1.0)]\n\
         points = [[[u, v], list(s.value(u, v))] for u, v in at]\n\
         print(\"SILICO_ORACLE \" + json.dumps({{\"brep\": shape.exportBrepToString(), \"z\": z, \"points\": points}}))\n"
    );
    let got = oracle::freecad(&script);
    let numbers = |v: &serde_json::Value| -> Vec<f64> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap())
            .collect()
    };
    let z = got["z"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let r = numbers(row);
            [r[0], r[1]]
        })
        .collect();
    let points = got["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let (uv, xyz) = (numbers(&p[0]), numbers(&p[1]));
            ([uv[0], uv[1]], [xyz[0], xyz[1], xyz[2]])
        })
        .collect();
    Rough {
        shape: from_freecad(got["brep"].as_str().unwrap()),
        trim,
        z,
        points,
    }
}

impl Rough {
    fn spans(&self) -> usize {
        self.z.len() - 3
    }

    /// The flat knot sequence along U: 0 four times, k/n, 1 four times.
    fn knots(&self) -> Vec<f64> {
        let n = self.spans();
        let mut flat = vec![0.0; 3];
        flat.extend((0..=n).map(|k| k as f64 / n as f64));
        flat.extend([1.0; 3]);
        flat
    }

    /// The two rows' points and U derivatives at `u` in span `k` (whose basis functions
    /// are k..k + 4), by Cox–de Boor on the drawn poles: `[(C0, C0'), (C1, C1')]`.
    fn rows(&self, knots: &[f64], k: usize, u: f64) -> [([f64; 3], [f64; 3]); 2] {
        let n = self.spans() as f64;
        let mut rows = [([0.0; 3], [0.0; 3]); 2];
        for i in k..k + 4 {
            let (b, db) = (basis(knots, i, 3, u), basis_derivative(knots, i, 3, u));
            for (j, (point, tangent)) in rows.iter_mut().enumerate() {
                let pole = [i as f64 / n, j as f64, self.z[i][j]];
                for d in 0..3 {
                    point[d] += b * pole[d];
                    tangent[d] += db * pole[d];
                }
            }
        }
        rows
    }

    /// S(u, v) = (1 − v) C0(u) + v C1(u), `u` inside span `k`.
    fn point(&self, knots: &[f64], k: usize, u: f64, v: f64) -> [f64; 3] {
        let [(c0, _), (c1, _)] = self.rows(knots, k, u);
        [0, 1, 2].map(|d| (1.0 - v) * c0[d] + v * c1[d])
    }

    /// The area of each span, ∫∫ |S_u × S_v| dv du, v from 0 to the trim's top: 1 on the
    /// square, u on the triangle. S_u = (1 − v) C0′ + v C1′ and S_v = C1 − C0, so S_u × S_v
    /// = A + vB, A = C0′ × S_v and B = (C1′ − C0′) × S_v, and along V it's exact: |A + vB| =
    /// |B| √((v + p)² + h²), p = A·B / |B|², h = |A × B| / |B|², whose integral is (t √(t² +
    /// h²) + h² asinh(t / h)) / 2 between t = p and top + p. Along U, adaptive, split at
    /// every knot: eight Gauss–Legendre points on an interval against eight on each half,
    /// bisected until the two agree to `tolerance` of the span's first estimate. (The poles
    /// jump by about a span's width, so the integrand along U is analytic in a strip only
    /// about a span/16 wide: fixed pieces converge slowly, bisection doesn't care.)
    fn span_areas(&self, tolerance: f64) -> Vec<f64> {
        let knots = self.knots();
        let n = self.spans();
        let across = |k: usize, u: f64| {
            let [(c0, d0), (c1, d1)] = self.rows(&knots, k, u);
            let sv = [0, 1, 2].map(|d| c1[d] - c0[d]);
            let a = cross(d0, sv);
            let b = cross([0, 1, 2].map(|d| d1[d] - d0[d]), sv);
            let bb = dot(b, b);
            if bb == 0.0 {
                return dot(a, a).sqrt();
            }
            let (p, h) = (
                dot(a, b) / bb,
                libm::sqrt(dot(cross(a, b), cross(a, b))) / bb,
            );
            let primitive = |t: f64| {
                let root = libm::sqrt(t * t + h * h);
                if h == 0.0 {
                    t * root / 2.0
                } else {
                    (t * root + h * h * libm::asinh(t / h)) / 2.0
                }
            };
            let top = match self.trim {
                Trim::Square => 1.0,
                Trim::Triangle => u,
            };
            libm::sqrt(bb) * (primitive(top + p) - primitive(p))
        };
        (0..n)
            .map(|k| {
                let f = |u: f64| across(k, u);
                let (a, b) = (knots[k + 3], knots[k + 4]);
                let whole = integral(a, b, 1, f);
                bisected(a, b, whole, tolerance * whole.abs(), 24, &f)
            })
            .collect()
    }

    /// The area by hand, each span's (bisected to 1e-15 of each), and how far the whole is
    /// from itself bisected to only 1e-11 (a bound on the looser one's error, relative, so
    /// far above the tighter one's).
    fn area(&self) -> (Vec<f64>, f64) {
        let (loose, tight) = (self.span_areas(1e-11), self.span_areas(1e-15));
        let (a, b): (f64, f64) = (loose.iter().sum(), tight.iter().sum());
        (tight, ((a - b) / b).abs())
    }

    /// The hand evaluation against OCCT's at FreeCAD's points, in the surface's own frame.
    fn assert_evaluated_as_occt(&self) {
        let knots = self.knots();
        let n = self.spans();
        for ([u, v], want) in &self.points {
            let k = ((u * n as f64).floor() as usize).min(n - 1);
            let got = self.point(&knots, k, *u, *v);
            for d in 0..3 {
                assert!(
                    (got[d] - want[d]).abs() <= 1e-14,
                    "S({u}, {v}): {got:?} for OCCT's {want:?}"
                );
            }
        }
    }
}

/// The rough face of 200 spans, under the cap: U refines at every L point, to the asked
/// 1e-9 of each strip's magnitude (here its value: an area's terms never cancel), and L to
/// its own 1e-9: by hand 1.0564688264694702, measured 1.0564688264646631 (4.6e-12) in 15
/// ms. Held to 1e-11, so it fails unrefined: without patch 0004, or under a cap rule that
/// withheld U's tolerance from faces under the cap too, U isn't refined and the area is
/// 2.0e-11 off.
#[test]
fn a_rough_bspline_face_of_two_hundred_spans_measures_its_area_by_hand() {
    let face = rough(200, Trim::Square);
    face.assert_evaluated_as_occt();
    let (spans, error) = face.area();
    assert!(error <= 1e-12, "the hand quadrature's own error: {error:e}");
    let area: f64 = spans.iter().sum();
    let mut k = kernel();
    let (m, took) = timed(&mut k, &face.shape);
    assert!(m.valid);
    let missed = misses(&[("area".to_owned(), m.area, area)], 1e-11);
    assert!(missed.is_empty(), "{missed:?}");
    assert!(took < Duration::from_secs(10), "{took:?}");
}

/// A rough face of `n` spans, `n` at or past the cap (`SM` = 1953): patch 0004 gives it no
/// U tolerance, so U isn't refined, as before the patch, and it's integrated at its first
/// pass's Gauss points, its span 1953 left out (OCCT's known wrong result, as the planar
/// face's above); L refines to its own 1e-9. Its measure is the area by hand without span
/// 1953, to the tolerance its unrefined U reaches (`ROUGH_AT_THE_CAP`), or the whole area
/// by hand, so a fixed OCCT passes; in time.
fn assert_past_the_cap(n: usize) {
    let face = rough(n, Trim::Square);
    face.assert_evaluated_as_occt();
    let (spans, error) = face.area();
    assert!(error <= 1e-12, "the hand quadrature's own error: {error:e}");
    let area: f64 = spans.iter().sum();
    let without = area - spans[1952];
    let mut k = kernel();
    let (m, took) = timed(&mut k, &face.shape);
    assert!(m.valid);
    let right = misses(&[("area".to_owned(), m.area, area)], 1e-9);
    let known = misses(&[("area".to_owned(), m.area, without)], ROUGH_AT_THE_CAP);
    assert!(
        right.is_empty() || known.is_empty(),
        "{n}: {right:?} {known:?} (span 1953: {})",
        spans[1952]
    );
    assert!(took < Duration::from_secs(10), "{n}: {took:?}");
}

/// 2000 spans. Before the amendment, U refined towards its arrays' end at every L point
/// and the worker was killed at its timeout. By hand: the area, 2.8315774454792653, and
/// the area without span 1953, 2.8292641882719094; the measure 2.8292747615806313, in 0.57
/// s (0.50 s without patch 0004).
#[test]
fn a_rough_bspline_face_of_two_thousand_spans_past_the_cap_measures_in_time() {
    assert_past_the_cap(2000);
}

/// 1953 spans, exactly the cap: the first with no U tolerance (its initial spans reach
/// `UMaxSubs`, `iUSubEnd < UMaxSubs` false), so a rule that took the cap's own count as
/// under it (`<=`) would refine it past its cap, as the 2000-span face once did. By hand
/// 2.736421101215214, and 2.734310161810381 without span 1953; measured
/// 2.7342996060588294 (3.86e-6 under the second) in 0.38 s. Such a rule is caught by the
/// worker's failure or its time (refining past the cap runs off OCCT's arrays or spins),
/// not by the value: the cap's own span is zeroed either way.
#[test]
fn a_rough_bspline_face_of_exactly_the_caps_spans_measures_as_past_it() {
    assert_past_the_cap(1953);
}

/// How far the rough faces of 1952, 1953, and 2000 spans may be from their hand values
/// (the last two's without their span 1953), relative: U unrefined, its first pass's Gauss
/// order on each rough span, and OCCT estimating 1.7e-12. Measured: 2000 spans 3.74e-6
/// (the same bits without patch 0004), 1953 spans 3.86e-6, 1952 spans 3.89e-6 (3.85e-6
/// without it: the cap branch loosens L). OCCT's known wrong result for faces of so many
/// spans (docs/ports/measure.md), pinned here so a worse one fails.
const ROUGH_AT_THE_CAP: f64 = 4e-6;

/// A rough face of 1952 spans, one under the cap: U gets its tolerance, and its first
/// refinement step reaches the cap (`JU == UMaxSubs` with `kUEnd == 2`), OCCT's cap
/// branch, which keeps the strip's first pass (the half it split off dropped with `--JU`,
/// the span it split still whole), loosens L's tolerance to ten times U's leftover, and
/// ends. Every span is integrated, none refined: by hand 2.735485100972233, measured
/// 2.735474451791847 in 0.16 s (2.7354745664738886 in 0.37 s without patch 0004), within
/// what its first pass reaches (`ROUGH_AT_THE_CAP`).
#[test]
fn a_rough_bspline_face_of_1952_spans_reaches_the_cap_on_its_first_refinement() {
    let face = rough(1952, Trim::Square);
    face.assert_evaluated_as_occt();
    let (spans, error) = face.area();
    assert!(error <= 1e-12, "the hand quadrature's own error: {error:e}");
    let area: f64 = spans.iter().sum();
    let mut k = kernel();
    let (m, took) = timed(&mut k, &face.shape);
    assert!(m.valid);
    let missed = misses(&[("area".to_owned(), m.area, area)], ROUGH_AT_THE_CAP);
    assert!(missed.is_empty(), "{missed:?}");
    assert!(took < Duration::from_secs(10), "{took:?}");
}

/// The rough face of 400 spans trimmed to the triangle under its diagonal: OCCT's L runs
/// along its side u = 1 first, each U strip there across all 400 spans, then along the
/// diagonal, each strip [0, u] across fewer, down to one. U's error vector is cleared only
/// to each strip's own cap (32 × its spans + 1), so past it the vector keeps a wider
/// strip's errors; patch 0004 searches only the strip's own intervals for the one to split
/// and for its error. By hand (`span_areas`: exact along V up to v = u, adaptive
/// Gauss–Legendre along U split at every knot) 0.590013413208074, measured
/// 0.590013413948603 (1.26e-9) in 1.3 s: a value pinned for this draw, not a bound (other
/// draws of the same face are about 1e-5 off, L's residual along the diagonal, the same
/// without patch 0004: docs/ports/measure.md); the 1000-span test below, in time, is the
/// guard that doesn't depend on L's accuracy. With a search of the
/// whole vector (0004 as amended before), a narrow strip split a wide one's stale
/// intervals, outside itself, and summed them: 0.5900096629043639, 6.4e-6 low, in 4.0 s.
#[test]
fn a_rough_bspline_face_trimmed_to_a_triangle_measures_its_area_by_hand() {
    let face = rough(400, Trim::Triangle);
    face.assert_evaluated_as_occt();
    let (spans, error) = face.area();
    assert!(error <= 1e-12, "the hand quadrature's own error: {error:e}");
    let area: f64 = spans.iter().sum();
    let mut k = kernel();
    let (m, _) = timed(&mut k, &face.shape);
    assert!(m.valid);
    let missed = misses(&[("area".to_owned(), m.area, area)], 2e-9);
    assert!(missed.is_empty(), "{missed:?}");
}

/// How far the triangle of 1000 spans may be from its area by hand, relative: along its
/// diagonal, a straight pcurve with no knots, L starts from one span and subdivides to at
/// most 33, which can't follow an integrand rough at every one of 1000 spans (as a
/// meridian of many spans, docs/ports/measure.md, Known wrong results). Measured 8.7e-6
/// over (8.6e-6 without patch 0004; 200 spans 5.2e-7 low either way). OCCT's known wrong
/// result, pinned here so a worse one fails.
const ROUGH_DIAGONAL: f64 = 2e-5;

/// The same triangle over 1000 spans, in time (30 s: measured 5.6 s here, up to about 9 s
/// on the gate's shared VM; a search of the whole vector takes past 60 s): 0.8318642493074171 for
/// 0.8318570454103432 by hand (8.7e-6 over: `ROUGH_DIAGONAL`). With a search of the whole
/// error vector (0004 as amended before), each step of every strip's refinement scanned
/// all of it, and every narrow strip refined a wide one's stale intervals to its cap: past
/// the worker's 60 s timeout (11.9 s, 24.9 s, and 37.6 s at 600, 700, and 800 spans, for
/// 2.1 s, 3.2 s, and 4.5 s).
#[test]
fn a_rough_bspline_face_of_a_thousand_spans_trimmed_to_a_triangle_measures_in_time() {
    let face = rough(1000, Trim::Triangle);
    let (spans, error) = face.area();
    assert!(error <= 1e-12, "the hand quadrature's own error: {error:e}");
    let area: f64 = spans.iter().sum();
    let mut k = kernel();
    let (m, took) = timed(&mut k, &face.shape);
    assert!(m.valid);
    let missed = misses(&[("area".to_owned(), m.area, area)], ROUGH_DIAGONAL);
    assert!(missed.is_empty(), "{missed:?}");
    assert!(took < Duration::from_secs(30), "{took:?}");
}

/// Python: `shape`, the saddle z = xy over [−1, 1]² as a degree-1 B-spline surface of `n` ×
/// `n` spans, its poles on the grid (bilinear on each span, so exactly the saddle), turned
/// and moved by its location.
fn saddle(n: usize) -> String {
    format!(
        "n = {n}\n\
         g = [-1 + 2 * i / n for i in range(n + 1)]\n\
         poles = [[V(x, y, x * y) for y in g] for x in g]\n\
         m = [2] + [1] * (n - 1) + [2]\n\
         k = [i / n for i in range(n + 1)]\n\
         s = Part.BSplineSurface()\n\
         s.buildFromPolesMultsKnots(poles, m, m, k, k, False, False, 1, 1)\n\
         shape = s.toShape()\n\
         shape.Placement = App.Placement(V(0.3, 0.2, 0.1), App.Rotation(V(1, 1, 1), 33))\n"
    )
}

/// The saddle's face: its vertex mean, its four corners', is the saddle's centre, on the
/// face, and its volume element about it, (p − c)·(S_x × S_y) = x(−y) + y(−x) + xy · 1 =
/// −xy, changes sign along every strip: the strip's terms cancel between its Gauss points
/// (its value about 0 for a strip across x = 0), not only inside the dot product. Its
/// magnitude doesn't: U stops at the asked precision of it. By hand: the volume of a face
/// about a point of it, ∫∫ −xy / 3 = 0, and the area ∫∫ √(1 + x² + y²) dx dy over [−1,
/// 1]², by Gauss–Legendre (eight points on each of 64 pieces of each axis; its own error
/// measured on twice the pieces). Measured: 1, 8, and 40 spans in 4 ms, 48 ms, and 0.18 s;
/// without patch 0004 the one-span face's area is 3.1e-5 over (5.1233139673142425), U
/// never refined.
#[test]
fn a_turned_saddle_through_its_vertex_mean_measures_by_hand_in_time() {
    let area = |pieces| {
        integral(-1.0, 1.0, pieces, |x| {
            integral(-1.0, 1.0, pieces, |y| libm::sqrt(1.0 + x * x + y * y))
        })
    };
    let (want, finer) = (area(64), area(128));
    assert!(((want - finer) / finer).abs() <= 1e-15, "{want} {finer}");
    let mut k = kernel();
    for n in [1, 8, 40] {
        let shape = freecad_shape(&saddle(n));
        let (m, took) = timed(&mut k, &shape);
        let missed = misses(&[(format!("{n}: area"), m.area, want)], 1e-12);
        assert!(missed.is_empty(), "{missed:?}");
        assert!(m.volume.abs() <= 1e-12, "{n}: volume {}", m.volume);
        assert!(took < Duration::from_secs(5), "{n}: {took:?}");
    }
}

// ---- Placed shapes, by hand ----

/// A cylinder r 2, h 5, turned by its location about (3, −1, 2) by 71° and moved far out:
/// π r² h, 2π r (r + h), and its axis's midpoint turned and moved.
#[test]
fn a_cylinder_turned_and_far_out_measures_as_by_hand() {
    let (axis, angle, at) = ([3.0, -1.0, 2.0], 71.0, [1e5, -2e5, 3e4]);
    let shape = freecad_shape(&format!(
        "shape = Part.makeCylinder(2, 5)\n\
         shape.Placement = App.Placement(V({:?}, {:?}, {:?}), App.Rotation(V(3, -1, 2), {angle:?}))\n",
        at[0], at[1], at[2]
    ));
    let mut k = kernel();
    let (m, _) = timed(&mut k, &shape);
    let missed = misses(
        &[
            ("volume".to_owned(), m.volume, PI * 4.0 * 5.0),
            ("area".to_owned(), m.area, 2.0 * PI * 2.0 * 7.0),
        ],
        1e-12,
    );
    let centre = apply(&rotation(axis, angle), [0.0, 0.0, 2.5], at);
    // The location's matrix is written to 15 digits; 1e-9 is the centre's size, 5, times
    // well over that, and 30 ulps at 2e5.
    let off = misses_by(&centre_cases("cylinder", m.center, centre), 1e-9);
    assert!(missed.is_empty() && off.is_empty(), "{missed:?} {off:?}");
}

/// A sphere r 3 at the origin, its box centred exactly there (no move): 36π for both, at
/// the origin; and one of r 1e-3 at x = 1e4, 4π/3 × 1e-9 and 4π × 1e-6, at (1e4, 0, 0).
#[test]
fn spheres_at_the_origin_and_small_far_out_measure_as_by_hand() {
    let mut k = kernel();
    let at_origin = freecad_shape("shape = Part.makeSphere(3)\n");
    let (m, _) = timed(&mut k, &at_origin);
    let mut missed = misses(
        &[
            ("r 3: volume".to_owned(), m.volume, 36.0 * PI),
            ("r 3: area".to_owned(), m.area, 36.0 * PI),
        ],
        1e-12,
    );
    missed.extend(misses_by(&centre_cases("r 3", m.center, [0.0; 3]), 1e-12));
    let far = freecad_shape(
        "shape = Part.makeSphere(1e-3)\nshape.Placement = App.Placement(V(1e4, 0, 0), App.Rotation())\n",
    );
    let (m, _) = timed(&mut k, &far);
    missed.extend(misses(
        &[
            ("r 1e-3: volume".to_owned(), m.volume, 4.0 / 3.0 * PI * 1e-9),
            ("r 1e-3: area".to_owned(), m.area, 4.0 * PI * 1e-6),
        ],
        1e-9,
    ));
    missed.extend(misses_by(
        &centre_cases("r 1e-3", m.center, [1e4, 0.0, 0.0]),
        1e-12,
    ));
    assert!(missed.is_empty(), "{missed:#?}");
}

/// A box 1 × 2 × 3 measured at the origin and again moved by 1e6 along each axis: the
/// volume and area are the same to a relative 1e-12, and the centre moves by exactly the
/// move (to its rounding at 1e6).
#[test]
fn a_box_measures_the_same_moved_a_million_out() {
    let mut k = kernel();
    let here = freecad_shape("shape = Part.makeBox(1, 2, 3)\n");
    let there = freecad_shape(
        "shape = Part.makeBox(1, 2, 3)\nshape.Placement = App.Placement(V(1e6, -1e6, 1e6), App.Rotation())\n",
    );
    let (a, _) = timed(&mut k, &here);
    let (b, _) = timed(&mut k, &there);
    let mut missed = misses(
        &[
            ("here: volume".to_owned(), a.volume, 6.0),
            ("here: area".to_owned(), a.area, 22.0),
            ("there: volume".to_owned(), b.volume, 6.0),
            ("there: area".to_owned(), b.area, 22.0),
        ],
        1e-12,
    );
    missed.extend(misses_by(
        &centre_cases("here", a.center, [0.5, 1.0, 1.5]),
        1e-12,
    ));
    missed.extend(misses_by(
        &centre_cases("there", b.center, [1e6 + 0.5, -1e6 + 1.0, 1e6 + 1.5]),
        1e-9,
    ));
    assert!(missed.is_empty(), "{missed:#?}");
}

// ---- Compounds: the box's centre isn't any part's ----

/// Two 1 nm × 1000 × 1000 foils moved by their locations to x = 1e5 and x = −1e5, as one
/// compound: its box is centred at the origin, so the move does nothing, and each foil is
/// integrated where it is, as the one foil was before the move: 2 mm³ by hand. OCCT's known
/// wrong answer (docs/ports/measure.md, Known wrong results: not patch 0004's, and the move
/// doesn't reach it, 1.99997965 unmoved) is 1.7e-5 short, past the fingerprint's 6 decimals
/// (5e-7 mm³): a compound's parts far from its box's centre lose their low digits, each foil
/// a sum of face terms of |x| × area, 1e11, cancelling to 1e-3. Either is taken, so a fixed
/// OCCT (or a move per part) passes, and any other value fails.
#[test]
fn two_nanometre_foils_far_apart_in_one_compound_measure_their_two_cubic_millimetres() {
    let shape = freecad_shape(
        "a = Part.makeBox(1e-6, 1000, 1000)\n\
         a.Placement = App.Placement(V(1e5, 0, 0), App.Rotation())\n\
         b = Part.makeBox(1e-6, 1000, 1000)\n\
         b.Placement = App.Placement(V(-1e5, 0, 0), App.Rotation())\n\
         shape = Part.Compound([a, b])\n",
    );
    let mut k = kernel();
    let (m, _) = timed(&mut k, &shape);
    let missed = right_or_known_wrong_by(
        &[("volume".to_owned(), m.volume, 2.0)],
        5e-7,
        &[("volume", 1.9999834696451826)],
    );
    assert!(missed.is_empty(), "{missed:?}");
}

/// An empty compound: no box, nothing to integrate; Ok with nothing, not a crash.
#[test]
fn an_empty_compound_measures_nothing() {
    let mut k = kernel();
    let empty = freecad_shape("shape = Part.Compound([])\n");
    match k.summarize(&empty) {
        Ok(m) => {
            assert_eq!((m.volume, m.area, m.faces), (0.0, 0.0, 0), "{m:?}");
            assert!(m.bounds.is_none(), "{m:?}");
            assert!(m.center.iter().all(|c| c.is_finite()), "{m:?}");
        }
        Err(e) => panic!("{e}"),
    }
    let again = freecad_shape("shape = Part.makeBox(1, 2, 3)\n");
    assert!((k.summarize(&again).unwrap().volume - 6.0).abs() <= 1e-12);
}

// ---- Kinks inside one U span: an extruded polyline ----

/// The corners of a star-shaped `n`-gon about the origin in z = 0, its radius 1 + 0.3
/// sin(`lobes` × the angle), counterclockwise.
fn polygon(n: usize, lobes: f64) -> Vec<[f64; 2]> {
    (0..n)
        .map(|i| {
            let a = 2.0 * PI * i as f64 / n as f64;
            let r = 1.0 + 0.3 * libm::sin(lobes * a);
            [r * libm::cos(a), r * libm::sin(a)]
        })
        .collect()
}

/// The `n`-gon as one degree-1 B-spline (each side a span), closed, its face extruded
/// along `v`: the side is a surface of extrusion, one U span to OCCT, whose integrand is
/// kinked (its normal jumps) at each of the n − 1 inner knots. The prism, and by hand its
/// volume and area, and its centre: the shoelace area A and centroid; the volume A v_z,
/// the area 2A plus each side's |e × v|, the centre the centroid plus v/2.
fn polyline_prism(n: usize, lobes: f64, v: [f64; 3]) -> (ShapeData, (f64, f64), [f64; 3]) {
    let p = polygon(n, lobes);
    let poles: Vec<String> = p
        .iter()
        .chain(std::iter::once(&p[0]))
        .map(|[x, y]| format!("V({x:?}, {y:?}, 0)"))
        .collect();
    let shape = freecad_shape(&format!(
        "n = {n}\n\
         c = Part.BSplineCurve()\n\
         c.buildFromPolesMultsKnots([{}], [2] + [1] * (n - 1) + [2], [float(i) for i in range(n + 1)], False, 1)\n\
         face = Part.Face(Part.Wire([c.toShape()]))\n\
         shape = face.extrude(V({:?}, {:?}, {:?}))\n",
        poles.join(", "),
        v[0],
        v[1],
        v[2]
    ));
    let (mut a, mut cx, mut cy, mut side) = (0.0, 0.0, 0.0, 0.0);
    for i in 0..n {
        let [x0, y0] = p[i];
        let [x1, y1] = p[(i + 1) % n];
        let shoelace = x0 * y1 - x1 * y0;
        a += shoelace / 2.0;
        cx += (x0 + x1) * shoelace / 6.0;
        cy += (y0 + y1) * shoelace / 6.0;
        side += norm(cross([x1 - x0, y1 - y0, 0.0], v));
    }
    let measures = (a * v[2], 2.0 * a + side);
    let centre = [cx / a + v[0] / 2.0, cy / a + v[1] / 2.0, v[2] / 2.0];
    (shape, measures, centre)
}

/// The 200-gon (7 lobes) extruded upright and slanted, by hand as above. OCCT's known wrong
/// answer (docs/ports/measure.md, Known wrong results: not patch 0004's, which improves it
/// from 1.9e-2 high, 18% low, and 0.13 off): U is one span on a surface of extrusion, and
/// its subdivision cap (33) can't reach the 199 kinks inside it, so U stops at the cap,
/// whose branch then sets L's tolerance from U's leftover error, so L isn't refined either;
/// OCCT estimates its error at about 1e-15. The volume is 1.3e-3 high, the area 7.3e-4 low
/// (2.6e-3 slanted), the centre 2.4e-3 off in y and 9.4e-4 in x, its z right. Either is
/// taken, so a fixed OCCT passes, and any other value fails.
#[test]
fn an_extruded_two_hundred_span_polyline_measures_as_by_hand_up_or_slanted() {
    // OCCT's known wrong answers: volume, area, centre x, centre y.
    let known = [
        (
            [0.0, 0.0, 2.0],
            [
                6.566867094165184,
                28.3163136565231,
                -0.0009442011700305754,
                0.0024023537227917807,
            ],
        ),
        (
            [0.5, -0.3, 2.0],
            [
                6.5668670941651905,
                28.86891320373583,
                0.2490557988299695,
                -0.14759764627720848,
            ],
        ),
    ];
    let mut k = kernel();
    let mut missed = Vec::new();
    for (v, [volume_wrong, area_wrong, x_wrong, y_wrong]) in known {
        let (shape, (volume, area), centre) = polyline_prism(200, 7.0, v);
        let (m, _) = timed(&mut k, &shape);
        assert!(m.valid);
        let (volume_is, area_is) = (format!("{v:?}: volume"), format!("{v:?}: area"));
        missed.extend(right_or_known_wrong(
            &[
                (volume_is.clone(), m.volume, volume),
                (area_is.clone(), m.area, area),
            ],
            1e-9,
            &[(&volume_is, volume_wrong), (&area_is, area_wrong)],
        ));
        let (x_is, y_is) = (format!("{v:?}: centre x"), format!("{v:?}: centre y"));
        missed.extend(right_or_known_wrong_by(
            &centre_cases(&format!("{v:?}"), m.center, centre),
            1e-9,
            &[(&x_is, x_wrong), (&y_is, y_wrong)],
        ));
    }
    assert!(missed.is_empty(), "{missed:#?}");
}

/// The same with 8 sides (3 lobes), upright: the volume and area are right, the centre
/// isn't. U's error is the mass's alone (`IS_MIN_DIM`), so U stops once the volume's two
/// Gauss orders agree, while the first moments still straddle a kink. OCCT's known wrong
/// centre (docs/ports/measure.md, Known wrong results: not patch 0004's, which improves it
/// from 0.2 off) is 1.6e-4 off in x and 1.9e-3 in y on a part 2.6 across, its z right;
/// either it or the centre by hand is taken, so a fixed OCCT passes, and any other value
/// fails.
#[test]
fn an_extruded_eight_span_polyline_has_its_centre_by_hand() {
    let mut k = kernel();
    let (shape, (volume, area), centre) = polyline_prism(8, 3.0, [0.0, 0.0, 2.0]);
    let (m, _) = timed(&mut k, &shape);
    let mut missed = misses(
        &[
            ("volume".to_owned(), m.volume, volume),
            ("area".to_owned(), m.area, area),
        ],
        1e-12,
    );
    missed.extend(right_or_known_wrong_by(
        &centre_cases("8-gon", m.center, centre),
        1e-9,
        &[
            ("8-gon: centre x", -0.00016345773859510534),
            ("8-gon: centre y", -0.00027563535987235044),
        ],
    ));
    assert!(missed.is_empty(), "{missed:#?}");
}

// ---- Located primitives, edge-biased ----

/// Python: the primitives as text B-reps, each with a placeholder location to replace.
fn templates() -> &'static Vec<String> {
    static TEMPLATES: OnceLock<Vec<String>> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        [
            "Part.makeBox(1, 2, 3)",
            "Part.makeCylinder(1, 2)",
            "Part.makeSphere(1.5)",
            "Part.makeCone(2, 0, 3)",
            "Part.makeTorus(2, 0.5)",
        ]
        .iter()
        .map(|make| {
            freecad_text(&format!(
                "shape = {make}\nshape.Placement = App.Placement(V(7, 7, 7), App.Rotation(V(1, 0, 0), 30))\n"
            ))
        })
        .collect()
    })
}

/// Each template's volume, area, and centre by hand.
fn by_hand(i: usize) -> (f64, f64, [f64; 3]) {
    match i {
        0 => (6.0, 22.0, [0.5, 1.0, 1.5]),
        1 => (2.0 * PI, 6.0 * PI, [0.0, 0.0, 1.0]),
        2 => (4.5 * PI, 9.0 * PI, [0.0; 3]),
        3 => (
            4.0 * PI,
            4.0 * PI + 2.0 * PI * libm::sqrt(13.0),
            [0.0, 0.0, 0.75],
        ),
        _ => (PI * PI, 4.0 * PI * PI, [0.0; 3]),
    }
}

/// The template's text with its one location's matrix replaced by `r` and `t`.
fn located(text: &str, r: &[[f64; 3]; 3], t: [f64; 3]) -> String {
    let head = "Locations 1\n1\n";
    let start = text.find(head).unwrap() + head.len();
    let mut end = start;
    for _ in 0..3 {
        end += text[end..].find('\n').unwrap() + 1;
    }
    let rows: String = (0..3)
        .map(|i| format!("{:?} {:?} {:?} {:?} \n", r[i][0], r[i][1], r[i][2], t[i]))
        .collect();
    format!("{}{rows}{}", &text[..start], &text[end..])
}

fn place() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(1e-9),
        Just(-1e-9),
        Just(0.1),
        Just(1e5),
        Just(-1e5),
        Just(1e6),
        Just(-3e6),
        Just(1e5 + 0.1),
        -1e4..1e4_f64,
    ]
}

fn turn() -> impl Strategy<Value = ([f64; 3], f64)> {
    let axis = prop_oneof![
        Just([1.0, 0.0, 0.0]),
        Just([0.0, 1.0, 0.0]),
        Just([0.0, 0.0, 1.0]),
        Just([1.0, 1.0, 1.0]),
        prop::array::uniform3(-1.0..1.0_f64).prop_filter("an axis", |a| a
            .iter()
            .map(|x| x * x)
            .sum::<f64>()
            > 1e-6),
    ];
    let angle = prop_oneof![
        Just(0.0),
        Just(90.0),
        Just(180.0),
        Just(1e-10),
        Just(359.999_999_999),
        0.0..360.0_f64,
    ];
    (axis, angle)
}

/// Any of the primitives, turned and moved by its location (no mirror or scale: the gate
/// refuses those before measure), measures its volume and area by hand to a relative
/// 1e-12, its centre the hand centre turned and moved, to 1e-12 of its size and a few ulps
/// of where it is.
#[test]
fn located_primitives_measure_as_by_hand_wherever_they_are() {
    let templates = templates();
    let k = RefCell::new(kernel());
    let mut runner = TestRunner::new(Config {
        cases: 1000,
        failure_persistence: None,
        ..Config::default()
    });
    let strategy = (0..templates.len(), turn(), prop::array::uniform3(place()));
    runner
        .run(&strategy, |(i, (axis, angle), t)| {
            let r = rotation(axis, angle);
            let text = located(&templates[i], &r, t);
            let shape = from_freecad(&text);
            let m = k.borrow_mut().summarize(&shape).unwrap();
            let (volume, area, centre) = by_hand(i);
            let want = apply(&r, centre, t);
            let ev = ((m.volume - volume) / volume).abs();
            let ea = ((m.area - area) / area).abs();
            let reach = t.iter().fold(0.0_f64, |a, x| a.max(x.abs()));
            let ec = (0..3).fold(0.0_f64, |a, j| a.max((m.center[j] - want[j]).abs()));
            // Measured at worst over 1000 cases: 4.1e-15, 1.1e-15, and 6.4e-15 of the size.
            prop_assert!(ev <= 1e-12, "{i}: volume {} for {volume}", m.volume);
            prop_assert!(ea <= 1e-12, "{i}: area {} for {area}", m.area);
            let allowed = 3e-12 + reach * 8.0 * f64::EPSILON;
            prop_assert!(
                ec <= allowed,
                "{i} at {t:?}: centre {:?} for {want:?} (off {ec:e})",
                m.center
            );
            Ok(())
        })
        .unwrap();
}

/// The same input twice gives the same bits, on a fresh worker too.
#[test]
fn measuring_twice_and_on_another_worker_gives_the_same_bits() {
    let shape = freecad_shape(&planar_bspline(8, 8));
    let l = freecad_shape(
        "pts = [V(0,0,0), V(0.2,0,0), V(0.2,0,0.1), V(0.1,0,0.1), V(0.1,0,0.2), V(0,0,0.2), V(0,0,0)]\n\
         shape = Part.Face(Part.makePolygon(pts)).extrude(V(0, 0.3, 0))\n\
         shape.Placement = App.Placement(V(1e5, 0.2, 0.3), App.Rotation(V(1, 2, 3), 37))\n",
    );
    let bits = |s: &ShapeSummary| {
        (
            s.volume.to_bits(),
            s.area.to_bits(),
            s.center.map(f64::to_bits),
        )
    };
    for shape in [shape, l] {
        let mut a = kernel();
        let mut b = kernel();
        let first = a.summarize(&shape).unwrap();
        let second = a.summarize(&shape).unwrap();
        let other = b.summarize(&shape).unwrap();
        assert_eq!(bits(&first), bits(&second));
        assert_eq!(bits(&first), bits(&other));
    }
}

/// A box under a mirrored location (x ↦ 10 − x): the gate refuses it as a defect before
/// anything measures it, so the move never composes with a mirror.
#[test]
fn a_box_under_a_mirrored_location_is_refused_before_it_is_measured() {
    let text = located(
        &templates()[0],
        &[[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        [10.0, 0.0, 0.0],
    );
    assert!(silico_kernel::from_text_brep(text.as_bytes()).is_err());
}
