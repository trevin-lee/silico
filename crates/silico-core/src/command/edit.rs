//! Edits of a session's document (docs/design/m4.1-command-api.md, section 5): a feature
//! added, a field set, a feature removed, the whole text replaced. A feature added and a
//! field set are made on a feature's JSON tree (`document::json::Node`): the request's
//! feature, or the feature's canonical tree with the request's value put where its field
//! is, read back by the one reader every file is read by, with no place in a text (so its
//! errors name fields by their paths alone). A feature is removed from the document as it
//! is; a text replaced is read as a file is. Each edited document is then written in
//! canonical form, and that text put through the reader and the gate, once: a bad edit is
//! refused whole, with every error the gate finds, places counted as the edited document
//! has them (the request's or the text's own order, not the canonical text's).
//! What's accepted is that text and the document it reads as, so the document a session
//! holds after an edit is always its canonical text's (its sketches' lists in ID order).
//!
//! Paths to a field follow decision 15 (the design's section 2): a sketch's members by ID
//! only, a place refused with the member's ID path as its candidate; any other array by
//! place, from 0, with no leading zeros. Nothing here holds the session's state: the
//! session decides, with the revision and the transaction, what an edit's candidate
//! becomes.

use std::sync::Arc;

use super::{Gated, Problems};
use crate::document::json::{self, Member, Node, Plain, Value};
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

/// `document`, edited, written in canonical form, and that text through the reader and the
/// gate, once: so the document held is the text's. Or, refused, every error the gate finds
/// in `document` as given, its places counted in its own order (a sketch's members as the
/// request or the text gave them, as `silico check` counts a file's), not the canonical
/// text's; or the canonical text's errors, if the gate takes `document` as given. `field`
/// is the request's field that holds the edit, which a document too long to write names.
fn candidate(document: &Document, field: &str) -> Result<Candidate, Problems> {
    let text = document::write(document).map_err(|e| e.with_parameter(field))?;
    let gated = Gated::of(text.as_bytes());
    if !matches!(gated, Gated::Checked(_)) {
        return Err(match Checked::try_new(document.clone()) {
            Err((_, invalid)) => Problems::from(invalid),
            Ok(_) => gated.problems(),
        });
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
/// document's canonical form writes it (`null` for a field the feature hasn't), and the path
/// that names the field after it (a sketch member's, by its new ID if the edit gave one), by
/// which a commit's saved document gives it back ([`value_of`]).
pub(super) struct FieldValues {
    pub(super) before: Plain,
    pub(super) after: Plain,
    pub(super) path: String,
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
    mut value: Node,
) -> Result<(FieldValues, Option<Candidate>), Problems> {
    let (index, feature) = place_of(document, id)?;
    let (text, tree) = feature_tree(feature)?;
    let target = resolve(feature, &tree, field)?;
    let before = found(&tree, &target.steps);
    let after_field = renamed(field, &target, &value);
    let mut edited = tree;
    json::unplace(&mut edited);
    json::unplace(&mut value);
    put(feature, &mut edited, &target.steps, value);
    let set = document::read_feature(index, edited)?;
    if let (Operation::Sketch(sketch), Some(sketched)) = (&set.operation, target.sketched) {
        repeated_ids(sketch, sketched, field, id)?;
    }
    let (set_text, set_tree) = feature_tree(&set)?;
    // Found again by the path that names it now: a field removed (a note, a base) is
    // there to give, as `null`, so any other miss is Silico's.
    let after = resolve(&set, &set_tree, &after_field)
        .map(|target| found(&set_tree, &target.steps))
        .map_err(|e| {
            Error::new(
                ErrorCode::Internal,
                format!(
                    "the field set, {}, isn't in the feature it was set in: {}",
                    shown_field(&after_field),
                    e.message()
                ),
            )
            .with_suggestion(crate::worker::SILICO_BUG)
        })?;
    let values = FieldValues {
        before,
        after,
        path: after_field,
    };
    if set_text == text {
        return Ok((values, None));
    }
    let mut edited = document.clone();
    if let Some(slot) = edited.features.get_mut(index) {
        *slot = set;
    }
    Ok((values, Some(candidate(&edited, "value")?)))
}

/// The value of `field` of the feature `id` as `document` holds it, as `set_field` finds a
/// field's: how a commit's saved document gives back the value an edit set, with the solve
/// it wrote. None if the document hasn't the feature, or the feature the field.
pub(super) fn value_of(document: &Document, id: &FeatureId, field: &str) -> Option<Plain> {
    let (_, feature) = place_of(document, id).ok()?;
    let (_, tree) = feature_tree(feature).ok()?;
    let target = resolve(feature, &tree, field).ok()?;
    Some(found(&tree, &target.steps))
}

/// One step of a path into a feature's tree: an object's member by its place among the
/// members, an array's item by its place, or a member the feature may have and hasn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Member(usize),
    Item(usize),
    Absent(&'static str),
}

/// Where a field's path leads in a feature's tree ([`resolve`]): the steps there, and, if
/// it names one of a sketch's lists whole, a member whole, or a member's `id`, which: the
/// fields an edit can give an ID another member holds, and whose path names what it set
/// by the member's new ID afterwards.
struct Target {
    steps: Vec<Step>,
    sketched: Option<Sketched>,
}

/// What part of a sketch a path names, of the parts that hold IDs: one of its lists
/// whole, a member whole, or a member's `id`, each member at its place in the tree's list.
#[derive(Clone, Copy)]
enum Sketched {
    List(ElementKind),
    Member(ElementKind, usize),
    MemberId(ElementKind, usize),
}

impl Sketched {
    /// Whether the member at `index` of `kind`'s list is in the part named.
    fn holds(self, kind: ElementKind, index: usize) -> bool {
        match self {
            Sketched::List(list) => list == kind,
            Sketched::Member(list, at) | Sketched::MemberId(list, at) => {
                list == kind && at == index
            }
        }
    }
}

/// Why `field`, a feature's own field, can't be set, and what to do instead: what it is
/// (a feature of another type is a new feature), its version (changed by an explicit
/// upgrade, later), and its fingerprint (a commit's to write). None for any other field.
fn unsettable(field: &str) -> Option<(&'static str, &'static str)> {
    Some(match field {
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
    })
}

/// The member `key` of `feature`, if it's one the feature may leave out
/// ([`document::optional_members`]): one an edit may give it, and take away with `null`.
fn optional(feature: &Feature, key: &str) -> Option<&'static str> {
    document::optional_members(feature.operation.type_name())
        .iter()
        .find(|optional| **optional == key)
        .copied()
}

/// The sketch, and the kind of its members the list `key` holds, if `feature` is a sketch
/// and `key` one of its lists of members.
fn member_list<'a>(feature: &'a Feature, key: &str) -> Option<(&'a Sketch, ElementKind)> {
    match &feature.operation {
        Operation::Sketch(sketch) => ElementKind::ALL
            .into_iter()
            .find(|kind| kind.list() == key)
            .map(|kind| (sketch, kind)),
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

/// Where `field` leads in `tree`, `feature`'s canonical tree; or the refusal of a field
/// it can't set or hasn't, or of a sketch's member named by its place. A path of more
/// segments than a tree nests deep is refused before it's split.
fn resolve(feature: &Feature, tree: &Node, field: &str) -> Result<Target, Error> {
    let first = field.split('.').next().unwrap_or_default();
    if let Some((message, suggestion)) = unsettable(first) {
        return Err(Error::new(ErrorCode::InvalidInput, message)
            .with_feature(feature.id.as_str())
            .with_parameter("field")
            .with_suggestion(suggestion));
    }
    let missing = || no_field(feature, tree, field);
    if field.split('.').nth(json::MAX_DEPTH).is_some() {
        return Err(missing());
    }
    let segments: Vec<&str> = field.split('.').collect();
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
            return match optional(feature, first) {
                Some(key) if segments.len() == 1 => Ok(Target {
                    steps: vec![Step::Absent(key)],
                    sketched: None,
                }),
                _ => Err(missing()),
            };
        }
    };
    let list = member_list(feature, first);
    let mut sketched = match (list, segments.len()) {
        (Some((_, kind)), 1) => Some(Sketched::List(kind)),
        _ => None,
    };
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
                    Some((sketch, kind)) => {
                        let i = member(feature, sketch, kind, items, &segments, field)?;
                        sketched = match segments.get(2..) {
                            Some([]) => Some(Sketched::Member(kind, i)),
                            Some(["id"]) => Some(Sketched::MemberId(kind, i)),
                            _ => None,
                        };
                        i
                    }
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
    Ok(Target { steps, sketched })
}

/// The place in `items`, the tree's list of `kind`'s members of `sketch` (the sketch
/// `feature`), of the member `segments[1]` of `field` names by ID. A place there is
/// refused, with the member's ID path as the candidate when the list has a member at that
/// place, counted in the list as the session's text has it (which the held sketch's lists
/// are in); an ID no member holds is refused, with its path in the list that holds it, if
/// another does. A candidate's path goes on with the rest of `field` only if that's plain
/// ([`plain`]); otherwise it's the member's alone.
fn member(
    feature: &Feature,
    sketch: &Sketch,
    kind: ElementKind,
    items: &[Node],
    segments: &[&str],
    field: &str,
) -> Result<usize, Error> {
    let segment = segments.get(1).copied().unwrap_or_default();
    let rest = segments.get(2..).unwrap_or_default();
    let path = |list: &str, id: &str| {
        let mut path = vec![list, id];
        if rest.iter().all(|segment| plain(segment)) {
            path.extend(rest);
        }
        path.join(".")
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
                "{} names {} by its place: a request names a sketch's members by ID",
                shown_field(field),
                kind.a_noun()
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
            json::shown(segment)
        ),
        format!("name {} of this sketch by its ID", kind.a_noun()),
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

/// The longest path a message repeats as written: two names' worth, past any path a
/// feature has (a sketch member's `constraints.k0000002.value` is 26).
const SHOWN_PATH_CHARS: usize = 2 * json::SHOWN_CHARS;

/// Whether `segment`, one name of a path, is written plainly: letters, digits, and `_`, at
/// most [`json::SHOWN_CHARS`] of them.
fn plain(segment: &str) -> bool {
    segment.len() <= json::SHOWN_CHARS
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A path as a message gives it: as written if it's plain (letters, digits, `_`, and `.`,
/// at most [`SHOWN_PATH_CHARS`]), quoted as messages show text otherwise.
fn shown_field(field: &str) -> String {
    let plain = !field.is_empty()
        && field.len() <= SHOWN_PATH_CHARS
        && field
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.');
    if plain {
        field.to_owned()
    } else {
        json::shown(field)
    }
}

/// Every field of `feature` an edit can set, in the order the design lists them: its name
/// and note, a member it may have and hasn't (a pad's base), then its operation's fields
/// in schema order, each object's members after it; a list's members aren't listed (a
/// sketch's by ID would be thousands).
fn fields(feature: &Feature, tree: &Node) -> Vec<String> {
    let members = match &tree.value {
        Value::Object(members) => members.as_slice(),
        _ => &[],
    };
    let mut fields = vec!["name".to_owned(), "note".to_owned()];
    for key in document::optional_members(feature.operation.type_name()) {
        if !fields.iter().any(|f| f.as_str() == *key) && !members.iter().any(|m| m.key == *key) {
            fields.push((*key).to_owned());
        }
    }
    for member in members {
        if unsettable(&member.key).is_none() && !fields.contains(&member.key) {
            listed(&member.key, &member.value, &mut fields);
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

/// What `steps` reach in `tree`, as it is; `null` for a member the feature hasn't.
fn found(tree: &Node, steps: &[Step]) -> Plain {
    let mut node = tree;
    for step in steps {
        let next = match (step, &node.value) {
            (Step::Member(i), Value::Object(members)) => members.get(*i).map(|m| &m.value),
            (Step::Item(i), Value::Array(items)) => items.get(*i),
            _ => None,
        };
        match next {
            Some(next) => node = next,
            None => return Plain::Null,
        }
    }
    Plain::of(node)
}

/// `value` put where `steps` reach in `tree`, the tree of `feature`: in place of what's
/// there, or as a member added; a member the feature may leave out (its note, a pad's
/// base) set to `null` removed (the design's section 5).
fn put(feature: &Feature, tree: &mut Node, steps: &[Step], value: Node) {
    let removes =
        |key: &str, value: &Node| value.value == Value::Null && optional(feature, key).is_some();
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
            if !removes(key, &value) {
                members.push(Member {
                    key: (*key).to_owned(),
                    at: json::At::UNPLACED,
                    value,
                });
            }
        }
        (Step::Member(i), Value::Object(members)) => {
            let removed =
                path.is_empty() && members.get(*i).is_some_and(|m| removes(&m.key, &value));
            if removed {
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

/// `field` as the edited feature names what it set (`target`, set to `value`): a sketch's
/// member whose ID the edit changed (a whole member set, or its `id`) is found by its new
/// ID.
fn renamed(field: &str, target: &Target, value: &Node) -> String {
    let renamed = match target.sketched {
        Some(Sketched::Member(kind, _)) => {
            document::member_id(value).map(|id| format!("{}.{id}", kind.list()))
        }
        Some(Sketched::MemberId(kind, _)) => value
            .value
            .as_str()
            .and_then(ElementId::new)
            .map(|id| format!("{}.{id}.id", kind.list())),
        Some(Sketched::List(_)) | None => None,
    };
    renamed.unwrap_or_else(|| field.to_owned())
}

/// The refusal of an edit that gives an ID to two members of `sketch` (the edited sketch of
/// feature `feature`), the part of it `sketched` the request's `field` set: each repeated
/// ID ([`document::repeats`]) refused at `field`, the member that already holds it in
/// `elements` and the message (the design's section 2), never at the untouched member the
/// gate would name by its place. The sketch before the edit held each ID once, so a
/// repeat is the edit's; one it didn't touch is left to the gate.
fn repeated_ids(
    sketch: &Sketch,
    sketched: Sketched,
    field: &str,
    feature: &FeatureId,
) -> Result<(), Problems> {
    let mut errors = Vec::new();
    let mut unlisted = 0u64;
    for repeat in document::repeats(sketch) {
        let id = repeat.id;
        let Some(&(kind, _)) = repeat
            .holders
            .iter()
            .find(|(kind, index)| sketched.holds(*kind, *index))
        else {
            continue;
        };
        if errors.len() >= PROBLEMS_LISTED {
            unlisted += 1;
            continue;
        }
        let outside = repeat
            .holders
            .iter()
            .find(|(kind, index)| !sketched.holds(*kind, *index));
        let error = match outside {
            Some(&(holder, _)) => Error::new(
                ErrorCode::InvalidInput,
                document::id_held(id, holder, &format!("{}.{id}", holder.list())),
            )
            .with_suggestion(document::an_id_of_its_own(kind)),
            None => Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "id {id} is given to more than one {} of {}",
                    kind.noun(),
                    shown_field(field)
                ),
            )
            .with_suggestion(format!("give each {} an ID of its own", kind.noun())),
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
/// the feature type's current version if it gives no `v` (each `null` given as none, as is
/// a `null` fingerprint, note, or other member the feature may leave out: the design's
/// section 2); the feature as the document then holds it, and the state it leads
/// to. Refused for a feature that isn't an object (in the words the wire refuses one in,
/// before it gets here: this is for the Rust API's callers), gives a fingerprint (a commit
/// writes it), or gives an ID that isn't one, or that the document holds (each naming
/// `feature.id`); and with every error the reader or the gate finds in the document with
/// it added.
pub(super) fn add_feature(
    document: &Document,
    feature: Node,
    new_id: impl FnOnce(&Document) -> Result<FeatureId, Error>,
) -> Result<(Feature, Candidate), Problems> {
    let Value::Object(mut members) = feature.value else {
        return Err(Problems::from(
            Error::new(
                ErrorCode::InvalidInput,
                format!("feature must be an object, got {}", feature.value.kind()),
            )
            .with_parameter("feature")
            .with_suggestion("give the feature as the document writes it: an object"),
        ));
    };
    // A member given as `null` that the feature may leave out is left out (the design's
    // section 2): its ID, its version, its fingerprint, and the members of its type that
    // may be ([`document::optional_members`]).
    let type_name = members
        .iter()
        .find(|m| m.key == "type")
        .and_then(|m| m.value.value.as_str())
        .map(str::to_owned);
    let optional = type_name
        .as_deref()
        .map_or(&[][..], document::optional_members);
    members.retain(|m| {
        !(m.value.value == Value::Null
            && (matches!(m.key.as_str(), "id" | "v" | "fingerprint")
                || optional.contains(&m.key.as_str())))
    });
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
    let unplaced = |value: Value| Node {
        at: json::At::UNPLACED,
        value,
    };
    match members.iter().find(|m| m.key == "id") {
        None => {
            let id = new_id(document)?;
            members.insert(
                0,
                Member {
                    key: "id".to_owned(),
                    at: json::At::UNPLACED,
                    value: unplaced(Value::String(id.to_string())),
                },
            );
        }
        Some(given) => {
            if let Some(holder) = given
                .value
                .value
                .as_str()
                .and_then(FeatureId::new)
                .and_then(|id| document.features.iter().find(|f| f.id == id))
            {
                return Err(Problems::from(
                    Error::new(
                        ErrorCode::InvalidInput,
                        format!(
                            "id {} is already the ID of feature {} ({})",
                            holder.id,
                            holder.id,
                            json::shown(holder.name.as_str())
                        ),
                    )
                    .with_parameter("feature.id")
                    .with_element(holder.id.as_str())
                    .with_suggestion(
                        "give the feature an ID the document doesn't hold, or leave id out for a new one",
                    ),
                ));
            }
        }
    }
    if !members.iter().any(|m| m.key == "v") {
        members.push(Member {
            key: "v".to_owned(),
            at: json::At::UNPLACED,
            value: unplaced(Value::Number(FeatureVersion::NUMBER.to_string())),
        });
    }
    let index = document.features.len();
    let mut node = unplaced(Value::Object(members));
    json::unplace(&mut node);
    let place = format!("features[{index}]");
    let added = document::read_feature(index, node).map_err(|e| {
        // The request's own field: an ID that isn't one is the request's `feature.id`. The
        // reader names a feature by its place only until it has read its ID, and the feature
        // is an object, so every error that names its place is its `id`'s.
        if e.feature() == Some(place.as_str()) {
            e.with_parameter("feature.id")
        } else {
            e
        }
    })?;
    let mut edited = document.clone();
    edited.features.push(added);
    let candidate = candidate(&edited, "feature")?;
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
    Ok((removed, index, candidate(&edited, "feature")?))
}

/// The document's text replaced by `text` (the design's `replace_text`), put through the
/// reader and the gate as `silico check` puts a file through them: the new document's
/// number of features, and the state it leads to, none if it's the document `before` holds
/// already, as canonical form writes them ([`Document::canonically_eq`]). Refused with
/// what the reader or the gate says of the text (the reader's own refusals naming `text`),
/// and if its header has no `naming`: the reader takes a document without it as one saved
/// before M4.0, and text written now isn't one.
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
    let candidate = candidate(&replaced, "text")?;
    let unchanged = before
        .document()
        .is_ok_and(|before| before.canonically_eq(&replaced));
    Ok((features, (!unchanged).then_some(candidate)))
}

/// Where new feature IDs come from (M2.1, decision 20): the system's randomness, five bytes
/// (as the cache's secret is read, [`crate::system_random`]) for the eight characters of an
/// ID, or, in this crate's own tests only, bytes they give (never through the API, the
/// Rust API, or a request).
pub(super) struct FeatureIds {
    #[cfg(test)]
    given: Option<std::collections::VecDeque<[u8; 5]>>,
}

/// The most IDs made for one feature before giving up. Each is one of 2⁴⁰, so one made
/// is an ID a document of a hundred thousand features holds with a chance of 1e5 / 2⁴⁰,
/// about 1e-7, and two in a row about 1e-14: sixteen in a row is a source that isn't
/// random.
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
        crate::system_random().map_err(|e| {
            Error::new(
                ErrorCode::IoFailed,
                format!("the system's randomness couldn't be read for a feature's ID: {e}"),
            )
            .with_parameter("feature.id")
            .with_suggestion("give the feature an id, or add it again")
        })
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
