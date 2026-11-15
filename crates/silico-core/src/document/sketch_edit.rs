//! What each sketch edit does to a sketch's geometry and constraints (docs/ports/sketch-edit.md,
//! whose rules are FreeCAD's behavior with Silico's shared points, M3.2's D2): pure
//! functions from a sketch to the sketch edited, or the reasons it can't be, as errors
//! naming its members. Each result is a sketch the validation gate then checks (a merge
//! checks its own, refusing what the gate would), and a solve then settles (the edit's
//! command, M5.10, which this doesn't hold).
//!
//! Here: removing members (the note's §2, decision 7), pulling a shared point apart (§5,
//! decision 9), and merging points (§6, decision 10). Splitting, joining a split's pieces
//! again, and trimming follow.

use std::collections::{BTreeMap, BTreeSet};

use super::sketch::{
    Direction, ElementId, ElementKind, Form, Reference, Sketch, SketchPoint, TangentSide,
};
use super::{PROBLEMS_LISTED, nearest_ids, with_candidates};
use crate::{Error, ErrorCode, listed, listed_first};

mod add;
mod join;
mod split;
mod trim;
pub use add::{Members, add};
pub use join::{Joined, join};
pub use split::{Split, split, split_with};
pub(crate) use trim::{Geo, crossings, end_ids, lies_along};
pub use trim::{Trim, trim, trim_with};

/// A sketch edited, and what changed (docs/design/m5.0-app.md, section 7, *The sketch
/// edits*): `added` in the order the edit made them, by kind (points, lines, circles, arcs,
/// constraints), then along the curve; `modified` and `removed` by kind, then by ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edited {
    pub sketch: Sketch,
    /// Members the edit made.
    pub added: Vec<ElementId>,
    /// Members the edit changed in place (a curve's point, a constraint's reference).
    pub modified: Vec<ElementId>,
    /// Members the edit removed.
    pub removed: Vec<ElementId>,
}

/// What [`remove`] can be asked to take: a member, by ID, or one of the sketch's fixed
/// references, which it refuses in its own words, never as a member it doesn't know.
pub trait Named {
    /// The member's ID; none for a fixed reference.
    fn member(&self) -> Option<&ElementId>;
    /// As the request names it.
    fn text(&self) -> &str;
}

impl Named for ElementId {
    fn member(&self) -> Option<&ElementId> {
        Some(self)
    }
    fn text(&self) -> &str {
        self.as_str()
    }
}

impl Named for Reference {
    fn member(&self) -> Option<&ElementId> {
        self.element()
    }
    fn text(&self) -> &str {
        self.as_str()
    }
}

/// Removes `members` (sketch-edit decision 7): each constraint named; each curve named,
/// with each of its points no curve uses after the edit; each point named, which no curve
/// may use after it; and every constraint that names a member removed, in any of its
/// references. The set is judged whole, its curves taken first, so a point and the curves
/// that use it go together (FreeCAD `41c91574cf`), and a member named twice is taken once.
/// A curve's points another curve uses stay, with every constraint on them, so the curves
/// that meet there stay joined (shared points, where FreeCAD re-points coincidences); a
/// `tangent_at` naming a removed curve goes with it, never onto a neighbour (`9ce5756011`).
/// A removed line's `midpoint` becomes a `symmetric` of its ends about the point while the
/// point and both ends stay (the origin, a fixed reference, always does), as FreeCAD's is
/// stored (a `Symmetric` of the line's ends, which
/// the line's delete leaves on the ends its neighbours keep), and as split makes it.
///
/// Refused, every problem in one answer, each once, at the first place in `members` that
/// names it, in their order: a member the sketch hasn't (with the sketch's IDs nearest it as
/// candidates, as the gate gives them for a reference), a fixed reference (`origin`,
/// `x_axis`, `y_axis`; FreeCAD `2a401ee895`, `83d14b785e`), and a point a curve still uses.
pub fn remove<N: Named>(sketch: &Sketch, members: &[N]) -> Result<Edited, Refused> {
    let kinds = kinds_of(sketch);
    let named: BTreeSet<&ElementId> = members
        .iter()
        .filter_map(Named::member)
        .filter(|id| kinds.contains_key(*id))
        .collect();
    let curves_removed: BTreeSet<&ElementId> = named
        .iter()
        .copied()
        .filter(|id| {
            matches!(
                kinds[*id],
                ElementKind::Line | ElementKind::Circle | ElementKind::Arc
            )
        })
        .collect();
    // Who uses each point, among the curves that stay, and whether as its centre: a
    // circle's one point, and an arc's first ([`curves`]' order).
    let mut users: BTreeMap<&ElementId, Vec<(ElementKind, &ElementId, bool)>> = BTreeMap::new();
    for (kind, curve, points) in curves(sketch) {
        if !curves_removed.contains(curve) {
            for (place, point) in points.into_iter().enumerate() {
                let centre = place == 0 && matches!(kind, ElementKind::Circle | ElementKind::Arc);
                users.entry(point).or_default().push((kind, curve, centre));
            }
        }
    }
    // Each problem once, at the first place its member is named, in the request's order.
    let mut refused = Refused::default();
    let mut said: BTreeSet<&ElementId> = BTreeSet::new();
    let mut fixed_said: BTreeSet<&str> = BTreeSet::new();
    for (place, member) in members.iter().enumerate() {
        let Some(id) = member.member() else {
            let fixed = member.text();
            if fixed_said.insert(fixed) {
                refused.push(|| {
                    Error::new(
                        ErrorCode::InvalidInput,
                        format!(
                            "{fixed} is one of the sketch's fixed references, which can't be removed"
                        ),
                    )
                    .with_parameter(format!("members.{place}"))
                    .with_suggestion("remove the constraints that name it instead")
                });
            }
            continue;
        };
        match kinds.get(id) {
            None if said.insert(id) => refused.push(|| {
                let near = nearest_ids(id, kinds.keys());
                Error::new(
                    ErrorCode::InvalidInput,
                    format!("the sketch has no member {id}"),
                )
                .with_parameter(format!("members.{place}"))
                .with_suggestion(with_candidates(
                    "name a point, curve, or constraint of this sketch by its ID".to_owned(),
                    id,
                    &near,
                ))
                .with_candidates(near)
            }),
            Some(ElementKind::Point) => {
                if let Some(using) = users.get(id)
                    && said.insert(id)
                {
                    refused.push(|| point_in_use(id, using, format!("members.{place}")));
                }
            }
            _ => {}
        }
    }
    if refused.total > 0 {
        return Err(refused);
    }
    // The points that go: those named, and a removed curve's that no curve uses after.
    let mut gone: BTreeSet<ElementId> = named.iter().map(|id| (*id).clone()).collect();
    for (_, curve, points) in curves(sketch) {
        if curves_removed.contains(curve) {
            gone.extend(
                points
                    .into_iter()
                    .filter(|p| !users.contains_key(p))
                    .cloned(),
            );
        }
    }
    let mut edited = sketch.clone();
    edited.points.retain(|p| !gone.contains(&p.id));
    edited.lines.retain(|c| !gone.contains(&c.id));
    edited.circles.retain(|c| !gone.contains(&c.id));
    edited.arcs.retain(|c| !gone.contains(&c.id));
    // A removed line's midpoint, its ends' symmetry about the point while they all stay.
    let mut modified = BTreeSet::new();
    for constraint in &mut edited.constraints {
        if let Form::Midpoint { point, line } = &constraint.form
            && let Some(removed) = line.element().filter(|l| gone.contains(*l))
            && !gone.contains(&constraint.id)
            && point.element().is_none_or(|p| !gone.contains(p))
            && let Some(l) = sketch.lines.iter().find(|l| l.id == *removed)
            && !gone.contains(&l.from)
            && !gone.contains(&l.to)
        {
            constraint.form = Form::Symmetric {
                a: Reference::Element(l.from.clone()),
                b: Reference::Element(l.to.clone()),
                about: point.clone(),
            };
            modified.insert(constraint.id.clone());
        }
    }
    // Every constraint naming anything gone, in any reference (a symmetric's `about` line
    // too: FreeCAD `0016d6f2f8`).
    for constraint in &edited.constraints {
        if constraint
            .form
            .references()
            .iter()
            .any(|(_, r, _)| r.element().is_some_and(|id| gone.contains(id)))
        {
            gone.insert(constraint.id.clone());
        }
    }
    edited.constraints.retain(|c| !gone.contains(&c.id));
    Ok(Edited {
        sketch: edited,
        added: Vec::new(),
        modified: ordered(&kinds, modified),
        removed: ordered(&kinds, gone),
    })
}

/// Pulls `curve`'s use of the shared `point` apart (sketch-edit decision 9): `curve` gets
/// `new_point` in its place, at the same coordinates, with none of the point's constraints;
/// the curves that stay keep the point and every constraint on it, and `curve` keeps every
/// constraint of its own (not FreeCAD `0358ff7ce2`'s dropped length). A `tangent_at` between
/// `curve` and another at the point is removed: they no longer meet there (FreeCAD's vertex
/// Delete keeps an end-to-end tangency, which an unjoin that left them joined wouldn't be).
/// A `tangent_on` whose `a` is `curve` and whose `at` is the point goes with `curve`, to the
/// new point, so its `at` stays an end of `a` (docs/design/m5.0-app.md, open item 14,
/// pending the maintainer). The point's other constraints (a lock, an `on`) stay with the
/// curves that keep it, where FreeCAD's vertex Delete (`delConstraintOnPoint`, its
/// `onlyCoincident`) leaves them on the detached curve's end: a shared point doesn't say
/// which curve's end a constraint was on. Refused, as data naming them: a point or curve the sketch hasn't,
/// a curve that doesn't use the point, and a point no other curve shares (FreeCAD
/// `bc4d463cdf`, where it was an error).
pub fn unjoin(
    sketch: &Sketch,
    point: &ElementId,
    curve: &ElementId,
    new_point: ElementId,
) -> Result<Edited, Error> {
    unjoin_with(
        sketch,
        point,
        curve,
        NewIds::Given(std::slice::from_ref(&new_point)),
    )
}

/// [`unjoin`], the new point's ID the request's (a list of one) or drawn ([`NewIds`]): drawn
/// only once every other refusal has been found.
pub fn unjoin_with(
    sketch: &Sketch,
    point: &ElementId,
    curve: &ElementId,
    ids: NewIds<'_>,
) -> Result<Edited, Error> {
    let kinds = kinds_of(sketch);
    let Some(original) = sketch.points.iter().find(|p| &p.id == point) else {
        return Err(no_member(point, ElementKind::Point, "point"));
    };
    if !matches!(
        kinds.get(curve),
        Some(ElementKind::Line | ElementKind::Circle | ElementKind::Arc)
    ) {
        return Err(no_member(curve, ElementKind::Line, "curve"));
    }
    let sharing: Vec<(ElementKind, &ElementId)> = curves(sketch)
        .into_iter()
        .filter(|(_, _, points)| points.contains(&point))
        .map(|(kind, id, _)| (kind, id))
        .collect();
    if !sharing.iter().any(|(_, id)| *id == curve) {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("{} {curve} doesn't use point {point}", kinds[curve].noun()),
        )
        .with_elements([point.to_string(), curve.to_string()])
        .with_parameter("curve")
        .with_suggestion("name a curve that ends at, or is centred on, the point"));
    }
    if sharing.len() < 2 {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "point {point} is used by no curve but {} {curve}, so there's nothing to unjoin",
                kinds[curve].noun()
            ),
        )
        .with_elements([point.to_string(), curve.to_string()])
        .with_parameter("point")
        .with_suggestion("name a point two or more curves share"));
    }
    let [new_point] =
        <[ElementId; 1]>::try_from(new_ids(&kinds, ids, 1, "the unjoin", "its new point")?)
            .map_err(|_| {
                Error::new(
                    ErrorCode::Internal,
                    "the unjoin was given other than one new ID",
                )
                .with_parameter("ids")
            })?;
    let mut edited = sketch.clone();
    edited.points.push(SketchPoint {
        id: new_point.clone(),
        ..original.clone()
    });
    let swap = |id: &mut ElementId| {
        if id == point {
            *id = new_point.clone();
        }
    };
    for line in edited.lines.iter_mut().filter(|l| &l.id == curve) {
        swap(&mut line.from);
        swap(&mut line.to);
    }
    for circle in edited.circles.iter_mut().filter(|c| &c.id == curve) {
        swap(&mut circle.center);
    }
    for arc in edited.arcs.iter_mut().filter(|a| &a.id == curve) {
        swap(&mut arc.center);
        swap(&mut arc.start);
        swap(&mut arc.end);
    }
    let mut modified = BTreeSet::from([curve.clone()]);
    let mut removed = BTreeSet::new();
    let at_point = Reference::Element(point.clone());
    let is_curve = Reference::Element(curve.clone());
    edited.constraints.retain_mut(|c| match &mut c.form {
        Form::TangentAt { a, b, at, .. }
            if *at == at_point && (*a == is_curve || *b == is_curve) =>
        {
            removed.insert(c.id.clone());
            false
        }
        Form::TangentOn { a, at, .. } if *a == is_curve && *at == at_point => {
            *at = Reference::Element(new_point.clone());
            modified.insert(c.id.clone());
            true
        }
        _ => true,
    });
    let mut kinds = kinds;
    kinds.insert(new_point.clone(), ElementKind::Point);
    Ok(Edited {
        sketch: edited,
        added: vec![new_point],
        modified: ordered(&kinds, modified),
        removed: ordered(&kinds, removed),
    })
}

/// Makes `points` one (sketch-edit decision 10): the first named survives, at its own
/// coordinates; every curve and constraint naming another names it instead, and the others
/// are removed. The merge places the point, as no FreeCAD coincidence does (that lands where
/// the solve takes it); the solve after the edit settles the rest. Refused, as data naming
/// them: fewer than two points, one the sketch hasn't, a point named twice (already one,
/// FreeCAD `cb2780e2fa`), a curve that would then use one point twice (a line's ends, an
/// arc's centre and an end, or its two ends: `084379651a`, #20771), and a constraint that
/// would then name one point twice (a `distance` or `horizontal_points` between merged
/// points), and a merged sketch the gate refuses, naming the points and what it names. An
/// `on` the merge makes hold a line's or an arc's end on that curve is implied by then, and
/// removed, listed in `removed` (docs/design/m5.0-app.md, open item 13b); any other
/// constraint it puts on its own curve is refused ([`own_curve`]).
///
/// An edge `tangent` between two curves that then share an end becomes a `tangent_at` at
/// that point, keeping its ID, as FreeCAD's coincident command substitutes one
/// (`620188073e`; docs/design/m5.0-app.md, open item 13): its `direction` keeps the
/// tangent's `side` (sketch-edit decision 8), listed in `modified`.
///
/// A request with one bad pair is refused whole, where FreeCAD's coincident command skips
/// that pair and makes the rest coincident (`CommandConstraints.cpp`, its multi-select): an
/// edit is made whole or not at all.
pub fn merge(sketch: &Sketch, points: &[ElementId]) -> Result<Edited, Error> {
    let kinds = kinds_of(sketch);
    let Some(kept) = points.first() else {
        return Err(too_few());
    };
    for (place, id) in points.iter().enumerate() {
        if kinds.get(id) != Some(&ElementKind::Point) {
            return Err(no_member(
                id,
                ElementKind::Point,
                &format!("points.{place}"),
            ));
        }
        if points[..place].contains(id) {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!("point {id} is named twice: it's already one point"),
            )
            .with_elements([id.to_string()])
            .with_parameter(format!("points.{place}"))
            .with_suggestion("name each point to merge once"));
        }
    }
    if points.len() < 2 {
        return Err(too_few());
    }
    let others: BTreeSet<&ElementId> = points[1..].iter().collect();
    let into = |id: &ElementId| {
        if others.contains(id) {
            kept.clone()
        } else {
            id.clone()
        }
    };
    // A curve that would use one point twice.
    for (kind, curve, ends) in curves(sketch) {
        let merged: Vec<ElementId> = ends.iter().map(|p| into(p)).collect();
        let twice = (0..merged.len()).any(|i| merged[i + 1..].contains(&merged[i]));
        if twice {
            // The curve's points the merge makes one, in the curve's order.
            let merging: Vec<String> = ends
                .iter()
                .filter(|p| **p == kept || others.contains(**p))
                .map(|p| p.to_string())
                .collect();
            let mut elements = merging.clone();
            elements.push(curve.to_string());
            // What the one point would be to the curve: its two ends (a line's, an arc's),
            // or an arc's centre and an end or both (the design's words, `sketch_merge`).
            let merged = |p: &ElementId| p == kept || others.contains(p);
            let how = match ends.as_slice() {
                [center, start, end] if merged(center) => {
                    if merged(start) && merged(end) {
                        "as its centre and both its ends"
                    } else {
                        "as its centre and an end"
                    }
                }
                _ => "at both ends",
            };
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "merging {} would give {} {curve} one point {how}",
                    listed(&merging),
                    kind.noun()
                ),
            )
            .with_elements(elements)
            .with_parameter("points")
            .with_suggestion("merge points of different curves"));
        }
    }
    let mut edited = sketch.clone();
    let mut modified = BTreeSet::new();
    for line in &mut edited.lines {
        let (from, to) = (into(&line.from), into(&line.to));
        if (&from, &to) != (&line.from, &line.to) {
            (line.from, line.to) = (from, to);
            modified.insert(line.id.clone());
        }
    }
    for circle in &mut edited.circles {
        let center = into(&circle.center);
        if center != circle.center {
            circle.center = center;
            modified.insert(circle.id.clone());
        }
    }
    for arc in &mut edited.arcs {
        let (center, start, end) = (into(&arc.center), into(&arc.start), into(&arc.end));
        if (&center, &start, &end) != (&arc.center, &arc.start, &arc.end) {
            (arc.center, arc.start, arc.end) = (center, start, end);
            modified.insert(arc.id.clone());
        }
    }
    let mut repointed = Vec::new();
    for constraint in &mut edited.constraints {
        let mut changed = false;
        for reference in references_mut(&mut constraint.form) {
            if let Reference::Element(id) = reference
                && others.contains(&*id)
            {
                *id = kept.clone();
                changed = true;
            }
        }
        // A constraint that names one point twice.
        let mut seen = BTreeSet::new();
        for (_, reference, _) in constraint.form.references() {
            if let Some(id) = reference.element()
                && kinds.get(id) == Some(&ElementKind::Point)
                && !seen.insert(id.clone())
            {
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "merging them would make constraint {} name point {kept} twice",
                        constraint.id
                    ),
                )
                .with_elements([constraint.id.to_string(), kept.to_string()])
                .with_parameter("points")
                .with_suggestion("remove the constraint first, or merge other points"));
            }
        }
        if changed {
            repointed.push(constraint.id.clone());
        }
    }
    repointed.extend(tangents_at_shared_ends(&mut edited));
    let implied = implied_ons(&mut edited);
    repointed.retain(|id| !implied.contains(id));
    own_curve(&edited, "merging them", "points", "merge other points")?;
    modified.extend(repointed);
    edited.points.retain(|p| !others.contains(&p.id));
    // A merge places points by ID: a curve it leaves with no size (its ends, or an arc's end
    // and centre, drawn at one place), or a sketch its tangent_ats take past the solver's
    // values, is the gate's to find.
    super::refused_by_gate(
        &edited,
        "merging them",
        points.iter().map(ToString::to_string).collect(),
        "points",
        "merge other points, or change what the merge would collapse",
    )?;
    Ok(Edited {
        sketch: edited,
        added: Vec::new(),
        modified: ordered(&kinds, modified),
        removed: ordered(&kinds, others.into_iter().cloned().chain(implied)),
    })
}

/// Each `on` of `sketch` that holds a line's or an arc's end on that curve itself, removed
/// as implied, as a merge leaves one where it made the `on`'s point the curve's end (a T
/// drawn at a line's end, which `silico fmt --merge-touching` meets): the shared point is
/// the end, so the `on` holds by construction, with no value changed and nothing moved
/// (docs/design/m5.0-app.md, open item 13b, as trim drops one at a shared cut, sketch-edit
/// decision 5). Only an end: an `on` of a circle's or arc's centre on it, which would
/// conflict rather than repeat, is left for [`own_curve`] to refuse, as is any other
/// constraint the merge puts on its own curve. FreeCAD keeps the `PointOnObject` and its
/// solve flags it redundant; the gate refuses a curve's own point held on it, so Silico
/// can't keep it. The IDs removed.
fn implied_ons(sketch: &mut Sketch) -> BTreeSet<ElementId> {
    let ends: BTreeMap<&ElementId, [&ElementId; 2]> = sketch
        .lines
        .iter()
        .map(|l| (&l.id, [&l.from, &l.to]))
        .chain(sketch.arcs.iter().map(|a| (&a.id, [&a.start, &a.end])))
        .collect();
    let implied: BTreeSet<ElementId> = sketch
        .constraints
        .iter()
        .filter(|c| match &c.form {
            Form::On { point, curve } => match (point.element(), curve.element()) {
                (Some(point), Some(curve)) => ends.get(curve).is_some_and(|e| e.contains(&point)),
                _ => false,
            },
            _ => false,
        })
        .map(|c| c.id.clone())
        .collect();
    sketch.constraints.retain(|c| !implied.contains(&c.id));
    implied
}

/// Each edge `tangent` of `sketch` between two curves that share one end, made a
/// `tangent_at` at that point, keeping its ID (docs/design/m5.0-app.md, open item 13), as a
/// merge leaves one where it made the shared end: FreeCAD's coincident command substitutes
/// an endpoint tangency for an edge one so (`620188073e`). Its `direction` keeps the
/// tangent's `side` (sketch-edit decision 8), whatever the merge moved: see [`kept_side`].
/// Curves sharing both ends, which have no one point for it, are left for [`own_curve`] to
/// refuse. The IDs made `tangent_at`s.
fn tangents_at_shared_ends(sketch: &mut Sketch) -> Vec<ElementId> {
    let Sketch {
        lines,
        arcs,
        constraints,
        ..
    } = sketch;
    let ends: BTreeMap<&ElementId, [&ElementId; 2]> = lines
        .iter()
        .map(|l| (&l.id, [&l.from, &l.to]))
        .chain(arcs.iter().map(|a| (&a.id, [&a.start, &a.end])))
        .collect();
    let mut made = Vec::new();
    for constraint in constraints {
        let Form::Tangent { a, b, side } = &constraint.form else {
            continue;
        };
        let (Some(of_a), Some(of_b)) = (
            a.element().and_then(|a| ends.get(a)),
            b.element().and_then(|b| ends.get(b)),
        ) else {
            continue;
        };
        let shared: Vec<&ElementId> = of_a.iter().copied().filter(|p| of_b.contains(p)).collect();
        let [at] = shared[..] else {
            continue;
        };
        // Whether the shared point ends one curve and starts the other.
        let end_to_start = (of_a[0] == at) != (of_b[0] == at);
        constraint.form = Form::TangentAt {
            a: a.clone(),
            b: b.clone(),
            at: Reference::Element(at.clone()),
            direction: kept_side(*side, end_to_start),
        };
        made.push(constraint.id.clone());
    }
    made
}

/// The `direction` of a `tangent_at` that keeps an edge tangent's `side` where the curves
/// now share an end, `end_to_start` if it ends one and starts the other. The side fixes the
/// angle between the two curves' own directions (a line's from start to end, a round's
/// counter-clockwise) at the point they touch: a round on the line's left, or two rounds one
/// inside the other, turn the same way there, an angle of 0; on its right, or outside each
/// other, opposite ways, π. The solve's translation reads 0 as `same` where the point ends
/// one and starts the other, and as `opposite` where it starts or ends both. Read from the
/// side, not from the drawing the merge leaves: points merged from apart move a curve's
/// end, and its drawing can then show the other side. FreeCAD's edge tangency holds no side,
/// so its coincident command locks the angle the unmerged drawing shows at the point
/// (`AutoLockTangencyAndPerpty`); where the merged points touch, that point is the contact
/// and the two agree.
fn kept_side(side: TangentSide, end_to_start: bool) -> Direction {
    let turn_alike = matches!(side, TangentSide::Left | TangentSide::Inside);
    if turn_alike == end_to_start {
        Direction::Same
    } else {
        Direction::Opposite
    }
}

/// The `direction` of a `tangent_on` of `piece`'s end `at` on `cutter`, as the stored drawing
/// shows it (trim's decision 8; the constraint tools' C3): `same` when the piece, leaving
/// `at`, runs as the cutter's own direction there, their dot product at least 0, so a right
/// angle is `same`. The piece and `at` are found in `edited`, the cutter in `sketch` (a
/// trim's, the sketch before it; the constraint tools give one sketch for both). `same`
/// where either direction can't be found.
pub(crate) fn drawn(
    sketch: &Sketch,
    edited: &Sketch,
    piece: &Reference,
    at: &Reference,
    cutter: &Reference,
) -> Direction {
    let (Some(xy), Some(out)) = (split::reference_at(edited, at), leaving(edited, piece, at))
    else {
        return Direction::Same;
    };
    let Some(along_cutter) = own_direction(sketch, cutter, xy) else {
        return Direction::Same;
    };
    if out[0] * along_cutter[0] + out[1] * along_cutter[1] >= 0.0 {
        Direction::Same
    } else {
        Direction::Opposite
    }
}

/// The direction the line or arc `curve` leaves its end `at` along, into the curve (not off
/// its end, as `View::ends_at`'s), as the stored drawing shows it: its own direction there
/// where it starts at `at`, the reverse where it ends there; none where `at` isn't one of its
/// ends.
pub(crate) fn leaving(sketch: &Sketch, curve: &Reference, at: &Reference) -> Option<[f64; 2]> {
    let xy = split::reference_at(sketch, at)?;
    let starts = split::starts_at(sketch, curve, at)?;
    let along = own_direction(sketch, curve, xy)?;
    Some(if starts {
        along
    } else {
        [-along[0], -along[1]]
    })
}

/// A curve's own direction at `xy`: a line's from `from` to `to`, a round's
/// counter-clockwise, an axis's along +x or +y.
fn own_direction(sketch: &Sketch, curve: &Reference, xy: [f64; 2]) -> Option<[f64; 2]> {
    let id = match curve {
        Reference::XAxis => return Some([1.0, 0.0]),
        Reference::YAxis => return Some([0.0, 1.0]),
        Reference::Origin => return None,
        Reference::Element(id) => id,
    };
    if let Some(line) = sketch.lines.iter().find(|l| &l.id == id) {
        let (from, to) = (
            split::point_at(sketch, &line.from)?,
            split::point_at(sketch, &line.to)?,
        );
        return Some([to[0] - from[0], to[1] - from[1]]);
    }
    let center = match sketch.arcs.iter().find(|a| &a.id == id) {
        Some(arc) => split::point_at(sketch, &arc.center)?,
        None => split::point_at(sketch, &sketch.circles.iter().find(|c| &c.id == id)?.center)?,
    };
    Some([-(xy[1] - center[1]), xy[0] - center[0]])
}

/// Refuses an edited sketch in which a constraint holds a curve's own point on it, or
/// leaves a tangency in a form the gate refuses where the curves meet: what the gate would
/// refuse of the edit's result, said as the edit's refusal (`edit` its subject, "merging
/// them"; `parameter` the request's field; `instead` what else to do), naming the
/// constraint. Before a merge these points were apart, and before a split the point wasn't
/// the curve's, so the sketch it was given had none of them.
///
/// A merge has removed an `on` that it makes hold an end on its own line or arc first, as
/// implied ([`implied_ons`], open item 13b): one of a centre, a `midpoint`, a
/// `point_line_distance`, or a mirrored `symmetric` would conflict, not repeat, and is
/// refused here.
fn own_curve(sketch: &Sketch, edit: &str, parameter: &str, instead: &str) -> Result<(), Error> {
    let points: BTreeMap<&ElementId, Vec<&ElementId>> = curves(sketch)
        .into_iter()
        .map(|(_, id, points)| (id, points))
        .collect();
    let ends: BTreeMap<&ElementId, Vec<&ElementId>> = sketch
        .lines
        .iter()
        .map(|l| (&l.id, vec![&l.from, &l.to]))
        .chain(sketch.arcs.iter().map(|a| (&a.id, vec![&a.start, &a.end])))
        .collect();
    let of = |point: &Reference,
              curve: &Reference,
              table: &BTreeMap<&ElementId, Vec<&ElementId>>| {
        match (point.element(), curve.element()) {
            (Some(point), Some(curve)) => table.get(curve).is_some_and(|ps| ps.contains(&point)),
            _ => false,
        }
    };
    let refuse = |constraint: &ElementId, why: String, elements: Vec<String>| {
        Err(Error::new(
            ErrorCode::InvalidInput,
            format!("{edit} would make constraint {constraint} {why}"),
        )
        .with_elements(std::iter::once(constraint.to_string()).chain(elements))
        .with_parameter(parameter.to_owned())
        .with_suggestion(format!("remove the constraint first, or {instead}")))
    };
    for constraint in &sketch.constraints {
        let id = &constraint.id;
        match &constraint.form {
            Form::On { point, curve }
            | Form::Midpoint { point, line: curve }
            | Form::PointLineDistance {
                point, line: curve, ..
            } if of(point, curve, &points) => {
                return refuse(
                    id,
                    format!("hold {point}, a point of {curve}, on {curve} itself"),
                    vec![point.to_string(), curve.to_string()],
                );
            }
            Form::Symmetric { a, b, about } if of(a, about, &points) || of(b, about, &points) => {
                return refuse(
                    id,
                    format!("mirror an end of {about} about {about} itself"),
                    vec![a.to_string(), b.to_string(), about.to_string()],
                );
            }
            Form::TangentOn { a, at, b, .. } if of(at, b, &points) => {
                return refuse(
                    id,
                    format!(
                        "hold {a}'s end {at}, a point of {b}, tangent on {b}: where curves meet, a tangency is a tangent_at"
                    ),
                    vec![a.to_string(), at.to_string(), b.to_string()],
                );
            }
            Form::Tangent { a, b, .. } => {
                // Curves that share an end: where they meet, a tangency is a tangent_at. A
                // merge's is made one (`tangents_at_shared_ends`), but not where the two share
                // both ends. A split meets one only at a curve's end held tangent on it,
                // whose tangent_on becomes that tangent_at, so it's refused, repeating it:
                // never from a checked sketch, since the gate refuses an edge tangency beside
                // an `on`, a `tangent_on` (#258), or a `midpoint` (#498) of an end.
                let shared = match (a.element(), b.element()) {
                    (Some(a), Some(b)) => ends
                        .get(a)
                        .zip(ends.get(b))
                        .and_then(|(of_a, of_b)| of_a.iter().find(|p| of_b.contains(p))),
                    _ => None,
                };
                if let Some(point) = shared {
                    return refuse(
                        id,
                        format!(
                            "an edge tangency of {a} and {b}, which would then share an end, {point}: where curves meet, a tangency is a tangent_at"
                        ),
                        vec![a.to_string(), b.to_string(), point.to_string()],
                    );
                }
                // An end of one held on the other, by an `on` or at the other's middle by a
                // `midpoint`: the gate takes that as a tangent_on, beside the midpoint. One
                // beside a tangent_on is the gate's to refuse, in #290's words.
                for hold in &sketch.constraints {
                    let (point, curve, held) = match &hold.form {
                        Form::On { point, curve } => {
                            (point, curve, "on the other: that's a tangent_on")
                        }
                        Form::Midpoint { point, line } => (
                            point,
                            line,
                            "at the other's middle: make the tangent a tangent_on, keeping the midpoint",
                        ),
                        _ => continue,
                    };
                    if (curve == b && of(point, a, &ends)) || (curve == a && of(point, b, &ends)) {
                        return refuse(
                            id,
                            format!(
                                "an edge tangency of {a} and {b} where {hold_id} holds an end of one {held}",
                                hold_id = hold.id
                            ),
                            vec![hold.id.to_string(), a.to_string(), b.to_string()],
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Why an edit that can find several problems ([`remove`]) was refused: the first
/// [`PROBLEMS_LISTED`] as errors, in the order found, and how many there are, listed or not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Refused {
    pub errors: Vec<Error>,
    pub total: u64,
}

impl Refused {
    /// Counts a problem, and makes its error while fewer than [`PROBLEMS_LISTED`] are listed.
    fn push(&mut self, error: impl FnOnce() -> Error) {
        if self.errors.len() < PROBLEMS_LISTED {
            self.errors.push(error());
        }
        self.total += 1;
    }
}

/// Each member's kind, by ID.
fn kinds_of(sketch: &Sketch) -> BTreeMap<ElementId, ElementKind> {
    sketch
        .elements()
        .map(|(id, kind)| (id.clone(), kind))
        .collect()
}

/// Each curve with its kind and its points: a line's ends, a circle's centre, an arc's
/// centre and ends.
fn curves(sketch: &Sketch) -> Vec<(ElementKind, &ElementId, Vec<&ElementId>)> {
    let lines = sketch
        .lines
        .iter()
        .map(|l| (ElementKind::Line, &l.id, vec![&l.from, &l.to]));
    let circles = sketch
        .circles
        .iter()
        .map(|c| (ElementKind::Circle, &c.id, vec![&c.center]));
    let arcs = sketch
        .arcs
        .iter()
        .map(|a| (ElementKind::Arc, &a.id, vec![&a.center, &a.start, &a.end]));
    lines.chain(circles).chain(arcs).collect()
}

/// `ids` by kind, in the format's order, then by ID.
fn ordered(
    kinds: &BTreeMap<ElementId, ElementKind>,
    ids: impl IntoIterator<Item = ElementId>,
) -> Vec<ElementId> {
    let mut ids: Vec<(ElementKind, ElementId)> = ids
        .into_iter()
        .filter_map(|id| kinds.get(&id).map(|kind| (*kind, id)))
        .collect();
    ids.sort();
    ids.dedup();
    ids.into_iter().map(|(_, id)| id).collect()
}

/// A form's references, to change in place: every one [`Form::references`] lists.
fn references_mut(form: &mut Form) -> Vec<&mut Reference> {
    match form {
        Form::Horizontal { line } | Form::Vertical { line } => vec![line],
        Form::HorizontalPoints { a, b }
        | Form::VerticalPoints { a, b }
        | Form::Distance { a, b, .. }
        | Form::DistanceX { a, b, .. }
        | Form::DistanceY { a, b, .. }
        | Form::Parallel { a, b }
        | Form::Perpendicular { a, b }
        | Form::Angle { a, b, .. }
        | Form::Equal { a, b }
        | Form::Tangent { a, b, .. } => vec![a, b],
        Form::Length { line, .. } => vec![line],
        Form::PointLineDistance { point, line, .. } => vec![point, line],
        Form::On { point, curve } => vec![point, curve],
        Form::Radius { curve, .. } | Form::Diameter { curve, .. } => vec![curve],
        Form::TangentOn { a, at, b, .. } => vec![a, at, b],
        Form::TangentAt { a, b, at, .. } => vec![a, b, at],
        Form::Symmetric { a, b, about } => vec![a, b, about],
        Form::Midpoint { point, line } => vec![point, line],
    }
}

/// A point that curves still use, refused as removed alone: the point and the first of
/// its curves in `elements`, up to [`PROBLEMS_LISTED`], all counted, and the message naming
/// the first few. Its cause says what the point is to them, from each use's `centre`:
/// their end, their centre, or (some of each) their end or centre (docs/design/m5.0-app.md,
/// `sketch_remove`'s error, for a point two lines end at).
fn point_in_use(
    point: &ElementId,
    using: &[(ElementKind, &ElementId, bool)],
    parameter: String,
) -> Error {
    const SAID: usize = 3;
    let nouns: BTreeSet<&str> = using.iter().map(|(kind, _, _)| kind.noun()).collect();
    let noun = match (using, nouns.len()) {
        ([(kind, _, _)], _) => kind.noun().to_owned(),
        ([(kind, _, _), ..], 1) => format!("{}s", kind.noun()),
        _ => "curves".to_owned(),
    };
    let role = match (
        using.iter().any(|(_, _, centre)| !centre),
        using.iter().any(|(_, _, centre)| *centre),
    ) {
        (true, true) => "end or centre",
        (false, true) => "centre",
        _ => "end",
    };
    let said: Vec<String> = using
        .iter()
        .take(SAID)
        .map(|(_, id, _)| id.to_string())
        .collect();
    let curves = listed_first(&said, using.len());
    let elements = std::iter::once(point.to_string())
        .chain(using.iter().map(|(_, id, _)| id.to_string()))
        .take(PROBLEMS_LISTED);
    let total = u64::try_from(using.len()).map_or(u64::MAX, |n| n.saturating_add(1));
    Error::new(
        ErrorCode::InvalidInput,
        format!("point {point} is used by {noun} {curves}"),
    )
    .with_elements(elements)
    .with_elements_total(total)
    .with_parameter(parameter)
    .with_cause(format!("a point that curves use is those curves' {role}"))
    .with_suggestion("remove the curves that use it with it, or unjoin it first")
}

/// A member the sketch hasn't, of the kind `parameter` takes.
fn no_member(id: &ElementId, kind: ElementKind, parameter: &str) -> Error {
    let what = match kind {
        ElementKind::Point => "point",
        _ => "curve",
    };
    Error::new(
        ErrorCode::InvalidInput,
        format!("the sketch has no {what} {id}"),
    )
    .with_parameter(parameter.to_owned())
    .with_suggestion(format!("name a {what} of this sketch by its ID"))
}

/// Where an edit's new members' IDs come from (docs/design/m5.0-app.md, section 7, *New
/// IDs*; decision 20): the request's, in `added`'s order, or, for a request whose `ids` is
/// `null`, drawn one at a time once the edit knows how many it needs, each by `draw`, told
/// which IDs are taken (the sketch's, and those drawn before), as `add_feature` draws a
/// feature's.
pub enum NewIds<'a> {
    Given(&'a [ElementId]),
    Drawn(&'a mut Draw<'a>),
}

/// What draws a new member's ID ([`NewIds::Drawn`]), told which IDs are taken.
pub type Draw<'a> = dyn FnMut(&dyn Fn(&ElementId) -> bool) -> Result<ElementId, Error> + 'a;

/// The `needed` new IDs of `edit` ("the split"), `order` saying what they're for, in order:
/// the request's, refused unless there are `needed` of them, none a member of the sketch
/// has, each once; or `needed` drawn, none taken.
fn new_ids(
    kinds: &BTreeMap<ElementId, ElementKind>,
    ids: NewIds<'_>,
    needed: usize,
    edit: &str,
    order: &str,
) -> Result<Vec<ElementId>, Error> {
    let ids = match ids {
        NewIds::Given(ids) => ids,
        NewIds::Drawn(draw) => {
            let mut drawn: Vec<ElementId> = Vec::with_capacity(needed);
            for _ in 0..needed {
                let taken = |id: &ElementId| kinds.contains_key(id) || drawn.contains(id);
                let id = draw(&taken)?;
                drawn.push(id);
            }
            return Ok(drawn);
        }
    };
    if ids.len() != needed {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("{edit} needs {needed} new IDs, got {}", ids.len()),
        )
        .with_parameter("ids")
        .with_suggestion(format!(
            "give one ID for each member {edit} adds, in order: {order}; or null, for the core to make them"
        )));
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
    Ok(ids.to_vec())
}

/// A merge of fewer than two points.
fn too_few() -> Error {
    Error::new(ErrorCode::InvalidInput, "a merge takes two or more points")
        .with_parameter("points")
        .with_suggestion("name the points to make one, the one to keep first")
}
