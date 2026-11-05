//! An in-memory kernel for tests: the [`Kernel`] that core tests run on, so they need no
//! OCCT and can see what was asked of the kernel. It runs requests on threads as the pool
//! runs them on workers, keeps generations for each of its clients, and keeps a record of
//! every call. Its own client behaves as the pool's own client ([`ClientId::KERNEL`])
//! does.
//!
//! Its shapes are symbolic, and their bytes say all there is to them, as a B-rep's do: any
//! fake kernel reads a shape any other made. A shape is the steps that made it, one to a
//! line, each operation naming its inputs by their lines (a primitive's values, a move's
//! offset, a boolean's two inputs), written in one canonical way, so the same request makes
//! the same bytes on any kernel, on any thread, in any run, and a shape has one spelling.
//! [`FakeKernel::describe`] writes a shape out as an expression: `cut(box(40, 20, 10),
//! translate(cylinder(2.5, 12), 20, 10, -1))`.
//!
//! As the real kernel: a primitive has the faces, edges, and vertices OCCT gives it, and a
//! zero of either sign is the same value; a move reports its elements as the kernel does
//! (each modified into its own number, or kept by a move of nothing), and composes as the
//! kernel's locations do (a moved shape moved again is the original moved by the sum, keeping
//! the sign of a zero; a move by nothing, or back to where it was, is the shape itself). A
//! boolean is the fake's own: it has both inputs' elements, as if they didn't touch, and
//! keeps them all. A summary is exact for a primitive (its volume, area, center, and box by
//! formula, where it sits as the kernel makes it) and a move; a boolean's, its own model's
//! again, is its inputs' together, as if they didn't touch: their volumes and areas added,
//! their centers weighed by volume, and the box around both. A boolean is one solid (a
//! fuse's) or its base's (a cut's and a common's), as if they met, and a cut whose tool is a
//! sweep of faces (a sketch feature's) is its base less the share of its tool inside the
//! base's box (`Measured::less`), so a pad adds and a pocket removes. Faces made from a profile's
//! regions are numbered and kept as the kernel keeps them (each curve its edge, each point
//! its vertex), with their area by formula and the box around their curves. A sweep of
//! faces (a prism, through all or not, or a revolution) reports each element as the kernel
//! does (its start `modified`, its end `generated` of its own kind, its sides `generated`),
//! with its own numbering, as if no element were on a revolution's axis, and, for a whole
//! turn, as if no edge were square to it (each edge's start its face's seam, each vertex's
//! itself; the face reported nothing); its measures are its own model's too (below). It
//! answers making primitives and faces, moving shapes, booleans, sweeps of faces,
//! summaries, descriptions (symbolic: each element stands for its number, `described`), and
//! echoes; anything else is refused as an internal error.
//!
//! The real kernel writes some primitives (a cylinder, a torus) differently when it makes
//! them than once it has read them back, so their bytes change once, on the first operation
//! they pass through, even a move by nothing; the fake's never do.
//!
//! It doesn't refuse what OCCT can't build from values the protocol's check accepts: a torus
//! whose tube is wider than its ring (by no more than OCCT's tolerance, which the check
//! allows) is one OCCT may refuse, by the floating point of its revolve, and the fake builds
//! it. A test of such a refusal gives it with [`FakeKernel::refuse`].
//!
//! Tests can hold requests until they're released ([`FakeKernel::hold`]), to see what runs
//! at once or to supersede a request while it runs, have requests refused, and have them
//! answered as a worker at fault would ([`FakeKernel::answer_instead`]).
//!
//! It has clients ([`Clients`]): the kernel's own ([`ClientId::KERNEL`]), and each
//! [`FakeClient`] it gives, each with its own generations, its waiting requests served in
//! turn with the other clients'.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use silico_protocol::vec3::{dot, sub};
use silico_protocol::{
    Algorithm, DESCRIBE_BATCH, Description, DragBegin, EdgeCurve, EdgeDescription, FaceDescription,
    FaceFrame, Moved, PointPlace, Reply, ShapeSummary, SketchSystem, SolveStatus, Solved, Surface,
    Tag, area_and_perimeter, loop_area,
};

use crate::kernel::{
    Axis, Blob, BooleanOp, BuiltData, ClientId, Clients, CopiesOp, Curve, ElementCounts,
    ElementFate, ElementKind, ElementRef, Ends, FaceRegions, Generation, History, InputHistory,
    Kernel, KernelRequest, Op, Placement, Primitive, Region, Reported, ShapeData, SketchPlane,
    SweepKind, Topology, cone_caps, primitive_counts,
};
use crate::pool::{ClientQueues, ClientTask, MAX_WORKERS, Outcome, Pending, deliver};
use crate::sketch::{DragStarted, SketchSolver, SolveStopper};
use crate::worker::lock;
use crate::worker::thread_error;
use crate::{Error, ErrorCode};

/// What the fake calls itself in its errors: a [`Pending`] stopped early, a thread that
/// couldn't start.
pub(crate) const KERNEL: &str = "fake kernel";

/// What the fake is, for the cache's keys: the same for the kernel and each of its clients,
/// so they share the results kept on disk.
const IDENTITY: &str = "silico fake kernel 1";

/// A request the fake kernel started: the client that submitted it, the generation of that
/// client's it was for, and its operation.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub client: ClientId,
    pub generation: Generation,
    pub op: Op,
}

/// The in-memory kernel. Dropping it ends requests still waiting or held without a
/// result, and joins its threads.
pub struct FakeKernel {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

/// A client of a [`FakeKernel`] ([`Clients::client`]): a handle with generations of its own,
/// whose advances supersede only the requests it submitted. Dropping it retires the client:
/// its requests waiting or held come to [`Outcome::Superseded`], as do those running when
/// they end. It may outlive its kernel: its requests then end without a result, as the
/// kernel's do once it's dropped.
pub struct FakeClient {
    shared: Arc<Shared>,
    id: ClientId,
}

/// Holds requests a predicate picks, from when they start until it's released or dropped,
/// or they're superseded, by a newer generation of their client's or its retirement (which
/// stops them, as the pool stops a superseded request).
#[must_use = "dropping a hold releases it"]
pub struct Hold {
    shared: Arc<Shared>,
    id: usize,
}

type Picks = dyn Fn(&Op) -> bool + Send + Sync;
type Refuses = dyn Fn(&Op) -> Option<Error> + Send + Sync;
type Answers = dyn Fn(&Op) -> Option<Reply> + Send + Sync;

struct Shared {
    state: Mutex<State>,
    /// A queue changed, or the kernel is stopping.
    work: Condvar,
    /// A request started or ended, or a hold was released.
    changed: Condvar,
}

struct State {
    /// The kernel's clients, and their requests waiting, kept as the pool keeps its own.
    clients: ClientQueues<dyn Task>,
    stopping: bool,
    calls: Vec<Call>,
    /// Every request submitted, in order, whether or not it started.
    asked: Vec<Call>,
    running: usize,
    most_running: usize,
    holds: Vec<HoldRule>,
    refusals: Vec<Arc<Refuses>>,
    answers: Vec<Arc<Answers>>,
}

struct HoldRule {
    picks: Arc<Picks>,
    released: bool,
}

impl FakeKernel {
    /// A fake kernel running up to `size` requests at once: at least one, and at most the
    /// pool's [`MAX_WORKERS`]. Fails as the pool does if the system can't start its threads.
    pub fn new(size: usize) -> Result<Self, Error> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                clients: ClientQueues::new(),
                stopping: false,
                calls: Vec::new(),
                asked: Vec::new(),
                running: 0,
                most_running: 0,
                holds: Vec::new(),
                refusals: Vec::new(),
                answers: Vec::new(),
            }),
            work: Condvar::new(),
            changed: Condvar::new(),
        });
        let mut kernel = Self {
            shared,
            threads: Vec::new(),
        };
        for n in 0..size.clamp(1, MAX_WORKERS) {
            let shared = Arc::clone(&kernel.shared);
            let thread = std::thread::Builder::new()
                .name(format!("fake-kernel-{n}"))
                .spawn(move || serve(&shared))
                // Dropping the kernel stops and joins the threads already started.
                .map_err(|e| thread_error(KERNEL, &e))?;
            kernel.threads.push(thread);
        }
        Ok(kernel)
    }

    /// Every request started so far, in the order they started.
    pub fn calls(&self) -> Vec<Call> {
        lock(&self.shared.state).calls.clone()
    }

    /// Every request submitted so far, in the order they were, whether or not it started. A
    /// request its caller dropped (a render that stops at the first body holding its eye) is
    /// asked at once, but only starts, and is only in [`calls`](Self::calls), once a thread
    /// takes it, which may be later: dropping it takes nothing from the queue.
    pub fn asked(&self) -> Vec<Call> {
        lock(&self.shared.state).asked.clone()
    }

    /// How many requests waiting their callers gave up (dropped their [`Pending`]), each to be
    /// dropped unrun when its turn comes: what a test waits on to know a caller cancelled
    /// them before a thread took them.
    pub fn cancelled_waiting(&self) -> usize {
        lock(&self.shared.state).clients.cancelled_waiting()
    }

    /// The most requests that have run at once.
    pub fn most_running(&self) -> usize {
        lock(&self.shared.state).most_running
    }

    /// Waits up to `timeout` until at least `count` requests are running at once: whether
    /// they were. A request has stopped running once its outcome is sent.
    pub fn wait_running(&self, count: usize, timeout: Duration) -> bool {
        self.wait_until(|running| running >= count, timeout)
    }

    /// Waits up to `timeout` until no request is running: whether none was. A held request
    /// stops running once it's superseded, its hold still in place.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        self.wait_until(|running| running == 0, timeout)
    }

    /// Waits up to `timeout` until `done` holds of how many requests are running: whether
    /// it did.
    fn wait_until(&self, done: impl Fn(usize) -> bool, timeout: Duration) -> bool {
        // No deadline: a timeout too long to add to now is forever.
        let deadline = Instant::now().checked_add(timeout);
        let mut state = lock(&self.shared.state);
        while !done(state.running) {
            state = match deadline {
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return false;
                    }
                    self.shared
                        .changed
                        .wait_timeout(state, left)
                        .map_or_else(|e| e.into_inner().0, |(state, _)| state)
                }
                None => self
                    .shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
            };
        }
        true
    }

    /// Holds every request `picks` (called once for each request as it starts, with the
    /// kernel's state locked, so it mustn't call the kernel) until the hold is released.
    pub fn hold(&self, picks: impl Fn(&Op) -> bool + Send + Sync + 'static) -> Hold {
        let mut state = lock(&self.shared.state);
        state.holds.push(HoldRule {
            picks: Arc::new(picks),
            released: false,
        });
        Hold {
            shared: Arc::clone(&self.shared),
            id: state.holds.len() - 1,
        }
    }

    /// Refuses every request `refuses` returns an error for (called with the kernel's state
    /// locked, so it mustn't call the kernel), with that error, after it starts.
    pub fn refuse(&self, refuses: impl Fn(&Op) -> Option<Error> + Send + Sync + 'static) {
        lock(&self.shared.state).refusals.push(Arc::new(refuses));
    }

    /// Answers every request `answers` returns a reply for (called with the kernel's state
    /// locked, so it mustn't call the kernel) with that reply instead of the fake's own,
    /// after it starts, unless a rule of [`FakeKernel::refuse`]'s refuses it: read as a
    /// worker's reply is, so a test can send what a worker at fault would (a description
    /// whose total isn't its summary's, say). The first rule given that answers is taken.
    pub fn answer_instead(&self, answers: impl Fn(&Op) -> Option<Reply> + Send + Sync + 'static) {
        lock(&self.shared.state).answers.push(Arc::new(answers));
    }

    /// Refuses each request from now on that `refuses` gives a failure for, as a kernel
    /// worker would: the failure, as data, worded as the worker client words a worker's.
    pub fn refuse_as_worker(
        &self,
        refuses: impl Fn(&Op) -> Option<silico_protocol::Failure> + Send + Sync + 'static,
    ) {
        self.refuse(move |op| {
            refuses(op).map(|failure| crate::worker::failure_error(op.name(), failure))
        });
    }

    /// How a fake kernel made `shape`, written out (`cut(box(40, 20, 10), sphere(5))`), if
    /// it's a fake kernel's shape.
    pub fn describe(shape: &ShapeData) -> Option<String> {
        Symbolic::read(shape).ok().map(|made| made.describe())
    }

    /// The faces, edges, vertices, and solids of `shape`, if it's a fake kernel's shape.
    pub fn counts(shape: &ShapeData) -> Option<ElementCounts> {
        Symbolic::read(shape).ok().map(|made| made.counts())
    }

    /// Every face, edge, and vertex of `shape` as the fake describes them, if it's a fake
    /// kernel's shape: what its [`Op::DescribeAll`] answers, for a test that answers part of
    /// a description itself and leaves the rest the fake's.
    pub fn described(shape: &ShapeData) -> Option<Description> {
        let counts = Symbolic::read(shape).ok()?.counts();
        match described_all_any_size(&counts) {
            Ok(Reply::DescribedAll(description)) => Some(description),
            _ => None,
        }
    }
}

impl Kernel for FakeKernel {
    fn identity(&self) -> &str {
        IDENTITY
    }

    fn generation(&self) -> Generation {
        self.shared.generation(ClientId::KERNEL)
    }

    fn advance(&self, generation: Generation) {
        self.shared.advance(ClientId::KERNEL, generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        self.shared.submit(ClientId::KERNEL, generation, request)
    }
}

impl Clients for FakeKernel {
    type Client = FakeClient;

    fn client(&self) -> FakeClient {
        let id = lock(&self.shared.state).clients.add_client();
        tracing::debug!(client = id.number(), "fake kernel client started");
        FakeClient {
            shared: Arc::clone(&self.shared),
            id,
        }
    }
}

impl Drop for FakeKernel {
    fn drop(&mut self) {
        let waiting: Vec<Box<dyn Task>> = {
            let mut state = lock(&self.shared.state);
            state.stopping = true;
            state.clients.take_all()
        };
        drop(waiting);
        self.shared.work.notify_all();
        self.shared.changed.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl FakeClient {
    /// The client it is: what its requests' [`Call`]s say.
    pub fn id(&self) -> ClientId {
        self.id
    }
}

impl Kernel for FakeClient {
    fn identity(&self) -> &str {
        IDENTITY
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

impl Drop for FakeClient {
    fn drop(&mut self) {
        let _span = tracing::debug_span!("retire_client", client = self.id.number()).entered();
        let mut state = lock(&self.shared.state);
        let stale = state.clients.retire(self.id);
        self.shared.supersede(state, stale);
    }
}

impl Shared {
    /// `client`'s generation: the newest it started.
    fn generation(&self, client: ClientId) -> Generation {
        lock(&self.state).clients.generation(client)
    }

    /// Starts `generation` for `client`, superseding its older requests waiting and held.
    fn advance(&self, client: ClientId, generation: Generation) {
        let mut state = lock(&self.state);
        if let Some(stale) = state.clients.advance(client, generation) {
            self.supersede(state, stale);
        }
    }

    /// Ends the requests a client's generation, or its retirement, just changed in `state`
    /// has superseded: `stale`, taken from its queue, are told outside the lock, and its held
    /// ones are woken to stop themselves.
    fn supersede(&self, state: MutexGuard<'_, State>, stale: VecDeque<Box<dyn Task>>) {
        drop(state);
        self.changed.notify_all();
        for task in stale {
            task.supersede();
        }
    }

    /// Queues `request`, `client`'s, for `generation`.
    fn submit<T: Send + 'static>(
        &self,
        client: ClientId,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        let (reply, outcome) = mpsc::sync_channel(1);
        let pending = Pending::new(request.name(), KERNEL, client, outcome);
        let task = Box::new(Submitted {
            client,
            generation,
            cancelled: pending.cancelled(),
            request,
            reply,
            span: tracing::Span::current(),
        });
        let mut state = lock(&self.state);
        state.asked.push(Call {
            client,
            generation,
            op: task.op().clone(),
        });
        if state.stopping {
            // Dropping the task tells the caller the kernel stopped.
            return pending;
        }
        if state.clients.superseded(client, generation) {
            drop(state);
            task.supersede();
            return pending;
        }
        state.clients.push(task);
        drop(state);
        self.work.notify_one();
        pending
    }
}

impl Hold {
    /// Lets the requests it holds, and any it would have held, run.
    pub fn release(&self) {
        let mut state = lock(&self.shared.state);
        if let Some(rule) = state.holds.get_mut(self.id) {
            rule.released = true;
        }
        drop(state);
        self.shared.changed.notify_all();
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.release();
    }
}

/// A queued request, its result's type hidden.
trait Task: ClientTask + Send {
    fn op(&self) -> &Op;
    /// Answers the request (with `given`, if a rule gave a refusal or a reply), reads the
    /// answer as a worker's is read, in the span it was submitted in, and ends it: under the
    /// lock `advance` takes, as the pool does, it stops counting as running and its outcome
    /// is sent, superseded if its client has started a newer generation or is retired. Once
    /// `advance` returns, no result of the client's older generations can still arrive.
    fn run(self: Box<Self>, given: Option<Result<Reply, Error>>, shared: &Shared);
    /// Ends a request that started without running it, under the lock, as `run` does.
    fn stop(self: Box<Self>, shared: &Shared);
    /// Sends [`Outcome::Superseded`] for a request that never started.
    fn supersede(self: Box<Self>);
}

struct Submitted<T> {
    client: ClientId,
    generation: Generation,
    /// Set once its caller gives it up ([`Pending`]).
    cancelled: Arc<AtomicBool>,
    request: KernelRequest<T>,
    reply: SyncSender<Outcome<T>>,
    /// The span current where the request was submitted.
    span: tracing::Span,
}

impl<T> ClientTask for Submitted<T> {
    fn client(&self) -> ClientId {
        self.client
    }

    fn generation(&self) -> Generation {
        self.generation
    }

    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl<T: Send + 'static> Task for Submitted<T> {
    fn op(&self) -> &Op {
        self.request.op()
    }

    fn run(self: Box<Self>, given: Option<Result<Reply, Error>>, shared: &Shared) {
        let Submitted {
            client,
            generation,
            cancelled: _,
            request,
            reply,
            span,
        } = *self;
        let result = span.in_scope(|| {
            let _request = tracing::debug_span!(
                "fake_kernel_request",
                client = client.number(),
                generation = generation.0
            )
            .entered();
            let reply = given.unwrap_or_else(|| answer(request.op()));
            request.answer(reply)
        });
        let mut state = lock(&shared.state);
        state.running -= 1;
        let outcome = if state.clients.superseded(client, generation) {
            Outcome::Superseded
        } else {
            Outcome::Done(result)
        };
        deliver(state, &reply, outcome);
        shared.changed.notify_all();
    }

    fn stop(self: Box<Self>, shared: &Shared) {
        let mut state = lock(&shared.state);
        state.running -= 1;
        if state.stopping {
            // The kernel stopped: no outcome, which tells the caller so.
            drop(state);
            drop(self);
        } else {
            deliver(state, &self.reply, Outcome::Superseded);
        }
        shared.changed.notify_all();
    }

    fn supersede(self: Box<Self>) {
        let _ = self.reply.send(Outcome::Superseded);
    }
}

/// One of the kernel's threads: runs queued requests, the clients' in turn, until the
/// kernel stops.
fn serve(shared: &Shared) {
    loop {
        // Declared before the lock's guard, so dropped after it.
        let mut given_up = Vec::new();
        let mut state = lock(&shared.state);
        let task = loop {
            if state.stopping {
                return;
            }
            if let Some(task) = state.clients.next(&mut given_up) {
                break task;
            }
            state = shared
                .work
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        };
        state.running += 1;
        state.most_running = state.most_running.max(state.running);
        state.calls.push(Call {
            client: task.client(),
            generation: task.generation(),
            op: task.op().clone(),
        });
        shared.changed.notify_all();
        // The rules are a test's code: one that panics fails its request, naming it, and
        // leaves the kernel running.
        let mut holding = Vec::new();
        let mut broken = None;
        for (index, rule) in state.holds.iter().enumerate() {
            match caught(|| (rule.picks)(task.op())) {
                Some(true) => holding.push(index),
                Some(false) => {}
                None => broken = Some(rule_panicked("hold", task.op())),
            }
        }
        while broken.is_none()
            && !state.stopping
            && !state.clients.superseded(task.client(), task.generation())
            && holding
                .iter()
                .any(|&index| state.holds.get(index).is_some_and(|rule| !rule.released))
        {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if state.stopping || state.clients.superseded(task.client(), task.generation()) {
            drop(state);
            task.stop(shared);
            continue;
        }
        let refusal = broken.or_else(|| {
            state.refusals.iter().find_map(|refuses| {
                caught(|| refuses(task.op()))
                    .unwrap_or_else(|| Some(rule_panicked("refusal", task.op())))
            })
        });
        let given = match refusal {
            Some(error) => Some(Err(error)),
            None => state.answers.iter().find_map(|answers| {
                caught(|| answers(task.op())).map_or_else(
                    || Some(Err(rule_panicked("answer", task.op()))),
                    |reply| reply.map(Ok),
                )
            }),
        };
        drop(state);
        task.run(given, shared);
    }
}

/// `rule`'s answer, or `None` if it panicked.
fn caught<T>(rule: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(rule)).ok()
}

fn rule_panicked(kind: &str, op: &Op) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("the fake kernel's {kind} rule panicked on {}", op.name()),
    )
    .with_suggestion("fix the test's rule")
}

/// The fake writes STEP as it answers `Op::ExportStep` (a text saying what was asked, or a
/// test's rule's reply), as the STEP worker would, on its own client's generation.
impl crate::step_worker::StepExporter for FakeKernel {
    fn export_step(
        &self,
        export: silico_protocol::ExportStep,
        started: &dyn Fn(crate::step_worker::StepStop),
    ) -> Result<(silico_protocol::StepFile, String), Error> {
        let request = crate::kernel::KernelRequest::export_step(export)?;
        let generation = self.generation();
        let pending = self.submit(generation, request);
        // Stopped, as the STEP worker is killed: its request superseded.
        let shared = Arc::clone(&self.shared);
        started(crate::step_worker::StepStop::new(move || {
            shared.advance(ClientId::KERNEL, Generation(generation.0 + 1));
        }));
        match pending.wait() {
            Outcome::Done(file) => Ok((file?, "fake STEP worker".to_owned())),
            Outcome::Superseded => Err(Error::new(
                ErrorCode::WorkerCrashed,
                "the fake STEP worker was stopped",
            )
            .with_suggestion("the export was stopped: run it again")),
        }
    }
}

/// The fake's reply to `op`.
fn answer(op: &Op) -> Result<Reply, Error> {
    match op {
        Op::MakePrimitive(primitive) => Ok(Reply::Shape(Symbolic::primitive(*primitive).shape())),
        Op::Translate { shape, offset } => {
            let made = Symbolic::read(shape)?;
            let counts = made.counts();
            // As the kernel reports a move: each element modified into its own number (a
            // moved element isn't the same sub-shape), or kept by a move of nothing.
            let fates = if nothing(offset) {
                kept(counts, ElementCounts::default())
            } else {
                moved(counts)
            };
            let history = History {
                inputs: vec![fates],
                result: counts,
                ends: Ends::default(),
                topology: made.topology(),
            };
            let shape = made.moved(*offset).shape();
            Ok(Reply::Built(BuiltData { shape, history }))
        }
        Op::Boolean { op, base, tool } => {
            let (base, tool) = (Symbolic::read(base)?, Symbolic::read(tool)?);
            let (a, b) = (base.counts(), tool.counts());
            let made = Symbolic::boolean(*op, base, tool);
            let history = History {
                inputs: vec![kept(a, ElementCounts::default()), kept(b, a)],
                result: boolean_counts(*op, a, b),
                ends: Ends::default(),
                topology: made.topology(),
            };
            let shape = made.shape();
            Ok(Reply::Built(BuiltData { shape, history }))
        }
        Op::Copies {
            base,
            tool,
            op,
            placements,
        } => copies(*op, base, tool, placements),
        Op::Fillet {
            shape,
            edges,
            radius,
        } => dressed(shape, edges, *radius, false),
        Op::Chamfer {
            shape, edges, size, ..
        } => match size {
            silico_protocol::ChamferSize::Equal { distance } => {
                dressed(shape, edges, *distance, true)
            }
            _ => Err(Error::new(
                ErrorCode::Internal,
                "the fake kernel chamfers by one distance only",
            )
            .with_suggestion("run this test on a real kernel worker")),
        },
        Op::MakeFaces(regions) => {
            // The request was checked when it was made; checked again, as the worker does.
            if let Err(invalid) = regions.check() {
                return Err(crate::kernel::faces_refused(invalid));
            }
            let layout = FacesLayout::of(regions);
            let kept_as = |numbers: &[u32]| -> Vec<ElementFate> {
                numbers
                    .iter()
                    .map(|&n| ElementFate {
                        kept: Some(n),
                        ..ElementFate::default()
                    })
                    .collect()
            };
            let curves: Vec<u32> = (1..=layout.counts.edges).collect();
            let history = History {
                inputs: vec![InputHistory {
                    faces: Vec::new(),
                    edges: kept_as(&curves),
                    vertices: kept_as(&layout.points),
                }],
                result: layout.counts,
                ends: Ends::default(),
                topology: layout.topology,
            };
            let shape = Symbolic::faces(regions).shape();
            Ok(Reply::Built(BuiltData { shape, history }))
        }
        Op::Prism {
            faces,
            start,
            vector,
        } => swept(
            faces,
            Sweep::Prism {
                start: *start,
                vector: *vector,
            },
        ),
        Op::PrismThroughAll {
            faces,
            base,
            direction,
        } => {
            // As the kernel: the faces, and the direction out of their plane, first; then
            // sized by the protocol's rule from the fake's boxes of the base and the faces,
            // and a length that makes no prism the rules admit is the base's doing.
            let regions = swept_faces(faces)?;
            flat_check(
                "PrismThroughAll",
                silico_protocol::check_leaves_plane(
                    "direction",
                    regions.plane.normal(),
                    *direction,
                ),
                "direction",
            )?;
            let [low, high] = Symbolic::read(base)?.summary().bounds.unwrap_or_default();
            let [f_low, f_high] = Symbolic::read(faces)?.summary().bounds.unwrap_or_default();
            let around = [
                [0, 1, 2].map(|k| low[k].min(f_low[k])),
                [0, 1, 2].map(|k| high[k].max(f_high[k])),
            ];
            let length = silico_protocol::through_all_length(around)
                .map_err(|invalid| crate::kernel::refused("through-all prism", None, invalid))?;
            let vector = direction.map(|c| c * length);
            silico_protocol::check_prism([0.0; 3], vector).map_err(|invalid| {
                crate::kernel::refused(
                    "through-all prism",
                    None,
                    silico_protocol::InvalidParameter {
                        parameter: "base",
                        message: invalid.message,
                    },
                )
            })?;
            swept(
                faces,
                Sweep::Prism {
                    start: [0.0; 3],
                    vector,
                },
            )
        }
        Op::Revolution {
            faces,
            axis,
            start,
            angle,
        } => swept(
            faces,
            Sweep::Revolution {
                axis: *axis,
                start: *start,
                angle: *angle,
            },
        ),
        Op::Summarize { shape } => Ok(Reply::Summary(Symbolic::read(shape)?.summary())),
        Op::Tessellate {
            shape,
            deflection,
            at_least_the_floor,
        } => fake_mesh(&Symbolic::read(shape)?, *deflection, *at_least_the_floor),
        // The fake's solids are their boxes: a point is inside one if it's strictly within
        // its box, on it if it's on the box's boundary.
        Op::Classify { shape, point } => {
            if let Err(invalid) = silico_protocol::check_classify_point(*point) {
                return Err(Error::new(ErrorCode::InvalidInput, invalid.message)
                    .with_parameter(invalid.parameter)
                    .with_suggestion("change the value the message names"));
            }
            let place = match Symbolic::read(shape)?.summary().bounds {
                Some([low, high]) if (0..3).all(|k| low[k] < point[k] && point[k] < high[k]) => {
                    PointPlace::Inside
                }
                Some([low, high]) if (0..3).all(|k| low[k] <= point[k] && point[k] <= high[k]) => {
                    PointPlace::OnBoundary
                }
                _ => PointPlace::Outside,
            };
            Ok(Reply::Classified { place })
        }
        Op::Describe {
            shape,
            kind,
            first,
            count,
        } => described(&Symbolic::read(shape)?.counts(), *kind, *first, *count),
        Op::DescribeAll { shape } => described_all(&Symbolic::read(shape)?.counts()),
        Op::Distance {
            shape,
            first,
            other,
            second,
        } => {
            let first_counts = Symbolic::read(shape)?.counts();
            let second_counts = match other {
                Some(other) => Symbolic::read(other)?.counts(),
                None => first_counts,
            };
            distance(&first_counts, *first, &second_counts, *second)
        }
        Op::PointDistance {
            shape,
            element,
            point,
        } => point_distance(&Symbolic::read(shape)?.counts(), *element, *point),
        Op::Echo { data } => Ok(Reply::Echo { data: data.clone() }),
        Op::ExportStep(export) => step_file(export),
        other => Err(Error::new(
            ErrorCode::Internal,
            format!("the fake kernel doesn't do {}", other.name()),
        )
        .with_suggestion("run this test on a real kernel worker")),
    }
}

/// The fake's STEP file of `export`: a text saying what was asked, line by line (the
/// header's file name; the assembly, if there are two or more parts; each part's product id,
/// name, and how the fake made its shape), so a test reads what the core sent; no face
/// left out, nothing dropped (neither faces nor their bounds counted).
fn step_file(export: &silico_protocol::ExportStep) -> Result<Reply, Error> {
    let mut text = format!("FAKE STEP\nFILE_NAME('{}')\n", export.file_name);
    if export.parts.len() > 1 {
        text.push_str(&format!("ASSEMBLY('{}')\n", export.assembly));
    }
    for part in &export.parts {
        let made = Symbolic::read(&part.shape)?;
        text.push_str(&format!(
            "PRODUCT('{}','{}') {}\n",
            part.id,
            part.name,
            made.describe()
        ));
    }
    Ok(Reply::StepFile(Box::new(silico_protocol::StepFile {
        step: Blob::inline(text.into_bytes()),
        left_out: vec![Vec::new(); export.parts.len()],
        unwritten: Vec::new(),
        faces: silico_protocol::StepCount {
            expected: 0,
            written: 0,
        },
        bounds: silico_protocol::StepCount {
            expected: 0,
            written: 0,
        },
    })))
}

/// The fake's display mesh of `made`, as the kernel's is checked: a triangle for each face,
/// a two-point polyline for each edge, and a point for each vertex, all on its box's corners (a mesh that holds
/// together, for the core's render, not one of the shape), at `deflection`. Its floor is
/// the kernel's rule, `mesh_floor`, on the fake's box.
/// Under it, refused as the kernel refuses, or with `at_least_the_floor` meshed at it and
/// said.
fn fake_mesh(made: &Symbolic, deflection: f64, at_least_the_floor: bool) -> Result<Reply, Error> {
    if !(deflection.is_finite() && deflection > 0.0) {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("deflection {deflection} must be finite and positive"),
        )
        .with_parameter("deflection")
        .with_suggestion("change the value the message names"));
    }
    let summary = made.summary();
    let [low, high] = summary.bounds.unwrap_or([[0.0; 3]; 2]);
    let diagonal = (0..3)
        .map(|k| (high[k] - low[k]) * (high[k] - low[k]))
        .sum::<f64>()
        .sqrt();
    let floor = silico_protocol::mesh_floor(diagonal);
    let used = if deflection >= floor {
        deflection
    } else if at_least_the_floor {
        floor
    } else {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("deflection {deflection} is finer than the shape's floor, {floor}"),
        )
        .with_parameter("deflection")
        .with_suggestion("ask for a coarser deflection"));
    };
    let corners = [low, [high[0], low[1], low[2]], [low[0], high[1], high[2]]];
    let faces = usize::try_from(summary.faces).unwrap_or(0);
    let edges = usize::try_from(summary.edges).unwrap_or(0);
    let vertices = usize::try_from(summary.vertices).unwrap_or(0);
    let mesh = silico_protocol::Mesh {
        positions: corners.to_vec(),
        normals: vec![[0.0, 0.0, 1.0]; 3],
        triangles: vec![[0, 1, 2]; faces],
        face_triangles: vec![1; faces],
        edge_points: (0..edges).flat_map(|_| [low, high]).collect(),
        edge_point_counts: vec![2; edges],
        vertex_points: vec![high; vertices],
        valid: None,
    };
    Ok(match at_least_the_floor {
        true => Reply::MeshAt(silico_protocol::MeshAt {
            mesh,
            deflection: used,
        }),
        false => Reply::Mesh(mesh),
    })
}

/// The fake's distance between `first`, of a shape whose counts are `first_counts`, and
/// `second`, of one whose counts are `second_counts`: each element at (n, 0, 0), its number
/// `n`, as its description has a face's centre and a vertex (`described`), so the distance
/// is how far apart the numbers are, met there, once. An element the shape hasn't is
/// refused, naming `first` or `second`, as a worker refuses it, in the same words.
fn distance(
    first_counts: &ElementCounts,
    first: ElementRef,
    second_counts: &ElementCounts,
    second: ElementRef,
) -> Result<Reply, Error> {
    // As a worker refuses them, worded as the worker client words a worker's refusal.
    let refused = |refusal| {
        Err(crate::worker::failure_error(
            "Distance",
            silico_protocol::Failure::new(refusal),
        ))
    };
    if let Err(refusal) = silico_protocol::check_distance(first, second) {
        return refused(refusal);
    }
    for (element, counts, parameter) in [
        (first, first_counts, "first"),
        (second, second_counts, "second"),
    ] {
        let count = counts.of(element.kind);
        if element.index > count {
            return refused(silico_protocol::Refusal::ElementMissing {
                parameter: parameter.to_owned(),
                element,
                count,
            });
        }
    }
    let at = |element: ElementRef| [f64::from(element.index), 0.0, 0.0];
    Ok(Reply::Distance(silico_protocol::Distance {
        distance: (f64::from(first.index) - f64::from(second.index)).abs(),
        from: at(first),
        to: at(second),
        solutions: 1,
    }))
}

/// The fake's distance from `point` to `element`, of a shape whose counts are `counts`: the
/// element at (n, 0, 0), its number `n`, as [`distance`] places it, so the distance is the
/// point's from there, met there, once. Refused as a worker refuses it, in the same words.
fn point_distance(
    counts: &ElementCounts,
    element: ElementRef,
    point: [f64; 3],
) -> Result<Reply, Error> {
    let refused = |refusal| {
        Err(crate::worker::failure_error(
            "PointDistance",
            silico_protocol::Failure::new(refusal),
        ))
    };
    if let Err(refusal) = silico_protocol::check_point_distance(element, point) {
        return refused(refusal);
    }
    let count = counts.of(element.kind);
    if element.index > count {
        return refused(silico_protocol::Refusal::ElementMissing {
            parameter: "element".to_owned(),
            element,
            count,
        });
    }
    let at = [f64::from(element.index), 0.0, 0.0];
    let d = sub(point, at);
    Ok(Reply::Distance(silico_protocol::Distance {
        distance: dot(d, d).sqrt(),
        from: point,
        to: at,
        solutions: 1,
    }))
}

/// Every element of a shape of `counts` described ([`Op::DescribeAll`]), each as
/// [`described`] describes it; refused past a batch in all, as a worker refuses it: as its
/// fault (`internal`).
fn described_all(counts: &ElementCounts) -> Result<Reply, Error> {
    let all = u64::from(counts.faces) + u64::from(counts.edges) + u64::from(counts.vertices);
    if all > u64::from(DESCRIBE_BATCH) {
        return Err(Error::new(
            ErrorCode::Internal,
            format!(
                "the fake kernel describes at most {DESCRIBE_BATCH} elements at once, not {all}"
            ),
        ));
    }
    described_all_any_size(counts)
}

/// [`described_all`] of a shape of any size.
fn described_all_any_size(counts: &ElementCounts) -> Result<Reply, Error> {
    let of = |kind, count| match described(counts, kind, 1, count)? {
        Reply::Described { description, .. } => Ok(description),
        other => Err(Error::new(
            ErrorCode::Internal,
            format!("the fake kernel described with a {} reply", other.kind()),
        )),
    };
    Ok(Reply::DescribedAll(Description {
        faces: of(ElementKind::Face, counts.faces)?.faces,
        edges: of(ElementKind::Edge, counts.edges)?.edges,
        vertices: of(ElementKind::Vertex, counts.vertices)?.vertices,
    }))
}

/// The fake's description of a shape whose counts are `counts`: its `kind`'s elements from
/// `first`, `count` of them or as many as there are, each standing for its number `n` alone
/// (a face of type other, area 1, centred at (n, 0, 0); an edge of type other from (n, 0,
/// 0) to (n, 1, 0), 1 long; a vertex at (n, 0, 0)), and the kind's total. Enough for a
/// test of what's done with a description; a real worker's geometry is tested on a real
/// worker.
fn described(
    counts: &ElementCounts,
    kind: ElementKind,
    first: u32,
    count: u32,
) -> Result<Reply, Error> {
    let total = match kind {
        ElementKind::Face => counts.faces,
        ElementKind::Edge => counts.edges,
        ElementKind::Vertex => counts.vertices,
        ElementKind::Solid => {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "the fake kernel describes faces, edges, and vertices, not solids",
            ));
        }
    };
    let numbers = first..=total.min(first.saturating_add(count).saturating_sub(1));
    let at = |n: u32, y: f64| [f64::from(n), y, 0.0];
    let mut description = Description {
        faces: Vec::new(),
        edges: Vec::new(),
        vertices: Vec::new(),
    };
    for n in numbers {
        match kind {
            ElementKind::Face => description.faces.push(FaceDescription {
                surface: Surface::Other,
                area: 1.0,
                centroid: at(n, 0.0),
            }),
            ElementKind::Edge => description.edges.push(EdgeDescription {
                curve: EdgeCurve::Other,
                length: 1.0,
                start: at(n, 0.0),
                end: at(n, 1.0),
                middle: at(n, 0.5),
            }),
            ElementKind::Vertex | ElementKind::Solid => description.vertices.push(at(n, 0.0)),
        }
    }
    Ok(Reply::Described { total, description })
}

/// The fake's sweep of `faces`, which must be faces a fake kernel made from regions, as the
/// kernel refuses a shape that isn't faces.
fn swept(faces: &ShapeData, sweep: Sweep) -> Result<Reply, Error> {
    let regions = swept_faces(faces)?;
    let normal = regions.plane.normal();
    match sweep {
        Sweep::Prism { vector, .. } => flat_check(
            "Prism",
            silico_protocol::check_leaves_plane("vector", normal, vector),
            "vector",
        )?,
        Sweep::Revolution { axis, .. } => flat_check(
            "Revolution",
            silico_protocol::check_axis_in_plane(normal, axis.direction),
            "axis",
        )?,
    }
    let made = Symbolic::read(faces)?;
    let layout = FacesLayout::of(&regions);
    let numbers = SweepNumbers::of(&layout, sweep.kind());
    let history = History {
        inputs: vec![numbers.fates()],
        result: numbers.counts,
        ends: Ends::default(),
        topology: numbers.topology(&layout),
    };
    let Symbolic(mut steps) = made;
    steps.push(Step::Sweep { input: 0, sweep });
    Ok(Reply::Built(BuiltData {
        shape: Symbolic(steps).shape(),
        history,
    }))
}

/// The regions of `faces`, if they're faces a fake kernel made from regions, as the kernel
/// refuses a shape that isn't faces.
fn swept_faces(faces: &ShapeData) -> Result<FaceRegions, Error> {
    let made = Symbolic::read(faces)?;
    match made.0.as_slice() {
        [Step::Faces(regions)] => Ok(regions.as_ref().clone()),
        _ => Err(Error::new(
            ErrorCode::InvalidInput,
            format!("a sweep takes a profile's faces, got {}", made.describe()),
        )
        .with_parameter("faces")
        .with_suggestion("sweep the faces the kernel made of a profile")),
    }
}

/// A sweep the plane checks refuse, as the kernel refuses it: its `Refusal::SweepFlat`,
/// worded by the same path a worker's reply takes (`worker::failure_error`), so the two
/// can't drift.
fn flat_check(
    op: &str,
    checked: Result<(), silico_protocol::InvalidParameter>,
    parameter: &str,
) -> Result<(), Error> {
    checked.map_err(|invalid| {
        crate::worker::failure_error(
            op,
            silico_protocol::Failure::new(silico_protocol::Refusal::SweepFlat {
                parameter: parameter.to_owned(),
                message: invalid.message,
            }),
        )
    })
}

/// A sweep of faces, as the fake keeps it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Sweep {
    Prism { start: [f64; 3], vector: [f64; 3] },
    Revolution { axis: Axis, start: f64, angle: f64 },
}

impl Sweep {
    fn kind(&self) -> SweepKind {
        match *self {
            Sweep::Prism { .. } => SweepKind::Prism,
            Sweep::Revolution { angle, .. } => SweepKind::of_turn(angle),
        }
    }

    /// Whether the kernel's rules accept its values.
    fn checked(&self) -> bool {
        match *self {
            Sweep::Prism { start, vector } => silico_protocol::check_prism(start, vector).is_ok(),
            Sweep::Revolution { axis, start, angle } => {
                silico_protocol::check_revolution(axis.origin, axis.direction, start, angle).is_ok()
            }
        }
    }

    /// Whether it sweeps faces whose plane's normal is `normal` into something with a
    /// volume, by the kernel's plane checks: a prism out of the plane, a turn about an axis
    /// in it.
    fn leaves(&self, normal: [f64; 3]) -> bool {
        match *self {
            Sweep::Prism { vector, .. } => {
                silico_protocol::check_leaves_plane("vector", normal, vector).is_ok()
            }
            Sweep::Revolution { axis, .. } => {
                silico_protocol::check_axis_in_plane(normal, axis.direction).is_ok()
            }
        }
    }

    /// Its values, in the order a shape's bytes write them.
    fn values(&self) -> Vec<f64> {
        match *self {
            Sweep::Prism { start, vector } => start.into_iter().chain(vector).collect(),
            Sweep::Revolution { axis, start, angle } => axis
                .origin
                .into_iter()
                .chain(axis.direction)
                .chain([start, angle])
                .collect(),
        }
    }

    fn word(&self) -> &'static str {
        match self {
            Sweep::Prism { .. } => "prism",
            Sweep::Revolution { .. } => "revolution",
        }
    }

    fn parse(word: &str, v: &[f64]) -> Option<Self> {
        let sweep = match (word, v) {
            ("prism", &[sx, sy, sz, vx, vy, vz]) => Sweep::Prism {
                start: [sx, sy, sz],
                vector: [vx, vy, vz],
            },
            ("revolution", &[ox, oy, oz, dx, dy, dz, start, angle]) => Sweep::Revolution {
                axis: Axis {
                    origin: [ox, oy, oz],
                    direction: [dx, dy, dz],
                },
                start,
                angle,
            },
            _ => return None,
        };
        sweep.checked().then_some(sweep)
    }
}

/// How the fake numbers a sweep's result, from its faces' F faces, E edges, and V vertices.
/// A prism or a turn: side faces 1..=E (edge i's is face i), then the start faces, then the
/// end faces; start edges 1..=E, end edges E+1..=2E, side edges after them (vertex j's is
/// 2E + j); start vertices 1..=V, end vertices V+1..=2V; face k's solid k. A whole turn:
/// faces 1..=E (edge i's), seams 1..=E (edge i's start), circles E+1..=E+V (vertex j's
/// side), vertices 1..=V (each vertex its own start).
struct SweepNumbers {
    whole: bool,
    faces: u32,
    edges: u32,
    vertices: u32,
    counts: ElementCounts,
}

impl SweepNumbers {
    fn of(layout: &FacesLayout, kind: SweepKind) -> Self {
        let ElementCounts {
            faces: f,
            edges: e,
            vertices: v,
            ..
        } = layout.counts;
        let whole = kind == SweepKind::WholeTurn;
        let counts = if whole {
            ElementCounts {
                faces: e,
                edges: e.saturating_add(v),
                vertices: v,
                solids: f,
            }
        } else {
            ElementCounts {
                faces: f.saturating_mul(2).saturating_add(e),
                edges: e.saturating_mul(2).saturating_add(v),
                vertices: v.saturating_mul(2),
                solids: f,
            }
        };
        Self {
            whole,
            faces: f,
            edges: e,
            vertices: v,
            counts,
        }
    }

    fn fates(&self) -> InputHistory {
        let (f, e, v) = (self.faces, self.edges, self.vertices);
        let r = |kind, index| Reported {
            element: ElementRef { kind, index },
            position: 1,
        };
        let fate = |start: Option<Reported>, mut generated: Vec<Reported>| {
            generated.sort();
            ElementFate {
                kept: None,
                modified: start.into_iter().collect(),
                generated,
            }
        };
        use ElementKind::{Edge, Face, Solid, Vertex};
        if self.whole {
            return InputHistory {
                faces: (1..=f).map(|_| ElementFate::default()).collect(),
                edges: (1..=e)
                    .map(|i| fate(Some(r(Edge, i)), vec![r(Face, i)]))
                    .collect(),
                vertices: (1..=v)
                    .map(|j| fate(Some(r(Vertex, j)), vec![r(Edge, e + j)]))
                    .collect(),
            };
        }
        InputHistory {
            faces: (1..=f)
                .map(|k| fate(Some(r(Face, e + k)), vec![r(Face, e + f + k), r(Solid, k)]))
                .collect(),
            edges: (1..=e)
                .map(|i| fate(Some(r(Edge, i)), vec![r(Edge, e + i), r(Face, i)]))
                .collect(),
            vertices: (1..=v)
                .map(|j| {
                    fate(
                        Some(r(Vertex, j)),
                        vec![r(Vertex, v + j), r(Edge, 2 * e + j)],
                    )
                })
                .collect(),
        }
    }

    /// How the result's elements bound each other, by the numbering above, from the faces'.
    fn topology(&self, layout: &FacesLayout) -> Topology {
        let (e, v) = (self.edges, self.vertices);
        let sorted = |mut list: Vec<u32>| {
            list.sort_unstable();
            list
        };
        let input = &layout.topology;
        let mut topology = Topology::default();
        // Each edge's face: its start and end (or its seam), and its vertices' sides.
        for (i, ends) in (1u32..).zip(&input.edge_vertices) {
            let sides = ends
                .iter()
                .map(|&j| if self.whole { e + j } else { 2 * e + j });
            let edges: Vec<u32> = if self.whole {
                std::iter::once(i).chain(sides).collect()
            } else {
                [i, e + i].into_iter().chain(sides).collect()
            };
            topology.outer_edges.push(sorted(edges.clone()));
            topology.face_edges.push(edges);
        }
        if !self.whole {
            for shift in [0, e] {
                for (all, outer) in input.face_edges.iter().zip(&input.outer_edges) {
                    topology
                        .face_edges
                        .push(all.iter().map(|&i| i + shift).collect());
                    topology
                        .outer_edges
                        .push(sorted(outer.iter().map(|&i| i + shift).collect()));
                }
            }
        }
        let shifted = |ends: &Vec<u32>, by: u32| ends.iter().map(|&j| j + by).collect();
        topology
            .edge_vertices
            .extend(input.edge_vertices.iter().map(|ends| shifted(ends, 0)));
        if !self.whole {
            topology
                .edge_vertices
                .extend(input.edge_vertices.iter().map(|ends| shifted(ends, v)));
        }
        let circle = |j: u32| if self.whole { vec![j] } else { vec![j, v + j] };
        topology.edge_vertices.extend((1..=v).map(circle));
        // Face k's solid: its edges' side faces, and its start and end faces (a whole turn's
        // none).
        let f = self.faces;
        for (k, edges) in (1u32..).zip(&input.face_edges) {
            let mut faces = edges.clone();
            if !self.whole {
                faces.extend([e + k, e + f + k]);
            }
            topology.solid_faces.push(sorted(faces));
        }
        topology
    }
}

/// The fake's copies of `tool` at `placements`, with `base`: every element of the base and
/// of each copy kept, the base's numbered first, then each copy's after the copy before it,
/// as the fake's boolean keeps its inputs'. Placements the protocol's rules refuse are
/// refused, as the kernel refuses them.
fn copies(
    op: CopiesOp,
    base: &ShapeData,
    tool: &ShapeData,
    placements: &[Placement],
) -> Result<Reply, Error> {
    if let Err(refused) = silico_protocol::check_copies(placements) {
        return Err(Error::new(ErrorCode::InvalidInput, refused.message)
            .with_parameter(refused.parameter)
            .with_suggestion("change the value the message names"));
    }
    let (base, tool) = (Symbolic::read(base)?, Symbolic::read(tool)?);
    let (a, b) = (base.counts(), tool.counts());
    let mut inputs = vec![kept(a, ElementCounts::default())];
    let mut before = a;
    for _ in placements {
        inputs.push(kept(b, before));
        before = sum(before, b);
    }
    let made = Symbolic::copies(op, base, tool, placements);
    let history = History {
        inputs,
        result: made.counts(),
        ends: Ends::default(),
        topology: made.topology(),
    };
    Ok(Reply::Built(BuiltData {
        shape: made.shape(),
        history,
    }))
}

/// The fake's fillet or chamfer of `edges` of `shape`: every element kept, as numbered;
/// edges the shape doesn't have refused, as the kernel refuses them.
fn dressed(shape: &ShapeData, edges: &[u32], size: f64, chamfer: bool) -> Result<Reply, Error> {
    let made = Symbolic::read(shape)?;
    let counts = made.counts();
    if edges.is_empty() || edges.iter().any(|&e| e == 0 || e > counts.edges) {
        return Err(Error::new(
            ErrorCode::KernelFailed,
            format!(
                "the fake kernel can't dress edges {edges:?} of a shape with {} edges",
                counts.edges
            ),
        )
        .with_parameter("edges"));
    }
    let history = History {
        inputs: vec![kept(counts, ElementCounts::default())],
        result: counts,
        ends: Ends::default(),
        topology: made.topology(),
    };
    let Symbolic(mut steps) = made;
    let input = steps.len() - 1;
    steps.push(Step::Dress {
        chamfer,
        input,
        size,
        edges: edges.to_vec(),
    });
    Ok(Reply::Built(BuiltData {
        shape: Symbolic(steps).shape(),
        history,
    }))
}

/// Whether a move by `offset` is a move by nothing: every component zero, of either sign.
fn nothing(offset: &[f64; 3]) -> bool {
    offset.iter().all(|&d| d == 0.0)
}

fn sum(a: ElementCounts, b: ElementCounts) -> ElementCounts {
    ElementCounts {
        faces: a.faces.saturating_add(b.faces),
        edges: a.edges.saturating_add(b.edges),
        vertices: a.vertices.saturating_add(b.vertices),
        solids: a.solids.saturating_add(b.solids),
    }
}

/// A boolean's counts, as the fake models it: both inputs' faces, edges, and vertices, and
/// the solids of one that met: a fuse's one (none of none), a cut's and a common's its
/// base's. So a boolean of the fake is one solid, as D2 holds them (`sweep`'s checks): the
/// fake can't model a split, which tests of that rule build on the real kernel or write by
/// hand.
fn boolean_counts(op: BooleanOp, a: ElementCounts, b: ElementCounts) -> ElementCounts {
    ElementCounts {
        solids: match op {
            BooleanOp::Fuse => u32::from(a.solids > 0 || b.solids > 0),
            BooleanOp::Cut | BooleanOp::Common => a.solids,
        },
        ..sum(a, b)
    }
}

/// The counts of `n` copies of a tool of `b` with a base of `a`, as the fake models them: all
/// their faces, edges, and vertices, and the solids of a boolean of them that met, as
/// [`boolean_counts`] has them.
fn copies_counts(op: CopiesOp, a: ElementCounts, b: ElementCounts, n: usize) -> ElementCounts {
    let n = u32::try_from(n).unwrap_or(u32::MAX);
    let copies = ElementCounts {
        faces: b.faces.saturating_mul(n),
        edges: b.edges.saturating_mul(n),
        vertices: b.vertices.saturating_mul(n),
        solids: b.solids.saturating_mul(n),
    };
    boolean_counts(op.boolean(), a, copies)
}

/// Each solid's faces in a shape of `counts` whose faces are all one solid's, if it has one
/// solid: every face. Otherwise none, or, for several, `several`.
fn one_solid(counts: ElementCounts, several: Vec<Vec<u32>>) -> Vec<Vec<u32>> {
    match counts.solids {
        0 => Vec::new(),
        1 => vec![(1..=counts.faces).collect()],
        _ => several,
    }
}

/// How OCCT's primitive's elements bound each other, as the kernel reports it: each face's
/// edges in OCCT's order, its outer wire's (sorted), each edge's vertices, and its one
/// solid's faces.
fn primitive_topology(primitive: &Primitive) -> Topology {
    let lists = |lists: &[&[u32]]| lists.iter().map(|l| l.to_vec()).collect::<Vec<_>>();
    type Lists = &'static [&'static [u32]];
    let (faces, outer, vertices): (Lists, Lists, Lists) = match *primitive {
        Primitive::Box { .. } => (
            &[
                &[1, 2, 3, 4],
                &[5, 6, 7, 8],
                &[9, 5, 10, 1],
                &[11, 7, 12, 3],
                &[4, 11, 8, 9],
                &[2, 12, 6, 10],
            ],
            &[
                &[1, 2, 3, 4],
                &[5, 6, 7, 8],
                &[1, 5, 9, 10],
                &[3, 7, 11, 12],
                &[4, 8, 9, 11],
                &[2, 6, 10, 12],
            ],
            &[
                &[1, 2],
                &[3, 1],
                &[3, 4],
                &[4, 2],
                &[5, 6],
                &[7, 5],
                &[7, 8],
                &[8, 6],
                &[6, 2],
                &[5, 1],
                &[8, 4],
                &[7, 3],
            ],
        ),
        Primitive::Cylinder { .. } => (
            &[&[1, 2, 3], &[2], &[3]],
            &[&[1, 2, 3], &[2], &[3]],
            &[&[1, 2], &[1], &[2]],
        ),
        Primitive::Sphere { .. } => (&[&[1, 2, 3]], &[&[1, 2, 3]], &[&[1], &[1, 2], &[2]]),
        Primitive::Cone {
            radius1,
            radius2,
            height,
        } => {
            let faces: Lists = match cone_caps(radius1, radius2, height) {
                (true, true) => &[&[1, 2, 3], &[1], &[3]],
                (true, false) => &[&[1, 2, 3], &[3]],
                _ => &[&[1, 2, 3], &[1]],
            };
            (faces, faces, &[&[1], &[1, 2], &[2]])
        }
        Primitive::Torus { .. } => (&[&[1, 2]], &[&[1, 2]], &[&[1], &[1]]),
    };
    let face_count = u32::try_from(faces.len()).unwrap_or(u32::MAX);
    Topology {
        face_edges: lists(faces),
        outer_edges: lists(outer),
        edge_vertices: lists(vertices),
        solid_faces: vec![(1..=face_count).collect()],
    }
}

/// `topology` with every element numbered after `before`'s.
fn shifted(topology: &Topology, before: ElementCounts) -> Topology {
    let shift = |lists: &[Vec<u32>], by: u32| -> Vec<Vec<u32>> {
        lists
            .iter()
            .map(|l| l.iter().map(|e| e.saturating_add(by)).collect())
            .collect()
    };
    Topology {
        face_edges: shift(&topology.face_edges, before.edges),
        outer_edges: shift(&topology.outer_edges, before.edges),
        edge_vertices: shift(&topology.edge_vertices, before.vertices),
        solid_faces: shift(&topology.solid_faces, before.faces),
    }
}

/// An input's history where it's kept whole, its elements numbered after `before`'s.
fn kept(counts: ElementCounts, before: ElementCounts) -> InputHistory {
    let fates = |count: u32, after: u32| -> Vec<ElementFate> {
        (1..=count)
            .map(|n| ElementFate {
                kept: Some(after.saturating_add(n)),
                ..ElementFate::default()
            })
            .collect()
    };
    InputHistory {
        faces: fates(counts.faces, before.faces),
        edges: fates(counts.edges, before.edges),
        vertices: fates(counts.vertices, before.vertices),
    }
}

/// An input's history where each element is modified into the result's element with its
/// own number, first and only in OCCT's list.
fn moved(counts: ElementCounts) -> InputHistory {
    let fates = |kind: ElementKind, count: u32| -> Vec<ElementFate> {
        (1..=count)
            .map(|index| ElementFate {
                modified: vec![Reported {
                    element: ElementRef { kind, index },
                    position: 1,
                }],
                ..ElementFate::default()
            })
            .collect()
    };
    InputHistory {
        faces: fates(ElementKind::Face, counts.faces),
        edges: fates(ElementKind::Edge, counts.edges),
        vertices: fates(ElementKind::Vertex, counts.vertices),
    }
}

/// The first line of a fake shape's bytes.
const HEADER: &str = "silico fake shape 1";

/// One step of making a shape: a primitive, faces from a profile's regions, or an operation
/// on earlier steps, by index.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Primitive(Primitive),
    Faces(Box<FaceRegions>),
    Translate {
        input: usize,
        offset: [f64; 3],
    },
    Boolean {
        op: BooleanOp,
        base: usize,
        tool: usize,
    },
    /// A sweep of the faces of the step before it: faces, and only faces, are swept.
    Sweep {
        input: usize,
        sweep: Sweep,
    },
    /// Copies of `tool` at `placements`, fused into or cut from `base`, as a boolean's
    /// inputs are laid out: the base's steps, then the tool's, which end just before it.
    Copies {
        op: CopiesOp,
        base: usize,
        tool: usize,
        placements: Vec<Placement>,
    },
    /// The fake's fillet or chamfer of the step before it: of `edges`, by `size` (a
    /// fillet's radius, an equal chamfer's distance). Every element is kept, as numbered;
    /// its measures are its input's.
    Dress {
        chamfer: bool,
        input: usize,
        size: f64,
        edges: Vec<u32>,
    },
}

/// A fake shape: the steps that made it, the shape the last, in canonical order. Each
/// step's inputs come just before it: a move's input is the step before it, and a boolean's
/// base's steps come before its tool's, which end just before it. A move's input is never a
/// move (moves compose), and no move is by nothing.
#[derive(Debug, Clone, PartialEq)]
struct Symbolic(Vec<Step>);

impl Symbolic {
    fn primitive(primitive: Primitive) -> Self {
        Symbolic(vec![Step::Primitive(primitive)])
    }

    fn faces(regions: &FaceRegions) -> Self {
        Symbolic(vec![Step::Faces(Box::new(regions.clone()))])
    }

    /// This shape moved by `offset`, as the kernel composes locations.
    fn moved(mut self, offset: [f64; 3]) -> Self {
        if nothing(&offset) {
            return self;
        }
        let last = self.0.len() - 1;
        match self.0.pop() {
            // A move of a move: one move, of what that moved, by the sum.
            Some(Step::Translate {
                input: moved,
                offset: before,
            }) => {
                let total = [0, 1, 2].map(|i| before[i] + offset[i]);
                if !nothing(&total) {
                    self.0.push(Step::Translate {
                        input: moved,
                        offset: total,
                    });
                }
            }
            Some(step) => {
                self.0.push(step);
                self.0.push(Step::Translate {
                    input: last,
                    offset,
                });
            }
            None => {}
        }
        self
    }

    fn boolean(op: BooleanOp, base: Symbolic, tool: Symbolic) -> Self {
        Self::joined(base, tool, |base, tool| Step::Boolean { op, base, tool })
    }

    fn copies(op: CopiesOp, base: Symbolic, tool: Symbolic, placements: &[Placement]) -> Self {
        Self::joined(base, tool, |base, tool| Step::Copies {
            op,
            base,
            tool,
            placements: placements.to_vec(),
        })
    }

    /// `base`'s steps, then `tool`'s, then the step `made` makes of the two (by their
    /// indices).
    fn joined(base: Symbolic, tool: Symbolic, made: impl FnOnce(usize, usize) -> Step) -> Self {
        let Symbolic(mut steps) = base;
        let shift = steps.len();
        steps.extend(tool.0.into_iter().map(|step| match step {
            Step::Primitive(_) | Step::Faces(_) => step,
            Step::Translate { input, offset } => Step::Translate {
                input: input + shift,
                offset,
            },
            Step::Boolean { op, base, tool } => Step::Boolean {
                op,
                base: base + shift,
                tool: tool + shift,
            },
            Step::Copies {
                op,
                base,
                tool,
                placements,
            } => Step::Copies {
                op,
                base: base + shift,
                tool: tool + shift,
                placements,
            },
            Step::Sweep { input, sweep } => Step::Sweep {
                input: input + shift,
                sweep,
            },
            Step::Dress {
                chamfer,
                input,
                size,
                edges,
            } => Step::Dress {
                chamfer,
                input: input + shift,
                size,
                edges,
            },
        }));
        let tool = steps.len() - 1;
        steps.push(made(shift - 1, tool));
        Symbolic(steps)
    }

    /// The shape's bytes: the header, then a line for each step.
    fn shape(&self) -> ShapeData {
        ShapeData::checked(Blob::inline(self.text().into_bytes()), true)
    }

    fn text(&self) -> String {
        let mut text = format!("{HEADER}\n");
        for step in &self.0 {
            match step {
                Step::Primitive(primitive) => text.push_str(&primitive_line(primitive)),
                Step::Faces(regions) => text.push_str(&faces_line(regions)),
                Step::Translate {
                    input,
                    offset: [x, y, z],
                } => text.push_str(&format!("translate {input} {x} {y} {z}")),
                Step::Boolean { op, base, tool } => {
                    text.push_str(&format!("{} {base} {tool}", op.word()));
                }
                Step::Copies {
                    op,
                    base,
                    tool,
                    placements,
                } => {
                    text.push_str(&format!("copies {} {base} {tool}", op.boolean().word()));
                    for placement in placements {
                        text.push(' ');
                        text.push_str(&placement_word(placement));
                    }
                }
                Step::Sweep { input, sweep } => {
                    text.push_str(&format!("{} {input}", sweep.word()));
                    for value in sweep.values() {
                        text.push_str(&format!(" {value}"));
                    }
                }
                Step::Dress {
                    chamfer,
                    input,
                    size,
                    edges,
                } => {
                    let edges: Vec<String> = edges.iter().map(u32::to_string).collect();
                    text.push_str(&format!(
                        "{} {input} {size} {}",
                        if *chamfer { "chamfer" } else { "fillet" },
                        edges.join(",")
                    ));
                }
            }
            text.push('\n');
        }
        text
    }

    /// The shape `shape`'s bytes say, if they're a fake shape exactly as [`shape`] writes
    /// one.
    ///
    /// [`shape`]: Self::shape
    fn read(shape: &ShapeData) -> Result<Self, Error> {
        let refused = || {
            Error::new(
                ErrorCode::InvalidInput,
                "the fake kernel was given a shape that isn't a fake kernel's",
            )
            .with_suggestion("give the fake kernel only shapes a fake kernel made")
        };
        let bytes = shape.brep().as_bytes();
        let text = std::str::from_utf8(bytes).map_err(|_| refused())?;
        let mut lines = text.strip_suffix('\n').ok_or_else(refused)?.split('\n');
        if lines.next() != Some(HEADER) {
            return Err(refused());
        }
        let mut steps = Vec::new();
        // How many steps each step's shape is made of, to check the order is canonical.
        let mut sizes: Vec<usize> = Vec::new();
        for line in lines {
            let index = steps.len();
            let step = parse_step(line).ok_or_else(refused)?;
            let size = match &step {
                Step::Primitive(_) | Step::Faces(_) => Some(1),
                Step::Translate { input, offset } => (index > 0
                    && *input == index - 1
                    && !nothing(offset)
                    && !matches!(steps.get(*input), Some(Step::Translate { .. })))
                .then(|| sizes.get(*input).map(|s| s + 1))
                .flatten(),
                Step::Boolean { base, tool, .. } | Step::Copies { base, tool, .. } => (index > 0
                    && *tool == index - 1)
                    .then(|| {
                        let tool_size = *sizes.get(*tool)?;
                        let base_size = *sizes.get(*base)?;
                        (index.checked_sub(1 + tool_size) == Some(*base))
                            .then_some(base_size + tool_size + 1)
                    })
                    .flatten(),
                Step::Dress { input, .. } => (index > 0 && *input == index - 1)
                    .then(|| sizes.get(*input).map(|s| s + 1))
                    .flatten(),
                // Faces, and only faces, are swept: the step just before.
                Step::Sweep { input, sweep } => (index > 0
                    && *input == index - 1
                    && matches!(
                        steps.get(*input),
                        Some(Step::Faces(regions)) if sweep.leaves(regions.plane.normal())
                    ))
                .then_some(2),
            };
            sizes.push(size.ok_or_else(refused)?);
            steps.push(step);
        }
        let made = Symbolic(steps);
        // A fillet or chamfer only as a request could have asked: a size the kernel's rules
        // accept, and each edge one of its input's, once.
        let mut counts: Vec<ElementCounts> = Vec::with_capacity(made.0.len());
        for step in &made.0 {
            let of = |i: usize| counts.get(i).copied().unwrap_or_default();
            if let Step::Dress {
                chamfer,
                input,
                size,
                edges,
            } = step
            {
                let sized = if *chamfer {
                    silico_protocol::ChamferSize::Equal { distance: *size }.check()
                } else {
                    silico_protocol::check_fillet_radius(*size)
                };
                let mut seen = BTreeSet::new();
                let edges_ok = !edges.is_empty()
                    && edges
                        .iter()
                        .all(|&e| e > 0 && e <= of(*input).edges && seen.insert(e));
                if sized.is_err() || !edges_ok {
                    return Err(refused());
                }
            }
            counts.push(step_counts(&made.0, step, of));
        }
        // One shape, every step part of it, and every number as `text` writes it.
        if sizes.last() != Some(&made.0.len()) || made.text().as_bytes() != bytes {
            return Err(refused());
        }
        Ok(made)
    }

    fn counts(&self) -> ElementCounts {
        let mut counts: Vec<ElementCounts> = Vec::with_capacity(self.0.len());
        for step in &self.0 {
            let of = |i: usize| counts.get(i).copied().unwrap_or_default();
            counts.push(step_counts(&self.0, step, of));
        }
        counts.last().copied().unwrap_or_default()
    }

    /// How the shape's elements bound each other: a primitive's as OCCT has them, a move's
    /// its input's, and the fake's boolean its inputs' together, the tool's numbered after
    /// the base's, as its history keeps them.
    fn topology(&self) -> Topology {
        // Each step's shape is used by one later step at most (`read` checks the shape is
        // one tree), so it's moved into that step, never copied: a chain of booleans costs
        // its size, not its size times its length.
        let mut made: Vec<(Topology, ElementCounts)> = Vec::with_capacity(self.0.len());
        for step in &self.0 {
            let mut of = |i: usize| made.get_mut(i).map(std::mem::take).unwrap_or_default();
            let next = match step {
                Step::Primitive(primitive) => {
                    (primitive_topology(primitive), primitive_counts(primitive))
                }
                Step::Faces(regions) => {
                    let layout = FacesLayout::of(regions);
                    (layout.topology, layout.counts)
                }
                Step::Translate { input, .. } | Step::Dress { input, .. } => of(*input),
                Step::Sweep { input, sweep } => match self.0.get(*input) {
                    Some(Step::Faces(regions)) => {
                        let layout = FacesLayout::of(regions);
                        let numbers = SweepNumbers::of(&layout, sweep.kind());
                        (numbers.topology(&layout), numbers.counts)
                    }
                    _ => Default::default(),
                },
                Step::Boolean { op, base, tool } => {
                    let ((mut topology, a), (tool, b)) = (of(*base), of(*tool));
                    let counts = boolean_counts(*op, a, b);
                    let several = std::mem::take(&mut topology.solid_faces);
                    let tool = shifted(&tool, a);
                    topology.face_edges.extend(tool.face_edges);
                    topology.outer_edges.extend(tool.outer_edges);
                    topology.edge_vertices.extend(tool.edge_vertices);
                    // One solid's faces all the faces, as if they met; a cut's or a common's
                    // several solids its base's.
                    topology.solid_faces = one_solid(counts, several);
                    (topology, counts)
                }
                Step::Copies {
                    op,
                    base,
                    tool,
                    placements,
                } => {
                    let ((mut topology, a), (tool, b)) = (of(*base), of(*tool));
                    let counts = copies_counts(*op, a, b, placements.len());
                    let several = std::mem::take(&mut topology.solid_faces);
                    let mut before = a;
                    for _ in placements {
                        let copy = shifted(&tool, before);
                        topology.face_edges.extend(copy.face_edges);
                        topology.outer_edges.extend(copy.outer_edges);
                        topology.edge_vertices.extend(copy.edge_vertices);
                        before = sum(before, b);
                    }
                    topology.solid_faces = one_solid(counts, several);
                    (topology, counts)
                }
            };
            made.push(next);
        }
        made.pop().map(|(topology, _)| topology).unwrap_or_default()
    }

    /// The shape's summary: each step's measures in turn, by formula for a primitive.
    fn summary(&self) -> ShapeSummary {
        let mut measured: Vec<Measured> = Vec::with_capacity(self.0.len());
        for step in &self.0 {
            let of = |i: usize| measured.get(i).copied().unwrap_or_default();
            let made = match step {
                Step::Primitive(primitive) => Measured::primitive(primitive),
                Step::Faces(regions) => Measured::faces(regions),
                Step::Translate { input, offset } => of(*input).moved(*offset),
                Step::Boolean { op, base, tool } => {
                    let swept = matches!(self.0.get(*tool), Some(Step::Sweep { .. }));
                    match op {
                        BooleanOp::Cut if swept => of(*base).less(of(*tool)),
                        _ => of(*base).with(of(*tool)),
                    }
                }
                // Each copy as the boolean of it with what came before would be.
                Step::Copies {
                    op,
                    base,
                    tool,
                    placements,
                } => {
                    let swept = matches!(self.0.get(*tool), Some(Step::Sweep { .. }));
                    placements.iter().fold(of(*base), |made, placement| {
                        let copy = of(*tool).placed(placement);
                        match op {
                            CopiesOp::Cut if swept => made.less(copy),
                            _ => made.with(copy),
                        }
                    })
                }
                Step::Dress { input, .. } => of(*input),
                Step::Sweep { input, sweep } => match self.0.get(*input) {
                    Some(Step::Faces(regions)) => Measured::swept(regions, sweep),
                    _ => Measured::default(),
                },
            };
            measured.push(made);
        }
        let m = measured.last().copied().unwrap_or_default();
        let counts = self.counts();
        ShapeSummary {
            volume: m.volume,
            area: m.area,
            center: if m.volume == 0.0 {
                [0.0; 3]
            } else {
                m.moment.map(|c| c / m.volume)
            },
            faces: counts.faces,
            edges: counts.edges,
            vertices: counts.vertices,
            bounds: Some(m.bounds),
            valid: true,
        }
    }

    /// The shape as an expression. Written from the last step down, with a stack, so a long
    /// chain of booleans can't overflow the thread's.
    fn describe(&self) -> String {
        enum Part {
            Step(usize),
            Text(String),
        }
        let mut out = String::new();
        let mut parts = vec![Part::Step(self.0.len().saturating_sub(1))];
        while let Some(part) = parts.pop() {
            match part {
                Part::Text(text) => out.push_str(&text),
                Part::Step(index) => match self.0.get(index) {
                    Some(Step::Primitive(primitive)) => out.push_str(&primitive_text(primitive)),
                    Some(Step::Faces(regions)) => out.push_str(&faces_text(regions)),
                    Some(Step::Translate {
                        input,
                        offset: [x, y, z],
                    }) => {
                        out.push_str("translate(");
                        parts.push(Part::Text(format!(", {x}, {y}, {z})")));
                        parts.push(Part::Step(*input));
                    }
                    Some(Step::Boolean { op, base, tool }) => {
                        out.push_str(op.word());
                        out.push('(');
                        parts.push(Part::Text(")".to_owned()));
                        parts.push(Part::Step(*tool));
                        parts.push(Part::Text(", ".to_owned()));
                        parts.push(Part::Step(*base));
                    }
                    Some(Step::Copies {
                        op,
                        base,
                        tool,
                        placements,
                    }) => {
                        out.push_str(&format!("copies_{}(", op.boolean().word()));
                        let placed: Vec<String> = placements.iter().map(placement_word).collect();
                        parts.push(Part::Text(format!(", [{}])", placed.join(", "))));
                        parts.push(Part::Step(*tool));
                        parts.push(Part::Text(", ".to_owned()));
                        parts.push(Part::Step(*base));
                    }
                    Some(Step::Sweep { input, sweep }) => {
                        out.push_str(sweep.word());
                        out.push('(');
                        let values: Vec<String> =
                            sweep.values().iter().map(f64::to_string).collect();
                        parts.push(Part::Text(format!(", {})", values.join(", "))));
                        parts.push(Part::Step(*input));
                    }
                    Some(Step::Dress {
                        chamfer,
                        input,
                        size,
                        edges,
                    }) => {
                        out.push_str(if *chamfer { "chamfer(" } else { "fillet(" });
                        let edges: Vec<String> = edges.iter().map(u32::to_string).collect();
                        parts.push(Part::Text(format!(", {size}, [{}])", edges.join(", "))));
                        parts.push(Part::Step(*input));
                    }
                    None => {}
                },
            }
        }
        out
    }
}

/// The counts of `step`'s shape, from those of the steps before it (`of`), in `steps`.
fn step_counts(steps: &[Step], step: &Step, of: impl Fn(usize) -> ElementCounts) -> ElementCounts {
    match step {
        Step::Primitive(primitive) => primitive_counts(primitive),
        Step::Faces(regions) => FacesLayout::of(regions).counts,
        Step::Translate { input, .. } | Step::Dress { input, .. } => of(*input),
        Step::Boolean { op, base, tool } => boolean_counts(*op, of(*base), of(*tool)),
        Step::Copies {
            op,
            base,
            tool,
            placements,
        } => copies_counts(*op, of(*base), of(*tool), placements.len()),
        Step::Sweep { input, sweep } => match steps.get(*input) {
            Some(Step::Faces(regions)) => {
                SweepNumbers::of(&FacesLayout::of(regions), sweep.kind()).counts
            }
            _ => ElementCounts::default(),
        },
    }
}

/// What the fake measures of a shape: its volume, area, first moment of volume (its center
/// times its volume), and box.
#[derive(Debug, Clone, Copy, Default)]
struct Measured {
    volume: f64,
    area: f64,
    moment: [f64; 3],
    bounds: [[f64; 3]; 2],
}

impl Measured {
    /// A primitive's measures, where the kernel makes it: a box from the origin, a cylinder
    /// and cone up z from a base centred on it, a sphere and torus centred on it.
    fn primitive(primitive: &Primitive) -> Self {
        use std::f64::consts::PI;
        let (volume, area, center, low, high) = match *primitive {
            Primitive::Box { dx, dy, dz } => (
                dx * dy * dz,
                2.0 * (dx * dy + dy * dz + dz * dx),
                [dx / 2.0, dy / 2.0, dz / 2.0],
                [0.0; 3],
                [dx, dy, dz],
            ),
            Primitive::Cylinder {
                radius: r,
                height: h,
            } => (
                PI * r * r * h,
                2.0 * PI * r * h + 2.0 * PI * r * r,
                [0.0, 0.0, h / 2.0],
                [-r, -r, 0.0],
                [r, r, h],
            ),
            Primitive::Sphere { radius: r } => (
                4.0 / 3.0 * PI * r * r * r,
                4.0 * PI * r * r,
                [0.0; 3],
                [-r; 3],
                [r; 3],
            ),
            Primitive::Cone {
                radius1: a,
                radius2: b,
                height: h,
            } => {
                let sum = a * a + a * b + b * b;
                let slant = ((a - b) * (a - b) + h * h).sqrt();
                let m = a.max(b);
                (
                    PI * h * sum / 3.0,
                    PI * (a + b) * slant + PI * a * a + PI * b * b,
                    [
                        0.0,
                        0.0,
                        h * (a * a + 2.0 * a * b + 3.0 * b * b) / (4.0 * sum),
                    ],
                    [-m, -m, 0.0],
                    [m, m, h],
                )
            }
            Primitive::Torus {
                major_radius: big,
                minor_radius: r,
            } => (
                2.0 * PI * PI * big * r * r,
                4.0 * PI * PI * big * r,
                [0.0; 3],
                [-(big + r), -(big + r), -r],
                [big + r, big + r, r],
            ),
        };
        Measured {
            volume,
            area,
            moment: center.map(|c| c * volume),
            bounds: [low, high],
        }
    }

    /// Faces' measures: no volume, their area by formula ([`loop_area`]: each outer loop's,
    /// less its holes'), and the box around their curves, on their plane.
    fn faces(regions: &FaceRegions) -> Self {
        let mut area = 0.0;
        for region in &regions.regions {
            for (contour, curves) in region.loops().enumerate() {
                area += loop_area(curves, contour == 0);
            }
        }
        // On an origin plane, whose axes point along model space's, the box of the curves in
        // the sketch's coordinates, its corners placed; on a face frame, in model space.
        let bounds = match regions.plane {
            SketchPlane::Face(frame) => frame_box(frame, regions),
            plane @ (SketchPlane::Xy | SketchPlane::Xz | SketchPlane::Yz) => {
                let [low, high] = sketch_box(regions);
                [plane.place(low), plane.place(high)]
            }
        };
        Measured {
            volume: 0.0,
            area,
            moment: [0.0; 3],
            bounds,
        }
    }

    /// A sweep's measures, the fake's own model, by formula from its faces' area A,
    /// perimeter P, and box. A prism: A times its height along the plane's normal, 2A plus P
    /// times its length, the box of the faces at its start and its end, and the centre of
    /// that box (exact for a rectangle swept square). A revolution, by Pappus's theorems with
    /// the faces' box centre at distance r̄ from the axis: A·r̄·θ, P·r̄·θ (and 2A, short of a
    /// whole turn), the centre where the axis's point is, and a cube about it reaching as far
    /// as the faces' box's farthest corner.
    fn swept(regions: &FaceRegions, sweep: &Sweep) -> Self {
        let faces = Measured::faces(regions);
        let mut perimeter = 0.0;
        for region in &regions.regions {
            for curves in region.loops() {
                perimeter += area_and_perimeter(curves).1;
            }
        }
        let [low, high] = faces.bounds;
        let middle = [0, 1, 2].map(|k| (low[k] + high[k]) / 2.0);
        let size = |a: [f64; 3]| dot(a, a).sqrt();
        let (volume, area, center, bounds) = match *sweep {
            Sweep::Prism { start, vector } => {
                let height = dot(vector, regions.plane.normal()).abs();
                let at = |p: [f64; 3], by: [f64; 3]| [0, 1, 2].map(|k| p[k] + by[k]);
                let end = at(start, vector);
                let lowest = [0, 1, 2].map(|k| (low[k] + start[k]).min(low[k] + end[k]));
                let highest = [0, 1, 2].map(|k| (high[k] + start[k]).max(high[k] + end[k]));
                (
                    faces.area * height,
                    2.0 * faces.area + perimeter * size(vector),
                    [0, 1, 2].map(|k| (lowest[k] + highest[k]) / 2.0),
                    [lowest, highest],
                )
            }
            Sweep::Revolution { axis, angle, .. } => {
                let from = |p: [f64; 3]| {
                    let w = [0, 1, 2].map(|k| p[k] - axis.origin[k]);
                    let along = dot(w, axis.direction);
                    size([0, 1, 2].map(|k| w[k] - along * axis.direction[k]))
                };
                let radius = from(middle);
                let reach = (0..8)
                    .map(|c: usize| {
                        let corner =
                            [0, 1, 2].map(|k| if c >> k & 1 == 1 { high[k] } else { low[k] });
                        size([0, 1, 2].map(|k| corner[k] - axis.origin[k]))
                    })
                    .fold(0.0, f64::max);
                let caps = if silico_protocol::is_whole_turn(angle) {
                    0.0
                } else {
                    2.0 * faces.area
                };
                (
                    faces.area * radius * angle,
                    perimeter * radius * angle + caps,
                    axis.origin,
                    [
                        axis.origin.map(|c| c - reach),
                        axis.origin.map(|c| c + reach),
                    ],
                )
            }
        };
        Measured {
            volume,
            area,
            moment: center.map(|c| c * volume),
            bounds,
        }
    }

    fn moved(self, offset: [f64; 3]) -> Self {
        let [low, high] = self.bounds;
        Measured {
            moment: [0, 1, 2].map(|i| self.moment[i] + self.volume * offset[i]),
            bounds: [
                [0, 1, 2].map(|i| low[i] + offset[i]),
                [0, 1, 2].map(|i| high[i] + offset[i]),
            ],
            ..self
        }
    }

    /// A shape's measures less what of `tool` lies in its box, as if the tool were spread
    /// evenly through its own box and the shape filled its: its volume and moment less the
    /// tool's in proportion to the share of the tool's box inside the shape's, its area
    /// more by that share of the tool's, its box its own. Exact for a prism of a rectangle
    /// cut square into a box (a pocket); the fake's own model otherwise.
    fn less(self, tool: Measured) -> Self {
        let ([a_low, a_high], [b_low, b_high]) = (self.bounds, tool.bounds);
        let size = |low: [f64; 3], high: [f64; 3]| -> f64 {
            (0..3).map(|k| (high[k] - low[k]).max(0.0)).product()
        };
        let inside = size(
            [0, 1, 2].map(|k| a_low[k].max(b_low[k])),
            [0, 1, 2].map(|k| a_high[k].min(b_high[k])),
        );
        let whole = size(b_low, b_high);
        let share = if whole > 0.0 { inside / whole } else { 0.0 };
        Measured {
            volume: self.volume - share * tool.volume,
            area: self.area + share * tool.area,
            moment: [0, 1, 2].map(|i| self.moment[i] - share * tool.moment[i]),
            bounds: self.bounds,
        }
    }

    /// Two shapes' measures together, as if they didn't touch.
    fn with(self, other: Measured) -> Self {
        let ([a_low, a_high], [b_low, b_high]) = (self.bounds, other.bounds);
        Measured {
            volume: self.volume + other.volume,
            area: self.area + other.area,
            moment: [0, 1, 2].map(|i| self.moment[i] + other.moment[i]),
            bounds: [
                [0, 1, 2].map(|i| a_low[i].min(b_low[i])),
                [0, 1, 2].map(|i| a_high[i].max(b_high[i])),
            ],
        }
    }
}

/// A placement as an affine map, `p ↦ a·(p − origin) + origin + offset`: its matrix (rows),
/// the point it turns or reflects about, and its move.
fn affine(placement: &Placement) -> ([[f64; 3]; 3], [f64; 3], [f64; 3]) {
    const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    // Rodrigues's rotation about the unit `k`, right-handed.
    let turn = |k: [f64; 3], angle: f64| {
        let (sin, cos) = (libm::sin(angle), libm::cos(angle));
        let cross = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
        [0, 1, 2].map(|i| {
            [0, 1, 2].map(|j| cos * IDENTITY[i][j] + sin * cross[i][j] + (1.0 - cos) * k[i] * k[j])
        })
    };
    match *placement {
        Placement::Translation { offset } => (IDENTITY, [0.0; 3], offset),
        Placement::Rotation { axis, angle } => (turn(axis.direction, angle), axis.origin, [0.0; 3]),
        Placement::Mirror { origin, normal: n } => (
            [0, 1, 2].map(|i| [0, 1, 2].map(|j| IDENTITY[i][j] - 2.0 * n[i] * n[j])),
            origin,
            [0.0; 3],
        ),
        Placement::TurnedAndMoved {
            axis,
            angle,
            offset,
        } => (turn(axis.direction, angle), axis.origin, offset),
    }
}

impl Measured {
    /// A shape's measures with the shape placed as `placement` says: its volume and area
    /// its own, its center placed, and its box the box around its own box's corners placed.
    fn placed(self, placement: &Placement) -> Self {
        let (a, origin, offset) = affine(placement);
        let place = |p: [f64; 3]| {
            let d = sub(p, origin);
            [0, 1, 2].map(|i| dot(a[i], d) + origin[i] + offset[i])
        };
        let [low, high] = self.bounds;
        let corners = (0..8)
            .map(|c| place([0, 1, 2].map(|k| if c >> k & 1 == 0 { low[k] } else { high[k] })));
        let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        for corner in corners {
            for k in 0..3 {
                bounds[0][k] = bounds[0][k].min(corner[k]);
                bounds[1][k] = bounds[1][k].max(corner[k]);
            }
        }
        let moment = if self.volume == 0.0 {
            self.moment
        } else {
            place(self.moment.map(|m| m / self.volume)).map(|c| c * self.volume)
        };
        Measured {
            moment,
            bounds,
            ..self
        }
    }
}

/// A placement as one word of a shape's bytes, its values as Rust writes them, which read
/// back as the same numbers: `t:20,0,0` (a translation by its offset), `r:` (a rotation: the
/// axis's point, its direction, the angle), `m:` (a mirror: the plane's point, its normal),
/// `tm:` (a rotation's values, then the move's).
fn placement_word(placement: &Placement) -> String {
    let (tag, values): (&str, Vec<f64>) = match *placement {
        Placement::Translation { offset } => ("t", offset.to_vec()),
        Placement::Rotation { axis, angle } => (
            "r",
            axis.origin
                .into_iter()
                .chain(axis.direction)
                .chain([angle])
                .collect(),
        ),
        Placement::Mirror { origin, normal } => ("m", origin.into_iter().chain(normal).collect()),
        Placement::TurnedAndMoved {
            axis,
            angle,
            offset,
        } => (
            "tm",
            axis.origin
                .into_iter()
                .chain(axis.direction)
                .chain([angle])
                .chain(offset)
                .collect(),
        ),
    };
    let values: Vec<String> = values.iter().map(f64::to_string).collect();
    format!("{tag}:{}", values.join(","))
}

/// The placement a word of a shape's bytes says, if it's one ([`placement_word`]).
fn parse_placement(word: &str) -> Option<Placement> {
    let (tag, values) = word.split_once(':')?;
    let v: Vec<f64> = values
        .split(',')
        .map(|w| w.parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect::<Option<_>>()?;
    let axis = |v: &[f64]| Axis {
        origin: [v[0], v[1], v[2]],
        direction: [v[3], v[4], v[5]],
    };
    Some(match (tag, v.as_slice()) {
        ("t", &[x, y, z]) => Placement::Translation { offset: [x, y, z] },
        ("r", &[.., angle]) if v.len() == 7 => Placement::Rotation {
            axis: axis(&v),
            angle,
        },
        ("m", &[ox, oy, oz, nx, ny, nz]) => Placement::Mirror {
            origin: [ox, oy, oz],
            normal: [nx, ny, nz],
        },
        ("tm", &[.., angle, x, y, z]) if v.len() == 10 => Placement::TurnedAndMoved {
            axis: axis(&v),
            angle,
            offset: [x, y, z],
        },
        _ => return None,
    })
}

/// `value`, with a zero of either sign as 0: the kernel makes a primitive the same whatever
/// the sign of a zero in its values.
fn zeroed(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

/// A primitive's name and values, its zeros unsigned.
fn primitive_parts(primitive: &Primitive) -> (&'static str, Vec<f64>) {
    let values = primitive.values().into_iter().map(zeroed).collect();
    (primitive.name(), values)
}

/// A primitive as a line of a shape's bytes: `box 40 20 10`. Numbers are written as Rust
/// writes them, which reads back as the same number.
fn primitive_line(primitive: &Primitive) -> String {
    let (name, values) = primitive_parts(primitive);
    let mut line = name.to_owned();
    for value in values {
        line.push_str(&format!(" {value}"));
    }
    line
}

/// A primitive in an expression: `box(40, 20, 10)`.
fn primitive_text(primitive: &Primitive) -> String {
    let (name, values) = primitive_parts(primitive);
    let values: Vec<String> = values.iter().map(f64::to_string).collect();
    format!("{name}({})", values.join(", "))
}

/// The boolean `name` names.
fn parse_boolean(name: &str) -> Option<BooleanOp> {
    BooleanOp::ALL.into_iter().find(|op| op.word() == name)
}

/// The step a line of a shape's bytes says, if it's one. Whether it's written canonically
/// is [`Symbolic::read`]'s to check.
fn parse_step(line: &str) -> Option<Step> {
    let mut words = line.split(' ');
    let name = words.next()?;
    let words: Vec<&str> = words.collect();
    let number = |w: &str| w.parse::<f64>().ok().filter(|v| v.is_finite());
    let numbers = || -> Option<Vec<f64>> { words.iter().map(|w| number(w)).collect() };
    let index = |w: &str| w.parse::<usize>().ok();
    if name == "faces" {
        return parse_faces(&words).map(|regions| Step::Faces(Box::new(regions)));
    }
    if let ("copies", [op, base, tool, placements @ ..]) = (name, words.as_slice()) {
        let op = match parse_boolean(op)? {
            BooleanOp::Fuse => CopiesOp::Fuse,
            BooleanOp::Cut => CopiesOp::Cut,
            BooleanOp::Common => return None,
        };
        let placements: Vec<Placement> = placements
            .iter()
            .map(|w| parse_placement(w))
            .collect::<Option<_>>()?;
        // Only what a request could have asked.
        silico_protocol::check_copies(&placements).ok()?;
        return Some(Step::Copies {
            op,
            base: index(base)?,
            tool: index(tool)?,
            placements,
        });
    }
    if let (Some(op), [base, tool]) = (parse_boolean(name), words.as_slice()) {
        return Some(Step::Boolean {
            op,
            base: index(base)?,
            tool: index(tool)?,
        });
    }
    Some(match (name, words.as_slice()) {
        ("translate", [input, x, y, z]) => Step::Translate {
            input: index(input)?,
            offset: [number(x)?, number(y)?, number(z)?],
        },
        ("prism" | "revolution", [input, values @ ..]) => Step::Sweep {
            input: index(input)?,
            sweep: Sweep::parse(
                name,
                &values
                    .iter()
                    .map(|w| number(w))
                    .collect::<Option<Vec<_>>>()?,
            )?,
        },
        ("fillet" | "chamfer", [input, size, edges]) => Step::Dress {
            chamfer: name == "chamfer",
            input: index(input)?,
            size: number(size)?,
            // Which are the input's, [`Symbolic::read`] checks.
            edges: edges
                .split(',')
                .map(|e| e.parse::<u32>().ok())
                .collect::<Option<_>>()?,
        },
        _ => Step::Primitive(parse_primitive(name, &numbers()?)?),
    })
}

/// The primitive a line names with these values, if the protocol's check accepts it: the
/// fake reads only what a request could have made.
fn parse_primitive(name: &str, values: &[f64]) -> Option<Primitive> {
    let primitive = match (name, values) {
        ("box", &[dx, dy, dz]) => Primitive::Box { dx, dy, dz },
        ("cylinder", &[radius, height]) => Primitive::Cylinder { radius, height },
        ("sphere", &[radius]) => Primitive::Sphere { radius },
        ("cone", &[radius1, radius2, height]) => Primitive::Cone {
            radius1,
            radius2,
            height,
        },
        ("torus", &[major_radius, minor_radius]) => Primitive::Torus {
            major_radius,
            minor_radius,
        },
        _ => return None,
    };
    primitive.check().is_ok().then_some(primitive)
}

/// How faces made from regions number their elements, as the kernel makes them: a face
/// for each region in order, an edge for each curve in order, and a vertex for each point,
/// numbered as OCCT first meets them, loop by loop (a loop's points in order, or a circle's
/// one vertex).
struct FacesLayout {
    counts: ElementCounts,
    topology: Topology,
    /// The vertex of each point, in the order sent.
    points: Vec<u32>,
}

impl FacesLayout {
    fn of(regions: &FaceRegions) -> Self {
        let mut layout = FacesLayout {
            counts: ElementCounts::default(),
            topology: Topology::default(),
            points: Vec::new(),
        };
        let (mut edge, mut vertex) = (0u32, 0u32);
        for region in &regions.regions {
            let mut face_edges = Vec::new();
            for (contour, curves) in region.loops().enumerate() {
                let first_edge = edge + 1;
                let first_vertex = vertex + 1;
                let is_circle = matches!(curves, [Curve::Circle { .. }]);
                let n = u32::try_from(curves.len()).unwrap_or(u32::MAX);
                for i in 0..n {
                    edge += 1;
                    face_edges.push(edge);
                    layout.topology.edge_vertices.push(if is_circle {
                        vec![first_vertex]
                    } else {
                        vec![first_vertex + i, first_vertex + (i + 1) % n]
                    });
                }
                if is_circle {
                    vertex += 1;
                } else {
                    layout.points.extend(first_vertex..first_vertex + n);
                    vertex += n;
                }
                if contour == 0 {
                    layout
                        .topology
                        .outer_edges
                        .push((first_edge..=edge).collect());
                }
            }
            layout.topology.face_edges.push(face_edges);
        }
        layout.counts = ElementCounts {
            faces: u32::try_from(regions.regions.len()).unwrap_or(u32::MAX),
            edges: edge,
            vertices: vertex,
            solids: 0,
        };
        layout
    }
}

/// The box around faces' curves in the sketch's own coordinates: each line's and arc's
/// ends, each circle's square, and each quarter turn an arc passes, where it reaches
/// furthest along an axis. On an origin plane, whose axes point along model space's, its
/// corners placed are the faces' box.
fn sketch_box(regions: &FaceRegions) -> [[f64; 2]; 2] {
    let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut include = |p: [f64; 2]| {
        for k in 0..2 {
            low[k] = low[k].min(p[k]);
            high[k] = high[k].max(p[k]);
        }
    };
    for curve in regions.curves() {
        match *curve {
            Curve::Line { start, end } => {
                include(start);
                include(end);
            }
            Curve::Circle { center, radius } => {
                include([center[0] - radius, center[1] - radius]);
                include([center[0] + radius, center[1] + radius]);
            }
            Curve::Arc {
                center,
                start,
                end,
                radius,
                start_angle,
                end_angle,
            } => {
                include(start);
                include(end);
                // Each quarter turn the arc passes: where it reaches furthest along an
                // axis.
                let quarter = std::f64::consts::FRAC_PI_2;
                let (from, to) = (start_angle.min(end_angle), start_angle.max(end_angle));
                let mut k = libm::ceil(from / quarter);
                while k * quarter <= to {
                    let [x, y] = match k.rem_euclid(4.0) as u8 {
                        0 => [1.0, 0.0],
                        1 => [0.0, 1.0],
                        2 => [-1.0, 0.0],
                        _ => [0.0, -1.0],
                    };
                    include([center[0] + radius * x, center[1] + radius * y]);
                    k += 1.0;
                }
            }
        }
    }
    [low, high]
}

/// The box in model space around faces' curves on a face frame, whose axes may point any
/// way: each line's and arc's ends placed, and each circle's or arc's furthest reach along
/// each axis k. About its centre a placed, a round is a + r(x cos t + y sin t), whose k-th
/// coordinate reaches a_k ± r·√(x_k² + y_k²) at t = φ_k and φ_k + π, φ_k the angle of
/// (x_k, y_k): a circle reaches both, an arc those it passes.
fn frame_box(frame: FaceFrame, regions: &FaceRegions) -> [[f64; 3]; 2] {
    let plane = SketchPlane::Face(frame);
    let (mut low, mut high) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    let mut include = |k: usize, v: f64| {
        low[k] = low[k].min(v);
        high[k] = high[k].max(v);
    };
    // Whether `angle`, a turn on or back any number of times, is from `from` to `to`.
    let passes = |angle: f64, from: f64, to: f64| {
        let turns = libm::ceil((from - angle) / std::f64::consts::TAU);
        angle + turns * std::f64::consts::TAU <= to
    };
    for curve in regions.curves() {
        let (center, radius, sweep) = match *curve {
            Curve::Line { start, end } => {
                for p in [start, end] {
                    let p = plane.place(p);
                    (0..3).for_each(|k| include(k, p[k]));
                }
                continue;
            }
            Curve::Circle { center, radius } => (center, radius, None),
            Curve::Arc {
                center,
                start,
                end,
                radius,
                start_angle,
                end_angle,
            } => {
                for p in [start, end] {
                    let p = plane.place(p);
                    (0..3).for_each(|k| include(k, p[k]));
                }
                (
                    center,
                    radius,
                    Some((start_angle.min(end_angle), start_angle.max(end_angle))),
                )
            }
        };
        let a = plane.place(center);
        for (k, a_k) in a.into_iter().enumerate() {
            let reach = radius * libm::hypot(frame.x[k], frame.y[k]);
            let phase = libm::atan2(frame.y[k], frame.x[k]);
            for (angle, sign) in [(phase, 1.0), (phase + std::f64::consts::PI, -1.0)] {
                if sweep.is_none_or(|(from, to)| passes(angle, from, to)) {
                    include(k, a_k + sign * reach);
                }
            }
        }
    }
    [low, high]
}

/// A plane in a shape's bytes: its word ([`SketchPlane::word`]), and a face frame's nine
/// numbers after it, its origin, x, and y, each as Rust writes it (a zero of either sign as
/// 0, as a curve's numbers are).
fn plane_line(plane: SketchPlane) -> String {
    let mut line = plane.word().to_owned();
    if let SketchPlane::Face(frame) = plane {
        for value in [frame.origin, frame.x, frame.y].into_iter().flatten() {
            line.push_str(&format!(" {}", value + 0.0));
        }
    }
    line
}

/// Faces as a line of a shape's bytes: `faces xy region loop line 0 0 4 0 ... loop circle 2
/// 1.5 0.5`, every number as Rust writes it, which reads back as the same number (a zero of
/// either sign written as 0).
fn faces_line(regions: &FaceRegions) -> String {
    let mut line = format!("faces {}", plane_line(regions.plane));
    for region in &regions.regions {
        line.push_str(" region");
        for curves in region.loops() {
            line.push_str(" loop");
            for curve in curves {
                let (word, values): (&str, Vec<f64>) = match *curve {
                    Curve::Line { start, end } => {
                        ("line", vec![start[0], start[1], end[0], end[1]])
                    }
                    Curve::Arc {
                        center,
                        start,
                        end,
                        radius,
                        start_angle,
                        end_angle,
                    } => (
                        "arc",
                        vec![
                            center[0],
                            center[1],
                            start[0],
                            start[1],
                            end[0],
                            end[1],
                            radius,
                            start_angle,
                            end_angle,
                        ],
                    ),
                    Curve::Circle { center, radius } => {
                        ("circle", vec![center[0], center[1], radius])
                    }
                };
                line.push(' ');
                line.push_str(word);
                // A zero of either sign as 0, as the kernel places points: one shape's
                // bytes whatever the sign of its zeros.
                for value in values {
                    line.push_str(&format!(" {}", value + 0.0));
                }
            }
        }
    }
    line
}

/// Faces in an expression: `faces(xy, 2 regions, 9 curves)`.
fn faces_text(regions: &FaceRegions) -> String {
    format!(
        "faces({}, {} regions, {} curves)",
        regions.plane.word(),
        regions.regions.len(),
        regions.curves().count()
    )
}

/// The regions a `faces` line's words (after `faces`) say, if the protocol's check accepts
/// them: the fake reads only what a request could have made.
fn parse_faces(words: &[&str]) -> Option<FaceRegions> {
    let number = |w: &str| w.parse::<f64>().ok().filter(|v| v.is_finite());
    let (plane, rest) = match words.split_first()? {
        (&"frame", rest) => {
            let (values, rest) = rest.split_first_chunk::<9>()?;
            let mut values = values.iter().map(|w| number(w));
            let mut next = || -> Option<[f64; 3]> {
                Some([values.next()??, values.next()??, values.next()??])
            };
            let frame = FaceFrame {
                origin: next()?,
                x: next()?,
                y: next()?,
            };
            (SketchPlane::Face(frame), rest)
        }
        (plane, rest) => (
            SketchPlane::ORIGIN_PLANES
                .into_iter()
                .find(|p| p.word() == *plane)?,
            rest,
        ),
    };
    let point = |x: &str, y: &str| Some([number(x)?, number(y)?]);
    let mut regions = Vec::new();
    // Each region's words, after the "region" that starts it.
    let mut parts = rest.split(|w| *w == "region");
    if !parts.next()?.is_empty() {
        return None;
    }
    for region in parts {
        let mut loops = region.split(|w| *w == "loop");
        if !loops.next()?.is_empty() {
            return None;
        }
        let mut contours: Vec<Vec<Curve>> = Vec::new();
        for words in loops {
            let mut curves = Vec::new();
            let mut rest = words;
            while let Some((&word, after)) = rest.split_first() {
                let (curve, used) = match (word, after) {
                    ("line", [sx, sy, ex, ey, ..]) => (
                        Curve::Line {
                            start: point(sx, sy)?,
                            end: point(ex, ey)?,
                        },
                        4,
                    ),
                    ("arc", [cx, cy, sx, sy, ex, ey, r, a0, a1, ..]) => (
                        Curve::Arc {
                            center: point(cx, cy)?,
                            start: point(sx, sy)?,
                            end: point(ex, ey)?,
                            radius: number(r)?,
                            start_angle: number(a0)?,
                            end_angle: number(a1)?,
                        },
                        9,
                    ),
                    ("circle", [cx, cy, r, ..]) => (
                        Curve::Circle {
                            center: point(cx, cy)?,
                            radius: number(r)?,
                        },
                        3,
                    ),
                    _ => return None,
                };
                curves.push(curve);
                rest = after.get(used..)?;
            }
            contours.push(curves);
        }
        let mut contours = contours.into_iter();
        regions.push(Region {
            outer: contours.next()?,
            holes: contours.collect(),
        });
    }
    let regions = FaceRegions { plane, regions };
    regions.check().is_ok().then_some(regions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_the_protocol_refuses_are_refused_as_the_kernel_refuses_them() {
        // Sent by hand, past the request's own check: the fake checks again, as the worker
        // does, naming the placement.
        let b = Symbolic::primitive(Primitive::Box {
            dx: 2.0,
            dy: 3.0,
            dz: 4.0,
        })
        .shape();
        let op = Op::Copies {
            base: b.clone(),
            tool: b,
            op: CopiesOp::Fuse,
            placements: vec![
                Placement::Translation {
                    offset: [5.0, 0.0, 0.0],
                },
                Placement::Translation {
                    offset: [0.0, 1e-9, 0.0],
                },
            ],
        };
        let e = answer(&op).unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidInput);
        assert_eq!(e.parameter(), Some("placements.1.offset"));
    }

    #[test]
    fn a_copies_shape_reads_only_as_written_and_as_a_request_could_have_made_it() {
        let text = |placements: &str| {
            ShapeData::checked(
                Blob::inline(
                    format!("{HEADER}\nbox 2 3 4\nbox 1 1 1\ncopies fuse 0 1 {placements}\n")
                        .into_bytes(),
                ),
                true,
            )
        };
        let read = |placements: &str| Symbolic::read(&text(placements)).is_ok();
        assert!(read(
            "t:5,0,0 r:0,0,0,0,0,1,1 m:1,0,0,1,0,0 tm:0,0,0,0,0,1,-1,0,0,2"
        ));
        for refused in [
            "",
            "t:0,0,0",
            "t:5,0",
            "t:5.0,0,0",
            "r:0,0,0,0,0,2,1",
            "r:0,0,0,0,0,1,0",
            "m:0,0,0,0,0,0",
            "tm:0,0,0,0,0,1,1,0,0,0",
            "q:1,2,3",
            "t:NaN,0,0",
        ] {
            assert!(!read(refused), "{refused:?} was read");
        }
        // A common of copies is no copies.
        let common = ShapeData::checked(
            Blob::inline(
                format!("{HEADER}\nbox 2 3 4\nbox 1 1 1\ncopies common 0 1 t:5,0,0\n").into_bytes(),
            ),
            true,
        );
        assert!(Symbolic::read(&common).is_err());
    }

    #[test]
    fn a_deep_chain_of_booleans_is_read_and_described_without_overflowing_the_stack() {
        // Built directly: through the kernel, each boolean's history lists its base's
        // elements, so a chain this deep would take quadratic time to make.
        let unit = Symbolic::primitive(Primitive::Sphere { radius: 1.0 });
        let depth = 100_000;
        let mut steps = unit.0.clone();
        for _ in 0..depth {
            let base = steps.len() - 1;
            steps.push(Step::Primitive(Primitive::Sphere { radius: 1.0 }));
            steps.push(Step::Boolean {
                op: BooleanOp::Fuse,
                base,
                tool: base + 1,
            });
        }
        let made = Symbolic(steps);
        let read = Symbolic::read(&made.shape()).unwrap();
        assert_eq!(read, made);
        let text = read.describe();
        assert!(text.starts_with("fuse(fuse(fuse("));
        assert_eq!(text.matches("sphere(1)").count(), depth + 1);
    }
}

// --- A thread that never waits ----------------------------------------------------------

/// A thread marked as one that must never wait on a command (the app's UI thread, M5.0's
/// section 3, *How this is tested*): a [`Running::wait`](crate::command::Running::wait) or
/// [`wait_timeout`](crate::command::Running::wait_timeout) called on it is counted, however
/// short, so a test fails on the rule itself, not on a frame counter that a short wait
/// wouldn't stop.
pub mod never_waits {
    use std::cell::Cell;

    thread_local! {
        static MARKED: Cell<bool> = const { Cell::new(false) };
        /// Waits called on this thread while it was marked: each thread its own count, so
        /// tests in one process never see one another's.
        static WAITED: Cell<usize> = const { Cell::new(0) };
    }

    /// Marks the calling thread until the guard is dropped: a wait called on it meanwhile
    /// is counted.
    pub fn marked() -> Marked {
        MARKED.with(|marked| marked.set(true));
        Marked(std::marker::PhantomData)
    }

    /// The calling thread marked, until it's dropped ([`marked`]).
    #[must_use = "the thread is marked only while this is held"]
    pub struct Marked(std::marker::PhantomData<*const ()>);

    impl Drop for Marked {
        fn drop(&mut self) {
            MARKED.with(|marked| marked.set(false));
        }
    }

    /// How many waits the calling thread called while it was marked.
    pub fn waits() -> usize {
        WAITED.with(Cell::get)
    }

    /// A wait was called: counted, if on a marked thread.
    pub(crate) fn waited() {
        if MARKED.with(Cell::get) {
            WAITED.with(|waited| waited.set(waited.get() + 1));
        }
    }
}

// --- Renders ----------------------------------------------------------------------------

/// A renderer for the core's tests (docs/design/m4.6-render.md, Tests, the boundary rule):
/// every scene it's sent recorded, in order, each answered at once with a stand-in PNG that
/// says the scene's size (`PNG 40x30`, not an image), or with the error it's told to give.
#[derive(Default)]
pub struct FakeRenderer {
    sent: Mutex<Vec<silico_protocol::RenderScene>>,
    failing: Mutex<Option<Error>>,
    cancelling: std::sync::atomic::AtomicBool,
    holding: std::sync::atomic::AtomicBool,
}

impl FakeRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every scene sent so far.
    pub fn sent(&self) -> Vec<silico_protocol::RenderScene> {
        lock(&self.sent).clone()
    }

    /// Answers every render from now on with `error`, as a worker that failed would.
    pub fn fail_with(&self, error: Error) {
        *lock(&self.failing) = Some(error);
    }

    /// Answers every render from now on cancelled, as one cancelled while it ran would.
    pub fn cancel_all(&self) {
        self.cancelling
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Holds every render from now on, queued and never drawn, until it's cancelled (its
    /// read's stop), which answers it cancelled, as a render still queued is.
    pub fn hold_all(&self) {
        self.holding
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl crate::command::render::Renderer for FakeRenderer {
    fn render(&self, scene: silico_protocol::RenderScene) -> crate::render::RenderTicket {
        let png = format!("PNG {}x{}", scene.width, scene.height).into_bytes();
        lock(&self.sent).push(scene);
        if self.cancelling.load(std::sync::atomic::Ordering::SeqCst) {
            return crate::render::RenderTicket::answered(crate::render::RenderOutcome::Cancelled);
        }
        if self.holding.load(std::sync::atomic::Ordering::SeqCst) {
            return crate::render::RenderTicket::queued();
        }
        let outcome = match lock(&self.failing).clone() {
            Some(error) => Err(error),
            None => Ok(crate::render::Rendered { png, ids: None }),
        };
        crate::render::RenderTicket::answered(crate::render::RenderOutcome::Done(outcome))
    }
}

// --- Sketches ---------------------------------------------------------------------------

/// An in-memory sketch solver for tests: the [`SketchSolver`] core tests run recomputes
/// with, so they need no sketch worker and can see what was asked of it. Unless told how to
/// answer ([`FakeSketches::answer`]), it answers every sketch with the values it was sent,
/// as solved ([`FakeSketches::as_sent`]), whether they meet the constraints or not. Clones
/// share one record, so a test keeps one while a recompute owns another.
///
/// Solves can be held until released ([`FakeSketches::hold`]), to supersede a recompute
/// while it solves: a held solve that's stopped ([`SketchSolver::stopper`]) fails as a
/// worker that was killed does.
#[derive(Clone, Default)]
pub struct FakeSketches {
    shared: Arc<SketchShared>,
}

#[derive(Default)]
struct SketchShared {
    state: Mutex<SketchState>,
    changed: Condvar,
}

/// How the fake answers a sketch.
type SketchAnswer = Arc<dyn Fn(&SketchSystem) -> Result<Solved, Error> + Send + Sync>;

/// How the fake answers a drag's move, from the drag's begin and the cursor.
type MoveAnswer = Arc<dyn Fn(&DragBegin, [f64; 2]) -> Result<Moved, Error> + Send + Sync>;

/// A drag request the fake was sent ([`FakeSketches::drag_calls`]).
#[derive(Debug, Clone, PartialEq)]
pub enum DragCall {
    Begin(DragBegin),
    Move([f64; 2]),
    End,
}

#[derive(Default)]
struct SketchState {
    requests: Vec<SketchSystem>,
    /// Every drag request, in order, and the drag under way.
    drag_calls: Vec<DragCall>,
    drag: Option<DragBegin>,
    /// Constraints a drag's begin finds in conflict, so it starts none.
    drag_conflicting: Vec<Tag>,
    /// How a move is answered, if not by [`FakeSketches::dragged`].
    move_answer: Option<MoveAnswer>,
    /// The time each move was given, in order.
    move_timeouts: Vec<Duration>,
    answer: Option<SketchAnswer>,
    held: bool,
    /// Solves waiting in a hold now.
    waiting: usize,
    /// Each solve's number, from 1; and the newest stopped.
    solves: u64,
    stopped: u64,
    stops: usize,
    /// Each stop's wait: from when its stopper was given out to when it stopped.
    stop_waits: Vec<Duration>,
}

impl FakeSketches {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every sketch it was asked to solve, in order.
    pub fn requests(&self) -> Vec<SketchSystem> {
        lock(&self.shared.state).requests.clone()
    }

    /// Every drag request it was sent, in order.
    pub fn drag_calls(&self) -> Vec<DragCall> {
        lock(&self.shared.state).drag_calls.clone()
    }

    /// The time each drag move was given, in order.
    pub fn move_timeouts(&self) -> Vec<Duration> {
        lock(&self.shared.state).move_timeouts.clone()
    }

    /// Finds `tags` in conflict at each drag's begin from now on, so it starts none.
    pub fn conflicting_drags(&self, tags: Vec<Tag>) {
        lock(&self.shared.state).drag_conflicting = tags;
    }

    /// Answers each drag's move from now on with `answer`'s.
    pub fn answer_moves(
        &self,
        answer: impl Fn(&DragBegin, [f64; 2]) -> Result<Moved, Error> + Send + Sync + 'static,
    ) {
        lock(&self.shared.state).move_answer = Some(Arc::new(answer));
    }

    /// A move of `begin`'s drag to `to`, as the fake answers one: from the begin state
    /// alone, each point a pin holds to the cursor (the dragged point; a line's ends) at the
    /// cursor and its offset, nudged by a thousandth of where it began, so a drag begun again
    /// from a later frame answers otherwise; every other value as it began (a target on a
    /// curve moves nothing).
    pub fn dragged(begin: &DragBegin, to: [f64; 2]) -> Moved {
        let mut values = begin.system.values.clone();
        for pin in begin.pins() {
            let (silico_protocol::PinAt::Point(point), Some(follow)) = (pin.at, pin.follows) else {
                continue;
            };
            let Some(p) = begin.system.point_values(point) else {
                continue;
            };
            let (x, y) = (p.x.0 as usize, p.y.0 as usize);
            if let (Some(&bx), Some(&by)) = (begin.system.values.get(x), begin.system.values.get(y))
            {
                let [tx, ty] = follow.at(to);
                values[x] = tx + bx / 1000.0;
                values[y] = ty + by / 1000.0;
            }
        }
        Moved {
            status: SolveStatus::Success,
            values: Some(values),
        }
    }

    /// Waits as a held solve does: until released or stopped. Whether it was stopped.
    fn held_until<'a>(
        &'a self,
        mut state: std::sync::MutexGuard<'a, SketchState>,
        solve: u64,
    ) -> (std::sync::MutexGuard<'a, SketchState>, bool) {
        if state.held {
            state.waiting += 1;
            self.shared.changed.notify_all();
            while state.held && state.stopped < solve {
                state = self
                    .shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            state.waiting -= 1;
        }
        let stopped = state.stopped >= solve;
        if stopped {
            state.stops += 1;
        }
        (state, stopped)
    }

    /// Answers each sketch from now on with `answer`'s.
    pub fn answer(
        &self,
        answer: impl Fn(&SketchSystem) -> Result<Solved, Error> + Send + Sync + 'static,
    ) {
        lock(&self.shared.state).answer = Some(Arc::new(answer));
    }

    /// Holds every solve from now on until [`release`](Self::release) or a stop.
    pub fn hold(&self) {
        lock(&self.shared.state).held = true;
    }

    /// Lets held solves, and later ones, go.
    pub fn release(&self) {
        lock(&self.shared.state).held = false;
        self.shared.changed.notify_all();
    }

    /// Waits until a solve is held, up to `timeout`: whether one is.
    pub fn wait_held(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.shared.state);
        while state.waiting == 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// How many solves were stopped while they ran.
    pub fn stops(&self) -> usize {
        lock(&self.shared.state).stops
    }

    /// For each stop, in order, how long after its stopper was given out it came.
    pub fn stop_waits(&self) -> Vec<Duration> {
        lock(&self.shared.state).stop_waits.clone()
    }

    /// `system` solved where it was sent: every value as given, each constraint met.
    pub fn as_sent(system: &SketchSystem) -> Solved {
        let mut tags: Vec<Tag> = system.constraints.iter().filter_map(|c| c.tag()).collect();
        tags.sort_unstable();
        Solved {
            dofs: 0,
            conflicting: Vec::new(),
            redundant: Vec::new(),
            partially_redundant: Vec::new(),
            free: Vec::new(),
            no_equations: false,
            status: SolveStatus::Success,
            algorithm: Some(Algorithm::DogLeg),
            values: system.values.clone(),
            residuals: tags.into_iter().map(|t| (t, Some(0.0))).collect(),
        }
    }
}

impl SketchSolver for FakeSketches {
    fn drag_begin(&mut self, begin: DragBegin) -> Result<DragStarted, Error> {
        let mut state = lock(&self.shared.state);
        state.solves += 1;
        state.drag_calls.push(DragCall::Begin(begin.clone()));
        let conflicting = state.drag_conflicting.clone();
        state.drag = conflicting.is_empty().then_some(begin);
        Ok(DragStarted {
            dofs: 1,
            conflicting,
        })
    }

    fn drag_move(&mut self, to: [f64; 2], timeout: Duration) -> Result<Moved, Error> {
        let mut state = lock(&self.shared.state);
        state.solves += 1;
        let solve = state.solves;
        state.drag_calls.push(DragCall::Move(to));
        state.move_timeouts.push(timeout);
        let (mut state, stopped) = self.held_until(state, solve);
        if stopped {
            // Stopped as a worker is, its session with it.
            state.drag = None;
            return Err(Error::new(
                ErrorCode::WorkerCrashed,
                "the fake sketch solver was stopped mid-move",
            ));
        }
        let Some(begin) = state.drag.clone() else {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "the fake sketch solver has no drag under way",
            ));
        };
        let answer = state.move_answer.clone();
        drop(state);
        match answer {
            Some(answer) => answer(&begin, to),
            None => Ok(Self::dragged(&begin, to)),
        }
    }

    /// One move as the worker's `MoveOnce`: its begin, the move under no time but the
    /// solver's own (`Duration::MAX`), and its end, whatever the move did. One request, as
    /// the worker's is, so one solve to a stopper: a stopper given out before it stops its
    /// move, held or not, as a killed worker ends the request.
    fn move_once(
        &mut self,
        begin: DragBegin,
        to: [f64; 2],
    ) -> Result<crate::sketch::MovedOnce, Error> {
        let before = lock(&self.shared.state).solves;
        let started = self.drag_begin(begin)?;
        if !started.conflicting.is_empty() {
            return Ok(crate::sketch::MovedOnce::Conflicting(started.conflicting));
        }
        lock(&self.shared.state).solves = before;
        let moved = self.drag_move(to, Duration::MAX);
        let _ = self.drag_end();
        lock(&self.shared.state).solves = before + 1;
        Ok(crate::sketch::MovedOnce::Moved(moved?))
    }

    fn drag_end(&mut self) -> Result<(), Error> {
        let mut state = lock(&self.shared.state);
        state.solves += 1;
        state.drag_calls.push(DragCall::End);
        match state.drag.take() {
            Some(_) => Ok(()),
            None => Err(Error::new(
                ErrorCode::InvalidInput,
                "the fake sketch solver has no drag under way",
            )),
        }
    }

    fn stopper(&mut self) -> Result<Option<SolveStopper>, Error> {
        let shared = Arc::clone(&self.shared);
        let solve = lock(&self.shared.state).solves + 1;
        let given = Instant::now();
        Ok(Some(SolveStopper::new(move || {
            let mut state = lock(&shared.state);
            state.stopped = state.stopped.max(solve);
            state.stop_waits.push(given.elapsed());
            shared.changed.notify_all();
        })))
    }

    fn solve(&mut self, system: SketchSystem) -> Result<Solved, Error> {
        let mut state = lock(&self.shared.state);
        state.solves += 1;
        let solve = state.solves;
        state.requests.push(system.clone());
        // A solve ends any drag under way, as the worker's does.
        state.drag = None;
        let (state, stopped) = self.held_until(state, solve);
        if stopped {
            return Err(Error::new(
                ErrorCode::WorkerCrashed,
                "the fake sketch solver was stopped mid-solve",
            ));
        }
        let answer = state.answer.clone();
        drop(state);
        match answer {
            Some(answer) => answer(&system),
            None => Ok(Self::as_sent(&system)),
        }
    }
}
