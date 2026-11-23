//! Suppression (docs/design/m5.0-app.md, section 7's `suppress_feature`, A10, and section
//! 12's items 3 and 4; docs/ports/suppression.md). A suppressed feature isn't built; a
//! feature whose `base` or `tool` names one builds on that input's **effective base**, the
//! nearest unsuppressed feature up its chain by `base` links ([`standing`]), as FreeCAD's
//! PartDesign passes a suppressed feature's base through. Suppression never rewrites a
//! reference: the text's links stay as written.
//!
//! As SolidWorks does it (docs/ux/solidworks.md, §5, *Editing upstream*), a feature's
//! dependents go with it, and unsuppressing a feature unsuppresses what it needs. A
//! **dependent** of a suppressed feature `X` ([`suppressed_parent`]) is:
//!
//! - a feature whose `sketch` is `X`;
//! - a feature that stores an element name one of whose makers is `X` (a fillet's or
//!   chamfer's `edges`: every feature its parsed name holds), or one of whose makers reaches
//!   the feature's base only as the `tool` of `X`, a boolean (a fuse keeps its tool's
//!   elements under the tool's names, and `X`, suppressed, passes only its base through);
//! - a feature one of whose inputs it needs has no unsuppressed feature up its chain. A
//!   pad's or revolve's base is optional, so with its whole chain suppressed it builds
//!   alone; every other input is needed ([`needs`]).
//!
//! The gate refuses a document in which a dependent of a suppressed feature is unsuppressed,
//! by the same rule; [`suppress`] and [`unsuppress`] edit a document the gate took into
//! another it takes. Every rule is a function of the text.

use std::collections::{BTreeMap, BTreeSet};

use super::{Checked, Document, Feature, FeatureId, NameField, Operation};
use crate::naming::{Maker, Name};
use crate::{Error, ErrorCode};

/// Why a feature that isn't suppressed would have to be: what it can't do without, among the
/// suppressed features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cause {
    /// Its sketch is suppressed.
    Sketch,
    /// A name it stores has a suppressed maker; which of its names, by where it's stored
    /// (`edges.0`, `plane.face`).
    Names(NameField),
    /// A name it stores (by where it's stored) has a maker (by its place in the timeline)
    /// that reaches the feature's base only as a suppressed boolean's tool.
    Lost { field: NameField, maker: usize },
    /// An input it needs (`base` or `tool`) has no unsuppressed feature up its chain.
    Chain(&'static str),
    /// An original it copies is suppressed (docs/design/m6.0-patterns.md, section 6): a
    /// pattern is a child of each original, as SolidWorks' is; which, by its place in
    /// `copies.features`.
    Copies(usize),
}

/// The place, in `features`, of the feature whose solid stands for each feature's: itself,
/// for a feature that makes a solid and isn't suppressed; for a suppressed one, what stands
/// for its base, or none if it has none; none for a sketch. So the solid a feature's input
/// `X` names is that of `standing[X]`, its effective base. `place` finds a feature's place
/// by ID, and only one before the feature asking (a document the gate refused may hold a
/// reference forward, which stands for nothing).
pub(crate) fn standing(
    features: &[Feature],
    place: &dyn Fn(&FeatureId) -> Option<usize>,
) -> Vec<Option<usize>> {
    let mut standing: Vec<Option<usize>> = Vec::with_capacity(features.len());
    for (at, feature) in features.iter().enumerate() {
        let stands = if !feature.operation.makes_solid() {
            None
        } else if !feature.suppressed {
            Some(at)
        } else {
            feature
                .operation
                .base()
                .and_then(place)
                .filter(|&base| base < at)
                .and_then(|base| standing.get(base).copied().flatten())
        };
        standing.push(stands);
    }
    standing
}

/// The solids `features` consume, by ID: for each feature `consumes` takes that isn't
/// suppressed, what stands for each of its inputs ([`standing`]), none where its whole chain
/// is suppressed; an input naming no feature before it (in a document the gate refused) is
/// taken as named. What a document's bodies are judged by: a solid none consumes is one.
pub(crate) fn consumed<'a>(
    features: &'a [Feature],
    consumes: &dyn Fn(&Feature) -> bool,
) -> BTreeSet<&'a FeatureId> {
    let places = first_places(features);
    let place = |id: &FeatureId| places.get(id).copied();
    let standing = standing(features, &place);
    let mut consumed = BTreeSet::new();
    for (at, feature) in features.iter().enumerate() {
        if feature.suppressed || !consumes(feature) {
            continue;
        }
        for (_, input) in feature.operation.inputs() {
            match place(input).filter(|&p| p < at) {
                Some(p) => consumed.extend(
                    standing
                        .get(p)
                        .copied()
                        .flatten()
                        .and_then(|s| features.get(s))
                        .map(|f| &f.id),
                ),
                None => {
                    consumed.insert(input);
                }
            }
        }
    }
    consumed
}

/// A document's features indexed as its text links them: each ID's first feature's place
/// (as the gate finds a reference), and each solid's first consumer, by place, with the field
/// that names it. Built once, so each question is a search, not a scan.
pub(crate) struct Links<'a> {
    places: BTreeMap<&'a FeatureId, usize>,
    consumers: Vec<Option<(usize, &'static str)>>,
}

impl<'a> Links<'a> {
    pub(crate) fn of(features: &'a [Feature]) -> Self {
        let places = first_places(features);
        let mut consumers: Vec<Option<(usize, &'static str)>> = vec![None; features.len()];
        for (at, feature) in features.iter().enumerate() {
            for (field, input) in feature.operation.inputs() {
                if let Some(slot) = places
                    .get(input)
                    .copied()
                    .filter(|&p| p < at)
                    .and_then(|p| consumers.get_mut(p))
                    && slot.is_none()
                {
                    *slot = Some((at, field));
                }
            }
        }
        Links { places, consumers }
    }

    /// The place of `id`'s first feature.
    pub(crate) fn place(&self, id: &FeatureId) -> Option<usize> {
        self.places.get(id).copied()
    }

    /// The feature that consumes the solid at `at`, by place, and the field naming it.
    fn consumer(&self, at: usize) -> Option<(usize, &'static str)> {
        self.consumers.get(at).copied().flatten()
    }
}

/// Each solid's way down its consumers, indexed so the question [`lost`] asks of a stored
/// name, whether its maker's way down to a feature's base passes a suppressed boolean that
/// consumes it as its `tool`, is answered without walking the way (issue #306: a walk step by
/// step is quadratic on a long chain).
///
/// Each solid has at most one consumer ([`Links`]), after it, so the consumer links make a
/// forest, a consumer the parent of what it consumes, and a solid's way down is its path to
/// its root. Two things answer the question:
///
/// - each place's span in a preorder of the forest (`enter`, `size`): a base lies on a maker's
///   way iff the maker's place in the order lies in the base's span;
/// - a union-find along the ways (`skip`): each place whose consumer passes its elements on,
///   decided so, points further down its way, so the first place from a maker that isn't
///   known to pass them on (one a suppressed boolean consumes as its tool, one undecided, or
///   a root) is found in near-constant time.
///
/// Whether a place passes its elements on is told as its consumer's suppression is settled
/// ([`settle`](Self::settle)): in timeline order, as [`dependents`] settles each feature, or
/// all at once and then each unsuppression, as [`unsuppress`] does.
pub(crate) struct ConsumerForest {
    /// Each place's first position in the preorder.
    enter: Vec<usize>,
    /// How many places each place's subtree holds, itself among them: its span's length.
    size: Vec<usize>,
    /// Each place's step down its way: itself where it may drop its elements there or is
    /// undecided, or else a place further down, each place between passing them on.
    skip: Vec<usize>,
}

impl ConsumerForest {
    /// The forest `links` gives, no place's passing decided.
    pub(crate) fn of(links: &Links) -> Self {
        let n = links.consumers.len();
        // Each consumer comes after what it consumes, so a subtree's places all come before
        // its root: sizes summed up in timeline order, and spans handed down in reverse.
        let mut size = vec![1_usize; n];
        for at in 0..n {
            if let Some((parent, _)) = links.consumer(at)
                && let Some(&own) = size.get(at)
                && let Some(slot) = size.get_mut(parent)
            {
                *slot += own;
            }
        }
        let mut enter = vec![0_usize; n];
        // The next free position in each place's span, past itself.
        let mut free = vec![0_usize; n];
        let mut roots = 0;
        for (at, &own) in size.iter().enumerate().rev() {
            // Its parent's next free position, or the roots' for a root.
            let next = match links.consumer(at) {
                Some((parent, _)) => free.get_mut(parent),
                None => Some(&mut roots),
            };
            if let Some(next) = next {
                let start = *next;
                *next += own;
                if let Some(slot) = enter.get_mut(at) {
                    *slot = start;
                }
                if let Some(slot) = free.get_mut(at) {
                    *slot = start + 1;
                }
            }
        }
        ConsumerForest {
            enter,
            size,
            skip: (0..n).collect(),
        }
    }

    /// The forest `links` gives, each place's passing decided by whether its consumer is
    /// suppressed.
    pub(crate) fn settled(
        features: &[Feature],
        links: &Links,
        suppressed: &dyn Fn(usize) -> bool,
    ) -> Self {
        let mut forest = ConsumerForest::of(links);
        for at in 0..features.len() {
            forest.settle(features, links, at, suppressed(at));
        }
        forest
    }

    /// The feature at `at` is settled as `suppressed` says: each solid it consumes (as its
    /// first consumer) passes its elements on down its way, but the one it consumes as its
    /// `tool` if it's suppressed, which passes only its base through. A place told it passes
    /// them on stays so, so an unsuppression is told by settling again.
    pub(crate) fn settle(
        &mut self,
        features: &[Feature],
        links: &Links,
        at: usize,
        suppressed: bool,
    ) {
        let Some(feature) = features.get(at) else {
            return;
        };
        for (field, input) in feature.operation.inputs() {
            if suppressed && field == "tool" {
                continue;
            }
            // Its consumer is `at`, so it comes before `at`.
            if let Some(p) = links.place(input)
                && links.consumer(p) == Some((at, field))
                && let Some(slot) = self.skip.get_mut(p)
                && *slot == p
            {
                *slot = at;
            }
        }
    }

    /// Whether `base` lies on the way down from `maker`, past `maker` itself: a maker that is
    /// the base itself loses nothing on the way, as [`lost`] would find of it too (its first
    /// stop is itself, the base, or past it).
    fn on_way(&self, maker: usize, base: usize) -> bool {
        match (
            self.enter.get(maker),
            self.enter.get(base),
            self.size.get(base),
        ) {
            (Some(&m), Some(&b), Some(&size)) => b < m && m < b + size,
            _ => false,
        }
    }

    /// The first place from `at` down its way not known to pass its elements on, the paths
    /// it went halved.
    fn stop(&mut self, mut at: usize) -> usize {
        while let Some(&next) = self.skip.get(at)
            && next != at
        {
            let further = self.skip.get(next).copied().unwrap_or(next);
            if let Some(slot) = self.skip.get_mut(at) {
                *slot = further;
            }
            at = further;
        }
        at
    }
}

/// Whether any of `features` that `suppressed` says is suppressed consumes a `tool`: only
/// then can a stored name be lost to a suppression ([`Cause::Lost`]), so only then is it
/// looked for.
pub(crate) fn any_tool_suppressed(
    features: &[Feature],
    suppressed: &dyn Fn(usize) -> bool,
) -> bool {
    features
        .iter()
        .enumerate()
        .any(|(at, f)| suppressed(at) && has_tool(&f.operation))
}

fn has_tool(operation: &Operation) -> bool {
    operation.inputs().iter().any(|(field, _)| *field == "tool")
}

/// Each ID's first feature's place.
fn first_places(features: &[Feature]) -> BTreeMap<&FeatureId, usize> {
    let mut places = BTreeMap::new();
    for (at, feature) in features.iter().enumerate() {
        places.entry(&feature.id).or_insert(at);
    }
    places
}

/// The feature at `at`'s inputs as it builds on them: each of its `base` and `tool` with the
/// place of the feature whose solid stands for it ([`standing`]), none where nothing does.
pub(crate) fn effective_inputs(
    features: &[Feature],
    standing: &[Option<usize>],
    place: &dyn Fn(&FeatureId) -> Option<usize>,
    at: usize,
) -> Vec<(&'static str, Option<usize>)> {
    let Some(feature) = features.get(at) else {
        return Vec::new();
    };
    feature
        .operation
        .inputs()
        .into_iter()
        .map(|(field, input)| {
            let stands = place(input)
                .filter(|&p| p < at)
                .and_then(|p| standing.get(p).copied().flatten());
            (field, stands)
        })
        .collect()
}

/// Whether `operation` needs its input `field` to build: every input but a pad's or
/// revolve's base, which can begin a body.
pub(crate) fn needs(operation: &Operation, field: &str) -> bool {
    !(field == "base" && matches!(operation, Operation::Pad { .. } | Operation::Revolve { .. }))
}

/// The suppressed feature the feature at `at` is a dependent of, if it's one, by its place
/// and why, the first in the order the feature's fields come: its sketch, its stored names
/// (a suppressed maker, then a maker lost to a suppressed boolean, looked for only where
/// `forest` is given, as it is where one may be: [`any_tool_suppressed`]), its inputs.
/// `suppressed` says which features are suppressed, by place; `standing` is theirs
/// ([`standing`]); `links` finds a feature by ID and a solid's consumer; `forest` knows each
/// feature before `at` as settled. For a chain's cause, the place is the input's, which names
/// it; for a lost name, the boolean's.
pub(crate) fn suppressed_parent(
    features: &[Feature],
    links: &Links,
    at: usize,
    suppressed: &dyn Fn(usize) -> bool,
    standing: &[Option<usize>],
    forest: Option<&mut ConsumerForest>,
) -> Option<(usize, Cause)> {
    let feature = features.get(at)?;
    let before = |id: &FeatureId| links.place(id).filter(|&p| p < at);
    if let Some(sketch) = feature.operation.sketch().and_then(before)
        && suppressed(sketch)
    {
        return Some((sketch, Cause::Sketch));
    }
    let made = makers(features, feature, &before);
    for &(i, maker) in &made {
        if suppressed(maker) {
            return Some((maker, Cause::Names(i)));
        }
    }
    if let Some(forest) = forest
        && let Some(lost) = lost(features, links, at, &made, forest)
    {
        return Some(lost);
    }
    for (i, original) in feature.operation.originals().iter().enumerate() {
        if let Some(p) = before(original)
            && suppressed(p)
        {
            return Some((p, Cause::Copies(i)));
        }
    }
    for (field, input) in feature.operation.inputs() {
        if !needs(&feature.operation, field) {
            continue;
        }
        if let Some(p) = before(input)
            && suppressed(p)
            && standing.get(p).copied().flatten().is_none()
        {
            return Some((p, Cause::Chain(field)));
        }
    }
    None
}

/// Of the stored names' makers `made` (each with its name's place) of the feature at `at`,
/// the first whose way down to the feature's base, by its consumers (each solid is consumed
/// once, so it's one way), passes a suppressed feature that consumes it as its `tool`: that
/// boolean passes only its base through, so the name is lost to the suppression. The
/// boolean nearest the maker, and the cause. A maker whose consumers never reach the base
/// was out of the base's lineage before any suppression (open item 2b): the rebuild's to
/// report, not suppression's. A sketch's elements are named only in what a feature made of
/// them, so its maker is a solid's too, which this takes. `forest` answers for each way
/// ([`ConsumerForest`]), every feature down to the base settled.
fn lost(
    features: &[Feature],
    links: &Links,
    at: usize,
    made: &[(NameField, usize)],
    forest: &mut ConsumerForest,
) -> Option<(usize, Cause)> {
    let base = features
        .get(at)
        .and_then(|f| f.operation.base())
        .and_then(|b| links.place(b))
        .filter(|&b| b < at)?;
    made.iter().find_map(|&(field, maker)| {
        if !forest.on_way(maker, base) {
            return None;
        }
        // The first place from the maker that doesn't pass its elements on: with the base on
        // the way, one before it has a consumer at the base or before, settled, so it's
        // consumed as a suppressed boolean's tool, and that boolean is the one.
        let stop = forest.stop(maker);
        if stop >= base {
            return None;
        }
        links
            .consumer(stop)
            .map(|(boolean, _)| (boolean, Cause::Lost { field, maker }))
    })
}

/// The places of the makers of each name `feature` stores ([`Operation::stored_names`]: a
/// fillet's or chamfer's `edges`, a sketch's face), with where the name is stored, in the
/// order written, each maker once a name; a name that doesn't parse names none (the gate
/// refuses it on its own). `before` finds an earlier feature by ID.
fn makers(
    features: &[Feature],
    feature: &Feature,
    before: &dyn Fn(&FeatureId) -> Option<usize>,
) -> Vec<(NameField, usize)> {
    let maker = |id: &FeatureId| {
        before(id)
            .and_then(|p| features.get(p))
            .map(|f| Maker::of(&f.operation))
    };
    let mut found = Vec::new();
    for (field, stored) in feature.operation.stored_names() {
        let Ok(name) = Name::parse(stored.as_str(), &maker) else {
            continue;
        };
        for id in name.features() {
            if let Some(p) = before(id) {
                found.push((field, p));
            }
        }
    }
    found
}

/// A feature that has to be suppressed, though it isn't: its place, the suppressed feature it
/// depends on (by place) and why, and the feature the text suppresses that it goes with: its
/// parent, if the text suppresses it, or else its parent's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Dependent {
    pub at: usize,
    pub parent: usize,
    pub cause: Cause,
    pub origin: usize,
}

/// Every feature that has to be suppressed with those `flags` says are, to a fixed point in
/// timeline order (each dependent comes after what it depends on, so one pass reaches it):
/// the flags after, and each feature they took, in timeline order. What `suppress` adds, and
/// what the gate refuses, all at once, so a hand edit's every dependent is named in one check.
pub(crate) fn dependents(
    features: &[Feature],
    links: &Links,
    mut flags: Vec<bool>,
) -> (Vec<bool>, Vec<Dependent>) {
    let mut walk = any_tool_suppressed(features, &|p| flags.get(p).copied().unwrap_or(false));
    let mut forest = ConsumerForest::of(links);
    let mut origins: Vec<usize> = (0..features.len()).collect();
    let mut standing: Vec<Option<usize>> = Vec::with_capacity(features.len());
    let mut found = Vec::new();
    for (i, feature) in features.iter().enumerate() {
        let suppressed = |p: usize| flags.get(p).copied().unwrap_or(false);
        if !suppressed(i)
            && let Some((parent, cause)) = suppressed_parent(
                features,
                links,
                i,
                &suppressed,
                &standing,
                walk.then_some(&mut forest),
            )
        {
            let origin = origins.get(parent).copied().unwrap_or(parent);
            if let Some(flag) = flags.get_mut(i) {
                *flag = true;
            }
            if let Some(slot) = origins.get_mut(i) {
                *slot = origin;
            }
            walk |= has_tool(&feature.operation);
            found.push(Dependent {
                at: i,
                parent,
                cause,
                origin,
            });
        }
        // What stands for it, now that whether it's suppressed is settled: those before it
        // are.
        let now = flags.get(i).copied().unwrap_or(false);
        forest.settle(features, links, i, now);
        let stands = if !feature.operation.makes_solid() {
            None
        } else if !now {
            Some(i)
        } else {
            feature
                .operation
                .base()
                .and_then(|b| links.place(b))
                .filter(|&b| b < i)
                .and_then(|b| standing.get(b).copied().flatten())
        };
        standing.push(stands);
    }
    (flags, found)
}

/// The refusal of a document in which `dependent` isn't suppressed though `parent`, which it
/// can't do without, is or has to be (the gate's), naming both, in the words of `cause`;
/// `origin` is the feature the text suppresses that the parent goes with.
pub(crate) fn unsuppressed_dependent(
    features: &[Feature],
    dependent: &Feature,
    parent: &Feature,
    cause: Cause,
    origin: &Feature,
) -> (String, String, String) {
    let (d, p, o) = (&dependent.id, &parent.id, &origin.id);
    let what = dependent.operation.type_name();
    // The parent's state: suppressed in the text, or to be, with the feature that is.
    let is = if parent.suppressed {
        "is suppressed".to_owned()
    } else {
        format!("has to be suppressed with {o}")
    };
    let (parameter, why) = match cause {
        Cause::Sketch => ("sketch".to_owned(), format!("its sketch {p} {is}")),
        Cause::Names(field) => (
            field.to_string(),
            format!("{field} names an element of {p}, which {is}"),
        ),
        Cause::Lost { field, maker } => {
            let maker = features.get(maker).map_or("", |f| f.id.as_str());
            (
                field.to_string(),
                format!(
                    "{field} names an element of {maker}, which reaches its base only as {p}'s tool, and {p} {is}"
                ),
            )
        }
        Cause::Copies(i) => (
            format!("copies.features.{i}"),
            format!("it copies {p}, which {is}"),
        ),
        Cause::Chain(field) => (
            field.to_owned(),
            if parent.suppressed {
                format!(
                    "its {field} {p} is suppressed with every feature up its chain, and a {what} needs its {field}"
                )
            } else {
                format!(
                    "its {field} {p} has to be suppressed with {o}, with every feature up its chain, and a {what} needs its {field}"
                )
            },
        ),
    };
    (
        parameter,
        format!(
            "{d} isn't suppressed, but {why}: a feature that depends on a suppressed one is suppressed with it"
        ),
        format!(r#"add "suppressed": true to {d}, or take it off {o}"#),
    )
}

/// What a suppression or an unsuppression did: the document after it, and the features it
/// suppressed and unsuppressed, in timeline order, the feature asked for among them; both
/// empty where it changed nothing (a feature suppressed already, or one that isn't).
#[derive(Debug, Clone)]
pub struct Suppression {
    pub document: Document,
    pub suppressed: Vec<FeatureId>,
    pub unsuppressed: Vec<FeatureId>,
}

impl Suppression {
    /// Whether it changed the document.
    pub fn changed(&self) -> bool {
        !(self.suppressed.is_empty() && self.unsuppressed.is_empty())
    }
}

/// The feature `id` suppressed (`true`) or unsuppressed (`false`): [`suppress`] or
/// [`unsuppress`].
pub fn set_suppressed(
    checked: &Checked,
    id: &FeatureId,
    suppressed: bool,
) -> Result<Suppression, Error> {
    if suppressed {
        suppress(checked, id)
    } else {
        unsuppress(checked, id)
    }
}

/// The feature `id` suppressed, with its dependents, taken to a fixed point in timeline
/// order (each dependent comes after what it depends on, so one pass reaches it). A feature
/// suppressed already changes nothing. Refused, naming `feature`, where the document has no
/// feature `id`.
pub fn suppress(checked: &Checked, id: &FeatureId) -> Result<Suppression, Error> {
    let _span = tracing::debug_span!("suppress", feature = %id).entered();
    let document = checked.document();
    let features = &document.features;
    let at = checked.place(id).ok_or_else(|| no_feature(id))?;
    let mut flags: Vec<bool> = features.iter().map(|f| f.suppressed).collect();
    if flags.get(at).copied().unwrap_or(true) {
        return Ok(unchanged(document));
    }
    if let Some(flag) = flags.get_mut(at) {
        *flag = true;
    }
    let links = Links::of(features);
    let (flags, taken) = dependents(features, &links, flags);
    let mut newly: Vec<usize> = std::iter::once(at)
        .chain(taken.iter().map(|d| d.at))
        .collect();
    newly.sort_unstable();
    let newly = newly
        .into_iter()
        .filter_map(|p| features.get(p).map(|f| f.id.clone()))
        .collect();
    Ok(Suppression {
        document: flagged(document, &flags),
        suppressed: newly,
        unsuppressed: Vec::new(),
    })
}

/// The feature `id` unsuppressed, with what it needs and lacks, recursively: its sketch;
/// each suppressed maker of a name it stores; each suppressed boolean whose tool a maker of
/// a name it stores reaches its base through; and, for an input it needs whose chain holds
/// no unsuppressed feature once those are unsuppressed, that chain's root. What depends on it
/// stays suppressed. A feature that isn't suppressed changes nothing. Refused, naming
/// `feature`, where the document has no feature `id`.
pub fn unsuppress(checked: &Checked, id: &FeatureId) -> Result<Suppression, Error> {
    let _span = tracing::debug_span!("unsuppress", feature = %id).entered();
    let document = checked.document();
    let features = &document.features;
    let at = checked.place(id).ok_or_else(|| no_feature(id))?;
    let flags: Vec<bool> = features.iter().map(|f| f.suppressed).collect();
    if !flags.get(at).copied().unwrap_or(false) {
        return Ok(unchanged(document));
    }
    let flags = unsuppressing(features, &|id| checked.place(id), at, flags);
    let unsuppressed: Vec<FeatureId> = features
        .iter()
        .zip(&flags)
        .filter(|(feature, now)| feature.suppressed && !**now)
        .map(|(feature, _)| feature.id.clone())
        .collect();
    Ok(Suppression {
        document: flagged(document, &flags),
        suppressed: Vec::new(),
        unsuppressed,
    })
}

/// The suppression `flags` after unsuppressing the feature at `at` ([`unsuppress`]), which
/// `flags` holds suppressed; `place` finds a feature's place by ID.
fn unsuppressing(
    features: &[Feature],
    place: &dyn Fn(&FeatureId) -> Option<usize>,
    at: usize,
    mut flags: Vec<bool>,
) -> Vec<bool> {
    let links = Links::of(features);
    // Unsuppressing only takes suppressions away, so a name can be lost only if one could
    // be before.
    let walk = any_tool_suppressed(features, &|p| flags.get(p).copied().unwrap_or(false));
    let mut forest = ConsumerForest::settled(features, &links, &|p| {
        flags.get(p).copied().unwrap_or(false)
    });
    // Its sketch and its names' makers first, all the way up; then each name lost to a
    // suppressed boolean gives that boolean, and each chain left with no unsuppressed
    // feature its root, whose own needs are taken the same way, the feature looked at again
    // until it lacks nothing. In timeline order, so the result is the text's alone.
    let mut direct: Vec<usize> = vec![at];
    let mut chains: BTreeSet<usize> = BTreeSet::new();
    loop {
        while let Some(w) = direct.pop() {
            match flags.get_mut(w) {
                Some(flag) if *flag => *flag = false,
                _ => continue,
            }
            forest.settle(features, &links, w, false);
            chains.insert(w);
            let Some(feature) = features.get(w) else {
                continue;
            };
            let before = |id: &FeatureId| place(id).filter(|&p| p < w);
            direct.extend(feature.operation.sketch().and_then(before));
            direct.extend(
                makers(features, feature, &before)
                    .into_iter()
                    .map(|(_, p)| p),
            );
            // A pattern's originals: it's their child (docs/design/m6.0-patterns.md,
            // section 6).
            direct.extend(feature.operation.originals().iter().filter_map(before));
        }
        let Some(w) = chains.pop_first() else {
            break;
        };
        let Some(feature) = features.get(w) else {
            continue;
        };
        if walk {
            let before = |id: &FeatureId| place(id).filter(|&p| p < w);
            let made = makers(features, feature, &before);
            if let Some((boolean, _)) = lost(features, &links, w, &made, &mut forest) {
                direct.push(boolean);
                chains.insert(w);
                continue;
            }
        }
        for (field, input) in feature.operation.inputs() {
            if !needs(&feature.operation, field) {
                continue;
            }
            let Some(start) = place(input).filter(|&p| p < w) else {
                continue;
            };
            if let Some(root) = suppressed_root(features, &flags, place, start) {
                direct.push(root);
            }
        }
    }
    flags
}

/// The root of the chain up from `start` by `base` links, if every feature of it is
/// suppressed (`flags`); none if one isn't.
fn suppressed_root(
    features: &[Feature],
    flags: &[bool],
    place: &dyn Fn(&FeatureId) -> Option<usize>,
    start: usize,
) -> Option<usize> {
    let mut at = start;
    loop {
        if !flags.get(at).copied().unwrap_or(false) {
            return None;
        }
        // Each base comes before its feature, so the walk ends.
        match features
            .get(at)
            .and_then(|f| f.operation.base())
            .and_then(place)
            .filter(|&b| b < at)
        {
            Some(base) => at = base,
            None => return Some(at),
        }
    }
}

/// `document` with each feature suppressed as `flags` says.
fn flagged(document: &Document, flags: &[bool]) -> Document {
    let mut edited = document.clone();
    for (feature, &flag) in edited.features.iter_mut().zip(flags) {
        feature.suppressed = flag;
    }
    edited
}

fn unchanged(document: &Document) -> Suppression {
    Suppression {
        document: document.clone(),
        suppressed: Vec::new(),
        unsuppressed: Vec::new(),
    }
}

fn no_feature(id: &FeatureId) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("the document has no feature {id}"),
    )
    .with_parameter("feature")
    .with_suggestion("name a feature by its ID, as the document holds it")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document of `features`, each a feature's JSON, as the reader reads it.
    fn document(features: &[&str]) -> Document {
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
            features.join(", ")
        );
        crate::document::read(text.as_bytes()).unwrap()
    }

    fn by_id(document: &Document) -> impl Fn(&FeatureId) -> Option<usize> + '_ {
        |id| document.features.iter().position(|f| f.id == *id)
    }

    const BOX: &str = r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#;
    const SKETCH: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}"#;

    /// A chamfer `id` of `base`, naming `edge`, suppressed if `suppressed`.
    fn chamfer(id: &str, base: &str, edge: &str, suppressed: bool) -> String {
        let flag = if suppressed {
            r#""suppressed": true, "#
        } else {
            ""
        };
        format!(
            r#"{{"id": "{id}", "type": "chamfer", "v": 1, "name": "C", {flag}"base": "{base}", "edges": ["{edge}"], "size": {{"distance": 1}}}}"#
        )
    }

    #[test]
    fn a_suppressed_features_solid_is_its_bases_all_the_way_up_and_a_root_passes_nothing() {
        // b, then c1 on b (suppressed), c2 on c1 (suppressed), c3 on c2: by hand, b stands
        // for itself and for c1 and c2, which pass it through, and c3 for itself; the sketch
        // stands for nothing. With b suppressed too, nothing stands for b, c1, or c2.
        let c1 = chamfer("c0000001", "b0000001", "b0000001.box.Edge1", true);
        let c2 = chamfer("c0000002", "c0000001", "b0000001.box.Edge3", true);
        let c3 = chamfer("c0000003", "c0000002", "b0000001.box.Edge5", false);
        let d = document(&[BOX, SKETCH, &c1, &c2, &c3]);
        let place = by_id(&d);
        assert_eq!(
            standing(&d.features, &place),
            [Some(0), None, Some(0), Some(0), Some(4)]
        );
        // c3's base, c2, is built as b: its effective base.
        assert_eq!(
            effective_inputs(&d.features, &standing(&d.features, &place), &place, 4),
            [("base", Some(0))]
        );
        let mut rootless = d.clone();
        rootless.features[0].suppressed = true;
        let place = by_id(&rootless);
        assert_eq!(
            standing(&rootless.features, &place),
            [None, None, None, None, Some(4)]
        );
        assert_eq!(
            effective_inputs(
                &rootless.features,
                &standing(&rootless.features, &place),
                &place,
                4
            ),
            [("base", None)]
        );
    }

    #[test]
    fn a_reference_to_a_later_feature_stands_for_nothing() {
        // In a document the gate refuses, c1's base is the later c2: never followed.
        let c1 = chamfer("c0000001", "c0000002", "b0000001.box.Edge1", true);
        let c2 = chamfer("c0000002", "b0000001", "b0000001.box.Edge3", false);
        let d = document(&[BOX, &c1, &c2]);
        let place = by_id(&d);
        assert_eq!(standing(&d.features, &place), [Some(0), None, Some(2)]);
    }

    #[test]
    fn only_a_pads_and_a_revolves_base_is_optional() {
        let pad = |base: Option<&str>| -> Operation {
            let base = base.map_or(String::new(), |b| format!(r#""base": "{b}", "#));
            let text = format!(
                r#"{{"id": "p0000001", "type": "pad", "v": 1, "name": "P", {base}"sketch": "s0000001", "extent": {{"type": "one_side", "length": 1, "reversed": false}}}}"#
            );
            document(&[BOX, SKETCH, &text]).features[2]
                .operation
                .clone()
        };
        assert!(!needs(&pad(Some("b0000001")), "base"));
        let c = document(&[
            BOX,
            &chamfer("c0000001", "b0000001", "b0000001.box.Edge1", false),
        ]);
        assert!(needs(&c.features[1].operation, "base"));
        let cut = document(&[
            BOX,
            r#"{"id": "b0000002", "type": "box", "v": 1, "name": "T", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
            r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "U", "base": "b0000001", "tool": "b0000002"}"#,
        ]);
        assert!(needs(&cut.features[2].operation, "base"));
        assert!(needs(&cut.features[2].operation, "tool"));
        // A revolve's base is optional as a pad's; a groove's and a pocket's are needed.
        let turned = |kind: &str| -> Operation {
            let text = format!(
                r#"{{"id": "v0000001", "type": "{kind}", "v": 1, "name": "V", "base": "b0000001", "sketch": "s0000001", "axis": "x_axis", "extent": {{"type": "one_side", "angle": 90, "reversed": false}}}}"#
            );
            document(&[BOX, SKETCH, &text]).features[2]
                .operation
                .clone()
        };
        assert!(!needs(&turned("revolve"), "base"));
        assert!(needs(&turned("groove"), "base"));
        let pocket = document(&[
            BOX,
            SKETCH,
            r#"{"id": "k0000001", "type": "pocket", "v": 1, "name": "K", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "through_all", "reversed": false}}"#,
        ]);
        assert!(needs(&pocket.features[2].operation, "base"));
    }

    #[test]
    fn a_dependents_cause_is_its_sketch_then_its_names_then_its_chain() {
        // c2 names b's edge and builds on c1: with b suppressed (and so c1 with it), its
        // first cause is its name of b's edge, edges.0, before its chain.
        let c1 = chamfer("c0000001", "b0000001", "b0000001.box.Edge1", true);
        let c2 = chamfer("c0000002", "c0000001", "b0000001.box.Edge3", false);
        let mut d = document(&[BOX, &c1, &c2]);
        d.features[0].suppressed = true;
        let place = by_id(&d);
        let st = standing(&d.features, &place);
        let suppressed = |p: usize| d.features[p].suppressed;
        assert_eq!(
            suppressed_parent(
                &d.features,
                &Links::of(&d.features),
                2,
                &suppressed,
                &st,
                Some(&mut settled(&d.features, &suppressed)),
            ),
            Some((0, Cause::Names(NameField::Edge(0))))
        );
        // Named elsewhere (a sketch's element can't be, so a second box), c2's only cause
        // is its chain, by its base, c1.
        let other = r#"{"id": "b0000002", "type": "box", "v": 1, "name": "O", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#;
        let c2 = chamfer("c0000002", "c0000001", "b0000002.box.Edge3", false);
        let mut d = document(&[BOX, other, &c1, &c2]);
        d.features[0].suppressed = true;
        let place = by_id(&d);
        let st = standing(&d.features, &place);
        let suppressed = |p: usize| d.features[p].suppressed;
        assert_eq!(
            suppressed_parent(
                &d.features,
                &Links::of(&d.features),
                3,
                &suppressed,
                &st,
                Some(&mut settled(&d.features, &suppressed)),
            ),
            Some((2, Cause::Chain("base")))
        );
        // b back: c1 passes it through, so c2 has none.
        let back = document(&[BOX, other, &c1, &c2]);
        let place = by_id(&back);
        let st = standing(&back.features, &place);
        let suppressed = |p: usize| back.features[p].suppressed;
        assert_eq!(
            suppressed_parent(
                &back.features,
                &Links::of(&back.features),
                3,
                &suppressed,
                &st,
                Some(&mut settled(&back.features, &suppressed)),
            ),
            None
        );
    }

    /// The consumer forest of `features` with each settled as `suppressed` says.
    fn settled(features: &[Feature], suppressed: &dyn Fn(usize) -> bool) -> ConsumerForest {
        ConsumerForest::settled(features, &Links::of(features), suppressed)
    }

    // -------------------------------------------------------------------------------------
    // The walk #304 made, step by step down each maker's consumers, kept as the reference the
    // consumer forest is held to (issue #306): `lost`, `suppressed_parent`, `dependents`, and
    // unsuppress's loop as they were before the consumer forest, word for word but for their names.

    fn reference_lost(
        features: &[Feature],
        links: &Links,
        at: usize,
        made: &[(NameField, usize)],
        suppressed: &dyn Fn(usize) -> bool,
    ) -> Option<(usize, Cause)> {
        let base = features
            .get(at)
            .and_then(|f| f.operation.base())
            .and_then(|b| links.place(b))
            .filter(|&b| b < at)?;
        made.iter().find_map(|&(field, maker)| {
            let mut step = maker;
            let mut through = None;
            while step != base {
                let (next, field) = links.consumer(step).filter(|&(n, _)| n <= base)?;
                if through.is_none() && field == "tool" && suppressed(next) {
                    through = Some(next);
                }
                step = next;
            }
            through.map(|boolean| (boolean, Cause::Lost { field, maker }))
        })
    }

    fn reference_parent(
        features: &[Feature],
        links: &Links,
        at: usize,
        suppressed: &dyn Fn(usize) -> bool,
        standing: &[Option<usize>],
        walk: bool,
    ) -> Option<(usize, Cause)> {
        let feature = features.get(at)?;
        let before = |id: &FeatureId| links.place(id).filter(|&p| p < at);
        if let Some(sketch) = feature.operation.sketch().and_then(before)
            && suppressed(sketch)
        {
            return Some((sketch, Cause::Sketch));
        }
        let made = makers(features, feature, &before);
        for &(i, maker) in &made {
            if suppressed(maker) {
                return Some((maker, Cause::Names(i)));
            }
        }
        if walk && let Some(lost) = reference_lost(features, links, at, &made, suppressed) {
            return Some(lost);
        }
        for (field, input) in feature.operation.inputs() {
            if !needs(&feature.operation, field) {
                continue;
            }
            if let Some(p) = before(input)
                && suppressed(p)
                && standing.get(p).copied().flatten().is_none()
            {
                return Some((p, Cause::Chain(field)));
            }
        }
        None
    }

    fn reference_dependents(
        features: &[Feature],
        links: &Links,
        mut flags: Vec<bool>,
    ) -> (Vec<bool>, Vec<Dependent>) {
        let mut walk = any_tool_suppressed(features, &|p| flags.get(p).copied().unwrap_or(false));
        let mut origins: Vec<usize> = (0..features.len()).collect();
        let mut standing: Vec<Option<usize>> = Vec::with_capacity(features.len());
        let mut found = Vec::new();
        for (i, feature) in features.iter().enumerate() {
            let suppressed = |p: usize| flags.get(p).copied().unwrap_or(false);
            if !suppressed(i)
                && let Some((parent, cause)) =
                    reference_parent(features, links, i, &suppressed, &standing, walk)
            {
                let origin = origins.get(parent).copied().unwrap_or(parent);
                if let Some(flag) = flags.get_mut(i) {
                    *flag = true;
                }
                if let Some(slot) = origins.get_mut(i) {
                    *slot = origin;
                }
                walk |= has_tool(&feature.operation);
                found.push(Dependent {
                    at: i,
                    parent,
                    cause,
                    origin,
                });
            }
            let stands = if !feature.operation.makes_solid() {
                None
            } else if !flags.get(i).copied().unwrap_or(false) {
                Some(i)
            } else {
                feature
                    .operation
                    .base()
                    .and_then(|b| links.place(b))
                    .filter(|&b| b < i)
                    .and_then(|b| standing.get(b).copied().flatten())
            };
            standing.push(stands);
        }
        (flags, found)
    }

    fn reference_unsuppressing(
        features: &[Feature],
        place: &dyn Fn(&FeatureId) -> Option<usize>,
        at: usize,
        mut flags: Vec<bool>,
    ) -> (Vec<bool>, usize) {
        let mut found = 0;
        let links = Links::of(features);
        let walk = any_tool_suppressed(features, &|p| flags.get(p).copied().unwrap_or(false));
        let mut direct: Vec<usize> = vec![at];
        let mut chains: BTreeSet<usize> = BTreeSet::new();
        loop {
            while let Some(w) = direct.pop() {
                match flags.get_mut(w) {
                    Some(flag) if *flag => *flag = false,
                    _ => continue,
                }
                chains.insert(w);
                let Some(feature) = features.get(w) else {
                    continue;
                };
                let before = |id: &FeatureId| place(id).filter(|&p| p < w);
                direct.extend(feature.operation.sketch().and_then(before));
                direct.extend(
                    makers(features, feature, &before)
                        .into_iter()
                        .map(|(_, p)| p),
                );
            }
            let Some(w) = chains.pop_first() else {
                break;
            };
            let Some(feature) = features.get(w) else {
                continue;
            };
            if walk {
                let before = |id: &FeatureId| place(id).filter(|&p| p < w);
                let made = makers(features, feature, &before);
                let suppressed = |p: usize| flags.get(p).copied().unwrap_or(false);
                if let Some((boolean, _)) = reference_lost(features, &links, w, &made, &suppressed)
                {
                    found += 1;
                    direct.push(boolean);
                    chains.insert(w);
                    continue;
                }
            }
            for (field, input) in feature.operation.inputs() {
                if !needs(&feature.operation, field) {
                    continue;
                }
                let Some(start) = place(input).filter(|&p| p < w) else {
                    continue;
                };
                if let Some(root) = suppressed_root(features, &flags, place, start) {
                    direct.push(root);
                }
            }
        }
        (flags, found)
    }

    /// SplitMix64: the same documents every run.
    struct Draws(u64);

    impl Draws {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// A draw below `n` (which isn't 0).
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// A random document of `n` features, each `f` and its place: boxes, chamfers, fuses,
    /// cuts, and commons, each input any feature, before it or not (a reference forward, to
    /// itself, a solid consumed twice, a boolean's base its tool, and a chamfer of a sketch
    /// are the gate's to refuse, and the walks must agree on them all the same), each chamfer
    /// naming one to three elements of features before it or not, by their own type words; a
    /// sketch now and then; each suppressed at random.
    fn random_document(draws: &mut Draws, n: usize) -> Document {
        let word = |kind: usize| ["box", "chamfer", "fuse", "cut", "common", "sketch"][kind];
        let kinds: Vec<usize> = (0..n)
            .map(|i| match draws.below(10) {
                _ if i == 0 => 0,
                0..=2 => 0,
                3..=5 => 1,
                6 => 2 + draws.below(3),
                7 | 8 => 2,
                _ => 5,
            })
            .collect();
        let id = |at: usize| format!("f{at:07}");
        // Mostly a solid none consumes yet, so the consumers make long ways down; now and
        // then any feature before, or any at all.
        let mut open: Vec<usize> = Vec::new();
        let input = |draws: &mut Draws, open: &mut Vec<usize>, at: usize| {
            let p = if !open.is_empty() && draws.below(4) != 0 {
                open.swap_remove(draws.below(open.len()))
            } else if at == 0 || draws.below(6) == 0 {
                draws.below(n)
            } else {
                draws.below(at)
            };
            open.retain(|&o| o != p);
            p
        };
        let mut features = Vec::new();
        for (at, &kind) in kinds.iter().enumerate() {
            // Booleans suppressed as often as not, so ways pass suppressed tools.
            let flag = if draws.below(if (2..=4).contains(&kind) { 2 } else { 4 }) == 0 {
                r#""suppressed": true, "#
            } else {
                ""
            };
            let fid = id(at);
            let text = match kind {
                0 => format!(
                    r#"{{"id": "{fid}", "type": "box", "v": 1, "name": "B", {flag}"position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#
                ),
                1 => {
                    let base = id(input(draws, &mut open, at));
                    let names: Vec<String> = (0..1 + draws.below(3))
                        .map(|_| {
                            let of = if draws.below(8) == 0 {
                                draws.below(n)
                            } else {
                                draws.below(at.max(1))
                            };
                            let element = if kinds[of] == 0 { "Edge1" } else { "Face1" };
                            format!(r#""{}.{}.{element}""#, id(of), word(kinds[of]))
                        })
                        .collect();
                    format!(
                        r#"{{"id": "{fid}", "type": "chamfer", "v": 1, "name": "C", {flag}"base": "{base}", "edges": [{}], "size": {{"distance": 1}}}}"#,
                        names.join(", ")
                    )
                }
                2..=4 => {
                    let base = id(input(draws, &mut open, at));
                    let tool = id(input(draws, &mut open, at));
                    format!(
                        r#"{{"id": "{fid}", "type": "{}", "v": 1, "name": "O", {flag}"base": "{base}", "tool": "{tool}"}}"#,
                        word(kind)
                    )
                }
                _ => format!(
                    r#"{{"id": "{fid}", "type": "sketch", "v": 1, "name": "S", {flag}"plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#
                ),
            };
            features.push(text);
            if kind != 5 {
                open.push(at);
            }
        }
        let refs: Vec<&str> = features.iter().map(String::as_str).collect();
        document(&refs)
    }

    #[test]
    fn the_consumer_forest_judges_every_random_document_as_the_step_by_step_walk_does() {
        // 20,000 random documents of 1 to 16 features: the gate's every dependent, with each
        // feature as the text has it and with each feature suppressed in turn, and every
        // unsuppression of each suppressed feature, as the reference walk gives them.
        let mut draws = Draws(306);
        let (mut lost, mut lost_unsuppressing, mut cases) = (0, 0, 0);
        for _ in 0..20_000 {
            let n = 1 + draws.below(16);
            let d = random_document(&mut draws, n);
            let features = &d.features;
            let links = Links::of(features);
            let text: Vec<bool> = features.iter().map(|f| f.suppressed).collect();
            for also in std::iter::once(None).chain((0..n).map(Some)) {
                let mut flags = text.clone();
                if let Some(at) = also {
                    flags[at] = true;
                }
                let ours = dependents(features, &links, flags.clone());
                let theirs = reference_dependents(features, &links, flags);
                assert_eq!(ours, theirs, "{:?}", crate::document::write(&d));
                lost += ours
                    .1
                    .iter()
                    .filter(|d| matches!(d.cause, Cause::Lost { .. }))
                    .count();
                cases += 1;
            }
            let place = by_id(&d);
            for at in (0..n).filter(|&at| text[at]) {
                let ours = unsuppressing(features, &place, at, text.clone());
                let (theirs, found) = reference_unsuppressing(features, &place, at, text.clone());
                assert_eq!(
                    ours,
                    theirs,
                    "unsuppressing {at}: {:?}",
                    crate::document::write(&d)
                );
                lost_unsuppressing += found;
                cases += 1;
            }
        }
        // The documents reach the walk: names lost to a suppressed boolean, found by both, and
        // booleans an unsuppression brings back for a name the reference found lost.
        // Fixed draws, so fixed counts: 7,978 and 561.
        assert!(lost > 4_000, "{lost} lost names in {cases} cases");
        assert!(
            lost_unsuppressing > 250,
            "{lost_unsuppressing} booleans brought back for a lost name, in {cases} cases"
        );
    }

    /// Whether the consumer forest says, for the feature at `at` of `d` and every place as its one
    /// name's maker, what the reference walk says with `suppressed`; how many names it found
    /// lost.
    fn every_way_agrees(
        d: &Document,
        links: &Links,
        forest: &mut ConsumerForest,
        suppressed: &dyn Fn(usize) -> bool,
        at: usize,
    ) -> usize {
        let features = &d.features;
        let mut lost_names = 0;
        for maker in 0..features.len() {
            let made = [(NameField::Edge(0), maker)];
            let ours = lost(features, links, at, &made, forest);
            let theirs = reference_lost(features, links, at, &made, suppressed);
            assert_eq!(
                ours,
                theirs,
                "at {at}, maker {maker}: {:?}",
                crate::document::write(d)
            );
            lost_names += usize::from(ours.is_some());
        }
        lost_names
    }

    #[test]
    fn the_consumer_forest_answers_every_way_as_the_step_by_step_walk_does() {
        // 20,000 random documents of 1 to 16 features, each feature asked of every place as
        // its name's maker (whether before it, after it, itself, or its base): settled in
        // timeline order, each asked once those before it are, as the gate asks; and settled
        // at once, then each suppressed feature unsuppressed in a random order, every feature
        // asked after each, as an unsuppression asks.
        let mut draws = Draws(1306);
        let (mut in_order, mut unsuppressing) = (0, 0);
        for _ in 0..20_000 {
            let n = 1 + draws.below(16);
            let d = random_document(&mut draws, n);
            let features = &d.features;
            let links = Links::of(features);
            let mut flags: Vec<bool> = features.iter().map(|f| f.suppressed).collect();
            let mut forest = ConsumerForest::of(&links);
            for at in 0..n {
                in_order += every_way_agrees(&d, &links, &mut forest, &|p| flags[p], at);
                forest.settle(features, &links, at, flags[at]);
            }
            let mut forest = settled(features, &|p| flags[p]);
            let mut order: Vec<usize> = (0..n).filter(|&p| flags[p]).collect();
            while !order.is_empty() {
                let at = order.swap_remove(draws.below(order.len()));
                flags[at] = false;
                forest.settle(features, &links, at, false);
                for w in 0..n {
                    unsuppressing += every_way_agrees(&d, &links, &mut forest, &|p| flags[p], w);
                }
            }
        }
        assert!(in_order > 10_000, "{in_order} lost in timeline order");
        assert!(
            unsuppressing > 10_000,
            "{unsuppressing} lost while unsuppressing"
        );
    }
}
