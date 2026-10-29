//! Measure cases (M4.5; docs/ports/measure.md, *Decided*; Fable's decision,
//! `m4-measure-fable.md`, *Oracle*): pairs of elements of one Silico document's solids,
//! each element by its stable name, measured by the core's `distance` and `angle`
//! (`silico_core::command::measure`) on the document as Silico rebuilds it, and by FreeCAD
//! 1.1.3's `MeasureDistance` and `MeasureAngle` on the same features as FreeCAD builds them.
//! FreeCAD finds each element by its geometry, as Silico describes it ([`Element`]), never by
//! its number, which the two builds needn't share. The two are compared by the decision's
//! rules ([`Rule`]), and Silico's answer with the case's values, worked by hand
//! ([`against_expected`]), so each case is checked independently of both.
//!
//! The cases are `measures/` in an oracle directory: its document, [`MODEL`], and its
//! cases, [`CASES`], a JSON list of [`MeasureCase`]s.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use silico_core::command::measure::{AngleAnswer, DistanceAnswer, angle, distance};
use silico_core::command::rebuild;
use silico_core::document::{Angle, Checked, Feature, FeatureId, Length, Operation, read};
use silico_core::kernel::{EdgeCurve, ElementKind, Kernel, Surface};
use silico_core::naming::{Maker, Name};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute};

use crate::{features, freecad};

/// The directory of measure cases in an oracle directory.
pub const DIR: &str = "measures";
/// Its document, whose solids the cases measure.
pub const MODEL: &str = "model.silico";
/// Its cases.
pub const CASES: &str = "cases.json";

/// How far apart Silico's lengths and FreeCAD's may be, in mm: Silico's are rounded to the
/// nanometre, so up to half of one off FreeCAD's, which OCCT finds to about 1e-12 on these
/// solids.
pub const LENGTH_TOLERANCE: f64 = 1e-6;

/// How far apart Silico's angles and FreeCAD's may be, in degrees: Silico's are rounded to
/// the nanodegree, and FreeCAD's `acos` is good to about 1e-12 degrees away from 0 and 180,
/// where it reads parallel directions as exactly 0.
pub const ANGLE_TOLERANCE: f64 = 1e-8;

/// One case: two elements, by name, and what Silico's measure of them is, by hand.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasureCase {
    pub name: String,
    pub note: String,
    pub elements: [String; 2],
    pub expected: Expected,
}

/// What a case's measure is, and what Silico's answer to it is expected to be.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Expected {
    Distance(ExpectedDistance),
    Angle(ExpectedAngle),
}

/// A `distance` answer's values, by hand: `center_distance`, `normal_distance`, and
/// `center_to_element` are written, `null` where the answer has none; the witness, where it's
/// given, is the pair of points the answer gives (from the first element, to the second).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedDistance {
    pub distance: f64,
    #[serde(deserialize_with = "written")]
    pub center_distance: Option<f64>,
    #[serde(deserialize_with = "written")]
    pub normal_distance: Option<f64>,
    #[serde(deserialize_with = "written")]
    pub center_to_element: Option<f64>,
    #[serde(default)]
    pub witness: Option<[[f64; 3]; 2]>,
}

/// An `angle` answer's values, by hand: the angle, and where the vertex it names is, `null`
/// where it names none.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedAngle {
    pub angle: f64,
    #[serde(deserialize_with = "written")]
    pub vertex: Option<[f64; 3]>,
}

/// A value that may be `null` but must be written: missing is an error (serde would take a
/// missing `Option` as `None`, which would check less than the case seems to).
fn written<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}

/// The document of cases, through the gate, and the cases, in their file's order.
#[derive(Debug, Clone)]
pub struct Cases {
    pub document: Checked,
    pub cases: Vec<MeasureCase>,
}

/// The cases in `dir`'s [`DIR`]; none if it has none. Each case has a name of its own.
pub fn load(dir: &Path) -> Result<Option<Cases>, String> {
    let dir = dir.join(DIR);
    let model = dir.join(MODEL);
    let bytes = match std::fs::read(&model) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("can't read {}: {e}", model.display())),
    };
    let at = |e: String| format!("{}: {e}", model.display());
    let document = read(&bytes).map_err(|e| at(e.to_string()))?;
    let checked = Checked::new(document).map_err(|invalid| {
        let errors: Vec<String> = invalid.errors().iter().map(ToString::to_string).collect();
        at(errors.join("; "))
    })?;
    let path = dir.join(CASES);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let cases: Vec<MeasureCase> = serde_json::from_str(&text)
        .map_err(|e| format!("{} isn't a list of measure cases: {e}", path.display()))?;
    if cases.is_empty() {
        return Err(format!("{} has no cases", path.display()));
    }
    let mut names = BTreeSet::new();
    for case in &cases {
        if !names.insert(case.name.as_str()) {
            return Err(format!(
                "two measure cases are named {} ({})",
                case.name,
                path.display()
            ));
        }
    }
    Ok(Some(Cases {
        document: checked,
        cases,
    }))
}

/// An element as Silico's rebuild describes it, which FreeCAD finds it by in its own build
/// (`oracle.py`'s `element`): a vertex by its point; an edge by its curve's kind, its
/// length, and its ends and middle, each on it; a face by its surface's kind, its area, and
/// its centre of area. Exactly one of FreeCAD's elements must fit.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Element {
    Vertex {
        point: [f64; 3],
    },
    Edge {
        curve: &'static str,
        length: f64,
        start: [f64; 3],
        end: [f64; 3],
        middle: [f64; 3],
    },
    Face {
        surface: &'static str,
        area: f64,
        centroid: [f64; 3],
    },
}

impl Element {
    /// A straight edge: a line (no case has a B-spline or Bézier of two poles).
    fn is_line(&self) -> bool {
        matches!(self, Element::Edge { curve: "line", .. })
    }

    fn is_circle(&self) -> bool {
        matches!(
            self,
            Element::Edge {
                curve: "circle",
                ..
            }
        )
    }

    /// A cylinder face, whose centre is its axis.
    fn is_cylinder(&self) -> bool {
        matches!(
            self,
            Element::Face {
                surface: "cylinder",
                ..
            }
        )
    }

    fn is_plane(&self) -> bool {
        matches!(
            self,
            Element::Face {
                surface: "plane",
                ..
            }
        )
    }
}

/// An element found by its name: the solid it's in, and its geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub body: FeatureId,
    pub element: Element,
}

/// The element `text` names, in the first of `solids` whose element map has it, as the
/// core's measures look for it (`find`, in `command::measure`): the name parsed as a rebuild
/// parses names, then looked for in each solid's map in turn.
pub fn resolve(model: &Model, solids: &[FeatureId], text: &str) -> Result<Found, String> {
    let makers = |id: &FeatureId| {
        model
            .document()
            .features
            .iter()
            .find(|f| f.id == *id)
            .map(|f| Maker::of(&f.operation))
    };
    let name = Name::parse(text, &makers).map_err(|e| format!("{text} isn't a name: {e}"))?;
    let (body, element) = solids
        .iter()
        .find_map(|id| Some((id, model.element_map(id)?.element(&name)?)))
        .ok_or_else(|| format!("{text} names no element of {solids:?}"))?;
    let description = model
        .description(body)
        .ok_or_else(|| format!("{body}'s solid wasn't described"))?;
    let at = (element.index as usize).checked_sub(1);
    let missing = || format!("{text} is {element}, which {body}'s description hasn't");
    let element = match element.kind {
        ElementKind::Vertex => Element::Vertex {
            point: *at
                .and_then(|i| description.vertices.get(i))
                .ok_or_else(missing)?,
        },
        ElementKind::Edge => {
            let edge = at
                .and_then(|i| description.edges.get(i))
                .ok_or_else(missing)?;
            Element::Edge {
                curve: match edge.curve {
                    EdgeCurve::Line => "line",
                    EdgeCurve::Circle { .. } => "circle",
                    EdgeCurve::Ellipse { .. } => "ellipse",
                    _ => return Err(format!("{text}'s curve is one no case measures")),
                },
                length: edge.length,
                start: edge.start,
                end: edge.end,
                middle: edge.middle,
            }
        }
        ElementKind::Face => {
            let face = at
                .and_then(|i| description.faces.get(i))
                .ok_or_else(missing)?;
            Element::Face {
                surface: match face.surface {
                    Surface::Plane { .. } => "plane",
                    Surface::Cylinder { .. } => "cylinder",
                    Surface::Cone { .. } => "cone",
                    Surface::Sphere { .. } => "sphere",
                    Surface::Torus { .. } => "torus",
                    _ => return Err(format!("{text}'s surface is one no case measures")),
                },
                area: face.area,
                centroid: face.centroid,
            }
        }
        ElementKind::Solid => return Err(format!("{text} names a solid")),
    };
    Ok(Found {
        body: body.clone(),
        element,
    })
}

/// How a case's Silico answer and FreeCAD's measure are compared (Fable's decision,
/// *Oracle*), by what its two elements are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// Silico's `distance` is FreeCAD's `MeasureDistance`, the least distance as OCCT
    /// finds it.
    Least,
    /// Two circles or arcs, which FreeCAD measures between their centres
    /// (`distanceCircleCircle`, `04a6f38ec8`): its `MeasureDistance` is Silico's
    /// `center_distance`, and its `distToShape` Silico's `distance`.
    Centres,
    /// Two planar faces of one body: Silico's angle (of the wedge both outward normals point
    /// into) is 180 less FreeCAD's (between the normals, each turned away from the middle of
    /// the two faces' centres); but where FreeCAD reads exactly 0, its reading of parallel
    /// and opposite normals alike, the two are compared folded, as min(a, 180 − a).
    ///
    /// It holds only while 1.1.3's turn flips neither outward normal or both, as it does
    /// for two faces of one body at a convex corner (neither: each face's centre is behind
    /// the other's plane) or a concave one (both: a V-groove, a valley, an L's inside
    /// corner). Where it flips just one, FreeCAD reads Silico's angle itself, not its
    /// supplement. Faces of two bodies have no corner between them, so which way each is
    /// turned is happenstance: [`Rule::of`] refuses them. A pair of one body whose turn
    /// flips one (a non-convex body's faces apart from any corner) is seen, not passed: it
    /// reads as a difference (but at 90, where the two agree anyway), and Silico's value is
    /// checked by hand besides.
    Supplement,
    /// Two straight edges: each folded, min(a, 180 − a) (FreeCAD's 0 to 180 turns each line
    /// by where the edges' centres are; Silico's is 0 to 180 at a shared vertex, 0 to 90
    /// otherwise).
    Folded,
    /// A straight edge and a planar face: Silico's angle (between the line and the plane)
    /// is |90 − FreeCAD's| (between the line and the normal).
    FromNormal,
}

impl Rule {
    /// The rule for `expected`'s measure of `a` and `b`; none for an angle FreeCAD's
    /// `MeasureAngle` doesn't take (its gate takes straight edges and planar faces), or for
    /// the angle of planar faces of two bodies, which [`Rule::Supplement`] can't compare.
    pub fn of(expected: &Expected, a: &Found, b: &Found) -> Result<Rule, String> {
        let (body, a, b) = (a.body == b.body, &a.element, &b.element);
        match expected {
            Expected::Distance(_) if a.is_circle() && b.is_circle() => Ok(Rule::Centres),
            Expected::Distance(_) => Ok(Rule::Least),
            Expected::Angle(_) => match (a.is_plane(), b.is_plane(), a.is_line(), b.is_line()) {
                (true, true, _, _) if body => Ok(Rule::Supplement),
                (true, true, _, _) => Err("the angle of planar faces of two bodies has no \
                    rule: FreeCAD turns each normal away from the middle of the faces' centres, \
                    which, for faces with no corner between them, may turn one and not the \
                    other, so its reading needn't be 180 less Silico's; name two faces of one \
                    body"
                    .to_owned()),
                (_, _, true, true) => Ok(Rule::Folded),
                (true, _, _, true) | (_, true, true, _) => Ok(Rule::FromNormal),
                _ => Err("an angle takes straight edges and planar faces only".to_owned()),
            },
        }
    }

    /// The rule as the report says it.
    pub fn says(self) -> &'static str {
        match self {
            Rule::Least => "distance = FreeCAD's",
            Rule::Centres => "center_distance = FreeCAD's, distance = its distToShape",
            Rule::Supplement => "angle = 180 - FreeCAD's (folded where FreeCAD reads 0)",
            Rule::Folded => "min(angle, 180 - angle) = min(FreeCAD's, 180 - FreeCAD's)",
            Rule::FromNormal => "angle = |90 - FreeCAD's|",
        }
    }
}

/// Silico's answer to a case.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Distance(DistanceAnswer),
    Angle(AngleAnswer),
}

/// What Silico made of a case: its two elements, as found, and its answer, or every
/// reason it refused; and, for an angle that names a vertex, where that vertex is.
#[derive(Debug, Clone, PartialEq)]
pub struct Ours {
    pub found: [Found; 2],
    pub answer: Result<Answer, Vec<silico_core::Error>>,
    pub vertex_at: Option<[f64; 3]>,
}

/// Silico's answer to each of `cases`: the document rebuilt through the core's commands (on
/// a pool of kernel workers, its sketches on the sketch worker), and each case measured by
/// the core's `distance` or `angle` on it.
pub fn silico(cases: &Cases) -> Result<Vec<Ours>, String> {
    let _span = tracing::info_span!("silico_measures", cases = cases.cases.len()).entered();
    let (engine, model) = rebuilt(cases)?;
    answers(&model, engine.kernel(), &cases.cases)
}

/// The cases' document rebuilt on a fresh pool of kernel workers, and the engine that did,
/// whose kernel measures distances.
fn rebuilt(cases: &Cases) -> Result<(Recompute<KernelPool>, Arc<Model>), String> {
    let engine = Recompute::new(KernelPool::start());
    let model = rebuild(&engine, cases.document.clone())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the rebuild was superseded, though nothing else ran".to_owned())?;
    Ok((engine, model))
}

/// Each of `cases` measured on `model`, `distance` on `kernel`. A name that names nothing
/// is the case file's mistake, not an answer.
pub fn answers<K: Kernel>(
    model: &Model,
    kernel: &K,
    cases: &[MeasureCase],
) -> Result<Vec<Ours>, String> {
    let bodies = model.bodies();
    cases
        .iter()
        .map(|case| {
            let at = |e: String| format!("{}: {e}", case.name);
            let found = [
                resolve(model, bodies, &case.elements[0]).map_err(at)?,
                resolve(model, bodies, &case.elements[1]).map_err(at)?,
            ];
            let refused = |p: silico_core::command::Problems| p.errors().to_vec();
            let answer = match case.expected {
                Expected::Distance(_) => distance(model, kernel, &case.elements, None)
                    .map(Answer::Distance)
                    .map_err(refused),
                Expected::Angle(_) => angle(model, &case.elements, None)
                    .map(Answer::Angle)
                    .map_err(refused),
            };
            let vertex_at = match &answer {
                Ok(Answer::Angle(AngleAnswer {
                    vertex: Some(name), ..
                })) => match resolve(model, bodies, name.as_str()).map_err(at)?.element {
                    Element::Vertex { point } => Some(point),
                    other => return Err(at(format!("its vertex {} is {other:?}", name.as_str()))),
                },
                _ => None,
            };
            Ok(Ours {
                found,
                answer,
                vertex_at,
            })
        })
        .collect()
}

/// What FreeCAD made of a case.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Theirs {
    Measured(Measured),
    /// FreeCAD couldn't measure it (its element not found, a measure it marked invalid, a
    /// value that isn't finite): why.
    Failed(String),
    /// FreeCAD crashed or hung on it: how it ended.
    #[serde(skip_deserializing)]
    Lost(String),
}

/// FreeCAD's measures of a case's pair: the sub-elements it found them as (`Face3`); for a
/// distance, `MeasureDistance`'s distance and points, `distToShape`'s least distance, how
/// far Silico's witness points are from the elements they're on, and, where one element is a
/// vertex or a circle or arc and the other a planar face or a straight edge, the distance from
/// its centre to the other as `Measurement::circleCenterDistance` builds it (the centre made a
/// vertex, `distToShape` from it); for an angle, `MeasureAngle`'s.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measured {
    pub elements: [String; 2],
    #[serde(default)]
    pub distance: Option<f64>,
    #[serde(default)]
    pub from: Option<[f64; 3]>,
    #[serde(default)]
    pub to: Option<[f64; 3]>,
    #[serde(default)]
    pub dist_to_shape: Option<f64>,
    #[serde(default)]
    pub witness: Option<[f64; 2]>,
    #[serde(default)]
    pub center_to_element: Option<f64>,
    #[serde(default)]
    pub angle: Option<f64>,
}

/// A case as `oracle.py` takes it.
#[derive(Serialize)]
struct FreecadCase<'a> {
    name: &'a str,
    measure: FreecadMeasure,
}

#[derive(Serialize)]
struct FreecadMeasure {
    kind: &'static str,
    /// Each body the pair is in, as FreeCAD builds it (`oracle.py`'s `build`).
    bodies: BTreeMap<String, Value>,
    elements: Vec<FreecadElement>,
    /// Silico's witness, whose points FreeCAD measures from the elements.
    witness: Option<[[f64; 3]; 2]>,
}

#[derive(Serialize)]
struct FreecadElement {
    body: String,
    element: Element,
}

/// The solid `id` of `model`'s document as FreeCAD builds it (`oracle.py`'s `build`): a
/// primitive by Part's `make<Kind>` at its position, a boolean of its two solids, a fillet
/// or chamfer of its base, each edge found by its geometry in FreeCAD's base as Silico's
/// base describes it, and a pad, pocket, revolve, or groove as a sketch-feature case's
/// PartDesign body.
pub fn body(model: &Model, id: &FeatureId) -> Result<Value, String> {
    let features: &[Feature] = &model.document().features;
    let feature = features
        .iter()
        .find(|f| f.id == *id)
        .ok_or_else(|| format!("{id} isn't a feature"))?;
    let placed = |shape: &str, position: &[Length; 3], values: Vec<f64>| {
        json!({"placed": {
            "shape": shape,
            "position": [position[0].mm(), position[1].mm(), position[2].mm()],
            "values": values,
        }})
    };
    let edges = |base: &FeatureId, names: &[silico_core::document::ElementName]| {
        names
            .iter()
            .map(|name| {
                resolve(model, std::slice::from_ref(base), name.as_str()).map(|found| found.element)
            })
            .collect::<Result<Vec<Element>, String>>()
    };
    Ok(match &feature.operation {
        Operation::Box {
            position,
            length,
            width,
            height,
        } => placed("box", position, vec![length.mm(), width.mm(), height.mm()]),
        Operation::Cylinder {
            position,
            radius,
            height,
        } => placed("cylinder", position, vec![radius.mm(), height.mm()]),
        Operation::Sphere { position, radius } => placed("sphere", position, vec![radius.mm()]),
        Operation::Cone {
            position,
            radius1,
            radius2,
            height,
        } => placed(
            "cone",
            position,
            vec![radius1.mm(), radius2.mm(), height.mm()],
        ),
        Operation::Torus {
            position,
            radius1,
            radius2,
        } => placed("torus", position, vec![radius1.mm(), radius2.mm()]),
        Operation::Fuse { base, tool } => {
            json!({"fuse": {"base": body(model, base)?, "tool": body(model, tool)?}})
        }
        Operation::Cut { base, tool } => {
            json!({"cut": {"base": body(model, base)?, "tool": body(model, tool)?}})
        }
        Operation::Common { base, tool } => {
            json!({"common": {"base": body(model, base)?, "tool": body(model, tool)?}})
        }
        Operation::Fillet {
            base,
            edges: names,
            radius,
        } => json!({"fillet": {
            "shape": body(model, base)?,
            "edges": edges(base, names)?,
            "radius": radius.mm(),
        }}),
        Operation::Chamfer {
            base,
            edges: names,
            size,
        } => json!({"chamfer": {
            "shape": body(model, base)?,
            "edges": edges(base, names)?,
            "size": {"equal": {"distance": size.distance.mm()}},
        }}),
        Operation::Pad { .. }
        | Operation::Pocket { .. }
        | Operation::Revolve { .. }
        | Operation::Groove { .. } => json!({"part_design": {
            "features": features::part_design_model(features, id)?,
        }}),
        Operation::Sketch(_) => return Err(format!("{id} is a sketch, which makes no solid")),
    })
}

/// What FreeCAD measures for each of `cases`, in order, given Silico's `ours` (each case's
/// elements, found, and Silico's witness, which FreeCAD measures from them).
pub fn freecad(model: &Model, cases: &[MeasureCase], ours: &[Ours]) -> Result<Vec<Theirs>, String> {
    let _span = tracing::info_span!("freecad_measures", cases = cases.len()).entered();
    let mut asked = Vec::with_capacity(cases.len());
    for (case, ours) in cases.iter().zip(ours) {
        let mut bodies = BTreeMap::new();
        for found in &ours.found {
            if !bodies.contains_key(found.body.as_str()) {
                bodies.insert(found.body.as_str().to_owned(), body(model, &found.body)?);
            }
        }
        let witness = match &ours.answer {
            Ok(Answer::Distance(d)) => Some([d.from.map(Length::mm), d.to.map(Length::mm)]),
            _ => None,
        };
        asked.push(FreecadCase {
            name: &case.name,
            measure: FreecadMeasure {
                kind: match case.expected {
                    Expected::Distance(_) => "distance",
                    Expected::Angle(_) => "angle",
                },
                bodies,
                elements: ours
                    .found
                    .iter()
                    .map(|f| FreecadElement {
                        body: f.body.as_str().to_owned(),
                        element: f.element.clone(),
                    })
                    .collect(),
                witness,
            },
        });
    }
    freecad::run_as(&asked, Theirs::Lost)
}

/// min(a, 180 − a): an angle between lines, whichever way each runs.
fn folded(a: f64) -> f64 {
    a.min(180.0 - a)
}

/// Every way Silico's answer `ours` differs from FreeCAD's measure `theirs` of `case`, by
/// `rule`; none if they agree.
pub fn compare(case: &MeasureCase, rule: Rule, ours: &Ours, theirs: &Theirs) -> Vec<String> {
    let measured = match theirs {
        Theirs::Measured(measured) => measured,
        Theirs::Failed(why) => return vec![format!("FreeCAD failed: {why}")],
        Theirs::Lost(why) => return vec![format!("FreeCAD was lost: {why}")],
    };
    let answer = match &ours.answer {
        Ok(answer) => answer,
        Err(errors) => {
            let said: Vec<String> = errors.iter().map(ToString::to_string).collect();
            return vec![format!(
                "Silico refused ({}) what FreeCAD measured",
                said.join("; ")
            )];
        }
    };
    let mut found = Vec::new();
    let length = |what: &str, ours: f64, theirs: Option<f64>| match theirs {
        Some(theirs) if (ours - theirs).abs() <= LENGTH_TOLERANCE => None,
        theirs => Some(format!(
            "Silico's {what} {ours} against FreeCAD's {theirs:?}"
        )),
    };
    match (answer, &case.expected) {
        (Answer::Distance(d), Expected::Distance(_)) => {
            match rule {
                Rule::Centres => {
                    match d.center_distance {
                        Some(centres) => found.extend(length(
                            "center_distance (FreeCAD's MeasureDistance)",
                            centres.mm(),
                            measured.distance,
                        )),
                        None => found.push(format!(
                            "Silico has no center_distance for two circles, FreeCAD {:?}",
                            measured.distance
                        )),
                    }
                    found.extend(length(
                        "distance (FreeCAD's distToShape)",
                        d.distance.mm(),
                        measured.dist_to_shape,
                    ));
                }
                _ => found.extend(length(
                    "distance (FreeCAD's MeasureDistance)",
                    d.distance.mm(),
                    measured.distance,
                )),
            }
            // The centre's distance from the other element: FreeCAD's where it builds one,
            // none on both sides where neither does. A cylinder's axis against a plane or a
            // line is Silico's own (docs/ports/measure.md), which FreeCAD has no measure of:
            // its number is checked by hand only ([`against_expected`]).
            let [first, second] = [&ours.found[0].element, &ours.found[1].element];
            let axis = |x: &Element, y: &Element| x.is_cylinder() && (y.is_plane() || y.is_line());
            match (d.center_to_element, measured.center_to_element) {
                (None, None) => {}
                (Some(_), None) if axis(first, second) || axis(second, first) => {}
                (Some(centre), theirs @ Some(_)) => found.extend(length(
                    "center_to_element (FreeCAD's circleCenterDistance)",
                    centre.mm(),
                    theirs,
                )),
                (centre, theirs) => found.push(format!(
                    "Silico's center_to_element {:?} against FreeCAD's {theirs:?}",
                    centre.map(Length::mm)
                )),
            }
            // Silico's witness, on its elements as FreeCAD has them.
            match measured.witness {
                Some(apart) => {
                    for (k, gap) in apart.iter().enumerate() {
                        if *gap > LENGTH_TOLERANCE {
                            found.push(format!(
                                "Silico's {} is {gap} from elements.{k} as FreeCAD has it",
                                ["from", "to"][k]
                            ));
                        }
                    }
                }
                None => found.push("FreeCAD didn't measure Silico's witness".to_owned()),
            }
        }
        (Answer::Angle(a), Expected::Angle(_)) => {
            let ours = a.angle.degrees();
            let Some(theirs) = measured.angle else {
                return vec!["FreeCAD gave no angle".to_owned()];
            };
            let (said, ours, theirs) = match rule {
                Rule::Supplement if theirs == 0.0 => ("folded", folded(ours), 0.0),
                Rule::Supplement => ("against 180 less", ours, 180.0 - theirs),
                Rule::FromNormal => ("against |90 less|", ours, (90.0 - theirs).abs()),
                Rule::Folded => ("folded", folded(ours), folded(theirs)),
                Rule::Least | Rule::Centres => {
                    return vec![format!("{rule:?} is no rule for an angle")];
                }
            };
            if (ours - theirs).abs() > ANGLE_TOLERANCE {
                found.push(format!(
                    "Silico's angle {}, {said} FreeCAD's {:?}: {ours} against {theirs}",
                    a.angle, measured.angle
                ));
            }
        }
        _ => found.push("Silico's answer isn't the case's measure".to_owned()),
    }
    found
}

/// Every way Silico's answer to `case` differs from the case's values, by hand: each length
/// and point rounded to the nanometre and each angle to the nanodegree, as Silico rounds
/// them, and equal; the vertex an angle names where it is. Silico's witness points must also
/// be its distance apart.
pub fn against_expected(case: &MeasureCase, ours: &Ours) -> Vec<String> {
    let answer = match &ours.answer {
        Ok(answer) => answer,
        Err(errors) => {
            let said: Vec<String> = errors.iter().map(ToString::to_string).collect();
            return vec![format!("Silico refused: {}", said.join("; "))];
        }
    };
    let mm = |v: f64| Length::rounded(v);
    let point = |p: [f64; 3]| [mm(p[0]), mm(p[1]), mm(p[2])];
    let mut found = Vec::new();
    match (answer, &case.expected) {
        (Answer::Distance(d), Expected::Distance(want)) => {
            if Some(d.distance) != mm(want.distance) {
                found.push(format!(
                    "distance {} against the expected {}",
                    d.distance, want.distance
                ));
            }
            for (what, got, want) in [
                ("center_distance", d.center_distance, want.center_distance),
                ("normal_distance", d.normal_distance, want.normal_distance),
                (
                    "center_to_element",
                    d.center_to_element,
                    want.center_to_element,
                ),
            ] {
                if got.map(Some) != want.map(mm) {
                    found.push(format!("{what} {got:?} against the expected {want:?}"));
                }
            }
            if let Some([from, to]) = want.witness
                && (d.from.map(Some), d.to.map(Some)) != (point(from), point(to))
            {
                found.push(format!(
                    "witness {:?} to {:?} against the expected {from:?} to {to:?}",
                    d.from, d.to
                ));
            }
            let gap: f64 = (0..3)
                .map(|k| {
                    let along = d.to[k].mm() - d.from[k].mm();
                    along * along
                })
                .sum::<f64>()
                .sqrt();
            // Each point is rounded to the nanometre: up to √3/2 nm off, each.
            if (gap - d.distance.mm()).abs() > 2e-6 {
                found.push(format!(
                    "its witness is {gap} apart, its distance {}",
                    d.distance
                ));
            }
        }
        (Answer::Angle(a), Expected::Angle(want)) => {
            if Some(a.angle) != Angle::rounded(want.angle) {
                found.push(format!(
                    "angle {} against the expected {}",
                    a.angle, want.angle
                ));
            }
            match (want.vertex, ours.vertex_at) {
                (None, None) => {}
                (Some(want), Some(at)) if point(at) == point(want) => {}
                (want, _) => found.push(format!(
                    "vertex {:?} (at {:?}) against the expected {want:?}",
                    a.vertex.as_ref().map(|v| v.as_str()),
                    ours.vertex_at
                )),
            }
        }
        _ => found.push("Silico's answer isn't the case's measure".to_owned()),
    }
    found
}

/// One case's outcome: the rule it's compared by, Silico's answer, FreeCAD's measure, and
/// every way they differ from each other or Silico's from the case's values.
#[derive(Debug, Clone)]
pub struct Compared {
    pub case: MeasureCase,
    pub rule: Rule,
    pub ours: Ours,
    pub theirs: Theirs,
    pub how: Vec<String>,
}

impl std::fmt::Display for Compared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.case.name, self.how.join("; "))
    }
}

/// Every case of `cases`, Silico's answer compared with FreeCAD's and with its values, for
/// `silico-oracle run` and the tests alike.
pub fn run(cases: &Cases) -> Result<Vec<Compared>, String> {
    let (engine, model) = rebuilt(cases)?;
    let ours = answers(&model, engine.kernel(), &cases.cases)?;
    // Each case's rule, before FreeCAD is asked: a pair no rule compares is the case file's
    // mistake.
    let rules = cases
        .cases
        .iter()
        .zip(&ours)
        .map(|(case, ours)| {
            Rule::of(&case.expected, &ours.found[0], &ours.found[1])
                .map_err(|e| format!("{}: {e}", case.name))
        })
        .collect::<Result<Vec<Rule>, String>>()?;
    let theirs = freecad(&model, &cases.cases, &ours)?;
    if theirs.len() != cases.cases.len() {
        return Err(format!(
            "{} measure cases gave {} outcomes from FreeCAD",
            cases.cases.len(),
            theirs.len()
        ));
    }
    Ok(cases
        .cases
        .iter()
        .zip(rules)
        .zip(ours)
        .zip(theirs)
        .map(|(((case, rule), ours), theirs)| {
            let mut how = compare(case, rule, &ours, &theirs);
            how.extend(
                against_expected(case, &ours)
                    .into_iter()
                    .map(|h| format!("Silico's {h}")),
            );
            Compared {
                case: case.clone(),
                rule,
                ours,
                theirs,
                how,
            }
        })
        .collect())
}
