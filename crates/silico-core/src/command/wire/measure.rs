//! M4.5's measures' wire types (docs/design/m4.1-command-api.md, section 7):
//! `mass_properties`' request, read strictly and written back as a client writes it; its
//! result, written field for field; and the mirrors its snapshot,
//! `commands/mass_properties.json`, is generated from.

use schemars::JsonSchema;

use super::elements::ModelChoiceSchema;
use super::{Json, RequestFields, ToWire, wire_shape};
use crate::command::measure::{Bodies, MassProperties};
use crate::command::{ModelChoice, Problems, SessionId};
use crate::document::json::Node;
use crate::document::{FeatureId, Length, Measure};

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
                Json::or_null(self.feature.as_ref(), |f| Json::str(f.as_str())),
            ),
        ])
    }
}

impl ToWire for MassProperties {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("feature", Json::str(self.feature.as_str())),
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
    #[schemars(regex(pattern = r"^s[1-9][0-9]*$"))]
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
