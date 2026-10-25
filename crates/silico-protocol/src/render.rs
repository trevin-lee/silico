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

impl Camera {
    /// Whether the camera's numbers can make a view: each finite, the eye apart from the
    /// target, the up with a part across the view of at least [`MIN_UP_ACROSS_VIEW`] of its
    /// length, and the projection's number in range. Named as M4.1's request names them.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        for (parameter, v) in [
            ("camera.eye", self.eye),
            ("camera.target", self.target),
            ("camera.up", self.up),
        ] {
            if !finite3(v) {
                return Err(invalid(
                    parameter,
                    format!("{parameter} {v:?} isn't finite"),
                ));
            }
        }
        let view = sub(self.target, self.eye);
        let view_length = dot(view, view).sqrt();
        if !positive(view_length) {
            return Err(invalid(
                "camera.eye",
                format!(
                    "camera.eye {:?} must be apart from camera.target {:?}",
                    self.eye, self.target
                ),
            ));
        }
        let up_length = dot(self.up, self.up).sqrt();
        // The up's part across the view: |up × view| / |view|, which keeps its precision
        // where the up is nearly along the view.
        let c = cross(self.up, view);
        let across = dot(c, c).sqrt() / view_length;
        if !positive(up_length) || across < MIN_UP_ACROSS_VIEW * up_length {
            return Err(invalid(
                "camera.up",
                format!(
                    "camera.up {:?} must have a part across the view, from camera.eye to camera.target",
                    self.up
                ),
            ));
        }
        match self.projection {
            Projection::Orthographic { view_height } => {
                if !positive(view_height) {
                    return Err(invalid(
                        "camera.view_height",
                        format!("camera.view_height {view_height} must be finite and positive"),
                    ));
                }
            }
            Projection::Perspective { field_of_view } => {
                if !(field_of_view > 0.0 && field_of_view < 180.0) {
                    return Err(invalid(
                        "camera.field_of_view",
                        format!(
                            "camera.field_of_view {field_of_view} must be more than 0 and less than 180 degrees"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl RenderScene {
    /// Whether the scene can be drawn without the worker trusting anything about it: the
    /// sizes, the camera ([`Camera::check`]), the bodies' numbers (at least 1, strictly
    /// ascending), each mesh's structure (positions and normals finite and one each, every
    /// index in range, the faces' and edges' counts adding up, every number fitting the ID
    /// buffer), and the highlights (ascending, each once, in range). Only structure: what
    /// needs the model is the core's.
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
        Ok(())
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
        if let Some(p) = mesh.positions.iter().position(|p| !finite3(*p)) {
            return Err(bad(format!("position {p} isn't finite")));
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
        if let Some(p) = mesh.edge_points.iter().position(|p| !finite3(*p)) {
            return Err(bad(format!("edge point {p} isn't finite")));
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
