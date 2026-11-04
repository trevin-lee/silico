//! Whether an edit that writes geometry leaves its sketch's solve worse than the solve
//! before it, and so is refused (SD7, refined: a conflict the sketch had already isn't the
//! edit's; sketch-edit decision 10), by one rule for split, trim, unjoin, merge, extend, and join, as
//! FreeCAD main's Coincident command rolls one back (`shouldRejectCoincidentSolve`,
//! `cb2780e2fa`). The sketch-edit commands judge their edits by it
//! (`command::session::sketch_edits`), and so does the sketch-edit oracle, which compares
//! the edits with FreeCAD's.

use std::collections::BTreeSet;

use crate::document::ElementId;
use crate::sketch::{Answer, Diagnosis, SketchSolution};
use crate::{Error, ErrorCode};

/// An edit that writes geometry, as its judgment says it: its name, as a refusal names it
/// ("after the split, ..."), and what to suggest where its solve fails, or none for the
/// solve's own advice.
#[derive(Clone, Copy)]
pub struct WritingEdit {
    name: &'static str,
    lever: Option<&'static str>,
}

pub const SPLIT: WritingEdit = WritingEdit {
    name: "split",
    lever: None,
};
pub const TRIM: WritingEdit = WritingEdit {
    name: "trim",
    lever: None,
};
pub const UNJOIN: WritingEdit = WritingEdit {
    name: "unjoin",
    lever: None,
};
pub const EXTEND: WritingEdit = WritingEdit {
    name: "extend",
    lever: None,
};
pub const JOIN: WritingEdit = WritingEdit {
    name: "join",
    lever: None,
};
/// A drag's end writes the sketch as its whole solve from the cursor left it
/// (docs/design/m5.0-app.md, `drag_end`): judged as every edit that writes geometry is, so a
/// release into a configuration the solve finds worse (a redundancy the sketch hadn't) is
/// refused, the document as it was. A solve that fails there already kept the begin state,
/// with no edit, so there's no lever to suggest.
pub const DRAG: WritingEdit = WritingEdit {
    name: "drag",
    lever: None,
};
/// A merge's solve starts from the kept point's place, by the request's order (Fable,
/// 2026-10-29): the lever is the request's.
pub const MERGE: WritingEdit = WritingEdit {
    name: "merge",
    lever: Some(
        "name first the point the constraints hold, or move the points together, then merge",
    ),
};

/// A sketch's solve, as an edit's judgment reads it: its outcome, the solution or the solve's
/// refusal of the sketch, and the diagnosis PlaneGCS gave with it, which every answer the
/// worker gave has, refused or not (none for a sketch that couldn't be sent).
pub struct Judged {
    pub outcome: Result<SketchSolution, Error>,
    pub diagnosis: Option<Diagnosis>,
}

impl Judged {
    /// A solve's answer as the judgment reads it: its outcome, and its diagnosis, where the
    /// core could read one.
    pub fn of(answer: Answer) -> Judged {
        let diagnosis = answer.diagnosed().ok().map(|d| d.diagnosis().clone());
        Judged {
            outcome: answer.into_outcome(),
            diagnosis,
        }
    }
}

/// How bad a solve is, as FreeCAD main's coincident rollback ranks it
/// (`getCoincidentSolveStatusSeverity`, `CommandConstraints.cpp`, with
/// `getCoincidentSolveStatus`, which reads a negative DoF count first): solved, 0; solved with
/// redundant constraints, 1; constraints in conflict, 2, or over-constrained, 3, where the
/// diagnosis leaves fewer than no degrees of freedom; no solution, 4; anything else (a result
/// the file can't hold, a sketch that can't be sent), [`UNBUILDABLE`].
pub fn severity(judged: &Judged) -> u8 {
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
pub const UNBUILDABLE: u8 = 5;

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
pub fn judge(before: Option<&Judged>, after: &Judged, edit: WritingEdit) -> Option<Error> {
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
            Error::new(
                ErrorCode::OverConstrained,
                format!(
                    "after the {name}, the constraints named repeat others, as the sketch's solve before it found none of them do"
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
