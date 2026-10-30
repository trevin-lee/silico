//! The sketch edits (docs/design/m5.0-app.md, section 7, *The sketch edits*), each an edit
//! of the session's document made against a revision, as `set_field` is ([`Session::edit`]):
//! held inside a transaction, on its undo stack; committed as its own transaction outside
//! one.
//!
//! - **What adds or removes what the person wrote**, `sketch_add` and `sketch_remove`, never
//!   solves the sketch, nor is refused for what its solve finds (SD7): once the gate takes
//!   the edit, it's applied, and the rebuild's verdict, a conflict, a redundancy, or no
//!   solution, is the sketch's state, which `check` reports and its features fail by.
//! - **What writes geometry**, `sketch_split`, `sketch_trim`, `sketch_unjoin`, and
//!   `sketch_merge`, writes exactly what its request names into the sketch as stored, and
//!   the sketch's ordinary solve from that written sketch (M3.4, FreeCAD's order) is the
//!   edit's solve (Fable, 2026-10-29): run here, on one of the sessions' sketch solvers,
//!   before the edit is held or committed, stopped by the command's `cancel`. A solve that
//!   conflicts, fails, or gives a result the file can't hold refuses the edit as data, the
//!   session as it was (SD7, M3.3); a merge is refused only where its solve is worse than the
//!   one before it, as FreeCAD main's Coincident command rolls one back
//!   (`shouldRejectCoincidentSolve`, `cb2780e2fa`; sketch-edit decision 10). No edit tries a
//!   second seed, retries, snaps, or solves again: the rebuild then solves the same text the
//!   same way.
//!
//! [`Session::edit`]: super::Session

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, Weak};

use super::cancel::{Interrupt, Ticket, Why, cancelled};
use super::{
    NewMembers, Problems, Running, SessionCommand, SessionId, Sessions, SketchCut, SketchEdited,
    reclaim,
};
use crate::command::Gated;
use crate::command::Revision;
use crate::command::edit::{self, Candidate, Cut, FeatureIds, SketchChanges};
use crate::document::sketch_edit::NewIds;
use crate::document::{Checked, ElementId, FeatureId, Reference};
use crate::drag::DragStop;
use crate::kernel::Clients;
use crate::recompute::SketchSolvers;
use crate::sketch::SketchSolution;
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
    /// judges it: refused, the session as it was, for a conflict, a failed solve, or a
    /// result the file can't hold (SD7), as for what the split itself refuses (a position
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
                        let document = base.checked()?.document();
                        let made = with_ids(ids.as_deref(), &drawn, |ids| {
                            edit::sketch_split(document, &sketch, &curve, &at, ids)
                        })?;
                        judged(&solvers, stop, &sketch, "split", made)
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
                        let document = base.checked()?.document();
                        let made = with_ids(ids.as_deref(), &drawn, |ids| {
                            edit::sketch_trim(document, &sketch, &curve, pick, ids)
                        })?;
                        judged(&solvers, stop, &sketch, "trim", made)
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
                        let document = base.checked()?.document();
                        let made = with_ids(ids.as_deref(), &drawn, |ids| {
                            edit::sketch_unjoin(document, &sketch, &point, &curve, ids)
                        })?;
                        judged(&solvers, stop, &sketch, "unjoin", made)
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
    /// it takes no `ids`. Refused, the session as it was, where its solve is worse than the
    /// one before it (FreeCAD main's `shouldRejectCoincidentSolve`, `cb2780e2fa`): a result
    /// the file can't hold, a solve that fails where the one before didn't fail so, or a
    /// constraint the solve finds conflicting or redundant that the one before didn't, each
    /// naming the constraints; and for what the merge itself refuses (points already one, a
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
                        let Some(candidate) = &made.1 else {
                            return Ok(made);
                        };
                        let after = solved(&solvers, stop, checked_of(candidate)?, &sketch)?;
                        if let Err(error) = &after
                            && always_refused(error)
                        {
                            return Err(refusal(&sketch, "merge", error.clone()));
                        }
                        let before = solved(&solvers, stop, checked, &sketch)?;
                        if let Some(error) = worse(&before, &after) {
                            return Err(Problems::from(error.with_feature(sketch.as_str())));
                        }
                        Ok(made)
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

/// An edit that writes geometry, `made`, judged by its sketch's solve (SD7): its candidate's
/// sketch `sketch` solved, and `made` given back if it solved; refused, as the `edit`
/// ("split") that wrote it, if the solve conflicts, fails, or gives a result the file can't
/// hold. An edit that changes nothing isn't solved.
fn judged<T>(
    solvers: &SketchSolvers,
    stop: &Ticket,
    sketch: &FeatureId,
    edit: &str,
    made: (T, Option<Candidate>),
) -> Result<(T, Option<Candidate>), Problems> {
    let Some(candidate) = &made.1 else {
        return Ok(made);
    };
    match solved(solvers, stop, checked_of(candidate)?, sketch)? {
        Ok(_) => Ok(made),
        Err(error) => Err(refusal(sketch, edit, error)),
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

/// What stops an edit's solve: its command's `cancel` (or its session's close) stops the
/// solve on the worker, which the edit then answers as `cancelled`.
struct SolveStop(DragStop);

impl Interrupt for SolveStop {
    fn interrupt(&self, _why: Why) {
        self.0.stop();
    }
}

/// The sketch `sketch` of `document` solved, as a rebuild solves it (M3.4: from the file's
/// coordinates, FreeCAD's order), on a solver of `solvers`, given back after: the solution,
/// or the solve's refusal of the sketch (a conflict, no solution, a result the file can't
/// hold, or a sketch that can't be sent). A stop by the command `stop` is `cancelled`, the
/// command's; a solver that couldn't be had, or failed in a way that isn't the sketch's (a
/// crash, a hang, a reply it can't be trusted for), is that fault, refusing the edit.
fn solved(
    solvers: &SketchSolvers,
    stop: &Ticket,
    document: &Checked,
    sketch: &FeatureId,
) -> Result<Result<SketchSolution, Error>, Problems> {
    let _span = tracing::info_span!("edit_solve", sketch = sketch.as_str()).entered();
    let halt = Arc::new(SolveStop(DragStop::new()));
    stop.interrupt_with(Arc::downgrade(&halt) as Weak<dyn Interrupt>);
    let mut solver = solvers.take().map_err(Problems::from)?;
    let outcome = halt.0.run(solver.as_mut(), |solver| {
        crate::sketch::solve(document, sketch, solver)
    });
    solvers.give_back(solver);
    match outcome {
        Err(error) if error.code() == ErrorCode::Cancelled || stop.cancelled() => {
            Err(Problems::from(cancelled(stop.name())))
        }
        Err(error) if error.code().is_kernel_fault() => Err(Problems::from(error)),
        outcome => Ok(outcome),
    }
}

/// The refusal of the `edit` ("split") of sketch `sketch` whose solve refused it as `error`
/// (SD7): the solve's own words, code, constraints, cause, and advice, said as after the
/// edit, naming the sketch. The session is left as it was.
fn refusal(sketch: &FeatureId, edit: &str, error: Error) -> Problems {
    let message = format!("after the {edit}, {}", error.message());
    Problems::from(error.with_message(message).with_feature(sketch.as_str()))
}

/// A merge's solve that refuses it whatever the solve before it found: a result the file
/// can't hold, or a sketch that can't be sent (FreeCAD's `InvalidGeometry`, which its
/// rollback always refuses).
fn always_refused(error: &Error) -> bool {
    !matches!(
        error.code(),
        ErrorCode::OverConstrained | ErrorCode::SolveFailed
    )
}

/// How bad a solve's outcome is, as FreeCAD main's coincident rollback ranks it
/// (`getCoincidentSolveStatusSeverity`, `CommandConstraints.cpp`): solved, 0; solved with
/// redundant constraints, 1; constraints in conflict, 2 (FreeCAD's conflicting and
/// over-constrained, which Silico's solve doesn't tell apart, 2 and 3); no solution, 4;
/// anything else (a result the file can't hold, a sketch that can't be sent), 5.
fn severity(outcome: &Result<SketchSolution, Error>) -> u8 {
    match outcome {
        Ok(solution) if solution.redundant.is_empty() => 0,
        Ok(_) => 1,
        Err(error) => match error.code() {
            ErrorCode::OverConstrained => 2,
            ErrorCode::SolveFailed => 4,
            _ => 5,
        },
    }
}

/// The constraints `outcome` flags redundant (each all of whose equations repeat others':
/// FreeCAD's redundant, not its partially redundant), and those in conflict.
fn flagged(outcome: &Result<SketchSolution, Error>) -> (BTreeSet<String>, BTreeSet<String>) {
    match outcome {
        Ok(solution) => (
            solution.redundant.keys().map(|id| id.to_string()).collect(),
            BTreeSet::new(),
        ),
        Err(error) if error.code() == ErrorCode::OverConstrained => {
            (BTreeSet::new(), error.elements().iter().cloned().collect())
        }
        Err(_) => (BTreeSet::new(), BTreeSet::new()),
    }
}

/// Why a merge's solve, `after`, is worse than the solve before it, `before`, if it is: as
/// FreeCAD main's `shouldRejectCoincidentSolve` decides (`cb2780e2fa`), a solve that's not a
/// clean success and either ranks worse ([`severity`]) or flags a constraint redundant or in
/// conflict that the solve before didn't, named. None if the merge stands.
fn worse(
    before: &Result<SketchSolution, Error>,
    after: &Result<SketchSolution, Error>,
) -> Option<Error> {
    let rank = severity(after);
    if rank == 0 {
        return None;
    }
    let (was_redundant, was_conflicting) = flagged(before);
    let (redundant, conflicting) = flagged(after);
    let new_redundant: Vec<String> = redundant.difference(&was_redundant).cloned().collect();
    let new_conflicting: Vec<String> = conflicting.difference(&was_conflicting).cloned().collect();
    let worse_rank = rank > severity(before);
    if !worse_rank && new_redundant.is_empty() && new_conflicting.is_empty() {
        return None;
    }
    const CAUSE: &str = "a merge whose solve is worse than the one before it is refused, as FreeCAD's Coincident command rolls it back";
    Some(match after {
        Ok(_) => {
            let named = if new_redundant.is_empty() {
                redundant.into_iter().collect()
            } else {
                new_redundant
            };
            let n = named.len();
            Error::new(
                ErrorCode::OverConstrained,
                format!("after the merge, these {n} constraints repeat others, as the sketch's solve before it found none of them do"),
            )
            .with_elements(named)
            .with_cause(CAUSE)
            .with_suggestion("remove the constraints named first, or merge other points")
        }
        Err(error) if error.code() == ErrorCode::OverConstrained => {
            let message = format!("after the merge, {}", error.message());
            error
                .clone()
                .with_message(message)
                .with_cause(CAUSE)
                .with_suggestion(
                    "remove or change one of the constraints named first, or merge other points",
                )
        }
        Err(error) => {
            let message = format!("after the merge, {}", error.message());
            error.clone().with_message(message).with_suggestion(
                "name first the point the constraints hold, or move the points together, then merge",
            )
        }
    })
}
