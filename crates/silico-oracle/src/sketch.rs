//! Sketch cases (M3.3, docs/design/m3.3-solving.md, *The oracle*): each sketch feature of a
//! Silico document, solved through Silico's sketch worker and by headless FreeCAD 1.1.3's
//! `sketch.solve()`, built from the same sketch with FreeCAD's model (each shared point as
//! coincident ends). Compared: the degrees of freedom, the conflicting, redundant, and
//! partially redundant constraints (FreeCAD's coincidences left out), how each side ends,
//! and, where both solve, every point, every circle's radius, every reference value, and
//! the direction FreeCAD locked each tangency at an end to.

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_PI_2, TAU};
use std::path::Path;

use serde::{Deserialize, Serialize};
use silico_core::document::{Checked, ElementId, Form, Operation, Reference, Sketch, read};
use silico_core::sketch::{Measured, SketchSolution, SketchSolver, Translation};
use silico_core::{Error, ErrorCode};
use silico_protocol::{Tag, arc_from_points};

use crate::freecad;

/// The file of sketch cases in an oracle directory: a Silico document, each sketch feature
/// a case (its name the case's, its note what it checks).
pub const FILE: &str = "sketches.silico";

/// How far apart Silico's and FreeCAD's points may be: both solvers stop at residuals of
/// 1e-10, from different paths (FreeCAD's coincidences are equations Silico doesn't have),
/// and their builds differ in the last bits.
pub const TOLERANCE: f64 = 1e-8;

/// One sketch case.
#[derive(Debug, Clone)]
pub struct SketchCase {
    pub name: String,
    pub note: String,
    pub sketch: Sketch,
}

/// The sketch cases in `dir`'s [`FILE`], none if it has none.
pub fn load(dir: &Path) -> Result<Vec<SketchCase>, String> {
    let path = dir.join(FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("can't read {}: {e}", path.display())),
    };
    let document = read(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let checked = Checked::new(document).map_err(|invalid| {
        let errors: Vec<String> = invalid.errors().iter().map(ToString::to_string).collect();
        format!("{}: {}", path.display(), errors.join("; "))
    })?;
    let mut cases = Vec::new();
    for feature in &checked.document().features {
        let Operation::Sketch(sketch) = &feature.operation else {
            return Err(format!(
                "{}: feature {} isn't a sketch",
                path.display(),
                feature.id.as_str()
            ));
        };
        cases.push(SketchCase {
            name: feature.name.as_str().to_owned(),
            note: feature.note.clone().unwrap_or_default(),
            sketch: sketch.clone(),
        });
    }
    Ok(cases)
}

/// A sketch in FreeCAD's model: its geometry, each shared point's ends, and Silico's
/// constraints as FreeCAD's, then the coincidences.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FreecadSketch {
    pub geometry: Vec<FreecadGeometry>,
    /// Silico's constraints first, in Silico's order (sorted by ID), then the
    /// coincidences: FreeCAD's index of each is its place here, plus one.
    pub constraints: Vec<FreecadConstraint>,
    /// Each point, by ID, where FreeCAD has it: a geometry and a position (1 start, 2 end,
    /// 3 centre).
    pub points: BTreeMap<String, (i32, i32)>,
    /// Each circle, by ID, its geometry.
    pub circles: BTreeMap<String, i32>,
    /// Each geometry's Silico element, by FreeCAD's index: a curve, or a point no curve
    /// uses. Not sent: the harness's own, for matching results (the sketch edits' oracle).
    #[serde(skip)]
    pub elements: Vec<String>,
    /// Every place FreeCAD has each point, by ID: each curve's end or centre that is it, in
    /// geometry order, the first being [`FreecadSketch::points`]'. Not sent.
    #[serde(skip)]
    pub places: BTreeMap<String, Vec<(i32, i32)>>,
}

/// A piece of FreeCAD geometry.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FreecadGeometry {
    Point {
        at: [f64; 2],
    },
    Line {
        start: [f64; 2],
        end: [f64; 2],
    },
    Circle {
        center: [f64; 2],
        radius: f64,
    },
    /// Counter-clockwise from `from` to `to` (radians), as Silico's arc is.
    Arc {
        center: [f64; 2],
        radius: f64,
        from: f64,
        to: f64,
    },
}

/// A FreeCAD constraint: `Sketcher.Constraint(kind, *ints, value)`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FreecadConstraint {
    pub kind: &'static str,
    pub ints: Vec<i32>,
    pub value: Option<f64>,
    pub driving: bool,
    /// The Silico constraint's ID; none for a coincidence.
    pub id: Option<String>,
    /// For a tangency at an end or a point: the value FreeCAD should lock it to, from
    /// Silico's direction (the angle between the curves, less π/2).
    pub locked: Option<f64>,
}

/// `sketch` in FreeCAD's model, with `translation` (Silico's own of it) for the tangencies'
/// directions, which FreeCAD locks from the drawing on its own and the harness checks.
pub fn freecad_sketch(sketch: &Sketch, translation: &Translation) -> Result<FreecadSketch, String> {
    let mut points: Vec<_> = sketch.points.iter().collect();
    points.sort_by(|a, b| a.id.cmp(&b.id));
    let at: BTreeMap<&ElementId, [f64; 2]> = points
        .iter()
        .map(|p| (&p.id, [p.x.mm(), p.y.mm()]))
        .collect();
    let place = |id: &ElementId| {
        at.get(id)
            .copied()
            .ok_or_else(|| format!("a curve names point {id}, which the sketch doesn't have"))
    };
    let mut geometry = Vec::new();
    let mut elements = Vec::new();
    // Every place FreeCAD has each point: a geometry and a position.
    let mut uses: BTreeMap<&ElementId, Vec<(i32, i32)>> = BTreeMap::new();
    let mut sorted_lines: Vec<_> = sketch.lines.iter().collect();
    sorted_lines.sort_by(|a, b| a.id.cmp(&b.id));
    let mut curves: BTreeMap<&ElementId, i32> = BTreeMap::new();
    for l in sorted_lines {
        let g = geometry.len() as i32;
        geometry.push(FreecadGeometry::Line {
            start: place(&l.from)?,
            end: place(&l.to)?,
        });
        uses.entry(&l.from).or_default().push((g, 1));
        uses.entry(&l.to).or_default().push((g, 2));
        curves.insert(&l.id, g);
        elements.push(l.id.as_str().to_owned());
    }
    let mut sorted_circles: Vec<_> = sketch.circles.iter().collect();
    sorted_circles.sort_by(|a, b| a.id.cmp(&b.id));
    let mut circles = BTreeMap::new();
    for c in sorted_circles {
        let g = geometry.len() as i32;
        geometry.push(FreecadGeometry::Circle {
            center: place(&c.center)?,
            radius: c.radius.mm(),
        });
        uses.entry(&c.center).or_default().push((g, 3));
        curves.insert(&c.id, g);
        circles.insert(c.id.as_str().to_owned(), g);
        elements.push(c.id.as_str().to_owned());
    }
    let mut sorted_arcs: Vec<_> = sketch.arcs.iter().collect();
    sorted_arcs.sort_by(|a, b| a.id.cmp(&b.id));
    for a in sorted_arcs {
        let g = geometry.len() as i32;
        let [radius, from, to] =
            arc_from_points(place(&a.center)?, place(&a.start)?, place(&a.end)?)
                .ok_or_else(|| format!("arc {} sweeps nothing", a.id))?;
        geometry.push(FreecadGeometry::Arc {
            center: place(&a.center)?,
            radius,
            from,
            to,
        });
        uses.entry(&a.start).or_default().push((g, 1));
        uses.entry(&a.end).or_default().push((g, 2));
        uses.entry(&a.center).or_default().push((g, 3));
        curves.insert(&a.id, g);
        elements.push(a.id.as_str().to_owned());
    }
    // A point no curve uses is FreeCAD geometry of its own.
    for p in &points {
        if !uses.contains_key(&p.id) {
            let g = geometry.len() as i32;
            geometry.push(FreecadGeometry::Point {
                at: [p.x.mm(), p.y.mm()],
            });
            uses.insert(&p.id, vec![(g, 1)]);
            elements.push(p.id.as_str().to_owned());
        }
    }
    let point = |r: &Reference| -> Result<(i32, i32), String> {
        match r {
            Reference::Origin => Ok((-1, 1)),
            Reference::Element(id) => uses
                .get(id)
                .and_then(|u| u.first().copied())
                .ok_or_else(|| format!("names {id} as a point")),
            _ => Err(format!("names {r} as a point")),
        }
    };
    let curve = |r: &Reference| -> Result<i32, String> {
        match r {
            Reference::XAxis => Ok(-1),
            Reference::YAxis => Ok(-2),
            Reference::Element(id) => curves
                .get(id)
                .copied()
                .ok_or_else(|| format!("names {id} as a curve")),
            Reference::Origin => Err("names the origin as a curve".to_owned()),
        }
    };
    // Where on curve `a` the point `at` is: 1 its start, 2 its end.
    let end_on = |a: &Reference, at: &Reference| -> Result<i32, String> {
        let (ga, (gp, pp)) = (curve(a)?, point(at)?);
        let Reference::Element(id) = at else {
            return Err(format!("names {at} as an end"));
        };
        let on_a = uses
            .get(id)
            .and_then(|u| u.iter().find(|(g, _)| *g == ga).copied());
        match on_a {
            Some((_, pos @ (1 | 2))) => Ok(pos),
            _ => Err(format!("names {at} ({gp}, {pp}) as an end of {a}")),
        }
    };
    let mut constraints = Vec::new();
    let mut sorted: Vec<_> = sketch.constraints.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    for (i, c) in sorted.iter().enumerate() {
        let make = |kind: &'static str, ints: Vec<i32>, value: Option<f64>, driving: bool| {
            FreecadConstraint {
                kind,
                ints,
                value,
                driving,
                id: Some(c.id.as_str().to_owned()),
                locked: None,
            }
        };
        let pair = |a: &Reference, b: &Reference| -> Result<Vec<i32>, String> {
            let ((ga, pa), (gb, pb)) = (point(a)?, point(b)?);
            Ok(vec![ga, pa, gb, pb])
        };
        let made = match &c.form {
            Form::Horizontal { line } => make("Horizontal", vec![curve(line)?], None, true),
            Form::Vertical { line } => make("Vertical", vec![curve(line)?], None, true),
            Form::HorizontalPoints { a, b } => make("Horizontal", pair(a, b)?, None, true),
            Form::VerticalPoints { a, b } => make("Vertical", pair(a, b)?, None, true),
            Form::Length {
                line,
                value,
                driving,
            } => make("Distance", vec![curve(line)?], Some(value.mm()), *driving),
            Form::Distance {
                a,
                b,
                value,
                driving,
            } => make("Distance", pair(a, b)?, Some(value.mm()), *driving),
            Form::DistanceX {
                a,
                b,
                value,
                driving,
            } => make("DistanceX", pair(a, b)?, Some(value.mm()), *driving),
            Form::DistanceY {
                a,
                b,
                value,
                driving,
            } => make("DistanceY", pair(a, b)?, Some(value.mm()), *driving),
            Form::PointLineDistance {
                point: p,
                line,
                value,
                driving,
                ..
            } => {
                let (g, pos) = point(p)?;
                make(
                    "Distance",
                    vec![g, pos, curve(line)?],
                    Some(value.mm()),
                    *driving,
                )
            }
            Form::On { point: p, curve: c } => {
                let (g, pos) = point(p)?;
                make("PointOnObject", vec![g, pos, curve(c)?], None, true)
            }
            Form::Parallel { a, b } => make("Parallel", vec![curve(a)?, curve(b)?], None, true),
            Form::Perpendicular { a, b } => {
                make("Perpendicular", vec![curve(a)?, curve(b)?], None, true)
            }
            // Unreversed, FreeCAD's angle without positions, as before `reversed` existed, so
            // the oracle's cases ask what they asked (split_line_with_angle's 1.1.3 crash
            // among them). Reversed, its angle with explicit start points (Sketch.cpp's
            // addAngleConstraint with PointPos): `a` from its start, `b` from its end. The
            // equation is the same either way for an unreversed angle.
            Form::Angle {
                a,
                b,
                reversed,
                value,
                driving,
            } => make(
                "Angle",
                if *reversed {
                    vec![curve(a)?, 1, curve(b)?, 2]
                } else {
                    vec![curve(a)?, curve(b)?]
                },
                Some(value.degrees().to_radians()),
                *driving,
            ),
            Form::Radius {
                curve: c,
                value,
                driving,
            } => make("Radius", vec![curve(c)?], Some(value.mm()), *driving),
            Form::Diameter {
                curve: c,
                value,
                driving,
            } => make("Diameter", vec![curve(c)?], Some(value.mm()), *driving),
            Form::Equal { a, b } => make("Equal", vec![curve(a)?, curve(b)?], None, true),
            Form::Tangent { a, b, .. } => make("Tangent", vec![curve(a)?, curve(b)?], None, true),
            Form::TangentOn { a, at, b, .. } => {
                let mut c = make(
                    "Tangent",
                    vec![curve(a)?, end_on(a, at)?, curve(b)?],
                    None,
                    true,
                );
                c.locked = Some(locked(translation, i)?);
                c
            }
            Form::TangentAt { a, b, at, .. } => {
                let mut c = make(
                    "Tangent",
                    vec![curve(a)?, end_on(a, at)?, curve(b)?, end_on(b, at)?],
                    None,
                    true,
                );
                c.locked = Some(locked(translation, i)?);
                c
            }
            Form::Symmetric { a, b, about } => {
                let mut ints = pair(a, b)?;
                match about {
                    Reference::Origin => ints.extend([-1, 1]),
                    Reference::Element(id) if at.contains_key(id) => {
                        let (g, pos) = point(about)?;
                        ints.extend([g, pos]);
                    }
                    _ => ints.push(curve(about)?),
                }
                make("Symmetric", ints, None, true)
            }
            Form::Midpoint { point: p, line } => {
                let (l, (g, pos)) = (curve(line)?, point(p)?);
                make("Symmetric", vec![l, 1, l, 2, g, pos], None, true)
            }
        };
        constraints.push(made);
    }
    // FreeCAD's coincidences: enough to join each point's places into one, less what its
    // endpoint-to-endpoint tangencies already join (each makes its own coincidence).
    let joined: Vec<[(i32, i32); 2]> = constraints
        .iter()
        .filter(|c| c.kind == "Tangent" && c.ints.len() == 4)
        .map(|c| [(c.ints[0], c.ints[1]), (c.ints[2], c.ints[3])])
        .collect();
    for places in uses.values() {
        // Which of the point's places are joined to which, as a forest over them.
        let mut root: Vec<usize> = (0..places.len()).collect();
        fn find(root: &mut [usize], mut i: usize) -> usize {
            while root[i] != i {
                i = root[i];
            }
            i
        }
        let index = |q: (i32, i32)| places.iter().position(|&p| p == q);
        for [a, b] in &joined {
            if let (Some(a), Some(b)) = (index(*a), index(*b)) {
                let (ra, rb) = (find(&mut root, a), find(&mut root, b));
                root[ra] = rb;
            }
        }
        for i in 1..places.len() {
            let (r0, ri) = (find(&mut root, 0), find(&mut root, i));
            if r0 != ri {
                root[ri] = r0;
                let ((g0, p0), (g, p)) = (places[0], places[i]);
                constraints.push(FreecadConstraint {
                    kind: "Coincident",
                    ints: vec![g0, p0, g, p],
                    value: None,
                    driving: true,
                    id: None,
                    locked: None,
                });
            }
        }
    }
    Ok(FreecadSketch {
        geometry,
        constraints,
        points: uses
            .iter()
            .filter_map(|(id, u)| u.first().map(|&p| (id.as_str().to_owned(), p)))
            .collect(),
        circles,
        elements,
        places: uses
            .iter()
            .map(|(id, u)| (id.as_str().to_owned(), u.clone()))
            .collect(),
    })
}

/// The value FreeCAD locks the tangency at Silico's `i`th constraint to: Silico's angle
/// between the curves (0 or π) less π/2, in (−π, π].
fn locked(translation: &Translation, i: usize) -> Result<f64, String> {
    let angle = translation
        .sent_angle(i)
        .ok_or_else(|| format!("constraint {i} sends no angle"))?;
    Ok(angle - FRAC_PI_2)
}

/// What FreeCAD's `sketch.solve()` found.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreecadSolved {
    /// `solve()`'s result: 0 solved, −1 the solver failed, −2 redundant constraints, −3
    /// conflicting, −4 over-constrained, −5 malformed.
    pub code: i32,
    pub dof: i32,
    /// Constraint indices from 1, in FreeCAD's lists.
    pub conflicting: Vec<usize>,
    pub redundant: Vec<usize>,
    pub partially_redundant: Vec<usize>,
    pub malformed: Vec<usize>,
    /// Each point's place after the solve, by ID.
    pub points: BTreeMap<String, [f64; 2]>,
    pub radii: BTreeMap<String, f64>,
    /// Each constraint's value after the solve, by index from 1.
    pub values: Vec<f64>,
}

pub(crate) const SCRIPT: &str = r#"
import json, math, os
import FreeCAD as App, Part, Sketcher

V = App.Vector
cases = json.load(open(os.environ["SILICO_SKETCH_CASES"], encoding="utf-8"))
out = []
for case in cases:
    doc = App.newDocument()
    s = doc.addObject("Sketcher::SketchObject", "S")
    for g in case["geometry"]:
        k = g["kind"]
        if k == "point":
            s.addGeometry(Part.Point(V(g["at"][0], g["at"][1], 0)))
        elif k == "line":
            s.addGeometry(Part.LineSegment(V(g["start"][0], g["start"][1], 0), V(g["end"][0], g["end"][1], 0)))
        elif k == "circle":
            s.addGeometry(Part.Circle(V(g["center"][0], g["center"][1], 0), V(0, 0, 1), g["radius"]))
        else:
            c = Part.Circle(V(g["center"][0], g["center"][1], 0), V(0, 0, 1), g["radius"])
            s.addGeometry(Part.ArcOfCircle(c, g["from"], g["to"]))
    made = []
    for c in case["constraints"]:
        args = [c["kind"]] + c["ints"] + ([c["value"]] if c["value"] is not None else [])
        made.append(Sketcher.Constraint(*args))
    s.addConstraint(made)
    for i, c in enumerate(case["constraints"]):
        if not c["driving"]:
            s.setDriving(i, False)
    code = s.solve()
    points = {}
    for pid, (g, pos) in case["points"].items():
        p = s.getPoint(g, pos)
        points[pid] = [p.x, p.y]
    radii = {cid: s.Geometry[g].Radius for cid, g in case["circles"].items()}
    out.append({
        "code": code,
        "dof": s.DoF,
        "conflicting": list(s.ConflictingConstraints),
        "redundant": list(s.RedundantConstraints),
        "partially_redundant": list(s.PartiallyRedundantConstraints),
        "malformed": list(s.MalformedConstraints),
        "points": points,
        "radii": radii,
        "values": [c.Value for c in s.Constraints],
    })
    App.closeDocument(doc.Name)
print("SILICO_ORACLE " + json.dumps(out))
"#;

/// What FreeCAD found for each of `sketches`, in order.
pub fn freecad_solve(sketches: &[FreecadSketch]) -> Result<Vec<FreecadSolved>, String> {
    // A directory of this run's own, removed however the run ends.
    let scratch = freecad::Scratch::new()?;
    let json =
        serde_json::to_string(sketches).map_err(|e| format!("can't write the cases: {e}"))?;
    let input = scratch.write("cases.json", &json)?;
    let source = format!(
        "import os\nos.environ['SILICO_SKETCH_CASES'] = {:?}\n{SCRIPT}",
        input.to_string_lossy()
    );
    let value = freecad::script(&source);
    drop(scratch);
    let solved: Vec<FreecadSolved> = serde_json::from_value(value?)
        .map_err(|e| format!("FreeCAD's sketch outcomes can't be read: {e}"))?;
    if solved.len() != sketches.len() {
        return Err(format!(
            "FreeCAD gave {} sketch outcomes for {} cases",
            solved.len(),
            sketches.len()
        ));
    }
    Ok(solved)
}

/// What Silico found: the worker's diagnosis by constraint ID, and the solution or error.
#[derive(Debug, Clone)]
pub struct Ours {
    pub dofs: i32,
    pub conflicting: Vec<String>,
    pub redundant: Vec<String>,
    pub partially_redundant: Vec<String>,
    pub outcome: Result<SketchSolution, Error>,
}

/// Solves `case` through `worker`.
pub fn silico_solve(case: &SketchCase, worker: &mut dyn SketchSolver) -> Result<Ours, String> {
    let translation = Translation::new(&case.sketch).map_err(|e| e.to_string())?;
    let answer = translation.solve(worker).map_err(|e| e.to_string())?;
    let solved = answer.solved();
    let ids = |tags: &[Tag]| -> Vec<String> {
        tags.iter()
            .filter_map(|&t| {
                translation
                    .constraint_id(t)
                    .map(|id| id.as_str().to_owned())
            })
            .collect()
    };
    let (dofs, conflicting, redundant, partially_redundant) = (
        solved.dofs,
        ids(&solved.conflicting),
        ids(&solved.redundant),
        ids(&solved.partially_redundant),
    );
    Ok(Ours {
        dofs,
        conflicting,
        redundant,
        partially_redundant,
        outcome: answer.into_outcome(),
    })
}

/// How `ours` and `theirs` differ for `case`, whose FreeCAD model is `model`.
pub fn compare(model: &FreecadSketch, ours: &Ours, theirs: &FreecadSolved) -> Vec<String> {
    compare_outcomes(model, ours, theirs, true)
}

/// As [`compare`], comparing where each side put the points, the radii, and the reference
/// values only if `geometry`: the sketch edits' oracle leaves them out where the port note
/// says the two start their solves from different places.
pub fn compare_outcomes(
    model: &FreecadSketch,
    ours: &Ours,
    theirs: &FreecadSolved,
    geometry: bool,
) -> Vec<String> {
    let mut how = Vec::new();
    // FreeCAD's indices, from 1, to Silico's IDs; its coincidences are none of Silico's.
    let ids = |list: &[usize]| -> Vec<String> {
        list.iter()
            .filter_map(|&i| i.checked_sub(1).and_then(|i| model.constraints.get(i)))
            .filter_map(|c| c.id.clone())
            .collect()
    };
    if ours.dofs != theirs.dof {
        how.push(format!(
            "degrees of freedom {} against FreeCAD's {}",
            ours.dofs, theirs.dof
        ));
    }
    for (name, ours, theirs) in [
        ("conflicting", &ours.conflicting, ids(&theirs.conflicting)),
        ("redundant", &ours.redundant, ids(&theirs.redundant)),
        (
            "partially redundant",
            &ours.partially_redundant,
            ids(&theirs.partially_redundant),
        ),
    ] {
        if *ours != theirs {
            how.push(format!("{name} {ours:?} against FreeCAD's {theirs:?}"));
        }
    }
    if !theirs.malformed.is_empty() {
        how.push(format!(
            "FreeCAD found constraints malformed: {:?}",
            ids(&theirs.malformed)
        ));
    }
    // How each ended. FreeCAD refuses a sketch with redundant constraints: as redundant (−2)
    // when they hold where it's drawn, and as the solver failing (−1) when they don't
    // (PlaneGCS checks them on the starting values). Silico solves it if they agree on the
    // solution (decision 5) and refuses it as over-constrained if they don't: either is
    // Silico's, and only the lists above are compared then.
    // FreeCAD's conflicting (−3) and over-constrained (−4) agree only with Silico's
    // over-constrained refusal (docs/ports/sketcher.md, Solving).
    let code = |e: &Error| e.code();
    let redundant =
        !ids(&theirs.redundant).is_empty() || !ids(&theirs.partially_redundant).is_empty();
    match (theirs.code, &ours.outcome) {
        (0, Ok(solution)) if geometry => how.extend(compare_solutions(model, solution, theirs)),
        (0, Ok(_)) => {}
        (-2, Ok(_)) => {}
        (-2, Err(e)) if code(e) == ErrorCode::OverConstrained => {}
        (-1, Ok(_)) if redundant => {}
        (-1, Err(e)) if redundant && code(e) == ErrorCode::OverConstrained => {}
        (-3 | -4, Err(e)) if code(e) == ErrorCode::OverConstrained => {}
        (-1, Err(e)) if matches!(code(e), ErrorCode::SolveFailed | ErrorCode::InvalidResult) => {}
        (code, outcome) => how.push(format!(
            "FreeCAD's solve gave {code} (degrees of freedom {}, values {:?}), Silico's {}",
            theirs.dof,
            theirs.values,
            match outcome {
                Ok(_) => "a solution".to_owned(),
                Err(e) => e.to_string(),
            }
        )),
    }
    how
}

fn compare_solutions(
    model: &FreecadSketch,
    ours: &SketchSolution,
    theirs: &FreecadSolved,
) -> Vec<String> {
    let mut how = Vec::new();
    let near = |a: f64, b: f64| (a - b).abs() <= TOLERANCE;
    for (id, [x, y]) in &ours.points {
        let p = [x.mm(), y.mm()];
        match theirs.points.get(id.as_str()) {
            Some(q) if near(p[0], q[0]) && near(p[1], q[1]) => {}
            other => how.push(format!("point {id} at {p:?}, FreeCAD's at {other:?}")),
        }
    }
    for (id, r) in &ours.radii {
        let r = r.mm();
        match theirs.radii.get(id.as_str()) {
            Some(q) if near(r, *q) => {}
            other => how.push(format!("circle {id}'s radius {r}, FreeCAD's {other:?}")),
        }
    }
    for (i, c) in model.constraints.iter().enumerate() {
        let Some(id) = &c.id else { continue };
        let Some(&value) = theirs.values.get(i) else {
            how.push(format!("FreeCAD has no value for {id}"));
            continue;
        };
        // The direction FreeCAD locked a tangency at an end to, against Silico's.
        if let Some(want) = c.locked
            && libm::remainder(value - want, TAU).abs() > 1e-12
        {
            how.push(format!("tangency {id} locked at {value}, Silico's {want}"));
        }
        let Some(element) = ElementId::new(id) else {
            continue;
        };
        if let Some(measured) = ours.references.get(&element) {
            let (ours, theirs) = match *measured {
                Measured::Length(v) => (v, value),
                Measured::FromLine { distance, .. } => (distance, value),
                Measured::Angle(degrees) => (degrees, value.to_degrees()),
            };
            if (ours - theirs).abs() > 1e-7 {
                how.push(format!(
                    "reference {id} measures {ours}, FreeCAD's {theirs}"
                ));
            }
        }
    }
    how
}

/// A sketch case whose two sides differ, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchDifference {
    pub case: String,
    pub how: Vec<String>,
}

impl std::fmt::Display for SketchDifference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.case, self.how.join("; "))
    }
}

/// Runs `cases` through Silico's sketch worker (`silico-sketch-worker` beside the running
/// executable) and FreeCAD, and gives the cases that differ.
pub fn run(cases: &[SketchCase]) -> Result<Vec<SketchDifference>, String> {
    // Silico's worker first: a missing one is found before FreeCAD's run, not after.
    let mut worker = silico_core::sketch::SketchWorker::start().map_err(|e| e.to_string())?;
    let mut models = Vec::with_capacity(cases.len());
    for case in cases {
        let translation =
            Translation::new(&case.sketch).map_err(|e| format!("{}: {e}", case.name))?;
        models.push(
            freecad_sketch(&case.sketch, &translation)
                .map_err(|e| format!("{}: {e}", case.name))?,
        );
    }
    let theirs = freecad_solve(&models)?;
    let mut differences = Vec::new();
    for ((case, model), theirs) in cases.iter().zip(&models).zip(&theirs) {
        let ours = silico_solve(case, &mut worker).map_err(|e| format!("{}: {e}", case.name))?;
        let how = compare(model, &ours, theirs);
        if !how.is_empty() {
            differences.push(SketchDifference {
                case: case.name.clone(),
                how,
            });
        }
    }
    Ok(differences)
}
