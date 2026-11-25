//! M6.5's pattern read, `pattern` (docs/design/m6.0-patterns.md, section 5): a linear
//! pattern, polar pattern, or mirror as the model a read names built it, for the app's
//! instance boxes and marks (M6.7) and for an agent choosing which copy to name (`C2,1`).
//! Its references resolved in 3D, by the same functions its rebuild's steps resolve and
//! place them with (`crate::pattern`); where its original sits (`anchor`); and every
//! instance but the original, with its move or turn, its anchor placed, and what the
//! rebuild's steps said of its copy (`state`); and the pattern's own problems.
//!
//! A read asks no worker anything: a named reference is found in its base's element map and
//! read from the base's description, which the rebuild kept (`Model::description`); a
//! profile's centre is worked out from the loops of its sketch's solve, which the model
//! keeps (`Model::diagnosed`); and the verdicts are those the model kept of each step
//! (`Model::pattern_verdicts`).

use std::collections::BTreeSet;

use silico_protocol::{Axis, Placement, SketchPlane, vec3};

use super::elements::{built, solid, summarized, worded};
use super::{ModelChoice, Problems, ReadCommand, Running, SessionId, Sessions};
use crate::document::{
    Angle, Copies, ElementName, FeatureId, Length, Made, NameField, Operation, PROBLEMS_LISTED,
    PatternFeature,
};
use crate::kernel::{Clients, ElementKind};
use crate::naming::Instance;
use crate::pattern::{Geometry, Resolved, StepVerdict};
use crate::profile;
use crate::recompute::Model;
use crate::references::CANDIDATES;
use crate::worker::SILICO_BUG;
use crate::{Error, ErrorCode};

impl<P: Clients + 'static> Sessions<P> {
    /// `pattern` (M6.5): the pattern `pattern` of the model `model` names, as its rebuild
    /// built it ([`PatternRead`]). Refused, naming `pattern`, for a feature the document
    /// doesn't have, or one that isn't a pattern, the document's first patterns the
    /// candidates; or for a suppressed one, which places nothing. A pattern that failed, or
    /// is blocked, isn't refused: its problems say why, and what of it can be read is.
    /// Read as every model is ([`read_model`](Self::read_model)): the current one queued,
    /// waiting for the revision's rebuild, or the newest published at once, counted among
    /// the session's published reads.
    pub fn pattern(
        &self,
        session: &SessionId,
        model: ModelChoice,
        pattern: FeatureId,
    ) -> Running<PatternRead> {
        self.read_model(session, ReadCommand::Pattern, model, move |read| {
            let _span = tracing::info_span!(
                "pattern",
                feature = pattern.as_str(),
                model = read.model_read().of.as_str()
            )
            .entered();
            self::read(read.model(), &pattern)
        })
    }
}

/// What `pattern` answers: the pattern as its model built it.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternRead {
    /// The pattern's ID, as the request named it.
    pub pattern: FeatureId,
    /// Its type.
    pub kind: PatternFeature,
    /// A linear pattern's directions, in order, resolved in 3D: none for another type, or
    /// where its references didn't resolve.
    pub directions: Option<Vec<DirectionRead>>,
    /// A polar pattern's axis, resolved in 3D: none for another type, or where it didn't
    /// resolve.
    pub axis: Option<AxisRead>,
    /// A mirror's plane, resolved in 3D: none for another type, or where it didn't resolve.
    pub plane: Option<PlaneRead>,
    /// Where the original sits: the centre of area of its originals' profiles, each on its
    /// sketch's plane, weighted by their areas; or, for a whole solid's copies, the base's
    /// centre of mass. None where it can't be had: `problems` says why.
    pub anchor: Option<Point>,
    /// Every instance but the original, in `skip`'s order: none where the references didn't
    /// resolve.
    pub instances: Vec<InstanceRead>,
    /// The pattern's own problems, as `check` gives them, the first
    /// [`PROBLEMS_LISTED`]: none when it built.
    pub problems: Vec<Error>,
    /// How many problems there are, listed or not.
    pub problems_total: u64,
}

/// A point in global coordinates, each coordinate rounded to the nanometre.
pub type Point = [Length; 3];

/// One direction of a linear pattern, resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectionRead {
    /// Its unit vector, `reversed` taken.
    pub direction: [f64; 3],
    /// The distance between neighbours: L / (count − 1) for an extent, S for a spacing;
    /// none for an extent of count 1, where there's none.
    pub step: Option<Length>,
}

/// A polar pattern's axis, resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisRead {
    /// A point on it.
    pub point: Point,
    /// Its unit direction, `reversed` taken: the instances turn right-handed about it.
    pub direction: [f64; 3],
    /// The angle between neighbours; none for an extent of count 1.
    pub step: Option<Angle>,
}

/// A mirror's plane, resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaneRead {
    /// The global origin projected onto it (docs/ports/attacher.md, F2).
    pub origin: Point,
    /// Its unit normal.
    pub normal: [f64; 3],
}

/// One instance of a pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceRead {
    /// Its index, as `skip` writes it and its copies' names carry it.
    pub index: Instance,
    /// A linear instance's move; none for another type's.
    pub offset: Option<Point>,
    /// A polar instance's turn about the axis; none for another type's.
    pub angle: Option<Angle>,
    /// The pattern's anchor where this instance puts it; none where the anchor is none.
    pub anchor: Option<Point>,
    /// What the rebuild said of its copy; none where it said nothing.
    pub state: Option<InstanceState>,
}

/// What the rebuild said of an instance's copy (section 5): the first that holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InstanceState {
    /// `skip` holds it.
    Skipped,
    /// A step found its copy apart from the base, which refuses the pattern.
    OffBase,
    /// A step found its copy changed nothing: none of its faces is in the result.
    ChangedNothing,
    /// Built.
    Built,
}

impl InstanceState {
    /// Every state, in the order the read takes the first that holds, `null` (no verdict)
    /// coming before `built`.
    pub const ALL: [InstanceState; 4] = [
        InstanceState::Skipped,
        InstanceState::OffBase,
        InstanceState::ChangedNothing,
        InstanceState::Built,
    ];

    /// As the read writes it.
    pub const fn as_str(self) -> &'static str {
        match self {
            InstanceState::Skipped => "skipped",
            InstanceState::OffBase => "off_base",
            InstanceState::ChangedNothing => "changed_nothing",
            InstanceState::Built => "built",
        }
    }
}

/// The problems a read gathers: the pattern's own, as `check` gives them, then any the read
/// meets that they don't hold already; the first [`PROBLEMS_LISTED`] listed, every one
/// counted.
struct Gathered {
    errors: Vec<Error>,
    total: u64,
}

impl Gathered {
    fn add(&mut self, error: Error) {
        if self.errors.contains(&error) {
            return;
        }
        if self.errors.len() < PROBLEMS_LISTED {
            self.errors.push(error);
        }
        self.total += 1;
    }
}

/// The pattern `id` of `model`, as its rebuild built it ([`PatternRead`]): what `pattern`
/// answers from the model it reads, refused as it refuses.
pub fn read(model: &Model, id: &FeatureId) -> Result<PatternRead, Problems> {
    let (operation, kind) = pattern_of(model, id)?;
    // Its own problems, as a read of its solid gives them: none when it built.
    let mut problems = match built(model, id, false) {
        Ok(_) => Gathered {
            errors: Vec::new(),
            total: 0,
        },
        Err(refused) => Gathered {
            errors: refused.errors().to_vec(),
            total: refused.total(),
        },
    };
    let base = model.standing_base(id);
    let resolved = match base {
        Some(base) => resolved(model, id, operation, base),
        None => Err(Unresolved::Unbuilt),
    };
    let anchor = anchor(model, id, operation, base);
    let resolved = match resolved {
        Ok(resolved) => Some(resolved),
        Err(Unresolved::Unbuilt) => None,
        Err(Unresolved::Because(errors)) => {
            for error in errors {
                problems.add(error);
            }
            None
        }
    };
    let anchor = match anchor {
        Ok(anchor) => Some(anchor),
        Err(Unanchored::Unbuilt) => None,
        Err(Unanchored::Because(error)) => {
            problems.add(*error);
            None
        }
    };
    let mut read = PatternRead {
        pattern: id.clone(),
        kind,
        directions: None,
        axis: None,
        plane: None,
        anchor: anchor.map(rounded).transpose()?,
        instances: Vec::new(),
        problems: Vec::new(),
        problems_total: 0,
    };
    if let Some(resolved) = &resolved {
        geometry_of(&mut read, operation, resolved)?;
        read.instances = instances(model, id, operation, resolved, anchor)?;
    }
    // A read never leaves its geometry null with no problem to say why.
    if (resolved.is_none() || anchor.is_none()) && problems.total == 0 {
        problems.add(bug(
            id,
            "the pattern's references or anchor couldn't be read, nor why",
        ));
    }
    read.problems = problems.errors;
    read.problems_total = problems.total;
    Ok(read)
}

/// The pattern `id` of `model`'s document, and its type, or why there's none: a feature the
/// document doesn't have, or one that isn't a pattern, each `invalid_input` naming
/// `pattern`, the document's first [`CANDIDATES`] patterns, in timeline order, the
/// candidates (as `sketch` offers sketches); or a suppressed pattern, which places nothing,
/// refused naming `pattern` and the feature, as `sketch` refuses a suppressed sketch.
fn pattern_of<'m>(
    model: &'m Model,
    id: &FeatureId,
) -> Result<(&'m Operation, PatternFeature), Problems> {
    let document = model.document();
    let refused = |message: String| {
        let patterns: Vec<String> = document
            .features
            .iter()
            .filter(|f| matches!(f.operation.made(), Made::Pattern(_)))
            .take(CANDIDATES)
            .map(|f| f.id.as_str().to_owned())
            .collect();
        let suggestion = if patterns.is_empty() {
            "the document has no pattern: add a linear_pattern, polar_pattern, or mirror, and read it by its ID"
        } else {
            "name a pattern of the document (a linear_pattern, polar_pattern, or mirror), by its ID"
        };
        Problems::from(
            Error::new(ErrorCode::InvalidInput, message)
                .with_parameter("pattern")
                .with_suggestion(suggestion)
                .with_candidates(patterns),
        )
    };
    let Some(feature) = document.features.iter().find(|f| &f.id == id) else {
        return Err(refused(format!("the document has no feature {id}")));
    };
    let Made::Pattern(kind) = feature.operation.made() else {
        return Err(refused(format!(
            "{id} is {}, not a pattern",
            crate::error::article(feature.operation.type_name())
        )));
    };
    if feature.suppressed {
        return Err(Problems::from(
            Error::new(
                ErrorCode::InvalidInput,
                format!("{id} is suppressed, so it places no instances"),
            )
            .with_feature(id.as_str())
            .with_parameter("pattern")
            .with_suggestion(format!(
                "unsuppress {id} first (suppress_feature with suppressed false), or name a pattern that isn't suppressed"
            )),
        ));
    }
    Ok((&feature.operation, kind))
}

/// Why a pattern's references, or its anchor, weren't read: the feature they're read from
/// didn't build, which the pattern's own problems say (it's blocked on it); or a reason of
/// the read's own.
enum Unresolved {
    Unbuilt,
    Because(Vec<Error>),
}

enum Unanchored {
    Unbuilt,
    Because(Box<Error>),
}

/// `operation`'s references, pattern `id`'s, resolved as its steps resolve them (section
/// 2.4): each stored name found in the map of `base`, the feature whose solid it builds on
/// (`references::find`, each broken one `reference_broken` with candidates), read from that
/// solid's kept description, and checked for its kind; then [`crate::pattern::resolve`],
/// the steps' own. A base that didn't build is the pattern's block, said already; a base
/// whose description wasn't kept is refused as `elements` refuses it.
fn resolved(
    model: &Model,
    id: &FeatureId,
    operation: &Operation,
    base: &FeatureId,
) -> Result<Resolved, Unresolved> {
    let makers = |f: &FeatureId| model.maker(f);
    let wanted: Vec<(NameField, &ElementName, ElementKind)> = operation
        .stored_names()
        .map(|(field, name)| {
            let kind = crate::naming::Name::parse(name.as_str(), &makers)
                .map_or(ElementKind::Edge, |parsed| parsed.kind());
            (field, name, kind)
        })
        .collect();
    let mut found = Vec::new();
    let mut description = None;
    if !wanted.is_empty() {
        if built(model, base, false).is_err() {
            return Err(Unresolved::Unbuilt);
        }
        let Some(described) = model.description(base) else {
            // Why it wasn't described, as `elements` says it of that solid.
            let why = solid(model, base, false)
                .err()
                .map(|problems| problems.errors().to_vec())
                .filter(|errors| !errors.is_empty())
                .unwrap_or_else(|| vec![bug(base, "its solid wasn't described, nor why")]);
            return Err(Unresolved::Because(why));
        };
        let Some(map) = model.element_map(base) else {
            return Err(Unresolved::Because(vec![bug(
                base,
                "its solid built, but its elements couldn't be named",
            )]));
        };
        match crate::references::find(id, &wanted, map, &makers) {
            Ok(elements) => {
                found = wanted
                    .iter()
                    .map(|(_, name, _)| *name)
                    .zip(elements)
                    .collect::<Vec<_>>();
            }
            Err(broken) => return Err(Unresolved::Because(broken.errors)),
        }
        description = Some(described);
    }
    let mut geometry = |name: &ElementName| -> Result<Geometry, Error> {
        let element = found
            .iter()
            .find(|(n, _)| *n == name)
            .map(|&(_, element)| element);
        let slot = element.and_then(|e| (e.index as usize).checked_sub(1).map(|i| (e.kind, i)));
        let described = match (description, slot) {
            (Some(d), Some((ElementKind::Face, i))) => d.faces.get(i).cloned().map(Geometry::Face),
            (Some(d), Some((ElementKind::Edge, i))) => d.edges.get(i).cloned().map(Geometry::Edge),
            _ => None,
        };
        described.ok_or_else(|| bug(id, "a pattern's reference was found, but not described"))
    };
    crate::pattern::resolve(id, operation, &mut geometry)
        .map_err(|error| Unresolved::Because(vec![worded(&error)]))
}

/// Where the original sits, unrounded: for a whole solid's copies, the centre of mass of
/// `base`, the solid it builds on; otherwise the centre of area of its originals' profiles,
/// each placed in 3D on its sketch's plane, weighted by their areas, Σ(Aᵢ · cᵢ) / Σ Aᵢ,
/// which depends on no order (Fable, 2026-11-10). An original whose profile or plane can't
/// be had didn't build, which the pattern's own problems say (it's blocked on it).
fn anchor(
    model: &Model,
    id: &FeatureId,
    operation: &Operation,
    base: Option<&FeatureId>,
) -> Result<[f64; 3], Unanchored> {
    if matches!(operation.copies(), Some(Copies::Solid)) {
        let Some(base) = base else {
            return Err(Unanchored::Unbuilt);
        };
        if built(model, base, false).is_err() {
            return Err(Unanchored::Unbuilt);
        }
        return match summarized(model, base, false) {
            Ok(summary) => Ok(summary.center),
            Err(problems) => Err(Unanchored::Because(Box::new(
                problems
                    .errors()
                    .first()
                    .cloned()
                    .unwrap_or_else(|| bug(base, "its solid wasn't measured, nor why")),
            ))),
        };
    }
    let (mut area, mut moment) = (0.0, [0.0; 3]);
    // Each original once, whatever the list holds.
    let originals: BTreeSet<&FeatureId> = operation.originals().iter().collect();
    for original in originals {
        let (a, centre) = profile_centre(model, original).ok_or(Unanchored::Unbuilt)?;
        area += a;
        moment = add(moment, centre.map(|c| c * a));
    }
    let anchor = moment.map(|m| m / area);
    if area > 0.0 && anchor.iter().all(|c| c.is_finite()) {
        Ok(anchor)
    } else {
        Err(Unanchored::Because(Box::new(bug(
            id,
            "its originals' profiles have no area to centre",
        ))))
    }
}

/// The area of the original `original`'s profile and its centre of area in 3D: the profile
/// of the sketch it sweeps, from the loops of that sketch's solve the model keeps, measured
/// exactly by the faces request's own measure (`silico_protocol::area_moments`), each loop
/// about a point of its own, then all about the first's, an outer loop's area counted and a
/// hole's (walked the other way) taken away; placed on its sketch's plane as the rebuild
/// resolved it. None where any of it can't be had.
fn profile_centre(model: &Model, original: &FeatureId) -> Option<(f64, [f64; 3])> {
    let sketch_id = model.feature(original)?.operation.sketch()?;
    let Operation::Sketch(sketch) = &model.feature(sketch_id)?.operation else {
        return None;
    };
    let diagnosed = model.diagnosed(sketch_id)?.ok()?;
    let solution = diagnosed.outcome().ok()?;
    let profile = profile::of_sketch(sketch_id, sketch, solution).ok()?;
    let plane = match sketch.plane.origin() {
        Some(origin) => SketchPlane::from(origin),
        None => SketchPlane::Face(*model.frame(sketch_id)?),
    };
    let mut about: Option<[f64; 2]> = None;
    let (mut area, mut moment) = (0.0, [0.0, 0.0]);
    for region in &profile.regions {
        for walked in std::iter::once(&region.outer).chain(&region.holes) {
            let measured = silico_protocol::area_moments(&walked.sent());
            let here = *about.get_or_insert(measured.about);
            let shift = [measured.about[0] - here[0], measured.about[1] - here[1]];
            area += measured.area;
            moment = [
                moment[0] + measured.moment[0] + shift[0] * measured.area,
                moment[1] + measured.moment[1] + shift[1] * measured.area,
            ];
        }
    }
    let about = about?;
    let centre = [about[0] + moment[0] / area, about[1] + moment[1] / area];
    (area > 0.0).then(|| (area, plane.place(centre)))
}

/// `read`'s references, as `resolved` has them in 3D, each step from the document's values
/// as the steps work it out (`crate::pattern::linear_step`, `polar_degrees`).
fn geometry_of(
    read: &mut PatternRead,
    operation: &Operation,
    resolved: &Resolved,
) -> Result<(), Problems> {
    match (operation, resolved) {
        (Operation::LinearPattern { directions, .. }, Resolved::Linear(along)) => {
            read.directions = Some(
                directions
                    .iter()
                    .zip(along)
                    .map(|(direction, unit)| {
                        let step = match direction.distribution {
                            crate::document::LinearDistribution::Extent { .. }
                                if direction.count < 2 =>
                            {
                                None
                            }
                            distribution => Some(length(crate::pattern::linear_step(
                                distribution,
                                direction.count,
                                1,
                            ))?),
                        };
                        Ok(DirectionRead {
                            direction: *unit,
                            step,
                        })
                    })
                    .collect::<Result<_, Problems>>()?,
            );
        }
        (
            Operation::PolarPattern {
                distribution,
                count,
                ..
            },
            Resolved::Polar(axis),
        ) => {
            let step = match distribution {
                crate::document::PolarDistribution::Extent { .. } if *count < 2 => None,
                distribution => Some(angle(crate::pattern::polar_degrees(
                    *distribution,
                    *count,
                    1,
                ))?),
            };
            read.axis = Some(AxisRead {
                point: rounded(axis.origin)?,
                direction: axis.direction,
                step,
            });
        }
        (Operation::Mirror { .. }, Resolved::Mirror { origin, normal }) => {
            read.plane = Some(PlaneRead {
                origin: rounded(*origin)?,
                normal: *normal,
            });
        }
        _ => {
            return Err(bug(
                &read.pattern,
                "a pattern's references were resolved as another type's",
            )
            .into());
        }
    }
    Ok(())
}

/// Every instance of the pattern `id` but the original, with its placement, the anchor
/// placed, and its state.
fn instances(
    model: &Model,
    id: &FeatureId,
    operation: &Operation,
    resolved: &Resolved,
    anchor: Option<[f64; 3]>,
) -> Result<Vec<InstanceRead>, Problems> {
    let skipped = crate::pattern::skipped(operation);
    let verdicts = model.pattern_verdicts(id).unwrap_or(&[]);
    // A step that didn't run, or read no instance (a base in pieces names none), gave no
    // verdict for any instance it didn't name.
    let silent = verdicts.len() < steps(operation)
        || verdicts.iter().any(|verdict| verdict.base_solids != 1);
    crate::pattern::instances(operation)
        .into_iter()
        .map(|index| {
            let placement = crate::pattern::placement(operation, resolved, &index)
                .ok_or_else(|| bug(id, "a pattern's references were resolved as another type's"))?;
            let (offset, angle_of) = match &placement {
                Placement::Translation { offset } => (Some(rounded(*offset)?), None),
                Placement::Rotation { .. } => {
                    let degrees = match operation {
                        Operation::PolarPattern {
                            distribution,
                            count,
                            ..
                        } => crate::pattern::polar_degrees(
                            *distribution,
                            *count,
                            index.offsets().first().copied().unwrap_or(0),
                        ),
                        _ => 0.0,
                    };
                    (None, Some(angle(degrees)?))
                }
                Placement::Mirror { .. } | Placement::TurnedAndMoved { .. } => (None, None),
            };
            let anchor = anchor
                .map(|point| rounded(placed(&placement, point)))
                .transpose()?;
            let state = state(&index, &skipped, verdicts, silent);
            Ok(InstanceRead {
                index,
                offset,
                angle: angle_of,
                anchor,
                state,
            })
        })
        .collect()
}

/// How many steps a pattern's rebuild runs: one for each original, or one for a whole
/// solid's copies.
fn steps(operation: &Operation) -> usize {
    match operation.copies() {
        Some(Copies::Features { features }) => features.iter().collect::<BTreeSet<_>>().len(),
        _ => 1,
    }
}

/// An instance's state, the first that holds (section 5): `skipped`, `off_base`,
/// `changed_nothing`, none where a step gave no verdict, `built`.
fn state(
    index: &Instance,
    skipped: &BTreeSet<&[u32]>,
    verdicts: &[StepVerdict],
    silent: bool,
) -> Option<InstanceState> {
    if skipped.contains(index.offsets()) {
        Some(InstanceState::Skipped)
    } else if verdicts.iter().any(|v| v.off_base.contains(index)) {
        Some(InstanceState::OffBase)
    } else if verdicts.iter().any(|v| v.changed_nothing.contains(index)) {
        Some(InstanceState::ChangedNothing)
    } else if silent {
        None
    } else {
        Some(InstanceState::Built)
    }
}

/// `point` where `placement` takes it: moved, turned right-handed about its axis (by
/// Rodrigues' rotation, through the libm shim), or reflected in its plane.
fn placed(placement: &Placement, point: [f64; 3]) -> [f64; 3] {
    match placement {
        Placement::Translation { offset } => add(point, *offset),
        Placement::Rotation { axis, angle } => turned(axis, *angle, point),
        Placement::Mirror { origin, normal } => {
            let n = vec3::normalize(*normal);
            let along = vec3::dot(vec3::sub(point, *origin), n);
            vec3::sub(point, n.map(|c| 2.0 * along * c))
        }
        Placement::TurnedAndMoved {
            axis,
            angle,
            offset,
        } => add(turned(axis, *angle, point), *offset),
    }
}

/// `point` turned by `angle` radians right-handed about `axis`.
fn turned(axis: &Axis, angle: f64, point: [f64; 3]) -> [f64; 3] {
    let k = vec3::normalize(axis.direction);
    let v = vec3::sub(point, axis.origin);
    let (sin, cos) = (libm::sin(angle), libm::cos(angle));
    let along = vec3::dot(k, v) * (1.0 - cos);
    let cross = vec3::cross(k, v);
    let turned = [0, 1, 2].map(|i| v[i] * cos + cross[i] * sin + k[i] * along);
    add(axis.origin, turned)
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `point`, each coordinate rounded to the nanometre, once.
fn rounded(point: [f64; 3]) -> Result<Point, Problems> {
    Ok([length(point[0])?, length(point[1])?, length(point[2])?])
}

fn length(mm: f64) -> Result<Length, Problems> {
    Length::rounded(mm).ok_or_else(|| {
        Error::new(
            ErrorCode::Internal,
            format!("a pattern's read worked out a length of {mm} mm, which isn't one"),
        )
        .with_suggestion(SILICO_BUG)
        .into()
    })
}

fn angle(degrees: f64) -> Result<Angle, Problems> {
    Angle::rounded(degrees).ok_or_else(|| {
        Error::new(
            ErrorCode::Internal,
            format!("a pattern's read worked out an angle of {degrees}°, which isn't one"),
        )
        .with_suggestion(SILICO_BUG)
        .into()
    })
}

/// A bug's refusal, naming `feature`.
fn bug(feature: &FeatureId, what: &str) -> Error {
    Error::new(ErrorCode::Internal, format!("{feature}: {what}"))
        .with_feature(feature.as_str())
        .with_suggestion(SILICO_BUG)
}
