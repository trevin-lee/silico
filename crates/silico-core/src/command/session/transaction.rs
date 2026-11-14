//! What a session holds ([`Held`]) and how it changes in transactions: `begin`, `rollback`,
//! `commit`, and `format`, each save written through [`file::save_over`], what a save that
//! finds the file changed says ([`Saver`]), and the reload of a file changed outside the
//! session before a command (decision 2).
//!
//! The order its locks are taken in is on [`Session`].

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use super::cancel::Ticket;
use super::rebuilds::{Kept, Rebuilds};
use super::registry::Unclaimed;
#[cfg(doc)]
use super::registry::claim;
use super::{
    Begun, Committed, Gated, ModelChoice, ModelRead, Problems, Published, Reformatted, Revision,
    RolledBack, Session, SessionId, Transaction, digest, shown_path,
};
use crate::command::edit::Candidate;
use crate::command::wire::Named;
use crate::document::json::MAX_INPUT_LEN;
use crate::document::undo::{After, Pushed, UndoStack};
use crate::document::{self, Document, FeatureId};
use crate::file::{self, Workspace};
use crate::kernel::Kernel;
use crate::recompute::{self, Model};
use crate::timeline::Excepted;
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// What a session holds (docs/design/m4.1-command-api.md, section 3). Its values are shared
/// with the commands reading them, so a command takes them under the lock at once and works
/// off it, and `status` never waits on a command's work. Changed only as `holding` allows:
/// by a command once its stop is settled, or by the session's own reload.
pub(super) struct Held {
    /// The document's file: none while the session is untitled (opened with no path), until
    /// its first save (a Save As) gives it one, which it holds from then on.
    pub(super) file: Option<HeldFile>,
    /// The file's bytes as the session last read or wrote them: what a save checks the file
    /// still holds, so one changed outside the session isn't written over. An untitled
    /// session's are the empty document's, as `open` with `create` would have written them.
    pub(super) bytes: Arc<Vec<u8>>,
    /// The document's text: the file's bytes, until an edit.
    pub(super) text: Arc<Vec<u8>>,
    /// The text as the reader and the gate found it.
    pub(super) gated: Arc<Gated>,
    pub(super) revision: Revision,
    /// The features edits reached before the last save whose records it couldn't write (a
    /// feature that failed): still reached, until a save writes them.
    pub(super) carried: Arc<BTreeSet<FeatureId>>,
    /// The transaction open, if one is: what `begin` kept, and its undo stack.
    pub(super) transaction: Option<Open>,
    /// The newest revision a reload of the file gave (decision 2): an edit made against a
    /// revision before it is stale because the file changed, which its refusal says.
    pub(super) last_reload: Option<Revision>,
    /// The revision the session's last reload replaced, and its text (Fable's decision of
    /// 2026-11-04, section 1): what `diff` compares the reloaded document with, so a client
    /// can say what changed outside the session. The text's `Arc` shared, never copied, and
    /// dropped by the next reload, which keeps the one it replaces.
    pub(super) reloaded_over: Option<(Revision, Arc<Vec<u8>>)>,
}

/// A session's file ([`Held::file`]): its path and where it leads, both there or neither.
pub(super) struct HeldFile {
    /// The document's path as `open` was given it (or, after a Save As, the commit).
    pub(super) path: String,
    /// The path as its links resolve, where saves and reloads go.
    pub(super) resolved: PathBuf,
    /// Where the file's old text is, if this session's own save left the file part
    /// written and couldn't put it back: the next save says so, rather than take the file
    /// for one changed outside the session; until the file holds what the session last read
    /// or wrote again, the old text put back ([`Session::reload`]).
    pub(super) part_written: Option<PathBuf>,
}

/// A transaction open in a session: what `begin` kept, and its undo stack (docs/design/
/// m5.0-app.md, section 3's A2), every state its edits left, from the one `begin` saw, for
/// `undo` and `redo`. Made by `begin` and ended whole ([`end_transaction`]), so the stack is
/// there exactly while the transaction is.
pub(super) struct Open {
    pub(super) begun: Snapshot,
    /// Each state holding what `format` had put into the document then ([`Formatted`]), so
    /// an undo or a redo of a `format` takes it back or gives it back with the text, and the
    /// stack drops and counts it with the state.
    pub(super) undo: UndoStack<Revision, Arc<Formatted>>,
    /// Whether the session's document differs from the one `begin` saw, as a commit finds
    /// it ([`Snapshot::differs`]): worked out off the lock as each edit is held and each
    /// undo or redo restores a state, never by `status` (the design's section 6).
    pub(super) changed: bool,
}

impl Open {
    /// The transaction as `status` gives it: its base, whether its document changed, and
    /// its undo stack's counts, each kept as the stack changes, so none is worked out here.
    pub(super) fn status(&self) -> Transaction {
        Transaction {
            base: self.begun.revision,
            changed: self.changed,
            undo: self.undo.undo_left(),
            redo: self.undo.redo_left(),
            dropped: self.undo.dropped(),
            bytes: self.undo.bytes(),
        }
    }
}

/// A transaction's mend ([`Snapshot::mend`]): the first state of its history that read as a
/// document, `begin`'s text having not, and the revision that state was made at.
#[derive(Clone)]
pub(super) struct Mend {
    revision: Revision,
    gated: Arc<Gated>,
}

/// A session's state as `begin` kept it, which `rollback` restores (its models are kept with
/// the session's rebuilds, [`Kept`]).
#[derive(Clone)]
pub(super) struct Snapshot {
    text: Arc<Vec<u8>>,
    gated: Arc<Gated>,
    pub(super) revision: Revision,
    carried: Arc<BTreeSet<FeatureId>>,
    /// If its text didn't read as a document, the IDs it holds, found once, the first time
    /// what the transaction's edits reach is worked out ([`Reached::get`]): off the caller's
    /// thread and outside every lock, so a long text's scan never keeps `status` or a
    /// published read waiting.
    quoted: Arc<OnceLock<document::QuotedIds>>,
    /// If its text didn't read as a document, the first state of the transaction's history
    /// that read as one: its mend, which the edits after it are compared with
    /// ([`Reaching::reach`]). Kept through undo and redo (a redo restores it) and when the
    /// stack's bound drops its state (it's still the history the edits standing came from),
    /// but given up when an edit after an undo drops its state from the redo: then the next
    /// state that reads is the mend. Not part of what `rollback` restores.
    mend: Option<Mend>,
    /// What `format` put into the transaction's document (M5.0's A6), which its commit
    /// saves as `format` saves outside one ([`Formatted`]).
    formatted: Arc<Formatted>,
}

/// What `format` put into a transaction's document: each feature it wrote another value
/// for (a sketch's solve), as it wrote it, which reaches nothing while the feature still
/// holds it, as `format` reaches nothing outside a transaction; and whether it wrote at all,
/// after which a commit compares the document's bytes, so a file not in canonical form is
/// written in it, as `format` writes it outside one.
#[derive(Clone, Default)]
pub(super) struct Formatted {
    written: BTreeMap<FeatureId, document::Feature>,
    layout: bool,
    /// What it counts for on the undo stack ([`Formatted::cost`]), worked out as it's made.
    bytes: usize,
}

/// What a [`Formatted`] counts for on the undo stack beyond its features: its own fields.
const FORMATTED_BYTES: usize = 32;

impl Formatted {
    /// What it counts for: each feature it wrote, as its canonical text's bytes, and
    /// [`FORMATTED_BYTES`].
    fn cost(&self) -> usize {
        FORMATTED_BYTES
            + self
                .written
                .values()
                .map(|feature| {
                    document::write::feature_text(feature, usize::MAX).map_or(0, |text| text.len())
                })
                .sum::<usize>()
    }
}

impl crate::document::undo::Extra for Arc<Formatted> {
    /// Shared by every state an edit after the `format` made (each takes the one before's):
    /// counted once. One that holds nothing (no `format` yet: a `format` sets `layout` as it
    /// writes, so `written` is never held without it) costs nothing.
    fn shared(&self) -> Option<usize> {
        self.layout.then_some(Arc::as_ptr(self) as usize)
    }

    fn cost(&self) -> usize {
        self.bytes
    }
}

impl Snapshot {
    /// Its text, and the reader's and the gate's finding of it: what `diff` and `text` read
    /// of the transaction's base.
    pub(super) fn held(&self) -> (Arc<Vec<u8>>, Arc<Gated>) {
        (Arc::clone(&self.text), Arc::clone(&self.gated))
    }

    /// What `format` has put into the transaction's document: `formatted` (an undo or a
    /// redo restoring a state's).
    pub(super) fn set_formatted(&mut self, formatted: Arc<Formatted>) {
        self.formatted = formatted;
    }

    /// Whether the document `text` holds, as the reader and the gate found it (`gated`),
    /// differs from this one, `begin`'s, as a commit decides whether it writes: compared as
    /// canonical form writes them, so a document read from a file whose sketch lists aren't
    /// in ID order is the one its canonical text reads as; byte for byte where either text
    /// doesn't read as a document, or once a `format` in the transaction has asked for
    /// canonical form.
    pub(super) fn differs(&self, text: &[u8], gated: &Gated) -> bool {
        self.differs_after(text, gated, false)
    }

    /// [`differs`](Self::differs), as it will compare once an edit that `formats` (a
    /// `format` inside the transaction, which asks for canonical form) is held: by bytes
    /// from then on, as after one already.
    pub(super) fn differs_after(&self, text: &[u8], gated: &Gated, formats: bool) -> bool {
        match (self.gated.document(), gated.document()) {
            (Ok(before), Ok(now)) if !(self.formatted.layout || formats) => {
                !before.canonically_eq(now)
            }
            _ => self.text.as_slice() != text,
        }
    }
}

/// A state of the session's document a commit saves ([`Session::save_state`]): the path its
/// result names (as `open` was given it; for an untitled session's Save As, as the commit
/// gives it), the text and the gate's finding of it, the revision whose model is
/// its model, what was carried before, the state the edits were made from, and how the file
/// was found before the command, if not as the session last read or wrote it (inside a
/// transaction, [`Reload::Outside`]).
struct Saving {
    path: String,
    text: Arc<Vec<u8>>,
    gated: Arc<Gated>,
    revision: Revision,
    carried: Arc<BTreeSet<FeatureId>>,
    begun: Snapshot,
    changed_outside: Option<Outside>,
}

/// What a save writes ([`Session::write`]): its text, the model it was formatted from, what
/// the edits it saves reach, and whether that model, rebound, is the saved revision's too
/// (the save wrote no sketch's solve).
struct Save<'a> {
    text: Arc<Vec<u8>>,
    model: &'a Model,
    reach: &'a BTreeSet<FeatureId>,
    publish: bool,
}

/// Where a Save As writes (M5.4, `commit`'s `path`; docs/design/m5.0-app.md, open item
/// 22): the path as the request gave it (in a workspace, relative to its root), whether a
/// file there may be replaced, the documents the other sessions hold (by their paths),
/// which session holds a file, looked up (`registry::holder`), and the claim on the path
/// the session takes before it writes and gives back if it writes nothing
/// (`registry::reserve`, `registry::unreserve`).
pub(super) struct SaveTo<'a> {
    pub(super) given: String,
    pub(super) replace: bool,
    pub(super) held: Vec<PathBuf>,
    pub(super) holder: &'a dyn Fn(&Path) -> Option<SessionId>,
    pub(super) reserve: &'a dyn Fn(&Path) -> Result<(), SessionId>,
    pub(super) unreserve: &'a dyn Fn(&Path),
}

impl SaveTo<'_> {
    /// The rules its path is held to (decision 35, as a document): ending in `.silico`,
    /// inside `workspace`, never through a link out of it, never in a `.silico-cache/`
    /// directory, a file if anything's there, and replaced only as `replace` says; and, as
    /// it's written, never another session's document (before, [`target`](Self::target)
    /// names the session).
    fn rules<'w>(
        &'w self,
        workspace: Option<&'w Workspace>,
        replace: bool,
        writing: bool,
    ) -> file::Output<'w> {
        file::Output {
            field: "path",
            extensions: &["silico"],
            writer: "a commit",
            replace,
            workspace,
            held: if writing { &self.held } else { &[] },
            document: true,
        }
    }

    /// Where it writes, through any links, or why it's refused, writing nothing: as
    /// [`rules`](Self::rules) refuse it, if it's the session's own file, `own` (by any path
    /// to it), which a commit with no path saves (none for an untitled session, which has
    /// none), or if another session holds it.
    fn target(&self, workspace: Option<&Workspace>, own: Option<&Path>) -> Result<PathBuf, Error> {
        let given = Path::new(&self.given);
        // Whether it's the session's own file is said before whether it's there.
        let target = file::check_output(given, &self.rules(workspace, true, false))?;
        let same = |a: &std::fs::Metadata, b: &std::fs::Metadata| {
            use std::os::unix::fs::MetadataExt as _;
            a.dev() == b.dev() && a.ino() == b.ino()
        };
        let there = std::fs::metadata(&target);
        if let Some(own) = own
            && (target == own
                || there
                    .as_ref()
                    .is_ok_and(|t| std::fs::metadata(own).is_ok_and(|o| same(t, &o))))
        {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!("{} is this session's own file", shown_path(given)),
            )
            .with_parameter("path")
            .with_suggestion(file::OWN_FILE_ADVICE));
        }
        if let Some(holder) = (self.holder)(&target) {
            return Err(held_elsewhere(&self.given, holder));
        }
        if there.is_ok() && !self.replace {
            return Err(file::exists(given, "path"));
        }
        Ok(target)
    }
}

/// The problem a Save As gives of the session's old file at `path` (as `open` was given
/// it), left part written by an earlier save that couldn't put its text back, its text
/// before in `old`: the session holds another file now, so nothing will say it again.
fn left_part_written(path: &str, old: &Path) -> Error {
    Error::new(
        ErrorCode::IoFailed,
        format!(
            "{} was left part written by this session's own save, which couldn't put its text back: its text before is in {}",
            shown_path(Path::new(path)),
            shown_path(old)
        ),
    )
    .with_parameter("path")
    .with_suggestion(
        "the session holds the new file now: put the old text back in the old file by hand if it's still wanted",
    )
}

/// The refusal of a Save As to `given`, a file session `holder` holds or is opening.
fn held_elsewhere(given: &str, holder: SessionId) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!(
            "{} is open in session {holder}",
            shown_path(Path::new(given))
        ),
    )
    .with_parameter("path")
    .with_suggestion(format!(
        "give another path, or close session {holder} first"
    ))
}

/// The refusal of a `commit` with no path in session `id`, which is untitled (opened with
/// no path): it has no file to save to, so it's given one (a Save As).
fn untitled_commit(id: SessionId) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("session {id} is untitled: it has no file to save to"),
    )
    .with_parameter("path")
    .with_suggestion("give path: where to save it (Save As)")
}

/// The refusal of what an untitled session `id` would save outside a transaction (an edit,
/// which is saved as it's made, decision 1; `format` writing; a drag, whose end is such an
/// edit, refused as it begins): it has no file to save to.
pub(super) fn untitled_save(id: SessionId) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("session {id} is untitled: it has no file to save to"),
    )
    .with_parameter("session")
    .with_cause("outside a transaction an edit is saved as it's made")
    .with_suggestion(
        "begin a transaction first; its commit, given a path, saves every edit it holds (Save As)",
    )
}

/// The refusal of `cache_clear` in session `id`, which is untitled: it keeps no rebuild
/// cache, having no file for one to be beside.
pub(super) fn untitled_cache(id: SessionId) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("session {id} is untitled: it has no file, so no rebuild cache beside one"),
    )
    .with_parameter("session")
    .with_suggestion("save it first (commit with a path), then clear the cache beside the file")
}

/// Whose state a commit saves: an open transaction's, or an edit's candidate, made outside
/// one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Through {
    Transaction,
    Candidate,
}

impl Held {
    /// The revision the session's next change gives, or why there's none.
    pub(super) fn next(&self) -> Result<Revision, Problems> {
        self.revision.next().ok_or_else(|| {
            Problems::from(
                Error::new(
                    ErrorCode::ResourceExhausted,
                    "the session has no revisions left to give",
                )
                .with_suggestion("close the session, then open the document again"),
            )
        })
    }

    /// The session's document changed to `text`, as the reader and the gate found it
    /// (`gated`), at `revision` (from [`next`](Self::next)), carrying `carried`: the one
    /// place a session's document changes (an edit, a save, a rollback, a reload). Inside a
    /// transaction begun from text that didn't read, the first document that reads is kept
    /// as its mend.
    pub(super) fn change(
        &mut self,
        revision: Revision,
        text: Arc<Vec<u8>>,
        gated: Arc<Gated>,
        carried: Arc<BTreeSet<FeatureId>>,
    ) {
        if let Some(Open { begun, .. }) = &mut self.transaction
            && begun.mend.is_none()
            && begun.gated.document().is_err()
            && gated.document().is_ok()
        {
            begun.mend = Some(Mend {
                revision,
                gated: Arc::clone(&gated),
            });
        }
        self.revision = revision;
        self.text = text;
        self.gated = gated;
        self.carried = carried;
    }

    /// The state held now, as `begin` keeps it.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: Arc::clone(&self.text),
            gated: Arc::clone(&self.gated),
            revision: self.revision,
            carried: Arc::clone(&self.carried),
            quoted: Arc::new(OnceLock::new()),
            mend: None,
            formatted: Arc::default(),
        }
    }

    /// The document an open transaction began from, as its features' stored records are
    /// read from it: `begin`'s, or, if its text didn't read as a document, the transaction's
    /// mend. None outside a transaction, or with neither read.
    pub(super) fn begun(&self) -> Option<Arc<Gated>> {
        let begun = &self.transaction.as_ref()?.begun;
        if begun.gated.document().is_ok() {
            return Some(Arc::clone(&begun.gated));
        }
        begun
            .mend
            .as_ref()
            .filter(|mend| mend.gated.document().is_ok())
            .map(|mend| Arc::clone(&mend.gated))
    }

    /// What the session's edits reach now, taken under the lock, to be worked out
    /// ([`Reaching::reach`]) once it's let go.
    pub(super) fn reaching(&self) -> Reaching {
        Reaching {
            gated: Arc::clone(&self.gated),
            carried: Arc::clone(&self.carried),
            begun: self.transaction.as_ref().map(|open| open.begun.clone()),
        }
    }

    /// An edit's `candidate` held inside the transaction open, at `revision`, as the
    /// transaction keeps it: pushed on its undo stack, which drops the redo states, by its
    /// features' canonical texts, which its text was assembled from ([`After::Canonical`]),
    /// so an undo writes it back byte for byte; and, if the push drops the mend's state, the
    /// mend given up first, so the edit's state, if it reads, is the mend from now on
    /// ([`Snapshot::mend`]). `changed` is whether its document differs from `begin`'s
    /// ([`Snapshot::differs`], worked out before the lock was taken). Then the session's
    /// document changed to it, as [`change`](Self::change) changes it, carrying what was
    /// carried. What the push said.
    pub(super) fn hold(
        &mut self,
        revision: Revision,
        candidate: Candidate,
        changed: bool,
    ) -> Option<Pushed> {
        let (text, gated, prepared) = candidate.into_parts();
        let after = After::Canonical(prepared);
        let pushed = self.transaction.as_mut().map(|open| {
            if open
                .begun
                .mend
                .as_ref()
                .is_some_and(|mend| open.undo.redo_holds(mend.revision))
            {
                open.begun.mend = None;
            }
            open.changed = changed;
            open.undo.edit(after, revision)
        });
        let carried = Arc::clone(&self.carried);
        self.change(revision, text, gated, carried);
        pushed
    }
}

/// What a session's edits reach at a revision, as its held state was then: its document,
/// what was carried, and the transaction's snapshot, if one was open. Cheap to take (shared
/// values), and worked out apart from taking it ([`reach`](Self::reach)), which may scan a
/// long text.
pub(super) struct Reaching {
    gated: Arc<Gated>,
    carried: Arc<BTreeSet<FeatureId>>,
    begun: Option<Snapshot>,
}

#[cfg(test)]
thread_local! {
    /// How many times this thread has worked out what a session's edits reach
    /// ([`Reaching::reach`]): so a test can say a caller's thread never does.
    pub(super) static REACHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Reaching {
    /// What the session's edits reach (the design's section 5, decision 28), net: the
    /// features they changed since the transaction began, compared by ID with the document
    /// `begin` saw (or, if that didn't read, those of its mend with IDs its text hasn't,
    /// and those changed since the mend), with those carried from before, and every feature
    /// that uses one (`document::net_reach`). An edit undone reaches nothing. Those a commit
    /// writes new records for, and whose drift a check doesn't say. Outside a transaction,
    /// what's carried.
    pub(super) fn reach(&self) -> BTreeSet<FeatureId> {
        #[cfg(test)]
        REACHES.with(|reaches| reaches.set(reaches.get() + 1));
        let Ok(now) = self.gated.document() else {
            return (*self.carried).clone();
        };
        let Some(begun) = &self.begun else {
            return document::with_users(now, &self.carried);
        };
        let unread = || {
            begun
                .quoted
                .get_or_init(|| document::QuotedIds::of(&begun.text))
        };
        let mend = begun.mend.as_ref().map(|mend| mend.gated.document());
        let before = match (begun.gated.document(), mend) {
            (Ok(before), _) => document::Before::Read(before),
            (Err(_), Some(Ok(mend))) => document::Before::Mended {
                unread: unread(),
                mend,
            },
            (Err(_), _) => document::Before::Unread(unread()),
        };
        document::net_reach_but(before, &self.carried, now, &begun.formatted.written)
    }
}

/// What a session's edits reach at a revision ([`Reaching::reach`]), kept with its rebuild
/// and the model that publishes, and worked out once, the first time it's asked for
/// ([`get`](Self::get)), by whoever asks: a check, on its session's thread or a published
/// read's own. Never by a caller that only takes it, nor under a lock, so a published read
/// (a UI's, each frame) never waits on a long text's scan. Until then it holds what it's
/// worked out from, the values the session held at that revision.
#[derive(Clone, Default)]
pub(crate) struct Reached(Arc<OnceReached>);

#[derive(Default)]
struct OnceReached {
    /// What it's worked out from: none for an edit's candidate, never published as it is,
    /// which reaches nothing.
    reaching: Option<Reaching>,
    reach: OnceLock<BTreeSet<FeatureId>>,
}

impl Reached {
    /// What the session's edits reach, as `reaching` was taken, not worked out yet.
    pub(super) fn of(reaching: Reaching) -> Self {
        Reached(Arc::new(OnceReached {
            reaching: Some(reaching),
            reach: OnceLock::new(),
        }))
    }

    /// The features reached, worked out now if they haven't been.
    pub(crate) fn get(&self) -> &BTreeSet<FeatureId> {
        let once = &*self.0;
        once.reach.get_or_init(|| {
            once.reaching
                .as_ref()
                .map_or_else(BTreeSet::new, Reaching::reach)
        })
    }

    /// The features whose records only a commit writes, as `checking` says the session is
    /// as it's checked: those reached ([`get`](Self::get)) where a transaction's commit
    /// writes them, and `format`, refused in a transaction, can't; none otherwise, where
    /// what's reached is only what's carried from a save that couldn't write it, which a
    /// `format` writes. Those whose missing fingerprint a check doesn't warn of
    /// ([`crate::warnings`]).
    ///
    /// For a model of the session's revision ([`Checking::Latest`]), exactly when a
    /// transaction is open now: not when the revision was, since a model published before a
    /// `begin` is the revision's still, and so is one relabelled by a commit that wrote
    /// nothing (`unwritten`). For an older model ([`Checking::Older`]), also when
    /// its own revision was a transaction's. With one open now, a `begin` made while its
    /// rebuild ran left it. With none, a commit that wrote the file without publishing (its
    /// sketches' solves written back), which wrote every built feature's record; or one that
    /// wrote nothing and left the text byte for byte as it was, keeping the revision, which a
    /// later reload, `format` or edit's commit leaves older until its own model is published.
    /// Then what's spared is only what was carried from an earlier save (the edits came back
    /// to `begin`'s document, so they reach nothing), whose warning is missed for that while.
    /// A rollback restores the model `begin` kept. A `format` or an edit's commit that wrote
    /// a sketch's solve, and so didn't publish, leaves an older model too: what it stored,
    /// the session's document stores now. So for an older model, [`Checking::spared`] asks
    /// this only when the session's document doesn't read: one that does says what the file
    /// stores, and the miss above isn't made.
    fn committing(&self, checking: &Checking) -> &BTreeSet<FeatureId> {
        static NONE: BTreeSet<FeatureId> = BTreeSet::new();
        let spared = match *checking {
            Checking::Latest { open } => open,
            Checking::Older { open, .. } => open || self.of_transaction(),
        };
        if spared { self.get() } else { &NONE }
    }

    /// Whether the revision this is of was a transaction's: what it's worked out from held
    /// the transaction's snapshot.
    fn of_transaction(&self) -> bool {
        self.0
            .reaching
            .as_ref()
            .is_some_and(|reaching| reaching.begun.is_some())
    }
}

/// The session as a check of a model finds it, taken under its lock with its revision: what
/// decides which missing fingerprints the check doesn't warn of ([`spared`](Self::spared)).
#[derive(Clone)]
pub(super) enum Checking {
    /// The model is of the session's revision; `open`, whether a transaction is open.
    Latest { open: bool },
    /// The model is an older revision's, the newest published; `open`, whether a
    /// transaction is open in the session now, `now`, the session's document now, as the
    /// reader and the gate found it, and `at`, the session's revision, whose document it is.
    Older {
        open: bool,
        now: Arc<Gated>,
        at: Revision,
    },
}

impl Checking {
    /// A check of the model of revision `model`, in a session at revision `now`, whose
    /// document is `gated`, a transaction open in it or not (`open`), all as one lock held
    /// them.
    pub(super) fn of(open: bool, now: Revision, gated: &Arc<Gated>, model: Revision) -> Self {
        if model == now {
            Checking::Latest { open }
        } else {
            Checking::Older {
                open,
                now: Arc::clone(gated),
                at: now,
            }
        }
    }

    /// The session's revision and its document, as the reader and the gate found it, if the
    /// model is an older revision's; none if it's of the session's revision, whose document
    /// is the model's own.
    pub(super) fn now(&self) -> Option<(Revision, &Gated)> {
        match self {
            Checking::Latest { .. } => None,
            Checking::Older { now, at, .. } => Some((*at, now)),
        }
    }

    /// What a check of the model, whose revision's edits reached `reached`, excepts, as
    /// `check` excepts it and so `timeline` marks by it: the missing fingerprints it doesn't
    /// warn of ([`spared`](Self::spared)), and the drift it doesn't say, of the features the
    /// edits reached (decision 28: a commit writes their records anew). One place for both,
    /// so a timeline's marks never disagree with the check of the same model.
    pub(super) fn excepted(&self, model: &Model, reached: &Reached) -> Excepted {
        Excepted {
            spared: self.spared(model, reached).into_owned(),
            drift: reached.get().clone(),
        }
    }

    /// The features whose missing fingerprint a check of the model, whose revision's edits
    /// reached `reached`, doesn't warn of ([`crate::warnings`]): those a commit writes
    /// ([`Reached::committing`]); but, for an older model whose session's document reads,
    /// those an open transaction's commit would write and those the document stores a
    /// fingerprint for now (#189's L3). The warning says what the
    /// file stores, so a fingerprint a save stored since the model's revision isn't
    /// missing, though the model's own document holds none: one stored by `format`, or by
    /// an edit's commit, that wrote a sketch's solve and so didn't publish its model (the
    /// saved document's rebuild solves from what it wrote), leaving this one the newest
    /// published until the saved revision's is. Only spared, never added: a fingerprint the
    /// model's document stores and the session's no longer does is warned of once the
    /// session's revision's model is published, as everything else of that revision is. A
    /// model of the session's revision has the session's document, which stores what it
    /// does.
    ///
    /// A fingerprint stored now that lacks a measure a later Silico added (its centre,
    /// `fingerprint_incomplete`) spares a feature only where the model's document stored
    /// none: the file isn't missing one, but the model can't say it's incomplete. Where the
    /// model's document stored one too, its incomplete record is warned of, as the file's is.
    /// And a feature the file stores no fingerprint for now, where the model's document
    /// stored one, is spared: the model can't say it's missing (it stored one), and what it
    /// would say of it, an incomplete record, isn't true of the file.
    pub(super) fn spared<'a>(
        &'a self,
        model: &Model,
        reached: &'a Reached,
    ) -> Cow<'a, BTreeSet<FeatureId>> {
        let committing = reached.committing(self);
        let Checking::Older { open, now, .. } = self else {
            return Cow::Borrowed(committing);
        };
        let Ok(now) = now.document() else {
            return Cow::Borrowed(committing);
        };
        // What the file stores now decides, beside what an open transaction's commit will
        // write: not whether the model's revision was a transaction's, which stands in for
        // the file only when it doesn't read. After a commit that wrote nothing and a reload,
        // a feature carried with no fingerprint is missing one, and said so.
        let mut spared = if *open {
            reached.get().clone()
        } else {
            BTreeSet::new()
        };
        let stored_then: BTreeSet<&FeatureId> = model
            .document()
            .features
            .iter()
            .filter(|feature| feature.fingerprint.is_some())
            .map(|feature| &feature.id)
            .collect();
        spared.extend(
            now.features
                .iter()
                .filter(|feature| match &feature.fingerprint {
                    Some(stored) => stored.records_center() || !stored_then.contains(&feature.id),
                    None => stored_then.contains(&feature.id),
                })
                .map(|feature| feature.id.clone()),
        );
        Cow::Owned(spared)
    }
}

impl std::fmt::Debug for Reached {
    /// The features, if they've been worked out; never working them out to say so.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.reach.get() {
            Some(reach) => f.debug_tuple("Reached").field(reach).finish(),
            None => f.write_str("Reached(not worked out)"),
        }
    }
}

/// The transaction open in `held` ended, its undo stack emptied, and the models its `begin`
/// kept with `rebuilds` let go: what `begin` kept, if one was open. Every way a transaction
/// ends (a commit, one that writes nothing, a rollback) goes through here, with both locks
/// held, so the two halves of what `begin` kept are never apart.
fn end_transaction<C: Kernel + 'static>(
    held: &mut Held,
    rebuilds: &mut Rebuilds<C>,
) -> Option<(Snapshot, Kept)> {
    let snapshot = held.transaction.take().map(|open| open.begun);
    let kept = rebuilds.kept.take();
    snapshot.zip(kept)
}

impl<C: Kernel + 'static> Session<C> {
    /// `begin`, as the command `stop` stops: the session's state kept, and a transaction
    /// opened; refused if one is open. The newest model published is kept with it, and its
    /// revision's model if it has been published, or, if its rebuild is running, once that
    /// publishes it.
    pub(super) fn begin(&self, stop: &Ticket) -> Result<Begun, Problems> {
        let held = self.held.read();
        if held.transaction.is_some() {
            // Design section 2's words, which differ from close's and format's (section 7).
            return Err(Problems::from(
                Error::new(
                    ErrorCode::InvalidInput,
                    "a transaction is already open in this session",
                )
                .with_parameter("session")
                .with_suggestion("commit or roll back the open transaction, then begin again"),
            ));
        }
        let mut held = held.changing(stop)?;
        let revision = held.revision;
        // Under the held state's lock, then the rebuilds' (the only order both are taken
        // in), so a model published meanwhile is either seen here or kept by its watcher.
        let mut rebuilds = lock(&self.rebuilds);
        let published = rebuilds.published.clone();
        let model = published
            .as_ref()
            .filter(|published| published.revision == revision)
            .map(|published| Arc::clone(&published.model));
        rebuilds.kept = Some(Kept {
            base: revision,
            published,
            model,
            started: false,
        });
        drop(rebuilds);
        held.transaction = Some(Open {
            begun: held.snapshot(),
            undo: UndoStack::holding(Arc::clone(&held.text), revision, self.undo_bytes),
            changed: false,
        });
        Ok(Begun { base: revision })
    }

    /// The transaction open, or the refusal of a command that needs one.
    fn transaction(&self, held: &Held) -> Result<Snapshot, Problems> {
        held.transaction
            .as_ref()
            .map(|open| open.begun.clone())
            .ok_or_else(|| Problems::from(no_transaction(self.id)))
    }

    /// `rollback`: what `begin` kept restored, exactly (the text byte for byte, the gate's
    /// finding, what was carried), at a new revision whose state equals the one `begin`
    /// saw, and the transaction ended. Its models are those `begin` saw: the newest
    /// published then, and its revision's, the same one, now the new revision's. If that
    /// one hasn't come yet, its rebuild, still running, is the new revision's; if it never
    /// will (a rebuild in the transaction superseded it), the restored document's rebuild
    /// is started, which the cache makes cheap; and if no rebuild started in the
    /// transaction at all, none is. A rebuild of the transaction's still running is
    /// stopped, and none of the transaction's models is published after. The file is never
    /// touched. As the command `stop` stops.
    pub(super) fn rollback(self: &Arc<Self>, stop: &Ticket) -> Result<RolledBack, Problems> {
        let held = self.held.read();
        self.transaction(&held)?;
        let revision = held.next()?;
        let mut held = held.changing(stop)?;
        let mut rebuilds = lock(&self.rebuilds);
        let Some((begun, kept)) = end_transaction(&mut held, &mut rebuilds) else {
            return Err(Problems::from(no_transaction(self.id)));
        };
        held.change(
            revision,
            Arc::clone(&begun.text),
            Arc::clone(&begun.gated),
            Arc::clone(&begun.carried),
        );
        rebuilds.floor = revision;
        rebuilds.published = kept.published;
        if let Some(running) = &rebuilds.running
            && running.revision() == kept.base
            && kept.model.is_none()
        {
            // `begin`'s own rebuild, still running: what it makes is the new revision's.
            running.give(revision);
        } else {
            // The transaction's: no command wants it now.
            if rebuilds.running.take().is_some() {
                rebuilds.engine.cancel();
            }
            rebuilds.ended = None;
            // What the edits reach at the restored state, the transaction ended: what they
            // reached at `begin`. Taken, not worked out, under the locks.
            let reached = Reached::of(held.reaching());
            match (kept.model, &*begun.gated) {
                (Some(model), _) => {
                    rebuilds.published = Some(Published {
                        revision,
                        model,
                        reached,
                        by: None,
                    });
                }
                (None, Gated::Checked(checked)) if kept.started => {
                    if let Err(problems) =
                        self.start_unless_newer(&mut rebuilds, revision, checked, &reached)
                        && !self.closed()
                    {
                        // Started again by the next command that needs the model.
                        tracing::warn!(
                            session = %self.id,
                            "the restored document's rebuild couldn't start: {:?}",
                            problems.errors()
                        );
                    }
                }
                // Nothing was built of it, nor asked for since (decision 13), or the gate
                // refused what `begin` saw: nothing to start.
                (None, _) => {}
            }
        }
        Ok(RolledBack {
            restored: begun.revision,
        })
    }

    /// `commit`: the current revision's rebuild waited for, then the document saved in
    /// canonical form through `file::save_over`, with new fingerprints and solves for what
    /// the transaction's edits reach ([`Reaching::reach`]) and every other feature's as
    /// stored (decision 28), its new file claimed with `claim`; the transaction ended, and
    /// the session at a new revision, its saved text, with only the reached features whose
    /// records couldn't be written still carried. A document equal to the one `begin` saw
    /// is written nowhere, and the session's text is `begin`'s again (the file's as the
    /// session last read or wrote it), even if the file was found changed outside the
    /// session (`changed_outside`, before the command), which then waits for no rebuild.
    /// Refused, the transaction left open and nothing written, if none is open, the file
    /// was found changed or gone before the command (a conflict, before any rebuild) or
    /// unreadable (its `io_failed`), the model can't be had (the session closed, a
    /// recompute that failed, the commit cancelled), the canonical text is too long, or the
    /// save is (a file changed since: a conflict too). A kernel fault the model holds is
    /// saved around (decision 32): the features it stopped are saved as failed ones are (a
    /// reached one with no fingerprint, decision 28 amended; any other as stored), and it's
    /// given once, in the model read, not among the problems. As the command `stop` stops.
    pub(super) fn commit(
        self: &Arc<Self>,
        stop: &Ticket,
        changed_outside: Option<Outside>,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
    ) -> Result<Committed, Problems> {
        let state = {
            let held = self.held.read();
            let begun = self.transaction(&held)?;
            let Some(file) = &held.file else {
                return Err(Problems::from(untitled_commit(self.id)));
            };
            Saving {
                path: file.path.clone(),
                text: Arc::clone(&held.text),
                gated: Arc::clone(&held.gated),
                revision: held.revision,
                carried: Arc::clone(&held.carried),
                begun,
                changed_outside,
            }
        };
        self.save_state(stop, state, Through::Transaction, read, claim, None)
            .map(|(committed, _)| committed)
    }

    /// `commit` with a `path` (a Save As, M5.4; docs/design/m5.0-app.md, open item 22): as
    /// [`commit`](Self::commit) saves, but to the file at `to.given`, written whole through
    /// a new file put into place, even if the document is the one `begin` saw, and
    /// whatever the session's own file holds now (a file changed on disk is kept there,
    /// never written over, decision 3); then the session holds the new path, its claim on
    /// it (`claim`) and on no other, its own file let go. Refused, nothing written and the
    /// transaction left open, as `commit` is (no transaction open, the model, the text's
    /// length), for a document the gate refuses, and as decision 35 refuses a path a
    /// command writes, as a document: not ending in `.silico`, outside the workspace or
    /// through a link out of it, in a `.silico-cache/` directory, a file there and no
    /// `replace`, or not a file; and for the session's own file (a commit with no path
    /// saves it) or one another session holds or is opening, by any path to it. The path
    /// is checked before the rebuild is waited for, and again as it's written. An untitled
    /// session has no own file to check it against: its first save is this, after which it
    /// holds the file as a session opened on it does.
    pub(super) fn commit_as(
        self: &Arc<Self>,
        stop: &Ticket,
        to: &SaveTo<'_>,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
    ) -> Result<Committed, Problems> {
        // Taken under the lock, and the path checked with it let go: the check reads the
        // file system, which may stall, and `status` reads the held state.
        let (state, own) = {
            let held = self.held.read();
            let begun = self.transaction(&held)?;
            let state = Saving {
                path: held
                    .file
                    .as_ref()
                    .map_or_else(|| to.given.clone(), |file| file.path.clone()),
                text: Arc::clone(&held.text),
                gated: Arc::clone(&held.gated),
                revision: held.revision,
                carried: Arc::clone(&held.carried),
                begun,
                changed_outside: None,
            };
            let own = held.file.as_ref().map(|file| {
                (
                    file.path.clone(),
                    file.resolved.clone(),
                    file.part_written.clone(),
                )
            });
            (state, own)
        };
        let own_file = own.as_ref().map(|(_, resolved, _)| resolved.as_path());
        to.target(self.workspace.as_ref(), own_file)?;
        let (mut committed, _) =
            self.save_state(stop, state, Through::Transaction, read, claim, Some(to))?;
        // The old file's mark, let go with it: said, so where its old text is isn't lost.
        if let Some((path, _, Some(old))) = own {
            committed.problems.push(left_part_written(&path, &old));
        }
        Ok(committed)
    }

    /// An edit's `candidate`, made outside a transaction, committed as its own transaction
    /// (decision 1): begun from the state held, applied, and committed, as [`commit`]
    /// commits, but never held until it's saved. Saved, the session is at the next revision,
    /// the text written; refused, the session is exactly as it was, at its revision, holding
    /// what it held, and its models as they were (the candidate's never published), and the
    /// response reads no model. With the commit's result, the document it saved, if it
    /// wrote one.
    ///
    /// [`commit`]: Self::commit
    pub(super) fn commit_candidate(
        self: &Arc<Self>,
        stop: &Ticket,
        candidate: Candidate,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
    ) -> Result<(Committed, Option<Arc<Gated>>), Problems> {
        // Its features' texts are an undo stack's, and no transaction is open: let go now,
        // not after its rebuild and save.
        let (text, gated, prepared) = candidate.into_parts();
        drop(prepared);
        let state = {
            let held = self.held.read();
            // Saved as it's made, so an untitled session, with no file to save it to,
            // refuses it as it is: nothing held, no revision given, no model read.
            let Some(file) = &held.file else {
                *read = None;
                return Err(Problems::from(untitled_save(self.id)));
            };
            Saving {
                path: file.path.clone(),
                text,
                gated,
                revision: held.next()?,
                carried: Arc::clone(&held.carried),
                begun: held.snapshot(),
                // Outside a transaction, a change found before the command was reloaded.
                changed_outside: None,
            }
        };
        let committed = self.save_state(stop, state, Through::Candidate, read, claim, None);
        if committed.is_err() {
            *read = None;
            // Its candidate's count is no revision's now, cancelled or refused (#423).
            let _rebuilds = lock(&self.rebuilds);
            self.newest.forget_candidate();
        }
        committed
    }

    /// `state` saved (a commit's body): its model, of `state.revision`, waited for, then
    /// its document in canonical form through `file::save_over`, with new fingerprints and
    /// solves for what the edits since `state.begun` reach and every other feature's as
    /// stored, the new file claimed with `claim`, and the session at a new revision, its
    /// saved text (and the transaction, if one is open, ended). A document equal to
    /// `begun`'s is written nowhere. `through` says whose state it is: a transaction's
    /// (held, its model the session's revision's, and a commit that writes nothing ends
    /// it), or an edit's candidate (held only once saved, its model rebuilt for this commit
    /// alone). With the commit's result, the document saved, as the session then holds it
    /// ([`write`](Self::write)), if one was written. Refused, nothing written and nothing
    /// held changed, as [`commit`](Self::commit) is. With `to`, a Save As
    /// ([`commit_as`](Self::commit_as)): written there whether or not it's changed, and a
    /// document the gate refuses refused. As the command `stop` stops.
    fn save_state(
        self: &Arc<Self>,
        stop: &Ticket,
        state: Saving,
        through: Through,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
        to: Option<&SaveTo<'_>>,
    ) -> Result<(Committed, Option<Arc<Gated>>), Problems> {
        let Saving {
            path,
            text,
            gated,
            revision,
            carried,
            begun,
            changed_outside,
        } = state;
        let saver = match through {
            Through::Transaction => Saver::Commit {
                base: begun.revision,
            },
            Through::Candidate => Saver::Edit,
        };
        let reached = Reached::of(Reaching {
            gated: Arc::clone(&gated),
            carried,
            begun: Some(begun.clone()),
        });
        let reach = reached.get();
        // As canonical form writes them: a document read from a file whose sketch lists
        // aren't in ID order is the one its canonical text reads as. After a `format` in
        // the transaction, by bytes: it asked for canonical form, as outside one. The same
        // compare as `status.transaction.changed`'s ([`Snapshot::differs`]).
        let unchanged = !begun.differs(&text, &gated);
        // Found before the command, so before any rebuild for it. A commit that would write
        // nothing loses nothing, and ends as one does, without waiting for a rebuild: its
        // problems the gate's alone.
        if let Some(outside) = changed_outside {
            if !unchanged {
                return Err(Problems::from(match outside {
                    Outside::Changed(_) | Outside::TooLong | Outside::Gone(_) => {
                        saver.conflict(&path)
                    }
                    Outside::Unreadable(error) => unread_by_commit(error),
                }));
            }
            // Only a transaction's commit is given what was found outside.
            self.unwritten(stop, &begun, revision)?;
            return Ok((Committed::unwritten(path, gated.problems()), None));
        }
        let checked = match &*gated {
            Gated::Checked(checked) => checked,
            refused if unchanged && to.is_none() => {
                let problems = refused.problems();
                if through == Through::Transaction {
                    self.unwritten(stop, &begun, revision)?;
                }
                return Ok((Committed::unwritten(path, problems), None));
            }
            refused => return Err(refused.problems()),
        };
        let model = match through {
            Through::Transaction => self.model(stop, revision, checked, &reached)?,
            Through::Candidate => self.candidate_model(stop, revision, checked)?,
        };
        *read = Some(ModelRead::of(revision, ModelChoice::Current, &model));
        let formatted = crate::command::formatted(
            &model,
            |id| {
                if reach.contains(id) {
                    recompute::Save::AsRebuilt
                } else {
                    recompute::Save::Stored
                }
            },
            &model.drifted(),
        )?;
        if unchanged && to.is_none() {
            if through == Through::Transaction {
                self.unwritten(stop, &begun, revision)?;
            }
            return Ok((Committed::unwritten(path, formatted.problems), None));
        }
        let saved = Arc::new(formatted.text.into_bytes());
        let publish = formatted.resolved.is_empty();
        let written = self.write(
            stop,
            Save {
                text: saved,
                model: &model,
                reach,
                publish,
            },
            saver,
            claim,
            to,
        )?;
        Ok((
            Committed {
                written: true,
                path: to.map_or(path, |to| to.given.clone()),
                refingerprinted: formatted.refingerprinted,
                resolved: formatted.resolved,
                problems: formatted.problems,
            },
            Some(written),
        ))
    }

    /// A commit that writes nothing (its document is the one `begin` saw, `begun`, at
    /// `revision` now): the transaction ended, and the session's text `begin`'s, which is
    /// the file's as the session last read or wrote it (a change made outside since is
    /// reloaded by the next command), at a new revision if the edits left other bytes (the
    /// same document in canonical form, say), whose model is `revision`'s if it was
    /// published. As the command `stop` stops.
    fn unwritten(
        &self,
        stop: &Ticket,
        begun: &Snapshot,
        revision: Revision,
    ) -> Result<(), Problems> {
        let held = self.held.read();
        let next = if held.text == begun.text {
            None
        } else {
            Some(held.next()?)
        };
        let mut held = held.changing(stop)?;
        let mut rebuilds = lock(&self.rebuilds);
        end_transaction(&mut held, &mut rebuilds);
        if let Some(next) = next {
            let carried = Arc::clone(&held.carried);
            held.change(
                next,
                Arc::clone(&begun.text),
                Arc::clone(&begun.gated),
                carried,
            );
            // The same document: its model is the new revision's too, and what it reached
            // is what's reached at the new revision: the transaction's edits came back to the
            // document `begin` saw, so they reach nothing (`document::net_reach`), and what's
            // reached at both is what was carried, with every feature that uses it. Whether a
            // transaction is open differs (it was then, it isn't now): for a model of the
            // session's revision, as this one is from now on, a check asks the session that
            // as it checks, never the revision (`Checking::spared`).
            if let Some(published) = &mut rebuilds.published
                && published.revision == revision
            {
                published.revision = next;
            }
        }
        Ok(())
    }

    /// `format` (decision 31): the current revision's rebuild waited for, and the document
    /// in canonical form with every feature's fingerprint and every sketch's solve as
    /// rebuilt, but those of a drifted feature `accept` doesn't name (all, if it's none),
    /// which keep what's stored with them and are problems; written through `file::save_over`
    /// if `write` and it differs from the file as the session last read or wrote it (a
    /// change made outside the session before the command was reloaded, decision 2), the
    /// new file claimed with `claim`, the session then at a new revision, its saved
    /// text. Inside a transaction (docs/design/m5.0-app.md, A6; decision 8), no file is
    /// written: if `write` and the formatted document isn't the transaction's (as canonical
    /// form compares them), it's held as one edit, at a new revision, on the transaction's
    /// undo stack, which its commit saves; `changed` says whether it differs, `written` is
    /// false, and with `write` false nothing changes. Refused, writing nothing, for a
    /// document the gate refused (its problems), for a feature in `accept` the document
    /// hasn't, and as a commit is. A kernel fault is saved around, as a commit saves around
    /// it. In an untitled session, with no file to write, `write` outside a transaction is
    /// refused before anything is rebuilt; its `path` is none. As the command `stop` stops.
    pub(super) fn format(
        self: &Arc<Self>,
        stop: &Ticket,
        accept: Option<&[FeatureId]>,
        write: bool,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
    ) -> Result<Reformatted, Problems> {
        let (path, bytes, text, gated, revision, reaching, transaction) = {
            let held = self.held.read();
            if write && held.transaction.is_none() && held.file.is_none() {
                return Err(Problems::from(untitled_save(self.id)));
            }
            (
                held.file.as_ref().map(|file| file.path.clone()),
                Arc::clone(&held.bytes),
                Arc::clone(&held.text),
                Arc::clone(&held.gated),
                held.revision,
                held.reaching(),
                held.transaction.is_some(),
            )
        };
        let reached = Reached::of(reaching);
        let reach = reached.get();
        let checked = match &*gated {
            Gated::Checked(checked) => checked,
            refused => return Err(refused.problems()),
        };
        let document = checked.document();
        let accept: Option<BTreeSet<&FeatureId>> = match accept {
            None => None,
            Some(accept) => Some(named(document, accept)?),
        };
        let model = self.model(stop, revision, checked, &reached)?;
        *read = Some(ModelRead::of(revision, ModelChoice::Current, &model));
        // Each feature's drift, once: a feature edits reached isn't drift.
        let drift = model.drifted();
        let drifted: BTreeSet<&FeatureId> = model
            .drift_left(&drift, reach)
            .filter(|(_, error)| error.code() == ErrorCode::Drift)
            .map(|(id, _)| id)
            .collect();
        let ids = || document.features.iter().map(|feature| &feature.id);
        let writes: BTreeSet<FeatureId> = ids()
            .filter(|id| {
                !drifted.contains(id) || accept.as_ref().is_none_or(|accept| accept.contains(id))
            })
            .cloned()
            .collect();
        // Accepted only where a record was written in its place.
        let accepted = ids()
            .filter(|id| drifted.contains(id) && writes.contains(*id) && model.records(id))
            .cloned()
            .collect();
        let formatted = crate::command::formatted(
            &model,
            |id| {
                if reach.contains(id) {
                    recompute::Save::AsRebuilt
                } else if writes.contains(id) {
                    recompute::Save::Recorded
                } else {
                    recompute::Save::Stored
                }
            },
            &drift,
        )?;
        let saved = formatted.text.into_bytes();
        if transaction {
            // Into the transaction's state, as an edit's candidate, the same document read
            // back as `replace_text` reads one. Changed if its bytes are other than the
            // transaction's, as outside one they'd be other than the file's: canonical form
            // too, which its commit then writes.
            let changed = saved != *text;
            if write && changed {
                let candidate = crate::command::edit::as_text(&saved).map_err(|refused| {
                    Problems::from(
                        Error::new(
                            ErrorCode::Internal,
                            "the formatted document doesn't read back as the transaction's",
                        )
                        .with_cause(format!("{:?}", refused.errors()))
                        .with_suggestion(crate::worker::SILICO_BUG),
                    )
                })?;
                // What it wrote other values for, as it wrote them: reaching nothing.
                let written: BTreeMap<FeatureId, document::Feature> =
                    match candidate.gated().document() {
                        Ok(now) => document::changed_features(document, now)
                            .into_iter()
                            .filter_map(|id| {
                                now.features
                                    .iter()
                                    .find(|feature| feature.id == id)
                                    .map(|feature| (id, feature.clone()))
                            })
                            .collect(),
                        Err(_) => BTreeMap::new(),
                    };
                self.hold_then(stop, candidate, true, |held| {
                    if let Some(open) = &mut held.transaction {
                        let formatted = Arc::make_mut(&mut open.begun.formatted);
                        formatted.written.extend(written);
                        formatted.layout = true;
                        formatted.bytes = formatted.cost();
                        // The format's own state holds it, for an undo past it to take back.
                        open.undo.set_extra(Arc::clone(&open.begun.formatted));
                    }
                })?;
            }
            return Ok(Reformatted {
                changed,
                written: false,
                path,
                refingerprinted: formatted.refingerprinted,
                resolved: formatted.resolved,
                accepted,
                problems: formatted.problems,
            });
        }
        let changed = saved != *bytes;
        let written = write && changed;
        if written {
            self.write(
                stop,
                Save {
                    text: Arc::new(saved),
                    model: &model,
                    reach,
                    publish: formatted.resolved.is_empty(),
                },
                Saver::Format,
                claim,
                None,
            )?;
        }
        Ok(Reformatted {
            changed,
            written,
            path,
            refingerprinted: formatted.refingerprinted,
            resolved: formatted.resolved,
            accepted,
            problems: formatted.problems,
        })
    }

    /// The save of a commit or a format, as the command `stop` stops: `save.text` written
    /// over the document's file, which the session last read or wrote as the bytes it holds
    /// ([`save`](Self::save)), its new file claimed with `claim`; then the session at a new
    /// revision whose text is the one saved, carrying the features of `save.reach` whose
    /// records `save.model` couldn't write, and any transaction ended. With `save.publish`,
    /// the model rebound to the saved document ([`Model::rebound`]) is published as the new
    /// revision's, in the same step: for a save (a commit, an edit's, or `format`'s) that
    /// wrote only fingerprints, whose document a rebuild would build from the steps the model
    /// took, so a kernel fault it holds is the saved revision's outcome too, never met again
    /// for it (decision 32). One that wrote a sketch's solve doesn't publish it: the saved
    /// document's rebuild solves from the values it wrote. The document saved, as the gate
    /// found it, which the session now holds. Refused, nothing changed, as the save is, a
    /// conflict with what `saver` advises; or, `cancelled`, if the command was stopped
    /// before the save began (past it, a cancel comes too late, and the command answers with
    /// its real result). With `to`, a Save As: written there ([`save_to`](Self::save_to)),
    /// not over the session's file, and the session's file from then on, its path as `to`
    /// gives it, the disk watch moved to it (an untitled session has none yet: it's given one
    /// after the save has written, before the commit answers,
    /// [`registry::watch_saved`](super::registry::watch_saved)).
    /// With none, refused for an
    /// untitled session, which has no file to save over.
    fn write(
        &self,
        stop: &Ticket,
        save: Save<'_>,
        saver: Saver,
        claim: &dyn Fn(&Path),
        to: Option<&SaveTo<'_>>,
    ) -> Result<Arc<Gated>, Problems> {
        let Save {
            text: saved,
            model,
            reach,
            publish,
        } = save;
        let (next, own, bytes) = {
            let held = self.held.read();
            (
                held.next()?,
                held.file
                    .as_ref()
                    .map(|own| (own.path.clone(), own.resolved.clone())),
                Arc::clone(&held.bytes),
            )
        };
        let (saving, resolved) = match (to, own) {
            (None, Some((path, resolved))) => (
                self.save(stop, &path, &resolved, &bytes, &saved, saver)?,
                resolved,
            ),
            // Every save over the session's file is refused before it's made for an untitled
            // session (`commit`, an edit's commit, `format`): this is never reached.
            (None, None) => return Err(Problems::from(untitled_commit(self.id))),
            (Some(to), own) => {
                let own = own.as_ref().map(|(_, resolved)| resolved.as_path());
                (None, self.save_to(stop, to, own, &saved)?)
            }
        };
        claim(&resolved);
        let carried = reach
            .iter()
            .filter(|id| !model.records(id))
            .cloned()
            .collect();
        let gated = Arc::new(Gated::of(&saved));
        // Settled by the save, so never refused.
        let mut held = self.held.read().changing(stop)?;
        held.change(
            next,
            Arc::clone(&saved),
            Arc::clone(&gated),
            Arc::new(carried),
        );
        held.bytes = Arc::clone(&saved);
        // Under the held state's lock, the watch's taken last: a comparison that read the
        // file before the save is dropped, and one after it finds the file unchanged.
        match (saving, to) {
            (Some(saving), _) => saving.saved(saved),
            (None, Some(to)) => {
                // The old file's mark, if it had one, goes with it.
                held.file = Some(HeldFile {
                    path: to.given.clone(),
                    resolved: resolved.clone(),
                    part_written: None,
                });
                if let Some(disk) = self.disk.get() {
                    disk.moved(resolved, saved);
                }
            }
            (None, None) => {}
        }
        // Under the held state's lock, so no published read of the new revision starts its
        // rebuild before its model is there.
        let mut rebuilds = lock(&self.rebuilds);
        end_transaction(&mut held, &mut rebuilds);
        if publish
            && let Gated::Checked(checked) = &*gated
            && next >= rebuilds.floor
            && rebuilds
                .published
                .as_ref()
                .is_none_or(|newest| newest.revision <= next)
        {
            rebuilds.published = Some(Published {
                revision: next,
                model: Arc::new(model.rebound(Arc::clone(checked))),
                // The saved revision's, its transaction ended: what's carried, and what uses
                // it. Taken, not worked out, under the locks.
                reached: Reached::of(held.reaching()),
                by: None,
            });
        }
        Ok(gated)
    }

    /// Before a queued command (decision 2), as `rereads` says: the session's file read
    /// again through [`file::opened_and_read`] (one read, bounded as open's is) and
    /// compared, byte for byte, with what the session last read or wrote: bytes, not times,
    /// so a change that keeps the file's size and its modification time is found too. The
    /// file read is claimed with `claim`, as `open` claims it, so a file another session
    /// holds (a link to its document put at the path, even to a file put at that session's
    /// path that it hasn't read yet) refuses the command, naming that session, before
    /// anything changes.
    ///
    /// Outside a transaction, a change is reloaded: the file's bytes are the session's text
    /// at a new revision, through the gate (a document it refuses held with its problems, as
    /// `open` holds one, decision 21), reaching nothing (a hand edit, the design's section
    /// 5: its stale fingerprints are drift until `format` accepts them), not even what was
    /// carried (so the session is as a fresh `open` of the file). A file gone or unreadable,
    /// or longer than Silico reads of a document, refuses the command, naming it, the
    /// session as it was; as does a session with no revisions left to give.
    ///
    /// Inside a transaction, only for `commit`, and `diff` and `text` naming `"disk"`
    /// ([`Rereads::Always`]): nothing is reloaded, and a file changed (its bytes kept, once
    /// claimed as a reload's are), longer than a document may be, gone, or unreadable is
    /// found for the command ([`Reload::Outside`]): for the commit to refuse, if it would
    /// write; for a read, the text `"disk"` names, or why there's none.
    ///
    /// A file this session's own save left part written isn't compared, nor reloaded (the
    /// next save says where its old text is), until it holds what the session last read or
    /// wrote again: the old text put back, which clears the mark.
    ///
    /// Given `settled`, the digest of the bytes the disk watch found settled (its own reload,
    /// A3, never a command's), a file found outside a transaction as neither those bytes nor
    /// what the session holds changed again after the watch found it: perhaps a write begun
    /// since, so perhaps part of one. It's found as inside a transaction (claimed as there,
    /// so only a claim refuses it), never reloaded ([`Reload::Unsettled`]); the watch reads
    /// it again, and its reload is queued once it has settled again (#479).
    ///
    /// An untitled session has no file to read: what it holds is as held, always.
    pub(super) fn reload(
        &self,
        rereads: Rereads,
        settled: Option<&[u8; 32]>,
        claim: &dyn Fn(Claim<'_>) -> Result<(), Unclaimed>,
    ) -> Result<Reload, Problems> {
        let (own, bytes, transaction) = {
            let held = self.held.read();
            (
                held.file.as_ref().map(|own| {
                    (
                        own.path.clone(),
                        own.resolved.clone(),
                        own.part_written.is_some(),
                    )
                }),
                Arc::clone(&held.bytes),
                held.transaction.is_some(),
            )
        };
        let Some((path, resolved, part_written)) = own else {
            return Ok(Reload::AsHeld);
        };
        if rereads == Rereads::Never || (transaction && rereads != Rereads::Always) {
            return Ok(Reload::AsHeld);
        }
        let _span = tracing::info_span!("reload").entered();
        // Where the path leads now, as `open` found where it led: through a link put there
        // since, to another session's path, say; refused if that's out of the workspace,
        // before the file's read (decision 26).
        let found_at = match file::resolve(self.workspace.as_ref(), &resolved) {
            Ok(found_at) => found_at,
            Err(file::Unresolved::Outside) => {
                return Err(Problems::from(Workspace::outside_now(Path::new(&path))));
            }
            Err(file::Unresolved::Unreachable(_)) => resolved.clone(),
        };
        let found = file::opened_and_read(&resolved);
        let claimed = |opened| {
            claim(Claim {
                opened,
                resolved: &resolved,
                found_at: &found_at,
            })
            .map_err(|unclaimed| Problems::from(unclaimed.rereading(&path)))
        };
        let gone = || matches!(resolved.try_exists(), Ok(false));
        if part_written {
            if let Ok((opened, now)) = found
                && now == *bytes
            {
                claimed(opened)?;
                if let Some(file) = &mut self.held.session_changing().file {
                    file.part_written = None;
                }
                tracing::info!(session = %self.id, "the file a save left part written holds its old text again");
            }
            return Ok(Reload::AsHeld);
        }
        // The disk watch's own reload, for the bytes it found settled: a file found as neither
        // those nor what the session holds changed again since, perhaps mid-write, and is
        // found as inside a transaction, never loaded (#479).
        let unsettled = settled.is_some_and(|settled| match &found {
            Ok((_, now)) => *now != *bytes && digest(now) != *settled,
            Err(_) => true,
        });
        if transaction || unsettled {
            let outside = match found {
                Ok((opened, now)) if now == *bytes => {
                    claimed(opened)?;
                    return Ok(Reload::AsHeld);
                }
                Ok((_, now)) if now.len() > MAX_INPUT_LEN => Outside::TooLong,
                // Kept, for a read that names the file (`"disk"`): the one read made of it,
                // claimed first, as outside a transaction, so another session's file (a link
                // to its document put at the path) is never given as this session's.
                Ok((opened, now)) => {
                    claimed(opened)?;
                    Outside::Changed(Arc::new(now))
                }
                Err(error) if gone() => Outside::Gone(of_file(&path, &error)),
                Err(error) => Outside::Unreadable(of_file(&path, &error)),
            };
            if transaction {
                tracing::info!(session = %self.id, "the file changed outside the session, inside a transaction: not reloaded");
                return Ok(Reload::Outside(outside));
            }
            tracing::info!(session = %self.id, "the file changed again before its reload read it: not reloaded until it settles");
            return Ok(Reload::Unsettled(outside));
        }
        let (opened, now) = found.map_err(|error| {
            let suggestion = if gone() {
                put_back(&path)
            } else {
                format!(
                    "make the file at {} a regular file this user may read, or close the session and open the file where it is now",
                    shown_path(Path::new(&path))
                )
            };
            Problems::from(not_reloaded(of_file(&path, &error), suggestion))
        })?;
        if now == *bytes {
            claimed(opened)?;
            return Ok(Reload::AsHeld);
        }
        if now.len() > MAX_INPUT_LEN {
            return Err(Problems::from(not_reloaded(
                too_long(Path::new(&path)),
                put_back(&path),
            )));
        }
        // Gated with the held state's lock let go, so `status` never waits on it; nothing
        // else changes what's held meanwhile, since only the session's thread changes it.
        let gated = Arc::new(Gated::of(&now));
        // Given before the claim moves, so a session with none left keeps its claim.
        let next = self.held.read().next()?;
        claimed(opened)?;
        let now = Arc::new(now);
        // The session's own change, not the command's: it stands whatever the command comes
        // to (a cancel after it too), and the response says `reloaded`.
        let mut held = self.held.session_changing();
        // What the reload replaces, kept for `diff` until the next reload.
        held.reloaded_over = Some((held.revision, Arc::clone(&held.text)));
        // Nothing carried: a reloaded session is as a fresh `open`.
        held.change(next, Arc::clone(&now), gated, Arc::new(BTreeSet::new()));
        held.bytes = Arc::clone(&now);
        // Under the held state's lock, the watch's taken last: what it compares with from
        // now on, the file unchanged.
        if let Some(disk) = self.disk.get() {
            disk.held(now);
        }
        held.last_reload = Some(next);
        drop(held);
        tracing::info!(session = %self.id, revision = next.number(), "the file changed outside the session: reloaded");
        Ok(Reload::Reloaded)
    }

    /// `text` saved over the document's file at `resolved` (`path` as `open` was given it),
    /// which the session last read or wrote as `bytes`, as the command `stop` stops: the one
    /// place a session writes its file, its stop settled first, so from here a cancel comes
    /// too late. Refused, nothing written, if the command was cancelled first, if the file
    /// changed since (a conflict, naming the file, with what `saver` advises) or can't be
    /// written; or if this session's own save left it part written before, saying where its
    /// old text is. A save that leaves it so is marked, for the next to say. Bracketed for
    /// the session's disk watch: what it gives back is [`saved`](crate::disk::Saving::saved)
    /// where the session takes what it wrote, so no comparison reads its own save as a
    /// change; dropped on a failure, the watch's finding unknown until it compares again.
    /// (A session with a file has its watch from the moment it has the file.)
    fn save(
        &self,
        stop: &Ticket,
        path: &str,
        resolved: &Path,
        bytes: &[u8],
        text: &[u8],
        saver: Saver,
    ) -> Result<Option<crate::disk::Saving<'_>>, Problems> {
        if let Some(old) = self
            .held
            .read()
            .file
            .as_ref()
            .and_then(|file| file.part_written.as_ref())
        {
            return Err(Problems::from(part_written(path, old)));
        }
        stop.settle()?;
        let within = self.workspace.as_ref().map(|workspace| file::Within {
            workspace,
            given: Path::new(path),
        });
        let saving = self.disk.get().map(crate::disk::DiskWatch::saving);
        #[cfg(test)]
        self.writing.pass();
        file::save_within(resolved, bytes, text, within)
            .map_err(|unsaved| self.unsaved(path, unsaved, saver))?;
        Ok(saving)
    }

    /// `text` saved as `to` says (a Save As), the session's own file at `own` (none for an
    /// untitled session) untouched, as
    /// the command `stop` stops: its stop settled first, so from here a cancel comes too
    /// late; the path checked again, claimed for the session, and written whole, through a
    /// new file put into place, never over a file put there meanwhile unless `replace`
    /// (on a file system without hard links, but for one put there between the path's last
    /// look and the rename).
    /// Where it wrote, through any links; or refused, nothing written and the path's claim
    /// given back.
    fn save_to(
        &self,
        stop: &Ticket,
        to: &SaveTo<'_>,
        own: Option<&Path>,
        text: &[u8],
    ) -> Result<PathBuf, Problems> {
        stop.settle()?;
        let target = to.target(self.workspace.as_ref(), own)?;
        (to.reserve)(&target).map_err(|holder| held_elsewhere(&to.given, holder))?;
        #[cfg(test)]
        self.writing.pass();
        // Written only where it was checked and claimed: a link put on its way since is
        // refused, so it never writes over another session's file.
        let written = file::write_output_at(
            Path::new(&to.given),
            text,
            &to.rules(self.workspace.as_ref(), to.replace, true),
            &target,
        );
        if written.is_err() {
            (to.unreserve)(&target);
        }
        // A write that failed names the path, as a commit's own does.
        Ok(written.map_err(|error| match error.parameter() {
            Some(_) => error,
            None => error.with_parameter("path"),
        })?)
    }

    /// The refusal of a save of the document at `path` (as `open` was given it) that failed,
    /// as `unsaved` says: a conflict, naming the file, with what `saver` advises, or why it
    /// couldn't be written; and, if it left the file part written, that marked, for the next
    /// save to say.
    pub(super) fn unsaved(&self, path: &str, unsaved: file::Unsaved, saver: Saver) -> Problems {
        if let Some(old) = unsaved.old_text_in
            && let Some(file) = &mut self.held.session_changing().file
        {
            // The session's record of its file, past the command's settling: it stands.
            file.part_written = Some(old);
        }
        let error = unsaved.error;
        Problems::from(if error.code() == ErrorCode::Conflict {
            saver.conflict(path)
        } else {
            error.with_parameter("path")
        })
    }
}

/// Whose save found the file changed outside the session (decision 2), which says what to
/// do about it: nothing merges, and the session's next command outside a transaction
/// reloads the file.
#[derive(Clone, Copy)]
pub(super) enum Saver {
    /// A transaction's commit, the transaction begun from `base`: rolled back, the next
    /// command (`begin`, say) reloads the file.
    Commit { base: Revision },
    /// An edit made outside a transaction, committed as its own: the next command reloads
    /// the file, at a new revision, which the edit is made against again.
    Edit,
    /// `format`, which reloads the file when it runs again.
    Format,
}

impl Saver {
    /// The conflict of a save of the document at `path` (as `open` was given it): the file
    /// named, and what to do.
    fn conflict(self, path: &str) -> Error {
        let error = Error::new(
            ErrorCode::Conflict,
            format!(
                "{} changed outside this session since it was read",
                shown_path(Path::new(path))
            ),
        )
        .with_parameter("path");
        match self {
            Saver::Commit { base } => error
                .with_cause(format!(
                    "it changed after the transaction began, from revision {}, and a transaction doesn't reload its file",
                    base.number()
                ))
                .with_suggestion(
                    "nothing was written: roll back, then begin again from the file as it is now; or save elsewhere with path",
                ),
            Saver::Edit => error.with_suggestion(
                "nothing was written: read it again and make the edit against the new revision",
            ),
            Saver::Format => {
                error.with_suggestion("nothing was written: run format again, on the file as it is now")
            }
        }
    }
}

/// The refusal of a file at `path` (as `open` was given it) longer than Silico reads of a
/// document: a session couldn't hold its text, or mend it, whole.
pub(super) fn too_long(path: &Path) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!(
            "{} is more than the {MAX_INPUT_LEN} bytes Silico reads of a document",
            shown_path(path)
        ),
    )
    .with_parameter("path")
    .with_suggestion(TOO_LONG)
}

/// What to do about a file longer than Silico reads of a document ([`too_long`]'s
/// suggestion): so a client that opens a session for each run (the CLI) can say it of a
/// file grown that long since, rather than a session's advice.
pub const TOO_LONG: &str = "check it's the document you mean: no document is this long";

/// `error`, a file's (as `file::opened_and_read` gives it, saying nothing of which file),
/// naming the file at `path` (as `open` was given it): its code, and its message after the
/// file's name.
pub(super) fn of_file(path: &str, error: &Error) -> Error {
    Error::new(
        error.code(),
        format!("{} {}", shown_path(Path::new(path)), error.message()),
    )
}

/// `error`, naming the file, why a session couldn't take its file as it is now, before a
/// command (decision 2): gone, unreadable, too long, or another session's. The command is
/// refused, with `suggestion`, how to go on.
pub(super) fn not_reloaded(error: Error, suggestion: String) -> Error {
    error
        .with_parameter("path")
        .with_cause(NOT_RELOADED)
        .with_suggestion(suggestion)
}

/// The cause of every refusal of a command whose session couldn't take its file as it is
/// now ([`not_reloaded`]): so a client that opens a session for each run (the CLI) can tell
/// these apart from the rest, and say how to go on in its own words.
pub const NOT_RELOADED: &str =
    "the session reads its file before each command to find changes made outside it";

/// What to do about a session's file at `path` (as `open` was given it) gone or grown too
/// long, outside a transaction.
pub(super) fn put_back(path: &str) -> String {
    format!(
        "put the file back at {}, or close the session and open the file where it is now",
        shown_path(Path::new(path))
    )
}

/// `error`, why the session's file couldn't be read before a transaction's `commit`, naming
/// the file ([`of_file`]): its `io_failed`, if the commit would write.
fn unread_by_commit(error: Error) -> Error {
    const SUGGESTION: &str = "nothing was written: make it a regular file this user may read again, then commit; or roll back";
    error.with_parameter("path").with_suggestion(SUGGESTION)
}

/// The refusal of a save after this session's own save left the file at `path` part
/// written, its old text in `old`.
pub(super) fn part_written(path: &str, old: &Path) -> Error {
    Error::new(
        ErrorCode::IoFailed,
        format!(
            "{} was left part written by this session's own save, which couldn't put its text back: its text before is in {}",
            shown_path(Path::new(path)),
            shown_path(old)
        ),
    )
    .with_parameter("path")
    .with_suggestion(
        "nothing was written: put the old text back in the file, then run the command again, or close the session and open the file again",
    )
}

/// The features of `document` that `named` names, each once; or, refusing, each name it
/// hasn't (`features.N`), the first [`document::PROBLEMS_LISTED`] listed and all counted.
fn named<'a>(
    document: &'a Document,
    named: &[FeatureId],
) -> Result<BTreeSet<&'a FeatureId>, Problems> {
    let ids: BTreeSet<&FeatureId> = document
        .features
        .iter()
        .map(|feature| &feature.id)
        .collect();
    let mut found = BTreeSet::new();
    let mut unknown = Vec::new();
    let mut unlisted = 0u64;
    for (i, id) in named.iter().enumerate() {
        match ids.get(id) {
            Some(id) => {
                found.insert(*id);
            }
            None if unknown.len() < document::PROBLEMS_LISTED => unknown.push(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!("the document has no feature {id} to accept the drift of"),
                )
                .with_parameter(format!("features.{i}"))
                .with_suggestion("name features by their IDs, as the document holds them"),
            ),
            None => unlisted += 1,
        }
    }
    if unknown.is_empty() {
        Ok(found)
    } else {
        Err(Problems::with_unlisted(unknown, unlisted))
    }
}

/// The refusal of a command that needs a transaction, with none open in session `id`.
fn no_transaction(id: SessionId) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("no transaction is open in session {id}"),
    )
    .with_parameter("session")
    .with_suggestion("begin a transaction first; outside one, every edit is saved as it's made")
}

/// The refusal of what isn't done while a transaction is open in session `id` (`then`:
/// close the session, format), in the words of the design's `close` example.
pub(super) fn transaction_open(id: SessionId, then: &str) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("a transaction is open in session {id}"),
    )
    .with_parameter("session")
    .with_suggestion(format!("commit or roll back the transaction, then {then}"))
}

/// When a queued command's session reads its file first (decision 2; the design's section
/// 4), each need stronger than the one before it (`Never < OutsideTransaction < Always`), so
/// a command whose request names several texts reads as the strongest asks
/// ([`needed`](Self::needed)).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Rereads {
    /// Never: the commands that need nothing of the document, `close` (which always
    /// closes) and `cache_clear` (which clears the directory's cache whatever the file
    /// holds); and reads that name only revisions by number, whose texts the session holds
    /// whatever the file holds now. (An untitled session never reads, having no file.)
    Never,
    /// Outside a transaction only: inside one, the command runs on what the session holds.
    OutsideTransaction,
    /// Inside a transaction too, where nothing is reloaded: what's found is the command's
    /// (`commit`'s to refuse, or a read's that names `"disk"`).
    Always,
}

impl Rereads {
    /// What a read of `sides` needs (Fable's decision of 2026-11-04, *6d's other half*,
    /// section 2): the strongest of each side's, a `null` side (the document now) as every
    /// command reads, outside a transaction; `"disk"` inside one too, as `commit` reads;
    /// a revision by number, nothing. None for no sides.
    pub(super) fn needed(sides: impl IntoIterator<Item = Option<Named>>) -> Rereads {
        sides
            .into_iter()
            .map(|side| match side {
                None => Rereads::OutsideTransaction,
                Some(Named::Disk) => Rereads::Always,
                Some(Named::Revision(_)) => Rereads::Never,
            })
            .max()
            .unwrap_or(Rereads::Never)
    }
}

/// What a session found of its file before a command ([`Session::reload`]), given to the
/// command.
pub(super) enum Reload {
    /// Nothing to change: the file unread (inside a transaction, for a command that needs
    /// nothing of the document, `close` and `cache_clear`, or with no file, the session
    /// untitled), or as the session last read or wrote it.
    AsHeld,
    /// Found changed outside the session, and reloaded: the response's `reloaded`.
    Reloaded,
    /// Inside a transaction, before `commit` or a read naming `"disk"`
    /// ([`Rereads::Always`]): found not as the session last read or wrote it, and not
    /// reloaded, for the commit to refuse if it would write, or the read to give or refuse.
    Outside(Outside),
    /// Outside a transaction, for the disk watch's own reload only (given the bytes it found
    /// settled): found as neither those bytes nor what the session last read or wrote, and
    /// so changed again since the watch found it, perhaps mid-write; found as
    /// [`Outside`](Reload::Outside) finds it, and not reloaded until the watch finds it
    /// settled again (#479).
    Unsettled(Outside),
}

impl Reload {
    /// Whether the file was reloaded: the response's `reloaded`.
    pub(super) fn reloaded(&self) -> bool {
        matches!(self, Reload::Reloaded)
    }

    /// How the file was found, inside a transaction, if not as the session last read or
    /// wrote it.
    pub(super) fn outside(self) -> Option<Outside> {
        match self {
            Reload::Outside(outside) | Reload::Unsettled(outside) => Some(outside),
            Reload::AsHeld | Reload::Reloaded => None,
        }
    }

    /// [`outside`](Self::outside), borrowed.
    pub(super) fn as_outside(&self) -> Option<&Outside> {
        match self {
            Reload::Outside(outside) | Reload::Unsettled(outside) => Some(outside),
            Reload::AsHeld | Reload::Reloaded => None,
        }
    }
}

/// A file found, inside a transaction, not as its session last read or wrote it: for a
/// commit that would write, each but `Unreadable` a conflict.
pub(super) enum Outside {
    /// It holds other bytes, these, at most what Silico reads of a document: what a read
    /// naming `"disk"` gives.
    Changed(Arc<Vec<u8>>),
    /// It holds more bytes than Silico reads of a document.
    TooLong,
    /// It's gone (nothing at its path, or a link there to nothing), as the read's error,
    /// naming the file, says.
    Gone(Error),
    /// It couldn't be read, as the error, naming the file, says: an `io_failed`.
    Unreadable(Error),
}

/// A file read before a command ([`Session::reload`]), to be claimed for its session as
/// `open` claims one ([`claim`]): the file opened, the session's path as its links resolved
/// at `open`, and where that path leads now.
pub(super) struct Claim<'a> {
    pub(super) opened: std::fs::File,
    pub(super) resolved: &'a Path,
    pub(super) found_at: &'a Path,
}
