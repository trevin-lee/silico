//! A session's rebuilds ([`Rebuilds`]): the model of a revision, waited for or as last
//! published, each rebuild started with a thread watching it ([`watch`]), and an edit's
//! candidate rebuilt for its commit.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use super::cancel::{Ticket, cancelled};
use super::threads::Threads;
use super::transaction::{Reached, Reaching};
use super::{
    Gated, ModelChoice, ModelRead, Problems, Progress, Published, Rebuild, RebuildState, Response,
    Revision, Session, SessionId, SessionInfo, not_open,
};
use crate::cache::Cache;
use crate::document::Checked;
use crate::kernel::Kernel;
use crate::recompute::{Model, Reads, Recompute, Run, RunOutcome, StepCounter};
use crate::worker::{Advice, lock};
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
    /// The cache the engine keeps its steps in, beside the session's document: what
    /// `cache_clear` clears.
    pub(super) cache: Cache,
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
    /// How many rebuilds it has started: the number of the newest ([`Label::number`]).
    pub(super) started: u64,
}

impl<C: Kernel + 'static> Rebuilds<C> {
    /// The revision whose rebuild is running, if one is.
    pub(super) fn running(&self) -> Option<Revision> {
        self.running.as_ref().map(Label::revision)
    }

    /// Whether the command `stop` stops was cancelled: if so, the rebuild it waits on,
    /// `waited`, stopped ([`stop_waited`](Self::stop_waited)), and its `cancelled` answer.
    fn stopped(&mut self, stop: &Ticket, waited: Option<&Label>) -> Result<(), Problems> {
        if !stop.cancelled() {
            return Ok(());
        }
        if let Some(label) = waited {
            self.stop_waited(label);
        }
        Err(Problems::from(cancelled(stop.name())))
    }

    /// `label` numbered as the newest rebuild started, after every one started before it.
    pub(super) fn numbered(&mut self, label: Label) -> Label {
        self.started = self.started.saturating_add(1);
        Label {
            number: self.started,
            ..label
        }
    }

    /// Whether a rebuild of `revision` that started after the one labelled `by` has said
    /// what it came to: published its model, or ended without one. If so, what `by`'s comes
    /// to, recorded late, never takes its place (a fresh check's rebuild, say, ending before
    /// the rebuild it superseded is recorded: the revision's model stays the fresh one's).
    fn said_later(&self, revision: Revision, by: &Label) -> bool {
        let later = |other: &Label| other.number > by.number;
        self.published.as_ref().is_some_and(|published| {
            published.revision == revision && published.by.as_ref().is_some_and(later)
        }) || self
            .ended
            .as_ref()
            .is_some_and(|ended| ended.revision == revision && later(&ended.by))
    }

    /// The rebuild labelled `label` stopped, if it's still the one running, for a command
    /// that was waiting on it and was cancelled (decision 34): through the pool's superseded
    /// path, as a newer generation's would, and running no more from here, its end said as
    /// cancelled ([`Ending::Cancelled`]; a candidate's is its commit's alone), so the command's
    /// answer never comes before what `status` says of it.
    fn stop_waited(&mut self, label: &Label) {
        if self
            .running
            .as_ref()
            .is_some_and(|running| running.is(label))
        {
            label.cancelled.store(true, Ordering::SeqCst);
            self.engine.cancel();
            self.running = None;
            if !label.candidate {
                self.end(label, Ending::Cancelled);
            }
        }
    }

    /// The rebuild labelled `by` ended as `how`, without publishing, as of the revision its
    /// label says now: kept unless a newer revision's end is, so one of an older revision
    /// that ends late never hides it; nor what a rebuild of the same revision started after
    /// it has said ([`said_later`](Self::said_later)), so a late end of one it superseded
    /// never hides that rebuild's model, failure, or stop.
    pub(super) fn end(&mut self, by: &Label, how: Ending) {
        let revision = by.revision();
        if self
            .ended
            .as_ref()
            .is_none_or(|ended| ended.revision <= revision)
            && !self.said_later(revision, by)
        {
            self.ended = Some(Ended {
                revision,
                how,
                by: by.clone(),
            });
        }
    }

    /// What's become of `revision`'s rebuild (`status`): an edit's candidate running counts
    /// as `revision`'s, the session's revision until its save. Its progress is left for the
    /// caller to read ([`Newest::of`]), once this lock is let go.
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
                Some(_) | None => RebuildState::Idle,
            }
        };
        Rebuild {
            revision,
            state,
            progress: None,
        }
    }
}

/// How far a session's newest rebuild has got: its label and its recompute's count, under a
/// lock of its own, taken after every other and held only to read or replace them, so
/// `status` and a command's handle read it without waiting on any of the session's work.
#[derive(Default)]
pub(super) struct Newest(Mutex<Option<(Label, StepCounter)>>);

impl Newest {
    /// The rebuild labelled `label` started, counted by `progress`.
    fn started(&self, label: Label, progress: StepCounter) {
        *lock(&self.0) = Some((label, progress));
    }

    /// The progress of `revision`'s rebuild, if the newest is that one: the revision it's of
    /// (as a rollback gave it on), or an edit's candidate still running, which counts as the
    /// session's revision until its save. Its count when it ended, once it has.
    pub(super) fn of(&self, revision: Revision) -> Option<Progress> {
        let newest = lock(&self.0);
        let (label, progress) = newest.as_ref()?;
        let count = progress.now();
        (label.revision() == revision || (label.candidate && !count.ended))
            .then(|| Progress::from(&count))
    }

    /// The progress of the newest rebuild, while it runs.
    pub(super) fn running(&self) -> Option<Progress> {
        let count = lock(&self.0).as_ref()?.1.now();
        (!count.ended).then(|| Progress::from(&count))
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
#[derive(Clone, Debug)]
pub(super) struct Label {
    revision: Arc<AtomicU64>,
    /// What the session's edits reached at the revision it's of, published with its model:
    /// the same at a revision a rollback gives it to, whose state is the one it was of.
    /// None reached for a candidate's, which is never published as it is. Carried, never
    /// worked out, by the rebuild: whoever reads its model works it out, if anyone does.
    reached: Reached,
    candidate: bool,
    /// Whether the command waiting on it was cancelled and stopped it
    /// ([`Rebuilds::stop_waited`], which says how it ended then): if it ends superseded,
    /// that's why.
    cancelled: Arc<AtomicBool>,
    /// What it reads of what was kept: nothing for `check`'s `fresh`, every step built anew
    /// ([`Recompute::start_fresh`]).
    reads: Reads,
    /// Its place among the session's rebuilds, numbered as each starts
    /// ([`Rebuilds::numbered`]): what a rebuild started later says of its revision is never
    /// replaced by what this one comes to. Zero until it starts.
    number: u64,
}

impl Label {
    /// The label of `revision`'s rebuild, what the session's edits reached at it `reached`.
    pub(super) fn new(revision: Revision, reached: Reached) -> Self {
        Label {
            revision: Arc::new(AtomicU64::new(revision.0)),
            reached,
            candidate: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            reads: Reads::Kept,
            number: 0,
        }
    }

    /// The label of `revision`'s rebuild reading what was kept as `reads` says (nothing,
    /// for `check`'s `fresh`), what the session's edits reached at it `reached`.
    fn reading(revision: Revision, reached: Reached, reads: Reads) -> Self {
        Label {
            reads,
            ..Label::new(revision, reached)
        }
    }

    /// The label of an edit's candidate, which its save would put the session at
    /// `revision`.
    fn candidate(revision: Revision) -> Self {
        Label {
            candidate: true,
            ..Label::new(revision, Reached::default())
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

/// How a revision's rebuild ended without publishing a model, until another rebuild starts.
pub(super) struct Ended {
    revision: Revision,
    how: Ending,
    /// The rebuild that ended: a fresh read answers only with its own rebuild's failure, and
    /// what a rebuild started after it comes to replaces it, never one started before.
    by: Label,
}

/// How a rebuild ended without publishing. Each is kept as any end is ([`Rebuilds::end`]):
/// over the end of its own revision or an older one, never a newer one's, nor over what a
/// rebuild of its revision started after it said.
pub(super) enum Ending {
    /// A newer one started first: whoever needs its model starts it again. One of an older
    /// revision, ending late, never hides a newer revision's failure or stop.
    Superseded,
    /// The command waiting on it was cancelled (decision 34): the next queued command that
    /// needs its model starts it again; a published read doesn't (the app's next frame
    /// would undo the stop). `status` says `stopped`.
    Cancelled,
    /// Its recompute failed (a bug), and why: said once, to the command waiting on it or
    /// the next to need its model; one after that rebuilds it. A published read never
    /// starts it again (it would fail again, each frame), and refuses with `why`. `status`
    /// says `faulted`, as of a model that holds a kernel fault.
    Failed { why: Error, said: bool },
}

impl Ended {
    /// Why `revision`'s recompute failed, if its rebuild ended so.
    fn failed(&self, revision: Revision) -> Option<&Error> {
        match &self.how {
            Ending::Failed { why, .. } if self.revision == revision => Some(why),
            Ending::Failed { .. } | Ending::Superseded | Ending::Cancelled => None,
        }
    }

    /// Whether `revision`'s rebuild was cancelled.
    fn cancelled(&self, revision: Revision) -> bool {
        self.revision == revision && matches!(self.how, Ending::Cancelled)
    }
}

impl<C: Kernel + 'static> Session<C> {
    /// The model of `revision`, whose document is `checked` and whose features the session's
    /// edits reached `reached` (published with it), for the command `stop` stops:
    /// the one its rebuild published, waited for, the rebuild started if none of it is
    /// running (decision 13). A model that
    /// holds a kernel fault (a worker that crashed or hung) is the revision's outcome all
    /// the same, kept, never rebuilt for another read of the revision (decision 32: a feature
    /// that crashes the worker every time would cost every read a minute). Refused if the
    /// session is dropped meanwhile (its rebuild stopped), if the recompute failed (a bug),
    /// or if the command reading it is cancelled (decision 34), which stops the rebuild it
    /// was waiting on (the next command that needs the model starts it again).
    pub(super) fn model(
        self: &Arc<Self>,
        stop: &Ticket,
        revision: Revision,
        checked: &Arc<Checked>,
        reached: &Reached,
    ) -> Result<Arc<Model>, Problems> {
        self.model_with(stop, revision, checked, reached, Reads::Kept)
    }

    /// [`model`](Self::model), or, reading [`Reads::Nothing`], the model of a rebuild of
    /// `revision` that reads nothing kept (`check`'s `fresh`, the design's section 7: every
    /// step built anew,
    /// [`Recompute::start_fresh`]), started now whatever the revision has: never the model
    /// published of it before (a kernel fault kept as its outcome is tried again, decision
    /// 32), nor one of a rebuild running that reads what's kept, which it supersedes, even
    /// one its engine published before the fresh rebuild started and its watcher records
    /// after: only its own rebuild's model ([`Published::by`]), or its own failure. Its
    /// model, once published, is the revision's, in the place of the one before.
    pub(super) fn model_with(
        self: &Arc<Self>,
        stop: &Ticket,
        revision: Revision,
        checked: &Arc<Checked>,
        reached: &Reached,
        reads: Reads,
    ) -> Result<Arc<Model>, Problems> {
        let fresh = reads == Reads::Nothing;
        let _span = tracing::info_span!("rebuild", revision = revision.number(), fresh).entered();
        let mut rebuilds = lock(&self.rebuilds);
        // The rebuild this read last waited on or started, which a cancel stops.
        let mut waited: Option<Label> = None;
        // A fresh read's own rebuild, once started: only what it comes to answers the read.
        let mut own: Option<Label> = None;
        // Whether what the rebuild labelled `by` came to answers this read: what any of the
        // revision's rebuilds came to, for a read of what's kept; for a fresh one, only
        // what its own did. Never the model published before it started (a kept fault is
        // tried again), nor one of a rebuild it superseded that its engine had published
        // before the fresh one started, and whose watcher records it only after.
        let answers = |own: &Option<Label>, by: Option<&Label>| {
            !fresh || own.as_ref().zip(by).is_some_and(|(own, by)| by.is(own))
        };
        loop {
            // Had, it's given even to a command cancelled meanwhile, whose work then stops
            // where it would leave a trace (its stop settled).
            if let Some(published) = &rebuilds.published
                && published.revision == revision
                && answers(&own, published.by.as_ref())
            {
                return Ok(Arc::clone(&published.model));
            }
            rebuilds.stopped(stop, waited.as_ref())?;
            if let Some(running) = &rebuilds.running
                && running.revision() == revision
                && (!fresh || own.as_ref().is_some_and(|own| running.is(own)))
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
                how: Ending::Failed { why, said },
                by,
            }) = &mut rebuilds.ended
                && *of == revision
                && !*said
                && answers(&own, Some(&*by))
            {
                *said = true;
                return Err(Problems::from(why.clone()));
            }
            // Not built, or superseded: started again, unless the session has gone; and
            // stopped by a cancel that comes before the wait does. One that comes after the
            // stop check above and before this start costs a rebuild started and stopped at
            // once (the next turn of the loop stops what it started): accepted, since
            // checking again here would only narrow that window, never close it. A fresh
            // read starts its own, superseding any running.
            let label = Label::reading(revision, reached.clone(), reads);
            if let Some(started) = self.start_labelled(&mut rebuilds, checked, label)? {
                if fresh {
                    own = Some(started.clone());
                }
                waited = Some(started);
            } else {
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

    /// Starts `revision`'s rebuild, whose document is `checked` (what the session's edits
    /// reached at it `reached`, published with its model), unless a newer revision's
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
        reached: &Reached,
    ) -> Result<bool, Problems> {
        self.start_labelled(rebuilds, checked, Label::new(revision, reached.clone()))
            .map(|started| started.is_some())
    }

    /// [`start_unless_newer`](Self::start_unless_newer), the rebuild labelled `label` (its
    /// revision, what the session's edits reached at it, and whether it reads what's kept):
    /// its label as it started, numbered ([`Rebuilds::numbered`]), if it did.
    fn start_labelled(
        self: &Arc<Self>,
        rebuilds: &mut Rebuilds<C>,
        checked: &Arc<Checked>,
        label: Label,
    ) -> Result<Option<Label>, Problems> {
        let revision = label.revision();
        if self.closed() {
            return Err(Problems::from(not_open(self.id)));
        }
        if rebuilds.running().is_some_and(|running| running > revision)
            || rebuilds
                .published
                .as_ref()
                .is_some_and(|published| published.revision > revision)
        {
            return Ok(None);
        }
        rebuilds.ended = None;
        if let Some(kept) = &mut rebuilds.kept {
            kept.started = true;
        }
        self.spawn_rebuild(rebuilds, checked, label).map(Some)
    }

    /// Starts a rebuild of `checked` on the session's engine, labelled `label`, with a
    /// thread watching it ([`watch`]) that records what it comes to: the rebuild running
    /// from here on, numbered as the newest started, its label as numbered given back. Takes
    /// the lock's guard, so the start and the record of it are one step; doesn't wait for
    /// it. Refused, with no rebuild running, if the watcher can't start.
    fn spawn_rebuild(
        self: &Arc<Self>,
        rebuilds: &mut Rebuilds<C>,
        checked: &Arc<Checked>,
        label: Label,
    ) -> Result<Label, Problems> {
        let label = rebuilds.numbered(label);
        let started = label.clone();
        rebuilds.watchers.reap();
        let run = rebuilds.engine.begin(Arc::clone(checked), label.reads);
        self.newest.started(label.clone(), run.progress());
        rebuilds.running = Some(label.clone());
        let session = Arc::downgrade(self);
        match std::thread::Builder::new()
            .name(format!("session-{}-rebuild", self.id))
            .spawn(move || watch(&session, &label, run))
        {
            Ok(watcher) => {
                rebuilds.watchers.push(watcher);
                Ok(started)
            }
            Err(e) => {
                rebuilds.running = None;
                rebuilds.engine.cancel();
                Err(Problems::from(crate::worker::thread_error(
                    "rebuild watcher",
                    &e,
                )))
            }
        }
    }

    /// The current revision's model, waited for by the command `stop` stops
    /// ([`model`](Self::model)), what was read, and what the session's edits reach at that
    /// revision, worked out only if it's asked for ([`Reached`]); or why there's none: the
    /// gate refused the document
    /// (its problems, the document's, which a check gives as its result), or the model
    /// couldn't be had.
    pub(super) fn current(self: &Arc<Self>, stop: &Ticket) -> Result<Current, Unbuilt> {
        self.current_with(stop, Reads::Kept)
    }

    /// [`current`](Self::current), or, reading [`Reads::Nothing`], the current revision
    /// rebuilt with every step built anew ([`model_with`](Self::model_with)), its model the
    /// revision's from then on.
    pub(super) fn current_with(
        self: &Arc<Self>,
        stop: &Ticket,
        reads: Reads,
    ) -> Result<Current, Unbuilt> {
        let (revision, gated, reaching) = {
            let held = self.held.read();
            (held.revision, Arc::clone(&held.gated), held.reaching())
        };
        let reached = Reached::of(reaching);
        match &*gated {
            Gated::Checked(checked) => self
                .model_with(stop, revision, checked, &reached, reads)
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
        let (info, revision, gated, reaching) = {
            let held = self.held.read();
            (
                self.info_of(&held, false),
                held.revision,
                Arc::clone(&held.gated),
                held.reaching(),
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
                .published_read(revision, checked, reaching)
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
    /// `checked`, and what the session's edits reach at it `reaching` (taken by its rebuild,
    /// if it starts, to publish with its model, and never worked out here: on the caller's
    /// thread, under the rebuilds' lock): the newest model published of `revision` or an
    /// older one, never waiting;
    /// `revision`'s rebuild started if nothing of it, or newer, is published or running, its
    /// recompute hasn't failed (a failure isn't started again each frame, but left for
    /// `check` to say once and rebuild), and it wasn't cancelled (a stop isn't undone by the
    /// app's next frame, but by the next queued command that needs the model). Refused with
    /// that failure, or, with nothing published, naming `model`.
    pub(super) fn published_read(
        self: &Arc<Self>,
        revision: Revision,
        checked: &Arc<Checked>,
        reaching: Reaching,
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
            self.start_unless_newer(&mut rebuilds, revision, checked, &Reached::of(reaching))?;
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
    /// `revision` with: rebuilt for the commit of an edit made outside a transaction, the
    /// command `stop` stops, and waited for. Never published as it is: its document holds the fingerprints stored
    /// before the save, so it's the model neither of the state the save leaves (a check of
    /// it would say the fingerprints written drifted) nor of any the session was at; the
    /// save publishes it rebound to the saved document ([`write`](Self::write)). While it runs
    /// it's the session's rebuild running, labelled with `revision`, so a published read of
    /// the session's revision starts none that would supersede it. Refused if the session
    /// closes meanwhile (its rebuild stopped), if the recompute fails (a bug), or if the edit
    /// is cancelled (decision 34), which stops it.
    pub(super) fn candidate_model(
        self: &Arc<Self>,
        stop: &Ticket,
        revision: Revision,
        checked: &Arc<Checked>,
    ) -> Result<Arc<Model>, Problems> {
        let _span = tracing::info_span!("rebuild", revision = revision.number(), candidate = true)
            .entered();
        let mut rebuilds = lock(&self.rebuilds);
        loop {
            if self.closed() {
                return Err(Problems::from(not_open(self.id)));
            }
            rebuilds.stopped(stop, None)?;
            rebuilds.candidate = None;
            let label = self.spawn_rebuild(&mut rebuilds, checked, Label::candidate(revision))?;
            let outcome = loop {
                if let Some((ended, _)) = &rebuilds.candidate
                    && ended.is(&label)
                    && let Some((_, outcome)) = rebuilds.candidate.take()
                {
                    break outcome;
                }
                rebuilds.stopped(stop, Some(&label))?;
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
    pub(super) reached: Reached,
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

/// What does `done` for a revision whose model holds a kernel fault, kept as its outcome
/// (decision 32), and what doesn't: so every suggestion that says how to try again says it
/// alike. A save that writes only fingerprints keeps the fault for the revision it saves
/// (its model, rebound, is that revision's).
pub(in crate::command) fn retried(done: &str) -> String {
    format!(
        "check with fresh, an edit that changes the document, or reopening the session, \
         {done}; a save that writes only fingerprints keeps the fault"
    )
}

/// A kernel fault as the model read holds it ([`ModelRead::fault`](super::ModelRead)): as
/// it was met, its suggestion the worker's own, but a worker that couldn't be had, which
/// says nothing of the feature that asked for it, naming none. A session's client words
/// how to try again its own way: the wire, and a session command refused with it, with
/// [`kept_fault`]; the CLI with its own.
pub(in crate::command) fn met_fault(fault: &Error) -> Error {
    match fault.code() {
        ErrorCode::WorkerUnavailable => fault.clone().without_feature(),
        _ => fault.clone(),
    }
}

/// A kernel fault as a session's client reads it (the envelope's `model.fault`, decision
/// 32), and as a command that needs a faulted feature's solid is refused with it: kept as
/// the revision's outcome, so its suggestion, the worker's own kept, goes on to say how to
/// try again ([`kept_suggestion`]).
pub(in crate::command) fn kept_fault(fault: &Error) -> Error {
    let fault = met_fault(fault);
    let suggestion = kept_suggestion(&fault);
    fault.with_suggestion(suggestion)
}

/// [`kept_fault`]'s suggestion (the design's section 6): reading the revision again won't try
/// it again, so how to, keeping the worker's own advice, read as the data it was worded from
/// ([`Advice`]), never by its words. A crash on a feature's values, which the same values
/// would crash again, says to edit them (an edit makes a new revision, which rebuilds
/// anyway), as the design's example does; with no feature named, the worker's advice alone.
/// A timeout says to simplify the operation, or `check` with `fresh`: the time allowed isn't
/// the command API's to give. Advice to try the request again says to `check` with `fresh`
/// in its place, so a retry is said once. Any other advice of the worker's own (the memory
/// it ran short of, its install), then `check` with `fresh`. With none of its own, a crash
/// that met a feature says `check` with `fresh`, then the document reported if the same
/// feature crashes the worker again; any other fault, `check` with `fresh` alone. (A fault
/// is never known to have met the same feature twice running: each model holds the one its
/// rebuild met.)
pub(in crate::command) fn kept_suggestion(fault: &Error) -> String {
    const FRESH: &str = "check with fresh to try it again";
    let Some(own) = fault.suggestion() else {
        return match (fault.code(), fault.feature()) {
            (ErrorCode::WorkerCrashed, Some(_)) => format!(
                "{FRESH}; if the same feature crashes the worker again, report it with the \
                 document"
            ),
            _ => FRESH.to_owned(),
        };
    };
    match (fault.advice(), fault.feature()) {
        (Some(Advice::ChangeTheValues { why }), Some(_)) => {
            format!(
                "edit the feature's values: {why}; an edit makes a new revision, which rebuilds it"
            )
        }
        (Some(Advice::ChangeTheValues { .. }), None) => own.to_owned(),
        (Some(Advice::SimplifyOrWait), _) => format!("simplify the operation, or {FRESH}"),
        (Some(Advice::TryAgain { once_more, why }), _) => {
            let again = if *once_more { "once more" } else { "again" };
            format!("check with fresh to try it {again}: {why}")
        }
        (Some(Advice::Other(_)) | None, _) => format!("{own}; then {FRESH}"),
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
/// rollback gave it to, if any did), unless a rebuild of that revision started after it has
/// said what it came to ([`Rebuilds::said_later`]); and, if it's still the rebuild running,
/// as running no more (one stopped and started again, or another's since, is left running).
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
        // transaction rolled back the restored state's, nor one of a rebuild superseded
        // (after its engine published) what a rebuild of its revision started after it said.
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
                && !rebuilds.said_later(revision, label)
            {
                rebuilds.published = Some(Published {
                    revision,
                    model,
                    reached: label.reached.clone(),
                    by: Some(label.clone()),
                });
            }
        }
        // Whoever needs its model starts it again. One a cancel stopped said so as it
        // stopped it ([`Rebuilds::stop_waited`]): said again now, it could hide what a
        // rebuild started since comes to.
        Ok(RunOutcome::Superseded) => {
            if !label.cancelled.load(Ordering::SeqCst) {
                rebuilds.end(label, Ending::Superseded);
            }
        }
        Err(why) => rebuilds.end(label, Ending::Failed { why, said: false }),
    }
    drop(rebuilds);
    session.rebuilt.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeClient, FakeKernel};
    use crate::kernel::Clients;

    /// A session's rebuilds as `start` makes them, with no rebuild run: nothing running,
    /// nothing published, and `ended` as each test sets it.
    fn rebuilds(kernel: &FakeKernel, ended: Option<Ended>) -> Rebuilds<FakeClient> {
        Rebuilds {
            engine: Recompute::new(kernel.client()),
            cache: Cache::unsealed("/nonexistent/.silico-cache"),
            running: None,
            published: None,
            ended,
            kept: None,
            floor: Revision::OPENED,
            candidate: None,
            watchers: Threads::new(),
            started: 0,
        }
    }

    fn failed() -> Ending {
        Ending::Failed {
            why: Error::new(ErrorCode::Internal, "the recompute failed"),
            said: false,
        }
    }

    const ONE: Revision = Revision(1);
    const TWO: Revision = Revision(2);

    /// The label of a rebuild of `revision` numbered as the newest `rebuilds` started.
    fn started(rebuilds: &mut Rebuilds<FakeClient>, revision: Revision) -> Label {
        rebuilds.numbered(Label::new(revision, Reached::default()))
    }

    fn states(rebuilds: &Rebuilds<FakeClient>) -> [RebuildState; 2] {
        [rebuilds.state(ONE).state, rebuilds.state(TWO).state]
    }

    #[test]
    fn an_end_replaces_its_own_revisions_and_an_older_ones_never_a_newer_ones() {
        // `end`: "kept unless a newer revision's end is". Status says `faulted` of a failed
        // end, `stopped` of a cancelled one, and nothing of a superseded one or another
        // revision's.
        use RebuildState::{Faulted, Idle, Stopped};
        let kernel = FakeKernel::new(1).unwrap();
        let mut rebuilds = rebuilds(&kernel, None);
        let first = started(&mut rebuilds, ONE);
        rebuilds.end(&first, failed());
        assert_eq!(states(&rebuilds), [Faulted, Idle]);
        // The same revision's later end replaces it.
        let again = started(&mut rebuilds, ONE);
        rebuilds.end(&again, Ending::Cancelled);
        assert_eq!(states(&rebuilds), [Stopped, Idle]);
        // A newer revision's replaces it.
        let second = started(&mut rebuilds, TWO);
        rebuilds.end(&second, failed());
        assert_eq!(states(&rebuilds), [Idle, Faulted]);
        // An older revision's, ending late, never hides it.
        let late = started(&mut rebuilds, ONE);
        rebuilds.end(&late, Ending::Cancelled);
        assert_eq!(states(&rebuilds), [Idle, Faulted]);
    }

    #[test]
    fn a_late_end_of_a_rebuild_started_before_never_replaces_a_later_ones_of_its_revision() {
        // Two rebuilds of revision 1, the second started after the first (a fresh check's,
        // superseding it). Whatever the first comes to, recorded after the second's end,
        // leaves the second's: its failure, its stop.
        use RebuildState::{Faulted, Idle, Stopped};
        let kernel = FakeKernel::new(1).unwrap();
        let mut rebuilds = rebuilds(&kernel, None);
        let before = started(&mut rebuilds, ONE);
        let after = started(&mut rebuilds, ONE);
        rebuilds.end(&after, failed());
        assert_eq!(states(&rebuilds), [Faulted, Idle]);
        // Superseded late: the failure stays, said as the second's.
        rebuilds.end(&before, Ending::Superseded);
        assert_eq!(states(&rebuilds), [Faulted, Idle]);
        // Failed late: the second's failure stays the one kept.
        rebuilds.end(&before, failed());
        let Some(ended) = &rebuilds.ended else {
            panic!("no end")
        };
        assert!(
            ended.by.is(&after),
            "the first's failure replaced the second's"
        );
        // The second's stop stays a stop, whatever the first's end.
        rebuilds.end(&after, Ending::Cancelled);
        assert_eq!(states(&rebuilds), [Stopped, Idle]);
        rebuilds.end(&before, failed());
        assert_eq!(states(&rebuilds), [Stopped, Idle]);
        rebuilds.end(&before, Ending::Superseded);
        assert_eq!(states(&rebuilds), [Stopped, Idle]);
        // A third, started after both, replaces it.
        let third = started(&mut rebuilds, ONE);
        rebuilds.end(&third, Ending::Superseded);
        assert_eq!(states(&rebuilds), [Idle, Idle]);
    }

    #[test]
    fn status_says_faulted_only_of_the_revision_whose_recompute_failed() {
        use RebuildState::{Faulted, Idle, Stopped};
        let kernel = FakeKernel::new(1).unwrap();
        let ended = |revision, how| {
            Some(Ended {
                revision,
                how,
                by: Label::new(revision, Reached::default()),
            })
        };
        // Revision 1's failure is its own, never revision 2's.
        let one_failed = rebuilds(&kernel, ended(ONE, failed()));
        assert_eq!(states(&one_failed), [Faulted, Idle]);
        let Some(end) = &one_failed.ended else {
            panic!("no end")
        };
        assert_eq!(
            end.failed(ONE).map(Error::message),
            Some("the recompute failed")
        );
        assert!(end.failed(TWO).is_none());
        // A superseded end is no failure, of its own revision or another.
        let superseded = rebuilds(&kernel, ended(ONE, Ending::Superseded));
        assert_eq!(states(&superseded), [Idle, Idle]);
        // Revision 2's cancel stops 2 alone, and says nothing of 1.
        let two_cancelled = rebuilds(&kernel, ended(TWO, Ending::Cancelled));
        assert_eq!(states(&two_cancelled), [Idle, Stopped]);
    }
}
