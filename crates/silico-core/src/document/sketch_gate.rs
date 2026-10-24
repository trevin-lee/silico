//! The validation gate's rules for a sketch (docs/design/m3.2-sketch-format.md, *The gate's
//! rules for sketches*): what FreeCAD finds only when it solves, or never, refused here,
//! naming the field and the element.
//! - Every ID is unique within its sketch; every reference names an element of a kind its
//!   place takes. A reference to an ID no element holds has as candidates the IDs of a
//!   kind it takes a character or two from it, a mistyped or swapped one, none taken for it.
//!   FreeCAD's `validateConstraints` deletes a constraint naming missing geometry
//!   (`testValidateConstraintsCase`); the gate refuses it, naming it.
//! - Nothing is drawn with no size as the kernel measures it (a line's ends, a circle's
//!   radius, an arc's center to its start and to its end, and its chord, each at most
//!   1e-7 mm, `LENGTH_TOLERANCE`, by `Curve::size_problem` as the profile converts it), or
//!   past what the solver takes (an arc's radius, center to start, within a kilometre).
//! - A curve's own point isn't constrained onto it, curves sharing an end are tangent only
//!   there (`tangent_at`), and forms that take two elements get two.
//! - Driving dimensions are above 0 (unless signed) and within a kilometre; reference ones
//!   hold what a solve can measure.
//! - Every constraint names an element of the sketch's own, and the sketch is one the sketch
//!   worker takes.
//!
//! Whether an arc's ends lie on its circle is the solve's to mend or name (D2, arcs), not
//! the gate's.

use std::collections::BTreeMap;

use silico_protocol::{
    LENGTH_TOLERANCE, MAX_LENGTH, MAX_SKETCH_CONSTRAINTS, MAX_SKETCH_GEOMETRY, MAX_SKETCH_UNKNOWNS,
    MAX_SKETCH_VALUES, SizeProblem, format_number, squared_distance,
};

use super::PROBLEMS_LISTED;
use super::sketch::{ElementId, ElementKind, Form, Reference, Sketch, Takes, TangentSide};
use crate::Error;
use crate::profile::{Drawn, sent, unsigned};
use crate::references::CANDIDATES;

/// What a line or arc too small is refused for: its ends within the kernel's tolerance.
const ENDS_RULE: &str =
    "lines and arcs are longer than 1e-7 mm between their ends, as the kernel measures them";

/// What a circle or arc too small is refused for: its radius within the kernel's tolerance.
const RADIUS_RULE: &str =
    "arcs and circles have a radius above 1e-7 mm, as the kernel measures them";

/// Whether `error` is the gate's refusal of a curve too small ([`ENDS_RULE`],
/// [`RADIUS_RULE`]).
pub(crate) fn too_small(error: &Error) -> bool {
    let message = error.message();
    message.ends_with(ENDS_RULE) || message.ends_with(RADIUS_RULE)
}

/// What's too small about a curve drawn as `drawn`, measured as the kernel request measures
/// it ([`silico_protocol::Curve::size_problem`]), after the profile's own conversion
/// ([`sent`]): at most [`LENGTH_TOLERANCE`] is no size. None for an arc whose points make
/// none (its ends at one angle from its centre), which the gate refuses otherwise or leaves
/// to the solve.
fn size_problem(drawn: Drawn) -> Option<SizeProblem> {
    let drawn = match drawn {
        Drawn::Line { from, to } => Drawn::Line {
            from: unsigned(from),
            to: unsigned(to),
        },
        Drawn::Arc { center, start, end } => Drawn::Arc {
            center: unsigned(center),
            start: unsigned(start),
            end: unsigned(end),
        },
        circle @ Drawn::Circle { .. } => circle,
    };
    sent(drawn, false)?.size_problem()
}

/// The largest angle a constraint may have, either way, in degrees.
pub const MAX_ANGLE_DEGREES: i64 = 360;

/// The largest value a reference dimension may hold, in millimetres: more than the
/// farthest two points within a kilometre of the origin can be apart (2√2 km).
pub const MAX_REFERENCE_LENGTH: f64 = 3e6;

/// A refusal of a sketch field: its dotted name, the message, and the suggestion.
pub(super) type Refuse<'a> = &'a dyn Fn(&str, String, String) -> Error;

/// What a sketch's element is, and where: its kind and its place in its list.
#[derive(Clone, Copy)]
struct Place {
    kind: ElementKind,
    index: usize,
}

impl Place {
    fn field(self) -> String {
        format!("{}.{}", self.kind.list(), self.index)
    }
}

/// The sketch's problems, in the order of its fields: each list in turn, each element's
/// fields in order, then its size.
pub(super) fn check(sketch: &Sketch, refuse: Refuse<'_>, add: &mut dyn FnMut(Error)) {
    // Every element by ID: the first with an ID holds it. Each element whose ID an
    // earlier one holds, by place, with that holder.
    let mut by_id: BTreeMap<&ElementId, Place> = BTreeMap::new();
    let mut taken: BTreeMap<(ElementKind, usize), Place> = BTreeMap::new();
    let mut counts: BTreeMap<ElementKind, usize> = BTreeMap::new();
    for (id, kind) in sketch.elements() {
        let slot = counts.entry(kind).or_default();
        let place = Place { kind, index: *slot };
        *slot += 1;
        match by_id.get(id) {
            Some(&first) => {
                taken.insert((kind, place.index), first);
            }
            None => {
                by_id.insert(id, place);
            }
        }
    }
    // The element's ID, if an earlier element holds it.
    let unique = |kind: ElementKind, index: usize, id: &ElementId, add: &mut dyn FnMut(Error)| {
        if let Some(first) = taken.get(&(kind, index)) {
            let place = Place { kind, index };
            add(refuse(
                &format!("{}.id", place.field()),
                format!(
                    "id {id} is already the ID of {} {id} ({})",
                    first.kind.noun(),
                    first.field()
                ),
                format!("give this {} an ID of its own", kind.noun()),
            ));
        }
    };

    for (i, point) in sketch.points.iter().enumerate() {
        let mut named = |e: Error| add(e.with_element(point.id.as_str()));
        let add: &mut dyn FnMut(Error) = &mut named;
        unique(ElementKind::Point, i, &point.id, add);
        for (axis, value) in [("x", point.x), ("y", point.y)] {
            if value.mm().abs() > MAX_LENGTH {
                add(refuse(
                    &format!("points.{i}.{axis}"),
                    format!(
                        "points.{i}.{axis} is {value} mm, farther from the sketch's origin than the {} mm (a kilometre) a point may be",
                        format_number(MAX_LENGTH)
                    ),
                    "draw the sketch within a kilometre of its origin".to_owned(),
                ));
            }
        }
    }
    let lookup = |id: &ElementId| by_id.get(id).copied();
    // The IDs a reference to `id`, which no element holds, most likely meant: each of an
    // element that `fits` its place and at most two characters from `id` (one mistyped, or
    // two swapped), the nearest first, then by ID; at most `CANDIDATES`. Only the first
    // `PROBLEMS_LISTED` such references get them, as no more of a document's problems are
    // listed, so a sketch of thousands doesn't compare each with every ID.
    let unknown = std::cell::Cell::new(0usize);
    let candidates = |id: &ElementId, fits: &dyn Fn(Place) -> bool| -> Vec<String> {
        let seen = unknown.get();
        unknown.set(seen + 1);
        if seen >= PROBLEMS_LISTED {
            return Vec::new();
        }
        let mut near: Vec<(usize, &ElementId)> = by_id
            .iter()
            .filter(|&(_, &place)| fits(place))
            .map(|(&other, _)| (differing(id, other), other))
            .filter(|&(n, _)| n <= 2)
            .collect();
        // Stable, so IDs as near stay in the map's order, by ID.
        near.sort_by_key(|&(n, _)| n);
        near.into_iter()
            .take(CANDIDATES)
            .map(|(_, other)| other.as_str().to_owned())
            .collect()
    };
    // Where a point of this sketch is drawn.
    let at_of = |id: &ElementId| match lookup(id) {
        Some(Place {
            kind: ElementKind::Point,
            index,
        }) => sketch.points.get(index).map(|p| [p.x.mm(), p.y.mm()]),
        _ => None,
    };

    // A curve's own points: each a point of this sketch.
    let point_of = |field: String, id: &ElementId, add: &mut dyn FnMut(Error)| match lookup(id) {
        Some(Place {
            kind: ElementKind::Point,
            ..
        }) => true,
        Some(other) => {
            add(refuse(
                &field,
                format!("{field} {id} is a {}, not a point", other.kind.noun()),
                format!("name one of this sketch's points in {field}"),
            ));
            false
        }
        None => {
            let near = candidates(id, &|place| place.kind == ElementKind::Point);
            add(refuse(
                &field,
                format!("{field} {id} isn't the ID of any element of this sketch"),
                with_candidates(
                    format!("name one of this sketch's points in {field}"),
                    id,
                    &near,
                ),
            )
            .with_candidates(near));
            false
        }
    };

    for (i, line) in sketch.lines.iter().enumerate() {
        let at = format!("lines.{i}");
        let mut named = |e: Error| add(e.with_element(line.id.as_str()));
        let add: &mut dyn FnMut(Error) = &mut named;
        unique(ElementKind::Line, i, &line.id, add);
        let from = point_of(format!("{at}.from"), &line.from, add);
        let to = point_of(format!("{at}.to"), &line.to, add);
        if from && to && line.from == line.to {
            add(refuse(
                &format!("{at}.to"),
                format!(
                    "line {} goes from point {} to itself, so it has no length",
                    line.id, line.from
                ),
                "end the line at another point".to_owned(),
            ));
        } else if let (Some(a), Some(b)) = (at_of(&line.from), at_of(&line.to))
            && let Some(SizeProblem::Ends(length)) = size_problem(Drawn::Line { from: a, to: b })
        {
            // Ends within the kernel's tolerance are one place to it (the profile's rule):
            // a line of no size.
            add(refuse(
                &format!("{at}.to"),
                format!(
                    "line {} is {} mm long, from point {} at ({}, {}) to {} at ({}, {}): {ENDS_RULE}",
                    line.id,
                    format_number(length),
                    line.from,
                    format_number(a[0]),
                    format_number(a[1]),
                    line.to,
                    format_number(b[0]),
                    format_number(b[1])
                ),
                "draw the line's ends further apart".to_owned(),
            )
            .with_element(line.from.as_str())
            .with_element(line.to.as_str()));
        }
    }
    for (i, circle) in sketch.circles.iter().enumerate() {
        let at = format!("circles.{i}");
        let mut named = |e: Error| add(e.with_element(circle.id.as_str()));
        let add: &mut dyn FnMut(Error) = &mut named;
        unique(ElementKind::Circle, i, &circle.id, add);
        point_of(format!("{at}.center"), &circle.center, add);
        let radius = circle.radius.mm();
        if let Some(SizeProblem::Radius(r)) = size_problem(Drawn::Circle {
            center: [0.0, 0.0],
            radius,
        }) {
            add(refuse(
                &format!("{at}.radius"),
                format!(
                    "circle {}'s radius is {} mm: {RADIUS_RULE}",
                    circle.id,
                    format_number(r)
                ),
                "draw the circle larger".to_owned(),
            ));
        } else if radius > MAX_LENGTH {
            add(refuse(
                &format!("{at}.radius"),
                format!(
                    "circle {}'s radius is {} mm, more than the {} mm (a kilometre) the solver takes",
                    circle.id,
                    format_number(radius),
                    format_number(MAX_LENGTH)
                ),
                "draw the circle smaller".to_owned(),
            ));
        }
    }
    for (i, arc) in sketch.arcs.iter().enumerate() {
        let at = format!("arcs.{i}");
        let mut named = |e: Error| add(e.with_element(arc.id.as_str()));
        let add: &mut dyn FnMut(Error) = &mut named;
        unique(ElementKind::Arc, i, &arc.id, add);
        let center = point_of(format!("{at}.center"), &arc.center, add);
        let start = point_of(format!("{at}.start"), &arc.start, add);
        let end = point_of(format!("{at}.end"), &arc.end, add);
        for (key, id, ok) in [("start", &arc.start, start), ("end", &arc.end, end)] {
            if center && ok && *id == arc.center {
                add(refuse(
                    &format!("{at}.{key}"),
                    format!(
                        "arc {}'s {key} is its center, point {id}, so it has no radius",
                        arc.id
                    ),
                    format!("give the arc a {key} point of its own"),
                ));
            }
        }
        // An end `length` from the center, at most the tolerance: at it, with no angle.
        let at_center = |key: &str, id: &ElementId, p: [f64; 2], c: [f64; 2], length: f64| {
            refuse(
                &format!("{at}.{key}"),
                format!(
                    "arc {}'s {key}, point {id} at ({}, {}), is {} mm from its center, point {} at ({}, {}): {RADIUS_RULE}",
                    arc.id,
                    format_number(p[0]),
                    format_number(p[1]),
                    format_number(length),
                    arc.center,
                    format_number(c[0]),
                    format_number(c[1])
                ),
                format!("draw the arc's {key} away from its center"),
            )
            .with_element(id.as_str())
            .with_element(arc.center.as_str())
        };
        // Drawn with no size as the kernel measures it, or with a radius past what the
        // solver takes: the radius is the center's distance to the start (D2, arcs), and
        // an end within the tolerance of the center is at it, with no angle.
        let mut sized = true;
        if let Some(c) = at_of(&arc.center) {
            for (key, id) in [("start", &arc.start), ("end", &arc.end)] {
                if *id == arc.center {
                    sized = false;
                    continue;
                }
                let Some(p) = at_of(id) else {
                    continue;
                };
                let length = silico_protocol::distance(c, p);
                if length <= LENGTH_TOLERANCE {
                    sized = false;
                    add(at_center(key, id, p, c, length));
                }
            }
            if let Some(p) = at_of(&arc.start) {
                let radius = squared_distance(c, p).sqrt();
                if radius > MAX_LENGTH {
                    add(refuse(
                        &format!("{at}.start"),
                        format!(
                            "arc {}'s radius, from its center to its start, is {} mm, more than the {} mm (a kilometre) the solver takes",
                            arc.id,
                            format_number(radius),
                            format_number(MAX_LENGTH)
                        ),
                        "draw the arc smaller".to_owned(),
                    ));
                }
            }
        }
        if arc.start != arc.end
            && let (Some(a), Some(b)) = (at_of(&arc.start), at_of(&arc.end))
        {
            if a == b {
                add(refuse(
                    &format!("{at}.end"),
                    format!(
                        "arc {}'s start and end, points {} and {}, are both drawn at ({}, {}), so its sweep is 0 or a whole turn (a whole circle is a circle)",
                        arc.id,
                        arc.start,
                        arc.end,
                        format_number(a[0]),
                        format_number(a[1])
                    ),
                    "draw the arc's end away from its start, or draw a circle".to_owned(),
                ));
            } else if sized
                && let Some(center) = at_of(&arc.center)
                && let Some(problem) = size_problem(Drawn::Arc {
                    center,
                    start: a,
                    end: b,
                })
            {
                add(match problem {
                    SizeProblem::Ends(d) => refuse(
                        &format!("{at}.end"),
                        format!(
                            "arc {}'s ends, points {} and {}, are {} mm apart: {ENDS_RULE}",
                            arc.id,
                            arc.start,
                            arc.end,
                            format_number(d)
                        ),
                        "draw a circle instead, or open the arc".to_owned(),
                    )
                    .with_element(arc.start.as_str())
                    .with_element(arc.end.as_str()),
                    // The radius as the request carries it, the square root of the squared
                    // distance from the center to the start, at most the tolerance where
                    // the start's own distance, `hypot`'s, is a rounding above it: refused
                    // as the start's check refuses it, as the start's.
                    SizeProblem::Radius(r) => at_center("start", &arc.start, a, center, r),
                });
            }
        }
        if start && end && arc.start == arc.end && arc.start != arc.center {
            add(refuse(
                &format!("{at}.end"),
                format!(
                    "arc {} starts and ends at point {}, so it has no sweep (a whole circle is a circle)",
                    arc.id, arc.start
                ),
                "end the arc at another point, or draw a circle".to_owned(),
            ));
        }
    }

    // Every `on`, by its point and curve, for the tangent rule below.
    let ons: Vec<(&Reference, &Reference, &ElementId)> = sketch
        .constraints
        .iter()
        .filter_map(|c| match &c.form {
            Form::On { point, curve } => Some((point, curve, &c.id)),
            _ => None,
        })
        .collect();

    for (i, constraint) in sketch.constraints.iter().enumerate() {
        let at = format!("constraints.{i}");
        let mut named = |e: Error| add(e.with_element(constraint.id.as_str()));
        let add: &mut dyn FnMut(Error) = &mut named;
        unique(ElementKind::Constraint, i, &constraint.id, add);
        let form = &constraint.form;
        let what = form.type_name();
        let references = form.references();
        // Each reference resolved, if it's of a kind its place takes.
        let mut resolved: Vec<Option<Target>> = Vec::with_capacity(references.len());
        for &(key, reference, takes) in &references {
            let field = format!("{at}.{key}");
            let target = match reference {
                Reference::Origin => Some(Target::Origin),
                Reference::XAxis | Reference::YAxis => Some(Target::Axis),
                Reference::Element(id) => match lookup(id) {
                    Some(place) => Some(Target::Element(place)),
                    None => {
                        let near = candidates(id, &|place| Target::Element(place).fits(takes));
                        add(refuse(
                            &field,
                            format!("{field} {id} isn't the ID of any element of this sketch"),
                            with_candidates(
                                format!("name {} in {field}", takes_noun(takes)),
                                id,
                                &near,
                            ),
                        )
                        .with_candidates(near));
                        None
                    }
                },
            };
            let fits = target.is_some_and(|t| t.fits(takes));
            if let Some(target) = target.filter(|_| !fits) {
                add(refuse(
                    &field,
                    format!(
                        "{field} {reference} is {}, but a {what}'s {key} is {}",
                        target.noun(),
                        takes_noun(takes)
                    ),
                    format!("name {} in {field}", takes_noun(takes)),
                ));
            }
            resolved.push(target.filter(|_| fits));
        }
        // From here on, only a constraint whose references all fit.
        if resolved.iter().any(Option::is_none) {
            continue;
        }
        let place = |n: usize| match resolved.get(n).copied().flatten() {
            Some(Target::Element(place)) => Some(place),
            _ => None,
        };
        // Whether reference `n` is a line or an axis.
        let is_line = |n: usize| match resolved.get(n).copied().flatten() {
            Some(Target::Axis) => true,
            Some(Target::Element(p)) => p.kind == ElementKind::Line,
            _ => false,
        };

        // Two references to one element where the form needs two.
        if let [(ka, ra, _), (kb, rb, _), ..] = references.as_slice()
            && ra == rb
        {
            add(refuse(
                &format!("{at}.{kb}"),
                format!("a {what}'s {ka} and {kb} are both {ra}: it needs two different elements"),
                format!("name another element in {kb}"),
            ));
        }
        if let Form::Symmetric { a, b, about } = form
            && (about == a || about == b)
        {
            add(refuse(
                &format!("{at}.about"),
                format!("a symmetric's about, {about}, is one of the points it mirrors"),
                "mirror the points about another point or a line".to_owned(),
            ));
        }

        match form {
            Form::Equal { .. } => {
                let kinds = (place(0).map(|p| p.kind), place(1).map(|p| p.kind));
                let round = |k: Option<ElementKind>| {
                    matches!(k, Some(ElementKind::Circle | ElementKind::Arc))
                };
                let lines =
                    kinds.0 == Some(ElementKind::Line) && kinds.1 == Some(ElementKind::Line);
                if !lines && !(round(kinds.0) && round(kinds.1)) {
                    add(refuse(
                        &format!("{at}.b"),
                        "an equal takes two lines, or two circles or arcs".to_owned(),
                        "make both lines, or both circles or arcs".to_owned(),
                    ));
                }
            }
            Form::Tangent { a, b, side } => {
                // Two curves sharing an end are tangent there or not at all: FreeCAD's
                // tangent command makes that a tangent_at (180d722a53), since an edge
                // tangency with the shared point is redundant where it holds.
                let shared = match (a.element(), b.element()) {
                    // A curve tangent to itself is refused above, as needing two elements.
                    (Some(a), Some(b)) if a != b => {
                        let theirs = ends_of(sketch, b, lookup);
                        ends_of(sketch, a, lookup)
                            .into_iter()
                            .find(|end| theirs.contains(end))
                    }
                    _ => None,
                };
                if let Some(point) = shared {
                    add(refuse(
                        &format!("{at}.b"),
                        format!(
                            "a tangent's {a} and {b} share an end, point {point}: curves that meet are tangent where they meet"
                        ),
                        format!("make it a tangent_at, at point {point}"),
                    ));
                } else if let Some((point, ending, other, on)) =
                    [(a, b), (b, a)].into_iter().find_map(|(x, y)| {
                        let ends = x.element().map(|x| ends_of(sketch, x, lookup))?;
                        ons.iter()
                            .find(|(p, c, _)| {
                                *c == y && p.element().is_some_and(|p| ends.contains(&p))
                            })
                            .map(|(p, _, id)| (*p, x, y, *id))
                    })
                {
                    // A curve that ends on another is tangent there or not at all: FreeCAD
                    // makes an `on` and an edge tangency one endpoint-to-edge tangency
                    // (3a656d8597), since the pair is redundant where it holds.
                    add(refuse(
                        &format!("{at}.b"),
                        format!(
                            "a tangent's {ending} ends at point {point}, which constraint {on} puts on {other}: a curve ending on another is tangent where it ends"
                        ),
                        format!(
                            "make the two one tangent_on, with a {ending}, at {point}, and b {other}"
                        ),
                    ));
                }
                let (line_a, line_b) = (is_line(0), is_line(1));
                let fits = match side {
                    TangentSide::Left | TangentSide::Right => line_a != line_b,
                    TangentSide::Outside | TangentSide::Inside => !line_a && !line_b,
                };
                if line_a && line_b {
                    add(refuse(
                        &format!("{at}.b"),
                        "a tangent takes a line and a circle or arc, or two circles or arcs: two lines can't be tangent".to_owned(),
                        "make one of them a circle or arc, or use parallel".to_owned(),
                    ));
                } else if !fits {
                    let want = if line_a || line_b {
                        "left or right (the side of the line the curve is on)"
                    } else {
                        "outside or inside (each outside the other, or one inside the other)"
                    };
                    add(refuse(
                        &format!("{at}.side"),
                        format!("a tangent's side for these curves is {want}"),
                        format!("write side as {want}"),
                    ));
                }
            }
            Form::TangentOn { a, b, .. } if a == b => {
                add(refuse(
                    &format!("{at}.b"),
                    format!("a tangent_on's a and b are both {a}: it needs two different curves"),
                    "name another curve in b".to_owned(),
                ));
            }
            Form::TangentOn { a, at: end, b, .. } => {
                if let Some(point) = end.element() {
                    let ends = a.element().map(|a| ends_of(sketch, a, lookup));
                    let ends_b = b.element().map(|b| ends_of(sketch, b, lookup));
                    if !ends.is_some_and(|e| e.contains(&point)) {
                        add(refuse(
                            &format!("{at}.at"),
                            format!("a tangent_on's at, point {point}, isn't an end of its a, {a}"),
                            format!("name the end of {a} that lies on {b}"),
                        ));
                    } else if ends_b.is_some_and(|e| e.contains(&point)) {
                        add(refuse(
                            &format!("{at}.at"),
                            format!(
                                "a tangent_on's at, point {point}, is an end of its b, {b}, too: the curves meet there"
                            ),
                            format!("make it a tangent_at, at point {point}"),
                        ));
                    } else if b
                        .element()
                        .is_some_and(|b| own_points(sketch, b, lookup).contains(&point))
                    {
                        // b's own centre: the end on b would need it of no radius.
                        add(refuse(
                            &format!("{at}.at"),
                            format!(
                                "a tangent_on's at, point {point}, is part of its b, {b}, itself"
                            ),
                            format!(
                                "name the end of {a} that lies on {b}, not a point of {b}'s own"
                            ),
                        ));
                    }
                }
                // Index 0 is a, 2 is b.
                if is_line(0) && is_line(2) {
                    add(refuse(
                        &format!("{at}.b"),
                        "a tangent_on takes a curve ending on a circle or arc, or an arc ending on a line: two lines can't be tangent".to_owned(),
                        "make one of them a circle or arc, or use parallel and on".to_owned(),
                    ));
                }
            }
            Form::TangentAt {
                a, b, at: shared, ..
            } => {
                if let Some(point) = shared.element() {
                    for (key, curve) in [("a", a), ("b", b)] {
                        let ends = curve.element().map(|c| ends_of(sketch, c, lookup));
                        if !ends.is_some_and(|e| e.contains(&point)) {
                            add(refuse(
                                &format!("{at}.at"),
                                format!(
                                    "a tangent_at's at, point {point}, isn't an end of its {key}, {curve}"
                                ),
                                format!("name the point where {a} and {b} meet"),
                            ));
                        }
                    }
                }
            }
            _ => {}
        }

        // A curve's own point constrained onto that curve: on it already, or never (an end
        // on its line, a center on its circle), as FreeCAD's commands refuse
        // (CommandConstraints.cpp, "parts of the same element", and a symmetry of a line's
        // ends about that line).
        let own = |point: &Reference, curve: &Reference| match (point.element(), curve.element()) {
            (Some(p), Some(c)) => own_points(sketch, c, lookup).contains(&p),
            _ => false,
        };
        match form {
            Form::On { point, curve }
            | Form::Midpoint { point, line: curve }
            | Form::PointLineDistance {
                point, line: curve, ..
            } if own(point, curve) => {
                add(refuse(
                    &format!("{at}.point"),
                    format!("a {what}'s point, {point}, is part of {curve} itself"),
                    format!("name a point that isn't part of {curve}"),
                ));
            }
            Form::Symmetric { a, b, about } if own(a, about) && own(b, about) => {
                add(refuse(
                    &format!("{at}.about"),
                    format!("a symmetric's a and b, {a} and {b}, are the ends of {about} itself"),
                    "mirror the points about another line or a point".to_owned(),
                ));
            }
            // A point on the line mirrors onto itself: the other would have to be the same
            // point, which two of the sketch's points can't be (break-it: it solves once,
            // then the next solve finds them at one place).
            Form::Symmetric { a, b, about } if own(a, about) || own(b, about) => {
                let end = if own(a, about) { a } else { b };
                add(refuse(
                    &format!("{at}.about"),
                    format!("a symmetric's {end} is an end of {about}, so it mirrors onto itself"),
                    "share the point instead, or mirror about another line or a point".to_owned(),
                ));
            }
            _ => {}
        }

        // Every constraint names an element of the sketch's own (FreeCAD's 3d7871fca5).
        if references.iter().all(|(_, r, _)| r.element().is_none()) {
            add(refuse(
                &at,
                format!(
                    "this {what} names only the sketch's fixed references, so it constrains nothing in the sketch"
                ),
                "name an element of this sketch".to_owned(),
            ));
        }

        // A driving dimension is one the solver holds: above 0 (unless signed) and within
        // what it takes. A reference one is what the last solve measured, so it may be 0,
        // and as long as the farthest two points within a kilometre can be apart.
        if let (Some((value, signed)), Some(driving)) = (form.length(), form.driving()) {
            let most = match form {
                _ if !driving => MAX_REFERENCE_LENGTH,
                Form::Diameter { .. } => 2.0 * MAX_LENGTH,
                _ => MAX_LENGTH,
            };
            let mm = value.mm();
            let low = if signed {
                false
            } else if driving {
                value.nm() <= 0
            } else {
                value.nm() < 0
            };
            if low || mm.abs() > most {
                let range = if signed {
                    format!("at most {} mm either way", format_number(most))
                } else if driving {
                    format!("above 0 and at most {} mm", format_number(most))
                } else {
                    format!("from 0 to {} mm", format_number(most))
                };
                let kind = if driving { "" } else { "reference " };
                add(refuse(
                    &format!("{at}.value"),
                    format!("a {kind}{what}'s value is {value} mm, but it's {range}"),
                    format!("give the {what} a value {range}"),
                ));
            }
        }
        if let Form::Angle { value, .. } = form
            && value.ndeg().unsigned_abs() > MAX_ANGLE_DEGREES.unsigned_abs() * 1_000_000_000
        {
            add(refuse(
                &format!("{at}.value"),
                format!(
                    "an angle's value is {value} degrees, but it's at most {MAX_ANGLE_DEGREES} either way"
                ),
                format!(
                    "give the angle a value from -{MAX_ANGLE_DEGREES} to {MAX_ANGLE_DEGREES} degrees"
                ),
            ));
        }
    }

    for (key, n, most, what) in size(sketch) {
        if n > most {
            add(refuse(
                key,
                format!("the sketch has {n} {what}, more than the {most} the sketch solver takes"),
                "split the sketch into smaller sketches: a single profile past this is more work than the sketch solver takes".to_owned(),
            ));
        }
    }
}

/// What a reference resolved to.
#[derive(Clone, Copy)]
enum Target {
    Element(Place),
    Origin,
    Axis,
}

impl Target {
    fn fits(self, takes: Takes) -> bool {
        use ElementKind::*;
        let kind = match self {
            Target::Element(place) => Some(place.kind),
            Target::Origin | Target::Axis => None,
        };
        match takes {
            Takes::Point => matches!(self, Target::Origin) || kind == Some(Point),
            Takes::SharedEnd | Takes::EndOn => kind == Some(Point),
            Takes::Line => kind == Some(Line),
            Takes::LineOrAxis => matches!(self, Target::Axis) || kind == Some(Line),
            Takes::Curve => {
                matches!(self, Target::Axis) || matches!(kind, Some(Line | Circle | Arc))
            }
            Takes::Round => matches!(kind, Some(Circle | Arc)),
            Takes::EndedCurve => matches!(kind, Some(Line | Arc)),
            Takes::Equal => matches!(kind, Some(Line | Circle | Arc)),
            Takes::LineOrPoint => {
                !matches!(self, Target::Element(_)) || matches!(kind, Some(Line | Point))
            }
        }
    }

    fn noun(self) -> String {
        match self {
            Target::Element(place) => format!("a {}", place.kind.noun()),
            Target::Origin => "the origin".to_owned(),
            Target::Axis => "an axis".to_owned(),
        }
    }
}

/// How many places two IDs differ in, character by character: a mistyped character is one,
/// two swapped are two. Every ID has `ID_LENGTH` characters, so one with a character
/// dropped or added isn't an ID, and the reader refuses it first.
fn differing(a: &ElementId, b: &ElementId) -> usize {
    a.as_str()
        .chars()
        .zip(b.as_str().chars())
        .filter(|(x, y)| x != y)
        .count()
}

/// The advice for a reference to `id`, which no element holds, saying what its candidates
/// are when it has any.
fn with_candidates(advice: String, id: &ElementId, candidates: &[String]) -> String {
    if candidates.is_empty() {
        advice
    } else {
        format!(
            "{advice}: the candidates are this sketch's IDs a character or two from {id}, none taken for it"
        )
    }
}

/// What a place takes, as messages say it.
fn takes_noun(takes: Takes) -> &'static str {
    match takes {
        Takes::Point => "a point of this sketch, or origin",
        Takes::SharedEnd => "a point of this sketch where the two curves meet",
        Takes::EndOn => "a point of this sketch where a ends, on b",
        Takes::Line => "a line of this sketch",
        Takes::LineOrAxis => "a line of this sketch, x_axis, or y_axis",
        Takes::Curve => "a line, circle, or arc of this sketch, x_axis, or y_axis",
        Takes::Round => "a circle or arc of this sketch",
        Takes::EndedCurve => "a line or arc of this sketch",
        Takes::Equal => "a line, circle, or arc of this sketch",
        Takes::LineOrPoint => "a line or point of this sketch, origin, x_axis, or y_axis",
    }
}

/// A curve's own points: a line's ends, a circle's center, an arc's center and ends.
fn own_points<'a>(
    sketch: &'a Sketch,
    curve: &ElementId,
    lookup: impl Fn(&ElementId) -> Option<Place>,
) -> Vec<&'a ElementId> {
    match lookup(curve) {
        Some(Place {
            kind: ElementKind::Circle,
            index,
        }) => sketch
            .circles
            .get(index)
            .map(|c| vec![&c.center])
            .unwrap_or_default(),
        Some(Place {
            kind: ElementKind::Arc,
            index,
        }) => sketch
            .arcs
            .get(index)
            .map(|a| vec![&a.center, &a.start, &a.end])
            .unwrap_or_default(),
        _ => ends_of(sketch, curve, lookup),
    }
}

/// A line's or arc's two ends (none for anything else).
fn ends_of<'a>(
    sketch: &'a Sketch,
    curve: &ElementId,
    lookup: impl Fn(&ElementId) -> Option<Place>,
) -> Vec<&'a ElementId> {
    match lookup(curve) {
        Some(Place {
            kind: ElementKind::Line,
            index,
        }) => sketch
            .lines
            .get(index)
            .map(|l| vec![&l.from, &l.to])
            .unwrap_or_default(),
        Some(Place {
            kind: ElementKind::Arc,
            index,
        }) => sketch
            .arcs
            .get(index)
            .map(|a| vec![&a.start, &a.end])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The sketch as the sketch worker will take it, at most (M3.3's translation stays within
/// these): each limit's field, the count, the limit, and what's counted. Every point's x
/// and y, a circle's radius, and an arc's radius and two angles are unknowns; the values
/// are the unknowns, the driving dimensions', a tangency's fixed angle for each
/// `tangent_on` and `tangent_at` (FreeCAD's translation fixes one), and the origin's and
/// the axes' ends (the translation's, silico_core::sketch). A reference dimension's value
/// isn't the solver's: it's evaluated from the solved geometry, as FreeCAD's main does
/// (`92894ea3a8`), not solved for as 1.1.3 does (docs/ports/sketcher.md), so it may be past
/// what the worker takes. The worker's Jacobian limit can't be reached within these: at
/// most two equations for each of 10,000 constraints and four for each arc's rules, times
/// 1,024 unknowns, is under 2^25 entries.
pub(super) fn size(sketch: &Sketch) -> [(&'static str, usize, usize, &'static str); 4] {
    let unknowns = 2 * sketch.points.len() + sketch.circles.len() + 3 * sketch.arcs.len();
    // The origin and the two axes are geometry too.
    let geometry =
        sketch.points.len() + sketch.lines.len() + sketch.circles.len() + sketch.arcs.len() + 3;
    let constraints = sketch.constraints.len() + sketch.arcs.len();
    let dimensions = sketch
        .constraints
        .iter()
        .filter(|c| c.form.driving() == Some(true))
        .count();
    let angles = sketch
        .constraints
        .iter()
        .filter(|c| matches!(c.form, Form::TangentOn { .. } | Form::TangentAt { .. }))
        .count();
    // The origin and the axes' far ends, fixed values all three.
    let values = unknowns + dimensions + angles + 6;
    [
        (
            "points",
            unknowns,
            MAX_SKETCH_UNKNOWNS,
            "unknowns (two for each point, one for each circle, three for each arc)",
        ),
        ("points", values, MAX_SKETCH_VALUES, "values"),
        (
            "lines",
            geometry,
            MAX_SKETCH_GEOMETRY,
            "points, lines, circles, and arcs",
        ),
        (
            "constraints",
            constraints,
            MAX_SKETCH_CONSTRAINTS,
            "constraints (counting each arc's rules as one)",
        ),
    ]
}
