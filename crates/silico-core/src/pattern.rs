//! A linear pattern's, polar pattern's, and mirror's rebuild (docs/design/m6.0-patterns.md,
//! sections 2.4, 4.2, and 4.3): the instances it places, its references resolved from what
//! the kernel describes of them, each instance's placement worked out from its index and the
//! document's integers, never accumulated, and what each step's history says of its copies.
//! The recompute plans and runs the steps (`recompute`); the kernel places the copies and
//! runs the one boolean, deciding nothing (ARCHITECTURE.md, invariant 2).

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;

use silico_protocol::{
    ANGULAR_TOLERANCE, Axis, CopiesOp, EdgeCurve, EdgeDescription, FaceDescription, History,
    MAX_LENGTH, Placement, SketchPlane, Surface, vec3,
};

use crate::document::json::shown;
use crate::document::{
    ElementName, FeatureId, Length, LinearDistribution, Made, NameField, Operation, OriginAxis,
    OriginOrName, PROBLEMS_LISTED, PolarDistribution, skipped_in_range,
};
use crate::error::{article, listed_first};
use crate::kernel::ElementKind;
use crate::naming::Instance;
use crate::references::cut_short;
use crate::{Error, ErrorCode};

/// Nanometres in a millimetre.
const NM_PER_MM: f64 = 1e6;

/// Nanodegrees in a degree.
const NDEG_PER_DEGREE: f64 = 1e9;

/// The pattern's type word (`linear_pattern`, `polar_pattern`, `mirror`), as its type's
/// own (`PatternFeature::word`); empty for a feature that isn't a pattern, which nothing
/// here is given.
fn word(operation: &Operation) -> &'static str {
    match operation.made() {
        Made::Pattern(kind) => kind.word(),
        _ => "",
    }
}

/// The counts a pattern's instances are indexed by, one a direction: a linear pattern's
/// directions', a polar pattern's one, a mirror's 2 (it's a pattern of two instances, N10).
pub(crate) fn counts(operation: &Operation) -> Vec<u32> {
    match operation {
        Operation::LinearPattern { directions, .. } => {
            directions.iter().map(|direction| direction.count).collect()
        }
        Operation::PolarPattern { count, .. } => vec![*count],
        Operation::Mirror { .. } => vec![2],
        _ => Vec::new(),
    }
}

/// The instances a pattern places: every one but the original and those `skip` leaves out
/// (those in range: a skip past a count takes nothing away, `skipped_in_range`), in `skip`'s
/// order, direction 1's offset first, each in its one spelling (N10). None for the identity:
/// a count of 1 in every direction, or every copy skipped (section 2.4).
pub(crate) fn placed(operation: &Operation) -> Vec<Instance> {
    let skipped = skipped(operation);
    instances(operation)
        .into_iter()
        .filter(|instance| !skipped.contains(instance.offsets()))
        .collect()
}

/// The instances `skip` leaves out that the pattern's counts reach (`skipped_in_range`), by
/// their offsets.
pub(crate) fn skipped(operation: &Operation) -> BTreeSet<&[u32]> {
    skipped_in_range(operation.skip(), &counts(operation))
        .into_iter()
        .map(|instance| instance.offsets())
        .collect()
}

/// Every instance of a pattern but the original, skipped or not, in `skip`'s order,
/// direction 1's offset first, each in its one spelling (N10).
pub(crate) fn instances(operation: &Operation) -> Vec<Instance> {
    let counts = counts(operation);
    // Every instance by its offsets, a direction at a time, direction 1 outer: (i) and
    // (i, 0) are one instance, written `i`, before (i, 1), so the order is the one `skip`
    // is written in. Built for as many directions as the pattern has (the gate allows two).
    let mut all: Vec<Vec<u32>> = vec![Vec::new()];
    for &count in &counts {
        all = all
            .into_iter()
            .flat_map(|before| {
                (0..count).map(move |offset| {
                    let mut offsets = before.clone();
                    offsets.push(offset);
                    offsets
                })
            })
            .collect();
    }
    all.iter()
        .filter_map(|offsets| Instance::new(offsets))
        .collect()
}

/// A pattern's values as its step's key holds them (section 2.4): `reversed`, each
/// distribution's kind and value, in nanometres or nanodegrees, each count, and `skip` as
/// written; the references aren't here, but in the step's digest of them.
pub(crate) fn values(operation: &Operation) -> String {
    use std::fmt::Write as _;
    let mut line = String::new();
    match operation {
        Operation::LinearPattern { directions, .. } => {
            for direction in directions {
                let (kind, length) = match direction.distribution {
                    LinearDistribution::Extent { length } => ("extent", length),
                    LinearDistribution::Spacing { length } => ("spacing", length),
                };
                let _ = write!(
                    line,
                    "direction {} {kind} {} {} ",
                    direction.reversed,
                    length.nm(),
                    direction.count
                );
            }
        }
        Operation::PolarPattern {
            reversed,
            distribution,
            count,
            ..
        } => {
            let (kind, angle) = match distribution {
                PolarDistribution::Extent { angle } => ("extent", angle),
                PolarDistribution::Spacing { angle } => ("spacing", angle),
            };
            let _ = write!(line, "turn {reversed} {kind} {} {count} ", angle.ndeg());
        }
        _ => line.push_str("mirror "),
    }
    line.push_str("skip");
    for instance in operation.skip() {
        line.push_str(" [");
        for (i, offset) in instance.offsets().iter().enumerate() {
            if i > 0 {
                line.push(',');
            }
            let _ = write!(line, "{offset}");
        }
        line.push(']');
    }
    line
}

/// A pattern's references, each its field's path and its text as written, a word or a name
/// alike: a linear pattern's `directions.N.along`, a polar pattern's `axis`, a mirror's
/// `plane`, each path as [`NameField`] writes it, where a name stored there is read
/// (`Operation::stored_names`).
pub(crate) fn references(operation: &Operation) -> Vec<(String, &str)> {
    match operation {
        Operation::LinearPattern { directions, .. } => directions
            .iter()
            .enumerate()
            .map(|(i, direction)| (NameField::Along(i).to_string(), direction.along.as_str()))
            .collect(),
        Operation::PolarPattern { axis, .. } => vec![(NameField::Axis.to_string(), axis.as_str())],
        Operation::Mirror { plane, .. } => vec![(NameField::Plane.to_string(), plane.as_str())],
        _ => Vec::new(),
    }
}

/// What the kernel describes of a referenced element (`describe`, M4.4).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Geometry {
    Face(FaceDescription),
    Edge(EdgeDescription),
}

/// A pattern's references resolved in 3D, `reversed` taken: a linear pattern's unit
/// direction for each of its directions, a polar pattern's axis (a point and a unit
/// direction, which it turns about right-handed), or a mirror's plane (a point and a unit
/// normal).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Resolved {
    Linear(Vec<[f64; 3]>),
    Polar(Axis),
    Mirror { origin: [f64; 3], normal: [f64; 3] },
}

/// An origin axis's unit direction.
fn unit(axis: OriginAxis) -> [f64; 3] {
    match axis {
        OriginAxis::X => [1.0, 0.0, 0.0],
        OriginAxis::Y => [0.0, 1.0, 0.0],
        OriginAxis::Z => [0.0, 0.0, 1.0],
    }
}

fn negated(v: [f64; 3]) -> [f64; 3] {
    v.map(|c| -c + 0.0)
}

/// A reference refused for what it is: `invalid_input`, naming the feature, the field, and
/// the element (decision 3, section 4.2).
fn wrong(id: &FeatureId, field: &str, name: &ElementName, is: &str, suggestion: &str) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("{field}, {}, {is}", shown(name.as_str())),
    )
    .with_feature(id.as_str())
    .with_parameter(field)
    .with_element(cut_short(name.as_str()))
    .with_suggestion(suggestion)
}

/// What an element is, in a refusal's words: `a circle`, `a cylinder face`.
fn what(geometry: &Geometry) -> String {
    match geometry {
        Geometry::Edge(EdgeDescription {
            curve: EdgeCurve::Other,
            ..
        }) => "an edge of a curve OCCT doesn't type".to_owned(),
        Geometry::Face(FaceDescription {
            surface: Surface::Other,
            ..
        }) => "a face of a surface OCCT doesn't type".to_owned(),
        Geometry::Edge(edge) => article(crate::elements::curve_type(&edge.curve).word()),
        Geometry::Face(face) => format!(
            "{} face",
            article(crate::elements::surface_type(&face.surface).word())
        ),
    }
}

/// A linear pattern's direction from a named element: a straight edge from its start to its
/// end, or a planar face along its outward normal, exactly as `describe` gives them
/// (decision 3), as a unit vector.
fn direction_of(
    id: &FeatureId,
    field: &str,
    name: &ElementName,
    geometry: &Geometry,
) -> Result<[f64; 3], Error> {
    let base = "name a straight edge or a planar face, or give x, y, or z";
    match geometry {
        Geometry::Edge(EdgeDescription {
            curve: EdgeCurve::Line,
            start,
            end,
            ..
        }) => Ok(vec3::normalize(vec3::sub(*end, *start))),
        Geometry::Face(FaceDescription {
            surface: Surface::Plane { normal, .. },
            orientation,
            ..
        }) => {
            if orientation.has_outside() {
                Ok(vec3::normalize(*normal))
            } else {
                Err(wrong(
                    id,
                    field,
                    name,
                    "is a face with no outside: the solid holds it within itself, so it points no way",
                    "name a face of the solid's boundary, or an edge",
                ))
            }
        }
        other => {
            let circle = matches!(
                other,
                Geometry::Edge(EdgeDescription {
                    curve: EdgeCurve::Circle { .. },
                    ..
                })
            );
            let suggestion = if circle {
                format!("{base}; a circle is a polar pattern's axis")
            } else {
                base.to_owned()
            };
            Err(wrong(
                id,
                field,
                name,
                &format!(
                    "is {}: a direction is a straight edge or a planar face",
                    what(other)
                ),
                &suggestion,
            ))
        }
    }
}

/// A polar pattern's axis from a named element: a straight edge, from its start to its end,
/// or a circular edge, about its circle's axis through its centre (decision 3).
fn axis_of(id: &FeatureId, name: &ElementName, geometry: &Geometry) -> Result<Axis, Error> {
    match geometry {
        Geometry::Edge(EdgeDescription {
            curve: EdgeCurve::Line,
            start,
            end,
            ..
        }) => Ok(Axis {
            origin: *start,
            direction: vec3::normalize(vec3::sub(*end, *start)),
        }),
        Geometry::Edge(EdgeDescription {
            curve: EdgeCurve::Circle { axis, .. },
            ..
        }) => Ok(Axis {
            origin: axis.origin,
            direction: vec3::normalize(axis.direction),
        }),
        other => Err(wrong(
            id,
            "axis",
            name,
            &format!("is {}: an axis is a straight or circular edge", what(other)),
            "name a straight or circular edge, or give x, y, or z",
        )),
    }
}

/// A mirror's plane from a named planar face: its normal, and its origin the global origin
/// projected onto it, o = n(n·p) (docs/ports/attacher.md, F2), p the plane's own location.
fn plane_of(
    id: &FeatureId,
    name: &ElementName,
    geometry: &Geometry,
) -> Result<([f64; 3], [f64; 3]), Error> {
    match geometry {
        Geometry::Face(FaceDescription {
            surface: Surface::Plane { normal, location },
            ..
        }) => {
            let n = vec3::normalize(*normal);
            let along = vec3::dot(n, *location);
            Ok((n.map(|c| c * along + 0.0), n))
        }
        other => Err(wrong(
            id,
            "plane",
            name,
            &format!("is {}: a mirror plane is a planar face", what(other)),
            "name a planar face, or give xy, xz, or yz",
        )),
    }
}

/// How a reference is said in a message: an origin word bare, a name quoted and cut short.
fn said<O: Copy>(reference: &OriginOrName<O>, text: &str) -> String {
    match reference {
        OriginOrName::Origin(_) => text.to_owned(),
        OriginOrName::Name(_) => shown(text),
    }
}

/// Whether two unit directions are parallel, either way, within OCCT's angular tolerance, as
/// `gp_Dir::IsParallel` takes them (decision 11): the angle between them, by its sine and
/// cosine, within 1e-12 radians of none or of a half turn.
pub(crate) fn parallel(a: [f64; 3], b: [f64; 3]) -> bool {
    let angle = libm::atan2(vec3::norm(vec3::cross(a, b)), vec3::dot(a, b));
    angle <= ANGULAR_TOLERANCE || PI - angle <= ANGULAR_TOLERANCE
}

/// `operation`'s references resolved, feature `id`'s, each named element's geometry as
/// `geometry` describes it (found in the base's map, which the caller has done: a name that
/// finds nothing is `reference_broken` before this). An origin word is exact; a name is read
/// from its element's description and checked for its kind (decision 3); `reversed` negates
/// a linear direction or a polar axis; two parallel directions are refused (decision 11).
pub(crate) fn resolve(
    id: &FeatureId,
    operation: &Operation,
    geometry: &mut dyn FnMut(&ElementName) -> Result<Geometry, Error>,
) -> Result<Resolved, Error> {
    match operation {
        Operation::LinearPattern { directions, .. } => {
            let mut resolved = Vec::with_capacity(directions.len());
            for (i, direction) in directions.iter().enumerate() {
                let field = format!("directions.{i}.along");
                let along = match &direction.along {
                    OriginOrName::Origin(axis) => unit(*axis),
                    OriginOrName::Name(name) => direction_of(id, &field, name, &geometry(name)?)?,
                };
                resolved.push(if direction.reversed {
                    negated(along)
                } else {
                    along
                });
            }
            if let ([first, second], [d0, d1]) = (directions.as_slice(), resolved.as_slice())
                && parallel(*d0, *d1)
            {
                let mut error = Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "linear_pattern {id}'s directions are parallel: directions.1.along, {}, runs along directions.0.along, {}, so copies would land on copies",
                        said(&second.along, second.along.as_str()),
                        said(&first.along, first.along.as_str())
                    ),
                )
                .with_feature(id.as_str())
                .with_parameter("directions.1.along")
                .with_cause("the two directions are within 1e-12 radians of each other, either way (OCCT's Precision::Angular)")
                .with_suggestion("give directions.1 another way, or drop it and raise directions.0.count");
                if let Some(name) = second.along.name() {
                    error = error.with_element(cut_short(name.as_str()));
                }
                return Err(error);
            }
            Ok(Resolved::Linear(resolved))
        }
        Operation::PolarPattern { axis, reversed, .. } => {
            let mut axis = match axis {
                OriginOrName::Origin(word) => Axis {
                    origin: [0.0; 3],
                    direction: unit(*word),
                },
                OriginOrName::Name(name) => axis_of(id, name, &geometry(name)?)?,
            };
            if *reversed {
                axis.direction = negated(axis.direction);
            }
            Ok(Resolved::Polar(axis))
        }
        Operation::Mirror { plane, .. } => {
            let (origin, normal) = match plane {
                OriginOrName::Origin(plane) => ([0.0; 3], SketchPlane::from(*plane).normal()),
                OriginOrName::Name(name) => plane_of(id, name, &geometry(name)?)?,
            };
            Ok(Resolved::Mirror { origin, normal })
        }
        _ => Err(Error::new(
            ErrorCode::Internal,
            "a feature that isn't a pattern was resolved as one",
        )
        .with_feature(id.as_str())
        .with_suggestion(crate::worker::SILICO_BUG)),
    }
}

/// How far a linear direction moves instance offset `i`, in millimetres: `i · L / (count −
/// 1)` for an extent, `i · L` taken exactly in nanometres and divided once, or `i · S` for a
/// spacing (section 2.4).
pub(crate) fn linear_step(distribution: LinearDistribution, count: u32, i: u32) -> f64 {
    let (length, divisor) = match distribution {
        LinearDistribution::Extent { length } => (length, i128::from(count.saturating_sub(1))),
        LinearDistribution::Spacing { length } => (length, 1),
    };
    if divisor == 0 {
        return 0.0;
    }
    // Exact: at most 999 × 1e12 nm, below 2^53; and the divisor at most 999 × 1e6.
    let along = i128::from(i) * i128::from(length.nm());
    along as f64 / (divisor as f64 * NM_PER_MM)
}

/// How far a polar pattern turns instance `k`, in degrees: `k · A / count` for an extent of
/// exactly 360°, `k · A / (count − 1)` for any other extent, or `k · A` for a spacing, `k · A`
/// exact in nanodegrees and divided once (section 2.4, decision 5).
pub(crate) fn polar_degrees(distribution: PolarDistribution, count: u32, k: u32) -> f64 {
    let (angle, divisor) = match distribution {
        PolarDistribution::Extent { angle } if angle.ndeg() == crate::document::TURN_NDEG => {
            (angle, i128::from(count))
        }
        PolarDistribution::Extent { angle } => (angle, i128::from(count.saturating_sub(1))),
        PolarDistribution::Spacing { angle } => (angle, 1),
    };
    if divisor == 0 {
        return 0.0;
    }
    let turned = i128::from(k) * i128::from(angle.ndeg());
    turned as f64 / (divisor as f64 * NDEG_PER_DEGREE)
}

/// Each instance's placement, `instances` in order, of feature `id`'s `operation` with its
/// references `resolved`: worked out from its index and the document's integers, never from
/// another instance's (section 2.4), and checked by the kernel's own rules for a placement
/// (`check_copies`) before any is sent, each failure worded for its instance (section 4.2).
pub(crate) fn placements(
    id: &FeatureId,
    operation: &Operation,
    resolved: &Resolved,
    instances: &[Instance],
) -> Result<Vec<Placement>, Error> {
    let kind = word(operation);
    instances
        .iter()
        .map(|instance| {
            let placement = placement(operation, resolved, instance).ok_or_else(|| {
                Error::new(
                    ErrorCode::Internal,
                    "a pattern's references were resolved as another type's",
                )
                .with_feature(id.as_str())
                .with_suggestion(crate::worker::SILICO_BUG)
            })?;
            placement
                .check()
                .map(|()| placement)
                .map_err(|invalid| unplaceable(id, kind, instance, &placement, &invalid.message))
        })
        .collect()
}

/// Instance `instance`'s placement, of `operation` with its references `resolved`: a linear
/// instance's move, i · s₁ · d₁ + j · s₂ · d₂; a polar instance's turn about the axis; a
/// mirror's reflection in its plane (section 2.4). Unchecked: [`placements`] checks each by
/// the kernel's rules. None where `resolved` is another type's than `operation`.
pub(crate) fn placement(
    operation: &Operation,
    resolved: &Resolved,
    instance: &Instance,
) -> Option<Placement> {
    let offsets = instance.offsets();
    let offset = |d: usize| offsets.get(d).copied().unwrap_or(0);
    match (operation, resolved) {
        (Operation::LinearPattern { directions, .. }, Resolved::Linear(along)) => {
            let mut moved = [0.0; 3];
            for (d, (direction, unit)) in directions.iter().zip(along).enumerate() {
                let step = linear_step(direction.distribution, direction.count, offset(d));
                for (m, u) in moved.iter_mut().zip(unit) {
                    *m += u * step;
                }
            }
            Some(Placement::Translation {
                offset: moved.map(|c| c + 0.0),
            })
        }
        (
            Operation::PolarPattern {
                distribution,
                count,
                ..
            },
            Resolved::Polar(axis),
        ) => Some(Placement::Rotation {
            axis: *axis,
            angle: silico_protocol::radians(polar_degrees(*distribution, *count, offset(0))),
        }),
        (Operation::Mirror { .. }, Resolved::Mirror { origin, normal }) => {
            Some(Placement::Mirror {
                origin: *origin,
                normal: *normal,
            })
        }
        _ => None,
    }
}

/// A placement the kernel's rules refuse, worded for its instance (section 4.2): one moved
/// past a kilometre along an axis, which two directions together can do though the gate
/// passed each, or else the rule's own words.
fn unplaceable(
    id: &FeatureId,
    kind: &str,
    instance: &Instance,
    placement: &Placement,
    why: &str,
) -> Error {
    let parameter = match placement {
        Placement::Translation { .. } | Placement::TurnedAndMoved { .. } => "directions",
        Placement::Rotation { .. } => "axis",
        Placement::Mirror { .. } => "plane",
    };
    let past = match placement {
        Placement::Translation { offset } | Placement::TurnedAndMoved { offset, .. } => offset
            .iter()
            .zip(["x", "y", "z"])
            .find(|(c, _)| c.abs() > MAX_LENGTH),
        _ => None,
    };
    let error = match past {
        Some((c, axis)) => {
            let along = if *c < 0.0 {
                format!("-{axis}")
            } else {
                axis.to_owned()
            };
            let distance = Length::rounded(c.abs())
                .map_or_else(|| format!("{}", c.abs()), |length| length.to_string());
            Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "{kind} {id} would move instance {instance} {distance} mm along {along}, past the {} mm (a kilometre) a copy may move",
                    silico_protocol::format_number(MAX_LENGTH)
                ),
            )
            .with_suggestion(
                "lower a count or a spacing, so every instance stays within a kilometre of the original",
            )
        }
        None => Error::new(
            ErrorCode::InvalidInput,
            format!("{kind} {id} can't place instance {instance}: {why}"),
        )
        .with_suggestion("change the directions, the axis, or the plane, or the distribution"),
    };
    error
        .with_feature(id.as_str())
        .with_parameter(parameter)
        .with_element(instance.to_string())
}

/// What one step's history says of its copies (section 4.2, *Every verdict kept*; 4.3):
/// which instances' copies lie off the base, and which changed nothing; and how many of the
/// result's solids the base's faces lie in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepVerdict {
    /// The original the step copies, none for a whole solid's copies.
    pub original: Option<FeatureId>,
    /// Whether the step fuses its copies or cuts them.
    pub op: CopiesOp,
    /// How many solids the result has.
    pub solids: u32,
    /// How many of them the base's faces lie in.
    pub base_solids: u32,
    /// The instances any of whose copy's faces lies in a solid other than the one holding
    /// the base's faces, in `skip`'s order: none where the base's faces lie in more than one.
    pub off_base: Vec<Instance>,
    /// The instances none of whose copy's faces is in the result, by the history's fates:
    /// none kept, modified, or generating (decision 6), in `skip`'s order.
    pub changed_nothing: Vec<Instance>,
}

/// The faces of the result each of input `input`'s faces is, kept or modified into.
fn result_faces(history: &History, input: usize) -> BTreeSet<u32> {
    history
        .inputs
        .get(input)
        .map_or_else(BTreeSet::new, |input| {
            input
                .faces
                .iter()
                .flat_map(|fate| {
                    fate.kept.into_iter().chain(
                        fate.modified
                            .iter()
                            .filter(|r| r.element.kind == ElementKind::Face)
                            .map(|r| r.element.index),
                    )
                })
                .collect()
        })
}

/// What step `history` says of its copies, the copy at input k being `instances[k − 1]`.
pub(crate) fn verdict(
    history: &History,
    instances: &[Instance],
    original: Option<FeatureId>,
    op: CopiesOp,
) -> StepVerdict {
    let mut solid_of: BTreeMap<u32, usize> = BTreeMap::new();
    for (s, faces) in history.topology.solid_faces.iter().enumerate() {
        for &face in faces {
            solid_of.entry(face).or_insert(s);
        }
    }
    let solids_of = |faces: &BTreeSet<u32>| -> BTreeSet<usize> {
        faces
            .iter()
            .filter_map(|face| solid_of.get(face).copied())
            .collect()
    };
    let base = solids_of(&result_faces(history, 0));
    let mut off_base = Vec::new();
    let mut changed_nothing = Vec::new();
    for (k, instance) in instances.iter().enumerate() {
        let input = k + 1;
        let faces = result_faces(history, input);
        let generating = history
            .inputs
            .get(input)
            .is_some_and(|input| input.faces.iter().any(|fate| !fate.generated.is_empty()));
        if faces.is_empty() && !generating {
            changed_nothing.push(instance.clone());
        }
        if let [held] = base.iter().copied().collect::<Vec<_>>().as_slice()
            && solids_of(&faces).iter().any(|s| s != held)
        {
            off_base.push(instance.clone());
        }
    }
    StepVerdict {
        original,
        op,
        solids: history.result.solids,
        base_solids: u32::try_from(base.len()).unwrap_or(u32::MAX),
        off_base,
        changed_nothing,
    }
}

/// The most instances a message names, the rest counted (`…, and 989 more`); `elements`
/// holds the first [`PROBLEMS_LISTED`].
const SAID: usize = 10;

/// `the copy of instance 3` or `the copies of instances 3 and 4`, of `original`'s where a
/// pattern copies several, and whether it's plural.
fn copies_of(instances: &[Instance], original: Option<&FeatureId>) -> (String, bool) {
    let texts: Vec<String> = instances.iter().map(Instance::to_string).collect();
    let whose = original.map_or_else(String::new, |o| format!("{o}'s "));
    match texts.as_slice() {
        [one] => (format!("the copy of {whose}instance {one}"), false),
        _ => (
            format!(
                "the copies of {whose}instances {}",
                listed_first(texts.get(..SAID).unwrap_or(&texts), texts.len())
            ),
            true,
        ),
    }
}

/// How many pieces `n` is, in words where it's short.
fn pieces(n: u32) -> String {
    match n {
        2 => "two".to_owned(),
        3 => "three".to_owned(),
        n => n.to_string(),
    }
}

/// `value` to three significant figures, for an "about".
fn about(value: f64) -> String {
    if value <= 0.0 || !value.is_finite() {
        return silico_protocol::format_number(value);
    }
    let scale = libm::pow(10.0, 2.0 - libm::floor(libm::log10(value)));
    silico_protocol::format_number(libm::round(value * scale) / scale)
}

/// A step's result of more than one solid refused (S5, decision 6, open decision 4(a)),
/// `kernel_failed`, naming feature `id`, a `kind` of pattern, built on `base`: copies off
/// the base, named, for a fuse whose base's faces lie in one solid; a base cut in pieces; or
/// a base a fuse leaves in pieces, the fuzzy value its cause, from the result's tight box
/// `bounds` (the copies' and the base's, which the boolean's fuzzy value is a millionth of).
/// `several` says whether the pattern copies more than one original, so the message names
/// whose copies. None for a result of one solid.
pub(crate) fn solids_refused(
    id: &FeatureId,
    kind: &str,
    base: &FeatureId,
    verdict: &StepVerdict,
    several: bool,
    bounds: Option<[[f64; 3]; 2]>,
) -> Option<Error> {
    let n = verdict.solids;
    if n <= 1 {
        return None;
    }
    let failed =
        |message: String| Error::new(ErrorCode::KernelFailed, message).with_feature(id.as_str());
    if verdict.op == CopiesOp::Cut {
        return Some(
            failed(format!(
                "{kind} {id} cuts its base {base} in {}: the result is {n} solids",
                pieces(n)
            ))
            .with_suggestion("shorten or move the copies so the base stays one piece"),
        );
    }
    if verdict.base_solids == 1 && !verdict.off_base.is_empty() {
        let original = if several {
            verdict.original.as_ref()
        } else {
            None
        };
        let (copies, plural) = copies_of(&verdict.off_base, original);
        let (touch, meet) = if plural {
            ("don't touch", "meet")
        } else {
            ("doesn't touch", "meets")
        };
        let elements: Vec<String> = verdict
            .off_base
            .iter()
            .take(PROBLEMS_LISTED)
            .map(Instance::to_string)
            .collect();
        return Some(
            failed(format!(
                "{kind} {id} makes {n} solids: {copies} {touch} the base, or {meet} it only along an edge or at a point"
            ))
            .with_elements(elements)
            .with_elements_total(verdict.off_base.len() as u64)
            .with_suggestion("lower count so every instance touches the base, add them to skip, or move the pattern onto the base"),
        );
    }
    let mut error = failed(format!(
        "{kind} {id} leaves its base {base} in pieces: the base's faces lie in {} of the result's {n} solids, so no instance is named",
        verdict.base_solids
    ))
    .with_suggestion("keep the copies near the base: a pattern a kilometre long builds its step to about a millimetre");
    if let Some([low, high]) = bounds {
        let across = vec3::norm(vec3::sub(high, low));
        error = error.with_cause(format!(
            "the copies make the boolean's box about {} mm across, so its fuzzy value is about {} mm, a millionth of it: OCCT takes what's closer than that as touching",
            about(across),
            about(across * 1e-6)
        ));
    }
    Some(error)
}

/// The warning's words (section 4.3) for a pattern `id` of `kind` whose steps changed
/// nothing of these instances' copies, each step's (its original, whether it fuses, and its
/// instances): one clause a step that has any, naming the original where there are several.
/// None if no step has any.
pub(crate) fn changed_nothing_message(
    id: &FeatureId,
    kind: &str,
    steps: &[(Option<&FeatureId>, bool, &[Instance])],
) -> Option<String> {
    let several = steps.len() > 1;
    let clauses: Vec<String> = steps
        .iter()
        .filter(|(_, _, instances)| !instances.is_empty())
        .map(|(original, fuses, instances)| {
            let (copies, plural) = copies_of(instances, if several { *original } else { None });
            let verb = match (fuses, plural) {
                (true, true) => "add nothing",
                (true, false) => "adds nothing",
                (false, true) => "remove nothing",
                (false, false) => "removes nothing",
            };
            let why = if plural {
                "no face of theirs is in the result"
            } else {
                "none of its faces is in the result"
            };
            format!("{copies} {verb} ({why})")
        })
        .collect();
    if clauses.is_empty() {
        return None;
    }
    Some(format!("{kind} {id}: {}", clauses.join("; ")))
}

#[cfg(test)]
mod tests;
