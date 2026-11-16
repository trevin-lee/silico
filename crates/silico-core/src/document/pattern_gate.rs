//! The gate's rules for a linear pattern, a polar pattern, and a mirror
//! (docs/design/m6.0-patterns.md, section 4.1): their counts, lengths, angles, originals,
//! and instances skipped, each refused from the text before any kernel call, in the
//! design's words. A pattern's references, its directions, axis, and plane, are stored
//! names, checked with every other (`validate`); its `base` by M2's base rules.

use silico_protocol::{MAX_LENGTH, format_number};

use super::model::{Feature, FeatureId, Operation};
use super::pattern::{
    Copies, LinearDirection, LinearDistribution, MOST_INSTANCES, PolarDistribution, Skipped,
};
use super::quantity::Angle;
use crate::Error;

/// A kilometre in nanometres: the farthest a pattern's direction may reach.
const KILOMETRE_NM: i64 = (MAX_LENGTH * 1e6) as i64;

/// A whole turn in nanodegrees.
const TURN_NDEG: i64 = 360 * 1_000_000_000;

/// OCCT's linear tolerance, 1e-7 mm, in tenths of a nanometre: a step of it or less leaves a
/// copy on the one before it (`check_copies`).
const STEP_TENTHS_NM: i64 = 1;

/// OCCT's angular tolerance, in radians: a turn within it of none leaves a copy on the one
/// before it (`check_copies`).
const ANGULAR_TOLERANCE: f64 = 1e-12;

/// What the gate needs of the document around a pattern: each feature by ID, and whether
/// another feature has a feature's ID too.
pub(super) struct Around<'a> {
    pub features: &'a [Feature],
    pub holder: &'a dyn Fn(&FeatureId) -> Option<usize>,
}

/// One pattern's checks: its type and ID for messages, how to build a refusal of one of
/// its fields (feature and parameter set), and where each goes.
struct Gate<'a> {
    what: &'a str,
    id: &'a FeatureId,
    refuse: &'a dyn Fn(&str, String, String) -> Error,
    found: &'a mut dyn FnMut(Error),
}

impl Gate<'_> {
    fn add(&mut self, parameter: &str, message: String, suggestion: String) {
        let error = (self.refuse)(parameter, message, suggestion);
        (self.found)(error);
    }
}

/// Every problem with feature `index`'s pattern fields, if it's a pattern or mirror, each
/// built by `refuse` (feature and parameter set) and handed to `found`.
pub(super) fn check(
    index: usize,
    around: &Around<'_>,
    refuse: &dyn Fn(&str, String, String) -> Error,
    found: &mut dyn FnMut(Error),
) {
    let Some(feature) = around.features.get(index) else {
        return;
    };
    let mut gate = Gate {
        what: feature.operation.type_name(),
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
                let (what, id) = (gate.what, gate.id);
                gate.add(
                    "directions",
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
            if (1..=2).contains(&directions.len()) && counts.iter().all(|&c| c >= 1) {
                gate.skipped(skip, &counts, true);
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
            if *count >= 1 {
                gate.skipped(skip, &[*count], false);
            }
        }
        Operation::Mirror { base, copies, .. } => {
            gate.originals(index, around, base, copies);
        }
        _ => {}
    }
}

impl Gate<'_> {
    /// A count below 1.
    fn too_few(&mut self, parameter: &str, count: u32) {
        let what = self.what;
        self.add(
            parameter,
            format!("{what} {parameter} must be at least 1, got {count}"),
            format!(
                "give the {what} a count of 1 or more: the count includes the original, so 1 places no copy"
            ),
        );
    }

    /// More than [`MOST_INSTANCES`] in all: named at the one count that passes it alone, or
    /// at `directions` (a linear pattern's) when only the counts' product does.
    fn instances(&mut self, counts: &[u32]) {
        let total: u64 = counts.iter().map(|&c| u64::from(c)).product();
        if counts.is_empty() || total <= MOST_INSTANCES {
            return;
        }
        let (what, id) = (self.what, self.id);
        let linear = what == "linear_pattern";
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
        let error = (self.refuse)(
            &parameter,
            format!(
                "{what} {id} has {total} instances in all{by}, more than the {MOST_INSTANCES} a pattern may have"
            ),
            format!("lower a count so the counts multiply to {MOST_INSTANCES} or fewer"),
        )
        .with_cause(
            "the cap is fixed in the format, counting the original and every instance skipped, so a document means the same on every machine",
        );
        (self.found)(error);
    }

    /// One of a linear pattern's directions: its count, its length, how far it reaches, and
    /// whether its step is one OCCT takes for none.
    fn linear_direction(&mut self, i: usize, direction: &LinearDirection) {
        let what = self.what;
        let count = direction.count;
        if count < 1 {
            self.too_few(&format!("directions.{i}.count"), count);
        }
        let parameter = format!("directions.{i}.distribution.length");
        let length = direction.distribution.length();
        if !(1..=KILOMETRE_NM).contains(&length.nm()) {
            self.add(
                &parameter,
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
        if count < 2 {
            return;
        }
        let steps = i64::from(count - 1);
        match direction.distribution {
            LinearDistribution::Spacing { length } => {
                // The last instance's reach, exactly, in nanometres.
                let reach = i128::from(length.nm()) * i128::from(steps);
                if reach > i128::from(KILOMETRE_NM) {
                    self.add(
                        &parameter,
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
                // A step of length / steps, at most 1e-7 mm: in tenths of a nanometre,
                // exactly.
                if i128::from(length.nm()) * 10 <= i128::from(steps) * i128::from(STEP_TENTHS_NM) {
                    let step = length.mm() / steps as f64;
                    self.add(
                        &parameter,
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
        let (what, id) = (self.what, self.id);
        let parameter = "distribution.angle";
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
            } else if ndeg > TURN_NDEG {
                (
                    String::new(),
                    format!(
                        "give the {what} an angle of at most 360°: 360 spreads its instances over a whole turn"
                    ),
                )
            } else {
                (String::new(), format!("give the {what} an angle above 0°"))
            };
            self.add(
                parameter,
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
        // The first copy's turn: a whole turn spread over every instance, a part of one over
        // the gaps between them, or the spacing.
        let (step, steps) = match distribution {
            PolarDistribution::Extent { .. } if ndeg == TURN_NDEG => {
                (ndeg as f64 / f64::from(count), i64::from(count))
            }
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
                let error = (self.refuse)(
                    parameter,
                    format!(
                        "{what} {id}: {count} instances {angle} degrees apart reach {} degrees, a whole turn or more: instance {landing}, at {} degrees, {how} the original",
                        degrees(reach),
                        degrees(at)
                    ),
                    format!(
                        "use at most {landing} instances at {angle} degrees, or give the total angle with extent"
                    ),
                )
                .with_elements([landing.to_string()])
                .with_elements_total(1);
                (self.found)(error);
            }
        }
    }

    /// A pattern's or mirror's originals: at least one, none twice, each a pad, pocket,
    /// revolve, or groove before this feature in its base's chain.
    fn originals(&mut self, index: usize, around: &Around<'_>, base: &FeatureId, copies: &Copies) {
        let Copies::Features {
            features: originals,
        } = copies
        else {
            return;
        };
        let (what, id) = (self.what, self.id);
        if originals.is_empty() {
            self.add(
                "copies.features",
                format!("{what} {id} copies no features"),
                "name the pads, pockets, revolves, or grooves to copy, or copy the whole base with a copies of type solid".to_owned(),
            );
            return;
        }
        let chain = chain_of(base, index, around);
        for (i, original) in originals.iter().enumerate() {
            let parameter = format!("copies.features.{i}");
            if let Some(j) = originals.iter().take(i).position(|o| o == original) {
                self.add(
                    &parameter,
                    format!("{parameter} {original} is copies.features.{j} again"),
                    "name each feature once".to_owned(),
                );
                continue;
            }
            let Some((at, copied)) = (around.holder)(original)
                .and_then(|at| around.features.get(at).map(|copied| (at, copied)))
            else {
                self.add(
                    &parameter,
                    format!("{parameter} {original} isn't the ID of any feature"),
                    "use the ID of a feature before this one".to_owned(),
                );
                continue;
            };
            let name = copied.name.as_str();
            if at == index {
                self.add(
                    &parameter,
                    format!("{parameter} {original} is this feature: a feature can't copy itself"),
                    "use the ID of a feature before this one".to_owned(),
                );
            } else if at > index {
                self.add(
                    &parameter,
                    format!(
                        "{parameter} {original} is {name}, which comes after this feature: a feature may use only features before it"
                    ),
                    "use the ID of a feature before this one".to_owned(),
                );
            } else if copied.operation.sketch().is_none() {
                let kind = copied.operation.type_name();
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
                self.add(
                    &parameter,
                    format!(
                        "{parameter} {original} is {name}, a {kind}, not a pad, pocket, revolve, or groove"
                    ),
                    suggestion.to_owned(),
                );
            } else if !chain.contains(&at) {
                self.add(
                    &parameter,
                    format!(
                        "{parameter} {original} is {name}, which isn't in base {base}'s chain, so {base}'s solid holds no instance of it"
                    ),
                    format!(
                        "copy a feature that {base} is built on, or make the base the body whose chain holds {original}"
                    ),
                );
            }
        }
    }

    /// A pattern's instances skipped: none the original, each in its one spelling, of no
    /// more offsets than the pattern has directions (`counts`), each in range, none twice.
    fn skipped(&mut self, skip: &[Skipped], counts: &[u32], linear: bool) {
        let (what, id) = (self.what, self.id);
        for (i, instance) in skip.iter().enumerate() {
            let parameter = format!("skip.{i}");
            let offsets = instance.offsets();
            if offsets.iter().all(|&o| o == 0) {
                self.add(
                    &parameter,
                    format!(
                        "{parameter}, {instance}, is the original, which a pattern always keeps"
                    ),
                    "take it out of skip".to_owned(),
                );
                continue;
            }
            if let Some(last) = offsets.iter().rposition(|&o| o > 0)
                && last + 1 < offsets.len()
            {
                let written = Skipped(offsets.iter().take(last + 1).copied().collect());
                let instance_text: Vec<String> =
                    written.offsets().iter().map(u32::to_string).collect();
                self.add(
                    &parameter,
                    format!(
                        "{parameter}, {instance}, isn't how instance {} is written: trailing zeros are left off",
                        instance_text.join(",")
                    ),
                    format!("write {written}"),
                );
                continue;
            }
            if offsets.len() > counts.len() {
                let directions = match counts.len() {
                    1 => "1 direction".to_owned(),
                    n => format!("{n} directions"),
                };
                self.add(
                    &parameter,
                    format!(
                        "{parameter}, {instance}, has {} offsets, and {what} {id} has {directions}",
                        offsets.len()
                    ),
                    "name an instance by its steps along each direction, one offset a direction"
                        .to_owned(),
                );
                continue;
            }
            let past = offsets
                .iter()
                .zip(counts)
                .enumerate()
                .find(|(_, (offset, count))| **offset >= **count);
            if let Some((d, (_, &count))) = past {
                let (field, raise) = if linear {
                    (format!("directions.{d}'s"), format!("directions.{d}.count"))
                } else {
                    ("count's".to_owned(), "count".to_owned())
                };
                self.add(
                    &parameter,
                    format!(
                        "{parameter}, {instance}, is past {field} {count} instances, offsets 0 to {}",
                        count.saturating_sub(1)
                    ),
                    format!("name an instance the pattern has, or raise {raise}"),
                );
                continue;
            }
            if let Some(j) = skip.iter().take(i).position(|earlier| earlier == instance) {
                self.add(
                    &parameter,
                    format!("{parameter}, {instance}, is skip.{j} again"),
                    "name each instance once".to_owned(),
                );
            }
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

/// The features `base` is built on by its `base` links alone, `base` among them, each before
/// feature `index` (an original in a tool's chain has its sense turned round by a cut or a
/// common: Fable, 2026-11-10).
fn chain_of(base: &FeatureId, index: usize, around: &Around<'_>) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut next = (around.holder)(base);
    while let Some(at) = next.filter(|&at| at < index && !chain.contains(&at)) {
        chain.push(at);
        next = around.features.get(at).and_then(|feature| {
            let inputs = feature.operation.inputs();
            let base = inputs.iter().find(|(field, _)| *field == "base")?;
            (around.holder)(base.1)
        });
    }
    chain
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
