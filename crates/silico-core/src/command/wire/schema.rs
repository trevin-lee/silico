//! `schema`'s wire types (docs/design/m4.1-command-api.md, section 7): its request, read
//! strictly; its result, the schema written as the writer writes any JSON Silico didn't
//! build field by field; and the mirrors its snapshot, `commands/schema.json`, is generated
//! from.

use schemars::JsonSchema;

use super::{Json, RequestFields, ToWire, wire_shape};
use crate::command::Problems;
use crate::command::schema::Described;
use crate::document::json::Node;

/// A `schema` request: what to describe.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaRequest {
    pub of: String,
}

impl SchemaRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let of = f.string("of");
            Some(SchemaRequest { of: of?.to_owned() })
        })
    }
}

impl ToWire for SchemaRequest {
    fn to_wire(&self) -> Json<'_> {
        Json::object([("of", Json::str(&self.of))])
    }
}

impl ToWire for Described {
    fn to_wire(&self) -> Json<'_> {
        Json::object([("schema", Json::value(&self.schema))])
    }
}

/// `schema`'s request: the document's JSON Schema, or a command's.
#[derive(JsonSchema)]
#[schemars(rename = "SchemaRequest", transform = wire_shape)]
pub struct SchemaRequestSchema {
    /// `document`, or a command's name (`elements`, `query`, ...): a name that's neither is
    /// refused, with every name `schema` describes as candidates.
    pub of: String,
}

/// `schema`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "SchemaResult", transform = wire_shape)]
pub struct SchemaResultSchema {
    /// The document's JSON Schema (draft 2020-12), or a command's,
    /// `{"request": ..., "result": ...}`, two JSON Schemas: exactly what the protected
    /// snapshots hold.
    pub schema: serde_json::Map<String, serde_json::Value>,
}
