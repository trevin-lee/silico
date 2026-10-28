//! A rebuilt document's warnings (docs/design/m4.1-command-api.md, *Warnings*, decision 17):
//! what `check` finds that fails nothing, each in the error contract's shape with a code of
//! its own ([`WarningCode`]). A warning never refuses a command, blocks a save, or sets an
//! exit code.
//!
//! Three codes, each from the model alone, so a gate's refusal (no model) has none:
//! - `ends_not_joined` (M3.2's D2): ends of a sketch's profile curves (lines and arcs that
//!   aren't construction geometry) that are separate points FreeCAD would join, on the sketch
//!   as it solved: points strictly closer than FreeCAD's joining distance, 1e-7 mm, grouped
//!   with each other and so on through each other, as FreeCAD's wire join chains them
//!   (`BRepLib_MakeWire` grows the tolerance of the vertex it joins an end to: ends at
//!   x = 0, 6e-8, and 1.2e-7 become one vertex, at 6e-8 with a tolerance of 1.6e-7, and an
//!   end at 3.2e-7 stays apart, in FreeCAD 1.1.3, run headless). One warning a group of
//!   points (a cluster), not one a pair, as FreeCAD's missing-coincidence analysis reports
//!   a group (k − 1 missing for a group of k: `SketchAnalysis`, c036128aa0, its tests
//!   6e4c61881b), though that analysis groups only the ends near the first of a run sorted
//!   by position, each axis apart, without chaining. Naming its points, then the curves
//!   ending at them; its cause the largest distance between two of its points; its
//!   suggestion to share one point, after, for each curve with both its ends in the
//!   cluster, which sharing one would collapse, the advice to lengthen it (a line, or an
//!   arc short of a half turn) or the sketch gate's for an arc whose ends are one place (an
//!   arc past a half turn), as the profile's refusal of the same ends says
//!   (`profile::collapsing_advice`). The grouping is the profile's ([`crate::profile`]'s
//!   open ends that should be one point), each with its own comparison.
//!
//!   *Scope.* Measured between the points the file holds, as M3.5's profile measures them
//!   and as a fix moves them: FreeCAD measures between the vertices of the edges it builds
//!   from them (a line's end at its start plus its direction times its length, an arc's at
//!   its angles on its circle), which sit some ulps off (an arc's, up to the solve's
//!   convergence), by arithmetic that differs between its platforms; so within that much of
//!   1e-7 the verdict can differ from FreeCAD's. FreeCAD's join also depends on the edges'
//!   order (which vertex the others join to, and how far its tolerance grows), which
//!   grouping through each other mostly matches. FreeCAD's Validate tool finds gaps up to
//!   1e-4 mm by default (`Precision::Confusion()` × 1000, each axis apart, inclusive);
//!   Silico warns only of the gaps FreeCAD's wires would join (1e-7, strictly), and a larger
//!   gap a feature sweeps is M3.5's profile refusal.
//! - `constraints_redundant` (M3.3): a solved sketch's redundant constraints, each agreeing
//!   with the rest (decision 5; one that disagrees is a problem, `over_constrained`, and the
//!   sketch doesn't solve). One warning a sketch for those all of whose equations repeat
//!   others' (FreeCAD's "redundant constraints", which it advises removing), and another for
//!   those only part of whose equations do (FreeCAD's "partially redundant", which it doesn't:
//!   removing one frees what only it fixes), each naming them all.
//! - `fingerprint_missing` (the maintainer, 2026-10-26): a feature that built, whose file
//!   stores no fingerprint, so a change in what it builds isn't caught as drift. Only where a
//!   save would store one: a feature that failed, was blocked, couldn't be measured, or
//!   measures past what a fingerprint holds has none to store, and a sketch never holds one.
//!   Nor one an open transaction's edits reached (decision 28): its commit writes its
//!   fingerprint, and `format` is refused there. Outside a transaction a feature carried
//!   from a save that couldn't write its record (it failed, or the kernel faulted) and has
//!   built since is warned of: nothing writes it until the next save, and `format` can.
//!   Judged by what the file stores: a model older than its session's document (the newest
//!   published, after a save that wrote a sketch's solve and so didn't publish) doesn't warn
//!   of a feature that document stores a fingerprint for now
//!   (docs/design/m4.1-warnings-addendum.md).
//!
//! **Order:** by timeline (the document's features, in order), then by code, in
//! [`WarningCode`]'s order (`ends_not_joined`, `constraints_redundant`,
//! `fingerprint_missing`), then, for a sketch's `ends_not_joined`, by cluster, by its least
//! point ID, and for its `constraints_redundant`, the redundant before the partially
//! redundant. The first [`PROBLEMS_LISTED`](crate::document::PROBLEMS_LISTED) are listed,
//! all counted, as problems are. Nothing depends on the order the file lists a sketch's
//! members in.

use std::collections::{BTreeMap, BTreeSet};

use silico_protocol::{LENGTH_TOLERANCE, format_number};

use crate::document::{
    ElementId, Feature, FeatureId, Fingerprint, Operation, PROBLEMS_LISTED, Sketch,
};
use crate::profile::{
    Collapsing, CurveKind, collapsing_advice, one_place_groups, sweeps_past_half_turn,
};
use crate::recompute::{Built, FeatureOutcome, Model};
use crate::sketch::SketchSolution;
use crate::{Warning, WarningCode};

/// FreeCAD's joining distance, in millimetres: two edge ends strictly closer than this are
/// joined into one vertex as FreeCAD builds a sketch's wires (`TopoShape::makeElementWires`,
/// whose `BRepBuilderAPI_MakeWire::Add` joins a vertex `l < tolerance` from one already in
/// the wire, each vertex's tolerance `Precision::Confusion()`, 1e-7 mm:
/// `BRepLib_MakeWire.cxx`). OCCT's length tolerance, [`LENGTH_TOLERANCE`].
const FREECAD_WIRE_JOIN: f64 = LENGTH_TOLERANCE;

/// A document's warnings: the first [`PROBLEMS_LISTED`] in order (the module's *Order*),
/// and how many there are, listed or not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Warnings {
    listed: Vec<Warning>,
    total: u64,
}

impl Warnings {
    /// None.
    pub fn none() -> Self {
        Self::default()
    }

    /// `warnings`, each listed up to the first [`PROBLEMS_LISTED`], all counted: for a
    /// client or a test with warnings of its own to write.
    pub fn of(warnings: impl IntoIterator<Item = Warning>) -> Self {
        let mut all = Self::none();
        for warning in warnings {
            all.push_with(|| warning);
        }
        all
    }

    /// The warnings listed: every one, or the first [`PROBLEMS_LISTED`].
    pub fn listed(&self) -> &[Warning] {
        &self.listed
    }

    /// How many there are, listed or not.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The warning `warning` makes counted, and listed if there's room: made only then.
    fn push_with(&mut self, warning: impl FnOnce() -> Warning) {
        self.total += 1;
        if self.listed.len() < PROBLEMS_LISTED {
            self.listed.push(warning());
        }
    }
}

/// `model`'s warnings, in order (the module's *Order*): none of a missing fingerprint for a
/// feature in `spared`: the features an open transaction's edits reached, whose commit
/// writes their fingerprints (decision 28), and, for a model older than its session's
/// document, those that document stores a fingerprint for now.
pub fn of(model: &Model, spared: &BTreeSet<FeatureId>) -> Warnings {
    let _span = tracing::info_span!("warnings").entered();
    let mut warnings = Warnings::none();
    // The model's features are its document's, in order.
    for (feature, (_, outcome)) in model.document().features.iter().zip(model.features()) {
        match (&feature.operation, outcome) {
            (Operation::Sketch(sketch), FeatureOutcome::Sketch(solution)) => {
                ends_not_joined(&feature.id, sketch, solution, &mut warnings);
                constraints_redundant(&feature.id, solution, &mut warnings);
            }
            (_, FeatureOutcome::Built { built, .. }) if !spared.contains(&feature.id) => {
                fingerprint_missing(feature, built, &mut warnings);
            }
            _ => {}
        }
    }
    tracing::debug!(total = warnings.total(), "warned");
    warnings
}

/// The distance between two points as FreeCAD measures it when it joins a sketch's wires
/// (`gp_Pnt::Distance` on the sketch's own coordinates, z 0): the square root of the sum of
/// the squared differences, each step rounded as a double, in that order. Not
/// `silico_protocol::distance`, the profile's and the kernel's (`libm`'s `hypot`), which
/// can round otherwise in the last bit, and so join otherwise at 1e-7.
fn freecad_distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

/// Whether FreeCAD joins two ends `distance` apart (by [`freecad_distance`]): strictly
/// closer than [`FREECAD_WIRE_JOIN`].
fn freecad_joins(distance: f64) -> bool {
    distance < FREECAD_WIRE_JOIN
}

/// An end of the sketch's profile curves: the point, where it solved, and the curves that end
/// there, in ID order.
struct End<'a> {
    point: &'a ElementId,
    at: [f64; 2],
    curves: Vec<&'a ElementId>,
}

/// A profile curve of the sketch: its kind, the points it ends at, and, for an arc, whether
/// it sweeps past a half turn as it solved ([`sweeps_past_half_turn`]).
struct Curve<'a> {
    kind: CurveKind,
    ends: [&'a ElementId; 2],
    past_half_turn: bool,
}

/// Each cluster of `sketch`'s curve ends FreeCAD would join that aren't one point, a warning
/// each (the module's `ends_not_joined`), by its least point ID.
fn ends_not_joined(
    id: &FeatureId,
    sketch: &Sketch,
    solution: &SketchSolution,
    warnings: &mut Warnings,
) {
    // The profile curves, by ID, and each point one ends at, by ID, with the curves ending
    // there.
    let lines = sketch.lines.iter().filter(|l| !l.construction).map(|l| {
        let ends = [&l.from, &l.to];
        let kind = CurveKind::Line;
        let past_half_turn = false;
        (
            &l.id,
            Curve {
                kind,
                ends,
                past_half_turn,
            },
        )
    });
    let arcs = sketch.arcs.iter().filter(|a| !a.construction).map(|a| {
        let ends = [&a.start, &a.end];
        let kind = CurveKind::Arc;
        let solved = [&a.center, &a.start, &a.end].map(|point| solution.point(point));
        let past_half_turn = match solved {
            [Some(c), Some(s), Some(e)] => sweeps_past_half_turn(c, s, e),
            // A point the solve doesn't have ends at no place, so in no cluster.
            _ => false,
        };
        (
            &a.id,
            Curve {
                kind,
                ends,
                past_half_turn,
            },
        )
    });
    let curves: BTreeMap<&ElementId, Curve<'_>> = lines.chain(arcs).collect();
    let mut at: BTreeMap<&ElementId, Vec<&ElementId>> = BTreeMap::new();
    for (&id, curve) in &curves {
        for point in curve.ends {
            at.entry(point).or_default().push(id);
        }
    }
    // In the points' ID order, so each cluster is too, and the clusters by their least.
    let ends: Vec<End<'_>> = at
        .into_iter()
        .filter_map(|(point, mut curves)| {
            curves.sort();
            curves.dedup();
            Some(End {
                point,
                at: solution.point(point)?,
                curves,
            })
        })
        .collect();
    if ends.len() < 2 {
        return;
    }
    let _span =
        tracing::debug_span!("ends_not_joined", sketch = id.as_str(), ends = ends.len()).entered();
    let places: Vec<[f64; 2]> = ends.iter().map(|end| end.at).collect();
    let all: Vec<usize> = (0..ends.len()).collect();
    let clusters = one_place_groups(&all, &places, |p, q| freecad_joins(freecad_distance(p, q)));
    for cluster in clusters.iter().filter(|cluster| cluster.len() >= 2) {
        warnings.push_with(|| cluster_warning(id, &ends, &curves, cluster));
    }
}

/// The warning of `cluster`, indices into `ends` in ID order: its points, then the curves
/// ending at them in ID order, each once (the first [`PROBLEMS_LISTED`] of them, all
/// counted); its cause the largest distance between two of its points. A curve (of
/// `profile`, the sketch's) with both its ends in the cluster would collapse if they were one
/// point, so its suggestion is first the advice for it, as the profile's refusal of the same
/// ends says ([`collapsing_advice`]).
fn cluster_warning(
    id: &FeatureId,
    ends: &[End<'_>],
    profile: &BTreeMap<&ElementId, Curve<'_>>,
    cluster: &[usize],
) -> Warning {
    let points: Vec<&ElementId> = cluster.iter().map(|&i| ends[i].point).collect();
    let mut curves: Vec<&ElementId> = cluster
        .iter()
        .flat_map(|&i| ends[i].curves.iter().copied())
        .collect();
    curves.sort();
    curves.dedup();
    let both_ends: Vec<Collapsing<&str>> = curves
        .iter()
        .filter_map(|&c| {
            let curve = profile.get(c)?;
            let both = curve.ends.iter().all(|end| points.contains(end));
            both.then_some(Collapsing {
                id: c.as_str(),
                kind: curve.kind,
                past_half_turn: curve.past_half_turn,
            })
        })
        .collect();
    let mut farthest = 0.0_f64;
    for (k, &i) in cluster.iter().enumerate() {
        for &j in &cluster[k + 1..] {
            farthest = farthest.max(freecad_distance(ends[i].at, ends[j].at));
        }
    }
    let n = points.len();
    let cause = if n == 2 {
        format!(
            "the points are {} mm apart, closer than FreeCAD's joining distance of {} mm",
            format_number(farthest),
            format_number(FREECAD_WIRE_JOIN)
        )
    } else {
        format!(
            "the points are at most {} mm apart, each closer than FreeCAD's joining distance \
             of {} mm to another of them",
            format_number(farthest),
            format_number(FREECAD_WIRE_JOIN)
        )
    };
    let named = points.iter().chain(&curves);
    Warning::new(
        WarningCode::EndsNotJoined,
        format!(
            "{n} points that end curves are at one place but aren't one point, so the \
             profile is open there"
        ),
    )
    .with_feature(id.as_str())
    .with_elements(named.clone().take(PROBLEMS_LISTED).map(|e| e.as_str()))
    .with_elements_total((n + curves.len()) as u64)
    .with_cause(cause)
    .with_suggestion(if both_ends.is_empty() {
        SHARE_ONE_POINT.to_owned()
    } else {
        collapsing_advice(&both_ends, SHARE_ONE_POINT)
    })
}

/// `ends_not_joined`'s suggestion: alone, or, after the advice for each curve with both its
/// ends in the cluster, after "then " ([`collapsing_advice`]).
const SHARE_ONE_POINT: &str =
    "share one point: make the curves named end at one of the points named";

/// A solved sketch's redundant constraints, which agree with the rest: one warning naming
/// those all of whose equations repeat others', then one naming those only part of whose
/// equations do (the first hundred of each, all counted), in ID order.
fn constraints_redundant(id: &FeatureId, solution: &SketchSolution, warnings: &mut Warnings) {
    let redundant = &solution.redundant;
    let n = redundant.len();
    if n > 0 {
        warnings.push_with(|| {
            // FreeCAD's "Sketch with redundant constraints", "Remove the following redundant
            // constraints:" (`SketchObject::execute`, `appendRedundantMsg`), in Silico's words.
            let message = if n == 1 {
                "a constraint of the sketch is redundant: it repeats what its other \
                 constraints already fix, and agrees with them"
                    .to_owned()
            } else {
                format!(
                    "{n} constraints of the sketch are redundant: they repeat what its other \
                     constraints already fix, and agree with them"
                )
            };
            Warning::new(WarningCode::ConstraintsRedundant, message)
                .with_feature(id.as_str())
                .with_elements(redundant.keys().take(PROBLEMS_LISTED).map(|c| c.as_str()))
                .with_elements_total(n as u64)
                .with_cause(
                    "the solver solved the sketch without the equations they repeat, and each \
                     holds on the solution",
                )
                .with_suggestion(
                    "remove the constraints named, or make a dimension among them a reference \
                     (`driving: false`), keeping the sketch as constrained as it is",
                )
        });
    }
    let partly = &solution.partially_redundant;
    let n = partly.len();
    if n > 0 {
        warnings.push_with(|| {
            // FreeCAD's "Some constraints in combination are partially redundant"
            // (`TaskSketcherMessages`), which it shows apart from the redundant ones, with no
            // advice to remove them (65fb44df6c, b01313c0d8), in Silico's words.
            let message = if n == 1 {
                "a constraint of the sketch is partly redundant: only part of what it fixes \
                 is repeated by its other constraints, and that part agrees with them"
                    .to_owned()
            } else {
                format!(
                    "{n} constraints of the sketch are partly redundant: only part of what each \
                     fixes is repeated by its other constraints, and that part agrees with them"
                )
            };
            Warning::new(WarningCode::ConstraintsRedundant, message)
                .with_feature(id.as_str())
                .with_elements(partly.keys().take(PROBLEMS_LISTED).map(|c| c.as_str()))
                .with_elements_total(n as u64)
                .with_cause(
                    "the solver solved the sketch without the equations of theirs that repeat \
                     others, and each holds on the solution",
                )
                .with_suggestion(
                    "review the constraints named: each also fixes what no other constraint \
                     does, so removing one would loosen the sketch",
                )
        });
    }
}

/// `feature`'s `fingerprint_missing` (the module's), if it built as `built` and a save
/// would store a fingerprint its file doesn't hold.
fn fingerprint_missing(feature: &Feature, built: &Built, warnings: &mut Warnings) {
    if feature.fingerprint.is_some() || built.summary().and_then(Fingerprint::of).is_none() {
        return;
    }
    warnings.push_with(|| {
        Warning::new(
            WarningCode::FingerprintMissing,
            format!(
                "{} built, but its file stores no fingerprint, so a change in what it builds \
                 isn't caught as drift",
                feature.id
            ),
        )
        .with_feature(feature.id.as_str())
        .with_parameter("fingerprint")
        .with_suggestion("format the document to store it")
    });
}
