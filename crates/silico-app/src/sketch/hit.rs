//! Hit-testing a sketch in its own millimetres (the M5.11 plan, *The pointer in the
//! sketch*): what's under the pointer, from the newest `sketch` read alone, no GPU picking.
//! A point first, then a curve, then a relation's glyph or a dimension, each the nearest
//! within the tolerance, ties to the lower ID.

use std::f64::consts::TAU;

use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, ElementKind};

use super::draw::{
    Curve, Geometry, Scale, add, angle, direction, dot, foot, length, mul, placed, sub,
};
use super::style::{SKETCH_GLYPH_SIDE, SKETCH_LABEL_HEIGHT};

/// A member under the pointer: its ID and its kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hit {
    pub id: ElementId,
    pub kind: ElementKind,
}

/// What of `read` is at `at` (the sketch's millimetres), within `tolerance` millimetres: the
/// nearest point, or, with none that near, the nearest curve, or else the nearest relation
/// glyph or dimension (placed at `scale`, as they're drawn), a glyph or a dimension's value
/// taking the pointer anywhere within half its height. Ties go to the lower ID. None for
/// nothing that near, a tolerance below zero, or a place that isn't one.
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
    let curves = geometry
        .curves()
        .map(|(id, curve, kind)| (distance(curve, at), id, kind))
        .collect();
    if let Some(hit) = near(curves) {
        return Some(hit);
    }
    let placed = placed(read, &geometry, scale);
    let glyph_half = scale.mm(SKETCH_GLYPH_SIDE) / 2.0;
    let text_half = scale.mm(SKETCH_LABEL_HEIGHT) / 2.0;
    let mut constraints: Vec<(f64, &ElementId, ElementKind)> = placed
        .glyphs
        .iter()
        .map(|g| {
            let d = (length(sub(g.at, at)) - glyph_half).max(0.0);
            (d, &g.of, ElementKind::Constraint)
        })
        .collect();
    for dimension in &placed.dimensions {
        let text = (length(sub(dimension.text_at, at)) - text_half).max(0.0);
        let lines = dimension
            .lines
            .iter()
            .flat_map(|line| line.windows(2))
            .map(|pair| segment_distance(at, pair[0], pair[1]));
        let d = lines.fold(text, f64::min);
        constraints.push((d, &dimension.of, ElementKind::Constraint));
    }
    near(constraints)
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
