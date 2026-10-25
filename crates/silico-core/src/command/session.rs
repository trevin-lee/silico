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
//! What it doesn't bound: the caches, one kept for each directory a session has opened a
//! document in, for the process's life; and each session's queue, which holds every command
//! queued and not yet run. Nor does it choose which files may be opened: `open` and `text`
//! read any file this user may read, so a server for clients it doesn't trust gives them a
//! workspace (decision 26) once there is one.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use super::{Gated, Problems};
use crate::cache::Cache;
use crate::document::json::MAX_INPUT_LEN;
use crate::document::{self, Checked, Document, FeatureId, FormatVersion, NamingScheme, Units};
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
    pub path: PathBuf,
    pub revision: Revision,
    /// Whether a transaction is open: never yet, until transactions come (the design's
    /// section 4).
    pub transaction: bool,
    /// Whether the session read its file again before the command, finding it changed:
    /// never yet, until a file changed outside the session is reloaded (decision 2).
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

/// The model a command read (the envelope's `model`): the revision it was rebuilt from, the
/// session's current one's (a read of the published model, and the fault a rebuild met, come
/// with decisions 33 and 32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRead {
    pub revision: Revision,
}

/// The newest model a session's rebuilds published, and the revision it was rebuilt from
/// (the same as its response's `model.revision`): what [`Sessions::published`] gives the
/// app to draw, never waiting for a rebuild.
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
}

impl<T> Running<T> {
    /// A command answered before it was queued (refused: no such session, too many open).
    fn answered(response: Response<T>) -> Self {
        let (reply, answer) = mpsc::sync_channel(1);
        let _ = reply.send(response);
        Running { answer }
    }

    /// Waits for the answer.
    pub fn wait(self) -> Response<T> {
        self.answer.recv().unwrap_or_else(|_| unanswered())
    }

    /// Waits up to `timeout` for the answer, or gives the handle back to wait on again.
    pub fn wait_timeout(self, timeout: Duration) -> Result<Response<T>, Self> {
        match self.answer.recv_timeout(timeout) {
            Ok(response) => Ok(response),
            Err(RecvTimeoutError::Timeout) => Err(self),
            Err(RecvTimeoutError::Disconnected) => Ok(unanswered()),
        }
    }

    /// The answer, if it has come, or the handle back to wait on.
    pub fn try_take(self) -> Result<Response<T>, Self> {
        match self.answer.try_recv() {
            Ok(response) => Ok(response),
            Err(TryRecvError::Empty) => Err(self),
            Err(TryRecvError::Disconnected) => Ok(unanswered()),
        }
    }
}

/// The answer of a command whose thread ended without giving one (a bug).
fn unanswered<T>() -> Response<T> {
    Response {
        session: None,
        model: None,
        result: Err(Problems::from(
            Error::new(
                ErrorCode::Internal,
                "the session stopped before the command answered",
            )
            .with_suggestion(crate::worker::SILICO_BUG),
        )),
    }
}

/// What `open` found: the path as given, whether it made the file, how many features the
/// document has (none if its text doesn't read as a document), the bodies its timeline ends
/// with (as `silico show` finds them, needing no kernel), and the gate's problems, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub path: PathBuf,
    pub created: bool,
    pub features: Option<usize>,
    pub bodies: Vec<FeatureId>,
    pub problems: Problems,
}

/// What `status` says of a session, from what it holds: how many features its document has
/// (none if its text doesn't read as one), and whether the gate took it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub features: Option<usize>,
    pub gated: bool,
}

/// A process's open sessions, on one kernel: each session's commands are run on its own
/// thread, in turn, on its own client of `kernel`. Dropping it closes every session,
/// stopping its rebuild, and waits for each session's thread to end.
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
    threads: BTreeMap<SessionId, JoinHandle<()>>,
    /// The sessions are being dropped: an open that finishes now doesn't open.
    stopping: bool,
}

impl<C: Kernel + 'static> Book<C> {
    /// Session `id` gone from every record but its thread's: open, opening, and the file
    /// it held.
    fn release(&mut self, id: SessionId) {
        self.sessions.remove(&id);
        self.opening.remove(&id);
        self.files.retain(|_, (holder, _)| *holder != id);
        self.paths.retain(|_, holder| *holder != id);
    }

    /// The threads that have ended, joined.
    fn reap(&mut self) {
        let ended: Vec<SessionId> = self
            .threads
            .iter()
            .filter(|(_, thread)| thread.is_finished())
            .map(|(id, _)| *id)
            .collect();
        for id in ended {
            if let Some(thread) = self.threads.remove(&id) {
                let _ = thread.join();
            }
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
    /// The revision whose rebuild is running, if one is.
    running: Option<Revision>,
    /// The newest model the session's rebuilds published.
    published: Option<Published>,
    /// The last rebuild whose recompute failed (a bug), until another starts.
    failed: Option<Failed>,
    /// The threads watching the rebuilds started: those ended joined as the next starts, the
    /// rest by `Sessions::drop`, for the sessions open then.
    watchers: Vec<JoinHandle<()>>,
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
    path: PathBuf,
    /// The path as its links resolve, where saves and reloads go.
    resolved: PathBuf,
    /// The document's text: the file's bytes as read or written, until an edit.
    text: Arc<Vec<u8>>,
    /// The text as the reader and the gate found it.
    gated: Arc<Gated>,
    revision: Revision,
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
        checked: &Checked,
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
            if rebuilds.running == Some(revision) {
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
            // Not built, or superseded: started again unless the session has gone.
            if self.closed() {
                return Err(Problems::from(not_open(self.id)));
            }
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
    /// whether it started. The caller decides whether the revision's model is wanted (none
    /// of it, and no rebuild of it running). Doesn't wait for it: a watcher thread records
    /// what it comes to. Takes the lock's guard, so the start and the record of it are one
    /// step.
    fn start_unless_newer(
        self: &Arc<Self>,
        rebuilds: &mut Rebuilds<C>,
        revision: Revision,
        checked: &Checked,
    ) -> Result<bool, Problems> {
        if rebuilds.running.is_some_and(|running| running > revision)
            || rebuilds
                .published
                .as_ref()
                .is_some_and(|published| published.revision > revision)
        {
            return Ok(false);
        }
        let (ended, watching): (Vec<_>, Vec<_>) = std::mem::take(&mut rebuilds.watchers)
            .into_iter()
            .partition(JoinHandle::is_finished);
        rebuilds.watchers = watching;
        for watcher in ended {
            let _ = watcher.join();
        }
        let run = rebuilds.engine.start(checked.clone());
        rebuilds.running = Some(revision);
        rebuilds.failed = None;
        let session = Arc::downgrade(self);
        match std::thread::Builder::new()
            .name(format!("session-{}-rebuild", self.id))
            .spawn(move || watch(&session, revision, run))
        {
            Ok(watcher) => {
                rebuilds.watchers.push(watcher);
                Ok(true)
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

    /// The design's published read of the session at `revision`, whose document is
    /// `checked`: the newest model published of `revision` or an older one, never waiting;
    /// `revision`'s rebuild started if nothing of it, or newer, is published or running, and
    /// its recompute hasn't failed (a failure isn't started again each frame, but left for
    /// `check` to say once and rebuild). Refused with that failure, or, with nothing
    /// published, naming `model`.
    fn published_read(
        self: &Arc<Self>,
        revision: Revision,
        checked: &Checked,
    ) -> Result<Published, Problems> {
        let mut rebuilds = lock(&self.rebuilds);
        let failed = rebuilds
            .failed
            .as_ref()
            .filter(|failed| failed.revision == revision)
            .map(|failed| failed.why.clone());
        if failed.is_none()
            && rebuilds.running != Some(revision)
            && rebuilds
                .published
                .as_ref()
                .is_none_or(|published| published.revision < revision)
            && !self.closed()
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

    /// The session as it is now.
    fn info(&self) -> SessionInfo {
        self.info_of(&lock(&self.held))
    }

    /// The session as `held` says it is.
    fn info_of(&self, held: &Held) -> SessionInfo {
        SessionInfo {
            id: self.id,
            path: held.path.clone(),
            revision: held.revision,
            transaction: false,
            reloaded: false,
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

/// Waits for `revision`'s rebuild, then records what it came to in `session`'s rebuilds and
/// says so, if the session is still there.
fn watch<C: Kernel + 'static>(session: &Weak<Session<C>>, revision: Revision, run: Run) {
    let outcome = run.wait();
    let Some(session) = session.upgrade() else {
        return;
    };
    let mut rebuilds = lock(&session.rebuilds);
    if rebuilds.running == Some(revision) {
        rebuilds.running = None;
    }
    match outcome {
        // A late model of an older revision never takes a newer one's place.
        Ok(RunOutcome::Published(model)) => {
            if rebuilds
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
                book: Mutex::new(Book {
                    next: 1,
                    sessions: BTreeMap::new(),
                    opening: BTreeSet::new(),
                    files: BTreeMap::new(),
                    paths: BTreeMap::new(),
                    caches: BTreeMap::new(),
                    threads: BTreeMap::new(),
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
    /// in another session of this process (by any path to it), or [`MAX_SESSIONS`] are open
    /// or opening. The session's ID is taken when the open is asked for, since its thread is
    /// named by it: an open refused on that thread (no such file, open already) uses an ID
    /// no session then has, and one refused here (too many open, the sessions closing)
    /// uses none.
    pub fn open(&self, path: impl Into<PathBuf>, create: bool) -> Running<Opened> {
        let path = path.into();
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
                book.threads.insert(id, thread);
                Running { answer }
            }
            Err(e) => Running::answered(refused(crate::pool::thread_error("session", &e))),
        }
    }

    /// Closes `session`, once the commands queued before it have run: commands queued
    /// after it are told the session closed. Its file is then free to open again.
    pub fn close(&self, session: &SessionId) -> Running<()> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(session, "close", move |session, _| {
            session.close();
            if let Some(registry) = registry.upgrade() {
                lock(&registry.book).release(session.id);
            }
            Ok(())
        })
    }

    /// `session`'s status, answered on the caller's thread from what the session holds,
    /// even while one of its commands runs.
    pub fn status(&self, session: &SessionId) -> Response<Status> {
        let Some(found) = self.find(session) else {
            return refused(not_open(*session));
        };
        let held = lock(&found.held);
        Response {
            session: Some(found.info_of(&held)),
            model: None,
            result: Ok(Status {
                features: held.gated.document().ok().map(|d| d.features.len()),
                gated: matches!(*held.gated, Gated::Checked(_)),
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
    /// it hasn't been: decision 13), each failed feature's and each drift. A kernel fault (a
    /// worker that crashed or hung) is the command's refusal, as `command::check` gives it,
    /// and the next check rebuilds, until faults are reported beside the result and kept as
    /// the revision's outcome (decision 32).
    pub fn check(&self, session: &SessionId) -> Running<Problems> {
        self.queued(session, "check", |session, model| {
            let (revision, gated) = {
                let held = lock(&session.held);
                (held.revision, Arc::clone(&held.gated))
            };
            match &*gated {
                Gated::Checked(checked) => {
                    let built = session.model(revision, checked)?;
                    *model = Some(ModelRead { revision });
                    super::check(&built).map_err(Problems::from)
                }
                refused => Ok(refused.problems()),
            }
        })
    }

    /// The newest model `session`'s rebuilds have published (the design's published read,
    /// section 2), of the session's revision or an older one, its revision in the response's
    /// `model`; refused, naming `model`, if none has been. Answered on the caller's thread,
    /// never waiting for a rebuild, for the app to draw each frame; it starts the current
    /// revision's rebuild if that has no model and none is running (decision 13), unless its
    /// recompute failed (then that failure is the refusal, until a check rebuilds it). A
    /// document the gate refused has nothing to publish: its read is refused with the gate's
    /// problems. What comes
    /// with decision 33's step: `published` as a request's `model` to every read command, and
    /// published reads that need the kernel on a read lane of their own.
    pub fn published(&self, session: &SessionId) -> Response<Published> {
        let Some(found) = self.find(session) else {
            return refused(not_open(*session));
        };
        let (info, revision, gated) = {
            let held = lock(&found.held);
            (found.info_of(&held), held.revision, Arc::clone(&held.gated))
        };
        let checked = match &*gated {
            Gated::Checked(checked) => checked,
            // Nothing is built of a document the gate refused: what's wrong is the gate's.
            refused => {
                return Response {
                    session: Some(info),
                    model: None,
                    result: Err(refused.problems()),
                };
            }
        };
        let read = found.published_read(revision, checked);
        Response {
            session: Some(info),
            model: read.as_ref().ok().map(|published| ModelRead {
                revision: published.revision,
            }),
            result: read,
        }
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

    fn find(&self, session: &SessionId) -> Option<Arc<Session<P::Client>>> {
        lock(&self.registry.book).sessions.get(session).cloned()
    }

    /// `work` queued on `session`'s thread, under the caller's span and the command's.
    fn queued<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: &'static str,
        work: impl FnOnce(&Arc<Session<P::Client>>, &mut Option<ModelRead>) -> Result<T, Problems>
        + Send
        + 'static,
    ) -> Running<T> {
        let Some(found) = self.find(session) else {
            return Running::answered(refused(not_open(*session)));
        };
        let (reply, answer) = mpsc::sync_channel(1);
        let caller = tracing::Span::current();
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
                    let result = work(session, &mut model);
                    Response {
                        session: Some(session.info()),
                        model,
                        result,
                    }
                }
                Turn::Closed(id) => refused(not_open(id)),
            };
            let _ = reply.send(response);
        }));
        Running { answer }
    }
}

impl<P: Clients + 'static> Drop for Sessions<P> {
    fn drop(&mut self) {
        let (sessions, threads) = {
            let mut book = lock(&self.registry.book);
            book.stopping = true;
            (
                std::mem::take(&mut book.sessions),
                std::mem::take(&mut book.threads),
            )
        };
        // Each queue ended, so its thread ends once its command does, and its rebuild
        // stopped, so a command waiting on one isn't waited for.
        for session in sessions.values() {
            session.close();
            lock(&session.rebuilds).engine.cancel();
        }
        for (_, thread) in threads {
            let _ = thread.join();
        }
        // And the threads watching its rebuilds, each of which ends once its stopped run
        // does: so no session outlives the drop.
        for session in sessions.values() {
            let watchers = std::mem::take(&mut lock(&session.rebuilds).watchers);
            for watcher in watchers {
                let _ = watcher.join();
            }
        }
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
    path: &Path,
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
                watchers: Vec::new(),
            }),
            rebuilt: Condvar::new(),
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
                session: Some(session.info()),
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
    path: &Path,
    create: bool,
) -> Result<(Held, Opened, Cache, Option<MadeFile>), Problems> {
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
            return Err(Problems::from(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "{} is more than the {MAX_INPUT_LEN} bytes Silico reads of a document",
                        shown(path)
                    ),
                )
                .with_parameter("path")
                .with_suggestion("check it's the document you mean: no document is this long"),
            ));
        }
        claim(registry, opened, &resolved, id, path)?;
        (resolved, text, None)
    };
    let gated = Gated::of(&text);
    let opened = Opened {
        path: path.to_owned(),
        created: create,
        features: gated.document().ok().map(|d| d.features.len()),
        bodies: gated
            .document()
            .map(|d| super::bodies(d).into_iter().cloned().collect())
            .unwrap_or_default(),
        problems: gated.problems(),
    };
    let held = Held {
        path: path.to_owned(),
        resolved,
        text: Arc::new(text),
        gated: Arc::new(gated),
        revision: Revision::OPENED,
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
    claim(registry, made, &resolved, id, path)?;
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

/// The file `opened`, at `resolved` (its path through links), claimed for session `id` by
/// both: the file it is and its path, the file kept open while it's claimed. Refused, naming
/// the session, if another session holds or is opening either: the same file by another name
/// (a hard link, macOS's data-volume name), or another file put at its path since.
fn claim<P: Clients + 'static>(
    registry: &Registry<P>,
    opened: std::fs::File,
    resolved: &Path,
    id: SessionId,
    given: &Path,
) -> Result<(), Problems> {
    let file = FileId::of(&opened).map_err(|e| e.with_parameter("path"))?;
    let mut book = lock(&registry.book);
    let holder = book
        .files
        .get(&file)
        .map(|(holder, _)| *holder)
        .into_iter()
        .chain(book.paths.get(resolved).copied())
        .find(|holder| *holder != id);
    match holder {
        Some(holder) => Err(Problems::from(
            Error::new(
                ErrorCode::InvalidInput,
                format!("{} is already open in session {holder}", shown(given)),
            )
            .with_parameter("path")
            .with_suggestion(format!(
                "use session {holder}, which this client opened, or close it first"
            )),
        )),
        None => {
            book.files.insert(file, (id, opened));
            book.paths.insert(resolved.to_owned(), id);
            Ok(())
        }
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
    use crate::fake::{FakeClient, FakeKernel};
    use crate::kernel::{Op, Primitive};

    const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
    const SPHERE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "a91cw0de", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}]}"#;
    const WAIT: Duration = Duration::from_secs(60);

    fn checked(text: &str) -> Checked {
        Checked::new(document::read(text.as_bytes()).unwrap()).unwrap()
    }

    /// A session as `start` makes one, at revision 1 of [`BOX`], on its own client of
    /// `kernel`; its queue open, though nothing serves it. Edits, which move a session to a
    /// new revision, come later: these tests drive `Session::model` at revisions 1 and 2.
    fn session(kernel: &FakeKernel) -> (Arc<Session<FakeClient>>, Receiver<Job<FakeClient>>) {
        let (sender, jobs) = mpsc::channel();
        let session = Arc::new(Session {
            id: SessionId(1),
            held: Mutex::new(Held {
                path: PathBuf::from("a.silico"),
                resolved: PathBuf::from("a.silico"),
                text: Arc::new(BOX.as_bytes().to_vec()),
                gated: Arc::new(Gated::of(BOX.as_bytes())),
                revision: Revision(1),
            }),
            queue: Mutex::new(Some(sender)),
            rebuilds: Mutex::new(Rebuilds {
                engine: Recompute::new(kernel.client()),
                running: None,
                published: None,
                failed: None,
                watchers: Vec::new(),
            }),
            rebuilt: Condvar::new(),
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
            built_from(&session.model(Revision(1), &checked(BOX)).unwrap()),
            "k7f2qx3m"
        );
        // Revision 2's read is never revision 1's model, published as it is.
        let second = session.model(Revision(2), &checked(SPHERE)).unwrap();
        assert_eq!(built_from(&second), "a91cw0de");
        // A model of revision 1 arriving late (another engine's, here) leaves 2's in place.
        let other = Recompute::new(kernel.client());
        watch(
            &Arc::downgrade(&session),
            Revision(1),
            other.start(checked(BOX)),
        );
        let published = lock(&session.rebuilds).published.clone().unwrap();
        assert_eq!(published.revision, Revision(2));
        assert!(Arc::ptr_eq(&published.model, &second));
        // Nor is revision 1's rebuild started again now that 2's has published.
        let mut rebuilds = lock(&session.rebuilds);
        assert!(
            !session
                .start_unless_newer(&mut rebuilds, Revision(1), &checked(BOX))
                .unwrap()
        );
        assert_eq!(rebuilds.running, None);
    }

    #[test]
    fn a_rebuild_is_refused_only_under_a_newer_revisions() {
        let kernel = FakeKernel::new(1).unwrap();
        let (session, _jobs) = session(&kernel);
        let _hold = kernel.hold(|_| true);
        let mut rebuilds = lock(&session.rebuilds);
        // Revision 2's rebuild running (as recorded): revision 1's isn't started under it.
        rebuilds.running = Some(Revision(2));
        assert!(
            !session
                .start_unless_newer(&mut rebuilds, Revision(1), &checked(BOX))
                .unwrap()
        );
        assert_eq!(rebuilds.running, Some(Revision(2)));
        // Its own revision's running: started (as a check after a fault starts it again).
        rebuilds.running = Some(Revision(1));
        assert!(
            session
                .start_unless_newer(&mut rebuilds, Revision(1), &checked(BOX))
                .unwrap()
        );
        assert_eq!(rebuilds.running, Some(Revision(1)));
        drop(rebuilds);
        assert!(kernel.wait_running(1, WAIT));
    }

    #[test]
    fn a_published_read_starts_its_revisions_rebuild_only_while_an_older_is_published() {
        let kernel = FakeKernel::new(1).unwrap();
        let (session, _jobs) = session(&kernel);
        let first = session.model(Revision(1), &checked(BOX)).unwrap();
        // At revision 1, with revision 1 published: that model, and nothing started.
        let read = session.published_read(Revision(1), &checked(BOX)).unwrap();
        assert_eq!(read.revision, Revision(1));
        assert!(Arc::ptr_eq(&read.model, &first));
        assert_eq!(lock(&session.rebuilds).running, None);
        // At revision 2, with only revision 1 published: revision 1's model, at once, and
        // revision 2's rebuild started (held, so it's seen running).
        let _hold = kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
        let read = session
            .published_read(Revision(2), &checked(SPHERE))
            .unwrap();
        assert_eq!(read.revision, Revision(1));
        assert!(Arc::ptr_eq(&read.model, &first));
        assert_eq!(lock(&session.rebuilds).running, Some(Revision(2)));
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
            let read = scope.spawn(|| session.model(Revision(1), &checked(BOX)));
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
            revision: Revision(2),
            model: newer,
        });
        let read = sessions.published(&id);
        assert_eq!(read.result.unwrap_err().errors()[0], nothing_published(id));
        // Nor does it start revision 1's rebuild, whose model would be older than one there.
        assert_eq!(lock(&session.rebuilds).running, None);
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
            &made,
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
                &kept,
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
}
