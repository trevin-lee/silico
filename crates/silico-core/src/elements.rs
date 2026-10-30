//! A solid's elements, each named and measured (M4.4, docs/design/m4.4-describe.md): the
//! kernel's description of the solid ([`KernelRequest::describe`](crate::kernel::KernelRequest::describe))
//! named by its element map (M2.7), in the element list's order: faces, then edges, then
//! vertices, each by name, byte by byte (docs/design/m4.1-command-api.md, `elements`), so
//! the list doesn't follow OCCT's numbering, which an upstream edit can change where it
//! keeps the names; and each as the entry `elements` and `query` give
//! (docs/design/m4.1-command-api.md, section 2's *Element entries*), which a model builds
//! once for each solid, the first time it's read (`Model::entries`).

use std::sync::Arc;

use crate::document::{Angle, FeatureId, Length, Measure};
use crate::kernel::{
    Axis, Description, EdgeCurve, EdgeDescription, ElementKind, ElementRef, FaceDescription,
    Surface,
};
use crate::naming::{ElementMap, Name};
use crate::{Error, ErrorCode};

/// One of a solid's faces, edges, or vertices: its own name, its aliases (any other
/// derivations that reach it: a reference by any of them finds it), the kernel's number for
/// it among its kind (from 1, in OCCT's order), and what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub name: Arc<Name>,
    pub aliases: Vec<Arc<Name>>,
    pub number: u32,
    pub geometry: Geometry,
}

impl Element {
    /// Which kind of element it is.
    pub fn kind(&self) -> ElementKind {
        match self.geometry {
            Geometry::Face(_) => ElementKind::Face,
            Geometry::Edge(_) => ElementKind::Edge,
            Geometry::Vertex(_) => ElementKind::Vertex,
        }
    }
}

/// What an element is and measures, as the kernel describes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    Face(FaceDescription),
    Edge(EdgeDescription),
    /// A vertex's point.
    Vertex([f64; 3]),
}

/// Each of a solid's elements, named by `map` and described by `description` (the solid's,
/// in OCCT's order), in the element list's order. Refused, as a fault, if the two don't
/// describe the same solid: a count that differs, or an element the map doesn't name.
pub fn elements(map: &ElementMap, description: Description) -> Result<Vec<Element>, Error> {
    let mut out = Vec::new();
    for (kind, geometries) in [
        (
            ElementKind::Face,
            description
                .faces
                .into_iter()
                .map(Geometry::Face)
                .collect::<Vec<_>>(),
        ),
        (
            ElementKind::Edge,
            description.edges.into_iter().map(Geometry::Edge).collect(),
        ),
        (
            ElementKind::Vertex,
            description
                .vertices
                .into_iter()
                .map(Geometry::Vertex)
                .collect(),
        ),
    ] {
        let named = map.count(kind);
        if named != geometries.len() {
            return Err(mismatch(
                map,
                format!(
                    "{}: the kernel describes {}, and its element map names {named}",
                    kind.plural(),
                    geometries.len()
                ),
            ));
        }
        let mut of_kind = Vec::with_capacity(geometries.len());
        for (index, geometry) in (1..).zip(geometries) {
            let element = ElementRef { kind, index };
            let mut names = map.names(element).into_iter();
            let Some(name) = names.next() else {
                return Err(mismatch(
                    map,
                    format!(
                        "its element map doesn't name its {}",
                        ElementRef { kind, index }
                    ),
                ));
            };
            of_kind.push((
                name.to_string(),
                Element {
                    name,
                    aliases: names.collect(),
                    number: index,
                    geometry,
                },
            ));
        }
        of_kind.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        out.extend(of_kind.into_iter().map(|(_, element)| element));
    }
    Ok(out)
}

fn mismatch(map: &ElementMap, cause: String) -> Error {
    Error::new(
        ErrorCode::Internal,
        "a solid's description and its element map don't describe the same solid",
    )
    .with_feature(map.feature().as_str())
    .with_cause(cause)
    .with_suggestion("report this with the document: it's a fault in Silico")
}

/// Each of a solid's elements as an entry, named by `map`, the feature `id`'s, and described
/// by `description`, in the element list's order ([`elements`]). Refused, as a fault, if the
/// two don't describe the same solid, or a value is past what an entry holds.
pub fn entries(
    map: &ElementMap,
    id: &FeatureId,
    description: &Description,
) -> Result<Vec<Entry>, Error> {
    elements(map, description.clone())?
        .into_iter()
        .map(|element| entry(map, id, element))
        .collect()
}

/// The name a stored reference to `element` writes: its own, if stable, or else its first
/// stable alias, in the map's order; none if every name of it is unstable. Each name judged
/// as an entry's `stable` and its aliases' are (`ElementMap::is_unstable`), so an element
/// this names is one `query`'s `"stable": true` keeps (M4.1 section 2: "a reference then
/// uses the stable alias").
pub fn stable_name(map: &ElementMap, element: ElementRef) -> Option<Arc<Name>> {
    map.names(element)
        .into_iter()
        .find(|name| !map.is_unstable(name))
}

/// An error in Silico itself, of the feature `id`'s solid: a fault, not the document's.
pub(crate) fn fault(id: &FeatureId, cause: &str) -> Error {
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
    let kind = element.kind();
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
        kind,
        // Each kind's arm below says its type.
        r#type: ElementType::Other,
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
    entry.r#type = match &element.geometry {
        Geometry::Face(face) => surface_type(&face.surface),
        Geometry::Edge(edge) => curve_type(&edge.curve),
        Geometry::Vertex(_) => ElementType::Point,
    };
    match element.geometry {
        Geometry::Face(face) => {
            entry.area = Some(
                Measure::rounded(face.area)
                    .ok_or_else(|| fault(id, "an area past what an area holds"))?,
            );
            entry.centroid = Some(point(face.centroid)?);
            match face.surface {
                Surface::Plane { normal } => {
                    entry.normal = Some(direction(normal));
                    entry.default_x = Some(AxisToken::default_x(normal));
                }
                Surface::Cylinder { axis: a, radius } => {
                    entry.axis = Some(axis(&a)?);
                    let r = length(radius)?;
                    entry.radius = Some(r);
                    entry.diameter = Some(twice(id, r)?);
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
                }
                Surface::Sphere { axis: a, radius } => {
                    entry.axis = Some(axis(&a)?);
                    entry.radius = Some(length(radius)?);
                }
                Surface::Torus {
                    axis: a,
                    major_radius,
                    minor_radius,
                } => {
                    entry.axis = Some(axis(&a)?);
                    entry.major_radius = Some(length(major_radius)?);
                    entry.minor_radius = Some(length(minor_radius)?);
                }
                Surface::Bezier
                | Surface::Bspline
                | Surface::Revolution
                | Surface::Extrusion
                | Surface::Offset
                | Surface::Other => {}
            }
        }
        Geometry::Edge(edge) => {
            entry.length = Some(length(edge.length)?);
            entry.start = Some(point(edge.start)?);
            entry.end = Some(point(edge.end)?);
            let straight = edge.curve.is_straight();
            entry.straight = Some(straight);
            if straight {
                entry.midpoint = Some(point(edge.middle)?);
            }
            match edge.curve {
                EdgeCurve::Line => {}
                EdgeCurve::Circle { axis: a, radius } => {
                    entry.axis = Some(axis(&a)?);
                    let r = length(radius)?;
                    entry.radius = Some(r);
                    entry.diameter = Some(twice(id, r)?);
                }
                // The design's entry gives an ellipse no axis or radii (docs/design/m4.4-describe.md).
                EdgeCurve::Ellipse { .. } => {}
                // No fields beyond length and ends (a degenerated edge, a sphere's pole or a
                // cone's apex, has no curve to speak of).
                EdgeCurve::Hyperbola
                | EdgeCurve::Parabola
                | EdgeCurve::Bezier { .. }
                | EdgeCurve::Bspline { .. }
                | EdgeCurve::Offset
                | EdgeCurve::Other
                | EdgeCurve::Degenerated => {}
            }
        }
        Geometry::Vertex(at) => {
            entry.position = Some(point(at)?);
        }
    };
    Ok(entry)
}

/// A curve's type, as an entry names it.
pub(crate) fn curve_type(curve: &EdgeCurve) -> ElementType {
    match curve {
        EdgeCurve::Line => ElementType::Line,
        EdgeCurve::Circle { .. } => ElementType::Circle,
        EdgeCurve::Ellipse { .. } => ElementType::Ellipse,
        EdgeCurve::Hyperbola => ElementType::Hyperbola,
        EdgeCurve::Parabola => ElementType::Parabola,
        EdgeCurve::Bezier { .. } => ElementType::Bezier,
        EdgeCurve::Bspline { .. } => ElementType::Bspline,
        EdgeCurve::Offset => ElementType::Offset,
        EdgeCurve::Other | EdgeCurve::Degenerated => ElementType::Other,
    }
}

/// A surface's type, as an entry names it.
pub(crate) fn surface_type(surface: &Surface) -> ElementType {
    match surface {
        Surface::Plane { .. } => ElementType::Plane,
        Surface::Cylinder { .. } => ElementType::Cylinder,
        Surface::Cone { .. } => ElementType::Cone,
        Surface::Sphere { .. } => ElementType::Sphere,
        Surface::Torus { .. } => ElementType::Torus,
        Surface::Bezier => ElementType::Bezier,
        Surface::Bspline => ElementType::Bspline,
        Surface::Revolution => ElementType::Revolution,
        Surface::Extrusion => ElementType::Extrusion,
        Surface::Offset => ElementType::Offset,
        Surface::Other => ElementType::Other,
    }
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
    let along = radius / libm::tan(half_angle);
    if !along.is_finite() {
        return None;
    }
    let at = |i: usize| Length::rounded(axis.origin[i] - axis.direction[i] * along);
    Some([at(0)?, at(1)?, at(2)?])
}

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
        match self {
            ElementType::Plane
            | ElementType::Cylinder
            | ElementType::Cone
            | ElementType::Sphere
            | ElementType::Torus
            | ElementType::Revolution
            | ElementType::Extrusion => kind == ElementKind::Face,
            ElementType::Bezier
            | ElementType::Bspline
            | ElementType::Offset
            | ElementType::Other => {
                matches!(kind, ElementKind::Face | ElementKind::Edge)
            }
            ElementType::Line
            | ElementType::Circle
            | ElementType::Ellipse
            | ElementType::Hyperbola
            | ElementType::Parabola => kind == ElementKind::Edge,
            ElementType::Point => kind == ElementKind::Vertex,
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
    pub const ALL: [AxisToken; 6] = [
        AxisToken::PlusX,
        AxisToken::MinusX,
        AxisToken::PlusY,
        AxisToken::MinusY,
        AxisToken::PlusZ,
        AxisToken::MinusZ,
    ];

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::FeatureId;
    use crate::kernel::{EdgeCurve, ElementCounts, Surface};
    use crate::naming::OpCode;

    #[test]
    fn each_axis_token_is_its_signed_axis_and_default_x_follows_f1() {
        let words: Vec<&str> = [
            AxisToken::PlusX,
            AxisToken::MinusX,
            AxisToken::PlusY,
            AxisToken::MinusY,
            AxisToken::PlusZ,
            AxisToken::MinusZ,
        ]
        .into_iter()
        .map(AxisToken::word)
        .collect();
        assert_eq!(words, ["+x", "-x", "+y", "-y", "+z", "-z"]);
        // F1: nearest x gives +y along +x, else -y; nearest y gives -x along +y, else +x;
        // nearest z gives +x either way; ties go to x, then y.
        for (normal, want) in [
            ([1.0, 0.0, 0.0], AxisToken::PlusY),
            ([-1.0, 0.0, 0.0], AxisToken::MinusY),
            ([0.0, 1.0, 0.0], AxisToken::MinusX),
            ([0.0, -1.0, 0.0], AxisToken::PlusX),
            ([0.0, 0.0, 1.0], AxisToken::PlusX),
            ([0.0, 0.0, -1.0], AxisToken::PlusX),
            ([0.6, -0.6, 0.5], AxisToken::PlusY),
            ([0.0, -0.6, 0.6], AxisToken::PlusX),
        ] {
            assert_eq!(AxisToken::default_x(normal), want, "{normal:?}");
        }
    }

    fn map(faces: u32, edges: u32, vertices: u32) -> ElementMap {
        ElementMap::by_number(
            FeatureId::new("k7f2qx3m").unwrap(),
            OpCode::Cut,
            ElementCounts {
                faces,
                edges,
                vertices,
                solids: 1,
            },
        )
    }

    /// Faces of areas 1 and 2, edges whose lengths are their numbers, and a vertex at
    /// (1, 2, 3): so each name can be seen to be on its own element.
    fn description(faces: usize, edges: usize) -> Description {
        Description {
            faces: (1..=faces)
                .map(|i| FaceDescription {
                    surface: Surface::Other,
                    area: i as f64,
                    centroid: [0.0; 3],
                })
                .collect(),
            edges: (1..=edges)
                .map(|i| EdgeDescription {
                    curve: EdgeCurve::Line,
                    length: i as f64,
                    start: [0.0; 3],
                    end: [i as f64, 0.0, 0.0],
                    middle: [i as f64 / 2.0, 0.0, 0.0],
                })
                .collect(),
            vertices: vec![[1.0, 2.0, 3.0]],
        }
    }

    #[test]
    fn faces_edges_and_vertices_are_listed_in_turn_each_by_name_byte_by_byte() {
        let listed = elements(&map(2, 11, 1), description(2, 11)).unwrap();
        let names: Vec<String> = listed.iter().map(|e| e.name.to_string()).collect();
        let want: Vec<String> = [
            "Face1", "Face2", "Edge1", "Edge10", "Edge11", "Edge2", "Edge3", "Edge4", "Edge5",
            "Edge6", "Edge7", "Edge8", "Edge9", "Vertex1",
        ]
        .iter()
        .map(|e| format!("k7f2qx3m.cut.{e}"))
        .collect();
        assert_eq!(names, want);
        // Each name on its own element: Edge10 is OCCT's tenth edge, 10 long.
        let edge10 = &listed[3];
        assert_eq!((edge10.kind(), edge10.number), (ElementKind::Edge, 10));
        match &edge10.geometry {
            Geometry::Edge(e) => assert_eq!(e.length, 10.0),
            other => panic!("{other:?}"),
        }
        match &listed[1].geometry {
            Geometry::Face(f) => assert_eq!(f.area, 2.0),
            other => panic!("{other:?}"),
        }
        assert_eq!(listed[13].geometry, Geometry::Vertex([1.0, 2.0, 3.0]));
    }

    #[test]
    fn a_description_of_another_solid_is_a_fault_naming_the_feature_and_the_counts() {
        for (map, description, cause) in [
            (
                map(2, 11, 1),
                description(3, 11),
                "faces: the kernel describes 3, and its element map names 2",
            ),
            (
                map(2, 11, 1),
                description(2, 10),
                "edges: the kernel describes 10, and its element map names 11",
            ),
            (
                map(2, 11, 2),
                description(2, 11),
                "vertices: the kernel describes 1, and its element map names 2",
            ),
        ] {
            let e = elements(&map, description).unwrap_err();
            assert_eq!(e.code(), ErrorCode::Internal);
            assert_eq!(e.feature(), Some("k7f2qx3m"));
            assert_eq!(e.cause(), Some(cause));
        }
    }
}
