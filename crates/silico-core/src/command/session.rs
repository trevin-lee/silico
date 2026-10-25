//! Sessions (docs/design/m4.1-command-api.md, section 3): a document held open between
//! commands, as every client holds one: MCP's tools, the app's windows, and the CLI, which
//! opens one, runs a command, and closes it.
//!
//! A process's sessions are one [`Sessions`], one client's (decision 30). Each session runs
//! its commands one at a time, in the order they arrive, on a thread of its own
//! (`session-s1`, ...), never the caller's (ARCHITECTURE.md, invariant 4): even `open`'s
//! file read and gate. A command returns a [`Running`] handle at once, which the app polls,
//! MCP's server waits on, and the CLI waits for; only [`Sessions::status`] and
//! [`Sessions::published`] answer on the caller's thread, from what the session holds. Each
//! answer is a [`Response`]: the session as the command left it, the model it read, if it
//! read one ([`ModelRead`]), and the command's result, or every reason it was refused.
//!
//! A file is open in one session a process (decision 22), known both by its path through
//! links and by the file it is (its device and inode, held open while claimed), so neither
//! another name for it (a link, a hard link, another name the system gives it) nor another
//! file put at its path (a save by rename) opens it twice, and two sessions never save one
//! file over each other. At most [`MAX_SESSIONS`] are open
//! at once. A document the gate refuses still opens, holding its text and the gate's
//! problems (decision 21), so it can be mended; a file longer than Silico reads doesn't,
//! since a session couldn't hold its text whole. Session IDs are `s1`, `s2`, ..., in the
//! order opens were asked for, never reused within the process. Each session has its own
//! client of the shared kernel ([`Clients::client`]: its rebuilds never supersede another
//! session's), and sessions on documents in one directory share one rebuild cache, one
//! value cloned, so its bound counts every session's writes.
//!
//! A session's document changes in transactions (the design's section 4): [`Sessions::begin`]
//! keeps the session's state, [`Sessions::commit`] saves what the transaction's edits
//! changed through [`file::save`], writing new fingerprints and solves only for the features
//! they reached (decision 28), and [`Sessions::rollback`] restores what `begin` kept, the
//! text byte for byte and the model `begin` saw, at a new revision whose state equals the
//! old. One at a time, never nested; a session isn't closed while one is open, and a client
//! that goes away (the sessions dropped) leaves its file as it was. An edit made outside a
//! transaction ([`Sessions::add_feature`], [`Sessions::set_field`],
//! [`Sessions::remove_feature`], [`Sessions::replace_text`]) is saved as it's made, as its
//! own transaction (decision 1): so only a commit, such an edit, or [`Sessions::format`],
//! which accepts drift by name (decision 31), writes the document.
//!
//! A file can change under a session (decision 2): before each queued command outside a
//! transaction, the session reads its file again and compares its bytes with those it last
//! read or wrote, and reloads a change at a new revision, the response's `reloaded` saying
//! so; a file gone, unreadable, or another session's refuses the command. Inside a
//! transaction, only `commit` reads the file, and a change (or a file gone) is its conflict:
//! every other command runs on what the session holds. `close`, `status`, and published
//! reads never read the file.
//!
//! A session's path is UTF-8, so every response names it exactly: `open` refuses one that
//! isn't.
//!
//! What it doesn't bound: the caches, one kept for each directory a session has opened a
//! document in, for the process's life; and each session's queue, which holds every command
//! queued and not yet run. Nor does it choose which files may be opened: `open` and `text`
//! read any file this user may read, so a server for clients it doesn't trust gives them a
//! workspace (decision 26) once there is one.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use super::edit::{self, Candidate, FeatureIds};
use super::{Gated, Problems};
use crate::cache::Cache;
use crate::document::json::{MAX_INPUT_LEN, Node, Plain};
use crate::document::{
    self, Checked, Document, Feature, FeatureId, FormatVersion, NamingScheme, Units,
};
use crate::file;
use crate::kernel::{Clients, Kernel};
use crate::recompute::{Model, Recompute, Run, RunOutcome};
use crate::sketch::SketchSolver;
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// The most sessions one process holds open at once (decision 18): each may start a sketch
/// worker and keep its models' B-reps, so the limit is what bounds their memory.
pub const MAX_SESSIONS: usize = 16;

/// A session's ID, written `s1`, `s2`, ...: the process's count of opens, never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(u64);

impl SessionId {
    /// Its number: 1 for the process's first open, and one more for each after it.
    pub const fn number(self) -> u64 {
        self.0
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
pub struct Revision(u64);

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
/// from, and whether it was read as the current revision's or as the newest published (an
/// older revision's, maybe). The fault its rebuild met comes with decision 32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRead {
    pub revision: Revision,
    pub of: ModelChoice,
}

/// What a read given to [`Sessions::read_model`] is handed: the model, and what was read.
/// The published read lane's kernel client and a check of whether the read was stopped
/// (decisions 33 and 34) come later, here, without changing a read's signature.
pub struct Read<'a> {
    model: &'a Model,
    read: ModelRead,
}

impl Read<'_> {
    /// The model read.
    pub fn model(&self) -> &Model {
        self.model
    }

    /// Which model it is: the response's `model`.
    pub fn model_read(&self) -> ModelRead {
        self.read
    }
}

/// The most published reads one session runs at once: each holds a thread, and the session,
/// until it ends.
pub const MAX_PUBLISHED_READS: usize = 8;

/// Which model a read takes (decision 33): the current revision's, waited for, or the
/// newest published, never waiting or queueing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelChoice {
    Current,
    Published,
}

/// The newest model a session's rebuilds published, and the revision it's the model of (the
/// same as its response's `model.revision`): the revision it was rebuilt from, or, after a
/// rollback, the new revision whose state is the one it was rebuilt from (the design's
/// section 4). What [`Sessions::published`] gives the app to draw, never waiting for a
/// rebuild.
#[derive(Debug, Clone, PartialEq)]
pub struct Published {
    pub revision: Revision,
    pub model: Arc<Model>,
}

/// A command running on its session's thread: wait for its answer, poll it, or drop it
/// (the command runs on, its answer unread).
#[must_use = "a command's answer is only had by waiting for it"]
pub struct Running<T> {
    answer: Receiver<Response<T>>,
    /// What's said if the thread answering ends without an answer (a bug): which stopped.
    lost: &'static str,
}

/// A queued command's thread ended without answering.
const SESSION_STOPPED: &str = "the session stopped before the command answered";

/// A published read's thread ended without answering (its read panicked).
const READ_STOPPED: &str = "the read stopped before it answered";

impl<T> Running<T> {
    fn new(answer: Receiver<Response<T>>, lost: &'static str) -> Self {
        Running { answer, lost }
    }

    /// A command answered before it was queued (refused: no such session, too many open).
    fn answered(response: Response<T>) -> Self {
        let (reply, answer) = mpsc::sync_channel(1);
        let _ = reply.send(response);
        Running::new(answer, SESSION_STOPPED)
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

/// What `open` found: the path as given, whether it made the file, how many features the
/// document has (none if its text doesn't read as a document), the bodies its timeline ends
/// with (as `silico show` finds them, needing no kernel), and the gate's problems, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub path: String,
    pub created: bool,
    pub features: Option<usize>,
    pub bodies: Vec<FeatureId>,
    pub problems: Problems,
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
/// reached, which keep what's stored with them.
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

/// What `format` did (decision 31): whether its canonical text differs from the file (as
/// the session read it before the command, reloading a change, decision 2), whether it
/// wrote it, the path as `open` was given it, the features whose fingerprints it wrote (or,
/// not writing, would write) anew and the sketches whose solves, the features among them
/// whose drift it accepted, each in timeline order, and what it can't fix: failed features,
/// solids that couldn't be measured, and the drift it didn't accept.
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
/// (its ID, the one given or the one made, and its version), whether the document changed
/// (always, a feature added), and, outside a transaction, the commit's result, none inside
/// one.
#[derive(Debug, Clone, PartialEq)]
pub struct Added {
    pub feature: Feature,
    pub changed: bool,
    pub commit: Option<Committed>,
}

/// What `set_field` did (the design's section 7): the feature and the field, as the request
/// named them, the field's value before and after, each as the document's canonical form
/// writes it (`null` for a field the feature hasn't, a note removed), whether the document
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
/// (none if its text doesn't read as one), whether the gate took it, and the transaction
/// open, if one is: the revision it began from, which `rollback` returns to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub features: Option<usize>,
    pub gated: bool,
    pub transaction: Option<Begun>,
}

/// A process's open sessions, on one kernel: each session's commands are run on its own
/// thread, in turn, on its own client of `kernel`. Dropping it closes every session,
/// stopping its rebuild and leaving its file as it was (an open transaction is rolled back:
/// only a commit or `format` writes), and waits for each session's thread to end, then for
/// the threads watching its rebuilds and running its published reads, a session's closed
/// before the drop too (a read blocked in its own code keeps the drop waiting until it
/// returns).
pub struct Sessions<P: Clients + 'static> {
    registry: Arc<Registry<P>>,
}

/// What opens a directory's cache: [`Cache::open`], or a test's.
type CacheOpener = dyn Fn(&Path) -> Cache + Send + Sync;

/// What starts a session's sketch solver, if not the sketch worker: a test's stand-in.
type SketchStarter = dyn Fn() -> Result<Box<dyn SketchSolver>, Error> + Send + Sync;

struct Registry<P: Clients + 'static> {
    kernel: P,
    open_cache: Box<CacheOpener>,
    /// Given to each session's engine as it opens; none for the sketch worker.
    sketches: Option<Arc<SketchStarter>>,
    /// Where `add_feature` takes the IDs of features given none.
    feature_ids: Arc<Mutex<FeatureIds>>,
    book: Mutex<Book<P::Client>>,
}

/// A file as the system knows it, whatever path reaches it: its device and inode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FileId {
    device: u64,
    inode: u64,
}

impl FileId {
    /// The file `opened` is.
    fn of(opened: &std::fs::File) -> Result<Self, Error> {
        let metadata = opened.metadata().map_err(file::unreadable)?;
        Ok(FileId {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

/// The registry's record of its sessions.
struct Book<C: Kernel + 'static> {
    /// The next session's number.
    next: u64,
    /// The sessions open.
    sessions: BTreeMap<SessionId, Arc<Session<C>>>,
    /// The sessions opening: not open yet, but counted against [`MAX_SESSIONS`].
    opening: BTreeSet<SessionId>,
    /// Each file a session holds, or is opening, as the system knows it, and the file kept
    /// open while it's claimed: an open file's inode isn't given to another (as ext4 gives a
    /// freed one at once), so a claim never comes to name some other file. A test can't make
    /// the system reuse an inode on demand; holding the file is what guarantees it.
    files: BTreeMap<FileId, (SessionId, std::fs::File)>,
    /// Each file a session holds, or is opening, by its path as its links resolve: a file
    /// replaced at its path (an editor's or git's save, a rename over it) is a new inode
    /// there, and still the session's document. Saves (the design's section 4) and reloads
    /// (decision 2) that put a new file at the path must claim its new inode too.
    paths: BTreeMap<PathBuf, SessionId>,
    /// Each directory's rebuild cache, by its path (`file::cache_directory`), cloned for
    /// each session on a document in it.
    caches: BTreeMap<PathBuf, Cache>,
    /// Every session thread not yet joined: those open, opening, or closed and ending,
    /// joined once they've ended (at the next open) or when the registry is dropped, so
    /// none outlives it.
    threads: Threads,
    /// The published reads of sessions released (closed, or their threads ended), each
    /// session's moved here as it's released, so the drop joins them too.
    reads: Threads,
    /// The sessions are being dropped: an open that finishes now doesn't open.
    stopping: bool,
}

impl<C: Kernel + 'static> Book<C> {
    /// Session `id` gone from every record but its thread's: open, opening, and the file
    /// it held; its published reads now the book's to join, and none started after.
    fn release(&mut self, id: SessionId) {
        if let Some(session) = self.sessions.remove(&id)
            && let Some(reads) = lock(&session.reads).threads.take()
        {
            self.reads.extend(reads);
        }
        self.opening.remove(&id);
        self.files.retain(|_, (holder, _)| *holder != id);
        self.paths.retain(|_, holder| *holder != id);
    }

    /// The threads that have ended, joined.
    fn reap(&mut self) {
        self.threads.reap();
        self.reads.reap();
    }
}

/// Threads started and joined later (a session's, its rebuilds' watchers, its published
/// reads): those that have ended joined as more start, the rest when what holds them is
/// dropped, so none outlives it.
struct Threads {
    running: Vec<JoinHandle<()>>,
}

impl Threads {
    fn new() -> Self {
        Threads {
            running: Vec::new(),
        }
    }

    fn push(&mut self, thread: JoinHandle<()>) {
        self.running.push(thread);
    }

    fn extend(&mut self, other: Threads) {
        self.running.extend(other.running);
    }

    /// Those that have ended, joined; the rest kept.
    fn reap(&mut self) {
        let (ended, running): (Vec<_>, Vec<_>) = std::mem::take(&mut self.running)
            .into_iter()
            .partition(JoinHandle::is_finished);
        self.running = running;
        for thread in ended {
            let _ = thread.join();
        }
    }

    /// Every one joined, waiting for each to end.
    fn join(self) {
        for thread in self.running {
            let _ = thread.join();
        }
    }
}

/// One open session, shared by its thread, the callers that queue its commands, and the
/// registry.
struct Session<C: Kernel + 'static> {
    id: SessionId,
    held: Mutex<Held>,
    /// Where its commands are queued, in order: none once it's closed.
    queue: Mutex<Option<Sender<Job<C>>>>,
    /// Its rebuilds, under a lock of their own, never `held`'s, so `status` never waits
    /// behind a rebuild's start; and reachable from outside its thread, so a rebuild can be
    /// stopped while a command waits on it.
    rebuilds: Mutex<Rebuilds<C>>,
    /// A rebuild ended: told to the command waiting on it.
    rebuilt: Condvar,
    /// Its published reads.
    reads: Mutex<Reads>,
}

/// A session's published reads, each on a thread of its own, at most
/// [`MAX_PUBLISHED_READS`] unanswered at once.
struct Reads {
    /// How many have started and not answered: each counted until just before it answers
    /// (however it ends, by a panic too), so one just answered, its thread still ending,
    /// leaves room for the next.
    unanswered: Arc<AtomicUsize>,
    /// Their threads: those ended joined as the next starts, the rest by
    /// `Sessions::drop`; none once the session is released (its reads then the book's, and
    /// a read asked for after it refused as of a session not open).
    threads: Option<Threads>,
}

impl Reads {
    fn new() -> Self {
        Reads {
            unanswered: Arc::new(AtomicUsize::new(0)),
            threads: Some(Threads::new()),
        }
    }
}

/// A published read counted unanswered, until it's dropped: just before the read answers,
/// or as its thread unwinds.
struct Unanswered(Arc<AtomicUsize>);

impl Unanswered {
    fn count(unanswered: &Arc<AtomicUsize>) -> Self {
        unanswered.fetch_add(1, Ordering::SeqCst);
        Unanswered(Arc::clone(unanswered))
    }
}

impl Drop for Unanswered {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

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
/// session's command stops rather than rebuilding again.
struct Rebuilds<C: Kernel + 'static> {
    engine: Recompute<C>,
    /// The rebuild running, if one is, labelled with the revision its model is of.
    running: Option<Label>,
    /// The newest model the session's rebuilds published.
    published: Option<Published>,
    /// The last rebuild whose recompute failed (a bug), until another starts.
    failed: Option<Failed>,
    /// While a transaction is open, the models `begin` saw: what a rollback restores.
    kept: Option<Kept>,
    /// No model of a revision before this one is published: a rollback's, so a rebuild of
    /// the transaction's documents that ends after it never takes the restored state's place.
    floor: Revision,
    /// What the rebuild of an edit's candidate came to (an edit outside a transaction,
    /// [`Session::candidate_model`]), for its commit, which takes it: never published as
    /// it is (its save publishes it as the saved revision's, rebound to the saved
    /// document, when it writes no sketch's solve).
    candidate: Option<(Label, Result<RunOutcome, Error>)>,
    /// The threads watching the rebuilds started: those ended joined as the next starts,
    /// the rest by `Sessions::drop`, for the sessions open then.
    watchers: Threads,
}

impl<C: Kernel + 'static> Rebuilds<C> {
    /// The revision whose rebuild is running, if one is.
    fn running(&self) -> Option<Revision> {
        self.running.as_ref().map(Label::revision)
    }
}

/// The revision a rebuild's model is of: shared by the session's rebuilds, while it's the
/// one running, and the thread watching it, so a rollback that gives the rebuild to the new
/// revision whose state equals its own gives it for good, however many rollbacks give it on
/// before it ends. Read and given only under the rebuilds' lock. A candidate's (an edit's
/// outside a transaction) is of the revision its save would give: its model is its
/// commit's, never published as it is. While it runs, the session's revision is still the
/// one before: so `status`, once it says what's running (M4.2 PR 8), says a candidate's
/// is the current revision's edit being committed, not a rebuild of a revision to come.
#[derive(Clone)]
struct Label {
    revision: Arc<AtomicU64>,
    candidate: bool,
}

impl Label {
    fn new(revision: Revision) -> Self {
        Label {
            revision: Arc::new(AtomicU64::new(revision.0)),
            candidate: false,
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

    fn revision(&self) -> Revision {
        Revision(self.revision.load(Ordering::SeqCst))
    }

    /// The rebuild given to `revision`: what it makes is that revision's.
    fn give(&self, revision: Revision) {
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
struct Kept {
    /// The revision the transaction began from.
    base: Revision,
    /// The newest model published when it began (of `base`, or an older revision).
    published: Option<Published>,
    /// `base`'s model: as `begin` found it published, or as its rebuild, running then,
    /// published it since.
    model: Option<Arc<Model>>,
    /// Whether a rebuild started inside the transaction, which may have superseded
    /// `base`'s: if so, and `base`'s model never came, a rollback starts the restored
    /// document's rebuild; if not, it starts none.
    started: bool,
}

/// A revision's rebuild whose recompute failed (a bug), and why: said once, to the check
/// waiting on it or the next to need its model; a check after that rebuilds it. A published
/// read never starts it again (it would fail again, each frame), and refuses with `why`.
struct Failed {
    revision: Revision,
    why: Error,
    said: bool,
}

/// What a session holds (docs/design/m4.1-command-api.md, section 3). Its values are shared
/// with the commands reading them, so a command takes them under the lock at once and works
/// off it, and `status` never waits on a command's work.
struct Held {
    /// The document's path as `open` was given it.
    path: String,
    /// The path as its links resolve, where saves and reloads go.
    resolved: PathBuf,
    /// The file's bytes as the session last read or wrote them: what a save checks the file
    /// still holds, so one changed outside the session isn't written over.
    bytes: Arc<Vec<u8>>,
    /// The document's text: the file's bytes, until an edit.
    text: Arc<Vec<u8>>,
    /// The text as the reader and the gate found it.
    gated: Arc<Gated>,
    revision: Revision,
    /// The features edits reached before the last save whose records it couldn't write (a
    /// feature that failed): still reached, until a save writes them.
    carried: Arc<BTreeSet<FeatureId>>,
    /// The state `begin` kept, while a transaction is open.
    transaction: Option<Snapshot>,
    /// Where the file's old text is, if this session's own save left the file part
    /// written and couldn't put it back: the next save says so, rather than take the file
    /// for one changed outside the session; until the file holds what the session last read
    /// or wrote again, the old text put back ([`Session::reload`]).
    part_written: Option<PathBuf>,
    /// The newest revision a reload of the file gave (decision 2): an edit made against a
    /// revision before it is stale because the file changed, which its refusal says.
    last_reload: Option<Revision>,
}

/// A session's state as `begin` kept it, which `rollback` restores (its models are kept with
/// the session's rebuilds, [`Kept`]).
#[derive(Clone)]
struct Snapshot {
    text: Arc<Vec<u8>>,
    gated: Arc<Gated>,
    revision: Revision,
    carried: Arc<BTreeSet<FeatureId>>,
    /// If its text didn't read as a document, the IDs it holds, found once, the first time
    /// what the transaction's edits reach is worked out ([`Reaching::reach`]): outside the
    /// held state's lock, so a long text's scan never keeps `status` or a published read
    /// waiting.
    quoted: Arc<OnceLock<document::QuotedIds>>,
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

/// Whose state a commit saves: an open transaction's, or an edit's candidate, made outside
/// one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Through {
    Transaction,
    Candidate,
}

impl Held {
    /// The revision the session's next change gives, or why there's none.
    fn next(&self) -> Result<Revision, Problems> {
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
    /// place a session's document changes (an edit, a save, a rollback).
    fn change(
        &mut self,
        revision: Revision,
        text: Arc<Vec<u8>>,
        gated: Arc<Gated>,
        carried: Arc<BTreeSet<FeatureId>>,
    ) {
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
        }
    }

    /// What the session's edits reach now, taken under the lock, to be worked out
    /// ([`Reaching::reach`]) once it's let go.
    fn reaching(&self) -> Reaching {
        Reaching {
            gated: Arc::clone(&self.gated),
            carried: Arc::clone(&self.carried),
            begun: self.transaction.clone(),
        }
    }
}

/// What a session's edits reach at a revision, as its held state was then: its document,
/// what was carried, and the transaction's snapshot, if one was open.
struct Reaching {
    gated: Arc<Gated>,
    carried: Arc<BTreeSet<FeatureId>>,
    begun: Option<Snapshot>,
}

impl Reaching {
    /// What the session's edits reach (the design's section 5, decision 28), net: the
    /// features they changed since the transaction began, compared by ID with the document
    /// `begin` saw (or, if that didn't read, those with IDs its text hasn't), with those
    /// carried from before, and every feature that uses one (`document::net_reach`). An
    /// edit undone reaches nothing. Those a commit writes new records for, and whose drift
    /// a check doesn't say. Outside a transaction, what's carried.
    fn reach(&self) -> BTreeSet<FeatureId> {
        let Ok(now) = self.gated.document() else {
            return (*self.carried).clone();
        };
        let Some(begun) = &self.begun else {
            return document::with_users(now, &self.carried);
        };
        let before = match begun.gated.document() {
            Ok(before) => document::Before::Read(before),
            Err(_) => document::Before::Unread(
                begun
                    .quoted
                    .get_or_init(|| document::QuotedIds::of(&begun.text)),
            ),
        };
        document::net_reach(before, &self.carried, now)
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

/// A command queued on a session: run on its thread with the session, or, if the session
/// closed before its turn, told so.
type Job<C> = Box<dyn FnOnce(Turn<'_, C>) + Send>;

enum Turn<'a, C: Kernel + 'static> {
    Run(&'a Arc<Session<C>>),
    Closed(SessionId),
}

impl<C: Kernel + 'static> Session<C> {
    /// The model of `revision`, whose document is `checked`: the one its rebuild published,
    /// waited for, the rebuild started if none of it is running (decision 13). A model of it
    /// published before that holds a kernel fault (a worker that crashed or hung) isn't taken
    /// again: it's rebuilt, so a fault that passes is tried again (until faults are kept as a
    /// revision's outcome, decision 32). Refused if the session is dropped meanwhile (its
    /// rebuild stopped), or the recompute failed (a bug).
    fn model(
        self: &Arc<Self>,
        revision: Revision,
        checked: &Arc<Checked>,
    ) -> Result<Arc<Model>, Problems> {
        let _span = tracing::info_span!("rebuild", revision = revision.number()).entered();
        let mut rebuilds = lock(&self.rebuilds);
        // Whether this read has waited for a rebuild of its own: then its model is the one,
        // faulted or not.
        let mut waited = false;
        loop {
            if let Some(published) = &rebuilds.published
                && published.revision == revision
                && (waited || !faulted(&published.model))
            {
                return Ok(Arc::clone(&published.model));
            }
            if rebuilds.running() == Some(revision) {
                rebuilds = self
                    .rebuilt
                    .wait(rebuilds)
                    .unwrap_or_else(PoisonError::into_inner);
                waited = true;
                continue;
            }
            // A failure of this revision's, said once; the read after that rebuilds it.
            if let Some(failed) = &mut rebuilds.failed
                && failed.revision == revision
                && !failed.said
            {
                failed.said = true;
                return Err(Problems::from(failed.why.clone()));
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
    fn start_unless_newer(
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
        rebuilds.failed = None;
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
    fn current(self: &Arc<Self>) -> Result<Current, Unbuilt> {
        let (revision, gated, reaching) = {
            let held = lock(&self.held);
            (held.revision, Arc::clone(&held.gated), held.reaching())
        };
        let reached = reaching.reach();
        match &*gated {
            Gated::Checked(checked) => self
                .model(revision, checked)
                .map(|model| Current {
                    model,
                    read: ModelRead {
                        revision,
                        of: ModelChoice::Current,
                    },
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
    fn published_now(self: &Arc<Self>) -> (Option<SessionInfo>, Result<Published, Unbuilt>) {
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
    /// `revision`'s rebuild started if nothing of it, or newer, is published or running, and
    /// its recompute hasn't failed (a failure isn't started again each frame, but left for
    /// `check` to say once and rebuild). Refused with that failure, or, with nothing
    /// published, naming `model`.
    fn published_read(
        self: &Arc<Self>,
        revision: Revision,
        checked: &Arc<Checked>,
    ) -> Result<Published, Problems> {
        let mut rebuilds = lock(&self.rebuilds);
        let failed = rebuilds
            .failed
            .as_ref()
            .filter(|failed| failed.revision == revision)
            .map(|failed| failed.why.clone());
        if failed.is_none()
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
            None => Err(Problems::from(
                failed.unwrap_or_else(|| nothing_published(self.id)),
            )),
        }
    }

    /// The session as it is now, `reloaded` saying whether it reloaded its file before the
    /// command.
    fn info(&self, reloaded: bool) -> SessionInfo {
        self.info_of(&lock(&self.held), reloaded)
    }

    /// The session as `held` says it is, `reloaded` saying whether it reloaded its file
    /// before the command.
    fn info_of(&self, held: &Held, reloaded: bool) -> SessionInfo {
        SessionInfo {
            id: self.id,
            path: held.path.clone(),
            revision: held.revision,
            transaction: held.transaction.is_some(),
            reloaded,
        }
    }

    /// The text and the gate's finding, as held now.
    fn document(&self) -> (Arc<Vec<u8>>, Arc<Gated>) {
        let held = lock(&self.held);
        (Arc::clone(&held.text), Arc::clone(&held.gated))
    }

    /// Queues `job`; or, if the session has closed, tells it so at once.
    fn queue(&self, job: Job<C>) {
        let queue = lock(&self.queue);
        match queue.as_ref() {
            Some(sender) => {
                if let Err(mpsc::SendError(job)) = sender.send(job) {
                    drop(queue);
                    job(Turn::Closed(self.id));
                }
            }
            None => {
                drop(queue);
                job(Turn::Closed(self.id));
            }
        }
    }

    /// Ends its queue: commands queued after it are told the session closed.
    fn close(&self) {
        lock(&self.queue).take();
    }

    fn closed(&self) -> bool {
        lock(&self.queue).is_none()
    }

    /// `begin`: the session's state kept, and a transaction opened; refused if one is open.
    /// The newest model published is kept with it, and its revision's model if it has been
    /// published, or, if its rebuild is running, once that publishes it.
    fn begin(&self) -> Result<Begun, Problems> {
        let mut held = lock(&self.held);
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
    /// touched.
    fn rollback(self: &Arc<Self>) -> Result<RolledBack, Problems> {
        let mut held = lock(&self.held);
        self.transaction(&held)?;
        let revision = held.next()?;
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
            rebuilds.failed = None;
            match (kept.model, &*begun.gated) {
                (Some(model), _) => rebuilds.published = Some(Published { revision, model }),
                (None, Gated::Checked(checked)) if kept.started => {
                    if let Err(problems) = self.start_unless_newer(&mut rebuilds, revision, checked)
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
    /// canonical form through `file::save`, with new fingerprints and solves for what the
    /// transaction's edits reach ([`Reaching::reach`]) and every other feature's as stored
    /// (decision 28), its new file claimed with `claim`; the transaction ended, and the
    /// session at a new revision, its saved text, with only the reached features whose
    /// records couldn't be written still carried. A document equal to the one `begin` saw is
    /// written nowhere, and the session's text is `begin`'s again (the file's as the session
    /// last read or wrote it), even if the file was found changed outside the session
    /// (`changed_outside`, before the command), which then waits for no rebuild. Refused,
    /// the transaction left open and nothing written, if none is open, the file was found
    /// changed or gone before the command (a conflict, before any rebuild) or unreadable
    /// (its `io_failed`), the model can't be had (a kernel fault, until decision 32 saves
    /// around one), the canonical text is too long, or the save is (a file changed since: a
    /// conflict too).
    fn commit(
        self: &Arc<Self>,
        changed_outside: Option<Outside>,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
    ) -> Result<Committed, Problems> {
        let state = {
            let held = lock(&self.held);
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
        self.save_state(state, Through::Transaction, read, claim)
    }

    /// An edit's `candidate`, made outside a transaction, committed as its own transaction
    /// (decision 1): begun from the state held, applied, and committed, as [`commit`]
    /// commits, but never held until it's saved. Saved, the session is at the next revision,
    /// the text written; refused, the session is exactly as it was, at its revision, holding
    /// what it held, and its models as they were (the candidate's never published), and the
    /// response reads no model.
    ///
    /// [`commit`]: Self::commit
    fn commit_candidate(
        self: &Arc<Self>,
        candidate: Candidate,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
    ) -> Result<Committed, Problems> {
        let state = {
            let held = lock(&self.held);
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
        let committed = self.save_state(state, Through::Candidate, read, claim);
        if committed.is_err() {
            *read = None;
        }
        committed
    }

    /// `state` saved (a commit's body): its model, of `state.revision`, waited for, then its
    /// document in canonical form through `file::save`, with new fingerprints and solves for
    /// what the edits since `state.begun` reach and every other feature's as stored, the new
    /// file claimed with `claim`, and the session at a new revision, its saved text (and the
    /// transaction, if one is open, ended). A document equal to `begun`'s is written nowhere.
    /// `through` says whose state it is: a transaction's (held, its model the session's
    /// revision's, and a commit that writes nothing ends it), or an edit's candidate (held
    /// only once saved, its model rebuilt for this commit alone). Refused, nothing written
    /// and nothing held changed, as [`commit`](Self::commit) is.
    fn save_state(
        self: &Arc<Self>,
        state: Saving,
        through: Through,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
    ) -> Result<Committed, Problems> {
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
        let reach = Reaching {
            gated: Arc::clone(&gated),
            carried,
            begun: Some(begun.clone()),
        }
        .reach();
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
            self.unwritten(&begun, revision)?;
            return Ok(Committed {
                written: false,
                path,
                refingerprinted: Vec::new(),
                resolved: Vec::new(),
                problems: gated.problems(),
            });
        }
        let checked = match &*gated {
            Gated::Checked(checked) => checked,
            refused if unchanged => {
                let problems = refused.problems();
                if through == Through::Transaction {
                    self.unwritten(&begun, revision)?;
                }
                return Ok(Committed {
                    written: false,
                    path,
                    refingerprinted: Vec::new(),
                    resolved: Vec::new(),
                    problems,
                });
            }
            refused => return Err(refused.problems()),
        };
        let model = match through {
            Through::Transaction => self.model(revision, checked)?,
            Through::Candidate => self.candidate_model(revision, checked)?,
        };
        *read = Some(ModelRead {
            revision,
            of: ModelChoice::Current,
        });
        let formatted =
            super::formatted(&model, &reach, &model.drifted()).map_err(Problems::from)??;
        if unchanged {
            if through == Through::Transaction {
                self.unwritten(&begun, revision)?;
            }
            return Ok(Committed {
                written: false,
                path,
                refingerprinted: Vec::new(),
                resolved: Vec::new(),
                problems: formatted.problems,
            });
        }
        let saved = Arc::new(formatted.text.into_bytes());
        // A candidate's model is the saved revision's too, unless the save wrote a sketch's
        // solve, which a rebuild of the saved document would solve again from.
        let publish = through == Through::Candidate && formatted.resolved.is_empty();
        self.write(saved, &model, &reach, publish, saver, claim)?;
        Ok(Committed {
            written: true,
            path,
            refingerprinted: formatted
                .refingerprinted
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
            resolved: formatted.resolved,
            problems: formatted.problems,
        })
    }

    /// The model of an edit's candidate, `checked`, which its save would put the session at
    /// `revision` with: rebuilt for the commit of an edit made outside a transaction, and
    /// waited for. Never published as it is: its document holds the fingerprints stored
    /// before the save, so it's the model neither of the state the save leaves (a check of
    /// it would say the fingerprints written drifted) nor of any the session was at; the
    /// save publishes it rebound to the saved document ([`write`](Self::write)). While it runs
    /// it's the session's rebuild running, labelled with `revision`, so a published read of
    /// the session's revision starts none that would supersede it. Refused if the session
    /// closes meanwhile (its rebuild stopped), or the recompute fails (a bug).
    fn candidate_model(
        self: &Arc<Self>,
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

    /// A commit that writes nothing (its document is the one `begin` saw, `begun`, at
    /// `revision` now): the transaction ended, and the session's text `begin`'s, which is
    /// the file's as the session last read or wrote it (a change made outside since is
    /// reloaded by the next command), at a new revision if the edits left other bytes (the
    /// same document in canonical form, say), whose model is `revision`'s if it was
    /// published.
    fn unwritten(&self, begun: &Snapshot, revision: Revision) -> Result<(), Problems> {
        let mut held = lock(&self.held);
        let next = if held.text == begun.text {
            None
        } else {
            Some(held.next()?)
        };
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
            // The same document: its model is the new revision's too.
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
    /// which keep what's stored with them and are problems; written through `file::save`
    /// if `write` and it differs from the file as the session last read or wrote it (a
    /// change made outside the session before the command was reloaded, decision 2), the
    /// new file claimed with `claim`, the session then at a new revision, its saved
    /// text. Refused, writing nothing, inside a transaction, for a document the gate
    /// refused (its problems), for a feature in `accept` the document hasn't, and as a
    /// commit is.
    fn format(
        self: &Arc<Self>,
        accept: Option<&[FeatureId]>,
        write: bool,
        read: &mut Option<ModelRead>,
        claim: &dyn Fn(&Path),
    ) -> Result<Reformatted, Problems> {
        let (path, bytes, gated, revision, reaching) = {
            let held = lock(&self.held);
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
        let reach = reaching.reach();
        let checked = match &*gated {
            Gated::Checked(checked) => checked,
            refused => return Err(refused.problems()),
        };
        let document = checked.document();
        let accept: Option<BTreeSet<&FeatureId>> = match accept {
            None => None,
            Some(accept) => Some(named(document, accept)?),
        };
        let model = self.model(revision, checked)?;
        *read = Some(ModelRead {
            revision,
            of: ModelChoice::Current,
        });
        // Each feature's drift, once: a feature edits reached isn't drift.
        let drift = model.drifted();
        let drifted: BTreeSet<&FeatureId> = model
            .drift_left(&drift, &reach)
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
        let formatted = super::formatted(&model, &writes, &drift).map_err(Problems::from)??;
        let saved = formatted.text.into_bytes();
        let changed = saved != *bytes;
        let written = write && changed;
        if written {
            self.write(Arc::new(saved), &model, &reach, false, Saver::Format, claim)?;
        }
        Ok(Reformatted {
            changed,
            written,
            path,
            refingerprinted: formatted
                .refingerprinted
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
            resolved: formatted.resolved,
            accepted,
            problems: formatted.problems,
        })
    }

    /// The save of a commit or a format: `saved` written over the document's file, which
    /// the session last read or wrote as the bytes it holds ([`save`](Self::save)), its new
    /// file claimed with `claim`; then the session at a new revision whose text is `saved`,
    /// carrying the features of `reach` whose records `model` couldn't write, and any
    /// transaction ended. With `publish`, `model` rebound to the saved document
    /// ([`Model::rebound`]) is published as the new revision's, in the same step: for a
    /// save that wrote only fingerprints, whose document a rebuild would build from the
    /// steps `model` took. Refused, nothing changed, as the save is, a conflict with what
    /// `saver` advises.
    fn write(
        &self,
        saved: Arc<Vec<u8>>,
        model: &Model,
        reach: &BTreeSet<FeatureId>,
        publish: bool,
        saver: Saver,
        claim: &dyn Fn(&Path),
    ) -> Result<(), Problems> {
        let (next, path, resolved, bytes) = {
            let held = lock(&self.held);
            (
                held.next()?,
                held.path.clone(),
                held.resolved.clone(),
                Arc::clone(&held.bytes),
            )
        };
        self.save(&path, &resolved, &bytes, &saved, saver)?;
        claim(&resolved);
        let carried = reach
            .iter()
            .filter(|id| !model.records(id))
            .cloned()
            .collect();
        let gated = Arc::new(Gated::of(&saved));
        let mut held = lock(&self.held);
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
            });
        }
        Ok(())
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
    fn reload(
        &self,
        rereads: Rereads,
        claim: &dyn Fn(Claim<'_>) -> Result<(), Unclaimed>,
    ) -> Result<Reload, Problems> {
        let (path, resolved, bytes, transaction, part_written) = {
            let held = lock(&self.held);
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
        // since, to another session's path, say.
        let found_at = std::fs::canonicalize(&resolved).unwrap_or_else(|_| resolved.clone());
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
                lock(&self.held).part_written = None;
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
                    shown(Path::new(&path))
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
        let next = lock(&self.held).next()?;
        claimed(opened)?;
        let now = Arc::new(now);
        let mut held = lock(&self.held);
        // Nothing carried: a reloaded session is as a fresh `open`.
        held.change(next, Arc::clone(&now), gated, Arc::new(BTreeSet::new()));
        held.bytes = now;
        held.last_reload = Some(next);
        drop(held);
        tracing::info!(session = %self.id, revision = next.number(), "the file changed outside the session: reloaded");
        Ok(Reload::Reloaded)
    }

    /// `text` saved over the document's file at `resolved` (`path` as `open` was given it),
    /// which the session last read or wrote as `bytes`: refused, nothing written, if the
    /// file changed since (a conflict, naming the file, with what `saver` advises) or can't
    /// be written; or if this session's own save left it part written before, saying where
    /// its old text is. A save that leaves it so is marked, for the next to say.
    fn save(
        &self,
        path: &str,
        resolved: &Path,
        bytes: &[u8],
        text: &[u8],
        saver: Saver,
    ) -> Result<(), Problems> {
        if let Some(old) = &lock(&self.held).part_written {
            return Err(Problems::from(part_written(path, old)));
        }
        file::save_over(resolved, bytes, text).map_err(|unsaved| self.unsaved(path, unsaved, saver))
    }

    /// The refusal of a save of the document at `path` (as `open` was given it) that failed,
    /// as `unsaved` says: a conflict, naming the file, with what `saver` advises, or why it
    /// couldn't be written; and, if it left the file part written, that marked, for the next
    /// save to say.
    fn unsaved(&self, path: &str, unsaved: file::Unsaved, saver: Saver) -> Problems {
        if let Some(old) = unsaved.old_text_in {
            lock(&self.held).part_written = Some(old);
        }
        let error = unsaved.error;
        Problems::from(if error.code() == ErrorCode::Conflict {
            saver.conflict(path)
        } else {
            error.with_parameter("path")
        })
    }

    /// An accepted edit's `candidate` held as the session's document, at a new revision,
    /// carrying what was carried: how an edit inside a transaction is applied (the design's
    /// section 4), starting no rebuild (decision 13).
    fn hold(&self, candidate: Candidate) -> Result<Revision, Problems> {
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
    fn edit<T>(
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
            });
        };
        let commit = if transaction {
            self.hold(candidate)?;
            None
        } else {
            Some(self.commit_candidate(candidate, read, claim)?)
        };
        Ok(Edited {
            made,
            changed: true,
            commit,
        })
    }
}

/// Whose save found the file changed outside the session (decision 2), which says what to
/// do about it: nothing merges, and the session's next command outside a transaction
/// reloads the file.
#[derive(Clone, Copy)]
enum Saver {
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
                shown(Path::new(path))
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

/// What an edit is made from ([`Session::edit`]): the session's ID, its text, the gate's
/// finding of it, and whether a transaction is open.
struct Base<'a> {
    id: SessionId,
    text: &'a [u8],
    gated: &'a Gated,
    transaction: bool,
}

impl<'a> Base<'a> {
    /// The document a typed edit (`add_feature`, `set_field`, `remove_feature`) edits: one
    /// the gate took. A document it refused is mended by `replace_text` alone (decision 21),
    /// so every other edit of it is refused.
    fn checked(&self) -> Result<&'a Document, Problems> {
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
/// changed, and, outside a transaction, the commit's result (none inside one, or if nothing
/// changed).
struct Edited<T> {
    made: T,
    changed: bool,
    commit: Option<Committed>,
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

/// The refusal of a file at `path` (as `open` was given it) longer than Silico reads of a
/// document: a session couldn't hold its text, or mend it, whole.
fn too_long(path: &Path) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!(
            "{} is more than the {MAX_INPUT_LEN} bytes Silico reads of a document",
            shown(path)
        ),
    )
    .with_parameter("path")
    .with_suggestion("check it's the document you mean: no document is this long")
}

/// `error`, a file's (as `file::opened_and_read` gives it, saying nothing of which file),
/// naming the file at `path` (as `open` was given it): its code, and its message after the
/// file's name.
fn of_file(path: &str, error: &Error) -> Error {
    Error::new(
        error.code(),
        format!("{} {}", shown(Path::new(path)), error.message()),
    )
}

/// `error`, naming the file, why a session couldn't take its file as it is now, before a
/// command (decision 2): gone, unreadable, too long, or another session's. The command is
/// refused, with `suggestion`, how to go on.
fn not_reloaded(error: Error, suggestion: String) -> Error {
    error
        .with_parameter("path")
        .with_cause(
            "the session reads its file before each command to find changes made outside it",
        )
        .with_suggestion(suggestion)
}

/// What to do about a session's file at `path` (as `open` was given it) gone or grown too
/// long, outside a transaction.
fn put_back(path: &str) -> String {
    format!(
        "put the file back at {}, or close the session and open the file where it is now",
        shown(Path::new(path))
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

/// The refusal of session `id`'s document replaced outside a transaction when its text
/// isn't UTF-8 (`valid_up_to` its valid bytes): `replace_text` gives back the text it
/// replaces, and couldn't give back this one.
fn not_given_back(id: SessionId, valid_up_to: usize) -> Error {
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

/// The refusal of a save after this session's own save left the file at `path` part
/// written, its old text in `old`.
fn part_written(path: &str, old: &Path) -> Error {
    Error::new(
        ErrorCode::IoFailed,
        format!(
            "{} was left part written by this session's own save, which couldn't put its text back: its text before is in {}",
            shown(Path::new(path)),
            shown(old)
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
fn transaction_open(id: SessionId, then: &str) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("a transaction is open in session {id}"),
    )
    .with_parameter("session")
    .with_suggestion(format!("commit or roll back the transaction, then {then}"))
}

/// The current revision's model, as a command that waited for it reads it.
struct Current {
    model: Arc<Model>,
    read: ModelRead,
    /// What the session's edits reach at its revision.
    reached: BTreeSet<FeatureId>,
}

/// Why a session's model couldn't be read.
enum Unbuilt {
    /// The gate refused the document: nothing is built of it, and these are its problems.
    Gated(Problems),
    /// No model could be had: the session closed or was dropped, its recompute failed, or
    /// (a published read) nothing's been published.
    Unavailable(Problems),
}

impl Unbuilt {
    fn problems(self) -> Problems {
        match self {
            Unbuilt::Gated(problems) | Unbuilt::Unavailable(problems) => problems,
        }
    }
}

/// A published read's response: `then` on the model read, if there was one, the response
/// naming it; or why there wasn't.
fn published_answer<T>(
    info: Option<SessionInfo>,
    read: Result<Published, Unbuilt>,
    then: impl FnOnce(&Published, ModelRead) -> Result<T, Problems>,
) -> Response<T> {
    match read {
        Ok(published) => {
            let read = ModelRead {
                revision: published.revision,
                of: ModelChoice::Published,
            };
            Response {
                session: info,
                model: Some(read),
                result: then(&published, read),
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
/// which a rebuild may not meet again.
fn faulted(model: &Model) -> bool {
    // As `check` finds one, in a feature's failure or a measure its drift needed, so the
    // two never differ (decision 32 changes both, there).
    super::check(model).is_err()
}

/// The refusal of a published read with nothing published yet, its rebuild started or
/// running: the design's section 2.
fn nothing_published(id: SessionId) -> Error {
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

/// Waits for a rebuild, `run`, then records what it came to in `session`'s rebuilds and
/// says so, if the session is still there: as the revision its `label` says now (the last a
/// rollback gave it to, if any did); and, if it's still the rebuild running, as running no
/// more (one stopped and started again, or another's since, is left running).
fn watch<C: Kernel + 'static>(session: &Weak<Session<C>>, label: &Label, run: Run) {
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
        // Whoever needs its model starts it again.
        Ok(RunOutcome::Superseded) => {}
        Err(why) => {
            rebuilds.failed = Some(Failed {
                revision,
                why,
                said: false,
            });
        }
    }
    drop(rebuilds);
    session.rebuilt.notify_all();
}

impl<P: Clients + 'static> Sessions<P> {
    /// Sessions on `kernel`, keeping each directory's results in its `.silico-cache/`
    /// ([`Cache::open`]).
    pub fn new(kernel: P) -> Self {
        Self::with_cache_opener(kernel, |dir| Cache::open(dir))
    }

    /// Sessions on `kernel`, opening each directory's cache with `open` (given the cache's
    /// directory, `.silico-cache/` beside the document) the first time a session on a
    /// document in it opens (or, if two open at once, by each, the first value kept), every
    /// later session sharing that value: for tests, a cache with a secret of their own. A
    /// cache without the user's secret, which keeps nothing, isn't kept, so the next open
    /// in its directory tries again.
    pub fn with_cache_opener(
        kernel: P,
        open: impl Fn(&Path) -> Cache + Send + Sync + 'static,
    ) -> Self {
        Self::build(kernel, Box::new(open), None)
    }

    /// Sessions on `kernel` with stand-ins, for tests: each directory's cache opened with
    /// `open`, as [`with_cache_opener`](Self::with_cache_opener) does, and each session's
    /// sketches solved with what `start` starts (`fake::FakeSketches`), rather than the
    /// sketch worker. `start` is called from each session's engine, on its own thread, so
    /// it's `Fn` and `Sync`.
    pub fn with_stand_ins(
        kernel: P,
        open: impl Fn(&Path) -> Cache + Send + Sync + 'static,
        start: impl Fn() -> Result<Box<dyn SketchSolver>, Error> + Send + Sync + 'static,
    ) -> Self {
        Self::build(kernel, Box::new(open), Some(Arc::new(start)))
    }

    fn build(
        kernel: P,
        open_cache: Box<CacheOpener>,
        sketches: Option<Arc<SketchStarter>>,
    ) -> Self {
        Sessions {
            registry: Arc::new(Registry {
                kernel,
                open_cache,
                sketches,
                feature_ids: Arc::new(Mutex::new(FeatureIds::system())),
                book: Mutex::new(Book {
                    next: 1,
                    sessions: BTreeMap::new(),
                    opening: BTreeSet::new(),
                    files: BTreeMap::new(),
                    paths: BTreeMap::new(),
                    caches: BTreeMap::new(),
                    threads: Threads::new(),
                    reads: Threads::new(),
                    stopping: false,
                }),
            }),
        }
    }

    /// Opens the document at `path` as a new session, on a thread of its own: reads the
    /// file (a regular file, no more than Silico reads of a document) and puts it through
    /// the gate. The session opens even if the gate refuses the document, holding it and the
    /// gate's problems (decision 21). With `create`, writes a new, empty document at `path`
    /// in canonical form first, refusing if anything is there. Refused if the file is open
    /// in another session of this process (by any path to it), if [`MAX_SESSIONS`] are open
    /// or opening, or if `path` isn't UTF-8 (a response couldn't name it exactly). The
    /// session's ID is taken when the open is asked for, since its thread is named by it:
    /// an open refused on that thread (no such file, open already) uses an ID no session
    /// then has, and one refused here (a path not UTF-8, too many open, the sessions
    /// closing) uses none.
    pub fn open(&self, path: impl Into<PathBuf>, create: bool) -> Running<Opened> {
        // Every response names the session's path exactly, so one it can't name isn't opened
        // (or made), and takes no ID; from here on it's UTF-8, as the session holds it.
        let path = match path.into().into_os_string().into_string() {
            Ok(path) => path,
            Err(path) => {
                return Running::answered(refused(
                    Error::new(
                        ErrorCode::InvalidInput,
                        format!(
                            "{} isn't UTF-8, so the API can't name it exactly",
                            shown(Path::new(&path))
                        ),
                    )
                    .with_parameter("path")
                    .with_suggestion("rename the file, or open it through a path that is UTF-8"),
                ));
            }
        };
        let mut book = lock(&self.registry.book);
        if book.stopping {
            return Running::answered(refused(closing()));
        }
        book.reap();
        if book.sessions.len() + book.opening.len() >= MAX_SESSIONS {
            return Running::answered(refused(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!("{MAX_SESSIONS} sessions are open, the most one process holds"),
                )
                .with_suggestion("close a session you're done with, then open this one"),
            ));
        }
        let Some(after) = book.next.checked_add(1) else {
            return Running::answered(refused(
                Error::new(
                    ErrorCode::ResourceExhausted,
                    "this process has no session IDs left to give",
                )
                .with_suggestion("start Silico again"),
            ));
        };
        let id = SessionId(book.next);
        book.next = after;
        let (reply, answer) = mpsc::sync_channel(1);
        let registry = Arc::clone(&self.registry);
        let caller = tracing::Span::current();
        // Spawned with the book held, so the thread is recorded opening before it can end.
        let started = std::thread::Builder::new()
            .name(format!("session-{id}"))
            .spawn(move || {
                // However the thread ends, even by a panic, the session is gone from the
                // records, and its file free.
                let _release = Release {
                    registry: Arc::downgrade(&registry),
                    id,
                };
                let started = {
                    let _caller = caller.enter();
                    let _span =
                        tracing::info_span!("session_command", session = %id, command = "open")
                            .entered();
                    start(registry, id, &path, create, &reply)
                };
                if let Some(Started { session, jobs }) = started {
                    serve(&session, &jobs);
                }
            });
        match started {
            Ok(thread) => {
                book.opening.insert(id);
                book.threads.push(thread);
                Running::new(answer, SESSION_STOPPED)
            }
            Err(e) => Running::answered(refused(crate::pool::thread_error("session", &e))),
        }
    }

    /// Closes `session`, once the commands queued before it have run: commands queued
    /// after it are told the session closed. Its file is then free to open again. Refused
    /// while a transaction is open (commit or roll back first), so nothing is dropped
    /// without the client saying so. It never reads the file (decision 2), so a file gone or
    /// changed never keeps it open.
    pub fn close(&self, session: &SessionId) -> Running<()> {
        let registry = Arc::downgrade(&self.registry);
        self.queued_with(session, "close", Rereads::Never, move |session, _, _| {
            if lock(&session.held).transaction.is_some() {
                return Err(Problems::from(transaction_open(
                    session.id,
                    "close the session",
                )));
            }
            session.close();
            if let Some(registry) = registry.upgrade() {
                lock(&registry.book).release(session.id);
            }
            Ok(())
        })
    }

    /// `session`'s status, answered on the caller's thread from what the session holds,
    /// even while one of its commands runs: it never reads the file (decision 2).
    pub fn status(&self, session: &SessionId) -> Response<Status> {
        let Some(found) = self.find(session) else {
            return refused(not_open(*session));
        };
        let held = lock(&found.held);
        Response {
            session: Some(found.info_of(&held, false)),
            model: None,
            result: Ok(Status {
                features: held.gated.document().ok().map(|d| d.features.len()),
                gated: matches!(*held.gated, Gated::Checked(_)),
                transaction: held.transaction.as_ref().map(|begun| Begun {
                    base: begun.revision,
                }),
            }),
        }
    }

    /// The text `session`'s document is: the file's bytes exactly, until an edit; or, given
    /// a `feature`, that feature's canonical text ([`document::write_feature`]). Refused if
    /// the text isn't UTF-8 (the reader refused it), or, for a feature, if the text doesn't
    /// read as a document or hasn't one feature of that ID.
    pub fn text(&self, session: &SessionId, feature: Option<FeatureId>) -> Running<String> {
        self.queued(session, "text", move |session, _| {
            let (text, gated) = session.document();
            match feature {
                None => String::from_utf8(text.to_vec()).map_err(|e| {
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
                    let document = gated.document().map_err(Clone::clone)?;
                    Ok(document::write_feature(document.feature(&id)?)?)
                }
            }
        })
    }

    /// `session`'s document's problems ([`super::check`]): the gate's, if it refused the
    /// document, or, from the session's current revision rebuilt (waited for, and started if
    /// it hasn't been: decision 13), each failed feature's and each drift of a feature no
    /// edit reached since the last save (decision 28: a reached feature's stored fingerprint
    /// was measured from other inputs, and a commit writes its new one). A kernel fault (a
    /// worker that crashed or hung) is the command's refusal, as `command::check` gives it,
    /// and the next check rebuilds, until faults are reported beside the result and kept as
    /// the revision's outcome (decision 32).
    pub fn check(&self, session: &SessionId) -> Running<Problems> {
        self.queued(session, "check", |session, model| match session.current() {
            Ok(current) => {
                *model = Some(current.read);
                let _span = tracing::info_span!("check").entered();
                super::check_except(&current.model, &current.reached).map_err(Problems::from)
            }
            Err(Unbuilt::Gated(problems)) => Ok(problems),
            Err(Unbuilt::Unavailable(problems)) => Err(problems),
        })
    }

    /// Runs `read` on the model `choice` names, off the caller's thread (decision 33), as
    /// the command named `command` (for its trace, and the session's record of reads).
    /// `Current` is queued on the session and waits for its revision's rebuild, as `check`
    /// does; `Published` runs at once on a thread of its own (`session-sN-read`), never
    /// queued, on the newest published model, as [`published`](Self::published) gives it:
    /// starting the current rebuild if needed, refused naming `model` if nothing's published,
    /// and with the gate's (or the reader's) problems for a document the gate refused, with
    /// nothing published. At most [`MAX_PUBLISHED_READS`] of a session's published reads are
    /// unanswered at once: past them, one is refused (each session's own, apart from every
    /// other's). The response's `model` is the [`ModelRead`] of the
    /// model read, which `read` is handed too ([`Read`]); `read`'s error is the response's
    /// refusal.
    pub fn read_model<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: &'static str,
        choice: ModelChoice,
        read: impl FnOnce(Read<'_>) -> Result<T, Problems> + Send + 'static,
    ) -> Running<T> {
        match choice {
            ModelChoice::Current => self.queued(session, command, move |session, model| {
                let current = session.current().map_err(Unbuilt::problems)?;
                *model = Some(current.read);
                read(Read {
                    model: &current.model,
                    read: current.read,
                })
            }),
            ModelChoice::Published => {
                let Some(found) = self.find(session) else {
                    return Running::answered(refused(not_open(*session)));
                };
                let mut reads = lock(&found.reads);
                let Reads {
                    unanswered,
                    threads,
                } = &mut *reads;
                // Released (closed) since it was found: its reads are the registry's now.
                let Some(threads) = threads else {
                    return Running::answered(refused(not_open(found.id)));
                };
                threads.reap();
                // Refusals of an open session say it, as every command's do.
                let refused_here = |error: Error| {
                    Running::answered(Response {
                        session: Some(found.info(false)),
                        model: None,
                        result: Err(Problems::from(error)),
                    })
                };
                if unanswered.load(Ordering::SeqCst) >= MAX_PUBLISHED_READS {
                    return refused_here(
                        Error::new(
                            ErrorCode::InvalidInput,
                            format!(
                                "{MAX_PUBLISHED_READS} published reads are running in session {}, the most one session runs at once",
                                found.id
                            ),
                        )
                        .with_suggestion("wait for one to answer, then read again"),
                    );
                }
                let (reply, answer) = mpsc::sync_channel(1);
                let caller = tracing::Span::current();
                let session = Arc::clone(&found);
                let counted = Unanswered::count(unanswered);
                let started = std::thread::Builder::new()
                    .name(format!("session-{}-read", found.id))
                    .spawn(move || {
                        let _caller = caller.enter();
                        let _span = tracing::info_span!(
                            "session_command",
                            session = %session.id,
                            command,
                            model = "published"
                        )
                        .entered();
                        let (info, published) = session.published_now();
                        let response = published_answer(info, published, |published, model| {
                            read(Read {
                                model: &published.model,
                                read: model,
                            })
                        });
                        drop(counted);
                        let _ = reply.send(response);
                    });
                match started {
                    Ok(thread) => {
                        threads.push(thread);
                        Running::new(answer, READ_STOPPED)
                    }
                    // The read, and its count, dropped with the thread that never started.
                    Err(e) => refused_here(crate::pool::thread_error("published read", &e)),
                }
            }
        }
    }

    /// The newest model `session`'s rebuilds have published (the design's published read,
    /// section 2), of the session's revision or an older one, its revision in the response's
    /// `model`. Answered on the caller's thread, never waiting for a rebuild, for the app to
    /// draw each frame; it starts the current revision's rebuild if that has no model and
    /// none is running (decision 13), unless its recompute failed (then that failure is the
    /// refusal, until a check rebuilds it). With nothing published, refused naming `model`;
    /// or, for a document the gate refused, with the gate's problems. Every read command's
    /// published read is [`read_model`](Self::read_model)'s, which gives the same model.
    pub fn published(&self, session: &SessionId) -> Response<Published> {
        let Some(found) = self.find(session) else {
            return refused(not_open(*session));
        };
        let (info, read) = found.published_now();
        published_answer(info, read, |published, _| Ok(published.clone()))
    }

    /// `session`'s document's outline ([`super::outline`]), as `silico show` prints it.
    /// Refused, with the reader's error, if the text doesn't read as a document.
    pub fn outline(&self, session: &SessionId) -> Running<String> {
        self.queued(session, "outline", |session, _| {
            let (_, gated) = session.document();
            let document = gated.document().map_err(Clone::clone)?;
            Ok(super::outline(document))
        })
    }

    /// Begins a transaction in `session` (the design's section 4): its state kept (its text,
    /// the gate's finding, its model or the rebuild running for it, its revision, what its
    /// edits reached) for `rollback` to restore. Inside it, edits are applied without being
    /// saved or starting a rebuild. Refused if one is open: transactions don't nest.
    pub fn begin(&self, session: &SessionId) -> Running<Begun> {
        self.queued(session, "begin", |session, _| session.begin())
    }

    /// Ends `session`'s transaction, saving its document (the design's section 4): waits for
    /// the current revision's rebuild, then writes the canonical text through `file::save`,
    /// with new fingerprints and solves only for the features its edits reached (decision
    /// 28; every other keeps what's stored, and its drift is a problem, never written over),
    /// and moves the session to a new revision, the text written. A document equal to the
    /// one `begin` saw writes nothing, even over a file changed outside the session. Refused,
    /// the transaction left open, if none is open, if the file changed since the session
    /// read or wrote it or is gone (`conflict`), or can't be read (`io_failed`), if its text
    /// would pass the limit, if it can't be written (`io_failed`), or if a kernel fault kept
    /// the rebuild from being had (until faults are saved around, decision 32).
    pub fn commit(&self, session: &SessionId) -> Running<Committed> {
        let registry = Arc::downgrade(&self.registry);
        self.queued_with(
            session,
            "commit",
            Rereads::Always,
            move |session, model, reload| {
                let id = session.id;
                session.commit(reload.outside(), model, &|resolved| {
                    reclaim(&registry, id, resolved);
                })
            },
        )
    }

    /// Ends `session`'s transaction, restoring what `begin` kept, at a new revision whose
    /// state equals the one `begin` saw: the text byte for byte, the gate's finding, what
    /// edits reached, and the model `begin` saw, the same one (or, if it was never made, the
    /// restored document's rebuild started). Never touches the file. Refused if no
    /// transaction is open.
    pub fn rollback(&self, session: &SessionId) -> Running<RolledBack> {
        self.queued(session, "rollback", |session, _| session.rollback())
    }

    /// Saves `session`'s document in canonical form with every feature's fingerprint and
    /// every sketch's solve as rebuilt, edited or not (decision 31: what `silico fmt` runs,
    /// and how drift is accepted, by name): with `features`, only those features' drift is
    /// accepted, and every other drifted feature keeps what's stored with it and is a
    /// problem. Written through `file::save` only if `write` and the text differs from the
    /// file; with `write` false, says whether it would be. Refused inside a transaction
    /// (commit or roll back first), for a document the gate refuses, and for a feature in
    /// `features` the document hasn't; and, writing, as a commit is.
    pub fn format(
        &self,
        session: &SessionId,
        features: Option<Vec<FeatureId>>,
        write: bool,
    ) -> Running<Reformatted> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(session, "format", move |session, model| {
            let id = session.id;
            session.format(features.as_deref(), write, model, &|resolved| {
                reclaim(&registry, id, resolved);
            })
        })
    }

    /// Appends `feature`, a feature as the document writes it, to the end of `session`'s
    /// timeline (the design's `add_feature`), as an edit made against `revision`: its `id`
    /// may be left out, for one made from the system's randomness that no feature holds
    /// (decision 20), and its `v`, for the type's current version; a `fingerprint` is
    /// refused (a commit writes it). Through the gate, all at once; inside a transaction,
    /// applied, starting no rebuild; outside one, saved as its own transaction (decision 1).
    /// Refused whole, the session as it was, as every edit is ([`Session::edit`]), and for a
    /// document the gate refused (only `replace_text` mends one).
    pub fn add_feature(
        &self,
        session: &SessionId,
        revision: Revision,
        feature: Node,
    ) -> Running<Added> {
        let registry = Arc::downgrade(&self.registry);
        let ids = Arc::clone(&self.registry.feature_ids);
        self.queued(session, "add_feature", move |session, model| {
            let id = session.id;
            let edited = session.edit(
                revision,
                |base| {
                    let (added, candidate) =
                        edit::add_feature(base.checked()?, feature, |document| {
                            lock(&ids).next_free(document)
                        })?;
                    Ok((added, Some(candidate)))
                },
                model,
                &|resolved| reclaim(&registry, id, resolved),
            )?;
            Ok(Added {
                feature: edited.made,
                changed: edited.changed,
                commit: edited.commit,
            })
        })
    }

    /// Sets `field` of `session`'s feature `feature` to `value` (the design's `set_field`),
    /// as an edit made against `revision`: the field named by its path (decision 15: a
    /// sketch's members by ID only, any other array's items by place), the value read as the
    /// document reader reads that field, replacing it whole; a note set to `null` removed.
    /// A feature's `id`, `type`, `v`, and `fingerprint` can't be set. Setting a field to the
    /// value it has changes nothing: no new revision, nothing written. Otherwise as
    /// [`add_feature`](Self::add_feature), and an ID the edit gives two of a sketch's
    /// members refused at `field`, naming the member that holds it.
    pub fn set_field(
        &self,
        session: &SessionId,
        revision: Revision,
        feature: FeatureId,
        field: String,
        value: Node,
    ) -> Running<FieldSet> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(session, "set_field", move |session, model| {
            let id = session.id;
            let edited = session.edit(
                revision,
                |base| edit::set_field(base.checked()?, &feature, &field, value),
                model,
                &|resolved| reclaim(&registry, id, resolved),
            )?;
            Ok(FieldSet {
                feature,
                field,
                before: edited.made.before,
                after: edited.made.after,
                changed: edited.changed,
                commit: edited.commit,
            })
        })
    }

    /// Removes `session`'s feature `feature` (the design's `remove_feature`), as an edit made
    /// against `revision`, giving it back as the document held it, with its place in the
    /// timeline, so `replace_text` can put it back. Refused, as the gate refuses it, with each
    /// feature that uses it naming it (the client removes those first); otherwise as
    /// [`add_feature`](Self::add_feature).
    pub fn remove_feature(
        &self,
        session: &SessionId,
        revision: Revision,
        feature: FeatureId,
    ) -> Running<Removed> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(session, "remove_feature", move |session, model| {
            let id = session.id;
            let edited = session.edit(
                revision,
                |base| {
                    let (removed, index, candidate) =
                        edit::remove_feature(base.checked()?, &feature)?;
                    Ok(((removed, index), Some(candidate)))
                },
                model,
                &|resolved| reclaim(&registry, id, resolved),
            )?;
            let (feature, index) = edited.made;
            Ok(Removed {
                feature,
                index,
                changed: edited.changed,
                commit: edited.commit,
            })
        })
    }

    /// Replaces `session`'s document's whole text with `text` (the design's `replace_text`),
    /// as an edit made against `revision`: through the reader and the gate as `silico check`
    /// puts a file through them, refused, naming `naming`, without the header's naming
    /// scheme. The one edit of a document the gate refused (decision 21), which it mends.
    /// Outside a transaction, it gives back the text it replaced (refused if that isn't
    /// UTF-8, which only a transaction can then replace); otherwise as
    /// [`add_feature`](Self::add_feature).
    pub fn replace_text(
        &self,
        session: &SessionId,
        revision: Revision,
        text: String,
    ) -> Running<Replaced> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(session, "replace_text", move |session, model| {
            let id = session.id;
            let edited = session.edit(
                revision,
                |base| {
                    let before = if base.transaction {
                        None
                    } else {
                        Some(
                            std::str::from_utf8(base.text)
                                .map_err(|e| not_given_back(base.id, e.valid_up_to()))?
                                .to_owned(),
                        )
                    };
                    let (features, candidate) = edit::replace_text(base.gated, text.as_bytes())?;
                    Ok(((features, before), candidate))
                },
                model,
                &|resolved| reclaim(&registry, id, resolved),
            )?;
            let (features, before) = edited.made;
            Ok(Replaced {
                changed: edited.changed,
                features,
                before,
                commit: edited.commit,
            })
        })
    }

    fn find(&self, session: &SessionId) -> Option<Arc<Session<P::Client>>> {
        lock(&self.registry.book).sessions.get(session).cloned()
    }

    /// `work` queued on `session`'s thread, under the caller's span and the command's, after
    /// the session reads its file again if no transaction is open ([`Session::reload`],
    /// decision 2), refused if it can't take the file as it is.
    fn queued<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: &'static str,
        work: impl FnOnce(&Arc<Session<P::Client>>, &mut Option<ModelRead>) -> Result<T, Problems>
        + Send
        + 'static,
    ) -> Running<T> {
        self.queued_with(
            session,
            command,
            Rereads::OutsideTransaction,
            move |session, model, _| work(session, model),
        )
    }

    /// [`queued`](Self::queued), the file read again as `rereads` says, and `work` given
    /// what the session found of it.
    fn queued_with<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: &'static str,
        rereads: Rereads,
        work: impl FnOnce(
            &Arc<Session<P::Client>>,
            &mut Option<ModelRead>,
            Reload,
        ) -> Result<T, Problems>
        + Send
        + 'static,
    ) -> Running<T> {
        let Some(found) = self.find(session) else {
            return Running::answered(refused(not_open(*session)));
        };
        let (reply, answer) = mpsc::sync_channel(1);
        let caller = tracing::Span::current();
        let registry = Arc::downgrade(&self.registry);
        found.queue(Box::new(move |turn| {
            let response = match turn {
                Turn::Run(session) => {
                    let _caller = caller.enter();
                    let _span = tracing::info_span!(
                        "session_command",
                        session = %session.id,
                        command
                    )
                    .entered();
                    let mut model = None;
                    let id = session.id;
                    let claimed = |found: Claim<'_>| match registry.upgrade() {
                        Some(registry) => {
                            claim(&registry, found.opened, found.resolved, found.found_at, id)
                        }
                        // The sessions are being dropped: no claim is kept.
                        None => Ok(()),
                    };
                    let (reloaded, result) = match session.reload(rereads, &claimed) {
                        Ok(reload) => (reload.reloaded(), work(session, &mut model, reload)),
                        Err(problems) => (false, Err(problems)),
                    };
                    Response {
                        session: Some(session.info(reloaded)),
                        model,
                        result,
                    }
                }
                Turn::Closed(id) => refused(not_open(id)),
            };
            let _ = reply.send(response);
        }));
        Running::new(answer, SESSION_STOPPED)
    }
}

impl<P: Clients + 'static> Drop for Sessions<P> {
    fn drop(&mut self) {
        let (sessions, threads) = {
            let mut book = lock(&self.registry.book);
            book.stopping = true;
            (
                std::mem::take(&mut book.sessions),
                std::mem::replace(&mut book.threads, Threads::new()),
            )
        };
        // Each queue ended, so its thread ends once its command does, and its rebuild
        // stopped, so a command waiting on one isn't waited for.
        for session in sessions.values() {
            session.close();
            lock(&session.rebuilds).engine.cancel();
        }
        threads.join();
        // And the threads watching its rebuilds, each of which ends once its stopped run
        // does, then its published reads, each of which ends with its read; then the reads
        // of sessions released before (closed, or their threads ended), which the book
        // took: so no session's thread outlives it.
        // Each taken from under its lock before it's joined: a watcher ends by taking the
        // rebuilds' lock, and a read may ask the sessions anything.
        for session in sessions.values() {
            let watchers = std::mem::replace(&mut lock(&session.rebuilds).watchers, Threads::new());
            watchers.join();
            let reads = lock(&session.reads).threads.take();
            if let Some(reads) = reads {
                reads.join();
            }
        }
        let released = std::mem::replace(&mut lock(&self.registry.book).reads, Threads::new());
        released.join();
    }
}

/// Removes its session from the registry's records when dropped: when the session's thread
/// ends, however it ends.
struct Release<P: Clients + 'static> {
    registry: Weak<Registry<P>>,
    id: SessionId,
}

impl<P: Clients + 'static> Drop for Release<P> {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            lock(&registry.book).release(self.id);
        }
    }
}

/// When a queued command's session reads its file first (decision 2; the design's section
/// 4).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rereads {
    /// Outside a transaction only: inside one, the command runs on what the session holds.
    OutsideTransaction,
    /// Inside a transaction too, where nothing is reloaded: what's found is the command's
    /// (`commit`'s) to refuse.
    Always,
    /// Never: `close`, which always closes.
    Never,
}

/// What a session found of its file before a command ([`Session::reload`]), given to the
/// command.
enum Reload {
    /// Nothing to change: the file unread (inside a transaction, or for `close`), or as the
    /// session last read or wrote it.
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
    fn reloaded(&self) -> bool {
        matches!(self, Reload::Reloaded)
    }

    /// How the file was found, inside a transaction, if not as the session last read or
    /// wrote it.
    fn outside(self) -> Option<Outside> {
        match self {
            Reload::Outside(outside) => Some(outside),
            Reload::AsHeld | Reload::Reloaded => None,
        }
    }
}

/// A file found, inside a transaction, not as its session last read or wrote it.
enum Outside {
    /// It holds other bytes (more than a document may be, perhaps), or it's gone (nothing
    /// at its path, or a link there to nothing): a conflict.
    ChangedOrGone,
    /// It couldn't be read, as the error says: an `io_failed`.
    Unreadable(Error),
}

/// A file read before a command ([`Session::reload`]), to be claimed for its session as
/// `open` claims one ([`claim`]): the file opened, the session's path as its links resolved
/// at `open`, and where that path leads now.
struct Claim<'a> {
    opened: std::fs::File,
    resolved: &'a Path,
    found_at: &'a Path,
}

/// A refusal, of no session.
fn refused<T>(error: Error) -> Response<T> {
    Response {
        session: None,
        model: None,
        result: Err(Problems::from(error)),
    }
}

fn not_open(id: SessionId) -> Error {
    Error::new(ErrorCode::InvalidInput, format!("no session {id} is open"))
        .with_parameter("session")
        .with_suggestion(
            "open the document again: a session ends when it's closed or its client goes away",
        )
}

fn closing() -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        "this process's sessions are closing, so none opens",
    )
    .with_suggestion("open it again once the sessions are made again")
}

/// A path as a message says it: quoted, its control and invisible characters escaped, as
/// `file`'s errors and the outline say text, so a file's name can't act on a terminal.
fn shown(path: &Path) -> String {
    super::quoted(&path.display().to_string())
}

/// A session opened on its thread: what the thread serves.
struct Started<C: Kernel + 'static> {
    session: Arc<Session<C>>,
    jobs: Receiver<Job<C>>,
}

/// The open, on its session's thread, answered through `reply`: if it opened, what the
/// thread serves.
fn start<P: Clients + 'static>(
    registry: Arc<Registry<P>>,
    id: SessionId,
    path: &str,
    create: bool,
    reply: &mpsc::SyncSender<Response<Opened>>,
) -> Option<Started<P::Client>> {
    let opened = open(&registry, id, path, create).and_then(|(held, opened, cache, made)| {
        let mut book = lock(&registry.book);
        if book.stopping {
            // The file it made, if it made one, goes with it (`made`'s drop).
            return Err(Problems::from(closing()));
        }
        let (sender, jobs) = mpsc::channel();
        let session = Arc::new(Session {
            id,
            held: Mutex::new(held),
            queue: Mutex::new(Some(sender)),
            rebuilds: Mutex::new(Rebuilds {
                engine: registry.engine(cache),
                running: None,
                published: None,
                failed: None,
                kept: None,
                floor: Revision::OPENED,
                candidate: None,
                watchers: Threads::new(),
            }),
            rebuilt: Condvar::new(),
            reads: Mutex::new(Reads::new()),
        });
        book.opening.remove(&id);
        book.sessions.insert(id, Arc::clone(&session));
        if let Some(made) = made {
            made.keep();
        }
        Ok((session, opened, jobs))
    });
    match opened {
        Ok((session, opened, jobs)) => {
            let _ = reply.send(Response {
                session: Some(session.info(false)),
                model: None,
                result: Ok(opened),
            });
            Some(Started { session, jobs })
        }
        Err(problems) => {
            lock(&registry.book).release(id);
            let _ = reply.send(Response {
                session: None,
                model: None,
                result: Err(problems),
            });
            None
        }
    }
}

/// The empty document `open` made: removed when dropped, unless kept for the session that
/// opened it, if it's still what was made (nothing else has written it since), so an open
/// that ends in no session, refused or by a panic, leaves no file it made.
struct MadeFile {
    path: PathBuf,
    text: Vec<u8>,
    kept: bool,
}

impl MadeFile {
    /// Left on disk: the session it was made for opened.
    fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for MadeFile {
    fn drop(&mut self) {
        if !self.kept && file::read(&self.path).is_ok_and(|bytes| bytes == self.text) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The session's commands, in the order they were queued, until its queue ends: each
/// queued after it closed is told so.
fn serve<C: Kernel + 'static>(session: &Arc<Session<C>>, jobs: &Receiver<Job<C>>) {
    while let Ok(job) = jobs.recv() {
        if session.closed() {
            job(Turn::Closed(session.id));
        } else {
            job(Turn::Run(session));
        }
    }
}

/// The file at `path` read (or, with `create`, made) and through the gate, claimed for
/// session `id`; what the session holds, `open`'s answer, the cache of its directory, and
/// the file it made, if it made one.
fn open<P: Clients + 'static>(
    registry: &Registry<P>,
    id: SessionId,
    given: &str,
    create: bool,
) -> Result<(Held, Opened, Cache, Option<MadeFile>), Problems> {
    let path = Path::new(given);
    let (resolved, text, made) = if create {
        let (resolved, text, made) = made(registry, id, path)?;
        (resolved, text, Some(made))
    } else {
        let resolved =
            std::fs::canonicalize(path).map_err(|e| file::unreadable(e).with_parameter("path"))?;
        let (opened, text) = {
            let _span = tracing::info_span!("read").entered();
            file::opened_and_read(&resolved).map_err(|e| e.with_parameter("path"))?
        };
        // Read one past the limit: what's past it isn't held, so the session couldn't give
        // the file's text, or mend it, whole.
        if text.len() > MAX_INPUT_LEN {
            return Err(Problems::from(too_long(path)));
        }
        claim(registry, opened, &resolved, &resolved, id).map_err(|e| e.opening(path))?;
        (resolved, text, None)
    };
    let gated = Gated::of(&text);
    let opened = Opened {
        path: given.to_owned(),
        created: create,
        features: gated.document().ok().map(|d| d.features.len()),
        bodies: gated
            .document()
            .map(|d| super::bodies(d).into_iter().cloned().collect())
            .unwrap_or_default(),
        problems: gated.problems(),
    };
    let text = Arc::new(text);
    let held = Held {
        path: given.to_owned(),
        resolved,
        bytes: Arc::clone(&text),
        text,
        gated: Arc::new(gated),
        revision: Revision::OPENED,
        carried: Arc::new(BTreeSet::new()),
        transaction: None,
        part_written: None,
        last_reload: None,
    };
    let cache = registry.cache(&file::cache_directory(&held.resolved));
    Ok((held, opened, cache, made))
}

/// A new, empty document made at `path`, claimed for session `id`: where it is through
/// links, its text, and the file made, removed again unless kept.
fn made<P: Clients + 'static>(
    registry: &Registry<P>,
    id: SessionId,
    path: &Path,
) -> Result<(PathBuf, Vec<u8>, MadeFile), Problems> {
    let _span = tracing::info_span!("create").entered();
    let name = path.file_name().ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("{} names no file to make", shown(path)),
        )
        .with_parameter("path")
        .with_suggestion("give the path of the document to make, ending in its file's name")
    })?;
    let uncreatable = |e: std::io::Error| {
        Error::new(ErrorCode::IoFailed, format!("couldn't be made: {e}"))
            .with_parameter("path")
            .with_suggestion("check that its directory exists, and that this user may write in it")
    };
    let resolved = std::fs::canonicalize(file::directory(path))
        .map_err(uncreatable)?
        .join(name);
    let text = document::write(&Document {
        silico: FormatVersion,
        units: Units,
        naming: NamingScheme,
        features: Vec::new(),
    })?
    .into_bytes();
    let made = match create_new(&resolved, &text) {
        Ok(made) => made,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Problems::from(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!("{} already exists", shown(path)),
                )
                .with_parameter("create")
                .with_suggestion("open it without create, or give another path"),
            ));
        }
        Err(e) => return Err(Problems::from(uncreatable(e))),
    };
    // Removed again if it can't be claimed (a session holds what was at its path, gone
    // from disk since), or the open ends in no session.
    let unmade = MadeFile {
        path: resolved.clone(),
        text: text.clone(),
        kept: false,
    };
    claim(registry, made, &resolved, &resolved, id).map_err(|e| e.opening(path))?;
    Ok((resolved, text, unmade))
}

/// `text` written to a new file at `path`, never over one (nor through a link) already
/// there, and the file; removed again if it couldn't be written whole.
fn create_new(path: &Path, text: &[u8]) -> std::io::Result<std::fs::File> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    match file.write_all(text).and_then(|()| file.sync_all()) {
        Ok(()) => Ok(file),
        Err(e) => {
            let _ = std::fs::remove_file(path);
            Err(e)
        }
    }
}

/// The file `opened`, at `resolved` (the session's path through links, as `open` found it),
/// claimed for session `id` by both: the file it is and its path, the file kept open while
/// it's claimed, and any file the session claimed before let go (a reload's, decision 2, of
/// a file put at its path). `found_at` is where `resolved` leads now, through links put
/// there since (`resolved` itself, at `open`). Refused, naming the session, nothing
/// changed, if another session holds or is opening the file, `resolved`, or `found_at`: the
/// same file by another name (a hard link, macOS's data-volume name, a link put at the
/// path), another file put at its path since, or a link to another session's path whose
/// file was replaced there, by a save that session hasn't read yet.
fn claim<P: Clients + 'static>(
    registry: &Registry<P>,
    opened: std::fs::File,
    resolved: &Path,
    found_at: &Path,
    id: SessionId,
) -> Result<(), Unclaimed> {
    let file = FileId::of(&opened).map_err(Unclaimed::Unreadable)?;
    let mut book = lock(&registry.book);
    let holder = book
        .files
        .get(&file)
        .map(|(holder, _)| *holder)
        .into_iter()
        .chain(book.paths.get(resolved).copied())
        .chain(book.paths.get(found_at).copied())
        .find(|holder| *holder != id);
    match holder {
        Some(holder) => Err(Unclaimed::Held(holder)),
        None => {
            book.files.retain(|_, (holder, _)| *holder != id);
            book.files.insert(file, (id, opened));
            book.paths.insert(resolved.to_owned(), id);
            Ok(())
        }
    }
}

/// Why a file couldn't be claimed for a session ([`claim`]).
enum Unclaimed {
    /// Another session holds it, or is opening it.
    Held(SessionId),
    /// What file it is couldn't be read.
    Unreadable(Error),
}

impl Unclaimed {
    /// The refusal of `open` of the file at `given`, as `open` was given it.
    fn opening(self, given: &Path) -> Problems {
        Problems::from(match self {
            Unclaimed::Held(holder) => Error::new(
                ErrorCode::InvalidInput,
                format!("{} is already open in session {holder}", shown(given)),
            )
            .with_parameter("path")
            .with_suggestion(format!(
                "use session {holder}, which this client opened, or close it first"
            )),
            Unclaimed::Unreadable(error) => error.with_parameter("path"),
        })
    }

    /// The refusal of a command whose session found the file at `path` (as `open` was given
    /// it) not its own (decision 2): another session's, or one it couldn't tell apart.
    fn rereading(self, path: &str) -> Error {
        let shown_path = shown(Path::new(path));
        match self {
            Unclaimed::Held(holder) => not_reloaded(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!("{shown_path} is already open in session {holder}"),
                ),
                format!(
                    "put this session's file back at {shown_path}: the file there now is session {holder}'s"
                ),
            ),
            Unclaimed::Unreadable(error) => not_reloaded(of_file(path, &error), put_back(path)),
        }
    }
}

/// Session `id`'s claim on its file moved to the file at `resolved` now, which the session's
/// own save just put there (a new file renamed over the old one, or the old one written in
/// place): the file kept open while it's claimed, so its inode isn't given to another, and
/// the one before let go. A file the session didn't write, a reload's, is claimed through
/// [`claim`] instead, which refuses one another session holds. If it can't be opened
/// (removed since the save), the claim is left as it was, said in the log: its path still
/// claims it.
fn reclaim<P: Clients + 'static>(registry: &Weak<Registry<P>>, id: SessionId, resolved: &Path) {
    let Some(registry) = registry.upgrade() else {
        return;
    };
    // Opened without waiting, as a read is: a pipe put at the path wouldn't block it.
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(resolved)
        .map_err(file::unreadable)
        .and_then(|opened| Ok((FileId::of(&opened)?, opened)));
    match opened {
        Ok((file, opened)) => {
            let mut book = lock(&registry.book);
            book.files.retain(|_, (holder, _)| *holder != id);
            book.files.insert(file, (id, opened));
        }
        Err(error) => tracing::warn!(
            session = %id,
            "the file a save wrote couldn't be claimed, so its claim is the file before: {error}"
        ),
    }
}

impl<P: Clients + 'static> Registry<P> {
    /// A new session's engine: on its own client of the kernel, keeping its steps in
    /// `cache`, and solving sketches with the registry's starter, if it has one.
    fn engine(&self, cache: Cache) -> Recompute<P::Client> {
        let engine = Recompute::with_cache(self.kernel.client(), cache);
        match self.sketches.clone() {
            Some(start) => engine.with_sketch_starter(move || start()),
            None => engine,
        }
    }

    /// The cache in `dir`: the one value its sessions share, opened the first time; one
    /// without the user's secret, which keeps nothing, isn't kept, so the next open tries
    /// again.
    fn cache(&self, dir: &Path) -> Cache {
        if let Some(cache) = lock(&self.book).caches.get(dir) {
            return cache.clone();
        }
        // Opened without the book held (it reads the user's secret): should another session
        // have opened it meanwhile, its value is the one kept.
        let opened = (self.open_cache)(dir);
        if !opened.sealed() {
            return opened;
        }
        lock(&self.book)
            .caches
            .entry(dir.to_owned())
            .or_insert(opened)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Measure;
    use crate::fake::{FakeClient, FakeKernel};
    use crate::kernel::{Op, Primitive};
    use crate::recompute::FeatureOutcome;

    const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
    const SPHERE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "a91cw0de", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}]}"#;
    const WAIT: Duration = Duration::from_secs(60);

    fn checked(text: &str) -> Arc<Checked> {
        Arc::new(Checked::new(document::read(text.as_bytes()).unwrap()).unwrap())
    }

    /// A session as `start` makes one, at revision 1 of [`BOX`], on its own client of
    /// `kernel`; its queue open, though nothing serves it. Edits, which move a session to a
    /// new revision, come later: these tests drive `Session::model` at revisions 1 and 2.
    fn session(kernel: &FakeKernel) -> (Arc<Session<FakeClient>>, Receiver<Job<FakeClient>>) {
        let (sender, jobs) = mpsc::channel();
        let session = Arc::new(Session {
            id: SessionId(1),
            held: Mutex::new(Held {
                path: "a.silico".to_owned(),
                resolved: PathBuf::from("a.silico"),
                bytes: Arc::new(BOX.as_bytes().to_vec()),
                text: Arc::new(BOX.as_bytes().to_vec()),
                gated: Arc::new(Gated::of(BOX.as_bytes())),
                revision: Revision::OPENED,
                carried: Arc::new(BTreeSet::new()),
                transaction: None,
                part_written: None,
                last_reload: None,
            }),
            queue: Mutex::new(Some(sender)),
            rebuilds: Mutex::new(Rebuilds {
                engine: Recompute::new(kernel.client()),
                running: None,
                published: None,
                failed: None,
                kept: None,
                floor: Revision::OPENED,
                candidate: None,
                watchers: Threads::new(),
            }),
            rebuilt: Condvar::new(),
            reads: Mutex::new(Reads::new()),
        });
        (session, jobs)
    }

    /// The ID of the one feature `model` was built from.
    fn built_from(model: &Model) -> String {
        model.document().features[0].id.to_string()
    }

    #[test]
    fn a_revisions_model_is_its_own_and_a_late_one_of_an_older_never_replaces_it() {
        let kernel = FakeKernel::new(2).unwrap();
        let (session, _jobs) = session(&kernel);
        assert_eq!(
            built_from(&session.model(Revision::OPENED, &checked(BOX)).unwrap()),
            "k7f2qx3m"
        );
        // Revision 2's read is never revision 1's model, published as it is.
        let second = session
            .model(Revision::OPENED.next().unwrap(), &checked(SPHERE))
            .unwrap();
        assert_eq!(built_from(&second), "a91cw0de");
        // A model of revision 1 arriving late (another engine's, here) leaves 2's in place.
        let other = Recompute::new(kernel.client());
        watch(
            &Arc::downgrade(&session),
            &Label::new(Revision::OPENED),
            other.start(checked(BOX)),
        );
        let published = lock(&session.rebuilds).published.clone().unwrap();
        assert_eq!(published.revision, Revision::OPENED.next().unwrap());
        assert!(Arc::ptr_eq(&published.model, &second));
        // Nor is revision 1's rebuild started again now that 2's has published.
        let mut rebuilds = lock(&session.rebuilds);
        assert!(
            !session
                .start_unless_newer(&mut rebuilds, Revision::OPENED, &checked(BOX))
                .unwrap()
        );
        assert_eq!(rebuilds.running(), None);
    }

    #[test]
    fn a_revision_has_a_next_until_there_are_none_left() {
        assert_eq!(Revision::OPENED.next(), Some(Revision(2)));
        assert_eq!(Revision(u64::MAX - 1).next(), Some(Revision(u64::MAX)));
        assert_eq!(Revision(u64::MAX).next(), None);
    }

    /// Sessions on a fake kernel with [`BOX`] open as s1, checked so revision 1's model is
    /// published, then moved to revision 2 with `text` as its document, as an edit would: so a
    /// read can be told of which revision it is.
    fn at_two(
        text: &str,
    ) -> (
        tempfile::TempDir,
        Arc<FakeKernel>,
        Sessions<Arc<FakeKernel>>,
        SessionId,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        let path = dir.path().join("a.silico");
        std::fs::write(&path, BOX).unwrap();
        let kernel = Arc::new(FakeKernel::new(1).unwrap());
        let sessions = Sessions::with_cache_opener(Arc::clone(&kernel), move |cache| {
            Cache::with_secret(cache, &secret)
        });
        let id = sessions.open(&path, false).wait().session.unwrap().id;
        assert!(sessions.check(&id).wait().result.unwrap().is_empty());
        let session = sessions.find(&id).unwrap();
        let mut held = lock(&session.held);
        held.revision = Revision::OPENED.next().unwrap();
        held.text = Arc::new(text.as_bytes().to_vec());
        held.gated = Arc::new(Gated::of(text.as_bytes()));
        drop(held);
        (dir, kernel, sessions, id)
    }

    /// A read that gives the volume of the one feature its model was built from, and what it
    /// read.
    fn volume(read: Read<'_>) -> Result<(Measure, ModelRead), Problems> {
        let feature = &read.model().document().features[0].id;
        let fingerprint = read.model().fingerprint(feature).unwrap();
        Ok((fingerprint.volume, read.model_read()))
    }

    #[test]
    fn a_published_read_says_the_older_revision_it_read() {
        let (_dir, kernel, sessions, id) = at_two(SPHERE);
        // Revision 2's sphere held, so its rebuild is seen running.
        let _hold = kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
        let response = sessions
            .read_model(&id, "probe", ModelChoice::Published, volume)
            .wait();
        let read = ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Published,
        };
        // The box, 40 × 20 × 10: 8000 mm³, of revision 1, while the session is at 2.
        assert_eq!(
            response.result.unwrap(),
            (Measure::from_millionths(8_000_000_000), read)
        );
        assert_eq!(response.model, Some(read));
        let session = response.session.unwrap();
        assert_eq!(session.revision, Revision::OPENED.next().unwrap());
        // And it started revision 2's rebuild.
        let found = sessions.find(&id).unwrap();
        assert_eq!(
            lock(&found.rebuilds).running(),
            Some(Revision::OPENED.next().unwrap())
        );
    }

    #[test]
    fn a_current_read_says_the_newer_revision_it_waited_for() {
        let (_dir, _kernel, sessions, id) = at_two(SPHERE);
        let response = sessions
            .read_model(&id, "probe", ModelChoice::Current, volume)
            .wait();
        let read = ModelRead {
            revision: Revision::OPENED.next().unwrap(),
            of: ModelChoice::Current,
        };
        // The sphere of radius 5: 4/3 π 5³ = 523.598776 mm³, of revision 2.
        assert_eq!(
            response.result.unwrap(),
            (Measure::from_millionths(523_598_776), read)
        );
        assert_eq!(response.model, Some(read));
        assert_eq!(
            response.session.unwrap().revision,
            Revision::OPENED.next().unwrap()
        );
    }

    #[test]
    fn a_published_read_of_a_refused_revision_gives_the_older_model_and_check_its_problems() {
        let flat = BOX.replace("\"length\": 40", "\"length\": 0");
        let (_dir, kernel, sessions, id) = at_two(&flat);
        let calls = kernel.calls().len();
        let read = ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Published,
        };
        // The newest published model, revision 1's, its revision saying it's older.
        let published = sessions.published(&id);
        assert_eq!(published.model, Some(read));
        assert_eq!(published.result.unwrap().revision, Revision::OPENED);
        let response = sessions
            .read_model(&id, "probe", ModelChoice::Published, volume)
            .wait();
        assert_eq!(
            response.result.unwrap(),
            (Measure::from_millionths(8_000_000_000), read)
        );
        // The gate's problems are check's; nothing is rebuilt.
        let problems = sessions.check(&id).wait().result.unwrap();
        assert_eq!(problems.errors()[0].parameter(), Some("length"));
        assert_eq!(kernel.calls().len(), calls);
    }

    #[test]
    fn a_published_read_racing_a_close_is_refused_as_not_open() {
        // Closed (its queue ended) but not yet gone from the registry, as a close is for a
        // moment: the read refuses it, though a model is published.
        let (_dir, _kernel, sessions, id) = at_two(SPHERE);
        sessions.find(&id).unwrap().close();
        let response = sessions.published(&id);
        // Of no session, as every refusal of a session not open is.
        assert_eq!(response.session, None);
        assert_eq!(response.model, None);
        assert_eq!(response.result.unwrap_err().errors()[0], not_open(id));
    }

    #[test]
    fn a_rebuild_is_refused_only_under_a_newer_revisions() {
        let kernel = FakeKernel::new(1).unwrap();
        let (session, _jobs) = session(&kernel);
        let _hold = kernel.hold(|_| true);
        let mut rebuilds = lock(&session.rebuilds);
        // Revision 2's rebuild running (as recorded): revision 1's isn't started under it.
        rebuilds.running = Some(Label::new(Revision::OPENED.next().unwrap()));
        assert!(
            !session
                .start_unless_newer(&mut rebuilds, Revision::OPENED, &checked(BOX))
                .unwrap()
        );
        assert_eq!(rebuilds.running(), Some(Revision::OPENED.next().unwrap()));
        // Its own revision's running: started (as a check after a fault starts it again).
        rebuilds.running = Some(Label::new(Revision::OPENED));
        assert!(
            session
                .start_unless_newer(&mut rebuilds, Revision::OPENED, &checked(BOX))
                .unwrap()
        );
        assert_eq!(rebuilds.running(), Some(Revision::OPENED));
        drop(rebuilds);
        assert!(kernel.wait_running(1, WAIT));
    }

    #[test]
    fn a_published_read_starts_its_revisions_rebuild_only_while_an_older_is_published() {
        let kernel = FakeKernel::new(1).unwrap();
        let (session, _jobs) = session(&kernel);
        let first = session.model(Revision::OPENED, &checked(BOX)).unwrap();
        // At revision 1, with revision 1 published: that model, and nothing started.
        let read = session
            .published_read(Revision::OPENED, &checked(BOX))
            .unwrap();
        assert_eq!(read.revision, Revision::OPENED);
        assert!(Arc::ptr_eq(&read.model, &first));
        assert_eq!(lock(&session.rebuilds).running(), None);
        // At revision 2, with only revision 1 published: revision 1's model, at once, and
        // revision 2's rebuild started (held, so it's seen running).
        let _hold = kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
        let read = session
            .published_read(Revision::OPENED.next().unwrap(), &checked(SPHERE))
            .unwrap();
        assert_eq!(read.revision, Revision::OPENED);
        assert!(Arc::ptr_eq(&read.model, &first));
        assert_eq!(
            lock(&session.rebuilds).running(),
            Some(Revision::OPENED.next().unwrap())
        );
    }

    #[test]
    fn a_rebuild_stopped_under_an_open_session_is_started_again() {
        let kernel = FakeKernel::new(1).unwrap();
        let (session, _jobs) = session(&kernel);
        let hold = kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
        let boxes = || {
            kernel
                .calls()
                .iter()
                .filter(|c| matches!(c.op, Op::MakePrimitive(Primitive::Box { .. })))
                .count()
        };
        std::thread::scope(|scope| {
            let read = scope.spawn(|| session.model(Revision::OPENED, &checked(BOX)));
            assert!(kernel.wait_running(1, WAIT));
            lock(&session.rebuilds).engine.cancel();
            // The box asked for again, by the read, still waiting.
            let deadline = std::time::Instant::now() + WAIT;
            while boxes() < 2 {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the rebuild wasn't started again"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            hold.release();
            let model = read.join().unwrap().unwrap();
            assert_eq!(built_from(&model), "k7f2qx3m");
        });
        assert_eq!(boxes(), 2);
    }

    #[test]
    fn a_published_read_never_gives_a_model_newer_than_the_sessions_revision() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        let path = dir.path().join("a.silico");
        std::fs::write(&path, BOX).unwrap();
        let kernel = Arc::new(FakeKernel::new(1).unwrap());
        let sessions = Sessions::with_cache_opener(Arc::clone(&kernel), move |cache| {
            Cache::with_secret(cache, &secret)
        });
        let id = sessions.open(&path, false).wait().session.unwrap().id;
        // A model of revision 2, which this session (at 1) hasn't reached.
        let newer = match Recompute::new(FakeKernel::new(1).unwrap())
            .start(checked(SPHERE))
            .wait()
            .unwrap()
        {
            RunOutcome::Published(model) => model,
            RunOutcome::Superseded => panic!("superseded"),
        };
        let session = sessions.find(&id).unwrap();
        lock(&session.rebuilds).published = Some(Published {
            revision: Revision::OPENED.next().unwrap(),
            model: newer,
        });
        let read = sessions.published(&id);
        assert_eq!(read.result.unwrap_err().errors()[0], nothing_published(id));
        // Nor does it start revision 1's rebuild, whose model would be older than one there.
        assert_eq!(lock(&session.rebuilds).running(), None);
    }

    #[test]
    fn a_document_made_for_a_session_the_sessions_closing_refuse_is_removed() {
        // The sessions began closing while the open made its file: it's refused, and what it
        // made is gone; a file it didn't make is left.
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        let sessions = Sessions::with_cache_opener(FakeKernel::new(1).unwrap(), move |cache| {
            Cache::with_secret(cache, &secret)
        });
        lock(&sessions.registry.book).stopping = true;
        let (reply, answer) = mpsc::sync_channel(1);
        let made = dir.path().join("new.silico");
        let started = start(
            Arc::clone(&sessions.registry),
            SessionId(1),
            made.to_str().unwrap(),
            true,
            &reply,
        );
        assert!(started.is_none());
        let response = answer.recv().unwrap();
        assert_eq!(response.result.unwrap_err().errors()[0], closing());
        assert!(!made.exists());
        let kept = dir.path().join("kept.silico");
        std::fs::write(&kept, "{}").unwrap();
        assert!(
            start(
                Arc::clone(&sessions.registry),
                SessionId(2),
                kept.to_str().unwrap(),
                false,
                &reply
            )
            .is_none()
        );
        assert!(kept.exists());
        let book = lock(&sessions.registry.book);
        assert!(book.files.is_empty());
        assert!(book.paths.is_empty());
    }

    impl Held {
        /// What the session's edits reach now ([`Reaching::reach`]).
        fn reach(&self) -> BTreeSet<FeatureId> {
            self.reaching().reach()
        }
    }

    impl<C: Kernel + 'static> Session<C> {
        /// The document replaced by `text` as `replace_text` replaces it inside a
        /// transaction, whether or not one is open: the production edit's candidate
        /// (`edit::replace_text`) held as an edit inside a transaction holds it
        /// (`Session::hold`), at a new revision unless it's the same document, starting no
        /// rebuild and saving nothing (what it reaches is the session's to work out,
        /// `Reaching::reach`); refused whole, the session as it was, as the edit is.
        fn applied(&self, text: &str) -> Result<Revision, Problems> {
            let (_, gated) = self.document();
            match edit::replace_text(&gated, text.as_bytes())?.1 {
                Some(candidate) => self.hold(candidate),
                None => Ok(lock(&self.held).revision),
            }
        }
    }

    /// Sessions on a fake kernel of two threads, with a document in `a.silico` open as s1.
    struct Fixture {
        _dir: tempfile::TempDir,
        kernel: Arc<FakeKernel>,
        sessions: Sessions<Arc<FakeKernel>>,
        id: SessionId,
        path: PathBuf,
    }

    fn opened(text: &str) -> Fixture {
        opened_solving(text, &crate::fake::FakeSketches::new())
    }

    /// [`opened`], its sketches solved by `sketches`.
    fn opened_solving(text: &str, sketches: &crate::fake::FakeSketches) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        let path = dir.path().join("a.silico");
        std::fs::write(&path, text).unwrap();
        let kernel = Arc::new(FakeKernel::new(2).unwrap());
        let solver = sketches.clone();
        let sessions = Sessions::with_stand_ins(
            Arc::clone(&kernel),
            move |cache| Cache::with_secret(cache, &secret),
            move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
        );
        let id = sessions.open(&path, false).wait().session.unwrap().id;
        Fixture {
            _dir: dir,
            kernel,
            sessions,
            id,
            path,
        }
    }

    impl Fixture {
        fn session(&self) -> Arc<Session<FakeClient>> {
            self.sessions.find(&self.id).unwrap()
        }

        fn edit(&self, text: &str) -> Revision {
            self.session().applied(text).unwrap()
        }

        fn file(&self) -> String {
            std::fs::read_to_string(&self.path).unwrap()
        }

        fn begin(&self) -> Begun {
            self.sessions.begin(&self.id).wait().result.unwrap()
        }

        /// How many requests of the kind `pick` picks the kernel has started.
        fn made(&self, pick: impl Fn(&Op) -> bool) -> usize {
            self.kernel.calls().iter().filter(|c| pick(&c.op)).count()
        }
    }

    fn id(text: &str) -> FeatureId {
        FeatureId::new(text).unwrap()
    }

    fn is_box(op: &Op) -> bool {
        matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
    }

    fn is_cylinder(op: &Op) -> bool {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
    }

    /// A block, a lug clear of it, and their fuse, each with the fingerprint the fake
    /// measures: the boxes' by formula (40 × 20 × 10 = 8000, area 2800; 10 × 10 × 10 =
    /// 1000, area 600), and the fuse's both together, as the fake fuses two solids as if
    /// they didn't touch (and these don't): 9000, area 3400. But the block's volume is
    /// stored 1e-6 mm³ over what it measures: drift, which no edit below reaches.
    const STORED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [
  {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "fingerprint": {"volume": 8000.000001, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}},
  {"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10, "fingerprint": {"volume": 1000, "area": 600, "bbox": [100, 0, 0, 110, 10, 10], "faces": 6, "edges": 12, "vertices": 8}},
  {"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "k7f2qx3m", "tool": "b0000001", "fingerprint": {"volume": 9000, "area": 3400, "bbox": [0, 0, 0, 110, 20, 10], "faces": 12, "edges": 24, "vertices": 16}}
]}
"#;

    /// [`STORED`] with the lug 20 long: an edit that reaches the lug and the fuse.
    fn longer() -> String {
        let longer = STORED.replace(
            r#""length": 10, "width": 10"#,
            r#""length": 20, "width": 10"#,
        );
        assert_ne!(longer, STORED);
        longer
    }

    /// [`longer`] committed: the block's fingerprint as stored, drift and all; the lug's
    /// and the fuse's new, by hand: the lug 20 × 10 × 10 = 2000, area 2(200 + 200 + 100) =
    /// 1000, to x = 120; the fuse 8000 + 2000 = 10000, area 2800 + 1000 = 3800.
    const COMMITTED: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 8000.000001, \"area\": 2800, \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    },\n    {\n      \"id\": \"b0000001\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Lug\",\n      \"position\": [100, 0, 0],\n      \"length\": 20,\n      \"width\": 10,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 2000, \"area\": 1000, \"bbox\": [100, 0, 0, 120, 10, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    },\n    {\n      \"id\": \"f0000001\",\n      \"type\": \"fuse\",\n      \"v\": 1,\n      \"name\": \"Both\",\n      \"base\": \"k7f2qx3m\",\n      \"tool\": \"b0000001\",\n      \"fingerprint\": {\"volume\": 10000, \"area\": 3800, \"bbox\": [0, 0, 0, 120, 20, 10], \"faces\": 12, \"edges\": 24, \"vertices\": 16}\n    }\n  ]\n}\n";

    /// The one drift `problems` hold: the block's volume, stored 1e-6 over.
    #[track_caller]
    fn the_blocks_drift(problems: &Problems) {
        assert_eq!(problems.total(), 1, "{problems:?}");
        let drift = &problems.errors()[0];
        assert_eq!(
            (drift.code(), drift.feature(), drift.parameter()),
            (
                ErrorCode::Drift,
                Some("k7f2qx3m"),
                Some("fingerprint.volume")
            ),
            "{drift:?}"
        );
        // Stored → rebuilt, as design section 7's commit example writes it.
        assert_eq!(
            drift.message(),
            "feature k7f2qx3m rebuilt to other measures than its fingerprint: volume 8000.000001 → 8000"
        );
        // The design's words (section 7's commit example), which name format (decision 31).
        assert_eq!(
            drift.suggestion(),
            Some(
                "if this feature or one it uses was edited by hand or in a merge since it was saved, run format (or silico fmt) to accept its new fingerprint; if not, this is a determinism bug or a change in the kernel: find which before accepting it"
            )
        );
    }

    #[test]
    fn a_commit_writes_new_fingerprints_only_for_what_edits_reached_and_keeps_drift_as_stored() {
        let f = opened(STORED);
        assert_eq!(
            f.begin(),
            Begun {
                base: Revision::OPENED
            }
        );
        assert_eq!(f.edit(&longer()), Revision(2));
        let response = f.sessions.commit(&f.id).wait();
        let session = response.session.unwrap();
        assert_eq!(
            (session.revision, session.transaction),
            (Revision(3), false)
        );
        assert_eq!(
            response.model,
            Some(ModelRead {
                revision: Revision(2),
                of: ModelChoice::Current
            })
        );
        let committed = response.result.unwrap();
        assert!(committed.written);
        assert_eq!(committed.path, f.path.to_str().unwrap());
        assert_eq!(committed.refingerprinted, [id("b0000001"), id("f0000001")]);
        assert!(committed.resolved.is_empty());
        the_blocks_drift(&committed.problems);
        assert_eq!(f.file(), COMMITTED);
        // The session holds what it wrote, as text and as the file's bytes, and nothing is
        // reached since the save.
        let session = f.session();
        let held = lock(&session.held);
        assert_eq!(held.text.as_slice(), COMMITTED.as_bytes());
        assert_eq!(held.bytes.as_slice(), COMMITTED.as_bytes());
        assert!(held.reach().is_empty());
        drop(held);
        // So a check of the saved revision finds the block's drift alone.
        the_blocks_drift(&f.sessions.check(&f.id).wait().result.unwrap());
    }

    #[test]
    fn a_check_says_no_drift_of_what_edits_reached() {
        // The lug's stored 1000 and the fuse's 9000 were measured from the old length: the
        // edit reached them, so only the block's drift is said.
        let f = opened(STORED);
        f.begin();
        f.edit(&longer());
        let response = f.sessions.check(&f.id).wait();
        the_blocks_drift(&response.result.unwrap());
        assert_eq!(response.model.unwrap().revision, Revision(2));
        assert_eq!(
            lock(&f.session().held).reach(),
            BTreeSet::from([id("b0000001"), id("f0000001")])
        );
    }

    #[test]
    fn a_notes_edit_reaches_nothing_and_its_commit_writes_no_fingerprint() {
        // The lug has no fingerprint stored: a commit that reached it would write one.
        let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}, {"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10}]}"#;
        let noted = text.replace(
            r#""name": "Block", "#,
            r#""name": "Block", "note": "Mount face", "#,
        );
        let f = opened(text);
        f.begin();
        f.edit(&noted);
        let committed = f.sessions.commit(&f.id).wait().result.unwrap();
        assert!(committed.written);
        assert!(committed.refingerprinted.is_empty());
        assert!(committed.problems.is_empty(), "{:?}", committed.problems);
        assert_eq!(
            f.file(),
            "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 10,\n      \"note\": \"Mount face\",\n      \"fingerprint\": {\"volume\": 8000, \"area\": 2800, \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    },\n    {\"id\": \"b0000001\", \"type\": \"box\", \"v\": 1, \"name\": \"Lug\", \"position\": [100, 0, 0], \"length\": 10, \"width\": 10, \"height\": 10}\n  ]\n}\n"
        );
    }

    #[test]
    fn a_commit_of_edits_undone_writes_nothing_and_reaches_what_begin_saw() {
        let f = opened(STORED);
        f.begin();
        f.edit(&longer());
        // Back as it was, though in canonical form now: the same document.
        f.edit(&document::write(&document::read(STORED.as_bytes()).unwrap()).unwrap());
        let response = f.sessions.commit(&f.id).wait();
        let committed = response.result.unwrap();
        assert!(!committed.written);
        assert!(committed.refingerprinted.is_empty());
        // The block's drift is said: the commit's check is begin's.
        the_blocks_drift(&committed.problems);
        assert_eq!(f.file(), STORED);
        // The session's text is the file's again (not the canonical form the second edit
        // left): a new revision, 4, after the two edits'.
        let session = response.session.unwrap();
        assert_eq!(
            (session.revision, session.transaction),
            (Revision(4), false)
        );
        // The commit's model, revision 3's, is revision 4's: the same document, so nothing
        // is rebuilt for it.
        assert_eq!(
            f.sessions.published(&f.id).result.unwrap().revision,
            Revision(4)
        );
        assert_eq!(lock(&f.session().rebuilds).running(), None);
        assert_eq!(f.sessions.text(&f.id, None).wait().result.unwrap(), STORED);
        assert!(lock(&f.session().held).reach().is_empty());
    }

    #[test]
    fn a_commit_over_a_file_changed_outside_is_a_conflict_writing_nothing_and_leaving_it_open() {
        let f = opened(STORED);
        f.begin();
        f.edit(&longer());
        std::fs::write(&f.path, "edited elsewhere").unwrap();
        let response = f.sessions.commit(&f.id).wait();
        let session = response.session.unwrap();
        // Inside the transaction, nothing reloaded.
        assert_eq!(
            (session.revision, session.transaction, session.reloaded),
            (Revision(2), true, false)
        );
        let problems = response.result.unwrap_err();
        assert_eq!(problems.total(), 1);
        let conflict = &problems.errors()[0];
        assert_eq!(conflict.code(), ErrorCode::Conflict);
        assert_eq!(conflict.parameter(), Some("path"));
        assert_eq!(
            conflict.message(),
            format!(
                "{} changed outside this session since it was read",
                shown(&f.path)
            )
        );
        assert_eq!(
            conflict.cause(),
            Some(
                "it changed after the transaction began, from revision 1, and a transaction doesn't reload its file"
            )
        );
        // The design's words (section 7's commit example).
        assert_eq!(
            conflict.suggestion(),
            Some("nothing was written: roll back, then begin again from the file as it is now")
        );
        // Found before the command, so the commit rebuilt nothing.
        assert!(f.kernel.calls().is_empty(), "{:?}", f.kernel.calls());
        assert_eq!(f.file(), "edited elsewhere");
        // Rolled back, as the advice says (inside the transaction still, so not reloaded),
        // then begun again: from the file as it is now, which begin reloads first. Revision
        // 1 at open, 2 the edit, 3 the rollback, 4 the reload.
        let rolled = f.sessions.rollback(&f.id).wait();
        assert_eq!(
            rolled.result.unwrap(),
            RolledBack {
                restored: Revision::OPENED
            }
        );
        assert!(!rolled.session.unwrap().reloaded);
        let begun = f.sessions.begin(&f.id).wait();
        assert_eq!(begun.result.unwrap(), Begun { base: Revision(4) });
        assert!(begun.session.unwrap().reloaded);
        assert_eq!(
            f.sessions.text(&f.id, None).wait().result.unwrap(),
            "edited elsewhere"
        );
    }

    #[test]
    fn a_reached_feature_that_failed_stays_reached_after_its_commit() {
        let f = opened(STORED);
        // The lug, 20 long, can't be made: it fails, and the fuse is blocked on it.
        f.kernel.refuse(|op| {
            matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 15.0 && *dx < 25.0)
                .then(|| Error::new(ErrorCode::KernelFailed, "the lug can't be made"))
        });
        f.begin();
        f.edit(&longer());
        let committed = f.sessions.commit(&f.id).wait().result.unwrap();
        assert!(committed.written);
        assert!(committed.refingerprinted.is_empty());
        let found: Vec<(ErrorCode, Option<&str>)> = committed
            .problems
            .errors()
            .iter()
            .map(|e| (e.code(), e.feature()))
            .collect();
        assert_eq!(
            found,
            [
                (ErrorCode::KernelFailed, Some("b0000001")),
                (ErrorCode::Drift, Some("k7f2qx3m"))
            ]
        );
        // Neither's fingerprint written, so both still reached, and their stale fingerprints
        // kept as stored.
        assert_eq!(
            *lock(&f.session().held).carried,
            BTreeSet::from([id("b0000001"), id("f0000001")])
        );
        let saved = document::read(f.file().as_bytes()).unwrap();
        let volumes: Vec<Measure> = saved
            .features
            .iter()
            .map(|feature| feature.fingerprint.unwrap().volume)
            .collect();
        assert_eq!(
            volumes,
            [
                Measure::from_millionths(8_000_000_001),
                Measure::from_millionths(1_000_000_000),
                Measure::from_millionths(9_000_000_000)
            ]
        );
    }

    #[test]
    fn a_rollback_restores_the_text_byte_for_byte_and_the_same_model_at_a_new_revision() {
        // BOX isn't in canonical form: it comes back as written.
        let f = opened(BOX);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        let model = f.sessions.published(&f.id).result.unwrap().model;
        f.begin();
        f.edit(SPHERE);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        let calls = f.kernel.calls().len();
        let response = f.sessions.rollback(&f.id).wait();
        assert_eq!(
            response.result.unwrap(),
            RolledBack {
                restored: Revision::OPENED
            }
        );
        let session = response.session.unwrap();
        assert_eq!(
            (session.revision, session.transaction),
            (Revision(3), false)
        );
        assert_eq!(f.sessions.text(&f.id, None).wait().result.unwrap(), BOX);
        // The model begin saw, the same one, as revision 3's.
        let published = f.sessions.published(&f.id).result.unwrap();
        assert_eq!(published.revision, Revision(3));
        assert!(Arc::ptr_eq(&published.model, &model));
        // A check reads it, and asks the kernel nothing.
        let check = f.sessions.check(&f.id).wait();
        assert!(check.result.unwrap().is_empty());
        assert_eq!(
            check.model,
            Some(ModelRead {
                revision: Revision(3),
                of: ModelChoice::Current
            })
        );
        assert_eq!(f.kernel.calls().len(), calls);
        assert_eq!(f.file(), BOX);
    }

    /// [`BOX`] and a cylinder clear of it, of radius 2 and height 5.
    const BOX_AND_PIN: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [100, 0, 0], "radius": 2, "height": 5}]}"#;

    #[test]
    fn a_rollback_whose_begin_came_mid_rebuild_and_whose_rebuild_was_superseded_starts_one() {
        let f = opened(BOX);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        f.edit(BOX_AND_PIN);
        // Revision 2's rebuild started by a published read, its pin held.
        let hold = f.kernel.hold(is_cylinder);
        assert_eq!(
            f.sessions.published(&f.id).result.unwrap().revision,
            Revision::OPENED
        );
        assert!(f.kernel.wait_running(1, WAIT));
        assert_eq!(f.begin().base, Revision(2));
        // A check in the transaction supersedes revision 2's rebuild: its model never comes.
        f.edit(SPHERE);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        let response = f.sessions.rollback(&f.id).wait();
        assert_eq!(response.result.unwrap().restored, Revision(2));
        // The rollback started the restored document's rebuild, as revision 4's.
        assert_eq!(lock(&f.session().rebuilds).running(), Some(Revision(4)));
        hold.release();
        let check = f.sessions.check(&f.id).wait();
        assert!(check.result.unwrap().is_empty());
        assert_eq!(check.model.unwrap().revision, Revision(4));
        // The block, built before, is never asked for again; the pin, stopped, is.
        assert_eq!(f.made(is_box), 1);
        assert_eq!(f.made(is_cylinder), 2);
    }

    #[test]
    fn a_rollback_restores_the_model_begins_rebuild_published_inside_the_transaction() {
        let f = opened(BOX);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        f.edit(BOX_AND_PIN);
        let hold = f.kernel.hold(is_cylinder);
        let _ = f.sessions.published(&f.id);
        assert!(f.kernel.wait_running(1, WAIT));
        f.begin();
        // Revision 2's rebuild publishes after begin, before anything supersedes it.
        hold.release();
        let deadline = std::time::Instant::now() + WAIT;
        let model = loop {
            let published = f.sessions.published(&f.id).result.unwrap();
            if published.revision == Revision(2) {
                break published.model;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "revision 2 never published"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        f.edit(SPHERE);
        let calls = f.kernel.calls().len();
        assert_eq!(
            f.sessions.rollback(&f.id).wait().result.unwrap().restored,
            Revision(2)
        );
        // That model, as revision 4's, with no rebuild started.
        let published = f.sessions.published(&f.id).result.unwrap();
        assert_eq!(published.revision, Revision(4));
        assert!(Arc::ptr_eq(&published.model, &model));
        assert_eq!(lock(&f.session().rebuilds).running(), None);
        assert_eq!(f.kernel.calls().len(), calls);
    }

    #[test]
    fn dropping_the_sessions_mid_transaction_leaves_the_file_as_it_was() {
        let f = opened(STORED);
        f.begin();
        f.edit(&longer());
        assert!(f.sessions.check(&f.id).wait().result.is_ok());
        let Fixture {
            _dir,
            sessions,
            path,
            ..
        } = f;
        drop(sessions);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), STORED);
    }

    /// Two sketches, each a line from `pa` at (0, 0) to `pb` at (10, 0) on xy, held
    /// horizontal; the second's `pb` at `x` instead.
    fn two_sketches(x: &str) -> String {
        let sketch = |id: &str, name: &str, x: &str| {
            format!(
                r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "{name}", "plane": "xy", "points": [{{"id": "pa000000", "x": 0, "y": 0, "construction": false}}, {{"id": "pb000000", "x": {x}, "y": 0, "construction": false}}], "lines": [{{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}}], "circles": [], "arcs": [], "constraints": [{{"id": "k0000001", "type": "horizontal", "line": "na000000"}}]}}"#
            )
        };
        format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}, {}]}}"#,
            sketch("s0000001", "First", "10"),
            sketch("s0000002", "Second", x)
        )
    }

    /// Where `pb`, the second point, of sketch `sketch` in `text` is along x.
    fn pb_x(text: &str, sketch: &str) -> f64 {
        let document = document::read(text.as_bytes()).unwrap();
        match &document.feature(&id(sketch)).unwrap().operation {
            document::Operation::Sketch(found) => found.points[1].x.mm(),
            _ => panic!("{sketch} isn't a sketch"),
        }
    }

    #[test]
    fn a_commit_writes_the_solves_only_of_the_sketches_edits_reached() {
        // A solve from `pb` at 10 or 12 puts it 1 to the right, and from anywhere else
        // leaves it, so a solve loaded again stays (the values sent are the origin and the
        // axes' ends, 0 to 5, then `pa`'s x and y, 6 and 7, then `pb`'s, 8 and 9): both
        // sketches solve to other values than they store.
        let sketches = crate::fake::FakeSketches::new();
        sketches.answer(|system| {
            let mut solved = crate::fake::FakeSketches::as_sent(system);
            if solved.values[8] == 10.0 || solved.values[8] == 12.0 {
                solved.values[8] += 1.0;
            }
            Ok(solved)
        });
        let f = opened_solving(&two_sketches("10"), &sketches);
        f.begin();
        // The second's `pb` moved to 12: the edit reaches it alone.
        f.edit(&two_sketches("12"));
        let committed = f.sessions.commit(&f.id).wait().result.unwrap();
        assert!(committed.written);
        assert_eq!(committed.resolved, [id("s0000002")]);
        assert!(committed.refingerprinted.is_empty());
        // The first's drift kept as stored, and said; the second's solve, 12 + 1, written.
        assert_eq!(committed.problems.total(), 1, "{:?}", committed.problems);
        let drift = &committed.problems.errors()[0];
        assert_eq!(
            (drift.code(), drift.feature(), drift.elements()),
            (
                ErrorCode::Drift,
                Some("s0000001"),
                &["pb000000".to_owned()][..]
            )
        );
        // Decision 31's words for a sketch: format, beside silico fmt, accepts its solve.
        assert_eq!(
            drift.suggestion(),
            Some(
                "if this sketch was edited by hand or in a merge since it was saved, run format (or silico fmt) to accept its new solve; if not, this is a determinism bug or a change in the sketch solver: find which before accepting it"
            )
        );
        // Its solve written, the second sketch isn't reached after the commit.
        assert!(lock(&f.session().held).reach().is_empty());
        let saved = f.file();
        assert_eq!(pb_x(&saved, "s0000001"), 10.0);
        assert_eq!(pb_x(&saved, "s0000002"), 13.0);
    }

    #[test]
    fn a_published_read_answered_but_its_thread_still_ending_leaves_room_for_another() {
        // Eight reads whose threads haven't ended, each having answered (counted no more):
        // a ninth runs, since what's bounded is reads unanswered, not threads.
        let (_dir, _kernel, sessions, id) = at_two(BOX);
        let (release, released) = mpsc::channel::<()>();
        let released = Arc::new(Mutex::new(released));
        {
            let found = sessions.find(&id).unwrap();
            let mut reads = lock(&found.reads);
            let threads = reads.threads.as_mut().unwrap();
            for _ in 0..MAX_PUBLISHED_READS {
                let released = Arc::clone(&released);
                threads.push(std::thread::spawn(move || {
                    let _ = lock(&released).recv();
                }));
            }
        }
        let ninth = sessions
            .read_model(&id, "probe", ModelChoice::Published, |read| {
                Ok(read.model_read().revision)
            })
            .wait();
        assert_eq!(ninth.result.unwrap(), Revision::OPENED);
        drop(release);
    }

    #[test]
    fn a_rollback_restores_the_checked_form() {
        let f = opened(BOX);
        f.begin();
        f.edit(BOX_AND_PIN);
        f.sessions.rollback(&f.id).wait().result.unwrap();
        assert_eq!(f.sessions.status(&f.id).result.unwrap().features, Some(1));
        assert!(
            f.sessions
                .text(&f.id, Some(id("c0000001")))
                .wait()
                .result
                .is_err()
        );
    }

    #[test]
    fn a_rollback_restores_what_edits_reached_so_drift_is_said_again() {
        let f = opened(STORED);
        f.begin();
        // Reaches the block and the fuse.
        f.edit(&STORED.replace(r#""length": 40"#, r#""length": 41"#));
        assert_eq!(
            lock(&f.session().held).reach(),
            BTreeSet::from([id("k7f2qx3m"), id("f0000001")])
        );
        f.sessions.rollback(&f.id).wait().result.unwrap();
        assert!(lock(&f.session().held).reach().is_empty());
        the_blocks_drift(&f.sessions.check(&f.id).wait().result.unwrap());
    }

    /// The lug, 20 long, can't be made, as `f`'s kernel is told.
    fn refuse_the_longer_lug(f: &Fixture, code: ErrorCode) {
        f.kernel.refuse(move |op| {
            matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 15.0 && *dx < 25.0)
                .then(|| Error::new(code, "the lug can't be made"))
        });
    }

    #[test]
    fn a_rollback_restores_what_was_reached_before_begin() {
        let f = opened(STORED);
        refuse_the_longer_lug(&f, ErrorCode::KernelFailed);
        f.begin();
        f.edit(&longer());
        f.sessions.commit(&f.id).wait().result.unwrap();
        let before = BTreeSet::from([id("b0000001"), id("f0000001")]);
        assert_eq!(*lock(&f.session().held).carried, before);
        f.begin();
        f.edit(&f.file().replace("\"length\": 40,", "\"length\": 41,"));
        f.sessions.rollback(&f.id).wait().result.unwrap();
        assert_eq!(*lock(&f.session().held).carried, before);
        assert_eq!(lock(&f.session().held).reach(), before);
    }

    #[test]
    fn format_of_no_features_accepts_no_drift() {
        let f = opened(STORED);
        let formatted = f
            .sessions
            .format(&f.id, Some(vec![]), false)
            .wait()
            .result
            .unwrap();
        assert!(formatted.accepted.is_empty());
        the_blocks_drift(&formatted.problems);
    }

    #[test]
    fn a_commits_new_file_is_claimed() {
        let f = opened(STORED);
        f.begin();
        f.edit(&longer());
        assert!(f.sessions.commit(&f.id).wait().result.unwrap().written);
        let link = f.path.with_file_name("link.silico");
        std::fs::hard_link(&f.path, &link).unwrap();
        let response = f.sessions.open(&link, false).wait();
        let problems = response.result.unwrap_err();
        let refusal = &problems.errors()[0];
        assert!(
            refusal.message().ends_with("is already open in session s1"),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_commit_a_kernel_fault_refuses_leaves_the_transaction_open() {
        let f = opened(STORED);
        refuse_the_longer_lug(&f, ErrorCode::WorkerCrashed);
        f.begin();
        f.edit(&longer());
        let response = f.sessions.commit(&f.id).wait();
        assert_eq!(
            response.result.unwrap_err().errors()[0].code(),
            ErrorCode::WorkerCrashed
        );
        assert!(response.session.unwrap().transaction);
        assert_eq!(f.file(), STORED);
    }

    #[test]
    fn a_rollback_stops_the_transactions_rebuild() {
        let f = opened(BOX);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        f.begin();
        f.edit(SPHERE);
        let _hold = f
            .kernel
            .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
        let _ = f.sessions.published(&f.id);
        assert!(f.kernel.wait_running(1, WAIT));
        assert_eq!(lock(&f.session().rebuilds).running(), Some(Revision(2)));
        f.sessions.rollback(&f.id).wait().result.unwrap();
        assert_eq!(lock(&f.session().rebuilds).running(), None);
        // Stopped: the kernel's held sphere ends, though its hold is never released.
        let deadline = std::time::Instant::now() + WAIT;
        while f.kernel.wait_running(1, Duration::ZERO) {
            assert!(
                std::time::Instant::now() < deadline,
                "the transaction's rebuild still runs"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// [`BOX`] with a length of 0, which the gate refuses.
    const FLAT: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 0, "width": 20, "height": 10}]}"#;

    #[test]
    fn a_commit_of_a_refused_document_unchanged_writes_nothing_and_ends() {
        let f = opened(FLAT);
        f.begin();
        let response = f.sessions.commit(&f.id).wait();
        let committed = response.result.unwrap();
        assert!(!committed.written);
        assert_eq!(committed.problems.errors()[0].parameter(), Some("length"));
        assert!(!response.session.unwrap().transaction);
        assert_eq!(f.file(), FLAT);
    }

    #[test]
    fn a_commit_writes_over_no_drift_of_an_edit_undone() {
        // The block made 41 long, then 40 again, and a note put on the lug: the block's
        // stored fingerprint (1e-6 over) is kept and its drift said; only the note is new.
        let f = opened(STORED);
        f.begin();
        f.edit(&STORED.replace(r#""length": 40"#, r#""length": 41"#));
        f.edit(&STORED.replace(
            r#""name": "Lug", "#,
            r#""name": "Lug", "note": "Lifting lug", "#,
        ));
        let committed = f.sessions.commit(&f.id).wait().result.unwrap();
        assert!(committed.written);
        assert!(committed.refingerprinted.is_empty());
        the_blocks_drift(&committed.problems);
        let saved = document::read(f.file().as_bytes()).unwrap();
        assert_eq!(
            saved.features[0].fingerprint.unwrap().volume,
            Measure::from_millionths(8_000_000_001)
        );
        assert_eq!(saved.features[1].note.as_deref(), Some("Lifting lug"));
    }

    #[test]
    fn from_text_that_didnt_read_a_commit_reaches_only_features_with_new_ids() {
        // A merge's conflict markers around the lug's length: the text doesn't read. Mended
        // with the lug 20 long and a ball added, only the ball, whose ID the text hadn't, is
        // reached: the rest keep their stored fingerprints, and their drift is said.
        let markers = STORED.replace(
            r#""length": 10, "width": 10"#,
            "\n<<<<<<< ours\n\"length\": 10, \"width\": 10\n=======\n\"length\": 20, \"width\": 10\n>>>>>>> theirs\n",
        );
        let f = opened(&markers);
        assert!(matches!(*lock(&f.session().held).gated, Gated::Unread(_)));
        f.begin();
        let ball = r#", {"id": "z0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 50], "radius": 5}"#;
        let mended = longer().replacen("\n]}", &format!("{ball}\n]}}"), 1);
        assert_ne!(mended, longer());
        f.edit(&mended);
        assert_eq!(
            lock(&f.session().held).reach(),
            BTreeSet::from([id("z0000001")])
        );
        let committed = f.sessions.commit(&f.id).wait().result.unwrap();
        assert!(committed.written);
        assert_eq!(committed.refingerprinted, [id("z0000001")]);
        let drifted: Vec<Option<&str>> = committed
            .problems
            .errors()
            .iter()
            .map(|e| e.feature())
            .collect();
        assert_eq!(
            drifted,
            [Some("k7f2qx3m"), Some("b0000001"), Some("f0000001")]
        );
    }

    #[test]
    fn a_rollback_gives_begins_still_running_rebuild_to_the_new_revision() {
        // Revision 2's rebuild running at begin, its pin held; an edit, then a rollback: the
        // rebuild goes on, as revision 4's, and the pin is asked for once.
        let f = opened(BOX);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        f.edit(BOX_AND_PIN);
        let hold = f.kernel.hold(is_cylinder);
        let _ = f.sessions.published(&f.id);
        assert!(f.kernel.wait_running(1, WAIT));
        assert_eq!(f.begin().base, Revision(2));
        f.edit(SPHERE);
        f.sessions.rollback(&f.id).wait().result.unwrap();
        assert_eq!(lock(&f.session().rebuilds).running(), Some(Revision(4)));
        hold.release();
        let check = f.sessions.check(&f.id).wait();
        assert!(check.result.unwrap().is_empty());
        assert_eq!(check.model.unwrap().revision, Revision(4));
        assert_eq!(f.made(is_cylinder), 1);
    }

    /// `f`'s check, which must answer within [`WAIT`]: if it doesn't (a revision left
    /// running for good), the sessions are leaked rather than dropped, since their drop would
    /// wait on the same rebuild, and the test fails rather than hangs.
    fn check_in_time(f: Fixture) -> (Fixture, Response<Problems>) {
        match f.sessions.check(&f.id).wait_timeout(WAIT) {
            Ok(check) => (f, check),
            Err(_) => {
                std::mem::forget(f);
                panic!("the check never answered: a revision is left running");
            }
        }
    }

    /// Revision 2's rebuild running, its pin held, when a transaction begins; then `rollbacks`
    /// transactions rolled back in a row, each begun from the one before's new revision,
    /// before the pin is released: the rebuild is each new revision's in turn, and its model
    /// the last's (1 + `rollbacks` + 1, the edit's 2 and a revision for each rollback).
    fn rollbacks_while_begins_rebuild_runs(rollbacks: u64) {
        let f = opened(BOX);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        f.edit(BOX_AND_PIN);
        let hold = f.kernel.hold(is_cylinder);
        let _ = f.sessions.published(&f.id);
        assert!(f.kernel.wait_running(1, WAIT));
        for rolled in 1..=rollbacks {
            assert_eq!(f.begin().base, Revision(1 + rolled));
            f.sessions.rollback(&f.id).wait().result.unwrap();
            assert_eq!(
                lock(&f.session().rebuilds).running(),
                Some(Revision(2 + rolled))
            );
        }
        hold.release();
        let (f, check) = check_in_time(f);
        assert!(check.result.unwrap().is_empty());
        assert_eq!(check.model.unwrap().revision, Revision(2 + rollbacks));
        // The one rebuild, never started again.
        assert_eq!(f.made(is_cylinder), 1);
    }

    #[test]
    fn two_rollbacks_while_begins_rebuild_runs_still_end() {
        rollbacks_while_begins_rebuild_runs(2);
    }

    #[test]
    fn three_rollbacks_while_begins_rebuild_runs_still_end() {
        rollbacks_while_begins_rebuild_runs(3);
    }

    #[test]
    fn a_rollback_after_the_rebuild_a_rollback_was_given_published_restores_its_model() {
        // Revision 2's rebuild given to revision 3 by a rollback, then published as 3's; a
        // transaction from 3 rolled back: that model is revision 5's, nothing rebuilt.
        let f = opened(BOX);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        f.edit(BOX_AND_PIN);
        let hold = f.kernel.hold(is_cylinder);
        let _ = f.sessions.published(&f.id);
        assert!(f.kernel.wait_running(1, WAIT));
        f.begin();
        f.sessions.rollback(&f.id).wait().result.unwrap();
        hold.release();
        let deadline = std::time::Instant::now() + WAIT;
        let model = loop {
            let published = f.sessions.published(&f.id).result.unwrap();
            if published.revision == Revision(3) {
                break published.model;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "revision 3 never published"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(f.begin().base, Revision(3));
        f.edit(SPHERE);
        let calls = f.kernel.calls().len();
        f.sessions.rollback(&f.id).wait().result.unwrap();
        let published = f.sessions.published(&f.id).result.unwrap();
        assert_eq!(published.revision, Revision(5));
        assert!(Arc::ptr_eq(&published.model, &model));
        assert_eq!(lock(&f.session().rebuilds).running(), None);
        let (f, check) = check_in_time(f);
        assert!(check.result.unwrap().is_empty());
        assert_eq!(check.model.unwrap().revision, Revision(5));
        assert_eq!(f.kernel.calls().len(), calls);
        assert_eq!(f.made(is_cylinder), 1);
    }

    #[test]
    fn a_rollback_of_a_transaction_that_built_nothing_starts_nothing() {
        let f = opened(BOX);
        f.begin();
        f.edit(SPHERE);
        f.sessions.rollback(&f.id).wait().result.unwrap();
        assert_eq!(lock(&f.session().rebuilds).running(), None);
        assert!(f.kernel.calls().is_empty());
    }

    #[test]
    fn a_rollback_restores_what_begin_saw_published_and_no_transaction_model_comes_after() {
        // Refused at begin, nothing published; the transaction's box built and published;
        // rolled back: nothing published again, and a late model of the transaction's
        // revision isn't either.
        let f = opened(FLAT);
        f.begin();
        f.edit(BOX);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        assert_eq!(
            f.sessions.published(&f.id).result.unwrap().revision,
            Revision(2)
        );
        f.sessions.rollback(&f.id).wait().result.unwrap();
        let refused = f.sessions.published(&f.id).result.unwrap_err();
        assert_eq!(refused.errors()[0].parameter(), Some("length"));
        let other = Recompute::new(f.kernel.client());
        watch(
            &Arc::downgrade(&f.session()),
            &Label::new(Revision(2)),
            other.start(checked(BOX)),
        );
        assert!(lock(&f.session().rebuilds).published.is_none());
    }

    #[test]
    fn a_save_this_session_left_part_written_refuses_the_next_saying_where_the_text_is() {
        let f = opened(STORED);
        let old = f.path.with_file_name(".a.silico.fmt-1");
        lock(&f.session().held).part_written = Some(old.clone());
        // What a write in place cut short leaves: the first half of the new text.
        let half = &COMMITTED[..COMMITTED.len() / 2];
        std::fs::write(&f.path, half).unwrap();
        let response = f.sessions.format(&f.id, None, true).wait();
        let problems = response.result.unwrap_err();
        assert_eq!(problems.total(), 1);
        assert_eq!(
            problems.errors()[0],
            part_written(f.path.to_str().unwrap(), &old)
        );
        assert_eq!(problems.errors()[0].code(), ErrorCode::IoFailed);
        assert!(
            problems.errors()[0]
                .message()
                .ends_with(&format!("its text before is in {}", shown(&old))),
            "{:?}",
            problems.errors()[0]
        );
        assert_eq!(
            problems.errors()[0].suggestion(),
            Some(
                "nothing was written: put the old text back in the file, then run the command again, or close the session and open the file again"
            )
        );
        assert_eq!(f.file(), half);
        // As the advice says: the old text put back, the command run again writes.
        std::fs::write(&f.path, STORED).unwrap();
        let again = f.sessions.format(&f.id, None, true).wait();
        assert!(!again.session.unwrap().reloaded);
        assert!(again.result.unwrap().written);
        assert_eq!(lock(&f.session().held).part_written, None);
        assert_ne!(f.file(), STORED);
    }

    #[test]
    fn a_file_this_sessions_own_save_left_part_written_isnt_reloaded() {
        let f = opened(STORED);
        let old = f.path.with_file_name(".a.silico.fmt-1");
        lock(&f.session().held).part_written = Some(old.clone());
        // What its save left: neither what the session wrote nor what was there.
        std::fs::write(&f.path, "part written").unwrap();
        let text = f.sessions.text(&f.id, None).wait();
        let session = text.session.unwrap();
        assert_eq!(
            (session.revision, session.reloaded),
            (Revision::OPENED, false)
        );
        assert_eq!(text.result.unwrap(), STORED);
        // And the next save says where the old text is.
        let problems = f
            .sessions
            .format(&f.id, None, true)
            .wait()
            .result
            .unwrap_err();
        assert_eq!(
            problems.errors()[0],
            part_written(f.path.to_str().unwrap(), &old)
        );
        assert_eq!(f.file(), "part written");
    }

    /// Makes `dir` read-only until dropped: a save there can't make its file beside.
    struct ReadOnly(PathBuf);

    impl ReadOnly {
        fn of(dir: &Path) -> Self {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
            ReadOnly(dir.to_owned())
        }
    }

    impl Drop for ReadOnly {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn a_save_that_cant_be_written_names_path_for_commit_and_format() {
        let f = opened(STORED);
        let dir = f.path.parent().unwrap().to_owned();
        f.begin();
        f.edit(&longer());
        let _read_only = ReadOnly::of(&dir);
        // Root (in a container, say) writes in a read-only directory all the same: then no
        // save here fails, and the test has nothing to show, so it ends.
        let probe = dir.join("probe");
        if std::fs::write(&probe, "").is_ok() {
            let _ = std::fs::remove_file(&probe);
            return;
        }
        let commit = f.sessions.commit(&f.id).wait();
        let problems = commit.result.unwrap_err();
        let refused = &problems.errors()[0];
        assert_eq!(
            (refused.code(), refused.parameter()),
            (ErrorCode::IoFailed, Some("path")),
            "{refused:?}"
        );
        assert!(commit.session.unwrap().transaction);
        f.sessions.rollback(&f.id).wait().result.unwrap();
        let format = f.sessions.format(&f.id, None, true).wait();
        let problems = format.result.unwrap_err();
        let refused = &problems.errors()[0];
        assert_eq!(
            (refused.code(), refused.parameter()),
            (ErrorCode::IoFailed, Some("path")),
            "{refused:?}"
        );
        assert_eq!(f.file(), STORED);
    }

    #[test]
    fn a_rollback_of_a_closed_session_starts_no_rebuild() {
        // A rebuild started in the transaction, so a rollback would start the restored
        // document's; but the session has closed (its sessions dropped, say): none starts.
        let f = opened(BOX);
        f.begin();
        f.edit(SPHERE);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        let session = f.session();
        session.close();
        let calls = f.kernel.calls().len();
        session.rollback().unwrap();
        assert_eq!(lock(&session.rebuilds).running(), None);
        assert_eq!(f.kernel.calls().len(), calls);
    }

    /// A 10 mm cube, its fingerprint stored as the fake measures it: 10³ = 1000 mm³, area
    /// 6 × 100 = 600.
    const CUBE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Cube", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10, "fingerprint": {"volume": 1000, "area": 600, "bbox": [0, 0, 0, 10, 10, 10], "faces": 6, "edges": 12, "vertices": 8}}]}"#;

    /// `f`'s model of the revision it's at, built by a check and published, put back with
    /// the cube measured 1e40 mm³: 1e46 millionths, past the 1.7e38 an i128 holds, so no
    /// fingerprint can hold it.
    fn plant_unholdable(f: &Fixture) {
        let revision = f.sessions.check(&f.id).wait().model.unwrap().revision;
        let session = f.session();
        let mut rebuilds = lock(&session.rebuilds);
        let published = rebuilds.published.as_mut().unwrap();
        assert_eq!(published.revision, revision);
        let cube = id("k7f2qx3m");
        let Some(FeatureOutcome::Built { built, .. }) = published.model.outcome(&cube) else {
            panic!("the cube didn't build");
        };
        let mut summary = built.summary().unwrap().clone();
        summary.volume = 1e40;
        published.model = Arc::new(published.model.measured_as(&cube, summary));
        assert!(!published.model.records(&cube));
    }

    /// The one problem `problems` holds: the cube's drift, measured past a fingerprint.
    #[track_caller]
    fn the_cubes_unholdable_drift(problems: &Problems) {
        assert_eq!(problems.total(), 1, "{problems:?}");
        let drift = &problems.errors()[0];
        assert_eq!(
            (drift.code(), drift.feature(), drift.parameter()),
            (ErrorCode::Drift, Some("k7f2qx3m"), Some("fingerprint")),
            "{drift:?}"
        );
    }

    #[test]
    fn measures_no_fingerprint_holds_are_drift_however_reached_or_accepted() {
        // Nothing takes the stored record's place: format accepts nothing, and an edit that
        // reaches the cube still has its drift said.
        let f = opened(CUBE);
        plant_unholdable(&f);
        let formatted = f.sessions.format(&f.id, None, false).wait().result.unwrap();
        assert!(formatted.accepted.is_empty(), "{:?}", formatted.accepted);
        the_cubes_unholdable_drift(&formatted.problems);
        f.begin();
        f.edit(&CUBE.replace(r#""length": 10"#, r#""length": 11"#));
        assert_eq!(
            lock(&f.session().held).reach(),
            BTreeSet::from([id("k7f2qx3m")])
        );
        plant_unholdable(&f);
        the_cubes_unholdable_drift(&f.sessions.check(&f.id).wait().result.unwrap());
    }

    #[test]
    fn a_save_that_left_the_file_part_written_is_marked_for_the_next() {
        let f = opened(STORED);
        let session = f.session();
        let failed = || Error::new(ErrorCode::IoFailed, "couldn't be written");
        // Its old text put back: nothing marked.
        let problems = session.unsaved(
            "a.silico",
            file::Unsaved {
                error: failed(),
                old_text_in: None,
            },
            Saver::Format,
        );
        assert_eq!(problems.errors()[0].parameter(), Some("path"));
        assert_eq!(lock(&session.held).part_written, None);
        // Left part written, its old text beside it: marked, so the next save says where.
        let old = f.path.with_file_name(".a.silico.fmt-1");
        let problems = session.unsaved(
            "a.silico",
            file::Unsaved {
                error: failed(),
                old_text_in: Some(old.clone()),
            },
            Saver::Format,
        );
        assert_eq!(
            (
                problems.errors()[0].code(),
                problems.errors()[0].parameter()
            ),
            (ErrorCode::IoFailed, Some("path"))
        );
        assert_eq!(lock(&session.held).part_written, Some(old));
    }

    #[test]
    fn a_format_after_a_reached_feature_builds_writes_its_record_and_carries_it_no_more() {
        // As `a_reached_feature_that_failed_stays_reached_after_its_commit`: the lug and the
        // fuse carried after the commit. Then the lug made 30 long, which builds: a format
        // writes both records, so neither is carried. By hand: the lug 30 × 10 × 10 = 3000,
        // area 2(300 + 100 + 300) = 1400; the fuse 8000 + 3000 = 11000, area 2800 + 1400 =
        // 4200.
        let f = opened(STORED);
        refuse_the_longer_lug(&f, ErrorCode::KernelFailed);
        f.begin();
        f.edit(&longer());
        assert!(f.sessions.commit(&f.id).wait().result.unwrap().written);
        assert_eq!(
            *lock(&f.session().held).carried,
            BTreeSet::from([id("b0000001"), id("f0000001")])
        );
        f.edit(&f.file().replace("\"length\": 20,", "\"length\": 30,"));
        let formatted = f.sessions.format(&f.id, None, true).wait().result.unwrap();
        assert!(formatted.written);
        assert!(lock(&f.session().held).carried.is_empty());
        assert!(lock(&f.session().held).reach().is_empty());
        let saved = document::read(f.file().as_bytes()).unwrap();
        let measures: Vec<(Measure, Measure)> = saved.features[1..]
            .iter()
            .map(|feature| {
                let fingerprint = feature.fingerprint.unwrap();
                (fingerprint.volume, fingerprint.area)
            })
            .collect();
        let mm = |whole: i128| Measure::from_millionths(whole * 1_000_000);
        assert_eq!(measures, [(mm(3000), mm(1400)), (mm(11000), mm(4200))]);
    }

    #[test]
    fn a_reload_carries_nothing_so_a_hand_edits_drift_is_said_and_not_written_over() {
        // As `a_reached_feature_that_failed_stays_reached_after_its_commit`: the lug and the
        // fuse carried after the commit, their fingerprints as stored (the lug 1000, the
        // fuse 9000). Then the lug made 30 long by hand, which builds: 30 × 10 × 10 = 3000,
        // area 2(300 + 100 + 300) = 1400; the fuse 8000 + 3000 = 11000, area 2800 + 1400 =
        // 4200. A hand edit reaches nothing, and a reload is as a fresh open: the lug's and
        // the fuse's stale fingerprints are drift, with the block's (stored 1e-6 over).
        let f = opened(STORED);
        refuse_the_longer_lug(&f, ErrorCode::KernelFailed);
        f.begin();
        f.edit(&longer());
        assert!(f.sessions.commit(&f.id).wait().result.unwrap().written);
        assert_eq!(
            *lock(&f.session().held).carried,
            BTreeSet::from([id("b0000001"), id("f0000001")])
        );
        let by_hand = f.file().replace("\"length\": 20,", "\"length\": 30,");
        assert_ne!(by_hand, f.file());
        std::fs::write(&f.path, &by_hand).unwrap();
        let check = f.sessions.check(&f.id).wait();
        let session = check.session.unwrap();
        assert!(session.reloaded);
        let drift: Vec<(ErrorCode, Option<&str>)> = check
            .result
            .as_ref()
            .unwrap()
            .errors()
            .iter()
            .map(|e| (e.code(), e.feature()))
            .collect();
        assert_eq!(
            drift,
            [
                (ErrorCode::Drift, Some("k7f2qx3m")),
                (ErrorCode::Drift, Some("b0000001")),
                (ErrorCode::Drift, Some("f0000001"))
            ]
        );
        assert!(lock(&f.session().held).carried.is_empty());
        // A note's edit reaches nothing: its commit writes no fingerprint over the drift.
        let note = f
            .sessions
            .set_field(
                &f.id,
                session.revision,
                id("k7f2qx3m"),
                "note".to_owned(),
                crate::document::json::read(br#""by hand""#).unwrap(),
            )
            .wait()
            .result
            .unwrap();
        let commit = note.commit.unwrap();
        assert!(commit.written);
        assert!(commit.refingerprinted.is_empty(), "{commit:?}");
        let saved = document::read(f.file().as_bytes()).unwrap();
        let volumes: Vec<Measure> = saved
            .features
            .iter()
            .map(|feature| feature.fingerprint.unwrap().volume)
            .collect();
        assert_eq!(
            volumes,
            [
                Measure::from_millionths(8_000_000_001),
                Measure::from_millionths(1_000_000_000),
                Measure::from_millionths(9_000_000_000)
            ]
        );
    }

    #[test]
    fn a_save_moves_only_its_own_sessions_claim() {
        // Two sessions, s1 on a.silico and s2 on b.silico; s1's format writes a.silico anew:
        // b.silico is still s2's, by its file too, so a hard link to it is refused.
        let f = opened(STORED);
        let other = f.path.with_file_name("b.silico");
        std::fs::write(&other, BOX).unwrap();
        let second = f.sessions.open(&other, false).wait().session.unwrap().id;
        assert_eq!(second, SessionId(2));
        assert!(
            f.sessions
                .format(&f.id, None, true)
                .wait()
                .result
                .unwrap()
                .written
        );
        let link = f.path.with_file_name("link.silico");
        std::fs::hard_link(&other, &link).unwrap();
        let problems = f.sessions.open(&link, false).wait().result.unwrap_err();
        let refusal = &problems.errors()[0];
        assert_eq!(refusal.parameter(), Some("path"), "{refusal:?}");
        assert!(
            refusal.message().ends_with("is already open in session s2"),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_commit_from_text_that_still_doesnt_read_writes_nothing_and_ends() {
        let f = opened("{");
        f.begin();
        let response = f.sessions.commit(&f.id).wait();
        let committed = response.result.unwrap();
        assert!(!committed.written);
        assert!(!response.session.unwrap().transaction);
        assert_eq!(f.file(), "{");
    }

    #[test]
    fn a_commit_of_a_document_the_gate_took_at_begin_and_refuses_now_is_refused_and_left_open() {
        // Taken at begin; then held refused, set here: no command holds one inside a
        // transaction (each edit goes through the gate, and a file changed outside isn't
        // reloaded inside one, decision 2), but the commit mustn't rely on it. The commit is
        // refused with the gate's problems, the document stays held as it is, and the
        // transaction open.
        let f = opened(STORED);
        f.begin();
        let session = f.session();
        {
            let mut held = lock(&session.held);
            let next = held.next().unwrap();
            let carried = Arc::clone(&held.carried);
            held.change(
                next,
                Arc::new(FLAT.as_bytes().to_vec()),
                Arc::new(Gated::of(FLAT.as_bytes())),
                carried,
            );
        }
        let response = f.sessions.commit(&f.id).wait();
        let problems = response.result.unwrap_err();
        assert_eq!(problems.total(), 1, "{problems:?}");
        assert_eq!(
            (
                problems.errors()[0].feature(),
                problems.errors()[0].parameter()
            ),
            (Some("k7f2qx3m"), Some("length"))
        );
        let after = response.session.unwrap();
        assert_eq!((after.revision, after.transaction), (Revision(2), true));
        assert_eq!(f.sessions.text(&f.id, None).wait().result.unwrap(), FLAT);
        assert_eq!(f.file(), STORED);
    }

    /// The bytes that map to `k7f2qx3m`, by hand: its characters' places in the alphabet,
    /// 19, 7, 15, 2, 23, 29, 3, 20, five bits each (10011 00111 01111 00010 10111 11101 00011
    /// 10100), eight at a time.
    const BLOCKS_ID: [u8; 5] = [0x99, 0xde, 0x2b, 0xf4, 0x74];

    /// The bytes that map to `12345678`: 00001 00010 ... 01000.
    const DIGITS_ID: [u8; 5] = [0x08, 0x86, 0x42, 0x98, 0xe8];

    #[test]
    fn a_feature_added_without_an_id_takes_the_next_id_the_document_doesnt_hold() {
        let f = opened(BOX);
        *lock(&f.sessions.registry.feature_ids) = FeatureIds::given([BLOCKS_ID, DIGITS_ID]);
        f.begin();
        let ball = crate::document::json::read(
            br#"{"type": "sphere", "name": "Ball", "position": [0, 0, 50], "radius": 5}"#,
        )
        .unwrap();
        let added = f
            .sessions
            .add_feature(&f.id, Revision::OPENED, ball)
            .wait()
            .result
            .unwrap();
        // The block's ID first, which the document holds: passed over.
        assert_eq!(added.feature.id, id("12345678"));
    }

    /// Waits, up to [`WAIT`], until `done` says the threads it looks at have ended.
    fn ended(done: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + WAIT;
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "the threads never ended"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn all_ended(threads: &Threads) -> bool {
        threads.running.iter().all(JoinHandle::is_finished)
    }

    #[test]
    fn published_reads_that_ended_are_joined_as_the_next_starts() {
        let f = opened(BOX);
        assert!(f.sessions.check(&f.id).wait().result.unwrap().is_empty());
        let read = || {
            f.sessions
                .read_model(&f.id, "probe", ModelChoice::Published, |read| {
                    Ok(read.model_read().revision)
                })
                .wait()
                .result
                .unwrap()
        };
        let session = f.session();
        let threads = || {
            lock(&session.reads)
                .threads
                .as_ref()
                .map(|t| t.running.len())
        };
        for _ in 0..4 {
            assert_eq!(read(), Revision::OPENED);
        }
        ended(|| all_ended(lock(&session.reads).threads.as_ref().unwrap()));
        // The four ended, joined as the fifth starts: its thread alone is held.
        assert_eq!(read(), Revision::OPENED);
        assert_eq!(threads(), Some(1));
    }

    #[test]
    fn session_threads_that_ended_are_joined_as_the_next_opens() {
        let f = opened(BOX);
        let other = f.path.with_file_name("b.silico");
        std::fs::write(&other, BOX).unwrap();
        for _ in 0..3 {
            let id = f.sessions.open(&other, false).wait().session.unwrap().id;
            f.sessions.close(&id).wait().result.unwrap();
        }
        let book = || lock(&f.sessions.registry.book);
        // Each closed session's thread ends (those not joined yet); s1's runs on.
        ended(|| {
            book()
                .threads
                .running
                .iter()
                .filter(|thread| !thread.is_finished())
                .count()
                == 1
        });
        f.sessions.open(&other, false).wait().session.unwrap();
        // The three ended, joined as the next opens: s1's thread and the new one's.
        assert_eq!(book().threads.running.len(), 2);
    }
}
