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
//! one that makes no profile fails it with one error, the sketch's elements at fault and
//! every refusal of its profile in it. Its checks before
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
//! A recompute may read nothing kept ([`Recompute::start_fresh`], what `check`'s `fresh`
//! starts): every step built, every sketch solved, and every element map named anew, none
//! taken from the engine or the cache, though what it builds is kept in both as any
//! recompute's is, so the next recompute hits it. A failure the engine kept for a step's
//! values is asked again then, and a kernel fault (never kept) too.
//!
//! Every sketch is solved too, from the coordinates its document stores
//! (docs/design/m3.4-stored-coordinates.md), on the engine's [`SketchSolver`] (the sketch
//! worker, started at the first sketch, unless [`Recompute::with_sketches`] or
//! [`Recompute::with_solvers`] gave another),
//! one at a time: those a feature sweeps before the plan, the rest beside the kernel's
//! steps. A solve is keyed by its feature's ID and the
//! sketch's canonical text, and kept as steps' results are, so an unchanged sketch isn't
//! solved again; a superseded recompute's solve is stopped.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::cache::Cache;
use crate::document::PatternFeature;
use crate::document::{
    Checked, Document, ElementId, ElementName, Feature, FeatureId, FeatureIndex, Fingerprint, Made,
    Operation, SweepFeature,
};
use crate::elements::Entry;
use crate::face_plane::{Defaulted, Live, Placed};
use crate::kernel::{
    BooleanOp, BuiltData, DESCRIBE_BATCH, DescribedBatch, ElementKind, FaceRegions, Generation,
    History, Kernel, KernelRequest, Primitive, ShapeData, ShapeSummary, SweepKind,
};
use crate::naming::{Copied, ElementMap, Instance, Maker, NAMING_SCHEME, OpCode};
use crate::pattern::StepVerdict;
use crate::pool::{Outcome, Pending};
use crate::profile::Profile;
use crate::references::EdgeNames;
use crate::sketch::{
    Diagnosed, SketchSolution, SketchSolver, SketchWorker, SolveStopper, Translation,
};
use crate::sweep::plus_zero;
use crate::worker::lock;
use crate::{Error, ErrorCode};
use silico_protocol::{CopiesOp, Description};

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

impl Key {
    /// The key `text` writes, as [`Display`](fmt::Display) writes one: exactly 64 lowercase
    /// hex digits (a `meshes` request's `held` gives one back). None for any other text.
    pub fn parse(text: &str) -> Option<Key> {
        let digits = text.as_bytes();
        if digits.len() != 64 {
            return None;
        }
        let digit = |d: u8| match d {
            b'0'..=b'9' => Some(d - b'0'),
            b'a'..=b'f' => Some(d - b'a' + 10),
            _ => None,
        };
        let mut key = [0u8; 32];
        for (byte, pair) in key.iter_mut().zip(digits.chunks_exact(2)) {
            let [high, low] = pair else { return None };
            *byte = (digit(*high)? << 4) | digit(*low)?;
        }
        Some(Key(key))
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({self})")
    }
}

/// What a kernel step made: its shape, for an operation on other shapes what it did to
/// them, and, if it was measured, the shape's summary (from which a feature's fingerprint
/// comes), or why the kernel refused to measure it, if that follows from its values (so it
/// would refuse again); and, once it's summarized, what became of the description of each
/// of its faces, edges, and vertices (which element reads give:
/// docs/design/m4.1-command-api.md, *Where the entries come from*). A step that's a
/// feature's solid is measured after it's made, by a job of its own, which puts a measured
/// copy in its place. Never changed once made: models share it.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub shape: ShapeData,
    pub history: Option<History>,
    summary: Option<ShapeSummary>,
    refused: Option<Error>,
    described: Option<Described>,
}

/// What became of a summarized shape's description. A description that couldn't be had
/// leaves the summary as it is: a sweep's checks and its fingerprint read the summary
/// alone, so only element reads report it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Described {
    /// Described whole: each of its faces, edges, and vertices, in the kernel's numbering.
    Whole(Description),
    /// Refused for the shape's values ([`kept`]), or hung ([`hung`]): it would be again, so
    /// it isn't asked again in the session, nor, if it lasts (`cache::lasting`: a refusal
    /// for invalid input or an invalid result, never a hang), from the cache.
    Refused(Error),
    /// Any other failure of a worker (a crash, a reply no shape has): asked again by the
    /// next recompute.
    Failed(Error),
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
            described: None,
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
            described: None,
        }
    }

    /// The shape's summary, if it was measured.
    pub fn summary(&self) -> Option<&ShapeSummary> {
        self.summary.as_ref()
    }

    /// Each of the shape's faces, edges, and vertices described, in the kernel's numbering
    /// (`TopExp::MapShapes` order), if it was.
    pub fn description(&self) -> Option<&Description> {
        match &self.described {
            Some(Described::Whole(description)) => Some(description),
            Some(Described::Refused(_) | Described::Failed(_)) | None => None,
        }
    }

    /// Why the shape's elements couldn't be described, if they couldn't: refused for its
    /// values, or a worker's failure.
    pub fn why_undescribed(&self) -> Option<&Error> {
        match &self.described {
            Some(Described::Refused(error) | Described::Failed(error)) => Some(error),
            Some(Described::Whole(_)) | None => None,
        }
    }

    /// What became of its description, if it was asked for.
    pub(crate) fn described(&self) -> Option<&Described> {
        self.described.as_ref()
    }

    /// Why the kernel refused to measure the shape, if it did for its values.
    pub fn measure_refused(&self) -> Option<&Error> {
        self.refused.as_ref()
    }

    /// Whether a measure job has something to do for it: a summary not refused, or, once
    /// summarized, a description not yet had or refused (one a worker failed is asked
    /// again).
    fn needs_measure(&self) -> bool {
        match (&self.summary, &self.described) {
            (None, _) => self.refused.is_none(),
            (Some(_), None | Some(Described::Failed(_))) => true,
            (Some(_), Some(Described::Whole(_) | Described::Refused(_))) => false,
        }
    }

    /// This result, summarized.
    fn summarized(&self, summary: ShapeSummary) -> Self {
        Built {
            summary: Some(summary),
            ..self.clone()
        }
    }

    /// This result, with what became of its description.
    pub(crate) fn with_described(&self, described: Described) -> Self {
        Built {
            described: Some(described),
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

/// The most batches of one solid's description in flight at once: each carries the solid's
/// B-rep, so it bounds the copies held, while the pool's workers describe in parallel. Only
/// a solid of more than one batch of a kind is described so: a smaller one's batches are
/// quick, and asking them at once would start a worker for each (a small document's check
/// starting three where it needs one).
const DESCRIBE_AT_ONCE: usize = 4;

/// What became of a feature in a recompute.
#[derive(Debug, Clone, PartialEq)]
pub enum FeatureOutcome {
    /// Built: its last step's key and result.
    Built { key: Key, built: Arc<Built> },
    /// The kernel refused one of its steps, or the rebuild refused it before any: why,
    /// naming the feature. A pad, pocket, revolve, or groove whose sketch solved but makes
    /// no profile fails with one error, its elements the sketch's at fault, the profile's
    /// refusals its cause ([`crate::sweep::unprofiled`]; docs/design/m3.6-sketch-features.md,
    /// S3).
    Failed(Error),
    /// References it holds (a fillet's or chamfer's edges, by name) name nothing it can
    /// use: each broken one's `reference_broken` error, in the order the feature holds them
    /// (N6: every one, at once), the first [`PROBLEMS_LISTED`](crate::document::PROBLEMS_LISTED)
    /// of them, and how many there are, listed or not. Its step isn't run.
    Broken { errors: Vec<Error>, total: u64 },
    /// Not built, since it uses a feature that failed: that feature (the first failure
    /// along its base, then its tool).
    Blocked { on: FeatureId },
    /// A sketch, solved: it makes no solid. One that didn't solve is `Failed`, with why.
    Sketch(Arc<SketchSolution>),
    /// Suppressed (the M5.0 design's `suppress_feature`): not built, nor a sketch solved. A
    /// feature that consumes it builds on what stands for it, the nearest unsuppressed
    /// feature up its chain.
    Suppressed,
}

impl FeatureOutcome {
    /// Why it failed, each error naming the feature: its failure, or its broken references
    /// (the first hundred); none if it built, is blocked on another, is a sketch, or is
    /// suppressed.
    pub fn errors(&self) -> &[Error] {
        match self {
            FeatureOutcome::Failed(error) => std::slice::from_ref(error),
            FeatureOutcome::Broken { errors, .. } => errors,
            FeatureOutcome::Built { .. }
            | FeatureOutcome::Blocked { .. }
            | FeatureOutcome::Sketch(_)
            | FeatureOutcome::Suppressed => &[],
        }
    }
}

/// A document as rebuilt: what became of each feature, in timeline order, and its bodies.
#[derive(Debug, Clone)]
pub struct Model {
    /// The document it was built from.
    source: Arc<Checked>,
    generation: Generation,
    /// What became of each feature: its document's features, in its order, one for one.
    /// So a feature's place in the document is its place here, found by the document's
    /// index ([`Model::place`]); and [`drifted`](Self::drifted) and
    /// [`saved_as`](Self::saved_as) pair the two lists in order. [`rebound`](Self::rebound)
    /// keeps it: its new document has the same features, in the same order.
    features: Vec<(FeatureId, FeatureOutcome)>,
    bodies: Vec<FeatureId>,
    /// Built features whose solids couldn't be measured, in timeline order: why.
    unmeasured: Vec<(FeatureId, Error)>,
    /// Each feature's place in `unmeasured`, by ID: derived from it, whose IDs nothing
    /// changes once it's built.
    unmeasured_index: Arc<FeatureIndex>,
    /// How each feature's solid's elements are named, in timeline order: none for one that
    /// didn't build, or a sketch, which makes no solid.
    naming: Vec<Option<FeatureNaming>>,
    /// Each feature's element names: named in the background once the model is published
    /// (after its recompute has replied, so publishing doesn't wait for them: naming every
    /// element of every feature before replying doubled the cost of a long timeline on the
    /// fake), or, asked for sooner, then, with those of the features it's made from. They're
    /// a function of the model's own data alone, so they're the same whenever they're
    /// named. Shared, as `entries` and `live` are, with every copy of the model, a clone or
    /// one [`rebound`](Self::rebound) to its saved document: each has the same steps and
    /// outcomes, so the same names, and the background's naming of the model it publishes
    /// reaches the copy a save publishes too, which no one else names. A copy whose
    /// outcomes differ is given slots of its own ([`unnamed`](Self::unnamed)).
    maps: Arc<[OnceLock<Option<Arc<ElementMap>>>]>,
    /// Each feature's solid's elements as entries (`elements` and `query`'s), built from its
    /// description and its names the first time they're read, once for the model, so paging
    /// through a large solid costs one build and a copy of each page, not a build a page.
    /// Of its outcomes and names alone, never its document, so shared as `maps` is.
    entries: Arc<[OnceLock<Entries>]>,
    /// Each feature's live body, by index ([`Model::live_body`]): built the first time a
    /// name is looked for in every body, once for the model, from `bodies` and `naming`,
    /// which nothing changes once it's built; shared as `maps` is.
    live: Arc<OnceLock<Vec<Option<usize>>>>,
    /// The engine's maps named so far.
    known: NamedMaps,
    /// Each sketch's diagnosis, paired with its solve's outcome, by its ID, from the answer
    /// its rebuild's solve read ([`Model::diagnosed`]); or why it has none.
    diagnoses: Arc<BTreeMap<FeatureId, Result<Arc<Diagnosed>, Error>>>,
    /// Each sketch on a face whose face was resolved, by its ID: its frame
    /// ([`Model::frame`]).
    frames: Arc<BTreeMap<FeatureId, silico_protocol::FaceFrame>>,
    /// What each pattern's steps' histories say of its copies, by its ID, each step that
    /// built in order ([`Model::pattern_verdicts`]).
    patterns: Arc<BTreeMap<FeatureId, Arc<[StepVerdict]>>>,
}

/// How many features [`Model::element_map`]'s reads have visited on this thread so far:
/// one a read of a feature named already, or of a primitive.
#[cfg(test)]
pub(crate) fn walked() -> u64 {
    WALKED.with(std::cell::Cell::get)
}

/// How many visits [`Model::live_bodies`]' second pass has made on this thread so far:
/// one for each feature whose uses it read, and one for each of those.
#[cfg(test)]
pub(crate) fn live_visits() -> u64 {
    LIVE_VISITS.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// How many features [`Model::element_map`]'s reads have visited on this thread (one
    /// for a read of a feature named already): a test's measure of a read's walk down what
    /// a feature's made from.
    static WALKED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// What [`live_visits`] says.
    static LIVE_VISITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A feature's solid's elements as entries, or why they couldn't be built
/// ([`Model::entries`]).
type Entries = Result<Arc<[Entry]>, Error>;

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
/// every feature came to the same: names are a function of those, and the index of the
/// unmeasured solids is derived from them.
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
            .get(self.place(id)?)
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
                | FeatureOutcome::Sketch(_)
                | FeatureOutcome::Suppressed => 0,
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
    /// broke (its first hundred: [`Model::error_count`] counts them all). A pad, pocket,
    /// revolve, or groove refused for its sketch (no profile, several regions, an axis
    /// across it) has one error, naming it, whose elements are the sketch's at fault; the
    /// sketch, solved, has none (design S3, S5, S6). But a kernel fault met describing the
    /// solid a face sketch's face is on, which names no feature: kept as met, so it's
    /// worded as the solid's read words it, and a read names the sketch it stopped
    /// (`elements::worded_of`).
    pub fn errors(&self) -> impl Iterator<Item = &Error> {
        self.features
            .iter()
            .flat_map(|(_, outcome)| outcome.errors())
    }

    /// The first kernel fault the rebuild met, in timeline order: a failure that says
    /// nothing about the document (a worker that crashed, hung, or couldn't be had, a reply
    /// that couldn't be read, a shortage, a bug), in a feature's failure or in a measure of
    /// its solid. One, however many features it stopped: a sketch solver that couldn't
    /// start fails every sketch with it, and is still one fault (docs/design/m4.1-command-api.md,
    /// section 6, after AR1). None if the rebuild met none.
    pub fn fault(&self) -> Option<&Error> {
        let mut unmeasured = self.unmeasured.iter().peekable();
        for (id, outcome) in &self.features {
            if let Some(fault) = outcome.errors().iter().find(|e| e.code().is_kernel_fault()) {
                return Some(fault);
            }
            if let Some((_, error)) = unmeasured.next_if(|(feature, _)| feature == id)
                && error.code().is_kernel_fault()
            {
                return Some(error);
            }
        }
        None
    }

    /// The names of the elements of the feature `id`'s solid, if it built. The recompute
    /// that published the model names every feature's elements in the background once
    /// it's published; asked before then, this names the feature's now, on the caller's
    /// thread, with those of the features it's made from (and no others).
    pub fn element_map(&self, id: &FeatureId) -> Option<&ElementMap> {
        let i = self.place(id)?;
        // Named already (the model's named, or this one read before): its map, or none, is
        // settled, so it's read as it is, with nothing walked or allocated. A visit to its own
        // feature only, as the walk below would make it.
        if let Some(map) = self.maps.get(i)?.get() {
            #[cfg(test)]
            WALKED.with(|walked| walked.set(walked.get() + 1));
            return map.as_deref();
        }
        // Those it's made from not named yet, all the way down, in timeline order, so each
        // is named after those it's made from. Not past one named already (its map, or
        // none, is settled, so it needs nothing it's made from): once the model's named,
        // a read visits its own feature only, however deep the timeline. An explicit stack:
        // a timeline can be thousands deep.
        let mut needed = std::collections::BTreeSet::new();
        let mut stack = vec![i];
        while let Some(j) = stack.pop() {
            #[cfg(test)]
            WALKED.with(|walked| walked.set(walked.get() + 1));
            let named = self.maps.get(j).is_some_and(|slot| slot.get().is_some());
            if !named
                && needed.insert(j)
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

    /// Each of the feature `id`'s solid's faces, edges, and vertices described, in the
    /// kernel's numbering, if it built and they were: described as part of the rebuild,
    /// before the model was published, so reading it asks the kernel nothing. Why they
    /// weren't, if they couldn't be, is its `Built::why_undescribed`.
    pub fn description(&self, id: &FeatureId) -> Option<&Description> {
        match self.outcome(id)? {
            FeatureOutcome::Built { built, .. } => built.description(),
            FeatureOutcome::Failed(_)
            | FeatureOutcome::Broken { .. }
            | FeatureOutcome::Blocked { .. }
            | FeatureOutcome::Sketch(_)
            | FeatureOutcome::Suppressed => None,
        }
    }

    /// Each of the feature `id`'s solid's elements as an entry, in the element list's order
    /// (`elements::entries`), if it built and was described ([`Model::description`]): built
    /// the first time they're asked for, naming its elements if they aren't yet, and kept for
    /// the model. A fault if they couldn't be named, or don't fit an entry.
    pub fn entries(&self, id: &FeatureId) -> Option<Result<&[Entry], &Error>> {
        let description = self.description(id)?;
        let slot = self.entries.get(self.place(id)?)?;
        let built = slot.get_or_init(|| match self.element_map(id) {
            Some(map) => crate::elements::entries(map, id, description).map(Arc::from),
            None => Err(crate::elements::fault(
                id,
                "its solid built, but its elements couldn't be named",
            )),
        });
        Some(built.as_deref())
    }

    /// The names of the elements of the feature `id`'s solid if they've been named
    /// already, without naming them, so a caller that mustn't wait (the UI's thread)
    /// never does.
    pub fn named_element_map(&self, id: &FeatureId) -> Option<&ElementMap> {
        self.maps.get(self.place(id)?)?.get()?.as_deref()
    }

    /// The feature `id`'s element map as named so far, never naming it, so a caller that
    /// mustn't wait never does: `None` while its names aren't settled yet (the background
    /// names them once the model is published, [`Recompute::start`]); `Some(None)` once
    /// settled with none (a feature that didn't build, a sketch, one whose names couldn't be
    /// made, or one the document hasn't).
    pub(crate) fn named_so_far(&self, id: &FeatureId) -> Option<Option<&ElementMap>> {
        let Some(slot) = self.place(id).and_then(|i| self.maps.get(i)) else {
            return Some(None);
        };
        slot.get().map(|map| map.as_deref())
    }

    /// This model with none of its elements named yet, as it is when it's published and
    /// before the background has named any: so a test can read it as a client that asks
    /// at once does, with no race against that naming (the app's tests read it as its
    /// selection does a model published a moment ago). Its slots are its own, shared with
    /// no other model, so naming either names nothing of the other.
    #[cfg(any(test, feature = "fake"))]
    pub fn unnamed(&self) -> Model {
        Model {
            maps: (0..self.features.len()).map(|_| OnceLock::new()).collect(),
            entries: (0..self.features.len()).map(|_| OnceLock::new()).collect(),
            live: Arc::default(),
            ..self.clone()
        }
    }

    /// This model with the feature `id`'s elements settled as never named (its solid built,
    /// but its names couldn't be made: Silico's own bug, which no document reaches on the
    /// fake), every other feature's as named so far: so a test can read a solid whose names
    /// couldn't be made, as a client of the published model would find it.
    #[cfg(any(test, feature = "fake"))]
    pub fn unnamable(&self, id: &FeatureId) -> Model {
        let maps = self
            .features
            .iter()
            .zip(self.maps.iter())
            .map(|((feature, _), map)| {
                if feature == id {
                    OnceLock::from(None)
                } else {
                    map.clone()
                }
            })
            .collect();
        Model {
            maps,
            entries: (0..self.features.len()).map(|_| OnceLock::new()).collect(),
            ..self.clone()
        }
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

    /// The place of the feature `id` in the timeline, if the document has it, here as in
    /// the document (`features`): found by the document's index, in logarithmic time, so a
    /// read that looks up each feature isn't quadratic.
    fn place(&self, id: &FeatureId) -> Option<usize> {
        self.source.place(id)
    }

    /// The body the feature `id`'s solid ends up in, if one does: the feature itself, if
    /// it's a body, or the body made from it, through the feature that consumes it, the one
    /// that consumes that, and so on. One at most: the gate lets each solid be consumed
    /// once (`document::validate`), so a feature's consumers make one chain, and it ends at
    /// one body. A feature's map names its elements from its inputs' maps (its `uses`, the
    /// features it consumes) and as its own, so every name in a body's element map is of
    /// that body or of a feature in its chain; never of a sketch, whose names a sweep makes
    /// its own (`OpCode::side`: a sweep keeps neither side of its profile), and whose own
    /// names no body's map holds. So a name's element can be in no body but its feature's
    /// live body: a name is looked for there, not in every body. None for a feature that
    /// didn't build, a sketch, and a feature the document hasn't. (A built feature always
    /// has its naming, whose key is made from its inputs', all built, so each solid a built
    /// feature consumes is in its `uses`.)
    pub(crate) fn live_body(&self, id: &FeatureId) -> Option<&FeatureId> {
        let live = self.live.get_or_init(|| self.live_bodies());
        let body = (*live.get(self.place(id)?)?)?;
        self.features.get(body).map(|(id, _)| id)
    }

    /// Each feature's live body's place in the timeline ([`live_body`](Self::live_body)),
    /// in two passes. The first walks the timeline forward beside the bodies (built
    /// features none consumes, in timeline order), each a feature's place found with no
    /// lookup: a body is its own. The second walks it back: each solid a feature with a
    /// live body consumes (by its naming's `uses`) takes that live body; a feature's
    /// consumer comes after it, so its live body is known by the time it's reached. Each
    /// feature visited once a pass, and each use once: linear in the timeline.
    fn live_bodies(&self) -> Vec<Option<usize>> {
        let _span = tracing::debug_span!("live_bodies", features = self.features.len()).entered();
        let mut live = vec![None; self.features.len()];
        let mut bodies = self.bodies.iter().peekable();
        for (i, (id, _)) in self.features.iter().enumerate() {
            if bodies.next_if(|body| *body == id).is_some()
                && let Some(slot) = live.get_mut(i)
            {
                *slot = Some(i);
            }
        }
        for j in (0..live.len()).rev() {
            let Some(body) = live.get(j).copied().flatten() else {
                continue;
            };
            // The features whose solids it's made from, by place, as its naming has them.
            let uses = self
                .naming
                .get(j)
                .and_then(Option::as_ref)
                .map_or(&[][..], |naming| naming.uses.as_slice());
            // Counted under test (`live_visits`): one visit for the feature, and one for
            // each it uses.
            #[cfg(test)]
            LIVE_VISITS.with(|visits| visits.set(visits.get() + 1 + uses.len() as u64));
            for &used in uses {
                if let Some(slot) = live.get_mut(used) {
                    *slot = Some(body);
                }
            }
        }
        live
    }

    /// The feature `id` as its document holds it, found by the document's index.
    pub(crate) fn feature(&self, id: &FeatureId) -> Option<&Feature> {
        self.source.feature(id).ok()
    }

    /// What the feature `id` is now, as [`Name::parse`](crate::naming::Name::parse) asks
    /// it of each feature a name mentions: none if the document has no such feature.
    pub(crate) fn maker(&self, id: &FeatureId) -> Option<Maker> {
        self.feature(id).map(|f| Maker::of(&f.operation))
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

    /// What pattern `id`'s steps' histories say of its copies (docs/design/m6.0-patterns.md,
    /// section 4.2, *Every verdict kept*): one verdict a step, each original's in timeline
    /// order, as far as its steps built; none for a feature that isn't a pattern with a
    /// copy placed (the identity has none).
    pub fn pattern_verdicts(&self, id: &FeatureId) -> Option<&[StepVerdict]> {
        self.patterns.get(id).map(|verdicts| &**verdicts)
    }

    /// The document it was built from.
    pub fn document(&self) -> &Document {
        self.source.document()
    }

    /// The sketch `id`'s diagnosis, paired with its solve's outcome, as its rebuild's solve
    /// read them from one answer (the definition states are read from it,
    /// [`crate::sketch_states::states`]); or why there's none: the solver failed, or its reply
    /// couldn't be read, its error naming the sketch. None for a feature that isn't a sketch
    /// this model solved.
    pub fn diagnosed(&self, id: &FeatureId) -> Option<Result<&Diagnosed, &Error>> {
        self.diagnoses
            .get(id)
            .map(|diagnosed| diagnosed.as_ref().map(AsRef::as_ref))
    }

    /// The frame the sketch `id`, on a face, sits on, as its rebuild resolved it from the
    /// face (docs/ports/attacher.md, F1 and F2): none for a sketch on an origin plane, one
    /// whose face couldn't be resolved, and a feature that isn't a sketch.
    pub fn frame(&self, id: &FeatureId) -> Option<&silico_protocol::FaceFrame> {
        self.frames.get(id)
    }

    /// This model as `source`'s: the same steps and outcomes, its document `source`. For a
    /// document that differs from the one it was built from only in what a save writes from
    /// it (its fingerprints, with no sketch's solve written), whose rebuild would take every
    /// step from those this one took: a session's save (a commit, an edit's, `format`'s),
    /// whose model is the saved revision's (the design's section 4). Its names, entries, and
    /// live bodies are this model's, shared (its `maps`, `entries`, and `live`): of the
    /// same steps and outcomes, never the document, so the same; named in the background as
    /// this one is, though only this one is the engine's.
    pub(crate) fn rebound(&self, source: Arc<Checked>) -> Model {
        Model {
            source,
            ..self.clone()
        }
    }

    /// The fingerprint of the feature `id`'s solid, if it built and was measured: none if it
    /// didn't (or the document has no such feature), or if a measure can't be written.
    pub fn fingerprint(&self, id: &FeatureId) -> Option<Fingerprint> {
        match self.outcome(id)? {
            FeatureOutcome::Built { built, .. } => built.summary().and_then(Fingerprint::of),
            FeatureOutcome::Failed(_)
            | FeatureOutcome::Broken { .. }
            | FeatureOutcome::Blocked { .. }
            | FeatureOutcome::Sketch(_)
            | FeatureOutcome::Suppressed => None,
        }
    }

    /// Each feature of its document whose record doesn't hold, in timeline order: one
    /// whose solid couldn't be measured (that error, naming it); one whose rebuild measures
    /// otherwise than the fingerprint stored with it (a `drift` error naming the feature
    /// and what moved); and a sketch whose solve differs from what its file holds (a
    /// `drift` error naming the sketch and, as elements, every point, circle, and reference
    /// dimension the solve changes). A feature with no fingerprint stored isn't drift (a
    /// commit whose edits reach it, or `format`, writes one), nor is one that didn't build
    /// or solve (its failure says why).
    pub fn drift(&self) -> Vec<Error> {
        self.drift_except(&BTreeSet::new())
    }

    /// [`drift`](Self::drift), but none of a feature in `except` whose record a save writes
    /// ([`records`](Self::records)): one a session's edits reached since its last save
    /// (docs/design/m4.1-command-api.md, decision 28), whose stored fingerprint, or a
    /// sketch's stored solve, was made from other inputs, so the rebuild isn't held to it
    /// (the commit writes its new one), or one `format` accepts the drift of. A feature in
    /// `except` whose record can't be written (it measures past what a fingerprint holds) is
    /// still said, and so is a solid that couldn't be measured: nothing takes the stored
    /// record's place.
    pub fn drift_except(&self, except: &BTreeSet<FeatureId>) -> Vec<Error> {
        self.drift_left(&self.drifted(), except)
            .map(|(_, error)| error.clone())
            .collect()
    }

    /// Of `drift` (this model's, [`drifted`](Self::drifted)), what's left with `except`
    /// excepted, as [`drift_except`](Self::drift_except) excepts them.
    pub(crate) fn drift_left<'a>(
        &'a self,
        drift: &'a [(FeatureId, Error)],
        except: &'a BTreeSet<FeatureId>,
    ) -> impl Iterator<Item = &'a (FeatureId, Error)> {
        drift.iter().filter(|(id, error)| {
            !(error.code() == ErrorCode::Drift && except.contains(id) && self.records(id))
        })
    }

    /// Why the feature `id`'s built solid couldn't be measured, if it couldn't: the kernel's
    /// refusal of its values (which its [`Built::measure_refused`] holds too, once kept) or
    /// a worker's failure (a crash, a timeout, a reply no shape has), as
    /// [`drift`](Self::drift) says it.
    pub fn unmeasured(&self, id: &FeatureId) -> Option<&Error> {
        let place = self
            .unmeasured_index
            .get(id, |place| self.unmeasured.get(place).map(|(id, _)| id))?;
        self.unmeasured.get(place).map(|(_, error)| error)
    }

    /// [`drift`](Self::drift), each error with the ID of the feature it's of, computed once
    /// for a caller that both picks features by their drift and says it.
    pub fn drifted(&self) -> Vec<(FeatureId, Error)> {
        let mut errors = Vec::new();
        let mut unmeasured = self.unmeasured.iter().peekable();
        for (feature, (_, outcome)) in self.document().features.iter().zip(&self.features) {
            if let Some((_, error)) = unmeasured.next_if(|(id, _)| *id == feature.id) {
                errors.push((feature.id.clone(), error.clone()));
                continue;
            }
            if let (Operation::Sketch(stored), FeatureOutcome::Sketch(solution)) =
                (&feature.operation, outcome)
            {
                let moved = crate::sketch::moved(stored, &solution.written(stored));
                if let Some(first) = moved.first() {
                    let said: Vec<&str> = moved.iter().take(5).map(|m| m.said.as_str()).collect();
                    let more = if moved.len() > 5 { ", …" } else { "" };
                    errors.push((
                        feature.id.clone(),
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
                        .with_elements(moved.iter().map(|m| m.id.as_str()))
                        .with_parameter(first.field.as_str())
                        .with_suggestion(SKETCH_DRIFT),
                    ));
                }
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
            errors.push((
                feature.id.clone(),
                error
                    .with_feature(feature.id.as_str())
                    .with_suggestion(DRIFT),
            ));
        }
        errors
    }

    /// Its document as `silico fmt` writes it: each feature's fingerprint as it rebuilt (a
    /// suppressed one's as stored), and each sketch as it solved (its points, circles' radii, and reference dimensions'
    /// values: [`SketchSolution::written`]). A feature that didn't rebuild or solve, or whose
    /// solid couldn't be measured or written, keeps what's stored with it: there's nothing to
    /// put in its place, and a failure that passes (a worker that hung, say) mustn't lose the
    /// record a later rebuild is checked against.
    pub fn saved(&self) -> Document {
        self.saved_as(|_| Save::Recorded)
    }

    /// Whether a save writes the record of the feature `id` as rebuilt (the rule
    /// [`saved_as`](Self::saved_as) writes by): a fingerprint for a feature that built and
    /// was measured, as a fingerprint can hold it, or the solve of a sketch that solved. Not
    /// for one that failed, was blocked, or couldn't be measured, which keeps what's stored
    /// with it, unless a session's edits reached it ([`Save::AsRebuilt`]).
    pub fn records(&self, id: &FeatureId) -> bool {
        self.outcome(id).and_then(Record::of).is_some()
    }

    /// Its document as a save writes it, each feature as `how` says ([`Save`]), but a
    /// suppressed one, whose stored fingerprint is carried as stored, never checked or
    /// rewritten, whatever `how` says: the one place the rule is held. A session's commit saves the features its edits reached as
    /// rebuilt and every other as stored (decision 28); its `format` saves the reached ones
    /// as rebuilt, those whose drift it isn't told to accept as stored, and the rest as
    /// recorded (decision 31); [`saved`](Self::saved), every feature as recorded.
    pub fn saved_as(&self, how: impl Fn(&FeatureId) -> Save) -> Document {
        let mut document = self.document().clone();
        // The model's features are its document's, in order.
        for (feature, (_, outcome)) in document.features.iter_mut().zip(&self.features) {
            // A suppressed feature's stored fingerprint is carried as stored, never checked
            // or rewritten (M5.0's A10, as amended), whatever reached it; and a suppressed
            // sketch keeps its coordinates, unsolved.
            if feature.suppressed {
                continue;
            }
            let how = how(&feature.id);
            if how == Save::Stored {
                continue;
            }
            match Record::of(outcome) {
                Some(Record::Fingerprint(rebuilt)) => feature.fingerprint = Some(rebuilt),
                Some(Record::Solve(solution)) => {
                    if let Operation::Sketch(sketch) = &mut feature.operation {
                        *sketch = solution.written(sketch);
                    }
                }
                // A sketch's coordinates are its data, not a record of it: kept.
                None if how == Save::AsRebuilt && feature.operation.makes_solid() => {
                    feature.fingerprint = None;
                }
                None => {}
            }
        }
        document
    }
}

/// How a save writes a feature ([`Model::saved_as`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Save {
    /// As the file holds it.
    Stored,
    /// Its record if it has one (its fingerprint as rebuilt, or a sketch's solve), or else
    /// as stored: a feature that didn't build keeps what's stored with it, since a failure
    /// that passes (a worker that hung, say) mustn't lose the record a later rebuild is
    /// checked against.
    Recorded,
    /// As it rebuilt, for a feature a session's edits reached (decision 28, amended): its
    /// record if it has one; or, for a feature that makes a solid and didn't build (it
    /// failed, a reference of its broke, a failure upstream blocked it, a kernel fault
    /// stopped it) or built to measures a fingerprint can't hold, no fingerprint, since
    /// what's stored was measured from the values the edit replaced and is no record of it.
    /// A sketch that didn't solve keeps its stored coordinates, which are its data.
    AsRebuilt,
}

/// What a save writes of a feature as rebuilt: its fingerprint, or a sketch's solve.
enum Record<'a> {
    Fingerprint(Fingerprint),
    Solve(&'a SketchSolution),
}

impl<'a> Record<'a> {
    /// What a save writes for a feature that came to `outcome`: none for one that failed or
    /// was blocked, or whose solid couldn't be measured, or measures past what a fingerprint
    /// holds, which keeps what's stored with it, unless a session's edits reached it
    /// ([`Save::AsRebuilt`]).
    fn of(outcome: &'a FeatureOutcome) -> Option<Self> {
        match outcome {
            FeatureOutcome::Built { built, .. } => built
                .summary()
                .and_then(Fingerprint::of)
                .map(Record::Fingerprint),
            FeatureOutcome::Sketch(solution) => Some(Record::Solve(solution)),
            FeatureOutcome::Failed(_)
            | FeatureOutcome::Broken { .. }
            | FeatureOutcome::Blocked { .. }
            | FeatureOutcome::Suppressed => None,
        }
    }
}

/// What to do about drift: most often the feature, or one it uses, was edited (by hand, or
/// in a merge) since it was saved; otherwise the kernel's results changed.
const DRIFT: &str = "if this feature or one it uses was edited by hand or in a merge since it was saved, run format (or silico fmt) to accept its new fingerprint; if not, this is a determinism bug or a change in the kernel: find which before accepting it";

/// What to do about a sketch's drift: as for a fingerprint's ([`DRIFT`]), for a sketch.
const SKETCH_DRIFT: &str = "if this sketch was edited by hand or in a merge since it was saved, run format (or silico fmt) to accept its new solve; if not, this is a determinism bug or a change in the sketch solver: find which before accepting it";

/// What a recompute came to.
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// It finished, and its model is now the published one.
    Published(Arc<Model>),
    /// A newer recompute started first: nothing of this one was published.
    Superseded,
}

/// A recompute running in the background: wait for it, poll it, read how far it has got
/// ([`progress`](Self::progress)), or drop it (it runs on).
#[must_use = "a recompute's outcome is only had by waiting for it"]
pub struct Run {
    outcome: Receiver<Result<RunOutcome, Error>>,
    progress: StepCounter,
}

/// How far a recompute has got (docs/design/m4.1-command-api.md, section 6): its steps
/// done, its steps, and the step running, counted as its schedule runs them, under a lock
/// of its own that's held only to count or to read the count, never across a step: so
/// reading it never waits on the recompute's work. A diagnostic, not a result (section
/// 10): which step finishes first depends on scheduling. Clones share the count. Each
/// change is told to whoever waits for one ([`changed_since`](Self::changed_since)): the
/// session's watcher of its rebuild, which wakes the client as the count changes. Its clock
/// (`elapsed`) starts as it's made, on the monotonic clock: a recompute's, as it's asked for
/// ([`Recompute::begin`]).
#[derive(Clone, Default)]
pub struct StepCounter(Arc<(Mutex<Counted>, Condvar)>);

/// A recompute's count of its steps, as [`StepCounter::now`] reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepCount {
    /// The steps settled: built, failed, kept from before, or never to run (an input
    /// failed, or only refused features need it). All of them once the recompute finishes.
    /// Each counted once, by its key, however many rounds settle it: it never counts a step
    /// twice, and never goes back, round to round.
    pub done: usize,
    /// The steps of its plan: none until its plan is made (after the sketches it sweeps are
    /// solved). A rebuild with sketches on faces plans in rounds, each adding the steps of
    /// the features whose faces the rounds before it placed (docs/ports/attacher.md, *In
    /// rounds*): so this grows, round to round, and never shrinks.
    pub steps: usize,
    /// The step started last of those running, as its feature and the feature's type
    /// (`f0000001 fillet`): none while no step runs.
    pub running: Option<String>,
    /// Whether the recompute has ended: finished, superseded, or failed.
    pub ended: bool,
    /// Each unsuppressed feature's state, in timeline order: listed from the recompute's
    /// start (each `queued`), and kept once it ends. A diagnostic, as `done` is: which
    /// feature runs first depends on scheduling.
    pub features: FeatureStates,
    /// How long the recompute has run, on the monotonic clock: from its being asked for
    /// ([`Recompute::begin`], before its thread starts) to its count's end (its model
    /// published, or its failure), or to its stop (a cancel, a newer recompute, or its
    /// engine's drop), though its thread may yet drain the requests it has running then;
    /// so far while it runs, and fixed once it ends or stops. A diagnostic, a clock, never a
    /// result (section 10).
    pub elapsed: Duration,
}

/// What's become of a feature's steps in a rebuild (`status.rebuild.features`, issue #367):
/// as the timeline draws it while the rebuild runs, and after it stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    /// Not settled yet: none of its steps running (none started, or one built and the next
    /// not started yet), or its own failure waits on the checks of what it uses, or the
    /// rebuild stopped before it settled; Rebuild resumes it.
    Queued,
    /// A step it owns is running; a sketch, its solve; a sketch on a face reads queued until
    /// the round that places it, its solve included, then its outcome: its result is the
    /// sketch placed on its face.
    Running,
    /// Its solid made (its last step settled with a result, or kept from before); a sketch,
    /// solved.
    Built,
    /// Its own problem: a step the kernel refused, a check its result failed, a refusal of
    /// its sketch's profile, a sketch that didn't solve, broken references.
    Failed,
    /// Settled unrun: a feature it uses failed, faulted, or was blocked. Mid-run, it may
    /// read blocked while the input it's blocked on still reads built or queued (that
    /// input's own failure is held until what it uses is firm); it settles to the model's
    /// outcome, and is never taken back.
    Blocked,
    /// A kernel fault stopped it (a worker that crashed, hung, or couldn't be had, a reply
    /// that couldn't be read, a shortage, a bug); what uses it is blocked.
    Faulted,
}

impl StepState {
    /// Every state, in the order the wire's schema lists them.
    pub const ALL: [StepState; 6] = [
        StepState::Queued,
        StepState::Running,
        StepState::Built,
        StepState::Failed,
        StepState::Blocked,
        StepState::Faulted,
    ];

    /// As `status` writes it.
    pub const fn as_str(self) -> &'static str {
        match self {
            StepState::Queued => "queued",
            StepState::Running => "running",
            StepState::Built => "built",
            StepState::Failed => "failed",
            StepState::Blocked => "blocked",
            StepState::Faulted => "faulted",
        }
    }

    /// Failed, or faulted if `error` is a kernel fault.
    fn failed(error: &Error) -> Self {
        if error.code().is_kernel_fault() {
            StepState::Faulted
        } else {
            StepState::Failed
        }
    }

    /// Whether a step's settle or start may move a feature from this state to `to`, as a
    /// rebuild runs: a failure or fault, once said, stays (the first wins, as the model's
    /// outcome does); a blocked feature is never failed by a step that ran though it was no
    /// longer needed; and only a running feature goes back to queued.
    fn may_become(self, to: StepState) -> bool {
        use StepState::{Blocked, Built, Failed, Faulted, Queued, Running};
        match to {
            Running => self == Queued,
            Queued => self == Running,
            Built => matches!(self, Queued | Running),
            Failed | Faulted | Blocked => matches!(self, Queued | Running | Built),
        }
    }

    /// The state a model's outcome says: none for a suppressed feature, which isn't listed.
    fn of_outcome(outcome: &FeatureOutcome) -> Option<Self> {
        Some(match outcome {
            FeatureOutcome::Built { .. } | FeatureOutcome::Sketch(_) => StepState::Built,
            FeatureOutcome::Blocked { .. } => StepState::Blocked,
            FeatureOutcome::Failed(_) | FeatureOutcome::Broken { .. } => {
                if outcome.errors().iter().any(|e| e.code().is_kernel_fault()) {
                    StepState::Faulted
                } else {
                    StepState::Failed
                }
            }
            FeatureOutcome::Suppressed => return None,
        })
    }
}

/// Each unsuppressed feature's [`StepState`] in a rebuild, in timeline order. Its IDs are
/// shared, so a copy costs a byte a feature: `status` reads it as often as it likes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureStates {
    ids: Arc<[FeatureId]>,
    states: Vec<StepState>,
}

impl FeatureStates {
    /// Each feature and its state, in timeline order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&FeatureId, StepState)> {
        self.ids.iter().zip(self.states.iter().copied())
    }

    /// How many features are listed.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Whether none is.
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// The state of the feature `id`, if it's listed: a walk of the list.
    pub fn get(&self, id: &FeatureId) -> Option<StepState> {
        self.iter()
            .find(|(listed, _)| *listed == id)
            .map(|(_, s)| s)
    }
    /// As a rebuild that isn't running says them: none running. A rebuild stopped is said
    /// so (`status`) as its cancel lands, before its recompute has ended and left what it
    /// was running queued ([`StepCounter`]'s end): what was running then is queued.
    pub(crate) fn none_running(&mut self) {
        for state in &mut self.states {
            if *state == StepState::Running {
                *state = StepState::Queued;
            }
        }
    }
}

impl FromIterator<(FeatureId, StepState)> for FeatureStates {
    fn from_iter<I: IntoIterator<Item = (FeatureId, StepState)>>(pairs: I) -> Self {
        let (ids, states): (Vec<FeatureId>, Vec<StepState>) = pairs.into_iter().unzip();
        FeatureStates {
            ids: ids.into(),
            states,
        }
    }
}

/// What became of a step as it settled, for its features' states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settled {
    /// It made its result.
    Made,
    /// It failed: its features' own failure, a kernel fault or not.
    Failed(StepState),
    /// It panicked, a bug: its features faulted at once, and what uses them blocked, since
    /// the recompute ends with the panic's error (no model to say otherwise).
    Panicked,
    /// It will never run: an input failed, or only refused features' users need it.
    Unrun,
    /// A newer recompute superseded it, or it was cancelled: nothing came of it, and the
    /// recompute ends.
    Superseded,
}

struct Counted {
    count: StepCount,
    /// When the recompute was asked for, on the monotonic clock: when its counter was made
    /// ([`Recompute::begin`]). `count.elapsed` is read from it while the count runs, unless
    /// it was stopped ([`stopped_after`](Self::stopped_after)), and fixed as the count ends.
    started: Instant,
    /// How long after `started` it was stopped (cancelled, superseded by a newer recompute,
    /// or its engine dropped), if it was: its clock's end, though its thread may still be
    /// draining the requests it has running ([`StepCounter::stopped`]). `count.elapsed` is
    /// this from then on, while the count runs; a stop after its end is recorded but never
    /// read, the end having fixed its time.
    stopped_after: Option<Duration>,
    /// Where each feature is listed in `count.features`, by its place in the document: none
    /// for a suppressed feature.
    slots: Vec<Option<usize>>,
    /// How many plans it has made: one a round.
    rounds: usize,
    /// What each step is called, by key: its first feature and that feature's type.
    labels: BTreeMap<Key, String>,
    /// The steps settled, by key, in any round: `done` counts them. A step a round settled
    /// without a result (never to run) is planned unsettled by the next, and settled there
    /// again; counted twice, `done` would reach `steps` with a step still running.
    settled: BTreeSet<Key>,
    /// The steps running, in the order they started.
    running: Vec<Key>,
    /// How many times the count has changed as the recompute ran: a plan made, a step or a
    /// solve started or settled, a check refused. Not the end, which its watcher tells once
    /// it's recorded. What [`StepCounter::changed_since`] waits on, so each change wakes the
    /// client once, whether or not it moved `done` or `steps`.
    changes: u64,
    /// The features whose result's check ([`Check`]: a boolean's solids, a sweep's measures)
    /// isn't decided yet, by place in the document: built, one may yet fail it.
    unchecked: BTreeSet<usize>,
    /// Each feature's own failure, by place, held while a feature it uses, all the way up,
    /// may yet fail its check: the model says it's blocked then, whatever its own problem,
    /// so it reads queued (or built, if it did) until the failure is said or it's blocked.
    held: BTreeMap<usize, StepState>,
    /// Whether each feature, by place, is firm: built, its check passed (or it has none),
    /// and all it uses firm. A failure is said once all a feature uses is firm.
    firm: Vec<bool>,
}

impl Default for Counted {
    /// Nothing counted yet, its clock started now.
    fn default() -> Self {
        Counted {
            count: StepCount::default(),
            started: Instant::now(),
            stopped_after: None,
            slots: Vec::new(),
            rounds: 0,
            labels: BTreeMap::new(),
            settled: BTreeSet::new(),
            running: Vec::new(),
            changes: 0,
            unchecked: BTreeSet::new(),
            held: BTreeMap::new(),
            firm: Vec::new(),
        }
    }
}

impl Counted {
    /// The count as it's read: its `elapsed` the time so far while it runs, fixed once it
    /// has ended ([`StepCounter::ended`]) or stopped ([`StepCounter::stopped`]).
    fn read(&self) -> StepCount {
        let mut count = self.count.clone();
        if !count.ended {
            count.elapsed = self.clock();
        }
        count
    }

    /// How long the recompute has run: to its stop, if it was stopped, else so far.
    fn clock(&self) -> Duration {
        self.stopped_after.unwrap_or_else(|| self.started.elapsed())
    }
}

/// Each feature's state as a round's plan finds it ([`Plan::states`]), by place in the
/// document, with the count's view of what may yet change it.
#[derive(Debug, Default)]
struct Found {
    states: Vec<StepState>,
    /// The features whose check isn't decided yet ([`Counted::unchecked`]).
    unchecked: BTreeSet<usize>,
    /// The failures held for what they use ([`Counted::held`]), each feature queued.
    held: BTreeMap<usize, StepState>,
    /// Which features are firm ([`Counted::firm`]).
    firm: Vec<bool>,
}

impl StepCounter {
    /// The count now.
    pub fn now(&self) -> StepCount {
        lock(&self.0.0).read()
    }

    /// The count once it has changed since the change numbered `seen` (0: the recompute's
    /// start, before anything is solved or planned), or it has ended, and the number of the
    /// change it's at: waited for, each change being told ([`Condvar`]). Of a count that has
    /// ended, at once. A change is any feature's state moving, or a plan's: a step's start
    /// or a sketch's settle wakes a waiter as a step's settle does, with `done` and `steps`
    /// as they were.
    pub fn changed_since(&self, seen: u64) -> (StepCount, u64) {
        let (counted, told) = &*self.0;
        let mut counted = lock(counted);
        while !counted.count.ended && counted.changes == seen {
            counted = told.wait(counted).unwrap_or_else(PoisonError::into_inner);
        }
        (counted.read(), counted.changes)
    }

    /// The number of the change the count is at now (0: none since the recompute's start), as
    /// [`changed_since`](Self::changed_since) numbers them, without waiting.
    pub fn changes(&self) -> u64 {
        lock(&self.0.0).changes
    }

    /// The count changed as `change` changes it: counted, and told to whoever waits.
    fn change(&self, change: impl FnOnce(&mut Counted)) {
        {
            let mut counted = lock(&self.0.0);
            change(&mut counted);
            counted.changes += 1;
        }
        self.0.1.notify_all();
    }

    /// The recompute of `document` started: each of its unsuppressed features listed, in
    /// timeline order, each `queued`, before anything is solved or planned.
    fn listing(&self, document: &Checked) {
        let features = &document.document().features;
        let mut slots = Vec::with_capacity(features.len());
        let mut ids = Vec::with_capacity(features.len());
        for feature in features {
            if feature.suppressed {
                slots.push(None);
            } else {
                slots.push(Some(ids.len()));
                ids.push(feature.id.clone());
            }
        }
        let mut counted = lock(&self.0.0);
        counted.count.features = FeatureStates {
            states: vec![StepState::Queued; ids.len()],
            ids: ids.into(),
        };
        counted.slots = slots;
    }

    /// A round's plan made: `steps` steps, `settled` those of them settled already, each
    /// called as `labels` says, and each feature's state as the plan finds it, by its place
    /// in the document (`found`: a feature not among its states is left as it was), with the
    /// checks, held failures, and firmness the count goes on from. Neither count goes back: a
    /// later round's plan holds the steps of the rounds before it (some settled there
    /// without a result kept, never to run, which it counts unsettled), and a step is counted
    /// once, by its key, whichever rounds settle it. (A plan holds a failure only on a check
    /// of a result kept but not yet measured, which this round's plan is the first to plan:
    /// each round ends with every check of its own decided, so no plan takes a feature back
    /// from built.)
    fn planned(
        &self,
        steps: usize,
        settled: impl IntoIterator<Item = Key>,
        labels: BTreeMap<Key, String>,
        found: &Found,
    ) {
        self.change(|counted| {
            counted.settled.extend(settled);
            for (place, state) in found.states.iter().enumerate() {
                counted.force(place, *state);
            }
            counted.unchecked.clone_from(&found.unchecked);
            counted.held.clone_from(&found.held);
            counted.firm.clone_from(&found.firm);
            let Counted {
                count,
                settled,
                labels: named,
                rounds,
                ..
            } = counted;
            count.steps = count.steps.max(steps);
            count.done = settled.len().min(count.steps);
            *named = labels;
            *rounds += 1;
        });
    }

    /// How many rounds the recompute has planned so far (docs/ports/attacher.md, *In
    /// rounds*): a test's measure of how far it went.
    #[cfg(any(test, feature = "fake"))]
    pub fn rounds(&self) -> usize {
        lock(&self.0.0).rounds
    }

    /// The step `key` of `plan` started: the features that own it running.
    fn started(&self, key: Key, plan: &Plan) {
        self.change(|counted| {
            counted.running.push(key);
            counted.count.running = counted.labels.get(&key).cloned();
            for &f in plan.owners.get(&key).into_iter().flatten() {
                counted.set(f, StepState::Running);
            }
        });
    }

    /// The step `key` of `plan` settled as `outcome` says: run (it was
    /// [`started`](Self::started)) or never to be. Its features' states follow: built once
    /// their last step is made, queued again between steps, failed or faulted by its failure
    /// (and what uses them, blocked), and blocked if it never runs. Superseded, they're left
    /// for the recompute's end ([`ended`](Self::ended)), which leaves what ran queued.
    fn settled(&self, key: Key, outcome: Settled, plan: &Plan) {
        self.change(|counted| counted.settle(key, outcome, plan));
    }

    /// The checks of the features of `plan` at `passed` passed, and those at `failed` failed,
    /// each as its state says (a measure that faulted, faulted): each failure said, and what
    /// uses it blocked, once all it uses is firm ([`Counted::fail`]); each that passed firm
    /// if it's built, and what its firmness decides, decided ([`Counted::firm_up`]).
    fn judged(&self, passed: &[usize], failed: &[(usize, StepState)], plan: &Plan) {
        if passed.is_empty() && failed.is_empty() {
            return;
        }
        self.change(|counted| {
            for &(f, state) in failed {
                counted.unchecked.remove(&f);
                counted.fail(f, state, plan);
            }
            for &f in passed {
                counted.unchecked.remove(&f);
                counted.firm_up(f, plan);
            }
        });
    }

    /// The sketch at `place` in the document started its solve.
    fn sketch_started(&self, place: Option<usize>) {
        if let Some(place) = place {
            self.change(|counted| {
                counted.set(place, StepState::Running);
            });
        }
    }

    /// The sketch at `place` in the document settled as `state`: built if it solved, failed
    /// or faulted if not.
    fn sketch_settled(&self, place: Option<usize>, state: StepState) {
        if let Some(place) = place {
            self.change(|counted| {
                counted.set(place, state);
            });
        }
    }

    /// The solves of the sketches at `places` panicked, a bug: the one solving, faulted,
    /// and what uses it (as `plan` has it, if one is made) blocked.
    fn sketches_panicked(&self, places: impl IntoIterator<Item = usize>, plan: Option<&Plan>) {
        self.change(|counted| {
            for place in places {
                if counted.state(place) == Some(StepState::Running)
                    && counted.set(place, StepState::Faulted)
                    && let Some(plan) = plan
                {
                    counted.block_users(place, plan);
                }
            }
        });
    }

    /// The rebuild's model made: each feature's state as its outcome is, whatever the
    /// rebuild's steps said, so the states a finished rebuild leaves are its model's.
    fn concluded(&self, model: &Model) {
        let mut counted = lock(&self.0.0);
        for (place, (_, outcome)) in model.features.iter().enumerate() {
            if let Some(state) = StepState::of_outcome(outcome) {
                counted.force(place, state);
            }
        }
    }

    /// The recompute ended: every step settled, if it finished, and the time it ran fixed.
    /// Of one that has ended, it changes nothing.
    fn ended(&self, finished: bool) {
        {
            let mut counted = lock(&self.0.0);
            if counted.count.ended {
                return;
            }
            counted.running.clear();
            // Stopped, superseded, or failed: what was running is unsettled, to resume.
            if !finished {
                counted.count.features.none_running();
            }
            let elapsed = counted.clock();
            let count = &mut counted.count;
            if finished {
                count.done = count.steps;
            }
            count.running = None;
            count.elapsed = elapsed;
            count.ended = true;
        }
        // Told, not counted: the watcher tells the end once it's recorded.
        self.0.1.notify_all();
    }

    /// The recompute was stopped (cancelled, superseded by a newer one, or its engine
    /// dropped): its clock ends now, though its thread may yet drain the requests it has
    /// running, so a stopped recompute's time never runs on beside `status`'s `stopped`, or
    /// its `done` when the revision keeps the model it had. Of one stopped already, it
    /// changes nothing (its first stop stands); of one that has ended, nothing either, its
    /// end having fixed its time ([`read`](Counted::read) and [`ended`](Self::ended) read
    /// the stop only while it runs).
    fn stopped(&self) {
        let mut counted = lock(&self.0.0);
        if counted.stopped_after.is_none() {
            counted.stopped_after = Some(counted.started.elapsed());
        }
    }
}

impl Counted {
    /// The step `key` of `plan` settled as `outcome` says ([`StepCounter::settled`]).
    fn settle(&mut self, key: Key, outcome: Settled, plan: &Plan) {
        // A superseded step isn't settled (built, failed, kept, or never to run): the next
        // rebuild resumes it. So a step a stopped recompute drains to its supersede isn't
        // counted (one that made its result, or was found never to run, as the stop came,
        // still is).
        if outcome != Settled::Superseded && self.settled.insert(key) {
            self.count.done = self.settled.len().min(self.count.steps);
        }
        if outcome != Settled::Unrun {
            self.running.retain(|other| *other != key);
            self.count.running = self
                .running
                .last()
                .and_then(|last| self.labels.get(last))
                .cloned();
        }
        for &f in plan.owners.get(&key).into_iter().flatten() {
            match outcome {
                Settled::Made => {
                    let last = plan.features.get(f).and_then(|p| p.steps.last());
                    if last != Some(&key) {
                        self.set(f, StepState::Queued);
                    } else if self.set(f, StepState::Built) {
                        self.firm_up(f, plan);
                    }
                }
                // The recompute is superseded, and ends: its end leaves what ran queued.
                Settled::Superseded => {}
                Settled::Failed(state) => self.fail(f, state, plan),
                Settled::Panicked => {
                    if self.set(f, StepState::Faulted) {
                        self.block_users(f, plan);
                    }
                }
                Settled::Unrun => {
                    if self.set(f, StepState::Blocked) {
                        self.block_users(f, plan);
                    }
                }
            }
        }
    }

    /// The feature at `place` failed on its own, as `state` says (failed or faulted): said,
    /// and what uses it blocked, if all it uses is firm. If not, held, the feature queued
    /// (or left built): the model says it's blocked if what it uses fails its check, so a
    /// failure said now could be taken back. [`firm_up`](Self::firm_up) says it later.
    fn fail(&mut self, place: usize, state: StepState, plan: &Plan) {
        if self.inputs_firm(place, plan) {
            self.held.remove(&place);
            if self.set(place, state) {
                self.block_users(place, plan);
            }
        } else {
            self.held.insert(place, state);
            if self.state(place) == Some(StepState::Running) {
                self.set(place, StepState::Queued);
            }
        }
    }

    /// Whether all the feature at `place` uses (its base, tool, sketch) is firm.
    fn inputs_firm(&self, place: usize, plan: &Plan) -> bool {
        plan.features.get(place).is_some_and(|planned| {
            planned
                .uses
                .iter()
                .chain(&planned.sweeps)
                .all(|&u| self.firm.get(u) == Some(&true))
        })
    }

    /// The feature at `place` built, or its check passed: if it's firm now, so may be what's
    /// made from it, all the way down; and each user of a firm feature whose own failure
    /// waits on what it uses (held, or the plan's refusal of its sketch's profile or axis)
    /// says it once all it uses is firm, as the model has it (blocked if an input didn't
    /// build or failed its check; failed if all did and passed).
    fn firm_up(&mut self, place: usize, plan: &Plan) {
        let mut stack = vec![place];
        while let Some(f) = stack.pop() {
            if self.firm.get(f) != Some(&false) || !self.inputs_firm(f, plan) {
                continue;
            }
            let held = self.held.remove(&f).or_else(|| {
                let refused = plan.features.get(f)?.refused.as_ref()?;
                Some(StepState::failed(refused))
            });
            if let Some(state) = held {
                if self.set(f, state) {
                    self.block_users(f, plan);
                }
            } else if self.state(f) == Some(StepState::Built) && !self.unchecked.contains(&f) {
                if let Some(firm) = self.firm.get_mut(f) {
                    *firm = true;
                }
                stack.extend(plan.dependants.get(f).into_iter().flatten().copied());
            }
        }
    }

    /// The state of the feature at `place` in the document, if it's listed.
    fn state(&self, place: usize) -> Option<StepState> {
        let slot = self.slots.get(place).copied().flatten()?;
        self.count.features.states.get(slot).copied()
    }

    /// The feature at `place` in the document moved to `to`, if its state may become that
    /// ([`StepState::may_become`]): whether it moved. A suppressed feature, or one past the
    /// list (a counter that listed no document), isn't listed: nothing moves.
    fn set(&mut self, place: usize, to: StepState) -> bool {
        let Some(state) = self.slot(place) else {
            return false;
        };
        let moved = state.may_become(to);
        if moved {
            *state = to;
        }
        moved
    }

    /// The feature at `place` set to `to`, whatever it was: as a plan or a model finds it.
    fn force(&mut self, place: usize, to: StepState) {
        if let Some(state) = self.slot(place) {
            *state = to;
        }
    }

    fn slot(&mut self, place: usize) -> Option<&mut StepState> {
        let slot = self.slots.get(place).copied().flatten()?;
        self.count.features.states.get_mut(slot)
    }

    /// Every feature of `plan` made from the feature at `place`, all the way down, blocked:
    /// but one failed or faulted, or blocked already (what's made from it was then).
    fn block_users(&mut self, place: usize, plan: &Plan) {
        let mut stack: Vec<usize> = plan.dependants.get(place).cloned().unwrap_or_default();
        while let Some(user) = stack.pop() {
            if self.set(user, StepState::Blocked) {
                stack.extend(plan.dependants.get(user).into_iter().flatten().copied());
            }
        }
    }
}

/// Where the sketch `id` is in `document`, if its solve's start and settle are reported
/// ([`StepCounter::sketch_started`]): not for a sketch on a face. Its solve comes before its
/// face is placed, and the model words it by its placement first (a face broken, or one whose
/// solid didn't build, is its outcome however it solved, as FreeCAD attaches a sketch before
/// solving it): so it's queued until the round that places it says what it came to, and
/// never goes back from what its solve said.
fn reported(document: &Checked, id: &FeatureId) -> Option<usize> {
    let place = document.place(id)?;
    match &document.document().features.get(place)?.operation {
        Operation::Sketch(sketch) if sketch.plane.face().is_some() => None,
        _ => Some(place),
    }
}

/// Features a check refused, each failed ([`StepCounter::judged`]).
fn failed(refused: Vec<usize>) -> Vec<(usize, StepState)> {
    refused
        .into_iter()
        .map(|f| (f, StepState::Failed))
        .collect()
}

/// A recompute's count ended however its thread ends, a panic too, so no one waiting for
/// its next change ([`StepCounter::changed_since`]) waits forever.
struct EndsCount(StepCounter);

impl Drop for EndsCount {
    fn drop(&mut self) {
        self.0.ended(false);
    }
}

impl Run {
    /// How far the recompute has got: shared with it, so it may be kept and read after the
    /// run is waited on, or by another thread.
    pub fn progress(&self) -> StepCounter {
        self.progress.clone()
    }

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

/// What a recompute reads of what was kept before it: everything it may ([`Recompute::start`]),
/// or nothing ([`Recompute::start_fresh`]), every step built anew.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reads {
    /// The engine's results, solves, profiles, and named maps, and the cache's entries.
    Kept,
    /// None of them: each is made again, and kept as any recompute's are.
    Nothing,
}

/// Rebuilds documents on a kernel, keeping each step's result by key between recomputes.
/// Dropping it supersedes the recompute running, if any.
pub struct Recompute<K: Kernel + 'static> {
    engine: Arc<Engine<K>>,
}

struct Engine<K> {
    kernel: K,
    /// Where results are kept on disk, if anywhere: given as the engine is made, or once
    /// after ([`Recompute::keep_in`]: an untitled session's, at its first save).
    cache: OnceLock<Cache>,
    state: Mutex<EngineState>,
    /// Signalled when a recompute's sketch thread gives back its turn at the solvers, or a
    /// recompute starts or is cancelled: a recompute waiting for its turn wakes to take it,
    /// or to find it's superseded and wait no longer.
    sketch_turn: Condvar,
    /// Where its sketch solver comes from: its own (the sketch worker, unless
    /// [`Recompute::with_sketches`] gave another), or those [`Recompute::with_solvers`] gave,
    /// which other engines may share. Taken only by the
    /// recompute whose turn it is.
    solvers: Mutex<SketchSolvers>,
    /// A test's hold on the background's naming of each model the engine publishes.
    #[cfg(test)]
    naming_hold: NamingHold,
}

/// A test's hold on an engine's background naming ([`Recompute::hold_naming`]): while
/// it's held, the background waits before naming the model it published; and the models
/// whose naming has ended, so a test can wait for one, with no sleep.
#[cfg(test)]
#[derive(Default)]
struct NamingHold {
    state: Mutex<NamingHeld>,
    /// Told as the hold is released, and as a model's naming ends.
    changed: Condvar,
}

#[cfg(test)]
#[derive(Default)]
struct NamingHeld {
    held: bool,
    /// Each model whose background naming has ended, in the order they ended.
    ended: Vec<Arc<Model>>,
}

#[cfg(test)]
impl NamingHold {
    /// Waits while the hold is held.
    fn wait_released(&self) {
        let mut state = lock(&self.state);
        while state.held {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// `model`'s naming has ended.
    fn ended(&self, model: &Arc<Model>) {
        lock(&self.state).ended.push(Arc::clone(model));
        self.changed.notify_all();
    }
}

/// Starts a sketch solver.
type SolverStarter = Box<dyn FnMut() -> Result<Box<dyn SketchSolver>, Error> + Send>;

/// Sketch solvers, idle, and how to start another: an engine's own, or shared by engines
/// ([`Recompute::with_solvers`]: a [`Sessions`](crate::command::Sessions)' sessions share
/// one, as they share the kernel pool). Each is lent to one recompute's sketch thread at a
/// time, for all its sketches, and given back when they're solved; another is started only
/// when none is idle. So engines that solve one after another (`silico check` of many
/// files) start one solver between them, and engines that solve at once each have one,
/// none waiting for another's solve. A solver keeps nothing of a solve once it's given
/// back (each solve carries its whole sketch). Clones share the same solvers.
#[derive(Clone)]
pub struct SketchSolvers(Arc<SharedSolvers>);

struct SharedSolvers {
    /// The solvers given back, the last given back taken first, and how many a warm start
    /// is still starting. Taken alone, or after an engine's state or `start`, never before
    /// either.
    idle: Mutex<Idle>,
    /// Told as each solver a warm start started is idle, or failed to start.
    warmed: Condvar,
    /// Starts another when none is idle. Taken alone, or before `idle`; held across the
    /// start, so engines that find none idle start one at a time.
    start: Mutex<SolverStarter>,
}

impl SketchSolvers {
    /// Solvers started as the sketch worker ([`SketchWorker::start`]), the first when a
    /// sketch is first solved.
    pub fn workers() -> Self {
        Self::starting(|| {
            SketchWorker::start().map(|worker| Box::new(worker) as Box<dyn SketchSolver>)
        })
    }

    /// Solvers started with `start` (a stand-in, or one that fails to start, in tests). A
    /// start that fails fails the sketches of the recompute that asked, and isn't kept:
    /// the next recompute to ask starts one again.
    pub fn starting(
        start: impl FnMut() -> Result<Box<dyn SketchSolver>, Error> + Send + 'static,
    ) -> Self {
        Self::of(Vec::new(), Box::new(start))
    }

    /// `solver`, idle, and others started as the sketch worker.
    fn holding(solver: Box<dyn SketchSolver>) -> Self {
        let workers =
            || SketchWorker::start().map(|worker| Box::new(worker) as Box<dyn SketchSolver>);
        Self::of(vec![solver], Box::new(workers))
    }

    fn of(idle: Vec<Box<dyn SketchSolver>>, start: SolverStarter) -> Self {
        SketchSolvers(Arc::new(SharedSolvers {
            idle: Mutex::new(Idle {
                solvers: idle,
                warming: 0,
            }),
            warmed: Condvar::new(),
            start: Mutex::new(start),
        }))
    }

    /// Starts solvers, here, until `n` are idle or starting (at most as many as the machine
    /// has cores, as a kernel pool's workers are), and waits until each has started or
    /// failed: an app's warm start (M5.0's decision 10), run off its UI thread, so its first
    /// rebuild needn't wait for a sketch worker to start. A solve that comes while they
    /// start waits for one, rather than starting another. A start that fails ends the warm
    /// start, none more tried, and the next solve starts one, as without a warm start.
    /// Called again with as many idle, it starts none more. How many it started.
    pub fn warm(&self, n: usize) -> usize {
        let _span = tracing::info_span!("sketch_solvers_warm", n).entered();
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        let n = n.min(cores.min(crate::pool::MAX_WORKERS));
        let wanted = {
            let mut idle = lock(&self.0.idle);
            let wanted = n.saturating_sub(idle.solvers.len().saturating_add(idle.warming));
            idle.warming += wanted;
            wanted
        };
        let mut warming = Warming {
            shared: &self.0,
            left: wanted,
        };
        let mut started = 0;
        while warming.left > 0 {
            let result = (*lock(&self.0.start))();
            let mut idle = lock(&self.0.idle);
            idle.warming -= 1;
            warming.left -= 1;
            match result {
                Ok(solver) => {
                    idle.solvers.push(solver);
                    started += 1;
                }
                Err(e) => {
                    tracing::warn!("a sketch solver started ahead of its solves failed: {e}");
                    // The rest would fail alike: they're counted off, and a solve waiting
                    // starts its own.
                    idle.warming -= warming.left;
                    warming.left = 0;
                }
            }
            drop(idle);
            self.0.warmed.notify_all();
        }
        tracing::debug!(started, "warmed the sketch solvers");
        started
    }

    /// An idle solver, or, with none idle, one a warm start is starting, or one started
    /// now. The idle list is looked at again once the start is this one's: a solver given
    /// back while another engine's start ran is taken, not one more started.
    pub(crate) fn take(&self) -> Result<Box<dyn SketchSolver>, Error> {
        {
            let mut idle = lock(&self.0.idle);
            loop {
                if let Some(solver) = idle.solvers.pop() {
                    return Ok(solver);
                }
                if idle.warming == 0 {
                    break;
                }
                idle = self
                    .0
                    .warmed
                    .wait(idle)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
        let mut start = lock(&self.0.start);
        if let Some(solver) = lock(&self.0.idle).solvers.pop() {
            return Ok(solver);
        }
        let started = (*start)();
        if started.is_ok() {
            tracing::debug!("started a sketch solver");
        }
        started
    }

    /// `solver`, idle again.
    pub(crate) fn give_back(&self, solver: Box<dyn SketchSolver>) {
        lock(&self.0.idle).solvers.push(solver);
        self.0.warmed.notify_all();
    }
}

/// The solvers given back, and how many a warm start is still starting.
struct Idle {
    solvers: Vec<Box<dyn SketchSolver>>,
    warming: usize,
}

/// A warm start's count of solvers left to start: those it didn't start (its starter
/// panicked) are counted off as it unwinds, so a solve waiting for them goes on.
struct Warming<'a> {
    shared: &'a SharedSolvers,
    left: usize,
}

impl Drop for Warming<'_> {
    fn drop(&mut self) {
        if self.left > 0 {
            lock(&self.shared.idle).warming -= self.left;
            self.shared.warmed.notify_all();
        }
    }
}

/// A solver, lent to a recompute's sketch thread: given back to its solvers when it's
/// dropped, even by a panic under a solve, and the engine's turn at them with it, so the
/// next recompute doesn't wait for it forever.
struct LentSolver<'a> {
    state: &'a Mutex<EngineState>,
    turn: &'a Condvar,
    solvers: SketchSolvers,
    solver: Option<Box<dyn SketchSolver>>,
}

impl Drop for LentSolver<'_> {
    fn drop(&mut self) {
        let mut state = lock(self.state);
        // A solve that panicked left its stop handle: nothing of it runs now. Cleared before
        // the solver is given back, so no stop of this engine's reaches another's solve.
        state.solving = None;
        // Given back before the turn, so the engine's next recompute finds it idle, and
        // starts none.
        if let Some(solver) = self.solver.take() {
            self.solvers.give_back(solver);
        }
        state.sketching = false;
        drop(state);
        self.turn.notify_all();
    }
}

struct EngineState {
    /// The newest recompute's generation, or a cancel's after it.
    generation: Generation,
    /// The newest recompute's generation, whether or not it was cancelled since.
    newest_started: Generation,
    /// The newest recompute's count, whose clock a cancel, a newer recompute, or the
    /// engine's drop stops ([`StepCounter::stopped`]); kept after its run ends (a stop then
    /// changes nothing); none before the first.
    newest_progress: Option<StepCounter>,
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
    /// Whether a recompute's sketch thread has the engine's turn at its solvers: one
    /// recompute's at a time.
    sketching: bool,
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
            Err(e) if started == 0 => return Err(crate::worker::thread_error("recompute", &e)),
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
/// A recompute's sketches' results, by key.
type SketchResults = BTreeMap<Key, SketchResult>;

/// A sketch's solve, as a recompute keeps it: its result (its solution, or why it has
/// none, naming the sketch, as `check` reports it), and the diagnosis read from the same
/// answer, paired with its outcome ([`Diagnosed`]), which the sketch read's states are read
/// from ([`Model::diagnosed`]), or why there's none (the solver failed before it diagnosed
/// the sketch, or its reply couldn't be read).
#[derive(Debug, Clone)]
struct SketchResult {
    result: Result<Arc<SketchSolution>, Error>,
    diagnosed: Result<Arc<Diagnosed>, Error>,
}

impl SketchResult {
    /// No solve: `error`, the solver's or a bug's, for both.
    fn failed(error: Error) -> Self {
        SketchResult {
            result: Err(error.clone()),
            diagnosed: Err(error),
        }
    }
}

/// A sketch's result in a recompute: its solve's, or the solver's failure, not the
/// sketch's (a worker that couldn't start), which isn't kept, so the next recompute tries
/// again.
#[derive(Clone)]
enum SketchOutcome {
    Solved(SketchResult),
    SolverFailed(Error),
}

impl SketchOutcome {
    fn result(self) -> SketchResult {
        match self {
            SketchOutcome::Solved(result) => result,
            SketchOutcome::SolverFailed(error) => SketchResult::failed(error),
        }
    }
}

/// What a recompute's sketch thread came to, by key.
type SketchRun = BTreeMap<Key, SketchOutcome>;

/// The sketches a recompute's features sweep, solved before its plan: each one's solve and
/// profile (`swept`, those that solved), every solve's result by key, the solves made now
/// (`fresh`) and the profiles made now, which the recompute keeps once its plan's keys say
/// they're worth keeping (kept sooner, a trim drops them: the review found every swept
/// sketch solved twice a recompute).
#[derive(Default)]
struct SolvedFirst {
    swept: BTreeMap<FeatureId, Swept>,
    solutions: SketchResults,
    fresh: SketchRun,
    profiles: Vec<(Key, ProfileResult)>,
}

/// A failure of the sketch solver, not the sketch: as the kernel's own failures, a fault
/// a client reports as its own (`check` and `fmt` exit 2), not the document's. It keeps its
/// own code (a worker that can't be had is `worker_unavailable`, from finding or starting
/// it), and names the feature it was solving.
fn solver_failure(error: Error, feature: &FeatureId) -> SketchOutcome {
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
    /// It replaces what [`with_solvers`](Self::with_solvers) gave: whichever is called last
    /// holds.
    pub fn with_sketches(self, sketches: impl SketchSolver + 'static) -> Self {
        *lock(&self.engine.solvers) = SketchSolvers::holding(Box::new(sketches));
        self
    }

    /// Solves sketches with `solvers`, which other engines may share (a
    /// [`Sessions`](crate::command::Sessions)' sessions do): a solver lent from them for
    /// each recompute's sketches, and given back after; or, with
    /// [`SketchSolvers::starting`], its own, started another way (a stand-in that fails to
    /// start, in tests). It replaces what [`with_sketches`](Self::with_sketches) gave:
    /// whichever is called last holds.
    pub fn with_solvers(self, solvers: SketchSolvers) -> Self {
        *lock(&self.engine.solvers) = solvers;
        self
    }

    fn build(kernel: K, cache: Option<Cache>) -> Self {
        let generation = kernel.generation();
        Self {
            engine: Arc::new(Engine {
                kernel,
                cache: cache.map_or_else(OnceLock::new, OnceLock::from),
                sketch_turn: Condvar::new(),
                solvers: Mutex::new(SketchSolvers::workers()),
                #[cfg(test)]
                naming_hold: NamingHold::default(),
                state: Mutex::new(EngineState {
                    generation,
                    newest_started: generation,
                    newest_progress: None,
                    cancelled: false,
                    dropped: false,
                    results: BTreeMap::new(),
                    solutions: BTreeMap::new(),
                    profiles: BTreeMap::new(),
                    sketching: false,
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

    /// From now on, each step's result is kept in `cache` too, and taken from there when
    /// it's kept, as [`with_cache`](Self::with_cache) gives it: for an engine made with
    /// none (an untitled session's, until its first save gives it the cache beside its
    /// file). A recompute running takes it for the steps it hasn't read or written yet.
    /// Given once: an engine that has a cache keeps it, and one given after is let go.
    pub fn keep_in(&self, cache: Cache) {
        let _ = self.engine.cache.set(cache);
    }

    /// The cache the engine keeps its steps in, if it has one.
    pub fn cache(&self) -> Option<&Cache> {
        self.engine.cache.get()
    }

    /// The model the newest finished recompute published, if any has.
    pub fn published(&self) -> Option<Arc<Model>> {
        lock(&self.engine.state).published.clone()
    }

    /// Holds the background's naming of each model published from now on until
    /// [`release_naming`](Self::release_naming): a test's way to read a model, and the
    /// copies made of it, before the background has named any of its elements.
    #[cfg(test)]
    pub(crate) fn hold_naming(&self) {
        lock(&self.engine.naming_hold.state).held = true;
    }

    /// Lets the background's naming held by [`hold_naming`](Self::hold_naming) go on.
    #[cfg(test)]
    pub(crate) fn release_naming(&self) {
        lock(&self.engine.naming_hold.state).held = false;
        self.engine.naming_hold.changed.notify_all();
    }

    /// Waits up to `timeout` for the background's naming of `model`, one this engine
    /// published, to end: whether it has.
    #[cfg(test)]
    pub(crate) fn wait_naming_ended(&self, model: &Arc<Model>, timeout: Duration) -> bool {
        let hold = &self.engine.naming_hold;
        let state = lock(&hold.state);
        hold.changed
            .wait_timeout_while(state, timeout, |state| {
                !state.ended.iter().any(|ended| Arc::ptr_eq(ended, model))
            })
            .is_ok_and(|(_, waited)| !waited.timed_out())
    }

    /// Starts rebuilding `document` in the background, superseding the recompute running,
    /// if any; returns at once. A document held already in an `Arc` (a session's) is shared,
    /// not copied.
    pub fn start(&self, document: impl Into<Arc<Checked>>) -> Run {
        self.begin(document.into(), Reads::Kept)
    }

    /// Starts rebuilding `document` as [`start`](Self::start) does, but with every step
    /// built and every sketch solved anew (`check`'s `fresh`, docs/design/m4.1-command-api.md,
    /// section 7; SolidWorks' forced rebuild, Ctrl+Q): nothing is read of what the engine
    /// keeps (its steps' results, the refusals among them, its sketches' solves and profiles,
    /// and the element maps it has named) or of the cache on disk. What it builds is kept
    /// as any recompute's is, in the engine and on disk, so a later recompute hits it; and
    /// its model, once published, is the engine's, with the maps it named. Nothing kept is
    /// cleared: the engine's other models, and other engines on the same cache, keep theirs.
    pub fn start_fresh(&self, document: impl Into<Arc<Checked>>) -> Run {
        self.begin(document.into(), Reads::Nothing)
    }

    /// [`start`](Self::start), reading what's kept as `reads` says
    /// ([`start_fresh`](Self::start_fresh) for [`Reads::Nothing`]).
    pub(crate) fn begin(&self, document: Arc<Checked>, reads: Reads) -> Run {
        let (reply, outcome) = mpsc::sync_channel(1);
        // Its clock starts here, as it's asked for, before its thread starts.
        let progress = StepCounter::default();
        progress.listing(&document);
        let (generation, later, older) = {
            let mut state = lock(&self.engine.state);
            let newest = state.generation.0.max(self.engine.kernel.generation().0);
            let Some(next) = newest.checked_add(1) else {
                let _ = reply.send(Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "the kernel has no generations left to recompute with",
                )
                .with_suggestion("start a new kernel")));
                progress.ended(false);
                return Run { outcome, progress };
            };
            state.generation = Generation(next);
            state.newest_started = Generation(next);
            state.cancelled = false;
            let older = state.newest_progress.replace(progress.clone());
            let later = state.stop_older_solve(Generation(next));
            (state.generation, later, older)
        };
        // The one this supersedes stopped, its clock with it: after the engine's lock, as a
        // count another thread shares is never locked under it. Should its thread see the
        // newer generation and end first, its own end has fixed its time, and this changes
        // nothing.
        if let Some(older) = older {
            older.stopped();
        }
        if let Some((after, number)) = later {
            Engine::stop_later(&self.engine, after, number);
        }
        // A superseded recompute waiting for the sketch solver stops waiting.
        self.engine.sketch_turn.notify_all();
        self.engine.kernel.advance(generation);
        let engine = Arc::clone(&self.engine);
        let counting = progress.clone();
        // The caller's span, then the recompute's: should the recompute's be filtered out,
        // its work still traces back to the caller.
        let caller = tracing::Span::current();
        // At warning level, as the worker's request span is: a warning during a recompute,
        // at the default level, says which generation and step it was.
        let span = tracing::warn_span!(
            "recompute",
            generation = generation.0,
            fresh = matches!(reads, Reads::Nothing)
        );
        let started = std::thread::Builder::new()
            .name(format!("recompute-{}", generation.0))
            .spawn(move || {
                let _caller = caller.enter();
                let _entered = span.enter();
                let ends = EndsCount(counting.clone());
                let outcome = engine.run(&document, generation, &counting, reads);
                counting.ended(matches!(outcome, Ok(RunOutcome::Published(_))));
                drop(ends);
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
                    #[cfg(test)]
                    engine.naming_hold.wait_released();
                    let _naming = tracing::debug_span!("name_elements").entered();
                    model.name_all(|| {
                        let state = lock(&engine.state);
                        !state.dropped
                            && state
                                .published
                                .as_ref()
                                .is_some_and(|newest| Arc::ptr_eq(newest, &model))
                    });
                    #[cfg(test)]
                    engine.naming_hold.ended(&model);
                }
            });
        if let Err(e) = started {
            let (reply, outcome) = mpsc::sync_channel(1);
            let _ = reply.send(Err(crate::worker::thread_error("recompute", &e)));
            progress.ended(false);
            return Run { outcome, progress };
        }
        Run { outcome, progress }
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
        let newest = state.newest_progress.clone();
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
        // Its clock stopped with it (after the engine's lock, as in `begin`).
        if let Some(newest) = newest {
            newest.stopped();
        }
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
    /// A linear pattern's, polar pattern's, or mirror's step (docs/design/m6.0-patterns.md,
    /// section 2.4): `copies` copies of `tool`'s solid fused into `base`'s or cut from it as
    /// `op` says, each placed where the step works out, when it runs, from its pattern's
    /// references, resolved in its base feature's map and description, and integers, which
    /// the plan holds (`Plan::patterns`). `values` is the digest of what decides the
    /// placements: the base feature's naming key and each reference as written, the
    /// pattern's integers, and which input it copies. No tool, and no copies, is the
    /// identity: it resolves and checks its references and passes its base through.
    Copies {
        base: Key,
        tool: Option<Key>,
        op: CopiesOp,
        copies: usize,
        values: [u8; 32],
    },
}

/// A pattern's step, as the plan holds it by its key: what it copies and where to, for the
/// engine to resolve and place when it runs.
#[derive(Debug, Clone)]
struct PatternStep {
    /// The pattern's own feature, by index.
    feature: usize,
    /// Its base feature (its effective base), by index: where its references are found.
    base: usize,
    /// The instances it places, in `skip`'s order: none for the identity.
    instances: Arc<[Instance]>,
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
            Step::Copies {
                base,
                tool,
                op,
                copies,
                values,
            } => {
                let op = match op {
                    CopiesOp::Fuse => "fuse",
                    CopiesOp::Cut => "cut",
                };
                let tool = tool.map_or_else(|| "none".to_owned(), |tool| tool.to_string());
                format!("copies {op} {base} {tool} {copies} {}", crate::hex(values))
            }
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
            Step::Copies { base, tool, .. } => std::iter::once(base).chain(tool).collect(),
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
            // The base, then each copy; the identity's is its base's, whatever it is
            // (`fits`).
            Step::Copies { copies, .. } => 1 + copies,
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
            | Step::Copies { .. }
            | Step::Faces(_) => None,
        }
    }
}

/// A faces step's line (design S3): `faces` and its plane (its word, `SketchPlane::word`:
/// an origin plane's, or `frame` and a face frame's nine numbers by their bits, its origin,
/// x, and y, so a face that moves moves the key: docs/ports/attacher.md, M5.9), then,
/// region by region and loop by loop (each region's outer loop, then its holes), each curve
/// as sent, every number by its bits: `line x0 y0 x1 y1`, `arc cx cy sx sy ex ey r a0 a1`,
/// `circle cx cy r`. Each region starts `region` and each loop `loop`, so regions and holes
/// of the same curves are other lines. No ID: two sketches of one profile share the step,
/// and a construction line toggled, added, or moved leaves it alone. A curve's bits are as
/// sent, a zero of either sign its own: −0 and +0 can be different requests (an arc's angle
/// of ±π). A frame's numbers are taken + 0, a zero of either sign as +0: a frame of −0s
/// places every point where the one of +0s does, so the two are one step.
fn faces_line(regions: &FaceRegions) -> String {
    use std::fmt::Write as _;
    let mut line = format!("faces {}", regions.plane.word());
    if let silico_protocol::SketchPlane::Face(frame) = regions.plane {
        for v in [frame.origin, frame.x, frame.y].into_iter().flatten() {
            let _ = write!(line, " {:016x}", (v + 0.0).to_bits());
        }
    }
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
    /// Each pattern's step, by key: whose it is, its base feature, and what it places.
    patterns: BTreeMap<Key, PatternStep>,
    /// The document planned, where each feature's [`Maker`] is found by its index when a
    /// fillet's names are read back ([`Plan::maker`]), rather than kept for every feature.
    document: Arc<Checked>,
    /// Each sketch to solve, in timeline order: its feature, and its solve's key.
    sketches: Vec<(FeatureId, Key)>,
    /// Each sweep feature's own check of its result, by ID, until its `Planned` takes it.
    checks_of: BTreeMap<FeatureId, SweepCheck>,
    /// Each step's features (those whose own steps hold it), by index: a step is run while
    /// one of them may still be built.
    owners: BTreeMap<Key, Vec<usize>>,
    /// The features whose checks a step's result (or measure) can decide, by the step's
    /// key: those whose solid it is, and those whose base's solid it is.
    judged_by: BTreeMap<Key, Vec<usize>>,
    /// Each feature's users, by index: those whose base, tool, or sketch it is.
    dependants: Vec<Vec<usize>>,
    /// The features that wait on a sketch's face not resolved yet, by index: each such sketch
    /// and everything made from it. Planned in a later round of the rebuild, once the face's
    /// solid is built ([`Engine::run`]).
    waits: BTreeSet<usize>,
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
/// change of volume (design S5, S7), or an M2 boolean's one solid (D2) and change of volume
/// (S7's rule). Each feature's own,
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
    /// A pattern's steps, each in turn on the one before (section 4.2): one solid, its
    /// copies on the base and its base in one piece, and a change of volume by S7's rule.
    Pattern(Arc<PatternCheck>),
}

/// What a pattern's result is checked for, and words its refusal by.
#[derive(Debug)]
struct PatternCheck {
    kind: PatternFeature,
    feature: FeatureId,
    base: FeatureId,
    /// Each step's original (none for a whole solid's copies), whether it fuses, and the
    /// instances it places, in order.
    steps: Vec<(Option<FeatureId>, CopiesOp, Arc<[Instance]>)>,
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
    /// A pattern's originals' faces and sweep steps, two an original, in timeline order: its
    /// copies' names are the sweeps' (N10, `ElementMap::copy`), so it's named from them too.
    tools: Vec<Key>,
    /// The earlier features it uses, base then tool (or a pattern's originals, in timeline
    /// order), by index.
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
    /// For a sketch on a face, where its face is, as far as the rounds so far have found.
    on_face: OnFace,
    /// Why it isn't built, though what it uses is: a refusal of its sketch's profile or its
    /// axis, found before any step (design S5, S6).
    refused: Option<Error>,
    /// What its result is checked for, once built ([`Check`]).
    check: Option<Check>,
    /// Whether it's suppressed: then it has nothing else, and its outcome is that.
    suppressed: bool,
}

/// Where a sketch's face is, as a round of the rebuild plans it.
#[derive(Debug, Clone)]
enum OnFace {
    /// Not a sketch on a face.
    No,
    /// On a face whose solid isn't built yet: resolved in a later round.
    Waits,
    /// On a face resolved: its frame, or why it has none.
    Placed(Placed),
}

/// What a pattern's plan came to ([`Plan::pattern`]).
struct PatternPlan {
    steps: Vec<Key>,
    uses: Vec<usize>,
    tools: Vec<Key>,
    naming: Option<Naming>,
    check: Option<Arc<PatternCheck>>,
}

/// What a sweep feature's plan came to.
enum SweepPlan {
    /// Its steps, and how its elements are named.
    Steps(Vec<Key>, Arc<SweepNaming>),
    /// Refused before any step: its sketch's profile, or regions, or its axis.
    Refused(Error),
    /// Its sketch didn't solve, or its base can't be built: blocked on that.
    Waits,
}

/// The sweep step of `feature`, a pad, pocket, revolve, or groove sweeping its sketch's
/// profile on `plane` (the sketch's plane as resolved: an origin plane, or its face's
/// frame), whose faces step is `faces`, onto `base`'s solid if it has one: the start offset,
/// direction, and axis worked out from the sketch's plane and the extent, from the
/// document's integers, so each is exact or rounded once (design S3).
///
/// A pad's or pocket's is [`crate::sweep::prism`]'s, the rule the app's arrows read too; an
/// origin plane's normal is a unit axis, so a length along it is exact, and a face frame's
/// is rounded once. `through_all` sweeps from the plane through all of the base. A revolve
/// turns right-handed about its axis (left-handed `reversed`), its angle by FreeCAD's
/// conversion, `symmetric` from minus half of it.
fn sweep_step(
    feature: &crate::document::Feature,
    plane: silico_protocol::SketchPlane,
    axis: Option<([f64; 2], [f64; 2])>,
    faces: Key,
    base: Option<Key>,
) -> Result<Step, Error> {
    use crate::document::{Extent, TurnExtent};
    let unchecked = || {
        silico_bug("a sketch feature's extent isn't one its type takes, though the gate checked it")
            .with_feature(feature.id.as_str())
    };
    let extent = feature.operation.extent().ok_or_else(unchecked)?;
    let Extent::Turn(extent) = extent else {
        let swept = crate::sweep::prism(extent, plane.normal()).ok_or_else(unchecked)?;
        return Ok(match swept.span {
            Some(crate::sweep::Span { start, vector }) => Step::Prism {
                faces,
                start,
                vector,
            },
            None => Step::PrismThroughAll {
                faces,
                base: base.ok_or_else(unchecked)?,
                direction: swept.direction,
            },
        });
    };
    let (origin, line) = axis.ok_or_else(unchecked)?;
    let (angle, reversed, symmetric) = match extent {
        TurnExtent::OneSide { angle, reversed } => (angle, reversed, false),
        TurnExtent::Symmetric { angle } => (angle, false, true),
    };
    // An origin plane's axes are unit axes of model space, so the line's direction
    // in it is its direction in the sketch, exactly. A face frame's are square only to
    // its check's 1e-14, so x·l₀ + y·l₁ can be that far from a unit: normalized, so
    // the revolution's own check of a unit direction (4ε) takes it.
    let (x, y) = (plane.x_axis(), plane.y_axis());
    let sign = if reversed { -1.0 } else { 1.0 };
    let along = [0, 1, 2].map(|k| sign * (x[k] * line[0] + y[k] * line[1]));
    let along = match plane {
        silico_protocol::SketchPlane::Face(_) => silico_protocol::vec3::normalize(along),
        silico_protocol::SketchPlane::Xy
        | silico_protocol::SketchPlane::Xz
        | silico_protocol::SketchPlane::Yz => along,
    };
    let direction = along.map(plus_zero);
    let angle = silico_protocol::radians(angle.degrees());
    Ok(Step::Revolution {
        faces,
        origin: plane.place(origin),
        direction,
        start: if symmetric {
            plus_zero(-(angle / 2.0))
        } else {
            0.0
        },
        angle,
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
    hash.update(crate::document::write::plane_text(&sketch.plane).as_bytes());
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
    Pattern(Arc<PatternNaming>),
}

/// How a pattern's elements are named (N10): each step's copies, from the map of what it
/// copies, wrapped once (`ElementMap::copy`), and its result from its base's map (the step
/// before's, after the first) and theirs. No step is the identity, which passes its base's
/// map through.
#[derive(Debug)]
struct PatternNaming {
    operation: OpCode,
    steps: Vec<CopyNaming>,
}

/// What one of a pattern's steps copies, and where: a tool's copy (`C`) of an original,
/// named from its sweep step's map (its every element the sweep's own: N10's third item),
/// which the original's faces and sweep steps and its own naming make; or the base's whole
/// solid's (`S`), named from the base's map.
#[derive(Debug)]
struct CopyNaming {
    copied: Copied,
    original: Option<(FeatureId, Arc<SweepNaming>)>,
    instances: Arc<[Instance]>,
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

    /// The steps its elements are named from: its own, then a pattern's originals' faces
    /// and sweeps.
    fn naming_steps(&self) -> impl Iterator<Item = &Key> {
        self.steps.iter().chain(&self.tools)
    }

    /// A sketch's plane as the rebuild resolved it, `plane` as its document writes it: an
    /// origin plane's, or its face's frame; none for a sketch on a face not resolved yet, or
    /// whose face couldn't be.
    fn plane(&self, plane: &crate::document::Plane) -> Option<silico_protocol::SketchPlane> {
        match (&self.on_face, plane.origin()) {
            (OnFace::No, Some(origin)) => Some(origin.into()),
            (OnFace::Placed(Placed::Frame(frame)), None) => {
                Some(silico_protocol::SketchPlane::Face(*frame))
            }
            _ => None,
        }
    }

    /// A suppressed feature's plan: no steps, nothing used, nothing named.
    fn suppressed(id: FeatureId) -> Self {
        Planned {
            id,
            steps: Vec::new(),
            tools: Vec::new(),
            uses: Vec::new(),
            naming: None,
            naming_key: None,
            sketch: None,
            sweeps: None,
            on_face: OnFace::No,
            refused: None,
            check: None,
            suppressed: true,
        }
    }
}

/// The place of the feature whose solid stands for a feature's `base`, of its inputs as it
/// builds on them (`document::suppression::effective_inputs`).
fn base_of(inputs: &[(&'static str, Option<usize>)]) -> Option<usize> {
    inputs
        .iter()
        .find(|(field, _)| *field == "base")
        .and_then(|(_, stands)| *stands)
}

impl Plan {
    /// Each sketch's solve's key, in timeline order, and which of them a pad, pocket,
    /// revolve, or groove sweeps: those are solved before the plan is made (design S3).
    /// A suppressed feature sweeps nothing, so a suppressed sketch, whose every user the gate
    /// has suppressed with it, is no sketch swept, and isn't solved before the plan (nor
    /// after it: the plan lists no suppressed feature's solve).
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
            .filter(|f| !f.suppressed)
            .filter_map(|f| f.operation.sketch().cloned())
            .collect();
        (sketches, swept)
    }

    /// `document`'s steps, every sketch a feature sweeps solved and profiled in `swept`
    /// (one that didn't solve is missing), and each sketch on a face resolved so far in
    /// `frames`, by ID (docs/ports/attacher.md, *In rounds*): a sketch on a face not there
    /// waits, with everything made from it ([`Plan::waits`]), planned in a later round once
    /// its face's solid is built. A suppressed feature has none, and a feature
    /// whose `base` or `tool` names one builds on that input's effective base, the nearest
    /// unsuppressed feature up its chain (`document::suppression`): its steps take that
    /// feature's solid, and its names that feature's names.
    fn of(
        document: &Arc<Checked>,
        swept: &BTreeMap<FeatureId, Swept>,
        frames: &BTreeMap<FeatureId, Placed>,
    ) -> Plan {
        let features = &document.document().features;
        let place = |id: &FeatureId| document.place(id);
        let standing = crate::document::suppression::standing(features, &place);
        let mut plan = Plan {
            steps: BTreeMap::new(),
            order: Vec::new(),
            first_feature: BTreeMap::new(),
            features: Vec::with_capacity(features.len()),
            results: BTreeSet::new(),
            dresses: BTreeMap::new(),
            patterns: BTreeMap::new(),
            document: Arc::clone(document),
            sketches: Vec::new(),
            checks_of: BTreeMap::new(),
            owners: BTreeMap::new(),
            judged_by: BTreeMap::new(),
            dependants: Vec::new(),
            waits: BTreeSet::new(),
        };
        for (at, feature) in features.iter().enumerate() {
            if feature.suppressed {
                // Nothing to build, solve, or name: what consumes it builds on what stands
                // for it.
                plan.features.push(Planned::suppressed(feature.id.clone()));
                plan.dependants.push(Vec::new());
                continue;
            }
            // Its inputs, each at the feature whose solid stands for it.
            let stands =
                crate::document::suppression::effective_inputs(features, &standing, &place, at);
            let mut own = Vec::new();
            let mut uses = Vec::new();
            let mut tools = Vec::new();
            let mut refused = None;
            let mut sweeps = None;
            let mut pattern_check = None;
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
                    // The gate has checked that both are earlier features, and that neither
                    // input has its whole chain suppressed, so both are planned.
                    if let [(_, Some(b)), (_, Some(t))] = stands.as_slice() {
                        let (b, t) = (*b, *t);
                        uses = vec![b, t];
                        let key = |i: usize| plan.features.get(i).and_then(Planned::key);
                        if let (Some(base), Some(tool)) = (key(b), key(t)) {
                            own.push(plan.add(Step::Boolean { op, base, tool }, &feature.id));
                        }
                    }
                    Some(Naming::Operation(op.into()))
                }
                // A sketch makes no solid: nothing to build or name.
                Made::Sketch => None,
                Made::Fillet { radius } => {
                    let dressing = Dressing::Fillet { radius };
                    (own, uses) = plan.dress(feature, base_of(&stands), dressing);
                    Some(Naming::Operation(OpCode::Fillet))
                }
                Made::Chamfer { distance } => {
                    let dressing = Dressing::Chamfer { distance };
                    (own, uses) = plan.dress(feature, base_of(&stands), dressing);
                    Some(Naming::Operation(OpCode::Chamfer))
                }
                Made::Pattern(kind) => {
                    let planned = plan.pattern(document, feature, kind, base_of(&stands));
                    own = planned.steps;
                    uses = planned.uses;
                    tools = planned.tools;
                    pattern_check = planned.check;
                    planned.naming
                }
                Made::Sweep(kind) => {
                    // A pad's or revolve's base whose whole chain is suppressed stands for
                    // nothing: it builds alone, as one with no base does.
                    uses = stands.iter().filter_map(|(_, stand)| *stand).collect();
                    sweeps = feature
                        .operation
                        .sketch()
                        .and_then(|sketch| document.place(sketch));
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
                        SweepPlan::Waits => None,
                    }
                }
            };
            if let Some(result) = own.last() {
                plan.results.insert(*result);
            }
            // Each of a pattern's steps is measured, as its check judges each on the one
            // before (section 4.2).
            if pattern_check.is_some() {
                plan.results.extend(own.iter().copied());
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
            let on_face = match &feature.operation {
                Operation::Sketch(sketch) if sketch.plane.face().is_some() => {
                    match frames.get(&feature.id) {
                        Some(placed) => OnFace::Placed(placed.clone()),
                        None => OnFace::Waits,
                    }
                }
                _ => OnFace::No,
            };
            plan.features.push(Planned {
                id: feature.id.clone(),
                steps: own,
                tools,
                uses,
                naming,
                naming_key,
                sketch,
                sweeps,
                on_face,
                refused,
                check: match feature.operation.made() {
                    // Its inputs as it builds on them, so its refusal names those.
                    Made::Boolean(op) => match stands.as_slice() {
                        [(_, Some(b)), (_, Some(t))] => {
                            match (features.get(*b), features.get(*t)) {
                                (Some(base), Some(tool)) => Some(Check::Boolean {
                                    op,
                                    base: base.id.clone(),
                                    tool: tool.id.clone(),
                                }),
                                _ => None,
                            }
                        }
                        _ => None,
                    },
                    Made::Pattern(_) => pattern_check.map(Check::Pattern),
                    _ => plan.checks_of.remove(&feature.id).map(Check::Sweep),
                },
                suppressed: false,
            });
            // Which features each step serves, which checks its result decides, and what
            // uses what.
            let index = plan.features.len() - 1;
            plan.dependants.push(Vec::new());
            if let Some(planned) = plan.features.last() {
                // A sketch whose face isn't resolved yet waits, and so does what's made from
                // one that waits.
                let waits = matches!(planned.on_face, OnFace::Waits)
                    || planned
                        .uses
                        .iter()
                        .chain(&planned.sweeps)
                        .any(|u| plan.waits.contains(u));
                if waits {
                    plan.waits.insert(index);
                }
                for &u in planned.uses.iter().chain(&planned.sweeps) {
                    if let Some(users) = plan.dependants.get_mut(u) {
                        users.push(index);
                    }
                }
                for key in &planned.steps {
                    plan.owners.entry(*key).or_default().push(index);
                }
                if planned.check.is_some() {
                    let base = planned
                        .uses
                        .first()
                        .and_then(|&b| plan.features.get(b))
                        .and_then(Planned::key);
                    // A pattern's every step, each judged on the one before.
                    let own: Vec<Key> = match planned.check {
                        Some(Check::Pattern(_)) => planned.steps.clone(),
                        _ => planned.key().into_iter().collect(),
                    };
                    for key in own.into_iter().chain(base) {
                        plan.judged_by.entry(key).or_default().push(index);
                    }
                }
            }
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
                return SweepPlan::Refused(crate::sweep::unprofiled(
                    &feature.id,
                    kind,
                    sketch,
                    refused,
                ));
            }
        };
        if let Err(error) = crate::sweep::check_regions(&feature.id, kind, sketch, profile) {
            return SweepPlan::Refused(error);
        }
        let mut axis = None;
        if matches!(kind, SweepFeature::Revolve | SweepFeature::Groove) {
            let checked =
                crate::sweep::check_axis(document, &feature.id, &solved.solution, profile);
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
        // The sketch's plane as resolved: an origin plane's, or its face's frame. A sketch on
        // a face not resolved yet, or whose face couldn't be, makes it wait or blocks it.
        let Some(plane) = document
            .place(sketch)
            .and_then(|at| self.features.get(at))
            .and_then(|planned| planned.plane(&profile.plane))
        else {
            return SweepPlan::Waits;
        };
        let faces = self.add(Step::Faces(Arc::new(profile.faces(plane))), &feature.id);
        let step = match sweep_step(feature, plane, axis, faces, base.map(|b| b.1)) {
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

    /// The steps of `feature`, a linear pattern, polar pattern, or mirror of type `kind`
    /// (the next feature planned), on its base at `base` (its effective base), by place
    /// (docs/design/m6.0-patterns.md, section 2.4): one step for each original, in timeline
    /// order, each on the one before, its tool the original's sweep step, reused, never
    /// rebuilt; or one for the base's whole solid, its tool the base's result; or, placing
    /// no copy, the identity, one step that passes its base through. With the features it
    /// uses (its base, then its originals in timeline order), the originals' faces and sweep
    /// steps its names are read from, how its elements are named, and what its result is
    /// checked for. A base or an original that can't be planned leaves it no steps: it's
    /// blocked on that.
    fn pattern(
        &mut self,
        document: &Checked,
        feature: &crate::document::Feature,
        kind: PatternFeature,
        base: Option<usize>,
    ) -> PatternPlan {
        let operation = &feature.operation;
        let features = &document.document().features;
        // The originals, in timeline order, whatever order `copies.features` lists them in.
        let mut originals: Vec<usize> = operation
            .originals()
            .iter()
            .filter_map(|id| document.place(id))
            .collect();
        originals.sort_unstable();
        originals.dedup();
        let mut planned = PatternPlan {
            steps: Vec::new(),
            uses: base.into_iter().chain(originals.iter().copied()).collect(),
            tools: Vec::new(),
            naming: None,
            check: None,
        };
        // The gate has checked the base is an earlier feature with something unsuppressed up
        // its chain, so it's planned; one whose steps couldn't be blocks this.
        let Some(b) = base else {
            return planned;
        };
        let (Some(base_key), Some(base_naming)) = (
            self.features.get(b).and_then(Planned::key),
            self.features.get(b).and_then(|p| p.naming_key),
        ) else {
            return planned;
        };
        // Each original's sweep (its tool) and faces, and its naming and kind.
        let mut tools = Vec::with_capacity(originals.len());
        for &o in &originals {
            let (Some(original), Some(made)) = (self.features.get(o), features.get(o)) else {
                return planned;
            };
            let (Some(Naming::Sweep(naming)), [faces, sweep, ..], Made::Sweep(sweeps)) = (
                &original.naming,
                original.steps.as_slice(),
                made.operation.made(),
            ) else {
                return planned;
            };
            tools.push((
                original.id.clone(),
                Arc::clone(naming),
                *faces,
                *sweep,
                if sweeps.adds() {
                    CopiesOp::Fuse
                } else {
                    CopiesOp::Cut
                },
            ));
        }
        // What decides the placements, but which input is copied: the base feature's naming
        // key and each reference as written, a word or a name alike, then the integers.
        let mut digest = Sha256::new();
        digest.update(base_naming.0);
        for (field, text) in crate::pattern::references(operation) {
            for part in [field.as_str(), text] {
                digest.update((part.len() as u64).to_be_bytes());
                digest.update(part.as_bytes());
            }
        }
        digest.update(crate::pattern::values(operation).as_bytes());
        let instances: Arc<[Instance]> = crate::pattern::placed(operation).into();
        let index = self.features.len();
        let values = |copied: &str, step: usize| -> [u8; 32] {
            let mut digest = digest.clone();
            digest.update(format!("\n{copied} {step}").as_bytes());
            digest.finalize().into()
        };
        let pattern_step = PatternStep {
            feature: index,
            base: b,
            instances: Arc::clone(&instances),
        };
        if instances.is_empty() {
            // The identity: no copy placed, its references still resolved and checked.
            let key = self.add(
                Step::Copies {
                    base: base_key,
                    tool: None,
                    op: CopiesOp::Fuse,
                    copies: 0,
                    values: values("identity", 0),
                },
                &feature.id,
            );
            self.patterns.insert(key, pattern_step);
            planned.steps.push(key);
            planned.naming = Some(Naming::Pattern(Arc::new(PatternNaming {
                operation: kind.into(),
                steps: Vec::new(),
            })));
            return planned;
        }
        let mut copies = Vec::new();
        let mut checked = Vec::new();
        let mut on = base_key;
        if operation
            .copies()
            .is_some_and(|c| *c == crate::document::Copies::Solid)
        {
            let key = self.add(
                Step::Copies {
                    base: base_key,
                    tool: Some(base_key),
                    op: CopiesOp::Fuse,
                    copies: instances.len(),
                    values: values("S", 0),
                },
                &feature.id,
            );
            self.patterns.insert(key, pattern_step);
            planned.steps.push(key);
            copies.push(CopyNaming {
                copied: Copied::Solid,
                original: None,
                instances: Arc::clone(&instances),
            });
            checked.push((None, CopiesOp::Fuse, Arc::clone(&instances)));
        } else {
            for (step, (id, naming, faces, sweep, op)) in tools.into_iter().enumerate() {
                let key = self.add(
                    Step::Copies {
                        base: on,
                        tool: Some(sweep),
                        op,
                        copies: instances.len(),
                        values: values("C", step),
                    },
                    &feature.id,
                );
                self.patterns.insert(key, pattern_step.clone());
                planned.steps.push(key);
                planned.tools.extend([faces, sweep]);
                copies.push(CopyNaming {
                    copied: Copied::Tool,
                    original: Some((id.clone(), naming)),
                    instances: Arc::clone(&instances),
                });
                checked.push((Some(id), op, Arc::clone(&instances)));
                on = key;
            }
        }
        planned.naming = Some(Naming::Pattern(Arc::new(PatternNaming {
            operation: kind.into(),
            steps: copies,
        })));
        planned.check = self.feature_id(b).cloned().map(|base| {
            Arc::new(PatternCheck {
                kind,
                feature: feature.id.clone(),
                base,
                steps: checked,
            })
        });
        planned
    }

    /// Whether feature `i`'s result passes its check ([`Check`]), from what `done` holds:
    /// yes, no, or not yet known (its solid, or its base's, not yet built or measured). A
    /// feature with no check: yes. The verdict is a function of the step's key alone (its
    /// result and its base's), the same for every feature sharing it; `model` words each
    /// feature's own refusal.
    fn verdict(&self, i: usize, done: &BTreeMap<Key, StepResult>) -> Option<bool> {
        let planned = self.features.get(i)?;
        let Some(check) = &planned.check else {
            return Some(true);
        };
        if let Check::Pattern(pattern) = check {
            let base = planned
                .uses
                .first()
                .and_then(|&b| self.features.get(b))
                .and_then(Planned::key)?;
            let steps: Option<Vec<&Built>> = std::iter::once(&base)
                .chain(&planned.steps)
                .map(|key| match done.get(key) {
                    Some(Ok(built)) => Some(&**built),
                    _ => None,
                })
                .collect();
            return pattern_holds(pattern, &steps?);
        }
        let built = match done.get(&planned.key()?) {
            Some(Ok(built)) => built,
            _ => return None,
        };
        // The base's measures: a sweep's and a boolean's base is its first input.
        let before = || {
            let base = planned
                .uses
                .first()
                .and_then(|&b| self.features.get(b))
                .and_then(Planned::key)?;
            match done.get(&base) {
                Some(Ok(base)) => measured_of(base),
                _ => None,
            }
        };
        match check {
            Check::Boolean { op, .. } => {
                if built.history.as_ref().map_or(1, |h| h.result.solids) > 1 {
                    return Some(false);
                }
                Some(crate::sweep::boolean_change_holds(
                    *op,
                    &before()?,
                    &measured_of(built)?,
                ))
            }
            Check::Sweep(sweep) => Some(crate::sweep::result_holds(
                sweep.kind,
                &before()?,
                &measured_of(built)?,
            )),
            // Judged above, every step.
            Check::Pattern(_) => None,
        }
    }

    /// The features refused so far, from `done`, and every feature made from one (its base,
    /// tool, or sketch one, all the way down).
    fn refusals(&self, done: &BTreeMap<Key, StepResult>) -> Refusals {
        let mut refusals = Refusals::default();
        for (i, planned) in self.features.iter().enumerate() {
            let after = planned
                .uses
                .iter()
                .chain(&planned.sweeps)
                .any(|u| refusals.dead.contains(u));
            let refused = self.verdict(i, done) == Some(false);
            if refused {
                refusals.refused.insert(i);
            }
            if after || refused {
                refusals.dead.insert(i);
            }
        }
        refusals
    }

    /// Each feature's state as a round's plan finds it, by place (a suppressed feature's
    /// is never read): from the results `done` holds (kept, read from the cache, or carried
    /// from the rounds before), the steps `doomed` (an input failed), the sketches `solved`
    /// so far, the features `refusals` holds, and the solids `unmeasured`. As the model words
    /// its outcomes, in timeline order: a sketch is placed, then solved (one whose face isn't
    /// resolved yet is queued); a feature using one that failed, faulted, or was blocked is
    /// blocked; one that waits on a face queued; then its own failure (the plan's refusal, a
    /// check's, a measure's, or its steps' first) if it has one, said once all it uses is
    /// firm (built, its check passed, and all it uses firm) and queued until then, since
    /// the model says blocked if what it uses fails its check; then built once its last step
    /// is made; and otherwise queued. With them, the checks not decided yet, the failures
    /// held, and which features are firm, for the count to go on from ([`Found`]).
    fn states(
        &self,
        done: &BTreeMap<Key, StepResult>,
        doomed: &BTreeSet<Key>,
        solved: &SketchResults,
        refusals: &Refusals,
        unmeasured: &BTreeMap<Key, Error>,
    ) -> Found {
        let mut states: Vec<StepState> = Vec::with_capacity(self.features.len());
        let mut firm: Vec<bool> = Vec::with_capacity(self.features.len());
        let mut unchecked = BTreeSet::new();
        let mut held = BTreeMap::new();
        for (i, planned) in self.features.iter().enumerate() {
            let inputs = || planned.uses.iter().chain(&planned.sweeps);
            let unbuilt = |u: &usize| {
                matches!(
                    states.get(*u),
                    Some(StepState::Failed | StepState::Faulted | StepState::Blocked)
                )
            };
            let inputs_firm = inputs().all(|u| firm.get(*u) == Some(&true));
            if planned.check.is_some()
                && self.verdict(i, done).is_none()
                && self.unmeasured_failure(i, done, unmeasured).is_none()
            {
                unchecked.insert(i);
            }
            let state = if planned.suppressed {
                StepState::Queued
            } else if let Some(key) = planned.sketch {
                match (&planned.on_face, solved.get(&key)) {
                    (OnFace::Waits, _) => StepState::Queued,
                    (OnFace::Placed(Placed::Broken(_)), _) => StepState::Failed,
                    (OnFace::Placed(Placed::Failed(error)), _) => StepState::failed(error),
                    (OnFace::Placed(Placed::Blocked(_)), _) => StepState::Blocked,
                    (OnFace::No | OnFace::Placed(Placed::Frame(_)), Some(solved)) => {
                        match &solved.result {
                            Ok(_) => StepState::Built,
                            Err(error) => StepState::failed(error),
                        }
                    }
                    (OnFace::No | OnFace::Placed(Placed::Frame(_)), None) => StepState::Queued,
                }
            } else if inputs().any(unbuilt) {
                StepState::Blocked
            } else if self.waits.contains(&i) {
                StepState::Queued
            } else {
                let own = if let Some(error) = &planned.refused {
                    Some(StepState::failed(error))
                } else if refusals.refused.contains(&i) {
                    Some(StepState::Failed)
                } else if let Some(error) = self.unmeasured_failure(i, done, unmeasured) {
                    Some(StepState::failed(error))
                } else {
                    planned.steps.iter().find_map(|key| match done.get(key) {
                        Some(Err(error)) => Some(StepState::failed(error)),
                        _ => None,
                    })
                };
                match (own, planned.steps.last().map(|key| done.get(key))) {
                    (Some(state), _) if inputs_firm => state,
                    // The plan's refusal is the count's to say, from the plan, once all it
                    // uses is firm; any other failure is held for it.
                    (Some(state), _) => {
                        if planned.refused.is_none() {
                            held.insert(i, state);
                        }
                        StepState::Queued
                    }
                    _ if planned.steps.iter().any(|key| doomed.contains(key)) => StepState::Blocked,
                    (None, Some(Some(Ok(_)))) => StepState::Built,
                    (None, _) => StepState::Queued,
                }
            };
            firm.push(state == StepState::Built && inputs_firm && !unchecked.contains(&i));
            states.push(state);
        }
        Found {
            states,
            unchecked,
            held,
            firm,
        }
    }

    /// The features whose checks the result (or measure) of `key`, now that `done` holds it,
    /// has passed.
    fn passed(&self, key: &Key, done: &BTreeMap<Key, StepResult>) -> Vec<usize> {
        self.judged_by
            .get(key)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&f| self.verdict(f, done) == Some(true))
            .collect()
    }

    /// The features whose checks the measure of `key` decided they fail for want of a
    /// measure ([`unmeasured_failure`](Self::unmeasured_failure)), each with its state.
    fn unmeasured_failures(
        &self,
        key: &Key,
        done: &BTreeMap<Key, StepResult>,
        unmeasured: &BTreeMap<Key, Error>,
    ) -> Vec<(usize, StepState)> {
        self.judged_by
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|&f| {
                let error = self.unmeasured_failure(f, done, unmeasured)?;
                Some((f, StepState::failed(error)))
            })
            .collect()
    }

    /// What the check of the feature at `i` fails with for want of a measure, as the model
    /// finds it ([`checked`]): a sweep's check, its solid built, and that solid's measure
    /// refused, or else, its solid measured, its base's. None while a measure it needs is
    /// still to come, or if none is refused.
    fn unmeasured_failure<'a>(
        &self,
        i: usize,
        done: &BTreeMap<Key, StepResult>,
        unmeasured: &'a BTreeMap<Key, Error>,
    ) -> Option<&'a Error> {
        let planned = self.features.get(i)?;
        if let Some(Check::Pattern(_)) = &planned.check {
            // Any of its steps' measures refused, once built, or its base's.
            let base = planned
                .uses
                .first()
                .and_then(|&b| self.features.get(b))
                .and_then(Planned::key);
            return base
                .iter()
                .chain(&planned.steps)
                .find_map(|key| match done.get(key) {
                    Some(Ok(_)) => unmeasured.get(key),
                    _ => None,
                });
        }
        let Some(Check::Sweep(_)) = &planned.check else {
            return None;
        };
        let key = planned.key()?;
        let Some(Ok(built)) = done.get(&key) else {
            return None;
        };
        if let Some(error) = unmeasured.get(&key) {
            return Some(error);
        }
        measured_of(built)?;
        let base = planned
            .uses
            .first()
            .and_then(|&b| self.features.get(b))
            .and_then(Planned::key)?;
        unmeasured.get(&base)
    }

    /// Adds to `refusals` the features `key`'s result (or measure) refuses, now that `done`
    /// holds it, and every feature made from one: those it refuses, by index.
    fn judge(
        &self,
        key: &Key,
        done: &BTreeMap<Key, StepResult>,
        refusals: &mut Refusals,
    ) -> Vec<usize> {
        let mut refused = Vec::new();
        for &f in self.judged_by.get(key).into_iter().flatten() {
            if refusals.refused.contains(&f) || self.verdict(f, done) != Some(false) {
                continue;
            }
            refused.push(f);
            refusals.refused.insert(f);
            let mut stack = vec![f];
            while let Some(i) = stack.pop() {
                if refusals.dead.insert(i) {
                    stack.extend(self.dependants.get(i).into_iter().flatten().copied());
                }
            }
        }
        refused
    }

    /// Whether step `key` is to be run: it serves a feature that isn't dead, or a refused
    /// feature that's reported as its own refusal (made from no other refused one), whose
    /// outcome is read from every one of its steps. A refused feature's last step may be
    /// kept or cached without its earlier ones (a cache entry dropped, a write that
    /// failed), so those are built again, as a fresh recompute builds them.
    fn needed(&self, key: &Key, refusals: &Refusals) -> bool {
        self.owners.get(key).is_none_or(|owners| {
            owners.iter().any(|&f| {
                !refusals.dead.contains(&f)
                    || (refusals.refused.contains(&f) && !self.made_from_dead(f, refusals))
            })
        })
    }

    /// Whether feature `i` is made from a dead feature: then it's blocked on it, and its
    /// own steps aren't read.
    fn made_from_dead(&self, i: usize, refusals: &Refusals) -> bool {
        self.features.get(i).is_some_and(|planned| {
            planned
                .uses
                .iter()
                .chain(&planned.sweeps)
                .any(|u| refusals.dead.contains(u))
        })
    }

    /// The step of a fillet or chamfer `feature` (the next feature planned), of its base
    /// `base`, by place (its effective base: the feature whose solid stands for the one it
    /// names): its steps, and the features it uses.
    fn dress(
        &mut self,
        feature: &crate::document::Feature,
        base: Option<usize>,
        dressing: Dressing,
    ) -> (Vec<Key>, Vec<usize>) {
        // The gate has checked that the base is an earlier feature, with something
        // unsuppressed up its chain, so it's planned.
        let Some(b) = base else {
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

    /// How the feature `id` makes its elements, as [`Name::parse`](crate::naming::Name::parse)
    /// asks it of each feature a name mentions: found in the planned document by its index,
    /// for only the features a fillet's or chamfer's names mention.
    fn maker(&self, id: &FeatureId) -> Option<Maker> {
        let feature = self.document.feature(id).ok()?;
        Some(Maker::of(&feature.operation))
    }

    /// Each step's progress label, by key: the ID and type of the first feature it's
    /// planned for (`4kq2m7vd pad`), its type found by `document`'s index, one lookup a
    /// step, so labelling a long timeline isn't quadratic.
    fn labels(&self, document: &Checked) -> BTreeMap<Key, String> {
        self.first_feature
            .iter()
            .map(|(key, id)| {
                let kind = document
                    .feature(id)
                    .map_or("", |feature| feature.operation.type_name());
                (*key, format!("{id} {kind}"))
            })
            .collect()
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
            let mut steps = Vec::with_capacity(planned.steps.len() + planned.tools.len());
            for key in planned.naming_steps() {
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
    // The identity passes its base through, history and all: its base's, whatever its
    // step, which holds together.
    if let Step::Copies { tool: None, .. } = step {
        return built
            .history
            .as_ref()
            .is_none_or(|history| history.validate(history.inputs.len()).is_ok());
    }
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
    /// Its summary, its description not had (not yet asked, or failed).
    Summary,
    /// Its summary, and why its description was refused or hung, kept.
    Kept,
    /// Its summary, and its description: a kept refusal or hang never replaces it.
    Whole,
}

/// How much `built` knows of its measure.
fn measure_known(built: &Built) -> MeasureKnown {
    match (built.summary(), built.described(), built.measure_refused()) {
        (Some(_), Some(Described::Whole(_)), _) => MeasureKnown::Whole,
        (Some(_), Some(Described::Refused(_)), _) => MeasureKnown::Kept,
        (Some(_), _, _) => MeasureKnown::Summary,
        (None, _, Some(_)) => MeasureKnown::Refused,
        (None, _, None) => MeasureKnown::Nothing,
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
        (_, Err(error)) => kept(error) || kept_hang(error),
    }
}

/// Whether a step's failure follows from its values, so asking again would fail again.
fn kept(error: &Error) -> bool {
    matches!(
        error.code(),
        ErrorCode::InvalidInput | ErrorCode::KernelFailed | ErrorCode::InvalidResult
    )
}

/// Why a pattern's step whose copies hung is kept for the session (section 4.2, *A hang
/// kept*): the advice its error says, as a description's that hung says [`HUNG_KEPT`].
const COPIES_HUNG: &str = "making this pattern's copies hung, which a busy machine can cause \
     as well as the copies; the session keeps that rather than wait on it at every rebuild";

/// Whether `error` is a pattern's step's hang, kept for the session as a description's is
/// ([`hung`]): never on disk, since a hang isn't a fact about the shape (`cache::lasting`).
fn kept_hang(error: &Error) -> bool {
    error.code() == ErrorCode::WorkerTimeout
        && matches!(
            error.advice(),
            Some(crate::worker::Advice::TryAgain { why, .. }) if why == COPIES_HUNG
        )
}

/// Why a description that hung is kept, and that it may not hang again: the advice a read of
/// the solid's elements words as `check` with `fresh` to try it again.
const HUNG_KEPT: &str = "describing this solid hung, which a busy machine can cause as well \
     as the solid; the session keeps that rather than wait on it at every rebuild";

/// Whether `error` is a hang of a worker that had started, which `measure` keeps for the
/// session beside the solid's summary, with [`HUNG_KEPT`] as its advice, as it keeps a
/// refusal for the solid's values ([`kept`]): not asked again until something changes its
/// step (a fresh rebuild asks again). A describe that hangs would likely hang again on the
/// same solid, and each rebuild asking it again would hold a worker for the kernel's
/// timeout (a viewport reading every rebuild would ask it every time). Not on disk: a hang
/// isn't a fact about the shape (`cache::lasting`), so the next session asks once again. A
/// worker that hadn't started (a slow first launch, whose advice is to try again) says
/// nothing about the solid, and is asked again, as a crash is.
fn hung(error: &Error) -> bool {
    error.code() == ErrorCode::WorkerTimeout
        && !matches!(error.advice(), Some(crate::worker::Advice::TryAgain { .. }))
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

impl Ran {
    /// Logs, at info, why the step failed, if it did: the first broken reference of one it
    /// couldn't run, or its error. A failure is the document's, not the process's, so it's
    /// no warning; the rebuild's result reports it too.
    fn log_failure(&self) {
        let error = match self {
            Ran::Made(Outcome::Done(Err(error))) | Ran::Measured(Outcome::Done(Err(error))) => {
                error
            }
            Ran::Unresolved(broken) => match broken.errors.first() {
                Some(error) => error,
                None => return,
            },
            _ => return,
        };
        error.log("the step failed");
    }
}

/// An error that's a bug in Silico.
fn silico_bug(message: &str) -> Error {
    Error::new(ErrorCode::Internal, message).with_suggestion(crate::worker::SILICO_BUG)
}

/// What every round of one rebuild shares ([`Engine::run`]): its document, the sketches it
/// solved before its first plan, its generation, its count of steps, what it reads of what's
/// kept, and the element maps its features are named with.
#[derive(Clone, Copy)]
struct Rebuild<'a> {
    document: &'a Arc<Checked>,
    first: &'a SolvedFirst,
    generation: Generation,
    progress: &'a StepCounter,
    reads: Reads,
    known: &'a NamedMaps,
}

/// What a round of a rebuild takes from those before it: its number, from 0, and the
/// results they had, by key, with the solids they couldn't measure, the solids they measured
/// or tried to, and the fillets whose edges didn't resolve: the steps they hold aren't asked
/// again, nor the measures (a measure a worker failed is asked again by the next rebuild,
/// not the next round).
#[derive(Default)]
struct Carried {
    round: usize,
    done: BTreeMap<Key, StepResult>,
    unmeasured: BTreeMap<Key, Error>,
    measured: BTreeSet<Key>,
    unresolved: BTreeMap<Key, crate::references::Broken>,
}

/// A round built ([`Engine::build_round`]): its schedule as it ended, and its plan's keys,
/// its steps' and solves'.
struct Round {
    schedule: Schedule,
    keys: Arc<BTreeSet<Key>>,
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
    /// Features' solids measured, or tried, in this rebuild (any round), by their steps'
    /// keys: none is measured twice in one rebuild.
    measured: BTreeSet<Key>,
    running: usize,
    superseded: bool,
    /// A step that panicked, a bug in Silico: the recompute stops, and fails with this.
    broken: Option<Error>,
    /// Fillets' and chamfers' steps whose edges didn't all resolve, by key: their broken
    /// references' errors (the first hundred), and how many there are.
    unresolved: BTreeMap<Key, crate::references::Broken>,
    /// Features refused by their result's check, and every feature made from one: a step
    /// only these need isn't started (one already running finishes, unused).
    refusals: Refusals,
}

/// Features refused by their result's check, and what's made from them, by index.
#[derive(Default)]
struct Refusals {
    /// Each feature its own check refused.
    refused: BTreeSet<usize>,
    /// Those and every feature made from one (its base, tool, or sketch one, all the way
    /// down).
    dead: BTreeSet<usize>,
}

impl<K: Kernel + 'static> Engine<K> {
    /// Rebuilds `document` for `generation`, in rounds (docs/ports/attacher.md, *In
    /// rounds*): each round plans the steps of every feature whose sketches' planes are
    /// known, builds them, and then resolves the face of each sketch on a face whose live
    /// solids that built; the next plans again with those frames, the steps already built
    /// kept, by key, in memory. It ends when a round resolves nothing new, and the model of
    /// that round, the last, is published, once. Each round's plan is a function of the
    /// document and the frames before it, so the rounds and the model don't depend on which
    /// thread finishes first. Sketches are solved in the first round only: later rounds find
    /// every solve done.
    fn run(
        &self,
        document: &Arc<Checked>,
        generation: Generation,
        progress: &StepCounter,
        reads: Reads,
    ) -> Result<RunOutcome, Error> {
        let kept = reads == Reads::Kept;
        // Every sketch a feature sweeps is solved, and its profile made, before the plan
        // (design S3): a sweep's steps depend on its profile. Unchanged sketches cost
        // nothing (their solves and profiles are kept by key), unless nothing kept is read.
        let first = match self.solve_swept(document, generation, reads, progress)? {
            Some(first) => first,
            None => return Ok(RunOutcome::Superseded),
        };
        // The maps its features are named with: those named before, or, reading nothing
        // kept, none, each named again from what this recompute built (and, once its model
        // is published, the engine's).
        let known: NamedMaps = if kept {
            Arc::clone(&lock(&self.state).known)
        } else {
            NamedMaps::default()
        };
        let rebuild = Rebuild {
            document,
            first: &first,
            generation,
            progress,
            reads,
            known: &known,
        };
        let mut solutions: SketchResults = BTreeMap::new();
        let mut frames: BTreeMap<FeatureId, Placed> = BTreeMap::new();
        let mut carried = Carried::default();
        let (model, keys) = loop {
            let round = carried.round;
            // A recompute cancelled or superseded meanwhile plans no more rounds: a warm
            // round asks nothing of the kernel, so nothing else would stop it.
            if round > 0 {
                let state = lock(&self.state);
                if state.generation != generation || state.cancelled {
                    return Ok(RunOutcome::Superseded);
                }
            }
            let plan = Plan::of(document, &first.swept, &frames);
            let Some(Round { schedule, keys }) =
                self.build_round(&rebuild, &plan, carried, &mut solutions)?
            else {
                return Ok(RunOutcome::Superseded);
            };
            let model = model(
                document,
                &plan,
                &schedule,
                &solutions,
                generation,
                Arc::clone(&known),
            );
            let Faces { placed, waiting } = placed_faces(document, &plan, &model, &first.swept);
            if !placed.is_empty() {
                tracing::debug!(round, faces = placed.len(), "sketches' faces resolved");
                frames.extend(placed);
            } else if !waiting.is_empty() {
                // Nothing new, yet some still wait: none ever will be placed (every round
                // places the first that waits, so this is a bug's), and each is blocked,
                // naming the feature it waits for, in one round more.
                frames.extend(
                    waiting
                        .into_iter()
                        .map(|(id, on)| (id, Placed::Blocked(on))),
                );
            } else {
                progress.concluded(&model);
                break (model, keys);
            }
            carried = Carried {
                round: round + 1,
                done: schedule.done,
                unmeasured: schedule.unmeasured,
                measured: schedule.measured,
                unresolved: schedule.unresolved,
            };
        };
        let model = Arc::new(model);
        let mut state = lock(&self.state);
        if state.generation != generation || state.cancelled {
            return Ok(RunOutcome::Superseded);
        }
        // Named anew, its maps are the engine's from here: those named before, of the
        // histories it built again, aren't read again.
        if !kept {
            state.known = known;
        }
        // Maps only the models worth keeping name are kept: theirs, and their sketches'
        // faces'.
        let named: BTreeSet<NamingKey> = model
            .naming
            .iter()
            .flatten()
            .flat_map(|n| {
                let faces: Vec<NamingKey> = match &n.how {
                    Naming::Sweep(sweep) => vec![sweep.faces],
                    // A pattern's copies of its originals' tools are named from their
                    // sketches' faces too.
                    Naming::Pattern(pattern) => pattern
                        .steps
                        .iter()
                        .filter_map(|step| step.original.as_ref().map(|(_, s)| s.faces))
                        .collect(),
                    Naming::Primitive(_) | Naming::Operation(_) => Vec::new(),
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

    /// One round of `rebuild` ([`run`](Self::run)): `plan`'s steps built, those `carried`
    /// from the rounds before taken as they are, and the rest read from what the engine
    /// keeps and the cache (unless nothing kept is read) or run; and, the first round, every
    /// sketch not solved before the plan solved beside them, into `solutions`. The schedule
    /// as it ended, and the plan's keys (its steps' and solves'); or none, if a newer
    /// recompute superseded this one meanwhile.
    fn build_round(
        &self,
        rebuild: &Rebuild<'_>,
        plan: &Plan,
        carried: Carried,
        solutions: &mut SketchResults,
    ) -> Result<Option<Round>, Error> {
        let Rebuild {
            document,
            first,
            generation,
            progress,
            reads,
            known,
        } = *rebuild;
        let kept = reads == Reads::Kept;
        let first_round = carried.round == 0;
        let keys: Arc<BTreeSet<Key>> = Arc::new(
            plan.steps
                .keys()
                .copied()
                .chain(plan.sketches.iter().map(|(_, key)| *key))
                .collect(),
        );
        let mut done = carried.done;
        {
            let mut state = lock(&self.state);
            // The newest recompute's plan is worth keeping results for, whether or not it
            // finishes: an older one finishing later keeps what this one will use.
            if state.newest_started == generation {
                state.newest_plan = Some(Arc::clone(&keys));
            }
            if first_round {
                // The sketches solved before the plan, and their profiles, kept now that
                // the plan's keys hold them.
                state.profiles.extend(first.profiles.iter().cloned());
            }
            // Reading nothing kept, every step is built and every sketch solved.
            if kept {
                for key in &plan.order {
                    if !done.contains_key(key)
                        && let Some(result) = state.results.get(key)
                    {
                        done.insert(*key, result.clone());
                    }
                }
                for (_, key) in &plan.sketches {
                    if !solutions.contains_key(key)
                        && let Some(result) = state.solutions.get(key)
                    {
                        solutions.insert(*key, result.clone());
                    }
                }
            }
            if first_round {
                solutions.extend(first.solutions.iter().map(|(k, r)| (*k, r.clone())));
            }
        }
        if first_round {
            self.keep_solutions(&first.fresh);
        }
        // The sketches not solved before, solved on a thread of their own: in the first
        // round, which leaves none for the rounds after.
        let to_solve: Vec<(FeatureId, Key)> = plan
            .sketches
            .iter()
            .filter(|(_, key)| !solutions.contains_key(key))
            .cloned()
            .collect();
        // What's kept on disk for this kernel, for the steps not kept in memory.
        if let Some(cache) = self.cache.get().filter(|_| kept) {
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
            unmeasured: carried.unmeasured,
            measured: carried.measured,
            unresolved: carried.unresolved,
            refusals: Refusals::default(),
            running: 0,
            superseded: false,
            broken: None,
        };
        // Features refused already, by results kept or read from the cache.
        schedule.refusals = plan.refusals(&schedule.done);
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
        // Features' solids built before (kept, or from the cache) but not summarized or
        // described yet, unless the kernel refused to for their values (it would again), or
        // a round before this one measured them or tried to (a worker that failed is asked
        // again by the next rebuild).
        for key in &plan.results {
            if let Some(Ok(built)) = schedule.done.get(key) {
                if built.needs_measure() {
                    if schedule.measured.insert(*key) {
                        schedule.ready.push_back(Job::Measure(*key));
                    }
                } else if let Some(error) = built.measure_refused() {
                    schedule.unmeasured.insert(*key, error.clone());
                }
            }
        }
        let to_run = schedule.ready.len() + schedule.waiting.len();
        tracing::debug!(steps = plan.order.len(), to_run, "recompute planned");
        // Settled before any runs: those kept, and those that can't run.
        let settled = plan
            .order
            .iter()
            .filter(|key| schedule.done.contains_key(key) || doomed.contains(key))
            .copied();
        let found = plan.states(
            &schedule.done,
            &doomed,
            solutions,
            &schedule.refusals,
            &schedule.unmeasured,
        );
        progress.planned(plan.order.len(), settled, plan.labels(document), &found);
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
                            self.solve_sketches(document, &to_solve, generation, progress)
                        }))
                    })
                    .map_err(|e| crate::worker::thread_error("recompute", &e))?;
                Some(spawned)
            };
            start_threads(threads, |n| {
                std::thread::Builder::new()
                    .name(format!("recompute-{}-{n}", generation.0))
                    .spawn_scoped(scope, || {
                        let _entered = parent.enter();
                        self.serve(plan, &schedule, &changed, generation, progress, known);
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
                    progress.sketches_panicked(
                        to_solve.iter().filter_map(|(id, _)| reported(document, id)),
                        Some(plan),
                    );
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
            return Ok(None);
        };
        self.keep_solutions(&solved);
        solutions.extend(solved.into_iter().map(|(key, s)| (key, s.result())));
        if schedule.superseded {
            return Ok(None);
        }
        Ok(Some(Round { schedule, keys }))
    }

    /// The sketches `document`'s pads, pockets, revolves, and grooves sweep, each solved
    /// (those not solved before, now, for `generation`) and its profile made: by sketch,
    /// those that solved. None if a newer recompute supersedes this one meanwhile.
    fn solve_swept(
        &self,
        document: &Arc<Checked>,
        generation: Generation,
        reads: Reads,
        progress: &StepCounter,
    ) -> Result<Option<SolvedFirst>, Error> {
        let (sketches, swept) = Plan::sketches_of(document);
        let used: Vec<(FeatureId, Key)> = sketches
            .into_iter()
            .filter(|(id, _)| swept.contains(id))
            .collect();
        if used.is_empty() {
            return Ok(Some(SolvedFirst::default()));
        }
        let _span = tracing::debug_span!("solve_swept", sketches = used.len()).entered();
        let (mut solutions, mut profiles): (SketchResults, BTreeMap<Key, ProfileResult>) =
            if reads == Reads::Nothing {
                (BTreeMap::new(), BTreeMap::new())
            } else {
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
        let mut fresh = SketchRun::new();
        if !to_solve.is_empty() {
            // A panic under a solve is a bug in Silico, as under a step.
            let solved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.solve_sketches(document, &to_solve, generation, progress)
            }))
            .map_err(|panic| {
                // What sweeps it isn't planned yet: blocked as a plan of the document has it.
                let plan = Plan::of(document, &BTreeMap::new(), &BTreeMap::new());
                progress.sketches_panicked(
                    to_solve.iter().filter_map(|(id, _)| reported(document, id)),
                    Some(&plan),
                );
                Error::new(ErrorCode::Internal, "a sketch's solve panicked")
                    .with_cause(panic_text(panic.as_ref()))
                    .with_suggestion(crate::worker::SILICO_BUG)
            })?;
            let Some(solved) = solved else {
                return Ok(None);
            };
            solutions.extend(solved.iter().map(|(key, s)| (*key, s.clone().result())));
            fresh = solved;
        }
        let mut swept_sketches = BTreeMap::new();
        let mut made = Vec::new();
        for (id, key) in &used {
            let Some(SketchResult {
                result: Ok(solution),
                ..
            }) = solutions.get(key)
            else {
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
        Ok(Some(SolvedFirst {
            swept: swept_sketches,
            solutions,
            fresh,
            profiles: made,
        }))
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
    /// failure only if it follows from the sketch (it would fail again), and only with its
    /// diagnosis (a solver that failed diagnosing a sketch the translation refuses may not
    /// fail again).
    fn keep_solutions(&self, solved: &SketchRun) {
        let mut state = lock(&self.state);
        for (key, outcome) in solved {
            let SketchOutcome::Solved(result) = outcome else {
                continue;
            };
            let keep = result.diagnosed.is_ok()
                && match &result.result {
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
        progress: &StepCounter,
    ) -> Option<SketchRun> {
        let mut solved = SketchRun::new();
        let superseded = |state: &EngineState| state.generation != generation || state.cancelled;
        // The engine's turn at its solvers, once no other recompute of its has it; or none,
        // once this one is superseded (a newer recompute's start wakes the wait), never
        // waiting for a newer one's solve.
        {
            let mut state = lock(&self.state);
            loop {
                if superseded(&state) {
                    return None;
                }
                if !state.sketching {
                    state.sketching = true;
                    break;
                }
                state = self
                    .sketch_turn
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
        let mut lent = LentSolver {
            state: &self.state,
            turn: &self.sketch_turn,
            solvers: lock(&self.solvers).clone(),
            solver: None,
        };
        match lent.solvers.take() {
            Ok(solver) => lent.solver = Some(solver),
            // Every sketch fails with it, not kept: the next recompute tries again.
            Err(error) => {
                let state = StepState::failed(&error);
                for (id, key) in to_solve {
                    progress.sketch_settled(reported(document, id), state);
                    solved.insert(*key, solver_failure(error.clone(), id));
                }
                return Some(solved);
            }
        }
        let solver = lent.solver.as_mut()?;
        for (id, key) in to_solve {
            let place = reported(document, id);
            let stopper = match solver.stopper() {
                Ok(stopper) => stopper,
                // The solver's failure, not the sketch's: not kept.
                Err(error) => {
                    progress.sketch_settled(place, StepState::failed(&error));
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
            progress.sketch_started(place);
            let result = solve_diagnosed(document, id, solver.as_mut());
            let mut state = lock(&self.state);
            state.solving = None;
            // Superseded: the recompute ends, leaving the sketch queued.
            if superseded(&state) {
                return None;
            }
            drop(state);
            let settled = match &result.result {
                Ok(_) => StepState::Built,
                Err(error) => StepState::failed(error),
            };
            progress.sketch_settled(place, settled);
            solved.insert(*key, SketchOutcome::Solved(result));
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
        progress: &StepCounter,
        known: &NamedMaps,
    ) {
        loop {
            let (job, inputs, sources, named_on) = {
                let mut s = lock(schedule);
                let (job, sources, named_on) = loop {
                    if s.superseded || s.broken.is_some() || (s.ready.is_empty() && s.running == 0)
                    {
                        changed.notify_all();
                        return;
                    }
                    let Some(job) = s.ready.pop_front() else {
                        s = changed.wait(s).unwrap_or_else(PoisonError::into_inner);
                        continue;
                    };
                    // A step only refused features' users need isn't started: they're
                    // reported blocked, whatever ran.
                    if let Job::Make(key) = job
                        && !plan.needed(&key, &s.refusals)
                    {
                        progress.settled(key, Settled::Unrun, plan);
                        continue;
                    }
                    // A fillet's, chamfer's, or pattern's base is built: what the elements
                    // it names are named from. Or a solid that isn't built yet, which it
                    // waits for (and, if it failed, waits for forever: the failure blocks
                    // it). A pattern's base's solid too, whose elements it describes.
                    let names_base = match job {
                        Job::Make(key) => plan
                            .dresses
                            .get(&key)
                            .map(|d| d.base)
                            .or_else(|| plan.patterns.get(&key).map(|p| p.base)),
                        Job::Measure(_) => None,
                    };
                    let sources = match names_base.map(|b| plan.sources(b, &s.done, known)) {
                        None => None,
                        Some(Ok(sources)) => Some(sources),
                        Some(Err(unbuilt)) => {
                            s.users.entry(unbuilt).or_default().push(job.key());
                            s.waiting.insert(job.key(), 1);
                            continue;
                        }
                    };
                    let pattern_base = match job {
                        Job::Make(key) => plan.patterns.get(&key).map(|p| p.base),
                        Job::Measure(_) => None,
                    };
                    let named_on = match pattern_base
                        .and_then(|b| plan.features.get(b))
                        .and_then(Planned::key)
                    {
                        None => None,
                        Some(base) => match s.done.get(&base) {
                            Some(Ok(built)) => Some(Arc::clone(built)),
                            _ => {
                                s.users.entry(base).or_default().push(job.key());
                                s.waiting.insert(job.key(), 1);
                                continue;
                            }
                        },
                    };
                    break (job, sources, named_on);
                };
                s.running += 1;
                if let Job::Make(key) = job {
                    progress.started(key, plan);
                }
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
                (job, inputs, sources, named_on)
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
                let ran = match job {
                    Job::Make(key) if let Some(pattern) = plan.patterns.get(&key) => {
                        let sources = sources.unwrap_or_default();
                        match (plan.steps.get(&key), named_on.as_ref()) {
                            (Some(step), Some(base)) => self.copied(
                                plan, pattern, step, &inputs, base, &sources, known, generation,
                            ),
                            _ => Ran::Made(Outcome::Done(Err(silico_bug(
                                "a pattern's step ran before its base was built",
                            )))),
                        }
                    }
                    Job::Make(key) => match (plan.steps.get(&key), plan.dresses.get(&key)) {
                        (Some(step), Some(dress)) => {
                            let sources = sources.unwrap_or_default();
                            match self.resolve(plan, dress, &sources, known) {
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
                };
                // In the step's span, so a session's logs hold why its feature failed.
                ran.log_failure();
                ran
            }));
            let mut s = lock(schedule);
            let ran = match outcome {
                Ok(ran) => ran,
                Err(panic) => {
                    // A bug's failure, as a fault's: its features faulted, and what uses them
                    // blocked, not left queued under a faulted rebuild.
                    progress.settled(key, Settled::Panicked, plan);
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
            if let Job::Make(key) = job {
                let settled = match &ran {
                    Ran::Made(Outcome::Done(Ok(_))) => Settled::Made,
                    Ran::Made(Outcome::Done(Err(error))) => {
                        Settled::Failed(StepState::failed(error))
                    }
                    Ran::Unresolved(broken) => Settled::Failed(
                        if broken.errors.iter().any(|e| e.code().is_kernel_fault()) {
                            StepState::Faulted
                        } else {
                            StepState::Failed
                        },
                    ),
                    Ran::Made(Outcome::Superseded) | Ran::Measured(_) => Settled::Superseded,
                };
                progress.settled(key, settled, plan);
            }
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
                    let made = result.is_ok();
                    let result = result.map(Arc::new);
                    // A feature's solid is kept on disk once it's measured, with its
                    // summary and description (written now too, it could land after, and
                    // over, that).
                    if let Ok(built) = &result
                        && !plan.results.contains(&key)
                    {
                        to_keep = Some(Arc::clone(built));
                    }
                    s.done.insert(key, result);
                    if made {
                        woken += release(&mut s, key);
                        let Schedule { done, refusals, .. } = &mut *s;
                        let refused = failed(plan.judge(&key, done, refusals));
                        progress.judged(&plan.passed(&key, done), &refused, plan);
                        // A feature's solid is measured once it's made, by a job of its own:
                        // what uses it doesn't wait for that.
                        // This thread takes a job itself next (it has nothing to write: a
                        // feature's solid is written once measured), so the measure wakes no
                        // other.
                        if plan.results.contains(&key) && s.measured.insert(key) {
                            s.ready.push_back(Job::Measure(key));
                        }
                    }
                }
                Ran::Measured(Outcome::Done(Ok(measured))) => {
                    // The measured copy takes the result's place, here and once kept; the
                    // result itself is never changed, since a published model may hold it.
                    let measured = Arc::new(measured);
                    to_keep = Some(Arc::clone(&measured));
                    s.done.insert(key, Ok(measured));
                    // Measured: so not unmeasured, whatever an earlier try said.
                    s.unmeasured.remove(&key);
                    // The checks this measure decides (its feature's, and those whose base it
                    // is): what only refused features need is no longer started.
                    let Schedule {
                        done,
                        refusals,
                        unmeasured,
                        ..
                    } = &mut *s;
                    let mut refused = failed(plan.judge(&key, done, refusals));
                    // Its base's measure refused, a sweep made from it fails its check.
                    refused.extend(plan.unmeasured_failures(&key, done, unmeasured));
                    progress.judged(&plan.passed(&key, done), &refused, plan);
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
                    // The checks that needed it fail now, as the model will say: its own
                    // sweep's, and those of the sweeps made from it.
                    progress.judged(
                        &[],
                        &plan.unmeasured_failures(&key, &s.done, &s.unmeasured),
                        plan,
                    );
                }
            }
            // A job counts as running until its result is written to the cache, so a
            // recompute doesn't end before its results are on disk; the jobs it made ready
            // start meanwhile.
            drop(s);
            for _ in 0..woken {
                changed.notify_one();
            }
            if let (Some(cache), Some(built)) = (self.cache.get(), to_keep) {
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
        crate::references::resolve(feature, &dress.edges, &map, &|f| plan.maker(f))
    }

    /// Runs a pattern's step, `pattern`'s (docs/design/m6.0-patterns.md, section 2.4): its
    /// references found in its base feature's element map, named from `sources`
    /// (`Plan::sources`), each broken one `reference_broken` with candidates; each named
    /// element described on `base`, its base feature's solid, and the references resolved
    /// from those descriptions and checked; then each instance placed, the placements
    /// checked, and the copies of `inputs`' tool made on `inputs`' base in one request, for
    /// `generation`. The identity places nothing: it passes its base through. A request that
    /// hangs is kept for the session ([`COPIES_HUNG`]).
    #[allow(clippy::too_many_arguments)]
    fn copied(
        &self,
        plan: &Plan,
        pattern: &PatternStep,
        step: &Step,
        inputs: &[Arc<Built>],
        base: &Arc<Built>,
        sources: &BTreeMap<usize, MapSource>,
        known: &NamedMaps,
        generation: Generation,
    ) -> Ran {
        let bug = |message: &str| Ran::Made(Outcome::Done(Err(silico_bug(message))));
        let Step::Copies { op, .. } = *step else {
            return bug("a pattern's step isn't a copies step");
        };
        let Some(feature) = plan
            .feature_id(pattern.feature)
            .and_then(|id| plan.document.feature(id).ok())
        else {
            return bug("a pattern's feature wasn't planned");
        };
        let id = &feature.id;
        let operation = &feature.operation;
        let makers = |f: &FeatureId| plan.maker(f);
        // Each named reference's element in the base feature's map, or why not.
        let named = crate::pattern::named_references(operation);
        let mut found: Vec<(&ElementName, crate::kernel::ElementRef)> = Vec::new();
        if !named.is_empty() {
            let _span = tracing::debug_span!("resolve", references = named.len()).entered();
            let Some(map) = plan.map_of(pattern.base, sources, known) else {
                return bug("a pattern's base was built, but its elements couldn't be named");
            };
            let mut broken = Vec::new();
            let mut candidates: BTreeMap<ElementKind, crate::references::Candidates> =
                BTreeMap::new();
            for (field, name) in named {
                let kind = crate::naming::Name::parse(name.as_str(), &makers)
                    .map_or(ElementKind::Edge, |name| name.kind());
                let (why, parsed) =
                    match crate::references::look_up(name.as_str(), kind, &[&map], &makers) {
                        crate::references::Lookup::Found { element, .. } => {
                            found.push((name, element));
                            continue;
                        }
                        crate::references::Lookup::Refused { why, name } => (why, name),
                        crate::references::Lookup::Missing { name, retyped } => (
                            retyped.unwrap_or_else(|| {
                                format!(
                                    "names no element of {}'s solid: it isn't one of the elements \
                                 that solid is made from, or an edit before this feature \
                                 removed or remade it",
                                    map.feature()
                                )
                            }),
                            Some(name),
                        ),
                    };
                let ranked = match &parsed {
                    Some(parsed) => candidates
                        .entry(kind)
                        .or_insert_with(|| crate::references::Candidates::of(&[&map], kind))
                        .of_name(parsed),
                    None => Vec::new(),
                };
                broken.push(
                    crate::references::refused(&field, name.as_str(), &why, kind)
                        .with_ranked(ranked)
                        .with_feature(id.as_str()),
                );
            }
            if !broken.is_empty() {
                let total = broken.len() as u64;
                return Ran::Unresolved(crate::references::Broken {
                    errors: broken,
                    total,
                });
            }
        }
        // Each found element described on the base feature's solid.
        let mut superseded = false;
        let mut geometry = |name: &ElementName| -> Result<crate::pattern::Geometry, Error> {
            let Some(&(_, element)) = found.iter().find(|(n, _)| *n == name) else {
                return Err(silico_bug(
                    "a pattern's reference wasn't found before it was described",
                ));
            };
            let request = KernelRequest::describe(&base.shape, element.kind, element.index, 1)?;
            match self.kernel.submit(generation, request).wait() {
                Outcome::Done(Ok(batch)) => match element.kind {
                    ElementKind::Face => batch
                        .description
                        .faces
                        .first()
                        .cloned()
                        .map(crate::pattern::Geometry::Face),
                    ElementKind::Edge => batch
                        .description
                        .edges
                        .first()
                        .cloned()
                        .map(crate::pattern::Geometry::Edge),
                    ElementKind::Vertex | ElementKind::Solid => None,
                }
                .ok_or_else(|| {
                    silico_bug("the kernel described no element of a pattern's reference")
                }),
                Outcome::Done(Err(error)) => Err(error.with_feature(id.as_str())),
                Outcome::Superseded => {
                    superseded = true;
                    Err(stopped())
                }
            }
        };
        let resolved = crate::pattern::resolve(id, operation, &mut geometry);
        if superseded {
            return Ran::Made(Outcome::Superseded);
        }
        let resolved = match resolved {
            Ok(resolved) => resolved,
            Err(error) => return Ran::Made(Outcome::Done(Err(error))),
        };
        let Some(on) = inputs.first() else {
            return bug("a pattern's step ran before its base was built");
        };
        // The identity: nothing placed, its base passed through.
        if pattern.instances.is_empty() {
            return Ran::Made(Outcome::Done(Ok((**on).clone())));
        }
        let placements =
            match crate::pattern::placements(id, operation, &resolved, &pattern.instances) {
                Ok(placements) => placements,
                Err(error) => return Ran::Made(Outcome::Done(Err(error))),
            };
        let Some(tool) = inputs.get(1) else {
            return bug("a pattern's step ran before its tool was built");
        };
        let request = match KernelRequest::copies(op, &on.shape, &tool.shape, &placements) {
            Ok(request) => request,
            Err(error) => return Ran::Made(Outcome::Done(Err(error.with_feature(id.as_str())))),
        };
        let _span = tracing::debug_span!("copies", copies = placements.len()).entered();
        match self.kernel.submit(generation, request).wait() {
            Outcome::Done(Ok(data)) => Ran::Made(Outcome::Done(Ok(Built::new(
                data.shape,
                Some(data.history),
                None,
            )))),
            // A hang is kept, as a description's is, but may be a busy machine's: the
            // feature says so, and to check with fresh to ask again (section 4.2).
            Outcome::Done(Err(error)) if hung(&error) => Ran::Made(Outcome::Done(Err(error
                .with_advice(crate::worker::Advice::TryAgain {
                    once_more: false,
                    why: COPIES_HUNG.to_owned(),
                })
                .reworded(
                    "check with fresh (silico check --fresh) to make the copies again; if they hang again, lower a count",
                )))),
            Outcome::Done(Err(error)) => Ran::Made(Outcome::Done(Err(error))),
            Outcome::Superseded => Ran::Made(Outcome::Superseded),
        }
    }

    /// Measures `built`, a feature's solid, on the kernel, for `generation`: a copy of it
    /// with its summary, unless it has one, and what became of its description, a batch at a
    /// time ([`Engine::describe`]), so a read of the model it's published in asks the kernel
    /// nothing. A summary refused or failed is the measure's failure; a description refused
    /// or failed is kept beside the summary, which stands: only element reads report it.
    fn measure(&self, built: &Built, generation: Generation) -> Outcome<Built> {
        let (built, summary) = match built.summary() {
            Some(summary) => (built.clone(), summary.clone()),
            None => match self
                .kernel
                .submit(generation, KernelRequest::summarize(&built.shape))
                .wait()
            {
                Outcome::Done(Ok(summary)) => (built.summarized(summary.clone()), summary),
                Outcome::Done(Err(error)) => return Outcome::Done(Err(error)),
                Outcome::Superseded => return Outcome::Superseded,
            },
        };
        let described = match self.describe(&built.shape, &summary, generation) {
            Outcome::Superseded => return Outcome::Superseded,
            Outcome::Done(Ok(description)) => Described::Whole(description),
            // A hang is kept, but may be a busy machine's: the read that meets it is told
            // so, and to check with fresh to ask again.
            Outcome::Done(Err(error)) if hung(&error) => {
                Described::Refused(error.with_advice(crate::worker::Advice::TryAgain {
                    once_more: false,
                    why: HUNG_KEPT.to_owned(),
                }))
            }
            Outcome::Done(Err(error)) if kept(&error) => Described::Refused(error),
            Outcome::Done(Err(error)) => Described::Failed(error),
        };
        Outcome::Done(Ok(built.with_described(described)))
    }

    /// Each of `shape`'s faces, edges, and vertices described, for `generation`: in one
    /// request if `summary` counts at most [`DESCRIBE_BATCH`](crate::kernel::DESCRIBE_BATCH)
    /// of them in all ([`KernelRequest::describe_all`]), else in batches of at most that many
    /// planned from its counts, [`DESCRIBE_AT_ONCE`] of them in flight at a time (each carries
    /// the shape), joined in order. Each batch's total is checked against the summary's count, and the
    /// whole against the summary, before it's taken. A batch that fails gives the
    /// description up: the batches still in flight are dropped, which cancels them
    /// ([`Pending`]), so no worker is spent on what won't be used.
    fn describe(
        &self,
        shape: &ShapeData,
        summary: &ShapeSummary,
        generation: Generation,
    ) -> Outcome<Description> {
        let all = u64::from(summary.faces) + u64::from(summary.edges) + u64::from(summary.vertices);
        if all <= u64::from(DESCRIBE_BATCH) {
            let description = match self
                .kernel
                .submit(generation, KernelRequest::describe_all(shape))
                .wait()
            {
                Outcome::Done(Ok(description)) => description,
                Outcome::Done(Err(error)) => return Outcome::Done(Err(error)),
                Outcome::Superseded => return Outcome::Superseded,
            };
            // As many of each kind as the summary counts.
            return match crate::kernel::description_fault(summary, &description) {
                None => Outcome::Done(Ok(description)),
                Some(fault) => Outcome::Done(Err(crate::kernel::description_error(fault))),
            };
        }
        let mut planned = silico_protocol::batches(summary.faces, summary.edges, summary.vertices);
        let mut description = Description {
            faces: Vec::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
        };
        let large = [summary.faces, summary.edges, summary.vertices]
            .iter()
            .any(|&n| n > DESCRIBE_BATCH);
        let at_once = if large { DESCRIBE_AT_ONCE } else { 1 };
        // Given up at a failure by returning: the batches still in flight are dropped with it,
        // which cancels them ([`Pending`]: one still waiting is dropped unrun).
        let mut in_flight: VecDeque<(ElementKind, Pending<DescribedBatch>)> = VecDeque::new();
        loop {
            while in_flight.len() < at_once
                && let Some((kind, first, count)) = planned.next()
            {
                let request = match KernelRequest::describe(shape, kind, first, count) {
                    Ok(request) => request,
                    Err(error) => return Outcome::Done(Err(error)),
                };
                in_flight.push_back((kind, self.kernel.submit(generation, request)));
            }
            let Some((kind, pending)) = in_flight.pop_front() else {
                break;
            };
            match pending.wait() {
                Outcome::Done(Ok(batch)) => {
                    if let Some(fault) = crate::kernel::total_fault(summary, kind, batch.total) {
                        return Outcome::Done(Err(fault));
                    }
                    description.faces.extend(batch.description.faces);
                    description.edges.extend(batch.description.edges);
                    description.vertices.extend(batch.description.vertices);
                }
                Outcome::Done(Err(error)) => return Outcome::Done(Err(error)),
                // The generation's other requests are superseded with it.
                Outcome::Superseded => return Outcome::Superseded,
            }
        }
        match crate::kernel::description_fault(summary, &description) {
            None => Outcome::Done(Ok(description)),
            Some(fault) => Outcome::Done(Err(crate::kernel::description_error(fault))),
        }
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
            // Run by `copied`, once its references are resolved.
            Step::Copies { .. } => Outcome::Done(Err(silico_bug(
                "a pattern's step ran without its references resolved",
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

/// Releases step `key`'s users waiting on it, now that it's built: each made ready once
/// nothing else holds it. How many were.
fn release(s: &mut Schedule, key: Key) -> usize {
    let mut woken = 0;
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
    woken
}

fn map<T, U>(outcome: Outcome<T>, f: impl FnOnce(T) -> U) -> Outcome<U> {
    match outcome {
        Outcome::Done(result) => Outcome::Done(result.map(f)),
        Outcome::Superseded => Outcome::Superseded,
    }
}

/// What a round finds of the faces of the sketches its plan has waiting
/// ([`placed_faces`]): those it places (a frame, or why there's none), and those that still
/// wait, each with the feature whose solid it waits for, by sketch.
#[derive(Default)]
struct Faces {
    placed: BTreeMap<FeatureId, Placed>,
    waiting: BTreeMap<FeatureId, FeatureId>,
}

/// What the round's `model` finds of the face of each sketch on a face that `plan` has
/// waiting (docs/ports/attacher.md, *Where the face is looked for*), each looked for in the
/// solids live at its place ([`faces_at`]) and placed there (`face_plane::place`). A sketch
/// placed on a frame whose profile (in `swept`) would reach past a kilometre there is
/// refused ([`beyond_reach`](crate::face_plane::beyond_reach)).
fn placed_faces(
    document: &Checked,
    plan: &Plan,
    model: &Model,
    swept: &BTreeMap<FeatureId, Swept>,
) -> Faces {
    use crate::face_plane::Resolved;
    let mut faces = Faces::default();
    let waiting = plan
        .features
        .iter()
        .enumerate()
        .filter(|(_, planned)| matches!(planned.on_face, OnFace::Waits))
        .map(|(at, _)| at);
    let maker = |f: &FeatureId| plan.maker(f);
    let found = faces_at(
        document,
        model,
        &plan.waits,
        waiting,
        &maker,
        |sketch, face, related, others| {
            crate::face_plane::place(sketch, face, related, others, &maker)
        },
    );
    for (sketch, found) in found {
        match found {
            AtPlace::Waits(on) | AtPlace::Found(Resolved::Waits(on)) => {
                faces.waiting.insert(sketch, on);
            }
            AtPlace::Unnamed(solid) => {
                faces.placed.insert(
                    sketch.clone(),
                    Placed::Failed(
                        silico_bug(&format!(
                            "{solid}'s solid built, but its elements couldn't be named"
                        ))
                        .with_feature(sketch.as_str()),
                    ),
                );
            }
            AtPlace::Found(Resolved::Placed(Placed::Frame(frame))) => {
                let placed = match placeable(document, &sketch, frame, swept.get(&sketch)) {
                    Some(error) => Placed::Failed(error),
                    None => Placed::Frame(frame),
                };
                faces.placed.insert(sketch, placed);
            }
            AtPlace::Found(Resolved::Placed(placed)) => {
                faces.placed.insert(sketch, placed);
            }
        }
    }
    faces
}

/// What [`faces_at`] finds at a sketch's place.
enum AtPlace<R> {
    /// What was made of the face, from the solids live there.
    Found(R),
    /// A link before the sketch of a chain one of its name's makers begins waits, not built
    /// yet: that link's feature.
    Waits(FeatureId),
    /// A solid that could hold the face built, but its elements couldn't be named (a bug's):
    /// its feature.
    Unnamed(FeatureId),
}

/// The default `x` of a sketch on a face, `plane.face` of the sketch at `at` in `document`,
/// as `add_feature` names it when the request leaves it out (M4.1 section 5; F1): its face
/// looked for in `model`, as a rebuild looks for it ([`faces_at`]), and the face's
/// `default_x` taken, or why it can't be (`face_plane::default_x`). `model` is the model of
/// the document the sketch is added to: every feature before its place is that document's,
/// unchanged, so the solids live there are as the model has them. None for a feature at
/// `at` that isn't a sketch on a face.
pub(crate) fn default_x(document: &Checked, model: &Model, at: usize) -> Option<Defaulted> {
    let maker = |f: &FeatureId| {
        document
            .feature(f)
            .ok()
            .map(|feature| Maker::of(&feature.operation))
    };
    let found = faces_at(
        document,
        model,
        &BTreeSet::new(),
        [at],
        &maker,
        |sketch, face, related, others| {
            crate::face_plane::default_x(sketch, &face.face, related, others, &maker)
        },
    );
    let (sketch, found) = found.into_iter().next()?;
    Some(match found {
        AtPlace::Found(defaulted) => defaulted,
        AtPlace::Waits(on) => Defaulted::Waits(on),
        AtPlace::Unnamed(solid) => Defaulted::Refused(Placed::Failed(
            silico_bug(&format!(
                "{solid}'s solid built, but its elements couldn't be named"
            ))
            .with_feature(sketch.as_str()),
        )),
    })
}

/// What `resolve` makes of the face of each sketch on a face at the places `sketches` (in
/// timeline order) of `document`, each looked for in the solids live at its place in the
/// timeline (docs/ports/attacher.md, *Where the face is looked for*), as `model` has them:
/// those made by features before it, unsuppressed, and consumed by none before it (a
/// suppressed feature's base passed through, `document::suppression::consumed`): M4.1
/// section 5's rule for `add_feature`. Only those built from one of its name's makers can
/// hold it, so only they're `resolve`'s `related` solids, waited for, or block it; the
/// rest, `others`, are read for a broken name's candidates. A feature in `waits` (by place)
/// isn't built yet, so its solid waits. By sketch, in order; a place that isn't a sketch on
/// a face is passed over.
///
/// Linear in the timeline each round, however many sketches wait: each solid has at most
/// one consumer (the gate's consumed-solid rule), so what's made from a maker is one chain
/// of consumers, and the solid of it live at a sketch's place is the chain's last link
/// before the sketch. Each maker's chain is walked once a round, up to its first link that
/// waits (every link after it waits too: each uses the one before), and each feature's
/// inputs are read once a round; the solids live at each sketch's place are kept as the
/// timeline is walked, for a broken name's candidates.
fn faces_at<'m, R>(
    document: &Checked,
    model: &'m Model,
    waits: &BTreeSet<usize>,
    sketches: impl IntoIterator<Item = usize>,
    maker: &dyn Fn(&FeatureId) -> Option<Maker>,
    mut resolve: impl FnMut(
        &FeatureId,
        &crate::document::FacePlane,
        &[Live<'m>],
        &dyn Fn() -> Vec<Live<'m>>,
    ) -> R,
) -> Vec<(FeatureId, AtPlace<R>)> {
    let features = &document.document().features;
    let mut found = Vec::new();
    let mut sketches = sketches.into_iter().peekable();
    if sketches.peek().is_none() {
        return found;
    }
    // Once a round: what stands for each feature's solid, and each solid's consumer (by
    // place, through what stands for each input), with the solids each feature consumes:
    // so a solid is live at a sketch's place exactly when none consumes it before the
    // sketch. Each a function of earlier features alone, so the same for every sketch.
    let place = |id: &FeatureId| document.place(id);
    let standing = crate::document::suppression::standing(features, &place);
    let mut consumer: Vec<Option<usize>> = vec![None; features.len()];
    let mut consumes: Vec<Vec<usize>> = vec![Vec::new(); features.len()];
    for (c, feature) in features.iter().enumerate() {
        face_walked(1);
        if feature.suppressed {
            continue;
        }
        let inputs = crate::document::suppression::effective_inputs(features, &standing, &place, c);
        for (_, stands) in inputs {
            if let Some(s) = stands
                && let Some(slot) = consumer.get_mut(s)
                && slot.is_none()
            {
                *slot = Some(c);
                if let Some(consumed) = consumes.get_mut(c) {
                    consumed.push(s);
                }
            }
        }
    }
    // The feature `j`'s solid as the round has it.
    let live = |j: usize, feature: &crate::document::Feature| -> Option<Result<Live<'m>, ()>> {
        if waits.contains(&j) {
            return Some(Ok(Live::Waits(feature.id.clone())));
        }
        match model.outcome(&feature.id)? {
            FeatureOutcome::Built { built, .. } => {
                // A built feature's map is named from its history: none is a bug's.
                let Some(map) = model.element_map(&feature.id) else {
                    return Some(Err(()));
                };
                let description = match built.description() {
                    Some(description) => Ok(description),
                    None => Err(built
                        .why_undescribed()
                        .or_else(|| model.unmeasured(&feature.id))
                        .cloned()
                        .unwrap_or_else(|| {
                            silico_bug("a live solid built, but it wasn't described")
                                .with_feature(feature.id.as_str())
                        })),
                };
                Some(Ok(Live::Built { map, description }))
            }
            FeatureOutcome::Failed(_) | FeatureOutcome::Broken { .. } => {
                Some(Ok(Live::Failed(feature.id.clone())))
            }
            FeatureOutcome::Blocked { on } => Some(Ok(Live::Failed(on.clone()))),
            // Not live: a sketch makes no solid, and a suppressed feature is left out.
            FeatureOutcome::Sketch(_) | FeatureOutcome::Suppressed => None,
        }
    };
    // Each maker's chain of consumers, by place, walked once a round on first need: its
    // links in order, and whether its last waits (the walk stops there: so does the rest).
    let mut chains: BTreeMap<usize, (Vec<usize>, bool)> = BTreeMap::new();
    let walk = |maker: usize| -> (Vec<usize>, bool) {
        let mut links = vec![maker];
        let mut at = maker;
        loop {
            face_walked(1);
            if waits.contains(&at) {
                return (links, true);
            }
            match consumer.get(at).copied().flatten() {
                Some(next) => {
                    links.push(next);
                    at = next;
                }
                None => return (links, false),
            }
        }
    };
    // The solids live at the place reached so far, by place: those made before it, not
    // suppressed, and consumed by none before it.
    let mut alive: BTreeSet<usize> = BTreeSet::new();
    let mut reached = 0;
    for at in sketches {
        let Some(sketch) = features.get(at) else {
            continue;
        };
        let face = match &sketch.operation {
            Operation::Sketch(drawn) => drawn.plane.face(),
            _ => None,
        };
        let Some(face) = face else {
            continue;
        };
        face_walked(1);
        for (j, feature) in features.iter().enumerate().take(at).skip(reached) {
            face_walked(1);
            for consumed in consumes.get(j).into_iter().flatten() {
                alive.remove(consumed);
            }
            if !feature.suppressed && feature.operation.makes_solid() {
                alive.insert(j);
            }
        }
        reached = reached.max(at);
        // The features before it its name's makers, by place; and the solid of each one's
        // chain live here, its last link before the sketch: unless a link before the
        // sketch waits, when it waits for that link's.
        let makers: BTreeSet<usize> = crate::naming::Name::parse(face.face.as_str(), maker)
            .map(|name| {
                name.features()
                    .into_iter()
                    .filter_map(|id| document.place(id).filter(|&p| p < at))
                    .collect()
            })
            .unwrap_or_default();
        let mut tails: BTreeSet<usize> = BTreeSet::new();
        let mut waits_on = None;
        for maker in makers {
            let (links, last_waits) = chains.entry(maker).or_insert_with(|| walk(maker));
            let before = links.partition_point(|&link| link < at);
            let Some(&tail) = before.checked_sub(1).and_then(|i| links.get(i)) else {
                continue;
            };
            if *last_waits && before == links.len() {
                // The chain's last link walked, which waits, is before the sketch.
                waits_on = waits_on.or_else(|| features.get(tail).map(|f| f.id.clone()));
                continue;
            }
            if alive.contains(&tail) {
                tails.insert(tail);
            }
        }
        if let Some(on) = waits_on {
            found.push((sketch.id.clone(), AtPlace::Waits(on)));
            continue;
        }
        let mut related = Vec::new();
        let mut unnamed = None;
        for &j in &tails {
            let Some(feature) = features.get(j) else {
                continue;
            };
            match live(j, feature) {
                Some(Ok(solid)) => related.push(solid),
                Some(Err(())) => {
                    unnamed = Some(feature.id.clone());
                    break;
                }
                None => {}
            }
        }
        if let Some(solid) = unnamed {
            found.push((sketch.id.clone(), AtPlace::Unnamed(solid)));
            continue;
        }
        // The rest of the live solids, read only for a broken name's candidates (a solid
        // whose map couldn't be named offers none).
        let others = || {
            alive
                .iter()
                .filter(|j| !tails.contains(j))
                .filter_map(|&j| live(j, features.get(j)?)?.ok())
                .collect()
        };
        let made = resolve(&sketch.id, face, &related, &others);
        found.push((sketch.id.clone(), AtPlace::Found(made)));
    }
    found
}

#[cfg(test)]
thread_local! {
    /// What [`face_walks`] says.
    static FACE_WALKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many steps [`placed_faces`] has taken on this thread so far: one for each feature
/// whose inputs it read, each link of a maker's chain walked, each sketch whose face it
/// looked for, and each feature passed to keep the solids live at a sketch's place.
#[cfg(test)]
pub(crate) fn face_walks() -> u64 {
    FACE_WALKS.with(std::cell::Cell::get)
}

/// `steps` more of [`placed_faces`]' walk, for [`face_walks`].
fn face_walked(steps: u64) {
    #[cfg(test)]
    FACE_WALKS.with(|walks| walks.set(walks.get() + steps));
    #[cfg(not(test))]
    let _ = steps;
}

/// Why the sketch `sketch` of `document`, its face's frame resolved as `frame`, can't be
/// placed there, if it can't (docs/ports/attacher.md, *The frame*): its profile (`swept`'s,
/// if a feature sweeps it), or the axis a revolve or groove of it turns about, placed past
/// a kilometre on any axis of the model ([`beyond_reach`](crate::face_plane::beyond_reach)):
/// `invalid_input` naming the sketch and `plane`, in the document's terms, so no later
/// request is refused naming a value the document never wrote. (A frame no sketch can be
/// placed on at all is refused as it's made: `face_plane::place`.) An axis that's no axis
/// of the profile is its feature's own refusal, left to its plan.
fn placeable(
    document: &Checked,
    sketch: &FeatureId,
    frame: silico_protocol::FaceFrame,
    swept: Option<&Swept>,
) -> Option<Error> {
    let swept = swept?;
    let profile = swept.profile.as_ref().ok()?;
    let plane = silico_protocol::SketchPlane::Face(frame);
    if let Some(error) = crate::face_plane::beyond_reach(sketch, &profile.faces(plane)) {
        return Some(error);
    }
    document.document().features.iter().find_map(|feature| {
        let turns = matches!(
            feature.operation.made(),
            Made::Sweep(SweepFeature::Revolve | SweepFeature::Groove)
        );
        if feature.suppressed || !turns || feature.operation.sketch() != Some(sketch) {
            return None;
        }
        let line = crate::sweep::check_axis(document, &feature.id, &swept.solution, profile).ok()?;
        let at = plane.place(line.origin);
        let within = at
            .iter()
            .all(|c| c.is_finite() && c.abs() <= silico_protocol::MAX_LENGTH);
        let named = feature.operation.axis().map_or("axis", |a| a.as_str());
        (!within).then(|| {
            Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "{}'s axis {named}, placed on the sketch's face's frame, passes through {at:?}: past 1e6 mm (a kilometre) of the model's origin, as far as an axis may be",
                    feature.id
                ),
            )
            .with_feature(sketch.as_str())
            .with_parameter("plane")
            .with_element(named)
            .with_suggestion(format!(
                "draw {}'s axis nearer the face's origin, or put the sketch on a face nearer the model's origin",
                feature.id
            ))
        })
    })
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
    let mut diagnoses = BTreeMap::new();
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
                    // A feature uses a suppressed one only through what stands for it.
                    Some((
                        _,
                        FeatureOutcome::Built { .. }
                        | FeatureOutcome::Sketch(_)
                        | FeatureOutcome::Suppressed,
                    ))
                    | None => None,
                });
        let outcome = match (blocked, planned.sketch) {
            _ if planned.suppressed => FeatureOutcome::Suppressed,
            (Some(on), _) => FeatureOutcome::Blocked { on },
            (None, Some(key)) => {
                let solved = solutions.get(&key);
                if let Some(solved) = solved {
                    diagnoses.insert(planned.id.clone(), solved.diagnosed.clone());
                }
                // A sketch on a face is placed before it's solved, as FreeCAD attaches it
                // before its solve: a face that couldn't be resolved is its outcome.
                match (&planned.on_face, solved) {
                    (OnFace::Placed(Placed::Broken(error)), _) => FeatureOutcome::Broken {
                        errors: vec![error.clone()],
                        total: 1,
                    },
                    (OnFace::Placed(Placed::Blocked(on)), _) => {
                        FeatureOutcome::Blocked { on: on.clone() }
                    }
                    (OnFace::Placed(Placed::Failed(error)), _) => {
                        FeatureOutcome::Failed(error.clone())
                    }
                    // Every round resolves the first sketch on a face waiting (all it
                    // could be on is before it, and built): one left waiting is a bug.
                    (OnFace::Waits, _) => FeatureOutcome::Failed(
                        silico_bug("a sketch's face was never resolved")
                            .with_feature(planned.id.as_str()),
                    ),
                    (OnFace::No | OnFace::Placed(Placed::Frame(_)), Some(solved)) => {
                        match &solved.result {
                            Ok(solution) => FeatureOutcome::Sketch(Arc::clone(solution)),
                            Err(error) => FeatureOutcome::Failed(error.clone()),
                        }
                    }
                    (OnFace::No | OnFace::Placed(Placed::Frame(_)), None) => {
                        FeatureOutcome::Failed(
                            silico_bug("a sketch wasn't solved").with_feature(planned.id.as_str()),
                        )
                    }
                }
            }
            (None, None) => match &planned.refused {
                Some(error) => FeatureOutcome::Failed(error.clone()),
                None => checked(
                    planned,
                    feature_outcome(planned, done, &schedule.unresolved),
                    &features,
                    done,
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
            | FeatureOutcome::Sketch(_)
            | FeatureOutcome::Suppressed => None,
        })
        .collect::<Vec<_>>();
    let naming = naming(plan, &features, done);
    let maps = (0..features.len()).map(|_| OnceLock::new()).collect();
    let entries = (0..features.len()).map(|_| OnceLock::new()).collect();
    let unmeasured_index = Arc::new(FeatureIndex::new(unmeasured.iter().map(|(id, _)| id)));
    let frames = plan
        .features
        .iter()
        .filter_map(|planned| match &planned.on_face {
            OnFace::Placed(Placed::Frame(frame)) => Some((planned.id.clone(), *frame)),
            OnFace::No | OnFace::Waits | OnFace::Placed(_) => None,
        })
        .collect();
    // Every verdict kept (section 4.2): each pattern's copies, step by step, as far as its
    // steps built, whatever its own outcome.
    let patterns = plan
        .features
        .iter()
        .filter_map(|planned| {
            let Some(Check::Pattern(check)) = &planned.check else {
                return None;
            };
            let verdicts: Vec<StepVerdict> = check
                .steps
                .iter()
                .zip(&planned.steps)
                .map_while(|((original, op, instances), key)| match done.get(key) {
                    Some(Ok(built)) => built.history.as_ref().map(|history| {
                        crate::pattern::verdict(
                            history,
                            instances,
                            original.clone(),
                            *op == CopiesOp::Fuse,
                        )
                    }),
                    _ => None,
                })
                .collect();
            Some((planned.id.clone(), verdicts.into()))
        })
        .collect();
    Model {
        source: Arc::clone(source),
        generation,
        features,
        bodies,
        unmeasured,
        unmeasured_index,
        naming,
        maps,
        entries,
        live: Arc::default(),
        known,
        diagnoses: Arc::new(diagnoses),
        frames: Arc::new(frames),
        patterns: Arc::new(patterns),
    }
}

/// `feature`, a sketch of `document`, solved by `solver` as [`crate::sketch::solve`] solves
/// it, its diagnosis kept with its result: the answer's, read from the same reply; or, for a
/// sketch the translation refuses, whose constraints never reach the solver, the diagnosis
/// of its geometry alone beside that refusal ([`Diagnosed::untranslated`]), one solve of no
/// constraint. A solver that fails that diagnosis leaves the sketch's result the
/// translation's refusal, and its diagnosis the solver's failure.
fn solve_diagnosed(
    document: &Checked,
    feature: &FeatureId,
    solver: &mut dyn SketchSolver,
) -> SketchResult {
    let named = |e: Error| e.with_feature(feature.as_str());
    let sketch = match document.feature(feature) {
        Ok(Feature {
            operation: Operation::Sketch(sketch),
            ..
        }) => sketch,
        Ok(_) => {
            return SketchResult::failed(named(silico_bug(
                "a feature that isn't a sketch was solved as one",
            )));
        }
        Err(error) => return SketchResult::failed(error),
    };
    let _span = tracing::info_span!("solve sketch", feature = feature.as_str()).entered();
    match Translation::new(sketch) {
        Ok(translation) => match translation.solve(solver) {
            Ok(answer) => match answer.diagnosed() {
                Ok(diagnosed) => SketchResult {
                    result: diagnosed
                        .outcome()
                        .map(|solution| Arc::new(solution.clone()))
                        .map_err(|e| named(e.clone())),
                    diagnosed: Ok(Arc::new(diagnosed.clone())),
                },
                Err(error) => SketchResult::failed(named(error.clone())),
            },
            Err(error) => SketchResult::failed(named(error)),
        },
        Err(refusal) => SketchResult {
            result: Err(named(refusal)),
            diagnosed: Diagnosed::untranslated(sketch, solver)
                .and_then(|diagnosed| {
                    diagnosed.ok_or_else(|| {
                        silico_bug("a sketch the translation refused was translated")
                    })
                })
                .map(Arc::new)
                .map_err(named),
        },
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
    for (planned, (_, outcome)) in plan.features.iter().zip(features) {
        let (FeatureOutcome::Built { .. }, Some(how)) = (outcome, planned.naming.as_ref()) else {
            naming.push(None);
            continue;
        };
        // Its own steps' results: a built feature's are all built.
        let steps: Option<Vec<Arc<Built>>> = planned
            .naming_steps()
            .map(|step| match done.get(step) {
                Some(Ok(built)) => Some(Arc::clone(built)),
                _ => None,
            })
            .collect();
        // (A feature whose inputs aren't named isn't either: naming it needs their maps.)
        // `features` are the plan's, in its order, and a built one's key is its last step's
        // (`feature_outcome`).
        naming.push(
            planned
                .naming_key
                .zip(steps)
                .map(|(key, steps)| FeatureNaming {
                    key,
                    how: how.clone(),
                    uses: planned.uses.clone(),
                    steps,
                }),
        );
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
            let swept = swept_map(id, sweep, history(0)?, history(1)?, known);
            match (inputs, steps.len()) {
                ([], 2) => swept,
                ([base], 3) => {
                    ElementMap::operation(id.clone(), sweep.operation, &[base, &swept], history(2)?)
                }
                _ => return None,
            }
        }
        Naming::Pattern(pattern) => {
            let base = inputs.first()?;
            // The identity passes its base's map through.
            if pattern.steps.is_empty() {
                return Some((*base).clone());
            }
            // Its own steps, then its originals' faces and sweeps, two an original.
            let (own, tools) = steps.split_at_checked(pattern.steps.len())?;
            let mut on: Option<ElementMap> = None;
            let mut tool = 0;
            for (step, copies) in own.iter().zip(&pattern.steps) {
                let copied = match (&copies.original, copies.copied) {
                    (Some((original, sweep)), Copied::Tool) => {
                        let faces = tools.get(tool)?.history.as_ref()?;
                        let swept = tools.get(tool + 1)?.history.as_ref()?;
                        tool += 2;
                        swept_map(original, sweep, faces, swept, known)
                    }
                    (None, Copied::Solid) => (*base).clone(),
                    _ => return None,
                };
                let placed: Option<Vec<ElementMap>> = copies
                    .instances
                    .iter()
                    .map(|instance| {
                        ElementMap::copy(
                            &copied,
                            id.clone(),
                            pattern.operation,
                            copies.copied,
                            instance,
                        )
                    })
                    .collect();
                let placed = placed?;
                let before = on.as_ref().unwrap_or(base);
                let maps: Vec<&ElementMap> = std::iter::once(before).chain(placed.iter()).collect();
                on = Some(ElementMap::operation(
                    id.clone(),
                    pattern.operation,
                    &maps,
                    step.history.as_ref()?,
                ));
            }
            on?
        }
    })
}

/// The map of feature `id`'s sweep of its sketch's profile's faces (S3's second pass): the
/// faces' map, kept in `known` by its own key, so features sharing a sketch name it once,
/// from `faces`, the faces step's history, then the sweep's from it by `swept`, the sweep's.
/// A sweep feature's names pass through it, and a pattern's copies of the feature's tool
/// are named from it (N10).
fn swept_map(
    id: &FeatureId,
    sweep: &SweepNaming,
    faces: &History,
    swept: &History,
    known: &NamedMaps,
) -> ElementMap {
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
                faces,
            ),
        ),
    };
    ElementMap::swept(id.clone(), sweep.operation, &faces, swept)
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
        Naming::Pattern(pattern) => pattern.operation as u8 + 1,
    }]);
    // A sweep feature's names come from its sketch's IDs too, which its steps' keys leave
    // out (design S3): two profiles alike but for an ID make one faces step and two maps.
    if let Naming::Sweep(sweep) = how {
        hash.update(sweep.faces.0);
    }
    // A pattern's names come from which input each step copies, as a tool's (with its
    // sweep's names) or the solid's, and the instances it places (section 2.4).
    if let Naming::Pattern(pattern) = how {
        hash.update((pattern.steps.len() as u64).to_be_bytes());
        for step in &pattern.steps {
            hash.update([step.copied.letter() as u8]);
            if let Some((original, sweep)) = &step.original {
                hash.update((original.as_str().len() as u64).to_be_bytes());
                hash.update(original.as_str().as_bytes());
                hash.update(sweep.faces.0);
            }
            hash.update((step.instances.len() as u64).to_be_bytes());
            for instance in step.instances.iter() {
                hash.update((instance.offsets().len() as u64).to_be_bytes());
                for offset in instance.offsets() {
                    hash.update(offset.to_be_bytes());
                }
            }
        }
    }
    for input in inputs {
        hash.update(input.0);
    }
    NamingKey(hash.finalize().into())
}

/// `outcome`, `planned`'s, as its result's check decides it (`Check`): a built feature
/// whose solid fails it is failed, in its own words, so what uses it is blocked on it. Read
/// from the solid's stored summary and history and its base's (`features`, decided before
/// it), the same whether the solid was made, kept, or read from the cache. A sweep feature's
/// solid that couldn't be measured can't be checked, and fails with why (`unmeasured`); an
/// M2 boolean's is built unchecked for its change, and says why, as before S7's rule reached
/// M2 booleans. A solid with no measure and no word why is Silico's fault, and fails as one
/// either way.
fn checked(
    planned: &Planned,
    outcome: FeatureOutcome,
    features: &[(FeatureId, FeatureOutcome)],
    done: &BTreeMap<Key, StepResult>,
    unmeasured: &BTreeMap<Key, Error>,
) -> FeatureOutcome {
    let (FeatureOutcome::Built { key, built }, Some(check)) = (&outcome, &planned.check) else {
        return outcome;
    };
    if let Check::Pattern(pattern) = check {
        return match pattern_checked(pattern, planned, features, done, unmeasured) {
            Ok(()) => outcome,
            Err(error) => FeatureOutcome::Failed(error),
        };
    }
    // Its base's measures and its own, or why they couldn't be had: a sweep's and a
    // boolean's base is its first input.
    let measures = || -> Result<_, Unmeasured> {
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
            (Some(before), Some(after)) => Ok((before, after)),
            _ => Err(
                match unmeasured
                    .get(key)
                    .or_else(|| base_key.and_then(|k| unmeasured.get(&k)))
                {
                    Some(why) => Unmeasured::Said(why.clone().with_feature(planned.id.as_str())),
                    None => Unmeasured::Bug(
                        silico_bug("a feature's solid or its base's wasn't measured")
                            .with_feature(planned.id.as_str()),
                    ),
                },
            ),
        }
    };
    let decided = match check {
        Check::Boolean { op, base, tool } => {
            let solids = built.history.as_ref().map_or(1, |h| h.result.solids);
            // Its change judged where both solids were measured: an M2 boolean whose solid or
            // base couldn't be, and that says why (`unmeasured`), is built, as before S7's
            // rule reached it; a measure missing for no reason is Silico's fault.
            crate::sweep::check_boolean_solids(&planned.id, *op, base, tool, solids).and_then(
                |()| match measures() {
                    Ok((before, after)) => {
                        crate::sweep::check_boolean_change(&planned.id, *op, base, &before, &after)
                    }
                    Err(Unmeasured::Said(_)) => Ok(()),
                    Err(Unmeasured::Bug(error)) => Err(error),
                },
            )
        }
        Check::Sweep(sweep) => measures()
            .map_err(Unmeasured::error)
            .and_then(|(before, after)| {
                crate::sweep::check_result(&sweep.feature, sweep.kind, &sweep.base, &before, &after)
            }),
        Check::Pattern(_) => Ok(()),
    };
    match decided {
        Ok(()) => outcome,
        Err(error) => FeatureOutcome::Failed(error),
    }
}

/// Whether a pattern's steps pass its check ([`PatternCheck`]), `built` its base's solid
/// then each step's, in order: each step's result one solid, then changed from the one
/// before by S7's rule (section 4.2). None while a measure it needs is still to come.
fn pattern_holds(check: &PatternCheck, built: &[&Built]) -> Option<bool> {
    let mut before = measured_of(built.first()?)?;
    for ((_, op, _), step) in check.steps.iter().zip(built.iter().skip(1)) {
        if step.history.as_ref().map_or(1, |h| h.result.solids) > 1 {
            return Some(false);
        }
        let after = measured_of(step)?;
        if !crate::sweep::pattern_change_holds(check.kind, *op, &before, &after) {
            return Some(false);
        }
        before = after;
    }
    Some(true)
}

/// A pattern's check (section 4.2), from its base's solid and its steps' in `done`: the
/// first step's refusal, in its words, or why a measure it needs couldn't be had.
fn pattern_checked(
    check: &PatternCheck,
    planned: &Planned,
    features: &[(FeatureId, FeatureOutcome)],
    done: &BTreeMap<Key, StepResult>,
    unmeasured: &BTreeMap<Key, Error>,
) -> Result<(), Error> {
    let id = &check.feature;
    let unbuilt =
        || silico_bug("a pattern's solid or its base's wasn't measured").with_feature(id.as_str());
    let base = planned
        .uses
        .first()
        .and_then(|&b| features.get(b))
        .and_then(|(_, outcome)| match outcome {
            FeatureOutcome::Built { key, .. } => Some(*key),
            _ => None,
        })
        .ok_or_else(unbuilt)?;
    let measured = |key: &Key| -> Result<(Arc<Built>, crate::sweep::Measured), Error> {
        let Some(Ok(built)) = done.get(key) else {
            return Err(unbuilt());
        };
        match measured_of(built) {
            Some(measured) => Ok((Arc::clone(built), measured)),
            None => Err(match unmeasured.get(key) {
                Some(why) => why.clone().with_feature(id.as_str()),
                None => unbuilt(),
            }),
        }
    };
    let (_, mut before) = measured(&base)?;
    let several = check.steps.len() > 1;
    for ((original, op, instances), key) in check.steps.iter().zip(&planned.steps) {
        let Some(Ok(built)) = done.get(key) else {
            return Err(unbuilt());
        };
        if let Some(history) = &built.history {
            let verdict = crate::pattern::verdict(
                history,
                instances,
                original.clone(),
                *op == CopiesOp::Fuse,
            );
            let bounds = built.summary().and_then(|summary| summary.bounds);
            if let Some(error) = crate::pattern::solids_refused(
                id,
                check.kind.word(),
                &check.base,
                &verdict,
                several,
                bounds,
            ) {
                return Err(error);
            }
        }
        let (_, after) = measured(key)?;
        crate::sweep::check_pattern_change(id, check.kind, *op, &check.base, &before, &after)?;
        before = after;
    }
    Ok(())
}

/// Why a checked feature's measures, or its base's, couldn't be had ([`checked`]).
enum Unmeasured {
    /// The measure failed, or was refused, and said so (`unmeasured`).
    Said(Error),
    /// No measure and no word why: Silico's fault.
    Bug(Error),
}

impl Unmeasured {
    fn error(self) -> Error {
        match self {
            Unmeasured::Said(error) | Unmeasured::Bug(error) => error,
        }
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
impl Model {
    /// This model with feature `id`'s outcome replaced by `outcome`: a test's stand-in for
    /// outcomes no document reaches on the fake (Silico's own bug naming a base's elements).
    /// Its slots its own ([`Model::unnamed`]): its outcomes aren't this model's.
    pub(crate) fn with_outcome(&self, id: &FeatureId, outcome: FeatureOutcome) -> Model {
        let mut model = self.unnamed();
        for (feature, held) in &mut model.features {
            if feature == id {
                *held = outcome.clone();
            }
        }
        model
    }

    /// This model with feature `id`'s solid measured as `summary` says, if it built: a
    /// test's stand-in for measures past what a fingerprint holds, which no solid the gate
    /// admits has (a kilometre each way is 1e18 mm³, and a volume holds to 1.7e32).
    /// Its slots its own ([`Model::unnamed`]): its outcomes aren't this model's.
    pub(crate) fn measured_as(&self, id: &FeatureId, summary: ShapeSummary) -> Model {
        let mut model = self.unnamed();
        for (feature, outcome) in &mut model.features {
            if feature == id
                && let FeatureOutcome::Built { built, .. } = outcome
            {
                *built = Arc::new(Built::new(
                    built.shape.clone(),
                    built.history.clone(),
                    Some(summary.clone()),
                ));
            }
        }
        model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_reads_back_from_the_64_lowercase_hex_digits_it_writes_and_from_nothing_else() {
        // By hand: 0x00, 0x01, ..., 0x1f, written two digits a byte.
        let mut bytes = [0u8; 32];
        for (at, byte) in bytes.iter_mut().enumerate() {
            *byte = at as u8;
        }
        let text: String = (0..32u8).map(|b| format!("{b:02x}")).collect();
        assert_eq!(text.len(), 64);
        assert_eq!(Key::parse(&text), Some(Key(bytes)));
        assert_eq!(Key(bytes).to_string(), text);
        assert_eq!(Key::parse(&"f".repeat(64)), Some(Key([0xff; 32])));
        assert_eq!(Key::parse(&"a9".repeat(32)), Some(Key([0xa9; 32])));
        for bad in [
            String::new(),
            "0".repeat(63),
            "0".repeat(65),
            "0".repeat(128),
            format!("A{}", "0".repeat(63)),
            format!("{}F", "0".repeat(63)),
            format!("g{}", "0".repeat(63)),
            format!("{} ", "0".repeat(63)),
            format!("-{}", "0".repeat(63)),
            format!("é{}", "0".repeat(62)),
        ] {
            assert_eq!(Key::parse(&bad), None, "{bad}");
        }
    }

    /// A plan of a document with no features: no step has an owner.
    fn empty_plan() -> Plan {
        Plan::of(
            &Arc::new(checked_of(&[])),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
    }

    /// Spins until `gap` has passed since `from` on the test's own monotonic clock: time let
    /// pass, never slept.
    fn spin_past(from: Instant, gap: Duration) {
        while from.elapsed() < gap {
            std::thread::yield_now();
        }
    }

    #[test]
    fn a_stopped_counts_clock_ends_at_its_stop_and_its_end_keeps_that_time() {
        // Stopped: its clock read at its stop from then on, though it hasn't ended, between
        // what the test's clock measures before its making and just after the stop; stopped
        // again, nothing changes; ended, its time is its stop's.
        let before = Instant::now();
        let counter = StepCounter::default();
        let made = Instant::now();
        spin_past(made, Duration::from_millis(5));
        counter.stopped();
        let after = before.elapsed();
        let at_stop = counter.now();
        assert!(!at_stop.ended);
        assert!(at_stop.elapsed >= Duration::from_millis(5));
        assert!(
            at_stop.elapsed <= after,
            "{:?} past {after:?}",
            at_stop.elapsed
        );
        spin_past(Instant::now(), Duration::from_millis(20));
        counter.stopped();
        assert_eq!(counter.now().elapsed, at_stop.elapsed);
        counter.ended(false);
        let ended = counter.now();
        assert!(ended.ended);
        assert_eq!(ended.elapsed, at_stop.elapsed);
        // Ended first: a stop after its end leaves the time its end fixed.
        let finished = StepCounter::default();
        finished.ended(true);
        let end = finished.now().elapsed;
        spin_past(Instant::now(), Duration::from_millis(20));
        finished.stopped();
        assert_eq!(finished.now().elapsed, end);
    }

    #[test]
    fn a_superseded_or_cancelled_runs_clock_ends_as_it_stops_though_its_step_is_still_held() {
        // A run held at its box past its supersede (as the pool lets a running request go on
        // for its grace): superseded by a newer run, then, another time, cancelled. Each
        // one's clock ends as it's stopped, though its box is still held and its count
        // hasn't ended: at most what the test's clock measures from before its start to just
        // after the stop; read again 20 ms on, the same; and its count's end, once the box is
        // let go, keeps it, its `done` as it was at the stop (the drained box isn't settled).
        let wait = Duration::from_secs(60);
        let block = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
        let checked = |text: &str| {
            Arc::new(Checked::new(crate::document::read(text.as_bytes()).unwrap()).unwrap())
        };
        for superseded in [true, false] {
            let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap());
            let hold = engine.kernel().hold_past_supersede(|op| {
                matches!(op, crate::kernel::Op::MakePrimitive(Primitive::Box { .. }))
            });
            let before = Instant::now();
            let first = engine.start(checked(block));
            assert!(engine.kernel().wait_running(1, wait));
            spin_past(before, Duration::from_millis(5));
            let newer = superseded.then(|| {
                engine.start(checked(
                    &block.replace(r#""length": 40"#, r#""length": 41"#),
                ))
            });
            if !superseded {
                engine.cancel();
            }
            let after = before.elapsed();
            let progress = first.progress();
            let at_stop = progress.now();
            assert!(
                !at_stop.ended,
                "superseded {superseded}: its box is still held"
            );
            assert!(
                at_stop.elapsed <= after,
                "{:?} past {after:?}",
                at_stop.elapsed
            );
            spin_past(Instant::now(), Duration::from_millis(20));
            assert_eq!(progress.now().elapsed, at_stop.elapsed);
            hold.release();
            assert!(matches!(
                first.wait_timeout(wait),
                Ok(Ok(RunOutcome::Superseded))
            ));
            let ended = progress.now();
            assert!(ended.ended);
            assert_eq!(
                (ended.elapsed, ended.done),
                (at_stop.elapsed, at_stop.done),
                "superseded {superseded}: its end kept its stop's time and done"
            );
            if let Some(newer) = newer {
                assert!(matches!(
                    newer.wait_timeout(wait),
                    Ok(Ok(RunOutcome::Published(_)))
                ));
            }
        }
    }

    #[test]
    fn a_step_counts_change_is_waited_for_and_nothing_else_ends_the_wait() {
        // `changed_since`: a waiter returns once the count changes (a plan, a step's or a
        // sketch's start or settle, whether or not it moves `done` or `steps`), or it ends;
        // never while nothing changes (the watcher would spin, waking for nothing): a
        // refusal of nothing, or a sketch's solve that isn't reported, changes nothing.
        let counter = StepCounter::default();
        counter.listing(&checked_of(&[
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
        ]));
        let plan = empty_plan();
        let (said, answer) = mpsc::channel();
        let waiting = counter.clone();
        let waiter = std::thread::spawn(move || {
            let mut seen = 0;
            for _ in 0..6 {
                let (count, changes) = waiting.changed_since(seen);
                let _ = said.send((count, changes));
                seen = changes;
            }
        });
        let quiet = Duration::from_millis(200);
        let wait = Duration::from_secs(60);
        let nothing = |why: &str| {
            assert!(answer.recv_timeout(quiet).is_err(), "returned with {why}");
        };
        nothing("nothing changed");
        counter.planned(2, [], BTreeMap::new(), &Found::default());
        let (planned, one) = answer.recv_timeout(wait).unwrap();
        assert_eq!(
            (planned.done, planned.steps, planned.ended, one),
            (0, 2, false, 1)
        );
        nothing("nothing changed");
        counter.judged(&[], &[], &plan);
        counter.sketch_started(None);
        counter.sketch_settled(None, StepState::Built);
        nothing("a refusal of nothing, or a solve not reported");
        // A step's start: neither count moves, but the step running does.
        let key = Key([7; 32]);
        counter.started(key, &plan);
        let (started, two) = answer.recv_timeout(wait).unwrap();
        assert_eq!((started.done, started.steps, two), (0, 2, 2));
        nothing("nothing changed");
        // A sketch's settle, with no plan's count moving: the box's state moves.
        counter.sketch_settled(Some(0), StepState::Failed);
        let (sketch, three) = answer.recv_timeout(wait).unwrap();
        assert_eq!((sketch.done, sketch.steps, three), (0, 2, 3));
        assert_eq!(
            sketch.features.get(&FeatureId::new("b0000001").unwrap()),
            Some(StepState::Failed)
        );
        counter.settled(key, Settled::Made, &plan);
        let (settled, four) = answer.recv_timeout(wait).unwrap();
        assert_eq!((settled.done, settled.steps, four), (1, 2, 4));
        // The end is told, not counted: the watcher tells it once it's recorded.
        counter.ended(true);
        let (ended, at) = answer.recv_timeout(wait).unwrap();
        assert_eq!((ended.done, ended.steps, ended.ended, at), (2, 2, true, 4));
        // Of a count that has ended, at once.
        let (again, _) = answer.recv_timeout(wait).unwrap();
        assert!(again.ended);
        waiter.join().unwrap();
    }

    #[test]
    fn a_counts_clock_runs_from_its_making_and_is_fixed_at_its_end() {
        // Bounds by the test's own monotonic clock, read around the counter's making and
        // each reading; time let pass by spinning on that clock, never by a sleep.
        let before = Instant::now();
        let counter = StepCounter::default();
        let made = Instant::now();
        // Running: the time so far, at least what passed since it was made, at most what
        // passed since before.
        spin_past(made, Duration::from_millis(5));
        let running = counter.now();
        let wall = before.elapsed();
        assert!(!running.ended);
        assert!(
            running.elapsed >= Duration::from_millis(5) && running.elapsed <= wall,
            "{:?}, not 5 ms to {wall:?}",
            running.elapsed
        );
        // It grows: read 2 ms after the first reading answered, at least 2 ms more.
        let answered = Instant::now();
        spin_past(answered, Duration::from_millis(2));
        let later = counter.now();
        assert!(later.elapsed >= running.elapsed + Duration::from_millis(2));
        // A waiter is given the time so far too.
        counter.planned(1, [], BTreeMap::new(), &Found::default());
        let (planned, _) = counter.changed_since(0);
        assert!(planned.elapsed >= later.elapsed);
        // Ended: fixed between what passed since it was made before the end, and what
        // passed since before after it.
        let ending = Instant::now();
        counter.ended(true);
        let wall = before.elapsed();
        let fixed = counter.now();
        assert!(fixed.ended);
        assert!(
            fixed.elapsed >= ending.duration_since(made) && fixed.elapsed <= wall,
            "{:?}, not {:?} to {wall:?}",
            fixed.elapsed,
            ending.duration_since(made)
        );
        // However long after, and however read, and an end said again changes nothing.
        spin_past(Instant::now(), Duration::from_millis(2));
        assert_eq!(counter.now().elapsed, fixed.elapsed);
        assert_eq!(counter.changed_since(0).0.elapsed, fixed.elapsed);
        counter.ended(false);
        assert_eq!(counter.now(), fixed);
    }

    #[test]
    fn a_later_rounds_plan_never_takes_the_count_back() {
        // Found by the robustness review: each round's plan set the count to its own, and a
        // later round counts unsettled the steps the round before settled without a result
        // (never to run): `done` went back. The steps of a later round's plan hold the
        // earlier's, so `steps` only grows; `done` stays at least what it was.
        let counter = StepCounter::default();
        let keys = |n: u8| (0..n).map(|k| Key([k; 32]));
        counter.planned(5, keys(5), BTreeMap::new(), &Found::default());
        assert_eq!((counter.now().done, counter.now().steps), (5, 5));
        counter.planned(8, keys(3), BTreeMap::new(), &Found::default());
        assert_eq!((counter.now().done, counter.now().steps), (5, 8));
        counter.planned(7, keys(6), BTreeMap::new(), &Found::default());
        assert_eq!((counter.now().done, counter.now().steps), (6, 8));
        assert_eq!(counter.rounds(), 3);
    }

    #[test]
    fn a_feature_state_is_a_byte_and_a_counts_copy_shares_its_ids() {
        // Issue #367's cost: `status` copies each state, a byte a feature, and never the
        // IDs, which every copy shares with the counter.
        assert_eq!(std::mem::size_of::<StepState>(), 1);
        let counter = StepCounter::default();
        counter.listing(&checked_of(&[
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
            r#"{"id": "b0000002", "type": "box", "v": 1, "name": "B", "suppressed": true, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
            r#"{"id": "b0000003", "type": "box", "v": 1, "name": "C", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
        ]));
        let (one, two) = (counter.now(), counter.now());
        assert!(Arc::ptr_eq(&one.features.ids, &two.features.ids));
        // The suppressed one isn't listed; the rest are, queued, in order.
        let listed: Vec<(&str, StepState)> = one
            .features
            .iter()
            .map(|(id, state)| (id.as_str(), state))
            .collect();
        assert_eq!(
            listed,
            [
                ("b0000001", StepState::Queued),
                ("b0000003", StepState::Queued)
            ]
        );
    }

    #[test]
    fn a_states_moves_are_the_ones_a_rebuild_may_make() {
        // By hand, from the decision: a failure or fault stays (the first wins); blocked
        // overrides what isn't failed; built only from queued or running; queued again only
        // from running; running only from queued.
        use StepState::{Blocked, Built, Failed, Faulted, Queued, Running};
        let allowed: [(StepState, &[StepState]); 6] = [
            (Queued, &[Running, Built, Failed, Blocked, Faulted]),
            (Running, &[Queued, Built, Failed, Blocked, Faulted]),
            (Built, &[Failed, Blocked, Faulted]),
            (Failed, &[]),
            (Blocked, &[]),
            (Faulted, &[]),
        ];
        for (from, to) in allowed {
            for state in StepState::ALL {
                assert_eq!(
                    from.may_become(state),
                    to.contains(&state),
                    "{from:?} to {state:?}"
                );
            }
        }
        // The words, as the wire's schema lists them.
        assert_eq!(
            StepState::ALL.map(StepState::as_str),
            ["queued", "running", "built", "failed", "blocked", "faulted"]
        );
    }

    #[test]
    fn a_rebuild_that_ends_unfinished_leaves_what_ran_queued_and_a_finished_one_leaves_it() {
        // A sketch's solve started and never settled (its rebuild superseded mid-solve):
        // stopped, it's queued, to resume; what had settled stays. Finished, nothing moves.
        let document = checked_of(&[
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
            r#"{"id": "b0000002", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 2, "width": 1, "height": 1}"#.to_owned(),
        ]);
        let of = |counter: &StepCounter| -> Vec<StepState> {
            counter.now().features.iter().map(|(_, s)| s).collect()
        };
        let stopped = StepCounter::default();
        stopped.listing(&document);
        // Listed, the two boxes are there: neither none, nor any other count.
        assert!(!stopped.now().features.is_empty());
        assert_eq!(stopped.now().features.len(), 2);
        stopped.sketch_settled(Some(0), StepState::Built);
        stopped.sketch_started(Some(1));
        assert_eq!(of(&stopped), [StepState::Built, StepState::Running]);
        stopped.ended(false);
        assert_eq!(of(&stopped), [StepState::Built, StepState::Queued]);
        let finished = StepCounter::default();
        finished.listing(&document);
        finished.sketch_started(Some(1));
        finished.ended(true);
        assert_eq!(of(&finished), [StepState::Queued, StepState::Running]);
        // A place past the list (none listed) moves nothing, and doesn't panic.
        let empty = StepCounter::default();
        empty.sketch_started(Some(7));
        empty.sketch_settled(None, StepState::Failed);
        assert!(empty.now().features.is_empty());
    }

    #[test]
    fn a_step_settled_in_two_rounds_is_counted_once_so_done_stays_under_steps_while_one_runs() {
        // From #344's review: a step settled without a result in one round (never to run)
        // is planned unsettled by the next, and settled there again. Counted each time,
        // `done` reached `steps` while another step still ran. Round 1: the step a alone,
        // settled unrun (1 of 1). Round 2: a and b, neither kept (so none settled by the
        // plan); a settled unrun again, b started and running: 1 of 2, by hand.
        let counter = StepCounter::default();
        let plan = empty_plan();
        let (a, b) = (Key([1; 32]), Key([2; 32]));
        counter.planned(1, [], BTreeMap::new(), &Found::default());
        counter.settled(a, Settled::Unrun, &plan);
        assert_eq!((counter.now().done, counter.now().steps), (1, 1));
        counter.planned(2, [], BTreeMap::new(), &Found::default());
        assert_eq!((counter.now().done, counter.now().steps), (1, 2));
        counter.started(b, &plan);
        counter.settled(a, Settled::Unrun, &plan);
        assert_eq!((counter.now().done, counter.now().steps), (1, 2));
        // b settles: 2 of 2; and a step a plan counts settled that a later settle names
        // again is counted once too.
        counter.settled(b, Settled::Made, &plan);
        assert_eq!((counter.now().done, counter.now().steps), (2, 2));
        counter.planned(3, [a, b], BTreeMap::new(), &Found::default());
        counter.settled(b, Settled::Unrun, &plan);
        assert_eq!((counter.now().done, counter.now().steps), (2, 3));
    }

    /// A rectangle sketch `id` on the face `face` (token `+x`), as a document writes it, its
    /// corners from (`x`, 1), 1 × 1.
    fn face_sketch(id: &str, face: &str, x: f64) -> String {
        // Its points p1… to p4…, and its lines n1… to n4…, each the sketch's ID's last six.
        let tail = &id[2..];
        let points: Vec<String> = [(x, 1.0), (x + 1.0, 1.0), (x + 1.0, 2.0), (x, 2.0)]
            .iter()
            .zip(1..)
            .map(|((px, py), n)| {
                format!(r#"{{"id": "p{n}{tail}", "x": {px}, "y": {py}, "construction": false}}"#)
            })
            .collect();
        let lines: Vec<String> = (1..=4)
            .map(|n| {
                format!(
                    r#"{{"id": "n{n}{tail}", "from": "p{n}{tail}", "to": "p{}{tail}", "construction": false}}"#,
                    n % 4 + 1
                )
            })
            .collect();
        format!(
            r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "S", "plane": {{"face": "{face}", "x": "+x", "reversed": false}}, "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
            points.join(", "),
            lines.join(", ")
        )
    }

    /// A pad `id` of `sketch`, 1 long, onto `base`.
    fn face_pad(id: &str, base: &str, sketch: &str) -> String {
        format!(
            r#"{{"id": "{id}", "type": "pad", "v": 1, "name": "P", "base": "{base}", "sketch": "{sketch}", "extent": {{"type": "one_side", "length": 1, "reversed": false}}}}"#
        )
    }

    fn checked_of(features: &[String]) -> Checked {
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
            features.join(", ")
        );
        Checked::new(crate::document::read(text.as_bytes()).unwrap()).unwrap()
    }

    /// `document` rebuilt on the fake, on this thread (so [`face_walks`] counts its walk):
    /// the walk's steps, and the rounds.
    fn walked_rebuilding(document: Checked) -> (u64, usize, Arc<Model>) {
        let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap())
            .with_sketches(crate::fake::FakeSketches::new());
        let generation = lock(&engine.engine.state).generation;
        let progress = StepCounter::default();
        let before = face_walks();
        let outcome = engine
            .engine
            .run(&Arc::new(document), generation, &progress, Reads::Kept)
            .unwrap();
        let RunOutcome::Published(model) = outcome else {
            panic!("not published");
        };
        (face_walks() - before, progress.rounds(), model)
    }

    /// Whether a feature read `was` in one copy of a count may read `is` in a later one, as
    /// a rebuild may move it, by hand from the decision: unsettled (queued, or running)
    /// becomes anything; built may yet fail its check, or fault, or be blocked; failed,
    /// faulted, and blocked are final. Copies may skip states between them, so this is the
    /// moves' closure, not each move.
    fn may_follow(was: StepState, is: StepState) -> bool {
        use StepState::{Blocked, Built, Failed, Faulted, Queued, Running};
        was == is
            || matches!(was, Queued | Running)
            || (was == Built && matches!(is, Failed | Faulted | Blocked))
    }

    /// Each feature's states over `copies` of a count, in the order they were taken, never
    /// go back ([`may_follow`]), and nothing reads running in a copy of a count that has
    /// ended.
    #[track_caller]
    fn never_back(copies: &[StepCount]) {
        for pair in copies.windows(2) {
            let [before, after] = pair else { continue };
            assert_eq!(before.features.len(), after.features.len());
            for ((feature, was), (_, is)) in before.features.iter().zip(after.features.iter()) {
                assert!(
                    may_follow(was, is),
                    "{feature} went from {was:?} to {is:?}: {copies:#?}"
                );
            }
        }
        for copy in copies.iter().filter(|copy| copy.ended) {
            for (feature, state) in copy.features.iter() {
                assert_ne!(state, StepState::Running, "{feature} running once ended");
            }
        }
    }

    #[test]
    fn a_rebuild_in_rounds_never_takes_a_feature_back_and_ends_with_its_models_outcome() {
        // A box, two sketches on its top in turn, each padded onto the last, and a third on
        // its top that nothing sweeps: planned in rounds, the sketches waiting on their faces.
        // The fake types no face a plane, so the first sketch's face is broken: failed, as its
        // own problem; its pad, the second sketch, its pad, and the third sketch blocked on
        // it (their faces' names are found through the first pad's solid); the box built.
        // After the last round, nothing is queued. And a counter told nothing but the model's
        // outcomes says the same: the states a finished rebuild leaves are its model's.
        //
        // On the way, a copy of the count is taken as each sketch's solve starts and each
        // kernel request is made (by the fakes' own rules), and as often as a waiter on its
        // changes is woken: no feature ever goes back in them. Once, the swept sketches
        // solved before the first plan read running, then built, and the first round's plan
        // took them back to queued (they wait on their faces), then failed or blocked.
        let mut features = vec![
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#
                .to_owned(),
        ];
        for n in 1..=2 {
            let sketch = format!("s{n:07}");
            let base = if n == 1 {
                "b0000001".to_owned()
            } else {
                format!("p{:07}", n - 1)
            };
            features.push(face_sketch(&sketch, "b0000001.box.Face6", n as f64));
            features.push(face_pad(&format!("p{n:07}"), &base, &sketch));
        }
        features.push(face_sketch("s0000003", "b0000001.box.Face6", 3.0));
        let document = Arc::new(checked_of(&features));
        let progress = StepCounter::default();
        progress.listing(&document);
        let copies: Arc<Mutex<Vec<StepCount>>> = Arc::default();
        // Each copy taken with the list held, so the list is in the order they were taken.
        let copy = {
            let (copies, progress) = (Arc::clone(&copies), progress.clone());
            move || {
                let mut copies = lock(&copies);
                copies.push(progress.now());
            }
        };
        let kernel = crate::fake::FakeKernel::new(2).unwrap();
        let on_request = copy.clone();
        kernel.refuse(move |_| {
            on_request();
            None
        });
        let sketches = crate::fake::FakeSketches::new();
        let on_solve = copy.clone();
        sketches.answer(move |system| {
            on_solve();
            Ok(crate::fake::FakeSketches::as_sent(system))
        });
        let engine = Recompute::new(kernel).with_sketches(sketches);
        let generation = lock(&engine.engine.state).generation;
        let waiter = {
            let (progress, copy) = (progress.clone(), copy.clone());
            std::thread::spawn(move || {
                let mut seen = 0;
                loop {
                    let (count, changes) = progress.changed_since(seen);
                    copy();
                    if count.ended {
                        break;
                    }
                    seen = changes;
                }
            })
        };
        let RunOutcome::Published(model) = engine
            .engine
            .run(&document, generation, &progress, Reads::Kept)
            .unwrap()
        else {
            panic!("not published");
        };
        // As the recompute's own thread ends it.
        progress.ended(true);
        waiter.join().unwrap();
        copy();
        assert!(progress.rounds() > 1, "{} rounds", progress.rounds());
        use StepState::{Blocked, Built, Failed, Queued};
        let want = [Built, Failed, Blocked, Blocked, Blocked, Blocked];
        let states = |counter: &StepCounter| -> Vec<StepState> {
            counter.now().features.iter().map(|(_, s)| s).collect()
        };
        assert_eq!(states(&progress), want);
        let told = StepCounter::default();
        told.listing(&document);
        told.concluded(&model);
        assert_eq!(states(&told), want);
        let copies = lock(&copies).clone();
        // Three solves and the box's step at least, each copied, and the end.
        assert!(copies.len() >= 5, "{copies:#?}");
        never_back(&copies);
        // A sketch on a face is queued until the round that places it says what it came to:
        // never running, nor built, before then (its solve isn't what it comes to).
        for copy in &copies {
            for (feature, state) in copy.features.iter() {
                if feature.as_str().starts_with('s') {
                    assert!(
                        matches!(state, Queued | Failed | Blocked),
                        "{feature} read {state:?}"
                    );
                }
            }
        }
        // The swept sketches' solves came before any plan; then the box's step; each
        // copied with every sketch queued.
        let first = FeatureId::new("s0000001").unwrap();
        assert!(
            copies
                .iter()
                .any(|c| c.steps == 0 && c.features.get(&first) == Some(Queued)),
            "{copies:#?}"
        );
    }

    #[test]
    fn a_sketch_on_a_planar_face_reads_queued_through_its_solve_then_built_in_its_round() {
        // A box; a sketch on its top, padded onto it; and another on its top that nothing
        // sweeps. The fake made to describe every face as the plane z = 10, so both faces are
        // found planar and placed once the box is built: the first round builds the box, the
        // second places both sketches and builds the pad. Each sketch on a face reads queued
        // until the round that places it, its solve included (the padded one solved before
        // any plan, the other beside the box's step), then built; never running, never
        // back. Built in its round, not only at the end: whenever the pad reads running
        // (copied as each of its requests is made) or built, both sketches read built.
        let features = vec![
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#
                .to_owned(),
            face_sketch("s0000001", "b0000001.box.Face6", 1.0),
            face_sketch("s0000003", "b0000001.box.Face6", 3.0),
            face_pad("p0000001", "b0000001", "s0000001"),
        ];
        let document = Arc::new(checked_of(&features));
        let progress = StepCounter::default();
        progress.listing(&document);
        // Each copy with whether a solve was running as it was taken.
        let copies: Arc<Mutex<Vec<(bool, StepCount)>>> = Arc::default();
        let copy = {
            let (copies, progress) = (Arc::clone(&copies), progress.clone());
            move |solving: bool| {
                let mut copies = lock(&copies);
                copies.push((solving, progress.now()));
            }
        };
        let kernel = crate::fake::FakeKernel::new(2).unwrap();
        let on_request = copy.clone();
        kernel.refuse(move |_| {
            on_request(false);
            None
        });
        kernel.answer_instead(|op| {
            let crate::kernel::Op::DescribeAll { shape } = op else {
                return None;
            };
            let mut description = crate::fake::FakeKernel::described(shape)?;
            for face in &mut description.faces {
                face.surface = silico_protocol::Surface::Plane {
                    normal: [0.0, 0.0, 1.0],
                    location: [0.0, 0.0, 10.0],
                };
            }
            Some(silico_protocol::Reply::DescribedAll(description))
        });
        let sketches = crate::fake::FakeSketches::new();
        let on_solve = copy.clone();
        sketches.answer(move |system| {
            on_solve(true);
            Ok(crate::fake::FakeSketches::as_sent(system))
        });
        let engine = Recompute::new(kernel).with_sketches(sketches);
        let generation = lock(&engine.engine.state).generation;
        let waiter = {
            let (progress, copy) = (progress.clone(), copy.clone());
            std::thread::spawn(move || {
                let mut seen = 0;
                loop {
                    let (count, changes) = progress.changed_since(seen);
                    copy(false);
                    if count.ended {
                        break;
                    }
                    seen = changes;
                }
            })
        };
        let RunOutcome::Published(model) = engine
            .engine
            .run(&document, generation, &progress, Reads::Kept)
            .unwrap()
        else {
            panic!("not published");
        };
        progress.ended(true);
        waiter.join().unwrap();
        copy(false);
        assert_eq!(progress.rounds(), 2);
        use StepState::{Built, Queued, Running};
        let states = |counter: &StepCounter| -> Vec<StepState> {
            counter.now().features.iter().map(|(_, s)| s).collect()
        };
        assert_eq!(states(&progress), [Built; 4]);
        let told = StepCounter::default();
        told.listing(&document);
        told.concluded(&model);
        assert_eq!(states(&told), [Built; 4]);
        let copies = lock(&copies).clone();
        never_back(&copies.iter().map(|(_, c)| c.clone()).collect::<Vec<_>>());
        let (s1, s3, pad) = (
            FeatureId::new("s0000001").unwrap(),
            FeatureId::new("s0000003").unwrap(),
            FeatureId::new("p0000001").unwrap(),
        );
        let solves = copies.iter().filter(|(solving, _)| *solving).count();
        assert_eq!(solves, 2, "{copies:#?}");
        for (solving, copy) in &copies {
            for sketch in [&s1, &s3] {
                let state = copy.features.get(sketch);
                assert_ne!(state, Some(Running), "{sketch}: {copies:#?}");
                if *solving {
                    assert_eq!(state, Some(Queued), "{sketch} while a solve ran");
                }
                if matches!(copy.features.get(&pad), Some(Running | Built)) {
                    assert_eq!(state, Some(Built), "{sketch} with the pad begun");
                }
            }
        }
        assert!(
            copies
                .iter()
                .any(|(_, c)| !c.ended && c.features.get(&pad) == Some(Running)),
            "{copies:#?}"
        );
    }

    /// Two unit boxes apart, `a0000001` and `a0000002`, fused (`f0000001`), and a pad
    /// (`p0000001`) onto the fuse of a segment (`s0000001`), which makes no profile: the pad
    /// is refused by its plan, before any step.
    fn refused_pad_on_a_fuse() -> Vec<String> {
        vec![
            r#"{"id": "a0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
            r#"{"id": "a0000002", "type": "box", "v": 1, "name": "B", "position": [5, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
            r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "F", "base": "a0000001", "tool": "a0000002"}"#.to_owned(),
            r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Segment", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#.to_owned(),
            r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "P", "base": "f0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 1, "reversed": false}}"#.to_owned(),
        ]
    }

    /// A fuse's reply of two solids, as a fuse of two boxes apart is: each box's elements
    /// kept, the second's numbered after the first's.
    fn split_fuse() -> impl Fn(&crate::kernel::Op) -> Option<silico_protocol::Reply> {
        use crate::kernel::{ElementCounts, ElementFate, History, InputHistory, Topology};
        let fates = |first: u32, n: u32| -> Vec<ElementFate> {
            (first..first + n)
                .map(|k| ElementFate {
                    kept: Some(k),
                    ..ElementFate::default()
                })
                .collect()
        };
        let split = History {
            inputs: vec![
                InputHistory {
                    faces: fates(1, 6),
                    edges: fates(1, 12),
                    vertices: fates(1, 8),
                },
                InputHistory {
                    faces: fates(7, 6),
                    edges: fates(13, 12),
                    vertices: fates(9, 8),
                },
            ],
            result: ElementCounts {
                faces: 12,
                edges: 24,
                vertices: 16,
                solids: 2,
            },
            ends: Default::default(),
            topology: Topology {
                face_edges: vec![vec![]; 12],
                outer_edges: vec![vec![]; 12],
                edge_vertices: vec![vec![]; 24],
                solid_faces: vec![(1..=6).collect(), (7..=12).collect()],
            },
        };
        move |op| match op {
            crate::kernel::Op::Boolean {
                op: BooleanOp::Fuse,
                base,
                ..
            } => Some(silico_protocol::Reply::Built(crate::kernel::BuiltData {
                shape: base.clone(),
                history: split.clone(),
            })),
            _ => None,
        }
    }

    #[test]
    fn a_pad_refused_by_its_plan_on_a_base_whose_check_then_fails_is_queued_then_blocked() {
        // Round 2's major: the fuse of two boxes apart makes two solids, which its check
        // refuses; the pad on it, of a segment, is refused by its plan. The model says the
        // pad is blocked on the fuse. The fuse's step settles built before its check is
        // judged (two changes of the count), and the pad read failed in between, then could
        // never be blocked. Driven by hand, as the recompute's threads drive the count, on
        // the plan of this document: the pad reads queued until the fuse's check is decided,
        // then blocked, its model's outcome; nothing goes back.
        let document = Arc::new(checked_of(&refused_pad_on_a_fuse()));
        let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap())
            .with_sketches(crate::fake::FakeSketches::new());
        engine.kernel().answer_instead(split_fuse());
        let generation = lock(&engine.engine.state).generation;
        let RunOutcome::Published(model) = engine
            .engine
            .run(&document, generation, &StepCounter::default(), Reads::Kept)
            .unwrap()
        else {
            panic!("not published");
        };
        let (fuse, pad) = (
            FeatureId::new("f0000001").unwrap(),
            FeatureId::new("p0000001").unwrap(),
        );
        assert!(
            matches!(model.outcome(&pad), Some(FeatureOutcome::Blocked { on }) if *on == fuse),
            "{:?}",
            model.outcome(&pad)
        );
        let first = engine
            .engine
            .solve_swept(&document, generation, Reads::Kept, &StepCounter::default())
            .unwrap()
            .unwrap();
        let plan = Plan::of(&document, &first.swept, &BTreeMap::new());
        let at = |id: &FeatureId| document.place(id).unwrap();
        let fuse_key = plan.features[at(&fuse)].key().unwrap();
        let counter = StepCounter::default();
        counter.listing(&document);
        let found = plan.states(
            &BTreeMap::new(),
            &BTreeSet::new(),
            &first.solutions,
            &Refusals::default(),
            &BTreeMap::new(),
        );
        counter.planned(plan.order.len(), [], plan.labels(&document), &found);
        let mut copies = vec![counter.now()];
        for &key in &plan.order {
            counter.started(key, &plan);
            copies.push(counter.now());
            counter.settled(key, Settled::Made, &plan);
            copies.push(counter.now());
            if key == fuse_key {
                // Built, its check not yet judged: the pad waits on it.
                assert_eq!(counter.now().features.get(&pad), Some(StepState::Queued));
                counter.judged(&[], &[(at(&fuse), StepState::Failed)], &plan);
                copies.push(counter.now());
            }
        }
        never_back(&copies);
        let want = StepState::of_outcome(model.outcome(&pad).unwrap());
        assert_eq!(counter.now().features.get(&pad), want);
        assert_eq!(want, Some(StepState::Blocked));
        assert_eq!(counter.now().features.get(&fuse), Some(StepState::Failed));
        // Its check passed instead, the pad says its own refusal at once.
        let passing = StepCounter::default();
        passing.listing(&document);
        passing.planned(plan.order.len(), [], plan.labels(&document), &found);
        for &key in &plan.order {
            passing.started(key, &plan);
            passing.settled(key, Settled::Made, &plan);
        }
        assert_eq!(passing.now().features.get(&pad), Some(StepState::Queued));
        passing.judged(&[at(&fuse)], &[], &plan);
        assert_eq!(passing.now().features.get(&pad), Some(StepState::Failed));
    }

    #[test]
    fn a_pad_refused_by_its_plan_on_a_fillet_not_yet_built_is_queued_then_blocked() {
        // Round 3's low: a box, a fillet of one of its edges, and a pad onto the fillet of a
        // segment, which makes no profile: the pad is refused by its plan, before any step.
        // The kernel refuses the fillet, so the model says the pad is blocked on it. The
        // fillet has no check, so once the box is built and firm, nothing but the fillet's
        // own step is left to decide the pad: a fillet taken as firm before it built would
        // say the pad's refusal (failed) as soon as the box's step settled, and the fillet's
        // failure then could never block it. Driven by hand, as the recompute's threads drive
        // the count: the pad reads queued until the fillet's step settles, then blocked, its
        // model's outcome; nothing goes back.
        let features = [
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#.to_owned(),
            r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "b0000001", "edges": ["b0000001.box.Edge1"], "radius": 1}"#.to_owned(),
            r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Segment", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#.to_owned(),
            r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "P", "base": "f0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 1, "reversed": false}}"#.to_owned(),
        ];
        let document = Arc::new(checked_of(&features));
        let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap())
            .with_sketches(crate::fake::FakeSketches::new());
        engine.kernel().refuse(|op| {
            matches!(op, silico_protocol::Op::Fillet { .. })
                .then(|| Error::new(ErrorCode::KernelFailed, "Fillet failed"))
        });
        let generation = lock(&engine.engine.state).generation;
        let RunOutcome::Published(model) = engine
            .engine
            .run(&document, generation, &StepCounter::default(), Reads::Kept)
            .unwrap()
        else {
            panic!("not published");
        };
        let (boxed, fillet, pad) = (
            FeatureId::new("b0000001").unwrap(),
            FeatureId::new("f0000001").unwrap(),
            FeatureId::new("p0000001").unwrap(),
        );
        assert!(
            matches!(model.outcome(&pad), Some(FeatureOutcome::Blocked { on }) if *on == fillet),
            "{:?}",
            model.outcome(&pad)
        );
        let first = engine
            .engine
            .solve_swept(&document, generation, Reads::Kept, &StepCounter::default())
            .unwrap()
            .unwrap();
        let plan = Plan::of(&document, &first.swept, &BTreeMap::new());
        let at = |id: &FeatureId| document.place(id).unwrap();
        let (box_key, fillet_key) = (
            plan.features[at(&boxed)].key().unwrap(),
            plan.features[at(&fillet)].key().unwrap(),
        );
        assert_eq!(plan.order, [box_key, fillet_key]);
        let counter = StepCounter::default();
        counter.listing(&document);
        let found = plan.states(
            &BTreeMap::new(),
            &BTreeSet::new(),
            &first.solutions,
            &Refusals::default(),
            &BTreeMap::new(),
        );
        counter.planned(plan.order.len(), [], plan.labels(&document), &found);
        let mut copies = vec![counter.now()];
        counter.started(box_key, &plan);
        copies.push(counter.now());
        counter.settled(box_key, Settled::Made, &plan);
        copies.push(counter.now());
        // The box built and firm, the fillet's step not yet started: the pad waits on it.
        assert_eq!(counter.now().features.get(&boxed), Some(StepState::Built));
        assert_eq!(counter.now().features.get(&fillet), Some(StepState::Queued));
        assert_eq!(counter.now().features.get(&pad), Some(StepState::Queued));
        counter.started(fillet_key, &plan);
        copies.push(counter.now());
        assert_eq!(counter.now().features.get(&pad), Some(StepState::Queued));
        counter.settled(fillet_key, Settled::Failed(StepState::Failed), &plan);
        copies.push(counter.now());
        assert_eq!(counter.now().features.get(&pad), Some(StepState::Blocked));
        assert_eq!(counter.now().features.get(&fillet), Some(StepState::Failed));
        never_back(&copies);
        let want = StepState::of_outcome(model.outcome(&pad).unwrap());
        assert_eq!(counter.now().features.get(&pad), want);
    }

    #[test]
    fn many_sketches_on_one_face_of_a_chain_cost_each_round_a_walk_linear_in_the_timeline() {
        // The robustness review: a box B, then K times a sketch S_k on B's top and a pad P_k
        // of it onto P_{k−1} (P_1 onto B). S_k's face is looked for in P_{k−1}, the last of
        // B's chain of consumers before it, so the rounds are K (each places one), and a
        // last that finds nothing waiting. Each round walked, for each sketch waiting, every
        // feature before it (what's made from B, then which of those are live): K² a round,
        // K³ in all. Now each round reads each feature's inputs once, walks B's chain once
        // (to its first link that waits), looks at each sketch once, and passes each feature
        // once to keep what's live: at most 4 F steps a round, F = 2K + 1 the features. For
        // K = 50, F = 101: at most 404 a round, where the walk per sketch made about 2 · 2k
        // for each sketch k waiting, some 3,300 a round on average.
        let k = 50;
        let mut features = vec![
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#
                .to_owned(),
        ];
        for n in 1..=k {
            let sketch = format!("s{n:07}");
            let base = if n == 1 {
                "b0000001".to_owned()
            } else {
                format!("p{:07}", n - 1)
            };
            features.push(face_sketch(&sketch, "b0000001.box.Face6", (n % 8) as f64));
            features.push(face_pad(&format!("p{n:07}"), &base, &sketch));
        }
        let f = features.len() as u64;
        let (walks, rounds, model) = walked_rebuilding(checked_of(&features));
        // The fake types no face a plane: S1 is refused, and the rest blocked on it.
        assert!(matches!(
            model.outcome(&FeatureId::new("s0000001").unwrap()),
            Some(FeatureOutcome::Broken { .. })
        ));
        assert_eq!(
            model.outcome(&FeatureId::new("p0000050").unwrap()),
            Some(&FeatureOutcome::Blocked {
                on: FeatureId::new("s0000001").unwrap()
            })
        );
        assert_eq!(rounds, k + 1);
        assert!(
            walks <= 4 * f * rounds as u64,
            "{walks} steps in {rounds} rounds, past 4 F = {} a round",
            4 * f
        );
        // And at least F for each round but the last: each round with a sketch waiting
        // passes each feature once to keep what's live, and the last finds none waiting and
        // walks nothing. A count that missed the walk (or a walk that skipped features)
        // reads under it.
        assert!(
            walks >= f * (rounds as u64 - 1),
            "{walks} steps in {rounds} rounds, under F = {f} for each but the last"
        );
    }

    #[test]
    fn four_hundred_sketches_on_one_boxs_faces_are_looked_for_in_one_walk_of_the_timeline() {
        // Each sketch is on the box, live before every one: all placed (refused, on the fake)
        // in the first round, and a second finds none waiting. The first's walk reads each
        // feature's inputs once, walks the box's chain (itself) once, and passes each feature
        // once: at most 4 F steps, F = 401. Each walking the features before it made
        // 400 · 401 of them.
        let mut features = vec![
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#
                .to_owned(),
        ];
        for n in 1..=400usize {
            features.push(face_sketch(
                &format!("s{n:07}"),
                &format!("b0000001.box.Face{}", n % 6 + 1),
                1.0,
            ));
        }
        let f = features.len() as u64;
        let (walks, rounds, model) = walked_rebuilding(checked_of(&features));
        assert_eq!(rounds, 2);
        assert!(walks <= 4 * f, "{walks} steps, past 4 F = {}", 4 * f);
        // And at least F: the first round passes each feature once; the second finds none
        // waiting and walks nothing.
        assert!(
            walks >= f * (rounds as u64 - 1),
            "{walks} steps in {rounds} rounds, under F = {f} for each but the last"
        );
        for n in 1..=400usize {
            let id = FeatureId::new(&format!("s{n:07}")).unwrap();
            let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id) else {
                panic!("{id}: {:?}", model.outcome(&id));
            };
            assert!(errors[0].message().contains("isn't planar"), "{errors:?}");
        }
    }

    #[test]
    fn a_describe_timed_out_before_its_worker_started_is_asked_again_and_one_after_is_kept() {
        // A box's description refused as a timeout, twice recomputed on one engine: one whose
        // worker hadn't started (advised to try again) says nothing about the solid, so the
        // second recompute asks again; one whose worker had (advised to simplify or wait) is
        // kept, so the second asks nothing.
        use crate::worker::Advice;
        let not_started = || Advice::TryAgain {
            once_more: true,
            why: "the worker was slow to start".to_owned(),
        };
        for (advice, asked) in [(not_started(), 2), (Advice::SimplifyOrWait, 1)] {
            let kernel = crate::fake::FakeKernel::new(2).unwrap();
            let given = advice.clone();
            kernel.refuse(move |op| {
                matches!(op, silico_protocol::Op::DescribeAll { .. }).then(|| {
                    Error::new(ErrorCode::WorkerTimeout, "the kernel worker timed out")
                        .with_advice(given.clone())
                })
            });
            let engine = Recompute::new(kernel);
            let document = Arc::new(boxes(1, |_| false));
            for _ in 0..2 {
                published(engine.start(Arc::clone(&document)));
            }
            let described = engine
                .kernel()
                .asked()
                .iter()
                .filter(|call| matches!(call.op, silico_protocol::Op::DescribeAll { .. }))
                .count();
            assert_eq!(described, asked, "{advice:?}");
        }
    }

    #[test]
    fn only_a_hang_of_a_worker_that_had_started_is_kept() {
        use crate::worker::Advice;
        let timeout = || Error::new(ErrorCode::WorkerTimeout, "the kernel worker timed out");
        // Started: its advice to simplify or wait, or none.
        assert!(hung(&timeout().with_advice(Advice::SimplifyOrWait)));
        assert!(hung(&timeout()));
        // Not started: try again, said of the launch, not the solid.
        assert!(!hung(&timeout().with_advice(Advice::TryAgain {
            once_more: true,
            why: "the worker was slow to start".to_owned(),
        })));
        // Not a hang.
        assert!(!hung(
            &Error::new(ErrorCode::KernelFailed, "no").with_advice(Advice::SimplifyOrWait)
        ));
    }

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

    /// A sketch of a 4 by 2 rectangle on xy, its pad, 10 long, and a box: what a fresh
    /// recompute makes again, each part of it.
    const PADDED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Outline", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 4, "y": 0, "construction": false}, {"id": "pa000003", "x": 4, "y": 2, "construction": false}, {"id": "pa000004", "x": 0, "y": 2, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}, {"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}}, {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

    fn published(run: Run) -> Arc<Model> {
        match run.wait_timeout(Duration::from_secs(60)) {
            Ok(Ok(RunOutcome::Published(model))) => model,
            _ => panic!("not published"),
        }
    }

    /// A document of `n` boxes, `b0000000` on: each its own body, 1 mm high, or 2 mm
    /// where `tall` says.
    fn boxes(n: usize, tall: impl Fn(usize) -> bool) -> Checked {
        boxes_with(n, |i| format!("b{i:07}"), tall)
    }

    /// [`boxes`], the box at place i given the ID `id(i)`.
    fn boxes_with(n: usize, id: impl Fn(usize) -> String, tall: impl Fn(usize) -> bool) -> Checked {
        let features: Vec<String> = (0..n)
            .map(|i| {
                let height = if tall(i) { 2 } else { 1 };
                let id = id(i);
                format!(
                    r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [{i}, 0, 0], "length": 1, "width": 1, "height": {height}}}"#
                )
            })
            .collect();
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
            features.join(", ")
        );
        Checked::new(crate::document::read(text.as_bytes()).unwrap()).unwrap()
    }

    /// This thread's comparisons of IDs and lookups in indexes so far.
    fn counted() -> (u64, u64) {
        (crate::document::compared(), crate::document::looked_up())
    }

    /// ⌈log₂ n⌉, for n ≥ 1.
    fn ceil_log2(n: usize) -> u64 {
        u64::from(n.next_power_of_two().trailing_zeros())
    }

    #[test]
    fn reading_every_feature_by_id_compares_n_log_n_ids_not_n_squared() {
        // #182's review: each lookup by ID scanned the timeline from its start, so a read
        // that looks up each feature compared N(N + 1) / 2 IDs (20,000 features, 1.45 s).
        // Counted, not timed, and counted where two IDs are compared (`FeatureId`'s `eq`
        // and `cmp`), so a scan put back is counted as the search is.
        //
        // Each feature is read 9 ways, which look it up by ID 11 times: `outcome`,
        // `feature`, `records`, `fingerprint`, `description` (the last three through its
        // outcome), `named_element_map`, `element_map`, and `unmeasured` once each, and
        // `entries` three times (its `description`'s outcome, its slot's place, and its
        // `element_map`, the slot built here, the first time it's read). All but
        // `unmeasured`'s search the document's index of N IDs: a binary search halves
        // what's left with each comparison, so ⌈log₂ N⌉ comparisons leave one ID and one
        // more decides it, ⌈log₂ N⌉ + 1 each. `unmeasured`'s searches the index of the
        // unmeasured solids, of which there are none, and compares nothing. The maps are
        // named before the reads are counted (naming is no lookup's work, and the
        // background's naming would race the count), and nothing else read compares two
        // IDs. So exactly 11 N lookups, and at most 10 N (⌈log₂ N⌉ + 1) comparisons, at
        // least 10 N: for N = 1,000, 110,000; for N = 4,000, 520,000. With `outcome`'s
        // and `place`'s scans put back, 9 of the lookups (all but `feature`'s and
        // `unmeasured`'s) compare the feature's place + 1 IDs, 9 N(N + 1) / 2 in all, and
        // `feature`'s searches 11 N: 4,515,500 for N = 1,000.
        for n in [1000usize, 4000] {
            let document = Arc::new(boxes(n, |_| false));
            let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap());
            let model = published(engine.start(Arc::clone(&document)));
            assert_eq!(model.features().len(), n);
            assert_eq!(model.bodies().len(), n);
            model.name_all(|| true);
            let (compared, looked_up) = counted();
            for (place, (id, outcome)) in model.features().iter().enumerate() {
                // The feature at its place, as the scan found it.
                assert!(std::ptr::eq(model.outcome(id).unwrap(), outcome), "{place}");
                assert!(std::ptr::eq(
                    model.feature(id).unwrap(),
                    &model.document().features[place]
                ));
                assert!(model.records(id), "{place}");
                assert!(model.fingerprint(id).is_some(), "{place}");
                assert!(model.description(id).is_some(), "{place}");
                assert!(model.named_element_map(id).is_some(), "{place}");
                assert!(model.element_map(id).is_some(), "{place}");
                assert!(matches!(model.entries(id), Some(Ok(_))), "{place}");
                assert!(model.unmeasured(id).is_none(), "{place}");
            }
            let (compared, looked_up) = (counted().0 - compared, counted().1 - looked_up);
            let log2 = ceil_log2(n);
            let n = n as u64;
            assert!(
                (10 * n..=10 * n * (log2 + 1)).contains(&compared),
                "{n} features: {compared} comparisons, not from 10 N to 10 N (⌈log₂ N⌉ + 1) = {}",
                10 * n * (log2 + 1)
            );
            assert_eq!(looked_up, 11 * n, "{n} features");
            // An ID the document hasn't: none, in as few.
            let missing = FeatureId::new("zzzzzzzz").unwrap();
            let (before, _) = counted();
            assert!(model.outcome(&missing).is_none());
            assert!(model.feature(&missing).is_none());
            assert!(counted().0 - before <= 2 * (log2 + 1));
        }
    }

    #[test]
    fn each_unmeasured_solid_is_found_by_id_in_log_u_comparisons() {
        // Every third of 1,000 boxes (`b0000000`, `b0000003`, ...) can't be measured: 334
        // unmeasured, each said for its own feature, and no other. A lookup is a binary
        // search of their index, so at most ⌈log₂ 334⌉ + 1 = 10 comparisons whether it's
        // there or not, and at least 1: from 1,000 to 10,000 for the 1,000, one lookup
        // each. A scan compared about 280,000 (each of the 666 measured ones all 334, each
        // unmeasured one up to its place: 666 · 334 + 334 · 335 / 2 = 278,389).
        let n = 1000;
        let tall = |i: usize| i.is_multiple_of(3);
        let document = Arc::new(boxes(n, tall));
        let kernel = crate::fake::FakeKernel::new(2).unwrap();
        kernel.refuse(|op| match op {
            silico_protocol::Op::Summarize { shape } => crate::fake::FakeKernel::describe(shape)
                .filter(|made| made.contains("box(1, 1, 2)"))
                .map(|_| Error::new(ErrorCode::KernelFailed, "Summarize failed")),
            _ => None,
        });
        let model = published(Recompute::new(kernel).start(document));
        let (compared, looked_up) = counted();
        let said: Vec<Option<Error>> = model
            .features()
            .iter()
            .map(|(id, _)| model.unmeasured(id).cloned())
            .collect();
        let (compared, looked_up) = (counted().0 - compared, counted().1 - looked_up);
        for (i, ((id, outcome), said)) in model.features().iter().zip(&said).enumerate() {
            assert!(matches!(outcome, FeatureOutcome::Built { .. }), "{id}");
            match said {
                Some(error) => {
                    assert!(tall(i), "{id}: {error:?}");
                    assert_eq!(error.code(), ErrorCode::KernelFailed, "{id}");
                    assert_eq!(error.feature(), Some(id.as_str()));
                }
                None => assert!(!tall(i), "{id}"),
            }
        }
        assert!(
            (n as u64..=10 * n as u64).contains(&compared),
            "{compared} comparisons"
        );
        assert_eq!(looked_up, n as u64);
    }

    /// A box (`b0000000`) and `n` fillets, `f0000001` on, each of the one before, of one
    /// of the box's edges.
    fn fillets(n: usize) -> Checked {
        let mut features = vec![
            r#"{"id": "b0000000", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#.to_owned(),
        ];
        for i in 1..=n {
            let base = if i == 1 {
                "b0000000".to_owned()
            } else {
                format!("f{:07}", i - 1)
            };
            features.push(format!(
                r#"{{"id": "f{i:07}", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": ["b0000000.box.Edge{}"], "radius": 1}}"#,
                i % 12 + 1
            ));
        }
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
            features.join(", ")
        );
        Checked::new(crate::document::read(text.as_bytes()).unwrap()).unwrap()
    }

    #[test]
    fn reading_a_named_chains_maps_walks_one_feature_a_read_not_the_chain() {
        // #187's architecture review: `element_map` walked down every feature a feature is
        // made from, on every read, named or not, so on a chain of fillets each read cost
        // its depth. It goes no further than a feature named already, whose map needs
        // nothing it's made from. Once every map is named, a read finds its own feature
        // named and returns its map: one feature visited a read (`WALKED`, counted as each
        // is popped, or as the read returns early). A box and 500 fillets, each of the one
        // before: 501 reads, 501 visits.
        // The full walk visited each feature's depth + 1, 1 + 2 + ... + 501 = 125,751.
        let n = 500;
        let model = published(
            Recompute::new(crate::fake::FakeKernel::new(2).unwrap()).start(Arc::new(fillets(n))),
        );
        assert_eq!(model.features().len(), n + 1);
        assert!(model.errors().next().is_none());
        // The last fillet is made from every feature before it: reading its map names them
        // all, if the background hasn't yet.
        let (last, _) = model.features().last().unwrap();
        assert!(model.element_map(last).is_some());
        assert!(model.maps.iter().all(|slot| slot.get().is_some()));
        let before = walked();
        let maps: Vec<&ElementMap> = model
            .features()
            .iter()
            .map(|(id, _)| model.element_map(id).unwrap())
            .collect();
        assert_eq!(walked() - before, n as u64 + 1);
        // Each the map named for it.
        for ((id, _), map) in model.features().iter().zip(maps) {
            assert!(std::ptr::eq(map, model.named_element_map(id).unwrap()));
        }
    }

    #[test]
    fn labelling_each_step_finds_its_feature_in_log_n_comparisons() {
        // Recompute's progress labels: each step's is its first feature's ID and type, the
        // type found by the document's index, one lookup a step. N boxes, each a 1 mm cube
        // at x = its place: one Make step for them all (one box, one key), and a Translate
        // step for each but the first, at the origin, which isn't moved: N steps, N
        // lookups, from N to N (⌈log₂ N⌉ + 1) comparisons, 11,000 for N = 1,000. The
        // document's scan (`Document::feature`, as the labels found their features before)
        // compares every ID each, N²: 1,000,000.
        let n = 1000;
        let document = Arc::new(boxes(n, |_| false));
        let plan = Plan::of(&document, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(plan.first_feature.len(), n);
        let (compared, looked_up) = counted();
        let labels = plan.labels(&document);
        let (compared, looked_up) = (counted().0 - compared, counted().1 - looked_up);
        let most = n as u64 * (ceil_log2(n) + 1);
        assert!(
            (n as u64..=most).contains(&compared),
            "{compared} comparisons, past {most}"
        );
        assert_eq!(looked_up, n as u64);
        assert_eq!(labels.len(), n);
        for (key, id) in &plan.first_feature {
            assert_eq!(labels[key], format!("{id} box"));
        }
    }

    #[test]
    fn planning_reads_no_features_maker_until_a_name_asks_for_one() {
        // From M4's review: each plan built a map of every feature's maker, a copy of each
        // feature's ID and a read of its operation, though only a fillet's or a chamfer's
        // names are read with it. The planned document's index finds a maker when a name
        // asks: planning 1,000 boxes reads none, and asking for one reads one.
        let n = 1000;
        let document = Arc::new(boxes(n, |_| false));
        let (read, copied) = (crate::naming::makers_read(), crate::document::cloned());
        let plan = Plan::of(&document, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(crate::naming::makers_read() - read, 0);
        let copies = crate::document::cloned() - copied;
        // Each asked for by the document's index: one lookup, of at most ⌈log₂ N⌉ + 1
        // comparisons (11 for N = 1,000), found or not. A scan of the timeline compares
        // up to all N.
        let most = ceil_log2(n) + 1;
        let last = &document.document().features[n - 1].id;
        let before = counted();
        let found = plan.maker(last);
        let (compared, looked_up) = (counted().0 - before.0, counted().1 - before.1);
        assert_eq!(
            found,
            Some(Maker::Itself(crate::naming::PrimitiveKind::Box))
        );
        assert_eq!(looked_up, 1);
        assert!((1..=most).contains(&compared), "{compared} comparisons");
        assert_eq!(crate::naming::makers_read() - read, 1);
        let missing = FeatureId::new("zzzzzzzz").unwrap();
        let before = counted();
        assert_eq!(plan.maker(&missing), None);
        let (compared, looked_up) = (counted().0 - before.0, counted().1 - before.1);
        assert_eq!(looked_up, 1);
        assert!((1..=most).contains(&compared), "{compared} comparisons");
        // The plan copies each box's ID twice: for the box's own entry (`Planned`), and for
        // its step's first feature (N steps: one Make step for them all, first used by the
        // first box, and a Translate step for each of the others). A map of makers copied
        // each a third time.
        assert_eq!(copies, 2 * n as u64, "{copies} IDs copied");
    }

    #[test]
    fn parsing_a_name_of_each_feature_finds_its_maker_in_log_n_comparisons() {
        // `lookup` and `elements` read a name with `Model::maker` saying what each feature
        // it mentions is: found by the document's index. A name of each of N boxes
        // (`b0000000.box.Face1`, ...) mentions one feature: N lookups, from N to
        // N (⌈log₂ N⌉ + 1) comparisons, 11,000 for N = 1,000. A scan from the start
        // compares each feature's place + 1: N(N + 1) / 2, 500,500. A feature the document
        // hasn't is one lookup too, of at most ⌈log₂ N⌉ + 1 comparisons (11), and refused;
        // a scan compares all N.
        let n = 1000;
        let model = published(
            Recompute::new(crate::fake::FakeKernel::new(2).unwrap())
                .start(Arc::new(boxes(n, |_| false))),
        );
        let texts: Vec<String> = (0..n)
            .map(|i| format!("b{i:07}.box.Face{}", i % 6 + 1))
            .collect();
        let makers = |id: &FeatureId| model.maker(id);
        let (compared, looked_up) = counted();
        let names: Vec<crate::naming::Name> = texts
            .iter()
            .map(|text| crate::naming::Name::parse(text, &makers).unwrap())
            .collect();
        let (compared, looked_up) = (counted().0 - compared, counted().1 - looked_up);
        let most = n as u64 * (ceil_log2(n) + 1);
        assert!(
            (n as u64..=most).contains(&compared),
            "{compared} comparisons, past {most}"
        );
        assert_eq!(looked_up, n as u64);
        for (name, text) in names.iter().zip(&texts) {
            assert_eq!(name.to_string(), *text);
        }
        let (compared, looked_up) = counted();
        assert!(crate::naming::Name::parse("zzzzzzzz.box.Face1", &makers).is_err());
        let (compared, looked_up) = (counted().0 - compared, counted().1 - looked_up);
        assert!(
            (1..=ceil_log2(n) + 1).contains(&compared),
            "{compared} comparisons"
        );
        assert_eq!(looked_up, 1);
    }

    #[test]
    fn a_rebound_model_finds_each_feature_where_it_did() {
        // A model rebound to its saved document finds its features by the new document's
        // index: the same IDs in the same order, so each at its place, as before. The IDs
        // descend through the timeline, so a model or a save that sorted its features by ID
        // would put each elsewhere.
        let document = Arc::new(boxes_with(40, |i| format!("b{:07}", 39 - i), |_| false));
        let ids: Vec<&FeatureId> = document.document().features.iter().map(|f| &f.id).collect();
        assert!(ids.windows(2).all(|pair| pair[0] > pair[1]));
        let model = published(
            Recompute::new(crate::fake::FakeKernel::new(2).unwrap()).start(Arc::clone(&document)),
        );
        let saved = Arc::new(Checked::new(model.saved()).unwrap());
        assert_ne!(saved.document(), document.document());
        let rebound = model.rebound(Arc::clone(&saved));
        for (place, (id, outcome)) in rebound.features().iter().enumerate() {
            assert!(std::ptr::eq(rebound.outcome(id).unwrap(), outcome), "{id}");
            assert_eq!(rebound.outcome(id), model.outcome(id), "{id}");
            assert!(std::ptr::eq(
                rebound.feature(id).unwrap(),
                &saved.document().features[place]
            ));
            assert_eq!(rebound.element_map(id), model.element_map(id), "{id}");
        }
    }

    #[test]
    fn a_models_saved_copy_made_before_it_was_named_is_named_by_the_background_with_it() {
        // An edit's save publishes the model rebound to its saved document: a copy made as
        // the model is published, before the background has named any of its elements. The
        // background names the model the engine published, never the copy; a copy that
        // kept slots of its own read "not named yet" for good, and a pick on it by name
        // stayed pending (the app's, of a face, then dropped by the next rebuild as an
        // element gone). The copy is read only as a client that mustn't wait reads it,
        // never naming anything itself.
        let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap());
        engine.hold_naming();
        let model = published(engine.start(Arc::new(boxes(2, |_| false))));
        let saved = Arc::new(Checked::new(model.saved()).unwrap());
        let copy = model.rebound(saved);
        let ids: Vec<FeatureId> = copy.features().iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids.len(), 2);
        let pick = ["b0000001.box.Face1"];
        // Held: nothing named yet, of the model or its copy, and the pick pending.
        for id in &ids {
            assert!(model.named_so_far(id).is_none(), "{id}");
            assert!(copy.named_so_far(id).is_none(), "{id}");
            assert!(copy.named_element_map(id).is_none(), "{id}");
        }
        let resolved = crate::selection::resolve(&copy, &pick, "selection").unwrap();
        assert!(resolved.kept.is_empty(), "{resolved:?}");
        assert_eq!(resolved.pending.len(), 1, "{resolved:?}");
        // Released, the background names the model it published; the copy reads the same
        // maps, the very ones, without naming them itself.
        engine.release_naming();
        assert!(engine.wait_naming_ended(&model, Duration::from_secs(60)));
        for id in &ids {
            let named = model.named_element_map(id).unwrap();
            let read = copy.named_element_map(id).unwrap();
            assert!(std::ptr::eq(named, read), "{id}");
            assert!(std::ptr::eq(copy.named_so_far(id).unwrap().unwrap(), named));
        }
        // And the pick finds the face: the second box's own Face1.
        let resolved = crate::selection::resolve(&copy, &pick, "selection").unwrap();
        assert!(resolved.pending.is_empty(), "{resolved:?}");
        assert!(resolved.gone.is_empty(), "{resolved:?}");
        assert_eq!(resolved.kept.len(), 1, "{resolved:?}");
        let kept = &resolved.kept[0];
        assert_eq!(kept.feature, FeatureId::new("b0000001").unwrap());
        assert_eq!(kept.element, silico_protocol::ElementRef::face(1));
        assert_eq!(kept.kind(), silico_protocol::ElementKind::Face);
        assert_eq!(kept.name.to_string(), pick[0]);
    }

    #[test]
    fn a_models_unnamed_copy_shares_nothing_named_with_it() {
        // `unnamed` (and the test models made from it) gives a copy slots of its own: the
        // model named, the copy still has nothing named, and naming the copy names nothing
        // of a fresh copy of the model's; nor is its live bodies' index the model's.
        let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap());
        let model = published(engine.start(Arc::new(boxes(2, |_| false))));
        model.name_all(|| true);
        for (id, _) in model.features() {
            model.live_body(id);
        }
        assert!(model.live.get().is_some());
        let unnamed = model.unnamed();
        let other = model.unnamed();
        assert!(unnamed.live.get().is_none());
        for (id, _) in model.features() {
            assert!(model.named_element_map(id).is_some(), "{id}");
            assert!(unnamed.named_so_far(id).is_none(), "{id}");
        }
        unnamed.name_all(|| true);
        for (id, _) in model.features() {
            assert!(unnamed.named_element_map(id).is_some(), "{id}");
            assert!(other.named_so_far(id).is_none(), "{id}");
        }
    }

    #[test]
    fn a_models_unnamable_copy_lists_none_of_that_features_elements_whatever_the_model_listed() {
        // `unnamable` settles one feature's names as never made: its elements can't be
        // listed, the fault saying why, though the model it's copied from listed them
        // already; every other feature's are listed as the model's are.
        let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap());
        let model = published(engine.start(Arc::new(boxes(2, |_| false))));
        let ids: Vec<FeatureId> = model.features().iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids.len(), 2);
        for id in &ids {
            assert!(!model.entries(id).unwrap().unwrap().is_empty(), "{id}");
        }
        let unnamable = model.unnamable(&ids[0]);
        assert_eq!(unnamable.named_so_far(&ids[0]), Some(None));
        let Some(Err(fault)) = unnamable.entries(&ids[0]) else {
            panic!("{:?}", unnamable.entries(&ids[0]))
        };
        assert_eq!(fault.code(), ErrorCode::Internal);
        assert_eq!(fault.message(), "b0000000's elements couldn't be listed");
        assert_eq!(
            fault.cause(),
            Some("its solid built, but its elements couldn't be named")
        );
        assert_eq!(
            unnamable.entries(&ids[1]).unwrap().unwrap(),
            model.entries(&ids[1]).unwrap().unwrap()
        );
        // The model's own are as they were.
        assert!(!model.entries(&ids[0]).unwrap().unwrap().is_empty());
    }

    #[test]
    fn a_fresh_recompute_makes_its_profiles_and_names_anew_and_they_are_the_engines_after() {
        // `start_fresh` reads nothing kept: not the engine's profiles (made again from the
        // sketch solved again), nor the maps it has named (named again from the histories
        // it built again); once published, its own are the engine's, so the next recompute
        // takes them.
        let document =
            Arc::new(Checked::new(crate::document::read(PADDED.as_bytes()).unwrap()).unwrap());
        let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap())
            .with_sketches(crate::fake::FakeSketches::new());
        let pad = FeatureId::new("p0000001").unwrap();
        let block = FeatureId::new("k7f2qx3m").unwrap();
        let profiles = |engine: &Recompute<crate::fake::FakeKernel>| {
            let state = lock(&engine.engine.state);
            let profiles: Vec<Arc<Profile>> = state
                .profiles
                .values()
                .map(|profile| Arc::clone(profile.as_ref().unwrap()))
                .collect();
            profiles
        };
        let first = published(engine.start(Arc::clone(&document)));
        let (pad_map, block_map) = (
            first.element_map(&pad).unwrap() as *const ElementMap,
            first.element_map(&block).unwrap() as *const ElementMap,
        );
        let made = profiles(&engine);
        assert_eq!(made.len(), 1);
        // Read again: the same profile, the same maps.
        let again = published(engine.start(Arc::clone(&document)));
        assert!(Arc::ptr_eq(&again.known, &first.known));
        assert!(std::ptr::eq(again.element_map(&pad).unwrap(), pad_map));
        assert!(Arc::ptr_eq(&profiles(&engine)[0], &made[0]));
        // Fresh: another profile, other maps, the same names.
        let fresh = published(engine.start_fresh(Arc::clone(&document)));
        assert!(!Arc::ptr_eq(&fresh.known, &first.known));
        let fresh_profiles = profiles(&engine);
        assert_eq!(fresh_profiles.len(), 1);
        assert!(!Arc::ptr_eq(&fresh_profiles[0], &made[0]));
        for (id, before) in [(&pad, pad_map), (&block, block_map)] {
            let named = fresh.element_map(id).unwrap();
            assert!(!std::ptr::eq(named, before), "{id}");
            assert_eq!(named, first.element_map(id).unwrap(), "{id}");
        }
        // The engine's from here: the next recompute names with them.
        assert!(Arc::ptr_eq(&lock(&engine.engine.state).known, &fresh.known));
        let next = published(engine.start(Arc::clone(&document)));
        assert!(Arc::ptr_eq(&next.known, &fresh.known));
        assert!(std::ptr::eq(
            next.element_map(&pad).unwrap(),
            fresh.element_map(&pad).unwrap()
        ));
    }

    #[test]
    fn a_fresh_recompute_of_another_document_leaves_the_profiles_kept_for_the_one_before() {
        // Reading nothing kept isn't clearing it: the padded document's profile, made by a
        // recompute that reads what's kept, is the engine's still after a fresh recompute of
        // a box (the two published models' plans are kept), and the next recompute of the
        // padded document takes it, the same profile, not one made again.
        let padded =
            Arc::new(Checked::new(crate::document::read(PADDED.as_bytes()).unwrap()).unwrap());
        let block = Arc::new(
            Checked::new(
                crate::document::read(
                    br#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#,
                )
                .unwrap(),
            )
            .unwrap(),
        );
        let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap())
            .with_sketches(crate::fake::FakeSketches::new());
        let profile = |engine: &Recompute<crate::fake::FakeKernel>| {
            let state = lock(&engine.engine.state);
            let profiles: Vec<Arc<Profile>> = state
                .profiles
                .values()
                .map(|profile| Arc::clone(profile.as_ref().unwrap()))
                .collect();
            assert_eq!(profiles.len(), 1);
            Arc::clone(&profiles[0])
        };
        published(engine.start(Arc::clone(&padded)));
        let made = profile(&engine);
        published(engine.start_fresh(Arc::clone(&block)));
        assert!(Arc::ptr_eq(&profile(&engine), &made));
        published(engine.start(Arc::clone(&padded)));
        assert!(Arc::ptr_eq(&profile(&engine), &made));
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

    /// A solver that reads, during each solve, the number the engine gave it.
    struct Numbered {
        engine: Arc<std::sync::OnceLock<std::sync::Weak<Engine<crate::fake::FakeKernel>>>>,
        seen: Arc<Mutex<Vec<Option<u64>>>>,
    }

    impl SketchSolver for Numbered {
        fn solve(
            &mut self,
            system: silico_protocol::SketchSystem,
        ) -> Result<silico_protocol::Solved, Error> {
            let engine = self.engine.get().and_then(std::sync::Weak::upgrade);
            let number = engine.and_then(|e| lock(&e.state).solving.as_ref().map(|s| s.number));
            lock(&self.seen).push(number);
            Ok(crate::fake::FakeSketches::as_sent(&system))
        }

        fn stopper(&mut self) -> Result<Option<SolveStopper>, Error> {
            Ok(Some(SolveStopper::new(|| {})))
        }
    }

    #[test]
    fn each_solve_is_numbered_after_the_last() {
        // A late stop finds its solve by number (`stop_later`), so no two solves may share
        // one: the first solve is 1, the next 2 (M3's milestone mutants: `solves *= 1`
        // numbered every solve 0, which stored_coordinates.rs's late-stop race caught only
        // when the newer solve began within the older's grace).
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handle = Arc::new(std::sync::OnceLock::new());
        let engine =
            Recompute::new(crate::fake::FakeKernel::new(1).unwrap()).with_sketches(Numbered {
                engine: Arc::clone(&handle),
                seen: Arc::clone(&seen),
            });
        assert!(handle.set(Arc::downgrade(&engine.engine)).is_ok());
        for x in ["10", "11"] {
            let text = format!(
                r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{{"id": "pa000000", "x": 0, "y": 0, "construction": false}}, {{"id": "pb000000", "x": {x}, "y": 0, "construction": false}}], "lines": [{{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}}], "circles": [], "arcs": [], "constraints": [{{"id": "k0000001", "type": "horizontal", "line": "na000000"}}]}}]}}"#
            );
            let document = crate::document::read(text.as_bytes()).unwrap();
            let run = engine.start(crate::document::Checked::new(document).unwrap());
            assert!(matches!(
                run.wait_timeout(Duration::from_secs(20)),
                Ok(Ok(RunOutcome::Published(_)))
            ));
        }
        assert_eq!(*lock(&seen), [Some(1), Some(2)]);
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
        let measured: StepResult = Ok(Arc::new(Built::new(shape(), None, Some(summary.clone()))));
        let refused: StepResult = Ok(Arc::new(Built::refused_measure(shape(), None, refusal)));
        let unmeasured: StepResult = Ok(Arc::new(Built::new(shape(), None, None)));
        let failed = |code| -> StepResult { Err(Error::new(code, "the step failed")) };
        // And with what became of its description: a summarized result that knows its
        // description, or why it was refused, knows more than one whose describe failed or
        // wasn't asked, which knows as much as a summary alone.
        {
            let summarized = Built::new(shape(), None, Some(summary.clone()));
            let description = Description {
                faces: Vec::new(),
                edges: Vec::new(),
                vertices: Vec::new(),
            };
            let whole: StepResult = Ok(Arc::new(
                summarized.with_described(Described::Whole(description)),
            ));
            let kept: StepResult = Ok(Arc::new(summarized.with_described(Described::Refused(
                Error::new(ErrorCode::InvalidResult, "Describe failed"),
            ))));
            let lost: StepResult = Ok(Arc::new(summarized.with_described(Described::Failed(
                Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed"),
            ))));
            let hung: StepResult = Ok(Arc::new(summarized.with_described(Described::Refused(
                Error::new(ErrorCode::WorkerTimeout, "the kernel worker took too long"),
            ))));
            // By hand: what each knows, most first; a refusal or a hang kept never replaces
            // a description had (a superseded recompute's whole one, say).
            let ranked = [
                (&whole, 4),
                (&kept, 3),
                (&hung, 3),
                (&lost, 2),
                (&measured, 2),
                (&refused, 1),
                (&unmeasured, 0),
            ];
            for (old, knows) in ranked {
                for (new, coming) in ranked {
                    assert_eq!(
                        replaces(Some(old), new),
                        coming >= knows,
                        "{old:?} by {new:?}"
                    );
                }
            }
        }
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
    /// scheme's line (`silico naming 6`, M6.1's), the step's key, the ID, how it names (0 a
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
            "7989332e0f447b3c011256790f1b6dfad221db2c9bc0e1510836639cd6614002"
        );
        let cut = naming_key(
            made,
            &id("bbbbbbbb"),
            &Naming::Operation(OpCode::Cut),
            &[block],
        );
        assert_eq!(
            crate::hex(&cut.0),
            "e1846c56c574c3e45b71c739be881a60850e8b3949273993a188c9ca114799ab"
        );
    }

    /// A sweep feature's naming key, pinned as the others are (the milestone's mutants:
    /// no test held its arm, so its op code's byte could be anything): the scheme's line,
    /// the step's key, the ID, 1 + the sweep's op code (a pad's, 5, so 6), its faces' naming
    /// key, then the inputs'. Computed by hand with Python's hashlib from bytes chosen here
    /// (each key a byte repeated), not from any key the code makes.
    #[test]
    fn a_sweeps_naming_key_holds_its_op_code_and_its_faces_key() {
        let id = |text: &str| FeatureId::new(text).unwrap();
        let pad = Naming::Sweep(Arc::new(SweepNaming {
            operation: OpCode::Pad,
            sketch: id("ssssssss"),
            curves: Vec::new(),
            points: Vec::new(),
            regions: Vec::new(),
            faces: NamingKey([2; 32]),
        }));
        let key = naming_key(Key([1; 32]), &id("cccccccc"), &pad, &[NamingKey([3; 32])]);
        assert_eq!(
            crate::hex(&key.0),
            "69a5b2ab9b8eb0828cff4b38c8eff23dfd51309a33c6f9b06c767df822285b3c"
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

    /// An origin plane's faces line is its word, as before face frames, so no step's key
    /// moves (the cache's format bump re-keys every cache entry once); a face frame's is
    /// `frame` and its nine numbers' bits, origin, x, y, so two frames a bit apart are two
    /// steps, and a frame's −0 is its +0. Spelled out by hand from the doc.
    #[test]
    fn a_revolves_axis_on_a_face_frame_square_only_to_its_check_is_made_a_unit() {
        // A frame whose x and y are 3e-15 from square (inside the check's 16ε, 3.6e-15): x =
        // +X, y = (3e-15, 1, 0) to rounding. A line at 45° in the sketch, (√½, √½): x·l₀ +
        // y·l₁ = (√½(1 + 3e-15), √½, 0), whose length is √(1 + 3e-15 + …) ≈ 1 + 1.5e-15,
        // past 4ε (8.9e-16) from a unit, which the revolution's check refuses. Normalized,
        // it's within it, and the request is made.
        let y = silico_protocol::vec3::normalize([3e-15, 1.0, 0.0]);
        let frame = silico_protocol::FaceFrame {
            origin: [0.0, 0.0, 10.0],
            x: [1.0, 0.0, 0.0],
            y,
        };
        frame.check().unwrap();
        let plane = silico_protocol::SketchPlane::Face(frame);
        let half = std::f64::consts::FRAC_1_SQRT_2;
        let raw = [0, 1, 2].map(|k| frame.x[k] * half + y[k] * half);
        let raw_length = silico_protocol::vec3::norm(raw);
        assert!(
            (raw_length - 1.0).abs() > 4.0 * f64::EPSILON,
            "{raw_length}"
        );
        let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}, {"id": "r0000001", "type": "revolve", "v": 1, "name": "R", "sketch": "s0000001", "axis": "x_axis", "extent": {"type": "one_side", "angle": 90, "reversed": false}}]}"#;
        let document = crate::document::read(text.as_bytes()).unwrap();
        let step = sweep_step(
            &document.features[1],
            plane,
            Some(([0.0, 0.0], [half, half])),
            Key([0; 32]),
            None,
        )
        .unwrap();
        let Step::Revolution {
            origin,
            direction,
            start,
            angle,
            ..
        } = step
        else {
            panic!("{step:?}");
        };
        let length = silico_protocol::vec3::norm(direction);
        assert!((length - 1.0).abs() <= 4.0 * f64::EPSILON, "{length}");
        silico_protocol::check_revolution(origin, direction, start, angle).unwrap();
        assert!(silico_protocol::check_revolution(origin, raw, start, angle).is_err());
        // On an origin plane, the direction is the line's own, exactly.
        let Step::Revolution { direction, .. } = sweep_step(
            &document.features[1],
            silico_protocol::SketchPlane::Xy,
            Some(([0.0, 0.0], [half, half])),
            Key([0; 32]),
            None,
        )
        .unwrap() else {
            panic!();
        };
        assert_eq!(direction, [half, half, 0.0]);
    }

    #[test]
    fn a_faces_line_holds_its_plane_s_word_or_its_frame_s_bits() {
        use silico_protocol::{Curve, FaceFrame, Region, SketchPlane};
        let on = |plane| FaceRegions {
            plane,
            regions: vec![Region {
                outer: vec![Curve::Circle {
                    center: [0.0, -2.0],
                    radius: 1.0,
                }],
                holes: Vec::new(),
            }],
        };
        assert_eq!(
            faces_line(&on(SketchPlane::Xz)),
            "faces xz region loop circle 0000000000000000 c000000000000000 3ff0000000000000"
        );
        let top = FaceFrame {
            origin: [0.0, 0.0, 10.0],
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let line = "faces frame \
             0000000000000000 0000000000000000 4024000000000000 \
             3ff0000000000000 0000000000000000 0000000000000000 \
             0000000000000000 3ff0000000000000 0000000000000000 \
             region loop circle 0000000000000000 c000000000000000 3ff0000000000000";
        assert_eq!(faces_line(&on(SketchPlane::Face(top))), line);
        // Each of the nine a bit off: another line, another key.
        let key = |frame| Step::Faces(Arc::new(on(SketchPlane::Face(frame)))).key();
        for k in 0..9 {
            let mut off = top;
            let v = match k / 3 {
                0 => &mut off.origin,
                1 => &mut off.x,
                _ => &mut off.y,
            };
            v[k % 3] = f64::from_bits(v[k % 3].to_bits() + 1);
            assert_ne!(faces_line(&on(SketchPlane::Face(off))), line, "{k}");
            assert_ne!(key(off), key(top), "{k}");
        }
        // Each zero of the frame written −0 (8000000000000000 as sent): the same line, the
        // same key.
        for k in 0..9 {
            let mut negative = top;
            let v = match k / 3 {
                0 => &mut negative.origin,
                1 => &mut negative.x,
                _ => &mut negative.y,
            };
            if v[k % 3] != 0.0 {
                continue;
            }
            v[k % 3] = -0.0;
            assert_eq!(faces_line(&on(SketchPlane::Face(negative))), line, "{k}");
            assert_eq!(key(negative), key(top), "{k}");
        }
        let all = FaceFrame {
            origin: [-0.0, -0.0, 10.0],
            x: [1.0, -0.0, -0.0],
            y: [-0.0, 1.0, -0.0],
        };
        assert_eq!(faces_line(&on(SketchPlane::Face(all))), line);
        // A reversed top's x, −X: its sign in the bits (bff0…), another key than +X's.
        let reversed = FaceFrame {
            x: [-1.0, 0.0, 0.0],
            ..top
        };
        assert_eq!(
            faces_line(&on(SketchPlane::Face(reversed))),
            "faces frame \
             0000000000000000 0000000000000000 4024000000000000 \
             bff0000000000000 0000000000000000 0000000000000000 \
             0000000000000000 3ff0000000000000 0000000000000000 \
             region loop circle 0000000000000000 c000000000000000 3ff0000000000000"
        );
        assert_ne!(key(reversed), key(top));
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

    #[test]
    fn a_broken_outcome_is_marked_in_the_timeline_as_the_check_counts_it() {
        // From #265's robustness review: an outcome's errors that are kernel faults (Silico's
        // own bug) are no problems of the document, as the check sets them aside; one that's
        // all faults is unknown.
        use crate::timeline::{Excepted, Health, timeline};
        let engine = Recompute::new(crate::fake::FakeKernel::new(2).unwrap());
        let model = published(engine.start(Arc::new(boxes(2, |_| false))));
        let first = FeatureId::new("b0000000").unwrap();
        let bug = || Error::new(ErrorCode::Internal, "a base's elements couldn't be named");
        let broken = || Error::new(ErrorCode::ReferenceBroken, "Edge90 isn't there");
        for (errors, total, want) in [
            (vec![bug()], 1, (Health::Unknown, 0)),
            // One fault, one listed of the document's, and one unlisted.
            (vec![bug(), broken()], 3, (Health::Error, 2)),
        ] {
            let marked = model.with_outcome(&first, FeatureOutcome::Broken { errors, total });
            let got = timeline(marked.document(), Some(&marked), &Excepted::default());
            let e = &got.features[0];
            assert_eq!((e.health, e.problems), want);
            assert_eq!(
                got.features.iter().map(|e| e.problems).sum::<u64>(),
                crate::command::check(&marked).total()
            );
        }
    }

    /// Solvers whose starts are counted, each run by `start` with its number from 0, and
    /// wait while `gate` is shut.
    fn counted_starts(
        start: impl Fn(usize) -> Result<Box<dyn SketchSolver>, Error> + Send + 'static,
    ) -> (
        SketchSolvers,
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<Gate>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let starts = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new((Mutex::new(true), Condvar::new()));
        let (count, shut) = (Arc::clone(&starts), Arc::clone(&gate));
        let solvers = SketchSolvers::starting(move || {
            let n = count.fetch_add(1, Ordering::SeqCst);
            let mut closed = lock(&shut.0);
            while *closed {
                closed = shut.1.wait(closed).unwrap();
            }
            drop(closed);
            start(n)
        });
        (solvers, starts, gate)
    }

    /// Shut while its flag is set; opened once.
    type Gate = (Mutex<bool>, Condvar);

    fn open(gate: &Gate) {
        *lock(&gate.0) = false;
        gate.1.notify_all();
    }

    fn fake() -> Result<Box<dyn SketchSolver>, Error> {
        Ok(Box::new(crate::fake::FakeSketches::new()))
    }

    /// `solvers.take()` on a thread of its own, within ten seconds.
    fn taken(solvers: &SketchSolvers) -> Result<Box<dyn SketchSolver>, Error> {
        let solvers = solvers.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(solvers.take().map(|_| ()));
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("a take waited past ten seconds")
            .map(|()| -> Box<dyn SketchSolver> { Box::new(crate::fake::FakeSketches::new()) })
    }

    #[test]
    fn a_solve_while_a_warm_start_runs_takes_the_warmed_solver_and_starts_no_other() {
        use std::sync::atomic::Ordering;
        let (solvers, starts, gate) = counted_starts(|_| fake());
        // The warm start, on a thread of its own as an app runs it, held mid-start; a take
        // now waits for it, rather than starting its own (which the shut gate would hold
        // too, and count).
        let warmed = {
            let solvers = solvers.clone();
            std::thread::spawn(move || solvers.warm(1))
        };
        while starts.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
        let waiting = {
            let solvers = solvers.clone();
            std::thread::spawn(move || solvers.take().is_ok())
        };
        std::thread::sleep(Duration::from_millis(50));
        open(&gate);
        assert_eq!(warmed.join().unwrap(), 1);
        assert!(waiting.join().unwrap());
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_warm_start_counts_the_solvers_idle_and_starting() {
        use std::sync::atomic::Ordering;
        let (solvers, starts, gate) = counted_starts(|_| fake());
        open(&gate);
        solvers.give_back(fake().unwrap());
        // One idle: one more makes two (on a machine of two cores or more).
        assert_eq!(solvers.warm(2), 1);
        // Two idle or starting: none more for two, nor for one.
        assert_eq!(solvers.warm(2), 0);
        assert_eq!(solvers.warm(1), 0);
        assert_eq!(solvers.warm(0), 0);
        // Both taken without a start of their own.
        taken(&solvers).unwrap();
        taken(&solvers).unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_warm_start_starts_at_most_as_many_solvers_as_the_machine_has_cores() {
        // From the tests review: asked for three more than there are cores, it starts one
        // a core, and says so.
        use std::sync::atomic::Ordering;
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        let (solvers, starts, gate) = counted_starts(|_| fake());
        open(&gate);
        assert_eq!(solvers.warm(cores + 3), cores);
        assert_eq!(starts.load(Ordering::SeqCst), cores);
    }

    #[test]
    fn a_warm_start_that_fails_or_panics_leaves_the_solve_to_start_its_own() {
        use std::sync::atomic::Ordering;
        for panics in [false, true] {
            let (solvers, starts, gate) = counted_starts(move |n| {
                if n == 0 {
                    if panics {
                        panic!("the warm start's starter panicked");
                    }
                    return Err(Error::new(ErrorCode::WorkerCrashed, "no sketch worker"));
                }
                fake()
            });
            open(&gate);
            // Started none: failed, or its panic its caller's.
            let warmed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| solvers.warm(1)));
            assert_eq!(warmed.ok(), (!panics).then_some(0), "panics: {panics}");
            taken(&solvers).unwrap();
            assert_eq!(starts.load(Ordering::SeqCst), 2, "panics: {panics}");
        }
    }
}

#[cfg(test)]
mod checked_tests {
    use super::*;

    /// An M2 cut `kkkkkkkk` of feature 0 (1 mm³) by feature 1, built, its solid `measured`
    /// at `volume` or not.
    fn cut_to(
        volume: f64,
        measured: bool,
    ) -> (Planned, FeatureOutcome, Vec<(FeatureId, FeatureOutcome)>) {
        let fid = |text: &str| FeatureId::new(text).unwrap();
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
        let solid = |volume: f64, measured: bool| {
            Built::new(
                ShapeData::unchecked(silico_protocol::Blob::inline(vec![0])),
                None,
                measured.then(|| ShapeSummary {
                    volume,
                    ..summary.clone()
                }),
            )
        };
        let planned = Planned {
            id: fid("kkkkkkkk"),
            tools: Vec::new(),
            steps: vec![Key([2; 32])],
            uses: vec![0, 1],
            naming: None,
            naming_key: None,
            sketch: None,
            sweeps: None,
            on_face: OnFace::No,
            refused: None,
            check: Some(Check::Boolean {
                op: BooleanOp::Cut,
                base: fid("bbbbbbbb"),
                tool: fid("tttttttt"),
            }),
            suppressed: false,
        };
        let outcome = FeatureOutcome::Built {
            key: Key([2; 32]),
            built: Arc::new(solid(volume, measured)),
        };
        let inputs = vec![
            (
                fid("bbbbbbbb"),
                FeatureOutcome::Built {
                    key: Key([0; 32]),
                    built: Arc::new(solid(1.0, true)),
                },
            ),
            (
                fid("tttttttt"),
                FeatureOutcome::Built {
                    key: Key([1; 32]),
                    built: Arc::new(solid(1.0, true)),
                },
            ),
        ];
        (planned, outcome, inputs)
    }

    /// [`cut_to`] at half its base's volume.
    fn cut(measured: bool) -> (Planned, FeatureOutcome, Vec<(FeatureId, FeatureOutcome)>) {
        cut_to(0.5, measured)
    }

    #[test]
    fn an_m2_boolean_whose_measure_failed_saying_why_is_built() {
        let (planned, outcome, inputs) = cut(false);
        let mut unmeasured = BTreeMap::new();
        unmeasured.insert(
            Key([2; 32]),
            Error::new(ErrorCode::KernelFailed, "the cut couldn't be measured"),
        );
        let decided = checked(&planned, outcome, &inputs, &BTreeMap::new(), &unmeasured);
        assert!(
            matches!(decided, FeatureOutcome::Built { .. }),
            "{decided:?}"
        );
    }

    #[test]
    fn an_m2_boolean_with_no_measure_and_no_word_why_fails_as_silicos_fault() {
        let (planned, outcome, inputs) = cut(false);
        let decided = checked(
            &planned,
            outcome,
            &inputs,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        let FeatureOutcome::Failed(error) = decided else {
            panic!("{decided:?}");
        };
        assert_eq!(error.code(), ErrorCode::Internal, "{error:?}");
        assert_eq!(error.feature(), Some("kkkkkkkk"));
        assert_eq!(
            error.message(),
            "a feature's solid or its base's wasn't measured"
        );
    }

    #[test]
    fn an_m2_boolean_measured_is_judged_by_s7s_rule() {
        // Measured, its volume half its base's: it removes something, and is built.
        let (planned, outcome, inputs) = cut(true);
        let decided = checked(
            &planned,
            outcome,
            &inputs,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(
            matches!(decided, FeatureOutcome::Built { .. }),
            "{decided:?}"
        );
        // Measured, its volume its base's: it removes nothing, and is refused, naming it.
        let (planned, outcome, inputs) = cut_to(1.0, true);
        let decided = checked(
            &planned,
            outcome,
            &inputs,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        let FeatureOutcome::Failed(error) = decided else {
            panic!("{decided:?}");
        };
        assert_eq!(error.code(), ErrorCode::KernelFailed, "{error:?}");
        assert_eq!(error.feature(), Some("kkkkkkkk"));
        assert!(
            error
                .message()
                .starts_with("cut kkkkkkkk removes nothing from its base bbbbbbbb"),
            "{error:?}"
        );
    }
}
