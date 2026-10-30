//! Trimming a curve back to its neighbours (docs/ports/sketch-edit.md, §4, which is
//! normative with the decisions it cites, 2, 5, 6, and 8; docs/design/m5.0-app.md,
//! `sketch_trim`). The cuts are found in closed form from the stored coordinates, with the
//! libm shim's math, so every platform finds the same; a gap of at most 1e-7 mm counts as
//! contact (decision 6).

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::f64::consts::TAU;

use silico_protocol::{LENGTH_TOLERANCE, arc_from_points, distance};

use super::split::{
    coordinate, line_parameter, next, point_at, project_on_line, reference_at, starts_at,
};
use super::{Edited, NewIds, kinds_of, new_ids, no_member, ordered, own_curve, remove};
use crate::document::sketch::{
    Direction, ElementId, ElementKind, Form, Reference, Sketch, SketchArc, SketchConstraint,
    SketchLine, SketchPoint, TangentSide,
};
use crate::{Error, ErrorCode};

/// A curve trimmed: the edit, and the pieces left, the first keeping the curve's ID
/// (FreeCAD `7d8a8b7cc8`): none when the trim removes the curve whole, one when it shortens
/// it or opens a circle, two when it takes out its middle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trim {
    pub edited: Edited,
    pub pieces: Vec<ElementId>,
}

/// Trims `curve` back to the cuts either side of `pick`'s projection onto it, removing the
/// part between (§4). Every other line, arc, and circle cuts, construction too
/// (`715f98d1f3`); a point doesn't, nor do the axes. A cut is a crossing, a tangent contact
/// or an overlap's end (`3cbe309330`), or another curve's end on it (`e10068a648`), and a
/// cut within 1e-7 mm of the curve's own end is none (`226d24792d`'s rule, at the gate's
/// floor). The curve is removed whole, as `remove` removes it, with no cut on either side,
/// two cuts within 1e-7 mm of each other, or a circle with fewer than two (`2d817128d0`).
///
/// Each new end at a cut shares the cutter's end where a constraint holds that end on the
/// curve (an `on`, which then goes as implied, or a `tangent_on`, which becomes a
/// `tangent_at`); else it's a new point, `on` the cutter (decision 5). An edge tangency
/// with the cutter becomes an end tangency there, its `direction` as the stored drawing
/// shows it (decision 8: the solve after the edit must agree, which is the command's). A
/// line's direction constraints and edge tangencies go to every piece (§4, item 9); the
/// rest follow §3's table, a point on the removed part dropped with its `on`
/// (`333ca836c0`). A curve's end trimmed away goes, with its constraints, unless another
/// curve uses it, which keeps it.
///
/// `ids` are the new members' IDs, in `added`'s order: the new points along the curve from
/// its start (a circle's along the arc left), the new piece, the constraints copied to it
/// (by the IDs of those they copy), then the new ends' `on`s.
///
/// Refused, as data: a member that isn't a line, arc, or circle; a pick that isn't finite;
/// IDs too few, too many, given twice, or the sketch's already; and decision 2's cases,
/// naming the constraint: a `symmetric` about a line trimmed into two, and an edge tangency
/// of an arc or circle that no piece touches.
pub fn trim(
    sketch: &Sketch,
    curve: &ElementId,
    pick: [f64; 2],
    ids: &[ElementId],
) -> Result<Trim, Error> {
    trim_with(sketch, curve, pick, NewIds::Given(ids))
}

/// [`trim`], its new IDs the request's or drawn ([`NewIds`]): drawn only once every other
/// refusal has been found, as many as the trim needs.
pub fn trim_with(
    sketch: &Sketch,
    curve: &ElementId,
    pick: [f64; 2],
    ids: NewIds<'_>,
) -> Result<Trim, Error> {
    let kinds = kinds_of(sketch);
    let kind = match kinds.get(curve) {
        None => return Err(no_member(curve, ElementKind::Line, "curve")),
        Some(kind @ (ElementKind::Line | ElementKind::Arc | ElementKind::Circle)) => *kind,
        Some(other) => {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "{} {curve} isn't a curve: a line, an arc, or a circle trims",
                    other.noun()
                ),
            )
            .with_elements([curve.to_string()])
            .with_parameter("curve")
            .with_suggestion("name a line, arc, or circle of this sketch"));
        }
    };
    if !pick.iter().all(|c| c.is_finite()) {
        return Err(
            Error::new(ErrorCode::InvalidInput, "the pick isn't a finite position")
                .with_elements([curve.to_string()])
                .with_parameter("pick")
                .with_suggestion("give a position on the part of the curve to trim away"),
        );
    }
    let Some(geo) = Geo::of(sketch, curve) else {
        return Err(no_member(curve, kind, "curve"));
    };
    let cuts = cuts(sketch, curve, &geo);
    let picked = match &geo {
        // A round's pick is taken at its own angle, however far out (a projection's
        // distance can overflow); its centre is at none.
        Geo::Arc { center, .. } | Geo::Circle { center, .. } if pick == *center => {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "the pick is at {} {curve}'s centre, on no part of it",
                    kind.noun()
                ),
            )
            .with_elements([curve.to_string()])
            .with_parameter("pick")
            .with_suggestion("give a position on the part of the curve to trim away"));
        }
        Geo::Arc { .. } | Geo::Circle { .. } => geo.along(pick),
        Geo::Line { .. } => geo.along(geo.project(pick)),
    };
    // An arc's pick outside its sweep is at the end nearer in angle, as its projection is:
    // past the end, or round to the start.
    let picked = match &geo {
        Geo::Arc {
            start_angle,
            end_angle,
            ..
        } if picked > *end_angle => {
            if start_angle + TAU - picked < picked - end_angle {
                *start_angle
            } else {
                *end_angle
            }
        }
        _ => picked,
    };
    if !picked.is_finite() {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "the pick is too far from {} {curve} to place on it",
                kind.noun()
            ),
        )
        .with_elements([curve.to_string()])
        .with_parameter("pick")
        .with_suggestion("give a position on the part of the curve to trim away"));
    }
    let Some(layout) = Layout::of(&geo, &cuts, picked) else {
        // Removed whole: a curve `remove` never refuses.
        new_ids(
            &kinds,
            ids,
            0,
            "the trim",
            "nothing, since it removes the curve whole",
        )?;
        let edited = remove(sketch, std::slice::from_ref(curve)).map_err(|refused| {
            refused.errors.into_iter().next().unwrap_or_else(|| {
                Error::new(ErrorCode::Internal, "the trim's removal refused nothing")
            })
        })?;
        return Ok(Trim {
            edited,
            pieces: Vec::new(),
        });
    };
    let original = Original::of(sketch, curve)?;
    let curve_ref = Reference::Element(curve.clone());
    // Each cut's join: the cutter's end shared, or a new point; and the constraints it
    // takes over.
    let mut joins: Vec<Join> = Vec::new();
    for (cut, side) in layout.cuts() {
        let shared = cut.end.as_ref().filter(|_| cut.held);
        // A cutter's tangency joins one end only: a cutter drawn crossing twice holds the
        // other with an on.
        let tangent = sketch
            .constraints
            .iter()
            .find(|c| match &c.form {
                Form::Tangent { a, b, .. } => {
                    (*a == curve_ref && b.element() == Some(&cut.cutter))
                        || (*b == curve_ref && a.element() == Some(&cut.cutter))
                }
                _ => false,
            })
            .filter(|c| {
                shared.is_none() && !joins.iter().any(|j| j.tangent.as_ref() == Some(&c.id))
            })
            .map(|c| c.id.clone());
        // A cutter's perpendicular goes with the first of its cuts.
        let perpendicular = sketch
            .constraints
            .iter()
            .find(|c| match &c.form {
                Form::Perpendicular { a, b } => {
                    (*a == curve_ref && b.element() == Some(&cut.cutter))
                        || (*b == curve_ref && a.element() == Some(&cut.cutter))
                }
                _ => false,
            })
            .filter(|c| {
                !joins
                    .iter()
                    .any(|j| j.perpendicular.as_ref() == Some(&c.id))
            })
            .map(|c| c.id.clone());
        joins.push(Join {
            cut: cut.clone(),
            side,
            shared: shared.cloned(),
            tangent,
            perpendicular,
        });
    }
    // A line's direction constraints and edge tangencies, copied to its second piece, but
    // those a cut takes.
    let taken_over: BTreeSet<&ElementId> = joins
        .iter()
        .flat_map(|j| [j.tangent.as_ref(), j.perpendicular.as_ref()])
        .flatten()
        .collect();
    let mut copies: Vec<&SketchConstraint> = if kind == ElementKind::Line && layout.two_pieces() {
        sketch
            .constraints
            .iter()
            .filter(|c| !taken_over.contains(&c.id) && copied_to_every_piece(&c.form, &curve_ref))
            .collect()
    } else {
        Vec::new()
    };
    copies.sort_by(|a, b| a.id.cmp(&b.id));
    let new_points = joins.iter().filter(|j| j.shared.is_none()).count();
    let new_ons = joins
        .iter()
        .filter(|j| j.shared.is_none() && j.tangent.is_none())
        .count();
    let needed = new_points + usize::from(layout.two_pieces()) + copies.len() + new_ons;
    let ids = new_ids(
        &kinds,
        ids,
        needed,
        "the trim",
        "its new points along the curve, its new piece, the constraints copied to it, then its new ends' ons",
    )?;
    let mut fresh = ids.into_iter();
    let mut added = Vec::new();
    let mut kinds = kinds;
    let mut edited = sketch.clone();
    // The new ends' points.
    let mut ends: Vec<ElementId> = Vec::new();
    for join in &joins {
        let point = match &join.shared {
            Some(point) => point.clone(),
            None => {
                let id = next(&mut fresh)?;
                edited.points.push(SketchPoint {
                    id: id.clone(),
                    x: coordinate(join.cut.at[0], "pick")?,
                    y: coordinate(join.cut.at[1], "pick")?,
                    construction: original.construction,
                });
                kinds.insert(id.clone(), ElementKind::Point);
                added.push(id.clone());
                id
            }
        };
        ends.push(point);
    }
    // The pieces.
    let second = if layout.two_pieces() {
        let id = next(&mut fresh)?;
        added.push(id.clone());
        Some(id)
    } else {
        None
    };
    let pieces = original.cut(&mut edited, &mut kinds, &layout, &ends, second.as_ref())?;
    // Which piece each original end is on, if it's kept.
    let start_piece = layout.keeps_start().then(|| curve_ref.clone());
    let end_piece = layout
        .keeps_end()
        .then(|| second.clone().map_or(curve_ref.clone(), Reference::Element));
    let placing = Placing {
        geo: &geo,
        layout: &layout,
        first: curve_ref.clone(),
        second: second.clone().map(Reference::Element),
    };
    // The constraints.
    let mut modified = BTreeSet::from([curve.clone()]);
    let mut removed: BTreeSet<ElementId> = BTreeSet::new();
    let mut constraints: Vec<SketchConstraint> = Vec::new();
    for constraint in &sketch.constraints {
        let id = &constraint.id;
        let form = &constraint.form;
        // A cutter's perpendicular: on the piece at its cut alone.
        if let Some(join) = joins.iter().find(|j| j.perpendicular.as_ref() == Some(id)) {
            let piece = piece_at(join.side, &curve_ref, second.as_ref());
            let mut form = form.clone();
            for reference in super::references_mut(&mut form) {
                if *reference == curve_ref {
                    *reference = piece.clone();
                }
            }
            if form != constraint.form {
                modified.insert(id.clone());
            }
            constraints.push(SketchConstraint {
                id: id.clone(),
                form,
            });
            continue;
        }
        // A join's: the on or tangent_on holding the shared end, and the cutter's tangency.
        if let Some(join) = joins
            .iter()
            .zip(&ends)
            .find_map(|(join, end)| joins_constraint(join, form, &curve_ref).then_some((join, end)))
        {
            let (join, end) = join;
            let piece = piece_at(join.side, &curve_ref, second.as_ref());
            match form {
                Form::On { .. } => {
                    removed.insert(id.clone());
                }
                Form::TangentOn {
                    a, at, direction, ..
                } => {
                    constraints.push(SketchConstraint {
                        id: id.clone(),
                        form: Form::TangentAt {
                            a: a.clone(),
                            b: piece.clone(),
                            at: at.clone(),
                            direction: mapped(&edited, a, &piece, at, *direction),
                        },
                    });
                    modified.insert(id.clone());
                }
                Form::Tangent { .. } => {
                    let at = Reference::Element(end.clone());
                    let cutter = Reference::Element(join.cut.cutter.clone());
                    constraints.push(SketchConstraint {
                        id: id.clone(),
                        form: Form::TangentOn {
                            direction: drawn(sketch, &edited, &piece, &at, &cutter),
                            a: piece,
                            at,
                            b: cutter,
                        },
                    });
                    modified.insert(id.clone());
                }
                _ => constraints.push(constraint.clone()),
            }
            continue;
        }
        let carried = carry(
            sketch,
            constraint,
            &curve_ref,
            &original,
            &placing,
            start_piece.as_ref(),
            end_piece.as_ref(),
        )?;
        match carried {
            None => {
                removed.insert(id.clone());
            }
            Some(next_form) => {
                if next_form != *form {
                    modified.insert(id.clone());
                }
                constraints.push(SketchConstraint {
                    id: id.clone(),
                    form: next_form,
                });
            }
        }
    }
    // The copies for the second piece, then the new ends' ons.
    if let Some(second) = &second {
        let to = Reference::Element(second.clone());
        for copy in &copies {
            let id = next(&mut fresh)?;
            let mut form = copy.form.clone();
            for reference in super::references_mut(&mut form) {
                if *reference == curve_ref {
                    *reference = to.clone();
                }
            }
            constraints.push(SketchConstraint {
                id: id.clone(),
                form,
            });
            kinds.insert(id.clone(), ElementKind::Constraint);
            added.push(id);
        }
    }
    for (join, end) in joins.iter().zip(&ends) {
        if join.shared.is_none() && join.tangent.is_none() {
            let id = next(&mut fresh)?;
            constraints.push(SketchConstraint {
                id: id.clone(),
                form: Form::On {
                    point: Reference::Element(end.clone()),
                    curve: Reference::Element(join.cut.cutter.clone()),
                },
            });
            kinds.insert(id.clone(), ElementKind::Constraint);
            added.push(id);
        }
    }
    edited.constraints = constraints;
    // The curve's ends trimmed away that no curve uses now go, with every constraint that
    // names them.
    let used: BTreeSet<ElementId> = super::curves(&edited)
        .into_iter()
        .flat_map(|(_, _, points)| points.into_iter().cloned())
        .collect();
    let gone: BTreeSet<ElementId> = original
        .end_points()
        .into_iter()
        .filter(|p| !used.contains(p))
        .collect();
    // A constraint on both of the curve's ends, one trimmed away, goes too, though another
    // curve keeps that end (`f0d30cb570`, FreeCAD's involvesBothButNotBothKept,
    // `SketchObjectOperations.cpp:1020-1031`).
    let both_ends: Option<[&ElementId; 2]> = match (&original.start, &original.end) {
        (Some(start), Some(end)) if layout.keeps_start() != layout.keeps_end() => {
            Some([start, end])
        }
        _ => None,
    };
    edited.points.retain(|p| !gone.contains(&p.id));
    edited.constraints.retain(|c| {
        let references = c.form.references();
        let names = |id: &ElementId| references.iter().any(|(_, r, _)| r.element() == Some(id));
        let names_gone = references
            .iter()
            .any(|(_, r, _)| r.element().is_some_and(|id| gone.contains(id)))
            || both_ends.is_some_and(|ends| ends.iter().all(|end| names(end)));
        if names_gone {
            removed.insert(c.id.clone());
            modified.remove(&c.id);
        }
        !names_gone
    });
    removed.extend(gone);
    own_curve(&edited, "the trim", "pick", "trim elsewhere")?;
    let before = kinds_of(sketch);
    Ok(Trim {
        edited: Edited {
            sketch: edited,
            added,
            modified: ordered(&kinds, modified),
            removed: ordered(&before, removed),
        },
        pieces,
    })
}

/// Whether a constraint on the trimmed line goes to each of its pieces: its direction
/// constraints and edge tangencies (§4, item 9; FreeCAD's `assumeTangency = false`).
fn copied_to_every_piece(form: &Form, curve: &Reference) -> bool {
    match form {
        Form::Horizontal { line } | Form::Vertical { line } => line == curve,
        Form::Parallel { a, b }
        | Form::Perpendicular { a, b }
        | Form::Angle { a, b, .. }
        | Form::Tangent { a, b, .. } => a == curve || b == curve,
        _ => false,
    }
}

/// Whether `form` is one `join` takes over: the `on` or `tangent_on` holding its shared
/// end on the curve, or the edge tangency of its cutter it turns into an end tangency.
fn joins_constraint(join: &Join, form: &Form, curve: &Reference) -> bool {
    let cutter = &join.cut.cutter;
    match (form, &join.shared) {
        (Form::On { point, curve: on }, Some(end)) => point.element() == Some(end) && on == curve,
        (Form::TangentOn { a, at, b, .. }, Some(end)) => {
            a.element() == Some(cutter) && at.element() == Some(end) && b == curve
        }
        (Form::Tangent { a, b, .. }, None) if join.tangent.is_some() => {
            (a == curve && b.element() == Some(cutter))
                || (b == curve && a.element() == Some(cutter))
        }
        _ => false,
    }
}

/// The piece a cut on `side` ends.
fn piece_at(side: Side, first: &Reference, second: Option<&ElementId>) -> Reference {
    match (side, second) {
        (Side::After, Some(second)) => Reference::Element(second.clone()),
        _ => first.clone(),
    }
}

/// A `tangent_on`'s `direction` (`a`'s leaving `at` against `b`'s own) as the `tangent_at`'s
/// of `a` and `piece` at `at` (a path along one into `at` and on along the other), so the
/// solver holds the same angle: never derived from the drawing (as split maps it).
fn mapped(
    edited: &Sketch,
    a: &Reference,
    piece: &Reference,
    at: &Reference,
    direction: Direction,
) -> Direction {
    match (starts_at(edited, a, at), starts_at(edited, piece, at)) {
        (Some(a_starts), Some(piece_starts)) => {
            if (a_starts == (direction == Direction::Same)) == (a_starts != piece_starts) {
                Direction::Same
            } else {
                Direction::Opposite
            }
        }
        _ => direction,
    }
}

/// The `direction` of a `tangent_on` of `piece`'s end `at` on `cutter`, as the stored drawing
/// shows it (decision 8): `same` when the piece, leaving `at`, runs as the cutter's own
/// direction there.
fn drawn(
    sketch: &Sketch,
    edited: &Sketch,
    piece: &Reference,
    at: &Reference,
    cutter: &Reference,
) -> Direction {
    let (Some(xy), Some(starts)) = (reference_at(edited, at), starts_at(edited, piece, at)) else {
        return Direction::Same;
    };
    let (Some(along_piece), Some(along_cutter)) = (
        own_direction(edited, piece, xy),
        own_direction(sketch, cutter, xy),
    ) else {
        return Direction::Same;
    };
    let leaving = if starts {
        along_piece
    } else {
        [-along_piece[0], -along_piece[1]]
    };
    if leaving[0] * along_cutter[0] + leaving[1] * along_cutter[1] >= 0.0 {
        Direction::Same
    } else {
        Direction::Opposite
    }
}

/// A curve's own direction at `xy`: a line's from `from` to `to`, a round's
/// counter-clockwise.
fn own_direction(sketch: &Sketch, curve: &Reference, xy: [f64; 2]) -> Option<[f64; 2]> {
    let id = curve.element()?;
    if let Some(line) = sketch.lines.iter().find(|l| &l.id == id) {
        let (from, to) = (point_at(sketch, &line.from)?, point_at(sketch, &line.to)?);
        return Some([to[0] - from[0], to[1] - from[1]]);
    }
    let center = match sketch.arcs.iter().find(|a| &a.id == id) {
        Some(arc) => point_at(sketch, &arc.center)?,
        None => point_at(sketch, &sketch.circles.iter().find(|c| &c.id == id)?.center)?,
    };
    Some([-(xy[1] - center[1]), xy[0] - center[0]])
}

/// `constraint`, not a join's, carried to the pieces; none if it's dropped.
fn carry(
    sketch: &Sketch,
    constraint: &SketchConstraint,
    curve: &Reference,
    original: &Original,
    placing: &Placing,
    start_piece: Option<&Reference>,
    end_piece: Option<&Reference>,
) -> Result<Option<Form>, Error> {
    let form = &constraint.form;
    let two = placing.second.is_some();
    let is_line = matches!(placing.geo, Geo::Line { .. });
    // The piece holding a point's stored place; none if it's on the part removed.
    let holding = |point: &Reference| match reference_at(sketch, point) {
        Some(xy) => placing.holding(xy),
        None => Some(curve.clone()),
    };
    // A constraint at one of the curve's ends: the piece that keeps that end, or none.
    let at_end = |r: &Reference, at: &Reference| -> Option<Reference> {
        if r != curve {
            return Some(r.clone());
        }
        if original.start.as_ref() == at.element() {
            start_piece.cloned()
        } else if original.end.as_ref() == at.element() {
            end_piece.cloned()
        } else {
            Some(r.clone())
        }
    };
    let ends = || {
        Some((
            Reference::Element(original.start.clone()?),
            Reference::Element(original.end.clone()?),
        ))
    };
    Ok(Some(match form {
        // A line's length, the distance of its outer ends with two pieces; with one, an end
        // gone, dropped (`2779f0b29f`).
        Form::Length {
            line,
            value,
            driving,
        } if line == curve => {
            let Some((a, b)) = ends().filter(|_| two) else {
                return Ok(None);
            };
            Form::Distance {
                a,
                b,
                value: *value,
                driving: *driving,
            }
        }
        // Its midpoint, the outer ends' symmetric about the point, while both are kept
        // (`f0d30cb570`: a constraint on both ends survives only if both do).
        Form::Midpoint { point, line } if line == curve => {
            let Some((a, b)) = ends().filter(|_| two) else {
                return Ok(None);
            };
            Form::Symmetric {
                a,
                b,
                about: point.clone(),
            }
        }
        Form::Equal { a, b } if is_line && (a == curve || b == curve) => return Ok(None),
        // A point on the removed part: dropped (`333ca836c0`).
        Form::On { point, curve: on } if on == curve => match holding(point) {
            Some(piece) => Form::On {
                point: point.clone(),
                curve: piece,
            },
            None => return Ok(None),
        },
        // A distance from the line, as an `on`: to the piece holding the point, dropped over
        // the part removed (FreeCAD's findPieceContainingPoint, which has no fallback:
        // `SketchObjectConstraints.cpp:1478-1500`).
        Form::PointLineDistance {
            point,
            line,
            value,
            side,
            driving,
        } if line == curve => match holding(point) {
            Some(line) => Form::PointLineDistance {
                point: point.clone(),
                line,
                value: *value,
                side: *side,
                driving: *driving,
            },
            None => return Ok(None),
        },
        // A round's edge tangency, to the piece touching the contact; refused if none does
        // (decision 2). A line's is on every piece (copied).
        Form::Tangent { a, b, side } if !is_line && (a == curve || b == curve) => {
            let other = if a == curve { b } else { a };
            let piece = match contact(sketch, placing.geo, other, *side) {
                Some(xy) => placing.touching(xy),
                None => Some(curve.clone()),
            };
            let Some(piece) = piece else {
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "trimming {} {curve} there would leave constraint {}, an edge tangency with {other}, on no piece of it",
                        original.noun, constraint.id
                    ),
                )
                .with_elements([constraint.id.to_string(), curve.to_string()])
                .with_parameter("pick")
                .with_cause(format!("no piece of the {} touches {other}", original.noun))
                .with_suggestion("remove the constraint first, or trim elsewhere"));
            };
            let swap = |r: &Reference| if r == curve { piece.clone() } else { r.clone() };
            Form::Tangent {
                a: swap(a),
                b: swap(b),
                side: *side,
            }
        }
        // A symmetry about the line, which no piece is when it's trimmed in two (decision 2).
        Form::Symmetric { about, .. } if about == curve && two => {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "trimming line {curve} there would leave constraint {}, a symmetry about it, on no piece of it",
                    constraint.id
                ),
            )
            .with_elements([constraint.id.to_string(), curve.to_string()])
            .with_parameter("pick")
            .with_cause("the trim's pieces are separate lines, and the symmetry is about the whole one")
            .with_suggestion(
                "remove the constraint first, or make the points symmetric about another line",
            ));
        }
        // A tangency at one of the curve's ends: on the piece that keeps the end, else gone.
        Form::TangentOn {
            a,
            at,
            b,
            direction,
        } if a == curve => match at_end(a, at) {
            Some(a) => Form::TangentOn {
                a,
                at: at.clone(),
                b: b.clone(),
                direction: *direction,
            },
            None => return Ok(None),
        },
        Form::TangentAt {
            a,
            b,
            at,
            direction,
        } if a == curve || b == curve => match (at_end(a, at), at_end(b, at)) {
            (Some(a), Some(b)) => Form::TangentAt {
                a,
                b,
                at: at.clone(),
                direction: *direction,
            },
            _ => return Ok(None),
        },
        // Another curve's end on this one: to the piece holding it, dropped with the part
        // removed.
        Form::TangentOn {
            a,
            at,
            b,
            direction,
        } if b == curve => match holding(at) {
            Some(b) => Form::TangentOn {
                a: a.clone(),
                at: at.clone(),
                b,
                direction: *direction,
            },
            None => return Ok(None),
        },
        // The rest stay on the first piece: a line's direction constraints (copied to the
        // second), a round's radius and equal, a symmetry about a line shortened.
        _ => form.clone(),
    }))
}

/// Where an edge tangency of the trimmed curve with `other` touches it, on the side `side`
/// says; none where nothing says (concentric circles, a reference with no geometry).
fn contact(sketch: &Sketch, geo: &Geo, other: &Reference, side: TangentSide) -> Option<[f64; 2]> {
    let (center, radius) = geo.round()?;
    if let Some(line) = Geo::of_reference(sketch, other)
        && let Geo::Line { from, to } = line
    {
        return Some(project_on_line(from, to, center));
    }
    let (other_center, other_radius) = Geo::of_reference(sketch, other)?.round()?;
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

/// A cut: where it is on the trimmed curve, how far along, by which curve, and which of that
/// curve's ends it is, if one is.
#[derive(Debug, Clone)]
struct Cut {
    at: [f64; 2],
    along: f64,
    cutter: ElementId,
    end: Option<ElementId>,
    /// Whether a constraint holds that end on the curve (an `on`, or the cutter's
    /// `tangent_on`), so the new end shares it (decision 5).
    held: bool,
}

impl Cut {
    /// How the trim ranks the cuts at one place: a held end, which the new end shares,
    /// before any other.
    fn rank(&self) -> bool {
        !self.held
    }
}

/// Which side of the pick a cut is on, along the curve: the piece before it ends there; the
/// one after starts there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Before,
    After,
}

/// A join at a cut: the cutter's end shared, or none for a new point; and the cutter's edge
/// tangency with the curve it turns into an end tangency there.
struct Join {
    cut: Cut,
    side: Side,
    shared: Option<ElementId>,
    tangent: Option<ElementId>,
    /// The line's perpendicular with the cutter, which goes to this cut's piece alone, as
    /// FreeCAD's end-to-edge perpendicular at the cut (`transformPreexistingConstraintForTrim`).
    perpendicular: Option<ElementId>,
}

/// Every cut on `curve` (`geo`), in order along it: each other curve's crossings, tangent
/// contacts, and ends on it, at most one of each cutter within 1e-7 mm (its end kept, which
/// may be shared), none within 1e-7 mm of the curve's own ends.
fn cuts(sketch: &Sketch, curve: &ElementId, geo: &Geo) -> Vec<Cut> {
    // The curve's own ends, as stored and on its carrier (an arc's end the solve left off its
    // circle is at its angle there too).
    let own_ends: Vec<[f64; 2]> = geo
        .ends()
        .into_iter()
        .flat_map(|end| [end, geo.project(end)])
        .collect();
    let curve_ref = Reference::Element(curve.clone());
    let held = |cutter: &ElementId, end: &ElementId| {
        sketch.constraints.iter().any(|c| match &c.form {
            Form::On { point, curve } => point.element() == Some(end) && *curve == curve_ref,
            Form::TangentOn { a, at, b, .. } => {
                a.element() == Some(cutter) && at.element() == Some(end) && *b == curve_ref
            }
            _ => false,
        })
    };
    let mut cuts: Vec<Cut> = Vec::new();
    let others = super::curves(sketch);
    for (_, cutter, _) in others.iter().filter(|(_, id, _)| *id != curve) {
        let Some(other) = Geo::of(sketch, cutter) else {
            continue;
        };
        let mut found: Vec<Cut> = Vec::new();
        // Its ends on the curve.
        for end in end_ids(sketch, cutter) {
            let Some(xy) = point_at(sketch, &end) else {
                continue;
            };
            let at = geo.project(xy);
            if geo.off(xy) <= LENGTH_TOLERANCE && geo.spans(at) {
                found.push(Cut {
                    at,
                    along: geo.along(at),
                    cutter: (*cutter).clone(),
                    held: held(cutter, &end),
                    end: Some(end),
                });
            }
        }
        // Its crossings and contacts with it, where both span them.
        for xy in crossings(geo, &other) {
            let at = geo.project(xy);
            if geo.spans(at) && other.spans(other.project(xy)) {
                let near_found = found.iter().any(|c| distance(c.at, at) <= LENGTH_TOLERANCE);
                if !near_found {
                    found.push(Cut {
                        at,
                        along: geo.along(at),
                        cutter: (*cutter).clone(),
                        end: None,
                        held: false,
                    });
                }
            }
        }
        cuts.extend(found.into_iter().filter(|cut| {
            !own_ends
                .iter()
                .any(|end| distance(cut.at, *end) <= LENGTH_TOLERANCE)
        }));
    }
    cuts.sort_by(|a, b| {
        a.along
            .partial_cmp(&b.along)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.cutter.cmp(&b.cutter))
    });
    cuts
}

/// A line's or arc's two end points' IDs; none for a circle.
pub(crate) fn end_ids(sketch: &Sketch, curve: &ElementId) -> Vec<ElementId> {
    if let Some(line) = sketch.lines.iter().find(|l| &l.id == curve) {
        return vec![line.from.clone(), line.to.clone()];
    }
    sketch
        .arcs
        .iter()
        .find(|a| &a.id == curve)
        .map(|a| vec![a.start.clone(), a.end.clone()])
        .unwrap_or_default()
}

/// Where the carriers of `a` and `b` (a line through its ends, a round's whole circle)
/// meet: crossings, and a tangent contact where the gap is at most 1e-7 mm. None where they
/// overlap: an overlap's cuts are the ends of each on the other.
pub(crate) fn crossings(a: &Geo, b: &Geo) -> Vec<[f64; 2]> {
    match (a, b) {
        (Geo::Line { from: p, to: q }, Geo::Line { from: r, to: s }) => {
            let collinear = [*r, *s]
                .iter()
                .all(|x| distance(project_on_line(*p, *q, *x), *x) <= LENGTH_TOLERANCE);
            let (d1, d2) = ([q[0] - p[0], q[1] - p[1]], [s[0] - r[0], s[1] - r[1]]);
            let det = d1[0] * d2[1] - d1[1] * d2[0];
            if collinear || det == 0.0 {
                return Vec::new();
            }
            let t = ((r[0] - p[0]) * d2[1] - (r[1] - p[1]) * d2[0]) / det;
            vec![[p[0] + t * d1[0], p[1] + t * d1[1]]]
        }
        (Geo::Line { from, to }, round) | (round, Geo::Line { from, to }) => {
            let Some((center, radius)) = round.round() else {
                return Vec::new();
            };
            let foot = project_on_line(*from, *to, center);
            let away = distance(foot, center);
            if (away - radius).abs() <= LENGTH_TOLERANCE {
                return vec![foot];
            }
            if away > radius {
                return Vec::new();
            }
            let half = (radius * radius - away * away).sqrt();
            let length = distance(*from, *to);
            let unit = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
            vec![
                [foot[0] - half * unit[0], foot[1] - half * unit[1]],
                [foot[0] + half * unit[0], foot[1] + half * unit[1]],
            ]
        }
        (one, other) => {
            let (Some((c1, r1)), Some((c2, r2))) = (one.round(), other.round()) else {
                return Vec::new();
            };
            let apart = distance(c1, c2);
            if apart <= LENGTH_TOLERANCE && (r1 - r2).abs() <= LENGTH_TOLERANCE || apart == 0.0 {
                return Vec::new();
            }
            let unit = [(c2[0] - c1[0]) / apart, (c2[1] - c1[1]) / apart];
            let outside = (apart - (r1 + r2)).abs() <= LENGTH_TOLERANCE;
            let inside = (apart - (r1 - r2).abs()).abs() <= LENGTH_TOLERANCE;
            if outside || inside {
                // Toward the other's centre, unless this one is inside it.
                let toward = if inside && r1 < r2 { -r1 } else { r1 };
                return vec![[c1[0] + unit[0] * toward, c1[1] + unit[1] * toward]];
            }
            if apart > r1 + r2 || apart < (r1 - r2).abs() {
                return Vec::new();
            }
            let along = (apart * apart + r1 * r1 - r2 * r2) / (2.0 * apart);
            let half = (r1 * r1 - along * along).max(0.0).sqrt();
            let base = [c1[0] + along * unit[0], c1[1] + along * unit[1]];
            vec![
                [base[0] - half * unit[1], base[1] + half * unit[0]],
                [base[0] + half * unit[1], base[1] - half * unit[0]],
            ]
        }
    }
}

/// A curve as its stored points place it.
#[derive(Debug, Clone)]
pub(crate) enum Geo {
    Line {
        from: [f64; 2],
        to: [f64; 2],
    },
    /// Counter-clockwise from `start_angle` to `end_angle`, more by less than a turn.
    Arc {
        center: [f64; 2],
        radius: f64,
        start: [f64; 2],
        end: [f64; 2],
        start_angle: f64,
        end_angle: f64,
    },
    Circle {
        center: [f64; 2],
        radius: f64,
    },
}

impl Geo {
    pub(crate) fn of(sketch: &Sketch, id: &ElementId) -> Option<Geo> {
        if let Some(line) = sketch.lines.iter().find(|l| &l.id == id) {
            return Some(Geo::Line {
                from: point_at(sketch, &line.from)?,
                to: point_at(sketch, &line.to)?,
            });
        }
        if let Some(arc) = sketch.arcs.iter().find(|a| &a.id == id) {
            let (center, start, end) = (
                point_at(sketch, &arc.center)?,
                point_at(sketch, &arc.start)?,
                point_at(sketch, &arc.end)?,
            );
            let [radius, start_angle, end_angle] = arc_from_points(center, start, end)?;
            return Some(Geo::Arc {
                center,
                radius,
                start,
                end,
                start_angle,
                end_angle,
            });
        }
        let circle = sketch.circles.iter().find(|c| &c.id == id)?;
        Some(Geo::Circle {
            center: point_at(sketch, &circle.center)?,
            radius: circle.radius.mm(),
        })
    }

    /// A reference's: a curve's, or an axis's line.
    fn of_reference(sketch: &Sketch, reference: &Reference) -> Option<Geo> {
        match reference {
            Reference::XAxis => Some(Geo::Line {
                from: [0.0, 0.0],
                to: [1.0, 0.0],
            }),
            Reference::YAxis => Some(Geo::Line {
                from: [0.0, 0.0],
                to: [0.0, 1.0],
            }),
            Reference::Element(id) => Geo::of(sketch, id),
            Reference::Origin => None,
        }
    }

    fn round(&self) -> Option<([f64; 2], f64)> {
        match self {
            Geo::Arc { center, radius, .. } | Geo::Circle { center, radius } => {
                Some((*center, *radius))
            }
            Geo::Line { .. } => None,
        }
    }

    fn ends(&self) -> Vec<[f64; 2]> {
        match self {
            Geo::Line { from, to } => vec![*from, *to],
            Geo::Arc { start, end, .. } => vec![*start, *end],
            Geo::Circle { .. } => Vec::new(),
        }
    }

    /// `point`'s projection onto the carrier; a round's centre stays where it is.
    pub(crate) fn project(&self, point: [f64; 2]) -> [f64; 2] {
        match self {
            Geo::Line { from, to } => project_on_line(*from, *to, point),
            Geo::Arc { center, radius, .. } | Geo::Circle { center, radius } => {
                let away = distance(*center, point);
                if away == 0.0 {
                    return point;
                }
                let scale = radius / away;
                [
                    center[0] + (point[0] - center[0]) * scale,
                    center[1] + (point[1] - center[1]) * scale,
                ]
            }
        }
    }

    /// How far `point` is from the carrier: from the line through a line's ends, from a
    /// round's circle (a round's centre is a radius away).
    pub(crate) fn off(&self, point: [f64; 2]) -> f64 {
        match self {
            Geo::Line { from, to } => distance(project_on_line(*from, *to, point), point),
            Geo::Arc { center, radius, .. } | Geo::Circle { center, radius } => {
                (distance(*center, point) - radius).abs()
            }
        }
    }

    /// How far along the curve a point on its carrier is: a line's fraction from `from`;
    /// a round's angle, an arc's from its start angle, in [start, start + a turn), a
    /// circle's in [0, a turn).
    fn along(&self, point: [f64; 2]) -> f64 {
        match self {
            Geo::Line { from, to } => line_parameter(*from, *to, point),
            Geo::Arc {
                center,
                start_angle,
                ..
            } => {
                let angle = libm::atan2(point[1] - center[1], point[0] - center[0]);
                start_angle + (angle - start_angle).rem_euclid(TAU)
            }
            Geo::Circle { center, .. } => {
                libm::atan2(point[1] - center[1], point[0] - center[0]).rem_euclid(TAU)
            }
        }
    }

    /// Whether a point on the carrier is on the curve, or within 1e-7 mm of its ends.
    pub(crate) fn spans(&self, point: [f64; 2]) -> bool {
        let near_end = self
            .ends()
            .iter()
            .any(|end| distance(*end, point) <= LENGTH_TOLERANCE);
        near_end
            || match self {
                Geo::Line { .. } => (0.0..=1.0).contains(&self.along(point)),
                Geo::Arc { end_angle, .. } => self.along(point) <= *end_angle,
                Geo::Circle { .. } => true,
            }
    }
}

/// Where the trim cuts: the curve removed whole (none), an open curve's cuts either side of
/// the pick, or a closed one's two around it.
enum Layout {
    Open {
        before: Option<Cut>,
        after: Option<Cut>,
    },
    /// The arc left runs counter-clockwise from `after` to `before`.
    Closed { before: Cut, after: Cut },
}

impl Layout {
    fn of(geo: &Geo, cuts: &[Cut], picked: f64) -> Option<Layout> {
        if let Geo::Circle { .. } = geo {
            // A cut at the pick is behind it: the part removed runs counter-clockwise from
            // the pick, as FreeCAD's (1.1.3, a circle picked at its cut).
            let behind = |c: &Cut| (picked - c.along).rem_euclid(TAU);
            let ahead = |c: &Cut| match (c.along - picked).rem_euclid(TAU) {
                0.0 => TAU,
                ahead => ahead,
            };
            let nearest = |key: &dyn Fn(&Cut) -> f64| {
                cuts.iter()
                    .min_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(Ordering::Equal))
                    .cloned()
            };
            let (before, after) = (
                best_at(cuts, &nearest(&behind)?),
                best_at(cuts, &nearest(&ahead)?),
            );
            // A circle with fewer than two distinct cuts goes whole (`testTrimCircleEnd`).
            if distance(before.at, after.at) <= LENGTH_TOLERANCE {
                return None;
            }
            return Some(Layout::Closed { before, after });
        }
        // A cut at the pick is none, as FreeCAD's (1.1.3: picked at a line's cut, the trim
        // runs on to the next either side).
        let before = cuts
            .iter()
            .rev()
            .find(|c| c.along < picked)
            .map(|c| best_at(cuts, c));
        let after = cuts
            .iter()
            .find(|c| c.along > picked)
            .map(|c| best_at(cuts, c));
        match (&before, &after) {
            (None, None) => None,
            (Some(b), Some(a)) if distance(b.at, a.at) <= LENGTH_TOLERANCE => None,
            _ => Some(Layout::Open { before, after }),
        }
    }

    /// The cuts used, in order along what's left: the start-side piece's end, then the
    /// other's start (a circle's arc: its start, then its end).
    fn cuts(&self) -> Vec<(&Cut, Side)> {
        match self {
            Layout::Open { before, after } => before
                .iter()
                .map(|c| (c, Side::Before))
                .chain(after.iter().map(|c| (c, Side::After)))
                .collect(),
            Layout::Closed { before, after } => vec![(after, Side::After), (before, Side::Before)],
        }
    }

    fn two_pieces(&self) -> bool {
        matches!(
            self,
            Layout::Open {
                before: Some(_),
                after: Some(_)
            }
        )
    }

    fn keeps_start(&self) -> bool {
        matches!(
            self,
            Layout::Open {
                before: Some(_),
                ..
            }
        )
    }

    fn keeps_end(&self) -> bool {
        matches!(self, Layout::Open { after: Some(_), .. })
    }
}

/// Of the cuts within 1e-7 mm of `cut`, the nearest nearest-ranked ([`Cut::rank`]): `cut`
/// itself, unless one there is a held end and it isn't.
fn best_at(cuts: &[Cut], cut: &Cut) -> Cut {
    cuts.iter()
        .filter(|c| distance(c.at, cut.at) <= LENGTH_TOLERANCE)
        .min_by(|a, b| {
            a.rank().cmp(&b.rank()).then_with(|| {
                distance(a.at, cut.at)
                    .partial_cmp(&distance(b.at, cut.at))
                    .unwrap_or(Ordering::Equal)
            })
        })
        .unwrap_or(cut)
        .clone()
}

/// Which piece a point of the trimmed curve's carrier lands on.
struct Placing<'a> {
    geo: &'a Geo,
    layout: &'a Layout,
    first: Reference,
    second: Option<Reference>,
}

impl Placing<'_> {
    /// The piece whose span holds `xy`'s projection, as [`holding`](Self::holding), but none
    /// past the curve's ends: a contact there touches no piece (decision 2).
    fn touching(&self, xy: [f64; 2]) -> Option<Reference> {
        if self.past(self.geo.along(self.geo.project(xy))) {
            None
        } else {
            self.holding(xy)
        }
    }

    /// Whether a place `along` the carrier is past the curve's ends.
    fn past(&self, along: f64) -> bool {
        match (self.layout, self.geo) {
            (Layout::Closed { .. }, _) | (_, Geo::Circle { .. }) => false,
            (_, Geo::Line { .. }) => !(0.0..=1.0).contains(&along),
            (_, Geo::Arc { end_angle, .. }) => along > *end_angle,
        }
    }

    /// The piece whose span holds `xy`'s projection, or past the curve's ends the piece
    /// that keeps the nearer end (a line's projection clamps to its ends, as FreeCAD's
    /// `closestParameter` does); none on the part removed, or past an end trimmed away.
    fn holding(&self, xy: [f64; 2]) -> Option<Reference> {
        let at = self.geo.project(xy);
        let along = self.geo.along(at);
        match self.layout {
            Layout::Closed { before, after } => {
                let kept = (along - after.along).rem_euclid(TAU)
                    <= (before.along - after.along).rem_euclid(TAU);
                kept.then(|| self.first.clone())
            }
            Layout::Open { before, after } => {
                if self.past(along) {
                    let ends = self.geo.ends();
                    let nearer_start = match &ends[..] {
                        [start, end] => distance(xy, *start) <= distance(xy, *end),
                        _ => true,
                    };
                    return match (nearer_start, before, after) {
                        (true, Some(_), _) => Some(self.first.clone()),
                        (false, _, Some(_)) => {
                            Some(self.second.clone().unwrap_or_else(|| self.first.clone()))
                        }
                        _ => None,
                    };
                }
                match (before, after) {
                    (Some(b), Some(a)) => {
                        if along <= b.along {
                            Some(self.first.clone())
                        } else if along >= a.along {
                            self.second.clone()
                        } else {
                            None
                        }
                    }
                    (Some(b), None) => (along <= b.along).then(|| self.first.clone()),
                    (None, Some(a)) => (along >= a.along).then(|| self.first.clone()),
                    (None, None) => None,
                }
            }
        }
    }
}

/// The trimmed curve as the sketch holds it.
struct Original {
    id: ElementId,
    noun: &'static str,
    construction: bool,
    /// A line's `from` or an arc's `start`; none for a circle.
    start: Option<ElementId>,
    end: Option<ElementId>,
}

impl Original {
    fn of(sketch: &Sketch, curve: &ElementId) -> Result<Original, Error> {
        if let Some(line) = sketch.lines.iter().find(|l| &l.id == curve) {
            return Ok(Original {
                id: curve.clone(),
                noun: "line",
                construction: line.construction,
                start: Some(line.from.clone()),
                end: Some(line.to.clone()),
            });
        }
        if let Some(arc) = sketch.arcs.iter().find(|a| &a.id == curve) {
            return Ok(Original {
                id: curve.clone(),
                noun: "arc",
                construction: arc.construction,
                start: Some(arc.start.clone()),
                end: Some(arc.end.clone()),
            });
        }
        match sketch.circles.iter().find(|c| &c.id == curve) {
            Some(circle) => Ok(Original {
                id: curve.clone(),
                noun: "circle",
                construction: circle.construction,
                start: None,
                end: None,
            }),
            None => Err(no_member(curve, ElementKind::Line, "curve")),
        }
    }

    fn end_points(&self) -> Vec<ElementId> {
        self.start.iter().chain(&self.end).cloned().collect()
    }

    /// Cuts the curve in `edited` to the layout's pieces, the new ends at `ends` (in
    /// [`Layout::cuts`]' order), and gives the pieces, the first keeping the curve's ID.
    fn cut(
        &self,
        edited: &mut Sketch,
        kinds: &mut std::collections::BTreeMap<ElementId, ElementKind>,
        layout: &Layout,
        ends: &[ElementId],
        second: Option<&ElementId>,
    ) -> Result<Vec<ElementId>, Error> {
        let missing = || no_member(&self.id, ElementKind::Line, "curve");
        let mut pieces = vec![self.id.clone()];
        match (layout, ends) {
            (Layout::Closed { .. }, [start, end]) => {
                let place = edited
                    .circles
                    .iter()
                    .position(|c| c.id == self.id)
                    .ok_or_else(missing)?;
                let circle = edited.circles.remove(place);
                edited.arcs.push(SketchArc {
                    id: self.id.clone(),
                    center: circle.center,
                    start: start.clone(),
                    end: end.clone(),
                    construction: circle.construction,
                });
                kinds.insert(self.id.clone(), ElementKind::Arc);
            }
            (Layout::Open { before, after }, _) => {
                let (cut_before, cut_after) = match (before, after, ends) {
                    (Some(_), Some(_), [b, a]) => (Some(b), Some(a)),
                    (Some(_), None, [b]) => (Some(b), None),
                    (None, Some(_), [a]) => (None, Some(a)),
                    _ => return Err(missing()),
                };
                if let Some(line) = edited.lines.iter_mut().find(|l| l.id == self.id) {
                    let to = line.to.clone();
                    if let Some(b) = cut_before {
                        line.to = b.clone();
                    }
                    match (cut_after, second) {
                        (Some(a), Some(second)) => {
                            let piece = SketchLine {
                                id: second.clone(),
                                from: a.clone(),
                                to,
                                construction: self.construction,
                            };
                            edited.lines.push(piece);
                            kinds.insert(second.clone(), ElementKind::Line);
                            pieces.push(second.clone());
                        }
                        (Some(a), None) => line.from = a.clone(),
                        _ => {}
                    }
                } else if let Some(arc) = edited.arcs.iter_mut().find(|a| a.id == self.id) {
                    let (center, end) = (arc.center.clone(), arc.end.clone());
                    if let Some(b) = cut_before {
                        arc.end = b.clone();
                    }
                    match (cut_after, second) {
                        (Some(a), Some(second)) => {
                            edited.arcs.push(SketchArc {
                                id: second.clone(),
                                center,
                                start: a.clone(),
                                end,
                                construction: self.construction,
                            });
                            kinds.insert(second.clone(), ElementKind::Arc);
                            pieces.push(second.clone());
                        }
                        (Some(a), None) => arc.start = a.clone(),
                        _ => {}
                    }
                } else {
                    return Err(missing());
                }
            }
            _ => return Err(missing()),
        }
        Ok(pieces)
    }
}
