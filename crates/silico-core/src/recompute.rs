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
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::cache::Cache;
use crate::document::{
    Checked, Document, ElementId, ElementName, Feature, FeatureId, FeatureIndex, Fingerprint, Made,
    Operation, SweepFeature,
};
use crate::elements::Entry;
use crate::kernel::{
    BooleanOp, BuiltData, DESCRIBE_BATCH, DescribedBatch, ElementKind, FaceRegions, Generation,
    History, Kernel, KernelRequest, Primitive, ShapeData, ShapeSummary, SweepKind,
};
use crate::naming::{ElementMap, Maker, NAMING_SCHEME, OpCode};
use crate::pool::{Outcome, Pending};
use crate::profile::Profile;
use crate::references::EdgeNames;
use crate::sketch::{SketchSolution, SketchSolver, SketchWorker, SolveStopper};
use crate::worker::lock;
use crate::{Error, ErrorCode};
use silico_protocol::Description;

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
    /// Refused for the shape's values, or hung ([`kept_undescribed`]): it would be again, so
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
}

impl FeatureOutcome {
    /// Why it failed, each error naming the feature: its failure, or its broken references
    /// (the first hundred); none if it built, is blocked on another, or is a sketch.
    pub fn errors(&self) -> &[Error] {
        match self {
            FeatureOutcome::Failed(error) => std::slice::from_ref(error),
            FeatureOutcome::Broken { errors, .. } => errors,
            FeatureOutcome::Built { .. }
            | FeatureOutcome::Blocked { .. }
            | FeatureOutcome::Sketch(_) => &[],
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
    /// named.
    maps: Vec<OnceLock<Option<Arc<ElementMap>>>>,
    /// Each feature's solid's elements as entries (`elements` and `query`'s), built from its
    /// description and its names the first time they're read, once for the model, so paging
    /// through a large solid costs one build and a copy of each page, not a build a page.
    entries: Vec<OnceLock<Result<Arc<[Entry]>, Error>>>,
    /// Each feature's live body, by index ([`Model::live_body`]): built the first time a
    /// name is looked for in every body, once for the model, from `bodies` and `naming`,
    /// which nothing changes once it's built.
    live: OnceLock<Vec<Option<usize>>>,
    /// The engine's maps named so far.
    known: NamedMaps,
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
    /// broke (its first hundred: [`Model::error_count`] counts them all). A pad, pocket,
    /// revolve, or groove refused for its sketch (no profile, several regions, an axis
    /// across it) has one error, naming it, whose elements are the sketch's at fault; the
    /// sketch, solved, has none (design S3, S5, S6).
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
            | FeatureOutcome::Sketch(_) => None,
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

    /// The document it was built from.
    pub fn document(&self) -> &Document {
        self.source.document()
    }

    /// This model as `source`'s: the same steps and outcomes, its document `source`. For a
    /// document that differs from the one it was built from only in what a save writes from
    /// it (its fingerprints, with no sketch's solve written), whose rebuild would take every
    /// step from those this one took: a session's save (a commit, an edit's, `format`'s),
    /// whose model is the saved revision's (the design's section 4).
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
            | FeatureOutcome::Sketch(_) => None,
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
        self.unmeasured
            .get(self.unmeasured_index.get(id)?)
            .map(|(_, error)| error)
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

    /// Its document as `silico fmt` writes it: each feature's fingerprint as it rebuilt,
    /// and each sketch as it solved (its points, circles' radii, and reference dimensions'
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

    /// Its document as a save writes it, each feature as `how` says ([`Save`]): the one
    /// place the rule is held. A session's commit saves the features its edits reached as
    /// rebuilt and every other as stored (decision 28); its `format` saves the reached ones
    /// as rebuilt, those whose drift it isn't told to accept as stored, and the rest as
    /// recorded (decision 31); [`saved`](Self::saved), every feature as recorded.
    pub fn saved_as(&self, how: impl Fn(&FeatureId) -> Save) -> Document {
        let mut document = self.document().clone();
        // The model's features are its document's, in order.
        for (feature, (_, outcome)) in document.features.iter_mut().zip(&self.features) {
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
            | FeatureOutcome::Blocked { .. } => None,
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
/// 10): which step finishes first depends on scheduling. Clones share the count.
#[derive(Clone, Default)]
pub struct StepCounter(Arc<Mutex<Counted>>);

/// A recompute's count of its steps, as [`StepCounter::now`] reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepCount {
    /// The steps settled: built, failed, kept from before, or never to run (an input
    /// failed, or only refused features need it). All of them once the recompute finishes.
    pub done: usize,
    /// The steps of its plan: none until its plan is made (after the sketches it sweeps are
    /// solved).
    pub steps: usize,
    /// The step started last of those running, as its feature and the feature's type
    /// (`f0000001 fillet`): none while no step runs.
    pub running: Option<String>,
    /// Whether the recompute has ended: finished, superseded, or failed.
    pub ended: bool,
}

#[derive(Default)]
struct Counted {
    count: StepCount,
    /// What each step is called, by key: its first feature and that feature's type.
    labels: BTreeMap<Key, String>,
    /// The steps running, in the order they started.
    running: Vec<Key>,
}

impl StepCounter {
    /// The count now.
    pub fn now(&self) -> StepCount {
        lock(&self.0).count.clone()
    }

    /// The plan made: `steps` steps, `done` of them settled already, each called as
    /// `labels` says.
    fn planned(&self, steps: usize, done: usize, labels: BTreeMap<Key, String>) {
        let mut counted = lock(&self.0);
        counted.count.steps = steps;
        counted.count.done = done;
        counted.labels = labels;
    }

    /// The step `key` started.
    fn started(&self, key: Key) {
        let mut counted = lock(&self.0);
        counted.running.push(key);
        counted.count.running = counted.labels.get(&key).cloned();
    }

    /// The step `key` settled: run (it was [`started`](Self::started)) or never to be.
    fn settled(&self, key: Key, ran: bool) {
        let mut counted = lock(&self.0);
        let Counted {
            count,
            labels,
            running,
        } = &mut *counted;
        count.done = (count.done + 1).min(count.steps);
        if ran {
            running.retain(|other| *other != key);
            count.running = running.last().and_then(|last| labels.get(last)).cloned();
        }
    }

    /// The recompute ended: every step settled, if it finished.
    fn ended(&self, finished: bool) {
        let mut counted = lock(&self.0);
        counted.running.clear();
        let count = &mut counted.count;
        if finished {
            count.done = count.steps;
        }
        count.running = None;
        count.ended = true;
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
    /// Where results are kept on disk, if anywhere.
    cache: Option<Cache>,
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
    /// The solvers given back, the last given back taken first. Taken alone, or after an
    /// engine's state or `start`, never before either.
    idle: Mutex<Vec<Box<dyn SketchSolver>>>,
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
            idle: Mutex::new(idle),
            start: Mutex::new(start),
        }))
    }

    /// An idle solver, or, with none idle, one started now. The idle list is looked at
    /// again once the start is this one's: a solver given back while another engine's start
    /// ran is taken, not one more started.
    fn take(&self) -> Result<Box<dyn SketchSolver>, Error> {
        if let Some(solver) = lock(&self.0.idle).pop() {
            return Ok(solver);
        }
        let mut start = lock(&self.0.start);
        if let Some(solver) = lock(&self.0.idle).pop() {
            return Ok(solver);
        }
        let started = (*start)();
        if started.is_ok() {
            tracing::debug!("started a sketch solver");
        }
        started
    }

    /// `solver`, idle again.
    fn give_back(&self, solver: Box<dyn SketchSolver>) {
        lock(&self.0.idle).push(solver);
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
type SketchResult = Result<Arc<SketchSolution>, Error>;
/// A recompute's sketches' results, by key.
type SketchResults = BTreeMap<Key, SketchResult>;

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
            SketchOutcome::SolverFailed(error) => Err(error),
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
                cache,
                sketch_turn: Condvar::new(),
                solvers: Mutex::new(SketchSolvers::workers()),
                state: Mutex::new(EngineState {
                    generation,
                    newest_started: generation,
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

    /// The model the newest finished recompute published, if any has.
    pub fn published(&self) -> Option<Arc<Model>> {
        lock(&self.engine.state).published.clone()
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
        let progress = StepCounter::default();
        let (generation, later) = {
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
                let outcome = engine.run(&document, generation, &counting, reads);
                counting.ended(matches!(outcome, Ok(RunOutcome::Published(_))));
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
    /// Each step's features (those whose own steps hold it), by index: a step is run while
    /// one of them may still be built.
    owners: BTreeMap<Key, Vec<usize>>,
    /// The features whose checks a step's result (or measure) can decide, by the step's
    /// key: those whose solid it is, and those whose base's solid it is.
    judged_by: BTreeMap<Key, Vec<usize>>,
    /// Each feature's users, by index: those whose base, tool, or sketch it is.
    dependants: Vec<Vec<usize>>,
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
    /// What its result is checked for, once built ([`Check`]).
    check: Option<Check>,
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
            owners: BTreeMap::new(),
            judged_by: BTreeMap::new(),
            dependants: Vec::new(),
        };
        for feature in features {
            let mut own = Vec::new();
            let mut uses = Vec::new();
            let mut refused = None;
            let mut sweeps = None;
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
                        let input = |id: &FeatureId| document.place(id);
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
                    (own, uses) = plan.dress(document, feature, dressing);
                    Some(Naming::Operation(OpCode::Fillet))
                }
                Made::Chamfer { distance } => {
                    let dressing = Dressing::Chamfer { distance };
                    (own, uses) = plan.dress(document, feature, dressing);
                    Some(Naming::Operation(OpCode::Chamfer))
                }
                Made::Sweep(kind) => {
                    uses = feature
                        .operation
                        .inputs()
                        .iter()
                        .filter_map(|(_, base)| document.place(base))
                        .collect();
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
            // Which features each step serves, which checks its result decides, and what
            // uses what.
            let index = plan.features.len() - 1;
            plan.dependants.push(Vec::new());
            if let Some(planned) = plan.features.last() {
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
                    for key in planned.key().into_iter().chain(base) {
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
        let built = match done.get(&planned.key()?) {
            Some(Ok(built)) => built,
            _ => return None,
        };
        match check {
            Check::Boolean { .. } => {
                Some(built.history.as_ref().map_or(1, |h| h.result.solids) <= 1)
            }
            Check::Sweep(sweep) => {
                let base = planned
                    .uses
                    .first()
                    .and_then(|&b| self.features.get(b))
                    .and_then(Planned::key)?;
                let before = match done.get(&base) {
                    Some(Ok(base)) => measured_of(base)?,
                    _ => return None,
                };
                Some(crate::sweep::result_holds(
                    sweep.kind,
                    &before,
                    &measured_of(built)?,
                ))
            }
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

    /// Adds to `refusals` the features `key`'s result (or measure) refuses, now that `done`
    /// holds it, and every feature made from one.
    fn judge(&self, key: &Key, done: &BTreeMap<Key, StepResult>, refusals: &mut Refusals) {
        for &f in self.judged_by.get(key).into_iter().flatten() {
            if refusals.refused.contains(&f) || self.verdict(f, done) != Some(false) {
                continue;
            }
            refusals.refused.insert(f);
            let mut stack = vec![f];
            while let Some(i) = stack.pop() {
                if refusals.dead.insert(i) {
                    stack.extend(self.dependants.get(i).into_iter().flatten().copied());
                }
            }
        }
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

    /// The step of a fillet or chamfer `feature` (the next feature planned), of its base:
    /// its steps, and the features it uses.
    fn dress(
        &mut self,
        document: &Checked,
        feature: &crate::document::Feature,
        dressing: Dressing,
    ) -> (Vec<Key>, Vec<usize>) {
        // The gate has checked that the base is an earlier feature, so it's planned.
        let Some(b) = feature
            .operation
            .inputs()
            .first()
            .and_then(|(_, base)| document.place(base))
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
    /// Its summary, its description not had (not yet asked, or failed).
    Summary,
    /// Its summary, and its description or why it was refused.
    Described,
}

/// How much `built` knows of its measure.
fn measure_known(built: &Built) -> MeasureKnown {
    match (built.summary(), built.described(), built.measure_refused()) {
        (Some(_), Some(Described::Whole(_) | Described::Refused(_)), _) => MeasureKnown::Described,
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

/// Why a description that hung is kept, and that it may not hang again: the advice a read of
/// the solid's elements words as `check` with `fresh` to try it again.
const HUNG_KEPT: &str = "describing this solid hung, which a busy machine can cause as well \
     as the solid; the session keeps that rather than wait on it at every rebuild";

/// Whether a solid's description that couldn't be had is kept for the session, beside its
/// summary, and not asked again until something changes its step (a fresh rebuild asks
/// again): refused for its values, as [`kept`] judges, or hung. A describe that hangs would
/// hang again on the same solid, and each rebuild asking it again would hold a worker for
/// the kernel's timeout (a viewport reading every rebuild would ask it every time). Not on
/// disk: a hang isn't a fact about the shape (`cache::lasting`), so the next session asks
/// once again. A crash, or a reply no shape has, is asked again, as before.
fn kept_undescribed(error: &Error) -> bool {
    // (`measure` keeps a hang with [`HUNG_KEPT`] as its advice.)
    kept(error) || error.code() == ErrorCode::WorkerTimeout
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
        let first = match self.solve_swept(document, generation, reads)? {
            Some(first) => first,
            None => return Ok(RunOutcome::Superseded),
        };
        let plan = Plan::of(document, &first.swept);
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
            // The sketches solved before the plan, and their profiles, kept now that the
            // plan's keys hold them.
            state.profiles.extend(first.profiles.iter().cloned());
            // Reading nothing kept, every step is built and every sketch solved.
            let (done, mut solutions): (BTreeMap<Key, StepResult>, SketchResults) = if kept {
                (
                    plan.order
                        .iter()
                        .filter_map(|key| state.results.get(key).map(|r| (*key, r.clone())))
                        .collect(),
                    plan.sketches
                        .iter()
                        .filter_map(|(_, key)| state.solutions.get(key).map(|r| (*key, r.clone())))
                        .collect(),
                )
            } else {
                (BTreeMap::new(), BTreeMap::new())
            };
            solutions.extend(first.solutions.iter().map(|(k, r)| (*k, r.clone())));
            (done, solutions)
        };
        self.keep_solutions(&first.fresh);
        // The sketches not solved before, solved on a thread of their own.
        let to_solve: Vec<(FeatureId, Key)> = plan
            .sketches
            .iter()
            .filter(|(_, key)| !solutions.contains_key(key))
            .cloned()
            .collect();
        // What's kept on disk for this kernel, for the steps not kept in memory.
        if let Some(cache) = self.cache.as_ref().filter(|_| kept) {
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
        // described yet, unless the kernel refused to for their values: it would again.
        for key in &plan.results {
            if let Some(Ok(built)) = schedule.done.get(key) {
                if built.needs_measure() {
                    schedule.ready.push_back(Job::Measure(*key));
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
            .count();
        progress.planned(plan.order.len(), settled, plan.labels(document));
        let schedule = Mutex::new(schedule);
        let changed = Condvar::new();
        let threads = to_run.min(MOST_AT_ONCE);
        // The maps its features are named with: those named before, or, reading nothing
        // kept, none, each named again from what this recompute built (and, once its model
        // is published, the engine's).
        let known: NamedMaps = if kept {
            Arc::clone(&lock(&self.state).known)
        } else {
            NamedMaps::default()
        };
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
                    .map_err(|e| crate::worker::thread_error("recompute", &e))?;
                Some(spawned)
            };
            start_threads(threads, |n| {
                std::thread::Builder::new()
                    .name(format!("recompute-{}-{n}", generation.0))
                    .spawn_scoped(scope, || {
                        let _entered = parent.enter();
                        self.serve(&plan, &schedule, &changed, generation, progress, &known);
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
        let model = Arc::new(model(
            document,
            &plan,
            &schedule,
            &solutions,
            generation,
            Arc::clone(&known),
        ));
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
        reads: Reads,
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
            solutions.extend(solved.iter().map(|(key, s)| (*key, s.clone().result())));
            fresh = solved;
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
                for (id, key) in to_solve {
                    solved.insert(*key, solver_failure(error.clone(), id));
                }
                return Some(solved);
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
        progress: &StepCounter,
        known: &NamedMaps,
    ) {
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
                    // A step only refused features' users need isn't started: they're
                    // reported blocked, whatever ran.
                    if let Job::Make(key) = job
                        && !plan.needed(&key, &s.refusals)
                    {
                        progress.settled(key, false);
                        continue;
                    }
                    // A fillet's or chamfer's base is built: what its elements are named
                    // from. Or a solid that isn't built yet, which it waits for (and, if it
                    // failed, waits for forever: the failure blocks it).
                    let dress = match job {
                        Job::Make(key) => plan.dresses.get(&key),
                        Job::Measure(_) => None,
                    };
                    let sources = match dress.map(|d| plan.sources(d.base, &s.done, known)) {
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
                if let Job::Make(key) = job {
                    progress.started(key);
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
                let ran = match job {
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
                progress.settled(key, true);
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
                        plan.judge(&key, done, refusals);
                        // A feature's solid is measured once it's made, by a job of its own:
                        // what uses it doesn't wait for that.
                        // This thread takes a job itself next (it has nothing to write: a
                        // feature's solid is written once measured), so the measure wakes no
                        // other.
                        if plan.results.contains(&key) {
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
                    // The checks this measure decides (its feature's, and those whose base it
                    // is): what only refused features need is no longer started.
                    let Schedule { done, refusals, .. } = &mut *s;
                    plan.judge(&key, done, refusals);
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
            Outcome::Done(Err(error)) if error.code() == ErrorCode::WorkerTimeout => {
                Described::Refused(error.with_advice(crate::worker::Advice::TryAgain {
                    once_more: false,
                    why: HUNG_KEPT.to_owned(),
                }))
            }
            Outcome::Done(Err(error)) if kept_undescribed(&error) => Described::Refused(error),
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
    /// description up: the batches still in flight are cancelled ([`Pending::cancel`]), so no
    /// worker is spent on what won't be used.
    fn describe(
        &self,
        shape: &ShapeData,
        summary: &ShapeSummary,
        generation: Generation,
    ) -> Outcome<Description> {
        let all = u64::from(summary.faces) + u64::from(summary.edges) + u64::from(summary.vertices);
        if all <= u64::from(DESCRIBE_BATCH) {
            let all = match self
                .kernel
                .submit(generation, KernelRequest::describe_all(shape))
                .wait()
            {
                Outcome::Done(Ok(all)) => all,
                Outcome::Done(Err(error)) => return Outcome::Done(Err(error)),
                Outcome::Superseded => return Outcome::Superseded,
            };
            let totals = [
                (ElementKind::Face, all.faces),
                (ElementKind::Edge, all.edges),
                (ElementKind::Vertex, all.vertices),
            ];
            if let Some(fault) = totals
                .into_iter()
                .find_map(|(kind, total)| crate::kernel::total_fault(summary, kind, total))
            {
                return Outcome::Done(Err(fault));
            }
            return match crate::kernel::description_fault(summary, &all.description) {
                None => Outcome::Done(Ok(all.description)),
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
        let mut in_flight = VecDeque::new();
        // The description given up: every batch still in flight cancelled.
        let given_up = |in_flight: VecDeque<(ElementKind, Pending<DescribedBatch>)>, error| {
            for (_, pending) in in_flight {
                pending.cancel();
            }
            Outcome::Done(Err(error))
        };
        loop {
            while in_flight.len() < at_once
                && let Some((kind, first, count)) = planned.next()
            {
                let request = match KernelRequest::describe(shape, kind, first, count) {
                    Ok(request) => request,
                    Err(error) => return given_up(in_flight, error),
                };
                in_flight.push_back((kind, self.kernel.submit(generation, request)));
            }
            let Some((kind, pending)) = in_flight.pop_front() else {
                break;
            };
            match pending.wait() {
                Outcome::Done(Ok(batch)) => {
                    if let Some(fault) = crate::kernel::total_fault(summary, kind, batch.total) {
                        return given_up(in_flight, fault);
                    }
                    description.faces.extend(batch.description.faces);
                    description.edges.extend(batch.description.edges);
                    description.vertices.extend(batch.description.vertices);
                }
                Outcome::Done(Err(error)) => return given_up(in_flight, error),
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
            (None, None) => match &planned.refused {
                Some(error) => FeatureOutcome::Failed(error.clone()),
                None => checked(
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
        .collect::<Vec<_>>();
    let naming = naming(plan, &features, done);
    let maps = (0..features.len()).map(|_| OnceLock::new()).collect();
    let entries = (0..features.len()).map(|_| OnceLock::new()).collect();
    let unmeasured_index = Arc::new(FeatureIndex::new(unmeasured.iter().map(|(id, _)| id)));
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
        live: OnceLock::new(),
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
    for (planned, (_, outcome)) in plan.features.iter().zip(features) {
        let (FeatureOutcome::Built { .. }, Some(how)) = (outcome, planned.naming.as_ref()) else {
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
impl Model {
    /// This model with feature `id`'s solid measured as `summary` says, if it built: a
    /// test's stand-in for measures past what a fingerprint holds, which no solid the gate
    /// admits has (a kilometre each way is 1e18 mm³, and a volume holds to 1.7e32).
    pub(crate) fn measured_as(&self, id: &FeatureId, summary: ShapeSummary) -> Model {
        let mut model = self.clone();
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
        let document = boxes(n, |_| false);
        let plan = Plan::of(&document, &BTreeMap::new());
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
            // By hand: what each knows, most first.
            let ranked = [
                (&whole, 3),
                (&kept, 3),
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
            "d50b3f99aaaedeaf675e75fce819ff74fe1a54adae4324b0dee633af355362f6"
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
