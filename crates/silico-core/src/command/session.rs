//! Sessions (docs/design/m4.1-command-api.md, section 3): a document held open between
//! commands, as every client holds one: MCP's tools, the app's windows, and the CLI, which
//! opens one, runs a command, and closes it.
//!
//! A process's sessions are one [`Sessions`], one client's (decision 30). Each session runs
//! its commands one at a time, in the order they arrive, on a thread of its own
//! (`session-s1`, ...), never the caller's (ARCHITECTURE.md, invariant 4): even `open`'s
//! file read and gate. A command returns a [`Running`] handle at once, which the app polls,
//! MCP's server waits on, and the CLI waits for; only [`Sessions::status`] answers on the
//! caller's thread, from what the session holds. Each answer is a [`Response`]: the session
//! as the command left it, and the command's result, or every reason it was refused.
//!
//! A file is open in one session a process (decision 22), known by its device and inode,
//! so no path to it (a link, a hard link, another name the system gives it) opens it twice
//! and two sessions never save one file over each other. At most [`MAX_SESSIONS`] are open
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
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use super::{Gated, Problems};
use crate::cache::Cache;
use crate::document::json::MAX_INPUT_LEN;
use crate::document::{self, Document, FeatureId, FormatVersion, NamingScheme, Units};
use crate::file;
use crate::kernel::{Clients, Kernel};
use crate::recompute::Recompute;
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
/// or a session that isn't open), and the command's result, or every reason it was refused,
/// in the order found, the first [`document::PROBLEMS_LISTED`] listed and all counted.
#[derive(Debug, Clone, PartialEq)]
pub struct Response<T> {
    pub session: Option<SessionInfo>,
    pub result: Result<T, Problems>,
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

struct Registry<P: Clients + 'static> {
    kernel: P,
    open_cache: Box<CacheOpener>,
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
    /// Each file a session holds, or is opening.
    files: BTreeMap<FileId, SessionId>,
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
        self.files.retain(|_, holder| *holder != id);
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
}

/// A session's rebuilds: its engine, on its own client of the kernel, keeping its steps in
/// its directory's cache. Its rebuilds come with the commands that read a model.
struct Rebuilds<C: Kernel + 'static> {
    engine: Recompute<C>,
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
    Run(&'a Session<C>),
    Closed(SessionId),
}

impl<C: Kernel + 'static> Session<C> {
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
        Sessions {
            registry: Arc::new(Registry {
                kernel,
                open_cache: Box::new(open),
                book: Mutex::new(Book {
                    next: 1,
                    sessions: BTreeMap::new(),
                    opening: BTreeSet::new(),
                    files: BTreeMap::new(),
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
    /// in another session of this process (by any path to it), or [`MAX_SESSIONS`] are open.
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
        self.queued(session, "close", move |session| {
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
        self.queued(session, "text", move |session| {
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

    /// `session`'s document's outline ([`super::outline`]), as `silico show` prints it.
    /// Refused, with the reader's error, if the text doesn't read as a document.
    pub fn outline(&self, session: &SessionId) -> Running<String> {
        self.queued(session, "outline", |session| {
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
        work: impl FnOnce(&Session<P::Client>) -> Result<T, Problems> + Send + 'static,
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
                    let result = work(session);
                    Response {
                        session: Some(session.info()),
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
        drop(sessions);
        for (_, thread) in threads {
            let _ = thread.join();
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
    let opened = open(&registry, id, path, create).and_then(|(held, opened, cache)| {
        let mut book = lock(&registry.book);
        if book.stopping {
            drop(book);
            if opened.created {
                unmake(&held);
            }
            return Err(Problems::from(closing()));
        }
        let (sender, jobs) = mpsc::channel();
        let session = Arc::new(Session {
            id,
            held: Mutex::new(held),
            queue: Mutex::new(Some(sender)),
            rebuilds: Mutex::new(Rebuilds {
                engine: Recompute::with_cache(registry.kernel.client(), cache),
            }),
        });
        book.opening.remove(&id);
        book.sessions.insert(id, Arc::clone(&session));
        Ok((session, opened, jobs))
    });
    match opened {
        Ok((session, opened, jobs)) => {
            let _ = reply.send(Response {
                session: Some(session.info()),
                result: Ok(opened),
            });
            Some(Started { session, jobs })
        }
        Err(problems) => {
            lock(&registry.book).release(id);
            let _ = reply.send(Response {
                session: None,
                result: Err(problems),
            });
            None
        }
    }
}

/// The empty document `open` made for a session that didn't open, removed, if it's still
/// what was made: nothing else has written it since.
fn unmake(held: &Held) {
    if file::read(&held.resolved).is_ok_and(|bytes| bytes == *held.text) {
        let _ = std::fs::remove_file(&held.resolved);
    }
}

/// The session's commands, in the order they were queued, until its queue ends: each
/// queued after it closed is told so.
fn serve<C: Kernel + 'static>(session: &Session<C>, jobs: &Receiver<Job<C>>) {
    while let Ok(job) = jobs.recv() {
        if session.closed() {
            job(Turn::Closed(session.id));
        } else {
            job(Turn::Run(session));
        }
    }
}

/// The file at `path` read (or, with `create`, made) and through the gate, claimed for
/// session `id`; what the session holds, `open`'s answer, and the cache of its directory.
fn open<P: Clients + 'static>(
    registry: &Registry<P>,
    id: SessionId,
    path: &Path,
    create: bool,
) -> Result<(Held, Opened, Cache), Problems> {
    let (resolved, text) = if create {
        made(registry, id, path)?
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
        claim(registry, FileId::of(&opened)?, id, path)?;
        (resolved, text)
    };
    let gated = Gated::of(&text);
    let cache = registry.cache(&file::cache_directory(&resolved));
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
    Ok((held, opened, cache))
}

/// A new, empty document made at `path`, claimed for session `id`: where it is through
/// links, and its text.
fn made<P: Clients + 'static>(
    registry: &Registry<P>,
    id: SessionId,
    path: &Path,
) -> Result<(PathBuf, Vec<u8>), Problems> {
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
    claim(registry, FileId::of(&made)?, id, path)?;
    Ok((resolved, text))
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

/// The file `file` claimed for session `id`; or, if another session holds or is opening
/// it, a refusal naming that session.
fn claim<P: Clients + 'static>(
    registry: &Registry<P>,
    file: FileId,
    id: SessionId,
    given: &Path,
) -> Result<(), Problems> {
    let mut book = lock(&registry.book);
    match book.files.get(&file) {
        Some(&holder) if holder != id => Err(Problems::from(
            Error::new(
                ErrorCode::InvalidInput,
                format!("{} is already open in session {holder}", shown(given)),
            )
            .with_parameter("path")
            .with_suggestion(format!(
                "use session {holder}, which this client opened, or close it first"
            )),
        )),
        _ => {
            book.files.insert(file, id);
            Ok(())
        }
    }
}

impl<P: Clients + 'static> Registry<P> {
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
    use crate::fake::FakeKernel;

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
        assert!(lock(&sessions.registry.book).files.is_empty());
    }
}
