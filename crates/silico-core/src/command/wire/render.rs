//! M4.6's wire types (docs/design/m4.1-command-api.md, section 7, `render`): its request,
//! read strictly and written back as a client writes it; its result, written field for
//! field, never the PNG's bytes; and the mirrors its snapshot, `commands/render.json`, is
//! generated from.

use std::path::PathBuf;

use schemars::JsonSchema;
use silico_protocol::{Camera, Projection};

use super::elements::ModelChoiceSchema;
use super::{Json, RequestFields, ToWire, wire_shape};
use crate::command::render::{
    AnsweredCamera, AnsweredProjection, MAX_HIGHLIGHTS, Render, RenderResult, too_many_highlights,
};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{ModelChoice, Problems, SessionId};
use crate::document::json::Node;
use crate::document::{Angle, Length};
use crate::views::{NAMED_VIEWS, NamedView};
use crate::{Error, ErrorCode};

/// A `render` request: the session, the model it reads, and what to render.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderRequest {
    pub session: SessionId,
    pub model: ModelChoice,
    pub render: Render,
}

/// How a request's camera projects, before its height or angle is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Projects {
    Orthographic,
    Perspective,
}

/// The projections, by the words a request writes them in.
const PROJECTIONS: [(&str, Projects); 2] = [
    ("orthographic", Projects::Orthographic),
    ("perspective", Projects::Perspective),
];

impl RenderRequest {
    /// The request `node` holds, or every reason its shape is refused. Its values are the
    /// command's to check (the sizes, the camera's numbers, the view and camera given
    /// together), in the design's order, as one refusal.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        let views: Vec<(&'static str, NamedView)> =
            NAMED_VIEWS.iter().map(|&v| (v.name(), v)).collect();
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = f.model();
            let view = f.nullable_word("view", &views, "named views");
            let camera = f.nullable_object("camera", read_camera);
            let width = f.u32("width");
            let height = f.u32("height");
            let highlight = f.string_list("highlight", MAX_HIGHLIGHTS, too_many_highlights);
            let to = f.nullable_string("to");
            let replace = f.bool("replace");
            Some(RenderRequest {
                session: session?,
                model: model?,
                render: Render {
                    view: view?,
                    camera: camera?,
                    width: width?,
                    height: height?,
                    highlight: highlight?,
                    to: to?.map(PathBuf::from),
                    replace: replace?,
                },
            })
        })
    }
}

/// A camera, as M4.1 writes one: its projection, the eye and the target as lengths, `up`
/// as a direction, and the field of the projection given, the other `null`. A field that
/// doesn't belong to its projection, or one of its own missing, is refused naming it.
fn read_camera(f: &mut RequestFields<'_>) -> Option<Camera> {
    let projects = f.word("projection", &PROJECTIONS, "projections");
    let eye = f.point("eye");
    let target = f.point("target");
    let up = f.direction("up");
    let view_height = f.nullable_length("view_height");
    let field_of_view = f.nullable_angle("field_of_view");
    let projects = projects?;
    let (view_height, field_of_view) = (view_height?, field_of_view?);
    let projection = match (projects, view_height, field_of_view) {
        (Projects::Orthographic, Some(h), None) => Projection::Orthographic {
            view_height: h.mm(),
        },
        (Projects::Perspective, None, Some(a)) => Projection::Perspective {
            field_of_view: a.degrees(),
        },
        (Projects::Orthographic, h, a) => {
            f.refuse(projection_fields(
                "an orthographic",
                "view_height",
                "field_of_view",
                h.is_none(),
                a.is_some(),
            ));
            return None;
        }
        (Projects::Perspective, h, a) => {
            f.refuse(projection_fields(
                "a perspective",
                "field_of_view",
                "view_height",
                a.is_none(),
                h.is_some(),
            ));
            return None;
        }
    };
    let mm = |p: [Length; 3]| p.map(Length::mm);
    Some(Camera {
        projection,
        eye: mm(eye?),
        target: mm(target?),
        up: up?,
    })
}

/// The refusal of a camera whose `projection` takes `own` and not `other`: `own` missing
/// (null), or `other` given.
fn projection_fields(
    projection: &str,
    own: &str,
    other: &str,
    missing: bool,
    given: bool,
) -> Error {
    let (field, message) = if missing {
        (
            own,
            format!("camera.{own} must be given for {projection} camera, got null"),
        )
    } else {
        debug_assert!(given);
        (
            other,
            format!("camera.{other} must be null for {projection} camera"),
        )
    };
    Error::new(ErrorCode::InvalidInput, message)
        .with_parameter(format!("camera.{field}"))
        .with_suggestion(format!(
            "give camera.{own} for {projection} camera, and camera.{other} as null"
        ))
}

/// A camera as a result writes it: in the document's quantities, as it was drawn, so
/// nothing is rounded here.
fn answered_to_wire(camera: &AnsweredCamera) -> Json<'static> {
    let (projection, view_height, field_of_view) = match camera.projection {
        AnsweredProjection::Orthographic { view_height } => {
            ("orthographic", Json::length(view_height), Json::null())
        }
        AnsweredProjection::Perspective { field_of_view } => {
            ("perspective", Json::null(), Json::angle(field_of_view))
        }
    };
    Json::object([
        ("projection", Json::str(projection)),
        ("eye", Json::point(camera.eye)),
        ("target", Json::point(camera.target)),
        ("up", Json::direction(camera.up)),
        ("view_height", view_height),
        ("field_of_view", field_of_view),
    ])
}

/// A request's camera, written back as a client writes it: its numbers were read from the
/// request's digits, so rounding them to the document's quantities gives those digits.
fn requested_to_wire(camera: &Camera) -> Json<'static> {
    match AnsweredCamera::rounded(camera) {
        Some(answered) => answered_to_wire(&answered),
        None => Json::null(),
    }
}

impl ToWire for RenderRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        let r = &self.render;
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
            ("view", Json::or_null(r.view, |v| Json::str(v.name()))),
            (
                "camera",
                Json::or_null(r.camera.as_ref(), requested_to_wire),
            ),
            ("width", Json::whole(r.width.into())),
            ("height", Json::whole(r.height.into())),
            (
                "highlight",
                Json::array(r.highlight.iter().map(|h| Json::str(h))),
            ),
            (
                "to",
                Json::or_null(r.to.as_ref(), |p| {
                    Json::string(p.to_string_lossy().into_owned())
                }),
            ),
            ("replace", Json::bool(r.replace)),
        ])
    }
}

impl ToWire for RenderResult {
    /// `render`'s result: the PNG's size, where it was written, the image's size, and the
    /// camera it was taken with.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("png_bytes", Json::whole(self.png_bytes)),
            (
                "path",
                Json::or_null(self.path.as_ref(), |p| {
                    Json::string(p.to_string_lossy().into_owned())
                }),
            ),
            ("width", Json::whole(self.width.into())),
            ("height", Json::whole(self.height.into())),
            ("camera", answered_to_wire(&self.camera)),
        ])
    }
}

/// How a camera projects: from the eye in parallel (`orthographic`, as every named view
/// is), or from the eye as a point (`perspective`).
#[derive(JsonSchema)]
#[schemars(rename = "Projection", rename_all = "lowercase")]
pub enum ProjectionSchema {
    Orthographic,
    Perspective,
}

/// A camera: where it looks from and at, which way is up in the image, and how much it
/// sees. The document is Z-up. A result's camera can be passed back unchanged.
#[derive(JsonSchema)]
#[schemars(rename = "Camera", transform = wire_shape)]
pub struct CameraSchema {
    pub projection: ProjectionSchema,
    /// The eye, in global coordinates (mm), within 1e12 mm of the origin, at least a
    /// nanometre from the target. An orthographic view uses only its direction from the
    /// target; a perspective eye mustn't be inside a body or on its surface.
    pub eye: [Length; 3],
    /// What the view is centred on, in global coordinates (mm).
    pub target: [Length; 3],
    /// Which way is up in the image: any vector with a part across the view, made
    /// perpendicular to the view and of unit length in a result.
    pub up: [f64; 3],
    /// An orthographic camera's view's height, in mm, from 1e-6 to 1e12; null for a
    /// perspective camera.
    pub view_height: Option<Length>,
    /// A perspective camera's vertical angle, in degrees, from 0.001 to 179.999; null for
    /// an orthographic camera.
    pub field_of_view: Option<Angle>,
}

/// A named view, as Fusion's ViewCube names them, the document Z-up: `front` looks from
/// −Y, `right` from +X, `top` from +Z, and the corners combine them.
#[derive(JsonSchema)]
#[schemars(rename = "NamedView", rename_all = "snake_case")]
pub enum NamedViewSchema {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    TopFrontLeft,
    TopFrontRight,
    TopBackLeft,
    TopBackRight,
    BottomFrontLeft,
    BottomFrontRight,
    BottomBackLeft,
    BottomBackRight,
}

/// `render`'s request: a PNG of the model's bodies from a named view or a camera (one,
/// the other null), with elements highlighted, and where to write it.
#[derive(JsonSchema)]
#[schemars(rename = "RenderRequest", transform = wire_shape)]
pub struct RenderRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
    /// A named view, fitted to the bodies; null with a camera.
    pub view: Option<NamedViewSchema>,
    /// A camera; null with a named view.
    pub camera: Option<CameraSchema>,
    /// The image's width in pixels.
    #[schemars(range(min = 1, max = silico_protocol::MAX_RENDER_SIDE))]
    pub width: u32,
    /// The image's height in pixels.
    #[schemars(range(min = 1, max = silico_protocol::MAX_RENDER_SIDE))]
    pub height: u32,
    /// The names of faces and edges to highlight in Fusion's selection blue, as `elements`
    /// names them: at most 1,000.
    #[schemars(length(max = MAX_HIGHLIGHTS))]
    pub highlight: Vec<String>,
    /// Where to write the PNG: a `.png` path in the workspace, never a document or its
    /// cache; or null.
    pub to: Option<String>,
    /// Whether to replace a file already at `to`.
    pub replace: bool,
}

/// `render`'s result. The PNG itself is never in it: it's the reply's image over MCP, or a
/// Rust client's to show.
#[derive(JsonSchema)]
#[schemars(rename = "RenderResult", transform = wire_shape)]
pub struct RenderResultSchema {
    /// The PNG's size, in bytes. It depends on the machine (its renderer), so it isn't a
    /// result to compare.
    pub png_bytes: u64,
    /// Where the PNG was written, through any links; null if `to` was.
    pub path: Option<String>,
    /// The image's width in pixels.
    pub width: u32,
    /// The image's height in pixels.
    pub height: u32,
    /// The camera it was taken with: the one given, or the named view's, its up made
    /// perpendicular to the view and of unit length.
    pub camera: CameraSchema,
}
