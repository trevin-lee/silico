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
//!   drift from its fingerprint (each a problem on it, as the session's check counts it); or
//!   blocked by a failure of the document's upstream, which is no problem of its own (the problem is at its root) but didn't compute, as
//!   Fusion's error is a feature that fails to compute;
//! - **suppressed**: never yet: suppression comes with its schema change (M5.1, the
//!   maintainer's);
//! - **unknown**: a feature the model hasn't (its revision is older); one a kernel fault
//!   stopped, building or measuring it, or blocked only by kernel faults upstream, which say
//!   nothing of the document (decision 32); every feature of a document with no model.

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

/// What a session's check excepts, so the timeline marks as it does: features whose
/// missing fingerprints aren't warned of ([`crate::warnings::of`]'s `spared`), and features
/// whose drift isn't a problem (an open transaction's edits' reach, whose commit writes
/// their fingerprints anew: decision 28, as the session's check excepts them).
#[derive(Debug, Clone, Default)]
pub struct Excepted {
    pub spared: BTreeSet<FeatureId>,
    pub drift: BTreeSet<FeatureId>,
}

/// `document`'s features, each with the health `model` gave it, if there's a model, and
/// the bodies it has that `document` has too; `excepted` as the session's check excepts
/// (so a feature has a problem here exactly where that check has one on it). The features
/// come from `document`, the session's at its revision; the model may be of an older one,
/// and a feature it hasn't is `unknown`. A feature is marked by its ID, as the model built
/// it: the command names the model to read.
pub fn timeline(document: &Document, model: Option<&Model>, excepted: &Excepted) -> Timeline {
    let _span = tracing::info_span!("timeline").entered();
    let marks = model
        .map(|model| marks(model, excepted))
        .unwrap_or_default();
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
    let present: BTreeSet<&FeatureId> = document.features.iter().map(|f| &f.id).collect();
    Timeline {
        features,
        bodies: model
            .map(|m| {
                m.bodies()
                    .iter()
                    .filter(|b| present.contains(b))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// What the model marks a feature with.
#[derive(Debug, Clone, Copy)]
struct Mark {
    health: Health,
    problems: u64,
    warnings: u64,
}

/// What a failure upstream of a blocked feature comes to: the document's own (a failure,
/// broken references), or only kernel faults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Root {
    Document,
    Fault,
}

/// Each of `model`'s features' marks, by ID.
fn marks(model: &Model, excepted: &Excepted) -> BTreeMap<FeatureId, Mark> {
    let drifted = model.drifted();
    // A feature's drift, as the session's check counts it: the document's (excepted as it
    // excepts), or a kernel fault measuring it, which leaves its fingerprint unchecked.
    let mut drift: BTreeSet<&FeatureId> = BTreeSet::new();
    let mut unmeasured: BTreeSet<&FeatureId> = BTreeSet::new();
    for (id, error) in model.drift_left(&drifted, &excepted.drift) {
        if error.code().is_kernel_fault() {
            unmeasured.insert(id);
        } else {
            drift.insert(id);
        }
    }
    let warnings: BTreeMap<FeatureId, u64> = crate::warnings::per_feature(model, &excepted.spared)
        .into_iter()
        .collect();
    let uses: BTreeMap<&FeatureId, Vec<&FeatureId>> = model
        .document()
        .features
        .iter()
        .map(|f| (&f.id, f.operation.uses().collect()))
        .collect();
    // What each failed or blocked feature comes to, in timeline order: a feature's inputs
    // come before it.
    let mut roots: BTreeMap<&FeatureId, Root> = BTreeMap::new();
    let mut marks = BTreeMap::new();
    for (id, outcome) in model.features() {
        let warned = warnings.get(id).copied().unwrap_or(0);
        let drifts = u64::from(drift.contains(id));
        let unknown = Mark {
            health: Health::Unknown,
            problems: 0,
            warnings: 0,
        };
        let mark = match outcome {
            FeatureOutcome::Failed(error) if error.code().is_kernel_fault() => {
                roots.insert(id, Root::Fault);
                unknown
            }
            FeatureOutcome::Failed(_) => {
                roots.insert(id, Root::Document);
                Mark {
                    health: Health::Error,
                    problems: 1 + drifts,
                    warnings: warned,
                }
            }
            FeatureOutcome::Broken { total, .. } => {
                roots.insert(id, Root::Document);
                Mark {
                    health: Health::Error,
                    problems: total + drifts,
                    warnings: warned,
                }
            }
            // Blocked: an error if any failure upstream is the document's, whichever of
            // its inputs names it first; unknown if every one is a kernel fault.
            FeatureOutcome::Blocked { .. } => {
                let inputs = uses.get(id).map(Vec::as_slice).unwrap_or_default();
                let root = if inputs.iter().any(|i| roots.get(i) == Some(&Root::Document)) {
                    Root::Document
                } else {
                    Root::Fault
                };
                roots.insert(id, root);
                match root {
                    Root::Document => Mark {
                        health: Health::Error,
                        problems: drifts,
                        warnings: warned,
                    },
                    Root::Fault => unknown,
                }
            }
            FeatureOutcome::Built { .. } | FeatureOutcome::Sketch(_) if unmeasured.contains(id) => {
                unknown
            }
            FeatureOutcome::Built { .. } | FeatureOutcome::Sketch(_) => Mark {
                health: if drifts > 0 {
                    Health::Error
                } else if warned > 0 {
                    Health::Warning
                } else {
                    Health::Healthy
                },
                problems: drifts,
                warnings: warned,
            },
        };
        marks.insert(id.clone(), mark);
    }
    marks
}
