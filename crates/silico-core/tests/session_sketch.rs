//! `Sessions::sketch` (docs/design/m5.0-app.md, section 7, *`sketch` (M5.10)*) on the fake
//! sketch solver, where the solve itself isn't the point (silico-sketch's
//! tests/sketch_read.rs reads sketches PlaneGCS solves): the read asks no solver anything,
//! its model holding each sketch's diagnosis (M5.0's A8: "`sketch` ... need[s] none"); and a
//! rebuild whose solver failed, or couldn't start, refuses the read with the rebuild's own
//! error, as `check` reports it.

use crate::sessions::{answer, id};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use silico_core::cache::Cache;
use silico_core::command::{ModelChoice, Sessions};
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
fn a_sketch_read_asks_no_solver_anything_current_or_published() {
    // The rebuild's solve is the only one: reads of either model, with the solver held so
    // any solve would hang, answer from the model, the solver asked nothing more and none
    // started (the design's bound on sketch workers, section 6, holds).
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let started = Arc::clone(&starts);
    let sessions = opened(&dir, move || {
        started.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    let asked = sketches.requests().len();
    assert_eq!((starts.load(Ordering::SeqCst), asked), (1, 1));
    sketches.hold();
    for model in [ModelChoice::Current, ModelChoice::Published] {
        let read = answer(sessions.sketch(&s1(), model, id("k7f2qx3m")));
        let read = read.result.unwrap();
        assert_eq!(read.members.points.len(), 2, "{model:?}");
    }
    assert_eq!(sketches.requests().len(), asked);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    sketches.release();
}

#[test]
fn a_rebuild_whose_solver_failed_refuses_the_read_with_its_error_as_check_reports_it() {
    // The worker's own failure under the rebuild says nothing of the sketch's state: the
    // read is refused with the model's error for the sketch, naming it, the error `check`
    // of the same model reports; the read solves nothing again (the solver now answers).
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
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
    let solver = sketches.clone();
    let sessions = opened(&dir, move || {
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    failing.store(false, Ordering::SeqCst);
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
    // Worded as every read words the fault its model kept: how to try again.
    assert_eq!(
        error.suggestion(),
        Some(
            "check with fresh to try it again; if the same feature crashes the worker again, \
             report it with the document"
        )
    );
    assert_eq!(refused.model.unwrap().fault.unwrap().code(), error.code());
}

#[test]
fn a_solver_that_couldnt_start_refuses_the_read_with_the_rebuilds_error() {
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
    // A worker that couldn't be had says nothing of the feature that asked (as every read
    // words a model's fault).
    assert_eq!(
        (error.code(), error.message(), error.feature()),
        (
            ErrorCode::WorkerUnavailable,
            "no sketch worker at /nowhere",
            None
        )
    );
    assert_eq!(problems.total(), 1);
}
