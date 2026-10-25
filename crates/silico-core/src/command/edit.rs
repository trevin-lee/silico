//! Edits of a session's document (docs/design/m4.1-command-api.md, section 5): a feature
//! added, a field set, a feature removed, the whole text replaced. Each is made on the
//! document's JSON tree (`document::json::Node`) and read back by the one reader every file
//! is read by, then put through the gate, so a value is read exactly as the document reader
//! reads it and a bad edit is refused whole, with every error the gate finds. What's
//! accepted becomes the document's canonical text, read again, so the document a session
//! holds is always the one its text reads as (its sketches' lists in the text's order).
//!
//! Paths to a field follow decision 15 (the design's section 2): a sketch's members by ID
//! only, a place refused with the member's ID path as its candidate; any other array by
//! place, from 0, with no leading zeros. Nothing here holds the session's state: the
//! session decides, with the revision and the transaction, what an edit's candidate
//! becomes.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::sync::Arc;

use super::{Gated, Problems};
use crate::document::json::{Member, Node, Value};
use crate::document::{
    self, Checked, Document, ElementId, ElementKind, Feature, FeatureId, FeatureVersion,
    ID_ALPHABET, ID_LENGTH, NamingScheme, Operation, PROBLEMS_LISTED, Sketch,
};
use crate::{Error, ErrorCode};

/// The state an accepted edit gives a session's document: its canonical text, and that
/// text as the reader and the gate find it.
pub(super) struct Candidate {
    pub(super) text: Arc<Vec<u8>>,
    pub(super) gated: Arc<Gated>,
}

/// `document`, edited, through the gate, then written in canonical form and read again,
/// so the document held is the text's; or every error the gate found. `field` is the
/// request's field that holds the edit, which a document too long to write names.
fn candidate(document: Document, field: &str) -> Result<Candidate, Problems> {
    let checked = Checked::try_new(document).map_err(|(_, invalid)| Problems::from(invalid))?;
    let text = document::write(checked.document()).map_err(|e| e.with_parameter(field))?;
    // The same document, its lists in the canonical text's order: what the gate took.
    let gated = Gated::of(text.as_bytes());
    if !matches!(gated, Gated::Checked(_)) {
        return Err(gated.problems());
    }
    Ok(Candidate {
        text: Arc::new(text.into_bytes()),
        gated: Arc::new(gated),
    })
}

/// The feature `id` of `document` and its place in the timeline, or the refusal of a
/// request whose `feature` names none.
fn place_of<'a>(document: &'a Document, id: &FeatureId) -> Result<(usize, &'a Feature), Error> {
    document
        .features
        .iter()
        .enumerate()
        .find(|(_, feature)| feature.id == *id)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidInput,
                format!("the document has no feature {id}"),
            )
            .with_parameter("feature")
            .with_suggestion("name a feature by its ID, as the document holds it")
        })
}

/// `feature`'s canonical text, and its tree as the reader reads that text.
fn feature_tree(feature: &Feature) -> Result<(String, Node), Error> {
    let text = document::write_feature(feature)?;
    let tree = document::tree(text.as_bytes())?;
    Ok((text, tree))
}

/// What `set_field` found: the field's value before the edit and after it, each as the
/// document's canonical form writes it (`null` for a field the feature hasn't).
pub(super) struct FieldValues {
    pub(super) before: Node,
    pub(super) after: Node,
}

/// `field` of the feature `id` of `document` set to `value` (the design's `set_field`):
/// the values before and after, and the state it leads to, none if the feature is the
/// same as before. Refused, naming the request's field, for a feature the document hasn't,
/// a field it can't set or hasn't, a sketch's member named by its place, or an ID the edit
/// gives two members of a sketch; and with every error the reader or the gate finds.
pub(super) fn set_field(
    document: &Document,
    id: &FeatureId,
    field: &str,
    value: Node,
) -> Result<(FieldValues, Option<Candidate>), Problems> {
    let (index, feature) = place_of(document, id)?;
    let (text, tree) = feature_tree(feature)?;
    let steps = resolve(feature, &tree, field)?;
    let before = found(&tree, &steps);
    let after_field = renamed(field, &steps, &value);
    let mut edited = tree;
    put(&mut edited, &steps, value);
    let changed = document::read_feature(index, edited)?;
    if let Operation::Sketch(sketch) = &changed.operation {
        repeated_ids(sketch, &steps, field, id)?;
    }
    let (changed_text, changed_tree) = feature_tree(&changed)?;
    let after = resolve(&changed, &changed_tree, &after_field)
        .map(|steps| found(&changed_tree, &steps))
        .unwrap_or_else(|_| null());
    let values = FieldValues { before, after };
    if changed_text == text {
        return Ok((values, None));
    }
    let mut edited = document.clone();
    if let Some(slot) = edited.features.get_mut(index) {
        *slot = changed;
    }
    Ok((values, Some(candidate(edited, "value")?)))
}

/// A `null`, as a field the feature hasn't is given.
fn null() -> Node {
    Node {
        at: document::json::At { line: 1, column: 1 },
        value: Value::Null,
    }
}

/// One step of a path into a feature's tree: an object's member by its place among the
/// members, an array's item by its place, or a member the feature may have and hasn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Member(usize),
    Item(usize),
    Absent(&'static str),
}

/// The fields of a feature no edit sets: what it is (a feature of another type is a new
/// feature), its version (changed by an explicit upgrade, later), and its fingerprint (a
/// commit's to write).
fn unsettable(field: &str, feature: &Feature) -> Option<Error> {
    let (message, suggestion) = match field {
        "id" => (
            "a feature's id can't be set",
            "add a feature with the ID you want, and remove this one",
        ),
        "type" => (
            "a feature's type can't be set",
            "add a feature of the type you want, and remove this one",
        ),
        "v" => (
            "a feature's version can't be set",
            "leave v as it is: a feature's version changes only by an upgrade",
        ),
        "fingerprint" => (
            "a feature's fingerprint can't be set: a commit writes it",
            "leave fingerprint to the commit; format accepts a fingerprint's drift by name",
        ),
        _ => return None,
    };
    Some(
        Error::new(ErrorCode::InvalidInput, message)
            .with_feature(feature.id.as_str())
            .with_parameter("field")
            .with_suggestion(suggestion),
    )
}

/// The members a feature may leave out that an edit may give it: its note, and a pad's or
/// revolve's base when it has none.
fn may_add(feature: &Feature, key: &str) -> Option<&'static str> {
    match (key, &feature.operation) {
        ("note", _) => Some("note"),
        ("base", Operation::Pad { base: None, .. } | Operation::Revolve { base: None, .. }) => {
            Some("base")
        }
        _ => None,
    }
}

/// The sketch list `key` is, if `feature` is a sketch and `key` one of its lists of members.
fn member_list(feature: &Feature, key: &str) -> Option<ElementKind> {
    match feature.operation {
        Operation::Sketch(_) => ElementKind::ALL.into_iter().find(|kind| kind.list() == key),
        _ => None,
    }
}

/// `segment` as a place in an array, from 0, with no leading zeros: none for anything else.
fn place(segment: &str) -> Option<usize> {
    let digits = !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit());
    if !digits || (segment.len() > 1 && segment.starts_with('0')) {
        return None;
    }
    segment.parse().ok()
}

/// The steps `field` takes into `tree`, `feature`'s canonical tree; or the refusal of a
/// field it can't set or hasn't, or of a sketch's member named by its place.
fn resolve(feature: &Feature, tree: &Node, field: &str) -> Result<Vec<Step>, Error> {
    let segments: Vec<&str> = field.split('.').collect();
    let first = segments.first().copied().unwrap_or_default();
    if let Some(refused) = unsettable(first, feature) {
        return Err(refused);
    }
    let missing = || no_field(feature, tree, field);
    let Value::Object(members) = &tree.value else {
        return Err(missing());
    };
    let mut steps = Vec::with_capacity(segments.len());
    let mut node = match members.iter().position(|m| m.key == first) {
        Some(i) => {
            steps.push(Step::Member(i));
            members.get(i).map(|m| &m.value).ok_or_else(missing)?
        }
        None => {
            return match may_add(feature, first) {
                Some(key) if segments.len() == 1 => Ok(vec![Step::Absent(key)]),
                _ => Err(missing()),
            };
        }
    };
    let list = member_list(feature, first);
    for (depth, segment) in segments.iter().enumerate().skip(1) {
        match &node.value {
            Value::Object(members) => {
                let i = members
                    .iter()
                    .position(|m| m.key == *segment)
                    .ok_or_else(missing)?;
                steps.push(Step::Member(i));
                node = members.get(i).map(|m| &m.value).ok_or_else(missing)?;
            }
            Value::Array(items) => {
                let i = match list.filter(|_| depth == 1) {
                    Some(kind) => member(feature, kind, items, &segments)?,
                    None => place(segment)
                        .filter(|&i| i < items.len())
                        .ok_or_else(missing)?,
                };
                steps.push(Step::Item(i));
                node = items.get(i).ok_or_else(missing)?;
            }
            _ => return Err(missing()),
        }
    }
    Ok(steps)
}

/// The place in `items`, the tree's list of `kind`'s members of the sketch `feature`, of
/// the member `segments[1]` names by ID. A place there is refused, with the member's ID
/// path as the candidate when the list has a member at that place, counted in the list as
/// the session's text has it (which the held sketch's lists are in); an ID no member holds
/// is refused, with its path in the list that holds it, if another does.
fn member(
    feature: &Feature,
    kind: ElementKind,
    items: &[Node],
    segments: &[&str],
) -> Result<usize, Error> {
    let segment = segments.get(1).copied().unwrap_or_default();
    let path = |list: &str, id: &str| {
        let mut path = vec![list, id];
        path.extend(segments.iter().skip(2));
        path.join(".")
    };
    let Operation::Sketch(sketch) = &feature.operation else {
        return Err(Error::new(
            ErrorCode::Internal,
            "a sketch's list outside a sketch",
        ));
    };
    let refuse = |message: String, suggestion: String| {
        Error::new(ErrorCode::InvalidInput, message)
            .with_feature(feature.id.as_str())
            .with_parameter("field")
            .with_suggestion(suggestion)
    };
    if segment.len() < ID_LENGTH
        && let Some(at) = place(segment)
    {
        let error = refuse(
            format!(
                "{} names a {} by its place: a request names a sketch's members by ID",
                segments.join("."),
                kind.noun()
            ),
            format!(
                "name the {} by its ID, as the sketch's text gives it",
                kind.noun()
            ),
        );
        return Err(match sketch.ids(kind).get(at) {
            Some(id) => error.with_candidates([path(kind.list(), id.as_str())]),
            None => error,
        });
    }
    if let Some(i) = items
        .iter()
        .position(|item| document::member_id(item).is_some_and(|id| id.as_str() == segment))
    {
        return Ok(i);
    }
    let error = refuse(
        format!(
            "sketch {} has no {} {}",
            feature.id,
            kind.noun(),
            document::json::shown(segment)
        ),
        format!("name a {} of this sketch by its ID", kind.noun()),
    );
    let elsewhere = ElementKind::ALL
        .into_iter()
        .find(|other| sketch.ids(*other).iter().any(|id| id.as_str() == segment));
    Err(match elsewhere {
        Some(other) => error.with_candidates([path(other.list(), segment)]),
        None => error,
    })
}

/// The refusal of a field `feature` hasn't, with every field it can set as candidates.
fn no_field(feature: &Feature, tree: &Node, field: &str) -> Error {
    let what = feature.operation.type_name();
    Error::new(
        ErrorCode::InvalidInput,
        format!("a {what} has no field {}", shown_field(field)),
    )
    .with_feature(feature.id.as_str())
    .with_parameter("field")
    .with_suggestion(format!("name one of the {what}'s fields"))
    .with_candidates(fields(feature, tree))
}

/// A path as a message gives it: as written if it's plain (letters, digits, `_`, and `.`),
/// quoted as messages show text otherwise.
fn shown_field(field: &str) -> String {
    let plain = !field.is_empty()
        && field.len() <= 2 * document::json::SHOWN_CHARS
        && field
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.');
    if plain {
        field.to_owned()
    } else {
        document::json::shown(field)
    }
}

/// Every field of `feature` an edit can set, in the order the design lists them: its name
/// and note, then its operation's fields in schema order, each object's members after it;
/// a list's members aren't listed (a sketch's by ID would be thousands).
fn fields(feature: &Feature, tree: &Node) -> Vec<String> {
    let mut fields = vec!["name".to_owned(), "note".to_owned()];
    if may_add(feature, "base").is_some() {
        fields.push("base".to_owned());
    }
    if let Value::Object(members) = &tree.value {
        for member in members {
            if !matches!(
                member.key.as_str(),
                "id" | "type" | "v" | "name" | "note" | "fingerprint"
            ) {
                listed(&member.key, &member.value, &mut fields);
            }
        }
    }
    fields
}

/// `path`, and each member of `node` below it if it's an object.
fn listed(path: &str, node: &Node, fields: &mut Vec<String>) {
    fields.push(path.to_owned());
    if let Value::Object(members) = &node.value {
        for member in members {
            listed(&format!("{path}.{}", member.key), &member.value, fields);
        }
    }
}

/// The node `steps` reach in `tree`, as it is; `null` for a member the feature hasn't.
fn found(tree: &Node, steps: &[Step]) -> Node {
    let mut node = tree;
    for step in steps {
        let next = match (step, &node.value) {
            (Step::Member(i), Value::Object(members)) => members.get(*i).map(|m| &m.value),
            (Step::Item(i), Value::Array(items)) => items.get(*i),
            _ => None,
        };
        match next {
            Some(next) => node = next,
            None => return null(),
        }
    }
    node.clone()
}

/// `value` put where `steps` reach in `tree`: in place of what's there, or as a member
/// added; a note set to `null` removed (the design's section 5).
fn put(tree: &mut Node, steps: &[Step], value: Node) {
    let removes_note = |key: &str, value: &Node| key == "note" && value.value == Value::Null;
    let Some((last, path)) = steps.split_last() else {
        return;
    };
    let mut node = tree;
    for step in path {
        let next = match (step, &mut node.value) {
            (Step::Member(i), Value::Object(members)) => members.get_mut(*i).map(|m| &mut m.value),
            (Step::Item(i), Value::Array(items)) => items.get_mut(*i),
            _ => None,
        };
        match next {
            Some(next) => node = next,
            None => return,
        }
    }
    match (last, &mut node.value) {
        (Step::Absent(key), Value::Object(members)) => {
            if !removes_note(key, &value) {
                members.push(Member {
                    key: (*key).to_owned(),
                    at: value.at,
                    value,
                });
            }
        }
        (Step::Member(i), Value::Object(members)) => {
            let note = path.is_empty()
                && members
                    .get(*i)
                    .is_some_and(|m| removes_note(&m.key, &value));
            if note {
                members.remove(*i);
            } else if let Some(member) = members.get_mut(*i) {
                member.value = value;
            }
        }
        (Step::Item(i), Value::Array(items)) => {
            if let Some(item) = items.get_mut(*i) {
                *item = value;
            }
        }
        _ => {}
    }
}

/// `field` as the edited feature names what it set: a sketch's member whose ID the edit
/// changed (a whole member set, or its `id`) is found by its new ID.
fn renamed(field: &str, steps: &[Step], value: &Node) -> String {
    let segments: Vec<&str> = field.split('.').collect();
    let new_id = match (steps, segments.as_slice()) {
        ([Step::Member(_), Step::Item(_)], [_, _]) => document::member_id(value),
        ([Step::Member(_), Step::Item(_), Step::Member(_)], [_, _, "id"]) => {
            value.value.as_str().and_then(ElementId::new)
        }
        _ => None,
    };
    match (new_id, segments.as_slice()) {
        (Some(id), [list, _, rest @ ..]) => {
            let mut path = vec![*list, id.as_str()];
            path.extend(rest);
            path.join(".")
        }
        _ => field.to_owned(),
    }
}

/// What part of a sketch an edit set: one of its lists whole, or one member of a list (at
/// its place in the edited list).
#[derive(Clone, Copy)]
enum Region {
    List(ElementKind),
    Member(ElementKind, usize),
}

impl Region {
    /// The region `steps` set, if they set one that can change an ID: a list, a member, or
    /// a member's `id`.
    fn of(steps: &[Step], field: &str) -> Option<Region> {
        let segments: Vec<&str> = field.split('.').collect();
        let kind = ElementKind::ALL
            .into_iter()
            .find(|kind| segments.first() == Some(&kind.list()))?;
        match (steps, segments.as_slice()) {
            ([Step::Member(_)], [_]) => Some(Region::List(kind)),
            ([Step::Member(_), Step::Item(i)], [_, _])
            | ([Step::Member(_), Step::Item(i), Step::Member(_)], [_, _, "id"]) => {
                Some(Region::Member(kind, *i))
            }
            _ => None,
        }
    }

    fn holds(self, kind: ElementKind, index: usize) -> bool {
        match self {
            Region::List(list) => list == kind,
            Region::Member(list, at) => list == kind && at == index,
        }
    }
}

/// The refusal of an edit that gives an ID to two members of `sketch` (the edited sketch of
/// feature `feature`), each repeated ID refused at the request's `field`, the member that
/// already holds it in `elements` and the message (the design's section 2): never at the
/// untouched member the gate would name by its place. The sketch before the edit held each
/// ID once, so a repeat is the edit's.
fn repeated_ids(
    sketch: &Sketch,
    steps: &[Step],
    field: &str,
    feature: &FeatureId,
) -> Result<(), Problems> {
    let Some(region) = Region::of(steps, field) else {
        return Ok(());
    };
    let mut holders: BTreeMap<&ElementId, Vec<(ElementKind, usize)>> = BTreeMap::new();
    let mut order = Vec::new();
    for kind in ElementKind::ALL {
        for (index, id) in sketch.ids(kind).into_iter().enumerate() {
            let held = holders.entry(id).or_default();
            if held.len() == 1 {
                order.push(id);
            }
            held.push((kind, index));
        }
    }
    let mut errors = Vec::new();
    let mut unlisted = 0u64;
    for id in order {
        if errors.len() >= PROBLEMS_LISTED {
            unlisted += 1;
            continue;
        }
        let held = holders.get(id).map(Vec::as_slice).unwrap_or_default();
        let outside = held
            .iter()
            .find(|(kind, index)| !region.holds(*kind, *index));
        let edited = held
            .iter()
            .find(|(kind, index)| region.holds(*kind, *index))
            .map(|(kind, _)| *kind);
        let noun = edited.map(ElementKind::noun).unwrap_or("member");
        let error = match outside {
            Some((holder, _)) => Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "id {id} is already the ID of {} {id} ({}.{id})",
                    holder.noun(),
                    holder.list()
                ),
            )
            .with_suggestion(format!("give this {noun} an ID of its own")),
            None => Error::new(
                ErrorCode::InvalidInput,
                format!("id {id} is given to more than one {noun} of {field}"),
            )
            .with_suggestion(format!("give each {noun} an ID of its own")),
        };
        errors.push(
            error
                .with_feature(feature.as_str())
                .with_element(id.as_str())
                .with_parameter(field),
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Problems::with_unlisted(errors, unlisted))
    }
}

/// `feature`, a feature as the document writes it, added to the end of `document`'s
/// timeline (the design's `add_feature`): with the ID `new_id` makes if it gives none, and
/// the feature type's current version if it gives no `v`; the feature as the document then
/// holds it, and the state it leads to. Refused for a feature that isn't an object or gives
/// a fingerprint (a commit writes it), and with every error the reader or the gate finds
/// in the document with it added.
pub(super) fn add_feature(
    document: &Document,
    feature: Node,
    new_id: impl FnOnce(&Document) -> Result<FeatureId, Error>,
) -> Result<(Feature, Candidate), Problems> {
    let Node { at, value } = feature;
    let Value::Object(mut members) = value else {
        return Err(Problems::from(
            Error::new(
                ErrorCode::InvalidInput,
                format!("feature is {}, not an object", value.kind()),
            )
            .with_parameter("feature")
            .with_suggestion("give the feature as the document writes it: an object"),
        ));
    };
    if members.iter().any(|m| m.key == "fingerprint") {
        return Err(Problems::from(
            Error::new(
                ErrorCode::InvalidInput,
                "a new feature has no fingerprint: a commit writes it",
            )
            .with_parameter("feature.fingerprint")
            .with_suggestion("leave fingerprint out"),
        ));
    }
    if !members.iter().any(|m| m.key == "id") {
        let id = new_id(document)?;
        members.insert(
            0,
            Member {
                key: "id".to_owned(),
                at,
                value: Node {
                    at,
                    value: Value::String(id.to_string()),
                },
            },
        );
    }
    if !members.iter().any(|m| m.key == "v") {
        members.push(Member {
            key: "v".to_owned(),
            at,
            value: Node {
                at,
                value: Value::Number(FeatureVersion::NUMBER.to_string()),
            },
        });
    }
    let index = document.features.len();
    let added = document::read_feature(
        index,
        Node {
            at,
            value: Value::Object(members),
        },
    )?;
    let mut edited = document.clone();
    edited.features.push(added);
    let candidate = candidate(edited, "feature")?;
    // As the document now holds it: the text read again.
    let held = candidate
        .gated
        .document()
        .ok()
        .and_then(|document| document.features.get(index))
        .cloned()
        .ok_or_else(|| {
            Problems::from(
                Error::new(
                    ErrorCode::Internal,
                    "the feature added isn't in the document",
                )
                .with_suggestion(crate::worker::SILICO_BUG),
            )
        })?;
    Ok((held, candidate))
}

/// The feature `id` removed from `document` (the design's `remove_feature`): the feature as
/// the document held it, its place in the timeline, from 0, and the state it leads to.
/// Refused for a feature the document hasn't, and with every error the gate finds in the
/// document without it: each feature that used it, naming it.
pub(super) fn remove_feature(
    document: &Document,
    id: &FeatureId,
) -> Result<(Feature, usize, Candidate), Problems> {
    let (index, removed) = place_of(document, id)?;
    let removed = removed.clone();
    let mut edited = document.clone();
    edited.features.retain(|feature| feature.id != *id);
    Ok((removed, index, candidate(edited, "feature")?))
}

/// The document's text replaced by `text` (the design's `replace_text`), put through the
/// reader and the gate as `silico check` puts a file through them: the new document's
/// number of features, and the state it leads to, none if it's the document `before` holds
/// already. Refused with what the reader or the gate says of the text (the reader's own
/// refusals naming `text`), and if its header has no `naming`: the reader takes a document
/// without it as one saved before M4.0, and text written now isn't one.
pub(super) fn replace_text(
    before: &Gated,
    text: &[u8],
) -> Result<(usize, Option<Candidate>), Problems> {
    let of_text = |e: Error| match e.parameter() {
        Some(_) => e,
        None => e.with_parameter("text"),
    };
    let root = document::tree(text).map_err(of_text)?;
    if let Value::Object(members) = &root.value
        && !members.iter().any(|m| m.key == "naming")
    {
        return Err(Problems::from(
            Error::new(ErrorCode::InvalidInput, "the document's header has no naming")
                .with_parameter("naming")
                .with_cause(
                    "only a document saved before the header recorded its naming scheme is read without it",
                )
                .with_suggestion(format!(
                    "add \"naming\": {} after units",
                    NamingScheme::NUMBER
                )),
        ));
    }
    let replaced = document::read_tree(root).map_err(of_text)?;
    let features = replaced.features.len();
    let candidate = candidate(replaced, "text")?;
    let unchanged = match before.document() {
        Ok(before) => document::write(before).is_ok_and(|was| was.as_bytes() == *candidate.text),
        Err(_) => false,
    };
    Ok((features, (!unchanged).then_some(candidate)))
}

/// Where new feature IDs come from (M2.1, decision 20): the system's randomness, five bytes
/// from `/dev/urandom` (as the cache reads its secret) for the eight characters of an ID,
/// or, in this crate's own tests only, bytes they give (never through the API, the Rust
/// API, or a request).
pub(super) struct FeatureIds {
    #[cfg(test)]
    given: Option<std::collections::VecDeque<[u8; 5]>>,
}

/// The most IDs made for one feature before giving up: the chance that 2⁴⁰ IDs a document
/// holds a hundred thousand of take even two in a row is about 1e-14.
const ID_TRIES: usize = 16;

impl FeatureIds {
    /// IDs from the system's randomness.
    pub(super) fn system() -> Self {
        FeatureIds {
            #[cfg(test)]
            given: None,
        }
    }

    /// IDs from `bytes`, in order, for tests: each five bytes the ID they map to.
    #[cfg(test)]
    pub(super) fn given(bytes: impl IntoIterator<Item = [u8; 5]>) -> Self {
        FeatureIds {
            given: Some(bytes.into_iter().collect()),
        }
    }

    /// Five bytes of the source's.
    fn bytes(&mut self) -> Result<[u8; 5], Error> {
        #[cfg(test)]
        if let Some(given) = &mut self.given {
            return given
                .pop_front()
                .ok_or_else(|| Error::new(ErrorCode::Internal, "the test gave no more ID bytes"));
        }
        let mut bytes = [0; 5];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut bytes))
            .map_err(|e| {
                Error::new(
                    ErrorCode::IoFailed,
                    format!("the system's randomness couldn't be read for a feature's ID: {e}"),
                )
                .with_parameter("feature.id")
                .with_suggestion("give the feature an id, or add it again")
            })?;
        Ok(bytes)
    }

    /// A new ID that no feature of `document` holds.
    pub(super) fn next_free(&mut self, document: &Document) -> Result<FeatureId, Error> {
        for _ in 0..ID_TRIES {
            let id = id_of(self.bytes()?)?;
            if !document.features.iter().any(|feature| feature.id == id) {
                return Ok(id);
            }
        }
        Err(Error::new(
            ErrorCode::Internal,
            format!("{ID_TRIES} new feature IDs in a row were IDs the document holds"),
        )
        .with_suggestion(crate::worker::SILICO_BUG))
    }
}

/// The ID five bytes map to: their 40 bits, most significant first, five at a time, each
/// the place of its character in [`ID_ALPHABET`].
fn id_of(bytes: [u8; 5]) -> Result<FeatureId, Error> {
    let bits = bytes
        .iter()
        .fold(0u64, |bits, &byte| (bits << 8) | u64::from(byte));
    let alphabet = ID_ALPHABET.as_bytes();
    let text: String = (0..ID_LENGTH)
        .rev()
        .filter_map(|i| alphabet.get(((bits >> (5 * i)) & 0x1f) as usize))
        .map(|&b| char::from(b))
        .collect();
    FeatureId::new(&text).ok_or_else(|| {
        Error::new(
            ErrorCode::Internal,
            format!("{text:?} made from random bytes isn't an ID"),
        )
        .with_suggestion(crate::worker::SILICO_BUG)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_bytes_map_onto_the_alphabet_five_bits_a_character() {
        // By hand: all bits clear is the first character eight times, all set the last.
        assert_eq!(id_of([0; 5]).unwrap().as_str(), "00000000");
        assert_eq!(id_of([0xff; 5]).unwrap().as_str(), "zzzzzzzz");
        // 10 to 17 (a to h, i skipped): 01010 01011 01100 01101 01110 01111 10000 10001.
        assert_eq!(
            id_of([0x52, 0xd8, 0xd7, 0x3e, 0x11]).unwrap().as_str(),
            "abcdefgh"
        );
        // 18 to 25 (j k m n p q r s): 10010 10011 10100 10101 10110 10111 11000 11001.
        assert_eq!(
            id_of([0x94, 0xe9, 0x5b, 0x5f, 0x19]).unwrap().as_str(),
            "jkmnpqrs"
        );
    }

    #[test]
    fn a_new_id_is_never_one_the_document_holds() {
        let held = |ids: &[&str]| {
            Document {
            silico: document::FormatVersion,
            units: document::Units,
            naming: NamingScheme,
            features: ids
                .iter()
                .map(|id| {
                    document::read_feature(
                        0,
                        document::tree(
                            format!(r#"{{"id": "{id}", "type": "sphere", "v": 1, "name": "B", "position": [0, 0, 0], "radius": 1}}"#)
                                .as_bytes(),
                        )
                        .unwrap(),
                    )
                    .unwrap()
                })
                .collect(),
        }
        };
        let mut ids = FeatureIds::given([[0; 5], [0xff; 5], [0; 5]]);
        let document = held(&["00000000"]);
        assert_eq!(ids.next_free(&document).unwrap().as_str(), "zzzzzzzz");
        // A source that keeps giving held IDs ends, refused.
        let mut ids = FeatureIds::given([[0; 5]; ID_TRIES]);
        assert_eq!(
            ids.next_free(&document).unwrap_err().code(),
            ErrorCode::Internal
        );
    }
}
