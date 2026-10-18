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
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::cache::Cache;
use crate::document::{Checked, Document, ElementName, FeatureId, Fingerprint, Made};
use crate::kernel::{
    BooleanOp, BuiltData, Generation, History, Kernel, KernelRequest, Primitive, ShapeData,
    ShapeSummary,
};
use crate::naming::{ElementMap, Maker, NAMING_SCHEME, OpCode};
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
/// which puts a measured copy in its place). Never changed once made: models share it.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub shape: ShapeData,
    pub history: Option<History>,
    summary: Option<ShapeSummary>,
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
        }
    }

    /// The shape's summary, if it was measured.
    pub fn summary(&self) -> Option<&ShapeSummary> {
        self.summary.as_ref()
    }

    /// This result, measured.
    fn measured(&self, summary: ShapeSummary) -> Self {
        Built {
            summary: Some(summary),
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
    /// of them, and how many there are, listed or not. Its step isn't run.
    Broken { errors: Vec<Error>, total: u64 },
    /// Not built, since it uses a feature that failed: that feature (the first failure
    /// along its base, then its tool).
    Blocked { on: FeatureId },
    /// A sketch, which makes no solid. (Solving it comes in M3.3.)
    Sketch,
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
}

/// Models are equal when they're of the same document and every feature came to the same:
/// names are a function of those.
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
                | FeatureOutcome::Sketch => 0,
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
            | FeatureOutcome::Sketch => &[],
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
        let (id, outcome) = self.features.get(j)?;
        let history = match outcome {
            FeatureOutcome::Built { built, .. } => built.history.as_ref(),
            _ => None,
        };
        let inputs: Option<Vec<&ElementMap>> = naming
            .uses
            .iter()
            .map(|&u| self.maps.get(u)?.get()?.as_deref())
            .collect();
        let map = named_as(id, naming.how, &inputs?, history)?;
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
            | FeatureOutcome::Sketch => None,
        }
    }

    /// Each feature of its document whose fingerprint doesn't hold, in timeline order: one
    /// whose solid couldn't be measured (that error, naming it), and one whose rebuild
    /// measures otherwise than the fingerprint stored with it (a `drift` error naming the
    /// feature and what moved). A feature with no fingerprint stored isn't drift (the next
    /// save writes one), nor is one that didn't build (its failure says why).
    pub fn drift(&self) -> Vec<Error> {
        let mut errors = Vec::new();
        let mut unmeasured = self.unmeasured.iter().peekable();
        for (feature, (_, outcome)) in self.document().features.iter().zip(&self.features) {
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

    /// Its document with each feature's fingerprint as it rebuilt. A feature that didn't
    /// rebuild, or whose solid couldn't be measured or written, keeps the fingerprint
    /// stored with it: there's nothing to put in its place, and a failure that passes (a
    /// worker that hung, say) mustn't lose the record a later rebuild is checked against.
    pub fn fingerprinted(&self) -> Document {
        let mut document = self.document().clone();
        // The model's features are its document's, in order.
        for (feature, (_, outcome)) in document.features.iter_mut().zip(&self.features) {
            if let FeatureOutcome::Built { built, .. } = outcome
                && let Some(rebuilt) = built.summary().and_then(Fingerprint::of)
            {
                feature.fingerprint = Some(rebuilt);
            }
        }
        document
    }
}

/// What to do about drift: most often the feature, or one it uses, was edited (by hand, or
/// in a merge) since it was saved; otherwise the kernel's results changed.
const DRIFT: &str = "if this feature or one it uses was edited by hand or in a merge since it was saved, save the document (or run `silico fmt`) to write its new fingerprint; if not, this is a determinism bug or a change in the kernel: find which, and once the new measures are right, save to write them";

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
        Self::build(kernel, None)
    }

    /// Rebuilds on `kernel`, keeping each step's result in `cache` too, and taking it from
    /// there when it's kept: a document opened again rebuilds without the kernel.
    pub fn with_cache(kernel: K, cache: Cache) -> Self {
        Self::build(kernel, Some(cache))
    }

    fn build(kernel: K, cache: Option<Cache>) -> Self {
        let generation = kernel.generation();
        Self {
            engine: Arc::new(Engine {
                kernel,
                cache,
                state: Mutex::new(EngineState {
                    generation,
                    newest_started: generation,
                    cancelled: false,
                    dropped: false,
                    results: BTreeMap::new(),
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
        lock(&self.engine.state).dropped = true;
        self.cancel();
    }
}

/// What a step computes.
#[derive(Debug, Clone, Copy)]
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
    /// The solids of its base and every feature its base is made from, by step: the step
    /// waits for all of them, since naming the base's elements reads them all (a cache can
    /// hold the base's solid without those it was made from).
    lineage: Vec<Key>,
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
            Step::Dress {
                dressing,
                base,
                names,
            } => match dressing {
                Dressing::Fillet { radius } => {
                    format!("fillet {base} {}{}", crate::hex(&names), bits(&[radius]))
                }
                Dressing::Chamfer { distance } => {
                    format!("chamfer {base} {}{}", crate::hex(&names), bits(&[distance]))
                }
            },
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
            Step::Dress { base, .. } => vec![base],
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
    /// The steps that make features' solids, which are measured once they're made.
    results: BTreeSet<Key>,
    /// Each fillet's or chamfer's step, by key: the edges it names, and of what.
    dresses: BTreeMap<Key, Dress>,
    /// How each feature makes its elements, for reading names back.
    makers: BTreeMap<FeatureId, Maker>,
}

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
}

/// How a feature's solid's elements are named: a primitive's by their numbers (a move
/// keeps them), an operation's from its history.
#[derive(Debug, Clone, Copy)]
enum Naming {
    Primitive(Primitive),
    Operation(OpCode),
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
            results: BTreeSet::new(),
            dresses: BTreeMap::new(),
            makers: features
                .iter()
                .map(|f| (f.id.clone(), Maker::of(&f.operation)))
                .collect(),
        };
        for feature in features {
            let mut own = Vec::new();
            let mut uses = Vec::new();
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
                .zip(naming)
                .map(|((key, inputs), naming)| naming_key(*key, &feature.id, naming, &inputs));
            plan.features.push(Planned {
                id: feature.id.clone(),
                steps: own,
                uses,
                naming,
                naming_key,
            });
        }
        plan
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
        let mut lineage = BTreeSet::new();
        let mut stack = vec![b];
        while let Some(i) = stack.pop() {
            if let Some(planned) = self.features.get(i)
                && let Some(solid) = planned.key()
                && lineage.insert(solid)
            {
                stack.extend(planned.uses.iter().copied());
            }
        }
        self.dresses.insert(
            key,
            Dress {
                feature: self.features.len(),
                base: b,
                edges,
                lineage: lineage.into_iter().collect(),
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

    /// The built solids of feature `base` and every feature it's made from, by index, from
    /// `done`.
    fn lineage(
        &self,
        base: usize,
        done: &BTreeMap<Key, StepResult>,
    ) -> BTreeMap<usize, Arc<Built>> {
        let mut lineage = BTreeMap::new();
        let mut stack = vec![base];
        while let Some(i) = stack.pop() {
            if lineage.contains_key(&i) {
                continue;
            }
            let Some(planned) = self.features.get(i) else {
                continue;
            };
            if let Some(Some(Ok(built))) = planned.key().map(|key| done.get(&key)) {
                lineage.insert(i, Arc::clone(built));
                stack.extend(planned.uses.iter().copied());
            }
        }
        lineage
    }

    /// Feature `j`'s element map, its lineage's solids in `lineage`: from `known` by its
    /// naming key, or named from its history and its inputs' maps (each named first, in
    /// timeline order, and kept in `known`), as a published model names them.
    fn map_of(
        &self,
        j: usize,
        lineage: &BTreeMap<usize, Arc<Built>>,
        known: &NamedMaps,
    ) -> Option<Arc<ElementMap>> {
        let mut named: BTreeMap<usize, Arc<ElementMap>> = BTreeMap::new();
        // Ascending, so each feature comes after those it's made from.
        for (&i, built) in lineage.range(..=j) {
            let planned = self.features.get(i)?;
            let key = planned.naming_key?;
            if let Some(map) = lock(known).get(&key) {
                named.insert(i, Arc::clone(map));
                continue;
            }
            let inputs: Option<Vec<&ElementMap>> = planned
                .uses
                .iter()
                .map(|u| named.get(u).map(Arc::as_ref))
                .collect();
            let map = named_as(
                &planned.id,
                planned.naming?,
                &inputs?,
                built.history.as_ref(),
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

    /// What step `key` waits for: its inputs, and, for a fillet or chamfer, the solids its
    /// base's names are read from.
    fn needs(&self, key: &Key) -> Vec<Key> {
        let mut needs = self.steps.get(key).map(Step::inputs).unwrap_or_default();
        if let Some(dress) = self.dresses.get(key) {
            needs.extend(
                dress
                    .lineage
                    .iter()
                    .filter(|k| !needs.contains(k))
                    .copied()
                    .collect::<Vec<_>>(),
            );
        }
        needs
    }

    /// The feature to name for step `key`.
    fn feature(&self, key: &Key) -> &str {
        self.first_feature.get(key).map_or("", FeatureId::as_str)
    }
}

/// Whether `built` could be what `step` makes: no history for a step with no inputs (a
/// primitive), and for one with inputs, a history of that many that holds together.
fn fits(step: &Step, built: &Built) -> bool {
    let inputs = step.inputs().len();
    match &built.history {
        None => inputs == 0,
        Some(history) => inputs > 0 && history.validate(inputs).is_ok(),
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
        let plan = Plan::of(document);
        let keys: Arc<BTreeSet<Key>> = Arc::new(plan.steps.keys().copied().collect());
        let mut done: BTreeMap<Key, StepResult> = {
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
            let inputs = plan.needs(key);
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
        // Features' solids built before (kept, or from the cache) but not measured yet.
        for key in &plan.results {
            if let Some(Ok(built)) = schedule.done.get(key)
                && built.summary().is_none()
            {
                schedule.ready.push_back(Job::Measure(*key));
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
        let known = Arc::clone(&lock(&self.state).known);
        let model = Arc::new(model(document, &plan, &schedule, generation, known));
        let mut state = lock(&self.state);
        if state.generation != generation || state.cancelled {
            return Ok(RunOutcome::Superseded);
        }
        // Maps only the models worth keeping name are kept.
        let named: BTreeSet<NamingKey> = model.naming.iter().flatten().map(|n| n.key).collect();
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

    /// One of a recompute's threads: runs ready jobs (steps, and measures of features'
    /// solids) until none are left to run.
    fn serve(
        &self,
        plan: &Plan,
        schedule: &Mutex<Schedule>,
        changed: &Condvar,
        generation: Generation,
    ) {
        loop {
            let (job, inputs, lineage) = {
                let mut s = lock(schedule);
                let job = loop {
                    if s.superseded || s.broken.is_some() || (s.ready.is_empty() && s.running == 0)
                    {
                        changed.notify_all();
                        return;
                    }
                    if let Some(job) = s.ready.pop_front() {
                        break job;
                    }
                    s = changed.wait(s).unwrap_or_else(PoisonError::into_inner);
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
                // A fillet's or chamfer's base is built, so every feature it's made from is:
                // their solids, to name the base's elements by.
                let lineage = match job {
                    Job::Make(key) => plan
                        .dresses
                        .get(&key)
                        .map(|dress| plan.lineage(dress.base, &s.done)),
                    Job::Measure(_) => None,
                };
                (job, inputs, lineage)
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
                            let lineage = lineage.unwrap_or_default();
                            match self.resolve(plan, dress, &lineage) {
                                Ok(edges) => {
                                    Ran::Made(self.dressed(*step, &inputs, &edges, generation))
                                }
                                Err(broken) => Ran::Unresolved(broken),
                            }
                        }
                        (Some(step), None) => Ran::Made(self.step(*step, &inputs, generation)),
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
                    // A solid that couldn't be measured is kept on disk unmeasured, so the
                    // next open measures it without making it again.
                    if let Some(Ok(built)) = s.done.get(&key) {
                        to_keep = Some(Arc::clone(built));
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

    /// Runs a fillet's or chamfer's step on the kernel, of `edges` (resolved), for
    /// `generation`.
    fn dressed(
        &self,
        step: Step,
        inputs: &[Arc<Built>],
        edges: &[u32],
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
            Ok(request) => map(self.kernel.submit(generation, request).wait(), |data| {
                Built::new(data.shape, Some(data.history), None)
            }),
            Err(e) => Outcome::Done(Err(e)),
        }
    }

    /// The edge numbers `dress`'s names find in its base feature's solid, its lineage's
    /// solids in `lineage`; or, for every name that finds none (or an edge whose name is
    /// unstable, N3), its `reference_broken` error, with candidates (N4, N6).
    fn resolve(
        &self,
        plan: &Plan,
        dress: &Dress,
        lineage: &BTreeMap<usize, Arc<Built>>,
    ) -> Result<Vec<u32>, crate::references::Broken> {
        let _span = tracing::debug_span!("resolve", edges = dress.edges.len()).entered();
        let known = Arc::clone(&lock(&self.state).known);
        let Some(map) = plan.map_of(dress.base, lineage, &known) else {
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
        }
    }
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
    generation: Generation,
    known: NamedMaps,
) -> Model {
    let done = &schedule.done;
    let mut features: Vec<(FeatureId, FeatureOutcome)> = Vec::with_capacity(plan.features.len());
    // The plan has one entry for each of the document's features, in its order.
    for (planned, feature) in plan.features.iter().zip(&source.document().features) {
        // A feature it uses that didn't build blocks it, on the failure at the root.
        let blocked = planned.uses.iter().find_map(|&i| match features.get(i) {
            Some((id, FeatureOutcome::Failed(_) | FeatureOutcome::Broken { .. })) => {
                Some(id.clone())
            }
            Some((_, FeatureOutcome::Blocked { on })) => Some(on.clone()),
            Some((_, FeatureOutcome::Built { .. } | FeatureOutcome::Sketch)) | None => None,
        });
        let makes_solid = feature.operation.makes_solid();
        let outcome = match blocked {
            Some(on) => FeatureOutcome::Blocked { on },
            None if !makes_solid => FeatureOutcome::Sketch,
            None => feature_outcome(planned, done, &schedule.unresolved),
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
            | FeatureOutcome::Sketch => None,
        })
        .collect();
    let naming = naming(plan, &features);
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
fn naming(plan: &Plan, features: &[(FeatureId, FeatureOutcome)]) -> Vec<Option<FeatureNaming>> {
    let mut naming: Vec<Option<FeatureNaming>> = Vec::with_capacity(features.len());
    for (planned, (id, outcome)) in plan.features.iter().zip(features) {
        let (FeatureOutcome::Built { key, .. }, Some(how)) = (outcome, planned.naming) else {
            naming.push(None);
            continue;
        };
        // (A feature whose inputs aren't named isn't either: naming it needs their maps.)
        let planned_key = planned
            .naming_key
            .filter(|_| planned.key() == Some(*key) && planned.id == *id);
        naming.push(planned_key.map(|key| FeatureNaming {
            key,
            how,
            uses: planned.uses.clone(),
        }));
    }
    naming
}

/// Feature `id`'s element map, as `how` names it: a primitive's by its numbers, an
/// operation's from its inputs' maps and its step's `history` (none for an operation: no
/// map). The one place maps are made, so the names a model gives and those a fillet's
/// references are found among are always the same.
fn named_as(
    id: &FeatureId,
    how: Naming,
    inputs: &[&ElementMap],
    history: Option<&History>,
) -> Option<ElementMap> {
    Some(match how {
        Naming::Primitive(primitive) => ElementMap::primitive(id.clone(), &primitive),
        Naming::Operation(operation) => {
            ElementMap::operation(id.clone(), operation, inputs, history?)
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
fn naming_key(key: Key, id: &FeatureId, how: Naming, inputs: &[NamingKey]) -> NamingKey {
    let mut hash = Sha256::new();
    hash.update(NAMING_SCHEME);
    hash.update(key.0);
    hash.update(id.as_str().as_bytes());
    hash.update([match how {
        Naming::Primitive(_) => 0,
        Naming::Operation(op) => op as u8 + 1,
    }]);
    for input in inputs {
        hash.update(input.0);
    }
    NamingKey(hash.finalize().into())
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

    /// A naming key holds the naming scheme, so a change to how elements are named makes a
    /// fillet's step another step: pinned, computed by hand with Python's hashlib from the
    /// scheme's line, the step's key, the ID, how it names (0 a primitive, 1 + the op code),
    /// and the inputs' naming keys.
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
            Naming::Primitive(Primitive::Box {
                dx: 10.0,
                dy: 20.0,
                dz: 30.0,
            }),
            &[],
        );
        assert_eq!(
            crate::hex(&block.0),
            "2a2f126374bb1252b1843e9c5d8a149e8cfce3af9d7b91421402d0173982113a"
        );
        let cut = naming_key(
            made,
            &id("bbbbbbbb"),
            Naming::Operation(OpCode::Cut),
            &[block],
        );
        assert_eq!(
            crate::hex(&cut.0),
            "35db188d9e9aa263e00cf6d1ab8bcfc2787444ef039fb8a86679bff2da8e9d8c"
        );
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
