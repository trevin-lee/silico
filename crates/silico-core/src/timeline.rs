//! A document's features as the browser and the timeline draw them, structured, so no
//! client parses an outline's text (docs/design/m5.0-app.md, `timeline`, M5.1; decision 33,
//! A8, pending the maintainer). Below any command: the session's
//! ([`Sessions::timeline`](crate::command::Sessions::timeline)) adds the session's revision,
//! the model's, and the gate's count of problems, and names the model it reads.
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
//! - **suppressed**: suppressed (`"suppressed": true`, the M5.0 design's
//!   `suppress_feature`): not built, what consumes it building on what stands for it;
//! - **unknown**: a feature the model hasn't built (added or edited since its revision, or
//!   using one that was); one a kernel fault
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
    /// Every health, in Fusion's order: what `timeline`'s snapshot lists.
    pub const ALL: [Health; 5] = [
        Health::Healthy,
        Health::Warning,
        Health::Error,
        Health::Suppressed,
        Health::Unknown,
    ];

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
    /// The features it names: its `base` and `tool`, then the sketch it sweeps, then, for a
    /// sketch on a face, the face's own feature (`k7f2qx3m` of `k7f2qx3m.box.Face6`).
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
/// its bodies (below); `excepted` as the session's check
/// excepts (so a feature has a problem here exactly where that check has one on it). The
/// features come from `document`, the session's at its revision; the model may be of an
/// older one, and a feature it hasn't built is `unknown`: one it hasn't, one whose values
/// changed since its document, and every feature that uses one of those (an edit's reach,
/// [`net_reach`](crate::document::net_reach)). A fingerprint edited since isn't an edit the
/// model builds otherwise: until the next model, the drift is the old fingerprint's, as the
/// session's check of that model says too.
pub fn timeline(document: &Document, model: Option<&Model>, excepted: &Excepted) -> Timeline {
    let _span = tracing::info_span!("timeline").entered();
    let marks = model
        .map(|model| marks(model, excepted))
        .unwrap_or_default();
    // What the model hasn't built: each feature that changed since its document (added, or
    // its values other), and every feature that uses one, as an edit's reach is found.
    let unbuilt = model
        // The current model's document is the session's: nothing to compare.
        .filter(|model| !std::ptr::eq(model.document(), document))
        .map(|model| {
            crate::document::net_reach(
                crate::document::Before::Read(model.document()),
                &BTreeSet::new(),
                document,
            )
        })
        .unwrap_or_default();
    let features = document
        .features
        .iter()
        .map(|feature| {
            let mark = marks
                .get(&feature.id)
                .filter(|_| !unbuilt.contains(&feature.id))
                .copied()
                .unwrap_or(Mark {
                    health: Health::Unknown,
                    problems: 0,
                    warnings: 0,
                });
            Entry {
                id: feature.id.clone(),
                kind: feature.operation.type_name(),
                name: feature.name.as_str().to_owned(),
                suppressed: feature.suppressed,
                inputs: feature.operation.uses().collect(),
                health: mark.health,
                problems: mark.problems,
                warnings: mark.warnings,
            }
        })
        .collect();
    let bodies = model
        .map(|model| bodies_of(document, model, &unbuilt))
        .unwrap_or_default();
    Timeline { features, bodies }
}

/// The bodies `model` has as it drew them: of its own document, every feature it built a
/// solid for that no feature of that document consumes, in its order. What a client draws
/// of the model whatever the session's document is now: one the gate refused, whose
/// features it doesn't mark, among them.
pub fn bodies(model: &Model) -> Vec<FeatureId> {
    bodies_of(model.document(), model, &BTreeSet::new())
}

/// The bodies of `model` under `document`: the document's features, in its order, that the
/// model built a solid for and no feature of the document consumes (below). One edited
/// since (in `unbuilt`) stays (marked unknown), so a body doesn't vanish while its rebuild
/// runs; one a feature added since consumes, or whose consumer was removed since, is judged
/// by the document. A feature consumes what stands for each of its inputs, a suppressed
/// one's base passed through (`document::suppression`), and a suppressed feature consumes
/// nothing and is no body, whatever the model built.
fn bodies_of(document: &Document, model: &Model, unbuilt: &BTreeSet<FeatureId>) -> Vec<FeatureId> {
    // Consumed: by a feature the model built, or one it hasn't built yet (whose build will).
    // A solid whose consumer failed is a body still, as the model has it.
    let consumed = crate::document::suppression::consumed(&document.features, &|f| {
        unbuilt.contains(&f.id)
            || matches!(model.outcome(&f.id), Some(FeatureOutcome::Built { .. }))
    });
    document
        .features
        .iter()
        // A feature edited into a sketch since makes no solid, whatever the model built.
        .filter(|f| !matches!(f.operation, crate::document::Operation::Sketch(_)) && !f.suppressed)
        .map(|f| &f.id)
        .filter(|id| {
            !consumed.contains(id)
                && matches!(model.outcome(id), Some(FeatureOutcome::Built { .. }))
        })
        .cloned()
        .collect()
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
    // What each feature uses as it builds: what stands for each of its inputs, a suppressed
    // one's base passed through (`document::suppression`), and the sketch it sweeps. A
    // feature blocked through a suppressed one is blocked by what that one passes through.
    let features = &model.document().features;
    let links = crate::document::suppression::Links::of(features);
    let place = |id: &FeatureId| links.place(id);
    let standing = crate::document::suppression::standing(features, &place);
    let uses: BTreeMap<&FeatureId, Vec<&FeatureId>> = features
        .iter()
        .enumerate()
        .map(|(at, f)| {
            let inputs =
                crate::document::suppression::effective_inputs(features, &standing, &place, at)
                    .into_iter()
                    .filter_map(|(_, stands)| stands.and_then(|s| features.get(s)))
                    .map(|stand| &stand.id);
            (&f.id, inputs.chain(f.operation.sketch()).collect())
        })
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
            // A feature that didn't build has no measure to drift: its problems are its own.
            FeatureOutcome::Failed(_) => {
                roots.insert(id, Root::Document);
                Mark {
                    health: Health::Error,
                    problems: 1,
                    warnings: warned,
                }
            }
            // Counted as the check counts them: the listed errors but kernel faults (Silico's
            // own bug naming a base's elements, say), and those unlisted. All faults: as
            // unknown as a failure that's one.
            FeatureOutcome::Broken { errors, total } => {
                let faults = errors.iter().filter(|e| e.code().is_kernel_fault()).count();
                let own = total.saturating_sub(u64::try_from(faults).unwrap_or(u64::MAX));
                if own == 0 {
                    roots.insert(id, Root::Fault);
                    unknown
                } else {
                    roots.insert(id, Root::Document);
                    Mark {
                        health: Health::Error,
                        problems: own,
                        warnings: warned,
                    }
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
                        problems: 0,
                        warnings: warned,
                    },
                    Root::Fault => unknown,
                }
            }
            FeatureOutcome::Suppressed => Mark {
                health: Health::Suppressed,
                problems: 0,
                warnings: 0,
            },
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

#[cfg(test)]
mod tests {
    use super::Health;

    #[test]
    fn each_health_has_the_word_fusion_names_it_by() {
        // The command writes these (docs/design/m5.0-app.md, `timeline`).
        for (health, word) in [
            (Health::Healthy, "healthy"),
            (Health::Warning, "warning"),
            (Health::Error, "error"),
            (Health::Suppressed, "suppressed"),
            (Health::Unknown, "unknown"),
        ] {
            assert_eq!(health.word(), word);
        }
    }
}
