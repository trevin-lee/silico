//! A rebuilt document's warnings (docs/design/m4.1-command-api.md, *Warnings*, decision 17):
//! what `check` finds that fails nothing, each in the error contract's shape with a code of
//! its own ([`WarningCode`]). A warning never refuses a command, blocks a save, or sets an
//! exit code.
//!
//! Three codes, each from the model alone, so a gate's refusal (no model) has none:
//! - `ends_not_joined` (M3.2's D2): two ends of a sketch's profile curves (lines and arcs
//!   that aren't construction geometry) that are separate points, closer than FreeCAD's
//!   joining distance, on the sketch as it solved. One warning a pair of points, naming the
//!   curves that end at them.
//! - `constraints_redundant` (M3.3): a solved sketch's redundant constraints, each agreeing
//!   with the rest (decision 5; one that disagrees is a problem, `over_constrained`, and the
//!   sketch doesn't solve). One warning a sketch, naming them all.
//! - `fingerprint_missing` (the maintainer, 2026-10-26): a feature that built, whose file
//!   stores no fingerprint, so a change in what it builds isn't caught as drift. Only where a
//!   save would store one: a feature that failed, was blocked, couldn't be measured, or
//!   measures past what a fingerprint holds has none to store, and a sketch never holds one.
//!
//! **Order:** by timeline (the document's features, in order), then by code, in
//! [`WarningCode`]'s order (`ends_not_joined`, `constraints_redundant`,
//! `fingerprint_missing`), then, for a sketch's `ends_not_joined`, by its pair of points, in
//! ID order (the lesser point's ID, then the greater's). The first
//! [`PROBLEMS_LISTED`](crate::document::PROBLEMS_LISTED) are listed, all counted, as problems
//! are. Nothing depends on the order the file lists a sketch's members in.

use std::collections::{BTreeMap, BTreeSet};

use silico_protocol::{LENGTH_TOLERANCE, format_number};

use crate::document::{ElementId, Feature, FeatureId, Operation, PROBLEMS_LISTED, Sketch};
use crate::recompute::{FeatureOutcome, Model};
use crate::sketch::SketchSolution;
use crate::{Warning, WarningCode};

/// FreeCAD's joining distance, in millimetres: two edge ends strictly closer than this are
/// joined into one vertex as FreeCAD builds a sketch's wires (`TopoShape::makeElementWires`,
/// whose `BRepBuilderAPI_MakeWire::Add` joins a vertex `l < tolerance` from one already in
/// the wire, each vertex's tolerance `Precision::Confusion()`, 1e-7 mm:
/// `BRepLib_MakeWire.cxx`). OCCT's length tolerance, [`LENGTH_TOLERANCE`].
pub const JOINING_DISTANCE: f64 = LENGTH_TOLERANCE;

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

    /// How many more can be listed.
    fn room(&self) -> usize {
        PROBLEMS_LISTED.saturating_sub(self.listed.len())
    }

    /// The warning `warning` makes counted, and listed if there's room: made only then.
    fn push_with(&mut self, warning: impl FnOnce() -> Warning) {
        self.total += 1;
        if self.room() > 0 {
            self.listed.push(warning());
        }
    }

    /// `unlisted` more counted, none of them listed.
    fn count(&mut self, unlisted: u64) {
        self.total += unlisted;
    }
}

/// `model`'s warnings, in order (the module's *Order*).
pub fn of(model: &Model) -> Warnings {
    let _span = tracing::info_span!("warnings").entered();
    let mut warnings = Warnings::none();
    // The model's features are its document's, in order.
    for (feature, (_, outcome)) in model.document().features.iter().zip(model.features()) {
        match (&feature.operation, outcome) {
            (Operation::Sketch(sketch), FeatureOutcome::Sketch(solution)) => {
                ends_not_joined(&feature.id, sketch, solution, &mut warnings);
                constraints_redundant(&feature.id, solution, &mut warnings);
            }
            (_, FeatureOutcome::Built { .. }) => {
                fingerprint_missing(model, feature, &mut warnings);
            }
            _ => {}
        }
    }
    tracing::debug!(total = warnings.total(), "warned");
    warnings
}

/// The distance between two points as FreeCAD measures it when it joins a sketch's wires
/// (`gp_Pnt::Distance` on the sketch's own coordinates, z 0): the square root of the sum of
/// the squared differences, each step rounded as a double, in that order.
pub fn joining_distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

/// Whether two ends `distance` apart would be joined by FreeCAD: strictly closer than
/// [`JOINING_DISTANCE`].
fn joined_by_freecad(distance: f64) -> bool {
    distance < JOINING_DISTANCE
}

/// An end of the sketch's profile curves: the point, where it solved, and the curves that end
/// there, in ID order.
struct End<'a> {
    at: [f64; 2],
    curves: Vec<&'a ElementId>,
}

/// Each pair of `sketch`'s curve ends that FreeCAD would join and that aren't one point, a
/// warning each (the module's `ends_not_joined`), by pair, the points in ID order.
fn ends_not_joined(
    id: &FeatureId,
    sketch: &Sketch,
    solution: &SketchSolution,
    warnings: &mut Warnings,
) {
    // Each point a profile curve ends at, by ID, with the curves ending there.
    let mut at: BTreeMap<&ElementId, Vec<&ElementId>> = BTreeMap::new();
    let lines = sketch
        .lines
        .iter()
        .filter(|l| !l.construction)
        .map(|l| (&l.id, [&l.from, &l.to]));
    let arcs = sketch
        .arcs
        .iter()
        .filter(|a| !a.construction)
        .map(|a| (&a.id, [&a.start, &a.end]));
    for (curve, ends) in lines.chain(arcs) {
        for point in ends {
            at.entry(point).or_default().push(curve);
        }
    }
    let ends: Vec<End<'_>> = at
        .into_iter()
        .filter_map(|(point, mut curves)| {
            curves.sort();
            curves.dedup();
            Some(End {
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
    // A sweep along x, each end met with the ends after it within twice the distance in x:
    // any pair FreeCAD would join is among them, whatever rounding does to the difference.
    let mut by_x: Vec<usize> = (0..ends.len()).collect();
    by_x.sort_by(|&a, &b| ends[a].at[0].total_cmp(&ends[b].at[0]).then(a.cmp(&b)));
    // The first pairs in ID order, as many as can be listed, and how many there are in all.
    let room = warnings.room();
    let mut first: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut found: u64 = 0;
    for (i, &a) in by_x.iter().enumerate() {
        for &b in &by_x[i + 1..] {
            if ends[b].at[0] - ends[a].at[0] > 2.0 * JOINING_DISTANCE {
                break;
            }
            if !joined_by_freecad(joining_distance(ends[a].at, ends[b].at)) {
                continue;
            }
            found += 1;
            let pair = (a.min(b), a.max(b));
            if first.len() < room {
                first.insert(pair);
            } else if first.last().is_some_and(|&last| pair < last) {
                first.pop_last();
                first.insert(pair);
            }
        }
    }
    let listed = first.len() as u64;
    for (a, b) in first {
        warnings.push_with(|| {
            let distance = joining_distance(ends[a].at, ends[b].at);
            let mut curves: Vec<&ElementId> = ends[a].curves.clone();
            curves.extend(ends[b].curves.iter().filter(|c| !ends[a].curves.contains(c)));
            Warning::new(
                WarningCode::EndsNotJoined,
                "two curve ends are at one place but aren't one point, so the profile is open there",
            )
            .with_feature(id.as_str())
            .with_elements(curves.iter().map(|c| c.as_str()))
            .with_cause(format!(
                "the ends are {} mm apart, within FreeCAD's joining distance of {} mm",
                format_number(distance),
                format_number(JOINING_DISTANCE)
            ))
            .with_suggestion("share the point: make one curve end at the other's point")
        });
    }
    warnings.count(found - listed);
}

/// A solved sketch's redundant constraints, which agree with the rest, as one warning naming
/// them all (the first hundred, all counted), in ID order.
fn constraints_redundant(id: &FeatureId, solution: &SketchSolution, warnings: &mut Warnings) {
    let redundant = &solution.redundant;
    let n = redundant.len();
    if n == 0 {
        return;
    }
    warnings.push_with(|| {
        // FreeCAD's "Sketch with redundant constraints", "Remove the following redundant
        // constraints:" (`SketchObject::execute`, `appendRedundantMsg`), in Silico's words.
        let message = if n == 1 {
            "a constraint of the sketch is redundant: it repeats what its other constraints \
             already fix, and agrees with them"
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
                "the solver solved the sketch without the equations they repeat, and each holds \
                 on the solution",
            )
            .with_suggestion(
                "remove the constraints named, or make a dimension among them a reference \
                 (`driving: false`), keeping the sketch as constrained as it is",
            )
    });
}

/// `feature`'s `fingerprint_missing` (the module's), if it built and a save would store a
/// fingerprint its file doesn't hold.
fn fingerprint_missing(model: &Model, feature: &Feature, warnings: &mut Warnings) {
    if feature.fingerprint.is_some() || model.fingerprint(&feature.id).is_none() {
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
        .with_suggestion("save the document (format or commit) to store it")
    });
}
