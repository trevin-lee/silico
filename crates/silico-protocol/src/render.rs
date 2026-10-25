//! What the core sends the render worker, [`Op::Render`](crate::Op::Render)'s scene, and the
//! checks both sides run on it (docs/design/m4.6-render.md, The protocol). The worker draws
//! it and answers [`Reply::Rendered`](crate::Reply::Rendered): a PNG, and the ID buffer when
//! asked. Neither is a result: nothing the render worker returns reaches a result, a name, a
//! document, the cache, or a stored expected value (R5).

use serde::{Deserialize, Serialize};

use crate::{InvalidParameter, Mesh};

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
/// view's near plane is 0 in f32 and its projection degenerate.
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

/// The least an orthographic view's height may be against how far the scene reaches from
/// the target: deeper, the scene's far points land past where f32 clips them exactly, and
/// pixels go missing (lavapipe drew a 40 mm face whole at 1e-4 of its reach, not at 1e-5).
pub const MIN_VIEW_HEIGHT_OF_REACH: f64 = 1e-5;

/// The narrowest a perspective view's angle may be, in degrees: narrower, what it sees lands
/// past where f32 clips it exactly (as [`MIN_VIEW_HEIGHT_OF_REACH`]; an orthographic view is
/// the one for so close a look).
pub const MIN_FIELD_OF_VIEW: f64 = 1e-3;
/// The widest a perspective view's angle may be, in degrees: wider, its half-angle in f32
/// rounds past a right angle, and the view is drawn turned half a turn.
pub const MAX_FIELD_OF_VIEW: f64 = 179.999;

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

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
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
    /// Whether the camera's numbers can make a view, named as M4.1's request names them:
    /// - the eye and the target finite and within [`MAX_RENDER_COORDINATE`], and apart;
    /// - the up finite, with a part across the view of at least [`MIN_UP_ACROSS_VIEW`] of
    ///   its length (its length is otherwise free: only its direction counts);
    /// - an orthographic view's height from [`MIN_VIEW_HEIGHT`] to [`MAX_RENDER_COORDINATE`];
    /// - a perspective view's angle from [`MIN_FIELD_OF_VIEW`] to [`MAX_FIELD_OF_VIEW`].
    ///
    /// The bounds are what a renderer drawing in f32 can draw, well past any document's
    /// lengths (1e6 mm at most).
    pub fn check(&self) -> Result<(), InvalidParameter> {
        self.frame().map(|_| ())
    }

    /// The camera's [`Frame`], if it passes [`Camera::check`]: one computation for the check
    /// and the renderer, so they can't disagree.
    pub fn frame(&self) -> Result<Frame, InvalidParameter> {
        for (parameter, v) in [("camera.eye", self.eye), ("camera.target", self.target)] {
            if !v.iter().all(|c| c.abs() <= MAX_RENDER_COORDINATE) {
                return Err(invalid(
                    parameter,
                    format!(
                        "{parameter} {v:?} must be finite and within {MAX_RENDER_COORDINATE} mm of the origin"
                    ),
                ));
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
                if !(MIN_FIELD_OF_VIEW..=MAX_FIELD_OF_VIEW).contains(&field_of_view) {
                    return Err(invalid(
                        "camera.field_of_view",
                        format!(
                            "camera.field_of_view {field_of_view:?} must be from {MIN_FIELD_OF_VIEW} to {MAX_FIELD_OF_VIEW} degrees"
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
    /// [`MAX_RENDER_FACES`], [`MAX_RENDER_EDGES`], and [`MAX_RENDER_TRIANGLES`]), and an orthographic view's height against the bodies'
    /// reach ([`MIN_VIEW_HEIGHT_OF_REACH`]). Only structure: what needs the model is the
    /// core's.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        for (parameter, side) in [("width", self.width), ("height", self.height)] {
            if !(1..=MAX_RENDER_SIDE).contains(&side) {
                return Err(invalid(
                    parameter,
                    format!("{parameter} {side} must be from 1 to {MAX_RENDER_SIDE} pixels"),
                ));
            }
        }
        self.camera.check()?;
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
        if let Projection::Orthographic { view_height } = self.camera.projection {
            let reach = self.reach(self.camera.target);
            let least = reach * MIN_VIEW_HEIGHT_OF_REACH;
            if view_height < least {
                return Err(invalid(
                    "camera.view_height",
                    format!(
                        "camera.view_height {view_height:?} is too small to draw the bodies in view, which reach {reach:?} mm from camera.target: at least {least:?}, or a camera.target nearer them"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// How far the scene reaches from `from`: its farthest point (a mesh's position or an
    /// edge's) from it, 0 for a scene with none. The check bounds an orthographic view's
    /// zoom by it, and the renderer sets its clipping from it.
    pub fn reach(&self, from: [f64; 3]) -> f64 {
        self.bodies
            .iter()
            .flat_map(|b| b.mesh.positions.iter().chain(&b.mesh.edge_points))
            .map(|p| {
                let d = sub(*p, from);
                dot(d, d).sqrt()
            })
            .fold(0.0, f64::max)
    }
}

impl RenderBody {
    fn check(&self, i: usize) -> Result<(), InvalidParameter> {
        let mesh = &self.mesh;
        let bad = |what: String| invalid("bodies", format!("bodies[{i}]'s mesh: {what}"));
        if mesh.normals.len() != mesh.positions.len() {
            return Err(bad(format!(
                "{} normals for {} positions",
                mesh.normals.len(),
                mesh.positions.len()
            )));
        }
        let within = |p: &[f64; 3]| p.iter().all(|c| c.abs() <= MAX_RENDER_COORDINATE);
        if let Some(p) = mesh.positions.iter().position(|p| !within(p)) {
            return Err(bad(format!(
                "position {p} must be finite and within {MAX_RENDER_COORDINATE} mm of the origin"
            )));
        }
        if let Some(n) = mesh
            .normals
            .iter()
            .position(|n| n.iter().any(|c| !c.is_finite()))
        {
            return Err(bad(format!("normal {n} isn't finite")));
        }
        let positions = mesh.positions.len();
        if let Some(t) = mesh.triangles.iter().position(|t| {
            t.iter()
                .any(|&v| usize::try_from(v).map_or(true, |v| v >= positions))
        }) {
            return Err(bad(format!(
                "triangle {t} indexes past its {positions} positions"
            )));
        }
        let counted: u64 = mesh.face_triangles.iter().map(|&c| u64::from(c)).sum();
        if counted != mesh.triangles.len() as u64 {
            return Err(bad(format!(
                "its faces count {counted} triangles, and it has {}",
                mesh.triangles.len()
            )));
        }
        if let Some(p) = mesh.edge_points.iter().position(|p| !within(p)) {
            return Err(bad(format!(
                "edge point {p} must be finite and within {MAX_RENDER_COORDINATE} mm of the origin"
            )));
        }
        let counted: u64 = mesh.edge_point_counts.iter().map(|&c| u64::from(c)).sum();
        if counted != mesh.edge_points.len() as u64 {
            return Err(bad(format!(
                "its edges count {counted} points, and it has {}",
                mesh.edge_points.len()
            )));
        }
        // Every face's and edge's number from 1 must fit under the edge bit.
        for (what, count) in [
            ("faces", mesh.face_triangles.len()),
            ("edges", mesh.edge_point_counts.len()),
        ] {
            if count as u64 >= u64::from(EDGE_BIT) {
                return Err(bad(format!(
                    "{count} {what} are more than the ID buffer numbers"
                )));
            }
        }
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
