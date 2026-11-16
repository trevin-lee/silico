//! Extending a sketch's line or arc at one end (docs/ports/sketch-edit.md, §7, decision 11;
//! docs/design/m5.0-app.md, open item 12): the core function the `sketch_extend` command
//! runs (open item 12c), as the other sketch edits' commands run `document::sketch_edit`. Its result is a function of
//! its inputs alone, the sketch as stored and the increment or point, as a drag's end is
//! (`drag`): a drag the app shows while the person picks the increment is only its preview.
//!
//! - **A line's end** moves by a `Length` along the line, through one move of the sketch
//!   worker's drag, as FreeCAD's `extend` moves it (`moveGeometry`): begun from the sketch
//!   as stored, moved once to the target, and ended, in one request held to a solve's
//!   limits and time, not a drag's (an extend isn't a drag), so its constraints win (a line with a
//!   length slides rather than grows, 1.1.3), and every curve sharing the end follows. The
//!   frame's values seed the rest of the edited sketch, and the moving end is written at
//!   the target, exactly; the edit's solve then settles them, as a drag's end does.
//! - **An arc's end** moves by an `Angle`, written: its point turned about the centre
//!   (the start back, the end on, so a positive angle lengthens the arc either way, as
//!   FreeCAD's `setRange(start − increment, end)` and `(start, end + increment)` do). The
//!   edit's solve then settles the rest, as FreeCAD's `solve()` after it does with the arc
//!   listed before its neighbours (FreeCAD's result hangs on that order; Silico's doesn't).
//!
//! Refused, as the port note recommends: a member that isn't a line or an arc, the wrong
//! kind of increment for it, an increment of nothing, and a result whose line or arc is
//! 1e-7 mm or less, flipped, or a whole turn or more. A line's end the drag can't move (its
//! constraints hold it, or the drag's frame isn't a success) is refused too, where FreeCAD
//! returns its solver's failure, or its success with nothing moved; so is an arc's end a
//! turn too small to move. Any result the gate would refuse (a neighbour the move
//! collapses, a radius rounding leaves at the floor) is refused as the edit's, naming what
//! the gate names. Every member the edit changes in place is `modified`: the points it
//! moves, a circle whose radius the drag changes, and a reference dimension it re-measures.
//!
//! An end can also go **to a point** on the curve's carrier (a line's line, an arc's
//! circle), placed there exactly: the line's one move goes to it, the arc's end is put at it.
//! [`next_crossing`] finds the point SolidWorks' Extend Entities extends to, the nearest
//! crossing with another curve past the end (open item 12c). No increment reaches it: a
//! `Length` is whole nanometres, an `Angle` whole nanodegrees, so an end moved by the
//! nearest would miss the other curve by more than the 1e-7 mm the gate and trim count as
//! touching. The edit adds nothing to hold the end there, as FreeCAD's App `extend` adds
//! nothing: an `on` or a shared point is the tool's or the caller's, as FreeCAD's Gui adds
//! its auto-constraints after the edit.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;

use silico_protocol::{DragBegin, PointOf, SolveStatus, check_drag_target};

use silico_protocol::distance;

use crate::document::sketch_edit::{Edited, Geo, crossings, end_ids};
use crate::document::{Angle, Coordinate, ElementId, Length, Sketch, SketchConstraint};
use crate::drag::{DragStop, checked_frame, checked_tags};
use crate::sketch::{MovedOnce, SketchSolver, Translation};
use crate::{Error, ErrorCode};

/// The gate's floor for a size: at most this is none (`LENGTH_TOLERANCE`, in millimetres).
const FLOOR: f64 = silico_protocol::LENGTH_TOLERANCE;

/// Which end of the curve moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Start,
    End,
}

/// How far the end moves: a line's along it, an arc's around its circle. Positive
/// lengthens, negative shortens. Or where it goes: `To` a point within 1e-7 mm of the
/// curve's carrier (a line's line, an arc's circle, as [`next_crossing`] measures it), as it
/// gives one: a line's one move goes there exactly (its constraints winning, as for a
/// length), an arc's end is put there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Length(Length),
    Angle(Angle),
    To([Coordinate; 2]),
}

/// Where a line's end goes: by a length along it, or to a point.
enum LineTo {
    By(Length),
    At([f64; 2]),
}

/// Where an arc's end goes: by an angle around its circle, or to a point.
enum ArcTo {
    By(Angle),
    At([f64; 2]),
}

/// Where `curve`'s end, extended, first meets another curve: [`next_crossing`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crossing {
    /// The curve the end meets.
    pub curve: ElementId,
    /// Where: that curve's end point as stored, where one is on the extended curve's
    /// carrier, else where the two carriers cross. [`By::To`] takes it as it is.
    pub at: [Coordinate; 2],
}

/// The suggestion for a line or arc an increment shortens too far.
const SHORTEN: &str = "shorten it by less";
/// For a line its constraints pull back past its other end, or onto it.
const PULLED: &str = "change the constraints that pull its end back, or extend it by less";

/// Extends `curve`'s `end` of `sketch` (a gated sketch, as stored) by `by`, a line's
/// through `solver`'s drag. The edit's points moved are `modified`; nothing is added or
/// removed. Refused, naming the parameter: see the module's notes.
pub fn extend(
    sketch: &Sketch,
    curve: &ElementId,
    end: End,
    by: By,
    solver: &mut dyn SketchSolver,
) -> Result<Edited, Error> {
    extend_under(sketch, curve, end, by, solver, &DragStop::new())
}

/// [`extend`], its line's move run under `stop` (docs/design/m5.0-app.md, open item 12c):
/// the solver's stopper taken by `stop`, so the command's `cancel` stops the move, which is
/// then refused as `cancelled`.
pub fn extend_under(
    sketch: &Sketch,
    curve: &ElementId,
    end: End,
    by: By,
    solver: &mut dyn SketchSolver,
    stop: &DragStop,
) -> Result<Edited, Error> {
    stop.run(solver, |solver| extended(sketch, curve, end, by, solver))
}

/// [`extend`] on `solver`, readied for its move by its caller's stop.
fn extended(
    sketch: &Sketch,
    curve: &ElementId,
    end: End,
    by: By,
    solver: &mut dyn SketchSolver,
) -> Result<Edited, Error> {
    let _span = tracing::info_span!("sketch_extend", curve = curve.as_str()).entered();
    if let Some(line) = sketch.lines.iter().find(|l| &l.id == curve) {
        let to = match by {
            By::Length(by) => LineTo::By(by),
            By::To([x, y]) => LineTo::At([x.mm(), y.mm()]),
            By::Angle(_) => {
                return Err(refused(
                    format!("line {curve} extends by a length, not an angle"),
                    "by",
                    "give the increment in millimetres",
                )
                .with_elements([curve.to_string()]));
            }
        };
        let (moving, other) = match end {
            End::Start => (&line.from, &line.to),
            End::End => (&line.to, &line.from),
        };
        return extend_line(sketch, curve, moving, other, to, solver);
    }
    if let Some(arc) = sketch.arcs.iter().find(|a| &a.id == curve) {
        let to = match by {
            By::Angle(by) => ArcTo::By(by),
            By::To([x, y]) => ArcTo::At([x.mm(), y.mm()]),
            By::Length(_) => {
                return Err(refused(
                    format!("arc {curve} extends by an angle, not a length"),
                    "by",
                    "give the increment in degrees",
                )
                .with_elements([curve.to_string()]));
            }
        };
        return extend_arc(sketch, curve, &arc.center, &arc.start, &arc.end, end, to);
    }
    Err(not_extendable(sketch, curve))
}

/// Where `curve`'s `end` of `sketch` (a gated sketch, as stored), extended, first meets
/// another of its curves, as SolidWorks' Extend Entities extends it (docs/design/m5.0-app.md,
/// open item 12c); none if it meets none. A line goes on along its line, an arc around its
/// circle short of its other end. What counts is what trim cuts at (`sketch_edit::trim`,
/// decision 6): every other curve, construction too, at a crossing of the two carriers on
/// the other curve, a tangent contact within 1e-7 mm, or an end of the other curve within
/// 1e-7 mm of the carrier, where its stored point is the answer, so the extended end lands
/// on it exactly. Past the end means farther than 1e-7 mm from it (and, for an arc, from its
/// other end), and within the sketch's reach. A curve the extension runs along to the point
/// (within 1e-7 mm of it halfway there) isn't met there: a collinear neighbour, or a round
/// the curve runs into nearly tangent. Nor is a curve built on the moving end's point, which
/// the edit moves with it: one that starts or ends there, or a round about it. The nearest
/// wins, a line's by distance, an arc's by angle; at the same distance, the least ID. Found
/// from the stored coordinates in closed form, with the libm shim's math, as trim finds its
/// cuts. Refused as [`extend`] refuses a member that isn't a line or an arc, and an arc
/// whose ends are at one angle from its centre, which has no circle to go around.
pub fn next_crossing(
    sketch: &Sketch,
    curve: &ElementId,
    end: End,
) -> Result<Option<Crossing>, Error> {
    let _span = tracing::info_span!("sketch_next_crossing", curve = curve.as_str()).entered();
    let geo = extendable(sketch, curve)?;
    // The end that moves, and the other.
    let (moving, fixed) = match (&geo, end) {
        (Geo::Line { from, to }, End::Start)
        | (
            Geo::Arc {
                start: from,
                end: to,
                ..
            },
            End::Start,
        ) => (*from, *to),
        (Geo::Line { from, to }, End::End)
        | (
            Geo::Arc {
                start: from,
                end: to,
                ..
            },
            End::End,
        ) => (*to, *from),
        (Geo::Circle { .. }, _) => return Err(not_extendable(sketch, curve)),
    };
    // How far past the end a point on the carrier is (a line's distance, an arc's angle),
    // and the place halfway there; none if it isn't past it.
    let ahead = |xy: [f64; 2]| -> Option<(f64, [f64; 2])> {
        if distance(xy, moving) <= FLOOR || distance(xy, fixed) <= FLOOR {
            return None;
        }
        match &geo {
            Geo::Line { .. } => {
                let (d, e) = (
                    [moving[0] - fixed[0], moving[1] - fixed[1]],
                    [xy[0] - moving[0], xy[1] - moving[1]],
                );
                let halfway = [(moving[0] + xy[0]) / 2.0, (moving[1] + xy[1]) / 2.0];
                (d[0] * e[0] + d[1] * e[1] > 0.0).then(|| (distance(xy, moving), halfway))
            }
            Geo::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                ..
            } => {
                let angle = libm::atan2(xy[1] - center[1], xy[0] - center[0]);
                let (on, half) = match end {
                    End::Start => {
                        let on = (start_angle - angle).rem_euclid(TAU);
                        (on, start_angle - on / 2.0)
                    }
                    End::End => {
                        let on = (angle - end_angle).rem_euclid(TAU);
                        (on, end_angle + on / 2.0)
                    }
                };
                let halfway = [
                    center[0] + radius * libm::cos(half),
                    center[1] + radius * libm::sin(half),
                ];
                (on > 0.0 && on < TAU - (end_angle - start_angle)).then_some((on, halfway))
            }
            Geo::Circle { .. } => None,
        }
    };
    // A curve built on the moving end's point (an arc starting at a line's end, a circle
    // about it) moves with it, so it isn't met.
    let ends: Vec<ElementId> = end_ids(sketch, curve).into_iter().cloned().collect();
    let Some(moving_id) = (match end {
        End::Start => ends.first(),
        End::End => ends.get(1),
    }) else {
        return Err(not_extendable(sketch, curve));
    };
    let free = |points: &[&ElementId]| !points.contains(&moving_id);
    let others = sketch
        .lines
        .iter()
        .filter(|l| free(&[&l.from, &l.to]))
        .map(|l| &l.id)
        .chain(
            sketch
                .circles
                .iter()
                .filter(|c| free(&[&c.center]))
                .map(|c| &c.id),
        )
        .chain(
            sketch
                .arcs
                .iter()
                .filter(|a| free(&[&a.center, &a.start, &a.end]))
                .map(|a| &a.id),
        )
        .filter(|id| *id != curve);
    let mut nearest: Option<(f64, &ElementId, [f64; 2])> = None;
    for id in others {
        let Some(other) = Geo::of(sketch, id) else {
            continue;
        };
        // Its ends on the carrier, as stored; then the carriers' crossings on it, but where
        // one of those ends is.
        let mut found: Vec<[f64; 2]> = end_ids(sketch, id)
            .iter()
            .filter_map(|e| point(sketch, e).ok())
            .filter(|xy| geo.off(*xy) <= FLOOR)
            .collect();
        for xy in crossings(&geo, &other) {
            if other.spans(other.project(xy)) && !found.iter().any(|f| distance(*f, xy) <= FLOOR) {
                found.push(xy);
            }
        }
        for xy in found {
            if !xy.iter().all(|v| silico_protocol::within_reach(*v)) {
                continue;
            }
            let Some((far, halfway)) = ahead(xy) else {
                continue;
            };
            // A curve the extension runs along to the point (within 1e-7 mm of it halfway
            // there, on it) isn't met there: a collinear neighbour, or a round the curve
            // runs into nearly tangent, whose contact is a few microns on.
            if other.off(halfway) <= FLOOR && other.spans(other.project(halfway)) {
                continue;
            }
            let nearer = nearest.as_ref().is_none_or(|(best, best_id, _)| {
                far.total_cmp(best).then_with(|| id.cmp(best_id)).is_lt()
            });
            if nearer {
                nearest = Some((far, id, xy));
            }
        }
    }
    Ok(nearest.and_then(|(_, id, xy)| {
        Some(Crossing {
            curve: id.clone(),
            at: [Coordinate::new(xy[0])?, Coordinate::new(xy[1])?],
        })
    }))
}

/// The refusal of a next curve for `curve`'s `end` of `sketch` to meet, where there's none:
/// `sketch_extend`'s, and Extend Entities' words for its hover (`sketch_tools::extend_preview`).
pub(crate) fn no_next_curve(sketch: &Sketch, curve: &ElementId, end: End) -> Error {
    let (noun, carrier, increment) = if sketch.arcs.iter().any(|a| &a.id == curve) {
        ("arc", "circle", "an angle")
    } else {
        ("line", "line", "a length")
    };
    let which = match end {
        End::Start => "start",
        End::End => "end",
    };
    Error::new(
        ErrorCode::InvalidInput,
        format!("{noun} {curve} meets no other curve past its {which}"),
    )
    .with_elements([curve.to_string()])
    .with_parameter("by")
    .with_cause(format!(
        "the next curve is the nearest that the {noun}, extended, crosses or touches past its {which}, as trim finds its cuts"
    ))
    .with_suggestion(format!(
        "extend it by {increment}, or to a point on its {carrier}"
    ))
}

/// `sketch`'s line or arc `curve`, as [`next_crossing`] and Extend Entities' hover take it
/// (`sketch_tools::extend_end`); refused as [`extend`] refuses a member that isn't a line or
/// an arc, and an arc whose ends are at one angle from its centre, which has no circle to go
/// around.
pub(crate) fn extendable(sketch: &Sketch, curve: &ElementId) -> Result<Geo, Error> {
    match Geo::of(sketch, curve) {
        Some(Geo::Circle { .. }) => Err(not_extendable(sketch, curve)),
        Some(geo) => Ok(geo),
        None if sketch.arcs.iter().any(|a| &a.id == curve) => Err(no_sweep(curve)),
        None => Err(not_extendable(sketch, curve)),
    }
}

/// Refuses a line whose ends are at one place: it has no direction to go on in.
pub(crate) fn no_length(curve: &ElementId) -> Error {
    refused(
        format!("line {curve} has no length, so it has no direction to extend"),
        "curve",
        "move one of its ends off the other",
    )
    .with_elements([curve.to_string()])
}

/// Refuses `curve` as no line or arc of `sketch`.
fn not_extendable(sketch: &Sketch, curve: &ElementId) -> Error {
    let what = sketch
        .elements()
        .find(|(id, _)| *id == curve)
        .map(|(_, kind)| kind.noun());
    match what {
        Some(noun) => refused(
            format!("{noun} {curve} can't be extended: only a line or an arc can"),
            "curve",
            "name a line or an arc",
        )
        .with_elements([curve.to_string()]),
        None => refused(
            format!("the sketch has no member {curve}"),
            "curve",
            "name a line or an arc of this sketch by its ID",
        ),
    }
}

/// A line's `moving` end, `other` its other, moved along it through the drag: `by` a
/// length, or to a point on its line.
fn extend_line(
    sketch: &Sketch,
    curve: &ElementId,
    moving: &ElementId,
    other: &ElementId,
    to: LineTo,
    solver: &mut dyn SketchSolver,
) -> Result<Edited, Error> {
    let (p, q) = (point(sketch, moving)?, point(sketch, other)?);
    let length = libm::hypot(p[0] - q[0], p[1] - q[1]);
    // The drag's target, how the messages say it, and what to suggest if the line comes
    // back too short or flipped.
    let (target, said, pulled) = match to {
        LineTo::By(by) => {
            let longer = length + by.mm();
            if by.nm() == 0 {
                return Err(nothing(curve));
            }
            if longer <= FLOOR {
                return Err(too_short(curve, "line", longer, SHORTEN));
            }
            // FreeCAD's target, the new length along the line from the other end
            // (`extend`), worked from the end itself, so an increment the line's numbers
            // hold lands exactly.
            let along = by.mm() / length;
            let target = [p[0] + (p[0] - q[0]) * along, p[1] + (p[1] - q[1]) * along];
            let pulled = if by.nm() < 0 { SHORTEN } else { PULLED };
            (target, format!("by {by} mm"), pulled)
        }
        LineTo::At(target) => {
            let said = to_point(target);
            if let Err(invalid) = check_drag_target(target) {
                return Err(past_reach(curve, "line", &said, &invalid.message));
            }
            // Measured as `next_crossing` measures it (trim's carrier), so a point it gives
            // is taken.
            let off = Geo::of(sketch, curve).map_or(f64::NAN, |geo| geo.off(target));
            if off.is_nan() || off > FLOOR {
                return Err(off_carrier(curve, "line", &said, off));
            }
            // By value: a zero of either sign is the same place.
            if target[0] == p[0] && target[1] == p[1] {
                return Err(nothing_there(curve));
            }
            let longer = libm::hypot(target[0] - q[0], target[1] - q[1]);
            let same_way =
                (target[0] - q[0]) * (p[0] - q[0]) + (target[1] - q[1]) * (p[1] - q[1]) > 0.0;
            if longer <= FLOOR || !same_way {
                return Err(too_short(
                    curve,
                    "line",
                    longer,
                    "give a point farther from its other end, the way the line runs",
                ));
            }
            (target, said, PULLED)
        }
    };
    if let Err(invalid) = check_drag_target(target) {
        return Err(past_reach(curve, "line", &said, &invalid.message));
    }
    let translation = Translation::new(sketch)?;
    let Some(geometry) = translation.point_geometry(moving) else {
        return Err(Error::new(
            ErrorCode::Internal,
            format!("the sketch's translation has no geometry for point {moving}"),
        )
        .with_suggestion(crate::worker::SILICO_BUG));
    };
    let begin = DragBegin::point(translation.system.clone(), PointOf::Point(geometry));
    // One move, held to a solve's limits, not a drag's: an extend isn't a drag.
    if let Err(invalid) = begin.check_move() {
        return Err(refused(
            format!(
                "line {curve}'s end {moving} can't be moved: {}",
                invalid.message
            ),
            "curve",
            "check the sketch with the gate first",
        )
        .with_elements([curve.to_string(), moving.to_string()]));
    }
    // The worker's own refusal of the move (a frame out of reach, past its time, a crash),
    // said as this edit's: naming the line, its end, and the increment to change.
    let once = solver.move_once(begin.clone(), target).map_err(|error| {
        let reach = error.code() == ErrorCode::KernelFailed;
        let error = error
            .with_elements([curve.to_string(), moving.to_string()])
            .with_parameter("by");
        if reach {
            error.with_suggestion("extend it by less, or change what moves with its end")
        } else {
            error
        }
    })?;
    let moved = match once {
        MovedOnce::Moved(moved) => moved,
        MovedOnce::Conflicting(tags) => {
            if let Err(error) = checked_tags(&begin.system, &tags) {
                solver.distrust();
                return Err(error);
            }
            if tags.is_empty() {
                solver.distrust();
                return Err(Error::new(
                    ErrorCode::ProtocolError,
                    "the sketch worker's one move was left unmade for constraints in conflict, naming none",
                )
                .with_suggestion("extend it again: its worker is started again"));
            }
            let ids: Vec<String> = tags
                .iter()
                .filter_map(|&t| translation.constraint_id(t).map(|id| id.to_string()))
                .collect();
            return Err(Error::new(
                ErrorCode::OverConstrained,
                format!("the sketch's constraints conflict, so line {curve} can't be extended"),
            )
            .with_elements(ids)
            .with_suggestion("remove or change one of the constraints named"));
        }
    };
    if let Err(error) = checked_frame(&begin.part(), &moved) {
        solver.distrust();
        return Err(error);
    }
    let values = match (moved.status, moved.values) {
        (SolveStatus::Success, Some(values)) => values,
        (SolveStatus::SolvedInvalid, _) => {
            // The worker withholds a frame it can't build: what the gate names of the end
            // put at the target says which curve that is (a neighbour collapsed).
            let mut elements = vec![curve.to_string(), moving.to_string()];
            let mut placed = sketch.clone();
            let why = match place(&mut placed, moving, target)
                .ok()
                .and_then(|()| crate::document::sketch_problems(&placed).into_iter().next())
            {
                Some(problem) => {
                    for e in problem.elements() {
                        if !elements.contains(e) {
                            elements.push(e.clone());
                        }
                    }
                    format!(": at the target, {}", problem.message())
                }
                None => String::new(),
            };
            return Err(Error::new(
                ErrorCode::InvalidResult,
                format!("line {curve}'s end {moving} can't move {said}: the drag reaches geometry that can't be built{why}"),
            )
            .with_elements(elements)
            .with_parameter("by")
            .with_suggestion("extend it by less, or change what moves with its end"));
        }
        (status, _) => {
            return Err(Error::new(
                ErrorCode::InvalidResult,
                format!("line {curve}'s end {moving} can't move {said}: the drag's solve ended {status:?}"),
            )
            .with_elements([curve.to_string(), moving.to_string()])
            .with_parameter("by")
            .with_suggestion("remove or change the constraints that hold the line's end"));
        }
    };
    let mut edited = translation.stored(&values)?;
    // The line the drag left: longer than the floor, and running the way it did.
    let (p1, q1) = (point(&edited, moving)?, point(&edited, other)?);
    let now = libm::hypot(p1[0] - q1[0], p1[1] - q1[1]);
    let same_way = (p1[0] - q1[0]) * (p[0] - q[0]) + (p1[1] - q1[1]) * (p[1] - q[1]) > 0.0;
    if now <= FLOOR || !same_way {
        return Err(too_short(curve, "line", now, pulled));
    }
    let modified = changed(sketch, &edited);
    if !modified.iter().any(|id| id == moving) {
        return Err(Error::new(
            ErrorCode::InvalidResult,
            format!(
                "extending line {curve} {said} changes nothing: its constraints hold its end {moving} where it is"
            ),
        )
        .with_elements([curve.to_string(), moving.to_string()])
        .with_parameter("by")
        .with_suggestion("remove or change the constraints that hold the line's end"));
    }
    // The drag's frame seeds the rest of the sketch; the end is written where the request
    // named, exactly (PlaneGCS's SQP stops on its step, never checking the pin's residual,
    // so the drag leaves it short). The command's solve then settles it, moving it back only
    // where constraints pull it.
    place(&mut edited, moving, target).map_err(|()| {
        past_reach(
            curve,
            "line",
            &said,
            "its target isn't a coordinate a document holds",
        )
    })?;
    gated(&edited, curve)?;
    Ok(Edited {
        modified,
        sketch: edited,
        added: Vec::new(),
        removed: Vec::new(),
    })
}

/// An arc's `end` moved around its circle: turned `by` an angle about its centre (its
/// start back, its end on), or put at a point on its circle.
fn extend_arc(
    sketch: &Sketch,
    curve: &ElementId,
    center: &ElementId,
    start: &ElementId,
    end_point: &ElementId,
    end: End,
    to: ArcTo,
) -> Result<Edited, Error> {
    let (c, s, e) = (
        point(sketch, center)?,
        point(sketch, start)?,
        point(sketch, end_point)?,
    );
    let angle_of = |p: [f64; 2]| libm::atan2(p[1] - c[1], p[0] - c[0]);
    let sweep = (angle_of(e) - angle_of(s)).rem_euclid(TAU);
    let (moving, p) = match end {
        End::Start => (start, s),
        End::End => (end_point, e),
    };
    let radius = libm::hypot(p[0] - c[0], p[1] - c[1]);
    // The arc's new sweep, its moved end, how the messages say it, and what to suggest if
    // it's left too short.
    let (swept, turned, said, shorter) = match to {
        ArcTo::By(by) => {
            if by.ndeg() == 0 {
                return Err(nothing(curve));
            }
            let turn = by.degrees().to_radians();
            let swept = sweep + turn;
            if swept <= 0.0 || radius * swept <= FLOOR {
                return Err(too_short(curve, "arc", radius * swept.max(0.0), SHORTEN));
            }
            let about = if end == End::Start { -turn } else { turn };
            let (sin, cos) = (libm::sin(about), libm::cos(about));
            let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
            let turned = [c[0] + dx * cos - dy * sin, c[1] + dx * sin + dy * cos];
            (swept, turned, format!("by {by}°"), SHORTEN)
        }
        ArcTo::At(target) => {
            let said = to_point(target);
            if !target.iter().all(|v| silico_protocol::within_reach(*v)) {
                return Err(past_reach(curve, "arc", &said, ""));
            }
            // Its circle as `next_crossing` measures it (trim's carrier, from the arc's
            // points), so a point it gives is taken.
            let Some(geo @ Geo::Arc { radius, .. }) = Geo::of(sketch, curve) else {
                return Err(no_sweep(curve));
            };
            let off = geo.off(target);
            if off.is_nan() || off > FLOOR {
                return Err(off_carrier(curve, "circle", &said, off));
            }
            // By value: a zero of either sign is the same place.
            if target[0] == p[0] && target[1] == p[1] {
                return Err(nothing_there(curve));
            }
            let swept = match end {
                End::Start => (angle_of(e) - angle_of(target)).rem_euclid(TAU),
                End::End => (angle_of(target) - angle_of(s)).rem_euclid(TAU),
            };
            let shorter = "give a point farther around its circle from its other end";
            if radius * swept <= FLOOR {
                return Err(too_short(curve, "arc", radius * swept, shorter));
            }
            (swept, target, said, shorter)
        }
    };
    let whole_turn = || {
        refused(
            format!(
                "extending arc {curve} {said} would sweep it {}°, a whole turn or more, or its ends within 1e-7 mm",
                swept.to_degrees()
            ),
            "by",
            "extend it by less, or draw a circle",
        )
        .with_elements([curve.to_string()])
    };
    if swept >= TAU {
        return Err(whole_turn());
    }
    // Its ends apart by more than the floor, as the gate has them: near a whole turn, or
    // nearly nothing.
    let other = if moving == start { e } else { s };
    if libm::hypot(turned[0] - other[0], turned[1] - other[1]) <= FLOOR {
        return Err(if swept > std::f64::consts::PI {
            whole_turn()
        } else {
            too_short(curve, "arc", radius * swept, shorter)
        });
    }
    // Its radius, as rounding leaves the turned point, above the floor too.
    let left = libm::hypot(turned[0] - c[0], turned[1] - c[1]);
    if left <= FLOOR {
        return Err(refused(
            format!(
                "extending arc {curve} {said} would leave its moved end {left} mm from its centre: an arc's radius is above 1e-7 mm"
            ),
            "by",
            "extend it by another angle, or draw the arc larger",
        )
        .with_elements([curve.to_string()]));
    }
    let coordinate = |v: f64| {
        Coordinate::new(v)
            .filter(|_| silico_protocol::within_reach(v))
            .ok_or_else(|| past_reach(curve, "arc", &said, ""))
    };
    let (x, y) = (coordinate(turned[0])?, coordinate(turned[1])?);
    let mut edited = sketch.clone();
    for p in &mut edited.points {
        if &p.id == moving {
            (p.x, p.y) = (x, y);
        }
    }
    let modified = changed(sketch, &edited);
    // A turn too small to move the point, bit for bit, changes nothing.
    if modified.is_empty() {
        return Err(nothing(curve));
    }
    gated(&edited, curve)?;
    Ok(Edited {
        modified,
        sketch: edited,
        added: Vec::new(),
        removed: Vec::new(),
    })
}

/// Point `id`'s place, in millimetres.
fn point(sketch: &Sketch, id: &ElementId) -> Result<[f64; 2], Error> {
    sketch
        .points
        .iter()
        .find(|p| &p.id == id)
        .map(|p| [p.x.mm(), p.y.mm()])
        .ok_or_else(|| {
            Error::new(
                ErrorCode::Internal,
                format!("the sketch's curve names point {id}, which it hasn't"),
            )
            .with_suggestion("check the sketch with the gate first")
        })
}

/// The members `edited` changes in place, in the format's order: the points it moves, bit
/// for bit, then the circles whose radius it changes, then the constraints it rewrites (a
/// reference dimension re-measured), each by ID.
pub(crate) fn changed(sketch: &Sketch, edited: &Sketch) -> Vec<ElementId> {
    let points: BTreeSet<(&ElementId, u64, u64)> = sketch
        .points
        .iter()
        .map(|p| (&p.id, p.x.mm().to_bits(), p.y.mm().to_bits()))
        .collect();
    let radii: BTreeSet<(&ElementId, u64)> = sketch
        .circles
        .iter()
        .map(|c| (&c.id, c.radius.mm().to_bits()))
        .collect();
    let mut moved: Vec<ElementId> = edited
        .points
        .iter()
        .filter(|p| !points.contains(&(&p.id, p.x.mm().to_bits(), p.y.mm().to_bits())))
        .map(|p| p.id.clone())
        .collect();
    moved.sort();
    let mut grown: Vec<ElementId> = edited
        .circles
        .iter()
        .filter(|c| !radii.contains(&(&c.id, c.radius.mm().to_bits())))
        .map(|c| c.id.clone())
        .collect();
    grown.sort();
    let before: BTreeMap<&ElementId, &SketchConstraint> =
        sketch.constraints.iter().map(|c| (&c.id, c)).collect();
    let mut rewritten: Vec<ElementId> = edited
        .constraints
        .iter()
        .filter(|c| before.get(&c.id) != Some(c))
        .map(|c| c.id.clone())
        .collect();
    rewritten.sort();
    moved.into_iter().chain(grown).chain(rewritten).collect()
}

/// Point `id` of `sketch` put at `xy`.
fn place(sketch: &mut Sketch, id: &ElementId, xy: [f64; 2]) -> Result<(), ()> {
    let (Some(x), Some(y)) = (Coordinate::new(xy[0]), Coordinate::new(xy[1])) else {
        return Err(());
    };
    for p in &mut sketch.points {
        if &p.id == id {
            (p.x, p.y) = (x, y);
        }
    }
    Ok(())
}

/// Refuses an edited sketch the gate would: as the edit's refusal, naming what the gate
/// names, with the curve extended.
fn gated(edited: &Sketch, curve: &ElementId) -> Result<(), Error> {
    crate::document::refused_by_gate(
        edited,
        &format!("extending {curve} there"),
        vec![curve.to_string()],
        "by",
        "extend it by less, or change what moves with its end",
    )
}

fn refused(message: String, parameter: &str, suggestion: &str) -> Error {
    Error::new(ErrorCode::InvalidInput, message)
        .with_parameter(parameter)
        .with_suggestion(suggestion)
}

fn nothing(curve: &ElementId) -> Error {
    refused(
        format!("extending {curve} by 0 changes nothing"),
        "by",
        "give an increment other than 0",
    )
    .with_elements([curve.to_string()])
}

fn too_short(curve: &ElementId, noun: &str, size: f64, suggestion: &str) -> Error {
    let article = if noun == "arc" { "an" } else { "a" };
    refused(
        format!(
            "extending {noun} {curve} that far would leave it {size} mm long or flipped: {article} {noun} must be longer than 1e-7 mm and keep its direction"
        ),
        "by",
        suggestion,
    )
    .with_elements([curve.to_string()])
}

/// Refuses a move `said` that takes the end past the sketch's reach, `why` the reach's own
/// words, if any.
fn past_reach(curve: &ElementId, noun: &str, said: &str, why: &str) -> Error {
    let why = if why.is_empty() {
        String::new()
    } else {
        format!(": {why}")
    };
    refused(
        format!("extending {noun} {curve} {said} would take its end past the sketch's reach{why}"),
        "by",
        "extend it by less",
    )
    .with_elements([curve.to_string()])
}

/// Refuses an arc whose ends are at one angle from its centre: it sweeps nothing, so it has
/// no circle to go around (the solve's translation refuses it too).
fn no_sweep(curve: &ElementId) -> Error {
    refused(
        format!(
            "arc {curve}'s start and end are at one angle from its centre, so it has no circle to extend around"
        ),
        "curve",
        "move one of its ends around its centre",
    )
    .with_elements([curve.to_string()])
}

fn nothing_there(curve: &ElementId) -> Error {
    refused(
        format!("extending {curve} to where its end is changes nothing"),
        "by",
        "give a point other than its end",
    )
    .with_elements([curve.to_string()])
}

/// Refuses a point `off` mm from the curve's `carrier` (its line or its circle).
fn off_carrier(curve: &ElementId, carrier: &str, said: &str, off: f64) -> Error {
    let noun = if carrier == "line" { "line" } else { "arc" };
    refused(
        format!(
            "{noun} {curve} can't extend {said}: the point is {off} mm from its {carrier}, farther than 1e-7 mm"
        ),
        "by",
        &format!("give a point on its {carrier}, such as where it meets another curve"),
    )
    .with_elements([curve.to_string()])
}

/// "to (x, y)", as a point is said in the messages.
fn to_point(xy: [f64; 2]) -> String {
    format!("to ({}, {})", xy[0], xy[1])
}
