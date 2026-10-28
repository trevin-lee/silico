//! `silico_core::drag` on the fake sketch solver (ARCHITECTURE.md, *Sketches solve in the
//! sketch worker*; docs/design/m5.0-app.md, the drag commands, decision 9): a drag's moves
//! reach the one worker that began it while other sessions' rebuilds run, a replaced move
//! is never solved, and the end is a function of the begin state and the final cursor alone,
//! however many moves were dropped. Expected values are by hand: the fake answers a move
//! from the begin state alone, the dragged point at the cursor nudged by a thousandth of
//! where it began, and solves a sketch where it's sent.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use silico_core::drag::{
    Begun, DRAG_MOVE_TIMEOUT, DragSession, Ended, Kept, MoveOutcome, Notify, begin, begin_with,
};
use silico_core::fake::{DragCall, FakeKernel, FakeSketches};
use silico_core::recompute::{Recompute, SketchSolvers};
use silico_core::sketch::SketchSolver;
use silico_core::{ErrorCode, command};
use silico_protocol::{
    Algorithm, DragBegin, Geometry, Moved, PointOf, PointValues, SketchGeometry, SketchSystem,
    SolveStatus, SolverConstraint, Value,
};

use crate::fixture::{document, fields, point};

/// How long a test waits for something it expects.
const WAIT: Duration = Duration::from_secs(10);

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

/// A lone point at (4, 8), free, and a fixed point at (1, 1): the drag's sketch.
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
        Begun::Open { session, dofs } => {
            assert_eq!(dofs, 1);
            session
        }
        Begun::Conflicting(tags) => panic!("{tags:?}"),
    }
}

/// The fake's values for the begin state dragged to `to`: (4, 8) moved to `to` plus a
/// thousandth of (4, 8); the fixed point where it began.
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

#[test]
fn a_drags_moves_all_reach_the_worker_that_began_it_while_other_sessions_rebuild() {
    // The drag takes the pool's first fake and holds it; engines on the same pool, rebuilding
    // a sketch over and over meanwhile (other sessions'), start fakes of their own. Every move,
    // and the end's closing solve, went to the drag's fake, and nothing of the rebuilds did;
    // no rebuild's supersession stopped it.
    let (solvers, started) = pool();
    let session = open(&solvers);
    let text = document(&fields(&point("p0000001", 1.0, 2.0), "", "", "", ""));
    let rebuilds: Vec<_> = (0..3)
        .map(|_| {
            let solvers = solvers.clone();
            let text = text.clone();
            std::thread::spawn(move || {
                let kernel = Arc::new(FakeKernel::new(1).unwrap());
                let engine = Recompute::new(kernel).with_solvers(solvers);
                for _ in 0..20 {
                    command::rebuild(&engine, command::gate(text.as_bytes()).unwrap())
                        .unwrap()
                        .unwrap();
                }
            })
        })
        .collect();
    let path: Vec<[f64; 2]> = (0..50)
        .map(|i| [f64::from(i), 0.5 * f64::from(i)])
        .collect();
    for to in &path {
        match session.move_to(*to).unwrap().wait() {
            MoveOutcome::Solved { to: got, moved } => {
                assert_eq!(got, *to);
                assert_eq!(moved.values.unwrap(), dragged_to(*to));
            }
            other => panic!("{to:?}: {other:?}"),
        }
    }
    for rebuild in rebuilds {
        rebuild.join().unwrap();
    }
    let ended = session.end(Some([49.0, 24.5])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor);
    let fakes = started.lock().unwrap().clone();
    let drag = &fakes[0];
    let calls = drag.drag_calls();
    assert!(matches!(calls.first(), Some(DragCall::Begin(_))));
    assert_eq!(calls.last(), Some(&DragCall::End));
    assert_eq!(moves_of(&calls), path);
    // Its one solve is the end's, of the sketch at the final cursor.
    let solves = drag.requests();
    assert_eq!(solves.len(), 1);
    assert_eq!(bits(&solves[0].values), bits(&dragged_to([49.0, 24.5])));
    assert_eq!(drag.stops(), 0);
    // The rebuilds ran on other fakes, which saw no drag.
    assert!(fakes.len() > 1);
    for other in &fakes[1..] {
        assert!(other.drag_calls().is_empty());
        assert!(!other.requests().is_empty());
    }
}

#[test]
fn a_move_replaced_while_one_solves_is_dropped_and_never_solved() {
    // The first move held solving; a second and a third sent meanwhile: the third replaces
    // the second, which answers dropped. Released, the third is solved: the worker saw the
    // first and the third alone.
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.hold();
    let first = session.move_to([1.0, 1.0]).unwrap();
    assert!(fake.wait_held(WAIT));
    let second = session.move_to([2.0, 2.0]).unwrap();
    let third = session.move_to([3.0, 3.0]).unwrap();
    assert_eq!(second.wait(), MoveOutcome::Dropped);
    fake.release();
    assert!(matches!(
        first.wait(),
        MoveOutcome::Solved { to: [1.0, 1.0], .. }
    ));
    assert!(matches!(
        third.wait(),
        MoveOutcome::Solved { to: [3.0, 3.0], .. }
    ));
    assert_eq!(moves_of(&fake.drag_calls()), [[1.0, 1.0], [3.0, 3.0]]);
    session.end(None).unwrap();
}

/// The end of a drag along `path` to `end`, the worker held while `held` of the moves are
/// sent and released after each of `releases`'s moves: what it kept, and the closing solve
/// sent.
fn ended_after(path: &[[f64; 2]], held: &[usize], end: [f64; 2]) -> (Ended, Vec<Vec<u64>>) {
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    let mut tickets = Vec::new();
    for (i, to) in path.iter().enumerate() {
        if held.contains(&i) {
            fake.hold();
        }
        tickets.push(session.move_to(*to).unwrap());
        if !held.contains(&i) {
            fake.release();
        }
    }
    fake.release();
    let ended = session.end(Some(end)).unwrap();
    for ticket in &tickets {
        assert!(!matches!(
            ticket.wait(),
            MoveOutcome::Failed(_) | MoveOutcome::Closed(_)
        ));
    }
    let solves = fake.requests().iter().map(|s| bits(&s.values)).collect();
    (ended, solves)
}

#[test]
fn the_same_begin_path_and_end_give_the_same_sketch_however_many_moves_were_dropped() {
    // A path of twenty cursors and an end, the worker held for none of the moves, for every
    // other one, for runs of them, and with no moves at all: the same closing solve, of the
    // begin state at the final cursor, and the same sketch kept, bit for bit.
    let path: Vec<[f64; 2]> = (0..20)
        .map(|i| [f64::from(i) * 0.25, 7.0 - f64::from(i)])
        .collect();
    let end = [12.5, -3.75];
    let want = dragged_to(end);
    let every_other: Vec<usize> = (0..20).step_by(2).collect();
    let runs: Vec<usize> = (3..9).chain(12..19).collect();
    for (held, path) in [
        (&[][..], &path[..]),
        (&every_other[..], &path[..]),
        (&runs[..], &path[..]),
        (&[][..], &[][..]),
    ] {
        let (ended, solves) = ended_after(path, held, end);
        assert_eq!(ended.kept, Kept::Cursor, "{held:?}");
        assert_eq!(bits(&ended.solved.unwrap().values), bits(&want), "{held:?}");
        assert_eq!(solves, [bits(&want)], "{held:?}");
    }
}

#[test]
fn an_end_at_the_newest_solved_moves_cursor_moves_no_more_and_another_moves_once() {
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    session.move_to([5.0, 5.0]).unwrap().wait();
    session.end(Some([5.0, 5.0])).unwrap();
    assert_eq!(moves_of(&fake.drag_calls()), [[5.0, 5.0]]);
    let session = open(&solvers);
    session.move_to([5.0, 5.0]).unwrap().wait();
    // One bit away: another cursor, moved to once more.
    let next = [f64::from_bits(5.0f64.to_bits() + 1), 5.0];
    session.end(Some(next)).unwrap();
    assert_eq!(moves_of(&fake.drag_calls()), [[5.0, 5.0], [5.0, 5.0], next]);
}

#[test]
fn an_end_with_no_cursor_stops_the_move_solving_keeps_the_begin_and_frees_the_worker() {
    // Esc: the held move is stopped (as restarting its worker stops it), answers cancelled,
    // and nothing is kept; the next drag takes the same fake again.
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.hold();
    let moving = session.move_to([1.0, 1.0]).unwrap();
    assert!(fake.wait_held(WAIT));
    let waiting = session.move_to([2.0, 2.0]).unwrap();
    let ended = session.end(None).unwrap();
    assert_eq!(
        ended,
        Ended {
            kept: Kept::Begin,
            solved: None
        }
    );
    assert_eq!(moving.wait(), MoveOutcome::Cancelled);
    assert_eq!(waiting.wait(), MoveOutcome::Dropped);
    assert_eq!(fake.stops(), 1);
    fake.release();
    let again = open(&solvers);
    assert!(matches!(
        again.move_to([3.0, 3.0]).unwrap().wait(),
        MoveOutcome::Solved { .. }
    ));
    again.end(None).unwrap();
    assert_eq!(started.lock().unwrap().len(), 1);
}

#[test]
fn a_final_move_that_doesnt_reach_the_cursor_keeps_the_begin_state() {
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.answer_moves(|_, _| {
        Ok(Moved {
            status: SolveStatus::Failed,
            values: None,
        })
    });
    let ended = session.end(Some([9.0, 9.0])).unwrap();
    assert_eq!(
        ended,
        Ended {
            kept: Kept::Begin,
            solved: None
        }
    );
    // No closing solve, and the worker's drag ended.
    assert!(fake.requests().is_empty());
    assert_eq!(fake.drag_calls().last(), Some(&DragCall::End));
}

#[test]
fn a_move_the_worker_fails_ends_the_drag_and_later_moves_and_its_end_are_refused() {
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.answer_moves(|_, _| {
        Err(silico_core::Error::new(
            ErrorCode::WorkerTimeout,
            "the fake move took too long",
        ))
    });
    match session.move_to([1.0, 1.0]).unwrap().wait() {
        MoveOutcome::Failed(e) => assert_eq!(e.code(), ErrorCode::WorkerTimeout),
        other => panic!("{other:?}"),
    }
    match session.move_to([2.0, 2.0]).unwrap().wait() {
        MoveOutcome::Closed(e) => {
            assert_eq!(e.message(), "no drag is open: its worker failed");
            assert_eq!(e.cause(), Some("the fake move took too long"));
        }
        other => panic!("{other:?}"),
    }
    let e = session.end(Some([2.0, 2.0])).unwrap_err();
    assert_eq!(e.message(), "no drag is open: its worker failed");
}

#[test]
fn a_cursor_out_of_reach_is_refused_naming_to_and_the_drag_goes_on() {
    let (solvers, _) = pool();
    let session = open(&solvers);
    for bad in [[f64::NAN, 0.0], [0.0, f64::INFINITY], [2e6, 0.0]] {
        let e = session.move_to(bad).err().unwrap();
        assert_eq!(e.parameter(), Some("to"), "{bad:?}");
    }
    assert!(matches!(
        session.move_to([1.0, 2.0]).unwrap().wait(),
        MoveOutcome::Solved { .. }
    ));
    let e = session.end(Some([f64::NAN, 0.0])).unwrap_err();
    assert_eq!(e.parameter(), Some("to"));
}

#[test]
fn a_sketch_whose_constraints_conflict_starts_no_drag_and_gives_its_solver_back() {
    // The fake started for a drag that conflicts is given back: the next drag, the
    // conflict gone, takes it again, and no other is started.
    let (solvers, started) = pool();
    let first = solvers_first(&solvers, &started);
    first.conflicting_drags(vec![silico_protocol::Tag(3), silico_protocol::Tag(7)]);
    match begin(&solvers, tagged(&[3, 7])).unwrap() {
        Begun::Conflicting(tags) => {
            assert_eq!(tags, [silico_protocol::Tag(3), silico_protocol::Tag(7)]);
        }
        Begun::Open { .. } => panic!("began"),
    }
    first.conflicting_drags(vec![]);
    let session = open(&solvers);
    session.end(None).unwrap();
    assert_eq!(started.lock().unwrap().len(), 1);
    assert_eq!(
        first
            .drag_calls()
            .iter()
            .filter(|c| matches!(c, DragCall::Begin(_)))
            .count(),
        // The helper's, the one that conflicted, and this one: all on the first fake.
        3
    );
}

/// [`two_points`] with the two points coincident once for each of `tags`.
fn tagged(tags: &[u32]) -> DragBegin {
    let mut begin = two_points();
    begin.system.constraints = tags
        .iter()
        .map(|&t| SolverConstraint::Coincident {
            tag: silico_protocol::Tag(t),
            a: PointOf::Point(Geometry(0)),
            b: PointOf::Point(Geometry(1)),
        })
        .collect();
    begin
}

/// The pool's first fake, started by a drag begun and ended.
fn solvers_first(solvers: &SketchSolvers, started: &Arc<Mutex<Vec<FakeSketches>>>) -> FakeSketches {
    open(solvers).end(None).unwrap();
    started.lock().unwrap()[0].clone()
}

#[test]
fn a_dropped_drag_ends_with_nothing_kept_and_gives_its_worker_back() {
    let (solvers, started) = pool();
    {
        let session = open(&solvers);
        session.move_to([1.0, 1.0]).unwrap().wait();
    }
    let fake = started.lock().unwrap()[0].clone();
    assert_eq!(fake.drag_calls().last(), Some(&DragCall::End));
    assert!(fake.requests().is_empty());
    let again = open(&solvers);
    again.end(None).unwrap();
    assert_eq!(started.lock().unwrap().len(), 1);
}

// ---- the tests review's

/// A fake that counts the times it's distrusted (a worker that sent a reply it can't be
/// trusted for is stopped, not given back to be used again).
struct Counting {
    fake: FakeSketches,
    distrusts: Arc<std::sync::atomic::AtomicUsize>,
}

impl SketchSolver for Counting {
    fn solve(
        &mut self,
        system: SketchSystem,
    ) -> Result<silico_protocol::Solved, silico_core::Error> {
        self.fake.solve(system)
    }
    fn distrust(&mut self) {
        self.distrusts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    fn stopper(&mut self) -> Result<Option<silico_core::sketch::SolveStopper>, silico_core::Error> {
        self.fake.stopper()
    }
    fn drag_begin(
        &mut self,
        begin: DragBegin,
    ) -> Result<silico_core::sketch::DragStarted, silico_core::Error> {
        self.fake.drag_begin(begin)
    }
    fn drag_move(&mut self, to: [f64; 2], timeout: Duration) -> Result<Moved, silico_core::Error> {
        self.fake.drag_move(to, timeout)
    }
    fn drag_end(&mut self) -> Result<(), silico_core::Error> {
        self.fake.drag_end()
    }
}

/// Solvers of one counting fake: the fake, and how many times it's been distrusted.
fn counted() -> (
    SketchSolvers,
    FakeSketches,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let fake = FakeSketches::new();
    let distrusts: Arc<std::sync::atomic::AtomicUsize> = Arc::default();
    let solvers = SketchSolvers::starting({
        let fake = fake.clone();
        let distrusts = Arc::clone(&distrusts);
        move || {
            Ok(Box::new(Counting {
                fake: fake.clone(),
                distrusts: Arc::clone(&distrusts),
            }) as Box<dyn SketchSolver>)
        }
    });
    (solvers, fake, distrusts)
}

#[test]
fn an_end_at_a_cursor_differing_from_the_newest_moves_in_y_alone_moves_once_more() {
    // Moved to (5, 5), ended at (5, 6): the newest move's cursor isn't the end's, so the end
    // moves again, and the sketch is the begin state at (5, 6), not at (5, 5).
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    session.move_to([5.0, 5.0]).unwrap().wait();
    let ended = session.end(Some([5.0, 6.0])).unwrap();
    assert_eq!(moves_of(&fake.drag_calls()), [[5.0, 5.0], [5.0, 6.0]]);
    assert_eq!(
        bits(&ended.solved.unwrap().values),
        bits(&dragged_to([5.0, 6.0]))
    );
}

#[test]
fn a_closing_solve_that_drops_redundant_equations_keeps_the_cursor() {
    // A sketch with a redundant constraint solves `SolvedExceptRedundant`: the drag is kept.
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    fake.answer(|system| {
        Ok(silico_protocol::Solved {
            status: SolveStatus::SolvedExceptRedundant,
            ..FakeSketches::as_sent(system)
        })
    });
    let ended = session.end(Some([2.0, 3.0])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor);
    assert_eq!(
        bits(&ended.solved.unwrap().values),
        bits(&dragged_to([2.0, 3.0]))
    );
}

#[test]
fn a_worker_that_sends_an_impossible_frame_or_solve_is_distrusted_not_reused() {
    // A frame moving a fixed value, the end's own move's frame of too few values, and a
    // closing solve moving a fixed value: each the worker's protocol error, and the worker
    // distrusted (stopped), once each.
    let moved_fixed = |_: &DragBegin, _: [f64; 2]| {
        Ok(Moved {
            status: SolveStatus::Success,
            values: Some(vec![1.0, 1.0, 50.0, 1.0]),
        })
    };
    let (solvers, fake, distrusts) = counted();
    fake.answer_moves(moved_fixed);
    let session = match begin(&solvers, two_points()).unwrap() {
        Begun::Open { session, .. } => session,
        Begun::Conflicting(tags) => panic!("{tags:?}"),
    };
    match session.move_to([1.0, 1.0]).unwrap().wait() {
        MoveOutcome::Failed(e) => assert_eq!(e.code(), ErrorCode::ProtocolError),
        other => panic!("{other:?}"),
    }
    assert_eq!(distrusts.load(std::sync::atomic::Ordering::SeqCst), 1);
    drop(session);
    // The end's own move.
    fake.answer_moves(|_, _| {
        Ok(Moved {
            status: SolveStatus::Success,
            values: Some(vec![1.0]),
        })
    });
    let session = match begin(&solvers, two_points()).unwrap() {
        Begun::Open { session, .. } => session,
        Begun::Conflicting(tags) => panic!("{tags:?}"),
    };
    let e = session.end(Some([1.0, 1.0])).unwrap_err();
    assert_eq!(e.code(), ErrorCode::ProtocolError);
    assert_eq!(distrusts.load(std::sync::atomic::Ordering::SeqCst), 2);
    // The closing solve.
    fake.answer_moves(|begin, to| Ok(FakeSketches::dragged(begin, to)));
    fake.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.values[2] = 50.0;
        Ok(solved)
    });
    let session = match begin(&solvers, two_points()).unwrap() {
        Begun::Open { session, .. } => session,
        Begun::Conflicting(tags) => panic!("{tags:?}"),
    };
    let e = session.end(Some([1.0, 1.0])).unwrap_err();
    assert_eq!(e.code(), ErrorCode::ProtocolError);
    assert_eq!(
        e.message(),
        "the sketch worker's solution moves value 2, which is fixed"
    );
    assert_eq!(distrusts.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[test]
fn a_begin_that_fails_its_check_is_refused_naming_it_and_sends_nothing() {
    // The drag's point names a geometry the sketch hasn't: refused, as the worker would,
    // before any solver is taken.
    let (solvers, started) = pool();
    let mut bad = two_points();
    bad.point = PointOf::Point(Geometry(5));
    let e = match begin(&solvers, bad) {
        Err(e) => e,
        Ok(_) => panic!("began"),
    };
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.parameter(), Some("point"));
    assert!(started.lock().unwrap().is_empty());
}

#[test]
fn a_frame_whose_values_dont_match_its_status_is_the_workers_fault() {
    // A success with no values, and a failure with values: neither is a frame a worker
    // gives, so each is a protocol error, the worker distrusted once each.
    let (solvers, fake, distrusts) = counted();
    for (status, values, message) in [
        (
            SolveStatus::Success,
            None,
            "the sketch worker's drag frame is a success with no values",
        ),
        (
            SolveStatus::Failed,
            Some(vec![4.0, 8.0, 1.0, 1.0]),
            "the sketch worker's drag frame has values with the status Failed",
        ),
    ] {
        fake.answer_moves(move |_, _| {
            Ok(Moved {
                status,
                values: values.clone(),
            })
        });
        let session = match begin(&solvers, two_points()).unwrap() {
            Begun::Open { session, .. } => session,
            Begun::Conflicting(tags) => panic!("{tags:?}"),
        };
        match session.move_to([1.0, 1.0]).unwrap().wait() {
            MoveOutcome::Failed(e) => {
                assert_eq!(e.code(), ErrorCode::ProtocolError);
                assert_eq!(e.message(), message);
            }
            other => panic!("{status:?}: {other:?}"),
        }
    }
    assert_eq!(distrusts.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn a_begin_listing_conflicts_it_wasnt_sent_or_out_of_order_is_the_workers_fault() {
    // Tags 3 and 7 are sent. Tag 5 wasn't; 7 before 3 is out of order; 3 twice is a repeat.
    let (solvers, fake, distrusts) = counted();
    for (n, listed) in [vec![5], vec![7, 3], vec![3, 3]].into_iter().enumerate() {
        fake.conflicting_drags(listed.iter().map(|&t| silico_protocol::Tag(t)).collect());
        match begin(&solvers, tagged(&[3, 7])) {
            Err(e) => {
                assert_eq!(e.code(), ErrorCode::ProtocolError, "{listed:?}");
                assert_eq!(
                    e.message(),
                    "the sketch worker's drag begin lists conflicting tags it wasn't sent, or out of order"
                );
            }
            Ok(_) => panic!("{listed:?}: began"),
        }
        assert_eq!(distrusts.load(std::sync::atomic::Ordering::SeqCst), n + 1);
    }
    // Both, in order: the conflict the begin reports.
    fake.conflicting_drags(vec![silico_protocol::Tag(3), silico_protocol::Tag(7)]);
    assert!(matches!(
        begin(&solvers, tagged(&[3, 7])).unwrap(),
        Begun::Conflicting(_)
    ));
    assert_eq!(distrusts.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[test]
fn a_begin_state_prepared_on_the_drags_solver_takes_no_second_worker() {
    // `prepare` solves on the solver the drag then holds: the fake it ran on is the one the
    // drag begins on, and no other is started.
    let (solvers, started) = pool();
    let first = solvers_first(&solvers, &started);
    let session = match begin_with(&solvers, None, |solver| {
        solver.solve(two_points().system)?;
        Ok(two_points())
    })
    .unwrap()
    {
        Begun::Open { session, .. } => session,
        Begun::Conflicting(tags) => panic!("{tags:?}"),
    };
    assert_eq!(first.requests().len(), 1);
    assert!(matches!(
        session.move_to([1.0, 1.0]).unwrap().wait(),
        MoveOutcome::Solved { .. }
    ));
    session.end(None).unwrap();
    assert_eq!(started.lock().unwrap().len(), 1);
    // A `prepare` that fails is refused with its error, the solver given back.
    let e = match begin_with(&solvers, None, |_| {
        Err(silico_core::Error::new(
            ErrorCode::Conflict,
            "no such sketch",
        ))
    }) {
        Err(e) => e,
        Ok(_) => panic!("began"),
    };
    assert_eq!(e.code(), ErrorCode::Conflict);
    // A begin state it makes that fails its check: refused naming it, nothing begun.
    let begins = |fake: &FakeSketches| {
        fake.drag_calls()
            .iter()
            .filter(|c| matches!(c, DragCall::Begin(_)))
            .count()
    };
    let before = begins(&first);
    let e = match begin_with(&solvers, None, |_| {
        let mut bad = two_points();
        bad.point = PointOf::Point(Geometry(5));
        Ok(bad)
    }) {
        Err(e) => e,
        Ok(_) => panic!("began"),
    };
    assert_eq!(e.parameter(), Some("point"));
    assert_eq!(begins(&first), before);
    open(&solvers).end(None).unwrap();
    assert_eq!(started.lock().unwrap().len(), 1);
}

#[test]
fn the_notifier_is_told_of_each_outcome_and_of_the_end() {
    // Told with no lock held: the notifier reads a ticket's outcome as it's told, which
    // takes the drag's lock, and would deadlock under it.
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (solvers, started) = pool();
    let told = Arc::new(AtomicUsize::new(0));
    let slot: Arc<Mutex<Option<silico_core::drag::MoveTicket>>> = Arc::new(Mutex::new(None));
    let (counter, read) = (Arc::clone(&told), Arc::clone(&slot));
    let notify: Notify = Arc::new(move || {
        if let Some(ticket) = read.lock().unwrap().as_ref() {
            let _ = ticket.try_take();
        }
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let session = match begin_with(&solvers, Some(notify), |_| Ok(two_points())).unwrap() {
        Begun::Open { session, .. } => session,
        Begun::Conflicting(tags) => panic!("{tags:?}"),
    };
    let fake = started.lock().unwrap()[0].clone();
    assert_eq!(told.load(Ordering::SeqCst), 0);
    let ticket = session.move_to([1.0, 1.0]).unwrap();
    *slot.lock().unwrap() = Some(ticket);
    // The move sent, and its outcome: each told.
    let (waited, seen) = (Arc::clone(&told), Arc::clone(&slot));
    assert!(within(move || {
        while waited.load(Ordering::SeqCst) < 2
            || seen.lock().unwrap().as_ref().unwrap().try_take().is_none()
        {
            std::thread::yield_now();
        }
    }));
    assert!(matches!(
        slot.lock().unwrap().as_ref().unwrap().try_take(),
        Some(MoveOutcome::Solved { .. })
    ));
    assert_eq!(told.load(Ordering::SeqCst), 2);
    session.end(None).unwrap();
    // The end asked for, the end's result, and the thread's return: three more.
    assert_eq!(told.load(Ordering::SeqCst), 5);
    assert_eq!(fake.drag_calls().last(), Some(&DragCall::End));
}

#[test]
fn a_cancelled_move_answers_cancelled_and_closes_the_drag_keeping_the_begin() {
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    // Nothing solving: nothing to cancel, and the drag stays open.
    assert!(!session.cancel_move());
    assert!(session.is_open());
    assert!(session.closed().is_none());
    fake.hold();
    let moving = session.move_to([1.0, 1.0]).unwrap();
    assert!(fake.wait_held(WAIT));
    assert!(session.cancel_move());
    assert_eq!(moving.wait(), MoveOutcome::Cancelled);
    assert_eq!(fake.stops(), 1);
    fake.release();
    assert!(!session.is_open());
    assert_eq!(
        session.closed().map(|e| e.message().to_owned()),
        Some("no drag is open: its move was stopped".to_owned())
    );
    match session.move_to([2.0, 2.0]).unwrap().wait() {
        MoveOutcome::Closed(e) => assert_eq!(e.message(), "no drag is open: its move was stopped"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        session.end(None).unwrap(),
        Ended {
            kept: Kept::Begin,
            solved: None
        }
    );
    // A cursor after a cancel is refused: the drag ended before it.
    let session = open(&solvers);
    fake.hold();
    let moving = session.move_to([1.0, 1.0]).unwrap();
    assert!(fake.wait_held(WAIT));
    assert!(session.cancel_move());
    assert_eq!(moving.wait(), MoveOutcome::Cancelled);
    fake.release();
    let e = session.end(Some([1.0, 1.0])).unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.message(), "no drag is open: its move was stopped");
    assert_eq!(started.lock().unwrap().len(), 1);
}

#[test]
fn an_open_drag_says_so_until_its_worker_fails() {
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    assert!(session.is_open());
    fake.answer_moves(|_, _| {
        Err(silico_core::Error::new(
            ErrorCode::WorkerCrashed,
            "the fake crashed",
        ))
    });
    assert!(matches!(
        session.move_to([1.0, 1.0]).unwrap().wait(),
        MoveOutcome::Failed(_)
    ));
    assert!(!session.is_open());
    let closed = session.closed().unwrap();
    assert_eq!(closed.code(), ErrorCode::WorkerCrashed);
    assert_eq!(closed.message(), "no drag is open: its worker failed");
    assert_eq!(closed.cause(), Some("the fake crashed"));
}

#[test]
fn each_move_and_the_ends_own_move_is_given_ten_seconds() {
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    session.move_to([1.0, 1.0]).unwrap().wait();
    session.end(Some([2.0, 2.0])).unwrap();
    assert_eq!(DRAG_MOVE_TIMEOUT, Duration::from_secs(10));
    assert_eq!(fake.move_timeouts(), [Duration::from_secs(10); 2]);
}

/// Whether `f` returns within [`WAIT`].
fn within(f: impl FnOnce() + Send + 'static) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(WAIT).is_ok()
}

#[test]
fn a_cancel_landing_as_the_move_answers_still_cancels_it_and_stops_only_the_drags_worker() {
    // The fake's move has its answer (a success) but the drag hasn't taken it when the
    // cancel lands: the move answers cancelled all the same, the drag ends, and the one stop
    // is the drag's worker's, made while the drag still held it.
    let (solvers, started) = pool();
    let session = open(&solvers);
    let fake = started.lock().unwrap()[0].clone();
    let (answering_tx, answering) = std::sync::mpsc::channel::<()>();
    let (go, go_rx) = std::sync::mpsc::channel::<()>();
    let go_rx = Mutex::new(go_rx);
    let answering_tx = Mutex::new(answering_tx);
    fake.answer_moves(move |begin, to| {
        answering_tx.lock().unwrap().send(()).unwrap();
        go_rx.lock().unwrap().recv().unwrap();
        Ok(FakeSketches::dragged(begin, to))
    });
    let moving = session.move_to([1.0, 1.0]).unwrap();
    answering.recv_timeout(WAIT).unwrap();
    assert!(session.cancel_move());
    // The stop is made by the time the cancel returns, before the drag has the answer.
    assert_eq!(fake.stop_waits().len(), 1);
    go.send(()).unwrap();
    assert_eq!(moving.wait(), MoveOutcome::Cancelled);
    assert!(!session.is_open());
    assert_eq!(
        session.closed().map(|e| e.message().to_owned()),
        Some("no drag is open: its move was stopped".to_owned())
    );
    let e = session.end(Some([1.0, 1.0])).unwrap_err();
    assert_eq!(e.message(), "no drag is open: its move was stopped");
    // No closing solve, and no more stops.
    assert!(fake.requests().is_empty());
    assert_eq!(fake.stop_waits().len(), 1);
}

#[test]
fn a_cursor_end_asked_for_while_a_cancelled_move_answers_is_refused_as_after_it() {
    // The end with a cursor is recorded (the notifier says so) before the cancelled move
    // comes back: refused, as it is once the move has answered, not kept as the begin.
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (solvers, started) = pool();
    let told = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&told);
    let notify: Notify = Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let session = match begin_with(&solvers, Some(notify), |_| Ok(two_points())).unwrap() {
        Begun::Open { session, .. } => session,
        Begun::Conflicting(tags) => panic!("{tags:?}"),
    };
    let fake = started.lock().unwrap()[0].clone();
    let (answering_tx, answering) = std::sync::mpsc::channel::<()>();
    let (go, go_rx) = std::sync::mpsc::channel::<()>();
    let (go_rx, answering_tx) = (Mutex::new(go_rx), Mutex::new(answering_tx));
    fake.answer_moves(move |begin, to| {
        answering_tx.lock().unwrap().send(()).unwrap();
        go_rx.lock().unwrap().recv().unwrap();
        Ok(FakeSketches::dragged(begin, to))
    });
    let moving = session.move_to([1.0, 1.0]).unwrap();
    answering.recv_timeout(WAIT).unwrap();
    assert!(session.cancel_move());
    let before = told.load(Ordering::SeqCst);
    let ending = std::thread::spawn(move || session.end(Some([1.0, 1.0])));
    // The end recorded: `finish` tells the notifier once it has.
    let seen = Arc::clone(&told);
    assert!(within(move || {
        while seen.load(Ordering::SeqCst) == before {
            std::thread::yield_now();
        }
    }));
    go.send(()).unwrap();
    let e = ending.join().unwrap().unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.message(), "no drag is open: its move was stopped");
    assert_eq!(moving.wait(), MoveOutcome::Cancelled);
    assert!(fake.requests().is_empty());
}
