//! `add_feature`, `set_field`, `remove_feature`, `replace_text`, and `move_feature`'s wire
//! types (docs/design/m4.1-command-api.md, sections 5 and 7; docs/design/m5.0-app.md,
//! section 7, for `add_feature`'s `before`, `remove_feature`'s `rewired`, and
//! `move_feature`): their requests, read strictly ([`AddFeatureRequest`],
//! [`SetFieldRequest`], [`RemoveFeatureRequest`], [`ReplaceTextRequest`],
//! [`MoveFeatureRequest`]); their results, written field for field ([`ToWire`]: a feature as
//! the document's canonical form writes it, a field's value as the document holds it, each
//! `base` and `tool` an edit of the chain changed as a [`Rewired`]); and the mirrors their
//! snapshots, `commands/add_feature.json`, `set_field.json`, `remove_feature.json`,
//! `replace_text.json`, and `move_feature.json`, are generated from. A request's shape is
//! read here; the feature an edit gives and the value it sets are read by the document's own
//! reader, in the session, so the wire and a Rust client meet one set of refusals.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};

use super::transaction::CommitResultSchema;
use super::{Json, RequestFields, ToWire, wire_shape, words};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{
    Added, FieldSet, Moved, Problems, Removed, Replaced, Revision, SessionId, Suppressed,
};
use crate::document::chain::Rewired;
use crate::document::json::Node;
use crate::document::{AxisToken, ElementName, Feature, FeatureId};

/// An `add_feature` request: the session, the revision the edit was made against, the
/// feature to add it before (none for the end), and the feature, an object as the document
/// writes it, read by the document's reader when it's added.
#[derive(Debug, Clone, PartialEq)]
pub struct AddFeatureRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub before: Option<FeatureId>,
    pub feature: Node,
}

impl AddFeatureRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let before = f.nullable_feature("before");
            let feature = f.whole_object("feature");
            Some(AddFeatureRequest {
                session: session?,
                revision: revision?,
                before: before?,
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
            (
                "before",
                Json::or_null(self.before.as_ref(), ToWire::to_wire),
            ),
            ("feature", Json::node(&self.feature)),
        ])
    }
}

/// A `move_feature` request: the session, the revision the edit was made against, the
/// feature to move, and the feature to move it before (none for the end).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveFeatureRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub feature: FeatureId,
    pub before: Option<FeatureId>,
}

impl MoveFeatureRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let feature = f.feature("feature");
            let before = f.nullable_feature("before");
            Some(MoveFeatureRequest {
                session: session?,
                revision: revision?,
                feature: feature?,
                before: before?,
            })
        })
    }
}

impl ToWire for MoveFeatureRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("feature", self.feature.to_wire()),
            (
                "before",
                Json::or_null(self.before.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

/// A `suppress_feature` request: the session, the revision the edit was made against, the
/// feature, and whether to suppress it (false: unsuppress it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuppressFeatureRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub feature: FeatureId,
    pub suppressed: bool,
}

impl SuppressFeatureRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let feature = f.feature("feature");
            let suppressed = f.bool("suppressed");
            Some(SuppressFeatureRequest {
                session: session?,
                revision: revision?,
                feature: feature?,
                suppressed: suppressed?,
            })
        })
    }
}

impl ToWire for SuppressFeatureRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("feature", self.feature.to_wire()),
            ("suppressed", Json::bool(self.suppressed)),
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
            ("feature", self.feature.to_wire()),
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
            ("feature", self.feature.to_wire()),
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

impl ToWire for Rewired {
    /// A feature whose `base` or `tool` an edit changed: the feature, the field, what it
    /// named before the edit, and what it names after (`null` for none).
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("feature", self.feature.to_wire()),
            ("field", Json::str(self.field)),
            ("from", Json::or_null(self.from.as_ref(), ToWire::to_wire)),
            ("to", Json::or_null(self.to.as_ref(), ToWire::to_wire)),
        ])
    }
}

/// Each of `rewired`, in its order.
fn rewired(rewired: &[Rewired]) -> Json<'_> {
    Json::array(rewired.iter().map(ToWire::to_wire))
}

impl ToWire for Added {
    /// `add_feature`'s result: the feature as the document now holds it, its place, what it
    /// rewired, whether the document changed, and the commit outside a transaction.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("feature", Json::feature(&self.feature)),
            ("index", Json::count(self.index)),
            ("rewired", rewired(&self.rewired)),
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
            ("feature", self.feature.to_wire()),
            ("field", Json::str(&self.field)),
            ("before", Json::field(&self.field, &self.before)),
            ("after", Json::field(&self.field, &self.after)),
            ("changed", Json::bool(self.changed)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

impl ToWire for Removed {
    /// `remove_feature`'s result: the feature as the document held it, its place, what its
    /// removal rewired, whether the document changed, and the commit outside a transaction.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("feature", Json::feature(&self.feature)),
            ("index", Json::count(self.index)),
            ("rewired", rewired(&self.rewired)),
            ("changed", Json::bool(self.changed)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

impl ToWire for Moved {
    /// `move_feature`'s result: the feature, where it was and where it is, what it rewired,
    /// whether the document changed, and the commit outside a transaction.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("feature", self.feature.to_wire()),
            ("from", Json::count(self.from)),
            ("to", Json::count(self.to)),
            ("rewired", rewired(&self.rewired)),
            ("changed", Json::bool(self.changed)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

impl ToWire for Suppressed {
    /// `suppress_feature`'s result: the feature, what it suppressed and unsuppressed, whether
    /// the document changed, and the commit outside a transaction.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("feature", self.feature.to_wire()),
            ("suppressed", Json::ids(&self.suppressed)),
            ("unsuppressed", Json::ids(&self.unsuppressed)),
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
            ("features", Json::count(self.features)),
            ("before", Json::or_null(self.before.as_deref(), Json::str)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

/// A feature to add: one as the document writes it, as the document's reader takes it for
/// `add_feature` (`edit::add_feature`): its `id` and `v` left out if wanted (the core makes an
/// ID, and gives the type's current version), and no `fingerprint`; and `null` for any
/// member it may leave out (its `id`, `v`, and `note`, a pad's or revolve's `base`, and its
/// fingerprint, which it can only leave out), taken as left out.
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
                    "A feature as the document writes it, to add to the timeline. \
                     Its id may be left out, for one the core makes that no feature holds, and \
                     its v, for the type's current version. A fingerprint is refused, but for \
                     null: a commit writes it. Each member the feature may leave out (its id, \
                     v, note, and fingerprint, and a pad's or revolve's base) may be given as \
                     null, which is the same as leaving it out."
                        .to_owned(),
                ),
            );
            if let Some(serde_json::Value::Object(properties)) = object.get_mut("properties") {
                for (key, said) in [
                    (
                        "id",
                        Some(
                            "Its ID: null, or left out, for one the core makes that no feature holds.",
                        ),
                    ),
                    (
                        "v",
                        Some("Its version: null, or left out, for the type's current version."),
                    ),
                    ("note", None),
                ] {
                    if let Some(property) = properties.get_mut(key) {
                        null_left_out(property, said);
                    }
                }
                properties.insert(
                    "fingerprint".to_owned(),
                    serde_json::json!({
                        "description": "Only null, which is the same as leaving it out: a \
                                        commit writes a feature's fingerprint.",
                        "type": "null",
                    }),
                );
            }
            if let Some(serde_json::Value::Array(required)) = object.get_mut("required") {
                required.retain(|key| key != "id" && key != "v");
            }
            if let Some(serde_json::Value::Array(types)) = object.get_mut("oneOf") {
                for variant in types {
                    let type_name = variant
                        .pointer("/properties/type/const")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    let Some(type_name) = type_name else {
                        continue;
                    };
                    let Some(serde_json::Value::Object(properties)) = variant.get_mut("properties")
                    else {
                        continue;
                    };
                    for key in crate::document::optional_members(&type_name) {
                        if let Some(property) = properties.get_mut(*key) {
                            null_left_out(property, None);
                        }
                    }
                    if type_name == "sketch"
                        && let Some(plane) = properties.get_mut("plane")
                    {
                        *plane = plane_request(generator);
                    }
                }
            }
        }
        schema
    }
}

/// A new sketch's `plane`: an origin plane's word, or a face plane whose `x` may be left
/// out ([`FacePlaneRequestSchema`]).
fn plane_request(generator: &mut SchemaGenerator) -> serde_json::Value {
    let origin = generator.subschema_for::<crate::document::OriginPlane>();
    let face = generator.subschema_for::<FacePlaneRequestSchema>();
    serde_json::json!({
        "anyOf": [origin, face],
        "description": "The plane the sketch is drawn on: one of the origin's, by its word \
                        (D6), or a planar face of the model, by its name, with the axis the \
                        sketch's x follows, which may be left out for the face's default, \
                        and whether it's flipped (docs/ports/attacher.md, F1 and F2; M5.9).",
    })
}

/// A face plane as `add_feature` takes it (M4.1 section 5): the document's face plane, but
/// its `x` may be left out, or given as null, for the face's default (F1), which the core
/// works out and writes. The face is looked for in the model of the document the sketch is
/// added to, in the solids live at the sketch's place, so the edit waits for that model.
/// The document always holds `x`.
#[derive(JsonSchema)]
#[schemars(rename = "FacePlaneRequest", deny_unknown_fields)]
pub struct FacePlaneRequestSchema {
    /// The face, by its name, as Silico names it (`k7f2qx3m.box.Face6`), of a feature
    /// before the sketch: one of the solids live at the sketch's place must hold it, by a
    /// stable name, and OCCT must type it a plane, for its default x to be known.
    pub face: ElementName,
    /// The global axis the sketch's x follows: projected onto the face, it's the sketch's
    /// x. Left out, or null, for the face's `default_x` (F1), as `elements` reports it,
    /// which the core writes.
    #[schemars(default)]
    pub x: Option<AxisToken>,
    /// Whether the sketch is flipped: its normal into the face's solid, not out of it, and
    /// its x against the axis's projection; its y kept.
    pub reversed: bool,
}

/// `property`, a member a new feature may leave out, taking `null` too, which the document's
/// reader takes as the member left out: described as `said`, if given, or else by its own
/// description and that null is the same as left out.
fn null_left_out(property: &mut serde_json::Value, said: Option<&str>) {
    let mut given = property.take();
    let description = given
        .as_object_mut()
        .and_then(|given| given.remove("description"))
        .and_then(|description| description.as_str().map(str::to_owned));
    let said = match (said, description) {
        (Some(said), _) => said.to_owned(),
        (None, Some(description)) => format!("{description} Null is the same as left out."),
        (None, None) => "Null is the same as left out.".to_owned(),
    };
    *property = serde_json::json!({
        "anyOf": [given, {"type": "null"}],
        "description": said,
    });
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

/// `add_feature`'s request: add a feature to the session's timeline, at the end or before
/// another.
#[derive(JsonSchema)]
#[schemars(rename = "AddFeatureRequest", transform = wire_shape)]
pub struct AddFeatureRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`, so it never writes over a change it didn't see.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The feature to add it before, by ID, or null for the end. Before a feature, each
    /// input it takes (its `base`, its `tool`) that a feature at or after that place
    /// consumed is consumed by it instead: that feature's field now names the new one,
    /// listed in `rewired`. One consumed by a feature before the place is refused.
    pub before: Option<FeatureId>,
    pub feature: NewFeatureSchema,
}

/// A feature whose `base` or `tool` an edit changed, which the edit reaches.
#[derive(JsonSchema)]
#[schemars(rename = "Rewired", transform = wire_shape)]
pub struct RewiredSchema {
    /// The feature, by ID.
    pub feature: FeatureId,
    pub field: InputFieldSchema,
    /// What the field named before the edit: null where the feature had no base.
    pub from: Option<FeatureId>,
    /// What it names after: null where the feature is left with no base (a pad or revolve,
    /// which then begins a body).
    pub to: Option<FeatureId>,
}

/// Which of a feature's inputs an edit changed: its `base` or its `tool`.
pub struct InputFieldSchema;

impl JsonSchema for InputFieldSchema {
    fn schema_name() -> Cow<'static, str> {
        "InputField".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "Which of the feature's inputs changed: its base, or a boolean's tool.",
            ["base", "tool"],
        )
    }
}

/// `add_feature`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "AddFeatureResult", transform = wire_shape)]
pub struct AddFeatureResultSchema {
    /// The feature as the document now holds it: with its ID (the one given, or the one
    /// made) and its version; outside a transaction, as the commit saved it, with the
    /// fingerprint it wrote.
    pub feature: Feature,
    /// Its place in the timeline, from 0.
    pub index: u64,
    /// Each feature whose `base` or `tool` now names the new feature instead of the input
    /// it consumes, in timeline order: empty at the end.
    pub rewired: Vec<RewiredSchema>,
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
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
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

/// `remove_feature`'s request: remove one feature, the feature that consumed it taking its
/// base.
#[derive(JsonSchema)]
#[schemars(rename = "RemoveFeatureRequest", transform = wire_shape)]
pub struct RemoveFeatureRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The feature to remove, by ID. The feature that consumed it takes its base. Refused,
    /// naming both, where nothing can stand in: a chain's root under a feature that needs a
    /// base, a feature whose sketch it is, or one that names its elements.
    pub feature: FeatureId,
}

/// `remove_feature`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "RemoveFeatureResult", transform = wire_shape)]
pub struct RemoveFeatureResultSchema {
    /// The feature removed, as the document held it, its fingerprint too. To put it back,
    /// give `add_feature` this feature without its `fingerprint`, `before` the feature that
    /// followed it (inside a transaction, the fingerprint `begin`'s document stored is
    /// kept); then, with `set_field`, set `base` back on each
    /// `rewired` entry whose `to` is null. Or undo the removal whole: remove inside a
    /// transaction (`begin`), and `rollback`.
    pub feature: Feature,
    /// Its place in the timeline, from 0.
    pub index: u64,
    /// Each feature whose `base` or `tool` named the removed feature and now names what it
    /// named (null where it named none, a pad or revolve then beginning a body), in
    /// timeline order.
    pub rewired: Vec<RewiredSchema>,
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
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
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

/// `move_feature`'s request: move one feature to another place in the timeline, rewiring the
/// chains it leaves and joins.
#[derive(JsonSchema)]
#[schemars(rename = "MoveFeatureRequest", transform = wire_shape)]
pub struct MoveFeatureRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The feature to move, by ID. Its consumer takes its base; it takes as its base the
    /// last feature of its chain before the new place, and the chain's next feature after
    /// that place takes it as its base. Refused, naming both, where a child would come
    /// before its parent: before its sketch or its tool, after a feature using it as its
    /// sketch, or where a feature's base would no longer be built from a feature whose
    /// elements it names.
    pub feature: FeatureId,
    /// The feature to move it before, by ID, or null for the end.
    pub before: Option<FeatureId>,
}

/// `move_feature`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "MoveFeatureResult", transform = wire_shape)]
pub struct MoveFeatureResultSchema {
    /// The feature moved, as the request named it.
    pub feature: FeatureId,
    /// Its place in the timeline before the move, from 0.
    pub from: u64,
    /// Its place after the move, from 0, counted in the timeline after it.
    pub to: u64,
    /// Each feature whose `base` or `tool` the move changed, in timeline order after the
    /// move: empty for a move that leaves it between its base and its consumer.
    pub rewired: Vec<RewiredSchema>,
    /// Whether the document changed: false for a move to where it is, which makes no new
    /// revision and writes nothing.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one, or if
    /// nothing changed.
    pub commit: Option<CommitResultSchema>,
}

/// `suppress_feature`'s request: suppress a feature with its dependents, or unsuppress it
/// with what it needs.
#[derive(JsonSchema)]
#[schemars(rename = "SuppressFeatureRequest", transform = wire_shape)]
pub struct SuppressFeatureRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The feature to suppress or unsuppress, by ID.
    pub feature: FeatureId,
    /// True to suppress it: it isn't built, and a feature that consumes it builds on the
    /// nearest unsuppressed feature up its chain; its dependents go with it (a feature whose
    /// sketch it is, one naming its elements, and one left with an input it needs and nothing
    /// unsuppressed up its chain). False to unsuppress it, with its sketch, the makers of the
    /// names it stores, and, for an input it needs with nothing unsuppressed up its chain,
    /// that chain's first feature; what depends on it stays suppressed.
    pub suppressed: bool,
}

/// `suppress_feature`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "SuppressFeatureResult", transform = wire_shape)]
pub struct SuppressFeatureResultSchema {
    /// The feature, as the request named it.
    pub feature: FeatureId,
    /// The features the edit suppressed, in timeline order, the feature itself among them:
    /// empty for an unsuppression, or for a feature suppressed already.
    pub suppressed: Vec<FeatureId>,
    /// The features the edit unsuppressed, in timeline order, the feature itself among them:
    /// empty for a suppression, or for a feature that isn't suppressed.
    pub unsuppressed: Vec<FeatureId>,
    /// Whether the document changed: false for a feature suppressed already, or unsuppressed
    /// that isn't, which makes no new revision and writes nothing.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit (which carries a suppressed
    /// feature's stored fingerprint as stored, never checked or rewritten, and writes a fresh
    /// one for each feature that uses it); null inside one, or if nothing changed.
    pub commit: Option<CommitResultSchema>,
}
