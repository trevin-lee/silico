//! M4.4's `schema` (docs/design/m4.1-command-api.md, section 7): the document's JSON
//! Schema, or a command's request and result schemas, as the protected snapshots in
//! `tests/contract/` hold them, so every client reads the shapes MCP's `tools/list` gives.

use super::Problems;
use super::wire;
use crate::document::{self, Document};
use crate::{Error, ErrorCode};

/// What `schema` answers: the schema asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaAnswer {
    /// The document's JSON Schema, or a command's `{"request": ..., "result": ...}`.
    pub schema: serde_json::Value,
}

impl SchemaAnswer {
    /// The schema as its committed snapshot holds it: its keys sorted, pretty, with a
    /// trailing newline (what `silico schema` prints).
    pub fn text(&self) -> Result<String, Error> {
        document::schema_text(&self.schema, "the answer")
    }
}

/// What `of` describes: `document`, or one of [`wire::COMMANDS`].
pub fn schema(of: &str) -> Result<SchemaAnswer, Problems> {
    let _span = tracing::debug_span!("schema", of).entered();
    if of == "document" {
        return Ok(SchemaAnswer {
            schema: document::schema_value_of::<Document>(),
        });
    }
    match wire::COMMANDS
        .contains(&of)
        .then(|| wire::snapshot_value(of))
        .flatten()
    {
        Some(schema) => Ok(SchemaAnswer { schema }),
        None => Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "of must be document or a command with a schema, got {}",
                document::field_name(of)
            ),
        )
        .with_parameter("of")
        .with_suggestion(
            "give document, or the name of a command with a schema: the candidates are every \
             one there is so far",
        )
        .with_candidates(
            std::iter::once("document")
                .chain(wire::COMMANDS)
                .map(str::to_owned),
        )
        .into()),
    }
}
