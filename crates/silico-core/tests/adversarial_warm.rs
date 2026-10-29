//! break-it: sketch solvers' warm start (`SketchSolvers::warm`) and the take that waits for
//! it, reached as an app reaches them: a drag's begin takes a solver (and holds it while the
//! drag is open), its end gives it back. Starts are counted by the starter itself, so each
//! expected count is the number of solvers asked for, worked by hand: the warmed ones, and
//! one more for each take that found none idle and none warming.
//!
//! A warm start waits until its solvers have started; the tests that hold one run it on a
//! thread of its own ([`warming`]), as an app runs it off its UI thread. Some leave that
//! thread blocked when they finish; they rely on a process of their own, as nextest runs
//! each test in.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use silico_core::drag::{Begun, DragSession, begin};
use silico_core::fake::FakeSketches;
use silico_core::recompute::SketchSolvers;
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};
use silico_protocol::{
    Algorithm, DragBegin, Geometry, PointOf, PointValues, SketchGeometry, SketchSystem, Value,
};

use crate::model::WAIT;

/// A lone point at (4, 8), free, and a fixed point at (1, 1).
fn two_points() -> DragBegin {
    DragBegin {
        system: SketchSystem {
            values: vec![4.0, 8.0, 1.0, 1.0],
            unknowns: vec![Value(0), Value(1)],
            geometry: vec![
                SketchGeometry::Point(PointValues {
                    x: Value(0),
                    y: Value(1),
                }),
                SketchGeometry::Point(PointValues {
                    x: Value(2),
                    y: Value(3),
                }),
            ],
            constraints: vec![],
            algorithms: vec![Algorithm::DogLeg],
        },
        dragged: silico_protocol::Dragged::Point(PointOf::Point(Geometry(0))),
    }
}

fn fake() -> Result<Box<dyn SketchSolver>, Error> {
    Ok(Box::new(FakeSketches::new()))
}

/// Shut until opened.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    fn wait(&self) {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        while !*open {
            open = self.opened.wait(open).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

/// Solvers whose starter numbers each start from 0 and runs `start` with its number.
fn counted(
    start: impl Fn(usize) -> Result<Box<dyn SketchSolver>, Error> + Send + 'static,
) -> (SketchSolvers, Arc<AtomicUsize>) {
    let starts = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&starts);
    let solvers = SketchSolvers::starting(move || start(count.fetch_add(1, Ordering::SeqCst)));
    (solvers, starts)
}

/// A drag begun on `solvers` on a thread of its own: its session (holding its solver), or
/// its error, or `None` if it didn't come back within `within`.
fn begun(solvers: &SketchSolvers, within: Duration) -> Option<Result<DragSession, Error>> {
    let solvers = solvers.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(begin(&solvers, two_points()).map(|b| match b {
            Begun::Open { session, .. } => session,
            Begun::Conflicting(tags) => panic!("{tags:?}"),
        }));
    });
    rx.recv_timeout(within).ok()
}

/// `solvers.warm(n)` on a thread of its own, as an app runs it: how many it started, once
/// it has.
fn warming(solvers: &SketchSolvers, n: usize) -> mpsc::Receiver<usize> {
    let solvers = solvers.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(solvers.warm(n));
    });
    rx
}

fn wait_for(what: &str, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < WAIT, "{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_warm_start_of_usize_max_whose_starts_fail_never_lets_a_solve_start_its_own() {
    // Found by break-it, fixed (a warm start is bounded by MAX_WORKERS, and its first
    // failed start ends it): `warm(usize::MAX)` counted `usize::MAX` solvers warming, and
    // a take waits while any are: each start fails (here after a millisecond: a sketch worker that can't be
    // started), so the warm start's thread goes on failing for ever, and a drag's begin
    // waits on it rather than starting a solver of its own, as it does after a smaller warm
    // start's failures (recompute.rs, `SketchSolvers::warm` takes `n` unbounded; `take`
    // waits on `warming`). The kernel pool's warm start is bounded by its slots.
    let (solvers, starts) = counted(|_| {
        std::thread::sleep(Duration::from_millis(1));
        Err(Error::new(ErrorCode::WorkerUnavailable, "no sketch worker"))
    });
    // Its first start fails, and ends it: none started, at once.
    assert_eq!(
        warming(&solvers, usize::MAX)
            .recv_timeout(Duration::from_secs(5))
            .expect("the warm start went on failing"),
        0
    );
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    let err = begun(&solvers, Duration::from_secs(5))
        .unwrap_or_else(|| {
            panic!(
                "a drag's begin waited 5 s on a warm start of usize::MAX solvers, all failing \
                 ({} starts so far)",
                starts.load(Ordering::SeqCst)
            )
        })
        .err()
        .expect("a solver, though every start fails");
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
}

#[test]
fn a_warm_start_after_a_huge_one_and_a_solver_given_back_counts_without_overflow() {
    // Found by break-it, fixed (bounded, and counted with saturation): a panic, with
    // overflow checks: after `warm(usize::MAX)` the solvers idle and
    // warming add to `usize::MAX`; a solver given back that the warm start didn't start (a
    // drag's, started before it) makes them one more, and the next `warm` adds them,
    // `idle.solvers.len() + idle.warming`, past `usize::MAX` (recompute.rs,
    // `SketchSolvers::warm`).
    let gate = Arc::new(Gate::default());
    let held = Arc::clone(&gate);
    // The drag's start (0) and the warm start's first two (1, 2) are had; the warm start's
    // third waits for ever.
    let (solvers, starts) = counted(move |n| {
        if n >= 3 {
            held.wait();
        }
        fake()
    });
    let drag = begun(&solvers, WAIT)
        .expect("the drag's begin waited")
        .unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    // As many as the machine has cores, held at its third.
    let _held = warming(&solvers, usize::MAX);
    wait_for("the warm start's third start never came", || {
        starts.load(Ordering::SeqCst) >= 4
    });
    // Given back: idle three, and the rest warming.
    drag.end(None).unwrap();
    let warmed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| solvers.warm(1)));
    assert_eq!(
        warmed.ok(),
        Some(0),
        "three idle: a warm start of one starts none"
    );
}

#[test]
fn warm_starts_from_many_threads_at_once_start_exactly_the_number_asked_for() {
    let gate = Arc::new(Gate::default());
    let held = Arc::clone(&gate);
    let (solvers, starts) = counted(move |_| {
        held.wait();
        fake()
    });
    let threads: Vec<_> = (0..8).map(|_| warming(&solvers, 3)).collect();
    // One start under way (the starts take turns), and none more until it ends.
    wait_for("no warm start began", || starts.load(Ordering::SeqCst) == 1);
    gate.open();
    let started: usize = threads
        .into_iter()
        .map(|t| t.recv_timeout(WAIT).expect("a warm start waited"))
        .sum();
    assert_eq!(started, 3, "eight warm starts of three at once");
    // Three drags take the three warmed; a fourth starts one of its own.
    let drags: Vec<DragSession> = (0..3)
        .map(|_| begun(&solvers, WAIT).expect("a begin waited").unwrap())
        .collect();
    assert_eq!(starts.load(Ordering::SeqCst), 3);
    let fourth = begun(&solvers, WAIT).expect("a begin waited").unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 4);
    drop(drags);
    drop(fourth);
}

#[test]
fn many_solves_that_come_while_a_warm_start_is_held_each_get_a_solver_and_none_more_start() {
    let gate = Arc::new(Gate::default());
    let held = Arc::clone(&gate);
    let (solvers, starts) = counted(move |_| {
        held.wait();
        fake()
    });
    let warmed = warming(&solvers, 2);
    wait_for("the warm start never began", || {
        starts.load(Ordering::SeqCst) == 1
    });
    let (tx, rx) = mpsc::channel();
    for _ in 0..8 {
        let (solvers, tx) = (solvers.clone(), tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(begin(&solvers, two_points()));
        });
    }
    drop(tx);
    std::thread::sleep(Duration::from_millis(50));
    // Every begin waits for the warm start: the one start under way (the warm start's
    // first), and none of their own.
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    gate.open();
    let mut sessions = Vec::new();
    for _ in 0..8 {
        let begun = rx.recv_timeout(WAIT).expect("a begin waited").unwrap();
        sessions.push(begun);
    }
    // Each holds its solver: the warm start's two, and six started for the rest.
    assert_eq!(starts.load(Ordering::SeqCst), 8);
    assert_eq!(warmed.recv_timeout(WAIT).expect("the warm start waited"), 2);
}

#[test]
fn a_solver_given_back_while_a_solve_waits_on_a_warm_start_is_taken_at_once() {
    let gate = Arc::new(Gate::default());
    let held = Arc::clone(&gate);
    // The first start (the drag's own) is had at once; the warm start's waits on the gate.
    let (solvers, starts) = counted(move |n| {
        if n >= 1 {
            held.wait();
        }
        fake()
    });
    let first = begun(&solvers, WAIT).expect("a begin waited").unwrap();
    let warmed = warming(&solvers, 1);
    wait_for("the warm start never began", || {
        starts.load(Ordering::SeqCst) == 2
    });
    // This one waits on the warm start, held; the first drag's end gives its solver back,
    // and it takes that one without the warm start's.
    let solvers2 = solvers.clone();
    let waiting = std::thread::spawn(move || begin(&solvers2, two_points()).is_ok());
    std::thread::sleep(Duration::from_millis(50));
    first.end(None).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(waiting.join().unwrap());
    });
    assert!(
        rx.recv_timeout(WAIT)
            .expect("the begin waited for the warm start"),
        "the begin failed"
    );
    // The drag's start, and the warm start's, held: none of the begin's own.
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    gate.open();
    assert_eq!(warmed.recv_timeout(WAIT).expect("the warm start waited"), 1);
}

#[test]
fn a_warm_start_whose_starter_panics_partway_leaves_the_rest_to_the_solves_and_warms_again() {
    // The warm start's second start panics, and the warm start with it (its caller's to
    // catch): its third is never tried, and counted off as it unwinds. Three begins: the
    // warm start's first, then two started their own (numbers 2 and 3). The starter's
    // lock, poisoned, is had again.
    let (solvers, starts) = counted(|n| {
        if n == 1 {
            panic!("the warm start's second start panicked");
        }
        fake()
    });
    let warmed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| solvers.warm(3)));
    assert!(warmed.is_err(), "the starter's panic");
    let sessions: Vec<DragSession> = (0..3)
        .map(|_| begun(&solvers, WAIT).expect("a begin waited").unwrap())
        .collect();
    assert_eq!(starts.load(Ordering::SeqCst), 4);
    // None idle and none warming: a warm start of one starts one.
    assert_eq!(solvers.warm(1), 1);
    assert_eq!(starts.load(Ordering::SeqCst), 5);
    // And a begin takes it, starting none.
    let fourth = begun(&solvers, WAIT).expect("a begin waited").unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 5);
    drop(sessions);
    drop(fourth);
}

#[test]
fn every_handle_dropped_mid_warm_start_frees_the_solvers_once_it_ends() {
    // The starter holds a token: once the warm start ends, nothing holds the solvers, so
    // their starter, and its token, are dropped.
    struct Token(mpsc::Sender<()>);
    impl Drop for Token {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let (tx, dropped) = mpsc::channel();
    let token = Token(tx);
    let gate = Arc::new(Gate::default());
    let held = Arc::clone(&gate);
    let solvers = SketchSolvers::starting(move || {
        let _ = &token;
        held.wait();
        fake()
    });
    let warmed = warming(&solvers, 2);
    drop(solvers);
    assert!(
        dropped.recv_timeout(Duration::from_millis(100)).is_err(),
        "dropped while the warm start ran"
    );
    gate.open();
    assert_eq!(warmed.recv_timeout(WAIT).expect("the warm start waited"), 2);
    dropped
        .recv_timeout(WAIT)
        .expect("the solvers were never freed after the warm start ended");
}

#[test]
fn a_warm_start_of_zero_or_of_none_more_starts_nothing_and_a_solve_starts_its_own() {
    let (solvers, starts) = counted(|_| fake());
    assert_eq!(solvers.warm(0), 0);
    let drag = begun(&solvers, WAIT).expect("a begin waited").unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    drag.end(None).unwrap();
    // One idle: a warm start of one starts none, and the next begin takes it.
    assert_eq!(solvers.warm(1), 0);
    let drag = begun(&solvers, WAIT).expect("a begin waited").unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    drop(drag);
}

#[test]
fn sessions_on_a_kernel_with_no_workers_to_start_warm_none_of_it() {
    // From #263's mutants: a kernel that isn't a pool (the fake) has no workers to warm, so
    // its half of the warm start is none; the sketch solvers' half starts as asked.
    use silico_core::cache::Cache;
    use silico_core::command::Sessions;
    use silico_core::fake::FakeKernel;
    let sessions = Sessions::with_stand_ins(
        Arc::new(FakeKernel::new(2).unwrap()),
        |dir: &std::path::Path| Cache::unsealed(dir),
        fake,
    );
    assert_eq!(sessions.warm(2, 1), (0, 1));
}
