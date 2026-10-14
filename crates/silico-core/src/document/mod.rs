//! A `.silico` document: its types, read exactly from strict JSON and written back in
//! canonical form (ARCHITECTURE.md, Document model; docs/design/m2.1-document-format.md).
//! Reading checks each value on its own; what features refer to is the validation gate's.

mod decode;
pub mod json;
mod model;
mod quantity;
mod write;

pub use model::{
    Document, FEATURE_TYPES, Feature, FeatureId, FeatureVersion, FormatVersion, ID_ALPHABET,
    ID_LENGTH, Name, Operation, Position, Units,
};
pub use quantity::{ANGLE_DECIMALS, Angle, DecimalError, LENGTH_DECIMALS, Length};

use schemars::generate::SchemaSettings;

use crate::{Error, ErrorCode};

/// The document `bytes` hold, or an `invalid_input` error saying what's wrong and where.
pub fn read(bytes: &[u8]) -> Result<Document, Error> {
    let _span = tracing::debug_span!("document_read", bytes = bytes.len()).entered();
    decode::decode(bytes)
}

/// The document's canonical text: the same document always writes the same bytes. Refused
/// if the text would be more than [`json::MAX_DOCUMENT_LEN`], which `read` would refuse:
/// whatever is written reads back as the same document.
pub fn write(document: &Document) -> Result<String, Error> {
    let _span =
        tracing::debug_span!("document_write", features = document.features.len()).entered();
    let text = write::write(document);
    if text.len() > json::MAX_DOCUMENT_LEN {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "the document's text would be {} bytes, more than the {} a document may be",
                text.len(),
                json::MAX_DOCUMENT_LEN
            ),
        )
        .with_suggestion("make it smaller: fewer features, or shorter names and notes"));
    }
    Ok(text)
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
