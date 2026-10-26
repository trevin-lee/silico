//! M4.5's measures (docs/design/m4.1-command-api.md, section 7; docs/ports/measure.md):
//! `mass_properties`, a solid's volume, area, and centre of mass as M1's `measure` gives
//! them (OCCT's integration, asked for a relative 1e-9: docs/ports/measure.md lists the
//! shapes it's known to miss that on), read from its rebuild's summary, so it asks the
//! kernel nothing: volume and area as the feature's rebuilt fingerprint holds them, as
//! `format` writes it, to a millionth of a mm³ and mm² (a stored fingerprint that differs
//! is `check`'s drift); the centre, which no fingerprint holds, rounded to the nanometre,
//! as a fingerprint's box is. `distance` between two elements by name, the least as OCCT
//! measures it, with their centres' and normal distances; and `angle` between two, from
//! their geometry.

use super::elements::summarized;
use super::lookup::{Kinds, Lookup};
use super::{ModelChoice, Problems, Running, SessionCommand, SessionId, Sessions};
use crate::document::{Angle, FeatureId, Fingerprint, Length, Measure};
use crate::kernel::{
    Clients, EdgeCurve, EdgeDescription, ElementKind, ElementRef, FaceDescription, Kernel,
    KernelRequest, ShapeData, Surface,
};
use crate::naming::ElementMap;
use crate::pool::Outcome;
use crate::recompute::Model;
use crate::{Error, ErrorCode};

impl<P: Clients + 'static> Sessions<P> {
    /// `mass_properties` (M4.5): the mass properties of the feature `feature`'s solid, or of
    /// every body, of the model `model` names (decision 33) ([`mass_properties`]).
    pub fn mass_properties(
        &self,
        session: &SessionId,
        model: ModelChoice,
        feature: Option<FeatureId>,
    ) -> Running<Bodies> {
        self.read_model(
            session,
            SessionCommand::MassProperties,
            model,
            move |read| mass_properties(read.model(), feature.as_ref()),
        )
    }

    /// `distance` (M4.5): the least distance between the two elements `elements` names, of
    /// the model `model` names, measured on the read's own client of the kernel
    /// ([`distance`]).
    pub fn distance(
        &self,
        session: &SessionId,
        model: ModelChoice,
        elements: [String; 2],
        feature: Option<FeatureId>,
    ) -> Running<DistanceAnswer> {
        self.read_model(session, SessionCommand::Distance, model, move |read| {
            distance(read.model(), read.kernel(), &elements, feature.as_ref())
        })
    }

    /// `angle` (M4.5): the angle between the two elements `elements` names, of the model
    /// `model` names, from their geometry, asking the kernel nothing ([`angle`]).
    pub fn angle(
        &self,
        session: &SessionId,
        model: ModelChoice,
        elements: [String; 2],
        feature: Option<FeatureId>,
    ) -> Running<AngleAnswer> {
        self.read_model(session, SessionCommand::Angle, model, move |read| {
            angle(read.model(), &elements, feature.as_ref())
        })
    }
}

/// A solid's mass properties: its volume (mm³) and area (mm²), to 6 decimals, as its
/// fingerprint holds them, and its centre of mass, to the nanometre.
#[derive(Debug, Clone, PartialEq)]
pub struct MassProperties {
    /// The feature whose solid it is.
    pub feature: FeatureId,
    pub volume: Measure,
    pub area: Measure,
    pub center_of_mass: [Length; 3],
}

/// `mass_properties`' answer: each solid's, in timeline order.
#[derive(Debug, Clone, PartialEq)]
pub struct Bodies {
    pub bodies: Vec<MassProperties>,
}

/// `mass_properties` on `model`: the feature `feature`'s solid's, or, for none, each
/// body's (the solids no later feature consumed, in timeline order). Refused as `elements`
/// refuses a solid it can't read (naming `feature` if the request did), for each solid that
/// can't be read, every one said; and for a solid that couldn't be measured, why, as
/// `check` says it: the kernel's refusal of its values, or the worker's failure to measure
/// it (`elements::summarized`).
pub fn mass_properties(model: &Model, feature: Option<&FeatureId>) -> Result<Bodies, Problems> {
    let _span = tracing::debug_span!("mass_properties", ?feature).entered();
    let mut refused = Problems::none();
    let mut bodies = Vec::new();
    let asked: Vec<(&FeatureId, bool)> = match feature {
        Some(id) => vec![(id, true)],
        None => model.bodies().iter().map(|id| (id, false)).collect(),
    };
    for (id, asked) in asked {
        match properties(model, id, asked) {
            Ok(body) => bodies.push(body),
            Err(problems) => refused.absorb(problems),
        }
    }
    refused.into_result()?;
    Ok(Bodies { bodies })
}

/// The feature `id`'s solid's mass properties, or why it has none.
fn properties(model: &Model, id: &FeatureId, asked: bool) -> Result<MassProperties, Problems> {
    let summary = summarized(model, id, asked)?;
    // The fingerprint's own rounding, so the two never disagree.
    let fingerprint = Fingerprint::of(summary);
    let center = summary.center.map(Length::rounded);
    match (fingerprint, center) {
        (Some(fingerprint), [Some(x), Some(y), Some(z)]) => Ok(MassProperties {
            feature: id.clone(),
            volume: fingerprint.volume,
            area: fingerprint.area,
            center_of_mass: [x, y, z],
        }),
        // The core refuses a summary that isn't finite when it's had, and the gate bounds a
        // solid to a kilometre: a finite value past what these hold is the worker's, a
        // summary no admitted solid has (as `check` takes it, never Silico's bug).
        _ => Err(Error::new(
            ErrorCode::ProtocolError,
            format!(
                "the kernel worker measured {id}'s solid past what a measure or a length holds"
            ),
        )
        .with_feature(id.as_str())
        .with_suggestion("start a new kernel worker")
        .into()),
    }
}

/// What `distance` answers (M4.5; Fable's decision, `m4-measure-fable.md`): the least
/// distance between the two elements as bounded, a pair of points where it's met (a witness,
/// not an identity: [`silico_protocol::witness_within`]), the distance between their centres
/// where both have one, and the normal distance where both are straight edges or planar
/// faces at exactly 0 or 180 degrees.
#[derive(Debug, Clone, PartialEq)]
pub struct DistanceAnswer {
    pub distance: Length,
    pub from: [Length; 3],
    pub to: [Length; 3],
    /// Between the elements' centres: a vertex's point, a circle's or arc's centre, or a
    /// cylinder face's axis, as an infinite line. Present iff both have one.
    pub center_distance: Option<Length>,
    /// Where both are straight edges or planar faces and `angle` would read exactly 0 or
    /// 180: two faces or two edges, the second's reference point's (a face's centre of area,
    /// an edge's midpoint) distance from the first's plane or line; an edge and a face, the
    /// edge's midpoint's from the face's plane, whichever comes first.
    pub normal_distance: Option<Length>,
}

/// What `angle` answers: the angle in degrees, and, for two straight edges that share one
/// vertex, the vertex, by its own name, as `elements` lists it (unstable, it may be: its
/// entry gives its aliases).
#[derive(Debug, Clone, PartialEq)]
pub struct AngleAnswer {
    pub angle: Angle,
    pub vertex: Option<String>,
}

/// One of a measure's two elements, found by its name in a solid of the model.
struct Found<'m> {
    /// Its place in the request (`elements.0`), and its name as written.
    at: usize,
    text: &'m str,
    feature: &'m FeatureId,
    shape: &'m ShapeData,
    element: ElementRef,
    geometry: Seen<'m>,
    /// Its solid's vertices' points (to find a vertex two edges share), and element map.
    vertices: &'m [[f64; 3]],
    map: &'m ElementMap,
}

/// An element's geometry as its solid's rebuild described it, unrounded (not
/// `elements::Geometry`, the description an entry is made from: this borrows it).
enum Seen<'m> {
    Face(&'m FaceDescription),
    Edge(&'m EdgeDescription),
    Vertex([f64; 3]),
}

/// The kinds a measure takes by name.
const MEASURED: Kinds = Kinds {
    taken: &[ElementKind::Face, ElementKind::Edge, ElementKind::Vertex],
    why: "a measure is between faces, edges, and vertices",
    suggestion: "name a face, an edge, or a vertex of the solid",
};

/// The two elements `names` name, each looked for by [`Lookup`] with [`MEASURED`], and
/// their geometry.
fn find<'m>(
    model: &'m Model,
    names: &'m [String; 2],
    feature: Option<&FeatureId>,
) -> Result<[Found<'m>; 2], Problems> {
    let mut lookup = Lookup::new(model, feature, MEASURED);
    let mut found = Vec::with_capacity(2);
    for (at, text) in names.iter().enumerate() {
        let Some(hit) = lookup.find(&format!("elements.{at}"), text) else {
            continue;
        };
        let (id, element, description) = (hit.feature, hit.element, hit.description);
        let slot = (element.index as usize).checked_sub(1);
        let geometry = match element.kind {
            ElementKind::Face => slot.and_then(|i| description.faces.get(i)).map(Seen::Face),
            ElementKind::Edge => slot.and_then(|i| description.edges.get(i)).map(Seen::Edge),
            _ => slot
                .and_then(|i| description.vertices.get(i))
                .map(|p| Seen::Vertex(*p)),
        };
        let Some(geometry) = geometry else {
            lookup.refuse(
                Error::new(
                    ErrorCode::Internal,
                    format!("{id}'s element map names {element}, which its description hasn't"),
                )
                .with_suggestion(crate::worker::SILICO_BUG),
            );
            continue;
        };
        found.push(Found {
            at,
            text,
            feature: id,
            shape: &hit.built.shape,
            element,
            geometry,
            vertices: &description.vertices,
            map: hit.map,
        });
    }
    lookup.finish()?;
    let [a, b]: [Found<'m>; 2] = found.try_into().map_err(|_| {
        Problems::from(
            Error::new(
                ErrorCode::Internal,
                "a measure found no two elements, yet refused none",
            )
            .with_suggestion(crate::worker::SILICO_BUG),
        )
    })?;
    Ok([a, b])
}

/// `distance` on `model` (M4.5): the elements `names` names, found as [`find`] finds them,
/// measured by `kernel` ([`KernelRequest::distance`], OCCT's least distance), and their
/// centre and normal distances worked out from their geometry. Everything is rounded once,
/// at the end: lengths to the nanometre.
pub fn distance<K: Kernel>(
    model: &Model,
    kernel: &K,
    names: &[String; 2],
    feature: Option<&FeatureId>,
) -> Result<DistanceAnswer, Problems> {
    let _span = tracing::debug_span!("distance", ?feature).entered();
    let [a, b] = find(model, names, feature)?;
    let other = (a.feature != b.feature).then_some(b.shape);
    let request = KernelRequest::distance(a.shape, a.element, other, b.element)
        .map_err(|e| Problems::from(named(e, &a, &b)))?;
    let measured = match kernel.submit(kernel.generation(), request).wait() {
        Outcome::Done(Ok(measured)) => measured,
        Outcome::Done(Err(error)) => return Err(named(kernel_words(error), &a, &b).into()),
        // A read's own client is advanced only when its session closes (and, with decision
        // 34's cancel, when the read is cancelled, which will say `cancelled`).
        Outcome::Superseded => {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "the session closed before the kernel measured the distance",
            )
            .with_parameter("session")
            .with_suggestion("open the document again, and measure it there")
            .into());
        }
    };
    let length = |value: f64| Length::rounded(value).ok_or_else(|| too_far(&a, &b));
    let point = |p: [f64; 3]| -> Result<[Length; 3], Problems> {
        Ok([length(p[0])?, length(p[1])?, length(p[2])?])
    };
    let center_distance = match (centre(&a.geometry), centre(&b.geometry)) {
        (Some(p), Some(q)) => Some(length(centre_gap(&p, &q))?),
        _ => None,
    };
    let normal_distance = match (carrier(&a.geometry), carrier(&b.geometry)) {
        (Some(first), Some(second)) => {
            let parallel = matches!(
                carriers_angle(&first, &second).map(Angle::ndeg),
                Some(0 | 180_000_000_000)
            );
            if parallel {
                // A line and a plane: the line's point from the plane, whichever is first (a
                // line's reference is its middle, so the second's from the first serves a
                // plane then a line too).
                let apart = match (&first, &second) {
                    (Carrier::Line { middle, .. }, Carrier::Plane { .. }) => {
                        second.distance_to(*middle)
                    }
                    _ => first.distance_to(second.reference()),
                };
                Some(length(apart)?)
            } else {
                None
            }
        }
        _ => None,
    };
    Ok(DistanceAnswer {
        distance: length(measured.distance)?,
        from: point(measured.from)?,
        to: point(measured.to)?,
        center_distance,
        normal_distance,
    })
}

/// `error`, a kernel's about the pair, naming both elements, as the request names them;
/// and the request's `elements` as at fault only if the pair is (OCCT finding no distance,
/// a value refused), not for a worker that crashed or hung.
fn named(error: Error, a: &Found<'_>, b: &Found<'_>) -> Error {
    let theirs = matches!(
        error.code(),
        ErrorCode::InvalidInput | ErrorCode::KernelFailed | ErrorCode::InvalidResult
    );
    let error = error.with_elements([
        crate::references::cut_short(a.text),
        crate::references::cut_short(b.text),
    ]);
    if theirs {
        error.with_parameter("elements")
    } else {
        error
    }
}

/// A kernel's failure to measure the pair, in the design's words where it has them
/// (OCCT finding no distance).
fn kernel_words(error: Error) -> Error {
    if error.code() == ErrorCode::KernelFailed
        && error.cause() == Some("BRepExtrema_DistShapeShape: not done")
    {
        return Error::new(
            ErrorCode::KernelFailed,
            "the kernel couldn't find the least distance between these elements",
        )
        .with_cause("BRepExtrema_DistShapeShape: not done")
        .with_suggestion("report it with the document: the kernel couldn't measure this pair");
    }
    error
}

/// A measure past what a length holds: the gate keeps every solid within a kilometre, so
/// none is, and one would be a bug.
fn too_far(a: &Found<'_>, b: &Found<'_>) -> Problems {
    named(
        Error::new(
            ErrorCode::Internal,
            "a measure of these elements is past what a length holds",
        )
        .with_suggestion(crate::worker::SILICO_BUG),
        a,
        b,
    )
    .into()
}

/// An element's centre, for `center_distance`: a vertex's point, a circle's or arc's
/// centre, or a cylinder face's axis, as an infinite line. None for any other.
enum Centre {
    Point([f64; 3]),
    Line {
        point: [f64; 3],
        direction: [f64; 3],
    },
}

fn centre(geometry: &Seen<'_>) -> Option<Centre> {
    match geometry {
        Seen::Vertex(p) => Some(Centre::Point(*p)),
        Seen::Edge(edge) => match &edge.curve {
            EdgeCurve::Circle { axis, .. } => Some(Centre::Point(axis.origin)),
            _ => None,
        },
        Seen::Face(face) => match &face.surface {
            Surface::Cylinder { axis, .. } => Some(Centre::Line {
                point: axis.origin,
                direction: axis.direction,
            }),
            _ => None,
        },
    }
}

/// The least distance between two centres: points, a point and a line, or two lines (the
/// least between them as infinite lines; parallel lines, the distance of either's point from
/// the other).
fn centre_gap(p: &Centre, q: &Centre) -> f64 {
    match (p, q) {
        (Centre::Point(a), Centre::Point(b)) => norm(sub(*a, *b)),
        (Centre::Point(a), Centre::Line { point, direction })
        | (Centre::Line { point, direction }, Centre::Point(a)) => {
            point_to_line(*a, *point, *direction)
        }
        (
            Centre::Line {
                point: o1,
                direction: d1,
            },
            Centre::Line {
                point: o2,
                direction: d2,
            },
        ) => {
            // Parallel as `angle` and `normal_distance` read lines: 0 between them at the
            // nanodegree, where the skew formula would put lines a bit apart meeting far off.
            if lines_angle(*d1, *d2).is_some_and(|a| a.ndeg() == 0) {
                point_to_line(*o2, *o1, *d1)
            } else {
                let n = cross(*d1, *d2);
                dot(sub(*o2, *o1), n).abs() / norm(n)
            }
        }
    }
}

/// A straight edge's or a planar face's carrier, for `angle` and `normal_distance`: a line
/// through its start in its direction, or a plane through its centre of area with its
/// outward unit normal; with its reference point (an edge's midpoint, a face's centre).
enum Carrier {
    Line {
        start: [f64; 3],
        end: [f64; 3],
        middle: [f64; 3],
    },
    Plane {
        centroid: [f64; 3],
        normal: [f64; 3],
    },
}

impl Carrier {
    fn reference(&self) -> [f64; 3] {
        match self {
            Carrier::Line { middle, .. } => *middle,
            Carrier::Plane { centroid, .. } => *centroid,
        }
    }

    /// The perpendicular distance from `p` to this carrier.
    fn distance_to(&self, p: [f64; 3]) -> f64 {
        match self {
            Carrier::Line { start, end, .. } => point_to_line(p, *start, sub(*end, *start)),
            Carrier::Plane { centroid, normal } => dot(sub(p, *centroid), *normal).abs(),
        }
    }
}

fn carrier(geometry: &Seen<'_>) -> Option<Carrier> {
    match geometry {
        Seen::Edge(edge) if edge.curve.is_straight() => Some(Carrier::Line {
            start: edge.start,
            end: edge.end,
            middle: edge.middle,
        }),
        Seen::Face(face) => match &face.surface {
            Surface::Plane { normal } => Some(Carrier::Plane {
                centroid: face.centroid,
                normal: *normal,
            }),
            _ => None,
        },
        _ => None,
    }
}

/// The angle between two carriers as `angle` reads it, with no shared vertex: two planes,
/// the wedge both outward normals point into, 0 to 180 (Trevin's choice, 2026-10-25: a 30°
/// wedge reads 30); two lines, 0 to 90 between them; a line and a plane, 0 to 90. Rounded
/// to the nanodegree; none past what an angle holds (none is).
fn carriers_angle(a: &Carrier, b: &Carrier) -> Option<Angle> {
    let radians = match (a, b) {
        (Carrier::Plane { normal: n1, .. }, Carrier::Plane { normal: n2, .. }) => {
            libm::atan2(norm(cross(*n1, *n2)), -dot(*n1, *n2))
        }
        (
            Carrier::Line {
                start: s1, end: e1, ..
            },
            Carrier::Line {
                start: s2, end: e2, ..
            },
        ) => return lines_angle(sub(*e1, *s1), sub(*e2, *s2)),
        (Carrier::Line { start, end, .. }, Carrier::Plane { normal, .. })
        | (Carrier::Plane { normal, .. }, Carrier::Line { start, end, .. }) => {
            let d = sub(*end, *start);
            libm::atan2(dot(d, *normal).abs(), norm(cross(d, *normal)))
        }
    };
    Angle::rounded(radians.to_degrees())
}

/// The angle between two lines of directions `d1` and `d2`, 0 to 90, rounded to the
/// nanodegree: the one test of whether two lines are parallel, for `angle`,
/// `normal_distance`, and `center_distance` alike.
fn lines_angle(d1: [f64; 3], d2: [f64; 3]) -> Option<Angle> {
    Angle::rounded(libm::atan2(norm(cross(d1, d2)), dot(d1, d2).abs()).to_degrees())
}

/// `angle` on `model` (M4.5): of the elements `names` names, found as [`find`] finds them:
/// two planar faces, the wedge both outward normals point into, 0 to 180; two straight edges
/// that share one vertex, the angle at it, 0 to 180, the vertex named; any other two
/// straight edges, 0 to 90 between their lines; a straight edge and a planar face, 0 to 90.
/// Anything else is refused, naming the element, saying what it is and what applies to it:
/// never NaN, never 0 in an answer's place. In the core, by `libm`'s `atan2`, never `acos`.
pub fn angle(
    model: &Model,
    names: &[String; 2],
    feature: Option<&FeatureId>,
) -> Result<AngleAnswer, Problems> {
    let _span = tracing::debug_span!("angle", ?feature).entered();
    let [a, b] = find(model, names, feature)?;
    let mut refused = Problems::none();
    let carriers = [&a, &b].map(|found| {
        let carrier = carrier(&found.geometry);
        if carrier.is_none() {
            refused.push(not_angled(found));
        }
        carrier
    });
    refused.into_result()?;
    let [Some(first), Some(second)] = carriers else {
        return Err(
            Error::new(ErrorCode::Internal, "an angle's carriers went missing")
                .with_suggestion(crate::worker::SILICO_BUG)
                .into(),
        );
    };
    let rounded = |angle: Option<Angle>| {
        angle.ok_or_else(|| {
            Problems::from(
                Error::new(ErrorCode::Internal, "an angle past what an angle holds")
                    .with_suggestion(crate::worker::SILICO_BUG),
            )
        })
    };
    if let (
        Carrier::Line {
            start: s1, end: e1, ..
        },
        Carrier::Line {
            start: s2, end: e2, ..
        },
    ) = (&first, &second)
        && a.feature == b.feature
        && let Some((at, u, w)) = shared_vertex([*s1, *e1], [*s2, *e2])
    {
        // The vertex by its own name, as `elements` lists it (an unstable one too: a read's
        // answer is stored nowhere; its entry gives its aliases).
        let vertex = a
            .vertices
            .iter()
            .position(|v| same(*v, at))
            .and_then(|i| u32::try_from(i + 1).ok())
            .and_then(|index| {
                a.map.name(ElementRef {
                    kind: ElementKind::Vertex,
                    index,
                })
            })
            .map(|name| name.to_string());
        // A valid solid's edges sharing an end share its named vertex: one not found is a
        // bug, said, never a quiet change to the angle between the lines.
        let Some(vertex) = vertex else {
            return Err(Error::new(
                ErrorCode::Internal,
                "two edges share an end that's none of their solid's named vertices",
            )
            .with_elements([
                crate::references::cut_short(a.text),
                crate::references::cut_short(b.text),
            ])
            .with_suggestion(crate::worker::SILICO_BUG)
            .into());
        };
        let degrees = libm::atan2(norm(cross(u, w)), dot(u, w)).to_degrees();
        return Ok(AngleAnswer {
            angle: rounded(Angle::rounded(degrees))?,
            vertex: Some(vertex),
        });
    }
    Ok(AngleAnswer {
        angle: rounded(carriers_angle(&first, &second))?,
        vertex: None,
    })
}

/// The one end two straight edges share, if they share exactly one (the same point, to the
/// bit: a valid solid's edges meeting at a point share its vertex), and each edge's
/// direction from it to its other end.
fn shared_vertex(a: [[f64; 3]; 2], b: [[f64; 3]; 2]) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    let mut shared = Vec::new();
    for (i, p) in a.iter().enumerate() {
        for (j, q) in b.iter().enumerate() {
            if same(*p, *q) {
                shared.push((*p, a[1 - i], b[1 - j]));
            }
        }
    }
    match shared.as_slice() {
        [(at, other_a, other_b)] => Some((*at, sub(*other_a, *at), sub(*other_b, *at))),
        _ => None,
    }
}

/// Whether two points are one, to the bit (-0 as 0).
fn same(p: [f64; 3], q: [f64; 3]) -> bool {
    p.iter().zip(q).all(|(a, b)| *a == b)
}

/// The refusal of `found` as an angle's element: not a straight edge or a planar face.
fn not_angled(found: &Found<'_>) -> Error {
    let at = format!("elements.{}", found.at);
    let (what, suggestion) = match &found.geometry {
        Seen::Vertex(_) => (
            "a vertex".to_owned(),
            "a vertex has a position (its entry in elements) and a distance to other \
             elements; an angle needs a straight edge or a planar face"
                .to_owned(),
        ),
        Seen::Edge(edge) => match &edge.curve {
            EdgeCurve::Degenerated => (
                "a degenerated edge, a pole or an apex, with no curve".to_owned(),
                "a degenerated edge is a point of no length (its entry in elements) with a \
                 distance to other elements; an angle needs a straight edge"
                    .to_owned(),
            ),
            EdgeCurve::Circle { .. } => (
                "an edge on a circle".to_owned(),
                "a circle has a radius and a centre (its entry in elements) and a distance to \
                 other elements; an angle needs a straight edge"
                    .to_owned(),
            ),
            curve => {
                let on = article(match crate::elements::curve_type(curve) {
                    crate::elements::ElementType::Other => "curve",
                    t => t.word(),
                });
                (
                    format!("an edge on {on}"),
                    format!(
                        "an edge on {on} has a length and ends (its entry in elements) and a \
                         distance to other elements; an angle needs a straight edge"
                    ),
                )
            }
        },
        Seen::Face(face) => {
            let on = article(match crate::elements::surface_type(&face.surface) {
                crate::elements::ElementType::Other => "surface",
                t => t.word(),
            });
            (
                format!("a face on {on}, not a plane"),
                format!(
                    "a face on {on} has an area and a centre (its entry in elements) and a \
                     distance to other elements; an angle needs a planar face"
                ),
            )
        }
    };
    Error::new(
        ErrorCode::InvalidInput,
        "an angle takes two straight edges, two planar faces, or one of each",
    )
    .with_parameter("elements")
    .with_element(crate::references::cut_short(found.text))
    .with_cause(format!("{at} is {what}"))
    .with_suggestion(suggestion)
}

/// `word` with its article: `a circle`, `an ellipse`.
fn article(word: &str) -> String {
    match word.chars().next() {
        Some('a' | 'e' | 'i' | 'o' | 'u') => format!("an {word}"),
        _ => format!("a {word}"),
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// The perpendicular distance from `p` to the line through `point` along `direction`.
fn point_to_line(p: [f64; 3], point: [f64; 3], direction: [f64; 3]) -> f64 {
    norm(cross(sub(p, point), direction)) / norm(direction)
}

#[cfg(test)]
mod tests {
    use super::{Centre, article, centre_gap, cross, kernel_words};
    use crate::{Error, ErrorCode};

    #[test]
    fn a_cross_product_is_each_component_by_hand() {
        // The mutation run: every angle tested lay in one plane, so two of the three
        // components' signs went untested. (1, 2, 3) × (4, 5, 6) = (2·6 − 3·5, 3·4 − 1·6,
        // 1·5 − 2·4) = (−3, 6, −3).
        assert_eq!(cross([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), [-3.0, 6.0, -3.0]);
    }

    #[test]
    fn skew_axes_are_as_far_apart_as_their_common_perpendicular() {
        // The mutation run: the axes tested met at right angles, so the cross product's
        // length was 1 and dividing by it untested. The x axis, and an axis 5 above it at 45°
        // in a plane parallel to xy: their common perpendicular is along z, 5 long.
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let x = Centre::Line {
            point: [0.0, 0.0, 0.0],
            direction: [1.0, 0.0, 0.0],
        };
        let skew = Centre::Line {
            point: [3.0, -2.0, 5.0],
            direction: [h, h, 0.0],
        };
        let gap = centre_gap(&x, &skew);
        assert!((gap - 5.0).abs() <= 1e-12, "{gap}");
    }

    #[test]
    fn only_the_kernels_not_done_is_reworded() {
        // The mutation run: either half of the test alone was untested. Kernel_failed with
        // another cause, and another code with the not-done cause, pass as they are.
        const NOT_DONE: &str = "BRepExtrema_DistShapeShape: not done";
        let reworded = kernel_words(Error::new(ErrorCode::KernelFailed, "x").with_cause(NOT_DONE));
        assert_eq!(
            reworded.message(),
            "the kernel couldn't find the least distance between these elements"
        );
        for kept in [
            Error::new(ErrorCode::KernelFailed, "x").with_cause("another"),
            Error::new(ErrorCode::InvalidResult, "x").with_cause(NOT_DONE),
        ] {
            assert_eq!(kernel_words(kept.clone()), kept);
        }
    }

    #[test]
    fn a_word_takes_its_article() {
        assert_eq!(article("circle"), "a circle");
        assert_eq!(article("ellipse"), "an ellipse");
        assert_eq!(article("offset"), "an offset");
        assert_eq!(article("bspline"), "a bspline");
    }
}
