//! A linear pattern's, polar pattern's, and mirror's fields (docs/design/m6.0-patterns.md,
//! section 2): what each copies, along or about what, how far, how many, and which instances
//! it leaves out. The document holds them as the file writes them; the gate checks them
//! (section 4.1), and the rebuild places the copies (M6.3).

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

use super::{Angle, ElementName, FeatureId, Length, OriginPlane};

/// The most instances a pattern may have, the original and every instance skipped counted
/// (decision 8): fixed in the format, so a document means the same on every machine.
pub const MOST_INSTANCES: u64 = 1000;

/// One of the model's axes from the origin, along +X, +Y, or +Z, as the origin's planes are
/// `xy`, `xz`, `yz`; its sense is a pattern's `reversed`'s alone (Fable, 2026-11-10). Not a
/// revolve's `x_axis`, which is its sketch's, nor a face sketch's `+x`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OriginAxis {
    X,
    Y,
    Z,
}

impl OriginAxis {
    pub const ALL: [OriginAxis; 3] = [OriginAxis::X, OriginAxis::Y, OriginAxis::Z];

    /// As the file writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            OriginAxis::X => "x",
            OriginAxis::Y => "y",
            OriginAxis::Z => "z",
        }
    }

    /// The axis `text` writes, if it's one.
    pub fn new(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|axis| axis.as_str() == text)
    }

    /// Its unit vector, along +X, +Y, or +Z.
    pub fn direction(self) -> [f64; 3] {
        match self {
            OriginAxis::X => [1.0, 0.0, 0.0],
            OriginAxis::Y => [0.0, 1.0, 0.0],
            OriginAxis::Z => [0.0, 0.0, 1.0],
        }
    }
}

/// An origin word or an element of the base's solid by its stable name: what a pattern's
/// direction, a polar pattern's axis, and a mirror's plane are each one of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginOrName<O> {
    Origin(O),
    Name(ElementName),
}

impl<O: Copy> OriginOrName<O> {
    /// The element's name, if it's one.
    pub fn name(&self) -> Option<&ElementName> {
        match self {
            OriginOrName::Origin(_) => None,
            OriginOrName::Name(name) => Some(name),
        }
    }

    /// The origin word, if it's one.
    pub fn origin(&self) -> Option<O> {
        match self {
            OriginOrName::Origin(origin) => Some(*origin),
            OriginOrName::Name(_) => None,
        }
    }
}

/// A linear pattern's direction (`directions.N.along`): `x`, `y`, `z`, or a straight edge or
/// a planar face of the base's solid by its name.
pub type DirectionReference = OriginOrName<OriginAxis>;

/// A polar pattern's axis: `x`, `y`, `z`, or a straight or circular edge of the base's solid
/// by its name.
pub type AxisReference = OriginOrName<OriginAxis>;

/// A mirror's plane: `xy`, `xz`, `yz`, or a planar face of the base's solid by its name.
pub type MirrorPlane = OriginOrName<OriginPlane>;

impl OriginOrName<OriginAxis> {
    /// As the file writes it.
    pub fn as_str(&self) -> &str {
        match self {
            OriginOrName::Origin(axis) => axis.as_str(),
            OriginOrName::Name(name) => name.as_str(),
        }
    }
}

impl OriginOrName<OriginPlane> {
    /// As the file writes it.
    pub fn as_str(&self) -> &str {
        match self {
            OriginOrName::Origin(plane) => plane.as_str(),
            OriginOrName::Name(name) => name.as_str(),
        }
    }
}

/// A direction's or an axis's schema: an origin axis or a name, each `$ref`d.
fn axis_or_name(generator: &mut SchemaGenerator, description: &str) -> Schema {
    let axis = generator.subschema_for::<OriginAxis>();
    let name = generator.subschema_for::<ElementName>();
    json_schema!({
        "description": description,
        "anyOf": [axis, name]
    })
}

/// The JSON Schema of a linear pattern's direction (its own `$defs` entry).
pub(crate) struct DirectionReferenceSchema;

impl JsonSchema for DirectionReferenceSchema {
    fn schema_name() -> Cow<'static, str> {
        "DirectionReference".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        axis_or_name(
            generator,
            "A linear pattern's direction: x, y, or z, the model's axes from the origin (their sense is reversed's alone), or a straight edge, from its start to its end, or a planar face, along its outward normal, of the base's solid by its stable name.",
        )
    }
}

/// The JSON Schema of a polar pattern's axis (its own `$defs` entry).
pub(crate) struct AxisReferenceSchema;

impl JsonSchema for AxisReferenceSchema {
    fn schema_name() -> Cow<'static, str> {
        "AxisReference".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        axis_or_name(
            generator,
            "A polar pattern's axis: x, y, or z, the model's axes through the origin (their sense is reversed's alone), or a straight edge of the base's solid, from its start to its end, or a circular edge, about its circle's axis through its centre, by its stable name.",
        )
    }
}

/// The JSON Schema of a mirror's plane (its own `$defs` entry).
pub(crate) struct MirrorPlaneSchema;

impl JsonSchema for MirrorPlaneSchema {
    fn schema_name() -> Cow<'static, str> {
        "MirrorPlane".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let plane = generator.subschema_for::<OriginPlane>();
        let name = generator.subschema_for::<ElementName>();
        json_schema!({
            "description": "A mirror's plane: xy, xz, or yz, the origin's planes, or a planar face of the base's solid by its stable name.",
            "anyOf": [plane, name]
        })
    }
}

/// What a pattern or mirror copies (`copies`).
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Copies {
    /// The tools of `features`, each a pad, pocket, revolve, or groove in the base's chain:
    /// a set, read in any order and written sorted.
    Features {
        #[schemars(length(min = 1))]
        features: Vec<FeatureId>,
    },
    /// The base's whole solid.
    Solid,
}

impl Copies {
    /// As the file writes its `type`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Copies::Features { .. } => "features",
            Copies::Solid => "solid",
        }
    }

    /// The originals it copies, in the order held; none for `solid`.
    pub fn features(&self) -> &[FeatureId] {
        match self {
            Copies::Features { features } => features,
            Copies::Solid => &[],
        }
    }
}

/// How a linear pattern's direction spreads its instances: over `length` from the first to
/// the last, or `length` between neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LinearDistribution {
    /// `length` from the first instance to the last.
    Extent { length: Length },
    /// `length` between neighbours.
    Spacing { length: Length },
}

impl LinearDistribution {
    /// As the file writes its `type`.
    pub fn type_name(self) -> &'static str {
        match self {
            LinearDistribution::Extent { .. } => "extent",
            LinearDistribution::Spacing { .. } => "spacing",
        }
    }

    /// Its length, whichever its form.
    pub fn length(self) -> Length {
        match self {
            LinearDistribution::Extent { length } | LinearDistribution::Spacing { length } => {
                length
            }
        }
    }
}

/// How a polar pattern spreads its instances: over `angle` from the first to the last
/// (exactly 360 spreads them over a whole turn), or `angle` between neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PolarDistribution {
    /// `angle` from the first instance to the last; 360 spreads them over a whole turn.
    Extent { angle: Angle },
    /// `angle` between neighbours.
    Spacing { angle: Angle },
}

impl PolarDistribution {
    /// As the file writes its `type`.
    pub fn type_name(self) -> &'static str {
        match self {
            PolarDistribution::Extent { .. } => "extent",
            PolarDistribution::Spacing { .. } => "spacing",
        }
    }

    /// Its angle, whichever its form.
    pub fn angle(self) -> Angle {
        match self {
            PolarDistribution::Extent { angle } | PolarDistribution::Spacing { angle } => angle,
        }
    }
}

/// One of a linear pattern's directions: what it steps along, which way, how far, and how
/// many instances, the original included.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinearDirection {
    #[schemars(with = "DirectionReferenceSchema")]
    pub along: DirectionReference,
    /// Turns the direction round.
    pub reversed: bool,
    pub distribution: LinearDistribution,
    /// The instances along it, the original included: 1 to 1,000, the directions' counts
    /// multiplied at most 1,000.
    #[schemars(range(min = 1, max = 1000))]
    pub count: u32,
}

/// An instance a pattern leaves out (`skip`): its steps from the original along each
/// direction, direction 1 first, in its one spelling, trailing zeros left off (N10), as its
/// copies' names write it. Held as the file writes it, so the gate can say what's wrong
/// with one that isn't an instance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Skipped(pub Vec<u32>);

impl Skipped {
    /// Its offsets, as written.
    pub fn offsets(&self) -> &[u32] {
        &self.0
    }
}

impl fmt::Display for Skipped {
    /// As the file writes it, `[2, 1]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        for (i, offset) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{offset}")?;
        }
        f.write_str("]")
    }
}

impl JsonSchema for Skipped {
    fn schema_name() -> Cow<'static, str> {
        "Instance".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "An instance of a pattern: its steps from the original along each direction, direction 1 first, trailing zeros left off, as its copies' names write it (C2, C2,1): [2], [2, 1], [0, 1].",
            "type": "array",
            "items": {"type": "integer", "minimum": 0, "maximum": u32::MAX},
            "minItems": 1
        })
    }
}

/// `skip`'s instances in the order the file writes them: direction 1's offset first, as
/// section 3 orders instances, then the next.
pub(crate) fn sorted_skip(skip: &[Skipped]) -> Vec<&Skipped> {
    let mut sorted: Vec<&Skipped> = skip.iter().collect();
    sorted.sort();
    sorted
}

/// `copies.features` in the order the file writes them: by ID.
pub(crate) fn sorted_features(features: &[FeatureId]) -> Vec<&FeatureId> {
    let mut sorted: Vec<&FeatureId> = features.iter().collect();
    sorted.sort();
    sorted
}

/// Which of the three a pattern feature is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PatternFeature {
    Linear,
    Polar,
    Mirror,
}

impl PatternFeature {
    /// The type the file writes.
    pub fn word(self) -> &'static str {
        match self {
            PatternFeature::Linear => "linear_pattern",
            PatternFeature::Polar => "polar_pattern",
            PatternFeature::Mirror => "mirror",
        }
    }
}

impl From<PatternFeature> for crate::naming::OpCode {
    fn from(kind: PatternFeature) -> Self {
        match kind {
            PatternFeature::Linear => crate::naming::OpCode::LinearPattern,
            PatternFeature::Polar => crate::naming::OpCode::PolarPattern,
            PatternFeature::Mirror => crate::naming::OpCode::Mirror,
        }
    }
}
