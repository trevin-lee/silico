//! M3.4: a sketch's file holds its last solve (docs/design/m3.4-stored-coordinates.md).
//! Every rebuild solves every sketch from the coordinates its file stores; a solve that
//! differs from them is drift, named by sketch and element; a save writes the solve back.
//! On the fake kernel and the fake sketch solver, whose answers are made here, so each
//! case is reached on purpose; the real solver's side is silico-cli's tests/commands.rs
//! (a hand edit through `silico check` and `fmt`) and silico-sketch's tests/fixed_point.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/command.rs"]
mod commands;
#[path = "common/sketch.rs"]
mod fixture;

use std::time::Duration;

use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{ErrorCode, command};
use silico_protocol::{SketchSystem, SolveStatus, Solved};

// The sent values: the origin and the axes' ends (0..6); the points by ID, pa (6, 7),
// pb (8, 9), pc (10, 11); the circle ca's radius (12); the arc aa's radius and angles
// (13..16); then each driving dimension's value.
const PB_X: usize = 8;
const PA_X: usize = 6;
const CA_RADIUS: usize = 12;

fn engine(sketches: &FakeSketches) -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(sketches.clone())
}

fn checked(text: &str) -> Checked {
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

fn model(engine: &Recompute<FakeKernel>, text: &str) -> std::sync::Arc<Model> {
    match engine
        .start(checked(text))
        .wait_timeout(Duration::from_secs(20))
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

fn feature() -> FeatureId {
    FeatureId::new(fixture::FEATURE).unwrap()
}

/// Answers each sketch as sent, with `change` made to its values.
fn moving(sketches: &FakeSketches, change: impl Fn(&mut Vec<f64>) + Send + Sync + 'static) {
    sketches.answer(move |system: &SketchSystem| {
        let mut solved = FakeSketches::as_sent(system);
        change(&mut solved.values);
        Ok(solved)
    });
}

/// The base sketch with these constraints, `pb` at `pb_x`.
fn sketch(pb_x: &str, constraints: &str) -> String {
    let points = fixture::POINTS.replace(r#""x": 10, "y": 0"#, &format!(r#""x": {pb_x}, "y": 0"#));
    fixture::document(&fixture::fields(
        &points,
        fixture::LINE,
        fixture::CIRCLE,
        fixture::ARC,
        constraints,
    ))
}

const HORIZONTAL: &str = r#"{"id": "k0000001", "type": "horizontal", "line": "na000000"}"#;

#[test]
fn an_unchanged_sketch_is_solved_once_and_an_edited_one_again() {
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    let text = sketch("10", HORIZONTAL);
    let first = model(&engine, &text);
    assert!(matches!(
        first.outcome(&feature()),
        Some(FeatureOutcome::Sketch(_))
    ));
    model(&engine, &text);
    assert_eq!(sketches.requests().len(), 1, "solved again unchanged");
    // A coordinate edited by hand is a new start: solved again.
    model(&engine, &sketch("11", HORIZONTAL));
    assert_eq!(sketches.requests().len(), 2);
    // And going back takes the first solve, kept.
    model(&engine, &text);
    assert_eq!(sketches.requests().len(), 2);
}

#[test]
fn a_sketch_that_doesnt_solve_fails_its_feature_and_isnt_drift() {
    let sketches = FakeSketches::new();
    sketches.answer(|system| {
        Ok(Solved {
            status: SolveStatus::Failed,
            algorithm: None,
            ..FakeSketches::as_sent(system)
        })
    });
    let engine = engine(&sketches);
    let model = model(&engine, &sketch("10", HORIZONTAL));
    match model.outcome(&feature()) {
        Some(FeatureOutcome::Failed(e)) => {
            assert_eq!(e.code(), ErrorCode::SolveFailed, "{e}");
            assert_eq!(e.feature(), Some(fixture::FEATURE));
        }
        other => panic!("{other:?}"),
    }
    assert!(model.drift().is_empty());
    let problems = command::check(&model).unwrap();
    assert_eq!(problems.errors().len(), 1);
    assert_eq!(problems.errors()[0].code(), ErrorCode::SolveFailed);
    // A save keeps what the file holds.
    let saved = command::format(&model).unwrap().unwrap();
    assert!(saved.resolved.is_empty());
    assert_eq!(
        read(saved.text.as_bytes()).unwrap(),
        read(sketch("10", HORIZONTAL).as_bytes()).unwrap()
    );
    // The failure follows from the sketch: kept, not asked again.
    model_again(&engine);
    assert_eq!(sketches.requests().len(), 1);
}

fn model_again(engine: &Recompute<FakeKernel>) {
    model(engine, &sketch("10", HORIZONTAL));
}

#[test]
fn a_recompute_superseded_mid_solve_stops_its_solve_and_the_newer_one_solves() {
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    sketches.hold();
    let older = engine.start(checked(&sketch("10", HORIZONTAL)));
    assert!(sketches.wait_held(Duration::from_secs(10)));
    let newer = engine.start(checked(&sketch("11", HORIZONTAL)));
    // The older one's solve is stopped at once, not waited for.
    match older.wait_timeout(Duration::from_secs(10)) {
        Ok(Ok(RunOutcome::Superseded)) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the superseded recompute waited out its solve"),
    }
    assert_eq!(sketches.stops(), 1);
    sketches.release();
    match newer.wait_timeout(Duration::from_secs(10)) {
        Ok(Ok(RunOutcome::Published(model))) => assert!(matches!(
            model.outcome(&feature()),
            Some(FeatureOutcome::Sketch(_))
        )),
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the newer recompute didn't finish"),
    }
    // Nothing of the stopped solve was kept: the older sketch is solved when asked again.
    let before = sketches.requests().len();
    model(&engine, &sketch("10", HORIZONTAL));
    assert_eq!(sketches.requests().len(), before + 1);
}

#[test]
fn a_cancel_mid_solve_stops_the_solve() {
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    sketches.hold();
    let run = engine.start(checked(&sketch("10", HORIZONTAL)));
    assert!(sketches.wait_held(Duration::from_secs(10)));
    engine.cancel();
    match run.wait_timeout(Duration::from_secs(10)) {
        Ok(Ok(RunOutcome::Superseded)) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the cancelled recompute waited out its solve"),
    }
    assert_eq!(sketches.stops(), 1);
    assert!(engine.published().is_none());
}

/// The drift of the base sketch with `constraints`, its solve changed by `change`.
fn drift(
    constraints: &str,
    change: impl Fn(&mut Vec<f64>) + Send + Sync + 'static,
) -> Vec<silico_core::Error> {
    let sketches = FakeSketches::new();
    moving(&sketches, change);
    model(&engine(&sketches), &sketch("10", constraints)).drift()
}

#[test]
fn a_solve_that_moves_a_point_or_a_radius_is_drift_naming_each() {
    let errors = drift(HORIZONTAL, |v| {
        v[PB_X] = 11.5;
        v[CA_RADIUS] = 6.0;
    });
    let [e] = &errors[..] else {
        panic!("{errors:?}")
    };
    assert_eq!(e.code(), ErrorCode::Drift);
    assert_eq!(e.feature(), Some(fixture::FEATURE));
    assert_eq!(e.elements(), ["pb000000", "ca000000"]);
    assert_eq!(e.parameter(), Some("pb000000"));
    assert!(
        e.message().contains("pb000000 x 10 → 11.5")
            && e.message().contains("ca000000 radius 5 → 6")
            && e.message().contains("(2 in all)"),
        "{e}"
    );
    assert!(
        e.suggestion().unwrap().contains("silico fmt"),
        "{:?}",
        e.suggestion()
    );
}

#[test]
fn a_coordinate_solved_to_minus_zero_from_zero_is_drift_as_the_file_writes_them_apart() {
    let errors = drift(HORIZONTAL, |v| v[PA_X] = -0.0);
    let [e] = &errors[..] else {
        panic!("{errors:?}")
    };
    assert_eq!(e.elements(), ["pa000000"]);
    assert!(e.message().contains("pa000000 x 0 → -0"), "{e}");
}

#[test]
fn a_solve_where_the_file_has_it_is_no_drift() {
    assert!(drift(HORIZONTAL, |_| {}).is_empty());
}

#[test]
fn a_reference_dimension_stored_otherwise_than_measured_is_drift_and_a_save_writes_the_measure() {
    // na is 10 long; its reference length stored 3. A reference angle from na (along x) to
    // nb (along y) is 90°, stored 0.
    let constraints = format!(
        r#"{HORIZONTAL}, {{"id": "r0000001", "type": "length", "line": "na000000", "value": 3, "driving": false}}, {{"id": "r0000002", "type": "angle", "a": "na000000", "b": "nb000000", "value": 0, "driving": false}}"#
    );
    let lines = format!(
        r#"{}, {{"id": "nb000000", "from": "pa000000", "to": "pc000000", "construction": false}}"#,
        fixture::LINE
    );
    let text = fixture::document(&fixture::fields(
        fixture::POINTS,
        &lines,
        fixture::CIRCLE,
        fixture::ARC,
        &constraints,
    ));
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    let model = model(&engine, &text);
    let errors = model.drift();
    let [e] = &errors[..] else {
        panic!("{errors:?}")
    };
    assert_eq!(e.elements(), ["r0000001", "r0000002"]);
    assert!(
        e.message().contains("r0000001 value 3 → 10")
            && e.message().contains("r0000002 value 0 → 90"),
        "{e}"
    );
    let formatted = command::format(&model).unwrap().unwrap();
    assert_eq!(formatted.resolved, [feature()]);
    assert!(
        formatted.text.contains(r#""value": 10, "driving": false"#)
            && formatted.text.contains(r#""value": 90, "driving": false"#),
        "{}",
        formatted.text
    );
    // Saved, it's what its solve gives: no drift, and the same bytes again.
    let again = self::model(&engine, &formatted.text);
    assert!(again.drift().is_empty(), "{:?}", again.drift());
    assert_eq!(
        command::format(&again).unwrap().unwrap().text,
        formatted.text
    );
}

#[test]
fn a_reference_measure_is_compared_as_the_file_writes_it_to_the_nanometre() {
    // pb at 10 + 0.4 nm: the length measures 10.0000004, written 10 (to the nanometre):
    // a reference stored 10 holds. At 10 + 1 nm it measures 10.000001: drift.
    let reference = r#"{"id": "r0000001", "type": "length", "line": "na000000", "value": 10, "driving": false}"#;
    let constraints = format!("{HORIZONTAL}, {reference}");
    for (pb_x, drifts) in [("10.0000004", false), ("10.000001", true)] {
        let sketches = FakeSketches::new();
        let model = model(&engine(&sketches), &sketch(pb_x, &constraints));
        let errors = model.drift();
        assert_eq!(!errors.is_empty(), drifts, "{pb_x}: {errors:?}");
        if drifts {
            assert_eq!(errors[0].elements(), ["r0000001"], "{pb_x}");
            assert!(
                errors[0]
                    .message()
                    .contains("r0000001 value 10 → 10.000001"),
                "{}",
                errors[0]
            );
        }
    }
}

#[test]
fn a_save_writes_the_solve_and_says_which_sketches_it_rewrote() {
    let sketches = FakeSketches::new();
    moving(&sketches, |v| v[PB_X] = 12.25);
    let engine = engine(&sketches);
    let formatted = commands::format_bytes(&engine, sketch("10", HORIZONTAL).as_bytes())
        .unwrap()
        .unwrap();
    assert_eq!(formatted.resolved, [feature()]);
    assert!(formatted.problems.is_empty());
    assert_eq!(
        read(formatted.text.as_bytes()).unwrap(),
        read(sketch("12.25", HORIZONTAL).as_bytes()).unwrap()
    );
    // check reports the drift a save fixes.
    let problems = commands::check_bytes(&engine, sketch("10", HORIZONTAL).as_bytes()).unwrap();
    assert_eq!(problems.errors().len(), 1);
    assert_eq!(problems.errors()[0].code(), ErrorCode::Drift);
}

#[test]
fn a_solve_that_moves_only_a_y_is_drift() {
    // Found by the council's tests lens: every move above was an x, so drift that compared
    // x alone passed them.
    const PC_Y: usize = 11; // pc (0, 10) solved to (0, 10.5)
    let errors = drift(HORIZONTAL, |v| v[PC_Y] = 10.5);
    let [e] = &errors[..] else {
        panic!("a y-only move isn't drift: {errors:?}")
    };
    assert_eq!(e.elements(), ["pc000000"]);
    assert!(e.message().contains("pc000000 y 10 → 10.5"), "{e}");
    let sketches = FakeSketches::new();
    moving(&sketches, |v| v[PC_Y] = 10.5);
    let formatted = commands::format_bytes(&engine(&sketches), sketch("10", HORIZONTAL).as_bytes())
        .unwrap()
        .unwrap();
    assert_eq!(formatted.resolved, [feature()]);
}

#[test]
fn every_reference_kind_stored_otherwise_is_drift_and_saved_as_measured() {
    // Found by the council's tests lens: only length, distance, and angle references were
    // written back under test. On the base sketch: ca's radius is 5 (diameter 10); pa → pb
    // is 10 in x, pa → pc 10 in y; pc (0, 10) is 10 left of na (pa → pb, along +x), stored
    // 10 to its right.
    let constraints = format!(
        r#"{HORIZONTAL}, {{"id": "r0000001", "type": "radius", "curve": "ca000000", "value": 1, "driving": false}}, {{"id": "r0000002", "type": "diameter", "curve": "ca000000", "value": 1, "driving": false}}, {{"id": "r0000003", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": 1, "driving": false}}, {{"id": "r0000004", "type": "distance_y", "a": "pa000000", "b": "pc000000", "value": 1, "driving": false}}, {{"id": "r0000005", "type": "point_line_distance", "point": "pc000000", "line": "na000000", "value": 10, "side": "right", "driving": false}}"#
    );
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    let model = model(&engine, &sketch("10", &constraints));
    let errors = model.drift();
    let [e] = &errors[..] else {
        panic!("{errors:?}")
    };
    assert_eq!(
        e.elements(),
        ["r0000001", "r0000002", "r0000003", "r0000004", "r0000005"],
        "{e}"
    );
    let formatted = command::format(&model).unwrap().unwrap();
    for want in [
        r#""id": "r0000001", "type": "radius", "curve": "ca000000", "value": 5,"#,
        r#""id": "r0000002", "type": "diameter", "curve": "ca000000", "value": 10,"#,
        r#""id": "r0000003", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": 10,"#,
        r#""id": "r0000004", "type": "distance_y", "a": "pa000000", "b": "pc000000", "value": 10,"#,
        r#""value": 10, "side": "left""#,
    ] {
        assert!(formatted.text.contains(want), "{want}\n{}", formatted.text);
    }
}

/// The fake, but readying it for its first solve fails as readying a started sketch worker
/// can: its executable changed since it started (`worker_crashed`, already the solver's
/// fault). A worker that can't be found at all fails earlier, starting: the starter's test.
struct FailsToStartOnce {
    sketches: FakeSketches,
    failed: bool,
}

impl silico_core::sketch::SketchSolver for FailsToStartOnce {
    fn solve(&mut self, system: SketchSystem) -> Result<Solved, silico_core::Error> {
        self.sketches.solve(system)
    }

    fn stopper(&mut self) -> Result<Option<silico_core::sketch::SolveStopper>, silico_core::Error> {
        if !self.failed {
            self.failed = true;
            return Err(silico_core::Error::new(
                ErrorCode::WorkerCrashed,
                "the sketch worker /bin/worker changed since it started",
            ));
        }
        self.sketches.stopper()
    }
}

#[test]
fn a_solver_that_couldnt_be_readied_is_tried_again_and_its_fault_passed_on_as_it_is() {
    // Found by the council's robustness lens: a solver that couldn't be readied failed
    // every sketch with an error kept with its key, so an unchanged sketch stayed failed
    // for the engine's life. What failed isn't the sketch's doing: not kept. A failure
    // already the solver's (a worker fault) is passed on as it is, not rewrapped (round 3's
    // tests lens: rewrapping every failure passed).
    let sketches = FakeSketches::new();
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FailsToStartOnce {
        sketches: sketches.clone(),
        failed: false,
    });
    let text = sketch("10", HORIZONTAL);
    match model(&engine, &text).outcome(&feature()) {
        Some(FeatureOutcome::Failed(e)) => {
            assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
            assert_eq!(
                e.message(),
                "the sketch worker /bin/worker changed since it started"
            );
            assert_eq!(e.cause(), None);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        model(&engine, &text).outcome(&feature()),
        Some(FeatureOutcome::Sketch(_))
    ));
    assert_eq!(sketches.requests().len(), 1);
}

#[test]
fn a_superseded_solve_runs_its_grace_before_its_stopped() {
    // Found by the council's architecture lens: a newer recompute killed the older one's
    // solve at once, against ARCHITECTURE.md's rule that superseded work is stopped only
    // once it has run longer than replacing its worker takes (unless it finishes first).
    // The stop now comes SKETCH_STOP_AFTER after the solve started, never sooner.
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    sketches.hold();
    let older = engine.start(checked(&sketch("10", HORIZONTAL)));
    assert!(sketches.wait_held(Duration::from_secs(10)));
    let newer = engine.start(checked(&sketch("11", HORIZONTAL)));
    assert!(matches!(
        older.wait_timeout(Duration::from_secs(10)),
        Ok(Ok(RunOutcome::Superseded))
    ));
    let waits = sketches.stop_waits();
    assert_eq!(waits.len(), 1, "{waits:?}");
    // Its grace, and not much more (round 2's tests lens: a stop seconds late passed): a
    // generous ceiling, for a loaded machine.
    assert!(
        waits[0] >= silico_core::recompute::SKETCH_STOP_AFTER
            && waits[0] < silico_core::recompute::SKETCH_STOP_AFTER + Duration::from_millis(250),
        "stopped {:?} after it started",
        waits[0]
    );
    sketches.release();
    assert!(matches!(
        newer.wait_timeout(Duration::from_secs(10)),
        Ok(Ok(RunOutcome::Published(_)))
    ));
}

#[test]
fn a_sketch_worker_that_cant_be_started_is_the_solvers_fault_and_started_again_next_time() {
    // Round 2's architecture and quality lenses: a missing or misnamed worker comes as
    // invalid_input from finding it, which blamed the sketch (check exited 1, fmt wrote the
    // file). It's the solver's fault, as a kernel's failure to start is: check fails as the
    // client's own failure. And it isn't kept (round 2's tests lens: the start path itself
    // was untested): the next recompute starts the solver again.
    let sketches = FakeSketches::new();
    let mut tries = 0;
    let given = sketches.clone();
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketch_starter(move || {
        tries += 1;
        if tries == 1 {
            Err(silico_core::Error::new(
                ErrorCode::InvalidInput,
                "SILICO_SKETCH_WORKER names /nowhere, which isn't a file",
            )
            .with_suggestion("name the sketch worker's executable"))
        } else {
            Ok(Box::new(given.clone()) as Box<dyn silico_core::sketch::SketchSolver>)
        }
    });
    let text = sketch("10", HORIZONTAL);
    let first = model(&engine, &text);
    match first.outcome(&feature()) {
        Some(FeatureOutcome::Failed(e)) => {
            assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
            assert!(e.code().is_kernel_fault());
            assert_eq!(
                e.cause(),
                Some("SILICO_SKETCH_WORKER names /nowhere, which isn't a file")
            );
            assert_eq!(e.suggestion(), Some("name the sketch worker's executable"));
        }
        other => panic!("{other:?}"),
    }
    let fault = command::check(&first).unwrap_err();
    assert_eq!(fault.code(), ErrorCode::WorkerCrashed);
    assert!(command::format(&first).is_err(), "nothing is written");
    assert!(matches!(
        model(&engine, &text).outcome(&feature()),
        Some(FeatureOutcome::Sketch(_))
    ));
    assert_eq!(sketches.requests().len(), 1);
}

/// Solve 1 waits until let go; solve 2 runs 300 ms unless stopped; each stop recorded by
/// solve number (round 2's tests lens), and when each solve began.
#[derive(Clone)]
struct Gated {
    count: std::sync::Arc<std::sync::Mutex<u64>>,
    began: std::sync::Arc<std::sync::Mutex<Vec<std::time::Instant>>>,
    stopped: std::sync::Arc<std::sync::Mutex<Vec<u64>>>,
    entered: std::sync::Arc<std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    go: std::sync::Arc<std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>>,
}

impl silico_core::sketch::SketchSolver for Gated {
    fn solve(&mut self, system: SketchSystem) -> Result<Solved, silico_core::Error> {
        let n = {
            let mut c = self.count.lock().unwrap();
            *c += 1;
            *c
        };
        self.began.lock().unwrap().push(std::time::Instant::now());
        if n == 1 {
            self.entered
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            self.go.lock().unwrap().take().unwrap().recv().unwrap();
        } else if n == 2 {
            let until = std::time::Instant::now() + Duration::from_millis(300);
            while std::time::Instant::now() < until {
                if self.stopped.lock().unwrap().contains(&2) {
                    return Err(silico_core::Error::new(ErrorCode::WorkerCrashed, "stopped"));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(FakeSketches::as_sent(&system))
    }

    fn stopper(&mut self) -> Result<Option<silico_core::sketch::SolveStopper>, silico_core::Error> {
        let n = *self.count.lock().unwrap() + 1;
        let stopped = std::sync::Arc::clone(&self.stopped);
        Ok(Some(silico_core::sketch::SolveStopper::new(move || {
            stopped.lock().unwrap().push(n)
        })))
    }
}

#[test]
fn a_late_stop_of_a_finished_solve_spares_the_next_solve() {
    // Round 2's tests lens: a stop meant for an older solve that has finished (its timer
    // still pending) must not stop the newer recompute's solve; only a randomized test
    // covered it. The stop comes due SKETCH_STOP_AFTER after the older solve began, so it
    // tests the newer solve only if that has begun by then: under load (the mutants
    // shards'), it can begin later, and the stop finds nothing to spare. So the race is run
    // again until the newer solve begins within half that, up to 50 times (M3's milestone
    // mutants: numbering every solve alike, `solves *= 1`, passed on a loaded shard).
    for attempt in 1..=50 {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel();
        let gated = Gated {
            count: Default::default(),
            began: Default::default(),
            stopped: Default::default(),
            entered: std::sync::Arc::new(std::sync::Mutex::new(Some(entered_tx))),
            go: std::sync::Arc::new(std::sync::Mutex::new(Some(go_rx))),
        };
        let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(gated.clone());
        let older = engine.start(checked(&sketch("10", HORIZONTAL)));
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let newer = engine.start(checked(&sketch("11", HORIZONTAL))); // within the grace
        go_tx.send(()).unwrap();
        assert!(matches!(
            older.wait_timeout(Duration::from_secs(10)),
            Ok(Ok(RunOutcome::Superseded))
        ));
        match newer.wait_timeout(Duration::from_secs(10)) {
            Ok(Ok(RunOutcome::Published(m))) => assert!(
                matches!(m.outcome(&feature()), Some(FeatureOutcome::Sketch(_))),
                "attempt {attempt}: {:?}",
                m.outcome(&feature())
            ),
            Ok(other) => panic!("{other:?}"),
            Err(_) => panic!("newer didn't finish"),
        }
        assert!(!gated.stopped.lock().unwrap().contains(&2));
        let began = gated.began.lock().unwrap().clone();
        if began[1] - began[0] < silico_core::recompute::SKETCH_STOP_AFTER / 2 {
            return;
        }
    }
}

#[test]
fn a_dropped_engine_stops_its_solve_before_the_drop_returns() {
    // Round 3's architecture and tests lenses: a cancel stops a superseded solve as
    // ARCHITECTURE.md has superseded work stopped, after its grace; a dropped engine stops
    // it at once, since nothing is left to stop it later and the process may be ending.
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    sketches.hold();
    let run = engine.start(checked(&sketch("10", HORIZONTAL)));
    assert!(sketches.wait_held(Duration::from_secs(10)));
    drop(engine);
    // Synchronously: the grace's stop would come later, on a timer thread. (Not timed
    // from the solve's start: on a loaded machine the drop itself can come 10 ms after.)
    // `stop_waits` is written by the stop itself; `stops` by the solve once it wakes.
    assert_eq!(
        sketches.stop_waits().len(),
        1,
        "stopped by the time the drop returned"
    );
    assert!(matches!(
        run.wait_timeout(Duration::from_secs(10)),
        Ok(Ok(RunOutcome::Superseded))
    ));
}

#[test]
fn a_cancelled_solve_runs_its_grace_before_its_stopped() {
    // Round 4's tests lens: a cancel stops a solve as superseded work is stopped, after its
    // grace (ARCHITECTURE.md); stopping it at once passed every test. Its grace, and not
    // much more, as the start path's test bounds it.
    let sketches = FakeSketches::new();
    let engine = engine(&sketches);
    sketches.hold();
    let run = engine.start(checked(&sketch("10", HORIZONTAL)));
    assert!(sketches.wait_held(Duration::from_secs(10)));
    engine.cancel();
    assert!(matches!(
        run.wait_timeout(Duration::from_secs(10)),
        Ok(Ok(RunOutcome::Superseded))
    ));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while sketches.stop_waits().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let waits = sketches.stop_waits();
    assert_eq!(waits.len(), 1, "{waits:?}");
    assert!(
        waits[0] >= silico_core::recompute::SKETCH_STOP_AFTER
            && waits[0] < silico_core::recompute::SKETCH_STOP_AFTER + Duration::from_millis(250),
        "stopped {:?} after it started",
        waits[0]
    );
    sketches.release();
}
