//! The registry of a process's sessions ([`Registry`], its [`Book`]): which sessions are
//! open, the files they claim (by path and by the file each is), their directories'
//! caches, and their threads; and opening a session, on its own thread, and serving its
//! commands.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, Weak};

#[cfg(doc)]
use super::MAX_SESSIONS;
use super::cancel::Ticket;
use super::holding::Holding;
use super::rebuilds::{Newest, Rebuilds};
use super::threads::{Reads, Threads};
use super::transaction::{Held, not_reloaded, of_file, put_back, too_long};
use super::{
    Gated, Job, Opened, Problems, Response, Revision, Session, SessionId, Turn, closing, shown_path,
};
use crate::cache::Cache;
use crate::command::edit::FeatureIds;
use crate::document::json::MAX_INPUT_LEN;
use crate::document::{self, Document, FormatVersion, NamingScheme, Units};
use crate::file::{self, Unresolved, Workspace};
use crate::kernel::{Clients, Kernel};
use crate::recompute::{Recompute, SketchSolvers};
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// What opens a directory's cache: [`Cache::open`], or a test's.
pub(super) type CacheOpener = dyn Fn(&Path) -> Cache + Send + Sync;

pub(super) struct Registry<P: Clients + 'static> {
    pub(super) kernel: P,
    pub(super) open_cache: Box<CacheOpener>,
    /// The sketch solvers every session's engine shares, as they share the kernel: the
    /// sketch worker, started when a sketch is first solved, or a test's stand-in.
    pub(super) sketches: SketchSolvers,
    /// Where `add_feature` takes the IDs of features given none.
    pub(super) feature_ids: Arc<Mutex<FeatureIds>>,
    pub(super) book: Mutex<Book<P::Client>>,
    /// What draws `render`'s scenes: the render worker's client, given when the sessions are
    /// made ([`Sessions::rendering`](super::Sessions::rendering)), or none (refused).
    pub(super) renderer: Mutex<Option<Arc<dyn crate::command::render::Renderer>>>,
    /// The display meshes `render` makes, shared by the sessions of this kernel.
    pub(super) meshes: Arc<crate::meshes::MeshCache<crate::recompute::Key>>,
    /// The workspace every path a session is given, and every file it reads, writes, or
    /// keeps its cache in, must lie in (decision 26): given as the sessions are made
    /// ([`Sessions::in_workspace`](super::Sessions::in_workspace)), fixed for their life.
    /// None for the CLI's, whose paths resolve as the system resolves them.
    pub(super) workspace: Option<Workspace>,
}

/// A file as the system knows it, whatever path reaches it: its device and inode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct FileId {
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
pub(super) struct Book<C: Kernel + 'static> {
    /// The next session's number.
    pub(super) next: u64,
    /// The sessions open.
    pub(super) sessions: BTreeMap<SessionId, Arc<Session<C>>>,
    /// The sessions opening: not open yet, but counted against [`MAX_SESSIONS`], and each
    /// holding the commands sent to it before it opened, to run once it has, in the order
    /// they were sent.
    pub(super) opening: BTreeMap<SessionId, Opening<C>>,
    /// Each file a session holds, or is opening, as the system knows it, and the file kept
    /// open while it's claimed: an open file's inode isn't given to another (as ext4 gives a
    /// freed one at once), so a claim never comes to name some other file. A test can't make
    /// the system reuse an inode on demand; holding the file is what guarantees it.
    pub(super) files: BTreeMap<FileId, (SessionId, std::fs::File)>,
    /// Each file a session holds, or is opening, by its path as its links resolve: a file
    /// replaced at its path (an editor's or git's save, a rename over it) is a new inode
    /// there, and still the session's document. Saves (the design's section 4) and reloads
    /// (decision 2) that put a new file at the path must claim its new inode too.
    pub(super) paths: BTreeMap<PathBuf, SessionId>,
    /// Each directory's rebuild cache, by its path (`file::cache_directory`), cloned for
    /// each session on a document in it.
    pub(super) caches: BTreeMap<PathBuf, Cache>,
    /// Every session thread not yet joined: those open, opening, or closed and ending,
    /// joined once they've ended (at the next open) or when the registry is dropped, so
    /// none outlives it.
    pub(super) threads: Threads,
    /// The threads of sessions released (closed, or their threads ended): their published
    /// reads and the threads watching their rebuilds, each session's moved here as it's
    /// released, so the drop joins them too.
    pub(super) released: Threads,
    /// The sessions are being dropped: an open that finishes now doesn't open.
    pub(super) stopping: bool,
}

impl<C: Kernel + 'static> Book<C> {
    /// Session `id` gone from every record but its thread's: open, opening, and the file
    /// it held; its published reads and its rebuilds' watchers now the book's to join, and
    /// no read started after.
    pub(super) fn release(&mut self, id: SessionId) {
        if let Some(session) = self.sessions.remove(&id) {
            if let Some(reads) = lock(&session.reads).threads.take() {
                self.released.extend(reads);
            }
            let watchers = std::mem::replace(&mut lock(&session.rebuilds).watchers, Threads::new());
            self.released.extend(watchers);
        }
        self.opening.remove(&id);
        self.files.retain(|_, (holder, _)| *holder != id);
        self.paths.retain(|_, holder| *holder != id);
    }

    /// The threads that have ended, joined.
    pub(super) fn reap(&mut self) {
        self.threads.reap();
        self.released.reap();
    }
}

/// A session opening ([`Book::opening`]): from when its `open` is called, a command naming it
/// is held here, behind the open, rather than refused as naming no session open (the design's
/// section 10: commands reach a session in the order sent, and nothing in a result depends on
/// how the threads ran). If it opens, they run as if sent after the open had answered; if
/// not, each is refused, `invalid_input` "no session sN is open", naming `session`, exactly
/// as a command sent after the refusal is, whatever the open came to: so whether a command
/// reached the session before its open was refused or after changes nothing in its answer.
pub(super) struct Opening<C: Kernel + 'static> {
    /// Its queue, made with the open: queued commands sent now wait here, and the session,
    /// once open, serves it, so they run first, in the order sent.
    pub(super) queue: Sender<Job<C>>,
    /// How far its newest rebuild has got, which its queued commands' handles read: the
    /// session's own once it opens.
    pub(super) newest: Arc<Newest>,
    /// Its published reads sent now, in the order sent: each started as the session opens,
    /// beside its queue, as one sent then would be.
    pub(super) reads: Vec<Job<C>>,
}

/// Removes its session from the registry's records when dropped: when the session's thread
/// ends, however it ends.
pub(super) struct Release<P: Clients + 'static> {
    pub(super) registry: Weak<Registry<P>>,
    pub(super) id: SessionId,
}

impl<P: Clients + 'static> Drop for Release<P> {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            lock(&registry.book).release(self.id);
        }
    }
}

/// A session opened on its thread: what the thread serves.
pub(super) struct Started<C: Kernel + 'static> {
    pub(super) session: Arc<Session<C>>,
    pub(super) jobs: Receiver<Job<C>>,
}

/// The open, on its session's thread, answered through `reply`: if it opened, what the
/// thread serves, `jobs`, its queue, the commands sent while it opened first. Cancelled
/// (`ticket`) before the session is recorded open, it answers `cancelled`, leaving no
/// session and no file it made. If it doesn't open, every command sent to it while it
/// opened is refused as naming no session open ([`Turn::Closed`]), once the open has
/// answered.
pub(super) fn start<P: Clients + 'static>(
    registry: Arc<Registry<P>>,
    id: SessionId,
    path: &str,
    create: bool,
    ticket: &Ticket,
    reply: &mpsc::SyncSender<Response<Opened>>,
    jobs: Receiver<Job<P::Client>>,
) -> Option<Started<P::Client>> {
    let opened = open(&registry, id, path, create).and_then(|(held, opened, cache, made)| {
        let mut book = lock(&registry.book);
        if book.stopping {
            // The file it made, if it made one, goes with it (`made`'s drop).
            return Err(Problems::from(closing()));
        }
        // Recorded opening as it was asked for, and only released by this thread.
        let Some(opening) = book.opening.get(&id) else {
            return Err(Problems::from(closing()));
        };
        let (queue, newest) = (opening.queue.clone(), Arc::clone(&opening.newest));
        // From here it opens, unless it was cancelled first (the file it made goes as above).
        ticket.settle()?;
        let session = Arc::new(Session {
            id,
            workspace: registry.workspace.clone(),
            held: Holding::new(held),
            queue: Mutex::new(Some(queue)),
            rebuilds: Mutex::new(Rebuilds {
                engine: registry.engine(cache.clone()),
                cache,
                running: None,
                published: None,
                ended: None,
                kept: None,
                floor: Revision::OPENED,
                candidate: None,
                watchers: Threads::new(),
                last_number: 0,
            }),
            rebuilt: Condvar::new(),
            reads: Mutex::new(Reads::new()),
            command: Mutex::new(None),
            newest,
            #[cfg(test)]
            gate: super::Gate::default(),
            #[cfg(test)]
            settled: super::Gate::default(),
        });
        // Open from here: the commands sent while it opened are in its queue, ahead of any
        // sent after, and its published reads sent then start now, in the order sent,
        // before any sent after can.
        let reads = book
            .opening
            .remove(&id)
            .map(|opening| opening.reads)
            .unwrap_or_default();
        book.sessions.insert(id, Arc::clone(&session));
        for read in reads {
            read(Turn::Run(&session));
        }
        if let Some(made) = made {
            made.keep();
        }
        Ok((session, opened))
    });
    match opened {
        Ok((session, opened)) => {
            let _ = reply.send(Response {
                session: Some(session.info(false)),
                model: None,
                result: Ok(opened),
            });
            Some(Started { session, jobs })
        }
        Err(problems) => {
            let opening = {
                let mut book = lock(&registry.book);
                let opening = book.opening.remove(&id);
                book.release(id);
                opening
            };
            // Settled, so a cancel after the refusal says it came too late; `cancelled`, if
            // one came first.
            let _ = reply.send(ticket.answer(
                || None,
                || Response {
                    session: None,
                    model: None,
                    result: Err(problems),
                },
            ));
            // Then each command sent while it opened, refused as naming no session open, as
            // one sent from here is: the queue's last sender let go with its record, so none
            // comes after these.
            if let Some(Opening { queue, reads, .. }) = opening {
                drop(queue);
                for read in reads {
                    read(Turn::Closed(id));
                }
            }
            while let Ok(job) = jobs.recv() {
                job(Turn::Closed(id));
            }
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
pub(super) fn serve<C: Kernel + 'static>(session: &Arc<Session<C>>, jobs: &Receiver<Job<C>>) {
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
    let workspace = registry.workspace.as_ref();
    let (resolved, text, made) = if create {
        let (resolved, text, made) = made(registry, id, path)?;
        (resolved, text, Some(made))
    } else {
        let resolved = file::resolve(workspace, path).map_err(|unresolved| match unresolved {
            Unresolved::Outside => Workspace::outside(path, "path"),
            Unresolved::Unreachable(e) => file::unreadable(e).with_parameter("path"),
        })?;
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
        session: id,
        path: given.to_owned(),
        created: create,
        features: gated.document().ok().map(|d| d.features.len()),
        bodies: gated
            .document()
            .map(|d| crate::command::bodies(d).into_iter().cloned().collect())
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

/// A new, empty document made at `path` (relative to the workspace, if there's one: its
/// directory refused if it's outside it), claimed for session `id`: where it is through
/// links, its text, and the file made, removed again unless kept.
fn made<P: Clients + 'static>(
    registry: &Registry<P>,
    id: SessionId,
    path: &Path,
) -> Result<(PathBuf, Vec<u8>, MadeFile), Problems> {
    let _span = tracing::info_span!("create").entered();
    // A path ending in `/` or `/.` names a directory, as `open(2)` reads it, though
    // `Path::file_name` skips the slash and the `.` (`brand/.` would make `brand`).
    let bytes = path.as_os_str().as_bytes();
    let directory = bytes.ends_with(b"/") || bytes.ends_with(b"/.");
    let name = path.file_name().filter(|_| !directory).ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("{} names no file to make", shown_path(path)),
        )
        .with_parameter("path")
        .with_suggestion("give the path of the document to make, ending in its file's name")
    })?;
    let uncreatable = |e: std::io::Error| {
        Error::new(ErrorCode::IoFailed, format!("couldn't be made: {e}"))
            .with_parameter("path")
            .with_suggestion("check that its directory exists, and that this user may write in it")
    };
    let workspace = registry.workspace.as_ref();
    let joined = file::joined(workspace, path);
    let resolved = file::resolve(workspace, file::directory(&joined))
        .map_err(|unresolved| match unresolved {
            Unresolved::Outside => Workspace::outside(path, "path"),
            Unresolved::Unreachable(e) => uncreatable(e),
        })?
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
                    format!("{} already exists", shown_path(path)),
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
pub(super) fn claim<P: Clients + 'static>(
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
pub(super) enum Unclaimed {
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
                format!("{} is already open in session {holder}", shown_path(given)),
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
    pub(super) fn rereading(self, path: &str) -> Error {
        let named = shown_path(Path::new(path));
        match self {
            Unclaimed::Held(holder) => not_reloaded(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!("{named} is already open in session {holder}"),
                ),
                format!(
                    "put this session's file back at {named}: the file there now is session {holder}'s"
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
pub(super) fn reclaim<P: Clients + 'static>(
    registry: &Weak<Registry<P>>,
    id: SessionId,
    resolved: &Path,
) {
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
    /// `cache`, and solving sketches with the registry's solvers, which every session
    /// shares.
    fn engine(&self, cache: Cache) -> Recompute<P::Client> {
        Recompute::with_cache(self.kernel.client(), cache).with_solvers(self.sketches.clone())
    }

    /// The cache in `dir`: the one value its sessions share, opened the first time; one
    /// without the user's secret, which keeps nothing, isn't kept, so the next open tries
    /// again. In a workspace, every cache it gives is [`within`](Cache::within) it, so each
    /// of its reads, writes, evictions, and clears checks that it still leads nowhere out of
    /// the workspace (a link put on its way after this, in a transaction that reads nothing
    /// again, say).
    fn cache(&self, dir: &Path) -> Cache {
        let within = |cache: Cache| match &self.workspace {
            Some(workspace) => cache.within(workspace.clone()),
            None => cache,
        };
        // A cache directory that leads out of the workspace (a link put there) is never
        // opened, the user's secret not read for it: a cache that keeps nothing and reads
        // nothing stands in for it, as the cache keeps none through a link (M2.5). It
        // changes no result, so the document still opens. Its clear checks again as it
        // clears, as every cache in the workspace does.
        if let Some(workspace) = &self.workspace
            && let Err(Unresolved::Outside) = file::resolve(Some(workspace), dir)
        {
            tracing::warn!(
                dir = %dir.display(),
                "the cache directory leads out of the workspace: keeping no results"
            );
            return within(Cache::unsealed(dir));
        }
        if let Some(cache) = lock(&self.book).caches.get(dir) {
            return cache.clone();
        }
        // Opened without the book held (it reads the user's secret): should another session
        // have opened it meanwhile, its value is the one kept.
        let opened = within((self.open_cache)(dir));
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
