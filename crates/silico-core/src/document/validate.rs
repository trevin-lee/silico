//! The validation gate: whether a document's features fit together and the kernel can
//! make each (docs/design/m2.1-document-format.md, D2). Reading checks each value on its
//! own; the gate checks what features say about each other: every ID names one feature,
//! every reference is to a feature before the one making it (so the timeline has no
//! cycles), every solid is consumed at most once, and every primitive's values are ones
//! the kernel accepts, by the kernel's own rules.

use silico_protocol::{MAX_LENGTH, check_offset, format_number};

use super::json::shown;
use super::model::{Document, FeatureId, ID_LENGTH};
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

        for (field, target) in feature.operation.inputs().into_iter().flatten() {
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

// An ID's key is its characters' bytes as one number, so no two IDs share a key and keys
// sort as IDs do. Every character is ASCII, so an ID of up to 8 characters fits.
const _: () = assert!(ID_LENGTH <= 8);

fn key(id: &FeatureId) -> u64 {
    id.as_str()
        .bytes()
        .fold(0, |key, byte| (key << 8) | u64::from(byte))
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
