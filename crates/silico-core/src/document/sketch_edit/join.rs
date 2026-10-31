//! Joining a split's two pieces back into one curve (docs/design/m5.0-app.md, open item 12b
//! and `sketch_join`; docs/ports/sketch-edit.md, §3, *Join*): split's exact inverse, as
//! SolidWorks' Delete on a split point gives one line or one arc. Nothing is placed: the
//! kept piece takes the other's far end, a point the sketch already holds, so no coordinate
//! moves; a split circle's two arcs become its circle again, its radius its kept arc's.

use std::collections::BTreeSet;

use silico_protocol::arc_from_points;

use super::split::point_at;
use super::{
    Edited, curves, implied_ons, kinds_of, listed, no_member, ordered, own_curve, references_mut,
};
use crate::document::sketch::{
    Coordinate, Direction, ElementId, ElementKind, Form, Reference, Sketch, SketchCircle,
    SketchConstraint,
};
use crate::{Error, ErrorCode};

/// Two pieces joined: the edit, and the pieces, the start-side one first, whose ID the
/// joined curve keeps, then the one removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    pub edited: Edited,
    pub pieces: [ElementId; 2],
}

/// What a point is to a curve that uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Start,
    End,
    Centre,
}

/// Joins the two pieces that share `point` into one curve, as a split left them (open item
/// 12b): two lines held on one line by a `tangent_at` (`same`) at the point, or two arcs
/// about one centre. The piece that ends at the point (the start-side one) keeps its ID and
/// takes the other's far end; a split circle's two arcs, which share both their ends, become
/// one circle again, of the kept arc's radius, as split made two arcs of it. The other
/// piece, the point (and a circle's other shared point), and every constraint naming a point
/// removed (the joining `tangent_at` among them) are removed. Every constraint naming the
/// other piece names the kept one instead, listed in `modified`; one that then repeats
/// another (a `horizontal` on each piece), or names the joined curve twice (an `equal` of
/// the two pieces), is dropped, listed in `removed`, as is an `on` that then holds an end of
/// the joined curve on it, implied. Nothing is retyped: a split's `distance` of the outer
/// ends stays one, exact on one line.
///
/// Refused, as data naming the point and the pieces: a member that isn't a point of the
/// sketch; a point fewer or more than two curves use, or one that's a curve's centre; a line
/// and an arc; construction joined to non-construction (FreeCAD's join refuses it); pieces
/// that don't run on through the point, one ending there and the other starting there; lines
/// no `tangent_at` (`same`) holds on one line there, and arcs with no centre in common (not
/// held on one carrier); lines whose join would end at one point at both ends (a curve's own
/// two ends, FreeCAD's refusal); arcs that sweep a turn or more between them (the joined
/// arc's own sweep no more than a piece's: see [`one_arc`]), or one that sweeps no angle; a
/// circle's two arcs whose other shared point another curve uses; a piece's `length`,
/// `midpoint`, or line's `equal`, which the joined line wouldn't mean, naming the
/// constraint; a constraint the join would make hold the joined curve's own point on it,
/// other than an implied `on`; and a joined sketch the gate refuses.
pub fn join(sketch: &Sketch, point: &ElementId) -> Result<Joined, Error> {
    let kinds = kinds_of(sketch);
    let p = point.to_string();
    let refused = |message: String, elements: Vec<String>, suggestion: &str| {
        Error::new(ErrorCode::InvalidInput, message)
            .with_elements(elements)
            .with_parameter("point")
            .with_suggestion(suggestion.to_owned())
    };
    match kinds.get(point) {
        None => return Err(no_member(point, ElementKind::Point, "point")),
        Some(ElementKind::Point) => {}
        Some(other) => {
            return Err(refused(
                format!(
                    "{} {point} isn't a point: a join takes the point two pieces share",
                    other.noun()
                ),
                vec![p],
                "name the point a split left between two pieces",
            ));
        }
    }
    let sharing: Vec<(ElementKind, &ElementId, Role)> = curves(sketch)
        .into_iter()
        .filter_map(|(kind, id, points)| {
            let place = points.iter().position(|q| *q == point)?;
            let role = match (kind, place) {
                (ElementKind::Line, 0) | (ElementKind::Arc, 1) => Role::Start,
                (ElementKind::Line, _) | (ElementKind::Arc, 2) => Role::End,
                _ => Role::Centre,
            };
            Some((kind, id, role))
        })
        .collect();
    let named = |kind: ElementKind, id: &ElementId| format!("{} {id}", kind.noun());
    let [(kind_a, a, role_a), (kind_b, b, role_b)] = match sharing.as_slice() {
        [] => {
            return Err(refused(
                format!("point {point} is used by no curve, so there's nothing to join"),
                vec![p],
                "name a point two pieces share",
            ));
        }
        [(kind, curve, _)] => {
            return Err(refused(
                format!(
                    "point {point} is used by no curve but {}, so there's nothing to join",
                    named(*kind, curve)
                ),
                vec![p, curve.to_string()],
                "name a point two pieces share",
            ));
        }
        [first, second] => [*first, *second],
        more => {
            let said: Vec<String> = more.iter().map(|(k, id, _)| named(*k, id)).collect();
            return Err(refused(
                format!(
                    "point {point} is shared by {}: a join takes a point only its two pieces use",
                    listed(&said)
                ),
                std::iter::once(p)
                    .chain(more.iter().map(|(_, id, _)| id.to_string()))
                    .collect(),
                "unjoin the other curves from it first",
            ));
        }
    };
    let pair = || vec![p.clone(), a.to_string(), b.to_string()];
    for (kind, curve, role) in [(kind_a, a, role_a), (kind_b, b, role_b)] {
        if role == Role::Centre {
            return Err(refused(
                format!(
                    "point {point} is {}'s centre: a join takes a point two pieces end at",
                    named(kind, curve)
                ),
                vec![p, curve.to_string()],
                "name the point a split left between two pieces",
            ));
        }
    }
    if kind_a != kind_b {
        return Err(refused(
            format!(
                "{} and {} meet at point {point}, but a line and an arc are never one curve",
                named(kind_a, a),
                named(kind_b, b)
            ),
            pair(),
            "join two lines held on one line, or two arcs about one centre",
        ));
    }
    let construction = |id: &ElementId| {
        sketch
            .lines
            .iter()
            .map(|l| (&l.id, l.construction))
            .chain(sketch.arcs.iter().map(|a| (&a.id, a.construction)))
            .find(|(curve, _)| *curve == id)
            .is_some_and(|(_, construction)| construction)
    };
    if construction(a) != construction(b) {
        let (yes, no) = if construction(a) { (a, b) } else { (b, a) };
        return Err(refused(
            format!(
                "{} is construction and {} isn't: a join makes one curve, construction or not",
                named(kind_a, yes),
                named(kind_a, no)
            ),
            pair(),
            "make both pieces construction, or neither, then join",
        ));
    }
    let nouns = format!("{}s", kind_a.noun());
    let (kept, other) = match (role_a, role_b) {
        (Role::End, Role::Start) => (a, b),
        (Role::Start, Role::End) => (b, a),
        (role, _) => {
            let how = if role == Role::Start { "start" } else { "end" };
            return Err(refused(
                format!(
                    "{nouns} {a} and {b} both {how} at point {point}: a join takes a piece that ends there and one that starts there"
                ),
                pair(),
                "join pieces that run on through the point, as a split leaves them",
            ));
        }
    };
    let (kept_ref, other_ref) = (
        Reference::Element(kept.clone()),
        Reference::Element(other.clone()),
    );
    let at_point = Reference::Element(point.clone());
    // The far ends: the kept piece's start, and the other's end, which the joined curve
    // takes; and, for arcs, whether they're a circle's two (sharing both ends).
    let mut gone_points: BTreeSet<ElementId> = BTreeSet::from([point.clone()]);
    let mut circle = None;
    if kind_a == ElementKind::Line {
        let (Some(kept_line), Some(other_line)) = (
            sketch.lines.iter().find(|l| &l.id == kept),
            sketch.lines.iter().find(|l| &l.id == other),
        ) else {
            return Err(no_member(kept, ElementKind::Line, "point"));
        };
        let held = sketch.constraints.iter().any(|c| match &c.form {
            Form::TangentAt {
                a,
                b,
                at,
                direction: Direction::Same,
            } => {
                *at == at_point
                    && ((*a == kept_ref && *b == other_ref) || (*a == other_ref && *b == kept_ref))
            }
            _ => false,
        });
        if !held {
            return Err(refused(
                format!(
                    "lines {kept} and {other} meet at point {point}, but no tangent_at (same) there holds them on one line"
                ),
                vec![p, kept.to_string(), other.to_string()],
                "add a tangent_at (same) of the two at the point, then join",
            )
            .with_cause(
                "a split holds a line's two pieces on one line by a tangent_at (same) at the point between them, which a join undoes",
            ));
        }
        if kept_line.from == other_line.to {
            return Err(refused(
                format!(
                    "joining lines {kept} and {other} at point {point} would give one line both of whose ends are point {}",
                    kept_line.from
                ),
                vec![
                    p,
                    kept.to_string(),
                    other.to_string(),
                    kept_line.from.to_string(),
                ],
                "remove one of the lines instead",
            ));
        }
    } else {
        let (Some(kept_arc), Some(other_arc)) = (
            sketch.arcs.iter().find(|a| &a.id == kept),
            sketch.arcs.iter().find(|a| &a.id == other),
        ) else {
            return Err(no_member(kept, ElementKind::Arc, "point"));
        };
        if kept_arc.center != other_arc.center {
            return Err(refused(
                format!(
                    "arcs {kept} and {other} meet at point {point}, but have no centre in common, so they aren't held on one circle"
                ),
                vec![p, kept.to_string(), other.to_string()],
                "merge their centres first, then join",
            ));
        }
        let mut swept = Vec::new();
        for arc in [kept_arc, other_arc] {
            let measured = point_at(sketch, &arc.center)
                .zip(point_at(sketch, &arc.start))
                .zip(point_at(sketch, &arc.end))
                .and_then(|((c, s), e)| arc_from_points(c, s, e));
            let Some([radius, start, end]) = measured else {
                return Err(refused(
                    format!("arc {} sweeps no angle, so it can't be joined", arc.id),
                    vec![p, arc.id.to_string()],
                    "remove the arc, or move its ends apart, then join",
                ));
            };
            swept.push((radius, end - start));
        }
        if kept_arc.start == other_arc.end {
            // A split circle's two arcs: its circle again, about their centre, of the kept
            // arc's radius as the profile measures it (from its centre to its start). Their
            // other shared point goes too, so no other curve may use it.
            let other_end = &kept_arc.start;
            let also: Vec<(ElementKind, &ElementId)> = curves(sketch)
                .into_iter()
                .filter(|(_, id, points)| {
                    *id != kept && *id != other && points.contains(&other_end)
                })
                .map(|(kind, id, _)| (kind, id))
                .collect();
            if !also.is_empty() {
                let said: Vec<String> = also.iter().map(|(k, id)| named(*k, id)).collect();
                return Err(refused(
                    format!(
                        "joining arcs {kept} and {other} at point {point} makes them a circle, which would leave {} without point {other_end}, their other shared point",
                        listed(&said)
                    ),
                    [
                        p,
                        kept.to_string(),
                        other.to_string(),
                        other_end.to_string(),
                    ]
                    .into_iter()
                    .chain(also.iter().map(|(_, id)| id.to_string()))
                    .collect(),
                    "unjoin the other curves from that point first",
                ));
            }
            gone_points.insert(kept_arc.start.clone());
            let radius = swept.first().map_or(f64::NAN, |(r, _)| *r);
            let Some(radius) = Coordinate::new(radius) else {
                return Err(refused(
                    format!(
                        "arc {kept}'s radius isn't a finite length, so it can't be joined into a circle"
                    ),
                    vec![p, kept.to_string()],
                    "move the arcs nearer the origin, then join",
                ));
            };
            circle = Some(SketchCircle {
                id: kept.clone(),
                center: kept_arc.center.clone(),
                radius,
                construction: kept_arc.construction,
            });
        } else if !one_arc(
            sketch,
            [&kept_arc.center, &kept_arc.start, &other_arc.end],
            &swept,
        ) {
            return Err(refused(
                format!(
                    "arcs {kept} and {other} sweep a whole turn or more between them, so one arc can't join them at point {point}"
                ),
                vec![p, kept.to_string(), other.to_string()],
                "trim one of them first, then join",
            ));
        }
    }
    // A piece's length, midpoint, or a line's equal measures the piece: on the joined line
    // it would measure the whole, which the person never placed. Refused, naming it, as a
    // trim refuses a constraint no piece can take (decision 2).
    if kind_a == ElementKind::Line {
        let piece = |r: &Reference| *r == kept_ref || *r == other_ref;
        for constraint in &sketch.constraints {
            let what = match &constraint.form {
                Form::Length { line, .. } if piece(line) => {
                    Some((line, format!("line {line}'s length")))
                }
                Form::Midpoint { line, .. } if piece(line) => {
                    Some((line, format!("the midpoint of line {line}")))
                }
                Form::Equal { a, b } if piece(a) != piece(b) => {
                    let (line, with) = if piece(a) { (a, b) } else { (b, a) };
                    Some((line, format!("an equal of line {line} and {with}")))
                }
                _ => None,
            };
            if let Some((line, what)) = what {
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "joining lines {kept} and {other} at point {point} would put constraint {}, {what}, on the whole joined line",
                        constraint.id
                    ),
                )
                .with_elements([constraint.id.to_string(), line.to_string()])
                .with_parameter("point")
                .with_cause(
                    "a piece's length, midpoint, or equal measures the piece, not the line a join makes: a split makes its curve's the outer ends' distance or symmetry, which the joined line keeps",
                )
                .with_suggestion(
                    "remove the constraint first, or dimension the outer ends instead",
                ));
            }
        }
    }
    let mut edited = sketch.clone();
    let mut removed: BTreeSet<ElementId> = gone_points.clone();
    removed.insert(other.clone());
    let mut modified: BTreeSet<ElementId> = BTreeSet::from([kept.clone()]);
    match circle {
        Some(circle) => {
            edited.arcs.retain(|a| &a.id != kept && &a.id != other);
            edited.circles.push(circle);
        }
        None => {
            let far = sketch
                .lines
                .iter()
                .find(|l| &l.id == other)
                .map(|l| l.to.clone())
                .or_else(|| {
                    sketch
                        .arcs
                        .iter()
                        .find(|a| &a.id == other)
                        .map(|a| a.end.clone())
                });
            let Some(far) = far else {
                return Err(no_member(other, kind_a, "point"));
            };
            for line in edited.lines.iter_mut().filter(|l| &l.id == kept) {
                line.to = far.clone();
            }
            for arc in edited.arcs.iter_mut().filter(|a| &a.id == kept) {
                arc.end = far.clone();
            }
            edited.lines.retain(|l| &l.id != other);
            edited.arcs.retain(|a| &a.id != other);
        }
    }
    edited.points.retain(|q| !gone_points.contains(&q.id));
    // The constraints: each naming a point removed goes; each naming the other piece names
    // the kept one, unless it then names the joined curve twice or repeats one kept.
    let names_gone = |form: &Form| {
        form.references()
            .iter()
            .any(|(_, r, _)| r.element().is_some_and(|id| gone_points.contains(id)))
    };
    let mut repointed: Vec<Option<Form>> = Vec::with_capacity(sketch.constraints.len());
    for constraint in &sketch.constraints {
        let form = &constraint.form;
        if names_gone(form) || !form.references().iter().any(|(_, r, _)| **r == other_ref) {
            repointed.push(None);
            continue;
        }
        let mut form = form.clone();
        for reference in references_mut(&mut form) {
            if *reference == other_ref {
                *reference = kept_ref.clone();
            }
        }
        repointed.push(Some(form));
    }
    // What a re-pointed constraint may repeat: any constraint of the sketch, or one
    // re-pointed before it, each by its form written out, in a sorted set. Only one that
    // names the kept piece and no point removed can be repeated, since a re-pointed form
    // names the kept piece, never the other, and no point removed; the rest never match.
    let mut seen: BTreeSet<String> = sketch
        .constraints
        .iter()
        .map(|c| unordered(&c.form))
        .collect();
    let mut constraints = Vec::with_capacity(sketch.constraints.len());
    for (constraint, again) in sketch.constraints.iter().zip(repointed) {
        if names_gone(&constraint.form) {
            removed.insert(constraint.id.clone());
            continue;
        }
        let Some(form) = again else {
            constraints.push(constraint.clone());
            continue;
        };
        let twice = form
            .references()
            .iter()
            .filter(|(_, r, _)| **r == kept_ref)
            .count()
            > 1;
        if twice || !seen.insert(unordered(&form)) {
            removed.insert(constraint.id.clone());
            continue;
        }
        modified.insert(constraint.id.clone());
        constraints.push(SketchConstraint {
            id: constraint.id.clone(),
            form,
        });
    }
    edited.constraints = constraints;
    let implied = implied_ons(&mut edited);
    modified.retain(|id| !implied.contains(id));
    removed.extend(implied);
    own_curve(&edited, "the join", "point", "leave the pieces apart")?;
    crate::document::refused_by_gate(
        &edited,
        "the join",
        vec![p, kept.to_string(), other.to_string()],
        "point",
        "change what the gate names, then join",
    )?;
    let after = kinds_of(&edited);
    Ok(Joined {
        edited: Edited {
            sketch: edited,
            added: Vec::new(),
            modified: ordered(&after, modified),
            removed: ordered(&kinds, removed),
        },
        pieces: [kept.clone(), other.clone()],
    })
}

/// Whether two arcs about `center`, of sweeps `swept`, one from `start` to the point they
/// share and the other on from there to `end`, are one arc from `start` to `end`, by one
/// rule: that arc's own sweep, as the document measures it (the angles of `start` and `end`
/// about `center`, by `libm::atan2`, as [`arc_from_points`] gives them), exceeds each
/// piece's. Pieces whose sweeps sum to less than a turn make an arc sweeping their sum,
/// which exceeds each; pieces sweeping a turn or more make, from those two ends, the sliver
/// left over, or no arc at all, which never does, however rounding falls near a whole turn.
fn one_arc(sketch: &Sketch, [center, start, end]: [&ElementId; 3], swept: &[(f64, f64)]) -> bool {
    let joined = point_at(sketch, center)
        .zip(point_at(sketch, start))
        .zip(point_at(sketch, end))
        .and_then(|((c, s), e)| arc_from_points(c, s, e));
    joined.is_some_and(|[_, from, to]| swept.iter().all(|(_, sweep)| to - from > *sweep))
}

/// `form` with the two curves of a constraint whose order means nothing (two lines parallel
/// or perpendicular, two curves equal) in one order, written out, so a repeat in the other
/// order is found. Only a constraint naming a curve is ever re-pointed, so only these, of
/// the forms whose two members may be swapped, can come to repeat another (the points'
/// forms, `horizontal_points`, `vertical_points`, and `distance`, never change).
fn unordered(form: &Form) -> String {
    let mut form = form.clone();
    match &mut form {
        Form::Parallel { a, b } | Form::Perpendicular { a, b } | Form::Equal { a, b } => {
            if b < a {
                std::mem::swap(a, b);
            }
        }
        _ => {}
    }
    format!("{form:?}")
}
