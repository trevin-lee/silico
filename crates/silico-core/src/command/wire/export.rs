//! M4.8's wire types (docs/design/m4.1-command-api.md, section 7, `export`): its request,
//! read strictly and written back as a client writes it; its result, written field for
//! field; and the mirrors its snapshot, `commands/export.json`, is generated from.

use std::path::PathBuf;

use schemars::JsonSchema;

use super::{Json, RequestFields, ToWire, wire_shape};
use crate::command::export::{Export, ExportFormat, Exported, ExportedObject};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{Problems, SessionId};
use crate::document::json::Node;
use crate::document::{FeatureId, Length, Measure};

/// An `export` request: the session, and what to export (an export reads the current
/// model only).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportRequest {
    pub session: SessionId,
    pub export: Export,
}

/// The formats, by the words a request writes them in.
const FORMATS: [(&str, ExportFormat); 1] = [(ExportFormat::ThreeMf.name(), ExportFormat::ThreeMf)];

impl ExportRequest {
    /// The request `node` holds, or every reason its shape is refused. Its values are the
    /// command's to check (the tolerance's range, the path, the features), in the design's
    /// order, as one refusal.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let format = f.word("format", &FORMATS, "formats");
            let path = f.string("path");
            let replace = f.bool("replace");
            let features = f.nullable_feature_ids("features");
            let tolerance = f.length("tolerance");
            Some(ExportRequest {
                session: session?,
                export: Export {
                    format: format?,
                    path: PathBuf::from(path?),
                    replace: replace?,
                    features: features?,
                    tolerance: tolerance?,
                },
            })
        })
    }
}

impl ToWire for ExportRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        let e = &self.export;
        Json::object([
            ("session", self.session.to_wire()),
            ("format", Json::str(e.format.name())),
            ("path", Json::string(e.path.to_string_lossy().into_owned())),
            ("replace", Json::bool(e.replace)),
            (
                "features",
                Json::or_null(e.features.as_ref(), |ids| {
                    Json::array(ids.iter().map(|id| Json::str(id.as_str())))
                }),
            ),
            ("tolerance", Json::length(e.tolerance)),
        ])
    }
}

impl ToWire for Exported {
    /// `export`'s result: the path written, the file's size, and each object in it.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            (
                "path",
                Json::string(self.path.to_string_lossy().into_owned()),
            ),
            ("bytes", Json::whole(self.bytes)),
            (
                "objects",
                Json::array(self.objects.iter().map(object_to_wire)),
            ),
        ])
    }
}

fn object_to_wire(object: &ExportedObject) -> Json<'_> {
    Json::object([
        ("feature", Json::str(object.feature.as_str())),
        ("name", Json::str(&object.name)),
        ("triangles", Json::whole(object.triangles)),
        ("vertices", Json::whole(object.vertices)),
        ("volume", Json::measure(object.volume)),
        (
            "left_out",
            Json::array(object.left_out.iter().map(|n| Json::str(n))),
        ),
    ])
}

/// What `export` writes: 3MF, each body a mesh, as a slicer prints it.
#[derive(JsonSchema)]
#[schemars(rename = "ExportFormat", rename_all = "lowercase")]
pub enum ExportFormatSchema {
    #[schemars(rename = "3mf")]
    ThreeMf,
}

/// `export`'s request: the current model's bodies, or the features named, written as a
/// file, each solid meshed at the tolerance.
#[derive(JsonSchema)]
#[schemars(rename = "ExportRequest", transform = wire_shape)]
pub struct ExportRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub format: ExportFormatSchema,
    /// Where to write it: a path in the workspace ending in the format's extension, never a
    /// document or its cache.
    pub path: String,
    /// Whether to replace a file already at `path`.
    pub replace: bool,
    /// The features whose solids to write, each by its ID, in the file's order; or null,
    /// for every body.
    pub features: Option<Vec<FeatureId>>,
    /// The mesh's tolerance, in mm, from 0.00001 to 100000000: the mesher's linear
    /// deflection, as FreeCAD's export takes it (its default is 0.1).
    pub tolerance: Length,
}

/// An object of an export: one solid's mesh.
#[derive(JsonSchema)]
#[schemars(rename = "ExportedObject", transform = wire_shape)]
pub struct ExportedObjectSchema {
    /// The feature whose solid it is.
    pub feature: FeatureId,
    /// The feature's name, the object's name in the file.
    pub name: String,
    /// The mesh's triangles.
    pub triangles: u64,
    /// The mesh's vertices.
    pub vertices: u64,
    /// The mesh's volume, in mm³, computed in doubles from the coordinates as written.
    pub volume: Measure,
    /// The faces left out of the mesh as bounding no material (OCCT's INTERNAL and
    /// EXTERNAL), by their names: none for most parts.
    pub left_out: Vec<String>,
}

/// `export`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "ExportResult", transform = wire_shape)]
pub struct ExportResultSchema {
    /// Where the file was written, as the request gave it.
    pub path: String,
    /// The file's size, in bytes.
    pub bytes: u64,
    /// Each object in the file, in its order.
    pub objects: Vec<ExportedObjectSchema>,
}
