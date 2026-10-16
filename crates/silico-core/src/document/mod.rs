//! A `.silico` document: its types, read exactly from strict JSON and written back in
//! canonical form (ARCHITECTURE.md, Document model; docs/design/m2.1-document-format.md).
//! Reading checks each value on its own; whether features fit together, and the kernel can
//! make each, is the validation gate's ([`validate`]).

mod decode;
mod fingerprint;
pub mod json;
mod model;
mod quantity;
mod validate;
mod write;

pub use fingerprint::{Difference, Fingerprint, MEASURE_DECIMALS, Measure};
pub use model::{
    Document, FEATURE_TYPES, Feature, FeatureId, FeatureVersion, FormatVersion, ID_ALPHABET,
    ID_LENGTH, Name, Operation, Position, Units,
};
pub use quantity::{ANGLE_DECIMALS, Angle, LENGTH_DECIMALS, Length, NumberError};
pub use validate::{Checked, Invalid, PROBLEMS_LISTED, validate};

use schemars::generate::SchemaSettings;

use crate::{Error, ErrorCode};

/// The document `bytes` hold, or an `invalid_input` error saying what's wrong and where.
/// A document whose canonical text would pass [`json::MAX_DOCUMENT_LEN`] is refused, as
/// `write` would refuse it: a document that opens can always be saved, and what's saved
/// always opens.
pub fn read(bytes: &[u8]) -> Result<Document, Error> {
    let _span = tracing::debug_span!("document_read", bytes = bytes.len()).entered();
    let document = decode::decode(bytes)?;
    if write::fits(&document, json::MAX_DOCUMENT_LEN) {
        Ok(document)
    } else {
        Err(too_long(
            &document,
            "the document is too large for Silico to read",
        ))
    }
}

/// The document's canonical text: the same document always writes the same bytes. Refused
/// if the text would be more than [`json::MAX_DOCUMENT_LEN`], as `read` would refuse it.
pub fn write(document: &Document) -> Result<String, Error> {
    let _span =
        tracing::debug_span!("document_write", features = document.features.len()).entered();
    write::write(document, json::MAX_DOCUMENT_LEN)
        .map_err(|write::TooLong| too_long(document, "the document is too large to write"))
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
            .with_suggestion("make this feature's name or note shorter"),
        None => {
            error.with_suggestion("make it smaller: fewer features, or shorter names and notes")
        }
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
