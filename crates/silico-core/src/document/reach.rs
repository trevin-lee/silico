//! What an edit reaches (docs/design/m4.1-command-api.md, section 5; decision 28): the
//! features whose own values it changed, compared by ID, and every feature that uses one of
//! them. A commit writes new fingerprints and solves for those only, and a check doesn't
//! hold them to the fingerprints stored with them, which were measured from other inputs;
//! every other feature keeps its stored record, and a rebuild that differs from it is drift.

use std::collections::{BTreeMap, BTreeSet};

use super::json::{self, Value};
use super::{Document, Feature, FeatureId};

/// The document an edit was made from: read, or text that didn't read as one (a merge's
/// conflict markers, say), which only `replace_text` mends, as the IDs it holds; or such
/// text and the first document that read after it, its mend, which later edits are made
/// from.
#[derive(Clone, Copy, Debug)]
pub enum Before<'a> {
    Read(&'a Document),
    Unread(&'a QuotedIds),
    Mended {
        unread: &'a QuotedIds,
        mend: &'a Document,
    },
}

/// The feature IDs a text that didn't read as a document holds quoted, as any reference to
/// a feature is: each run of the text between two quotes in a row that no backslash escapes
/// (whichever way its quotes pair, so a stray quote in what didn't read can't hide an ID
/// after it), its JSON escapes decoded as the reader decodes them (`"k7f2qx3\u006d"` holds
/// k7f2qx3m). Found in one pass over the text, so whether it holds a feature's ID is a
/// lookup, not a search of the text for each feature.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuotedIds(BTreeSet<FeatureId>);

/// The most bytes an ID written with escapes takes between its quotes: each of its
/// characters as a `\uXXXX` escape.
const MOST_ESCAPED_ID: usize = 6 * super::model::ID_LENGTH;

impl QuotedIds {
    /// The IDs `text` holds quoted.
    pub fn of(text: &[u8]) -> Self {
        let mut ids = BTreeSet::new();
        // Where the run after the last unescaped quote starts.
        let mut run: Option<usize> = None;
        let mut escaped = false;
        for (at, byte) in text.iter().enumerate() {
            if escaped {
                escaped = false;
                continue;
            }
            match byte {
                b'\\' => escaped = true,
                b'"' => {
                    if let Some(id) = run.and_then(|start| text.get(start..at)).and_then(id_of) {
                        ids.insert(id);
                    }
                    run = Some(at + 1);
                }
                _ => {}
            }
        }
        QuotedIds(ids)
    }

    /// Whether the text holds `id` quoted.
    pub fn holds(&self, id: &FeatureId) -> bool {
        self.0.contains(id)
    }
}

/// The ID that `between`, the bytes between two quotes, writes, if it writes one: as it is,
/// or, with a backslash in it, as the reader decodes it as a JSON string.
fn id_of(between: &[u8]) -> Option<FeatureId> {
    if between.len() > MOST_ESCAPED_ID {
        return None;
    }
    if !between.contains(&b'\\') {
        // An ID's characters are ASCII: one byte each.
        if between.len() != super::model::ID_LENGTH {
            return None;
        }
        return FeatureId::new(std::str::from_utf8(between).ok()?);
    }
    // An escape takes two bytes or more for one character, so an escaped run shorter than
    // an ID and one byte more can't decode to one.
    if between.len() <= super::model::ID_LENGTH {
        return None;
    }
    let mut literal = Vec::with_capacity(between.len() + 2);
    literal.push(b'"');
    literal.extend_from_slice(between);
    literal.push(b'"');
    match json::read(&literal).ok()?.value {
        Value::String(text) => FeatureId::new(&text),
        _ => None,
    }
}

/// What the edits from `before` to `after` reach, net, with `carried` (features reached
/// before whose records couldn't be written yet): [`with_users`] of `carried` and of what
/// changed from `before` to `after`, so an edit undone reaches nothing. From text that
/// didn't read, what changed is only each feature whose ID the text doesn't hold
/// ([`QuotedIds`]): one it can't have had. Every other feature keeps its stored
/// record, and its drift is said for `format` to accept by name, rather than every
/// fingerprint written over because a merge left markers in the file. From such text
/// mended, what changed is that of the mend's features, and what changed from the mend
/// to `after`: so an edit after the mend reaches what it changed, and, undone, nothing.
pub fn net_reach(
    before: Before<'_>,
    carried: &BTreeSet<FeatureId>,
    after: &Document,
) -> BTreeSet<FeatureId> {
    let mut seeds = carried.clone();
    match before {
        Before::Read(before) => seeds.extend(changed(before, after)),
        Before::Unread(ids) => seeds.extend(not_held(ids, after)),
        Before::Mended { unread, mend } => {
            seeds.extend(not_held(unread, mend));
            seeds.extend(changed(mend, after));
        }
    }
    with_users(after, &seeds)
}

/// The features of `document` whose IDs the text that didn't read doesn't hold (`ids`).
fn not_held<'a>(
    ids: &'a QuotedIds,
    document: &'a Document,
) -> impl Iterator<Item = FeatureId> + 'a {
    document
        .features
        .iter()
        .map(|feature| &feature.id)
        .filter(|id| !ids.holds(id))
        .cloned()
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
/// which reaches nothing (the design's section 5), compared as canonical form writes them,
/// so a sketch whose lists are only put in ID order (a file's, and its canonical text's)
/// hasn't changed.
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
    operation.canonically_eq(&after.operation) && *version == after.version
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
