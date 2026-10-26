//! Values by hand for measures of revolutions, independent of OCCT: Gauss–Legendre on
//! [a, b], cubic Bézier spans, a cap between a curve and its chord integrated along the
//! curve, a face's kind of surface as a word, and the cases that miss their values.

use super::numbers::relative_error;

use silico_protocol::Surface;

/// Gauss–Legendre's eight nodes on [-1, 1] and their weights: exact for polynomials up to
/// degree 15.
pub const NODES: [f64; 8] = [
    -0.960_289_856_497_536_2,
    -0.796_666_477_413_626_7,
    -0.525_532_409_916_329,
    -0.183_434_642_495_649_8,
    0.183_434_642_495_649_8,
    0.525_532_409_916_329,
    0.796_666_477_413_626_7,
    0.960_289_856_497_536_2,
];
pub const WEIGHTS: [f64; 8] = [
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
pub fn integral(a: f64, b: f64, pieces: u32, f: impl Fn(f64) -> f64) -> f64 {
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

/// A cubic Bézier span's point and derivative at `t`.
pub fn bezier(p: &[[f64; 2]; 4], t: f64) -> ([f64; 2], [f64; 2]) {
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

/// ∫∫ f dA over the cap between a curve and its chord, the curve running once from left to
/// right above the chord: Green's theorem, ∮ F dx with F(x, y) = −∫ f dy from the chord to
/// y, which is 0 along the chord, so only the curve adds, as ∫ `inner`(point) dx along it.
/// `inner` is ∫ f(x, y') dy' from the chord's y to the point's (the caller's, by hand);
/// the curve is `at(span, t)`, its point and derivative, over each span's parameters in
/// `spans`, eight points on each: exact where `inner` along a span is a polynomial of
/// degree 15 or less.
pub fn cap_integral(
    spans: &[(f64, f64)],
    at: impl Fn(usize, f64) -> ([f64; 2], [f64; 2]),
    inner: impl Fn([f64; 2]) -> f64,
) -> f64 {
    spans
        .iter()
        .enumerate()
        .map(|(span, &(lo, hi))| {
            integral(lo, hi, 1, |t| {
                let (point, tangent) = at(span, t);
                inner(point) * tangent[0]
            })
        })
        .sum()
}

/// A face's kind of surface, as a word.
pub fn kind(surface: &Surface) -> &'static str {
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

/// Each `(what, got, want)` further than `tolerance` (relative) from `want`, or not a
/// number, said with its error, so a failure lists every case that misses, not only the
/// first.
pub fn misses(cases: &[(String, f64, f64)], tolerance: f64) -> Vec<String> {
    cases
        .iter()
        .filter(|(_, got, want)| {
            let error = relative_error(*got, *want);
            error.is_nan() || error > tolerance
        })
        .map(|(what, got, want)| {
            format!(
                "{what}: got {got}, want {want} (relative error {:e})",
                relative_error(*got, *want)
            )
        })
        .collect()
}

/// Each `(what, got, want)` further than `tolerance` from `want` (absolute: for values
/// that are 0 by hand, as a centre on an axis is), or not a number, said with its error.
pub fn misses_by(cases: &[(String, f64, f64)], tolerance: f64) -> Vec<String> {
    cases
        .iter()
        .filter(|(_, got, want)| {
            let error = (got - want).abs();
            error.is_nan() || error > tolerance
        })
        .map(|(what, got, want)| {
            format!(
                "{what}: got {got}, want {want} (off by {:e})",
                (got - want).abs()
            )
        })
        .collect()
}

/// The cases that aren't OCCT's known wrong answer for them, `known`: `got` within
/// `pinned(got, wrong)` of the `wrong` pinned under the case's name.
fn not_known_wrong(
    cases: &[(String, f64, f64)],
    known: &[(&str, f64)],
    pinned: fn(f64, f64) -> bool,
) -> Vec<(String, f64, f64)> {
    cases
        .iter()
        .filter(|(what, got, _)| {
            !known
                .iter()
                .any(|(name, wrong)| *name == what && pinned(*got, *wrong))
        })
        .cloned()
        .collect()
}

/// Each case right to `tolerance` (relative), or OCCT's known wrong answer for it, `known`,
/// to a relative 1e-12 (docs/ports/measure.md's *Known wrong results*: either is taken, so
/// a fixed OCCT passes, and any other value fails); said, each case that's neither.
pub fn right_or_known_wrong(
    cases: &[(String, f64, f64)],
    tolerance: f64,
    known: &[(&str, f64)],
) -> Vec<String> {
    let pinned = |got: f64, wrong: f64| relative_error(got, wrong) <= 1e-12;
    misses(&not_known_wrong(cases, known, pinned), tolerance)
}

/// [`right_or_known_wrong`] for values right to an absolute `tolerance` (as [`misses_by`]),
/// such as a centre's coordinates: OCCT's known wrong answer is taken to a relative 1e-12,
/// or an absolute 1e-12 where it's near 0.
pub fn right_or_known_wrong_by(
    cases: &[(String, f64, f64)],
    tolerance: f64,
    known: &[(&str, f64)],
) -> Vec<String> {
    let pinned = |got: f64, wrong: f64| (got - wrong).abs() <= 1e-12 * wrong.abs().max(1.0);
    misses_by(&not_known_wrong(cases, known, pinned), tolerance)
}
