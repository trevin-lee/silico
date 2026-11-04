//! The open sketch's plane on the viewport's screen, and back (the M5.11 plan, *The pointer
//! in the sketch*): through the pane's camera, by the projection the renderer draws with
//! ([`silico_render::render::ray`]), so what the overlay draws sits where the renderer draws
//! the model; or flat, for a sketch whose face broke ([`Flat`]).
//!
//! Places on the screen are in points, as egui's painter takes them; the renderer's image is
//! in physical pixels from the viewport's top left, `pixels_per_point` to a point, the place
//! a pixel's centre is half a pixel in, as the pane measures its events
//! ([`super::frame_events`]).

use eframe::egui::Pos2;
use silico_core::command::sketch::SketchRead;
use silico_core::kernel::SketchPlane;
use silico_protocol::vec3::{dot, sub};
use silico_protocol::{Camera, Projection};
use silico_render::render::ray;

use super::draw::Flat;
use crate::viewport::ViewFrame;

/// Where the open sketch is on the screen.
#[derive(Debug, Clone)]
pub enum Screen {
    /// Through the viewport's camera.
    Camera(Box<Placed>),
    /// Fitted into the viewport, with no frame in 3D behind it: a face sketch whose face
    /// broke (#347).
    Flat(Flat),
}

impl Screen {
    /// `read`'s sketch in the viewport `frame`: on its plane through the frame's camera, or,
    /// with its plane unread (a broken face), flat in the frame's rectangle. None for a
    /// camera or a size the renderer refuses, or a display scale that isn't one.
    pub fn of(read: &SketchRead, frame: &ViewFrame) -> Option<Screen> {
        match &read.plane {
            Ok(plane) => Placed::new(
                &frame.camera,
                frame.size_px,
                frame.pixels_per_point,
                frame.rect.left_top(),
                *plane,
            )
            .map(|placed| Screen::Camera(Box::new(placed))),
            Err(_) => Some(Screen::Flat(Flat::fit(frame.rect, read))),
        }
    }

    /// The sketch's place `at`, in millimetres, on the screen; none where it can't be shown.
    pub fn project(&self, at: [f64; 2]) -> Option<Pos2> {
        match self {
            Screen::Camera(placed) => placed.project(at),
            Screen::Flat(flat) => finite(at).then(|| flat.project(at)).flatten(),
        }
    }

    /// The screen's point `at` on the sketch's plane, in its millimetres; none where its ray
    /// misses the plane.
    pub fn to_plane(&self, at: Pos2) -> Option<[f64; 2]> {
        match self {
            Screen::Camera(placed) => placed.to_plane(at),
            Screen::Flat(flat) => flat.to_plane(at).filter(|p| finite(*p)),
        }
    }
}

/// A sketch's plane seen through a camera: its frame in model space, and the view's, with
/// the renderer's projection read off its own rays, once.
#[derive(Debug, Clone)]
pub struct Placed {
    camera: Camera,
    size_px: [u32; 2],
    pixels_per_point: f64,
    /// The viewport's top left, in points: where the image's (0, 0) is.
    origin: Pos2,
    plane: SketchPlane,
    forward: Forward,
}

/// The renderer's projection from model space to the image, as its rays give it: the place
/// in model space each of three of the image's corners' pixels stands for, on a plane square
/// to the view (`base` at pixel (0, 0); `across` and `down`, from it, at (width, 0) and
/// (0, height)), which every other pixel's is an affine mix of.
#[derive(Debug, Clone, Copy)]
enum Forward {
    /// Each ray from the eye, met with the plane a millimetre ahead of it along the view.
    Perspective {
        eye: [f64; 3],
        ahead: [f64; 3],
        corners: Corners,
    },
    /// Each ray's origin, on the plane through the target.
    Orthographic { corners: Corners },
}

#[derive(Debug, Clone, Copy)]
struct Corners {
    base: [f64; 3],
    across: [f64; 3],
    down: [f64; 3],
}

impl Corners {
    /// `p`, a place on the corners' plane (or off it along the view, which drops out), as a
    /// pixel: its parts along `across` and `down`, solved together (they're square to
    /// rounding), times the image's size.
    fn pixel(&self, p: [f64; 3], size: [f64; 2]) -> Option<[f64; 2]> {
        let r = sub(p, self.base);
        let (uu, uv, vv) = (
            dot(self.across, self.across),
            dot(self.across, self.down),
            dot(self.down, self.down),
        );
        let determinant = uu * vv - uv * uv;
        let (ru, rv) = (dot(self.across, r), dot(self.down, r));
        let a = (vv * ru - uv * rv) / determinant;
        let b = (uu * rv - uv * ru) / determinant;
        let pixel = [a * size[0], b * size[1]];
        finite(pixel).then_some(pixel)
    }
}

impl Placed {
    /// `plane` through `camera`, its image `size_px` in pixels at `pixels_per_point` to a
    /// point, its top left at `origin` on the screen. None for a camera or size the renderer
    /// refuses ([`ray`]'s none), or a scale that isn't finite and above 0.
    pub fn new(
        camera: &Camera,
        size_px: [u32; 2],
        pixels_per_point: f32,
        origin: Pos2,
        plane: SketchPlane,
    ) -> Option<Placed> {
        let pixels_per_point = f64::from(pixels_per_point);
        if !(pixels_per_point > 0.0 && pixels_per_point.is_finite()) {
            return None;
        }
        let [width, height] = size_px.map(f64::from);
        let rays = [[0.0, 0.0], [width, 0.0], [0.0, height]].map(|p| ray(camera, size_px, p));
        let [Some(base), Some(right), Some(below)] = rays else {
            return None;
        };
        let corners = |at: [[f64; 3]; 3]| Corners {
            base: at[0],
            across: sub(at[1], at[0]),
            down: sub(at[2], at[0]),
        };
        let forward = match camera.projection {
            Projection::Perspective { .. } => {
                let ahead = camera.check(size_px[0], size_px[1]).ok()?.back.map(|c| -c);
                // Each ray's direction, scaled to a millimetre ahead along the view.
                let unit = |(_, d): ([f64; 3], [f64; 3])| d.map(|c| c / dot(d, ahead));
                Forward::Perspective {
                    eye: camera.eye,
                    ahead,
                    corners: corners([unit(base), unit(right), unit(below)]),
                }
            }
            Projection::Orthographic { .. } => Forward::Orthographic {
                corners: corners([base.0, right.0, below.0]),
            },
        };
        Some(Placed {
            camera: camera.clone(),
            size_px,
            pixels_per_point,
            origin,
            plane,
            forward,
        })
    }

    /// The sketch's place `at` (millimetres along its x and y) as a pixel of the image, by
    /// the projection the renderer draws with: none behind the eye, or for a place that
    /// isn't one.
    pub fn pixel_of(&self, at: [f64; 2]) -> Option<[f64; 2]> {
        if !finite(at) {
            return None;
        }
        let (o, x, y) = (
            self.plane.origin(),
            self.plane.x_axis(),
            self.plane.y_axis(),
        );
        let p: [f64; 3] = std::array::from_fn(|k| o[k] + at[0] * x[k] + at[1] * y[k]);
        let size = self.size_px.map(f64::from);
        match self.forward {
            Forward::Perspective {
                eye,
                ahead,
                corners,
            } => {
                let d = sub(p, eye);
                let depth = dot(d, ahead);
                if !(depth > 0.0 && depth.is_finite()) {
                    return None;
                }
                corners.pixel(d.map(|c| c / depth), size)
            }
            Forward::Orthographic { corners } => corners.pixel(p, size),
        }
    }

    /// The pixel `pixel`'s ray met with the sketch's plane, in its millimetres: along the
    /// whole line for an orthographic view (which draws what's in front of its target too),
    /// ahead of the eye for a perspective one (behind it, nothing is drawn). None for a ray
    /// along the plane, one that meets it behind the eye, or a pixel the renderer has no ray
    /// for.
    pub fn on_plane(&self, pixel: [f64; 2]) -> Option<[f64; 2]> {
        let (origin, direction) = ray(&self.camera, self.size_px, pixel)?;
        let normal = self.plane.normal();
        let across = dot(direction, normal);
        let t = dot(sub(self.plane.origin(), origin), normal) / across;
        let ahead = matches!(self.forward, Forward::Orthographic { .. }) || t > 0.0;
        if across == 0.0 || !t.is_finite() || !ahead {
            return None;
        }
        let met: [f64; 3] = std::array::from_fn(|k| origin[k] + t * direction[k]);
        let d = sub(met, self.plane.origin());
        let at = [dot(d, self.plane.x_axis()), dot(d, self.plane.y_axis())];
        finite(at).then_some(at)
    }

    /// The sketch's place `at` on the screen, in points: [`pixel_of`](Self::pixel_of)'s
    /// pixel from the viewport's top left.
    pub fn project(&self, at: [f64; 2]) -> Option<Pos2> {
        let [x, y] = self.pixel_of(at)?;
        let ppp = self.pixels_per_point;
        let on = [
            f64::from(self.origin.x) + x / ppp,
            f64::from(self.origin.y) + y / ppp,
        ];
        let point = Pos2::new(on[0] as f32, on[1] as f32);
        (point.x.is_finite() && point.y.is_finite()).then_some(point)
    }

    /// The screen's point `at`, in points, on the sketch's plane: its pixel from the
    /// viewport's top left, as the pane measures its events, then
    /// [`on_plane`](Self::on_plane).
    pub fn to_plane(&self, at: Pos2) -> Option<[f64; 2]> {
        self.on_plane(self.pixel_at(at))
    }

    /// The screen's point `at`, in points, as a pixel of the image.
    pub fn pixel_at(&self, at: Pos2) -> [f64; 2] {
        let ppp = self.pixels_per_point;
        [
            (f64::from(at.x) - f64::from(self.origin.x)) * ppp,
            (f64::from(at.y) - f64::from(self.origin.y)) * ppp,
        ]
    }
}

/// Whether both of `p`'s parts are finite.
fn finite(p: [f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

#[cfg(test)]
mod tests;
