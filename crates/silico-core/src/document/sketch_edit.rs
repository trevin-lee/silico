//! What each sketch edit does to a sketch's geometry and constraints (docs/ports/sketch-edit.md,
//! whose rules are FreeCAD's behavior with Silico's shared points, M3.2's D2): pure
//! functions from a sketch to the sketch edited, or the reasons it can't be, as errors
//! naming its members. Each result is a sketch the validation gate then checks, and a
//! solve then settles (the edit's command, M5.10, which this doesn't hold).
//!
//! Here: removing members (the note's §2, decision 7), pulling a shared point apart (§5,
//! decision 9), and merging points (§6, decision 10). Splitting and trimming follow.

use std::collections::{BTreeMap, BTreeSet};

use super::sketch::{ElementId, ElementKind, Form, Reference, Sketch, SketchPoint};
use crate::{Error, ErrorCode};

/// A sketch edited, and what changed: each list by kind (points, lines, circles, arcs,
/// constraints), then by ID (docs/design/m5.0-app.md, section 7, *The sketch edits*).
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

/// Removes `members` (sketch-edit decision 7): each constraint named; each curve named,
/// with each of its points no curve uses after the edit; each point named, which no curve
/// may use after it; and every constraint that names a member removed, in any of its
/// references. The set is judged whole, its curves taken first, so a point and the curves
/// that use it go together (FreeCAD `41c91574cf`), and a member named twice is taken once.
/// A curve's points another curve uses stay, with every constraint on them, so the curves
/// that meet there stay joined (shared points, where FreeCAD re-points coincidences); a
/// `tangent_at` naming a removed curve goes with it, never onto a neighbour (`9ce5756011`).
pub fn remove(sketch: &Sketch, members: &[ElementId]) -> Result<Edited, Vec<Error>> {
    let kinds = kinds_of(sketch);
    let mut errors = Vec::new();
    let mut named: BTreeSet<&ElementId> = BTreeSet::new();
    for (place, id) in members.iter().enumerate() {
        if kinds.contains_key(id) {
            named.insert(id);
        } else {
            errors.push(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!("the sketch has no member {id}"),
                )
                .with_parameter(format!("members.{place}"))
                .with_suggestion("name a point, curve, or constraint of this sketch by its ID"),
            );
        }
    }
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
    // Who uses each point, among the curves that stay.
    let mut users: BTreeMap<&ElementId, Vec<(ElementKind, &ElementId)>> = BTreeMap::new();
    for (kind, curve, points) in curves(sketch) {
        if !curves_removed.contains(curve) {
            for point in points {
                users.entry(point).or_default().push((kind, curve));
            }
        }
    }
    for (place, id) in members.iter().enumerate() {
        if kinds.get(id) != Some(&ElementKind::Point) {
            continue;
        }
        if let Some(using) = users.get(id) {
            errors.push(point_in_use(id, using, format!("members.{place}")));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
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
    // Every constraint naming anything gone, in any reference (a symmetric's `about` line
    // too: FreeCAD `0016d6f2f8`).
    for constraint in &sketch.constraints {
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
        modified: Vec::new(),
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
/// pending the maintainer). Refused, as data naming them: a point or curve the sketch hasn't,
/// a curve that doesn't use the point, and a point no other curve shares (FreeCAD
/// `bc4d463cdf`, where it was an error).
pub fn unjoin(
    sketch: &Sketch,
    point: &ElementId,
    curve: &ElementId,
    new_point: ElementId,
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
    if kinds.contains_key(&new_point) {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("the sketch already has a member {new_point}"),
        )
        .with_parameter("ids.0")
        .with_suggestion("give an ID no member of the sketch has"));
    }
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
/// points).
///
/// An edge `tangent` between two curves that would then share an end is refused too, as
/// decision 10 has it, until the maintainer settles docs/design/m5.0-app.md's open item 13,
/// which recommends making it a `tangent_at` instead (sketch-edit decision 8).
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
            let mut elements: Vec<String> = ends.iter().map(|p| p.to_string()).collect();
            elements.push(curve.to_string());
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "merging them would give {} {curve} one point twice",
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
    // Which curves end at the kept point, after the merge, for the tangent rule.
    let ends_at: BTreeSet<&ElementId> = edited
        .lines
        .iter()
        .filter(|l| l.from == *kept || l.to == *kept)
        .map(|l| &l.id)
        .chain(
            edited
                .arcs
                .iter()
                .filter(|a| a.start == *kept || a.end == *kept)
                .map(|a| &a.id),
        )
        .collect();
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
        if let Form::Tangent { a, b, .. } = &constraint.form
            && let (Some(a), Some(b)) = (a.element(), b.element())
            && ends_at.contains(a)
            && ends_at.contains(b)
        {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "merging them would leave tangent {} between {a} and {b}, which would then share an end, where a tangency must be a tangent_at",
                    constraint.id
                ),
            )
            .with_elements([constraint.id.to_string(), a.to_string(), b.to_string()])
            .with_parameter("points")
            .with_suggestion("replace the tangent with a tangent_at at the shared point first"));
        }
        if changed {
            repointed.push(constraint.id.clone());
        }
    }
    modified.extend(repointed);
    edited.points.retain(|p| !others.contains(&p.id));
    Ok(Edited {
        sketch: edited,
        added: Vec::new(),
        modified: ordered(&kinds, modified),
        removed: ordered(&kinds, others.into_iter().cloned()),
    })
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

/// A point that curves still use, refused as removed alone.
fn point_in_use(
    point: &ElementId,
    using: &[(ElementKind, &ElementId)],
    parameter: String,
) -> Error {
    let nouns: BTreeSet<&str> = using.iter().map(|(kind, _)| kind.noun()).collect();
    let noun = if using.len() == 1 {
        using[0].0.noun().to_owned()
    } else if nouns.len() == 1 {
        format!("{}s", using[0].0.noun())
    } else {
        "curves".to_owned()
    };
    let ids: Vec<String> = using.iter().map(|(_, id)| id.to_string()).collect();
    Error::new(
        ErrorCode::InvalidInput,
        format!("point {point} is used by {noun} {}", ids.join(" and ")),
    )
    .with_elements(std::iter::once(point.to_string()).chain(ids))
    .with_parameter(parameter)
    .with_cause("a point that curves use is those curves' end or centre")
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

/// A merge of fewer than two points.
fn too_few() -> Error {
    Error::new(ErrorCode::InvalidInput, "a merge takes two or more points")
        .with_parameter("points")
        .with_suggestion("name the points to make one, the one to keep first")
}
