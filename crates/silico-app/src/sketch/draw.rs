//! A sketch drawn (docs/design/m5.0-app.md, section 9, *Colours by state*): what the newest
//! `sketch` read holds, as data first ([`drawing`]: each stroke, dot, glyph, and text, with
//! the member it's of and its colour by state), then turned into egui's shapes by one
//! function ([`to_shapes`]), so a restyle touches only that and `theme`'s values. The
//! projection from the sketch's millimetres to the screen is a closure, so nothing here
//! knows of a camera: the viewport gives its own, and a sketch whose face broke is drawn
//! flat ([`Flat`]).
//!
//! Where a relation's glyph or a dimension stands is worked out once, in the sketch's
//! millimetres, at the view's scale ([`placed`]), so hit-testing ([`super::hit`]) finds them
//! where they're drawn.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use eframe::egui::{self, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke};
use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, ElementKind, Form, Reference, SketchConstraint};
use silico_core::sketch_states::{ConstraintState, MemberState, SketchState};

use crate::theme::SketchStyle;

/// The most segments one curve's polyline has, however near the view: past it, a curve far
/// larger than the screen strays further than the chord tolerance, off the screen.
pub const MAX_CURVE_SEGMENTS: usize = 1024;

/// How many segments an angle dimension's arc has.
const ANGLE_ARC_SEGMENTS: usize = 24;

/// Millimetres per screen point at the sketch's plane: what turns the style's sizes, in
/// points, into the sketch's own lengths, so a glyph or a dimension stands as far from its
/// member at every zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale(pub f64);

impl Scale {
    /// `points` screen points, in millimetres.
    pub fn mm(self, points: f32) -> f64 {
        f64::from(points) * self.0
    }
}

/// What's shown: the palette's Show Dimensions and Show Constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub dimensions: bool,
    pub constraints: bool,
}

/// A relation's glyph (fusion.md, *Sketch mode*): one kind for each relation the document
/// has (a coincidence is a shared point, so none), and the redundant relation's own (open
/// item 28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GlyphKind {
    Horizontal,
    Vertical,
    On,
    Midpoint,
    Tangent,
    Perpendicular,
    Parallel,
    Equal,
    Symmetric,
    Redundant,
}

/// One thing drawn, in screen points, with the member it's of (a dimension's lines and text
/// are its constraint's).
#[derive(Debug, Clone, PartialEq)]
pub enum Drawn {
    /// A curve's polyline, or a dimension's line (`thin`).
    Stroke {
        of: ElementId,
        points: Vec<Pos2>,
        colour: Color32,
        dashed: bool,
        thin: bool,
    },
    Dot {
        of: ElementId,
        at: Pos2,
        colour: Color32,
    },
    Glyph {
        of: ElementId,
        kind: GlyphKind,
        at: Pos2,
        colour: Color32,
    },
    /// A dimension's value.
    Text {
        of: ElementId,
        at: Pos2,
        text: String,
        colour: Color32,
    },
}

impl Drawn {
    /// The member it's of.
    pub fn of(&self) -> &ElementId {
        match self {
            Drawn::Stroke { of, .. }
            | Drawn::Dot { of, .. }
            | Drawn::Glyph { of, .. }
            | Drawn::Text { of, .. } => of,
        }
    }

    /// Its colour.
    pub fn colour(&self) -> Color32 {
        match self {
            Drawn::Stroke { colour, .. }
            | Drawn::Dot { colour, .. }
            | Drawn::Glyph { colour, .. }
            | Drawn::Text { colour, .. } => *colour,
        }
    }

    /// Every screen position it holds.
    pub fn positions(&self) -> Vec<Pos2> {
        match self {
            Drawn::Stroke { points, .. } => points.clone(),
            Drawn::Dot { at, .. } | Drawn::Glyph { at, .. } | Drawn::Text { at, .. } => {
                vec![*at]
            }
        }
    }
}

/// A curve of the read, in the sketch's millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Curve {
    Line {
        from: [f64; 2],
        to: [f64; 2],
    },
    /// A circle, or an arc counter-clockwise from `start` (radians) through `sweep`, its
    /// radius the centre's distance to its start point (as the core takes an arc's).
    Round {
        centre: [f64; 2],
        radius: f64,
        start: f64,
        sweep: f64,
    },
}

/// The read's geometry, found by ID: each point's solved place, and each curve.
pub(crate) struct Geometry<'a> {
    points: BTreeMap<&'a ElementId, [f64; 2]>,
    curves: BTreeMap<&'a ElementId, (Curve, ElementKind)>,
    /// The middle of the points' bounds: what a dimension stands away from.
    pub(crate) middle: [f64; 2],
}

impl<'a> Geometry<'a> {
    pub(crate) fn of(read: &'a SketchRead) -> Self {
        let sketch = &read.members;
        let points: BTreeMap<&ElementId, [f64; 2]> = sketch
            .points
            .iter()
            .map(|p| (&p.id, [p.x.mm(), p.y.mm()]))
            .collect();
        let mut curves = BTreeMap::new();
        for line in &sketch.lines {
            if let (Some(&from), Some(&to)) = (points.get(&line.from), points.get(&line.to)) {
                curves.insert(&line.id, (Curve::Line { from, to }, ElementKind::Line));
            }
        }
        for circle in &sketch.circles {
            if let Some(&centre) = points.get(&circle.center) {
                let circle_curve = Curve::Round {
                    centre,
                    radius: circle.radius.mm(),
                    start: 0.0,
                    sweep: TAU,
                };
                curves.insert(&circle.id, (circle_curve, ElementKind::Circle));
            }
        }
        for arc in &sketch.arcs {
            if let (Some(&centre), Some(&start), Some(&end)) = (
                points.get(&arc.center),
                points.get(&arc.start),
                points.get(&arc.end),
            ) {
                let from = angle(sub(start, centre));
                let to = angle(sub(end, centre));
                let arc_curve = Curve::Round {
                    centre,
                    radius: length(sub(start, centre)),
                    start: from,
                    sweep: (to - from).rem_euclid(TAU),
                };
                curves.insert(&arc.id, (arc_curve, ElementKind::Arc));
            }
        }
        let middle = bounds(points.values().copied()).map_or([0.0, 0.0], |(lo, hi)| {
            [mid(lo[0], hi[0]), mid(lo[1], hi[1])]
        });
        Geometry {
            points,
            curves,
            middle,
        }
    }

    /// Where `reference` is, if it's a point: one of the sketch's, or the origin.
    pub(crate) fn point(&self, reference: &Reference) -> Option<[f64; 2]> {
        match reference {
            Reference::Origin => Some([0.0, 0.0]),
            Reference::Element(id) => self.points.get(id).copied(),
            Reference::XAxis | Reference::YAxis => None,
        }
    }

    /// The curve `reference` names: one of the sketch's, or an axis, as a line from the
    /// origin along it.
    pub(crate) fn curve(&self, reference: &Reference) -> Option<Curve> {
        match reference {
            Reference::XAxis => Some(Curve::Line {
                from: [0.0, 0.0],
                to: [1.0, 0.0],
            }),
            Reference::YAxis => Some(Curve::Line {
                from: [0.0, 0.0],
                to: [0.0, 1.0],
            }),
            Reference::Element(id) => self.curves.get(id).map(|(curve, _)| *curve),
            Reference::Origin => None,
        }
    }

    /// Each point, by ID.
    pub(crate) fn points(&self) -> impl Iterator<Item = (&'a ElementId, [f64; 2])> + '_ {
        self.points.iter().map(|(id, at)| (*id, *at))
    }

    /// Each curve, by ID, with its kind.
    pub(crate) fn curves(&self) -> impl Iterator<Item = (&'a ElementId, Curve, ElementKind)> + '_ {
        self.curves
            .iter()
            .map(|(id, (curve, kind))| (*id, *curve, *kind))
    }
}

/// A relation's glyph, placed in the sketch's millimetres.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlacedGlyph {
    pub(crate) of: ElementId,
    pub(crate) kind: GlyphKind,
    pub(crate) at: [f64; 2],
}

/// A dimension, placed in the sketch's millimetres: its leader and extension lines, and its
/// value's text and where it stands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlacedDimension {
    pub(crate) of: ElementId,
    pub(crate) lines: Vec<Vec<[f64; 2]>>,
    pub(crate) text: String,
    pub(crate) text_at: [f64; 2],
}

/// Where each relation's glyphs and each dimension stand.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Placed {
    pub(crate) glyphs: Vec<PlacedGlyph>,
    pub(crate) dimensions: Vec<PlacedDimension>,
}

/// Where each constraint of `read` is drawn, at `scale`, with `style`'s offsets: each
/// relation's glyph beside each member it names (stacked, in the constraints' order, where
/// a member has several), and each dimension, as SolidWorks draws one, from the geometry it
/// measures, its text at the middle of its line, offset outward from the sketch's middle.
pub(crate) fn placed(
    read: &SketchRead,
    geometry: &Geometry,
    scale: Scale,
    style: &SketchStyle,
) -> Placed {
    let mut placed = Placed::default();
    let mut stacked: BTreeMap<Reference, usize> = BTreeMap::new();
    let flagged = |id: &ElementId| {
        matches!(
            read.states.constraints.get(id),
            Some(ConstraintState::Redundant | ConstraintState::PartiallyRedundant)
        )
    };
    for constraint in &read.members.constraints {
        if constraint.form.is_dimension() {
            if let Some(dimension) = dimension(constraint, geometry, scale, style) {
                placed.dimensions.push(dimension);
            }
            continue;
        }
        let Some((kind, on)) = relation(&constraint.form) else {
            continue;
        };
        // A redundant relation's own glyph, its members untouched (open item 28).
        let kind = if flagged(&constraint.id) {
            GlyphKind::Redundant
        } else {
            kind
        };
        for member in on {
            let count = stacked.entry(member.clone()).or_insert(0);
            if let Some(at) = glyph_place(member, *count, geometry, scale, style) {
                placed.glyphs.push(PlacedGlyph {
                    of: constraint.id.clone(),
                    kind,
                    at,
                });
                *count += 1;
            }
        }
    }
    placed
}

/// A relation's glyph and the members it stands beside: each it names but its anchor (a
/// symmetry's `about`), or the point a tangency is at.
fn relation(form: &Form) -> Option<(GlyphKind, Vec<&Reference>)> {
    Some(match form {
        Form::Horizontal { line } => (GlyphKind::Horizontal, vec![line]),
        Form::Vertical { line } => (GlyphKind::Vertical, vec![line]),
        Form::HorizontalPoints { a, b } => (GlyphKind::Horizontal, vec![a, b]),
        Form::VerticalPoints { a, b } => (GlyphKind::Vertical, vec![a, b]),
        Form::On { point, .. } => (GlyphKind::On, vec![point]),
        Form::Parallel { a, b } => (GlyphKind::Parallel, vec![a, b]),
        Form::Perpendicular { a, b } => (GlyphKind::Perpendicular, vec![a, b]),
        Form::Equal { a, b } => (GlyphKind::Equal, vec![a, b]),
        Form::Tangent { a, b, .. } => (GlyphKind::Tangent, vec![a, b]),
        Form::TangentOn { at, .. } | Form::TangentAt { at, .. } => (GlyphKind::Tangent, vec![at]),
        Form::Symmetric { a, b, .. } => (GlyphKind::Symmetric, vec![a, b]),
        Form::Midpoint { point, .. } => (GlyphKind::Midpoint, vec![point]),
        Form::Length { .. }
        | Form::Distance { .. }
        | Form::DistanceX { .. }
        | Form::DistanceY { .. }
        | Form::PointLineDistance { .. }
        | Form::Angle { .. }
        | Form::Radius { .. }
        | Form::Diameter { .. } => return None,
    })
}

/// Where the `count`th glyph beside `member` stands: beside a point, up and to its right;
/// beside a line, off its middle on its left, along it; beside a circle or an arc, outside
/// it, at 45° or its middle. Each further one a step on. None for the origin and the axes,
/// which aren't drawn.
fn glyph_place(
    member: &Reference,
    count: usize,
    geometry: &Geometry,
    scale: Scale,
    style: &SketchStyle,
) -> Option<[f64; 2]> {
    let (offset, step) = (scale.mm(style.glyph_offset), scale.mm(style.glyph_step));
    let along = step * count as f64;
    let Reference::Element(_) = member else {
        return None;
    };
    if let Some(p) = geometry.point(member) {
        return Some([p[0] + offset + along, p[1] + offset]);
    }
    match geometry.curve(member)? {
        Curve::Line { from, to } => {
            let t = unit(sub(to, from)).unwrap_or([1.0, 0.0]);
            let n = [-t[1], t[0]];
            let m = [mid(from[0], to[0]), mid(from[1], to[1])];
            Some(add(add(m, mul(n, offset)), mul(t, along)))
        }
        Curve::Round {
            centre,
            radius,
            start,
            sweep,
        } => {
            let at = if sweep >= TAU {
                [FRAC_1_SQRT_2, FRAC_1_SQRT_2]
            } else {
                direction(start + sweep / 2.0)
            };
            Some(add(add(centre, mul(at, radius + offset)), [along, 0.0]))
        }
    }
}

/// A dimension placed, if the geometry it measures is there.
fn dimension(
    constraint: &SketchConstraint,
    geometry: &Geometry,
    scale: Scale,
    style: &SketchStyle,
) -> Option<PlacedDimension> {
    let offset = scale.mm(style.dimension_offset);
    let gap = scale.mm(style.dimension_text_gap);
    let driving = constraint.form.driving().unwrap_or(true);
    let text = |value: String| {
        if driving { value } else { format!("({value})") }
    };
    let away = |at: [f64; 2], n: [f64; 2]| {
        if dot(sub(at, geometry.middle), n) < 0.0 {
            mul(n, -1.0)
        } else {
            n
        }
    };
    let of = constraint.id.clone();
    // Two points apart, along the line between them: extension lines out from each, the
    // dimension line between their ends, the text beyond its middle.
    let aligned = |a: [f64; 2], b: [f64; 2], value: String| {
        let t = unit(sub(b, a)).unwrap_or([1.0, 0.0]);
        let m = [mid(a[0], b[0]), mid(a[1], b[1])];
        let n = away(m, [-t[1], t[0]]);
        let (a2, b2) = (add(a, mul(n, offset)), add(b, mul(n, offset)));
        PlacedDimension {
            of: of.clone(),
            lines: vec![vec![a, a2], vec![b, b2], vec![a2, b2]],
            text: text(value),
            text_at: add(m, mul(n, offset + gap)),
        }
    };
    Some(match &constraint.form {
        Form::Length { line, value, .. } => {
            let Curve::Line { from, to } = geometry.curve(line)? else {
                return None;
            };
            aligned(from, to, value.to_string())
        }
        Form::Distance { a, b, value, .. } => {
            aligned(geometry.point(a)?, geometry.point(b)?, value.to_string())
        }
        Form::DistanceX { a, b, value, .. } | Form::DistanceY { a, b, value, .. } => {
            let (pa, pb) = (geometry.point(a)?, geometry.point(b)?);
            // The axis it measures along, and the one it stands off along.
            let (along, off) = match constraint.form {
                Form::DistanceX { .. } => (0, 1),
                _ => (1, 0),
            };
            let m = [mid(pa[0], pb[0]), mid(pa[1], pb[1])];
            let outward = m[off] >= geometry.middle[off];
            let level = if outward {
                pa[off].max(pb[off]) + offset
            } else {
                pa[off].min(pb[off]) - offset
            };
            let sign = if outward { 1.0 } else { -1.0 };
            let at = |p: [f64; 2], level: f64| {
                let mut q = [0.0; 2];
                q[along] = p[along];
                q[off] = level;
                q
            };
            let (a2, b2) = (at(pa, level), at(pb, level));
            PlacedDimension {
                of,
                lines: vec![vec![pa, a2], vec![pb, b2], vec![a2, b2]],
                text: text(value.to_string()),
                text_at: at(m, level + sign * gap),
            }
        }
        Form::PointLineDistance {
            point, line, value, ..
        } => {
            let p = geometry.point(point)?;
            let Curve::Line { from, to } = geometry.curve(line)? else {
                return None;
            };
            let foot = foot(p, from, to);
            let t = unit(sub(p, foot)).unwrap_or([0.0, 1.0]);
            let m = [mid(p[0], foot[0]), mid(p[1], foot[1])];
            PlacedDimension {
                of,
                lines: vec![vec![p, foot]],
                text: text(value.to_string()),
                text_at: add(m, mul([-t[1], t[0]], gap)),
            }
        }
        Form::Radius { curve, value, .. } | Form::Diameter { curve, value, .. } => {
            let Curve::Round {
                centre,
                radius,
                start,
                sweep,
            } = geometry.curve(curve)?
            else {
                return None;
            };
            // Out through an arc's middle; for a circle, away from the sketch's middle,
            // or at 45° from a circle about it.
            let u = if sweep < TAU {
                direction(start + sweep / 2.0)
            } else {
                unit(sub(centre, geometry.middle)).unwrap_or([FRAC_1_SQRT_2, FRAC_1_SQRT_2])
            };
            let rim = add(centre, mul(u, radius));
            let diameter = matches!(constraint.form, Form::Diameter { .. });
            let (from, word) = if diameter {
                (sub(centre, mul(u, radius)), "Ø")
            } else {
                (centre, "R")
            };
            PlacedDimension {
                of,
                lines: vec![vec![from, rim]],
                text: text(format!("{word}{value}")),
                text_at: add(centre, mul(u, radius + gap)),
            }
        }
        Form::Angle {
            a,
            b,
            reversed,
            value,
            ..
        } => {
            let (Curve::Line { from: a0, to: a1 }, Curve::Line { from: b0, to: b1 }) =
                (geometry.curve(a)?, geometry.curve(b)?)
            else {
                return None;
            };
            let da = sub(a1, a0);
            let db = if *reversed { sub(b0, b1) } else { sub(b1, b0) };
            let corner = crossing(a0, da, b0, db).unwrap_or(a0);
            let from = angle(da);
            let sweep = value.degrees().to_radians();
            let arc: Vec<[f64; 2]> = (0..=ANGLE_ARC_SEGMENTS)
                .map(|k| {
                    let at = from + sweep * k as f64 / ANGLE_ARC_SEGMENTS as f64;
                    add(corner, mul(direction(at), offset))
                })
                .collect();
            PlacedDimension {
                of,
                lines: vec![arc],
                text: text(format!("{value}°")),
                text_at: add(corner, mul(direction(from + sweep / 2.0), offset + gap)),
            }
        }
        _ => return None,
    })
}

/// The colour of a point or a curve in state `state`, of a sketch in state `sketch`: a
/// sketch not solved to buildable geometry colours every member its own way (§2: invalid
/// yellow, unsolvable red); otherwise each member's own state.
pub fn member_colour(
    style: &SketchStyle,
    sketch: SketchState,
    state: Option<MemberState>,
    selected: bool,
) -> Color32 {
    if selected {
        return style.selected;
    }
    match (sketch, state) {
        (SketchState::NoSolution, _) => style.no_solution,
        (SketchState::Invalid, _) => style.invalid,
        (_, Some(MemberState::FullyDefined)) => style.fully_defined,
        (_, Some(MemberState::OverDefined)) => style.over_defined,
        (_, Some(MemberState::UnderDefined) | None) => style.under_defined,
    }
}

/// The colour of a constraint of form `form` in state `state`, of a sketch in state
/// `sketch`: one in conflict, or a dimension repeating others, over defined; a relation
/// repeating others its own glyph's colour; a driven dimension gray; one unsolved, on a
/// sketch with no buildable solution, that sketch's colour; a driving dimension as fully
/// defined geometry, and a relation in the glyphs' colour.
pub fn constraint_colour(
    style: &SketchStyle,
    sketch: SketchState,
    form: &Form,
    state: Option<ConstraintState>,
    selected: bool,
) -> Color32 {
    if selected {
        return style.selected;
    }
    let dimension = form.is_dimension();
    match state {
        Some(ConstraintState::Conflicting) => style.over_defined,
        Some(ConstraintState::Redundant | ConstraintState::PartiallyRedundant) if dimension => {
            style.over_defined
        }
        Some(ConstraintState::Redundant | ConstraintState::PartiallyRedundant) => style.redundant,
        Some(ConstraintState::Unsolved) if sketch == SketchState::NoSolution => style.no_solution,
        Some(ConstraintState::Unsolved) if sketch == SketchState::Invalid => style.invalid,
        _ if form.driving() == Some(false) => style.driven,
        _ if dimension => style.fully_defined,
        _ => style.glyph,
    }
}

/// What's drawn of `read`, projected by `project` (the sketch's millimetres to screen
/// points; none for a place it can't show), at `scale`, with `style`, the members in
/// `selected` in the selection's colour: each curve a polyline (an arc or a circle within
/// the style's chord tolerance of itself), construction dashed; each point a dot; and, as
/// `shown` says, each relation's glyphs and each dimension.
pub fn drawing(
    read: &SketchRead,
    project: &dyn Fn([f64; 2]) -> Option<Pos2>,
    scale: Scale,
    style: &SketchStyle,
    shown: Shown,
    selected: &BTreeSet<ElementId>,
) -> Vec<Drawn> {
    let project = |at: [f64; 2]| project(at).filter(|p| p.x.is_finite() && p.y.is_finite());
    let geometry = Geometry::of(read);
    let sketch = read.states.state;
    let states = &read.states;
    let sketch_members = &read.members;
    let mut drawn = Vec::new();
    let construction: BTreeMap<&ElementId, bool> = sketch_members
        .lines
        .iter()
        .map(|l| (&l.id, l.construction))
        .chain(
            sketch_members
                .circles
                .iter()
                .map(|c| (&c.id, c.construction)),
        )
        .chain(sketch_members.arcs.iter().map(|a| (&a.id, a.construction)))
        .collect();
    for (id, curve, _) in geometry.curves() {
        let state = states
            .lines
            .get(id)
            .or_else(|| states.circles.get(id))
            .or_else(|| states.arcs.get(id))
            .copied();
        let colour = member_colour(style, sketch, state, selected.contains(id));
        let dashed = construction.get(id).copied().unwrap_or(false);
        let points = match curve {
            Curve::Line { from, to } => vec![project(from), project(to)],
            Curve::Round {
                centre,
                radius,
                start,
                sweep,
            } => round_points(
                centre,
                radius,
                start,
                sweep,
                &project,
                style.chord_tolerance,
            ),
        };
        for run in runs(points) {
            drawn.push(Drawn::Stroke {
                of: id.clone(),
                points: run,
                colour,
                dashed,
                thin: false,
            });
        }
    }
    for (id, at) in geometry.points() {
        let colour = member_colour(
            style,
            sketch,
            states.points.get(id).copied(),
            selected.contains(id),
        );
        if let Some(at) = project(at) {
            drawn.push(Drawn::Dot {
                of: id.clone(),
                at,
                colour,
            });
        }
    }
    let placed = placed(read, &geometry, scale, style);
    let forms: BTreeMap<&ElementId, &Form> = sketch_members
        .constraints
        .iter()
        .map(|c| (&c.id, &c.form))
        .collect();
    let colour_of = |id: &ElementId| {
        forms.get(id).map_or(style.glyph, |form| {
            constraint_colour(
                style,
                sketch,
                form,
                states.constraints.get(id).copied(),
                selected.contains(id),
            )
        })
    };
    if shown.constraints {
        for glyph in &placed.glyphs {
            if let Some(at) = project(glyph.at) {
                let colour = match glyph.kind {
                    GlyphKind::Redundant if !selected.contains(&glyph.of) => style.redundant,
                    _ => colour_of(&glyph.of),
                };
                drawn.push(Drawn::Glyph {
                    of: glyph.of.clone(),
                    kind: glyph.kind,
                    at,
                    colour,
                });
            }
        }
    }
    if shown.dimensions {
        for dimension in &placed.dimensions {
            let colour = colour_of(&dimension.of);
            for line in &dimension.lines {
                for run in runs(line.iter().map(|&p| project(p)).collect()) {
                    drawn.push(Drawn::Stroke {
                        of: dimension.of.clone(),
                        points: run,
                        colour,
                        dashed: false,
                        thin: true,
                    });
                }
            }
            if let Some(at) = project(dimension.text_at) {
                drawn.push(Drawn::Text {
                    of: dimension.of.clone(),
                    at,
                    text: dimension.text.clone(),
                    colour,
                });
            }
        }
    }
    drawn
}

/// The polyline of a circle or an arc about `centre`, of `radius`, from the angle `start`
/// through `sweep`, its vertices on the curve and its chords within `tolerance` screen
/// points of it: as many segments as that needs at the projection's largest stretch about
/// the centre, at most [`MAX_CURVE_SEGMENTS`].
pub(crate) fn round_points(
    centre: [f64; 2],
    radius: f64,
    start: f64,
    sweep: f64,
    project: &dyn Fn([f64; 2]) -> Option<Pos2>,
    tolerance: f32,
) -> Vec<Option<Pos2>> {
    let segments = segments(stretch(centre, radius, project) * radius, sweep, tolerance);
    let mut points: Vec<Option<Pos2>> = (0..=segments)
        .map(|k| {
            let at = start + sweep * k as f64 / segments as f64;
            project(add(centre, mul(direction(at), radius)))
        })
        .collect();
    // A whole turn ends exactly where it starts, not a rounding away.
    if sweep >= TAU
        && let Some(&first) = points.first()
        && let Some(last) = points.last_mut()
    {
        *last = first;
    }
    points
}

/// How many segments keep chords within `tolerance` of a curve of `radius` screen points
/// through `sweep` radians: each segment's angle θ at most 2·acos(1 − tolerance/radius),
/// where its chord's sagitta, radius·(1 − cos θ/2), is the tolerance. At least one;
/// [`MAX_CURVE_SEGMENTS`] where that's more, or the stretch is unknown.
pub(crate) fn segments(radius: f64, sweep: f64, tolerance: f32) -> usize {
    let tolerance = f64::from(tolerance);
    if !(radius.is_finite() && sweep.is_finite()) || tolerance <= 0.0 || radius < 0.0 {
        return MAX_CURVE_SEGMENTS;
    }
    if radius <= tolerance || sweep <= 0.0 {
        return 1;
    }
    let step = 2.0 * libm::acos(1.0 - tolerance / radius);
    if step <= 0.0 || !step.is_finite() {
        return MAX_CURVE_SEGMENTS;
    }
    let needed = (sweep / step).ceil();
    if needed >= MAX_CURVE_SEGMENTS as f64 {
        MAX_CURVE_SEGMENTS
    } else {
        (needed as usize).max(1)
    }
}

/// The projection's largest stretch about `centre`, in screen points per millimetre, across
/// `radius`: the larger singular value of its 2×2 linear part there, so a circle's chords,
/// which an affine map can stretch at most that much, stay within the tolerance; infinite
/// where it can't be worked out (the centre or the rim unprojected).
fn stretch(centre: [f64; 2], radius: f64, project: &dyn Fn([f64; 2]) -> Option<Pos2>) -> f64 {
    let at = |p: [f64; 2]| project(p).map(|p| [f64::from(p.x), f64::from(p.y)]);
    let (Some(c), Some(x), Some(y)) = (
        at(centre),
        at(add(centre, [radius, 0.0])),
        at(add(centre, [0.0, radius])),
    ) else {
        return f64::INFINITY;
    };
    if radius <= 0.0 {
        return 0.0;
    }
    let (a, c2) = ((x[0] - c[0]) / radius, (x[1] - c[1]) / radius);
    let (b, d) = ((y[0] - c[0]) / radius, (y[1] - c[1]) / radius);
    let sum = a * a + b * b + c2 * c2 + d * d;
    let det = a * d - b * c2;
    let root = (sum * sum - 4.0 * det * det).max(0.0).sqrt();
    ((sum + root) / 2.0).sqrt()
}

/// `points` split where a place couldn't be projected: each run of two or more.
fn runs(points: Vec<Option<Pos2>>) -> Vec<Vec<Pos2>> {
    let mut runs = Vec::new();
    let mut run = Vec::new();
    for point in points {
        match point {
            Some(p) => run.push(p),
            None => {
                if run.len() >= 2 {
                    runs.push(std::mem::take(&mut run));
                } else {
                    run.clear();
                }
            }
        }
    }
    if run.len() >= 2 {
        runs.push(run);
    }
    runs
}

/// The one function that turns what's drawn into egui's shapes, with `style`'s sizes: a
/// restyle of sketch mode is this, and `theme`'s values.
pub fn to_shapes(painter: &Painter, drawn: &[Drawn], style: &SketchStyle) -> Vec<Shape> {
    let mut shapes = Vec::new();
    for item in drawn {
        match item {
            Drawn::Stroke {
                points,
                colour,
                dashed,
                thin,
                ..
            } => {
                let width = if *thin {
                    style.dimension_line_width
                } else {
                    style.line_width
                };
                let stroke = Stroke::new(width, *colour);
                if *dashed {
                    shapes.extend(Shape::dashed_line(points, stroke, style.dash, style.gap));
                } else {
                    shapes.push(Shape::line(points.clone(), stroke));
                }
            }
            Drawn::Dot { at, colour, .. } => {
                shapes.push(Shape::circle_filled(*at, style.point_radius, *colour));
            }
            Drawn::Glyph {
                kind, at, colour, ..
            } => shapes.extend(glyph(painter, *kind, *at, *colour, style)),
            Drawn::Text {
                at, text, colour, ..
            } => shapes.push(label(
                painter,
                *at,
                text,
                *colour,
                style.dimension_text_size,
            )),
        }
    }
    shapes
}

/// A glyph of `kind` at `at`, in `colour`: one function for each kind, so each can be drawn
/// its own way. Plain text labels for now.
fn glyph(
    painter: &Painter,
    kind: GlyphKind,
    at: Pos2,
    colour: Color32,
    style: &SketchStyle,
) -> Vec<Shape> {
    let size = style.glyph_size;
    match kind {
        GlyphKind::Horizontal => horizontal_glyph(painter, at, colour, size),
        GlyphKind::Vertical => vertical_glyph(painter, at, colour, size),
        GlyphKind::On => on_glyph(painter, at, colour, size),
        GlyphKind::Midpoint => midpoint_glyph(painter, at, colour, size),
        GlyphKind::Tangent => tangent_glyph(painter, at, colour, size),
        GlyphKind::Perpendicular => perpendicular_glyph(painter, at, colour, size),
        GlyphKind::Parallel => parallel_glyph(painter, at, colour, size),
        GlyphKind::Equal => equal_glyph(painter, at, colour, size),
        GlyphKind::Symmetric => symmetric_glyph(painter, at, colour, size),
        GlyphKind::Redundant => redundant_glyph(painter, at, colour, size),
    }
}

fn horizontal_glyph(painter: &Painter, at: Pos2, colour: Color32, size: f32) -> Vec<Shape> {
    vec![label(painter, at, "H", colour, size)]
}

fn vertical_glyph(painter: &Painter, at: Pos2, colour: Color32, size: f32) -> Vec<Shape> {
    vec![label(painter, at, "V", colour, size)]
}

fn on_glyph(painter: &Painter, at: Pos2, colour: Color32, size: f32) -> Vec<Shape> {
    vec![label(painter, at, "On", colour, size)]
}

fn midpoint_glyph(painter: &Painter, at: Pos2, colour: Color32, size: f32) -> Vec<Shape> {
    vec![label(painter, at, "Mid", colour, size)]
}

fn tangent_glyph(painter: &Painter, at: Pos2, colour: Color32, size: f32) -> Vec<Shape> {
    vec![label(painter, at, "Tan", colour, size)]
}

fn perpendicular_glyph(painter: &Painter, at: Pos2, colour: Color32, size: f32) -> Vec<Shape> {
    vec![label(painter, at, "Perp", colour, size)]
}

fn parallel_glyph(painter: &Painter, at: Pos2, colour: Color32, size: f32) -> Vec<Shape> {
    vec![label(painter, at, "Par", colour, size)]
}

fn equal_glyph(painter: &Painter, at: Pos2, colour: Color32, size: f32) -> Vec<Shape> {
    vec![label(painter, at, "=", colour, size)]
}

fn symmetric_glyph(painter: &Painter, at: Pos2, colour: Color32, size: f32) -> Vec<Shape> {
    vec![label(painter, at, "Sym", colour, size)]
}

fn redundant_glyph(painter: &Painter, at: Pos2, colour: Color32, size: f32) -> Vec<Shape> {
    vec![label(painter, at, "Red", colour, size)]
}

/// `text` centred at `at`, `size` points high, in `colour`.
fn label(painter: &Painter, at: Pos2, text: &str, colour: Color32, size: f32) -> Shape {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(size), colour);
    let corner = at - galley.size() / 2.0;
    Shape::galley(corner, galley, colour)
}

/// A sketch drawn flat in a rectangle of the screen, with no frame in 3D behind it: a
/// sketch whose face broke (#347). Its points' bounds (and the origin) fill `rect` but a
/// margin, uniformly, y up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flat {
    middle: [f64; 2],
    /// Screen points per millimetre.
    zoom: f64,
    centre: Pos2,
}

/// The margin a flat sketch leaves inside its rectangle, as a share of its smaller side.
const FLAT_MARGIN: f64 = 0.1;

impl Flat {
    /// `read`'s sketch fitted into `rect`.
    pub fn fit(rect: Rect, read: &SketchRead) -> Self {
        let points = read
            .members
            .points
            .iter()
            .map(|p| [p.x.mm(), p.y.mm()])
            .chain(std::iter::once([0.0, 0.0]));
        let (lo, hi) = bounds(points).unwrap_or(([0.0, 0.0], [0.0, 0.0]));
        let span = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        let side = f64::from(rect.width().min(rect.height())) * (1.0 - 2.0 * FLAT_MARGIN);
        let zoom = if span > 0.0 && side > 0.0 {
            side / span
        } else {
            1.0
        };
        Flat {
            middle: [mid(lo[0], hi[0]), mid(lo[1], hi[1])],
            zoom,
            centre: rect.center(),
        }
    }

    /// The sketch's place `at`, on the screen.
    pub fn project(&self, at: [f64; 2]) -> Option<Pos2> {
        let x = f64::from(self.centre.x) + (at[0] - self.middle[0]) * self.zoom;
        let y = f64::from(self.centre.y) - (at[1] - self.middle[1]) * self.zoom;
        Some(egui::pos2(x as f32, y as f32))
    }

    /// The screen's point `at`, in the sketch.
    pub fn to_plane(&self, at: Pos2) -> Option<[f64; 2]> {
        Some([
            self.middle[0] + (f64::from(at.x) - f64::from(self.centre.x)) / self.zoom,
            self.middle[1] - (f64::from(at.y) - f64::from(self.centre.y)) / self.zoom,
        ])
    }
}

/// The smallest box holding `points`: none for none.
fn bounds(points: impl Iterator<Item = [f64; 2]>) -> Option<([f64; 2], [f64; 2])> {
    points.fold(None, |found, p| {
        Some(match found {
            None => (p, p),
            Some((lo, hi)) => (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            ),
        })
    })
}

/// The point of the line through `from` and `to` nearest `p` (`from` if they're one).
pub(crate) fn foot(p: [f64; 2], from: [f64; 2], to: [f64; 2]) -> [f64; 2] {
    let d = sub(to, from);
    let dd = dot(d, d);
    if dd <= 0.0 {
        return from;
    }
    add(from, mul(d, dot(sub(p, from), d) / dd))
}

/// Where the line through `a` along `da` meets the one through `b` along `db`: none if
/// they're parallel.
fn crossing(a: [f64; 2], da: [f64; 2], b: [f64; 2], db: [f64; 2]) -> Option<[f64; 2]> {
    let cross = da[0] * db[1] - da[1] * db[0];
    if cross == 0.0 || !cross.is_finite() {
        return None;
    }
    let ab = sub(b, a);
    let t = (ab[0] * db[1] - ab[1] * db[0]) / cross;
    Some(add(a, mul(da, t)))
}

pub(crate) fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

pub(crate) fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn mul(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}

pub(crate) fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

pub(crate) fn length(a: [f64; 2]) -> f64 {
    libm::hypot(a[0], a[1])
}

fn mid(a: f64, b: f64) -> f64 {
    a + (b - a) / 2.0
}

/// `a` scaled to length 1: none for a zero or non-finite one.
fn unit(a: [f64; 2]) -> Option<[f64; 2]> {
    let l = length(a);
    (l > 0.0 && l.is_finite()).then(|| [a[0] / l, a[1] / l])
}

pub(crate) fn angle(a: [f64; 2]) -> f64 {
    libm::atan2(a[1], a[0])
}

pub(crate) fn direction(angle: f64) -> [f64; 2] {
    [libm::cos(angle), libm::sin(angle)]
}
