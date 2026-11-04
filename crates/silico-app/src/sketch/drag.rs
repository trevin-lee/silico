//! A drag in sketch mode (docs/design/m5.0-app.md, section 9, *Dragging*; section 7's
//! `drag_begin`, `drag_move`, and `drag_end`; section 3's one move in flight): a press on an
//! under-defined point, or a line's, a circle's, or an arc's edge, with no tool on, that
//! moves past [`SKETCH_DRAG_THRESHOLD`](super::style::SKETCH_DRAG_THRESHOLD) drags it through
//! the core's drag commands, never waited on.
//!
//! - The begin is sent once the press has moved and nothing of the tab's is under way (the
//!   tab says when), so the drag drags the sketch the read drew; until it answers, no move
//!   is sent (section 7: a move sent before is refused).
//! - Then one move at a time, of the newest cursor only: a cursor given while one is in
//!   flight waits, and a newer one replaces it; each solved frame redraws the dragged part
//!   ([`Drag::shown`]), the begin state's ghost beneath it.
//! - The release sends `drag_end` with its cursor, once the begin has answered: one edit,
//!   one undo step. A cancel, or Esc, sends it with none, ending the drag from the begin
//!   state; a press cancelled before its begin went out sends nothing.
//! - The frame the drag ended on is drawn until a read of the revision its edit made comes.

use eframe::egui::Pos2;
use silico_core::command::sketch::SketchRead;
use silico_core::command::{
    DragBegun, DragEnded, DragFrame, DragGrab, FrameEnd, Problems, Response, Revision, Running,
    SessionId, Sessions,
};
use silico_core::document::{Coordinate, FeatureId};
use silico_core::kernel::Clients;

/// Where a drag is.
enum Phase {
    /// Pressed, nothing sent: a drag once it moves, and the tab lets it begin.
    Pressed,
    /// Its begin sent, not yet answered.
    Beginning(Running<DragBegun>),
    /// Open: the move in flight, if one is, and the cursor it was sent with.
    Open {
        moving: Option<(Running<DragFrame>, [f64; 2])>,
    },
    /// Its end sent, not yet answered.
    Ending(Running<DragEnded>),
    /// Ended, its edit at `made`: its last frame drawn until a read of that revision.
    Ended { made: Revision },
    /// Over: nothing more to send, draw, or wait for.
    Over,
}

/// How the press ended: released at a cursor on the plane, or cancelled (a cancel, Esc, or
/// a release off the plane before any cursor on it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Release {
    At([f64; 2]),
    Cancel,
}

/// What a drag's poll found, for the tab to take: its begin refused, a move the core
/// refused (its cursor, the drag going on) or answered as ending the drag, its end sent,
/// and its end's answer.
pub enum News {
    Refused(Problems),
    EndSent,
    Ended(Response<DragEnded>),
}

/// A drag, from its press to its end.
pub struct Drag {
    /// The sketch, and what's dragged: a point, or a curve's edge with where on it the press
    /// was, put on the exact curve.
    pub sketch: FeatureId,
    pub grab: DragGrab,
    /// The read it was pressed on: the begin state, as drawn, and its ghost's.
    begin: SketchRead,
    /// Where it was pressed on the screen, and whether it has moved past the threshold.
    pub(crate) press: Pos2,
    pub(crate) moved: bool,
    /// The newest cursor on the plane given since the press.
    cursor: Option<[f64; 2]>,
    /// How it was ended, once it was.
    released: Option<Release>,
    phase: Phase,
    /// The begin state with the newest solved frame's values, once one came.
    shown: Option<SketchRead>,
    /// How many solved frames it has drawn, and how many moves it has sent: what the
    /// drawing is keyed on, and what a test counts.
    pub frames: u64,
    pub moves_sent: u64,
    /// The cursor each move was sent with, in order: what a test reads.
    pub sent: Vec<[f64; 2]>,
}

impl Drag {
    /// A press on `grab` of `sketch`, at `press` on the screen, over the read `begin`.
    pub fn pressed(sketch: FeatureId, grab: DragGrab, begin: SketchRead, press: Pos2) -> Self {
        Drag {
            sketch,
            grab,
            begin,
            press,
            moved: false,
            cursor: None,
            released: None,
            phase: Phase::Pressed,
            shown: None,
            frames: 0,
            moves_sent: 0,
            sent: Vec::new(),
        }
    }

    /// Whether its press is still held: neither released nor cancelled.
    pub fn held(&self) -> bool {
        self.released.is_none()
    }

    /// The cursor, on the plane, moved to `at`: the newest, which the next move sends.
    pub fn cursor(&mut self, at: [f64; 2]) {
        if self.held() && at[0].is_finite() && at[1].is_finite() {
            self.cursor = Some(at);
        }
    }

    /// Released at `at` on the plane (none off it: its newest cursor on the plane then, or a
    /// cancel with none).
    pub fn release(&mut self, at: Option<[f64; 2]>) {
        if !self.held() {
            return;
        }
        self.cursor(at.unwrap_or([f64::NAN; 2]));
        self.released = Some(match self.cursor {
            Some(at) => Release::At(at),
            None => Release::Cancel,
        });
    }

    /// Cancelled (a cancel of the press, or Esc): ended from the begin state. Whether it
    /// was held to cancel.
    pub fn cancel(&mut self) -> bool {
        if !self.held() {
            return false;
        }
        self.released = Some(Release::Cancel);
        true
    }

    /// Whether its begin waits to be sent: it has moved, and wasn't cancelled before.
    pub fn wants_begin(&self) -> bool {
        matches!(self.phase, Phase::Pressed) && self.moved && self.released != Some(Release::Cancel)
    }

    /// Its begin, sent.
    pub fn begun(&mut self, running: Running<DragBegun>) {
        self.phase = Phase::Beginning(running);
    }

    /// Whether anything of it is under way with the core: its begin, moves, or end sent and
    /// not answered, or the drag open. The tab's queue waits for it.
    pub fn in_flight(&self) -> bool {
        matches!(
            self.phase,
            Phase::Beginning(_) | Phase::Open { .. } | Phase::Ending(_)
        )
    }

    /// Whether it's over: nothing more to send or draw. A press cancelled, or released
    /// without having moved, before its begin went out is too.
    pub fn over(&self) -> bool {
        matches!(self.phase, Phase::Over)
            || (matches!(self.phase, Phase::Pressed)
                && (self.released == Some(Release::Cancel)
                    || (self.released.is_some() && !self.moved)))
    }

    /// Whether it's open: its begin answered, its end not yet sent.
    pub fn open(&self) -> bool {
        matches!(self.phase, Phase::Open { .. })
    }

    /// Whether its end has been sent, or it never began.
    pub fn ended(&self) -> bool {
        matches!(
            self.phase,
            Phase::Ending(_) | Phase::Ended { .. } | Phase::Over
        )
    }

    /// What's drawn of the sketch while it lasts: the begin state with the newest solved
    /// frame's values, or the begin state until one comes; none before it has moved.
    pub fn shown(&self) -> Option<&SketchRead> {
        if !self.moved || matches!(self.phase, Phase::Over) {
            return None;
        }
        Some(self.shown.as_ref().unwrap_or(&self.begin))
    }

    /// The begin state, as the press found it.
    pub fn begin(&self) -> &SketchRead {
        &self.begin
    }

    /// A read of the model at `model` has come: a drag ended at that revision or before is
    /// over, the read drawing its edit.
    pub fn read(&mut self, model: Option<Revision>) {
        if let Phase::Ended { made } = self.phase
            && model.is_some_and(|model| model >= made)
        {
            self.phase = Phase::Over;
        }
    }

    /// Its answers polled and what's next sent, without waiting: the begin's answer (a
    /// refusal ends it, said); then, open, the move in flight's answer (a solved frame
    /// redrawn; a refusal ends it, said), and the newest cursor sent if none is in flight
    /// and it's newer than the last sent; and once released, its end, with its cursor or
    /// none; and the end's answer.
    pub fn poll<P: Clients + 'static>(
        &mut self,
        sessions: &Sessions<P>,
        session: &SessionId,
    ) -> Vec<News> {
        let mut news = Vec::new();
        loop {
            match std::mem::replace(&mut self.phase, Phase::Over) {
                Phase::Beginning(running) => match running.try_take() {
                    Ok(response) => match response.result {
                        Ok(_) => {
                            self.phase = Phase::Open { moving: None };
                            continue;
                        }
                        Err(problems) => news.push(News::Refused(problems)),
                    },
                    Err(running) => self.phase = Phase::Beginning(running),
                },
                Phase::Open { moving } => {
                    let moving = match moving {
                        Some((running, to)) => match running.try_take() {
                            Ok(response) => {
                                match response.result {
                                    Ok(frame) => self.took(&frame),
                                    // Cancelled: an Esc's end stopped it, and says nothing.
                                    Err(problems) if cancelled(&problems) => {}
                                    // A cursor the core refused (out of its reach): said,
                                    // and the drag goes on (section 7, `drag_move`).
                                    Err(problems) if names(&problems, "to") => {
                                        news.push(News::Refused(problems));
                                    }
                                    Err(problems) => {
                                        // The drag ended with it (a timeout, a worker's
                                        // failure, a new revision): nothing more to send.
                                        news.push(News::Refused(problems));
                                        break;
                                    }
                                }
                                None
                            }
                            Err(running) => Some((running, to)),
                        },
                        None => None,
                    };
                    if let Some(release) = self.released {
                        let to = match release {
                            Release::At(at) => Some(at),
                            Release::Cancel => None,
                        };
                        self.phase = Phase::Ending(sessions.drag_end(session, to));
                        news.push(News::EndSent);
                        continue;
                    }
                    let moving = match (moving, self.cursor) {
                        (None, Some(to)) if self.sent.last() != Some(&to) => {
                            self.moves_sent += 1;
                            self.sent.push(to);
                            Some((sessions.drag_move(session, to), to))
                        }
                        (moving, _) => moving,
                    };
                    self.phase = Phase::Open { moving };
                }
                Phase::Ending(running) => match running.try_take() {
                    Ok(response) => {
                        let made = response.session.as_ref().map(|s| s.revision);
                        let changed = response.result.as_ref().is_ok_and(|ended| ended.changed);
                        self.phase = match made {
                            Some(made) if changed => Phase::Ended { made },
                            _ => Phase::Over,
                        };
                        news.push(News::Ended(response));
                    }
                    Err(running) => self.phase = Phase::Ending(running),
                },
                phase => self.phase = phase,
            }
            break;
        }
        news
    }

    /// A move's frame: a solved one's points and radii set in what's shown, from the begin
    /// state; any other keeps the last.
    fn took(&mut self, frame: &DragFrame) {
        if frame.frame != FrameEnd::Solved {
            return;
        }
        let mut shown = self.begin.clone();
        for (id, [x, y]) in &frame.points {
            if let (Some(point), Some(x), Some(y)) = (
                shown.members.points.iter_mut().find(|p| &p.id == id),
                Coordinate::new(*x),
                Coordinate::new(*y),
            ) {
                point.x = x;
                point.y = y;
            }
        }
        for (id, r) in &frame.radii {
            if let (Some(circle), Some(r)) = (
                shown.members.circles.iter_mut().find(|c| &c.id == id),
                Coordinate::new(*r),
            ) {
                circle.radius = r;
            }
        }
        self.shown = Some(shown);
        self.frames += 1;
    }
}

/// Whether `problems`' first error names the parameter `parameter`.
fn names(problems: &Problems, parameter: &str) -> bool {
    problems
        .errors()
        .first()
        .is_some_and(|error| error.parameter() == Some(parameter))
}

/// Whether `problems` say the command was cancelled.
fn cancelled(problems: &Problems) -> bool {
    problems
        .errors()
        .first()
        .is_some_and(|error| error.code() == silico_core::ErrorCode::Cancelled)
}
