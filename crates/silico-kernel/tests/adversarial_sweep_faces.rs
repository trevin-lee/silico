//! break-it: hostile faces, vectors, axes, and angles for M3.6's sweeps of a profile's faces
//! (`prism`, `prism_through_all`, `revolution`, docs/design/m3.6-sketch-features.md S3, S4),
//! in process and through the worker, where the core reads the reply by
//! `History::sweep_fault`. Each sweep must give a valid solid of the volume a hand
//! calculation gives (a box's, or Pappus's: the angle times the area times its centroid's
//! distance from the axis), whose history keeps the design's rule, or a structured refusal
//! that isn't the core blaming the worker (`protocol_error`, "start a new kernel worker").
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::imports;
use super::numbers;

use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::path::Path;

use numbers::relative_error;
use silico_core::ErrorCode;
use silico_core::kernel::KernelWorker;
use silico_kernel::{
    Axis, Built, Curve, FaceRegions, KernelError, KernelErrorKind, Region, ShapeSummary,
    SketchPlane, make_faces, prism, prism_through_all, revolution, summarize, to_brep,
};
use silico_protocol::{Operation, Refusal, SweepKind, THROUGH_ALL_FACTOR};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
    Curve::Line { start, end }
}

/// The closed polygon through `points`, in order.
fn polygon(points: &[[f64; 2]]) -> Vec<Curve> {
    (0..points.len())
        .map(|i| line(points[i], points[(i + 1) % points.len()]))
        .collect()
}

fn rectangle(x: f64, y: f64, w: f64, h: f64) -> Vec<Curve> {
    polygon(&[[x, y], [x + w, y], [x + w, y + h], [x, y + h]])
}

fn one(plane: SketchPlane, outer: Vec<Curve>) -> FaceRegions {
    FaceRegions {
        plane,
        regions: vec![Region {
            outer,
            holes: vec![],
        }],
    }
}

/// The z axis: on the XZ plane, sketch x is model x and sketch y is model z, so a profile
/// at sketch x ≥ 0 lies on one side of it.
const Z: Axis = Axis {
    origin: [0.0; 3],
    direction: [0.0, 0.0, 1.0],
};

/// A sweep's outcome judged: a valid solid of `volume` (to 1e-6), whose history is
/// consistent and keeps the sweep's rule; or a refusal that says what to change and isn't
/// a bug report. Returns the summary of a built one.
fn judge(
    what: &str,
    result: Result<Built, KernelError>,
    kind: SweepKind,
    volume: f64,
) -> Option<ShapeSummary> {
    match result {
        Ok(built) => {
            assert_eq!(built.history.validate(1), Ok(()), "{what}");
            assert_eq!(built.history.sweep_fault(kind), None, "{what}");
            let s = summarize(&built.shape).unwrap();
            assert!(s.valid, "{what}: built an invalid solid, {s:?}");
            assert!(
                relative_error(s.volume, volume) <= 1e-6,
                "{what}: volume {}, want {volume}",
                s.volume
            );
            Some(s)
        }
        Err(e) => {
            assert!(
                matches!(
                    e.kind(),
                    KernelErrorKind::Failed
                        | KernelErrorKind::InvalidResult
                        | KernelErrorKind::InvalidInput
                ),
                "{what}: {e:?}"
            );
            assert!(e.suggestion().is_some(), "{what}: no suggestion, {e:?}");
            None
        }
    }
}

/// The same through the worker and the core's typed request, which reads the reply by the
/// sweep's rule: a valid input's sweep is built or refused for a reason the user can act
/// on, never called the worker's fault.
fn judge_worker(what: &str, result: Result<silico_core::kernel::BuiltData, silico_core::Error>) {
    if let Err(e) = result {
        assert_ne!(
            e.code(),
            ErrorCode::ProtocolError,
            "{what}: a valid sweep's reply refused as the worker's fault: {e:?}"
        );
        assert_ne!(e.code(), ErrorCode::Internal, "{what}: {e:?}");
        assert!(e.suggestion().is_some(), "{what}: {e:?}");
    }
}

fn kernel() -> KernelWorker {
    KernelWorker::start_worker(Path::new(WORKER)).unwrap()
}

// --- Profiles touching the axis at a point -------------------------------------------------

/// A right triangle on XZ with one corner on the z axis and no side along it: (0, 0),
/// (3, 0), (3, 4). Turned about z it's a cylinder (r 3, h 4) less a cone: area 6, centroid
/// at x 2, so θ × 12 (24π for a whole turn: 36π − 12π).
fn corner_on_axis() -> FaceRegions {
    one(
        SketchPlane::Xz,
        polygon(&[[0.0, 0.0], [3.0, 0.0], [3.0, 4.0]]),
    )
}

#[test]
fn a_triangle_with_one_corner_on_the_axis_turns_a_quarter() {
    // Found by break-it, fixed: OCCT reports the corner on the axis as generating the cone's
    // degenerate apex edge, so the vertex has a side and no end, which History::sweep_fault
    // called "neither on the axis nor off", and the core refused as the worker's fault
    // (protocol_error) the commonest revolve there is. The turn's rule now says what OCCT
    // does: a vertex on the axis has no end and sweeps only edges that are a point at it.
    let made = make_faces(&corner_on_axis()).unwrap();
    judge(
        "corner on the axis, a quarter turn",
        revolution(&made.shape, &Z, 0.0, FRAC_PI_2),
        SweepKind::Turn,
        12.0 * FRAC_PI_2,
    );
}

#[test]
fn a_triangle_with_one_corner_on_the_axis_turns_a_quarter_through_the_worker() {
    // Found by break-it, fixed (as above): through the core it was a ProtocolError, "start
    // a new kernel worker", for a valid input.
    let mut k = kernel();
    let made = k.make_faces(&corner_on_axis()).unwrap();
    let built = k.revolution(&made.shape, Z, 0.0, FRAC_PI_2);
    judge_worker("corner on the axis, a quarter turn", built);
    // And the worker goes on.
    let again = k.revolution(&made.shape, Z, 0.0, TAU);
    judge_worker("corner on the axis, a whole turn", again);
}

#[test]
fn a_triangle_with_one_corner_on_the_axis_turns_whole() {
    let made = make_faces(&corner_on_axis()).unwrap();
    judge(
        "corner on the axis, a whole turn",
        revolution(&made.shape, &Z, 0.0, TAU),
        SweepKind::WholeTurn,
        24.0 * PI,
    );
}

/// (0, 0), (3, 0), (0, 4): a cone, its side x = 0 on the axis. Area 6, centroid x 1:
/// θ × 6 (12π, the cone r 3 h 4, for a whole turn).
fn cone() -> FaceRegions {
    one(
        SketchPlane::Xz,
        polygon(&[[0.0, 0.0], [3.0, 0.0], [0.0, 4.0]]),
    )
}

#[test]
fn a_cone_turns_whole() {
    let made = make_faces(&cone()).unwrap();
    judge(
        "cone, a whole turn",
        revolution(&made.shape, &Z, 0.0, TAU),
        SweepKind::WholeTurn,
        6.0 * TAU,
    );
}

#[test]
fn a_cone_whose_side_lies_on_the_axis_turns_part_way() {
    // Found by break-it, fixed: the apex (0, 4), on the axis where the slanted side meets the
    // side along it, generates the cone's degenerate apex edge, which the turn's rule
    // refused ("Vertex3 has 0 end and 1 side counterparts"); the rectangle against the axis
    // passed only because its corners on the axis meet sides square to it.
    let made = make_faces(&cone()).unwrap();
    for angle in [1e-3, FRAC_PI_2, PI, 1.5 * PI] {
        judge(
            &format!("cone, {angle} rad"),
            revolution(&made.shape, &Z, 0.0, angle),
            SweepKind::of_turn(angle),
            6.0 * angle,
        );
    }
}

/// (0, 2), (2, 0), (4, 2), (2, 4): a diamond touching the z axis at one corner. Area 8,
/// centroid x 2, so θ × 16.
fn diamond() -> FaceRegions {
    one(
        SketchPlane::Xz,
        polygon(&[[0.0, 2.0], [2.0, 0.0], [4.0, 2.0], [2.0, 4.0]]),
    )
}

#[test]
fn a_diamond_touching_the_axis_at_one_corner_turns_whole() {
    let made = make_faces(&diamond()).unwrap();
    judge(
        "diamond, a whole turn",
        revolution(&made.shape, &Z, 0.0, TAU),
        SweepKind::WholeTurn,
        16.0 * TAU,
    );
}

#[test]
fn a_diamond_touching_the_axis_at_one_corner_turns_part_way() {
    // Found by break-it, fixed: as the triangle with a corner on the axis, but the corner
    // generates two degenerate edges (MakeRevol copies one two faces share); the rule takes
    // any number of them, each a point at the vertex.
    let made = make_faces(&diamond()).unwrap();
    for angle in [FRAC_PI_2, PI] {
        judge(
            &format!("diamond, {angle} rad"),
            revolution(&made.shape, &Z, 0.0, angle),
            SweepKind::of_turn(angle),
            16.0 * angle,
        );
    }
}

#[test]
fn a_circle_tangent_to_the_axis_turns_into_a_horn_torus_or_is_refused() {
    // Centre (2, 0), radius 2 on XZ: it touches the z axis at the origin. Pappus: 4π × 2 × θ.
    let made = make_faces(&one(
        SketchPlane::Xz,
        vec![Curve::Circle {
            center: [2.0, 0.0],
            radius: 2.0,
        }],
    ))
    .unwrap();
    for angle in [FRAC_PI_2, TAU] {
        judge(
            &format!("horn torus, {angle} rad"),
            revolution(&made.shape, &Z, 0.0, angle),
            SweepKind::of_turn(angle),
            8.0 * PI * angle,
        );
    }
}

// --- Angles at OCCT's edges ----------------------------------------------------------------

#[test]
fn angles_a_hair_short_of_a_turn_build_the_turn_they_say_or_are_refused() {
    // The square x 1..3, z 0..2 on XZ about z: area 4, centroid x 2, so 8θ.
    let made = make_faces(&one(SketchPlane::Xz, rectangle(1.0, 0.0, 2.0, 2.0))).unwrap();
    let mut k = kernel();
    let sent = k
        .make_faces(&one(SketchPlane::Xz, rectangle(1.0, 0.0, 2.0, 2.0)))
        .unwrap();
    for short in [
        1e-6,
        1e-9,
        1e-10,
        2e-12,
        1.0000000000000002e-12,
        1e-12,
        0.5e-12,
    ] {
        let angle = TAU - short;
        let what = format!("2π − {short:e}");
        judge(
            &what,
            revolution(&made.shape, &Z, 0.0, angle),
            SweepKind::of_turn(angle),
            8.0 * angle,
        );
        judge_worker(&what, k.revolution(&sent.shape, Z, 0.0, angle));
    }
}

#[test]
fn angles_just_above_occts_angular_precision_build_a_sliver_or_are_refused() {
    // A unit square 1000 mm from the axis: 1 × 1000.5 × θ.
    let made = make_faces(&one(SketchPlane::Xz, rectangle(1000.0, 0.0, 1.0, 1.0))).unwrap();
    for angle in [1.0000000000000002e-12, 1.1e-12, 1e-11, 1e-10] {
        judge(
            &format!("{angle:e} rad"),
            revolution(&made.shape, &Z, 0.0, angle),
            SweepKind::Turn,
            1000.5 * angle,
        );
    }
}

#[test]
fn a_whole_turn_from_any_start_is_the_same_solid() {
    let made = make_faces(&one(SketchPlane::Xz, rectangle(1.0, 0.0, 2.0, 2.0))).unwrap();
    let here = revolution(&made.shape, &Z, 0.0, TAU).unwrap();
    for start in [-TAU, -PI, 1e-300, PI / 3.0, TAU] {
        let what = format!("start {start}");
        let turned = judge(
            &what,
            revolution(&made.shape, &Z, start, TAU),
            SweepKind::WholeTurn,
            16.0 * PI,
        );
        assert!(turned.is_some(), "{what}");
        let built = revolution(&made.shape, &Z, start, TAU).unwrap();
        assert_eq!(built.history.result, here.history.result, "{what}");
    }
}

// --- Directions and axes the faces can't make a solid along --------------------------------

#[test]
fn a_prism_along_its_own_plane_is_refused_not_a_flat_solid() {
    // Found by break-it, fixed: this built, valid, volume 0 (6 faces, 12 edges, 8 vertices,
    // all at z = 0). The kernel now refuses a vector within 1e-7 of the faces' plane, as M1's
    // extrude does (check_leaves_plane, Refusal::SweepFlat), and any sweep whose result has
    // no volume to speak of or is turned inside out (amount_of, Refusal::ResultInvalid).
    // A 2 × 2 square on XY swept along +x: no volume. The core never sends it (its vector is
    // the plane's normal), but the kernel takes any vector; it must not build a flat
    // "solid".
    let made = make_faces(&one(SketchPlane::Xy, rectangle(1.0, 1.0, 2.0, 2.0))).unwrap();
    let flat = prism(&made.shape, [0.0; 3], [5.0, 0.0, 0.0]);
    match flat {
        Ok(built) => {
            let s = summarize(&built.shape).unwrap();
            panic!("a prism along its own plane built: {s:?}");
        }
        Err(e) => assert!(e.suggestion().is_some(), "{e:?}"),
    }
    // A hair off the plane: 4 × 1e-9 thick, built exactly or refused.
    judge(
        "a prism 1e-9 off its plane",
        prism(&made.shape, [0.0; 3], [5.0, 0.0, 1e-9]),
        SweepKind::Prism,
        4e-9,
    );
}

#[test]
fn a_prism_just_longer_than_the_tolerance_is_built_exactly_or_refused() {
    let made = make_faces(&one(SketchPlane::Xy, rectangle(1.0, 1.0, 2.0, 2.0))).unwrap();
    for length in [1.0000000000000002e-7, 1.01e-7, 2e-7, 4e-7] {
        judge(
            &format!("a prism {length:e} long"),
            prism(&made.shape, [0.0; 3], [0.0, 0.0, length]),
            SweepKind::Prism,
            4.0 * length,
        );
    }
}

/// The refusal `swept` gives a sweep whose result has no volume to speak of (`amount_of`):
/// an invalid result of `operation`, naming no edges.
fn no_volume_to_speak_of(what: &str, result: Result<Built, KernelError>, operation: Operation) {
    match result {
        Ok(built) => {
            let s = summarize(&built.shape).unwrap();
            panic!("{what}: built, {s:?}");
        }
        Err(e) => assert_eq!(
            e.refusal(),
            &Refusal::ResultInvalid {
                operation,
                edges: vec![],
            },
            "{what}"
        ),
    }
}

#[test]
fn a_prism_thinner_than_twice_the_tolerance_is_refused_as_having_no_volume() {
    // A 10 × 10 square on XY swept 1.5e-7 along z: longer than the 1e-7 check_prism asks,
    // and straight out of its plane, so only the result's amount can refuse it. By hand:
    // volume 100 × 1.5e-7 = 1.5e-5; area 2 × 100 + 4 × 10 × 1.5e-7 = 200.000006, so its
    // mean thickness, 7.5e-8, is within the 1e-7 tolerance of none (1.5e-5 ≤ 1e-7 × 200):
    // no volume to speak of. Its rough volume (1.5e-5) is far under 1e-5 × its box's
    // diagonal cubed (√200³ ≈ 2828, so 0.028), so it's measured precisely, not cleared.
    let made = make_faces(&one(SketchPlane::Xy, rectangle(0.0, 0.0, 10.0, 10.0))).unwrap();
    no_volume_to_speak_of(
        "a 10 × 10 prism 1.5e-7 long",
        prism(&made.shape, [0.0; 3], [0.0, 0.0, 1.5e-7]),
        Operation::Extrusion,
    );
    // Twice as long, its mean thickness 1.5e-7 (3e-5 > 1e-7 × 200): built, of the volume
    // 100 × 3e-7.
    let built = judge(
        "a 10 × 10 prism 3e-7 long",
        prism(&made.shape, [0.0; 3], [0.0, 0.0, 3e-7]),
        SweepKind::Prism,
        3e-5,
    );
    assert!(built.is_some(), "a 10 × 10 prism 3e-7 long was refused");
}

#[test]
fn a_turn_thinner_than_twice_the_tolerance_is_refused_as_having_no_volume() {
    // The unit square x 100..101, z 0..1 on XZ turned 1.5e-9 rad about z: above OCCT's 1e-12
    // radians, and every edge it sweeps at least 100 × 1.5e-9 = 1.5e-7 long, longer than the
    // tolerance. By hand (Pappus): volume 1.5e-9 × 1 × 100.5 = 1.5075e-7; area 2 (the
    // square, at each end) + θ × (101 + 100) (the cylinders) + θ × (101² − 100²) (the two
    // annulus sectors) = 2 + 402θ ≈ 2.0000006, so its mean thickness, 7.5e-8, is within the
    // tolerance of none (1.5075e-7 ≤ 1e-7 × 2.0000006). Its rough volume is far under
    // 1e-5 × √2³ ≈ 2.8e-5, so it's measured precisely.
    let made = make_faces(&one(SketchPlane::Xz, rectangle(100.0, 0.0, 1.0, 1.0))).unwrap();
    no_volume_to_speak_of(
        "a 1.5e-9 rad turn 100 from the axis",
        revolution(&made.shape, &Z, 0.0, 1.5e-9),
        Operation::Revolve,
    );
    // Twice the angle, its mean thickness 1.5e-7 (3.015e-7 > 1e-7 × 2.0000012): built, of
    // the volume 3e-9 × 100.5.
    let built = judge(
        "a 3e-9 rad turn 100 from the axis",
        revolution(&made.shape, &Z, 0.0, 3e-9),
        SweepKind::Turn,
        3e-9 * 100.5,
    );
    assert!(
        built.is_some(),
        "a 3e-9 rad turn 100 from the axis was refused"
    );
}

#[test]
fn a_turn_about_the_faces_own_normal_is_refused_not_a_flat_solid() {
    // Found by break-it, fixed: this built, valid, volume 0 (area 26.85, every point at
    // z = 0). The kernel now refuses an axis leaning more than 1e-7 out of the faces' plane
    // (check_axis_in_plane, Refusal::SweepFlat), as M1's revolve refuses one out of the
    // profile's.
    // The square x 1..3, y 0..2 on XY, about z: it spins in its own plane, sweeping an
    // annulus sector with no thickness.
    let made = make_faces(&one(SketchPlane::Xy, rectangle(1.0, 0.0, 2.0, 2.0))).unwrap();
    for angle in [FRAC_PI_2, TAU] {
        match revolution(&made.shape, &Z, 0.0, angle) {
            Ok(built) => {
                let s = summarize(&built.shape).unwrap();
                panic!("a turn about the faces' normal built ({angle} rad): {s:?}");
            }
            Err(e) => assert!(e.suggestion().is_some(), "{angle}: {e:?}"),
        }
    }
}

#[test]
fn an_axis_leaning_out_of_the_faces_plane_sweeps_a_valid_solid_or_is_refused() {
    // The square x 1..3, z 0..2 on XZ about an axis through the origin leaning out of the
    // plane by 1e-9: Pappus still gives about 8θ (the centroid's distance changes by 1e-9).
    let made = make_faces(&one(SketchPlane::Xz, rectangle(1.0, 0.0, 2.0, 2.0))).unwrap();
    let lean: f64 = 1e-9;
    let n = (1.0 + lean * lean).sqrt();
    let axis = Axis {
        origin: [0.0; 3],
        direction: [0.0, lean / n, 1.0 / n],
    };
    for angle in [FRAC_PI_2, TAU] {
        judge(
            &format!("leaning axis, {angle} rad"),
            revolution(&made.shape, &axis, 0.0, angle),
            SweepKind::of_turn(angle),
            8.0 * angle,
        );
    }
}

// --- Through all ---------------------------------------------------------------------------

#[test]
fn through_all_of_faces_with_the_faces_as_the_base_is_sized_by_the_faces_alone() {
    // A base with no solid (the faces themselves): the box is the faces', 4 × 3, diagonal 5,
    // so 2.02 × 5 = 10.1 along z.
    let made = make_faces(&one(SketchPlane::Xy, rectangle(0.0, 0.0, 4.0, 3.0))).unwrap();
    judge(
        "faces as the base",
        prism_through_all(&made.shape, &made.shape, [0.0, 0.0, 1.0]),
        SweepKind::Prism,
        12.0 * THROUGH_ALL_FACTOR * 5.0,
    );
}

#[test]
fn through_all_of_a_base_the_rule_admits_is_never_refused_for_a_vector_it_wasnt_given() {
    // Found by break-it, fixed: a 495 049.5 × 1 × 1 base whose through-all length is a few
    // ulps under a kilometre, along (1 + 2ε, 0, 0), a direction check_through_all admits,
    // made a vector of 1000000.0000000005, which check_prism refused naming `vector`, a
    // parameter the through-all request doesn't have. check_prism now allows a unit
    // direction's rounding past the kilometre, and the kernel names `base` should the
    // rules ever not compose.
    let direction = [1.0 + 2.0 * f64::EPSILON, 0.0, 0.0];
    // On YZ, whose normal is +x, so the direction leaves the faces' plane (the kernel
    // refuses a sweep along its own plane, below, before it sizes one).
    let square = make_faces(&one(SketchPlane::Yz, rectangle(0.0, 0.0, 1.0, 1.0))).unwrap();
    let reach = silico_kernel::MAX_LENGTH / THROUGH_ALL_FACTOR;
    let mut d = (reach * reach - 2.0).sqrt();
    let mut tried = 0;
    loop {
        tried += 1;
        assert!(tried < 200, "no base found at the kilometre");
        let base = silico_kernel::make(&silico_kernel::Primitive::Box {
            dx: d,
            dy: 1.0,
            dz: 1.0,
        })
        .unwrap();
        let [low, high] = summarize(&base).unwrap().bounds.unwrap();
        match silico_protocol::through_all_length([low, high]) {
            Ok(length) if length * direction[0] > silico_kernel::MAX_LENGTH => {
                match prism_through_all(&square.shape, &base, direction) {
                    Ok(built) => {
                        let s = summarize(&built.shape).unwrap();
                        assert!(relative_error(s.volume, length) <= 1e-9, "{s:?}");
                    }
                    Err(e) => match e.refusal() {
                        silico_kernel::Refusal::InvalidParameter { parameter, .. } => {
                            assert_eq!(parameter, "base", "{e:?}");
                        }
                        other => panic!("{other:?}"),
                    },
                }
                return;
            }
            Ok(_) => d = d.next_up(),
            Err(_) => d = d.next_down(),
        }
    }
}

// --- Scale -------------------------------------------------------------------------------

#[test]
fn faces_at_the_edge_of_the_kilometre_sweep_as_their_values_say() {
    // A 10 × 10 square with its far corner at 1e6, swept the full kilometre.
    let made = make_faces(&one(
        SketchPlane::Xy,
        rectangle(999_990.0, 999_990.0, 10.0, 10.0),
    ))
    .unwrap();
    judge(
        "far square, a kilometre",
        prism(&made.shape, [0.0, 0.0, -1e6], [0.0, 0.0, 1e6]),
        SweepKind::Prism,
        100.0 * 1e6,
    );
    // Turned about an axis a kilometre out: Pappus, 100 × (1e6 − 5 − 999 000) × θ, about
    // the line x = 999 000 along y.
    let axis = Axis {
        origin: [999_000.0, 0.0, 0.0],
        direction: [0.0, 1.0, 0.0],
    };
    judge(
        "far square, turned",
        revolution(&made.shape, &axis, 0.0, FRAC_PI_2),
        SweepKind::Turn,
        100.0 * 995.0 * FRAC_PI_2,
    );
}

#[test]
fn a_profile_of_thousands_of_sides_and_a_hundred_holes_sweeps_in_reasonable_time() {
    // A 2,000-gon of radius 50 about (100, 0) on XZ, with 100 round holes: every side and
    // hole is an element whose start, end, and sides the bridge looks up. Areas by formula:
    // the polygon's ½ n r² sin(2π/n), less the holes'; its centroid at x 100 by symmetry.
    let n = 2000;
    let r = 50.0;
    let points: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let t = TAU * i as f64 / n as f64;
            [100.0 + r * libm::cos(t), r * libm::sin(t)]
        })
        .collect();
    let holes: Vec<Vec<Curve>> = (0..100)
        .map(|k| {
            let (i, j) = ((k % 10) as f64, (k / 10) as f64);
            vec![Curve::Circle {
                center: [100.0 - 27.0 + 6.0 * i, -27.0 + 6.0 * j],
                radius: 1.0,
            }]
        })
        .collect();
    let regions = FaceRegions {
        plane: SketchPlane::Xz,
        regions: vec![Region {
            outer: polygon(&points),
            holes,
        }],
    };
    let area = 0.5 * n as f64 * r * r * libm::sin(TAU / n as f64) - 100.0 * PI;
    // The holes sit symmetrically about x 100 too.
    let started = std::time::Instant::now();
    let made = make_faces(&regions).unwrap();
    judge(
        "2,000-gon prism",
        prism(&made.shape, [0.0; 3], [0.0, 10.0, 0.0]),
        SweepKind::Prism,
        area * 10.0,
    );
    judge(
        "2,000-gon whole turn",
        revolution(&made.shape, &Z, 0.0, TAU),
        SweepKind::WholeTurn,
        area * 100.0 * TAU,
    );
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs() < 60, "took {elapsed:?}");
}

// --- Determinism ---------------------------------------------------------------------------

#[test]
fn a_sweep_leaves_the_faces_it_was_given_as_they_were() {
    // Found by break-it, fixed: `revolution` took `&Shape` but wrote into it: OCCT's
    // MakeRevol shared the faces' edges with the result's start face and added the new
    // faces' curves and locations to them, so the faces' bytes changed and each later sweep
    // of the same faces wrote more (3649, 4659, 5670 bytes for three identical
    // revolutions); a prism changed them too. The worker reads its faces fresh each request,
    // so its replies didn't show it. Both sweeps now build with OCCT's Copy flag, on copies
    // of the faces' edges and vertices, with the same history.
    let made = make_faces(&one(SketchPlane::Xz, rectangle(1.0, 0.0, 2.0, 2.0))).unwrap();
    let before = to_brep(&made.shape).unwrap();
    let a = revolution(&made.shape, &Z, 0.0, 1.0).unwrap();
    assert!(
        to_brep(&made.shape).unwrap() == before,
        "the revolution changed the faces it was given"
    );
    let b = revolution(&made.shape, &Z, 0.0, 1.0).unwrap();
    assert!(
        to_brep(&a.shape).unwrap() == to_brep(&b.shape).unwrap(),
        "the same revolution of the same faces, twice, gave different bytes"
    );
    assert_eq!(a.history, b.history);
    let p = prism(&made.shape, [0.0; 3], [0.0, 1.0, 0.0]).unwrap();
    let q = prism(&made.shape, [0.0; 3], [0.0, 1.0, 0.0]).unwrap();
    assert!(to_brep(&p.shape).unwrap() == to_brep(&q.shape).unwrap());
}

#[test]
fn the_same_sweep_of_the_same_bytes_and_a_signed_zero_start_give_the_same_bytes_and_history() {
    // As the worker runs it: the faces read fresh from their bytes each time.
    let made = make_faces(&one(SketchPlane::Xz, rectangle(1.0, 0.0, 2.0, 2.0))).unwrap();
    let bytes = to_brep(&made.shape).unwrap();
    let fresh = || silico_kernel::from_brep(&bytes).unwrap();
    let a = revolution(&fresh(), &Z, 0.0, 1.0).unwrap();
    let b = revolution(&fresh(), &Z, 0.0, 1.0).unwrap();
    let c = revolution(&fresh(), &Z, -0.0, 1.0).unwrap();
    assert!(to_brep(&a.shape).unwrap() == to_brep(&b.shape).unwrap());
    assert!(to_brep(&a.shape).unwrap() == to_brep(&c.shape).unwrap());
    assert_eq!(a.history, b.history);
    assert_eq!(a.history, c.history);
    let p = prism(&fresh(), [0.0; 3], [0.0, 1.0, 0.0]).unwrap();
    let q = prism(&fresh(), [-0.0, 0.0, -0.0], [0.0, 1.0, 0.0]).unwrap();
    assert!(to_brep(&p.shape).unwrap() == to_brep(&q.shape).unwrap());
    assert_eq!(p.history, q.history);
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}
