//! `undo` and `redo` (docs/design/m5.0-app.md, section 7; M5.2): a transaction's edits
//! stepped back and forth through its undo stack ([`crate::document::undo`]), each step at a
//! new revision whose state equals an earlier one, as `rollback` restores at a new revision.
//!
//! The stack keeps texts only. What a step restores is put through the gate again, which
//! gives the same checked form, and what the transaction's edits reach is worked out again
//! from it and what the transaction holds fixed (what was carried, `begin`'s document and
//! its mend: [`Reaching::reach`](super::transaction::Reaching::reach)), as after every
//! change; so a commit after an undo writes fingerprints only for what the edits still
//! standing reach. Its model is rebuilt when a command first needs it (decision 13), as an
//! edit's is, its steps found in the session's engine or its cache.

use std::sync::Arc;

use super::cancel::Ticket;
use super::edits::{Stale, stale};
use super::{Gated, Problems, Revision, Session, SessionId, Stepped};
use crate::document::PROBLEMS_LISTED;
use crate::document::undo::Refusal;
use crate::kernel::Kernel;
use crate::{Error, ErrorCode};

/// Which way a step goes: back to an earlier state, or forward to one undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Way {
    Undo,
    Redo,
}

impl Way {
    /// The command's name, as its refusals say it.
    fn name(self) -> &'static str {
        match self {
            Way::Undo => "undo",
            Way::Redo => "redo",
        }
    }
}

impl<C: Kernel + 'static> Session<C> {
    /// `undo` or `redo` (`way`), made against `revision`: the state before the session's (or
    /// after it, for a redo), or with `to`, the state the session was at at that revision,
    /// restored at a new revision, through the gate, carrying what was carried. Refused, the
    /// session as it was, with no transaction open, against a revision that isn't the
    /// session's (a conflict, decision 12), and as the stack refuses it: nothing to undo or
    /// redo, a `to` it doesn't hold, or one on the wrong side of the session's state (its own
    /// among them). As the command `stop` stops.
    pub(super) fn step(
        &self,
        stop: &Ticket,
        way: Way,
        revision: Revision,
        to: Option<Revision>,
    ) -> Result<Stepped, Problems> {
        let _span = tracing::info_span!(
            "step",
            way = way.name(),
            revision = revision.number(),
            to = to.map(Revision::number)
        )
        .entered();
        // What the step restores, found under the held state's lock with the stack left as
        // it is, so a `status` read meanwhile is the state before the step, whole.
        let (next, plan, begun, formatted) = {
            let held = self.held.read();
            let Some(open) = held.transaction.as_ref() else {
                return Err(Problems::from(no_transaction(self.id, way)));
            };
            if held.revision != revision {
                let what = match way {
                    Way::Undo => Stale::Undo,
                    Way::Redo => Stale::Redo,
                };
                // Inside a transaction, the file is never reloaded: no reload to name.
                return Err(Problems::from(stale(what, revision, held.revision, None)));
            }
            let next = held.next()?;
            let plan = match way {
                Way::Undo => open.undo.plan_undo(to),
                Way::Redo => open.undo.plan_redo(to),
            }
            .map_err(|refusal| refused(self.id, way, refusal))?;
            // What `format` had put into the document at the state restored: an undo of a
            // `format` takes it back, a redo gives it back.
            let formatted = open.formatted_at(plan.restored());
            let mut begun = open.begun.clone();
            begun.set_formatted(Arc::clone(&formatted));
            (next, plan, begun, formatted)
        };
        // Written from the stack's chunks, gated, and compared with `begin`'s document with
        // the lock let go, so `status` never waits on any of it. Only the session's thread
        // moves the stack or changes what's held, and this is it: the plan is still the
        // stack's when the lock is taken again.
        let text = plan.text();
        let gated = Arc::new(Gated::of(&text));
        let changed = begun.differs(&text, &gated);
        // Then the stack stepped, `changed`, and the document changed, under one lock, so a
        // `status` read after is the state after the step, whole.
        let mut held = self.held.read().changing(stop)?;
        let Some(open) = held.transaction.as_mut() else {
            return Err(Problems::from(no_transaction(self.id, way)));
        };
        let restored = open
            .undo
            .apply(plan, next)
            .map_err(|refusal| refused(self.id, way, refusal))?;
        open.changed = changed;
        open.begun.set_formatted(formatted);
        let carried = Arc::clone(&held.carried);
        held.change(next, text, gated, carried);
        drop(held);
        tracing::info!(
            session = %self.id,
            revision = next.number(),
            restored = restored.restored.number(),
            undo = restored.undo,
            redo = restored.redo,
            dropped = restored.dropped,
            "{}",
            way.name()
        );
        Ok(Stepped {
            restored: restored.restored,
            undo: restored.undo,
            redo: restored.redo,
            dropped: restored.dropped,
        })
    }
}

/// The refusal of `way` in session `id` with no transaction open (the design's section 7).
fn no_transaction(id: SessionId, way: Way) -> Error {
    let way = way.name();
    Error::new(
        ErrorCode::InvalidInput,
        format!("no transaction is open in session {id}"),
    )
    .with_parameter("session")
    .with_cause(format!(
        "{way} steps through a transaction's edits: outside one, each edit was saved as it was made"
    ))
    .with_suggestion(format!(
        "edit inside a transaction to {way}; a saved file's history is git's"
    ))
}

/// The refusal of `way` in session `id` that the stack refused as `refusal` (the design's
/// section 7).
fn refused(id: SessionId, way: Way, refusal: Refusal<Revision>) -> Problems {
    let name = way.name();
    Problems::from(match refusal {
        Refusal::NothingToUndo | Refusal::NothingToRedo => {
            let error = Error::new(
                ErrorCode::InvalidInput,
                format!("nothing to {name} in session {id}'s transaction"),
            )
            .with_parameter("session")
            .with_suggestion("roll back to return to the state the transaction began from");
            match way {
                Way::Undo => error,
                Way::Redo => error.with_cause("an edit after an undo drops what was undone"),
            }
        }
        Refusal::NotHeld { to, held } => not_held(id, way, to, &held),
        Refusal::WrongSide { to } => {
            let to = to.number();
            let (message, cause, suggestion) = match way {
                Way::Undo => (
                    format!(
                        "revision {to} isn't before session {id}'s state now, so undo can't restore it"
                    ),
                    format!(
                        "undo restores an earlier state: revision {to} is the session's state now, or one after it"
                    ),
                    "undo to an earlier revision the stack holds, or redo to a later one",
                ),
                Way::Redo => (
                    format!(
                        "revision {to} isn't after session {id}'s state now, so redo can't restore it"
                    ),
                    format!(
                        "redo restores a later state: revision {to} is the session's state now, or one before it"
                    ),
                    "redo to a later revision the stack holds, or undo to an earlier one",
                ),
            };
            Error::new(ErrorCode::InvalidInput, message)
                .with_parameter("to")
                .with_cause(cause)
                .with_suggestion(suggestion)
        }
    })
}

/// The refusal of `way` in session `id` to `to`, a revision no state `way` can restore
/// was: the revisions those states were, `held` (in order: for undo, those before the
/// session's state that undo reaches; for redo, those after it), named as a range, and the
/// newest [`PROBLEMS_LISTED`] of them its candidates, newest first, each a `to` the same
/// command takes. The range can have gaps (an edit after an undo drops the redo states'
/// revisions from between held ones), so it can even include `to`.
fn not_held(id: SessionId, way: Way, to: Revision, held: &[Revision]) -> Error {
    let holds = match (held.first(), held.last()) {
        (Some(first), Some(last)) if first == last => format!("revision {}", first.number()),
        (Some(first), Some(last)) => {
            format!("revisions {} to {}", first.number(), last.number())
        }
        _ => "none".to_owned(),
    };
    let cause = match way {
        Way::Undo => {
            "the stack dropped its oldest edits past its bound, or the revision is from before the transaction"
        }
        Way::Redo => {
            "an edit after an undo drops what was undone, or the revision isn't one of the transaction's states the stack kept"
        }
    };
    Error::new(
        ErrorCode::InvalidInput,
        format!(
            "revision {} isn't a state of session {id}'s undo stack that {way} can restore: it can restore {holds}",
            to.number(),
            way = way.name()
        ),
    )
    .with_parameter("to")
    .with_cause(cause)
    .with_suggestion(format!(
        "{} to a revision it can restore, or roll back to the transaction's start",
        way.name()
    ))
    .with_candidates(
        held.iter()
            .rev()
            .take(PROBLEMS_LISTED)
            .map(|revision| revision.number().to_string()),
    )
}
