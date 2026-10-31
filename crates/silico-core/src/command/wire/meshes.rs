//! `meshes`' wire types (docs/design/m5.0-app.md, section 7, `meshes`, M5.5): its request,
//! read strictly ([`MeshesRequest`]) and written back as a client writes it; its result,
//! written field for field ([`ToWire`] for [`MeshesRead`]), its numbers as the design's A5
//! gives them; the answer of a response too long to send, naming the field that makes it
//! ([`answer_text`]); and the mirrors its snapshot, `commands/meshes.json`, is generated
//! from.
//!
//! **Numbers** (A5). A mesh's coordinates (`positions`, `edge_points`) and a body's `box` are
//! the kernel's `f64`, each written as a `Coordinate` is (the shortest decimal that reads
//! back as it, never an exponent), but `-0` as `0`: they're the mesh, not measures of it,
//! never rounded to a length. Its normals are `f32`, as OCCT keeps them, each the shortest
//! decimal that reads back as the same `f32`, `-0` as `0`. A level (`level`, `deflection`,
//! `serves`) is written as a coordinate is. A `held` mesh's `deflection` is read correctly
//! rounded to the nearest `f64`, so the decimal a result wrote reads back as the deflection
//! it was.
//!
//! **The response's bound** (section 6, *Limits*). A response past the 128 MiB one may be is
//! refused as `invalid_input`, naming `features` when the answer holds more than one body
//! (ask for fewer), or, when one body alone passes it, the side whose fewer pixels mesh it
//! coarser: a camera's `height`, or the side that bound a named view's fit, as `render`
//! names it for a scene past a frame. The session and model are kept, as any response too
//! long keeps them. A Rust client takes the typed answer, whose meshes are shared, never
//! written: only a response written is bounded.

use schemars::JsonSchema;

use super::elements::ModelChoiceSchema;
use super::render::{
    CameraSchema, NamedViewSchema, answered_to_wire, read_camera, requested_to_wire,
};
use super::{
    Json, Layout, MAX_RESPONSE_LEN, RequestFields, ToWire, answer_within_or, invalid, join,
    wire_shape, written,
};
use crate::command::meshes::{Held, MeshElement, MeshedBody, Meshes, MeshesRead};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{ModelChoice, Problems, Response, SessionId};
use crate::document::json::{Node, Value};
use crate::document::{Coordinate, FeatureId};
use crate::recompute::Key;
use crate::views::{NAMED_VIEWS, NamedView};
use crate::{Error, ErrorCode};

/// The pattern a mesh's key matches: 64 lowercase hex digits.
pub const KEY_PATTERN: &str = "^[0-9a-f]{64}$";

/// A `meshes` request: the session, the model it reads, and the view and bodies to mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshesRequest {
    pub session: SessionId,
    pub model: ModelChoice,
    pub meshes: Meshes,
}

impl MeshesRequest {
    /// The request `node` holds, or every reason its shape is refused. Its values are the
    /// command's to check (the sizes, the camera's numbers, the view and camera given
    /// together, the features named), as `render`'s are.
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
            let features = f.nullable_feature_ids("features");
            let held = f.objects("held", usize::MAX, never_too_many, read_held);
            Some(MeshesRequest {
                session: session?,
                model: model?,
                meshes: Meshes {
                    view: view?,
                    camera: camera?,
                    width: width?,
                    height: height?,
                    features: features?,
                    held: held?,
                },
            })
        })
    }
}

/// A list `held` can't be too long for: it's bounded by the request's own size alone.
fn never_too_many(count: usize) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("{count} meshes held, refused, though held has no bound"),
    )
    .with_parameter("held")
    .with_suggestion(crate::worker::SILICO_BUG)
}

/// A mesh the client holds: its `key`, 64 lowercase hex digits, and its `deflection`, a
/// positive number, each refused naming its path (`held.0.key`).
fn read_held(f: &mut RequestFields<'_>) -> Option<Held> {
    let key = f.take("key").and_then(|node| {
        let key = match &node.value {
            Value::String(text) => Key::parse(text),
            _ => None,
        };
        if key.is_none() {
            f.wrong("key", "a mesh's key, 64 lowercase hex digits", node);
        }
        key
    });
    let deflection = f.take("deflection").and_then(|node| {
        // Valid JSON number syntax always parses: past the largest `f64`, as infinite,
        // which `Coordinate` refuses.
        let deflection = match &node.value {
            Value::Number(text) => Coordinate::parse(text)
                .map(Coordinate::mm)
                .filter(|d| *d > 0.0),
            _ => None,
        };
        if deflection.is_none() {
            let at = join(&f.path, "deflection");
            f.refuse(
                invalid(
                    &at,
                    format!("{at} must be a positive number, got {}", written(node)),
                )
                .with_suggestion("give back the deflection a meshes result gave the mesh"),
            );
        }
        deflection
    });
    Some(Held {
        key: key?,
        deflection: deflection?,
    })
}

/// A coordinate of a mesh (a position, an edge's point, a box's corner): as a coordinate
/// is written, but `-0` as `0`.
fn coordinate(value: f64) -> Json<'static> {
    Json::coordinate(if value == 0.0 { 0.0 } else { value })
}

/// A point of a mesh, `[x, y, z]`.
fn point(p: [f64; 3]) -> Json<'static> {
    Json::array(p.map(coordinate))
}

/// A held mesh as a request writes it.
fn held_to_wire(held: &Held) -> Json<'static> {
    Json::object([
        ("key", Json::string(held.key.to_string())),
        ("deflection", Json::coordinate(held.deflection)),
    ])
}

impl ToWire for MeshesRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        let m = &self.meshes;
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
            ("view", Json::or_null(m.view, |v| Json::str(v.name()))),
            (
                "camera",
                Json::or_null(m.camera.as_ref(), requested_to_wire),
            ),
            ("width", Json::whole(m.width.into())),
            ("height", Json::whole(m.height.into())),
            ("features", Json::or_null(m.features.as_deref(), Json::ids)),
            ("held", Json::array(m.held.iter().map(held_to_wire))),
        ])
    }
}

/// Each face or edge of a body: its name, its stability, and its count, `key` (`triangles`
/// or `points`), with the geometry only (`null` when held).
fn elements<'a>(elements: &'a [MeshElement], key: &'static str) -> Json<'a> {
    Json::array(elements.iter().map(move |e| {
        Json::object([
            ("name", Json::string(e.name.to_string())),
            ("stable", Json::bool(e.stable)),
            (key, Json::or_null(e.count, |n| Json::whole(n.into()))),
        ])
    }))
}

impl ToWire for MeshedBody {
    /// A body's mesh: what it's of, its level and what it serves, and, unless the client
    /// holds it, its geometry, each face and edge named either way.
    fn to_wire(&self) -> Json<'_> {
        let mesh = self.mesh.as_deref();
        Json::object([
            ("feature", self.feature.to_wire()),
            ("body", Json::or_null(self.body, |n| Json::whole(n.into()))),
            ("key", Json::string(self.key.to_string())),
            ("level", Json::coordinate(self.level)),
            ("deflection", Json::coordinate(self.deflection)),
            (
                "box",
                Json::or_null(self.bounds, |b| Json::array(b.map(point))),
            ),
            ("serves", Json::array(self.serves.map(coordinate))),
            ("held", Json::bool(self.held)),
            (
                "positions",
                Json::array(
                    mesh.into_iter()
                        .flat_map(|m| &m.positions)
                        .map(|p| point(*p)),
                ),
            ),
            (
                "normals",
                Json::array(
                    mesh.into_iter()
                        .flat_map(|m| &m.normals)
                        .map(|n| Json::array(n.map(Json::single))),
                ),
            ),
            (
                "triangles",
                Json::array(
                    mesh.into_iter()
                        .flat_map(|m| &m.triangles)
                        .map(|t| Json::array(t.map(|i| Json::whole(i.into())))),
                ),
            ),
            ("faces", elements(&self.faces, "triangles")),
            (
                "edge_points",
                Json::array(
                    mesh.into_iter()
                        .flat_map(|m| &m.edge_points)
                        .map(|p| point(*p)),
                ),
            ),
            ("edges", elements(&self.edges, "points")),
        ])
    }
}

impl ToWire for MeshesRead {
    /// `meshes`' result: the camera, as `render` answers it, and each body in timeline
    /// order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("camera", answered_to_wire(&self.camera)),
            (
                "bodies",
                Json::array(self.bodies.iter().map(ToWire::to_wire)),
            ),
        ])
    }
}

/// `meshes`' response as text, in `layout`, and whether it ran: its envelope; or, past the
/// [`MAX_RESPONSE_LEN`] a response may be, its refusal naming the field that makes it
/// ([`too_long`]).
pub(crate) fn answer_text(response: &Response<MeshesRead>, layout: Layout) -> (bool, String) {
    answer_within(response, layout, MAX_RESPONSE_LEN)
}

/// [`answer_text`], against `limit`.
pub(crate) fn answer_within(
    response: &Response<MeshesRead>,
    layout: Layout,
    limit: usize,
) -> (bool, String) {
    answer_within_or(response, layout, limit, |response| {
        response
            .result
            .as_ref()
            .ok()
            .map(|read| too_long(read, limit))
    })
}

/// The refusal of `read`, whose response passes `limit`: naming `features` if it holds more
/// than one body, or, for one body alone, the side whose fewer pixels mesh it coarser.
fn too_long(read: &MeshesRead, limit: usize) -> Error {
    let message = format!(
        "the meshes would make a response of more than the {limit} bytes a response may be"
    );
    match read.bodies.as_slice() {
        [_, _, ..] => Error::new(ErrorCode::InvalidInput, message)
            .with_parameter("features")
            .with_suggestion(format!(
                "name fewer of the {} bodies in features, and read the rest in another request",
                read.bodies.len()
            )),
        [body] => Error::new(ErrorCode::InvalidInput, message)
            .with_feature(body.feature.as_str())
            .with_parameter(read.coarser)
            .with_suggestion(format!(
                "give a smaller {}: it meshes the body coarser",
                read.coarser
            )),
        // No body: the camera alone, which no response passes.
        [] => Error::new(ErrorCode::InvalidInput, message).with_suggestion("ask for less at once"),
    }
}

/// A mesh the client holds, as `held` gives it.
#[derive(JsonSchema)]
#[schemars(rename = "HeldMesh", transform = wire_shape)]
pub struct HeldMeshSchema {
    /// Its body's `key`, as a meshes result gave it.
    #[schemars(regex(pattern = KEY_PATTERN))]
    pub key: String,
    /// The `deflection` a meshes result gave the mesh, as it gave it.
    #[schemars(extend("exclusiveMinimum" = 0))]
    pub deflection: f64,
}

/// `meshes`' request: the display meshes of the model's bodies for a view, from a named
/// view or a camera (one, the other null), each body at the largest power of two no coarser
/// than half a pixel, as `render` meshes it. Every body asked is meshed, never culled to the
/// view.
#[derive(JsonSchema)]
#[schemars(rename = "MeshesRequest", transform = wire_shape)]
pub struct MeshesRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
    /// A named view, fitted to the bodies; null with a camera.
    pub view: Option<NamedViewSchema>,
    /// A camera; null with a named view.
    pub camera: Option<CameraSchema>,
    /// The view's width in pixels.
    #[schemars(range(min = 1, max = silico_protocol::MAX_RENDER_SIDE))]
    pub width: u32,
    /// The view's height in pixels.
    #[schemars(range(min = 1, max = silico_protocol::MAX_RENDER_SIDE))]
    pub height: u32,
    /// The features whose solids to mesh, by ID, each once; null for every body.
    pub features: Option<Vec<FeatureId>>,
    /// The meshes the client holds already, each its key and deflection as a result gave
    /// them: a body held is named, but its geometry isn't given again.
    pub held: Vec<HeldMeshSchema>,
}

/// A face of a body's mesh.
#[derive(JsonSchema)]
#[schemars(rename = "MeshFace", transform = wire_shape)]
pub struct MeshFaceSchema {
    /// Its name, by the element map: give it to `distance`, `angle`, or `render`'s
    /// `highlight`, or, if it's stable, to a fillet's `edges`.
    pub name: String,
    /// Whether `name` is stable: one a reference stored in the document may use.
    pub stable: bool,
    /// How many of the mesh's triangles are its, the next in `triangles` after the faces'
    /// before it; null when the body is held.
    pub triangles: Option<u32>,
}

/// An edge of a body's mesh.
#[derive(JsonSchema)]
#[schemars(rename = "MeshEdge", transform = wire_shape)]
pub struct MeshEdgeSchema {
    /// Its name, by the element map.
    pub name: String,
    /// Whether `name` is stable.
    pub stable: bool,
    /// How many of `edge_points` are its polyline's, the next after the edges' before it;
    /// null when the body is held.
    pub points: Option<u32>,
}

/// A body's display mesh.
#[derive(JsonSchema)]
#[schemars(rename = "MeshedBody", transform = wire_shape)]
pub struct MeshedBodySchema {
    /// The feature whose solid it is.
    pub feature: FeatureId,
    /// Its number among the model's bodies, from 1, as a render's ID buffer numbers it;
    /// null for a feature's solid that isn't a body (one another feature consumes).
    #[schemars(range(min = 1))]
    pub body: Option<u32>,
    /// Its step's key: the same mesh has the same key, on every platform, and a change to
    /// anything it's built from changes it.
    #[schemars(regex(pattern = KEY_PATTERN))]
    pub key: String,
    /// The deflection, in mm, the view asked for: a power of two.
    pub level: f64,
    /// The deflection the kernel used: the level, or the body's floor, which is coarser.
    pub deflection: f64,
    /// Its tight box, `[min, max]`, in mm; null if it couldn't be measured.
    pub r#box: Option<[[f64; 3]; 2]>,
    /// The half pixels, in mm, `[at_least, below]`, this mesh serves: read again only when
    /// the view's half pixel at the body's nearest point leaves them.
    pub serves: [f64; 2],
    /// Whether the client holds this mesh (its key at its deflection): then `positions`,
    /// `normals`, `triangles`, and `edge_points` are empty, and each face's and edge's count
    /// null.
    pub held: bool,
    /// The mesh's vertices, in the document's coordinates (mm), shared within a face.
    pub positions: Vec<[f64; 3]>,
    /// One per position: the unit normal of its face's surface there, out of the solid.
    pub normals: Vec<[f32; 3]>,
    /// Vertex indices, three a triangle, wound counterclockwise seen from outside, face by
    /// face in `faces`' order.
    pub triangles: Vec<[u32; 3]>,
    /// Each face, in the solid's own order (`Face1` first).
    pub faces: Vec<MeshFaceSchema>,
    /// The points of every edge's polyline, one edge after another, in `edges`' order.
    pub edge_points: Vec<[f64; 3]>,
    /// Each edge, in the solid's own order (`Edge1` first).
    pub edges: Vec<MeshEdgeSchema>,
}

/// `meshes`' result.
#[derive(JsonSchema)]
#[schemars(rename = "MeshesResult", transform = wire_shape)]
pub struct MeshesResultSchema {
    /// The camera of the view: the one given, or the named view's, its up made
    /// perpendicular to the view and of unit length.
    pub camera: CameraSchema,
    /// Each body meshed, in timeline order.
    pub bodies: Vec<MeshedBodySchema>,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use silico_protocol::{Camera, Mesh, Projection};

    use super::*;
    use crate::command::render::AnsweredCamera;
    use crate::command::{ModelRead, Revision, SessionInfo};
    use crate::naming::{Maker, Name, OpCode};

    fn name(text: &str) -> Arc<Name> {
        let maker = |f: &FeatureId| {
            Some(Maker::By(if f.as_str().starts_with('s') {
                OpCode::Sketch
            } else {
                OpCode::Pad
            }))
        };
        Arc::new(Name::parse(text, &maker).unwrap())
    }

    /// The design's example's camera, orthographic, 80 mm high.
    fn camera() -> AnsweredCamera {
        AnsweredCamera::rounded(&Camera {
            projection: Projection::Orthographic { view_height: 80.0 },
            eye: [100.0, -50.0, 95.0],
            target: [30.0, 20.0, 25.0],
            up: [
                -0.408_248_290_463_863_13,
                0.408_248_290_463_863_13,
                0.816_496_580_927_725_8,
            ],
        })
        .unwrap()
    }

    /// A body of `triangles` triangles in one face, its mesh of three positions repeated, not
    /// held.
    fn body(feature: &str, triangles: usize) -> MeshedBody {
        let mesh = Mesh {
            positions: vec![[0.0, 0.0, 0.0]; 3],
            normals: vec![[0.0, 0.0, -1.0]; 3],
            triangles: vec![[0, 2, 1]; triangles],
            face_triangles: vec![triangles as u32],
            valid: None,
            edge_points: vec![],
            edge_point_counts: vec![],
        };
        MeshedBody {
            feature: FeatureId::new(feature).unwrap(),
            body: Some(1),
            key: Key::parse(&"0".repeat(64)).unwrap(),
            level: 0.03125,
            deflection: 0.03125,
            bounds: Some([[0.0; 3], [1.0; 3]]),
            serves: [0.03125, 0.0625],
            held: false,
            mesh: Some(Arc::new(mesh)),
            faces: vec![MeshElement {
                name: name(&format!("{feature}.pad.Face1")),
                stable: false,
                count: Some(triangles as u32),
            }],
            edges: vec![],
        }
    }

    fn response(bodies: Vec<MeshedBody>, coarser: &'static str) -> Response<MeshesRead> {
        Response {
            session: Some(SessionInfo {
                id: SessionId::parse("s1").unwrap(),
                path: "bracket.silico".to_owned(),
                revision: Revision::OPENED,
                transaction: false,
                reloaded: false,
            }),
            model: Some(ModelRead {
                revision: Revision::OPENED,
                of: ModelChoice::Published,
                fault: None,
            }),
            result: Ok(MeshesRead {
                camera: camera(),
                bodies,
                coarser,
            }),
        }
    }

    fn contract(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/contract/commands/examples/meshes")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// `error` as an example file holds it.
    fn example_of(error: &Error) -> String {
        super::super::write(&error.to_wire(), Layout::Canonical).unwrap() + "\n"
    }

    /// The refusal a too-long `response` is answered with, at `limit`, and its envelope.
    fn refused_at(response: &Response<MeshesRead>, limit: usize) -> serde_json::Value {
        let (ok, text) = answer_within(response, Layout::OneLine, limit);
        assert!(!ok, "{text}");
        assert!(text.len() <= limit, "{} bytes", text.len());
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn a_response_within_the_limit_is_its_envelope_and_one_past_it_names_features_or_the_side() {
        // Two bodies: the whole envelope's length is the limit it fits at; one byte under,
        // refused naming `features`, its session and model kept.
        let two = response(vec![body("f0000001", 4), body("f0000002", 4)], "height");
        let whole = super::super::write(&super::super::response(&two), Layout::OneLine).unwrap();
        assert_eq!(
            answer_within(&two, Layout::OneLine, whole.len()),
            (true, whole.clone())
        );
        let refused = refused_at(&two, whole.len() - 1);
        assert_eq!(refused["session"]["id"], "s1");
        assert_eq!(refused["model"]["of"], "published");
        let e = &refused["errors"][0];
        assert_eq!(e["code"], "invalid_input");
        assert_eq!(e["parameter"], "features");
        assert_eq!(e["feature"], serde_json::Value::Null);
        assert_eq!(
            e["message"],
            format!(
                "the meshes would make a response of more than the {} bytes a response may be",
                whole.len() - 1
            )
        );
        assert_eq!(
            e["suggestion"],
            "name fewer of the 2 bodies in features, and read the rest in another request"
        );
        assert_eq!(refused["total"], 1);
        // One body alone: the side whose fewer pixels mesh it coarser, as the read says it
        // (a camera's height; a named view's side that bound its fit), and the body.
        for side in ["height", "width"] {
            let one = response(vec![body("f0000001", 400)], side);
            let refused = refused_at(&one, 1000);
            let e = &refused["errors"][0];
            assert_eq!(e["parameter"], side);
            assert_eq!(e["feature"], "f0000001");
            assert_eq!(
                e["suggestion"],
                format!("give a smaller {side}: it meshes the body coarser")
            );
        }
        // A refusal isn't a result to bound: one too long is any response's.
        let refusal: Response<MeshesRead> = Response {
            result: Err(Problems::from(
                Error::new(ErrorCode::InvalidInput, "x".repeat(2000)).with_parameter("width"),
            )),
            ..response(vec![], "height")
        };
        let refused = refused_at(&refusal, 1000);
        assert_eq!(
            refused["errors"][0]["message"],
            "the response would be more than the 1000 bytes a response may be"
        );
        // The real limit, 128 MiB.
        let small = response(vec![body("f0000001", 1)], "height");
        assert_eq!(
            answer_text(&small, Layout::OneLine),
            super::super::answer_text(&small, Layout::OneLine)
        );
    }

    #[test]
    fn the_too_long_refusals_are_the_contracts_examples() {
        let two = response(vec![body("f0000001", 1), body("f0000002", 1)], "height");
        let Ok(read) = &two.result else { panic!() };
        assert_eq!(
            example_of(&too_long(read, MAX_RESPONSE_LEN)),
            contract("refused-too-long-bodies.json")
        );
        let one = response(vec![body("f0000001", 1)], "height");
        let Ok(read) = &one.result else { panic!() };
        assert_eq!(
            example_of(&too_long(read, MAX_RESPONSE_LEN)),
            contract("refused-too-long-body.json")
        );
    }

    #[test]
    fn a_single_is_its_shortest_decimal_never_an_exponent_and_minus_zero_is_zero() {
        let written = |v: f32| super::super::write(&Json::single(v), Layout::OneLine).unwrap();
        // Each the shortest decimal that reads back as the f32, worked by hand from the
        // f32 nearest: 0.1 is 0.100000001490116..., 1/3 is 0.3333333432674408..., whose
        // shortest round trips are 0.1 and 0.33333334 (an f64's would need 17 digits).
        assert_eq!(written(0.1), "0.1");
        assert_eq!(written(1.0 / 3.0), "0.33333334");
        assert_eq!(written(-0.707_106_77), "-0.70710677");
        assert_eq!(written(1.0), "1");
        assert_eq!(written(-1.0), "-1");
        assert_eq!(written(-0.0), "0");
        assert_eq!(written(0.0), "0");
        // Tiny and huge: written out in full, as a coordinate is.
        assert_eq!(written(1e-7), "0.0000001");
        assert_eq!(
            written(f32::from_bits(1)),
            "0.000000000000000000000000000000000000000000001"
        );
        assert_eq!(written(3e38), "300000000000000000000000000000000000000");
        // No number for what isn't finite.
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(written(v), "null");
        }
        // Each reads back as itself.
        for v in [
            0.1f32,
            1.0 / 3.0,
            -0.707_106_77,
            1e-7,
            f32::from_bits(1),
            3e38,
        ] {
            assert_eq!(written(v).parse::<f32>().unwrap().to_bits(), v.to_bits());
        }
    }
}
