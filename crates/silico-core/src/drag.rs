//! A drag of a sketch's point, in the core (ARCHITECTURE.md, *Sketches solve in the sketch
//! worker*: the pool's rules, planned for M5; docs/design/m5.0-app.md, `drag_begin`,
//! `drag_move`, `drag_end`, decision 9, pending the maintainer). Below any command:
//!
//! - **One worker.** A drag takes a sketch solver from the solvers a session's engines share
//!   ([`SketchSolvers`]) and holds it for as long as the drag lasts, apart from every
//!   recompute: its begin state is solved on it ([`begin_with`]), its moves all go to it (a
//!   sketch worker holds one session), and no recompute's supersession stops it, since none
//!   has it lent.
//! - **The newest cursor.** Its moves are solved one at a time, on a thread of the drag's
//!   own, under [`DRAG_MOVE_TIMEOUT`] each; a move sent while one solves waits, and a newer
//!   one replaces it, the replaced one answering [`MoveOutcome::Dropped`], never solved. Each
//!   change of a move's outcome, or of the drag, is told to the drag's notifier, so a client
//!   needn't wait on a ticket to learn of it.
//! - **The end from the begin state.** [`DragSession::end`] with a cursor moves the sketch
//!   from the begin state to the final cursor (the newest solved move's values, if its
//!   cursor has the same bits, or else one move more), and then solves it as a whole. So
//!   the result is a function of the begin state and the final cursor alone, however many
//!   moves were dropped. If the move or the solve fails, the begin state is kept. With no
//!   cursor (Esc), the drag ends with nothing kept, a move solving then stopped by
//!   restarting its worker.
//!
//! A drag ends at its end, when its worker fails (a move past its time, a crash: the
//! worker restarts, and the session with it), when its move is cancelled, or when the
//! session is dropped; a move after that is refused, and the client begins again. Every
//! reply the worker gives is checked as a solve's is: one that can't be the sketch's is the
//! worker's protocol error, and the worker is stopped.

pub mod sketch;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use silico_protocol::{
    DragBegin, Dragged, Moved, SketchGeometry, SketchSystem, SolveStatus, Solved, SolverConstraint,
    Tag, Value, check_drag_target,
};

use crate::recompute::SketchSolvers;
use crate::sketch::{DragStarted, SketchSolver, SolveStopper, check_solved};
use crate::worker::{lock, thread_error};
use crate::{Error, ErrorCode};

/// How long a drag's move may take (docs/design/m5.0-app.md, its limits): ten times the
/// most `MAX_DRAG_UNKNOWNS` was sized for (0.76 s at 256 unknowns, loaded, M3.7). A move past
/// it is hung, and ends the drag; so is a drag's end in the worker, which solves nothing. A
/// whole-sketch solve, the begin's and the end's, runs under the solver's own time, as any
/// solve does.
pub const DRAG_MOVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Told of each change of a drag's moves or state, with no lock held: the caller's waker.
pub use crate::notify::Notify;

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
    /// Refused by the worker (its solution out of reach, say), which keeps the drag: the
    /// client keeps its last frame, and the drag goes on (the design's `failed` frame).
    Refused(Error),
    /// Stopped while it solved: by the drag's end with no cursor, or a cancel.
    Cancelled,
    /// The worker failed solving it (past its time, a crash, a reply it can't be trusted
    /// for): the drag has ended.
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

/// How a drag ended: the state kept; with the cursor's, the values the move to it reached
/// and, from [`DragSession::end`], the whole sketch's solve of them; and where a cursor was
/// given but the begin state kept, why.
#[derive(Debug, Clone, PartialEq)]
pub struct Ended {
    pub kept: Kept,
    pub values: Option<Vec<f64>>,
    pub solved: Option<Solved>,
    pub why: Option<Error>,
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
    notify: Option<Notify>,
    /// The drag's whole work's stop (its begin's, given to [`begin_under`]): the end's move
    /// to its cursor runs under it too, so what stops the end's solve stops that move.
    stop: DragStop,
}

impl Shared {
    /// Wakes the waiters and tells the notifier: called with no lock held.
    fn changed(&self) {
        self.changed.notify_all();
        if let Some(notify) = &self.notify {
            notify();
        }
    }
}

#[derive(Default)]
struct State {
    /// The next move's number.
    next: u64,
    /// The newest move not yet sent: its number and cursor.
    waiting: Option<(u64, [f64; 2])>,
    /// The move solving, and how to stop it.
    running: Option<(u64, Option<SolveStopper>)>,
    /// Each move's outcome, by number, until its ticket is dropped.
    outcomes: BTreeMap<u64, MoveOutcome>,
    /// Tickets dropped before their outcome came: it isn't kept.
    abandoned: BTreeSet<u64>,
    /// The newest move that reached its cursor: the cursor's bits and the values.
    last: Option<([u64; 2], Vec<f64>)>,
    /// The end asked for: with a cursor, or none.
    end: Option<Option<[f64; 2]>>,
    /// Whether the end solves the sketch as a whole ([`DragSession::end`]), or leaves that
    /// to its caller ([`DragSession::release`]).
    whole: bool,
    /// Stopping the move solving: for an end with no cursor, or a cancel.
    cancelling: bool,
    /// Why the drag is no longer open, once it isn't.
    closed: Option<Error>,
    /// The end's result, once it's run.
    ended: Option<Result<Ended, Error>>,
    /// Whether the drag's thread has returned.
    done: bool,
}

/// Stops the whole-sketch work a drag's begin or end runs (its solves, and the begin's
/// diagnosis in the worker), from another thread (the
/// command's `cancel`), by restarting its worker; that begin or end is then refused as
/// `cancelled`, and one not yet solving when it's stopped is refused before it solves.
/// Clones stop the same drag's.
#[derive(Clone, Default)]
pub struct DragStop(Arc<Mutex<Stopping>>);

#[derive(Default)]
struct Stopping {
    stopped: bool,
    stopper: Option<SolveStopper>,
}

impl DragStop {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stops the whole solve running under it, if one is, and every one after.
    pub fn stop(&self) {
        let mut stopping = lock(&self.0);
        stopping.stopped = true;
        if let Some(stopper) = &stopping.stopper {
            stopper.stop();
        }
    }

    /// `solve` on `solver`, readied for it, stoppable by [`stop`](Self::stop): a drag's whole
    /// solves, and a sketch edit's (`command`'s), whose command's `cancel` stops it so.
    pub(crate) fn run<T>(
        &self,
        solver: &mut dyn SketchSolver,
        solve: impl FnOnce(&mut dyn SketchSolver) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let stopper = solver.stopper()?;
        self.under(stopper, || solve(solver))
    }

    /// `solve`, a request `stopper` stops, stoppable by [`stop`](Self::stop): refused as
    /// `cancelled` if it's stopped before it starts, or while it runs.
    fn under<T>(
        &self,
        stopper: Option<SolveStopper>,
        solve: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        {
            let mut stopping = lock(&self.0);
            if stopping.stopped {
                return Err(cancelled());
            }
            stopping.stopper = stopper;
        }
        let result = solve();
        let mut stopping = lock(&self.0);
        stopping.stopper = None;
        if stopping.stopped {
            return Err(cancelled());
        }
        result
    }
}

/// A drag's whole solve, stopped.
pub(crate) fn cancelled() -> Error {
    Error::new(
        ErrorCode::Cancelled,
        "the drag's solve of its sketch was stopped",
    )
    .with_suggestion("begin the drag again")
}

/// Begins a drag of `begin`: as [`begin_with`], its begin state given.
pub fn begin(solvers: &SketchSolvers, begin: DragBegin) -> Result<Begun, Error> {
    // Checked before a solver is taken: nothing is sent for a begin that can't run.
    checked_begin(&begin)?;
    begin_with(solvers, None, |_| Ok(begin))
}

/// Begins a drag: a solver taken from `solvers` and held; the begin state made by `prepare`
/// on it (the core's solve of the revision's sketch, where the model doesn't hold it: so
/// the drag never takes a second worker, nor waits on a rebuild), checked (its part within
/// `MAX_DRAG_UNKNOWNS`, as the worker checks it); and the drag begun on it, the sketch
/// diagnosed once. Each change of its moves or state is told to `notify`. A sketch whose
/// constraints conflict starts no drag, its solver given back. Refused: a begin that fails
/// its check (naming its parameter); `prepare`'s error; and a solver that couldn't be had
/// or failed, or answered what it can't be trusted for (its error).
pub fn begin_with(
    solvers: &SketchSolvers,
    notify: Option<Notify>,
    prepare: impl FnOnce(&mut dyn SketchSolver) -> Result<DragBegin, Error>,
) -> Result<Begun, Error> {
    begin_under(solvers, notify, &DragStop::new(), prepare)
}

/// Begins a drag as [`begin_with`] does, the worker's begin (its diagnosis of the whole
/// sketch) under `stop`, as `prepare` runs its solves: a stop refuses the begin as
/// `cancelled`.
pub fn begin_under(
    solvers: &SketchSolvers,
    notify: Option<Notify>,
    stop: &DragStop,
    prepare: impl FnOnce(&mut dyn SketchSolver) -> Result<DragBegin, Error>,
) -> Result<Begun, Error> {
    let _span = tracing::info_span!("drag_begin").entered();
    let mut solver = solvers.take()?;
    let (begin, started) = match prepared(solver.as_mut(), prepare, stop) {
        Ok(prepared) => prepared,
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
        notify,
        stop: stop.clone(),
    });
    let running = Arc::clone(&shared);
    let back = solvers.clone();
    // The begin's span, the drag's thread's parent: its moves' events are its caller's (a
    // session's, filed in its logs).
    let parent = tracing::Span::current();
    let thread = std::thread::Builder::new()
        .name("silico-drag".to_owned())
        .spawn(move || {
            let _parent = parent.entered();
            let _guard = Unwinding(&running);
            if let Some(solver) = drag(&running, solver, &begin, &back) {
                back.give_back(solver);
            }
            lock(&running.state).done = true;
            running.changed();
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

/// The begin state made on `solver`, checked, and the drag begun on it.
fn prepared(
    solver: &mut dyn SketchSolver,
    prepare: impl FnOnce(&mut dyn SketchSolver) -> Result<DragBegin, Error>,
    stop: &DragStop,
) -> Result<(DragBegin, DragStarted), Error> {
    // `prepare` readies the solver for what it sends, as any solve's caller does.
    let begin = prepare(solver)?;
    checked_begin(&begin)?;
    let started = stop.run(solver, |solver| solver.drag_begin(begin.clone()))?;
    if let Err(error) = checked_tags(&begin.system, &started.conflicting) {
        solver.distrust();
        return Err(error);
    }
    Ok((begin, started))
}

/// `begin`'s own check, as the worker makes it: refused naming its parameter. A part past
/// `MAX_DRAG_UNKNOWNS` (the one check a single move doesn't make) is told what to drag
/// instead (docs/design/m5.0-app.md, `drag_begin`).
fn checked_begin(begin: &DragBegin) -> Result<(), Error> {
    begin.check().map_err(|invalid| {
        if invalid.parameter == "press" {
            return Error::new(ErrorCode::InvalidInput, invalid.message)
                .with_parameter("press")
                .with_suggestion(ON_THE_EDGE);
        }
        let suggestion = if begin.check_move().is_ok() {
            "drag a point of a smaller connected part, or set the value with a dimension"
        } else {
            "change the value the message names"
        };
        Error::new(ErrorCode::InvalidInput, invalid.message)
            .with_parameter(invalid.parameter)
            .with_suggestion(suggestion)
    })?;
    if let Dragged::Edge { curve, press } = begin.dragged
        && let Some(off) = off_the_edge(&begin.system, curve, press)
        && off > silico_protocol::LENGTH_TOLERANCE
    {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "the drag's press at ({}, {}) is {} mm off the edge it drags: a press lands on its edge",
                silico_protocol::format_number(press[0]),
                silico_protocol::format_number(press[1]),
                silico_protocol::format_number(off)
            ),
        )
        .with_parameter("press")
        .with_suggestion(ON_THE_EDGE));
    }
    Ok(())
}

/// What a press is told to do: land on the edge it drags.
const ON_THE_EDGE: &str =
    "press a point of the edge, as a pick of it gives: a line's, or its circle's or arc's";

/// How far `press` is from geometry `curve`'s edge in `system`, as the begin state draws it
/// (a line's segment, a circle, an arc between its ends), in millimetres; none for geometry
/// that isn't a curve, which the drag's own check refuses. Exactly, in `f64`: 0 for a press
/// computed on the edge, within a few ulps.
fn off_the_edge(
    system: &SketchSystem,
    curve: silico_protocol::Geometry,
    press: [f64; 2],
) -> Option<f64> {
    let at = |v: Value| system.values.get(v.0 as usize).copied();
    let point = |p: silico_protocol::PointValues| Some([at(p.x)?, at(p.y)?]);
    let distance = silico_protocol::distance;
    match system.geometry.get(curve.0 as usize)? {
        SketchGeometry::Line { start, end } => {
            let (a, b) = (point(*start)?, point(*end)?);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let (px, py) = (press[0] - a[0], press[1] - a[1]);
            let squared = dx * dx + dy * dy;
            let t = if squared > 0.0 {
                (px * dx + py * dy) / squared
            } else {
                0.0
            };
            // Beside the line, its distance from it measured across the line's unit direction,
            // never by a foot rounded along the line (which put a press exactly the tolerance
            // off a horizontal line a hair past it): exact for a line along an axis, whose
            // direction is (±1, 0) or (0, ±1). Past an end, from that end.
            Some(if t <= 0.0 {
                distance(press, a)
            } else if t >= 1.0 {
                distance(press, b)
            } else {
                let length = libm::hypot(dx, dy);
                ((dx / length) * py - (dy / length) * px).abs()
            })
        }
        SketchGeometry::Circle { center, radius } => {
            Some((distance(press, point(*center)?) - at(*radius)?).abs())
        }
        SketchGeometry::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            start,
            end,
        } => {
            let c = point(*center)?;
            let (from, to) = (at(*start_angle)?, at(*end_angle)?);
            let angle = libm::atan2(press[1] - c[1], press[0] - c[0]);
            // Counter-clockwise from its start's angle: within its sweep, or past it.
            let turn = std::f64::consts::TAU;
            let sweep = (to - from).rem_euclid(turn);
            let into = (angle - from).rem_euclid(turn);
            if into <= sweep {
                Some((distance(press, c) - at(*radius)?).abs())
            } else {
                Some(distance(press, point(*start)?).min(distance(press, point(*end)?)))
            }
        }
        SketchGeometry::Point(_) => None,
    }
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
            settle(&mut state, id, MoveOutcome::Closed(closed));
        } else if let Some((replaced, _)) = state.waiting.replace((id, to)) {
            settle(&mut state, replaced, MoveOutcome::Dropped);
        }
        drop(state);
        self.shared.changed();
        Ok(MoveTicket {
            shared: Arc::clone(&self.shared),
            id,
        })
    }

    /// Stops the move solving now, if one is, by restarting its worker (`cancel` of a
    /// `drag_move`): it answers [`MoveOutcome::Cancelled`], whatever the worker answers, and
    /// the drag ends, as its worker's session does. Doesn't wait. Whether a move was solving.
    pub fn cancel_move(&self) -> bool {
        let mut state = lock(&self.shared.state);
        match state.running.clone() {
            Some((_, Some(stopper))) => {
                state.cancelling = true;
                // Under the lock, so the worker is still the drag's: its thread gives it back
                // only once it has the lock again.
                stopper.stop();
                true
            }
            _ => false,
        }
    }

    /// Ends the drag with nothing kept, as [`end`](Self::end) with no cursor does, but
    /// without waiting: a move solving now is stopped by restarting the worker, and the
    /// drag's thread ends its worker's session and closes the drag on its own. Its own
    /// [`end`](Self::end) (or its drop) then answers with nothing kept, joining the thread.
    /// What a cancel from a thread that mustn't wait (an app's) calls.
    pub fn abandon(&self) {
        let mut state = lock(&self.shared.state);
        if state.end.is_none() && state.ended.is_none() && state.closed.is_none() {
            state.end = Some(None);
            state.whole = true;
            if let Some((_, Some(stopper))) = state.running.clone() {
                state.cancelling = true;
                stopper.stop();
            }
        }
        drop(state);
        // The drag's thread woken; the notifier is told by that thread as the moves settle,
        // never on this one.
        self.shared.changed.notify_all();
    }

    /// Whether the drag is still open: no end, no failure, no cancel yet.
    pub fn is_open(&self) -> bool {
        let state = lock(&self.shared.state);
        state.closed.is_none() && state.end.is_none()
    }

    /// Why the drag is no longer open, if it isn't.
    pub fn closed(&self) -> Option<Error> {
        lock(&self.shared.state).closed.clone()
    }

    /// Ends the drag. With a cursor `to`: the sketch moved from the begin state to `to` (the
    /// newest solved move's values, if its cursor is `to`'s bits, or one move more) and solved
    /// as a whole; the begin state kept if either fails. With none: nothing kept, a move
    /// solving now stopped by restarting the worker, and a drag its worker's failure already
    /// ended answered alike. A move waiting is dropped either way. Refused: a cursor that
    /// isn't finite or is out of reach (the drag ended with nothing kept); a cursor for a
    /// drag that had already ended (why, a failure's with the worker's own code); and a
    /// worker that failed the end's move or its solve, or answered what it can't be trusted
    /// for (its error).
    pub fn end(self, to: Option<[f64; 2]>) -> Result<Ended, Error> {
        let _span = tracing::info_span!("drag_end", cursor = to.is_some()).entered();
        self.close(to, true)
    }

    /// Ends the drag as [`end`](Self::end) does, but with a cursor, solves the sketch at it
    /// no further than the move there: the values it reached are the caller's to solve as a
    /// whole (the core's document drag, which settles them as a load, `drag::sketch`), with
    /// a solver of its own, under a stopper of its own. So the end runs no whole solve of
    /// its own: only a move to the cursor, if the newest frame wasn't of it, which its
    /// time ([`DRAG_MOVE_TIMEOUT`]) bounds.
    pub fn release(self, to: Option<[f64; 2]>) -> Result<Ended, Error> {
        let _span = tracing::info_span!("drag_release", cursor = to.is_some()).entered();
        self.close(to, false)
    }

    fn close(mut self, to: Option<[f64; 2]>, whole: bool) -> Result<Ended, Error> {
        let refused = to.and_then(|to| check_drag_target(to).err());
        let to = if refused.is_some() { None } else { to };
        let result = self.finish(to, whole);
        match refused {
            Some(invalid) => Err(Error::new(ErrorCode::InvalidInput, invalid.message)
                .with_parameter(invalid.parameter)
                .with_suggestion("the drag ended with the sketch as it began: begin it again")),
            None => result,
        }
    }

    /// Asks the drag's thread to end with `to`, and waits for it.
    fn finish(&mut self, to: Option<[f64; 2]>, whole: bool) -> Result<Ended, Error> {
        let mut state = lock(&self.shared.state);
        if state.end.is_none() && state.ended.is_none() && state.closed.is_none() {
            state.end = Some(to);
            state.whole = whole;
            if to.is_none()
                && let Some((_, Some(stopper))) = state.running.clone()
            {
                state.cancelling = true;
                stopper.stop();
            }
        }
        drop(state);
        self.shared.changed();
        let mut state = lock(&self.shared.state);
        while state.ended.is_none() && !state.done {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let ended = match (state.ended.clone(), &state.closed, to) {
            (Some(ended), _, _) => ended,
            // Ended by a failure before this end: nothing to keep, as Esc asks; a cursor
            // is refused, saying why.
            (None, _, None) => Ok(Ended {
                kept: Kept::Begin,
                values: None,
                solved: None,
                why: None,
            }),
            (None, Some(closed), Some(_)) => Err(closed.clone()),
            (None, None, Some(_)) => Err(no_drag("its thread ended")),
        };
        drop(state);
        if let Some(thread) = self.thread.take() {
            // The thread has set its end, or closed the drag, so it's returning. A thread
            // that panicked has nothing more to say.
            let _ = thread.join();
        }
        ended
    }
}

impl Drop for DragSession {
    /// A drag dropped open ends with nothing kept, as an end with no cursor.
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.finish(None, true);
        }
    }
}

impl MoveTicket {
    /// Waits for the move's outcome: as often as it's asked, the same.
    pub fn wait(&self) -> MoveOutcome {
        let mut state = lock(&self.shared.state);
        loop {
            if let Some(outcome) = state.outcomes.get(&self.id) {
                return outcome.clone();
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
        lock(&self.shared.state).outcomes.get(&self.id).cloned()
    }
}

impl Drop for MoveTicket {
    /// The outcome goes with its ticket, so a client that sends a move per event and drops
    /// the tickets keeps none.
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        if state.outcomes.remove(&self.id).is_none() {
            state.abandoned.insert(self.id);
        }
    }
}

/// `outcome` kept for move `id`'s ticket, unless the ticket was dropped first.
fn settle(state: &mut State, id: u64, outcome: MoveOutcome) {
    if !state.abandoned.remove(&id) {
        state.outcomes.insert(id, outcome);
    }
}

/// The drag's thread unwinding from a panic (a solver's, under a request): every move and
/// the end answered, so no client waits forever, and the drag closed. Its solver is lost
/// with the panic; the pool starts another.
struct Unwinding<'a>(&'a Shared);

impl Drop for Unwinding<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let mut state = lock(&self.0.state);
        let why = Error::new(
            ErrorCode::Internal,
            "the drag's thread panicked under its solver",
        )
        .with_suggestion(crate::worker::SILICO_BUG);
        if let Some((id, _)) = state.running.take() {
            settle(&mut state, id, MoveOutcome::Failed(why.clone()));
        }
        if let Some((id, _)) = state.waiting.take() {
            settle(&mut state, id, MoveOutcome::Dropped);
        }
        if state.ended.is_none() {
            state.ended = Some(Err(why.clone()));
        }
        state.closed = Some(why);
        state.done = true;
        drop(state);
        self.0.changed();
    }
}

/// The drag's thread: each newest move solved in turn, until the end is asked for or the
/// worker fails; then the end's work, and its result set. Returns the solver for the thread
/// to give back once its end has been asked for (the end joins the thread, so its caller
/// finds it back); a move that ends the drag (its worker failed, or it was cancelled) gives
/// it back to `back` itself, before that move's outcome is told, so a client woken by the
/// outcome finds it back too, and a drag begun then starts no other worker.
fn drag(
    shared: &Shared,
    mut solver: Box<dyn SketchSolver>,
    begin: &DragBegin,
    back: &SketchSolvers,
) -> Option<Box<dyn SketchSolver>> {
    // Each frame holds every value outside the part where the drag began.
    let part = begin.part();
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
                settle(&mut state, dropped, MoveOutcome::Dropped);
            }
            let last = state.last.clone();
            let whole = state.whole;
            drop(state);
            let ended = ending(solver.as_mut(), begin, &part, to, last, whole, &shared.stop);
            let mut state = lock(&shared.state);
            state.closed = Some(no_drag("it ended"));
            state.ended = Some(ended);
            drop(state);
            shared.changed();
            return Some(solver);
        }
        let Some((id, to)) = state.waiting.take() else {
            continue;
        };
        let stopper = match solver.session_stopper() {
            Ok(stopper) => stopper,
            Err(error) => {
                drop(state);
                back.give_back(solver);
                let mut state = lock(&shared.state);
                settle(&mut state, id, MoveOutcome::Failed(error.clone()));
                close(&mut state, &error);
                drop(state);
                shared.changed();
                return None;
            }
        };
        state.running = Some((id, stopper));
        drop(state);
        let solved = {
            let _span = tracing::debug_span!("drag_move").entered();
            solver.drag_move(to, DRAG_MOVE_TIMEOUT)
        };
        // A frame that can't be the sketch's is the worker's fault, as a solve's is.
        let solved = solved.and_then(|moved| match checked_frame(&part, &moved) {
            Ok(()) => Ok(startable(&begin.system, moved)),
            Err(error) => {
                solver.distrust();
                Err(error)
            }
        });
        let mut state = lock(&shared.state);
        state.running = None;
        // A move that ends the drag gives its solver back before its outcome is told. With
        // the move no longer running, no cancel can come between.
        let ends =
            state.cancelling || matches!(&solved, Err(error) if error.code().is_kernel_fault());
        if ends {
            drop(state);
            back.give_back(solver);
            state = lock(&shared.state);
            match solved {
                // Cancelled as it answered: the worker is stopped all the same, its session
                // with it, so the drag ends as the cancel said.
                _ if state.cancelling => {
                    settle(&mut state, id, MoveOutcome::Cancelled);
                    close(&mut state, &no_drag("its move was stopped"));
                }
                // The worker failed: its session went with it.
                Err(error) => {
                    settle(&mut state, id, MoveOutcome::Failed(error.clone()));
                    close(&mut state, &error);
                }
                Ok(_) => {}
            }
            drop(state);
            shared.changed();
            return None;
        }
        match solved {
            Ok(moved) => {
                if let (SolveStatus::Success, Some(values)) = (moved.status, &moved.values) {
                    state.last = Some((to.map(f64::to_bits), values.clone()));
                }
                settle(&mut state, id, MoveOutcome::Solved { to, moved });
            }
            // Refused by the worker, which keeps its drag: the drag goes on.
            Err(error) => settle(&mut state, id, MoveOutcome::Refused(error)),
        }
        drop(state);
        shared.changed();
    }
}

/// The drag no longer open, for `why`: a move waiting dropped, and an end already asked
/// for answered (with no cursor, or by a cancel, nothing kept; with one, refused, as the
/// drag ended first). A failure is said with the worker's own code, so it's a fault, not the
/// client's; a cancel as no drag being open.
fn close(state: &mut State, why: &Error) {
    if let Some((dropped, _)) = state.waiting.take() {
        settle(state, dropped, MoveOutcome::Dropped);
    }
    let closed = if state.cancelling {
        no_drag("its move was stopped")
    } else {
        Error::new(why.code(), "no drag is open: its worker failed")
            .with_cause(why.message().to_owned())
            .with_suggestion("begin the drag again: the next begin starts another worker")
    };
    if let Some(end) = state.end {
        // By the end asked for alone, so it answers alike however the cancel raced it.
        state.ended = Some(match end {
            None => Ok(Ended {
                kept: Kept::Begin,
                values: None,
                solved: None,
                why: None,
            }),
            Some(_) => Err(closed.clone()),
        });
    }
    state.closed = Some(closed);
}

/// The end's work on the solver: with a cursor, the sketch moved there from the begin
/// state and solved as a whole, or the begin state kept, saying why; with none, nothing
/// kept. A move or a whole solve the worker refuses, rather than fails, keeps the begin
/// state. A worker that ended took the drag's session with it: a frame in hand is still
/// solved whole, on another worker, since the closing solve carries the whole sketch.
fn ending(
    solver: &mut dyn SketchSolver,
    begin: &DragBegin,
    part: &SketchSystem,
    to: Option<[f64; 2]>,
    last: Option<([u64; 2], Vec<f64>)>,
    whole: bool,
    stop: &DragStop,
) -> Result<Ended, Error> {
    let kept_begin = |why: Option<Error>| Ended {
        kept: Kept::Begin,
        values: None,
        solved: None,
        why,
    };
    let Some(to) = to else {
        // The worker's session ends with it; a failure to end it changes nothing kept.
        let _ = solver.session_stopper().and_then(|_| solver.drag_end());
        return Ok(kept_begin(None));
    };
    // The values at the final cursor, from the begin state: the newest solved move's, if it
    // was this cursor, or one move more.
    let values = match last {
        Some((bits, values)) if bits == to.map(f64::to_bits) => Ok(values),
        _ => {
            // Under the drag's stop: an end that's stopped (its command's cancel, its
            // session's close) stops this move, as a cancel stops a move.
            let stopper = solver.session_stopper()?;
            match stop.under(stopper, || solver.drag_move(to, DRAG_MOVE_TIMEOUT)) {
                Ok(moved) => {
                    if let Err(error) = checked_frame(part, &moved) {
                        solver.distrust();
                        return Err(error);
                    }
                    let moved = startable(&begin.system, moved);
                    match (moved.status, moved.values) {
                        (SolveStatus::Success, Some(values)) => Ok(values),
                        (SolveStatus::SolvedInvalid, _) => Err(unbuildable_at_cursor()),
                        _ => Err(Error::new(
                            ErrorCode::SolveFailed,
                            "the solver didn't reach the drag's final cursor from where the drag began",
                        )
                        .with_suggestion("release the drag nearer where it began")),
                    }
                }
                Err(error) if error.code() == ErrorCode::Cancelled => return Err(error),
                Err(error) if !error.code().is_kernel_fault() => Err(error),
                Err(error) => return Err(error),
            }
        }
    };
    // The worker's session ends; one already gone (its worker ended) needs no end.
    let _ = solver.session_stopper().and_then(|_| solver.drag_end());
    let values = match values {
        Ok(values) => values,
        Err(why) => return Ok(kept_begin(Some(why))),
    };
    if !whole {
        return Ok(Ended {
            kept: Kept::Cursor,
            values: Some(values),
            solved: None,
            why: None,
        });
    }
    let system = SketchSystem {
        values: values.clone(),
        ..begin.system.clone()
    };
    solver.stopper()?;
    let solved = match solver.solve(system.clone()) {
        Ok(solved) => solved,
        Err(error) if !error.code().is_kernel_fault() => return Ok(kept_begin(Some(error))),
        Err(error) => return Err(error),
    };
    if let Err(error) = check_solved(&system, &solved) {
        solver.distrust();
        return Err(error);
    }
    match solved.status {
        SolveStatus::Success | SolveStatus::SolvedExceptRedundant => Ok(Ended {
            kept: Kept::Cursor,
            values: Some(values),
            solved: Some(solved),
            why: None,
        }),
        SolveStatus::SolvedInvalid => Ok(kept_begin(Some(unbuildable_at_cursor()))),
        _ => Ok(kept_begin(Some(
            Error::new(
                ErrorCode::SolveFailed,
                "the sketch didn't solve as a whole at the drag's final cursor",
            )
            .with_suggestion("release the drag nearer where it began"),
        ))),
    }
}

/// `moved`, but a `Success` whose values the next solve couldn't start from (its system's
/// own check refuses them: a point held on a circle drawn at that circle's centre, say,
/// which a radius just above 0 leaves the worker taking) as `SolvedInvalid`, its values
/// withheld, as M3.7 withholds a frame whose geometry can't be built.
fn startable(system: &SketchSystem, moved: Moved) -> Moved {
    match (moved.status, &moved.values) {
        (SolveStatus::Success, Some(values))
            if SketchSystem {
                values: values.clone(),
                ..system.clone()
            }
            .check()
            .is_err() =>
        {
            Moved {
                status: SolveStatus::SolvedInvalid,
                values: None,
            }
        }
        _ => moved,
    }
}

/// The final cursor solved to geometry that can't be built: the worker withholds such a
/// frame's values, so what can't be is said, not named.
fn unbuildable_at_cursor() -> Error {
    Error::new(
        ErrorCode::InvalidResult,
        "at the drag's final cursor the sketch solves to geometry that can't be built: a line of no length, a circle or arc of no radius, or a point held on a circle drawn at its centre",
    )
    .with_suggestion("release the drag where the geometry it moves keeps its size")
}

/// Refuses a frame unless it could be `part`'s: values exactly with a `Success`, as many as
/// it has, its fixed ones (every value outside the part too) as sent, bit for bit, and
/// every one within reach.
pub(crate) fn checked_frame(part: &SketchSystem, moved: &Moved) -> Result<(), Error> {
    let bad = |problem: String| {
        Err(Error::new(
            ErrorCode::ProtocolError,
            format!("the sketch worker's drag frame {problem}"),
        )
        .with_suggestion("begin the drag again: its worker is started again"))
    };
    let values = match (moved.status, &moved.values) {
        (SolveStatus::Success, Some(values)) => values,
        (SolveStatus::Success, None) => return bad("is a success with no values".to_owned()),
        (status, Some(_)) => return bad(format!("has values with the status {status:?}")),
        (_, None) => return Ok(()),
    };
    if values.len() != part.values.len() {
        return bad(format!(
            "has {} values for {} sent",
            values.len(),
            part.values.len()
        ));
    }
    for (i, (sent, got)) in part.values.iter().zip(values).enumerate() {
        let unknown =
            u32::try_from(i).is_ok_and(|i| part.unknowns.binary_search(&Value(i)).is_ok());
        if !unknown && sent.to_bits() != got.to_bits() {
            return bad(format!("moves value {i}, which is fixed"));
        }
        if !silico_protocol::within_reach(*got) {
            return bad(format!("has value {i} out of reach"));
        }
    }
    Ok(())
}

/// Refuses a begin's constraints in conflict unless they're tags `system` was sent, in
/// increasing order, each once.
pub(crate) fn checked_tags(system: &SketchSystem, conflicting: &[Tag]) -> Result<(), Error> {
    let sent: BTreeSet<Tag> = system
        .constraints
        .iter()
        .filter_map(SolverConstraint::tag)
        .collect();
    if conflicting.windows(2).any(|w| w[0] >= w[1]) || conflicting.iter().any(|t| !sent.contains(t))
    {
        return Err(Error::new(
            ErrorCode::ProtocolError,
            "the sketch worker's drag begin lists conflicting tags it wasn't sent, or out of order",
        )
        .with_suggestion("begin the drag again: its worker is started again"));
    }
    Ok(())
}

/// A drag that's no longer open, and `why`.
fn no_drag(why: &str) -> Error {
    Error::new(ErrorCode::InvalidInput, format!("no drag is open: {why}"))
        .with_cause("a drag ends at its end, when its worker fails, or when it's dropped")
        .with_suggestion("begin the drag again")
}

#[cfg(test)]
mod tests {
    use super::{Begun, MoveOutcome, begin, lock, off_the_edge};
    use crate::Error;
    use crate::fake::FakeSketches;
    use crate::recompute::SketchSolvers;
    use crate::sketch::{DragStarted, SketchSolver, SolveStopper};
    use silico_protocol::{
        Algorithm, DragBegin, Geometry, Moved, PointOf, PointValues, SketchGeometry, SketchSystem,
        Solved, Value,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn a_point() -> DragBegin {
        DragBegin {
            system: SketchSystem {
                values: vec![4.0, 8.0],
                unknowns: vec![Value(0), Value(1)],
                geometry: vec![SketchGeometry::Point(PointValues {
                    x: Value(0),
                    y: Value(1),
                })],
                constraints: vec![],
                algorithms: vec![Algorithm::DogLeg],
            },
            dragged: silico_protocol::Dragged::Point(PointOf::Point(Geometry(0))),
        }
    }

    /// The point of values `x` and `y` of a system.
    fn values_at(x: u32, y: u32) -> PointValues {
        PointValues {
            x: Value(x),
            y: Value(y),
        }
    }

    /// A system of one curve, `geometry` 0, over `values`.
    fn one_curve(values: Vec<f64>, curve: SketchGeometry) -> SketchSystem {
        SketchSystem {
            values,
            unknowns: vec![],
            geometry: vec![curve],
            constraints: vec![],
            algorithms: vec![Algorithm::DogLeg],
        }
    }

    /// How far `press` is from the line from `a` to `b`.
    fn off_line(a: [f64; 2], b: [f64; 2], press: [f64; 2]) -> f64 {
        let system = one_curve(
            vec![a[0], a[1], b[0], b[1]],
            SketchGeometry::Line {
                start: values_at(0, 1),
                end: values_at(2, 3),
            },
        );
        off_the_edge(&system, Geometry(0), press).unwrap()
    }

    /// `off` is `by hand` to within a few ulps: for a line along no axis, whose unit direction
    /// isn't exact in binary.
    fn about(off: f64, by_hand: f64) {
        assert!(
            (off - by_hand).abs() <= 4.0 * f64::EPSILON * by_hand,
            "{off} for {by_hand}"
        );
    }

    #[test]
    fn a_press_beside_a_line_is_measured_across_it_and_one_past_an_end_from_that_end() {
        // Up the y axis, from (0, 0) to (0, 4). (1, 3) is beside it, three quarters along
        // (its foot (0, 3)): 1 across. Its foot's place along the line is the press's
        // projection over the length squared, 12 / 16: reckoned over 4 + 4, 12 / 8, it would
        // lie past the end, and be √2 from (0, 4).
        assert_eq!(off_line([0.0, 0.0], [0.0, 4.0], [1.0, 3.0]), 1.0);
        // On the line's own axis, past its end and before its start: from the nearer end, 1
        // and 2. Projected as 0 + (5 + 4) = 9 over 16, or (-2 + 4) = 2 over 16, each would
        // fall on the segment and read 0.
        assert_eq!(off_line([0.0, 0.0], [0.0, 4.0], [0.0, 5.0]), 1.0);
        assert_eq!(off_line([0.0, 0.0], [0.0, 4.0], [0.0, -2.0]), 2.0);
        // A diagonal, from (0, 0) to (3, 4), of length 5 and direction (0.6, 0.8). (5.5, -1)
        // is its midpoint (1.5, 2) and 5 along its normal (0.8, -0.6): 5 across it,
        // |0.6 × -1 - 0.8 × 5.5|. Not 0.6 × -1 + 0.8 × 5.5 = 3.8, nor with the direction's y
        // as 4 % 5 or 4 × 5.
        about(off_line([0.0, 0.0], [3.0, 4.0], [5.5, -1.0]), 5.0);
        // Past its ends along it: (6, 8) is 5 from (3, 4), and (-3, -4) 5 from (0, 0).
        assert_eq!(off_line([0.0, 0.0], [3.0, 4.0], [6.0, 8.0]), 5.0);
        assert_eq!(off_line([0.0, 0.0], [3.0, 4.0], [-3.0, -4.0]), 5.0);
    }

    #[test]
    fn a_press_by_a_line_of_no_length_is_measured_from_its_point() {
        // Both ends at (1, 2): a press at (4, 6) is 5 from it (3, 4, 5), and one on it 0.
        // Its projection, 0 over a length squared of 0, is no number: a press measured by it
        // would read NaN, never more than the tolerance, and be taken wherever it was.
        assert_eq!(off_line([1.0, 2.0], [1.0, 2.0], [4.0, 6.0]), 5.0);
        assert_eq!(off_line([1.0, 2.0], [1.0, 2.0], [1.0, 2.0]), 0.0);
    }

    #[test]
    fn a_press_by_an_arc_away_from_the_origin_is_on_its_sweep_by_its_angle_about_its_center() {
        // The quarter of radius 5 about (10, 20) from angle π to 3π/2: from (5, 20) to
        // (10, 15).
        let system = one_curve(
            vec![
                10.0,
                20.0,
                5.0,
                std::f64::consts::PI,
                3.0 * std::f64::consts::FRAC_PI_2,
                5.0,
                20.0,
                10.0,
                15.0,
            ],
            SketchGeometry::Arc {
                center: values_at(0, 1),
                radius: Value(2),
                start_angle: Value(3),
                end_angle: Value(4),
                start: values_at(5, 6),
                end: values_at(7, 8),
            },
        );
        let off = |press: [f64; 2]| off_the_edge(&system, Geometry(0), press).unwrap();
        // (4, 12) is (-6, -8) from the center, at 10, beyond the arc's middle (at angle
        // π + 0.93, within the sweep): 10 - 5 off its circle. Its angle reckoned from the
        // press's y plus the center's (atan2(32, -6)), or its x plus the center's
        // (atan2(-8, 14)) or over it (atan2(-8, 0.4)), lies off the sweep, and the press
        // would be measured from its nearer end instead, √45 from (10, 15).
        assert_eq!(off([4.0, 12.0]), 5.0);
        // (13, 11) is (3, -9) from the center, at angle -1.25, past the sweep's end: from
        // its nearer end, (10, 15), 5, not from its circle (√90 - 5).
        assert_eq!(off([13.0, 11.0]), 5.0);
        // A circle about (10, 20) of radius 5: (4, 12) is 10 - 5 from it.
        let circle = one_curve(
            vec![10.0, 20.0, 5.0],
            SketchGeometry::Circle {
                center: values_at(0, 1),
                radius: Value(2),
            },
        );
        assert_eq!(off_the_edge(&circle, Geometry(0), [4.0, 12.0]), Some(5.0));
    }

    /// The fake, but its drag's requests stopped only in name: each stop counted, the move
    /// solving left as it is (held, if the fake holds it), so a test can stop it again
    /// while it still runs.
    struct CountedStops {
        fake: FakeSketches,
        stops: Arc<AtomicUsize>,
    }

    impl SketchSolver for CountedStops {
        fn solve(&mut self, system: SketchSystem) -> Result<Solved, Error> {
            self.fake.solve(system)
        }

        fn session_stopper(&mut self) -> Result<Option<SolveStopper>, Error> {
            let stops = Arc::clone(&self.stops);
            Ok(Some(SolveStopper::new(move || {
                stops.fetch_add(1, Ordering::SeqCst);
            })))
        }

        fn drag_begin(&mut self, begin: DragBegin) -> Result<DragStarted, Error> {
            self.fake.drag_begin(begin)
        }

        fn drag_move(&mut self, to: [f64; 2], timeout: Duration) -> Result<Moved, Error> {
            self.fake.drag_move(to, timeout)
        }

        fn drag_end(&mut self) -> Result<(), Error> {
            self.fake.drag_end()
        }
    }

    #[test]
    fn a_drag_abandoned_again_while_its_move_still_solves_stops_that_move_once() {
        // A cancel of `drag_move` stops each of the drag's moves unanswered, each of which
        // abandons the drag: the move solving and the one waiting abandon it twice. The
        // first stops the move solving and ends the drag; the second, the drag already
        // ending, stops nothing again, though the move hasn't yet answered.
        let fake = FakeSketches::new();
        let stops = Arc::new(AtomicUsize::new(0));
        let solvers = {
            let (fake, stops) = (fake.clone(), Arc::clone(&stops));
            SketchSolvers::starting(move || {
                Ok(Box::new(CountedStops {
                    fake: fake.clone(),
                    stops: Arc::clone(&stops),
                }) as Box<dyn SketchSolver>)
            })
        };
        let Begun::Open { session, .. } = begin(&solvers, a_point()).unwrap() else {
            panic!("conflicting")
        };
        fake.hold();
        let moving = session.move_to([1.0, 1.0]).unwrap();
        assert!(fake.wait_held(Duration::from_secs(60)));
        let waiting = session.move_to([2.0, 2.0]).unwrap();
        session.abandon();
        session.abandon();
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        fake.release();
        assert_eq!(moving.wait(), MoveOutcome::Cancelled);
        assert_eq!(waiting.wait(), MoveOutcome::Dropped);
        assert!(!session.is_open());
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert_eq!(session.end(None).unwrap().kept, super::Kept::Begin);
    }

    #[test]
    fn a_dropped_tickets_outcome_isnt_kept_whether_it_came_before_or_after() {
        // A drag of many moves keeps none of them once their tickets go: one waited for and
        // dropped, one dropped while it's waiting (so its outcome comes after), and one
        // dropped unanswered at the end.
        let solvers =
            SketchSolvers::starting(|| Ok(Box::new(FakeSketches::new()) as Box<dyn SketchSolver>));
        let Begun::Open { session, .. } = begin(&solvers, a_point()).unwrap() else {
            panic!("conflicting")
        };
        let shared = Arc::clone(&session.shared);
        let waited = session.move_to([1.0, 1.0]).unwrap();
        assert!(matches!(waited.wait(), MoveOutcome::Solved { .. }));
        drop(waited);
        drop(session.move_to([2.0, 2.0]).unwrap());
        let last = session.move_to([3.0, 3.0]).unwrap();
        assert!(matches!(last.wait(), MoveOutcome::Solved { .. }));
        session.end(None).unwrap();
        drop(last);
        let state = lock(&shared.state);
        assert!(state.outcomes.is_empty(), "{:?}", state.outcomes.keys());
        assert!(state.abandoned.is_empty(), "{:?}", state.abandoned);
    }
}
