//! The bracket of `tests/bracket/spec.md` worked out on paper: its mass properties summed
//! from prisms, cylinders, and the fillet's spandrel; the measure's bounds on them; and its
//! 3MF mesh by hand from OCCT's rule, its chords and what it encloses. Shared by the MCP
//! session's done test (`bracket.rs`) and the app's scripted slice (silico-app's
//! `tests/slice.rs`), so both check against the same hand.
#![allow(dead_code)]

use serde_json::Value;

use super::threemf::{Object, printable};

/// π, as `std::f64::consts::PI`.
pub const PI: f64 = std::f64::consts::PI;

/// The spec's hole radius (an M5 clearance hole, 5.5 mm across) and fillet radius.
pub const HOLE_R: f64 = 2.75;
pub const FILLET_R: f64 = 5.0;

// --- Hand calculations ---------------------------------------------------------------------

/// A body's mass properties, summed from parts: a box, a cylinder, or the fillet's
/// spandrel, each of its volume (negative for a hole) and its centroid.
#[derive(Default)]
pub struct Sum {
    pub volume: f64,
    pub moment: [f64; 3],
}

impl Sum {
    pub fn add(&mut self, volume: f64, centroid: [f64; 3]) {
        self.volume += volume;
        for (m, c) in self.moment.iter_mut().zip(centroid) {
            *m += volume * c;
        }
    }

    /// An axis-aligned box from `min` to `max`.
    pub fn cuboid(&mut self, min: [f64; 3], max: [f64; 3]) {
        let volume = (max[0] - min[0]) * (max[1] - min[1]) * (max[2] - min[2]);
        self.add(volume, [0, 1, 2].map(|i| (min[i] + max[i]) / 2.0));
    }

    pub fn centre(&self) -> [f64; 3] {
        self.moment.map(|m| m / self.volume)
    }
}

/// The legs, `width` along y: the base [0, 60] × [0, w] × [0, 5] and the upright
/// [0, 5] × [0, w] × [5, 50] on it.
pub fn legs(width: f64) -> Sum {
    let mut sum = Sum::default();
    sum.cuboid([0.0, 0.0, 0.0], [60.0, width, 5.0]);
    sum.cuboid([0.0, 0.0, 5.0], [5.0, width, 50.0]);
    sum
}

/// One hole: a cylinder of radius 2.75 through a 5 mm leg, π · 2.75² · 5 = 37.8125π.
pub const HOLE: f64 = PI * HOLE_R * HOLE_R * 5.0;

/// The legs less the four holes: the base's at x = 40, y = 12 and 28, z from 0 to 5; the
/// upright's at z = 35, y = 12 and 28, x from 0 to 5.
pub fn drilled() -> Sum {
    let mut sum = legs(40.0);
    for y in [12.0, 28.0] {
        sum.add(-HOLE, [40.0, y, 2.5]);
        sum.add(-HOLE, [2.5, y, 35.0]);
    }
    sum
}

/// The bracket: the drilled legs and the fillet's spandrel in the inside corner, the
/// square [5, 10] × [5, 10] (x, z) less the quarter disc of radius 5 about (10, 10), 40
/// along y. Its section is r²(1 − π/4); its centroid is d from the corner on each axis,
/// d = (r²·r/2 − (πr²/4)(r − 4r/(3π))) / (r²(1 − π/4)) = r(10 − 3π)/(12 − 3π) (the square's
/// moment less the quarter disc's, whose centroid is 4r/(3π) from its centre).
pub fn bracket() -> Sum {
    let mut sum = drilled();
    let r = FILLET_R;
    let d = r * (10.0 - 3.0 * PI) / (12.0 - 3.0 * PI);
    sum.add(r * r * (1.0 - PI / 4.0) * 40.0, [5.0 + d, 20.0, 5.0 + d]);
    sum
}

/// One of the bracket's solids by hand: its volume and centre (`sum`), its area, and its
/// faces, edges and vertices.
pub struct Solid {
    pub sum: Sum,
    pub area: f64,
    pub counts: [u64; 3],
}

/// The bracket's four solids by hand, in the order both its builds make them: the legs; the
/// base's holes pocketed; the upright's too; and the inside corner filleted.
/// - The legs, 40 wide: 60 · 40 · 5 + 5 · 40 · 45 = 21,000 mm³; 2 · 525 + 220 · 40 = 9,850
///   mm²; the L-prism's 8 faces, 18 edges, and 12 vertices.
/// - Each hole a cylinder of 37.8125π mm³ less ([`HOLE`]); it takes two discs of π · 2.75²
///   from its leg's faces and adds a wall of 2π · 2.75 · 5, so 27.5π − 15.125π = 12.375π mm²
///   more; and a face (its wall), three edges (two rims and a seam), and two vertices (on
///   the seam). So the base's two holes 24.75π more area, the four 49.5π.
/// - The fillet: the spandrel, (1 − π/4) · 5² · 40, more ([`bracket`]). It takes 5 · 40 from
///   each leg's inner face and adds a quarter cylinder, (π/2) · 5 · 40 = 100π, and each end
///   face gains the spandrel, 25(1 − π/4): 9,850 + 49.5π − 400 + 100π + 50 − 12.5π = 9,500 +
///   137π mm². It adds a face for the corner's edge, its two lines and two arcs for the one
///   edge, and four vertices for its two: 13 faces, 33 edges, 22 vertices.
pub fn solids() -> [Solid; 4] {
    let mut base = legs(40.0);
    for y in [12.0, 28.0] {
        base.add(-HOLE, [40.0, y, 2.5]);
    }
    [
        Solid {
            sum: legs(40.0),
            area: 9850.0,
            counts: [8, 18, 12],
        },
        Solid {
            sum: base,
            area: 9850.0 + 24.75 * PI,
            counts: [10, 24, 16],
        },
        Solid {
            sum: drilled(),
            area: 9850.0 + 49.5 * PI,
            counts: [12, 30, 20],
        },
        Solid {
            sum: bracket(),
            area: 9500.0 + 137.0 * PI,
            counts: [13, 33, 22],
        },
    ]
}

/// The measure's bound on a volume or area by hand: OCCT's integration to a relative 1e-9
/// (docs/ports/measure.md, `MEASURE_PRECISION`), then one rounding to 6 decimals.
pub fn measured(what: &str, got: &Value, hand: f64) {
    let got = got.as_f64().unwrap_or_else(|| panic!("{what}: {got}"));
    let bound = 1e-9 * hand.abs() + 5e-7;
    assert!(
        (got - hand).abs() <= bound,
        "{what}: measured {got}, by hand {hand} (bound {bound})"
    );
}

/// A centre of mass against the hand's, to the measure's bound (the moments and the volume
/// each to a relative 1e-9, the centre their quotient) and the nanometre it's rounded to.
pub fn centred(what: &str, got: &Value, hand: [f64; 3]) {
    for (i, h) in hand.into_iter().enumerate() {
        let g = got[i].as_f64().unwrap();
        let bound = 2e-9 * h.abs() + 5e-7;
        assert!(
            (g - h).abs() <= bound,
            "{what}[{i}]: measured {g}, by hand {h} (bound {bound})"
        );
    }
}

// --- The export's mesh, by hand -------------------------------------------------------------

/// How many chords OCCT cuts an arc of `angle` radians and radius `r` into, meshing at the
/// tolerance `t` with FreeCAD's angular deflection `a` = min(0.1, 5t + 0.005) (FreeCAD's
/// `defaultAngularDeflection`, docs/ports/mesh-export.md, *Same as FreeCAD*): an edge is
/// meshed at half the face's deflections (`BRepMesh_CurveTessellator::init`, 0.5 × the
/// angular and the linear), and a circle in equal steps of at most min(2 acos(1 − (t/2)/r),
/// a/2) (`GCPnts_TangentialDeflection::PerformCircular` and `ArcAngularStep`), so in
/// ⌈angle / step⌉. Here the angle is the smaller, which the assert holds: so it's the
/// angle's rule alone, whatever the worker takes off the tolerance for rounding.
pub fn chords(angle: f64, r: f64, t: f64) -> usize {
    let a = (5.0 * t + 0.005).min(0.1);
    let linear = 2.0 * libm::acos(1.0 - t / 2.0 / r);
    assert!(linear > a / 2.0, "the linear rule binds at r {r}, t {t}");
    (angle / (a / 2.0)).ceil() as usize
}

/// How many of `vertices` lie on the circle of radius `r` about `centre` in the plane
/// `axis` = `at` (0, 1, 2 for x, y, z), and within `within` of it (a box about the arc).
pub fn on_circle(
    vertices: &[[f64; 3]],
    axis: usize,
    at: f64,
    centre: [f64; 3],
    r: f64,
    within: impl Fn(&[f64; 3]) -> bool,
) -> usize {
    vertices
        .iter()
        .filter(|v| v[axis] == at && within(v))
        .filter(|v| {
            let d: f64 = (0..3)
                .filter(|&i| i != axis)
                .map(|i| (v[i] - centre[i]) * (v[i] - centre[i]))
                .sum();
            (d.sqrt() - r).abs() < 1e-5
        })
        .count()
}

/// What the bracket's mesh encloses, by hand, its rims of `rim` chords and its fillet's
/// arcs of `arc` ([`chords`]), the part's volume `volume`. Every vertex is on the B-rep, on
/// the rims and arcs, so each hole's wall is the prism on its rim's chords and the fillet's
/// the prism on its arc's: each curved face concave to the material, the mesh holds the
/// part's material and, past each arc, the segments between it and its chords. A hole's
/// prism is short of its cylinder by h(πr² − (n/2)r² sin(2π/n)); a chord across θ cuts off
/// (r²/2)(θ − sin θ), under r²θ³/12. So the mesh encloses
/// V + 4 · 5 · (π · 2.75² − (n/2) · 2.75² sin(2π/n)) + 40 · m · (25/2)(θ − sin θ) at
/// θ = (π/2)/m, n chords a rim and m an arc (at 0.1 mm, 126 and 32: about
/// V + 0.197 + 0.315 = V + 0.512 mm³).
pub fn enclosed(volume: f64, rim: usize, arc: usize) -> f64 {
    let n = rim as f64;
    let hole_short = HOLE_R * HOLE_R * (PI - n / 2.0 * libm::sin(2.0 * PI / n));
    let theta = PI / 2.0 / arc as f64;
    let spandrel = arc as f64 * FILLET_R * FILLET_R / 2.0 * (theta - libm::sin(theta));
    assert!(spandrel <= arc as f64 * FILLET_R * FILLET_R * theta * theta * theta / 12.0);
    volume + 4.0 * 5.0 * hole_short + 40.0 * spandrel
}

/// The bracket's mesh, as a 3MF holds it, against the hand, meshed at the tolerance
/// `tolerance`; the volume it encloses. Printable ([`printable`]); each hole's two rims of
/// [`chords`]' count of points, and each of the fillet's two arcs one more than its chords;
/// nothing else a vertex but the profile's five corners left at each end (the walls and the
/// fillet are meshed with no nodes inside them, a cylinder's splitter placing none,
/// docs/ports/mesh-export.md), so 8 rims, 2 arcs and 10 corners; and, closed and of genus 4
/// (four through holes), V − E + F = 2 − 2 · 4 with E = 3F/2, so F = 2V + 12. Its box the
/// part's, [0, 0, 0]–[60, 40, 50], exactly (its corners are the B-rep's vertices, whole
/// numbers in 32 bits too); and its volume the part's and its chords' segments ([`enclosed`]),
/// to what rounding each coordinate to 32 bits moves it ([`rounding`]).
pub fn the_bracket_s_mesh(mesh: &Object, tolerance: f64) -> f64 {
    let mesh_volume = printable(mesh);
    let rim = chords(2.0 * PI, HOLE_R, tolerance);
    let arc = chords(PI / 2.0, FILLET_R, tolerance);
    let any = |_: &[f64; 3]| true;
    for y in [12.0, 28.0] {
        for at in [0.0, 5.0] {
            let v = &mesh.vertices;
            assert_eq!(
                on_circle(v, 2, at, [40.0, y, at], HOLE_R, any),
                rim,
                "base, y {y}"
            );
            assert_eq!(
                on_circle(v, 0, at, [at, y, 35.0], HOLE_R, any),
                rim,
                "upright, y {y}"
            );
        }
    }
    let corner = |v: &[f64; 3]| v[0] <= 10.0 && v[2] <= 10.0;
    for y in [0.0, 40.0] {
        let points = on_circle(&mesh.vertices, 1, y, [10.0, y, 10.0], FILLET_R, corner);
        assert_eq!(points, arc + 1, "the fillet's arc at y {y}");
    }
    let vertices = 8 * rim + 2 * (arc + 1) + 2 * 5;
    assert_eq!(mesh.vertices.len(), vertices);
    assert_eq!(mesh.triangles.len(), 2 * vertices + 12);
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for p in &mesh.vertices {
        for i in 0..3 {
            low[i] = low[i].min(p[i]);
            high[i] = high[i].max(p[i]);
        }
    }
    assert_eq!((low, high), ([0.0; 3], [60.0, 40.0, 50.0]));
    let volume = 22000.0 - 401.25 * PI;
    let enclosed = enclosed(volume, rim, arc);
    let rounding = rounding();
    assert!(
        (mesh_volume - enclosed).abs() <= rounding,
        "the mesh encloses {mesh_volume} mm³; by hand {enclosed} (the part's {volume}), to {rounding}"
    );
    mesh_volume
}

/// How far rounding each of the mesh's coordinates to 32 bits may move the volume it
/// encloses: a vertex by at most √3 · 60 · 2⁻²⁴, the volume by at most that times the
/// surface (under 10,000 mm²), ε ≈ 0.062 mm³ either way.
pub fn rounding() -> f64 {
    10_000.0 * 3f64.sqrt() * 60.0 / 16_777_216.0
}
