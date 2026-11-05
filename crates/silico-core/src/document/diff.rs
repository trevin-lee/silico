//! The changes between two documents (`diff`, the per-feature change summary: Fable's
//! decision of 2026-11-04, *the three core gaps*, section 1): each feature added, removed, or
//! moved, by its place in the timeline, and each field of a feature that changed, by the
//! path `set_field` names it by, its value before and after as the document writes it.
//!
//! A function of the two documents' canonical texts alone, so the same pair always gives the
//! same changes, in the same order. Each feature's canonical text ([`write::placed`], what
//! every edit writes) is compared with its counterpart's, found by ID, as a string: only the
//! features whose texts differ are read as trees (`document::tree`, as `set_field` reads a
//! feature) and walked, to the first level where the two trees differ and either side is a
//! scalar or absent. Paths follow decision 15, as `set_field`'s do: a sketch's members by
//! ID, every other array by place, from 0.
//!
//! A document that reads but that the gate refuses is diffed as it reads: an ID two features
//! share pairs the first of them on one side with the first on the other, the second with
//! the second, and so on (a sketch's members likewise).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::json::{self, MAX_DOCUMENT_LEN, Node, Plain, Value};
use super::{Document, Feature, FeatureId, Operation, write};
use crate::{Error, ErrorCode};

/// One change between two documents: a feature's place in the timeline, or one of its fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The feature changed, by ID.
    pub feature: FeatureId,
    /// Its name: the later document's, or, for a feature removed, the earlier one's. Shared
    /// by each of the feature's changes, never copied for each: a name may be megabytes, and
    /// a sketch's changes thousands.
    pub name: Arc<str>,
    /// The path to the field that changed, as `set_field` names it; none for a change of
    /// the feature's place (added, removed, or moved).
    pub field: Option<String>,
    /// The field's value before, as the document writes it, or the feature's place before
    /// (from 0): `null` where the feature hadn't the field, or wasn't there.
    pub before: Plain,
    /// The field's value after, or the feature's place after: `null` likewise.
    pub after: Plain,
}

/// A feature's key on one side: its ID and how many features before it on that side share
/// it (0 for a document the gate takes, whose IDs are unique).
type Key = (FeatureId, usize);

/// Every change from `from` to `to`, in `to`'s timeline order (each feature's change of
/// place first, then its fields in canonical key order), then each feature removed, in
/// `from`'s order. A feature is moved if its rank among the features both documents hold
/// changed: so a feature removed or added moves none of the others, and moving a feature to
/// the front moves each it passed. Refused only if either document's canonical text is
/// longer than a document may be, which a document that was read never is.
pub fn diff(from: &Document, to: &Document) -> Result<Vec<Change>, Error> {
    diff_until(from, to, || Ok(()))
}

/// [`diff`], `go` asked before each of `to`'s features is compared: its error (a command's
/// `cancelled`) ends the diff, and is its answer.
pub fn diff_until(
    from: &Document,
    to: &Document,
    mut go: impl FnMut() -> Result<(), Error>,
) -> Result<Vec<Change>, Error> {
    let _span =
        tracing::info_span!("diff", from = from.features.len(), to = to.features.len()).entered();
    let from_texts = placed(from)?;
    let to_texts = placed(to)?;
    let from_keys = keys(from);
    let to_keys = keys(to);
    let from_at: BTreeMap<&Key, usize> =
        from_keys.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let to_at: BTreeMap<&Key, usize> = to_keys.iter().enumerate().map(|(i, k)| (k, i)).collect();
    // Each feature both hold, its rank among them on each side.
    let from_rank = ranks(&from_keys, &to_at);
    let to_rank = ranks(&to_keys, &from_at);

    let mut changes = Vec::new();
    for (place, (key, feature)) in to_keys.iter().zip(&to.features).enumerate() {
        go()?;
        let name: Arc<str> = Arc::from(feature.name.as_str());
        let Some(&was) = from_at.get(key) else {
            changes.push(placed_change(feature, name, None, Some(place)));
            continue;
        };
        if from_rank.get(key) != to_rank.get(key) {
            changes.push(placed_change(
                feature,
                Arc::clone(&name),
                Some(was),
                Some(place),
            ));
        }
        let (Some(before_text), Some(after_text), Some(before)) = (
            from_texts.get(was),
            to_texts.get(place),
            from.features.get(was),
        ) else {
            continue;
        };
        if before_text != after_text {
            fields(
                &mut changes,
                before,
                before_text,
                feature,
                after_text,
                &name,
            )?;
        }
    }
    for (place, (key, feature)) in from_keys.iter().zip(&from.features).enumerate() {
        if !to_at.contains_key(key) {
            changes.push(placed_change(
                feature,
                Arc::from(feature.name.as_str()),
                Some(place),
                None,
            ));
        }
    }
    Ok(changes)
}

/// Each of `document`'s features' canonical texts, as an edit writes them.
fn placed(document: &Document) -> Result<Vec<String>, Error> {
    write::placed(document, MAX_DOCUMENT_LEN)
        .map_err(|write::TooLong| super::too_long(document, "the document is too large to compare"))
}

/// Each feature's key, in timeline order.
fn keys(document: &Document) -> Vec<Key> {
    let mut seen: BTreeMap<&FeatureId, usize> = BTreeMap::new();
    document
        .features
        .iter()
        .map(|feature| {
            let n = seen.entry(&feature.id).or_insert(0);
            let key = (feature.id.clone(), *n);
            *n += 1;
            key
        })
        .collect()
}

/// The rank of each of `keys` that `other` holds too, among those, in `keys`' order.
fn ranks<'a>(keys: &'a [Key], other: &BTreeMap<&Key, usize>) -> BTreeMap<&'a Key, usize> {
    keys.iter()
        .filter(|key| other.contains_key(key))
        .enumerate()
        .map(|(rank, key)| (key, rank))
        .collect()
}

/// The change of `feature`'s place, from `before` to `after` (none where it isn't).
fn placed_change(
    feature: &Feature,
    name: Arc<str>,
    before: Option<usize>,
    after: Option<usize>,
) -> Change {
    let place = |at: Option<usize>| at.map_or(Plain::Null, |at| Plain::Number(at.to_string()));
    Change {
        feature: feature.id.clone(),
        name,
        field: None,
        before: place(before),
        after: place(after),
    }
}

/// Each field that differs between `before` and `after`, one feature on each side, whose
/// canonical texts are `before_text` and `after_text`: read as trees and walked. `name` is
/// `after`'s.
fn fields(
    changes: &mut Vec<Change>,
    before: &Feature,
    before_text: &str,
    after: &Feature,
    after_text: &str,
    name: &Arc<str>,
) -> Result<(), Error> {
    let tree = |text: &str| -> Result<Node, Error> {
        let mut tree = super::tree(text.as_bytes()).map_err(|e| unreadable(after, &e))?;
        // Compared by value, never by where each value was in its text.
        json::unplace(&mut tree);
        Ok(tree)
    };
    let (a, b) = (tree(before_text)?, tree(after_text)?);
    let sketch = [before, after]
        .iter()
        .any(|f| matches!(f.operation, Operation::Sketch(_)));
    let order = top_level_order(before, &a, after, &b);
    let mut found = Vec::new();
    walk(
        &mut found,
        "",
        Some(&a),
        Some(&b),
        Within::Feature {
            sketch,
            order: order.as_deref(),
        },
    );
    changes.extend(found.into_iter().map(|(field, was, is)| Change {
        feature: after.id.clone(),
        name: Arc::clone(name),
        field: Some(field),
        before: was,
        after: is,
    }));
    Ok(())
}

/// The order canonical form writes the top-level members of `before` and `after` in, each
/// a feature, whose trees are `a` and `b`, when each has a member the other hasn't (a
/// `suppressed` beside a `base`, a `fingerprint` beside a `note`), so neither side's order
/// says which of the two comes first: the order the writer gives a feature holding every
/// member either has ([`union`]). None where one side's order says it all, or where the
/// two are of different types, which no one feature is.
fn top_level_order(
    before: &Feature,
    a: &Node,
    after: &Feature,
    b: &Node,
) -> Option<Vec<&'static str>> {
    let (Value::Object(am), Value::Object(bm)) = (&a.value, &b.value) else {
        return None;
    };
    let only = |of: &[json::Member], other: &[json::Member]| {
        of.iter().any(|m| !other.iter().any(|o| o.key == m.key))
    };
    if !(only(am, bm) && only(bm, am)) {
        return None;
    }
    Some(write::feature_keys(&union(before, after)?))
}

/// `after`, with each member a feature may leave out that it hasn't and `before` has given
/// `before`'s: a feature holding every top-level member either has, so the writer orders
/// them all. None if the two are of different types. `before` taken apart whole, so a
/// field added to a feature is a compile error here until it's said.
fn union(before: &Feature, after: &Feature) -> Option<Feature> {
    let Feature {
        id: _,
        operation,
        version: _,
        name: _,
        suppressed,
        note,
        fingerprint,
    } = before;
    if operation.type_name() != after.operation.type_name() {
        return None;
    }
    let mut union = after.clone();
    union.suppressed |= *suppressed;
    if union.note.is_none() {
        union.note.clone_from(note);
    }
    if union.fingerprint.is_none() {
        union.fingerprint.clone_from(fingerprint);
    }
    if let (
        Operation::Pad { base, .. } | Operation::Revolve { base, .. },
        Operation::Pad { base: other, .. } | Operation::Revolve { base: other, .. },
    ) = (&mut union.operation, operation)
        && base.is_none()
    {
        base.clone_from(other);
    }
    Some(union)
}

/// A feature's canonical text that the reader doesn't read: Silico's bug.
fn unreadable(feature: &Feature, error: &Error) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!(
            "feature {}'s canonical text didn't read back: {}",
            feature.id,
            error.message()
        ),
    )
    .with_feature(feature.id.as_str())
    .with_suggestion(crate::worker::SILICO_BUG)
}

/// Where a walk is: at a feature's root (`sketch` saying whether either side is a sketch,
/// whose arrays there are its member lists, keyed by ID, and `order` the writer's order of
/// its members where neither side's says it, [`top_level_order`]), in one of a sketch's
/// member lists, or anywhere else.
#[derive(Clone, Copy)]
enum Within<'o> {
    Feature {
        sketch: bool,
        order: Option<&'o [&'static str]>,
    },
    Members,
    Other,
}

/// Each path below `path` where `a` and `b` differ, to the first level where either is a
/// scalar or absent, with each side's value there (`null` where absent), into `found`.
fn walk(
    found: &mut Vec<(String, Plain, Plain)>,
    path: &str,
    a: Option<&Node>,
    b: Option<&Node>,
    within: Within,
) {
    if a == b {
        return;
    }
    let child = |segment: &str| {
        if path.is_empty() {
            segment.to_owned()
        } else {
            format!("{path}.{segment}")
        }
    };
    match (a.map(|n| &n.value), b.map(|n| &n.value)) {
        (Some(Value::Object(am)), Some(Value::Object(bm))) => {
            let a_keys: Vec<&str> = am.iter().map(|m| m.key.as_str()).collect();
            let b_keys: Vec<&str> = bm.iter().map(|m| m.key.as_str()).collect();
            let mut keys = merged(&a_keys, &b_keys);
            if let Within::Feature {
                order: Some(order), ..
            } = within
            {
                // The order holds every member either side has ([`union`]); a key it
                // hadn't would come first, never be left out.
                keys.sort_by_key(|key| order.iter().position(|o| o == key));
            }
            for key in keys {
                // Every array at a sketch's root is one of its member lists.
                let inner = match within {
                    Within::Feature { sketch: true, .. } => Within::Members,
                    _ => Within::Other,
                };
                walk(found, &child(key), value(am, key), value(bm, key), inner);
            }
        }
        (Some(Value::Array(ai)), Some(Value::Array(bi))) => match within {
            Within::Members => {
                let a_keys = member_keys(ai);
                let b_keys = member_keys(bi);
                let a_at: BTreeMap<_, _> = a_keys.iter().zip(ai).collect();
                let b_at: BTreeMap<_, _> = b_keys.iter().zip(bi).collect();
                // Both sides' members, sorted by ID: the order the writer writes a list in.
                let keys: BTreeSet<&(String, usize)> = a_keys.iter().chain(&b_keys).collect();
                for key in keys {
                    let (segment, _) = key;
                    walk(
                        found,
                        &child(segment),
                        a_at.get(key).copied(),
                        b_at.get(key).copied(),
                        Within::Other,
                    );
                }
            }
            Within::Feature { .. } | Within::Other => {
                for i in 0..ai.len().max(bi.len()) {
                    walk(
                        found,
                        &child(&i.to_string()),
                        ai.get(i),
                        bi.get(i),
                        Within::Other,
                    );
                }
            }
        },
        _ => found.push((path.to_owned(), plain(a), plain(b))),
    }
}

/// The value of `members`' member `key`, if it has one.
fn value<'n>(members: &'n [json::Member], key: &str) -> Option<&'n Node> {
    members.iter().find(|m| m.key == key).map(|m| &m.value)
}

/// What `node` holds, `null` for none.
fn plain(node: Option<&Node>) -> Plain {
    node.map_or(Plain::Null, Plain::of)
}

/// Each of a sketch's members' keys, in the list's order: its ID (or, for one without,
/// which a document that reads never has, its place), and how many before it in the list
/// share that.
fn member_keys(items: &[Node]) -> Vec<(String, usize)> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    items
        .iter()
        .enumerate()
        .map(|(place, item)| {
            let segment = super::member_id(item)
                .map_or_else(|| place.to_string(), |id| id.as_str().to_owned());
            let n = seen.entry(segment.clone()).or_insert(0);
            let key = (segment, *n);
            *n += 1;
            key
        })
        .collect()
}

/// The keys of `a` and `b`, each once, in the order both follow: canonical form writes an
/// object's members in one order, so each side's keys are in one order and a key only one
/// side has goes where that side has it. Where neither side's order places two keys (each
/// side's only, at one point, which the writer never gives one object of one type below a
/// feature's top level: an extent or a constraint whose type changed), `a`'s comes first;
/// where the two disagree (two features of different types under one ID), `b`'s order
/// wins, and each key of `a`'s is placed as soon as `b`'s order allows.
fn merged<K: Ord + Clone>(a: &[K], b: &[K]) -> Vec<K> {
    let in_b: BTreeSet<&K> = b.iter().collect();
    let mut done: BTreeSet<&K> = BTreeSet::new();
    let mut out = Vec::with_capacity(a.len().max(b.len()));
    let (mut i, mut j) = (0, 0);
    loop {
        while a.get(i).is_some_and(|k| done.contains(k)) {
            i += 1;
        }
        while b.get(j).is_some_and(|k| done.contains(k)) {
            j += 1;
        }
        let next = match (a.get(i), b.get(j)) {
            (None, None) => break,
            (Some(x), None) => x,
            (None, Some(y)) => y,
            // `a`'s next key, if `b` hasn't it; else `b`'s next, which is `a`'s too where
            // the two agree.
            (Some(x), Some(_)) if !in_b.contains(x) => x,
            (Some(_), Some(y)) => y,
        };
        done.insert(next);
        out.push(next.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{diff_until, merged};
    use crate::{Error, ErrorCode};

    fn three_boxes(length: u32) -> super::Document {
        let boxes: Vec<String> = ["a0000001", "b0000001", "c0000001"]
            .iter()
            .map(|id| {
                format!(
                    r#"{{"id": "{id}", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": {length}, "width": 1, "height": 1}}"#
                )
            })
            .collect();
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
            boxes.join(", ")
        );
        crate::document::read(text.as_bytes()).unwrap()
    }

    #[test]
    fn go_is_asked_before_each_feature_and_its_refusal_ends_the_diff() {
        let (from, to) = (three_boxes(4), three_boxes(5));
        // Going on: asked once for each of `to`'s three features, each lengthened.
        let mut asked = 0;
        let changes = diff_until(&from, &to, || {
            asked += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!((asked, changes.len()), (3, 3));
        // Refused as the second comes: asked no more, and the refusal is the answer.
        let mut asked = 0;
        let stopped = diff_until(&from, &to, || {
            asked += 1;
            if asked == 2 {
                return Err(Error::new(ErrorCode::Cancelled, "stopped"));
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!((asked, stopped.code()), (2, ErrorCode::Cancelled));
    }

    #[test]
    fn keys_are_merged_in_the_order_both_sides_follow() {
        // By hand: a's `note` sits between `name` and `length`; b's `base` after `length`.
        let a = ["id", "name", "note", "length"];
        let b = ["id", "name", "length", "base"];
        assert_eq!(merged(&a, &b), ["id", "name", "note", "length", "base"]);
        // Disagreeing orders: each key once, b's order first.
        assert_eq!(merged(&["x", "y"], &["y", "x"]), ["y", "x"]);
        // Interleaved, by hand: k1 both; a1 only a's, before both's k2; b1 only b's, at
        // the same point as a2, a's own, so a's first; then k3 both, and b2 after it.
        assert_eq!(
            merged(
                &["k1", "a1", "k2", "a2", "k3"],
                &["k1", "k2", "b1", "k3", "b2"]
            ),
            ["k1", "a1", "k2", "a2", "b1", "k3", "b2"]
        );
        assert_eq!(merged::<&str>(&[], &[]), [] as [&str; 0]);
        assert_eq!(merged(&["a"], &[]), ["a"]);
    }
}
