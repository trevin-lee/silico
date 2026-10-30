//! `Sessions::sketch` (docs/design/m5.0-app.md, section 7, *`sketch` (M5.10)*) on the fake
//! sketch solver, where the solve itself isn't the point (silico-sketch's
//! tests/sketch_read.rs reads sketches PlaneGCS solves): the read's solve stopped by a cancel
//! and its solver given back, a published read among the session's reads, and a solver that
//! fails or can't start refusing the read with its own error, naming the sketch.

use crate::sessions::{HANG, answer, id};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use silico_core::cache::Cache;
use silico_core::command::{Cancelled, ModelChoice, ReadCommand, SessionCommand, Sessions};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};

/// A sketch `k7f2qx3m` on xy: a free line.
const LINE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 3, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}]}"#;

/// Sessions on a fake kernel, every sketch solved by `start`'s solver, and [`LINE`] open
/// in the first, its rebuild done.
fn opened(
    dir: &tempfile::TempDir,
    start: impl Fn() -> Result<Box<dyn SketchSolver>, Error> + Send + Sync + 'static,
) -> Sessions<Arc<FakeKernel>> {
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = Sessions::with_stand_ins(
        kernel,
        |cache: &std::path::Path| Cache::unsealed(cache),
        start,
    );
    let path = dir.path().join("line.silico");
    std::fs::write(&path, LINE).unwrap();
    let opened = answer(sessions.open(path, false));
    assert!(opened.result.is_ok(), "{:?}", opened.result);
    answer(sessions.check(
        &opened.session.unwrap().id,
        silico_core::command::CheckModel::Current { fresh: false },
    ));
    sessions
}

fn s1() -> silico_core::command::SessionId {
    silico_core::command::SessionId::parse("s1").unwrap()
}

#[test]
fn a_cancelled_sketch_read_stops_its_solve_and_gives_its_solver_back() {
    // Held mid-solve, a published read is among the session's reads by its name; cancelled,
    // its solve is stopped (the fake counts the stop), it answers cancelled, and the
    // solver goes back to the sessions' solvers: the next read, released, answers, solved
    // on it (no other was started).
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let started = Arc::clone(&starts);
    let sessions = opened(&dir, move || {
        started.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    sketches.hold();
    let read = sessions.sketch(&s1(), ModelChoice::Published, id("k7f2qx3m"));
    assert!(sketches.wait_held(HANG));
    let status = sessions.status(&s1()).result.unwrap();
    assert_eq!(status.reads, [ReadCommand::Sketch]);
    assert_eq!(
        sessions
            .cancel(&s1(), SessionCommand::Sketch)
            .result
            .unwrap(),
        Cancelled {
            command: SessionCommand::Sketch,
            stopped: true
        }
    );
    let cancelled = answer(read);
    let errors = cancelled.result.unwrap_err();
    assert_eq!(errors.errors()[0].code(), ErrorCode::Cancelled);
    assert_eq!(sketches.stops(), 1);
    sketches.release();
    let again = answer(sessions.sketch(&s1(), ModelChoice::Current, id("k7f2qx3m")));
    let read = again.result.unwrap();
    assert_eq!(
        read.states.dofs, 0,
        "the fake's dofs, as it answers every solve"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    // A stop the read let go of stops nothing once its solve is over: a cancel now finds
    // nothing running, and no solve after it is stopped.
    assert!(
        sessions
            .cancel(&s1(), SessionCommand::Sketch)
            .result
            .is_err()
    );
    answer(sessions.sketch(&s1(), ModelChoice::Current, id("k7f2qx3m")))
        .result
        .unwrap();
    assert_eq!(sketches.stops(), 1);
}

#[test]
fn a_solver_that_fails_refuses_the_read_with_its_error_naming_the_sketch_and_isnt_lost() {
    // The worker's own failure says nothing of the sketch's state: the read is refused with
    // it (`sketch_states::states`), naming the sketch, and the solver given back serves the
    // next read once it answers again.
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let sessions = opened(&dir, move || {
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    let failing = Arc::new(AtomicBool::new(true));
    let fails = Arc::clone(&failing);
    sketches.answer(move |system| {
        if fails.load(Ordering::SeqCst) {
            Err(Error::new(
                ErrorCode::WorkerCrashed,
                "the sketch worker crashed",
            ))
        } else {
            Ok(FakeSketches::as_sent(system))
        }
    });
    let refused = answer(sessions.sketch(&s1(), ModelChoice::Current, id("k7f2qx3m")));
    let problems = refused.result.unwrap_err();
    assert_eq!(problems.total(), 1);
    let error = &problems.errors()[0];
    assert_eq!(
        (error.code(), error.message(), error.feature()),
        (
            ErrorCode::WorkerCrashed,
            "the sketch worker crashed",
            Some("k7f2qx3m")
        )
    );
    failing.store(false, Ordering::SeqCst);
    let read = answer(sessions.sketch(&s1(), ModelChoice::Current, id("k7f2qx3m")))
        .result
        .unwrap();
    assert_eq!(read.members.points.len(), 2);
}

#[test]
fn a_solver_that_cant_start_refuses_the_read_with_its_error_naming_the_sketch() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = opened(&dir, || {
        Err(Error::new(
            ErrorCode::WorkerUnavailable,
            "no sketch worker at /nowhere",
        ))
    });
    let refused = answer(sessions.sketch(&s1(), ModelChoice::Current, id("k7f2qx3m")));
    let problems = refused.result.unwrap_err();
    let error = &problems.errors()[0];
    assert_eq!(
        (error.code(), error.message(), error.feature()),
        (
            ErrorCode::WorkerUnavailable,
            "no sketch worker at /nowhere",
            Some("k7f2qx3m")
        )
    );
    assert_eq!(problems.total(), 1);
}

#[test]
fn a_session_closed_under_a_published_sketch_read_stops_its_solve() {
    // The session's close interrupts its published reads: the read held mid-solve is
    // stopped, and answers as of a session that closed, never cancelled (no client stopped
    // it).
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let sessions = opened(&dir, move || {
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    sketches.hold();
    let read = sessions.sketch(&s1(), ModelChoice::Published, id("k7f2qx3m"));
    assert!(sketches.wait_held(HANG));
    assert!(answer(sessions.close(&s1())).result.is_ok());
    let closed = answer(read);
    let problems = closed.result.unwrap_err();
    assert_ne!(problems.errors()[0].code(), ErrorCode::Cancelled);
    assert_eq!(sketches.stops(), 1);
    sketches.release();
}
