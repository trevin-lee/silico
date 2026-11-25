//! M6.5's pattern read's wire types (docs/design/m6.0-patterns.md, section 5): its request,
//! read strictly and written back as a client writes it; its result, written field for
//! field ([`ToWire`]: [`PatternRead`], what
//! [`Sessions::pattern`](crate::command::Sessions::pattern) gives); and the mirrors its
//! snapshot, `commands/pattern.json`, is generated from.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};

use super::elements::ModelChoiceSchema;
use super::{ErrorSchema, Json, RequestFields, ToWire, wire_shape, words};
use crate::command::pattern::{
    AxisRead, DirectionRead, InstanceRead, InstanceState, PatternRead, PlaneRead,
};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{ModelChoice, Problems, SessionId};
use crate::document::json::Node;
use crate::document::{Angle, FeatureId, Length, PROBLEMS_LISTED, PatternFeature, Skipped};

/// A `pattern` request: the session, the model it reads, and the pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternRequest {
    pub session: SessionId,
    pub model: ModelChoice,
    pub pattern: FeatureId,
}

impl PatternRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = f.model();
            let pattern = f.feature("pattern");
            Some(PatternRequest {
                session: session?,
                model: model?,
                pattern: pattern?,
            })
        })
    }
}

impl ToWire for PatternRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
            ("pattern", self.pattern.to_wire()),
        ])
    }
}

impl ToWire for PatternRead {
    /// `pattern`'s result: the pattern's type; its directions, axis, or plane, each null
    /// for the types without it or where it didn't resolve; its anchor; every instance but
    /// the original; and its problems.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("pattern", self.pattern.to_wire()),
            ("type", Json::str(self.kind.word())),
            (
                "directions",
                Json::or_null(self.directions.as_ref(), |directions| {
                    Json::array(directions.iter().map(ToWire::to_wire))
                }),
            ),
            ("axis", Json::or_null(self.axis.as_ref(), ToWire::to_wire)),
            ("plane", Json::or_null(self.plane.as_ref(), ToWire::to_wire)),
            ("anchor", Json::or_null(self.anchor, Json::point)),
            (
                "instances",
                Json::array(self.instances.iter().map(ToWire::to_wire)),
            ),
            (
                "problems",
                Json::array(self.problems.iter().map(ToWire::to_wire)),
            ),
            ("problems_total", Json::whole(self.problems_total)),
        ])
    }
}

impl ToWire for DirectionRead {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("direction", Json::direction(self.direction)),
            ("step", Json::or_null(self.step, Json::length)),
        ])
    }
}

impl ToWire for AxisRead {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("point", Json::point(self.point)),
            ("direction", Json::direction(self.direction)),
            ("step", Json::or_null(self.step, Json::angle)),
        ])
    }
}

impl ToWire for PlaneRead {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("origin", Json::point(self.origin)),
            ("normal", Json::direction(self.normal)),
        ])
    }
}

impl ToWire for InstanceRead {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            (
                "index",
                Json::array(
                    self.index
                        .offsets()
                        .iter()
                        .map(|&offset| Json::whole(u64::from(offset))),
                ),
            ),
            ("offset", Json::or_null(self.offset, Json::point)),
            ("angle", Json::or_null(self.angle, Json::angle)),
            ("anchor", Json::or_null(self.anchor, Json::point)),
            (
                "state",
                Json::or_null(self.state, |state| Json::str(state.as_str())),
            ),
        ])
    }
}

/// `pattern`'s request: a pattern as the model named built it.
#[derive(JsonSchema)]
#[schemars(rename = "PatternRequest", transform = wire_shape)]
pub struct PatternRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
    /// The pattern, by its feature's ID: a linear_pattern, polar_pattern, or mirror.
    pub pattern: FeatureId,
}

/// `pattern`'s result: the pattern as the model's rebuild built it, its references resolved
/// in 3D and each instance placed, for the app's instance boxes and for an agent choosing
/// which copy to name.
#[derive(JsonSchema)]
#[schemars(rename = "PatternResult", transform = wire_shape)]
pub struct PatternResultSchema {
    /// The pattern, by its feature's ID.
    pub pattern: FeatureId,
    #[schemars(rename = "type")]
    pub kind: PatternTypeSchema,
    /// A linear_pattern's directions, in the document's order, each resolved in 3D with its
    /// sense (`reversed`) taken: null for another type, or where a direction's reference
    /// didn't resolve (`problems` says why).
    pub directions: Option<Vec<PatternDirectionSchema>>,
    /// A polar_pattern's axis, resolved in 3D with its sense (`reversed`) taken: null for
    /// another type, or where it didn't resolve (`problems` says why).
    pub axis: Option<PatternAxisSchema>,
    /// A mirror's plane, resolved in 3D: null for another type, or where it didn't resolve
    /// (`problems` says why).
    pub plane: Option<PatternPlaneSchema>,
    /// Where the original sits (mm): the centre of area of its originals' profiles, each on
    /// its sketch's plane, weighted by their areas; or, for a whole solid's copies
    /// (`copies` of type `solid`), the base's centre of mass. Null where it can't be had
    /// (`problems` says why).
    pub anchor: Option<[Length; 3]>,
    /// Every instance but the original, skipped or not, in `skip`'s order (direction 1's
    /// offset first): none where the references didn't resolve.
    pub instances: Vec<PatternInstanceSchema>,
    /// The pattern's own problems, as `check` gives them (the first 100): none when it
    /// built. For a pattern blocked on another feature, the error a read of its solid
    /// gives; where its base's elements couldn't be described, the error `elements` gives
    /// for that solid.
    #[schemars(length(max = PROBLEMS_LISTED))]
    pub problems: Vec<ErrorSchema>,
    /// How many problems there are, listed or not.
    pub problems_total: u64,
}

/// One direction of a linear pattern, resolved.
#[derive(JsonSchema)]
#[schemars(rename = "PatternDirection", transform = wire_shape)]
pub struct PatternDirectionSchema {
    /// The direction its instances step along, a unit vector, `reversed` taken.
    pub direction: [f64; 3],
    /// The distance between neighbours (mm): the extent's length over the count less one,
    /// or the spacing; null for an extent of count 1, which has no neighbours.
    pub step: Option<Length>,
}

/// A polar pattern's axis, resolved.
#[derive(JsonSchema)]
#[schemars(rename = "PatternAxis", transform = wire_shape)]
pub struct PatternAxisSchema {
    /// A point on the axis (mm).
    pub point: [Length; 3],
    /// The axis's direction, a unit vector, `reversed` taken: the instances turn
    /// right-handed about it.
    pub direction: [f64; 3],
    /// The angle between neighbours (degrees): a whole turn's extent over the count, any
    /// other extent over the count less one, or the spacing; null for an extent of count 1.
    pub step: Option<Angle>,
}

/// A mirror's plane, resolved.
#[derive(JsonSchema)]
#[schemars(rename = "PatternPlane", transform = wire_shape)]
pub struct PatternPlaneSchema {
    /// The global origin projected onto the plane (mm).
    pub origin: [Length; 3],
    /// The plane's normal, a unit vector.
    pub normal: [f64; 3],
}

/// One instance of a pattern, the original's left out.
#[derive(JsonSchema)]
#[schemars(rename = "PatternInstance", transform = wire_shape)]
pub struct PatternInstanceSchema {
    /// The instance, as `skip` writes it and its copies' names carry it after `C` or `S`.
    pub index: Skipped,
    /// A linear instance's move from the original (mm); null for another type's.
    pub offset: Option<[Length; 3]>,
    /// A polar instance's turn about the axis from the original (degrees); null for another
    /// type's. A mirror's copy has neither: it's a reflection in `plane`.
    pub angle: Option<Angle>,
    /// Where this instance puts the pattern's anchor (mm): where the copied profile's
    /// centre, or a whole solid's centre of mass, lands. Null where `anchor` is.
    pub anchor: Option<[Length; 3]>,
    /// What the rebuild said of its copy; null where it said nothing.
    pub state: Option<InstanceStateSchema>,
}

/// A pattern's type.
pub struct PatternTypeSchema;

impl JsonSchema for PatternTypeSchema {
    fn schema_name() -> Cow<'static, str> {
        "PatternType".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "The pattern's type, as the document writes it.",
            [
                PatternFeature::Linear,
                PatternFeature::Polar,
                PatternFeature::Mirror,
            ]
            .map(PatternFeature::word),
        )
    }
}

/// An instance's state.
pub struct InstanceStateSchema;

impl JsonSchema for InstanceStateSchema {
    fn schema_name() -> Cow<'static, str> {
        "InstanceState".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "What the rebuild said of the instance's copy, the first that holds: skipped (`skip` holds it), off_base (its copy misses the base, which refuses the pattern), changed_nothing (its copy adds or removes nothing: none of its faces is in the result), or built. Null where a step that would place it didn't run, or named no instance.",
            InstanceState::ALL.map(InstanceState::as_str),
        )
    }
}
