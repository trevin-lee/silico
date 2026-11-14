//! The revisions a session holds the text of, and `diff`, which reads the changes between
//! two of them (Fable's decision of 2026-11-04, *the three core gaps*, sections 1 and 2): a
//! session holds a few revisions' texts (its current one; the one its last reload replaced;
//! inside a transaction, `base`), and a command may name any of them. One resolver,
//! [`Held::text_at`], answers for each command that names one. And the session's file, as
//! the command's own read of it found it (`"disk"`: Fable's decision of 2026-11-04, *6d's
//! other half*), never held: [`Session::side`] resolves either.

use std::path::Path;
use std::sync::Arc;

use super::cancel::{Ticket, cancelled};
use super::transaction::{Held, Outside, Reload, part_written, too_long};
use super::{Gated, Problems, Revision, Session, SessionId};
use crate::command::wire::Named;
use crate::document::{self, Change, Document, FeatureId};
use crate::file::shown_path;
use crate::kernel::Kernel;
use crate::{Error, ErrorCode};

/// A text a read names, as the session resolved it: a revision's it holds, or its file's as
/// the command read it; and, where the session keeps it, the reader's and the gate's finding
/// of it (the current revision's and the transaction's base's), so it isn't read again.
pub(super) struct HeldText {
    /// The revision, or `"disk"`.
    pub(super) named: Named,
    /// Whether it's the session's revision now.
    pub(super) current: bool,
    pub(super) text: Arc<Vec<u8>>,
    pub(super) gated: Option<Arc<Gated>>,
}

/// What `diff` answers: the two sides compared, as the session resolved them (the request's
/// `null` as its revision then; `"disk"` as itself), and each change from the first to the
/// second ([`document::diff`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffAnswer {
    pub from: Named,
    pub to: Named,
    pub changes: Vec<Change>,
}

/// The cause of a refusal of a `"disk"` side whose file a transaction's read couldn't take.
const DISK_READ: &str =
    "\"disk\" is the session's file as this command reads it, inside a transaction too";

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
            named: Named::Revision(self.revision),
            current: true,
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
                named: Named::Revision(asked),
                current: false,
                text,
                gated: Some(gated),
            });
        }
        if let Some((replaced, text)) = &self.reloaded_over
            && *replaced == asked
        {
            return Ok(HeldText {
                named: Named::Revision(asked),
                current: false,
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

/// The refusal of `held`'s text as text: it isn't UTF-8, the first `valid` bytes are. For
/// the session's revision now, the file is to be mended; for an older one, the file was,
/// so the refusal names `revision`, whose text can't be given as text; for the file on
/// disk, it names `revision` too, the file to be mended.
fn not_utf8(held: &HeldText, valid: usize) -> Error {
    if held.current {
        return Error::new(
            ErrorCode::InvalidInput,
            format!(
                "the document's text isn't UTF-8 (at byte {valid}), so it can't be given as text"
            ),
        )
        .with_suggestion("mend the file: a document is UTF-8 text");
    }
    let n = match held.named {
        Named::Revision(revision) => revision.number(),
        Named::Disk => {
            return Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "the file on disk isn't UTF-8 (at byte {valid}), so it can't be given as text"
                ),
            )
            .with_parameter("revision")
            .with_suggestion(
                "mend the file: a document is UTF-8 text; or give revision null for the document now",
            );
        }
    };
    Error::new(
        ErrorCode::InvalidInput,
        format!("revision {n}'s text isn't UTF-8 (at byte {valid}), so it can't be given as text"),
    )
    .with_parameter("revision")
    .with_cause(format!(
        "revision {n}'s text is the bytes the session held then, and they aren't UTF-8"
    ))
    .with_suggestion(
        "give revision null for the document now, or another revision the session holds",
    )
}

/// The refusal of feature `id`, which the document `named` names, not the session's now,
/// hasn't (an older revision's, or the file's on disk): naming `feature`, and pointing to
/// the document now.
fn no_feature_at(named: Named, id: &FeatureId) -> Error {
    let document = match named {
        Named::Revision(revision) => format!("revision {}'s document", revision.number()),
        Named::Disk => "the file on disk".to_owned(),
    };
    Error::new(
        ErrorCode::InvalidInput,
        format!("{document} has no feature {id}"),
    )
    .with_parameter("feature")
    .with_suggestion(format!(
        "name a feature {document} has, or give revision null for the document now"
    ))
}

/// The document `held` holds, read; or the refusal of a text that doesn't read as one,
/// naming the request's `parameter`, with the reader's message as its cause.
fn read_side(held: &HeldText, parameter: &'static str) -> Result<SideDocument, Error> {
    let refused = |message: &str| {
        Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{parameter}, {}, doesn't read as a document, so it can't be compared",
                held.named
            ),
        )
        .with_parameter(parameter)
        .with_cause(message.to_owned())
        .with_suggestion(match held.named {
            Named::Disk => {
                format!("mend the file on disk, or give {parameter} a revision the session holds")
            }
            Named::Revision(_) => format!(
                "give {parameter} a revision whose text reads as a document; text gives a revision's text"
            ),
        })
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

/// The refusal of a `"disk"` side, `parameter`, in session `id`, which is untitled: it has
/// no file until its first save, and the empty document it holds stands in for none. Its
/// first save is a commit given a path, so outside a transaction, where `commit` is refused,
/// one is begun first, as [`untitled_save`](super::transaction::untitled_save) says.
fn untitled_disk(id: SessionId, parameter: &str, transaction: bool) -> Error {
    let save = if transaction {
        "commit it given a path, which saves it (Save As)"
    } else {
        "begin a transaction first; its commit, given a path, saves it (Save As)"
    };
    Error::new(
        ErrorCode::InvalidInput,
        format!("session {id} is untitled: it has no file on disk to read"),
    )
    .with_parameter(parameter)
    .with_cause("the session is untitled: nothing is on disk until its first save")
    .with_suggestion(format!(
        "{save}; until then, give {parameter} a revision the session holds, or null for the document now"
    ))
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

    /// The text `named` names, the request's `parameter`: a revision the session holds, or
    /// its revision now for `None` ([`text_at`](Self::text_at)); or, for `"disk"`, the
    /// session's file as `reload`, the command's own read of it, found it
    /// ([`on_disk`](Self::on_disk)).
    pub(super) fn side(
        &self,
        named: Option<Named>,
        parameter: &'static str,
        reload: &Reload,
    ) -> Result<HeldText, Error> {
        match named {
            None => self.text_at(None, parameter),
            Some(Named::Revision(revision)) => self.text_at(Some(revision), parameter),
            Some(Named::Disk) => self.on_disk(reload, parameter),
        }
    }

    /// The session's file, as `reload`, the read every command needing it makes before it
    /// runs ([`Session::reload`], as `commit` reads it), found it, never held: inside a
    /// transaction, other bytes than the session last read or wrote, those bytes; otherwise
    /// (found as the session holds it, or reloaded outside a transaction) the bytes the
    /// session holds as the file's, which the read proved it holds. Refused, naming
    /// `parameter`, the request's field, for a file gone, unreadable, or longer than Silico
    /// reads of a document (each refused by the read itself outside a transaction); for an
    /// untitled session, which has no file; and for a file this session's own save left
    /// part written, as its next save would be.
    fn on_disk(&self, reload: &Reload, parameter: &'static str) -> Result<HeldText, Error> {
        let disk = |text| HeldText {
            named: Named::Disk,
            current: false,
            text,
            gated: None,
        };
        let held = self.held.read();
        let Some(file) = &held.file else {
            return Err(untitled_disk(
                self.id,
                parameter,
                held.transaction.is_some(),
            ));
        };
        let shown = shown_path(Path::new(&file.path));
        match reload {
            Reload::Outside(Outside::Changed(bytes)) => Ok(disk(Arc::clone(bytes))),
            Reload::Outside(Outside::TooLong) => Err(too_long(Path::new(&file.path))
                .with_parameter(parameter)
                .with_cause(DISK_READ)),
            Reload::Outside(Outside::Gone(error)) => Err(error
                .clone()
                .with_parameter(parameter)
                .with_cause(DISK_READ)
                .with_suggestion(format!(
                    "put the file back at {shown}, then read \"disk\" again"
                ))),
            Reload::Outside(Outside::Unreadable(error)) => Err(error
                .clone()
                .with_parameter(parameter)
                .with_cause(DISK_READ)
                .with_suggestion(format!(
                    "make the file at {shown} a regular file this user may read, then read \"disk\" again"
                ))),
            // `Unsettled` is the disk watch's own reload's (`Rereads::Settled`), never a
            // command's, so never a read's that names the file.
            Reload::AsHeld | Reload::Reloaded | Reload::Unsettled => match &file.part_written {
                // The file isn't what the session holds, and the session knows it: never its
                // held bytes for it.
                Some(old) => Err(part_written(&file.path, old).with_parameter(parameter)),
                None => Ok(disk(Arc::clone(&held.bytes))),
            },
        }
    }

    /// `text`'s work: the text `revision` names (`None` for the session's revision now; a
    /// revision the session holds; or `"disk"`, the file as `reload` found it), or, given a
    /// `feature`, that feature's canonical text as that text's document has it
    /// ([`document::write_feature`]). Refused for a revision not held (naming `revision`), a
    /// `"disk"` the file can't give ([`on_disk`](Self::on_disk)), a text that isn't UTF-8
    /// (naming `revision` too, for one not the session's now), and, for a feature, a text
    /// that doesn't read as a document or hasn't one feature of that ID (saying which
    /// revision's, or the file's, for one not the session's now).
    pub(super) fn text(
        &self,
        feature: Option<FeatureId>,
        revision: Option<Named>,
        reload: &Reload,
    ) -> Result<String, Problems> {
        let held = self.side(revision, "revision", reload)?;
        match feature {
            None => String::from_utf8(held.text.to_vec())
                .map_err(|e| Problems::from(not_utf8(&held, e.utf8_error().valid_up_to()))),
            Some(id) => {
                // The gate's finding where the session keeps it; the text read again where
                // it doesn't (the one a reload replaced, the file's), on each call: the
                // session keeps only those bytes, not a document read from them, so each
                // `text` of one of its features reads them again. No client needs that text
                // a feature at a time: the whole text always fits one answer
                // (`MAX_RESPONSE_LEN` is more than twice `MAX_DOCUMENT_LEN`), so `text` with
                // no `feature` gives it once.
                let read;
                let document = match &held.gated {
                    Some(gated) => gated.document().map_err(Clone::clone)?,
                    None => {
                        read = document::read(&held.text)?;
                        &read
                    }
                };
                if !held.current && !document.features.iter().any(|f| f.id == id) {
                    return Err(Problems::from(no_feature_at(held.named, &id)));
                }
                Ok(document::write_feature(document.feature(&id)?)?)
            }
        }
    }

    /// `diff`'s work: the changes from the text `from` names to the one `to` names (`None`
    /// for the current revision; a revision the session holds; or `"disk"`, the file as
    /// `reload` found it, both `"disk"` sides the same bytes), each read as a document.
    /// Refused with every reason: a side not held, a `"disk"` the file can't give, or a side
    /// that doesn't read; and stopped, `cancelled`, before the next feature is compared once
    /// `stop` is cancelled.
    pub(super) fn diff(
        &self,
        stop: &Ticket,
        from: Option<Named>,
        to: Option<Named>,
        reload: &Reload,
    ) -> Result<DiffAnswer, Problems> {
        let shown = |side: Option<Named>| side.map_or_else(|| "null".to_owned(), |s| s.to_string());
        let _span = tracing::info_span!("diff", from = %shown(from), to = %shown(to)).entered();
        // Only this session's thread changes what's held, and this command runs on it: both
        // sides are of one state, each taken under the lock, read with it let go.
        let from = self.side(from, "from", reload);
        let to = self.side(to, "to", reload);
        let read = |side: &Result<HeldText, Error>, parameter| match side {
            Ok(held) => read_side(held, parameter).map(|doc| (held.named, doc)),
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
