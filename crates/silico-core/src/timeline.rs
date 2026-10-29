//! A document's features as the browser and the timeline draw them, structured, so no
//! client parses an outline's text (docs/design/m5.0-app.md, `timeline`, M5.1; decision 33,
//! A8, pending the maintainer). Below any command: the command adds the session's revision
//! and the gate's count of problems, and names the model it reads.
//!
//! Each feature's health is one of Fusion's health states (docs/ux/fusion.md, *The
//! timeline*), as the model gave it:
//!
//! - **healthy**: built, or solved, with no problem or warning;
//! - **warning**: built, or solved, with warnings (a missing fingerprint, ends not joined,
//!   redundant constraints): Fusion's yellow, a feature that still shows;
//! - **error**: not built, or built but failing a check: a failure, broken references, or
//!   drift from its fingerprint (each a problem on it); or blocked by a failure upstream,
//!   which is no problem of its own (the problem is at its root) but didn't compute, as
//!   Fusion's error is a feature that fails to compute;
//! - **suppressed**: never yet: suppression comes with its schema change (M5.1, the
//!   maintainer's);
//! - **unknown**: a feature the model hasn't (its revision is older), or one a kernel fault
//!   stopped (or a failure it blocked on), which says nothing of the document (decision 32),
//!   or a document with no model.

use std::collections::{BTreeMap, BTreeSet};

use crate::document::{Document, FeatureId};
use crate::recompute::{FeatureOutcome, Model};

/// A feature's health, in Fusion's states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Healthy,
    Warning,
    Error,
    Suppressed,
    Unknown,
}

impl Health {
    /// The word the command writes.
    pub fn word(self) -> &'static str {
        match self {
            Health::Healthy => "healthy",
            Health::Warning => "warning",
            Health::Error => "error",
            Health::Suppressed => "suppressed",
            Health::Unknown => "unknown",
        }
    }
}

/// A feature as the timeline draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: FeatureId,
    /// Its `type`, as the file writes it.
    pub kind: &'static str,
    pub name: String,
    pub suppressed: bool,
    /// The features it names: its `base` and `tool`, then the sketch it sweeps.
    pub inputs: Vec<FeatureId>,
    pub health: Health,
    /// How many problems the model has on it, and warnings, whose text `check` gives.
    pub problems: u64,
    pub warnings: u64,
}

/// The document's features in timeline order, and the bodies the model has (none without a
/// model).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub features: Vec<Entry>,
    pub bodies: Vec<FeatureId>,
}

/// `document`'s features, each with the health `model` gave it, if there's a model, and
/// the bodies it has. `spared` are the features an open transaction's edits reached, whose
/// missing fingerprints aren't warned of (as [`crate::warnings::of`]'s). The features come
/// from `document`, the session's at its revision; the model may be of an older one, and a
/// feature it hasn't is `unknown`.
pub fn timeline(
    document: &Document,
    model: Option<&Model>,
    spared: &BTreeSet<FeatureId>,
) -> Timeline {
    let _span = tracing::info_span!("timeline").entered();
    let marks = model.map(|model| marks(model, spared)).unwrap_or_default();
    let features = document
        .features
        .iter()
        .map(|feature| {
            let mark = marks.get(&feature.id).copied().unwrap_or(Mark {
                health: Health::Unknown,
                problems: 0,
                warnings: 0,
            });
            Entry {
                id: feature.id.clone(),
                kind: feature.operation.type_name(),
                name: feature.name.as_str().to_owned(),
                suppressed: false,
                inputs: feature.operation.uses().cloned().collect(),
                health: mark.health,
                problems: mark.problems,
                warnings: mark.warnings,
            }
        })
        .collect();
    Timeline {
        features,
        bodies: model.map(|m| m.bodies().to_vec()).unwrap_or_default(),
    }
}

/// What the model marks a feature with.
#[derive(Debug, Clone, Copy)]
struct Mark {
    health: Health,
    problems: u64,
    warnings: u64,
}

/// Each of `model`'s features' marks, by ID.
fn marks(model: &Model, spared: &BTreeSet<FeatureId>) -> BTreeMap<FeatureId, Mark> {
    let drifted: BTreeSet<FeatureId> = model
        .drifted()
        .into_iter()
        .filter(|(_, error)| !error.code().is_kernel_fault())
        .map(|(id, _)| id)
        .collect();
    let warnings: BTreeMap<FeatureId, u64> = crate::warnings::per_feature(model, spared)
        .into_iter()
        .collect();
    model
        .features()
        .iter()
        .map(|(id, outcome)| {
            let warned = warnings.get(id).copied().unwrap_or(0);
            let drift = u64::from(drifted.contains(id));
            let mark = match outcome {
                FeatureOutcome::Failed(error) if error.code().is_kernel_fault() => Mark {
                    health: Health::Unknown,
                    problems: 0,
                    warnings: 0,
                },
                FeatureOutcome::Failed(_) => Mark {
                    health: Health::Error,
                    problems: 1 + drift,
                    warnings: warned,
                },
                FeatureOutcome::Broken { total, .. } => Mark {
                    health: Health::Error,
                    problems: total + drift,
                    warnings: warned,
                },
                // Blocked by a kernel fault upstream: as unknown as the fault.
                FeatureOutcome::Blocked { on } if faulted(model, on) => Mark {
                    health: Health::Unknown,
                    problems: 0,
                    warnings: 0,
                },
                FeatureOutcome::Blocked { .. } => Mark {
                    health: Health::Error,
                    problems: drift,
                    warnings: warned,
                },
                FeatureOutcome::Built { .. } | FeatureOutcome::Sketch(_) => Mark {
                    health: if drift > 0 {
                        Health::Error
                    } else if warned > 0 {
                        Health::Warning
                    } else {
                        Health::Healthy
                    },
                    problems: drift,
                    warnings: warned,
                },
            };
            (id.clone(), mark)
        })
        .collect()
}

/// Whether `model` failed feature `id` with a kernel fault.
fn faulted(model: &Model, id: &FeatureId) -> bool {
    matches!(model.outcome(id), Some(FeatureOutcome::Failed(error)) if error.code().is_kernel_fault())
}
