//! What only a solve can tell of a pad, pocket, revolve, or groove
//! (docs/design/m3.6-sketch-features.md, S5 to S7): checked in the core, from the solved
//! sketch, before the kernel is asked for anything (decision 2). Each check is a pure
//! function of the checked document and the sketch's solution, for recompute to call once
//! the sketch is solved. So far: a revolve's or groove's axis of no length (S6).

use crate::document::{Axis, Checked, FeatureId, Operation};
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
