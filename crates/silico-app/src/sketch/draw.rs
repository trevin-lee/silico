//! A sketch drawn as the approved prototype draws it (docs/design/m5-ui/, `js/sketch.js`;
//! docs/design/m5.0-app.md, section 9, *Colours by state*): what the newest `sketch` read
//! holds, as data first ([`drawing`]: each stroke, fill, point, glyph, arrowhead, and label,
//! with the member it's of and its colour by state), and a tool's preview likewise
//! ([`preview`]); then turned into egui's shapes by one function ([`to_shapes`]), so a
//! restyle touches only that and `style`'s values. The projection from the sketch's
//! millimetres to the screen is a closure, so nothing here knows of a camera: the viewport
//! gives its own, and a sketch whose face broke is drawn flat ([`Flat`]).
//!
//! Where a relation's glyph or a dimension stands is worked out once, in the sketch's
//! millimetres, at the view's scale ([`placed`]), so hit-testing ([`super::hit`]) finds them
//! where they're drawn.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use eframe::egui::{self, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, ElementKind, Form, Reference};
use silico_core::kernel::SketchPlane;
use silico_core::sketch_states::{ConstraintState, MemberState, SketchState};
use silico_core::sketch_tools::{DimensionPreview, MeasureKind, Preview, Relation};

use super::style::*;

/// The most segments one curve's polyline has, however near the view: past it, a curve far
/// larger than the screen strays further than the chord tolerance, off the screen.
pub const MAX_CURVE_SEGMENTS: usize = 1024;

/// The least spacing of the grid's lines on the screen, in points: a step closer is left
/// out (the minor lines first, then the major).
const GRID_LEAST_SPACING: f32 = 8.0;

/// The most vertices a profile's region is filled for: past them (a sketch far beyond any
/// part's, where the tessellation would take more than a frame), its fill is left out, its
/// outline still drawn.
pub const MAX_FILL_VERTICES: usize = 200_000;

/// The fewest segments a lone circle has, in a fill and its outline: a ring still. An arc
/// has its share of them by its sweep, at least one.
pub const MIN_FILL_SEGMENTS: usize = 4;

/// The layout's own rules, the prototype's: a line's glyph a share of its way along it;
/// glyphs on one member this far apart; a glyph box's corner; a shown relation's glyph from
/// the pointer, and a measure's label from its segment's middle; the crosshair's arms and a
/// snap's ring; how far extension
/// lines run past a dimension's line; and a dimension's line's width and a grid line's, in
/// points.
const GLYPH_ALONG: f64 = 0.28;
const GLYPH_STEP: f32 = 18.0;
const GLYPH_RADIUS: u8 = 3;
const SHOWN_AT: Vec2 = Vec2::new(18.0, -16.0);
const MEASURE_LABEL_OFFSET: f32 = 12.0;
const CROSSHAIR_ARM: f32 = 8.0;
const SNAP_RADIUS: f32 = 6.0;
const EXTENSION_PAST: f32 = 5.0;
const DIMENSION_LINE_WIDTH: f32 = 1.0;
const GRID_LINE_WIDTH: f32 = 1.0;
/// A dimension's value's box's height, and a glyph's icon's stroke's width.
const LABEL_HEIGHT: f32 = 18.0;
const ICON_WIDTH: f32 = 1.3;

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

/// How the sketch is seen: the projection from its millimetres to screen points (none for
/// a place it can't show), the scale there, the part of its plane in view (its lower and
/// upper corners, for the grid; none for no grid), and how far a curve's polyline may stray
/// from it, in points.
pub struct View<'a> {
    pub project: &'a dyn Fn([f64; 2]) -> Option<Pos2>,
    pub scale: Scale,
    pub visible: Option<([f64; 2], [f64; 2])>,
    pub chord_tolerance: f32,
}

impl View<'_> {
    /// `at` projected, if it's a place on the screen.
    fn at(&self, at: [f64; 2]) -> Option<Pos2> {
        (self.project)(at).filter(|p| p.x.is_finite() && p.y.is_finite())
    }

    /// The screen's direction from `at` along the sketch's `direction`, as a unit vector.
    fn direction(&self, at: [f64; 2], direction: [f64; 2]) -> Option<Vec2> {
        let step = self.scale.0.max(f64::MIN_POSITIVE);
        let from = self.at(at)?;
        let to = self.at(add(at, mul(direction, step)))?;
        let d = to - from;
        (d.length() > 0.0 && d.length().is_finite()).then(|| d / d.length())
    }
}

/// What's shown: the palette's Show toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub profile: bool,
    pub points: bool,
    pub dimensions: bool,
    pub constraints: bool,
}

impl Default for Shown {
    fn default() -> Self {
        Shown {
            profile: true,
            points: true,
            dimensions: true,
            constraints: true,
        }
    }
}

/// A relation's glyph (fusion.md, *Sketch mode*): one kind for each relation the document
/// has (a coincidence of points is a shared point, drawn as none; a point on a curve shows
/// the coincident glyph), and the redundant relation's own (open item 28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GlyphKind {
    Horizontal,
    Vertical,
    Coincident,
    Midpoint,
    Tangent,
    Perpendicular,
    Parallel,
    Equal,
    Symmetric,
    Redundant,
}

/// What a stroke is: a curve (construction dashed), a dimension's line, the grid's, an
/// axis, a tool's entity under way, an inference line, or the pointer's crosshair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeKind {
    Curve {
        dashed: bool,
    },
    Dimension,
    GridMinor,
    GridMajor,
    Axis,
    /// A tool's entity under way, its `construction` as the add will make it.
    Preview {
        construction: bool,
    },
    Guide,
    Crosshair,
}

/// What a label is: a dimension's value, or a tool's word beside the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    Dimension,
    Tool,
}

/// One thing drawn, in screen points, with the member it's of, if any (a dimension's lines,
/// arrowheads, and value are its constraint's; a profile's fill is its region's first
/// curve's), its colour by state, and whether it's selected (drawn white over a halo).
#[derive(Debug, Clone, PartialEq)]
pub enum Drawn {
    Stroke {
        of: Option<ElementId>,
        kind: StrokeKind,
        points: Vec<Pos2>,
        colour: Color32,
        selected: bool,
    },
    Fill {
        of: Option<ElementId>,
        triangles: Vec<[Pos2; 3]>,
        colour: Color32,
    },
    Point {
        of: Option<ElementId>,
        at: Pos2,
        colour: Color32,
        selected: bool,
    },
    Origin {
        at: Pos2,
    },
    Glyph {
        of: Option<ElementId>,
        kind: GlyphKind,
        at: Pos2,
        colour: Color32,
        selected: bool,
    },
    /// An arrowhead, its tip at `tip`, its body along `toward` (a unit vector).
    Arrow {
        of: Option<ElementId>,
        tip: Pos2,
        toward: Vec2,
        colour: Color32,
        selected: bool,
    },
    Label {
        of: Option<ElementId>,
        kind: LabelKind,
        at: Pos2,
        text: String,
        colour: Color32,
        selected: bool,
    },
    /// A snap onto a point.
    Ring {
        at: Pos2,
        colour: Color32,
    },
}

impl Drawn {
    /// The member it's of, if any.
    pub fn of(&self) -> Option<&ElementId> {
        match self {
            Drawn::Stroke { of, .. }
            | Drawn::Fill { of, .. }
            | Drawn::Point { of, .. }
            | Drawn::Glyph { of, .. }
            | Drawn::Arrow { of, .. }
            | Drawn::Label { of, .. } => of.as_ref(),
            Drawn::Origin { .. } | Drawn::Ring { .. } => None,
        }
    }

    /// Its colour.
    pub fn colour(&self) -> Color32 {
        match self {
            Drawn::Stroke { colour, .. }
            | Drawn::Fill { colour, .. }
            | Drawn::Point { colour, .. }
            | Drawn::Glyph { colour, .. }
            | Drawn::Arrow { colour, .. }
            | Drawn::Label { colour, .. }
            | Drawn::Ring { colour, .. } => *colour,
            Drawn::Origin { .. } => SKETCH_ORIGIN_RING.color,
        }
    }

    /// Whether it's drawn as selected.
    pub fn selected(&self) -> bool {
        match self {
            Drawn::Stroke { selected, .. }
            | Drawn::Point { selected, .. }
            | Drawn::Glyph { selected, .. }
            | Drawn::Arrow { selected, .. }
            | Drawn::Label { selected, .. } => *selected,
            Drawn::Fill { .. } | Drawn::Origin { .. } | Drawn::Ring { .. } => false,
        }
    }

    /// Every screen position it holds.
    pub fn positions(&self) -> Vec<Pos2> {
        match self {
            Drawn::Stroke { points, .. } => points.clone(),
            Drawn::Fill { triangles, .. } => triangles.iter().flatten().copied().collect(),
            Drawn::Point { at, .. }
            | Drawn::Origin { at }
            | Drawn::Glyph { at, .. }
            | Drawn::Label { at, .. }
            | Drawn::Ring { at, .. } => vec![*at],
            Drawn::Arrow { tip, .. } => vec![*tip],
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

/// The read's geometry, found by ID: each point's solved place, and each curve, with its
/// kind and, for a line or an arc, its end points' IDs.
pub(crate) struct Geometry<'a> {
    points: BTreeMap<&'a ElementId, [f64; 2]>,
    curves: BTreeMap<&'a ElementId, (Curve, ElementKind)>,
    ends: BTreeMap<&'a ElementId, (&'a ElementId, &'a ElementId)>,
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
        let mut ends = BTreeMap::new();
        for line in &sketch.lines {
            if let (Some(&from), Some(&to)) = (points.get(&line.from), points.get(&line.to)) {
                curves.insert(&line.id, (Curve::Line { from, to }, ElementKind::Line));
                ends.insert(&line.id, (&line.from, &line.to));
            }
        }
        for circle in &sketch.circles {
            if let Some(&centre) = points.get(&circle.center) {
                let round = Curve::Round {
                    centre,
                    radius: circle.radius.mm(),
                    start: 0.0,
                    sweep: TAU,
                };
                curves.insert(&circle.id, (round, ElementKind::Circle));
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
                let round = Curve::Round {
                    centre,
                    radius: length(sub(start, centre)),
                    start: from,
                    sweep: (to - from).rem_euclid(TAU),
                };
                curves.insert(&arc.id, (round, ElementKind::Arc));
                ends.insert(&arc.id, (&arc.start, &arc.end));
            }
        }
        let middle = bounds(points.values().copied()).map_or([0.0, 0.0], |(lo, hi)| {
            [mid(lo[0], hi[0]), mid(lo[1], hi[1])]
        });
        Geometry {
            points,
            curves,
            ends,
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

/// A dimension, placed in the sketch's millimetres: its extension and dimension lines, its
/// arrowheads (each tip, and the direction its body runs from it), and its value's text and
/// where it stands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlacedDimension {
    pub(crate) of: ElementId,
    pub(crate) lines: Vec<Vec<[f64; 2]>>,
    pub(crate) arrows: Vec<([f64; 2], [f64; 2])>,
    pub(crate) text: String,
    pub(crate) text_at: [f64; 2],
}

/// Where each relation's glyphs and each dimension stand.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Placed {
    pub(crate) glyphs: Vec<PlacedGlyph>,
    pub(crate) dimensions: Vec<PlacedDimension>,
}

/// Where each constraint of `read` is drawn, at `scale`: each relation's glyph beside each
/// member it names (stacked, in the constraints' order, where a member has several), and
/// each dimension, as SolidWorks draws one, from the geometry it measures, its value at
/// the middle of its line, off the side away from the sketch's middle.
pub(crate) fn placed(read: &SketchRead, geometry: &Geometry, scale: Scale) -> Placed {
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
            if let Some(shape) = measure(&constraint.form, geometry, scale, None) {
                placed.dimensions.push(shape.of(constraint.id.clone()));
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
            if let Some(at) = glyph_place(member, *count, geometry, scale) {
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
/// symmetry's `about`, a point's curve), or the point a tangency is at.
fn relation(form: &Form) -> Option<(GlyphKind, Vec<&Reference>)> {
    Some(match form {
        Form::Horizontal { line } => (GlyphKind::Horizontal, vec![line]),
        Form::Vertical { line } => (GlyphKind::Vertical, vec![line]),
        Form::HorizontalPoints { a, b } => (GlyphKind::Horizontal, vec![a, b]),
        Form::VerticalPoints { a, b } => (GlyphKind::Vertical, vec![a, b]),
        Form::On { point, .. } => (GlyphKind::Coincident, vec![point]),
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

/// Where the `count`th glyph beside `member` stands, as the prototype places them: beside a
/// line, a share of its way along it, off its right side (below a line drawn left to
/// right), each further one a step along; beside a point, below and left of it, each
/// further one a step left; beside a circle or an arc, outside it, at 45° or its middle.
/// None for the origin and the axes, which aren't drawn.
fn glyph_place(
    member: &Reference,
    count: usize,
    geometry: &Geometry,
    scale: Scale,
) -> Option<[f64; 2]> {
    let (offset, step) = (scale.mm(SKETCH_GLYPH_OFFSET), scale.mm(GLYPH_STEP));
    let along = step * count as f64;
    let Reference::Element(_) = member else {
        return None;
    };
    if let Some(p) = geometry.point(member) {
        return Some([p[0] - offset - along, p[1] - offset]);
    }
    match geometry.curve(member)? {
        Curve::Line { from, to } => {
            let t = unit(sub(to, from)).unwrap_or([1.0, 0.0]);
            let right = [t[1], -t[0]];
            let at = add(from, mul(sub(to, from), GLYPH_ALONG));
            Some(add(add(at, mul(right, offset)), mul(t, along)))
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

/// A dimension's shape, in the sketch's millimetres, before it's of a constraint: as
/// [`PlacedDimension`]'s.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DimensionShape {
    pub(crate) lines: Vec<Vec<[f64; 2]>>,
    pub(crate) arrows: Vec<([f64; 2], [f64; 2])>,
    pub(crate) text: String,
    pub(crate) text_at: [f64; 2],
}

impl DimensionShape {
    /// Placed, as the constraint `of`'s.
    fn of(self, of: ElementId) -> PlacedDimension {
        PlacedDimension {
            of,
            lines: self.lines,
            arrows: self.arrows,
            text: self.text,
            text_at: self.text_at,
        }
    }
}

/// The dimension of form `form` (a driven one's value in parentheses), if the geometry it
/// measures is there: standing off from it as SolidWorks draws one, its value at its line's
/// middle; or, `through` a place (Smart Dimension's pointer), standing so its line runs
/// through it, and its value there.
pub(crate) fn measure(
    form: &Form,
    geometry: &Geometry,
    scale: Scale,
    through: Option<[f64; 2]>,
) -> Option<DimensionShape> {
    let offset = scale.mm(SKETCH_DIMENSION_OFFSET);
    let gap = scale.mm(SKETCH_DIMENSION_GAP);
    let past = scale.mm(EXTENSION_PAST);
    let driving = form.driving().unwrap_or(true);
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
    // From `a` and `b`, standing off along `n`: extension lines out from each (a gap off
    // the geometry, a little past the dimension line), the dimension line between them, an
    // arrowhead at each of its ends, and the value at its middle, or at `through`.
    let standing = |a: [f64; 2], b: [f64; 2], a1: [f64; 2], b1: [f64; 2], n: [f64; 2], value| {
        let t = unit(sub(b1, a1)).unwrap_or([1.0, 0.0]);
        DimensionShape {
            lines: vec![
                vec![add(a, mul(n, gap)), add(a1, mul(n, past))],
                vec![add(b, mul(n, gap)), add(b1, mul(n, past))],
                vec![a1, b1],
            ],
            arrows: vec![(a1, t), (b1, mul(t, -1.0))],
            text: text(value),
            text_at: through.unwrap_or([mid(a1[0], b1[0]), mid(a1[1], b1[1])]),
        }
    };
    let aligned = |a: [f64; 2], b: [f64; 2], value: String| {
        let t = unit(sub(b, a)).unwrap_or([1.0, 0.0]);
        let m = [mid(a[0], b[0]), mid(a[1], b[1])];
        let normal = [-t[1], t[0]];
        // Through the pointer: as far off as it is, on its side.
        let (n, off) = match through {
            Some(p) => {
                let off = dot(sub(p, a), normal);
                if off < 0.0 {
                    (mul(normal, -1.0), -off)
                } else {
                    (normal, off)
                }
            }
            None => (away(m, normal), offset),
        };
        standing(a, b, add(a, mul(n, off)), add(b, mul(n, off)), n, value)
    };
    Some(match form {
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
            let off = match form {
                Form::DistanceX { .. } => 1,
                _ => 0,
            };
            let m = [mid(pa[0], pb[0]), mid(pa[1], pb[1])];
            let (level, outward) = match through {
                Some(p) => (p[off], p[off] >= m[off]),
                None => {
                    let outward = m[off] >= geometry.middle[off];
                    let level = if outward {
                        pa[off].max(pb[off]) + offset
                    } else {
                        pa[off].min(pb[off]) - offset
                    };
                    (level, outward)
                }
            };
            let mut n = [0.0; 2];
            n[off] = if outward { 1.0 } else { -1.0 };
            let at = |p: [f64; 2]| {
                let mut q = p;
                q[off] = level;
                q
            };
            let extension = |p: [f64; 2]| {
                let mut from = p;
                from[off] += n[off] * gap;
                let mut to = at(p);
                to[off] += n[off] * past;
                vec![from, to]
            };
            let (a1, b1) = (at(pa), at(pb));
            let t = unit(sub(b1, a1)).unwrap_or([1.0, 0.0]);
            DimensionShape {
                lines: vec![extension(pa), extension(pb), vec![a1, b1]],
                arrows: vec![(a1, t), (b1, mul(t, -1.0))],
                text: text(value.to_string()),
                text_at: through.unwrap_or([mid(a1[0], b1[0]), mid(a1[1], b1[1])]),
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
            let t = unit(sub(foot, p)).unwrap_or([0.0, -1.0]);
            DimensionShape {
                lines: vec![vec![p, foot]],
                arrows: vec![(p, t), (foot, mul(t, -1.0))],
                text: text(value.to_string()),
                text_at: through.unwrap_or([mid(p[0], foot[0]), mid(p[1], foot[1])]),
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
            // Out toward the pointer; else through an arc's middle, or, for a circle, away
            // from the sketch's middle, or at 45° from a circle about it.
            let toward = through.and_then(|p| unit(sub(p, centre)));
            let u = match toward {
                Some(u) => u,
                None if sweep < TAU => direction(start + sweep / 2.0),
                None => {
                    unit(sub(centre, geometry.middle)).unwrap_or([FRAC_1_SQRT_2, FRAC_1_SQRT_2])
                }
            };
            let rim = add(centre, mul(u, radius));
            let beyond = match (through, toward) {
                (Some(p), Some(_)) => p,
                _ => add(centre, mul(u, radius + offset)),
            };
            let (lines, arrows, word) = if matches!(form, Form::Diameter { .. }) {
                let far = sub(centre, mul(u, radius));
                (
                    vec![vec![far, beyond]],
                    vec![(rim, mul(u, -1.0)), (far, u)],
                    "Ø",
                )
            } else {
                (vec![vec![centre, beyond]], vec![(rim, mul(u, -1.0))], "R")
            };
            DimensionShape {
                lines,
                arrows,
                text: text(format!("{word}{value}")),
                text_at: beyond,
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
            // Its arc's radius: as far as the pointer is from the corner, if it's given.
            let reach = through
                .map(|p| length(sub(p, corner)))
                .filter(|r| *r > 0.0 && r.is_finite())
                .unwrap_or(offset);
            let on = |at: f64| add(corner, mul(direction(at), reach));
            let arc: Vec<[f64; 2]> = (0..=ANGLE_ARC_SEGMENTS)
                .map(|k| on(from + sweep * k as f64 / ANGLE_ARC_SEGMENTS as f64))
                .collect();
            // The arrows along the arc's tangent, each pointing out of it.
            let sign = if sweep < 0.0 { -1.0 } else { 1.0 };
            let tangent = |at: f64| mul([-direction(at)[1], direction(at)[0]], sign);
            DimensionShape {
                lines: vec![arc],
                arrows: vec![
                    (on(from), tangent(from)),
                    (on(from + sweep), mul(tangent(from + sweep), -1.0)),
                ],
                text: text(format!("{value}°")),
                text_at: through.unwrap_or(on(from + sweep / 2.0)),
            }
        }
        _ => return None,
    })
}

/// The colour of a point or a curve in state `state`, of a sketch in state `sketch`: a
/// sketch not solved to buildable geometry colours every member its own way (unsolvable
/// red, invalid yellow); otherwise each member's own state.
pub fn member_colour(sketch: SketchState, state: Option<MemberState>) -> Color32 {
    match (sketch, state) {
        (SketchState::NoSolution, _) => SKETCH_OVER_DEFINED,
        (SketchState::Invalid, _) => SKETCH_INVALID,
        (_, Some(MemberState::FullyDefined)) => SKETCH_FULLY_DEFINED,
        (_, Some(MemberState::OverDefined)) => SKETCH_OVER_DEFINED,
        (_, Some(MemberState::UnderDefined) | None) => SKETCH_UNDER_DEFINED,
    }
}

/// The colour of a constraint of form `form` in state `state`, of a sketch in state
/// `sketch`: one in conflict, or a dimension repeating others, over defined; a relation
/// repeating others its own glyph's colour; one unsolved, on a sketch with no buildable
/// solution, that sketch's colour; a driven dimension gray; a driving one in the dimensions'
/// colour, and a relation in the glyphs'.
pub fn constraint_colour(
    sketch: SketchState,
    form: &Form,
    state: Option<ConstraintState>,
) -> Color32 {
    let dimension = form.is_dimension();
    match state {
        Some(ConstraintState::Conflicting) => SKETCH_OVER_DEFINED,
        Some(ConstraintState::Redundant | ConstraintState::PartiallyRedundant) if dimension => {
            SKETCH_OVER_DEFINED
        }
        Some(ConstraintState::Redundant | ConstraintState::PartiallyRedundant) => SKETCH_REDUNDANT,
        Some(ConstraintState::Unsolved) if sketch == SketchState::NoSolution => SKETCH_OVER_DEFINED,
        Some(ConstraintState::Unsolved) if sketch == SketchState::Invalid => SKETCH_INVALID,
        _ if form.driving() == Some(false) => SKETCH_DRIVEN,
        _ if dimension => SKETCH_DIMENSION_TEXT,
        _ => GRAY_700,
    }
}

/// The colours of the plane's two axes, the sketch's x and y: the model's axes they lie
/// along, for an origin plane; X's and Z's for a face's (as the prototype draws one).
pub fn axis_colours(plane: Option<&SketchPlane>) -> [Color32; 2] {
    let [u, v] = match plane {
        Some(SketchPlane::Xy) => [AXIS_X, AXIS_Y],
        Some(SketchPlane::Xz) => [AXIS_X, AXIS_Z],
        Some(SketchPlane::Yz) => [AXIS_Y, AXIS_Z],
        Some(SketchPlane::Face(_)) | None => [AXIS_X, AXIS_Z],
    };
    [color32(u), color32(v)]
}

/// A profile's region filled: its triangles, in the sketch's millimetres, and the curve it's
/// drawn as of (its outer loop's first).
#[derive(Debug, Clone, PartialEq)]
pub struct RegionFill {
    pub of: Option<ElementId>,
    pub triangles: Vec<[[f64; 2]; 3]>,
}

/// The scale a fill is worked out at for a view at `scale`: the power of two at or below it,
/// so a fill worked out once serves every zoom within a factor of two (its chords then stray
/// at most the chord tolerance at that zoom, less further in), and that power's exponent,
/// its key.
pub fn fill_scale(scale: Scale) -> (i32, Scale) {
    if !(scale.0 > 0.0 && scale.0.is_finite()) {
        return (0, Scale(1.0));
    }
    let (_, exponent) = libm::frexp(scale.0);
    let bucket = exponent - 1;
    (bucket, Scale(libm::ldexp(1.0, bucket)))
}

/// Each region of `read`'s profile filled, worked out in the sketch's millimetres at
/// `scale` (a frame's [`fill_scale`]), so a frame only projects it: the plane-to-screen map
/// takes triangles to triangles. Its loops are its outline's own polylines, each curve at
/// the segments its outline takes at that scale and chord `tolerance` (the view's, the
/// outline's: [`curve_segments`]), tessellated even-odd
/// (the outer loop less its holes) by `lyon_tessellation`: the fill meets the drawn outline
/// vertex for vertex, at every zoom. A region whose loops can't be walked, that holds more
/// than [`MAX_FILL_VERTICES`], or that lyon refuses or panics on, has none, and a warning of
/// its own says which.
pub fn region_fills(read: &SketchRead, scale: Scale, tolerance: f32) -> Vec<RegionFill> {
    let geometry = Geometry::of(read);
    read.regions
        .iter()
        .filter_map(|region| {
            let of = region.outer.first().cloned();
            let walked = std::iter::once(&region.outer)
                .chain(&region.holes)
                .map(|ring| walk(ring, &geometry, scale, tolerance))
                .collect::<Option<Vec<_>>>();
            let Some(loops) = walked else {
                tracing::warn!(
                    sketch = read.sketch.as_str(),
                    region = of.as_ref().map_or("", ElementId::as_str),
                    "a profile region's loop doesn't run end to end: it's left unfilled"
                );
                return None;
            };
            let count = loops.iter().map(Vec::len).sum::<usize>();
            if count > MAX_FILL_VERTICES {
                tracing::warn!(
                    sketch = read.sketch.as_str(),
                    region = of.as_ref().map_or("", ElementId::as_str),
                    vertices = count,
                    "a profile region past {MAX_FILL_VERTICES} vertices is left unfilled"
                );
                return None;
            }
            // A panic in the tessellator is no fill, as its refusal is: the outline is
            // drawn all the same.
            let tessellated =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tessellate(&loops)));
            match tessellated {
                Ok(Some(triangles)) => Some(RegionFill { of, triangles }),
                Ok(None) => {
                    tracing::warn!(
                        sketch = read.sketch.as_str(),
                        region = of.as_ref().map_or("", ElementId::as_str),
                        "the tessellator refused a profile region: it's left unfilled"
                    );
                    None
                }
                Err(payload) => {
                    let said = payload
                        .downcast_ref::<&str>()
                        .map(|said| (*said).to_owned())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_default();
                    tracing::warn!(
                        sketch = read.sketch.as_str(),
                        region = of.as_ref().map_or("", ElementId::as_str),
                        panic = said.as_str(),
                        "the tessellator panicked on a profile region: it's left unfilled"
                    );
                    None
                }
            }
        })
        .collect()
}

/// How many segments a circle or an arc of `radius` through `sweep` takes at `scale`, its
/// chords within `tolerance` points: a fill's and its outline's alike. A lone circle at
/// least [`MIN_FILL_SEGMENTS`], an arc its share of them by its sweep, at least one.
pub(crate) fn curve_segments(radius: f64, sweep: f64, scale: Scale, tolerance: f32) -> usize {
    let least = (MIN_FILL_SEGMENTS as f64 * sweep / TAU).ceil().max(1.0) as usize;
    segments(radius / scale.0.max(f64::MIN_POSITIVE), sweep, tolerance).max(least)
}

/// The triangles that fill `loops`, even-odd (an outer loop less its holes), as
/// `lyon_tessellation`'s fill tessellator gives them, in the order it gives them: the same
/// for the same loops on every run. It works in f32, so the loops are taken about their
/// bounds' middle (keeping the sketch's own precision near its region), and the triangles
/// moved back. None where it refuses the loops.
pub(crate) fn tessellate(loops: &[Vec<[f64; 2]>]) -> Option<Vec<[[f64; 2]; 3]>> {
    use lyon_tessellation::math::point;
    use lyon_tessellation::path::Path;
    use lyon_tessellation::{
        BuffersBuilder, FillOptions, FillTessellator, FillVertex, VertexBuffers,
    };
    let (lo, hi) = bounds(loops.iter().flatten().copied())?;
    let middle = [mid(lo[0], hi[0]), mid(lo[1], hi[1])];
    let mut path = Path::builder();
    for ring in loops {
        let mut points = ring.iter().map(|&p| {
            let p = sub(p, middle);
            point(p[0] as f32, p[1] as f32)
        });
        let Some(first) = points.next() else {
            continue;
        };
        path.begin(first);
        for p in points {
            path.line_to(p);
        }
        path.end(true);
    }
    let path = path.build();
    let mut buffers: VertexBuffers<[f64; 2], u32> = VertexBuffers::new();
    let mut builder = BuffersBuilder::new(&mut buffers, |vertex: FillVertex| {
        let p = vertex.position();
        add([f64::from(p.x), f64::from(p.y)], middle)
    });
    FillTessellator::new()
        .tessellate_path(&path, &FillOptions::even_odd(), &mut builder)
        .ok()?;
    let vertex = |i: u32| buffers.vertices.get(i as usize).copied();
    buffers
        .indices
        .chunks_exact(3)
        .map(|t| Some([vertex(t[0])?, vertex(t[1])?, vertex(t[2])?]))
        .collect()
}

/// What's drawn of `read`, seen through `view`, the members in `selected` drawn selected:
/// the plane's grid and axes (where the view says what's in it) and the origin; the
/// profile's fill (unless the sketch is over defined); each curve a polyline (an arc or a
/// circle within the view's chord tolerance of itself), construction dashed; each point a
/// square; and each relation's glyphs and each dimension; each as `shown` says.
pub fn drawing(
    read: &SketchRead,
    view: &View<'_>,
    shown: Shown,
    selected: &BTreeSet<ElementId>,
    fills: &[RegionFill],
) -> Vec<Drawn> {
    let geometry = Geometry::of(read);
    let sketch = read.states.state;
    let states = &read.states;
    let members = &read.members;
    let mut drawn = grid(view, axis_colours(read.plane.as_ref().ok()));
    if let Some(at) = view.at([0.0, 0.0]) {
        drawn.push(Drawn::Origin { at });
    }
    if shown.profile && sketch != SketchState::OverDefined {
        for fill in fills {
            let triangles = fill
                .triangles
                .iter()
                .map(|t| Some([view.at(t[0])?, view.at(t[1])?, view.at(t[2])?]))
                .collect::<Option<Vec<_>>>();
            if let Some(triangles) = triangles {
                drawn.push(Drawn::Fill {
                    of: fill.of.clone(),
                    triangles,
                    colour: SKETCH_PROFILE_FILL,
                });
            }
        }
    }
    let construction: BTreeMap<&ElementId, bool> = members
        .lines
        .iter()
        .map(|l| (&l.id, l.construction))
        .chain(members.circles.iter().map(|c| (&c.id, c.construction)))
        .chain(members.arcs.iter().map(|a| (&a.id, a.construction)))
        .collect();
    for (id, curve, _) in geometry.curves() {
        let state = states
            .lines
            .get(id)
            .or_else(|| states.circles.get(id))
            .or_else(|| states.arcs.get(id))
            .copied();
        let points = match curve {
            Curve::Line { from, to } => vec![view.at(from), view.at(to)],
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
                view.project,
                view.chord_tolerance,
                curve_segments(
                    radius,
                    sweep,
                    fill_scale(view.scale).1,
                    view.chord_tolerance,
                ),
            ),
        };
        let kind = StrokeKind::Curve {
            dashed: construction.get(id).copied().unwrap_or(false),
        };
        for run in runs(points) {
            drawn.push(Drawn::Stroke {
                of: Some(id.clone()),
                kind,
                points: run,
                colour: member_colour(sketch, state),
                selected: selected.contains(id),
            });
        }
    }
    if shown.points {
        for (id, at) in geometry.points() {
            if let Some(at) = view.at(at) {
                drawn.push(Drawn::Point {
                    of: Some(id.clone()),
                    at,
                    colour: member_colour(sketch, states.points.get(id).copied()),
                    selected: selected.contains(id),
                });
            }
        }
    }
    let placed = placed(read, &geometry, view.scale);
    let forms: BTreeMap<&ElementId, &Form> = members
        .constraints
        .iter()
        .map(|c| (&c.id, &c.form))
        .collect();
    let colour_of = |id: &ElementId| {
        forms.get(id).map_or(GRAY_700, |form| {
            constraint_colour(sketch, form, states.constraints.get(id).copied())
        })
    };
    if shown.constraints {
        for glyph in &placed.glyphs {
            if let Some(at) = view.at(glyph.at) {
                drawn.push(Drawn::Glyph {
                    of: Some(glyph.of.clone()),
                    kind: glyph.kind,
                    at,
                    colour: colour_of(&glyph.of),
                    selected: selected.contains(&glyph.of),
                });
            }
        }
    }
    if shown.dimensions {
        for dimension in &placed.dimensions {
            let colour = colour_of(&dimension.of);
            let selected = selected.contains(&dimension.of);
            for line in &dimension.lines {
                for run in runs(line.iter().map(|&p| view.at(p)).collect()) {
                    drawn.push(Drawn::Stroke {
                        of: Some(dimension.of.clone()),
                        kind: StrokeKind::Dimension,
                        points: run,
                        colour,
                        selected,
                    });
                }
            }
            for &(tip, along) in &dimension.arrows {
                if let (Some(at), Some(toward)) = (view.at(tip), view.direction(tip, along)) {
                    drawn.push(Drawn::Arrow {
                        of: Some(dimension.of.clone()),
                        tip: at,
                        toward,
                        colour,
                        selected,
                    });
                }
            }
            if let Some(at) = view.at(dimension.text_at) {
                drawn.push(Drawn::Label {
                    of: Some(dimension.of.clone()),
                    kind: LabelKind::Dimension,
                    at,
                    text: dimension.text.clone(),
                    colour,
                    selected,
                });
            }
        }
    }
    drawn
}

/// The grid on the plane, as the viewport's is: a line every 10 mm, each 50 mm's stronger,
/// fading out linearly from the origin to 200 mm, over the part of the plane in view; a
/// step whose lines would stand closer than [`GRID_LEAST_SPACING`] on the screen left out
/// (the minor lines first). And its two axes, in `axes`' colours. None where the view gives
/// no part of the plane.
fn grid(view: &View<'_>, axes: [Color32; 2]) -> Vec<Drawn> {
    let Some((lo, hi)) = view.visible else {
        return Vec::new();
    };
    let fade = f64::from(VIEWPORT_GRID_FADE_MM);
    let (lo, hi) = (
        [lo[0].max(-fade), lo[1].max(-fade)],
        [hi[0].min(fade), hi[1].min(fade)],
    );
    if !(lo[0] <= hi[0] && lo[1] <= hi[1]) {
        return Vec::new();
    }
    let spaced = |mm: f32| {
        view.scale.0 > 0.0 && f64::from(mm) / view.scale.0 >= f64::from(GRID_LEAST_SPACING)
    };
    let (minor, major) = (
        f64::from(VIEWPORT_GRID_MINOR_MM),
        f64::from(VIEWPORT_GRID_MAJOR_MM),
    );
    let step = if spaced(VIEWPORT_GRID_MINOR_MM) {
        minor
    } else if spaced(VIEWPORT_GRID_MAJOR_MM) {
        major
    } else {
        return Vec::new();
    };
    let mut drawn = Vec::new();
    for across in [0, 1] {
        let first = (lo[across] / step).ceil() as i64;
        let last = (hi[across] / step).floor() as i64;
        for k in first..=last {
            let at = k as f64 * step;
            let ends = if across == 0 {
                [[at, lo[1]], [at, hi[1]]]
            } else {
                [[lo[0], at], [hi[0], at]]
            };
            let is_major = (at / major).fract() == 0.0;
            let (kind, colour) = if is_major {
                (StrokeKind::GridMajor, color32(VIEWPORT_GRID_MAJOR))
            } else {
                (StrokeKind::GridMinor, color32(VIEWPORT_GRID_MINOR))
            };
            let faded = (1.0 - at.abs() / fade) as f32;
            if faded <= 0.0 {
                continue;
            }
            if let Some(run) = runs(ends.iter().map(|&p| view.at(p)).collect()).pop() {
                drawn.push(Drawn::Stroke {
                    of: None,
                    kind,
                    points: run,
                    colour: colour.gamma_multiply(faded),
                    selected: false,
                });
            }
        }
    }
    // The axes: the sketch's x along y = 0, its y along x = 0.
    for (axis, colour) in [0, 1].into_iter().zip(axes) {
        let other = 1 - axis;
        if !(lo[other] <= 0.0 && 0.0 <= hi[other]) {
            continue;
        }
        let ends = if axis == 0 {
            [[lo[0], 0.0], [hi[0], 0.0]]
        } else {
            [[0.0, lo[1]], [0.0, hi[1]]]
        };
        if let Some(run) = runs(ends.iter().map(|&p| view.at(p)).collect()).pop() {
            drawn.push(Drawn::Stroke {
                of: None,
                kind: StrokeKind::Axis,
                points: run,
                colour,
                selected: false,
            });
        }
    }
    drawn
}

#[cfg(test)]
thread_local! {
    /// How many polylines this thread has made ([`polyline`]): what a test counts.
    pub(crate) static POLYLINES_MADE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `curve`'s polyline in the sketch's millimetres, at `scale` and `tolerance`: a line its
/// two ends; a circle or an arc its points from its start round its sweep, as many segments
/// as [`curve_segments`] says keep every chord within `tolerance` points at `scale`, the
/// first and the last its ends (a circle's the same point, to rounding). The one polyline of
/// a curve: region fills ([`walk`]) and the 3D view's lines ([`super::lines`]) meet outlines
/// vertex for vertex.
pub(crate) fn polyline(curve: Curve, scale: Scale, tolerance: f32) -> Vec<[f64; 2]> {
    #[cfg(test)]
    POLYLINES_MADE.with(|made| made.set(made.get() + 1));
    match curve {
        Curve::Line { from, to } => vec![from, to],
        Curve::Round {
            centre,
            radius,
            start,
            sweep,
        } => {
            let n = curve_segments(radius, sweep, scale, tolerance);
            (0..=n)
                .map(|k| {
                    add(
                        centre,
                        mul(direction(start + sweep * k as f64 / n as f64), radius),
                    )
                })
                .collect()
        }
    }
}

/// The polygon a loop of `curves` walks, in order, in the sketch's millimetres, its arcs and
/// circles at `scale`'s and `tolerance`'s segments ([`curve_segments`]), its vertices on them
/// as its outline's: each curve run as [`oriented`] runs it. None for a curve the read
/// hasn't, or a loop whose curves don't meet end to end.
fn walk(
    curves: &[ElementId],
    geometry: &Geometry,
    scale: Scale,
    tolerance: f32,
) -> Option<Vec<[f64; 2]>> {
    let mut polygon = Vec::new();
    // A lone curve, a circle's whole turn, too: its ring closed by its last point let go.
    for walked in oriented(curves, geometry)? {
        let mut piece = polyline(walked.curve, scale, tolerance);
        if walked.backwards {
            piece.reverse();
        }
        piece.pop();
        polygon.extend(piece);
    }
    Some(polygon)
}

/// A curve as a loop's walk runs it: as drawn, or `backwards`, from its end to its start;
/// with its kind and its end points as the read stores them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Walked {
    pub(crate) curve: Curve,
    pub(crate) kind: ElementKind,
    /// A line's or an arc's end points as stored, from and to as drawn; none for a circle.
    pub(crate) points: Option<([f64; 2], [f64; 2])>,
    pub(crate) backwards: bool,
}

impl Walked {
    /// Where the walk enters it and where it leaves it: its stored end points, the other way
    /// round when it's run backwards; none for a circle.
    pub(crate) fn ends(&self) -> Option<([f64; 2], [f64; 2])> {
        self.points.map(|(from, to)| {
            if self.backwards {
                (to, from)
            } else {
                (from, to)
            }
        })
    }
}

/// The curves a loop of `curves` walks, in order, each as the walk runs it ([`Walked`]):
/// each from the end the last one ended at (the first, toward the one after it); a lone
/// curve, a circle's whole turn, as drawn. None for a curve the read hasn't, or a loop whose
/// curves don't meet end to end.
pub(crate) fn oriented(curves: &[ElementId], geometry: &Geometry) -> Option<Vec<Walked>> {
    let walked = |id: &ElementId, backwards: bool| {
        let &(curve, kind) = geometry.curves.get(id)?;
        let points = geometry.ends.get(id).and_then(|(from, to)| {
            Some((*geometry.points.get(*from)?, *geometry.points.get(*to)?))
        });
        Some(Walked {
            curve,
            kind,
            points,
            backwards,
        })
    };
    let [first, rest @ ..] = curves else {
        return None;
    };
    if rest.is_empty() {
        return Some(vec![walked(first, false)?]);
    }
    let ends = |id: &ElementId| geometry.ends.get(id).copied();
    let (start, end) = ends(first)?;
    let (next_start, next_end) = ends(&rest[0])?;
    let forward = end == next_start || end == next_end;
    let mut at = if forward { end } else { start };
    let mut walk = vec![walked(first, !forward)?];
    for id in rest {
        let (start, end) = ends(id)?;
        let backwards = if start == at {
            at = end;
            false
        } else if end == at {
            at = start;
            true
        } else {
            return None;
        };
        walk.push(walked(id, backwards)?);
    }
    Some(walk)
}

/// What's drawn of a tool's preview (the core's `sketch_tools::Preview`), seen through
/// `view`: the entity under way dashed, its points, the dotted inference lines, the
/// relations the pointer shows as glyphs beside it (a snap onto a point as a ring), the
/// pointer's crosshair, and each length and radius it measures, labelled beside the middle
/// of its segment.
pub fn preview(preview: &Preview, view: &View<'_>) -> Vec<Drawn> {
    let mut drawn = Vec::new();
    let stroke = |points: Vec<Option<Pos2>>, kind, colour, drawn: &mut Vec<Drawn>| {
        for run in runs(points) {
            drawn.push(Drawn::Stroke {
                of: None,
                kind,
                points: run,
                colour,
                selected: false,
            });
        }
    };
    for guide in &preview.guides {
        stroke(
            guide.iter().map(|&p| view.at(p)).collect(),
            StrokeKind::Guide,
            SKETCH_GUIDE_STROKE.color,
            &mut drawn,
        );
    }
    for line in &preview.lines {
        stroke(
            line.iter().map(|&p| view.at(p)).collect(),
            StrokeKind::Preview {
                construction: preview.construction,
            },
            SKETCH_PREVIEW_STROKE.color,
            &mut drawn,
        );
    }
    for &(centre, radius) in &preview.circles {
        let points = round_points(
            centre,
            radius,
            0.0,
            TAU,
            view.project,
            view.chord_tolerance,
            0,
        );
        stroke(
            points,
            StrokeKind::Preview {
                construction: preview.construction,
            },
            SKETCH_PREVIEW_STROKE.color,
            &mut drawn,
        );
    }
    for arc in &preview.arcs {
        let from = angle(sub(arc.start, arc.center));
        let sweep = (angle(sub(arc.end, arc.center)) - from).rem_euclid(TAU);
        let radius = length(sub(arc.start, arc.center));
        let points = round_points(
            arc.center,
            radius,
            from,
            sweep,
            view.project,
            view.chord_tolerance,
            0,
        );
        stroke(
            points,
            StrokeKind::Preview {
                construction: preview.construction,
            },
            SKETCH_PREVIEW_STROKE.color,
            &mut drawn,
        );
    }
    for &p in preview.points.iter().chain(&preview.construction_points) {
        if let Some(at) = view.at(p) {
            drawn.push(Drawn::Point {
                of: None,
                at,
                colour: SKETCH_PREVIEW_STROKE.color,
                selected: false,
            });
        }
    }
    let mut stacked = 0.0;
    for shown in &preview.shown {
        let Some(at) = view.at(shown.at) else {
            continue;
        };
        let (kind, colour) = match &shown.relation {
            Relation::Shared(_) | Relation::Origin => {
                drawn.push(Drawn::Ring {
                    at,
                    colour: SKETCH_SNAP_STROKE.color,
                });
                (GlyphKind::Coincident, SKETCH_SNAP_STROKE.color)
            }
            Relation::Midpoint(_) => (GlyphKind::Midpoint, FOCUS),
            Relation::On(_) => (GlyphKind::Coincident, FOCUS),
            Relation::Horizontal | Relation::HorizontalPoints(_) => (GlyphKind::Horizontal, FOCUS),
            Relation::Vertical | Relation::VerticalPoints(_) => (GlyphKind::Vertical, FOCUS),
            Relation::Perpendicular(_) => (GlyphKind::Perpendicular, FOCUS),
            Relation::Parallel(_) => (GlyphKind::Parallel, FOCUS),
            Relation::Tangent(_) => (GlyphKind::Tangent, FOCUS),
        };
        drawn.push(Drawn::Glyph {
            of: None,
            kind,
            at: at + SHOWN_AT + Vec2::new(stacked, 0.0),
            colour,
            selected: false,
        });
        stacked += GLYPH_STEP;
    }
    if let Some(pointer) = preview.pointer.and_then(|p| view.at(p)) {
        let arm = CROSSHAIR_ARM;
        for (dx, dy) in [(arm, 0.0), (0.0, arm)] {
            let d = Vec2::new(dx, dy);
            stroke(
                vec![Some(pointer - d), Some(pointer + d)],
                StrokeKind::Crosshair,
                SKETCH_CROSSHAIR_STROKE.color,
                &mut drawn,
            );
        }
    }
    for measure in &preview.measures {
        let (Some(from), Some(to)) = (view.at(measure.from), view.at(measure.to)) else {
            continue;
        };
        let value = length(sub(measure.to, measure.from));
        let text = match measure.kind {
            MeasureKind::Length => format!("{value:.2} mm"),
            MeasureKind::Radius => format!("R {value:.2} mm"),
        };
        drawn.push(Drawn::Label {
            of: None,
            kind: LabelKind::Tool,
            at: from + (to - from) / 2.0 + aside(to - from) * MEASURE_LABEL_OFFSET,
            text,
            colour: SKETCH_DIMENSION_TEXT,
            selected: false,
        });
    }
    drawn
}

/// What's drawn of Smart Dimension's preview (the core's `DimensionPreview`) over `read`,
/// seen through `view`: what's picked drawn selected; what a click would pick in the focus
/// blue; the dimension a click would place, or the Modify box's, standing through the
/// pointer (or, while the box is open, through `held`, the click that opened it), in the
/// focus blue, its value there unless the box stands there in its place; and the pointer's
/// crosshair.
pub fn dimension_preview(
    preview: &DimensionPreview,
    read: &SketchRead,
    view: &View<'_>,
    held: Option<[f64; 2]>,
) -> Vec<Drawn> {
    let geometry = Geometry::of(read);
    let mut drawn = Vec::new();
    for (reference, picked) in preview
        .picks
        .iter()
        .map(|r| (r, true))
        .chain(preview.hover.iter().map(|r| (r, false)))
    {
        let of = reference.element().cloned();
        if let Some(p) = geometry.point(reference) {
            if let Some(at) = view.at(p) {
                drawn.push(Drawn::Point {
                    of,
                    at,
                    colour: FOCUS,
                    selected: picked,
                });
                if !picked {
                    drawn.push(Drawn::Ring { at, colour: FOCUS });
                }
            }
            continue;
        }
        let Some(curve) = geometry.curve(reference) else {
            continue;
        };
        let points = match curve {
            // An axis is drawn along the grid's: its pick as a stretch of it about the origin.
            Curve::Line { from, to } if of.is_none() => {
                let reach = view.scale.mm(SKETCH_DIMENSION_OFFSET * 4.0);
                let d = sub(to, from);
                vec![view.at(mul(d, -reach)), view.at(mul(d, reach))]
            }
            Curve::Line { from, to } => vec![view.at(from), view.at(to)],
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
                view.project,
                view.chord_tolerance,
                0,
            ),
        };
        for run in runs(points) {
            drawn.push(Drawn::Stroke {
                of: of.clone(),
                kind: StrokeKind::Curve { dashed: false },
                points: run,
                colour: FOCUS,
                selected: picked,
            });
        }
    }
    let through = held.or(preview.pointer);
    if let (Some(form), Some(through)) = (&preview.dimension, through)
        && let Some(shape) = measure(form, &geometry, view.scale, Some(through))
    {
        for line in &shape.lines {
            for run in runs(line.iter().map(|&p| view.at(p)).collect()) {
                drawn.push(Drawn::Stroke {
                    of: None,
                    kind: StrokeKind::Dimension,
                    points: run,
                    colour: FOCUS,
                    selected: false,
                });
            }
        }
        for &(tip, along) in &shape.arrows {
            if let (Some(at), Some(toward)) = (view.at(tip), view.direction(tip, along)) {
                drawn.push(Drawn::Arrow {
                    of: None,
                    tip: at,
                    toward,
                    colour: FOCUS,
                    selected: false,
                });
            }
        }
        if preview.modify.is_none()
            && let Some(at) = view.at(shape.text_at)
        {
            drawn.push(Drawn::Label {
                of: None,
                kind: LabelKind::Dimension,
                at,
                text: shape.text,
                colour: FOCUS,
                selected: false,
            });
        }
    }
    if preview.modify.is_none()
        && let Some(pointer) = preview.pointer.and_then(|p| view.at(p))
    {
        for d in [Vec2::new(CROSSHAIR_ARM, 0.0), Vec2::new(0.0, CROSSHAIR_ARM)] {
            drawn.push(Drawn::Stroke {
                of: None,
                kind: StrokeKind::Crosshair,
                points: vec![pointer - d, pointer + d],
                colour: SKETCH_CROSSHAIR_STROKE.color,
                selected: false,
            });
        }
    }
    drawn
}

/// The ghost of a drag (section 9, *Ghost image on drag*): what's drawn of the begin state,
/// `begun`, of the members the drag has moved from there, faint, and never selected or a
/// fill. A member moved is a point whose place differs from where it began, a circle whose
/// radius does, and a curve with a point moved; a constraint's glyphs and dimensions
/// aren't, their members' ghosts showing where they were.
pub fn ghost(begun: &[Drawn], begin: &SketchRead, now: &SketchRead) -> Vec<Drawn> {
    let at = |read: &SketchRead| -> BTreeMap<ElementId, [f64; 2]> {
        read.members
            .points
            .iter()
            .map(|p| (p.id.clone(), [p.x.mm(), p.y.mm()]))
            .collect()
    };
    let (was, is) = (at(begin), at(now));
    let moved_points: BTreeSet<&ElementId> = was
        .iter()
        .filter(|(id, p)| is.get(*id).is_some_and(|q| q != *p))
        .map(|(id, _)| id)
        .collect();
    let radius = |read: &SketchRead| -> BTreeMap<ElementId, f64> {
        read.members
            .circles
            .iter()
            .map(|c| (c.id.clone(), c.radius.mm()))
            .collect()
    };
    let (was_r, is_r) = (radius(begin), radius(now));
    let members = &begin.members;
    let mut moved: BTreeSet<&ElementId> = moved_points.clone();
    moved.extend(
        members
            .lines
            .iter()
            .filter(|l| moved_points.contains(&l.from) || moved_points.contains(&l.to))
            .map(|l| &l.id),
    );
    moved.extend(
        members
            .circles
            .iter()
            .filter(|c| {
                moved_points.contains(&c.center)
                    || is_r.get(&c.id).is_some_and(|r| was_r.get(&c.id) != Some(r))
            })
            .map(|c| &c.id),
    );
    moved.extend(
        members
            .arcs
            .iter()
            .filter(|a| {
                [&a.center, &a.start, &a.end]
                    .iter()
                    .any(|p| moved_points.contains(p))
            })
            .map(|a| &a.id),
    );
    begun
        .iter()
        .filter(|d| {
            matches!(
                d,
                Drawn::Stroke {
                    kind: StrokeKind::Curve { .. },
                    ..
                } | Drawn::Point { .. }
            )
        })
        .filter(|d| d.of().is_some_and(|id| moved.contains(id)))
        .map(|d| {
            let mut d = d.clone();
            match &mut d {
                Drawn::Stroke {
                    colour, selected, ..
                }
                | Drawn::Point {
                    colour, selected, ..
                } => {
                    *colour = colour.gamma_multiply(SKETCH_GHOST_OPACITY);
                    *selected = false;
                }
                _ => {}
            }
            d
        })
        .collect()
}

/// The screen's unit direction square to `along`, on its lower side (to the right of a
/// segment going up, below one across); straight down for none.
fn aside(along: Vec2) -> Vec2 {
    let normal = Vec2::new(-along.y, along.x);
    let length = normal.length();
    if length <= 0.0 || !length.is_finite() {
        return Vec2::new(0.0, 1.0);
    }
    let normal = normal / length;
    if normal.y < 0.0 || (normal.y == 0.0 && normal.x < 0.0) {
        -normal
    } else {
        normal
    }
}

/// The polyline of a circle or an arc about `centre`, of `radius`, from the angle `start`
/// through `sweep`, its vertices on the curve and its chords within `tolerance` screen
/// points of it: as many segments as that needs at the projection's largest stretch about
/// the centre, and at least `at_least` (the profile fill's own count, so the two share
/// their vertices), at most [`MAX_CURVE_SEGMENTS`]; a whole turn ending where it starts.
pub(crate) fn round_points(
    centre: [f64; 2],
    radius: f64,
    start: f64,
    sweep: f64,
    project: &dyn Fn([f64; 2]) -> Option<Pos2>,
    tolerance: f32,
    at_least: usize,
) -> Vec<Option<Pos2>> {
    let project = |at: [f64; 2]| project(at).filter(|p| p.x.is_finite() && p.y.is_finite());
    let segments = segments(stretch(centre, radius, &project) * radius, sweep, tolerance)
        .max(at_least)
        .min(MAX_CURVE_SEGMENTS);
    let mut points: Vec<Option<Pos2>> = (0..=segments)
        .map(|k| {
            let at = start + sweep * k as f64 / segments as f64;
            project(add(centre, mul(direction(at), radius)))
        })
        .collect();
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
    if radius <= 0.0 {
        return 0.0;
    }
    let (Some(c), Some(x), Some(y)) = (
        at(centre),
        at(add(centre, [radius, 0.0])),
        at(add(centre, [0.0, radius])),
    ) else {
        return f64::INFINITY;
    };
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
/// restyle of sketch mode is this, and `style`'s values. A selected item is drawn white
/// over a wider halo.
pub fn to_shapes(painter: &Painter, drawn: &[Drawn]) -> Vec<Shape> {
    let mut shapes = Vec::new();
    let halo = Stroke::new(SKETCH_SELECTED_HALO_WIDTH, SKETCH_SELECTED_HALO);
    for item in drawn {
        match item {
            Drawn::Stroke {
                kind,
                points,
                colour,
                selected,
                ..
            } => {
                let (width, dash) = match kind {
                    StrokeKind::Curve { dashed } => (
                        SKETCH_STROKE_WIDTH,
                        dashed.then_some(SKETCH_CONSTRUCTION_DASH),
                    ),
                    StrokeKind::Dimension => (DIMENSION_LINE_WIDTH, None),
                    StrokeKind::GridMinor | StrokeKind::GridMajor => (GRID_LINE_WIDTH, None),
                    StrokeKind::Axis => (VIEWPORT_GRID_AXIS_WIDTH, None),
                    // Dashed as construction is, construction or not (the prototype's): the
                    // preview's dash and construction's are one.
                    StrokeKind::Preview { .. } => {
                        (SKETCH_PREVIEW_STROKE.width, Some(SKETCH_CONSTRUCTION_DASH))
                    }
                    StrokeKind::Guide => (SKETCH_GUIDE_STROKE.width, Some(SKETCH_GUIDE_DASH)),
                    StrokeKind::Crosshair => (SKETCH_CROSSHAIR_STROKE.width, None),
                };
                let stroke = if *selected {
                    shapes.push(Shape::line(points.clone(), halo));
                    Stroke::new(SKETCH_SELECTED_WIDTH, SKETCH_SELECTED_STROKE)
                } else {
                    Stroke::new(width, *colour)
                };
                match dash {
                    Some([dash, gap]) => {
                        shapes.extend(Shape::dashed_line(points, stroke, dash, gap))
                    }
                    None => shapes.push(Shape::line(points.clone(), stroke)),
                }
            }
            Drawn::Fill {
                triangles, colour, ..
            } => {
                let mut mesh = egui::Mesh::default();
                for triangle in triangles {
                    let base = mesh.vertices.len() as u32;
                    for &p in triangle {
                        mesh.colored_vertex(p, *colour);
                    }
                    mesh.add_triangle(base, base + 1, base + 2);
                }
                shapes.push(Shape::mesh(mesh));
            }
            Drawn::Point {
                at,
                colour,
                selected,
                ..
            } => {
                let square = Rect::from_center_size(*at, Vec2::splat(SKETCH_POINT_SIZE));
                if *selected {
                    let wide = square.expand(SKETCH_SELECTED_HALO_WIDTH / 2.0);
                    shapes.push(Shape::rect_filled(wide, 0.0, SKETCH_SELECTED_HALO));
                    shapes.push(Shape::rect_filled(square, 0.0, SKETCH_SELECTED_STROKE));
                } else {
                    shapes.push(Shape::rect_filled(square, 0.0, *colour));
                }
            }
            Drawn::Origin { at } => {
                shapes.push(Shape::circle_stroke(
                    *at,
                    SKETCH_ORIGIN_RING_RADIUS,
                    Stroke::new(SKETCH_ORIGIN_RING.width, SKETCH_ORIGIN_RING.color),
                ));
            }
            Drawn::Glyph {
                kind,
                at,
                colour,
                selected,
                ..
            } => shapes.extend(glyph(*kind, *at, *colour, *selected)),
            Drawn::Arrow {
                tip,
                toward,
                colour,
                selected,
                ..
            } => {
                let colour = if *selected {
                    SKETCH_SELECTED_STROKE
                } else {
                    *colour
                };
                let base = *tip + *toward * SKETCH_DIMENSION_ARROW[0];
                let side = toward.rot90() * (SKETCH_DIMENSION_ARROW[1] / 2.0);
                shapes.push(Shape::convex_polygon(
                    vec![*tip, base + side, base - side],
                    colour,
                    Stroke::NONE,
                ));
            }
            Drawn::Label {
                kind,
                at,
                text,
                colour,
                selected,
                ..
            } => shapes.extend(label(painter, *kind, *at, text, *colour, *selected)),
            Drawn::Ring { at, colour } => {
                shapes.push(Shape::circle_stroke(
                    *at,
                    SNAP_RADIUS,
                    Stroke::new(SKETCH_SNAP_STROKE.width, *colour),
                ));
            }
        }
    }
    shapes
}

/// A label's shapes: its box (a dimension's dark, a tool's as a floating panel), then its
/// text, centred at `at` (a tool's from its left).
fn label(
    painter: &Painter,
    kind: LabelKind,
    at: Pos2,
    text: &str,
    colour: Color32,
    selected: bool,
) -> Vec<Shape> {
    let (size, fill, border) = match kind {
        LabelKind::Dimension => (
            CANVAS_TEXT_SIZE,
            SKETCH_DIMENSION_LABEL_BG,
            selected.then_some(SKETCH_SELECTED_STROKE),
        ),
        LabelKind::Tool => (CANVAS_LABEL_SIZE, SKETCH_GLYPH_BG, Some(GRAY_350)),
    };
    let colour = if selected {
        SKETCH_SELECTED_STROKE
    } else {
        colour
    };
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(size), colour);
    let box_size = Vec2::new(
        galley.size().x + 2.0 * f32::from(SKETCH_DIMENSION_LABEL_PADDING.left),
        LABEL_HEIGHT,
    );
    let frame = match kind {
        LabelKind::Dimension => Rect::from_center_size(at, box_size),
        LabelKind::Tool => Rect::from_min_size(at, box_size),
    };
    let mut shapes = vec![Shape::rect_filled(
        frame,
        CornerRadius::same(GLYPH_RADIUS),
        fill,
    )];
    if let Some(border) = border {
        shapes.push(Shape::rect_stroke(
            frame,
            CornerRadius::same(GLYPH_RADIUS),
            Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        ));
    }
    let corner = frame.center() - galley.size() / 2.0;
    shapes.push(Shape::galley(corner, galley, colour));
    shapes
}

/// A glyph of `kind` at `at`: its box (bordered in `colour` where that says a state, white
/// over a halo if selected), then its icon, drawn by one function for each kind, in the
/// prototype's 20-unit icon square scaled into the box.
fn glyph(kind: GlyphKind, at: Pos2, colour: Color32, selected: bool) -> Vec<Shape> {
    let side = Rect::from_center_size(at, Vec2::splat(SKETCH_GLYPH_SIZE));
    let radius = CornerRadius::same(GLYPH_RADIUS);
    let mut shapes = Vec::new();
    if selected {
        shapes.push(Shape::rect_filled(
            side.expand(SKETCH_SELECTED_HALO_WIDTH / 2.0),
            radius,
            SKETCH_SELECTED_HALO,
        ));
    }
    let border = match (selected, colour == GRAY_700) {
        (true, _) => SKETCH_SELECTED_STROKE,
        (false, true) => SKETCH_GLYPH_STROKE,
        (false, false) => colour,
    };
    shapes.push(Shape::rect_filled(side, radius, SKETCH_GLYPH_BG));
    shapes.push(Shape::rect_stroke(
        side,
        radius,
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    ));
    let icon = Icon { at, colour };
    shapes.extend(match kind {
        GlyphKind::Horizontal => horizontal_icon(icon),
        GlyphKind::Vertical => vertical_icon(icon),
        GlyphKind::Coincident => coincident_icon(icon),
        GlyphKind::Midpoint => midpoint_icon(icon),
        GlyphKind::Tangent => tangent_icon(icon),
        GlyphKind::Perpendicular => perpendicular_icon(icon),
        GlyphKind::Parallel => parallel_icon(icon),
        GlyphKind::Equal => equal_icon(icon),
        GlyphKind::Symmetric => symmetric_icon(icon),
        GlyphKind::Redundant => redundant_icon(icon),
    });
    shapes
}

/// An icon's place and colour: the prototype's icons are drawn in a 20-unit square, scaled
/// to 0.6 of it about the glyph's centre.
#[derive(Clone, Copy)]
struct Icon {
    at: Pos2,
    colour: Color32,
}

impl Icon {
    const SCALE: f32 = 0.6;

    /// The icon square's `(x, y)`, on the screen.
    fn p(self, x: f32, y: f32) -> Pos2 {
        self.at + Vec2::new(x - 10.0, y - 10.0) * Self::SCALE
    }

    fn stroke(self) -> Stroke {
        Stroke::new(ICON_WIDTH, self.colour)
    }

    /// A line from `a` to `b`, in the icon square, at `opacity`.
    fn line(self, a: (f32, f32), b: (f32, f32), opacity: f32) -> Shape {
        let stroke = Stroke::new(ICON_WIDTH, self.colour.gamma_multiply(opacity));
        Shape::line_segment([self.p(a.0, a.1), self.p(b.0, b.1)], stroke)
    }

    fn ring(self, centre: (f32, f32), radius: f32) -> Shape {
        Shape::circle_stroke(
            self.p(centre.0, centre.1),
            radius * Self::SCALE,
            self.stroke(),
        )
    }

    fn dot(self, centre: (f32, f32), radius: f32) -> Shape {
        Shape::circle_filled(
            self.p(centre.0, centre.1),
            radius * Self::SCALE,
            self.colour,
        )
    }
}

fn horizontal_icon(i: Icon) -> Vec<Shape> {
    vec![
        i.line((3.5, 10.0), (16.5, 10.0), 1.0),
        i.line((3.5, 7.0), (3.5, 13.0), 0.5),
        i.line((16.5, 7.0), (16.5, 13.0), 0.5),
    ]
}

fn vertical_icon(i: Icon) -> Vec<Shape> {
    vec![
        i.line((10.0, 3.5), (10.0, 16.5), 1.0),
        i.line((7.0, 3.5), (13.0, 3.5), 0.5),
        i.line((7.0, 16.5), (13.0, 16.5), 0.5),
    ]
}

fn coincident_icon(i: Icon) -> Vec<Shape> {
    vec![i.ring((10.0, 10.0), 5.5), i.dot((10.0, 10.0), 1.8)]
}

fn midpoint_icon(i: Icon) -> Vec<Shape> {
    vec![
        i.line((3.0, 13.0), (17.0, 13.0), 1.0),
        Shape::convex_polygon(
            vec![i.p(10.0, 7.5), i.p(13.5, 13.0), i.p(6.5, 13.0)],
            i.colour,
            Stroke::NONE,
        ),
    ]
}

fn tangent_icon(i: Icon) -> Vec<Shape> {
    vec![
        i.ring((10.0, 12.0), 4.5),
        i.line((3.0, 7.5), (17.0, 7.5), 1.0),
    ]
}

fn perpendicular_icon(i: Icon) -> Vec<Shape> {
    vec![
        i.line((4.0, 16.0), (16.0, 16.0), 1.0),
        i.line((10.0, 16.0), (10.0, 4.0), 1.0),
    ]
}

fn parallel_icon(i: Icon) -> Vec<Shape> {
    vec![
        i.line((6.0, 16.0), (11.0, 4.0), 1.0),
        i.line((10.0, 16.0), (15.0, 4.0), 1.0),
    ]
}

fn equal_icon(i: Icon) -> Vec<Shape> {
    vec![
        i.line((4.5, 7.5), (15.5, 7.5), 1.0),
        i.line((4.5, 12.5), (15.5, 12.5), 1.0),
    ]
}

fn symmetric_icon(i: Icon) -> Vec<Shape> {
    let mut shapes = Shape::dashed_line(&[i.p(10.0, 3.0), i.p(10.0, 17.0)], i.stroke(), 1.2, 1.0);
    for (a, b) in [
        ((6.0, 6.0), (6.0, 14.0)),
        ((14.0, 6.0), (14.0, 14.0)),
        ((3.5, 6.0), (6.0, 6.0)),
        ((3.5, 14.0), (6.0, 14.0)),
        ((14.0, 6.0), (16.5, 6.0)),
        ((14.0, 14.0), (16.5, 14.0)),
    ] {
        shapes.push(i.line(a, b, 1.0));
    }
    shapes
}

/// A redundant relation's own: an exclamation mark, as a warning is.
fn redundant_icon(i: Icon) -> Vec<Shape> {
    vec![
        i.line((10.0, 4.0), (10.0, 12.0), 1.0),
        i.dot((10.0, 15.5), 1.4),
    ]
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

    /// Millimetres per screen point.
    pub fn scale(&self) -> Scale {
        Scale(1.0 / self.zoom)
    }
}

/// The smallest box holding `points`: none for none.
pub(crate) fn bounds(points: impl Iterator<Item = [f64; 2]>) -> Option<([f64; 2], [f64; 2])> {
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

/// How far `at` is from the segment from `a` to `b`: from its nearest point on it, an end
/// past either end (from `a` for a segment of no length).
pub(crate) fn segment_distance(at: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = sub(b, a);
    let dd = dot(d, d);
    if dd <= 0.0 {
        return length(sub(at, a));
    }
    let t = dot(sub(at, a), d) / dd;
    if t <= 0.0 {
        length(sub(at, a))
    } else if t >= 1.0 {
        length(sub(at, b))
    } else {
        length(sub(at, foot(at, a, b)))
    }
}

/// Where the line through `a` along `da` meets the one through `b` along `db`: none if
/// they're parallel.
fn crossing(a: [f64; 2], da: [f64; 2], b: [f64; 2], db: [f64; 2]) -> Option<[f64; 2]> {
    let c = cross(da, db);
    if c == 0.0 || !c.is_finite() {
        return None;
    }
    let t = cross(sub(b, a), db) / c;
    Some(add(a, mul(da, t)))
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

pub(crate) fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

pub(crate) fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

pub(crate) fn mul(a: [f64; 2], k: f64) -> [f64; 2] {
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

#[cfg(test)]
mod tests {
    //! The private geometry's own tests, each worked by hand.

    use super::*;

    #[test]
    fn aside_is_square_to_a_segment_on_its_lower_side_and_down_for_none() {
        // Right, (1, 0): its normal (0, 1), down the screen, kept.
        assert_eq!(aside(Vec2::new(1.0, 0.0)), Vec2::new(0.0, 1.0));
        // Left: its normal (0, −1) is up, so turned down.
        assert_eq!(aside(Vec2::new(-1.0, 0.0)), Vec2::new(0.0, 1.0));
        // Down, (0, 1): its normal (−1, 0) lies level, pointing left, so turned right.
        assert_eq!(aside(Vec2::new(0.0, 1.0)), Vec2::new(1.0, 0.0));
        // Up: its normal (1, 0), right, kept.
        assert_eq!(aside(Vec2::new(0.0, -1.0)), Vec2::new(1.0, 0.0));
        // None: straight down.
        assert_eq!(aside(Vec2::ZERO), Vec2::new(0.0, 1.0));
    }

    #[test]
    fn stretch_is_the_projections_larger_singular_value() {
        // The linear part [[9/5, −4/5], [12/5, 3/5]]: x stretched by 3, then turned by
        // (3/5, 4/5), so its singular values are 3 and 1.
        let project = |p: [f64; 2]| {
            Some(Pos2::new(
                (1.8 * p[0] - 0.8 * p[1]) as f32,
                (2.4 * p[0] + 0.6 * p[1]) as f32,
            ))
        };
        let s = stretch([0.0, 0.0], 2.0, &project);
        assert!((s - 3.0).abs() < 1e-5, "{s}");
    }

    #[test]
    fn cross_crossing_and_unit_by_hand() {
        // 2·11 − 3·5.
        assert_eq!(cross([2.0, 3.0], [5.0, 11.0]), 7.0);
        // The diagonal from (0, 0) meets x = 4 at (4, 4); parallel lines meet nowhere.
        assert_eq!(
            crossing([0.0, 0.0], [1.0, 1.0], [4.0, 0.0], [0.0, 1.0]),
            Some([4.0, 4.0])
        );
        assert_eq!(
            crossing([0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [2.0, 0.0]),
            None
        );
        assert_eq!(unit([3.0, 4.0]), Some([0.6, 0.8]));
        assert_eq!(unit([0.0, 0.0]), None);
        assert_eq!(unit([f64::INFINITY, 0.0]), None);
    }

    #[test]
    fn a_views_direction_is_none_where_the_projection_doesnt_move() {
        let still = |_: [f64; 2]| Some(Pos2::new(5.0, 5.0));
        let view = View {
            project: &still,
            scale: Scale(0.1),
            visible: None,
            chord_tolerance: 0.25,
        };
        assert_eq!(view.direction([0.0, 0.0], [1.0, 0.0]), None);
        // Ten points a millimetre, y up: +y in the sketch is up the screen.
        let up = |p: [f64; 2]| Some(Pos2::new((10.0 * p[0]) as f32, (-10.0 * p[1]) as f32));
        let view = View {
            project: &up,
            ..view
        };
        assert_eq!(
            view.direction([0.0, 0.0], [0.0, 1.0]),
            Some(Vec2::new(0.0, -1.0))
        );
    }

    #[test]
    fn a_views_direction_is_a_unit_vector_where_the_projection_stretches() {
        // A plane seen at an angle: x stretched to 20 points a millimetre, y 10. A step of
        // 0.1 mm along +x moves 2 points, (2, 0): a unit, (1, 0). Along (1, 1)/√2 it moves
        // (√2, 1/√2), √(5/2) long: a unit, (2, 1)/√5.
        let stretched = |p: [f64; 2]| Some(Pos2::new((20.0 * p[0]) as f32, (10.0 * p[1]) as f32));
        let view = View {
            project: &stretched,
            scale: Scale(0.1),
            visible: None,
            chord_tolerance: 0.25,
        };
        let close = |got: Option<Vec2>, want: [f32; 2]| {
            got.is_some_and(|d| (d.x - want[0]).abs() < 1e-6 && (d.y - want[1]).abs() < 1e-6)
        };
        let along = view.direction([0.0, 0.0], [1.0, 0.0]);
        assert!(close(along, [1.0, 0.0]), "{along:?}");
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let slant = view.direction([0.0, 0.0], [h, h]);
        let r5 = 5.0_f32.sqrt();
        assert!(close(slant, [2.0 / r5, 1.0 / r5]), "{slant:?}");
    }
}
