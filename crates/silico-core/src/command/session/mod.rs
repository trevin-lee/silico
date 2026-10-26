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
//! file over each other. At most [`MAX_SESSIONS`] are open at once. A document the gate
//! refuses still opens, holding its text and the gate's problems (decision 21), so it can
//! be mended; a file longer than Silico reads doesn't, since a session couldn't hold its
//! text whole. Session IDs are `s1`, `s2`, ..., in the order opens were asked for, never
//! reused within the process. Each session has its own client of the shared kernel
//! ([`Clients::client`]: its rebuilds never supersede another session's), and sessions on
//! documents in one directory share one rebuild cache, one value cloned, so its bound
//! counts every session's writes.
//!
//! A session's document changes in transactions (the design's section 4):
//! [`Sessions::begin`] keeps the session's state, [`Sessions::commit`] saves what the
//! transaction's edits changed through [`file::save_over`], writing new fingerprints and
//! solves only for the features they reached (decision 28), and [`Sessions::rollback`]
//! restores what `begin` kept, the text byte for byte and the model `begin` saw, at a new
//! revision whose state equals the old. One at a time, never nested; a session isn't closed
//! while one is open, and a client that goes away (the sessions dropped) leaves its file as
//! it was. An edit made outside a transaction ([`Sessions::add_feature`],
//! [`Sessions::set_field`], [`Sessions::remove_feature`], [`Sessions::replace_text`]) is
//! saved as it's made, as its own transaction (decision 1): so only a commit, such an edit,
//! or [`Sessions::format`], which accepts drift by name (decision 31), writes the document.
//!
//! A file can change under a session (decision 2): before each queued command outside a
//! transaction, the session reads its file again and compares its bytes with those it last
//! read or wrote, and reloads a change at a new revision, the response's `reloaded` saying
//! so; a file gone, unreadable, or another session's refuses the command. The reload is the
//! session's own work, not the command's: it stands whatever the command comes to, so a
//! command cancelled after it answers `cancelled` with `reloaded` true and the new
//! revision. Inside a transaction, only `commit` reads the file, and a change (or a file
//! gone) is its conflict: every other command runs on what the session holds. `close`,
//! `status`, and published reads never read the file.
//!
//! A session's path is UTF-8, so every response names it exactly: `open` refuses one that
//! isn't.
//!
//! A command can be stopped (decisions 10 and 34): through its handle ([`Running::cancel`],
//! MCP's cancellation), or by [`Sessions::cancel`], naming the command running. Either way
//! it stops before its work leaves a trace and answers `cancelled`, or, if it had done its
//! work (an edit saved), answers with its real result: what the session holds is changed
//! only once the command's stop is settled (`holding`), and its file written only so too. A
//! kernel fault (a worker that crashed, hung, or couldn't be had) is given once, beside a
//! command's result, in the model it read ([`ModelRead::fault`]), never among the
//! document's problems, and kept as its revision's outcome: no read of that revision
//! rebuilds it, and a save writes around it, saving its features as failed ones are
//! (decision 32).
//!
//! What it doesn't bound: the caches, one kept for each directory a session has opened a
//! document in, for the process's life; and each session's queue, which holds every command
//! queued and not yet run. Nor does it choose which files may be opened: `open` and `text`
//! read any file this user may read, so a server for clients it doesn't trust gives them a
//! workspace (decision 26) once there is one.

mod cancel;
mod edits;
mod holding;
mod rebuilds;
mod registry;
mod threads;
mod transaction;
mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, Weak};

use super::edit::{self, FeatureIds};
use super::{Gated, Problems, PublishedRead, SessionCommand};
use crate::cache::Cache;
use crate::document::json::Node;
use crate::document::{self, FeatureId};
#[cfg(doc)]
use crate::file;
use crate::file::shown_path;
use crate::kernel::{Clients, Kernel};
use crate::recompute::SketchSolvers;
use crate::sketch::SketchSolver;
use crate::worker::lock;
use crate::{Error, ErrorCode};

use cancel::{Interrupt, Ticket, Why, not_running};
use edits::{as_committed, not_given_back};
use holding::Holding;
use rebuilds::{Newest, Rebuilds, Unbuilt, published_answer};
pub(super) use rebuilds::{kept_fault, kept_suggestion, retried};
use registry::{Book, CacheOpener, Registry, Release, Started, claim, reclaim, serve, start};
pub use threads::MAX_PUBLISHED_READS;
use threads::{Counted, Reads, Threads};
use transaction::{Claim, Held, Reload, Rereads, transaction_open};
pub use transaction::{NOT_RELOADED, TOO_LONG};
pub(crate) use types::SESSION_ID_PATTERN;
pub use types::{
    Added, Begun, Cancelled, CheckAnswer, Closed, Committed, FieldSet, ModelChoice, ModelRead,
    Opened, OutlineAnswer, Progress, Published, Read, ReadClient, Rebuild, RebuildState,
    Reformatted, Removed, Replaced, Response, Revision, RolledBack, Running, SessionId,
    SessionInfo, Status, Stopper, TextAnswer,
};
use types::{READ_STOPPED, SESSION_STOPPED};

/// The most sessions one process holds open at once (decision 18): each may start a sketch
/// worker and keep its models' B-reps, so the limit is what bounds their memory.
pub const MAX_SESSIONS: usize = 16;

/// A process's open sessions, on one kernel: each session's commands are run on its own
/// thread, in turn, on its own client of `kernel`. Dropping it closes every session,
/// stopping its rebuild and leaving its file as it was (an open transaction is rolled back:
/// only a commit or `format` writes), and waits for each session's thread to end, then for
/// the threads watching its rebuilds and running its published reads, a session's closed
/// before the drop too (a read blocked in its own code keeps the drop waiting until it
/// returns). A rebuild's own recompute thread isn't waited for: its engine, dropped with
/// its session, stops it, and it may end a little after (naming a model's elements once its
/// rebuild has answered).
pub struct Sessions<P: Clients + 'static> {
    registry: Arc<Registry<P>>,
}

/// One open session, shared by its thread, the callers that queue its commands, and the
/// registry.
///
/// Its locks, and the registry's, are taken in one order, each let go before one earlier in
/// the order is taken: the registry's book, then a session's published reads (then those
/// unanswered), then its held state, then its rebuilds, then its queue, then the command
/// running, under which none is taken. The registry's `feature_ids` is taken alone.
struct Session<C: Kernel + 'static> {
    id: SessionId,
    held: Holding,
    /// Where its commands are queued, in order: none once it's closed.
    queue: Mutex<Option<Sender<Job<C>>>>,
    /// Its rebuilds, under a lock of their own, never `held`'s, so `status` never waits
    /// behind a rebuild's start; and reachable from outside its thread, so a rebuild can be
    /// stopped while a command waits on it.
    rebuilds: Mutex<Rebuilds<C>>,
    /// A rebuild ended: told to the command waiting on it.
    rebuilt: Condvar,
    /// Its reads: the published ones' threads, and every read's kernel client.
    reads: Mutex<Reads>,
    /// The queued command running, if one is: its stop, which `cancel` finds here.
    command: Mutex<Option<Arc<Ticket>>>,
    /// How far its newest rebuild has got, under a lock of its own: what `status` and each
    /// command's handle read.
    newest: Arc<Newest>,
    /// Where its commands wait as each starts, in this crate's tests only.
    #[cfg(test)]
    gate: Gate,
}

/// In this crate's tests only: where a session's commands, queued and published reads,
/// wait as each starts (its stop recorded, its work not begun), once a test has shut it:
/// each says it started, then waits until the test lets it through. Its lock is taken
/// alone.
#[cfg(test)]
#[derive(Default)]
struct Gate(Mutex<Option<Shut>>);

/// A shut [`Gate`]'s ends.
#[cfg(test)]
#[derive(Clone)]
struct Shut {
    /// Told as each command starts.
    started: Sender<()>,
    /// What each waits on until the test lets it through (or goes).
    through: Arc<Mutex<mpsc::Receiver<()>>>,
}

#[cfg(test)]
impl Gate {
    /// Shut: each command that starts says so on `started`, then waits for one word on
    /// `through`.
    fn shut(&self, started: Sender<()>, through: mpsc::Receiver<()>) {
        *lock(&self.0) = Some(Shut {
            started,
            through: Arc::new(Mutex::new(through)),
        });
    }

    fn pass(&self) {
        let shut = lock(&self.0).clone();
        if let Some(Shut { started, through }) = shut {
            let _ = started.send(());
            let _ = lock(&through).recv();
        }
    }
}

/// A command waiting on a session's rebuild is woken by its cancel, to stop it.
impl<C: Kernel + 'static> Interrupt for Session<C> {
    fn interrupt(&self, _why: Why) {
        // Under the rebuilds' lock, so a waiter that saw no cancel before it waited is
        // waiting when it's told.
        let _rebuilds = lock(&self.rebuilds);
        self.rebuilt.notify_all();
    }
}

/// A command queued on a session: run on its thread with the session, or, if the session
/// closed before its turn, told so.
type Job<C> = Box<dyn FnOnce(Turn<'_, C>) + Send>;

enum Turn<'a, C: Kernel + 'static> {
    Run(&'a Arc<Session<C>>),
    Closed(SessionId),
}

impl<C: Kernel + 'static> Session<C> {
    /// The session as it is now, `reloaded` saying whether it reloaded its file before the
    /// command.
    fn info(&self, reloaded: bool) -> SessionInfo {
        self.info_of(&self.held.read(), reloaded)
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
        let held = self.held.read();
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

    /// Ends its queue: commands queued after it are told the session closed. And its
    /// published reads interrupted as closed ([`Why::Closed`]), each running now and, since
    /// the session's record of them stays closed, each counted after, at once: so a read's
    /// own kernel client is retired however late it's added, and the read answers
    /// as of a session that closed, not `cancelled`. The command running on its thread is
    /// interrupted as closed too, so a current read's kernel work stops with the session
    /// (`Sessions`' drop closes every session); a queued `close` running this is that
    /// command itself, which closing does nothing to.
    fn close(&self) {
        lock(&self.queue).take();
        let unanswered = Arc::clone(&lock(&self.reads).unanswered);
        let running = lock(&unanswered).close();
        for ticket in running {
            ticket.close();
        }
        // Taken last and alone, as `Session` orders its locks: let go before the ticket's
        // interrupts take theirs (the robustness review: an `if let`'s guard lives through
        // its block).
        let running = lock(&self.command).clone();
        if let Some(ticket) = running {
            ticket.close();
        }
    }

    fn closed(&self) -> bool {
        lock(&self.queue).is_none()
    }

    /// `close`'s work, as the command `stop` stops: its queue ended, unless a transaction is
    /// open (commit or roll back first), or the close was cancelled first.
    fn shut(&self, stop: &Ticket) -> Result<(), Problems> {
        let held = self.held.read();
        if held.transaction.is_some() {
            return Err(Problems::from(transaction_open(
                self.id,
                "close the session",
            )));
        }
        // Settled with no transaction open: from here it closes. Let go first: `close`
        // takes the published reads' locks (the reads', then those unanswered), which come
        // before the held state's.
        drop(held.changing(stop)?);
        self.close();
        Ok(())
    }

    /// Its published reads running, in the order they started.
    fn reads_running(&self) -> Vec<PublishedRead> {
        let reads = lock(&self.reads);
        lock(&reads.unanswered).commands().collect()
    }

    /// The stop of the queued command running, if one is: for `status` and `cancel`, never
    /// for a command's own work, which is handed its own stop.
    fn running(&self) -> Option<Arc<Ticket>> {
        lock(&self.command).clone()
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
    /// later session sharing that value. A cache without the user's secret, which keeps
    /// nothing, isn't kept, so the next open in its directory tries again. For tests, a
    /// cache with a secret of their own; for a client that rebuilds nothing (`silico
    /// show`), one that keeps nothing and never reads the user's secret
    /// ([`Cache::unsealed`]).
    pub fn with_cache_opener(
        kernel: P,
        open: impl Fn(&Path) -> Cache + Send + Sync + 'static,
    ) -> Self {
        Self::build(kernel, Box::new(open), SketchSolvers::workers())
    }

    /// Sessions on `kernel` with stand-ins, for tests: each directory's cache opened with
    /// `open`, as [`with_cache_opener`](Self::with_cache_opener) does, and sketches solved
    /// with what `start` starts (`fake::FakeSketches`), rather than the sketch worker: the
    /// solvers every session shares, `start` called when none is idle
    /// ([`SketchSolvers`]).
    pub fn with_stand_ins(
        kernel: P,
        open: impl Fn(&Path) -> Cache + Send + Sync + 'static,
        start: impl Fn() -> Result<Box<dyn SketchSolver>, Error> + Send + Sync + 'static,
    ) -> Self {
        Self::build(kernel, Box::new(open), SketchSolvers::starting(start))
    }

    fn build(kernel: P, open_cache: Box<CacheOpener>, sketches: SketchSolvers) -> Self {
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
                    released: Threads::new(),
                    stopping: false,
                }),
                renderer: Mutex::new(None),
                meshes: Arc::new(crate::meshes::MeshCache::new(
                    crate::meshes::MESH_CACHE_BYTES,
                )),
            }),
        }
    }

    /// The same sessions, drawing `render`'s scenes with `renderer` (the render worker's
    /// client, [`crate::render::RenderClient`], or a test's fake), as they're given their
    /// kernel. Without one, `render` is refused as `worker_unavailable`.
    #[must_use]
    pub fn rendering(self, renderer: impl crate::command::render::Renderer + 'static) -> Self {
        *lock(&self.registry.renderer) = Some(Arc::new(renderer));
        self
    }

    /// The renderer `render` draws with, and the meshes it keeps.
    pub(crate) fn render_on(
        &self,
    ) -> (
        Option<Arc<dyn crate::command::render::Renderer>>,
        Arc<crate::meshes::MeshCache<crate::recompute::Key>>,
    ) {
        (
            lock(&self.registry.renderer).clone(),
            Arc::clone(&self.registry.meshes),
        )
    }

    /// The documents the sessions hold, by their paths: what a file a command writes is
    /// never (decision 35).
    pub(crate) fn held_paths(&self) -> Vec<PathBuf> {
        lock(&self.registry.book).paths.keys().cloned().collect()
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
                            shown_path(Path::new(&path))
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
        let ticket = Ticket::opening();
        let stop = Arc::clone(&ticket);
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
                    start(registry, id, &path, create, &stop, &reply)
                };
                if let Some(Started { session, jobs }) = started {
                    serve(&session, &jobs);
                }
            });
        match started {
            Ok(thread) => {
                book.opening.insert(id);
                book.threads.push(thread);
                Running::new(answer, SESSION_STOPPED, Some(ticket), None)
            }
            Err(e) => Running::answered(refused(crate::worker::thread_error("session", &e))),
        }
    }

    /// Closes `session`, once the commands queued before it have run: commands queued
    /// after it are told the session closed. Its file is then free to open again. Refused
    /// while a transaction is open (commit or roll back first), so nothing is dropped
    /// without the client saying so. It never reads the file (decision 2), so a file gone or
    /// changed never keeps it open. It doesn't cancel the command ahead of it: commands run
    /// in the order they arrive, each to its end (the design's section 3), and stopping one
    /// is said by naming it ([`cancel`](Self::cancel)).
    pub fn close(&self, session: &SessionId) -> Running<Closed> {
        let registry = Arc::downgrade(&self.registry);
        self.queued_with(
            session,
            SessionCommand::Close,
            Rereads::Never,
            move |session, stop, _, _| {
                session.shut(stop)?;
                if let Some(registry) = registry.upgrade() {
                    lock(&registry.book).release(session.id);
                }
                Ok(Closed)
            },
        )
    }

    /// `session`'s status, answered on the caller's thread from what the session holds,
    /// even while one of its commands runs: it never reads the file (decision 2). Besides
    /// what it holds, the queued command running, the published reads running, and its
    /// current revision's rebuild: `faulted` if its model holds a kernel fault, kept as the
    /// revision's outcome (decision 32), `stopped` if it was cancelled.
    pub fn status(&self, session: &SessionId) -> Response<Status> {
        let Some(found) = self.find(session) else {
            return refused(not_open(*session));
        };
        let reads = found.reads_running();
        let running = found.running().and_then(|ticket| ticket.command());
        let held = found.held.read();
        let gated = matches!(*held.gated, Gated::Checked(_));
        let rebuild = if gated {
            let state = lock(&found.rebuilds).state(held.revision);
            Rebuild {
                progress: found.newest.of(held.revision),
                ..state
            }
        } else {
            Rebuild {
                revision: held.revision,
                state: RebuildState::Idle,
                progress: None,
            }
        };
        Response {
            session: Some(found.info_of(&held, false)),
            model: None,
            result: Ok(Status {
                features: held.gated.document().ok().map(|d| d.features.len()),
                gated,
                transaction: held.transaction.as_ref().map(|begun| Begun {
                    base: begun.revision,
                }),
                running,
                reads,
                rebuild,
            }),
        }
    }

    /// Stops `session`'s command named `command` (decision 34), answered at once on the
    /// caller's thread, never queued and never reading the file: the queued command, if
    /// it's the one running, and every published read of that name running. Each stops as
    /// [`Running::cancel`] stops it, and answers too: `cancelled` if it left no trace, its
    /// real result if it had done its work. The answer says which (`stopped`: whether any
    /// was stopped before it had). Refused, naming `command`, if no command of that name is
    /// running, what is given as candidates: so a cancel that comes late never stops the
    /// command queued after the one it meant.
    pub fn cancel(&self, session: &SessionId, command: SessionCommand) -> Response<Cancelled> {
        let _span = tracing::info_span!("session_command", session = %session, command = "cancel")
            .entered();
        let Some(found) = self.find(session) else {
            return refused(not_open(*session));
        };
        let queued = found.running();
        let reads: Vec<Arc<Ticket>> = {
            let reads = lock(&found.reads);
            lock(&reads.unanswered).tickets().cloned().collect()
        };
        let named: Vec<&Arc<Ticket>> = queued
            .iter()
            .chain(&reads)
            .filter(|ticket| ticket.command() == Some(command))
            .collect();
        let result = if named.is_empty() {
            // Each, as `status` lists them.
            let running: Vec<SessionCommand> = queued
                .iter()
                .chain(&reads)
                .filter_map(|ticket| ticket.command())
                .collect();
            Err(Problems::from(not_running(found.id, command, &running)))
        } else {
            // Every one stopped, none passed over once one is.
            let stopped = named
                .iter()
                .fold(false, |stopped, ticket| ticket.cancel() | stopped);
            tracing::info!(command = command.as_str(), stopped, "cancelled");
            Ok(Cancelled { command, stopped })
        };
        Response {
            session: Some(found.info(false)),
            model: None,
            result,
        }
    }

    /// The text `session`'s document is: the file's bytes exactly, until an edit; or, given
    /// a `feature`, that feature's canonical text ([`document::write_feature`]). Refused if
    /// the text isn't UTF-8 (the reader refused it), or, for a feature, if the text doesn't
    /// read as a document or hasn't one feature of that ID.
    pub fn text(&self, session: &SessionId, feature: Option<FeatureId>) -> Running<TextAnswer> {
        self.queued(session, SessionCommand::Text, move |session, _, _| {
            let (text, gated) = session.document();
            let text = match feature {
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
            }?;
            Ok(TextAnswer { text })
        })
    }

    /// `session`'s document's problems ([`super::check`]) from the model `choice` names
    /// (decision 33), and its warnings (none yet).
    ///
    /// The current revision's: the gate's problems, if it refused the document, or, from the
    /// session's current revision rebuilt (waited for, and started if it hasn't been:
    /// decision 13), each failed feature's and each drift of a feature no edit reached since
    /// the last save (decision 28: a reached feature's stored fingerprint was measured from
    /// other inputs, and a commit writes its new one).
    ///
    /// Or the newest published model's, answered at once from what the session's rebuilds
    /// have published, never waiting for a rebuild or queueing behind its commands, as every
    /// published read is ([`read_model`](Self::read_model)): its revision in the response's
    /// `model`, which may be an older one's. Its drift leaves out what the session's edits
    /// reached at that model's revision, kept with it ([`Read::reached`]): exactly that
    /// revision's, whatever edits came since. With nothing published, it's refused as every
    /// published read is: naming `model`, or, for a document the gate refused, with the
    /// gate's problems.
    ///
    /// Either way, a kernel fault (a worker that crashed or hung) isn't the document's: it's
    /// given once, beside them, in the response's `model` ([`ModelRead::fault`]), and kept as
    /// the revision's outcome, so the next check of the revision reads the same model
    /// (decision 32).
    pub fn check(&self, session: &SessionId, choice: ModelChoice) -> Running<CheckAnswer> {
        let answer = |problems| CheckAnswer {
            problems,
            warnings: Problems::none(),
        };
        match choice {
            ModelChoice::Current => self.queued(
                session,
                SessionCommand::Check,
                move |session, stop, model| match session.current(stop) {
                    Ok(current) => {
                        *model = Some(current.read);
                        let _span = tracing::info_span!("check").entered();
                        Ok(answer(super::check_except(
                            &current.model,
                            current.reached.get(),
                        )))
                    }
                    Err(Unbuilt::Gated(problems)) => Ok(answer(problems)),
                    Err(Unbuilt::Unavailable(problems)) => Err(problems),
                },
            ),
            ModelChoice::Published => self.read_model(
                session,
                PublishedRead::Check,
                ModelChoice::Published,
                move |read| {
                    let _span = tracing::info_span!("check").entered();
                    Ok(answer(super::check_except(read.model(), read.reached())))
                },
            ),
        }
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
        command: PublishedRead,
        choice: ModelChoice,
        read: impl FnOnce(Read<'_, P::Client>) -> Result<T, Problems> + Send + 'static,
    ) -> Running<T> {
        // The read's own client, made as it's asked for and dropped when it returns, its stop
        // able to retire it if it's cancelled or its session closes first.
        let kernel: Arc<ReadClient<P::Client>> =
            Arc::new(ReadClient::new(self.registry.kernel.client()));
        match choice {
            ModelChoice::Current => {
                self.queued(session, command.into(), move |session, stop, model| {
                    stop.interrupt_with(Arc::downgrade(&kernel) as Weak<dyn Interrupt>);
                    let current = session.current(stop).map_err(Unbuilt::problems)?;
                    *model = Some(current.read.clone());
                    read(Read {
                        model: &current.model,
                        read: current.read,
                        stop,
                        kernel: &kernel,
                        reached: &current.reached,
                    })
                })
            }
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
                if lock(unanswered).len() >= MAX_PUBLISHED_READS {
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
                let ticket = Ticket::new(command.into());
                let counted = Counted::count(unanswered, command, &ticket);
                ticket.interrupt_with(Arc::downgrade(&kernel) as Weak<dyn Interrupt>);
                let stop = Arc::clone(&ticket);
                let started = std::thread::Builder::new()
                    .name(format!("session-{}-read", found.id))
                    .spawn(move || {
                        let _caller = caller.enter();
                        let _span = tracing::info_span!(
                            "session_command",
                            session = %session.id,
                            command = command.as_str(),
                            model = "published"
                        )
                        .entered();
                        #[cfg(test)]
                        session.gate.pass();
                        let response = stop.answer(
                            || Some(session.info(false)),
                            || {
                                let (info, published) = session.published_now();
                                published_answer(info, published, |published, model| {
                                    read(Read {
                                        model: &published.model,
                                        read: model,
                                        stop: &stop,
                                        kernel: &kernel,
                                        reached: &published.reached,
                                    })
                                })
                            },
                        );
                        drop(counted);
                        let _ = reply.send(response);
                    });
                match started {
                    Ok(thread) => {
                        threads.push(thread);
                        // It waits on no rebuild, so it reports none's progress.
                        Running::new(answer, READ_STOPPED, Some(ticket), None)
                    }
                    // The read, and its count, dropped with the thread that never started.
                    Err(e) => refused_here(crate::worker::thread_error("published read", &e)),
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
    pub fn outline(&self, session: &SessionId) -> Running<OutlineAnswer> {
        self.queued(session, SessionCommand::Outline, |session, _, _| {
            let (_, gated) = session.document();
            let document = gated.document().map_err(Clone::clone)?;
            Ok(OutlineAnswer {
                outline: super::outline(document),
            })
        })
    }

    /// Begins a transaction in `session` (the design's section 4): its state kept (its text,
    /// the gate's finding, its model or the rebuild running for it, its revision, what its
    /// edits reached) for `rollback` to restore. Inside it, edits are applied without being
    /// saved or starting a rebuild. Refused if one is open: transactions don't nest.
    pub fn begin(&self, session: &SessionId) -> Running<Begun> {
        self.queued(session, SessionCommand::Begin, |session, stop, _| {
            session.begin(stop)
        })
    }

    /// Ends `session`'s transaction, saving its document (the design's section 4): waits
    /// for the current revision's rebuild, then writes the canonical text through
    /// `file::save_over`, with new fingerprints and solves only for the features its edits
    /// reached (decision 28; every other keeps what's stored, and its drift is a problem,
    /// never written over), and moves the session to a new revision, the text written. A
    /// document equal to the one `begin` saw writes nothing, even over a file changed
    /// outside the session. Refused, the transaction left open, if none is open, if the
    /// file changed since the session read or wrote it or is gone (`conflict`), or can't be
    /// read (`io_failed`), if its text would pass the limit, if it can't be written
    /// (`io_failed`), or if it's cancelled before it writes. A kernel fault is saved around
    /// (decision 32): the features it stopped are saved as failed ones are (one its edits
    /// reached with no fingerprint, decision 28 amended; any other as stored), and it's
    /// given once, in the response's `model`.
    pub fn commit(&self, session: &SessionId) -> Running<Committed> {
        let registry = Arc::downgrade(&self.registry);
        self.queued_with(
            session,
            SessionCommand::Commit,
            Rereads::Always,
            move |session, stop, model, reload| {
                let id = session.id;
                session.commit(stop, reload.outside(), model, &|resolved| {
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
        self.queued(session, SessionCommand::Rollback, |session, stop, _| {
            session.rollback(stop)
        })
    }

    /// Saves `session`'s document in canonical form with every feature's fingerprint and
    /// every sketch's solve as rebuilt, edited or not (decision 31: what `silico fmt` runs,
    /// and how drift is accepted, by name): with `features`, only those features' drift is
    /// accepted, and every other drifted feature keeps what's stored with it and is a
    /// problem. Written through `file::save_over` only if `write` and the text differs from
    /// the file; with `write` false, says whether it would be. Refused inside a transaction
    /// (commit or roll back first), for a document the gate refuses, and for a feature in
    /// `features` the document hasn't; and, writing, as a commit is.
    pub fn format(
        &self,
        session: &SessionId,
        features: Option<Vec<FeatureId>>,
        write: bool,
    ) -> Running<Reformatted> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(
            session,
            SessionCommand::Format,
            move |session, stop, model| {
                let id = session.id;
                session.format(stop, features.as_deref(), write, model, &|resolved| {
                    reclaim(&registry, id, resolved);
                })
            },
        )
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
        self.queued(
            session,
            SessionCommand::AddFeature,
            move |session, stop, model| {
                let id = session.id;
                let edited = session.edit(
                    stop,
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
                let id = edited.made.id.clone();
                let feature = as_committed(edited.saved.as_deref(), edited.made, |saved| {
                    saved.feature(&id).ok().cloned()
                })?;
                Ok(Added {
                    feature,
                    changed: edited.changed,
                    commit: edited.commit,
                })
            },
        )
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
        self.queued(
            session,
            SessionCommand::SetField,
            move |session, stop, model| {
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| edit::set_field(base.checked()?, &feature, &field, value),
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                let edit::FieldValues {
                    before,
                    after,
                    path,
                } = edited.made;
                let after = as_committed(edited.saved.as_deref(), after, |saved| {
                    edit::value_of(saved, &feature, &path)
                })?;
                Ok(FieldSet {
                    feature,
                    field,
                    before,
                    after,
                    changed: edited.changed,
                    commit: edited.commit,
                })
            },
        )
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
        self.queued(
            session,
            SessionCommand::RemoveFeature,
            move |session, stop, model| {
                let id = session.id;
                let edited = session.edit(
                    stop,
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
            },
        )
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
        self.queued(
            session,
            SessionCommand::ReplaceText,
            move |session, stop, model| {
                let id = session.id;
                let edited = session.edit(
                    stop,
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
                        let (features, candidate) =
                            edit::replace_text(base.gated, text.as_bytes())?;
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
            },
        )
    }

    fn find(&self, session: &SessionId) -> Option<Arc<Session<P::Client>>> {
        lock(&self.registry.book).sessions.get(session).cloned()
    }

    /// `work` queued on `session`'s thread, under the caller's span and the command's, after
    /// the session reads its file again if no transaction is open ([`Session::reload`],
    /// decision 2), refused if it can't take the file as it is. `work` is handed the
    /// command's stop, which it settles before it leaves a trace, and the model it reads.
    fn queued<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: SessionCommand,
        work: impl FnOnce(
            &Arc<Session<P::Client>>,
            &Ticket,
            &mut Option<ModelRead>,
        ) -> Result<T, Problems>
        + Send
        + 'static,
    ) -> Running<T> {
        self.queued_with(
            session,
            command,
            Rereads::OutsideTransaction,
            move |session, stop, model, _| work(session, stop, model),
        )
    }

    /// [`queued`](Self::queued), the file read again as `rereads` says, and `work` given
    /// what the session found of it.
    fn queued_with<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: SessionCommand,
        rereads: Rereads,
        work: impl FnOnce(
            &Arc<Session<P::Client>>,
            &Ticket,
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
        let ticket = Ticket::new(command);
        let woken: Weak<dyn Interrupt> = Arc::downgrade(&found) as Weak<dyn Interrupt>;
        ticket.interrupt_with(woken);
        let stop = Arc::clone(&ticket);
        found.queue(Box::new(move |turn| {
            let response = match turn {
                Turn::Run(session) => {
                    let _caller = caller.enter();
                    let _span = tracing::info_span!(
                        "session_command",
                        session = %session.id,
                        command = command.as_str()
                    )
                    .entered();
                    *lock(&session.command) = Some(Arc::clone(&stop));
                    // Closed as it started (after `serve` found it open, before it was
                    // recorded): closed now, as `close` would have, since `close` ends the
                    // queue before it reads the command.
                    if session.closed() {
                        stop.close();
                    }
                    #[cfg(test)]
                    session.gate.pass();
                    let response = stop.answer(
                        // Cancelled while it was queued: never run, the file not read.
                        || Some(session.info(false)),
                        || {
                            let mut model = None;
                            let id = session.id;
                            let claimed = |found: Claim<'_>| match registry.upgrade() {
                                Some(registry) => claim(
                                    &registry,
                                    found.opened,
                                    found.resolved,
                                    found.found_at,
                                    id,
                                ),
                                // The sessions are being dropped: no claim is kept.
                                None => Ok(()),
                            };
                            // The session's own work, which stands whatever the command
                            // comes to: a cancel after it still says `reloaded`.
                            let (reloaded, result) = match session.reload(rereads, &claimed) {
                                Ok(reload) => {
                                    (reload.reloaded(), work(session, &stop, &mut model, reload))
                                }
                                Err(problems) => (false, Err(problems)),
                            };
                            Response {
                                session: Some(session.info(reloaded)),
                                model,
                                result,
                            }
                        },
                    );
                    lock(&session.command).take();
                    response
                }
                // Never run: cancelled, if it was first, or refused, and either way settled.
                Turn::Closed(id) => stop.answer(|| None, || refused(not_open(id))),
            };
            let _ = reply.send(response);
        }));
        Running::new(
            answer,
            SESSION_STOPPED,
            Some(ticket),
            Some(Arc::clone(&found.newest)),
        )
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
        // and watchers of sessions released before (closed, or their threads ended), which
        // the book took: so no session's thread outlives it.
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
        let released = std::mem::replace(&mut lock(&self.registry.book).released, Threads::new());
        released.join();
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

#[cfg(test)]
mod tests;
