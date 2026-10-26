//! OCCT patch 0004 (`BRepGProp_Gauss` refines U to the asked precision, as it refines L):
//! linear extrusions of B-spline curves, and B-spline faces' areas, measured as they are
//! through a kernel worker (docs/ports/measure.md, Known wrong results: what OCCT's precise
//! integration gave before the patch). Each curve is made by FreeCAD from its poles and
//! knots (`pixi install -e oracle`), and its expected measures come from the same poles and
//! knots by hand, independently of OCCT: the curve evaluated by Cox–de Boor, the cap's area
//! and centroid by Green's theorem, and the curve's length, each integrated per knot span
//! by Gauss–Legendre. The hand values are task B's (dropped), whose conversion of an
//! extrusion to a B-spline surface before measuring the patch makes unneeded.

use super::close::assert_close;
use super::geometry::{apply, cross, norm, rotation};
use super::kernel::kernel;
use super::oracle::{self, freecad_face, from_freecad};
use super::quadrature::{basis, basis_derivative, integral, kind, misses};

use std::path::Path;

use silico_core::kernel::KernelWorker;
use silico_protocol::{ShapeData, ShapeSummary};

/// How far from its value by hand a measure may be, relative, where the patched
/// integration gets it to round-off or near it (6e-13 at worst measured: the tube's
/// volume). Before the patch, these were up to 9.4e-4 off, and areas up to 3.9%
/// (Known wrong results).
const TIGHT: f64 = 1e-12;

/// The same for the uneven four-span cubic and the tube, whose integrand along U the
/// patch refines until its two Gauss orders agree to the asked 1e-9 of each strip's
/// magnitude, not exactly: measured, the cubic's volume 7.5e-11 (upright, down, or
/// slanted), its extruded face's area 3.5e-10 upright and its whole area 1.2e-10 (3.5e-10
/// slanted), and the tube's face 3.8e-10 and whole area 1.7e-10.
const UNEVEN_VOLUME: f64 = 1e-10;
const UNEVEN_AREA: f64 = 5e-10;

/// The same for the slanted cubic's extruded face, described alone: measured 1.14e-9. An
/// adaptive integration to 0.9 × 1e-9 along U and again along L promises the asked
/// precision in each direction, not round-off, and the two can add: twice the asked
/// precision is the bound (docs/ports/measure.md, Known wrong results), measured 1.14 of it
/// (the bisection never lands on the knots 0.3, 1.1, 2, under either U rule).
const SLANTED_AREA: f64 = 2e-9;

/// A B-spline curve in z = 0, as FreeCAD's `buildFromPolesMultsKnots` takes it.
struct Spline {
    poles: Vec<[f64; 2]>,
    knots: Vec<f64>,
    multiplicities: Vec<i32>,
    degree: usize,
    periodic: bool,
}

impl Spline {
    /// Python: the curve as `curve`, its poles and knots exact (Rust's `{:?}` of an f64
    /// is the shortest text that reads back as it).
    fn script(&self) -> String {
        let poles: Vec<String> = self
            .poles
            .iter()
            .map(|[x, y]| format!("V({x:?}, {y:?}, 0)"))
            .collect();
        let knots: Vec<String> = self.knots.iter().map(|k| format!("{k:?}")).collect();
        format!(
            "curve = Part.BSplineCurve()\n\
             curve.buildFromPolesMultsKnots([{}], {:?}, [{}], {}, {})\n",
            poles.join(", "),
            self.multiplicities,
            knots.join(", "),
            if self.periodic { "True" } else { "False" },
            self.degree,
        )
    }

    /// Python: the cap as `face`, the curve closed by its chord (or by itself, periodic).
    fn cap_script(&self) -> String {
        let wire = if self.periodic {
            "Part.Wire([curve.toShape()])".to_owned()
        } else {
            let ([x0, y0], [x1, y1]) = self.ends();
            format!(
                "Part.Wire([curve.toShape(), Part.makeLine(V({x1:?}, {y1:?}, 0), V({x0:?}, {y0:?}, 0))])"
            )
        };
        format!("{}face = Part.Face({wire})\n", self.script())
    }

    /// The cap's face, made by FreeCAD.
    fn cap(&self) -> ShapeData {
        freecad_face(&self.cap_script())
    }

    /// The poles and the flat knot sequence the curve is evaluated on, and the parameter
    /// range of the curve. A periodic curve's poles are wrapped by its degree and its knots
    /// continued by its period, so it's evaluated as the open curve it is over one period.
    fn flat(&self) -> (Vec<[f64; 2]>, Vec<f64>, [f64; 2]) {
        let p = self.degree;
        if !self.periodic {
            let mut flat = Vec::new();
            for (k, &m) in self.knots.iter().zip(&self.multiplicities) {
                flat.extend(std::iter::repeat_n(*k, usize::try_from(m).unwrap()));
            }
            let range = [self.knots[0], *self.knots.last().unwrap()];
            return (self.poles.clone(), flat, range);
        }
        assert!(self.multiplicities.iter().all(|&m| m == 1));
        let n = self.poles.len();
        let period = self.knots[n] - self.knots[0];
        let knot = |j: isize| {
            let n = isize::try_from(n).unwrap();
            let (wraps, i) = (j.div_euclid(n), j.rem_euclid(n));
            self.knots[usize::try_from(i).unwrap()]
                + period * f64::from(i32::try_from(wraps).unwrap())
        };
        let p_i = isize::try_from(p).unwrap();
        let flat = (0..n + 2 * p + 1)
            .map(|j| knot(isize::try_from(j).unwrap() - p_i))
            .collect();
        let poles = (0..n + p).map(|i| self.poles[i % n]).collect();
        (poles, flat, [self.knots[0], self.knots[n]])
    }

    /// The point and the first derivative at `t`, by Cox–de Boor and its derivative.
    fn at(&self, t: f64) -> ([f64; 2], [f64; 2]) {
        let (poles, flat, _) = self.flat();
        let p = self.degree;
        let mut point = [0.0; 2];
        let mut tangent = [0.0; 2];
        for (i, pole) in poles.iter().enumerate() {
            let (n, dn) = (basis(&flat, i, p, t), basis_derivative(&flat, i, p, t));
            for d in 0..2 {
                point[d] += n * pole[d];
                tangent[d] += dn * pole[d];
            }
        }
        (point, tangent)
    }

    /// The spans the curve's distinct knots divide its range into.
    fn spans(&self) -> Vec<[f64; 2]> {
        let [first, last] = self.flat().2;
        let mut knots: Vec<f64> = self
            .knots
            .iter()
            .copied()
            .filter(|k| (first..=last).contains(k))
            .collect();
        knots.dedup();
        knots.windows(2).map(|w| [w[0], w[1]]).collect()
    }

    /// The integral of `f(point, tangent)` along the curve, each span cut in `pieces`.
    fn integral(&self, pieces: u32, f: impl Fn([f64; 2], [f64; 2]) -> f64) -> f64 {
        self.spans()
            .into_iter()
            .map(|[a, b]| {
                integral(a, b, pieces, |t| {
                    let (c, d) = self.at(t);
                    f(c, d)
                })
            })
            .sum()
    }

    /// The curve's ends: its first and last poles, for an open, clamped curve.
    fn ends(&self) -> ([f64; 2], [f64; 2]) {
        (self.poles[0], *self.poles.last().unwrap())
    }

    /// The cap's signed area, ½∮(x dy − y dx) along the curve and back along its chord
    /// (exact: a polynomial of degree 2p − 1 on each span).
    fn signed_area(&self) -> f64 {
        let curve = self.integral(1, |c, d| (c[0] * d[1] - c[1] * d[0]) / 2.0);
        if self.periodic {
            return curve;
        }
        let (start, end) = self.ends();
        curve + (end[0] * start[1] - end[1] * start[0]) / 2.0
    }

    fn area(&self) -> f64 {
        self.signed_area().abs()
    }

    /// The height of the cap's centroid above the x axis: ∬y dA = −∮ y²/2 dx (exact, as the
    /// area's), signed as the area is.
    fn centroid_y(&self) -> f64 {
        let mut moment = self.integral(1, |c, d| -c[1] * c[1] / 2.0 * d[0]);
        if !self.periodic {
            let (start, end) = self.ends();
            // The chord, from the end back to the start: y linear, so y²dx integrates to
            // (x₁ − x₀)(y₀² + y₀y₁ + y₁²)/3.
            let (y0, y1) = (end[1], start[1]);
            moment -= (start[0] - end[0]) * (y0 * y0 + y0 * y1 + y1 * y1) / 6.0;
        }
        moment / self.signed_area()
    }

    /// The area of the curve swept straight along `v`: ∫|C′(t) × v| dt, each span cut in
    /// 64 (smooth there, so to the last digits).
    fn swept_area(&self, v: [f64; 3]) -> f64 {
        self.integral(64, |_, d| norm(cross([d[0], d[1], 0.0], v)))
    }

    /// The area of the chord swept along `v`, none for a closed curve.
    fn chord_swept_area(&self, v: [f64; 3]) -> f64 {
        if self.periodic {
            return 0.0;
        }
        let (start, end) = self.ends();
        norm(cross([start[0] - end[0], start[1] - end[1], 0.0], v))
    }

    /// The prism of the cap along `v`: its volume and its whole area by hand.
    fn prism_by_hand(&self, v: [f64; 3]) -> (f64, f64) {
        let area = 2.0 * self.area() + self.swept_area(v) + self.chord_swept_area(v);
        (self.area() * v[2].abs(), area)
    }
}

/// The fixture's cubic: Edge4 of curves_and_surfaces.brep, which extruded_bspline.brep
/// extrudes (the fixtures' README).
fn fixtures_cubic() -> Spline {
    Spline {
        poles: vec![[0.0, 4.0], [1.0, 5.0], [2.0, 4.0], [3.0, 5.0], [4.0, 4.0]],
        knots: vec![0.0, 0.5, 1.0],
        multiplicities: vec![4, 1, 4],
        degree: 3,
        periodic: false,
    }
}

/// A cubic of four spans over uneven knots, above its chord on y = 0.
fn uneven_cubic() -> Spline {
    Spline {
        poles: vec![
            [0.0, 0.0],
            [1.0, 3.0],
            [2.0, 1.0],
            [3.0, 4.0],
            [4.0, 2.0],
            [5.0, 3.0],
            [6.0, 0.0],
        ],
        knots: vec![0.0, 0.3, 1.1, 2.0, 2.5],
        multiplicities: vec![4, 1, 1, 1, 4],
        degree: 3,
        periodic: false,
    }
}

/// A cubic of one span, as the stress test's prisms (GeomAPI_PointsToBSpline's) are.
fn single_span_cubic() -> Spline {
    Spline {
        poles: vec![[0.0, 0.0], [1.0, 3.0], [3.0, 3.0], [4.0, 0.0]],
        knots: vec![0.0, 1.0],
        multiplicities: vec![4, 4],
        degree: 3,
        periodic: false,
    }
}

/// The measures `measure` gives `shape`, which must be valid.
fn measured(k: &mut KernelWorker, shape: &ShapeData) -> ShapeSummary {
    let s = k.summarize(shape).unwrap();
    assert!(s.valid);
    s
}

/// The area of `shape`'s one face of surface `surface` (a word, as `kind` says it).
fn face_area(k: &mut KernelWorker, shape: &ShapeData, surface: &str) -> f64 {
    let faces: Vec<f64> = k
        .describe(shape)
        .unwrap()
        .faces
        .iter()
        .filter(|f| kind(&f.surface) == surface)
        .map(|f| f.area)
        .collect();
    assert_eq!(faces.len(), 1, "{surface}: {faces:?}");
    faces[0]
}

/// Measured values beside their values by hand: `(what, got, want)`.
type Cases = Vec<(String, f64, f64)>;

/// `shape`, `spline`'s cap swept along `v`, its volume, its whole area, and its extruded
/// face's area as `spline`'s by hand, each `(what, got, want)`.
fn prism_cases(
    k: &mut KernelWorker,
    what: &str,
    spline: &Spline,
    shape: &ShapeData,
    v: [f64; 3],
) -> (Cases, Cases) {
    let s = measured(k, shape);
    let (volume, area) = spline.prism_by_hand(v);
    let swept = face_area(k, shape, "extrusion");
    (
        vec![(format!("{what}: the volume"), s.volume, volume)],
        vec![
            (format!("{what}: the area"), s.area, area),
            (
                format!("{what}: the extruded face's area"),
                swept,
                spline.swept_area(v),
            ),
        ],
    )
}

/// Fails, listing every case that misses, unless each volume is within `volume` of its
/// value by hand and each area within `area`, relatively.
fn assert_cases(
    volumes: &[(String, f64, f64)],
    areas: &[(String, f64, f64)],
    volume: f64,
    area: f64,
) {
    let mut missed = misses(volumes, volume);
    missed.extend(misses(areas, area));
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

fn swept(k: &mut KernelWorker, cap: &ShapeData, v: [f64; 3]) -> ShapeData {
    k.prism(cap, [0.0; 3], v).unwrap().shape
}

#[test]
fn the_hand_measures_give_the_fixtures_cubics_independent_values() {
    // length.py's (outside the repo, Python's own Gauss–Legendre on the same poles): the
    // cap's area 1.9 and the cubic's length, which the fixture's tests pin; and the cap's
    // centroid, 4.266290726817047 above the x axis.
    let cubic = fixtures_cubic();
    assert_close("the cap's area", cubic.area(), 1.9, 1e-14);
    assert_close(
        "the cubic's length",
        cubic.swept_area([0.0, 0.0, 1.0]),
        4.404_055_873_552_449,
        1e-14,
    );
    assert_close(
        "the centroid's height",
        cubic.centroid_y(),
        4.266_290_726_817_047,
        1e-14,
    );
    // The OCCT issue's values (upstream/occt-bgprop-swept-bspline-knots.md): twice the
    // single-span and four-span cubics' lengths, by Python's own Gauss–Legendre.
    assert_close(
        "the single-span cubic's length, twice",
        single_span_cubic().swept_area([0.0, 0.0, 2.0]),
        12.781_952_154_765_39,
        1e-14,
    );
    assert_close(
        "the four-span cubic's length, twice",
        uneven_cubic().swept_area([0.0, 0.0, 2.0]),
        19.869_012_098_272_37,
        1e-14,
    );
}

#[test]
fn the_extruded_fixture_measures_as_by_hand() {
    // extruded_bspline.brep (the fixtures' README): the cubic closed by its chord and
    // extruded 2 along +z. Before patch 0004, 3.7964232643296967 (9.4e-4 low) and its
    // extruded face 8.83202504977072 (0.27% over), estimating 3.5e-16.
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brep/extruded_bspline.brep");
    let shape = from_freecad(&std::fs::read_to_string(path).unwrap());
    let mut k = kernel();
    let s = measured(&mut k, &shape);
    assert_eq!((s.faces, s.edges, s.vertices), (4, 6, 4));
    let cubic = fixtures_cubic();
    let v = [0.0, 0.0, 2.0];
    let (volume, area) = cubic.prism_by_hand(v);
    assert_close("3.8 by hand", volume, 3.8, 1e-14);
    let swept = face_area(&mut k, &shape, "extrusion");
    assert_cases(
        &[("the volume".to_owned(), s.volume, 3.8)],
        &[
            ("the area".to_owned(), s.area, area),
            (
                "the extruded face's area".to_owned(),
                swept,
                8.808_111_747_104_897,
            ),
        ],
        TIGHT,
        TIGHT,
    );
    // The centre is the cap's centroid by hand, lifted half the height.
    for (axis, want) in [2.0, cubic.centroid_y(), 1.0].into_iter().enumerate() {
        assert_close("the centre", s.center[axis], want, 1e-12);
    }
    // FreeCAD's measure, OCCT's default integration, which the patch doesn't touch: the
    // same bits as before it, 4.4e-5 low (Known wrong results).
    let freecad = k.measure_as_freecad(&shape).unwrap();
    let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(
        bits(&[freecad.volume, freecad.area]),
        bits(&[3.799_834_574_236_94, 20.608_071_865_351_118]),
        "FreeCAD's volume and area: {freecad:?}"
    );
}

#[test]
fn an_extruded_multi_span_bspline_measures_as_by_hand_up_down_or_slanted() {
    // Before patch 0004, the swept face's area 3.9% over (along +z).
    let spline = uneven_cubic();
    let mut k = kernel();
    let cap = spline.cap();
    let (mut volumes, mut areas, mut slanted) = (Vec::new(), Vec::new(), Vec::new());
    for v in [[0.0, 0.0, 2.0], [0.0, 0.0, -2.0], [0.5, -0.3, 1.5]] {
        let shape = swept(&mut k, &cap, v);
        let (vs, mut a) = prism_cases(&mut k, &format!("along {v:?}"), &spline, &shape, v);
        volumes.extend(vs);
        if v[0] != 0.0 {
            // The extruded face's area, the second.
            slanted.push(a.remove(1));
        }
        areas.extend(a);
    }
    assert_eq!(slanted.len(), 1);
    assert_cases(&volumes, &areas, UNEVEN_VOLUME, UNEVEN_AREA);
    assert_cases(&[], &slanted, UNEVEN_VOLUME, SLANTED_AREA);
}

/// Python: `spline`'s cap extruded along `v` by FreeCAD, its extruded face's surface put
/// under a `Geom_RectangularTrimmedSurface` (`trim_surface`) and its basis curve under a
/// `Geom_TrimmedCurve` (`trim_curve`), each to the face's bounds, by rewriting the
/// surface's record in the text B-rep (types 10, 6, and 8, GeomTools_SurfaceSet and
/// GeomTools_CurveSet); read back, checked valid and wrapped, and printed.
fn trimmed_prism_script(
    spline: &Spline,
    v: [f64; 3],
    trim_curve: bool,
    trim_surface: bool,
) -> String {
    let [x, y, z] = v;
    format!(
        r#"import json, Part, FreeCAD as App
V = App.Vector
{cap}
solid = face.extrude(V({x:?}, {y:?}, {z:?}))
side = [f for f in solid.Faces if f.Surface.TypeId == "Part::GeomSurfaceOfExtrusion"]
assert len(side) == 1, [f.Surface.TypeId for f in solid.Faces]
u1, u2, v1, v2 = side[0].ParameterRange
def surfaces(lines):
    start = next(i for i, l in enumerate(lines) if l.startswith("Surfaces "))
    end = next(i for i in range(start, len(lines)) if lines[i].startswith("Triangulations"))
    return start, end
lines = solid.exportBrepToString().split("\n")
start, end = surfaces(lines)
at = [i for i in range(start + 1, end) if lines[i].startswith("6 ")]
assert len(at) == 1, at
i = at[0]
wrap = ["10 %r %r %r %r" % (u1, u2, v1, v2)] if {trim} else []
lines = lines[:i] + wrap + [lines[i]] + ["8 %r %r" % (u1, u2)] * {curves} + lines[i + 1:]
shape = Part.Shape()
shape.importBrepFromString("\n".join(lines))
assert shape.isValid()
again = shape.exportBrepToString().split("\n")
start, end = surfaces(again)
records = [l.split(" ")[0] for l in again[start + 1:end] if not l.startswith(" ")]
assert records.count("10") == (1 if {trim} else 0), records
assert records.count("8") == {curves}, records
print("SILICO_ORACLE " + json.dumps(shape.exportBrepToString()))
"#,
        cap = spline.cap_script(),
        trim = if trim_surface { "True" } else { "False" },
        curves = u8::from(trim_curve),
    )
}

#[test]
fn an_extrusion_under_trims_measures_as_by_hand() {
    // Its surface under a trim to the face's bounds, its curve under a trim, or both: the
    // integration's U spans (one, for an extrusion) and its result don't change. (Two
    // trims of the curve are one: Geom_TrimmedCurve takes a trimmed basis's own basis.)
    let spline = uneven_cubic();
    let v = [0.0, 0.0, 2.0];
    let mut k = kernel();
    let (mut volumes, mut areas) = (Vec::new(), Vec::new());
    for (trim_curve, trim_surface) in [(true, false), (false, true), (true, true)] {
        let script = trimmed_prism_script(&spline, v, trim_curve, trim_surface);
        let shape = from_freecad(oracle::freecad(&script).as_str().unwrap());
        let what = format!("the curve trimmed: {trim_curve}, the surface: {trim_surface}");
        let (vs, a) = prism_cases(&mut k, &what, &spline, &shape, v);
        volumes.extend(vs);
        areas.extend(a);
    }
    assert_cases(&volumes, &areas, UNEVEN_VOLUME, UNEVEN_AREA);
}

#[test]
fn an_extrusion_of_a_bspline_kinked_at_a_full_knot_measures_as_by_hand() {
    // A knot of multiplicity 3 in a cubic: C0 there, a corner at the pole (3, 1).
    let spline = Spline {
        poles: vec![
            [0.0, 0.0],
            [1.0, 2.0],
            [2.0, 3.0],
            [3.0, 1.0],
            [4.0, 3.0],
            [5.0, 2.0],
            [6.0, 0.0],
        ],
        knots: vec![0.0, 1.0, 2.0],
        multiplicities: vec![4, 3, 4],
        degree: 3,
        periodic: false,
    };
    let v = [0.0, 0.0, 2.0];
    let mut k = kernel();
    let cap = spline.cap();
    let shape = swept(&mut k, &cap, v);
    let (volumes, areas) = prism_cases(&mut k, "kinked", &spline, &shape, v);
    assert_cases(&volumes, &areas, TIGHT, TIGHT);
}

#[test]
fn a_tube_of_a_periodic_bspline_measures_as_by_hand() {
    // A closed (periodic) cubic over a hexagon's corners: its extrusion is closed in U, its
    // seam edge on it twice.
    let h = 1.5 * 3f64.sqrt();
    let spline = Spline {
        poles: vec![
            [3.0, 0.0],
            [1.5, h],
            [-1.5, h],
            [-3.0, 0.0],
            [-1.5, -h],
            [1.5, -h],
        ],
        knots: (0..7).map(f64::from).collect(),
        multiplicities: vec![1; 7],
        degree: 3,
        periodic: true,
    };
    let v = [0.0, 0.0, 2.0];
    let mut k = kernel();
    let cap = spline.cap();
    let shape = swept(&mut k, &cap, v);
    assert_eq!(measured(&mut k, &shape).faces, 3);
    let (volumes, areas) = prism_cases(&mut k, "the tube", &spline, &shape, v);
    assert_cases(&volumes, &areas, TIGHT, UNEVEN_AREA);
}

#[test]
fn an_extrusion_of_a_single_span_bspline_measures_as_by_hand() {
    // One span, as the stress test's prisms are: its volume was right before patch 0004,
    // its extruded face's area 7.4e-5 over.
    let spline = single_span_cubic();
    let v = [0.0, 0.0, 2.0];
    let mut k = kernel();
    let cap = spline.cap();
    let shape = swept(&mut k, &cap, v);
    let (volumes, areas) = prism_cases(&mut k, "one span", &spline, &shape, v);
    assert_cases(&volumes, &areas, TIGHT, TIGHT);
}

#[test]
fn a_bspline_surface_ruled_between_a_cubic_and_its_copy_has_the_area_by_hand() {
    // The extrusion of each cubic as the B-spline surface it equals (FreeCAD's
    // makeRuledSurface between the curve and its copy 2 along z, a GeomAbs_BSplineSurface):
    // 2 × the curve's length. Before patch 0004, the single span 5.9e-6 over and the four
    // spans 1.3e-5 short, each estimating 1e-16 (Known wrong results).
    let mut k = kernel();
    let mut areas = Vec::new();
    for (what, spline) in [
        ("one span", single_span_cubic()),
        ("four spans", uneven_cubic()),
    ] {
        let face = freecad_face(&format!(
            "{}edge = curve.toShape()\nface = Part.makeRuledSurface(edge, edge.translated(V(0, 0, 2)))\n\
             assert face.Surface.TypeId == \"Part::GeomBSplineSurface\", face.Surface.TypeId\n",
            spline.script()
        ));
        let d = k.describe(&face).unwrap();
        assert_eq!(d.faces.len(), 1);
        assert_eq!(kind(&d.faces[0].surface), "bspline");
        areas.push((
            format!("{what}: the area"),
            d.faces[0].area,
            spline.swept_area([0.0, 0.0, 2.0]),
        ));
    }
    assert_cases(&[], &areas, TIGHT, TIGHT);
}

#[test]
fn a_half_turn_whose_cut_face_holds_the_reference_point_measures_as_by_hand() {
    // The rectangle 0 ≤ x ≤ 3, 0 ≤ z ≤ 5 turned half a turn about z: every vertex is on y
    // = 0, so OCCT's reference point (the mean of the vertices' occurrences) is on the cut
    // face, the plane y = 0, where every term of the volume element is 0. There the patch's
    // U tolerance (0.9 × 1e-9 × the strip's magnitude, the sum of those terms' absolute
    // values) is 0, and U's loop ends after its first pass by its own condition. What this
    // covers: a revolve with such a face measures by hand, the cut face adding nothing and
    // the others refined as ever. Not that U stopped there: a plane is integrated exactly
    // either way. The cost of not stopping is pinned on B-spline faces through the vertex
    // mean (adversarial_measure_refine_u.rs, its tests in time). By hand, a half cylinder
    // of radius 3 and height 5: volume 22.5π; area two half discs (9π), the curved side
    // (15π), and the cut face (6 × 5).
    use silico_kernel::{Axis, Curve, FaceRegions, Region, SketchPlane};
    let line = |start, end| Curve::Line { start, end };
    let regions = FaceRegions {
        plane: SketchPlane::Xz,
        regions: vec![Region {
            outer: vec![
                line([0.0, 0.0], [3.0, 0.0]),
                line([3.0, 0.0], [3.0, 5.0]),
                line([3.0, 5.0], [0.0, 5.0]),
                line([0.0, 5.0], [0.0, 0.0]),
            ],
            holes: vec![],
        }],
    };
    let axis = Axis {
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
    };
    let mut k = kernel();
    let faces = k.make_faces(&regions).unwrap();
    let shape = k
        .revolution(&faces.shape, axis, 0.0, std::f64::consts::PI)
        .unwrap()
        .shape;
    let s = measured(&mut k, &shape);
    let pi = std::f64::consts::PI;
    assert_cases(
        &[("the volume".to_owned(), s.volume, 22.5 * pi)],
        &[("the area".to_owned(), s.area, 24.0 * pi + 30.0)],
        TIGHT,
        TIGHT,
    );
}

#[test]
fn the_extruded_fixture_turned_so_its_volume_terms_mix_signs_measures_as_upright() {
    // The fixture's prism turned 37° about the axis (1, 2, 3) through (0.7, −0.4, 0.2), and
    // 45° about z: its faces' normals and their places about the vertex mean are no longer
    // along the axes, so the volume element's terms x·xn, y·yn, z·zn mix signs within a
    // strip, and the strip's magnitude (the sum of their absolute values, U's scale) is
    // larger than its value, so U's tolerance is looser than the asked precision of the
    // strip. What this covers: on this shape the looser tolerance still gives the volume
    // to 1e-12 (3.8 as upright) and the centre the upright one's turned. The areas, by
    // hand too, don't reach that rule (an area's magnitude is the strip itself); and it's
    // one shape, not a bound: docs/ports/measure.md gives the bound. Before patch 0004,
    // 9.4e-4 low, as upright.
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brep/extruded_bspline.brep");
    let text = std::fs::read_to_string(path).unwrap();
    let cubic = fixtures_cubic();
    let v = [0.0, 0.0, 2.0];
    let (_, area) = cubic.prism_by_hand(v);
    let upright = [2.0, cubic.centroid_y(), 1.0];
    let mut k = kernel();
    let mut volumes = Vec::new();
    let mut areas = Vec::new();
    let mut centres = Vec::new();
    let turns: [(&str, [f64; 3], f64, [f64; 3]); 2] = [
        (
            "37° about (1, 2, 3)",
            [1.0, 2.0, 3.0],
            37.0,
            [0.7, -0.4, 0.2],
        ),
        ("45° about z", [0.0, 0.0, 1.0], 45.0, [0.0, 0.0, 0.0]),
    ];
    for (what, axis, degrees, through) in turns {
        let [ax, ay, az] = axis;
        let [cx, cy, cz] = through;
        let script = format!(
            "import json, Part, FreeCAD as App\n\
             V = App.Vector\n\
             shape = Part.Shape()\n\
             shape.importBrepFromString({text:?})\n\
             shape.rotate(V({cx:?}, {cy:?}, {cz:?}), V({ax:?}, {ay:?}, {az:?}), {degrees:?})\n\
             assert shape.isValid()\n\
             print(\"SILICO_ORACLE \" + json.dumps(shape.exportBrepToString()))\n"
        );
        let shape = from_freecad(oracle::freecad(&script).as_str().unwrap());
        let s = measured(&mut k, &shape);
        volumes.push((format!("{what}: the volume"), s.volume, 3.8));
        areas.push((format!("{what}: the area"), s.area, area));
        areas.push((
            format!("{what}: the extruded face's area"),
            face_area(&mut k, &shape, "extrusion"),
            8.808_111_747_104_897,
        ));
        // Rodrigues: the upright centre turned about the axis through the point.
        let p = [0, 1, 2].map(|i| upright[i] - through[i]);
        let want = apply(&rotation(axis, degrees), p, through);
        for (i, (got, want)) in s.center.iter().zip(want).enumerate() {
            centres.push((format!("{what}: the centre's {i}"), *got, want));
        }
    }
    assert_cases(&volumes, &areas, TIGHT, TIGHT);
    let off: Vec<_> = centres
        .iter()
        .filter(|(_, got, want)| (got - want).abs() > 1e-12)
        .collect();
    assert!(off.is_empty(), "{off:?}");
}
