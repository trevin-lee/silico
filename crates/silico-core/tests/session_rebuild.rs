//! M4.2: a session rebuilds its document when a command first needs its model (decision
//! 13), on its own client of the shared kernel, so one session's rebuild never supersedes
//! another's (MA2); sessions in one directory share its cache; a revision is rebuilt once;
//! and dropping the sessions stops a rebuild rather than waiting for it. Expected values are
//! the fake kernel's call record, the box's analytic measures (40 × 20 × 10: volume 8000,
//! area 2800), and the limits as the design's table gives them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{
    self, ModelChoice, ModelRead, PublishedRead, Response, Revision, Running, SessionId, Sessions,
};
use silico_core::document::Measure;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Op, Primitive};
use silico_core::sketch::SketchSolver;
use silico_protocol::{SketchSystem, Solved};
use tempfile::TempDir;

use crate::sessions::{self, HANG, answer, id};

use crate::lateness::{Lateness, allowance};

const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

/// [`BOX`]'s box under another ID and name: the same steps, which are keyed by their values.
const SAME_BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "b0000000", "type": "box", "v": 1, "name": "Other", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

const SPHERE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "a91cw0de", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}]}"#;

/// Sessions on a fake kernel of two threads, which the test keeps a handle on, with each
/// cache's secret outside the documents' scratch directory, and sketches solved by a fake.
struct Fixture {
    dir: TempDir,
    kernel: Arc<FakeKernel>,
    sketches: FakeSketches,
    solvers: Arc<AtomicUsize>,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let solvers = Arc::new(AtomicUsize::new(0));
    let (solver, started) = (sketches.clone(), Arc::clone(&solvers));
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || {
            started.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
        },
    );
    Fixture {
        dir,
        kernel,
        sketches,
        solvers,
        sessions,
    }
}

impl Fixture {
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    fn open(&self, path: &Path) -> SessionId {
        let response = answer(self.sessions.open(path, false));
        assert!(response.result.is_ok(), "{response:?}");
        response.session.unwrap().id
    }

    /// How many boxes the kernel has been asked to make.
    fn boxes(&self) -> usize {
        self.kernel
            .calls()
            .iter()
            .filter(|c| matches!(c.op, Op::MakePrimitive(Primitive::Box { .. })))
            .count()
    }
}

/// A check that found no problems, of the model rebuilt from revision 1.
fn clean(response: &Response<command::CheckAnswer>) {
    assert!(
        response
            .result
            .as_ref()
            .is_ok_and(|checked| checked.problems.is_empty()),
        "{response:?}"
    );
    assert_eq!(
        response.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Current,
            fault: None,
        })
    );
}

/// The one error a refused response gives: its code, message, and parameter.
fn refusal<T: std::fmt::Debug>(response: &Response<T>) -> (ErrorCode, String, Option<String>) {
    let (code, message, parameter, _) = sessions::refusal(response);
    (code, message, parameter)
}

#[test]
fn one_sessions_rebuild_never_supersedes_anothers() {
    // MA2 for sessions: A's box is held while B checks its sphere, which would advance a
    // generation every session shared.
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    let b = f.open(&f.file("b.silico", SPHERE));
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let mut held = f.sessions.check(&a, ModelChoice::Current);
    assert!(f.kernel.wait_running(1, HANG));
    clean(&answer(f.sessions.check(&b, ModelChoice::Current)));
    held = match held.try_take() {
        Ok(response) => panic!("A answered while its box was held: {response:?}"),
        Err(running) => running,
    };
    hold.release();
    clean(&answer(held));
    let model = f.sessions.published(&a).result.unwrap();
    assert_eq!(model.revision, Revision::OPENED);
    let fingerprint = model.model.fingerprint(&id("k7f2qx3m")).unwrap();
    assert_eq!(fingerprint.volume, Measure::from_millionths(8_000_000_000));
    assert_eq!(fingerprint.area, Measure::from_millionths(2_800_000_000));
    // The box was made once, never superseded and made again; and A's and B's work came
    // from two clients.
    assert_eq!(f.boxes(), 1);
    let clients: std::collections::BTreeSet<_> =
        f.kernel.calls().iter().map(|c| c.client).collect();
    assert_eq!(clients.len(), 2);
}

#[test]
fn sessions_in_one_directory_build_its_common_steps_once() {
    let f = fixture();
    let a = f.open(&f.file("one/a.silico", BOX));
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    assert_eq!(f.boxes(), 1);
    // The same box under another ID, beside it: taken from the directory's cache.
    let b = f.open(&f.file("one/b.silico", SAME_BOX));
    clean(&answer(f.sessions.check(&b, ModelChoice::Current)));
    assert_eq!(f.boxes(), 1);
    // In another directory, another cache: made again.
    let c = f.open(&f.file("two/c.silico", SAME_BOX));
    clean(&answer(f.sessions.check(&c, ModelChoice::Current)));
    assert_eq!(f.boxes(), 2);
}

#[test]
fn a_second_check_of_a_revision_reads_the_model_its_first_made() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    let first = f.sessions.published(&a).result.unwrap();
    let calls = f.kernel.calls().len();
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    // No rebuild: the kernel asked nothing, and the model is the same one, not one rebuilt.
    assert_eq!(f.kernel.calls().len(), calls);
    let second = f.sessions.published(&a).result.unwrap();
    assert!(Arc::ptr_eq(&first.model, &second.model));
}

#[test]
fn a_check_of_a_document_the_gate_refused_gives_its_problems_and_rebuilds_nothing() {
    let flat = BOX.replace("\"length\": 40", "\"length\": 0");
    let f = fixture();
    let s = f.open(&f.file("flat.silico", &flat));
    let response = answer(f.sessions.check(&s, ModelChoice::Current));
    let problems = response.result.unwrap().problems;
    assert_eq!(problems.total(), 1);
    assert_eq!(
        problems.errors()[0].message(),
        "box length must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got 0"
    );
    assert_eq!(response.model, None);
    assert!(f.kernel.calls().is_empty());
    // Nothing to publish: a published read is refused with the gate's problem, and starts
    // nothing.
    assert_eq!(
        refusal(&f.sessions.published(&s)),
        (
            ErrorCode::InvalidInput,
            "box length must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got 0"
                .to_owned(),
            Some("length".to_owned()),
        )
    );
    assert!(f.kernel.calls().is_empty());
}

#[test]
fn a_sessions_sketches_are_solved_by_the_registrys_starter() {
    let sketch = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}]}"#;
    let f = fixture();
    let s = f.open(&f.file("sketch.silico", sketch));
    clean(&answer(f.sessions.check(&s, ModelChoice::Current)));
    // One solver started, which solved the one sketch.
    assert_eq!(f.solvers.load(Ordering::SeqCst), 1);
    assert_eq!(f.sketches.requests().len(), 1);
}

#[test]
fn every_session_shares_the_registrys_sketch_solver_one_after_another() {
    // A-M2: one solver for every session that solves after another has (`silico check` of
    // many files), not one a session. Three sessions, two open at once, each a sketch of
    // its own (a point at another x, so none is solved from another's result).
    let sketch = |x: u32| {
        format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{{"id": "pa000000", "x": {x}, "y": 0, "construction": false}}], "lines": [], "circles": [], "arcs": [], "constraints": []}}]}}"#
        )
    };
    let f = fixture();
    let a = f.open(&f.file("a.silico", &sketch(1)));
    let b = f.open(&f.file("b.silico", &sketch(2)));
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    clean(&answer(f.sessions.check(&b, ModelChoice::Current)));
    answer(f.sessions.close(&a)).result.unwrap();
    let c = f.open(&f.file("c.silico", &sketch(3)));
    clean(&answer(f.sessions.check(&c, ModelChoice::Current)));
    // Three sketches solved, on one solver.
    assert_eq!(f.sketches.requests().len(), 3);
    assert_eq!(f.solvers.load(Ordering::SeqCst), 1);
}

#[test]
fn a_sessions_solve_never_waits_for_another_sessions_solve() {
    // Sessions solving at once each have a solver: one session's solve, held, leaves the
    // other's to finish on a second solver started for it. The first solver started holds
    // its solves, the second doesn't.
    let sketch = |x: u32| {
        format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{{"id": "pa000000", "x": {x}, "y": 0, "construction": false}}], "lines": [], "circles": [], "arcs": [], "constraints": []}}]}}"#
        )
    };
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let (held, free) = (FakeSketches::new(), FakeSketches::new());
    held.hold();
    let starts = Arc::new(AtomicUsize::new(0));
    let (first, second, counted) = (held.clone(), free.clone(), Arc::clone(&starts));
    let sessions = Sessions::with_stand_ins(
        Arc::new(FakeKernel::new(2).unwrap()),
        move |cache| Cache::with_secret(cache, &secret),
        move || {
            let solver = match counted.fetch_add(1, Ordering::SeqCst) {
                0 => first.clone(),
                _ => second.clone(),
            };
            Ok(Box::new(solver) as Box<dyn SketchSolver>)
        },
    );
    let open = |name: &str, x: u32| {
        let path = dir.path().join(name);
        std::fs::write(&path, sketch(x)).unwrap();
        answer(sessions.open(&path, false)).session.unwrap().id
    };
    let (a, b) = (open("a.silico", 1), open("b.silico", 2));
    let checking_a = sessions.check(&a, ModelChoice::Current);
    assert!(held.wait_held(HANG), "a's solve was never held");
    // b's check answers while a's solve is still held.
    clean(&answer(sessions.check(&b, ModelChoice::Current)));
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    assert_eq!(free.requests().len(), 1);
    held.release();
    clean(&answer(checking_a));
    assert_eq!(held.requests().len(), 1);
    assert_eq!(starts.load(Ordering::SeqCst), 2);
}

#[test]
fn a_solver_given_back_while_another_session_starts_one_is_taken_not_started_again() {
    // Session o's solve is held on the first solver; p, finding none idle, starts a second,
    // which is held in its start; q, finding none idle either, waits for p's start to end.
    // Meanwhile o's solver is given back: q takes it, so two are started, not three.
    let sketch = |x: u32| {
        format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{{"id": "pa000000", "x": {x}, "y": 0, "construction": false}}], "lines": [], "circles": [], "arcs": [], "constraints": []}}]}}"#
        )
    };
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let (held, free) = (FakeSketches::new(), FakeSketches::new());
    held.hold();
    let starts = Arc::new(AtomicUsize::new(0));
    let (entered, in_start) = std::sync::mpsc::channel::<()>();
    let (go, gate) = std::sync::mpsc::channel::<()>();
    let gate = std::sync::Mutex::new(gate);
    let (first, second, counted) = (held.clone(), free.clone(), Arc::clone(&starts));
    let sessions = Sessions::with_stand_ins(
        Arc::new(FakeKernel::new(2).unwrap()),
        move |cache| Cache::with_secret(cache, &secret),
        move || {
            let solver = match counted.fetch_add(1, Ordering::SeqCst) {
                0 => first.clone(),
                1 => {
                    entered.send(()).unwrap();
                    gate.lock().unwrap().recv_timeout(HANG).unwrap();
                    second.clone()
                }
                _ => second.clone(),
            };
            Ok(Box::new(solver) as Box<dyn SketchSolver>)
        },
    );
    let open = |name: &str, x: u32| {
        let path = dir.path().join(name);
        std::fs::write(&path, sketch(x)).unwrap();
        answer(sessions.open(&path, false)).session.unwrap().id
    };
    let (o, p, q) = (
        open("o.silico", 1),
        open("p.silico", 2),
        open("q.silico", 3),
    );
    let checking_o = sessions.check(&o, ModelChoice::Current);
    assert!(held.wait_held(HANG), "o's solve was never held");
    let checking_p = sessions.check(&p, ModelChoice::Current);
    in_start.recv_timeout(HANG).unwrap();
    let checking_q = sessions.check(&q, ModelChoice::Current);
    // Time for q to find none idle and wait for p's start; were it slower, q would find o's
    // solver idle at once, and the test pass for any version.
    std::thread::sleep(Duration::from_millis(300));
    held.release();
    clean(&answer(checking_o));
    go.send(()).unwrap();
    clean(&answer(checking_p));
    clean(&answer(checking_q));
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    // Three solves on the two: which one q's took depends on when it came (o's solver, given
    // back while it waited, or p's, given back first on a slow machine), so only the total.
    assert_eq!(held.requests().len() + free.requests().len(), 3);
}

#[test]
fn dropping_the_sessions_mid_rebuild_stops_it_and_returns_at_once() {
    // A step is held, and let go only after the drop returns: a drop that waited for the
    // rebuild before stopping it would never return.
    let f = fixture();
    let path = f.file("a.silico", BOX);
    let a = f.open(&path);
    let _hold = f.kernel.hold(|_| true);
    let check = f.sessions.check(&a, ModelChoice::Current);
    assert!(f.kernel.wait_running(1, HANG));
    let Fixture { dir, sessions, .. } = f;
    let lateness = Lateness::start();
    let (dropped, returned) = std::sync::mpsc::channel();
    let at = Instant::now();
    std::thread::spawn(move || {
        drop(sessions);
        let _ = dropped.send(());
    });
    assert!(
        returned.recv_timeout(HANG).is_ok(),
        "dropping the sessions waited for the rebuild"
    );
    let took = at.elapsed();
    // A drop stops the rebuild, which takes microseconds: a second for the session's thread
    // to see it and end on a quiet machine, and ten times the machine's lateness on a
    // loaded one.
    let most = allowance(Duration::from_secs(1), lateness.most());
    assert!(
        took < most,
        "dropping the sessions took {took:?}, past {most:?}"
    );
    // The check ends without a model: its session closed under it.
    let response = answer(check);
    assert_eq!(response.model, None);
    let problems = response.result.unwrap_err();
    assert_eq!(problems.errors()[0].code(), ErrorCode::InvalidInput);
    assert_eq!(problems.errors()[0].message(), "no session s1 is open");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), BOX);
    drop(dir);
}

#[test]
fn a_sessions_check_gives_its_models_failures() {
    let f = fixture();
    f.kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
            .then(|| silico_core::Error::new(ErrorCode::KernelFailed, "MakeBox failed"))
    });
    let a = f.open(&f.file("a.silico", BOX));
    let response = answer(f.sessions.check(&a, ModelChoice::Current));
    assert_eq!(
        response.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Current,
            fault: None,
        })
    );
    let problems = response.result.unwrap().problems;
    assert_eq!(problems.total(), 1);
    assert_eq!(problems.errors()[0].code(), ErrorCode::KernelFailed);
    assert_eq!(problems.errors()[0].feature(), Some("k7f2qx3m"));
}

#[test]
fn a_sessions_check_gives_drift_of_a_stored_fingerprint() {
    // The box's volume is 8000: one stored as 8001 drifts.
    let stored = BOX.replace(
        "\"height\": 10}",
        "\"height\": 10, \"fingerprint\": {\"volume\": 8001, \"area\": 2800, \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}}",
    );
    let f = fixture();
    let a = f.open(&f.file("a.silico", &stored));
    let problems = answer(f.sessions.check(&a, ModelChoice::Current))
        .result
        .unwrap()
        .problems;
    let codes: Vec<ErrorCode> = problems.errors().iter().map(|e| e.code()).collect();
    assert_eq!(codes, [ErrorCode::Drift]);
    assert_eq!(problems.errors()[0].feature(), Some("k7f2qx3m"));
}

/// [`BOX`] and [`SPHERE`]'s ball, neither using the other: two features a worker that
/// crashes on every request stops alike.
const BOX_AND_BALL: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "a91cw0de", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}]}"#;

/// The fault `response`'s model read gives: its code, feature, message, and suggestion, as
/// it was met (the worker's own; the wire adds how a session's client tries again).
fn fault<T>(response: &Response<T>) -> (ErrorCode, Option<String>, String, Option<String>) {
    let fault = response.model.as_ref().unwrap().fault.as_ref().unwrap();
    (
        fault.code(),
        fault.feature().map(str::to_owned),
        fault.message().to_owned(),
        fault.suggestion().map(str::to_owned),
    )
}

#[test]
fn a_kernel_fault_is_given_once_beside_the_checks_result_and_kept_for_the_revision() {
    // A worker that crashes on every request stops both features: the check answers, the
    // document's problems none (a fault is the kernel's), and the fault given once, the
    // first the timeline met (the box's), not once a feature (AR1's pattern).
    let f = fixture();
    f.kernel.refuse(|_| {
        Some(
            silico_core::Error::new(ErrorCode::WorkerCrashed, "crashed")
                .with_suggestion("try again"),
        )
    });
    let s = f.open(&f.file("s.silico", BOX_AND_BALL));
    let first = answer(f.sessions.check(&s, ModelChoice::Current));
    assert!(
        first.result.as_ref().unwrap().problems.is_empty(),
        "{first:?}"
    );
    assert_eq!(first.model.as_ref().unwrap().revision, Revision::OPENED);
    assert_eq!(
        fault(&first),
        (
            ErrorCode::WorkerCrashed,
            Some("k7f2qx3m".to_owned()),
            "crashed".to_owned(),
            Some("try again".to_owned())
        )
    );
    // The box and the ball, each asked once.
    assert_eq!(f.kernel.calls().len(), 2);
    // Kept as the revision's outcome: the next check reads the same model, asking the
    // kernel nothing, and gives the same fault.
    let second = answer(f.sessions.check(&s, ModelChoice::Current));
    assert_eq!(f.kernel.calls().len(), 2);
    assert_eq!(second.model, first.model);
    assert_eq!(second.result, first.result);
}

#[test]
fn the_fault_given_is_the_first_the_timeline_met_in_a_measure_or_a_failure() {
    // The box, its fingerprint stored, is measured, and the measure times out; the ball,
    // after it, crashes the worker. The fault given is the box's, met first in timeline
    // order, though it's a measure's and the ball's a failure.
    let stored = BOX_AND_BALL.replace(
        "\"height\": 10}",
        "\"height\": 10, \"fingerprint\": {\"volume\": 8000, \"area\": 2800, \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}}",
    );
    assert_ne!(stored, BOX_AND_BALL);
    let f = fixture();
    f.kernel.refuse(|op| match op {
        Op::Summarize { .. } => Some(silico_core::Error::new(
            ErrorCode::WorkerTimeout,
            "timed out",
        )),
        Op::MakePrimitive(Primitive::Sphere { .. }) => {
            Some(silico_core::Error::new(ErrorCode::WorkerCrashed, "crashed"))
        }
        _ => None,
    });
    let s = f.open(&f.file("a.silico", &stored));
    let checked = answer(f.sessions.check(&s, ModelChoice::Current));
    assert!(
        checked.result.as_ref().unwrap().problems.is_empty(),
        "{checked:?}"
    );
    let (code, feature, message, _) = fault(&checked);
    assert_eq!(
        (code, feature.as_deref(), message.as_str()),
        (ErrorCode::WorkerTimeout, Some("k7f2qx3m"), "timed out")
    );
}

#[test]
fn a_sketch_solver_that_cant_be_had_is_one_fault_naming_no_feature() {
    // The solver's start fails every sketch with the same error (AR1's pattern): the check
    // gives it once, naming none of them, since it says nothing of any.
    let two = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "A", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}, {"id": "s0000002", "type": "sketch", "v": 1, "name": "B", "plane": "xy", "points": [{"id": "pa000000", "x": 1, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}]}"#;
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let starts = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&starts);
    let sessions = Sessions::with_stand_ins(
        FakeKernel::new(1).unwrap(),
        move |cache| Cache::with_secret(cache, &secret),
        move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(silico_core::Error::new(
                ErrorCode::WorkerUnavailable,
                "SILICO_SKETCH_WORKER names /nowhere, which isn't a file",
            )
            .with_suggestion("name the sketch worker's executable"))
        },
    );
    let path = dir.path().join("two.silico");
    std::fs::write(&path, two).unwrap();
    let s = answer(sessions.open(&path, false)).session.unwrap().id;
    let checked = answer(sessions.check(&s, ModelChoice::Current));
    assert!(
        checked.result.as_ref().unwrap().problems.is_empty(),
        "{checked:?}"
    );
    assert_eq!(
        fault(&checked),
        (
            ErrorCode::WorkerUnavailable,
            None,
            "SILICO_SKETCH_WORKER names /nowhere, which isn't a file".to_owned(),
            Some("name the sketch worker's executable".to_owned())
        )
    );
    assert_eq!(
        sessions.status(&s).result.unwrap().rebuild.state,
        command::RebuildState::Faulted
    );
    // Kept: the next check starts no solver.
    let again = answer(sessions.check(&s, ModelChoice::Current));
    assert_eq!(again.model, checked.model);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
}

#[test]
fn a_fault_that_passed_is_tried_again_only_at_a_new_revision() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    let once = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let first = Arc::clone(&once);
    f.kernel.refuse(move |op| {
        (matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
            && first.swap(false, Ordering::SeqCst))
        .then(|| silico_core::Error::new(ErrorCode::WorkerTimeout, "timed out"))
    });
    let faulted = answer(f.sessions.check(&a, ModelChoice::Current));
    assert!(faulted.result.as_ref().unwrap().problems.is_empty());
    assert_eq!(fault(&faulted).0, ErrorCode::WorkerTimeout);
    // Passed, but the revision keeps it: no box made again.
    let again = answer(f.sessions.check(&a, ModelChoice::Current));
    assert_eq!(fault(&again).0, ErrorCode::WorkerTimeout);
    assert_eq!(f.boxes(), 1);
    // An edit, its block renamed: a new revision, whose rebuild makes the box again, and
    // builds.
    let renamed = BOX.replace("\"Block\"", "\"Plate\"");
    let edited = answer(
        f.sessions
            .replace_text(&a, Revision::OPENED, renamed.clone()),
    );
    let saved = edited.session.as_ref().unwrap().revision;
    assert_eq!(saved, Revision::OPENED.next().unwrap());
    assert_eq!(edited.model.as_ref().unwrap().fault, None, "{edited:?}");
    assert_eq!(f.boxes(), 2);
    let checked = answer(f.sessions.check(&a, ModelChoice::Current));
    assert!(checked.result.unwrap().problems.is_empty());
    assert_eq!(
        checked.model,
        Some(ModelRead {
            revision: saved,
            of: ModelChoice::Current,
            fault: None,
        })
    );
}

#[test]
fn a_published_read_never_waits_for_a_rebuild() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let check = f.sessions.check(&a, ModelChoice::Current);
    assert!(f.kernel.wait_running(1, HANG));
    let lateness = Lateness::start();
    let at = Instant::now();
    let sessions = &f.sessions;
    let (reply, read) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let _ = reply.send(sessions.published(&a));
        });
        let read = read.recv_timeout(HANG);
        let took = at.elapsed();
        let Ok(read) = read else {
            hold.release();
            panic!("the published read waited for the rebuild");
        };
        // At once: a second, and ten times the machine's lateness on a loaded one.
        let most = allowance(Duration::from_secs(1), lateness.most());
        assert!(
            took < most,
            "the published read took {took:?}, past {most:?}"
        );
        assert_eq!(read.session.as_ref().map(|s| s.id), Some(a));
        assert_eq!(read.model, None);
        assert_eq!(
            refusal(&read),
            (
                ErrorCode::InvalidInput,
                "session s1 has published no model yet".to_owned(),
                Some("model".to_owned()),
            )
        );
    });
    hold.release();
    clean(&answer(check));
    let read = f.sessions.published(&a);
    assert_eq!(
        read.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Published,
            fault: None,
        })
    );
    assert_eq!(read.result.unwrap().revision, Revision::OPENED);
}

#[test]
fn a_published_read_starts_the_rebuild_it_needs_without_waiting() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    assert_eq!(
        refusal(&f.sessions.published(&a)).2.as_deref(),
        Some("model")
    );
    // No check: the published read's own rebuild publishes, and only one is started.
    let deadline = Instant::now() + HANG;
    let published = loop {
        if let Ok(published) = f.sessions.published(&a).result {
            break published;
        }
        assert!(
            Instant::now() < deadline,
            "the published read started no rebuild"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(published.revision, Revision::OPENED);
    assert_eq!(f.boxes(), 1);
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    assert_eq!(f.boxes(), 1);
}

#[test]
fn a_published_read_of_a_session_that_isnt_open_is_refused() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    assert!(answer(f.sessions.close(&a)).result.is_ok());
    let read = f.sessions.published(&a);
    assert_eq!(read.session, None);
    assert_eq!(
        refusal(&read),
        (
            ErrorCode::InvalidInput,
            "no session s1 is open".to_owned(),
            Some("session".to_owned()),
        )
    );
}

#[test]
fn a_check_of_text_that_isnt_a_document_gives_the_readers_error_and_rebuilds_nothing() {
    let f = fixture();
    let s = f.open(&f.file("junk.silico", "{"));
    let response = answer(f.sessions.check(&s, ModelChoice::Current));
    assert_eq!(response.model, None);
    let problems = response.result.unwrap().problems;
    assert_eq!(problems.total(), 1);
    assert_eq!(problems.errors()[0].code(), ErrorCode::InvalidInput);
    assert!(
        problems.errors()[0]
            .message()
            .starts_with("the document isn't valid JSON: "),
        "{problems:?}"
    );
    assert!(f.kernel.calls().is_empty());
    // Nothing to publish: the reader's error, the same one.
    let read = f.sessions.published(&s);
    assert_eq!(read.result.unwrap_err(), problems);
    assert!(f.kernel.calls().is_empty());
}

/// A sketch solver with a bug: every solve panics, after counting itself.
struct Panics(Arc<AtomicUsize>);

impl SketchSolver for Panics {
    fn solve(&mut self, _: SketchSystem) -> Result<Solved, silico_core::Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("a bug in the solver")
    }
}

#[test]
fn a_check_whose_recompute_failed_is_refused_with_why_once_and_the_next_rebuilds() {
    let sketch = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}]}"#;
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let solves = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&solves);
    let sessions = Sessions::with_stand_ins(
        FakeKernel::new(1).unwrap(),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(Panics(Arc::clone(&counted))) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("sketch.silico");
    std::fs::write(&path, sketch).unwrap();
    let s = answer(sessions.open(&path, false)).session.unwrap().id;
    let panicked = (
        ErrorCode::Internal,
        "a sketch's solve panicked".to_owned(),
        None,
    );
    let first = answer(sessions.check(&s, ModelChoice::Current));
    assert_eq!(first.model, None);
    assert_eq!(refusal(&first), panicked);
    assert_eq!(solves.load(Ordering::SeqCst), 1);
    // `status` says it faulted (the design's `faulted`, a bug's failure as a fault's).
    assert_eq!(
        sessions.status(&s).result.unwrap().rebuild.state,
        command::RebuildState::Faulted
    );
    // A published read doesn't start it again (it would fail again, each frame): it says
    // why there's nothing.
    for _ in 0..3 {
        assert_eq!(refusal(&sessions.published(&s)), panicked);
    }
    assert_eq!(solves.load(Ordering::SeqCst), 1);
    // The next check, the failure said, rebuilds: and it fails again, said again.
    let second = answer(sessions.check(&s, ModelChoice::Current));
    assert_eq!(refusal(&second), panicked);
    assert_eq!(solves.load(Ordering::SeqCst), 2);
}

#[test]
fn a_measure_that_faulted_is_the_revisions_fault_and_not_measured_again() {
    // The box's measure times out once: no failure of the box, a fault in its drift (a
    // stored fingerprint the rebuild couldn't measure to compare), given once beside the
    // check's result, and kept: the next check measures nothing again.
    let stored = BOX.replace(
        "\"height\": 10}",
        "\"height\": 10, \"fingerprint\": {\"volume\": 8000, \"area\": 2800, \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}}",
    );
    let f = fixture();
    let once = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let first = Arc::clone(&once);
    f.kernel.refuse(move |op| {
        (matches!(op, Op::Summarize { .. }) && first.swap(false, Ordering::SeqCst))
            .then(|| silico_core::Error::new(ErrorCode::WorkerTimeout, "timed out"))
    });
    let a = f.open(&f.file("a.silico", &stored));
    let measures = || {
        f.kernel
            .calls()
            .iter()
            .filter(|c| matches!(c.op, Op::Summarize { .. }))
            .count()
    };
    let faulted = answer(f.sessions.check(&a, ModelChoice::Current));
    assert!(faulted.result.as_ref().unwrap().problems.is_empty());
    assert_eq!(
        fault(&faulted),
        (
            ErrorCode::WorkerTimeout,
            Some("k7f2qx3m".to_owned()),
            "timed out".to_owned(),
            None
        )
    );
    assert_eq!(measures(), 1);
    let again = answer(f.sessions.check(&a, ModelChoice::Current));
    assert_eq!(again.model, faulted.model);
    assert_eq!(measures(), 1);
}

/// A read that gives the box's volume, as its fingerprint says it, and what it read.
fn volume<C>(
    read: silico_core::command::Read<'_, C>,
) -> Result<(Option<Measure>, ModelRead), command::Problems> {
    Ok((
        read.model().fingerprint(&id("k7f2qx3m")).map(|f| f.volume),
        read.model_read().clone(),
    ))
}

/// The box's volume, 40 × 20 × 10 mm.
const EIGHT_THOUSAND: Measure = Measure::from_millionths(8_000_000_000);

const CURRENT: ModelRead = ModelRead {
    revision: Revision::OPENED,
    of: ModelChoice::Current,
    fault: None,
};

const PUBLISHED: ModelRead = ModelRead {
    revision: Revision::OPENED,
    of: ModelChoice::Published,
    fault: None,
};

#[test]
fn a_current_read_waits_for_the_revisions_rebuild_and_is_handed_what_it_read() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let mut read = f
        .sessions
        .read_model(&a, PublishedRead::Check, ModelChoice::Current, volume);
    assert!(f.kernel.wait_running(1, HANG));
    read = match read.try_take() {
        Ok(response) => panic!("the current read didn't wait for its rebuild: {response:?}"),
        Err(running) => running,
    };
    hold.release();
    let response = answer(read);
    assert_eq!(response.model, Some(CURRENT));
    assert_eq!(response.session.unwrap().id, a);
    assert_eq!(response.result.unwrap(), (Some(EIGHT_THOUSAND), CURRENT));
}

#[test]
fn a_published_read_runs_beside_a_waiting_current_one_and_gives_the_newest_model() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let current = f
        .sessions
        .read_model(&a, PublishedRead::Check, ModelChoice::Current, |read| {
            Ok(read.model_read().clone())
        });
    assert!(f.kernel.wait_running(1, HANG));
    // Queued behind nothing: answered while the current read waits on the session's rebuild.
    let lateness = Lateness::start();
    let at = Instant::now();
    let published =
        f.sessions
            .read_model(&a, PublishedRead::Check, ModelChoice::Published, |read| {
                Ok(read.model_read().clone())
            });
    let Ok(early) = published.wait_timeout(HANG) else {
        hold.release();
        panic!("the published read waited behind the current one");
    };
    let took = at.elapsed();
    let most = allowance(Duration::from_secs(1), lateness.most());
    assert!(
        took < most,
        "the published read took {took:?}, past {most:?}"
    );
    assert_eq!(early.model, None);
    assert_eq!(early.session.as_ref().map(|s| s.id), Some(a));
    assert_eq!(
        refusal(&early),
        (
            ErrorCode::InvalidInput,
            "session s1 has published no model yet".to_owned(),
            Some("model".to_owned()),
        )
    );
    hold.release();
    assert_eq!(answer(current).result.unwrap(), CURRENT);
    // Now published: the newest model, and the read handed what it read.
    let late =
        answer(
            f.sessions
                .read_model(&a, PublishedRead::Check, ModelChoice::Published, volume),
        );
    assert_eq!(late.model, Some(PUBLISHED));
    assert_eq!(late.result.unwrap(), (Some(EIGHT_THOUSAND), PUBLISHED));
}

#[test]
fn a_published_read_alone_starts_the_rebuild() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    let first =
        answer(
            f.sessions
                .read_model(&a, PublishedRead::Check, ModelChoice::Published, volume),
        );
    assert_eq!(refusal(&first).2.as_deref(), Some("model"));
    // No check, no current read: the published read's own rebuild publishes, once.
    let deadline = Instant::now() + HANG;
    let read = loop {
        let read =
            answer(
                f.sessions
                    .read_model(&a, PublishedRead::Check, ModelChoice::Published, volume),
            );
        if read.result.is_ok() {
            break read;
        }
        assert!(
            Instant::now() < deadline,
            "the published read started no rebuild"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(read.result.unwrap(), (Some(EIGHT_THOUSAND), PUBLISHED));
    assert_eq!(f.boxes(), 1);
}

#[test]
fn a_published_read_runs_off_the_callers_thread() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    let (release, released) = std::sync::mpsc::channel::<()>();
    let mut read = f.sessions.read_model(
        &a,
        PublishedRead::Check,
        ModelChoice::Published,
        move |read| {
            let _ = released.recv();
            volume(read)
        },
    );
    // Returned while its read waits, on a thread of its own.
    for _ in 0..5 {
        read = match read.try_take() {
            Ok(response) => panic!("answered while its read waited: {response:?}"),
            Err(running) => running,
        };
        std::thread::sleep(Duration::from_millis(20));
    }
    release.send(()).unwrap();
    let response = answer(read);
    assert_eq!(response.session.unwrap().id, a);
    assert_eq!(response.model, Some(PUBLISHED));
    assert_eq!(response.result.unwrap(), (Some(EIGHT_THOUSAND), PUBLISHED));
}

#[test]
fn a_reads_own_refusal_is_the_responses_beside_the_model_it_read() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    let stale = || {
        command::Problems::from(silico_core::Error::new(
            ErrorCode::Conflict,
            "the cursor is of another revision",
        ))
    };
    // The current read first, so the published one has a model to read.
    for (choice, read) in [
        (ModelChoice::Current, CURRENT),
        (ModelChoice::Published, PUBLISHED),
    ] {
        let response = answer(
            f.sessions
                .read_model(&a, PublishedRead::Check, choice, move |_| {
                    Err::<(), _>(stale())
                }),
        );
        assert_eq!(response.result.unwrap_err(), stale(), "{choice:?}");
        assert_eq!(response.model, Some(read), "{choice:?}");
    }
}

#[test]
fn a_read_of_a_document_the_gate_refused_is_refused_with_its_problems_unread() {
    let flat = BOX.replace("\"length\": 40", "\"length\": 0");
    let cases = [
        (
            flat.as_str(),
            (
                ErrorCode::InvalidInput,
                "box length must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got 0"
                    .to_owned(),
                Some("length".to_owned()),
            ),
        ),
        (
            "// not a document",
            (
                ErrorCode::InvalidInput,
                "the document isn't valid JSON: expected a value, found `/`, at line 1, column 1"
                    .to_owned(),
                None,
            ),
        ),
    ];
    let f = fixture();
    for (n, (text, problem)) in cases.into_iter().enumerate() {
        let s = f.open(&f.file(&format!("refused{n}.silico"), text));
        for choice in [ModelChoice::Current, ModelChoice::Published] {
            let response = answer(f.sessions.read_model(
                &s,
                PublishedRead::Check,
                choice,
                |_| -> Result<(), _> { panic!("a model was read of a document the gate refused") },
            ));
            assert_eq!(response.model, None);
            assert_eq!(refusal(&response), problem, "{choice:?}");
        }
    }
    assert!(f.kernel.calls().is_empty());
}

#[test]
fn a_read_of_a_session_that_isnt_open_is_refused() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    assert!(answer(f.sessions.close(&a)).result.is_ok());
    for choice in [ModelChoice::Current, ModelChoice::Published] {
        let response = answer(
            f.sessions
                .read_model(&a, PublishedRead::Check, choice, volume),
        );
        assert_eq!(response.session, None);
        assert_eq!(
            refusal(&response),
            (
                ErrorCode::InvalidInput,
                "no session s1 is open".to_owned(),
                Some("session".to_owned()),
            ),
            "{choice:?}"
        );
    }
}

#[test]
fn a_published_read_that_panics_is_said_to_have_stopped() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    let response = answer(f.sessions.read_model(
        &a,
        PublishedRead::Check,
        ModelChoice::Published,
        |_| -> Result<(), _> { panic!("a bug in a read") },
    ));
    assert_eq!(
        refusal(&response),
        (
            ErrorCode::Internal,
            "the read stopped before it answered".to_owned(),
            None,
        )
    );
}

/// `count` published reads of `a`, each blocked until `release` drops, each having said it
/// started.
fn blocked_reads(
    sessions: &Sessions<Arc<FakeKernel>>,
    a: &SessionId,
    count: usize,
) -> (Vec<Running<ModelRead>>, std::sync::mpsc::Sender<()>) {
    let (started, starts) = std::sync::mpsc::channel::<()>();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = Arc::new(std::sync::Mutex::new(released));
    let reads = (0..count)
        .map(|_| {
            let (started, released) = (started.clone(), Arc::clone(&released));
            sessions.read_model(
                a,
                PublishedRead::Check,
                ModelChoice::Published,
                move |read| {
                    let _ = started.send(());
                    // Until `release` drops: no message is ever sent.
                    let _ = released.lock().unwrap().recv();
                    Ok(read.model_read().clone())
                },
            )
        })
        .collect();
    for _ in 0..count {
        starts.recv_timeout(HANG).unwrap();
    }
    (reads, release)
}

#[test]
fn a_ninth_published_read_running_at_once_is_refused() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    let (reads, release) = blocked_reads(&f.sessions, &a, 8);
    let ninth =
        answer(
            f.sessions
                .read_model(&a, PublishedRead::Check, ModelChoice::Published, volume),
        );
    assert_eq!(
        refusal(&ninth),
        (
            ErrorCode::InvalidInput,
            "8 published reads are running in session s1, the most one session runs at once"
                .to_owned(),
            None,
        )
    );
    // Refused for an open session, which it says, as every command's refusal does.
    assert_eq!(ninth.session.unwrap().id, a);
    drop(release);
    for read in reads {
        assert_eq!(answer(read).result.unwrap(), PUBLISHED);
    }
    // The eight ended: a ninth runs.
    let ninth =
        answer(
            f.sessions
                .read_model(&a, PublishedRead::Check, ModelChoice::Published, volume),
        );
    assert_eq!(ninth.result.unwrap(), (Some(EIGHT_THOUSAND), PUBLISHED));
}

#[test]
fn dropping_the_sessions_waits_for_a_published_read_to_end() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    let (mut reads, release) = blocked_reads(&f.sessions, &a, 1);
    let Fixture { dir, sessions, .. } = f;
    let (dropped, returned) = std::sync::mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(sessions);
        let _ = dropped.send(());
    });
    // The read holds the session: the drop waits for it.
    if returned.recv_timeout(Duration::from_millis(200)).is_ok() {
        panic!("the drop returned while a published read ran");
    }
    let lateness = Lateness::start();
    let at = Instant::now();
    drop(release);
    assert!(
        returned.recv_timeout(HANG).is_ok(),
        "the drop never returned"
    );
    let took = at.elapsed();
    let most = allowance(Duration::from_secs(1), lateness.most());
    assert!(
        took < most,
        "the drop took {took:?} after the read ended, past {most:?}"
    );
    dropper.join().unwrap();
    assert_eq!(answer(reads.remove(0)).result.unwrap(), PUBLISHED);
    drop(dir);
}

#[test]
fn the_published_reads_bound_is_each_sessions_own() {
    // s1's eight reads running leave s2's published read room to run.
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    let b = f.open(&f.file("b.silico", BOX));
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    clean(&answer(f.sessions.check(&b, ModelChoice::Current)));
    let (reads, release) = blocked_reads(&f.sessions, &a, 8);
    let other =
        answer(
            f.sessions
                .read_model(&b, PublishedRead::Check, ModelChoice::Published, volume),
        );
    assert_eq!(other.result.unwrap(), (Some(EIGHT_THOUSAND), PUBLISHED));
    assert_eq!(other.session.unwrap().id, b);
    drop(release);
    for read in reads {
        assert_eq!(answer(read).result.unwrap(), PUBLISHED);
    }
}

#[test]
fn dropping_the_sessions_waits_for_a_published_read_of_a_session_closed_before() {
    // The read holds its session, closed since: the drop waits for it all the same, so no
    // session's thread outlives the sessions.
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    clean(&answer(f.sessions.check(&a, ModelChoice::Current)));
    let (mut reads, release) = blocked_reads(&f.sessions, &a, 1);
    answer(f.sessions.close(&a)).result.unwrap();
    let Fixture { dir, sessions, .. } = f;
    let (dropped, returned) = std::sync::mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(sessions);
        let _ = dropped.send(());
    });
    if returned.recv_timeout(Duration::from_millis(200)).is_ok() {
        panic!("the drop returned while a closed session's published read ran");
    }
    drop(release);
    assert!(
        returned.recv_timeout(HANG).is_ok(),
        "the drop never returned"
    );
    dropper.join().unwrap();
    assert_eq!(answer(reads.remove(0)).result.unwrap(), PUBLISHED);
    drop(dir);
}
