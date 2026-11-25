//! What the pointer shows (docs/design/m5.0-app.md, section 9, *Sketching*; #278's A to G;
//! docs/study/sketch-tool-relations.md, *Recommended for M5.11, by tool*): where a click
//! would put a point, the point it would share or the relation it would leave there, the
//! points it's level with or above, and the direction a line would take.
//!
//! The pointer looks, nearest first within each kind, ties in [`Reference`]'s order
//! (Silico's rule): a curve of the sketch against the origin or an axis, the sketch's curve,
//! never by an ID; among the sketch's own curves, by ID, the document's own order. In this
//! order:
//! 1. a point of the sketch, shared (D2), always, whatever the setting and Ctrl (#278's F);
//! 2. with relations inferred: the origin (#278's A), then a line's middle (with an `on` of
//!    each other curve through it exactly, #278's G, but an axis the line lies along: the
//!    line stands for the axis there; and but one the sketch already implies, #278's G as
//!    amended, [`Implied`]), then where two curves cross (two `on`s; at one
//!    place, the sketch's own pair before one with an axis), then a curve (`on`), each
//!    within the pick tolerance. The pointer finds a line
//!    and an arc only where they're drawn (not past a line's end, nor on an arc's circle
//!    where the arc isn't), a circle all round, and the axes anywhere along them; the `on`
//!    it leaves holds the point on the line through a line's ends, or an arc's whole circle,
//!    as SolidWorks' relations are (§2 of the help);
//! 3. nothing: the point is placed where the pointer is.
//!
//! A point placed on nothing may line up with a point the pointer has woken (#278's E): one
//! it has passed within the pick tolerance of while the tool's in use, or one the tool has
//! added, the last [`WAKE`] of them (SolidWorks' inference wakes a point by hovering it). It's
//! moved onto the line, which a dotted guide shows, and the relation (`horizontal_points`,
//! `vertical_points`) is the pointer's only where the tool says the pointer shows it.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use silico_protocol::{LENGTH_TOLERANCE, arc_from_points};

use super::geometry::{
    add, angle_between, cross, distance, dot, left, scale, sub, to_segment, unit,
};
use super::implied::{Implied, ViewClasses};
use super::{Relation, Shown, Xy};
use crate::document::sketch_edit::{Geo, Members, crossings, lies_along};
use crate::document::{Direction, ElementId, Form, Reference, Sketch};

/// How far from horizontal or vertical a line may be drawn, and from perpendicular,
/// parallel, or tangent, for the pointer to show it: 2°, FreeCAD 1.1.3's, since SolidWorks'
/// help gives none (docs/study/sketch-tool-relations.md, §10 and *Item 26's text*).
pub const ANGLE_TOLERANCE_DEGREES: f64 = 2.0;

/// How many points the pointer keeps woken, the newest first (#278's E, decision of
/// 2026-10-31: only woken points line up).
pub const WAKE: usize = 8;

/// How many of the curves near the pointer, the nearest, are tried for where they cross:
/// so a pointer over a crowd of curves tries at most 32·31/2 pairs.
pub const CROSSING_CURVES: usize = 32;

/// How near counts as exactly, for a relation the tool adds without moving a point to make
/// it hold. As a length, a place on a curve (a line through a line's middle, a centrepoint
/// arc's end the pointer finds on its circle): 1e-9 mm, a picometre, far inside the kernel's
/// 1e-7 mm. As an angle, an arc leaving along an axis: its unit direction's other component
/// at most 1e-9, about 1e-9 rad. Either way the relation holds where it's written.
pub(super) const EXACT: f64 = 1e-9;

fn angle_tolerance() -> f64 {
    ANGLE_TOLERANCE_DEGREES.to_radians()
}

/// How much work the views and the pointer did, counted in tests: each curve's shape built,
/// each pair of curves tried for a crossing, and each line or point on one the implied
/// classes look at as they join.
#[cfg(test)]
pub(super) mod work {
    use std::cell::Cell;

    thread_local! {
        pub(in crate::sketch_tools) static SHAPES: Cell<usize> = const { Cell::new(0) };
        pub(in crate::sketch_tools) static PAIRS: Cell<usize> = const { Cell::new(0) };
        pub(in crate::sketch_tools) static VISITS: Cell<usize> = const { Cell::new(0) };
    }

    pub(in crate::sketch_tools) fn visit() {
        VISITS.with(|c| c.set(c.get() + 1));
    }

    pub(in crate::sketch_tools) fn shape() {
        SHAPES.with(|c| c.set(c.get() + 1));
    }

    pub(in crate::sketch_tools) fn pair() {
        PAIRS.with(|c| c.set(c.get() + 1));
    }
}

/// The sketch a tool snaps to: the newest `sketch` read's members, with what the tool has
/// added since that the read doesn't hold yet (a chain's last segments), so a click can
/// share a point the core hasn't answered with. Each point's place, and each curve's shape,
/// found once.
pub(super) struct View<'s> {
    pub(super) sketch: Cow<'s, Sketch>,
    points: BTreeMap<ElementId, Xy>,
    /// Each line's, circle's, and arc's shape, from `points`: none for one whose points the
    /// sketch hasn't, or an arc with no sweep.
    curves: BTreeMap<ElementId, Geo>,
}

impl<'s> View<'s> {
    /// `read` with each member of `pending` it doesn't hold (by ID) added.
    pub(super) fn of(read: &'s Sketch, pending: &Members) -> View<'s> {
        let sketch = if pending == &Members::default() {
            Cow::Borrowed(read)
        } else {
            let held: BTreeSet<&ElementId> = read.elements().map(|(id, _)| id).collect();
            let missing = |id: &ElementId| !held.contains(id);
            let mut merged = read.clone();
            merged
                .points
                .extend(pending.points.iter().filter(|p| missing(&p.id)).cloned());
            merged
                .lines
                .extend(pending.lines.iter().filter(|l| missing(&l.id)).cloned());
            merged
                .circles
                .extend(pending.circles.iter().filter(|c| missing(&c.id)).cloned());
            merged
                .arcs
                .extend(pending.arcs.iter().filter(|a| missing(&a.id)).cloned());
            merged.constraints.extend(
                pending
                    .constraints
                    .iter()
                    .filter(|c| missing(&c.id))
                    .cloned(),
            );
            Cow::Owned(merged)
        };
        // The first holder of an ID is the one a reference finds, as the gate's.
        let mut points = BTreeMap::new();
        for p in &sketch.points {
            points.entry(p.id.clone()).or_insert([p.x.mm(), p.y.mm()]);
        }
        let at = |id: &ElementId| points.get(id).copied();
        let mut curves = BTreeMap::new();
        for line in &sketch.lines {
            if let (Some(from), Some(to)) = (at(&line.from), at(&line.to)) {
                #[cfg(test)]
                work::shape();
                curves
                    .entry(line.id.clone())
                    .or_insert(Geo::Line { from, to });
            }
        }
        for circle in &sketch.circles {
            if let Some(center) = at(&circle.center) {
                #[cfg(test)]
                work::shape();
                curves.entry(circle.id.clone()).or_insert(Geo::Circle {
                    center,
                    radius: circle.radius.mm(),
                });
            }
        }
        for arc in &sketch.arcs {
            if let (Some(center), Some(start), Some(end)) =
                (at(&arc.center), at(&arc.start), at(&arc.end))
                && let Some([radius, start_angle, end_angle]) = arc_from_points(center, start, end)
            {
                #[cfg(test)]
                work::shape();
                curves.entry(arc.id.clone()).or_insert(Geo::Arc {
                    center,
                    radius,
                    start,
                    end,
                    start_angle,
                    end_angle,
                });
            }
        }
        View {
            sketch,
            points,
            curves,
        }
    }

    /// Where the point `id` is.
    pub(super) fn point(&self, id: &ElementId) -> Option<Xy> {
        self.points.get(id).copied()
    }

    /// Whether the sketch has the curve `id`, with a shape.
    pub(super) fn has_curve(&self, id: &ElementId) -> bool {
        self.curves.contains_key(id)
    }

    /// Whether a reference names what the sketch has: a fixed reference, or a point or a
    /// curve of it.
    pub(super) fn has(&self, reference: &Reference) -> bool {
        match reference {
            Reference::Element(id) => self.points.contains_key(id) || self.has_curve(id),
            Reference::Origin | Reference::XAxis | Reference::YAxis => true,
        }
    }

    /// A curve's or an axis's shape.
    pub(super) fn geo(&self, reference: &Reference) -> Option<Geo> {
        match reference {
            Reference::Element(id) => self.curves.get(id).cloned(),
            Reference::XAxis => Some(Geo::Line {
                from: [0.0, 0.0],
                to: [1.0, 0.0],
            }),
            Reference::YAxis => Some(Geo::Line {
                from: [0.0, 0.0],
                to: [0.0, 1.0],
            }),
            Reference::Origin => None,
        }
    }

    /// Every ID the sketch holds, of any kind.
    pub(super) fn ids(&self) -> BTreeSet<ElementId> {
        self.sketch.elements().map(|(id, _)| id.clone()).collect()
    }

    /// The points within `tolerance` of `at`, nearest first, ties by ID.
    pub(super) fn points_near(&self, at: Xy, tolerance: f64) -> Vec<ElementId> {
        let mut near: Vec<(f64, &ElementId)> = self
            .points
            .iter()
            .map(|(id, &p)| (distance(p, at), id))
            .filter(|(d, _)| *d <= tolerance)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)));
        near.into_iter().map(|(_, id)| id.clone()).collect()
    }

    /// The lines and arcs that end at the point `id`, each with the direction it leaves
    /// there along (outward: a line from its other end, an arc on past its end, or back
    /// past its start), in the sketch's order. One whose direction there can't be found (no
    /// length) is left out.
    pub(super) fn ends_at(&self, id: &ElementId) -> Vec<(ElementId, Xy, EndOf)> {
        let mut ends = Vec::new();
        for line in &self.sketch.lines {
            let other = if &line.to == id {
                &line.from
            } else if &line.from == id {
                &line.to
            } else {
                continue;
            };
            if let (Some(at), Some(far)) = (self.point(id), self.point(other))
                && let Some(out) = unit(sub(at, far))
            {
                ends.push((line.id.clone(), out, EndOf::Line));
            }
        }
        for arc in &self.sketch.arcs {
            // Counter-clockwise, an arc runs on past its end a quarter turn left of the
            // radius there, and back past its start a quarter turn right.
            let sign = if &arc.end == id {
                1.0
            } else if &arc.start == id {
                -1.0
            } else {
                continue;
            };
            if let (Some(at), Some(centre)) = (self.point(id), self.point(&arc.center))
                && let Some(radial) = unit(sub(at, centre))
            {
                ends.push((arc.id.clone(), scale(left(radial), sign), EndOf::Arc));
            }
        }
        ends
    }
}

/// A point held at the origin (#278's A): level with it and above it, `point` first in
/// each, as the drawing tools write a point placed there, and Coincident a point and the
/// origin picked together, in either order (the constraint tools' C1).
pub(super) fn at_origin(point: Reference) -> [Form; 2] {
    [
        Form::HorizontalPoints {
            a: point.clone(),
            b: Reference::Origin,
        },
        Form::VerticalPoints {
            a: point,
            b: Reference::Origin,
        },
    ]
}

/// What kind of curve ends at a point ([`View::ends_at`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EndOf {
    Line,
    Arc,
}

/// Where a click puts a point, and what it leaves there.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Target {
    /// A point the sketch holds, shared (D2): structure, never a relation (#278's F).
    Shared(ElementId),
    /// The origin: a new point, level with it and above it (#278's A).
    Origin,
    /// A line's middle: a new point and `midpoint`, and an `on` of each other curve that
    /// crosses it there (#278's G), but one the sketch already implies with the `midpoint`
    /// held (#278's G as amended, [`Implied`]).
    Midpoint(ElementId, Vec<Reference>),
    /// A curve or an axis: a new point and `on`.
    On(Reference),
    /// Where two curves cross: a new point and two `on`s.
    Crossing(Reference, Reference),
    /// Nothing: a new point.
    Free,
}

/// A point a click places: where, on what, and the points it lines up with.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Placed {
    pub(super) at: Xy,
    pub(super) target: Target,
    /// A point it's level with, the relation the pointer shows (`horizontal_points`).
    pub(super) level_with: Option<ElementId>,
    /// A point it's above or below, the relation the pointer shows (`vertical_points`).
    pub(super) above: Option<ElementId>,
    /// The dotted lines from it to what it lines up with.
    pub(super) guides: Vec<[Xy; 2]>,
}

impl Placed {
    pub(super) fn new(at: Xy, target: Target) -> Self {
        Placed {
            at,
            target,
            level_with: None,
            above: None,
            guides: Vec::new(),
        }
    }

    /// The point it shares, if it shares one.
    pub(super) fn shared(&self) -> Option<&ElementId> {
        match &self.target {
            Target::Shared(id) => Some(id),
            _ => None,
        }
    }

    /// `self` against `view`, a newer sketch: a point it shares where the sketch now puts
    /// it; and what the sketch no longer has given up, a point shared becoming a new one at
    /// its last place, and a relation to a curve or a point gone dropped.
    pub(super) fn revalidated(&self, view: &View<'_>) -> Placed {
        let mut placed = self.clone();
        placed.target = match &self.target {
            Target::Shared(id) => match view.point(id) {
                Some(at) => {
                    placed.at = at;
                    Target::Shared(id.clone())
                }
                None => Target::Free,
            },
            Target::Midpoint(line, ons) if view.has_curve(line) => Target::Midpoint(
                line.clone(),
                ons.iter().filter(|r| view.has(r)).cloned().collect(),
            ),
            Target::On(curve) if view.has(curve) => Target::On(curve.clone()),
            Target::Crossing(a, b) if view.has(a) && view.has(b) => {
                Target::Crossing(a.clone(), b.clone())
            }
            Target::Origin => Target::Origin,
            _ => Target::Free,
        };
        let present = |id: &Option<ElementId>| id.clone().filter(|id| view.point(id).is_some());
        placed.level_with = present(&self.level_with);
        placed.above = present(&self.above);
        placed
    }

    /// The relations a new point placed so keeps, `point` being its ID.
    pub(super) fn relations(&self, point: &ElementId) -> Vec<Form> {
        let p = || Reference::Element(point.clone());
        let on = |curve: &Reference| Form::On {
            point: p(),
            curve: curve.clone(),
        };
        let mut forms = match &self.target {
            Target::Shared(_) | Target::Free => Vec::new(),
            Target::Origin => at_origin(p()).into(),
            Target::Midpoint(line, ons) => {
                let mut forms = vec![Form::Midpoint {
                    point: p(),
                    line: Reference::Element(line.clone()),
                }];
                forms.extend(ons.iter().map(on));
                forms
            }
            Target::On(curve) => vec![on(curve)],
            Target::Crossing(a, b) => vec![on(a), on(b)],
        };
        if let Some(q) = &self.level_with {
            forms.push(Form::HorizontalPoints {
                a: p(),
                b: Reference::Element(q.clone()),
            });
        }
        if let Some(q) = &self.above {
            forms.push(Form::VerticalPoints {
                a: p(),
                b: Reference::Element(q.clone()),
            });
        }
        forms
    }

    /// What the pointer shows of it, drawn at it.
    pub(super) fn shown(&self) -> Vec<Shown> {
        let at = self.at;
        let mut shown: Vec<Relation> = match &self.target {
            Target::Free => Vec::new(),
            Target::Shared(id) => vec![Relation::Shared(id.clone())],
            Target::Origin => vec![Relation::Origin],
            Target::Midpoint(line, ons) => {
                let mut shown = vec![Relation::Midpoint(line.clone())];
                shown.extend(ons.iter().cloned().map(Relation::On));
                shown
            }
            Target::On(curve) => vec![Relation::On(curve.clone())],
            Target::Crossing(a, b) => vec![Relation::On(a.clone()), Relation::On(b.clone())],
        };
        shown.extend(
            self.level_with
                .iter()
                .cloned()
                .map(Relation::HorizontalPoints),
        );
        shown.extend(self.above.iter().cloned().map(Relation::VerticalPoints));
        shown
            .into_iter()
            .map(|relation| Shown { at, relation })
            .collect()
    }
}

/// What lining up with another point does to a point placed on nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Align {
    /// Moved onto both lines, and both relations the pointer's (#278's E).
    Relations,
    /// Moved onto both lines, a guide only: the entity's own relations show instead (a
    /// rectangle's sides, the 2025 help's horizontal line).
    Guides,
    /// Moved onto the vertical line through the point (x) only, a guide only: a horizontal
    /// line, which keeps its relation.
    GuideX,
    /// Moved onto the horizontal line (y) only, a guide only: a vertical line.
    GuideY,
    /// Not lined up.
    None,
}

/// The pointer over a sketch: what a click there would place.
pub(super) struct Pointer<'v, 's> {
    pub(super) view: &'v View<'s>,
    pub(super) tolerance: f64,
    /// Whether the pointer infers relations: *Automatic relations* on and Ctrl not held.
    pub(super) infer: bool,
    /// The points the pointer has woken, the newest first: the only ones a point lines up
    /// with.
    pub(super) woken: &'v [ElementId],
    /// The view's classes, built once for it ([`ViewClasses`]), its points fixed as the
    /// newest read's member states mark them: which of the relations it infers the sketch
    /// already implies ([`Implied`]).
    pub(super) classes: &'v ViewClasses,
    /// Which of the axes it may take ([`super::Axes`]): an axis it doesn't allow is no curve
    /// near it, so nothing is placed on it, nor where it crosses a curve, nor given an `on`
    /// of it beside a line's middle.
    pub(super) axes: super::Axes,
}

/// What a click at `at` picks of `view`, within `tolerance` millimetres, the axes `axes`
/// allows among what it may pick ([`super::Axes`]): Smart Dimension's picks and sketch
/// mode's selection both, by [`Pointer::pick`]'s one rule. A tolerance that isn't a finite
/// number above 0 is taken as 0, as the tools take one; a place that isn't finite picks
/// nothing.
pub(super) fn pick_in(
    view: &View<'_>,
    tolerance: f64,
    at: Xy,
    axes: super::Axes,
) -> Option<Reference> {
    picking(view, tolerance, at, axes, |pointer| pointer.pick(at))
}

/// The curve of `view` a click at `at` is on, within `tolerance` millimetres, by
/// [`Pointer::pick`]'s rule for curves ([`Pointer::curves_near`]: a line or an arc where
/// it's drawn, a circle all round), with no point in its way and no axis a candidate: the
/// nearest, ties in ID order. Trim, split, and extend take a curve by it. The tolerance and
/// the place are taken as [`pick_in`] takes them.
pub(super) fn curve_in(view: &View<'_>, tolerance: f64, at: Xy) -> Option<ElementId> {
    let no_axes = super::Axes { x: false, y: false };
    picking(view, tolerance, at, no_axes, |pointer| {
        let near = pointer.curves_near(at);
        nearest(near.iter().filter_map(|n| match &n.reference {
            Reference::Element(id) => Some((n.off, id, id)),
            _ => None,
        }))
        .cloned()
    })
}

/// `pick` asked of a pointer on `view` that infers nothing, within `tolerance` (one that
/// isn't a finite number above 0 taken as 0), the axes `axes` allows; nothing for a place
/// `at` that isn't finite.
fn picking<T>(
    view: &View<'_>,
    tolerance: f64,
    at: Xy,
    axes: super::Axes,
    pick: impl FnOnce(&Pointer<'_, '_>) -> Option<T>,
) -> Option<T> {
    if !(at[0].is_finite() && at[1].is_finite()) {
        return None;
    }
    let tolerance = if tolerance.is_finite() && tolerance > 0.0 {
        tolerance
    } else {
        0.0
    };
    // A pick infers no relation, so it judges none: its pointer's classes are never built.
    let classes = ViewClasses::default();
    let pointer = Pointer {
        view,
        tolerance,
        infer: true,
        woken: &[],
        classes: &classes,
        axes,
    };
    pick(&pointer)
}

/// IDs for a gesture's point or line not named yet, none a point or curve of the view: what
/// its relations are written with to be judged ([`Implied::rel`]), each of its points a
/// class of its own until a relation joins it.
pub(super) struct Unnamed {
    pub(super) start: ElementId,
    pub(super) end: ElementId,
    pub(super) line: ElementId,
}

impl Unnamed {
    /// The first three IDs of `zz000000`, `zz000001`, ... that `view` doesn't hold as a point
    /// or a curve: none only if it holds all million (more than the gate takes).
    pub(super) fn of(view: &View<'_>) -> Option<Unnamed> {
        let mut free = Self::ids(view, 3)?.into_iter();
        Some(Unnamed {
            start: free.next()?,
            end: free.next()?,
            line: free.next()?,
        })
    }

    /// The first `n` IDs of `zz000000`, `zz000001`, ... that `view` doesn't hold as a point
    /// or a curve: none only if fewer than `n` of the million are free.
    pub(super) fn ids(view: &View<'_>, n: usize) -> Option<Vec<ElementId>> {
        let ids: Vec<ElementId> = (0u32..1_000_000)
            .filter_map(|k| ElementId::new(&format!("zz{k:06}")))
            .filter(|id| view.point(id).is_none() && !view.has_curve(id))
            .take(n)
            .collect();
        (ids.len() == n).then_some(ids)
    }
}

/// Where two curves near the pointer cross: how far from it, the place, whether one of the
/// two is an axis, and the pair, in [`Reference`]'s order.
struct Crossing {
    d: f64,
    at: Xy,
    axis: bool,
    pair: (Reference, Reference),
}

/// A curve near the pointer: the reference, its shape, where the pointer's nearest to it, and
/// how far that is.
struct Near {
    reference: Reference,
    geo: Geo,
    /// An axis, infinite.
    infinite: bool,
    foot: Xy,
    off: f64,
}

/// The nearest of `items` by distance (the first of equals): none if there are none.
pub(super) fn nearest_of<T>(items: impl Iterator<Item = (f64, T)>) -> Option<T> {
    items.min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, t)| t)
}

/// The nearest of `items` by distance, then by key: none if there are none.
fn nearest<K: Ord, T>(items: impl Iterator<Item = (f64, K, T)>) -> Option<T> {
    items
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
        .map(|(_, _, t)| t)
}

impl Pointer<'_, '_> {
    /// The view's classes, built the first time a relation is judged on it.
    pub(super) fn implied(&self) -> &Implied {
        self.classes.of()
    }

    /// The point of the sketch nearest `at`, within the tolerance.
    fn point_near(&self, at: Xy) -> Option<(f64, ElementId)> {
        nearest(self.view.points.iter().filter_map(|(id, &p)| {
            let d = distance(p, at);
            (d <= self.tolerance).then(|| (d, id.as_str().to_owned(), (d, id.clone())))
        }))
    }

    /// The point of the sketch or the origin a click at `at` takes, within the tolerance:
    /// the nearer of the sketch's nearest point and the origin, the point on a tie. A point
    /// at the origin's place (one place with it to the kernel, [`one_place`]) stands for
    /// the origin, as a line along an axis stands for the axis ([`covers`](Self::covers)):
    /// the origin is never taken over it, so which of the two is taken never turns on the
    /// last bits of where a solve put the point. None for neither within the tolerance.
    fn point_or_origin(&self, at: Xy) -> Option<Reference> {
        let point = self.point_near(at);
        let origin = distance(at, [0.0, 0.0]);
        // The points at the origin's place, the nearest the click, ties by ID.
        let stand_in = nearest(
            self.view
                .points
                .iter()
                .filter(|(_, p)| one_place(**p, [0.0, 0.0]))
                .map(|(id, &p)| (distance(p, at), id.as_str(), id)),
        );
        match (point, stand_in) {
            (Some((_, id)), Some(_)) => Some(Reference::Element(id)),
            (None, Some(id)) if origin <= self.tolerance => Some(Reference::Element(id.clone())),
            (Some((d, _)), None) if origin < d => Some(Reference::Origin),
            (Some((_, id)), None) => Some(Reference::Element(id)),
            (None, None) if origin <= self.tolerance => Some(Reference::Origin),
            (None, _) => None,
        }
    }

    /// Where a click at `at` puts a point: a point of the sketch it shares, whatever the
    /// pointer infers; then, if it infers, the origin, a line's middle, two curves' crossing,
    /// or a curve; or nothing.
    pub(super) fn snap(&self, at: Xy) -> Placed {
        self.snap_after(at, &[])
    }

    /// Where a click at `at` puts a gesture's later point, after the points it placed before
    /// it, `before`, in their order (a segment's start; an arc's start, or its centre and
    /// start; a rectangle's first point): as [`snap`](Self::snap), but their relations are
    /// merged into the classes first (a point of the sketch has none of its own), so the
    /// `on`s beside this point's `midpoint` are judged after them, as the add writes them
    /// (docs/ports/sketch-tools.md: the start point's relations, then the end point's, each
    /// kept merged before the next).
    pub(super) fn snap_after(&self, at: Xy, before: &[&Placed]) -> Placed {
        let point = self.point_near(at);
        if !self.infer {
            return match point {
                Some((_, id)) => self.shared(id, at),
                None => Placed::new(at, Target::Free),
            };
        }
        // The origin wins only over a point farther away: a point at the origin's place is
        // shared, not placed there again.
        match self.point_or_origin(at) {
            Some(Reference::Element(id)) => return self.shared(id, at),
            Some(_) => return Placed::new([0.0, 0.0], Target::Origin),
            None => {}
        }
        let near = self.curves_near(at);
        if let Some((mid, line)) = self.midpoint_near(at) {
            // Each other curve through the middle exactly, as where it crosses the line, but
            // one the sketch already holds the middle on (#278's G as amended).
            let mut ons: Vec<Reference> = near
                .iter()
                .filter(|n| n.reference != Reference::Element(line.clone()))
                .filter(|n| n.geo.off(mid) <= EXACT && (n.infinite || n.geo.spans(mid)))
                .map(|n| n.reference.clone())
                .collect();
            ons.sort();
            let ons = self.ons_not_implied(mid, &line, ons, before);
            return Placed::new(mid, Target::Midpoint(line, ons));
        }
        if let Some((cross, a, b)) = self.crossing_near(at, &near) {
            return Placed::new(cross, Target::Crossing(a, b));
        }
        if let Some(curve) = nearest(near.iter().map(|n| (n.off, &n.reference, n))) {
            return Placed::new(curve.foot, Target::On(curve.reference.clone()));
        }
        Placed::new(at, Target::Free)
    }

    /// Of `ons`, in [`Reference`]'s order, the `on`s of a new point at `mid`, the middle of
    /// `line`, the sketch doesn't already imply ([`Implied`], Rule E): the forms the add
    /// writes for it ([`Placed::relations`]), judged in their order, the `midpoint` first
    /// and asserted, and each `on` kept merged before the next. The relations of the
    /// gesture's points placed `before` it are merged before them, as
    /// [`snap_after`](Self::snap_after) says. An `on` of the x axis at the middle of a level
    /// line along it, or of a line level with it, goes.
    fn ons_not_implied(
        &self,
        mid: Xy,
        line: &ElementId,
        ons: Vec<Reference>,
        before: &[&Placed],
    ) -> Vec<Reference> {
        let Some(mut ids) = Unnamed::ids(self.view, before.len() + 1).filter(|_| !ons.is_empty())
        else {
            return ons;
        };
        // This point is the gesture's last so far, each before it one of its own, in order,
        // as a segment's are when its directions are judged (`line.rs`'s `not_implied`).
        let Some(point) = ids.pop() else {
            return ons;
        };
        let mut implied = self.implied().clone();
        for (earlier, id) in before.iter().zip(&ids) {
            for form in earlier.relations(id) {
                let rel = implied.rel(&form);
                implied.merge(&rel);
            }
        }
        let placed = Placed::new(mid, Target::Midpoint(line.clone(), ons));
        let mut kept = Vec::new();
        for form in placed.relations(&point) {
            let rel = implied.rel(&form);
            match form {
                Form::On { curve, .. } => {
                    if implied.admit(&rel) {
                        kept.push(curve);
                    }
                }
                _ => implied.merge(&rel),
            }
        }
        kept
    }

    /// What a click at `at` picks to dimension (Smart Dimension), or selects in sketch mode
    /// ([`pick_in`]): a point of the sketch within the tolerance, or the origin where it's
    /// nearer but no point stands at its place, as [`snap`](Self::snap) finds them
    /// ([`point_or_origin`](Self::point_or_origin)); else the curve nearest, within the
    /// tolerance, as `snap` finds curves (a line or an arc where it's drawn, a circle all
    /// round, an axis `axes` allows anywhere along it but where a line of the sketch lies
    /// along it), ties as `snap`'s (in [`Reference`]'s order); else nothing. An axis the
    /// pointer's `axes` doesn't allow is no candidate, so the nearest of the rest is taken.
    /// Whether relations are inferred plays no part: a pick is no relation.
    fn pick(&self, at: Xy) -> Option<Reference> {
        if let Some(found) = self.point_or_origin(at) {
            return Some(found);
        }
        let near = self.curves_near(at);
        nearest(near.iter().map(|n| (n.off, &n.reference, n))).map(|n| n.reference.clone())
    }

    /// Only a point of the sketch the click shares, else nothing: where a tool fixes the
    /// point's place itself (a square's corner, a tangent arc's start).
    pub(super) fn shared_only(&self, at: Xy) -> Placed {
        match self.point_near(at) {
            Some((_, id)) => self.shared(id, at),
            None => Placed::new(at, Target::Free),
        }
    }

    fn shared(&self, id: ElementId, at: Xy) -> Placed {
        let where_ = self.view.point(&id).unwrap_or(at);
        Placed::new(where_, Target::Shared(id))
    }

    /// The middle of a line nearest `at`, within the tolerance.
    fn midpoint_near(&self, at: Xy) -> Option<(Xy, ElementId)> {
        nearest(self.view.curves.iter().filter_map(|(id, geo)| {
            let Geo::Line { from, to } = geo else {
                return None;
            };
            let mid = scale(add(*from, *to), 0.5);
            let d = distance(mid, at);
            (d <= self.tolerance).then(|| (d, id.as_str().to_owned(), (mid, id.clone())))
        }))
    }

    /// The curves within the tolerance of `at`: lines as drawn, circles, arcs where they're
    /// drawn, and the axes the pointer's `axes` allows, infinite, but where a line of the
    /// sketch lies along one ([`covers`](Self::covers)).
    fn curves_near(&self, at: Xy) -> Vec<Near> {
        let mut near = Vec::new();
        let tolerance = self.tolerance;
        let axes = self.axes;
        for (allowed, reference, foot) in [
            (axes.x, Reference::XAxis, [at[0], 0.0]),
            (axes.y, Reference::YAxis, [0.0, at[1]]),
        ] {
            let off = distance(foot, at);
            if allowed
                && off <= tolerance
                && let Some(geo) = self.view.geo(&reference)
                && !self.covers(&geo, foot, at)
            {
                near.push(Near {
                    reference,
                    geo,
                    infinite: true,
                    foot,
                    off,
                });
            }
        }
        for (id, geo) in &self.view.curves {
            let (foot, off) = match geo {
                Geo::Line { from, to } => to_segment(*from, *to, at),
                Geo::Circle { .. } | Geo::Arc { .. } => {
                    let foot = geo.project(at);
                    if !geo.spans(foot) {
                        continue;
                    }
                    (foot, distance(foot, at))
                }
            };
            if off <= tolerance && off.is_finite() {
                near.push(Near {
                    reference: Reference::Element(id.clone()),
                    geo: geo.clone(),
                    infinite: false,
                    foot,
                    off,
                });
            }
        }
        near
    }

    /// Whether a line of the sketch within the tolerance of `at` lies along the axis `axis`
    /// ([`lies_along`], as [`crossings`] decides two lines overlap) and spans its place
    /// `foot`, the axis's nearest `at`: there the sketch's own line covers the axis, and the
    /// pointer finds the line alone, so what it takes never turns on two ways of working out
    /// one place agreeing to the last bit.
    fn covers(&self, axis: &Geo, foot: Xy, at: Xy) -> bool {
        let Geo::Line {
            from: origin,
            to: along,
        } = axis
        else {
            return false;
        };
        self.view.curves.values().any(|geo| match geo {
            Geo::Line { from, to } => {
                lies_along(*origin, *along, *from, *to)
                    && to_segment(*from, *to, foot).1 <= LENGTH_TOLERANCE
                    && to_segment(*from, *to, at).1 <= self.tolerance
            }
            Geo::Circle { .. } | Geo::Arc { .. } => false,
        })
    }

    /// Where two of the curves `near` cross, nearest `at` and within the tolerance of it, on
    /// both as drawn: of the [`CROSSING_CURVES`] nearest curves only. The nearest crossing
    /// is taken by its distance, then whether its pair includes an axis, then the pair in
    /// [`Reference`]'s order; then, of every crossing within [`LENGTH_TOLERANCE`] of its
    /// place (one place, as the kernel and [`crossings`] take it), the one whose pair has no
    /// axis, then the pair first in that order. So where curves of the sketch cross on an
    /// axis, which crosses them there too by other arithmetic, an ulp or so off, the point is
    /// on the sketch's own two. A sketch curve against the origin or an axis: the sketch
    /// curve, never by an ID; among the sketch's own curves: by ID, the document's own order.
    fn crossing_near(&self, at: Xy, near: &[Near]) -> Option<(Xy, Reference, Reference)> {
        let mut nearest_first: Vec<&Near> = near.iter().collect();
        nearest_first.sort_by(|a, b| {
            a.off
                .total_cmp(&b.off)
                .then_with(|| a.reference.cmp(&b.reference))
        });
        nearest_first.truncate(CROSSING_CURVES);
        // Each pair in the references' order (the sketch's curves by ID, then the x axis,
        // then the y axis), so a crossing's two `on`s come out the same however near each was.
        nearest_first.sort_by(|a, b| a.reference.cmp(&b.reference));
        let mut found = Vec::new();
        for (i, a) in nearest_first.iter().enumerate() {
            for b in nearest_first.iter().skip(i + 1) {
                #[cfg(test)]
                work::pair();
                for x in crossings(&a.geo, &b.geo) {
                    let d = distance(x, at);
                    let on = |n: &Near| n.infinite || n.geo.spans(x);
                    if d <= self.tolerance && on(a) && on(b) {
                        found.push(Crossing {
                            d,
                            at: x,
                            axis: a.infinite || b.infinite,
                            pair: (a.reference.clone(), b.reference.clone()),
                        });
                    }
                }
            }
        }
        let rank = |c: &Crossing| (c.axis, c.pair.clone());
        let first = found
            .iter()
            .min_by(|a, b| a.d.total_cmp(&b.d).then_with(|| rank(a).cmp(&rank(b))))?;
        let taken = found
            .iter()
            .filter(|c| distance(c.at, first.at) <= LENGTH_TOLERANCE)
            .min_by_key(|c| rank(c))?;
        Some((taken.at, taken.pair.0.clone(), taken.pair.1.clone()))
    }

    /// `placed`, placed on nothing, lined up with the woken points within the tolerance of
    /// its x (one above or below it) and its y (one level with it), `align` saying how; the
    /// points `besides` (the entity's own) left out.
    pub(super) fn align(&self, placed: &mut Placed, align: Align, besides: &[&ElementId]) {
        if !self.infer || placed.target != Target::Free || align == Align::None {
            return;
        }
        let at = placed.at;
        let candidates = || {
            self.woken
                .iter()
                .filter(|id| !besides.contains(id))
                .filter_map(|id| Some((id, self.view.point(id)?)))
        };
        let (want_x, want_y) = match align {
            Align::GuideX => (true, false),
            Align::GuideY => (false, true),
            _ => (true, true),
        };
        let above = want_x
            .then(|| {
                nearest(candidates().filter_map(|(id, p)| {
                    let d = (p[0] - at[0]).abs();
                    (d <= self.tolerance).then(|| (d, id.as_str().to_owned(), (id.clone(), p)))
                }))
            })
            .flatten();
        let level = want_y
            .then(|| {
                nearest(candidates().filter_map(|(id, p)| {
                    let d = (p[1] - at[1]).abs();
                    (d <= self.tolerance).then(|| (d, id.as_str().to_owned(), (id.clone(), p)))
                }))
            })
            .flatten();
        if let Some((_, p)) = &above {
            placed.at[0] = p[0];
        }
        if let Some((_, p)) = &level {
            placed.at[1] = p[1];
        }
        for (_, p) in above.iter().chain(level.iter()) {
            placed.guides.push([*p, placed.at]);
        }
        if align == Align::Relations {
            placed.above = above.map(|(id, _)| id);
            placed.level_with = level.map(|(id, _)| id);
        }
    }
}

/// A line's own direction the pointer shows, from its start.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Along {
    Horizontal,
    Vertical,
    /// Leaving the arc that ends at its start smoothly: `tangent_at`, `same`.
    Tangent(ElementId),
    Perpendicular(ElementId),
    Parallel(ElementId),
}

impl Along {
    /// The relation, of the line `line` from the point `start`.
    pub(super) fn form(&self, line: &ElementId, start: &ElementId) -> Form {
        let l = || Reference::Element(line.clone());
        let r = |id: &ElementId| Reference::Element(id.clone());
        match self {
            Along::Horizontal => Form::Horizontal { line: l() },
            Along::Vertical => Form::Vertical { line: l() },
            Along::Tangent(arc) => Form::TangentAt {
                a: r(arc),
                b: l(),
                at: r(start),
                direction: Direction::Same,
            },
            Along::Perpendicular(other) => Form::Perpendicular {
                a: r(other),
                b: l(),
            },
            Along::Parallel(other) => Form::Parallel {
                a: r(other),
                b: l(),
            },
        }
    }

    pub(super) fn relation(&self) -> Relation {
        match self {
            Along::Horizontal => Relation::Horizontal,
            Along::Vertical => Relation::Vertical,
            Along::Tangent(arc) => Relation::Tangent(arc.clone()),
            Along::Perpendicular(other) => Relation::Perpendicular(other.clone()),
            Along::Parallel(other) => Relation::Parallel(other.clone()),
        }
    }
}

/// The directions a line from `start` (the point `start_id`, if the sketch holds it) to
/// `end` shows, and the exact direction that holds them, within
/// [`ANGLE_TOLERANCE_DEGREES`]: tangent to an arc ending at its start, with horizontal or
/// vertical too only where the arc leaves along that axis to within [`EXACT`] as an angle
/// (its direction's other component at most 1e-9, about 1e-9 rad; the coordinator's rule,
/// #335 round 2: the tangent wins); else horizontal; else vertical; else perpendicular to a
/// line ending there; else parallel to one. The nearest of each kind, ties by ID.
pub(super) fn along(
    view: &View<'_>,
    start: Xy,
    start_id: Option<&ElementId>,
    end: Xy,
) -> Option<(Vec<Along>, Xy)> {
    let d = sub(end, start);
    unit(d)?;
    let tolerance = angle_tolerance();
    // Each direction is the line's through `start`, either way along it: the end is moved
    // onto it ([`onto`]), which its sign doesn't change.
    let axis = if libm::atan2(d[1].abs(), d[0].abs()) <= tolerance {
        Some((Along::Horizontal, [1.0, 0.0]))
    } else if libm::atan2(d[0].abs(), d[1].abs()) <= tolerance {
        Some((Along::Vertical, [0.0, 1.0]))
    } else {
        None
    };
    let ends = start_id.map(|id| view.ends_at(id)).unwrap_or_default();
    let tangent = nearest(
        ends.iter()
            .filter(|(_, _, of)| *of == EndOf::Arc)
            .filter_map(|(arc, out, _)| {
                let off = angle_between(d, *out);
                (off <= tolerance).then(|| (off, arc.as_str().to_owned(), (arc.clone(), *out)))
            }),
    );
    if let Some((arc, out)) = tangent {
        // The tangent wins. The axis is added too only where the arc leaves along it
        // exactly (an obround's join), so both hold where the line is written; otherwise
        // the line runs along the arc, tangent alone.
        let on_axis = |a: &Along| match a {
            Along::Horizontal => out[1].abs() <= EXACT,
            Along::Vertical => out[0].abs() <= EXACT,
            _ => false,
        };
        let mut shows = vec![Along::Tangent(arc)];
        shows.extend(axis.map(|(a, _)| a).filter(on_axis));
        return Some((shows, out));
    }
    if let Some((a, direction)) = axis {
        return Some((vec![a], direction));
    }
    let lines: Vec<(&ElementId, Xy)> = ends
        .iter()
        .filter(|(_, _, of)| *of == EndOf::Line)
        .map(|(id, out, _)| (id, *out))
        .collect();
    let perpendicular = nearest(lines.iter().filter_map(|(id, out)| {
        let off = (angle_between(d, *out) - std::f64::consts::FRAC_PI_2).abs();
        (off <= tolerance).then(|| {
            (
                off,
                id.as_str().to_owned(),
                (Along::Perpendicular((*id).clone()), left(*out)),
            )
        })
    }));
    if let Some((a, direction)) = perpendicular {
        return Some((vec![a], direction));
    }
    nearest(lines.iter().filter_map(|(id, out)| {
        let a = angle_between(d, *out);
        let off = a.min(std::f64::consts::PI - a);
        (off <= tolerance).then(|| {
            (
                off,
                id.as_str().to_owned(),
                (Along::Parallel((*id).clone()), *out),
            )
        })
    }))
    .map(|(a, direction)| (vec![a], direction))
}

/// `end` moved onto the line through `start` along `direction` (a unit vector, either way
/// along the line: its sign changes nothing), its foot there.
pub(super) fn onto(start: Xy, direction: Xy, end: Xy) -> Xy {
    add(start, scale(direction, dot(sub(end, start), direction)))
}

/// Whether two places are one to the kernel: at most [`LENGTH_TOLERANCE`] apart.
pub(super) fn one_place(a: Xy, b: Xy) -> bool {
    distance(a, b) <= LENGTH_TOLERANCE
}

/// Which way an arc turns from `from`, leaving along `direction`, to reach `to`: its
/// centre, and whether it turns counter-clockwise. None where `to` lies straight ahead or
/// behind (no arc), or the centre isn't finite.
pub(super) fn tangent_arc(from: Xy, direction: Xy, to: Xy) -> Option<(Xy, bool)> {
    let d = sub(to, from);
    let n = left(direction);
    let across = dot(d, n);
    if across == 0.0 {
        return None;
    }
    // The centre is `s` along the normal: finite exactly when `s` is (the normal is a unit
    // vector, and `from` within a kilometre).
    let s = dot(d, d) / (2.0 * across);
    s.is_finite().then(|| (add(from, scale(n, s)), s > 0.0))
}

/// Which way the arc from `start` through `middle` to `end` turns: counter-clockwise from
/// `start` to `end` when `middle` is to the right of the chord from `start` to `end`.
pub(super) fn counter_clockwise(start: Xy, middle: Xy, end: Xy) -> bool {
    cross(sub(end, start), sub(middle, start)) < 0.0
}
