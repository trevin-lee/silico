//! Sketch-edit cases (M5.10; docs/ports/sketch-edit.md, *The oracle*): a sketch and one edit
//! (`remove`, `unjoin`, `merge`, `split`, or `trim`), applied through Silico's
//! `document::sketch_edit` and through FreeCAD 1.1.3's Sketcher (`delGeometries`, the vertex
//! Delete's `delConstraintOnPoint`, `Coincident` constraints, `split`, and `trim`), each
//! result solved, Silico's through its sketch worker and FreeCAD's by `sketch.solve()`, and
//! compared: the geometry, matched by FreeCAD's geometry IDs (which its edits keep) to
//! Silico's element IDs; which ends are one point; the constraints kept, dropped, and moved,
//! by kind and by what they name; the degrees of freedom; the conflicting and redundant
//! constraints; where the solved points are; and whether each side refuses.
//!
//! The port note is the spec for where they differ. Where it says Silico deliberately
//! departs from FreeCAD, the comparison is made modulo that departure, and every report
//! names the note's lines it was made modulo ([`NOTE`]): FreeCAD's result is brought to the
//! note's form after its edit (`oracle.py`'s `sketch_edit`, which lists the lines it
//! applied), or a refusal the note gives Silico is excused ([`Verdict::excused`]). Anything
//! the note doesn't explain is a difference.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;
use std::path::Path;

use serde::{Deserialize, Serialize};
use silico_core::document::sketch_edit::{self, Edited};
use silico_core::document::{
    Checked, Document, ElementId, Form, Operation, Reference, Sketch, read, validate,
};
use silico_core::sketch::{SketchSolver, Translation};
use silico_protocol::{LENGTH_TOLERANCE, arc_from_points};

use crate::freecad;
use crate::sketch::{self, FreecadGeometry, FreecadSketch, FreecadSolved, Ours, SketchCase};

/// The directory of sketch-edit cases in an oracle directory.
pub const DIR: &str = "sketch-edits";
/// Its document, each sketch feature a sketch the cases edit.
pub const MODEL: &str = "sketches.silico";
/// Its cases, a JSON list of [`CaseEntry`]s.
pub const CASES: &str = "cases.json";

/// The port note whose lines the reports cite.
pub const NOTE: &str = "docs/ports/sketch-edit.md";

/// How far apart the two sides' stored values (a dimension's value, in mm or radians) may
/// be: both are the value the case wrote, carried by each side's edit.
pub const VALUE_TOLERANCE: f64 = 1e-9;

/// One edit, as Silico's command takes it: members by ID, positions in mm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    Remove { members: Vec<String> },
    Unjoin { point: String, curve: String },
    Merge { points: Vec<String> },
    Split { curve: String, at: Vec<[f64; 2]> },
    Trim { curve: String, pick: [f64; 2] },
}

/// A case as its file writes it: the sketch feature it edits, by name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseEntry {
    pub name: String,
    pub note: String,
    pub sketch: String,
    pub edit: Edit,
}

/// One case: a document, the sketch feature edited (by index), and the edit.
#[derive(Debug, Clone)]
pub struct EditCase {
    pub name: String,
    pub note: String,
    pub document: Document,
    pub feature: usize,
    pub edit: Edit,
}

impl EditCase {
    /// The sketch the case edits.
    pub fn sketch(&self) -> Result<&Sketch, String> {
        match self
            .document
            .features
            .get(self.feature)
            .map(|f| &f.operation)
        {
            Some(Operation::Sketch(sketch)) => Ok(sketch),
            _ => Err(format!("{}: its feature isn't a sketch", self.name)),
        }
    }
}

/// The cases in `dir`'s [`DIR`]; none if it has none. Each names a sketch of the document,
/// and has a name of its own.
pub fn load(dir: &Path) -> Result<Vec<EditCase>, String> {
    let dir = dir.join(DIR);
    let model = dir.join(MODEL);
    let bytes = match std::fs::read(&model) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("can't read {}: {e}", model.display())),
    };
    let at = |e: String| format!("{}: {e}", model.display());
    let document = read(&bytes).map_err(|e| at(e.to_string()))?;
    let checked = Checked::new(document).map_err(|invalid| {
        let errors: Vec<String> = invalid.errors().iter().map(ToString::to_string).collect();
        at(errors.join("; "))
    })?;
    let document = checked.document().clone();
    let path = dir.join(CASES);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let entries: Vec<CaseEntry> = serde_json::from_str(&text)
        .map_err(|e| format!("{} isn't a list of sketch-edit cases: {e}", path.display()))?;
    if entries.is_empty() {
        return Err(format!("{} has no cases", path.display()));
    }
    let mut names = BTreeSet::new();
    let mut cases = Vec::with_capacity(entries.len());
    for entry in entries {
        if !names.insert(entry.name.clone()) {
            return Err(format!(
                "two sketch-edit cases are named {} ({})",
                entry.name,
                path.display()
            ));
        }
        let feature = document
            .features
            .iter()
            .position(|f| f.name.as_str() == entry.sketch)
            .ok_or_else(|| {
                format!(
                    "case {} edits sketch {}, which {} hasn't",
                    entry.name,
                    entry.sketch,
                    model.display()
                )
            })?;
        let case = EditCase {
            name: entry.name,
            note: entry.note,
            document: document.clone(),
            feature,
            edit: entry.edit,
        };
        case.sketch()?;
        cases.push(case);
    }
    Ok(cases)
}

/// Writes `case` to `dir` (made if it's missing) as `<name>.json`, for review: the sketch
/// feature as the document writes it, the edit (a split's positions, a trim's pick), and
/// what Silico's edit makes of it: its edited sketch feature, what it added, changed, and
/// removed, and each line's and arc's length in it (mm); or its refusal.
pub fn dump(case: &EditCase, dir: &Path) -> Result<std::path::PathBuf, String> {
    let at = |e: String| format!("{}: {e}", case.name);
    let feature = case
        .document
        .features
        .get(case.feature)
        .ok_or_else(|| at("its feature is missing".to_owned()))?;
    let as_json = |f: &silico_core::document::Feature| -> Result<serde_json::Value, String> {
        let text = silico_core::document::write_feature(f).map_err(|e| at(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| at(e.to_string()))
    };
    let sketch = case.sketch()?;
    let silico = match silico_edit(sketch, &case.edit).map_err(at)? {
        Err(refusal) => serde_json::json!({ "refused": refusal.text }),
        Ok(edited) => {
            let mut changed = feature.clone();
            changed.operation = Operation::Sketch(edited.sketch.clone());
            let shape = Shape::of(&edited.sketch);
            let place = |id: &ElementId| shape.points.get(id.as_str()).copied();
            let mut lengths = BTreeMap::new();
            for l in &edited.sketch.lines {
                if let (Some(a), Some(b)) = (place(&l.from), place(&l.to)) {
                    lengths.insert(l.id.to_string(), libm::hypot(b[0] - a[0], b[1] - a[1]));
                }
            }
            for a in &edited.sketch.arcs {
                if let (Some(c), Some(s), Some(e)) =
                    (place(&a.center), place(&a.start), place(&a.end))
                    && let Some([r, from, to]) = arc_from_points(c, s, e)
                {
                    lengths.insert(a.id.to_string(), r * (to - from));
                }
            }
            let ids = |list: &[ElementId]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
            serde_json::json!({
                "edited": as_json(&changed)?,
                "added": ids(&edited.added),
                "modified": ids(&edited.modified),
                "removed": ids(&edited.removed),
                "lengths": lengths,
            })
        }
    };
    let value = serde_json::json!({
        "case": case.name,
        "sketch": as_json(feature)?,
        "edit": case.edit,
        "silico": silico,
    });
    std::fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.json", case.name));
    let text = serde_json::to_string_pretty(&value).map_err(|e| at(e.to_string()))?;
    std::fs::write(&path, text + "\n")
        .map_err(|e| format!("can't write {}: {e}", path.display()))?;
    Ok(path)
}

/// A refusal of Silico's edit, as its error gives it: what the excusals check against the
/// note's rule (code, the elements it names, the request's field), and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: String,
    pub message: String,
    pub elements: Vec<String>,
    pub parameter: Option<String>,
    pub text: String,
}

impl Refusal {
    pub fn of(e: &silico_core::Error) -> Refusal {
        Refusal {
            code: e.code().as_str().to_owned(),
            message: e.message().to_owned(),
            elements: e.elements().to_vec(),
            parameter: e.parameter().map(str::to_owned),
            text: e.to_string(),
        }
    }
}

/// A refusal written by hand (tests): no code, elements, or field, so it matches no rule.
impl From<&str> for Refusal {
    fn from(text: &str) -> Refusal {
        Refusal {
            code: String::new(),
            message: text.to_owned(),
            elements: Vec::new(),
            parameter: None,
            text: text.to_owned(),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// What Silico made of a case.
#[derive(Debug, Clone)]
pub enum SilicoSide {
    /// The edit refused, as its error says.
    Refused(Refusal),
    /// The edit's result, which the validation gate refused.
    Gated(Vec<String>),
    /// The edit's result, through the gate, and its solve.
    Edited(Box<SilicoEdited>),
}

/// Silico's edited sketch, in FreeCAD's model too, and its solve.
#[derive(Debug, Clone)]
pub struct SilicoEdited {
    pub edited: Edited,
    pub model: FreecadSketch,
    pub ours: Ours,
}

/// `n` IDs for new members that `sketch` doesn't hold.
fn fresh(sketch: &Sketch, n: usize) -> Vec<ElementId> {
    let held: BTreeSet<&ElementId> = sketch.elements().map(|(id, _)| id).collect();
    (0..)
        .filter_map(|i: u32| ElementId::new(&format!("w{i:07}")))
        .filter(|id| !held.contains(id))
        .take(n)
        .collect()
}

fn element_id(text: &str) -> Result<ElementId, String> {
    ElementId::new(text).ok_or_else(|| format!("{text:?} isn't an element ID"))
}

/// The most new IDs an edit is offered: a split takes at most 4, a trim fewer than this
/// unless a line carries a dozen constraints to copy.
const MOST_IDS: usize = 64;

/// `edit` applied to `sketch` by Silico's `sketch_edit`, or its refusal. A split or trim is
/// given as many fresh IDs as it asks for, as its own tests give them.
pub fn silico_edit(sketch: &Sketch, edit: &Edit) -> Result<Result<Edited, Refusal>, String> {
    let ids = |list: &[String]| {
        list.iter()
            .map(|s| element_id(s))
            .collect::<Result<Vec<_>, _>>()
    };
    let refused = |e: silico_core::Error| Refusal::of(&e);
    Ok(match edit {
        // A removal's first problem, which the rules check; its text gives them all.
        Edit::Remove { members } => sketch_edit::remove(sketch, &ids(members)?).map_err(|r| {
            let text = r
                .errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            match r.errors.first() {
                Some(first) => Refusal {
                    text,
                    ..Refusal::of(first)
                },
                None => Refusal::from(text.as_str()),
            }
        }),
        Edit::Unjoin { point, curve } => {
            let new = fresh(sketch, 1);
            let Some(new) = new.into_iter().next() else {
                return Err("no fresh ID is left".to_owned());
            };
            sketch_edit::unjoin(sketch, &element_id(point)?, &element_id(curve)?, new)
                .map_err(refused)
        }
        Edit::Merge { points } => sketch_edit::merge(sketch, &ids(points)?).map_err(refused),
        Edit::Split { curve, at } => {
            let curve = element_id(curve)?;
            asking(|n| sketch_edit::split(sketch, &curve, at, &fresh(sketch, n)))
                .map(|s| s.edited)
                .map_err(refused)
        }
        Edit::Trim { curve, pick } => {
            let curve = element_id(curve)?;
            asking(|n| sketch_edit::trim(sketch, &curve, *pick, &fresh(sketch, n)))
                .map(|t| t.edited)
                .map_err(refused)
        }
    })
}

/// `edit` with as many new IDs as it asks for: the count it refuses passed over.
fn asking<T>(
    edit: impl Fn(usize) -> Result<T, silico_core::Error>,
) -> Result<T, silico_core::Error> {
    let mut last = None;
    for n in 0..=MOST_IDS {
        match edit(n) {
            Err(e) if e.parameter() == Some("ids") && e.message().contains(" needs ") => {
                last = Some(e);
            }
            other => return other,
        }
    }
    match last {
        Some(e) => Err(e),
        None => edit(0),
    }
}

/// What a sketch is, in FreeCAD's terms: each curve's points by FreeCAD's position (1 start,
/// 2 end, 3 centre), each point no curve uses as geometry of its own, and every point's
/// coordinates.
#[derive(Debug, Clone, Default)]
struct Shape {
    members: BTreeMap<String, Member>,
    points: BTreeMap<String, [f64; 2]>,
    radii: BTreeMap<String, f64>,
}

#[derive(Debug, Clone)]
struct Member {
    kind: &'static str,
    at: BTreeMap<i32, String>,
    construction: bool,
}

impl Shape {
    fn of(sketch: &Sketch) -> Shape {
        let mut shape = Shape::default();
        let id = |e: &ElementId| e.as_str().to_owned();
        for l in &sketch.lines {
            shape.members.insert(
                id(&l.id),
                Member {
                    kind: "line",
                    at: BTreeMap::from([(1, id(&l.from)), (2, id(&l.to))]),
                    construction: l.construction,
                },
            );
        }
        for c in &sketch.circles {
            shape.members.insert(
                id(&c.id),
                Member {
                    kind: "circle",
                    at: BTreeMap::from([(3, id(&c.center))]),
                    construction: c.construction,
                },
            );
            shape.radii.insert(id(&c.id), c.radius.mm());
        }
        for a in &sketch.arcs {
            shape.members.insert(
                id(&a.id),
                Member {
                    kind: "arc",
                    at: BTreeMap::from([(1, id(&a.start)), (2, id(&a.end)), (3, id(&a.center))]),
                    construction: a.construction,
                },
            );
        }
        let used: BTreeSet<String> = shape
            .members
            .values()
            .flat_map(|m| m.at.values().cloned())
            .collect();
        for p in &sketch.points {
            shape.points.insert(id(&p.id), [p.x.mm(), p.y.mm()]);
            if !used.contains(p.id.as_str()) {
                shape.members.insert(
                    id(&p.id),
                    Member {
                        kind: "point",
                        at: BTreeMap::from([(1, id(&p.id))]),
                        construction: p.construction,
                    },
                );
            }
        }
        // An arc's radius, from its centre to its start, as FreeCAD's model takes it.
        for a in &sketch.arcs {
            if let (Some(c), Some(s)) = (
                shape.points.get(a.center.as_str()),
                shape.points.get(a.start.as_str()),
            ) {
                shape
                    .radii
                    .insert(id(&a.id), libm::hypot(s[0] - c[0], s[1] - c[1]));
            }
        }
        shape
    }

    /// The point at `member`'s position `pos`.
    fn point(&self, member: &str, pos: i32) -> Option<&String> {
        self.members.get(member).and_then(|m| m.at.get(&pos))
    }

    /// The curves (not points) that use `point`.
    fn users(&self, point: &str) -> Vec<&String> {
        self.members
            .iter()
            .filter(|(_, m)| m.kind != "point" && m.at.values().any(|p| p == point))
            .map(|(id, _)| id)
            .collect()
    }
}

/// A FreeCAD sketch to build, the edit to make on it, and what it's made with.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Job {
    pub geometry: Vec<FreecadGeometry>,
    pub construction: Vec<bool>,
    pub constraints: Vec<JobConstraint>,
    pub edit: FreecadEdit,
}

/// A constraint of a [`Job`]: `Sketcher.Constraint(kind, *ints, value)`, named `s_` and its
/// Silico ID (FreeCAD's names follow constraints its edits copy), none for a coincidence.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobConstraint {
    pub kind: &'static str,
    pub ints: Vec<i32>,
    pub value: Option<f64>,
    pub driving: bool,
    pub name: Option<String>,
}

/// A FreeCAD edit, by geometry index (from 0) and position.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FreecadEdit {
    /// `delConstraints` of those named, then `delGeometries`.
    Delete {
        geometries: Vec<i32>,
        constraints: Vec<String>,
    },
    /// The vertex Delete: `delConstraintOnPoint(geometry, position)`.
    Unjoin { geometry: i32, position: i32 },
    /// The Gui's Delete on a curve's end (:105-107), `delConstraintOnPoint(geometry,
    /// position)` as it is, none of the note's forms for an unjoin applied.
    VertexDelete { geometry: i32, position: i32 },
    /// A `Coincident` between each pair.
    Merge { coincidences: Vec<[i32; 4]> },
    /// `split(geometry, at)`, a circle's at each of two positions.
    Split { geometry: i32, at: Vec<[f64; 2]> },
    /// `trim(geometry, pick)`.
    Trim { geometry: i32, pick: [f64; 2] },
}

/// What FreeCAD is asked for a case: a job, or nothing, since FreeCAD refuses it before any
/// call (its counterpart is a Gui command's refusal, or there is none), as the note says.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Run { job: Job, modulo: Vec<u32> },
    Refuses(String),
}

/// How far inside an arc's sweep, in radians, FreeCAD is given a pick that was outside it.
const INSIDE: f64 = 1e-6;

/// For an arc `curve` picked at `pick` outside its sweep: the point Silico takes it at, its
/// end nearer in angle, moved [`INSIDE`] into the sweep; none for a pick inside, or another
/// curve.
fn inside_sweep(sketch: &Sketch, curve: &str, pick: [f64; 2]) -> Option<[f64; 2]> {
    let arc = sketch.arcs.iter().find(|a| a.id.as_str() == curve)?;
    let shape = Shape::of(sketch);
    let at = |id: &ElementId| shape.points.get(id.as_str()).copied();
    let center = at(&arc.center)?;
    let [r, from, to] = arc_from_points(center, at(&arc.start)?, at(&arc.end)?)?;
    let mut angle = libm::atan2(pick[1] - center[1], pick[0] - center[0]);
    while angle < from {
        angle += TAU;
    }
    if angle <= to || pick == center {
        return None;
    }
    // As Silico's trim places it: the end nearer in angle.
    let placed = if from + TAU - angle < angle - to {
        from + INSIDE
    } else {
        to - INSIDE
    };
    Some([
        center[0] + r * libm::cos(placed),
        center[1] + r * libm::sin(placed),
    ])
}

/// The name a FreeCAD constraint carries for Silico's constraint `id`.
fn fc_name(id: &str) -> String {
    format!("s_{id}")
}

/// The positions in a translated constraint's `ints` that are points (a geometry and a
/// position), by its Silico form: those a shared point's constraint may sit on any curve's
/// end for (a `midpoint`'s line ends and a tangency's ends are the curve's own).
fn point_slots(form: &Form, len: usize) -> Vec<usize> {
    match form {
        Form::HorizontalPoints { .. }
        | Form::VerticalPoints { .. }
        | Form::Distance { .. }
        | Form::DistanceX { .. }
        | Form::DistanceY { .. } => vec![0, 2],
        Form::PointLineDistance { .. } | Form::On { .. } => vec![0],
        Form::Symmetric { .. } if len == 6 => vec![0, 2, 4],
        Form::Symmetric { .. } => vec![0, 2],
        Form::Midpoint { .. } => vec![4],
        _ => Vec::new(),
    }
}

/// FreeCAD's job for `edit` of `sketch`, whose FreeCAD model is `model`, or FreeCAD's
/// refusal.
pub fn plan(sketch: &Sketch, model: &FreecadSketch, edit: &Edit) -> Plan {
    let shape = Shape::of(sketch);
    let index = |id: &str| model.elements.iter().position(|e| e == id);
    let as_i32 = |i: usize| i32::try_from(i).unwrap_or(i32::MAX);
    let mut modulo = Vec::new();
    // The curves the edit takes a point from: a shared point's constraints sit, in FreeCAD's
    // model, on the end of a curve that keeps it, as Silico's stay with the point
    // (:95, :148, :213, :237-238); FreeCAD's own rules then have nothing to move.
    let (away, line): (BTreeSet<&str>, u32) = match edit {
        Edit::Remove { members } => (members.iter().map(String::as_str).collect(), 95),
        Edit::Unjoin { curve, .. } => (BTreeSet::from([curve.as_str()]), 237),
        Edit::Trim { curve, .. } => (BTreeSet::from([curve.as_str()]), 213),
        Edit::Split { curve, .. } => (BTreeSet::from([curve.as_str()]), 148),
        Edit::Merge { .. } => (BTreeSet::new(), 0),
    };
    let rehub = matches!(edit, Edit::Trim { .. });
    let (constraints, moved) = job_constraints(sketch, model, &away, rehub);
    if moved {
        modulo.push(line);
    }
    let edit = match edit {
        Edit::Remove { members } => {
            let mut geometries = Vec::new();
            let mut named = Vec::new();
            let removed: BTreeSet<&str> = members.iter().map(String::as_str).collect();
            for member in members {
                if sketch.constraints.iter().any(|c| c.id.as_str() == member) {
                    named.push(fc_name(member));
                } else if let Some(i) = index(member) {
                    geometries.push(as_i32(i));
                } else if shape.points.contains_key(member) {
                    let keeping: Vec<&String> = shape
                        .users(member)
                        .into_iter()
                        .filter(|c| !removed.contains(c.as_str()))
                        .collect();
                    if !keeping.is_empty() {
                        // FreeCAD has no edit that deletes a point curves end at (:109): its
                        // Delete on that vertex, one curve's end, unjoins it (:105-107).
                        return match model.places.get(member).and_then(|ps| ps.first()) {
                            Some(&(geometry, position)) => Plan::Run {
                                job: Job {
                                    geometry: model.geometry.clone(),
                                    construction: model
                                        .elements
                                        .iter()
                                        .map(|e| {
                                            shape.members.get(e).is_some_and(|m| m.construction)
                                        })
                                        .collect(),
                                    constraints,
                                    edit: FreecadEdit::VertexDelete { geometry, position },
                                },
                                modulo,
                            },
                            None => Plan::Refuses(format!("FreeCAD has no point {member}")),
                        };
                    }
                } else {
                    return Plan::Refuses(format!("FreeCAD has no member {member}"));
                }
            }
            geometries.sort_unstable();
            geometries.dedup();
            // Coincidences listed first, so 1.1.3's delete doesn't depend on their order
            // (:91): Silico's constraints have none.
            modulo.push(91);
            FreecadEdit::Delete {
                geometries,
                constraints: named,
            }
        }
        Edit::Unjoin { point, curve } => {
            let place = index(curve).and_then(|g| {
                model
                    .places
                    .get(point)
                    .and_then(|places| places.iter().find(|(pg, _)| *pg == as_i32(g)).copied())
            });
            match place {
                Some((geometry, position)) => FreecadEdit::Unjoin { geometry, position },
                None => {
                    return Plan::Refuses(format!(
                        "curve {curve} doesn't end at or centre on point {point}: there's no vertex to delete"
                    ));
                }
            }
        }
        Edit::Merge { points } => {
            // A point named twice is given to FreeCAD as named: a coincidence of a place with
            // itself, which its Coincident command refuses as already coincident (:51).
            let first = |p: &String| model.places.get(p).and_then(|ps| ps.first().copied());
            let Some((kept, others)) = points.split_first() else {
                return Plan::Refuses("a merge of no points".to_owned());
            };
            let Some((kg, kp)) = first(kept) else {
                return Plan::Refuses(format!("FreeCAD has no point {kept}"));
            };
            let mut coincidences = Vec::new();
            for other in others {
                let Some((og, op)) = first(other) else {
                    return Plan::Refuses(format!("FreeCAD has no point {other}"));
                };
                coincidences.push([kg, kp, og, op]);
            }
            FreecadEdit::Merge { coincidences }
        }
        Edit::Split { curve, at } => match index(curve) {
            Some(g) => FreecadEdit::Split {
                geometry: as_i32(g),
                at: at.clone(),
            },
            None => return Plan::Refuses(format!("FreeCAD has no geometry {curve}")),
        },
        Edit::Trim { curve, pick } => match index(curve) {
            Some(g) => FreecadEdit::Trim {
                geometry: as_i32(g),
                pick: match inside_sweep(sketch, curve, *pick) {
                    // An arc's pick outside its sweep: Silico places it at the end nearer in
                    // angle, as its projection is, where 1.1.3 removes a middle piece
                    // (:221); FreeCAD is given it there, a hair inside the sweep.
                    Some(inside) => {
                        modulo.push(221);
                        inside
                    }
                    None => *pick,
                },
            },
            None => return Plan::Refuses(format!("FreeCAD has no geometry {curve}")),
        },
    };
    let construction = model
        .elements
        .iter()
        .map(|e| shape.members.get(e).is_some_and(|m| m.construction))
        .collect();
    Plan::Run {
        job: Job {
            geometry: model.geometry.clone(),
            construction,
            constraints,
            edit,
        },
        modulo,
    }
}

/// `model`'s constraints for a job: its coincidences first (FreeCAD's delete then takes the
/// replacement and the transfer target from the same coincidence, :91), then Silico's,
/// named, each point that sits on the end of a curve in `away` moved to the end of another
/// curve that uses the point, where one does; and, if `rehub` (a trim, which has no
/// repair for it, :184), the coincidences of a point whose hub (the end every other is
/// joined to) is such an end joined to another end instead, so a trim that takes the hub's
/// end away doesn't part the curves that keep the point (:213). And whether any moved.
fn job_constraints(
    sketch: &Sketch,
    model: &FreecadSketch,
    away: &BTreeSet<&str>,
    rehub: bool,
) -> (Vec<JobConstraint>, bool) {
    let forms: BTreeMap<&str, &Form> = sketch
        .constraints
        .iter()
        .map(|c| (c.id.as_str(), &c.form))
        .collect();
    let element = |g: i32| {
        usize::try_from(g)
            .ok()
            .and_then(|g| model.elements.get(g))
            .map(String::as_str)
    };
    let mut moved = false;
    let mut silicos = Vec::new();
    let mut coincidences = Vec::new();
    for c in &model.constraints {
        let mut ints = c.ints.clone();
        if let Some(id) = &c.id
            && let Some(form) = forms.get(id.as_str())
        {
            for slot in point_slots(form, ints.len()) {
                let (Some(&g), Some(&pos)) = (ints.get(slot), ints.get(slot + 1)) else {
                    continue;
                };
                if !element(g).is_some_and(|e| away.contains(e)) {
                    continue;
                }
                let elsewhere = model
                    .places
                    .values()
                    .find(|places| places.contains(&(g, pos)))
                    .and_then(|places| {
                        places
                            .iter()
                            .find(|(pg, _)| !element(*pg).is_some_and(|e| away.contains(e)))
                    });
                if let Some(&(ng, npos)) = elsewhere {
                    ints[slot] = ng;
                    ints[slot + 1] = npos;
                    moved = true;
                }
            }
        }
        if rehub
            && c.id.is_none()
            && let [g0, p0, g, p] = ints[..]
            && element(g0).is_some_and(|e| away.contains(e))
            && let Some(hub) = model
                .places
                .values()
                .find(|places| places.first() == Some(&(g0, p0)))
                .and_then(|places| {
                    places
                        .iter()
                        .find(|(pg, _)| !element(*pg).is_some_and(|e| away.contains(e)))
                })
        {
            ints = if (g, p) == *hub {
                vec![hub.0, hub.1, g0, p0]
            } else {
                vec![hub.0, hub.1, g, p]
            };
            moved = true;
        }
        let made = JobConstraint {
            kind: c.kind,
            ints,
            value: c.value,
            driving: c.driving,
            name: c.id.as_deref().map(fc_name),
        };
        if c.id.is_some() {
            silicos.push(made);
        } else {
            coincidences.push(made);
        }
    }
    coincidences.extend(silicos);
    (coincidences, moved)
}

/// FreeCAD's sketch after an edit, as `oracle.py` reports it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreecadEdited {
    /// The solve of the sketch as built, before the edit.
    pub before: Diagnosis,
    /// Each geometry's ID before the edit, by index.
    pub ids_before: Vec<i64>,
    /// The geometry after the edit (and the note's forms), before the last solve.
    pub edited: Vec<FcGeometry>,
    /// The geometry after the last solve.
    pub geometry: Vec<FcGeometry>,
    pub constraints: Vec<FcConstraint>,
    /// The last solve.
    pub after: Diagnosis,
    /// The note's lines whose forms were applied to FreeCAD's result.
    pub applied: Vec<u32>,
}

/// A solve's result: `solve()`'s code, the degrees of freedom, and constraint indices from 1.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnosis {
    pub code: i32,
    pub dof: i32,
    pub conflicting: Vec<usize>,
    pub redundant: Vec<usize>,
    pub partially_redundant: Vec<usize>,
    pub malformed: Vec<usize>,
}

/// A piece of FreeCAD geometry: its ID, kind, points by position, and radius.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FcGeometry {
    pub id: i64,
    pub kind: String,
    pub points: BTreeMap<String, [f64; 2]>,
    pub radius: Option<f64>,
    pub construction: bool,
}

/// A FreeCAD constraint: its kind, references (a geometry and a position, 0 for a whole
/// curve), value, and name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FcConstraint {
    pub kind: String,
    pub refs: Vec<[i32; 2]>,
    pub value: f64,
    pub driving: bool,
    pub name: String,
}

/// What FreeCAD made of a case.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theirs {
    Edited(Box<FreecadEdited>),
    /// Its edit (FreeCAD's own call) raised, as it says.
    Failed(String),
    /// It crashed or hung: its own outcome, never its refusal.
    Lost(String),
    /// The harness's own code around FreeCAD's edit raised: a bug of the oracle's, never
    /// FreeCAD's refusal.
    Harness(String),
}

/// How a case came out: what differs (empty when the two agree modulo the note), what the
/// note excuses, and the note's lines it was compared modulo.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Verdict {
    pub case: String,
    pub how: Vec<String>,
    pub excused: Vec<String>,
    pub modulo: Vec<u32>,
    /// Both sides refused it.
    pub refused: bool,
    /// FreeCAD crashed where the note says it does (splitting a point, :115), and Silico
    /// refused it by the note's rule: a crash is FreeCAD's own outcome, never its refusal.
    pub crashed: bool,
    /// The note's rule Silico's refusal is, if it refused by one.
    pub rule: Option<Rule>,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.case, self.how.join("; "))?;
        if !self.modulo.is_empty() {
            write!(f, " (compared modulo {NOTE}{})", cited(&self.modulo))?;
        }
        Ok(())
    }
}

/// A case of `tests/oracle/sketch-edits` whose difference from FreeCAD the port note doesn't
/// explain, kept for a decision (a fix to the edit, or a line in the note): shown as known,
/// not as agreement, by its whole report, so any other difference in the case still shows;
/// and one that no longer shows is a problem, so the list can't go stale. Or a case FreeCAD
/// answers otherwise run to run, which it's no oracle for.
#[derive(Debug, Clone, PartialEq)]
pub struct Known {
    pub case: &'static str,
    pub shows: Shows,
    pub why: &'static str,
}

/// What a [`Known`] case shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Shows {
    /// This report ([`Verdict::how`], joined by "; "), exactly.
    Difference(&'static str),
    /// Whatever it shows, where Silico's side is still this rule's refusal: FreeCAD 1.1.3
    /// doesn't answer the case the same every run, so its answer is no oracle, and the case
    /// is shown as known, its report with it, never as an agreement or a difference. Silico's
    /// side otherwise is compared, and the known case a problem.
    Varies(Rule),
}

/// The case file's differences the note doesn't explain, each reported with its judgment;
/// and the sweep's, by the name a seed gives them, where one is pending a decision.
pub const KNOWN: &[Known] = &[
    Known {
        case: "merge_holds_own_point_on_line",
        shows: Shows::Difference(
            "both refused, but Silico's refusal isn't one the note gives (its edit refused: invalid_input: merging them would make constraint k0000005 hold p0000004, a point of n0000003, on n0000003 itself (elements k0000005, p0000004, n0000003, parameter points). Suggestion: remove the constraint first, or merge other points.); FreeCAD's: FreeCAD's Coincident command rolls back a worse solve (docs/ports/sketch-edit.md:53): solve -2 with conflicting [], redundant [1], malformed []",
        ),
        why: "pending decision 13b (#222), not a fix: Silico's merge refuses one after which a constraint would hold a curve's own point on that curve (#239's own-curve refusal, the gate's rule, :31, applied to the edit's result up front, :34), which the note gives only generally, as the gate's (:31, :276); FreeCAD 1.1.3 makes the merge, its solve finding the `on` redundant, which main's Coincident command (emulated) rolls back. Whether the merge refuses it or drops the `on` as implied is decision 13b of docs/design/m5.0-app.md (#222, not on this base), which the maintainer hasn't answered",
    },
    // `sketch-edit-sweep --seed 32 --count 500`'s case 490.
    Known {
        case: "edit_32_490",
        shows: Shows::Difference(
            "both refused, but Silico's refusal isn't one the note gives (its edit refused: invalid_input: merging them would make constraint k0000028 hold p0000027, a point of n0000005, on n0000005 itself (elements k0000028, p0000027, n0000005, parameter points). Suggestion: remove the constraint first, or merge other points.); FreeCAD's: FreeCAD's Coincident command rolls back a worse solve (docs/ports/sketch-edit.md:53): solve -2 with conflicting [], redundant [12], malformed []",
        ),
        why: "pending decision 13b (#222), not a fix: Silico's merge refuses one after which a constraint would hold a curve's own point on that curve (#239's own-curve refusal, the gate's rule, :31, applied to the edit's result up front, :34), which the note gives only generally, as the gate's (:31, :276); FreeCAD 1.1.3 makes the merge, its solve finding the `on` redundant, which main's Coincident command (emulated) rolls back. Whether the merge refuses it or drops the `on` as implied is decision 13b of docs/design/m5.0-app.md (#222, not on this base), which the maintainer hasn't answered",
    },
    // `sketch-edit-sweep --seed 32 --count 500`'s case 413: a decision-13b merge, as case 490
    // is; first seen in sweep 40851790630, after #251's rebase.
    Known {
        case: "edit_32_413",
        shows: Shows::Difference(
            "both refused, but Silico's refusal isn't one the note gives (its edit refused: invalid_input: merging them would make constraint k0000022 hold p0000001, a point of n0000005, on n0000005 itself (elements k0000022, p0000001, n0000005, parameter points). Suggestion: remove the constraint first, or merge other points.); FreeCAD's: FreeCAD's Coincident command rolls back a worse solve (docs/ports/sketch-edit.md:53): solve -2 with conflicting [], redundant [13], malformed []",
        ),
        why: "pending decision 13b (#222), not a fix: Silico's merge refuses one after which a constraint would hold a curve's own point on that curve (#239's own-curve refusal, the gate's rule, :31, applied to the edit's result up front, :34), which the note gives only generally, as the gate's (:31, :276); FreeCAD 1.1.3 makes the merge, its solve finding the `on` redundant, which main's Coincident command (emulated) rolls back. Whether the merge refuses it or drops the `on` as implied is decision 13b of docs/design/m5.0-app.md (#222, not on this base), which the maintainer hasn't answered",
    },
    Known {
        case: "merge_two_locked_ends",
        shows: Shows::Difference(
            "FreeCAD refused (FreeCAD's Coincident command rolls back a worse solve (docs/ports/sketch-edit.md:53): solve -2 with conflicting [], redundant [3, 4], malformed []); Silico made the edit",
        ),
        why: "the two locks agree once the points are one, so Silico's solve keeps them as redundant (M3.3's decision 5) and the merge stands (as the note's own merge tests have a lock merge, :452); FreeCAD's Coincident command rolls back a solve that finds new redundancy (:53); the note says only that a conflict refuses the merge (:277)",
    },
    // `sketch-edit-sweep --seed 32 --count 500`'s case 154.
    Known {
        case: "edit_32_154",
        shows: Shows::Varies(Rule::MergeSolveUnbuildable),
        why: "FreeCAD 1.1.3 isn't the same run to run here (:278): points merged from apart, whose solve shortens a line to nothing, which Silico refuses (:280); FreeCAD's solve mostly succeeds, its line 5e-13 mm long, and now and then (about 4 of 36 runs) fails (-1), which its Coincident command rolls back; alone in a script, it failed 3 of 20 runs and its 17 solves landed in three different places. Either outcome is the note's, but an answer that varies is no oracle",
    },
];

/// `verdicts` less the [`KNOWN`] differences that showed: the cases that differ, a line for
/// each known one that showed, and a problem for each known one of these cases that didn't.
pub fn without_known(
    verdicts: Vec<Verdict>,
    known: &[Known],
) -> (Vec<Verdict>, Vec<String>, Vec<String>) {
    let (mut differ, mut shown, mut missing) = (Vec::new(), Vec::new(), Vec::new());
    for k in known {
        let Some(v) = verdicts.iter().find(|v| v.case == k.case) else {
            continue;
        };
        let difference = match &k.shows {
            Shows::Difference(difference) => *difference,
            Shows::Varies(rule) if v.rule.as_ref() == Some(rule) => continue,
            Shows::Varies(rule) => {
                missing.push(format!(
                    "{}: FreeCAD varies run to run here, but Silico's side is no longer the note's rule {rule:?} ({}); it now reads: {}",
                    k.case,
                    k.why,
                    if v.how.is_empty() {
                        "no difference".to_owned()
                    } else {
                        v.how.join("; ")
                    }
                ));
                continue;
            }
        };
        if v.how.join("; ") != difference {
            missing.push(format!(
                "{}: its known difference didn't show as it's written ({}); it now reads: {}",
                k.case,
                k.why,
                if v.how.is_empty() {
                    "no difference".to_owned()
                } else {
                    v.how.join("; ")
                }
            ));
        }
    }
    for v in verdicts {
        match known.iter().find(|k| k.case == v.case) {
            Some(k) if matches!(&k.shows, Shows::Varies(rule) if v.rule.as_ref() == Some(rule)) => {
                let report = if v.how.is_empty() {
                    "agreed this run".to_owned()
                } else {
                    v.how.join("; ")
                };
                shown.push(format!(
                    "known: {}: FreeCAD's answer varies run to run, so it isn't compared ({report}) -- {}",
                    v.case, k.why
                ));
            }
            Some(k) if matches!(k.shows, Shows::Difference(d) if d == v.how.join("; ")) => {
                shown.push(format!("known: {v} -- {}", k.why));
            }
            Some(_) if v.how.is_empty() => {}
            _ if v.how.is_empty() => {}
            _ => differ.push(v),
        }
    }
    (differ, shown, missing)
}

/// A case prepared: Silico's side done, FreeCAD's planned.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub name: String,
    pub sketch: Sketch,
    pub edit: Edit,
    pub model: FreecadSketch,
    pub silico: SilicoSide,
    pub plan: Plan,
    /// What's wrong with the case itself (a sketch not drawn as it solves).
    pub problems: Vec<String>,
}

/// How far a solve may move a case's points before its sketch is taken as not drawn as it
/// solves, which would make the two sides edit different drawings.
pub const DRAWN_TOLERANCE: f64 = 1e-9;

/// Silico's side of `case`, through `worker`, and FreeCAD's plan.
pub fn prepare(case: &EditCase, worker: &mut dyn SketchSolver) -> Result<Prepared, String> {
    let at = |e: String| format!("{}: {e}", case.name);
    let sketch = case.sketch()?.clone();
    let translation = Translation::new(&sketch).map_err(|e| at(e.to_string()))?;
    let model = sketch::freecad_sketch(&sketch, &translation).map_err(at)?;
    let mut problems = Vec::new();
    let solved = |s: &Sketch, worker: &mut dyn SketchSolver| {
        sketch::silico_solve(
            &SketchCase {
                name: case.name.clone(),
                note: String::new(),
                sketch: s.clone(),
            },
            worker,
        )
    };
    match solved(&sketch, worker) {
        Ok(Ours {
            outcome: Ok(solution),
            ..
        }) => {
            let shape = Shape::of(&sketch);
            for (id, [x, y]) in &solution.points {
                if let Some(p) = shape.points.get(id.as_str())
                    && ((p[0] - x.mm()).abs() > DRAWN_TOLERANCE
                        || (p[1] - y.mm()).abs() > DRAWN_TOLERANCE)
                {
                    problems.push(format!(
                        "the case's sketch isn't drawn as it solves: point {id} at {p:?} solves to [{}, {}]",
                        x.mm(),
                        y.mm()
                    ));
                }
            }
        }
        Ok(Ours {
            outcome: Err(e), ..
        }) => {
            problems.push(format!("the case's sketch doesn't solve: {e}"));
        }
        Err(e) => problems.push(format!("the case's sketch can't be solved: {e}")),
    }
    let silico = match silico_edit(&sketch, &case.edit).map_err(at)? {
        Err(refusal) => SilicoSide::Refused(refusal),
        Ok(edited) => silico_side(&case.document, case.feature, edited, worker).map_err(at)?,
    };
    let plan = plan(&sketch, &model, &case.edit);
    Ok(Prepared {
        name: case.name.clone(),
        sketch,
        edit: case.edit.clone(),
        model,
        silico,
        plan,
        problems,
    })
}

/// A job as `oracle.py` reads a case.
#[derive(Serialize)]
struct Sent<'a> {
    sketch_edit: &'a Job,
}

/// Runs `cases` through Silico (`silico-sketch-worker` beside the running executable) and
/// FreeCAD, and gives each case's verdict, in order.
pub fn run(cases: &[EditCase]) -> Result<Vec<Verdict>, String> {
    if cases.is_empty() {
        return Ok(Vec::new());
    }
    // Silico's worker first: a missing one is found before FreeCAD's run, not after.
    let mut worker = silico_core::sketch::SketchWorker::start().map_err(|e| e.to_string())?;
    let mut prepared = Vec::with_capacity(cases.len());
    for case in cases {
        prepared.push(prepare(case, &mut worker)?);
    }
    let theirs = freecad_outcomes(&prepared)?;
    Ok(prepared
        .iter()
        .zip(&theirs)
        .map(|(p, fc)| verdict(p, fc.as_ref()))
        .collect())
}

/// What FreeCAD made of each prepared case, in order, in one run of FreeCAD: none for a
/// case whose plan refuses.
pub fn freecad_outcomes(prepared: &[Prepared]) -> Result<Vec<Option<Theirs>>, String> {
    let jobs: Vec<Sent> = prepared
        .iter()
        .filter_map(|p| match &p.plan {
            Plan::Run { job, .. } => Some(Sent { sketch_edit: job }),
            Plan::Refuses(_) => None,
        })
        .collect();
    let mut theirs = if jobs.is_empty() {
        Vec::new()
    } else {
        freecad::run_as(&jobs, Theirs::Lost)?
    }
    .into_iter();
    prepared
        .iter()
        .map(|p| match &p.plan {
            Plan::Run { .. } => theirs
                .next()
                .map(Some)
                .ok_or_else(|| format!("FreeCAD gave no outcome for {}", p.name)),
            Plan::Refuses(_) => Ok(None),
        })
        .collect()
}

/// Silico's side of `sketch` edited as `edited`: through the gate (as `document`'s feature
/// `feature`), in FreeCAD's model, and solved through `worker`.
pub fn silico_side(
    document: &Document,
    feature: usize,
    edited: Edited,
    worker: &mut dyn SketchSolver,
) -> Result<SilicoSide, String> {
    let mut document = document.clone();
    if let Some(f) = document.features.get_mut(feature) {
        f.operation = Operation::Sketch(edited.sketch.clone());
    }
    if let Err(invalid) = validate(&document) {
        return Ok(SilicoSide::Gated(
            invalid.errors().iter().map(ToString::to_string).collect(),
        ));
    }
    let translation = Translation::new(&edited.sketch).map_err(|e| e.to_string())?;
    let model = sketch::freecad_sketch(&edited.sketch, &translation)?;
    let ours = sketch::silico_solve(
        &SketchCase {
            name: String::new(),
            note: String::new(),
            sketch: edited.sketch.clone(),
        },
        worker,
    )?;
    Ok(SilicoSide::Edited(Box::new(SilicoEdited {
        edited,
        model,
        ours,
    })))
}

/// How `p` came out, FreeCAD's side `theirs` (none when its plan refuses).
pub fn verdict(p: &Prepared, theirs: Option<&Theirs>) -> Verdict {
    let mut v = Verdict {
        case: p.name.clone(),
        how: p.problems.clone(),
        rule: rule(p),
        ..Verdict::default()
    };
    if let Plan::Run { modulo, .. } = &p.plan {
        v.modulo.extend(modulo);
    }
    if let Some(Theirs::Edited(fc)) = theirs {
        v.modulo.extend(&fc.applied);
        // The case's sketch as FreeCAD's model has it, before the edit: what the two edit
        // has to be a sketch FreeCAD reads as Silico does.
        if !fc.before.malformed.is_empty() || !fc.before.conflicting.is_empty() {
            v.how.push(format!(
                "FreeCAD's model of the sketch, before the edit, has malformed constraints {:?} and conflicting {:?}",
                fc.before.malformed, fc.before.conflicting
            ));
        }
    }
    if let Some(Theirs::Harness(why)) = theirs {
        v.how.push(format!(
            "the harness's FreeCAD side failed, not FreeCAD's edit: {why}"
        ));
        return finish(v);
    }
    // A result the gate refuses is Silico's bug, whatever FreeCAD does: every edit's result
    // has to pass it (:34, :153).
    if let SilicoSide::Gated(why) = &p.silico {
        v.how.push(format!(
            "Silico's edit made a sketch the gate refuses, where every edit's result has to pass it ({NOTE}:34, :153): {}",
            why.join("; ")
        ));
        return finish(v);
    }
    // FreeCAD's refusal, if it refused: its edit raised, its plan refuses, or (a merge) its
    // Coincident command would refuse what the edit made. A crash or hang is none of these:
    // it's its own outcome, compared on its own.
    let fc_refusal: Option<String> = match (theirs, &p.plan) {
        (Some(Theirs::Lost(why)), _) => return lost(p, v, why),
        (_, Plan::Refuses(why)) => Some(why.clone()),
        (Some(Theirs::Failed(why)), _) => Some(format!("its edit raised: {why}")),
        (Some(Theirs::Edited(fc)), _) => merge_refusal(p, fc),
        (Some(Theirs::Harness(_)), _) | (None, Plan::Run { .. }) => {
            v.how
                .push("FreeCAD was run on it and gave no outcome".to_owned());
            return finish(v);
        }
    };
    let silico_refusal: Option<String> = match &p.silico {
        SilicoSide::Refused(why) => Some(format!("its edit refused: {why}")),
        SilicoSide::Gated(_) => None,
        SilicoSide::Edited(s) => match (&p.edit, &s.ours.outcome) {
            // A merge's solve settles it (:277).
            (Edit::Merge { .. }, Err(e)) => Some(format!("its merge's solve refused: {e}")),
            _ => None,
        },
    };
    match (silico_refusal, fc_refusal) {
        // Two refusals agree only where Silico's is one the note gives, for its reason, and
        // the note doesn't pin FreeCAD's outcome as another than a refusal.
        (Some(ours), Some(theirs_why)) => match rule(p) {
            Some(r) => {
                // FreeCAD's edit raising with the text the note gives it.
                let raised = |text: &str| {
                    matches!(theirs, Some(Theirs::Failed(why)) if why.contains(text))
                };
                let agrees = match r.pinned() {
                    None => true,
                    // A merge FreeCAD's solve fails, which its Coincident command rolls back:
                    // one of the two outcomes the note gives it (:278).
                    Some(Pinned::SolvesOrRollsBack) => rolled_back(p, theirs),
                    Some(Pinned::Raises(text)) => raised(text),
                    // 1.1.3's vertex Delete raises only on an end with no coincidence (:234).
                    Some(Pinned::UnjoinsVertex) => raised(VERTEX_DELETE_RAISES) && !vertex_shared(p),
                    Some(Pinned::Crashes | Pinned::Edits) => false,
                };
                match r.pinned() {
                    Some(pinned) if !agrees => v.how.push(format!(
                        "both refused, but the note has FreeCAD {} ({NOTE}{}): Silico's refusal ({ours}); FreeCAD's: {theirs_why}",
                        pinned.does(),
                        cited(&r.lines())
                    )),
                    _ => {
                        v.refused = true;
                        v.modulo.extend(r.lines());
                    }
                }
            }
            None => v.how.push(format!(
                "both refused, but Silico's refusal isn't one the note gives ({ours}); FreeCAD's: {theirs_why}"
            )),
        },
        (Some(ours), None) => {
            let Some(Theirs::Edited(fc)) = theirs else {
                v.how
                    .push(format!("Silico refused ({ours}); FreeCAD gave nothing"));
                return finish(v);
            };
            match rule(p).and_then(|r| excused(p, &r, fc)) {
                Some(excuse) => v.excused.push(excuse),
                None => v.how.push(format!(
                    "Silico refused ({ours}); FreeCAD made the edit ({})",
                    summary(fc)
                )),
            }
        }
        (None, Some(theirs_why)) => {
            v.how.push(format!(
                "FreeCAD refused ({theirs_why}); Silico made the edit"
            ));
        }
        (None, None) => {
            if let (SilicoSide::Edited(s), Some(Theirs::Edited(fc))) = (&p.silico, theirs) {
                let band = contact_band(p);
                if let Some(band) = &band {
                    v.excused.extend(band.excuses());
                    v.modulo.push(476);
                }
                let merged = merged_apart(p);
                if merged.is_some() {
                    v.excused.push(format!(
                        "points merged from apart: Silico places them, FreeCAD's coincidences are solved ({NOTE}:270); the merged point, and what each solve moves, aren't compared by place"
                    ));
                    v.modulo.push(270);
                }
                let unsettled = Unsettled {
                    merged: merged.as_ref(),
                    band: band.as_ref(),
                };
                let how = compare_edited(p, s, fc, &unsettled, &mut v.excused, &mut v.modulo);
                v.how.extend(how);
            }
        }
    }
    finish(v)
}

/// How `p` came out where FreeCAD crashed or hung on it (`why`): its own outcome, never its
/// refusal. It agrees only where Silico refuses it by a rule whose edit the note says FreeCAD
/// 1.1.3 crashes on (splitting a point, :115), and FreeCAD crashed as the note says, by a
/// segmentation fault, not otherwise and not by hanging; anything else is a difference.
fn lost(p: &Prepared, mut v: Verdict, why: &str) -> Verdict {
    let crashes = match &p.silico {
        SilicoSide::Refused(_) => rule(p).filter(|r| r.pinned() == Some(Pinned::Crashes)),
        _ => None,
    };
    match crashes {
        Some(r) if why.starts_with("FreeCAD crashed") && why.contains("SIGSEGV") => {
            v.crashed = true;
            v.modulo.extend(r.lines());
        }
        _ => {
            let silico = match &p.silico {
                SilicoSide::Refused(r) => format!("Silico refused it ({r})"),
                SilicoSide::Edited(s) => match (&p.edit, &s.ours.outcome) {
                    (Edit::Merge { .. }, Err(e)) => {
                        format!("Silico's merge's solve refused it ({e})")
                    }
                    _ => "Silico made the edit".to_owned(),
                },
                SilicoSide::Gated(_) => "Silico's result is gated".to_owned(),
            };
            v.how.push(format!(
                "FreeCAD crashed or hung ({why}), its own outcome, never a refusal, which the note gives only for a split of a point ({NOTE}:115); {silico}"
            ));
        }
    }
    finish(v)
}

/// The note's `lines`, as a report cites them after [`NOTE`]: `:115, :153`.
fn cited(lines: &[u32]) -> String {
    let lines: Vec<String> = lines.iter().map(|l| format!(":{l}")).collect();
    lines.join(", ")
}

fn finish(mut v: Verdict) -> Verdict {
    v.modulo.sort_unstable();
    v.modulo.dedup();
    v.modulo.retain(|l| *l != 0);
    v
}

/// A refusal of Silico's that the note gives: which rule, with what it names.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    /// Splitting a point: the gate's kinds (:153), where 1.1.3 segfaults (:115).
    SplitNotCurve,
    /// A circle split at one position (decision 3, :473).
    SplitCircleAtOne,
    /// A split position farther than 1e-7 mm off the curve (decision 4, :474), by how far.
    SplitOffCurve(f64),
    /// A split position at, past, or within 1e-7 mm of an end (:153).
    SplitAtEnd,
    /// A split at the end of a curve held `tangent_on` the split one, with an edge `tangent`
    /// between the two, which would then share that end (:152): the constraint.
    SplitEdgeTangent(String),
    /// Trimming a point (:330).
    TrimPoint,
    /// A trim picked at a round's centre (:222).
    TrimAtCentre,
    /// A trim leaving a constraint no piece can take (decision 2, :217, :472).
    NoPieceTakes(String),
    /// Unjoining a point no other curve shares (decision 9, :239).
    UnjoinUnshared,
    /// A merge naming a point twice (:272).
    MergeNamedTwice,
    /// A merge after which a curve uses one point twice (:273).
    MergeCollapses,
    /// A merge after which a constraint names one point twice (:274).
    MergeNamesTwice(String),
    /// A merge whose solve conflicts (:277).
    MergeSolveConflicts,
    /// A merge from apart whose solve's result Silico can't take (:278-280).
    MergeSolveUnbuildable,
    /// A merge whose sketch the gate refuses, its curve left 1e-7 mm or less where Silico
    /// places the merged point (:276): the curve.
    MergeGated(String),
    /// Removing a point curves still use (decision 7, :109).
    RemovePointInUse,
}

impl Rule {
    /// The note's lines it cites.
    pub fn lines(&self) -> Vec<u32> {
        match self {
            Rule::SplitNotCurve => vec![115, 153],
            Rule::SplitAtEnd => vec![153],
            Rule::SplitEdgeTangent(_) => vec![152],
            Rule::SplitCircleAtOne => vec![473],
            Rule::SplitOffCurve(_) => vec![474],
            Rule::TrimPoint => vec![330],
            Rule::TrimAtCentre => vec![222],
            Rule::NoPieceTakes(_) => vec![217, 472],
            Rule::UnjoinUnshared => vec![234, 239],
            Rule::MergeNamedTwice => vec![272],
            Rule::MergeCollapses => vec![273],
            Rule::MergeNamesTwice(_) => vec![274],
            Rule::MergeSolveConflicts => vec![277],
            Rule::MergeSolveUnbuildable => vec![278, 279, 280],
            Rule::MergeGated(_) => vec![276],
            Rule::RemovePointInUse => vec![107, 109],
        }
    }

    /// What the note says FreeCAD 1.1.3 does with an edit Silico refuses by this rule, where
    /// it pins that: then nothing else of FreeCAD's agrees, a refusal included. None where
    /// any refusal of FreeCAD's is the rule's counterpart (a merge its Coincident command
    /// refuses, :51-53), or the note gives FreeCAD both a refusal and an edit (a split at or
    /// beyond an end raises, one near it makes a tiny piece, :117-118; a merge the gate
    /// refuses, :276, whose coincidences FreeCAD's solve takes to a curve with length, or its
    /// Coincident command rolls back as a worse solve, :53).
    pub fn pinned(&self) -> Option<Pinned> {
        match self {
            Rule::SplitNotCurve => Some(Pinned::Crashes),
            Rule::TrimAtCentre => Some(Pinned::Raises("Unknown C++ exception")),
            Rule::UnjoinUnshared => Some(Pinned::Raises(VERTEX_DELETE_RAISES)),
            Rule::SplitCircleAtOne
            | Rule::SplitOffCurve(_)
            | Rule::SplitEdgeTangent(_)
            | Rule::TrimPoint
            | Rule::NoPieceTakes(_) => Some(Pinned::Edits),
            Rule::MergeSolveUnbuildable => Some(Pinned::SolvesOrRollsBack),
            Rule::RemovePointInUse => Some(Pinned::UnjoinsVertex),
            _ => None,
        }
    }
}

/// What FreeCAD 1.1.3's vertex Delete raises on an end with no coincidence (:234).
pub const VERTEX_DELETE_RAISES: &str = "Not able to delete a constraint on point";

/// FreeCAD 1.1.3's outcome as the note pins it for a [`Rule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pinned {
    /// It crashes, by a segmentation fault (and the harness sees that, never a hang or a
    /// refusal).
    Crashes,
    /// Its edit raises, with this text, as the note gives it (a trim picked at a round's
    /// centre, "Unknown C++ exception", :222; an unjoin of an end with no coincidence,
    /// [`VERTEX_DELETE_RAISES`], :234).
    Raises(&'static str),
    /// It makes the edit, and its result is what the note says, which the excusal checks.
    Edits,
    /// It makes the merge, and its solve either succeeds, which the excusal checks, or fails
    /// (-1), which its Coincident command rolls back, the sketch as it was before
    /// ([`rolled_back`]): FreeCAD 1.1.3 isn't the same every run here (:278).
    SolvesOrRollsBack,
    /// Its Delete on the vertex (one curve's end at the point) deletes no curve: it unjoins
    /// that end (:105-107), which the excusal checks, or, on an end with no coincidence,
    /// 1.1.3 raises (:234).
    UnjoinsVertex,
}

impl Pinned {
    fn does(self) -> &'static str {
        match self {
            Pinned::Crashes => "crash, by a segmentation fault",
            Pinned::Raises(_) => "raise as the note gives it",
            Pinned::UnjoinsVertex => {
                "unjoin the vertex its Delete is on, deleting no curve, or raise only where the end has no coincidence"
            }
            Pinned::Edits => "make the edit",
            Pinned::SolvesOrRollsBack => {
                "make the merge, its solve succeeding, or failing (-1) and rolled back to the sketch before it"
            }
        }
    }
}

/// The rule Silico's refusal of `p` is, if it's one the note gives: checked against the
/// refusal itself (its code, the elements it names, the request's field) and against the
/// sketch (what it names is so), never inferred from the case alone.
pub fn rule(p: &Prepared) -> Option<Rule> {
    let shape = Shape::of(&p.sketch);
    let r = match &p.silico {
        SilicoSide::Refused(r) => r,
        SilicoSide::Edited(s) => {
            let Err(e) = &s.ours.outcome else {
                return None;
            };
            if !matches!(p.edit, Edit::Merge { .. }) {
                return None;
            }
            return match e.code() {
                silico_core::ErrorCode::OverConstrained | silico_core::ErrorCode::SolveFailed => {
                    Some(Rule::MergeSolveConflicts)
                }
                silico_core::ErrorCode::InvalidResult if merged_apart(p).is_some() => {
                    Some(Rule::MergeSolveUnbuildable)
                }
                _ => None,
            };
        }
        SilicoSide::Gated(_) => return None,
    };
    if r.code != "invalid_input" {
        return None;
    }
    let field = r.parameter.as_deref().unwrap_or_default();
    let names = |want: &[&str]| {
        r.elements
            .iter()
            .map(String::as_str)
            .eq(want.iter().copied())
    };
    let index =
        |prefix: &str| -> Option<usize> { field.strip_prefix(prefix).and_then(|n| n.parse().ok()) };
    let kind = |id: &str| shape.members.get(id).map(|m| m.kind);
    match &p.edit {
        Edit::Split { curve, at } => {
            if field == "curve" && names(&[curve]) && kind(curve) == Some("point") {
                return Some(Rule::SplitNotCurve);
            }
            if field == "at" && names(&[curve]) && kind(curve) == Some("circle") && at.len() != 2 {
                return Some(Rule::SplitCircleAtOne);
            }
            let position = *at.get(index("at.")?)?;
            // The edge tangent, of the split curve and another that ends where the split is
            // asked for: the pieces would share that end with it. A curve the sketch hasn't is
            // a piece the split made, the tangency carried to it (:152), so the curve's.
            let piece = |c: &String| {
                let known = shape.members.contains_key(c)
                    || shape.points.contains_key(c)
                    || p.sketch.constraints.iter().any(|k| k.id.as_str() == c);
                if known { c.clone() } else { curve.clone() }
            };
            if let [id, a, b, point] = &r.elements[..] {
                let (a, b) = (piece(a), piece(b));
                let tangent = p.sketch.constraints.iter().any(|k| {
                    k.id.as_str() == id
                        && matches!(&k.form, Form::Tangent { a: x, b: y, .. }
                            if x.element().is_some_and(|x| x.as_str() == a)
                                && y.element().is_some_and(|y| y.as_str() == b))
                });
                if tangent && (a == *curve || b == *curve) {
                    let other = if a == *curve { b } else { a };
                    let ends_there = shape.members.get(&other).is_some_and(|m| {
                        m.kind != "point"
                            && [1, 2]
                                .iter()
                                .filter_map(|pos| m.at.get(pos))
                                .any(|q| q == point)
                    });
                    let at_point = shape.points.get(point).is_some_and(|q| {
                        libm::hypot(q[0] - position[0], q[1] - position[1]) <= LENGTH_TOLERANCE
                    });
                    return (ends_there && at_point).then(|| Rule::SplitEdgeTangent(id.clone()));
                }
            }
            if names(&[curve]) && r.message.contains("farther than 1e-7 mm") {
                let off = off_curve(&shape, curve, position)?;
                return (off > LENGTH_TOLERANCE).then_some(Rule::SplitOffCurve(off));
            }
            match &r.elements[..] {
                [c, end] if c == curve && near_end(&shape, curve, end, position) => {
                    Some(Rule::SplitAtEnd)
                }
                _ => None,
            }
        }
        Edit::Trim { curve, pick } => {
            if field == "curve" && names(&[curve]) && kind(curve) == Some("point") {
                return Some(Rule::TrimPoint);
            }
            if field != "pick" {
                return None;
            }
            let member = shape.members.get(curve)?;
            if names(&[curve]) && matches!(member.kind, "arc" | "circle") {
                let center = shape.points.get(member.at.get(&3)?)?;
                return (pick == center).then_some(Rule::TrimAtCentre);
            }
            let [constraint, c] = &r.elements[..] else {
                return None;
            };
            if c != curve {
                return None;
            }
            let curve_ref = Reference::Element(element_id(curve).ok()?);
            let form = &p
                .sketch
                .constraints
                .iter()
                .find(|k| k.id.as_str() == constraint)?
                .form;
            let cannot = match form {
                Form::Symmetric { about, .. } => *about == curve_ref && member.kind == "line",
                Form::Tangent { a, b, .. } => {
                    matches!(member.kind, "arc" | "circle") && (*a == curve_ref || *b == curve_ref)
                }
                _ => false,
            };
            cannot.then(|| Rule::NoPieceTakes(constraint.clone()))
        }
        Edit::Unjoin { point, curve } => (field == "point"
            && names(&[point, curve])
            && shape.users(point).len() < 2
            && shape.users(point).contains(&curve))
        .then_some(Rule::UnjoinUnshared),
        Edit::Merge { points } => {
            if let Some(n) = index("points.") {
                return (points[..n.min(points.len())].contains(points.get(n)?)
                    && names(&[points.get(n)?.as_str()]))
                .then_some(Rule::MergeNamedTwice);
            }
            if field != "points" {
                return None;
            }
            let merged: BTreeSet<&str> = points.iter().map(String::as_str).collect();
            let constraint = |id: &str| p.sketch.constraints.iter().find(|k| k.id.as_str() == id);
            // The merged sketch gated (:276), naming the request's points first and, last, a
            // line its placement (every merged point at the first one's place) leaves 1e-7 mm
            // or less.
            if r.message.contains("would leave a sketch the gate refuses") {
                let named_first = r.elements.len() > points.len()
                    && r.elements[..points.len()].iter().eq(points.iter());
                let placed = |id: &String| {
                    let id = if merged.contains(id.as_str()) {
                        &points[0]
                    } else {
                        id
                    };
                    shape.points.get(id).copied()
                };
                let collapsed = r.elements.last().and_then(|last| {
                    let member = shape.members.get(last)?;
                    (member.kind == "line").then_some(())?;
                    let (a, b) = (placed(member.at.get(&1)?)?, placed(member.at.get(&2)?)?);
                    (libm::hypot(b[0] - a[0], b[1] - a[1]) <= LENGTH_TOLERANCE)
                        .then(|| last.clone())
                });
                return collapsed.filter(|_| named_first).map(Rule::MergeGated);
            }
            // A curve using two of the merged points, named last.
            if let Some(last) = r.elements.last()
                && let Some(member) = shape.members.get(last)
                && member.kind != "point"
                && member
                    .at
                    .values()
                    .filter(|q| merged.contains(q.as_str()))
                    .count()
                    >= 2
            {
                return Some(Rule::MergeCollapses);
            }
            match &r.elements[..] {
                [id, _kept] => {
                    let form = &constraint(id)?.form;
                    let named = point_references(form)
                        .into_iter()
                        .filter(|reference| {
                            reference
                                .element()
                                .is_some_and(|e| merged.contains(e.as_str()))
                        })
                        .count();
                    (named >= 2).then(|| Rule::MergeNamesTwice(id.clone()))
                }
                _ => None,
            }
        }
        Edit::Remove { members } => {
            let n = index("members.")?;
            let point = members.get(n)?;
            let keeping = shape.users(point).into_iter().any(|c| !members.contains(c));
            (r.elements.first() == Some(point) && keeping).then_some(Rule::RemovePointInUse)
        }
    }
}

/// The points a constraint names.
fn point_references(form: &Form) -> Vec<&Reference> {
    match form {
        Form::HorizontalPoints { a, b }
        | Form::VerticalPoints { a, b }
        | Form::Distance { a, b, .. }
        | Form::DistanceX { a, b, .. }
        | Form::DistanceY { a, b, .. } => vec![a, b],
        Form::Symmetric { a, b, about } => vec![a, b, about],
        Form::PointLineDistance { point, .. }
        | Form::On { point, .. }
        | Form::Midpoint { point, .. } => vec![point],
        Form::TangentOn { at, .. } | Form::TangentAt { at, .. } => vec![at],
        _ => Vec::new(),
    }
}

/// Whether `position` is at, past, or within 1e-7 mm of `curve`'s end `end`, along it.
fn near_end(shape: &Shape, curve: &str, end: &str, position: [f64; 2]) -> bool {
    let Some(member) = shape.members.get(curve) else {
        return false;
    };
    let at = |pos: i32| {
        member
            .at
            .get(&pos)
            .and_then(|id| shape.points.get(id))
            .copied()
    };
    let is_start = member.at.get(&1).is_some_and(|s| s == end);
    let is_end = member.at.get(&2).is_some_and(|s| s == end);
    if !(is_start || is_end) {
        return false;
    }
    match member.kind {
        "line" => {
            let (Some(a), Some(b)) = (at(1), at(2)) else {
                return false;
            };
            let length = libm::hypot(b[0] - a[0], b[1] - a[1]);
            let t = ((position[0] - a[0]) * (b[0] - a[0]) + (position[1] - a[1]) * (b[1] - a[1]))
                / length;
            if is_start {
                t <= LENGTH_TOLERANCE
            } else {
                t >= length - LENGTH_TOLERANCE
            }
        }
        "arc" => {
            let (Some(c), Some(s), Some(e)) = (at(3), at(1), at(2)) else {
                return false;
            };
            let Some([r, from, to]) = arc_from_points(c, s, e) else {
                return false;
            };
            let mut angle = libm::atan2(position[1] - c[1], position[0] - c[0]);
            while angle < from {
                angle += TAU;
            }
            // Past the sweep is past an end; within it, by arc length from that end.
            angle > to
                || (is_start && r * (angle - from) <= LENGTH_TOLERANCE)
                || (is_end && r * (to - angle) <= LENGTH_TOLERANCE)
        }
        _ => false,
    }
}

/// Why Silico's refusal by `rule` of an edit FreeCAD made is the note's, if FreeCAD's side
/// is what the note says it is: none where it isn't, which is then a difference.
fn excused(p: &Prepared, rule: &Rule, fc: &FreecadEdited) -> Option<String> {
    let curve = match &p.edit {
        Edit::Split { curve, .. } | Edit::Trim { curve, .. } => Some(curve.as_str()),
        _ => None,
    };
    match rule {
        Rule::SplitCircleAtOne => circle_opened(p, fc, curve?).then(|| format!(
            "a circle split at one position: Silico refuses it (decision 3, {NOTE}:473), FreeCAD makes one arc whose ends aren't joined (:121)"
        )),
        Rule::SplitOffCurve(off) => split_at_projections(p, fc, curve?).then(|| format!(
            "a split position {off:e} mm off the curve: Silico refuses it (decision 4, {NOTE}:474), FreeCAD splits at its projection (:116)"
        )),
        Rule::SplitAtEnd => {
            let shortest = fc
                .edited
                .iter()
                .filter_map(curve_length)
                .fold(f64::INFINITY, f64::min);
            (shortest <= LENGTH_TOLERANCE).then(|| {
                format!(
                    "a split within 1e-7 mm of an end: Silico refuses it ({NOTE}:153), FreeCAD 1.1.3 makes a piece {shortest:e} mm long (:118)"
                )
            })
        }
        Rule::SplitEdgeTangent(id) => split_at_projections(p, fc, curve?).then(|| format!(
            "edge tangent {id} with a curve ending at the split point: Silico refuses the split ({NOTE}:152), where FreeCAD's split makes the pieces new ends and never shares the point"
        )),
        Rule::TrimPoint => deleted_alone(p, fc, curve?).then(|| format!(
            "a trim of a point: Silico refuses it ({NOTE}:330), FreeCAD 1.1.3 deletes it (:159)"
        )),
        Rule::NoPieceTakes(id) => {
            let dropped = !fc.constraints.iter().any(|c| c.name == fc_name(id));
            dropped.then(|| {
                format!(
                    "constraint {id} no piece can take: Silico refuses the trim (decision 2, {NOTE}:217, :472), FreeCAD drops it"
                )
            })
        }
        Rule::MergeGated(curve) => {
            // FreeCAD's coincidences solve to a curve longer than 1e-7 mm, where Silico's
            // placement collapses it.
            let i = p.model.elements.iter().position(|e| e == curve)?;
            let id = fc.ids_before.get(i)?;
            let solved = fc.geometry.iter().find(|g| g.id == *id)?;
            curve_length(solved)
                .is_some_and(|l| l > LENGTH_TOLERANCE)
                .then(|| {
                    format!(
                        "points merged from apart, whose placement leaves {curve} 1e-7 mm or less, a sketch the gate refuses: Silico refuses the merge ({NOTE}:276), where FreeCAD's coincidences solve to a curve with length"
                    )
                })
        }
        // FreeCAD's coincidences solve: its solve after the edit succeeds, every coincidence
        // made, and each pair's places at one place.
        Rule::MergeSolveUnbuildable => {
            (fc.after.code == 0 && merged_by_freecad(p, fc)).then(|| format!(
            "points merged from apart, whose solve Silico refuses as a result it can't take ({NOTE}:278-280), where FreeCAD's coincidences solve"
        ))
        }
        Rule::RemovePointInUse => {
            vertex_unjoined(p, fc).then(|| format!(
                "removing a point curves use: Silico refuses it ({NOTE}:109, decision 7), FreeCAD's Delete on the vertex unjoins that curve's end and deletes no curve (:105-107)"
            ))
        }
        _ => None,
    }
}

/// Whether FreeCAD's merge of `p` failed its solve (-1, where the sketch before it solved)
/// and is rolled back to just what it was before, as its Coincident command does (:53,
/// :278): its geometry where the case drew it, every curve and point kept and none made,
/// each place, radius, and construction as drawn; and its constraints the case's, as FreeCAD
/// was given them, and the merge's coincidences, which the rollback takes off. Anything else
/// (a partial merge, a solve that moved something, another failure) isn't the rollback.
pub fn rolled_back(p: &Prepared, theirs: Option<&Theirs>) -> bool {
    let (Some(Theirs::Edited(fc)), Plan::Run { job, .. }) = (theirs, &p.plan) else {
        return false;
    };
    let FreecadEdit::Merge { coincidences } = &job.edit else {
        return false;
    };
    if fc.before.code != 0 || fc.after.code != -1 {
        return false;
    }
    let shape = Shape::of(&p.sketch);
    let drawn = |g: &FcGeometry| as_drawn(p, &shape, fc, g);
    let ids = |list: &[FcGeometry]| list.iter().map(|g| g.id).collect::<Vec<_>>();
    if ids(&fc.geometry) != fc.ids_before
        || ids(&fc.edited) != fc.ids_before
        || !fc.geometry.iter().all(drawn)
        || !fc.edited.iter().all(drawn)
    {
        return false;
    }
    // Each constraint by what it is, and its value where FreeCAD was given one.
    type Made = ((String, Vec<[i32; 2]>, bool, String), Option<f64>);
    let mut given: Vec<Made> = job
        .constraints
        .iter()
        .map(|c| {
            let key = (
                c.kind.to_owned(),
                pairs(c.kind, &c.ints),
                c.driving,
                c.name.clone().unwrap_or_default(),
            );
            (key, c.value)
        })
        .chain(coincidences.iter().map(|&[g1, p1, g2, p2]| {
            let key = (
                "Coincident".to_owned(),
                vec![[g1, p1], [g2, p2]],
                true,
                String::new(),
            );
            (key, None)
        }))
        .collect();
    let mut found: Vec<Made> = fc
        .constraints
        .iter()
        .map(|c| {
            let key = (c.kind.clone(), c.refs.clone(), c.driving, c.name.clone());
            (key, Some(c.value))
        })
        .collect();
    given.sort_by(|a, b| a.0.cmp(&b.0));
    found.sort_by(|a, b| a.0.cmp(&b.0));
    given.len() == found.len()
        && given.iter().zip(&found).all(|((key, want), (got, value))| {
            key == got
                && match (want, value) {
                    (Some(want), Some(value)) => (want - value).abs() <= VALUE_TOLERANCE,
                    _ => true,
                }
        })
}

/// Whether FreeCAD's geometry `g` (after a merge's edit, `fc`) is where `p`'s case drew it:
/// the member of its ID before the edit, each of its places, its radius, its kind, and its
/// construction, within 1e-8 mm.
fn as_drawn(p: &Prepared, shape: &Shape, fc: &FreecadEdited, g: &FcGeometry) -> bool {
    let Some(member) = fc
        .ids_before
        .iter()
        .position(|id| *id == g.id)
        .and_then(|i| p.model.elements.get(i))
    else {
        return false;
    };
    let Some(m) = shape.members.get(member) else {
        return false;
    };
    let places = g.points.iter().all(|(pos, at)| {
        let point = match (m.kind, pos.parse::<i32>()) {
            ("point", Ok(1)) => Some(member),
            (_, Ok(pos)) => m.at.get(&pos),
            _ => None,
        };
        point
            .and_then(|q| shape.points.get(q))
            .is_some_and(|q| near(*q, *at))
    });
    let radius = match (g.radius, shape.radii.get(member)) {
        (Some(r), Some(drawn)) => (r - drawn).abs() <= sketch::TOLERANCE,
        (None, None) => true,
        _ => false,
    };
    g.kind == m.kind && places && radius && g.construction == m.construction
}

/// A place of FreeCAD's model: a geometry index and a position.
type Place = (i32, i32);

/// The vertex FreeCAD's Delete is on (the job's geometry and position), and every other place
/// of FreeCAD's model joined to it before the edit.
fn vertex_deleted(p: &Prepared) -> Option<(Place, Vec<Place>)> {
    let Plan::Run { job, .. } = &p.plan else {
        return None;
    };
    let FreecadEdit::VertexDelete { geometry, position } = job.edit else {
        return None;
    };
    let vertex = (geometry, position);
    let others = p
        .model
        .places
        .values()
        .find(|places| places.contains(&vertex))
        .map(|places| places.iter().copied().filter(|q| *q != vertex).collect())
        .unwrap_or_default();
    Some((vertex, others))
}

/// Whether the vertex FreeCAD's Delete is on is shared in its model: joined to another place
/// (a coincidence there).
fn vertex_shared(p: &Prepared) -> bool {
    vertex_deleted(p).is_some_and(|(_, others)| !others.is_empty())
}

/// Whether FreeCAD's Delete on the job's vertex did what the note says (:105-107, :231):
/// every geometry kept where the case drew it, nothing made, and that end joined to none of
/// the places it was joined to any more.
fn vertex_unjoined(p: &Prepared, fc: &FreecadEdited) -> bool {
    let Some(((g, pos), others)) = vertex_deleted(p) else {
        return false;
    };
    let shape = Shape::of(&p.sketch);
    let ids = |list: &[FcGeometry]| list.iter().map(|g| g.id).collect::<Vec<_>>();
    let joins = Joins::of(fc);
    ids(&fc.edited) == fc.ids_before
        && fc.edited.iter().all(|e| as_drawn(p, &shape, fc, e))
        && others.iter().all(|other| !joins.joined((g, pos), *other))
}

/// Whether FreeCAD's merge made its coincidences, each of the job's in FreeCAD's constraints
/// (or the end-to-end tangency the Coincident command puts in its place, :55), and its solve
/// put each pair's places at one place, within 1e-8 mm.
fn merged_by_freecad(p: &Prepared, fc: &FreecadEdited) -> bool {
    let Plan::Run { job, .. } = &p.plan else {
        return false;
    };
    let FreecadEdit::Merge { coincidences } = &job.edit else {
        return false;
    };
    let place = |g: i32, pos: i32| {
        usize::try_from(g)
            .ok()
            .and_then(|i| fc.geometry.get(i))
            .and_then(|geo| geo.points.get(&pos.to_string()).copied())
    };
    coincidences.iter().all(|&[g1, p1, g2, p2]| {
        let made = fc.constraints.iter().any(|c| {
            matches!(c.kind.as_str(), "Coincident" | "Tangent")
                && (c.refs == [[g1, p1], [g2, p2]] || c.refs == [[g2, p2], [g1, p1]])
        });
        let together = match (place(g1, p1), place(g2, p2)) {
            (Some(a), Some(b)) => near(a, b),
            _ => false,
        };
        made && together
    })
}

/// `member`'s FreeCAD geometry after the edit, before the solve, by the ID it had before.
fn edited_geometry<'a>(
    p: &Prepared,
    fc: &'a FreecadEdited,
    member: &str,
) -> Option<&'a FcGeometry> {
    let i = p.model.elements.iter().position(|e| e == member)?;
    let id = fc.ids_before.get(i)?;
    fc.edited.iter().find(|g| g.id == *id)
}

/// The geometry FreeCAD's edit made: what has an ID it hadn't before.
fn made_geometry(fc: &FreecadEdited) -> Vec<&FcGeometry> {
    fc.edited
        .iter()
        .filter(|g| !fc.ids_before.contains(&g.id))
        .collect()
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() <= sketch::TOLERANCE && (a[1] - b[1]).abs() <= sketch::TOLERANCE
}

/// Whether FreeCAD's split of circle `curve` at one position made what the note says
/// (:121): one arc in the circle's place (its ID, centre and radius), nothing more, opening
/// at the position's projection onto the circle, its start and end at one place and not
/// joined.
fn circle_opened(p: &Prepared, fc: &FreecadEdited, curve: &str) -> bool {
    let shape = Shape::of(&p.sketch);
    let (Some(arc), Some(centre), Some(radius)) = (
        edited_geometry(p, fc, curve),
        shape.point(curve, 3).and_then(|c| shape.points.get(c)),
        shape.radii.get(curve),
    ) else {
        return false;
    };
    let at = |k: &str| arc.points.get(k).copied();
    let (Some(start), Some(end), Some(c)) = (at("1"), at("2"), at("3")) else {
        return false;
    };
    let Some(g) = fc
        .geometry
        .iter()
        .position(|solved| solved.id == arc.id)
        .and_then(|i| i32::try_from(i).ok())
    else {
        return false;
    };
    // It opens at the split position's projection onto the circle (:121).
    let opens = match &p.edit {
        Edit::Split { at, .. } => at
            .first()
            .and_then(|q| projection(&shape, curve, *q))
            .is_some_and(|q| near(start, q)),
        _ => false,
    };
    arc.kind == "arc"
        && opens
        && made_geometry(fc).is_empty()
        && near(start, end)
        && near(c, *centre)
        && arc
            .radius
            .is_some_and(|r| (r - radius).abs() <= sketch::TOLERANCE)
        && !Joins::of(fc).joined((g, 1), (g, 2))
}

/// Whether FreeCAD's split of `curve` made what the note says (:116): two pieces of its kind
/// (a circle's arcs), the one keeping its ID and one new, which meet, an end of each, at each
/// split position's projection onto the curve; a line's or an arc's far ends where they
/// were, and an arc's pieces about its centre with its radius.
fn split_at_projections(p: &Prepared, fc: &FreecadEdited, curve: &str) -> bool {
    let Edit::Split { at, .. } = &p.edit else {
        return false;
    };
    let shape = Shape::of(&p.sketch);
    let Some(kind) = shape.members.get(curve).map(|m| match m.kind {
        "circle" => "arc",
        other => other,
    }) else {
        return false;
    };
    let (Some(kept), [made]) = (edited_geometry(p, fc, curve), &made_geometry(fc)[..]) else {
        return false;
    };
    let ends = |g: &FcGeometry| -> Vec<[f64; 2]> {
        ["1", "2"]
            .iter()
            .filter_map(|k| g.points.get(*k).copied())
            .collect()
    };
    let meet = |q: [f64; 2]| {
        ends(kept).iter().any(|e| near(*e, q)) && ends(made).iter().any(|e| near(*e, q))
    };
    let drawn = |pos: i32| {
        shape
            .point(curve, pos)
            .and_then(|q| shape.points.get(q))
            .copied()
    };
    let at_pos = |g: &FcGeometry, k: &str, want: Option<[f64; 2]>| matches!((g.points.get(k), want), (Some(a), Some(b)) if near(*a, b));
    // A line's or an arc's far ends where they were: the kept piece from the curve's start,
    // the new one to its end. An arc's pieces, and a circle's, on its circle.
    let far_ends = shape.members.get(curve).is_some_and(|m| m.kind == "circle")
        || (at_pos(kept, "1", drawn(1)) && at_pos(made, "2", drawn(2)));
    let on_circle = kind != "arc"
        || [kept, made].iter().all(|g| {
            at_pos(g, "3", drawn(3))
                && matches!((g.radius, shape.radii.get(curve)), (Some(r), Some(want)) if (r - want).abs() <= sketch::TOLERANCE)
        });
    kept.kind == kind
        && made.kind == kind
        && far_ends
        && on_circle
        && !at.is_empty()
        && at
            .iter()
            .all(|position| projection(&shape, curve, *position).is_some_and(meet))
}

/// Whether FreeCAD's trim of the lone point `point` deleted it, as the note says 1.1.3 does
/// (:159): it's gone, and every other geometry is kept where the case drew it, nothing made.
fn deleted_alone(p: &Prepared, fc: &FreecadEdited, point: &str) -> bool {
    let Some(i) = p.model.elements.iter().position(|e| e == point) else {
        return false;
    };
    let Some(&gone) = fc.ids_before.get(i) else {
        return false;
    };
    let left: BTreeSet<i64> = fc.edited.iter().map(|g| g.id).collect();
    let kept: BTreeSet<i64> = fc
        .ids_before
        .iter()
        .copied()
        .filter(|id| *id != gone)
        .collect();
    let shape = Shape::of(&p.sketch);
    left == kept && fc.edited.iter().all(|g| as_drawn(p, &shape, fc, g))
}

/// A trim's curves within decision 6's band of tangency to the trimmed one: Silico counts a
/// contact within 1e-7 mm as a cut, FreeCAD 1.1.3's intersector one within 1e-12 (:476), so
/// the two may cut the trimmed curve at different places.
#[derive(Debug)]
struct Band {
    /// The trimmed curve.
    trimmed: String,
    /// Each curve in the band, and its gap from tangent, in mm.
    near: BTreeMap<String, f64>,
}

impl Band {
    /// The note's excuse for each curve in the band.
    fn excuses(&self) -> Vec<String> {
        self.near
            .iter()
            .map(|(other, gap)| {
                format!(
                    "{other} is {gap:e} mm from tangent to {}: Silico's trim counts that contact as a cut, FreeCAD 1.1.3's doesn't (decision 6, {NOTE}:476); the two curves, the trimmed one's pieces, their points, and what names them aren't compared",
                    self.trimmed
                )
            })
            .collect()
    }
}

/// What an edit leaves unsettled, whose places or curves the comparison takes only where
/// both sides settle them alike: points merged from apart (:270), and a trim's curves
/// within decision 6's band (:476).
#[derive(Clone, Copy)]
struct Unsettled<'a> {
    merged: Option<&'a BTreeSet<String>>,
    band: Option<&'a Band>,
}

/// For a trim, the curves whose gap from the trimmed one, as a tangency, is in decision 6's
/// band (:476), if any is.
fn contact_band(p: &Prepared) -> Option<Band> {
    let Edit::Trim { curve, .. } = &p.edit else {
        return None;
    };
    let shape = Shape::of(&p.sketch);
    let at = |m: &Member, pos: i32| m.at.get(&pos).and_then(|id| shape.points.get(id)).copied();
    let geometry = |id: &str| -> Option<Geometry2> {
        let m = shape.members.get(id)?;
        match m.kind {
            "line" => Some(Geometry2::Line(at(m, 1)?, at(m, 2)?)),
            "arc" | "circle" => Some(Geometry2::Round(at(m, 3)?, *shape.radii.get(id)?)),
            _ => None,
        }
    };
    let trimmed = geometry(curve)?;
    let near: BTreeMap<String, f64> = shape
        .members
        .keys()
        .filter(|id| *id != curve)
        .filter_map(|other| {
            let gap = match (trimmed, geometry(other)?) {
                (Geometry2::Line(a, b), Geometry2::Round(c, r))
                | (Geometry2::Round(c, r), Geometry2::Line(a, b)) => {
                    let (ux, uy) = (b[0] - a[0], b[1] - a[1]);
                    let off = (ux * (c[1] - a[1]) - uy * (c[0] - a[0])).abs() / libm::hypot(ux, uy);
                    (off - r).abs()
                }
                (Geometry2::Round(c, r), Geometry2::Round(d, q)) => {
                    let apart = libm::hypot(d[0] - c[0], d[1] - c[1]);
                    ((apart - (r + q)).abs()).min((apart - (r - q).abs()).abs())
                }
                (Geometry2::Line(..), Geometry2::Line(..)) => return None,
            };
            (gap > 1e-12 && gap <= LENGTH_TOLERANCE).then(|| (other.clone(), gap))
        })
        .collect();
    (!near.is_empty()).then(|| Band {
        trimmed: curve.clone(),
        near,
    })
}

#[derive(Clone, Copy)]
enum Geometry2 {
    Line([f64; 2], [f64; 2]),
    Round([f64; 2], f64),
}

/// For a merge of points apart (farther than the comparison's tolerance): the point they
/// become. Silico places it at the first named point's coordinates; FreeCAD's coincidences
/// are solved (:270).
fn merged_apart(p: &Prepared) -> Option<BTreeSet<String>> {
    let Edit::Merge { points } = &p.edit else {
        return None;
    };
    let shape = Shape::of(&p.sketch);
    let kept_id = points.first()?;
    let kept = shape.points.get(kept_id)?;
    points[1..]
        .iter()
        .filter_map(|q| shape.points.get(q))
        .any(|q| libm::hypot(q[0] - kept[0], q[1] - kept[1]) > sketch::TOLERANCE)
        .then(|| BTreeSet::from([kept_id.clone()]))
}

/// A line of what FreeCAD made: its curves and constraints, counted.
fn summary(fc: &FreecadEdited) -> String {
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for g in &fc.geometry {
        *kinds.entry(g.kind.as_str()).or_default() += 1;
    }
    let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect();
    format!(
        "{}, {} constraints, solve {} with {} degrees of freedom",
        kinds.join(", "),
        fc.constraints.len(),
        fc.after.code,
        fc.after.dof
    )
}

/// For a merge, whether FreeCAD's Coincident command would refuse what the coincidences
/// made: a curve whose two ends or centre and end are now one (:52, #20771), or a solve
/// worse than before, which it rolls back (:53).
fn merge_refusal(p: &Prepared, fc: &FreecadEdited) -> Option<String> {
    if !matches!(p.edit, Edit::Merge { .. }) {
        return None;
    }
    // Points already coincident (:51): a place named with itself, or two the sketch FreeCAD
    // was given already joins.
    if let Plan::Run { job, .. } = &p.plan
        && let FreecadEdit::Merge { coincidences } = &job.edit
    {
        let given = Joins::of_constraints(
            job.constraints
                .iter()
                .filter(|c| matches!(c.kind, "Coincident" | "Tangent" | "Perpendicular"))
                .filter_map(|c| match c.ints[..] {
                    [ga, pa, gb, pb] if pa != 0 && pb != 0 => Some(((ga, pa), (gb, pb))),
                    _ => None,
                }),
        );
        if coincidences
            .iter()
            .any(|&[g1, p1, g2, p2]| (g1, p1) == (g2, p2) || given.joined((g1, p1), (g2, p2)))
        {
            return Some(format!(
                "FreeCAD's Coincident command refuses points already coincident ({NOTE}:51)"
            ));
        }
    }
    let joins = Joins::of(fc);
    for (i, g) in fc.geometry.iter().enumerate() {
        let places: Vec<i32> = g.points.keys().filter_map(|k| k.parse().ok()).collect();
        let g = i32::try_from(i).unwrap_or(i32::MAX);
        for (n, a) in places.iter().enumerate() {
            for b in &places[n + 1..] {
                if joins.joined((g, *a), (g, *b)) {
                    return Some(format!(
                        "FreeCAD's Coincident command refuses a coincidence that collapses geometry {g} ({NOTE}:52)"
                    ));
                }
            }
        }
    }
    let new = |after: &[usize], before: &[usize]| after.iter().any(|i| !before.contains(i));
    let (b, a) = (&fc.before, &fc.after);
    if (a.code != 0 && b.code == 0)
        || new(&a.conflicting, &b.conflicting)
        || new(&a.redundant, &b.redundant)
        || new(&a.malformed, &b.malformed)
    {
        return Some(format!(
            "FreeCAD's Coincident command rolls back a worse solve ({NOTE}:53): solve {} with conflicting {:?}, redundant {:?}, malformed {:?}",
            a.code, a.conflicting, a.redundant, a.malformed
        ));
    }
    None
}

/// How far `position` is from `curve` of `shape` (a line extended, a round's circle).
fn off_curve(shape: &Shape, curve: &str, position: [f64; 2]) -> Option<f64> {
    let member = shape.members.get(curve)?;
    let p = |pos: i32| {
        member
            .at
            .get(&pos)
            .and_then(|id| shape.points.get(id))
            .copied()
    };
    match member.kind {
        "line" => {
            let (a, b) = (p(1)?, p(2)?);
            let (ux, uy) = (b[0] - a[0], b[1] - a[1]);
            let cross = ux * (position[1] - a[1]) - uy * (position[0] - a[0]);
            Some(cross.abs() / libm::hypot(ux, uy))
        }
        "arc" | "circle" => {
            let c = p(3)?;
            let r = *shape.radii.get(curve)?;
            Some((libm::hypot(position[0] - c[0], position[1] - c[1]) - r).abs())
        }
        _ => None,
    }
}

/// `position`'s projection onto `curve` of `shape` (a line extended, a round's circle).
fn projection(shape: &Shape, curve: &str, position: [f64; 2]) -> Option<[f64; 2]> {
    let member = shape.members.get(curve)?;
    let p = |pos: i32| {
        member
            .at
            .get(&pos)
            .and_then(|id| shape.points.get(id))
            .copied()
    };
    match member.kind {
        "line" => {
            let (a, b) = (p(1)?, p(2)?);
            let (ux, uy) = (b[0] - a[0], b[1] - a[1]);
            let t = ((position[0] - a[0]) * ux + (position[1] - a[1]) * uy) / (ux * ux + uy * uy);
            Some([a[0] + t * ux, a[1] + t * uy])
        }
        "arc" | "circle" => {
            let c = p(3)?;
            let r = *shape.radii.get(curve)?;
            let (dx, dy) = (position[0] - c[0], position[1] - c[1]);
            let d = libm::hypot(dx, dy);
            (d > 0.0).then(|| [c[0] + r * dx / d, c[1] + r * dy / d])
        }
        _ => None,
    }
}

/// A line's or an arc's length.
fn curve_length(g: &FcGeometry) -> Option<f64> {
    let at = |k: &str| g.points.get(k).copied();
    match g.kind.as_str() {
        "line" => {
            let (a, b) = (at("1")?, at("2")?);
            Some(libm::hypot(b[0] - a[0], b[1] - a[1]))
        }
        "arc" => {
            let (s, e, c) = (at("1")?, at("2")?, at("3")?);
            let [r, from, to] = arc_from_points(c, s, e)?;
            Some(r * (to - from))
        }
        _ => None,
    }
}

/// Which of FreeCAD's places are one point: joined by a `Coincident`, or by an end-to-end
/// `Tangent` or `Perpendicular`, which implies one.
struct Joins {
    places: BTreeMap<(i32, i32), usize>,
    root: Vec<usize>,
}

impl Joins {
    /// The places `pairs` join, each pair one join.
    fn of_constraints(pairs: impl Iterator<Item = ((i32, i32), (i32, i32))>) -> Joins {
        let mut joins = Joins {
            places: BTreeMap::new(),
            root: Vec::new(),
        };
        for (a, b) in pairs {
            joins.join(a, b);
        }
        joins
    }

    fn of(fc: &FreecadEdited) -> Joins {
        let mut joins = Joins {
            places: BTreeMap::new(),
            root: Vec::new(),
        };
        for (i, geometry) in fc.geometry.iter().enumerate() {
            let g = i32::try_from(i).unwrap_or(i32::MAX);
            for pos in geometry.points.keys() {
                if let Ok(pos) = pos.parse::<i32>() {
                    joins.node((g, pos));
                }
            }
        }
        for c in &fc.constraints {
            let ends = matches!(c.kind.as_str(), "Coincident" | "Tangent" | "Perpendicular");
            if let [[ga, pa], [gb, pb]] = c.refs[..]
                && ends
                && pa != 0
                && pb != 0
            {
                joins.join((ga, pa), (gb, pb));
            }
        }
        joins
    }

    fn node(&mut self, place: (i32, i32)) -> usize {
        if let Some(&n) = self.places.get(&place) {
            return n;
        }
        let n = self.root.len();
        self.root.push(n);
        self.places.insert(place, n);
        n
    }

    fn find(&mut self, mut n: usize) -> usize {
        while self.root[n] != n {
            n = self.root[n];
        }
        n
    }

    fn join(&mut self, a: (i32, i32), b: (i32, i32)) {
        let (a, b) = (self.node(a), self.node(b));
        let (ra, rb) = (self.find(a), self.find(b));
        self.root[ra] = rb;
    }

    fn joined(&self, a: (i32, i32), b: (i32, i32)) -> bool {
        let find = |mut n: usize| {
            while self.root[n] != n {
                n = self.root[n];
            }
            n
        };
        match (self.places.get(&a), self.places.get(&b)) {
            (Some(&a), Some(&b)) => find(a) == find(b),
            _ => false,
        }
    }

    /// Each group of places joined, in order of their first place.
    fn groups(&self) -> Vec<Vec<(i32, i32)>> {
        let find = |mut n: usize| {
            while self.root[n] != n {
                n = self.root[n];
            }
            n
        };
        let mut groups: BTreeMap<usize, Vec<(i32, i32)>> = BTreeMap::new();
        for (place, &n) in &self.places {
            groups.entry(find(n)).or_default().push(*place);
        }
        groups.into_values().collect()
    }
}

/// The kinds whose references are a set, not a sequence.
fn unordered(kind: &str) -> bool {
    matches!(
        kind,
        "Coincident" | "Equal" | "Parallel" | "Perpendicular" | "Tangent" | "Distance"
    ) || kind == "Horizontal"
        || kind == "Vertical"
}

const DIMENSIONS: &[&str] = &[
    "Distance",
    "DistanceX",
    "DistanceY",
    "Radius",
    "Diameter",
    "Angle",
];

/// A constraint as both sides are compared: its kind, what it names in Silico's terms (a
/// curve by ID, a point by ID, a curve's end as `curve@point`), and whether it drives.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    kind: String,
    refs: Vec<String>,
    driving: bool,
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.kind, self.refs.join(", "))?;
        if !self.driving {
            write!(f, " (reference)")?;
        }
        Ok(())
    }
}

/// Names a FreeCAD place in Silico's terms, given which Silico member each geometry index
/// is (`member`) and what the edited sketch is (`shape`), through `renamed` (a merge's
/// points, each to the one it became).
fn name_place(
    shape: &Shape,
    member: &dyn Fn(i32) -> Option<String>,
    renamed: &BTreeMap<String, String>,
    g: i32,
    pos: i32,
    end: bool,
) -> String {
    match (g, pos) {
        (-1, 1) => return "origin".to_owned(),
        (-1, 0) => return "x_axis".to_owned(),
        (-2, 0) => return "y_axis".to_owned(),
        _ => {}
    }
    let Some(m) = member(g) else {
        return format!("FreeCAD's geometry {g}");
    };
    if pos == 0 {
        return m;
    }
    let m = renamed.get(&m).cloned().unwrap_or(m);
    let point = match shape.members.get(&m) {
        Some(Member { kind: "point", .. }) => Some(m.clone()),
        Some(_) => shape.point(&m, pos).cloned(),
        None if shape.points.contains_key(&m) => Some(m.clone()),
        None => None,
    };
    match point {
        Some(p) if end && shape.members.get(&m).is_some_and(|x| x.kind != "point") => {
            format!("{m}@{p}")
        }
        Some(p) => p,
        None => format!("{m}.{pos}"),
    }
}

fn key(
    kind: &str,
    refs: &[[i32; 2]],
    driving: bool,
    name: &dyn Fn(i32, i32, bool) -> String,
) -> Key {
    let ends = matches!(kind, "Tangent" | "Perpendicular");
    let mut named: Vec<String> = refs.iter().map(|[g, p]| name(*g, *p, ends)).collect();
    if unordered(kind) {
        named.sort();
    } else if kind == "Symmetric" && named.len() == 3 {
        named[..2].sort();
    }
    Key {
        kind: kind.to_owned(),
        refs: named,
        driving,
    }
}

/// A translated constraint's references as pairs: which of its `ints` are a geometry and a
/// position, and which a whole curve (position 0), by FreeCAD's argument forms.
fn pairs(kind: &str, ints: &[i32]) -> Vec<[i32; 2]> {
    let pattern: &[bool] = match (kind, ints.len()) {
        // true: a geometry and a position; false: a curve.
        ("Coincident" | "DistanceX" | "DistanceY", 4) => &[true, true],
        ("Horizontal" | "Vertical" | "Distance" | "Radius" | "Diameter", 1) => &[false],
        ("Horizontal" | "Vertical" | "Distance", 4) => &[true, true],
        ("Distance" | "PointOnObject", 3) => &[true, false],
        ("Parallel" | "Perpendicular" | "Equal" | "Angle" | "Tangent", 2) => &[false, false],
        ("Tangent", 3) => &[true, false],
        ("Tangent", 4) => &[true, true],
        ("Symmetric", 5) => &[true, true, false],
        ("Symmetric", 6) => &[true, true, true],
        _ => &[],
    };
    let mut out = Vec::new();
    let mut i = 0;
    for &point in pattern {
        match (point, ints.get(i), ints.get(i + 1)) {
            (true, Some(&g), Some(&p)) => {
                out.push([g, p]);
                i += 2;
            }
            (false, Some(&g), _) => {
                out.push([g, 0]);
                i += 1;
            }
            _ => {}
        }
    }
    out
}

/// What a trim in decision 6's band leaves uncompared (:476): Silico's members (the trimmed
/// curve, its pieces, and the curves in the band), FreeCAD's geometry for them by ID, and
/// the points only they use. Empty for any other edit.
#[derive(Default)]
struct Touched {
    members: BTreeSet<String>,
    ids: BTreeSet<i64>,
    points: BTreeSet<String>,
}

impl Touched {
    /// Whether a reference to a curve's place `pos` (0 the curve itself) names what's
    /// touched: a touched curve itself or at its end (`end`, a tangency's), or a point of one
    /// that no untouched curve uses. `point` is the place's point as the edited sketch names
    /// it, which a FreeCAD piece Silico hasn't leaves unnamed. An untouched curve's point is
    /// compared, whatever else uses it.
    fn names(&self, curve_touched: bool, pos: i32, end: bool, point: &str, after: &Shape) -> bool {
        curve_touched
            && (pos == 0 || end || self.points.contains(point) || !after.points.contains_key(point))
    }
}

/// How Silico's edited sketch and FreeCAD's differ, both edited and solved, but for what
/// `unsettled` leaves unsettled.
fn compare_edited(
    p: &Prepared,
    s: &SilicoEdited,
    fc: &FreecadEdited,
    unsettled: &Unsettled<'_>,
    excused: &mut Vec<String>,
    modulo: &mut Vec<u32>,
) -> Vec<String> {
    let Unsettled { merged, band } = *unsettled;
    let mut how = Vec::new();
    let before = Shape::of(&p.sketch);
    // A split construction circle: FreeCAD 1.1.3's pieces aren't construction, where every
    // piece of Silico's keeps it (:150). Its pieces only: the circle's ID, kept by its
    // start-side piece, and the curves the split made.
    let split_circle = match &p.edit {
        Edit::Split { curve, .. }
            if before
                .members
                .get(curve)
                .is_some_and(|m| m.kind == "circle" && m.construction) =>
        {
            Some(curve.as_str())
        }
        _ => None,
    };
    let after = Shape::of(&s.edited.sketch);
    let renamed: BTreeMap<String, String> = match &p.edit {
        Edit::Merge { points } => match points.split_first() {
            Some((kept, others)) => others.iter().map(|o| (o.clone(), kept.clone())).collect(),
            None => BTreeMap::new(),
        },
        _ => BTreeMap::new(),
    };
    // FreeCAD's geometry IDs to Silico's members: those it had before, by the model's
    // order; the new ones, in order, to Silico's new curves, in the order its edit made them.
    let mut by_id: BTreeMap<i64, String> = BTreeMap::new();
    for (i, id) in fc.ids_before.iter().enumerate() {
        if let Some(e) = p.model.elements.get(i) {
            by_id.insert(*id, e.clone());
        }
    }
    let new_fc: Vec<i64> = fc
        .geometry
        .iter()
        .map(|g| g.id)
        .filter(|id| !fc.ids_before.contains(id))
        .collect();
    let new_ours: Vec<String> = s
        .edited
        .added
        .iter()
        .map(|id| id.as_str().to_owned())
        .filter(|id| {
            after.members.get(id).is_some_and(|m| m.kind != "point")
                && !before.members.contains_key(id)
        })
        .collect();
    // A trim in the band: the trimmed curve, its pieces on both sides, and the curves in the
    // band, and the points only they use, aren't compared (:476).
    let mut touched = Touched::default();
    if let Some(band) = band {
        touched.members = new_ours
            .iter()
            .chain(std::iter::once(&band.trimmed))
            .chain(band.near.keys())
            .cloned()
            .collect();
        touched.ids = new_fc.iter().copied().collect();
        for (i, id) in fc.ids_before.iter().enumerate() {
            if p.model
                .elements
                .get(i)
                .is_some_and(|e| touched.members.contains(e))
            {
                touched.ids.insert(*id);
            }
        }
        let used = |want: bool| -> BTreeSet<String> {
            [&before, &after]
                .iter()
                .flat_map(|shape| shape.members.iter())
                .filter(|(id, _)| touched.members.contains(*id) == want)
                .flat_map(|(_, m)| m.at.values().cloned())
                .collect()
        };
        let elsewhere = used(false);
        touched.points = used(true)
            .into_iter()
            .filter(|point| !elsewhere.contains(point))
            .collect();
    }
    if new_fc.len() == new_ours.len() {
        for (id, ours) in new_fc.iter().zip(&new_ours) {
            by_id.insert(*id, ours.clone());
        }
    } else if band.is_none() {
        how.push(format!(
            "Silico's edit made {} new curves ({}), FreeCAD's {}",
            new_ours.len(),
            new_ours.join(", "),
            new_fc.len()
        ));
    }
    let fc_member = |g: i32| -> Option<String> {
        usize::try_from(g)
            .ok()
            .and_then(|i| fc.geometry.get(i))
            .and_then(|geo| by_id.get(&geo.id).cloned())
    };
    let fc_name_place =
        |g: i32, pos: i32, end: bool| name_place(&after, &fc_member, &renamed, g, pos, end);
    let fc_touched = |g: i32| {
        usize::try_from(g)
            .ok()
            .and_then(|i| fc.geometry.get(i))
            .is_some_and(|geo| touched.ids.contains(&geo.id))
    };
    // Whether a FreeCAD reference names what's touched.
    let fc_touches = |&[g, pos]: &[i32; 2], end: bool| {
        touched.names(
            fc_touched(g),
            pos,
            end,
            &fc_name_place(g, pos, false),
            &after,
        )
    };
    // The elements: each of Silico's curves and lone points has FreeCAD geometry, of its
    // kind and construction, and FreeCAD has nothing more.
    let mut seen = BTreeSet::new();
    for (i, g) in fc.geometry.iter().enumerate() {
        if touched.ids.contains(&g.id) {
            if let Some(m) = by_id.get(&g.id) {
                seen.insert(renamed.get(m).unwrap_or(m).clone());
            }
            continue;
        }
        let Some(m) = by_id.get(&g.id) else {
            how.push(format!(
                "FreeCAD has a {} ({:?}) that Silico hasn't",
                g.kind, g.points
            ));
            continue;
        };
        let m = renamed.get(m).unwrap_or(m);
        seen.insert(m.clone());
        match after.members.get(m) {
            Some(member) => {
                if member.kind != g.kind {
                    how.push(format!(
                        "Silico's {m} is a {}, FreeCAD's a {}",
                        member.kind, g.kind
                    ));
                }
                let piece = split_circle
                    .is_some_and(|circle| m == circle || new_ours.iter().any(|n| n == m));
                if piece && member.construction && !g.construction {
                    excused.push(format!(
                        "Silico's {m} is construction, FreeCAD's isn't: FreeCAD 1.1.3's split of a construction circle drops it, where Silico's pieces keep it ({NOTE}:150)"
                    ));
                    modulo.push(150);
                } else if member.construction != g.construction {
                    how.push(format!(
                        "Silico's {m} is {}construction, FreeCAD's {}",
                        if member.construction { "" } else { "not " },
                        if g.construction { "is" } else { "isn't" }
                    ));
                }
            }
            // A lone point of FreeCAD's that Silico's curves now use (a split at it).
            None if g.kind == "point" && after.points.contains_key(m) => {}
            None => how.push(format!(
                "FreeCAD keeps {} {m} (geometry {i}), which Silico's edit removed",
                g.kind
            )),
        }
    }
    for (m, member) in &after.members {
        if !seen.contains(m) && !touched.members.contains(m) {
            how.push(format!(
                "Silico has {} {m}, which FreeCAD hasn't",
                member.kind
            ));
        }
    }
    // Which ends are one point (D2: a Silico shared point is FreeCAD's coincident ends,
    // :25): each of Silico's points one group of FreeCAD's places, each group one point.
    let joins = Joins::of(fc);
    let describe = |(g, pos): (i32, i32)| -> String {
        let role = match pos {
            1 => "start",
            2 => "end",
            3 => "centre",
            _ => "edge",
        };
        match fc_member(g) {
            Some(m) => format!("{m}'s {role}"),
            None if g < 0 => format!("FreeCAD's {g}"),
            None => format!("FreeCAD's geometry {g}'s {role}"),
        }
    };
    let mut groups_of: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    // The places on touched curves left out: an untouched curve's are compared, so a point
    // it shares with a touched one is too.
    let groups: Vec<Vec<(i32, i32)>> = joins
        .groups()
        .into_iter()
        .map(|group| group.into_iter().filter(|&(g, _)| !fc_touched(g)).collect())
        .collect();
    for (n, group) in groups.iter().enumerate() {
        let points: BTreeSet<String> = group
            .iter()
            .map(|&(g, pos)| fc_name_place(g, pos, false))
            .filter(|name| after.points.contains_key(name) || name == "origin")
            .collect();
        if points.len() > 1 {
            let places: Vec<String> = group.iter().map(|&pl| describe(pl)).collect();
            how.push(format!(
                "FreeCAD joins {}, which are Silico's points {} apart ({NOTE}:25)",
                places.join(", "),
                points.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        for point in group.iter().map(|&(g, pos)| fc_name_place(g, pos, false)) {
            let list = groups_of.entry(point).or_default();
            if !list.contains(&n) {
                list.push(n);
            }
        }
    }
    for (point, list) in &groups_of {
        if list.len() > 1 && after.points.contains_key(point) {
            let split: Vec<String> = list
                .iter()
                .filter_map(|&n| groups.get(n))
                .map(|g| {
                    let places: Vec<String> = g.iter().map(|&pl| describe(pl)).collect();
                    format!("[{}]", places.join(", "))
                })
                .collect();
            how.push(format!(
                "Silico's point {point} is one point, which FreeCAD has apart: {} ({NOTE}:25)",
                split.join(" ")
            ));
        }
    }
    // Where the edit put things, before either solve: every point but a merge's from
    // apart, which Silico places and FreeCAD's coincidences pull (:270). FreeCAD solves as
    // it adds a constraint, so a merge's FreeCAD side before the solve is the sketch as it
    // was: its points where the case drew them.
    let exempt = |point: &str| merged.is_some_and(|m| m.contains(point));
    if merged.is_some() {
        for (id, ours) in &after.points {
            if exempt(id) {
                continue;
            }
            if let Some(drawn) = before.points.get(id)
                && ((ours[0] - drawn[0]).abs() > sketch::TOLERANCE
                    || (ours[1] - drawn[1]).abs() > sketch::TOLERANCE)
            {
                how.push(format!(
                    "after the edit, before the solve, Silico's point {id} is at {ours:?}, where FreeCAD's merge starts it from {drawn:?}"
                ));
            }
        }
    } else {
        for (i, g) in fc.edited.iter().enumerate() {
            let gi = i32::try_from(i).unwrap_or(i32::MAX);
            if touched.ids.contains(&g.id) {
                continue;
            }
            for (pos, at) in &g.points {
                let Ok(pos) = pos.parse::<i32>() else {
                    continue;
                };
                let point = fc_name_place(gi, pos, false);
                if exempt(&point) {
                    continue;
                }
                if let Some(ours) = after.points.get(&point)
                    && ((ours[0] - at[0]).abs() > sketch::TOLERANCE
                        || (ours[1] - at[1]).abs() > sketch::TOLERANCE)
                {
                    how.push(format!(
                        "after the edit, before the solve, Silico's point {point} is at {ours:?}, FreeCAD's {} at {at:?}",
                        describe((gi, pos))
                    ));
                }
            }
            if let (Some(m), Some(r)) = (fc_member(gi), g.radius)
                && let Some(ours) = after.radii.get(&m)
                && (ours - r).abs() > sketch::TOLERANCE
            {
                how.push(format!(
                    "after the edit, before the solve, Silico's {m} has radius {ours}, FreeCAD's {r}"
                ));
            }
        }
    }
    // The constraints, by what they are and name.
    let model = &s.model;
    let ours_member = |g: i32| -> Option<String> {
        usize::try_from(g)
            .ok()
            .and_then(|i| model.elements.get(i))
            .cloned()
    };
    let none = BTreeMap::new();
    let ours_name =
        |g: i32, pos: i32, end: bool| name_place(&after, &ours_member, &none, g, pos, end);
    let ours_touches = |&[g, pos]: &[i32; 2], end: bool| {
        let curve = ours_member(g).is_some_and(|m| touched.members.contains(&m));
        touched.names(curve, pos, end, &ours_name(g, pos, false), &after)
    };
    let ends = |kind: &str| matches!(kind, "Tangent" | "Perpendicular");
    // Silico's constraints that name what's touched, by ID, and FreeCAD's, by index: neither
    // compared.
    let mut ours_touched: BTreeSet<String> = BTreeSet::new();
    let mut theirs_touched: BTreeSet<usize> = BTreeSet::new();
    let mut ours_keys: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    for (i, c) in model.constraints.iter().enumerate() {
        let Some(id) = &c.id else {
            continue;
        };
        let refs = pairs(c.kind, &c.ints);
        if refs.iter().any(|r| ours_touches(r, ends(c.kind))) {
            ours_touched.insert(id.clone());
            continue;
        }
        let k = key(c.kind, &refs, c.driving, &ours_name);
        ours_keys.entry(k).or_default().push(i);
    }
    let mut theirs_keys: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    for (i, c) in fc.constraints.iter().enumerate() {
        if c.kind == "Coincident" {
            continue;
        }
        if c.refs.iter().any(|r| fc_touches(r, ends(&c.kind))) {
            theirs_touched.insert(i);
            continue;
        }
        let k = key(&c.kind, &c.refs, c.driving, &fc_name_place);
        theirs_keys.entry(k).or_default().push(i);
    }
    // FreeCAD's index to the model's, for the matched.
    let mut to_model: BTreeMap<usize, usize> = BTreeMap::new();
    let keys: BTreeSet<&Key> = ours_keys.keys().chain(theirs_keys.keys()).collect();
    for k in keys {
        let o = ours_keys.get(k).map_or(&[][..], Vec::as_slice);
        let t = theirs_keys.get(k).map_or(&[][..], Vec::as_slice);
        for (&oi, &ti) in o.iter().zip(t) {
            to_model.insert(ti, oi);
            let (oc, tc) = (&model.constraints[oi], &fc.constraints[ti]);
            if DIMENSIONS.contains(&oc.kind)
                && oc.driving
                && let Some(v) = oc.value
                && (v - tc.value).abs() > VALUE_TOLERANCE
            {
                how.push(format!(
                    "{k} is {v} in Silico's ({}), {} in FreeCAD's",
                    oc.id.as_deref().unwrap_or("?"),
                    tc.value
                ));
            }
        }
        for &oi in o.iter().skip(t.len()) {
            how.push(format!(
                "Silico has {k} ({}), FreeCAD hasn't",
                model.constraints[oi].id.as_deref().unwrap_or("?")
            ));
        }
        for &ti in t.iter().skip(o.len()) {
            let name = fc.constraints[ti].name.trim_start_matches("s_");
            let named = if name.is_empty() {
                String::new()
            } else {
                format!(" (from {name})")
            };
            how.push(format!("FreeCAD has {k}{named}, Silico hasn't"));
        }
    }
    // The solve, by the sketch oracle's rules, FreeCAD's results in the model's terms.
    let shift = |list: &[usize]| -> Vec<usize> {
        list.iter()
            .filter_map(|&i| {
                i.checked_sub(1)
                    .and_then(|i| to_model.get(&i))
                    .map(|m| m + 1)
            })
            .collect()
    };
    for (name, list) in [
        ("conflicting", &fc.after.conflicting),
        ("redundant", &fc.after.redundant),
        ("partially redundant", &fc.after.partially_redundant),
        ("malformed", &fc.after.malformed),
    ] {
        for &i in list {
            let Some(c) = i.checked_sub(1).and_then(|i| fc.constraints.get(i)) else {
                continue;
            };
            if c.kind != "Coincident"
                && !to_model.contains_key(&(i - 1))
                && !theirs_touched.contains(&(i - 1))
            {
                let k = key(&c.kind, &c.refs, c.driving, &fc_name_place);
                how.push(format!(
                    "FreeCAD finds its {k}, which Silico hasn't, {name}"
                ));
            }
        }
    }
    let mut points = BTreeMap::new();
    for (i, g) in fc.geometry.iter().enumerate() {
        let gi = i32::try_from(i).unwrap_or(i32::MAX);
        for (pos, at) in &g.points {
            if let Ok(pos) = pos.parse::<i32>() {
                points.entry(fc_name_place(gi, pos, false)).or_insert(*at);
            }
        }
    }
    let mut radii = BTreeMap::new();
    for circle in model.circles.keys() {
        let found = fc
            .geometry
            .iter()
            .enumerate()
            .find(|(i, _)| {
                fc_member(i32::try_from(*i).unwrap_or(i32::MAX)).as_ref() == Some(circle)
            })
            .and_then(|(_, geo)| geo.radius);
        if let Some(r) = found {
            radii.insert(circle.clone(), r);
        }
    }
    let mut values = vec![f64::NAN; model.constraints.len()];
    for (&ti, &oi) in &to_model {
        if let (Some(v), Some(c)) = (values.get_mut(oi), fc.constraints.get(ti)) {
            *v = c.value;
        }
    }
    let theirs = FreecadSolved {
        code: fc.after.code,
        dof: fc.after.dof,
        conflicting: shift(&fc.after.conflicting),
        redundant: shift(&fc.after.redundant),
        partially_redundant: shift(&fc.after.partially_redundant),
        malformed: shift(&fc.after.malformed),
        points,
        radii,
        values,
    };
    // Silico's solve less what's touched: its touched constraints in no list, its touched
    // points and radii not compared.
    let mut ours = s.ours.clone();
    for list in [
        &mut ours.conflicting,
        &mut ours.redundant,
        &mut ours.partially_redundant,
    ] {
        list.retain(|id| !ours_touched.contains(id));
    }
    if let Ok(solution) = &mut ours.outcome {
        solution
            .points
            .retain(|id, _| !touched.points.contains(id.as_str()));
        solution
            .radii
            .retain(|id, _| !touched.members.contains(id.as_str()));
    }
    how.extend(sketch::compare_outcomes(
        model,
        &ours,
        &theirs,
        merged.is_none(),
    ));
    // A merge from apart: after the solve, every point neither solve moved, but the merged
    // one (:270); what the solves move from different starts isn't compared by place.
    if let (Some(merged), Ok(solution), 0) = (merged, &s.ours.outcome, fc.after.code) {
        let near = |a: [f64; 2], b: [f64; 2]| {
            (a[0] - b[0]).abs() <= sketch::TOLERANCE && (a[1] - b[1]).abs() <= sketch::TOLERANCE
        };
        // Each point's places in FreeCAD's result, where the case drew the point and where
        // FreeCAD's solves left it.
        // Where the case drew it, and where FreeCAD left it.
        type Places = Vec<([f64; 2], [f64; 2])>;
        let mut places: BTreeMap<String, Places> = BTreeMap::new();
        for (i, solved) in fc.geometry.iter().enumerate() {
            let gi = i32::try_from(i).unwrap_or(i32::MAX);
            for (pos, at) in &solved.points {
                let Ok(n) = pos.parse::<i32>() else {
                    continue;
                };
                let id = fc_name_place(gi, n, false);
                if let Some(was) = before.points.get(&id) {
                    places.entry(id).or_default().push((*was, *at));
                }
            }
        }
        for (id, [x, y]) in &solution.points {
            let ours = [x.mm(), y.mm()];
            let Some(start) = after.points.get(id.as_str()) else {
                continue;
            };
            let Some(theirs) = places.get(id.as_str()) else {
                continue;
            };
            let moved = !near(ours, *start) || theirs.iter().any(|(was, at)| !near(*was, *at));
            if merged.contains(id.as_str()) || moved {
                continue;
            }
            if let Some((_, at)) = theirs.iter().find(|(_, at)| !near(ours, *at)) {
                how.push(format!(
                    "after the solve, Silico's point {id}, which neither solve moved, is at {ours:?}, FreeCAD's at {at:?}"
                ));
            }
        }
    }
    how
}
