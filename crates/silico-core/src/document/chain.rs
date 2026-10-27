//! A document's timeline edited as its chains have it (the M5.0 design, section 7's chain,
//! A9, and section 12's items 1, 2, and 5): the `base` links in the text are each body's
//! chain, exactly as written. Inserting a feature, moving one, and removing one edit the
//! chain in the text, and say every `base` and `tool` they changed ([`Rewired`]). Every rule
//! is a function of the text: chains by `base` links, consumers by the gate's one-consumer
//! rule, the makers of a stored element name by parsing it.
//!
//! Pure functions over a document the gate took: each gives the edited document, which the
//! caller puts through the gate ([`validate`](super::validate)), or why the edit is refused,
//! naming both features where a child would lose its parent. No command runs them yet.

use super::{Document, Feature, FeatureId, Operation};
use crate::naming::{Maker, Name};
use crate::{Error, ErrorCode};

/// A feature whose `base` or `tool` an edit changed: `from` what it named before the edit,
/// `to` what it names after, either none where the feature had no base or is left with
/// none (a pad or revolve, whose base is optional).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewired {
    pub feature: FeatureId,
    pub field: &'static str,
    pub from: Option<FeatureId>,
    pub to: Option<FeatureId>,
}

/// A feature inserted: the document with it, its place from 0, and what it rewired.
#[derive(Debug, Clone)]
pub struct Inserted {
    pub document: Document,
    pub index: usize,
    pub rewired: Vec<Rewired>,
}

/// A feature removed: the document without it, the feature as it was, its place from 0,
/// and what its removal rewired.
#[derive(Debug, Clone)]
pub struct Removed {
    pub document: Document,
    pub feature: Feature,
    pub index: usize,
    pub rewired: Vec<Rewired>,
}

/// A feature moved: the document after the move, where the feature was and where it is
/// (from 0, `to` counted after the move), what it rewired, and whether anything changed.
#[derive(Debug, Clone)]
pub struct Moved {
    pub document: Document,
    pub from: usize,
    pub to: usize,
    pub rewired: Vec<Rewired>,
    pub changed: bool,
}

/// `feature` added to `document` before the feature `before`, or at the end for none (the
/// design's `add_feature` with `before`). Each input it consumes (its `base`, its `tool`)
/// that a feature consumed already is consumed by it instead: that consumer's field now
/// names it, listed in [`Inserted::rewired`]. Refused where that consumer comes before the
/// place asked for, where both of a boolean's inputs had consumers (it would be consumed
/// twice), and where `before` names no feature.
pub fn insert(
    document: &Document,
    feature: Feature,
    before: Option<&FeatureId>,
) -> Result<Inserted, Error> {
    let index = match before {
        None => document.features.len(),
        Some(target) => place(document, target).ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidInput,
                format!("the document has no feature {target} to add this one before"),
            )
            .with_parameter("before")
            .with_suggestion("name a feature by its ID, or give null to add at the end")
        })?,
    };
    let consumed: Vec<(&'static str, FeatureId, usize, &'static str)> = feature
        .operation
        .inputs()
        .into_iter()
        .filter_map(|(field, input)| {
            consumer(document, input).map(|(at, by)| (field, input.clone(), at, by))
        })
        .collect();
    if let [(_, base, base_at, _), (_, tool, tool_at, _)] = consumed.as_slice() {
        let (base_by, tool_by) = (
            &document.features[*base_at].id,
            &document.features[*tool_at].id,
        );
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{} would be consumed twice: its base {base}'s solid is consumed by {base_by}, and its tool {tool}'s by {tool_by}",
                feature.id
            ),
        )
        .with_feature(feature.id.as_str())
        .with_parameter("feature")
        .with_suggestion(format!(
            "use {base_by}'s or {tool_by}'s result as one of its inputs, or inputs nothing consumes"
        )));
    }
    let mut edited = document.clone();
    for (_, input, at, field) in &consumed {
        let by = &document.features[*at].id;
        if *at < index {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "{input}'s solid is already consumed by {by}, which comes before the place asked for"
                ),
            )
            .with_feature(feature.id.as_str())
            .with_parameter("before")
            .with_suggestion(format!("add it before {by}, or on {by}'s result")));
        }
        set_input(
            &mut edited.features[*at].operation,
            field,
            Some(feature.id.clone()),
        );
    }
    edited.features.insert(index, feature);
    let rewired = rewired(document, &edited);
    Ok(Inserted {
        document: edited,
        index,
        rewired,
    })
}

/// The feature `id` removed from `document` (the design's `remove_feature`, as A9 amends
/// it). Its consumer, if it has one, takes its base: the field that named it now names
/// what it named, listed in [`Removed::rewired`], or none for a pad's or revolve's base,
/// which then begins the body. Refused, naming both features, where nothing can stand in:
/// a chain's root under a feature that needs a base (or as a boolean's tool), a feature
/// whose sketch it is, and a feature that stores names of its elements.
pub fn remove(document: &Document, id: &FeatureId) -> Result<Removed, Error> {
    let index = place(document, id).ok_or_else(|| no_feature(id, "feature"))?;
    let removed = &document.features[index];
    if let Some((child, why)) = children(document, id).next() {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{} {}, so {} can't be removed",
                child.id,
                why.why(id),
                removed.id
            ),
        )
        .with_feature(removed.id.as_str())
        .with_parameter("feature")
        .with_suggestion(match why {
            Child::Sketch => format!("remove {} first", child.id),
            Child::Names => format!(
                "remove {} first, or set its edges to names of other features' elements",
                child.id
            ),
        }));
    }
    let mut edited = document.clone();
    if let Some((at, field)) = consumer(document, id) {
        stand_in(document, &mut edited, removed, at, field, "remove")?;
    }
    edited.features.remove(index);
    let rewired = rewired(document, &edited);
    Ok(Removed {
        document: edited,
        feature: removed.clone(),
        index,
        rewired,
    })
}

/// The feature `id` moved before the feature `before`, or to the end for none (the design's
/// `move_feature`): a removal then an insert. Its consumer takes its base, as [`remove`]
/// rewires it; its base becomes the last member of its chain before the new place (none for
/// a pad or revolve placed before the chain's root); and the chain's next member after that
/// place takes it as its base, never as its tool. A move that leaves it between its base and
/// its consumer rewires nothing; a move to where it is changes nothing.
///
/// Refused, naming both features: a move that puts it before its sketch, its tool, or a
/// feature whose elements it names; after a feature that uses it as its sketch or names its
/// elements; one that leaves a chain's root's consumer without a base; one off a tool's
/// chain past the boolean that consumes it; and one that would give a primitive a base or
/// leave a feature that needs one without.
pub fn move_feature(
    document: &Document,
    id: &FeatureId,
    before: Option<&FeatureId>,
) -> Result<Moved, Error> {
    let from = place(document, id).ok_or_else(|| no_feature(id, "feature"))?;
    let target = match before {
        None => document.features.len(),
        Some(target) => place(document, target).ok_or_else(|| no_feature(target, "before"))?,
    };
    if target == from || target == from + 1 {
        return Ok(Moved {
            document: document.clone(),
            from,
            to: from,
            rewired: Vec::new(),
            changed: false,
        });
    }
    let moving = &document.features[from];
    // Places in the document without the feature moved: where it's inserted, and where the
    // others stand.
    let at = |i: usize| if i > from { i - 1 } else { i };
    let place_to = at(target);
    let others = |other: &FeatureId| place(document, other).map(at);

    // The parents it would come before, and the children it would come after.
    if let Some(sketch) = moving.operation.sketch()
        && others(sketch).is_some_and(|p| p >= place_to)
    {
        return Err(come_before(moving, sketch, "which it uses as its sketch"));
    }
    if let Some((_, tool)) = moving
        .operation
        .inputs()
        .into_iter()
        .find(|(f, _)| *f == "tool")
        && others(tool).is_some_and(|p| p >= place_to)
    {
        return Err(come_before(moving, tool, "which it uses as its tool"));
    }
    for named in makers_named(document, moving) {
        if named != id && others(named).is_some_and(|p| p >= place_to) {
            return Err(come_before(moving, named, "whose elements it names"));
        }
    }
    if let Some(child) =
        children(document, id).find(|(child, _)| others(&child.id).is_some_and(|p| p < place_to))
    {
        let (child, why) = child;
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{} would come after {}, which {}",
                moving.id,
                child.id,
                why.why(id)
            ),
        )
        .with_feature(moving.id.as_str())
        .with_parameter("before")
        .with_suggestion(format!(
            "move {} after {} first, or move {} before it",
            child.id, moving.id, moving.id
        )));
    }

    let base = base_of(&moving.operation);
    let consumer_of = consumer(document, id);
    // Left between its base and its consumer: only its place changes.
    let after_base = base.and_then(others).is_none_or(|p| p < place_to);
    let by_consumer = consumer_of.is_none_or(|(c, _)| place_to <= at(c));
    let stays = after_base && by_consumer;
    let mut edited = document.clone();
    if !stays && let Some((c, field)) = consumer_of {
        stand_in(document, &mut edited, moving, c, field, "move")?;
    }
    let mut moved = edited.features.remove(from);
    if !stays {
        join(
            &mut edited,
            &mut moved,
            base,
            consumer_of.map(|(c, f)| (at(c), f)),
            place_to,
        )?;
    }
    edited.features.insert(place_to, moved);
    let rewired = rewired(document, &edited);
    Ok(Moved {
        document: edited,
        from,
        to: place_to,
        rewired,
        changed: true,
    })
}

/// `moved` joined to the chain it left in `edited` (which no longer holds it), at
/// `place_to`: its base becomes the chain's last member before that place, and the next
/// member after it takes it as its base. The chain is what's left of the one it was in:
/// its old `base` and that one's own bases, and the features after it down the `base`
/// links from its old consumer (`consumed`, at its place in `edited`, and the field that
/// named it).
fn join(
    edited: &mut Document,
    moved: &mut Feature,
    base: Option<&FeatureId>,
    consumed: Option<(usize, &'static str)>,
    place_to: usize,
) -> Result<(), Error> {
    let mut members: Vec<usize> = Vec::new();
    // Up the chain from its old base.
    let mut up = base.and_then(|b| place(edited, b));
    while let Some(i) = up {
        if members.contains(&i) {
            break;
        }
        members.push(i);
        up = base_of(&edited.features[i].operation).and_then(|b| place(edited, b));
    }
    // Down the chain from its old consumer, where it named it as its base: each next member
    // the feature whose base the last is.
    let mut down = consumed
        .filter(|(_, field)| *field == "base")
        .map(|(c, _)| c);
    while let Some(i) = down {
        if members.contains(&i) {
            break;
        }
        members.push(i);
        down = consumer(edited, &edited.features[i].id)
            .filter(|(_, field)| *field == "base")
            .map(|(c, _)| c);
    }
    members.sort_unstable();
    let previous = members.iter().rev().find(|&&m| m < place_to).copied();
    let next = members.iter().find(|&&m| m >= place_to).copied();

    // The chain's end, if a boolean consumes it as its tool: a move to its end would make
    // the moved feature that tool.
    if next.is_none()
        && let Some(&last) = members.last()
        && let Some((boolean, "tool")) = consumer(edited, &edited.features[last].id)
    {
        let (boolean, end) = (&edited.features[boolean].id, &edited.features[last].id);
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{} would leave the chain {end} ends, which {boolean} consumes as its tool: a move never makes a feature another's tool",
                moved.id
            ),
        )
        .with_feature(moved.id.as_str())
        .with_parameter("before")
        .with_suggestion(format!(
            "move {} before {end}, within the tool's chain, or set {boolean}'s tool first",
            moved.id
        )));
    }

    let wants_base = has_base_field(&moved.operation);
    match previous {
        Some(p) => {
            let previous = edited.features[p].id.clone();
            if !wants_base {
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "{}, a {}, takes no base, and the place asked for comes after {previous} in its chain",
                        moved.id,
                        moved.operation.type_name()
                    ),
                )
                .with_feature(moved.id.as_str())
                .with_parameter("before")
                .with_suggestion(format!(
                    "move {} before its chain's first feature, or move another feature",
                    moved.id
                )));
            }
            set_input(&mut moved.operation, "base", Some(previous));
        }
        None if base_optional(&moved.operation) => {
            set_input(&mut moved.operation, "base", None);
        }
        None if wants_base => {
            let root = members
                .first()
                .map(|&m| edited.features[m].id.to_string())
                .unwrap_or_default();
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "{}, a {}, can't be placed before {root}, its chain's root: it needs a base",
                    moved.id,
                    moved.operation.type_name()
                ),
            )
            .with_feature(moved.id.as_str())
            .with_parameter("before")
            .with_suggestion(format!("move {} after {root}", moved.id)));
        }
        None => {}
    }
    if let Some(n) = next {
        let next_feature = &mut edited.features[n];
        if !has_base_field(&next_feature.operation) {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "{} would come before {}, its chain's root, a {}, which takes no base",
                    moved.id,
                    next_feature.id,
                    next_feature.operation.type_name()
                ),
            )
            .with_feature(moved.id.as_str())
            .with_parameter("before")
            .with_suggestion(format!("move {} after {}", moved.id, next_feature.id)));
        }
        set_input(&mut next_feature.operation, "base", Some(moved.id.clone()));
    }
    Ok(())
}

/// The consumer `at` of `removed` (by `field`) given `removed`'s base in its place, in
/// `edited`: refused where it has none and the consumer needs one there.
fn stand_in(
    document: &Document,
    edited: &mut Document,
    removed: &Feature,
    at: usize,
    field: &'static str,
    verb: &str,
) -> Result<(), Error> {
    let user = &document.features[at];
    match base_of(&removed.operation) {
        Some(base) => {
            set_input(
                &mut edited.features[at].operation,
                field,
                Some(base.clone()),
            );
            Ok(())
        }
        None if field == "base" && base_optional(&user.operation) => {
            set_input(&mut edited.features[at].operation, field, None);
            Ok(())
        }
        None => {
            // Earlier features nothing consumes, which could be its base instead.
            let candidates: Vec<&FeatureId> = document.features[..at]
                .iter()
                .filter(|f| f.id != removed.id && f.operation.makes_solid())
                .filter(|f| consumer(document, &f.id).is_none())
                .map(|f| &f.id)
                .collect();
            let what = if field == "tool" {
                format!(
                    "{} is {}'s tool, and has no base to stand in for it",
                    removed.id, user.id
                )
            } else {
                format!(
                    "{} is the root of {}'s chain, and {}, a {}, can't be left without a base",
                    removed.id,
                    user.id,
                    user.id,
                    user.operation.type_name()
                )
            };
            let suggestion = if candidates.is_empty() && verb == "move" {
                format!(
                    "nothing before {} but {} can be its {field}, so {} can't move past it; move another feature instead, or remove {} first",
                    user.id, removed.id, removed.id, user.id
                )
            } else if candidates.is_empty() {
                format!(
                    "nothing before {} but {} can be its {field}; remove {} first",
                    user.id, removed.id, user.id
                )
            } else {
                format!(
                    "set {}'s {field} to one of the candidates first (set_field), then {verb} {}",
                    user.id, removed.id
                )
            };
            Err(Error::new(ErrorCode::InvalidInput, what)
                .with_feature(removed.id.as_str())
                .with_parameter("feature")
                .with_suggestion(suggestion)
                .with_candidates(candidates.iter().map(|c| c.as_str())))
        }
    }
}

/// The refusal of a move that puts `moving` before `parent`.
fn come_before(moving: &Feature, parent: &FeatureId, why: &str) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("{} would come before {parent}, {why}", moving.id),
    )
    .with_feature(moving.id.as_str())
    .with_parameter("before")
    .with_suggestion(format!(
        "move {parent} first, or move {} after it",
        moving.id
    ))
}

fn no_feature(id: &FeatureId, parameter: &str) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("the document has no feature {id}"),
    )
    .with_parameter(parameter)
    .with_suggestion("name a feature by its ID")
}

/// Why a feature can't do without another: it's its sketch, or it stores names of its
/// elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Child {
    Sketch,
    Names,
}

impl Child {
    /// Why, as "{child} {why}" says it of `parent`.
    fn why(self, parent: &FeatureId) -> String {
        match self {
            Child::Sketch => format!("uses {parent} as its sketch"),
            Child::Names => format!("names {parent}'s elements"),
        }
    }
}

/// The features that can't do without `id` where they are: those whose sketch it is, and
/// those storing a name one of whose makers it is, in timeline order, each with why.
fn children<'a>(
    document: &'a Document,
    id: &'a FeatureId,
) -> impl Iterator<Item = (&'a Feature, Child)> + 'a {
    document
        .features
        .iter()
        .filter(move |f| f.id != *id)
        .filter_map(move |f| {
            if f.operation.sketch() == Some(id) {
                Some((f, Child::Sketch))
            } else if makers_named(document, f).contains(&id) {
                Some((f, Child::Names))
            } else {
                None
            }
        })
}

/// Every feature the element names `feature` stores name as a maker, in the order they're
/// written; a name that doesn't parse names none (the gate refuses it on its own).
fn makers_named<'a>(document: &'a Document, feature: &'a Feature) -> Vec<&'a FeatureId> {
    let maker = |id: &FeatureId| {
        document
            .features
            .iter()
            .find(|f| f.id == *id)
            .map(|f| Maker::of(&f.operation))
    };
    let mut found: Vec<&FeatureId> = Vec::new();
    for edge in feature.operation.edges() {
        if let Ok(name) = Name::parse(edge.as_str(), &maker) {
            for id in name.features() {
                if let Some(held) = document.features.iter().find(|f| f.id == *id)
                    && !found.contains(&&held.id)
                {
                    found.push(&held.id);
                }
            }
        }
    }
    found
}

fn place(document: &Document, id: &FeatureId) -> Option<usize> {
    document.features.iter().position(|f| f.id == *id)
}

/// The feature that consumes `id`'s solid, by its place and the field naming it, if one does.
fn consumer(document: &Document, id: &FeatureId) -> Option<(usize, &'static str)> {
    document.features.iter().enumerate().find_map(|(at, f)| {
        f.operation
            .inputs()
            .into_iter()
            .find(|(_, input)| *input == id)
            .map(|(field, _)| (at, field))
    })
}

fn base_of(operation: &Operation) -> Option<&FeatureId> {
    operation
        .inputs()
        .into_iter()
        .find(|(field, _)| *field == "base")
        .map(|(_, base)| base)
}

/// Whether a feature of this type has a `base` at all (a primitive and a sketch haven't).
fn has_base_field(operation: &Operation) -> bool {
    !input_fields(operation).is_empty()
}

/// Whether its `base` may be left out: a pad's or a revolve's, which can begin a body.
fn base_optional(operation: &Operation) -> bool {
    matches!(operation, Operation::Pad { .. } | Operation::Revolve { .. })
}

/// Its `base` and `tool` fields, each with what it names.
fn input_fields(operation: &Operation) -> Vec<(&'static str, Option<&FeatureId>)> {
    match operation {
        Operation::Fuse { base, tool }
        | Operation::Cut { base, tool }
        | Operation::Common { base, tool } => vec![("base", Some(base)), ("tool", Some(tool))],
        Operation::Fillet { base, .. }
        | Operation::Chamfer { base, .. }
        | Operation::Pocket { base, .. }
        | Operation::Groove { base, .. } => vec![("base", Some(base))],
        Operation::Pad { base, .. } | Operation::Revolve { base, .. } => {
            vec![("base", base.as_ref())]
        }
        Operation::Box { .. }
        | Operation::Cylinder { .. }
        | Operation::Sphere { .. }
        | Operation::Cone { .. }
        | Operation::Torus { .. }
        | Operation::Sketch(_) => Vec::new(),
    }
}

/// `operation`'s `field` set to `to`. A required field is left as it is when `to` is none:
/// every caller has refused that already.
fn set_input(operation: &mut Operation, field: &str, to: Option<FeatureId>) {
    match (operation, field) {
        (
            Operation::Fuse { base, .. }
            | Operation::Cut { base, .. }
            | Operation::Common { base, .. }
            | Operation::Fillet { base, .. }
            | Operation::Chamfer { base, .. }
            | Operation::Pocket { base, .. }
            | Operation::Groove { base, .. },
            "base",
        ) => {
            if let Some(to) = to {
                *base = to;
            }
        }
        (
            Operation::Fuse { tool, .. }
            | Operation::Cut { tool, .. }
            | Operation::Common { tool, .. },
            "tool",
        ) => {
            if let Some(to) = to {
                *tool = to;
            }
        }
        (Operation::Pad { base, .. } | Operation::Revolve { base, .. }, "base") => *base = to,
        _ => {}
    }
}

/// Every `base` and `tool` `after` holds otherwise than `before` held it, in `after`'s
/// timeline order: what an edit rewired. A feature `before` hasn't (one inserted) is its
/// own as given, not rewired.
fn rewired(before: &Document, after: &Document) -> Vec<Rewired> {
    let mut changed = Vec::new();
    for feature in &after.features {
        let Some(old) = before.features.iter().find(|f| f.id == feature.id) else {
            continue;
        };
        let old_fields = input_fields(&old.operation);
        for (field, to) in input_fields(&feature.operation) {
            let from = old_fields
                .iter()
                .find(|(f, _)| *f == field)
                .and_then(|(_, from)| *from);
            if from != to {
                changed.push(Rewired {
                    feature: feature.id.clone(),
                    field,
                    from: from.cloned(),
                    to: to.cloned(),
                });
            }
        }
    }
    changed
}
