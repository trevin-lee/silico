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
//! A file is open in one session a process (decision 22), by its path through links, so two
//! sessions never save one file over each other. At most [`MAX_SESSIONS`] are open at once.
//! A document the gate refuses still opens, holding its text and the gate's problems
//! (decision 21), so it can be mended. Session IDs are `s1`, `s2`, ..., in the order opens
//! were asked for, never reused within the process. Each session has its own client of the
//! shared kernel ([`Clients::client`]: its rebuilds never supersede another session's), and
//! sessions on documents in one directory share one rebuild cache, one value cloned, so its
//! bound counts every session's writes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::Problems;
use crate::cache::Cache;
use crate::document::{self, Checked, Document, FeatureId, FormatVersion, NamingScheme, Units};
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
    /// Whether a transaction is open: never yet, until transactions come (M4.2's PR 5).
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
/// thread, in turn, on its own client of `kernel`. Dropping it closes every session, waiting
/// for the command each is running.
pub struct Sessions<P: Clients + 'static> {
    registry: Arc<Registry<P>>,
}

/// What opens a directory's cache: [`Cache::open`], or a test's.
type CacheOpener = dyn Fn(&Path) -> Cache + Send + Sync;

struct Registry<P> {
    kernel: P,
    open_cache: Box<CacheOpener>,
    book: Mutex<Book>,
}

/// The registry's record of its sessions.
struct Book {
    /// The next session's number.
    next: u64,
    /// The sessions open.
    sessions: BTreeMap<SessionId, Arc<Session>>,
    /// The sessions opening: not open yet, but counted against [`MAX_SESSIONS`].
    opening: BTreeSet<SessionId>,
    /// Each file a session holds, or is opening, by its path through links.
    files: BTreeMap<PathBuf, SessionId>,
    /// Each directory's rebuild cache, by its path (`file::cache_directory`), cloned for
    /// each session on a document in it.
    caches: BTreeMap<PathBuf, Cache>,
    /// The threads of the sessions open or opening, joined when the registry is dropped.
    threads: BTreeMap<SessionId, JoinHandle<()>>,
    /// The sessions are being dropped: an open that finishes now doesn't open.
    stopping: bool,
}

/// One open session, shared by its thread and the callers that queue its commands.
struct Session {
    id: SessionId,
    held: Mutex<Held>,
    /// Where its commands are queued, in order: none once it's closed.
    queue: Mutex<Option<Sender<Job>>>,
}

/// What a session holds (docs/design/m4.1-command-api.md, section 3). Its values are shared
/// with the commands reading them, so a command takes them under the lock at once and works
/// off it, and `status` never waits on a command's work.
struct Held {
    /// The document's path as `open` was given it.
    path: PathBuf,
    /// The document's text: the file's bytes as read or written, until an edit.
    text: Arc<Vec<u8>>,
    /// The text as the reader and the gate found it.
    gated: Arc<Gated>,
    revision: Revision,
}

/// A document's text as the reader and the gate found it.
enum Gated {
    /// It doesn't read as a document: the reader's error.
    Unread(Problems),
    /// It reads, but the gate refuses it: the document, and every problem with it.
    Refused(Document, Problems),
    Checked(Checked),
}

impl Gated {
    /// `text` read and through the gate.
    fn of(text: &[u8]) -> Self {
        let _span = tracing::info_span!("gate", bytes = text.len()).entered();
        match document::read(text) {
            Err(error) => Gated::Unread(Problems::from(error)),
            Ok(read) => match Checked::try_new(read) {
                Ok(checked) => Gated::Checked(checked),
                Err((read, invalid)) => Gated::Refused(read, Problems::from(invalid)),
            },
        }
    }

    /// The document, if the text reads as one; or the reader's error.
    fn document(&self) -> Result<&Document, &Problems> {
        match self {
            Gated::Unread(problems) => Err(problems),
            Gated::Refused(document, _) => Ok(document),
            Gated::Checked(checked) => Ok(checked.document()),
        }
    }

    /// The reader's or the gate's problems: none if the gate took it.
    fn problems(&self) -> Problems {
        match self {
            Gated::Unread(problems) | Gated::Refused(_, problems) => problems.clone(),
            Gated::Checked(_) => Problems::of(Vec::new()),
        }
    }
}

/// A command queued on a session: run on its thread with the session, or, if the session
/// closed before its turn, told so.
type Job = Box<dyn FnOnce(Turn<'_>) + Send>;

enum Turn<'a> {
    Run(&'a Session),
    Closed(SessionId),
}

impl Session {
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
    fn queue(&self, job: Job) {
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
    /// later session sharing that value: for tests, a cache with a secret of their own.
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
    /// file (a regular file, no more than a document may be) and puts it through the gate.
    /// The session opens even if the gate refuses the document, holding it and the gate's
    /// problems (decision 21). With `create`, writes a new, empty document at `path` in
    /// canonical form first, refusing if anything is there. Refused if the file is open in
    /// another session of this process (by its path through links), or [`MAX_SESSIONS`]
    /// are open.
    pub fn open(&self, path: impl Into<PathBuf>, create: bool) -> Running<Opened> {
        let path = path.into();
        let mut book = lock(&self.registry.book);
        if book.stopping {
            return Running::answered(refused(closing()));
        }
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
        // Spawned with the book held, so the thread's handle is recorded before the thread
        // can look for it (an open that fails removes its own).
        let started = std::thread::Builder::new()
            .name(format!("session-{id}"))
            .spawn(move || {
                let started = {
                    let _caller = caller.enter();
                    let _span =
                        tracing::info_span!("session_command", session = %id, command = "open")
                            .entered();
                    start(registry, id, &path, create, &reply)
                };
                if let Some(Started {
                    session,
                    jobs,
                    engine,
                }) = started
                {
                    serve(&session, &jobs);
                    drop(engine);
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
        let registry = Arc::clone(&self.registry);
        self.queued(session, "close", move |session| {
            session.close();
            let mut book = lock(&registry.book);
            book.sessions.remove(&session.id);
            book.files.retain(|_, holder| *holder != session.id);
            // Its thread ends once this command answers; nothing need wait for it.
            book.threads.remove(&session.id);
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
    /// read as a document or has no such feature.
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
                    let found = document.features.iter().find(|f| f.id == id).ok_or_else(|| {
                        Error::new(
                            ErrorCode::InvalidInput,
                            format!("the document has no feature {id}"),
                        )
                        .with_parameter("feature")
                        .with_suggestion(
                            "name a feature by its ID, as the document's outline lists them",
                        )
                    })?;
                    Ok(document::write_feature(found)?)
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

    fn find(&self, session: &SessionId) -> Option<Arc<Session>> {
        lock(&self.registry.book).sessions.get(session).cloned()
    }

    /// `work` queued on `session`'s thread, under the caller's span and the command's.
    fn queued<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: &'static str,
        work: impl FnOnce(&Session) -> Result<T, Problems> + Send + 'static,
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
        for session in sessions.values() {
            session.close();
        }
        drop(sessions);
        for (_, thread) in threads {
            let _ = thread.join();
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

/// A session opened on its thread: what the thread serves.
struct Started<C: Kernel + 'static> {
    session: Arc<Session>,
    jobs: Receiver<Job>,
    /// Its rebuilds' engine, on its own client of the kernel.
    engine: Recompute<C>,
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
        // The session's own client of the kernel, and its directory's cache: its rebuilds
        // supersede only its own work, and keep their steps where the directory's other
        // documents find them. Held for the session's life; its rebuilds come with the
        // commands that read a model.
        let engine = Recompute::with_cache(registry.kernel.client(), cache);
        let (sender, jobs) = mpsc::channel();
        let session = Arc::new(Session {
            id,
            held: Mutex::new(held),
            queue: Mutex::new(Some(sender)),
        });
        let mut book = lock(&registry.book);
        book.opening.remove(&id);
        if book.stopping {
            return Err(Problems::from(closing()));
        }
        book.sessions.insert(id, Arc::clone(&session));
        Ok((session, opened, jobs, engine))
    });
    match opened {
        Ok((session, opened, jobs, engine)) => {
            let _ = reply.send(Response {
                session: Some(session.info()),
                result: Ok(opened),
            });
            // The session's thread doesn't keep the registry: a dropped one ends with its
            // kernel.
            drop(registry);
            Some(Started {
                session,
                jobs,
                engine,
            })
        }
        Err(problems) => {
            let mut book = lock(&registry.book);
            book.opening.remove(&id);
            book.files.retain(|_, holder| *holder != id);
            book.threads.remove(&id);
            drop(book);
            let _ = reply.send(Response {
                session: None,
                result: Err(problems),
            });
            None
        }
    }
}

/// The session's commands, in the order they were queued, until its queue ends: each
/// queued after it closed is told so.
fn serve(session: &Session, jobs: &Receiver<Job>) {
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
fn open<P: Clients>(
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
        claim(registry, &resolved, id, path)?;
        let text = {
            let _span = tracing::info_span!("read").entered();
            file::read(&resolved).map_err(|e| e.with_parameter("path"))?
        };
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
        text: Arc::new(text),
        gated: Arc::new(gated),
        revision: Revision::OPENED,
    };
    Ok((held, opened, cache))
}

/// A new, empty document made at `path`, claimed for session `id`: where it is through
/// links, and its text.
fn made<P: Clients>(
    registry: &Registry<P>,
    id: SessionId,
    path: &Path,
) -> Result<(PathBuf, Vec<u8>), Problems> {
    let name = path.file_name().ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("{} names no file to make", path.display()),
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
    claim(registry, &resolved, id, path)?;
    let text = document::write(&Document {
        silico: FormatVersion,
        units: Units,
        naming: NamingScheme,
        features: Vec::new(),
    })?
    .into_bytes();
    match create_new(&resolved, &text) {
        Ok(()) => Ok((resolved, text)),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(Problems::from(
            Error::new(
                ErrorCode::InvalidInput,
                format!("{} already exists", path.display()),
            )
            .with_parameter("create")
            .with_suggestion("open it without create, or give another path"),
        )),
        Err(e) => Err(Problems::from(uncreatable(e))),
    }
}

/// `text` written to a new file at `path`, never over one (nor through a link) already
/// there; removed again if it couldn't be written whole.
fn create_new(path: &Path, text: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let written = file.write_all(text).and_then(|()| file.sync_all());
    if written.is_err() {
        let _ = std::fs::remove_file(path);
    }
    written
}

/// The file at `resolved` (its path through links) claimed for session `id`; or, if another
/// session holds or is opening it, a refusal naming that session.
fn claim<P>(
    registry: &Registry<P>,
    resolved: &Path,
    id: SessionId,
    given: &Path,
) -> Result<(), Problems> {
    let mut book = lock(&registry.book);
    match book.files.get(resolved) {
        Some(&holder) if holder != id => Err(Problems::from(
            Error::new(
                ErrorCode::InvalidInput,
                format!("{} is already open in session {holder}", given.display()),
            )
            .with_parameter("path")
            .with_suggestion(format!(
                "use session {holder}, which this client opened, or close it first"
            )),
        )),
        _ => {
            book.files.insert(resolved.to_owned(), id);
            Ok(())
        }
    }
}

impl<P> Registry<P> {
    /// The cache in `dir`: the one value its sessions share, opened the first time.
    fn cache(&self, dir: &Path) -> Cache {
        if let Some(cache) = lock(&self.book).caches.get(dir) {
            return cache.clone();
        }
        // Opened without the book held (it reads the user's secret): should another session
        // have opened it meanwhile, its value is the one kept.
        let opened = (self.open_cache)(dir);
        lock(&self.book)
            .caches
            .entry(dir.to_owned())
            .or_insert(opened)
            .clone()
    }
}
