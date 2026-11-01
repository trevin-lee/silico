//! What the core sends the render worker, [`Op::Render`](crate::Op::Render)'s scene, and the
//! checks both sides run on it (docs/design/m4.6-render.md, The protocol). The worker draws
//! it and answers [`Reply::Rendered`](crate::Reply::Rendered): a PNG, and the ID buffer when
//! asked. Neither is a result: nothing the render worker returns reaches a result, a name, a
//! document, the cache, or a stored expected value (R5).

use std::ffi::OsString;

use serde::{Deserialize, Serialize};

use crate::classify::point_beyond;
use crate::vec3::{cross, dot, sub};
use crate::{InvalidParameter, Mesh};

/// Which adapter the render worker may draw on: the core's one say about the renderer, given
/// to the worker as its arguments (docs/design/m4.6-render.md, *How the worker gets its
/// rasterizer*), written by [`RenderAdapter::args`] and read by [`RenderAdapter::from_args`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RenderAdapter {
    /// Only a CPU adapter (Mesa's lavapipe), or the render is `worker_unavailable`.
    #[default]
    Software,
    /// A CPU adapter if there is one, then a discrete GPU, then any.
    Any,
}

/// The render worker's argument for [`RenderAdapter::Any`]; none is [`RenderAdapter::Software`].
const ANY_ADAPTER: &str = "--any-adapter";

impl RenderAdapter {
    /// The render worker's arguments for this policy.
    pub fn args(self) -> &'static [&'static str] {
        match self {
            RenderAdapter::Software => &[],
            RenderAdapter::Any => &[ANY_ADAPTER],
        }
    }

    /// The policy the render worker's arguments give, as [`RenderAdapter::args`] writes
    /// them. Any other argument, or one given twice, is refused, naming it: a worker never
    /// draws under a policy it wasn't given.
    pub fn from_args(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut policy = RenderAdapter::Software;
        for arg in args {
            match policy {
                RenderAdapter::Software if arg == ANY_ADAPTER => policy = RenderAdapter::Any,
                _ => return Err(format!("unexpected argument {arg:?}")),
            }
        }
        Ok(policy)
    }
}

/// The most pixels along either side of a render (M4.1's `render_side`).
pub const MAX_RENDER_SIDE: u32 = 4096;

/// The bit an edge's number carries in the ID buffer, to tell it from a face's: the second
/// of a pixel's pair is a face's number from 1, or an edge's from 1 with this bit set.
pub const EDGE_BIT: u32 = 0x8000_0000;

/// The least an up vector's part across the view may be, against its length: below it, the
/// image's rotation about the view is lost in rounding.
pub const MIN_UP_ACROSS_VIEW: f64 = 1e-9;

/// The farthest from the origin, in mm, any point of a scene may be (the eye, the target,
/// a position), and the tallest an orthographic view may be: what f32 draws with room to
/// spare, a million times a document's largest length.
pub const MAX_RENDER_COORDINATE: f64 = 1e12;

/// The least an orthographic view's height may be, in mm: a nanometre.
pub const MIN_VIEW_HEIGHT: f64 = 1e-6;

/// The least the eye may be from the target, in mm: a nanometre. Nearer, a perspective
/// view's projection is degenerate. And how deep along a perspective view a point must be
/// to be drawn: what's within a nanometre of the eye (the document's resolution) is cut
/// away, its near plane half that.
pub const MIN_EYE_DISTANCE: f64 = 1e-6;

/// The most faces a scene may have, across its bodies. Each is drawn as a mesh of its own
/// (its own ID in the ID buffer), at about 75 KiB of the worker's memory each: 100,000
/// held 7.5 GiB, and a million took past the 30 s a render has. 20,000 faces hold about
/// 1.4 GiB themselves, and peaked at 3.4 GiB drawn at 4,096² with the ID buffer: far past
/// a real part's hundreds to thousands.
pub const MAX_RENDER_FACES: usize = 20_000;

/// The most triangles a scene may have, across its bodies: 4 million drew in 12.5 s (a
/// release build) but 34 s in a debug one, past the 30 s a render has; NIST's CTC 02 has
/// 54,126 at its floor.
pub const MAX_RENDER_TRIANGLES: usize = 2_000_000;

/// The most edges a scene may have, across its bodies: a million held 0.6 GiB and drew in
/// 6.5 s (a debug build); 200,000 leaves room beside the faces.
pub const MAX_RENDER_EDGES: usize = 200_000;

/// How much f32 may be off at a point drawn, of the point's distance from where it's drawn
/// about (the camera's point, or a face's centre): f32's step, at most 2^-23 of it, twice,
/// for the transform's own rounding. 2^-22.
pub const F32_ROUNDING: f64 = 1.0 / 4_194_304.0;

/// The most of a pixel that f32's rounding may move what's drawn: an eighth. Every limit a
/// view has against its image's size is this, with [`F32_ROUNDING`] (docs/design/m4.6-render.md,
/// *A given camera*): an orthographic view's least height against the bodies' reach
/// ([`least_view_height`]), a perspective view's angles ([`field_of_view_range`]), and the
/// pyramid a perspective view's faces and edges are cut to as they're drawn (no farther
/// across the view than `2^20 t / height` times their depth); and the point each face is
/// drawn about.
pub const PRECISION_OF_PIXEL: f64 = 0.125;

/// The least an orthographic view `height` pixels high may be, in mm, of bodies that reach
/// `reach` from its target: f32 resolves a point to [`F32_ROUNDING`] of its distance from the
/// target, which must be at most [`PRECISION_OF_PIXEL`] of a pixel, so `height × reach /
/// 2^19`. The depth (the camera sits the reach behind the target, so the depth buffer spans
/// twice it) and the rasterizer (no point lands more than 2^19 pixels off the image's centre)
/// are inside the same bound.
pub fn least_view_height(height: u32, reach: f64) -> f64 {
    f64::from(height) * reach * F32_ROUNDING / PRECISION_OF_PIXEL
}

/// Whether a perspective view at `field_of_view` degrees draws to [`PRECISION_OF_PIXEL`] in an
/// image `width` by `height`: a point's error, [`F32_ROUNDING`] of its distance from the eye,
/// grows to its corner by the corner ray's sec², `c = 1 + t²(1 + aspect²)` with `t` the tangent
/// of half the angle, while a pixel at its depth is `2t / height` of it: so `height × c ≤ 2^20 ×
/// t`. A quadratic in `t`, so the view's angle has both ends ([`field_of_view_range`]); NaN
/// draws nothing.
fn angle_draws(field_of_view: f64, width: u32, height: u32) -> bool {
    let tan = libm::tan(field_of_view.to_radians() / 2.0);
    let aspect = f64::from(width) / f64::from(height);
    let corner = 1.0 + tan * tan * (1.0 + aspect * aspect);
    F32_ROUNDING * corner * f64::from(height) <= PRECISION_OF_PIXEL * 2.0 * tan
}

/// The narrowest and widest a perspective view's angle may be, in degrees, in an image `width`
/// by `height` pixels (each from 1): the roots of [`angle_draws`]'s quadratic, `height (1 +
/// a²) t² − 2^20 t + height = 0`, each the angle at its end that the check takes, and every
/// angle between them taken. At 4096 by 4096, 0.4476° to 179.105°; at 1920 by 1080, 0.118°
/// to 179.509°; the narrower the image, the wider both. Every image up to 4096 pixels on a
/// side (and far past) has some: the quadratic's roots are real while `height √(1 + a²)`,
/// the image's diagonal, is under 2^19 pixels.
pub fn field_of_view_range(width: u32, height: u32) -> (f64, f64) {
    let h = f64::from(height.max(1));
    let aspect = f64::from(width.max(1)) / h;
    let k = 1.0 + aspect * aspect;
    // 2^20, the bound's factor: F32_ROUNDING's 2^22 times an eighth times 2.
    let b = 2.0 * PRECISION_OF_PIXEL / F32_ROUNDING;
    let root = (b * b - 4.0 * h * h * k).max(0.0).sqrt();
    // The smaller root as c / (a x_large), so it keeps its digits.
    let large = (b + root) / (2.0 * h * k);
    let small = 1.0 / (k * large);
    let degrees = |t: f64| 2.0 * libm::atan(t).to_degrees();
    let at = |angle: f64| angle_draws(angle, width, height);
    // Each end moved to the last angle the check takes, as it rounds: f64 steps from the
    // root found in f64, a few at most.
    let mut low = degrees(small);
    for _ in 0..64 {
        match (at(low), at(low.next_down())) {
            (true, false) => break,
            (true, true) => low = low.next_down(),
            (false, _) => low = low.next_up(),
        }
    }
    let mut high = degrees(large);
    for _ in 0..64 {
        match (at(high), at(high.next_up())) {
            (true, false) => break,
            (true, true) => high = high.next_up(),
            (false, _) => high = high.next_down(),
        }
    }
    (low, high)
}

/// `x` given to at most six decimals, the nearest such value on the side of `x` that `up`
/// says (at least `x` if `up`, else at most it): a bound a message gives, which a request
/// can send back as written (a result writes lengths to the nanometre and angles past it) and
/// still be within it.
fn to_six_decimals(x: f64, up: bool) -> f64 {
    let scaled = x * 1e6;
    let shown = if up { scaled.ceil() } else { scaled.floor() } / 1e6;
    match up {
        true if shown < x => x.next_up(),
        false if shown > x => x.next_down(),
        _ => shown,
    }
}

/// A scene for the render worker: the bodies in view, the camera, and the image's size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderScene {
    /// One per solid drawn, in the model's order.
    pub bodies: Vec<RenderBody>,
    pub camera: Camera,
    /// The image's size in pixels, each from 1 to [`MAX_RENDER_SIDE`].
    pub width: u32,
    pub height: u32,
    /// Whether to read the ID buffer back (tests, and the app's picking later).
    pub ids: bool,
}

/// A body drawn: its mesh, and which of its faces and edges are highlighted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderBody {
    /// The model's body number, from 1 (its place among the model's bodies), which the ID
    /// buffer writes, so a body left out of the view never renumbers the others.
    pub body: u32,
    /// The kernel's display mesh, as `Op::Tessellate` gives it.
    pub mesh: Mesh,
    /// Faces by index from 0, ascending, each once.
    pub highlight_faces: Vec<u32>,
    /// Edges by index from 0, ascending, each once.
    pub highlight_edges: Vec<u32>,
}

/// Where the image is seen from, in the document's coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub projection: Projection,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    /// Which way is up in the image: any vector with a part across the view.
    pub up: [f64; 3],
}

/// How the view is projected.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Projection {
    /// Parallel rays; the view `view_height` high, in the document's units.
    Orthographic { view_height: f64 },
    /// From the eye; the view's vertical angle, in degrees as the document's angles are.
    Perspective { field_of_view: f64 },
}

/// A pixel of the ID buffer: the body's number from 1 and its element's (a face's from 1,
/// or an edge's from 1 with [`EDGE_BIT`]), or `[0, 0]` for the background.
pub type PixelId = [u32; 2];

fn invalid(parameter: &'static str, message: String) -> InvalidParameter {
    InvalidParameter { parameter, message }
}

fn finite3(v: [f64; 3]) -> bool {
    v.iter().all(|c| c.is_finite())
}

/// Finite and above 0.
fn positive(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

/// The camera's frame, unit vectors in f64: from the target toward the eye (`back`), up in
/// the image (`up`, the given up made perpendicular to the view), and to the image's right
/// (`right`, `up × back`); and the eye's distance from the target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub back: [f64; 3],
    pub up: [f64; 3],
    pub right: [f64; 3],
    pub distance: f64,
}

/// `v`'s length and direction, scaled first by its largest part, so neither overflows nor
/// underflows for any finite `v`; none for zero.
fn length_and_unit(v: [f64; 3]) -> Option<(f64, [f64; 3])> {
    let largest = v.iter().fold(0.0f64, |m, c| m.max(c.abs()));
    if !positive(largest) {
        return None;
    }
    let w = v.map(|c| c / largest);
    let length = dot(w, w).sqrt();
    Some((largest * length, w.map(|c| c / length)))
}

impl Camera {
    /// Whether the camera's numbers can make a view of an image `width` by `height` pixels
    /// (each from 1), named as M4.1's request names them, and its [`Frame`] if so:
    /// - the eye and the target finite and within [`MAX_RENDER_COORDINATE`], and apart;
    /// - the up finite, with a part across the view of at least [`MIN_UP_ACROSS_VIEW`] of
    ///   its length (its length is otherwise free: only its direction counts);
    /// - an orthographic view's height from [`MIN_VIEW_HEIGHT`] to [`MAX_RENDER_COORDINATE`]
    ///   (its least against the bodies it draws is the scene's: [`least_view_height`]);
    /// - a perspective view's angle within [`field_of_view_range`] at this size, where f32
    ///   draws it to [`PRECISION_OF_PIXEL`].
    ///
    /// The bounds are what a renderer drawing in f32 can draw, well past any document's
    /// lengths (1e6 mm at most). One computation for the check and the renderer, so they
    /// can't disagree.
    pub fn check(&self, width: u32, height: u32) -> Result<Frame, InvalidParameter> {
        let frame = self.frame()?;
        if let Projection::Perspective { field_of_view } = self.projection
            && !angle_draws(field_of_view, width, height)
        {
            let (low, high) = field_of_view_range(width, height);
            return Err(invalid(
                "camera.field_of_view",
                format!(
                    "camera.field_of_view {field_of_view:?} must be from {} to {} degrees for an image {width} by {height} pixels, where f32 draws the view to an eighth of a pixel",
                    to_six_decimals(low, true),
                    to_six_decimals(high, false),
                ),
            ));
        }
        Ok(frame)
    }

    /// The camera's [`Frame`], if its numbers make one whatever the image's size: all of
    /// [`Camera::check`] but a perspective angle's range at a size, which here need only be
    /// above 0 and under 180°. For what's computed of a camera that passed its check.
    pub fn frame(&self) -> Result<Frame, InvalidParameter> {
        for (parameter, v) in [("camera.eye", self.eye), ("camera.target", self.target)] {
            if let Some(message) = point_beyond(parameter, v, MAX_RENDER_COORDINATE) {
                return Err(invalid(parameter, message));
            }
        }
        if !finite3(self.up) {
            return Err(invalid(
                "camera.up",
                format!("camera.up {:?} isn't finite", self.up),
            ));
        }
        let apart = length_and_unit(sub(self.eye, self.target))
            .filter(|(distance, _)| *distance >= MIN_EYE_DISTANCE);
        let Some((distance, back)) = apart else {
            return Err(invalid(
                "camera.eye",
                format!(
                    "camera.eye {:?} must be at least {MIN_EYE_DISTANCE} mm from camera.target {:?}",
                    self.eye, self.target
                ),
            ));
        };
        // The up's part across the view, of its length: |û × back|, which keeps its
        // precision where the up is nearly along the view.
        let across = length_and_unit(self.up).and_then(|(_, up)| {
            let c = cross(up, back);
            let across = dot(c, c).sqrt();
            (across >= MIN_UP_ACROSS_VIEW).then_some(up)
        });
        let Some(up) = across else {
            return Err(invalid(
                "camera.up",
                format!(
                    "camera.up {:?} must have a part across the view, from camera.eye to camera.target, of at least {MIN_UP_ACROSS_VIEW} of its length",
                    self.up
                ),
            ));
        };
        // Made perpendicular to the view: its length is the part across, at least
        // MIN_UP_ACROSS_VIEW, so it divides safely.
        let along = dot(up, back);
        let perpendicular = [
            up[0] - along * back[0],
            up[1] - along * back[1],
            up[2] - along * back[2],
        ];
        let length = dot(perpendicular, perpendicular).sqrt();
        let up = perpendicular.map(|c| c / length);
        match self.projection {
            Projection::Orthographic { view_height } => {
                if !(MIN_VIEW_HEIGHT..=MAX_RENDER_COORDINATE).contains(&view_height) {
                    return Err(invalid(
                        "camera.view_height",
                        format!(
                            "camera.view_height {view_height:?} must be from {MIN_VIEW_HEIGHT} to {MAX_RENDER_COORDINATE} mm"
                        ),
                    ));
                }
            }
            Projection::Perspective { field_of_view } => {
                if !(field_of_view > 0.0 && field_of_view < 180.0) {
                    return Err(invalid(
                        "camera.field_of_view",
                        format!(
                            "camera.field_of_view {field_of_view:?} must be above 0 and under 180 degrees, and within the range the image's size allows"
                        ),
                    ));
                }
            }
        }
        Ok(Frame {
            back,
            up,
            right: cross(up, back),
            distance,
        })
    }
}

impl RenderScene {
    /// Whether the scene can be drawn without the worker trusting anything about it: the
    /// sizes, the camera ([`Camera::check`]), the bodies' numbers (at least 1, strictly
    /// ascending), each mesh's structure (positions and normals finite, within
    /// [`MAX_RENDER_COORDINATE`], and one each, every index in range, the faces' and edges'
    /// counts adding up, every number fitting the ID buffer), the highlights (ascending, each
    /// once, in range), the faces, edges, and triangles across the bodies (at most
    /// [`MAX_RENDER_FACES`], [`MAX_RENDER_EDGES`], and [`MAX_RENDER_TRIANGLES`]), and the
    /// bodies in the view at this size ([`check_bodies_in_view`]: an orthographic view's
    /// height against their reach). Only structure: what needs the model is the core's.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        for (parameter, side) in [("width", self.width), ("height", self.height)] {
            if !(1..=MAX_RENDER_SIDE).contains(&side) {
                return Err(invalid(
                    parameter,
                    format!("{parameter} {side} must be from 1 to {MAX_RENDER_SIDE} pixels"),
                ));
            }
        }
        self.camera.check(self.width, self.height)?;
        let mut last = 0;
        for (i, body) in self.bodies.iter().enumerate() {
            if body.body <= last {
                return Err(invalid(
                    "bodies",
                    format!(
                        "bodies[{i}]'s number {} must be at least 1 and above the one before it ({last})",
                        body.body
                    ),
                ));
            }
            last = body.body;
            body.check(i)?;
        }
        for (what, count, most) in [
            (
                "faces",
                self.bodies
                    .iter()
                    .map(|b| b.mesh.face_triangles.len())
                    .sum::<usize>(),
                MAX_RENDER_FACES,
            ),
            (
                "edges",
                self.bodies
                    .iter()
                    .map(|b| b.mesh.edge_point_counts.len())
                    .sum(),
                MAX_RENDER_EDGES,
            ),
            (
                "triangles",
                self.bodies.iter().map(|b| b.mesh.triangles.len()).sum(),
                MAX_RENDER_TRIANGLES,
            ),
        ] {
            if count > most {
                return Err(invalid(
                    "bodies",
                    format!(
                        "the bodies have {count} {what}, more than the {most} a render draws within its time"
                    ),
                ));
            }
        }
        let bodies = self.bodies.iter().map(|b| InViewBody::of(b.body, &b.mesh));
        let target = self.camera.target;
        let mut exact = |_: u32, mesh: &Mesh| reach_of_points(mesh, target);
        check_bodies_in_view(&self.camera, self.height, bodies, &mut exact).map(|_| ())
    }

    /// How far the scene reaches from `from`: its farthest point (a mesh's position or an
    /// edge's) from it, 0 for a scene with none.
    pub fn reach(&self, from: [f64; 3]) -> f64 {
        self.bodies
            .iter()
            .flat_map(|b| b.mesh.positions.iter().chain(&b.mesh.edge_points))
            .map(|p| distance(*p, from))
            .fold(0.0, f64::max)
    }
}

fn distance(p: [f64; 3], from: [f64; 3]) -> f64 {
    let d = sub(p, from);
    dot(d, d).sqrt()
}

/// A box of points: its least and its greatest corner.
pub type PointBox = ([f64; 3], [f64; 3]);

/// The box of `mesh`'s points, its positions and its edges' points; none for a mesh of
/// neither.
pub fn points_box(mesh: &Mesh) -> Option<PointBox> {
    let mut points = mesh.positions.iter().chain(&mesh.edge_points);
    let first = *points.next()?;
    Some(points.fold((first, first), |(low, high), p| {
        (
            std::array::from_fn(|k| low[k].min(p[k])),
            std::array::from_fn(|k| high[k].max(p[k])),
        )
    }))
}

/// How far `mesh`'s farthest point (a position or an edge's point) is from `from`: exact,
/// one pass over them; 0 for none.
pub fn reach_of_points(mesh: &Mesh, from: [f64; 3]) -> f64 {
    mesh.positions
        .iter()
        .chain(&mesh.edge_points)
        .map(|p| distance(*p, from))
        .fold(0.0, f64::max)
}

/// How far a box's farthest corner is from `from`: at least its every point's distance, as
/// the distance is convex and a box's farthest point from anywhere is a corner.
pub fn reach_of_box((low, high): PointBox, from: [f64; 3]) -> f64 {
    let d: [f64; 3] =
        std::array::from_fn(|k| (from[k] - low[k]).abs().max((high[k] - from[k]).abs()));
    dot(d, d).sqrt()
}

/// A body as [`check_bodies_in_view`] takes it: its number, its mesh, and its points' box
/// ([`points_box`]; none for a mesh with no points, which the rules don't see).
#[derive(Debug, Clone, Copy)]
pub struct InViewBody<'a> {
    pub body: u32,
    pub mesh: &'a Mesh,
    pub bounds: Option<PointBox>,
}

impl<'a> InViewBody<'a> {
    /// Body `body` of `mesh`, its box found from its points.
    pub fn of(body: u32, mesh: &'a Mesh) -> Self {
        InViewBody {
            body,
            mesh,
            bounds: points_box(mesh),
        }
    }
}

/// A view as [`check_bodies_in_view`] found its bodies, which the renderer's clipping is set
/// from: an orthographic view's height and how far the bodies reach from its target (at
/// most: a bound found from their boxes' corners where those pass); a perspective view's
/// angle, which needs nothing of them (what it draws is cut to its pyramid, *A given
/// camera*).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InView {
    Orthographic { view_height: f64, reach: f64 },
    Perspective { field_of_view: f64 },
}

/// Whether f32 draws `bodies` to [`PRECISION_OF_PIXEL`] in `camera`'s view, an image
/// `height` pixels high (docs/design/m4.6-render.md, *A given camera*): an orthographic
/// view's height at least [`least_view_height`] of the bodies' reach from its target, else
/// `camera.view_height`, naming the least. A perspective view takes any bodies: what lies
/// outside its pyramid (behind the eye's nanometre, or more than 2^19 pixels off its axis)
/// is cut away as it's drawn.
///
/// Each body is taken first by its box's farthest corner ([`reach_of_box`], a sufficient
/// bound); only a body whose corner the rule refuses is measured exactly, by `exact` (its
/// points' [`reach_of_points`]), and refused only if its points are. A refusal's reach is
/// the points', never a corner's. The scene's check ([`RenderScene::check`]), the worker's
/// view, and the app's frames all take the rule here.
pub fn check_bodies_in_view<'a>(
    camera: &Camera,
    height: u32,
    bodies: impl IntoIterator<Item = InViewBody<'a>>,
    exact: &mut dyn FnMut(u32, &Mesh) -> f64,
) -> Result<InView, InvalidParameter> {
    let view_height = match camera.projection {
        Projection::Perspective { field_of_view } => {
            return Ok(InView::Perspective { field_of_view });
        }
        Projection::Orthographic { view_height } => view_height,
    };
    let takes = |reach: f64| view_height >= least_view_height(height, reach);
    // How far the bodies the rule takes reach (by a corner where it passes), and the
    // farthest the rule refuses, by its points.
    let mut reach = 0.0f64;
    let mut farthest: Option<f64> = None;
    for body in bodies {
        let Some(bounds) = body.bounds else {
            continue;
        };
        let corner = reach_of_box(bounds, camera.target);
        let this = if takes(corner) {
            corner
        } else {
            let points = exact(body.body, body.mesh);
            if !takes(points) && farthest.is_none_or(|f| points > f) {
                farthest = Some(points);
            }
            points
        };
        reach = reach.max(this);
    }
    if let Some(reach) = farthest {
        let least = least_view_height(height, reach);
        return Err(invalid(
            "camera.view_height",
            format!(
                "camera.view_height {view_height:?} is too small for an image {height} pixels high of bodies that reach {reach:?} mm from camera.target: at least {} mm, where f32 draws them to an eighth of a pixel (the image's height times the reach over 2^19), or a camera.target nearer them",
                to_six_decimals(least, true)
            ),
        ));
    }
    Ok(InView::Orthographic { view_height, reach })
}

/// Whether `mesh` can be drawn without the renderer trusting anything about it: its
/// positions and normals finite, its positions within [`MAX_RENDER_COORDINATE`], one normal
/// each, every index in range, the faces' and edges' counts adding up, and every number
/// fitting the ID buffer. What [`RenderScene::check`] asks of each body's mesh, and what the
/// app's renderer asks of a mesh it uploads; the message says what isn't so.
pub fn check_render_mesh(mesh: &Mesh) -> Result<(), String> {
    if mesh.normals.len() != mesh.positions.len() {
        return Err(format!(
            "{} normals for {} positions",
            mesh.normals.len(),
            mesh.positions.len()
        ));
    }
    let within = |p: &[f64; 3]| p.iter().all(|c| c.abs() <= MAX_RENDER_COORDINATE);
    if let Some(p) = mesh.positions.iter().position(|p| !within(p)) {
        return Err(format!(
            "position {p} must be finite and within {MAX_RENDER_COORDINATE} mm of the origin"
        ));
    }
    if let Some(n) = mesh
        .normals
        .iter()
        .position(|n| n.iter().any(|c| !c.is_finite()))
    {
        return Err(format!("normal {n} isn't finite"));
    }
    let positions = mesh.positions.len();
    if let Some(t) = mesh.triangles.iter().position(|t| {
        t.iter()
            .any(|&v| usize::try_from(v).map_or(true, |v| v >= positions))
    }) {
        return Err(format!(
            "triangle {t} indexes past its {positions} positions"
        ));
    }
    let counted: u64 = mesh.face_triangles.iter().map(|&c| u64::from(c)).sum();
    if counted != mesh.triangles.len() as u64 {
        return Err(format!(
            "its faces count {counted} triangles, and it has {}",
            mesh.triangles.len()
        ));
    }
    if let Some(p) = mesh.edge_points.iter().position(|p| !within(p)) {
        return Err(format!(
            "edge point {p} must be finite and within {MAX_RENDER_COORDINATE} mm of the origin"
        ));
    }
    let counted: u64 = mesh.edge_point_counts.iter().map(|&c| u64::from(c)).sum();
    if counted != mesh.edge_points.len() as u64 {
        return Err(format!(
            "its edges count {counted} points, and it has {}",
            mesh.edge_points.len()
        ));
    }
    // Every face's and edge's number from 1 must fit under the edge bit.
    for (what, count) in [
        ("faces", mesh.face_triangles.len()),
        ("edges", mesh.edge_point_counts.len()),
    ] {
        if count as u64 >= u64::from(EDGE_BIT) {
            return Err(format!(
                "{count} {what} are more than the ID buffer numbers"
            ));
        }
    }
    Ok(())
}

impl RenderBody {
    fn check(&self, i: usize) -> Result<(), InvalidParameter> {
        let mesh = &self.mesh;
        check_render_mesh(mesh)
            .map_err(|what| invalid("bodies", format!("bodies[{i}]'s mesh: {what}")))?;
        for (what, highlights, count) in [
            (
                "highlight_faces",
                &self.highlight_faces,
                mesh.face_triangles.len(),
            ),
            (
                "highlight_edges",
                &self.highlight_edges,
                mesh.edge_point_counts.len(),
            ),
        ] {
            let mut before: Option<u32> = None;
            for &h in highlights {
                if before.is_some_and(|b| h <= b) || usize::try_from(h).map_or(true, |h| h >= count)
                {
                    return Err(invalid(
                        "bodies",
                        format!(
                            "bodies[{i}]'s {what} must be ascending, each once, and under {count}: {h} isn't"
                        ),
                    ));
                }
                before = Some(h);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::length_and_unit;

    #[test]
    fn a_zero_vector_has_no_length_or_direction_and_the_least_positive_one_has() {
        // Found by the M4 milestone's mutation run: taking 0 (or anything at all) as
        // positive passed every test, as the camera's checks refuse a zero vector after,
        // through the NaN it then gives. A direction is a vector over its length: a zero
        // vector, of either sign, has none.
        assert_eq!(length_and_unit([0.0; 3]), None);
        assert_eq!(length_and_unit([-0.0, 0.0, -0.0]), None);
        // By hand: the least positive double, 2^-1074, alone on x, is its own length, along
        // x; and (3, 4, 0) is 5 long, along (0.6, 0.8, 0): scaled by its largest part, 4, it's
        // (0.75, 1, 0), whose squares sum to 1.5625 and whose length is 1.25, all exact, so
        // 0.75 / 1.25 and 1 / 1.25 round to the doubles nearest 0.6 and 0.8.
        assert_eq!(
            length_and_unit([f64::from_bits(1), 0.0, 0.0]),
            Some((f64::from_bits(1), [1.0, 0.0, 0.0]))
        );
        assert_eq!(
            length_and_unit([3.0, 4.0, 0.0]),
            Some((5.0, [0.6, 0.8, 0.0]))
        );
    }
}
