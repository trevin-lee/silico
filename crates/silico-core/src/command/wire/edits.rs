//! `add_feature`, `set_field`, `remove_feature`, and `replace_text`'s wire types
//! (docs/design/m4.1-command-api.md, sections 5 and 7): their requests, read strictly
//! ([`AddFeatureRequest`], [`SetFieldRequest`], [`RemoveFeatureRequest`],
//! [`ReplaceTextRequest`]); their results, written field for field ([`ToWire`]: a feature as
//! the document's canonical form writes it, a field's value as the document holds it); and
//! the mirrors their snapshots, `commands/add_feature.json`, `set_field.json`,
//! `remove_feature.json`, and `replace_text.json`, are generated from. A request's shape is
//! read here; the feature an edit gives and the value it sets are read by the document's own
//! reader, in the session, so the wire and a Rust client meet one set of refusals.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};

use super::transaction::CommitResultSchema;
use super::{Json, RequestFields, ToWire, wire_shape};
use crate::command::{Added, FieldSet, Problems, Removed, Replaced, Revision, SessionId};
use crate::document::json::Node;
use crate::document::{Feature, FeatureId};

/// An `add_feature` request: the session, the revision the edit was made against, and the
/// feature, as the document writes it, read by the document's reader when it's added.
#[derive(Debug, Clone, PartialEq)]
pub struct AddFeatureRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub feature: Node,
}

impl AddFeatureRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let feature = f.node("feature");
            Some(AddFeatureRequest {
                session: session?,
                revision: revision?,
                feature: feature?.clone(),
            })
        })
    }
}

impl ToWire for AddFeatureRequest {
    /// The request as a client writes it, every field in the shape's order, the feature's
    /// members in theirs.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("feature", Json::node(&self.feature)),
        ])
    }
}

/// A `set_field` request: the session, the revision the edit was made against, the feature,
/// the field's path, and its value, read by the document's reader as that field is.
#[derive(Debug, Clone, PartialEq)]
pub struct SetFieldRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub feature: FeatureId,
    pub field: String,
    pub value: Node,
}

impl SetFieldRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let feature = f.feature("feature");
            let field = f.string("field");
            let value = f.node("value");
            Some(SetFieldRequest {
                session: session?,
                revision: revision?,
                feature: feature?,
                field: field?.to_owned(),
                value: value?.clone(),
            })
        })
    }
}

impl ToWire for SetFieldRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("feature", Json::str(self.feature.as_str())),
            ("field", Json::str(&self.field)),
            ("value", Json::node(&self.value)),
        ])
    }
}

/// A `remove_feature` request: the session, the revision the edit was made against, and the
/// feature.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoveFeatureRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub feature: FeatureId,
}

impl RemoveFeatureRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let feature = f.feature("feature");
            Some(RemoveFeatureRequest {
                session: session?,
                revision: revision?,
                feature: feature?,
            })
        })
    }
}

impl ToWire for RemoveFeatureRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("feature", Json::str(self.feature.as_str())),
        ])
    }
}

/// A `replace_text` request: the session, the revision the edit was made against, and the
/// document's whole new text.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplaceTextRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub text: String,
}

impl ReplaceTextRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let text = f.string("text");
            Some(ReplaceTextRequest {
                session: session?,
                revision: revision?,
                text: text?.to_owned(),
            })
        })
    }
}

impl ToWire for ReplaceTextRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("text", Json::str(&self.text)),
        ])
    }
}

impl ToWire for Added {
    /// `add_feature`'s result: the feature as the document now holds it, whether the document
    /// changed, and the commit outside a transaction.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("feature", Json::feature(&self.feature)),
            ("changed", Json::bool(self.changed)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

impl ToWire for FieldSet {
    /// `set_field`'s result: the feature and field, the field's value before and after,
    /// whether the document changed, and the commit outside a transaction.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("feature", Json::str(self.feature.as_str())),
            ("field", Json::str(&self.field)),
            ("before", Json::plain(&self.before)),
            ("after", Json::plain(&self.after)),
            ("changed", Json::bool(self.changed)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

impl ToWire for Removed {
    /// `remove_feature`'s result: the feature as the document held it, its place, whether
    /// the document changed, and the commit outside a transaction.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("feature", Json::feature(&self.feature)),
            ("index", Json::whole(self.index as u64)),
            ("changed", Json::bool(self.changed)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

impl ToWire for Replaced {
    /// `replace_text`'s result: whether the document changed, its number of features, and,
    /// outside a transaction, the text replaced and the commit.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("changed", Json::bool(self.changed)),
            ("features", Json::whole(self.features as u64)),
            ("before", Json::or_null(self.before.as_deref(), Json::str)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

/// A feature to add: one as the document writes it, its `id` and `v` left out if wanted
/// (the core makes an ID, and gives the type's current version), and no `fingerprint`.
pub struct NewFeatureSchema;

impl JsonSchema for NewFeatureSchema {
    fn schema_name() -> Cow<'static, str> {
        "NewFeature".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = <Feature as JsonSchema>::json_schema(generator);
        if let Some(object) = schema.as_object_mut() {
            object.insert(
                "description".to_owned(),
                serde_json::Value::String(
                    "A feature as the document writes it, to add at the end of the timeline. \
                     Its id may be left out, for one the core makes that no feature holds, and \
                     its v, for the type's current version. A fingerprint is refused: a commit \
                     writes it."
                        .to_owned(),
                ),
            );
            if let Some(serde_json::Value::Object(properties)) = object.get_mut("properties") {
                properties.remove("fingerprint");
            }
            if let Some(serde_json::Value::Array(required)) = object.get_mut("required") {
                required.retain(|key| key != "id" && key != "v");
            }
        }
        schema
    }
}

/// A value of a document's, any JSON: a field's, as the document's reader reads that field
/// and its canonical form writes it.
pub struct FieldValueSchema;

impl JsonSchema for FieldValueSchema {
    fn schema_name() -> Cow<'static, str> {
        "FieldValue".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "description": "A field's value, as the document writes that field: a number, a \
                            string, true or false, a list, an object, or null (for a field \
                            the feature hasn't, or a note removed)."
        })
    }
}

/// `add_feature`'s request: add a feature at the end of the session's timeline.
#[derive(JsonSchema)]
#[schemars(rename = "AddFeatureRequest", transform = wire_shape)]
pub struct AddFeatureRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = r"^s[1-9][0-9]*$"))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`, so it never writes over a change it didn't see.
    #[schemars(range(min = 1))]
    pub revision: u64,
    pub feature: NewFeatureSchema,
}

/// `add_feature`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "AddFeatureResult", transform = wire_shape)]
pub struct AddFeatureResultSchema {
    /// The feature as the document now holds it: with its ID (the one given, or the one
    /// made) and its version; outside a transaction, as the commit saved it, with the
    /// fingerprint it wrote.
    pub feature: Feature,
    /// Whether the document changed: always, a feature added.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one.
    pub commit: Option<CommitResultSchema>,
}

/// `set_field`'s request: set one field of one feature.
#[derive(JsonSchema)]
#[schemars(rename = "SetFieldRequest", transform = wire_shape)]
pub struct SetFieldRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = r"^s[1-9][0-9]*$"))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The feature, by ID.
    pub feature: FeatureId,
    /// The field's dotted path (`extent.length`, `position.2`, `note`): a sketch's points,
    /// lines, circles, arcs, and constraints by their IDs (`constraints.g4h5j6k7.value`),
    /// any other list's items by place, from 0. A feature's id, type, v, and fingerprint
    /// can't be set.
    pub field: String,
    /// The field's whole new value, read as the document reads that field: to add a circle
    /// to a sketch, set its `circles`. A note set to null is removed.
    pub value: FieldValueSchema,
}

/// `set_field`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "SetFieldResult", transform = wire_shape)]
pub struct SetFieldResultSchema {
    /// The feature, as the request named it.
    pub feature: FeatureId,
    /// The field, as the request named it.
    pub field: String,
    /// The field's value before, as the document wrote it: null for a field the feature
    /// hadn't.
    pub before: FieldValueSchema,
    /// The field's value after, as the document writes it: outside a transaction, as the
    /// commit saved it (a sketch's member where its solve put it).
    pub after: FieldValueSchema,
    /// Whether the document changed: false for a field given the value it had, which makes
    /// no new revision and writes nothing.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one, or if
    /// nothing changed.
    pub commit: Option<CommitResultSchema>,
}

/// `remove_feature`'s request: remove one feature, which no other may use.
#[derive(JsonSchema)]
#[schemars(rename = "RemoveFeatureRequest", transform = wire_shape)]
pub struct RemoveFeatureRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = r"^s[1-9][0-9]*$"))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The feature to remove, by ID. A feature others use is refused, each naming it:
    /// remove those first.
    pub feature: FeatureId,
}

/// `remove_feature`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "RemoveFeatureResult", transform = wire_shape)]
pub struct RemoveFeatureResultSchema {
    /// The feature removed, as the document held it: `replace_text` can put it back.
    pub feature: Feature,
    /// Its place in the timeline, from 0.
    pub index: u64,
    /// Whether the document changed: always, a feature removed.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one.
    pub commit: Option<CommitResultSchema>,
}

/// `replace_text`'s request: replace the document's whole text, read and checked as
/// `silico check` reads a file.
#[derive(JsonSchema)]
#[schemars(rename = "ReplaceTextRequest", transform = wire_shape)]
pub struct ReplaceTextRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = r"^s[1-9][0-9]*$"))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The document's whole new text: strict JSON, its header with `naming`.
    pub text: String,
}

/// `replace_text`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "ReplaceTextResult", transform = wire_shape)]
pub struct ReplaceTextResultSchema {
    /// Whether the document changed: false for the document the session holds, in whatever
    /// layout, which makes no new revision and writes nothing.
    pub changed: bool,
    /// How many features the document has.
    pub features: u64,
    /// Outside a transaction, the text replaced, so it can be put back; null inside one,
    /// where `rollback` restores it.
    pub before: Option<String>,
    /// Outside a transaction, the commit that saved the edit; null inside one, or if
    /// nothing changed.
    pub commit: Option<CommitResultSchema>,
}
