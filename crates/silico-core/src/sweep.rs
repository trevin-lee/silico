//! What a pad, pocket, revolve, or groove's rebuild refuses that the gate can't see
//! (docs/design/m3.6-sketch-features.md, S5 to S7), as pure functions of what the rebuild
//! already has, for recompute to call (M3.6's step iv): from the solved sketch, before the
//! kernel is asked for anything (decision 2), a revolve's or groove's axis of no length (S6),
//! and a pad or revolve of more than one region (decision 3); and from the built result, more
//! than one solid (D2), or a volume that shows the feature changed nothing (S7). And M2's
//! booleans' one-solid rule (D2), which keeps every base one solid. Recompute asks the
//! result's checks of each feature when its outcome is decided, from its solid's stored
//! measures, however the solid was had (made, kept, or read from the cache).
//!
//! And where a pad or pocket sweeps from its sketch's plane ([`prism`]): its prism step's
//! start and vector, and where each of its lengths ends, so the rebuild's step and the app's
//! arrows on the canvas read one rule.

use silico_protocol::{EndAtStart, MEASURE_PRECISION, format_number, no_distance};

use crate::document::{
    Axis, Checked, Extent, FeatureId, Length, Operation, PROBLEMS_LISTED, SweepFeature,
};
use crate::profile::Profile;
use crate::sketch::SketchSolution;
use crate::{Error, ErrorCode};

/// A revolve's or groove's axis as its sketch solved it: the feature's type word, its
/// sketch, its axis as written, the line's ID if it's a line, and two points along it
/// (`x_axis` the origin and (1, 0), `y_axis` the origin and (0, 1), a line its solved
/// `from` and `to`). Found once, for every check of the axis and the step's axis.
struct SolvedAxis<'a> {
    word: &'static str,
    sketch: &'a FeatureId,
    axis: &'a Axis,
    line: Option<&'a crate::document::ElementId>,
    from: [f64; 2],
    to: [f64; 2],
}

/// `feature`'s axis, as its sketch solved it ([`SolvedAxis`]).
fn solved_axis<'a>(
    document: &'a Checked,
    feature: &FeatureId,
    solution: &SketchSolution,
) -> Result<SolvedAxis<'a>, Error> {
    let found = document.feature(feature)?;
    let (Some(axis), Some(sketch)) = (found.operation.axis(), found.operation.sketch()) else {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("feature {feature} isn't a revolve or groove: it has no axis"),
        )
        .with_feature(feature.as_str()));
    };
    let word = match found.operation.made() {
        crate::document::Made::Sweep(kind) => kind.word(),
        _ => "revolve",
    };
    let along = |to| SolvedAxis {
        word,
        sketch,
        axis,
        line: None,
        from: [0.0, 0.0],
        to,
    };
    let line_id = match axis {
        Axis::XAxis => return Ok(along([1.0, 0.0])),
        Axis::YAxis => return Ok(along([0.0, 1.0])),
        Axis::Line(line) => line,
    };
    // The gate has checked that the sketch is one, and the axis a line of it.
    let line = document
        .feature(sketch)
        .ok()
        .and_then(|f| match &f.operation {
            Operation::Sketch(drawn) => drawn.lines.iter().find(|l| &l.id == line_id),
            _ => None,
        })
        .ok_or_else(|| {
            unchecked(format!(
                "turns {feature} about {line_id}, which isn't a line of sketch {sketch}"
            ))
        })?;
    let end = |id| {
        solution.point(id).ok_or_else(|| {
            unchecked(format!(
                "solves sketch {sketch} with no point {id}, an end of {feature}'s axis"
            ))
        })
    };
    Ok(SolvedAxis {
        word,
        sketch,
        axis,
        line: Some(line_id),
        from: end(&line.from)?,
        to: end(&line.to)?,
    })
}

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
    length_of(feature, &solved_axis(document, feature, solution)?).map(|_| ())
}

/// [`check_axis_length`] of an axis found: its unit direction, if it has one.
fn length_of(feature: &FeatureId, axis: &SolvedAxis<'_>) -> Result<[f64; 2], Error> {
    // As the gate and the solver measure a line of no size: -0 and 0 are one place.
    if let Some(line_id) = axis.line
        && no_distance(axis.from, axis.to)
    {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "axis {line_id}, a line of sketch {}, has its ends at one point: it gives no direction to turn about",
                axis.sketch
            ),
        )
        .with_feature(feature.as_str())
        .with_parameter("axis")
        .with_suggestion("give the line some length, or turn about another line"));
    }
    // The ends are apart (dx² + dy² isn't 0) and within reach (a solution's every value is,
    // or it isn't kept), so the length is finite and above 0.
    let (dx, dy) = (axis.to[0] - axis.from[0], axis.to[1] - axis.from[1]);
    let length = (dx * dx + dy * dy).sqrt();
    Ok([dx / length, dy / length])
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
/// `to`, its direction divided by its length. An axis of no length is refused, as
/// [`check_axis_length`] refuses it.
pub fn axis_line(
    document: &Checked,
    feature: &FeatureId,
    solution: &SketchSolution,
) -> Result<AxisLine, Error> {
    let axis = solved_axis(document, feature, solution)?;
    let direction = length_of(feature, &axis)?;
    Ok(AxisLine {
        origin: axis.from,
        direction,
        line: axis.line.cloned(),
    })
}

/// The signed distance of `p` from the line through `origin` along the unit `direction`,
/// positive to the line's left.
fn side(origin: [f64; 2], direction: [f64; 2], p: [f64; 2]) -> f64 {
    direction[0] * (p[1] - origin[1]) - direction[1] * (p[0] - origin[0])
}

/// Whether the direction `angle` from an arc's centre lies on the arc, which runs from the
/// lesser of its angles to the greater, less than a turn.
fn on_arc(start_angle: f64, end_angle: f64, angle: f64) -> bool {
    let (from, to) = (start_angle.min(end_angle), start_angle.max(end_angle));
    from + (angle - from).rem_euclid(std::f64::consts::TAU) <= to
}

/// How far each of a profile's curves reaches either side of the line through `origin`
/// along the unit `direction`: its least and greatest signed distance from it (positive to
/// the line's left), closed-form (design S6): a line's ends; an arc's ends, and its circle's
/// two points farthest from the line where they lie on the arc; a circle's centre ± its
/// radius.
fn reach(edge: &crate::profile::Edge, origin: [f64; 2], direction: [f64; 2]) -> (f64, f64) {
    let side = |p: [f64; 2]| side(origin, direction, p);
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
            // The line's left normal, and the circle's points along it and against it.
            let normal = [-direction[1], direction[0]];
            let c = side(center);
            if on_arc(start_angle, end_angle, libm::atan2(normal[1], normal[0])) {
                high = high.max(c + radius);
            }
            if on_arc(start_angle, end_angle, libm::atan2(-normal[1], -normal[0])) {
                low = low.min(c - radius);
            }
            (low, high)
        }
    }
}

/// The distances from the line through `origin` along the unit `direction` of a curve's
/// points that could be the nearest of a profile to it: its ends; and its interior point
/// nearest the line, where it has one: where its circle lies off the line, the circle's
/// point toward it, the centre's distance less the radius (a circle's always; an arc's
/// where that point lies on it). A line's points between its ends are no nearer than its
/// nearer end, and an arc whose circle meets the line is nearest it at an end or where it
/// meets it.
fn nearest_distances(
    edge: &crate::profile::Edge,
    origin: [f64; 2],
    direction: [f64; 2],
) -> [Option<f64>; 3] {
    let side = |p: [f64; 2]| side(origin, direction, p);
    // The circle's point toward the line, as an angle, and its distance from the line, if
    // the circle lies off it.
    let toward = |center: [f64; 2], radius: f64| {
        let c = side(center);
        // The line's normal toward it from the centre: against the left normal from the
        // left of it, along it from the right.
        let normal = if c >= 0.0 {
            [direction[1], -direction[0]]
        } else {
            [-direction[1], direction[0]]
        };
        (c.abs() > radius).then(|| (libm::atan2(normal[1], normal[0]), c.abs() - radius))
    };
    match edge.sent {
        silico_protocol::Curve::Line { start, end } => {
            [Some(side(start).abs()), Some(side(end).abs()), None]
        }
        silico_protocol::Curve::Circle { center, radius } => {
            [None, None, toward(center, radius).map(|(_, d)| d)]
        }
        silico_protocol::Curve::Arc {
            center,
            start,
            end,
            radius,
            start_angle,
            end_angle,
        } => {
            let inside = toward(center, radius)
                .and_then(|(angle, d)| on_arc(start_angle, end_angle, angle).then_some(d));
            [Some(side(start).abs()), Some(side(end).abs()), inside]
        }
    }
}

/// Whether revolve or groove `feature` can turn its sketch's `profile` (solved as
/// `solution`) about its axis (design S6): every check of the axis, its length first
/// ([`check_axis_length`]), the axis found once; its line, if it passes, for the step.
///
/// - **the axis mustn't cross any region**: each region's curves lie on one side of the
///   axis's line, a point within OCCT's tolerance (1e-7 mm) of it counting as on it, so a
///   region may touch the axis or run along it, and two regions may lie on opposite sides
///   (FreeCAD checks each face's outer wire alone: `checkLineCrossesFace`, per face). A
///   crossing is refused as `invalid_input`, naming the feature and its `axis`, the
///   elements the sketch's, where it's fixed: the crossing region's curves reaching off
///   the axis, by ID, and the axis, if it's a line;
/// - **the turn's end mustn't be where it started** ([`silico_protocol::end_at_start`], M1's
///   revolve's rule): the profile's farthest point from the axis, over every region, must
///   move more than the tolerance, and, short of a whole turn, its slit must be wider than
///   that where it's narrowest, at its nearest point off the axis over every region and
///   hole ([`silico_protocol::slit_radius`]): 360 degrees less a few
///   nanodegrees leaves ends one place, which OCCT builds as two; refused as
///   `invalid_input`, naming the feature and its `extent.angle`.
pub fn check_axis(
    document: &Checked,
    feature: &FeatureId,
    solution: &SketchSolution,
    profile: &Profile,
) -> Result<AxisLine, Error> {
    let _span = tracing::debug_span!("check_axis", feature = feature.as_str()).entered();
    let axis = solved_axis(document, feature, solution)?;
    let direction = length_of(feature, &axis)?;
    let origin = axis.from;
    let word = axis.word;
    let tolerance = silico_protocol::LENGTH_TOLERANCE;
    let mut farthest = 0.0f64;
    // Every point that could be the profile's nearest to the axis, over every region and
    // hole, with its curve: where its slit is measured, and the curves there.
    let mut near: Vec<(f64, &crate::document::ElementId)> = Vec::new();
    // Each curve's farthest reach, for the curves there when the slit is measured there.
    let mut far: Vec<(f64, &crate::document::ElementId)> = Vec::new();
    for region in &profile.regions {
        let (mut low, mut high) = (0.0f64, 0.0f64);
        let mut off: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for edge in std::iter::once(&region.outer)
            .chain(&region.holes)
            .flat_map(|l| &l.curves)
        {
            let (l, h) = reach(edge, origin, direction);
            low = low.min(l);
            high = high.max(h);
            near.extend(
                nearest_distances(edge, origin, direction)
                    .into_iter()
                    .flatten()
                    .map(|d| (d, &edge.curve)),
            );
            far.push(((-l).max(h), &edge.curve));
            if l < -tolerance || h > tolerance {
                off.insert(edge.curve.to_string());
            }
        }
        farthest = farthest.max(-low).max(high);
        if low < -tolerance && high > tolerance {
            let total = off.len() + usize::from(axis.line.is_some());
            let mut elements: Vec<String> = off.iter().take(PROBLEMS_LISTED).cloned().collect();
            if let Some(line) = axis.line
                && elements.len() < PROBLEMS_LISTED
            {
                elements.push(line.to_string());
            }
            let said = match axis.line {
                Some(line) => format!("its axis, line {line},"),
                None => format!("its axis, the sketch's {},", axis.axis),
            };
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "{word} {feature} turns sketch {} about a line that crosses its profile: {said} has the profile's curves on both sides, {} mm on one and {} mm on the other",
                    axis.sketch,
                    format_number(-low),
                    format_number(high)
                ),
            )
            .with_feature(feature.as_str())
            .with_parameter("axis")
            .with_elements(elements)
            .with_elements_total(total as u64)
            .with_suggestion(
                "move the profile to one side of the axis, or turn it about another line; a profile may touch its axis",
            ));
        }
    }
    let found = document.feature(feature).ok();
    let degrees = match found.and_then(|f| f.operation.extent()) {
        Some(crate::document::Extent::Turn(
            crate::document::TurnExtent::OneSide { angle, .. }
            | crate::document::TurnExtent::Symmetric { angle },
        )) => angle.degrees(),
        _ => return Err(unchecked(format!("has {feature}, which turns by no angle"))),
    };
    let refused = |message: String, suggestion: &str| {
        Error::new(ErrorCode::InvalidInput, message)
            .with_feature(feature.as_str())
            .with_parameter("extent.angle")
            .with_suggestion(suggestion)
    };
    let turned = silico_protocol::radians(degrees);
    let slit_at = silico_protocol::slit_radius(near.iter().map(|&(d, _)| d), farthest);
    match silico_protocol::end_at_start(slit_at.distance(), farthest, turned) {
        Some(EndAtStart::Unmoved) => {
            return Err(refused(
                format!(
                    "{word} {feature}'s angle, {} degrees, moves its profile's farthest point, {} mm from the axis, by at most 1e-7 mm: it would make no solid",
                    format_number(degrees),
                    format_number(farthest)
                ),
                "turn it by a larger angle",
            ));
        }
        Some(EndAtStart::ShortOfWhole { at, slit }) => {
            // The sketch's curves where the slit is measured, by ID: a sweep refusal's
            // elements are its sketch's. Those with a point there that could be nearest; or,
            // where the rule fell back to the farthest point (no point off the axis), those
            // reaching it. `far` names curves only in that fallback: a curve whose farthest
            // point merely lies at `at` beside a measured corner isn't named, since its edges
            // running out from the axis share the axis vertex and are harmless, and its
            // extreme sitting at `at` is a coincidence, not where the rule measured.
            let measured = match slit_at {
                silico_protocol::SlitAt::Nearest(_) => &near,
                silico_protocol::SlitAt::Farthest(_) => &far,
            };
            let there: std::collections::BTreeSet<String> = measured
                .iter()
                .filter(|&&(d, _)| d == at)
                .map(|(_, curve)| curve.to_string())
                .collect();
            return Err(refused(
                format!(
                    "{word} {feature}'s angle, {} degrees, short of a whole turn, leaves a slit of {} mm at its profile's point {} mm from the axis: within 1e-7 mm, its ends would be one place",
                    format_number(degrees),
                    format_number(slit),
                    format_number(at)
                ),
                "turn it by 360 degrees for a whole turn, or by less, so its ends are more than 1e-7 mm apart",
            )
            .with_elements(there.iter().take(PROBLEMS_LISTED).cloned())
            .with_elements_total(there.len() as u64));
        }
        None => {}
    }
    Ok(AxisLine {
        origin,
        direction,
        line: axis.line.cloned(),
    })
}

/// [`check_axis`]'s checks of the profile, once [`check_axis_length`] has passed.
pub fn check_axis_turn(
    document: &Checked,
    feature: &FeatureId,
    solution: &SketchSolution,
    profile: &Profile,
) -> Result<(), Error> {
    check_axis(document, feature, solution, profile).map(|_| ())
}

/// A document or solution the caller should have checked, and didn't: a bug in Silico.
fn unchecked(what: String) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("a revolve's axis was checked against a document or solution that {what}"),
    )
    .with_suggestion(crate::worker::SILICO_BUG)
}

/// Design S3: pad, pocket, revolve, or groove `feature`, of type `kind`, sweeps `sketch`,
/// which solved but makes no profile (`refused`). The feature's own error, one however many
/// refusals there are, each sweep of the sketch its own, whole: `invalid_input` (or the
/// first listed refusal's code that isn't, a bug, with a bug's advice), naming the feature
/// and its `sketch`; the sketch's elements at fault, every refusal's, by ID (the first
/// hundred, and their total: a sweep refusal's elements are its sketch's); its cause each
/// listed refusal's message, each distinct advice given once, after the first refusal it
/// follows, and how many more. The sketch itself isn't failed, and no error names it: a
/// sketch nothing sweeps may be open.
pub fn unprofiled(
    feature: &FeatureId,
    kind: SweepFeature,
    sketch: &FeatureId,
    refused: &crate::profile::Refused,
) -> Error {
    let listed = refused.errors();
    let code = listed
        .iter()
        .map(Error::code)
        .find(|&code| code != ErrorCode::InvalidInput)
        .unwrap_or(ErrorCode::InvalidInput);
    // Each refusal's message, with its advice the first time that advice is given.
    let mut given: Vec<&str> = Vec::new();
    let mut cause = listed
        .iter()
        .map(|e| match e.suggestion() {
            Some(advice) if !given.contains(&advice) => {
                given.push(advice);
                format!("{} ({advice})", e.message())
            }
            _ => e.message().to_owned(),
        })
        .collect::<Vec<_>>()
        .join("; ");
    let more = refused.total().saturating_sub(listed.len() as u64);
    if more > 0 {
        cause.push_str(&format!("; and {more} more"));
    }
    let count = match refused.total() {
        1 => "1 refusal".to_owned(),
        n => format!("{n} refusals"),
    };
    let suggestion = if code == ErrorCode::Internal {
        crate::worker::SILICO_BUG.to_owned()
    } else {
        format!("fix sketch {sketch} where the cause says, or sweep another sketch")
    };
    Error::new(
        code,
        format!(
            "{} {feature} sweeps sketch {sketch}, which makes no profile ({count})",
            kind.word()
        ),
    )
    .with_feature(feature.as_str())
    .with_parameter("sketch")
    .with_elements(refused.elements())
    .with_elements_total(refused.elements_total())
    .with_cause(cause)
    .with_suggestion(suggestion)
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
/// join them. `invalid_input`, naming the feature and its `sketch`, the elements the
/// sketch's, where they're fixed: every region's outer-loop curves in region order (the
/// first hundred, and their total). A pocket or a groove takes every region.
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
        .with_feature(feature.as_str())
        .with_parameter("sketch")
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

/// D2's letter for M2's booleans (Fable's decision, 2026-10-18; design S5): a `fuse`, `cut`,
/// or `common` feature whose result holds more than one solid is refused, `kernel_failed`,
/// naming the feature: two boxes apart fused, a box cut in two, two pieces in common. A
/// solid nothing can address can't be made until bodies exist; the kernel's boolean
/// returns what OCCT builds, and the feature layer refuses it here.
pub fn check_boolean_solids(
    feature: &FeatureId,
    op: silico_protocol::BooleanOp,
    base: &FeatureId,
    tool: &FeatureId,
    solids: u32,
) -> Result<(), Error> {
    use silico_protocol::BooleanOp;
    if solids <= 1 {
        return Ok(());
    }
    let (message, suggestion) = match op {
        BooleanOp::Fuse => (
            format!(
                "fuse {feature} makes {solids} solids: its base {base} and tool {tool} don't touch, or meet only along an edge or at a point"
            ),
            "move them to overlap or share a face; a solid nothing can address can't be made until bodies exist",
        ),
        BooleanOp::Cut => (
            format!(
                "cut {feature} cuts its base {base} in {}: the result is {solids} solids",
                pieces(solids)
            ),
            "shorten or move the tool so the base stays one piece",
        ),
        BooleanOp::Common => (
            format!(
                "common {feature} makes {solids} solids of base {base} and tool {tool}: they overlap in {} places",
                pieces(solids)
            ),
            "move them so they overlap in one place",
        ),
    };
    Err(Error::new(ErrorCode::KernelFailed, message)
        .with_feature(feature.as_str())
        .with_suggestion(suggestion))
}

/// What a sketch feature's result must be, once its boolean is built (design S5, S7): one
/// solid, and a change to its base's volume. Both `kernel_failed`, naming the feature: the
/// geometry is valid and so is the input, but the feature didn't do what it's for.
///
/// - **One solid** (D2): more than one is refused (a pad that misses its base, a pocket that
///   cuts it in two), and so is none, which the kernel's empty boolean refuses first. A base
///   is always one solid: M2's booleans hold to D2 too ([`check_boolean_solids`]).
/// - **A change**: a pad or revolve is refused when V(result) − V(base) is at most
///   [`MEASURE_PRECISION`] × V(base), and a pocket or groove when V(base) − V(result) is: the
///   change within the precise measure's own precision is no change. By volume alone: a fuse
///   only adds and a cut only removes, so the volume is the witness, and counts aren't (a pad
///   reversed into its base scars its top, 7 faces at the base's volume). The counts are in
///   the message as evidence only. A change above 0 but within the precision (a small
///   feature on a large part, S11) is said as what it is, the change and the limit.
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
    judged(feature.as_str(), kind, base.as_str(), before, after)
}

/// Whether [`check_result`] passes a result, whatever feature it's for: the verdict is the
/// measures', the same for every feature sharing the step (only the words are each
/// feature's), so the scheduler stops the work only a refused feature's users need by it.
pub fn result_holds(kind: SweepFeature, before: &Measured, after: &Measured) -> bool {
    judged("", kind, "", before, after).is_ok()
}

/// What an M2 `fuse`, `cut`, or `common` feature's result must be, once
/// [`check_boolean_solids`] has passed it: a change to its base's volume, by S7's rule and
/// in its words, as [`check_result`] judges a sketch feature's (SolidWorks refuses a
/// feature that changes nothing). A fuse adds to its base, and a cut and a common take from
/// it, so a fuse whose tool lies inside its base, a cut whose tool misses it, and a common
/// whose tool holds all of it are refused, `kernel_failed`, naming the feature; and so is
/// one whose tool OCCT took for nothing under the fuzzy value every boolean runs with (a
/// tool under a millionth of the boolean's size, docs/ports/booleans.md).
pub fn check_boolean_change(
    feature: &FeatureId,
    op: silico_protocol::BooleanOp,
    base: &FeatureId,
    before: &Measured,
    after: &Measured,
) -> Result<(), Error> {
    judged(feature.as_str(), op, base.as_str(), before, after)
}

/// Whether [`check_boolean_change`] passes a result, as [`result_holds`] is for a sketch
/// feature's.
pub fn boolean_change_holds(
    op: silico_protocol::BooleanOp,
    before: &Measured,
    after: &Measured,
) -> bool {
    judged("", op, "", before, after).is_ok()
}

/// What a linear pattern's, polar pattern's, or mirror's step must be, once its result is
/// one solid (docs/design/m6.0-patterns.md, section 4.2): a change to the volume of what it
/// builds on (its base, or the step before), by S7's rule and in its words, as
/// [`check_result`] judges a sketch feature's (decisions 6 and 9): a fuse step adds, a cut
/// step removes. A mirror about its original's own plane of symmetry, or copies all inside
/// the base or all missing it, change nothing, and are refused, `kernel_failed`, naming the
/// feature.
pub fn check_pattern_change(
    feature: &FeatureId,
    kind: crate::document::PatternFeature,
    op: silico_protocol::CopiesOp,
    base: &FeatureId,
    before: &Measured,
    after: &Measured,
) -> Result<(), Error> {
    judged(
        feature.as_str(),
        PatternChange { kind, op },
        base.as_str(),
        before,
        after,
    )
}

/// Whether [`check_pattern_change`] passes a step's result, as [`result_holds`] is for a
/// sketch feature's.
pub fn pattern_change_holds(
    kind: crate::document::PatternFeature,
    op: silico_protocol::CopiesOp,
    before: &Measured,
    after: &Measured,
) -> bool {
    judged("", PatternChange { kind, op }, "", before, after).is_ok()
}

/// A pattern's step, as S7 judges its change.
#[derive(Debug, Clone, Copy)]
struct PatternChange {
    kind: crate::document::PatternFeature,
    op: silico_protocol::CopiesOp,
}

impl Changes for PatternChange {
    fn word(self) -> &'static str {
        self.kind.word()
    }

    fn adds(self) -> bool {
        self.op == silico_protocol::CopiesOp::Fuse
    }

    fn when_lost(self) -> &'static str {
        "move or shrink the copies, so the result keeps some of its base"
    }

    fn when_parted(self) -> String {
        "move the copies so the result is one piece".to_owned()
    }

    fn when_too_small(self) -> String {
        format!(
            "make what the {} copies larger, or the {} on a smaller part",
            self.word(),
            self.word()
        )
    }

    fn when_unchanged(self) -> String {
        "if every copy lies inside the base or misses it, change the direction, length, count, or plane; a mirror about the original's own plane of symmetry changes nothing".to_owned()
    }
}

/// What changes a base's volume and is judged for it (S7): a sketch feature, or an M2
/// boolean. Its word, whether it adds, and what to try when it changes nothing.
trait Changes: Copy {
    /// The `type` the file writes for it.
    fn word(self) -> &'static str;
    /// Whether it adds to its base (a fuse), rather than taking from it (a cut).
    fn adds(self) -> bool;
    /// What to try when the result is no solid.
    fn when_lost(self) -> &'static str;
    /// What to try when the result is several solids.
    fn when_parted(self) -> String;
    /// What to try when the change is above 0 but within the measure's precision.
    fn when_too_small(self) -> String;
    /// What to try when the result's volume is the base's.
    fn when_unchanged(self) -> String;
}

impl Changes for SweepFeature {
    fn word(self) -> &'static str {
        SweepFeature::word(self)
    }

    fn adds(self) -> bool {
        SweepFeature::adds(self)
    }

    fn when_lost(self) -> &'static str {
        if self.adds() {
            "OCCT lost both shapes: move the sketch slightly"
        } else {
            "move the sketch or shorten the cut, so it leaves some of its base"
        }
    }

    fn when_parted(self) -> String {
        let word = self.word();
        if self.adds() {
            format!("move the sketch or lengthen the {word}, or {word} it with no base")
        } else {
            format!("move the sketch or shorten the {word}")
        }
    }

    fn when_too_small(self) -> String {
        format!(
            "make the {} larger, or make it on a smaller part; if it has a thin wall flush with a face of the base, OCCT can lose it (docs/ports/sketch-features.md): thicken it",
            self.word()
        )
    }

    fn when_unchanged(self) -> String {
        let word = self.word();
        if self.adds() {
            format!(
                "if the {word} points into its base, reverse it; if it lies inside it, move its sketch; if neither, OCCT lost it: a thin wall of its profile flush with a face of the base can do this (docs/ports/sketch-features.md), so thicken it"
            )
        } else {
            format!(
                "if the {word} points away from its base, reverse it; if it misses it, move its sketch; if neither, OCCT lost it: a thin wall of its profile flush with a face of the base can do this (docs/ports/sketch-features.md), so thicken it"
            )
        }
    }
}

/// What every boolean's change refusal says beside its own advice: the fuzzy value.
const MILLIONTH: &str = "a boolean is built to a millionth of its size, so a tool that reaches less than that past or into its base is lost";

impl Changes for silico_protocol::BooleanOp {
    fn word(self) -> &'static str {
        silico_protocol::BooleanOp::word(self)
    }

    fn adds(self) -> bool {
        self == silico_protocol::BooleanOp::Fuse
    }

    fn when_lost(self) -> &'static str {
        "move or shrink the tool, so the result keeps some of its base"
    }

    fn when_parted(self) -> String {
        "move the tool so the result is one piece".to_owned()
    }

    fn when_too_small(self) -> String {
        format!(
            "make the tool larger, or the {} on a smaller part",
            self.word()
        )
    }

    fn when_unchanged(self) -> String {
        use silico_protocol::BooleanOp;
        match self {
            BooleanOp::Fuse => format!(
                "if its tool lies inside its base, move it so it reaches out of it, or remove or suppress the fuse; {MILLIONTH}"
            ),
            BooleanOp::Cut => format!(
                "if its tool misses its base, move it so they overlap, or remove or suppress the cut; {MILLIONTH}"
            ),
            BooleanOp::Common => format!(
                "if its tool holds all of its base, the common is its base: move or shrink the tool, or remove or suppress the common; {MILLIONTH}"
            ),
        }
    }
}

/// [`check_result`] and [`check_boolean_change`], the feature and base by ID.
fn judged(
    id: &str,
    kind: impl Changes,
    base_id: &str,
    before: &Measured,
    after: &Measured,
) -> Result<(), Error> {
    let word = kind.word();
    let failed = |message: String, suggestion: &str| {
        Err(Error::new(ErrorCode::KernelFailed, message)
            .with_feature(id)
            .with_suggestion(suggestion))
    };
    match after.solids {
        0 => {
            return failed(
                format!("{word} {id} leaves nothing of its base {base_id}: the result is no solid"),
                kind.when_lost(),
            );
        }
        1 => {}
        n => {
            return if kind.adds() {
                failed(
                    format!(
                        "{word} {id} doesn't reach its base {base_id}: the result is {n} solids"
                    ),
                    &kind.when_parted(),
                )
            } else {
                failed(
                    format!(
                        "{word} {id} cuts its base {base_id} in {}: the result is {n} solids",
                        pieces(n)
                    ),
                    &kind.when_parted(),
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
    let limit = MEASURE_PRECISION * before.volume;
    if change <= limit {
        let (verb, preposition) = if kind.adds() {
            ("adds", "to")
        } else {
            ("removes", "from")
        };
        let counts = format!(
            "{} faces, {} edges, and {} vertices, against the base's {}, {}, and {}",
            after.faces, after.edges, after.vertices, before.faces, before.edges, before.vertices
        );
        if change > 0.0 {
            // Something changed, but no more than the volume is measured to: a feature small
            // against its part (S11), or OCCT's thin-wall loss measured a hair off nothing.
            return failed(
                format!(
                    "{word} {id} {verb} nothing {preposition} its base {base_id} that its volume can tell: the {} mm³ it {verb} is no more than the {} mm³ (1e-9 of the base's volume) the volume is measured to ({counts})",
                    format_number(significant(change)),
                    format_number(significant(limit))
                ),
                &kind.when_too_small(),
            );
        }
        let suggestion = kind.when_unchanged();
        // No change, or one the wrong way: a result that measures less than its base for
        // what adds, or more for what takes away, says both volumes.
        let said = if change < 0.0 {
            let than = if after.volume < before.volume {
                "less"
            } else {
                "more"
            };
            format!(
                "the result's volume, {} mm³, is {than} than the base's, {} mm³",
                format_number(after.volume),
                format_number(before.volume)
            )
        } else {
            format!(
                "the result's volume is the base's, {} mm³",
                format_number(after.volume)
            )
        };
        return failed(
            format!(
                "{word} {id} {verb} nothing {preposition} its base {base_id}: {said} ({counts})"
            ),
            &suggestion,
        );
    }
    Ok(())
}

/// `value` to 9 significant figures, for a message: a measure and its limit, which the
/// precise measure gives to 1e-9 of the base's volume, read without its last bits' noise
/// (1e-9 × 8e15 is 8000000.000000001 in floating point).
fn significant(value: f64) -> f64 {
    if value == 0.0 || !value.is_finite() {
        return value;
    }
    let scale = libm::pow(10.0, 8.0 - libm::floor(libm::log10(value.abs())));
    if !scale.is_finite() || scale == 0.0 {
        return value;
    }
    let rounded = libm::round(value * scale) / scale;
    if rounded.is_finite() { rounded } else { value }
}

/// How many pieces `n` is, in words where it's short: `two`, `three`, else the number.
fn pieces(n: u32) -> String {
    match n {
        2 => "two".to_owned(),
        3 => "three".to_owned(),
        n => n.to_string(),
    }
}

/// `value`, a zero of either sign as +0: the requests' zeros are all +0 (design S3).
pub(crate) fn plus_zero(value: f64) -> f64 {
    value + 0.0
}

/// Where a pad or pocket sweeps from its sketch's plane, each place from the profile on the
/// plane ([`prism`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Prism {
    /// The way it sweeps, a unit vector for a unit normal: along the sketch's normal for a
    /// pad, against it for a pocket, the other way when the extent is `reversed`.
    pub direction: [f64; 3],
    /// Its prism step's start and vector; none for `through_all`, which sweeps from the
    /// plane through all of its base.
    pub span: Option<Span>,
    /// Each length the extent holds, in the order the file writes them: where it ends.
    pub ends: Vec<LengthEnd>,
}

/// A prism's start, from the profile on the plane, and the vector it sweeps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: [f64; 3],
    pub vector: [f64; 3],
}

/// Where one of an extent's lengths ends: the end of the sweep the length sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LengthEnd {
    /// The extent's field: `length`, or `length2`.
    pub field: &'static str,
    /// What it holds.
    pub length: Length,
    /// The end, from the profile on the plane.
    pub offset: [f64; 3],
    /// The way the end moves as the length grows: the sweep's direction, or against it for
    /// `length2`.
    pub way: [f64; 3],
    /// How far the length grows for each millimetre its end moves: 1, or 2 for a
    /// `symmetric` extent's, half of which lies each side of the plane.
    pub per_mm: f64,
}

/// The way a pad (`kind` adding) or a pocket sweeps from a plane whose normal is `normal`:
/// along it for a pad, against it for a pocket, and either the other way when `reversed`
/// (docs/design/m3.6-sketch-features.md, *Directions*).
pub fn prism_direction(kind: SweepFeature, reversed: bool, normal: [f64; 3]) -> [f64; 3] {
    let sign = if kind.adds() != reversed { 1.0 } else { -1.0 };
    normal.map(|c| plus_zero(c * sign))
}

/// Where the pad or pocket of `extent` sweeps from its sketch's plane, whose normal is
/// `normal`; none for a revolve's or groove's extent. Each value is from the document's
/// integers, exact or rounded once (design S3): `one_side` starts on the plane; `symmetric`
/// half its length back (halving is exact), its end half its length on; `two_sides`
/// `length2` back, swept their sum, taken in nanometres, `length` ending on the sweep and
/// `length2` against it. `through_all` has no length.
pub fn prism(extent: Extent, normal: [f64; 3]) -> Option<Prism> {
    use crate::document::{PadExtent, PocketExtent};
    let kind = match extent {
        Extent::Pad(_) => SweepFeature::Pad,
        Extent::Pocket(_) => SweepFeature::Pocket,
        Extent::Turn(_) => return None,
    };
    let along = |d: [f64; 3], length: f64| d.map(|c| plus_zero(c * length));
    let swept = |direction: [f64; 3], back: f64, length: f64, ends: Vec<LengthEnd>| Prism {
        direction,
        span: Some(Span {
            start: along(direction, -back),
            vector: along(direction, length),
        }),
        ends,
    };
    let end = |field, length: Length, offset: f64, way: [f64; 3], per_mm| LengthEnd {
        field,
        length,
        offset: along(way, offset),
        way,
        per_mm,
    };
    Some(match extent {
        Extent::Pad(PadExtent::OneSide { length, reversed })
        | Extent::Pocket(PocketExtent::OneSide { length, reversed }) => {
            let d = prism_direction(kind, reversed, normal);
            let ends = vec![end("length", length, length.mm(), d, 1.0)];
            swept(d, 0.0, length.mm(), ends)
        }
        Extent::Pad(PadExtent::Symmetric { length })
        | Extent::Pocket(PocketExtent::Symmetric { length }) => {
            let d = prism_direction(kind, false, normal);
            let half = length.mm() / 2.0;
            swept(
                d,
                half,
                length.mm(),
                vec![end("length", length, half, d, 2.0)],
            )
        }
        Extent::Pad(PadExtent::TwoSides { length, length2 })
        | Extent::Pocket(PocketExtent::TwoSides { length, length2 }) => {
            let d = prism_direction(kind, false, normal);
            let total = Length::from_nm(length.nm().saturating_add(length2.nm()));
            let against = d.map(|c| plus_zero(-c));
            let ends = vec![
                end("length", length, length.mm(), d, 1.0),
                end("length2", length2, length2.mm(), against, 1.0),
            ];
            swept(d, length2.mm(), total.mm(), ends)
        }
        Extent::Pocket(PocketExtent::ThroughAll { reversed }) => Prism {
            direction: prism_direction(kind, reversed, normal),
            span: None,
            ends: Vec::new(),
        },
        Extent::Turn(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{compared, looked_up};

    /// A document of `n` sketches on xz, `s0000000` on, each followed by its revolve,
    /// `r0000000` on: the square from (1, 0) to (2, 1), turned a whole turn about the
    /// construction line na000000 from (0, 0) to (0, 1).
    fn revolves(n: usize) -> Checked {
        let point = |id: &str, x: u32, y: u32| {
            format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
        };
        let line = |id: &str, from: &str, to: &str, construction: bool| {
            format!(
                r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": {construction}}}"#
            )
        };
        let points = [
            point("pa000000", 0, 0),
            point("pb000000", 0, 1),
            point("pc000000", 1, 0),
            point("pd000000", 2, 0),
            point("pe000000", 2, 1),
            point("pf000000", 1, 1),
        ]
        .join(", ");
        let lines = [
            line("na000000", "pa000000", "pb000000", true),
            line("nb000000", "pc000000", "pd000000", false),
            line("nc000000", "pd000000", "pe000000", false),
            line("nd000000", "pe000000", "pf000000", false),
            line("ne000000", "pf000000", "pc000000", false),
        ]
        .join(", ");
        let features: Vec<String> = (0..n)
            .flat_map(|i| {
                [
                    format!(
                        r#"{{"id": "s{i:07}", "type": "sketch", "v": 1, "name": "Profile", "plane": "xz", "points": [{points}], "lines": [{lines}], "circles": [], "arcs": [], "constraints": []}}"#
                    ),
                    format!(
                        r#"{{"id": "r{i:07}", "type": "revolve", "v": 1, "name": "Turned", "sketch": "s{i:07}", "axis": "na000000", "extent": {{"type": "one_side", "angle": 360, "reversed": false}}}}"#
                    ),
                ]
            })
            .collect();
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
            features.join(", ")
        );
        Checked::new(crate::document::read(text.as_bytes()).unwrap()).unwrap()
    }

    /// This thread's comparisons of IDs and lookups in indexes made by `run`.
    fn counting<T>(run: impl FnOnce() -> T) -> (T, u64, u64) {
        let (compared_before, looked_up_before) = (compared(), looked_up());
        let made = run();
        (
            made,
            compared() - compared_before,
            looked_up() - looked_up_before,
        )
    }

    #[test]
    fn solving_profiling_and_checking_the_axes_of_n_revolves_compares_n_log_n_ids() {
        // Each of these finds the features it's given by the document's index, once each
        // feature it reads: `sketch::solve` its sketch, `profile::profile` its sketch,
        // `check_axis` its revolve (for the axis), its sketch (for the axis's line), and its
        // revolve again (for its angle), `check_axis_length` the first two. Of N = 1,000
        // features (500 sketches, 500 revolves), a lookup is a binary search of 1,000 IDs:
        // ⌈log₂ 1,000⌉ + 1 = 11 comparisons each, and at least 1. Nothing else they do
        // compares two features' IDs. So for the 500: 500 lookups and at most 5,500
        // comparisons solving, the same profiling, 1,500 and 16,500 checking the axes, and
        // 1,000 and 11,000 checking their lengths. A scan put back in any of them compares
        // far more: `Document::feature` every ID, 1,000 a lookup, 500,000 for the 500; a
        // scan to the sketch or the revolve, its place + 1, 2i + 1 or 2i + 2 for the i-th,
        // about 250,000 for the 500.
        let pairs = 500;
        let document = revolves(pairs);
        let n = document.document().features.len();
        assert_eq!(n, 2 * pairs);
        let each = u64::from(n.next_power_of_two().trailing_zeros()) + 1;
        assert_eq!(each, 11);
        let mut solver = crate::fake::FakeSketches::new();
        let ids = |prefix: char| -> Vec<FeatureId> {
            (0..pairs)
                .map(|i| FeatureId::new(&format!("{prefix}{i:07}")).unwrap())
                .collect()
        };
        let (sketches, turned) = (ids('s'), ids('r'));
        let within = |what: &str, compared: u64, looked_up: u64, lookups: u64| {
            assert!(
                (lookups..=lookups * each).contains(&compared),
                "{what}: {compared} comparisons, not from {lookups} to {}",
                lookups * each
            );
            assert_eq!(looked_up, lookups, "{what}");
        };
        let pairs = pairs as u64;

        let (solutions, compared, looked_up) = counting(|| {
            sketches
                .iter()
                .map(|sketch| crate::sketch::solve(&document, sketch, &mut solver).unwrap())
                .collect::<Vec<_>>()
        });
        within("solving", compared, looked_up, pairs);

        let (profiles, compared, looked_up) = counting(|| {
            sketches
                .iter()
                .zip(&solutions)
                .map(|(sketch, solution)| {
                    crate::profile::profile(&document, sketch, solution).unwrap()
                })
                .collect::<Vec<_>>()
        });
        within("profiling", compared, looked_up, pairs);
        assert!(profiles.iter().all(|p| p.regions.len() == 1));

        let (axes, compared, looked_up) = counting(|| {
            turned
                .iter()
                .zip(solutions.iter().zip(&profiles))
                .map(|(revolve, (solution, profile))| {
                    check_axis(&document, revolve, solution, profile).unwrap()
                })
                .collect::<Vec<_>>()
        });
        within("checking the axes", compared, looked_up, 3 * pairs);
        // About (0, 0), along +y: the line na000000.
        for axis in &axes {
            assert_eq!(axis.origin, [0.0, 0.0]);
            assert_eq!(axis.direction, [0.0, 1.0]);
            assert_eq!(axis.line.as_ref().map(|l| l.as_str()), Some("na000000"));
        }

        let (lengths, compared, looked_up) = counting(|| {
            turned
                .iter()
                .zip(&solutions)
                .map(|(revolve, solution)| check_axis_length(&document, revolve, solution))
                .collect::<Vec<_>>()
        });
        within("checking the axes' lengths", compared, looked_up, 2 * pairs);
        assert!(lengths.iter().all(Result::is_ok));
    }

    /// Whether every zero of `v` is +0, as the requests' are (design S3).
    fn plus_zeros(v: [f64; 3]) -> bool {
        v.iter().all(|c| *c != 0.0 || c.to_bits() == 0)
    }

    #[test]
    fn a_prism_sweeps_each_form_as_worked_by_hand_and_each_length_ends_where_it_does() {
        use crate::document::{Angle, PadExtent, PocketExtent, TurnExtent};
        let mm = |n: i64| Length::from_nm(n * 1_000_000);
        let up = [0.0, 0.0, 1.0];
        let span = |p: &Prism| p.span.unwrap();
        let end = |field, length, offset, way, per_mm| LengthEnd {
            field,
            length,
            offset,
            way,
            per_mm,
        };
        // A pad's one side, 10, on +Z: from the plane up to 10; reversed, down.
        let p = prism(
            Extent::Pad(PadExtent::OneSide {
                length: mm(10),
                reversed: false,
            }),
            up,
        )
        .unwrap();
        assert_eq!(p.direction, [0.0, 0.0, 1.0]);
        assert_eq!(
            span(&p),
            Span {
                start: [0.0, 0.0, 0.0],
                vector: [0.0, 0.0, 10.0]
            }
        );
        assert_eq!(p.ends, [end("length", mm(10), [0.0, 0.0, 10.0], up, 1.0)]);
        let p = prism(
            Extent::Pad(PadExtent::OneSide {
                length: mm(10),
                reversed: true,
            }),
            up,
        )
        .unwrap();
        assert_eq!(span(&p).vector, [0.0, 0.0, -10.0]);
        assert_eq!(p.ends[0].offset, [0.0, 0.0, -10.0]);
        assert_eq!(p.ends[0].way, [0.0, 0.0, -1.0]);
        // A pocket's goes against the normal, the other way reversed.
        let p = prism(
            Extent::Pocket(PocketExtent::OneSide {
                length: mm(5),
                reversed: false,
            }),
            up,
        )
        .unwrap();
        assert_eq!(p.direction, [0.0, 0.0, -1.0]);
        assert_eq!(span(&p).vector, [0.0, 0.0, -5.0]);
        let p = prism(
            Extent::Pocket(PocketExtent::OneSide {
                length: mm(5),
                reversed: true,
            }),
            up,
        )
        .unwrap();
        assert_eq!(p.ends[0].offset, [0.0, 0.0, 5.0]);
        // Symmetric 10 on XZ, whose normal is −Y: from +5 to −5, its end at −5, each
        // millimetre the end moves two of the length.
        let p = prism(
            Extent::Pad(PadExtent::Symmetric { length: mm(10) }),
            [0.0, -1.0, 0.0],
        )
        .unwrap();
        assert_eq!(
            span(&p),
            Span {
                start: [0.0, 5.0, 0.0],
                vector: [0.0, -10.0, 0.0]
            }
        );
        assert_eq!(
            p.ends,
            [end(
                "length",
                mm(10),
                [0.0, -5.0, 0.0],
                [0.0, -1.0, 0.0],
                2.0
            )]
        );
        // A pocket's two sides, 10 and 4, on +Z: from +4 down 14; `length` ends at −10 along
        // the sweep, `length2` at +4 against it.
        let p = prism(
            Extent::Pocket(PocketExtent::TwoSides {
                length: mm(10),
                length2: mm(4),
            }),
            up,
        )
        .unwrap();
        assert_eq!(
            span(&p),
            Span {
                start: [0.0, 0.0, 4.0],
                vector: [0.0, 0.0, -14.0]
            }
        );
        assert_eq!(
            p.ends,
            [
                end("length", mm(10), [0.0, 0.0, -10.0], [0.0, 0.0, -1.0], 1.0),
                end("length2", mm(4), [0.0, 0.0, 4.0], up, 1.0),
            ]
        );
        // Through all: its way alone.
        let p = prism(
            Extent::Pocket(PocketExtent::ThroughAll { reversed: true }),
            up,
        )
        .unwrap();
        assert_eq!(
            p,
            Prism {
                direction: up,
                span: None,
                ends: Vec::new()
            }
        );
        // A turn has no prism.
        let angle = Angle::parse("90").unwrap();
        assert_eq!(
            prism(Extent::Turn(TurnExtent::Symmetric { angle }), up),
            None
        );
    }

    #[test]
    fn each_length_ends_at_its_prisms_start_or_end_and_every_zero_is_plus() {
        use crate::document::{PadExtent, PocketExtent};
        // On a tilted face's normal, whose products round: `length` ends where the prism
        // does, `length2` where it starts, and a symmetric end halfway, each exactly as the
        // prism's own arithmetic places it.
        let n = silico_protocol::vec3::normalize([1.0, -2.0, 3.0]);
        let mm = |text: &str| Length::parse(text).unwrap();
        let (l, l2) = (mm("10.3"), mm("0.7"));
        let forms = [
            Extent::Pad(PadExtent::OneSide {
                length: l,
                reversed: true,
            }),
            Extent::Pocket(PocketExtent::OneSide {
                length: l,
                reversed: false,
            }),
            Extent::Pad(PadExtent::Symmetric { length: l }),
            Extent::Pocket(PocketExtent::Symmetric { length: l }),
            Extent::Pad(PadExtent::TwoSides {
                length: l,
                length2: l2,
            }),
            Extent::Pocket(PocketExtent::TwoSides {
                length: l,
                length2: l2,
            }),
        ];
        for extent in forms {
            let p = prism(extent, n).unwrap();
            let Span { start, vector } = p.span.unwrap();
            for end in &p.ends {
                let (expected, way) = match (end.field, extent.type_name()) {
                    ("length2", _) => (start, p.direction.map(|c| -c)),
                    ("length", "symmetric") => {
                        (p.direction.map(|c| c * (l.mm() / 2.0)), p.direction)
                    }
                    _ => (p.direction.map(|c| c * l.mm()), p.direction),
                };
                assert_eq!(end.offset, expected, "{extent:?} {}", end.field);
                assert_eq!(end.way, way, "{extent:?} {}", end.field);
            }
            // The sweep's far end is `length`'s for one side and two sides (start + vector,
            // to within the one rounding of the sum).
            if extent.type_name() != "symmetric" {
                let far: [f64; 3] = std::array::from_fn(|k| start[k] + vector[k]);
                let at = p.ends[0].offset;
                for k in 0..3 {
                    assert!(
                        (far[k] - at[k]).abs() <= 1e-14,
                        "{extent:?}: {far:?} {at:?}"
                    );
                }
            }
            assert!(plus_zeros(start) && plus_zeros(vector) && plus_zeros(p.direction));
        }
        // On an origin plane, the zeros are +0 whichever way it goes.
        let p = prism(
            Extent::Pocket(PocketExtent::TwoSides {
                length: l,
                length2: l2,
            }),
            [0.0, 0.0, 1.0],
        )
        .unwrap();
        let Span { start, vector } = p.span.unwrap();
        assert!(plus_zeros(start) && plus_zeros(vector) && plus_zeros(p.direction));
        assert!(
            p.ends
                .iter()
                .all(|e| plus_zeros(e.offset) && plus_zeros(e.way))
        );
    }

    #[test]
    fn two_sides_sweep_their_sum_taken_in_nanometres_not_millimetres() {
        // 0.1 + 0.2 in f64 is 0.30000000000000004; 300000 nm is 0.3.
        use crate::document::PadExtent;
        let mm = |t: &str| Length::parse(t).unwrap();
        let p = prism(
            Extent::Pad(PadExtent::TwoSides {
                length: mm("0.1"),
                length2: mm("0.2"),
            }),
            [0.0, 0.0, 1.0],
        )
        .unwrap();
        assert_eq!(
            p.span.unwrap(),
            Span {
                start: [0.0, 0.0, -0.2],
                vector: [0.0, 0.0, 0.3]
            }
        );
    }
}
