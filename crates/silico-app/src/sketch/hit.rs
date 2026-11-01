//! Hit-testing a sketch in its own millimetres (the M5.11 plan, *The pointer in the
//! sketch*): what's under the pointer, from the newest `sketch` read alone, no GPU picking.
//! In the order they're drawn over one another: a point first, then a relation's glyph or a
//! dimension's value (each a box on the screen), then a curve, then a dimension's lines;
//! each the nearest within the tolerance, ties to the lower ID.

use std::f64::consts::TAU;

use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, ElementKind};

use super::draw::{
    Curve, Geometry, Scale, add, angle, direction, dot, foot, length, mul, placed, sub,
};
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

/// What of `read` is at `at` (the sketch's millimetres), within `tolerance` millimetres, in
/// the order they're drawn over one another: the nearest point; or else the nearest
/// relation glyph or dimension value, each a box (placed at `scale`, as they're drawn, the
/// box upright on the screen); or else the nearest curve; or else the nearest dimension's
/// line. Ties go to the lower ID. None for nothing that near, a tolerance below zero, or a
/// place that isn't one.
pub fn hit(read: &SketchRead, at: [f64; 2], tolerance: f64, scale: Scale) -> Option<Hit> {
    if !(at[0].is_finite() && at[1].is_finite()) || tolerance.is_nan() || tolerance < 0.0 {
        return None;
    }
    let geometry = Geometry::of(read);
    let near = |found: Vec<(f64, &ElementId, ElementKind)>| {
        found
            .into_iter()
            .filter(|(d, ..)| *d <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)))
            .map(|(_, id, kind)| Hit {
                id: id.clone(),
                kind,
            })
    };
    let points = geometry
        .points()
        .map(|(id, p)| (length(sub(p, at)), id, ElementKind::Point))
        .collect();
    if let Some(hit) = near(points) {
        return Some(hit);
    }
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
    let curves = geometry
        .curves()
        .map(|(id, curve, kind)| (distance(curve, at), id, kind))
        .collect();
    if let Some(hit) = near(curves) {
        return Some(hit);
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
    near(lines)
}

/// How far `at` is from the box about `centre` reaching `half` either way: none inside it.
fn box_distance(at: [f64; 2], centre: [f64; 2], half: [f64; 2]) -> f64 {
    let dx = ((at[0] - centre[0]).abs() - half[0]).max(0.0);
    let dy = ((at[1] - centre[1]).abs() - half[1]).max(0.0);
    libm::hypot(dx, dy)
}

/// How far `at` is from `curve`: from a line's segment, a circle's rim, or an arc's (or its
/// nearer end, past its sweep).
pub(crate) fn distance(curve: Curve, at: [f64; 2]) -> f64 {
    match curve {
        Curve::Line { from, to } => segment_distance(at, from, to),
        Curve::Round {
            centre,
            radius,
            start,
            sweep,
        } => {
            let off = sub(at, centre);
            let within = (angle(off) - start).rem_euclid(TAU) <= sweep;
            if sweep >= TAU || within {
                (length(off) - radius).abs()
            } else {
                let end = |a: f64| add(centre, mul(direction(a), radius));
                length(sub(at, end(start))).min(length(sub(at, end(start + sweep))))
            }
        }
    }
}

/// How far `at` is from the segment from `a` to `b`.
fn segment_distance(at: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
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
