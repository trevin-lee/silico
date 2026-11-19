//! The gate's rules for a linear pattern, a polar pattern, and a mirror
//! (docs/design/m6.0-patterns.md, section 4.1): their counts, lengths, angles, originals,
//! and instances skipped, each refused from the text before any kernel call, in the
//! design's words. A pattern's references, its directions, axis, and plane, are stored
//! names, checked with every other (`validate`); its `base` by M2's base rules.

use silico_protocol::{MAX_LENGTH, format_number};

use std::cmp::Ordering;
use std::collections::BTreeMap;

use super::model::{Feature, FeatureId, Made, Operation};
use super::pattern::{
    Copies, LinearDirection, LinearDistribution, MOST_INSTANCES, PatternFeature, PolarDistribution,
    Skipped,
};
use super::quantity::Angle;
use crate::Error;
use crate::naming::Instance;

/// A kilometre in nanometres: the farthest a pattern's direction may reach.
const KILOMETRE_NM: i64 = (MAX_LENGTH * 1e6) as i64;

/// A whole turn in nanodegrees.
const TURN_NDEG: i64 = 360 * 1_000_000_000;

/// OCCT's angular tolerance, in radians: a turn within it of none leaves a copy on the one
/// before it (`check_copies`).
const ANGULAR_TOLERANCE: f64 = 1e-12;

/// What the gate needs of the document around a pattern: each feature by ID, and each
/// feature's chain's root ([`roots`]).
pub(super) struct Around<'a> {
    pub features: &'a [Feature],
    pub holder: &'a dyn Fn(&FeatureId) -> Option<usize>,
    pub roots: &'a [usize],
}

/// Each feature's chain's root, by place: the first feature down its `base` links, each
/// link to an earlier feature (a link forward, or to nothing, ends it there, as the gate
/// refuses such a base on its own; only an earlier feature's root is worked yet, so the
/// look-up finds none). A feature is in another's chain when they share a root and it comes
/// no later: a solid is consumed once, so a chain is one line. Worked once a document, in
/// timeline order, so the chain test costs a look-up, not a walk.
pub(super) fn roots(
    features: &[Feature],
    holder: &dyn Fn(&FeatureId) -> Option<usize>,
) -> Vec<usize> {
    let mut roots: Vec<usize> = Vec::with_capacity(features.len());
    for (at, feature) in features.iter().enumerate() {
        let root = feature
            .operation
            .base()
            .and_then(holder)
            .and_then(|base| roots.get(base).copied())
            .unwrap_or(at);
        roots.push(root);
    }
    roots
}

/// A refusal made only when it's listed: the gate counts every problem but builds only the
/// first [`super::PROBLEMS_LISTED`], so a list of a million bad items costs a count.
pub(super) type Refusal<'a> = Box<dyn FnOnce() -> Error + 'a>;

/// One pattern's checks: which of the three it is and its ID, for messages, how to build a
/// refusal of one of its fields (feature and parameter set), and where each goes.
struct Gate<'a, 'f> {
    kind: PatternFeature,
    id: &'a FeatureId,
    refuse: &'a dyn Fn(&str, String, String) -> Error,
    found: &'f mut dyn FnMut(Refusal<'a>),
}

impl<'a> Gate<'a, '_> {
    /// The type the file writes.
    fn what(&self) -> &'static str {
        self.kind.word()
    }

    /// A refusal whose words are already made: for a field's one refusal. A refusal of each
    /// item of a list, which may be a million, goes through [`Gate::add_with`].
    fn add(&mut self, parameter: String, message: String, suggestion: String) {
        let refuse = self.refuse;
        (self.found)(Box::new(move || refuse(&parameter, message, suggestion)));
    }

    /// A refusal whose words are made only if it's listed.
    fn add_with(&mut self, make: impl FnOnce() -> (String, String, String) + 'a) {
        let refuse = self.refuse;
        (self.found)(Box::new(move || {
            let (parameter, message, suggestion) = make();
            refuse(&parameter, message, suggestion)
        }));
    }
}

/// Every problem with feature `index`'s pattern fields, if it's a pattern or mirror, each
/// built by `refuse` (feature and parameter set) and handed to `found`.
pub(super) fn check<'a>(
    index: usize,
    around: &Around<'a>,
    refuse: &'a dyn Fn(&str, String, String) -> Error,
    found: &mut dyn FnMut(Refusal<'a>),
) {
    let Some(feature) = around.features.get(index) else {
        return;
    };
    let Made::Pattern(kind) = feature.operation.made() else {
        return;
    };
    let mut gate = Gate {
        kind,
        id: &feature.id,
        refuse,
        found,
    };
    match &feature.operation {
        Operation::LinearPattern {
            base,
            copies,
            directions,
            skip,
        } => {
            gate.originals(index, around, base, copies);
            if directions.is_empty() || directions.len() > 2 {
                let (what, id) = (gate.what(), gate.id);
                gate.add(
                    "directions".to_owned(),
                    format!(
                        "{what} {id} has {} directions: a linear pattern steps along one direction or two",
                        directions.len()
                    ),
                    "give the linear_pattern one direction, or two".to_owned(),
                );
            }
            for (i, direction) in directions.iter().enumerate() {
                gate.linear_direction(i, direction);
            }
            let counts: Vec<u32> = directions.iter().map(|d| d.count).collect();
            gate.instances(&counts);
            if (1..=2).contains(&directions.len()) {
                gate.skipped(skip, counts.len());
            }
        }
        Operation::PolarPattern {
            base,
            copies,
            distribution,
            count,
            skip,
            ..
        } => {
            gate.originals(index, around, base, copies);
            if *count < 1 {
                gate.too_few("count", *count);
            }
            gate.instances(&[*count]);
            gate.polar_distribution(*distribution, *count);
            gate.skipped(skip, 1);
        }
        Operation::Mirror { base, copies, .. } => {
            gate.originals(index, around, base, copies);
        }
        _ => {}
    }
}

impl<'a> Gate<'a, '_> {
    /// A count below 1.
    fn too_few(&mut self, parameter: &str, count: u32) {
        let what = self.what();
        self.add(
            parameter.to_owned(),
            format!("{what} {parameter} must be at least 1, got {count}"),
            format!(
                "give the {what} a count of 1 or more: the count includes the original, so 1 places no copy"
            ),
        );
    }

    /// More than [`MOST_INSTANCES`] in all: named at the one count that passes it alone, or
    /// at `directions` (a linear pattern's) when only the counts' product does.
    fn instances(&mut self, counts: &[u32]) {
        // A product past u64 (three directions of four million) is past the cap too.
        let total = counts
            .iter()
            .try_fold(1u64, |total, &count| total.checked_mul(u64::from(count)));
        if counts.is_empty() || total.is_some_and(|total| total <= MOST_INSTANCES) {
            return;
        }
        let total = total.map_or_else(|| format!("more than {}", u64::MAX), |t| t.to_string());
        let (what, id) = (self.what(), self.id);
        let linear = self.kind == PatternFeature::Linear;
        let parameter = match counts.iter().position(|&c| u64::from(c) > MOST_INSTANCES) {
            Some(i) if linear => format!("directions.{i}.count"),
            Some(_) => "count".to_owned(),
            None => "directions".to_owned(),
        };
        let by = if counts.len() > 1 {
            let each: Vec<String> = counts.iter().map(u32::to_string).collect();
            format!(" ({})", each.join(" by "))
        } else {
            String::new()
        };
        let refuse = self.refuse;
        let message = format!(
            "{what} {id} has {total} instances in all{by}, more than the {MOST_INSTANCES} a pattern may have"
        );
        (self.found)(Box::new(move || {
            refuse(
                &parameter,
                message,
                format!("lower a count so the counts multiply to {MOST_INSTANCES} or fewer"),
            )
            .with_cause(
                "the cap is fixed in the format, counting the original and every instance skipped, so a document means the same on every machine",
            )
        }));
    }

    /// One of a linear pattern's directions: its count, its length, how far it reaches, and
    /// whether its step is one OCCT takes for none.
    fn linear_direction(&mut self, i: usize, direction: &LinearDirection) {
        let what = self.what();
        let count = direction.count;
        if count < 1 {
            self.too_few(&format!("directions.{i}.count"), count);
        }
        let parameter = format!("directions.{i}.distribution.length");
        let length = direction.distribution.length();
        if !(1..=KILOMETRE_NM).contains(&length.nm()) {
            self.add(
                parameter.clone(),
                format!(
                    "{what} {parameter} must be a length above 0 mm and at most {} mm (a kilometre), got {length}",
                    format_number(MAX_LENGTH)
                ),
                format!(
                    "give the {what} a valid {parameter}; to go the other way, set directions.{i}.reversed"
                ),
            );
            return;
        }
        // One instance takes no step, and reaches nothing.
        let Some(steps) = count.checked_sub(1).map(i64::from) else {
            return;
        };
        match direction.distribution {
            LinearDistribution::Spacing { length } => {
                // The last instance's reach, exactly, in nanometres.
                let reach = i128::from(length.nm()) * i128::from(steps);
                if reach > i128::from(KILOMETRE_NM) {
                    self.add(
                        parameter,
                        format!(
                            "{what} directions.{i} reaches {} mm from the original ({steps} steps of {length} mm), past the {} mm (a kilometre) a pattern may reach",
                            wide_mm(reach),
                            format_number(MAX_LENGTH)
                        ),
                        "lower the count or the spacing, so the last instance is within a kilometre of the original"
                            .to_owned(),
                    );
                }
            }
            LinearDistribution::Extent { length } => {
                // A step of length / steps, at most OCCT's linear tolerance, 1e-7 mm, a tenth
                // of a nanometre, where a copy lies on the one before it (`check_copies`):
                // length * 10 tenths of a nanometre over steps at most 1, exactly.
                if i128::from(length.nm()) * 10 <= i128::from(steps) {
                    let step = length.mm() / steps as f64;
                    self.add(
                        parameter,
                        format!(
                            "{what} directions.{i} puts its {count} instances {} mm apart ({length} mm in {steps} steps), within OCCT's 1e-7 mm, where a copy lies on the one before it",
                            format_number(step)
                        ),
                        "give a longer length, or fewer instances".to_owned(),
                    );
                }
            }
        }
    }

    /// A polar pattern's angle: above 0° and at most 360°, a turn the other way written with
    /// `reversed`; its step one OCCT takes for a turn; and, spaced, its instances short of a
    /// whole turn.
    fn polar_distribution(&mut self, distribution: PolarDistribution, count: u32) {
        let (what, id) = (self.what(), self.id);
        let parameter = "distribution.angle".to_owned();
        let angle = distribution.angle();
        let ndeg = angle.ndeg();
        if !(1..=TURN_NDEG).contains(&ndeg) {
            let (said, suggestion) = if ndeg < 0 {
                (
                    ": a turn the other way is written as its size, with reversed".to_owned(),
                    format!(
                        "write {}, and set reversed to true",
                        Angle::from_ndeg(ndeg.saturating_neg())
                    ),
                )
            } else if ndeg == 0 {
                (String::new(), format!("give the {what} an angle above 0°"))
            } else {
                (
                    String::new(),
                    format!(
                        "give the {what} an angle of at most 360°: 360 spreads its instances over a whole turn"
                    ),
                )
            };
            self.add(
                parameter.clone(),
                format!(
                    "{what} {parameter} must be an angle above 0° and at most 360°, got {angle}°{said}"
                ),
                suggestion,
            );
            return;
        }
        if count < 2 {
            return;
        }
        // The first copy's turn: the angle over the gaps between the instances, or the
        // spacing. A whole turn spreads over one gap more, the last instance short of the
        // original, but no count brings it near the tolerance even over count - 1 gaps
        // (360° in u32::MAX - 1 steps is 1.5e-9 radians), so only part of a turn is refused.
        let (step, steps) = match distribution {
            PolarDistribution::Extent { .. } => {
                (ndeg as f64 / f64::from(count - 1), i64::from(count - 1))
            }
            PolarDistribution::Spacing { .. } => (ndeg as f64, 1),
        };
        let radians = step * (std::f64::consts::PI / 180.0) / 1e9;
        if radians <= ANGULAR_TOLERANCE {
            self.add(
                parameter,
                format!(
                    "{what} puts its {count} instances {} degrees apart ({angle} degrees in {steps} steps), within OCCT's 1e-12 radians, where a copy lies on the one before it",
                    format_number(step / 1e9)
                ),
                "give a larger angle, or fewer instances".to_owned(),
            );
            return;
        }
        if let PolarDistribution::Spacing { .. } = distribution {
            // The first instance whose turn reaches a whole one, exactly, in nanodegrees.
            let landing = (TURN_NDEG + ndeg - 1) / ndeg;
            let reach = i128::from(ndeg) * i128::from(count - 1);
            if reach >= i128::from(TURN_NDEG) {
                let at = i128::from(ndeg) * i128::from(landing);
                let how = if at == i128::from(TURN_NDEG) {
                    "lands on"
                } else {
                    "passes"
                };
                let refuse = self.refuse;
                let message = format!(
                    "{what} {id}: {count} instances {angle} degrees apart reach {} degrees, a whole turn or more: instance {landing}, at {} degrees, {how} the original",
                    degrees(reach),
                    degrees(at)
                );
                (self.found)(Box::new(move || {
                    refuse(
                        &parameter,
                        message,
                        format!(
                            "use at most {landing} instances at {angle} degrees, or give the total angle with extent"
                        ),
                    )
                    .with_elements([landing.to_string()])
                    .with_elements_total(1)
                }));
            }
        }
    }

    /// A pattern's or mirror's originals: at least one, none twice, each a pad, pocket,
    /// revolve, or groove before this feature in its base's chain. Each repeat found by a
    /// look-up, so a long list costs its length, not its square.
    fn originals(
        &mut self,
        index: usize,
        around: &Around<'a>,
        base: &FeatureId,
        copies: &'a Copies,
    ) {
        let Copies::Features {
            features: originals,
        } = copies
        else {
            return;
        };
        let (what, id) = (self.what(), self.id);
        if originals.is_empty() {
            self.add(
                "copies.features".to_owned(),
                format!("{what} {id} copies no features"),
                "name the pads, pockets, revolves, or grooves to copy, or copy the whole base with a copies of type solid".to_owned(),
            );
            return;
        }
        let base_at = (around.holder)(base).filter(|&at| at < index);
        let mut first: BTreeMap<&FeatureId, usize> = BTreeMap::new();
        for (i, original) in originals.iter().enumerate() {
            if let Some(&j) = first.get(original) {
                self.add_with(move || {
                    let parameter = format!("copies.features.{i}");
                    (
                        parameter.clone(),
                        format!("{parameter} {original} is copies.features.{j} again"),
                        "name each feature once".to_owned(),
                    )
                });
                continue;
            }
            first.insert(original, i);
            let Some((at, copied)) = (around.holder)(original)
                .and_then(|at| around.features.get(at).map(|copied| (at, copied)))
            else {
                self.add_with(move || {
                    let parameter = format!("copies.features.{i}");
                    (
                        parameter.clone(),
                        format!("{parameter} {original} isn't the ID of any feature"),
                        "use the ID of a feature before this one".to_owned(),
                    )
                });
                continue;
            };
            // In base's chain: the same root, and no later than base.
            let in_chain = base_at.is_some_and(|base_at| {
                at <= base_at && around.roots.get(at) == around.roots.get(base_at)
            });
            let refusal: Option<(String, String)> = match at.cmp(&index) {
                Ordering::Equal => Some((
                    "is this feature: a feature can't copy itself".to_owned(),
                    "use the ID of a feature before this one".to_owned(),
                )),
                Ordering::Greater => Some((
                    format!(
                        "is {}, which comes after this feature: a feature may use only features before it",
                        copied.name.as_str()
                    ),
                    "use the ID of a feature before this one".to_owned(),
                )),
                Ordering::Less if copied.operation.sketch().is_none() => {
                    let suggestion = match &copied.operation {
                        Operation::Fillet { .. } => {
                            "copy the feature it rounds, then fillet the copies' edges by name; or copy the whole base with a copies of type solid"
                        }
                        Operation::Chamfer { .. } => {
                            "copy the feature it bevels, then chamfer the copies' edges by name; or copy the whole base with a copies of type solid"
                        }
                        _ => {
                            "copy the pads, pockets, revolves, or grooves it's built from; or copy the whole base with a copies of type solid"
                        }
                    };
                    Some((
                        format!(
                            "is {}, a {}, not a pad, pocket, revolve, or groove",
                            copied.name.as_str(),
                            copied.operation.type_name()
                        ),
                        suggestion.to_owned(),
                    ))
                }
                Ordering::Less if !in_chain => Some((
                    format!(
                        "is {}, which isn't in base {base}'s chain, so {base}'s solid holds no instance of it",
                        copied.name.as_str()
                    ),
                    format!(
                        "copy a feature that {base} is built on, or make the base the body whose chain holds {original}"
                    ),
                )),
                Ordering::Less => None,
            };
            if let Some((said, suggestion)) = refusal {
                let parameter = format!("copies.features.{i}");
                self.add(
                    parameter.clone(),
                    format!("{parameter} {original} {said}"),
                    suggestion,
                );
            }
        }
    }

    /// A pattern's instances skipped: none the original, each in its one spelling (N10's,
    /// [`Instance`]), of no more offsets than the pattern has directions, none twice. One
    /// past a count is taken, and kept: it's skipped again once the count reaches it, as
    /// FreeCAD main keeps "positions outside the current pattern" (`0ea40a10e2`; Main,
    /// 2026-11-17). Each repeat found by a look-up.
    fn skipped(&mut self, skip: &'a [Skipped], directions: usize) {
        let (what, id) = (self.what(), self.id);
        let mut first: BTreeMap<&Skipped, usize> = BTreeMap::new();
        for (i, instance) in skip.iter().enumerate() {
            let offsets = instance.offsets();
            let Some(spelled) = Instance::new(offsets) else {
                self.add_with(move || {
                    let parameter = format!("skip.{i}");
                    (
                        parameter.clone(),
                        format!(
                            "{parameter}, {instance}, is the original, which a pattern always keeps"
                        ),
                        "take it out of skip".to_owned(),
                    )
                });
                continue;
            };
            if spelled.offsets().len() < offsets.len() {
                self.add_with(move || {
                    let parameter = format!("skip.{i}");
                    let written = Skipped(spelled.offsets().to_vec());
                    (
                        parameter.clone(),
                        format!(
                            "{parameter}, {instance}, isn't how instance {spelled} is written: trailing zeros are left off"
                        ),
                        format!("write {written}"),
                    )
                });
                continue;
            }
            if offsets.len() > directions {
                self.add_with(move || {
                    let parameter = format!("skip.{i}");
                    let has = match directions {
                        1 => "1 direction".to_owned(),
                        n => format!("{n} directions"),
                    };
                    (
                        parameter.clone(),
                        format!(
                            "{parameter}, {instance}, has {} offsets, and {what} {id} has {has}",
                            offsets.len()
                        ),
                        "name an instance by its steps along each direction, one offset a direction"
                            .to_owned(),
                    )
                });
                continue;
            }
            if let Some(&j) = first.get(instance) {
                self.add_with(move || {
                    let parameter = format!("skip.{i}");
                    (
                        parameter.clone(),
                        format!("{parameter}, {instance}, is skip.{j} again"),
                        "name each instance once".to_owned(),
                    )
                });
                continue;
            }
            first.insert(instance, i);
        }
    }
}

/// `nm` nanometres as millimetres, as the file writes a length: exact, past what a `Length`
/// holds.
fn wide_mm(nm: i128) -> String {
    super::quantity::write_decimal(nm, super::quantity::LENGTH_DECIMALS)
}

/// `ndeg` nanodegrees as degrees, as the file writes an angle: exact.
fn degrees(ndeg: i128) -> String {
    super::quantity::write_decimal(ndeg, super::quantity::ANGLE_DECIMALS)
}

#[cfg(test)]
mod tests {
    use super::super::quantity::Length;
    use super::*;

    #[test]
    fn a_kilometre_and_a_turn_are_the_formats() {
        assert_eq!(KILOMETRE_NM, 1_000_000_000_000);
        assert_eq!(TURN_NDEG, 360_000_000_000);
        assert_eq!(Length::from_nm(KILOMETRE_NM).to_string(), "1000000");
        assert_eq!(wide_mm(1_000_998_000_000), "1000998");
        assert_eq!(degrees(450_000_000_000), "450");
    }
}
