//! The validation gate: whether a document's features fit together and the kernel can
//! make each (docs/design/m2.1-document-format.md, D2). Reading checks each value on its
//! own; the gate checks what features say about each other: every ID names one feature,
//! every reference is to a feature before the one making it (so the timeline has no
//! cycles), every solid is consumed at most once (and is a solid: a sketch makes none),
//! every primitive's, fillet's, and chamfer's values are ones the kernel accepts, by the
//! kernel's own rules, every sketch holds together (`sketch_gate`), and every element a
//! fillet or chamfer names is named as Silico names an edge (docs/ports/naming.md), by
//! features before it. Whether a name finds its edge is the rebuild's to say: it's in the
//! base's solid, which only a rebuild makes. A pad, pocket, revolve, or groove
//! (docs/design/m3.6-sketch-features.md, S1) sweeps a sketch before it, which it uses
//! without consuming (any number of features may sweep one sketch), goes a length above 0
//! and at most a kilometre or turns an angle above 0° and at most 360°, and turns about the
//! sketch's x or y axis or a line of the sketch. What only a solve can tell (an axis whose
//! ends the solve brings together) is the rebuild's.

use silico_protocol::{ChamferSize, MAX_LENGTH, check_fillet_radius, check_offset, format_number};

use super::json::shown;
use super::model::{Axis, Document, ExtentValue, FeatureId, ID_LENGTH, NamingScheme, Operation};
use super::quantity::Length;
use super::sketch::ElementKind as SketchElement;
use crate::kernel::ElementKind;
use crate::naming::{Maker, Name, OpCode, TypeWord};
use crate::{Error, ErrorCode};

/// How many of a document's problems [`Invalid`] lists; it counts the rest.
pub const PROBLEMS_LISTED: usize = 100;

/// The farthest a pad or pocket may go, in nanometres: [`MAX_LENGTH`], a kilometre, each
/// length and a two-sided prism's whole length alike.
const MAX_SWEEP_NM: i64 = (MAX_LENGTH * 1e6) as i64;

/// The most a revolve or groove may turn, in nanodegrees: a whole turn.
const MAX_TURN_NDEG: i64 = 360 * 1_000_000_000;

/// Why a document isn't valid: its problems, in timeline order, as `invalid_input` errors
/// naming the feature and the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    errors: Vec<Error>,
    total: u64,
}

impl Invalid {
    /// The first [`PROBLEMS_LISTED`] problems, in timeline order.
    pub fn errors(&self) -> &[Error] {
        &self.errors
    }

    /// How many problems the document has: more than [`errors`](Self::errors) lists only
    /// when there are too many to list.
    pub fn total(&self) -> u64 {
        self.total
    }
}

/// Whether the document's features fit together and the kernel can make each, or every
/// problem with them. Within a feature, problems come in the order of its fields: its ID,
/// its values, its references.
pub fn validate(document: &Document) -> Result<(), Invalid> {
    // The names below are read under the scheme Silico names by (`Name::parse`), the only
    // one a document can record (`NamingScheme`: the reader refuses every other), so no
    // name is read under a scheme other than the one it was written under. A scheme added
    // beside it makes this pattern refutable, and the gate has to say how it reads each.
    let NamingScheme = document.naming;
    let features = &document.features;
    let _span = tracing::debug_span!("document_validate", features = features.len()).entered();

    // Every feature by ID, then by place: each ID's first feature heads its run.
    let mut by_id: Vec<(u64, usize)> = features
        .iter()
        .enumerate()
        .map(|(index, feature)| (key(&feature.id), index))
        .collect();
    by_id.sort_unstable();
    // The first feature with an ID, if any has it.
    let holder = |id: &FeatureId| {
        let key = key(id);
        let at = by_id.partition_point(|&(k, _)| k < key);
        by_id
            .get(at)
            .filter(|&&(k, _)| k == key)
            .map(|&(_, index)| index)
    };
    // Each feature's ID's first feature, and whether another feature has its ID too.
    let mut first = vec![0; features.len()];
    let mut shared = vec![false; features.len()];
    for run in by_id.chunk_by(|a, b| a.0 == b.0) {
        let head = run[0].1;
        for &(_, index) in run {
            first[index] = head;
            shared[index] = run.len() > 1;
        }
    }
    // A feature is named by its ID, or by its place where another feature has the ID too.
    let label = |index: usize| {
        if shared[index] {
            format!("features[{index}]")
        } else {
            features[index].id.to_string()
        }
    };
    let described = |index: usize| {
        format!(
            "{} ({})",
            label(index),
            shown(features[index].name.as_str())
        )
    };

    // Each sketch's elements by ID, sorted once (stably, so each ID's first element heads
    // its run), so an axis is found in its sketch by a search, not a scan: a scan per
    // revolve took revolves × elements (break-it).
    let elements: Vec<Vec<(u64, SketchElement)>> = features
        .iter()
        .map(|feature| match &feature.operation {
            Operation::Sketch(sketch) => {
                let mut elements: Vec<(u64, SketchElement)> = sketch
                    .elements()
                    .map(|(id, kind)| (bytes_key(id.as_str()), kind))
                    .collect();
                elements.sort_by_key(|&(key, _)| key);
                elements
            }
            _ => Vec::new(),
        })
        .collect();
    // The kind of sketch `used`'s first element with ID `id`, if it has one.
    let element_of = |used: usize, id: &str| {
        let list = elements.get(used)?;
        let key = bytes_key(id);
        let at = list.partition_point(|&(k, _)| k < key);
        list.get(at)
            .filter(|&&(k, _)| k == key)
            .map(|&(_, kind)| kind)
    };

    let mut problems = Problems::default();
    // The feature that consumed each feature's solid, if one has.
    let mut consumer: Vec<Option<usize>> = vec![None; features.len()];
    for (index, feature) in features.iter().enumerate() {
        let refuse = |parameter: &str, message: String, suggestion: String| {
            Error::new(ErrorCode::InvalidInput, message)
                .with_feature(label(index))
                .with_parameter(parameter)
                .with_suggestion(suggestion)
        };

        if first[index] != index {
            problems.add(|| {
                refuse(
                    "id",
                    format!(
                        "id {} is already the ID of {}",
                        feature.id,
                        described(first[index])
                    ),
                    "give this feature an ID of its own".to_owned(),
                )
            });
        }

        let what = feature.operation.type_name();
        // The kernel's rule for a move holds for each component alone.
        for (axis, written) in feature
            .operation
            .position()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if check_offset([written.mm(), 0.0, 0.0]).is_err() {
                problems.add(|| {
                    refuse(
                        &format!("position.{axis}"),
                        format!(
                            "position.{axis} is {written} mm, farther from the origin than the {} mm (a kilometre) a {what} may be placed",
                            format_number(MAX_LENGTH)
                        ),
                        format!("place the {what} within a kilometre of the origin"),
                    )
                });
            }
        }
        if let Some(primitive) = feature.operation.primitive() {
            for invalid in primitive.faults(|p| feature.operation.document_field(p)) {
                problems.add(|| {
                    refuse(
                        invalid.parameter,
                        invalid.message.clone(),
                        format!("give the {what} a valid {}", invalid.parameter),
                    )
                });
            }
        }

        if let Operation::Sketch(sketch) = &feature.operation {
            super::sketch_gate::check(sketch, &refuse, &mut |error| problems.add(|| error));
            if feature.fingerprint.is_some() {
                problems.add(|| {
                    refuse(
                        "fingerprint",
                        "a sketch has a fingerprint, but it makes no solid to measure".to_owned(),
                        "remove the sketch's fingerprint".to_owned(),
                    )
                });
            }
        }

        // A fillet's radius and a chamfer's size, by the kernel's rules.
        let size = match &feature.operation {
            Operation::Fillet { radius, .. } => Some(check_fillet_radius(radius.mm())),
            Operation::Chamfer { size, .. } => Some(
                ChamferSize::Equal {
                    distance: size.distance.mm(),
                }
                .check()
                .map_err(|mut invalid| {
                    invalid.parameter = "size.distance";
                    invalid
                }),
            ),
            _ => None,
        };
        if let Some(Err(invalid)) = size {
            problems.add(|| {
                refuse(
                    invalid.parameter,
                    invalid.message.clone(),
                    format!("give the {what} a valid {}", invalid.parameter),
                )
            });
        }
        // A sweep's extent: each length above 0 and at most a kilometre, and a two-sided
        // prism's whole length too; an angle above 0° and at most a whole turn. Exactly, in
        // the file's integers.
        if let Some(extent) = feature.operation.extent() {
            let mut lengths = Vec::new();
            for (field, value) in extent.values() {
                let parameter = format!("extent.{field}");
                match value {
                    ExtentValue::Length(length) if (1..=MAX_SWEEP_NM).contains(&length.nm()) => {
                        lengths.push(length.nm());
                    }
                    ExtentValue::Length(length) => problems.add(|| {
                        refuse(
                            &parameter,
                            format!(
                                "{what} {parameter} must be a length above 0 mm and at most {} mm (a kilometre), got {length}",
                                format_number(MAX_LENGTH)
                            ),
                            format!("give the {what} a valid {parameter}"),
                        )
                    }),
                    ExtentValue::Angle(angle) if (1..=MAX_TURN_NDEG).contains(&angle.ndeg()) => {}
                    ExtentValue::Angle(angle) => problems.add(|| {
                        refuse(
                            &parameter,
                            format!(
                                "{what} {parameter} must be an angle above 0° and at most 360°, got {angle}°"
                            ),
                            format!("give the {what} a valid {parameter}"),
                        )
                    }),
                    ExtentValue::Reversed(_) => {}
                }
            }
            // Two sides, each valid alone: the prism is as long as both.
            if let [length, length2] = lengths[..]
                && length + length2 > MAX_SWEEP_NM
            {
                problems.add(|| {
                    refuse(
                        "extent",
                        format!(
                            "{what} extent.length and extent.length2 together must be at most {} mm (a kilometre), the prism's length, got {}",
                            format_number(MAX_LENGTH),
                            Length::from_nm(length + length2)
                        ),
                        "shorten extent.length or extent.length2".to_owned(),
                    )
                });
            }
        }
        // The edges a fillet or chamfer names: at least one, each an edge's name, by
        // features before it, none twice.
        if matches!(
            feature.operation,
            Operation::Fillet { .. } | Operation::Chamfer { .. }
        ) && feature.operation.edges().is_empty()
        {
            problems.add(|| {
                refuse(
                    "edges",
                    format!("the {what} names no edges"),
                    format!("name the edges to {what}, as Silico names them"),
                )
            });
        }
        let makers = |id: &FeatureId| {
            holder(id)
                .filter(|&h| h < index)
                .and_then(|h| features.get(h))
                .map(|f| Maker::of(&f.operation))
        };
        // Each name's first place, to say which an edge named twice repeats.
        let mut first_place: std::collections::BTreeMap<&str, usize> =
            std::collections::BTreeMap::new();
        for (i, edge) in feature.operation.edges().iter().enumerate() {
            let parameter = format!("edges.{i}");
            let advice = "write the edge's name as Silico names it (feature.type.Edge(...)), of a feature before this one";
            match Name::parse(edge.as_str(), &makers) {
                Err(error) => problems.add(|| {
                    refuse(
                        &parameter,
                        format!(
                            "{parameter}, {}, isn't an edge's name: {}{}, at column {}",
                            shown(edge.as_str()),
                            error.why,
                            made_of_sketch(edge.as_str(), &feature.operation, features, &makers),
                            error.column
                        ),
                        advice.to_owned(),
                    )
                }),
                Ok(name) if name.kind() != ElementKind::Edge => problems.add(|| {
                    refuse(
                        &parameter,
                        format!(
                            "{parameter}, {}, names a {}, not an edge",
                            shown(edge.as_str()),
                            match name.kind() {
                                ElementKind::Face => "face",
                                ElementKind::Vertex => "vertex",
                                ElementKind::Solid => "solid",
                                ElementKind::Edge => "edge",
                            }
                        ),
                        advice.to_owned(),
                    )
                }),
                // An operation's element by its number, of a feature that's a primitive now:
                // the word isn't the feature's, and no number of the primitive's is the one
                // the operation's was, so none is suggested.
                Ok(Name::Element {
                    feature: named,
                    made: TypeWord::Operation(op),
                    element,
                }) if matches!(makers(&named), Some(Maker::Itself(_))) => problems.add(|| {
                    let kind = match makers(&named) {
                        Some(Maker::Itself(kind)) => kind.word(),
                        _ => "primitive",
                    };
                    refuse(
                        &parameter,
                        format!(
                            "{parameter}, {}, names {element} of a {}, but {named} is a {kind}: its elements are another's",
                            shown(edge.as_str()),
                            op.word()
                        ),
                        format!(
                            "name the edge as {named}'s solid names it now (a {kind}'s elements are `{named}.{kind}.` and their numbers)"
                        ),
                    )
                }),
                // Names a rebuild never takes (N3): a duplicate's, and an operation's element
                // by its number, which says nothing of how it came about.
                Ok(name) if unstable(&name) => problems.add(|| {
                    refuse(
                        &parameter,
                        format!(
                            "{parameter}, {}, names an edge by a name that doesn't say which: a duplicate (~n) or an operation's element by its number",
                            shown(edge.as_str())
                        ),
                        "name the edge by how it came about, as Silico names it now".to_owned(),
                    )
                }),
                Ok(_) => {}
            }
            if let Some(&j) = first_place.get(edge.as_str()) {
                problems.add(|| {
                    refuse(
                        &parameter,
                        format!("{parameter} names the edge edges.{j} names"),
                        "name each edge once".to_owned(),
                    )
                });
            } else {
                first_place.insert(edge.as_str(), i);
            }
        }

        for (field, target) in feature.operation.inputs() {
            let Some(used) = holder(target) else {
                problems.add(|| {
                    refuse(
                        field,
                        format!("{field} {target} isn't the ID of any feature"),
                        "use the ID of a feature before this one".to_owned(),
                    )
                });
                continue;
            };
            if !features[used].operation.makes_solid() {
                problems.add(|| {
                    refuse(
                        field,
                        format!(
                            "{field} {target} is {}, a {}, which makes no solid for a {what} to use",
                            described(used),
                            features[used].operation.type_name()
                        ),
                        "use a feature that makes a solid".to_owned(),
                    )
                });
            } else if used == index {
                problems.add(|| {
                    refuse(
                        field,
                        format!("{field} {target} is this feature: a feature can't use itself"),
                        "use the ID of a feature before this one".to_owned(),
                    )
                });
            } else if used > index {
                problems.add(|| {
                    refuse(
                        field,
                        format!(
                            "{field} {target} is {}, which comes after this feature: a feature may use only features before it",
                            described(used)
                        ),
                        "move this feature after the one it uses, or use one before it".to_owned(),
                    )
                });
            } else {
                match consumer[used] {
                    Some(earlier) if earlier == index => problems.add(|| {
                        refuse(
                            field,
                            format!(
                                "{field} {target} is base too: a {what} takes two different solids"
                            ),
                            format!("give the {what} a different {field}"),
                        )
                    }),
                    Some(earlier) => problems.add(|| {
                        refuse(
                            field,
                            format!(
                                "{field} {target}'s solid was already consumed by {}: a solid can be used once",
                                described(earlier)
                            ),
                            if first[earlier] != earlier {
                                // Its ID names an earlier feature, so it can't be used by ID.
                                format!(
                                    "give {} an ID of its own and use its result, or use another feature",
                                    label(earlier)
                                )
                            } else {
                                // Its ID names it, even where a later feature has it too.
                                format!(
                                    "use {}'s result, or another feature's",
                                    features[earlier].id
                                )
                            },
                        )
                    }),
                    None => consumer[used] = Some(index),
                }
            }
        }

        // The sketch a sweep uses: a sketch before it. It isn't consumed, so any number of
        // features may use it.
        let Some(target) = feature.operation.sketch() else {
            continue;
        };
        let sketch_advice = || "use the ID of a sketch before this feature".to_owned();
        let used = holder(target);
        match used {
            None => problems.add(|| {
                refuse(
                    "sketch",
                    format!("sketch {target} isn't the ID of any feature"),
                    sketch_advice(),
                )
            }),
            Some(used) if used == index => problems.add(|| {
                refuse(
                    "sketch",
                    format!("sketch {target} is this feature: a feature can't use itself"),
                    sketch_advice(),
                )
            }),
            Some(used) if !matches!(features[used].operation, Operation::Sketch(_)) => problems
                .add(|| {
                    refuse(
                        "sketch",
                        format!(
                            "sketch {target} is {}, a {}, not a sketch",
                            described(used),
                            features[used].operation.type_name()
                        ),
                        sketch_advice(),
                    )
                }),
            Some(used) if used > index => problems.add(|| {
                refuse(
                    "sketch",
                    format!(
                        "sketch {target} is {}, which comes after this feature: a feature may use only features before it",
                        described(used)
                    ),
                    "move this feature after the sketch it uses, or use a sketch before it"
                        .to_owned(),
                )
            }),
            Some(_) => {}
        }
        // The line it turns about: one of that sketch's, construction or not. (An axis
        // naming an element of a sketch the feature can't use is still checked against it.)
        let swept =
            used.filter(|&used| used != index)
                .and_then(|used| match &features[used].operation {
                    Operation::Sketch(_) => Some(used),
                    _ => None,
                });
        if let (Some(Axis::Line(line)), Some(used)) = (feature.operation.axis(), swept) {
            let advice = || "name a line of the sketch, x_axis, or y_axis".to_owned();
            match element_of(used, line.as_str()) {
                Some(SketchElement::Line) => {}
                Some(kind) => problems.add(|| {
                    refuse(
                        "axis",
                        format!(
                            "axis {line} is a {} of sketch {}: a {what} turns about a line",
                            kind.noun(),
                            label(used)
                        ),
                        advice(),
                    )
                }),
                None => problems.add(|| {
                    refuse(
                        "axis",
                        format!(
                            "axis {line} isn't an element of sketch {}: a {what} turns about a line of its sketch, x_axis, or y_axis",
                            label(used)
                        ),
                        advice(),
                    )
                }),
            }
        }
    }
    problems.into_result()
}

/// Where `text` names a sketch's edge whole (`s.sketch.Edge.<id>`, which the parser
/// refuses: its elements are in no solid), and the fillet's or chamfer's base sweeps that
/// sketch, what to write instead. Where the sweep starts at the sketch's plane (one side,
/// or through all), the edge it made there from that curve, its start (design S4):
/// ` (p.pad.Edge(M s.sketch.Edge.<id>))`, the rim where the sketch lies in the base's face.
/// Where it starts elsewhere (symmetric, two sides), that edge is cut away wherever the
/// base is, so no name is offered that might name nothing: only the form the feature's own
/// edges take. Otherwise nothing.
fn made_of_sketch(
    text: &str,
    operation: &Operation,
    features: &[crate::document::Feature],
    makers: &dyn Fn(&FeatureId) -> Option<Maker>,
) -> String {
    let sketch = text.get(..ID_LENGTH).and_then(FeatureId::new);
    let Some(sketch) = sketch.filter(|s| makers(s) == Some(Maker::By(OpCode::Sketch))) else {
        return String::new();
    };
    let is_edge = text
        .strip_prefix(sketch.as_str())
        .and_then(|rest| rest.strip_prefix(".sketch.Edge."))
        .is_some_and(|id| crate::document::ElementId::new(id).is_some());
    let base = operation
        .inputs()
        .first()
        .and_then(|(_, base)| features.iter().find(|f| &f.id == *base));
    use super::model::{Extent, PadExtent, PocketExtent, TurnExtent};
    match base {
        Some(base) if is_edge && base.operation.sketch() == Some(&sketch) => {
            let word = Maker::of(&base.operation).type_word();
            let at_its_plane = matches!(
                base.operation.extent(),
                Some(
                    Extent::Pad(PadExtent::OneSide { .. })
                        | Extent::Pocket(
                            PocketExtent::OneSide { .. } | PocketExtent::ThroughAll { .. }
                        )
                        | Extent::Turn(TurnExtent::OneSide { .. })
                )
            );
            if at_its_plane {
                format!(" ({}.{word}.Edge(M {text}))", base.id)
            } else {
                format!(
                    " (an edge {} made, as it names it: {}.{word}.Edge(...); its extent, {}, doesn't start at the sketch's plane, so no edge of it is the curve's own)",
                    base.id,
                    base.id,
                    base.operation.extent().map_or("extent", |e| e.type_name())
                )
            }
        }
        _ => String::new(),
    }
}

/// Whether `name` is one no map holds stable: a duplicate's (`~n`), or names, at its top or
/// anywhere inside, an operation's element by its number (as the name writes it:
/// `m3tq8v0z.cut.Edge5`).
fn unstable(name: &Name) -> bool {
    let mut stack = vec![name];
    while let Some(name) = stack.pop() {
        match name {
            Name::Duplicate { .. } => return true,
            // A sketch's only region is named `Face1`, by design (S4): stable.
            Name::Element {
                made: TypeWord::Operation(op),
                ..
            } if *op != OpCode::Sketch => {
                return true;
            }
            _ => stack.extend(name.parts().into_iter().map(std::sync::Arc::as_ref)),
        }
    }
    false
}

// An ID's key is its characters' bytes as one number, so no two IDs share a key and keys
// sort as IDs do. Every character is ASCII, so an ID of up to 8 characters fits.
const _: () = assert!(ID_LENGTH <= 8);

fn key(id: &FeatureId) -> u64 {
    bytes_key(id.as_str())
}

/// An ID's characters' bytes as one number, a feature's or a sketch element's alike: the
/// reader takes only IDs of [`ID_LENGTH`] ASCII characters.
fn bytes_key(id: &str) -> u64 {
    id.bytes().fold(0, |key, byte| (key << 8) | u64::from(byte))
}

/// A document the validation gate passed: the only kind recompute takes, so nothing is
/// rebuilt from a document whose references haven't been checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked(Document);

impl Checked {
    /// `document`, if it passes the gate ([`validate`]); every problem with it if not.
    pub fn new(document: Document) -> Result<Self, Invalid> {
        Self::try_new(document).map_err(|(_, invalid)| invalid)
    }

    /// `document`, if it passes the gate; if not, the document back with every problem with
    /// it, for a client that keeps a document the gate refuses (a session holds one, so it
    /// can be mended).
    pub fn try_new(document: Document) -> Result<Self, (Document, Invalid)> {
        match validate(&document) {
            Ok(()) => Ok(Checked(document)),
            Err(invalid) => Err((document, invalid)),
        }
    }

    pub fn document(&self) -> &Document {
        &self.0
    }

    pub fn into_document(self) -> Document {
        self.0
    }
}

/// Problems as they're found: the first [`PROBLEMS_LISTED`] made into errors, the rest
/// only counted, so a document with a problem in every feature costs no more to refuse
/// than to read.
#[derive(Default)]
struct Problems {
    errors: Vec<Error>,
    total: u64,
}

impl Problems {
    fn add(&mut self, error: impl FnOnce() -> Error) {
        self.total += 1;
        if self.errors.len() < PROBLEMS_LISTED {
            self.errors.push(error());
        }
    }

    fn into_result(self) -> Result<(), Invalid> {
        if self.total == 0 {
            Ok(())
        } else {
            Err(Invalid {
                errors: self.errors,
                total: self.total,
            })
        }
    }
}
