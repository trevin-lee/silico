//! A `.silico` document's types (docs/design/m2.1-document-format.md). The JSON Schema
//! agents get is generated from them; the reader and writer are Silico's own (`decode`,
//! `write`), so the `serde` attributes here only shape the schema.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use silico_protocol::{BooleanOp, Primitive};

use super::fingerprint::Fingerprint;
use super::quantity::{Angle, Length};
use super::sketch::{ElementId, Reference, Sketch};

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
        let pattern = format!("^[{ID_ALPHABET}]{{{ID_LENGTH}}}$");
        json_schema!({
            "description": "A feature's ID: 8 characters of Crockford's base32 in lower case (digits and letters but i, l, o, and u).",
            "type": "string",
            "pattern": pattern
        })
    }
}

/// A point, `[x, y, z]`, in millimetres.
pub type Position = [Length; 3];

/// An element of a feature's solid, by its name in Silico's form as written
/// (docs/ports/naming.md, `m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)`):
/// never an element's number, which an upstream edit can make another's. The gate reads it back
/// (`naming::Name::parse`), knowing what each feature before it is; recompute finds the
/// element it names, or fails the feature saying so.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ElementName(String);

impl ElementName {
    /// The name `text` writes, if it's not empty (whether it's a name, and whose, is the
    /// gate's to say).
    pub fn new(text: &str) -> Option<Self> {
        (!text.is_empty()).then(|| ElementName(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl JsonSchema for ElementName {
    fn schema_name() -> Cow<'static, str> {
        "ElementName".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "An element of an earlier feature's solid, by its name in Silico's form (feature.type.Kind(...)), never by its number.",
            "type": "string",
            "minLength": 1
        })
    }
}

/// How far a chamfer cuts each edge back: along both its faces alike. (The uneven forms,
/// measured from one of each edge's faces, are left for later: which face is the document's
/// to name, not OCCT's order to pick.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChamferSize {
    pub distance: Length,
}

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
    /// What the feature's solid measured when it was last built and saved; a rebuild that
    /// measures otherwise is drift (left out when it has none: new, or not built).
    #[serde(default)]
    #[schemars(with = "Fingerprint")]
    pub fingerprint: Option<Fingerprint>,
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
/// features' solids, consuming them; a sketch makes no solid; a pad, pocket, revolve, or
/// groove sweeps a sketch's profile into a solid, and fuses it with its base or cuts it from
/// it, consuming the base and using the sketch (docs/design/m3.6-sketch-features.md, S1).
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
    /// A constrained 2D sketch (docs/design/m3.2-sketch-format.md).
    Sketch(Sketch),
    /// `base`'s solid with `edges` rounded to `radius`.
    Fillet {
        base: FeatureId,
        #[schemars(length(min = 1))]
        edges: Vec<ElementName>,
        radius: Length,
    },
    /// `base`'s solid with `edges` cut back by `size`.
    Chamfer {
        base: FeatureId,
        #[schemars(length(min = 1))]
        edges: Vec<ElementName>,
        size: ChamferSize,
    },
    /// `sketch`'s profile swept along the sketch's normal as `extent` says, and fused with
    /// `base`'s solid, if it has one (left out when it has none).
    Pad {
        #[serde(default)]
        #[schemars(with = "FeatureId")]
        base: Option<FeatureId>,
        sketch: FeatureId,
        extent: PadExtent,
    },
    /// `sketch`'s profile swept against the sketch's normal as `extent` says, and cut from
    /// `base`'s solid.
    Pocket {
        base: FeatureId,
        sketch: FeatureId,
        extent: PocketExtent,
    },
    /// `sketch`'s profile turned about `axis` as `extent` says, and fused with `base`'s
    /// solid, if it has one (left out when it has none).
    Revolve {
        #[serde(default)]
        #[schemars(with = "FeatureId")]
        base: Option<FeatureId>,
        sketch: FeatureId,
        axis: Axis,
        extent: TurnExtent,
    },
    /// `sketch`'s profile turned about `axis` as `extent` says, and cut from `base`'s solid.
    Groove {
        base: FeatureId,
        sketch: FeatureId,
        axis: Axis,
        extent: TurnExtent,
    },
}

/// How far a pad goes from its sketch's plane: each form holds exactly the values it needs.
/// Lengths are above 0; `reversed` only where it changes the solid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PadExtent {
    /// `length` along the feature's direction, or against it when `reversed`.
    OneSide { length: Length, reversed: bool },
    /// `length / 2` each way from the plane.
    Symmetric { length: Length },
    /// `length` along the feature's direction and `length2` against it.
    TwoSides { length: Length, length2: Length },
}

/// How far a pocket goes from its sketch's plane: a pad's forms, or through everything of
/// its base along its direction (against it when `reversed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PocketExtent {
    /// `length` along the feature's direction, or against it when `reversed`.
    OneSide { length: Length, reversed: bool },
    /// `length / 2` each way from the plane.
    Symmetric { length: Length },
    /// `length` along the feature's direction and `length2` against it.
    TwoSides { length: Length, length2: Length },
    /// Through everything of its base along the feature's direction, or against it when
    /// `reversed`.
    ThroughAll { reversed: bool },
}

/// How far a revolve or groove turns its profile: an angle above 0° and at most 360°.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TurnExtent {
    /// `angle` right-handed about the axis, or left-handed when `reversed`.
    OneSide { angle: Angle, reversed: bool },
    /// `angle / 2` each way from the sketch.
    Symmetric { angle: Angle },
}

/// A pad's, pocket's, revolve's, or groove's extent, for what reads every form alike: the
/// writer, the gate, and the outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extent {
    Pad(PadExtent),
    Pocket(PocketExtent),
    Turn(TurnExtent),
}

impl Extent {
    /// The form's `type`, as the file writes it.
    pub fn type_name(self) -> &'static str {
        match self {
            Extent::Pad(PadExtent::OneSide { .. })
            | Extent::Pocket(PocketExtent::OneSide { .. })
            | Extent::Turn(TurnExtent::OneSide { .. }) => "one_side",
            Extent::Pad(PadExtent::Symmetric { .. })
            | Extent::Pocket(PocketExtent::Symmetric { .. })
            | Extent::Turn(TurnExtent::Symmetric { .. }) => "symmetric",
            Extent::Pad(PadExtent::TwoSides { .. })
            | Extent::Pocket(PocketExtent::TwoSides { .. }) => "two_sides",
            Extent::Pocket(PocketExtent::ThroughAll { .. }) => "through_all",
        }
    }

    /// The form's values after its `type`, in the order the file writes them, each by its
    /// field.
    pub fn values(self) -> Vec<(&'static str, ExtentValue)> {
        use ExtentValue::{Angle as A, Length as L, Reversed as R};
        match self {
            Extent::Pad(PadExtent::OneSide { length, reversed })
            | Extent::Pocket(PocketExtent::OneSide { length, reversed }) => {
                vec![("length", L(length)), ("reversed", R(reversed))]
            }
            Extent::Pad(PadExtent::Symmetric { length })
            | Extent::Pocket(PocketExtent::Symmetric { length }) => vec![("length", L(length))],
            Extent::Pad(PadExtent::TwoSides { length, length2 })
            | Extent::Pocket(PocketExtent::TwoSides { length, length2 }) => {
                vec![("length", L(length)), ("length2", L(length2))]
            }
            Extent::Pocket(PocketExtent::ThroughAll { reversed }) => {
                vec![("reversed", R(reversed))]
            }
            Extent::Turn(TurnExtent::OneSide { angle, reversed }) => {
                vec![("angle", A(angle)), ("reversed", R(reversed))]
            }
            Extent::Turn(TurnExtent::Symmetric { angle }) => vec![("angle", A(angle))],
        }
    }
}

/// One of an extent's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtentValue {
    Length(Length),
    Angle(Angle),
    Reversed(bool),
}

/// What a revolve or groove turns about: the sketch's x axis (from its origin along +x), its
/// y axis (along +y), or a line of the sketch, construction or not, from its `from` point to
/// its `to`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Axis {
    XAxis,
    YAxis,
    Line(ElementId),
}

impl Axis {
    /// The axis `text` writes, if it's one: `x_axis`, `y_axis`, or an element's ID (whether
    /// it's a line of the sketch is the gate's to say).
    pub fn new(text: &str) -> Option<Self> {
        match Reference::new(text)? {
            Reference::XAxis => Some(Axis::XAxis),
            Reference::YAxis => Some(Axis::YAxis),
            Reference::Element(id) => Some(Axis::Line(id)),
            Reference::Origin => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Axis::XAxis => Reference::X_AXIS,
            Axis::YAxis => Reference::Y_AXIS,
            Axis::Line(id) => id.as_str(),
        }
    }
}

impl fmt::Display for Axis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl JsonSchema for Axis {
    fn schema_name() -> Cow<'static, str> {
        "Axis".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let pattern = format!("^([{ID_ALPHABET}]{{{ID_LENGTH}}}|x_axis|y_axis)$");
        json_schema!({
            "description": "What a revolve or groove turns about: x_axis (the sketch's, from its origin along +x), y_axis (along +y), or the ID of a line of the sketch, construction or not, from its `from` point to its `to`.",
            "type": "string",
            "pattern": pattern
        })
    }
}

/// Which of the features that sweep a sketch's profile a feature is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SweepKind {
    Pad,
    Pocket,
    Revolve,
    Groove,
}

impl SweepKind {
    /// The `type` the file writes.
    pub fn word(self) -> &'static str {
        match self {
            SweepKind::Pad => "pad",
            SweepKind::Pocket => "pocket",
            SweepKind::Revolve => "revolve",
            SweepKind::Groove => "groove",
        }
    }
}

/// What a feature makes its solid with ([`Operation::made`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Made {
    /// A primitive, from nothing.
    Primitive(Primitive),
    /// A boolean of its `base` and `tool` features' solids.
    Boolean(BooleanOp),
    /// No solid: a sketch (solved in M3.3).
    Sketch,
    /// Its `base` feature's solid with edges rounded, to the radius.
    Fillet { radius: f64 },
    /// Its `base` feature's solid with edges cut back, by the distance.
    Chamfer { distance: f64 },
    /// A sketch's profile swept, and fused with or cut from its `base`, if it has one.
    Sweep(SweepKind),
}

impl Operation {
    /// What the feature makes its solid with: a primitive from nothing, a boolean of two
    /// earlier solids, or a fillet or chamfer of one; or, for a sketch, no solid. Every
    /// kind of feature is one, so a new kind has to say which (and so how its elements are
    /// named).
    pub fn made(&self) -> Made {
        match *self {
            Operation::Box {
                length,
                width,
                height,
                ..
            } => Made::Primitive(Primitive::Box {
                dx: length.mm(),
                dy: width.mm(),
                dz: height.mm(),
            }),
            Operation::Cylinder { radius, height, .. } => Made::Primitive(Primitive::Cylinder {
                radius: radius.mm(),
                height: height.mm(),
            }),
            Operation::Sphere { radius, .. } => Made::Primitive(Primitive::Sphere {
                radius: radius.mm(),
            }),
            Operation::Cone {
                radius1,
                radius2,
                height,
                ..
            } => Made::Primitive(Primitive::Cone {
                radius1: radius1.mm(),
                radius2: radius2.mm(),
                height: height.mm(),
            }),
            Operation::Torus {
                radius1, radius2, ..
            } => Made::Primitive(Primitive::Torus {
                major_radius: radius1.mm(),
                minor_radius: radius2.mm(),
            }),
            Operation::Fuse { .. } => Made::Boolean(BooleanOp::Fuse),
            Operation::Cut { .. } => Made::Boolean(BooleanOp::Cut),
            Operation::Common { .. } => Made::Boolean(BooleanOp::Common),
            Operation::Sketch(_) => Made::Sketch,
            Operation::Fillet { radius, .. } => Made::Fillet {
                radius: radius.mm(),
            },
            Operation::Chamfer { size, .. } => Made::Chamfer {
                distance: size.distance.mm(),
            },
            Operation::Pad { .. } => Made::Sweep(SweepKind::Pad),
            Operation::Pocket { .. } => Made::Sweep(SweepKind::Pocket),
            Operation::Revolve { .. } => Made::Sweep(SweepKind::Revolve),
            Operation::Groove { .. } => Made::Sweep(SweepKind::Groove),
        }
    }

    /// The primitive the kernel makes for this feature, at the origin, before it's moved to
    /// its [`position`](Self::position); `None` for any other feature.
    pub fn primitive(&self) -> Option<Primitive> {
        match self.made() {
            Made::Primitive(primitive) => Some(primitive),
            Made::Boolean(_)
            | Made::Fillet { .. }
            | Made::Chamfer { .. }
            | Made::Sketch
            | Made::Sweep(_) => None,
        }
    }

    /// Where a primitive sits; `None` for any other feature.
    pub fn position(&self) -> Option<&Position> {
        match self {
            Operation::Box { position, .. }
            | Operation::Cylinder { position, .. }
            | Operation::Sphere { position, .. }
            | Operation::Cone { position, .. }
            | Operation::Torus { position, .. } => Some(position),
            Operation::Fuse { .. }
            | Operation::Cut { .. }
            | Operation::Common { .. }
            | Operation::Sketch(_)
            | Operation::Fillet { .. }
            | Operation::Chamfer { .. }
            | Operation::Pad { .. }
            | Operation::Pocket { .. }
            | Operation::Revolve { .. }
            | Operation::Groove { .. } => None,
        }
    }

    /// The field a [`primitive`](Self::primitive)'s parameter is in the file: a box's `dx`
    /// is its `length`, a torus's `major_radius` its `radius1`.
    pub fn document_field(&self, parameter: &'static str) -> &'static str {
        match self {
            Operation::Box { .. } => match parameter {
                "dx" => "length",
                "dy" => "width",
                "dz" => "height",
                other => other,
            },
            Operation::Torus { .. } => match parameter {
                "major_radius" => "radius1",
                "minor_radius" => "radius2",
                other => other,
            },
            // The kernel's names are the file's.
            Operation::Cylinder { .. }
            | Operation::Sphere { .. }
            | Operation::Cone { .. }
            | Operation::Fuse { .. }
            | Operation::Cut { .. }
            | Operation::Common { .. }
            | Operation::Sketch(_)
            | Operation::Fillet { .. }
            | Operation::Chamfer { .. }
            | Operation::Pad { .. }
            | Operation::Pocket { .. }
            | Operation::Revolve { .. }
            | Operation::Groove { .. } => parameter,
        }
    }

    /// The features this one consumes, by field: a boolean's `base` then `tool`, a fillet's,
    /// chamfer's, pocket's, or groove's `base`, and a pad's or revolve's if it has one; none
    /// for a primitive or a sketch. (A sketch a feature sweeps is used, not consumed:
    /// [`sketch`](Self::sketch).)
    pub fn inputs(&self) -> Vec<(&'static str, &FeatureId)> {
        match self {
            Operation::Fuse { base, tool }
            | Operation::Cut { base, tool }
            | Operation::Common { base, tool } => vec![("base", base), ("tool", tool)],
            Operation::Fillet { base, .. }
            | Operation::Chamfer { base, .. }
            | Operation::Pocket { base, .. }
            | Operation::Groove { base, .. } => {
                vec![("base", base)]
            }
            Operation::Pad { base, .. } | Operation::Revolve { base, .. } => {
                base.iter().map(|base| ("base", base)).collect()
            }
            Operation::Box { .. }
            | Operation::Cylinder { .. }
            | Operation::Sphere { .. }
            | Operation::Cone { .. }
            | Operation::Torus { .. }
            | Operation::Sketch(_) => Vec::new(),
        }
    }

    /// The elements a fillet or chamfer names, by name; none for another feature.
    pub fn edges(&self) -> &[ElementName] {
        match self {
            Operation::Fillet { edges, .. } | Operation::Chamfer { edges, .. } => edges,
            Operation::Box { .. }
            | Operation::Cylinder { .. }
            | Operation::Sphere { .. }
            | Operation::Cone { .. }
            | Operation::Torus { .. }
            | Operation::Fuse { .. }
            | Operation::Cut { .. }
            | Operation::Common { .. }
            | Operation::Sketch(_)
            | Operation::Pad { .. }
            | Operation::Pocket { .. }
            | Operation::Revolve { .. }
            | Operation::Groove { .. } => &[],
        }
    }

    /// The sketch a pad, pocket, revolve, or groove sweeps; none for another feature.
    pub fn sketch(&self) -> Option<&FeatureId> {
        match self {
            Operation::Pad { sketch, .. }
            | Operation::Pocket { sketch, .. }
            | Operation::Revolve { sketch, .. }
            | Operation::Groove { sketch, .. } => Some(sketch),
            Operation::Box { .. }
            | Operation::Cylinder { .. }
            | Operation::Sphere { .. }
            | Operation::Cone { .. }
            | Operation::Torus { .. }
            | Operation::Fuse { .. }
            | Operation::Cut { .. }
            | Operation::Common { .. }
            | Operation::Sketch(_)
            | Operation::Fillet { .. }
            | Operation::Chamfer { .. } => None,
        }
    }

    /// What a revolve or groove turns about; none for another feature.
    pub fn axis(&self) -> Option<&Axis> {
        match self {
            Operation::Revolve { axis, .. } | Operation::Groove { axis, .. } => Some(axis),
            Operation::Box { .. }
            | Operation::Cylinder { .. }
            | Operation::Sphere { .. }
            | Operation::Cone { .. }
            | Operation::Torus { .. }
            | Operation::Fuse { .. }
            | Operation::Cut { .. }
            | Operation::Common { .. }
            | Operation::Sketch(_)
            | Operation::Fillet { .. }
            | Operation::Chamfer { .. }
            | Operation::Pad { .. }
            | Operation::Pocket { .. } => None,
        }
    }

    /// How far a pad, pocket, revolve, or groove goes; none for another feature.
    pub fn extent(&self) -> Option<Extent> {
        match *self {
            Operation::Pad { extent, .. } => Some(Extent::Pad(extent)),
            Operation::Pocket { extent, .. } => Some(Extent::Pocket(extent)),
            Operation::Revolve { extent, .. } | Operation::Groove { extent, .. } => {
                Some(Extent::Turn(extent))
            }
            Operation::Box { .. }
            | Operation::Cylinder { .. }
            | Operation::Sphere { .. }
            | Operation::Cone { .. }
            | Operation::Torus { .. }
            | Operation::Fuse { .. }
            | Operation::Cut { .. }
            | Operation::Common { .. }
            | Operation::Sketch(_)
            | Operation::Fillet { .. }
            | Operation::Chamfer { .. } => None,
        }
    }

    /// Whether the feature makes a solid: every type but a sketch.
    pub fn makes_solid(&self) -> bool {
        // As `made` says, so the two can't disagree.
        !matches!(self.made(), Made::Sketch)
    }

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
            Operation::Sketch(_) => "sketch",
            Operation::Fillet { .. } => "fillet",
            Operation::Chamfer { .. } => "chamfer",
            Operation::Pad { .. } => "pad",
            Operation::Pocket { .. } => "pocket",
            Operation::Revolve { .. } => "revolve",
            Operation::Groove { .. } => "groove",
        }
    }
}

/// Every feature type, as the file writes it.
pub const FEATURE_TYPES: [&str; 15] = [
    "box", "cylinder", "sphere", "cone", "torus", "fuse", "cut", "common", "sketch", "fillet",
    "chamfer", "pad", "pocket", "revolve", "groove",
];
