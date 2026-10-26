//! /break-it on OCCT patch 0004 (`BRepGProp_Gauss::Compute` refines U: `EpsU` from the
//! asked precision and the strip's value) and on `measure` integrating a shape moved by
//! minus its tight box's centre (bridge.cpp). Every value here is by hand (analytic, the
//! shoelace formula, Rodrigues' rotation), and every measure goes through a kernel worker.
//!
//! Attacked: faces whose strips integrate to about nothing (a plane through the point the
//! volume is integrated about, where `EpsU` is 0 or round-off and U can only stop at its
//! cap), B-spline faces of many spans (the cap's cost, and past `SM` spans), integrands
//! kinked inside one U span (an extruded polyline), located and rotated shapes far out,
//! shapes centred exactly at the origin, compounds whose box centre is nowhere near their
//! parts, an empty compound, and the worker's 60 s timeout.

use super::adversarial_revolution_measures::from_freecad;
use super::oracle;
use super::quadrature::{misses, misses_by};

use std::cell::RefCell;
use std::f64::consts::PI;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use silico_core::kernel::KernelWorker;
use silico_protocol::{ShapeData, ShapeSummary};

fn worker() -> KernelWorker {
    KernelWorker::start_worker(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap()
}

/// The text B-rep of `shape`, as FreeCAD makes it from `body` (Python that sets `shape`).
fn freecad_text(body: &str) -> String {
    let script = format!(
        "import json, math, Part, FreeCAD as App\nV = App.Vector\n{body}\n\
         print(\"SILICO_ORACLE \" + json.dumps(shape.exportBrepToString()))\n"
    );
    oracle::freecad(&script).as_str().unwrap().to_owned()
}

fn freecad_shape(body: &str) -> ShapeData {
    from_freecad(&freecad_text(body))
}

/// `shape` summarized through `k`, and how long that took.
fn timed(k: &mut KernelWorker, shape: &ShapeData) -> (ShapeSummary, Duration) {
    let start = Instant::now();
    let s = k.summarize(shape).unwrap();
    (s, start.elapsed())
}

/// Rodrigues: the rotation by `degrees` about `axis` (not unit).
fn rotation(axis: [f64; 3], degrees: f64) -> [[f64; 3]; 3] {
    let n = libm::sqrt(axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]);
    let [x, y, z] = axis.map(|a| a / n);
    let (s, c) = (
        libm::sin(degrees.to_radians()),
        libm::cos(degrees.to_radians()),
    );
    let t = 1.0 - c;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

fn apply(r: &[[f64; 3]; 3], p: [f64; 3], t: [f64; 3]) -> [f64; 3] {
    let mut out = t;
    for (i, row) in r.iter().enumerate() {
        out[i] += row[0] * p[0] + row[1] * p[1] + row[2] * p[2];
    }
    out
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
    let mut k = worker();
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
    let mut k = worker();
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
    let mut k = worker();
    let (m, took) = timed(&mut k, &shape);
    assert!((m.area - 1.0).abs() <= 1e-12, "{m:?}");
    assert!(m.volume.abs() <= 1e-12, "{m:?}");
    // BREAKS: 59.96 s through the worker alone, and past its 60 s timeout (kernel_failed,
    // "didn't finish Summarize") beside other tests; 0.245 s without patch 0004
    // (58.8 s with it, the same OCCT otherwise, measured outside the worker). Every strip
    // is round-off, so EpsU = 0.9 |strip × 1e-9| is below what U's two Gauss orders can
    // agree to, and U refines to its cap (1921 subdivisions) at every L point.
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
    let mut k = worker();
    for n in [20, 60] {
        let shape = freecad_shape(&l_with_bspline_inner_faces(n));
        let start = Instant::now();
        let got = k.summarize(&shape);
        let took = start.elapsed();
        // BREAKS: 20 × 20 takes 29.6 s (0.19 s without patch 0004), and 60 × 60 122.8 s
        // (0.48 s without it), so through the worker it times out after 60 s: the solid's
        // summary, its fingerprint, and any feature on it fail. The faces' strips are
        // round-off about the vertex mean, as the plain L's, which costs 79 ms for 8 ms.
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

/// A square B-spline face of 2000 U spans, past `SM` (32 × 61 + 1 = 1953), where U's cap
/// is below its spans: OCCT drops the spans past the cap, and patch 0004's `JU >= iUSubEnd`
/// keeps refining past the cap's index, which it then never meets again.
#[test]
fn a_planar_bspline_face_of_two_thousand_u_spans_measures_its_area() {
    let shape = freecad_shape(&planar_bspline(2000, 1));
    let mut k = worker();
    let got = k.summarize(&shape);
    // BREAKS: with patch 0004, kernel_failed "Standard_OutOfRange:
    // NCollection_Array1::ChangeValue" after 8.9 s (U refines past its arrays: the cap
    // `JU == UMaxSubs` is passed before `JU >= iUSubEnd` starts refining, so it never
    // ends it). Without the patch, Ok with an area of 0.9995 (the span past `SM` dropped).
    let m = got.unwrap();
    assert!((m.area - 1.0).abs() <= 1e-9, "{m:?}");
    assert!(m.volume.abs() <= 1e-9, "{m:?}");
}

/// The worker is well after the refusal above.
#[test]
fn the_worker_measures_a_box_after_the_two_thousand_span_face_fails() {
    let mut k = worker();
    let _ = k.summarize(&freecad_shape(&planar_bspline(2000, 1)));
    let again = freecad_shape("shape = Part.makeBox(1, 2, 3)\n");
    let m = k.summarize(&again).unwrap();
    assert!(
        (m.volume - 6.0).abs() <= 1e-12 && (m.area - 22.0).abs() <= 1e-12,
        "{m:?}"
    );
}

/// That face (unit square, 2000 U spans) extruded 1: a unit cube, valid, 1 and 6 by hand.
#[test]
fn a_prism_of_a_two_thousand_span_bspline_face_is_a_unit_cube() {
    let shape = freecad_shape(&format!(
        "{}shape.Placement = App.Placement()\nshape = shape.extrude(V(0, 0, 1))\nassert shape.isValid()\n",
        planar_bspline(2000, 1)
    ));
    let mut k = worker();
    let m = k.summarize(&shape).unwrap();
    assert!(m.valid);
    let missed = misses(
        &[
            ("volume".to_owned(), m.volume, 1.0),
            ("area".to_owned(), m.area, 6.0),
        ],
        1e-9,
    );
    // BREAKS (not patch 0004's: the same without it): 0.99983 and 5.999, with OCCT's
    // estimate 2e-13. `BRepGProp_Gauss` caps U at `SM` = 1953 subdivisions and zeroes the
    // spans past it (`JU == UMaxSubs` while `kUEnd == 1`), so the 47 last spans of each
    // strip are left out.
    assert!(missed.is_empty(), "{missed:?}");
}

/// Plane faces infinite both ways and one way (u from 0 to OCCT's infinity, 2e100): no
/// finite box to move to, nothing finite to integrate. Refused as data, when read or when
/// measured; never a crash, a NaN, or an area of 1e100.
#[test]
fn infinite_plane_faces_are_refused() {
    let mut k = worker();
    let mut nonsense = Vec::new();
    for (what, make) in [
        ("both ways", "shape = Part.Plane().toShape()\n"),
        ("one way", "shape = Part.Plane().toShape(0, 2e100, 0, 1)\n"),
    ] {
        let text = freecad_text(make);
        match silico_kernel::from_text_brep(text.as_bytes()) {
            Err(_) => {}
            Ok(shape) => {
                let data = ShapeData::checked(
                    silico_protocol::Blob::inline(silico_kernel::to_brep(&shape).unwrap()),
                    shape.is_valid().unwrap(),
                );
                let got = k.summarize(&data);
                if let Ok(m) = got {
                    nonsense.push(format!("{what}: {m:?}"));
                }
            }
        }
    }
    // BREAKS (not patch 0004's or the move's: the same unmoved): Ok, both ways with an
    // area of 2e100 and a box of ±1e100, one way with an area of 0, valid by `BRepCheck`; the gate (`brep_defect`) takes infinite
    // surfaces. Silico's own features never make one; an import or a cache file could.
    assert!(nonsense.is_empty(), "{nonsense:#?}");
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
    let mut k = worker();
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
    let mut k = worker();
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
    let mut k = worker();
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
/// integrated where it is, as the one foil was before the move: 2 mm³ by hand.
#[test]
fn two_nanometre_foils_far_apart_in_one_compound_measure_their_two_cubic_millimetres() {
    let shape = freecad_shape(
        "a = Part.makeBox(1e-6, 1000, 1000)\n\
         a.Placement = App.Placement(V(1e5, 0, 0), App.Rotation())\n\
         b = Part.makeBox(1e-6, 1000, 1000)\n\
         b.Placement = App.Placement(V(-1e5, 0, 0), App.Rotation())\n\
         shape = Part.Compound([a, b])\n",
    );
    let mut k = worker();
    let (m, _) = timed(&mut k, &shape);
    // BREAKS (the move doesn't reach it; it doesn't make it worse: 1.99997965 unmoved):
    // 1.99998347, 1.7e-5 short, past the fingerprint's 6 decimals (5e-7 mm³). A compound's
    // parts far from its box's centre lose their low digits as the one foil did before.
    assert!((m.volume - 2.0).abs() <= 5e-7, "volume {}", m.volume);
}

/// An empty compound: no box, nothing to integrate; Ok with nothing, not a crash.
#[test]
fn an_empty_compound_measures_nothing() {
    let mut k = worker();
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

/// A star-shaped 200-gon as one degree-1 B-spline (each side a span), closed, its face
/// extruded along `v`: the side is a surface of extrusion, one U span to OCCT, whose
/// integrand is kinked (its normal jumps) at each of the 199 inner knots. By hand: the
/// shoelace area A and centroid; the volume A v_z, the area 2A plus each side's |e × v|,
/// the centre the centroid plus v/2.
fn polygon(n: usize, lobes: f64) -> Vec<[f64; 2]> {
    (0..n)
        .map(|i| {
            let a = 2.0 * PI * i as f64 / n as f64;
            let r = 1.0 + 0.3 * libm::sin(lobes * a);
            [r * libm::cos(a), r * libm::sin(a)]
        })
        .collect()
}

fn polyline_prism(n: usize, lobes: f64, v: [f64; 3]) -> (ShapeData, [f64; 3], [f64; 3]) {
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
        let cross = x0 * y1 - x1 * y0;
        a += cross / 2.0;
        cx += (x0 + x1) * cross / 6.0;
        cy += (y0 + y1) * cross / 6.0;
        let e = [x1 - x0, y1 - y0, 0.0];
        let c = [
            e[1] * v[2] - e[2] * v[1],
            e[2] * v[0] - e[0] * v[2],
            e[0] * v[1] - e[1] * v[0],
        ];
        side += libm::sqrt(c[0] * c[0] + c[1] * c[1] + c[2] * c[2]);
    }
    let measures = [a * v[2], 2.0 * a + side, 0.0];
    let centre = [cx / a + v[0] / 2.0, cy / a + v[1] / 2.0, v[2] / 2.0];
    (shape, measures, centre)
}

#[test]
fn an_extruded_two_hundred_span_polyline_measures_as_by_hand_up_or_slanted() {
    let mut k = worker();
    let mut missed = Vec::new();
    for v in [[0.0, 0.0, 2.0], [0.5, -0.3, 2.0]] {
        let (shape, [volume, area, _], centre) = polyline_prism(200, 7.0, v);
        let (m, _) = timed(&mut k, &shape);
        assert!(m.valid);
        missed.extend(misses(
            &[
                (format!("{v:?}: volume"), m.volume, volume),
                (format!("{v:?}: area"), m.area, area),
            ],
            1e-9,
        ));
        missed.extend(misses_by(
            &centre_cases(&format!("{v:?}"), m.center, centre),
            1e-9,
        ));
    }
    // BREAKS (improved by patch 0004, not mended): volume 1.3e-3 high, area 7.3e-4 low
    // (2.6e-3 slanted), centre 2.4e-3 off, each with OCCT's estimate about 1e-15. U is one
    // span on a surface of extrusion; its cap (33 subdivisions) can't reach 199 kinks, and
    // the cap's branch then sets L's tolerance from U's leftover error, so L isn't refined
    // either. Without the patch: volume 1.9e-2 high, area 18% low, centre 0.13 off.
    assert!(missed.is_empty(), "{missed:#?}");
}

/// The same with 8 sides (3 lobes), upright: the volume and area are right, the centre
/// isn't. U's error is the mass's alone (`IS_MIN_DIM`), so U stops once the volume's two
/// Gauss orders agree, while the first moments still straddle a kink.
#[test]
fn an_extruded_eight_span_polyline_has_its_centre_by_hand() {
    let mut k = worker();
    let (shape, [volume, area, _], centre) = polyline_prism(8, 3.0, [0.0, 0.0, 2.0]);
    let (m, _) = timed(&mut k, &shape);
    let mut missed = misses(
        &[
            ("volume".to_owned(), m.volume, volume),
            ("area".to_owned(), m.area, area),
        ],
        1e-12,
    );
    // BREAKS (improved by patch 0004, from 0.2 off): the centre is 1.9e-3 off in y and
    // 1.6e-4 in x on a part 2.6 across, the volume and area right to 1e-15.
    missed.extend(misses_by(&centre_cases("8-gon", m.center, centre), 1e-9));
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
    let k = RefCell::new(worker());
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
        let mut a = worker();
        let mut b = worker();
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
