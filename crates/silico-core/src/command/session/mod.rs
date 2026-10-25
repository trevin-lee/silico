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

mod edits;
mod rebuilds;
mod registry;
mod threads;
mod transaction;
mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};

use super::edit::{self, FeatureIds};
use super::{Gated, Problems};
use crate::cache::Cache;
use crate::document::json::Node;
use crate::document::{self, FeatureId};
#[cfg(doc)]
use crate::file;
use crate::kernel::{Clients, Kernel};
use crate::sketch::SketchSolver;
use crate::worker::lock;
use crate::{Error, ErrorCode};

use edits::{as_committed, not_given_back};
pub(super) use rebuilds::faulted;
use rebuilds::{Rebuilds, Unbuilt, published_answer};
use registry::{
    Book, CacheOpener, Registry, Release, SketchStarter, Started, claim, reclaim, serve, start,
};
pub use threads::MAX_PUBLISHED_READS;
use threads::{Reads, Threads, Unanswered};
use transaction::{Claim, Held, Reload, Rereads, transaction_open};
pub use types::{
    Added, Begun, Committed, FieldSet, ModelChoice, ModelRead, Opened, Published, Read,
    Reformatted, Removed, Replaced, Response, Revision, RolledBack, Running, SessionId,
    SessionInfo, Status,
};
pub(crate) use types::SESSION_ID_PATTERN;
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
/// returns).
pub struct Sessions<P: Clients + 'static> {
    registry: Arc<Registry<P>>,
}

/// One open session, shared by its thread, the callers that queue its commands, and the
/// registry.
///
/// Its locks, and the registry's, are taken in one order, each let go before one earlier in
/// the order is taken: the registry's book, then a session's published reads, then its held
/// state, then its rebuilds, then its queue. The registry's `feature_ids` is taken alone.
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
            let id = edited.made.id.clone();
            let feature = as_committed(edited.saved.as_deref(), edited.made, |saved| {
                saved.feature(&id).ok().cloned()
            })?;
            Ok(Added {
                feature,
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

#[cfg(test)]
mod tests;
