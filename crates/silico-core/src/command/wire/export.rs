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
use crate::{Error, ErrorCode};

/// An `export` request: the session, and what to export (an export reads the current
/// model only).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportRequest {
    pub session: SessionId,
    pub export: Export,
}

/// A format by its word, before its tolerance is read with it.
#[derive(Debug, Clone, Copy)]
enum Formats {
    ThreeMf,
    Step,
}

/// The formats, by the words a request writes them in.
const FORMATS: [(&str, Formats); 2] = [("3mf", Formats::ThreeMf), ("step", Formats::Step)];

impl ExportRequest {
    /// The request `node` holds, or every reason its shape is refused: as well as each
    /// field's own, a tolerance that doesn't fit the format (a length for a mesh format, null
    /// for an exact one), as a camera's fields are refused by its projection. Its values are
    /// the command's to check (the tolerance's range, the path, the features), in the
    /// design's order, as one refusal.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let format = f.word("format", &FORMATS, "formats");
            let path = f.string("path");
            let replace = f.bool("replace");
            let features = f.nullable_feature_ids("features");
            let tolerance = f.nullable_length("tolerance");
            let format = match (format?, tolerance?) {
                (Formats::ThreeMf, Some(tolerance)) => ExportFormat::ThreeMf { tolerance },
                (Formats::Step, None) => ExportFormat::Step,
                (Formats::ThreeMf, None) => {
                    f.refuse(
                        Error::new(
                            ErrorCode::InvalidInput,
                            "tolerance must be given for a 3mf export, got null",
                        )
                        .with_parameter("tolerance")
                        .with_suggestion(
                            "give the mesh's tolerance in mm; 0.1 mm is FreeCAD's default",
                        ),
                    );
                    return None;
                }
                (Formats::Step, Some(tolerance)) => {
                    f.refuse(
                        Error::new(
                            ErrorCode::InvalidInput,
                            format!("tolerance must be null for a step export, got {tolerance}"),
                        )
                        .with_parameter("tolerance")
                        .with_suggestion(
                            "give null: STEP writes each body's exact B-rep, not a mesh, so it \
                             takes no tolerance",
                        ),
                    );
                    return None;
                }
            };
            Some(ExportRequest {
                session: session?,
                export: Export {
                    format,
                    path: PathBuf::from(path?),
                    replace: replace?,
                    features: features?,
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
            (
                "tolerance",
                Json::or_null(e.format.tolerance().as_ref(), |t| Json::length(*t)),
            ),
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
        (
            "triangles",
            Json::or_null(object.mesh.as_ref(), |m| Json::whole(m.triangles)),
        ),
        (
            "vertices",
            Json::or_null(object.mesh.as_ref(), |m| Json::whole(m.vertices)),
        ),
        (
            "volume",
            Json::or_null(object.mesh.as_ref(), |m| Json::measure(m.volume)),
        ),
        (
            "left_out",
            Json::array(object.left_out.iter().map(|n| Json::str(n))),
        ),
    ])
}

/// What `export` writes: 3MF, each body a mesh, as a slicer prints it; or STEP, each body's
/// exact B-rep, as other CAD systems read it.
#[derive(JsonSchema)]
#[schemars(rename = "ExportFormat", rename_all = "lowercase")]
pub enum ExportFormatSchema {
    #[schemars(rename = "3mf")]
    ThreeMf,
    #[schemars(rename = "step")]
    Step,
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
    /// A mesh format's tolerance (3mf), in mm, from 0.00001 to 100000000: the mesher's
    /// linear deflection, as FreeCAD's export takes it (its default is 0.1); null for an
    /// exact format (step), which takes none.
    pub tolerance: Option<Length>,
}

/// An object of an export: one solid as the file holds it, its mesh's counts and volume
/// for a mesh format, null for an exact one.
#[derive(JsonSchema)]
#[schemars(rename = "ExportedObject", transform = wire_shape)]
pub struct ExportedObjectSchema {
    /// The feature whose solid it is.
    pub feature: FeatureId,
    /// The feature's name, the object's name in the file.
    pub name: String,
    /// The mesh's triangles; null for a format that writes no mesh (step).
    pub triangles: Option<u64>,
    /// The mesh's vertices; null for a format that writes no mesh (step).
    pub vertices: Option<u64>,
    /// The mesh's volume, in mm³, computed in doubles from the coordinates as written; null
    /// for a format that writes no mesh (step).
    pub volume: Option<Measure>,
    /// The faces left out of the file as bounding no material (OCCT's INTERNAL and
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
