//! What a pad, pocket, revolve, or groove's rebuild refuses that the gate can't see
//! (docs/design/m3.6-sketch-features.md, S5 to S7), as pure functions of what the rebuild
//! already has, for recompute to call (M3.6's step iv): from the solved sketch, before the
//! kernel is asked for anything (decision 2), a revolve's or groove's axis of no length (S6),
//! and a pad or revolve of more than one region (decision 3); and from the built result, more
//! than one solid (D2), or a volume that shows the feature changed nothing (S7).

use silico_protocol::{MEASURE_PRECISION, format_number, no_distance};

use crate::document::{Axis, Checked, FeatureId, Operation, PROBLEMS_LISTED, SweepFeature};
use crate::profile::Profile;
use crate::sketch::SketchSolution;
use crate::{Error, ErrorCode};

/// Whether revolve or groove `feature`'s axis gives a direction to turn about, its sketch
/// solved as `solution`: `x_axis` and `y_axis` always do; a line does unless its solved ends
/// are no distance apart by M3.2's zero-size rule, the solver's own squared length
/// (`dx² + dy²`, exactly 0: [`silico_protocol::no_distance`]), which the gate can't see (a
/// solve can bring two points together). So ends at one place are refused, and so are ends
/// so close their squared distance underflows to 0 (5e-324 apart), where OCCT's direction
/// would throw. Refused as `invalid_input`, naming the feature and its `axis`. Ends whose
/// squared distance isn't 0 pass, however small: whether the profile can be turned about
/// them is the kernel's to say.
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
    // As the gate and the solver measure a line of no size: -0 and 0 are one place.
    if no_distance(from, to) {
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

/// Where a revolve's or groove's axis lies in its sketch's plane, in the sketch's
/// coordinates: a point on it, its unit direction, and its line's ID, if it's a line.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisLine {
    pub origin: [f64; 2],
    pub direction: [f64; 2],
    pub line: Option<crate::document::ElementId>,
}

/// Where revolve or groove `feature`'s axis lies, its sketch solved as `solution`: `x_axis`
/// from the origin along +x, `y_axis` along +y, a line from its solved `from` towards its
/// `to`, its direction divided by its length. Call [`check_axis_length`] first: a line of no
/// length gives no direction, and is refused here as a bug.
pub fn axis_line(
    document: &Checked,
    feature: &FeatureId,
    solution: &SketchSolution,
) -> Result<AxisLine, Error> {
    let features = &document.document().features;
    let found = features
        .iter()
        .find(|f| &f.id == feature)
        .ok_or_else(|| unchecked(format!("has no feature {feature}")))?;
    let (Some(axis), Some(sketch_id)) = (found.operation.axis(), found.operation.sketch()) else {
        return Err(unchecked(format!("has {feature}, which has no axis")));
    };
    let along = |direction| AxisLine {
        origin: [0.0, 0.0],
        direction,
        line: None,
    };
    let line_id = match axis {
        Axis::XAxis => return Ok(along([1.0, 0.0])),
        Axis::YAxis => return Ok(along([0.0, 1.0])),
        Axis::Line(line) => line,
    };
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
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let length = (dx * dx + dy * dy).sqrt();
    if !(length.is_finite() && length > 0.0) {
        return Err(unchecked(format!(
            "turns {feature} about {line_id}, whose ends are no distance apart"
        )));
    }
    Ok(AxisLine {
        origin: from,
        direction: [dx / length, dy / length],
        line: Some(line_id.clone()),
    })
}

/// How far each of a profile's curves reaches either side of the line through `origin`
/// along the unit `direction`: its least and greatest signed distance from it (positive to
/// the line's left), closed-form (design S6): a line's ends; an arc's ends, and its circle's
/// two points farthest from the line where they lie on the arc; a circle's centre ± its
/// radius.
fn reach(edge: &crate::profile::Edge, origin: [f64; 2], direction: [f64; 2]) -> (f64, f64) {
    let side = |p: [f64; 2]| direction[0] * (p[1] - origin[1]) - direction[1] * (p[0] - origin[0]);
    match edge.sent {
        silico_protocol::Curve::Line { start, end } => {
            let (a, b) = (side(start), side(end));
            (a.min(b), a.max(b))
        }
        silico_protocol::Curve::Circle { center, radius } => {
            let c = side(center);
            (c - radius, c + radius)
        }
        silico_protocol::Curve::Arc {
            center,
            start,
            end,
            radius,
            start_angle,
            end_angle,
        } => {
            let (a, b) = (side(start), side(end));
            let (mut low, mut high) = (a.min(b), a.max(b));
            // The arc runs from the lesser of its angles to the greater, less than a turn.
            let (from, to) = (start_angle.min(end_angle), start_angle.max(end_angle));
            let on_arc = |angle: f64| {
                let turned = from + (angle - from).rem_euclid(std::f64::consts::TAU);
                turned <= to
            };
            // The line's left normal, and the circle's points along it and against it.
            let normal = [-direction[1], direction[0]];
            let c = side(center);
            if on_arc(libm::atan2(normal[1], normal[0])) {
                high = high.max(c + radius);
            }
            if on_arc(libm::atan2(-normal[1], -normal[0])) {
                low = low.min(c - radius);
            }
            (low, high)
        }
    }
}

/// Whether revolve or groove `feature` can turn its sketch's `profile` (solved as
/// `solution`) about its axis (design S6), once [`check_axis_length`] has passed:
///
/// - **the axis mustn't cross the profile**: every curve lies on one side of the axis's
///   line, a point within OCCT's tolerance (1e-7 mm) of it counting as on it, so the profile
///   may touch the axis or run along it. A crossing is refused as `invalid_input`, the
///   feature the sketch (its elements are where it's fixed), the elements every curve
///   reaching off the axis (on one side or the other), by ID, and the axis, if it's a line;
/// - **the angle must move something**: the profile's farthest point from the axis times
///   the angle in radians more than the tolerance, as M1's revolve refuses one; refused as
///   `invalid_input`, naming the feature and its `extent.angle`.
pub fn check_axis_turn(
    document: &Checked,
    feature: &FeatureId,
    solution: &SketchSolution,
    profile: &Profile,
) -> Result<(), Error> {
    let _span = tracing::debug_span!("check_axis_turn", feature = feature.as_str()).entered();
    let AxisLine {
        origin,
        direction,
        line,
    } = axis_line(document, feature, solution)?;
    let found = document
        .document()
        .features
        .iter()
        .find(|f| &f.id == feature)
        .ok_or_else(|| unchecked(format!("has no feature {feature}")))?;
    let (Some(sketch), Some(extent)) = (found.operation.sketch(), found.operation.extent()) else {
        return Err(unchecked(format!("has {feature}, which turns no sketch")));
    };
    let word = match found.operation.made() {
        crate::document::Made::Sweep(kind) => kind.word(),
        _ => "revolve",
    };
    let tolerance = silico_protocol::LENGTH_TOLERANCE;
    let (mut low, mut high) = (0.0f64, 0.0f64);
    let mut off: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for region in &profile.regions {
        for edge in std::iter::once(&region.outer)
            .chain(&region.holes)
            .flat_map(|l| &l.curves)
        {
            let (l, h) = reach(edge, origin, direction);
            low = low.min(l);
            high = high.max(h);
            if l < -tolerance || h > tolerance {
                off.insert(edge.curve.to_string());
            }
        }
    }
    if low < -tolerance && high > tolerance {
        let total = off.len() + usize::from(line.is_some());
        let mut elements: Vec<String> = off.iter().take(PROBLEMS_LISTED).cloned().collect();
        if let Some(line) = &line
            && elements.len() < PROBLEMS_LISTED
        {
            elements.push(line.to_string());
        }
        let axis = match &line {
            Some(line) => format!("its axis, line {line},"),
            None => format!(
                "its axis, the sketch's {},",
                found.operation.axis().map_or("axis", |a| a.as_str())
            ),
        };
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{word} {feature} turns sketch {sketch} about a line that crosses its profile: {axis} has the profile's curves on both sides, {} mm on one and {} mm on the other",
                format_number(-low),
                format_number(high)
            ),
        )
        .with_feature(sketch.as_str())
        .with_elements(elements)
        .with_elements_total(total as u64)
        .with_suggestion(
            "move the profile to one side of the axis, or turn it about another line; a profile may touch its axis",
        ));
    }
    let degrees = match extent {
        crate::document::Extent::Turn(
            crate::document::TurnExtent::OneSide { angle, .. }
            | crate::document::TurnExtent::Symmetric { angle },
        ) => angle.degrees(),
        _ => return Err(unchecked(format!("has {feature}, which turns by no angle"))),
    };
    let farthest = (-low).max(high);
    if farthest * silico_protocol::radians(degrees) <= tolerance {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{word} {feature}'s angle, {} degrees, moves its profile's farthest point, {} mm from the axis, by at most 1e-7 mm: it would make no solid",
                format_number(degrees),
                format_number(farthest)
            ),
        )
        .with_feature(feature.as_str())
        .with_parameter("extent.angle")
        .with_suggestion("turn it by a larger angle"));
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
/// - **A volume**: the change needs both volumes to be ones a solid has, finite and above 0
///   (the base's) or at least 0 (the result's); one that isn't (NaN, infinite, or negative,
///   as a broken or inverted shape measures) witnesses nothing, and is refused, as the
///   kernel's boolean refuses a result of no volume (break-it found NaN passing every
///   comparison, and an inverted base making the threshold negative).
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
    for (whose, volume, least) in [
        ("its base's", before.volume, f64::MIN_POSITIVE),
        ("its result's", after.volume, 0.0),
    ] {
        if !(volume.is_finite() && volume >= least) {
            return failed(
                format!(
                    "{word} {id} can't be told to change its base {base_id}: {whose} volume measured {}, which no solid has",
                    format_number(volume)
                ),
                "OCCT measured a broken shape: rebuild the base, or change the feature slightly",
            );
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
