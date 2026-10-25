//! A session's rebuilds ([`Rebuilds`]): the model of a revision, waited for or as last
//! published, each rebuild started with a thread watching it ([`watch`]), and an edit's
//! candidate rebuilt for its commit.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, Weak};

use super::cancel::cancelled;
use super::threads::Threads;
#[cfg(doc)]
use super::transaction::Reaching;
use super::{
    Gated, ModelChoice, ModelRead, Problems, Published, Rebuild, RebuildState, Response, Revision,
    Session, SessionId, SessionInfo, not_open,
};
use crate::document::{Checked, FeatureId};
use crate::kernel::Kernel;
use crate::recompute::{Model, Recompute, Run, RunOutcome};
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// A session's rebuilds: its engine, on its own client of the kernel, keeping its steps in
/// its directory's cache, and what its rebuilds came to. A revision's rebuild starts when a
/// command first needs its model (decision 13), and a revision has one rebuild at a time.
/// The session keeps the models its rebuilds published itself, never relying on the
/// engine's newest (which a transaction's rebuild may make, the design's section 4).
///
/// The rule for its lock: take it, start (or cancel) a rebuild, let it go, and only then
/// wait, on `Session::rebuilt` (which lets it go). Never wait on a `Run` holding it, or
/// `Sessions::drop`'s cancel, and any other command's, waits behind the whole rebuild; a
/// watcher thread waits on each `Run`, holding only a `Weak` to the session. A command whose
/// rebuild ends superseded checks `Session::closed` before starting another, so a dropped
/// session's command stops rather than rebuilding again; and one cancelled (decision 34)
/// stops the rebuild it waits on and answers `cancelled`, rather than starting it again.
pub(super) struct Rebuilds<C: Kernel + 'static> {
    pub(super) engine: Recompute<C>,
    /// The rebuild running, if one is, labelled with the revision its model is of.
    pub(super) running: Option<Label>,
    /// The newest model the session's rebuilds published.
    pub(super) published: Option<Published>,
    /// How the newest rebuild that ended without publishing ended, until another starts.
    pub(super) ended: Option<Ended>,
    /// While a transaction is open, the models `begin` saw: what a rollback restores.
    pub(super) kept: Option<Kept>,
    /// No model of a revision before this one is published: a rollback's, so a rebuild of
    /// the transaction's documents that ends after it never takes the restored state's place.
    pub(super) floor: Revision,
    /// What the rebuild of an edit's candidate came to (an edit outside a transaction,
    /// [`Session::candidate_model`]), for its commit, which takes it: never published as
    /// it is (its save publishes it as the saved revision's, rebound to the saved
    /// document, when it writes no sketch's solve).
    pub(super) candidate: Option<(Label, Result<RunOutcome, Error>)>,
    /// The threads watching the rebuilds started: those ended joined as the next starts,
    /// the rest by `Sessions::drop`, for the sessions open then.
    pub(super) watchers: Threads,
}

impl<C: Kernel + 'static> Rebuilds<C> {
    /// The revision whose rebuild is running, if one is.
    pub(super) fn running(&self) -> Option<Revision> {
        self.running.as_ref().map(Label::revision)
    }

    /// `revision`'s rebuild ended as `how`, without publishing: kept unless a newer
    /// revision's end is, so one of an older revision that ends late never hides it.
    fn end(&mut self, revision: Revision, how: How) {
        if self
            .ended
            .as_ref()
            .is_none_or(|ended| ended.revision <= revision)
        {
            self.ended = Some(Ended { revision, how });
        }
    }

    /// What's become of `revision`'s rebuild (`status`): an edit's candidate running counts
    /// as `revision`'s, the session's revision until its save.
    pub(super) fn state(&self, revision: Revision) -> Rebuild {
        let running = self
            .running
            .as_ref()
            .is_some_and(|running| running.candidate || running.revision() == revision);
        let published = self
            .published
            .as_ref()
            .filter(|published| published.revision == revision);
        let state = if running {
            RebuildState::Running
        } else if let Some(published) = published {
            if published.model.fault().is_some() {
                RebuildState::Faulted
            } else {
                RebuildState::Done
            }
        } else {
            match &self.ended {
                Some(ended) if ended.cancelled(revision) => RebuildState::Stopped,
                Some(ended) if ended.failed(revision).is_some() => RebuildState::Faulted,
                Some(_) | None => RebuildState::None,
            }
        };
        Rebuild { revision, state }
    }
}

/// The revision a rebuild's model is of: shared by the session's rebuilds, while it's the
/// one running, and the thread watching it, so a rollback that gives the rebuild to the new
/// revision whose state equals its own gives it for good, however many rollbacks give it on
/// before it ends. Read and given only under the rebuilds' lock. A candidate's (an edit's
/// outside a transaction) is of the revision its save would give: its model is its
/// commit's, never published as it is. While it runs, the session's revision is still the
/// one before: so `status` says a candidate's is the current revision's rebuild running
/// (its edit being committed), not a rebuild of a revision to come.
#[derive(Clone)]
pub(super) struct Label {
    revision: Arc<AtomicU64>,
    candidate: bool,
    /// Whether the command waiting on it was cancelled and stopped it ([`stop`], which says
    /// how it ended then): if it ends superseded, that's why.
    cancelled: Arc<AtomicBool>,
}

impl Label {
    pub(super) fn new(revision: Revision) -> Self {
        Label {
            revision: Arc::new(AtomicU64::new(revision.0)),
            candidate: false,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The label of an edit's candidate, which its save would put the session at
    /// `revision`.
    fn candidate(revision: Revision) -> Self {
        Label {
            candidate: true,
            ..Label::new(revision)
        }
    }

    pub(super) fn revision(&self) -> Revision {
        Revision(self.revision.load(Ordering::SeqCst))
    }

    /// The rebuild given to `revision`: what it makes is that revision's.
    pub(super) fn give(&self, revision: Revision) {
        self.revision.store(revision.0, Ordering::SeqCst);
    }

    /// Whether `other` is this rebuild's label.
    fn is(&self, other: &Label) -> bool {
        Arc::ptr_eq(&self.revision, &other.revision)
    }
}

/// The models a transaction's `begin` saw, kept with the session's rebuilds (whose lock a
/// rebuild's watcher takes, so a model of the base revision published after `begin` is kept
/// too).
pub(super) struct Kept {
    /// The revision the transaction began from.
    pub(super) base: Revision,
    /// The newest model published when it began (of `base`, or an older revision).
    pub(super) published: Option<Published>,
    /// `base`'s model: as `begin` found it published, or as its rebuild, running then,
    /// published it since.
    pub(super) model: Option<Arc<Model>>,
    /// Whether a rebuild started inside the transaction, which may have superseded
    /// `base`'s: if so, and `base`'s model never came, a rollback starts the restored
    /// document's rebuild; if not, it starts none.
    pub(super) started: bool,
}

/// How a revision's rebuild ended without publishing a model (the plan's `ended`), until
/// another rebuild starts.
pub(super) struct Ended {
    revision: Revision,
    how: How,
}

/// How a rebuild ended without publishing.
pub(super) enum How {
    /// A newer one started first: whoever needs its model starts it again.
    Superseded,
    /// The command waiting on it was cancelled (decision 34): the next queued command that
    /// needs its model starts it again; a published read doesn't (the app's next frame
    /// would undo the stop).
    Cancelled,
    /// Its recompute failed (a bug), and why: said once, to the command waiting on it or
    /// the next to need its model; one after that rebuilds it. A published read never
    /// starts it again (it would fail again, each frame), and refuses with `why`.
    Stopped { why: Error, said: bool },
}

impl Ended {
    /// Why `revision`'s recompute failed, if its rebuild ended so.
    fn failed(&self, revision: Revision) -> Option<&Error> {
        match &self.how {
            How::Stopped { why, .. } if self.revision == revision => Some(why),
            How::Stopped { .. } | How::Superseded | How::Cancelled => None,
        }
    }

    /// Whether `revision`'s rebuild was cancelled.
    fn cancelled(&self, revision: Revision) -> bool {
        self.revision == revision && matches!(self.how, How::Cancelled)
    }
}

impl<C: Kernel + 'static> Session<C> {
    /// The model of `revision`, whose document is `checked`: the one its rebuild published,
    /// waited for, the rebuild started if none of it is running (decision 13). A model that
    /// holds a kernel fault (a worker that crashed or hung) is the revision's outcome all
    /// the same, kept, never rebuilt for another read of the revision (decision 32: a feature
    /// that crashes the worker every time would cost every read a minute). Refused if the
    /// session is dropped meanwhile (its rebuild stopped), if the recompute failed (a bug),
    /// or if the command reading it is cancelled (decision 34), which stops the rebuild it
    /// was waiting on (the next command that needs the model starts it again).
    pub(super) fn model(
        self: &Arc<Self>,
        revision: Revision,
        checked: &Arc<Checked>,
    ) -> Result<Arc<Model>, Problems> {
        let _span = tracing::info_span!("rebuild", revision = revision.number()).entered();
        let ticket = self.ticket();
        let mut rebuilds = lock(&self.rebuilds);
        // The rebuild this read last waited on, which a cancel stops.
        let mut waited: Option<Label> = None;
        loop {
            // Had, it's given even to a command cancelled meanwhile, whose work then stops
            // where it would leave a trace (`Session::keep`).
            if let Some(published) = &rebuilds.published
                && published.revision == revision
            {
                return Ok(Arc::clone(&published.model));
            }
            if let Some(ticket) = &ticket
                && ticket.cancelled()
            {
                if let Some(label) = &waited {
                    stop(&mut rebuilds, label);
                }
                return Err(Problems::from(cancelled(ticket.command())));
            }
            if let Some(running) = &rebuilds.running
                && running.revision() == revision
            {
                waited = Some(running.clone());
                rebuilds = self
                    .rebuilt
                    .wait(rebuilds)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            // A failure of this revision's, said once; the read after that rebuilds it.
            if let Some(Ended {
                revision: of,
                how: How::Stopped { why, said },
            }) = &mut rebuilds.ended
                && *of == revision
                && !*said
            {
                *said = true;
                return Err(Problems::from(why.clone()));
            }
            // Not built, or superseded: started again, unless the session has gone.
            if !self.start_unless_newer(&mut rebuilds, revision, checked)? {
                return Err(Problems::from(
                    Error::new(
                        ErrorCode::Internal,
                        format!(
                            "session {} was asked for revision {}'s model after a newer revision's rebuild",
                            self.id,
                            revision.number()
                        ),
                    )
                    .with_suggestion(crate::worker::SILICO_BUG),
                ));
            }
        }
    }

    /// Starts `revision`'s rebuild, whose document is `checked`, unless a newer revision's
    /// rebuild is running or has published (whose model it would supersede, or come after):
    /// whether it started. Refused if the session has closed (the sessions dropped, which
    /// stopped its rebuilds: none starts after). The caller decides whether the revision's
    /// model is wanted (none of it, and no rebuild of it running). Doesn't wait for it: a
    /// watcher thread records what it comes to. Takes the lock's guard, so the start and the
    /// record of it are one step.
    pub(super) fn start_unless_newer(
        self: &Arc<Self>,
        rebuilds: &mut Rebuilds<C>,
        revision: Revision,
        checked: &Arc<Checked>,
    ) -> Result<bool, Problems> {
        if self.closed() {
            return Err(Problems::from(not_open(self.id)));
        }
        if rebuilds.running().is_some_and(|running| running > revision)
            || rebuilds
                .published
                .as_ref()
                .is_some_and(|published| published.revision > revision)
        {
            return Ok(false);
        }
        rebuilds.ended = None;
        if let Some(kept) = &mut rebuilds.kept {
            kept.started = true;
        }
        self.spawn_rebuild(rebuilds, checked, Label::new(revision))?;
        Ok(true)
    }

    /// Starts a rebuild of `checked` on the session's engine, labelled `label`, with a
    /// thread watching it ([`watch`]) that records what it comes to: the rebuild running
    /// from here on. Takes the lock's guard, so the start and the record of it are one
    /// step; doesn't wait for it. Refused, with no rebuild running, if the watcher can't
    /// start.
    fn spawn_rebuild(
        self: &Arc<Self>,
        rebuilds: &mut Rebuilds<C>,
        checked: &Arc<Checked>,
        label: Label,
    ) -> Result<(), Problems> {
        rebuilds.watchers.reap();
        let run = rebuilds.engine.start(Arc::clone(checked));
        rebuilds.running = Some(label.clone());
        let session = Arc::downgrade(self);
        match std::thread::Builder::new()
            .name(format!("session-{}-rebuild", self.id))
            .spawn(move || watch(&session, &label, run))
        {
            Ok(watcher) => {
                rebuilds.watchers.push(watcher);
                Ok(())
            }
            Err(e) => {
                rebuilds.running = None;
                rebuilds.engine.cancel();
                Err(Problems::from(crate::pool::thread_error(
                    "rebuild watcher",
                    &e,
                )))
            }
        }
    }

    /// The current revision's model, waited for ([`model`](Self::model)), what was read,
    /// and what the session's edits reach at that revision ([`Reaching::reach`]); or why
    /// there's none: the gate refused the document (its problems, the document's, which a
    /// check gives as its result), or the model couldn't be had.
    pub(super) fn current(self: &Arc<Self>) -> Result<Current, Unbuilt> {
        let (revision, gated, reaching) = {
            let held = lock(&self.held);
            (held.revision, Arc::clone(&held.gated), held.reaching())
        };
        let reached = reaching.reach();
        match &*gated {
            Gated::Checked(checked) => self
                .model(revision, checked)
                .map(|model| Current {
                    read: ModelRead::of(revision, ModelChoice::Current, &model),
                    model,
                    reached,
                })
                .map_err(Unbuilt::Unavailable),
            refused => Err(Unbuilt::Gated(refused.problems())),
        }
    }

    /// The published read of the session as it is now ([`published_read`](Self::published_read)),
    /// with the session as it was read. Refused, of no session, if it has closed. Its
    /// newest published model, if there's one, even when the gate refused the current
    /// revision's document (the model's revision says it's an older one's; the gate's problems
    /// are `check`'s); with none, the gate's (or the reader's) problems for a document it
    /// refused, starting nothing.
    pub(super) fn published_now(
        self: &Arc<Self>,
    ) -> (Option<SessionInfo>, Result<Published, Unbuilt>) {
        let (info, revision, gated) = {
            let held = lock(&self.held);
            (
                self.info_of(&held, false),
                held.revision,
                Arc::clone(&held.gated),
            )
        };
        // Of a session not open, as every refusal of one is: no session.
        if self.closed() {
            return (
                None,
                Err(Unbuilt::Unavailable(Problems::from(not_open(self.id)))),
            );
        }
        let read = match &*gated {
            Gated::Checked(checked) => self
                .published_read(revision, checked)
                .map_err(Unbuilt::Unavailable),
            refused => lock(&self.rebuilds)
                .published
                .clone()
                .filter(|published| published.revision <= revision)
                .ok_or_else(|| Unbuilt::Gated(refused.problems())),
        };
        (Some(info), read)
    }

    /// The design's published read of the session at `revision`, whose document is
    /// `checked`: the newest model published of `revision` or an older one, never waiting;
    /// `revision`'s rebuild started if nothing of it, or newer, is published or running, its
    /// recompute hasn't failed (a failure isn't started again each frame, but left for
    /// `check` to say once and rebuild), and it wasn't cancelled (a stop isn't undone by the
    /// app's next frame, but by the next queued command that needs the model). Refused with
    /// that failure, or, with nothing published, naming `model`.
    pub(super) fn published_read(
        self: &Arc<Self>,
        revision: Revision,
        checked: &Arc<Checked>,
    ) -> Result<Published, Problems> {
        let mut rebuilds = lock(&self.rebuilds);
        let failed = rebuilds
            .ended
            .as_ref()
            .and_then(|ended| ended.failed(revision))
            .cloned();
        let stopped = rebuilds
            .ended
            .as_ref()
            .is_some_and(|ended| ended.cancelled(revision));
        if failed.is_none()
            && !stopped
            && rebuilds.running() != Some(revision)
            && rebuilds
                .published
                .as_ref()
                .is_none_or(|published| published.revision < revision)
        {
            self.start_unless_newer(&mut rebuilds, revision, checked)?;
        }
        match rebuilds
            .published
            .as_ref()
            .filter(|published| published.revision <= revision)
        {
            Some(published) => Ok(published.clone()),
            None => Err(Problems::from(match failed {
                Some(failed) => failed,
                None if stopped => stopped_unpublished(self.id),
                None => nothing_published(self.id),
            })),
        }
    }

    /// The model of an edit's candidate, `checked`, which its save would put the session at
    /// `revision` with: rebuilt for the commit of an edit made outside a transaction, and
    /// waited for. Never published as it is: its document holds the fingerprints stored
    /// before the save, so it's the model neither of the state the save leaves (a check of
    /// it would say the fingerprints written drifted) nor of any the session was at; the
    /// save publishes it rebound to the saved document ([`write`](Self::write)). While it runs
    /// it's the session's rebuild running, labelled with `revision`, so a published read of
    /// the session's revision starts none that would supersede it. Refused if the session
    /// closes meanwhile (its rebuild stopped), if the recompute fails (a bug), or if the edit
    /// is cancelled (decision 34), which stops it.
    pub(super) fn candidate_model(
        self: &Arc<Self>,
        revision: Revision,
        checked: &Arc<Checked>,
    ) -> Result<Arc<Model>, Problems> {
        let _span = tracing::info_span!("rebuild", revision = revision.number(), candidate = true)
            .entered();
        let ticket = self.ticket();
        let stopped = |rebuilds: &mut Rebuilds<C>, label: Option<&Label>| {
            let ticket = ticket.as_ref().filter(|ticket| ticket.cancelled())?;
            if let Some(label) = label {
                stop(rebuilds, label);
            }
            Some(Problems::from(cancelled(ticket.command())))
        };
        let mut rebuilds = lock(&self.rebuilds);
        loop {
            if self.closed() {
                return Err(Problems::from(not_open(self.id)));
            }
            if let Some(cancelled) = stopped(&mut rebuilds, None) {
                return Err(cancelled);
            }
            rebuilds.candidate = None;
            let label = Label::candidate(revision);
            self.spawn_rebuild(&mut rebuilds, checked, label.clone())?;
            let outcome = loop {
                if let Some((ended, _)) = &rebuilds.candidate
                    && ended.is(&label)
                    && let Some((_, outcome)) = rebuilds.candidate.take()
                {
                    break outcome;
                }
                if let Some(cancelled) = stopped(&mut rebuilds, Some(&label)) {
                    return Err(cancelled);
                }
                rebuilds = self
                    .rebuilt
                    .wait(rebuilds)
                    .unwrap_or_else(PoisonError::into_inner);
            };
            match outcome {
                Ok(RunOutcome::Published(model)) => return Ok(model),
                // Stopped (the sessions dropped, which the loop's start says), or superseded:
                // started again.
                Ok(RunOutcome::Superseded) => {}
                Err(why) => return Err(Problems::from(why)),
            }
        }
    }
}

/// The current revision's model, as a command that waited for it reads it.
pub(super) struct Current {
    pub(super) model: Arc<Model>,
    pub(super) read: ModelRead,
    /// What the session's edits reach at its revision.
    pub(super) reached: BTreeSet<FeatureId>,
}

/// Why a session's model couldn't be read.
pub(super) enum Unbuilt {
    /// The gate refused the document: nothing is built of it, and these are its problems.
    Gated(Problems),
    /// No model could be had: the session closed or was dropped, its recompute failed, or
    /// (a published read) nothing's been published.
    Unavailable(Problems),
}

impl Unbuilt {
    pub(super) fn problems(self) -> Problems {
        match self {
            Unbuilt::Gated(problems) | Unbuilt::Unavailable(problems) => problems,
        }
    }
}

/// A published read's response: `then` on the model read, if there was one, the response
/// naming it; or why there wasn't.
pub(super) fn published_answer<T>(
    info: Option<SessionInfo>,
    read: Result<Published, Unbuilt>,
    then: impl FnOnce(&Published, ModelRead) -> Result<T, Problems>,
) -> Response<T> {
    match read {
        Ok(published) => {
            let read = ModelRead::of(published.revision, ModelChoice::Published, &published.model);
            Response {
                session: info,
                result: then(&published, read.clone()),
                model: Some(read),
            }
        }
        Err(unbuilt) => Response {
            session: info,
            model: None,
            result: Err(unbuilt.problems()),
        },
    }
}

/// Whether `model` holds a kernel fault: a failure that says nothing about the document,
/// which a rebuild may not meet again ([`Model::fault`]).
pub(in crate::command) fn faulted(model: &Model) -> bool {
    model.fault().is_some()
}

/// The rebuild labelled `label` stopped, if it's still the one running, for a command that
/// was waiting on it and was cancelled (decision 34): through the pool's superseded path,
/// as a newer generation's would, and running no more from here, its end said as cancelled
/// ([`How::Cancelled`]; a candidate's is its commit's alone), so the command's answer never
/// comes before what `status` says of it.
fn stop<C: Kernel + 'static>(rebuilds: &mut Rebuilds<C>, label: &Label) {
    if rebuilds
        .running
        .as_ref()
        .is_some_and(|running| running.is(label))
    {
        label.cancelled.store(true, Ordering::SeqCst);
        rebuilds.engine.cancel();
        rebuilds.running = None;
        if !label.candidate {
            rebuilds.end(label.revision(), How::Cancelled);
        }
    }
}

/// The refusal of a published read with nothing published yet, its rebuild started or
/// running: the design's section 2.
pub(super) fn nothing_published(id: SessionId) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("session {id} has published no model yet"),
    )
    .with_parameter("model")
    .with_suggestion(
        "read the current model, which waits for its rebuild, or the published one again \
         once the rebuild this read started is done",
    )
}

/// The refusal of a published read with nothing published, its revision's rebuild
/// cancelled: started again by a read of the current model, never by a published read.
fn stopped_unpublished(id: SessionId) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("session {id} has published no model yet: its rebuild was cancelled"),
    )
    .with_parameter("model")
    .with_suggestion("read the current model, which starts the rebuild again and waits for it")
}

/// Waits for a rebuild, `run`, then records what it came to in `session`'s rebuilds and
/// says so, if the session is still there: as the revision its `label` says now (the last a
/// rollback gave it to, if any did); and, if it's still the rebuild running, as running no
/// more (one stopped and started again, or another's since, is left running).
pub(super) fn watch<C: Kernel + 'static>(session: &Weak<Session<C>>, label: &Label, run: Run) {
    let outcome = run.wait();
    let Some(session) = session.upgrade() else {
        return;
    };
    let mut rebuilds = lock(&session.rebuilds);
    let revision = label.revision();
    if rebuilds
        .running
        .as_ref()
        .is_some_and(|running| running.is(label))
    {
        rebuilds.running = None;
    }
    if label.candidate {
        // An edit's candidate: its commit's, waiting for it, never anyone else's.
        rebuilds.candidate = Some((label.clone(), outcome));
        drop(rebuilds);
        session.rebuilt.notify_all();
        return;
    }
    match outcome {
        // A late model of an older revision never takes a newer one's place, nor one of a
        // transaction rolled back the restored state's.
        Ok(RunOutcome::Published(model)) => {
            // The model of the revision a transaction began from, if `begin` came while it
            // was rebuilding: what a rollback restores.
            if let Some(kept) = &mut rebuilds.kept
                && kept.base == revision
                && kept.model.is_none()
            {
                kept.model = Some(Arc::clone(&model));
            }
            if revision >= rebuilds.floor
                && rebuilds
                    .published
                    .as_ref()
                    .is_none_or(|newest| newest.revision <= revision)
            {
                rebuilds.published = Some(Published { revision, model });
            }
        }
        // Whoever needs its model starts it again. One a cancel stopped said so as it
        // stopped it ([`stop`]): said again now, it could hide what a rebuild started since
        // comes to.
        Ok(RunOutcome::Superseded) => {
            if !label.cancelled.load(Ordering::SeqCst) {
                rebuilds.end(revision, How::Superseded);
            }
        }
        Err(why) => rebuilds.end(revision, How::Stopped { why, said: false }),
    }
    drop(rebuilds);
    session.rebuilt.notify_all();
}
