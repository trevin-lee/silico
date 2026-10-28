//! break-it: `silico_core::drag` on the fake sketch solver, and on a scripted solver that
//! fails where a test says. Races of moves, ends, Esc, drops, and worker failures at every
//! interleaving; tickets that must each answer once; replies a worker can't be trusted for;
//! panics on the drag's thread. Expected values are by hand: the fake answers a move from
//! the begin state alone, the dragged point at the cursor nudged by a thousandth of where
//! it began ((4, 8) here), and solves a sketch where it's sent.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use silico_core::drag::{Begun, DragSession, Ended, Kept, MoveOutcome, MoveTicket, begin};
use silico_core::fake::{DragCall, FakeSketches};
use silico_core::recompute::SketchSolvers;
use silico_core::sketch::{DragStarted, SketchSolver, SolveStopper};
use silico_core::{Error, ErrorCode};
use silico_protocol::{
    Algorithm, DragBegin, Geometry, Moved, PointOf, PointValues, SketchGeometry, SketchSystem,
    SolveStatus, Solved, Value,
};

use crate::model::WAIT;

/// Solvers that start fakes, and every fake started, in order.
fn pool() -> (SketchSolvers, Arc<Mutex<Vec<FakeSketches>>>) {
    let started: Arc<Mutex<Vec<FakeSketches>>> = Arc::default();
    let list = Arc::clone(&started);
    let solvers = SketchSolvers::starting(move || {
        let fake = FakeSketches::new();
        list.lock().unwrap().push(fake.clone());
        Ok(Box::new(fake) as Box<dyn SketchSolver>)
    });
    (solvers, started)
}

/// A lone point at (4, 8), free, and a fixed point at (1, 1): values 2 and 3 are fixed.
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
        point: PointOf::Point(Geometry(0)),
    }
}

fn open(solvers: &SketchSolvers) -> DragSession {
    match begin(solvers, two_points()).unwrap() {
        Begun::Open { session, .. } => session,
        Begun::Conflicting(tags) => panic!("{tags:?}"),
    }
}

/// The fake's values for the begin state dragged to `to`, worked by hand.
fn dragged_to(to: [f64; 2]) -> Vec<f64> {
    vec![to[0] + 4.0 / 1000.0, to[1] + 8.0 / 1000.0, 1.0, 1.0]
}

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|v| v.to_bits()).collect()
}

fn moves_of(calls: &[DragCall]) -> Vec<[f64; 2]> {
    calls
        .iter()
        .filter_map(|c| match c {
            DragCall::Move(to) => Some(*to),
            _ => None,
        })
        .collect()
}

/// `f` on a thread of its own: its result, or none if it didn't finish within [`WAIT`] (a
/// hang; the thread is left behind).
fn within<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(f());
    });
    receive.recv_timeout(WAIT).ok()
}

/// A tiny deterministic generator for the interleaving sweep.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

// --- A scripted solver: the fake, failing where told ----------------------------------

#[derive(Default)]
struct Script {
    /// `stopper` calls so far.
    stoppers: usize,
    /// The `stopper` call (from 1) that fails as a worker that can't be started does.
    fail_stopper_at: Option<usize>,
}

struct Scripted {
    inner: FakeSketches,
    script: Arc<Mutex<Script>>,
}

impl SketchSolver for Scripted {
    fn solve(&mut self, system: SketchSystem) -> Result<Solved, Error> {
        self.inner.solve(system)
    }

    fn stopper(&mut self) -> Result<Option<SolveStopper>, Error> {
        let mut script = self.script.lock().unwrap();
        script.stoppers += 1;
        if Some(script.stoppers) == script.fail_stopper_at {
            return Err(Error::new(
                ErrorCode::WorkerUnavailable,
                "the scripted sketch worker couldn't be started again",
            ));
        }
        drop(script);
        self.inner.stopper()
    }

    fn drag_begin(&mut self, begin: DragBegin) -> Result<DragStarted, Error> {
        self.inner.drag_begin(begin)
    }

    fn drag_move(&mut self, to: [f64; 2], timeout: Duration) -> Result<Moved, Error> {
        self.inner.drag_move(to, timeout)
    }

    fn drag_end(&mut self) -> Result<(), Error> {
        self.inner.drag_end()
    }
}

/// Solvers that start scripted fakes sharing `script`, and how many were started.
fn scripted_pool(script: &Arc<Mutex<Script>>) -> (SketchSolvers, Arc<Mutex<Vec<FakeSketches>>>) {
    let started: Arc<Mutex<Vec<FakeSketches>>> = Arc::default();
    let list = Arc::clone(&started);
    let script = Arc::clone(script);
    let solvers = SketchSolvers::starting(move || {
        let fake = FakeSketches::new();
        list.lock().unwrap().push(fake.clone());
        Ok(Box::new(Scripted {
            inner: fake,
            script: Arc::clone(&script),
        }) as Box<dyn SketchSolver>)
    });
    (solvers, started)
}

// --- Panics on the drag's thread --------------------------------------------------------

#[test]
fn a_solver_that_panics_mid_move_still_answers_the_ticket_and_lets_the_drag_end() {
    // Broke, before the fix: a panic under `drag_move` unwinds the drag's thread before it sets the move's
    // outcome or the drag's end, so `MoveTicket::wait` and `DragSession::end` (and the
    // session's drop) wait on the condvar forever, and the solver is never given back. The
    // recompute guards exactly this for its solves (`LentSolver`: "given back ... even by a
    // panic under a solve, ... so the next recompute doesn't wait for it forever").
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.answer_moves(|_, _| panic!("a bug in the sketch solver"));
    let got = within(move || {
        let outcome = session.move_to([1.0, 1.0]).unwrap().wait();
        let ended = session.end(Some([2.0, 2.0]));
        (outcome, ended)
    });
    let (outcome, ended) = got.expect("the move's ticket or the drag's end never answered");
    assert!(matches!(outcome, MoveOutcome::Failed(_)), "{outcome:?}");
    assert!(ended.is_err());
}

#[test]
fn a_solver_that_panics_in_the_closing_solve_still_lets_the_end_answer() {
    // Broke, before the fix: the same unwinding, from the end's whole-sketch solve: `end` never returns.
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.answer(|_| panic!("a bug in the sketch solver's solve"));
    let ended = within(move || session.end(Some([2.0, 2.0])))
        .expect("the drag's end never answered after its solver panicked");
    assert!(ended.is_err(), "{ended:?}");
}

// --- Tickets ------------------------------------------------------------------------------

#[test]
fn a_ticket_waited_on_a_second_time_answers_instead_of_hanging() {
    // Broke, before the fix: `wait` takes `&self` and removes the outcome, so a second `wait` (or a `wait`
    // after `try_take` took it) finds nothing and blocks forever, even after the drag ended:
    // nothing will ever set it again. A ticket should be consumed by `wait`, or answer again.
    let (solvers, _) = pool();
    let session = open(&solvers);
    let ticket = session.move_to([1.0, 1.0]).unwrap();
    assert!(matches!(ticket.wait(), MoveOutcome::Solved { .. }));
    session.end(None).unwrap();
    let again = within(move || ticket.wait());
    assert!(again.is_some(), "a ticket's second wait never answered");
}

#[test]
fn after_any_end_every_ticket_has_its_outcome_already() {
    // The end joins the drag's thread only once every move it took or replaced has an
    // outcome: after `end` (cursor, Esc, a bad cursor) or a drop, `try_take` finds each.
    for how in 0..4 {
        let (solvers, started) = pool();
        let session = open(&solvers);
        let fake = started.lock().unwrap()[0].clone();
        fake.hold();
        let mut tickets = vec![session.move_to([0.0, 1.0]).unwrap()];
        assert!(fake.wait_held(WAIT));
        tickets.extend((1..5).map(|i| session.move_to([f64::from(i), 1.0]).unwrap()));
        match how {
            0 => {
                fake.release();
                session.end(Some([9.0, 9.0])).unwrap();
            }
            1 => {
                session.end(None).unwrap();
            }
            2 => {
                assert!(session.end(Some([f64::NAN, 0.0])).is_err());
            }
            _ => drop(session),
        }
        fake.release();
        for (i, ticket) in tickets.iter().enumerate() {
            let outcome = ticket.try_take();
            assert!(outcome.is_some(), "how {how}: ticket {i} has no outcome");
            // The first was held solving; 1..3 replaced; the last replaced at the end, or
            // solved after the release (how 0 released before ending).
            match (i, outcome.unwrap()) {
                (0, MoveOutcome::Solved { to, moved }) if how == 0 => {
                    assert_eq!(bits(&moved.values.unwrap()), bits(&dragged_to(to)));
                }
                (0, MoveOutcome::Cancelled) if how != 0 => {}
                (1..=3, MoveOutcome::Dropped) => {}
                (4, MoveOutcome::Dropped) => {}
                (4, MoveOutcome::Solved { to, moved }) if how == 0 => {
                    assert_eq!(bits(&moved.values.unwrap()), bits(&dragged_to(to)));
                }
                (i, other) => panic!("how {how}: ticket {i}: {other:?}"),
            }
        }
        assert_eq!(started.lock().unwrap().len(), 1);
    }
}

#[test]
fn a_ticket_waited_on_another_thread_answers_when_the_session_is_dropped() {
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.hold();
    let ticket = session.move_to([1.0, 1.0]).unwrap();
    assert!(fake.wait_held(WAIT));
    let waiting = std::thread::spawn(move || ticket.wait());
    drop(session);
    fake.release();
    assert_eq!(waiting.join().unwrap(), MoveOutcome::Cancelled);
    // Given back: the next drag takes the same fake.
    open(&solvers).end(None).unwrap();
    assert_eq!(started.lock().unwrap().len(), 1);
}

// --- Many movers at once ---------------------------------------------------------------

#[test]
fn moves_from_many_threads_at_once_each_answer_once_and_only_the_solved_reached_the_worker() {
    // Eight threads send 300 moves each, every cursor its own, on one session: each ticket
    // answers solved (with the fake's values for its own cursor) or dropped; the cursors
    // solved are exactly the moves the worker was sent, in its order; the end is the
    // begin state at the final cursor, bit for bit.
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    let outcomes: Vec<([f64; 2], MoveOutcome)> = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let session = &session;
                scope.spawn(move || {
                    (0..300)
                        .map(|i| {
                            let to = [f64::from(t) * 1000.0 + f64::from(i), -f64::from(i)];
                            (to, session.move_to(to).unwrap().wait())
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect()
    });
    let mut solved = Vec::new();
    for (to, outcome) in &outcomes {
        match outcome {
            MoveOutcome::Solved { to: got, moved } => {
                assert_eq!(got, to);
                assert_eq!(moved.status, SolveStatus::Success);
                assert_eq!(bits(moved.values.as_ref().unwrap()), bits(&dragged_to(*to)));
                solved.push(to.map(f64::to_bits));
            }
            MoveOutcome::Dropped => {}
            other => panic!("{to:?}: {other:?}"),
        }
    }
    let mut sent: Vec<[u64; 2]> = moves_of(&fake.drag_calls())
        .iter()
        .map(|to| to.map(f64::to_bits))
        .collect();
    solved.sort_unstable();
    sent.sort_unstable();
    assert_eq!(solved, sent);
    let end = [-7.25, 3.5];
    let ended = session.end(Some(end)).unwrap();
    assert_eq!(ended.kept, Kept::Cursor);
    assert_eq!(bits(&ended.solved.unwrap().values), bits(&dragged_to(end)));
}

#[test]
fn two_drags_open_at_once_each_hold_their_own_worker() {
    let (solvers, started) = pool();
    let a = open(&solvers);
    let b = open(&solvers);
    let fakes = started.lock().unwrap().clone();
    assert_eq!(fakes.len(), 2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for i in 0..100 {
                a.move_to([f64::from(i), 1.0]).unwrap().wait();
            }
        });
        scope.spawn(|| {
            for i in 0..100 {
                b.move_to([f64::from(i), 2.0]).unwrap().wait();
            }
        });
    });
    let ea = a.end(Some([1.0, 1.0])).unwrap();
    let eb = b.end(Some([2.0, 2.0])).unwrap();
    assert_eq!(
        bits(&ea.solved.unwrap().values),
        bits(&dragged_to([1.0, 1.0]))
    );
    assert_eq!(
        bits(&eb.solved.unwrap().values),
        bits(&dragged_to([2.0, 2.0]))
    );
    // Each fake saw one session's moves alone: one y each.
    let ys: Vec<Vec<u64>> = fakes
        .iter()
        .map(|f| {
            let mut ys: Vec<u64> = moves_of(&f.drag_calls())
                .iter()
                .map(|to| to[1].to_bits())
                .collect();
            ys.dedup();
            ys
        })
        .collect();
    assert_eq!(ys.len(), 2);
    for y in &ys {
        assert_eq!(y.len(), 1, "{ys:?}");
    }
    assert_ne!(ys[0], ys[1]);
}

// --- Every interleaving --------------------------------------------------------------------

/// How one generated session ends.
#[derive(Debug, Clone, Copy)]
enum Finish {
    Cursor([f64; 2]),
    Esc,
    BadCursor,
    Drop,
}

/// Each kind of ticket outcome the sweep saw: solved, dropped, cancelled, failed, closed.
static SEEN: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];

/// One session of random moves, holds, releases, short sleeps, and a failing cursor, ended
/// one of four ways; checks every ticket answered once and what the end kept. Returns the
/// failures it found, as text.
fn session_of(seed: u64, solvers: &SketchSolvers, fake: &FakeSketches) -> Result<(), String> {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let calls_before = fake.drag_calls().len();
    let solves_before = fake.requests().len();
    // One cursor in four sessions fails as a worker timing out does.
    let failing = (rng.below(4) == 0).then(|| [f64::from(rng.below(8) as u32), 0.5]);
    fake.answer_moves(move |b, to| {
        if Some(to) == failing {
            Err(Error::new(
                ErrorCode::WorkerTimeout,
                "the fake move took too long",
            ))
        } else {
            Ok(FakeSketches::dragged(b, to))
        }
    });
    let session = open(solvers);
    let mut tickets: Vec<([f64; 2], MoveTicket)> = Vec::new();
    for _ in 0..rng.below(30) {
        match rng.below(6) {
            0..=2 => {
                let to = [f64::from(rng.below(8) as u32), 0.5];
                tickets.push((to, session.move_to(to).unwrap()));
            }
            3 => fake.hold(),
            4 => fake.release(),
            _ => std::thread::sleep(Duration::from_micros(rng.below(300))),
        }
    }
    let finish = match rng.below(4) {
        0 => Finish::Cursor([f64::from(rng.below(8) as u32), 0.5]),
        1 => Finish::Esc,
        2 => Finish::BadCursor,
        _ => Finish::Drop,
    };
    if matches!(finish, Finish::Cursor(_)) {
        // The fake ignores a move's time limit: a held move would never end.
        fake.release();
    }
    let ended: Option<Result<Ended, Error>> = match finish {
        Finish::Cursor(to) => Some(session.end(Some(to))),
        Finish::Esc => Some(session.end(None)),
        Finish::BadCursor => Some(session.end(Some([f64::INFINITY, 0.0]))),
        Finish::Drop => {
            drop(session);
            None
        }
    };
    fake.release();
    let mut failed = false;
    for (i, (to, ticket)) in tickets.iter().enumerate() {
        let Some(outcome) = ticket.try_take() else {
            return Err(format!("{finish:?}: ticket {i} to {to:?} has no outcome"));
        };
        match outcome {
            MoveOutcome::Solved { to: got, moved } => {
                SEEN[0].fetch_add(1, Ordering::Relaxed);
                if got != *to || moved.values.as_deref().map(bits) != Some(bits(&dragged_to(*to))) {
                    return Err(format!(
                        "{finish:?}: ticket {i} to {to:?} solved as {moved:?}"
                    ));
                }
            }
            MoveOutcome::Dropped => {
                SEEN[1].fetch_add(1, Ordering::Relaxed);
            }
            // The sweep's failing move times out, a fault: nothing here is refused.
            MoveOutcome::Refused(e) => {
                return Err(format!("{finish:?}: ticket {i} to {to:?} refused: {e}"));
            }
            MoveOutcome::Cancelled => {
                SEEN[2].fetch_add(1, Ordering::Relaxed);
                if matches!(finish, Finish::Cursor(_)) {
                    return Err(format!("{finish:?}: ticket {i} cancelled by a release"));
                }
            }
            MoveOutcome::Failed(e) => {
                SEEN[3].fetch_add(1, Ordering::Relaxed);
                if Some(*to) != failing || e.code() != ErrorCode::WorkerTimeout {
                    return Err(format!("{finish:?}: ticket {i} to {to:?} failed: {e}"));
                }
                failed = true;
            }
            MoveOutcome::Closed(e) => {
                SEEN[4].fetch_add(1, Ordering::Relaxed);
                if !failed {
                    return Err(format!(
                        "{finish:?}: ticket {i} closed before a failure: {e}"
                    ));
                }
                if e.message() != "no drag is open: its worker failed" {
                    return Err(format!("{finish:?}: ticket {i}: {e}"));
                }
            }
        }
    }
    let calls = fake.drag_calls()[calls_before..].to_vec();
    if !matches!(calls.first(), Some(DragCall::Begin(_))) {
        return Err(format!(
            "{finish:?}: the session's first call was {calls:?}"
        ));
    }
    match (finish, ended, failed) {
        // Esc after a failure: nothing kept, as asked.
        (Finish::Esc, Some(Ok(ended)), true) => {
            if ended.kept != Kept::Begin || ended.solved.is_some() {
                return Err(format!("{finish:?}: kept {ended:?} after a failure"));
            }
        }
        (Finish::Cursor(_) | Finish::BadCursor, Some(Err(e)), true) => {
            if e.message() != "no drag is open: its worker failed"
                && !matches!(finish, Finish::BadCursor)
            {
                return Err(format!("{finish:?}: ended after a failure with {e}"));
            }
        }
        (_, None, _) => {}
        // The end's own move, at the failing cursor: refused with the worker's error.
        (Finish::Cursor(to), Some(Err(e)), false)
            if Some(to) == failing && e.code() == ErrorCode::WorkerTimeout => {}
        (Finish::Cursor(to), Some(Ok(ended)), false) => {
            let want = dragged_to(to);
            if ended.kept != Kept::Cursor
                || ended.solved.as_ref().map(|s| bits(&s.values)) != Some(bits(&want))
            {
                return Err(format!("{finish:?}: kept {ended:?}"));
            }
            let solves = fake.requests()[solves_before..].to_vec();
            if solves.len() != 1 || bits(&solves[0].values) != bits(&want) {
                return Err(format!("{finish:?}: closing solves {solves:?}"));
            }
            if calls.last() != Some(&DragCall::End) {
                return Err(format!(
                    "{finish:?}: the worker's drag wasn't ended: {calls:?}"
                ));
            }
        }
        (Finish::Esc, Some(Ok(ended)), false) => {
            if ended.kept != Kept::Begin || ended.solved.is_some() {
                return Err(format!("{finish:?}: kept {ended:?}"));
            }
        }
        (Finish::BadCursor, Some(Err(e)), false) => {
            if e.parameter() != Some("to") {
                return Err(format!("{finish:?}: {e}"));
            }
        }
        (finish, ended, failed) => {
            return Err(format!("{finish:?}: ended {ended:?}, failed {failed}"));
        }
    }
    Ok(())
}

#[test]
fn every_interleaving_of_moves_holds_failures_and_ends_answers_every_ticket_and_frees_the_worker() {
    // 600 generated sessions on one pool, one after another, each run under WAIT: no hang,
    // every ticket answered, the end as worked out by hand, and the one fake reused by all.
    let (solvers, started) = pool();
    // The pool's first fake, started by a drag begun and ended.
    open(&solvers).end(None).unwrap();
    let fake = started.lock().unwrap()[0].clone();
    for seed in 0..600 {
        let solvers = solvers.clone();
        let session_fake = fake.clone();
        match within(move || session_of(seed, &solvers, &session_fake)) {
            Some(Ok(())) => {}
            Some(Err(e)) => panic!("seed {seed}: {e}"),
            None => panic!("seed {seed}: the session hung"),
        }
    }
    assert_eq!(started.lock().unwrap().len(), 1);
    // The sweep reached every outcome, so it can fail: solved, dropped, cancelled, failed,
    // and closed tickets each came up.
    for (kind, seen) in ["solved", "dropped", "cancelled", "failed", "closed"]
        .iter()
        .zip(&SEEN)
    {
        assert!(
            seen.load(Ordering::Relaxed) > 0,
            "no {kind} ticket in the sweep"
        );
    }
}

// --- Worker failures ---------------------------------------------------------------------

#[test]
fn a_worker_that_cant_be_readied_for_a_move_fails_it_and_gives_its_solver_back() {
    // The begin's `stopper` is the first; the first move's is the second, and fails as a
    // worker that couldn't be started again: that move fails with the worker's error, a
    // later move is closed, the end refused, and the solver back in the pool already.
    let script = Arc::new(Mutex::new(Script {
        fail_stopper_at: Some(2),
        ..Script::default()
    }));
    let (solvers, started) = scripted_pool(&script);
    let session = open(&solvers);
    match session.move_to([1.0, 1.0]).unwrap().wait() {
        MoveOutcome::Failed(e) => assert_eq!(e.code(), ErrorCode::WorkerUnavailable),
        other => panic!("{other:?}"),
    }
    match session.move_to([2.0, 2.0]).unwrap().wait() {
        MoveOutcome::Closed(e) => {
            assert_eq!(e.message(), "no drag is open: its worker failed");
            assert_eq!(
                e.cause(),
                Some("the scripted sketch worker couldn't be started again")
            );
        }
        other => panic!("{other:?}"),
    }
    // Back before the session ends: a second drag takes it, starting none.
    let second = open(&solvers);
    assert_eq!(started.lock().unwrap().len(), 1);
    let e = session.end(Some([2.0, 2.0])).unwrap_err();
    assert_eq!(e.message(), "no drag is open: its worker failed");
    let ended = second.end(Some([3.0, 3.0])).unwrap();
    assert_eq!(
        bits(&ended.solved.unwrap().values),
        bits(&dragged_to([3.0, 3.0]))
    );
}

#[test]
fn a_worker_that_cant_be_readied_for_the_ends_move_refuses_the_end_and_comes_back() {
    // The end's own move (the cursor never solved) finds its worker can't be readied: the end
    // is refused with that error, and the solver is given back for the next drag.
    let script = Arc::new(Mutex::new(Script {
        fail_stopper_at: Some(2),
        ..Script::default()
    }));
    let (solvers, started) = scripted_pool(&script);
    let session = open(&solvers);
    let e = session.end(Some([5.0, 5.0])).unwrap_err();
    assert_eq!(e.code(), ErrorCode::WorkerUnavailable);
    open(&solvers).end(None).unwrap();
    assert_eq!(started.lock().unwrap().len(), 1);
}

#[test]
fn an_esc_after_the_worker_failed_keeps_the_begin_and_the_drop_after_it_is_quiet() {
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.answer_moves(|_, _| Err(Error::new(ErrorCode::WorkerCrashed, "the fake crashed")));
    assert!(matches!(
        session.move_to([1.0, 1.0]).unwrap().wait(),
        MoveOutcome::Failed(_)
    ));
    // Esc asks for nothing kept, which the failure already gave: not an error.
    assert_eq!(
        session.end(None).unwrap(),
        Ended {
            kept: Kept::Begin,
            solved: None,
            why: None
        }
    );
    // A second session, dropped after the same failure, without an end.
    let session = open(&solvers);
    assert!(matches!(
        session.move_to([1.0, 1.0]).unwrap().wait(),
        MoveOutcome::Failed(_)
    ));
    assert!(within(move || drop(session)).is_some());
    assert_eq!(started.lock().unwrap().len(), 1);
}

#[test]
fn a_bad_cursor_at_the_end_stops_the_move_held_solving_and_frees_the_worker() {
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.hold();
    let moving = session.move_to([1.0, 1.0]).unwrap();
    assert!(fake.wait_held(WAIT));
    let e = session.end(Some([0.0, f64::NEG_INFINITY])).unwrap_err();
    assert_eq!(e.parameter(), Some("to"));
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(moving.wait(), MoveOutcome::Cancelled);
    fake.release();
    let again = open(&solvers);
    let ended = again.end(Some([1.0, 1.0])).unwrap();
    assert_eq!(
        bits(&ended.solved.unwrap().values),
        bits(&dragged_to([1.0, 1.0]))
    );
    assert_eq!(started.lock().unwrap().len(), 1);
}

// --- The end's determinism -----------------------------------------------------------------

#[test]
fn an_end_at_minus_zero_after_a_move_to_zero_gives_the_zero_move_drags_sketch() {
    // Bits differ (−0 and +0), so the end moves once more; the sketch kept is the same as a
    // drag ended at −0 with no moves at all.
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    session.move_to([0.0, 0.0]).unwrap().wait();
    let after = session.end(Some([-0.0, 0.0])).unwrap();
    assert_eq!(moves_of(&fake.drag_calls()), [[0.0, 0.0], [-0.0, 0.0]]);
    let none = open(&solvers).end(Some([-0.0, 0.0])).unwrap();
    assert_eq!(after, none);
    assert_eq!(
        bits(&after.solved.unwrap().values),
        bits(&dragged_to([-0.0, 0.0]))
    );
}

#[test]
fn an_end_at_a_cursor_whose_newest_move_failed_moves_again_and_keeps_the_begin() {
    // (5, 5) solved, then (6, 6) answered `Failed`: the newest move that reached its cursor
    // is still (5, 5), but the end is at (6, 6): one more move, `Failed` again, the begin kept.
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.answer_moves(|b, to| {
        Ok(if to == [6.0, 6.0] {
            Moved {
                status: SolveStatus::Failed,
                values: None,
            }
        } else {
            FakeSketches::dragged(b, to)
        })
    });
    session.move_to([5.0, 5.0]).unwrap().wait();
    session.move_to([6.0, 6.0]).unwrap().wait();
    let ended = session.end(Some([6.0, 6.0])).unwrap();
    assert_eq!((ended.kept, ended.solved), (Kept::Begin, None));
    let why = ended.why.unwrap();
    assert_eq!(
        (why.code(), why.message()),
        (
            ErrorCode::SolveFailed,
            "the solver didn't reach the drag's final cursor from where the drag began"
        )
    );
    assert_eq!(
        moves_of(&fake.drag_calls()),
        [[5.0, 5.0], [6.0, 6.0], [6.0, 6.0]]
    );
    assert!(fake.requests().is_empty());
}

#[test]
fn a_closing_solve_that_doesnt_solve_keeps_the_begin_state() {
    for status in [
        SolveStatus::Failed,
        SolveStatus::Converged,
        SolveStatus::SolvedInvalid,
    ] {
        let (solvers, started) = pool();
        let session = open(&solvers);
        let fake = started.lock().unwrap()[0].clone();
        // A solution has its algorithm, and a failure none, as a worker answers them.
        fake.answer(move |s| {
            let sent = FakeSketches::as_sent(s);
            Ok(Solved {
                status,
                algorithm: sent
                    .algorithm
                    .filter(|_| status == SolveStatus::SolvedInvalid),
                ..sent
            })
        });
        let ended = session.end(Some([3.0, 3.0])).unwrap();
        assert_eq!(
            (ended.kept, &ended.solved),
            (Kept::Begin, &None),
            "{status:?}"
        );
        // Why: geometry that can't be built, or no solve at all.
        let why = ended.why.unwrap();
        let (code, message) = if status == SolveStatus::SolvedInvalid {
            (
                ErrorCode::InvalidResult,
                "at the drag's final cursor the sketch solves to geometry that can't be built: a line of no length, a circle or arc of no radius, or a point held on a circle drawn at its centre",
            )
        } else {
            (
                ErrorCode::SolveFailed,
                "the sketch didn't solve as a whole at the drag's final cursor",
            )
        };
        assert_eq!((why.code(), why.message()), (code, message), "{status:?}");
    }
}

// --- Replies the core can't trust ----------------------------------------------------------

#[test]
fn a_move_answered_with_fewer_values_than_the_sketch_has_is_not_a_solved_frame() {
    // Broke, before the fix: a `Moved` reply isn't checked against what was sent (the recompute checks every
    // `Solved` with `Translation::check_reply`: as many values, fixed ones as sent, all within
    // reach). A frame of one value for a four-value sketch is handed back as `Solved`, and
    // the end at its cursor takes it as the dragged sketch, so `Kept::Cursor` comes back with
    // one value.
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.answer_moves(|_, _| {
        Ok(Moved {
            status: SolveStatus::Success,
            values: Some(vec![1.0]),
        })
    });
    let outcome = session.move_to([1.0, 1.0]).unwrap().wait();
    let ended = session.end(Some([1.0, 1.0]));
    // The frame is the worker's protocol error, which ends the drag, so the end that
    // follows is refused as no drag open, saying why, with the worker's code: a fault.
    match outcome {
        MoveOutcome::Failed(e) => {
            assert_eq!(e.code(), ErrorCode::ProtocolError, "{e}");
            assert_eq!(
                e.message(),
                "the sketch worker's drag frame has 1 values for 4 sent"
            );
        }
        other => panic!("a one-value frame for a four-value sketch: {other:?}"),
    }
    match ended {
        Err(e) => {
            assert_eq!(e.code(), ErrorCode::ProtocolError, "{e}");
            assert_eq!(e.message(), "no drag is open: its worker failed");
            assert_eq!(
                e.cause(),
                Some("the sketch worker's drag frame has 1 values for 4 sent")
            );
        }
        Ok(ended) => panic!("the end after the drag closed: {ended:?}"),
    }
}

#[test]
fn a_move_that_moves_a_fixed_value_or_answers_nan_is_not_a_solved_frame() {
    // Broke, before the fix: as above: a frame that moves the fixed point (value 2), or gives the dragged
    // point a NaN, is `Solved`, and the end at its cursor keeps it as the cursor's sketch.
    for (what, bad) in [("moved fixed", 0), ("nan", 1), ("out of reach", 2)] {
        let (solvers, started) = pool();
        let session = open(&solvers);
        let fake = started.lock().unwrap()[0].clone();
        fake.answer_moves(move |b, to| {
            let mut moved = FakeSketches::dragged(b, to);
            let values = moved.values.as_mut().unwrap();
            match bad {
                0 => values[2] = 50.0,
                1 => values[0] = f64::NAN,
                _ => values[1] = 5e6,
            }
            Ok(moved)
        });
        let outcome = session.move_to([1.0, 1.0]).unwrap().wait();
        let ended = session.end(Some([1.0, 1.0]));
        let kept_cursor = matches!(
            ended,
            Ok(Ended {
                kept: Kept::Cursor,
                ..
            })
        );
        assert!(
            !matches!(outcome, MoveOutcome::Solved { .. }) && !kept_cursor,
            "{what}: frame {outcome:?}, end {ended:?}"
        );
    }
}

#[test]
fn a_closing_solve_that_moves_a_fixed_value_or_answers_too_few_is_refused() {
    // Broke, before the fix: the end's whole-sketch solve isn't checked either: a solution that moves the
    // fixed point, has values out of reach, or has fewer values than were sent comes back
    // as `Kept::Cursor`, for M5.3 to write into the document.
    for (what, bad) in [("moved fixed", 0), ("too few", 1), ("out of reach", 2)] {
        let (solvers, started) = pool();
        let session = open(&solvers);
        let fake = started.lock().unwrap()[0].clone();
        fake.answer(move |s| {
            let mut solved = FakeSketches::as_sent(s);
            match bad {
                0 => solved.values[3] = -40.0,
                1 => solved.values.truncate(2),
                _ => solved.values[0] = -3e6,
            }
            Ok(solved)
        });
        let ended = session.end(Some([2.0, 2.0]));
        match ended {
            Err(e) => assert_eq!(e.code(), ErrorCode::ProtocolError, "{what}: {e}"),
            Ok(ended) => panic!("{what}: {ended:?}"),
        }
    }
}
