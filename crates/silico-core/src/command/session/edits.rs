//! An edit of a session's document ([`Session::edit`]), made against a revision: held inside
//! a transaction, or committed as its own outside one.

use std::path::Path;
use std::sync::Arc;

use super::{Committed, Gated, ModelRead, Problems, Revision, Session, SessionId, shown};
use crate::command::edit::Candidate;
use crate::document::Document;
use crate::kernel::Kernel;
use crate::worker::lock;
use crate::{Error, ErrorCode};

impl<C: Kernel + 'static> Session<C> {
    /// An accepted edit's `candidate` held as the session's document, at a new revision,
    /// carrying what was carried: how an edit inside a transaction is applied (the design's
    /// section 4), starting no rebuild (decision 13).
    pub(super) fn hold(&self, candidate: Candidate) -> Result<Revision, Problems> {
        let mut held = lock(&self.held);
        let next = held.next()?;
        let carried = Arc::clone(&held.carried);
        held.change(next, candidate.text, candidate.gated, carried);
        Ok(next)
    }

    /// An edit (the design's section 5) made against `revision`: refused as a conflict if
    /// that isn't the session's (decision 12). `make` is given the session's text, the
    /// gate's finding of it, and whether a transaction is open, and gives what it found and
    /// the state the edit leads to, none if the document is the same. Inside a transaction
    /// that state is held, starting no rebuild (decision 13); outside one, it's committed as
    /// its own transaction ([`commit_candidate`](Self::commit_candidate), decision 1), its
    /// new file claimed with `claim`, and the session holds it only once it's saved. An edit
    /// that changes nothing gives no new revision and writes nothing. Refused whole, the
    /// session as it was, as `make` or the commit is.
    pub(super) fn edit<T>(
        self: &Arc<Self>,
        revision: Revision,
        make: impl FnOnce(Base<'_>) -> Result<(T, Option<Candidate>), Problems>,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
    ) -> Result<Edited<T>, Problems> {
        let (text, gated, transaction) = {
            let held = lock(&self.held);
            if held.revision != revision {
                // Stale because the file changed outside the session, if a reload since
                // the edit's revision moved the session on.
                let reloaded = held
                    .last_reload
                    .filter(|reloaded| *reloaded > revision)
                    .map(|_| held.path.as_str());
                return Err(Problems::from(stale(revision, held.revision, reloaded)));
            }
            (
                Arc::clone(&held.text),
                Arc::clone(&held.gated),
                held.transaction.is_some(),
            )
        };
        let (made, candidate) = {
            let _span = tracing::info_span!("edit", revision = revision.number()).entered();
            make(Base {
                id: self.id,
                text: &text,
                gated: &gated,
                transaction,
            })?
        };
        let Some(candidate) = candidate else {
            return Ok(Edited {
                made,
                changed: false,
                commit: None,
                saved: None,
            });
        };
        let (commit, saved) = if transaction {
            self.hold(candidate)?;
            (None, None)
        } else {
            let committed = self.commit_candidate(candidate, read, claim)?;
            // What the commit saved is what the session holds now: its commands run in turn,
            // so none has changed it since.
            let saved = committed
                .written
                .then(|| Arc::clone(&lock(&self.held).gated));
            (Some(committed), saved)
        };
        Ok(Edited {
            made,
            changed: true,
            commit,
            saved,
        })
    }
}

/// What an edit is made from ([`Session::edit`]): the session's ID, its text, the gate's
/// finding of it, and whether a transaction is open.
pub(super) struct Base<'a> {
    pub(super) id: SessionId,
    pub(super) text: &'a [u8],
    pub(super) gated: &'a Gated,
    pub(super) transaction: bool,
}

impl<'a> Base<'a> {
    /// The document a typed edit (`add_feature`, `set_field`, `remove_feature`) edits: one
    /// the gate took. A document it refused is mended by `replace_text` alone (decision 21),
    /// so every other edit of it is refused.
    pub(super) fn checked(&self) -> Result<&'a Document, Problems> {
        match self.gated {
            Gated::Checked(checked) => Ok(checked.document()),
            Gated::Unread(_) | Gated::Refused(..) => Err(Problems::from(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "session {}'s document is one the gate refuses, so only replace_text edits it",
                        self.id
                    ),
                )
                .with_parameter("session")
                .with_suggestion(
                    "read its problems (check), then mend its text with replace_text",
                ),
            )),
        }
    }
}

/// What an edit gives its command ([`Session::edit`]): what it found, whether the document
/// changed, and, outside a transaction, the commit's result and the document it saved (none
/// inside one, or if nothing changed or was written).
pub(super) struct Edited<T> {
    pub(super) made: T,
    pub(super) changed: bool,
    pub(super) commit: Option<Committed>,
    pub(super) saved: Option<Arc<Gated>>,
}

/// What an edit gives back as the document holds it once committed (the design's section
/// 7): `made`, the edit's own, if its commit saved no document (`saved` none: inside a
/// transaction, or nothing written); otherwise found by `find` in the document saved, with
/// the fingerprints and solves the commit wrote. A saved document `find` finds nothing in is
/// Silico's bug, said as one, the edit saved.
pub(super) fn as_committed<U>(
    saved: Option<&Gated>,
    made: U,
    find: impl FnOnce(&Document) -> Option<U>,
) -> Result<U, Problems> {
    let Some(saved) = saved else {
        return Ok(made);
    };
    let found = match saved {
        Gated::Checked(checked) => find(checked.document()),
        Gated::Unread(_) | Gated::Refused(..) => None,
    };
    found.ok_or_else(|| {
        Problems::from(
            Error::new(
                ErrorCode::Internal,
                "the edit was saved, but the document saved doesn't hold what it made",
            )
            .with_cause("the file was written: the session's text is what it holds")
            .with_suggestion(crate::worker::SILICO_BUG),
        )
    })
}

/// The refusal of an edit made against revision `made`, the session being at `at`
/// (decision 12): a conflict, so an agent that read the document before it changed never
/// writes over the change; its cause, if a reload of the file at `reloaded` (as `open` was
/// given it) moved the session on since `made`, says so.
fn stale(made: Revision, at: Revision, reloaded: Option<&str>) -> Error {
    let error = Error::new(
        ErrorCode::Conflict,
        format!(
            "the edit was made against revision {}, and the session is at revision {}",
            made.number(),
            at.number()
        ),
    )
    .with_parameter("revision")
    .with_suggestion(format!(
        "read the document again (text or outline), then make the edit against revision {}",
        at.number()
    ));
    match reloaded {
        Some(path) => error.with_cause(format!(
            "{} was reloaded: it changed outside this session",
            shown(Path::new(path))
        )),
        None => error,
    }
}

/// The refusal of session `id`'s document replaced outside a transaction when its text
/// isn't UTF-8 (`valid_up_to` its valid bytes): `replace_text` gives back the text it
/// replaces, and couldn't give back this one.
pub(super) fn not_given_back(id: SessionId, valid_up_to: usize) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!(
            "session {id}'s text isn't UTF-8 (at byte {valid_up_to}), so replace_text couldn't give it back"
        ),
    )
    .with_parameter("session")
    .with_suggestion(
        "replace it inside a transaction (begin, replace_text, commit), where no text is given back",
    )
}
