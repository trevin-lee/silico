//! break-it: `silico_core::drag`, the core's drag client, on the real sketch worker: a
//! frame the worker refuses while its drag goes on, a worker killed between moves, the end's
//! determinism through PlaneGCS, and Esc at once after a move, over and over. Where a point
//! lands is worked out by hand (a projection onto a circle, or a fixed offset).

use super::systems;
use super::wait::WAIT;

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use silico_core::ErrorCode;
use silico_core::drag::{Begun, DragSession, Kept, MoveOutcome, begin};
use silico_core::recompute::SketchSolvers;
use silico_core::sketch::{SketchSolver, SketchWorker};
use silico_protocol::{
    Algorithm, DragBegin, PointOf, PointValues, SketchGeometry, SolverConstraint,
};
use systems::Builder;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// Solvers that start the sketch worker, and each started worker's process, in order.
fn workers() -> (SketchSolvers, Arc<Mutex<Vec<u32>>>) {
    let pids: Arc<Mutex<Vec<u32>>> = Arc::default();
    let list = Arc::clone(&pids);
    let solvers = SketchSolvers::starting(move || {
        let worker = SketchWorker::start_worker(Path::new(WORKER))?;
        list.lock().unwrap().push(worker.pid());
        Ok(Box::new(worker) as Box<dyn SketchSolver>)
    });
    (solvers, pids)
}

fn open(solvers: &SketchSolvers, begin_: DragBegin) -> DragSession {
    match begin(solvers, begin_).unwrap() {
        Begun::Open { session, .. } => session,
        Begun::Conflicting(tags) => panic!("{tags:?}"),
    }
}

/// Two free points, the second 10 right of the first (M3.7's out-of-reach case): the first
/// dragged to x = 1e6 puts the second at 1e6 + 10, out of reach, which the worker refuses
/// while its drag goes on.
fn ten_apart() -> (DragBegin, PointValues, PointValues) {
    let mut b = Builder::default();
    let p = b.point_values(0.0, 0.0);
    let q = b.point_values(10.0, 0.0);
    let first = b.add(SketchGeometry::Point(p));
    let second = b.add(SketchGeometry::Point(q));
    let ten = b.fixed(10.0);
    b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Point(first),
        b: PointOf::Point(second),
        value: ten,
    });
    (
        DragBegin {
            system: b.system(Algorithm::DogLeg),
            dragged: silico_protocol::Dragged::Point(PointOf::Point(first)),
        },
        p,
        q,
    )
}

/// A 10 mm line from a fixed (3, -2), its end dragged: lands on the circle about the start.
fn fixed_length_line() -> (DragBegin, PointValues) {
    let mut b = Builder::default();
    let start = PointValues {
        x: b.fixed(3.0),
        y: b.fixed(-2.0),
    };
    let end = b.point_values(13.0, -2.0);
    let line = b.add(SketchGeometry::Line { start, end });
    let length = b.fixed(10.0);
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Start(line),
        b: PointOf::End(line),
        value: length,
    });
    (
        DragBegin {
            system: b.system(Algorithm::DogLeg),
            dragged: silico_protocol::Dragged::Point(PointOf::End(line)),
        },
        end,
    )
}

fn at(values: &[f64], p: PointValues) -> [f64; 2] {
    [values[p.x.0 as usize], values[p.y.0 as usize]]
}

fn near(got: [f64; 2], want: [f64; 2]) -> bool {
    libm::hypot(got[0] - want[0], got[1] - want[1]) <= 1e-9
}

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// Kills `pid` and waits until it's gone or a zombie (the worker's own wait reaps it).
fn kill(pid: u32) {
    assert!(
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + WAIT;
    loop {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout);
        if stat.trim().is_empty() || stat.trim_start().starts_with('Z') {
            return;
        }
        assert!(Instant::now() < deadline, "{pid} still running");
        std::thread::yield_now();
    }
}

#[test]
fn a_frame_the_worker_refuses_as_out_of_reach_leaves_the_drag_open() {
    // Broke, before the fix: the worker refuses the frame (`SolutionOutOfReach`, `kernel_failed`) and keeps
    // its drag (M3.7: "a solution out of reach is refused and the drag goes on"; the worker's
    // `State::handle` ends a drag on a refused move only for `SolverThrew` and
    // `OutOfMemory`). The design ends a drag only for a move that hangs or crashes
    // (docs/design/m5.0-app.md, `drag_move`). The core's drag thread treats every `Err`
    // from `drag_move` as the worker failing: the frame answers `Failed`, the drag closes
    // ("no drag is open: its worker failed", though nothing failed), and the next move, a
    // good one, is refused.
    let (begin_, p, q) = ten_apart();
    let (solvers, _) = workers();
    let session = open(&solvers, begin_);
    let refused = session.move_to([1e6, 0.0]).unwrap().wait();
    // Refused, the worker's own refusal kept whole: the drag goes on.
    match &refused {
        MoveOutcome::Refused(e) => {
            assert_eq!(e.code(), ErrorCode::KernelFailed, "{e}");
            assert!(
                e.cause().is_some_and(|c| c.contains("more than 1000000")),
                "{e}"
            );
        }
        other => panic!("{other:?}"),
    }
    match session.move_to([-6.0, 9.0]).unwrap().wait() {
        MoveOutcome::Solved { moved, .. } => {
            let values = moved.values.unwrap();
            assert!(near(at(&values, p), [-6.0, 9.0]), "{values:?}");
            // 10 right of the first; its height, which nothing ties, where it began.
            assert!(near(at(&values, q), [4.0, 0.0]), "{values:?}");
        }
        other => panic!("the drag ended at a frame the worker refused: {other:?}"),
    }
    session.end(None).unwrap();
}

#[test]
fn an_end_at_a_cursor_the_worker_refuses_as_out_of_reach_keeps_the_begin_state() {
    // Broke, before the fix: the design's `drag_end`: "If the final move fails, the begin state is kept, and
    // the result says so" (`kept: begin`). The end's move is refused out of reach, and the
    // core's `ending` passes the refusal up with `?`: the end is an error, `kernel_failed`,
    // and the worker's drag is never ended (no `drag_end` is sent).
    let (begin_, _, _) = ten_apart();
    let (solvers, _) = workers();
    let session = open(&solvers, begin_);
    match session.end(Some([1e6, 0.0])) {
        Ok(ended) => assert_eq!(ended.kept, Kept::Begin),
        Err(e) => panic!("the end at a refused frame was an error, not kept: begin: {e}"),
    }
}

#[test]
fn a_worker_killed_between_moves_fails_the_next_move_as_a_crash_and_the_next_drag_works() {
    // The worker killed while idle mid-drag: the next move should fail as the crash it is
    // (`worker_crashed`), ending the drag, and the drag after it should start a worker again.
    // Broke, before the fix: the drag's thread readies the solver with `stopper` before each move, which
    // starts a new worker silently (`SketchWorker::ready`), and the move then goes to a worker
    // with no drag: the frame fails `invalid_input`, "no drag session", blaming the client's
    // request order for a crash.
    let (begin_, end) = fixed_length_line();
    let (solvers, pids) = workers();
    let session = open(&solvers, begin_.clone());
    assert!(matches!(
        session.move_to([20.0, 5.0]).unwrap().wait(),
        MoveOutcome::Solved { .. }
    ));
    kill(pids.lock().unwrap()[0]);
    let failed = session.move_to([5.0, 30.0]).unwrap().wait();
    let after = session.end(Some([5.0, 30.0]));
    // Whatever the code, the drag after it works, on a worker started again.
    let again = open(&solvers, begin_);
    let ended = again.end(Some([3.0, 20.0])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor);
    assert!(near(at(&ended.solved.unwrap().values, end), [3.0, 8.0]));
    assert!(after.is_err(), "{after:?}");
    match failed {
        MoveOutcome::Failed(e) => {
            assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
            // Found before the move is sent, by the session's own check: not the move's
            // crash on a worker that's gone, nor a new worker with no drag.
            assert_eq!(
                e.message(),
                "the sketch worker ended during the drag, and its session with it"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_move_after_the_workers_drag_ended_is_refused_as_no_drag() {
    // The worker's drag ends at its end, so a move after it is the worker's refusal, as no
    // drag is under way; a solver that only said it ended would have moved it.
    let (begin_, _) = fixed_length_line();
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    worker.stopper().unwrap();
    worker.drag_begin(begin_).unwrap();
    worker.session_stopper().unwrap();
    worker
        .drag_move([5.0, 30.0], silico_core::drag::DRAG_MOVE_TIMEOUT)
        .unwrap();
    worker.session_stopper().unwrap();
    worker.drag_end().unwrap();
    worker.session_stopper().unwrap();
    let e = worker
        .drag_move([6.0, 30.0], silico_core::drag::DRAG_MOVE_TIMEOUT)
        .unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
    assert_eq!(
        e.message(),
        "DragMove needs a drag under way, and there's none"
    );
}

#[test]
fn an_end_after_the_worker_was_killed_keeps_the_solved_frame_or_says_it_crashed() {
    // The final cursor's frame is in hand (solved before the kill), so the end needs no move:
    // only its `drag_end` and the closing solve. The closing solve can run on a worker
    // started again (it carries the whole sketch); failing that, the end should say the
    // worker crashed. BREAKS: `ending` readies the solver (a new worker), sends `drag_end`
    // to it, and the new worker's "no drag session" refusal fails the end as
    // `invalid_input`, the solved frame thrown away.
    let (begin_, end) = fixed_length_line();
    let (solvers, pids) = workers();
    let session = open(&solvers, begin_);
    assert!(matches!(
        session.move_to([5.0, 30.0]).unwrap().wait(),
        MoveOutcome::Solved { .. }
    ));
    kill(pids.lock().unwrap()[0]);
    match session.end(Some([5.0, 30.0])) {
        Ok(ended) => {
            assert_eq!(ended.kept, Kept::Cursor);
            let (dx, dy) = (5.0 - 3.0, 30.0 + 2.0);
            let d = libm::hypot(dx, dy);
            assert!(near(
                at(&ended.solved.unwrap().values, end),
                [3.0 + 10.0 * dx / d, -2.0 + 10.0 * dy / d]
            ));
        }
        Err(e) => assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}"),
    }
}

#[test]
fn the_end_through_planegcs_is_the_same_however_many_moves_came_before() {
    // The same begin and final cursor, after no moves, after a waited path, after 200 moves
    // fired without waiting (most dropped), and after a path that last solved the final
    // cursor itself (the end reusing that frame): the same sketch kept, bit for bit. The
    // final cursors include the 149° one, where the solve from the begin state lands opposite
    // the cursor (FreeCAD's `moveGeometry`: (8.574929257125444, -5.144957554275264) for a line
    // about the origin), so a client that solved from a later frame would differ.
    let (begin_, end) = fixed_length_line();
    let (solvers, _) = workers();
    let path: Vec<[f64; 2]> = (0..200)
        .map(|i| {
            let a = f64::from(i) * 0.05;
            [3.0 + 12.0 * libm::cos(a), -2.0 + 12.0 * libm::sin(a)]
        })
        .collect();
    for last in [[-7.0, 4.0], [5.0, 30.0], [20.0, 5.0]] {
        let mut kept = Vec::new();
        for how in 0..4 {
            let session = open(&solvers, begin_.clone());
            match how {
                0 => {}
                1 => {
                    for to in path.iter().step_by(20) {
                        session.move_to(*to).unwrap().wait();
                    }
                }
                2 => {
                    let tickets: Vec<_> = path
                        .iter()
                        .map(|to| session.move_to(*to).unwrap())
                        .collect();
                    drop(tickets);
                }
                _ => {
                    for to in path.iter().step_by(40).chain([&last]) {
                        session.move_to(*to).unwrap().wait();
                    }
                }
            }
            let ended = session.end(Some(last)).unwrap();
            assert_eq!(ended.kept, Kept::Cursor, "{last:?} {how}");
            kept.push(bits(&ended.solved.unwrap().values));
        }
        for (how, k) in kept.iter().enumerate() {
            assert_eq!(k, &kept[0], "{last:?}: path {how}");
        }
        let values: Vec<f64> = kept[0].iter().map(|b| f64::from_bits(*b)).collect();
        if last == [-7.0, 4.0] {
            let d = 136.0_f64.sqrt();
            assert!(near(at(&values, end), [3.0 + 100.0 / d, -2.0 - 60.0 / d]));
        }
    }
}

#[test]
fn esc_at_once_after_a_move_fifty_times_answers_each_and_leaves_a_worker_that_drags() {
    // A move sent and Esc at once: the move solving is stopped by killing its worker, or it
    // had answered, or it was never taken. Every time the end answers `kept: begin` and the
    // ticket answers; then a drag on the same solvers works, its frame where it belongs.
    let (begin_, end) = fixed_length_line();
    let (solvers, _) = workers();
    for i in 0..50 {
        let session = open(&solvers, begin_.clone());
        let ticket = session.move_to([20.0, 5.0 + f64::from(i)]).unwrap();
        let ended = session.end(None).unwrap();
        assert_eq!(ended.kept, Kept::Begin, "{i}");
        assert!(
            matches!(
                ticket.try_take(),
                Some(MoveOutcome::Solved { .. } | MoveOutcome::Cancelled | MoveOutcome::Dropped)
            ),
            "{i}"
        );
    }
    let session = open(&solvers, begin_);
    match session.move_to([3.0, 20.0]).unwrap().wait() {
        MoveOutcome::Solved { moved, .. } => {
            assert!(near(at(&moved.values.unwrap(), end), [3.0, 8.0]));
        }
        other => panic!("{other:?}"),
    }
    session.end(None).unwrap();
}

#[test]
fn a_move_once_ends_the_workers_drag_under_way() {
    // One move is its own drag, begun and ended in the request: a drag under way before it
    // is over, so a move after it is the worker's refusal, as no drag is under way.
    let (begin_, _) = fixed_length_line();
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    worker.stopper().unwrap();
    worker.drag_begin(begin_.clone()).unwrap();
    worker.stopper().unwrap();
    assert!(matches!(
        worker.move_once(begin_, [5.0, 30.0]).unwrap(),
        silico_core::sketch::MovedOnce::Moved(_)
    ));
    worker.session_stopper().unwrap();
    let e = worker
        .drag_move([6.0, 30.0], silico_core::drag::DRAG_MOVE_TIMEOUT)
        .unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
    assert_eq!(
        e.message(),
        "DragMove needs a drag under way, and there's none"
    );
}
