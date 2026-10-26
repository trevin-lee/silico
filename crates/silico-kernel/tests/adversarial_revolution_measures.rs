//! break-it on OCCT patch 0003 (`BRepGProp_Face` gives a `GeomAbs_SurfaceOfRevolution`
//! three U spans, as a torus): hostile surfaces of revolution measured through a kernel
//! worker against values by hand, independent of OCCT (closed forms, Pappus, and
//! Gauss–Legendre on smooth integrands to convergence): an arc whose circle is tangent to
//! the axis, one nearly centred on it, turns of 1° and 359.999° and at the knots the patch
//! adds, a C⁰ B-spline meridian, a surface of revolution under a
//! `Geom_RectangularTrimmedSurface`, and the barrel fused to a box and filleted along the
//! seam; with lines skew to the axis (hyperboloids), ellipses, rational B-splines, faces
//! whose U starts off 0, and a property over barrels of every proportion. Arcs here keep
//! clear of their circle's angle 0, where OCCT 7.9.3's booleans ignore the turned face
//! (docs/ports/measure.md, Known wrong results: a sketch arc across its circle's angle 0),
//! except where said. The tests that need FreeCAD to make a face (`pixi install -e
//! oracle`) say so.

use super::close::assert_close;
use super::kernel::kernel;
use super::numbers::relative_error;
use super::oracle;
use super::quadrature::{bezier, cap_integral, integral, kind, misses, right_or_known_wrong};
use super::revolution_measures::barrel_by_hand;

use std::cell::RefCell;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::kernel::KernelWorker;
use silico_kernel::{Axis, BooleanOp, Curve, FaceRegions, Region, SketchPlane};
use silico_protocol::{Blob, Primitive, ShapeData};

/// OCCT's precise measure's promise: a relative 1e-9 (`MEASURE_PRECISION`).
const ASKED: f64 = 1e-9;

fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
    Curve::Line { start, end }
}

/// An arc of the circle of `radius` about `center` from angle `a0` to `a1` (counter-clockwise
/// where `a1` is the larger), its ends at `start` and `end` as given (exact where the
/// caller has them exactly).
fn arc(center: [f64; 2], radius: f64, start: [f64; 2], end: [f64; 2], a0: f64, a1: f64) -> Curve {
    Curve::Arc {
        center,
        start,
        end,
        radius,
        start_angle: a0,
        end_angle: a1,
    }
}

/// On the circle about `center` of `radius`, at `angle`.
fn on(center: [f64; 2], radius: f64, angle: f64) -> [f64; 2] {
    [
        center[0] + radius * libm::cos(angle),
        center[1] + radius * libm::sin(angle),
    ]
}

/// One region on XZ: the sketch's x is the model's x and its y the model's z.
fn region(outer: Vec<Curve>) -> FaceRegions {
    FaceRegions {
        plane: SketchPlane::Xz,
        regions: vec![Region {
            outer,
            holes: vec![],
        }],
    }
}

const ABOUT_Z: Axis = Axis {
    origin: [0.0; 3],
    direction: [0.0, 0.0, 1.0],
};
const ABOUT_X: Axis = Axis {
    origin: [0.0; 3],
    direction: [1.0, 0.0, 0.0],
};

fn turn(
    k: &mut KernelWorker,
    regions: &FaceRegions,
    axis: Axis,
    start: f64,
    angle: f64,
) -> Result<ShapeData, silico_core::Error> {
    let faces = k.make_faces(regions)?;
    Ok(k.revolution(&faces.shape, axis, start, angle)?.shape)
}

fn kinds(k: &mut KernelWorker, shape: &ShapeData) -> Vec<&'static str> {
    k.describe(shape)
        .unwrap()
        .faces
        .iter()
        .map(|f| kind(&f.surface))
        .collect()
}

/// A shape made by FreeCAD from `script`'s printed text B-rep, as a kernel worker's shape,
/// its validity OCCT's.
pub fn from_freecad(text: &str) -> ShapeData {
    let shape = silico_kernel::from_text_brep(text.as_bytes()).unwrap();
    let valid = shape.is_valid().unwrap();
    ShapeData::checked(Blob::inline(silico_kernel::to_brep(&shape).unwrap()), valid)
}

/// A revolution OCCT couldn't build, refused as such, with advice that names the axis.
fn assert_refused(e: &silico_core::Error) {
    assert_eq!(e.code(), ErrorCode::KernelFailed, "{e}");
    assert!(e.suggestion().is_some_and(|s| s.contains("axis")), "{e}");
}

// ---- The barrel about x (its arc from 62° to 118°, clear of angle 0) ----

/// A barrel about the model's x: x = ±w from z = `inner` up to the arc of the circle about
/// (0, c) of radius r between its points at angles a and π − a. The general form of
/// revolution_measures.rs's barrel_about_x (c = −3.5, r = 8.5, a = atan2(7.5, 4)).
#[derive(Debug, Clone, Copy)]
struct Barrel {
    c: f64,
    r: f64,
    a: f64,
    inner: f64,
}

/// revolution_measures.rs's barrel: its arc's ends at (±4, 4), at angles atan2(7.5, 4)
/// and π less that from its centre.
fn barrel() -> Barrel {
    Barrel {
        c: -3.5,
        r: 8.5,
        a: libm::atan2(7.5, 4.0),
        inner: 1.0,
    }
}

impl Barrel {
    fn ends(self) -> ([f64; 2], [f64; 2]) {
        let right = on([0.0, self.c], self.r, self.a);
        let left = [-right[0], right[1]];
        (right, left)
    }

    fn regions(self) -> FaceRegions {
        let (right, left) = self.ends();
        let w = right[0];
        region(vec![
            line([-w, self.inner], [w, self.inner]),
            line([w, self.inner], right),
            arc([0.0, self.c], self.r, right, left, self.a, PI - self.a),
            line(left, [-w, self.inner]),
        ])
    }

    /// Its volume turned `angle`: angle/2 ∫ (y(x)² − inner²) dx over [−w, w], y = c + √(r² − x²):
    /// angle/2 [2w (c² + r² − inner²) − 2w³/3 + 2c (w √(r² − w²) + r² asin(w/r))].
    fn volume(self, angle: f64) -> f64 {
        let (right, _) = self.ends();
        let w = right[0];
        let Barrel { c, r, inner, .. } = self;
        let root = w * libm::sqrt(r * r - w * w) + r * r * libm::asin(w / r);
        angle / 2.0
            * (2.0 * w * (c * c + r * r - inner * inner) - 2.0 * w * w * w / 3.0 + 2.0 * c * root)
    }

    /// The profile's area: ∫ (y − inner) dx.
    fn profile(self) -> f64 {
        let (right, _) = self.ends();
        let w = right[0];
        let Barrel { c, r, inner, .. } = self;
        2.0 * w * (c - inner) + w * libm::sqrt(r * r - w * w) + r * r * libm::asin(w / r)
    }

    /// The solid's area turned `angle`: the arc's side by Pappus (its length 2r(π/2 − a)
    /// times its centroid's turn, the centroid r sin(π/2 − a)/(π/2 − a) above the centre),
    /// the bore, the two end rings, and, short of a whole turn, the profile at each end.
    fn area(self, angle: f64, whole: bool) -> f64 {
        let (right, _) = self.ends();
        let (w, top) = (right[0], right[1]);
        let Barrel { c, r, inner, a } = self;
        let half = FRAC_PI_2 - a;
        let side = angle * (2.0 * r * half * c + 2.0 * r * r * libm::sin(half));
        let bore = angle * inner * 2.0 * w;
        let rings = 2.0 * angle / 2.0 * (top * top - inner * inner);
        side + bore + rings + if whole { 0.0 } else { 2.0 * self.profile() }
    }
}

#[test]
fn the_general_barrel_is_revolution_measures_barrel() {
    // barrel() is revolution_measures.rs's barrel with the sketch's x and y swapped: its
    // ends at (±4, 4), and its volume, area, and profile those by hand there (whose closed
    // form and quadrature that file checks), from the general formulas here.
    let (right, left) = barrel().ends();
    assert_close("right x", right[0], 4.0, 1e-15);
    assert_close("right z", right[1], 4.0, 1e-15);
    assert_eq!(left[0], -right[0]);
    let b = barrel_by_hand();
    assert_close("volume", barrel().volume(TAU), b.volume, 1e-15);
    assert_close(
        "area",
        barrel().area(TAU, true),
        2.0 * b.annulus + b.side + b.bore,
        1e-15,
    );
    assert_close("profile", barrel().profile(), b.profile, 1e-15);
}

#[test]
fn a_barrel_turned_one_degree_and_359_999_degrees_measures_as_by_hand() {
    // 1° has no knot inside its U (one span, as before the patch); 359.999° has 2π/3 and
    // 4π/3 inside and stops short of 2π, so FillIntervalBounds breaks on the last knot.
    let mut k = kernel();
    let mut cases = Vec::new();
    for degrees in [1.0, 359.999, 0.001, 179.999_999] {
        let angle = silico_protocol::radians(degrees);
        let shape = turn(&mut k, &barrel().regions(), ABOUT_X, 0.0, angle).unwrap();
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{degrees}°");
        cases.push((
            format!("{degrees}°: volume"),
            s.volume,
            barrel().volume(angle),
        ));
        cases.push((
            format!("{degrees}°: area"),
            s.area,
            barrel().area(angle, false),
        ));
        assert!(
            kinds(&mut k, &shape).contains(&"revolution"),
            "{degrees}°: {:?}",
            kinds(&mut k, &shape)
        );
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn a_barrel_turned_to_each_knot_and_an_ulp_either_side_measures_as_by_hand() {
    // The patch's knots, 2π/3 and 4π/3, at the turn's end exactly (`kn < theB` false: no
    // span added), an ulp and 1e-13 past (a span narrower than EPS_PARAM, 1e-12, which the
    // U loop drops, its inertia reset), and 1e-9 past (a span of 1e-9 integrated).
    let mut k = kernel();
    let mut cases = Vec::new();
    for knot in [TAU / 3.0, 2.0 * TAU / 3.0] {
        for angle in [
            knot.next_down(),
            knot,
            knot.next_up(),
            knot + 1e-13,
            knot + 1e-9,
            knot - 1e-9,
        ] {
            let shape = turn(&mut k, &barrel().regions(), ABOUT_X, 0.0, angle).unwrap();
            let s = k.summarize(&shape).unwrap();
            assert!(s.valid, "{angle}");
            cases.push((format!("{angle}: volume"), s.volume, barrel().volume(angle)));
            cases.push((
                format!("{angle}: area"),
                s.area,
                barrel().area(angle, false),
            ));
        }
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn a_barrel_turned_from_any_start_or_moved_far_measures_the_same() {
    // A start rotates the profile first (sweep_revolution's gp_Trsf), so U still runs from
    // 0; a move puts the solid under a location. Neither may change the measure.
    let mut k = kernel();
    let mut cases = Vec::new();
    for start in [-TAU, -1.0, 1.0, PI, TAU] {
        for angle in [TAU, 1.5 * PI] {
            let shape = turn(&mut k, &barrel().regions(), ABOUT_X, start, angle).unwrap();
            let s = k.summarize(&shape).unwrap();
            cases.push((
                format!("start {start}, {angle}"),
                s.volume,
                barrel().volume(angle),
            ));
            let moved = k.translate(&shape, [1000.0, -1000.0, 500.0]).unwrap().shape;
            let m = k.summarize(&moved).unwrap();
            assert!(m.valid);
            cases.push((
                format!("start {start}, {angle}, moved"),
                m.volume,
                barrel().volume(angle),
            ));
        }
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn a_barrel_measures_the_same_bits_every_time_and_on_a_fresh_worker() {
    let mut k = kernel();
    let shape = turn(&mut k, &barrel().regions(), ABOUT_X, 0.0, TAU).unwrap();
    let a = k.summarize(&shape).unwrap();
    let b = k.summarize(&shape).unwrap();
    let mut fresh = kernel();
    let c = fresh.summarize(&shape).unwrap();
    for (x, y) in [(&a, &b), (&a, &c)] {
        assert_eq!(x.volume.to_bits(), y.volume.to_bits());
        assert_eq!(x.area.to_bits(), y.area.to_bits());
        for i in 0..3 {
            assert_eq!(x.center[i].to_bits(), y.center[i].to_bits());
        }
    }
}

#[test]
fn the_default_integration_still_measures_the_barrel_right() {
    // FreeCAD's integration reads neither UKnots nor SUIntSubs; it had the barrel to 4e-16.
    let mut k = kernel();
    for angle in [TAU, 1.5 * PI] {
        let shape = turn(&mut k, &barrel().regions(), ABOUT_X, 0.0, angle).unwrap();
        let m = k.measure_as_freecad(&shape).unwrap();
        assert_close("default volume", m.volume, barrel().volume(angle), 1e-12);
        assert_close(
            "default area",
            m.area,
            barrel().area(angle, angle == TAU),
            1e-12,
        );
    }
}

// ---- Circles that reach the axis ----

#[test]
fn an_arc_whose_circle_is_tangent_to_the_axis_turned_measures_as_pappus() {
    // The half disc left of x = 3 in the circle about (3, 0) of radius 3, tangent to the
    // axis (model z) at the origin, its arc split there (90° to 180°, 180° to 270°): its
    // circle's centre is exactly its radius from the axis, so neither a sphere nor a torus
    // (MajorRadius > aR is false): a surface of revolution with a point on the axis.
    // Pappus: the half disc's area πr²/2, its centroid 4r/(3π) left of the centre, so
    // V = 2π (r − 4r/(3π)) πr²/2 = r³ (π² − 4π/3); its side the half circle's length πr
    // turned about its centroid's r − 2r/π, 2π²r² − 4πr², the cylinder 4πr², and short of a
    // turn the two half discs.
    let r: f64 = 3.0;
    let regions = region(vec![
        arc([r, 0.0], r, [r, r], [0.0, 0.0], FRAC_PI_2, PI),
        arc([r, 0.0], r, [0.0, 0.0], [r, -r], PI, 1.5 * PI),
        line([r, -r], [r, r]),
    ]);
    let volume = r * r * r * (PI * PI - 4.0 * PI / 3.0);
    let whole_area = 2.0 * PI * PI * r * r;
    let mut k = kernel();
    let mut cases = Vec::new();
    for angle in [TAU, 1.5 * PI, 0.5 * PI, silico_protocol::radians(1.0)] {
        let shape = turn(&mut k, &regions, ABOUT_Z, 0.0, angle).unwrap();
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{angle}");
        let kinds = kinds(&mut k, &shape);
        assert!(kinds.contains(&"revolution"), "{angle}: {kinds:?}");
        let part = angle / TAU;
        let ends = if angle == TAU { 0.0 } else { PI * r * r };
        cases.push((format!("{angle}: volume"), s.volume, volume * part));
        cases.push((format!("{angle}: area"), s.area, whole_area * part + ends));
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn an_arc_tangent_to_the_axis_at_its_middle_is_measured_right_or_refused() {
    // The same half disc with its arc whole (90° to 270°), so it touches the axis at an
    // interior point, not a vertex: OCCT may build a face that pinches there. Right (as
    // above) or refused, or flagged invalid; never Ok, valid, and wrong.
    let r: f64 = 3.0;
    let regions = region(vec![
        arc([r, 0.0], r, [r, r], [r, -r], FRAC_PI_2, 1.5 * PI),
        line([r, -r], [r, r]),
    ]);
    let volume = r * r * r * (PI * PI - 4.0 * PI / 3.0);
    let mut k = kernel();
    match turn(&mut k, &regions, ABOUT_Z, 0.0, TAU) {
        Err(e) => assert_refused(&e),
        Ok(shape) => {
            let s = k.summarize(&shape).unwrap();
            if s.valid {
                assert_close("volume", s.volume, volume, 1e-12);
                assert_close("area", s.area, 2.0 * PI * PI * r * r, 1e-12);
            }
        }
    }
    // And the worker still works.
    let shape = turn(&mut k, &barrel().regions(), ABOUT_X, 0.0, TAU).unwrap();
    assert_close(
        "next",
        k.summarize(&shape).unwrap().volume,
        barrel().volume(TAU),
        1e-12,
    );
}

#[test]
fn a_segment_of_a_circle_tangent_to_the_axis_measures_as_pappus() {
    // The segment of the circle about (3, 0) of radius 3 above its chord z = 1.5 (its arc
    // from 30° to 150°, the circle tangent to the axis but the arc clear of it): area
    // r²/2 (θ − sin θ), θ = 2π/3, its centroid on x = 3; its arc's length 2π and centroid on
    // x = 3; its ring at z = 1.5 from 3 − 3√3/2 to 3 + 3√3/2.
    let r: f64 = 3.0;
    let half = 1.5 * libm::sqrt(3.0);
    let regions = region(vec![
        arc(
            [r, 0.0],
            r,
            [r + half, 1.5],
            [r - half, 1.5],
            PI / 6.0,
            5.0 * PI / 6.0,
        ),
        line([r - half, 1.5], [r + half, 1.5]),
    ]);
    let theta = 2.0 * PI / 3.0;
    let segment = r * r / 2.0 * (theta - libm::sin(theta));
    let volume = TAU * r * segment;
    let ring = PI * ((r + half) * (r + half) - (r - half) * (r - half));
    let side = TAU * r * r * theta;
    let mut k = kernel();
    let mut cases = Vec::new();
    for angle in [TAU, 1.5 * PI] {
        let shape = turn(&mut k, &regions, ABOUT_Z, 0.0, angle).unwrap();
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid);
        let part = angle / TAU;
        let ends = if angle == TAU { 0.0 } else { 2.0 * segment };
        cases.push((format!("{angle}: volume"), s.volume, volume * part));
        cases.push((
            format!("{angle}: area"),
            s.area,
            (ring + side) * part + ends,
        ));
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn a_circle_crossing_the_axis_at_an_arcs_end_measures_as_by_hand() {
    // Under the arc of the circle about (3, 0) of radius 5 from (3, 5) (90°) to (0, 4)
    // (atan2(4, −3)), where it crosses the axis at a slant: a cone-like point. The shells,
    // V = 2π ∫ x √(25 − (x − 3)²) dx over [0, 3] = 2π (−61/3 + 18 + 37.5 asin(3/5)).
    let regions = region(vec![
        line([0.0, 0.0], [3.0, 0.0]),
        line([3.0, 0.0], [3.0, 5.0]),
        arc(
            [3.0, 0.0],
            5.0,
            [3.0, 5.0],
            [0.0, 4.0],
            FRAC_PI_2,
            libm::atan2(4.0, -3.0),
        ),
        line([0.0, 4.0], [0.0, 0.0]),
    ]);
    let volume = TAU * (-61.0 / 3.0 + 18.0 + 37.5 * libm::asin(0.6));
    let q = TAU
        * integral(0.0, 3.0, 64, |x| {
            x * libm::sqrt(25.0 - (x - 3.0) * (x - 3.0))
        });
    assert_close("closed form", volume, q, 1e-14);
    let mut k = kernel();
    let mut cases = Vec::new();
    for angle in [TAU, 1.5 * PI] {
        let shape = turn(&mut k, &regions, ABOUT_Z, 0.0, angle).unwrap();
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid);
        assert!(kinds(&mut k, &shape).contains(&"revolution"));
        cases.push((format!("{angle}"), s.volume, volume * angle / TAU));
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn a_half_disc_nearly_tangent_to_the_axis_is_measured_right_or_refused() {
    // The tangent half disc with its circle's centre 1e-9 farther from the axis (a torus
    // whose inner equator is a circle of radius 1e-9, under OCCT's tolerance) and 1e-9
    // nearer (a surface of revolution whose arc crosses the axis by 1e-9). Right by Pappus
    // with the centre as given, or refused, or flagged invalid; never valid and wrong.
    let r: f64 = 3.0;
    let mut k = kernel();
    for off in [1e-9, -1e-9, 1e-7, -1e-7, 2e-7] {
        let c = r + off;
        let regions = region(vec![
            arc([c, 0.0], r, [c, r], [c - r, 0.0], FRAC_PI_2, PI),
            arc([c, 0.0], r, [c - r, 0.0], [c, -r], PI, 1.5 * PI),
            line([c, -r], [c, r]),
        ]);
        let volume = TAU * (c - 4.0 * r / (3.0 * PI)) * PI * r * r / 2.0;
        // Measured (2026-10-25): 1e-9, 1e-7, and 2e-7 a valid torus, −1e-9 a valid surface
        // of revolution, each exact to 2e-16; −1e-7 (a corner exactly the tolerance across
        // the axis) refused by OCCT, kernel_failed.
        match turn(&mut k, &regions, ABOUT_Z, 0.0, TAU) {
            Err(e) => {
                assert_eq!(off, -1e-7, "{e}");
                assert_refused(&e);
            }
            Ok(shape) => {
                let s = k.summarize(&shape).unwrap();
                if s.valid {
                    assert_close(&format!("{off}"), s.volume, volume, ASKED);
                }
            }
        }
    }
}

#[test]
fn an_arc_nearly_centred_on_the_axis_measures_as_by_hand_whatever_its_kind() {
    // About the model's x, the circle about (0, δ) of radius 5 from 10° to 170°, down to
    // the axis at each end. V = π ∫ y² dx over [−a, a], y = δ + √(25 − x²), a = 5 cos 10°:
    // π [2a (δ² + 25) − 2a³/3 + 2δ (a √(25 − a²) + 25 asin(a/5))]. OCCT takes a circle whose
    // centre is within 1e-7 of the axis as centred on it (GetType's TolConf), and builds a
    // sphere: that solid is the δ = 0 one, 2πδ S / V = 4.4e-9 from the profile's at δ = 1e-8
    // and 4.4e-8 at 1e-7 (a modelling tolerance, not the measure: FreeCAD builds the same).
    // So a sphere is held between the δ = 0 value and its own δ's (its caps' edges keep the
    // profile's ends, δ off the sphere: 3.4e-9 from δ = 0 at δ = 1e-7), and a surface of
    // revolution to its own δ's, each to the asked 1e-9; the kind is OCCT's to choose.
    let mut k = kernel();
    let mut cases = Vec::new();
    let mut spheres = 0;
    let (r, a0) = (5.0, PI / 18.0);
    let volume = |delta: f64| {
        let a = on([0.0, delta], r, a0)[0];
        PI * (2.0 * a * (delta * delta + r * r) - 2.0 * a * a * a / 3.0
            + 2.0 * delta * (a * libm::sqrt(r * r - a * a) + r * r * libm::asin(a / r)))
    };
    for delta in [
        0.0,
        1e-8,
        -1e-8,
        1e-7,
        1.000_000_1e-7,
        1e-6,
        -1e-6,
        1e-3,
        -1e-3,
    ] {
        let right = on([0.0, delta], r, a0);
        let left = on([0.0, delta], r, PI - a0);
        let regions = region(vec![
            line([left[0], 0.0], [right[0], 0.0]),
            line([right[0], 0.0], right),
            arc([0.0, delta], r, right, left, a0, PI - a0),
            line(left, [left[0], 0.0]),
        ]);
        for angle in [TAU, 1.5 * PI] {
            let shape = turn(&mut k, &regions, ABOUT_X, 0.0, angle).unwrap();
            let s = k.summarize(&shape).unwrap();
            assert!(s.valid, "{delta}");
            let ks = kinds(&mut k, &shape);
            let sphere = ks.contains(&"sphere");
            assert!(sphere || ks.contains(&"revolution"), "{delta}: {ks:?}");
            // Where OCCT's rule puts each: a sphere at |δ| ≤ 1e-7, past it not.
            assert_eq!(sphere, delta.abs() <= 1e-7, "{delta}: {ks:?}");
            spheres += usize::from(sphere);
            let part = angle / TAU;
            let want = if sphere {
                let (lo, hi) = (
                    volume(0.0).min(volume(delta)),
                    volume(0.0).max(volume(delta)),
                );
                s.volume.clamp(lo * part, hi * part)
            } else {
                volume(delta) * part
            };
            cases.push((format!("δ {delta}, {angle} ({ks:?})"), s.volume, want));
        }
    }
    assert_eq!(spheres, 8);
    let missed = misses(&cases, ASKED);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

// ---- Lines skew to the axis: hyperboloids ----

#[test]
fn a_triangle_off_the_axis_plane_turned_about_a_skew_axis_measures_by_hand() {
    // The triangle (1, 0), (3, 0), (1, 3) on XZ (y = 0) turned about the line along z
    // through (0, 2, 0), which its plane doesn't hold: its hypotenuse is skew to the axis,
    // so its turn is a hyperboloid OCCT keeps as a surface of revolution of a line. A point
    // (x, 0, z) is √(x² + 4) from the axis, and r dr = x dx, so V = 2π ∫∫ x dA = 2π · 3 · 5/3
    // = 10π, whatever the axis's distance. Faces: the ring at z = 0 from √5 to √13 (8π), the
    // cylinder of radius √5 and height 3 (6√5 π), and the hyperboloid, 2π ∫ ρ √(ρ'² + z'²) dt
    // along x = 3 − 2t, z = 3t, ρ = √(x² + 4).
    let regions = region(vec![
        line([1.0, 0.0], [3.0, 0.0]),
        line([3.0, 0.0], [1.0, 3.0]),
        line([1.0, 3.0], [1.0, 0.0]),
    ]);
    let axis = Axis {
        origin: [0.0, 2.0, 0.0],
        direction: [0.0, 0.0, 1.0],
    };
    let hyperboloid = TAU
        * integral(0.0, 1.0, 64, |t| {
            let x = 3.0 - 2.0 * t;
            let rho = libm::sqrt(x * x + 4.0);
            let drho = -2.0 * x / rho;
            rho * libm::sqrt(drho * drho + 9.0)
        });
    let ring = 8.0 * PI;
    let cylinder = 6.0 * libm::sqrt(5.0) * PI;
    let mut k = kernel();
    let mut cases = Vec::new();
    for angle in [TAU, 1.5 * PI] {
        let shape = turn(&mut k, &regions, axis, 0.0, angle).unwrap();
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{angle}");
        let d = k.describe(&shape).unwrap();
        let ks: Vec<&str> = d.faces.iter().map(|f| kind(&f.surface)).collect();
        assert!(ks.contains(&"revolution"), "{ks:?}");
        let part = angle / TAU;
        cases.push((format!("{angle}: volume"), s.volume, 10.0 * PI * part));
        let ends = if angle == TAU { 0.0 } else { 2.0 * 3.0 };
        cases.push((
            format!("{angle}: area"),
            s.area,
            (ring + cylinder + hyperboloid) * part + ends,
        ));
        let side = d
            .faces
            .iter()
            .find(|f| kind(&f.surface) == "revolution")
            .unwrap();
        cases.push((
            format!("{angle}: hyperboloid"),
            side.area,
            hyperboloid * part,
        ));
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

// ---- Meridians FreeCAD makes: B-splines, ellipses ----

/// Python: `face` (a FreeCAD face) printed as its text B-rep.
const PRINT_FACE: &str = r#"
assert face.isValid()
print("SILICO_ORACLE " + json.dumps(face.exportBrepToString()))
"#;

/// The face FreeCAD makes from `body` (Python that sets `face`), checked valid there.
pub fn freecad_face(body: &str) -> ShapeData {
    let script =
        format!("import json, math, Part, FreeCAD as App\nV = App.Vector\n{body}\n{PRINT_FACE}");
    let text = oracle::freecad(&script);
    from_freecad(text.as_str().unwrap())
}

/// A C⁰ cubic: poles (0, 4), (0.5, 5), (1.5, 5.5), (2, 4.8), (2.5, 5.6), (3.5, 5.2),
/// (4, 4) on XY, knots 0, 0.5, 1 of multiplicities 4, 3, 4: two Bézier spans meeting at a
/// kink at (2, 4.8), each its four poles as they stand.
const C0_SPANS: [[[f64; 2]; 4]; 2] = [
    [[0.0, 4.0], [0.5, 5.0], [1.5, 5.5], [2.0, 4.8]],
    [[2.0, 4.8], [2.5, 5.6], [3.5, 5.2], [4.0, 4.0]],
];

#[test]
fn a_c0_bspline_meridian_with_a_triple_knot_turned_measures_as_pappus() {
    // The cap between the C⁰ cubic and its chord on y = 4, turned about the line along x
    // through (0, −5, 0): Pappus, 2π (ȳ + 5) A, A = ∫ (y − 4) dx and A ȳ = ∫ (y² − 16)/2 dx
    // along the curve (Green; polynomials of degree 5 and 8 per span, exact at 8 points).
    let along = |inner: fn([f64; 2]) -> f64| {
        cap_integral(
            &[(0.0, 1.0); 2],
            |span, t| bezier(&C0_SPANS[span], t),
            inner,
        )
    };
    let area = along(|[_, y]| y - 4.0);
    let moment = along(|[_, y]| (y * y - 16.0) / 2.0);
    let pappus = TAU * (moment + 5.0 * area);
    let cap = freecad_face(
        r#"
poles = [V(0, 4, 0), V(0.5, 5, 0), V(1.5, 5.5, 0), V(2, 4.8, 0), V(2.5, 5.6, 0), V(3.5, 5.2, 0), V(4, 4, 0)]
curve = Part.BSplineCurve()
curve.buildFromPolesMultsKnots(poles, [4, 3, 4], [0.0, 0.5, 1.0], False, 3)
assert curve.Continuity == "C0", curve.Continuity
edge = curve.toShape()
face = Part.Face(Part.Wire([edge, Part.makeLine(V(4, 4, 0), V(0, 4, 0))]))
"#,
    );
    let axis = Axis {
        origin: [0.0, -5.0, 0.0],
        direction: [1.0, 0.0, 0.0],
    };
    let mut k = kernel();
    let mut cases = Vec::new();
    for angle in [TAU, 1.5 * PI, 0.5 * PI, silico_protocol::radians(1.0)] {
        let shape = k.revolution(&cap, axis, 0.0, angle).unwrap().shape;
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{angle}");
        let ks = kinds(&mut k, &shape);
        assert!(ks.contains(&"revolution"), "{ks:?}");
        cases.push((format!("{angle}"), s.volume, pappus * angle / TAU));
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn the_barrels_arc_as_a_rational_bspline_turned_measures_as_the_barrel() {
    // The barrel's arc from (4, 4) to (−4, 4) about (0, −3.5) as the one rational quadratic
    // Bézier it is: its middle pole where the end tangents meet, (0, −3.5 + r / cos α), its
    // weight cos α, α the half-angle (sin α = 4/8.5, cos α = 7.5/8.5). Its turn is a surface
    // of revolution of a B-spline whose volume is the barrel's.
    let face = freecad_face(
        r#"
w = 7.5 / 8.5
poles = [V(4, 0, 4), V(0, 0, -3.5 + 8.5 / w), V(-4, 0, 4)]
curve = Part.BSplineCurve()
curve.buildFromPolesMultsKnots(poles, [3, 3], [0.0, 1.0], False, 2, [1.0, w, 1.0])
mid = curve.value(0.5)
assert abs(mid.z - 5.0) < 1e-14 and abs(mid.x) < 1e-14, mid
edges = [curve.toShape(), Part.makeLine(V(-4, 0, 4), V(-4, 0, 1)), Part.makeLine(V(-4, 0, 1), V(4, 0, 1)), Part.makeLine(V(4, 0, 1), V(4, 0, 4))]
face = Part.Face(Part.Wire(edges))
"#,
    );
    let mut k = kernel();
    let mut cases = Vec::new();
    for angle in [TAU, 1.5 * PI, 0.5 * PI] {
        let shape = k.revolution(&face, ABOUT_X, 0.0, angle).unwrap().shape;
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{angle}");
        assert!(kinds(&mut k, &shape).contains(&"revolution"));
        cases.push((format!("{angle}: volume"), s.volume, barrel().volume(angle)));
        cases.push((
            format!("{angle}: area"),
            s.area,
            barrel().area(angle, angle == TAU),
        ));
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn a_tilted_ellipse_turned_about_an_axis_in_its_plane_measures_as_pappus() {
    // The ellipse about (0, 0, 6) on XZ, semi-axes 4 (tilted 30° from x) and 2: a surface
    // of revolution of an ellipse (no elementary kind). Pappus: V = 2π · 6 · π · 4 · 2 =
    // 96π²; its area the ellipse's perimeter, ∫ √(16 sin² t + 4 cos² t) dt over a turn, times
    // 2π · 6.
    let face = freecad_face(
        r#"
c = V(0, 0, 6)
t = math.radians(30)
major = c + V(4 * math.cos(t), 0, 4 * math.sin(t))
minor = c + V(-2 * math.sin(t), 0, 2 * math.cos(t))
e = Part.Ellipse(major, minor, c)
face = Part.Face(Part.Wire([e.toShape()]))
"#,
    );
    let perimeter = integral(0.0, TAU, 64, |t| {
        libm::sqrt(16.0 * libm::sin(t) * libm::sin(t) + 4.0 * libm::cos(t) * libm::cos(t))
    });
    let mut k = kernel();
    let mut cases = Vec::new();
    for angle in [TAU, 1.5 * PI] {
        let shape = k.revolution(&face, ABOUT_X, 0.0, angle).unwrap().shape;
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{angle}");
        let ks = kinds(&mut k, &shape);
        assert!(ks.contains(&"revolution"), "{ks:?}");
        let part = angle / TAU;
        let ends = if angle == TAU { 0.0 } else { 2.0 * 8.0 * PI };
        cases.push((format!("{angle}: volume"), s.volume, 96.0 * PI * PI * part));
        cases.push((
            format!("{angle}: area"),
            s.area,
            TAU * 6.0 * perimeter * part + ends,
        ));
    }
    // An area's integrand is a square root, which no fixed rule integrates exactly: OCCT
    // has these 4e-12 over, so they're held to the asked 1e-9, the volumes to 1e-12.
    let (volumes, areas): (Vec<_>, Vec<_>) = cases
        .into_iter()
        .partition(|(what, _, _)| what.ends_with("volume"));
    let mut missed = misses(&volumes, 1e-12);
    missed.extend(misses(&areas, ASKED));
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

// ---- Surfaces of revolution as files can hold them ----

/// Python: the barrel about x, turned a whole turn by FreeCAD (`sol`), and its side (`side`).
const FREECAD_BARREL: &str = r#"
arc = Part.Arc(V(4, 0, 4), V(0, 0, 5), V(-4, 0, 4)).toShape()
edges = [arc, Part.makeLine(V(-4, 0, 4), V(-4, 0, 1)), Part.makeLine(V(-4, 0, 1), V(4, 0, 1)), Part.makeLine(V(4, 0, 1), V(4, 0, 4))]
sol = Part.Face(Part.Wire(edges)).revolve(V(0, 0, 0), V(1, 0, 0), 360)
side = [f for f in sol.Faces if f.Surface.TypeId == "Part::GeomSurfaceOfRevolution"][0]
"#;

#[test]
fn a_surface_of_revolution_under_a_rectangular_trimmed_surface_measures_as_by_hand() {
    // The barrel's side written as a Geom_RectangularTrimmedSurface (type 10) over its
    // surface of revolution (type 7), trimmed to the face's own bounds: GeomAdaptor looks
    // through the trim, so GetType is still SurfaceOfRevolution and the patch applies.
    let script = format!(
        r#"import json, math, Part, FreeCAD as App
V = App.Vector
{FREECAD_BARREL}
u1, u2, v1, v2 = side.ParameterRange
lines = sol.exportBrepToString().split("\n")
first = lines.index([l for l in lines if l.startswith("Surfaces ")][0])
at = [k for k in range(first + 1, len(lines)) if lines[k].startswith("7 ") and len(lines[k].split()) == 7]
assert len(at) == 1, at
lines.insert(at[0], "10 %r %r %r %r" % (u1, u2, v1, v2))
text = "\n".join(lines)
check = Part.Shape()
check.importBrepFromString(text)
again = check.exportBrepToString()
assert "\n10 " in again[again.index("Surfaces"):], "the trim was dropped"
assert check.isValid()
print("SILICO_ORACLE " + json.dumps(text))
"#
    );
    let shape = from_freecad(oracle::freecad(&script).as_str().unwrap());
    assert_eq!(shape.valid(), Some(true));
    let mut k = kernel();
    let s = k.summarize(&shape).unwrap();
    assert!(s.valid);
    let ks = kinds(&mut k, &shape);
    assert!(ks.contains(&"revolution"), "{ks:?}");
    assert_close("volume", s.volume, barrel().volume(TAU), 1e-12);
    assert_close("area", s.area, barrel().area(TAU, true), 1e-12);
}

/// Python: the barrel `b` about x, turned a whole turn by FreeCAD, its side's face rebuilt
/// over U from `u0` to `u0` + 2π (`toShape`), and the faces sewn back into a solid, printed.
fn shifted_barrel_script(b: Barrel, u0: f64) -> String {
    let Barrel { c, r, a, inner } = b;
    format!(
        r#"import json, math, Part, FreeCAD as App
V = App.Vector
c, r, a, inner = {c:?}, {r:?}, {a:?}, {inner:?}
right = V(r * math.cos(a), 0, c + r * math.sin(a))
left = V(-right.x, 0, right.z)
arc = Part.Arc(right, V(0, 0, c + r), left).toShape()
edges = [arc, Part.makeLine(left, V(left.x, 0, inner)), Part.makeLine(V(left.x, 0, inner), V(right.x, 0, inner)), Part.makeLine(V(right.x, 0, inner), right)]
sol = Part.Face(Part.Wire(edges)).revolve(V(0, 0, 0), V(1, 0, 0), 360)
plain = ("Part::GeomPlane", "Part::GeomCylinder")
side = [f for f in sol.Faces if f.Surface.TypeId not in plain]
assert len(side) == 1, [f.Surface.TypeId for f in sol.Faces]
side = side[0]
others = [f for f in sol.Faces if f.Surface.TypeId in plain]
u1, u2, v1, v2 = side.ParameterRange
moved = side.Surface.toShape(u1 + {u0:?}, u2 + {u0:?}, v1, v2)
shell = Part.Shell([moved] + others)
shell.sewShape()
solid = Part.Solid(shell)
assert solid.isValid()
spans = [(f.Surface.TypeId, f.ParameterRange) for f in solid.Faces if f.Surface.TypeId not in plain]
assert len(spans) == 1 and abs(spans[0][1][0] - {u0:?}) < 1e-12, spans
print("SILICO_ORACLE " + json.dumps(solid.exportBrepToString()))
"#
    )
}

#[test]
fn a_surface_of_revolution_whose_u_starts_off_zero_measures_as_by_hand() {
    // The barrel with its side rebuilt over U from u0 to u0 + 2π: a file can hold it so
    // (STEP, a B-rep from elsewhere). The patch's knots are fixed at 0, 2π/3, 4π/3, 2π
    // (FillIntervalBounds keeps those inside the face's U), not the face's own thirds, so a
    // face from 1 to 1 + 2π gets four spans, from 3 to 3 + 2π three, the last of π, and from
    // 6 to 6 + 2π one of 0.28 and one of 6.0: nearly the one span the patch set out to
    // split. OCCT's torus shares the knots, so a torus barrel (its circle about (0, 8) of
    // radius 3, from 60° to 120°) is checked beside it.
    let torus = Barrel {
        c: 8.0,
        r: 3.0,
        a: PI / 3.0,
        inner: 1.0,
    };
    let mut k = kernel();
    let mut cases = Vec::new();
    for u0 in [1.0, 3.0, 5.0, 6.0, 6.2] {
        for (what, b) in [("barrel", barrel()), ("torus", torus)] {
            let script = shifted_barrel_script(b, u0);
            let shape = from_freecad(oracle::freecad(&script).as_str().unwrap());
            let s = k.summarize(&shape).unwrap();
            assert!(s.valid, "{what} {u0}");
            let ks = kinds(&mut k, &shape);
            assert!(
                ks.contains(&if what == "torus" {
                    "torus"
                } else {
                    "revolution"
                }),
                "{ks:?}"
            );
            cases.push((format!("{what} from u = {u0}"), s.volume, b.volume(TAU)));
        }
    }
    // Found by /break-it, a known wrong result (docs/ports/measure.md): the knots don't
    // follow the face's U, so a face whose U starts past 4π/3 is integrated over nearly one
    // span, as before the patch, and as stock OCCT does a torus's. Silico's own revolves
    // start U at 0; only a file reaches this. Right from 1 and 3 (to 1e-11; 1.4e-12
    // measured), OCCT's known wrong answer from 5, 6, and 6.2 (or right, once OCCT places
    // them).
    let known = [
        ("barrel from u = 5", 526.334_308_090_930_6),
        ("barrel from u = 6", 526.334_239_754_574_1),
        ("barrel from u = 6.2", 526.334_200_496_085_5),
        ("torus from u = 5", 1_104.276_254_148_789_6),
        ("torus from u = 6", 1_104.276_109_099_114_4),
        ("torus from u = 6.2", 1_104.276_025_769_664),
    ];
    let missed = right_or_known_wrong(&cases, 1e-11, &known);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

// ---- The barrel fused to a box, and filleted where they meet ----

/// The barrel about x and the box from (−1, −1, 2) to (1, 1, 7) standing out of its top.
fn barrel_and_box(k: &mut KernelWorker) -> ShapeData {
    let barrel = turn(k, &barrel().regions(), ABOUT_X, 0.0, TAU).unwrap();
    let block = k
        .make(Primitive::Box {
            dx: 2.0,
            dy: 2.0,
            dz: 5.0,
        })
        .unwrap();
    let block = k.translate(&block, [-1.0, -1.0, 2.0]).unwrap().shape;
    k.boolean(BooleanOp::Fuse, &barrel, &block).unwrap().shape
}

/// The barrel's outer radius at x.
fn outer(x: f64) -> f64 {
    -3.5 + libm::sqrt(8.5 * 8.5 - x * x)
}

#[test]
fn the_barrel_fused_to_a_box_measures_as_by_hand() {
    // The barrel plus the box's part outside it: ∫∫ (7 − √(ρ(x)² − y²)) dy dx over
    // |x|, |y| ≤ 1 (the box's floor, z = 2, is inside the wall: √(y² + 4) is between 2 and
    // √5, the bore 1 and the outside 4.9 or more there). Smooth: 16 pieces and 32 agree.
    let outside = |pieces| {
        integral(-1.0, 1.0, pieces, |x| {
            integral(-1.0, 1.0, pieces, |y| {
                7.0 - libm::sqrt(outer(x) * outer(x) - y * y)
            })
        })
    };
    assert_close("converged", outside(16), outside(32), 1e-15);
    let by_hand = barrel().volume(TAU) + outside(32);
    let mut k = kernel();
    let fused = barrel_and_box(&mut k);
    let s = k.summarize(&fused).unwrap();
    assert!(s.valid);
    // Its side now holes where the box stands, across its seam (the profile's plane, y = 0,
    // z > 0): a surface of revolution trimmed by curves that are no iso-lines. The boolean
    // writes those curves as B-splines within 5e-6 (their edges' tolerance; FreeCAD's fuse
    // the same, 534.5476025859015 by its default integration), so the solid itself is that
    // far from the one by hand: along the seam's 8 mm, faces up to 5 from the origin, at
    // most 5e-6 × 8 × (1 + 5) / 3 = 8e-5 mm³, 1.5e-7 of it (2.1e-8 measured). The measure
    // is then held to OCCT's other integration on the same B-rep, to the asked 1e-9.
    assert_close("fused, by hand", s.volume, by_hand, 1.5e-7);
    let default = k.measure_as_freecad(&fused).unwrap();
    assert_close(
        "fused, both integrations",
        s.volume,
        default.volume,
        2.0 * ASKED,
    );
}

#[test]
fn the_barrel_fused_to_a_box_and_filleted_where_they_meet_is_valid_and_pinned() {
    // The edges where the box's sides meet the barrel's side, filleted at 0.3: each
    // fillet a surface trimmed by B-spline pcurves on a surface of revolution. No closed
    // form; bounded by hand (the fillet only adds, and adds less than the r × r square's
    // corner, r² (1 − π/4), along the seam's length, where the faces meet at no less than a
    // right angle), and pinned (macOS ARM, OCCT 7.9.3 with patches 0001-0003).
    let mut k = kernel();
    let fused = barrel_and_box(&mut k);
    let before = k.summarize(&fused).unwrap();
    let d = k.describe(&fused).unwrap();
    let seam: Vec<u32> = d
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let [x, y, z] = e.middle;
            x.abs() <= 1.0 + 1e-9
                && y.abs() <= 1.0 + 1e-9
                && z > 0.0
                && (libm::sqrt(y * y + z * z) - outer(x)).abs() < 1e-6
        })
        .map(|(i, _)| i as u32 + 1)
        .collect();
    // The four sides' curves, those of x = ±1 cut in two by the side's seam.
    assert_eq!(seam.len(), 6, "{seam:?}");
    let length: f64 = seam.iter().map(|&i| d.edges[i as usize - 1].length).sum();
    let r = 0.3;
    let filleted = match k.fillet(&fused, &seam, r) {
        Ok(b) => b.shape,
        Err(e) => panic!("the seam's fillet: {e}"),
    };
    let s = k.summarize(&filleted).unwrap();
    assert!(s.valid);
    let added = s.volume - before.volume;
    assert!(
        added > 0.0 && added < r * r * (1.0 - PI / 4.0) * length * 1.05,
        "added {added}, the seam {length}"
    );
    // FreeCAD's integration on the same B-rep agrees to 1.1e-10 (534.6524054584669).
    let default = k.measure_as_freecad(&filleted).unwrap();
    assert_close("against the default", default.volume, s.volume, 2.0 * ASKED);
    assert_close("pinned", s.volume, PINNED_FILLETED_VOLUME, 1e-12);
}

/// The filleted fusion's volume as measured (macOS ARM, 2026-10-25).
const PINNED_FILLETED_VOLUME: f64 = 534.652_405_400_704_3;

// ---- Barrels of every proportion ----

fn barrel_case() -> impl Strategy<Value = (Barrel, f64, f64)> {
    // The radius from 0.5 to 50; the arc's end angle a from 0.05 to 1.5 rad; the centre
    // anywhere from just above the axis's reach to the radius (a torus past that), biased
    // to the edges (near −r sin a + inner, near 0 but off it, near r); the bore from 0 (a
    // line on the axis) up; the turn whole, at a knot, or anywhere, from any start.
    let unit = prop_oneof![
        3 => 0.0..1.0f64,
        1 => Just(0.0),
        1 => Just(1.0),
        1 => 0.0..1e-6f64,
        1 => (1.0 - 1e-6)..1.0f64,
    ];
    let angle = prop_oneof![
        3 => Just(TAU),
        1 => Just(TAU / 3.0),
        1 => Just(2.0 * TAU / 3.0),
        1 => (TAU / 3.0 - 1e-6)..(TAU / 3.0 + 1e-6),
        1 => (2.0 * TAU / 3.0 - 1e-6)..(2.0 * TAU / 3.0 + 1e-6),
        4 => 1e-3..(TAU - 1e-4),
    ];
    (
        0.5..50.0f64,
        0.05..1.5f64,
        unit,
        prop_oneof![1 => Just(0.0), 3 => 0.01..0.9f64],
        angle,
        -TAU..TAU,
    )
        .prop_map(|(r, a, t, inner_part, angle, start)| {
            let top = r * libm::sin(a);
            // The centre's lowest: its arc's ends 2% of r above the bore; its highest, r.
            let inner = inner_part * 0.5 * r;
            let low = inner + 0.02 * r - top;
            let c = low + t * (r - low);
            (Barrel { c, r, a, inner }, angle, start)
        })
}

#[test]
fn barrels_of_every_proportion_measure_as_by_hand() {
    // 1,000 barrels about x: every one a surface of revolution where |c| is past 1e-7 and
    // under r, a sphere or torus otherwise; its volume by the closed form to 1e-11 (the
    // worst of a run 6.0e-13). Cases too near a sphere (|c| ≤ 1e-6, where OCCT may take the
    // centre on the axis) are held to the asked 1e-9 plus what that moves,
    // 2|c| r² · 2 / volume.
    let k = RefCell::new(kernel());
    let worst = RefCell::new((0.0f64, String::new()));
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    let result = runner.run(&barrel_case(), |(b, angle, start)| {
        let mut k = k.borrow_mut();
        let shape = match turn(&mut k, &b.regions(), ABOUT_X, start, angle) {
            Ok(s) => s,
            Err(e) => return Err(TestCaseError::fail(format!("{b:?} {angle}: refused: {e}"))),
        };
        let s = k.summarize(&shape).unwrap();
        prop_assert!(s.valid, "{b:?} {angle} invalid");
        let want = b.volume(angle);
        let err = relative_error(s.volume, want);
        let tolerance = if b.c.abs() <= 1e-6 {
            ASKED + 4.0 * b.c.abs() * b.r * b.r * angle / want.abs()
        } else {
            1e-11
        };
        {
            let mut w = worst.borrow_mut();
            if err > w.0 {
                *w = (err, format!("{b:?} angle {angle} start {start}"));
            }
        }
        prop_assert!(
            err <= tolerance,
            "{b:?} angle {angle} start {start}: got {}, want {want} ({err:e})",
            s.volume
        );
        Ok(())
    });
    // The worst of a run (2026-10-25): 6.0e-13.
    if let Err(e) = result {
        panic!("{e} (the worst passing: {:?})", worst.borrow());
    }
}

// ---- B-spline meridians of many spans ----

/// (1 − alpha) a + alpha b.
fn lerp(a: [f64; 2], b: [f64; 2], alpha: f64) -> [f64; 2] {
    [
        (1.0 - alpha) * a[0] + alpha * b[0],
        (1.0 - alpha) * a[1] + alpha * b[1],
    ]
}

/// A wavy cubic of `n` poles from x = 0 to 4: y = 4 at the ends and 4.5 + 0.3 sin i
/// between; its knots uniform on [0, 1], clamped (multiplicity 4 at each end): n − 3 spans.
struct Wavy {
    poles: Vec<[f64; 2]>,
}

impl Wavy {
    fn new(n: usize) -> Self {
        let poles = (0..n)
            .map(|i| {
                let x = 4.0 * i as f64 / (n - 1) as f64;
                let y = if i == 0 || i == n - 1 {
                    4.0
                } else {
                    4.5 + 0.3 * libm::sin(i as f64)
                };
                [x, y]
            })
            .collect();
        Wavy { poles }
    }

    fn spans(&self) -> usize {
        self.poles.len() - 3
    }

    /// The knot vector's entry j: 0 up to 3, 1 from n on, else (j − 3) / spans.
    fn knot(&self, j: usize) -> f64 {
        (j.saturating_sub(3) as f64 / self.spans() as f64).min(1.0)
    }

    /// Its point and derivative at `t` in span `span` (0-based: [knot(span + 3),
    /// knot(span + 4)], its poles span to span + 3), by de Boor's algorithm; the derivative
    /// by de Boor's on the degree-2 curve of 3 (P_i − P_{i−1}) / (t_{i+3} − t_i), whose knots
    /// are the cubic's less one at each end.
    fn at(&self, span: usize, t: f64) -> ([f64; 2], [f64; 2]) {
        let p = &self.poles;
        let mut d = [p[span], p[span + 1], p[span + 2], p[span + 3]];
        for r in 1..=3 {
            for j in (r..=3).rev() {
                let i = span + j;
                let alpha = (t - self.knot(i)) / (self.knot(i + 4 - r) - self.knot(i));
                d[j] = lerp(d[j - 1], d[j], alpha);
            }
        }
        let mut q = [[0.0; 2]; 3];
        for (j, qj) in q.iter_mut().enumerate() {
            let i = span + 1 + j;
            let scale = 3.0 / (self.knot(i + 3) - self.knot(i));
            *qj = [
                scale * (p[i][0] - p[i - 1][0]),
                scale * (p[i][1] - p[i - 1][1]),
            ];
        }
        for r in 1..=2 {
            for j in (r..=2).rev() {
                let i = span + j;
                let (lo, hi) = (self.knot(i + 1), self.knot(i + 1 + 3 - r));
                let alpha = (t - lo) / (hi - lo);
                q[j] = lerp(q[j - 1], q[j], alpha);
            }
        }
        (d[3], q[2])
    }

    /// Its cap above the chord y = 4, turned a whole turn about the line along x through
    /// (0, −5, 0), by Pappus: 2π (Aȳ + 5A), A = ∫ (y − 4) dx and Aȳ = ∫ (y² − 16)/2 dx along
    /// the curve (Green; polynomials of degree 5 and 8 on each span, exact at 8 points).
    fn pappus(&self) -> f64 {
        let spans = self.spans() as f64;
        let ranges: Vec<(f64, f64)> = (0..self.spans())
            .map(|span| (span as f64 / spans, (span + 1) as f64 / spans))
            .collect();
        let along =
            |inner: fn([f64; 2]) -> f64| cap_integral(&ranges, |span, t| self.at(span, t), inner);
        let area = along(|[_, y]| y - 4.0);
        let moment = along(|[_, y]| (y * y - 16.0) / 2.0);
        TAU * (moment + 5.0 * area)
    }

    /// The cap's face, made by FreeCAD from the same poles and knots.
    fn cap(&self) -> ShapeData {
        let n = self.poles.len();
        freecad_face(&format!(
            r#"
n = {n}
poles = [V(4.0 * i / (n - 1), 4.0 if i in (0, n - 1) else 4.5 + 0.3 * math.sin(i), 0) for i in range(n)]
knots = [k / (n - 3) for k in range(n - 2)]
mults = [4] + [1] * (n - 4) + [4]
curve = Part.BSplineCurve()
curve.buildFromPolesMultsKnots(poles, mults, knots, False, 3)
face = Part.Face(Part.Wire([curve.toShape(), Part.makeLine(V(4, 4, 0), V(0, 4, 0))]))
"#
        ))
    }
}

const BELOW_CHORD: Axis = Axis {
    origin: [0.0, -5.0, 0.0],
    direction: [1.0, 0.0, 0.0],
};

#[test]
fn the_wavy_cubics_de_boor_is_right_at_its_ends_and_knots() {
    // Clamped: its ends are its end poles, its tangent there 3 (P1 − P0) spans; C² at an
    // interior knot, both spans agreeing; the tangent the point's own difference quotient.
    let w = Wavy::new(403);
    let spans = w.spans() as f64;
    let (p0, d0) = w.at(0, 0.0);
    assert_eq!(p0, [0.0, 4.0]);
    for (c, d) in d0.iter().enumerate() {
        let want = 3.0 * spans * (w.poles[1][c] - w.poles[0][c]);
        assert_close("start tangent", *d, want, 1e-12);
    }
    let (p1, _) = w.at(w.spans() - 1, 1.0);
    assert_close("end x", p1[0], 4.0, 1e-15);
    assert_close("end y", p1[1], 4.0, 1e-15);
    for span in [10, 200, 390] {
        let t = (span + 1) as f64 / spans;
        let (a, da) = w.at(span, t);
        let (b, db) = w.at(span + 1, t);
        for c in 0..2 {
            assert_close("point at a knot", a[c], b[c], 1e-13);
            assert_close("tangent at a knot", da[c], db[c], 1e-10);
        }
        let mid = (span as f64 + 0.5) / spans;
        let h = 1e-7 / spans;
        let (lo, _) = w.at(span, mid - h);
        let (hi, _) = w.at(span, mid + h);
        let (_, d) = w.at(span, mid);
        for c in 0..2 {
            assert_close(
                "difference quotient",
                (hi[c] - lo[c]) / (2.0 * h),
                d[c],
                1e-6,
            );
        }
    }
    // FreeCAD's own evaluation of the same curve (OCCT's), at t = 0.3: (1.2039800995024876,
    // 4.753729261850152), and Pappus by FreeCAD's evaluation, 116.29361086380439 (2026-10-25).
    let (p, _) = w.at((0.3 * spans) as usize, 0.3);
    assert_close("x at 0.3", p[0], 1.203_980_099_502_487_6, 1e-14);
    assert_close("y at 0.3", p[1], 4.753_729_261_850_152, 1e-14);
    assert_close("Pappus", w.pappus(), 116.293_610_863_804_39, 1e-14);
}

#[test]
fn a_bspline_meridian_of_many_spans_turned_measures_as_pappus() {
    // Found by /break-it, a known wrong result (docs/ports/measure.md): OCCT's precise
    // integration of a surface of revolution follows its meridian
    // (V) along the seam's pcurve, a straight line in (u, v) with no knots, so the L loop
    // starts from one span and stops subdividing at 33 (BRepGProp_Gauss's MaxSubs) or when
    // its two Gauss orders agree: a meridian of many spans is under-resolved. 403 poles
    // (400 spans) are 1.3e-3 high at every angle (1°, 90°, 180°, 360°), so it's not U's
    // and patch 0003 neither causes nor mends it; the cap's own plane face is right
    // (3.4e-14). FreeCAD's default integration has the whole turn 33% low (78.03). Two and
    // seven spans are right to the asked 1e-9; more, OCCT's known wrong answer (or right,
    // once OCCT resolves them); and the turn must measure in 10 s. Sketches make no
    // B-splines: only an imported part reaches this.
    let mut k = kernel();
    let mut cases = Vec::new();
    for n in [5, 10, 20, 40, 80, 160, 403] {
        let w = Wavy::new(n);
        let pappus = w.pappus();
        let cap = w.cap();
        for angle in [TAU, silico_protocol::radians(1.0)] {
            let shape = k.revolution(&cap, BELOW_CHORD, 0.0, angle).unwrap().shape;
            let started = std::time::Instant::now();
            let s = k.summarize(&shape).unwrap();
            let took = started.elapsed();
            assert!(s.valid, "{n}");
            assert!(took.as_secs_f64() < 10.0, "{n}: {took:?}");
            cases.push((
                format!("{} spans, {angle}", n - 3),
                s.volume,
                pappus * angle / TAU,
            ));
        }
    }
    let known = [
        ("17 spans, 6.283185307179586", 108.656_578_282_884_54),
        ("17 spans, 0.017453292519943295", 0.301_823_871_400_129_73),
        ("37 spans, 6.283185307179586", 112.879_676_310_221_61),
        ("37 spans, 0.017453292519943295", 0.313_554_425_535_817_8),
        ("77 spans, 6.283185307179586", 116.350_031_429_535_99),
        ("77 spans, 0.017453292519943295", 0.323_191_849_798_806_7),
        ("157 spans, 6.283185307179586", 115.910_220_116_324_35),
        ("157 spans, 0.017453292519943295", 0.321_971_162_646_625_84),
        ("400 spans, 6.283185307179586", 116.448_476_416_511_77),
        ("400 spans, 0.017453292519943295", 0.323_455_730_492_252_85),
    ];
    let missed = right_or_known_wrong(&cases, ASKED, &known);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}
