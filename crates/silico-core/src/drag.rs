//! A drag of a sketch's point, in the core (ARCHITECTURE.md, *Sketches solve in the sketch
//! worker*: the pool's rules, planned for M5; docs/design/m5.0-app.md, `drag_begin`,
//! `drag_move`, `drag_end`, decision 9, pending the maintainer). Below any command:
//!
//! - **One worker.** A drag takes a sketch solver from the solvers a session's engines share
//!   ([`SketchSolvers`]) and holds it for as long as the drag lasts, apart from every
//!   recompute: its moves all go to the worker that began it (a sketch worker holds one
//!   session), and no recompute's supersession stops that worker, since none has it lent.
//! - **The newest cursor.** Its moves are solved one at a time, on a thread of the drag's
//!   own, under [`DRAG_MOVE_TIMEOUT`] each; a move sent while one solves waits, and a newer
//!   one replaces it, the replaced one answering [`MoveOutcome::Dropped`], never solved.
//! - **The end from the begin state.** [`DragSession::end`] with a cursor moves the sketch
//!   from the begin state to the final cursor (the newest solved move's values, if its
//!   cursor has the same bits, or else one move more), and then solves it as a whole. So
//!   the result is a function of the begin state and the final cursor alone, however many
//!   moves were dropped. If the move or the solve fails, the begin state is kept. With no
//!   cursor (Esc), the drag ends with nothing kept, a move solving then stopped by
//!   restarting its worker.
//!
//! A drag ends at its end, when its worker fails (a move past its time, a crash: the
//! worker restarts, and the session with it), or when the session is dropped; a move after
//! that is refused, and the client begins again.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use silico_protocol::{
    DragBegin, Moved, SketchSystem, SolveStatus, Solved, Tag, check_drag_target,
};

use crate::recompute::SketchSolvers;
use crate::sketch::{SketchSolver, SolveStopper};
use crate::worker::{lock, thread_error};
use crate::{Error, ErrorCode};

/// How long a drag's move may take (docs/design/m5.0-app.md, its limits): ten times the
/// most `MAX_DRAG_UNKNOWNS` was sized for (0.76 s at 256 unknowns, loaded, M3.7). A move past
/// it is hung, and ends the drag. A whole-sketch solve, the begin's and the end's, runs
/// under the solver's own time, as any solve does.
pub const DRAG_MOVE_TIMEOUT: Duration = Duration::from_secs(10);

/// A drag begun: open, or not started for the constraints in conflict, by tag.
pub enum Begun {
    Open { session: DragSession, dofs: i32 },
    Conflicting(Vec<Tag>),
}

/// What became of a move.
#[derive(Debug, Clone, PartialEq)]
pub enum MoveOutcome {
    /// Solved, from the begin state, for the cursor `to`: the worker's frame, whose status
    /// says whether it reached the cursor (values only with `Success`).
    Solved { to: [f64; 2], moved: Moved },
    /// Replaced by a newer move before it was sent: never solved.
    Dropped,
    /// Stopped while it solved, by the drag's end with no cursor.
    Cancelled,
    /// The worker failed solving it (past its time, a crash): the drag has ended.
    Failed(Error),
    /// The drag had ended before the move could run.
    Closed(Error),
}

/// Which state a drag's end kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// The sketch moved to the final cursor and solved as a whole.
    Cursor,
    /// The sketch as the drag began: no cursor given, or the final move or the solve failed.
    Begin,
}

/// How a drag ended: the state kept, and with the cursor's, the whole sketch's solve.
#[derive(Debug, Clone, PartialEq)]
pub struct Ended {
    pub kept: Kept,
    pub solved: Option<Solved>,
}

/// A drag open on its own sketch solver.
pub struct DragSession {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// A move sent: its outcome, when it has one.
pub struct MoveTicket {
    shared: Arc<Shared>,
    id: u64,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// The next move's number.
    next: u64,
    /// The newest move not yet sent: its number and cursor.
    waiting: Option<(u64, [f64; 2])>,
    /// The move solving, and how to stop it.
    running: Option<(u64, Option<SolveStopper>)>,
    /// Each move's outcome, by number, until its ticket takes it.
    outcomes: BTreeMap<u64, MoveOutcome>,
    /// The newest move that reached its cursor: the cursor's bits and the values.
    last: Option<([u64; 2], Vec<f64>)>,
    /// The end asked for: with a cursor, or none.
    end: Option<Option<[f64; 2]>>,
    /// Stopping the move solving, for an end with no cursor.
    cancelling: bool,
    /// Why the drag is no longer open, once it isn't.
    closed: Option<Error>,
    /// The end's result, once it's run.
    ended: Option<Result<Ended, Error>>,
}

/// Begins a drag: `begin` checked (its part within `MAX_DRAG_UNKNOWNS`, as the worker
/// checks it), a solver taken from `solvers` and held, and the drag begun on it, the sketch
/// diagnosed once. A sketch whose constraints conflict starts no drag, its solver given back.
/// Refused: a `begin` that fails its check (naming its parameter), and a solver that
/// couldn't be had or failed (its error).
pub fn begin(solvers: &SketchSolvers, begin: DragBegin) -> Result<Begun, Error> {
    let _span = tracing::info_span!("drag_begin").entered();
    if let Err(invalid) = begin.check() {
        return Err(Error::new(ErrorCode::InvalidInput, invalid.message)
            .with_parameter(invalid.parameter)
            .with_suggestion("change the value the message names"));
    }
    let mut solver = solvers.take()?;
    let started = solver
        .stopper()
        .and_then(|_| solver.drag_begin(begin.clone()));
    let started = match started {
        Ok(started) => started,
        Err(error) => {
            solvers.give_back(solver);
            return Err(error);
        }
    };
    if !started.conflicting.is_empty() {
        solvers.give_back(solver);
        return Ok(Begun::Conflicting(started.conflicting));
    }
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        changed: Condvar::new(),
    });
    let running = Arc::clone(&shared);
    let back = solvers.clone();
    let thread = std::thread::Builder::new()
        .name("silico-drag".to_owned())
        .spawn(move || {
            let solver = drag(&running, solver, &begin);
            back.give_back(solver);
        })
        .map_err(|e| thread_error("drag", &e))?;
    Ok(Begun::Open {
        session: DragSession {
            shared,
            thread: Some(thread),
        },
        dofs: started.dofs,
    })
}

impl DragSession {
    /// Sends a move to `to`: solved when the move before it has been, unless a newer one
    /// replaces it first. Refused: a cursor that isn't finite or is out of reach (M3.7's
    /// `check_drag_target`, naming `to`); the drag stays open.
    pub fn move_to(&self, to: [f64; 2]) -> Result<MoveTicket, Error> {
        if let Err(invalid) = check_drag_target(to) {
            return Err(Error::new(ErrorCode::InvalidInput, invalid.message)
                .with_parameter(invalid.parameter)
                .with_suggestion("move the cursor within the sketch's reach"));
        }
        let mut state = lock(&self.shared.state);
        let id = state.next;
        state.next += 1;
        if let Some(closed) = state.closed.clone() {
            state.outcomes.insert(id, MoveOutcome::Closed(closed));
        } else if let Some((replaced, _)) = state.waiting.replace((id, to)) {
            state.outcomes.insert(replaced, MoveOutcome::Dropped);
        }
        drop(state);
        self.shared.changed.notify_all();
        Ok(MoveTicket {
            shared: Arc::clone(&self.shared),
            id,
        })
    }

    /// Ends the drag. With a cursor `to`: the sketch moved from the begin state to `to` (the
    /// newest solved move's values, if its cursor is `to`'s bits, or one move more) and solved
    /// as a whole; the begin state kept if either fails. With none: nothing kept, a move
    /// solving now stopped by restarting the worker. A move waiting is dropped either way.
    /// Refused: a cursor that isn't finite or is out of reach (the drag ended with nothing
    /// kept); a drag that had already ended (why); and a worker that failed the end's move,
    /// its end, or its solve (its error).
    pub fn end(mut self, to: Option<[f64; 2]>) -> Result<Ended, Error> {
        let _span = tracing::info_span!("drag_end", cursor = to.is_some()).entered();
        let refused = to.and_then(|to| check_drag_target(to).err());
        let to = if refused.is_some() { None } else { to };
        let result = self.finish(to);
        match refused {
            Some(invalid) => Err(Error::new(ErrorCode::InvalidInput, invalid.message)
                .with_parameter(invalid.parameter)
                .with_suggestion("end the drag again: it ended with the sketch as it began")),
            None => result,
        }
    }

    /// Asks the drag's thread to end with `to`, and waits for it.
    fn finish(&mut self, to: Option<[f64; 2]>) -> Result<Ended, Error> {
        let mut state = lock(&self.shared.state);
        if state.end.is_none() && state.ended.is_none() {
            state.end = Some(to);
            if to.is_none()
                && let Some((_, Some(stopper))) = state.running.clone()
            {
                state.cancelling = true;
                stopper.stop();
            }
        }
        self.shared.changed.notify_all();
        while state.ended.is_none() {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let ended = state
            .ended
            .clone()
            .unwrap_or_else(|| Err(no_drag("its thread ended")));
        drop(state);
        if let Some(thread) = self.thread.take() {
            // The thread has set its end, so it's ending: it gives its solver back, and
            // returns. A thread that panicked has nothing more to say.
            let _ = thread.join();
        }
        ended
    }
}

impl Drop for DragSession {
    /// A drag dropped open ends with nothing kept, as an end with no cursor.
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.finish(None);
        }
    }
}

impl MoveTicket {
    /// Waits for the move's outcome.
    pub fn wait(&self) -> MoveOutcome {
        let mut state = lock(&self.shared.state);
        loop {
            if let Some(outcome) = state.outcomes.remove(&self.id) {
                return outcome;
            }
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The move's outcome, if it has one yet.
    pub fn try_take(&self) -> Option<MoveOutcome> {
        lock(&self.shared.state).outcomes.remove(&self.id)
    }
}

/// The drag's thread: each newest move solved in turn, until the end is asked for or the
/// worker fails; then the end's work, and its result set. Gives back the solver.
fn drag(
    shared: &Shared,
    mut solver: Box<dyn SketchSolver>,
    begin: &DragBegin,
) -> Box<dyn SketchSolver> {
    loop {
        let mut state = lock(&shared.state);
        while state.waiting.is_none() && state.end.is_none() {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if let Some(to) = state.end {
            if let Some((dropped, _)) = state.waiting.take() {
                state.outcomes.insert(dropped, MoveOutcome::Dropped);
            }
            let last = state.last.clone();
            drop(state);
            let ended = ending(solver.as_mut(), begin, to, last);
            let mut state = lock(&shared.state);
            state.closed = Some(no_drag("it ended"));
            state.ended = Some(ended);
            drop(state);
            shared.changed.notify_all();
            return solver;
        }
        let Some((id, to)) = state.waiting.take() else {
            continue;
        };
        let stopper = match solver.stopper() {
            Ok(stopper) => stopper,
            Err(error) => {
                state
                    .outcomes
                    .insert(id, MoveOutcome::Failed(error.clone()));
                close(&mut state, error);
                drop(state);
                shared.changed.notify_all();
                return solver;
            }
        };
        state.running = Some((id, stopper));
        drop(state);
        let solved = {
            let _span = tracing::debug_span!("drag_move").entered();
            solver.drag_move(to, DRAG_MOVE_TIMEOUT)
        };
        let mut state = lock(&shared.state);
        state.running = None;
        match solved {
            Ok(moved) => {
                if let (SolveStatus::Success, Some(values)) = (moved.status, &moved.values) {
                    state.last = Some((to.map(f64::to_bits), values.clone()));
                }
                state.outcomes.insert(id, MoveOutcome::Solved { to, moved });
                drop(state);
                shared.changed.notify_all();
            }
            Err(error) => {
                // The worker failed or was stopped: its session went with it.
                let outcome = if state.cancelling {
                    MoveOutcome::Cancelled
                } else {
                    MoveOutcome::Failed(error.clone())
                };
                state.outcomes.insert(id, outcome);
                close(&mut state, error);
                drop(state);
                shared.changed.notify_all();
                return solver;
            }
        }
    }
}

/// The drag no longer open, for `why`: a move waiting dropped, and an end asked for (or
/// one to come) answered: with no cursor, nothing kept; with one, refused, as the drag
/// ended first.
fn close(state: &mut State, why: Error) {
    if let Some((dropped, _)) = state.waiting.take() {
        state.outcomes.insert(dropped, MoveOutcome::Dropped);
    }
    let closed = no_drag("its worker failed").with_cause(why.message().to_owned());
    state.ended = Some(match (state.cancelling, state.end) {
        (true, _) | (false, Some(None)) => Ok(Ended {
            kept: Kept::Begin,
            solved: None,
        }),
        _ => Err(closed.clone()),
    });
    state.closed = Some(closed);
}

/// The end's work on the solver: with a cursor, the sketch moved there from the begin
/// state and solved as a whole, or the begin state kept; with none, nothing kept.
fn ending(
    solver: &mut dyn SketchSolver,
    begin: &DragBegin,
    to: Option<[f64; 2]>,
    last: Option<([u64; 2], Vec<f64>)>,
) -> Result<Ended, Error> {
    let kept_begin = Ended {
        kept: Kept::Begin,
        solved: None,
    };
    let Some(to) = to else {
        // The worker's session ends with it; a failure to end it changes nothing kept.
        let _ = solver.stopper().and_then(|_| solver.drag_end());
        return Ok(kept_begin);
    };
    // The values at the final cursor, from the begin state: the newest solved move's, if it
    // was this cursor, or one move more.
    let values = match last {
        Some((bits, values)) if bits == to.map(f64::to_bits) => Some(values),
        _ => {
            solver.stopper()?;
            let moved = solver.drag_move(to, DRAG_MOVE_TIMEOUT)?;
            match (moved.status, moved.values) {
                (SolveStatus::Success, Some(values)) => Some(values),
                _ => None,
            }
        }
    };
    solver.stopper()?;
    solver.drag_end()?;
    let Some(values) = values else {
        return Ok(kept_begin);
    };
    let system = SketchSystem {
        values,
        ..begin.system.clone()
    };
    solver.stopper()?;
    let solved = solver.solve(system)?;
    match solved.status {
        SolveStatus::Success | SolveStatus::SolvedExceptRedundant if !solved.values.is_empty() => {
            Ok(Ended {
                kept: Kept::Cursor,
                solved: Some(solved),
            })
        }
        _ => Ok(kept_begin),
    }
}

/// A drag that's no longer open, and `why`.
fn no_drag(why: &str) -> Error {
    Error::new(ErrorCode::InvalidInput, format!("no drag is open: {why}"))
        .with_cause("a drag ends at its end, when its worker fails, or when it's dropped")
        .with_suggestion("begin the drag again")
}
