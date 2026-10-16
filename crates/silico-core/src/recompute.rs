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
//! refuses fails its feature, and each feature using a failed (or blocked) one is blocked
//! on the failure at the root, as in SolidWorks; every other feature builds. A refusal that
//! follows from the step's values (`invalid_input`, `kernel_failed`, `invalid_result`) is
//! kept with its key, so it isn't asked again until something changes; anything else (a
//! crash, a timeout, a shortage) is tried again by the next recompute.
//!
//! Every recompute is a new generation of the kernel's ([`Kernel::advance`]), which
//! supersedes the one before: its steps waiting are dropped and those running stopped. A
//! recompute builds its [`Model`] aside and publishes it whole, only if no newer one has
//! started, so a superseded recompute leaves the published model as it was.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
    /// The newest recompute's generation, or a cancel's after it.
    generation: Generation,
    /// The newest recompute's generation, whether or not it was cancelled since.
    newest_started: Generation,
    /// Whether the newest recompute was cancelled when no newer generation was left to
    /// supersede it with: it publishes nothing.
    cancelled: bool,
    /// Every step result kept, by key: those the last [`PUBLISHED_KEPT`] published models'
    /// plans use, so going back to one (an undo) runs nothing, and those the newest
    /// recompute's uses, so a superseded one's work isn't lost to the next; nothing more, so
    /// a run of superseded recomputes holds no more than one's.
    results: BTreeMap<Key, StepResult>,
    published_plans: VecDeque<Arc<BTreeSet<Key>>>,
    newest_plan: Option<Arc<BTreeSet<Key>>>,
    published: Option<Arc<Model>>,
}

/// How many published models' results are kept.
const PUBLISHED_KEPT: usize = 2;

impl EngineState {
    /// Lets go of every result no plan worth keeping uses.
    fn trim(&mut self) {
        let EngineState {
            results,
            published_plans,
            newest_plan,
            ..
        } = self;
        results.retain(|key, _| {
            published_plans
                .iter()
                .chain(newest_plan.iter())
                .any(|plan| plan.contains(key))
        });
    }
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
                    newest_started: generation,
                    cancelled: false,
                    results: BTreeMap::new(),
                    published_plans: VecDeque::new(),
                    newest_plan: None,
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
            state.newest_started = Generation(next);
            state.cancelled = false;
            state.generation
        };
        self.engine.kernel.advance(generation);
        let engine = Arc::clone(&self.engine);
        // The caller's span, then the recompute's: should the recompute's be filtered out,
        // its work still traces back to the caller.
        let caller = tracing::Span::current();
        // At warning level, as the worker's request span is: a warning during a recompute,
        // at the default level, says which generation and step it was.
        let span = tracing::warn_span!("recompute", generation = generation.0);
        let started = std::thread::Builder::new()
            .name(format!("recompute-{}", generation.0))
            .spawn(move || {
                let _caller = caller.enter();
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
    /// The key: SHA-256 of a domain line (`silico recompute 1`) and a line saying what the
    /// step asks of the kernel, every number by its bits. Keys outlive a run (M2.5's cache
    /// keeps them on disk), so any change to what a step sends the kernel for the same
    /// values, or to this spelling, must change the domain line's number.
    fn key(&self) -> Key {
        let bits = |values: &[f64]| -> String {
            values
                .iter()
                .map(|v| format!(" {:016x}", v.to_bits()))
                .collect()
        };
        let line = match *self {
            Step::Make(primitive) => {
                format!("make {}{}", primitive.name(), bits(&primitive.values()))
            }
            Step::Translate { input, offset } => format!("translate {input}{}", bits(&offset)),
            Step::Boolean { op, base, tool } => format!("{} {base} {tool}", op.word()),
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

/// A document's steps, all keyed before anything runs.
struct Plan {
    /// Every step, by key: identical steps are one.
    steps: BTreeMap<Key, Step>,
    /// The steps in the order the timeline first uses them.
    order: Vec<Key>,
    /// The first feature, in timeline order, each step is one of, to name in spans and
    /// errors.
    first_feature: BTreeMap<Key, FeatureId>,
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
            first_feature: BTreeMap::new(),
            features: Vec::with_capacity(features.len()),
        };
        for feature in features {
            let mut own = Vec::new();
            let mut uses = Vec::new();
            if let Some(primitive) = feature.operation.primitive() {
                let made = plan.add(Step::Make(primitive), &feature.id);
                own.push(made);
                // A primitive at the origin isn't moved.
                if let Some(position) = feature.operation.position()
                    && position.iter().any(|length| length.nm() != 0)
                {
                    let offset = position.map(crate::document::Length::mm);
                    own.push(plan.add(
                        Step::Translate {
                            input: made,
                            offset,
                        },
                        &feature.id,
                    ));
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
                        own.push(plan.add(Step::Boolean { op, base, tool }, &feature.id));
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

    fn add(&mut self, step: Step, feature: &FeatureId) -> Key {
        let key = step.key();
        if self.steps.insert(key, step).is_none() {
            self.order.push(key);
            self.first_feature.insert(key, feature.clone());
        }
        key
    }

    /// The feature to name for step `key`.
    fn feature(&self, key: &Key) -> &str {
        self.first_feature.get(key).map_or("", FeatureId::as_str)
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
        let keys: Arc<BTreeSet<Key>> = Arc::new(plan.steps.keys().copied().collect());
        let done: BTreeMap<Key, StepResult> = {
            let mut state = lock(&self.state);
            // The newest recompute's plan is worth keeping results for, whether or not it
            // finishes: an older one finishing later keeps what this one will use.
            if state.newest_started == generation {
                state.newest_plan = Some(Arc::clone(&keys));
            }
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
        // Steps that can't run, since an input failed in an earlier recompute (and its
        // failure was kept): left out, and decided blocked or failed from `done`.
        let mut doomed: BTreeSet<Key> = BTreeSet::new();
        for key in &plan.order {
            if schedule.done.contains_key(key) {
                continue;
            }
            let inputs = plan.steps.get(key).map(Step::inputs).unwrap_or_default();
            let cant = inputs.iter().any(|input| {
                doomed.contains(input) || matches!(schedule.done.get(input), Some(Err(_)))
            });
            if cant {
                doomed.insert(*key);
                continue;
            }
            let mut unbuilt = 0;
            for input in inputs {
                if !schedule.done.contains_key(&input) {
                    unbuilt += 1;
                    schedule.users.entry(input).or_default().push(*key);
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
        // The recompute's span, for its threads' steps: so a worker's call traces back to
        // the recompute, and the edit that started it (ARCHITECTURE.md, invariant 9).
        let parent = tracing::Span::current();
        std::thread::scope(|scope| -> Result<(), Error> {
            let mut started = 0;
            for n in 0..threads {
                let spawned = std::thread::Builder::new()
                    .name(format!("recompute-{}-{n}", generation.0))
                    .spawn_scoped(scope, || {
                        let _entered = parent.enter();
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
        state.published = Some(Arc::clone(&model));
        state.published_plans.push_front(keys);
        state.published_plans.truncate(PUBLISHED_KEPT);
        state.trim();
        Ok(RunOutcome::Published(model))
    }

    /// Keeps a recompute's results by key, but for failures that might not happen again,
    /// and only what the plans worth keeping use: a superseded recompute's, if it's the
    /// newest, and the published ones'.
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
        state.trim();
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
                        let _span = tracing::warn_span!(
                            "recompute_step",
                            feature = plan.feature(&key),
                            key = %key
                        )
                        .entered();
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
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(panic) => {
                    let said = panic
                        .downcast_ref::<&str>()
                        .map(|said| (*said).to_owned())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "a panic with no message".to_owned());
                    s.broken = Some(
                        Error::new(ErrorCode::Internal, "a recompute step panicked")
                            .with_feature(plan.feature(&key))
                            .with_cause(said)
                            .with_suggestion(crate::worker::SILICO_BUG),
                    );
                    drop(s);
                    changed.notify_all();
                    return;
                }
            };
            let mut woken = 0;
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
                                woken += 1;
                            }
                        }
                    }
                    s.done.insert(key, result.map(Arc::new));
                }
            }
            // Wake a thread for each step made ready; every thread, once nothing is left.
            let finished = s.superseded || (s.ready.is_empty() && s.running == 0);
            drop(s);
            if finished {
                changed.notify_all();
            } else {
                for _ in 0..woken {
                    changed.notify_one();
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys outlive a run, so their spelling is pinned: computed by hand with Python's
    /// hashlib from the lines the key doc describes (`make box`, then each value's bits).
    #[test]
    fn keys_are_the_sha256_of_the_lines_their_doc_describes() {
        let made = Step::Make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .key();
        assert_eq!(
            made.to_string(),
            "2edae721daaca5b7e817c5038272f1e7a5a452e44971016e6c8946955742ab50"
        );
        let moved = Step::Translate {
            input: made,
            offset: [1.0, 0.0, 0.0],
        }
        .key();
        assert_eq!(
            moved.to_string(),
            "c703a0763043020655e2a22aac0d718ef001700d1ffa6a95c99d755c5212ecfd"
        );
        let cut = Step::Boolean {
            op: BooleanOp::Cut,
            base: made,
            tool: moved,
        }
        .key();
        assert_eq!(
            cut.to_string(),
            "d863527bc7862614802a5ffd30e2e8823da09e1791712803fac2111dd8048fe8"
        );
        // Every other primitive's and boolean's spelling, and its values' order.
        let pinned = [
            (
                Step::Make(Primitive::Cylinder {
                    radius: 2.5,
                    height: 12.0,
                }),
                "f0450ad96cbae6a87593abd22d4c343eb1d47883872603dc74ffcd68988908e6",
            ),
            (
                Step::Make(Primitive::Sphere { radius: 4.0 }),
                "8b714bde158cd45fc304d69ae5d95bdb193d9fbb0d1e61285e3d927facee4c9f",
            ),
            (
                Step::Make(Primitive::Cone {
                    radius1: 3.0,
                    radius2: 1.0,
                    height: 5.0,
                }),
                "accae08c48878692d0b897ce82088d897b9cb5d14991512161887b084faf3a47",
            ),
            (
                Step::Make(Primitive::Torus {
                    major_radius: 10.0,
                    minor_radius: 2.0,
                }),
                "b86b95e8bc83a6aa81808375a9e4b38519ed5d40e1fba42225b0dfef34b77d86",
            ),
            (
                Step::Boolean {
                    op: BooleanOp::Fuse,
                    base: made,
                    tool: made,
                },
                "49ea5738883c5cf746ad01391d1d996f9bc6c8cf5c4ff74288c4e6b6a83d4e12",
            ),
            (
                Step::Boolean {
                    op: BooleanOp::Common,
                    base: made,
                    tool: made,
                },
                "647ea820758b11dd1e81e732956d1af7490ffdb8cff3fd96f554327488614069",
            ),
        ];
        for (step, key) in pinned {
            assert_eq!(step.key().to_string(), key, "{step:?}");
        }
    }
}
