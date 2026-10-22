//! What a pad, pocket, revolve, or groove's rebuild refuses that the gate can't see
//! (docs/design/m3.6-sketch-features.md, S5 to S7), as pure functions of what the rebuild
//! already has, for recompute to call (M3.6's step iv): from the solved sketch, before the
//! kernel is asked for anything (decision 2), a revolve's or groove's axis of no length (S6),
//! and a pad or revolve of more than one region (decision 3); and from the built result, more
//! than one solid (D2), or a volume that shows the feature changed nothing (S7). And M2's
//! booleans' one-solid rule (D2), which keeps every base one solid. Recompute asks the
//! result's checks of each feature when its outcome is decided, from its solid's stored
//! measures, however the solid was had (made, kept, or read from the cache).

use silico_protocol::{EndAtStart, MEASURE_PRECISION, format_number, no_distance};

use crate::document::{Axis, Checked, FeatureId, Operation, PROBLEMS_LISTED, SweepFeature};
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
    let features = &document.document().features;
    let found = features.iter().find(|f| &f.id == feature).ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("the document has no feature {feature}"),
        )
    })?;
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
    let line = features
        .iter()
        .find(|f| &f.id == sketch)
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
    let found = document
        .document()
        .features
        .iter()
        .find(|f| &f.id == feature);
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
    match silico_protocol::end_at_start(slit_at, farthest, turned) {
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
            // if no point is off the axis and the slit is measured at the farthest, those
            // reaching it. `far` names curves only in that fallback: a curve whose farthest
            // point merely lies at `at` beside a measured corner isn't named, since its edges
            // running out from the axis share the axis vertex and are harmless, and its
            // extreme sitting at `at` is a coincidence, not where the rule measured.
            let off_axis = near.iter().any(|&(d, _)| d > tolerance);
            let there: std::collections::BTreeSet<String> = (if off_axis { &near } else { &far })
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

/// [`check_result`], its feature and base by ID.
fn judged(
    id: &str,
    kind: SweepFeature,
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
                if kind.adds() {
                    "OCCT lost both shapes: move the sketch slightly"
                } else {
                    "move the sketch or shorten the cut, so it leaves some of its base"
                },
            );
        }
        1 => {}
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
                &format!(
                    "make the {word} larger, or make it on a smaller part; if it has a thin wall flush with a face of the base, OCCT can lose it (docs/ports/sketch-features.md): thicken it"
                ),
            );
        }
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
