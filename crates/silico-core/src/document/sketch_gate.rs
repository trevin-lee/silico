//! The validation gate's rules for a sketch (docs/design/m3.2-sketch-format.md, *The gate's
//! rules for sketches*): what FreeCAD finds only when it solves, or never, refused here,
//! naming the field and the element. Every ID is unique within its sketch; every reference
//! names an element of a kind its place takes; nothing is drawn with no size; dimensions
//! are in range; every constraint names an element of the sketch's own; and the sketch is
//! one the sketch worker takes. Whether an arc's ends lie on its circle is the solve's to
//! mend or name (D2, arcs), not the gate's.

use std::collections::BTreeMap;

use silico_protocol::{
    MAX_JACOBIAN_ENTRIES, MAX_LENGTH, MAX_SKETCH_CONSTRAINTS, MAX_SKETCH_GEOMETRY,
    MAX_SKETCH_UNKNOWNS, MAX_SKETCH_VALUES, format_number,
};

use super::sketch::{ElementId, ElementKind, Form, Reference, Sketch, Takes, TangentSide};
use crate::Error;

/// The largest angle a constraint may have, either way, in degrees.
pub const MAX_ANGLE_DEGREES: i64 = 360;

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
    let mut counts = [0usize; 5];
    for (id, kind) in sketch.elements() {
        let slot = &mut counts[kind as usize];
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
            add(refuse(
                &field,
                format!("{field} {id} isn't the ID of any element of this sketch"),
                format!("name one of this sketch's points in {field}"),
            ));
            false
        }
    };

    for (i, line) in sketch.lines.iter().enumerate() {
        let at = format!("lines.{i}");
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
        }
    }
    for (i, circle) in sketch.circles.iter().enumerate() {
        let at = format!("circles.{i}");
        unique(ElementKind::Circle, i, &circle.id, add);
        point_of(format!("{at}.center"), &circle.center, add);
        let radius = circle.radius.mm();
        if radius <= 0.0 || radius > MAX_LENGTH {
            add(refuse(
                &format!("{at}.radius"),
                format!(
                    "{at}.radius is {} mm: a circle's radius is above 0 and at most {} mm (a kilometre)",
                    circle.radius,
                    format_number(MAX_LENGTH)
                ),
                "give the circle a radius above 0".to_owned(),
            ));
        }
    }
    for (i, arc) in sketch.arcs.iter().enumerate() {
        let at = format!("arcs.{i}");
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

    for (i, constraint) in sketch.constraints.iter().enumerate() {
        let at = format!("constraints.{i}");
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
                        add(refuse(
                            &field,
                            format!("{field} {id} isn't the ID of any element of this sketch"),
                            format!("name {} in {field}", takes_noun(takes)),
                        ));
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
            Form::Tangent { side, .. } => {
                let is_line = |n: usize| match resolved.get(n).copied().flatten() {
                    Some(Target::Axis) => true,
                    Some(Target::Element(p)) => p.kind == ElementKind::Line,
                    _ => false,
                };
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

        if let Some((value, signed)) = form.length() {
            let most = match form {
                Form::Diameter { .. } => 2.0 * MAX_LENGTH,
                _ => MAX_LENGTH,
            };
            let mm = value.mm();
            if (!signed && value.nm() <= 0) || mm.abs() > most {
                let range = if signed {
                    format!("at most {} mm either way", format_number(most))
                } else {
                    format!("above 0 and at most {} mm", format_number(most))
                };
                add(refuse(
                    &format!("{at}.value"),
                    format!("a {what}'s value is {value} mm, but it's {range}"),
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
                "split the sketch into smaller sketches".to_owned(),
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
            Takes::SharedEnd => kind == Some(Point),
            Takes::Line => kind == Some(Line),
            Takes::LineOrAxis => matches!(self, Target::Axis) || kind == Some(Line),
            Takes::Curve | Takes::TangentCurve => {
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

/// What a place takes, as messages say it.
fn takes_noun(takes: Takes) -> &'static str {
    match takes {
        Takes::Point => "a point of this sketch, or origin",
        Takes::SharedEnd => "a point of this sketch where the two curves meet",
        Takes::Line => "a line of this sketch",
        Takes::LineOrAxis => "a line of this sketch, x_axis, or y_axis",
        Takes::Curve | Takes::TangentCurve => {
            "a line, circle, or arc of this sketch, x_axis, or y_axis"
        }
        Takes::Round => "a circle or arc of this sketch",
        Takes::EndedCurve => "a line or arc of this sketch",
        Takes::Equal => "a line, circle, or arc of this sketch",
        Takes::LineOrPoint => "a line or point of this sketch, origin, x_axis, or y_axis",
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
/// and y, a circle's radius, and an arc's radius and two angles are unknowns; each
/// constraint is one or two equations (a symmetry and a midpoint two), and an arc's rules
/// four; the values are the unknowns, the dimensions', and the origin's.
pub(super) fn size(sketch: &Sketch) -> [(&'static str, usize, usize, &'static str); 5] {
    let unknowns = 2 * sketch.points.len() + sketch.circles.len() + 3 * sketch.arcs.len();
    let geometry =
        sketch.points.len() + sketch.lines.len() + sketch.circles.len() + sketch.arcs.len();
    let constraints = sketch.constraints.len() + sketch.arcs.len();
    let dimensions = sketch
        .constraints
        .iter()
        .filter(|c| c.form.is_dimension())
        .count();
    let values = unknowns + dimensions + 2;
    let equations: usize = sketch
        .constraints
        .iter()
        .map(|c| match c.form {
            Form::Symmetric { .. } | Form::Midpoint { .. } => 2,
            _ => 1,
        })
        .sum::<usize>()
        + 4 * sketch.arcs.len();
    let entries = equations.saturating_mul(unknowns);
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
        (
            "constraints",
            entries,
            MAX_JACOBIAN_ENTRIES,
            "entries in its equations' Jacobian (equations × unknowns)",
        ),
    ]
}
