//! What an edit reaches (docs/design/m4.1-command-api.md, section 5; decision 28): the
//! features whose own values it changed, compared by ID, and every feature that uses one of
//! them. A commit writes new fingerprints and solves for those only, and a check doesn't
//! hold them to the fingerprints stored with them, which were measured from other inputs;
//! every other feature keeps its stored record, and a rebuild that differs from it is drift.

use std::collections::{BTreeMap, BTreeSet};

use super::{Document, Feature, FeatureId};

/// The document an edit was made from: read, or text that didn't read as one (a merge's
/// conflict markers, say), which only `replace_text` mends.
#[derive(Clone, Copy, Debug)]
pub enum Before<'a> {
    Read(&'a Document),
    Unread(&'a [u8]),
}

/// What the edits from `before` to `after` reach, net, with `carried` (features reached
/// before whose records couldn't be written yet): [`with_users`] of `carried` and of what
/// changed from `before` to `after`, so an edit undone reaches nothing. From text that
/// didn't read, what changed is only each feature whose ID the text doesn't hold (quoted, as
/// any reference to it is): one it can't have had. Every other feature keeps its stored
/// record, and its drift is said for `format` to accept by name, rather than every
/// fingerprint written over because a merge left markers in the file.
pub fn net_reach(
    before: Before<'_>,
    carried: &BTreeSet<FeatureId>,
    after: &Document,
) -> BTreeSet<FeatureId> {
    let mut seeds = carried.clone();
    match before {
        Before::Read(before) => seeds.extend(changed(before, after)),
        Before::Unread(text) => seeds.extend(
            after
                .features
                .iter()
                .map(|feature| &feature.id)
                .filter(|id| !holds(text, &format!("\"{id}\"")))
                .cloned(),
        ),
    }
    with_users(after, &seeds)
}

/// Whether `text` holds `needle`'s bytes anywhere.
fn holds(text: &[u8], needle: &str) -> bool {
    text.windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// The features of `after` that an edit from `before` reaches: each whose own values the
/// edit changed (its type and values, or its version: anything but its name, its note, and
/// its fingerprint), compared with the feature of the same ID in `before`; each it added
/// (one whose ID `before` hasn't); and every feature of `after` that uses one of those,
/// directly or through others ([`with_users`]). A removed feature reaches nothing itself:
/// a feature that used it either changed what it uses, and is reached, or is refused by the
/// gate. In timeline order is no matter: the set is by ID.
pub fn reached(before: &Document, after: &Document) -> BTreeSet<FeatureId> {
    with_users(after, &changed(before, after))
}

/// The features of `after` whose own values differ from those of the feature of the same ID
/// in `before`, and those `before` hasn't: [`reached`] without what uses them.
fn changed(before: &Document, after: &Document) -> BTreeSet<FeatureId> {
    let mut was: BTreeMap<&FeatureId, &Feature> = BTreeMap::new();
    for feature in &before.features {
        // A document the gate refused may hold an ID twice: the first is the one compared.
        was.entry(&feature.id).or_insert(feature);
    }
    after
        .features
        .iter()
        .filter(|feature| {
            was.get(&feature.id)
                .is_none_or(|before| !same_values(before, feature))
        })
        .map(|feature| feature.id.clone())
        .collect()
}

/// Whether two features hold the same values of their own: every field but those an edit of
/// which reaches nothing (the design's section 5).
fn same_values(before: &Feature, after: &Feature) -> bool {
    // Every field taken apart, so one added to a feature is a compile error here until it
    // says whether changing it reaches the feature.
    let Feature {
        id: _,
        operation,
        version,
        name: _,
        note: _,
        fingerprint: _,
    } = before;
    *operation == after.operation && *version == after.version
}

/// The features of `features` that `document` has, and every feature of `document` that
/// uses one of them, directly or through others ([`Operation::uses`](super::Operation::uses):
/// the solids it consumes and the sketch it sweeps). An ID `document` hasn't is left out.
pub fn with_users(document: &Document, features: &BTreeSet<FeatureId>) -> BTreeSet<FeatureId> {
    let mut users: BTreeMap<&FeatureId, Vec<&FeatureId>> = BTreeMap::new();
    for feature in &document.features {
        for used in feature.operation.uses() {
            users.entry(used).or_default().push(&feature.id);
        }
    }
    let mut reached = BTreeSet::new();
    // An explicit stack: a timeline can be thousands deep.
    let mut stack: Vec<&FeatureId> = document
        .features
        .iter()
        .map(|feature| &feature.id)
        .filter(|id| features.contains(*id))
        .collect();
    while let Some(id) = stack.pop() {
        if reached.insert(id.clone())
            && let Some(next) = users.get(id)
        {
            stack.extend(next.iter().copied());
        }
    }
    reached
}
