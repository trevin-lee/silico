//! Revolutions OCCT keeps as a `SurfaceOfRevolution`, measured through a kernel worker: a
//! barrel (a revolved arc whose circle reaches the axis) through 2π, 1.5π, and 0.5π, its
//! volume, area, and centre of mass, and with part of it cut away by a box; the fixture's
//! cubic cap revolved 2π and 1.5π, and its centre; and the horn torus primitive, alone and
//! fused with a box off its axis. Whole turns to round-off, a part turn's centre to 2e-9,
//! the fused torus to 1e-10 (its cut circles written as B-splines). OCCT 7.9.3's precise integration took such a surface's angle as one span of six
//! Gauss points, 2.2e-7 low on the barrel and 3.9e-6 on the cap; patch 0003 splits it in
//! three, as OCCT splits a torus's (docs/ports/measure.md, Known wrong results). Every
//! expected value is by hand from the profile, independent of OCCT: closed forms, Pappus,
//! and Gauss–Legendre on polynomials it integrates exactly or on a smooth integrand to
//! convergence.
//! The cap needs the FreeCAD oracle (`pixi install -e oracle`), which makes its face from
//! the fixture's curve.

use super::close::assert_close;
use super::kernel::kernel;
use super::oracle;
use super::quadrature::{bezier, cap_integral, integral, kind, misses, misses_by};

use std::f64::consts::{PI, TAU};

use silico_core::kernel::KernelWorker;
use silico_kernel::{Axis, BooleanOp, Curve, FaceRegions, Region, SketchPlane};
use silico_protocol::{Blob, Primitive, ShapeData};

// ---- The barrel ----

/// The barrel's profile on XZ (model x and z): x = 1 to the arc through (4, −4), (5, 0),
/// and (4, 4), closed by the lines z = ±4. Its circle's centre is on z = 0 by symmetry, at
/// c with (5 − c)² = (4 − c)² + 4²: c = (5² − 4² − 4²) / (2 (5 − 4)) = −3.5, radius
/// R = 5 − c = 8.5. The centre is 3.5 short of the axis (x = 0) and the radius reaches past
/// it, so OCCT keeps the turned arc as a `SurfaceOfRevolution`: neither a sphere (centre on
/// the axis) nor a torus (centre farther from it than the radius).
const MIDDLE: f64 = 5.0;
const END: [f64; 2] = [4.0, 4.0];
const INNER: f64 = 1.0;

/// The arc's circle: its centre's x and its radius.
fn barrel_circle() -> (f64, f64) {
    let [x, z] = END;
    let c = (MIDDLE * MIDDLE - x * x - z * z) / (2.0 * (MIDDLE - x));
    (c, MIDDLE - c)
}

fn barrel_regions() -> FaceRegions {
    let (c, r) = barrel_circle();
    let [x, z] = END;
    let (low, high) = ([x, -z], [x, z]);
    let angle = libm::atan2(z, x - c);
    let line = |start, end| Curve::Line { start, end };
    FaceRegions {
        plane: SketchPlane::Xz,
        regions: vec![Region {
            outer: vec![
                line([INNER, -z], low),
                Curve::Arc {
                    center: [c, 0.0],
                    start: low,
                    end: high,
                    radius: r,
                    start_angle: -angle,
                    end_angle: angle,
                },
                line(high, [INNER, z]),
                line([INNER, z], [INNER, -z]),
            ],
            holes: vec![],
        }],
    }
}

/// The barrel's measures by hand, for a whole turn: its volume, and its faces' areas (the
/// annulus at each end, the arc's turned side, the bore), the profile's own area (each
/// end face of a part turn), and the profile's second moment about the axis, ∫∫ x² dA,
/// whose turn places the centre of mass.
pub struct BarrelMeasures {
    pub volume: f64,
    pub annulus: f64,
    pub side: f64,
    pub bore: f64,
    pub profile: f64,
    pub moment: f64,
}

pub fn barrel_by_hand() -> BarrelMeasures {
    let (c, r) = barrel_circle();
    let [x, h] = END;
    // The arc's x at height z: xo(z) = c + √(R² − z²), from −h to h; its half-angle α,
    // sin α = h/R. ∫ √(R² − z²) dz over [−h, h] = h √(R² − h²) + R² α.
    let alpha = libm::asin(h / r);
    let root = h * libm::sqrt(r * r - h * h) + r * r * alpha;
    // V = π ∫ (xo² − 1) dz = π [2h (c² + R² − 1) − 2h³/3 + 2c ∫ √(R² − z²) dz].
    let volume =
        PI * (2.0 * h * (c * c + r * r - INNER * INNER) - 2.0 * h * h * h / 3.0 + 2.0 * c * root);
    // Pappus on the side: the arc's length 2Rα times the turn of its centroid, which is
    // R sin α / α from the circle's centre: 2π (2Rα c + 2R² sin α) = 4πR (cα + h).
    let side = 4.0 * PI * r * (c * alpha + h);
    // ∫∫ x² dA = ∫ (xo³ − 1)/3 dz, xo³ = c³ + 3c² s + 3c s² + s³ with s = √(R² − z²):
    // ∫ s² dz = 2hR² − 2h³/3, and ∫ s³ dz = (h/4)(5R² − 2h²) √(R² − h²) + (3R⁴/4) α.
    let squares = 2.0 * h * r * r - 2.0 * h * h * h / 3.0;
    let cubes = h / 4.0 * (5.0 * r * r - 2.0 * h * h) * libm::sqrt(r * r - h * h)
        + 3.0 * r * r * r * r / 4.0 * alpha;
    let moment = (2.0 * h * c * c * c + 3.0 * c * c * root + 3.0 * c * squares + cubes
        - 2.0 * h * INNER * INNER * INNER)
        / 3.0;
    BarrelMeasures {
        volume,
        annulus: PI * (x * x - INNER * INNER),
        side,
        bore: TAU * INNER * 2.0 * h,
        // ∫ (xo − 1) dz.
        profile: 2.0 * h * (c - INNER) + root,
        moment,
    }
}

fn barrel_turned(k: &mut KernelWorker, angle: f64) -> ShapeData {
    let faces = k.make_faces(&barrel_regions()).unwrap();
    let z = Axis {
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
    };
    k.revolution(&faces.shape, z, 0.0, angle).unwrap().shape
}

#[test]
fn the_barrels_hand_values_are_the_closed_form() {
    // π (668 − 128/3 + 2c (30 + R² asin(4/R))), c = −3.5 and R = 8.5 (Fable, 2026-10-25),
    // 526.3343139891515 in Python; and the areas, whose sum (388.5995488823284) OCCT's
    // default and precise integrations both give.
    let (c, r) = barrel_circle();
    assert_eq!((c, r), (-3.5, 8.5));
    let b = barrel_by_hand();
    let closed = PI * (668.0 - 128.0 / 3.0 + 2.0 * c * (30.0 + r * r * libm::asin(4.0 / r)));
    assert_close("the volume", b.volume, closed, 1e-15);
    assert_close("the volume", b.volume, 526.334_313_989_151_5, 1e-15);
    assert_close(
        "the area",
        2.0 * b.annulus + b.side + b.bore,
        388.599_548_882_328_4,
        1e-15,
    );
    // The volume again, as π ∫ (xo² − 1) dz by quadrature.
    let xo = |z: f64| c + libm::sqrt(r * r - z * z);
    let quadrature = PI * integral(-4.0, 4.0, 64, |z| xo(z) * xo(z) - 1.0);
    assert_close("by quadrature", quadrature, b.volume, 1e-14);
    // The second moment, 273.0031557451118 in Python (closed form, and 20-point
    // Gauss–Legendre on 64 pieces), and by quadrature here.
    assert_close("the moment", b.moment, 273.003_155_745_111_8, 1e-15);
    let quadrature = integral(-4.0, 4.0, 64, |z| (xo(z) * xo(z) * xo(z) - 1.0) / 3.0);
    assert_close("the moment by quadrature", quadrature, b.moment, 1e-14);
}

#[test]
fn a_barrel_turned_whole_or_in_part_measures_as_by_hand() {
    // Its volume to 1e-12, every face's kind and area, and the solid's area. OCCT 7.9.3
    // without patch 0003 had the whole turn 2.2e-7 low (526.334197 in a fingerprint) and
    // 1.5π 5.4e-9 low.
    let b = barrel_by_hand();
    let mut k = kernel();
    let mut cases = Vec::new();
    for (angle, what) in [(TAU, "2π"), (1.5 * PI, "1.5π"), (0.5 * PI, "0.5π")] {
        let part = angle / TAU;
        let shape = barrel_turned(&mut k, angle);
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{what}");
        cases.push((format!("{what}: the volume"), s.volume, b.volume * part));
        let ends = if angle == TAU { 0.0 } else { 2.0 * b.profile };
        cases.push((
            format!("{what}: the area"),
            s.area,
            (2.0 * b.annulus + b.side + b.bore) * part + ends,
        ));
        // The faces in OCCT's order: the bottom annulus, the arc's side (a surface of
        // revolution, never a torus), the top annulus, the bore; then, for a part turn, the
        // profile at its start and at its end.
        let d = k.describe(&shape).unwrap();
        let mut want = vec![
            ("plane", b.annulus * part),
            ("revolution", b.side * part),
            ("plane", b.annulus * part),
            ("cylinder", b.bore * part),
        ];
        if angle != TAU {
            want.extend([("plane", b.profile), ("plane", b.profile)]);
        }
        let kinds: Vec<&str> = d.faces.iter().map(|f| kind(&f.surface)).collect();
        let want_kinds: Vec<&str> = want.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, want_kinds, "{what}");
        for (i, (face, (_, area))) in d.faces.iter().zip(&want).enumerate() {
            cases.push((format!("{what}: face {}'s area", i + 1), face.area, *area));
        }
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn a_barrels_centre_of_mass_turned_whole_or_in_part_is_by_hand() {
    // The profile is in the half plane y = 0, x > 0, turned about +z from angle 0 (no
    // start), right-handed: (ρ, 0, z) goes to (ρ cos φ, ρ sin φ, z), and dV = ρ dρ dz dφ.
    // So ∫ x dV = M sin θ and ∫ y dV = M (1 − cos θ), M = ∫∫ ρ² dA (the barrel's moment),
    // and ∫ z dV = 0, the profile being symmetric about z = 0; each over V θ/2π. A whole
    // turn's centre is the origin; 1.5π's (−0.6915836531753319, 0.6915836531753321, 0) and
    // 0.5π's (2.0747509595259954, 2.074750959525995, 0) in Python. OCCT 7.9.3 without
    // patch 0003 had the whole turn's 9.3e-4 off the axis. The whole turn's is held to
    // 1e-12; a part turn's to 2e-9 (9.6e-10 measured at 1.5π, 5.2e-11 at 0.5π, 2026-10-25):
    // not the U integral's (six points a span get its sine and cosine to about 2e-12), but
    // OCCT's U error estimate (its four-point pass's, 3.7e-7 on the first span) loosening
    // the tolerance its refinement along the edges (L) holds to.
    let b = barrel_by_hand();
    let mut k = kernel();
    let mut whole = Vec::new();
    let mut part = Vec::new();
    for (angle, what) in [(TAU, "2π"), (1.5 * PI, "1.5π"), (0.5 * PI, "0.5π")] {
        let cases = if angle == TAU { &mut whole } else { &mut part };
        let volume = b.volume * angle / TAU;
        let want = [
            b.moment * libm::sin(angle) / volume,
            b.moment * (1.0 - libm::cos(angle)) / volume,
            0.0,
        ];
        let shape = barrel_turned(&mut k, angle);
        let s = k.summarize(&shape).unwrap();
        for (axis, (got, want)) in ["x", "y", "z"].iter().zip(s.center.iter().zip(want)) {
            cases.push((format!("{what}: the centre's {axis}"), *got, want));
        }
    }
    let mut missed = misses_by(&whole, 1e-12);
    missed.extend(misses_by(&part, 2e-9));
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

/// The barrel turned about the sketch's x (model x), not its y: its profile on XZ is the
/// first one's with the sketch's x and y swapped, x = ±4 from y = 1 to the arc through
/// (4, 4), (0, 5), and (−4, 4) about (0, −3.5), counter-clockwise from 62° to 118°. (The
/// barrel about z, its arc from −28° to 28° across the circle's angle 0, isn't cut by
/// OCCT 7.9.3's boolean at all: the cut below leaves it whole, and so does a box through
/// its side; open, docs/ports/measure.md, Known wrong results: a sketch arc across its
/// circle's angle 0.)
fn barrel_about_x(k: &mut KernelWorker) -> ShapeData {
    let (c, r) = barrel_circle();
    let [x, z] = END;
    let (right, left) = ([z, x], [-z, x]);
    let angle = libm::atan2(x - c, z);
    let line = |start, end| Curve::Line { start, end };
    let regions = FaceRegions {
        plane: SketchPlane::Xz,
        regions: vec![Region {
            outer: vec![
                line([-z, INNER], [z, INNER]),
                line([z, INNER], right),
                Curve::Arc {
                    center: [0.0, c],
                    start: right,
                    end: left,
                    radius: r,
                    start_angle: angle,
                    end_angle: PI - angle,
                },
                line(left, [-z, INNER]),
            ],
            holes: vec![],
        }],
    };
    let faces = k.make_faces(&regions).unwrap();
    let along_x = Axis {
        origin: [0.0; 3],
        direction: [1.0, 0.0, 0.0],
    };
    k.revolution(&faces.shape, along_x, 0.0, TAU).unwrap().shape
}

/// The barrel's volume with y > 2 cut away, by quadrature along its axis: at each point
/// its section is the annulus 1 ≤ r ≤ xo less the cap of the disc of radius xo beyond the
/// chord y = 2, xo² acos(2/xo) − 2 √(xo² − 4) (the bore, r ≤ 1, is all on the near side).
/// Smooth on [−4, 4] (xo ≥ 4 > 2), so Gauss–Legendre converges fast: 32 and 64 pieces
/// agree to round-off.
fn cut_barrel_by_hand(pieces: u32) -> f64 {
    let (c, r) = barrel_circle();
    let section = |z: f64| {
        let rho = c + libm::sqrt(r * r - z * z);
        let cap = rho * rho * libm::acos(2.0 / rho) - 2.0 * libm::sqrt(rho * rho - 4.0);
        PI * rho * rho - cap - PI * INNER * INNER
    };
    integral(-4.0, 4.0, pieces, section)
}

#[test]
fn a_barrel_cut_by_a_box_measures_as_by_hand_to_the_asked_precision() {
    // The barrel about x, whole to 1e-12, then less the box from (−20, 2, −20) to
    // (20, 20, 20): its side is then a surface of revolution trimmed by an intersection
    // curve that's no iso-line, so the integration runs along that curve (L) to the asked
    // 1e-9. Fable's quadrature in Python (20 points, composite, converged to 1e-16) gives
    // 395.4776670194995; OCCT gives it 1.2e-9 low with patch 0003 and 1.3e-9 without (on
    // the barrel about z, in Fable's probe): the residual is L's, so this test proves the
    // patch doesn't make a trimmed surface of revolution worse, not that it mends one.
    let by_hand = cut_barrel_by_hand(64);
    assert_close("converged", cut_barrel_by_hand(32), by_hand, 1e-15);
    assert_close("Fable's quadrature", by_hand, 395.477_667_019_499_5, 1e-15);
    let mut k = kernel();
    let barrel = barrel_about_x(&mut k);
    let whole = k.summarize(&barrel).unwrap();
    assert_close("the whole", whole.volume, barrel_by_hand().volume, 1e-12);
    let block = k
        .make(Primitive::Box {
            dx: 40.0,
            dy: 18.0,
            dz: 40.0,
        })
        .unwrap();
    let block = k.translate(&block, [-20.0, 2.0, -20.0]).unwrap().shape;
    let cut = k.boolean(BooleanOp::Cut, &barrel, &block).unwrap().shape;
    let s = k.summarize(&cut).unwrap();
    assert!(s.valid);
    assert_close("the cut", s.volume, by_hand, 2e-9);
    // Its side, cut in two along y = 2, still a surface of revolution.
    let d = k.describe(&cut).unwrap();
    let kinds: Vec<&str> = d.faces.iter().map(|f| kind(&f.surface)).collect();
    assert_eq!(
        kinds.iter().filter(|k| **k == "revolution").count(),
        2,
        "{kinds:?}"
    );
}

// ---- The fixture's cubic cap ----

/// The cap's face: Edge4 of curves_and_surfaces.brep (the fixtures' README), closed by its
/// chord, made a face by FreeCAD; and the curve's degree, poles, knots, and multiplicities,
/// so the test knows the curve its hand values are of.
const CAP: &str = r#"import json, Part
shape = Part.Shape()
shape.importBrep(PATH)
edge = shape.Edges[3]
curve = edge.Curve
assert curve.TypeId == "Part::GeomBSplineCurve", curve.TypeId
assert (edge.FirstParameter, edge.LastParameter) == (0.0, 1.0)
first, last = edge.Vertexes[0].Point, edge.Vertexes[-1].Point
face = Part.Face(Part.Wire([edge, Part.makeLine(last, first)]))
assert face.isValid()
spline = [curve.Degree, [[p.x, p.y, p.z] for p in curve.getPoles()], curve.getKnots(), curve.getMultiplicities()]
print("SILICO_ORACLE " + json.dumps([spline, face.exportBrepToString()]))
"#;

/// The fixture's cubic (poles (0, 4), (1, 5), (2, 4), (3, 5), (4, 4), knots 0, 0.5, 1 of
/// multiplicities 4, 1, 4) as its two Bézier spans: the knot 0.5 inserted twice by Boehm's
/// rule (each new pole halfway between two, as every α there is ½), by hand: (1.5, 4.5)
/// and (2.5, 4.5), then (2, 4.5) between those.
const SPANS: [[[f64; 2]; 4]; 2] = [
    [[0.0, 4.0], [1.0, 5.0], [1.5, 4.5], [2.0, 4.5]],
    [[2.0, 4.5], [2.5, 4.5], [3.0, 5.0], [4.0, 4.0]],
];

/// ∫∫ f dA over the cap between the cubic and its chord on y = 4, by Green's theorem along
/// the cubic's two spans; `inner` is ∫ f dy from the chord to the point.
fn over_cap(inner: impl Fn([f64; 2]) -> f64) -> f64 {
    cap_integral(&[(0.0, 1.0); 2], |span, t| bezier(&SPANS[span], t), inner)
}

/// The cap's area, ∫∫ dA, and its centroid's height, ∫∫ y dA over that: polynomials of
/// degree 5 and 8 along each span, so the eight-point rule is exact.
fn cap_by_hand() -> (f64, f64) {
    let area = over_cap(|[_, y]| y - 4.0);
    let moment = over_cap(|[_, y]| (y * y - 16.0) / 2.0);
    (area, moment / area)
}

/// The cap's face, made by FreeCAD from the fixture's curve, which must be the cubic
/// `SPANS` are of.
fn cap_face() -> ShapeData {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/brep/curves_and_surfaces.brep"
    );
    let script = CAP.replace("PATH", &format!("{path:?}"));
    let [spline, face]: [serde_json::Value; 2] =
        serde_json::from_value(oracle::freecad(&script)).unwrap();
    assert_eq!(
        spline,
        serde_json::json!([
            3,
            POLES.map(|[x, y]| [x, y, 0.0]),
            [0.0, 0.5, 1.0],
            [4, 1, 4]
        ])
    );
    let cap = silico_kernel::from_text_brep(face.as_str().unwrap().as_bytes()).unwrap();
    ShapeData::checked(Blob::inline(silico_kernel::to_brep(&cap).unwrap()), true)
}

/// The cubic's poles.
const POLES: [[f64; 2]; 5] = [[0.0, 4.0], [1.0, 5.0], [2.0, 4.0], [3.0, 5.0], [4.0, 4.0]];

/// The line the cap turns about: along x through (0, −5, 0), in the cap's plane, 9 below
/// its chord.
const BELOW_CHORD: Axis = Axis {
    origin: [0.0, -5.0, 0.0],
    direction: [1.0, 0.0, 0.0],
};

#[test]
fn the_fixtures_cubic_cap_turned_whole_and_three_quarters_measures_as_pappus() {
    // About the line along x through (0, −5, 0), in the cap's plane, 9 below its chord:
    // Pappus, 2π × (6809/1596 + 5) × 19/10 = 14789π/420 = 110.62146131890346, and three
    // quarters of that. OCCT 7.9.3 without patch 0003 had them 3.9e-6 and 9.3e-8 low.
    let (area, height) = cap_by_hand();
    assert_close("the cap's area", area, 19.0 / 10.0, 1e-15);
    assert_close("its centroid's height", height, 6809.0 / 1596.0, 1e-15);
    let pappus = TAU * (height + 5.0) * area;
    assert_close("Pappus", pappus, 14789.0 * PI / 420.0, 1e-15);
    assert_close("Pappus", pappus, 110.621_461_318_903_46, 1e-15);

    let cap = cap_face();
    let mut k = kernel();
    let mut cases = Vec::new();
    for (angle, what) in [(TAU, "2π"), (1.5 * PI, "1.5π")] {
        let shape = k.revolution(&cap, BELOW_CHORD, 0.0, angle).unwrap().shape;
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{what}");
        let d = k.describe(&shape).unwrap();
        assert_eq!(kind(&d.faces[0].surface), "revolution", "{what}");
        cases.push((
            format!("{what}: the volume"),
            s.volume,
            pappus * angle / TAU,
        ));
    }
    let missed = misses(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn the_fixtures_cubic_cap_turned_whole_has_its_centre_where_its_symmetry_puts_it() {
    // The poles mirror about x = 2, (0, 4) and (4, 4), (1, 5) and (3, 5), (2, 4) on it, and
    // the knots (0, 0.5, 1) about 0.5, so the curve does, and the cap with its chord; and a
    // whole turn about the line along x through (0, −5, 0) is symmetric about that line. So
    // the solid's centre is (2, −5, 0). And by Green's theorem, its x is ∫∫ x ρ dA over
    // ∫∫ ρ dA, ρ = y + 5 the distance from the axis: polynomials of degree 11 and 8 along
    // each span, so exact, and 2 to round-off. OCCT 7.9.3 without patch 0003 had the centre
    // 0.033 off.
    for (i, [x, y]) in POLES.iter().enumerate() {
        assert_eq!([4.0 - x, *y], POLES[POLES.len() - 1 - i]);
    }
    for (i, [x, y]) in SPANS[0].iter().enumerate() {
        assert_eq!([4.0 - x, *y], SPANS[1][3 - i]);
    }
    // ∫ from the chord, y' = 4, to y of ρ' = y' + 5: ((y + 5)² − 81)/2.
    let turned = over_cap(|[_, y]| ((y + 5.0) * (y + 5.0) - 81.0) / 2.0);
    let x = over_cap(|[x, y]| x * ((y + 5.0) * (y + 5.0) - 81.0) / 2.0) / turned;
    assert_close("the centre's x by Green's theorem", x, 2.0, 1e-15);

    let cap = cap_face();
    let mut k = kernel();
    let shape = k.revolution(&cap, BELOW_CHORD, 0.0, TAU).unwrap().shape;
    let s = k.summarize(&shape).unwrap();
    assert!(s.valid);
    let cases: Vec<(String, f64, f64)> = ["x", "y", "z"]
        .iter()
        .zip(s.center.iter().zip([2.0, -5.0, 0.0]))
        .map(|(axis, (got, want))| (format!("the centre's {axis}"), *got, want))
        .collect();
    let missed = misses_by(&cases, 1e-12);
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

// ---- The horn torus ----

/// Silico's torus primitive with its tube touching the axis, radius1 = radius2 = 10
/// (tests/oracle/primitives.json's torus_horn): FreeCAD's construction (bridge.cpp's
/// make_torus), the circle about (R, 0, 0) of radius r = R in XZ turned about z, which
/// OCCT keeps as a `SurfaceOfRevolution`, its circle's centre exactly its radius from the
/// axis (neither sphere nor torus).
const HORN: f64 = 10.0;

fn horn_torus(k: &mut KernelWorker) -> ShapeData {
    k.make(Primitive::Torus {
        major_radius: HORN,
        minor_radius: HORN,
    })
    .unwrap()
}

#[test]
fn a_horn_torus_measures_as_by_hand() {
    // Pappus: the disc πr² turned about R, V = 2π²Rr²; the circle 2πr turned about R,
    // A = 4π²Rr; the centre at its placement's origin, by symmetry. OCCT 7.9.3 without
    // patch 0003 had the centre 7.46e-6 off (the determinism sweep's torus_horn summary).
    let (big, small) = (HORN, HORN);
    let mut k = kernel();
    let shape = horn_torus(&mut k);
    let s = k.summarize(&shape).unwrap();
    assert!(s.valid);
    let d = k.describe(&shape).unwrap();
    let kinds: Vec<&str> = d.faces.iter().map(|f| kind(&f.surface)).collect();
    assert_eq!(kinds, ["revolution"]);
    let mut missed = misses(
        &[
            (
                "the volume".into(),
                s.volume,
                2.0 * PI * PI * big * small * small,
            ),
            ("the area".into(), s.area, 4.0 * PI * PI * big * small),
        ],
        1e-12,
    );
    let centre: Vec<(String, f64, f64)> = ["x", "y", "z"]
        .iter()
        .zip(s.center)
        .map(|(axis, got)| (format!("the centre's {axis}"), got, 0.0))
        .collect();
    missed.extend(misses_by(&centre, 1e-12 * big));
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn a_horn_torus_fused_with_a_box_off_its_axis_measures_as_by_hand() {
    // The box from (−22, −25, 5) to (30, 25, 15): off the axis (its middle at x = 4), its
    // sides clear of the torus (which reaches 20 from the axis), its floor, z = 5, through
    // the tube, which it cuts along two circles, ρ = R ∓ w with w = √(r² − 25) = 5√3:
    // iso-lines of the torus's surface. The torus above z = 5, by slices, each the annulus
    // between R ∓ √(r² − z²), of area 4πR √(r² − z²): its volume 4πR ∫ √(r² − z²) dz over
    // [5, r] = 4πR [z √(r² − z²)/2 + (r²/2) asin(z/r)] = 40π (50π/3 − 12.5√3), and its
    // moment ∫ z dV = 4πR (r² − 25)^(3/2)/3 = 5000√3 π; its side the tube's circle from
    // 30° to 150° turned, 2πr ∫ (R + r cos ψ) dψ = 4π²Rr/3. So the fusion is the box's
    // 26000 plus the torus's 2000π² less that cap; its area the box's 7240 less the floor's
    // annulus inside the tube, 4πRw = 200√3 π, plus two thirds of the torus's 400π²; and
    // its centre the box's (4, 0, 10) times 26000 less the cap's moment, over the fusion's
    // volume (the torus's own centre is the origin). The boolean writes the two circles as
    // B-splines (of degree 7 and 188 poles, and of 8 and 233), about 2e-10 off the circles
    // at their middles, so the solid is that far from the one by hand: held to 1e-10, its
    // volume, area, and centre measured 9.7e-12, 4.7e-11, and 2.6e-11 off (2026-10-25).
    let (big, small) = (HORN, HORN);
    let root3 = libm::sqrt(3.0);
    let cap = 40.0 * PI * (50.0 * PI / 3.0 - 12.5 * root3);
    let cap_moment = 5000.0 * root3 * PI;
    let torus = 2.0 * PI * PI * big * small * small;
    let volume = 26_000.0 + torus - cap;
    let area = 7240.0 - 200.0 * root3 * PI + 2.0 / 3.0 * 4.0 * PI * PI * big * small;
    let centre = [
        26_000.0 * 4.0 / volume,
        0.0,
        (26_000.0 * 10.0 - cap_moment) / volume,
    ];
    let mut k = kernel();
    let torus = horn_torus(&mut k);
    let block = k
        .make(Primitive::Box {
            dx: 52.0,
            dy: 50.0,
            dz: 10.0,
        })
        .unwrap();
    let block = k.translate(&block, [-22.0, -25.0, 5.0]).unwrap().shape;
    let fused = k.boolean(BooleanOp::Fuse, &torus, &block).unwrap().shape;
    let s = k.summarize(&fused).unwrap();
    assert!(s.valid);
    let d = k.describe(&fused).unwrap();
    // The torus below the floor in two faces, both surfaces of revolution: the tube's
    // outside, and its inside, which meets the axis at the origin; the box's floor in two,
    // the disc of ρ < R − w inside the torus's hole and the rest; the box's top and sides.
    let kinds: Vec<&str> = d.faces.iter().map(|f| kind(&f.surface)).collect();
    let count = |which: &str| kinds.iter().filter(|k| **k == which).count();
    assert_eq!((count("revolution"), count("plane")), (2, 7), "{kinds:?}");
    let mut missed = misses(
        &[
            ("the volume".into(), s.volume, volume),
            ("the area".into(), s.area, area),
        ],
        1e-10,
    );
    let centre: Vec<(String, f64, f64)> = ["x", "y", "z"]
        .iter()
        .zip(s.center.iter().zip(centre))
        .map(|(axis, (got, want))| (format!("the centre's {axis}"), *got, want))
        .collect();
    missed.extend(misses_by(&centre, 1e-10));
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}
