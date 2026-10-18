//! The validation gate: whether a document's features fit together and the kernel can
//! make each (docs/design/m2.1-document-format.md, D2). Reading checks each value on its
//! own; the gate checks what features say about each other: every ID names one feature,
//! every reference is to a feature before the one making it (so the timeline has no
//! cycles), every solid is consumed at most once (and is a solid: a sketch makes none),
//! every primitive's, fillet's, and chamfer's values are ones the kernel accepts, by the
//! kernel's own rules, every sketch holds together (`sketch_gate`), and every element a
//! fillet or chamfer names is named as Silico names an edge (docs/ports/naming.md), by
//! features before it. Whether a name finds its edge is the rebuild's to say: it's in the
//! base's solid, which only a rebuild makes.

use silico_protocol::{ChamferSize, MAX_LENGTH, check_fillet_radius, check_offset, format_number};

use super::json::shown;
use super::model::{Document, FeatureId, ID_LENGTH, Operation};
use crate::kernel::ElementKind;
use crate::naming::{Maker, Name};
use crate::{Error, ErrorCode};

/// How many of a document's problems [`Invalid`] lists; it counts the rest.
pub const PROBLEMS_LISTED: usize = 100;

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
            let advice = "write the edge's name as Silico names it (feature.Edge(...)), of a feature before this one";
            match Name::parse(edge.as_str(), &makers) {
                Err(error) => problems.add(|| {
                    refuse(
                        &parameter,
                        format!(
                            "{parameter}, {}, isn't an edge's name: {error}",
                            shown(edge.as_str())
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
                // Names a rebuild never takes (N3): a duplicate's, and an operation's element
                // by its number, which says nothing of how it came about.
                Ok(name) if unstable(&name, &makers) => problems.add(|| {
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
    }
    problems.into_result()
}

/// Whether `name` is one no map holds stable: a duplicate's (`~n`), or names, at its top or
/// anywhere inside, an operation's element by its number.
fn unstable(name: &Name, makers: &dyn Fn(&FeatureId) -> Option<Maker>) -> bool {
    let mut stack = vec![name];
    while let Some(name) = stack.pop() {
        match name {
            Name::Duplicate { .. } => return true,
            Name::Element { feature, .. } if matches!(makers(feature), Some(Maker::By(_))) => {
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
    id.as_str()
        .bytes()
        .fold(0, |key, byte| (key << 8) | u64::from(byte))
}

/// A document the validation gate passed: the only kind recompute takes, so nothing is
/// rebuilt from a document whose references haven't been checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked(Document);

impl Checked {
    /// `document`, if it passes the gate ([`validate`]); every problem with it if not.
    pub fn new(document: Document) -> Result<Self, Invalid> {
        validate(&document)?;
        Ok(Checked(document))
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
