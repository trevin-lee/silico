//! The ground grid (the approved prototype's `buildGrid`, docs/design/m5-ui/js/viewport.js;
//! DESIGN.md's *Viewport*): a line every 10 mm across the XY plane out to 200 mm, each 50 mm's
//! stronger, faded out from the origin to 200 mm, and the three axes, X and Y across it and Z
//! up from the origin, each in its colour.
//!
//! The viewport pane paints it in egui under the renderer's frame, between the canvas's
//! gradient and the frame, each line projected by the renderer's own projection
//! ([`SegmentProjection`], cut at the eye in perspective as the drawing cuts its edges). So it's
//! never in the picking layer, and never over a body. **A deliberate difference from the
//! prototype** (Main, 2026-11-09): there the grid is drawn in the scene, depth-tested, so its
//! lines show over a body's part below z = 0, and over its bottom seen from below; here a body
//! hides the grid wherever the body is drawn, so the grid never covers a body or what picks it.
//! The prototype's offsets below the plane (its lines at z = −0.05, its axes at −0.04), which
//! kept its depth-tested lines off the origin planes, have nothing to do here: all of it is at
//! z = 0.
//!
//! It's hidden while a sketch is open, as the prototype hides it (sketch.js, as a sketch
//! opens), and shown or hidden by the navigation bar's Grid ([`crate::viewport::View::grid`]),
//! on at first.

use eframe::egui::epaint::{PathShape, PathStroke};
use eframe::egui::{Color32, Pos2, Rect, Shape, Vec2};
use silico_protocol::Camera;
use silico_render::render::SegmentProjection;

use crate::theme::{self, Rgba8};

/// Each line's segments' length, in millimetres (the prototype's 20): each end faded by its
/// own distance from the origin.
pub const SEGMENT_MM: f64 = 20.0;

/// The Z axis's length, up from the origin, in millimetres (the prototype's 40).
pub const Z_AXIS_MM: f64 = 40.0;

/// A stroke of the grid, in the document's coordinates: its two ends; its colour, and its
/// alpha at each end, out of 255 (faded, not rounded); and its width, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridStroke {
    pub ends: [[f64; 3]; 2],
    pub rgb: [u8; 3],
    pub alpha: [f64; 2],
    pub width: f32,
}

/// How much of a line's alpha is left `r` mm from the origin: all of it at the origin, none
/// from [`theme::VIEWPORT_GRID_FADE_MM`] on, linearly between (the prototype's
/// `max(0, 1 − r / R)`).
pub fn fade(r: f64) -> f64 {
    (1.0 - r / f64::from(theme::VIEWPORT_GRID_FADE_MM)).max(0.0)
}

/// The grid's strokes, in the order they're painted, the prototype's: for each place `v`,
/// every 10 mm from −200 to 200 but 0 (where the axes are), its two lines, segment by segment,
/// 20 mm each, side by side: x = v from (v, s) to (v, s + 20), then y = v from (s, v) to
/// (s + 20, v), for s from −200 to 180. Each 50 mm's lines are major
/// ([`theme::VIEWPORT_GRID_MAJOR`]), the rest minor ([`theme::VIEWPORT_GRID_MINOR`]),
/// [`theme::VIEWPORT_GRID_WIDTH`] wide, each end's alpha its colour's [`fade`]d by the end's
/// distance from the origin. Then the axes, unfaded, each in its colour at
/// [`theme::VIEWPORT_GRID_AXIS_ALPHA`], [`theme::VIEWPORT_GRID_AXIS_WIDTH`] wide: X from
/// (−200, 0, 0) to (200, 0, 0) ([`theme::AXIS_X`]), Y from (0, −200, 0) to (0, 200, 0)
/// ([`theme::AXIS_Y`]), and Z from the origin to (0, 0, 40) ([`theme::AXIS_Z`]).
pub fn strokes() -> Vec<GridStroke> {
    let reach = f64::from(theme::VIEWPORT_GRID_FADE_MM);
    let minor = f64::from(theme::VIEWPORT_GRID_MINOR_MM);
    let major = f64::from(theme::VIEWPORT_GRID_MAJOR_MM);
    let places = (reach / minor).round() as i64;
    let segments = (2.0 * reach / SEGMENT_MM).round() as i64;
    let faded = |ends: [[f64; 3]; 2], colour: Rgba8| GridStroke {
        ends,
        rgb: [colour[0], colour[1], colour[2]],
        alpha: ends.map(|[x, y, _]| f64::from(colour[3]) * fade((x * x + y * y).sqrt())),
        width: theme::VIEWPORT_GRID_WIDTH,
    };
    let mut strokes = Vec::new();
    for k in (-places..=places).filter(|k| *k != 0) {
        let v = k as f64 * minor;
        let colour = if (v / major).fract() == 0.0 {
            theme::VIEWPORT_GRID_MAJOR
        } else {
            theme::VIEWPORT_GRID_MINOR
        };
        for j in 0..segments {
            let s = -reach + j as f64 * SEGMENT_MM;
            strokes.push(faded([[v, s, 0.0], [v, s + SEGMENT_MM, 0.0]], colour));
            strokes.push(faded([[s, v, 0.0], [s + SEGMENT_MM, v, 0.0]], colour));
        }
    }
    let axis = |ends: [[f64; 3]; 2], colour: Rgba8| GridStroke {
        ends,
        rgb: [colour[0], colour[1], colour[2]],
        alpha: [f64::from(theme::VIEWPORT_GRID_AXIS_ALPHA); 2],
        width: theme::VIEWPORT_GRID_AXIS_WIDTH,
    };
    strokes.push(axis([[-reach, 0.0, 0.0], [reach, 0.0, 0.0]], theme::AXIS_X));
    strokes.push(axis([[0.0, -reach, 0.0], [0.0, reach, 0.0]], theme::AXIS_Y));
    strokes.push(axis([[0.0; 3], [0.0, 0.0, Z_AXIS_MM]], theme::AXIS_Z));
    strokes
}

/// The grid as a view from `camera` sees it over `rect`, the view's frame being `size_px` in
/// pixels, `pixels_per_point` of them a point: each of [`strokes`], in its order, a path
/// between its drawn part's ends ([`SegmentProjection::segment`]: in perspective, cut where
/// it's a nanometre in front of the eye), its colour running from one end's alpha to the
/// other's along the path on the screen, a cut end's alpha the stroke's own where it's cut.
/// On the screen, not along the line as the prototype's WebGL interpolates it: in
/// perspective the two part where a stroke's depth changes much along it, so in a view that
/// grazes the plane a stroke cut near the eye shows nearer its far end's alpha (a few parts
/// in a hundred of the line's 0.75 or 0.9). Accepted: egui blends a path's colours between
/// its points.
/// None of a stroke drawn wholly transparent (its alpha under a half at both ends: past
/// 200 mm), of one seen end on (its ends at one point: the Z axis from above), nor of one an
/// end of which lands nowhere finite; none at all for a view the renderer refuses, or a
/// `pixels_per_point` that isn't above 0.
pub fn shapes(camera: &Camera, size_px: [u32; 2], pixels_per_point: f32, rect: Rect) -> Vec<Shape> {
    let _span = tracing::debug_span!("ground_grid").entered();
    let ppp = f64::from(pixels_per_point);
    let Some(view) = SegmentProjection::new(camera, size_px).filter(|_| ppp > 0.0) else {
        return Vec::new();
    };
    let point = |[x, y]: [f64; 2]| rect.min + Vec2::new((x / ppp) as f32, (y / ppp) as f32);
    strokes()
        .into_iter()
        .filter_map(|stroke| {
            let drawn = view.segment(stroke.ends[0], stroke.ends[1])?;
            let [Some(first), Some(last)] = drawn.pixels else {
                return None;
            };
            let [a, b] = stroke.alpha;
            let alpha = drawn.along.map(|t| a + (b - a) * t);
            if alpha.iter().all(|end| *end < 0.5) {
                return None;
            }
            let ends = [point(first), point(last)];
            if ends[0] == ends[1] {
                return None;
            }
            let colour = along(ends, stroke.rgb, alpha);
            let stroke = PathStroke::new_uv(stroke.width, move |_, at| colour(at));
            Some(Shape::Path(PathShape::line(ends.to_vec(), stroke)))
        })
        .collect()
}

/// The colour at a point of a path from `ends[0]` to `ends[1]`, `rgb` at the alpha that runs
/// from `alpha[0]` to `alpha[1]` along it: of the point nearest along the path (one past an
/// end, as the stroke's cap and its smoothing are, that end's).
fn along(ends: [Pos2; 2], rgb: [u8; 3], alpha: [f64; 2]) -> impl Fn(Pos2) -> Color32 {
    move |at| {
        let d = ends[1] - ends[0];
        let length = d.length_sq();
        let t = if length > 0.0 {
            ((at - ends[0]).dot(d) / length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let alpha = alpha[0] + (alpha[1] - alpha[0]) * f64::from(t);
        let [r, g, b] = rgb;
        Color32::from_rgba_unmultiplied(r, g, b, alpha.round().clamp(0.0, 255.0) as u8)
    }
}

#[cfg(test)]
mod tests;
