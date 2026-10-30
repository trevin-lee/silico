//! A drag of a document's sketch point (docs/design/m5.0-app.md, `drag_begin`, `drag_move`,
//! `drag_end`; decision 9, pending the maintainer): [`super`]'s session in the sketch's
//! terms, below any command. A point by its ID, with shared points (M3.2's D2) a line's
//! end, an arc's start, end, or centre, a circle's centre, or a lone point:
//!
//! - **The begin state.** The revision's solve of the sketch: the model's, if the caller
//!   holds it, or else solved on the drag's own solver, never waiting on a rebuild. A
//!   sketch whose constraints conflict is refused, naming them; one whose solve fails
//!   otherwise begins where its file draws it, as FreeCAD's drag begins from its geometry
//!   (open item 15).
//! - **Frames by ID.** Each move answers the points of the part the drag moves, and the
//!   radii of its circles and arcs that are free, by ID; every other point stays where it
//!   began.
//! - **The end, settled.** The move to the final cursor is solved as a whole as a load
//!   solves a file (M3.4), so the sketch it leaves is one its own file gives back: no drift
//!   at the next rebuild. The edit is a function of the begin state and the final cursor
//!   alone.
//! - **Stopped.** The whole-sketch solves, the begin's and the end's, run under the drag's
//!   [`DragStop`], so a command's `cancel` stops them, as [`SketchDrag::cancel_move`] stops
//!   a move.

use std::sync::Arc;

use silico_protocol::{DragBegin, PointOf, PointValues, SolveStatus, Value};

pub use super::DragStop;
use super::{Begun, DragSession, Kept, MoveOutcome, MoveTicket, Notify, cancelled};
use crate::document::{Checked, ElementId, FeatureId, Operation, Sketch};
use crate::recompute::{FeatureOutcome, Model, SketchSolvers};
use crate::sketch::{SketchSolution, SketchSolver, Translation};
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// A drag of a sketch's point, open.
pub struct SketchDrag {
    session: DragSession,
    reading: Arc<Reading>,
    solvers: SketchSolvers,
    feature: FeatureId,
    /// The sketch as its file holds it: what an end that keeps the begin state leaves.
    sketch: Sketch,
    /// The begin state's translation: an end's solve is read by it.
    translation: Translation,
    /// Where the dragged point began, or where an edge's press landed: a release there is a
    /// click, and changes nothing.
    origin: [f64; 2],
    /// Why the begin state is where the file draws the sketch, if its solve failed.
    unsolved: Option<Error>,
    stop: DragStop,
}

/// A drag begun: the drag, the sketch's degrees of freedom left, the points of the part it
/// moves, by ID (`moves`, a diagnostic), and, if the sketch didn't solve and the drag began
/// where its file draws it, why (open item 15).
pub struct Opened {
    pub drag: SketchDrag,
    pub dofs: i32,
    pub moves: Vec<ElementId>,
    pub unsolved: Option<Error>,
}

/// The model's solve of a sketch, held by the caller: a drag begins from it only if it
/// solved the sketch the drag is of, as the revision holds it.
#[derive(Clone, Copy)]
pub struct Held<'a> {
    /// The sketch it solved.
    pub sketch: &'a Sketch,
    pub solution: &'a SketchSolution,
}

impl<'a> Held<'a> {
    /// `model`'s solve of sketch `feature`, if it solved it.
    pub fn of(model: &'a Model, feature: &FeatureId) -> Option<Self> {
        let sketch = model
            .document()
            .features
            .iter()
            .find(|f| &f.id == feature)
            .and_then(|f| match &f.operation {
                Operation::Sketch(sketch) => Some(sketch),
                _ => None,
            })?;
        match model.outcome(feature)? {
            FeatureOutcome::Sketch(solution) => Some(Held { sketch, solution }),
            _ => None,
        }
    }
}

/// How a frame ended (the design's `frame`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// The cursor solved for: the frame holds the part's points and free radii.
    Solved,
    /// The solver didn't reach the cursor, or refused the move: the drag goes on.
    Failed,
    /// It reached geometry that can't be built (M3.7's `SolvedInvalid`).
    Invalid,
    /// A newer move replaced it before it was solved.
    Dropped,
    /// Stopped while it solved: by a cancel, or the drag's end with no cursor.
    Cancelled,
}

/// A move's frame: the cursor, how it ended, and with `Solved`, every point of the part
/// and each free radius, by ID; otherwise none, and the client keeps its last frame. A move
/// the worker refused is `Failed` with the worker's reason (a solution out of reach, say).
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub to: [f64; 2],
    pub kind: FrameKind,
    pub points: Vec<(ElementId, [f64; 2])>,
    pub radii: Vec<(ElementId, f64)>,
    pub why: Option<Error>,
}

/// A move sent: its frame, when it has one.
pub struct FrameTicket {
    ticket: MoveTicket,
    to: [f64; 2],
    reading: Arc<Reading>,
}

/// How a drag ended: the state kept, the sketch it leaves (as its file holds it), whether
/// that differs from the sketch the file held, and, where a cursor was given and the begin
/// state kept, why.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchEnded {
    pub kept: Kept,
    pub sketch: Sketch,
    pub changed: bool,
    pub why: Option<Error>,
}

/// What a frame's values are read into: the part's points and its free radii, by ID.
struct Reading {
    points: Vec<(ElementId, PointValues)>,
    radii: Vec<(ElementId, Value)>,
}

/// Begins a drag, as [`begin_stoppable`] does, under a stop of its own
/// ([`SketchDrag::stop`] gives it).
pub fn begin(
    solvers: &SketchSolvers,
    notify: Option<Notify>,
    document: &Checked,
    feature: &FeatureId,
    point: &ElementId,
    held: Option<Held<'_>>,
) -> Result<Opened, Error> {
    begin_stoppable(
        solvers,
        notify,
        document,
        feature,
        point,
        held,
        &DragStop::new(),
    )
}

/// Begins a drag of `point` in sketch `feature` of `document`, on a solver taken from
/// `solvers` and held until the drag ends, each change of its moves told to `notify`, its
/// whole solves under `stop`. The begin state is `held`, the model's solve of the sketch,
/// if it solved the sketch as `document` holds it; otherwise the sketch is solved on the
/// drag's solver. Refused, as `invalid_input`: a feature the document hasn't, or one that
/// isn't a sketch (naming `sketch`); a point the sketch hasn't, or a part with more
/// unknowns than a drag takes (naming `point`). A sketch whose constraints conflict is
/// `over_constrained`, naming them. A solve stopped is `cancelled`; a solver that failed is
/// its error. Each names the sketch.
pub fn begin_stoppable(
    solvers: &SketchSolvers,
    notify: Option<Notify>,
    document: &Checked,
    feature: &FeatureId,
    point: &ElementId,
    held: Option<Held<'_>>,
    stop: &DragStop,
) -> Result<Opened, Error> {
    start(
        solvers,
        notify,
        document,
        feature,
        Grab::Point(point),
        held,
        stop,
    )
}

/// Begins a drag of curve `curve`'s edge in sketch `feature` of `document`, pressed at
/// `press` in the sketch's coordinates (docs/ports/sketcher.md, *Dragging an edge*): a line
/// moved whole, each cursor's offset from the press; a circle or an arc sized to pass through
/// the cursor, its centre pinned where it starts, more weakly. As
/// [`begin_stoppable`] begins a point's, refused alike, and a curve the sketch hasn't
/// refused naming `curve`.
pub fn begin_edge(
    solvers: &SketchSolvers,
    notify: Option<Notify>,
    document: &Checked,
    feature: &FeatureId,
    edge: EdgeGrab<'_>,
    held: Option<Held<'_>>,
    stop: &DragStop,
) -> Result<Opened, Error> {
    let EdgeGrab { curve, press } = edge;
    start(
        solvers,
        notify,
        document,
        feature,
        Grab::Edge { curve, press },
        held,
        stop,
    )
}

/// A curve's edge, pressed: the curve, by ID, and where the press landed, in the sketch's
/// coordinates.
#[derive(Debug, Clone, Copy)]
pub struct EdgeGrab<'a> {
    pub curve: &'a ElementId,
    pub press: [f64; 2],
}

/// A drag's begin, made on its solver: the begin state's translation, what its frames are
/// read into, where a release is a click, and why the sketch didn't
/// solve, if it didn't.
struct Prepared {
    translation: Translation,
    reading: Reading,
    origin: [f64; 2],
    unsolved: Option<Error>,
}

/// What a drag grabs: a point, or a curve's edge where the press landed.
#[derive(Clone, Copy)]
enum Grab<'a> {
    Point(&'a ElementId),
    Edge {
        curve: &'a ElementId,
        press: [f64; 2],
    },
}

fn start(
    solvers: &SketchSolvers,
    notify: Option<Notify>,
    document: &Checked,
    feature: &FeatureId,
    grab: Grab<'_>,
    held: Option<Held<'_>>,
    stop: &DragStop,
) -> Result<Opened, Error> {
    let named = |e: Error| e.with_feature(feature.as_str());
    let sketch = match document.feature(feature) {
        Ok(found) => match &found.operation {
            Operation::Sketch(sketch) => sketch,
            _ => {
                return Err(named(
                    Error::new(
                        ErrorCode::InvalidInput,
                        format!("feature {feature} isn't a sketch"),
                    )
                    .with_parameter("sketch")
                    .with_suggestion("name a sketch, whose points a drag moves"),
                ));
            }
        },
        Err(error) => return Err(error.with_parameter("sketch")),
    };
    let has = match grab {
        Grab::Point(point) => sketch.points.iter().any(|p| &p.id == point),
        Grab::Edge { curve, .. } => {
            sketch.lines.iter().any(|l| &l.id == curve)
                || sketch.circles.iter().any(|c| &c.id == curve)
                || sketch.arcs.iter().any(|a| &a.id == curve)
        }
    };
    if !has {
        let (what, id, parameter) = match grab {
            Grab::Point(point) => ("point", point, "point"),
            Grab::Edge { curve, .. } => ("curve", curve, "curve"),
        };
        return Err(named(
            Error::new(
                ErrorCode::InvalidInput,
                format!("sketch {feature} has no {what} {id}"),
            )
            .with_parameter(parameter)
            .with_suggestion(format!("name one of the sketch's {what}s")),
        ));
    }
    let mut prepared: Option<Prepared> = None;
    let begun = super::begin_under(solvers, notify, stop, |solver| {
        let (begin_state, unsolved) = match held.filter(|held| held.sketch == sketch) {
            Some(held) => (held.solution.written(sketch), None),
            None => solved_or_drawn(document, feature, sketch, solver, stop)?,
        };
        let translation = Translation::new(&begin_state)?;
        let missing = |what: &str, id: &ElementId| {
            Error::new(
                ErrorCode::Internal,
                format!("the translation of sketch {feature} has no {what} {id}"),
            )
            .with_suggestion(crate::worker::SILICO_BUG)
        };
        let value = |v: silico_protocol::Value| {
            translation
                .system
                .values
                .get(v.0 as usize)
                .copied()
                .unwrap_or(f64::NAN)
        };
        let (begin, origin) = match grab {
            Grab::Point(point) => {
                let geometry = translation
                    .point_geometry(point)
                    .ok_or_else(|| missing("point", point))?;
                let begin = DragBegin::point(translation.system.clone(), PointOf::Point(geometry));
                let origin = begin
                    .system
                    .point_values(PointOf::Point(geometry))
                    .map(|p| [value(p.x), value(p.y)])
                    .unwrap_or([f64::NAN; 2]);
                (begin, origin)
            }
            Grab::Edge { curve, press } => {
                let geometry = translation
                    .curve_geometry(curve)
                    .ok_or_else(|| missing("curve", curve))?;
                let begin = DragBegin {
                    system: translation.system.clone(),
                    dragged: silico_protocol::Dragged::Edge {
                        curve: geometry,
                        press,
                    },
                };
                (begin, press)
            }
        };
        let reading = reading(&translation, &begin);
        prepared = Some(Prepared {
            translation,
            reading,
            origin,
            unsolved,
        });
        Ok(begin)
    })
    .map_err(named)?;
    let Prepared {
        translation,
        reading,
        origin,
        unsolved,
    } = prepared.ok_or_else(|| {
        Error::new(ErrorCode::Internal, "a drag began with no begin state")
            .with_suggestion(crate::worker::SILICO_BUG)
    })?;
    let (session, dofs) = match begun {
        Begun::Open { session, dofs } => (session, dofs),
        Begun::Conflicting(tags) => {
            let ids: Vec<String> = tags
                .iter()
                .filter_map(|&t| translation.constraint_id(t))
                .map(|id| id.as_str().to_owned())
                .collect();
            let n = ids.len();
            return Err(named(
                Error::new(
                    ErrorCode::OverConstrained,
                    format!("the sketch's constraints conflict: these {n} can't all hold"),
                )
                .with_elements(ids)
                .with_cause(
                    "a drag starts only from a sketch whose constraints agree, as FreeCAD refuses to drag one with conflicts",
                )
                .with_suggestion("remove or change one of the constraints named"),
            ));
        }
    };
    // A stop that came after the begin's solve (or with no solve to stop) is the begin's
    // still: the drag isn't left open for every release to be cancelled.
    if lock(&stop.0).stopped {
        let _ = session.end(None);
        return Err(named(cancelled()));
    }
    let moves = reading.points.iter().map(|(id, _)| id.clone()).collect();
    Ok(Opened {
        drag: SketchDrag {
            session,
            reading: Arc::new(reading),
            solvers: solvers.clone(),
            feature: feature.clone(),
            sketch: sketch.clone(),
            translation,
            origin,
            unsolved: unsolved.clone(),
            stop: stop.clone(),
        },
        dofs,
        moves,
        unsolved,
    })
}

/// The sketch as solved on `solver` under `stop`, as its file would hold it; or, where the
/// solve fails for the sketch (not the solver, nor a stop), as its file draws it, and why.
/// A drag begins from it, and so does an extend (`command::edit::sketch_extend`).
pub(crate) fn solved_or_drawn(
    document: &Checked,
    feature: &FeatureId,
    sketch: &Sketch,
    solver: &mut dyn SketchSolver,
    stop: &DragStop,
) -> Result<(Sketch, Option<Error>), Error> {
    match stop.run(solver, |solver| {
        crate::sketch::solve(document, feature, solver)
    }) {
        Ok(solution) => Ok((solution.written(sketch), None)),
        Err(error) if error.code().is_kernel_fault() || error.code() == ErrorCode::Cancelled => {
            Err(error)
        }
        Err(why) => Ok((sketch.clone(), Some(why))),
    }
}

/// The points of the part `begin` moves, and its free radii, by ID.
fn reading(translation: &Translation, begin: &DragBegin) -> Reading {
    let part = begin.part();
    let free = |v: Value| part.unknowns.binary_search(&v).is_ok();
    Reading {
        points: translation
            .point_values()
            .filter(|(_, p)| free(p.x) || free(p.y))
            .map(|(id, p)| (id.clone(), p))
            .collect(),
        radii: translation
            .radius_values()
            .into_iter()
            .filter(|&(_, r)| free(r))
            .map(|(id, r)| (id.clone(), r))
            .collect(),
    }
}

impl SketchDrag {
    /// Sends a move to `to`, as [`DragSession::move_to`] does.
    pub fn move_to(&self, to: [f64; 2]) -> Result<FrameTicket, Error> {
        let ticket = self.session.move_to(to)?;
        Ok(FrameTicket {
            ticket,
            to,
            reading: Arc::clone(&self.reading),
        })
    }

    /// Stops the move solving, as [`DragSession::cancel_move`] does.
    pub fn cancel_move(&self) -> bool {
        self.session.cancel_move()
    }

    /// Whether the drag is still open.
    pub fn is_open(&self) -> bool {
        self.session.is_open()
    }

    /// Why the drag is no longer open, if it isn't.
    pub fn closed(&self) -> Option<Error> {
        self.session.closed()
    }

    /// The stop its whole solves run under: [`DragStop::stop`] stops the end's.
    pub fn stop(&self) -> DragStop {
        self.stop.clone()
    }

    /// Ends the drag. With a cursor `to`, the sketch moved there from the begin state
    /// ([`DragSession::release`]), then solved as a whole as a load solves a file (M3.4):
    /// its points where that leaves them, as its file holds them. The begin state is kept,
    /// and the sketch as its file held it, if the move or the solve fails for the sketch
    /// (why, in [`SketchEnded::why`]: for a sketch that didn't solve when the drag began,
    /// that failure); with no cursor, always; and with the cursor where the dragged point
    /// began, a click, which moves nothing (a solve from there could still move the sketch
    /// by its rounding). Refused: as [`DragSession::release`] is; a solve stopped
    /// (`cancelled`); and a solver that failed (its error). Each names the sketch.
    pub fn end(self, to: Option<[f64; 2]>) -> Result<SketchEnded, Error> {
        let _span =
            tracing::info_span!("drag_sketch_end", feature = self.feature.as_str()).entered();
        let named = |e: Error| e.with_feature(self.feature.as_str());
        let to = to.filter(|&to| to != self.origin);
        let ended = self.session.release(to).map_err(named)?;
        let kept_begin = |why: Option<Error>| SketchEnded {
            kept: Kept::Begin,
            sketch: self.sketch.clone(),
            changed: false,
            why: why.map(named),
        };
        // A sketch that didn't solve when the drag began fails as it did then: that's why.
        let unsettled = |why: Error| kept_begin(Some(self.unsolved.clone().unwrap_or(why)));
        let values = match (ended.kept, ended.values) {
            (Kept::Cursor, Some(values)) => values,
            _ => return Ok(kept_begin(ended.why)),
        };
        // Solved as a whole from where the move left it, then loaded until it settles.
        let mut closing = self.translation.clone();
        closing.system.values = values;
        let mut solver = self.solvers.take().map_err(named)?;
        let answer = self
            .stop
            .run(solver.as_mut(), |solver| closing.solve(solver));
        self.solvers.give_back(solver);
        let answer = match answer {
            Ok(answer) => answer,
            Err(error)
                if error.code().is_kernel_fault() || error.code() == ErrorCode::Cancelled =>
            {
                return Err(named(error));
            }
            // Refused by the worker for the sketch's values, not failed: the begin kept.
            Err(why) => return Ok(unsettled(why)),
        };
        match answer.into_outcome() {
            Ok(solution) => {
                let sketch = solution.written(&self.sketch);
                let changed = sketch != self.sketch;
                Ok(SketchEnded {
                    kept: Kept::Cursor,
                    sketch,
                    changed,
                    why: None,
                })
            }
            Err(why) => Ok(unsettled(why)),
        }
    }
}

impl FrameTicket {
    /// Waits for the move's frame. A drag that ended, or whose worker failed, before or
    /// while it solved, is the error.
    pub fn wait(&self) -> Result<Frame, Error> {
        self.frame(self.ticket.wait())
    }

    /// The move's frame, if it has one yet.
    pub fn try_take(&self) -> Option<Result<Frame, Error>> {
        self.ticket.try_take().map(|outcome| self.frame(outcome))
    }

    fn frame(&self, outcome: MoveOutcome) -> Result<Frame, Error> {
        let (kind, why) = match outcome {
            MoveOutcome::Solved { moved, .. } => match (moved.status, moved.values) {
                (SolveStatus::Success, Some(values)) => return Ok(self.solved(&values)),
                (SolveStatus::SolvedInvalid, _) => (FrameKind::Invalid, None),
                _ => (FrameKind::Failed, None),
            },
            MoveOutcome::Refused(why) => (FrameKind::Failed, Some(why)),
            MoveOutcome::Dropped => (FrameKind::Dropped, None),
            MoveOutcome::Cancelled => (FrameKind::Cancelled, None),
            MoveOutcome::Failed(error) | MoveOutcome::Closed(error) => return Err(error),
        };
        Ok(Frame {
            to: self.to,
            kind,
            points: Vec::new(),
            radii: Vec::new(),
            why,
        })
    }

    /// A solved frame, read from `values` (which the drag checked are the part's).
    fn solved(&self, values: &[f64]) -> Frame {
        let at = |v: Value| values.get(v.0 as usize).copied().unwrap_or(f64::NAN);
        Frame {
            to: self.to,
            kind: FrameKind::Solved,
            points: self
                .reading
                .points
                .iter()
                .map(|(id, p)| (id.clone(), [at(p.x), at(p.y)]))
                .collect(),
            radii: self
                .reading
                .radii
                .iter()
                .map(|(id, r)| (id.clone(), at(*r)))
                .collect(),
            why: None,
        }
    }
}
