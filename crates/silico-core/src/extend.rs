//! Extending a sketch's line or arc at one end (docs/ports/sketch-edit.md, §7, decision 11;
//! docs/design/m5.0-app.md, open item 12, pending Trevin): a core function, with no command
//! yet, as the other sketch edits are (`document::sketch_edit`). Its result is a function of
//! its inputs alone, the sketch as stored and the increment, as a drag's end is (`drag`):
//! a drag the app shows while the person picks the increment is only its preview.
//!
//! - **A line's end** moves by a `Length` along the line, through one move of the sketch
//!   worker's drag, as FreeCAD's `extend` moves it (`moveGeometry`): begun from the sketch
//!   as stored, moved once to the target, and ended, in one request held to a solve's
//!   limits and time, not a drag's (an extend isn't a drag), so its constraints win (a line with a
//!   length slides rather than grows, 1.1.3), and every curve sharing the end follows. The
//!   frame's values are the edited sketch's coordinates, which the edit's solve then
//!   settles, as a drag's end does.
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

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;

use silico_protocol::{DragBegin, PointOf, SolveStatus, check_drag_target};

use crate::document::sketch_edit::Edited;
use crate::document::{Angle, Coordinate, ElementId, Length, Sketch, SketchConstraint};
use crate::drag::{checked_frame, checked_tags};
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
/// lengthens, negative shortens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Length(Length),
    Angle(Angle),
}

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
    let _span = tracing::info_span!("sketch_extend", curve = curve.as_str()).entered();
    if let Some(line) = sketch.lines.iter().find(|l| &l.id == curve) {
        let By::Length(by) = by else {
            return Err(refused(
                format!("line {curve} extends by a length, not an angle"),
                "by",
                "give the increment in millimetres",
            )
            .with_elements([curve.to_string()]));
        };
        let (moving, other) = match end {
            End::Start => (&line.from, &line.to),
            End::End => (&line.to, &line.from),
        };
        return extend_line(sketch, curve, moving, other, by, solver);
    }
    if let Some(arc) = sketch.arcs.iter().find(|a| &a.id == curve) {
        let By::Angle(by) = by else {
            return Err(refused(
                format!("arc {curve} extends by an angle, not a length"),
                "by",
                "give the increment in degrees",
            )
            .with_elements([curve.to_string()]));
        };
        return extend_arc(sketch, curve, &arc.center, &arc.start, &arc.end, end, by);
    }
    let what = sketch
        .elements()
        .find(|(id, _)| *id == curve)
        .map(|(_, kind)| kind.noun());
    Err(match what {
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
    })
}

/// A line's `moving` end, `other` its other, moved `by` along it through the drag.
fn extend_line(
    sketch: &Sketch,
    curve: &ElementId,
    moving: &ElementId,
    other: &ElementId,
    by: Length,
    solver: &mut dyn SketchSolver,
) -> Result<Edited, Error> {
    let (p, q) = (point(sketch, moving)?, point(sketch, other)?);
    let length = libm::hypot(p[0] - q[0], p[1] - q[1]);
    let longer = length + by.mm();
    if by.nm() == 0 {
        return Err(nothing(curve));
    }
    if longer <= FLOOR {
        return Err(too_short(curve, "line", longer, true));
    }
    // FreeCAD's target, the new length along the line from the other end (`extend`),
    // worked from the end itself, so an increment the line's numbers hold lands exactly.
    let along = by.mm() / length;
    let target = [p[0] + (p[0] - q[0]) * along, p[1] + (p[1] - q[1]) * along];
    if let Err(invalid) = check_drag_target(target) {
        return Err(refused(
            format!(
                "extending line {curve} by {by} mm would take its end past the sketch's reach: {}",
                invalid.message
            ),
            "by",
            "extend it by less",
        )
        .with_elements([curve.to_string()]));
    }
    let translation = Translation::new(sketch)?;
    let Some(geometry) = translation.point_geometry(moving) else {
        return Err(Error::new(
            ErrorCode::Internal,
            format!("the sketch's translation has no geometry for point {moving}"),
        )
        .with_suggestion(crate::worker::SILICO_BUG));
    };
    let begin = DragBegin {
        system: translation.system.clone(),
        point: PointOf::Point(geometry),
    };
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
    solver.stopper()?;
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
                format!("line {curve}'s end {moving} can't move by {by} mm: the drag reaches geometry that can't be built{why}"),
            )
            .with_elements(elements)
            .with_parameter("by")
            .with_suggestion("extend it by less, or change what moves with its end"));
        }
        (status, _) => {
            return Err(Error::new(
                ErrorCode::InvalidResult,
                format!("line {curve}'s end {moving} can't move by {by} mm: the drag's solve ended {status:?}"),
            )
            .with_elements([curve.to_string(), moving.to_string()])
            .with_parameter("by")
            .with_suggestion("remove or change the constraints that hold the line's end"));
        }
    };
    let edited = translation.stored(&values)?;
    // The line the drag left: longer than the floor, and running the way it did.
    let (p1, q1) = (point(&edited, moving)?, point(&edited, other)?);
    let now = libm::hypot(p1[0] - q1[0], p1[1] - q1[1]);
    let same_way = (p1[0] - q1[0]) * (p[0] - q[0]) + (p1[1] - q1[1]) * (p[1] - q[1]) > 0.0;
    if now <= FLOOR || !same_way {
        return Err(too_short(curve, "line", now, by.nm() < 0));
    }
    let modified = changed(sketch, &edited);
    if !modified.iter().any(|id| id == moving) {
        return Err(Error::new(
            ErrorCode::InvalidResult,
            format!("line {curve}'s end {moving} can't move by {by} mm: its constraints hold it where it is"),
        )
        .with_elements([curve.to_string(), moving.to_string()])
        .with_parameter("by")
        .with_suggestion("remove or change the constraints that hold the line's end"));
    }
    gated(&edited, curve)?;
    Ok(Edited {
        modified,
        sketch: edited,
        added: Vec::new(),
        removed: Vec::new(),
    })
}

/// An arc's `end` turned `by` about its centre: its start back, its end on.
fn extend_arc(
    sketch: &Sketch,
    curve: &ElementId,
    center: &ElementId,
    start: &ElementId,
    end_point: &ElementId,
    end: End,
    by: Angle,
) -> Result<Edited, Error> {
    if by.ndeg() == 0 {
        return Err(nothing(curve));
    }
    let (c, s, e) = (
        point(sketch, center)?,
        point(sketch, start)?,
        point(sketch, end_point)?,
    );
    let angle_of = |p: [f64; 2]| libm::atan2(p[1] - c[1], p[0] - c[0]);
    let sweep = (angle_of(e) - angle_of(s)).rem_euclid(TAU);
    let turn = by.degrees().to_radians();
    let swept = sweep + turn;
    let (moving, p, about) = match end {
        End::Start => (start, s, -turn),
        End::End => (end_point, e, turn),
    };
    let radius = libm::hypot(p[0] - c[0], p[1] - c[1]);
    if swept <= 0.0 || radius * swept <= FLOOR {
        return Err(too_short(curve, "arc", radius * swept.max(0.0), true));
    }
    let whole_turn = || {
        refused(
            format!(
                "extending arc {curve} by {by}° would sweep it {}°, a whole turn or more, or its ends within 1e-7 mm",
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
    let (sin, cos) = (libm::sin(about), libm::cos(about));
    let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
    let turned = [c[0] + dx * cos - dy * sin, c[1] + dx * sin + dy * cos];
    // Its ends apart by more than the floor, as the gate has them: near a whole turn, or
    // nearly nothing.
    let other = if moving == start { e } else { s };
    if libm::hypot(turned[0] - other[0], turned[1] - other[1]) <= FLOOR {
        return Err(if swept > std::f64::consts::PI {
            whole_turn()
        } else {
            too_short(curve, "arc", radius * swept, true)
        });
    }
    // Its radius, as rounding leaves the turned point, above the floor too.
    let left = libm::hypot(turned[0] - c[0], turned[1] - c[1]);
    if left <= FLOOR {
        return Err(refused(
            format!(
                "extending arc {curve} by {by}° would leave its turned end {left} mm from its centre, as rounding there takes it: an arc's radius is above 1e-7 mm"
            ),
            "by",
            "extend it by another angle, or draw the arc larger",
        )
        .with_elements([curve.to_string()]));
    }
    let coordinate = |v: f64| {
        Coordinate::new(v)
            .filter(|_| silico_protocol::within_reach(v))
            .ok_or_else(|| {
                refused(
                    format!(
                        "extending arc {curve} by {by}° would take its end past the sketch's reach"
                    ),
                    "by",
                    "extend it by less",
                )
                .with_elements([curve.to_string()])
            })
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
fn changed(sketch: &Sketch, edited: &Sketch) -> Vec<ElementId> {
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
    match crate::document::refused_by_gate(
        edited,
        &format!("extending {curve} there"),
        vec![curve.to_string()],
        "by",
        "extend it by less, or change what moves with its end",
    ) {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
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

fn too_short(curve: &ElementId, noun: &str, size: f64, shortening: bool) -> Error {
    let article = if noun == "arc" { "an" } else { "a" };
    refused(
        format!(
            "extending {noun} {curve} that far would leave it {size} mm long or flipped: {article} {noun} must be longer than 1e-7 mm and keep its direction"
        ),
        "by",
        if shortening {
            "shorten it by less"
        } else {
            "change the constraints that pull its end back, or extend it by less"
        },
    )
    .with_elements([curve.to_string()])
}
