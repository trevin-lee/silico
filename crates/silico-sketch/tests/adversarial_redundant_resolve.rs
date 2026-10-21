//! /break-it on `Translation::solve`'s solve again from the solution: a sketch the solver
//! reached a solution for, and either called over-constrained or solved with constraints
//! dropped, is solved again from each solution until a solve leaves it where it was, at
//! most MAX_LOAD_RESOLVES times. The claim under attack: the answer that stands "is what
//! every later load gives" (M3.4: a sketch is solved on every load from the coordinates its
//! file stores). Each case here is solved, written through the file as a save writes it,
//! read back, and solved again as a load does; the two answers must agree: Ok on both or a
//! refusal on both, the same bits, and the same redundant constraints, degrees of freedom,
//! and free geometry. Shapes beyond the rectangle the fix was found on: a slot (arcs, whose
//! angles a load works out from the points), a triangle held by angles, and a rectangle
//! with every constraint given twice.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/saves.rs"]
mod saves;

use proptest::prelude::*;
use saves::{FEATURE, arc, checked, line, nm, point, said, solved, worker, written_back};
use silico_core::ErrorCode;
use silico_core::document::Checked;
use silico_core::sketch::{SketchSolution, SketchSolver, SketchWorker};
use silico_protocol::{SketchSystem, Solved};

const CLOSE: f64 = 1e-8;

/// How far a kept slot's corner may sit from the analytic one: decision 5's ε = max(1e-8 mm,
/// 1e-9 × L), L the slot's longer of its side and radius (docs/design/m3.3-solving.md),
/// worked out here from the slot's own sizes. Rectangles and triangles are held to CLOSE,
/// which every case of their properties meets. A slot's corners are fixed through angular
/// rows (the tangencies at its ends, its sides' parallel), carried to them by the arcs'
/// radii, and land further off: the long slot's p2, 2e-8 mm at L 4,289 mm. A bound by hand,
/// chaining PlaneGCS's 1e-10 per row from p1 (held) to the farthest corner, is no tighter
/// than 1e-9 × L (a horizontal's 1e-10 mm across a short side, carried by a large arc,
/// gives 1e-10 × r / len, looser still), so a slot's corners get the slack the core itself
/// keeps a redundant row within.
fn within_decision_5(lever: f64) -> f64 {
    CLOSE.max(1e-9 * lever)
}

fn text(points: &[String], lines: &[String], arcs: &[String], constraints: &[String]) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"{FEATURE}\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", \"plane\": \"xy\", \"points\": [{}], \"lines\": [{}], \"circles\": [], \"arcs\": [{}], \"constraints\": [{}]}}]}}",
        points.join(", "),
        lines.join(", "),
        arcs.join(", "),
        constraints.join(", ")
    )
}

/// The redundant constraints a solve names, by ID.
fn dropped(s: &SketchSolution) -> Vec<String> {
    s.redundant
        .keys()
        .map(|id| id.as_str().to_owned())
        .collect()
}

/// How a case went, if it didn't hold.
#[derive(Debug)]
enum Broke {
    /// Solved, but not to where the constraints put it (the analytic answer).
    Wrong(String),
    /// Refused, though its redundancies agree: decision 5's own refusal, or another.
    Refused(ErrorCode, String),
    /// Solved, then solved from its own saved solution, a load: a different answer.
    LoadDiffers(String),
}

impl std::fmt::Display for Broke {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Broke::Wrong(m) => write!(f, "solved wrong: {m}"),
            Broke::Refused(_, m) => write!(f, "refused: {m}"),
            Broke::LoadDiffers(m) => write!(f, "the load differs: {m}"),
        }
    }
}

/// Solves `doc`, checks the solution with `check` (the analytic answer), then solves it as
/// a load does, from the solution written through the file, and compares the two answers.
/// A refusal as conflicting by PlaneGCS's diagnosis on the drawing is a spec question
/// (FreeCAD refuses the same), not a break.
fn case(
    doc: &Checked,
    what: &str,
    w: &mut dyn SketchSolver,
    check: impl Fn(&SketchSolution) -> bool,
) -> Result<(), Broke> {
    let s = match solved(doc, w) {
        Ok(s) => s,
        Err(e)
            if e.code() == ErrorCode::OverConstrained
                && e.message().starts_with("the sketch's constraints conflict") =>
        {
            return Ok(());
        }
        Err(e) => {
            return Err(Broke::Refused(
                e.code(),
                format!("{what}: {e} / {:?}", e.elements()),
            ));
        }
    };
    if !check(&s) {
        return Err(Broke::Wrong(format!("{what}: {s:?}")));
    }
    let again = written_back(doc, &s);
    match solved(&again, w) {
        Ok(l) if said(&l) == said(&s) && dropped(&l) == dropped(&s) => Ok(()),
        Ok(l) => Err(Broke::LoadDiffers(format!(
            "{what}: saved {} redundant {:?}\n  loaded {} redundant {:?}",
            said(&s),
            dropped(&s),
            said(&l),
            dropped(&l)
        ))),
        Err(e) => Err(Broke::LoadDiffers(format!(
            "{what}: saved Ok {}\n  loaded refused: {e} / {:?}",
            said(&s),
            e.elements()
        ))),
    }
}

// --- Shapes ---------------------------------------------------------------------------------

fn c(s: &str) -> String {
    s.to_owned()
}

/// A rectangle w × h, lower-left corner held at (x0, y0), drawn off by `jitter` (a fraction
/// of its smaller side), with every redundancy that agrees, each given `times` times.
fn rectangle(x0: f64, y0: f64, w: f64, h: f64, jitter: [f64; 6], times: usize) -> String {
    let j = |k: usize| jitter[k] * w.min(h);
    let points = [
        point("p0000001", x0, y0),
        point("p0000002", x0 + w + j(0), y0 + j(1)),
        point("p0000003", x0 + w + j(2), y0 + h + j(3)),
        point("p0000004", x0 + j(4), y0 + h + j(5)),
    ];
    let lines = [
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000002", "p0000003"),
        line("n0000003", "p0000003", "p0000004"),
        line("n0000004", "p0000004", "p0000001"),
    ];
    let forms = [
        format!(
            r#""type": "distance_x", "a": "origin", "b": "p0000001", "value": {x0}, "driving": true"#
        ),
        format!(
            r#""type": "distance_y", "a": "origin", "b": "p0000001", "value": {y0}, "driving": true"#
        ),
        c(r#""type": "horizontal", "line": "n0000001""#),
        c(r#""type": "vertical", "line": "n0000002""#),
        c(r#""type": "horizontal", "line": "n0000003""#),
        c(r#""type": "vertical", "line": "n0000004""#),
        format!(r#""type": "length", "line": "n0000001", "value": {w}, "driving": true"#),
        format!(r#""type": "length", "line": "n0000002", "value": {h}, "driving": true"#),
        c(r#""type": "parallel", "a": "n0000001", "b": "n0000003""#),
        c(r#""type": "perpendicular", "a": "n0000001", "b": "n0000002""#),
        c(r#""type": "equal", "a": "n0000002", "b": "n0000004""#),
        c(r#""type": "equal", "a": "n0000001", "b": "n0000003""#),
        c(r#""type": "perpendicular", "a": "n0000003", "b": "n0000004""#),
    ];
    let mut constraints = Vec::new();
    for t in 0..times {
        for (i, f) in forms.iter().enumerate() {
            constraints.push(format!(r#"{{"id": "k{:07}", {f}}}"#, t * 100 + i + 1));
        }
    }
    text(&points, &lines, &[], &constraints)
}

fn rectangle_is(s: &SketchSolution, x0: f64, y0: f64, w: f64, h: f64) -> bool {
    let at = |p: &str| {
        s.point(&silico_core::document::ElementId::new(p).unwrap())
            .unwrap()
    };
    let near = |a: f64, b: f64| (a - b).abs() <= CLOSE;
    let (p1, p2, p3, p4) = (
        at("p0000001"),
        at("p0000002"),
        at("p0000003"),
        at("p0000004"),
    );
    near(p1[0], x0)
        && near(p1[1], y0)
        && near((p2[0] - x0).abs(), w)
        && near(p2[1], y0)
        && near(p3[0], p2[0])
        && near((p3[1] - y0).abs(), h)
        && near(p4[0], x0)
        && near(p4[1], p3[1])
}

/// A slot: centres c1 (x0, y0) and c2 (x0 + len, y0), radius r, its straight sides tangent
/// to both arcs where they meet, drawn off by `jitter` (a fraction of the smaller of len and r), with redundancies
/// that agree: both arcs' radii and their equality, both sides horizontal and parallel and
/// equal, the centres level.
fn slot(x0: f64, y0: f64, len: f64, r: f64, jitter: [f64; 6]) -> String {
    let j = |k: usize| jitter[k] * r.min(len);
    let points = [
        point("p0000001", x0, y0),
        point("p0000002", x0 + len + j(0), y0 + j(1)),
        point("p0000003", x0 + j(2), y0 + r + j(3)),
        point("p0000004", x0 + len + j(4), y0 + r + j(5)),
        point("p0000005", x0 + len - j(1), y0 - r + j(0)),
        point("p0000006", x0 - j(3), y0 - r - j(2)),
    ];
    let lines = [
        line("n0000001", "p0000003", "p0000004"),
        line("n0000002", "p0000005", "p0000006"),
    ];
    let arcs = [
        arc("a0000001", "p0000001", "p0000003", "p0000006"),
        arc("a0000002", "p0000002", "p0000005", "p0000004"),
    ];
    let constraints = [
        format!(
            r#"{{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": {x0}, "driving": true}}"#
        ),
        format!(
            r#"{{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": {y0}, "driving": true}}"#
        ),
        c(r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#),
        c(r#"{"id": "k0000004", "type": "horizontal", "line": "n0000002"}"#),
        format!(
            r#"{{"id": "k0000005", "type": "length", "line": "n0000001", "value": {len}, "driving": true}}"#
        ),
        format!(
            r#"{{"id": "k0000006", "type": "radius", "curve": "a0000001", "value": {r}, "driving": true}}"#
        ),
        format!(
            r#"{{"id": "k0000007", "type": "radius", "curve": "a0000002", "value": {r}, "driving": true}}"#
        ),
        c(r#"{"id": "k0000008", "type": "equal", "a": "a0000001", "b": "a0000002"}"#),
        c(r#"{"id": "k0000009", "type": "parallel", "a": "n0000001", "b": "n0000002"}"#),
        c(
            r#"{"id": "k0000010", "type": "tangent_at", "a": "n0000001", "b": "a0000002", "at": "p0000004", "direction": "same"}"#,
        ),
        c(
            r#"{"id": "k0000011", "type": "tangent_at", "a": "n0000002", "b": "a0000002", "at": "p0000005", "direction": "same"}"#,
        ),
        c(
            r#"{"id": "k0000012", "type": "tangent_at", "a": "n0000001", "b": "a0000001", "at": "p0000003", "direction": "same"}"#,
        ),
        c(
            r#"{"id": "k0000013", "type": "tangent_at", "a": "n0000002", "b": "a0000001", "at": "p0000006", "direction": "same"}"#,
        ),
        c(r#"{"id": "k0000014", "type": "horizontal_points", "a": "p0000001", "b": "p0000002"}"#),
        c(r#"{"id": "k0000015", "type": "equal", "a": "n0000001", "b": "n0000002"}"#),
    ];
    text(&points, &lines, &arcs, &constraints)
}

fn slot_is(s: &SketchSolution, x0: f64, y0: f64, len: f64, r: f64) -> bool {
    slot_within(s, x0, y0, len, r, within_decision_5(len.max(r)))
}

/// [`slot_is`], each point within `close` of where it should be.
fn slot_within(s: &SketchSolution, x0: f64, y0: f64, len: f64, r: f64, close: f64) -> bool {
    // Either way round: a length has no direction, so a slot drawn far off may land on its
    // mirror image (its arcs and sides still tangent the way they're drawn).
    let at = |p: &str| {
        s.point(&silico_core::document::ElementId::new(p).unwrap())
            .unwrap()
    };
    let near =
        |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() <= close && (a[1] - b[1]).abs() <= close;
    [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)]
        .iter()
        .any(|&(sx, sy): &(f64, f64)| {
            near(at("p0000001"), [x0, y0])
                && near(at("p0000002"), [x0 + sx * len, y0])
                && near(at("p0000003"), [x0, y0 + sy * r])
                && near(at("p0000004"), [x0 + sx * len, y0 + sy * r])
                && near(at("p0000005"), [x0 + sx * len, y0 - sy * r])
                && near(at("p0000006"), [x0, y0 - sy * r])
        })
}

/// An equilateral triangle of side `side`, its first corner held at (x0, y0) and its base
/// horizontal, its sides equal (one redundantly) and its angles given too (redundantly),
/// drawn off by `jitter` (a fraction of the side).
fn triangle(x0: f64, y0: f64, side: f64, jitter: [f64; 4]) -> String {
    let j = |k: usize| jitter[k] * side;
    let hgt = side * 3f64.sqrt() / 2.0;
    let points = [
        point("p0000001", x0, y0),
        point("p0000002", x0 + side + j(0), y0 + j(1)),
        point("p0000003", x0 + side / 2.0 + j(2), y0 + hgt + j(3)),
    ];
    let lines = [
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000002", "p0000003"),
        line("n0000003", "p0000003", "p0000001"),
    ];
    let constraints = [
        format!(
            r#"{{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": {x0}, "driving": true}}"#
        ),
        format!(
            r#"{{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": {y0}, "driving": true}}"#
        ),
        c(r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#),
        format!(
            r#"{{"id": "k0000004", "type": "length", "line": "n0000001", "value": {side}, "driving": true}}"#
        ),
        c(r#"{"id": "k0000005", "type": "equal", "a": "n0000001", "b": "n0000002"}"#),
        c(r#"{"id": "k0000006", "type": "equal", "a": "n0000002", "b": "n0000003"}"#),
        c(r#"{"id": "k0000007", "type": "equal", "a": "n0000003", "b": "n0000001"}"#),
        c(
            r#"{"id": "k0000008", "type": "angle", "a": "n0000001", "b": "n0000002", "value": 120, "driving": true}"#,
        ),
        c(
            r#"{"id": "k0000009", "type": "angle", "a": "n0000002", "b": "n0000003", "value": 120, "driving": true}"#,
        ),
    ];
    text(&points, &lines, &[], &constraints)
}

fn triangle_is(s: &SketchSolution, x0: f64, y0: f64, side: f64) -> bool {
    let at = |p: &str| {
        s.point(&silico_core::document::ElementId::new(p).unwrap())
            .unwrap()
    };
    let near =
        |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() <= CLOSE && (a[1] - b[1]).abs() <= CLOSE;
    let hgt = side * 3f64.sqrt() / 2.0;
    near(at("p0000001"), [x0, y0])
        && near(at("p0000002"), [x0 + side, y0])
        && near(at("p0000003"), [x0 + side / 2.0, y0 + hgt])
}

/// Two circles of radius r, c1 about p1 held at (x0, y0) and c2 about p2 held d to its
/// right, drawn with radii off by `jitter` (fractions of r): c1's radius and diameter both
/// given, c2's radius given, and the two equal: the radii given redundantly, so the circles'
/// radii are what a save writes and a load starts from.
fn equal_circles(x0: f64, y0: f64, r: f64, d: f64, jitter: [f64; 2]) -> String {
    let points = [point("p0000001", x0, y0), point("p0000002", x0 + d, y0)];
    let circles = [
        format!(
            r#"{{"id": "c0000001", "center": "p0000001", "radius": {}, "construction": false}}"#,
            nm(r * (1.0 + jitter[0]))
        ),
        format!(
            r#"{{"id": "c0000002", "center": "p0000002", "radius": {}, "construction": false}}"#,
            nm(r * (1.0 + jitter[1]))
        ),
    ];
    let constraints = [
        format!(
            r#"{{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": {x0}, "driving": true}}"#
        ),
        format!(
            r#"{{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": {y0}, "driving": true}}"#
        ),
        format!(
            r#"{{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": {}, "driving": true}}"#,
            nm(x0 + d)
        ),
        format!(
            r#"{{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": {y0}, "driving": true}}"#
        ),
        format!(
            r#"{{"id": "k0000005", "type": "radius", "curve": "c0000001", "value": {r}, "driving": true}}"#
        ),
        format!(
            r#"{{"id": "k0000006", "type": "diameter", "curve": "c0000001", "value": {}, "driving": true}}"#,
            nm(2.0 * r)
        ),
        format!(
            r#"{{"id": "k0000007", "type": "radius", "curve": "c0000002", "value": {r}, "driving": true}}"#
        ),
        c(r#"{"id": "k0000008", "type": "equal", "a": "c0000001", "b": "c0000002"}"#),
    ];
    text(&points, &[], &[], &constraints).replace(
        "\"circles\": []",
        &format!("\"circles\": [{}]", circles.join(", ")),
    )
}

fn equal_circles_are(s: &SketchSolution, r: f64) -> bool {
    ["c0000001", "c0000002"].iter().all(|c| {
        s.radius(&silico_core::document::ElementId::new(c).unwrap())
            .is_some_and(|got| (got - r).abs() <= CLOSE)
    })
}

// --- Targeted --------------------------------------------------------------------------------

#[test]
fn each_shape_drawn_true_solves_to_itself_and_loads_unchanged() {
    // The shapes' constraint forms, checked on their exact drawings first, so the properties
    // below fail only on what they're after.
    let mut w = worker();
    let r = rectangle(0.0, 0.0, 10.0, 5.0, [0.0; 6], 1);
    case(&checked(&r).unwrap(), "rectangle", &mut w, |s| {
        rectangle_is(s, 0.0, 0.0, 10.0, 5.0)
    })
    .unwrap();
    let r2 = rectangle(0.0, 0.0, 10.0, 5.0, [0.0; 6], 2);
    case(&checked(&r2).unwrap(), "rectangle twice", &mut w, |s| {
        rectangle_is(s, 0.0, 0.0, 10.0, 5.0)
    })
    .unwrap();
    let sl = slot(0.0, 0.0, 20.0, 3.0, [0.0; 6]);
    case(&checked(&sl).unwrap(), "slot", &mut w, |s| {
        slot_is(s, 0.0, 0.0, 20.0, 3.0)
    })
    .unwrap();
    let tr = triangle(0.0, 0.0, 10.0, [0.0; 4]);
    case(&checked(&tr).unwrap(), "triangle", &mut w, |s| {
        triangle_is(s, 0.0, 0.0, 10.0)
    })
    .unwrap();
}

/// The redundant constraints `doc`'s solve names, and those its load names: solved, written
/// through the file, read back, and solved again.
fn named_on_save_and_load(doc: &Checked) -> (Vec<String>, Vec<String>) {
    let mut w = worker();
    let s = solved(doc, &mut w).unwrap_or_else(|e| panic!("{e} {:?}", e.elements()));
    let l = solved(&written_back(doc, &s), &mut w).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(said(&l), said(&s), "the load moved it");
    (dropped(&s), dropped(&l))
}

#[test]
fn a_unit_square_names_the_same_redundant_constraints_when_loaded_as_when_saved() {
    // Found by break-it on the first solve again (from the solver's own values, until it
    // didn't move, keeping the first answer), fixed: a 1 × 1 square, every redundancy
    // agreeing, drawn with p4's y 0.14 low. The save solved it to the exact square and
    // named k0000012 (the top and bottom equal) redundant: PlaneGCS's pick on the drawing.
    // Solved again from that square it didn't move, so Translation::solve kept the first
    // answer "as it was diagnosed"; but the load diagnoses on the square and names k0000005
    // and k0000010, and every load after says the same. Now the answer that stands is the
    // load's (docs/design/m3.3-solving.md): the save names what the load names.
    let doc = checked(&rectangle(
        0.0,
        0.0,
        1.0,
        1.0,
        [0.0, 0.0, 0.0, 0.0, 0.0, -0.13981727856408283],
        1,
    ))
    .unwrap();
    let (saved, loaded) = named_on_save_and_load(&doc);
    assert_eq!(saved, loaded);
}

#[test]
fn a_slot_names_the_same_redundant_constraints_when_loaded_as_when_saved() {
    // Found by break-it on the first solve again, fixed as above: on a 1 mm slot of radius
    // 1 the save named k0000015 (the sides equal), the load k0000009 (the sides parallel).
    let doc = checked(&slot(
        0.0,
        0.0,
        1.0,
        1.0,
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.1754657099906572],
    ))
    .unwrap();
    let (saved, loaded) = named_on_save_and_load(&doc);
    assert_eq!(saved, loaded);
}

#[test]
fn a_triangle_names_the_same_redundant_constraints_when_loaded_as_when_saved() {
    // Found by break-it on the first solve again, fixed as above: on a unit equilateral
    // triangle held by its angles too the save named k0000008 (the first angle), the load
    // k0000009 (the second).
    let doc = checked(&triangle(
        0.0,
        0.0,
        1.0,
        [0.0, 0.0, 0.0, 0.19372426983688423],
    ))
    .unwrap();
    let (saved, loaded) = named_on_save_and_load(&doc);
    assert_eq!(saved, loaded);
}

/// A solver that counts the solves it's asked for, and keeps the replies.
struct Counting {
    inner: SketchWorker,
    sent: Vec<SketchSystem>,
    replies: Vec<Solved>,
}

impl SketchSolver for Counting {
    fn solve(&mut self, system: SketchSystem) -> Result<Solved, silico_core::Error> {
        self.sent.push(system.clone());
        let r = self.inner.solve(system)?;
        self.replies.push(r.clone());
        Ok(r)
    }
    fn distrust(&mut self) {
        self.inner.distrust();
    }
}

#[test]
fn an_equilateral_triangle_far_out_is_refused_on_save_as_its_load_refuses_it() {
    // Found by break-it on the first solve again, fixed: a 404.17214 mm equilateral
    // triangle at (999,000, -999,000), its sides equal (one redundantly) and two angles
    // given too, drawn with its apex off by (-51, 31) mm. The save's first solve, diagnosed
    // on the drawing, drops k0000007 and solves it to the exact triangle, Ok. A load from
    // that triangle, diagnosed there, doesn't move it but calls seven constraints
    // conflicting (PlaneGCS's absolute convergence, 1e-10 mm, is under the spacing of
    // doubles a kilometre out, 1.16e-10 mm); that solve again kept the first answer, so the
    // save wrote the triangle and every load was refused: a document that saved cleanly
    // failed `silico check` on the next open. Now the answer that stands is the load's, and
    // the save is refused as every load would be: as the same sketch drawn at the exact
    // triangle is. FreeCAD, which solves from the stored geometry on every recompute,
    // refuses it the same way; that a consistent triangle is refused there is the solver's
    // precision at a kilometre, as for conflicting diagnoses on the drawing
    // (adversarial_document.rs).
    let mut w = worker();
    let refusal = |doc: &Checked, w: &mut SketchWorker| {
        let e = solved(doc, w).map(|s| said(&s)).unwrap_err();
        (e.code(), e.message().to_owned(), e.elements().to_vec())
    };
    let drawn = checked(&triangle(
        999_000.0,
        -999_000.0,
        404.17214,
        [0.0, 0.0, -0.12651966648114912, 0.07789295283955709],
    ))
    .unwrap();
    let exact = checked(&triangle(999_000.0, -999_000.0, 404.17214, [0.0; 4])).unwrap();
    let saved = refusal(&drawn, &mut w);
    assert_eq!(saved.0, ErrorCode::OverConstrained);
    assert!(
        saved.1.starts_with("the sketch's constraints conflict"),
        "{}",
        saved.1
    );
    assert_eq!(saved, refusal(&exact, &mut w));
}

/// The gap the solution reports for redundant constraint `id`, in millimetres.
fn gap_of(s: &SketchSolution, id: &str) -> f64 {
    s.redundant[&silico_core::document::ElementId::new(id).unwrap()]
}

#[test]
fn a_micrometre_equilateral_triangle_keeps_its_agreeing_angle() {
    // Found by break-it on the first solve again (not introduced by it), fixed by decision
    // 5's positional rule (Fable, 2026-10-18): a 1.597 µm equilateral triangle at the
    // origin, drawn a little off, was over_constrained, "disagree with them, by up to 0e0
    // mm and 8.09e-9 rad", naming k0000009 (the second angle), against a scale-free 1e-9
    // rad. DogLeg meets the kept equal lengths to about 1.5e-11 mm, a relative 1e-8 on a
    // 1.6 µm side. As a distance, the angle's gap is that angle carried by its sides:
    // 8.09e-9 rad × 0.001597 mm = 1.3e-11 mm (hand), well within the sketch's 1e-8 mm.
    // Kept, k0000009 redundant.
    let doc = checked(&triangle(
        0.0,
        0.0,
        0.001597,
        [
            0.05809065911905431,
            -0.05506065990341277,
            -0.020687603556015788,
            0.0793544942689125,
        ],
    ))
    .unwrap();
    let s = solved(&doc, &mut worker()).unwrap_or_else(|e| panic!("{e}"));
    assert!(triangle_is(&s, 0.0, 0.0, 0.001597), "{s:?}");
    assert_eq!(dropped(&s), ["k0000009"]);
    let gap = gap_of(&s, "k0000009");
    assert!((1e-12..=1e-10).contains(&gap), "{gap:e}");
}

#[test]
fn a_long_slot_keeps_its_equal_sides_within_the_slack_its_arcs_allow() {
    // Found by break-it on the first check by load (only a solve that dropped constraints
    // was checked), fixed by decision 5's positional rule: a 4,289.278884 mm slot of radius
    // 430.346545 at the origin was refused, its sides' equality "disagreeing by 2.4e-8 mm"
    // against a fixed 1e-8. The slack reaches it through the tangencies' angle rows on 430
    // mm arcs: two ends, each up to 1e-10 rad × 430 mm, bound it by 8.6e-8 mm (hand), and
    // the sketch's tolerance is 1e-9 × 4,289.28 = 4.29e-6 mm. The same slack leaves the
    // corners off the analytic slot by up to that bound too (p2 lands 2e-8 out), so they're
    // checked to 1e-7.
    let doc = checked(&slot(
        0.0,
        0.0,
        4289.278884,
        430.346545,
        [
            0.059784654735803223,
            -0.17701446655438366,
            0.1294286237483277,
            -0.002774020427998682,
            0.1762997203303423,
            0.16106275270293036,
        ],
    ))
    .unwrap();
    let r = case(&doc, "slot 4289 r 430", &mut worker(), |s| {
        slot_within(s, 0.0, 0.0, 4289.278884, 430.346545, 1e-7)
            && s.redundant.values().all(|&gap| gap <= 1e-7)
    });
    assert!(r.is_ok(), "{}", r.unwrap_err());
}

#[test]
fn a_fat_slot_keeps_its_redundancies_by_the_sketchs_longest_lever() {
    // Fable's case for the sketch-wide lever: sides 1 mm long, arcs of radius 100, whose
    // rows carry the slack to the sides' equality; the sketch's longest lever, 100, gives a
    // tolerance of 1e-7. Kept, from starts a little off, each gap within 2e-8. (Of 3,000
    // starts drawn up to 0.2 off, the largest gap was 2.15e-9, the last start here, so the
    // sides' own length would keep these too; the lever is pinned by the core's stand-in
    // tests, a_sketchs_tolerance_is_its_longest_lever_..., not here.)
    let mut w = worker();
    for jitter in [
        [0.1, -0.1, 0.05, 0.05, -0.1, 0.1],
        [-0.2, 0.15, -0.1, 0.0, 0.2, -0.05],
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.17, 0.03, -0.12, 0.09, 0.04, -0.18],
        [
            -0.029471552426800385,
            -0.1319170970587054,
            -0.07179943605187483,
            0.17774541629131313,
            -0.04942675742535213,
            0.16435915483845925,
        ],
    ] {
        let doc = checked(&slot(0.0, 0.0, 1.0, 100.0, jitter)).unwrap();
        let r = case(&doc, &format!("fat slot {jitter:?}"), &mut w, |s| {
            slot_is(s, 0.0, 0.0, 1.0, 100.0) && s.redundant.values().all(|&gap| gap <= 2e-8)
        });
        assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

#[test]
fn a_redundant_slot_saved_by_silico_loads_without_moving() {
    // Found by break-it on the first solve again, fixed: a 0.670725 mm slot of radius
    // 16.786477 at the origin, drawn a little off. The save's first solve dropped k0000015;
    // its solve again, from the solver's own values, dropped k0000009 and k0000015 and
    // didn't move, so the loop called it settled. But that start isn't a load's: a load
    // starts from the points the file stores and works each arc's radius and angles out
    // from them (Translation::new), which differ from the solver's own by ulps (a far
    // slot's: radius 93.111393 against 93.11139299999923, an angle 2.6e-14 off). Diagnosed
    // there, PlaneGCS picked other constraints and moved points by an ulp, so `silico
    // check` reported drift on a file Silico saved and nobody edited. Now the loop solves
    // the sketch as a load does, written as the file holds it and translated again: the
    // load gives the saved bits.
    let doc = checked(&slot(
        0.0,
        0.0,
        0.670725,
        16.786477,
        [
            0.051804177088782044,
            -0.06501281476058751,
            -0.07722122613058052,
            0.0044180058732064265,
            0.1190593453395338,
            0.056550846532394644,
        ],
    ))
    .unwrap();
    let r = case(&doc, "slot 0.670725 r 16.786477", &mut worker(), |s| {
        slot_is(s, 0.0, 0.0, 0.670725, 16.786477)
    });
    assert!(r.is_ok(), "{}", r.unwrap_err());
}

#[test]
fn a_sketch_with_every_constraint_given_three_times_is_saved_in_two_solves_and_loaded_in_one() {
    // Each constraint three times: PlaneGCS drops two of every three. A load from a solution
    // DogLeg meets as it starts returns it untouched: the save is the first solve and one
    // load, and the load of the saved file is one solve (it leaves its file as drawn).
    let mut w = Counting {
        inner: worker(),
        sent: Vec::new(),
        replies: Vec::new(),
    };
    let doc = checked(&rectangle(
        3.0,
        -4.0,
        25.0,
        7.0,
        [0.1, -0.05, 0.2, 0.0, -0.1, 0.1],
        3,
    ))
    .unwrap();
    let r = case(&doc, "rectangle thrice", &mut w, |s| {
        rectangle_is(s, 3.0, -4.0, 25.0, 7.0)
    });
    assert!(r.is_ok(), "{}", r.unwrap_err());
    assert_eq!(w.sent.len(), 2 + 1, "solves");
}

#[test]
fn the_same_redundant_sketch_in_two_workers_gives_the_same_answer() {
    // Determinism across the loop: the number of solves again and the answer that stands.
    let doc = checked(&slot(
        123.456,
        -78.9,
        4321.0,
        0.05,
        [0.2, -0.1, 0.15, 0.05, -0.2, 0.1],
    ))
    .unwrap();
    let run = || {
        let mut w = worker();
        match solved(&doc, &mut w) {
            Ok(s) => said(&s),
            Err(e) => format!("{e} {:?}", e.elements()),
        }
    };
    assert_eq!(run(), run());
}

#[test]
fn a_large_redundant_chain_is_saved_in_two_solves() {
    // 200 lines in a chain, each horizontal twice and equal to the next, the first held:
    // every redundancy agrees. The load from the solution re-diagnoses 1,000 equations,
    // once: it leaves the solution where it is.
    let n = 200;
    let mut points = Vec::new();
    let mut lines = Vec::new();
    let mut constraints = vec![
        c(
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#,
        ),
        c(
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#,
        ),
        c(
            r#"{"id": "k0000003", "type": "length", "line": "n0000000", "value": 5, "driving": true}"#,
        ),
    ];
    for i in 0..=n {
        points.push(point(
            &format!("p{i:07}"),
            5.0 * i as f64 + 0.1 * ((i % 3) as f64),
            0.05 * ((i % 5) as f64),
        ));
    }
    for i in 0..n {
        lines.push(line(
            &format!("n{i:07}"),
            &format!("p{i:07}"),
            &format!("p{:07}", i + 1),
        ));
        constraints.push(format!(
            r#"{{"id": "h{i:07}", "type": "horizontal", "line": "n{i:07}"}}"#
        ));
        constraints.push(format!(
            r#"{{"id": "j{i:07}", "type": "horizontal", "line": "n{i:07}"}}"#
        ));
        if i + 1 < n {
            constraints.push(format!(
                r#"{{"id": "e{i:07}", "type": "equal", "a": "n{i:07}", "b": "n{:07}"}}"#,
                i + 1
            ));
        }
    }
    let doc = checked(&text(&points, &lines, &[], &constraints)).unwrap();
    let mut w = Counting {
        inner: worker(),
        sent: Vec::new(),
        replies: Vec::new(),
    };
    let s = solved(&doc, &mut w);
    let solves = w.sent.len();
    let s = s.unwrap_or_else(|e| panic!("{e} {:?}", e.elements()));
    assert_eq!(
        s.point(&silico_core::document::ElementId::new("p0000200").unwrap()),
        Some([1000.0, 0.0])
    );
    // The first solve and one load, which leaves the solution where it is.
    assert_eq!(solves, 2, "solves");
    let again = written_back(&doc, &s);
    let l = solved(&again, &mut w.inner).unwrap();
    assert_eq!(said(&l), said(&s));
}

#[test]
fn after_a_solve_again_the_worker_solves_the_next_sketch_as_a_fresh_one_does() {
    // The worker is reused across the loop's requests: the next, unrelated sketch must come
    // out as it does in a fresh worker.
    let probe = checked(&triangle(1.0, 2.0, 30.0, [0.1, 0.05, -0.1, 0.02])).unwrap();
    let fresh = solved(&probe, &mut worker())
        .map(|s| said(&s))
        .map_err(|e| e.to_string());
    let mut w = worker();
    let doc = checked(&rectangle(
        0.0,
        0.0,
        2.395781,
        1984.774709,
        [-0.28, -0.07, -0.29, 0.0, 0.17, -0.22],
        2,
    ))
    .unwrap();
    let _ = solved(&doc, &mut w);
    let reused = solved(&probe, &mut w)
        .map(|s| said(&s))
        .map_err(|e| e.to_string());
    assert_eq!(fresh, reused);
}

// --- Properties ------------------------------------------------------------------------------

thread_local!(static W: std::cell::RefCell<SketchWorker> = std::cell::RefCell::new(worker()));

fn report(r: Result<(), Broke>) -> Result<(), TestCaseError> {
    match r {
        Ok(()) => Ok(()),
        Err(b) => Err(TestCaseError::fail(b.to_string())),
    }
}

/// Whether a sketch reaching `far` from the origin, its smallest size `size`, is where
/// doubles can't hold PlaneGCS's convergence (1e-10, FreeCAD's): the spacing of its
/// coordinates is at least 1e-10 mm (a length's residual) or 1e-10 rad across `size` (an
/// angle's). There a load from an exact solution may not converge (`Converged`, FreeCAD's
/// solver failure on recompute too), and a save is refused as its load is.
fn past_the_solvers_precision(far: f64, size: f64) -> bool {
    let spacing = f64::from_bits(far.to_bits() + 1) - far;
    spacing >= 1e-10 * size.min(1.0)
}

/// `r`, with a solve_failed refusal taken as holding where `limited`
/// ([`past_the_solvers_precision`]): a consistent sketch the solver can't meet there,
/// refused on save as on load, which is the property's invariant. Any refusal by decision 5
/// of these agreeing redundancies is a break.
fn unless_past_precision(r: Result<(), Broke>, limited: bool) -> Result<(), Broke> {
    match r {
        Err(Broke::Refused(ErrorCode::SolveFailed, _)) if limited => Ok(()),
        r => r,
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, .. ProptestConfig::default() })]

    /// A redundant rectangle, its constraints given once or twice: the answer a save keeps
    /// is the answer its load gives, whole.
    #[test]
    fn a_redundant_rectangles_load_gives_the_answer_its_save_kept(
        log_w in -3.0f64..5.0,
        log_h in -3.0f64..5.0,
        place in prop_oneof![Just(0.0), Just(999_000.0), -999_000.0f64..999_000.0],
        jitter in proptest::array::uniform6(-0.3f64..0.3),
        times in 1usize..=2,
    ) {
        let (wd, ht) = (nm(libm::pow(10.0, log_w)), nm(libm::pow(10.0, log_h)));
        prop_assume!(wd > 0.0 && ht > 0.0 && (log_w - log_h).abs() <= 8.0);
        let room = 1e6 - 2.0 * (wd + ht);
        let (x0, y0) = (nm(place.clamp(-room, room)), nm((-place).clamp(-room, room)));
        let Ok(doc) = checked(&rectangle(x0, y0, wd, ht, jitter, times)) else {
            return Ok(());
        };
        let what = format!("rectangle {wd} x {ht} at ({x0}, {y0}), jitter {jitter:?}, times {times}");
        let r = W.with(|w| case(&doc, &what, &mut *w.borrow_mut(), |s| rectangle_is(s, x0, y0, wd, ht)));
        report(r)?;
    }

    /// Two equal circles, their radii given redundantly: a load starts from the radii a
    /// save writes (the council, on the first check by load).
    #[test]
    fn redundant_equal_circles_load_gives_the_answer_its_save_kept(
        log_r in -3.0f64..5.0,
        log_d in -3.0f64..5.0,
        place in prop_oneof![Just(0.0), Just(999_000.0), -999_000.0f64..999_000.0],
        j0 in -0.4f64..0.4,
        j1 in -0.4f64..0.4,
    ) {
        let (r, d) = (nm(libm::pow(10.0, log_r)), nm(libm::pow(10.0, log_d)));
        prop_assume!(r > 0.0 && d > 0.0 && 2.0 * r < 1e6);
        let room = 1e6 - 2.0 * (r + d);
        prop_assume!(room > 0.0);
        let (x0, y0) = (nm(place.clamp(-room, room)), nm((-place).clamp(-room, room)));
        let Ok(doc) = checked(&equal_circles(x0, y0, r, d, [j0, j1])) else {
            return Ok(());
        };
        let what = format!("equal circles r {r} d {d} at ({x0}, {y0}), jitter {:?}", [j0, j1]);
        let res = W.with(|w| case(&doc, &what, &mut *w.borrow_mut(), |s| equal_circles_are(s, r)));
        let far = x0.abs().max(y0.abs()) + d + r;
        report(unless_past_precision(res, past_the_solvers_precision(far, r.min(d))))?;
    }

    // Found by break-it on the first solve again, fixed: about 1 in 2,000 slots loaded a
    // few ulps (up to 2.6e-10 mm) from where their save put them (the regression file's
    // case; a_redundant_slot_saved_by_silico_loads_without_moving): the solve again started
    // from the solver's arc values, not a load's. About 1 in 10,000, far out, can't be met
    // by a load from their exact solution and are refused on save as on load
    // (past_the_solvers_precision). About 1 in 20,000 was refused by decision 5's fixed
    // tolerances (a_long_slot_keeps_its_equal_sides_within_the_slack_its_arcs_allow), until
    // its positional rule; now any such refusal is a break.
    /// A redundant slot: arcs, whose angles a load works out from the saved points rather
    /// than taking the solve's own.
    #[test]
    fn a_redundant_slots_load_gives_the_answer_its_save_kept(
        log_len in -2.0f64..5.0,
        log_r in -3.0f64..3.0,
        place in prop_oneof![Just(0.0), Just(999_000.0), -999_000.0f64..999_000.0],
        jitter in proptest::array::uniform6(-0.2f64..0.2),
    ) {
        let (len, r) = (nm(libm::pow(10.0, log_len)), nm(libm::pow(10.0, log_r)));
        prop_assume!(len > 0.0 && r > 0.0);
        let room = 1e6 - 2.0 * (len + 2.0 * r);
        prop_assume!(room > 0.0);
        let (x0, y0) = (nm(place.clamp(-room, room)), nm((-place).clamp(-room, room)));
        let Ok(doc) = checked(&slot(x0, y0, len, r, jitter)) else {
            return Ok(());
        };
        let what = format!("slot {len} r {r} at ({x0}, {y0}), jitter {jitter:?}");
        let res = W.with(|w| case(&doc, &what, &mut *w.borrow_mut(), |s| slot_is(s, x0, y0, len, r)));
        let far = x0.abs().max(y0.abs()) + len + 2.0 * r;
        let limited = past_the_solvers_precision(far, len.min(r));
        report(unless_past_precision(res, limited))?;
    }

    // Found by break-it on the first solve again, fixed: about 1 in 250 triangles far out
    // saved Ok and loaded refused, as conflicting or as solve_failed (the regression file's
    // case; an_equilateral_triangle_far_out_is_refused_on_save_as_its_load_refuses_it); now
    // the save is refused as its load is. About 1 in 1,000 under 0.1 mm was refused by
    // decision 5's fixed angle tolerance
    // (a_micrometre_equilateral_triangle_keeps_its_agreeing_angle), until its positional
    // rule; now any such refusal is a break.
    /// A triangle held by its angles as well as its sides.
    #[test]
    fn a_redundant_triangles_load_gives_the_answer_its_save_kept(
        log_side in -3.0f64..5.5,
        place in prop_oneof![Just(0.0), Just(999_000.0), -999_000.0f64..999_000.0],
        jitter in proptest::array::uniform4(-0.2f64..0.2),
    ) {
        let side = nm(libm::pow(10.0, log_side));
        prop_assume!(side > 0.0);
        let room = 1e6 - 2.0 * side;
        let (x0, y0) = (nm(place.clamp(-room, room)), nm((-place).clamp(-room, room)));
        let Ok(doc) = checked(&triangle(x0, y0, side, jitter)) else {
            return Ok(());
        };
        let what = format!("triangle {side} at ({x0}, {y0}), jitter {jitter:?}");
        let res = W.with(|w| case(&doc, &what, &mut *w.borrow_mut(), |s| triangle_is(s, x0, y0, side)));
        let far = x0.abs().max(y0.abs()) + 2.0 * side;
        report(unless_past_precision(res, past_the_solvers_precision(far, side)))?;
    }
}

/// A rectangle w × h at the origin, held and dimensioned once (as the property above, with
/// no redundancy but its horizontals' and verticals' pairing), with its width stated a
/// second way: a horizontal distance from p1 to p2 of w + `more`.
fn rectangle_with_width_twice(w: f64, h: f64, more: f64) -> String {
    let text = rectangle(0.0, 0.0, w, h, [0.0; 6], 1);
    let twice = format!(
        r#"{{"id": "k0000099", "type": "distance_x", "a": "p0000001", "b": "p0000002", "value": {}, "driving": true}}"#,
        nm(w + more)
    );
    text.replacen(
        "\"constraints\": [",
        &format!("\"constraints\": [{twice}, "),
        1,
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 500, .. ProptestConfig::default() })]

    /// Decision 5's converse (Fable, 2026-10-18): on any sketch under a metre, two
    /// dimensions a nanometre apart conflict. A rectangle whose longest side is under 999
    /// mm (so its tolerance, 1e-9 × the side, is under 1e-6), its width stated again 1 nm
    /// more, is always over_constrained.
    #[test]
    fn two_dimensions_a_nanometre_apart_on_a_sketch_under_a_metre_conflict(
        log_w in -1.0f64..2.999,
        log_h in -1.0f64..2.999,
    ) {
        let (w, h) = (nm(libm::pow(10.0, log_w)), nm(libm::pow(10.0, log_h)));
        prop_assume!(w > 0.0 && h > 0.0 && w.max(h) < 999.0);
        let doc = checked(&rectangle_with_width_twice(w, h, 1e-6)).unwrap();
        let got = W.with(|wk| solved(&doc, &mut *wk.borrow_mut()));
        let e = got.map(|s| said(&s)).unwrap_err();
        prop_assert_eq!(e.code(), ErrorCode::OverConstrained, "{}", e);
    }
}
