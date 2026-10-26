//! /break-it on M3.4, stored coordinates (docs/design/m3.4-stored-coordinates.md): a
//! sketch's file holds its last solve, every rebuild solves every sketch, a solve that
//! differs from the file is drift, and a save writes the solve back. Hostile here: values a
//! solve may give that the file must hold and read back exactly (−0, subnormals, the edge of
//! reach), reference measures that depend on what's stored, keys that must change with what
//! the solve reads, a solver that panics or fails transiently, and recomputes superseded and
//! cancelled mid-solve, at the generations' end too. On the fake kernel and the fake sketch
//! solver, so each answer is the one under test; the real worker's side is silico-sketch's
//! tests/adversarial_stored_coordinates.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::commands;
use super::fixture;

use fixture::{line, point_debug};

use std::sync::Arc;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use silico_core::command::{self, Formatted};
use silico_core::document::{Angle, Checked, FeatureId, Operation, read};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Generation, Kernel};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use silico_protocol::SketchSystem;

/// The values sent before the points: the origin and the axes' ends.
const FIRST_POINT: usize = 6;

fn engine(sketches: &FakeSketches) -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(sketches.clone())
}

fn checked(text: &str) -> Checked {
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

fn published(engine: &Recompute<FakeKernel>, text: &str) -> Arc<Model> {
    match engine
        .start(checked(text))
        .wait_timeout(Duration::from_secs(20))
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

fn format(engine: &Recompute<FakeKernel>, text: &str) -> Formatted {
    match commands::format_bytes(engine, text.as_bytes()).unwrap() {
        Ok(formatted) => formatted,
        Err(problems) => panic!("the gate refused it: {:?}", problems.errors()),
    }
}

fn feature() -> FeatureId {
    FeatureId::new(fixture::FEATURE).unwrap()
}

/// What a file saved with a solve reads as again, on an engine whose solver gives back what
/// it's sent: a fixed point has no drift there, and formats to the same bytes.
fn holds_as_saved(saved: &str) {
    command::gate(saved.as_bytes())
        .unwrap_or_else(|p| panic!("the save wrote a file the gate refuses: {:?}", p.errors()));
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    let model = published(&engine, saved);
    assert!(
        model.drift().is_empty(),
        "the file a save wrote drifts: {:?}\n{saved}",
        model.drift()
    );
    let again = format(&engine, saved);
    assert!(again.resolved.is_empty(), "{:?}", again.resolved);
    assert_eq!(again.text, saved, "a second save rewrote the file");
}

// --- Reference angles: a measure that depends on what's stored ---------------------------

/// pa (0, 0), pb (1, 0), pc (`cx`, `cy`); na from pa to pb, nb from pa to pc; a reference
/// angle from na to nb stored `value` degrees.
fn reference_angle(cx: f64, cy: f64, value: &str) -> String {
    let points = [
        point_debug("pa000000", 0.0, 0.0),
        point_debug("pb000000", 1.0, 0.0),
        point_debug("pc000000", cx, cy),
    ]
    .join(", ");
    let lines = [
        line("na000000", "pa000000", "pb000000"),
        line("nb000000", "pa000000", "pc000000"),
    ]
    .join(", ");
    let constraint = format!(
        r#"{{"id": "r0000001", "type": "angle", "a": "na000000", "b": "nb000000", "value": {value}, "driving": false}}"#
    );
    fixture::document(&fixture::fields(&points, &lines, "", "", &constraint))
}

/// The reference angle's value as `text` stores it.
fn stored_angle(text: &str) -> Angle {
    match &read(text.as_bytes()).unwrap().features[0].operation {
        Operation::Sketch(s) => match s.constraints[0].form {
            silico_core::document::Form::Angle { value, .. } => value,
            ref other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn reference_angle_stored_a_turn_away_flips_its_last_nanodegree_on_every_save() {
    // Found by break-it, fixed (now the measure plus whole turns): a reference angle was
    // measured on its stored value's turn,
    // `fmod(stored + remainder(measured − stored, 2π), 2π)`, so the measure depends on the
    // stored value's low bits when the two are a turn apart (stored 213.95760857°, measured
    // −146.04239143° ≈ the same direction). Here the true measure sits within 1e-13° of a
    // half nanodegree: from ...570 it rounds to ...571, and from ...571 back to ...570. Every
    // save rewrites the file and every check after it reports drift, for good: the fixed
    // point the design rests on (an unchanged sketch re-solved leaves the file unchanged)
    // fails. Same-turn stores are exact (Sterbenz), so only a turn away flips; found by a
    // search of 2e6 directions near half-nanodegree boundaries, 3 of them.
    let text = reference_angle(-0.829451075670893, -0.5585793704285886, "213.95760857");
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    let first = format(&engine, &text);
    let second = format(&engine, &first.text);
    let third = format(&engine, &second.text);
    assert_eq!(
        (
            stored_angle(&first.text).ndeg(),
            stored_angle(&second.text).ndeg(),
            stored_angle(&third.text).ndeg()
        ),
        (
            stored_angle(&first.text).ndeg(),
            stored_angle(&first.text).ndeg(),
            stored_angle(&first.text).ndeg()
        ),
        "the reference angle's stored value keeps changing save after save"
    );
    holds_as_saved(&first.text);
}

#[test]
fn reference_angle_stored_far_from_its_measure_saves_a_value_the_next_save_changes() {
    // Found by break-it, fixed: the same root cause as above, with no turn between them.
    // Measured
    // 4.9999999995e-10° (just under half a nanodegree, so 0), stored −114.591559026°: the
    // measure worked out from that stored value carries its rounding (≈1e-14°) and comes to
    // half a nanodegree or more, so the save writes 0.000000001. Read back, stored 1e-9° is
    // near the measure, which then comes out exact: 0. The first save isn't a fixed point:
    // `silico fmt` then `silico check` reports drift, and a second `fmt` rewrites the file.
    // Proptest's minimal case (a_saved_reference_angle_is_a_fixed_point).
    let rad = 4.999_999_999_5e-10f64.to_radians();
    let text = reference_angle(libm::cos(rad), libm::sin(rad), "-114.591559026");
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    let first = format(&engine, &text);
    assert_eq!(
        format(&engine, &first.text).text,
        first.text,
        "a second save changed the reference angle the first wrote"
    );
}

/// Degrees, biased toward the turn's edges and the ±360 the gate allows.
fn degrees() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(180.0),
        Just(-180.0),
        Just(359.999_999_999),
        Just(-359.999_999_999),
        Just(1e-9),
        Just(-1e-9),
        Just(4.999_999_999_5e-10),
        -180.0..180.0f64,
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    /// A reference angle stored anywhere within ±360 reaches a fixed point by the first
    /// save: saved again, it's unchanged.
    // Found by break-it, fixed: as the two targeted tests above (a measure worked out from the stored value,
    // so a direction within ~1e-13° of a half nanodegree saves one value, then another);
    // the bias toward 4.9999999995e-10° finds it.
    #[test]
    fn a_saved_reference_angle_is_a_fixed_point(
        direction in degrees(),
        stored in -360_000_000_000i64..=360_000_000_000i64,
    ) {
        let rad = direction.to_radians();
        let (cx, cy) = (libm::cos(rad), libm::sin(rad));
        let text = reference_angle(cx, cy, &Angle::from_ndeg(stored).to_string());
        let sketches = FakeSketches::new();
        let engine = engine(&sketches);
        let first = format(&engine, &text);
        holds_as_saved(&first.text);
        // The measure written is the angle drawn, on some turn.
        let written = stored_angle(&first.text).degrees();
        let measured = libm::atan2(cy, cx).to_degrees();
        let off = libm::remainder(written - measured, 360.0).abs();
        prop_assert!(off < 1e-6, "{written} for {measured}");
    }
}

// --- Coordinates: every value a solve may give is written and read back exactly -----------

/// A coordinate a solve may give: anything within reach, biased toward the values whose
/// text is unusual (−0, subnormals, the reach itself, 17 significant digits).
fn coordinate() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(5e-324),
        Just(-5e-324),
        Just(f64::MIN_POSITIVE),
        Just(2.225_073_858_507_201e-308),
        Just(1e6),
        Just(-1e6),
        Just(f64::from_bits(1e6f64.to_bits() - 1)),
        Just(0.1 + 0.2),
        Just(1e-300),
        (-1e6..1e6f64),
        (any::<u64>()).prop_map(|bits| {
            // A random float's mantissa and sign at a random scale within reach.
            let m = f64::from_bits((bits & 0x800F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000);
            m * libm::exp2(f64::from((bits >> 52 & 0x3F) as u8) - 40.0)
        }),
    ]
    .prop_filter("within reach", |v| v.abs() <= 1e6)
}

/// A radius a sketch can hold: above the gate's floor, the kernel's 1e-7 mm (one at or
/// under it is refused: a_solve_to_a_radius_at_the_floor_is_refused_naming_the_circle).
fn radius() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(1e-7f64.next_up()),
        Just(1e6),
        Just(f64::from_bits(1e6f64.to_bits() - 1)),
        Just(1e-6),
        Just(4.9e-7),
        (1e-7f64.next_up()..1e6f64),
    ]
}

#[test]
fn a_solve_to_a_radius_at_the_floor_is_refused_naming_the_circle() {
    // The gate's floor reaches every solve (M3.4's check that a solve's file is one the gate
    // takes, `holdable`): a solve that leaves the circle a radius of 1e-7 mm or less, the
    // kernel's tolerance, is an invalid result naming it, with the gate's own advice; the
    // next float up is kept, bit for bit.
    for r in [1e-7, 1e-9, 5e-324] {
        let sketches = FakeSketches::new();
        sketches.answer(move |system: &SketchSystem| {
            let mut solved = FakeSketches::as_sent(system);
            solved.values[FIRST_POINT + 6] = r;
            Ok(solved)
        });
        let model = published(&engine(&sketches), &many_values());
        let Some(FeatureOutcome::Failed(error)) = model.outcome(&feature()) else {
            panic!("{r}: {:?}", model.outcome(&feature()));
        };
        assert_eq!(error.code(), ErrorCode::InvalidResult, "{r}");
        assert_eq!(error.elements(), ["c0000001"], "{r}");
        assert!(
            error
                .message()
                .contains("arcs and circles have a radius above 1e-7 mm"),
            "{r}: {error}"
        );
        assert_eq!(
            error.suggestion(),
            Some("change the constraints that draw it this small: draw the circle larger"),
            "{r}"
        );
    }
    let kept = 1e-7f64.next_up();
    let sketches = FakeSketches::new();
    sketches.answer(move |system: &SketchSystem| {
        let mut solved = FakeSketches::as_sent(system);
        solved.values[FIRST_POINT + 6] = kept;
        Ok(solved)
    });
    let saved = format(&engine(&sketches), &many_values());
    let document = read(saved.text.as_bytes()).unwrap();
    let Operation::Sketch(s) = &document.features[0].operation else {
        panic!()
    };
    assert_eq!(s.circles[0].radius.mm().to_bits(), kept.to_bits());
}

/// Three points, p1..p3 (two drawn at the origin), a circle about p1, and reference
/// dimensions on them: a distance, a distance_x, a radius and a diameter.
fn many_values() -> String {
    let points = [
        point_debug("p0000001", 0.0, 0.0),
        point_debug("p0000002", 1.0, 1.0),
        point_debug("p0000003", 2.0, -3.0),
    ]
    .join(", ");
    let circle = r#"{"id": "c0000001", "center": "p0000001", "radius": 1, "construction": false}"#;
    let constraints = [
        r#"{"id": "r0000001", "type": "distance", "a": "p0000001", "b": "p0000002", "value": 0, "driving": false}"#,
        r#"{"id": "r0000002", "type": "distance_x", "a": "p0000003", "b": "p0000002", "value": 0, "driving": false}"#,
        r#"{"id": "r0000003", "type": "radius", "curve": "c0000001", "value": 0, "driving": false}"#,
        r#"{"id": "r0000004", "type": "diameter", "curve": "c0000001", "value": 0, "driving": false}"#,
    ]
    .join(", ");
    fixture::document(&fixture::fields(&points, "", circle, "", &constraints))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    /// Whatever a solve gives within reach is written so it reads back bit for bit: the save
    /// passes the gate, has no drift, and saves again to the same bytes.
    #[test]
    fn every_solved_value_within_reach_is_saved_and_read_back_exactly(
        values in proptest::collection::vec(coordinate(), 6),
        r in radius(),
    ) {
        let sketches = FakeSketches::new();
        let answer = values.clone();
        sketches.answer(move |system: &SketchSystem| {
            let mut solved = FakeSketches::as_sent(system);
            solved.values[FIRST_POINT..FIRST_POINT + 6].copy_from_slice(&answer);
            solved.values[FIRST_POINT + 6] = r;
            Ok(solved)
        });
        let engine = engine(&sketches);
        let saved = format(&engine, &many_values());
        // What the solve gave, bit for bit, in the file.
        let document = read(saved.text.as_bytes()).unwrap();
        let Operation::Sketch(s) = &document.features[0].operation else { panic!() };
        let read_back: Vec<u64> = s.points.iter().flat_map(|p| [p.x.mm().to_bits(), p.y.mm().to_bits()]).collect();
        let given: Vec<u64> = values.iter().map(|v| v.to_bits()).collect();
        prop_assert_eq!(read_back, given);
        prop_assert_eq!(s.circles[0].radius.mm().to_bits(), r.to_bits());
        holds_as_saved(&saved.text);
    }
}

#[test]
fn the_farthest_reference_distance_a_sketch_can_hold_is_written_and_read_back() {
    // Two points at opposite corners of the reach: 2√2 km apart, under the 3e6 mm a
    // reference holds.
    let sketches = FakeSketches::new();
    sketches.answer(|system: &SketchSystem| {
        let mut solved = FakeSketches::as_sent(system);
        solved.values[FIRST_POINT..FIRST_POINT + 4].copy_from_slice(&[-1e6, -1e6, 1e6, 1e6]);
        Ok(solved)
    });
    let engine = engine(&sketches);
    let saved = format(&engine, &many_values());
    assert!(
        saved
            .text
            .contains(r#""value": 2828427.124746, "driving": false"#),
        "{}",
        saved.text
    );
    holds_as_saved(&saved.text);
}

// --- Keys: what the solve reads, and only that, is what a solve is kept by ---------------

/// The base sketch with a length on na, `driving` or not.
fn length(driving: bool) -> String {
    fixture::constrained(&format!(
        r#"{{"id": "k0000001", "type": "length", "line": "na000000", "value": 10, "driving": {driving}}}"#
    ))
}

#[test]
fn a_dimension_turned_from_driving_to_reference_is_solved_again() {
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    published(&engine, &length(true));
    published(&engine, &length(false));
    let requests = sketches.requests();
    assert_eq!(
        requests.len(),
        2,
        "a reference dimension took a driving one's solve"
    );
    assert_ne!(requests[0], requests[1]);
    // And back: the first solve, kept.
    published(&engine, &length(true));
    assert_eq!(sketches.requests().len(), 2);
}

/// Two copies of the base sketch, each with this feature ID.
fn two_sketches(a: &str, b: &str) -> String {
    let fields = fixture::fields(
        fixture::POINTS,
        fixture::LINE,
        fixture::CIRCLE,
        fixture::ARC,
        r#"{"id": "k0000001", "type": "horizontal", "line": "na000000"}"#,
    );
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"{a}\", \"type\": \"sketch\", \"v\": 1, \"name\": \"A\", {fields}}}, {{\"id\": \"{b}\", \"type\": \"sketch\", \"v\": 1, \"name\": \"B\", {fields}}}]}}"
    )
}

#[test]
fn two_identical_sketches_are_each_solved_and_each_failure_names_its_own() {
    let sketches = FakeSketches::new();
    sketches.answer(|_| {
        Err(Error::new(
            ErrorCode::SolveFailed,
            "the fake can't meet these constraints",
        ))
    });
    let engine = engine(&sketches);
    let model = published(&engine, &two_sketches("ka000000", "kb000000"));
    assert_eq!(sketches.requests().len(), 2);
    for id in ["ka000000", "kb000000"] {
        match model.outcome(&FeatureId::new(id).unwrap()) {
            Some(FeatureOutcome::Failed(e)) => {
                assert_eq!(e.code(), ErrorCode::SolveFailed);
                assert_eq!(e.feature(), Some(id), "{e}");
            }
            other => panic!("{id}: {other:?}"),
        }
    }
}

// --- A solver that fails, transiently or by panicking --------------------------------------

#[test]
fn a_solve_that_fails_with_the_worker_is_not_kept_and_the_next_rebuild_solves() {
    for code in [
        ErrorCode::WorkerCrashed,
        ErrorCode::WorkerTimeout,
        ErrorCode::ProtocolError,
    ] {
        let sketches = FakeSketches::new();
        sketches.answer(move |_| Err(Error::new(code, "the fake's worker went away")));
        let engine = engine(&sketches);
        let text = length(true);
        let model = published(&engine, &text);
        match model.outcome(&feature()) {
            Some(FeatureOutcome::Failed(e)) => {
                assert_eq!(e.code(), code);
                assert_eq!(e.feature(), Some(fixture::FEATURE), "{e}");
            }
            other => panic!("{code:?}: {other:?}"),
        }
        // A failure that isn't the document's is set aside: the model holds it, and the
        // format saves around it, the sketch keeping what its file holds.
        assert_eq!(model.fault().map(Error::code), Some(code));
        let formatted = command::format(&model).unwrap();
        assert!(
            formatted.problems.is_empty() && formatted.resolved.is_empty(),
            "{code:?}: {formatted:?}"
        );
        assert_eq!(
            formatted.text,
            silico_core::document::write(&read(text.as_bytes()).unwrap()).unwrap(),
            "{code:?}"
        );
        // Asked again once the worker's back: solved, not the failure kept.
        sketches.answer(|system| Ok(FakeSketches::as_sent(system)));
        let model = published(&engine, &text);
        assert!(
            matches!(model.outcome(&feature()), Some(FeatureOutcome::Sketch(_))),
            "{code:?}: {:?}",
            model.outcome(&feature())
        );
        assert_eq!(sketches.requests().len(), 2, "{code:?}");
    }
}

#[test]
fn a_solver_that_panics_fails_the_rebuild_and_the_engine_still_solves_after() {
    let sketches = FakeSketches::new();
    sketches.answer(|_| panic!("the fake solver panicked"));
    let engine = engine(&sketches);
    let text = length(true);
    match engine
        .start(checked(&text))
        .wait_timeout(Duration::from_secs(20))
    {
        Ok(Err(e)) => {
            assert_eq!(e.code(), ErrorCode::Internal, "{e}");
            assert!(
                e.cause()
                    .is_some_and(|c| c.contains("the fake solver panicked")),
                "{e}"
            );
        }
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("a panicking solve hung the recompute"),
    }
    sketches.answer(|system| Ok(FakeSketches::as_sent(system)));
    let model = published(&engine, &text);
    assert!(matches!(
        model.outcome(&feature()),
        Some(FeatureOutcome::Sketch(_))
    ));
    // And a supersede after it still stops a solve, despite the stopper the panic left.
    sketches.hold();
    let older = engine.start(checked(&length(false)));
    assert!(sketches.wait_held(Duration::from_secs(10)));
    let newer = engine.start(checked(&fixture::constrained("")));
    assert!(matches!(
        older.wait_timeout(Duration::from_secs(10)),
        Ok(Ok(RunOutcome::Superseded))
    ));
    sketches.release();
    assert!(matches!(
        newer.wait_timeout(Duration::from_secs(10)),
        Ok(Ok(RunOutcome::Published(_)))
    ));
}

// --- Superseding and cancelling mid-solve ------------------------------------------------

/// The base sketch with pb at `x`, and `n` copies of it as separate sketches after it.
fn sketches_at(x: u32, n: usize) -> String {
    let points = fixture::POINTS.replace(r#""x": 10, "y": 0"#, &format!(r#""x": {x}, "y": 0"#));
    let fields = fixture::fields(&points, fixture::LINE, fixture::CIRCLE, fixture::ARC, "");
    let features: Vec<String> = (0..n)
        .map(|i| {
            format!(
                "{{\"id\": \"ks{i:06}\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S{i}\", {fields}}}"
            )
        })
        .collect();
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}]}}",
        features.join(", ")
    )
}

/// Each sketch of the published model solved where its document draws pb.
fn solved_as_drawn(model: &Model) {
    for f in &model.document().features {
        let Operation::Sketch(s) = &f.operation else {
            continue;
        };
        let Some(FeatureOutcome::Sketch(solution)) = model.outcome(&f.id) else {
            panic!("{}: {:?}", f.id, model.outcome(&f.id));
        };
        for p in &s.points {
            assert_eq!(
                solution.point(&p.id),
                Some([p.x.mm(), p.y.mm()]),
                "{}: {} is another document's solve",
                f.id,
                p.id
            );
        }
    }
}

#[test]
fn a_superseded_recompute_returns_without_waiting_behind_the_newer_ones_solve() {
    // Found by break-it, fixed: a recompute's sketch thread took the engine's solver (then
    // a Mutex) before it looked at whether it had been superseded, and held it for all its
    // sketches. Superseded before its sketch thread got the solver, by a newer recompute
    // whose sketch thread got there first, it blocked on the Mutex behind the newer one's
    // solve, and its `Run` didn't say Superseded until that solve ended: up to
    // SKETCH_TIMEOUT (300 s) for a real worker. Now the solver is lent under the engine's
    // state lock, and a newer start wakes a thread waiting for it. Here b, a document whose
    // plan takes a moment, is superseded by c at once; c's solve is held, and b's Run must
    // not wait on it.
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    let mut waited = Vec::new();
    for round in 0..5u32 {
        sketches.hold();
        let stops = sketches.stops();
        let a = engine.start(checked(&sketches_at(100 + round, 3)));
        assert!(sketches.wait_held(Duration::from_secs(10)), "{round}");
        let b = engine.start(checked(&sketches_at(200 + round, 300)));
        let c = engine.start(checked(&sketches_at(300 + round, 1)));
        match a.wait_timeout(Duration::from_secs(10)) {
            Ok(Ok(RunOutcome::Superseded)) => {}
            Ok(other) => panic!("{round} a: {other:?}"),
            Err(_) => panic!("{round} a: waited out its solve"),
        }
        assert!(sketches.stops() > stops, "{round}: nothing was stopped");
        match b.wait_timeout(Duration::from_secs(2)) {
            Ok(Ok(RunOutcome::Superseded)) => {}
            Ok(other) => panic!("{round} b: {other:?}"),
            Err(b) => {
                // Waiting behind c's held solve: let c's solve go, and b comes back.
                sketches.release();
                match b.wait_timeout(Duration::from_secs(10)) {
                    Ok(Ok(RunOutcome::Superseded)) => waited.push(round),
                    Ok(other) => panic!("{round} b: {other:?}"),
                    Err(_) => panic!("{round} b: hung even with every solve let go"),
                }
            }
        }
        sketches.release();
        match c.wait_timeout(Duration::from_secs(10)) {
            Ok(Ok(RunOutcome::Published(model))) => solved_as_drawn(&model),
            Ok(other) => panic!("{round} c: {other:?}"),
            Err(_) => panic!("{round} c: didn't finish"),
        }
    }
    assert!(
        waited.is_empty(),
        "superseded, b waited behind the newer recompute's solve in rounds {waited:?}"
    );
}

#[test]
fn recomputes_started_at_random_moments_while_sketches_solve_publish_only_their_own_solves() {
    // Each solve takes a little while; new recomputes land at every point of it (before the
    // solver's taken, between a solve's stopper and its start, as it returns).
    let sketches = FakeSketches::new();
    sketches.answer(|system| {
        let spin = Instant::now();
        let micros = (system.values.len() as u64 * 37) % 400;
        while spin.elapsed() < Duration::from_micros(micros) {
            std::hint::spin_loop();
        }
        Ok(FakeSketches::as_sent(system))
    });
    let engine = engine(&sketches);
    let mut seed: u64 = 0x5eed_1234;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut runs = Vec::new();
    for i in 0..300u32 {
        runs.push(engine.start(checked(&sketches_at(
            1 + (next() % 50) as u32,
            1 + (i % 3) as usize,
        ))));
        let pause = Instant::now();
        let micros = next() % 600;
        while pause.elapsed() < Duration::from_micros(micros) {
            std::hint::spin_loop();
        }
    }
    let last = engine.start(checked(&sketches_at(77, 3)));
    for run in runs {
        match run.wait_timeout(Duration::from_secs(20)) {
            Ok(Ok(RunOutcome::Published(model))) => {
                solved_as_drawn(&model);
            }
            Ok(Ok(RunOutcome::Superseded)) => {}
            Ok(other) => panic!("{other:?}"),
            Err(_) => panic!("a superseded recompute hung"),
        }
    }
    match last.wait_timeout(Duration::from_secs(20)) {
        Ok(Ok(RunOutcome::Published(model))) => solved_as_drawn(&model),
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the newest recompute hung"),
    }
}

#[test]
fn a_cancel_at_the_generations_end_still_stops_the_solve() {
    // The kernel's generation is one short of the end: the recompute takes the last, and a
    // cancel has no newer one to supersede it with.
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    engine.kernel().advance(Generation(u64::MAX - 1));
    sketches.hold();
    let run = engine.start(checked(&length(true)));
    assert!(sketches.wait_held(Duration::from_secs(10)));
    engine.cancel();
    match run.wait_timeout(Duration::from_secs(10)) {
        Ok(Ok(RunOutcome::Superseded)) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the cancelled recompute waited out its solve"),
    }
    assert_eq!(sketches.stops(), 1);
    sketches.release();
    // No generation left: the next start says so, rather than hanging or solving.
    match engine
        .start(checked(&length(true)))
        .wait_timeout(Duration::from_secs(10))
    {
        Ok(Err(e)) => assert_eq!(e.code(), ErrorCode::ResourceExhausted, "{e}"),
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("hung"),
    }
}

// --- Drift at size ------------------------------------------------------------------------

/// A sketch at the gate's limit: 512 points (1,024 unknowns), and 9,000 reference
/// distances between them, each stored 0.
fn at_the_limit() -> String {
    let points: Vec<String> = (0..512)
        .map(|i| point_debug(&format!("p{i:07}"), f64::from(i), f64::from(i % 7)))
        .collect();
    let constraints: Vec<String> = (0..9000)
        .map(|i| {
            format!(
                r#"{{"id": "r{i:07}", "type": "distance", "a": "p{:07}", "b": "p{:07}", "value": 0, "driving": false}}"#,
                i % 512,
                (i * 7 + 1) % 512
            )
        })
        .collect();
    fixture::document(&fixture::fields(
        &points.join(", "),
        "",
        "",
        "",
        &constraints.join(", "),
    ))
}

#[test]
fn a_sketch_drift_lists_five_moved_and_says_more_only_past_five() {
    // `n` points on the x axis, each moved half a unit along it by the solve: the message
    // names the first five, and says there are more (`, …`) only when there are.
    for n in [5u32, 6] {
        let points: Vec<String> = (0..n)
            .map(|i| point_debug(&format!("p{i:07}"), f64::from(i), 0.0))
            .collect();
        let sketches = FakeSketches::new();
        sketches.answer(move |system| {
            let mut solved = FakeSketches::as_sent(system);
            // To its whole number and a half, which a load of the solution leaves there.
            for i in 0..n as usize {
                let x = &mut solved.values[FIRST_POINT + 2 * i];
                *x = x.floor() + 0.5;
            }
            Ok(solved)
        });
        // A reference to solve for, which the move leaves as it was (both points move alike).
        let reference = r#"{"id": "r0000000", "type": "distance", "a": "p0000000", "b": "p0000001", "value": 1, "driving": false}"#;
        let text = fixture::document(&fixture::fields(&points.join(", "), "", "", "", reference));
        let errors = published(&engine(&sketches), &text).drift();
        let [e] = &errors[..] else {
            panic!("{n}: {errors:?}")
        };
        assert_eq!(e.elements_total(), u64::from(n));
        let listed = "p0000000 x 0 → 0.5, p0000001 x 1 → 1.5, p0000002 x 2 → 2.5, \
                      p0000003 x 3 → 3.5, p0000004 x 4 → 4.5";
        let end = if n == 5 {
            format!("{listed} (5 in all)")
        } else {
            format!("{listed}, … (6 in all)")
        };
        assert!(e.message().ends_with(&end), "{n}: {e}");
        assert!(!e.message().contains("p0000005 x"), "{n}: {e}");
    }
}

#[test]
fn a_sketch_at_the_limit_with_everything_moved_reports_drift_and_saves_promptly() {
    let sketches = FakeSketches::new();
    sketches.answer(|system| {
        // Each coordinate to its whole number and a half: every point moves from where
        // it's drawn, and a load of the solution leaves it there, as a solve must
        // (Translation::solve keeps only a solution its own file gives back).
        let mut solved = FakeSketches::as_sent(system);
        for v in &mut solved.values[FIRST_POINT..FIRST_POINT + 1024] {
            *v = v.floor() + 0.5;
        }
        Ok(solved)
    });
    let engine = engine(&sketches);
    let text = at_the_limit();
    let start = Instant::now();
    let model = published(&engine, &text);
    let errors = model.drift();
    let [e] = &errors[..] else {
        panic!("{errors:?}")
    };
    assert_eq!(e.code(), ErrorCode::Drift);
    // Every point moved (512) and every reference measures more than its stored 0, but
    // those whose two points coincide (i % 512 == (7i + 1) % 512 never holds here).
    assert_eq!(e.elements_total(), 512 + 9000);
    let head: String = e.message().chars().take(300).collect();
    assert!(
        e.message().contains(", …") && e.message().contains("(9512 in all)"),
        "{head}"
    );
    // The message names a few, not all ten thousand.
    assert!(e.message().len() < 1000, "{} bytes", e.message().len());
    let saved = command::format(&model).unwrap();
    assert_eq!(saved.resolved, [feature()]);
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    holds_as_saved(&saved.text);
}

#[test]
fn a_solve_past_a_limit_not_a_size_keeps_the_solves_own_advice() {
    // The gate's advice is carried into an unsavable solve's only when the problem is a
    // curve too small (the floor); any other limit keeps the solve's own advice. An arc
    // about pa000000 from (1000, 0) to (0, 1000), its centre solved to (−999001, 0): every
    // coordinate within a kilometre, but the radius, centre to start, is 1000 + 999001 =
    // 1000001 mm, past the kilometre the solver takes. An invalid result naming the arc,
    // with "keep the sketch's dimensions inside their limits", not "change the constraints
    // that draw it this small".
    let doc = fixture::document(&fixture::fields(
        &[
            fixture::point("pa000000", 0.0, 0.0),
            fixture::point("pa000001", 1000.0, 0.0),
            fixture::point("pa000002", 0.0, 1000.0),
        ]
        .join(", "),
        "",
        "",
        &fixture::arc("aa000001", "pa000000", "pa000001", "pa000002"),
        "",
    ));
    let sketches = FakeSketches::new();
    sketches.answer(move |system: &SketchSystem| {
        let mut solved = FakeSketches::as_sent(system);
        // The centre's x, the first point's first value.
        solved.values[FIRST_POINT] = -999_001.0;
        Ok(solved)
    });
    let model = published(&engine(&sketches), &doc);
    let Some(FeatureOutcome::Failed(error)) = model.outcome(&feature()) else {
        panic!("{:?}", model.outcome(&feature()));
    };
    assert_eq!(error.code(), ErrorCode::InvalidResult);
    assert_eq!(error.elements(), ["aa000001"]);
    assert_eq!(
        error.message(),
        "the sketch solves to geometry its file can't hold: arc aa000001's radius, from its center to its start, is 1000001 mm, more than the 1000000 mm (a kilometre) the solver takes"
    );
    assert_eq!(
        error.suggestion(),
        Some(
            "keep the sketch's dimensions inside their limits, not at them: a solve meets them only to rounding"
        )
    );
}
