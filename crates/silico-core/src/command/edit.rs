//! Edits of a session's document (docs/design/m4.1-command-api.md, section 5; and
//! docs/design/m5.0-app.md, section 7): a feature added (at the end, or before another), a
//! field set, a feature removed or moved, the whole text replaced. A feature added and a
//! field set are made on a feature's JSON tree (`document::json::Node`): the request's
//! feature, or the feature's canonical tree with the request's value put where its field
//! is, read back by the one reader every file is read by, with no place in a text (so its
//! errors name fields by their paths alone). A feature inserted before another, removed, or
//! moved is placed in its chain by `document::chain`, which rewires the `base` and `tool`
//! links the edit changes; a text replaced is read as a file is. Each edited document is then written in
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

use std::collections::BTreeSet;
use std::sync::Arc;

use super::elements::{worded, worded_of};
use super::{ExtendBy, Gated, NewMembers, Problems};
use crate::document::chain::{self, Rewired};
use crate::document::json::{self, Member, Node, Plain, Value};
use crate::document::suppression;
use crate::document::undo::Prepared;
use crate::document::{
    self, AxisToken, Checked, Document, ElementId, ElementKind, Feature, FeatureId, FeatureVersion,
    ID_ALPHABET, ID_LENGTH, NamingScheme, Operation, PROBLEMS_LISTED, Reference, Sketch,
    sketch_edit,
};
use crate::drag::DragStop;
use crate::extend;
use crate::face_plane::{Defaulted, Placed};
use crate::naming::{Maker, Name};
use crate::recompute::Model;
use crate::references::{CANDIDATES, cut_short};
use crate::sketch::SketchSolver;
use crate::{Error, ErrorCode};

/// The state an accepted edit gives a session's document: its canonical text, that text as
/// the reader and the gate find it, and its features' canonical texts, which the text was
/// assembled from, written once: what a transaction's undo stack keeps of it
/// (`document::undo`), made here, off every lock. Made only by [`candidate`], so its text is
/// always the one its features' texts assemble.
pub(super) struct Candidate {
    text: Arc<Vec<u8>>,
    gated: Arc<Gated>,
    prepared: Prepared,
}

impl Candidate {
    /// Its text, and the gate's finding of it.
    pub(super) fn parts(&self) -> (&[u8], &Gated) {
        (&self.text, &self.gated)
    }

    /// The gate's finding of its text.
    pub(super) fn gated(&self) -> &Gated {
        &self.gated
    }

    /// Its text, the gate's finding of it, and its features' canonical texts.
    pub(super) fn into_parts(self) -> (Arc<Vec<u8>>, Arc<Gated>, Prepared) {
        (self.text, self.gated, self.prepared)
    }
}

/// `document`, edited, written in canonical form, and that text through the reader and the
/// gate, once: so the document held is the text's. Or, refused, every error the gate finds
/// in `document` as given, its places counted in its own order (a sketch's members as the
/// request or the text gave them, as `silico check` counts a file's), not the canonical
/// text's; or the canonical text's errors, if the gate takes `document` as given. `field`
/// is the request's field that holds the edit, which a document too long to write names.
fn candidate(document: &Document, field: &str) -> Result<Candidate, Problems> {
    let prepared = Prepared::of(document).map_err(|e| e.with_parameter(field))?;
    let text = prepared.text();
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
        prepared,
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
/// ([`document::optional_members`], or `suppressed`, which every feature may): one an edit
/// may give it, and take away with `null` (and `suppressed` with `false` too).
fn optional(feature: &Feature, key: &str) -> Option<&'static str> {
    if key == SUPPRESSED {
        return Some(SUPPRESSED);
    }
    document::optional_members(feature.operation.type_name())
        .iter()
        .find(|optional| **optional == key)
        .copied()
}

/// The member a suppressed feature has, and an unsuppressed one leaves out: so `set_field`
/// takes `false` for it by removing it, as the reader refuses `false` (decision 4).
const SUPPRESSED: &str = "suppressed";

/// The member a sketch's angle has when it takes `b` from its `to` to its `from`, and
/// leaves out otherwise: so `set_field` adds it with `true` and takes `false` or `null` for
/// it by removing it, as the reader refuses `false`, as for `suppressed`.
const REVERSED: &str = "reversed";

/// Whether `node` is a sketch's angle: an object of type `angle`, at the depth of a
/// constraint (`path`, two steps into a sketch's tree: its list, then the member).
fn is_angle(feature: &Feature, depth: usize, node: &Node) -> bool {
    matches!(feature.operation, Operation::Sketch(_))
        && depth == 2
        && match &node.value {
            Value::Object(members) => members
                .iter()
                .any(|m| m.key == "type" && m.value.value.as_str() == Some("angle")),
            _ => false,
        }
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
    if let Some(refused) = origin_plane_member(feature, &segments) {
        return Err(refused);
    }
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
                let Some(i) = members.iter().position(|m| m.key == *segment) else {
                    // An angle's `reversed`, which it may leave out, is there to set.
                    if *segment == REVERSED
                        && depth + 1 == segments.len()
                        && matches!(list, Some((_, ElementKind::Constraint)))
                        && is_angle(feature, depth, node)
                    {
                        steps.push(Step::Absent(REVERSED));
                        return Ok(Target { steps, sketched });
                    }
                    return Err(missing());
                };
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

/// The refusal of a path into a face plane's members (`plane.face`, `plane.x`,
/// `plane.reversed`) of `feature`, a sketch on an origin plane, which has none of them:
/// naming the field, and saying to set `plane` whole. None for any other path or feature.
fn origin_plane_member(feature: &Feature, segments: &[&str]) -> Option<Error> {
    let Operation::Sketch(sketch) = &feature.operation else {
        return None;
    };
    let origin = sketch.plane.origin()?;
    let ["plane", member @ ("face" | "x" | "reversed")] = segments else {
        return None;
    };
    Some(
        Error::new(
            ErrorCode::InvalidInput,
            format!(
                "plane.{member} is a face plane's, and sketch {} is on the origin plane {}, which has none",
                feature.id,
                origin.as_str()
            ),
        )
        .with_feature(feature.id.as_str())
        .with_parameter("field")
        .with_suggestion(
            r#"set plane whole: to a face plane, {"face": "<its name>", "x": "+x", "reversed": false}, or to xy, xz, or yz"#,
        ),
    )
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
/// base) set to `null` removed (the design's section 5), and `suppressed` set to `false`;
/// and a sketch angle's `reversed` set to `false` or `null` removed.
fn put(feature: &Feature, tree: &mut Node, steps: &[Step], value: Node) {
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
    let angle = is_angle(feature, path.len(), node);
    let removes = |key: &str, value: &Node| {
        let off = matches!(value.value, Value::Null | Value::Bool(false));
        (path.is_empty()
            && ((value.value == Value::Null && optional(feature, key).is_some())
                || (key == SUPPRESSED && value.value == Value::Bool(false))))
            || (angle && key == REVERSED && off)
    };
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
            let removed = members.get(*i).is_some_and(|m| removes(&m.key, &value));
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

/// What `add_feature` made: the feature as the document then holds it, its place, from 0,
/// what its insert rewired, and the state it leads to.
pub(super) struct Inserted {
    pub(super) feature: Feature,
    pub(super) index: usize,
    pub(super) rewired: Vec<Rewired>,
    pub(super) candidate: Candidate,
}

/// `feature`, a feature as the document writes it, added to `checked`'s timeline before the
/// feature `before`, or at the end for none (the design's `add_feature`, with M5.0's
/// `before`): with the ID `new_id` makes if it gives none, and the feature type's current
/// version if it gives no `v` (each `null` given as none, as is a `null` fingerprint, note,
/// or other member the feature may leave out: the design's section 2). Before a feature,
/// it's inserted into its chain (`document::chain::insert`): each input it consumes that a
/// feature at or after its place consumed is consumed by it instead, that feature's field
/// rewired to it. At the end, it's appended, as M4 adds a feature, and an input another
/// consumes is the gate's to refuse ("already consumed"). Refused for a `before` the
/// document hasn't (naming `before`), for a feature that isn't an object (in the words the
/// wire refuses one in, before it gets here: this is for the Rust API's callers), gives a
/// fingerprint (a commit writes it), or gives an ID that isn't one, or that the document
/// holds (each naming `feature.id`), each of these at once with a `before` the document
/// hasn't; for an insert the chain refuses; and with every error the reader or the gate
/// finds in the document with it added. Inside a transaction (`begun`, the document it
/// began from), a feature of an ID `begun` stored a fingerprint for carries it
/// ([`kept_record`]).
pub(super) fn add_feature(
    checked: &Checked,
    before: Option<&FeatureId>,
    feature: Node,
    begun: Option<&Document>,
    new_id: impl FnOnce(&Document) -> Result<FeatureId, Error>,
) -> Result<Inserted, Problems> {
    let document = checked.document();
    let place = chain::insert_place(document, before);
    let index = *place.as_ref().unwrap_or(&document.features.len());
    // A `before` the document hasn't, refused with every refusal of the feature's own, at
    // once, as every edit's are.
    let added = match (place, new_feature(document, index, feature, new_id)) {
        (Ok(_), Ok(added)) => added,
        (place, read) => {
            let mut problems = Problems::none();
            if let Err(error) = place {
                problems.push(error);
            }
            if let Err(read) = read {
                problems.absorb(read);
            }
            return Err(problems);
        }
    };
    let added = kept_record(added, begun);
    let (edited, rewired) = match before {
        None => {
            let mut edited = document.clone();
            edited.features.push(added);
            (edited, Vec::new())
        }
        Some(before) => {
            let inserted = chain::insert(checked, added, Some(before))?;
            (inserted.document, inserted.rewired)
        }
    };
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
    Ok(Inserted {
        feature: held,
        index,
        rewired,
        candidate,
    })
}

/// `feature`, a request's feature, read as the document's feature at place `index`: with
/// the ID `new_id` makes if it gives none, and its type's current version if it gives no
/// `v`; refused as [`add_feature`] says, every reason the reader finds.
fn new_feature(
    document: &Document,
    index: usize,
    feature: Node,
    new_id: impl FnOnce(&Document) -> Result<FeatureId, Error>,
) -> Result<Feature, Problems> {
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
    let mut node = unplaced(Value::Object(members));
    json::unplace(&mut node);
    let place = format!("features[{index}]");
    document::read_feature(index, node)
        .map_err(|e| {
            // The request's own field: an ID that isn't one is the request's `feature.id`. The
            // reader names a feature by its place only until it has read its ID, and the feature
            // is an object, so every error that names its place is its `id`'s.
            if e.feature() == Some(place.as_str()) {
                e.with_parameter("feature.id")
            } else {
                e
            }
        })
        .map_err(Problems::from)
}

/// The path to a face plane's face, which a refusal of the face's default `x` names
/// ([`with_default_x`]): the document's, as the gate's refusals of it and the rebuild's name
/// it, with the new sketch's ID as the feature.
const FACE_PATH: &str = "plane.face";

/// Whether `feature`, an `add_feature` request's feature, is a sketch on a face whose plane
/// leaves out its `x`, or gives it as `null`, which is the same (M4.1 section 5: the core
/// names the face's `default_x`, F1; the document always holds `x`, so the reader never
/// takes a plane without one). If so, its `x` is given meanwhile as `+x`, any token, so the
/// rest of the feature is read and put through the gate as it will be, which no token
/// changes: [`with_default_x`] then puts the face's own in its place. Only a sketch has a
/// `plane`, so the reader refuses one of any other feature, `x` or none.
pub(super) fn leaves_out_x(feature: &mut Node) -> bool {
    let Value::Object(members) = &mut feature.value else {
        return false;
    };
    let Some(Value::Object(plane)) = members
        .iter_mut()
        .find(|m| m.key == "plane")
        .map(|m| &mut m.value.value)
    else {
        return false;
    };
    let meanwhile = || Node {
        at: json::At::UNPLACED,
        value: Value::String(AxisToken::PlusX.word().to_owned()),
    };
    match plane.iter_mut().find(|m| m.key == "x") {
        Some(given) if given.value.value == Value::Null => {
            given.value = meanwhile();
            true
        }
        Some(_) => false,
        // Read by key, so at its end: the document writes the plane in its own order.
        None => {
            plane.push(Member {
                key: "x".to_owned(),
                at: json::At::UNPLACED,
                value: meanwhile(),
            });
            true
        }
    }
}

/// `inserted`, a sketch on a face added with its `x` left out ([`leaves_out_x`]), given the
/// face's default (M4.1 section 5; F1): its face looked for in `model`, the model of the
/// document it was added to, in the solids live at its place in the timeline, by the rule
/// and the code a rebuild places it by (`recompute::default_x`), and its `x` written as the
/// face's `default_x`, as `elements` reports it. Refused, the edit not made, in the words a
/// rebuild would refuse the sketch's face in, naming the new sketch and `plane.face`, as the
/// gate's refusals of the face do: a name that finds nothing, finds an element by an
/// unstable name or one that isn't a face, or a face OCCT doesn't type as a plane, as
/// `reference_broken` with candidates; a face whose solid didn't build, saying which feature
/// it's blocked on (or with the kernel's fault itself, if one stopped it, as every read of a
/// solid refuses one); and a solid that couldn't be described, with why.
pub(super) fn with_default_x(inserted: Inserted, model: &Model) -> Result<Inserted, Problems> {
    let Inserted {
        index,
        rewired,
        candidate: trial,
        ..
    } = inserted;
    let Gated::Checked(checked) = trial.gated() else {
        return Err(bug("a feature added through the gate isn't in a checked document").into());
    };
    let face = match checked.document().features.get(index).map(|f| &f.operation) {
        Some(Operation::Sketch(sketch)) => sketch.plane.face().map(|plane| plane.face.clone()),
        _ => None,
    };
    let Some(face) = face else {
        return Err(
            bug("the sketch added for its face's default x isn't a sketch on a face").into(),
        );
    };
    let token = match crate::recompute::default_x(checked, model, index) {
        Some(Defaulted::X(token)) => token,
        Some(Defaulted::Refused(placed)) => {
            let sketch = sketch_id(checked, index)?;
            return Err(no_default(placed, model, &sketch, face.as_str()));
        }
        // Worded as the solid's own read words it, then on the new sketch, as the
        // rebuild puts it.
        Some(Defaulted::Undescribed(error)) => {
            let sketch = sketch_id(checked, index)?;
            return Err(worded_of(&error, &sketch).into());
        }
        Some(Defaulted::Waits(on)) => {
            return Err(bug(&format!(
                "the face's default x waits for {on}'s solid in a model already published"
            ))
            .into());
        }
        None => {
            return Err(bug("the sketch added for its face's default x wasn't looked at").into());
        }
    };
    let mut document = checked.document().clone();
    if let Some(Operation::Sketch(sketch)) = document
        .features
        .get_mut(index)
        .map(|feature| &mut feature.operation)
        && let document::Plane::Face(plane) = &mut sketch.plane
    {
        plane.x = token;
    }
    let candidate = candidate(&document, "feature")?;
    let feature = candidate
        .gated
        .document()
        .ok()
        .and_then(|document| document.features.get(index))
        .cloned()
        .ok_or_else(|| Problems::from(bug("the feature added isn't in the document")))?;
    Ok(Inserted {
        feature,
        index,
        rewired,
        candidate,
    })
}

/// The ID of the feature at `index` of `checked`: the sketch added.
fn sketch_id(checked: &Checked, index: usize) -> Result<FeatureId, Problems> {
    checked
        .document()
        .features
        .get(index)
        .map(|f| f.id.clone())
        .ok_or_else(|| Problems::from(bug("the sketch added isn't in the document")))
}

/// The refusal of the sketch `sketch`'s face's default `x` where a rebuild would place the
/// sketch so (`placed`, never a frame), its face `face`, in `model`: as [`with_default_x`]
/// says.
fn no_default(placed: Placed, model: &Model, sketch: &FeatureId, face: &str) -> Problems {
    match placed {
        Placed::Broken(error) => error.into(),
        Placed::Failed(error) => worded(&error).into(),
        Placed::Blocked(root) => {
            // Stopped by a kernel fault, which isn't the document's: refused with the fault
            // itself, naming the root whose outcome holds it, as a read of the solid is
            // (`elements`).
            if let Some(fault) = model.outcome(&root).and_then(|outcome| {
                outcome
                    .errors()
                    .iter()
                    .find(|error| error.code().is_kernel_fault())
            }) {
                return worded_of(fault, &root).into();
            }
            Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "plane.face, {}, is looked for in a solid that didn't build, blocked on {root}, so the face's default x isn't known",
                    json::shown(face)
                ),
            )
            .with_feature(sketch.as_str())
            .with_parameter(FACE_PATH)
            .with_element(cut_short(face))
            .with_suggestion(format!(
                "mend {root} first (`check` says why it failed), or give plane.x to add the sketch all the same"
            ))
            .into()
        }
        Placed::Frame(_) => bug("a face's default x was refused with a frame").into(),
    }
}

/// An error that's a bug in Silico's.
fn bug(message: &str) -> Error {
    Error::new(ErrorCode::Internal, message).with_suggestion(crate::worker::SILICO_BUG)
}

/// `added`, carrying the fingerprint the document a transaction began from (`begun`) stored
/// for the feature of its ID, whatever its fields: the saved file is a function of the
/// document's state, not of the edits' path, and what the edits reach (decision 28) decides
/// whether that record is read. Added back with values as `begun` held them (its name and
/// note aside, which reach nothing, or set back after), it's unreached, held to its old
/// record, its drift reported as an untouched feature's is; with others, it's reached, its
/// drift not read, and its commit writes it fresh. Outside a transaction (`begun` none), or
/// for an ID `begun` stored no fingerprint for, as given.
fn kept_record(mut added: Feature, begun: Option<&Document>) -> Feature {
    if let Some(fingerprint) = begun
        .and_then(|begun| begun.features.iter().find(|f| f.id == added.id))
        .and_then(|was| was.fingerprint)
    {
        added.fingerprint = Some(fingerprint);
    }
    added
}

/// The feature `id` removed from `checked` (the design's `remove_feature`, as M5.0's A9
/// amends it: `document::chain::remove`): the chain's removal (the feature as the document
/// held it, its place in the timeline, from 0, and each consumer that took its base), and
/// the state it leads to. Refused for a feature the document hasn't, where nothing can stand
/// in for it (a chain's root under a feature that needs a base, a feature whose sketch it
/// is, or one that stores names of its elements, each naming both), where a feature's base
/// would no longer be built from a maker of its stored names (open item 2b), and with every
/// error the gate finds in the document without it.
pub(super) fn remove_feature(
    checked: &Checked,
    id: &FeatureId,
) -> Result<(chain::Removed, Candidate), Problems> {
    place_of(checked.document(), id)?;
    let removed = chain::remove(checked, id)?;
    let candidate = candidate(&removed.document, "feature")?;
    Ok((removed, candidate))
}

/// The feature `id` of `checked` moved before the feature `before`, or to the end for none
/// (docs/design/m5.0-app.md, `move_feature`: `document::chain::move_feature`): the chain's
/// move, and the state it leads to, none for a move to where it is. Refused for a feature
/// or a `before` the document hasn't, for each move the chain refuses (naming both
/// features), and with every error the gate finds in the document after the move.
pub(super) fn move_feature(
    checked: &Checked,
    id: &FeatureId,
    before: Option<&FeatureId>,
) -> Result<(chain::Moved, Option<Candidate>), Problems> {
    place_of(checked.document(), id)?;
    let moved = chain::move_feature(checked, id, before)?;
    if !moved.changed {
        return Ok((moved, None));
    }
    let candidate = candidate(&moved.document, "feature")?;
    Ok((moved, Some(candidate)))
}

/// The feature `id` of `checked` suppressed, with its dependents, or unsuppressed, with what
/// it needs (docs/design/m5.0-app.md, `suppress_feature`: `document::suppression`): what it
/// suppressed and unsuppressed, and the state it leads to, none for a feature suppressed
/// already, or one unsuppressed that isn't. Refused for a feature the document hasn't, and
/// with every error the gate finds in the document after it (none, by the rule it keeps).
pub(super) fn suppress_feature(
    checked: &Checked,
    id: &FeatureId,
    suppressed: bool,
) -> Result<(suppression::Suppression, Option<Candidate>), Problems> {
    place_of(checked.document(), id)?;
    let done = suppression::set_suppressed(checked, id, suppressed)?;
    if !done.changed() {
        return Ok((done, None));
    }
    let candidate = candidate(&done.document, "feature")?;
    Ok((done, Some(candidate)))
}

/// What a sketch edit changed, by member ID, as [`sketch_edit::Edited`] lists them.
pub(super) struct SketchChanges {
    pub(super) added: Vec<ElementId>,
    pub(super) modified: Vec<ElementId>,
    pub(super) removed: Vec<ElementId>,
}

/// `members` added to the sketch `id` of `document` (docs/design/m5.0-app.md, section 7,
/// `sketch_add`): what it added, and the state it leads to, none for an empty batch, which
/// changes nothing. Each list is read by the document's own reader as the sketch's lists are
/// read (its members named by their ID paths, `circles.cb000003.radius`), then added by
/// [`sketch_edit::add`], and the document with the sketch so edited put through the gate.
/// Never solved: once the gate takes it, it's applied, whatever its solve then finds (SD7).
/// Refused for a feature that isn't a sketch, naming `sketch`, and with every problem the
/// reader, the add, or the gate finds, each naming the sketch.
pub(super) fn sketch_add(
    document: &Document,
    id: &FeatureId,
    members: NewMembers,
) -> Result<(SketchChanges, Option<Candidate>), Problems> {
    let (index, feature, sketch) = sketch_of(document, id)?;
    let members = read_members(index, feature, members)?;
    let edited = sketch_edit::add(sketch, &members).map_err(|refused| of_sketch(id, refused))?;
    sketch_edited(document, index, sketch, edited)
}

/// `members` removed from the sketch `id` of `document` (docs/design/m5.0-app.md, section
/// 7, `sketch_remove`; sketch-edit decision 7): what it removed and changed, and the state it
/// leads to, none for an empty list, which removes nothing. Removed by
/// [`sketch_edit::remove`] (a curve with the points only it used, every constraint naming
/// what goes, a point only if no curve uses it, duplicates taken once), and the document
/// put through the gate. Never solved, as [`sketch_add`] isn't (SD7). Refused for a feature
/// that isn't a sketch, naming `sketch`; and with every problem the removal finds (a member
/// the sketch hasn't, a fixed reference, `origin`, `x_axis`, or `y_axis`, a point a curve
/// still uses), all in one answer, each once at its first place, or the gate finds, each
/// naming the sketch.
pub(super) fn sketch_remove(
    document: &Document,
    id: &FeatureId,
    members: &[Reference],
) -> Result<(SketchChanges, Option<Candidate>), Problems> {
    let (index, _, sketch) = sketch_of(document, id)?;
    let edited = sketch_edit::remove(sketch, members).map_err(|refused| of_sketch(id, refused))?;
    sketch_edited(document, index, sketch, edited)
}

/// The sketch `id` of `document` replaced by `after`, its points where a drag's release
/// solved them (docs/design/m5.0-app.md, `drag_end`): the state it leads to, none if the
/// sketch is the same, through the gate otherwise. Refused, naming `sketch`, for a feature
/// that isn't a sketch, and with every problem the gate finds.
pub(super) fn sketch_replaced(
    document: &Document,
    id: &FeatureId,
    after: Sketch,
) -> Result<Option<Candidate>, Problems> {
    let (index, _, sketch) = sketch_of(document, id)?;
    if after == *sketch {
        return Ok(None);
    }
    let mut edited = document.clone();
    if let Some(feature) = edited.features.get_mut(index) {
        feature.operation = Operation::Sketch(after);
    }
    Ok(Some(candidate(&edited, "sketch")?))
}

/// What a split or a trim did (docs/design/m5.0-app.md, section 7, `sketch_split` and
/// `sketch_trim`): what it changed, the curve's pieces left (the start-side one first,
/// which keeps the curve's ID), and the features that store a name its curve reaches, in
/// timeline order (*What a result lists*).
pub(super) struct Cut {
    pub(super) changes: SketchChanges,
    pub(super) pieces: Vec<ElementId>,
    pub(super) references: Vec<FeatureId>,
}

/// `curve` of the sketch `id` of `document` split at `at` (docs/design/m5.0-app.md,
/// section 7, `sketch_split`; sketch-edit 1 to 4, §3's table): what it changed, its two
/// pieces, the features storing a name it reaches, and the state it leads to, by
/// [`sketch_edit::split_with`], its new members' IDs `ids`, and the document put through
/// the gate. Never solved here: the session's solve then judges it (SD7). Refused for a
/// feature that isn't a sketch, naming `sketch`, and with what the split or the gate
/// finds, naming the sketch.
pub(super) fn sketch_split(
    document: &Document,
    id: &FeatureId,
    curve: &ElementId,
    at: &[[f64; 2]],
    ids: sketch_edit::NewIds<'_>,
) -> Result<(Cut, Option<Candidate>), Problems> {
    let (index, _, sketch) = sketch_of(document, id)?;
    let split = sketch_edit::split_with(sketch, curve, at, ids).map_err(|e| one(id, e))?;
    let references = referencing(document, id, sketch, curve);
    let (changes, candidate) = sketch_edited(document, index, sketch, split.edited)?;
    Ok((
        Cut {
            changes,
            pieces: split.pieces.to_vec(),
            references,
        },
        candidate,
    ))
}

/// `curve` of the sketch `id` of `document` trimmed back to the cuts either side of `pick`
/// (docs/design/m5.0-app.md, section 7, `sketch_trim`; sketch-edit 2, 5, 6, 8, and §4): what
/// it changed, the pieces left, the features storing a name it reaches, and the state it
/// leads to, by [`sketch_edit::trim_with`], as [`sketch_split`] makes a split.
pub(super) fn sketch_trim(
    document: &Document,
    id: &FeatureId,
    curve: &ElementId,
    pick: [f64; 2],
    ids: sketch_edit::NewIds<'_>,
) -> Result<(Cut, Option<Candidate>), Problems> {
    let (index, _, sketch) = sketch_of(document, id)?;
    let trim = sketch_edit::trim_with(sketch, curve, pick, ids).map_err(|e| one(id, e))?;
    let references = referencing(document, id, sketch, curve);
    let (changes, candidate) = sketch_edited(document, index, sketch, trim.edited)?;
    Ok((
        Cut {
            changes,
            pieces: trim.pieces,
            references,
        },
        candidate,
    ))
}

/// `curve`'s use of `point`, of the sketch `id` of `document`, pulled apart
/// (docs/design/m5.0-app.md, section 7, `sketch_unjoin`; sketch-edit 9), by
/// [`sketch_edit::unjoin_with`], its new point's ID `ids`, and the document put through the
/// gate: what it changed, and the state it leads to. Refused as [`sketch_split`] is.
pub(super) fn sketch_unjoin(
    document: &Document,
    id: &FeatureId,
    point: &ElementId,
    curve: &ElementId,
    ids: sketch_edit::NewIds<'_>,
) -> Result<(SketchChanges, Option<Candidate>), Problems> {
    let (index, _, sketch) = sketch_of(document, id)?;
    let edited = sketch_edit::unjoin_with(sketch, point, curve, ids).map_err(|e| one(id, e))?;
    sketch_edited(document, index, sketch, edited)
}

/// `points` of the sketch `id` of `document` made one, the first kept
/// (docs/design/m5.0-app.md, section 7, `sketch_merge`; sketch-edit 10), by
/// [`sketch_edit::merge`], and the document put through the gate: what it changed, and the
/// state it leads to. Whether its solve is worse than the one before is the session's to
/// judge. Refused as [`sketch_split`] is.
pub(super) fn sketch_merge(
    document: &Document,
    id: &FeatureId,
    points: &[ElementId],
) -> Result<(SketchChanges, Option<Candidate>), Problems> {
    let (index, _, sketch) = sketch_of(document, id)?;
    let edited = sketch_edit::merge(sketch, points).map_err(|e| one(id, e))?;
    sketch_edited(document, index, sketch, edited)
}

/// `curve`'s `end` of the sketch `id` of `document` extended `by` (docs/design/m5.0-app.md,
/// section 7, `sketch_extend`; open item 12c), on `solver` under `stop`: from the sketch as
/// solved and written back (M3.4), as a drag begins, or from its file's coordinates if its
/// solve fails otherwise; to the next curve as [`extend::next_crossing`] finds it there,
/// placed as a point is. What it changed, and the state it leads to, through the gate. The
/// session's solve then judges it (SD7). Refused for a feature that isn't a sketch, naming
/// `sketch`; with what the extend refuses, or a next curve there's none of, naming the
/// sketch; and, stopped, as `cancelled`.
pub(super) fn sketch_extend(
    document: &Checked,
    id: &FeatureId,
    curve: &ElementId,
    end: extend::End,
    by: ExtendBy,
    solver: &mut dyn SketchSolver,
    stop: &DragStop,
) -> Result<(SketchChanges, Option<Candidate>), Problems> {
    let (index, _, sketch) = sketch_of(document.document(), id)?;
    let (from, _) = crate::drag::sketch::solved_or_drawn(document, id, sketch, solver, stop)
        .map_err(|error| Problems::from(error.with_feature(id.as_str())))?;
    let by = match by {
        ExtendBy::Length(length) => extend::By::Length(length),
        ExtendBy::Angle(angle) => extend::By::Angle(angle),
        ExtendBy::Point(point) => extend::By::To(point),
        ExtendBy::NextCurve => match extend::next_crossing(&from, curve, end) {
            Ok(Some(crossing)) => extend::By::To(crossing.at),
            Ok(None) => return Err(one(id, extend::no_next_curve(&from, curve, end))),
            Err(error) => return Err(one(id, error)),
        },
    };
    let mut edited =
        extend::extend_under(&from, curve, end, by, solver, stop).map_err(|e| one(id, e))?;
    // Begun from the solve written back, the edit writes every value the solve moved too:
    // each change is listed against the sketch the document held, not the solved copy.
    edited.modified = extend::changed(sketch, &edited.sketch);
    sketch_edited(document.document(), index, sketch, edited)
}

/// What a join did (docs/design/m5.0-app.md, section 7, `sketch_join`): what it changed
/// (the joined curve first among `modified`), and the features that store a name either
/// piece reaches, in timeline order (*What a result lists*).
pub(super) struct Join {
    pub(super) changes: SketchChanges,
    pub(super) references: Vec<FeatureId>,
}

/// The two pieces sharing `point`, of the sketch `id` of `document`, joined into one curve
/// (docs/design/m5.0-app.md, section 7, `sketch_join`; open item 12b), by
/// [`sketch_edit::join`], and the document put through the gate: what it changed, the
/// features storing a name either piece reaches, and the state it leads to.
/// Whether its solve is worse than the one before is the session's to judge. Refused as
/// [`sketch_split`] is.
pub(super) fn sketch_join(
    document: &Document,
    id: &FeatureId,
    point: &ElementId,
) -> Result<(Join, Option<Candidate>), Problems> {
    let (index, _, sketch) = sketch_of(document, id)?;
    let joined = sketch_edit::join(sketch, point).map_err(|e| one(id, e))?;
    let references = referencing_any(document, id, sketch, &joined.pieces);
    let (changes, candidate) = sketch_edited(document, index, sketch, joined.edited)?;
    Ok((
        Join {
            changes,
            references,
        },
        candidate,
    ))
}

/// A sketch edit's one refusal, naming the sketch `id` if it names no feature.
fn one(id: &FeatureId, error: Error) -> Problems {
    of_sketch(
        id,
        sketch_edit::Refused {
            errors: vec![error],
            total: 1,
        },
    )
}

/// The features of `document` after the sketch `id` (`sketch`) that store a name made from
/// `curve`'s ID or one of its points' (`s0000001.sketch.Edge.na000001`, as a source anywhere
/// in the name), in timeline order (docs/design/m5.0-app.md, section 7, *What a result
/// lists*): each such name finds, after a split or a trim, what the start-side piece makes,
/// only part of what it found (open item 11), so the edit says which to look at. A name that
/// doesn't parse reaches nothing (the gate refuses it on its own).
fn referencing(
    document: &Document,
    id: &FeatureId,
    sketch: &Sketch,
    curve: &ElementId,
) -> Vec<FeatureId> {
    referencing_any(document, id, sketch, std::slice::from_ref(curve))
}

/// [`referencing`] for any of `curves`: the features storing a name made from one of their
/// IDs or one of their points', each once, in timeline order (a join's two pieces).
fn referencing_any(
    document: &Document,
    id: &FeatureId,
    sketch: &Sketch,
    curves: &[ElementId],
) -> Vec<FeatureId> {
    let mut reached: BTreeSet<&ElementId> = curves.iter().collect();
    for line in sketch.lines.iter().filter(|l| curves.contains(&l.id)) {
        reached.extend([&line.from, &line.to]);
    }
    for circle in sketch.circles.iter().filter(|c| curves.contains(&c.id)) {
        reached.insert(&circle.center);
    }
    for arc in sketch.arcs.iter().filter(|a| curves.contains(&a.id)) {
        reached.extend([&arc.center, &arc.start, &arc.end]);
    }
    let features = &document.features;
    features
        .iter()
        .enumerate()
        .filter(|(index, feature)| {
            let makers = |maker: &FeatureId| {
                features[..*index]
                    .iter()
                    .find(|f| &f.id == maker)
                    .map(|f| Maker::of(&f.operation))
            };
            feature.operation.stored_names().any(|(_, stored)| {
                Name::parse(stored.as_str(), &makers).is_ok_and(|name| reaches(&name, id, &reached))
            })
        })
        .map(|(_, feature)| feature.id.clone())
        .collect()
}

/// Whether `name`, or any name it's made of, is a sketch element of `sketch` named by one of
/// `ids`.
fn reaches(name: &Name, sketch: &FeatureId, ids: &BTreeSet<&ElementId>) -> bool {
    match name {
        Name::Sketch { feature, id, .. } => feature == sketch && ids.contains(id),
        _ => name
            .parts()
            .into_iter()
            .any(|part| reaches(part, sketch, ids)),
    }
}

/// The sketch `id` of `document`, with its place in the timeline and its feature; or the
/// refusal, naming `sketch`, of a feature the document hasn't or one that isn't a sketch,
/// the document's first sketches the candidates (as the `sketch` read refuses them).
fn sketch_of<'a>(
    document: &'a Document,
    id: &FeatureId,
) -> Result<(usize, &'a Feature, &'a Sketch), Problems> {
    let refused = |message: String| {
        let sketches: Vec<String> = document
            .features
            .iter()
            .filter(|f| matches!(f.operation, Operation::Sketch(_)))
            .take(CANDIDATES)
            .map(|f| f.id.as_str().to_owned())
            .collect();
        let suggestion = if sketches.is_empty() {
            "the document has no sketch: add one, and edit it by its ID"
        } else {
            "name a sketch of the document, by its ID"
        };
        Problems::from(
            Error::new(ErrorCode::InvalidInput, message)
                .with_parameter("sketch")
                .with_suggestion(suggestion)
                .with_candidates(sketches),
        )
    };
    let Some((index, feature)) = document
        .features
        .iter()
        .enumerate()
        .find(|(_, f)| f.id == *id)
    else {
        return Err(refused(format!("the document has no feature {id}")));
    };
    match &feature.operation {
        Operation::Sketch(sketch) => Ok((index, feature, sketch)),
        other => Err(refused(format!(
            "{id} is a {}, not a sketch",
            other.type_name()
        ))),
    }
}

/// `members`' lists, read by the document's reader as `feature`'s (a sketch at `index`)
/// own lists are: its canonical tree with each list replaced by the request's, read whole,
/// so a member is refused in the words and at the path (`circles.cb000003.radius`) a file's
/// would be, naming the sketch.
fn read_members(
    index: usize,
    feature: &Feature,
    members: NewMembers,
) -> Result<sketch_edit::Members, Problems> {
    let (_, mut tree) = feature_tree(feature)?;
    let NewMembers {
        points,
        lines,
        circles,
        arcs,
        constraints,
    } = members;
    let given = [points, lines, circles, arcs, constraints];
    if let Value::Object(fields) = &mut tree.value {
        for (kind, mut list) in ElementKind::ALL.into_iter().zip(given) {
            json::unplace(&mut list);
            match fields.iter_mut().find(|m| m.key == kind.list()) {
                Some(field) => field.value = list,
                None => fields.push(Member {
                    key: kind.list().to_owned(),
                    at: json::At::UNPLACED,
                    value: list,
                }),
            }
        }
    }
    let read = document::read_feature(index, tree)?;
    match read.operation {
        Operation::Sketch(sketch) => Ok(sketch_edit::Members {
            points: sketch.points,
            lines: sketch.lines,
            circles: sketch.circles,
            arcs: sketch.arcs,
            constraints: sketch.constraints,
        }),
        _ => Err(Problems::from(
            Error::new(
                ErrorCode::Internal,
                "a sketch's lists, read as its own, didn't read as a sketch",
            )
            .with_suggestion(crate::worker::SILICO_BUG),
        )),
    }
}

/// A sketch edit's refusal, each problem naming the sketch `id`, all counted.
fn of_sketch(id: &FeatureId, refused: sketch_edit::Refused) -> Problems {
    let listed = u64::try_from(refused.errors.len()).unwrap_or(u64::MAX);
    let errors = refused
        .errors
        .into_iter()
        .map(|e| match e.feature() {
            Some(_) => e,
            None => e.with_feature(id.as_str()),
        })
        .collect();
    Problems::with_unlisted(errors, refused.total.saturating_sub(listed))
}

/// What `edited`, the sketch at `index` of `document` (`sketch`, before) edited, changed,
/// and the state the document with it leads to: none if the sketch is the same, through the
/// gate otherwise.
fn sketch_edited(
    document: &Document,
    index: usize,
    sketch: &Sketch,
    edited: sketch_edit::Edited,
) -> Result<(SketchChanges, Option<Candidate>), Problems> {
    let sketch_edit::Edited {
        sketch: after,
        added,
        modified,
        removed,
    } = edited;
    let changes = SketchChanges {
        added,
        modified,
        removed,
    };
    if after == *sketch {
        return Ok((changes, None));
    }
    let mut edited = document.clone();
    if let Some(feature) = edited.features.get_mut(index) {
        feature.operation = Operation::Sketch(after);
    }
    Ok((changes, Some(candidate(&edited, "sketch")?)))
}

/// The document's text replaced by `text` (the design's `replace_text`), put through the
/// reader and the gate as `silico check` puts a file through them: the new document's
/// number of features, and the state it leads to, none if it's the document `before` holds
/// already, as canonical form writes them ([`Document::canonically_eq`]). Refused with
/// what the reader or the gate says of the text (the reader's own refusals naming `text`),
/// and if its header has no `naming` but the document's text (`held`, the session's) has
/// one: the reader takes a document without it as one saved before M4.0, and text written
/// now isn't one; but the text of a document saved before M4.0, as `text` gives it back, is.
pub(super) fn replace_text(
    before: &Gated,
    held: &[u8],
    text: &[u8],
) -> Result<(usize, Option<Candidate>), Problems> {
    let of_text = |e: Error| match e.parameter() {
        Some(_) => e,
        None => e.with_parameter("text"),
    };
    let root = document::tree(text).map_err(of_text)?;
    if !names_its_scheme(&root) && document::tree(held).is_ok_and(|held| names_its_scheme(&held)) {
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

/// `text`, a document's canonical text (`format`'s, inside a transaction), as an edit's
/// candidate, read as a file is and through the gate.
pub(super) fn as_text(text: &[u8]) -> Result<Candidate, Problems> {
    let read = document::read_tree(document::tree(text)?)?;
    candidate(&read, "text")
}

/// Whether a document's tree is an object whose header has a `naming` (or isn't an object
/// at all, which the reader refuses for itself).
fn names_its_scheme(root: &Node) -> bool {
    match &root.value {
        Value::Object(members) => members.iter().any(|m| m.key == "naming"),
        _ => true,
    }
}

/// Where new feature IDs, and new sketch members' IDs, come from (M2.1, decision 20): the
/// system's randomness, five bytes (as the cache's secret is read, [`crate::system_random`])
/// for the eight characters of an ID, or, in this crate's own tests only, bytes they give
/// (never through the API, the Rust API, or a request).
pub(crate) struct FeatureIds {
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
    pub(crate) fn system() -> Self {
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

    /// Five bytes of the source's: for a feature's ID, or, `member`, a sketch member's.
    fn bytes(&mut self, member: bool) -> Result<[u8; 5], Error> {
        #[cfg(test)]
        if let Some(given) = &mut self.given {
            return given
                .pop_front()
                .ok_or_else(|| Error::new(ErrorCode::Internal, "the test gave no more ID bytes"));
        }
        crate::system_random().map_err(|e| {
            let (what, parameter, suggestion) = if member {
                (
                    "a sketch member's ID",
                    "ids",
                    "give the edit's ids, or make it again",
                )
            } else {
                (
                    "a feature's ID",
                    "feature.id",
                    "give the feature an id, or add it again",
                )
            };
            Error::new(
                ErrorCode::IoFailed,
                format!("the system's randomness couldn't be read for {what}: {e}"),
            )
            .with_parameter(parameter)
            .with_suggestion(suggestion)
        })
    }

    /// A new ID that no feature of `document` holds.
    pub(super) fn next_free(&mut self, document: &Document) -> Result<FeatureId, Error> {
        for _ in 0..ID_TRIES {
            let id = id_of(self.bytes(false)?)?;
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

    /// A new sketch member's ID (docs/design/m5.0-app.md, section 7, *New IDs*: a sketch
    /// edit's `ids` given as `null`) that isn't `taken`: one of the sketch's, or one drawn
    /// for the same edit before it. From the same source as a feature's, with the same
    /// alphabet and length.
    pub(crate) fn next_member(
        &mut self,
        taken: &dyn Fn(&ElementId) -> bool,
    ) -> Result<ElementId, Error> {
        for _ in 0..ID_TRIES {
            let id = id_of(self.bytes(true)?)?;
            let id = ElementId::new(id.as_str()).ok_or_else(|| {
                Error::new(
                    ErrorCode::Internal,
                    format!("{id}, made for a sketch member, isn't a member's ID"),
                )
                .with_suggestion(crate::worker::SILICO_BUG)
            })?;
            if !taken(&id) {
                return Ok(id);
            }
        }
        Err(Error::new(
            ErrorCode::Internal,
            format!("{ID_TRIES} new member IDs in a row were IDs the sketch holds"),
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

    /// A sketch with lines na (along x) and nb (along y), an angle between them (`angle`,
    /// its fields after `b`) and a parallel.
    fn angled(angle: &str) -> Document {
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{{"id": "pa000000", "x": 0, "y": 0, "construction": false}}, {{"id": "pb000000", "x": 10, "y": 0, "construction": false}}, {{"id": "pc000000", "x": 0, "y": 10, "construction": false}}], "lines": [{{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}}, {{"id": "nb000000", "from": "pa000000", "to": "pc000000", "construction": false}}], "circles": [], "arcs": [], "constraints": [{{"id": "k0000001", "type": "angle", "a": "na000000", "b": "nb000000", {angle}"value": 90, "driving": false}}, {{"id": "k0000002", "type": "parallel", "a": "na000000", "b": "x_axis"}}]}}]}}"#
        );
        document::read(text.as_bytes()).unwrap()
    }

    /// `field` of the sketch set to `value`: the values before and after, and the feature's
    /// constraints as the candidate's text writes them.
    fn set(document: &Document, field: &str, value: &str) -> (Plain, Plain, Option<String>) {
        let id = FeatureId::new("k7f2qx3m").unwrap();
        let value = json::read(value.as_bytes()).unwrap();
        let (values, candidate) = set_field(document, &id, field, value).unwrap();
        let text = candidate.map(|c| String::from_utf8(c.parts().0.to_vec()).unwrap());
        (values.before, values.after, text)
    }

    #[test]
    fn set_field_adds_an_angles_reversed_with_true_and_removes_it_with_false_or_null() {
        let field = "constraints.k0000001.reversed";
        let drawn = angled("");
        let reversed = angled(r#""reversed": true, "#);
        // Added: written after b.
        let (before, after, text) = set(&drawn, field, "true");
        assert_eq!((before, after), (Plain::Null, Plain::Bool(true)));
        let text = text.unwrap();
        assert!(
            text.contains(r#""a": "na000000", "b": "nb000000", "reversed": true, "value": 90"#),
            "{text}"
        );
        // Removed, by false and by null, both ways back to the angle as drawn.
        for off in ["false", "null"] {
            let (before, after, text) = set(&reversed, field, off);
            assert_eq!((before, after), (Plain::Bool(true), Plain::Null), "{off}");
            let text = text.unwrap();
            assert!(!text.contains("reversed"), "{off}: {text}");
            let back = document::read(text.as_bytes()).unwrap();
            assert!(back.features[0].canonically_eq(&drawn.features[0]), "{off}");
        }
        // On an angle as drawn, false and null change nothing; true on a reversed one neither.
        for off in ["false", "null"] {
            assert_eq!(set(&drawn, field, off).2, None, "{off}");
        }
        assert_eq!(set(&reversed, field, "true").2, None);
    }

    #[test]
    fn set_field_keeps_a_pads_and_a_revolves_extent_reversed_false_written() {
        // An extent's `reversed` is required, written false or true: only a sketch angle's
        // is left out when false.
        let sketch = r#"{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}, {"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}, {"id": "nc000000", "from": "pc000000", "to": "pa000000", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
        for (feature, want) in [
            (
                r#"{"id": "f0000001", "type": "pad", "v": 1, "name": "P", "sketch": "k7f2qx3m", "extent": {"type": "one_side", "length": 5, "reversed": true}}"#,
                r#""extent": {"type": "one_side", "length": 5, "reversed": false}"#,
            ),
            (
                r#"{"id": "f0000001", "type": "revolve", "v": 1, "name": "R", "sketch": "k7f2qx3m", "axis": "x_axis", "extent": {"type": "one_side", "angle": 90, "reversed": true}}"#,
                r#""extent": {"type": "one_side", "angle": 90, "reversed": false}"#,
            ),
        ] {
            let text = format!(
                r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{sketch}, {feature}]}}"#
            );
            let document = document::read(text.as_bytes()).unwrap();
            let id = FeatureId::new("f0000001").unwrap();
            let value = json::read(b"false").unwrap();
            let (values, candidate) = set_field(&document, &id, "extent.reversed", value).unwrap();
            assert_eq!(
                (values.before, values.after),
                (Plain::Bool(true), Plain::Bool(false))
            );
            let written = String::from_utf8(candidate.unwrap().parts().0.to_vec()).unwrap();
            assert!(written.contains(want), "{written}");
        }
    }

    #[test]
    fn set_field_refuses_reversed_on_a_member_that_isnt_an_angle_and_a_wrong_kind() {
        let id = FeatureId::new("k7f2qx3m").unwrap();
        let refused = |document: &Document, field: &str, value: &str| {
            let value = json::read(value.as_bytes()).unwrap();
            match set_field(document, &id, field, value) {
                Ok(_) => panic!("{field} = taken"),
                Err(problems) => problems.errors()[0].clone(),
            }
        };
        let drawn = angled("");
        // A parallel has no reversed, nor a point, nor a path below one.
        for field in [
            "constraints.k0000002.reversed",
            "points.pa000000.reversed",
            "constraints.k0000001.reversed.x",
        ] {
            let e = refused(&drawn, field, "true");
            assert_eq!(e.parameter(), Some("field"), "{field}: {e}");
        }
        // A reversed that isn't true or false or null: the reader's refusal of it.
        let e = refused(&drawn, "constraints.k0000001.reversed", "1");
        assert_eq!(e.parameter(), Some("constraints.k0000001.reversed"), "{e}");
    }

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
        // One held ID fewer than the tries, then a free one: the last try takes it.
        let mut ids =
            FeatureIds::given(std::iter::repeat_n([0; 5], ID_TRIES - 1).chain([[0xff; 5]]));
        assert_eq!(ids.next_free(&document).unwrap().as_str(), "zzzzzzzz");
        // A source that keeps giving held IDs ends after sixteen tries, refused as a bug,
        // the free ID after them never taken.
        assert_eq!(ID_TRIES, 16);
        let mut ids = FeatureIds::given(std::iter::repeat_n([0; 5], ID_TRIES).chain([[0xff; 5]]));
        let refused = ids.next_free(&document).unwrap_err();
        assert_eq!(
            (refused.code(), refused.message(), refused.suggestion()),
            (
                ErrorCode::Internal,
                "16 new feature IDs in a row were IDs the document holds",
                Some(crate::worker::SILICO_BUG)
            )
        );
        assert_eq!(ids.next_free(&document).unwrap().as_str(), "zzzzzzzz");
    }

    #[test]
    fn a_new_members_id_is_never_one_taken_and_a_source_of_only_taken_ones_is_a_bug() {
        // "00000000" taken (a member of the sketch, or drawn for the edit before): skipped,
        // and "zzzzzzzz", the next, given.
        let taken = |id: &ElementId| id.as_str() == "00000000";
        let mut ids = FeatureIds::given([[0; 5], [0xff; 5]]);
        assert_eq!(ids.next_member(&taken).unwrap().as_str(), "zzzzzzzz");
        let mut ids =
            FeatureIds::given(std::iter::repeat_n([0; 5], ID_TRIES - 1).chain([[0xff; 5]]));
        assert_eq!(ids.next_member(&taken).unwrap().as_str(), "zzzzzzzz");
        let mut ids = FeatureIds::given(std::iter::repeat_n([0; 5], ID_TRIES).chain([[0xff; 5]]));
        let refused = ids.next_member(&taken).unwrap_err();
        assert_eq!(
            (refused.code(), refused.message(), refused.suggestion()),
            (
                ErrorCode::Internal,
                "16 new member IDs in a row were IDs the sketch holds",
                Some(crate::worker::SILICO_BUG)
            )
        );
        assert_eq!(ids.next_member(&taken).unwrap().as_str(), "zzzzzzzz");
    }
}
