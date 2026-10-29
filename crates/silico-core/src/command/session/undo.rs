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
        let (next, restored) = {
            let held = self.held.read();
            if held.transaction.is_none() {
                return Err(Problems::from(no_transaction(self.id, way)));
            }
            if held.revision != revision {
                let what = match way {
                    Way::Undo => Stale::Undo,
                    Way::Redo => Stale::Redo,
                };
                // Inside a transaction, the file is never reloaded: no reload to name.
                return Err(Problems::from(stale(what, revision, held.revision, None)));
            }
            let next = held.next()?;
            let mut held = held.changing(stop)?;
            let Some(stack) = held.undo.as_mut() else {
                return Err(Problems::from(no_transaction(self.id, way)));
            };
            let stepped = match way {
                Way::Undo => stack.undo(to, next),
                Way::Redo => stack.redo(to, next),
            };
            (
                next,
                stepped.map_err(|refusal| refused(self.id, way, refusal))?,
            )
        };
        // Gated with the held state's lock let go, so `status` never waits on it; nothing
        // else changes what's held meanwhile, since only the session's thread changes it.
        let gated = Arc::new(Gated::of(&restored.text));
        // Settled above, so never refused.
        let mut held = self.held.read().changing(stop)?;
        let carried = Arc::clone(&held.carried);
        held.change(next, restored.text, gated, carried);
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

/// The refusal of `way` in session `id` to `to`, a revision its undo stack doesn't hold:
/// the revisions it holds, `held` (in order), named as a range, and the newest
/// [`PROBLEMS_LISTED`] of them its candidates, newest first, since the range can have gaps
/// (an edit after an undo drops the redo states' revisions from between held ones, so the
/// range can even include `to`).
fn not_held(id: SessionId, way: Way, to: Revision, held: &[Revision]) -> Error {
    let holds = match (held.first(), held.last()) {
        (Some(first), Some(last)) if first == last => format!("revision {}", first.number()),
        (Some(first), Some(last)) => {
            format!("revisions {} to {}", first.number(), last.number())
        }
        _ => "no revision".to_owned(),
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
            "revision {} isn't a state of session {id}'s undo stack, which holds {holds}",
            to.number()
        ),
    )
    .with_parameter("to")
    .with_cause(cause)
    .with_suggestion(format!(
        "{} to a revision the stack holds, or roll back to the transaction's start",
        way.name()
    ))
    .with_candidates(
        held.iter()
            .rev()
            .take(PROBLEMS_LISTED)
            .map(|revision| revision.number().to_string()),
    )
}
