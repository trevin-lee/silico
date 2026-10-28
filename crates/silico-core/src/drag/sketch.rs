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
//! - **The end, settled.** The end's whole-sketch solve is solved again as a load solves a
//!   file (M3.4), so the sketch it leaves is one its own file gives back: no drift at the
//!   next rebuild. The edit is a function of the begin state and the final cursor alone.

use std::sync::Arc;

use silico_protocol::{DragBegin, PointOf, PointValues, SolveStatus, Value};

use super::{Begun, DragSession, Kept, MoveOutcome, MoveTicket, Notify};
use crate::document::{Checked, ElementId, FeatureId, Operation, Sketch};
use crate::recompute::SketchSolvers;
use crate::sketch::{SketchSolution, SketchSolver, Translation};
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
}

/// A drag begun: the drag, the sketch's degrees of freedom left, and the points of the
/// part it moves, by ID (`moves`, a diagnostic).
pub struct Opened {
    pub drag: SketchDrag,
    pub dofs: i32,
    pub moves: Vec<ElementId>,
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
/// and each free radius, by ID; otherwise none, and the client keeps its last frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub to: [f64; 2],
    pub kind: FrameKind,
    pub points: Vec<(ElementId, [f64; 2])>,
    pub radii: Vec<(ElementId, f64)>,
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

/// Begins a drag of `point` in sketch `feature` of `document`, on a solver taken from
/// `solvers` and held until the drag ends, each change of its moves told to `notify`. The
/// begin state is `held`, the model's solve of the sketch at this revision, if the caller
/// has it; otherwise the sketch is solved on the drag's solver. Refused, as `invalid_input`:
/// a feature the document hasn't, or one that isn't a sketch (naming `sketch`); a point the
/// sketch hasn't, or a part with more unknowns than a drag takes (naming `point`). A sketch
/// whose constraints conflict is `over_constrained`, naming them. A solver that failed is
/// its error. Each names the sketch.
pub fn begin(
    solvers: &SketchSolvers,
    notify: Option<Notify>,
    document: &Checked,
    feature: &FeatureId,
    point: &ElementId,
    held: Option<&SketchSolution>,
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
    if !sketch.points.iter().any(|p| &p.id == point) {
        return Err(named(
            Error::new(
                ErrorCode::InvalidInput,
                format!("sketch {feature} has no point {point}"),
            )
            .with_parameter("point")
            .with_suggestion("name one of the sketch's points"),
        ));
    }
    let mut prepared: Option<(Translation, Reading)> = None;
    let begun = super::begin_with(solvers, notify, |solver| {
        let begin_state = match held {
            Some(solution) => solution.written(sketch),
            None => solved_or_drawn(document, feature, sketch, solver)?,
        };
        let translation = Translation::new(&begin_state)?;
        let geometry = translation.point_geometry(point).ok_or_else(|| {
            Error::new(
                ErrorCode::Internal,
                format!("the translation of sketch {feature} has no point {point}"),
            )
            .with_suggestion(crate::worker::SILICO_BUG)
        })?;
        let begin = DragBegin {
            system: translation.system.clone(),
            point: PointOf::Point(geometry),
        };
        let reading = reading(&translation, &begin);
        prepared = Some((translation, reading));
        Ok(begin)
    })
    .map_err(named)?;
    let (translation, reading) = prepared.ok_or_else(|| {
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
    let moves = reading.points.iter().map(|(id, _)| id.clone()).collect();
    Ok(Opened {
        drag: SketchDrag {
            session,
            reading: Arc::new(reading),
            solvers: solvers.clone(),
            feature: feature.clone(),
            sketch: sketch.clone(),
            translation,
        },
        dofs,
        moves,
    })
}

/// The sketch as solved on `solver`, as its file would hold it; or, where the solve fails
/// for the sketch (not the solver), as its file draws it.
fn solved_or_drawn(
    document: &Checked,
    feature: &FeatureId,
    sketch: &Sketch,
    solver: &mut dyn SketchSolver,
) -> Result<Sketch, Error> {
    solver.stopper()?;
    match crate::sketch::solve(document, feature, solver) {
        Ok(solution) => Ok(solution.written(sketch)),
        Err(error) if error.code().is_kernel_fault() => Err(error),
        Err(_) => Ok(sketch.clone()),
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

    /// Ends the drag. With a cursor `to`, the sketch moved there from the begin state and
    /// solved as a whole ([`DragSession::end`]), then settled as a load settles a solve
    /// (M3.4): its points where that leaves them, as its file holds them. The begin state
    /// is kept, and the sketch as its file held it, if the move or either solve fails for
    /// the sketch (why, in [`SketchEnded::why`]); with no cursor, always. Refused: as
    /// [`DragSession::end`] is; and a solver that failed settling (its error). Each names
    /// the sketch.
    pub fn end(self, to: Option<[f64; 2]>) -> Result<SketchEnded, Error> {
        let named = |e: Error| e.with_feature(self.feature.as_str());
        let ended = self.session.end(to).map_err(named)?;
        let kept_begin = |why: Option<Error>| SketchEnded {
            kept: Kept::Begin,
            sketch: self.sketch.clone(),
            changed: false,
            why,
        };
        let solved = match (ended.kept, ended.solved) {
            (Kept::Cursor, Some(solved)) => solved,
            _ => {
                let why = to.map(|_| {
                    named(
                        Error::new(
                            ErrorCode::SolveFailed,
                            "the sketch couldn't be moved to the drag's final cursor and solved there",
                        )
                        .with_suggestion("drag it again, nearer where it began"),
                    )
                });
                return Ok(kept_begin(why));
            }
        };
        // Solved again from where the closing solve left it, as a load is.
        let mut closing = self.translation.clone();
        closing.system.values = solved.values;
        let mut solver = self.solvers.take().map_err(named)?;
        let answer = solver
            .stopper()
            .and_then(|_| closing.solve(solver.as_mut()));
        self.solvers.give_back(solver);
        match answer.map_err(named)?.outcome {
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
            Err(why) => Ok(kept_begin(Some(named(why)))),
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
        let kind = match outcome {
            MoveOutcome::Solved { moved, .. } => match (moved.status, moved.values) {
                (SolveStatus::Success, Some(values)) => return Ok(self.solved(&values)),
                (SolveStatus::SolvedInvalid, _) => FrameKind::Invalid,
                _ => FrameKind::Failed,
            },
            MoveOutcome::Refused(_) => FrameKind::Failed,
            MoveOutcome::Dropped => FrameKind::Dropped,
            MoveOutcome::Cancelled => FrameKind::Cancelled,
            MoveOutcome::Failed(error) | MoveOutcome::Closed(error) => return Err(error),
        };
        Ok(Frame {
            to: self.to,
            kind,
            points: Vec::new(),
            radii: Vec::new(),
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
        }
    }
}
