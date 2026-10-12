//! A pool of kernel workers (ARCHITECTURE.md, Process model): as many workers as the
//! machine has cores, each served by its own thread, taking requests from one queue.
//! Workers keep no state, so any worker can run any request, and a request's result doesn't
//! depend on which worker ran it or when.
//!
//! A worker that crashes, hangs, or sends a reply that can't be trusted is stopped, its
//! request fails with a structured error, and a new worker takes its place. The request is
//! never retried: the same input would likely crash the new worker too. A worker that has
//! died between requests (killed from outside, say) is replaced before it's given one, and
//! an idle thread checks its worker every [`IDLE_CHECK`], so the pool returns to full size
//! on its own. (A worker still in the moment of exiting can't be told from a live one: a
//! request given to it then fails as a crash, as any request whose worker is killed under
//! it does.)
//!
//! Every request carries the document generation it's for. When a newer generation starts
//! ([`KernelPool::advance`]), requests of older ones still waiting are dropped without
//! running, and those running are stopped (their workers killed and replaced) once they've
//! run the pool's stop-after ([`STOP_AFTER`] unless set): at once if they already have, and
//! otherwise when they do, unless they finish first. Their callers get
//! [`Outcome::Superseded`], never a result.

use std::collections::VecDeque;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use silico_protocol::Generation;

use crate::kernel::KernelRequest;
use crate::worker::{Worker, WorkerKiller, lock};
use crate::{Error, ErrorCode};

/// How often an idle thread checks that its worker is still alive.
pub const IDLE_CHECK: Duration = Duration::from_secs(1);

/// The longest an idle thread waits before trying again to start a worker that couldn't
/// be started (the wait doubles from [`IDLE_CHECK`] each time).
const MAX_RETRY: Duration = Duration::from_secs(60);

/// How long a superseded request may run, counted from its start, before the pool stops it,
/// unless [`KernelPool::set_stop_after`] says otherwise. Replacing a stopped worker takes
/// about 30 ms of wall time and CPU (noticing it has ended, and starting another), so
/// stopping a request costs more than letting it finish when less than that is left, as it
/// is for most operations on simple parts (a box or a mesh under a millisecond, a fillet of
/// twelve edges about 9 ms). One that has run this long is likely to run longer. So a
/// superseded request holds its worker at most this long from its start, and the worker is
/// then replaced.
pub const STOP_AFTER: Duration = Duration::from_millis(50);

/// The most workers a pool may have: far more than any machine's cores, and few enough
/// that a size from a setting can't make the pool try to start without end.
pub const MAX_WORKERS: usize = 1024;

/// Kernel workers, and a queue of requests for them. Dropping the pool stops its workers at
/// once (requests still waiting or running end without a result) and joins its threads.
pub struct KernelPool {
    shared: Arc<Shared>,
    /// One per slot.
    threads: Vec<JoinHandle<()>>,
    /// Stops superseded requests when they're due.
    stopper: Option<JoinHandle<()>>,
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

/// A request submitted to a [`KernelPool`], whose outcome comes when a worker has run it.
#[must_use = "a request's outcome is only had by waiting for it"]
pub struct Pending<T> {
    name: &'static str,
    outcome: Receiver<Outcome<T>>,
}

impl<T> Pending<T> {
    /// Waits for the outcome.
    pub fn wait(self) -> Outcome<T> {
        match self.outcome.recv() {
            Ok(outcome) => outcome,
            Err(_) => Outcome::Done(Err(stopped(self.name))),
        }
    }

    /// Waits up to `timeout` for the outcome, or gives the request back to wait on again.
    pub fn wait_timeout(self, timeout: Duration) -> Result<Outcome<T>, Self> {
        match self.outcome.recv_timeout(timeout) {
            Ok(outcome) => Ok(outcome),
            Err(RecvTimeoutError::Timeout) => Err(self),
            Err(RecvTimeoutError::Disconnected) => Ok(Outcome::Done(Err(stopped(self.name)))),
        }
    }

    /// The outcome, if it has come, or the request back to wait on.
    pub fn try_take(self) -> Result<Outcome<T>, Self> {
        match self.outcome.try_recv() {
            Ok(outcome) => Ok(outcome),
            Err(TryRecvError::Empty) => Err(self),
            Err(TryRecvError::Disconnected) => Ok(Outcome::Done(Err(stopped(self.name)))),
        }
    }
}

/// The outcome of a request whose pool stopped before running it.
fn stopped(name: &str) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("the kernel pool stopped before {name} was done"),
    )
    .with_suggestion("submit the request to a running pool")
}

impl KernelPool {
    /// Starts as many kernel workers as the machine has cores (at most [`MAX_WORKERS`]),
    /// each the one [`KERNEL_WORKER_ENV`](crate::kernel::KERNEL_WORKER_ENV) names or
    /// `silico-kernel-worker` next to the running executable, as
    /// [`KernelWorker::start`](crate::kernel::KernelWorker::start) finds it.
    pub fn start() -> Result<Self, Error> {
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        Self::start_workers(&crate::kernel::worker_path()?, cores.min(MAX_WORKERS))
    }

    /// Starts `size` workers (1 to [`MAX_WORKERS`]) of the executable at `exe`, which is
    /// found once, now, so replacements come from the same place whatever the working
    /// directory later. Fails if any can't be started, and then starts none.
    pub fn start_workers(exe: &Path, size: usize) -> Result<Self, Error> {
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
        let exe = exe.as_path();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                generation: Generation(0),
                stopping: false,
                stop_after: STOP_AFTER,
            }),
            work: Condvar::new(),
            due: Condvar::new(),
            answered: AtomicU64::new(0),
            slots: Mutex::new((0..size).map(|_| Slot::default()).collect()),
        });
        let mut pool = Self {
            shared,
            threads: Vec::with_capacity(size),
            stopper: None,
        };
        let shared = Arc::clone(&pool.shared);
        pool.stopper = Some(spawn_thread("kernel-pool-stopper".to_owned(), move || {
            stop_when_due(&shared);
        })?);
        // Each thread starts its worker (one at a time: see `Worker::spawn`); the pool is
        // ready when all have.
        let (ready_tx, ready) = mpsc::channel();
        for slot in 0..size {
            let shared = Arc::clone(&pool.shared);
            let exe = exe.to_owned();
            let ready = ready_tx.clone();
            let thread = spawn_thread(format!("kernel-pool-{slot}"), move || {
                serve(slot, &exe, &shared, &ready);
            })?;
            pool.threads.push(thread);
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

    /// How many workers the pool keeps.
    pub fn size(&self) -> usize {
        self.threads.len()
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

    /// The generation requests are for: the newest [`advance`](Self::advance) started.
    pub fn generation(&self) -> Generation {
        lock(&self.shared.state).generation
    }

    /// How long a superseded request may run, from its start, before the pool stops it
    /// ([`STOP_AFTER`] unless set here), for requests running now as well as later ones.
    /// Zero stops every superseded request at once. For tests, and for tuning.
    pub fn set_stop_after(&self, after: Duration) {
        lock(&self.shared.state).stop_after = after;
        self.shared.due.notify_all();
    }

    /// Starts `generation`, superseding every older one: their requests still waiting are
    /// dropped without running, and those running are stopped (their workers killed and
    /// replaced) once they've run the pool's stop-after, now if they already have; their
    /// results are discarded. A generation no newer than the current one changes nothing.
    pub fn advance(&self, generation: Generation) {
        let superseded: Vec<Box<dyn Task>> = {
            let mut state = lock(&self.shared.state);
            if generation <= state.generation {
                return;
            }
            state.generation = generation;
            let (stale, current): (VecDeque<_>, VecDeque<_>) = state
                .queue
                .drain(..)
                .partition(|task| task.generation() < generation);
            state.queue = current;
            self.shared.stop_superseded(&state);
            stale.into_iter().collect()
        };
        // The rest are the stopper's, when they're due.
        self.shared.due.notify_all();
        for task in superseded {
            task.supersede();
        }
    }

    /// Queues `request` for `generation`, to run on the next free worker. A request for a
    /// generation older than the current one is superseded at once. The request runs in the
    /// tracing span current here, so its worker's call and logs trace back to whatever
    /// submitted it (ARCHITECTURE.md, invariant 9).
    pub fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        let name = request.name();
        let (reply, outcome) = mpsc::sync_channel(1);
        let task = Box::new(Submitted {
            generation,
            request,
            reply,
            span: tracing::Span::current(),
        });
        {
            let mut state = lock(&self.shared.state);
            if generation >= state.generation && !state.stopping {
                state.queue.push_back(task);
                drop(state);
                self.shared.work.notify_one();
                return Pending { name, outcome };
            }
            if state.stopping {
                // Dropping the task tells the caller the pool stopped.
                return Pending { name, outcome };
            }
        }
        task.supersede();
        Pending { name, outcome }
    }
}

impl Drop for KernelPool {
    fn drop(&mut self) {
        let waiting: Vec<Box<dyn Task>> = {
            let mut state = lock(&self.shared.state);
            state.stopping = true;
            state.queue.drain(..).collect()
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
        for thread in self.threads.drain(..).chain(self.stopper.take()) {
            let _ = thread.join();
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
        .map_err(|e| {
            Error::new(ErrorCode::Internal, "couldn't start a kernel pool thread")
                .with_cause(e.to_string())
        })
}

/// What the pool's threads share. Its locks are taken in the order `state`, then `slots`
/// (then a worker's process lock), never the other way.
struct Shared {
    state: Mutex<State>,
    /// Signalled when a request is queued or the pool stops.
    work: Condvar,
    /// Signalled when the generation or the stop-after changes, or the pool stops: for the
    /// stopper.
    due: Condvar,
    /// What each slot holds and runs, to kill its worker when the pool stops or its request
    /// is superseded, and to report its process ID.
    slots: Mutex<Vec<Slot>>,
    /// How many requests workers have answered: once one has, slots waiting to start a
    /// worker again (its binary missing, say) know it starts now.
    answered: AtomicU64,
}

struct State {
    queue: VecDeque<Box<dyn Task>>,
    generation: Generation,
    stopping: bool,
    stop_after: Duration,
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
    /// Running a request of `generation`, since `since`.
    Running {
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

    fn generation(&self) -> Generation {
        lock(&self.state).generation
    }

    /// The next request for a thread, waiting up to [`IDLE_CHECK`] for one.
    fn next(&self) -> Next {
        let mut state = lock(&self.state);
        loop {
            if state.stopping {
                return Next::Stop;
            }
            if let Some(task) = state.queue.pop_front() {
                return Next::Task(task);
            }
            let (next, timeout) = self
                .work
                .wait_timeout(state, IDLE_CHECK)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
            if timeout.timed_out() && !state.stopping && state.queue.is_empty() {
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

    /// Stops the superseded requests (those of generations older than `state`'s) that have
    /// run the stop-after, and says when the next of those still running will have.
    fn stop_superseded(&self, state: &State) -> Option<Instant> {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        // A slot marks its request running, and ends it, under this lock, so a worker killed
        // here is one still on a superseded request; its slot finds it condemned and replaces
        // it before giving it another (a worker killed an instant before can't be told from a
        // live one).
        for slot in lock(&self.slots).iter_mut() {
            let Activity::Running { generation, since } = slot.activity else {
                continue;
            };
            // A stop-after too long to add to an instant is never due.
            let Some(due) = since.checked_add(state.stop_after) else {
                continue;
            };
            if generation >= state.generation {
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
    fn generation(&self) -> Generation;
    /// Runs the request on `worker`, in `slot`, ends it there, and delivers its outcome. Says
    /// whether the pool stopped it, condemning the worker.
    fn run(self: Box<Self>, worker: &mut Worker, shared: &Shared, slot: usize) -> bool;
    /// Delivers `error` without running the request.
    fn fail(self: Box<Self>, error: Error);
    /// Delivers [`Outcome::Superseded`] without running the request.
    fn supersede(self: Box<Self>);
}

struct Submitted<T> {
    generation: Generation,
    request: KernelRequest<T>,
    reply: SyncSender<Outcome<T>>,
    /// The span current where the request was submitted.
    span: tracing::Span,
}

impl<T: Send + 'static> Task for Submitted<T> {
    fn generation(&self) -> Generation {
        self.generation
    }

    fn run(self: Box<Self>, worker: &mut Worker, shared: &Shared, slot: usize) -> bool {
        let Submitted {
            generation,
            request,
            reply,
            span,
        } = *self;
        let name = request.name();
        let result = span.in_scope(|| {
            tracing::debug_span!("kernel_pool_request", slot)
                .in_scope(|| request.run_on(worker, generation))
        });
        // Ended, decided, and sent under the lock `advance` takes, so once `advance` has
        // returned no result of an older generation is delivered, and an `advance` after the
        // result never finds the slot still running (and stops a worker with nothing left to
        // do). The channel holds one outcome and gets only this one, so the send never waits.
        let state = lock(&shared.state);
        let ended = shared.end(slot);
        if state.stopping {
            // The pool stopped (its worker was killed under it): no result.
            return ended.is_condemned();
        }
        let (outcome, undelivered) = if state.generation > generation {
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
                    generation = generation.0,
                    code = e.code().as_str(),
                    "{name} stopped: superseded ({e})"
                );
            } else {
                tracing::warn!(
                    op = name,
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
fn deliver<G, T>(state: G, reply: &SyncSender<Outcome<T>>, outcome: Outcome<T>) {
    let unsent = reply.send(outcome).err();
    drop(state);
    drop(unsent);
}

/// One slot of the pool: starts its worker, reports it ready (or not), then runs requests
/// from the queue until the pool stops, replacing its worker whenever it stops, dies, or
/// its request is superseded.
fn serve(slot: usize, exe: &Path, shared: &Shared, ready: &mpsc::Sender<Result<(), Error>>) {
    let span = tracing::debug_span!("kernel_pool", slot);
    let _entered = span.enter();
    let mut keeper = Keeper {
        slot,
        exe: exe.to_owned(),
        shared,
        worker: None,
        served: false,
        retry: None,
    };
    let started = keeper.live_worker().map(|_| ());
    let failed = started.is_err();
    let _ = ready.send(started);
    if failed {
        return;
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
                task.fail(e);
                continue;
            }
        };
        // Marked running before the generation is checked: an `advance` either came first
        // (and the request is superseded here, without running) or finds it marked (and the
        // pool may stop it, condemning the worker, even before it runs).
        let generation = task.generation();
        shared.with_slot(slot, |s| {
            s.activity = Activity::Running {
                generation,
                since: Instant::now(),
            };
        });
        if shared.generation() > generation {
            let ended = shared.end(slot);
            task.supersede();
            if ended.is_condemned() {
                keeper.replace();
            }
            continue;
        }
        let condemned = task.run(worker, shared, slot);
        if condemned || worker.is_stopped() {
            keeper.replace();
        } else {
            keeper.served = true;
            keeper.retry = None;
            shared.answered.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A slot's worker, kept running.
struct Keeper<'a> {
    slot: usize,
    exe: PathBuf,
    shared: &'a Shared,
    worker: Option<Worker>,
    /// Whether the worker has answered a request.
    served: bool,
    /// After workers ended before answering, or couldn't be started: when to start one
    /// again, how long that wait was, and how many requests the pool's workers had
    /// answered then. Cleared when one answers.
    retry: Option<Retry>,
}

#[derive(Clone, Copy)]
struct Retry {
    at: Instant,
    wait: Duration,
    answered: u64,
}

impl Keeper<'_> {
    /// The slot's worker, starting a new one (a process) if it has none, or if its worker
    /// stopped, died, or sent something unasked (which is then discarded).
    fn live_worker(&mut self) -> Result<&mut Worker, Error> {
        if self
            .worker
            .as_mut()
            .is_some_and(|w| w.is_stopped() || w.has_exited() || w.has_stray_reply())
        {
            self.discard();
        }
        match self.worker {
            Some(ref mut worker) => Ok(worker),
            None => {
                let worker = match Worker::spawn(&self.exe) {
                    Ok(worker) => worker,
                    Err(e) => {
                        self.wait_to_retry();
                        return Err(e);
                    }
                };
                self.served = false;
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

    /// While the slot is idle: a worker that died is replaced, so the pool stays full. One
    /// killed from outside, or that had answered requests, is replaced now. One that
    /// ended on its own before answering any (a worker binary that can't start: a missing
    /// library, a refusal at start) is started again after a wait that doubles each time
    /// from [`IDLE_CHECK`] up to [`MAX_RETRY`], as is one that couldn't be started at all
    /// (its executable missing while it's rebuilt), rather than every [`IDLE_CHECK`] for as
    /// long as the pool is idle; or at the next check, once another slot's worker has
    /// answered a request. A request starts one at once either way, and reports why it
    /// can't.
    fn tend(&mut self) {
        let slot = self.slot;
        if let Some(w) = self.worker.as_mut() {
            let replace_now = if w.is_stopped() || w.has_stray_reply() {
                true
            } else {
                match w.exit_status() {
                    None => return,
                    Some(status) => self.served || status.signal() == Some(libc::SIGKILL),
                }
            };
            self.discard();
            if !replace_now {
                self.wait_to_retry();
                tracing::warn!(
                    slot,
                    "a kernel worker ended before answering a request; starting another later"
                );
                return;
            }
        } else if let Some(retry) = self.retry {
            // Another slot's worker answering since shows the binary works now.
            if self.shared.answered.load(Ordering::Relaxed) != retry.answered {
                self.retry = None;
            } else if Instant::now() < retry.at {
                return;
            }
        } else {
            return;
        }
        if let Err(e) = self.live_worker() {
            tracing::warn!(slot, error = %e, "can't start a kernel worker");
        }
    }

    /// Puts off the next start, twice as long as the last time (or [`IDLE_CHECK`]).
    fn wait_to_retry(&mut self) {
        let wait = self
            .retry
            .map_or(IDLE_CHECK, |retry| (retry.wait * 2).min(MAX_RETRY));
        self.retry = Some(Retry {
            at: Instant::now() + wait,
            wait,
            answered: self.shared.answered.load(Ordering::Relaxed),
        });
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
