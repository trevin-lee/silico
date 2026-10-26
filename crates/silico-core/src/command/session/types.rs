//! A session's IDs and revisions, what its responses say of it and of the model read, the
//! handle a command answers through ([`Running`]), and each command's result.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;

#[cfg(doc)]
use super::Sessions;
use super::cancel::Ticket;
use super::rebuilds::Newest;
use super::rebuilds::met_fault;
use super::transaction::Reached;
use super::{Problems, ReadCommand, SessionCommand};
#[cfg(doc)]
use crate::document;
use crate::document::json::Plain;
use crate::document::{Feature, FeatureId};
use crate::recompute::{Model, StepCount};
use crate::{Error, ErrorCode};

/// A session's ID as a request's schema gives it, the IDs [`SessionId::parse`] reads: `s`
/// and a whole number from 1, with no leading zeros.
pub(crate) const SESSION_ID_PATTERN: &str = r"^s[1-9][0-9]*$";

/// A session's ID, written `s1`, `s2`, ...: the process's count of opens, never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub(super) u64);

impl SessionId {
    /// Its number: 1 for the process's first open, and one more for each after it.
    pub const fn number(self) -> u64 {
        self.0
    }

    /// The ID `text` writes, `s1`, `s2`, ...: `s` and a whole number from 1, with no
    /// leading zeros; none for anything else. A request names its session so
    /// (`command::wire`).
    pub fn parse(text: &str) -> Option<SessionId> {
        crate::command::whole_number(text.strip_prefix('s')?)
            .filter(|&n| n >= 1)
            .map(SessionId)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s{}", self.0)
    }
}

/// Which state of its document a session is at: 1 at `open`, and one more for each change
/// of the document. It never goes back and is never reused, so a result and an edit can say
/// which state they're of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(pub(super) u64);

impl Revision {
    /// A session's revision when it opens.
    pub const OPENED: Revision = Revision(1);

    /// The revision after this one: each change to a session's document gives one more
    /// (design §3); none past `u64::MAX`, so a revision is never reused (an edit then is
    /// refused, as an open is when session IDs run out).
    #[must_use]
    pub const fn next(self) -> Option<Revision> {
        match self.0.checked_add(1) {
            Some(next) => Some(Revision(next)),
            None => None,
        }
    }

    /// Its number, as a response writes it: 1 at `open`, then one more for each change.
    pub const fn number(self) -> u64 {
        self.0
    }

    /// The revision numbered `number`, as a request names one: none for 0, which no
    /// revision is.
    pub const fn from_number(number: u64) -> Option<Revision> {
        if number == 0 {
            None
        } else {
            Some(Revision(number))
        }
    }
}

/// A session as a command left it: what every response says of it (the envelope's
/// `session`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: SessionId,
    /// Its document's path as `open` was given it.
    pub path: String,
    pub revision: Revision,
    /// Whether a transaction is open (the design's section 4).
    pub transaction: bool,
    /// Whether the session reloaded its file before the command, having found it changed
    /// outside the session (decision 2): never inside a transaction, nor for `close`,
    /// `status`, and published reads, which don't read the file.
    pub reloaded: bool,
}

/// A command's answer: the session as the command left it (none for an `open` that failed,
/// or a session that isn't open), the model it read, if it read one, and the command's
/// result, or every reason it was refused, in the order found, the first
/// [`document::PROBLEMS_LISTED`] listed and all counted.
#[derive(Debug, Clone, PartialEq)]
pub struct Response<T> {
    pub session: Option<SessionInfo>,
    pub model: Option<ModelRead>,
    pub result: Result<T, Problems>,
}

/// The model a command read (the envelope's `model`, design §2): the revision it was rebuilt
/// from, whether it was read as the current revision's or as the newest published (an older
/// revision's, maybe), and the kernel fault its rebuild met, if it met one (section 6,
/// decision 32): given once, beside the command's result, never among the document's
/// problems, and kept as that revision's outcome. The fault is as it was met, its suggestion
/// the worker's own: each client says how to try again its own way (the wire, for a
/// session's, that reading the revision again won't, and what does; the CLI, to run it
/// again).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRead {
    pub revision: Revision,
    pub of: ModelChoice,
    pub fault: Option<Error>,
}

impl ModelRead {
    /// `model`, of `revision`, read as `of` says: its fault, if it holds one, as a read
    /// gives it.
    pub(super) fn of(revision: Revision, of: ModelChoice, model: &Model) -> Self {
        ModelRead {
            revision,
            of,
            fault: model.fault().map(met_fault),
        }
    }
}

/// What a read given to [`Sessions::read_model`] is handed: the model, what was read,
/// whether it was stopped (decision 34), and a kernel client of the read's own, for a read
/// that asks the kernel (`distance`).
pub struct Read<'a, C> {
    pub(super) model: &'a Model,
    pub(super) read: ModelRead,
    pub(super) stop: &'a Ticket,
    pub(super) kernel: &'a ReadClient<C>,
    pub(super) reached: &'a Reached,
}

impl<C> Read<'_, C> {
    /// The model read.
    pub fn model(&self) -> &Model {
        self.model
    }

    /// Which model it is: the response's `model`.
    pub fn model_read(&self) -> &ModelRead {
        &self.read
    }

    /// What the session's edits reached at the model's revision (decision 28): the features
    /// whose drift a check of the model doesn't say. Worked out here, the first time a read
    /// of the model asks: on the session's thread for the current model, on the read's own
    /// for the published one.
    pub fn reached(&self) -> &BTreeSet<FeatureId> {
        self.reached.get()
    }

    /// Whether the read was cancelled (`cancel`, or its handle's [`Running::cancel`]): it
    /// answers `cancelled` whatever it gives, so a read that's long may stop when it sees
    /// this.
    pub fn stopped(&self) -> bool {
        self.stop.cancelled()
    }

    /// The read's own client of the kernel (M4.1 §2's *What a published read costs*, as
    /// #125's review settled it): made for this read, never the session's rebuild client, so
    /// no rebuild's advance supersedes it, and dropped when the read ends, which is all it
    /// takes to stop its kernel work. Each read in flight is a client of its own, served in
    /// turn with the others, waiting at most for the first free worker.
    pub fn kernel(&self) -> &ReadClient<C> {
        self.kernel
    }

    /// Settles the read before it leaves its one trace (a file written, decision 34): from
    /// here a cancel comes too late, and if one came first, its answer, `cancelled`.
    pub fn settle(&self) -> Result<(), Error> {
        self.stop.settle()
    }

    /// `f` given `renderer`, its renders cancelled with the read: a cancel, or the session
    /// closing, cancels each render it asked for (a queued one never sent, a running one's
    /// worker killed).
    pub(crate) fn rendering<T>(
        &self,
        renderer: &dyn crate::command::render::Renderer,
        f: impl FnOnce(&dyn crate::command::render::Renderer) -> T,
    ) -> T {
        let read = ReadRenderer {
            inner: renderer,
            stop: self.stop,
            kept: std::sync::Mutex::new(Vec::new()),
        };
        f(&read)
    }
}

/// A read's renderer: each render it asks for cancelled when the read is (its stop holds a
/// weak handle to each render's cancel; this keeps them while the read runs).
struct ReadRenderer<'a> {
    inner: &'a dyn crate::command::render::Renderer,
    stop: &'a Ticket,
    kept: std::sync::Mutex<Vec<Arc<dyn super::cancel::Interrupt>>>,
}

impl crate::command::render::Renderer for ReadRenderer<'_> {
    fn render(&self, scene: silico_protocol::RenderScene) -> crate::render::RenderTicket {
        let ticket = self.inner.render(scene);
        let cancel: Arc<dyn super::cancel::Interrupt> = Arc::new(RenderStop(ticket.canceller()));
        self.stop.interrupt_with(Arc::downgrade(&cancel));
        crate::worker::lock(&self.kept).push(cancel);
        ticket
    }
}

/// A render's cancel, as a read's stop interrupts it: cancelled or closed, the render stops.
struct RenderStop(crate::render::RenderCancel);

impl super::cancel::Interrupt for RenderStop {
    fn interrupt(&self, _why: super::cancel::Why) {
        self.0.cancel();
    }
}

/// A read's own kernel client, as [`Read::kernel`] gives it: every request it makes is at
/// the generation the client began at, so once its read's stop interrupts it (a cancel, or
/// its session's close: advancing it past every generation), each of them is superseded,
/// whether in flight then or asked for after.
pub struct ReadClient<C> {
    pub(super) client: C,
    generation: crate::kernel::Generation,
}

impl<C: crate::kernel::Kernel> ReadClient<C> {
    pub(super) fn new(client: C) -> Self {
        let generation = client.generation();
        ReadClient { client, generation }
    }
}

/// A cancel or the session's close, either way, stops the read's kernel work: the read
/// answers `cancelled` or as of a session that closed, as its stop says.
impl<C: crate::kernel::Kernel> super::cancel::Interrupt for ReadClient<C> {
    fn interrupt(&self, _why: super::cancel::Why) {
        self.client.advance(crate::kernel::Generation(u64::MAX));
    }
}

impl<C: crate::kernel::Kernel> crate::kernel::Kernel for ReadClient<C> {
    fn identity(&self) -> &str {
        self.client.identity()
    }

    /// The generation the client began at, never advanced for the read.
    fn generation(&self) -> crate::kernel::Generation {
        self.generation
    }

    fn advance(&self, generation: crate::kernel::Generation) {
        self.client.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: crate::kernel::Generation,
        request: crate::kernel::KernelRequest<T>,
    ) -> crate::pool::Pending<T> {
        self.client.submit(generation, request)
    }
}

/// Which model a read takes (decision 33): the current revision's, waited for, or the
/// newest published, never waiting or queueing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelChoice {
    Current,
    Published,
}

impl ModelChoice {
    /// Every choice, in the order a refusal lists them.
    pub const ALL: [ModelChoice; 2] = [ModelChoice::Current, ModelChoice::Published];

    /// As a request and a response write it: `current` or `published`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ModelChoice::Current => "current",
            ModelChoice::Published => "published",
        }
    }
}

/// The newest model a session's rebuilds published, and the revision it's the model of (the
/// same as its response's `model.revision`): the revision it was rebuilt from, or, after a
/// rollback, the new revision whose state is the one it was rebuilt from (the design's
/// section 4). What [`Sessions::published`] gives the app to draw, never waiting for a
/// rebuild. With it, what the session's edits reached at that revision (decision 28): the
/// features whose drift a check of it doesn't say, since a commit writes their records anew,
/// which a read of it works out if it asks ([`Read::reached`]).
#[derive(Debug, Clone)]
pub struct Published {
    pub revision: Revision,
    pub model: Arc<Model>,
    pub(crate) reached: Reached,
}

/// A command running on its session's thread: wait for its answer, poll it, cancel it, or
/// drop it (the command runs on, its answer unread).
#[must_use = "a command's answer is only had by waiting for it"]
pub struct Running<T> {
    answer: Receiver<Response<T>>,
    /// What's said if the thread answering ends without an answer (a bug): which stopped.
    lost: &'static str,
    /// How it's stopped: none for a command answered before it ran (refused).
    ticket: Option<Arc<Ticket>>,
    /// Its session's newest rebuild: none for a command answered before it ran, and for a
    /// published read, which waits on no rebuild.
    rebuild: Option<Arc<Newest>>,
}

/// What cancels a command apart from its handle ([`Running::stopper`]), from any thread.
#[derive(Clone)]
pub struct Stopper {
    /// None for a command answered before it ran (refused).
    ticket: Option<Arc<Ticket>>,
}

impl Stopper {
    /// The stopper of a command that's never stopped, being answered at once when it's
    /// waited on (`status`, `cancel`, `version`, `schema`, through `Sessions::call`), or
    /// refused before it ran: its [`cancel`](Self::cancel) stops nothing.
    pub(crate) const fn none() -> Self {
        Stopper { ticket: None }
    }

    /// Cancels the command, as [`Running::cancel`] does: whether it was stopped before it
    /// had done its work.
    pub fn cancel(&self) -> bool {
        self.ticket.as_ref().is_some_and(|ticket| ticket.cancel())
    }
}

/// A queued command's thread ended without answering.
pub(super) const SESSION_STOPPED: &str = "the session stopped before the command answered";

/// A published read's thread ended without answering (its read panicked).
pub(super) const READ_STOPPED: &str = "the read stopped before it answered";

impl<T> Running<T> {
    pub(super) fn new(
        answer: Receiver<Response<T>>,
        lost: &'static str,
        ticket: Option<Arc<Ticket>>,
        rebuild: Option<Arc<Newest>>,
    ) -> Self {
        Running {
            answer,
            lost,
            ticket,
            rebuild,
        }
    }

    /// A command answered before it was queued (refused: no such session, too many open).
    pub(super) fn answered(response: Response<T>) -> Self {
        let (reply, answer) = mpsc::sync_channel(1);
        let _ = reply.send(response);
        Running::new(answer, SESSION_STOPPED, None, None)
    }

    /// How far its session's rebuild has got, its step running too, while one runs (one the
    /// command waits for, or any other): none while none runs, and none for a read of the
    /// published model, which waits on no rebuild. Read at once, never waiting on the
    /// rebuild or the command, so the app may poll it each frame.
    pub fn progress(&self) -> Option<Progress> {
        self.rebuild.as_ref().and_then(|rebuild| rebuild.running())
    }

    /// Cancels the command, as MCP's cancellation does (decision 10): if it hasn't done its
    /// work, it stops (a rebuild it waits on stopped through the pool's superseded path, a
    /// command still queued never run) and answers `cancelled`, having left no trace;
    /// otherwise it answers with its real result. Whether it was stopped before it had done
    /// its work; false too for a command already answered without running (refused). As its
    /// [`stopper`](Self::stopper) cancels it.
    pub fn cancel(&self) -> bool {
        self.stopper().cancel()
    }

    /// What cancels the command apart from this handle: cloned and sent to another thread,
    /// it cancels the command while this handle waits on its answer (MCP's server, told a
    /// request it waits on was cancelled).
    pub fn stopper(&self) -> Stopper {
        Stopper {
            ticket: self.ticket.clone(),
        }
    }

    /// Waits for the answer.
    pub fn wait(self) -> Response<T> {
        let lost = self.lost;
        self.answer.recv().unwrap_or_else(|_| unanswered(lost))
    }

    /// Waits up to `timeout` for the answer, or gives the handle back to wait on again.
    pub fn wait_timeout(self, timeout: Duration) -> Result<Response<T>, Self> {
        match self.answer.recv_timeout(timeout) {
            Ok(response) => Ok(response),
            Err(RecvTimeoutError::Timeout) => Err(self),
            Err(RecvTimeoutError::Disconnected) => Ok(unanswered(self.lost)),
        }
    }

    /// The answer, if it has come, or the handle back to wait on.
    pub fn try_take(self) -> Result<Response<T>, Self> {
        match self.answer.try_recv() {
            Ok(response) => Ok(response),
            Err(TryRecvError::Empty) => Err(self),
            Err(TryRecvError::Disconnected) => Ok(unanswered(self.lost)),
        }
    }
}

/// The answer of a command whose thread ended without giving one (a bug): `lost` says which
/// stopped.
fn unanswered<T>(lost: &str) -> Response<T> {
    Response {
        session: None,
        model: None,
        result: Err(Problems::from(
            Error::new(ErrorCode::Internal, lost).with_suggestion(crate::worker::SILICO_BUG),
        )),
    }
}

/// What `open` found: the session it opened, the path as given, whether it made the file,
/// how many features the document has (none if its text doesn't read as a document), the
/// bodies its timeline ends with (as `silico show` finds them, needing no kernel), and the
/// gate's problems, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    /// The session opened: the envelope's `session.id`, so not written in the result again.
    pub session: SessionId,
    pub path: String,
    pub created: bool,
    pub features: Option<usize>,
    pub bodies: Vec<FeatureId>,
    pub problems: Problems,
}

/// What `close` answers: nothing but that it closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// What `text` answers: the text the session's document is, or one feature's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextAnswer {
    pub text: String,
}

/// What `outline` answers: the document's outline, as `silico show` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineAnswer {
    pub outline: String,
}

/// What `check` answers: the document's problems, and its warnings, which never fail
/// anything, and of which there are none yet (the first, M3's two curve ends at one place
/// that aren't one point, comes with its code).
#[derive(Debug, Clone, PartialEq)]
pub struct CheckAnswer {
    pub problems: Problems,
    /// In the error contract's shape, as the wire writes them. Decision 17 gives warnings
    /// codes of their own: this type changes with the first warning code, when warnings
    /// are made.
    pub warnings: Problems,
}

impl CheckAnswer {
    /// The answer of a check that found `problems`, and no warnings (none are made yet).
    pub(super) fn of(problems: Problems) -> Self {
        CheckAnswer {
            problems,
            warnings: Problems::none(),
        }
    }
}

/// What `cache_clear` answers (the design's section 7): the rebuild cache's directory, as
/// the session's path names it (`.silico-cache` beside `bracket.silico`, `parts/.silico-cache`
/// beside `parts/bracket.silico`; where it is, through links, if the path is a link from
/// another directory), and how many entries it removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheCleared {
    pub directory: String,
    pub removed: u64,
}

/// What `begin` answers: the revision the transaction began from, which `rollback` returns
/// to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Begun {
    pub base: Revision,
}

/// What `commit` did (the design's section 7): whether it wrote the file (not if the
/// document is the one `begin` saw), its path as `open` was given it, the features the
/// transaction's edits reached whose fingerprints it wrote anew and the reached sketches
/// whose solves it wrote, in timeline order (decision 28), and the problems it can't fix:
/// failed features, solids that couldn't be measured, and the drift of features no edit
/// reached, which keep what's stored with them. A failed feature the edits reached is saved
/// with no fingerprint (decision 28, amended), and isn't among those whose fingerprints it
/// wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct Committed {
    pub written: bool,
    pub path: String,
    pub refingerprinted: Vec<FeatureId>,
    pub resolved: Vec<FeatureId>,
    pub problems: Problems,
}

/// What `rollback` answers: the revision whose state it restored. The session is at a new
/// revision, whose state equals that one's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RolledBack {
    pub restored: Revision,
}

impl Committed {
    /// A commit that wrote nothing, of the document at `path`: what it found, `problems`.
    pub(super) fn unwritten(path: String, problems: Problems) -> Self {
        Committed {
            written: false,
            path,
            refingerprinted: Vec::new(),
            resolved: Vec::new(),
            problems,
        }
    }
}

/// What `format` did (decision 31): whether its canonical text differs from the file (as
/// the session read it before the command, reloading a change, decision 2), whether it
/// wrote it, the path as `open` was given it, the features whose fingerprints it wrote (or,
/// not writing, would write) anew and the sketches whose solves, the features among them
/// whose drift it accepted, each in timeline order, and what it can't fix: failed features
/// (one the session's edits reached saved with no fingerprint, decision 28 amended; any
/// other keeping what's stored with it), solids that couldn't be measured, and the drift it
/// didn't accept.
#[derive(Debug, Clone, PartialEq)]
pub struct Reformatted {
    pub changed: bool,
    pub written: bool,
    pub path: String,
    pub refingerprinted: Vec<FeatureId>,
    pub resolved: Vec<FeatureId>,
    pub accepted: Vec<FeatureId>,
    pub problems: Problems,
}

/// What `add_feature` did (the design's section 7): the feature as the document now holds it
/// (its ID, the one given or the one made, and its version; outside a transaction, as its
/// commit saved it, with the fingerprint written), whether the document changed (always, a
/// feature added), and, outside a transaction, the commit's result, none inside one.
#[derive(Debug, Clone, PartialEq)]
pub struct Added {
    pub feature: Feature,
    pub changed: bool,
    pub commit: Option<Committed>,
}

/// What `set_field` did (the design's section 7): the feature and the field, as the request
/// named them, the field's value before and after, each as the document's canonical form
/// writes it (`null` for a field the feature hasn't, a note removed; after, outside a
/// transaction, as its commit saved it, a sketch's solve and all), whether the document
/// changed (not if the field was given the value it has: then there's no new revision and
/// nothing is written), and, outside a transaction, the commit's result, none inside one or
/// if nothing changed.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSet {
    pub feature: FeatureId,
    pub field: String,
    pub before: Plain,
    pub after: Plain,
    pub changed: bool,
    pub commit: Option<Committed>,
}

/// What `remove_feature` did (the design's section 7): the feature removed, as the document
/// held it, its place in the timeline, from 0, so `replace_text` can put it back, whether the
/// document changed (always), and, outside a transaction, the commit's result, none inside
/// one.
#[derive(Debug, Clone, PartialEq)]
pub struct Removed {
    pub feature: Feature,
    pub index: usize,
    pub changed: bool,
    pub commit: Option<Committed>,
}

/// What `replace_text` did (the design's section 7): whether the document changed (not if
/// the text is the document the session holds, in whatever layout: then there's no new
/// revision and nothing is written), its number of features, and, outside a transaction,
/// the text it replaced (so a client can put it back) and the commit's result (none if
/// nothing changed); inside one, neither (a rollback restores the text).
#[derive(Debug, Clone, PartialEq)]
pub struct Replaced {
    pub changed: bool,
    pub features: usize,
    pub before: Option<String>,
    pub commit: Option<Committed>,
}

/// What `status` says of a session, from what it holds: how many features its document has
/// (none if its text doesn't read as one), whether the gate took it, the transaction open,
/// if one is (the revision it began from, which `rollback` returns to), the queued command
/// running, if one is, the published reads running, in the order they started, and its
/// current revision's rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub features: Option<usize>,
    pub gated: bool,
    pub transaction: Option<Begun>,
    pub running: Option<SessionCommand>,
    pub reads: Vec<ReadCommand>,
    pub rebuild: Rebuild,
}

/// A session's current revision's rebuild, as `status` gives it: the revision, what's
/// become of its rebuild, and how far that has got, if a rebuild of it has started. The
/// wire gives its progress's `done` and `steps`, not the step running, as the design's
/// `status` does (section 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebuild {
    pub revision: Revision,
    pub state: RebuildState,
    pub progress: Option<Progress>,
}

/// How far a rebuild has got (the design's section 6): its steps settled (built, failed,
/// kept from before, or never to run; all of them once it finishes), the steps of its plan
/// (none until its plan is made, after the sketches it sweeps are solved), and the step
/// started last of those running, as its feature and the feature's type (`f0000001
/// fillet`, MCP's progress message), none while no step runs. A diagnostic, not a result
/// (section 10): which step finishes first depends on scheduling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub steps: usize,
    pub running: Option<String>,
}

impl From<&StepCount> for Progress {
    fn from(count: &StepCount) -> Self {
        Progress {
            done: count.done,
            steps: count.steps,
            running: count.running.clone(),
        }
    }
}

/// What's become of a revision's rebuild (the design's `status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildState {
    /// None running, and no end it reports (failed or cancelled): no command has needed
    /// its model, or its rebuild was superseded, or an edit's commit was cancelled while its
    /// rebuild ran; or the gate refused the document, so there's nothing to rebuild. Written
    /// `none`.
    Idle,
    /// Running: the revision's, or, for an edit made outside a transaction, its commit's.
    Running,
    /// Done, its model published.
    Done,
    /// Done, its model holding a kernel fault, kept as the revision's outcome (decision
    /// 32); or its recompute failed (a bug), said once to the command that needs it.
    Faulted,
    /// Cancelled: the next queued command that needs its model starts it again.
    Stopped,
}

impl RebuildState {
    /// As `status` writes it: `none`, `running`, `done`, `faulted`, or `stopped`.
    pub const fn as_str(self) -> &'static str {
        match self {
            RebuildState::Idle => "none",
            RebuildState::Running => "running",
            RebuildState::Done => "done",
            RebuildState::Faulted => "faulted",
            RebuildState::Stopped => "stopped",
        }
    }
}

/// What `cancel` answers (decision 34): the command it named, and whether it was stopped
/// before it had done its work (false: it had, an edit already saved, say, and it answers
/// with its real result).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancelled {
    pub command: SessionCommand,
    pub stopped: bool,
}
