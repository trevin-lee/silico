//! Recompute: a checked document's features rebuilt on a [`Kernel`] (docs/ports/recompute.md).
//!
//! Each feature is one or more kernel steps (a primitive made at the origin, then moved to
//! its position; a boolean of two earlier features' solids; a pad's, pocket's, revolve's, or
//! groove's faces of its sketch's profile, their sweep, and its fuse or cut with its base:
//! docs/design/m3.6-sketch-features.md, S3), and each step has a [`Key`]: a hash of exactly
//! what it computes and of its inputs' keys, never of a feature's ID, name, note, or place
//! in the timeline. The keys are all known before anything runs, so a recompute runs only
//! the steps whose keys it has no result for: an unchanged document runs nothing, a rename
//! runs nothing, and a value edit runs that feature's steps and those below it. Identical
//! steps run once.
//!
//! A sweep's steps depend on its sketch's profile, so every sketch a feature sweeps is
//! solved, and its profile made, before the plan (both kept by the sketch's key, so an
//! unchanged sketch costs nothing); a sketch that doesn't solve blocks what sweeps it, and
//! one that makes no profile fails it with every refusal of its profile. Its checks before
//! any kernel step (several regions, a revolve's axis) and of its result (one solid, a
//! change of volume, measured as its fuse or cut is made: `sweep`) fail it as a step's
//! refusal does.
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
//!
//! Every sketch is solved too, from the coordinates its document stores
//! (docs/design/m3.4-stored-coordinates.md), on the engine's [`SketchSolver`] (the sketch
//! worker, started at the first sketch, unless [`Recompute::with_sketches`] or
//! [`Recompute::with_sketch_starter`] gave another),
//! one at a time: those a feature sweeps before the plan, the rest beside the kernel's
//! steps. A solve is keyed by its feature's ID and the
//! sketch's canonical text, and kept as steps' results are, so an unchanged sketch isn't
//! solved again; a superseded recompute's solve is stopped.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::cache::Cache;
use crate::document::{
    Checked, Document, ElementId, ElementName, FeatureId, Fingerprint, Made, Operation,
    SweepFeature,
};
use crate::kernel::{
    BooleanOp, BuiltData, FaceRegions, Generation, History, Kernel, KernelRequest, Primitive,
    ShapeData, ShapeSummary, SweepKind,
};
use crate::naming::{ElementMap, Maker, NAMING_SCHEME, OpCode};
use crate::pool::Outcome;
use crate::profile::Profile;
use crate::references::EdgeNames;
use crate::sketch::{SketchSolution, SketchSolver, SketchWorker, SolveStopper};
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
        f.write_str(&crate::hex(&self.0))
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({self})")
    }
}

/// What a kernel step made: its shape, for an operation on other shapes what it did to
/// them, and, if it was measured, the shape's summary (from which a feature's fingerprint
/// comes; a step that's a feature's solid is measured after it's made, by a job of its own,
/// which puts a measured copy in its place), or why the kernel refused to measure it, if
/// that follows from its values (so it would refuse again). Never changed once made: models
/// share it.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub shape: ShapeData,
    pub history: Option<History>,
    summary: Option<ShapeSummary>,
    refused: Option<Error>,
}

impl Built {
    pub(crate) fn new(
        shape: ShapeData,
        history: Option<History>,
        summary: Option<ShapeSummary>,
    ) -> Self {
        Built {
            shape,
            history,
            summary,
            refused: None,
        }
    }

    /// A result the kernel refused to measure, for its values, as `refused` says.
    pub(crate) fn refused_measure(
        shape: ShapeData,
        history: Option<History>,
        refused: Error,
    ) -> Self {
        Built {
            shape,
            history,
            summary: None,
            refused: Some(refused),
        }
    }

    /// The shape's summary, if it was measured.
    pub fn summary(&self) -> Option<&ShapeSummary> {
        self.summary.as_ref()
    }

    /// Why the kernel refused to measure the shape, if it did for its values.
    pub fn measure_refused(&self) -> Option<&Error> {
        self.refused.as_ref()
    }

    /// This result, measured.
    fn measured(&self, summary: ShapeSummary) -> Self {
        Built {
            summary: Some(summary),
            ..self.clone()
        }
    }

    /// This result, the kernel having refused to measure it for its values.
    fn with_refused_measure(&self, error: Error) -> Self {
        Built {
            refused: Some(error),
            ..self.clone()
        }
    }
}

/// What became of a feature in a recompute.
#[derive(Debug, Clone, PartialEq)]
pub enum FeatureOutcome {
    /// Built: its last step's key and result.
    Built { key: Key, built: Arc<Built> },
    /// The kernel refused one of its steps: why, naming the feature.
    Failed(Error),
    /// References it holds (a fillet's or chamfer's edges, by name) name nothing it can
    /// use: each broken one's `reference_broken` error, in the order the feature holds them
    /// (N6: every one, at once), the first [`PROBLEMS_LISTED`](crate::document::PROBLEMS_LISTED)
    /// of them, and how many there are, listed or not. Its step isn't run. Or the sketch a
    /// pad, pocket, revolve, or groove sweeps solved but makes no profile: each of the
    /// profile's refusals, an `invalid_input` naming the sketch and its elements, listed and
    /// counted alike (docs/design/m3.6-sketch-features.md, S3).
    Broken { errors: Vec<Error>, total: u64 },
    /// Not built, since it uses a feature that failed: that feature (the first failure
    /// along its base, then its tool).
    Blocked { on: FeatureId },
    /// A sketch, solved: it makes no solid. One that didn't solve is `Failed`, with why.
    Sketch(Arc<SketchSolution>),
}

/// A document as rebuilt: what became of each feature, in timeline order, and its bodies.
#[derive(Debug, Clone)]
pub struct Model {
    /// The document it was built from.
    source: Arc<Checked>,
    generation: Generation,
    features: Vec<(FeatureId, FeatureOutcome)>,
    bodies: Vec<FeatureId>,
    /// Built features whose solids couldn't be measured, in timeline order: why.
    unmeasured: Vec<(FeatureId, Error)>,
    /// How each feature's solid's elements are named, in timeline order: none for one that
    /// didn't build, or a sketch, which makes no solid.
    naming: Vec<Option<FeatureNaming>>,
    /// Each feature's element names: named in the background once the model is published
    /// (after its recompute has replied, so publishing doesn't wait for them: naming every
    /// element of every feature before replying doubled the cost of a long timeline on the
    /// fake), or, asked for sooner, then, with those of the features it's made from. They're
    /// a function of the model's own data alone, so they're the same whenever they're
    /// named.
    maps: Vec<OnceLock<Option<Arc<ElementMap>>>>,
    /// The engine's maps named so far.
    known: NamedMaps,
}

/// Element maps named so far, by naming key: shared by the engine and the models it
/// publishes, so a feature named alike in an earlier model isn't named again.
type NamedMaps = Arc<Mutex<BTreeMap<NamingKey, Arc<ElementMap>>>>;

/// How a feature's solid's elements are named, and what from.
#[derive(Debug, Clone)]
struct FeatureNaming {
    key: NamingKey,
    how: Naming,
    /// The features whose solids it's made from, by index.
    uses: Vec<usize>,
    /// Its own steps' results, in order: the last its solid, and a sweep feature's faces
    /// and sweep before it, which it's named from too.
    steps: Vec<Arc<Built>>,
}

/// Models are equal when they're of the same document, built in the same generation, and
/// every feature came to the same: names are a function of those.
impl PartialEq for Model {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.generation == other.generation
            && self.features == other.features
            && self.bodies == other.bodies
            && self.unmeasured == other.unmeasured
    }
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

    /// How many errors the failed features have that [`Model::errors`] doesn't list: a
    /// feature's broken references past its first hundred.
    pub fn unlisted_errors(&self) -> u64 {
        self.features
            .iter()
            .map(|(_, outcome)| match outcome {
                FeatureOutcome::Broken { errors, total } => {
                    total.saturating_sub(errors.len() as u64)
                }
                FeatureOutcome::Built { .. }
                | FeatureOutcome::Failed(_)
                | FeatureOutcome::Blocked { .. }
                | FeatureOutcome::Sketch(_) => 0,
            })
            .sum()
    }

    /// How many errors the failed features have, listed in [`Model::errors`] or not: one
    /// for a failure, and every broken reference of a feature whose references broke.
    pub fn error_count(&self) -> u64 {
        self.errors().count() as u64 + self.unlisted_errors()
    }

    /// Why each failed feature failed, in timeline order, each error naming its feature:
    /// one for a failure, and one for each broken reference of a feature whose references
    /// broke (its first hundred: [`Model::error_count`] counts them all).
    pub fn errors(&self) -> impl Iterator<Item = &Error> {
        self.features.iter().flat_map(|(_, outcome)| match outcome {
            FeatureOutcome::Failed(error) => std::slice::from_ref(error),
            FeatureOutcome::Broken { errors, .. } => errors.as_slice(),
            FeatureOutcome::Built { .. }
            | FeatureOutcome::Blocked { .. }
            | FeatureOutcome::Sketch(_) => &[],
        })
    }

    /// The names of the elements of the feature `id`'s solid, if it built. The recompute
    /// that published the model names every feature's elements in the background once
    /// it's published; asked before then, this names the feature's now, on the caller's
    /// thread, with those of the features it's made from (and no others).
    pub fn element_map(&self, id: &FeatureId) -> Option<&ElementMap> {
        let i = self.index(id)?;
        // Those it's made from, all the way down, in timeline order, so each is named after
        // those it's made from. An explicit stack: a timeline can be thousands deep.
        let mut needed = std::collections::BTreeSet::new();
        let mut stack = vec![i];
        while let Some(j) = stack.pop() {
            if needed.insert(j)
                && let Some(Some(naming)) = self.naming.get(j)
            {
                stack.extend(naming.uses.iter().copied());
            }
        }
        for j in needed {
            if let Some(slot) = self.maps.get(j) {
                slot.get_or_init(|| self.name(j));
            }
        }
        self.maps.get(i)?.get()?.as_deref()
    }

    /// The names of the elements of the feature `id`'s solid if they've been named
    /// already, without naming them, so a caller that mustn't wait (the UI's thread)
    /// never does.
    pub fn named_element_map(&self, id: &FeatureId) -> Option<&ElementMap> {
        self.maps.get(self.index(id)?)?.get()?.as_deref()
    }

    /// Names every feature's elements not yet named, in timeline order, while `go_on` says
    /// to.
    fn name_all(&self, go_on: impl Fn() -> bool) {
        for (j, slot) in self.maps.iter().enumerate() {
            if !go_on() {
                return;
            }
            slot.get_or_init(|| self.name(j));
        }
    }

    fn index(&self, id: &FeatureId) -> Option<usize> {
        self.features.iter().position(|(feature, _)| feature == id)
    }

    /// The `j`th feature's element names, those it's made from named already: from the maps
    /// known by its naming key, or named from its history.
    fn name(&self, j: usize) -> Option<Arc<ElementMap>> {
        let naming = self.naming.get(j)?.as_ref()?;
        if let Some(map) = lock(&self.known).get(&naming.key) {
            return Some(Arc::clone(map));
        }
        let (id, _) = self.features.get(j)?;
        let inputs: Option<Vec<&ElementMap>> = naming
            .uses
            .iter()
            .map(|&u| self.maps.get(u)?.get()?.as_deref())
            .collect();
        let map = named_as(id, &naming.how, &inputs?, &naming.steps, &self.known)?;
        Some(kept_in(&self.known, naming.key, map))
    }

    /// The document it was built from.
    pub fn document(&self) -> &Document {
        self.source.document()
    }

    /// The fingerprint of the feature `id`'s solid, if it built and was measured: none if it
    /// didn't (or the document has no such feature), or if a measure can't be written.
    pub fn fingerprint(&self, id: &FeatureId) -> Option<Fingerprint> {
        match self.outcome(id)? {
            FeatureOutcome::Built { built, .. } => built.summary().and_then(Fingerprint::of),
            FeatureOutcome::Failed(_)
            | FeatureOutcome::Broken { .. }
            | FeatureOutcome::Blocked { .. }
            | FeatureOutcome::Sketch(_) => None,
        }
    }

    /// Each feature of its document whose record doesn't hold, in timeline order: one
    /// whose solid couldn't be measured (that error, naming it); one whose rebuild measures
    /// otherwise than the fingerprint stored with it (a `drift` error naming the feature
    /// and what moved); and a sketch whose solve differs from what its file holds (a
    /// `drift` error naming the sketch and, as elements, every point, circle, and reference
    /// dimension the solve changes). A feature with no fingerprint stored isn't drift (the
    /// next save writes one), nor is one that didn't build or solve (its failure says why).
    pub fn drift(&self) -> Vec<Error> {
        let mut errors = Vec::new();
        let mut unmeasured = self.unmeasured.iter().peekable();
        for (feature, (_, outcome)) in self.document().features.iter().zip(&self.features) {
            if let (Operation::Sketch(stored), FeatureOutcome::Sketch(solution)) =
                (&feature.operation, outcome)
            {
                let moved = crate::sketch::moved(stored, &solution.written(stored));
                if let Some((first, _)) = moved.first() {
                    let said: Vec<&str> = moved.iter().take(5).map(|(_, s)| s.as_str()).collect();
                    let more = if moved.len() > 5 { ", …" } else { "" };
                    errors.push(
                        Error::new(
                            ErrorCode::Drift,
                            format!(
                                "sketch {} solves to other values than its file holds: {}{more} ({} in all)",
                                feature.id,
                                said.join(", "),
                                moved.len()
                            ),
                        )
                        .with_feature(feature.id.as_str())
                        .with_elements(moved.iter().map(|(id, _)| id.as_str()))
                        .with_parameter(first.as_str())
                        .with_suggestion(SKETCH_DRIFT),
                    );
                }
                continue;
            }
            if let Some((_, error)) = unmeasured.next_if(|(id, _)| *id == feature.id) {
                errors.push(error.clone());
                continue;
            }
            let (Some(stored), FeatureOutcome::Built { built, .. }) =
                (&feature.fingerprint, outcome)
            else {
                continue;
            };
            let Some(summary) = built.summary() else {
                continue;
            };
            let error = match Fingerprint::of(summary) {
                Some(rebuilt) => {
                    let differences = stored.differences(&rebuilt);
                    let Some(first) = differences.first() else {
                        continue;
                    };
                    let moved: Vec<String> = differences
                        .iter()
                        .map(|d| format!("{} {} → {}", d.field, d.stored, d.rebuilt))
                        .collect();
                    Error::new(
                        ErrorCode::Drift,
                        format!(
                            "feature {} rebuilt to other measures than its fingerprint: {}",
                            feature.id,
                            moved.join(", ")
                        ),
                    )
                    .with_parameter(format!("fingerprint.{}", first.field))
                }
                None => Error::new(
                    ErrorCode::Drift,
                    format!(
                        "feature {} rebuilt to measures a fingerprint can't hold (not finite, or too large)",
                        feature.id
                    ),
                )
                .with_parameter("fingerprint"),
            };
            errors.push(
                error
                    .with_feature(feature.id.as_str())
                    .with_suggestion(DRIFT),
            );
        }
        errors
    }

    /// Its document as a save writes it: each feature's fingerprint as it rebuilt, and each
    /// sketch as it solved (its points, circles' radii, and reference dimensions' values:
    /// [`SketchSolution::written`]). A feature that didn't rebuild or solve, or whose solid
    /// couldn't be measured or written, keeps what's stored with it: there's nothing to put
    /// in its place, and a failure that passes (a worker that hung, say) mustn't lose the
    /// record a later rebuild is checked against.
    pub fn saved(&self) -> Document {
        let mut document = self.document().clone();
        // The model's features are its document's, in order.
        for (feature, (_, outcome)) in document.features.iter_mut().zip(&self.features) {
            match outcome {
                FeatureOutcome::Built { built, .. } => {
                    if let Some(rebuilt) = built.summary().and_then(Fingerprint::of) {
                        feature.fingerprint = Some(rebuilt);
                    }
                }
                FeatureOutcome::Sketch(solution) => {
                    if let Operation::Sketch(sketch) = &mut feature.operation {
                        *sketch = solution.written(sketch);
                    }
                }
                FeatureOutcome::Failed(_)
                | FeatureOutcome::Broken { .. }
                | FeatureOutcome::Blocked { .. } => {}
            }
        }
        document
    }
}

/// What to do about drift: most often the feature, or one it uses, was edited (by hand, or
/// in a merge) since it was saved; otherwise the kernel's results changed.
const DRIFT: &str = "if this feature or one it uses was edited by hand or in a merge since it was saved, save the document (or run `silico fmt`) to write its new fingerprint; if not, this is a determinism bug or a change in the kernel: find which, and once the new measures are right, save to write them";

/// What to do about a sketch's drift: as for a fingerprint's ([`DRIFT`]), for a sketch.
const SKETCH_DRIFT: &str = "if this sketch was edited by hand or in a merge since it was saved, save the document (or run `silico fmt`) to write its solved coordinates; if not, this is a determinism bug or a change in the sketch solver: find which, and once the new solve is right, save to write it";

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
    /// Where results are kept on disk, if anywhere.
    cache: Option<Cache>,
    state: Mutex<EngineState>,
    /// Signalled when the sketch solver is given back, or a recompute starts or is
    /// cancelled: a recompute waiting for the solver wakes to take it, or to find it's
    /// superseded and wait no longer.
    sketch_turn: Condvar,
    /// Starts the sketch solver for the first sketch: the sketch worker, unless
    /// [`Recompute::with_sketch_starter`] gave another. Called only by the recompute the
    /// solver is lent to.
    start_solver: Mutex<SolverStarter>,
}

/// Starts a sketch solver.
type SolverStarter = Box<dyn FnMut() -> Result<Box<dyn SketchSolver>, Error> + Send>;

/// The engine's sketch solver: one recompute's at a time, for all its sketches.
enum Solver {
    /// None yet: the first sketch starts one with the engine's starter (the sketch worker,
    /// unless [`Recompute::with_sketch_starter`] gave another).
    Unstarted,
    Here(Box<dyn SketchSolver>),
    /// A recompute's sketch thread has it.
    Lent,
}

/// The solver, lent to a recompute's sketch thread: given back when it's dropped, even by
/// a panic under a solve, so the next recompute doesn't wait for it forever.
struct LentSolver<'a> {
    state: &'a Mutex<EngineState>,
    turn: &'a Condvar,
    solver: Option<Box<dyn SketchSolver>>,
}

impl Drop for LentSolver<'_> {
    fn drop(&mut self) {
        let mut state = lock(self.state);
        // A solve that panicked left its stop handle: nothing of it runs now.
        state.solving = None;
        state.solver = match self.solver.take() {
            Some(solver) => Solver::Here(solver),
            None => Solver::Unstarted,
        };
        drop(state);
        self.turn.notify_all();
    }
}

struct EngineState {
    /// The newest recompute's generation, or a cancel's after it.
    generation: Generation,
    /// The newest recompute's generation, whether or not it was cancelled since.
    newest_started: Generation,
    /// Whether the newest recompute was cancelled when no newer generation was left to
    /// supersede it with: it publishes nothing.
    cancelled: bool,
    /// Whether the engine has been dropped: the published model's background naming stops.
    dropped: bool,
    /// Every step result kept, by key: those the last [`PUBLISHED_KEPT`] published models'
    /// plans use, so going back to one (an undo) runs nothing, and those the newest
    /// recompute's uses, so a superseded one's work isn't lost to the next; nothing more, so
    /// a run of superseded recomputes holds no more than one's.
    results: BTreeMap<Key, StepResult>,
    /// Each sketch solved, by its key, kept as step results are.
    solutions: BTreeMap<Key, SketchResult>,
    /// The profile of each sketch a feature sweeps, by its solve's key, kept as solves are.
    profiles: BTreeMap<Key, ProfileResult>,
    solver: Solver,
    /// The sketch being solved, and how to stop it once a newer recompute starts.
    solving: Option<Solving>,
    /// How many solves have been started: each solve's number.
    solves: u64,
    published_plans: VecDeque<Arc<BTreeSet<Key>>>,
    newest_plan: Option<Arc<BTreeSet<Key>>>,
    published: Option<Arc<Model>>,
    /// Maps named so far, kept for the naming keys the last [`PUBLISHED_KEPT`] published
    /// models have (`published_keys`).
    known: NamedMaps,
    published_keys: VecDeque<BTreeSet<NamingKey>>,
}

/// How many published models' results are kept.
const PUBLISHED_KEPT: usize = 2;

impl EngineState {
    /// Lets go of every result no plan worth keeping uses.
    fn trim(&mut self) {
        let EngineState {
            results,
            solutions,
            profiles,
            published_plans,
            newest_plan,
            ..
        } = self;
        let used = |key: &Key| {
            published_plans
                .iter()
                .chain(newest_plan.iter())
                .any(|plan| plan.contains(key))
        };
        results.retain(|key, _| used(key));
        solutions.retain(|key, _| used(key));
        profiles.retain(|key, _| used(key));
    }

    /// Stops the sketch solve running, if a recompute older than `generation` is running
    /// it, once it has run [`SKETCH_STOP_AFTER`]: now, if it has; otherwise this says how
    /// much longer it may run, and which solve, for [`Engine::stop_later`].
    fn stop_older_solve(&mut self, generation: Generation) -> Option<(Duration, u64)> {
        let solving = self.solving.as_ref()?;
        if solving.generation.0 >= generation.0 {
            return None;
        }
        let ran = solving.started.elapsed();
        if ran < SKETCH_STOP_AFTER {
            return Some((SKETCH_STOP_AFTER - ran, solving.number));
        }
        self.stop_solve();
        None
    }

    /// Stops the sketch solve running, if any, now.
    fn stop_solve(&mut self) {
        if let Some(solving) = self.solving.take() {
            solving.stopper.stop();
        }
    }
}

/// Starts up to `threads` of a recompute's threads, `spawn` starting the `n`th: as many as
/// start, up to the first that doesn't, since a recompute runs with the threads there are;
/// with none, why.
fn start_threads(
    threads: usize,
    mut spawn: impl FnMut(usize) -> std::io::Result<()>,
) -> Result<usize, Error> {
    let mut started = 0;
    for n in 0..threads {
        match spawn(n) {
            Ok(()) => started += 1,
            Err(e) if started == 0 => return Err(crate::pool::thread_error("recompute", &e)),
            Err(_) => break,
        }
    }
    Ok(started)
}

/// A sketch solve running: its recompute's generation, its number among the engine's
/// solves, when it started, and how to stop it.
struct Solving {
    generation: Generation,
    number: u64,
    started: std::time::Instant,
    stopper: SolveStopper,
}

/// How long a superseded sketch solve may run before it's stopped, as ARCHITECTURE.md's
/// rule for superseded work has it (stopped once it has run longer than replacing its
/// worker takes, unless it finishes first): starting a sketch worker again and solving
/// took 6 to 8 ms (the council's skeptic, unloaded). A kernel worker takes about 30 ms to
/// replace, and the pool's grace is `pool::STOP_AFTER`, 50 ms.
pub const SKETCH_STOP_AFTER: Duration = Duration::from_millis(10);

type StepResult = Result<Arc<Built>, Error>;
type SketchResult = Result<Arc<SketchSolution>, Error>;
/// A recompute's sketches' results, by key.
type SketchResults = BTreeMap<Key, SketchResult>;

/// A sketch's result in a recompute: its solve's, or the solver's failure, not the
/// sketch's (a worker that couldn't start), which isn't kept, so the next recompute tries
/// again.
enum SketchOutcome {
    Solved(SketchResult),
    SolverFailed(Error),
}

impl SketchOutcome {
    fn result(self) -> SketchResult {
        match self {
            SketchOutcome::Solved(result) => result,
            SketchOutcome::SolverFailed(error) => Err(error),
        }
    }
}

/// What a recompute's sketch thread came to, by key.
type SketchRun = BTreeMap<Key, SketchOutcome>;

/// A failure of the sketch solver, not the sketch: as the kernel's own failures, a fault
/// a client reports as its own (`check` and `fmt` exit 2), not the document's. A missing
/// or misnamed worker comes as `invalid_input` from finding it, which would blame the sketch
/// (the council: `check` exited 1, and `fmt` wrote the file); it's kept as the cause.
fn solver_failure(error: Error, feature: &FeatureId) -> SketchOutcome {
    let error = if error.code().is_kernel_fault() {
        error
    } else {
        let mut wrapped = Error::new(
            ErrorCode::WorkerCrashed,
            "the sketch worker couldn't be started",
        )
        .with_cause(error.message().to_owned());
        if let Some(suggestion) = error.suggestion() {
            wrapped = wrapped.with_suggestion(suggestion.to_owned());
        }
        wrapped
    };
    SketchOutcome::SolverFailed(error.with_feature(feature.as_str()))
}

impl<K: Kernel + 'static> Recompute<K> {
    pub fn new(kernel: K) -> Self {
        Self::build(kernel, None)
    }

    /// Rebuilds on `kernel`, keeping each step's result in `cache` too, and taking it from
    /// there when it's kept: a document opened again rebuilds without the kernel.
    pub fn with_cache(kernel: K, cache: Cache) -> Self {
        Self::build(kernel, Some(cache))
    }

    /// Solves sketches with `sketches` rather than the sketch worker (a stand-in, in tests).
    pub fn with_sketches(self, sketches: impl SketchSolver + 'static) -> Self {
        lock(&self.engine.state).solver = Solver::Here(Box::new(sketches));
        self
    }

    /// Starts the sketch solver with `start` at the first sketch, rather than the sketch
    /// worker (a stand-in that fails to start, in tests). It replaces a solver given with
    /// [`with_sketches`](Self::with_sketches): whichever is called last holds.
    pub fn with_sketch_starter(
        self,
        start: impl FnMut() -> Result<Box<dyn SketchSolver>, Error> + Send + 'static,
    ) -> Self {
        lock(&self.engine.state).solver = Solver::Unstarted;
        *lock(&self.engine.start_solver) = Box::new(start);
        self
    }

    fn build(kernel: K, cache: Option<Cache>) -> Self {
        let generation = kernel.generation();
        Self {
            engine: Arc::new(Engine {
                kernel,
                cache,
                sketch_turn: Condvar::new(),
                start_solver: Mutex::new(Box::new(|| {
                    SketchWorker::start().map(|worker| Box::new(worker) as Box<dyn SketchSolver>)
                })),
                state: Mutex::new(EngineState {
                    generation,
                    newest_started: generation,
                    cancelled: false,
                    dropped: false,
                    results: BTreeMap::new(),
                    solutions: BTreeMap::new(),
                    profiles: BTreeMap::new(),
                    solver: Solver::Unstarted,
                    solving: None,
                    solves: 0,
                    published_plans: VecDeque::new(),
                    newest_plan: None,
                    published: None,
                    known: Arc::default(),
                    published_keys: VecDeque::new(),
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
        let (generation, later) = {
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
            let later = state.stop_older_solve(Generation(next));
            (state.generation, later)
        };
        if let Some((after, number)) = later {
            Engine::stop_later(&self.engine, after, number);
        }
        // A superseded recompute waiting for the sketch solver stops waiting.
        self.engine.sketch_turn.notify_all();
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
                let outcome = engine.run(&Arc::new(document), generation);
                let published = match &outcome {
                    Ok(RunOutcome::Published(model)) => Some(Arc::clone(model)),
                    _ => None,
                };
                let _ = reply.send(outcome);
                // The model's names, in the background, so a client asking for them later
                // doesn't wait; stopping once a newer model is published (it's the one to
                // name) or the engine is dropped, but not for a cancel or a newer recompute
                // that publishes nothing, which leave this model the one clients have.
                if let Some(model) = published {
                    let _naming = tracing::debug_span!("name_elements").entered();
                    model.name_all(|| {
                        let state = lock(&engine.state);
                        !state.dropped
                            && state
                                .published
                                .as_ref()
                                .is_some_and(|newest| Arc::ptr_eq(newest, &model))
                    });
                }
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
        self.supersede(false);
    }

    /// Supersedes the recompute running. Its sketch solve, if it's solving one, is stopped
    /// as superseded work is (ARCHITECTURE.md: once it has run [`SKETCH_STOP_AFTER`]); or,
    /// `at_once`, now (a dropped engine: nothing is left to stop it later, and the process
    /// may be ending).
    fn supersede(&self, at_once: bool) {
        let mut state = lock(&self.engine.state);
        let next = state.generation.0.checked_add(1);
        let later = match next {
            Some(next) => {
                state.generation = Generation(next);
                if at_once {
                    state.stop_solve();
                    None
                } else {
                    state.stop_older_solve(Generation(next))
                }
            }
            // No newer generation to stop its steps with: they run on, but it publishes
            // nothing; its solve, if it's solving a sketch, is stopped.
            None => {
                state.cancelled = true;
                state.stop_solve();
                None
            }
        };
        drop(state);
        if let Some((after, number)) = later {
            Engine::stop_later(&self.engine, after, number);
        }
        self.engine.sketch_turn.notify_all();
        if let Some(next) = next {
            self.engine.kernel.advance(Generation(next));
        }
    }
}

impl<K: Kernel + 'static> Drop for Recompute<K> {
    fn drop(&mut self) {
        lock(&self.engine.state).dropped = true;
        self.supersede(true);
    }
}

/// What a step computes.
#[derive(Debug, Clone)]
enum Step {
    Make(Primitive),
    Translate {
        input: Key,
        offset: [f64; 3],
    },
    Boolean {
        op: BooleanOp,
        base: Key,
        tool: Key,
    },
    /// A sketch's profile's faces, as `KernelRequest::make_faces` sends them (design S3):
    /// no inputs, and a history of one, the regions.
    Faces(Arc<FaceRegions>),
    /// `faces` moved by `start`, then swept along `vector`.
    Prism {
        faces: Key,
        start: [f64; 3],
        vector: [f64; 3],
    },
    /// `faces` swept along `direction` through all of `base`, which only sizes it: a
    /// history of one input, the faces.
    PrismThroughAll {
        faces: Key,
        base: Key,
        direction: [f64; 3],
    },
    /// `faces` turned `start` radians about the axis through `origin` along `direction`,
    /// then swept `angle` radians about it.
    Revolution {
        faces: Key,
        origin: [f64; 3],
        direction: [f64; 3],
        start: f64,
        angle: f64,
    },
    /// A fillet or chamfer of `base`'s solid: of the edges its feature names, which the
    /// plan holds (`Plan::dresses`), found by name in the base feature's element map when
    /// the step runs. `names` is the digest of what decides which edges those are: the
    /// base feature's naming key and the names as written.
    Dress {
        dressing: Dressing,
        base: Key,
        names: [u8; 32],
    },
}

/// What a fillet or chamfer does to each edge.
#[derive(Debug, Clone, Copy)]
enum Dressing {
    Fillet { radius: f64 },
    Chamfer { distance: f64 },
}

/// A fillet's or chamfer's edges, as its feature names them, and the feature whose solid's
/// elements they name.
#[derive(Debug, Clone)]
struct Dress {
    /// The fillet's or chamfer's own feature, by index.
    feature: usize,
    /// Its base feature, by index.
    base: usize,
    edges: Vec<ElementName>,
}

/// What a feature's element map is named from, for a fillet or chamfer to read its base's:
/// the map, if it's named already, or the feature's solid, to name it from (its inputs'
/// maps first).
enum MapSource {
    Named(Arc<ElementMap>),
    /// Its own steps' results, in order.
    Built(Vec<Arc<Built>>),
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
        let line = match self {
            Step::Make(primitive) => {
                format!("make {}{}", primitive.name(), bits(&primitive.values()))
            }
            Step::Translate { input, offset } => format!("translate {input}{}", bits(offset)),
            Step::Boolean { op, base, tool } => format!("{} {base} {tool}", op.word()),
            Step::Faces(regions) => faces_line(regions),
            Step::Prism {
                faces,
                start,
                vector,
            } => format!("prism {faces}{}{}", bits(start), bits(vector)),
            Step::PrismThroughAll {
                faces,
                base,
                direction,
            } => format!("prism-through-all {faces} {base}{}", bits(direction)),
            Step::Revolution {
                faces,
                origin,
                direction,
                start,
                angle,
            } => format!(
                "revolution {faces}{}{}{}",
                bits(origin),
                bits(direction),
                bits(&[*start, *angle])
            ),
            Step::Dress {
                dressing,
                base,
                names,
            } => match *dressing {
                Dressing::Fillet { radius } => {
                    format!("fillet {base} {}{}", crate::hex(names), bits(&[radius]))
                }
                Dressing::Chamfer { distance } => {
                    format!("chamfer {base} {}{}", crate::hex(names), bits(&[distance]))
                }
            },
        };
        let mut hash = Sha256::new();
        hash.update(b"silico recompute 1\n");
        hash.update(line.as_bytes());
        Key(hash.finalize().into())
    }

    /// The steps whose results it takes, in order.
    fn inputs(&self) -> Vec<Key> {
        match *self {
            Step::Make(_) | Step::Faces(_) => Vec::new(),
            Step::Translate { input, .. } => vec![input],
            Step::Boolean { base, tool, .. } => vec![base, tool],
            Step::Dress { base, .. } => vec![base],
            Step::Prism { faces, .. } | Step::Revolution { faces, .. } => vec![faces],
            // The base only sizes the prism.
            Step::PrismThroughAll { faces, base, .. } => vec![faces, base],
        }
    }

    /// How many inputs its result's history has: none for a primitive, which has no
    /// history; one for faces (the regions), which take no step's result; one for a
    /// through-all prism (its faces), whose base only sizes it.
    fn history_inputs(&self) -> usize {
        match self {
            Step::Make(_) => 0,
            Step::Boolean { .. } => 2,
            Step::Translate { .. }
            | Step::Dress { .. }
            | Step::Faces(_)
            | Step::Prism { .. }
            | Step::PrismThroughAll { .. }
            | Step::Revolution { .. } => 1,
        }
    }

    /// The sweep it is, for the rule its history keeps (`History::sweep_fault`).
    fn sweep_kind(&self) -> Option<SweepKind> {
        match *self {
            Step::Prism { .. } | Step::PrismThroughAll { .. } => Some(SweepKind::Prism),
            Step::Revolution { angle, .. } => Some(SweepKind::of_turn(angle)),
            Step::Make(_)
            | Step::Translate { .. }
            | Step::Boolean { .. }
            | Step::Dress { .. }
            | Step::Faces(_) => None,
        }
    }
}

/// A faces step's line (design S3): `faces` and its plane, then, region by region and loop
/// by loop (each region's outer loop, then its holes), each curve as sent, every number by
/// its bits: `line x0 y0 x1 y1`, `arc cx cy sx sy ex ey r a0 a1`, `circle cx cy r`. Each
/// region starts `region` and each loop `loop`, so regions and holes of the same curves
/// are other lines. No ID: two sketches of one profile share the step, and a construction
/// line toggled, added, or moved leaves it alone. The bits are as sent, a zero of either
/// sign its own: −0 and +0 can be different requests (an arc's angle of ±π).
fn faces_line(regions: &FaceRegions) -> String {
    use std::fmt::Write as _;
    let mut line = format!(
        "faces {}",
        match regions.plane {
            silico_protocol::SketchPlane::Xy => "xy",
            silico_protocol::SketchPlane::Xz => "xz",
            silico_protocol::SketchPlane::Yz => "yz",
        }
    );
    for region in &regions.regions {
        line.push_str(" region");
        for curves in region.loops() {
            line.push_str(" loop");
            for curve in curves {
                let (word, values): (&str, Vec<f64>) = match *curve {
                    silico_protocol::Curve::Line { start, end } => {
                        ("line", vec![start[0], start[1], end[0], end[1]])
                    }
                    silico_protocol::Curve::Arc {
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
                    silico_protocol::Curve::Circle { center, radius } => {
                        ("circle", vec![center[0], center[1], radius])
                    }
                };
                line.push(' ');
                line.push_str(word);
                for v in values {
                    let _ = write!(line, " {:016x}", v.to_bits());
                }
            }
        }
    }
    line
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
    /// The steps that make features' solids, which are measured once they're made.
    results: BTreeSet<Key>,
    /// Each fillet's or chamfer's step, by key: the edges it names, and of what.
    dresses: BTreeMap<Key, Dress>,
    /// How each feature makes its elements, for reading names back.
    makers: BTreeMap<FeatureId, Maker>,
    /// Each sketch to solve, in timeline order: its feature, and its solve's key.
    sketches: Vec<(FeatureId, Key)>,
    /// Each sweep feature's own check of its result, by ID, until its `Planned` takes it.
    checks_of: BTreeMap<FeatureId, SweepCheck>,
}

/// A pad's, pocket's, revolve's, or groove's fuse or cut: its feature, what it is, and its
/// base, for `sweep::check_result`.
#[derive(Debug, Clone)]
struct SweepCheck {
    feature: FeatureId,
    kind: SweepFeature,
    base: FeatureId,
}

/// What a feature's result is checked for once it's built: a sweep feature's one solid and
/// change of volume (design S5, S7), or an M2 boolean's one solid (D2). Each feature's own,
/// decided when its outcome is (`model`), from its solid's stored measures and history and
/// its base's, however they were had (made, kept in memory, or read from the cache): a step
/// is shared by every feature alike (its key holds no ID), so a check by step would be the
/// first feature's, or none where the step was had another way (the review proved both).
#[derive(Debug, Clone)]
enum Check {
    Sweep(SweepCheck),
    Boolean {
        op: BooleanOp,
        base: FeatureId,
        tool: FeatureId,
    },
}

/// A solid as the checks read it: its stored summary, and its solids as its last step's
/// history counts them (a primitive is one).
fn measured_of(built: &Built) -> Option<crate::sweep::Measured> {
    let summary = built.summary()?;
    Some(crate::sweep::Measured {
        volume: summary.volume,
        solids: built.history.as_ref().map_or(1, |h| h.result.solids),
        faces: summary.faces,
        edges: summary.edges,
        vertices: summary.vertices,
    })
}

/// A sketch some pad, pocket, revolve, or groove sweeps, solved before the plan is made
/// (design S3): its solve, and its profile, or why it makes none.
#[derive(Debug, Clone)]
struct Swept {
    solution: Arc<SketchSolution>,
    profile: ProfileResult,
}

/// A sketch's profile, or why it makes none, kept by its solve's key as solves are.
type ProfileResult = Result<Arc<Profile>, Arc<crate::profile::Refused>>;

struct Planned {
    id: FeatureId,
    /// Its own steps, in order; the last makes its solid.
    steps: Vec<Key>,
    /// The earlier features it uses, base then tool, by index.
    uses: Vec<usize>,
    /// How its solid's elements are named; `None` for a sketch, which makes no solid.
    naming: Option<Naming>,
    /// Its naming key, if its steps and those of every feature it's made from could be
    /// planned: of its solid's step, its ID, how it names, and its inputs' naming keys,
    /// so two features whose maps are alike have one.
    naming_key: Option<NamingKey>,
    /// A sketch's solve's key.
    sketch: Option<Key>,
    /// The sketch a pad, pocket, revolve, or groove sweeps, by index: a sketch that doesn't
    /// solve blocks it.
    sweeps: Option<usize>,
    /// Why it isn't built, though what it uses is: a refusal of its sketch's profile or its
    /// axis, found before any step (design S5, S6).
    refused: Option<Error>,
    /// Why a pad, pocket, revolve, or groove isn't built, its sketch solved: every refusal
    /// of the sketch's profile (the first hundred), and how many there are (design S3).
    unprofiled: Option<(Vec<Error>, u64)>,
    /// What its result is checked for, once built ([`Check`]).
    check: Option<Check>,
}

/// What a sweep feature's plan came to.
enum SweepPlan {
    /// Its steps, and how its elements are named.
    Steps(Vec<Key>, Arc<SweepNaming>),
    /// Refused before any step: its sketch's regions, or its axis.
    Refused(Error),
    /// Its sketch makes no profile: its refusals (the first hundred), and how many.
    Unprofiled(Vec<Error>, u64),
    /// Its sketch didn't solve, or its base can't be built: blocked on that.
    Waits,
}

/// `value`, a zero of either sign as +0: the requests' zeros are all +0 (design S3).
fn plus_zero(value: f64) -> f64 {
    value + 0.0
}

/// The sweep step of `feature`, a pad, pocket, revolve, or groove of type `kind` sweeping
/// `profile`, whose faces step is `faces`, onto `base`'s solid if it has one: the start
/// offset, direction, and axis worked out from the sketch's plane and the extent, from the
/// document's integers, so each is exact or rounded once (design S3).
///
/// A pad goes along the sketch's normal, a pocket against it, and either the other way when
/// `reversed`; the normal is a unit axis, so a length along it is exact. `one_side` starts
/// on the plane; `symmetric` half its length back (halving is exact); `two_sides` `length2`
/// back, swept their sum, taken in nanometres. `through_all` sweeps from the plane through
/// all of the base. A revolve turns right-handed about its axis (left-handed `reversed`),
/// its angle by FreeCAD's conversion, `symmetric` from minus half of it.
fn sweep_step(
    feature: &crate::document::Feature,
    kind: SweepFeature,
    profile: &Profile,
    axis: Option<([f64; 2], [f64; 2])>,
    faces: Key,
    base: Option<Key>,
) -> Result<Step, Error> {
    use crate::document::{Extent, PadExtent, PocketExtent, TurnExtent};
    let plane = silico_protocol::SketchPlane::from(profile.plane);
    let along = |d: [f64; 3], length: f64| d.map(|c| plus_zero(c * length));
    let direction = |reversed: bool| {
        let sign = if kind.adds() != reversed { 1.0 } else { -1.0 };
        plane.normal().map(|c| plus_zero(c * sign))
    };
    let prism = |d: [f64; 3], back: f64, length: f64| Step::Prism {
        faces,
        start: along(d, -back),
        vector: along(d, length),
    };
    let unchecked = || {
        silico_bug("a sketch feature's extent isn't one its type takes, though the gate checked it")
            .with_feature(feature.id.as_str())
    };
    let length = |l: crate::document::Length| l.mm();
    Ok(match feature.operation.extent().ok_or_else(unchecked)? {
        Extent::Pad(PadExtent::OneSide {
            length: l,
            reversed,
        })
        | Extent::Pocket(PocketExtent::OneSide {
            length: l,
            reversed,
        }) => prism(direction(reversed), 0.0, length(l)),
        Extent::Pad(PadExtent::Symmetric { length: l })
        | Extent::Pocket(PocketExtent::Symmetric { length: l }) => {
            prism(direction(false), length(l) / 2.0, length(l))
        }
        Extent::Pad(PadExtent::TwoSides { length: l, length2 })
        | Extent::Pocket(PocketExtent::TwoSides { length: l, length2 }) => {
            let total = crate::document::Length::from_nm(l.nm().saturating_add(length2.nm()));
            prism(direction(false), length(length2), length(total))
        }
        Extent::Pocket(PocketExtent::ThroughAll { reversed }) => Step::PrismThroughAll {
            faces,
            base: base.ok_or_else(unchecked)?,
            direction: direction(reversed),
        },
        Extent::Turn(extent) => {
            let (origin, line) = axis.ok_or_else(unchecked)?;
            let (angle, reversed, symmetric) = match extent {
                TurnExtent::OneSide { angle, reversed } => (angle, reversed, false),
                TurnExtent::Symmetric { angle } => (angle, false, true),
            };
            // The sketch's axes are unit axes of model space, so the line's direction in
            // it is its direction in the sketch, exactly.
            let (x, y) = (plane.x_axis(), plane.y_axis());
            let sign = if reversed { -1.0 } else { 1.0 };
            let direction = [0, 1, 2].map(|k| plus_zero(sign * (x[k] * line[0] + y[k] * line[1])));
            let angle = silico_protocol::radians(angle.degrees());
            Step::Revolution {
                faces,
                origin: plane.place(origin),
                direction,
                start: if symmetric {
                    plus_zero(-(angle / 2.0))
                } else {
                    0.0
                },
                angle,
            }
        }
    })
}

/// A sketch's solve's key: SHA-256 of a domain line (`silico sketch 1`), its feature's ID
/// (a solve's errors name it), and the sketch as the file writes it, plane and elements:
/// what the solve reads, the stored coordinates included. In memory only, on one engine,
/// whose solver doesn't change (a worker is started again only from the same build), so
/// the solver's identity needn't be in it.
fn sketch_key(id: &FeatureId, sketch: &crate::document::Sketch) -> Key {
    let mut hash = Sha256::new();
    hash.update(b"silico sketch 1\n");
    hash.update(id.as_str().as_bytes());
    hash.update(b"\nplane ");
    hash.update(sketch.plane.as_str().as_bytes());
    for (kind, text) in crate::document::sketch_elements(sketch) {
        hash.update(b"\n");
        hash.update(kind.as_bytes());
        hash.update(b" ");
        hash.update(text.as_bytes());
    }
    Key(hash.finalize().into())
}

/// How a feature's solid's elements are named: a primitive's by their numbers (a move
/// keeps them), an operation's from its history, and a sweep feature's from its sketch's
/// IDs and its steps' histories (design S4).
#[derive(Debug, Clone)]
enum Naming {
    Primitive(Primitive),
    Operation(OpCode),
    Sweep(Arc<SweepNaming>),
}

/// What a pad's, pocket's, revolve's, or groove's names come from beside its steps'
/// histories: its type, its sketch, the IDs of the curves and points in the order its
/// faces step sends them, and of each region's outer loop's smallest curve; and the naming
/// key of its faces' map, which every feature of the same faces and IDs shares.
#[derive(Debug)]
struct SweepNaming {
    operation: OpCode,
    sketch: FeatureId,
    curves: Vec<ElementId>,
    points: Vec<ElementId>,
    regions: Vec<ElementId>,
    faces: NamingKey,
}

impl SweepNaming {
    /// `profile`'s IDs, for `feature` of type `kind`, sweeping `sketch`, whose faces are made
    /// by step `faces`.
    fn of(kind: SweepFeature, sketch: &FeatureId, profile: &Profile, faces: Key) -> Self {
        let mut curves = Vec::new();
        let mut points = Vec::new();
        for region in &profile.regions {
            for edge in std::iter::once(&region.outer)
                .chain(&region.holes)
                .flat_map(|l| &l.curves)
            {
                curves.push(edge.curve.clone());
                // Each curve's start, as the faces request sends its points; a circle has
                // none.
                if let Some([start, _]) = &edge.ends {
                    points.push(start.clone());
                }
            }
        }
        let regions: Vec<ElementId> = profile
            .regions
            .iter()
            .filter_map(|r| r.outer.curves.first().map(|e| e.curve.clone()))
            .collect();
        let mut hash = Sha256::new();
        hash.update(NAMING_SCHEME);
        hash.update(b"faces\n");
        hash.update(faces.0);
        hash.update(sketch.as_str().as_bytes());
        for list in [&curves, &points, &regions] {
            hash.update((list.len() as u64).to_be_bytes());
            for id in list {
                hash.update(id.as_str().as_bytes());
            }
        }
        SweepNaming {
            operation: kind.into(),
            sketch: sketch.clone(),
            curves,
            points,
            regions,
            faces: NamingKey(hash.finalize().into()),
        }
    }
}

impl Planned {
    fn key(&self) -> Option<Key> {
        self.steps.last().copied()
    }
}

impl Plan {
    /// Each sketch's solve's key, in timeline order, and which of them a pad, pocket,
    /// revolve, or groove sweeps: those are solved before the plan is made (design S3).
    fn sketches_of(document: &Checked) -> (Vec<(FeatureId, Key)>, BTreeSet<FeatureId>) {
        let features = &document.document().features;
        let sketches = features
            .iter()
            .filter_map(|f| match &f.operation {
                Operation::Sketch(sketch) => Some((f.id.clone(), sketch_key(&f.id, sketch))),
                _ => None,
            })
            .collect();
        let swept = features
            .iter()
            .filter_map(|f| f.operation.sketch().cloned())
            .collect();
        (sketches, swept)
    }

    /// `document`'s steps, every sketch a feature sweeps solved and profiled in `swept`
    /// (one that didn't solve is missing).
    fn of(document: &Checked, swept: &BTreeMap<FeatureId, Swept>) -> Plan {
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
            results: BTreeSet::new(),
            dresses: BTreeMap::new(),
            makers: features
                .iter()
                .map(|f| (f.id.clone(), Maker::of(&f.operation)))
                .collect(),
            sketches: Vec::new(),
            checks_of: BTreeMap::new(),
        };
        for feature in features {
            let mut own = Vec::new();
            let mut uses = Vec::new();
            let mut refused = None;
            let mut sweeps = None;
            let mut unprofiled = None;
            let naming = match feature.operation.made() {
                Made::Primitive(primitive) => {
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
                    Some(Naming::Primitive(primitive))
                }
                Made::Boolean(op) => {
                    // The gate has checked that both are earlier features, so both are
                    // planned.
                    if let [(_, base), (_, tool)] = feature.operation.inputs().as_slice() {
                        let input = |id: &FeatureId| index.get(id).copied();
                        if let (Some(b), Some(t)) = (input(base), input(tool)) {
                            uses = vec![b, t];
                            let key = |i: usize| plan.features.get(i).and_then(Planned::key);
                            if let (Some(base), Some(tool)) = (key(b), key(t)) {
                                own.push(plan.add(Step::Boolean { op, base, tool }, &feature.id));
                            }
                        }
                    }
                    Some(Naming::Operation(op.into()))
                }
                // A sketch makes no solid: nothing to build or name.
                Made::Sketch => None,
                Made::Fillet { radius } => {
                    let dressing = Dressing::Fillet { radius };
                    (own, uses) = plan.dress(&index, feature, dressing);
                    Some(Naming::Operation(OpCode::Fillet))
                }
                Made::Chamfer { distance } => {
                    let dressing = Dressing::Chamfer { distance };
                    (own, uses) = plan.dress(&index, feature, dressing);
                    Some(Naming::Operation(OpCode::Chamfer))
                }
                Made::Sweep(kind) => {
                    uses = feature
                        .operation
                        .inputs()
                        .iter()
                        .filter_map(|(_, base)| index.get(*base).copied())
                        .collect();
                    sweeps = feature
                        .operation
                        .sketch()
                        .and_then(|sketch| index.get(sketch).copied());
                    let planned = plan.sweep(document, feature, kind, &uses, swept);
                    match planned {
                        SweepPlan::Steps(steps, naming) => {
                            own = steps;
                            Some(Naming::Sweep(naming))
                        }
                        SweepPlan::Refused(error) => {
                            refused = Some(error);
                            None
                        }
                        SweepPlan::Unprofiled(errors, total) => {
                            unprofiled = Some((errors, total));
                            None
                        }
                        SweepPlan::Waits => None,
                    }
                }
            };
            if let Some(result) = own.last() {
                plan.results.insert(*result);
            }
            let inputs: Option<Vec<NamingKey>> = uses
                .iter()
                .map(|&i| plan.features.get(i).and_then(|p| p.naming_key))
                .collect();
            let naming_key = own
                .last()
                .zip(inputs)
                .zip(naming.as_ref())
                .map(|((key, inputs), naming)| naming_key(*key, &feature.id, naming, &inputs));
            let sketch = match &feature.operation {
                Operation::Sketch(sketch) => {
                    let key = sketch_key(&feature.id, sketch);
                    plan.sketches.push((feature.id.clone(), key));
                    Some(key)
                }
                _ => None,
            };
            plan.features.push(Planned {
                id: feature.id.clone(),
                steps: own,
                uses,
                naming,
                naming_key,
                sketch,
                sweeps,
                refused,
                unprofiled,
                check: match feature.operation.made() {
                    Made::Boolean(op) => match feature.operation.inputs().as_slice() {
                        [(_, base), (_, tool)] => Some(Check::Boolean {
                            op,
                            base: (*base).clone(),
                            tool: (*tool).clone(),
                        }),
                        _ => None,
                    },
                    _ => plan.checks_of.remove(&feature.id).map(Check::Sweep),
                },
            });
        }
        plan
    }

    /// The steps of `feature`, a pad, pocket, revolve, or groove of type `kind` (the next
    /// feature planned), whose base is `uses` (if it has one), its sketch solved in
    /// `swept` (design S3): the checks its solved sketch must pass before any kernel step
    /// (S5, S6), then its faces, its sweep, and, with a base, its fuse or cut.
    fn sweep(
        &mut self,
        document: &Checked,
        feature: &crate::document::Feature,
        kind: SweepFeature,
        uses: &[usize],
        swept: &BTreeMap<FeatureId, Swept>,
    ) -> SweepPlan {
        let operation = &feature.operation;
        // A sketch that didn't solve blocks it; one that made no profile fails it.
        let Some((sketch, solved)) = operation
            .sketch()
            .and_then(|sketch| swept.get(sketch).map(|solved| (sketch, solved)))
        else {
            return SweepPlan::Waits;
        };
        let profile = match &solved.profile {
            Ok(profile) => profile,
            Err(refused) => {
                return SweepPlan::Unprofiled(refused.errors().to_vec(), refused.total());
            }
        };
        if let Err(error) = crate::sweep::check_regions(&feature.id, kind, sketch, profile) {
            return SweepPlan::Refused(error);
        }
        let mut axis = None;
        if matches!(kind, SweepFeature::Revolve | SweepFeature::Groove) {
            let checked = crate::sweep::check_axis_length(document, &feature.id, &solved.solution)
                .and_then(|()| {
                    crate::sweep::check_axis_turn(document, &feature.id, &solved.solution, profile)
                })
                .and_then(|()| crate::sweep::axis_line(document, &feature.id, &solved.solution));
            match checked {
                Ok(line) => axis = Some((line.origin, line.direction)),
                Err(error) => return SweepPlan::Refused(error),
            }
        }
        // The base's solid, if it has one: a base that can't be planned has failed, and
        // blocks it.
        let base = match uses.first() {
            Some(&b) => match self.features.get(b).and_then(Planned::key) {
                Some(key) => Some((b, key)),
                None => return SweepPlan::Waits,
            },
            None => None,
        };
        let faces = self.add(Step::Faces(Arc::new(profile.faces())), &feature.id);
        let step = match sweep_step(feature, kind, profile, axis, faces, base.map(|b| b.1)) {
            Ok(step) => step,
            Err(error) => return SweepPlan::Refused(error),
        };
        let sweep = self.add(step, &feature.id);
        let mut steps = vec![faces, sweep];
        if let Some((b, base)) = base {
            let op = if kind.adds() {
                BooleanOp::Fuse
            } else {
                BooleanOp::Cut
            };
            let key = self.add(
                Step::Boolean {
                    op,
                    base,
                    tool: sweep,
                },
                &feature.id,
            );
            if let Some(base_id) = self.feature_id(b).cloned() {
                self.checks_of.insert(
                    feature.id.clone(),
                    SweepCheck {
                        feature: feature.id.clone(),
                        kind,
                        base: base_id,
                    },
                );
            }
            steps.push(key);
        }
        SweepPlan::Steps(
            steps,
            Arc::new(SweepNaming::of(kind, sketch, profile, faces)),
        )
    }

    /// The step of a fillet or chamfer `feature` (the next feature planned), of its base:
    /// its steps, and the features it uses.
    fn dress(
        &mut self,
        index: &BTreeMap<&FeatureId, usize>,
        feature: &crate::document::Feature,
        dressing: Dressing,
    ) -> (Vec<Key>, Vec<usize>) {
        // The gate has checked that the base is an earlier feature, so it's planned.
        let Some(b) = feature
            .operation
            .inputs()
            .first()
            .and_then(|(_, base)| index.get(*base).copied())
        else {
            return (Vec::new(), Vec::new());
        };
        let planned = self.features.get(b);
        let (Some(base), Some(naming)) = (
            planned.and_then(Planned::key),
            planned.and_then(|p| p.naming_key),
        ) else {
            return (Vec::new(), vec![b]);
        };
        let edges = feature.operation.edges().to_vec();
        let mut digest = Sha256::new();
        digest.update(naming.0);
        for edge in &edges {
            digest.update((edge.as_str().len() as u64).to_be_bytes());
            digest.update(edge.as_str().as_bytes());
        }
        let step = Step::Dress {
            dressing,
            base,
            names: digest.finalize().into(),
        };
        let key = self.add(step, &feature.id);
        self.dresses.insert(
            key,
            Dress {
                feature: self.features.len(),
                base: b,
                edges,
            },
        );
        (vec![key], vec![b])
    }

    fn add(&mut self, step: Step, feature: &FeatureId) -> Key {
        let key = step.key();
        if self.steps.insert(key, step).is_none() {
            self.order.push(key);
            self.first_feature.insert(key, feature.clone());
        }
        key
    }

    /// What feature `base`'s element map is named from, by index: its map, if `known`
    /// holds it; or its solid, from `done`, and what its inputs' maps are named from, each
    /// alike. So naming it reads only the solids of the features whose maps aren't named
    /// yet: in a chain of fillets, each named as the next is resolved, one or two. Or the
    /// step whose solid it needs, not built yet (a cache can hold a feature's solid
    /// without those it was made from), for the fillet to wait for.
    fn sources(
        &self,
        base: usize,
        done: &BTreeMap<Key, StepResult>,
        known: &NamedMaps,
    ) -> Result<BTreeMap<usize, MapSource>, Key> {
        let mut sources = BTreeMap::new();
        let mut stack = vec![base];
        while let Some(i) = stack.pop() {
            if sources.contains_key(&i) {
                continue;
            }
            // A feature that can't be named (a sketch, or one whose steps couldn't be
            // planned) is left out, and naming what's made from it fails (`map_of`).
            let Some(planned) = self.features.get(i) else {
                continue;
            };
            let (Some(naming), Some(_)) = (planned.naming_key, planned.key()) else {
                continue;
            };
            let named = lock(known).get(&naming).map(Arc::clone);
            if let Some(map) = named {
                sources.insert(i, MapSource::Named(map));
                continue;
            }
            // Every one of its own steps, since a sweep feature's map is named from each
            // (design S3, MA1): the first not built yet is waited for (a cache can hold the
            // last without the others, which the schedule then runs: they're planned).
            let mut steps = Vec::with_capacity(planned.steps.len());
            for key in &planned.steps {
                match done.get(key) {
                    Some(Ok(built)) => steps.push(Arc::clone(built)),
                    // Not built yet, or failed (which blocks the fillet: it never runs).
                    _ => return Err(*key),
                }
            }
            sources.insert(i, MapSource::Built(steps));
            stack.extend(planned.uses.iter().copied());
        }
        Ok(sources)
    }

    /// Feature `j`'s element map, named from `sources` (`Plan::sources`): each map named
    /// already as it is, or named from its feature's history and its inputs' maps (each
    /// named first, in timeline order, and kept in `known`), as a published model names
    /// them.
    fn map_of(
        &self,
        j: usize,
        sources: &BTreeMap<usize, MapSource>,
        known: &NamedMaps,
    ) -> Option<Arc<ElementMap>> {
        let mut named: BTreeMap<usize, Arc<ElementMap>> = BTreeMap::new();
        // Ascending, so each feature comes after those it's made from.
        for (&i, source) in sources.range(..=j) {
            let steps = match source {
                MapSource::Named(map) => {
                    named.insert(i, Arc::clone(map));
                    continue;
                }
                MapSource::Built(steps) => steps,
            };
            let planned = self.features.get(i)?;
            let key = planned.naming_key?;
            // Named meanwhile, by another fillet's step.
            let meanwhile = lock(known).get(&key).map(Arc::clone);
            if let Some(map) = meanwhile {
                named.insert(i, map);
                continue;
            }
            let inputs: Option<Vec<&ElementMap>> = planned
                .uses
                .iter()
                .map(|u| named.get(u).map(Arc::as_ref))
                .collect();
            let map = named_as(
                &planned.id,
                planned.naming.as_ref()?,
                &inputs?,
                steps,
                known,
            )?;
            let map = kept_in(known, key, map);
            named.insert(i, map);
        }
        named.remove(&j)
    }

    /// Feature `i`'s ID.
    fn feature_id(&self, i: usize) -> Option<&FeatureId> {
        self.features.get(i).map(|p| &p.id)
    }

    /// The feature to name for step `key`.
    fn feature(&self, key: &Key) -> &str {
        self.first_feature.get(key).map_or("", FeatureId::as_str)
    }
}

/// Whether `built` could be what `step` makes: no history for a primitive, and for any
/// other step a history of as many inputs as it has ([`Step::history_inputs`]: one for a
/// faces step, which takes no step's result, and one for a through-all prism, whose base
/// only sizes it) that holds together, and, for a sweep, keeps the sweep's rule
/// (`History::sweep_fault`).
fn fits(step: &Step, built: &Built) -> bool {
    match (step.history_inputs(), &built.history) {
        (0, None) => true,
        (inputs, Some(history)) if inputs > 0 => {
            history.validate(inputs).is_ok()
                && step
                    .sweep_kind()
                    .is_none_or(|kind| history.sweep_fault(kind).is_none())
        }
        _ => false,
    }
}

/// How much a result knows of its measure, least first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MeasureKnown {
    Nothing,
    /// Why the kernel refused it, which, for the session, may not last.
    Refused,
    Summary,
}

/// How much `built` knows of its measure.
fn measure_known(built: &Built) -> MeasureKnown {
    match (built.summary(), built.measure_refused()) {
        (Some(_), _) => MeasureKnown::Summary,
        (None, Some(_)) => MeasureKnown::Refused,
        (None, None) => MeasureKnown::Nothing,
    }
}

/// Whether a recompute's `result` for a step is kept in place of `kept_now`, what's kept
/// for it: a result, unless what's kept knows more of its measure; a failure, if it follows
/// from the step's values, and nothing kept is a result (a step's result is a function of
/// its key, so a failure beside one was a passing one: a superseded recompute's, say, short
/// of memory, finishing last).
fn replaces(kept_now: Option<&StepResult>, result: &StepResult) -> bool {
    match (kept_now, result) {
        (Some(Ok(old)), Ok(new)) => measure_known(new) >= measure_known(old),
        (_, Ok(_)) => true,
        (Some(Ok(_)), Err(_)) => false,
        (_, Err(error)) => kept(error),
    }
}

/// Whether a step's failure follows from its values, so asking again would fail again.
fn kept(error: &Error) -> bool {
    matches!(
        error.code(),
        ErrorCode::InvalidInput | ErrorCode::KernelFailed | ErrorCode::InvalidResult
    )
}

/// A job a recompute's threads run: making a step, or measuring a feature's solid.
#[derive(Debug, Clone, Copy)]
enum Job {
    Make(Key),
    Measure(Key),
}

impl Job {
    fn key(self) -> Key {
        match self {
            Job::Make(key) | Job::Measure(key) => key,
        }
    }

    /// What the job does, for its span.
    fn name(self) -> &'static str {
        match self {
            Job::Make(_) => "make",
            Job::Measure(_) => "measure",
        }
    }
}

/// What a job came to.
enum Ran {
    Made(Outcome<Built>),
    /// A fillet's or chamfer's edges that name none of its base's: the first hundred,
    /// each its `reference_broken` error, and how many there are. The step isn't run.
    Unresolved(crate::references::Broken),
    /// The solid, measured: a copy, made outside the schedule's lock.
    Measured(Outcome<Built>),
}

/// An error that's a bug in Silico.
fn silico_bug(message: &str) -> Error {
    Error::new(ErrorCode::Internal, message).with_suggestion(crate::worker::SILICO_BUG)
}

/// A recompute's steps in flight.
struct Schedule {
    /// Results so far: the kept ones it started with, and its own.
    done: BTreeMap<Key, StepResult>,
    /// For each step not yet run, how many of its inputs aren't built yet.
    waiting: BTreeMap<Key, usize>,
    /// The steps that use each step.
    users: BTreeMap<Key, Vec<Key>>,
    ready: VecDeque<Job>,
    /// Features' solids that couldn't be measured, by their steps' keys: why.
    unmeasured: BTreeMap<Key, Error>,
    running: usize,
    superseded: bool,
    /// A step that panicked, a bug in Silico: the recompute stops, and fails with this.
    broken: Option<Error>,
    /// Fillets' and chamfers' steps whose edges didn't all resolve, by key: their broken
    /// references' errors (the first hundred), and how many there are.
    unresolved: BTreeMap<Key, crate::references::Broken>,
}

impl<K: Kernel + 'static> Engine<K> {
    fn run(&self, document: &Arc<Checked>, generation: Generation) -> Result<RunOutcome, Error> {
        // Every sketch a feature sweeps is solved, and its profile made, before the plan
        // (design S3): a sweep's steps depend on its profile. Unchanged sketches cost
        // nothing (their solves and profiles are kept by key).
        let swept = match self.solve_swept(document, generation)? {
            Some(swept) => swept,
            None => return Ok(RunOutcome::Superseded),
        };
        let plan = Plan::of(document, &swept);
        let keys: Arc<BTreeSet<Key>> = Arc::new(
            plan.steps
                .keys()
                .copied()
                .chain(plan.sketches.iter().map(|(_, key)| *key))
                .collect(),
        );
        let (mut done, mut solutions): (BTreeMap<Key, StepResult>, BTreeMap<Key, SketchResult>) = {
            let mut state = lock(&self.state);
            // The newest recompute's plan is worth keeping results for, whether or not it
            // finishes: an older one finishing later keeps what this one will use.
            if state.newest_started == generation {
                state.newest_plan = Some(Arc::clone(&keys));
            }
            let done = plan
                .order
                .iter()
                .filter_map(|key| state.results.get(key).map(|r| (*key, r.clone())))
                .collect();
            let solutions = plan
                .sketches
                .iter()
                .filter_map(|(_, key)| state.solutions.get(key).map(|r| (*key, r.clone())))
                .collect();
            (done, solutions)
        };
        // The sketches not solved before, solved on a thread of their own.
        let to_solve: Vec<(FeatureId, Key)> = plan
            .sketches
            .iter()
            .filter(|(_, key)| !solutions.contains_key(key))
            .cloned()
            .collect();
        // What's kept on disk for this kernel, for the steps not kept in memory.
        if let Some(cache) = &self.cache {
            let kernel = self.kernel.identity();
            for key in &plan.order {
                if !done.contains_key(key)
                    && let Some(built) = cache.get(kernel, key)
                {
                    // A seal says this user's Silico wrote the bytes, not that what it wrote
                    // fits the step: a history that doesn't is a miss, as a worker's reply
                    // with one would be refused.
                    if plan.steps.get(key).is_some_and(|step| fits(step, &built)) {
                        done.insert(*key, Ok(Arc::new(built)));
                    } else {
                        tracing::warn!(key = %key, "a cache entry doesn't fit its step: rebuilding it");
                    }
                }
            }
        }
        let mut schedule = Schedule {
            done,
            waiting: BTreeMap::new(),
            users: BTreeMap::new(),
            ready: VecDeque::new(),
            unmeasured: BTreeMap::new(),
            unresolved: BTreeMap::new(),
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
                schedule.ready.push_back(Job::Make(*key));
            } else {
                schedule.waiting.insert(*key, unbuilt);
            }
        }
        // Features' solids built before (kept, or from the cache) but not measured yet,
        // unless the kernel refused to for their values: it would again.
        for key in &plan.results {
            if let Some(Ok(built)) = schedule.done.get(key)
                && built.summary().is_none()
            {
                match built.measure_refused() {
                    Some(error) => {
                        schedule.unmeasured.insert(*key, error.clone());
                    }
                    None => schedule.ready.push_back(Job::Measure(*key)),
                }
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
        let solved = std::thread::scope(|scope| -> Result<Option<SketchRun>, Error> {
            let sketches = if to_solve.is_empty() {
                None
            } else {
                let spawned = std::thread::Builder::new()
                    .name(format!("recompute-{}-sketches", generation.0))
                    .spawn_scoped(scope, || {
                        let _entered = parent.enter();
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            self.solve_sketches(document, &to_solve, generation)
                        }))
                    })
                    .map_err(|e| crate::pool::thread_error("recompute", &e))?;
                Some(spawned)
            };
            start_threads(threads, |n| {
                std::thread::Builder::new()
                    .name(format!("recompute-{}-{n}", generation.0))
                    .spawn_scoped(scope, || {
                        let _entered = parent.enter();
                        self.serve(&plan, &schedule, &changed, generation);
                    })
                    .map(drop)
            })?;
            let Some(sketches) = sketches else {
                return Ok(Some(SketchRun::new()));
            };
            match sketches.join() {
                Ok(Ok(solved)) => Ok(solved),
                // A panic under a solve is a bug in Silico, as under a step.
                Ok(Err(panic)) | Err(panic) => {
                    Err(Error::new(ErrorCode::Internal, "a sketch's solve panicked")
                        .with_cause(panic_text(panic.as_ref()))
                        .with_suggestion(crate::worker::SILICO_BUG))
                }
            }
        })?;
        let schedule = schedule
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(error) = schedule.broken {
            return Err(error);
        }
        self.keep(&schedule.done);
        // A superseded solve gives nothing to keep or publish.
        let Some(solved) = solved else {
            return Ok(RunOutcome::Superseded);
        };
        self.keep_solutions(&solved);
        solutions.extend(solved.into_iter().map(|(key, s)| (key, s.result())));
        if schedule.superseded {
            return Ok(RunOutcome::Superseded);
        }
        let known = Arc::clone(&lock(&self.state).known);
        let model = Arc::new(model(
            document, &plan, &schedule, &solutions, generation, known,
        ));
        let mut state = lock(&self.state);
        if state.generation != generation || state.cancelled {
            return Ok(RunOutcome::Superseded);
        }
        // Maps only the models worth keeping name are kept: theirs, and their sketches'
        // faces'.
        let named: BTreeSet<NamingKey> = model
            .naming
            .iter()
            .flatten()
            .flat_map(|n| {
                let faces = match &n.how {
                    Naming::Sweep(sweep) => Some(sweep.faces),
                    Naming::Primitive(_) | Naming::Operation(_) => None,
                };
                std::iter::once(n.key).chain(faces)
            })
            .collect();
        state.published_keys.push_front(named);
        state.published_keys.truncate(PUBLISHED_KEPT);
        let kept: BTreeSet<NamingKey> = state.published_keys.iter().flatten().copied().collect();
        lock(&state.known).retain(|key, _| kept.contains(key));
        state.published = Some(Arc::clone(&model));
        state.published_plans.push_front(keys);
        state.published_plans.truncate(PUBLISHED_KEPT);
        state.trim();
        Ok(RunOutcome::Published(model))
    }

    /// The sketches `document`'s pads, pockets, revolves, and grooves sweep, each solved
    /// (those not solved before, now, for `generation`) and its profile made: by sketch,
    /// those that solved. None if a newer recompute supersedes this one meanwhile.
    fn solve_swept(
        &self,
        document: &Checked,
        generation: Generation,
    ) -> Result<Option<BTreeMap<FeatureId, Swept>>, Error> {
        let (sketches, swept) = Plan::sketches_of(document);
        let used: Vec<(FeatureId, Key)> = sketches
            .into_iter()
            .filter(|(id, _)| swept.contains(id))
            .collect();
        if used.is_empty() {
            return Ok(Some(BTreeMap::new()));
        }
        let _span = tracing::debug_span!("solve_swept", sketches = used.len()).entered();
        let (mut solutions, mut profiles): (SketchResults, BTreeMap<Key, ProfileResult>) = {
            let state = lock(&self.state);
            let solutions = used
                .iter()
                .filter_map(|(_, key)| state.solutions.get(key).map(|r| (*key, r.clone())))
                .collect();
            let profiles = used
                .iter()
                .filter_map(|(_, key)| state.profiles.get(key).map(|p| (*key, p.clone())))
                .collect();
            (solutions, profiles)
        };
        let to_solve: Vec<(FeatureId, Key)> = used
            .iter()
            .filter(|(_, key)| !solutions.contains_key(key))
            .cloned()
            .collect();
        if !to_solve.is_empty() {
            // A panic under a solve is a bug in Silico, as under a step.
            let solved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.solve_sketches(document, &to_solve, generation)
            }))
            .map_err(|panic| {
                Error::new(ErrorCode::Internal, "a sketch's solve panicked")
                    .with_cause(panic_text(panic.as_ref()))
                    .with_suggestion(crate::worker::SILICO_BUG)
            })?;
            let Some(solved) = solved else {
                return Ok(None);
            };
            self.keep_solutions(&solved);
            solutions.extend(solved.into_iter().map(|(key, s)| (key, s.result())));
        }
        let mut swept_sketches = BTreeMap::new();
        let mut made = Vec::new();
        for (id, key) in &used {
            let Some(Ok(solution)) = solutions.get(key) else {
                continue;
            };
            let profile = match profiles.get(key) {
                Some(profile) => profile.clone(),
                None => {
                    let profile = crate::profile::profile(document, id, solution)
                        .map(Arc::new)
                        .map_err(Arc::new);
                    made.push((*key, profile.clone()));
                    profiles.insert(*key, profile.clone());
                    profile
                }
            };
            swept_sketches.insert(
                id.clone(),
                Swept {
                    solution: Arc::clone(solution),
                    profile,
                },
            );
        }
        if !made.is_empty() {
            let mut state = lock(&self.state);
            state.profiles.extend(made);
            state.trim();
        }
        Ok(Some(swept_sketches))
    }

    /// Keeps a recompute's results by key, but for failures that might not happen again,
    /// and only what the plans worth keeping use: a superseded recompute's, if it's the
    /// newest, and the published ones'.
    ///
    /// A result already kept isn't replaced by one that knows less of its measure
    /// ([`replaces`]): a superseded recompute finishing last would otherwise put back what a
    /// newer one had measured.
    fn keep(&self, done: &BTreeMap<Key, StepResult>) {
        let mut state = lock(&self.state);
        for (key, result) in done {
            if replaces(state.results.get(key), result) {
                state.results.insert(*key, result.clone());
            }
        }
        state.trim();
    }

    /// Stops solve `number` after `after`, if it's still running then (a superseded solve
    /// that hadn't run [`SKETCH_STOP_AFTER`] when a newer recompute started). On a thread of
    /// its own, which holds the engine no longer than that; if none can start, the solve is
    /// stopped now.
    fn stop_later(engine: &Arc<Self>, after: Duration, number: u64) {
        let stop = move |engine: &Self| {
            let mut state = lock(&engine.state);
            if state.solving.as_ref().is_some_and(|s| s.number == number) {
                state.stop_solve();
            }
        };
        let held = Arc::clone(engine);
        let spawned = std::thread::Builder::new()
            .name("recompute-stop-solve".to_owned())
            .spawn(move || {
                std::thread::sleep(after);
                stop(&held);
            });
        if spawned.is_err() {
            stop(engine);
        }
    }

    /// Keeps a recompute's solves by key, as [`keep`](Self::keep) keeps steps' results: a
    /// failure only if it follows from the sketch (it would fail again).
    fn keep_solutions(&self, solved: &SketchRun) {
        let mut state = lock(&self.state);
        for (key, outcome) in solved {
            let SketchOutcome::Solved(result) = outcome else {
                continue;
            };
            let keep = match result {
                Ok(_) => true,
                Err(error) => matches!(
                    error.code(),
                    ErrorCode::InvalidInput
                        | ErrorCode::InvalidResult
                        | ErrorCode::OverConstrained
                        | ErrorCode::SolveFailed
                ),
            };
            if keep {
                state.solutions.insert(*key, result.clone());
            }
        }
        state.trim();
    }

    /// Solves `to_solve`'s sketches of `document`, one at a time, for `generation`: each
    /// solve's result by key; or none, once a newer recompute supersedes this one (the
    /// solve it was in is stopped then, [`EngineState::stop_older_solve`]).
    fn solve_sketches(
        &self,
        document: &Checked,
        to_solve: &[(FeatureId, Key)],
        generation: Generation,
    ) -> Option<SketchRun> {
        let mut solved = SketchRun::new();
        let superseded = |state: &EngineState| state.generation != generation || state.cancelled;
        // The solver, once no other recompute has it; or none, once this one is superseded
        // (a newer recompute's start wakes the wait), never waiting for a newer one's solve.
        let here = {
            let mut state = lock(&self.state);
            loop {
                if superseded(&state) {
                    return None;
                }
                match std::mem::replace(&mut state.solver, Solver::Lent) {
                    Solver::Here(solver) => break Some(solver),
                    Solver::Unstarted => break None,
                    Solver::Lent => {
                        state = self
                            .sketch_turn
                            .wait(state)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                }
            }
        };
        let mut lent = LentSolver {
            state: &self.state,
            turn: &self.sketch_turn,
            solver: here,
        };
        if lent.solver.is_none() {
            let started = (lock(&self.start_solver))();
            match started {
                Ok(solver) => lent.solver = Some(solver),
                // Every sketch fails with it, not kept: the next recompute tries again.
                Err(error) => {
                    for (id, key) in to_solve {
                        solved.insert(*key, solver_failure(error.clone(), id));
                    }
                    return Some(solved);
                }
            }
        }
        let solver = lent.solver.as_mut()?;
        for (id, key) in to_solve {
            let stopper = match solver.stopper() {
                Ok(stopper) => stopper,
                // The solver's failure, not the sketch's: not kept.
                Err(error) => {
                    solved.insert(*key, solver_failure(error, id));
                    continue;
                }
            };
            {
                let mut state = lock(&self.state);
                if superseded(&state) {
                    return None;
                }
                state.solves += 1;
                let number = state.solves;
                state.solving = stopper.map(|stopper| Solving {
                    generation,
                    number,
                    started: std::time::Instant::now(),
                    stopper,
                });
            }
            let result = crate::sketch::solve(document, id, solver.as_mut());
            let mut state = lock(&self.state);
            state.solving = None;
            if superseded(&state) {
                return None;
            }
            drop(state);
            solved.insert(*key, SketchOutcome::Solved(result.map(Arc::new)));
        }
        Some(solved)
    }

    /// One of a recompute's threads: runs ready jobs (steps, and measures of features'
    /// solids) until none are left to run.
    fn serve(
        &self,
        plan: &Plan,
        schedule: &Mutex<Schedule>,
        changed: &Condvar,
        generation: Generation,
    ) {
        let known = Arc::clone(&lock(&self.state).known);
        loop {
            let (job, inputs, sources) = {
                let mut s = lock(schedule);
                let (job, sources) = loop {
                    if s.superseded || s.broken.is_some() || (s.ready.is_empty() && s.running == 0)
                    {
                        changed.notify_all();
                        return;
                    }
                    let Some(job) = s.ready.pop_front() else {
                        s = changed.wait(s).unwrap_or_else(PoisonError::into_inner);
                        continue;
                    };
                    // A fillet's or chamfer's base is built: what its elements are named
                    // from. Or a solid that isn't built yet, which it waits for (and, if it
                    // failed, waits for forever: the failure blocks it).
                    let dress = match job {
                        Job::Make(key) => plan.dresses.get(&key),
                        Job::Measure(_) => None,
                    };
                    let sources = match dress.map(|d| plan.sources(d.base, &s.done, &known)) {
                        None => None,
                        Some(Ok(sources)) => Some(sources),
                        Some(Err(unbuilt)) => {
                            s.users.entry(unbuilt).or_default().push(job.key());
                            s.waiting.insert(job.key(), 1);
                            continue;
                        }
                    };
                    break (job, sources);
                };
                s.running += 1;
                // A step's inputs' results; a measure's, its step's own.
                let keys = match job {
                    Job::Make(key) => plan.steps.get(&key).map(Step::inputs).unwrap_or_default(),
                    Job::Measure(key) => vec![key],
                };
                let inputs: Vec<Arc<Built>> = keys
                    .iter()
                    .filter_map(|input| match s.done.get(input) {
                        Some(Ok(built)) => Some(Arc::clone(built)),
                        _ => None,
                    })
                    .collect();
                (job, inputs, sources)
            };
            let key = job.key();
            // A panic under a job is a bug in Silico: caught, so the recompute ends with an
            // error rather than waiting forever on a job that will never finish.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _span = tracing::warn_span!(
                    "recompute_step",
                    feature = plan.feature(&key),
                    key = %key,
                    job = job.name(),
                )
                .entered();
                match job {
                    Job::Make(key) => match (plan.steps.get(&key), plan.dresses.get(&key)) {
                        (Some(step), Some(dress)) => {
                            let sources = sources.unwrap_or_default();
                            match self.resolve(plan, dress, &sources, &known) {
                                Ok((edges, names)) => Ran::Made(self.dressed(
                                    step.clone(),
                                    &inputs,
                                    &edges,
                                    names,
                                    generation,
                                )),
                                Err(broken) => Ran::Unresolved(broken),
                            }
                        }
                        (Some(step), None) => {
                            Ran::Made(self.step(step.clone(), &inputs, generation))
                        }
                        (None, _) => Ran::Made(Outcome::Done(Err(silico_bug(
                            "a recompute step wasn't planned",
                        )))),
                    },
                    Job::Measure(_) => match inputs.first() {
                        Some(built) => Ran::Measured(self.measure(built, generation)),
                        None => Ran::Measured(Outcome::Done(Err(silico_bug(
                            "a feature's solid was measured before it was made",
                        )))),
                    },
                }
            }));
            let mut s = lock(schedule);
            let ran = match outcome {
                Ok(ran) => ran,
                Err(panic) => {
                    s.running -= 1;
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
            let mut to_keep = None;
            match ran {
                Ran::Made(Outcome::Superseded) | Ran::Measured(Outcome::Superseded) => {
                    s.superseded = true;
                }
                Ran::Unresolved(broken) => {
                    // The step fails on its first broken reference, so nothing that uses it
                    // runs; the feature says them all.
                    let first = broken.errors.first().cloned().unwrap_or_else(|| {
                        silico_bug("a fillet's edges were refused, though none is broken")
                    });
                    s.done.insert(key, Err(first));
                    s.unresolved.insert(key, broken);
                }
                Ran::Made(Outcome::Done(result)) => {
                    if result.is_ok() {
                        for user in s.users.remove(&key).unwrap_or_default() {
                            let now_ready = s.waiting.get_mut(&user).is_some_and(|n| {
                                *n = n.saturating_sub(1);
                                *n == 0
                            });
                            if now_ready {
                                s.waiting.remove(&user);
                                s.ready.push_back(Job::Make(user));
                                woken += 1;
                            }
                        }
                        // A feature's solid is measured once it's made, by a job of its own:
                        // what uses it doesn't wait for that.
                        if plan.results.contains(&key) {
                            s.ready.push_back(Job::Measure(key));
                            woken += 1;
                        }
                    }
                    let result = result.map(Arc::new);
                    // A feature's solid is kept on disk once it's measured, with its
                    // summary (written now too, it could land after, and over, that).
                    if let Ok(built) = &result
                        && !plan.results.contains(&key)
                    {
                        to_keep = Some(Arc::clone(built));
                    }
                    s.done.insert(key, result);
                }
                Ran::Measured(Outcome::Done(Ok(measured))) => {
                    // The measured copy takes the result's place, here and once kept; the
                    // result itself is never changed, since a published model may hold it.
                    let measured = Arc::new(measured);
                    to_keep = Some(Arc::clone(&measured));
                    s.done.insert(key, Ok(measured));
                }
                Ran::Measured(Outcome::Done(Err(error))) => {
                    // A solid that couldn't be measured is kept on disk all the same, so the
                    // next open needn't make it again. With the refusal, if it follows from
                    // the solid's values: kept for the session (not asked again), and on disk
                    // only if the cache keeps it (`cache::lasting`: not `kernel_failed`,
                    // which it writes unmeasured). Unmeasured if not, so the next recompute
                    // measures it.
                    if let Some(Ok(built)) = s.done.get(&key) {
                        let built = if kept(&error) {
                            let refused = Arc::new(built.with_refused_measure(error.clone()));
                            s.done.insert(key, Ok(Arc::clone(&refused)));
                            refused
                        } else {
                            Arc::clone(built)
                        };
                        to_keep = Some(built);
                    }
                    s.unmeasured.insert(key, error);
                }
            }
            // A job counts as running until its result is written to the cache, so a
            // recompute doesn't end before its results are on disk; the jobs it made ready
            // start meanwhile.
            drop(s);
            for _ in 0..woken {
                changed.notify_one();
            }
            if let (Some(cache), Some(built)) = (&self.cache, to_keep) {
                cache.put(self.kernel.identity(), &key, &built);
            }
            let mut s = lock(schedule);
            s.running -= 1;
            // Every thread, once nothing is left.
            let finished = s.superseded || (s.ready.is_empty() && s.running == 0);
            drop(s);
            if finished {
                changed.notify_all();
            }
        }
    }

    /// Runs one step on the kernel, for `generation`.
    fn step(&self, step: Step, inputs: &[Arc<Built>], generation: Generation) -> Outcome<Built> {
        map(self.make(step, inputs, generation), |(shape, history)| {
            Built::new(shape, history, None)
        })
    }

    /// Runs a fillet's or chamfer's step on the kernel, of `edges` (resolved, the document
    /// naming them as `names` says, so the kernel's errors name them so), for `generation`.
    fn dressed(
        &self,
        step: Step,
        inputs: &[Arc<Built>],
        edges: &[u32],
        names: EdgeNames,
        generation: Generation,
    ) -> Outcome<Built> {
        let Step::Dress { dressing, .. } = step else {
            return self.step(step, inputs, generation);
        };
        let Some(base) = inputs.first() else {
            return Outcome::Done(Err(silico_bug(
                "a fillet's step ran before its base was built",
            )));
        };
        let request = match dressing {
            Dressing::Fillet { radius } => KernelRequest::fillet(&base.shape, edges, radius),
            Dressing::Chamfer { distance } => KernelRequest::chamfer(
                &base.shape,
                edges,
                crate::kernel::ChamferSize::Equal { distance },
                false,
            ),
        };
        match request {
            Ok(request) => map(
                self.kernel
                    .submit(generation, request.with_edge_names(names))
                    .wait(),
                |data| Built::new(data.shape, Some(data.history), None),
            ),
            Err(e) => Outcome::Done(Err(e)),
        }
    }

    /// The edge numbers `dress`'s names find in its base feature's solid, its map named
    /// from `sources` (`Plan::sources`), and the names by number; or, for every name that
    /// finds none (or an edge whose name is unstable, N3), its `reference_broken` error,
    /// with candidates (N4, N6).
    fn resolve(
        &self,
        plan: &Plan,
        dress: &Dress,
        sources: &BTreeMap<usize, MapSource>,
        known: &NamedMaps,
    ) -> Result<(Vec<u32>, EdgeNames), crate::references::Broken> {
        // How many maps the names are read from, named or to be named: one or two in a
        // chain of fillets, each named as the next is resolved.
        let _span = tracing::debug_span!(
            "resolve",
            edges = dress.edges.len(),
            sources = sources.len()
        )
        .entered();
        let Some(map) = plan.map_of(dress.base, sources, known) else {
            return Err(crate::references::Broken::bug(silico_bug(
                "a fillet's base was built, but its elements couldn't be named",
            )));
        };
        let Some(feature) = plan.feature_id(dress.feature) else {
            return Err(crate::references::Broken::bug(silico_bug(
                "a fillet's feature wasn't planned",
            )));
        };
        crate::references::resolve(feature, &dress.edges, &map, &|f| {
            plan.makers.get(f).copied()
        })
    }

    /// Measures `built`, a feature's solid, on the kernel, for `generation`: a copy of it
    /// with its summary.
    fn measure(&self, built: &Built, generation: Generation) -> Outcome<Built> {
        map(
            self.kernel
                .submit(generation, KernelRequest::summarize(&built.shape))
                .wait(),
            |summary| built.measured(summary),
        )
    }

    /// What one step makes on the kernel, for `generation`: its shape, and its history.
    fn make(
        &self,
        step: Step,
        inputs: &[Arc<Built>],
        generation: Generation,
    ) -> Outcome<(ShapeData, Option<History>)> {
        let shape = |i: usize| inputs.get(i).map(|built| &built.shape);
        let built = |data: BuiltData| (data.shape, Some(data.history));
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
                    (shape, None)
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
            // Run by `dressed`, once its edges are resolved.
            Step::Dress { .. } => Outcome::Done(Err(silico_bug(
                "a fillet's step ran without its edges resolved",
            ))),
            Step::Faces(regions) => match KernelRequest::make_faces(&regions) {
                Ok(request) => map(self.kernel.submit(generation, request).wait(), built),
                Err(e) => Outcome::Done(Err(e)),
            },
            Step::Prism { start, vector, .. } => match shape(0) {
                Some(faces) => match KernelRequest::prism(faces, start, vector) {
                    Ok(request) => map(self.kernel.submit(generation, request).wait(), built),
                    Err(e) => Outcome::Done(Err(e)),
                },
                None => missing(),
            },
            Step::PrismThroughAll { direction, .. } => match (shape(0), shape(1)) {
                (Some(faces), Some(base)) => {
                    match KernelRequest::prism_through_all(faces, base, direction) {
                        Ok(request) => map(self.kernel.submit(generation, request).wait(), built),
                        Err(e) => Outcome::Done(Err(e)),
                    }
                }
                _ => missing(),
            },
            Step::Revolution {
                origin,
                direction,
                start,
                angle,
                ..
            } => match shape(0) {
                Some(faces) => {
                    let axis = crate::kernel::Axis { origin, direction };
                    match KernelRequest::revolution(faces, axis, start, angle) {
                        Ok(request) => map(self.kernel.submit(generation, request).wait(), built),
                        Err(e) => Outcome::Done(Err(e)),
                    }
                }
                None => missing(),
            },
        }
    }
}

/// What a panic said, if it said anything.
fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|said| (*said).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".to_owned())
}

fn map<T, U>(outcome: Outcome<T>, f: impl FnOnce(T) -> U) -> Outcome<U> {
    match outcome {
        Outcome::Done(result) => Outcome::Done(result.map(f)),
        Outcome::Superseded => Outcome::Superseded,
    }
}

/// What became of each feature of `source`, from its steps' results, and the bodies.
fn model(
    source: &Arc<Checked>,
    plan: &Plan,
    schedule: &Schedule,
    solutions: &SketchResults,
    generation: Generation,
    known: NamedMaps,
) -> Model {
    let done = &schedule.done;
    let mut features: Vec<(FeatureId, FeatureOutcome)> = Vec::with_capacity(plan.features.len());
    for planned in &plan.features {
        // A feature it uses that didn't build blocks it, on the failure at the root: its
        // base, then the sketch it sweeps.
        let blocked =
            planned
                .uses
                .iter()
                .chain(&planned.sweeps)
                .find_map(|&i| match features.get(i) {
                    Some((id, FeatureOutcome::Failed(_) | FeatureOutcome::Broken { .. })) => {
                        Some(id.clone())
                    }
                    Some((_, FeatureOutcome::Blocked { on })) => Some(on.clone()),
                    Some((_, FeatureOutcome::Built { .. } | FeatureOutcome::Sketch(_))) | None => {
                        None
                    }
                });
        let outcome = match (blocked, planned.sketch) {
            (Some(on), _) => FeatureOutcome::Blocked { on },
            (None, Some(key)) => match solutions.get(&key) {
                Some(Ok(solution)) => FeatureOutcome::Sketch(Arc::clone(solution)),
                Some(Err(error)) => FeatureOutcome::Failed(error.clone()),
                None => FeatureOutcome::Failed(
                    silico_bug("a sketch wasn't solved").with_feature(planned.id.as_str()),
                ),
            },
            (None, None) => match (&planned.refused, &planned.unprofiled) {
                (Some(error), _) => FeatureOutcome::Failed(error.clone()),
                (None, Some((errors, total))) => FeatureOutcome::Broken {
                    errors: errors.clone(),
                    total: *total,
                },
                (None, None) => checked(
                    planned,
                    feature_outcome(planned, done, &schedule.unresolved),
                    &features,
                    &schedule.unmeasured,
                ),
            },
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
    // Built features whose solids couldn't be measured: why, naming the feature.
    let unmeasured = features
        .iter()
        .filter_map(|(id, outcome)| match outcome {
            FeatureOutcome::Built { key, .. } => schedule
                .unmeasured
                .get(key)
                .map(|error| (id.clone(), error.clone().with_feature(id.as_str()))),
            FeatureOutcome::Failed(_)
            | FeatureOutcome::Broken { .. }
            | FeatureOutcome::Blocked { .. }
            | FeatureOutcome::Sketch(_) => None,
        })
        .collect();
    let naming = naming(plan, &features, done);
    let maps = (0..features.len()).map(|_| OnceLock::new()).collect();
    Model {
        source: Arc::clone(source),
        generation,
        features,
        bodies,
        unmeasured,
        naming,
        maps,
        known,
    }
}

/// What a feature's element names are a function of: its solid's step (what made it, and
/// so its history), its own ID and how it names (the names hold it), and its inputs'
/// naming keys. A feature with the same key is named alike, so a recompute takes its map
/// from the model before it rather than naming its elements again: an edit names only the
/// features it changes (`/break-it` found every map rebuilt on every edit).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NamingKey([u8; 32]);

impl fmt::Debug for NamingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NamingKey({})", crate::hex(&self.0))
    }
}

/// How each built feature's elements are named, in timeline order: a primitive's by their
/// numbers, a boolean's from its inputs' names and its step's history
/// (docs/ports/naming.md), each with its naming key.
fn naming(
    plan: &Plan,
    features: &[(FeatureId, FeatureOutcome)],
    done: &BTreeMap<Key, StepResult>,
) -> Vec<Option<FeatureNaming>> {
    let mut naming: Vec<Option<FeatureNaming>> = Vec::with_capacity(features.len());
    for (planned, (id, outcome)) in plan.features.iter().zip(features) {
        let (FeatureOutcome::Built { key, .. }, Some(how)) = (outcome, planned.naming.as_ref())
        else {
            naming.push(None);
            continue;
        };
        // Its own steps' results: a built feature's are all built.
        let steps: Option<Vec<Arc<Built>>> = planned
            .steps
            .iter()
            .map(|step| match done.get(step) {
                Some(Ok(built)) => Some(Arc::clone(built)),
                _ => None,
            })
            .collect();
        // (A feature whose inputs aren't named isn't either: naming it needs their maps.)
        let planned_key = planned
            .naming_key
            .filter(|_| planned.key() == Some(*key) && planned.id == *id);
        naming.push(planned_key.zip(steps).map(|(key, steps)| FeatureNaming {
            key,
            how: how.clone(),
            uses: planned.uses.clone(),
            steps,
        }));
    }
    naming
}

/// Feature `id`'s element map, as `how` names it: a primitive's by its numbers, an
/// operation's from its inputs' maps and its last step's history, and a sweep feature's in
/// three passes, one a step (design S3): its sketch's faces (that map kept in `known` by its
/// own key, so features sharing a sketch name it once), their sweep, and, with a base, its
/// fuse or cut of the base's map and the sweep's. `steps` are the feature's own steps'
/// results, in order (none with no history: no map). The one place maps are made, so the
/// names a model gives and those a fillet's references are found among are always the same.
fn named_as(
    id: &FeatureId,
    how: &Naming,
    inputs: &[&ElementMap],
    steps: &[Arc<Built>],
    known: &NamedMaps,
) -> Option<ElementMap> {
    let history = |i: usize| steps.get(i).and_then(|built| built.history.as_ref());
    Some(match how {
        Naming::Primitive(primitive) => ElementMap::primitive(id.clone(), primitive),
        Naming::Operation(operation) => ElementMap::operation(
            id.clone(),
            *operation,
            inputs,
            history(steps.len().checked_sub(1)?)?,
        ),
        Naming::Sweep(sweep) => {
            let named = lock(known).get(&sweep.faces).map(Arc::clone);
            let faces = match named {
                Some(map) => map,
                None => kept_in(
                    known,
                    sweep.faces,
                    ElementMap::sketch(
                        sweep.sketch.clone(),
                        &sweep.curves,
                        &sweep.points,
                        &sweep.regions,
                        history(0)?,
                    ),
                ),
            };
            let swept = ElementMap::swept(id.clone(), sweep.operation, &faces, history(1)?);
            match (inputs, steps.len()) {
                ([], 2) => swept,
                ([base], 3) => {
                    ElementMap::operation(id.clone(), sweep.operation, &[base, &swept], history(2)?)
                }
                _ => return None,
            }
        }
    })
}

/// `map`, kept in `known` by its naming key; or the one kept there first, so a map named in
/// two threads at once is one map.
fn kept_in(known: &NamedMaps, key: NamingKey, map: ElementMap) -> Arc<ElementMap> {
    Arc::clone(lock(known).entry(key).or_insert_with(|| Arc::new(map)))
}

/// A feature's naming key: of the naming scheme, its solid's step `key`, its ID, how it
/// names, and its inputs' naming keys.
fn naming_key(key: Key, id: &FeatureId, how: &Naming, inputs: &[NamingKey]) -> NamingKey {
    let mut hash = Sha256::new();
    hash.update(NAMING_SCHEME);
    hash.update(key.0);
    hash.update(id.as_str().as_bytes());
    hash.update([match how {
        Naming::Primitive(_) => 0,
        Naming::Operation(op) => *op as u8 + 1,
        Naming::Sweep(sweep) => sweep.operation as u8 + 1,
    }]);
    // A sweep feature's names come from its sketch's IDs too, which its steps' keys leave
    // out (design S3): two profiles alike but for an ID make one faces step and two maps.
    if let Naming::Sweep(sweep) = how {
        hash.update(sweep.faces.0);
    }
    for input in inputs {
        hash.update(input.0);
    }
    NamingKey(hash.finalize().into())
}

/// `outcome`, `planned`'s, as its result's check decides it (`Check`): a built feature
/// whose solid fails it is failed, in its own words, so what uses it is blocked on it. Read
/// from the solid's stored summary and history and its base's (`features`, decided before
/// it), the same whether the solid was made, kept, or read from the cache; a solid that
/// couldn't be measured can't be checked, and fails with why (`unmeasured`).
fn checked(
    planned: &Planned,
    outcome: FeatureOutcome,
    features: &[(FeatureId, FeatureOutcome)],
    unmeasured: &BTreeMap<Key, Error>,
) -> FeatureOutcome {
    let (FeatureOutcome::Built { key, built }, Some(check)) = (&outcome, &planned.check) else {
        return outcome;
    };
    let decided = match check {
        Check::Boolean { op, base, tool } => {
            let solids = built.history.as_ref().map_or(1, |h| h.result.solids);
            crate::sweep::check_boolean_solids(&planned.id, *op, base, tool, solids)
        }
        Check::Sweep(sweep) => {
            let base = planned
                .uses
                .first()
                .and_then(|&b| features.get(b))
                .and_then(|(_, outcome)| match outcome {
                    FeatureOutcome::Built { key, built } => Some((*key, built)),
                    _ => None,
                });
            let base_key = base.map(|(key, _)| key);
            let base = base.and_then(|(_, built)| measured_of(built));
            match (base, measured_of(built)) {
                (Some(before), Some(after)) => crate::sweep::check_result(
                    &sweep.feature,
                    sweep.kind,
                    &sweep.base,
                    &before,
                    &after,
                ),
                _ => Err(unmeasured
                    .get(key)
                    .or_else(|| base_key.and_then(|k| unmeasured.get(&k)))
                    .cloned()
                    .unwrap_or_else(|| {
                        silico_bug("a sketch feature's solid or its base's wasn't measured")
                    })
                    .with_feature(planned.id.as_str())),
            }
        }
    };
    match decided {
        Ok(()) => outcome,
        Err(error) => FeatureOutcome::Failed(error),
    }
}

/// A feature whose inputs built: built, if its own steps did, or failed on the first that
/// didn't.
fn feature_outcome(
    planned: &Planned,
    done: &BTreeMap<Key, StepResult>,
    unresolved: &BTreeMap<Key, crate::references::Broken>,
) -> FeatureOutcome {
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
            Some(Err(_)) if let Some(broken) = unresolved.get(key) => {
                return FeatureOutcome::Broken {
                    errors: broken.errors.clone(),
                    total: broken.total,
                };
            }
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

    #[test]
    fn a_recompute_runs_with_the_threads_that_start_and_fails_only_with_none() {
        let short = || std::io::Error::from_raw_os_error(libc::EAGAIN);
        // Every thread starts: all of them.
        assert_eq!(start_threads(3, |_| Ok(())).unwrap(), 3);
        // The first doesn't: the shortage, said as a recompute's.
        let err = start_threads(3, |_| Err(short())).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ResourceExhausted, "{err}");
        assert_eq!(err.message(), "couldn't start a recompute thread");
        // The third of four doesn't: the two before it, and none tried after it.
        let mut tried = Vec::new();
        let started = start_threads(4, |n| {
            tried.push(n);
            if n == 2 { Err(short()) } else { Ok(()) }
        });
        assert_eq!(started.unwrap(), 2);
        assert_eq!(tried, [0, 1, 2]);
    }

    #[test]
    fn a_superseded_solve_is_left_only_what_remains_of_its_grace() {
        // Started 1 ms ago, superseded now: it may run on for at most the 9 ms left of
        // SKETCH_STOP_AFTER's 10, not stopped yet. (Had this thread stalled past the
        // grace, it's stopped now instead.)
        let engine = Recompute::new(crate::fake::FakeKernel::new(1).unwrap());
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut state = crate::worker::lock(&engine.engine.state);
        state.solving = Some(Solving {
            generation: Generation(1),
            number: 7,
            started: std::time::Instant::now()
                .checked_sub(Duration::from_millis(1))
                .unwrap(),
            stopper: SolveStopper::new({
                let stopped = Arc::clone(&stopped);
                move || stopped.store(true, std::sync::atomic::Ordering::SeqCst)
            }),
        });
        match state.stop_older_solve(Generation(2)) {
            Some((left, number)) => {
                assert_eq!(number, 7);
                assert!(left <= Duration::from_millis(9), "{left:?} left");
                assert!(!stopped.load(std::sync::atomic::Ordering::SeqCst));
            }
            None => {
                assert!(stopped.load(std::sync::atomic::Ordering::SeqCst));
                assert!(state.solving.is_none());
            }
        }
    }

    #[test]
    fn a_naming_key_debugs_as_its_hex() {
        let key = NamingKey(core::array::from_fn(|i| i as u8));
        assert_eq!(
            format!("{key:?}"),
            "NamingKey(000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f)"
        );
    }

    #[test]
    fn a_result_kept_is_replaced_only_by_one_that_knows_as_much_of_its_measure() {
        use crate::kernel::Blob;
        // Found by review: a superseded recompute finishing last put back an unmeasured copy
        // over a newer run's measure, or its session refusal over a newer run's summary.
        let shape = || ShapeData::checked(Blob::inline(b"a shape".to_vec()), true);
        let summary = ShapeSummary {
            volume: 1.0,
            area: 6.0,
            center: [0.5; 3],
            faces: 6,
            edges: 12,
            vertices: 8,
            bounds: Some([[0.0; 3], [1.0; 3]]),
            valid: true,
        };
        let refusal = Error::new(ErrorCode::KernelFailed, "Summarize failed");
        let measured: StepResult = Ok(Arc::new(Built::new(shape(), None, Some(summary))));
        let refused: StepResult = Ok(Arc::new(Built::refused_measure(shape(), None, refusal)));
        let unmeasured: StepResult = Ok(Arc::new(Built::new(shape(), None, None)));
        let failed = |code| -> StepResult { Err(Error::new(code, "the step failed")) };
        // [kept now][coming], as measured, refused, unmeasured: a result replaces one that
        // knows as much or less.
        let results = [&measured, &refused, &unmeasured];
        let replaced = [
            [true, false, false],
            [true, true, false],
            [true, true, true],
        ];
        for (old, row) in results.iter().zip(replaced) {
            for (new, expected) in results.iter().zip(row) {
                assert_eq!(replaces(Some(old), new), expected, "{old:?} by {new:?}");
            }
        }
        // Nothing kept: any result is.
        for new in results {
            assert!(replaces(None, new));
        }
        // A failure is kept if it follows from the step's values, and only in place of
        // nothing, or of a failure: never of a result, which shows the failure passed.
        let kept_failure = failed(ErrorCode::KernelFailed);
        for old in [None, Some(&kept_failure)] {
            assert!(replaces(old, &failed(ErrorCode::InvalidInput)));
            assert!(!replaces(old, &failed(ErrorCode::WorkerCrashed)));
        }
        for old in results {
            assert!(
                !replaces(Some(old), &failed(ErrorCode::KernelFailed)),
                "{old:?}"
            );
        }
        // And a result replaces a kept failure.
        for new in results {
            assert!(replaces(Some(&kept_failure), new), "{new:?}");
        }
    }

    /// A naming key holds the naming scheme, so a change to how elements are named makes a
    /// fillet's step another step: pinned, computed by hand with Python's hashlib from the
    /// scheme's line (`silico naming 5`, M3.6's), the step's key, the ID, how it names (0 a
    /// primitive, 1 + the op code), and the inputs' naming keys.
    #[test]
    fn naming_keys_are_the_sha256_of_the_scheme_the_step_the_id_and_the_inputs() {
        let made = Step::Make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .key();
        let id = |text: &str| FeatureId::new(text).unwrap();
        let block = naming_key(
            made,
            &id("aaaaaaaa"),
            &Naming::Primitive(Primitive::Box {
                dx: 10.0,
                dy: 20.0,
                dz: 30.0,
            }),
            &[],
        );
        assert_eq!(
            crate::hex(&block.0),
            "a032aa1bd93fd19700c2b7615d4bd5aa87281e4ef8a292ac607a610c888f95f3"
        );
        let cut = naming_key(
            made,
            &id("bbbbbbbb"),
            &Naming::Operation(OpCode::Cut),
            &[block],
        );
        assert_eq!(
            crate::hex(&cut.0),
            "3aa8bf27792db5e33382f34bc8e60d6ef30415c50f5ec28070857bf1a54019ae"
        );
    }

    /// M3.6's step kinds' keys (design S3), pinned, computed by hand with Python's hashlib
    /// from the lines their doc describes: a faces step's plane, its regions and loops
    /// marked, each curve by its kind and its values' bits; a prism's faces key, start, and
    /// vector; a through-all prism's faces and base keys and direction; a revolution's faces
    /// key, axis, start, and angle.
    #[test]
    fn a_sweeps_keys_are_the_sha256_of_the_lines_their_doc_describes() {
        use silico_protocol::{Curve, Region, SketchPlane};
        let regions = FaceRegions {
            plane: SketchPlane::Xz,
            regions: vec![Region {
                outer: vec![
                    Curve::Line {
                        start: [0.0, 0.0],
                        end: [2.0, 0.0],
                    },
                    Curve::Arc {
                        center: [1.0, 0.0],
                        start: [2.0, 0.0],
                        end: [0.0, 0.0],
                        radius: 1.0,
                        start_angle: 0.0,
                        end_angle: std::f64::consts::PI,
                    },
                ],
                holes: vec![vec![Curve::Circle {
                    center: [1.0, 0.5],
                    radius: 0.25,
                }]],
            }],
        };
        let faces = Step::Faces(Arc::new(regions.clone())).key();
        assert_eq!(
            faces.to_string(),
            "30cb738ebdb757b3da923e9d2070c3994472dacd4310635157a6eb5f3fce59e1"
        );
        let prism = Step::Prism {
            faces,
            start: [0.0, 0.0, -1.5],
            vector: [0.0, 0.0, 3.0],
        };
        assert_eq!(
            prism.key().to_string(),
            "c0951601a3fb50cd77220b21c7d3861e869aa6431eec4afa38d232c0c8bb6875"
        );
        let made = Step::Make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .key();
        let through = Step::PrismThroughAll {
            faces,
            base: made,
            direction: [0.0, 0.0, -1.0],
        };
        assert_eq!(
            through.key().to_string(),
            "12c919661abe58ba096937fc445b89fe7b6749843be15897d478b8bff743f41c"
        );
        let revolution = Step::Revolution {
            faces,
            origin: [1.0, 0.0, 0.0],
            direction: [0.0, 0.0, 1.0],
            start: -std::f64::consts::FRAC_PI_4,
            angle: std::f64::consts::FRAC_PI_2,
        };
        assert_eq!(
            revolution.key().to_string(),
            "4194e7ed4f32a2955a5b4314e5c4c5dd36fc2410941529b21a91e5b06775e751"
        );
        // Their inputs, and the histories they make.
        assert_eq!(Step::Faces(Arc::new(regions)).inputs(), []);
        assert_eq!(through.inputs(), [faces, made]);
        assert_eq!(through.history_inputs(), 1);
        assert_eq!(prism.history_inputs(), 1);
    }

    /// A −0 in a faces step is another request than +0 (an arc's angle from atan2(±0, −1)
    /// is ±π), so another key (design S3): nothing is normalized. And a region with a hole
    /// is another line than two regions of the same curves.
    #[test]
    fn a_faces_key_tells_minus_zero_and_regions_apart() {
        use silico_protocol::{Curve, Region, SketchPlane};
        let square = |x0: f64| {
            vec![
                Curve::Line {
                    start: [x0, 0.0],
                    end: [1.0, 0.0],
                },
                Curve::Line {
                    start: [1.0, 0.0],
                    end: [1.0, 1.0],
                },
                Curve::Line {
                    start: [1.0, 1.0],
                    end: [x0, 1.0],
                },
                Curve::Line {
                    start: [x0, 1.0],
                    end: [x0, 0.0],
                },
            ]
        };
        let key = |regions: Vec<Region>| {
            Step::Faces(Arc::new(FaceRegions {
                plane: SketchPlane::Xy,
                regions,
            }))
            .key()
        };
        let one = |outer| Region {
            outer,
            holes: Vec::new(),
        };
        assert_ne!(key(vec![one(square(0.0))]), key(vec![one(square(-0.0))]));
        let circle = vec![Curve::Circle {
            center: [0.5, 0.5],
            radius: 0.25,
        }];
        let holed = Region {
            outer: square(0.0),
            holes: vec![circle.clone()],
        };
        assert_ne!(key(vec![holed]), key(vec![one(square(0.0)), one(circle)]));
    }

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
