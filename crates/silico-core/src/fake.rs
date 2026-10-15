//! An in-memory kernel for tests: the [`Kernel`] that core tests run on, so they need no
//! OCCT and can see what was asked of the kernel. It runs requests on threads as the pool
//! runs them on workers, with the same generations, and keeps a record of every call.
//!
//! Its shapes are symbolic: a primitive is its values, a moved shape what was moved and by
//! how much, a boolean its operation and inputs (`cut(box(40, 20, 10), translate(cylinder(
//! 2.5, 12), 20, 10, -1))`, as [`FakeKernel::describe`] writes it). Each is named by a hash
//! of what made it, so the same request makes the same bytes whichever thread runs it and
//! whenever. A primitive has the faces, edges, and vertices OCCT gives it; a move reports
//! its elements as the kernel does (each modified into its own number, or kept by a move of
//! nothing); a boolean has both inputs' elements, as if they didn't touch, and keeps
//! them all. It answers making primitives, moving shapes, booleans, and echoes; anything
//! else is refused as an internal error.
//!
//! Tests can hold requests until they're released ([`FakeKernel::hold`]), to see what runs
//! at once or to supersede a request while it runs, and have requests refused
//! ([`FakeKernel::refuse`]).

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use silico_protocol::Reply;

use crate::kernel::{
    Blob, BooleanOp, BuiltData, ElementCounts, ElementFate, ElementKind, ElementRef, Ends,
    Generation, History, InputHistory, Kernel, KernelRequest, Op, Primitive, Reported, ShapeData,
};
use crate::pool::{Outcome, Pending};
use crate::worker::lock;
use crate::{Error, ErrorCode};

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
    shapes: BTreeMap<u128, Node>,
    holds: Vec<HoldRule>,
    refusals: Vec<Arc<Refuses>>,
}

struct HoldRule {
    picks: Arc<Picks>,
    released: bool,
}

/// A shape the fake made: how, and its element counts.
#[derive(Debug, Clone)]
struct Node {
    made: Made,
    counts: ElementCounts,
}

#[derive(Debug, Clone)]
enum Made {
    Primitive(Primitive),
    Translated {
        shape: u128,
        offset: [f64; 3],
    },
    Boolean {
        op: BooleanOp,
        base: u128,
        tool: u128,
    },
}

impl FakeKernel {
    /// A fake kernel running up to `size` requests at once (at least one).
    pub fn new(size: usize) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                generation: Generation(0),
                queue: VecDeque::new(),
                stopping: false,
                calls: Vec::new(),
                running: 0,
                most_running: 0,
                shapes: BTreeMap::new(),
                holds: Vec::new(),
                refusals: Vec::new(),
            }),
            work: Condvar::new(),
            changed: Condvar::new(),
        });
        let threads = (0..size.max(1))
            .map(|_| {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || serve(&shared))
            })
            .collect();
        Self { shared, threads }
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
    /// they were.
    pub fn wait_running(&self, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.shared.state);
        while state.running < count {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, left)
                .map_or_else(|e| e.into_inner().0, |(state, _)| state);
        }
        true
    }

    /// Holds every request `picks` (called with the kernel's state locked, so it mustn't
    /// call the kernel) once it has started, until the hold is released.
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

    /// How the fake made `shape`, written out (`cut(box(40, 20, 10), sphere(5))`), if it
    /// made it.
    pub fn describe(&self, shape: &ShapeData) -> Option<String> {
        let state = lock(&self.shared.state);
        let mut out = String::new();
        write_shape(&state.shapes, id_of(shape)?, &mut out).then_some(out)
    }

    /// The faces, edges, vertices, and solids of `shape`, if the fake made it.
    pub fn counts(&self, shape: &ShapeData) -> Option<ElementCounts> {
        let state = lock(&self.shared.state);
        state.shapes.get(&id_of(shape)?).map(|node| node.counts)
    }
}

impl Kernel for FakeKernel {
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
        let pending = Pending::new(request.op().name(), outcome);
        let task = Box::new(Submitted {
            generation,
            request,
            reply,
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
    /// Reads `reply` as a worker's is read, and sends the outcome: superseded, if a newer
    /// generation has started by then.
    fn finish(self: Box<Self>, reply: Result<Reply, Error>, shared: &Shared);
    fn supersede(self: Box<Self>);
}

struct Submitted<T> {
    generation: Generation,
    request: KernelRequest<T>,
    reply: SyncSender<Outcome<T>>,
}

impl<T: Send + 'static> Task for Submitted<T> {
    fn generation(&self) -> Generation {
        self.generation
    }

    fn op(&self) -> &Op {
        self.request.op()
    }

    fn finish(self: Box<Self>, reply: Result<Reply, Error>, shared: &Shared) {
        let result = self.request.answer(reply);
        let superseded = {
            let mut state = lock(&shared.state);
            state.running -= 1;
            self.generation < state.generation
        };
        shared.changed.notify_all();
        let outcome = if superseded {
            Outcome::Superseded
        } else {
            Outcome::Done(result)
        };
        let _ = self.reply.send(outcome);
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
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        };
        state.running += 1;
        state.most_running = state.most_running.max(state.running);
        state.calls.push(Call {
            generation: task.generation(),
            op: task.op().clone(),
        });
        shared.changed.notify_all();
        while !state.stopping
            && task.generation() >= state.generation
            && state
                .holds
                .iter()
                .any(|rule| !rule.released && (rule.picks)(task.op()))
        {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if state.stopping || task.generation() < state.generation {
            state.running -= 1;
            let stopping = state.stopping;
            drop(state);
            shared.changed.notify_all();
            if !stopping {
                task.supersede();
            }
            continue;
        }
        let refusal = state.refusals.iter().find_map(|refuses| refuses(task.op()));
        let reply = match refusal {
            Some(error) => Err(error),
            None => answer(&mut state.shapes, task.op()),
        };
        drop(state);
        task.finish(reply, shared);
    }
}

/// The fake's reply to `op`, recording the shapes it makes in `shapes`.
fn answer(shapes: &mut BTreeMap<u128, Node>, op: &Op) -> Result<Reply, Error> {
    match op {
        Op::MakePrimitive(primitive) => {
            let node = Node {
                made: Made::Primitive(*primitive),
                counts: primitive_counts(primitive),
            };
            Ok(Reply::Shape(store(shapes, node)))
        }
        Op::Translate { shape, offset } => {
            let (id, input) = find(shapes, shape)?;
            let node = Node {
                made: Made::Translated {
                    shape: id,
                    offset: *offset,
                },
                counts: input.counts,
            };
            // As the kernel reports a move: each element modified into its own number (a
            // moved element isn't the same sub-shape), or kept by a move of nothing.
            let fates = if offset.iter().all(|&d| d == 0.0) {
                kept(input.counts, ElementCounts::default())
            } else {
                moved(input.counts)
            };
            let history = History {
                inputs: vec![fates],
                result: input.counts,
                ends: Ends::default(),
            };
            let shape = store(shapes, node);
            Ok(Reply::Built(BuiltData { shape, history }))
        }
        Op::Boolean { op, base, tool } => {
            let (base_id, base) = find(shapes, base)?;
            let (tool_id, tool) = find(shapes, tool)?;
            let (a, b) = (base.counts, tool.counts);
            let counts = ElementCounts {
                faces: a.faces.saturating_add(b.faces),
                edges: a.edges.saturating_add(b.edges),
                vertices: a.vertices.saturating_add(b.vertices),
                solids: a.solids.saturating_add(b.solids),
            };
            let history = History {
                inputs: vec![kept(a, ElementCounts::default()), kept(b, a)],
                result: counts,
                ends: Ends::default(),
            };
            let node = Node {
                made: Made::Boolean {
                    op: *op,
                    base: base_id,
                    tool: tool_id,
                },
                counts,
            };
            let shape = store(shapes, node);
            Ok(Reply::Built(BuiltData { shape, history }))
        }
        Op::Echo { data } => Ok(Reply::Echo { data: data.clone() }),
        other => Err(Error::new(
            ErrorCode::Internal,
            format!("the fake kernel doesn't do {}", other.name()),
        )
        .with_suggestion("run this test on a real kernel worker")),
    }
}

/// The faces, edges, vertices, and solids OCCT gives a primitive.
fn primitive_counts(primitive: &Primitive) -> ElementCounts {
    let (faces, edges, vertices) = match *primitive {
        Primitive::Box { .. } => (6, 12, 8),
        Primitive::Cylinder { .. } => (3, 3, 2),
        Primitive::Sphere { .. } => (1, 3, 2),
        Primitive::Cone {
            radius1, radius2, ..
        } => {
            if radius1 == 0.0 || radius2 == 0.0 {
                (2, 3, 2)
            } else {
                (3, 3, 2)
            }
        }
        Primitive::Torus { .. } => (1, 2, 1),
    };
    ElementCounts {
        faces,
        edges,
        vertices,
        solids: 1,
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

/// The shape `shape` names, if the fake made it.
fn find<'a>(
    shapes: &'a BTreeMap<u128, Node>,
    shape: &ShapeData,
) -> Result<(u128, &'a Node), Error> {
    id_of(shape)
        .and_then(|id| shapes.get(&id).map(|node| (id, node)))
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidInput,
                "the fake kernel was given a shape it didn't make",
            )
            .with_suggestion("give the fake kernel only shapes it made")
        })
}

/// Records `node`, and the shape that names it.
fn store(shapes: &mut BTreeMap<u128, Node>, node: Node) -> ShapeData {
    let id = hash(&made_key(&node.made));
    shapes.entry(id).or_insert(node);
    ShapeData::checked(Blob::inline(format!("fake:{id:032x}").into_bytes()), true)
}

fn id_of(shape: &ShapeData) -> Option<u128> {
    let text = std::str::from_utf8(shape.brep().as_bytes()).ok()?;
    let hex = text.strip_prefix("fake:")?;
    (hex.len() == 32)
        .then(|| u128::from_str_radix(hex, 16).ok())
        .flatten()
}

/// What made a shape, as text naming its inputs by their IDs: what its ID is a hash of.
/// Numbers are written as Rust writes them, which reads back as the same number.
fn made_key(made: &Made) -> String {
    match made {
        Made::Primitive(primitive) => primitive_text(primitive),
        Made::Translated { shape, offset } => {
            let [x, y, z] = offset;
            format!("translate({shape:032x}, {x}, {y}, {z})")
        }
        Made::Boolean { op, base, tool } => {
            format!("{}({base:032x}, {tool:032x})", boolean_name(*op))
        }
    }
}

fn primitive_text(primitive: &Primitive) -> String {
    match *primitive {
        Primitive::Box { dx, dy, dz } => format!("box({dx}, {dy}, {dz})"),
        Primitive::Cylinder { radius, height } => format!("cylinder({radius}, {height})"),
        Primitive::Sphere { radius } => format!("sphere({radius})"),
        Primitive::Cone {
            radius1,
            radius2,
            height,
        } => format!("cone({radius1}, {radius2}, {height})"),
        Primitive::Torus {
            major_radius,
            minor_radius,
        } => format!("torus({major_radius}, {minor_radius})"),
    }
}

fn boolean_name(op: BooleanOp) -> &'static str {
    match op {
        BooleanOp::Fuse => "fuse",
        BooleanOp::Cut => "cut",
        BooleanOp::Common => "common",
    }
}

/// Writes shape `id` out in full into `out`: whether the fake has it and every shape it was
/// made from. Iterative, so a long chain of booleans can't overflow the stack.
fn write_shape(shapes: &BTreeMap<u128, Node>, id: u128, out: &mut String) -> bool {
    enum Step {
        Shape(u128),
        Text(String),
    }
    let mut steps = vec![Step::Shape(id)];
    while let Some(step) = steps.pop() {
        match step {
            Step::Text(text) => out.push_str(&text),
            Step::Shape(id) => {
                let Some(node) = shapes.get(&id) else {
                    return false;
                };
                match &node.made {
                    Made::Primitive(primitive) => out.push_str(&primitive_text(primitive)),
                    Made::Translated { shape, offset } => {
                        let [x, y, z] = offset;
                        out.push_str("translate(");
                        steps.push(Step::Text(format!(", {x}, {y}, {z})")));
                        steps.push(Step::Shape(*shape));
                    }
                    Made::Boolean { op, base, tool } => {
                        out.push_str(boolean_name(*op));
                        out.push('(');
                        steps.push(Step::Text(")".to_owned()));
                        steps.push(Step::Shape(*tool));
                        steps.push(Step::Text(", ".to_owned()));
                        steps.push(Step::Shape(*base));
                    }
                }
            }
        }
    }
    true
}

/// FNV-1a, 128 bits: a stable hash, the same on every platform and in every run.
fn hash(text: &str) -> u128 {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    text.bytes().fold(OFFSET, |hash, byte| {
        (hash ^ u128::from(byte)).wrapping_mul(PRIME)
    })
}
