//! Recompute: a checked document's features rebuilt on a [`Kernel`] (docs/ports/recompute.md).
//!
//! Each feature is one or two kernel steps (a primitive made at the origin, then moved to its
//! position; a boolean of two earlier features' solids), and each step has a [`Key`]: a hash
//! of exactly what it computes and of its inputs' keys, never of a feature's ID, name, note,
//! or place in the timeline. The keys are all known before anything runs, so a recompute
//! runs only the steps whose keys it has no result for: an unchanged document runs nothing,
//! a rename runs nothing, and a value edit runs that feature's steps and those below it.
//! Identical steps run once.
//!
//! Steps run as soon as their inputs are built, several at once, so independent branches
//! overlap on the kernel; nothing depends on the order they finish in. A step the kernel
//! refuses fails its feature, and each feature using a failed (or blocked) one is blocked on
//! it, as in SolidWorks; every other feature builds. A refusal that follows from the step's
//! values (`invalid_input`, `kernel_failed`, `invalid_result`) is kept with its key, so it
//! isn't asked again until something changes; anything else (a crash, a timeout, a
//! shortage) is tried again by the next recompute.
//!
//! Every recompute is a new generation of the kernel's ([`Kernel::advance`]), which
//! supersedes the one before: its steps waiting are dropped and those running stopped. A
//! recompute builds its [`Model`] aside and publishes it whole, only if no newer one has
//! started, so a superseded recompute leaves the published model as it was.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::document::{Checked, FeatureId, Operation};
use crate::kernel::{
    BooleanOp, BuiltData, Generation, History, Kernel, KernelRequest, Primitive, ShapeData,
};
use crate::pool::Outcome;
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// The most steps one recompute waits on at once: enough to keep a pool of workers busy
/// (the kernel queues what it can't run yet), few enough that a document of thousands of
/// independent features doesn't start thousands of threads.
pub const MOST_AT_ONCE: usize = 64;

/// A kernel step's key: SHA-256 of what the step computes, exactly (every number by its
/// bits), and of its inputs' keys.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key([u8; 32]);

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({self})")
    }
}

/// What a kernel step made: its shape, and, for an operation on other shapes, what it did
/// to them.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub shape: ShapeData,
    pub history: Option<History>,
}

/// What became of a feature in a recompute.
#[derive(Debug, Clone, PartialEq)]
pub enum FeatureOutcome {
    /// Built: its last step's key and result.
    Built { key: Key, built: Arc<Built> },
    /// The kernel refused one of its steps: why, naming the feature.
    Failed(Error),
    /// Not built, since it uses a feature that failed: that feature (the first failure
    /// along its base, then its tool).
    Blocked { on: FeatureId },
}

/// A document as rebuilt: what became of each feature, in timeline order, and its bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    generation: Generation,
    features: Vec<(FeatureId, FeatureOutcome)>,
    bodies: Vec<FeatureId>,
}

impl Model {
    /// The generation that built it.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// What became of each feature, in timeline order.
    pub fn features(&self) -> &[(FeatureId, FeatureOutcome)] {
        &self.features
    }

    /// What became of the feature `id`, if the document has it.
    pub fn outcome(&self, id: &FeatureId) -> Option<&FeatureOutcome> {
        self.features
            .iter()
            .find(|(feature, _)| feature == id)
            .map(|(_, outcome)| outcome)
    }

    /// The solids the model ends up with: every built feature no built feature consumes, in
    /// timeline order. A failed or blocked feature consumes nothing, so its inputs stay
    /// bodies, as if it were suppressed.
    pub fn bodies(&self) -> &[FeatureId] {
        &self.bodies
    }

    /// Why each failed feature failed, in timeline order: one error per failure, each
    /// naming its feature.
    pub fn errors(&self) -> impl Iterator<Item = &Error> {
        self.features
            .iter()
            .filter_map(|(_, outcome)| match outcome {
                FeatureOutcome::Failed(error) => Some(error),
                FeatureOutcome::Built { .. } | FeatureOutcome::Blocked { .. } => None,
            })
    }
}

/// What a recompute came to.
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// It finished, and its model is now the published one.
    Published(Arc<Model>),
    /// A newer recompute started first: nothing of this one was published.
    Superseded,
}

/// A recompute running in the background: wait for it, poll it, or drop it (it runs on).
#[must_use = "a recompute's outcome is only had by waiting for it"]
pub struct Run {
    outcome: Receiver<Result<RunOutcome, Error>>,
}

impl Run {
    /// Waits for the outcome.
    pub fn wait(self) -> Result<RunOutcome, Error> {
        self.outcome.recv().unwrap_or_else(|_| Err(stopped()))
    }

    /// Waits up to `timeout` for the outcome, or gives the run back to wait on again.
    pub fn wait_timeout(self, timeout: Duration) -> Result<Result<RunOutcome, Error>, Self> {
        match self.outcome.recv_timeout(timeout) {
            Ok(outcome) => Ok(outcome),
            Err(RecvTimeoutError::Timeout) => Err(self),
            Err(RecvTimeoutError::Disconnected) => Ok(Err(stopped())),
        }
    }

    /// The outcome, if it has come, or the run back to wait on.
    pub fn try_take(self) -> Result<Result<RunOutcome, Error>, Self> {
        match self.outcome.try_recv() {
            Ok(outcome) => Ok(outcome),
            Err(TryRecvError::Empty) => Err(self),
            Err(TryRecvError::Disconnected) => Ok(Err(stopped())),
        }
    }
}

fn stopped() -> Error {
    Error::new(
        ErrorCode::Internal,
        "the recompute stopped before it finished",
    )
    .with_suggestion(crate::worker::SILICO_BUG)
}

/// Rebuilds documents on a kernel, keeping each step's result by key between recomputes.
/// Dropping it supersedes the recompute running, if any.
pub struct Recompute<K: Kernel + 'static> {
    engine: Arc<Engine<K>>,
}

struct Engine<K> {
    kernel: K,
    state: Mutex<EngineState>,
}

struct EngineState {
    /// The newest recompute's generation.
    generation: Generation,
    /// Whether the newest recompute was cancelled when no newer generation was left to
    /// supersede it with: it publishes nothing.
    cancelled: bool,
    /// Every step result kept, by key: the published model's, and any a recompute finished
    /// since.
    results: BTreeMap<Key, StepResult>,
    published: Option<Arc<Model>>,
}

type StepResult = Result<Arc<Built>, Error>;

impl<K: Kernel + 'static> Recompute<K> {
    pub fn new(kernel: K) -> Self {
        let generation = kernel.generation();
        Self {
            engine: Arc::new(Engine {
                kernel,
                state: Mutex::new(EngineState {
                    generation,
                    cancelled: false,
                    results: BTreeMap::new(),
                    published: None,
                }),
            }),
        }
    }

    pub fn kernel(&self) -> &K {
        &self.engine.kernel
    }

    /// The model the newest finished recompute published, if any has.
    pub fn published(&self) -> Option<Arc<Model>> {
        lock(&self.engine.state).published.clone()
    }

    /// Starts rebuilding `document` in the background, superseding the recompute running,
    /// if any; returns at once.
    pub fn start(&self, document: Checked) -> Run {
        let (reply, outcome) = mpsc::sync_channel(1);
        let generation = {
            let mut state = lock(&self.engine.state);
            let newest = state.generation.0.max(self.engine.kernel.generation().0);
            let Some(next) = newest.checked_add(1) else {
                let _ = reply.send(Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "the kernel has no generations left to recompute with",
                )
                .with_suggestion("start a new kernel")));
                return Run { outcome };
            };
            state.generation = Generation(next);
            state.cancelled = false;
            state.generation
        };
        self.engine.kernel.advance(generation);
        let engine = Arc::clone(&self.engine);
        let span = tracing::debug_span!("recompute", generation = generation.0);
        let started = std::thread::Builder::new()
            .name(format!("recompute-{}", generation.0))
            .spawn(move || {
                let _entered = span.enter();
                let outcome = engine.run(&document, generation);
                let _ = reply.send(outcome);
            });
        if let Err(e) = started {
            let (reply, outcome) = mpsc::sync_channel(1);
            let _ = reply.send(Err(crate::pool::thread_error("recompute", &e)));
            return Run { outcome };
        }
        Run { outcome }
    }

    /// Supersedes the recompute running, if any: nothing of it is published.
    pub fn cancel(&self) {
        let mut state = lock(&self.engine.state);
        match state.generation.0.checked_add(1) {
            Some(next) => {
                state.generation = Generation(next);
                drop(state);
                self.engine.kernel.advance(Generation(next));
            }
            // No newer generation to stop its steps with: they run on, but it publishes
            // nothing.
            None => state.cancelled = true,
        }
    }
}

impl<K: Kernel + 'static> Drop for Recompute<K> {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// What a step computes.
#[derive(Debug, Clone, Copy)]
enum Step {
    Make(Primitive),
    Translate { input: Key, offset: [f64; 3] },
    Boolean { op: BooleanOp, base: Key, tool: Key },
}

impl Step {
    /// The key: SHA-256 of a line saying what the step computes, every number by its bits.
    fn key(&self) -> Key {
        let bits = |values: &[f64]| -> String {
            values
                .iter()
                .map(|v| format!(" {:016x}", v.to_bits()))
                .collect()
        };
        let line = match *self {
            Step::Make(primitive) => {
                let values: Vec<f64> = match primitive {
                    Primitive::Box { dx, dy, dz } => vec![dx, dy, dz],
                    Primitive::Cylinder { radius, height } => vec![radius, height],
                    Primitive::Sphere { radius } => vec![radius],
                    Primitive::Cone {
                        radius1,
                        radius2,
                        height,
                    } => vec![radius1, radius2, height],
                    Primitive::Torus {
                        major_radius,
                        minor_radius,
                    } => vec![major_radius, minor_radius],
                };
                format!("make {}{}", primitive.name(), bits(&values))
            }
            Step::Translate { input, offset } => format!("translate {input}{}", bits(&offset)),
            Step::Boolean { op, base, tool } => format!("{} {base} {tool}", boolean_name(op)),
        };
        let mut hash = Sha256::new();
        hash.update(b"silico recompute 1\n");
        hash.update(line.as_bytes());
        Key(hash.finalize().into())
    }

    fn inputs(&self) -> Vec<Key> {
        match *self {
            Step::Make(_) => Vec::new(),
            Step::Translate { input, .. } => vec![input],
            Step::Boolean { base, tool, .. } => vec![base, tool],
        }
    }
}

fn boolean_name(op: BooleanOp) -> &'static str {
    match op {
        BooleanOp::Fuse => "fuse",
        BooleanOp::Cut => "cut",
        BooleanOp::Common => "common",
    }
}

/// A document's steps, all keyed before anything runs.
struct Plan {
    /// Every step, by key: identical steps are one.
    steps: BTreeMap<Key, Step>,
    /// The steps in the order the timeline first uses them.
    order: Vec<Key>,
    features: Vec<Planned>,
}

struct Planned {
    id: FeatureId,
    /// Its own steps, in order; the last makes its solid.
    steps: Vec<Key>,
    /// The earlier features it uses, base then tool, by index.
    uses: Vec<usize>,
}

impl Planned {
    fn key(&self) -> Option<Key> {
        self.steps.last().copied()
    }
}

impl Plan {
    fn of(document: &Checked) -> Plan {
        let features = &document.document().features;
        let index: BTreeMap<&FeatureId, usize> = features
            .iter()
            .enumerate()
            .map(|(i, feature)| (&feature.id, i))
            .collect();
        let mut plan = Plan {
            steps: BTreeMap::new(),
            order: Vec::new(),
            features: Vec::with_capacity(features.len()),
        };
        for feature in features {
            let mut own = Vec::new();
            let mut uses = Vec::new();
            if let Some(primitive) = feature.operation.primitive() {
                let made = plan.add(Step::Make(primitive));
                own.push(made);
                // A primitive at the origin isn't moved.
                if let Some(position) = feature.operation.position()
                    && position.iter().any(|length| length.nm() != 0)
                {
                    own.push(plan.add(Step::Translate {
                        input: made,
                        offset: position.map(crate::document::Length::mm),
                    }));
                }
            }
            if let (Some(op), Some([(_, base), (_, tool)])) =
                (boolean_op(&feature.operation), feature.operation.inputs())
            {
                // The gate has checked that both are earlier features, so both are planned.
                let input = |id: &FeatureId| index.get(id).copied();
                if let (Some(b), Some(t)) = (input(base), input(tool)) {
                    uses = vec![b, t];
                    let key = |i: usize| plan.features.get(i).and_then(Planned::key);
                    if let (Some(base), Some(tool)) = (key(b), key(t)) {
                        own.push(plan.add(Step::Boolean { op, base, tool }));
                    }
                }
            }
            plan.features.push(Planned {
                id: feature.id.clone(),
                steps: own,
                uses,
            });
        }
        plan
    }

    fn add(&mut self, step: Step) -> Key {
        let key = step.key();
        if self.steps.insert(key, step).is_none() {
            self.order.push(key);
        }
        key
    }
}

fn boolean_op(operation: &Operation) -> Option<BooleanOp> {
    match operation {
        Operation::Fuse { .. } => Some(BooleanOp::Fuse),
        Operation::Cut { .. } => Some(BooleanOp::Cut),
        Operation::Common { .. } => Some(BooleanOp::Common),
        Operation::Box { .. }
        | Operation::Cylinder { .. }
        | Operation::Sphere { .. }
        | Operation::Cone { .. }
        | Operation::Torus { .. } => None,
    }
}

/// Whether a step's failure follows from its values, so asking again would fail again.
fn kept(error: &Error) -> bool {
    matches!(
        error.code(),
        ErrorCode::InvalidInput | ErrorCode::KernelFailed | ErrorCode::InvalidResult
    )
}

/// A recompute's steps in flight.
struct Schedule {
    /// Results so far: the kept ones it started with, and its own.
    done: BTreeMap<Key, StepResult>,
    /// For each step not yet run, how many of its inputs aren't built yet.
    waiting: BTreeMap<Key, usize>,
    /// The steps that use each step.
    users: BTreeMap<Key, Vec<Key>>,
    ready: VecDeque<Key>,
    running: usize,
    superseded: bool,
    /// A step that panicked, a bug in Silico: the recompute stops, and fails with this.
    broken: Option<Error>,
}

impl<K: Kernel + 'static> Engine<K> {
    fn run(&self, document: &Checked, generation: Generation) -> Result<RunOutcome, Error> {
        let plan = Plan::of(document);
        let done: BTreeMap<Key, StepResult> = {
            let state = lock(&self.state);
            plan.order
                .iter()
                .filter_map(|key| state.results.get(key).map(|r| (*key, r.clone())))
                .collect()
        };
        let mut schedule = Schedule {
            done,
            waiting: BTreeMap::new(),
            users: BTreeMap::new(),
            ready: VecDeque::new(),
            running: 0,
            superseded: false,
            broken: None,
        };
        for key in &plan.order {
            if schedule.done.contains_key(key) {
                continue;
            }
            let inputs = plan.steps.get(key).map(Step::inputs).unwrap_or_default();
            let mut unbuilt = 0;
            for input in inputs {
                match schedule.done.get(&input) {
                    Some(Ok(_)) => {}
                    // An input that failed: this step never runs.
                    Some(Err(_)) => unbuilt = usize::MAX,
                    None => {
                        unbuilt = unbuilt.saturating_add(1);
                        schedule.users.entry(input).or_default().push(*key);
                    }
                }
            }
            if unbuilt == 0 {
                schedule.ready.push_back(*key);
            } else {
                schedule.waiting.insert(*key, unbuilt);
            }
        }
        let to_run = schedule.ready.len() + schedule.waiting.len();
        tracing::debug!(steps = plan.order.len(), to_run, "recompute planned");
        let schedule = Mutex::new(schedule);
        let changed = Condvar::new();
        let threads = to_run.min(MOST_AT_ONCE);
        std::thread::scope(|scope| -> Result<(), Error> {
            let mut started = 0;
            for n in 0..threads {
                let spawned = std::thread::Builder::new()
                    .name(format!("recompute-{}-{n}", generation.0))
                    .spawn_scoped(scope, || {
                        self.serve(&plan, &schedule, &changed, generation);
                    });
                match spawned {
                    Ok(_) => started += 1,
                    // Run with the threads there are; with none, fail.
                    Err(e) if started == 0 => {
                        return Err(crate::pool::thread_error("recompute", &e));
                    }
                    Err(_) => break,
                }
            }
            Ok(())
        })?;
        let schedule = schedule
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(error) = schedule.broken {
            return Err(error);
        }
        self.keep(&schedule.done);
        if schedule.superseded {
            return Ok(RunOutcome::Superseded);
        }
        let model = Arc::new(model(&plan, &schedule.done, generation));
        let mut state = lock(&self.state);
        if state.generation != generation || state.cancelled {
            return Ok(RunOutcome::Superseded);
        }
        // Keep only what this document uses.
        state.results.retain(|key, _| plan.steps.contains_key(key));
        state.published = Some(Arc::clone(&model));
        Ok(RunOutcome::Published(model))
    }

    /// Keeps a recompute's results by key, but for failures that might not happen again.
    fn keep(&self, done: &BTreeMap<Key, StepResult>) {
        let mut state = lock(&self.state);
        for (key, result) in done {
            let keep = match result {
                Ok(_) => true,
                Err(error) => kept(error),
            };
            if keep {
                state.results.insert(*key, result.clone());
            }
        }
    }

    /// One of a recompute's threads: runs ready steps until none are left to run.
    fn serve(
        &self,
        plan: &Plan,
        schedule: &Mutex<Schedule>,
        changed: &Condvar,
        generation: Generation,
    ) {
        loop {
            let (key, inputs) = {
                let mut s = lock(schedule);
                let key = loop {
                    if s.superseded || s.broken.is_some() || (s.ready.is_empty() && s.running == 0)
                    {
                        changed.notify_all();
                        return;
                    }
                    if let Some(key) = s.ready.pop_front() {
                        break key;
                    }
                    s = changed.wait(s).unwrap_or_else(PoisonError::into_inner);
                };
                s.running += 1;
                let inputs: Vec<Arc<Built>> = plan
                    .steps
                    .get(&key)
                    .map(Step::inputs)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|input| match s.done.get(input) {
                        Some(Ok(built)) => Some(Arc::clone(built)),
                        _ => None,
                    })
                    .collect();
                (key, inputs)
            };
            // A panic under a step is a bug in Silico: caught, so the recompute ends with an
            // error rather than waiting forever on a step that will never finish.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match plan.steps.get(&key) {
                    Some(step) => {
                        let _span = tracing::debug_span!("recompute_step", key = %key).entered();
                        self.step(*step, &inputs, generation)
                    }
                    None => Outcome::Done(Err(Error::new(
                        ErrorCode::Internal,
                        "a recompute step wasn't planned",
                    )
                    .with_suggestion(crate::worker::SILICO_BUG))),
                }
            }));
            let mut s = lock(schedule);
            s.running -= 1;
            let Ok(outcome) = outcome else {
                s.broken = Some(
                    Error::new(ErrorCode::Internal, "a recompute step panicked")
                        .with_suggestion(crate::worker::SILICO_BUG),
                );
                drop(s);
                changed.notify_all();
                return;
            };
            match outcome {
                Outcome::Superseded => s.superseded = true,
                Outcome::Done(result) => {
                    if result.is_ok() {
                        for user in s.users.remove(&key).unwrap_or_default() {
                            let now_ready = s.waiting.get_mut(&user).is_some_and(|n| {
                                *n = n.saturating_sub(1);
                                *n == 0
                            });
                            if now_ready {
                                s.waiting.remove(&user);
                                s.ready.push_back(user);
                            }
                        }
                    }
                    s.done.insert(key, result.map(Arc::new));
                }
            }
            drop(s);
            changed.notify_all();
        }
    }

    /// Runs one step on the kernel, for `generation`.
    fn step(&self, step: Step, inputs: &[Arc<Built>], generation: Generation) -> Outcome<Built> {
        let shape = |i: usize| inputs.get(i).map(|built| &built.shape);
        let built = |data: BuiltData| Built {
            shape: data.shape,
            history: Some(data.history),
        };
        let missing = || {
            Outcome::Done(Err(Error::new(
                ErrorCode::Internal,
                "a recompute step ran before its inputs were built",
            )
            .with_suggestion(crate::worker::SILICO_BUG)))
        };
        match step {
            Step::Make(primitive) => match KernelRequest::make(primitive) {
                Ok(request) => map(self.kernel.submit(generation, request).wait(), |shape| {
                    Built {
                        shape,
                        history: None,
                    }
                }),
                Err(e) => Outcome::Done(Err(e)),
            },
            Step::Translate { offset, .. } => match shape(0) {
                Some(input) => match KernelRequest::translate(input, offset) {
                    Ok(request) => map(self.kernel.submit(generation, request).wait(), built),
                    Err(e) => Outcome::Done(Err(e)),
                },
                None => missing(),
            },
            Step::Boolean { op, .. } => match (shape(0), shape(1)) {
                (Some(base), Some(tool)) => map(
                    self.kernel
                        .submit(generation, KernelRequest::boolean(op, base, tool))
                        .wait(),
                    built,
                ),
                _ => missing(),
            },
        }
    }
}

fn map<T, U>(outcome: Outcome<T>, f: impl FnOnce(T) -> U) -> Outcome<U> {
    match outcome {
        Outcome::Done(result) => Outcome::Done(result.map(f)),
        Outcome::Superseded => Outcome::Superseded,
    }
}

/// What became of each feature, from its steps' results, and the bodies.
fn model(plan: &Plan, done: &BTreeMap<Key, StepResult>, generation: Generation) -> Model {
    let mut features: Vec<(FeatureId, FeatureOutcome)> = Vec::with_capacity(plan.features.len());
    for planned in &plan.features {
        // A feature it uses that didn't build blocks it, on the failure at the root.
        let blocked = planned.uses.iter().find_map(|&i| match features.get(i) {
            Some((id, FeatureOutcome::Failed(_))) => Some(id.clone()),
            Some((_, FeatureOutcome::Blocked { on })) => Some(on.clone()),
            Some((_, FeatureOutcome::Built { .. })) | None => None,
        });
        let outcome = match blocked {
            Some(on) => FeatureOutcome::Blocked { on },
            None => feature_outcome(planned, done),
        };
        features.push((planned.id.clone(), outcome));
    }
    let mut consumed = vec![false; features.len()];
    for (planned, (_, outcome)) in plan.features.iter().zip(&features) {
        if matches!(outcome, FeatureOutcome::Built { .. }) {
            for &i in &planned.uses {
                if let Some(c) = consumed.get_mut(i) {
                    *c = true;
                }
            }
        }
    }
    let bodies = features
        .iter()
        .zip(&consumed)
        .filter(|((_, outcome), consumed)| {
            !**consumed && matches!(outcome, FeatureOutcome::Built { .. })
        })
        .map(|((id, _), _)| id.clone())
        .collect();
    Model {
        generation,
        features,
        bodies,
    }
}

/// A feature whose inputs built: built, if its own steps did, or failed on the first that
/// didn't.
fn feature_outcome(planned: &Planned, done: &BTreeMap<Key, StepResult>) -> FeatureOutcome {
    let unbuilt = || {
        FeatureOutcome::Failed(
            Error::new(
                ErrorCode::Internal,
                "a feature's step wasn't run, though its inputs built",
            )
            .with_feature(planned.id.as_str())
            .with_suggestion(crate::worker::SILICO_BUG),
        )
    };
    for key in &planned.steps {
        match done.get(key) {
            Some(Ok(_)) => {}
            Some(Err(error)) => {
                return FeatureOutcome::Failed(error.clone().with_feature(planned.id.as_str()));
            }
            None => return unbuilt(),
        }
    }
    match planned.key().and_then(|key| match done.get(&key) {
        Some(Ok(built)) => Some((key, Arc::clone(built))),
        _ => None,
    }) {
        Some((key, built)) => FeatureOutcome::Built { key, built },
        None => unbuilt(),
    }
}
