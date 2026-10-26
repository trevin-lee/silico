//! M4.5's measures' wire types (docs/design/m4.1-command-api.md, section 7):
//! `mass_properties`', `distance`'s, and `angle`'s requests, read strictly and written back
//! as a client writes them; their results, written field for field; and the mirrors their
//! snapshots, `commands/mass_properties.json`, `distance.json`, and `angle.json`, are
//! generated from.

use schemars::JsonSchema;

use super::elements::ModelChoiceSchema;
use super::{Json, RequestFields, ToWire, wire_shape};
use crate::command::measure::{AngleAnswer, Bodies, DistanceAnswer, MassProperties};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{ModelChoice, Problems, SessionId};
use crate::document::json::Node;
use crate::document::{Angle, FeatureId, Length, Measure};

/// A `mass_properties` request: the session, the model it reads, and which solid (`None`
/// for every body).
#[derive(Debug, Clone, PartialEq)]
pub struct MassPropertiesRequest {
    pub session: SessionId,
    pub model: ModelChoice,
    pub feature: Option<FeatureId>,
}

impl MassPropertiesRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = f.model();
            let feature = f.nullable_feature("feature");
            Some(MassPropertiesRequest {
                session: session?,
                model: model?,
                feature: feature?,
            })
        })
    }
}

impl ToWire for MassPropertiesRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
            (
                "feature",
                Json::or_null(self.feature.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

impl ToWire for MassProperties {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("feature", self.feature.to_wire()),
            ("volume", Json::measure(self.volume)),
            ("area", Json::measure(self.area)),
            ("center_of_mass", Json::point(self.center_of_mass)),
        ])
    }
}

impl ToWire for Bodies {
    /// `mass_properties`' result: each solid's, in timeline order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([(
            "bodies",
            Json::array(self.bodies.iter().map(ToWire::to_wire)),
        )])
    }
}

/// `mass_properties`' request: the mass properties of one feature's solid, or of every
/// body.
#[derive(JsonSchema)]
#[schemars(rename = "MassPropertiesRequest", transform = wire_shape)]
pub struct MassPropertiesRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
    /// The feature whose solid to measure (any feature that built one, consumed or not), or
    /// null for every body (the solids no later feature consumed, in timeline order).
    pub feature: Option<FeatureId>,
}

/// A solid's mass properties, from OCCT's integration asked for a relative 1e-9: volume
/// and area as its feature's rebuilt fingerprint holds them, to a millionth of a mm³ and
/// mm²; the centre of mass rounded to the nanometre. None is more precise than it's
/// written.
#[derive(JsonSchema)]
#[schemars(rename = "MassProperties", transform = wire_shape)]
pub struct MassPropertiesSchema {
    /// The feature whose solid it is.
    pub feature: FeatureId,
    /// Its volume, in mm³.
    pub volume: Measure,
    /// Its surface's area, in mm².
    pub area: Measure,
    /// Its centre of mass, of a uniform density, in global coordinates.
    pub center_of_mass: [Length; 3],
}

/// `mass_properties`' result.
#[derive(JsonSchema)]
#[schemars(rename = "MassPropertiesResult", transform = wire_shape)]
pub struct MassPropertiesResultSchema {
    /// Each solid's mass properties: the feature's, or each body's, in timeline order.
    pub bodies: Vec<MassPropertiesSchema>,
}

/// A `distance` or `angle` request: the session, the model it reads, the two elements by
/// name, and which solid to find them in (`None` for every body).
#[derive(Debug, Clone, PartialEq)]
pub struct PairRequest {
    pub session: SessionId,
    pub model: ModelChoice,
    pub elements: [String; 2],
    pub feature: Option<FeatureId>,
}

impl PairRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = f.model();
            let elements = f.strings("elements", "name");
            let feature = f.nullable_feature("feature");
            Some(PairRequest {
                session: session?,
                model: model?,
                elements: elements?,
                feature: feature?,
            })
        })
    }
}

impl ToWire for PairRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
            (
                "elements",
                Json::array(self.elements.iter().map(|name| Json::str(name))),
            ),
            (
                "feature",
                Json::or_null(self.feature.as_ref(), |f| Json::str(f.as_str())),
            ),
        ])
    }
}

impl ToWire for DistanceAnswer {
    /// `distance`'s result.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("distance", Json::length(self.distance)),
            ("from", Json::point(self.from)),
            ("to", Json::point(self.to)),
            (
                "center_distance",
                Json::or_null(self.center_distance, Json::length),
            ),
            (
                "normal_distance",
                Json::or_null(self.normal_distance, Json::length),
            ),
        ])
    }
}

impl ToWire for AngleAnswer {
    /// `angle`'s result.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("angle", Json::angle(self.angle)),
            ("vertex", Json::or_null(self.vertex.as_deref(), Json::str)),
        ])
    }
}

/// `distance`'s request: the least distance between two elements, and their centres' and
/// normal distances where they have them.
#[derive(JsonSchema)]
#[schemars(rename = "DistanceRequest", transform = wire_shape)]
pub struct DistanceRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
    /// The two elements, by name (as `elements` and `query` give them; an unstable name is
    /// taken). The same name twice is 0 apart.
    pub elements: [String; 2],
    /// The feature whose solid to find them in, or null for every body.
    pub feature: Option<FeatureId>,
}

/// `distance`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "DistanceResult", transform = wire_shape)]
pub struct DistanceResultSchema {
    /// The least distance between the elements as bounded (never their infinite carriers),
    /// in mm: 0 where they touch or cross.
    pub distance: Length,
    /// A point where it's met, on the first element: with `to`, one pair of points where
    /// it's met (the pair first by its points' coordinates), a witness, not an identity.
    pub from: [Length; 3],
    /// The pair's point on the second element.
    pub to: [Length; 3],
    /// The least distance between the elements' centres (a vertex's point, a circle's or
    /// arc's centre, a cylinder face's axis as an infinite line), if both have one.
    pub center_distance: Option<Length>,
    /// Where both are straight edges or planar faces at exactly 0 or 180 degrees: two faces
    /// or two edges, the perpendicular distance from the second's reference point (a face's
    /// centre of area, an edge's midpoint) to the first's plane or line; an edge and a face,
    /// the edge's midpoint's from the face's plane, whichever comes first. Null otherwise.
    pub normal_distance: Option<Length>,
}

/// `angle`'s request: the angle between two elements, each a straight edge or a planar face.
#[derive(JsonSchema)]
#[schemars(rename = "AngleRequest", transform = wire_shape)]
pub struct AngleRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
    /// The two elements, by name: each a straight edge or a planar face.
    pub elements: [String; 2],
    /// The feature whose solid to find them in, or null for every body.
    pub feature: Option<FeatureId>,
}

/// `angle`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "AngleResult", transform = wire_shape)]
pub struct AngleResultSchema {
    /// In degrees. Two planar faces: the angle of the wedge both outward normals point into,
    /// 0 to 180 (a 30° wedge reads 30; parallel faces 0 facing each other, 180 facing the
    /// same way). Two straight edges sharing one vertex: the angle at it, 0 to 180. Other
    /// straight edges, or a straight edge and a planar face: 0 to 90 between their lines and
    /// planes.
    pub angle: Angle,
    /// For two straight edges sharing one vertex, the vertex, by name; null otherwise.
    pub vertex: Option<String>,
}
