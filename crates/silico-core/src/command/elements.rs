//! M4.4's element reads, `elements` and `query` (docs/design/m4.1-command-api.md, section 2's
//! *Element entries* and section 7): a model's solids' faces, edges, and vertices as entries,
//! each named by its element map and measured by its rebuild's description
//! ([`Model::description`]), so a read asks the kernel nothing. These are the commands'
//! answers from a model; a session runs them on the model a read names (decision 33).

use std::sync::Arc;

use super::{ModelChoice, Problems, Revision, Running, SessionId, Sessions};
use crate::document::{Angle, FeatureId, Length, Measure, PROBLEMS_LISTED};
use crate::elements::{Element, Geometry, elements};
use crate::kernel::{Axis, Clients, EdgeCurve, ElementKind, Surface};
use crate::naming::{ElementMap, Name};
use crate::recompute::{FeatureOutcome, Model};
use crate::{Error, ErrorCode};

impl<P: Clients + 'static> Sessions<P> {
    /// `elements` (M4.4): `limit` entries of the model `model` names (decision 33), of the
    /// feature `feature`'s solid or of every body, after `after`'s entry ([`list`]). A read
    /// of a published model asks the kernel nothing: its solids were described as it was
    /// rebuilt.
    pub fn elements(
        &self,
        session: &SessionId,
        model: ModelChoice,
        feature: Option<FeatureId>,
        after: Option<Cursor>,
        limit: u32,
    ) -> Running<Listed> {
        self.read_model(session, "elements", model, move |read| {
            list(
                read.model(),
                read.model_read().revision,
                feature.as_ref(),
                after.as_ref(),
                limit,
            )
        })
    }

    /// `query` (M4.4): the entries of the model `model` names that match `filter`
    /// ([`query`]).
    pub fn query(
        &self,
        session: &SessionId,
        model: ModelChoice,
        filter: Query,
    ) -> Running<Queried> {
        self.read_model(session, "query", model, move |read| {
            query(read.model(), &filter)
        })
    }
}

/// The most entries one `elements` or `query` call returns, and the most a `limit` asks for.
pub const ENTRIES_AT_MOST: u32 = 1000;

/// One face, edge, or vertex, as `elements` and `query` give it: every field always there,
/// `None` where it doesn't apply (the design's *Element entries*, whose order the fields
/// keep). Lengths are rounded to the nanometre, areas to 6 decimals, and angles to 9, once,
/// at the end; directions are unit vectors, a zero written `0`, never `-0`.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Its own name: the first of its names in the element map.
    pub name: Arc<Name>,
    /// Its other names, in the map's order, each with its stability.
    pub aliases: Vec<Alias>,
    /// Whether `name` is stable (`ElementMap::is_unstable`).
    pub stable: bool,
    /// The feature whose solid it's of.
    pub feature: FeatureId,
    pub kind: ElementKind,
    pub r#type: ElementType,
    /// A face's area.
    pub area: Option<Measure>,
    /// An edge's length.
    pub length: Option<Length>,
    /// A vertex's point.
    pub position: Option<[Length; 3]>,
    /// A planar face's outward unit normal.
    pub normal: Option<[f64; 3]>,
    /// A face's centre of area.
    pub centroid: Option<[Length; 3]>,
    /// An edge's ends, in its curve's own order.
    pub start: Option<[Length; 3]>,
    pub end: Option<[Length; 3]>,
    /// A straight edge's middle.
    pub midpoint: Option<[Length; 3]>,
    /// Whether an edge is straight: a line, or a Bézier or B-spline of two poles.
    pub straight: Option<bool>,
    /// A cylinder's, cone's, sphere's, torus's, circle's, or arc's axis.
    pub axis: Option<EntryAxis>,
    /// A cylinder's, sphere's, circle's, or arc's radius.
    pub radius: Option<Length>,
    /// Twice the radius, for a circle or arc edge and a cylinder face.
    pub diameter: Option<Length>,
    /// A torus's radii.
    pub major_radius: Option<Length>,
    pub minor_radius: Option<Length>,
    /// A cone's half-angle in degrees, keeping its sign, and its apex (`None` where it lies
    /// further than a length holds: a cone all but a cylinder).
    pub half_angle: Option<Angle>,
    pub apex: Option<[Length; 3]>,
    /// For a planar face, the axis a sketch on it would take as its x by default (F1).
    pub default_x: Option<AxisToken>,
}

/// One of an element's other names, and whether it's stable.
#[derive(Debug, Clone, PartialEq)]
pub struct Alias {
    pub name: Arc<Name>,
    pub stable: bool,
}

/// An axis: a point on it, and its unit direction, as its surface or curve has it.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryAxis {
    pub point: [Length; 3],
    pub direction: [f64; 3],
}

/// An element's geometry, as OCCT names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementType {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    Bezier,
    Bspline,
    Revolution,
    Extrusion,
    Offset,
    Other,
    Line,
    Circle,
    Ellipse,
    Hyperbola,
    Parabola,
    Point,
}

impl ElementType {
    /// Every type, faces' then edges' then a vertex's.
    pub const ALL: [ElementType; 17] = [
        ElementType::Plane,
        ElementType::Cylinder,
        ElementType::Cone,
        ElementType::Sphere,
        ElementType::Torus,
        ElementType::Bezier,
        ElementType::Bspline,
        ElementType::Revolution,
        ElementType::Extrusion,
        ElementType::Offset,
        ElementType::Other,
        ElementType::Line,
        ElementType::Circle,
        ElementType::Ellipse,
        ElementType::Hyperbola,
        ElementType::Parabola,
        ElementType::Point,
    ];

    /// Its word in an entry.
    pub fn word(self) -> &'static str {
        match self {
            ElementType::Plane => "plane",
            ElementType::Cylinder => "cylinder",
            ElementType::Cone => "cone",
            ElementType::Sphere => "sphere",
            ElementType::Torus => "torus",
            ElementType::Bezier => "bezier",
            ElementType::Bspline => "bspline",
            ElementType::Revolution => "revolution",
            ElementType::Extrusion => "extrusion",
            ElementType::Offset => "offset",
            ElementType::Other => "other",
            ElementType::Line => "line",
            ElementType::Circle => "circle",
            ElementType::Ellipse => "ellipse",
            ElementType::Hyperbola => "hyperbola",
            ElementType::Parabola => "parabola",
            ElementType::Point => "point",
        }
    }

    /// Whether an element of `kind` can be of this type.
    pub fn is_of(self, kind: ElementKind) -> bool {
        match kind {
            ElementKind::Face => matches!(
                self,
                ElementType::Plane
                    | ElementType::Cylinder
                    | ElementType::Cone
                    | ElementType::Sphere
                    | ElementType::Torus
                    | ElementType::Bezier
                    | ElementType::Bspline
                    | ElementType::Revolution
                    | ElementType::Extrusion
                    | ElementType::Offset
                    | ElementType::Other
            ),
            ElementKind::Edge => matches!(
                self,
                ElementType::Line
                    | ElementType::Circle
                    | ElementType::Ellipse
                    | ElementType::Hyperbola
                    | ElementType::Parabola
                    | ElementType::Bezier
                    | ElementType::Bspline
                    | ElementType::Offset
                    | ElementType::Other
            ),
            ElementKind::Vertex => self == ElementType::Point,
            ElementKind::Solid => false,
        }
    }
}

/// A global axis, as a sketch on a face names its x (F1): `+x`, `-x`, `+y`, `-y`, `+z`, `-z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisToken {
    PlusX,
    MinusX,
    PlusY,
    MinusY,
    PlusZ,
    MinusZ,
}

impl AxisToken {
    pub fn word(self) -> &'static str {
        match self {
            AxisToken::PlusX => "+x",
            AxisToken::MinusX => "-x",
            AxisToken::PlusY => "+y",
            AxisToken::MinusY => "-y",
            AxisToken::PlusZ => "+z",
            AxisToken::MinusZ => "-z",
        }
    }

    /// The default x of a sketch on a planar face of outward normal `n` (F1: FreeCAD's legacy
    /// nearest-axis table, in global axes, docs/ports/attacher.md): the axis `n` is nearest,
    /// ties going to x, then y, then z; nearest x gives `+y` if `n` points along +x, else
    /// `-y`; nearest y gives `-x` if it points along +y, else `+x`; nearest z gives `+x`.
    pub fn default_x(n: [f64; 3]) -> Self {
        let [x, y, z] = n.map(f64::abs);
        if x >= y && x >= z {
            if n[0] > 0.0 {
                AxisToken::PlusY
            } else {
                AxisToken::MinusY
            }
        } else if y >= z {
            if n[1] > 0.0 {
                AxisToken::MinusX
            } else {
                AxisToken::PlusX
            }
        } else {
            AxisToken::PlusX
        }
    }
}

/// A measure an entry has, which `query` filters and orders by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasureName {
    Area,
    Length,
    Radius,
    Diameter,
    MajorRadius,
    MinorRadius,
    HalfAngle,
}

impl MeasureName {
    pub const ALL: [MeasureName; 7] = [
        MeasureName::Area,
        MeasureName::Length,
        MeasureName::Radius,
        MeasureName::Diameter,
        MeasureName::MajorRadius,
        MeasureName::MinorRadius,
        MeasureName::HalfAngle,
    ];

    pub fn word(self) -> &'static str {
        match self {
            MeasureName::Area => "area",
            MeasureName::Length => "length",
            MeasureName::Radius => "radius",
            MeasureName::Diameter => "diameter",
            MeasureName::MajorRadius => "major_radius",
            MeasureName::MinorRadius => "minor_radius",
            MeasureName::HalfAngle => "half_angle",
        }
    }

    /// Whether an element of `kind` can have it: a face its area, a cylinder's or sphere's
    /// radius, a cylinder's diameter, a torus's radii, and a cone's half-angle; an edge its
    /// length, and a circle's or arc's radius and diameter; a vertex none.
    pub fn is_of(self, kind: ElementKind) -> bool {
        match kind {
            ElementKind::Face => self != MeasureName::Length,
            ElementKind::Edge => matches!(
                self,
                MeasureName::Length | MeasureName::Radius | MeasureName::Diameter
            ),
            ElementKind::Vertex | ElementKind::Solid => false,
        }
    }

    /// The measure's quantity, as written: an area, a length, or an angle.
    fn quantity(self) -> QuantityKind {
        match self {
            MeasureName::Area => QuantityKind::Area,
            MeasureName::HalfAngle => QuantityKind::Angle,
            MeasureName::Length
            | MeasureName::Radius
            | MeasureName::Diameter
            | MeasureName::MajorRadius
            | MeasureName::MinorRadius => QuantityKind::Length,
        }
    }

    /// The entry's value of it, as written, if it has it.
    fn of(self, entry: &Entry) -> Option<Quantity> {
        match self {
            MeasureName::Area => entry.area.map(Quantity::Area),
            MeasureName::Length => entry.length.map(Quantity::Length),
            MeasureName::Radius => entry.radius.map(Quantity::Length),
            MeasureName::Diameter => entry.diameter.map(Quantity::Length),
            MeasureName::MajorRadius => entry.major_radius.map(Quantity::Length),
            MeasureName::MinorRadius => entry.minor_radius.map(Quantity::Length),
            MeasureName::HalfAngle => entry.half_angle.map(Quantity::Angle),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuantityKind {
    Area,
    Length,
    Angle,
}

/// A measure's value, as written: compared exactly, so values equal as written are equal
/// (decision 19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Area(Measure),
    Length(Length),
    Angle(Angle),
}

impl Quantity {
    fn kind(self) -> QuantityKind {
        match self {
            Quantity::Area(_) => QuantityKind::Area,
            Quantity::Length(_) => QuantityKind::Length,
            Quantity::Angle(_) => QuantityKind::Angle,
        }
    }

    /// Its value in its unit's least step (millionths of a mm² or mm³, nanometres,
    /// nanodegrees), for comparing two of one kind.
    fn steps(self) -> i128 {
        match self {
            Quantity::Area(m) => m.millionths(),
            Quantity::Length(l) => i128::from(l.nm()),
            Quantity::Angle(a) => i128::from(a.ndeg()),
        }
    }
}

/// Each of the feature `id`'s solid's elements as entries, in the element list's order:
/// faces, then edges, then vertices, each by name, byte by byte. Refused, naming `feature`,
/// for a feature the document doesn't have, a sketch, or one that didn't build or couldn't
/// be measured (its failure, as `check` reports it).
pub fn entries(model: &Model, id: &FeatureId) -> Result<Vec<Entry>, Problems> {
    let built = match model.outcome(id) {
        None => {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!("the document has no feature {id}"),
            )
            .with_parameter("feature")
            .with_suggestion("name a feature of the document, by its ID")
            .into());
        }
        Some(FeatureOutcome::Sketch(_)) => {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!("{id} is a sketch, which makes no solid"),
            )
            .with_feature(id.as_str())
            .with_parameter("feature")
            .with_suggestion("name a feature that makes a solid")
            .into());
        }
        Some(FeatureOutcome::Failed(error)) => return Err(error.clone().into()),
        Some(FeatureOutcome::Broken { errors, total }) => {
            return Err(Problems::with_unlisted(
                errors.clone(),
                total.saturating_sub(errors.len() as u64),
            ));
        }
        Some(FeatureOutcome::Blocked { on }) => {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!("{id} didn't build, since it uses {on}, which failed"),
            )
            .with_feature(id.as_str())
            .with_parameter("feature")
            .with_suggestion(format!("mend {on} first: `check` says why it failed"))
            .into());
        }
        Some(FeatureOutcome::Built { built, .. }) => built,
    };
    let Some(description) = built.description() else {
        // Why: its description refused or failed, or, before that, its summary.
        return Err(
            match built.description_refused().or(built.measure_refused()) {
                Some(refused) => refused.clone(),
                None => Error::new(
                    ErrorCode::KernelFailed,
                    format!("{id}'s solid couldn't be described, so its elements aren't known"),
                )
                .with_suggestion("check the document again: its rebuild describes it again"),
            }
            .with_feature(id.as_str())
            .with_parameter("feature")
            .into(),
        );
    };
    let Some(map) = model.element_map(id) else {
        return Err(fault(id, "its solid built, but its elements couldn't be named").into());
    };
    elements(map, description.clone())?
        .into_iter()
        .map(|element| entry(map, id, element))
        .collect::<Result<_, _>>()
        .map_err(Problems::from)
}

/// An error in Silico itself, of the feature `id`'s solid: a fault, not the document's.
fn fault(id: &FeatureId, cause: &str) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("{id}'s elements couldn't be listed"),
    )
    .with_feature(id.as_str())
    .with_cause(cause)
    .with_suggestion(crate::worker::SILICO_BUG)
}

/// `element`, of the feature `id`'s solid, whose names `map` holds, as an entry.
fn entry(map: &ElementMap, id: &FeatureId, element: Element) -> Result<Entry, Error> {
    let length =
        |mm: f64| Length::rounded(mm).ok_or_else(|| fault(id, "a length past what a length holds"));
    let point = |p: [f64; 3]| -> Result<[Length; 3], Error> {
        Ok([length(p[0])?, length(p[1])?, length(p[2])?])
    };
    let axis = |a: &Axis| -> Result<EntryAxis, Error> {
        Ok(EntryAxis {
            point: point(a.origin)?,
            direction: direction(a.direction),
        })
    };
    let mut entry = Entry {
        stable: !map.is_unstable(&element.name),
        aliases: element
            .aliases
            .iter()
            .map(|name| Alias {
                stable: !map.is_unstable(name),
                name: Arc::clone(name),
            })
            .collect(),
        name: element.name,
        feature: id.clone(),
        kind: ElementKind::Vertex,
        r#type: ElementType::Point,
        area: None,
        length: None,
        position: None,
        normal: None,
        centroid: None,
        start: None,
        end: None,
        midpoint: None,
        straight: None,
        axis: None,
        radius: None,
        diameter: None,
        major_radius: None,
        minor_radius: None,
        half_angle: None,
        apex: None,
        default_x: None,
    };
    match element.geometry {
        Geometry::Face(face) => {
            entry.kind = ElementKind::Face;
            entry.area = Some(
                Measure::rounded(face.area)
                    .ok_or_else(|| fault(id, "an area past what an area holds"))?,
            );
            entry.centroid = Some(point(face.centroid)?);
            entry.r#type = match face.surface {
                Surface::Plane { normal } => {
                    entry.normal = Some(direction(normal));
                    entry.default_x = Some(AxisToken::default_x(normal));
                    ElementType::Plane
                }
                Surface::Cylinder { axis: a, radius } => {
                    entry.axis = Some(axis(&a)?);
                    let r = length(radius)?;
                    entry.radius = Some(r);
                    entry.diameter = Some(twice(id, r)?);
                    ElementType::Cylinder
                }
                Surface::Cone {
                    axis: a,
                    radius,
                    half_angle,
                } => {
                    entry.axis = Some(axis(&a)?);
                    entry.half_angle = Some(
                        Angle::rounded(half_angle.to_degrees())
                            .ok_or_else(|| fault(id, "a cone's half-angle isn't an angle"))?,
                    );
                    entry.apex = apex(&a, radius, half_angle);
                    ElementType::Cone
                }
                Surface::Sphere { axis: a, radius } => {
                    entry.axis = Some(axis(&a)?);
                    entry.radius = Some(length(radius)?);
                    ElementType::Sphere
                }
                Surface::Torus {
                    axis: a,
                    major_radius,
                    minor_radius,
                } => {
                    entry.axis = Some(axis(&a)?);
                    entry.major_radius = Some(length(major_radius)?);
                    entry.minor_radius = Some(length(minor_radius)?);
                    ElementType::Torus
                }
                Surface::Bezier => ElementType::Bezier,
                Surface::Bspline => ElementType::Bspline,
                Surface::Revolution => ElementType::Revolution,
                Surface::Extrusion => ElementType::Extrusion,
                Surface::Offset => ElementType::Offset,
                Surface::Other => ElementType::Other,
            };
        }
        Geometry::Edge(edge) => {
            entry.kind = ElementKind::Edge;
            entry.length = Some(length(edge.length)?);
            entry.start = Some(point(edge.start)?);
            entry.end = Some(point(edge.end)?);
            let straight = edge.curve.is_straight();
            entry.straight = Some(straight);
            if straight {
                entry.midpoint = Some(point(edge.middle)?);
            }
            entry.r#type = match edge.curve {
                EdgeCurve::Line => ElementType::Line,
                EdgeCurve::Circle { axis: a, radius } => {
                    entry.axis = Some(axis(&a)?);
                    let r = length(radius)?;
                    entry.radius = Some(r);
                    entry.diameter = Some(twice(id, r)?);
                    ElementType::Circle
                }
                EdgeCurve::Ellipse { .. } => ElementType::Ellipse,
                EdgeCurve::Hyperbola => ElementType::Hyperbola,
                EdgeCurve::Parabola => ElementType::Parabola,
                EdgeCurve::Bezier { .. } => ElementType::Bezier,
                EdgeCurve::Bspline { .. } => ElementType::Bspline,
                EdgeCurve::Offset => ElementType::Offset,
                // A sphere's pole, a cone's apex: no curve to speak of.
                EdgeCurve::Other | EdgeCurve::Degenerated => ElementType::Other,
            };
        }
        Geometry::Vertex(at) => {
            entry.position = Some(point(at)?);
        }
    }
    Ok(entry)
}

/// A direction as an entry writes it: a zero `0`, never `-0`.
fn direction(d: [f64; 3]) -> [f64; 3] {
    d.map(|c| if c == 0.0 { 0.0 } else { c })
}

/// Twice `radius`, exactly: a diameter as written is twice its radius as written.
fn twice(id: &FeatureId, radius: Length) -> Result<Length, Error> {
    radius
        .nm()
        .checked_mul(2)
        .map(Length::from_nm)
        .ok_or_else(|| fault(id, "a diameter past what a length holds"))
}

/// A cone's apex: where its radius would be 0, `radius` at `axis.origin` and shrinking at
/// `half_angle` (radians, signed) along `axis.direction`. `None` where it lies past what a
/// length holds (a cone all but a cylinder), or the cone has no apex (a half-angle of 0).
fn apex(axis: &Axis, radius: f64, half_angle: f64) -> Option<[Length; 3]> {
    let along = radius / half_angle.tan();
    if !along.is_finite() {
        return None;
    }
    let at = |i: usize| Length::rounded(axis.origin[i] - axis.direction[i] * along);
    Some([at(0)?, at(1)?, at(2)?])
}

/// Where a listing of elements goes on from: the revision it's of, and the last entry
/// listed, by its solid's feature and its own name.
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    pub revision: Revision,
    pub feature: FeatureId,
    pub name: String,
}

/// A page of `elements`: its entries, how many there are in all, and where the next page
/// starts (`None` after the last).
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub entries: Vec<Entry>,
    pub total: u64,
    pub next: Option<Cursor>,
}

/// `elements` on `model`, of `revision`: `limit` entries of the feature `feature`'s solid,
/// or, for none, of every body (the solids no later feature consumed, in timeline order),
/// after `after`'s entry. Every reason it's refused is given, in the order found.
pub fn list(
    model: &Model,
    revision: Revision,
    feature: Option<&FeatureId>,
    after: Option<&Cursor>,
    limit: u32,
) -> Result<Listed, Problems> {
    let mut refused = Vec::new();
    if let Some(error) = limit_refused(limit) {
        refused.push(error);
    }
    if let Some(cursor) = after
        && cursor.revision != revision
    {
        refused.push(
            Error::new(
                ErrorCode::Conflict,
                format!(
                    "the cursor is of revision {}, and the model read is of revision {}",
                    cursor.revision.number(),
                    revision.number()
                ),
            )
            .with_parameter("after")
            .with_suggestion("start again with after set to null"),
        );
    }
    if !refused.is_empty() {
        return Err(Problems::of(refused));
    }
    let solids: Vec<FeatureId> = match feature {
        Some(id) => vec![id.clone()],
        None => model.bodies().to_vec(),
    };
    let mut all = Vec::new();
    for id in &solids {
        all.extend(entries(model, id)?);
    }
    let from = match after {
        None => 0,
        Some(cursor) => {
            match all
                .iter()
                .position(|e| e.feature == cursor.feature && e.name.to_string() == cursor.name)
            {
                Some(at) => at + 1,
                None => {
                    return Err(Error::new(
                        ErrorCode::InvalidInput,
                        format!(
                            "the cursor names {} of {}, which this listing doesn't hold",
                            cursor.name, cursor.feature
                        ),
                    )
                    .with_parameter("after")
                    .with_suggestion("start again with after set to null, or pass the last call's next as it was")
                    .into());
                }
            }
        }
    };
    let total = all.len() as u64;
    let to = from.saturating_add(limit as usize).min(all.len());
    let entries: Vec<Entry> = all.drain(from..to).collect();
    let next = (to < total as usize)
        .then(|| entries.last())
        .flatten()
        .map(|last| Cursor {
            revision,
            feature: last.feature.clone(),
            name: last.name.to_string(),
        });
    Ok(Listed {
        entries,
        total,
        next,
    })
}

/// The refusal of a `limit` past what a call returns, if it is.
fn limit_refused(limit: u32) -> Option<Error> {
    (limit == 0 || limit > ENTRIES_AT_MOST).then(|| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("limit must be from 1 to {ENTRIES_AT_MOST}, got {limit}"),
        )
        .with_parameter("limit")
        .with_suggestion(format!(
            "list in pages of at most {ENTRIES_AT_MOST}, with after set to the last call's next"
        ))
    })
}

/// `query`'s filter: a kind, and optionally a type, stability, a direction a planar face
/// faces, ranges of measures, and an order (plan decision 4).
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub feature: Option<FeatureId>,
    pub kind: ElementKind,
    pub r#type: Option<ElementType>,
    /// `Some(true)`: elements with a stable name (their own or an alias); `Some(false)`:
    /// those with none.
    pub stable: Option<bool>,
    pub facing: Option<Facing>,
    pub measures: Vec<Range>,
    pub order: Option<Order>,
    pub limit: u32,
}

/// Faces facing `direction`: whose outward normal is at most `within` from it.
#[derive(Debug, Clone, PartialEq)]
pub struct Facing {
    pub direction: [f64; 3],
    pub within: Angle,
}

/// A measure from `at_least` to `at_most`, either end open, both included.
#[derive(Debug, Clone, PartialEq)]
pub struct Range {
    pub measure: MeasureName,
    pub at_least: Option<Quantity>,
    pub at_most: Option<Quantity>,
}

/// Entries by a measure, the smallest first unless `descending`. Only entries that have
/// the measure are ordered by it, so ordering by one keeps only those.
#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub by: MeasureName,
    pub descending: bool,
}

/// `query`'s answer: the entries, how many matched the filter before the limit, and the
/// tie at the limit's cut, if there is one.
#[derive(Debug, Clone, PartialEq)]
pub struct Queried {
    pub entries: Vec<Entry>,
    pub matched: u64,
    pub tied: Option<Tied>,
}

/// Entries equal by the order's measure, as written, across the limit's cut: how many, and
/// whether they're all returned (past `limit`), or none of them are.
#[derive(Debug, Clone, PartialEq)]
pub struct Tied {
    pub count: u64,
    pub included: bool,
}

/// `query` on `model`: the entries that match `query`, in its order (or `elements`'s), the
/// first `limit` of them. A tie at the cut is never broken: it's returned whole, past
/// `limit`, if it ends within [`ENTRIES_AT_MOST`] entries, and otherwise none of it is
/// (decision 4). Every reason the query is refused is given, in the order found.
pub fn query(model: &Model, query: &Query) -> Result<Queried, Problems> {
    let refused = query_refused(query);
    if !refused.is_empty() {
        return Err(Problems::of(refused));
    }
    let solids: Vec<FeatureId> = match &query.feature {
        Some(id) => vec![id.clone()],
        None => model.bodies().to_vec(),
    };
    let mut matched = Vec::new();
    for id in &solids {
        matched.extend(
            entries(model, id)?
                .into_iter()
                .filter(|entry| matches(entry, query)),
        );
    }
    let Some(order) = &query.order else {
        let count = matched.len() as u64;
        matched.truncate(query.limit as usize);
        return Ok(Queried {
            entries: matched,
            matched: count,
            tied: None,
        });
    };
    let mut ordered: Vec<(i128, Entry)> = matched
        .into_iter()
        .filter_map(|entry| order.by.of(&entry).map(|v| (v.steps(), entry)))
        .collect();
    // Stable, so entries equal by the measure keep `elements`'s order.
    ordered.sort_by(|(a, _), (b, _)| {
        let by = a.cmp(b);
        if order.descending { by.reverse() } else { by }
    });
    let count = ordered.len() as u64;
    let limit = query.limit as usize;
    let cut = match (ordered.get(limit.wrapping_sub(1)), ordered.get(limit)) {
        (Some((last, _)), Some((next, _))) if last == next => Some(*last),
        _ => None,
    };
    let (keep, tied) = match cut {
        None => (limit.min(ordered.len()), None),
        Some(value) => {
            let first = ordered.iter().position(|(v, _)| *v == value).unwrap_or(0);
            let end = first
                + ordered[first..]
                    .iter()
                    .take_while(|(v, _)| *v == value)
                    .count();
            let fits = end <= ENTRIES_AT_MOST as usize;
            (
                if fits { end } else { first },
                Some(Tied {
                    count: (end - first) as u64,
                    included: fits,
                }),
            )
        }
    };
    ordered.truncate(keep);
    Ok(Queried {
        entries: ordered.into_iter().map(|(_, entry)| entry).collect(),
        matched: count,
        tied,
    })
}

/// Whether `entry` matches `query`'s filter (its order's measure aside).
fn matches(entry: &Entry, query: &Query) -> bool {
    entry.kind == query.kind
        && query.r#type.is_none_or(|t| entry.r#type == t)
        && query.stable.is_none_or(|stable| {
            (entry.stable || entry.aliases.iter().any(|alias| alias.stable)) == stable
        })
        && query
            .facing
            .as_ref()
            .is_none_or(|facing| entry.normal.is_some_and(|normal| faces(normal, facing)))
        && query.measures.iter().all(|range| {
            range.measure.of(entry).is_some_and(|value| {
                range
                    .at_least
                    .is_none_or(|low| value.steps() >= low.steps())
                    && range
                        .at_most
                        .is_none_or(|high| value.steps() <= high.steps())
            })
        })
}

/// Whether a face of outward normal `normal` faces `facing.direction` within its angle: the
/// angle between them, as written (to the nanodegree), at most `within`.
fn faces(normal: [f64; 3], facing: &Facing) -> bool {
    let d = facing.direction;
    let cross = [
        normal[1] * d[2] - normal[2] * d[1],
        normal[2] * d[0] - normal[0] * d[2],
        normal[0] * d[1] - normal[1] * d[0],
    ];
    let sin = cross.iter().map(|c| c * c).sum::<f64>().sqrt();
    let cos: f64 = normal.iter().zip(d).map(|(n, d)| n * d).sum();
    Angle::rounded(sin.atan2(cos).to_degrees()).is_some_and(|angle| angle <= facing.within)
}

/// Every reason `query` is refused, in the order its fields come.
fn query_refused(query: &Query) -> Vec<Error> {
    let mut refused = Vec::new();
    let invalid = |message: String, parameter: &str| {
        Error::new(ErrorCode::InvalidInput, message).with_parameter(parameter)
    };
    if query.kind == ElementKind::Solid {
        refused.push(
            invalid(
                "kind must be face, edge, or vertex, got solid".to_owned(),
                "kind",
            )
            .with_suggestion("give one of the kinds")
            .with_candidates(["face", "edge", "vertex"].map(str::to_owned)),
        );
        return refused;
    }
    let kind = kind_word(query.kind);
    let a_kind = a_kind(query.kind);
    if let Some(t) = query.r#type
        && !t.is_of(query.kind)
    {
        refused.push(
            invalid(format!("{a_kind} is never of type {}", t.word()), "type")
                .with_suggestion(format!("give a type {a_kind} can have"))
                .with_candidates(
                    ElementType::ALL
                        .into_iter()
                        .filter(|t| t.is_of(query.kind))
                        .map(|t| t.word().to_owned()),
                ),
        );
    }
    if let Some(facing) = &query.facing {
        if query.kind != ElementKind::Face {
            refused.push(
                invalid(
                    format!("only planar faces face a direction, and kind is {kind}"),
                    "facing",
                )
                .with_suggestion("query faces of type plane to filter by direction"),
            );
        } else if query.r#type.is_some_and(|t| t != ElementType::Plane) {
            refused.push(
                invalid(
                    "only planar faces face a direction, and type isn't plane".to_owned(),
                    "facing",
                )
                .with_suggestion(
                    "query faces of type plane, or of no type, to filter by direction",
                ),
            );
        }
        let d = facing.direction;
        let size = d.iter().map(|c| c * c).sum::<f64>().sqrt();
        if !(size.is_finite() && size > 0.0) {
            refused.push(
                invalid(
                    format!(
                        "facing.direction must be a direction, got [{}, {}, {}]",
                        d[0], d[1], d[2]
                    ),
                    "facing.direction",
                )
                .with_suggestion("give a direction that isn't zero"),
            );
        }
        if facing.within < Angle::from_ndeg(0) || facing.within > Angle::from_ndeg(180_000_000_000)
        {
            refused.push(
                invalid(
                    format!(
                        "facing.within must be from 0 to 180 degrees, got {}",
                        facing.within
                    ),
                    "facing.within",
                )
                .with_suggestion(
                    "give the angle a face's normal may be off the direction, in degrees",
                ),
            );
        }
    }
    for (i, range) in query.measures.iter().enumerate() {
        let at = |field: &str| format!("measures.{i}.{field}");
        if !range.measure.is_of(query.kind) {
            refused.push(
                invalid(
                    format!("{a_kind} has no {}", range.measure.word()),
                    &at("measure"),
                )
                .with_suggestion(format!("give a measure {a_kind} has"))
                .with_candidates(measures_of(query.kind)),
            );
        }
        for (field, bound) in [("at_least", range.at_least), ("at_most", range.at_most)] {
            if let Some(bound) = bound
                && bound.kind() != range.measure.quantity()
            {
                refused.push(
                    invalid(
                        format!(
                            "{} is {}, and {field} isn't",
                            range.measure.word(),
                            quantity_word(range.measure.quantity())
                        ),
                        &at(field),
                    )
                    .with_suggestion(format!(
                        "give {field} as {}",
                        quantity_word(range.measure.quantity())
                    )),
                );
            }
        }
        if let (Some(low), Some(high)) = (range.at_least, range.at_most)
            && low.kind() == high.kind()
            && low.steps() > high.steps()
        {
            refused.push(
                invalid(
                    format!("measures.{i} is empty: at_least is more than at_most"),
                    &at("at_least"),
                )
                .with_suggestion("give at_least no more than at_most"),
            );
        }
    }
    if let Some(order) = &query.order
        && !order.by.is_of(query.kind)
    {
        refused.push(
            invalid(format!("{a_kind} has no {}", order.by.word()), "order.by")
                .with_suggestion(format!("order by a measure {a_kind} has"))
                .with_candidates(measures_of(query.kind)),
        );
    }
    if let Some(error) = limit_refused(query.limit) {
        refused.push(error);
    }
    refused.truncate(PROBLEMS_LISTED);
    refused
}

fn kind_word(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Face => "face",
        ElementKind::Edge => "edge",
        ElementKind::Vertex => "vertex",
        ElementKind::Solid => "solid",
    }
}

/// "a face", "an edge", "a vertex".
fn a_kind(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Face => "a face",
        ElementKind::Edge => "an edge",
        ElementKind::Vertex => "a vertex",
        ElementKind::Solid => "a solid",
    }
}

fn quantity_word(kind: QuantityKind) -> &'static str {
    match kind {
        QuantityKind::Area => "an area",
        QuantityKind::Length => "a length",
        QuantityKind::Angle => "an angle",
    }
}

/// The measures an element of `kind` can have, by name.
fn measures_of(kind: ElementKind) -> impl Iterator<Item = String> {
    MeasureName::ALL
        .into_iter()
        .filter(move |m| m.is_of(kind))
        .map(|m| m.word().to_owned())
}
