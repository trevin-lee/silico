//! Revolutions OCCT keeps as a `SurfaceOfRevolution`, measured through a kernel worker to
//! round-off: a barrel (a revolved arc whose circle reaches the axis) through 2π, 1.5π, and
//! 0.5π, and with part of it cut away by a box; and the fixture's cubic cap revolved 2π and
//! 1.5π. OCCT 7.9.3's precise integration took such a surface's angle as one span of six
//! Gauss points, 2.2e-7 low on the barrel and 3.9e-6 on the cap; patch 0003 splits it in
//! three, as OCCT splits a torus's (docs/ports/measure.md, Known wrong results). Every
//! expected value is by hand from the profile, independent of OCCT: closed forms, Pappus,
//! and Gauss–Legendre on polynomials it integrates exactly or on a smooth integrand to
//! convergence.
//! The cap needs the FreeCAD oracle (`pixi install -e oracle`), which makes its face from
//! the fixture's curve.

use super::close::assert_close;
use super::kernel::kernel;
use super::numbers::relative_error;
use super::oracle;

use std::f64::consts::{PI, TAU};

use silico_core::kernel::KernelWorker;
use silico_kernel::{Axis, BooleanOp, Curve, FaceRegions, Region, SketchPlane};
use silico_protocol::{Blob, Primitive, ShapeData, Surface};

/// Gauss–Legendre's eight nodes on [-1, 1] and their weights: exact for polynomials up to
/// degree 15.
const NODES: [f64; 8] = [
    -0.960_289_856_497_536_2,
    -0.796_666_477_413_626_7,
    -0.525_532_409_916_329,
    -0.183_434_642_495_649_8,
    0.183_434_642_495_649_8,
    0.525_532_409_916_329,
    0.796_666_477_413_626_7,
    0.960_289_856_497_536_2,
];
const WEIGHTS: [f64; 8] = [
    0.101_228_536_290_376_26,
    0.222_381_034_453_374_47,
    0.313_706_645_877_887_3,
    0.362_683_783_378_362,
    0.362_683_783_378_362,
    0.313_706_645_877_887_3,
    0.222_381_034_453_374_47,
    0.101_228_536_290_376_26,
];

/// The integral of `f` over [a, b], cut in `pieces`, eight Gauss–Legendre points on each.
fn integral(a: f64, b: f64, pieces: u32, f: impl Fn(f64) -> f64) -> f64 {
    let step = (b - a) / f64::from(pieces);
    let mut sum = 0.0;
    for k in 0..pieces {
        let (lo, hi) = (a + step * f64::from(k), a + step * f64::from(k + 1));
        for (x, w) in NODES.iter().zip(WEIGHTS) {
            sum += w * (hi - lo) / 2.0 * f((lo + hi) / 2.0 + (hi - lo) / 2.0 * x);
        }
    }
    sum
}

/// Each case whose `got` is further than `tolerance` (relative) from `want`, said with its
/// error, so a failure lists every case that misses, not only the first.
fn misses(cases: &[(String, f64, f64)], tolerance: f64) -> Vec<String> {
    cases
        .iter()
        .filter(|(_, got, want)| relative_error(*got, *want) > tolerance)
        .map(|(what, got, want)| {
            format!(
                "{what}: got {got}, want {want} (relative error {:e})",
                relative_error(*got, *want)
            )
        })
        .collect()
}

/// A face's kind of surface, as a word.
fn kind(surface: &Surface) -> &'static str {
    match surface {
        Surface::Plane { .. } => "plane",
        Surface::Cylinder { .. } => "cylinder",
        Surface::Cone { .. } => "cone",
        Surface::Sphere { .. } => "sphere",
        Surface::Torus { .. } => "torus",
        Surface::Revolution => "revolution",
        Surface::Bezier => "bezier",
        Surface::Bspline => "bspline",
        Surface::Extrusion => "extrusion",
        Surface::Offset => "offset",
        Surface::Other => "other",
    }
}

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
/// annulus at each end, the arc's turned side, the bore), and the profile's own area (each
/// end face of a part turn).
struct Barrel {
    volume: f64,
    annulus: f64,
    side: f64,
    bore: f64,
    profile: f64,
}

fn barrel_by_hand() -> Barrel {
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
    Barrel {
        volume,
        annulus: PI * (x * x - INNER * INNER),
        side,
        bore: TAU * INNER * 2.0 * h,
        // ∫ (xo − 1) dz.
        profile: 2.0 * h * (c - INNER) + root,
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

/// The barrel turned about the sketch's x (model x), not its y: its profile on XZ is the
/// first one's with the sketch's x and y swapped, x = ±4 from y = 1 to the arc through
/// (4, 4), (0, 5), and (−4, 4) about (0, −3.5), counter-clockwise from 62° to 118°. (The
/// barrel about z, its arc from −28° to 28° across the circle's angle 0, isn't cut by
/// OCCT 7.9.3's boolean at all: the cut below leaves it whole, and so does a box through
/// its side; open, 2026-10-25.)
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

/// A cubic Bézier span's point and derivative at `t`.
fn bezier(p: &[[f64; 2]; 4], t: f64) -> ([f64; 2], [f64; 2]) {
    let s = 1.0 - t;
    let b = [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t];
    let db = [
        -3.0 * s * s,
        3.0 * s * s - 6.0 * t * s,
        6.0 * t * s - 3.0 * t * t,
        3.0 * t * t,
    ];
    let mut point = [0.0; 2];
    let mut tangent = [0.0; 2];
    for i in 0..4 {
        for d in 0..2 {
            point[d] += b[i] * p[i][d];
            tangent[d] += db[i] * p[i][d];
        }
    }
    (point, tangent)
}

/// The cap between the cubic and its chord on y = 4: its area, ∫ (y − 4) dx, and its
/// centroid's height, ∫ (y² − 16)/2 dx over that (Green's theorem along the curve; the
/// chord adds nothing, y constant and dx along it the curve's back). Polynomials of degree
/// 5 and 8 on each span, so the eight-point rule is exact.
fn cap_by_hand() -> (f64, f64) {
    let along = |f: &dyn Fn([f64; 2], [f64; 2]) -> f64| {
        SPANS
            .iter()
            .map(|span| {
                integral(0.0, 1.0, 1, |t| {
                    let (c, d) = bezier(span, t);
                    f(c, d)
                })
            })
            .sum::<f64>()
    };
    let area = along(&|c, d| (c[1] - 4.0) * d[0]);
    let moment = along(&|c, d| (c[1] * c[1] - 16.0) / 2.0 * d[0]);
    (area, moment / area)
}

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

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/brep/curves_and_surfaces.brep"
    );
    let script = CAP.replace("PATH", &format!("{path:?}"));
    let [spline, face]: [serde_json::Value; 2] =
        serde_json::from_value(oracle::freecad(&script)).unwrap();
    // The curve the spans are of.
    assert_eq!(
        spline,
        serde_json::json!([
            3,
            [
                [0.0, 4.0, 0.0],
                [1.0, 5.0, 0.0],
                [2.0, 4.0, 0.0],
                [3.0, 5.0, 0.0],
                [4.0, 4.0, 0.0]
            ],
            [0.0, 0.5, 1.0],
            [4, 1, 4]
        ])
    );
    let cap = silico_kernel::from_text_brep(face.as_str().unwrap().as_bytes()).unwrap();
    let cap = ShapeData::checked(Blob::inline(silico_kernel::to_brep(&cap).unwrap()), true);
    let mut k = kernel();
    let axis = Axis {
        origin: [0.0, -5.0, 0.0],
        direction: [1.0, 0.0, 0.0],
    };
    let mut cases = Vec::new();
    for (angle, what) in [(TAU, "2π"), (1.5 * PI, "1.5π")] {
        let shape = k.revolution(&cap, axis, 0.0, angle).unwrap().shape;
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
