//! What a pad, pocket, revolve, or groove's rebuild refuses that the gate can't see
//! (docs/design/m3.6-sketch-features.md, S5 to S7), as pure functions of what the rebuild
//! already has, for recompute to call (M3.6's step iv): from the solved sketch, before the
//! kernel is asked for anything (decision 2), a revolve's or groove's axis of no length (S6),
//! and a pad or revolve of more than one region (decision 3); and from the built result, more
//! than one solid (D2), or a volume that shows the feature changed nothing (S7).

use silico_protocol::{MEASURE_PRECISION, format_number};

use crate::document::{Axis, Checked, FeatureId, Operation, PROBLEMS_LISTED};
use crate::profile::Profile;
use crate::sketch::SketchSolution;
use crate::{Error, ErrorCode};

/// Whether revolve or groove `feature`'s axis gives a direction to turn about, its sketch
/// solved as `solution`: `x_axis` and `y_axis` always do; a line does unless its solved ends
/// are at one point, exactly, which the gate can't see (a solve can bring two points
/// together). Refused as `invalid_input`, naming the feature and its `axis`. Ends any
/// distance apart pass, however small: whether the profile can be turned about them is the
/// kernel's to say.
pub fn check_axis_length(
    document: &Checked,
    feature: &FeatureId,
    solution: &SketchSolution,
) -> Result<(), Error> {
    let _span = tracing::debug_span!("check_axis_length", feature = feature.as_str()).entered();
    let features = &document.document().features;
    let found = features.iter().find(|f| &f.id == feature).ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("the document has no feature {feature}"),
        )
    })?;
    let (Some(axis), Some(sketch_id)) = (found.operation.axis(), found.operation.sketch()) else {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("feature {feature} isn't a revolve or groove: it has no axis"),
        )
        .with_feature(feature.as_str()));
    };
    let line_id = match axis {
        Axis::XAxis | Axis::YAxis => return Ok(()),
        Axis::Line(line) => line,
    };
    // The gate has checked that the sketch is one, and the axis a line of it.
    let line = features
        .iter()
        .find(|f| &f.id == sketch_id)
        .and_then(|f| match &f.operation {
            Operation::Sketch(sketch) => sketch.lines.iter().find(|l| &l.id == line_id),
            _ => None,
        })
        .ok_or_else(|| {
            unchecked(format!(
                "turns {feature} about {line_id}, which isn't a line of sketch {sketch_id}"
            ))
        })?;
    let end = |id| {
        solution.point(id).ok_or_else(|| {
            unchecked(format!(
                "solves sketch {sketch_id} with no point {id}, an end of {feature}'s axis"
            ))
        })
    };
    let (from, to) = (end(&line.from)?, end(&line.to)?);
    // Exactly: by value, so -0 and 0 are one place.
    if from == to {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "axis {line_id}, a line of sketch {sketch_id}, has its ends at one point: it gives no direction to turn about"
            ),
        )
        .with_feature(feature.as_str())
        .with_parameter("axis")
        .with_suggestion("give the line some length, or turn about another line"));
    }
    Ok(())
}

/// A document or solution the caller should have checked, and didn't: a bug in Silico.
fn unchecked(what: String) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("a revolve's axis was checked against a document or solution that {what}"),
    )
    .with_suggestion(crate::worker::SILICO_BUG)
}

/// A sketch feature's type: what it does with its sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepFeature {
    Pad,
    Pocket,
    Revolve,
    Groove,
}

impl SweepFeature {
    /// Every sketch feature.
    pub const ALL: [SweepFeature; 4] = [
        SweepFeature::Pad,
        SweepFeature::Pocket,
        SweepFeature::Revolve,
        SweepFeature::Groove,
    ];

    /// The document's word for it: `pad`, `pocket`, `revolve`, `groove`.
    pub fn word(self) -> &'static str {
        match self {
            SweepFeature::Pad => "pad",
            SweepFeature::Pocket => "pocket",
            SweepFeature::Revolve => "revolve",
            SweepFeature::Groove => "groove",
        }
    }

    /// Whether it fuses its sweep with its base (a pad, a revolve), rather than cutting it
    /// away (a pocket, a groove).
    pub fn adds(self) -> bool {
        matches!(self, SweepFeature::Pad | SweepFeature::Revolve)
    }
}

/// A solid as a sketch feature's checks see it: its volume, by the precise measure, and how
/// many solids, faces, edges, and vertices it has.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measured {
    pub volume: f64,
    pub solids: u32,
    pub faces: u32,
    pub edges: u32,
    pub vertices: u32,
}

/// Decision 3, strict (design S5): a pad or a revolve makes one solid of one region, so a
/// profile of several is refused before any kernel step, with or without a base that would
/// join them. `invalid_input`, the feature the sketch (its elements are the sketch's, and
/// that's where they're fixed), the elements every region's outer-loop curves in region
/// order (the first hundred, and their total). A pocket or a groove takes every region.
pub fn check_regions(
    feature: &FeatureId,
    kind: SweepFeature,
    sketch: &FeatureId,
    profile: &Profile,
) -> Result<(), Error> {
    if !kind.adds() || profile.regions.len() <= 1 {
        return Ok(());
    }
    let loops: Vec<Vec<String>> = profile
        .regions
        .iter()
        .map(|region| {
            region
                .outer
                .curves
                .iter()
                .map(|edge| edge.curve.to_string())
                .collect()
        })
        .collect();
    let total: usize = loops.iter().map(Vec::len).sum();
    let listed: Vec<String> = loops
        .iter()
        .flatten()
        .take(PROBLEMS_LISTED)
        .cloned()
        .collect();
    // The listed curves, region by region, as the message says them.
    let mut said: Vec<String> = Vec::new();
    let mut left = PROBLEMS_LISTED;
    for curves in &loops {
        if left == 0 {
            break;
        }
        let shown: Vec<&str> = curves.iter().take(left).map(String::as_str).collect();
        left -= shown.len();
        said.push(shown.join(", "));
    }
    let more = match total.saturating_sub(PROBLEMS_LISTED) {
        0 => String::new(),
        n => format!(", and {n} more curves"),
    };
    let message = format!(
        "sketch {} makes {} separate regions (inside loops {}{more}), and {} {} makes one solid of one region",
        sketch.as_str(),
        loops.len(),
        and_list(&said),
        kind.word(),
        feature.as_str()
    );
    Err(Error::new(ErrorCode::InvalidInput, message)
        .with_feature(sketch.as_str())
        .with_elements(listed)
        .with_elements_total(total as u64)
        .with_suggestion(format!(
            "{} each region from its own sketch, or join them into one region; a {} takes several",
            kind.word(),
            if kind == SweepFeature::Pad {
                "pocket"
            } else {
                "groove"
            }
        )))
}

/// `a and b`, `a; b; and c`: the loops, each its curves joined by commas.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}; and {last}", rest.join("; ")),
    }
}

/// What a sketch feature's result must be, once its boolean is built (design S5, S7): one
/// solid, and a change to its base's volume. Both `kernel_failed`, naming the feature: the
/// geometry is valid and so is the input, but the feature didn't do what it's for.
///
/// - **One solid** (D2): more than one is refused (a pad that misses its base, a pocket that
///   cuts it in two), and so is none, which the kernel's empty boolean refuses first.
/// - **A change**: a pad or revolve is refused when V(result) − V(base) is at most
///   [`MEASURE_PRECISION`] × V(base), and a pocket or groove when V(base) − V(result) is: the
///   change within the precise measure's own precision is no change. By volume alone: a fuse
///   only adds and a cut only removes, so the volume is the witness, and counts aren't (a pad
///   reversed into its base scars its top, 7 faces at the base's volume). The counts are in
///   the message as evidence only.
pub fn check_result(
    feature: &FeatureId,
    kind: SweepFeature,
    base: &FeatureId,
    before: &Measured,
    after: &Measured,
) -> Result<(), Error> {
    let (id, base_id, word) = (feature.as_str(), base.as_str(), kind.word());
    let failed = |message: String, suggestion: &str| {
        Err(Error::new(ErrorCode::KernelFailed, message)
            .with_feature(id)
            .with_suggestion(suggestion))
    };
    match after.solids {
        1 => {}
        0 => {
            return failed(
                format!("{word} {id} leaves nothing of its base {base_id}: the result is no solid"),
                if kind.adds() {
                    "OCCT lost both shapes: move the sketch slightly"
                } else {
                    "move the sketch or shorten the cut, so it leaves some of its base"
                },
            );
        }
        n => {
            return if kind.adds() {
                failed(
                    format!(
                        "{word} {id} doesn't reach its base {base_id}: the result is {n} solids"
                    ),
                    &format!("move the sketch or lengthen the {word}, or {word} it with no base"),
                )
            } else {
                failed(
                    format!(
                        "{word} {id} cuts its base {base_id} in {}: the result is {n} solids",
                        pieces(n)
                    ),
                    &format!("move the sketch or shorten the {word}"),
                )
            };
        }
    }
    let change = if kind.adds() {
        after.volume - before.volume
    } else {
        before.volume - after.volume
    };
    if change <= MEASURE_PRECISION * before.volume {
        let (verb, preposition) = if kind.adds() {
            ("adds", "to")
        } else {
            ("removes", "from")
        };
        let counts = format!(
            "{} faces, {} edges, and {} vertices, against the base's {}, {}, and {}",
            after.faces, after.edges, after.vertices, before.faces, before.edges, before.vertices
        );
        let suggestion = if kind.adds() {
            format!(
                "if the {word} points into its base, reverse it; if it lies inside it, move its sketch; if neither, OCCT lost it: a thin wall of its profile flush with a face of the base can do this (docs/ports/sketch-features.md), so thicken it"
            )
        } else {
            format!(
                "if the {word} points away from its base, reverse it; if it misses it, move its sketch; if neither, OCCT lost it: a thin wall of its profile flush with a face of the base can do this (docs/ports/sketch-features.md), so thicken it"
            )
        };
        return failed(
            format!(
                "{word} {id} {verb} nothing {preposition} its base {base_id}: the result's volume is the base's, {} mm³ ({counts})",
                format_number(after.volume)
            ),
            &suggestion,
        );
    }
    Ok(())
}

/// How many pieces `n` is, in words where it's short: `two`, `three`, else the number.
fn pieces(n: u32) -> String {
    match n {
        2 => "two".to_owned(),
        3 => "three".to_owned(),
        n => n.to_string(),
    }
}
