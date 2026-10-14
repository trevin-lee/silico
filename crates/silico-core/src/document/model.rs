//! A `.silico` document's types (docs/design/m2.1-document-format.md). The JSON Schema
//! agents get is generated from them; the reader and writer are Silico's own (`decode`,
//! `write`), so the `serde` attributes here only shape the schema.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

use super::quantity::Length;

/// A Silico document: a part's feature timeline, rebuilt from this text.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "Silico document")]
pub struct Document {
    /// The format's version.
    pub silico: FormatVersion,
    /// The units every length and angle is written in.
    pub units: Units,
    /// The features, in timeline order: each may use only features before it.
    pub features: Vec<Feature>,
}

/// The format's version: 1. A document of another version is refused, naming it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FormatVersion;

impl FormatVersion {
    pub const NUMBER: u32 = 1;
}

impl JsonSchema for FormatVersion {
    fn schema_name() -> Cow<'static, str> {
        "FormatVersion".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "The format's version: 1.",
            "const": 1
        })
    }
}

/// The units header. Version 1 reads only millimetres at a resolution of 1 nm (angles in
/// degrees at 1 nanodegree).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Units;

impl Units {
    pub const LENGTH: &'static str = "mm";
    pub const RESOLUTION: &'static str = "1nm";
}

impl JsonSchema for Units {
    fn schema_name() -> Cow<'static, str> {
        "Units".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "The units lengths and angles are written in. Version 1 reads only millimetres at 1 nm (angles in degrees at 1 nanodegree).",
            "type": "object",
            "properties": {
                "length": {"const": "mm"},
                "resolution": {"const": "1nm"}
            },
            "required": ["length", "resolution"],
            "additionalProperties": false
        })
    }
}

/// A feature's ID: 8 characters of Crockford's base32 in lower case, unique within the
/// document. Other features refer to it by this.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeatureId(String);

/// The characters of an ID: Crockford's base32, lower case (no `i`, `l`, `o`, or `u`).
pub const ID_ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";

/// How many characters an ID has.
pub const ID_LENGTH: usize = 8;

impl FeatureId {
    /// The ID `text` writes, if it's one.
    pub fn new(text: &str) -> Option<Self> {
        let ok = text.chars().count() == ID_LENGTH && text.chars().all(|c| ID_ALPHABET.contains(c));
        ok.then(|| FeatureId(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FeatureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl JsonSchema for FeatureId {
    fn schema_name() -> Cow<'static, str> {
        "FeatureId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A feature's ID: 8 characters of Crockford's base32 in lower case (digits and letters but i, l, o, and u).",
            "type": "string",
            "pattern": "^[0-9a-hjkmnp-tv-z]{8}$"
        })
    }
}

/// A point, `[x, y, z]`, in millimetres.
pub type Position = [Length; 3];

/// One feature of the timeline: what it is, its version, and its name.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    pub id: FeatureId,
    #[serde(flatten)]
    pub operation: Operation,
    #[serde(rename = "v")]
    pub version: FeatureVersion,
    pub name: Name,
    /// A note about the feature, kept with it (left out when there's none).
    #[schemars(extend("type" = "string"))]
    pub note: Option<String>,
}

/// A feature's version: changing a feature type's behaviour bumps it, and a document keeps
/// the behaviour it was made with. 1 for every type so far, so it holds no other value: a
/// document in memory can hold only what the file can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FeatureVersion;

impl FeatureVersion {
    pub const NUMBER: u32 = 1;
}

impl JsonSchema for FeatureVersion {
    fn schema_name() -> Cow<'static, str> {
        "FeatureVersion".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "The feature's version: changing a feature type's behaviour bumps it, and a document keeps the behaviour it was made with. 1 for every type so far.",
            "const": 1
        })
    }
}

/// A feature's display name: any text but none, and needn't be unique.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(String);

impl Name {
    /// The name `text` is, if it isn't empty.
    pub fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into();
        (!text.is_empty()).then_some(Name(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl JsonSchema for Name {
    fn schema_name() -> Cow<'static, str> {
        "Name".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A display name, which needn't be unique.",
            "type": "string",
            "minLength": 1
        })
    }
}

/// What a feature makes. A primitive makes a solid; a boolean makes one from two earlier
/// features' solids, consuming them.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Operation {
    /// A box from `position` (its lowest corner), `length` along x, `width` along y, and
    /// `height` along z.
    Box {
        position: Position,
        length: Length,
        width: Length,
        height: Length,
    },
    /// A cylinder along z, its base centred on `position`.
    Cylinder {
        position: Position,
        radius: Length,
        height: Length,
    },
    /// A sphere centred on `position`.
    Sphere { position: Position, radius: Length },
    /// A cone along z, its base (`radius1`) centred on `position`, `radius2` at `height`.
    /// Either radius may be zero, not both.
    Cone {
        position: Position,
        radius1: Length,
        radius2: Length,
        height: Length,
    },
    /// A torus around z, centred on `position`: `radius1` from the axis to the tube's
    /// centre, `radius2` the tube's.
    Torus {
        position: Position,
        radius1: Length,
        radius2: Length,
    },
    /// Everything in either of two features' solids.
    Fuse { base: FeatureId, tool: FeatureId },
    /// `base`'s solid less what's in `tool`'s.
    Cut { base: FeatureId, tool: FeatureId },
    /// What's in both of two features' solids.
    Common { base: FeatureId, tool: FeatureId },
}

impl Operation {
    /// The `type` the file writes.
    pub fn type_name(&self) -> &'static str {
        match self {
            Operation::Box { .. } => "box",
            Operation::Cylinder { .. } => "cylinder",
            Operation::Sphere { .. } => "sphere",
            Operation::Cone { .. } => "cone",
            Operation::Torus { .. } => "torus",
            Operation::Fuse { .. } => "fuse",
            Operation::Cut { .. } => "cut",
            Operation::Common { .. } => "common",
        }
    }
}

/// Every feature type, as the file writes it.
pub const FEATURE_TYPES: [&str; 8] = [
    "box", "cylinder", "sphere", "cone", "torus", "fuse", "cut", "common",
];
