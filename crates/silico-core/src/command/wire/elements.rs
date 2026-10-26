//! `elements` and `query`'s wire types (docs/design/m4.1-command-api.md, section 2's
//! *Element entries* and section 7): their requests, read strictly ([`ElementsRequest`],
//! [`QueryRequest`]); their results and the entries they hold, written field for field
//! ([`ToWire`]); and the mirrors their snapshots, `commands/elements.json` and
//! `commands/query.json`, are generated from. A request's shape is read here; what its
//! values mean (a limit past what a call returns, a type the kind can't have, a facing on
//! edges) the command says, so the wire and a Rust client meet one set of refusals.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

use super::{Json, RequestFields, ToWire, wire_shape};
use crate::command::elements::{
    Alias, AxisToken, Cursor, ENTRIES_AT_MOST, ElementType, Entry, EntryAxis, Facing, Listed,
    MEASURES_AT_MOST, MeasureName, Order, Quantity, QuantityKind, Queried, Query, Range, Tied,
    too_many_ranges,
};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{ModelChoice, Problems, SessionId};
use crate::document::json::Node;
use crate::document::{Angle, FeatureId, Length, Measure};
use crate::kernel::ElementKind;

/// The kinds a request names: a face, an edge, or a vertex.
const KINDS: [ElementKind; 3] = [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex];

fn kinds() -> [(&'static str, ElementKind); 3] {
    KINDS.map(|kind| (kind.word(), kind))
}

fn types() -> [(&'static str, ElementType); 17] {
    ElementType::ALL.map(|t| (t.word(), t))
}

fn measure_names() -> [(&'static str, MeasureName); 7] {
    MeasureName::ALL.map(|m| (m.word(), m))
}

/// An `elements` request: the session, the model it reads, which solid (`None` for every
/// body), where it goes on from, and how many to list.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementsRequest {
    pub session: SessionId,
    pub model: ModelChoice,
    pub feature: Option<FeatureId>,
    pub after: Option<Cursor>,
    pub limit: u32,
}

impl ElementsRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = f.model();
            let feature = f.nullable_feature("feature");
            let after = f.nullable_object("after", |c| {
                let revision = c.revision("revision");
                let feature = c.feature("feature");
                let name = c.string("name");
                Some(Cursor {
                    revision: revision?,
                    feature: feature?,
                    name: name?.to_owned(),
                })
            });
            let limit = f.u32("limit");
            Some(ElementsRequest {
                session: session?,
                model: model?,
                feature: feature?,
                after: after?,
                limit: limit?,
            })
        })
    }
}

/// A `query` request: the session, the model it reads, and the filter.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryRequest {
    pub session: SessionId,
    pub model: ModelChoice,
    pub query: Query,
}

impl QueryRequest {
    /// The request `node` holds, or every reason its shape is refused. A range's bounds are
    /// read as its measure's quantity is written: an area, a length, or an angle.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = f.model();
            let feature = f.nullable_feature("feature");
            let kind = f.word("kind", &kinds(), "kinds");
            let r#type = f.nullable_word("type", &types(), "types");
            let stable = f.nullable_bool("stable");
            let facing = f.nullable_object("facing", |g| {
                let direction = g.direction("direction");
                let within = g.angle("within");
                Some(Facing {
                    direction: direction?,
                    within: within?,
                })
            });
            let measures = f.objects("measures", MEASURES_AT_MOST, too_many_ranges, range);
            let order = f.nullable_object("order", |g| {
                let by = g.word("by", &measure_names(), "measures");
                let descending = g.bool("descending");
                Some(Order {
                    by: by?,
                    descending: descending?,
                })
            });
            let limit = f.u32("limit");
            Some(QueryRequest {
                session: session?,
                model: model?,
                query: Query {
                    feature: feature?,
                    kind: kind?,
                    r#type: r#type?,
                    stable: stable?,
                    facing: facing?,
                    measures: measures?,
                    order: order?,
                    limit: limit?,
                },
            })
        })
    }
}

/// A range of `measures`: its measure, then its bounds, each read as the measure's
/// quantity, or left unread if the measure was refused.
fn range(f: &mut RequestFields<'_>) -> Option<Range> {
    let measure = f.word("measure", &measure_names(), "measures");
    let mut bound = |key: &str| match measure.map(MeasureName::quantity) {
        Some(QuantityKind::Area) => f.nullable_area(key).map(|b| b.map(Quantity::Area)),
        Some(QuantityKind::Length) => f.nullable_length(key).map(|b| b.map(Quantity::Length)),
        Some(QuantityKind::Angle) => f.nullable_angle(key).map(|b| b.map(Quantity::Angle)),
        None => {
            // Taken unread: its measure was refused.
            f.node(key);
            None
        }
    };
    let at_least = bound("at_least");
    let at_most = bound("at_most");
    Some(Range {
        measure: measure?,
        at_least: at_least?,
        at_most: at_most?,
    })
}

impl ToWire for ElementsRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
            (
                "feature",
                Json::or_null(self.feature.as_ref(), ToWire::to_wire),
            ),
            ("after", Json::or_null(self.after.as_ref(), ToWire::to_wire)),
            ("limit", Json::whole(self.limit.into())),
        ])
    }
}

impl ToWire for QueryRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        let q = &self.query;
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
            (
                "feature",
                Json::or_null(q.feature.as_ref(), ToWire::to_wire),
            ),
            ("kind", Json::str(q.kind.word())),
            ("type", Json::or_null(q.r#type, |t| Json::str(t.word()))),
            ("stable", Json::or_null(q.stable, Json::bool)),
            ("facing", Json::or_null(q.facing.as_ref(), ToWire::to_wire)),
            (
                "measures",
                Json::array(q.measures.iter().map(ToWire::to_wire)),
            ),
            ("order", Json::or_null(q.order.as_ref(), ToWire::to_wire)),
            ("limit", Json::whole(q.limit.into())),
        ])
    }
}

impl ToWire for Facing {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("direction", Json::direction(self.direction)),
            ("within", Json::angle(self.within)),
        ])
    }
}

impl ToWire for Range {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("measure", Json::str(self.measure.word())),
            ("at_least", Json::or_null(self.at_least, quantity)),
            ("at_most", Json::or_null(self.at_most, quantity)),
        ])
    }
}

impl ToWire for Order {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("by", Json::str(self.by.word())),
            ("descending", Json::bool(self.descending)),
        ])
    }
}

/// A measure's value, as its quantity is written.
fn quantity(q: Quantity) -> Json<'static> {
    match q {
        Quantity::Area(m) => Json::measure(m),
        Quantity::Length(l) => Json::length(l),
        Quantity::Angle(a) => Json::angle(a),
    }
}

/// A point's three lengths, or `null`.
fn point(point: Option<&[Length; 3]>) -> Json<'static> {
    Json::or_null(point, |p| Json::point(*p))
}

impl ToWire for Entry {
    /// The design's entry, every field always present, `null` where it doesn't apply.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("name", Json::string(self.name.to_string())),
            (
                "aliases",
                Json::array(self.aliases.iter().map(ToWire::to_wire)),
            ),
            ("stable", Json::bool(self.stable)),
            ("feature", self.feature.to_wire()),
            ("kind", Json::str(self.kind.word())),
            ("type", Json::str(self.r#type.word())),
            ("area", Json::or_null(self.area, Json::measure)),
            ("length", Json::or_null(self.length, Json::length)),
            ("position", point(self.position.as_ref())),
            ("normal", Json::or_null(self.normal, Json::direction)),
            ("centroid", point(self.centroid.as_ref())),
            ("start", point(self.start.as_ref())),
            ("end", point(self.end.as_ref())),
            ("midpoint", point(self.midpoint.as_ref())),
            ("straight", Json::or_null(self.straight, Json::bool)),
            ("axis", Json::or_null(self.axis.as_ref(), ToWire::to_wire)),
            ("radius", Json::or_null(self.radius, Json::length)),
            ("diameter", Json::or_null(self.diameter, Json::length)),
            (
                "major_radius",
                Json::or_null(self.major_radius, Json::length),
            ),
            (
                "minor_radius",
                Json::or_null(self.minor_radius, Json::length),
            ),
            ("half_angle", Json::or_null(self.half_angle, Json::angle)),
            ("apex", point(self.apex.as_ref())),
            (
                "default_x",
                Json::or_null(self.default_x, |x| Json::str(x.word())),
            ),
        ])
    }
}

impl ToWire for Alias {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("name", Json::string(self.name.to_string())),
            ("stable", Json::bool(self.stable)),
        ])
    }
}

impl ToWire for EntryAxis {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("point", Json::point(self.point)),
            ("direction", Json::direction(self.direction)),
        ])
    }
}

impl ToWire for Cursor {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("revision", self.revision.to_wire()),
            ("feature", self.feature.to_wire()),
            ("name", Json::str(&self.name)),
        ])
    }
}

impl ToWire for Listed {
    /// `elements`' result: the entries, how many there are in all, and the next page's
    /// cursor.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            (
                "elements",
                Json::array(self.entries.iter().map(ToWire::to_wire)),
            ),
            ("total", Json::whole(self.total)),
            ("next", Json::or_null(self.next.as_ref(), ToWire::to_wire)),
        ])
    }
}

impl ToWire for Queried {
    /// `query`'s result: the entries, how many matched before the limit, and the tie at
    /// its cut.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            (
                "elements",
                Json::array(self.entries.iter().map(ToWire::to_wire)),
            ),
            ("matched", Json::whole(self.matched)),
            ("tied", Json::or_null(self.tied.as_ref(), ToWire::to_wire)),
        ])
    }
}

impl ToWire for Tied {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("count", Json::whole(self.count)),
            ("included", Json::bool(self.included)),
        ])
    }
}

/// A string schema of one of `words`.
pub(super) fn words(description: &str, words: impl IntoIterator<Item = &'static str>) -> Schema {
    let words: Vec<&str> = words.into_iter().collect();
    json_schema!({
        "description": description,
        "type": "string",
        "enum": words,
    })
}

/// The kinds an element is: `face`, `edge`, or `vertex`.
pub struct KindSchema;

impl JsonSchema for KindSchema {
    fn schema_name() -> Cow<'static, str> {
        "ElementKind".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "What an element is: a face, an edge, or a vertex.",
            KINDS.map(ElementKind::word),
        )
    }
}

/// The geometry's types, as OCCT names them.
pub struct TypeSchema;

impl JsonSchema for TypeSchema {
    fn schema_name() -> Cow<'static, str> {
        "ElementType".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "An element's geometry, as OCCT names it. A face is a plane, cylinder, cone, \
             sphere, torus, bezier, bspline, revolution, extrusion, offset, or other; an edge \
             a line, circle (an arc too), ellipse, hyperbola, parabola, bezier, bspline, \
             offset, or other (a degenerated edge, such as a sphere's pole, is other); a \
             vertex a point.",
            ElementType::ALL.map(ElementType::word),
        )
    }
}

/// The measures `query` filters and orders by.
pub struct MeasureNameSchema;

impl JsonSchema for MeasureNameSchema {
    fn schema_name() -> Cow<'static, str> {
        "MeasureName".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "A measure an entry has: a face's area, an edge's length, a radius or diameter (a \
             cylinder's, sphere's, circle's, or arc's; a diameter a cylinder's, circle's, or \
             arc's), a torus's major_radius and minor_radius, or a cone's half_angle.",
            MeasureName::ALL.map(MeasureName::word),
        )
    }
}

/// A sketch's default x on a planar face.
pub struct AxisTokenSchema;

impl JsonSchema for AxisTokenSchema {
    fn schema_name() -> Cow<'static, str> {
        "AxisToken".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "A global axis, signed, as a sketch on a face names its x.",
            AxisToken::ALL.map(AxisToken::word),
        )
    }
}

/// A range's bound: a number, as its measure is written.
pub struct BoundSchema;

impl JsonSchema for BoundSchema {
    fn schema_name() -> Cow<'static, str> {
        "Bound".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A measure's value, as the measure is written: an area in mm² or a \
                            length in mm to at most 6 decimals, an angle in degrees to at most \
                            9. Read exactly from its digits.",
            "type": "number"
        })
    }
}

/// Which model to read: the current one, or the newest published.
#[derive(JsonSchema)]
#[schemars(rename = "ModelChoice", rename_all = "lowercase")]
pub enum ModelChoiceSchema {
    /// The session's current revision's model, rebuilt if it isn't yet: what an agent that
    /// just edited reads.
    Current,
    /// The newest model published, which may be an older revision's, read at once: what a
    /// UI reads while a rebuild runs.
    Published,
}

/// Where a listing goes on from, as the last call's `next` gave it: pass it back as it
/// was.
#[derive(JsonSchema)]
#[schemars(rename = "Cursor", transform = wire_shape)]
pub struct CursorSchema {
    /// The revision the listing is of: a cursor of another is refused, as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The feature whose solid the last entry listed is of.
    pub feature: FeatureId,
    /// The last entry listed, by its own name.
    pub name: String,
}

/// `elements`' request: a page of the model's elements, of one feature's solid or of
/// every body.
#[derive(JsonSchema)]
#[schemars(rename = "ElementsRequest", transform = wire_shape)]
pub struct ElementsRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
    /// The feature whose solid to list, or null for every body (the solids no later
    /// feature consumed, in timeline order).
    pub feature: Option<FeatureId>,
    /// Where to go on from: null to start, else the last call's `next`.
    pub after: Option<CursorSchema>,
    /// How many entries to list.
    #[schemars(range(min = 1, max = ENTRIES_AT_MOST))]
    pub limit: u32,
}

/// One of an element's other names.
#[derive(JsonSchema)]
#[schemars(rename = "Alias", transform = wire_shape)]
pub struct AliasSchema {
    pub name: String,
    /// Whether this name is stable: one a reference stored in the document may use.
    pub stable: bool,
}

/// An axis: a point on it and its direction, a unit vector keeping its surface's own sign.
#[derive(JsonSchema)]
#[schemars(rename = "EntryAxis", transform = wire_shape)]
pub struct EntryAxisSchema {
    pub point: [Length; 3],
    pub direction: [f64; 3],
}

/// A face, an edge, or a vertex, named and measured. Every field is always present, null
/// where it doesn't apply to the element's kind and type.
#[derive(JsonSchema)]
#[schemars(rename = "Entry", transform = wire_shape)]
pub struct EntrySchema {
    /// The element's own name, the first of its names: give it to a fillet's `edges` (if
    /// it's stable), `distance`, `angle`, or `render`'s `highlight`.
    pub name: String,
    /// Its other names, in the element map's order, each with its own stability.
    pub aliases: Vec<AliasSchema>,
    /// Whether `name` is stable: false for a name that came about for more than one
    /// element, a duplicate (`~n`), or an element by its number. A reference stored in the
    /// document refuses an unstable name; use a stable alias.
    pub stable: bool,
    /// The feature whose solid the element is of.
    pub feature: FeatureId,
    pub kind: KindSchema,
    #[schemars(rename = "type")]
    pub geometry: TypeSchema,
    /// A face's area, in mm².
    pub area: Option<Measure>,
    /// An edge's length (0 for a degenerated edge).
    pub length: Option<Length>,
    /// A vertex's point.
    pub position: Option<[Length; 3]>,
    /// A planar face's outward unit normal.
    pub normal: Option<[f64; 3]>,
    /// A face's centre of area.
    pub centroid: Option<[Length; 3]>,
    /// An edge's first end, in its curve's own order (FreeCAD's `Edge.firstVertex()`), not
    /// the order a face uses it in.
    pub start: Option<[Length; 3]>,
    /// An edge's last end (FreeCAD's `Edge.lastVertex()`).
    pub end: Option<[Length; 3]>,
    /// A straight edge's middle.
    pub midpoint: Option<[Length; 3]>,
    /// Whether an edge is straight: a line, or a degree-1 B-spline of two poles.
    pub straight: Option<bool>,
    /// A cylinder's, cone's, sphere's, torus's, circle's, or arc's axis.
    pub axis: Option<EntryAxisSchema>,
    /// A cylinder's, sphere's, circle's, or arc's radius.
    pub radius: Option<Length>,
    /// Twice the radius: a circle's or arc's, and a cylinder face's.
    pub diameter: Option<Length>,
    /// A torus's major radius (FreeCAD's Toroid MajorRadius).
    pub major_radius: Option<Length>,
    /// A torus's minor radius.
    pub minor_radius: Option<Length>,
    /// A cone's half-angle in degrees, keeping its sign, as FreeCAD's `Part.Cone.SemiAngle`
    /// gives it.
    pub half_angle: Option<Angle>,
    /// A cone's apex; null for a cone so nearly a cylinder that its apex lies further than a
    /// length holds.
    pub apex: Option<[Length; 3]>,
    /// For a planar face, the axis a sketch on it takes as its x by default.
    pub default_x: Option<AxisTokenSchema>,
}

/// `elements`' result: a page of entries.
#[derive(JsonSchema)]
#[schemars(rename = "ElementsResult", transform = wire_shape)]
pub struct ElementsResultSchema {
    /// The entries, in order: by body in timeline order, then faces, edges, and vertices,
    /// each by name, byte by byte.
    #[schemars(length(max = ENTRIES_AT_MOST))]
    pub elements: Vec<EntrySchema>,
    /// How many entries the listing holds in all, on every page.
    pub total: u64,
    /// Where the next page starts: null after the last, and for a listing of every body of
    /// a model whose rebuild met a kernel fault.
    pub next: Option<CursorSchema>,
}

/// Planar faces facing a direction.
#[derive(JsonSchema)]
#[schemars(rename = "Facing", transform = wire_shape)]
pub struct FacingSchema {
    /// The direction, which needn't be a unit vector, but can't be zero.
    pub direction: [f64; 3],
    /// The most a face's outward normal may be off the direction, from 0 to 180 degrees.
    pub within: Angle,
}

/// A measure's range: at_least to at_most, both included, either open (null).
#[derive(JsonSchema)]
#[schemars(rename = "MeasureRange", transform = wire_shape)]
pub struct RangeSchema {
    /// A measure the query's kind has.
    pub measure: MeasureNameSchema,
    pub at_least: Option<BoundSchema>,
    pub at_most: Option<BoundSchema>,
}

/// Entries by a measure: the smallest first, unless descending. Only entries that have the
/// measure are kept.
#[derive(JsonSchema)]
#[schemars(rename = "Order", transform = wire_shape)]
pub struct OrderSchema {
    /// A measure the query's kind has.
    pub by: MeasureNameSchema,
    pub descending: bool,
}

/// `query`'s request: the model's elements of a kind that match a filter, such as "the
/// largest planar face facing +Z".
#[derive(JsonSchema)]
#[schemars(rename = "QueryRequest", transform = wire_shape)]
pub struct QueryRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
    /// The feature whose solid to query, or null for every body.
    pub feature: Option<FeatureId>,
    pub kind: KindSchema,
    /// The geometry's type, one the kind can have, or null for any.
    #[schemars(rename = "type")]
    pub geometry: Option<TypeSchema>,
    /// true for elements with a stable name (their own or an alias), false for those with
    /// none, null for either.
    pub stable: Option<bool>,
    /// Planar faces facing a direction, or null; only for kind face, of type plane or null.
    pub facing: Option<FacingSchema>,
    /// Ranges of measures, each kept only by entries that have the measure: at most 7.
    #[schemars(length(max = MEASURES_AT_MOST))]
    pub measures: Vec<RangeSchema>,
    /// The order, or null for `elements`' order.
    pub order: Option<OrderSchema>,
    /// How many entries to return, before a tie at the cut.
    #[schemars(range(min = 1, max = ENTRIES_AT_MOST))]
    pub limit: u32,
}

/// Entries equal by the order's measure, as written, across the limit's cut.
#[derive(JsonSchema)]
#[schemars(rename = "Tied", transform = wire_shape)]
pub struct TiedSchema {
    /// How many entries are equal there.
    pub count: u64,
    /// Whether they're all returned, past the limit (when they end within 1,000 entries);
    /// if not, none of them is, and the filter needs narrowing.
    pub included: bool,
}

/// `query`'s result: the entries that matched, the first `limit` of them, and the tie at the
/// cut, if there's one. A query never picks among ties.
#[derive(JsonSchema)]
#[schemars(rename = "QueryResult", transform = wire_shape)]
pub struct QueryResultSchema {
    /// The entries, in the order's order (or `elements`').
    #[schemars(length(max = ENTRIES_AT_MOST))]
    pub elements: Vec<EntrySchema>,
    /// How many entries matched the filter, before the limit.
    pub matched: u64,
    /// The tie at the limit's cut, or null if there's none.
    pub tied: Option<TiedSchema>,
}
