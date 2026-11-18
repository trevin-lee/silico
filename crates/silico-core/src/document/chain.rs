//! A document's timeline edited as its chains have it (the M5.0 design, section 7's chain,
//! A9, and section 12's items 1, 2, and 5): the `base` links in the text are each body's
//! chain, exactly as written. Inserting a feature, moving one, and removing one edit the
//! chain in the text, and say every `base` and `tool` they changed ([`Rewired`]). Every rule
//! is a function of the text: chains by `base` links, consumers by the gate's one-consumer
//! rule, the makers of a stored element name by parsing it.
//!
//! Pure functions over a document the gate took ([`Checked`]): each gives the edited
//! document, which the caller puts through the gate ([`validate`](super::validate)), or why
//! the edit is refused, naming both features where a child would lose its parent. The
//! session's edits run them (`add_feature` with a `before`, `move_feature`, and
//! `remove_feature`). Each costs about what the gate does: the document indexed once, by ID
//! and by consumer, never searched feature by feature.

use std::collections::{BTreeMap, BTreeSet};

use super::{Checked, Document, Feature, FeatureId, Operation};
use crate::naming::{Maker, Name};
use crate::references::CANDIDATES;
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
/// twice), where its base and tool are one solid (as the gate words it), and where `before`
/// names no feature.
pub fn insert(
    checked: &Checked,
    feature: Feature,
    before: Option<&FeatureId>,
) -> Result<Inserted, Error> {
    let document = checked.document();
    let index = Index::of(document);
    let place = insert_place(document, before)?;
    let inputs = feature.operation.inputs();
    if let [(_, base), (field, tool)] = inputs.as_slice()
        && base == tool
    {
        let what = feature.operation.type_name();
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("{field} {tool} is base too: a {what} takes two different solids"),
        )
        .with_feature(feature.id.as_str())
        .with_parameter("feature.tool")
        .with_suggestion(format!("give the {what} a different {field}")));
    }
    let consumed: Vec<(FeatureId, usize, &'static str)> = inputs
        .iter()
        .filter_map(|(_, input)| {
            index
                .consumer(input)
                .map(|(at, by)| ((*input).clone(), at, by))
        })
        .collect();
    if let [(base, base_at, _), (tool, tool_at, _)] = consumed.as_slice() {
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
    for (input, at, field) in &consumed {
        let by = &document.features[*at].id;
        if *at < place {
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
    edited.features.insert(place, feature);
    let rewired = rewired(document, &edited);
    Ok(Inserted {
        document: edited,
        index: place,
        rewired,
    })
}

/// Where a feature added before the feature `before` goes in `document`, from 0: that
/// feature's place, or the end for none. Refused, naming `before`, where it names no feature.
pub fn insert_place(document: &Document, before: Option<&FeatureId>) -> Result<usize, Error> {
    let Some(target) = before else {
        return Ok(document.features.len());
    };
    document
        .features
        .iter()
        .position(|feature| feature.id == *target)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidInput,
                format!("the document has no feature {target} to add this one before"),
            )
            .with_parameter("before")
            .with_suggestion("name a feature by its ID, or give null to add at the end")
        })
}

/// The feature `id` removed from `document` (the design's `remove_feature`, as A9 amends
/// it). Its consumer, if it has one, takes its base: the field that named it now names
/// what it named, listed in [`Removed::rewired`], or none for a pad's or revolve's base,
/// which then begins the body. Refused, naming both features, where nothing can stand in:
/// a chain's root under a feature that needs a base (or as a boolean's tool), a feature
/// whose sketch it is, a feature that stores names of its elements (checked directly, since
/// a removed maker's name no longer parses), a pattern that copies it, and a feature whose
/// stored names' makers its base would no longer be built from (the lineage rule, open item
/// 2b). Refused with the first refusal; [`remove_all`] gives every one.
pub fn remove(checked: &Checked, id: &FeatureId) -> Result<Removed, Error> {
    remove_all(checked, id).map_err(|refusals| {
        refusals.into_iter().next().unwrap_or_else(|| {
            Error::new(
                ErrorCode::Internal,
                format!("removing {id} was refused with no reason given"),
            )
            .with_suggestion(crate::worker::SILICO_BUG)
        })
    })
}

/// As [`remove`], refused with every feature that can't do without `id`, at once
/// (`remove_feature`: a pattern copying it and a fillet naming its edges alike,
/// docs/design/m6.0-patterns.md, section 6), the children found in one pass.
pub fn remove_all(checked: &Checked, id: &FeatureId) -> Result<Removed, Vec<Error>> {
    let document = checked.document();
    let index = Index::of(document);
    let at = index
        .place(id)
        .ok_or_else(|| vec![no_feature(id, "feature")])?;
    let removed = &document.features[at];
    let refusals = removal_refusals(checked, id);
    if !refusals.is_empty() {
        return Err(refusals);
    }
    let mut edited = document.clone();
    if let Some((c, field)) = index.consumer(id) {
        stand_in(document, &index, &mut edited, removed, c, field, "remove")
            .map_err(|e| vec![e])?;
    }
    edited.features.remove(at);
    if let Some((feature, maker)) = lineage_broken(document, &edited) {
        return Err(vec![Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{feature} names elements of {maker}, which its base would no longer be built from without {}",
                removed.id
            ),
        )
        .with_feature(removed.id.as_str())
        .with_parameter("feature")
        .with_suggestion(format!("remove {feature} first"))]);
    }
    let rewired = rewired(document, &edited);
    Ok(Removed {
        document: edited,
        feature: removed.clone(),
        index: at,
        rewired,
    })
}

/// Every refusal of removing `id` for a feature that can't do without it, one a child, in
/// timeline order, naming both (the design's `remove_feature`, and a pattern's originals,
/// docs/design/m6.0-patterns.md, section 6): its sketch's users, the features naming its
/// elements, and the patterns copying it. [`remove`] refuses with the first; `remove_feature`
/// gives them all at once. None for a feature the document hasn't.
pub fn removal_refusals(checked: &Checked, id: &FeatureId) -> Vec<Error> {
    let document = checked.document();
    let index = Index::of(document);
    let Some(removed) = index.place(id).and_then(|at| document.features.get(at)) else {
        return Vec::new();
    };
    children(document, &index, id)
        .map(|(child, why)| {
            Error::new(
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
                // A sketch's stored name is its face's; a fillet's or chamfer's, its edges'.
                Child::Names if matches!(child.operation, Operation::Sketch(_)) => format!(
                    "remove {} first, or set its plane.face to another feature's face",
                    child.id
                ),
                Child::Names => format!(
                    "remove {} first, or set its {} to names of other features' elements",
                    child.id,
                    names_field(&child.operation)
                ),
                Child::Copies => format!(
                    "remove {} first, or take {} out of its copies.features",
                    child.id, removed.id
                ),
            })
        })
        .collect()
}

/// The feature `id` moved before the feature `before`, or to the end for none (the design's
/// `move_feature`): a removal then an insert. Its consumer takes its base, as [`remove`]
/// rewires it; its base becomes the last member of its chain before the new place (none for
/// a pad or revolve placed before the chain's root); and the chain's next member after that
/// place takes it as its base, never as its tool. A move that leaves it between its base and
/// its consumer rewires nothing; a move to where it is (before the feature after it, or to
/// the end when it's last) changes nothing; and a move before itself is refused, naming
/// `before`, since it names no place.
///
/// Refused, naming both features: a move that puts it before its sketch or its tool; after
/// a feature that uses it as its sketch; one that leaves a chain's root's consumer without
/// a base; one that would make it the end of a chain a boolean consumes as its tool; one
/// that would give a primitive a base or leave a feature that needs one without; and one
/// after which a feature's base is no longer built from a maker of the names it stores (the
/// lineage rule, open item 2b), which is how a move before a feature whose elements it
/// names, or after one that names its elements, is refused (SolidWorks refuses a reorder
/// that puts a child before what its references come from: docs/ux/solidworks.md, §5,
/// *Editing upstream*). A stored name whose maker was outside its feature's lineage already
/// (broken before the move, the rebuild's to report) isn't the lineage rule's: a move that
/// puts its maker after it gives a document the gate refuses, as the caller puts it through
/// the gate.
///
/// A move within a chain a boolean consumes as its tool can't always be undone by moving
/// back: the move that would make the feature that boolean's tool again is refused.
pub fn move_feature(
    checked: &Checked,
    id: &FeatureId,
    before: Option<&FeatureId>,
) -> Result<Moved, Error> {
    let document = checked.document();
    let index = Index::of(document);
    let from = index.place(id).ok_or_else(|| no_feature(id, "feature"))?;
    let target = match before {
        None => document.features.len(),
        Some(target) => index
            .place(target)
            .ok_or_else(|| no_feature(target, "before"))?,
    };
    if target == from {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("{id} can't be moved before itself"),
        )
        .with_feature(id.as_str())
        .with_parameter("before")
        .with_suggestion(format!(
            "name the feature {id} is to come before, another than {id} itself, or give null to move it to the end"
        )));
    }
    if target == from + 1 {
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
    let others = |other: &FeatureId| index.place(other).map(at);

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
    // A pattern before an original it copies (docs/design/m6.0-patterns.md, section 6).
    if let Some(original) = moving
        .operation
        .originals()
        .iter()
        .find(|original| others(original).is_some_and(|p| p >= place_to))
    {
        return Err(come_before(moving, original, "which it copies"));
    }
    // A feature whose elements it names, or that names its elements, is the lineage
    // rule's to refuse (open item 2b), below: a stored name whose makers its base was built
    // from must keep them.
    if let Some((child, why)) = children(document, &index, id)
        .filter(|(_, why)| matches!(why, Child::Sketch | Child::Copies))
        .find(|(child, _)| others(&child.id).is_some_and(|p| p < place_to))
    {
        let (which, suggestion) = match why {
            Child::Copies => (
                "copies it".to_owned(),
                format!(
                    "move {} after it too, or take {} out of {}'s copies.features",
                    child.id, moving.id, child.id
                ),
            ),
            _ => (
                why.why(id),
                format!(
                    "move {} after {} first, or move {} before it",
                    child.id, moving.id, moving.id
                ),
            ),
        };
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("{} would come after {}, which {which}", moving.id, child.id),
        )
        .with_feature(moving.id.as_str())
        .with_parameter("before")
        .with_suggestion(suggestion));
    }

    let base = moving.operation.base();
    let consumer = index.consumer(id);
    // Left between its base and its consumer: only its place changes.
    let after_base = base.and_then(others).is_none_or(|p| p < place_to);
    let by_consumer = consumer.is_none_or(|(c, _)| place_to <= at(c));
    let stays = after_base && by_consumer;
    let mut edited = document.clone();
    if !stays && let Some((c, field)) = consumer {
        stand_in(document, &index, &mut edited, moving, c, field, "move")?;
    }
    let mut moved = edited.features.remove(from);
    if !stays {
        join(
            &mut edited,
            &mut moved,
            base,
            consumer.map(|(c, f)| (at(c), f)),
            place_to,
        )?;
    }
    edited.features.insert(place_to, moved);
    if let Some((feature, maker)) = lineage_broken(document, &edited) {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{feature} names elements of {maker}, which its base would no longer be built from once {} moves there",
                moving.id
            ),
        )
        .with_feature(moving.id.as_str())
        .with_parameter("before")
        .with_suggestion(if feature == moving.id {
            format!(
                "keep {feature} after {maker}, whose elements it names, or set its {} to names of other features' elements",
                names_field(&moving.operation)
            )
        } else {
            format!("move {} elsewhere, or remove {feature} first", moving.id)
        }));
    }
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
    // The chain's members and its end's consumer, read before anything's written.
    let (previous, next, tool_end, root) = {
        let index = Index::of(edited);
        let mut members: BTreeSet<usize> = BTreeSet::new();
        // Up the chain from its old base.
        let mut up = base.and_then(|b| index.place(b));
        while let Some(i) = up.filter(|i| members.insert(*i)) {
            up = edited
                .features
                .get(i)
                .and_then(|f| f.operation.base())
                .and_then(|b| index.place(b));
        }
        // Down the chain from its old consumer, where it named it as its base: each next
        // member the feature whose base the last is.
        let mut down = consumed
            .filter(|(_, field)| *field == "base")
            .map(|(c, _)| c);
        while let Some(i) = down.filter(|i| members.insert(*i)) {
            down = edited
                .features
                .get(i)
                .and_then(|f| index.consumer(&f.id))
                .filter(|(_, field)| *field == "base")
                .map(|(c, _)| c);
        }
        let previous = members.range(..place_to).next_back().copied();
        let next = members.range(place_to..).next().copied();
        // The chain's end, if a boolean consumes it as its tool: a move to its end would
        // make the moved feature that tool.
        let tool_end = members.last().and_then(|&last| {
            let end = edited.features.get(last)?;
            match index.consumer(&end.id) {
                Some((boolean, "tool")) => {
                    Some((edited.features.get(boolean)?.id.clone(), end.id.clone()))
                }
                _ => None,
            }
        });
        let root = members
            .first()
            .and_then(|&m| edited.features.get(m))
            .map(|f| f.id.clone());
        (previous, next, tool_end, root)
    };

    if next.is_none()
        && let Some((boolean, end)) = tool_end
    {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{} would follow {end}, the end of the chain {boolean} consumes as its tool: a move never makes a feature another's tool",
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
    match previous.and_then(|p| edited.features.get(p)) {
        Some(previous) => {
            if !wants_base {
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "{}, a {}, takes no base, and the place asked for comes after {} in its chain",
                        moved.id,
                        moved.operation.type_name(),
                        previous.id
                    ),
                )
                .with_feature(moved.id.as_str())
                .with_parameter("before")
                .with_suggestion(format!(
                    "move {} before its chain's first feature, or move another feature",
                    moved.id
                )));
            }
            let previous = previous.id.clone();
            set_input(&mut moved.operation, "base", Some(previous));
        }
        None if base_optional(&moved.operation) => {
            set_input(&mut moved.operation, "base", None);
        }
        // A feature with no base field never comes here: with no consumer, a move leaves it
        // where it stands; its consumer a boolean's tool, a pocket, fillet, or chamfer, the
        // stand-in refuses; a pad or revolve, that consumer is a member before the place,
        // and the case above refuses it, as taking no base.
        None => {
            let root = root.map(|r| r.to_string()).unwrap_or_default();
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
    }
    if let Some(next_feature) = next.and_then(|n| edited.features.get_mut(n)) {
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
/// `edited`: refused where it has none and the consumer needs one there, with up to
/// [`CANDIDATES`] earlier solids nothing consumes, which could be its base instead.
fn stand_in(
    document: &Document,
    index: &Index,
    edited: &mut Document,
    removed: &Feature,
    at: usize,
    field: &'static str,
    verb: &str,
) -> Result<(), Error> {
    let Some(user) = document.features.get(at) else {
        return Ok(());
    };
    let Some(target) = edited.features.get_mut(at) else {
        return Ok(());
    };
    match removed.operation.base() {
        Some(base) => {
            set_input(&mut target.operation, field, Some(base.clone()));
            Ok(())
        }
        None if field == "base" && base_optional(&user.operation) => {
            set_input(&mut target.operation, field, None);
            Ok(())
        }
        None => {
            let candidates: Vec<&str> = document.features[..at]
                .iter()
                .filter(|f| f.id != removed.id && f.operation.makes_solid())
                .filter(|f| index.consumer(&f.id).is_none())
                .take(CANDIDATES)
                .map(|f| f.id.as_str())
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
                .with_candidates(candidates))
        }
    }
}

/// The first feature of `edited`, in its timeline order, with a stored name whose makers
/// were all in what its base was built from in `before`, one of which its base is no longer
/// built from; with that maker: the lineage rule (open item 2b, as decided), which extends
/// the design's "names its elements" to what a feature's base is built from, so an edit
/// that would break a reference the text can see is refused rather than left to fail at the
/// rebuild. A name
/// one of whose makers was outside the base's lineage before the edit was broken already:
/// it's the rebuild's to report, whatever the edit does to its other makers.
fn lineage_broken(before: &Document, edited: &Document) -> Option<(FeatureId, FeatureId)> {
    let (old, new) = (Lineage::of(before), Lineage::of(edited));
    edited.features.iter().find_map(|feature| {
        let base = feature.operation.base()?;
        let was = old
            .index
            .feature(&feature.id)
            .and_then(|f| f.operation.base())?;
        feature.operation.stored_names().find_map(|(_, stored)| {
            let makers: Vec<&FeatureId> = name_makers(&new.index, stored.as_str())
                .into_iter()
                .filter(|maker| **maker != feature.id)
                .collect();
            if !makers.iter().all(|maker| old.built_from(was, maker)) {
                return None;
            }
            makers
                .into_iter()
                .find(|maker| !new.built_from(base, maker))
                .map(|maker| (feature.id.clone(), maker.clone()))
        })
    })
}

/// What each feature of a document is built from, read once: a walk of the document's
/// solids down from each one nothing consumes, through the features each consumes, gives
/// each a span, so a solid is built from another exactly when the other's place in the walk
/// falls in its span, and from a sketch when one of the sketch's users' does. Each question
/// then costs a search, not a walk up the chain.
struct Lineage<'a> {
    index: Index<'a>,
    /// By place in the timeline: the feature's place in the walk, and the last place of
    /// anything it's built from; none for one the walk never reached (only in a document
    /// the gate would refuse).
    spans: Vec<Option<(usize, usize)>>,
    /// Each sketch's users' places in the walk.
    users: BTreeMap<&'a FeatureId, BTreeSet<usize>>,
}

impl<'a> Lineage<'a> {
    fn of(document: &'a Document) -> Self {
        let index = Index::of(document);
        let features = &document.features;
        let mut spans: Vec<Option<(usize, usize)>> = vec![None; features.len()];
        let mut walked: usize = 0;
        for (root, feature) in features.iter().enumerate() {
            if index.consumer(&feature.id).is_some() {
                continue;
            }
            // Each feature entered, then left once everything it's built from was walked.
            let mut stack = vec![(root, false)];
            while let Some((at, leaving)) = stack.pop() {
                let Some(span) = spans.get_mut(at) else {
                    continue;
                };
                match (leaving, *span) {
                    // Entered at `start`, so `walked` is past it.
                    (true, Some((start, _))) => *span = Some((start, walked.saturating_sub(1))),
                    (false, None) => {
                        *span = Some((walked, walked));
                        walked += 1;
                        stack.push((at, true));
                        let inputs = features.get(at).map(|f| f.operation.inputs());
                        for (_, input) in inputs.into_iter().flatten() {
                            if let Some(place) = index.place(input) {
                                stack.push((place, false));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut users: BTreeMap<&'a FeatureId, BTreeSet<usize>> = BTreeMap::new();
        for (at, feature) in features.iter().enumerate() {
            if let Some(sketch) = feature.operation.sketch()
                && let Some(Some((walk, _))) = spans.get(at)
            {
                users.entry(sketch).or_default().insert(*walk);
            }
        }
        Lineage {
            index,
            spans,
            users,
        }
    }

    fn span(&self, id: &FeatureId) -> Option<(usize, usize)> {
        self.spans.get(self.index.place(id)?).copied().flatten()
    }

    /// Whether `base` is built from `maker`: is it, or one of the solids it's built from, or
    /// has a sketch one of those sweeps.
    fn built_from(&self, base: &FeatureId, maker: &FeatureId) -> bool {
        let Some((start, end)) = self.span(base) else {
            return false;
        };
        self.span(maker)
            .is_some_and(|(walk, _)| start <= walk && walk <= end)
            || self
                .users
                .get(maker)
                .is_some_and(|users| users.range(start..=end).next().is_some())
    }
}

/// Why a feature can't do without another: it's its sketch, it stores names of its
/// elements, or it's an original it copies (a pattern is a child of each original,
/// docs/design/m6.0-patterns.md, section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Child {
    Sketch,
    Names,
    Copies,
}

impl Child {
    /// Why, as "{child} {why}" says it of `parent`.
    fn why(self, parent: &FeatureId) -> String {
        match self {
            Child::Sketch => format!("uses {parent} as its sketch"),
            Child::Names => format!("names {parent}'s elements"),
            Child::Copies => format!("copies {parent}"),
        }
    }
}

/// The features that can't do without `id` where they are: those whose sketch it is, those
/// storing a name one of whose makers it is, and the patterns copying it, in timeline order,
/// each with why.
fn children<'a>(
    document: &'a Document,
    index: &'a Index,
    id: &'a FeatureId,
) -> impl Iterator<Item = (&'a Feature, Child)> + 'a {
    document
        .features
        .iter()
        .filter(move |f| f.id != *id)
        .filter_map(move |f| {
            if f.operation.sketch() == Some(id) {
                Some((f, Child::Sketch))
            } else if f.operation.originals().contains(id) {
                Some((f, Child::Copies))
            } else if makers_named(index, f).contains(&id) {
                Some((f, Child::Names))
            } else {
                None
            }
        })
}

/// The field of `operation` that stores element names, as a suggestion names it: a fillet's
/// or chamfer's `edges`, a sketch's `plane.face`, a linear pattern's directions' `along`, a
/// polar pattern's `axis`, a mirror's `plane`.
fn names_field(operation: &Operation) -> &'static str {
    match operation {
        Operation::Sketch(_) => "plane.face",
        Operation::LinearPattern { .. } => "directions' along",
        Operation::PolarPattern { .. } => "axis",
        Operation::Mirror { .. } => "plane",
        _ => "edges",
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

/// The refusal of an edit naming a feature the document hasn't: as its `feature`, the one
/// to remove or move, or as a move's `before`.
fn no_feature(id: &FeatureId, parameter: &str) -> Error {
    let (message, suggestion) = if parameter == "before" {
        (
            format!("the document has no feature {id} to move this one before"),
            "name a feature by its ID, or give null to move it to the end",
        )
    } else {
        (
            format!("the document has no feature {id}"),
            "name a feature by its ID, as the document holds it",
        )
    };
    Error::new(ErrorCode::InvalidInput, message)
        .with_parameter(parameter)
        .with_suggestion(suggestion)
}

/// Every feature the element names `feature` stores ([`Operation::stored_names`]: a
/// fillet's or chamfer's `edges`, a sketch's face) name as a maker, as the document holds
/// its ID, in the order they're written; a name that doesn't parse names none (the gate
/// refuses it on its own).
fn makers_named<'a>(index: &Index<'a>, feature: &Feature) -> Vec<&'a FeatureId> {
    let mut found: Vec<&'a FeatureId> = Vec::new();
    let mut seen: BTreeSet<&'a FeatureId> = BTreeSet::new();
    for (_, stored) in feature.operation.stored_names() {
        for maker in name_makers(index, stored.as_str()) {
            if seen.insert(maker) {
                found.push(maker);
            }
        }
    }
    found
}

/// Every feature the element name `text` names as a maker, as the document holds its ID,
/// each once, in the order written; none if it doesn't parse.
fn name_makers<'a>(index: &Index<'a>, text: &str) -> Vec<&'a FeatureId> {
    let maker = |id: &FeatureId| index.feature(id).map(|f| Maker::of(&f.operation));
    let Ok(name) = Name::parse(text, &maker) else {
        return Vec::new();
    };
    name.features()
        .into_iter()
        .filter_map(|id| index.feature(id).map(|held| &held.id))
        .collect()
}

/// A document's features by ID, and each solid's consumer, read once: each lookup a search
/// of a sorted map, so an edit costs about what the gate does.
struct Index<'a> {
    features: &'a [Feature],
    places: BTreeMap<&'a FeatureId, usize>,
    consumers: BTreeMap<&'a FeatureId, (usize, &'static str)>,
}

impl<'a> Index<'a> {
    fn of(document: &'a Document) -> Self {
        let mut places = BTreeMap::new();
        let mut consumers = BTreeMap::new();
        for (at, feature) in document.features.iter().enumerate() {
            places.entry(&feature.id).or_insert(at);
            for (field, input) in feature.operation.inputs() {
                consumers.entry(input).or_insert((at, field));
            }
        }
        Index {
            features: &document.features,
            places,
            consumers,
        }
    }

    fn place(&self, id: &FeatureId) -> Option<usize> {
        self.places.get(id).copied()
    }

    fn feature(&self, id: &FeatureId) -> Option<&'a Feature> {
        self.features.get(self.place(id)?)
    }

    /// The feature that consumes `id`'s solid, by its place and the field naming it.
    fn consumer(&self, id: &FeatureId) -> Option<(usize, &'static str)> {
        self.consumers.get(id).copied()
    }
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
        | Operation::Groove { base, .. }
        | Operation::LinearPattern { base, .. }
        | Operation::PolarPattern { base, .. }
        | Operation::Mirror { base, .. } => vec![("base", Some(base))],
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
            | Operation::Groove { base, .. }
            | Operation::LinearPattern { base, .. }
            | Operation::PolarPattern { base, .. }
            | Operation::Mirror { base, .. },
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
    let old = Index::of(before);
    let mut changed = Vec::new();
    for feature in &after.features {
        let Some(previous) = old.feature(&feature.id) else {
            continue;
        };
        let old_fields = input_fields(&previous.operation);
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
