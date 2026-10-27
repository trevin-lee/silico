//! What a session holds ([`Held`]) and how it changes in transactions: `begin`, `rollback`,
//! `commit`, and `format`, each save written through [`file::save_over`], what a save that
//! finds the file changed says ([`Saver`]), and the reload of a file changed outside the
//! session before a command (decision 2).
//!
//! The order its locks are taken in is on [`Session`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use super::cancel::Ticket;
use super::rebuilds::{Kept, Rebuilds};
use super::registry::Unclaimed;
#[cfg(doc)]
use super::registry::claim;
use super::{
    Begun, Committed, Gated, ModelChoice, ModelRead, Problems, Published, Reformatted, Revision,
    RolledBack, Session, SessionId, shown_path,
};
use crate::command::edit::Candidate;
use crate::document::json::MAX_INPUT_LEN;
use crate::document::{self, Document, FeatureId};
use crate::file::{self, Workspace};
use crate::kernel::Kernel;
use crate::recompute::{self, Model};
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// What a session holds (docs/design/m4.1-command-api.md, section 3). Its values are shared
/// with the commands reading them, so a command takes them under the lock at once and works
/// off it, and `status` never waits on a command's work. Changed only as `holding` allows:
/// by a command once its stop is settled, or by the session's own reload.
pub(super) struct Held {
    /// The document's path as `open` was given it.
    pub(super) path: String,
    /// The path as its links resolve, where saves and reloads go.
    pub(super) resolved: PathBuf,
    /// The file's bytes as the session last read or wrote them: what a save checks the file
    /// still holds, so one changed outside the session isn't written over.
    pub(super) bytes: Arc<Vec<u8>>,
    /// The document's text: the file's bytes, until an edit.
    pub(super) text: Arc<Vec<u8>>,
    /// The text as the reader and the gate found it.
    pub(super) gated: Arc<Gated>,
    pub(super) revision: Revision,
    /// The features edits reached before the last save whose records it couldn't write (a
    /// feature that failed): still reached, until a save writes them.
    pub(super) carried: Arc<BTreeSet<FeatureId>>,
    /// The state `begin` kept, while a transaction is open.
    pub(super) transaction: Option<Snapshot>,
    /// Where the file's old text is, if this session's own save left the file part
    /// written and couldn't put it back: the next save says so, rather than take the file
    /// for one changed outside the session; until the file holds what the session last read
    /// or wrote again, the old text put back ([`Session::reload`]).
    pub(super) part_written: Option<PathBuf>,
    /// The newest revision a reload of the file gave (decision 2): an edit made against a
    /// revision before it is stale because the file changed, which its refusal says.
    pub(super) last_reload: Option<Revision>,
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
    /// If its text didn't read as a document, the first state the transaction's edits held
    /// that read as one: its mend, which the edits after it are compared with
    /// ([`Reaching::reach`]). Not part of what `rollback` restores.
    mend: Option<Arc<Gated>>,
}

/// A state of the session's document a commit saves ([`Session::save_state`]): the path as
/// `open` was given it, the text and the gate's finding of it, the revision whose model is
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
        if let Some(begun) = &mut self.transaction
            && begun.mend.is_none()
            && begun.gated.document().is_err()
            && gated.document().is_ok()
        {
            begun.mend = Some(Arc::clone(&gated));
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
        }
    }

    /// What the session's edits reach now, taken under the lock, to be worked out
    /// ([`Reaching::reach`]) once it's let go.
    pub(super) fn reaching(&self) -> Reaching {
        Reaching {
            gated: Arc::clone(&self.gated),
            carried: Arc::clone(&self.carried),
            begun: self.transaction.clone(),
        }
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
        let mend = begun.mend.as_deref().map(Gated::document);
        let before = match (begun.gated.document(), mend) {
            (Ok(before), _) => document::Before::Read(before),
            (Err(_), Some(Ok(mend))) => document::Before::Mended {
                unread: unread(),
                mend,
            },
            (Err(_), _) => document::Before::Unread(unread()),
        };
        document::net_reach(before, &self.carried, now)
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
    /// A rollback restores the model `begin` kept.
    pub(crate) fn committing(&self, checking: Checking) -> &BTreeSet<FeatureId> {
        static NONE: BTreeSet<FeatureId> = BTreeSet::new();
        let spared = match checking {
            Checking::Latest { open } => open,
            Checking::Older { open } => open || self.of_transaction(),
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
/// decides which missing fingerprints the check leaves to a commit
/// ([`Reached::committing`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Checking {
    /// The model is of the session's revision; `open`, whether a transaction is open.
    Latest { open: bool },
    /// The model is an older revision's, the newest published; `open`, whether a
    /// transaction is open in the session now.
    Older { open: bool },
}

impl Checking {
    /// A check of the model of revision `model`, in a session at revision `now`, a
    /// transaction open in it or not (`open`), both as one lock held them.
    pub(crate) fn of(open: bool, now: Revision, model: Revision) -> Self {
        if model == now {
            Checking::Latest { open }
        } else {
            Checking::Older { open }
        }
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

/// The transaction open in `held` ended, and the models its `begin` kept with `rebuilds`
/// let go: what `begin` kept, if one was open. Every way a transaction ends (a commit, one
/// that writes nothing, a rollback) goes through here, with both locks held, so the two
/// halves of what `begin` kept are never apart.
fn end_transaction<C: Kernel + 'static>(
    held: &mut Held,
    rebuilds: &mut Rebuilds<C>,
) -> Option<(Snapshot, Kept)> {
    let snapshot = held.transaction.take();
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
        held.transaction = Some(held.snapshot());
        Ok(Begun { base: revision })
    }

    /// The transaction open, or the refusal of a command that needs one.
    fn transaction(&self, held: &Held) -> Result<Snapshot, Problems> {
        held.transaction
            .clone()
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
            Saving {
                path: held.path.clone(),
                text: Arc::clone(&held.text),
                gated: Arc::clone(&held.gated),
                revision: held.revision,
                carried: Arc::clone(&held.carried),
                begun,
                changed_outside,
            }
        };
        self.save_state(stop, state, Through::Transaction, read, claim)
            .map(|(committed, _)| committed)
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
        let state = {
            let held = self.held.read();
            Saving {
                path: held.path.clone(),
                text: candidate.text,
                gated: candidate.gated,
                revision: held.next()?,
                carried: Arc::clone(&held.carried),
                begun: held.snapshot(),
                // Outside a transaction, a change found before the command was reloaded.
                changed_outside: None,
            }
        };
        let committed = self.save_state(stop, state, Through::Candidate, read, claim);
        if committed.is_err() {
            *read = None;
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
    /// held changed, as [`commit`](Self::commit) is. As the command `stop` stops.
    fn save_state(
        self: &Arc<Self>,
        stop: &Ticket,
        state: Saving,
        through: Through,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
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
        // aren't in ID order is the one its canonical text reads as.
        let unchanged = match (begun.gated.document(), gated.document()) {
            (Ok(before), Ok(now)) => before.canonically_eq(now),
            _ => begun.text == text,
        };
        // Found before the command, so before any rebuild for it. A commit that would write
        // nothing loses nothing, and ends as one does, without waiting for a rebuild: its
        // problems the gate's alone.
        if let Some(outside) = changed_outside {
            if !unchanged {
                return Err(Problems::from(match outside {
                    Outside::ChangedOrGone => saver.conflict(&path),
                    Outside::Unreadable(error) => error,
                }));
            }
            // Only a transaction's commit is given what was found outside.
            self.unwritten(stop, &begun, revision)?;
            return Ok((Committed::unwritten(path, gated.problems()), None));
        }
        let checked = match &*gated {
            Gated::Checked(checked) => checked,
            refused if unchanged => {
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
        if unchanged {
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
        )?;
        Ok((
            Committed {
                written: true,
                path,
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
            // as it checks, never the revision (`Reached::committing`).
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
    /// text. Refused, writing nothing, inside a transaction, for a document the gate
    /// refused (its problems), for a feature in `accept` the document hasn't, and as a
    /// commit is. A kernel fault is saved around, as a commit saves around it. As the command
    /// `stop` stops.
    pub(super) fn format(
        self: &Arc<Self>,
        stop: &Ticket,
        accept: Option<&[FeatureId]>,
        write: bool,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
    ) -> Result<Reformatted, Problems> {
        let (path, bytes, gated, revision, reaching) = {
            let held = self.held.read();
            if held.transaction.is_some() {
                return Err(Problems::from(transaction_open(self.id, "format")));
            }
            (
                held.path.clone(),
                Arc::clone(&held.bytes),
                Arc::clone(&held.gated),
                held.revision,
                held.reaching(),
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
    /// its real result).
    fn write(
        &self,
        stop: &Ticket,
        save: Save<'_>,
        saver: Saver,
        claim: &dyn Fn(&Path),
    ) -> Result<Arc<Gated>, Problems> {
        let Save {
            text: saved,
            model,
            reach,
            publish,
        } = save;
        let (next, path, resolved, bytes) = {
            let held = self.held.read();
            (
                held.next()?,
                held.path.clone(),
                held.resolved.clone(),
                Arc::clone(&held.bytes),
            )
        };
        self.save(stop, &path, &resolved, &bytes, &saved, saver)?;
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
        held.bytes = saved;
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
    /// Inside a transaction, only for `commit` ([`Rereads::Always`]): nothing is reloaded,
    /// and a file changed (longer than a document may be, or gone) or unreadable is found
    /// for the commit to refuse, if it would write ([`Reload::Outside`]).
    ///
    /// A file this session's own save left part written isn't compared, nor reloaded (the
    /// next save says where its old text is), until it holds what the session last read or
    /// wrote again: the old text put back, which clears the mark.
    pub(super) fn reload(
        &self,
        rereads: Rereads,
        claim: &dyn Fn(Claim<'_>) -> Result<(), Unclaimed>,
    ) -> Result<Reload, Problems> {
        let (path, resolved, bytes, transaction, part_written) = {
            let held = self.held.read();
            (
                held.path.clone(),
                held.resolved.clone(),
                Arc::clone(&held.bytes),
                held.transaction.is_some(),
                held.part_written.is_some(),
            )
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
                self.held.session_changing().part_written = None;
                tracing::info!(session = %self.id, "the file a save left part written holds its old text again");
            }
            return Ok(Reload::AsHeld);
        }
        if transaction {
            let outside = match found {
                Ok((opened, now)) if now == *bytes => {
                    claimed(opened)?;
                    return Ok(Reload::AsHeld);
                }
                // Other bytes, or more than a document may be.
                Ok(_) => Outside::ChangedOrGone,
                Err(_) if gone() => Outside::ChangedOrGone,
                Err(error) => Outside::Unreadable(unread_by_commit(&path, &error)),
            };
            tracing::info!(session = %self.id, "the file changed outside the session, inside a transaction: not reloaded");
            return Ok(Reload::Outside(outside));
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
        // Nothing carried: a reloaded session is as a fresh `open`.
        held.change(next, Arc::clone(&now), gated, Arc::new(BTreeSet::new()));
        held.bytes = now;
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
    /// old text is. A save that leaves it so is marked, for the next to say.
    fn save(
        &self,
        stop: &Ticket,
        path: &str,
        resolved: &Path,
        bytes: &[u8],
        text: &[u8],
        saver: Saver,
    ) -> Result<(), Problems> {
        if let Some(old) = &self.held.read().part_written {
            return Err(Problems::from(part_written(path, old)));
        }
        stop.settle()?;
        let within = self.workspace.as_ref().map(|workspace| file::Within {
            workspace,
            given: Path::new(path),
        });
        file::save_within(resolved, bytes, text, within)
            .map_err(|unsaved| self.unsaved(path, unsaved, saver))
    }

    /// The refusal of a save of the document at `path` (as `open` was given it) that failed,
    /// as `unsaved` says: a conflict, naming the file, with what `saver` advises, or why it
    /// couldn't be written; and, if it left the file part written, that marked, for the next
    /// save to say.
    pub(super) fn unsaved(&self, path: &str, unsaved: file::Unsaved, saver: Saver) -> Problems {
        if let Some(old) = unsaved.old_text_in {
            // The session's record of its file, past the command's settling: it stands.
            self.held.session_changing().part_written = Some(old);
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
                    "nothing was written: roll back, then begin again from the file as it is now",
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

/// `error`, why the file at `path` (as `open` was given it) couldn't be read before a
/// transaction's `commit`: its `io_failed`, if the commit would write, naming the file.
fn unread_by_commit(path: &str, error: &Error) -> Error {
    const SUGGESTION: &str = "nothing was written: make it a regular file this user may read again, then commit; or roll back";
    of_file(path, error)
        .with_parameter("path")
        .with_suggestion(SUGGESTION)
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
/// 4).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Rereads {
    /// Outside a transaction only: inside one, the command runs on what the session holds.
    OutsideTransaction,
    /// Inside a transaction too, where nothing is reloaded: what's found is the command's
    /// (`commit`'s) to refuse.
    Always,
    /// Never: the commands that need nothing of the document, `close` (which always
    /// closes) and `cache_clear` (which clears the directory's cache whatever the file holds).
    Never,
}

/// What a session found of its file before a command ([`Session::reload`]), given to the
/// command.
pub(super) enum Reload {
    /// Nothing to change: the file unread (inside a transaction, or for a command that needs
    /// nothing of the document: `close`, `cache_clear`), or as the session last read or
    /// wrote it.
    AsHeld,
    /// Found changed outside the session, and reloaded: the response's `reloaded`.
    Reloaded,
    /// Inside a transaction, before `commit` ([`Rereads::Always`]): found not as the
    /// session last read or wrote it, and not reloaded, for the commit to refuse if it
    /// would write.
    Outside(Outside),
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
            Reload::Outside(outside) => Some(outside),
            Reload::AsHeld | Reload::Reloaded => None,
        }
    }
}

/// A file found, inside a transaction, not as its session last read or wrote it.
pub(super) enum Outside {
    /// It holds other bytes (more than a document may be, perhaps), or it's gone (nothing
    /// at its path, or a link there to nothing): a conflict.
    ChangedOrGone,
    /// It couldn't be read, as the error says: an `io_failed`.
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
