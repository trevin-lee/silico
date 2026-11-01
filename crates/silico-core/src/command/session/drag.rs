//! The drag commands (docs/design/m5.0-app.md, section 7, `drag_begin`, `drag_move`,
//! `drag_end`; decision 9; A1, A2, A4): a sketch's point, or a curve's edge, dragged through
//! the core's drag ([`crate::drag::sketch`]), held by the session between its commands.
//!
//! - **`drag_begin` and `drag_end` queue** as commands do (A1), so they apply in the order
//!   they're sent: a begin drags the sketch as the session holds it when it runs, at that
//!   revision, on a sketch worker of its own, never waiting on a rebuild; an end with a
//!   cursor is one edit through the gate at a new revision, undoable inside a transaction,
//!   its own commit outside one.
//! - **`drag_move` runs beside the queue**, on the drag's own thread: it's answered when its
//!   frame comes, the newest move solved and a move replaced before it was answering
//!   `dropped`. Nothing waits on it: each frame is sent to its handle as the drag tells of
//!   it, and the client woken.
//! - **A drag ends** at `drag_end`, at any new revision of the session (an edit, a reload,
//!   an undo: ended as the command that made it answers), when the session closes, or when
//!   its worker fails or its move is cancelled; a drag its worker ended stays the session's,
//!   closed, so a later move or a release with a cursor is refused with the worker's own
//!   error, until a release or a begin takes its place.
//!
//! Its lock ([`DragSlot`]) is taken alone, and never held while a drag ends (which waits on
//! the drag's thread) nor while the client is woken.

use std::cell::Cell;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, Weak};

use super::cancel::{Interrupt, Ticket, Why, cancelled};
use super::sketch_edits::judged;
use super::{
    DragBegun, DragEnded, DragFrame, DragGrab, DragOpen, FrameEnd, Gated, Problems, Response,
    Revision, Running, Session, SessionCommand, SessionId, SessionInfo, Sessions, command_span,
    find_in, not_open, reclaim, refused,
};
use crate::command::edit;
use crate::document::FeatureId;
use crate::drag::sketch::{
    DragStop, EdgeGrab, Frame, FrameKind, FrameTicket, Held, SketchDrag, begin_edge,
    begin_stoppable,
};
use crate::drag::{Kept, Notify};
use crate::kernel::{Clients, Kernel};
use crate::recompute::SketchSolvers;
use crate::sketch_judge::DRAG;
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// A move's thread ended without answering (a bug).
const MOVE_STOPPED: &str = "the drag stopped before the move answered";

/// A session's drag: the one open, or closed by its worker's failure and not yet replaced;
/// how many `drag_begin`s are queued and not yet answered (so a move sent before its begin
/// answered is told why there's no drag); and the next drag's number.
#[derive(Default)]
pub(super) struct DragSlot {
    open: Option<OpenDrag>,
    begins: usize,
    next: u64,
    /// The stop of the drag a `drag_end` is ending now, taken out of `open`: so a close
    /// stops its release's move and solve.
    ending: Option<DragStop>,
}

/// A drag the session holds (A2): the core's drag, its sketch, what it grabbed (the core's
/// drag holds no ID), the revision it began at, the moves sent and not yet answered, and its
/// number, which a move's cancel names it by.
struct OpenDrag {
    drag: SketchDrag,
    sketch: FeatureId,
    grab: DragGrab,
    revision: Revision,
    frames: Arc<Frames>,
    number: u64,
}

impl OpenDrag {
    /// Ended with nothing kept: a move solving then stopped by restarting its worker.
    fn end_quietly(self) {
        let _span = tracing::info_span!("drag_ended", sketch = self.sketch.as_str()).entered();
        if let Err(error) = self.drag.end(None) {
            tracing::warn!(error = ?error, "the drag's end failed");
        }
    }

    /// What `status` says of it.
    fn shown(&self) -> DragOpen {
        DragOpen {
            sketch: self.sketch.clone(),
            grab: self.grab.clone(),
        }
    }
}

/// The moves of a drag sent and not yet answered, each answered as its frame comes
/// ([`answer`](Self::answer)): told by the drag of each change, with no lock of the drag's
/// held. Its lock is taken under no session lock but the drag slot's, and holds none but the
/// session's queue's, a leaf, asked whether the session closed as a move is kept.
struct Frames {
    pending: Mutex<Vec<PendingFrame>>,
    /// The span of the command that began the drag: each frame's events are filed under it,
    /// whichever thread answers the frame (the drag's own, or a `drag_move` that found its
    /// frame already come), so `read_logs` files them alike whatever the scheduling.
    span: tracing::Span,
}

/// A move sent: its frame's ticket, where its answer goes, the session as it was sent, and
/// its stop, which `cancel` and its handle share.
struct PendingFrame {
    ticket: FrameTicket,
    reply: SyncSender<Response<DragFrame>>,
    info: SessionInfo,
    stop: Arc<Ticket>,
    /// Whether its session had closed as it was kept: a close that took the drag's moves
    /// before it was among them told it nothing, so it's told here, and answers `not_open`.
    closed: bool,
    /// What its stop interrupts: held while it's unanswered.
    _interrupt: Arc<dyn Interrupt>,
}

thread_local! {
    /// Set while this thread sends a move under its session's drag lock: a frame answered
    /// then (a move it replaced) wakes the client only once the lock is let go, since a
    /// waker may ask the sessions anything.
    static SENDING: Cell<Option<bool>> = const { Cell::new(None) };
}

impl Frames {
    /// Each move whose frame has come answered: whether any was.
    fn answer(&self) -> bool {
        let answered: Vec<(PendingFrame, Result<Frame, Error>)> = {
            let mut pending = lock(&self.pending);
            let mut answered = Vec::new();
            let mut kept = Vec::with_capacity(pending.len());
            for frame in pending.drain(..) {
                match frame.ticket.try_take() {
                    Some(outcome) => answered.push((frame, outcome)),
                    None => kept.push(frame),
                }
            }
            *pending = kept;
            answered
        };
        let any = !answered.is_empty();
        for (frame, outcome) in answered {
            let PendingFrame {
                reply,
                info,
                stop,
                closed,
                ..
            } = frame;
            let response = stop.answer(
                || Some(info.clone()),
                || Response {
                    session: Some(info.clone()),
                    model: None,
                    // Its session's close ended the drag, or came as the frame did: the
                    // session is gone, and no client stopped the move.
                    result: if closed || stop.closed() {
                        Err(Problems::from(not_open(info.id)))
                    } else {
                        self.span
                            .in_scope(|| drag_frame(outcome))
                            .map_err(Problems::from)
                    },
                },
            );
            // One answer to each channel of one: never waits.
            let _ = reply.try_send(response);
        }
        any
    }

    /// [`answer`](Self::answer), then the client woken if any was: at once, or, on a thread
    /// sending a move under its session's drag lock, once that's let go.
    fn answer_then_wake(&self, waker: &super::waking::Waker) {
        if !self.answer() {
            return;
        }
        let deferred = SENDING.with(|sending| match sending.get() {
            Some(_) => {
                sending.set(Some(true));
                true
            }
            None => false,
        });
        if !deferred {
            waker.wake();
        }
    }

    /// The stops of the moves unanswered: what `cancel` of `drag_move` stops.
    fn stops(&self) -> Vec<Arc<Ticket>> {
        lock(&self.pending)
            .iter()
            .map(|frame| Arc::clone(&frame.stop))
            .collect()
    }
}

/// A move's frame as `drag_move` answers it: a frame of each kind the design gives; a move
/// stopped while it solved (a cancel, or an Esc) `cancelled`; and a drag ended, or its
/// worker failed, the error.
fn drag_frame(outcome: Result<Frame, Error>) -> Result<DragFrame, Error> {
    let frame = match outcome {
        Ok(frame) => frame,
        Err(error) => {
            tracing::info!(
                code = error.code().as_str(),
                "the drag's move failed: {}",
                error.message()
            );
            return Err(error);
        }
    };
    let end = match frame.kind {
        FrameKind::Solved => FrameEnd::Solved,
        FrameKind::Failed => {
            match &frame.why {
                Some(why) => tracing::info!("the drag's move was refused: {}", why.message()),
                None => tracing::info!("the drag's move didn't reach its cursor"),
            }
            FrameEnd::Failed
        }
        FrameKind::Invalid => {
            tracing::info!("the drag's move reached geometry that can't be built");
            FrameEnd::Invalid
        }
        FrameKind::Dropped => FrameEnd::Dropped,
        FrameKind::Cancelled => return Err(cancelled(SessionCommand::DragMove.as_str())),
    };
    Ok(DragFrame {
        to: frame.to,
        frame: end,
        points: frame.points,
        radii: frame.radii,
    })
}

/// A move's stop, interrupting its session's drag as `cancel` of `drag_move` does: the drag
/// ended, never waited for, the move solving stopped by restarting the drag's worker, and
/// every move unanswered answered (the cancelled ones `cancelled`), so a cancel that's said
/// to have stopped one always has. Only the drag the move was sent to, by its number.
struct MoveStop<C: Kernel + 'static> {
    session: Weak<Session<C>>,
    number: u64,
}

impl<C: Kernel + 'static> Interrupt for MoveStop<C> {
    fn interrupt(&self, _why: Why) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        // Ended without waiting, on whichever thread cancels (an app's Esc, MCP's reader):
        // its move killed, the drag closed, and left in the slot, closed, for the session's
        // own thread to join (the next begin, release, new revision, or close).
        let slot = lock(&session.drags);
        if let Some(open) = slot.open.as_ref().filter(|open| open.number == self.number) {
            open.drag.abandon();
        }
    }
}

/// A drag's whole solves stopped (`drag_begin`'s, `drag_end`'s), by its command's cancel.
struct StopDrag(DragStop);

impl Interrupt for StopDrag {
    fn interrupt(&self, _why: Why) {
        self.0.stop();
    }
}

/// A `drag_begin` queued and not yet answered, counted in its session's drag slot while it
/// lives: dropped as its command answers, or is dropped unrun.
struct QueuedBegin<C: Kernel + 'static>(Weak<Session<C>>);

impl<C: Kernel + 'static> QueuedBegin<C> {
    fn counted(session: &Arc<Session<C>>) -> Self {
        lock(&session.drags).begins += 1;
        QueuedBegin(Arc::downgrade(session))
    }
}

impl<C: Kernel + 'static> Drop for QueuedBegin<C> {
    fn drop(&mut self) {
        if let Some(session) = self.0.upgrade() {
            let mut slot = lock(&session.drags);
            slot.begins = slot.begins.saturating_sub(1);
        }
    }
}

/// The refusal of a drag's move or end in session `id`, which has no drag open: `queued`
/// if a `drag_begin` sent to it hasn't answered yet, so the client sent too soon.
fn no_drag(id: SessionId, queued: bool) -> Error {
    let error = Error::new(
        ErrorCode::InvalidInput,
        format!("no drag is open in session {id}"),
    )
    .with_parameter("session");
    if queued {
        error
            .with_cause("a drag_begin sent to the session hasn't answered yet: it may still be queued, and its drag opens when it runs")
            .with_suggestion("send moves once drag_begin has answered")
    } else {
        error
            .with_cause(
                "a drag ends at drag_end, at a new revision of the session, or when its worker restarts",
            )
            .with_suggestion("begin the drag again")
    }
}

/// The refusal of a second drag in session `id`, whose drag `open` is.
fn already_open(id: SessionId, open: &OpenDrag) -> Error {
    let grabbed = match &open.grab {
        DragGrab::Point(point) => point.as_str(),
        DragGrab::Edge { curve, .. } => curve.as_str(),
    };
    Error::new(
        ErrorCode::InvalidInput,
        format!(
            "a drag is already open in session {id}, of {grabbed} in {}",
            open.sketch
        ),
    )
    .with_parameter("session")
    .with_suggestion("end it first: drag_end with to null leaves the sketch as it was")
}

impl<C: Kernel + 'static> Session<C> {
    /// The drag open, as `status` gives it: none once it has ended, its worker's failure
    /// or a cancel among the ways.
    pub(super) fn drag_open(&self) -> Option<DragOpen> {
        let slot = lock(&self.drags);
        slot.open
            .as_ref()
            .filter(|open| open.drag.is_open())
            .map(OpenDrag::shown)
    }

    /// The stops of the drag's moves unanswered: what `cancel` of `drag_move` stops.
    pub(super) fn drag_moves(&self) -> Vec<Arc<Ticket>> {
        let frames = lock(&self.drags)
            .open
            .as_ref()
            .map(|open| Arc::clone(&open.frames));
        frames.map(|frames| frames.stops()).unwrap_or_default()
    }

    /// Ends the drag, if one began at a revision the session has moved on from (an edit, a
    /// reload, an undo): after each command, and the session's own reload.
    pub(super) fn end_stale_drag(&self) {
        let revision = self.held.read().revision;
        let stale = {
            let mut slot = lock(&self.drags);
            if slot
                .open
                .as_ref()
                .is_some_and(|open| open.revision != revision)
            {
                slot.open.take()
            } else {
                None
            }
        };
        if let Some(open) = stale {
            open.end_quietly();
        }
    }

    /// Stops the release of a drag a `drag_end` is ending now, its move and its solve: as
    /// the session closes.
    pub(super) fn stop_release(&self) {
        let ending = lock(&self.drags).ending.clone();
        if let Some(stop) = ending {
            stop.stop();
        }
    }

    /// Ends the drag, if there's one, and stops a release under way: as the session closes.
    /// Its moves unanswered are told the session closed first, so each answers `not_open`,
    /// as a command of a session that isn't open does, never `cancelled`: no client stopped
    /// them.
    pub(super) fn end_drag(&self) {
        for stop in self.drag_moves() {
            stop.close();
        }
        self.stop_release();
        let open = lock(&self.drags).open.take();
        if let Some(open) = open {
            open.end_quietly();
        }
    }

    /// `drag_begin`'s work, as the command `stop` stops: a drag of `grab` in sketch `sketch`
    /// of the session's document at its revision, on a worker of `solvers`, its begin state
    /// the published model's solve of the sketch if it solved the sketch as the revision
    /// holds it, or else solved on the drag's worker.
    fn begin_drag(
        &self,
        stop: &Ticket,
        solvers: &SketchSolvers,
        sketch: FeatureId,
        grab: DragGrab,
    ) -> Result<DragBegun, Problems> {
        let (revision, gated) = {
            let held = self.held.read();
            (held.revision, Arc::clone(&held.gated))
        };
        let replaced = {
            let mut slot = lock(&self.drags);
            if let Some(open) = slot
                .open
                .as_ref()
                .filter(|open| open.drag.is_open() && open.revision == revision)
            {
                return Err(Problems::from(already_open(self.id, open)));
            }
            // A drag its worker ended, closed, or one of a revision since left: this one
            // takes its place.
            slot.open.take()
        };
        if let Some(replaced) = replaced {
            replaced.end_quietly();
        }
        let Gated::Checked(checked) = &*gated else {
            return Err(Problems::from(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "session {}'s document is one the gate refuses, so none of its sketches can be dragged",
                        self.id
                    ),
                )
                .with_parameter("session")
                .with_suggestion("read its problems (check), then mend its text with replace_text"),
            ));
        };
        let published = lock(&self.rebuilds).published.clone();
        let held = published
            .as_ref()
            .and_then(|published| Held::of(&published.model, &sketch));
        let frames = Arc::new(Frames {
            pending: Mutex::new(Vec::new()),
            span: tracing::Span::current(),
        });
        let notify: Notify = {
            let frames = Arc::clone(&frames);
            let waker = Arc::clone(&self.waker);
            Arc::new(move || frames.answer_then_wake(&waker))
        };
        let drag_stop = DragStop::new();
        let interrupt: Arc<dyn Interrupt> = Arc::new(StopDrag(drag_stop.clone()));
        stop.interrupt_with(Arc::downgrade(&interrupt));
        let opened = match &grab {
            DragGrab::Point(point) => begin_stoppable(
                solvers,
                Some(notify),
                checked,
                &sketch,
                point,
                held,
                &drag_stop,
            ),
            DragGrab::Edge { curve, press } => begin_edge(
                solvers,
                Some(notify),
                checked,
                &sketch,
                EdgeGrab {
                    curve,
                    press: *press,
                },
                held,
                &drag_stop,
            ),
        }
        .map_err(|error| {
            // Its session's close stopped the solve, or came as it ended: the session is
            // gone, and the drag's words for a stopped solve never reach the client.
            if stop.closed() {
                Problems::from(not_open(self.id))
            } else {
                Problems::from(error)
            }
        })?;
        // Past here a cancel comes too late: the drag stands, and the begin answers it.
        if let Err(error) = stop.settle() {
            if let Err(why) = opened.drag.end(None) {
                tracing::warn!(error = ?why, "a cancelled begin's drag didn't end");
            }
            return Err(Problems::from(error));
        }
        drop(interrupt);
        if let Some(why) = &opened.unsolved {
            tracing::info!(
                why = why.message(),
                "the sketch didn't solve: the drag begins where its file draws it"
            );
        }
        let begun = DragBegun {
            sketch: sketch.clone(),
            grab: grab.clone(),
            dofs: opened.dofs,
            moves: opened.moves,
        };
        #[cfg(test)]
        self.dragged.pass();
        let number = {
            let mut slot = lock(&self.drags);
            let number = slot.next;
            slot.next = slot.next.wrapping_add(1);
            slot.open = Some(OpenDrag {
                drag: opened.drag,
                sketch,
                grab,
                revision,
                frames,
                number,
            });
            number
        };
        // Its session closed as it began: the close, which marks the begin closed before it
        // ends the drag, may have ended it before the slot held it, so it's ended here (by
        // whichever takes it first), and the begin is refused as of a session not open.
        if stop.closed() {
            let mine = {
                let mut slot = lock(&self.drags);
                match slot.open.as_ref() {
                    Some(open) if open.number == number => slot.open.take(),
                    _ => None,
                }
            };
            if let Some(open) = mine {
                open.end_quietly();
            }
            return Err(Problems::from(not_open(self.id)));
        }
        Ok(begun)
    }

    /// `drag_end`'s work, as the command `stop` stops: the drag ended at `to`, or with no
    /// edit for none, and what it kept made one edit, judged by the sketch's solve on a
    /// solver of `solvers` as every edit that writes geometry is (refused where that solve is
    /// worse than the sketch's before it), read into `read`, its new file claimed with
    /// `claim`.
    fn end_drag_with(
        self: &Arc<Self>,
        stop: &Ticket,
        solvers: &SketchSolvers,
        to: Option<[f64; 2]>,
        read: &mut Option<super::ModelRead>,
        claim: &dyn Fn(&std::path::Path),
    ) -> Result<DragEnded, Problems> {
        let revision = self.held.read().revision;
        let (open, queued) = {
            let mut slot = lock(&self.drags);
            (slot.open.take(), slot.begins > 0)
        };
        let Some(open) = open else {
            return Err(Problems::from(no_drag(self.id, queued)));
        };
        let OpenDrag {
            drag,
            sketch,
            revision: began,
            ..
        } = open;
        let _span =
            tracing::info_span!("drag_end", sketch = sketch.as_str(), cursor = to.is_some())
                .entered();
        // Ended by a revision since (a reload before this command): no drag.
        if began != revision {
            if let Err(why) = drag.end(None) {
                tracing::warn!(error = ?why, "a stale drag didn't end");
            }
            return Err(Problems::from(no_drag(self.id, queued)));
        }
        if to.is_some() && !drag.is_open() {
            // A worker's failure is said with its own code; any other end (a cancel, its
            // thread perhaps still closing) as no drag being open.
            let closed = drag.closed();
            if let Err(why) = drag.end(None) {
                tracing::warn!(error = ?why, "a closed drag didn't end");
            }
            return Err(Problems::from(match closed {
                Some(closed) if closed.code().is_kernel_fault() => closed,
                _ => no_drag(self.id, queued),
            }));
        }
        // Its move and its solve under the drag's stop, which the command's cancel reaches,
        // and the session's close as the sessions are dropped.
        let interrupt: Arc<dyn Interrupt> = Arc::new(StopDrag(drag.stop()));
        lock(&self.drags).ending = Some(drag.stop());
        stop.interrupt_with(Arc::downgrade(&interrupt));
        let ended = drag.end(to);
        lock(&self.drags).ending = None;
        drop(interrupt);
        // Its session's close stopped the release, or came as it ended: the session is gone,
        // and the drag's words for a stopped solve never reach the client.
        if stop.closed() {
            return Err(Problems::from(not_open(self.id)));
        }
        let ended = ended.map_err(Problems::from)?;
        if let Some(why) = &ended.why {
            tracing::info!(why = why.message(), "the drag kept its begin state");
        }
        if ended.kept == Kept::Begin || !ended.changed {
            return Ok(DragEnded {
                sketch,
                kept: ended.kept,
                changed: false,
                commit: None,
            });
        }
        let after = ended.sketch;
        let edited = self.edit(
            stop,
            revision,
            |base| {
                let checked = base.checked()?;
                let made = (
                    (),
                    edit::sketch_replaced(checked.document(), &sketch, after)?,
                );
                judged(solvers, stop, base.id, checked, &sketch, DRAG, made)
            },
            read,
            claim,
        )?;
        Ok(DragEnded {
            sketch,
            kept: Kept::Cursor,
            changed: edited.changed,
            commit: edited.commit,
        })
    }
}

impl<P: Clients + 'static> Sessions<P> {
    /// Begins a drag of `grab` in `session`'s sketch `sketch` (docs/design/m5.0-app.md,
    /// `drag_begin`), queued as every command is (A1), so it drags the sketch as the session
    /// holds it when it runs, at that revision, which the drag's end is an edit against. It
    /// waits only for the commands before it, never for a rebuild: its begin state is the
    /// published model's solve of the sketch, if that solved the sketch as the revision
    /// holds it; otherwise the sketch is solved on the drag's own sketch worker, which it
    /// holds until it ends. A sketch whose solve fails but for a conflict begins where its
    /// file draws it (open item 15). Refused: a drag open in the session already (naming
    /// `session`); a document the gate refused; a feature that isn't a sketch (naming
    /// `sketch`); a point or curve the sketch hasn't (naming `point` or `curve`); a press
    /// that isn't finite, is out of reach, or is more than `LENGTH_TOLERANCE` off its curve's
    /// edge as the begin state draws it (naming `press`: the app projects its pick, which lies
    /// on a tessellation, onto the exact curve); a part of more than
    /// `MAX_DRAG_UNKNOWNS` unknowns (naming `point`, or `curve`); a sketch whose constraints
    /// conflict (`over_constrained`, naming them); and a worker that failed (its error).
    pub fn drag_begin(
        &self,
        session: &SessionId,
        sketch: FeatureId,
        grab: DragGrab,
    ) -> Running<DragBegun> {
        let solvers = self.registry.sketches.clone();
        let queued = self.find(session).map(|found| QueuedBegin::counted(&found));
        self.queued(
            session,
            SessionCommand::DragBegin,
            move |session, stop, _| {
                let _span = tracing::info_span!("drag_begin", sketch = sketch.as_str()).entered();
                let begun = session.begin_drag(stop, &solvers, sketch, grab);
                // Answered: no longer queued, the drag (if it began) already the session's.
                drop(queued);
                begun
            },
        )
    }

    /// Moves `session`'s drag's cursor to `to` (docs/design/m5.0-app.md, `drag_move`), in
    /// the sketch's coordinates: run beside the session's queue, on the drag's own thread,
    /// never waiting (A1). Answered when its frame comes: `solved`, with the part's points
    /// and free radii; `failed`, `invalid`, or `dropped` (a newer move replaced it before it
    /// was solved), with none. A move stopped while it solved (a `cancel` of `drag_move`, an
    /// Esc) answers `cancelled`, and its drag has ended. Refused: no drag open (naming
    /// `session`, saying so if a `drag_begin` hasn't answered yet); a cursor that isn't
    /// finite or is out of reach (naming `to`; the drag goes on); and a drag its worker
    /// ended, with the worker's own error. A move past `DRAG_MOVE_TIMEOUT` is
    /// `worker_timeout`, and ends the drag.
    pub fn drag_move(&self, session: &SessionId, to: [f64; 2]) -> Running<DragFrame> {
        let Some(found) = find_in(&self.registry, session) else {
            return Running::answered(refused(not_open(*session)));
        };
        let _span = command_span(&found.logs, found.id, "drag_move", None).entered();
        let info = found.info(false);
        let here = |error: Error| {
            Running::answered(Response {
                session: Some(info.clone()),
                model: None,
                result: Err(Problems::from(error)),
            })
        };
        let slot = lock(&found.drags);
        let Some(open) = slot.open.as_ref() else {
            return here(no_drag(found.id, slot.begins > 0));
        };
        if !open.drag.is_open() {
            let closed = open.drag.closed();
            let queued = slot.begins > 0;
            drop(slot);
            return here(match closed {
                Some(closed) if closed.code().is_kernel_fault() => closed,
                _ => no_drag(found.id, queued),
            });
        }
        SENDING.set(Some(false));
        let sent = open.drag.move_to(to);
        let wake = SENDING.replace(None) == Some(true);
        let frames = Arc::clone(&open.frames);
        let number = open.number;
        drop(slot);
        if wake {
            found.waker.wake();
        }
        let ticket = match sent {
            Ok(ticket) => ticket,
            Err(error) => return here(error),
        };
        let stop = Ticket::new(SessionCommand::DragMove);
        let interrupt: Arc<dyn Interrupt> = Arc::new(MoveStop {
            session: Arc::downgrade(&found),
            number,
        });
        stop.interrupt_with(Arc::downgrade(&interrupt));
        let (reply, answer) = mpsc::sync_channel(1);
        {
            let mut pending = lock(&frames.pending);
            // A close that took the drag's moves before this one was kept told it nothing: it
            // learns here, under the lock its answer is given under, so it answers `not_open`
            // too, never `cancelled` (the close takes the queue before it takes the moves).
            // Only marked, not closed: closing its stop interrupts it, which takes the drag
            // slot's lock, taken before this one as a move is sent (the robustness review).
            let closed = found.closed();
            pending.push(PendingFrame {
                ticket,
                reply,
                info,
                stop: Arc::clone(&stop),
                closed,
                _interrupt: interrupt,
            });
        }
        // Its frame may have come before it was kept.
        if frames.answer() {
            found.waker.wake();
        }
        Running::new(answer, MOVE_STOPPED, Some(stop), None)
    }

    /// Ends `session`'s drag (docs/design/m5.0-app.md, `drag_end`), queued as every command
    /// is (A1). With a cursor `to`: the sketch moved there from the begin state (the newest
    /// solved move's values, if its cursor was `to` exactly) and solved as a whole, and its
    /// points' coordinates, as solved, the sketch's: one edit through the gate at a new
    /// revision, held and undoable inside a transaction, committed outside one. A function of
    /// the begin state and `to` alone (A5). The begin state is kept, with no edit, if the
    /// final move or the solve fails, and for a release where the drag began, a click. With
    /// none (Esc): the drag ended with no edit, a move solving then stopped by restarting the
    /// drag's worker. Refused: no drag open (a new revision since the begin ended it); a
    /// cursor that isn't finite or is out of reach (naming `to`; the drag ended with no
    /// edit); a drag its worker ended, given a cursor, with the worker's error; the gate's
    /// errors for the sketch, the drag ended with no edit; and, as every edit that writes
    /// geometry is, a sketch whose solve as written is worse than its solve before the drag
    /// ("after the drag, ..."), the drag ended with no edit, though a problem the sketch had
    /// already isn't the drag's: a redundancy it adds, the one way a drag's can be worse,
    /// since the drag's own solve keeps the begin state for a conflict, a failure, or a
    /// result the file can't hold, and its file solves as that solve did (M3.4).
    pub fn drag_end(&self, session: &SessionId, to: Option<[f64; 2]>) -> Running<DragEnded> {
        let registry = Arc::downgrade(&self.registry);
        let solvers = self.registry.sketches.clone();
        self.queued(
            session,
            SessionCommand::DragEnd,
            move |session, stop, model| {
                let id = session.id;
                session.end_drag_with(stop, &solvers, to, model, &|resolved| {
                    reclaim(&registry, id, resolved);
                })
            },
        )
    }
}
