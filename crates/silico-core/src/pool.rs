//! A pool of kernel workers (ARCHITECTURE.md, Process model): up to as many as the machine
//! has cores, each in a slot served by its own thread, taking requests from one queue, and
//! started as requests need them ([`KernelPool::start_on_demand`]; [`KernelPool::start_workers`]
//! starts them all at once).
//! Workers keep no state, so any worker can run any request, and a request's result doesn't
//! depend on which worker ran it or when.
//!
//! A worker that crashes, hangs, or sends a reply that can't be trusted is stopped, its
//! request fails with a structured error, and a new worker takes its place. The request is
//! never retried: the same input would likely crash the new worker too. A worker that has
//! died between requests (killed from outside, say) is replaced before it's given one, and
//! an idle thread checks its worker every [`IDLE_CHECK`], so each started slot returns to a
//! live worker on its own. (A worker still in the moment of exiting can't be told from a live one: a
//! request given to it then fails as a crash, as any request whose worker is killed under
//! it does.)
//!
//! Every request carries a generation of the client that submitted it: the pool's own
//! ([`ClientId::KERNEL`], through the pool's [`Kernel`](crate::kernel::Kernel)), or a
//! [`PoolClient`]'s ([`KernelPool::client`]), each with generations of its own. When a client
//! starts a newer generation ([`KernelPool::advance`], [`PoolClient`]'s `advance`), its
//! requests of older ones still waiting are dropped without running, and those running are
//! stopped (their workers killed and replaced) once they've run the pool's stop-after
//! ([`STOP_AFTER`] unless set): at once if they already have, and otherwise when they do,
//! unless they finish first. Their callers get [`Outcome::Superseded`], never a result. No
//! other client's request is touched: one document's rebuild never stops another's work.
//! Dropping a client retires it, superseding its requests in the same way.
//!
//! The pool serves its clients in turn: the request a slot takes next is the first waiting
//! of the next client after the one whose request was taken last, round the clients in
//! order, so one client's long queue never keeps another's request waiting behind all of it.

use std::collections::{BTreeMap, VecDeque};
use std::ops::Bound;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use silico_protocol::Generation;

use crate::kernel::{ClientId, Clients, KernelRequest};
use crate::worker::{Executable, Worker, WorkerKiller, WorkerKind, lock};
use crate::{Error, ErrorCode};

/// How often an idle thread checks that its worker is still alive.
pub const IDLE_CHECK: Duration = Duration::from_secs(1);

/// The longest an idle thread waits before starting a worker again, after workers that
/// ended on their own or before they started, or couldn't be started (the wait doubles
/// from [`IDLE_CHECK`] each time).
const MAX_RETRY: Duration = Duration::from_secs(60);

/// How long an idle thread gives a worker whose socket closed to exit, to tell one that's
/// ending (and how) from one that's running on with its socket closed.
const HUNG_UP_GRACE: Duration = Duration::from_millis(100);

/// How long a superseded request may run, counted from its start, before the pool stops it,
/// unless [`KernelPool::set_stop_after`] says otherwise. Replacing a stopped worker takes
/// about 30 ms of wall time and CPU (noticing it has ended, and starting another), so
/// stopping a request costs more than letting it finish when less than that is left, as it
/// is for most operations on simple parts (a box or a mesh under a millisecond, a fillet of
/// twelve edges about 9 ms). One that has run this long is likely to run longer. So a
/// superseded request holds its worker at most this long from its start, and the worker is
/// then replaced.
pub const STOP_AFTER: Duration = Duration::from_millis(50);

/// What the pool calls itself in its errors: a [`Pending`] stopped early, a thread that
/// couldn't start.
const KERNEL: &str = "kernel pool";

/// The most workers a pool may have: far more than any machine's cores, and few enough
/// that a size from a setting can't make the pool try to start without end.
pub const MAX_WORKERS: usize = 1024;

/// Kernel workers, and a queue of requests for them. Dropping the pool stops its workers at
/// once (requests still waiting or running end without a result) and joins its threads.
pub struct KernelPool {
    shared: Arc<Shared>,
    /// How many slots the pool has.
    size: usize,
    /// Stops superseded requests when they're due.
    stopper: Option<JoinHandle<()>>,
}

/// A client of a [`KernelPool`] ([`KernelPool::client`]): a handle with generations of its
/// own, whose advances supersede only the requests it submitted, and whose waiting requests
/// the pool serves in turn with the other clients'. Dropping it retires the client: its
/// requests still waiting come to [`Outcome::Superseded`] at once, and those running are
/// stopped once they've run the pool's stop-after, as superseded requests are. It may outlive
/// its pool: its requests then end without a result, as the pool's do once it's dropped.
pub struct PoolClient {
    shared: Arc<Shared>,
    id: ClientId,
}

/// What a request came to.
#[derive(Debug)]
pub enum Outcome<T> {
    /// The request ran: its result, or why it failed.
    Done(Result<T, Error>),
    /// A newer generation started first: the request didn't run, was stopped, or its result
    /// was discarded.
    Superseded,
}

impl<T> Outcome<T> {
    /// The result, if the request wasn't superseded.
    pub fn done(self) -> Option<Result<T, Error>> {
        match self {
            Outcome::Done(result) => Some(result),
            Outcome::Superseded => None,
        }
    }
}

/// A request submitted to a [`Kernel`](crate::kernel::Kernel), whose outcome comes when the
/// kernel has run it.
#[must_use = "a request's outcome is only had by waiting for it"]
pub struct Pending<T> {
    name: &'static str,
    /// What ran it, for the error if it stops first: `kernel pool`, `fake kernel`.
    kernel: &'static str,
    /// Whether a client of the kernel submitted it ([`Clients::client`]), not the kernel's
    /// own [`Kernel`](crate::kernel::Kernel): what the error if it stops first suggests.
    client: bool,
    outcome: Receiver<Outcome<T>>,
}

impl<T> Pending<T> {
    /// A request named `name`, whose outcome comes on `outcome` from `kernel`.
    pub(crate) fn new(
        name: &'static str,
        kernel: &'static str,
        outcome: Receiver<Outcome<T>>,
    ) -> Self {
        Self {
            name,
            kernel,
            client: false,
            outcome,
        }
    }

    /// A request of a client of `kernel` ([`Clients::client`]), named `name`, whose outcome
    /// comes on `outcome`: if the kernel stops first, the error suggests taking a client of a
    /// running one.
    pub(crate) fn of_client(
        name: &'static str,
        kernel: &'static str,
        outcome: Receiver<Outcome<T>>,
    ) -> Self {
        Self {
            client: true,
            ..Self::new(name, kernel, outcome)
        }
    }

    /// The outcome of the request once its kernel has stopped before running it.
    fn stopped(&self) -> Outcome<T> {
        Outcome::Done(Err(stopped(self.kernel, self.name, self.client)))
    }

    /// Waits for the outcome.
    pub fn wait(self) -> Outcome<T> {
        match self.outcome.recv() {
            Ok(outcome) => outcome,
            Err(_) => self.stopped(),
        }
    }

    /// Waits up to `timeout` for the outcome, or gives the request back to wait on again.
    pub fn wait_timeout(self, timeout: Duration) -> Result<Outcome<T>, Self> {
        match self.outcome.recv_timeout(timeout) {
            Ok(outcome) => Ok(outcome),
            Err(RecvTimeoutError::Timeout) => Err(self),
            Err(RecvTimeoutError::Disconnected) => Ok(self.stopped()),
        }
    }

    /// The outcome, if it has come, or the request back to wait on.
    pub fn try_take(self) -> Result<Outcome<T>, Self> {
        match self.outcome.try_recv() {
            Ok(outcome) => Ok(outcome),
            Err(TryRecvError::Empty) => Err(self),
            Err(TryRecvError::Disconnected) => Ok(self.stopped()),
        }
    }
}

/// The error of a request whose kernel stopped before running it; `client`, whether a
/// client of the kernel submitted it (a client of a stopped kernel stays stopped, so it's
/// another client, of a running kernel, that can run it).
fn stopped(kernel: &str, name: &str, client: bool) -> Error {
    let suggestion = if client {
        format!("take a client of a running {kernel}")
    } else {
        format!("submit the request to a running {kernel}")
    };
    Error::new(
        ErrorCode::Internal,
        format!("the {kernel} stopped before {name} was done"),
    )
    .with_suggestion(suggestion)
}

impl KernelPool {
    /// A pool of as many kernel workers as the machine has cores (at most [`MAX_WORKERS`]),
    /// each the one [`KERNEL_WORKER_ENV`](crate::kernel::KERNEL_WORKER_ENV) names or
    /// `silico-kernel-worker` next to the running executable, as
    /// [`KernelWorker::start`](crate::kernel::KernelWorker::start) finds it, started on
    /// demand ([`start_on_demand`](Self::start_on_demand)): a rebuild pays to start only
    /// the workers it uses at once, and one from the cache none.
    pub fn start() -> Result<Self, Error> {
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        Self::start_on_demand(&crate::kernel::worker_path()?, cores.min(MAX_WORKERS))
    }

    /// Starts `size` workers (1 to [`MAX_WORKERS`]) of the executable at `exe`, which is
    /// found once, now, so replacements come from the same place whatever the working
    /// directory later. Fails if any can't be started, and then starts none.
    pub fn start_workers(exe: &Path, size: usize) -> Result<Self, Error> {
        Self::open(exe, size, Starts::Now)
    }

    /// A pool of `size` slots (1 to [`MAX_WORKERS`]) for workers of the executable at
    /// `exe`, found once, now, as [`start_workers`](Self::start_workers) finds it, none
    /// started yet: a slot starts, and starts its worker, when the requests waiting
    /// outnumber the started slots free to take them ([`grow`]), and keeps it
    /// as `start_workers`'s slots do. Starting a worker
    /// costs a process and OCCT's libraries (on a loaded Linux machine, a large part of a
    /// small document's rebuild), so a slot no request needs costs nothing. A worker that
    /// can't be started fails the request that started it, as a crashed one does.
    pub fn start_on_demand(exe: &Path, size: usize) -> Result<Self, Error> {
        Self::open(exe, size, Starts::OnDemand)
    }

    fn open(exe: &Path, size: usize, starts: Starts) -> Result<Self, Error> {
        if !(1..=MAX_WORKERS).contains(&size) {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!("a kernel pool needs 1 to {MAX_WORKERS} workers, not {size}"),
            ));
        }
        let exe = std::path::absolute(exe).map_err(|e| {
            Error::new(
                ErrorCode::Internal,
                format!("couldn't find the kernel worker {}", exe.display()),
            )
            .with_cause(e.to_string())
        })?;
        let shared = Arc::new(Shared {
            executable: Executable::of(&exe, WorkerKind::Kernel)?,
            state: Mutex::new(State {
                generations: BTreeMap::from([(ClientId::KERNEL, Generation(0))]),
                queues: BTreeMap::new(),
                queued: 0,
                turn: None,
                next_client: 1,
                stopping: false,
                stop_after: STOP_AFTER,
                started: 0,
                busy: 0,
                unstarted: match starts {
                    Starts::Now => Vec::new(),
                    Starts::OnDemand => (0..size).rev().collect(),
                },
            }),
            work: Condvar::new(),
            due: Condvar::new(),
            spawns: AtomicU64::new(1),
            newest_started: AtomicU64::new(0),
            slots: Mutex::new((0..size).map(|_| Slot::default()).collect()),
            threads: Mutex::new(Vec::with_capacity(size)),
        });
        let mut pool = Self {
            shared,
            size,
            stopper: None,
        };
        let shared = Arc::clone(&pool.shared);
        pool.stopper = Some(spawn_thread("kernel-pool-stopper".to_owned(), move || {
            stop_when_due(&shared);
        })?);
        if let Starts::OnDemand = starts {
            // Slots start as requests need them (`grow`).
            tracing::debug!(size, exe = %exe.display(), "opened kernel pool, its workers on demand");
            return Ok(pool);
        }
        // Each thread starts its worker (one at a time: see `Worker::spawn`); the pool is
        // ready when all have.
        let (ready_tx, ready) = mpsc::channel();
        lock(&pool.shared.state).started = size;
        for slot in 0..size {
            let thread = spawn_slot(&pool.shared, slot, SlotStart::Now(ready_tx.clone()))?;
            lock(&pool.shared.threads).push(thread);
        }
        drop(ready_tx);
        for _ in 0..size {
            match ready.recv() {
                Ok(Ok(())) => {}
                // Dropping the pool stops the workers that did start.
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(Error::new(
                        ErrorCode::Internal,
                        "a kernel pool thread ended before its worker started",
                    ));
                }
            }
        }
        tracing::debug!(size, exe = %exe.display(), "started kernel pool");
        Ok(pool)
    }

    /// How many slots the pool has: the most workers it holds at once (on demand, fewer
    /// until requests need them: see [`pids`](Self::pids)).
    pub fn size(&self) -> usize {
        self.size
    }

    /// The process IDs of the pool's workers, one per worker it holds (a worker being
    /// replaced has none for a moment). For tests and diagnostics.
    pub fn pids(&self) -> Vec<u32> {
        lock(&self.shared.slots)
            .iter()
            .filter_map(|slot| slot.killer.as_ref())
            .map(WorkerKiller::pid)
            .collect()
    }

    /// The generation the pool's own requests are for ([`ClientId::KERNEL`]'s): the newest
    /// [`advance`](Self::advance) started. Each [`PoolClient`] has its own.
    pub fn generation(&self) -> Generation {
        self.shared.generation(ClientId::KERNEL)
    }

    /// How long a superseded request may run, from its start, before the pool stops it
    /// ([`STOP_AFTER`] unless set here), for requests running now as well as later ones.
    /// Zero stops every superseded request at once. For tests, and for tuning.
    pub fn set_stop_after(&self, after: Duration) {
        lock(&self.shared.state).stop_after = after;
        self.shared.due.notify_all();
    }

    /// Starts `generation` for the pool's own requests ([`ClientId::KERNEL`]'s), superseding
    /// every older one of theirs: those still waiting are dropped without running, and those
    /// running are stopped (their workers killed and replaced) once they've run the pool's
    /// stop-after, now if they already have; their results are discarded. A generation no
    /// newer than the current one changes nothing. No [`PoolClient`]'s request is touched.
    pub fn advance(&self, generation: Generation) {
        self.shared.advance(ClientId::KERNEL, generation);
    }

    /// Queues `request` for `generation` of the pool's own ([`ClientId::KERNEL`]'s), to run
    /// on the next free worker when it's their turn. A request for a generation older than
    /// the current one is superseded at once. The request runs in the tracing span current
    /// here, so its worker's call and logs trace back to whatever submitted it
    /// (ARCHITECTURE.md, invariant 9).
    pub fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        self.shared.submit(ClientId::KERNEL, generation, request)
    }

    /// A new client of the pool, at generation 0, numbered after the last one the pool gave
    /// ([`Clients::client`]).
    pub fn client(&self) -> PoolClient {
        let mut state = lock(&self.shared.state);
        // Never given again: 2^64 clients can't be made in any run.
        let id = ClientId::new(state.next_client);
        state.next_client = state.next_client.saturating_add(1);
        state.generations.insert(id, Generation(0));
        drop(state);
        tracing::debug!(client = id.number(), "kernel pool client started");
        PoolClient {
            shared: Arc::clone(&self.shared),
            id,
        }
    }
}

impl crate::kernel::Kernel for KernelPool {
    fn identity(&self) -> &str {
        self.shared.executable.identity()
    }

    fn generation(&self) -> Generation {
        KernelPool::generation(self)
    }

    fn advance(&self, generation: Generation) {
        KernelPool::advance(self, generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        KernelPool::submit(self, generation, request)
    }
}

impl Clients for KernelPool {
    type Client = PoolClient;

    fn client(&self) -> PoolClient {
        KernelPool::client(self)
    }
}

impl PoolClient {
    /// The client it is.
    pub fn id(&self) -> ClientId {
        self.id
    }
}

impl crate::kernel::Kernel for PoolClient {
    fn identity(&self) -> &str {
        self.shared.executable.identity()
    }

    fn generation(&self) -> Generation {
        self.shared.generation(self.id)
    }

    fn advance(&self, generation: Generation) {
        self.shared.advance(self.id, generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        self.shared.submit(self.id, generation, request)
    }
}

impl Drop for PoolClient {
    fn drop(&mut self) {
        let _span = tracing::debug_span!("retire_client", client = self.id.number()).entered();
        let waiting = {
            let mut state = lock(&self.shared.state);
            state.generations.remove(&self.id);
            let waiting = state.queues.remove(&self.id).unwrap_or_default();
            state.queued -= waiting.len();
            if !state.stopping {
                // Its requests running are superseded now: those that have run the
                // stop-after are stopped at once, the rest by the stopper when they're due.
                self.shared.stop_superseded(&state);
            }
            waiting
        };
        self.shared.due.notify_all();
        for task in waiting {
            task.supersede();
        }
    }
}

impl Drop for KernelPool {
    fn drop(&mut self) {
        let waiting: Vec<Box<dyn Task>> = {
            let mut state = lock(&self.shared.state);
            state.stopping = true;
            state.take_all()
        };
        // Requests that never ran end without a result.
        drop(waiting);
        self.shared.work.notify_all();
        self.shared.due.notify_all();
        // Requests running end at once: their workers are killed, and their results dropped.
        for slot in lock(&self.shared.slots).iter() {
            if let Some(killer) = &slot.killer {
                killer.kill();
            }
        }
        self.shared.join_threads();
        if let Some(stopper) = self.stopper.take() {
            let _ = stopper.join();
        }
    }
}

/// Starts slot `slot`'s thread, which starts its worker as `start` says.
fn spawn_slot(
    shared: &Arc<Shared>,
    slot: usize,
    start: SlotStart,
) -> Result<JoinHandle<()>, Error> {
    let shared = Arc::clone(shared);
    spawn_thread(format!("kernel-pool-{slot}"), move || {
        serve(slot, &shared, start);
    })
}

/// Starts another slot, on demand, if the requests waiting are more than the slots free to
/// take them, and the pool has a slot not yet started: as a request is queued, and as a
/// slot's thread ends ([`SlotEnded`]).
fn grow(shared: &Arc<Shared>) {
    let slot = {
        let mut state = lock(&shared.state);
        let free = state.started - state.busy;
        if state.stopping || state.queued <= free {
            return;
        }
        let Some(slot) = state.unstarted.pop() else {
            return;
        };
        state.started += 1;
        slot
    };
    match spawn_slot(shared, slot, SlotStart::OnDemand) {
        Ok(thread) => lock(&shared.threads).push(thread),
        Err(e) => {
            // The system is short of threads: the slots started take the requests in turn,
            // or, if there are none, the requests fail with it. The slot is left to start
            // later, as itself.
            let mut state = lock(&shared.state);
            state.started -= 1;
            state.unstarted.push(slot);
            let none = state.started == 0;
            let waiting: Vec<Box<dyn Task>> = if none { state.take_all() } else { Vec::new() };
            drop(state);
            for task in waiting {
                task.fail(e.clone());
            }
        }
    }
}

/// Starts a pool thread.
fn spawn_thread(
    name: String,
    run: impl FnOnce() + Send + 'static,
) -> Result<JoinHandle<()>, Error> {
    std::thread::Builder::new()
        .name(name)
        .spawn(run)
        .map_err(|e| thread_error(KERNEL, &e))
}

/// A thread of `kernel` (`kernel pool`, `fake kernel`) that couldn't start: the system short
/// of threads (`resource_exhausted`), or a bug.
pub(crate) fn thread_error(kernel: &str, e: &std::io::Error) -> Error {
    let message = format!("couldn't start a {kernel} thread");
    crate::worker::shortage(e, &message).unwrap_or_else(|| {
        Error::new(ErrorCode::Internal, message)
            .with_cause(e.to_string())
            .with_suggestion(crate::worker::SILICO_BUG)
    })
}

/// What the pool's threads share. Its locks are taken in the order `state`, then `slots`
/// (then a worker's process lock), never the other way; the executable's stamp lock
/// (`Executable`) is taken alone, never while another is.
struct Shared {
    /// The workers' executable: the pool's identity is its identity, as the pool started,
    /// and a worker is started from it, and taken, only while it's still that build.
    executable: Arc<Executable>,
    state: Mutex<State>,
    /// Signalled when a request is queued or the pool stops.
    work: Condvar,
    /// Signalled when the generation or the stop-after changes, or the pool stops: for the
    /// stopper.
    due: Condvar,
    /// What each slot holds and runs, to kill its worker when the pool stops or its request
    /// is superseded, and to report its process ID.
    slots: Mutex<Vec<Slot>>,
    /// One per slot's thread started: every slot for [`KernelPool::start_workers`], those
    /// requests have needed so far for [`KernelPool::start_on_demand`], and those started
    /// again as their threads ended. Locked alone, never with another lock.
    threads: Mutex<Vec<JoinHandle<()>>>,
    /// The next number to give a worker the pool starts (from 1).
    spawns: AtomicU64,
    /// The number of the newest worker known to have started (0 for none). Once a worker
    /// started after a slot's failure has, the binary works: the slot starts one now.
    newest_started: AtomicU64,
}

struct State {
    /// Each client's generation, the pool's own ([`ClientId::KERNEL`]) always among them. A
    /// client absent is retired: whatever it submitted is superseded.
    generations: BTreeMap<ClientId, Generation>,
    /// Each client's requests waiting, in the order it submitted them; only clients with
    /// some.
    queues: BTreeMap<ClientId, VecDeque<Box<dyn Task>>>,
    /// How many requests wait, over every client's queue.
    queued: usize,
    /// The client whose request a slot took last: the next is the first waiting of the next
    /// client after it with one.
    turn: Option<ClientId>,
    /// The number the next client gets.
    next_client: u64,
    stopping: bool,
    stop_after: Duration,
    /// How many slots have been started, and how many of them hold a request whose outcome
    /// isn't delivered yet: the rest are free to take one (waiting, tending their worker, or
    /// replacing it).
    started: usize,
    busy: usize,
    /// The slots not started yet, the lowest last.
    unstarted: Vec<usize>,
}

impl State {
    /// Whether a request of `client`'s for `generation` is superseded: the client has
    /// started a newer generation, or is retired.
    fn superseded(&self, client: ClientId, generation: Generation) -> bool {
        self.generations
            .get(&client)
            .is_none_or(|&current| generation < current)
    }

    /// The next request to take: the first waiting of the next client after the one whose
    /// request was taken last, round the clients in order, so each client's turn comes.
    fn next(&mut self) -> Option<Box<dyn Task>> {
        let after = self.turn.map_or(Bound::Unbounded, Bound::Excluded);
        let client = self
            .queues
            .range((after, Bound::Unbounded))
            .next()
            .or_else(|| self.queues.first_key_value())
            .map(|(&client, _)| client)?;
        let queue = self.queues.get_mut(&client)?;
        let task = queue.pop_front()?;
        if queue.is_empty() {
            self.queues.remove(&client);
        }
        self.queued -= 1;
        self.turn = Some(client);
        Some(task)
    }

    /// Every request waiting, taken from the queues, client by client.
    fn take_all(&mut self) -> Vec<Box<dyn Task>> {
        self.queued = 0;
        std::mem::take(&mut self.queues)
            .into_values()
            .flatten()
            .collect()
    }
}

#[derive(Default)]
struct Slot {
    killer: Option<WorkerKiller>,
    activity: Activity,
}

/// What a slot's worker is doing, changed only under the slots' lock.
#[derive(Default, Clone, Copy)]
enum Activity {
    #[default]
    Idle,
    /// Running a request of `client`'s `generation`, since `since`.
    Running {
        client: ClientId,
        generation: Generation,
        since: Instant,
    },
    /// Its request was superseded and stopped: its worker killed, if `killed`, or found
    /// already dead (its request's failure its own). The slot must replace the worker before
    /// giving it another request.
    Condemned { killed: bool },
}

impl Activity {
    /// Whether the slot's worker was condemned, and must be replaced.
    fn is_condemned(self) -> bool {
        matches!(self, Activity::Condemned { .. })
    }
}

impl Shared {
    fn stopping(&self) -> bool {
        lock(&self.state).stopping
    }

    /// `client`'s generation: the newest it started.
    fn generation(&self, client: ClientId) -> Generation {
        let state = lock(&self.state);
        // Only a live client asks, and a live client's generation is there: a client is
        // retired only by its own handle's drop.
        debug_assert!(
            state.generations.contains_key(&client),
            "client {} asked for its generation once retired",
            client.number()
        );
        state
            .generations
            .get(&client)
            .copied()
            .unwrap_or(Generation(0))
    }

    /// Whether a request of `client`'s for `generation` is superseded ([`State::superseded`]).
    fn superseded(&self, client: ClientId, generation: Generation) -> bool {
        lock(&self.state).superseded(client, generation)
    }

    /// Starts `generation` for `client`, superseding its older requests: those waiting are
    /// dropped without running, and those running are stopped once they've run the
    /// stop-after. Every other client's requests are left as they are.
    fn advance(&self, client: ClientId, generation: Generation) {
        let superseded: VecDeque<Box<dyn Task>> = {
            let mut state = lock(&self.state);
            match state.generations.get_mut(&client) {
                Some(current) if generation > *current => *current = generation,
                // Not newer, or the client is retired.
                _ => return,
            }
            let queue = state.queues.remove(&client).unwrap_or_default();
            let (stale, current): (VecDeque<_>, VecDeque<_>) = queue
                .into_iter()
                .partition(|task| task.generation() < generation);
            if !current.is_empty() {
                state.queues.insert(client, current);
            }
            state.queued -= stale.len();
            self.stop_superseded(&state);
            stale
        };
        // The rest are the stopper's, when they're due.
        self.due.notify_all();
        for task in superseded {
            task.supersede();
        }
    }

    /// Queues `request`, `client`'s, for `generation`, to run on the next free worker when
    /// it's the client's turn; superseded at once if `generation` is older than the
    /// client's, or the client is retired; ended without a result if the pool is stopping.
    fn submit<T: Send + 'static>(
        self: &Arc<Self>,
        client: ClientId,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        let name = request.name();
        let (reply, outcome) = mpsc::sync_channel(1);
        let pending = if client == ClientId::KERNEL {
            Pending::new(name, KERNEL, outcome)
        } else {
            Pending::of_client(name, KERNEL, outcome)
        };
        let task = Box::new(Submitted {
            client,
            generation,
            request,
            reply,
            span: tracing::Span::current(),
        });
        let mut state = lock(&self.state);
        if state.stopping {
            // Dropping the task tells the caller the pool stopped.
            return pending;
        }
        if state.superseded(client, generation) {
            drop(state);
            task.supersede();
            return pending;
        }
        state.queues.entry(client).or_default().push_back(task);
        state.queued += 1;
        drop(state);
        self.work.notify_one();
        grow(self);
        // A client's submit can race the pool's drop (the pool's own can't: it borrows the
        // pool), and `grow` may have started a slot's thread after the drop's last look at
        // them. That thread starts no worker (it finds the pool stopping first), and is
        // joined here; one listed before the drop's last look, the drop joins.
        if self.stopping() {
            self.join_threads();
        }
        pending
    }

    /// Joins the slots' threads, until none are left: a slot whose thread ended just before
    /// the stop may have started its slot again (none starts once the pool is stopping, but
    /// for the one a racing submit's [`grow`] started, whose submit joins it). Only once the
    /// pool is stopping, and never from a slot's own thread.
    fn join_threads(&self) {
        loop {
            let threads = std::mem::take(&mut *lock(&self.threads));
            if threads.is_empty() {
                break;
            }
            for thread in threads {
                let _ = thread.join();
            }
        }
    }

    /// The next request for a thread, waiting up to [`IDLE_CHECK`] for one.
    fn next(&self) -> Next {
        let mut state = lock(&self.state);
        loop {
            if state.stopping {
                return Next::Stop;
            }
            if let Some(task) = state.next() {
                state.busy += 1;
                return Next::Task(task);
            }
            let (next, timeout) = self
                .work
                .wait_timeout(state, IDLE_CHECK)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
            if timeout.timed_out() && !state.stopping && state.queued == 0 {
                return Next::Idle;
            }
        }
    }

    fn with_slot<R>(&self, slot: usize, f: impl FnOnce(&mut Slot) -> R) -> Option<R> {
        lock(&self.slots).get_mut(slot).map(f)
    }

    /// Ends the slot's request, however it ended: the slot is idle again. Gives what it was
    /// doing; if [`Activity::Condemned`], its worker must be replaced. Every path that ends a
    /// request must call this, or a slot left marked running an old generation's request
    /// would have its next worker killed under its next request.
    fn end(&self, slot: usize) -> Activity {
        self.with_slot(slot, |s| std::mem::take(&mut s.activity))
            .unwrap_or_default()
    }

    /// Stops the superseded requests (each of a generation older than its own client's, or of
    /// a retired client) that have run the stop-after, and says when the next of those still
    /// running will have.
    fn stop_superseded(&self, state: &State) -> Option<Instant> {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        // A slot marks its request running, and ends it, under this lock, so a worker killed
        // here is one still on a superseded request; its slot finds it condemned and replaces
        // it before giving it another (a worker killed an instant before can't be told from a
        // live one).
        for slot in lock(&self.slots).iter_mut() {
            let Activity::Running {
                client,
                generation,
                since,
            } = slot.activity
            else {
                continue;
            };
            // A stop-after too long to add to an instant is never due.
            let Some(due) = since.checked_add(state.stop_after) else {
                continue;
            };
            if !state.superseded(client, generation) {
                continue;
            }
            if due <= now {
                let killed = slot.killer.as_ref().is_some_and(WorkerKiller::kill);
                slot.activity = Activity::Condemned { killed };
            } else {
                next = Some(next.map_or(due, |n| n.min(due)));
            }
        }
        next
    }
}

/// The stopper: stops each superseded request when it's due, until the pool stops.
fn stop_when_due(shared: &Shared) {
    let mut state = lock(&shared.state);
    while !state.stopping {
        state = match shared.stop_superseded(&state) {
            Some(due) => {
                let wait = due.saturating_duration_since(Instant::now());
                shared
                    .due
                    .wait_timeout(state, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => shared
                .due
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner),
        };
    }
}

enum Next {
    Task(Box<dyn Task>),
    Idle,
    Stop,
}

/// A queued request, its result's type hidden so requests of every kind share the queue.
trait Task: Send {
    fn client(&self) -> ClientId;
    fn generation(&self) -> Generation;
    /// Runs the request on `worker`, in `slot`, ends it there, frees the slot (`busy`) and
    /// delivers its outcome, under one lock. Says whether the pool stopped it, condemning
    /// the worker.
    fn run(
        self: Box<Self>,
        worker: &mut Worker,
        shared: &Shared,
        slot: usize,
        busy: Busy<'_>,
    ) -> bool;
    /// Delivers `error` without running the request.
    fn fail(self: Box<Self>, error: Error);
    /// Delivers [`Outcome::Superseded`] without running the request.
    fn supersede(self: Box<Self>);
}

struct Submitted<T> {
    client: ClientId,
    generation: Generation,
    request: KernelRequest<T>,
    reply: SyncSender<Outcome<T>>,
    /// The span current where the request was submitted.
    span: tracing::Span,
}

impl<T: Send + 'static> Task for Submitted<T> {
    fn client(&self) -> ClientId {
        self.client
    }

    fn generation(&self) -> Generation {
        self.generation
    }

    fn run(
        self: Box<Self>,
        worker: &mut Worker,
        shared: &Shared,
        slot: usize,
        busy: Busy<'_>,
    ) -> bool {
        let Submitted {
            client,
            generation,
            request,
            reply,
            span,
        } = *self;
        let name = request.name();
        let result = span.in_scope(|| {
            tracing::debug_span!("kernel_pool_request", slot, client = client.number())
                .in_scope(|| request.run_on(worker, generation))
        });
        // Ended, decided, and sent under the lock `advance` takes, so once `advance` has
        // returned no result of an older generation is delivered, and an `advance` after the
        // result never finds the slot still running (and stops a worker with nothing left to
        // do). The channel holds one outcome and gets only this one, so the send never waits.
        let mut state = lock(&shared.state);
        let ended = shared.end(slot);
        // Free again as its outcome is delivered: a caller that submits on its result finds
        // it free, and doesn't start another slot.
        busy.release(&mut state);
        if state.stopping {
            // The pool stopped (its worker was killed under it): no result.
            return ended.is_condemned();
        }
        let (outcome, undelivered) = if state.superseded(client, generation) {
            (Outcome::Superseded, Some(result))
        } else {
            (Outcome::Done(result), None)
        };
        deliver(state, &reply, outcome);
        // Not delivered, so said here (after the lock: a log that blocks mustn't hold up the
        // pool). A failure that wasn't the pool's stopping of it (a kill that found the worker
        // alive) is a worker failing, worth seeing in the logs.
        if let Some(Err(e)) = undelivered {
            let _entered = span.enter();
            if matches!(ended, Activity::Condemned { killed: true }) {
                tracing::debug!(
                    op = name,
                    client = client.number(),
                    generation = generation.0,
                    code = e.code().as_str(),
                    "{name} stopped: superseded ({e})"
                );
            } else {
                tracing::warn!(
                    op = name,
                    client = client.number(),
                    generation = generation.0,
                    code = e.code().as_str(),
                    "{name} failed after it was superseded: {e}"
                );
            }
        }
        ended.is_condemned()
    }

    fn fail(self: Box<Self>, error: Error) {
        let _ = self.reply.send(Outcome::Done(Err(error)));
    }

    fn supersede(self: Box<Self>) {
        let _ = self.reply.send(Outcome::Superseded);
    }
}

/// Sends `outcome` while `state` (the pool's lock) is held, then lets the lock go. The
/// caller may have stopped waiting; nothing more to do then, but to free the outcome, after
/// the lock (a large result takes milliseconds to free).
pub(crate) fn deliver<G, T>(state: G, reply: &SyncSender<Outcome<T>>, outcome: Outcome<T>) {
    let unsent = reply.send(outcome).err();
    drop(state);
    drop(unsent);
}

/// A slot's hold on a request (see [`State::busy`]): let go before its outcome is
/// delivered, by dropping it, or by [`release`](Self::release) under the lock that
/// delivers it.
struct Busy<'a>(&'a Shared);

impl Busy<'_> {
    /// Lets go of the request in `state`, which the caller holds locked.
    fn release(self, state: &mut State) {
        state.busy -= 1;
        std::mem::forget(self);
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        lock(&self.0.state).busy -= 1;
    }
}

/// A slot's thread, ending: unless the pool is stopping, its slot goes back among those
/// not started, and starts again at once if requests are waiting with no slot free to take
/// them ([`grow`]). However the thread ended (a panic, which only a bug causes), a request
/// queued meanwhile is taken, not failed for want of a slot: its own caller's, say, woken
/// as the request the thread held ended with it, and asking again before the slot was back.
struct SlotEnded<'a> {
    shared: &'a Arc<Shared>,
    slot: usize,
}

impl Drop for SlotEnded<'_> {
    fn drop(&mut self) {
        tracing::debug!(slot = self.slot, "the slot's thread is ending");
        {
            let mut state = lock(&self.shared.state);
            if state.stopping {
                return;
            }
            state.started -= 1;
            state.unstarted.push(self.slot);
            // Its worker went with its keeper; its entry goes too, so the slot started again
            // isn't taken for still running the old request.
            self.shared.with_slot(self.slot, |s| *s = Slot::default());
        }
        grow(self.shared);
    }
}

/// When a pool's slots start their workers.
#[derive(Clone, Copy, Debug)]
enum Starts {
    /// As the pool starts, which fails if any can't.
    Now,
    /// Each with the first request its slot takes.
    OnDemand,
}

/// When one slot starts its worker: now, saying through the sender whether it could; or
/// with its first request.
enum SlotStart {
    Now(mpsc::Sender<Result<(), Error>>),
    OnDemand,
}

/// One slot of the pool: starts its worker (or, on demand, leaves it to its first
/// request), reports it ready (or not), then runs requests
/// from the queue until the pool stops, replacing its worker whenever it stops, dies, or
/// its request is superseded.
fn serve(slot: usize, shared: &Arc<Shared>, start: SlotStart) {
    // However the thread ends but by the pool's stopping (a panic, say, even in its span),
    // the slot is left to start again rather than counted free: once its keeper, declared
    // after, has let its worker go.
    let _ended = SlotEnded { shared, slot };
    let span = tracing::debug_span!("kernel_pool", slot);
    let _entered = span.enter();
    let mut keeper = Keeper {
        slot,
        shared,
        worker: None,
        number: 0,
        noted: false,
        killed_unstarted: false,
        retry: None,
    };
    match start {
        SlotStart::Now(ready) => {
            let started = keeper.live_worker().map(|_| ());
            let failed = started.is_err();
            let _ = ready.send(started);
            if failed {
                return;
            }
        }
        // Its first request starts it (`live_worker`, below); idle, a slot with no worker
        // and no failure to retry after starts none (`Keeper::tend`).
        SlotStart::OnDemand => {}
    }
    loop {
        let task = match shared.next() {
            Next::Stop => return,
            Next::Idle => {
                if shared.stopping() {
                    return;
                }
                keeper.tend();
                continue;
            }
            Next::Task(task) => task,
        };
        // The slot is free again however its request ends: before its outcome is delivered.
        let busy = Busy(shared);
        if shared.stopping() {
            return;
        }
        let worker = match keeper.live_worker() {
            // A worker started just now, while the pool was stopping, may have missed the
            // kill: don't give it the request. (Its killer was listed before this check, so a
            // stop that comes after it kills it.)
            Ok(_) if shared.stopping() => return,
            Ok(worker) => worker,
            Err(e) => {
                drop(busy);
                task.fail(e);
                continue;
            }
        };
        // Marked running before the generation is checked: an `advance` either came first
        // (and the request is superseded here, without running) or finds it marked (and the
        // pool may stop it, condemning the worker, even before it runs).
        let (client, generation) = (task.client(), task.generation());
        shared.with_slot(slot, |s| {
            s.activity = Activity::Running {
                client,
                generation,
                since: Instant::now(),
            };
        });
        if shared.superseded(client, generation) {
            let ended = shared.end(slot);
            drop(busy);
            task.supersede();
            if ended.is_condemned() {
                keeper.replace();
            }
            continue;
        }
        let answers = worker.answers();
        let condemned = task.run(worker, shared, slot, busy);
        let then = after_run(worker.answers() > answers, worker.is_stopped(), condemned);
        // In this order: the answer is the worker's, noted before it's replaced.
        if then.ends_wait {
            keeper.answered();
        }
        if then.replace {
            keeper.replace();
        }
    }
}

/// A slot's worker, kept running.
struct Keeper<'a> {
    slot: usize,
    shared: &'a Shared,
    worker: Option<Worker>,
    /// The worker's number (see [`Shared::spawns`]), and whether its start has been noted.
    number: u64,
    noted: bool,
    /// Whether the slot's last worker was killed (SIGKILL) before it started.
    killed_unstarted: bool,
    /// After workers ended on their own or before starting, or couldn't be started: when to
    /// start one again. Cleared when one of the slot's workers answers a request.
    retry: Option<Retry>,
}

#[derive(Clone, Copy, Debug)]
struct Retry {
    at: Instant,
    /// How long this wait is.
    wait: Duration,
    /// The number the next worker started would get, when the wait began: a worker with
    /// this number or higher was started after the failure.
    after: u64,
}

impl Retry {
    /// Whether to start a worker now: the wait is over, or a worker started after the
    /// failure has started, so the binary works.
    fn is_due(self, now: Instant, newest_started: u64) -> bool {
        now >= self.at || self.is_moot(newest_started)
    }

    /// Whether a worker started after the failure has started: the wait, and how long it
    /// had grown, no longer apply.
    fn is_moot(self, newest_started: u64) -> bool {
        newest_started >= self.after
    }
}

/// What a request's run means for its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AfterRun {
    /// The slot's wait ends: the binary works.
    ends_wait: bool,
    /// The slot's worker is replaced.
    replace: bool,
}

/// After a request ran on a slot's worker. `answered`, whether the worker read back an
/// answer (a result or a refusal); `stopped`, whether the worker was stopped (it crashed,
/// hung, or answered wrongly); `condemned`, whether its request was superseded and stopped
/// (its worker killed, or found dead). An answer shows the binary works, even one followed
/// by a kill; one the worker was stopped for doesn't.
fn after_run(answered: bool, stopped: bool, condemned: bool) -> AfterRun {
    AfterRun {
        ends_wait: answered && !stopped,
        replace: stopped || condemned,
    }
}

/// Whether an idle slot replaces a worker that ended with `status` at once, rather than
/// after a wait: `started`, whether it had said it started; `killed_before`, whether the
/// slot's last worker was killed (SIGKILL) before it started too.
fn replace_at_once(status: ExitStatus, started: bool, killed_before: bool) -> bool {
    match (status.code(), status.signal()) {
        // An exit of its own is its install's, started or not.
        (Some(_), _) => false,
        // Killed before it started: from outside, unless the last one was too.
        (None, Some(libc::SIGKILL)) if !started => !killed_before,
        // Killed or crashed once started: from outside. Any other end before it started is
        // its install's.
        _ => started,
    }
}

impl Keeper<'_> {
    /// The slot's worker, starting a new one (a process) if it has none, or if its worker
    /// stopped, died, closed its socket, or sent something unasked (which is then
    /// discarded).
    fn live_worker(&mut self) -> Result<&mut Worker, Error> {
        if self
            .worker
            .as_mut()
            .is_some_and(|w| w.is_stopped() || w.has_exited() || w.has_hung_up())
        {
            self.discard();
        }
        match self.worker {
            Some(ref mut worker) => Ok(worker),
            None => {
                // Started only from the pool's own build, and taken only if it is once it's
                // started (`Executable::spawn`).
                let worker = match self.shared.executable.spawn() {
                    Ok(worker) => worker,
                    Err(e) => {
                        self.wait_to_retry();
                        return Err(e);
                    }
                };
                self.number = self.shared.spawns.fetch_add(1, Ordering::Relaxed);
                self.noted = false;
                let killer = worker.killer();
                self.shared
                    .with_slot(self.slot, |s| s.killer = Some(killer));
                Ok(self.worker.insert(worker))
            }
        }
    }

    /// Stops the slot's worker and starts another now, so the pool stays full. If it can't
    /// start, the next request tries again, and reports why if it still can't.
    fn replace(&mut self) {
        self.discard();
        if !self.shared.stopping() {
            let _ = self.live_worker();
        }
    }

    /// While the slot is idle: keeps its worker running, so the pool stays full. A worker
    /// that ended, closed its socket, or sent something unasked is replaced:
    /// - now, if it runs on with its socket closed; or had started (said so: see
    ///   [`Worker::has_started`]) and was killed or crashed; or was killed (SIGKILL) before
    ///   it started, unless the slot's last worker was too (a kill from outside);
    /// - after a wait, if it exited on its own (an exit code), or ended before it started (a
    ///   worker binary that can't start: a missing library, a refusal at start, or a kill at
    ///   launch again, as macOS kills a binary whose code signature is broken), or couldn't be
    ///   started at all (its executable missing while it's rebuilt). The wait doubles each
    ///   time from [`IDLE_CHECK`] up to [`MAX_RETRY`] until one of the slot's workers answers
    ///   a request, and ends early once a worker started after the failure is running.
    ///
    /// A request starts a worker at once either way, and reports why it can't.
    fn tend(&mut self) {
        let slot = self.slot;
        if let Some(w) = self.worker.as_mut() {
            let replace_now = if w.is_stopped() {
                true
            } else {
                let status = match w.exit_status() {
                    Some(status) => Some(status),
                    // Its socket closed (or it sent something unasked): it may be ending.
                    None if w.has_hung_up() => w.exit_status_within(HUNG_UP_GRACE),
                    None => {
                        // Running, and of use.
                        self.note_started();
                        return;
                    }
                };
                match status {
                    // Running on, of no use.
                    None => true,
                    Some(status) => {
                        w.drain_replies(Instant::now() + HUNG_UP_GRACE);
                        let started = w.has_started();
                        let at_once = replace_at_once(status, started, self.killed_unstarted);
                        self.killed_unstarted = !started && status.signal() == Some(libc::SIGKILL);
                        at_once
                    }
                }
            };
            self.discard();
            if !replace_now {
                let wait = self.wait_to_retry();
                tracing::warn!(
                    slot,
                    wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                    "a kernel worker ended on its own, or before it started; starting another later"
                );
                return;
            }
            // The decision, as the wait's is: what the back-off's tests follow.
            tracing::debug!(
                slot,
                "a kernel worker ended, or was ended; starting another at once"
            );
        } else if let Some(retry) = self.retry {
            // Due, the wait is kept: if this start fails too, the next wait is longer.
            let now = Instant::now();
            if !retry.is_due(now, self.shared.newest_started.load(Ordering::Relaxed)) {
                return;
            }
            if now < retry.at {
                // The decision, as the wait's is: what the back-off's tests follow.
                tracing::debug!(
                    slot,
                    "a kernel worker started since this slot's wait began: starting another now"
                );
            }
        } else {
            return;
        }
        if let Err(e) = self.live_worker() {
            tracing::warn!(slot, error = %e, "can't start a kernel worker");
        }
    }

    /// The slot's worker answered a request: the binary works, so the slot's wait ends.
    fn answered(&mut self) {
        self.retry = None;
        self.note_started();
    }

    /// Notes the slot's worker having started and running, once: the binary works, so other
    /// slots' waits that began before it was started end. (A worker that starts and exits
    /// before an idle check proves nothing.)
    fn note_started(&mut self) {
        if !self.noted
            && self
                .worker
                .as_mut()
                .is_some_and(|w| w.has_started() && !w.has_exited())
        {
            self.noted = true;
            self.killed_unstarted = false;
            // What a kill of it now is taken as (from outside): the pool's tests follow it.
            tracing::debug!(slot = self.slot, "a kernel worker is running");
            self.shared
                .newest_started
                .fetch_max(self.number, Ordering::Relaxed);
        }
    }

    /// Puts off the next start, twice as long as the last time (or [`IDLE_CHECK`]), and
    /// says for how long.
    fn wait_to_retry(&mut self) -> Duration {
        let wait = self
            .retry
            .map_or(IDLE_CHECK, |retry| (retry.wait * 2).min(MAX_RETRY));
        self.retry = Some(Retry {
            at: Instant::now() + wait,
            wait,
            after: self.shared.spawns.load(Ordering::Relaxed),
        });
        wait
    }

    fn discard(&mut self) {
        self.shared.with_slot(self.slot, |s| s.killer = None);
        // Dropping the worker kills and reaps it.
        self.worker = None;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::{Arc, Mutex, mpsc};

    use super::{Outcome, deliver};

    #[test]
    fn a_pool_thread_that_cant_start_is_a_shortage_or_a_bug() {
        use std::io;

        use super::thread_error;
        use crate::ErrorCode;
        let short = thread_error(super::KERNEL, &io::Error::from_raw_os_error(libc::EAGAIN));
        assert_eq!(short.code(), ErrorCode::ResourceExhausted, "{short}");
        assert_eq!(short.message(), "couldn't start a kernel pool thread");
        // The OS's own words for it.
        let words = |errno| io::Error::from_raw_os_error(errno).to_string();
        assert_eq!(short.cause(), Some(words(libc::EAGAIN).as_str()), "{short}");
        assert!(
            short
                .suggestion()
                .is_some_and(|s| s.contains("processes or memory")),
            "{short}"
        );
        let bug = thread_error(super::KERNEL, &io::Error::from_raw_os_error(libc::EINVAL));
        assert_eq!(bug.code(), ErrorCode::Internal, "{bug}");
        assert_eq!(bug.message(), "couldn't start a kernel pool thread");
        // The fake names itself, not the pool.
        let fake = thread_error(
            crate::fake::KERNEL,
            &io::Error::from_raw_os_error(libc::EINVAL),
        );
        assert_eq!(fake.message(), "couldn't start a fake kernel thread");
        assert_eq!(bug.cause(), Some(words(libc::EINVAL).as_str()), "{bug}");
        assert_eq!(bug.suggestion(), Some(crate::worker::SILICO_BUG), "{bug}");
    }

    #[test]
    fn an_answer_ends_a_slots_wait_unless_the_worker_was_stopped_for_it() {
        use super::{AfterRun, after_run};
        // (answered, stopped, condemned) → (ends the wait, replaced)
        for (answered, stopped, condemned, then) in [
            (true, false, false, (true, false)),
            // Answered, then killed as superseded: the binary worked.
            (true, false, true, (true, true)),
            // Answered wrongly, and stopped for it.
            (true, true, false, (false, true)),
            (true, true, true, (false, true)),
            // No answer: refused before it was sent, crashed, hung, or superseded first.
            (false, false, false, (false, false)),
            (false, false, true, (false, true)),
            (false, true, false, (false, true)),
            (false, true, true, (false, true)),
        ] {
            let (ends_wait, replace) = then;
            assert_eq!(
                after_run(answered, stopped, condemned),
                AfterRun { ends_wait, replace },
                "answered {answered} stopped {stopped} condemned {condemned}"
            );
        }
    }

    #[test]
    fn an_idle_slot_replaces_a_worker_at_once_only_if_its_end_came_from_outside() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;

        use super::replace_at_once;
        let exited = ExitStatus::from_raw(3 << 8);
        let signalled = ExitStatus::from_raw;
        // (status, started, the last one killed before it started too) → at once
        for (status, started, killed_before, at_once) in [
            // An exit of its own: the install's, started or not.
            (exited, true, false, false),
            (exited, false, false, false),
            (exited, false, true, false),
            // Killed before it started: from outside the first time, not again.
            (signalled(libc::SIGKILL), false, false, true),
            (signalled(libc::SIGKILL), false, true, false),
            // Killed or crashed once started: from outside.
            (signalled(libc::SIGKILL), true, true, true),
            (signalled(libc::SIGTERM), true, false, true),
            (signalled(libc::SIGSEGV), true, false, true),
            // Any other end before it started: the install's (dyld aborting it, say).
            (signalled(libc::SIGABRT), false, false, false),
            (signalled(libc::SIGTERM), false, false, false),
        ] {
            assert_eq!(
                replace_at_once(status, started, killed_before),
                at_once,
                "{status:?} started {started} killed before {killed_before}"
            );
        }
    }

    #[test]
    fn a_retry_is_due_when_its_wait_ends_or_a_worker_started_since_has_started() {
        use std::time::{Duration, Instant};

        use super::Retry;
        let now = Instant::now();
        let retry = Retry {
            at: now + Duration::from_secs(8),
            wait: Duration::from_secs(8),
            after: 5,
        };
        // Workers 1 to 4 were started before the failure: their starting says nothing.
        assert!(!retry.is_due(now, 0));
        assert!(!retry.is_due(now, 4));
        assert!(retry.is_due(now, 5));
        assert!(retry.is_due(now, 9));
        assert!(!retry.is_due(now + Duration::from_secs(7), 4));
        assert!(retry.is_due(now + Duration::from_secs(8), 4));
    }

    #[test]
    fn an_outcome_is_sent_before_the_lock_is_let_go() {
        // A lock that, as it's let go, notes whether the outcome is already on its way: so
        // once `advance` (which takes the lock) has returned, no older result is delivered.
        struct Lock<'a>(&'a mpsc::Receiver<Outcome<u8>>, &'a Cell<bool>);
        impl Drop for Lock<'_> {
            fn drop(&mut self) {
                self.1.set(self.0.try_recv().is_ok());
            }
        }
        let sent_under_lock = Cell::new(false);
        let (reply, waiting) = mpsc::sync_channel(1);
        deliver(
            Lock(&waiting, &sent_under_lock),
            &reply,
            Outcome::Done(Ok(7)),
        );
        assert!(sent_under_lock.get(), "sent after the lock was let go");
    }

    #[test]
    fn an_outcome_no_one_waits_for_is_freed_after_the_lock() {
        // A result that, as it's freed, notes whether the lock is still held.
        struct Tattle(Arc<Mutex<()>>, Arc<Mutex<Option<bool>>>);
        impl Drop for Tattle {
            fn drop(&mut self) {
                *self.1.lock().unwrap() = Some(self.0.try_lock().is_err());
            }
        }
        let lock = Arc::new(Mutex::new(()));
        let freed_under_lock = Arc::new(Mutex::new(None));
        let (reply, waiting) = mpsc::sync_channel(1);
        // The caller stopped waiting.
        drop(waiting);
        let result = Tattle(Arc::clone(&lock), Arc::clone(&freed_under_lock));
        deliver(lock.lock().unwrap(), &reply, Outcome::Done(Ok(result)));
        assert_eq!(
            *freed_under_lock.lock().unwrap(),
            Some(false),
            "Some(true): freed under the lock; None: never freed"
        );
    }
}
