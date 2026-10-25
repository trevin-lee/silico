//! A `.silico` document: its types, read exactly from strict JSON and written back in
//! canonical form (ARCHITECTURE.md, Document model; docs/design/m2.1-document-format.md).
//! Reading checks each value on its own; whether features fit together, and the kernel can
//! make each, is the validation gate's ([`validate`]).

mod decode;
mod fingerprint;
pub mod json;
mod model;
mod quantity;
mod reach;
mod sketch;
mod sketch_gate;
mod validate;
mod write;

pub(crate) use decode::optional_members;
pub use fingerprint::{Difference, Fingerprint, MEASURE_DECIMALS, Measure};
pub use model::{
    Axis, ChamferSize, Document, ElementName, Extent, ExtentValue, FEATURE_TYPES, Feature,
    FeatureId, FeatureVersion, FormatVersion, ID_ALPHABET, ID_LENGTH, Made, Name, NamingScheme,
    Operation, PadExtent, PocketExtent, Position, SweepFeature, TurnExtent, Units,
};
pub use quantity::{ANGLE_DECIMALS, Angle, LENGTH_DECIMALS, Length, NumberError};
pub use reach::{Before, QuotedIds, net_reach, reached, with_users};
pub use sketch::{
    CONSTRAINT_TYPES, Coordinate, Direction, ElementId, ElementKind, Form, LineSide, Plane,
    Reference, Sketch, SketchArc, SketchCircle, SketchConstraint, SketchLine, SketchPoint,
    TangentSide,
};
pub(crate) use sketch::{MemberPaths, member_id};
pub use sketch_gate::{MAX_ANGLE_DEGREES, MAX_REFERENCE_LENGTH};
pub(crate) use sketch_gate::{an_id_of_its_own, id_held, repeats, too_small};
pub use validate::{Checked, Invalid, PROBLEMS_LISTED, validate};
pub(crate) use write::sketch_elements;

use schemars::generate::SchemaSettings;

use crate::{Error, ErrorCode};

/// The document `bytes` hold, or an `invalid_input` error saying what's wrong and where.
/// A document whose canonical text would pass [`json::MAX_DOCUMENT_LEN`] is refused, as
/// `write` would refuse it, its stored fingerprints counted as `write` counts them (the
/// maintainer, 2026-10-21): a document that opens can always be written back as it is,
/// and what's written always opens. A save writes the fingerprints the rebuild measured
/// and the sketches as solved, which can take a document that opens past the limit; then
/// it's refused, as `write` refuses any document past it.
pub fn read(bytes: &[u8]) -> Result<Document, Error> {
    let _span = tracing::debug_span!("document_read", bytes = bytes.len()).entered();
    read_tree(decode::tree(bytes)?)
}

/// The reader's tree of `bytes` ([`json::read`]), or why they aren't a document's JSON, as
/// [`read`] says it.
pub(crate) fn tree(bytes: &[u8]) -> Result<json::Node, Error> {
    decode::tree(bytes)
}

/// The document the reader's tree `root` holds, read and refused as [`read`] reads and
/// refuses the text it's the tree of: so an edit made on a document's tree is read by the
/// one reader every file is.
pub(crate) fn read_tree(root: json::Node) -> Result<Document, Error> {
    let document = decode::decode_node(root)?;
    if fits(&document) {
        Ok(document)
    } else {
        Err(too_long(
            &document,
            "the document is too large for Silico to read",
        ))
    }
}

/// The feature the reader's tree `node` holds, at `index` in the timeline, read as [`read`]
/// reads each of a document's features.
pub(crate) fn read_feature(index: usize, node: json::Node) -> Result<Feature, Error> {
    decode::feature(index, node)
}

/// The document's canonical text: the same document always writes the same bytes. Refused
/// if the text would be more than [`json::MAX_DOCUMENT_LEN`], as `read` would refuse it.
pub fn write(document: &Document) -> Result<String, Error> {
    let _span =
        tracing::debug_span!("document_write", features = document.features.len()).entered();
    write::write(document, json::MAX_DOCUMENT_LEN)
        .map_err(|write::TooLong| too_long(document, "the document is too large to write"))
}

/// One feature's canonical text, as the document's canonical form writes it, but not
/// indented and with no newline after it: a value, as a command's result gives it. Refused,
/// as `write` refuses a document, if it would be more than [`json::MAX_DOCUMENT_LEN`]: a
/// feature of a document that was read never is, since the document's text holds it.
pub fn write_feature(feature: &Feature) -> Result<String, Error> {
    let limit = json::MAX_DOCUMENT_LEN;
    write::feature_text(feature, limit).map_err(|write::TooLong| {
        Error::new(
            ErrorCode::InvalidInput,
            format!(
                "the feature is too large to write: its canonical text would be more than the {limit} bytes a document may be"
            ),
        )
        .with_feature(feature.id.as_str())
        .with_suggestion(smaller(feature))
    })
}

/// Whether the document's canonical text is within [`json::MAX_DOCUMENT_LEN`]: whether it
/// can be written, counted without writing it.
pub(crate) fn fits(document: &Document) -> bool {
    write::fits(document, json::MAX_DOCUMENT_LEN)
}

/// What the gate refuses of `sketch` on its own (`sketch_gate`): M3.4 checks a solve's
/// result as its file would hold it, so a save never writes what the gate refuses.
pub(crate) fn sketch_problems(sketch: &Sketch) -> Vec<Error> {
    let refuse = |parameter: &str, message: String, suggestion: String| {
        Error::new(ErrorCode::InvalidInput, message)
            .with_parameter(parameter)
            .with_suggestion(suggestion)
    };
    let mut problems = Vec::new();
    sketch_gate::check(sketch, &refuse, &mut |error| problems.push(error));
    problems
}

/// A document whose canonical text would pass the limit, naming the first feature (in
/// timeline order) whose own text is more than half the limit, if one is: it alone is
/// most of the text.
fn too_long(document: &Document, what: &str) -> Error {
    let limit = json::MAX_DOCUMENT_LEN;
    let error = Error::new(
        ErrorCode::InvalidInput,
        format!(
            "{what}: its canonical text would be more than the {limit} bytes a document may be"
        ),
    );
    let large = document
        .features
        .iter()
        .find(|f| !write::feature_fits(f, limit / 2));
    match large {
        Some(feature) => error
            .with_feature(feature.id.as_str())
            .with_suggestion(smaller(feature)),
        None => error.with_suggestion(
            "make it smaller: fewer features, smaller sketches, fewer edges named, or shorter \
             names and notes",
        ),
    }
}

/// How to make `feature`'s text shorter: by what can make it large, every type saying, so a
/// new one has to.
fn smaller(feature: &Feature) -> &'static str {
    match feature.operation {
        Operation::Sketch(_) => "make this sketch smaller: split it into several sketches",
        Operation::Fillet { .. } | Operation::Chamfer { .. } => {
            "name fewer edges, or shorten this feature's name or note"
        }
        Operation::Box { .. }
        | Operation::Cylinder { .. }
        | Operation::Sphere { .. }
        | Operation::Cone { .. }
        | Operation::Torus { .. }
        | Operation::Fuse { .. }
        | Operation::Cut { .. }
        | Operation::Common { .. }
        | Operation::Pad { .. }
        | Operation::Pocket { .. }
        | Operation::Revolve { .. }
        | Operation::Groove { .. } => "make this feature's name or note shorter",
    }
}

/// The JSON Schema of a document (draft 2020-12), as agents get it. The reader is the
/// authority where a schema can't say enough: a seventh decimal, a key written twice.
pub fn schema() -> Result<String, Error> {
    let schema = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<Document>();
    serde_json::to_string_pretty(&sorted(schema.as_value()))
        .map(|text| text + "\n")
        .map_err(|e| {
            Error::new(
                ErrorCode::Internal,
                "the document schema couldn't be written",
            )
            .with_cause(e.to_string())
            .with_suggestion(crate::worker::SILICO_BUG)
        })
}

/// `value` with every object's keys in sorted order. serde_json keeps an object's keys in
/// the order they were added when a crate in the build turns on its `preserve_order`
/// feature, and sorts them otherwise: sorting here makes the schema the same either way.
fn sorted(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.clone(), sorted(v)))
                    .collect(),
            )
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(sorted).collect())
        }
        other => other.clone(),
    }
}
