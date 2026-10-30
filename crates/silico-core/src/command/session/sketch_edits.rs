//! The sketch edits (docs/design/m5.0-app.md, section 7, *The sketch edits*), each an edit
//! of the session's document made against a revision, as `set_field` is ([`Session::edit`]):
//! held inside a transaction, on its undo stack; committed as its own transaction outside
//! one.
//!
//! - **What adds or removes what the person wrote**, `sketch_add` and `sketch_remove`, never
//!   solves the sketch, nor is refused for what its solve finds (SD7): once the gate takes
//!   the edit, it's applied, and the rebuild's verdict, a conflict, a redundancy, or no
//!   solution, is the sketch's state, which `check` reports and its features fail by.
//! - **What writes geometry**, `sketch_split`, `sketch_trim`, `sketch_unjoin`,
//!   `sketch_merge`, and `sketch_extend`, writes exactly what its request names into the sketch as stored, and
//!   the sketch's ordinary solve from that written sketch (M3.4, FreeCAD's order) is the
//!   edit's solve (Fable, 2026-10-29): run here, on one of the sessions' sketch solvers,
//!   before the edit is held or committed, stopped by the command's `cancel`. An edit whose
//!   solve is worse than the sketch's before it refuses as data, the session as it was (SD7,
//!   refined: a conflict the sketch had already isn't the edit's), by one rule for all of
//!   them, as FreeCAD main's Coincident command rolls one back (`shouldRejectCoincidentSolve`,
//!   `cb2780e2fa`; sketch-edit decision 10): a result the file can't hold, a solve that ranks
//!   worse (a conflict, then over-constrained, then no solution), or one that flags a
//!   constraint redundant or in conflict that the solve before didn't. No edit tries a
//!   second seed, retries, snaps, or solves again: the rebuild then solves the same text the
//!   same way.
//!
//! [`Session::edit`]: super::Session

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, Weak};

use super::cancel::{Interrupt, Ticket, Why};
use super::{
    ExtendBy, NewMembers, Problems, Running, SessionCommand, SessionId, Sessions, SketchCut,
    SketchEdited, not_open, reclaim,
};
use crate::command::Gated;
use crate::command::Revision;
use crate::command::edit::{self, Candidate, Cut, FeatureIds, SketchChanges};
use crate::document::sketch_edit::NewIds;
use crate::document::{Checked, ElementId, FeatureId, Operation, Reference};
use crate::drag::DragStop;
use crate::extend::End;
use crate::kernel::Clients;
use crate::recompute::SketchSolvers;
use crate::sketch::{Diagnosis, SketchSolution, Translation};
use crate::worker::lock;
use crate::{Error, ErrorCode};

impl<P: Clients + 'static> Sessions<P> {
    /// Adds `members` to `session`'s sketch `sketch` (the design's `sketch_add`), as an edit
    /// made against `revision`: each list as the document writes a sketch's, each member
    /// with its own ID, the batch checked whole by the gate before any of it is added. Not
    /// refused for a conflict or a redundancy it makes (SD7): the sketch's solve then reads
    /// so. An empty batch changes nothing: no new revision, nothing written. Refused whole,
    /// the session as it was, as every edit is ([`set_field`](Self::set_field)), for a
    /// feature that isn't a sketch (naming `sketch`, the document's sketches as
    /// candidates), and with every problem the reader or the gate finds in the members, each
    /// at its ID path (`circles.cb000003.radius`).
    pub fn sketch_add(
        &self,
        session: &SessionId,
        revision: Revision,
        sketch: FeatureId,
        members: NewMembers,
    ) -> Running<SketchEdited> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(
            session,
            SessionCommand::SketchAdd,
            move |session, stop, model| {
                let _span = tracing::info_span!("sketch_add", sketch = sketch.as_str()).entered();
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| edit::sketch_add(base.checked()?.document(), &sketch, members),
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                Ok(sketch_edited(sketch, edited))
            },
        )
    }

    /// Removes `members` from `session`'s sketch `sketch` (the design's `sketch_remove`;
    /// sketch-edit decision 7), as an edit made against `revision`: a constraint; a curve,
    /// with each of its points no curve uses after the edit, and every constraint naming
    /// what goes; a point only if no curve uses it after the edit. The set is judged whole,
    /// duplicates taken once. Not refused for what the sketch's solve then finds (SD7). An
    /// empty list changes nothing. Refused as [`sketch_add`](Self::sketch_add) is, and for a
    /// member the sketch hasn't, a fixed reference (`origin`, `x_axis`, `y_axis`), and a
    /// point a curve still uses, each at its place in `members`.
    pub fn sketch_remove(
        &self,
        session: &SessionId,
        revision: Revision,
        sketch: FeatureId,
        members: Vec<Reference>,
    ) -> Running<SketchEdited> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(
            session,
            SessionCommand::SketchRemove,
            move |session, stop, model| {
                let _span = tracing::info_span!(
                    "sketch_remove",
                    sketch = sketch.as_str(),
                    members = members.len()
                )
                .entered();
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| edit::sketch_remove(base.checked()?.document(), &sketch, &members),
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                Ok(sketch_edited(sketch, edited))
            },
        )
    }

    /// Splits `curve` of `session`'s sketch `sketch` at `at` (the design's `sketch_split`;
    /// sketch-edit 1 to 4, §3's table), as an edit made against `revision`: a line or an arc
    /// at one position, into two pieces sharing a new point at its projection; a circle at
    /// two, into two arcs. The start-side piece keeps the curve's ID. The new members' IDs
    /// are `ids`, in `added`'s order, or, if `None`, the core's. The sketch's solve then
    /// judges it: refused, the session as it was, where it's worse than the sketch's solve
    /// before the split (the module's rule: a result the file can't hold, a solve that ranks
    /// worse, or one flagging a constraint the solve before didn't), as for what the split
    /// itself refuses (a position
    /// off the curve, at or near an end, the wrong number of positions or of `ids`) and a
    /// feature that isn't a sketch. Its result lists the features storing a name made from
    /// the curve or its points (`references`).
    pub fn sketch_split(
        &self,
        session: &SessionId,
        revision: Revision,
        sketch: FeatureId,
        curve: ElementId,
        at: Vec<[f64; 2]>,
        ids: Option<Vec<ElementId>>,
    ) -> Running<SketchCut> {
        let registry = Arc::downgrade(&self.registry);
        let solvers = self.registry.sketches.clone();
        let drawn = Arc::clone(&self.registry.feature_ids);
        self.queued(
            session,
            SessionCommand::SketchSplit,
            move |session, stop, model| {
                let _span = tracing::info_span!(
                    "sketch_split",
                    sketch = sketch.as_str(),
                    curve = curve.as_str()
                )
                .entered();
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| {
                        let checked = base.checked()?;
                        let made = with_ids(ids.as_deref(), &drawn, |ids| {
                            edit::sketch_split(checked.document(), &sketch, &curve, &at, ids)
                        })?;
                        judged(&solvers, stop, base.id, checked, &sketch, SPLIT, made)
                    },
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                Ok(sketch_cut(sketch, edited))
            },
        )
    }

    /// Trims `curve` of `session`'s sketch `sketch` back to its neighbours either side of
    /// `pick` (the design's `sketch_trim`; sketch-edit 2, 5, 6, 8, and §4), as an edit made
    /// against `revision`: the piece holding the pick's projection is removed, or the curve
    /// whole where nothing cuts it there. Its new members' IDs, its solve's judgment, and its
    /// result as [`sketch_split`](Self::sketch_split)'s; refused, too, for decision 2's cases,
    /// naming the constraint (a `symmetric` about the trimmed line, an arc's edge tangency no
    /// piece touches), and for a solve that flips a tangency it made an end tangency
    /// (sketch-edit 8).
    pub fn sketch_trim(
        &self,
        session: &SessionId,
        revision: Revision,
        sketch: FeatureId,
        curve: ElementId,
        pick: [f64; 2],
        ids: Option<Vec<ElementId>>,
    ) -> Running<SketchCut> {
        let registry = Arc::downgrade(&self.registry);
        let solvers = self.registry.sketches.clone();
        let drawn = Arc::clone(&self.registry.feature_ids);
        self.queued(
            session,
            SessionCommand::SketchTrim,
            move |session, stop, model| {
                let _span = tracing::info_span!(
                    "sketch_trim",
                    sketch = sketch.as_str(),
                    curve = curve.as_str()
                )
                .entered();
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| {
                        let checked = base.checked()?;
                        let made = with_ids(ids.as_deref(), &drawn, |ids| {
                            edit::sketch_trim(checked.document(), &sketch, &curve, pick, ids)
                        })?;
                        judged(&solvers, stop, base.id, checked, &sketch, TRIM, made)
                    },
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                Ok(sketch_cut(sketch, edited))
            },
        )
    }

    /// Pulls `curve`'s use of the shared `point` of `session`'s sketch `sketch` apart (the
    /// design's `sketch_unjoin`; sketch-edit 9), as an edit made against `revision`: `curve`
    /// gets a new point at the same coordinates, its ID `ids`' one, or, if `None`, the
    /// core's. Judged by its solve as [`sketch_split`](Self::sketch_split) is; refused, too,
    /// for a curve that doesn't use the point and a point no other curve shares.
    pub fn sketch_unjoin(
        &self,
        session: &SessionId,
        revision: Revision,
        sketch: FeatureId,
        point: ElementId,
        curve: ElementId,
        ids: Option<Vec<ElementId>>,
    ) -> Running<SketchEdited> {
        let registry = Arc::downgrade(&self.registry);
        let solvers = self.registry.sketches.clone();
        let drawn = Arc::clone(&self.registry.feature_ids);
        self.queued(
            session,
            SessionCommand::SketchUnjoin,
            move |session, stop, model| {
                let _span = tracing::info_span!(
                    "sketch_unjoin",
                    sketch = sketch.as_str(),
                    point = point.as_str(),
                    curve = curve.as_str()
                )
                .entered();
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| {
                        let checked = base.checked()?;
                        let made = with_ids(ids.as_deref(), &drawn, |ids| {
                            edit::sketch_unjoin(checked.document(), &sketch, &point, &curve, ids)
                        })?;
                        judged(&solvers, stop, base.id, checked, &sketch, UNJOIN, made)
                    },
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                Ok(sketch_edited(sketch, edited))
            },
        )
    }

    /// Makes `points` of `session`'s sketch `sketch` one (the design's `sketch_merge`;
    /// sketch-edit 10), as an edit made against `revision`: the first named survives at its
    /// own coordinates, the others' curves and constraints re-pointed to it; an edge
    /// `tangent` left at a shared end becomes a `tangent_at`, and an `on` it makes hold an
    /// end on its own curve goes as implied (open items 13 and 13b). It makes no member, so
    /// it takes no `ids`. Judged by its solve as [`sketch_split`](Self::sketch_split) is,
    /// a solve that fails suggesting the request's lever (name first the point the
    /// constraints hold); and refused for what the merge itself refuses (points already one, a
    /// curve that would use one point twice, a constraint that would name one twice).
    pub fn sketch_merge(
        &self,
        session: &SessionId,
        revision: Revision,
        sketch: FeatureId,
        points: Vec<ElementId>,
    ) -> Running<SketchEdited> {
        let registry = Arc::downgrade(&self.registry);
        let solvers = self.registry.sketches.clone();
        self.queued(
            session,
            SessionCommand::SketchMerge,
            move |session, stop, model| {
                let _span = tracing::info_span!(
                    "sketch_merge",
                    sketch = sketch.as_str(),
                    points = points.len()
                )
                .entered();
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| {
                        let checked = base.checked()?;
                        let made = edit::sketch_merge(checked.document(), &sketch, &points)?;
                        judged(&solvers, stop, base.id, checked, &sketch, MERGE, made)
                    },
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                Ok(sketch_edited(sketch, edited))
            },
        )
    }

    /// Extends `curve`'s `end` of `session`'s sketch `sketch` `by` a length (a line's), an
    /// angle (an arc's), to a point on its carrier, or to the next curve it meets (the
    /// design's `sketch_extend`; sketch-edit 11, open item 12c), as an edit made against
    /// `revision`: from the revision's sketch as solved and written back, as a drag begins,
    /// a line's end through one move of a drag, its constraints winning, the end then written
    /// where the request named; an arc's written turned. On one of the sessions' sketch
    /// solvers, the move and the solves stopped by the command's `cancel`. Judged by its
    /// solve as [`sketch_split`](Self::sketch_split) is; refused, too, for what the extend
    /// refuses (a curve that isn't a line or an arc, the wrong kind of increment, one of 0, a
    /// result too short, flipped, or a whole turn, a point off the carrier, an end its
    /// constraints hold, a frame that can't be built) and a next curve there's none of.
    pub fn sketch_extend(
        &self,
        session: &SessionId,
        revision: Revision,
        sketch: FeatureId,
        curve: ElementId,
        end: End,
        by: ExtendBy,
    ) -> Running<SketchEdited> {
        let registry = Arc::downgrade(&self.registry);
        let solvers = self.registry.sketches.clone();
        self.queued(
            session,
            SessionCommand::SketchExtend,
            move |session, stop, model| {
                let _span = tracing::info_span!(
                    "sketch_extend",
                    sketch = sketch.as_str(),
                    curve = curve.as_str()
                )
                .entered();
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| {
                        let checked = base.checked()?;
                        let halt = Arc::new(SolveStop(DragStop::new()));
                        stop.interrupt_with(Arc::downgrade(&halt) as Weak<dyn Interrupt>);
                        let mut solver = solvers.take().map_err(Problems::from)?;
                        let made = edit::sketch_extend(
                            checked,
                            &sketch,
                            &curve,
                            end,
                            by,
                            solver.as_mut(),
                            &halt.0,
                        );
                        solvers.give_back(solver);
                        // Its session's close, which stops the drag, is the session's: never
                        // the drag's words. A cancel answers `cancelled` whatever the extend
                        // returns (`Ticket::answer`).
                        if stop.closed() {
                            return Err(Problems::from(not_open(base.id)));
                        }
                        judged(&solvers, stop, base.id, checked, &sketch, EXTEND, made?)
                    },
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                Ok(sketch_edited(sketch, edited))
            },
        )
    }
}

/// A sketch edit's result: what it changed, as the edit found it (a commit's save changes no
/// member's ID, only a solve's coordinates), whether the document changed, and the commit.
fn sketch_edited(sketch: FeatureId, edited: super::edits::Edited<SketchChanges>) -> SketchEdited {
    let SketchChanges {
        added,
        modified,
        removed,
    } = edited.made;
    SketchEdited {
        sketch,
        added,
        modified,
        removed,
        changed: edited.changed,
        commit: edited.commit,
    }
}

/// A split's or a trim's result, as [`sketch_edited`] gives an edit's, with its pieces and
/// the features storing a name its curve reaches.
fn sketch_cut(sketch: FeatureId, edited: super::edits::Edited<Cut>) -> SketchCut {
    let Cut {
        changes:
            SketchChanges {
                added,
                modified,
                removed,
            },
        pieces,
        references,
    } = edited.made;
    SketchCut {
        sketch,
        pieces,
        added,
        modified,
        removed,
        references,
        changed: edited.changed,
        commit: edited.commit,
    }
}

/// `make` given where an edit's new IDs come from: `given`, the request's, or, for `None`,
/// the sessions' generator, `drawn` (decision 20), each ID one the sketch doesn't hold.
fn with_ids<T>(
    given: Option<&[ElementId]>,
    drawn: &Mutex<FeatureIds>,
    make: impl FnOnce(NewIds<'_>) -> T,
) -> T {
    match given {
        Some(ids) => make(NewIds::Given(ids)),
        None => {
            let mut draw = |taken: &dyn Fn(&ElementId) -> bool| lock(drawn).next_member(taken);
            make(NewIds::Drawn(&mut draw))
        }
    }
}

/// An edit that writes geometry, as its judgment says it: its name, as a refusal names it
/// ("after the split, ..."), and what to suggest where its solve fails, or none for the
/// solve's own advice.
#[derive(Clone, Copy)]
pub(super) struct Edit {
    name: &'static str,
    lever: Option<&'static str>,
}

const SPLIT: Edit = Edit {
    name: "split",
    lever: None,
};
const TRIM: Edit = Edit {
    name: "trim",
    lever: None,
};
const UNJOIN: Edit = Edit {
    name: "unjoin",
    lever: None,
};
const EXTEND: Edit = Edit {
    name: "extend",
    lever: None,
};
/// A merge's solve starts from the kept point's place, by the request's order (Fable,
/// 2026-10-29): the lever is the request's.
const MERGE: Edit = Edit {
    name: "merge",
    lever: Some(
        "name first the point the constraints hold, or move the points together, then merge",
    ),
};

/// An edit that writes geometry, `made`, judged by its sketch's solve (SD7, refined: a
/// conflict the sketch had already isn't the edit's): its candidate's sketch `sketch` solved,
/// and `made` given back unless that solve is worse than the solve of `before`, the document
/// the edit was made to ([`worse`]): refused then, as `edit`'s. An edit that changes nothing
/// isn't solved, nor is `before` where the edit's solve is clean. Its solves are stopped by
/// the command `stop`, of the session `session` ([`solved`]).
fn judged<T>(
    solvers: &SketchSolvers,
    stop: &Ticket,
    session: SessionId,
    before: &Checked,
    sketch: &FeatureId,
    edit: Edit,
    made: (T, Option<Candidate>),
) -> Result<(T, Option<Candidate>), Problems> {
    let Some(candidate) = &made.1 else {
        return Ok(made);
    };
    let after = solved(solvers, stop, session, checked_of(candidate)?, sketch)?;
    // A clean solve is never worse, and one the file can't hold always is: neither needs the
    // solve before.
    let refusal = match severity(&after) {
        0 => None,
        UNBUILDABLE => worse(None, &after, edit),
        _ => worse(
            Some(&solved(solvers, stop, session, before, sketch)?),
            &after,
            edit,
        ),
    };
    match refusal {
        Some(error) => Err(Problems::from(error.with_feature(sketch.as_str()))),
        None => Ok(made),
    }
}

/// The document a candidate the gate took holds.
fn checked_of(candidate: &Candidate) -> Result<&Checked, Problems> {
    match candidate.gated() {
        Gated::Checked(checked) => Ok(checked),
        Gated::Unread(_) | Gated::Refused(..) => Err(Problems::from(
            Error::new(
                ErrorCode::Internal,
                "a sketch edit's candidate wasn't one the gate took",
            )
            .with_suggestion(crate::worker::SILICO_BUG),
        )),
    }
}

/// What stops an edit's solve: its command's `cancel`, or its session's close, stops the
/// solve on the worker, which the edit then answers as `cancelled` or `not_open`
/// ([`solved`]).
struct SolveStop(DragStop);

impl Interrupt for SolveStop {
    fn interrupt(&self, _why: Why) {
        self.0.stop();
    }
}

/// A sketch's solve, as an edit's judgment reads it: its outcome, the solution or the solve's
/// refusal of the sketch, and the diagnosis PlaneGCS gave with it, which every answer the
/// worker gave has, refused or not (none for a sketch that couldn't be sent).
struct Judged {
    outcome: Result<SketchSolution, Error>,
    diagnosis: Option<Diagnosis>,
}

/// The sketch `sketch` of `document` solved, as a rebuild solves it (M3.4: from the file's
/// coordinates, FreeCAD's order), on a solver of `solvers`, given back after: the solution,
/// or the solve's refusal of the sketch (a conflict, no solution, a result the file can't
/// hold, or a sketch that can't be sent), and its diagnosis. Stopped by the command `stop`:
/// refused `not_open` if `session` closed, as a session that isn't open is, since no client
/// stopped it; a cancel answers `cancelled` whatever the edit returns, as every command
/// cancelled before it settles does ([`Ticket::answer`]). A solver that couldn't be had, or
/// failed in a way that isn't the sketch's (a crash, a hang, a reply it can't be trusted
/// for), is that fault, refusing the edit.
fn solved(
    solvers: &SketchSolvers,
    stop: &Ticket,
    session: SessionId,
    document: &Checked,
    sketch: &FeatureId,
) -> Result<Judged, Problems> {
    let _span = tracing::info_span!("edit_solve", sketch = sketch.as_str()).entered();
    let named = |e: Error| e.with_feature(sketch.as_str());
    let found = document.feature(sketch).map_err(Problems::from)?;
    let Operation::Sketch(drawn) = &found.operation else {
        return Err(Problems::from(named(Error::new(
            ErrorCode::Internal,
            format!("an edit's sketch {sketch} isn't a sketch"),
        ))));
    };
    let translation = match Translation::new(drawn) {
        Ok(translation) => translation,
        Err(refused) => {
            return Ok(Judged {
                outcome: Err(named(refused)),
                diagnosis: None,
            });
        }
    };
    let halt = Arc::new(SolveStop(DragStop::new()));
    stop.interrupt_with(Arc::downgrade(&halt) as Weak<dyn Interrupt>);
    let mut solver = solvers.take().map_err(Problems::from)?;
    let answer = halt
        .0
        .run(solver.as_mut(), |solver| translation.solve(solver));
    solvers.give_back(solver);
    // Its session's close stopped it, or came as it ended: either way the session is gone,
    // and the drag's words for a stopped solve never reach the client.
    let answer = match answer {
        _ if stop.closed() => return Err(Problems::from(not_open(session))),
        Err(error) => return Err(Problems::from(named(error))),
        Ok(answer) => answer,
    };
    let diagnosis = answer.diagnosed().ok().map(|d| d.diagnosis().clone());
    match answer.into_outcome() {
        Err(error) if error.code().is_kernel_fault() => Err(Problems::from(named(error))),
        outcome => Ok(Judged {
            outcome: outcome.map_err(named),
            diagnosis,
        }),
    }
}

/// How bad a solve is, as FreeCAD main's coincident rollback ranks it
/// (`getCoincidentSolveStatusSeverity`, `CommandConstraints.cpp`, with
/// `getCoincidentSolveStatus`, which reads a negative DoF count first): solved, 0; solved with
/// redundant constraints, 1; constraints in conflict, 2, or over-constrained, 3, where the
/// diagnosis leaves fewer than no degrees of freedom; no solution, 4; anything else (a result
/// the file can't hold, a sketch that can't be sent), [`UNBUILDABLE`].
fn severity(judged: &Judged) -> u8 {
    match &judged.outcome {
        Ok(solution) if solution.redundant.is_empty() => 0,
        Ok(_) => 1,
        Err(error) => match error.code() {
            ErrorCode::OverConstrained if judged.diagnosis.as_ref().is_some_and(|d| d.dofs < 0) => {
                3
            }
            ErrorCode::OverConstrained => 2,
            ErrorCode::SolveFailed => 4,
            _ => UNBUILDABLE,
        },
    }
}

/// The rank of a solve whose result the file can't hold, or of a sketch that can't be sent
/// (FreeCAD's `InvalidGeometry`): an edit that leaves one is always refused.
const UNBUILDABLE: u8 = 5;

/// The constraints a solve flags redundant (each all of whose equations repeat others':
/// FreeCAD's redundant, not its partially redundant) and in conflict, from its diagnosis,
/// which a solve that failed gives too, as FreeCAD keeps its lists whatever the solve
/// returned; or, for a sketch that couldn't be sent, from its refusal: none.
fn flagged(judged: &Judged) -> (BTreeSet<String>, BTreeSet<String>) {
    let ids = |set: &BTreeSet<ElementId>| set.iter().map(ToString::to_string).collect();
    match &judged.diagnosis {
        Some(diagnosis) => (ids(&diagnosis.redundant), ids(&diagnosis.conflicting)),
        None => (BTreeSet::new(), BTreeSet::new()),
    }
}

/// Why `edit`'s solve, `after`, is worse than the solve of the sketch before it, `before`, if
/// it is (none, unasked, where `after` ranks [`UNBUILDABLE`], which is always worse): as
/// FreeCAD main's `shouldRejectCoincidentSolve` decides (`cb2780e2fa`), a solve
/// that isn't a clean success and ranks [`UNBUILDABLE`], or worse than the solve before
/// ([`severity`]), or flags a constraint redundant or in conflict that the solve before
/// didn't, those named. One rule for every edit that writes geometry: a conflict the sketch
/// had already isn't the edit's. None if the edit stands.
fn worse(before: Option<&Judged>, after: &Judged, edit: Edit) -> Option<Error> {
    let rank = severity(after);
    if rank == 0 {
        return None;
    }
    let (was_redundant, was_conflicting) = before.map(flagged).unwrap_or_default();
    let (redundant, conflicting) = flagged(after);
    let new_redundant: Vec<String> = redundant.difference(&was_redundant).cloned().collect();
    let new_conflicting: Vec<String> = conflicting.difference(&was_conflicting).cloned().collect();
    let worse_rank = rank == UNBUILDABLE || rank > before.map_or(0, severity);
    if !worse_rank && new_redundant.is_empty() && new_conflicting.is_empty() {
        return None;
    }
    let name = edit.name;
    Some(match &after.outcome {
        Ok(_) => {
            let named = if new_redundant.is_empty() {
                redundant.into_iter().collect()
            } else {
                new_redundant
            };
            let n = named.len();
            Error::new(
                ErrorCode::OverConstrained,
                format!(
                    "after the {name}, these {n} constraints repeat others, as the sketch's solve before it found none of them do"
                ),
            )
            .with_elements(named)
            .with_cause(WORSE)
            .with_suggestion(format!(
                "remove the constraints named, then make the {name} again"
            ))
        }
        Err(error) if error.code() == ErrorCode::OverConstrained => {
            let message = format!("after the {name}, {}", error.message());
            error.clone().with_message(message).with_cause(WORSE)
        }
        Err(error) => {
            let message = format!("after the {name}, {}", error.message());
            let error = error.clone().with_message(message);
            match edit.lever {
                Some(lever) if error.code() == ErrorCode::SolveFailed => {
                    error.with_suggestion(lever)
                }
                _ => error,
            }
        }
    })
}

/// Why an edit whose solve is worse is refused.
const WORSE: &str = "an edit that writes geometry is refused where the sketch's solve after it is worse than before it (a conflict the sketch had already isn't the edit's), as FreeCAD's Coincident command rolls one back";
