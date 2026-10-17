//! An in-memory kernel for tests: the [`Kernel`] that core tests run on, so they need no
//! OCCT and can see what was asked of the kernel. It runs requests on threads as the pool
//! runs them on workers, with the same generations, and keeps a record of every call.
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
//! their centers weighed by volume, and the box around both. It answers making primitives,
//! moving shapes, booleans, summaries, and echoes; anything else is refused as an internal
//! error.
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
//! at once or to supersede a request while it runs, and have requests refused.

use std::collections::VecDeque;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use silico_protocol::{Reply, ShapeSummary};

use crate::kernel::{
    Blob, BooleanOp, BuiltData, ElementCounts, ElementFate, ElementKind, ElementRef, Ends,
    Generation, History, InputHistory, Kernel, KernelRequest, Op, Primitive, Reported, ShapeData,
    Topology, cone_caps, primitive_counts,
};
use crate::pool::{MAX_WORKERS, Outcome, Pending, deliver, thread_error};
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// What the fake calls itself in its errors: a [`Pending`] stopped early, a thread that
/// couldn't start.
pub(crate) const KERNEL: &str = "fake kernel";

/// A request the fake kernel started: the generation it was for, and its operation.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub generation: Generation,
    pub op: Op,
}

/// The in-memory kernel. Dropping it ends requests still waiting or held without a
/// result, and joins its threads.
pub struct FakeKernel {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

/// Holds requests a predicate picks, from when they start until it's released or dropped,
/// or their generation is superseded (which stops them, as the pool stops a superseded
/// request).
#[must_use = "dropping a hold releases it"]
pub struct Hold {
    shared: Arc<Shared>,
    id: usize,
}

type Picks = dyn Fn(&Op) -> bool + Send + Sync;
type Refuses = dyn Fn(&Op) -> Option<Error> + Send + Sync;

struct Shared {
    state: Mutex<State>,
    /// The queue changed, or the kernel is stopping.
    work: Condvar,
    /// A request started or ended, or a hold was released.
    changed: Condvar,
}

struct State {
    generation: Generation,
    queue: VecDeque<Box<dyn Task>>,
    stopping: bool,
    calls: Vec<Call>,
    running: usize,
    most_running: usize,
    holds: Vec<HoldRule>,
    refusals: Vec<Arc<Refuses>>,
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
                generation: Generation(0),
                queue: VecDeque::new(),
                stopping: false,
                calls: Vec::new(),
                running: 0,
                most_running: 0,
                holds: Vec::new(),
                refusals: Vec::new(),
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

    /// The most requests that have run at once.
    pub fn most_running(&self) -> usize {
        lock(&self.shared.state).most_running
    }

    /// Waits up to `timeout` until at least `count` requests are running at once: whether
    /// they were. A request has stopped running once its outcome is sent.
    pub fn wait_running(&self, count: usize, timeout: Duration) -> bool {
        // No deadline: a timeout too long to add to now is forever.
        let deadline = Instant::now().checked_add(timeout);
        let mut state = lock(&self.shared.state);
        while state.running < count {
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

    /// How a fake kernel made `shape`, written out (`cut(box(40, 20, 10), sphere(5))`), if
    /// it's a fake kernel's shape.
    pub fn describe(shape: &ShapeData) -> Option<String> {
        Symbolic::read(shape).ok().map(|made| made.describe())
    }

    /// The faces, edges, vertices, and solids of `shape`, if it's a fake kernel's shape.
    pub fn counts(shape: &ShapeData) -> Option<ElementCounts> {
        Symbolic::read(shape).ok().map(|made| made.counts())
    }
}

impl Kernel for FakeKernel {
    fn identity(&self) -> &str {
        "silico fake kernel 1"
    }

    fn generation(&self) -> Generation {
        lock(&self.shared.state).generation
    }

    fn advance(&self, generation: Generation) {
        let stale: Vec<Box<dyn Task>> = {
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
            stale.into_iter().collect()
        };
        // Held requests of older generations stop.
        self.shared.changed.notify_all();
        for task in stale {
            task.supersede();
        }
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        let (reply, outcome) = mpsc::sync_channel(1);
        let pending = Pending::new(request.name(), KERNEL, outcome);
        let task = Box::new(Submitted {
            generation,
            request,
            reply,
            span: tracing::Span::current(),
        });
        let mut state = lock(&self.shared.state);
        if state.stopping {
            // Dropping the task tells the caller the kernel stopped.
            return pending;
        }
        if generation < state.generation {
            drop(state);
            task.supersede();
            return pending;
        }
        state.queue.push_back(task);
        drop(state);
        self.shared.work.notify_one();
        pending
    }
}

impl Drop for FakeKernel {
    fn drop(&mut self) {
        let waiting: Vec<Box<dyn Task>> = {
            let mut state = lock(&self.shared.state);
            state.stopping = true;
            state.queue.drain(..).collect()
        };
        drop(waiting);
        self.shared.work.notify_all();
        self.shared.changed.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
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
trait Task: Send {
    fn generation(&self) -> Generation;
    fn op(&self) -> &Op;
    /// Answers the request (with `refusal`, if a rule gave one), reads the answer as a
    /// worker's is read, in the span it was submitted in, and ends it: under the lock
    /// `advance` takes, as the pool does, it stops counting as running and its outcome is
    /// sent, superseded if a newer generation has started. Once `advance` returns, no result
    /// of an older generation can still arrive.
    fn run(self: Box<Self>, refusal: Option<Error>, shared: &Shared);
    /// Ends a request that started without running it, under the lock, as `run` does.
    fn stop(self: Box<Self>, shared: &Shared);
    /// Sends [`Outcome::Superseded`] for a request that never started.
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

    fn op(&self) -> &Op {
        self.request.op()
    }

    fn run(self: Box<Self>, refusal: Option<Error>, shared: &Shared) {
        let Submitted {
            generation,
            request,
            reply,
            span,
        } = *self;
        let result = span.in_scope(|| {
            let reply = match refusal {
                Some(error) => Err(error),
                None => answer(request.op()),
            };
            request.answer(reply)
        });
        let mut state = lock(&shared.state);
        state.running -= 1;
        let outcome = if generation < state.generation {
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

/// One of the kernel's threads: runs queued requests until the kernel stops.
fn serve(shared: &Shared) {
    loop {
        let mut state = lock(&shared.state);
        let task = loop {
            if state.stopping {
                return;
            }
            if let Some(task) = state.queue.pop_front() {
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
            && task.generation() >= state.generation
            && holding
                .iter()
                .any(|&index| state.holds.get(index).is_some_and(|rule| !rule.released))
        {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if state.stopping || task.generation() < state.generation {
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
        drop(state);
        task.run(refusal, shared);
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
                result: sum(a, b),
                ends: Ends::default(),
                topology: made.topology(),
            };
            let shape = made.shape();
            Ok(Reply::Built(BuiltData { shape, history }))
        }
        Op::Summarize { shape } => Ok(Reply::Summary(Symbolic::read(shape)?.summary())),
        Op::Echo { data } => Ok(Reply::Echo { data: data.clone() }),
        other => Err(Error::new(
            ErrorCode::Internal,
            format!("the fake kernel doesn't do {}", other.name()),
        )
        .with_suggestion("run this test on a real kernel worker")),
    }
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

/// How OCCT's primitive's elements bound each other, as the kernel reports it: each face's
/// edges in OCCT's order, its outer wire's (sorted), and each edge's vertices.
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
    Topology {
        face_edges: lists(faces),
        outer_edges: lists(outer),
        edge_vertices: lists(vertices),
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

/// One step of making a shape: a primitive, or an operation on earlier steps, by index.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Primitive(Primitive),
    Translate {
        input: usize,
        offset: [f64; 3],
    },
    Boolean {
        op: BooleanOp,
        base: usize,
        tool: usize,
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
        let Symbolic(mut steps) = base;
        let shift = steps.len();
        steps.extend(tool.0.into_iter().map(|step| match step {
            Step::Primitive(_) => step,
            Step::Translate { input, offset } => Step::Translate {
                input: input + shift,
                offset,
            },
            Step::Boolean { op, base, tool } => Step::Boolean {
                op,
                base: base + shift,
                tool: tool + shift,
            },
        }));
        let tool = steps.len() - 1;
        steps.push(Step::Boolean {
            op,
            base: shift - 1,
            tool,
        });
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
                Step::Translate {
                    input,
                    offset: [x, y, z],
                } => text.push_str(&format!("translate {input} {x} {y} {z}")),
                Step::Boolean { op, base, tool } => {
                    text.push_str(&format!("{} {base} {tool}", op.word()));
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
            let size = match step {
                Step::Primitive(_) => Some(1),
                Step::Translate { input, offset } => (index > 0
                    && input == index - 1
                    && !nothing(&offset)
                    && !matches!(steps.get(input), Some(Step::Translate { .. })))
                .then(|| sizes.get(input).map(|s| s + 1))
                .flatten(),
                Step::Boolean { base, tool, .. } => (index > 0 && tool == index - 1)
                    .then(|| {
                        let tool_size = *sizes.get(tool)?;
                        let base_size = *sizes.get(base)?;
                        (index.checked_sub(1 + tool_size) == Some(base))
                            .then_some(base_size + tool_size + 1)
                    })
                    .flatten(),
            };
            sizes.push(size.ok_or_else(refused)?);
            steps.push(step);
        }
        let made = Symbolic(steps);
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
            let made = match step {
                Step::Primitive(primitive) => primitive_counts(primitive),
                Step::Translate { input, .. } => of(*input),
                Step::Boolean { base, tool, .. } => sum(of(*base), of(*tool)),
            };
            counts.push(made);
        }
        counts.last().copied().unwrap_or_default()
    }

    /// How the shape's elements bound each other: a primitive's as OCCT has them, a move's
    /// its input's, and the fake's boolean its inputs' together, the tool's numbered after
    /// the base's, as its history keeps them.
    fn topology(&self) -> Topology {
        let mut made: Vec<(Topology, ElementCounts)> = Vec::with_capacity(self.0.len());
        for step in &self.0 {
            let of = |i: usize| made.get(i).cloned().unwrap_or_default();
            let next = match step {
                Step::Primitive(primitive) => {
                    (primitive_topology(primitive), primitive_counts(primitive))
                }
                Step::Translate { input, .. } => of(*input),
                Step::Boolean { base, tool, .. } => {
                    let ((mut topology, a), (tool, b)) = (of(*base), of(*tool));
                    let tool = shifted(&tool, a);
                    topology.face_edges.extend(tool.face_edges);
                    topology.outer_edges.extend(tool.outer_edges);
                    topology.edge_vertices.extend(tool.edge_vertices);
                    (topology, sum(a, b))
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
                Step::Translate { input, offset } => of(*input).moved(*offset),
                Step::Boolean { base, tool, .. } => of(*base).with(of(*tool)),
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
                    None => {}
                },
            }
        }
        out
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

#[cfg(test)]
mod tests {
    use super::*;

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
