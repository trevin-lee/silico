//! Hit-testing a sketch in its own millimetres (the M5.11 plan, *The pointer in the
//! sketch*): what's under the pointer, from the newest `sketch` read alone, no GPU picking.
//! A point or the origin first, the nearer, ranked as Smart Dimension ranks its picks by
//! the core's one rule (`sketch_tools::pick`; the constraint tools' C9); then a relation's
//! glyph or a dimension's value, each a box on the screen, as they're drawn over the
//! curves; then a curve of the sketch; then a dimension's lines; then an axis, which runs
//! the whole plane, so a dimension's extension line along one still takes the click. Each
//! the nearest within the tolerance; a curve against an axis by the core's rule too.

use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, ElementKind, Reference};
use silico_core::sketch_tools;

use super::draw::{Geometry, Scale, dot, foot, length, placed, sub};
use super::style::{CANVAS_TEXT_SIZE, SKETCH_DIMENSION_LABEL_PADDING, SKETCH_GLYPH_SIZE};

/// A dimension's value's box's height, as it's drawn.
const LABEL_HEIGHT: f32 = 18.0;

/// A dimension's value's character's width, as a share of the text's size: the box a value
/// is hit in is worked out from its length, as the prototype sizes it (6.6 points a
/// character at 11.5).
const CHARACTER_WIDTH: f32 = 0.574;

/// A member under the pointer: its ID and its kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hit {
    pub id: ElementId,
    pub kind: ElementKind,
}

/// What a click picks in sketch mode, and what its selection holds: a member of the sketch,
/// or the origin or one of its axes (the constraint tools' C9). In [`Reference`]'s order:
/// the members by ID, then the origin, the x axis, and the y axis.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SketchPick {
    Member(Hit),
    Origin,
    XAxis,
    YAxis,
}

impl SketchPick {
    /// What it is as a constraint names it.
    pub fn reference(&self) -> Reference {
        match self {
            SketchPick::Member(hit) => Reference::Element(hit.id.clone()),
            SketchPick::Origin => Reference::Origin,
            SketchPick::XAxis => Reference::XAxis,
            SketchPick::YAxis => Reference::YAxis,
        }
    }

    /// Whether a read whose members `holds` says hold it has it: a member it holds, or the
    /// origin or an axis, every sketch's. The one rule for what a read keeps selected.
    pub fn held_by(&self, holds: impl Fn(&ElementId) -> bool) -> bool {
        self.member().is_none_or(|hit| holds(&hit.id))
    }

    /// The member it is, if it's one.
    pub fn member(&self) -> Option<&Hit> {
        match self {
            SketchPick::Member(hit) => Some(hit),
            SketchPick::Origin | SketchPick::XAxis | SketchPick::YAxis => None,
        }
    }

    /// What `reference` names in `read`: a member, with its kind, if the read holds it; the
    /// origin; or an axis.
    fn of(read: &SketchRead, reference: Reference) -> Option<SketchPick> {
        match reference {
            Reference::Element(id) => {
                let kind = read
                    .members
                    .elements()
                    .find(|(held, _)| **held == id)
                    .map(|(_, kind)| kind)?;
                Some(SketchPick::Member(Hit { id, kind }))
            }
            Reference::Origin => Some(SketchPick::Origin),
            Reference::XAxis => Some(SketchPick::XAxis),
            Reference::YAxis => Some(SketchPick::YAxis),
        }
    }
}

impl From<Hit> for SketchPick {
    fn from(hit: Hit) -> Self {
        SketchPick::Member(hit)
    }
}

/// What of `read` is at `at` (the sketch's millimetres), within `tolerance` millimetres: the
/// nearest point, or the origin where it's nearer (the point on a tie, and a point at its
/// place always), by the core's `sketch_tools::pick`, as Smart Dimension picks; or else the
/// nearest relation glyph or dimension value, each a box (placed at `scale`, as they're
/// drawn, the box upright on the screen); or else the nearest curve of the sketch, by the
/// core's rule; or else the nearest dimension's line; or else an axis, anywhere along it
/// (where the core's rule takes it: not where a curve of the sketch is as near, nor where a
/// line of the sketch lies along it). Other ties go to the lower ID. None for nothing that
/// near, a tolerance below zero, or a place that isn't one.
pub fn hit(read: &SketchRead, at: [f64; 2], tolerance: f64, scale: Scale) -> Option<SketchPick> {
    if !(at[0].is_finite() && at[1].is_finite()) || tolerance.is_nan() || tolerance < 0.0 {
        return None;
    }
    let geometry = Geometry::of(read);
    let near = |found: Vec<(f64, &ElementId, ElementKind)>| {
        found
            .into_iter()
            .filter(|(d, ..)| *d <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)))
            .map(|(_, id, kind)| {
                SketchPick::Member(Hit {
                    id: id.clone(),
                    kind,
                })
            })
    };
    // A point or the origin, if one is near, else a curve or an axis, if one is: the curve
    // after the boxes, the axis after the dimensions' lines too.
    let picked = sketch_tools::pick(&read.members, at, tolerance)
        .and_then(|reference| SketchPick::of(read, reference));
    let (curve, axis) = match picked {
        Some(SketchPick::Origin) => return Some(SketchPick::Origin),
        Some(SketchPick::Member(hit)) if hit.kind == ElementKind::Point => {
            return Some(SketchPick::Member(hit));
        }
        Some(axis @ (SketchPick::XAxis | SketchPick::YAxis)) => (None, Some(axis)),
        curve => (curve, None),
    };
    let placed = placed(read, &geometry, scale);
    let glyph = scale.mm(SKETCH_GLYPH_SIZE) / 2.0;
    let mut boxes: Vec<(f64, &ElementId, ElementKind)> = placed
        .glyphs
        .iter()
        .map(|g| {
            (
                box_distance(at, g.at, [glyph, glyph]),
                &g.of,
                ElementKind::Constraint,
            )
        })
        .collect();
    let pad = f32::from(SKETCH_DIMENSION_LABEL_PADDING.left);
    for dimension in &placed.dimensions {
        let wide = dimension.text.chars().count() as f32 * CANVAS_TEXT_SIZE * CHARACTER_WIDTH;
        let half = [scale.mm(wide / 2.0 + pad), scale.mm(LABEL_HEIGHT / 2.0)];
        boxes.push((
            box_distance(at, dimension.text_at, half),
            &dimension.of,
            ElementKind::Constraint,
        ));
    }
    if let Some(hit) = near(boxes) {
        return Some(hit);
    }
    if curve.is_some() {
        return curve;
    }
    let lines = placed
        .dimensions
        .iter()
        .map(|dimension| {
            let d = dimension
                .lines
                .iter()
                .flat_map(|line| line.windows(2))
                .map(|pair| segment_distance(at, pair[0], pair[1]))
                .fold(f64::INFINITY, f64::min);
            (d, &dimension.of, ElementKind::Constraint)
        })
        .collect();
    near(lines).or(axis)
}

/// How far `at` is from the box about `centre` reaching `half` either way: none inside it.
fn box_distance(at: [f64; 2], centre: [f64; 2], half: [f64; 2]) -> f64 {
    let dx = ((at[0] - centre[0]).abs() - half[0]).max(0.0);
    let dy = ((at[1] - centre[1]).abs() - half[1]).max(0.0);
    libm::hypot(dx, dy)
}

/// How far `at` is from the segment from `a` to `b`.
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
