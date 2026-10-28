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

/// What Silico made of a case.
#[derive(Debug, Clone)]
pub enum SilicoSide {
    /// The edit refused, as its error says.
    Refused(String),
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
pub fn silico_edit(sketch: &Sketch, edit: &Edit) -> Result<Result<Edited, String>, String> {
    let ids = |list: &[String]| {
        list.iter()
            .map(|s| element_id(s))
            .collect::<Result<Vec<_>, _>>()
    };
    let refused = |e: silico_core::Error| e.to_string();
    Ok(match edit {
        Edit::Remove { members } => sketch_edit::remove(sketch, &ids(members)?).map_err(|r| {
            r.errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
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
    // (:95, :210, :233-234); FreeCAD's own rules then have nothing to move.
    let (away, line): (BTreeSet<&str>, u32) = match edit {
        Edit::Remove { members } => (members.iter().map(String::as_str).collect(), 95),
        Edit::Unjoin { curve, .. } => (BTreeSet::from([curve.as_str()]), 233),
        Edit::Trim { curve, .. } => (BTreeSet::from([curve.as_str()]), 210),
        Edit::Split { curve, .. } => (BTreeSet::from([curve.as_str()]), 147),
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
                        return Plan::Refuses(format!(
                            "FreeCAD has no edit that deletes a point curves end at, and Silico refuses it ({NOTE}:107-108, decision 7)"
                        ));
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
            let mut seen = BTreeSet::new();
            for p in points {
                if !seen.insert(p) {
                    return Plan::Refuses(format!(
                        "point {p} is named twice: FreeCAD's Coincident command refuses points already coincident ({NOTE}:51)"
                    ));
                }
            }
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
                pick: *pick,
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
/// repair for it, :181), the coincidences of a point whose hub (the end every other is
/// joined to) is such an end joined to another end instead, so a trim that takes the hub's
/// end away doesn't part the curves that keep the point (:210). And whether any moved.
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
    /// Its edit raised, as it says.
    Failed(String),
    /// It crashed or hung.
    Lost(String),
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
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.case, self.how.join("; "))?;
        if !self.modulo.is_empty() {
            let lines: Vec<String> = self.modulo.iter().map(|l| format!(":{l}")).collect();
            write!(f, " (compared modulo {NOTE}{})", lines.join(", "))?;
        }
        Ok(())
    }
}

/// A case of `tests/oracle/sketch-edits` whose difference from FreeCAD the port note doesn't
/// explain, kept for a decision (a fix to the edit, or a line in the note): shown as known,
/// not as agreement, by its whole report, so any other difference in the case still shows;
/// and one that no longer shows is a problem, so the list can't go stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    pub case: &'static str,
    /// The case's report ([`Verdict::how`], joined by "; "), exactly.
    pub difference: &'static str,
    pub why: &'static str,
}

/// The case file's differences the note doesn't explain, each reported with its judgment.
pub const KNOWN: &[Known] = &[
    Known {
        case: "split_line_tangent_on_second_half",
        difference: "FreeCAD has Tangent(a0000008@p0000006, n0000003) (from k0000009), Silico hasn't; redundant [] against FreeCAD's [\"k0000009\"]",
        why: "FreeCAD 1.1.3 copies a tangent_on onto the split line to both pieces, one of them then redundant; the note's table (:143) has 1.1.3 as not probed, and Silico follows main (the piece holding `at`)",
    },
    Known {
        case: "split_circle_tangent_on",
        difference: "Silico has Tangent(c0000002, n0000005@p0000003) (k0000006), FreeCAD hasn't; degrees of freedom 7 against FreeCAD's 9",
        why: "FreeCAD 1.1.3 drops a tangent_on onto a circle or arc it splits (as it drops an arc's edge tangency, c823a6b270, :326), where it copies one onto a split line to both pieces; the note's table (:143) has 1.1.3 as not probed, and Silico follows main (the piece holding `at`)",
    },
    Known {
        case: "trim_arc_tangent_no_piece_touches",
        difference: "Silico has Tangent(a0000004, n0000007) (k0000008), FreeCAD hasn't; degrees of freedom 17 against FreeCAD's 18",
        why: "the arc's edge tangency touches its circle at 270 degrees, past the arc's ends: Silico's trim keeps it on the piece with the nearer end, which doesn't touch the line either; decision 2 (:214, :443) says a trim refuses an arc's edge tangent that no piece touches, and FreeCAD drops it",
    },
    Known {
        case: "remove_line_with_midpoint",
        difference: "FreeCAD has Symmetric(p0000001, p0000002, p0000009) (from k0000010), Silico hasn't; degrees of freedom 10 against FreeCAD's 8",
        why: "FreeCAD's midpoint is a Symmetric of the line's two ends, which the sides keep, so it survives the line's deletion; Silico's midpoint names the line and goes with it; the note doesn't say (its split turns a midpoint into a symmetric of the outer ends, :140)",
    },
    Known {
        case: "trim_perpendicular_cutter",
        difference: "Silico has Perpendicular(n0000003, n0000006) (k0000008), FreeCAD hasn't; FreeCAD has Perpendicular(n0000003@w0000000, n0000006) (from k0000008), Silico hasn't; Silico has PointOnObject(w0000000, n0000006) (w0000001), FreeCAD hasn't",
        why: "FreeCAD 1.1.3's trim turns the line's edge Perpendicular with its cutter into an endpoint-to-edge Perpendicular at the new end, as dec01d1df0 does a tangency (:179); Silico keeps the perpendicular between directions (D4) and puts the new end on the cutter: the same conditions, said otherwise (the degrees of freedom agree); the note says it only of a tangency",
    },
    Known {
        case: "trim_perpendicular_cutter_into_two",
        difference: "Silico has Perpendicular(n0000003, n0000006) (k0000011), FreeCAD hasn't; Silico has Perpendicular(n0000006, w0000002) (w0000004), FreeCAD hasn't; FreeCAD has Perpendicular(n0000006, w0000002@w0000001) (from k0000011), Silico hasn't; Silico has PointOnObject(w0000001, n0000006) (w0000006), FreeCAD hasn't; redundant [\"w0000004\"] against FreeCAD's []",
        why: "trimmed into two between a slanted cutter and the upright it's perpendicular to, FreeCAD 1.1.3 moves the perpendicular to the cut, as an end perpendicular of the second piece, and leaves none on the first; Silico copies it to both pieces (:183 lists a perpendicular among what goes to every line piece), and with both pieces horizontal the copy is redundant: Silico's trim makes a redundant constraint FreeCAD's doesn't",
    },
    Known {
        case: "merge_two_locked_ends",
        difference: "FreeCAD refused (FreeCAD's Coincident command rolls back a worse solve (docs/ports/sketch-edit.md:53): solve -2 with conflicting [], redundant [3, 4], malformed []); Silico made the edit",
        why: "the two locks agree once the points are one, so Silico's solve keeps them as redundant (M3.3's decision 5) and the merge stands (as the note's own merge tests have a lock merge, :423); FreeCAD's Coincident command rolls back a solve that finds new redundancy (:53); the note says only that a conflict refuses the merge (:272)",
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
        if v.how.join("; ") != k.difference {
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
            Some(k) if v.how.join("; ") == k.difference => {
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
    // FreeCAD's refusal, if it refused: its edit raised, it crashed, its plan refuses, or
    // (a merge) its Coincident command would refuse what the edit made.
    let fc_refusal: Option<String> = match (theirs, &p.plan) {
        (_, Plan::Refuses(why)) => Some(why.clone()),
        (Some(Theirs::Failed(why)), _) => Some(format!("its edit raised: {why}")),
        (Some(Theirs::Lost(why)), _) => Some(format!("it crashed or hung: {why}")),
        (Some(Theirs::Edited(fc)), _) => merge_refusal(p, fc),
        (None, Plan::Run { .. }) => Some("FreeCAD gave no outcome".to_owned()),
    };
    let silico_refusal: Option<String> = match &p.silico {
        SilicoSide::Refused(why) => Some(format!("its edit refused: {why}")),
        SilicoSide::Gated(why) => Some(format!(
            "the gate refused its edit's result: {}",
            why.join("; ")
        )),
        SilicoSide::Edited(s) => match (&p.edit, &s.ours.outcome) {
            // A merge's solve settles it: a conflict refuses the edit (:272).
            (Edit::Merge { .. }, Err(e)) => {
                v.modulo.push(272);
                Some(format!("its merge's solve refused: {e}"))
            }
            _ => None,
        },
    };
    match (silico_refusal, fc_refusal) {
        (Some(ours), Some(_)) => {
            v.refused = true;
            if let SilicoSide::Gated(_) = &p.silico {
                v.excused
                    .push(format!("both refused, but Silico by the gate: {ours}"));
            }
        }
        (Some(ours), None) => {
            let Some(Theirs::Edited(fc)) = theirs else {
                v.how
                    .push(format!("Silico refused ({ours}); FreeCAD gave nothing"));
                return v;
            };
            match excuse_refusal(p, fc) {
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
                match skipped(p) {
                    Some(excuse) => v.excused.push(excuse),
                    None => {
                        let geometry = merge_apart(p).is_none();
                        if let Some(why) = merge_apart(p) {
                            v.excused.push(why);
                            v.modulo.push(266);
                        }
                        v.how.extend(compare_edited(p, s, fc, geometry));
                    }
                }
            }
        }
    }
    v.modulo.sort_unstable();
    v.modulo.dedup();
    v.modulo.retain(|l| *l != 0);
    v
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

/// Why Silico's refusal of an edit FreeCAD made is the note's, if it is.
fn excuse_refusal(p: &Prepared, fc: &FreecadEdited) -> Option<String> {
    let shape = Shape::of(&p.sketch);
    match &p.edit {
        Edit::Split { curve, at } => {
            let member = shape.members.get(curve)?;
            if member.kind == "circle" && at.len() == 1 {
                return Some(format!(
                    "a circle split at one position: Silico refuses it (decision 3, {NOTE}:444), FreeCAD makes one arc whose ends aren't joined (:120)"
                ));
            }
            for position in at {
                let off = off_curve(&shape, curve, *position)?;
                if off > LENGTH_TOLERANCE {
                    return Some(format!(
                        "a split position {off:e} mm off the curve: Silico refuses it (decision 4, {NOTE}:445), FreeCAD splits at its projection (:115)"
                    ));
                }
            }
            let shortest = fc
                .edited
                .iter()
                .filter_map(curve_length)
                .fold(f64::INFINITY, f64::min);
            (shortest <= LENGTH_TOLERANCE).then(|| {
                format!(
                    "a split within 1e-7 mm of an end: Silico refuses it ({NOTE}:151), FreeCAD 1.1.3 makes a piece {shortest:e} mm long (:117)"
                )
            })
        }
        Edit::Trim { curve, .. } => {
            let member = shape.members.get(curve)?;
            if member.kind == "point" {
                return Some(format!(
                    "a trim of a point: Silico refuses it ({NOTE}:301), FreeCAD 1.1.3 deletes it (:157)"
                ));
            }
            // Decision 2: a constraint no piece can take, which FreeCAD drops.
            let round = matches!(member.kind, "arc" | "circle");
            let names: BTreeSet<&str> = fc.constraints.iter().map(|c| c.name.as_str()).collect();
            let curve_ref = Reference::Element(element_id(curve).ok()?);
            p.sketch.constraints.iter().find_map(|c| {
                let cannot = match &c.form {
                    Form::Symmetric { about, .. } => *about == curve_ref,
                    Form::Tangent { a, b, .. } => round && (*a == curve_ref || *b == curve_ref),
                    _ => false,
                };
                (cannot && !names.contains(fc_name(c.id.as_str()).as_str())).then(|| {
                    format!(
                        "constraint {} no piece can take: Silico refuses the trim (decision 2, {NOTE}:214, :443), FreeCAD drops it",
                        c.id
                    )
                })
            })
        }
        Edit::Unjoin { point, .. } => {
            let sent = match &p.plan {
                Plan::Run { job, .. } => job.constraints.len(),
                Plan::Refuses(_) => return None,
            };
            (shape.users(point).len() < 2 && fc.constraints.len() == sent).then(|| {
                format!(
                    "unjoining point {point}, which no other curve shares: Silico refuses it ({NOTE}:235), FreeCAD's vertex Delete does nothing (:230)"
                )
            })
        }
        Edit::Merge { points } => {
            // An edge tangent between two curves that would then share an end (:271).
            let kept = points.first()?;
            let merged: BTreeSet<&str> = points.iter().map(String::as_str).collect();
            let ends_at = |curve: &str| {
                shape.members.get(curve).is_some_and(|m| {
                    m.kind != "point"
                        && [1, 2]
                            .iter()
                            .filter_map(|pos| m.at.get(pos))
                            .any(|p| merged.contains(p.as_str()))
                })
            };
            p.sketch.constraints.iter().find_map(|c| match &c.form {
                Form::Tangent { a, b, .. }
                    if a.element().is_some_and(|a| ends_at(a.as_str()))
                        && b.element().is_some_and(|b| ends_at(b.as_str())) =>
                {
                    Some(format!(
                        "merging into {kept} leaves edge tangent {} at a shared end: Silico refuses it (decision 8, {NOTE}:271), FreeCAD's App keeps both",
                        c.id
                    ))
                }
                _ => None,
            })
        }
        Edit::Remove { .. } => None,
    }
}

/// Whether the note says the two can't be compared past their outcomes: an arc's pick
/// outside its sweep, which Silico places at the nearer end and FreeCAD 1.1.3 doesn't
/// (:217).
fn skipped(p: &Prepared) -> Option<String> {
    let Edit::Trim { curve, pick } = &p.edit else {
        return None;
    };
    let arc = p.sketch.arcs.iter().find(|a| a.id.as_str() == curve)?;
    let shape = Shape::of(&p.sketch);
    let at = |id: &ElementId| shape.points.get(id.as_str()).copied();
    let center = at(&arc.center)?;
    let [_, from, to] = arc_from_points(center, at(&arc.start)?, at(&arc.end)?)?;
    let mut angle = libm::atan2(pick[1] - center[1], pick[0] - center[0]);
    while angle < from {
        angle += TAU;
    }
    (angle > to).then(|| {
        format!(
            "a pick outside arc {curve}'s sweep: Silico trims at the nearer end, FreeCAD 1.1.3 otherwise ({NOTE}:217); only the outcomes compared"
        )
    })
}

/// For a merge of points apart: Silico places the kept point, FreeCAD's coincidences are
/// solved (:266), so where the points end up isn't compared.
fn merge_apart(p: &Prepared) -> Option<String> {
    let Edit::Merge { points } = &p.edit else {
        return None;
    };
    let shape = Shape::of(&p.sketch);
    let kept = shape.points.get(points.first()?)?;
    points[1..]
        .iter()
        .filter_map(|q| shape.points.get(q))
        .any(|q| q != kept)
        .then(|| {
            format!(
                "points merged from apart: Silico places them, FreeCAD's coincidences are solved ({NOTE}:266); where the points end up isn't compared"
            )
        })
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

/// How Silico's edited sketch and FreeCAD's differ, both edited and solved; their solved
/// geometry compared only if `geometry`.
fn compare_edited(
    p: &Prepared,
    s: &SilicoEdited,
    fc: &FreecadEdited,
    geometry: bool,
) -> Vec<String> {
    let mut how = Vec::new();
    let before = Shape::of(&p.sketch);
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
    if new_fc.len() == new_ours.len() {
        for (id, ours) in new_fc.iter().zip(&new_ours) {
            by_id.insert(*id, ours.clone());
        }
    } else {
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
    // The elements: each of Silico's curves and lone points has FreeCAD geometry, of its
    // kind and construction, and FreeCAD has nothing more.
    let mut seen = BTreeSet::new();
    for (i, g) in fc.geometry.iter().enumerate() {
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
                if member.construction != g.construction {
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
        if !seen.contains(m) {
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
    for (n, group) in joins.groups().iter().enumerate() {
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
    let groups = joins.groups();
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
    // Where the edit put things, before either solve.
    if geometry {
        for (i, g) in fc.edited.iter().enumerate() {
            let gi = i32::try_from(i).unwrap_or(i32::MAX);
            for (pos, at) in &g.points {
                let Ok(pos) = pos.parse::<i32>() else {
                    continue;
                };
                let point = fc_name_place(gi, pos, false);
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
    let mut ours_keys: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    for (i, c) in model.constraints.iter().enumerate() {
        if c.id.is_none() {
            continue;
        }
        let k = key(c.kind, &pairs(c.kind, &c.ints), c.driving, &ours_name);
        ours_keys.entry(k).or_default().push(i);
    }
    let mut theirs_keys: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    for (i, c) in fc.constraints.iter().enumerate() {
        if c.kind == "Coincident" {
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
            if c.kind != "Coincident" && !to_model.contains_key(&(i - 1)) {
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
    how.extend(sketch::compare_outcomes(model, &s.ours, &theirs, geometry));
    how
}
