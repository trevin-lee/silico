//! The validation gate: whether a document's features fit together and the kernel can
//! make each (docs/design/m2.1-document-format.md, D2). Reading checks each value on its
//! own; the gate checks what features say about each other: every ID names one feature,
//! every reference is to a feature before the one making it (so the timeline has no
//! cycles), every solid is consumed at most once, and every primitive's values are ones
//! the kernel accepts, by the kernel's own rules.

use std::collections::BTreeMap;

use silico_protocol::{MAX_LENGTH, check_offset, format_number};

use super::json::shown;
use super::model::{Document, FeatureId, Operation};
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

    // Each ID's first feature, and how many features have it.
    let mut ids: BTreeMap<&FeatureId, (usize, usize)> = BTreeMap::new();
    for (index, feature) in features.iter().enumerate() {
        ids.entry(&feature.id).or_insert((index, 0)).1 += 1;
    }
    // A feature is named by its ID, or by its place where another feature has the ID too.
    let label = |index: usize| {
        let id = &features[index].id;
        match ids.get(id) {
            Some(&(_, 1)) => id.to_string(),
            _ => format!("features[{index}]"),
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

        if let Some(&(first, _)) = ids.get(&feature.id)
            && first != index
        {
            problems.add(|| {
                refuse(
                    "id",
                    format!(
                        "id {} is already the ID of {}",
                        feature.id,
                        described(first)
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
        if let Some(primitive) = feature.operation.primitive()
            && let Err(invalid) = primitive.check_named(Operation::field)
        {
            problems.add(|| {
                refuse(
                    invalid.parameter,
                    invalid.message.clone(),
                    format!("give the {what} a valid {}", invalid.parameter),
                )
            });
        }

        for (field, target) in feature.operation.inputs().into_iter().flatten() {
            let Some(&(used, _)) = ids.get(target) else {
                problems.add(|| {
                    refuse(
                        field,
                        format!("{field} {target} isn't the ID of any feature"),
                        "use the ID of a feature before this one".to_owned(),
                    )
                });
                continue;
            };
            if used == index {
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
                            format!(
                                "use {}'s result, or another feature's",
                                label(earlier)
                            ),
                        )
                    }),
                    None => consumer[used] = Some(index),
                }
            }
        }
    }
    problems.into_result()
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
