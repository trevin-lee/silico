//! /break-it on M3.4, stored coordinates (docs/design/m3.4-stored-coordinates.md), through
//! the real sketch worker: a solve at the edge of what a file may hold written back and read
//! again, the worker started again only from the build it started from, and a recompute
//! superseded, cancelled, or dropped while a long solve runs. The fake solver's side is
//! silico-core's tests/adversarial_stored_coordinates.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::document::{Checked, FeatureId, Operation, read, validate, write};
use silico_core::kernel::{Generation, Kernel, KernelRequest};
use silico_core::pool::Pending;
use silico_core::recompute::{FeatureOutcome, Recompute, RunOutcome};
use silico_core::sketch::{SketchSolver, SketchWorker, solve};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
const OTHER_BUILD: &str = env!("CARGO_BIN_EXE_silico-sketch-test-worker");
const FEATURE: &str = "k7f2qx3m";

fn point(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x:?}, "y": {y:?}, "construction": false}}"#)
}

fn document(points: &[String], arcs: &[String], constraints: &[String]) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"{FEATURE}\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", \"plane\": \"xy\", \"points\": [{}], \"lines\": [], \"circles\": [], \"arcs\": [{}], \"constraints\": [{}]}}]}}",
        points.join(", "),
        arcs.join(", "),
        constraints.join(", ")
    )
}

fn checked(text: &str) -> Checked {
    Checked::new(read(text.as_bytes()).unwrap())
        .unwrap_or_else(|e| panic!("the gate refused: {:?}", e.errors()))
}

fn feature() -> FeatureId {
    FeatureId::new(FEATURE).unwrap()
}

/// `text` with its sketch solved by `w` and written back, as a save writes it
/// (`SketchSolution::written`, what `Model::saved` uses).
fn saved(text: &str, w: &mut dyn SketchSolver) -> String {
    let doc = checked(text);
    let solution = solve(&doc, &feature(), w).unwrap_or_else(|e| panic!("{e}"));
    let mut document = doc.document().clone();
    let Operation::Sketch(sketch) = &mut document.features[0].operation else {
        panic!()
    };
    *sketch = solution.written(sketch);
    write(&document).unwrap()
}

// --- A solve at the reach, written back --------------------------------------------------

/// An arc about p0, held at the origin, from p1 at `start` degrees to p2 a quarter turn on,
/// drawn at radius 999,000 and dimensioned 1,000,000: the largest radius a driving
/// dimension may have.
fn arc_at_the_reach(start: f64) -> String {
    let r = 999_000.0;
    let (a, b) = (start.to_radians(), (start + 90.0).to_radians());
    document(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", r * libm::cos(a), r * libm::sin(a)),
            point("p0000002", r * libm::cos(b), r * libm::sin(b)),
        ],
        &[r#"{"id": "a0000001", "center": "p0000000", "start": "p0000001", "end": "p0000002", "construction": false}"#.to_owned()],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#.to_owned(),
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#.to_owned(),
            r#"{"id": "k0000003", "type": "radius", "curve": "a0000001", "value": 1000000, "driving": true}"#.to_owned(),
        ],
    )
}

#[test]
fn an_arc_dimensioned_to_the_reach_saves_a_file_the_gate_takes_or_is_refused_naming_it() {
    // Found by break-it, fixed: a driving radius may be 1,000,000 (MAX_LENGTH), and the solve
    // checked the arc's radius *value* against that reach (protocol `unbuildable`,
    // `within_reach`). The file holds no radius for an arc, only its points; the gate,
    // reading the save back, measures the radius from the centre to the start's stored
    // coordinates. The solver meets the arc's rules to within rounding, so that measure came
    // out 1e6 + 1.16e-10 (one ulp past) for about a quarter of start angles (42°, 98°, 105°,
    // 119°, 168°, ...): the solve was Ok, the save wrote the points, and the gate refused the
    // file it wrote ("arc a0000001's radius, from its center to its start, is
    // 1000000.0000000001 mm, more than the 1000000 mm"), so a sketch that solved and was
    // saved couldn't be opened again. Now a solve is checked as its file would hold it
    // (`sketch::savable`): one the gate would refuse is an invalid result naming the arc.
    let mut w = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let (mut refused, mut saved_ok, mut unsavable, mut unsettled) = (Vec::new(), 0, 0, 0);
    for start in (0..360).step_by(7) {
        let doc = checked(&arc_at_the_reach(f64::from(start)));
        let solution = match solve(&doc, &feature(), &mut w) {
            Ok(solution) => solution,
            Err(e) if e.code() == ErrorCode::InvalidResult => {
                assert_eq!(e.elements(), ["a0000001"], "{start}: {e}");
                assert!(e.message().contains("its file can't hold"), "{start}: {e}");
                unsavable += 1;
                continue;
            }
            // Some starts don't solve at all (196°, 210°, 217°: solve_failed, "Converged"),
            // M3.3's to answer. Ten more (14°, 28°, 91°, ...) solved, but every load of their
            // file moved them, by ulps at a kilometre, round and round (not within 30
            // loads): they saved Ok and drifted on every open until loads were checked
            // (Translation::solve), and are refused now as not settling.
            Err(e) => {
                assert_eq!(e.code(), ErrorCode::SolveFailed, "{start}: {e}");
                unsettled += usize::from(e.message().contains("doesn't settle on load"));
                continue;
            }
        };
        saved_ok += 1;
        let mut document = doc.document().clone();
        let Operation::Sketch(sketch) = &mut document.features[0].operation else {
            panic!()
        };
        *sketch = solution.written(sketch);
        let text = write(&document).unwrap();
        if let Err(invalid) = validate(&read(text.as_bytes()).unwrap()) {
            refused.push((start, invalid.errors()[0].message().to_owned()));
        }
    }
    assert!(
        refused.is_empty(),
        "{} saves refused: {refused:#?}",
        refused.len()
    );
    // Both ways happen, so neither half of this is empty.
    assert!(
        saved_ok >= 30 && unsavable > 0,
        "{saved_ok} saved, {unsavable} refused"
    );
    assert_eq!(unsettled, 10, "refused as not settling on load");
}

#[test]
fn an_arc_dimensioned_to_the_reach_saved_solves_to_the_same_file() {
    let mut w = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let mut checked_once = 0;
    for start in [0.0, 30.0, 45.0, 123.0, 270.0] {
        // A start whose solve can't be saved is refused (the test above).
        let doc = checked(&arc_at_the_reach(start));
        if solve(&doc, &feature(), &mut w).is_err() {
            continue;
        }
        let once = saved(&arc_at_the_reach(start), &mut w);
        let twice = saved(&once, &mut w);
        assert_eq!(
            twice, once,
            "start {start}: a saved solve moved when solved again"
        );
        checked_once += 1;
    }
    assert!(checked_once >= 3, "{checked_once}");
}

// --- Started again only from the same build ----------------------------------------------

/// A fresh directory in cargo's temporary directory for tests, under the target directory:
/// on the build's filesystem, so the worker can be hard-linked into it (the system's
/// temporary directory is another filesystem on the Linux runners: EXDEV).
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("silico-break-it-m34-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// p0 held at the origin: a sketch any worker solves at once.
fn small() -> String {
    document(
        &[point("p0000000", 3.0, 4.0)],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#.to_owned(),
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#.to_owned(),
        ],
    )
}

#[test]
fn a_worker_whose_executable_changes_as_it_starts_isnt_used() {
    // The executable is replaced between the check and the worker's start: here by the
    // worker itself (a script that puts other bytes in its place, then runs the worker), so
    // always after the check. The worker running may be either build: its answer isn't
    // taken, and the solve fails as a change.
    let dir = scratch("changed-as-started");
    let exe = dir.join("silico-sketch-worker");
    let tmp = dir.join("next");
    let script = format!(
        "#!/bin/sh\n# build 1\nprintf '#!/bin/sh\\n# build 2\\n' > '{tmp}' && mv '{tmp}' '{exe}'\nexec '{WORKER}'\n",
        tmp = tmp.display(),
        exe = exe.display(),
    );
    std::fs::write(&exe, script).unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut w = SketchWorker::start_worker(&exe).unwrap();
    let refused = w.pid();
    let e = solve(&checked(&small()), &feature(), &mut w).unwrap_err();
    assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
    assert!(e.message().contains("changed since it started"), "{e}");
    assert!(e.message().contains(&exe.display().to_string()), "{e}");
    // And the worker refused is gone (stopped and reaped), not left running.
    let alive = std::process::Command::new("kill")
        .args(["-0", &refused.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "the refused worker {refused} is still running");
}

#[test]
fn a_worker_whose_executable_was_replaced_isnt_started_again_and_one_put_back_is() {
    // Linked, not copied: macOS holds a newly written executable at launch while it assesses
    // it (the catalog's worker-held-at-launch entry).
    let dir = scratch("replaced");
    let exe = dir.join("silico-sketch-worker");
    std::fs::hard_link(WORKER, &exe).unwrap();
    let mut w = SketchWorker::start_worker(&exe).unwrap();
    let doc = checked(&small());
    solve(&doc, &feature(), &mut w).unwrap();

    // Another build in its place, and the worker stopped: not started from it.
    std::fs::remove_file(&exe).unwrap();
    std::fs::hard_link(OTHER_BUILD, &exe).unwrap();
    w.distrust();
    // No worker is started from it: the stopped one is still the one held.
    let stopped = w.pid();
    let e = solve(&doc, &feature(), &mut w).unwrap_err();
    assert_eq!(
        w.pid(),
        stopped,
        "a worker was started from the other build"
    );
    assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
    assert!(e.message().contains("changed since it started"), "{e}");
    assert!(
        e.suggestion().is_some_and(|s| s.contains("restart")),
        "{e:?}"
    );
    assert_eq!(e.feature(), Some(FEATURE), "{e:?}");
    // Asked again, still refused: not started from the other build on a second try.
    let e = solve(&doc, &feature(), &mut w).unwrap_err();
    assert!(e.message().contains("changed since it started"), "{e}");
    assert_eq!(
        w.pid(),
        stopped,
        "a worker was started from the other build"
    );

    // The same build back (the same file, linked again): started again.
    std::fs::remove_file(&exe).unwrap();
    std::fs::hard_link(WORKER, &exe).unwrap();
    solve(&doc, &feature(), &mut w).unwrap();

    // Gone: an error that says what to fix, not a panic.
    std::fs::remove_file(&exe).unwrap();
    w.distrust();
    let e = solve(&doc, &feature(), &mut w).unwrap_err();
    assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
    assert!(e.message().contains(&exe.display().to_string()), "{e}");
    assert!(e.suggestion().is_some(), "{e:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_worker_killed_from_outside_while_idle_is_started_again() {
    let mut w = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let doc = checked(&small());
    solve(&doc, &feature(), &mut w).unwrap();
    let pid = w.pid();
    std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    solve(&doc, &feature(), &mut w).unwrap();
    assert_ne!(w.pid(), pid);
}

// --- A long solve stopped ------------------------------------------------------------------

/// A chain of `n` points, each 1 from the next, held `reach` apart: unreachable, so every
/// algorithm fails, slowly (512 points: a minute or more).
fn impossible_chain(n: usize, reach: f64) -> String {
    let points: Vec<String> = (0..n)
        .map(|i| point(&format!("p{i:07}"), i as f64 * 0.7, (i % 2) as f64 * 0.5))
        .collect();
    let mut constraints = vec![
        r#"{"id": "a0000001", "type": "distance_x", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#.to_owned(),
        r#"{"id": "a0000002", "type": "distance_y", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#.to_owned(),
        format!(
            r#"{{"id": "a0000003", "type": "distance_x", "a": "origin", "b": "p{:07}", "value": {reach}, "driving": true}}"#,
            n - 1
        ),
    ];
    for i in 1..n {
        constraints.push(format!(
            r#"{{"id": "d{i:07}", "type": "distance", "a": "p{:07}", "b": "p{i:07}", "value": 1, "driving": true}}"#,
            i - 1
        ));
    }
    document(&points, &[], &constraints)
}

#[test]
fn a_solve_stopped_mid_run_fails_at_once_and_the_next_solve_starts_a_worker() {
    let mut w = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let stopper = w
        .stopper()
        .unwrap()
        .expect("the worker's solves can be stopped");
    let pid = w.pid();
    let doc = checked(&impossible_chain(512, 2560.0));
    let started = Instant::now();
    let running = std::thread::spawn(move || {
        let e = solve(&doc, &feature(), &mut w).unwrap_err();
        (e, w)
    });
    std::thread::sleep(Duration::from_millis(500));
    stopper.stop();
    let (e, mut w) = running.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the stopped solve ran on: {:?}",
        started.elapsed()
    );
    assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
    assert_eq!(e.feature(), Some(FEATURE));
    // The next solve, readied as the engine readies one (a stopped solve stays stopped
    // until a new one begins: round 2), runs on a new worker.
    w.stopper().unwrap();
    solve(&checked(&small()), &feature(), &mut w).unwrap();
    assert_ne!(w.pid(), pid, "the killed worker is still the one asked");
}

/// A kernel for documents with no solids: sketches alone ask nothing of it.
struct NoSolids(Mutex<Generation>);

impl Kernel for NoSolids {
    fn identity(&self) -> &str {
        "no solids"
    }

    fn generation(&self) -> Generation {
        *self.0.lock().unwrap()
    }

    fn advance(&self, generation: Generation) {
        let mut g = self.0.lock().unwrap();
        if generation > *g {
            *g = generation;
        }
    }

    fn submit<T: Send + 'static>(&self, _: Generation, _: KernelRequest<T>) -> Pending<T> {
        panic!("a sketch-only document asked the kernel for a solid")
    }
}

fn engine() -> Recompute<NoSolids> {
    Recompute::new(NoSolids(Mutex::new(Generation(0))))
        .with_sketches(SketchWorker::start_worker(Path::new(WORKER)).unwrap())
}

#[test]
fn a_recompute_superseded_during_a_long_solve_returns_at_once_and_the_newer_one_solves() {
    let engine = engine();
    let started = Instant::now();
    let older = engine.start(checked(&impossible_chain(512, 2560.0)));
    std::thread::sleep(Duration::from_millis(500));
    let newer = engine.start(checked(&small()));
    match older.wait_timeout(Duration::from_secs(20)) {
        Ok(Ok(RunOutcome::Superseded)) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the superseded recompute waited out its solve"),
    }
    match newer.wait_timeout(Duration::from_secs(20)) {
        Ok(Ok(RunOutcome::Published(model))) => {
            assert!(matches!(
                model.outcome(&feature()),
                Some(FeatureOutcome::Sketch(_))
            ));
        }
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the newer recompute waited behind the older one's solve"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_recompute_dropped_during_a_long_solve_stops_it() {
    let engine = engine();
    let run = engine.start(checked(&impossible_chain(512, 2560.0)));
    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();
    drop(engine);
    match run.wait_timeout(Duration::from_secs(20)) {
        Ok(Ok(RunOutcome::Superseded)) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("a dropped engine's solve ran on"),
    }
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
fn a_recompute_cancelled_during_a_long_solve_returns_and_the_engine_solves_again() {
    let engine = engine();
    let run = engine.start(checked(&impossible_chain(512, 2560.0)));
    std::thread::sleep(Duration::from_millis(500));
    engine.cancel();
    match run.wait_timeout(Duration::from_secs(20)) {
        Ok(Ok(RunOutcome::Superseded)) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the cancelled recompute waited out its solve"),
    }
    match engine
        .start(checked(&small()))
        .wait_timeout(Duration::from_secs(20))
    {
        Ok(Ok(RunOutcome::Published(model))) => assert!(matches!(
            model.outcome(&feature()),
            Some(FeatureOutcome::Sketch(_))
        )),
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the engine didn't solve after a cancel"),
    }
}

#[test]
fn a_stop_that_lands_after_the_solve_returned_doesnt_fail_the_next_solve() {
    // Found by the council's robustness lens: a newer recompute's start stops the older's
    // solve while the older is still reading its reply (or running `savable`); the kill
    // lands on a worker that has answered and sits idle, and no call fails to mark it
    // stopped. The next solve (the newer recompute's) found the dead worker not yet reaped
    // and failed as worker_crashed, publishing the sketch as failed.
    let text = small();
    let doc = checked(&text);
    let mut failed = Vec::new();
    for round in 0..10 {
        let mut w = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
        let stopper = w.stopper().unwrap().unwrap();
        solve(&doc, &feature(), &mut w).unwrap();
        stopper.stop();
        w.stopper().unwrap();
        if let Err(e) = solve(&doc, &feature(), &mut w) {
            failed.push((round, e.to_string()));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
fn a_second_supersede_on_one_engine_stops_the_solve_too() {
    // From the council's tests lens and its skeptic: every test stopped at most one solve
    // per worker, so a stopper handed out for a worker already killed (taken before the
    // worker was started again) passed them; the second supersede then killed nothing, and
    // its recompute waited out the solve. The skeptic's mutant (no `ready` in `stopper`)
    // failed this after 20 s.
    let engine = engine();
    let first = engine.start(checked(&impossible_chain(512, 2560.0)));
    std::thread::sleep(Duration::from_millis(500));
    let second = engine.start(checked(&impossible_chain(512, 2560.0)));
    match first.wait_timeout(Duration::from_secs(20)) {
        Ok(Ok(RunOutcome::Superseded)) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the first superseded recompute waited out its solve"),
    }
    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();
    let third = engine.start(checked(&small()));
    match second.wait_timeout(Duration::from_secs(20)) {
        Ok(Ok(RunOutcome::Superseded)) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!(
            "the second superseded recompute waited out its solve: {:?}",
            started.elapsed()
        ),
    }
    match third.wait_timeout(Duration::from_secs(20)) {
        Ok(Ok(RunOutcome::Published(_))) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the newest recompute didn't publish"),
    }
}

#[test]
fn a_stop_landing_between_a_solves_two_requests_stops_the_second_too() {
    // Round 2's robustness lens: a stop that landed between a solve's requests (after the
    // first answered, before a re-diagnosis from its solution) killed the idle worker, and
    // the second request started a new one and ran to the end: 125 s on the impossible
    // chain, while the newer recompute waited. A stopped solve's later requests are refused.
    let mut w = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let stopper = w.stopper().unwrap().unwrap();
    solve(&checked(&small()), &feature(), &mut w).unwrap();
    stopper.stop();
    let started = Instant::now();
    let e = solve(&checked(&impossible_chain(512, 2560.0)), &feature(), &mut w).unwrap_err();
    let took = started.elapsed();
    assert!(
        took < Duration::from_millis(500),
        "a stopped solve's next request ran {took:?}"
    );
    assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
    // The next solve, readied as the engine readies one, runs.
    w.stopper().unwrap();
    solve(&checked(&small()), &feature(), &mut w).unwrap();
}

#[test]
fn a_worker_that_dies_after_its_solve_began_isnt_replaced_out_of_its_stoppers_reach() {
    // Round 3's robustness lens: the worker died after the engine took its stopper (the
    // solve's start), and the solve's request started a new worker the stopper couldn't
    // reach; a stop then killed nothing, and the request ran on (10 s and more, the
    // impossible chain) while a newer recompute waited. Once a solve has begun, it starts
    // no worker: a dead one fails it as a crash, and the next solve's start replaces it.
    let mut w = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let stopper = w.stopper().unwrap().unwrap();
    let old = w.pid();
    std::process::Command::new("kill")
        .args(["-9", &old.to_string()])
        .status()
        .unwrap();
    let doc = checked(&impossible_chain(512, 2560.0));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = solve(&doc, &feature(), &mut w);
        let _ = tx.send((r.map(|_| ()), w));
    });
    std::thread::sleep(Duration::from_millis(500));
    stopper.stop();
    let (result, mut w) = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the solve ran on after its stop, on a worker its stopper can't reach");
    assert_eq!(result.unwrap_err().code(), ErrorCode::WorkerCrashed);
    // The next solve's start replaces the dead worker.
    w.stopper().unwrap();
    solve(&checked(&small()), &feature(), &mut w).unwrap();
    assert_ne!(w.pid(), old);
}
