//! The revisions a session holds the text of, and `diff`, which reads the changes between
//! two of them (Fable's decision of 2026-11-04, *the three core gaps*, sections 1 and 2): a
//! session holds a few revisions' texts (its current one; the one its last reload replaced;
//! inside a transaction, `base`), and a command may name any of them. One resolver,
//! [`Held::text_at`], answers for each command that names one.

use std::sync::Arc;

use super::cancel::{Ticket, cancelled};
use super::transaction::Held;
use super::{Gated, Problems, Revision, Session, SessionId};
use crate::document::{self, Change, Document, FeatureId};
use crate::kernel::Kernel;
use crate::{Error, ErrorCode};

/// A text the session holds, at its revision: and, where the session keeps it, the reader's
/// and the gate's finding of it (the current revision's and the transaction's base's), so
/// it isn't read again.
pub(super) struct HeldText {
    pub(super) revision: Revision,
    pub(super) text: Arc<Vec<u8>>,
    pub(super) gated: Option<Arc<Gated>>,
}

/// What `diff` answers: the two revisions compared, as the session resolved them (the
/// request's `null` as its revision then), and each change from the first to the second
/// ([`document::diff`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffAnswer {
    pub from: Revision,
    pub to: Revision,
    pub changes: Vec<Change>,
}

impl Held {
    /// The text the session holds at `revision` (`None` for its current revision), or the
    /// refusal of one it doesn't hold, naming `parameter`, the request's field, with the
    /// revisions it holds as candidates, oldest first: its current one, the one its last
    /// reload replaced, and, inside a transaction, the one `begin` saw.
    pub(super) fn text_at(
        &self,
        session: SessionId,
        revision: Option<Revision>,
        parameter: &'static str,
    ) -> Result<HeldText, Error> {
        let current = || HeldText {
            revision: self.revision,
            text: Arc::clone(&self.text),
            gated: Some(Arc::clone(&self.gated)),
        };
        let Some(asked) = revision else {
            return Ok(current());
        };
        if asked == self.revision {
            return Ok(current());
        }
        if let Some(open) = &self.transaction
            && open.begun.revision == asked
        {
            let (text, gated) = open.begun.held();
            return Ok(HeldText {
                revision: asked,
                text,
                gated: Some(gated),
            });
        }
        if let Some((replaced, text)) = &self.reloaded_over
            && *replaced == asked
        {
            return Ok(HeldText {
                revision: asked,
                text: Arc::clone(text),
                gated: None,
            });
        }
        Err(not_held(session, asked, &self.held_revisions(), parameter))
    }

    /// Every revision whose text the session holds, oldest first, each once.
    fn held_revisions(&self) -> Vec<Revision> {
        let mut held = vec![self.revision];
        held.extend(self.reloaded_over.as_ref().map(|(revision, _)| *revision));
        held.extend(self.transaction.as_ref().map(|open| open.begun.revision));
        held.sort_unstable();
        held.dedup();
        held
    }
}

/// The refusal of `asked`, a revision session `id` doesn't hold the text of, named by the
/// request's `parameter`, with the revisions it does (`held`, oldest first) as candidates.
fn not_held(id: SessionId, asked: Revision, held: &[Revision], parameter: &str) -> Error {
    let numbers: Vec<String> = held.iter().map(|r| r.number().to_string()).collect();
    let holds = match numbers.as_slice() {
        [] => "none".to_owned(),
        [one] => format!("revision {one}"),
        [first, second] => format!("revisions {first} and {second}"),
        [rest @ .., last] => format!("revisions {}, and {last}", rest.join(", ")),
    };
    Error::new(
        ErrorCode::InvalidInput,
        format!(
            "session {id} doesn't hold the text of revision {}: it holds {holds}",
            asked.number()
        ),
    )
    .with_parameter(parameter)
    .with_cause(
        "a session holds its current revision's text, the one its last reload replaced, and, \
         inside a transaction, the one it began at",
    )
    .with_suggestion(format!(
        "give {parameter} a revision the session holds, or null for the document now"
    ))
    .with_candidates(numbers)
}

/// The document `held` holds, read; or the refusal of a text that doesn't read as one,
/// naming the request's `parameter`, with the reader's message as its cause.
fn read_side(held: &HeldText, parameter: &'static str) -> Result<SideDocument, Error> {
    let refused = |message: &str| {
        Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{parameter}, revision {}, doesn't read as a document, so it can't be compared",
                held.revision.number()
            ),
        )
        .with_parameter(parameter)
        .with_cause(message.to_owned())
        .with_suggestion(format!(
            "give {parameter} a revision whose text reads as a document; text gives a revision's text"
        ))
    };
    match &held.gated {
        Some(gated) => match gated.document() {
            Ok(_) => Ok(SideDocument::Held(Arc::clone(gated))),
            Err(problems) => Err(refused(
                problems
                    .errors()
                    .first()
                    .map_or("the reader refused it", Error::message),
            )),
        },
        None => document::read(&held.text)
            .map(|document| SideDocument::Read(Box::new(document)))
            .map_err(|e| refused(e.message())),
    }
}

/// A side of a diff, read: the gate's finding the session kept, or the document read again.
enum SideDocument {
    Held(Arc<Gated>),
    Read(Box<Document>),
}

impl SideDocument {
    fn document(&self) -> Option<&Document> {
        match self {
            SideDocument::Held(gated) => gated.document().ok(),
            SideDocument::Read(document) => Some(document),
        }
    }
}

impl<C: Kernel + 'static> Session<C> {
    /// [`Held::text_at`], of what the session holds now.
    pub(super) fn text_at(
        &self,
        revision: Option<Revision>,
        parameter: &'static str,
    ) -> Result<HeldText, Error> {
        self.held.read().text_at(self.id, revision, parameter)
    }

    /// `text`'s work: the text the session holds at `revision` (`None` for its revision
    /// now), or, given a `feature`, that feature's canonical text as that revision's
    /// document has it ([`document::write_feature`]). Refused for a revision not held
    /// (naming `revision`), a text that isn't UTF-8, and, for a feature, a text that doesn't
    /// read as a document or hasn't one feature of that ID.
    pub(super) fn text(
        &self,
        feature: Option<FeatureId>,
        revision: Option<Revision>,
    ) -> Result<String, Problems> {
        let held = self.text_at(revision, "revision")?;
        match feature {
            None => String::from_utf8(held.text.to_vec()).map_err(|e| {
                Problems::from(
                    Error::new(
                        ErrorCode::InvalidInput,
                        format!(
                            "the document's text isn't UTF-8 (at byte {}), so it can't be given as text",
                            e.utf8_error().valid_up_to()
                        ),
                    )
                    .with_suggestion("mend the file: a document is UTF-8 text"),
                )
            }),
            Some(id) => {
                // The gate's finding where the session keeps it; the text read again where
                // it doesn't (the one a reload replaced).
                let read;
                let document = match &held.gated {
                    Some(gated) => gated.document().map_err(Clone::clone)?,
                    None => {
                        read = document::read(&held.text)?;
                        &read
                    }
                };
                Ok(document::write_feature(document.feature(&id)?)?)
            }
        }
    }

    /// `diff`'s work: the changes from the revision `from` names to the one `to` names
    /// (`None` for the current revision), each one the session holds, each read as a
    /// document. Refused with every reason: a side not held, or one that doesn't read; and
    /// stopped, `cancelled`, before the next feature is compared once `stop` is cancelled.
    pub(super) fn diff(
        &self,
        stop: &Ticket,
        from: Option<Revision>,
        to: Option<Revision>,
    ) -> Result<DiffAnswer, Problems> {
        let _span = tracing::info_span!(
            "diff",
            from = from.map(Revision::number),
            to = to.map(Revision::number)
        )
        .entered();
        // Only this session's thread changes what's held, and this command runs on it: both
        // sides are of one state, each taken under the lock, read with it let go.
        let from = self.text_at(from, "from");
        let to = self.text_at(to, "to");
        let read = |side: &Result<HeldText, Error>, parameter| match side {
            Ok(held) => read_side(held, parameter).map(|doc| (held.revision, doc)),
            Err(e) => Err(e.clone()),
        };
        match (read(&from, "from"), read(&to, "to")) {
            (Ok((from, a)), Ok((to, b))) => {
                let (Some(a), Some(b)) = (a.document(), b.document()) else {
                    return Err(Problems::from(
                        Error::new(ErrorCode::Internal, "a side read as a document is gone")
                            .with_suggestion(crate::worker::SILICO_BUG),
                    ));
                };
                // Stopped between features once cancelled, answering `cancelled`.
                let go = || {
                    #[cfg(test)]
                    self.gate.pass();
                    if stop.cancelled() {
                        return Err(cancelled(stop.name()));
                    }
                    Ok(())
                };
                Ok(DiffAnswer {
                    from,
                    to,
                    changes: document::diff_until(a, b, go)?,
                })
            }
            (a, b) => Err(Problems::of(
                [a.err(), b.err()].into_iter().flatten().collect(),
            )),
        }
    }
}
