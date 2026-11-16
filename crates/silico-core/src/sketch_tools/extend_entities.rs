//! Extend Entities (docs/ux/solidworks.md, §4, *Trim, extend, split*; docs/design/m5.0-app.md,
//! section 9, *Extend*): hovering a line or an arc previews it extended to the next curve,
//! the half of it under the pointer picking the end; a click accepts, as one `sketch_extend`
//! of that end to `next_curve`. What's here is the hover's: which end ([`extend_end`]), and
//! the extension the click would make, drawn ([`extend_preview`]), found by the core's own
//! [`next_crossing`], the function the command extends by, so the preview and the edit
//! agree. Which curve is under the pointer is the viewport's pick, the one trim and split
//! take.

use std::f64::consts::TAU;

use super::geometry::{angle_of, dot, finite, length, sub};
use super::{Preview, PreviewArc, Xy};
use crate::Error;
use crate::document::sketch_edit::Geo;
use crate::document::{ElementId, Sketch};
use crate::extend::{End, next_crossing, no_next_curve};

/// The end of `sketch`'s line or arc `curve` that the pointer at `at` picks: the end of the
/// half under it. A line's start if `at`, put on its line, falls before its midpoint, else
/// its end (the midpoint itself picks the end). An arc's start if `at`'s angle about its
/// centre falls within its sweep before its mid-angle, else its end (the mid-angle picks
/// the end); an angle past the sweep (a pointer within the pick of an arc's end, just past
/// it) picks the nearer end by angle, the start where they're as near. None for a member
/// that isn't a line or an arc (a circle, a point, a constraint, or none of the sketch's),
/// a line of no length, a pointer at an arc's centre, which is on neither half, and a
/// place that isn't finite.
pub fn extend_end(sketch: &Sketch, curve: &ElementId, at: Xy) -> Option<End> {
    if !finite(at) {
        return None;
    }
    match Geo::of(sketch, curve)? {
        Geo::Line { from, to } => {
            let d = sub(to, from);
            let span = dot(d, d);
            if span <= 0.0 || !span.is_finite() {
                return None;
            }
            let t = dot(sub(at, from), d) / span;
            if !t.is_finite() {
                return None;
            }
            Some(if t < 0.5 { End::Start } else { End::End })
        }
        Geo::Arc {
            center,
            start_angle,
            end_angle,
            ..
        } => {
            let off = sub(at, center);
            if length(off) <= 0.0 {
                return None;
            }
            let sweep = end_angle - start_angle;
            let on = (angle_of(off) - start_angle).rem_euclid(TAU);
            Some(if on <= sweep {
                if on < sweep / 2.0 {
                    End::Start
                } else {
                    End::End
                }
            } else if on - sweep < TAU - on {
                End::End
            } else {
                End::Start
            })
        }
        Geo::Circle { .. } => None,
    }
}

/// `sketch`'s line or arc `curve` with its `end` extended to the next curve, as the click
/// would extend it (`sketch_extend` with `next_curve`, which finds the place by
/// [`next_crossing`] too): the extension drawn from the end to the place, a line's piece of
/// its line or an arc's of its circle (counter-clockwise, from the end on round, or from
/// the place on round to the start), the place as a point, and construction if the curve
/// is. Refused as `sketch_extend` refuses it: with its words for a curve that meets nothing
/// past the end, or as [`next_crossing`] refuses a member that isn't a line or an arc.
pub fn extend_preview(sketch: &Sketch, curve: &ElementId, end: End) -> Result<Preview, Error> {
    let crossing =
        next_crossing(sketch, curve, end)?.ok_or_else(|| no_next_curve(sketch, curve, end))?;
    let to = [crossing.at[0].mm(), crossing.at[1].mm()];
    let mut preview = Preview {
        points: vec![to],
        ..Preview::default()
    };
    match (Geo::of(sketch, curve), end) {
        (Some(Geo::Line { from, .. }), End::Start) => preview.lines.push([from, to]),
        (Some(Geo::Line { to: moving, .. }), End::End) => preview.lines.push([moving, to]),
        (Some(Geo::Arc { center, start, .. }), End::Start) => preview.arcs.push(PreviewArc {
            center,
            start: to,
            end: start,
        }),
        (Some(Geo::Arc { center, end, .. }), End::End) => preview.arcs.push(PreviewArc {
            center,
            start: end,
            end: to,
        }),
        // A circle, or none: `next_crossing` has refused them already.
        (Some(Geo::Circle { .. }) | None, _) => {}
    }
    preview.construction = sketch
        .lines
        .iter()
        .find(|l| &l.id == curve)
        .map(|l| l.construction)
        .or_else(|| {
            sketch
                .arcs
                .iter()
                .find(|a| &a.id == curve)
                .map(|a| a.construction)
        })
        .unwrap_or(false);
    Ok(preview)
}
