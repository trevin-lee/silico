//! Splitting a curve at a position (docs/ports/sketch-edit.md, §3 and its table, which are
//! normative; decisions 1 to 4; docs/design/m5.0-app.md, `sketch_split`). The geometry is
//! the stored coordinates', computed in closed form with the libm shim's math, so every
//! platform makes the same bits; no point the split doesn't make moves.

use std::collections::BTreeSet;
use std::f64::consts::TAU;

use silico_protocol::{LENGTH_TOLERANCE, arc_from_points, distance, format_number};

use super::{Edited, kinds_of, no_member, ordered};
use crate::document::sketch::{
    Coordinate, Direction, ElementId, ElementKind, Form, Reference, Sketch, SketchArc,
    SketchConstraint, SketchLine, SketchPoint, TangentSide,
};
use crate::{Error, ErrorCode};

/// A curve split: the edit, and its two pieces, the start-side one first, which keeps the
/// curve's ID (FreeCAD `82610ca8fa`; a circle's goes counter-clockwise from `at.0` to
/// `at.1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub edited: Edited,
    pub pieces: [ElementId; 2],
}

/// Splits `curve` at `at`: a line or an arc at one position, into two pieces that share a
/// new point at the position's projection onto the curve; a circle at two, into two arcs
/// sharing two (decision 3). A line's pieces are joined by a `tangent_at` (`same`) at the
/// split point, which holds them collinear, as FreeCAD main does (`6d06b61c7e`, decision
/// 1); an arc's share its centre, so they lie on its circle. The constraints on the curve
/// follow §3's table, each keeping its ID: a retype or a move to the other piece is listed
/// in `modified`, a constraint dropped in `removed`.
///
/// A point the sketch holds `on` the curve within 1e-7 mm of the split position is the
/// split point, shared, in place of a new one, and its `on` is removed (implied by then);
/// a `tangent_on` whose `at` it is becomes a `tangent_at`.
///
/// `ids` are the new members' IDs, in `added`'s order: the new points along the curve from
/// its start (a circle's from `at.0`), the new piece, then a line's join.
///
/// Refused, as data naming the curve: a member that isn't a line, arc, or circle; the wrong
/// number of positions; a position farther than 1e-7 mm from the curve (decision 4: FreeCAD
/// projects any point); one at, past, or within 1e-7 mm of an end (`82633fb5df`'s refusal,
/// which `215fbc5187` lost, with the gate's size floor); a circle's two within 1e-7 mm of
/// each other; and IDs too few, too many, given twice, or the sketch's already.
pub fn split(
    sketch: &Sketch,
    curve: &ElementId,
    at: &[[f64; 2]],
    ids: &[ElementId],
) -> Result<Split, Error> {
    let kinds = kinds_of(sketch);
    let kind = match kinds.get(curve) {
        None => return Err(no_member(curve, ElementKind::Line, "curve")),
        Some(kind @ (ElementKind::Line | ElementKind::Arc | ElementKind::Circle)) => *kind,
        Some(other) => {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "{} {curve} isn't a curve: a line, an arc, or a circle splits",
                    other.noun()
                ),
            )
            .with_elements([curve.to_string()])
            .with_parameter("curve")
            .with_suggestion("name a line, arc, or circle of this sketch"));
        }
    };
    let noun = kind.noun();
    let refused = |message: String, parameter: &str| {
        Error::new(ErrorCode::InvalidInput, message)
            .with_elements([curve.to_string()])
            .with_parameter(parameter.to_owned())
    };
    let wanted = if kind == ElementKind::Circle { 2 } else { 1 };
    if at.len() != wanted {
        let error = if kind == ElementKind::Circle {
            let error = refused(
                format!(
                    "a circle splits at two positions into two arcs, got {}",
                    at.len()
                ),
                "at",
            );
            match at.len() {
                1 => error.with_cause("an arc whose start and end are one point can't be built"),
                _ => error,
            }
            .with_suggestion("give two positions on the circle")
        } else {
            refused(
                format!("{} splits at one position, got {}", kind.a_noun(), at.len()),
                "at",
            )
            .with_suggestion(format!("give one position on the {noun}"))
        };
        return Err(error);
    }
    for (place, position) in at.iter().enumerate() {
        if !position.iter().all(|c| c.is_finite()) {
            return Err(refused(
                "the split position isn't a finite position".to_owned(),
                &format!("at.{place}"),
            )
            .with_suggestion("give a position on the curve"));
        }
    }
    let shape = Shape::of(sketch, curve)?;
    // Each position's projection onto the curve, checked.
    let mut cuts: Vec<[f64; 2]> = Vec::new();
    for (place, position) in at.iter().enumerate() {
        let parameter = format!("at.{place}");
        let (cut, off) = shape.project(*position);
        if off > LENGTH_TOLERANCE {
            return Err(refused(
                format!(
                    "the split position is {} mm from {noun} {curve}, farther than 1e-7 mm",
                    format_number(off)
                ),
                &parameter,
            )
            .with_suggestion("give a position on the curve"));
        }
        if let Some((end, problem)) = shape.end_problem(cut) {
            let message = match problem {
                EndProblem::Past => {
                    format!("the split position is past {noun} {curve}'s end {end}")
                }
                EndProblem::Near(0.0) => {
                    format!("the split position is at {noun} {curve}'s end {end}")
                }
                EndProblem::Near(d) => format!(
                    "the split position is {} mm from {noun} {curve}'s end {end}, within 1e-7 mm",
                    format_number(d)
                ),
            };
            return Err(Error::new(ErrorCode::InvalidInput, message)
                .with_elements([curve.to_string(), end.to_string()])
                .with_parameter(parameter)
                .with_suggestion("give a position between the curve's ends"));
        }
        cuts.push(cut);
    }
    if let [first, second] = cuts[..] {
        let apart = distance(first, second);
        if apart <= LENGTH_TOLERANCE {
            return Err(refused(
                format!(
                    "the two split positions are {} mm apart on circle {curve}, within 1e-7 mm",
                    format_number(apart)
                ),
                "at.1",
            )
            .with_suggestion("give two positions apart on the circle"));
        }
    }
    // A point held on the curve at a cut is that cut's point; each taken once.
    let on_curve = Reference::Element(curve.clone());
    let mut shared: Vec<Option<ElementId>> = Vec::new();
    for cut in &cuts {
        let taken = sketch
            .constraints
            .iter()
            .filter_map(|c| match &c.form {
                Form::On { point, curve } if *curve == on_curve => point.element(),
                _ => None,
            })
            .filter(|p| !shared.iter().flatten().any(|s| s == *p))
            .filter_map(|p| point_at(sketch, p).map(|xy| (p, xy)))
            .filter(|(_, xy)| distance(*xy, *cut) <= LENGTH_TOLERANCE)
            .map(|(p, _)| p)
            .min()
            .cloned();
        shared.push(taken);
    }
    // The new IDs, in added's order.
    let needed =
        shared.iter().filter(|s| s.is_none()).count() + 1 + usize::from(kind == ElementKind::Line);
    if ids.len() != needed {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("the split needs {needed} new IDs, got {}", ids.len()),
        )
        .with_parameter("ids")
        .with_suggestion(
            "give one ID for each member the split adds, in order: its new points along the curve, its new piece, then a line's join",
        ));
    }
    for (place, id) in ids.iter().enumerate() {
        let message = if kinds.contains_key(id) {
            format!("the sketch already has a member {id}")
        } else if ids[..place].contains(id) {
            format!("ID {id} is given twice")
        } else {
            continue;
        };
        return Err(Error::new(ErrorCode::InvalidInput, message)
            .with_parameter(format!("ids.{place}"))
            .with_suggestion("give IDs no member of the sketch has, each once"));
    }
    let mut fresh = ids.iter().cloned();
    let construction = shape.construction(sketch, curve);
    let mut edited = sketch.clone();
    let mut added = Vec::new();
    let mut points = Vec::new();
    for (cut, shared) in cuts.iter().zip(&shared) {
        let point = match shared {
            Some(point) => point.clone(),
            None => {
                let id = next(&mut fresh)?;
                edited.points.push(SketchPoint {
                    id: id.clone(),
                    x: coordinate(cut[0])?,
                    y: coordinate(cut[1])?,
                    construction,
                });
                added.push(id.clone());
                id
            }
        };
        points.push(point);
    }
    let piece = next(&mut fresh)?;
    added.push(piece.clone());
    let mut kinds = kinds;
    for point in &points {
        kinds.insert(point.clone(), ElementKind::Point);
    }
    // The pieces, the start-side one keeping the curve's ID; `outer` is the curve's far end,
    // now the new piece's.
    let outer = match (&shape, &points[..]) {
        (Shape::Line { .. }, [cut]) => {
            let Some(line) = edited.lines.iter_mut().find(|l| &l.id == curve) else {
                return Err(no_member(curve, ElementKind::Line, "curve"));
            };
            let to = std::mem::replace(&mut line.to, cut.clone());
            edited.lines.push(SketchLine {
                id: piece.clone(),
                from: cut.clone(),
                to: to.clone(),
                construction,
            });
            kinds.insert(piece.clone(), ElementKind::Line);
            Some(to)
        }
        (Shape::Arc { .. }, [cut]) => {
            let Some(arc) = edited.arcs.iter_mut().find(|a| &a.id == curve) else {
                return Err(no_member(curve, ElementKind::Arc, "curve"));
            };
            let end = std::mem::replace(&mut arc.end, cut.clone());
            let center = arc.center.clone();
            edited.arcs.push(SketchArc {
                id: piece.clone(),
                center,
                start: cut.clone(),
                end: end.clone(),
                construction,
            });
            kinds.insert(piece.clone(), ElementKind::Arc);
            Some(end)
        }
        (Shape::Circle { .. }, [first, second]) => {
            let Some(place) = edited.circles.iter().position(|c| &c.id == curve) else {
                return Err(no_member(curve, ElementKind::Circle, "curve"));
            };
            let circle = edited.circles.remove(place);
            for (id, start, end) in [
                (curve.clone(), first.clone(), second.clone()),
                (piece.clone(), second.clone(), first.clone()),
            ] {
                edited.arcs.push(SketchArc {
                    id: id.clone(),
                    center: circle.center.clone(),
                    start,
                    end,
                    construction,
                });
                kinds.insert(id, ElementKind::Arc);
            }
            None
        }
        _ => return Err(no_member(curve, kind, "curve")),
    };
    // The constraints, by §3's table.
    let pieces = Pieces {
        shape: &shape,
        cuts: &cuts,
        curve: Reference::Element(curve.clone()),
        piece: Reference::Element(piece.clone()),
    };
    let mut modified = BTreeSet::from([curve.clone()]);
    let mut removed = BTreeSet::new();
    let mut constraints = Vec::new();
    for constraint in &sketch.constraints {
        match pieces.carry(sketch, constraint, outer.as_ref(), &shared) {
            None => {
                removed.insert(constraint.id.clone());
            }
            Some(form) => {
                if form != constraint.form {
                    modified.insert(constraint.id.clone());
                }
                constraints.push(SketchConstraint {
                    id: constraint.id.clone(),
                    form,
                });
            }
        }
    }
    if let (Shape::Line { .. }, [cut]) = (&shape, &points[..]) {
        let join = next(&mut fresh)?;
        constraints.push(SketchConstraint {
            id: join.clone(),
            form: Form::TangentAt {
                a: pieces.curve.clone(),
                b: pieces.piece.clone(),
                at: Reference::Element(cut.clone()),
                direction: Direction::Same,
            },
        });
        added.push(join);
    }
    edited.constraints = constraints;
    let removed_kinds = kinds_of(sketch);
    Ok(Split {
        edited: Edited {
            sketch: edited,
            added,
            modified: ordered(&kinds, modified),
            removed: ordered(&removed_kinds, removed),
        },
        pieces: [curve.clone(), piece],
    })
}

/// The next of the request's IDs: counted already, so never short.
fn next(fresh: &mut impl Iterator<Item = ElementId>) -> Result<ElementId, Error> {
    fresh.next().ok_or_else(|| {
        Error::new(
            ErrorCode::Internal,
            "the split ran out of the IDs it counted",
        )
        .with_parameter("ids")
    })
}

/// A computed coordinate, which is finite for any curve the gate takes.
fn coordinate(value: f64) -> Result<Coordinate, Error> {
    Coordinate::new(value).ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidInput,
            "the split point isn't a finite position",
        )
        .with_parameter("at")
        .with_suggestion("give a position on the curve")
    })
}

/// A point's stored coordinates; the origin's for `origin`.
fn point_at(sketch: &Sketch, id: &ElementId) -> Option<[f64; 2]> {
    sketch
        .points
        .iter()
        .find(|p| &p.id == id)
        .map(|p| [p.x.mm(), p.y.mm()])
}

fn reference_at(sketch: &Sketch, reference: &Reference) -> Option<[f64; 2]> {
    match reference {
        Reference::Origin => Some([0.0, 0.0]),
        Reference::Element(id) => point_at(sketch, id),
        Reference::XAxis | Reference::YAxis => None,
    }
}

/// The curve split, as its stored points place it.
enum Shape {
    Line {
        from: (ElementId, [f64; 2]),
        to: (ElementId, [f64; 2]),
    },
    /// Counter-clockwise from `start_angle` to `end_angle`, which is more, by less than a
    /// turn (`arc_from_points`, the profile's).
    Arc {
        center: [f64; 2],
        radius: f64,
        start: (ElementId, [f64; 2]),
        end: (ElementId, [f64; 2]),
        start_angle: f64,
        end_angle: f64,
    },
    Circle {
        center: [f64; 2],
        radius: f64,
    },
}

/// What's wrong with a split point near an end.
enum EndProblem {
    Past,
    Near(f64),
}

impl Shape {
    fn of(sketch: &Sketch, curve: &ElementId) -> Result<Shape, Error> {
        let at = |id: &ElementId| {
            point_at(sketch, id)
                .map(|xy| (id.clone(), xy))
                .ok_or_else(|| no_member(id, ElementKind::Point, "curve"))
        };
        if let Some(line) = sketch.lines.iter().find(|l| &l.id == curve) {
            return Ok(Shape::Line {
                from: at(&line.from)?,
                to: at(&line.to)?,
            });
        }
        if let Some(arc) = sketch.arcs.iter().find(|a| &a.id == curve) {
            let (center, start, end) = (at(&arc.center)?.1, at(&arc.start)?, at(&arc.end)?);
            let Some([radius, start_angle, end_angle]) = arc_from_points(center, start.1, end.1)
            else {
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!("arc {curve} sweeps no angle, so it can't be split"),
                )
                .with_elements([curve.to_string()])
                .with_parameter("curve"));
            };
            return Ok(Shape::Arc {
                center,
                radius,
                start,
                end,
                start_angle,
                end_angle,
            });
        }
        match sketch.circles.iter().find(|c| &c.id == curve) {
            Some(circle) => Ok(Shape::Circle {
                center: at(&circle.center)?.1,
                radius: circle.radius.mm(),
            }),
            None => Err(no_member(curve, ElementKind::Line, "curve")),
        }
    }

    fn construction(&self, sketch: &Sketch, curve: &ElementId) -> bool {
        sketch
            .lines
            .iter()
            .map(|l| (&l.id, l.construction))
            .chain(sketch.arcs.iter().map(|a| (&a.id, a.construction)))
            .chain(sketch.circles.iter().map(|c| (&c.id, c.construction)))
            .find(|(id, _)| *id == curve)
            .is_some_and(|(_, construction)| construction)
    }

    /// `position`'s projection onto the curve (a line's onto the line through its ends, a
    /// round's onto its circle), and how far it is from there.
    fn project(&self, position: [f64; 2]) -> ([f64; 2], f64) {
        match self {
            Shape::Line { from, to } => {
                let projected = project_on_line(from.1, to.1, position);
                (projected, distance(projected, position))
            }
            Shape::Arc { center, radius, .. } | Shape::Circle { center, radius } => {
                let away = distance(*center, position);
                if away == 0.0 {
                    // Every point of the circle is as near: none is the projection.
                    return (position, *radius);
                }
                let scale = radius / away;
                (
                    [
                        center[0] + (position[0] - center[0]) * scale,
                        center[1] + (position[1] - center[1]) * scale,
                    ],
                    (away - radius).abs(),
                )
            }
        }
    }

    /// The end a split point on the curve is within 1e-7 mm of, or else past; none for a
    /// circle.
    fn end_problem(&self, cut: [f64; 2]) -> Option<(&ElementId, EndProblem)> {
        let (start, end, past) = match self {
            Shape::Line { from, to } => {
                let t = line_parameter(from.1, to.1, cut);
                let past = (t < 0.0).then_some(&from.0).or((t > 1.0).then_some(&to.0));
                (from, to, past)
            }
            Shape::Arc {
                start,
                end,
                end_angle,
                ..
            } => {
                let past = (self.angle_along(cut) >= *end_angle).then(|| {
                    if distance(cut, start.1) <= distance(cut, end.1) {
                        &start.0
                    } else {
                        &end.0
                    }
                });
                (start, end, past)
            }
            Shape::Circle { .. } => return None,
        };
        // Within 1e-7 mm of an end is said so, past it or not.
        [start, end]
            .into_iter()
            .find_map(|(id, xy)| {
                let d = distance(cut, *xy);
                (d <= LENGTH_TOLERANCE).then_some((id, EndProblem::Near(d)))
            })
            .or(past.map(|past| (past, EndProblem::Past)))
    }

    /// For a round, `point`'s angle about the centre counter-clockwise from the arc's start
    /// angle (0 for a circle), in [start, start + a turn).
    fn angle_along(&self, point: [f64; 2]) -> f64 {
        let (center, from) = match self {
            Shape::Arc {
                center,
                start_angle,
                ..
            } => (*center, *start_angle),
            Shape::Circle { center, .. } => (*center, 0.0),
            Shape::Line { .. } => return 0.0,
        };
        let angle = libm::atan2(point[1] - center[1], point[0] - center[0]);
        from + (angle - from).rem_euclid(TAU)
    }

    fn round(&self) -> Option<([f64; 2], f64)> {
        match self {
            Shape::Arc { center, radius, .. } | Shape::Circle { center, radius } => {
                Some((*center, *radius))
            }
            Shape::Line { .. } => None,
        }
    }
}

/// Where `point` projects onto the line through `from` and `to`, as a fraction of the way
/// from one to the other.
fn line_parameter(from: [f64; 2], to: [f64; 2], point: [f64; 2]) -> f64 {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    ((point[0] - from[0]) * dx + (point[1] - from[1]) * dy) / (dx * dx + dy * dy)
}

fn project_on_line(from: [f64; 2], to: [f64; 2], point: [f64; 2]) -> [f64; 2] {
    let t = line_parameter(from, to, point);
    [
        from[0] + t * (to[0] - from[0]),
        from[1] + t * (to[1] - from[1]),
    ]
}

/// The split's two pieces, for carrying constraints to them.
struct Pieces<'a> {
    shape: &'a Shape,
    cuts: &'a [[f64; 2]],
    /// The start-side piece, which keeps the curve's ID.
    curve: Reference,
    /// The new piece.
    piece: Reference,
}

impl Pieces<'_> {
    /// The piece whose span holds `point`'s projection; outside every span, the nearer
    /// end's (FreeCAD's `closestParameter` fallback, `Part/App/Geometry.cpp:1073-1083`,
    /// `bbbff1f8f1`); a circle's pieces span the whole turn between them (`263331f6fb`).
    fn holding(&self, point: [f64; 2]) -> &Reference {
        let start_side = match (self.shape, self.cuts) {
            (Shape::Line { from, to }, [cut]) => {
                line_parameter(from.1, to.1, point) <= line_parameter(from.1, to.1, *cut)
            }
            (
                Shape::Arc {
                    start,
                    end,
                    end_angle,
                    ..
                },
                [cut],
            ) => {
                let along = self.shape.angle_along(point);
                if along <= self.shape.angle_along(*cut) {
                    true
                } else if along <= *end_angle {
                    false
                } else {
                    distance(point, start.1) <= distance(point, end.1)
                }
            }
            (Shape::Circle { center, .. }, [first, second]) => {
                let angle = |p: [f64; 2]| libm::atan2(p[1] - center[1], p[0] - center[0]);
                let from = angle(*first);
                (angle(point) - from).rem_euclid(TAU) <= (angle(*second) - from).rem_euclid(TAU)
            }
            _ => true,
        };
        if start_side { &self.curve } else { &self.piece }
    }

    /// Where an edge tangency of the curve with `other` touches it, on the side `side`
    /// says; none where nothing says (concentric circles, a reference with no geometry).
    fn contact(&self, sketch: &Sketch, other: &Reference, side: TangentSide) -> Option<[f64; 2]> {
        match (self.shape, line_of(sketch, other), round_of(sketch, other)) {
            // A line's contact with a round is the round's centre projected onto it.
            (Shape::Line { from, to }, _, Some((center, _))) => {
                Some(project_on_line(from.1, to.1, center))
            }
            // A round's with a line, the foot of its centre's perpendicular.
            (_, Some((a, b)), _) => {
                let (center, _) = self.shape.round()?;
                Some(project_on_line(a, b, center))
            }
            // A round's with a round, along the line between their centres: toward the
            // other's, unless this one is inside it.
            (_, _, Some((other_center, other_radius))) => {
                let (center, radius) = self.shape.round()?;
                let apart = distance(center, other_center);
                if apart == 0.0 {
                    return None;
                }
                let toward = if side == TangentSide::Inside && radius < other_radius {
                    -radius / apart
                } else {
                    radius / apart
                };
                Some([
                    center[0] + (other_center[0] - center[0]) * toward,
                    center[1] + (other_center[1] - center[1]) * toward,
                ])
            }
            _ => None,
        }
    }

    /// `constraint` carried to the pieces by §3's table; none if it's dropped.
    fn carry(
        &self,
        sketch: &Sketch,
        constraint: &SketchConstraint,
        outer: Option<&ElementId>,
        shared: &[Option<ElementId>],
    ) -> Option<Form> {
        let curve = &self.curve;
        let is_line = matches!(self.shape, Shape::Line { .. });
        // The line's original ends, for the forms that become point constraints.
        let ends = match self.shape {
            Shape::Line { from, to } => Some((
                Reference::Element(from.0.clone()),
                Reference::Element(to.0.clone()),
            )),
            _ => None,
        };
        let outer = outer.map(|id| Reference::Element(id.clone()));
        let is_shared = |point: &Reference| {
            point
                .element()
                .is_some_and(|p| shared.iter().flatten().any(|s| s == p))
        };
        // The piece holding a point's stored place (the start-side piece if it has none).
        let holding = |point: &Reference| {
            reference_at(sketch, point)
                .map_or(curve, |xy| self.holding(xy))
                .clone()
        };
        // A constraint at the curve's far end is the new piece's.
        let at_end = |r: &Reference, at: &Reference| {
            if r == curve && Some(at) == outer.as_ref() {
                self.piece.clone()
            } else {
                r.clone()
            }
        };
        let form = &constraint.form;
        Some(match form {
            // A line's length is the distance between its outer ends (`4d6b1f3eb8`).
            Form::Length {
                line,
                value,
                driving,
            } if line == curve => {
                let (a, b) = ends?;
                Form::Distance {
                    a,
                    b,
                    value: *value,
                    driving: *driving,
                }
            }
            // Its midpoint, the point its outer ends are symmetric about.
            Form::Midpoint { point, line } if line == curve => {
                let (a, b) = ends?;
                Form::Symmetric {
                    a,
                    b,
                    about: point.clone(),
                }
            }
            // A line's equal is dropped (`857190a20a`, #25008); a round's stays with the
            // start-side piece, whose radius the other shares through their points.
            Form::Equal { a, b } if is_line && (a == curve || b == curve) => return None,
            // A point held on the curve at the split is the split point: implied.
            Form::On { point, curve: on } if on == curve && is_shared(point) => return None,
            Form::On { point, curve: on } if on == curve => Form::On {
                point: point.clone(),
                curve: holding(point),
            },
            Form::PointLineDistance {
                point,
                line,
                value,
                side,
                driving,
            } if line == curve => Form::PointLineDistance {
                point: point.clone(),
                line: holding(point),
                value: *value,
                side: *side,
                driving: *driving,
            },
            // An edge tangency goes to the piece whose span holds the contact, else the
            // start-side piece (decision 2): each lies on the original's line or circle.
            Form::Tangent { a, b, side } if a == curve || b == curve => {
                let other = if a == curve { b } else { a };
                let to = self
                    .contact(sketch, other, *side)
                    .map_or(curve, |xy| self.holding(xy))
                    .clone();
                let swap = |r: &Reference| if r == curve { to.clone() } else { r.clone() };
                Form::Tangent {
                    a: swap(a),
                    b: swap(b),
                    side: *side,
                }
            }
            // A tangency at the curve's far end stays at that point, the new piece's end.
            Form::TangentOn {
                a,
                at,
                b,
                direction,
            } if a == curve => Form::TangentOn {
                a: at_end(a, at),
                at: at.clone(),
                b: b.clone(),
                direction: *direction,
            },
            // Another curve's end on the split one: at the split point, a tangent_at with
            // the start-side piece, whose own direction there is the curve's, so `direction`
            // carries as stored (the note leaves which piece open, both meeting there; M5.0
            // to settle); elsewhere, on the piece holding it.
            Form::TangentOn {
                a,
                at,
                b,
                direction,
            } if b == curve && is_shared(at) => Form::TangentAt {
                a: a.clone(),
                b: curve.clone(),
                at: at.clone(),
                direction: *direction,
            },
            Form::TangentOn {
                a,
                at,
                b,
                direction,
            } if b == curve => Form::TangentOn {
                a: a.clone(),
                at: at.clone(),
                b: holding(at),
                direction: *direction,
            },
            Form::TangentAt {
                a,
                b,
                at,
                direction,
            } if a == curve || b == curve => Form::TangentAt {
                a: at_end(a, at),
                b: at_end(b, at),
                at: at.clone(),
                direction: *direction,
            },
            // Everything else stays: direction constraints, an angle, a symmetry about the
            // line, and a round's radius on the start-side piece (decision 1).
            _ => form.clone(),
        })
    }
}

/// A line's two points: a sketch line's, or an axis's.
fn line_of(sketch: &Sketch, reference: &Reference) -> Option<([f64; 2], [f64; 2])> {
    match reference {
        Reference::XAxis => Some(([0.0, 0.0], [1.0, 0.0])),
        Reference::YAxis => Some(([0.0, 0.0], [0.0, 1.0])),
        Reference::Element(id) => {
            let line = sketch.lines.iter().find(|l| &l.id == id)?;
            Some((point_at(sketch, &line.from)?, point_at(sketch, &line.to)?))
        }
        Reference::Origin => None,
    }
}

/// A round's centre and radius: a circle's, or an arc's, as the profile measures it.
fn round_of(sketch: &Sketch, reference: &Reference) -> Option<([f64; 2], f64)> {
    let id = reference.element()?;
    if let Some(circle) = sketch.circles.iter().find(|c| &c.id == id) {
        return Some((point_at(sketch, &circle.center)?, circle.radius.mm()));
    }
    let arc = sketch.arcs.iter().find(|a| &a.id == id)?;
    let center = point_at(sketch, &arc.center)?;
    let [radius, ..] = arc_from_points(
        center,
        point_at(sketch, &arc.start)?,
        point_at(sketch, &arc.end)?,
    )?;
    Some((center, radius))
}
