//! `/break-it` on the arc edge's range (`make_arc_edge` in cpp/bridge.cpp): every sketch
//! arc's range moved by whole turns as `Geom_TrimmedCurve::SetTrim` moves it
//! (`ElCLib::AdjustPeriodic`, its precision `min(sweep/2, 1e-9)`), so a revolve's surface of
//! revolution, built on such a trimmed curve of the edge's curve, has the face's own range.
//! Hostile ranges: arcs under 2e-9 rad across angle 0, starts at ±0, at and a hair under
//! 2π, a turn on or back, at ±π, arcs of more than π across 0, arcs in holes and walked
//! clockwise, revolves about the sketch's other axis, part turns and turns from elsewhere,
//! cuts, commons, and fuses, tight boxes, and random barrels.
//!
//! Every expected value is by hand, independent of OCCT: a solid of revolution's section at
//! each height is a set of annuli, and what a box beyond the plane y = Y0 leaves of an
//! annulus's turn is K(outer) − K(inner) for a closed-form K of the radius (`kept`), so its
//! volume is ∫∫ K′(x) dA over the profile, which Green's theorem makes ∮ K(x) dz along the
//! profile's boundary, counter-clockwise: each line and arc integrated along itself by
//! Gauss–Legendre, the integrand smooth on each.

use super::kernel::kernel;
use super::logs::warnings_of;
use super::quadrature::integral;

use std::cell::RefCell;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use proptest::prelude::*;
use silico_core::kernel::KernelWorker;
use silico_kernel::{Axis, BooleanOp, Curve, FaceRegions, Region, SketchPlane};
use silico_protocol::{Primitive, ShapeData, THROUGH_ALL_FACTOR};

/// The bore of every ring here.
const INNER: f64 = 1.0;
/// The boxes here keep away y > Y0 (or y < −Y0): nearer the axis than any of the solids'
/// radii, so each cap below is a smooth function of its radius.
const Y0: f64 = 0.5;
/// Half the boxes' width and depth: wider than every solid here.
const HALF: f64 = 100.0;

fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
    Curve::Line { start, end }
}

fn at([cx, cy]: [f64; 2], radius: f64, angle: f64) -> [f64; 2] {
    [
        cx + radius * libm::cos(angle),
        cy + radius * libm::sin(angle),
    ]
}

/// The arc of the circle of `radius` about `center` from angle `from` to `to`, as sent
/// (counter-clockwise where `to` is the larger), its points where the angles put them.
fn arc(center: [f64; 2], radius: f64, from: f64, to: f64) -> Curve {
    Curve::Arc {
        center,
        start: at(center, radius, from),
        end: at(center, radius, to),
        radius,
        start_angle: from,
        end_angle: to,
    }
}

fn ends(curve: &Curve) -> ([f64; 2], [f64; 2]) {
    match *curve {
        Curve::Line { start, end } | Curve::Arc { start, end, .. } => (start, end),
        Curve::Circle { .. } => unreachable!("no circles here"),
    }
}

/// A loop of `curves` closed by lines: each curve joined to the next by a line where its
/// end isn't the next one's start.
fn joined(curves: Vec<Curve>) -> Vec<Curve> {
    let mut out = Vec::new();
    for i in 0..curves.len() {
        let (_, end) = ends(&curves[i]);
        let (next, _) = ends(&curves[(i + 1) % curves.len()]);
        out.push(curves[i]);
        if end != next {
            out.push(line(end, next));
        }
    }
    out
}

fn xz(outer: Vec<Curve>, holes: Vec<Vec<Curve>>) -> FaceRegions {
    FaceRegions {
        plane: SketchPlane::Xz,
        regions: vec![Region { outer, holes }],
    }
}

const Z_AXIS: Axis = Axis {
    origin: [0.0; 3],
    direction: [0.0, 0.0, 1.0],
};
const X_AXIS: Axis = Axis {
    origin: [0.0; 3],
    direction: [1.0, 0.0, 0.0],
};

fn revolved(
    k: &mut KernelWorker,
    regions: &FaceRegions,
    axis: Axis,
    start: f64,
    turn: f64,
) -> ShapeData {
    let faces = k.make_faces(regions).unwrap();
    k.revolution(&faces.shape, axis, start, turn).unwrap().shape
}

/// The box from (−`half`, `low`, −`half`) to (`half`, `high`, `half`).
fn slab_of(k: &mut KernelWorker, half: f64, low: f64, high: f64) -> ShapeData {
    let block = k
        .make(Primitive::Box {
            dx: 2.0 * half,
            dy: high - low,
            dz: 2.0 * half,
        })
        .unwrap();
    k.translate(&block, [-half, low, -half]).unwrap().shape
}

/// The box beyond y = Y0: y > Y0 for a turn about z, which turns +x towards +y; y < −Y0
/// about x, which turns +z towards −y. Either way at angles in (0, π) from the profile.
fn beyond(k: &mut KernelWorker, axis: Axis) -> ShapeData {
    beyond_of(k, axis, HALF)
}

/// [`beyond`]'s box, `half` wide and deep each way and reaching `half` from the axis.
fn beyond_of(k: &mut KernelWorker, axis: Axis, half: f64) -> ShapeData {
    if axis == X_AXIS {
        slab_of(k, half, -half, -Y0)
    } else {
        slab_of(k, half, Y0, half)
    }
}

fn beyond_volume_of(half: f64) -> f64 {
    4.0 * half * half * (half - Y0)
}

/// The area of the disc of radius `rho` beyond a chord Y0 from its centre.
fn cap(rho: f64) -> f64 {
    if rho <= Y0 {
        return 0.0;
    }
    rho * rho * libm::acos(Y0 / rho) - Y0 * libm::sqrt(rho * rho - Y0 * Y0)
}

/// Of the sector of radius `rho` turning `turn` from the profile's side, what the box
/// leaves: the cap is at angles in (0, π), so inside every turn of π or more, and a quarter
/// turn holds half of it (the cap's half on the profile's side of the perpendicular).
fn kept(rho: f64, turn: f64) -> f64 {
    if turn >= PI {
        turn / 2.0 * rho * rho - cap(rho)
    } else {
        assert_eq!(turn, FRAC_PI_2, "only quarter turns below π");
        turn / 2.0 * rho * rho - cap(rho) / 2.0
    }
}

/// What the box takes of the same sector.
fn taken(rho: f64, turn: f64) -> f64 {
    turn / 2.0 * rho * rho - kept(rho, turn)
}

/// ∮ K(radius) d(along) over a region's loops, as each walks: ∫∫ K′(radius) dA over the
/// region, by Green's theorem, where `radius` is a point's distance from the axis and
/// `along` its place along it. About z (`about_x` false) that's the sketch's (x, y) = (r, z),
/// ∮ K(x) dy; about x it's (along, r) = (x, y), and −∮ K(y) dx = ∫∫ K′(y) dA.
fn by_hand(regions: &FaceRegions, about_x: bool, big_k: impl Fn(f64) -> f64) -> f64 {
    let mut sum = 0.0;
    for region in &regions.regions {
        for curves in region.loops() {
            for curve in curves {
                // The point and its derivative along the curve's parameter.
                let term = |p: [f64; 2], d: [f64; 2]| {
                    if about_x {
                        -big_k(p[1]) * d[0]
                    } else {
                        big_k(p[0]) * d[1]
                    }
                };
                sum += match *curve {
                    Curve::Line { start, end } => {
                        let d = [end[0] - start[0], end[1] - start[1]];
                        integral(0.0, 1.0, 16, |t| {
                            term([start[0] + t * d[0], start[1] + t * d[1]], d)
                        })
                    }
                    Curve::Arc {
                        center,
                        radius,
                        start_angle,
                        end_angle,
                        ..
                    } => integral(start_angle, end_angle, 64, |t| {
                        let d = [-radius * libm::sin(t), radius * libm::cos(t)];
                        term(at(center, radius, t), d)
                    }),
                    Curve::Circle { .. } => unreachable!("no circles here"),
                };
            }
        }
    }
    sum
}

/// Whether `got` is within `tolerance` of `want`, relative.
fn near(got: f64, want: f64, tolerance: f64) -> bool {
    (got - want).abs() <= tolerance * want.abs()
}

/// How near a whole revolve's volume is to its hand value: patch 0003's precise
/// integration, to round-off on a whole turn and 2e-9 on a part turn (revolution_measures.rs).
const SOLID: f64 = 2e-9;

/// A revolved `regions` (about `axis`, from `start`, turning `turn`) and what each boolean
/// with the box beyond Y0 leaves, against the hand values: each miss, said. The solid to
/// [`SOLID`], each boolean to `tolerance`: a boolean that skipped a face is off by 1e-3 or
/// more, and OCCT measures a cut surface of revolution up to 1.1e-7 off here (the about-x
/// common: 1.04e-7 high, its cut 4e-9 high, so their sum isn't the solid's; the same with
/// the arc split at angle 0, or at −0.1, so not the arc's range).
fn misses(
    k: &mut KernelWorker,
    what: &str,
    regions: &FaceRegions,
    axis: Axis,
    start: f64,
    turn: f64,
    tolerance: f64,
) -> Vec<String> {
    let relative = |_op: BooleanOp, want: f64, _area: f64| tolerance * want.abs();
    let against = Against {
        half: HALF,
        allowed: &relative,
    };
    misses_against(k, what, regions, axis, start, turn, against)
}

/// The box a revolve's booleans are made with, and how near each result is held.
struct Against<'a> {
    /// [`beyond_of`]'s box, this wide each way.
    half: f64,
    /// How far each boolean's volume may be from its hand value, from the boolean, that
    /// value, and the result's area.
    allowed: &'a dyn Fn(BooleanOp, f64, f64) -> f64,
}

/// [`misses`] against `against`'s box, each boolean held as it says.
fn misses_against(
    k: &mut KernelWorker,
    what: &str,
    regions: &FaceRegions,
    axis: Axis,
    start: f64,
    turn: f64,
    against: Against,
) -> Vec<String> {
    let Against { half, allowed } = against;
    let about_x = axis == X_AXIS;
    let whole = by_hand(regions, about_x, |r| turn / 2.0 * r * r);
    let cut = by_hand(regions, about_x, |r| kept(r, turn));
    let common = by_hand(regions, about_x, |r| taken(r, turn));
    let solid = revolved(k, regions, axis, start, turn);
    let block = beyond_of(k, axis, half);
    let mut results = vec![("the solid".to_string(), solid.clone(), whole, None)];
    for (op, want) in [
        (BooleanOp::Cut, cut),
        (BooleanOp::Common, common),
        (BooleanOp::Fuse, beyond_volume_of(half) + cut),
    ] {
        let result = k.boolean(op, &solid, &block).unwrap().shape;
        results.push((format!("{op:?}"), result, want, Some(op)));
    }
    let mut wrong = Vec::new();
    for (name, shape, want, op) in results {
        let s = k.summarize(&shape).unwrap();
        let within = match op {
            None => SOLID * want.abs(),
            Some(op) => allowed(op, want, s.area),
        };
        if !(s.valid && (s.volume - want).abs() <= within) {
            wrong.push(format!(
                "{what}, {name}: valid {}, volume {} for {want} within {within:e} (relative \
                 error {:e})",
                s.valid,
                s.volume,
                ((s.volume - want) / want).abs()
            ));
        }
    }
    wrong
}

// ---- The hand rule itself ----

/// The barrel of revolution_measures.rs: x = 1 to the arc of the circle about (−3.5, 0) of
/// radius 8.5 through (4, −4), (5, 0), (4, 4), its angles ±atan2(4, 7.5).
fn barrel() -> FaceRegions {
    let a = libm::atan2(4.0, 7.5);
    xz(
        joined(vec![
            arc([-3.5, 0.0], 8.5, -a, a),
            line([INNER, 4.0], [INNER, -4.0]),
        ]),
        vec![],
    )
}

#[test]
fn the_hand_rule_gives_the_barrels_closed_form() {
    // The barrel's volume, π (668 − 128/3 + 2c (30 + R² asin(4/R))) with c = −3.5 and
    // R = 8.5: 526.3343139891515 (revolution_measures.rs). And the rule's cut on a plain
    // tube, the annulus 1 ≤ r ≤ 5, 8 high: 8 (K(5) − K(1)).
    let whole = by_hand(&barrel(), false, |r| PI * r * r);
    assert!(near(whole, 526.334_313_989_151_5, 1e-14), "{whole}");
    let tube = xz(
        joined(vec![
            line([INNER, -4.0], [5.0, -4.0]),
            line([5.0, 4.0], [INNER, 4.0]),
        ]),
        vec![],
    );
    let cut = by_hand(&tube, false, |r| kept(r, TAU));
    assert!(near(cut, 8.0 * (kept(5.0, TAU) - kept(INNER, TAU)), 1e-14));
    // A cap by hand at ρ = 1: the segment beyond the chord at 1/2, π/3 − √3/4.
    assert!(near(cap(1.0), PI / 3.0 - libm::sqrt(3.0) / 4.0, 1e-15));
}

// ---- Arcs under 2e-9 rad across angle 0 ----

/// A tube, bore 1, wall x = 5, z from −4 to 4, its wall's middle `band`: an arc within
/// 1e-16 of x = 5 between two cylinders.
fn banded_tube(band: Curve) -> FaceRegions {
    let (low, high) = ends(&band);
    xz(
        vec![
            line([INNER, -4.0], [5.0, -4.0]),
            line([5.0, -4.0], low),
            band,
            line(high, [5.0, 4.0]),
            line([5.0, 4.0], [INNER, 4.0]),
            line([INNER, 4.0], [INNER, -4.0]),
        ],
        vec![],
    )
}

#[test]
fn an_arc_under_2e_9_rad_a_hair_off_symmetric_about_angle_0_keeps_its_surfaces_range() {
    // BREAKS (fixed): the edge's range and its revolved surface's domain were a turn apart.
    // The arc from −5.549279854515954e-10 to 5.549273573009629e-10 (1.1e-9 rad; radius
    // 150, a band 1.7e-7 high) starts within half its sweep (SetTrim's precision, under
    // 1e-9) of angle 0: make_arc_edge's AdjustPeriodic moves its start up a turn, to
    // 5.549276593797003e-10 below 2π, under the precision (5.549276713762792e-10), so down
    // a turn again, rounded: the edge's range is [−5.549276593797003e-10, ...], its sweep
    // 3.3e-16 shorter. BRepSweep_Rotation's Geom_TrimmedCurve runs SetTrim on that range,
    // its precision from that sweep, 5.549275083403316e-10, and the start's distance below
    // 2π is no longer under it: the trimmed curve's domain, the surface's, is
    // [2π − 5.5e-10, ...], a turn from the face's pcurves, and the cut skips the band
    // (BOPAlgo_AlertIntersectionOfPairOfShapesFailed), the defect the fix closed. The
    // volume is right only because the band is 1.7e-7 high; were that alert refused,
    // these would be refused. The same arcs symmetric get no
    // alert. Fixed by running AdjustPeriodic until it changes nothing, a fixed point of
    // SetTrim: in a Python model of the two calls, each of the 4,202 ranges among 2e6
    // random ones that SetTrim moves again (every one with a sweep under 2e-9) settles on
    // the second run. Only the boolean's log shows the alert, so this runs in process.
    let block = silico_kernel::translate(
        &silico_kernel::make_box(2.0 * HALF, HALF - Y0, 2.0 * HALF).unwrap(),
        [-HALF, Y0, -HALF],
    )
    .unwrap()
    .shape;
    let want = 8.0 * (kept(5.0, TAU) - kept(INNER, TAU));
    let mut wrong = Vec::new();
    for (from, to) in [
        (-5.549_279_854_515_954e-10, 5.549_273_573_009_629e-10),
        (-8.958_384_352_254_341e-10, 8.958_377_660_109_311e-10),
        // Controls: symmetric, which settle at once.
        (-5.549_276e-10, 5.549_276e-10),
        (-8.958_381e-10, 8.958_381e-10),
    ] {
        let regions = banded_tube(arc([5.0 - 150.0, 0.0], 150.0, from, to));
        let faces = silico_kernel::make_faces(&regions).unwrap().shape;
        let tube = silico_kernel::revolution(&faces, &Z_AXIS, 0.0, TAU)
            .unwrap()
            .shape;
        let (cut, text) = warnings_of(|| {
            silico_kernel::boolean(BooleanOp::Cut, &tube, &block)
                .unwrap()
                .shape
        });
        let s = silico_kernel::summarize(&cut).unwrap();
        if !(s.valid && near(s.volume, want, 1e-9))
            || text.contains("IntersectionOfPairOfShapesFailed")
        {
            wrong.push(format!(
                "[{from:e}, {to:e}]: valid {}, volume {} for {want}; {}",
                s.valid,
                s.volume,
                text.trim()
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_band_turning_a_micro_radian_is_cut_by_a_box_or_is_occts_known_wrong_answer() {
    // Found by /break-it, a known wrong result (docs/ports/booleans.md), not the range's:
    // it's the same from angle 0, where nothing is adjusted, and on a torus. A tube whose
    // wall has a band of arc turning 1e-6 rad (radius 150, 1.5e-4 high), of a circle
    // reaching the axis (a surface of revolution) or not (a torus), less the box beyond
    // y = 0.5, comes back whole, 8 · 24π = 603.186 for 333.873, "valid", 14 faces, with no
    // warning; FreeCAD 1.1.3's `Shape.cut` gives the same (2026-10-25). With FreeCAD 1.1.3's
    // fuzzy value the torus's band is cut right (FreeCAD 1.1.3's `cut([box], -1.0)`:
    // 333.8732359), and the others are too against a box 200 wide (a fuzzy value of 3e-4),
    // but not against one at the tube's own scale, as here ([`THROUGH_ALL_FACTOR`] times its
    // diagonal each way, as a through-all tool is: 1.0e-4). Turning 1e-5 rad it's right, and
    // a band of line of the same height (a cylinder) is right. Silico takes OCCT's answer,
    // as FreeCAD does: each cut is right to 1e-9, or, for the two bands of revolution, that
    // wrong one, so OCCT giving any other fails here, and a fixed OCCT passes. The uncut
    // tube is also what the range's defect gave, so this documents OCCT's result and doesn't
    // guard the range fix: the tests above and below do.
    let mut k = kernel();
    let diagonal = (10.0f64 * 10.0 + 10.0 * 10.0 + 8.0 * 8.0).sqrt();
    let half = THROUGH_ALL_FACTOR * diagonal;
    let block = beyond_of(&mut k, Z_AXIS, half);
    let want = 8.0 * (kept(5.0, TAU) - kept(INNER, TAU));
    let whole = 8.0 * PI * (5.0 * 5.0 - INNER * INNER);
    let r = 150.0;
    let mut wrong = Vec::new();
    for (what, band, may_be_known) in [
        (
            "of revolution, across 0",
            arc([5.0 - r, 0.0], r, -5e-7, 5e-7),
            true,
        ),
        (
            "of revolution, from 0",
            arc([5.0 - r, 0.0], r, 0.0, 1e-6),
            true,
        ),
        (
            "of a torus, about π",
            arc([5.0 + r, 0.0], r, PI + 5e-7, PI - 5e-7),
            false,
        ),
        // Turning 1e-5 rad: right.
        (
            "of revolution, turning 1e-5 rad",
            arc([5.0 - r, 0.0], r, -5e-6, 5e-6),
            false,
        ),
    ] {
        let tube = revolved(&mut k, &banded_tube(band), Z_AXIS, 0.0, TAU);
        let cut = k.boolean(BooleanOp::Cut, &tube, &block).unwrap().shape;
        let s = k.summarize(&cut).unwrap();
        let right = near(s.volume, want, 1e-9);
        let known = may_be_known && (near(s.volume, whole, 1e-9), s.faces) == (true, 14);
        if !(s.valid && (right || known)) {
            wrong.push(format!(
                "a band {what}: valid {}, {} faces, volume {} for {want} (or {whole} whole)",
                s.valid, s.faces, s.volume
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---- Starts at and around the period's ends ----

/// A barrel's upper part: the arc of the circle about (c, 0) of radius `r`, from the true
/// angle `from` up to the angle where it meets z = 4, sent as those angles plus `shift`;
/// closed by z = 4, the bore, and z at its start.
fn barrel_from(c: f64, r: f64, from: f64, shift: f64) -> FaceRegions {
    let top = libm::asin(4.0 / r);
    let mut side = arc([c, 0.0], r, from + shift, top + shift);
    // The ends where the true angles put them (the shifted ones round differently).
    if let Curve::Arc { start, end, .. } = &mut side {
        *start = at([c, 0.0], r, from);
        *end = at([c, 0.0], r, top);
    }
    let (low, high) = ends(&side);
    xz(
        joined(vec![side, line([INNER, high[1]], [INNER, low[1]])]),
        vec![],
    )
}

#[test]
fn starts_at_and_a_hair_from_the_periods_ends_are_cut_as_by_hand() {
    // The barrel's circle (about (−3.5, 0), radius 8.5) from each start up to z = 4, its
    // angles sent as they are, a turn on, and a turn back: the start at ±0, PConfusion and
    // half of it below 0 (2π − PConfusion and 2π − PConfusion/2 a turn on, AdjustPeriodic's
    // edge) and a hair either side, the double just under 2π less 2π, the least normal
    // number below 0, 1e-12 below, and a plain −0.3. Each revolved, cut, commoned, and
    // fused with the box beyond y = 0.5.
    let mut k = kernel();
    let mut wrong = Vec::new();
    let just_under_turn = f64::from_bits(TAU.to_bits() - 1) - TAU;
    for from in [
        0.0,
        -0.0,
        -1e-9,
        -5e-10,
        -1e-9 * (1.0 + 1e-7),
        -1e-9 * (1.0 - 1e-7),
        just_under_turn,
        -f64::MIN_POSITIVE,
        -1e-12,
        -0.3,
    ] {
        for shift in [0.0, TAU, -TAU] {
            let regions = barrel_from(-3.5, 8.5, from, shift);
            for turn in [TAU, 1.5 * PI] {
                wrong.extend(misses(
                    &mut k,
                    &format!("from {from:e} sent {shift:+} turning {turn}"),
                    &regions,
                    Z_AXIS,
                    0.0,
                    turn,
                    // The barrel about z, cut, to 2e-9 (revolution_measures.rs).
                    2e-9,
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn an_arc_of_more_than_half_a_turn_across_0_is_cut_as_by_hand() {
    // A D: the arc of the circle about (6, 0) of radius 10 from −2 to 2 rad (229°, across
    // 0, x from 1.84 to 16 and back), closed by its chord x = 6 + 10 cos 2. Its circle
    // reaches the axis. Sent as is, a turn back (from −2 − 2π, below −π), and a turn on.
    let mut k = kernel();
    let mut wrong = Vec::new();
    for shift in [0.0, -TAU, TAU] {
        let mut side = arc([6.0, 0.0], 10.0, -2.0 + shift, 2.0 + shift);
        if let Curve::Arc { start, end, .. } = &mut side {
            *start = at([6.0, 0.0], 10.0, -2.0);
            *end = at([6.0, 0.0], 10.0, 2.0);
        }
        let regions = xz(joined(vec![side]), vec![]);
        for turn in [TAU, 1.5 * PI, FRAC_PI_2] {
            wrong.extend(misses(
                &mut k,
                &format!("sent {shift:+}, turning {turn}"),
                &regions,
                Z_AXIS,
                0.0,
                turn,
                5e-7,
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_barrel_across_angle_pi_left_of_the_axis_is_its_mirror() {
    // The barrel mirrored to x < 0: its arc of the circle about (3.5, 0) from π − α to
    // π + α (the core's angles: atan2's start, its end a turn on), and the same from
    // −π − α to −π + α (below −π). Turned a whole turn about z it's the barrel's solid, and
    // the box beyond y = 0.5 takes the same from it: the barrel's own hand values.
    let mut k = kernel();
    let a = libm::atan2(4.0, 7.5);
    let mut wrong = Vec::new();
    for (from, to) in [(PI - a, PI + a), (-PI - a, -PI + a)] {
        let mut side = arc([3.5, 0.0], 8.5, from, to);
        if let Curve::Arc { start, end, .. } = &mut side {
            *start = [-4.0, 4.0];
            *end = [-4.0, -4.0];
        }
        let regions = xz(
            joined(vec![side, line([-INNER, -4.0], [-INNER, 4.0])]),
            vec![],
        );
        let solid = revolved(&mut k, &regions, Z_AXIS, 0.0, TAU);
        let block = beyond(&mut k, Z_AXIS);
        let cut = k.boolean(BooleanOp::Cut, &solid, &block).unwrap().shape;
        for (what, shape, want) in [
            ("whole", &solid, by_hand(&barrel(), false, |r| PI * r * r)),
            ("cut", &cut, by_hand(&barrel(), false, |r| kept(r, TAU))),
        ] {
            let s = k.summarize(shape).unwrap();
            if !(s.valid && near(s.volume, want, 2e-9)) {
                wrong.push(format!(
                    "[{from}, {to}] {what}: valid {}, volume {} for {want}",
                    s.valid, s.volume
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---- Holes, and arcs walked clockwise ----

#[test]
fn arcs_across_angle_0_in_holes_either_way_round_are_cut_as_by_hand() {
    // Two rings with a void bounded by the barrel's arc. In the first, the void is the lens
    // between x = 4 and the arc, and the hole, clockwise, walks the arc clockwise (sent end
    // first, its angles swapped: about the reversed normal); in the second, the void is
    // between the arc and x = 6, and the hole walks the arc counter-clockwise.
    let a = libm::atan2(4.0, 7.5);
    let exact = |mut side: Curve, from: [f64; 2], to: [f64; 2]| {
        if let Curve::Arc { start, end, .. } = &mut side {
            *start = from;
            *end = to;
        }
        side
    };
    let lens = vec![
        line([4.0, -4.0], [4.0, 4.0]),
        exact(arc([-3.5, 0.0], 8.5, a, -a), [4.0, 4.0], [4.0, -4.0]),
    ];
    let outside = joined(vec![
        line([6.0, -4.0], [4.0, -4.0]),
        exact(arc([-3.5, 0.0], 8.5, -a, a), [4.0, -4.0], [4.0, 4.0]),
        line([4.0, 4.0], [6.0, 4.0]),
    ]);
    let rectangle = |right: f64| {
        joined(vec![
            line([INNER, -5.0], [right, -5.0]),
            line([right, 5.0], [INNER, 5.0]),
        ])
    };
    let mut k = kernel();
    let mut wrong = Vec::new();
    for (what, regions) in [
        ("the lens", xz(rectangle(6.0), vec![lens])),
        ("outside the arc", xz(rectangle(7.0), vec![outside])),
    ] {
        for turn in [TAU, 1.5 * PI, FRAC_PI_2] {
            wrong.extend(misses(
                &mut k,
                &format!("{what}, turning {turn}"),
                &regions,
                Z_AXIS,
                0.0,
                turn,
                5e-7,
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---- About the sketch's other axis ----

/// A profile above the x axis whose right side is the arc of the circle about (0, 3) of
/// radius 4 from −0.5 to 0.5 rad, across its angle 0, closed by lines to x = 0. Turned
/// about x, the circle reaches the axis (3 < 4): a surface of revolution.
fn above_x() -> FaceRegions {
    let side = arc([0.0, 3.0], 4.0, -0.5, 0.5);
    let (low, high) = ends(&side);
    xz(
        joined(vec![side, line([0.0, high[1]], [0.0, low[1]])]),
        vec![],
    )
}

#[test]
fn an_arc_across_angle_0_turned_about_the_sketchs_x_axis_is_cut_as_by_hand() {
    // Turned about x from +z towards −y, so the box is beyond y = −0.5. Whole turns, part
    // turns, and a whole turn from −π/2 and from 1.
    let mut k = kernel();
    let regions = above_x();
    let mut wrong = Vec::new();
    for (start, turn) in [
        (0.0, TAU),
        (0.0, 1.5 * PI),
        (0.0, FRAC_PI_2),
        (-FRAC_PI_2, TAU),
        (1.0, TAU),
    ] {
        wrong.extend(misses(
            &mut k,
            &format!("from {start}, turning {turn}"),
            &regions,
            X_AXIS,
            start,
            turn,
            5e-7,
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn an_arc_across_angle_0_turned_about_x_has_its_tight_box() {
    // From (0, −h, −h) to (4, h, h), h = 3 + 4 sin 0.5 (the top line's height): the arc's
    // farthest x, 4, at angle 0; turned, the top corner's circle.
    let mut k = kernel();
    let solid = revolved(&mut k, &above_x(), X_AXIS, 0.0, TAU);
    let [low, high] = k.summarize(&solid).unwrap().bounds.unwrap();
    let h = 3.0 + 4.0 * libm::sin(0.5);
    for (got, want) in low.iter().chain(&high).zip([0.0, -h, -h, 4.0, h, h]) {
        assert!((got - want).abs() < 1e-6, "{low:?} {high:?}");
    }
}

// ---- Random barrels ----

/// A barrel: the arc of the circle about (c, zc) of radius r from the true angle `from` to
/// `to` (from ≤ 0 < to, each within 0.95 π/2, so the arc rises), sent with `shift` added,
/// and split at angle 0 into two arcs where `split`; closed by lines to the bore.
#[derive(Debug, Clone, Copy)]
struct Barrel {
    c: f64,
    zc: f64,
    r: f64,
    from: f64,
    to: f64,
    shift: f64,
    split: bool,
}

impl Barrel {
    fn regions(&self) -> FaceRegions {
        let centre = [self.c, self.zc];
        let piece = |from: f64, to: f64| {
            let mut side = arc(centre, self.r, from + self.shift, to + self.shift);
            if let Curve::Arc { start, end, .. } = &mut side {
                *start = at(centre, self.r, from);
                *end = at(centre, self.r, to);
            }
            side
        };
        let mut sides = if self.split {
            vec![piece(self.from, 0.0), piece(0.0, self.to)]
        } else {
            vec![piece(self.from, self.to)]
        };
        let (low, _) = ends(&sides[0]);
        let (_, high) = ends(&sides[sides.len() - 1]);
        sides.push(line([INNER, high[1]], [INNER, low[1]]));
        xz(joined(sides), vec![])
    }

    /// The tight box of a whole turn by hand: out to the arc's farthest x, c + r at angle 0,
    /// and from the arc's lower end to its upper.
    fn bounds(&self) -> [f64; 6] {
        let x = self.c + self.r;
        let z = |t: f64| self.zc + self.r * libm::sin(t);
        [-x, -x, z(self.from), x, x, z(self.to)]
    }

    /// The box its booleans are made with, at its own scale as a through-all tool is:
    /// [`THROUGH_ALL_FACTOR`] times its whole turn's diagonal each way from the axis.
    fn half(&self) -> f64 {
        let [x0, y0, z0, x1, y1, z1] = self.bounds();
        let (dx, dy, dz) = (x1 - x0, y1 - y0, z1 - z0);
        let diagonal = (dx * dx + dy * dy + dz * dz).sqrt();
        THROUGH_ALL_FACTOR * diagonal
    }

    /// The fuzzy value its booleans with [`beyond_of`]'s box `half` run with, by hand:
    /// 1e-6 of the diagonal of the box round both, from (−half, −x, −half) to (half, half,
    /// half), x its farthest reach.
    fn fuzzy(&self, half: f64) -> f64 {
        let x = self.c + self.r;
        1e-6 * (8.0 * half * half + (half + x) * (half + x)).sqrt()
    }
}

fn barrels() -> impl Strategy<Value = Barrel> {
    let edge_starts = prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(-1e-12),
        Just(-5e-10),
        Just(-1e-9),
        Just(-2e-9),
        Just(-1e-6),
    ];
    (
        2.0f64..15.0,
        // The arc's farthest x, as a share of the way from the bore plus 1/2 to twice r.
        0.0f64..1.0,
        -3.0f64..3.0,
        prop_oneof![1 => edge_starts.prop_map(|s| (s, true)), 3 => (0.0f64..1.0).prop_map(|s| (s, false))],
        0.02f64..1.0,
        prop_oneof![Just(0.0), Just(TAU), Just(-TAU)],
        any::<bool>(),
    )
        .prop_filter_map(
            "an arc that keeps to x > 1.2 and rises",
            |(r, share, zc, (start, exact), to_share, shift, split)| {
                // c + r between 1.5 and 2r (so |c| < r: the circle reaches the axis), off r
                // (a centre on the axis is a sphere).
                let far = INNER + 0.5 + share * (2.0 * r - INNER - 0.5);
                let c = far - r;
                if c.abs() < 1e-3 || c.abs() >= r {
                    return None;
                }
                // The widest angle with x ≥ 1.2.
                let m = (INNER + 0.2 - c) / r;
                let limit = 0.95 * FRAC_PI_2;
                let widest = if m <= 0.0 {
                    limit
                } else {
                    libm::acos(m).min(limit)
                };
                if widest < 0.05 {
                    return None;
                }
                let from = if exact { start } else { -start * widest };
                let to = to_share * widest;
                // Split pieces worth the name.
                let split = split && from < -1e-6;
                Some(Barrel {
                    c,
                    zc,
                    r,
                    from,
                    to,
                    shift,
                    split,
                })
            },
        )
}

thread_local! {
    static WORKER: RefCell<Option<KernelWorker>> = const { RefCell::new(None) };
}

fn with_worker<T>(f: impl FnOnce(&mut KernelWorker) -> T) -> T {
    WORKER.with(|cell| {
        let mut slot = cell.borrow_mut();
        f(slot.get_or_insert_with(kernel))
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 200,
        failure_persistence: None,
        // Fixed, so a red run is red every time.
        rng_seed: proptest::test_runner::RngSeed::Fixed(0xa5c0_0000),
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_random_barrel_across_angle_0_is_cut_commoned_and_fused_as_by_hand(
        barrel in barrels(),
        turn in prop_oneof![Just(TAU), Just(1.5 * PI), Just(FRAC_PI_2)],
        start in prop_oneof![Just(0.0), -PI..PI],
    ) {
        // A part turn from elsewhere would hold another share of the cap: whole turns only
        // from elsewhere.
        let start = if turn == TAU { start } else { 0.0 };
        let regions = barrel.regions();
        // The box at the barrel's own scale, holding all of it. The cut and the common to
        // 5e-7, as before the fuzzy value: no face of the barrel is within it of the box's
        // (FreeCAD 1.1.3's `-1.0` and `0.0` cuts of 120 such barrels at this box agree
        // exactly). The fuse, whose volume is the box's, within the fuzzy value times its
        // area: every edge OCCT makes is within the fuzzy value of the true intersection, so
        // no face moves farther (docs/ports/booleans.md), tighter here than 5e-7 of the box.
        let half = barrel.half();
        let [x0, _, z0, x1, _, z1] = barrel.bounds();
        prop_assert!(half > x1.max(-x0).max(z1.abs()).max(z0.abs()).max(Y0), "{barrel:?}");
        let fuzzy = barrel.fuzzy(half);
        let within = |op: BooleanOp, want: f64, area: f64| match op {
            BooleanOp::Fuse => fuzzy * area,
            _ => 5e-7 * want.abs(),
        };
        let wrong = with_worker(|k| {
            let against = Against { half, allowed: &within };
            let mut wrong = misses_against(k, &format!("{barrel:?} from {start}"), &regions, Z_AXIS, start, turn, against);
            if turn == TAU && start == 0.0 {
                let solid = revolved(k, &regions, Z_AXIS, 0.0, TAU);
                let [low, high] = k.summarize(&solid).unwrap().bounds.unwrap();
                let want = barrel.bounds();
                if low.iter().chain(&high).zip(want).any(|(got, want)| (got - want).abs() > 1e-6) {
                    wrong.push(format!("{barrel:?}: box {low:?} {high:?} for {want:?}"));
                }
            }
            wrong
        });
        prop_assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
