//! Sessions (docs/design/m4.1-command-api.md, section 3): a document held open between
//! commands, as every client holds one: MCP's tools, the app's windows, and the CLI, which
//! opens one, runs a command, and closes it.
//!
//! A process's sessions are one [`Sessions`], one client's (decision 30). Each session runs
//! its commands one at a time, in the order they arrive, on a thread of its own
//! (`session-s1`, ...), never the caller's (ARCHITECTURE.md, invariant 4): even `open`'s
//! file read and gate. A command returns a [`Running`] handle at once, which the app polls,
//! MCP's server waits on, and the CLI waits for; only [`Sessions::status`],
//! [`Sessions::read_logs`], and [`Sessions::published`] answer on the caller's thread, from
//! what the session holds (its logs, kept by the process's subscriber: `logs`). Each
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
//! revision whose state equals the old. Inside a transaction, [`Sessions::undo`] and
//! [`Sessions::redo`] step back and forth through its edits, kept on its undo stack
//! ([`crate::document::undo`]), each at a new revision whose state equals an earlier one;
//! a commit or a rollback empties it. One at a time, never nested; a session isn't closed
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
//! Sessions given a workspace ([`Sessions::in_workspace`], decision 26: `silico serve`'s)
//! read and write only inside it: `open`'s path and `render`'s `to` are relative to its
//! root, a path out of it (or through a link out of it) is refused, naming the field, and so
//! is a command whose session's file leads out of it now, found as the file is read again
//! or saved; a `.silico-cache/` that leads out of it is never opened, and one that comes to
//! lead out of it later is read, written, and cleared nothing through while it does (the
//! cache checks as each of those opens its directory, and does all it does through the
//! handle it opened, [`Cache::within`]). Without one (the CLI's), `open` reads any file
//! this user may read.
//!
//! A session also watches its file on disk (docs/design/m5.0-app.md, section 4): `status`
//! starts a comparison off the caller's thread at most once each [`DISK_CHECK_INTERVAL`],
//! and says what the last found (`disk`). One that finds the file changed with no
//! transaction open queues the session's own reload on its thread, at most one at a time
//! (A3): a new revision no command asked for, named `reload` in `status.running`, which
//! nothing cancels. A client that polls is woken as things it draws change
//! ([`Sessions::on_change`]).
//!
//! What it doesn't bound: the caches, one kept for each directory a session has opened a
//! document in, for the process's life; each session's queue, which holds every command
//! queued and not yet run; and a comparison stuck reading its file (a stalled network
//! mount), whose thread ends with the read, after its session has closed if it closes
//! first, holding the session's bytes until then.

mod cancel;
mod clearing;
mod edits;
mod holding;
mod rebuilds;
mod registry;
mod sketch_edits;
mod threads;
mod transaction;
mod types;
mod undo;
mod waking;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, Weak};

use super::edit::{self, FeatureIds};
use super::logs::{LogBuffer, LogQuery, Logs};
use super::{Cancellable, Gated, Problems, ReadCommand, RunningCommand, SessionCommand};
use crate::cache::{Cache, Cleared};
use crate::disk::{DISK_CHECK_INTERVAL, Disk, DiskWatch};
use crate::document::json::Node;
use crate::document::{self, FeatureId};
#[cfg(doc)]
use crate::file;
use crate::file::{Workspace, shown_path};
use crate::kernel::{Clients, Kernel};
use crate::recompute::{self, SketchSolvers};
use crate::sketch::SketchSolver;
use crate::worker::lock;
use crate::{Error, ErrorCode};

use cancel::{Interrupt, Ticket, Why, not_running};
use clearing::{partly_cleared_problems, shown_cache_directory};
use edits::{as_committed, not_given_back};
use holding::Holding;
use rebuilds::{Newest, Rebuilds, Unbuilt, published_answer};
pub use rebuilds::{TryAgainBy, fault_suggestion};
pub(super) use rebuilds::{kept_fault, retried};
use registry::{
    Book, CacheOpener, Opening, Registry, Release, Started, claim, holder, moved, reclaim, reserve,
    serve, start, unreserve,
};
pub use threads::MAX_PUBLISHED_READS;
use threads::{Counted, Reads, Threads};
use transaction::{Claim, Held, Open, Reload, Rereads, SaveTo, transaction_open};
pub use transaction::{NOT_RELOADED, TOO_LONG};
pub(crate) use types::SESSION_ID_PATTERN;
pub use types::{
    Added, Begun, CacheCleared, Cancelled, CheckAnswer, CheckModel, Closed, Committed, ExtendBy,
    FieldSet, ModelChoice, ModelRead, Moved, NewMembers, Opened, OutlineAnswer, Progress,
    Published, Read, ReadClient, Rebuild, RebuildState, Reformatted, Removed, Replaced, Response,
    Revision, RolledBack, Running, SessionId, SessionInfo, SketchCut, SketchEdited, Status,
    Stepped, Stopper, Suppressed, TextAnswer, TimelineAnswer, Transaction,
};
use types::{READ_STOPPED, SESSION_STOPPED};
use waking::Waker;

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
/// running, then its reloads, then its disk watch's, under which none is taken. The
/// registry's `feature_ids` is taken alone.
struct Session<C: Kernel + 'static> {
    id: SessionId,
    /// The sessions' workspace, if they have one: each re-read and save of the session's file
    /// is refused if the file now leads out of it.
    workspace: Option<Workspace>,
    /// The bound of each of its transactions' undo stacks, in bytes as the stack counts them.
    undo_bytes: usize,
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
    /// Its logs: the events of its commands' spans, as `read_logs` reads them.
    logs: Arc<LogBuffer>,
    /// Whether its file changed on disk ([`Sessions::disk`]): told what the session reads
    /// and writes of it, by its reloads and its saves.
    disk: DiskWatch,
    /// Its own reload of its file, found changed on disk (A3): whether one is queued, and
    /// the bytes a reload last refused, so the same refusal isn't queued at each comparison.
    /// Taken before the disk watch's lock (whether a comparison is still current is asked
    /// under it), and under none of the others.
    reloads: Mutex<Reloads>,
    /// The client's waker, the sessions' ([`Sessions::on_change`]).
    waker: Arc<Waker>,
    /// Where its commands wait as each starts, in this crate's tests only.
    #[cfg(test)]
    gate: Gate,
    /// Where `cache_clear` waits once it has settled, before it removes anything, in this
    /// crate's tests only.
    #[cfg(test)]
    settled: Gate,
    /// Where a save waits before it writes its file, in this crate's tests only.
    #[cfg(test)]
    writing: Gate,
    /// Where the session's own reload waits as it starts, in this crate's tests only.
    #[cfg(test)]
    reloading: Gate,
    /// Where a rebuild's watcher waits before it first looks at the rebuild's progress, in
    /// this crate's tests only.
    #[cfg(test)]
    watching: Gate,
}

/// A session's own reloads (A3): one queued, and what the last refused, if it was refused
/// and none has been taken since: the digest of the bytes it was queued for, and the
/// registry's count of claims then (`Book::claims`), since a file another session claimed
/// may be taken once its claim changes.
#[derive(Default)]
struct Reloads {
    queued: bool,
    refused: Option<([u8; 32], u64)>,
}

/// The digest of a file's bytes, as a refused reload keeps them.
fn digest(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).into()
}

/// Where a file a command writes may go ([`Sessions::write_rules`]).
pub(crate) struct WriteRules {
    pub(crate) workspace: Option<crate::file::Workspace>,
    pub(crate) held: Vec<PathBuf>,
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
    /// The session closed before its turn, or, for a command sent while it opened, never
    /// opened (its open refused or cancelled): either way it isn't open, which is all the
    /// command is told, as one sent after is.
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
    fn reads_running(&self) -> Vec<ReadCommand> {
        let reads = lock(&self.reads);
        lock(&reads.unanswered).commands().collect()
    }

    /// The stop of the queued command running, if one is: for `status` and `cancel`, never
    /// for a command's own work, which is handed its own stop.
    fn running(&self) -> Option<Arc<Ticket>> {
        lock(&self.command).clone()
    }
}

impl<C: Kernel + 'static> Session<C> {
    /// Whether a comparison that found the file changed, as `found`, should queue the
    /// session's own reload (A3): not inside a transaction (only `commit` reads the file
    /// there), nor while its own save has left the file part written (the next save says
    /// where its old text is), nor while one is queued, nor for the bytes a reload refused
    /// last (a file another session holds, say), which would be refused again; nor for a
    /// comparison made against `counter`, a count of the session's reads and writes no
    /// longer its own (it read or wrote since: dropped, never reported). If it should, it's
    /// marked queued.
    fn reload_wanted(&self, found: &[u8], claims: u64, counter: u64) -> bool {
        {
            let held = self.held.read();
            if held.transaction.is_some() || held.part_written.is_some() {
                return false;
            }
        }
        let mut reloads = lock(&self.reloads);
        // A comparison the session has read or written since (its own reload, taking these
        // bytes) is dropped, never reported (the design's section 4): checked under the
        // reloads' lock, as a reload moves the watch on before it clears `queued` there, so
        // a comparison found current here either finds `queued` still set or is current.
        if !self.disk.current(counter)
            || reloads.queued
            || reloads
                .refused
                .is_some_and(|refused| refused == (digest(found), claims))
        {
            return false;
        }
        reloads.queued = true;
        true
    }

    /// What the session's own reload came to, queued for a file found as `found`
    /// ([`Session::reload`] as `commit` reads the file, inside a transaction too): reloaded
    /// at a new revision; the file found as the session holds it (a `begin` or another
    /// command ahead of it reloaded it first), the finding then `unchanged`; inside a
    /// transaction and changed, nothing, the finding left `changed`; or refused, `found`
    /// kept with `claims` so the same bytes aren't queued again until a claim changes; any
    /// other answer forgets a refusal. Whether the client should be woken.
    fn reloaded_itself(
        &self,
        reloaded: Result<Reload, Problems>,
        found: [u8; 32],
        claims: u64,
    ) -> bool {
        let (woken, refused) = match reloaded {
            Ok(Reload::Reloaded) => (true, None),
            Ok(Reload::AsHeld) => {
                let held = self.held.read();
                if held.part_written.is_none() {
                    // The file as the session holds it: the finding was stale.
                    self.disk.held(Arc::clone(&held.bytes));
                }
                (true, None)
            }
            Ok(Reload::Outside(_)) => (false, None),
            Err(problems) => {
                tracing::warn!(
                    session = %self.id,
                    "the file changed on disk, and the session couldn't reload it: {:?}",
                    problems.errors()
                );
                (false, Some((found, claims)))
            }
        };
        let mut reloads = lock(&self.reloads);
        reloads.queued = false;
        // Any reload taken forgets a refusal: its bytes may come back, to be tried again.
        reloads.refused = refused;
        woken
    }
}

/// What a session's disk watch tells it when a comparison finds its file changed: the
/// session's own reload queued on its thread, behind its commands, if one is wanted
/// ([`Session::reload_wanted`]), as the next command would reload it (A3). Holds both
/// weakly, so the watch keeps neither alive.
fn reloader<P: Clients + 'static>(
    registry: Weak<Registry<P>>,
    session: Weak<Session<P::Client>>,
) -> crate::disk::Changed {
    Arc::new(move |found: &[u8], counter: u64| {
        let Some(session) = session.upgrade() else {
            return;
        };
        let claims = registry
            .upgrade()
            .map_or(0, |registry| lock(&registry.book).claims);
        if session.reload_wanted(found, claims, counter) {
            session.queue(reload_job(Weak::clone(&registry), digest(found)));
        }
    })
}

/// The session's own reload, a job on its queue (A3): the file read as `commit` reads it
/// (claimed as every command's read is, outside a transaction reloaded at a new revision),
/// named `reload` in `status.running` and the session's logs while it runs, and the client
/// woken by what it changed. A session closed first never runs it.
fn reload_job<P: Clients + 'static>(
    registry: Weak<Registry<P>>,
    found: [u8; 32],
) -> Job<P::Client> {
    Box::new(move |turn| {
        let Turn::Run(session) = turn else {
            return;
        };
        let stop = Ticket::reloading();
        *lock(&session.command) = Some(Arc::clone(&stop));
        let woken = {
            let _span = command_span(
                &session.logs,
                session.id,
                RunningCommand::Reload.as_str(),
                None,
            )
            .entered();
            #[cfg(test)]
            session.reloading.pass();
            let id = session.id;
            let claimed = |found: Claim<'_>| match registry.upgrade() {
                Some(registry) => {
                    claim(&registry, found.opened, found.resolved, found.found_at, id)
                }
                // The sessions are being dropped: no claim is kept.
                None => Ok(()),
            };
            // The claims as the reload found them: a refusal is kept until they change.
            let claims = registry
                .upgrade()
                .map_or(0, |registry| lock(&registry.book).claims);
            let reloaded = session.reload(Rereads::Always, &claimed);
            session.reloaded_itself(reloaded, found, claims)
        };
        lock(&session.command).take();
        if woken {
            session.waker.wake();
        }
    })
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

    /// Starts up to `kernel` kernel workers and `sketches` sketch workers ahead of any
    /// rebuild, the two at once, and waits until each has started or failed: an app's warm
    /// start at launch (M5.0's decision 10), run off its UI thread. A rebuild that comes
    /// meanwhile waits for them rather than starting more; one that comes after starts none.
    /// How many of each it started.
    pub fn warm(&self, kernel: usize, sketches: usize) -> (usize, usize) {
        let registry = &self.registry;
        std::thread::scope(|scope| {
            let kernel_warm = std::thread::Builder::new()
                .name("silico-warm-kernel".to_owned())
                .spawn_scoped(scope, || registry.kernel.warm(kernel));
            let sketches_started = registry.sketches.warm(sketches);
            let kernel_started = match kernel_warm {
                Ok(thread) => thread.join().unwrap_or(0),
                // No thread to spare: one after the other, here.
                Err(_) => registry.kernel.warm(kernel),
            };
            (kernel_started, sketches_started)
        })
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
                    opening: BTreeMap::new(),
                    files: BTreeMap::new(),
                    paths: BTreeMap::new(),
                    caches: BTreeMap::new(),
                    threads: Threads::new(),
                    released: Threads::new(),
                    stopping: false,
                    claims: 0,
                }),
                renderer: Mutex::new(None),
                step: Mutex::new(Arc::new(crate::step_worker::StepWorkers::new())),
                meshes: Arc::new(crate::meshes::MeshCache::new(
                    crate::meshes::MESH_CACHE_BYTES,
                )),
                export_turns: Arc::default(),
                workspace: None,
                disk_interval: DISK_CHECK_INTERVAL,
                undo_bytes: crate::document::undo::UNDO_BYTES,
                disk_stand_in: None,
                waker: Arc::default(),
            }),
        }
    }

    /// The client's waker (docs/design/m5.0-app.md, section 3, *Waking without input*):
    /// from now on, `waker` is called when something the client draws may have changed, so
    /// a client that polls (the app, once a frame) draws a frame then: as a command's answer
    /// can be taken ([`Running::try_take`] gives it); as a rebuild's step settles and as the
    /// rebuild ends (its model published, or its end recorded); as a disk comparison's
    /// finding changes; and as the session reloads its file itself. A command answered as
    /// it's asked for (refused before it's queued) wakes nothing: its caller has the answer.
    /// It's called with no lock of the core's held, so it may ask the sessions anything,
    /// and off the caller's thread, but for one case: a command sent to a session that has
    /// just closed (its queue ended) is answered, and wakes the client, on the thread that
    /// sent it. It should only ask for a frame, never draw. Given again, the new one replaces
    /// the old.
    pub fn on_change(&self, waker: impl Fn() + Send + Sync + 'static) {
        self.registry.waker.set(Arc::new(waker));
    }

    /// The same sessions, each comparing its file on disk at most once each `interval`,
    /// reading it with `read` and taking the time from `now` (a test's stand-ins, to hold a
    /// comparison and to say when the next is due without waiting for it), as they're made:
    /// refused once a document has been asked to open.
    #[cfg(any(test, feature = "fake"))]
    pub fn watching_disk(
        mut self,
        interval: std::time::Duration,
        read: crate::disk::Reader,
        now: crate::disk::Clock,
    ) -> Result<Self, Error> {
        let opened = Error::new(
            ErrorCode::InvalidInput,
            "these sessions can't be given a disk watch's stand-ins now",
        )
        .with_suggestion("give them as the sessions are made, before any document opens");
        let Some(registry) = Arc::get_mut(&mut self.registry) else {
            return Err(opened);
        };
        if !registry.book.get_mut().is_ok_and(|book| book.next == 1) {
            return Err(opened);
        }
        registry.disk_interval = interval;
        registry.disk_stand_in = Some((read, now));
        Ok(self)
    }

    /// The same sessions, each comparing its file on disk at most once each `interval`,
    /// as they're made: in this crate's tests, which don't wait out the second.
    #[cfg(test)]
    fn checking_disk_every(mut self, interval: std::time::Duration) -> Self {
        let registry = Arc::get_mut(&mut self.registry);
        assert!(
            registry.is_some(),
            "sessions are given their interval as they're made"
        );
        if let Some(registry) = registry {
            registry.disk_interval = interval;
        }
        self
    }

    /// The same sessions, each transaction's undo stack bounded by `bytes` as the stack counts
    /// them, as they're made: in this crate's tests, which don't fill 256 MiB.
    #[cfg(test)]
    fn bounding_undo(mut self, bytes: usize) -> Self {
        let registry = Arc::get_mut(&mut self.registry);
        assert!(
            registry.is_some(),
            "sessions are given their undo bound as they're made"
        );
        if let Some(registry) = registry {
            registry.undo_bytes = bytes;
        }
        self
    }

    /// The same sessions, in the workspace at `root` (decision 26), as they're made, before
    /// any document opens, as [`rendering`](Self::rendering) gives them a renderer: its root
    /// canonicalized once, and fixed for the sessions' life. A relative path `open` is
    /// given, or `render`'s `to`, is relative to the root; a path outside the workspace, or
    /// one that reaches out of it through a link, is refused, `invalid_input` naming the
    /// field, and so is a command whose session's file now leads out of it, as the session
    /// reads it again or saves it (a link put on its path since `open`). A `.silico-cache/`
    /// that leads out of it is never opened: its sessions keep no results; nor through one
    /// that comes to lead out later, while it does (the cache checks as each read, write,
    /// eviction, and clear opens its directory, and does all it does through the handle it
    /// opened, [`Cache::within`]). Without one (the CLI), paths resolve as the system
    /// resolves them. Refused if the root isn't a
    /// directory that can be reached, and if these sessions have a workspace already, or
    /// have been asked to open a document.
    pub fn in_workspace(mut self, root: &Path) -> Result<Self, Error> {
        let workspace = Workspace::new(root)?;
        let too_late = || {
            Error::new(
                ErrorCode::InvalidInput,
                "these sessions can't be given a workspace now",
            )
            .with_cause(
                "a workspace is given once, as the sessions are made, before any document opens",
            )
            .with_suggestion(crate::worker::SILICO_BUG)
        };
        let Some(registry) = Arc::get_mut(&mut self.registry) else {
            return Err(too_late());
        };
        let unopened = registry.book.get_mut().is_ok_and(|book| book.next == 1);
        if registry.workspace.is_some() || !unopened {
            return Err(too_late());
        }
        registry.workspace = Some(workspace);
        Ok(self)
    }

    /// The sessions' workspace, if they have one.
    pub fn workspace(&self) -> Option<&Workspace> {
        self.registry.workspace.as_ref()
    }

    /// The same sessions, drawing `render`'s scenes with `renderer` (the render worker's
    /// client, [`crate::render::RenderClient`], or a test's fake), as they're given their
    /// kernel. Without one, `render` is refused as `worker_unavailable`.
    #[must_use]
    pub fn rendering(self, renderer: impl crate::command::render::Renderer + 'static) -> Self {
        *lock(&self.registry.renderer) = Some(Arc::new(renderer));
        self
    }

    /// The same sessions, writing `export`'s STEP files with `exporter` (a test's stand-in)
    /// rather than the STEP worker.
    #[must_use]
    pub fn exporting_step(self, exporter: impl crate::step_worker::StepExporter + 'static) -> Self {
        *lock(&self.registry.step) = Arc::new(exporter);
        self
    }

    /// What writes `export`'s STEP files.
    pub(crate) fn step_exporter(&self) -> Arc<dyn crate::step_worker::StepExporter> {
        Arc::clone(&lock(&self.registry.step))
    }

    /// The turn each export's mesh waits for, one for every session.
    pub(crate) fn export_turns(&self) -> Arc<crate::command::export::MeshTurns> {
        Arc::clone(&self.registry.export_turns)
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

    /// Where a file a command writes may go (decision 35), taken by every command that
    /// writes one: the workspace it must be inside, if the sessions have one (decision 26),
    /// and the documents the sessions hold, by their paths, which it never is.
    pub(crate) fn write_rules(&self) -> WriteRules {
        WriteRules {
            workspace: self.workspace().cloned(),
            held: lock(&self.registry.book).paths.keys().cloned().collect(),
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
    /// closing) uses none. From the call, the session is opening: a command naming it,
    /// sent before the open answers, waits behind the open and runs once it opens, as if
    /// sent after (a queued command in the session's queue, first; a published read
    /// started as it opens), or, if it doesn't open, is refused as naming no session open,
    /// exactly as one sent after the refusal is, whatever the open came to.
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
        // Its logs, kept from here, so its open's are too.
        let logs = LogBuffer::new();
        let opening_logs = Arc::clone(&logs);
        // Its queue, made now, so a command sent before it opens waits in it.
        let (queue, jobs) = mpsc::channel();
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
                    let _span = command_span(&opening_logs, id, "open", None).entered();
                    start(registry, id, &path, create, &stop, &reply, jobs)
                };
                if let Some(Started { session, jobs }) = started {
                    serve(&session, &jobs);
                }
            });
        match started {
            Ok(thread) => {
                book.opening.insert(
                    id,
                    Opening {
                        queue,
                        newest: Arc::default(),
                        logs,
                        reads: Vec::new(),
                    },
                );
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
        status_in(&self.registry, session)
    }

    /// Whether `session`'s file changed on disk since the session last read or wrote it
    /// (docs/design/m5.0-app.md, section 4; what `status`'s `disk` will say, M5.4): the
    /// last comparison's finding, none before one has ended, and the next comparison
    /// started off the caller's thread if one is due ([`DiskWatch::finding`]). Answered on
    /// the caller's thread, never waiting on the file or on the session's commands. It
    /// reloads nothing: the next command does, as it always has (decision 2). Refused,
    /// naming `session`, if it isn't open.
    pub fn disk(&self, session: &SessionId) -> Result<Option<Disk>, Error> {
        let found = find_in(&self.registry, session).ok_or_else(|| not_open(*session))?;
        Ok(found.disk.finding())
    }

    /// `session`'s logs (M4.7): the events `query` asks for, from what its buffer keeps,
    /// answered on the caller's thread, never queued and never reading the file, and adding
    /// no event of its own, so a read of the same cursor again finds only what's new.
    pub fn read_logs(&self, session: &SessionId, query: &LogQuery) -> Response<Logs> {
        read_logs_in(&self.registry, session, query)
    }

    /// Stops what `session` is running that `named` names (decision 34), answered at once on
    /// the caller's thread, never queued and never reading the file. A command: the queued
    /// command, if it's the one running, and every published read of that name running.
    /// Each stops as [`Running::cancel`] stops it, and answers too: `cancelled` if it left
    /// no trace, its real result if it had done its work. The answer says which (`stopped`:
    /// whether any was stopped before it had). The rebuild ([`Cancellable::Rebuild`]): the
    /// one `status` reports running, stopped as a cancelled command's wait stops it, and the
    /// command waiting on it, if one is, answers `cancelled` too. Refused, naming `command`,
    /// if what it names isn't running, what is (as `status` reports it) given as candidates:
    /// so a cancel that comes late never stops the command queued after the one it meant.
    pub fn cancel(
        &self,
        session: &SessionId,
        named: impl Into<Cancellable>,
    ) -> Response<Cancelled> {
        cancel_in(&self.registry, session, named.into())
    }

    /// What answers [`status`](Self::status) and [`cancel`](Self::cancel) later, on whichever
    /// thread asks, as they're answered here: for `call`, whose caller (MCP's reader) never
    /// waits on what a session holds.
    pub(crate) fn weak(&self) -> WeakSessions<P> {
        WeakSessions(Arc::downgrade(&self.registry))
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

    /// `session`'s document's problems ([`super::check`]) from the model `model` names
    /// (decision 33), and its warnings ([`crate::warnings`]: none for a document the gate
    /// refused, which has no model to find them in): a `check` request's `model` and `fresh`,
    /// as the wire reads them (`CheckRequest::model`).
    ///
    /// The current revision's: the gate's problems, if it refused the document, or, from the
    /// session's current revision rebuilt (waited for, and started if it hasn't been:
    /// decision 13), each failed feature's and each drift of a feature no edit reached since
    /// the last save (decision 28: a reached feature's stored fingerprint was measured from
    /// other inputs, and a commit writes its new one).
    ///
    /// With `fresh` (the design's section 7, M4.3), the current revision rebuilt with every
    /// step built anew, as SolidWorks' forced rebuild (Ctrl+Q) does, reading nothing kept,
    /// neither in the session's engine (its steps' results, the refusals among them, its
    /// sketches' solves) nor in the cache on disk
    /// ([`Recompute::start_fresh`](crate::recompute::Recompute::start_fresh)), though what it
    /// builds is kept in both, so the next rebuild hits it. A kernel fault the revision kept
    /// as its outcome (decision 32) is tried again, and the model this rebuild publishes is
    /// the revision's from then on, the fault's or not. For a document the gate refused, the
    /// gate's problems, nothing rebuilt.
    ///
    /// Or the newest published model's, answered at once from what the session's rebuilds
    /// have published, never waiting for a rebuild or queueing behind its commands, as every
    /// published read is ([`read_model`](Self::read_model)): its revision in the response's
    /// `model`, which may be an older one's. Its drift leaves out what the session's edits
    /// reached at that model's revision, kept with it ([`Read::reached`]): exactly that
    /// revision's, whatever edits came since. With nothing published, it's refused as every
    /// published read is: naming `model`, or, for a document the gate refused, with the
    /// gate's problems. Its warnings leave out a missing fingerprint a commit writes: if a
    /// transaction is open as it's checked, or, for a model older than the session's
    /// revision, if that model's revision was a transaction's; and, for an older model, one
    /// the session's document stores now, which a save since stored
    /// ([`Checking::spared`](transaction::Checking::spared)).
    ///
    /// Either way, a kernel fault (a worker that crashed or hung) isn't the document's: it's
    /// given once, beside them, in the response's `model` ([`ModelRead::fault`]), and kept as
    /// the revision's outcome, so the next check of the revision reads the same model
    /// (decision 32).
    pub fn check(&self, session: &SessionId, model: CheckModel) -> Running<CheckAnswer> {
        match model {
            CheckModel::Current { fresh } => self.check_current(
                session,
                if fresh {
                    recompute::Reads::Nothing
                } else {
                    recompute::Reads::Kept
                },
            ),
            CheckModel::Published => self.read_model(
                session,
                ReadCommand::Check,
                ModelChoice::Published,
                move |read| {
                    let _span = tracing::info_span!("check").entered();
                    Ok(CheckAnswer::of(
                        read.model(),
                        &read.checking.excepted(read.reached),
                    ))
                },
            ),
        }
    }

    /// Runs `read` on the session's current model, queued as the command `command`, waiting
    /// for its revision's rebuild: [`read_model`](Self::read_model)'s `Current`, for a command
    /// that reads only the current model (`export`), so is never one of the published reads.
    pub(crate) fn read_current<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: SessionCommand,
        read: impl FnOnce(Read<'_, P::Client>) -> Result<T, Problems> + Send + 'static,
    ) -> Running<T> {
        self.read_current_or(session, command, read, rebuilds::gated_problems)
    }

    /// [`read_current`](Self::read_current), but answering a document the gate (or the
    /// reader) refused with `answer_refused`, given the session's revision and that document.
    fn read_current_or<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: SessionCommand,
        read: impl FnOnce(Read<'_, P::Client>) -> Result<T, Problems> + Send + 'static,
        answer_refused: impl FnOnce(Revision, &Gated) -> Result<T, Problems> + Send + 'static,
    ) -> Running<T> {
        // The read's own client, made as it's asked for and dropped when it returns, its stop
        // able to retire it if it's cancelled or its session closes first.
        let kernel: Arc<ReadClient<P::Client>> =
            Arc::new(ReadClient::new(self.registry.kernel.client()));
        self.queued(session, command, move |session, stop, model| {
            stop.interrupt_with(Arc::downgrade(&kernel) as Weak<dyn Interrupt>);
            let current = match session.current(stop) {
                Ok(current) => current,
                Err(Unbuilt::Gated { revision, gated }) => return answer_refused(revision, &gated),
                Err(unbuilt) => return Err(unbuilt.problems()),
            };
            *model = Some(current.read.clone());
            let path = session.held.read().path.clone();
            read(Read {
                path,
                model: &current.model,
                read: current.read,
                stop,
                kernel: &kernel,
                reached: &current.reached,
                checking: current.checking,
            })
        })
    }

    /// `check` of `session`'s current model, its rebuild reading what's kept as `reads` says.
    fn check_current(&self, session: &SessionId, reads: recompute::Reads) -> Running<CheckAnswer> {
        self.queued(
            session,
            SessionCommand::Check,
            move |session, stop, model| match session.current_with(stop, reads) {
                Ok(current) => {
                    *model = Some(current.read);
                    let fresh = reads == recompute::Reads::Nothing;
                    let _span = tracing::info_span!("check", fresh).entered();
                    Ok(CheckAnswer::of(
                        &current.model,
                        &current.checking.excepted(&current.reached),
                    ))
                }
                Err(Unbuilt::Gated { gated, .. }) => Ok(CheckAnswer::refused(gated.problems())),
                Err(Unbuilt::Unavailable(problems)) => Err(problems),
            },
        )
    }

    /// `cache_clear` (the design's section 7, M4.3; decision 25): every entry of the
    /// rebuild cache beside `session`'s document removed ([`Cache::clear`]), the
    /// directory's, shared by every document in it, and only the files the cache made as
    /// entries: never another file in `.silico-cache/`, its `.gitignore`, the partial files
    /// of writes under way (another process's, whose rename would fail), its subdirectories,
    /// or the user's secret, which isn't in it; never through a link. An entry removed is
    /// only a miss, and one another process is reading it reads whole (it holds the file
    /// open); one written after it's listed is left, whole. Answers with the directory, as
    /// the session's path names it, and how many entries it removed. Refused, `io_failed`,
    /// naming each file it couldn't remove and each directory it couldn't read (the first
    /// hundred, all counted), having removed every other. Settled before it removes any, so
    /// a cancel that comes after it starts comes too late. It never reads the session's
    /// file (decision 2, as `close`): it needs nothing of the document, so a file gone or
    /// changed neither refuses it nor moves the session's revision; and it runs inside a
    /// transaction too. In a workspace (decision 26), the directory is named from its root,
    /// and whether it leads out of it is checked by the cache as the clear opens it
    /// ([`Cache::within`]): nothing is removed through one that does then, and the clear is
    /// refused, `invalid_input` naming no parameter, its cause the link; one inside then is
    /// cleared, whatever it was as the session opened, through the handle
    /// the clear opened, so a link put on its way during the clear redirects nothing.
    pub fn cache_clear(&self, session: &SessionId) -> Running<CacheCleared> {
        self.queued_with(
            session,
            SessionCommand::CacheClear,
            Rereads::Never,
            |session, stop, _, _| {
                // The engine's own cache, in the workspace if there's one: a stand-in that
                // writes nothing if its directory led out of it as the session opened
                // (`Registry::cache`). Whether this clear goes through is the cache's to
                // check, as it opens the directory (decision 26: where a path resolves):
                // nothing is removed through one that leads out then, and the clear is
                // refused, saying so, never answered as an empty cache's.
                let cache = lock(&session.rebuilds).cache.clone();
                let path = session.held.read().path.clone();
                let workspace = session.workspace.as_ref();
                let directory = shown_cache_directory(workspace, &path, cache.directory());
                stop.settle()?;
                #[cfg(test)]
                session.settled.pass();
                match cache.clear_unless_out() {
                    Ok(Cleared::Removed(removed)) => Ok(CacheCleared { directory, removed }),
                    Ok(Cleared::LeadsOut) => Err(Problems::from(clearing::leads_out(
                        workspace,
                        &path,
                        cache.directory(),
                    ))),
                    Err(partly) => Err(partly_cleared_problems(
                        &directory,
                        cache.directory(),
                        partly,
                    )),
                }
            },
        )
    }

    /// Runs `read` on the model `choice` names, off the caller's thread (decision 33), as
    /// the command named `command` (for its trace, and the session's record of reads).
    /// `Current` is queued on the session and waits for its revision's rebuild, as `check`
    /// does; `Published` runs at once on a thread of its own (`session-sN-read`), never
    /// queued, on the newest published model, as [`published`](Self::published) gives it:
    /// starting the current rebuild if needed, refused naming `model` if nothing's published,
    /// and with the gate's (or the reader's) problems for a document the gate refused, with
    /// nothing published. At most [`MAX_PUBLISHED_READS`] of a session's published reads are
    /// unanswered at once, or held while it opens: past them, one is refused (each
    /// session's own, apart from every other's). The response's `model` is the [`ModelRead`] of the
    /// model read, which `read` is handed too ([`Read`]); `read`'s error is the response's
    /// refusal.
    pub fn read_model<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: ReadCommand,
        choice: ModelChoice,
        read: impl FnOnce(Read<'_, P::Client>) -> Result<T, Problems> + Send + 'static,
    ) -> Running<T> {
        self.read_model_or(session, command, choice, read, rebuilds::gated_problems)
    }

    /// [`read_model`](Self::read_model), but answering a document the gate (or the reader)
    /// refused, where there's no model to read (the current one, or nothing published),
    /// with `answer_refused`, given the session's revision and that document, the response
    /// naming no model: for `timeline`, which answers one.
    fn read_model_or<T: Send + 'static>(
        &self,
        session: &SessionId,
        command: ReadCommand,
        choice: ModelChoice,
        read: impl FnOnce(Read<'_, P::Client>) -> Result<T, Problems> + Send + 'static,
        answer_refused: impl FnOnce(Revision, &Gated) -> Result<T, Problems> + Send + 'static,
    ) -> Running<T> {
        match choice {
            ModelChoice::Current => {
                self.read_current_or(session, command.into(), read, answer_refused)
            }
            ModelChoice::Published => {
                // The read's own client, made as it's asked for and dropped when it returns,
                // its stop able to retire it if it's cancelled or its session closes first.
                let kernel: Arc<ReadClient<P::Client>> =
                    Arc::new(ReadClient::new(self.registry.kernel.client()));
                let ticket = Ticket::new(command.into());
                let (reply, answer) = mpsc::sync_channel(1);
                let caller = tracing::Span::current();
                let mut book = lock(&self.registry.book);
                if let Some(found) = book.sessions.get(session).cloned() {
                    drop(book);
                    return match start_published(
                        &found,
                        command,
                        kernel,
                        (read, answer_refused),
                        &ticket,
                        &reply,
                        caller,
                    ) {
                        Ok(()) => Running::new(answer, READ_STOPPED, Some(ticket), None),
                        Err(response) => Running::answered(response),
                    };
                }
                // Opening: started as it opens, before any read sent after it opens, or
                // refused if it doesn't.
                let Some(opening) = book.opening.get_mut(session) else {
                    return Running::answered(refused(not_open(*session)));
                };
                // As many held as an open session runs at once, and refused alike past them.
                if opening.reads.len() >= MAX_PUBLISHED_READS {
                    return Running::answered(refused(too_many_reads(*session)));
                }
                let stop = Arc::clone(&ticket);
                // Answered here only as its session opens or doesn't, whose open's answer
                // wakes the client after this one's is sent, with no lock held.
                opening.reads.push(Box::new(move |turn| {
                    let refusal = match turn {
                        Turn::Run(found) => {
                            match start_published(
                                found,
                                command,
                                kernel,
                                (read, answer_refused),
                                &stop,
                                &reply,
                                caller,
                            ) {
                                Ok(()) => return,
                                Err(response) => response,
                            }
                        }
                        Turn::Closed(id) => refused(not_open(id)),
                    };
                    // Never run: cancelled, if it was first, or refused, and either way settled.
                    let _ = reply.send(stop.answer(|| None, || refusal));
                }));
                Running::new(answer, READ_STOPPED, Some(ticket), None)
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
        published_answer(
            info,
            read,
            |published, _, _| Ok(published.clone()),
            rebuilds::gated_problems,
        )
    }

    /// `session`'s document's features as the browser and the timeline draw them
    /// (docs/design/m5.0-app.md, section 7's `timeline`, M5.1), each with the health the
    /// model `model` names gave it ([`crate::timeline`]), read as every model is
    /// ([`read_model`](Self::read_model)): the current one queued, waiting for the
    /// revision's rebuild, or the newest published at once, beside the queue, counted among
    /// the session's published reads. The features are the session's document's at its
    /// revision, and the model is the one named, whatever revision it's of: a feature it
    /// hasn't built, changed since or using one that was, is `unknown`, and the result says
    /// the model's revision. Each feature's problems and warnings are those a `check` of the
    /// same model has on it, excepting what that check excepts ([`Checking::excepted`]:
    /// during an open transaction, the drift and the missing fingerprints of what its edits
    /// reached, which its commit writes).
    ///
    /// A document the gate refused is answered, not refused: the document as read, every
    /// feature `unknown`, and the gate's count of problems; text that doesn't read as a
    /// document gives no features, and the reader's count. With no model to read (the
    /// current one, or nothing published), it names none and has no bodies; under a
    /// published model of an older revision, which marks nothing of this document, it names
    /// that model and gives its bodies, as a client still draws them. Otherwise refused as
    /// every read is.
    ///
    /// [`Checking::excepted`]: transaction::Checking::excepted
    pub fn timeline(&self, session: &SessionId, model: ModelChoice) -> Running<TimelineAnswer> {
        self.read_model_or(
            session,
            ReadCommand::Timeline,
            model,
            |read| {
                let _span =
                    tracing::info_span!("timeline", model = read.read.of.as_str()).entered();
                Ok(TimelineAnswer::of(&read))
            },
            |revision, gated| {
                let _span = tracing::info_span!("timeline", refused = true).entered();
                Ok(TimelineAnswer::refused(revision, gated))
            },
        )
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

    /// Ends `session`'s transaction, saving its document to the file at `path` (a Save As,
    /// M5.4; docs/design/m5.0-app.md, open item 22): as [`commit`](Self::commit) saves,
    /// but there, written even if the document is the one `begin` saw, and whatever the
    /// session's own file holds now (a file changed on disk is left as it is). The session
    /// then holds the new path, as given (in a workspace, relative to its root), and claims
    /// its file, letting its old one go. Refused, the transaction left open and nothing
    /// written, as `commit` is, for a document the gate refuses, and as a file a command
    /// writes is (decision 35, as a document): not ending in `.silico`, out of the
    /// workspace or through a link out of it, in a `.silico-cache/` directory, not a file,
    /// or a file there without `replace`; and for the session's own file, one another
    /// session holds or is opening, or a path that isn't UTF-8 (a response couldn't name
    /// it exactly). Its rebuild cache stays the directory's it opened in until it's opened
    /// again.
    pub fn save_as(
        &self,
        session: &SessionId,
        path: impl Into<PathBuf>,
        replace: bool,
    ) -> Running<Committed> {
        let given = match path.into().into_os_string().into_string() {
            Ok(given) => given,
            Err(given) => {
                return Running::answered(refused(
                    Error::new(
                        ErrorCode::InvalidInput,
                        format!(
                            "{} isn't UTF-8, so the API can't name it exactly",
                            shown_path(Path::new(&given))
                        ),
                    )
                    .with_parameter("path")
                    .with_suggestion("give a path that is UTF-8"),
                ));
            }
        };
        let registry = Arc::downgrade(&self.registry);
        self.queued_with(
            session,
            SessionCommand::Commit,
            Rereads::OutsideTransaction,
            move |session, stop, model, _| {
                let id = session.id;
                let reserve = |target: &Path| match registry.upgrade() {
                    Some(registry) => reserve(&registry, id, target),
                    // The sessions are being dropped: no claim is kept.
                    None => Ok(()),
                };
                let unreserve = |target: &Path| {
                    if let Some(registry) = registry.upgrade() {
                        unreserve(&registry, id, target);
                    }
                };
                let holder =
                    |target: &Path| registry.upgrade().and_then(|r| holder(&r, id, target));
                // The other sessions' documents, which it never writes over.
                let held = registry.upgrade().map_or_else(Vec::new, |registry| {
                    lock(&registry.book)
                        .paths
                        .iter()
                        .filter(|(_, holder)| **holder != id)
                        .map(|(path, _)| path.clone())
                        .collect()
                });
                let to = SaveTo {
                    given,
                    replace,
                    held,
                    holder: &holder,
                    reserve: &reserve,
                    unreserve: &unreserve,
                };
                session.commit_as(stop, &to, model, &|resolved| {
                    moved(&registry, id, resolved);
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

    /// Undoes the last edit of `session`'s transaction not undone (docs/design/m5.0-app.md,
    /// section 7), or, with `to`, every edit after that revision's state, made against
    /// `revision` (decision 12): the state before restored at a new revision whose state
    /// equals it, byte for byte, through the gate, what the edits reach worked out again from
    /// it, and its model rebuilt when a command needs it, from the cache. Refused, the
    /// session as it was, with no transaction open, against a revision that isn't the
    /// session's (`conflict`), with nothing to undo (none made, all undone, or the rest
    /// dropped past the stack's bound), and for a `to` the stack doesn't hold, or one that
    /// isn't before the session's state (its own revision among them), naming `to`. Never
    /// touches the file.
    pub fn undo(
        &self,
        session: &SessionId,
        revision: Revision,
        to: Option<Revision>,
    ) -> Running<Stepped> {
        self.queued(session, SessionCommand::Undo, move |session, stop, _| {
            session.step(stop, undo::Way::Undo, revision, to)
        })
    }

    /// Redoes the last edit of `session`'s transaction undone, or, with `to`, every edit
    /// undone up to that revision's state, as [`undo`](Self::undo) undoes them. Refused as
    /// `undo` is, with nothing to redo (none undone, or an edit since dropped what was), and
    /// for a `to` that isn't after the session's state.
    pub fn redo(
        &self,
        session: &SessionId,
        revision: Revision,
        to: Option<Revision>,
    ) -> Running<Stepped> {
        self.queued(session, SessionCommand::Redo, move |session, stop, _| {
            session.step(stop, undo::Way::Redo, revision, to)
        })
    }

    /// Saves `session`'s document in canonical form with every feature's fingerprint and
    /// every sketch's solve as rebuilt, edited or not (decision 31: what `silico fmt` runs,
    /// and how drift is accepted, by name): with `features`, only those features' drift is
    /// accepted, and every other drifted feature keeps what's stored with it and is a
    /// problem. Written through `file::save_over` only if `write` and the text differs from
    /// the file; with `write` false, says whether it would be. Inside a transaction
    /// (docs/design/m5.0-app.md, A6), nothing is written: what it would write is held as the
    /// transaction's document, one undoable edit at a new revision, if it differs and
    /// `write`, for the commit to save. Refused for a document the gate refuses, and for a
    /// feature in `features` the document hasn't; and, writing, as a commit is.
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

    /// Adds `feature`, a feature as the document writes it, to `session`'s timeline before
    /// the feature `before`, or at the end for none (the design's `add_feature`, with
    /// M5.0's `before`), as an edit made against `revision`: its `id` may be left out, for
    /// one made from the system's randomness that no feature holds (decision 20), and its
    /// `v`, for the type's current version; a `fingerprint` is refused (a commit writes
    /// it). Before a feature, each input it consumes that a feature at or after its place
    /// consumed is consumed by it instead, that feature's field rewired to it and listed
    /// (`document::chain::insert`); refused, naming `before`, where that consumer comes
    /// before the place, and naming both where a boolean's two inputs both had consumers.
    /// Inside a transaction, a feature added with the ID of one `begin`'s document (or its
    /// mend) stored a fingerprint for carries that fingerprint, whatever its fields, so the
    /// saved file is a function of the document's state, not of the edits' path: what the
    /// edits reach decides whether it's read, as for a feature never removed.
    /// Through the gate, all at once; inside a transaction, applied, starting no rebuild;
    /// outside one, saved as its own transaction (decision 1). Refused whole, the session as
    /// it was, as every edit is ([`Session::edit`]), for a `before` the document hasn't, and
    /// for a document the gate refused (only `replace_text` mends one).
    pub fn add_feature(
        &self,
        session: &SessionId,
        revision: Revision,
        before: Option<FeatureId>,
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
                        let edit::Inserted {
                            feature,
                            index,
                            rewired,
                            candidate,
                        } = edit::add_feature(
                            base.checked()?,
                            before.as_ref(),
                            feature,
                            base.begun,
                            |document| lock(&ids).next_free(document),
                        )?;
                        Ok(((feature, index, rewired), Some(candidate)))
                    },
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                let (made, index, rewired) = edited.made;
                let id = made.id.clone();
                let feature = as_committed(edited.saved.as_deref(), made, |saved| {
                    saved.feature(&id).ok().cloned()
                })?;
                Ok(Added {
                    feature,
                    index,
                    rewired,
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
                    |base| edit::set_field(base.checked()?.document(), &feature, &field, value),
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

    /// Removes `session`'s feature `feature` (the design's `remove_feature`, as M5.0's A9
    /// amends it), as an edit made against `revision`, giving it back as the document held
    /// it (its fingerprint too), with its place in the timeline: to put it back, give
    /// `add_feature` the feature without its fingerprint, `before` the feature that followed
    /// it (inside a transaction, the fingerprint `begin`'s document stored is kept), then
    /// set `base` back with `set_field` on each rewired entry
    /// whose `to` is none; or `rollback`, of a removal inside a transaction. The feature
    /// that consumed it takes its base, listed in
    /// `rewired` (`document::chain::remove`). Refused, naming both features, where nothing
    /// can stand in: a chain's root under a feature that needs a base, a feature whose
    /// sketch it is, or one that stores names of its elements; and where a feature's base
    /// would no longer be built from a maker of the names it stores (open item 2b);
    /// otherwise as [`add_feature`](Self::add_feature).
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
                        let (removed, candidate) = edit::remove_feature(base.checked()?, &feature)?;
                        Ok((removed, Some(candidate)))
                    },
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                let removed = edited.made;
                Ok(Removed {
                    feature: removed.feature,
                    index: removed.index,
                    rewired: removed.rewired,
                    changed: edited.changed,
                    commit: edited.commit,
                })
            },
        )
    }

    /// Moves `session`'s feature `feature` before the feature `before`, or to the end for
    /// none (docs/design/m5.0-app.md, `move_feature`), as an edit made against `revision`:
    /// a removal then an insert (`document::chain::move_feature`). Its consumer takes its
    /// base, as `remove_feature` rewires it; its base becomes the last member of its chain
    /// before the new place; and the chain's next member after that place takes it as its
    /// base, every change listed in `rewired`. A move to where it is changes nothing: no new
    /// revision, nothing written. Refused, naming both features, for a move before its sketch
    /// or its tool, after a feature that uses it as its sketch, that leaves a chain's root's
    /// consumer without a base, that would give a primitive a base or leave a feature that
    /// needs one without, that takes a feature off a tool's chain past its boolean, or after
    /// which a feature's base is no longer built from a maker of the names it stores (open
    /// item 2b); refused, naming `before`, before itself. A move that leaves a stored name
    /// already broken (its maker never in its base's lineage) before its maker is refused by
    /// the gate, in its words, naming the feature that stores it. Otherwise as
    /// [`add_feature`](Self::add_feature).
    pub fn move_feature(
        &self,
        session: &SessionId,
        revision: Revision,
        feature: FeatureId,
        before: Option<FeatureId>,
    ) -> Running<Moved> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(
            session,
            SessionCommand::MoveFeature,
            move |session, stop, model| {
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| edit::move_feature(base.checked()?, &feature, before.as_ref()),
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                let moved = edited.made;
                Ok(Moved {
                    feature,
                    from: moved.from,
                    to: moved.to,
                    rewired: moved.rewired,
                    changed: edited.changed,
                    commit: edited.commit,
                })
            },
        )
    }

    /// Suppresses `session`'s feature `feature` (`suppressed` true), with its dependents, or
    /// unsuppresses it (false), with what it needs and lacks (docs/design/m5.0-app.md,
    /// `suppress_feature`; `document::suppression`), as an edit made against `revision`.
    /// A feature's dependents are those whose sketch it is, those that store names of its
    /// elements, and those left with an input they need and nothing unsuppressed up its
    /// chain (SolidWorks' children: docs/ux/solidworks.md, §5); a feature suppressed passes
    /// its base through, so what consumes it builds on the nearest unsuppressed feature up
    /// its chain, and its users are reached (M5.0's A10), its own stored fingerprint carried
    /// as stored, never checked or rewritten. Unsuppressing a feature unsuppresses its sketch, each suppressed maker of a name
    /// it stores, and, for an input it needs with nothing unsuppressed up its chain, that
    /// chain's root, recursively; what depends on it stays suppressed. Suppressing a feature
    /// suppressed already, or unsuppressing one that isn't, changes nothing: no new
    /// revision, nothing written. Otherwise as [`add_feature`](Self::add_feature).
    pub fn suppress_feature(
        &self,
        session: &SessionId,
        revision: Revision,
        feature: FeatureId,
        suppressed: bool,
    ) -> Running<Suppressed> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(
            session,
            SessionCommand::SuppressFeature,
            move |session, stop, model| {
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| edit::suppress_feature(base.checked()?, &feature, suppressed),
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                let done = edited.made;
                Ok(Suppressed {
                    feature,
                    suppressed: done.suppressed,
                    unsuppressed: done.unsuppressed,
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
                            edit::replace_text(base.gated, base.text, text.as_bytes())?;
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
        find_in(&self.registry, session)
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
        let (reply, answer) = mpsc::sync_channel(1);
        let caller = tracing::Span::current();
        let registry = Arc::downgrade(&self.registry);
        let ticket = Ticket::new(command);
        let stop = Arc::clone(&ticket);
        let waker = Arc::clone(&self.registry.waker);
        let job: Job<P::Client> = Box::new(move |turn| {
            let response = match turn {
                Turn::Run(session) => {
                    let _caller = caller.enter();
                    let _span =
                        command_span(&session.logs, session.id, command.as_str(), None).entered();
                    // A cancel wakes it from a wait on the session's rebuild (at once, if
                    // one came while it was queued).
                    stop.interrupt_with(Arc::downgrade(session) as Weak<dyn Interrupt>);
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
            waker.wake();
        });
        let newest = match self.queue_on(session, job) {
            Some(newest) => newest,
            None => return Running::answered(refused(not_open(*session))),
        };
        Running::new(answer, SESSION_STOPPED, Some(ticket), Some(newest))
    }

    /// `job` queued on `session`, open or opening (behind its open), and the session's
    /// newest rebuild, which the job's handle reads; none if no session of that ID is open
    /// or opening (`job` dropped, never run).
    fn queue_on(&self, session: &SessionId, job: Job<P::Client>) -> Option<Arc<Newest>> {
        let book = lock(&self.registry.book);
        if let Some(found) = book.sessions.get(session).cloned() {
            // Let go first: the queue's lock comes after the book's, but `queue` runs a job
            // told its session closed at once, and that may take anything.
            drop(book);
            found.queue(job);
            return Some(Arc::clone(&found.newest));
        }
        let opening = book.opening.get(session)?;
        let newest = Arc::clone(&opening.newest);
        // Sent with the book held, so it's in the queue before the session can be recorded
        // open, and before any command sent after.
        match opening.queue.send(job) {
            Ok(()) => {}
            // Its thread gone (it ended without opening, by a panic, before its record went):
            // as never opened.
            Err(mpsc::SendError(job)) => {
                drop(book);
                job(Turn::Closed(*session));
            }
        }
        Some(newest)
    }
}

/// `read`, the published read named `command`, started on `found` on a thread of its own
/// (`session-sN-read`), stopped by `ticket`, answered through `reply`, under the caller's
/// span, or `answer_refused`, for a document the gate refused with nothing published to
/// read ([`published_answer`]): or, refused before it starts, the refusal (the session
/// closed since it was found, [`MAX_PUBLISHED_READS`] unanswered, or no thread to be had),
/// never sent.
fn start_published<C: Kernel + 'static, T: Send + 'static>(
    found: &Arc<Session<C>>,
    command: ReadCommand,
    kernel: Arc<ReadClient<C>>,
    (read, answer_refused): (
        impl FnOnce(Read<'_, C>) -> Result<T, Problems> + Send + 'static,
        impl FnOnce(Revision, &Gated) -> Result<T, Problems> + Send + 'static,
    ),
    ticket: &Arc<Ticket>,
    reply: &mpsc::SyncSender<Response<T>>,
    caller: tracing::Span,
) -> Result<(), Response<T>> {
    let mut reads = lock(&found.reads);
    let Reads {
        unanswered,
        threads,
    } = &mut *reads;
    // Released (closed) since it was found: its reads are the registry's now.
    let Some(threads) = threads else {
        return Err(refused(not_open(found.id)));
    };
    threads.reap();
    // Refusals of an open session say it, as every command's do.
    let refused_here = |error: Error| Response {
        session: Some(found.info(false)),
        model: None,
        result: Err(Problems::from(error)),
    };
    if lock(unanswered).len() >= MAX_PUBLISHED_READS {
        return Err(refused_here(too_many_reads(found.id)));
    }
    let session = Arc::clone(found);
    let counted = Counted::count(unanswered, command, ticket);
    ticket.interrupt_with(Arc::downgrade(&kernel) as Weak<dyn Interrupt>);
    let stop = Arc::clone(ticket);
    let reply = reply.clone();
    let started = std::thread::Builder::new()
        .name(format!("session-{}-read", found.id))
        .spawn(move || {
            let _caller = caller.enter();
            let _span = command_span(
                &session.logs,
                session.id,
                command.as_str(),
                Some("published"),
            )
            .entered();
            #[cfg(test)]
            session.gate.pass();
            let response = stop.answer(
                || Some(session.info(false)),
                || {
                    let (info, published) = session.published_now();
                    let path = session.held.read().path.clone();
                    published_answer(
                        info,
                        published,
                        |published, checking, model| {
                            read(Read {
                                path,
                                model: &published.model,
                                read: model,
                                stop: &stop,
                                kernel: &kernel,
                                reached: &published.reached,
                                checking,
                            })
                        },
                        answer_refused,
                    )
                },
            );
            drop(counted);
            let _ = reply.send(response);
            session.waker.wake();
        });
    match started {
        Ok(thread) => {
            threads.push(thread);
            Ok(())
        }
        // The read, and its count, dropped with the thread that never started.
        Err(e) => Err(refused_here(crate::worker::thread_error(
            "published read",
            &e,
        ))),
    }
}

/// The session `session` of `registry`, if it's open.
fn find_in<P: Clients + 'static>(
    registry: &Registry<P>,
    session: &SessionId,
) -> Option<Arc<Session<P::Client>>> {
    lock(&registry.book).sessions.get(session).cloned()
}

/// [`Sessions::status`], of `registry`'s sessions.
fn status_in<P: Clients + 'static>(
    registry: &Registry<P>,
    session: &SessionId,
) -> Response<Status> {
    let Some(found) = find_in(registry, session) else {
        return refused(not_open(*session));
    };
    // First, alone: the watch's lock is taken last of the session's.
    let disk = found.disk.finding();
    let reads = found.reads_running();
    let running = found.running().and_then(|ticket| ticket.running());
    let held = found.held.read();
    let gated = matches!(*held.gated, Gated::Checked(_));
    let rebuild = {
        let state = lock(&found.rebuilds).reported(held.revision, gated);
        let progress = if gated || state.state == RebuildState::Running {
            found.newest.of(held.revision)
        } else {
            None
        };
        Rebuild { progress, ..state }
    };
    Response {
        session: Some(found.info_of(&held, false)),
        model: None,
        result: Ok(Status {
            features: held.gated.document().ok().map(|d| d.features.len()),
            gated,
            transaction: held.transaction.as_ref().map(Open::status),
            running,
            reads,
            rebuild,
            disk,
        }),
    }
}

/// [`Sessions::read_logs`], of `registry`'s sessions.
fn read_logs_in<P: Clients + 'static>(
    registry: &Registry<P>,
    session: &SessionId,
    query: &LogQuery,
) -> Response<Logs> {
    let Some(found) = find_in(registry, session) else {
        return refused(not_open(*session));
    };
    Response {
        session: Some(found.info(false)),
        model: None,
        result: found.logs.read(found.id, query).map_err(Problems::from),
    }
}

/// The span a command of the session `id` runs in, its events kept in `logs`, written in
/// them as `command` (and `model=` the model it reads, for a read of the published one). At
/// info level: kept by the log layer, which filters nothing, while a log filtered at warn
/// (`silico_core::logging`'s default, a filter of that layer's own) leaves it out.
fn command_span(
    logs: &Arc<LogBuffer>,
    id: SessionId,
    command: &'static str,
    model: Option<&'static str>,
) -> tracing::Span {
    let span = match model {
        Some(model) => tracing::info_span!("session_command", session = %id, command, model),
        None => tracing::info_span!("session_command", session = %id, command),
    };
    let label = match model {
        Some(model) => format!("{command} model={model}"),
        None => command.to_owned(),
    };
    logs.attach(&span, label);
    span
}

/// [`Sessions::cancel`], of `registry`'s sessions.
fn cancel_in<P: Clients + 'static>(
    registry: &Registry<P>,
    session: &SessionId,
    named: Cancellable,
) -> Response<Cancelled> {
    let Some(found) = find_in(registry, session) else {
        return refused(not_open(*session));
    };
    let _span = command_span(&found.logs, found.id, "cancel", None).entered();
    let result = match named {
        Cancellable::Command(command) => found.cancel_command(command),
        Cancellable::Rebuild => found.cancel_rebuild(),
    };
    Response {
        session: Some(found.info(false)),
        model: None,
        result,
    }
}

impl<C: Kernel + 'static> Session<C> {
    /// `cancel` naming `command`: the queued command, if it's the one running, and every
    /// published read of that name, each stopped; or, if none is, refused, naming what's
    /// running.
    fn cancel_command(&self, command: SessionCommand) -> Result<Cancelled, Problems> {
        let running = self.running_tickets();
        let named: Vec<&Arc<Ticket>> = running
            .iter()
            .filter(|ticket| ticket.command() == Some(command))
            .collect();
        if named.is_empty() {
            return Err(self.not_running(Cancellable::Command(command), &running));
        }
        // Every one stopped, none passed over once one is.
        let stopped = named
            .iter()
            .fold(false, |stopped, ticket| ticket.cancel() | stopped);
        tracing::info!(command = command.as_str(), stopped, "cancelled");
        Ok(Cancelled {
            command: Cancellable::Command(command),
            stopped,
        })
    }

    /// `cancel` naming `rebuild`: the rebuild `status` reports running stopped, under the
    /// rebuilds' lock, as a cancelled waiter stops it, and the command waiting on it, if
    /// one is, told, so it answers `cancelled` (decision 34); the queued command running,
    /// if it isn't waiting yet, answers so when it asks for a model. Refused, naming what's
    /// running, if none is (it published, failed, or was superseded first).
    fn cancel_rebuild(&self) -> Result<Cancelled, Problems> {
        let stopped = {
            let held = self.held.read();
            let gated = matches!(*held.gated, Gated::Checked(_));
            let mut rebuilds = lock(&self.rebuilds);
            // The queued command running as the rebuild stops, read under the rebuilds'
            // lock (the command's lock comes after it, and is taken alone): it answers
            // `cancelled` too when it then asks for the model, waiting on the rebuild or
            // not yet. A command that starts after this asks for the model only once the
            // stop is recorded, and starts the rebuild again, as any command after does.
            let queued = self.running().map(|ticket| Arc::downgrade(&ticket));
            rebuilds.stop_running(held.revision, gated, queued)
        };
        if !stopped {
            return Err(self.not_running(Cancellable::Rebuild, &self.running_tickets()));
        }
        // Told once the stop is recorded, so a waiter that saw none before it waited is
        // waiting when it's told.
        self.rebuilt.notify_all();
        self.waker.wake();
        tracing::info!(command = "rebuild", stopped, "cancelled");
        Ok(Cancelled {
            command: Cancellable::Rebuild,
            stopped: true,
        })
    }

    /// The stops of the commands running, as `status` lists them: the queued command, then
    /// each published read.
    fn running_tickets(&self) -> Vec<Arc<Ticket>> {
        let queued = self.running();
        let reads = lock(&self.reads);
        let unanswered = lock(&reads.unanswered);
        queued
            .into_iter()
            .chain(unanswered.tickets().cloned())
            .collect()
    }

    /// The refusal of a `cancel` of `named`, which isn't running: what is, as `status`
    /// reports it, the commands of `tickets` ([`running_tickets`](Self::running_tickets)),
    /// then the rebuild, if `status.rebuild.state` is `running`.
    fn not_running(&self, named: Cancellable, tickets: &[Arc<Ticket>]) -> Problems {
        let mut running: Vec<Cancellable> = tickets
            .iter()
            .filter_map(|ticket| ticket.command())
            .map(Cancellable::Command)
            .collect();
        if self.rebuild_running() {
            running.push(Cancellable::Rebuild);
        }
        Problems::from(not_running(self.id, named, &running))
    }

    /// Whether `status` reports the session's rebuild `running`: its revision's, while the
    /// gate takes its document, or an edit's candidate, whatever it holds.
    fn rebuild_running(&self) -> bool {
        let held = self.held.read();
        let gated = matches!(*held.gated, Gated::Checked(_));
        lock(&self.rebuilds).reported(held.revision, gated).state == RebuildState::Running
    }
}

/// `status`, `read_logs`, and `cancel` of a [`Sessions`], answered later, on whichever thread
/// asks ([`Sessions::weak`]): as the sessions answer them, or, once they're dropped, refused
/// as of sessions closed ([`gone`]). It holds them only weakly, so it keeps nothing of
/// theirs alive.
pub(crate) struct WeakSessions<P: Clients + 'static>(Weak<Registry<P>>);

impl<P: Clients + 'static> WeakSessions<P> {
    /// [`Sessions::status`].
    pub(crate) fn status(&self, session: &SessionId) -> Response<Status> {
        match self.0.upgrade() {
            Some(registry) => status_in(&registry, session),
            None => refused(gone()),
        }
    }

    /// `session`'s state, as a refusal of one of its commands carries it: none if it isn't
    /// open, or the sessions are dropped. Read as `status` reads it, on the asking thread,
    /// from what the session holds, never taking a place in its queue.
    pub(crate) fn info(&self, session: &SessionId) -> Option<SessionInfo> {
        let registry = self.0.upgrade()?;
        find_in(&registry, session).map(|found| found.info(false))
    }

    /// [`Sessions::read_logs`].
    pub(crate) fn read_logs(&self, session: &SessionId, query: &LogQuery) -> Response<Logs> {
        match self.0.upgrade() {
            Some(registry) => read_logs_in(&registry, session, query),
            None => refused(gone()),
        }
    }

    /// [`Sessions::cancel`].
    pub(crate) fn cancel(&self, session: &SessionId, named: Cancellable) -> Response<Cancelled> {
        match self.0.upgrade() {
            Some(registry) => cancel_in(&registry, session, named),
            None => refused(gone()),
        }
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

/// The refusal of a published read past the [`MAX_PUBLISHED_READS`] session `id` runs at
/// once, or holds while it opens.
fn too_many_reads(id: SessionId) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!(
            "{MAX_PUBLISHED_READS} published reads are running in session {id}, the most one session runs at once"
        ),
    )
    .with_suggestion("wait for one to answer, then read again")
}

fn not_open(id: SessionId) -> Error {
    Error::new(ErrorCode::InvalidInput, format!("no session {id} is open"))
        .with_parameter("session")
        .with_suggestion(
            "open the document again: a session ends when it's closed or its client goes away",
        )
}

/// The refusal of `status`, `read_logs`, or `cancel` answered after the sessions were dropped
/// ([`WeakSessions`]): every session closed with them, so there's none to report on or stop.
fn gone() -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        "this process's sessions are closing, so none is open",
    )
    .with_parameter("session")
    .with_suggestion("open the document again once the sessions are made again")
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
