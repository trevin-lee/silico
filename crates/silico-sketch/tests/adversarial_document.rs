//! /break-it on M3.3: documents' sketches solved through the sketch worker
//! (silico_core::sketch), with hostile, degenerate, and edge-of-tolerance inputs. Each test
//! says where its expected answer comes from. Each break they exposed is fixed, and marked
//! where it was found.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::document::{Checked, ElementId, FeatureId, Operation, read, write};
use silico_core::sketch::{SketchSolution, SketchSolver, SketchWorker, Translation, solve};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
const FEATURE: &str = "k7f2qx3m";
const CLOSE: f64 = 1e-8;

fn worker() -> SketchWorker {
    SketchWorker::start_worker(Path::new(WORKER)).unwrap()
}

fn point(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
}

fn line(id: &str, from: &str, to: &str) -> String {
    format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#)
}

fn arc(id: &str, center: &str, start: &str, end: &str) -> String {
    format!(
        r#"{{"id": "{id}", "center": "{center}", "start": "{start}", "end": "{end}", "construction": false}}"#
    )
}

fn text(
    points: &[String],
    lines: &[String],
    circles: &[String],
    arcs: &[String],
    constraints: &[String],
) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"{FEATURE}\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", \"plane\": \"xy\", \"points\": [{}], \"lines\": [{}], \"circles\": [{}], \"arcs\": [{}], \"constraints\": [{}]}}]}}",
        points.join(", "),
        lines.join(", "),
        circles.join(", "),
        arcs.join(", "),
        constraints.join(", ")
    )
}

fn checked(text: &str) -> Checked {
    Checked::new(read(text.as_bytes()).unwrap())
        .unwrap_or_else(|e| panic!("the gate refused: {:?}", e.errors()))
}

fn solved(doc: &Checked, w: &mut dyn SketchSolver) -> Result<SketchSolution, silico_core::Error> {
    solve(doc, &FeatureId::new(FEATURE).unwrap(), w)
}

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn c(s: &str) -> String {
    s.to_owned()
}

/// Runs `f` on a thread, failing the test if it takes longer than `limit`.
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit)
        .unwrap_or_else(|_| panic!("took longer than {limit:?}"))
}

// --- The symmetric arc (FreeCAD 365af33fc6, decided from references) ------------------

/// A half circle about point c, its centre held on the y axis, from a at 30° to b at 210°,
/// and its ends symmetric about the y axis.
fn half_circle_symmetric_about_y(sweep_end_deg: f64) -> String {
    let r = 10.0;
    let (a0, a1) = (30f64.to_radians(), sweep_end_deg.to_radians());
    text(
        &[
            point("pc000000", 0.0, 0.0),
            point("pa000000", r * libm::cos(a0), r * libm::sin(a0)),
            point("pb000000", r * libm::cos(a1), r * libm::sin(a1)),
        ],
        &[],
        &[],
        &[arc("aa000000", "pc000000", "pa000000", "pb000000")],
        &[
            c(r#"{"id": "k0000001", "type": "on", "point": "pc000000", "curve": "y_axis"}"#),
            c(
                r#"{"id": "k0000002", "type": "symmetric", "a": "pa000000", "b": "pb000000", "about": "y_axis"}"#,
            ),
        ],
    )
}

#[test]
fn half_circle_ends_symmetric_about_a_line_through_its_centre_are_mirrored() {
    // Found by break-it, now refused: for an arc's own ends about a line its centre is held on, only the midpoint
    // row is sent; on a half circle the chord's midpoint is the centre, so that row is the
    // centre-on-line constraint again (redundant, and met), and the ends are never mirrored.
    // The solve says Ok with a at (8.66, 5) and b at (-8.66, -5): mirror(b) = (8.66, -5) ≠ a.
    // FreeCAD 1.1.3 sends the same single row here (Sketch::addSymmetricConstraint), so it
    // shares the bug. Expected: either the ends mirrored (a.x = -b.x, a.y = b.y, an analytic
    // consequence of the constraint) or a refusal; never Ok with the constraint unmet. The
    // core now checks the symmetry whole on the solution, and refuses it as an invalid
    // result, naming it.
    let doc = checked(&half_circle_symmetric_about_y(210.0));
    match solved(&doc, &mut worker()) {
        Ok(s) => {
            let (a, b) = (
                s.point(&id("pa000000")).unwrap(),
                s.point(&id("pb000000")).unwrap(),
            );
            assert!(
                (a[0] + b[0]).abs() <= CLOSE && (a[1] - b[1]).abs() <= CLOSE,
                "symmetric ends not mirrored about the y axis: a {a:?}, b {b:?}, {s:?}"
            );
        }
        Err(e) => assert!(
            matches!(
                e.code(),
                ErrorCode::OverConstrained
                    | ErrorCode::InvalidInput
                    | ErrorCode::SolveFailed
                    | ErrorCode::InvalidResult
            ) && e.elements().iter().any(|x| x == "k0000002"),
            "{e}"
        ),
    }
}

#[test]
fn nearly_half_circle_ends_symmetric_about_a_line_through_its_centre_are_mirrored() {
    // One degree short of a half circle: the midpoint row and the centre on the line would
    // pin the chord's perpendicular bisector to the y axis. Found by break-it, fixed: the
    // solve moved the centre to the chord's midpoint instead, (0, 0.076), making it a half
    // circle, and said Ok with a (8.663, 5) and b (-8.663, -4.848); the symmetry is now
    // checked whole on the solution, and refused unmet.
    let doc = checked(&half_circle_symmetric_about_y(209.0));
    match solved(&doc, &mut worker()) {
        Ok(s) => {
            let (a, b) = (
                s.point(&id("pa000000")).unwrap(),
                s.point(&id("pb000000")).unwrap(),
            );
            assert!(
                (a[0] + b[0]).abs() <= CLOSE && (a[1] - b[1]).abs() <= CLOSE,
                "a {a:?}, b {b:?}, {s:?}"
            );
        }
        Err(e) => assert!(e.elements().iter().any(|x| x == "k0000002"), "{e}"),
    }
}

// --- Symmetries that force their two points to one place --------------------------------

/// Line n from a (0, 0) to b (10, 0), held; p at (3, 4), and a's mirror about n is p.
fn end_mirrored_about_its_own_line(p: [f64; 2]) -> String {
    text(
        &[
            point("pa000000", 0.0, 0.0),
            point("pb000000", 10.0, 0.0),
            point("pp000000", p[0], p[1]),
        ],
        &[line("na000000", "pa000000", "pb000000")],
        &[],
        &[],
        &[
            c(
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "pa000000", "value": 0, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "pa000000", "value": 0, "driving": true}"#,
            ),
            c(r#"{"id": "k0000003", "type": "horizontal", "line": "na000000"}"#),
            c(
                r#"{"id": "k0000004", "type": "length", "line": "na000000", "value": 10, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000005", "type": "symmetric", "a": "pa000000", "b": "pp000000", "about": "na000000"}"#,
            ),
        ],
    )
}

#[test]
fn a_lines_end_mirrored_about_that_line_is_refused_by_the_gate() {
    // A point on a line is its own mirror, so p's only solution is a's place, (0, 0): two
    // points at one place, which the next solve refuses (a symmetry measured by the line
    // between them). Found by break-it: it solved once, then the sketch drawn at its solution
    // was refused. The gate now refuses one end as it refused both, naming the constraint.
    let text = end_mirrored_about_its_own_line([3.0, 4.0]);
    let invalid = Checked::new(read(text.as_bytes()).unwrap()).unwrap_err();
    let e = &invalid.errors()[0];
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.elements(), ["k0000005"]);
    assert!(e.message().ends_with("so it mirrors onto itself"), "{e}");
}

// --- Solutions out of reach --------------------------------------------------------------

#[test]
fn a_solution_past_a_kilometre_names_the_point_that_went_there() {
    // q is held a kilometre right of p, which is held a kilometre right of the origin: q's
    // only place is x = 2e6, past what the worker returns. The refusal must say which
    // element, and what to do, and the worker must still solve the next sketch.
    let mut w = worker();
    let doc = checked(&text(
        &[point("pp000000", 0.0, 0.0), point("pq000000", 5.0, 0.0)],
        &[line("na000000", "pp000000", "pq000000")],
        &[],
        &[],
        &[
            c(
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "pp000000", "value": 1000000, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000002", "type": "distance_x", "a": "pp000000", "b": "pq000000", "value": 1000000, "driving": true}"#,
            ),
        ],
    ));
    let e = solved(&doc, &mut w).unwrap_err();
    // Found by break-it, fixed: the error was `kernel_failed: Solve failed`, its cause "the
    // sketch solver's solution has a value that isn't finite or is more than 1000000 from 0
    // (value 8)": no feature, no element (value 8 is pq000000's x, which the core could
    // name), and a code that blames a kernel. A solution out of reach is now one that can't
    // be built: the worker passes it over, and the core names the point.
    assert!(
        e.elements().iter().any(|x| x == "pq000000"),
        "the error doesn't name the point: {e} / {e:?}"
    );
    assert!(e.suggestion().is_some(), "{e:?}");
    // The worker still works.
    let ok = checked(&text(
        &[point("pp000000", 0.0, 0.0), point("pq000000", 5.0, 0.0)],
        &[line("na000000", "pp000000", "pq000000")],
        &[],
        &[],
        &[],
    ));
    assert_eq!(solved(&ok, &mut w).unwrap().dofs, 4);
}

// --- Arcs the core refuses in translation ------------------------------------------------

#[test]
fn an_arc_whose_end_is_further_out_along_its_start_is_refused_naming_it_and_the_sketch() {
    // Centre (0, 0), start (10, 0), end (20, 0): one angle, no sweep. The gate takes it
    // (its ends are apart); the translation refuses it, naming the arc and the feature.
    let doc = checked(&text(
        &[
            point("pc000000", 0.0, 0.0),
            point("pa000000", 10.0, 0.0),
            point("pb000000", 20.0, 0.0),
        ],
        &[],
        &[],
        &[arc("aa000000", "pc000000", "pa000000", "pb000000")],
        &[],
    ));
    let e = solved(&doc, &mut worker()).unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
    assert_eq!(e.elements(), ["aa000000"]);
    assert_eq!(e.feature(), Some(FEATURE));
    assert!(e.suggestion().is_some());
}

#[test]
fn an_arc_ending_just_below_its_start_on_the_far_side_sweeps_nearly_a_whole_turn() {
    // Start at (-10, +0), end at (-10, -1e-300): atan2 gives π and −π + tiny, a turn on is
    // just under π + ... The sweep is nearly a whole turn and must solve as an arc, or be
    // refused naming it; never a panic or an unnamed error.
    let doc = checked(&text(
        &[
            point("pc000000", 0.0, 0.0),
            point("pa000000", -10.0, 0.0),
            point("pb000000", -10.0, -1e-300),
        ],
        &[],
        &[],
        &[arc("aa000000", "pc000000", "pa000000", "pb000000")],
        &[],
    ));
    match solved(&doc, &mut worker()) {
        Ok(s) => assert!(s.free.contains(&id("aa000000")), "{s:?}"),
        Err(e) => assert_eq!(e.elements(), ["aa000000"], "{e}"),
    }
}

// --- The gate's size limits against what the translation sends ---------------------------

#[test]
fn a_sketch_at_the_gates_value_limit_is_one_the_worker_takes() {
    // Two points (4 unknowns) and 9,990 driving dimensions: the gate counts 4 + 9,990 + 6
    // (the origin and the axes' far ends) = 10,000 values, at MAX_SKETCH_VALUES, and lets it
    // through; the worker takes what the translation sends. Found by break-it: the gate
    // counted 2 fixed values where the translation sends 6, so a sketch it admitted at its
    // limit reached the worker 4 over, refused.
    let mut constraints = Vec::new();
    for i in 0..9990 {
        constraints.push(format!(
            r#"{{"id": "d{i:07}", "type": "distance_x", "a": "pp000000", "b": "pq000000", "value": 5, "driving": true}}"#
        ));
    }
    let doc = checked(&text(
        &[point("pp000000", 0.0, 0.0), point("pq000000", 5.0, 0.0)],
        &[],
        &[],
        &[],
        &constraints,
    ));
    let Operation::Sketch(sketch) = &doc.document().features[0].operation else {
        panic!()
    };
    let t = Translation::new(sketch).unwrap();
    assert!(
        t.system.check().is_ok(),
        "the gate passed it, the worker's check refuses: {:?}",
        t.system.check().unwrap_err()
    );
}

#[test]
fn a_sketch_at_the_gates_geometry_limit_is_one_the_worker_takes() {
    // 100 points and 9,897 lines among them, and the origin and the two axes the translation
    // adds: 10,000 pieces of geometry, at MAX_SKETCH_GEOMETRY. Found by break-it: the gate
    // didn't count those three.
    let points: Vec<String> = (0..100)
        .map(|i| point(&format!("p{i:07}"), i as f64, (i * i) as f64))
        .collect();
    let mut lines = Vec::new();
    'all: for a in 0..100 {
        for b in 0..100 {
            if a != b {
                lines.push(line(
                    &format!("n{:07}", lines.len()),
                    &format!("p{a:07}"),
                    &format!("p{b:07}"),
                ));
                if lines.len() == 9897 {
                    break 'all;
                }
            }
        }
    }
    let doc = checked(&text(&points, &lines, &[], &[], &[]));
    let Operation::Sketch(sketch) = &doc.document().features[0].operation else {
        panic!()
    };
    let t = Translation::new(sketch).unwrap();
    assert!(
        t.system.check().is_ok(),
        "the gate passed it, the worker's check refuses: {:?}",
        t.system.check().unwrap_err()
    );
}

// --- The fallback's cost on the largest sketch the gate admits ---------------------------

/// A chain of `n` points, each 1 from the next, the first at the origin and the last
/// held `reach` right of it: unreachable when `reach` > n - 1, so every algorithm fails and
/// the anchored SQP step runs on one component of 2n unknowns.
fn impossible_chain(n: usize, reach: f64) -> String {
    let points: Vec<String> = (0..n)
        .map(|i| point(&format!("p{i:07}"), i as f64 * 0.7, (i % 2) as f64 * 0.5))
        .collect();
    let mut constraints = vec![
        c(
            r#"{"id": "a0000001", "type": "distance_x", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#,
        ),
        c(
            r#"{"id": "a0000002", "type": "distance_y", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#,
        ),
        format!(
            r#"{{"id": "a0000003", "type": "distance_x", "a": "origin", "b": "p{:07}", "value": {reach}, "driving": true}}"#,
            n - 1
        ),
    ];
    for i in 1..n {
        constraints.push(format!(
            r#"{{"id": "d{i:07}", "type": "distance", "a": "p{:07}", "b": "p{i:07}", "value": 1, "driving": true}}"#,
            i - 1
        ));
    }
    text(&points, &[], &[], &[], &constraints)
}

/// `impossible_chain(8, 50.0)` with one more line, m1 from q1 (0, 20) to q2 (5, 21), given
/// `horizontal` `n` times.
fn impossible_chain_and_a_line(horizontals: usize) -> String {
    let chain = impossible_chain(8, 50.0);
    let line = line("m0000001", "q0000001", "q0000002");
    let points = format!(
        "{}, {}",
        point("q0000001", 0.0, 20.0),
        point("q0000002", 5.0, 21.0)
    );
    let constraints: Vec<String> = (1..=horizontals)
        .map(|i| format!(r#"{{"id": "h000000{i}", "type": "horizontal", "line": "m0000001"}}"#))
        .collect();
    let mut text = chain.replacen("\"points\": [", &format!("\"points\": [{points}, "), 1);
    text = text.replacen("\"lines\": []", &format!("\"lines\": [{line}]"), 1);
    for c in constraints {
        text = text.replacen("\"constraints\": [", &format!("\"constraints\": [{c}, "), 1);
    }
    text
}

#[test]
fn a_constraint_given_twice_beside_an_impossible_chain_conflicts_and_once_fails() {
    // Fable's case (2026-10-17): the two horizontals are one row twice, so one dependent
    // group; PlaneGCS's trial solve without one of them is the impossible chain, which
    // fails, so neither is redundant (GCS.cpp:5578) and both are conflicting, which the core
    // reads first: over_constrained, naming exactly them. With one, nothing repeats, and
    // the impossible chain is solve_failed. FreeCAD 1.1.3 gives -3 with both conflicting.
    let mut w = worker();
    let e = solved(&checked(&impossible_chain_and_a_line(2)), &mut w).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert_eq!(e.elements(), ["h0000001", "h0000002"]);
    assert_eq!(e.elements_total(), 2);
    assert_eq!(e.feature(), Some(FEATURE));
    let e = solved(&checked(&impossible_chain_and_a_line(1)), &mut w).unwrap_err();
    assert_eq!(e.code(), ErrorCode::SolveFailed, "{e}");
}

#[test]
fn an_impossible_chain_of_the_largest_size_fails_as_solve_failed_not_a_timeout() {
    // 512 points (1,024 unknowns, at MAX_SKETCH_UNKNOWNS) in one chain of unit links held
    // 2,560 apart: the gate admits it, and every algorithm fails. The answer is
    // solve_failed, not a timeout. The worker is given 900 s, so this tests what the answer
    // is, not the machine's speed: the default, SKETCH_TIMEOUT's 300 s, is a measurement
    // (74 s on a quiet-ish M4 Pro, 206 s at a load average of 80; ~/.cache/silico-review's
    // m3-open notes), and that a worker waits the time it's given is document.rs's.
    // Found by break-it: worker_timeout after 60 s, the kernel's request time. Measured one
    // algorithm at a time on this chain (silico_sketch::solve, a loaded M4 Pro): DogLeg
    // 11.5 s, Levenberg–Marquardt 28.9 s, BFGS 0.2 s, anchored SQP 33.2 s: 74 s in all, where
    // the three M3.1 tried took about 40 s. A sketch solve now has its own time,
    // silico_core::sketch::SKETCH_TIMEOUT (300 s), which SketchWorker::with_timeout sets.
    let doc = checked(&impossible_chain(512, 2560.0));
    let started = Instant::now();
    let given = Duration::from_secs(900);
    let (e, cpu) = within(given + Duration::from_secs(60), move || {
        let mut w = worker().with_timeout(given);
        let e = solved(&doc, &mut w).unwrap_err();
        (e, cpu_time(w.pid()))
    });
    let took = started.elapsed();
    assert_eq!(
        e.code(),
        ErrorCode::SolveFailed,
        "after {took:?}: {e} / {e:?}"
    );
    // That SKETCH_TIMEOUT covers it, held to what load can't move much: the worker's own
    // CPU time for the whole solve (about 64 s at loads of 55 to 97, round 2's skeptic),
    // within the default with room to spare, so the shipped time answers what the gate
    // admits (round 3's tests lens: a default of 60 s, break-it's bug, would fail here).
    assert!(
        cpu < silico_core::sketch::SKETCH_TIMEOUT / 2,
        "the worker spent {cpu:?} of CPU, {took:?} in all"
    );
}

/// The CPU time process `pid` has used, as `ps` reports it (`[[dd-]hh:]mm:ss[.ss]`).
fn cpu_time(pid: u32) -> Duration {
    let out = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let text = text.trim();
    let (days, clock) = text
        .split_once('-')
        .map_or((0.0, text), |(d, c)| (d.parse::<f64>().unwrap(), c));
    let seconds = clock.split(':').fold(0.0, |total, part| {
        total * 60.0 + part.parse::<f64>().unwrap()
    });
    Duration::from_secs_f64(days * 86_400.0 + seconds)
}

// --- Decision 5's tolerances at the edges of what the gate admits --------------------------

/// A rectangle w × h with its lower-left corner held at (x0, y0), drawn off true by `jitter`
/// (a fraction of its size), constrained with every redundancy that agrees: both pairs of
/// sides horizontal and vertical, and parallel, perpendicular, and equal on top.
fn redundant_rectangle(x0: f64, y0: f64, w: f64, h: f64, jitter: [f64; 6], rough: bool) -> String {
    // Off by a fraction of the smaller side, or (rough) a sixth of that fraction of the
    // larger: a rectangle drawn roughly, its short sides leaning.
    let j = |k: usize, _: f64| {
        if rough {
            jitter[k] / 6.0 * w.max(h)
        } else {
            jitter[k] * w.min(h)
        }
    };
    let points = [
        point("p0000001", x0, y0),
        point("p0000002", x0 + w + j(0, w), y0 + j(1, h)),
        point("p0000003", x0 + w + j(2, w), y0 + h + j(3, h)),
        point("p0000004", x0 + j(4, w), y0 + h + j(5, h)),
    ];
    let lines = [
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000002", "p0000003"),
        line("n0000003", "p0000003", "p0000004"),
        line("n0000004", "p0000004", "p0000001"),
    ];
    let constraints = [
        format!(
            r#"{{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": {x0}, "driving": true}}"#
        ),
        format!(
            r#"{{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": {y0}, "driving": true}}"#
        ),
        c(r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#),
        c(r#"{"id": "k0000004", "type": "vertical", "line": "n0000002"}"#),
        c(r#"{"id": "k0000005", "type": "horizontal", "line": "n0000003"}"#),
        c(r#"{"id": "k0000006", "type": "vertical", "line": "n0000004"}"#),
        format!(
            r#"{{"id": "k0000007", "type": "length", "line": "n0000001", "value": {w}, "driving": true}}"#
        ),
        format!(
            r#"{{"id": "k0000008", "type": "length", "line": "n0000002", "value": {h}, "driving": true}}"#
        ),
        c(r#"{"id": "k0000009", "type": "parallel", "a": "n0000001", "b": "n0000003"}"#),
        c(r#"{"id": "k0000010", "type": "perpendicular", "a": "n0000001", "b": "n0000002"}"#),
        c(r#"{"id": "k0000011", "type": "equal", "a": "n0000002", "b": "n0000004"}"#),
    ];
    text(&points, &lines, &[], &[], &constraints)
}

/// A whole number of nanometres, as the file holds dimensions.
fn nm(v: f64) -> f64 {
    (v * 1e6).round() / 1e6 + 0.0
}

#[test]
fn a_small_redundant_rectangle_far_out_keeps_its_agreeing_constraints() {
    // A 0.01 × 0.01 mm rectangle a kilometre out, its redundancies all agreeing: spacing of
    // doubles there is 1.16e-10 mm, so a side's direction can't be known better than about
    // 1e-8 rad; carried by a 0.01 mm side that's 1e-10 mm, and the solve leaves the sides
    // exact.
    let doc = checked(&redundant_rectangle(
        999_000.0,
        999_000.0,
        0.01,
        0.01,
        [0.1, -0.2, 0.15, 0.1, -0.1, 0.05],
        false,
    ));
    let s = solved(&doc, &mut worker()).unwrap_or_else(|e| panic!("{e}"));
    assert!(!s.redundant.is_empty(), "{s:?}");
}

/// `doc`'s sketch with its points where `solution` puts them, through the file's text: the
/// sketch M3.4's re-solve on load starts from.
fn written_back(doc: &Checked, solution: &SketchSolution) -> Checked {
    let mut document = doc.document().clone();
    let Operation::Sketch(sketch) = &mut document.features[0].operation else {
        panic!("not a sketch")
    };
    *sketch = solution.written(sketch);
    checked(&write(&document).unwrap())
}

/// The bits of each point's coordinates, by ID.
fn point_bits(solution: &SketchSolution) -> Vec<u64> {
    solution
        .points
        .values()
        .flat_map(|[x, y]| [x.mm().to_bits(), y.mm().to_bits()])
        .collect()
}

/// A redundant rectangle from the property's inputs (sizes as powers of ten, a place, and
/// the jitter it's drawn off by), solved: Ok if it solves to the analytic corners within
/// CLOSE and is its own solution (M3.4: solved again from its solution as the file stores
/// it, it doesn't move), or if PlaneGCS's diagnosis on the drawing refuses it as
/// conflicting; otherwise what went wrong.
fn redundant_rectangle_case(
    log_w: f64,
    log_h: f64,
    place: f64,
    jitter: [f64; 6],
    rough: bool,
    w: &mut dyn SketchSolver,
) -> Result<(), String> {
    let (wide, high) = (nm(libm::pow(10.0, log_w)), nm(libm::pow(10.0, log_h)));
    let room = 1e6 - 2.0 * (wide + high);
    let (x0, y0) = (
        nm(place.clamp(-room, room)),
        nm((-place).clamp(-room, room)),
    );
    let doc = checked(&redundant_rectangle(x0, y0, wide, high, jitter, rough));
    let case = format!("w {wide}, h {high}, at ({x0}, {y0}), jitter {jitter:?}, rough {rough}");
    let s = match solved(&doc, w) {
        Ok(s) => s,
        // PlaneGCS's diagnosis, on the drawing, can call agreeing redundancies conflicting;
        // FreeCAD 1.1.3 says the same (adversarial_sketch_oracle.rs), so that's a spec
        // question, not this property's. Decision 5's own refusal of redundancies the
        // diagnosis found is.
        Err(e)
            if e.code() == ErrorCode::OverConstrained
                && e.message().starts_with("the sketch's constraints conflict") =>
        {
            return Ok(());
        }
        Err(e) => return Err(format!("{case}: {e} / {:?}", e.elements())),
    };
    // The analytic corners, either way up or round: a length has no direction, so a rough
    // drawing may land on the rectangle's mirror image.
    let at = |p: &str| s.point(&id(p)).unwrap();
    let (p1, p2, p3, p4) = (
        at("p0000001"),
        at("p0000002"),
        at("p0000003"),
        at("p0000004"),
    );
    let near = |a: f64, b: f64| (a - b).abs() <= CLOSE;
    let corners = near(p1[0], x0)
        && near(p1[1], y0)
        && near((p2[0] - x0).abs(), wide)
        && near(p2[1], y0)
        && near(p3[0], p2[0])
        && near((p3[1] - y0).abs(), high)
        && near(p4[0], x0)
        && near(p4[1], p3[1]);
    if !corners {
        return Err(format!(
            "{case}: corners {p1:?} {p2:?} {p3:?} {p4:?}, redundant {:?}",
            s.redundant
        ));
    }
    match solved(&written_back(&doc, &s), w) {
        Ok(again) if point_bits(&again) == point_bits(&s) => Ok(()),
        Ok(again) => Err(format!(
            "{case}: solved again from its solution, it moved: {s:?} to {again:?}"
        )),
        Err(e) => Err(format!("{case}: solved, then not from its solution: {e}")),
    }
}

#[test]
fn a_redundant_rectangle_kept_through_an_angle_along_a_long_side_is_exact_and_its_own_solution() {
    // Found by the property below, in CI, fixed: a 2.395781 × 1,984.774709 rectangle at the
    // origin solved, its redundancies agreeing, with its top-right corner 45 nm off the
    // vertical through the bottom-right one. PlaneGCS, diagnosing on the drawing, dropped
    // that vertical (whose residual is the corner's x offset, 4.5e-8 mm) and kept the
    // perpendicular, whose residual is a sine scaled by the sides' drawn lengths: DogLeg
    // stops once it's below 1e-10, and 3.1e-11 of it is 45 nm at the end of a 1,985 mm side.
    // Decision 5 measures the dropped vertical as an angle, 2.3e-11 rad, and keeps it. But
    // the solution wasn't its own: diagnosed again from it, PlaneGCS dropped the bottom
    // side's horizontal instead, and the corner moved 45 nm on every load (M3.4). Now a
    // solve that dropped constraints is solved as a load solves it, from its solution as
    // the file holds it, until a load doesn't move it (Translation::solve), and the corner
    // lands exactly.
    let mut w = worker();
    let r = redundant_rectangle_case(
        0.3794470496640547,
        3.2977112173238883,
        0.0,
        [
            -0.28134042662659664,
            -0.06908254986966719,
            -0.2873223268194091,
            0.0,
            0.16938440505469204,
            -0.221262413456824,
        ],
        false,
        &mut w,
    );
    assert_eq!(r, Ok(()));
}

#[test]
fn a_flat_redundant_rectangle_far_out_that_needs_two_solves_from_its_solution_is_kept() {
    // Found while fixing the case above, by 500,000 cases of the property below: a
    // 59,924.087627 × 0.008231 rectangle, drawn roughly 786 m out, was refused as
    // over_constrained, its left side's vertical "disagreeing by 8.1e-5 rad". PlaneGCS kept
    // an equal length that fixes the corner's x only through a square root, leaving it
    // 0.67 nm off on an 8 µm side. Solved again from that solution, PlaneGCS called eight
    // constraints conflicting but moved the corner exactly; solved from there, the sketch
    // solves and doesn't move. One solve again wasn't enough; now it's loaded until a load
    // doesn't move it.
    let mut w = worker();
    let r = redundant_rectangle_case(
        4.777601430413569,
        -2.0845302759679507,
        786_269.243_248_198_8,
        [
            -0.06600214974689472,
            -0.04202863432615801,
            0.26719447145621483,
            -0.09911831107945834,
            -0.0685890538671934,
            0.20701873743368476,
        ],
        true,
        &mut w,
    );
    assert_eq!(r, Ok(()));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, .. ProptestConfig::default() })]

    // Found by break-it, fixed: a 31,012.404909 × 1 rectangle drawn roughly (its short sides
    // leaning, a corner 1,229 mm from where it lands; the regression file's case) was
    // over_constrained,
    // "constraints repeat others but disagree with them, by up to 0e0 mm and 1.22e-5 rad",
    // naming k0000006 (the left side's vertical). They agree: PlaneGCS dropped the one
    // constraint that fixes the corner's x well and kept an equal length that fixes it only
    // through a square root (|x| = sqrt(2 h · residual)), so the corner lands 12 nm off and
    // decision 5 blames the constraint it dropped. Removing it, as the advice says, would
    // leave the same 12 nm silently. Now a sketch called over-constrained that the solver
    // solved anyway is diagnosed and solved again from its solution, where PlaneGCS drops a
    // different constraint and the corner lands within 1e-15 (Translation::solve). The
    // same mechanism, through a kept angle, and through a solve again that wasn't yet its
    // own solution: the two tests above.
    #[test]
    fn redundant_rectangles_that_agree_are_kept_at_every_scale_and_place(
        // Log-uniform sizes from a micrometre to 100 m, places out to a kilometre, biased
        // toward the edges: the origin and 999,000 mm.
        log_w in -3.0f64..5.0,
        log_h in -3.0f64..5.0,
        place in prop_oneof![Just(0.0), Just(999_000.0), -999_000.0f64..999_000.0],
        jitter in proptest::array::uniform6(-0.3f64..0.3),
        rough in any::<bool>(),
    ) {
        let (w, h) = (nm(libm::pow(10.0, log_w)), nm(libm::pow(10.0, log_h)));
        prop_assume!(w > 0.0 && h > 0.0 && (log_w - log_h).abs() <= 8.0);
        thread_local!(static W: std::cell::RefCell<SketchWorker> = std::cell::RefCell::new(worker()));
        let got = W.with(|w| {
            redundant_rectangle_case(log_w, log_h, place, jitter, rough, &mut *w.borrow_mut())
        });
        prop_assert!(got.is_ok(), "{}", got.unwrap_err());
    }
}

// --- Determinism, and a worker reused after failures ---------------------------------------

#[test]
fn the_anchored_sqp_fallback_gives_the_same_bits_in_two_workers() {
    // A small impossible chain: everything fails, including anchored SQP; and a sketch the
    // first three fail but anchored SQP solves is hard to make on purpose, so check the
    // failing path's error and a following solve, twice, in two workers.
    let run = || {
        let mut w = worker();
        let e = solved(&checked(&impossible_chain(8, 50.0)), &mut w).unwrap_err();
        let s = solved(&checked(&half_circle_symmetric_about_y(209.0)), &mut w);
        (e.to_string(), format!("{s:?}"))
    };
    assert_eq!(run(), run());
}

/// A 10 × 10 square, drawn true, with the redundant rectangle's constraints dimensioned
/// w × h: an agent's template, redimensioned.
fn redimensioned_square(w: f64, h: f64) -> String {
    let rect = redundant_rectangle(0.0, 0.0, 10.0, 10.0, [0.0; 6], false);
    rect.replace(
        r#""line": "n0000001", "value": 10,"#,
        &format!(r#""line": "n0000001", "value": {w},"#),
    )
    .replace(
        r#""line": "n0000002", "value": 10,"#,
        &format!(r#""line": "n0000002", "value": {h},"#),
    )
}

#[test]
fn a_square_template_redimensioned_keeps_its_agreeing_redundancies() {
    // Drawn as a true 10 × 10 square, every redundancy met where it's drawn; then given
    // lengths from 1 µm to 100 m. The solution is the w × h rectangle (analytic), and its
    // redundancies (parallel, perpendicular, equal) agree with it exactly.
    let mut w = worker();
    let mut broke = Vec::new();
    for (a, b) in [
        (1000.0, 0.1),
        (0.1, 1000.0),
        (10000.0, 1.0),
        (100000.0, 1.0),
        (1.0, 100000.0),
        (0.001, 10.0),
        (10.0, 0.001),
        (100000.0, 0.01),
    ] {
        let doc = checked(&redimensioned_square(a, b));
        match solved(&doc, &mut w) {
            Ok(s) => {
                let p3 = s.point(&id("p0000003")).unwrap();
                assert!(
                    // Either way up: lengths have no direction.
                    (p3[0].abs() - a).abs() <= CLOSE && (p3[1].abs() - b).abs() <= CLOSE,
                    "{a} × {b}: {p3:?}"
                );
            }
            Err(e) => broke.push(format!("{a} × {b}: {e}")),
        }
    }
    // Found by break-it, fixed: 100,000 × 0.01 was over_constrained, "these 6 can't all
    // hold" (k0000003, 4, 5, 9, 10, 11), degrees of freedom -3, diagnosed on the 10 × 10
    // drawing. FreeCAD 1.1.3, the same sketch with its coincidences
    // (adversarial_sketch_oracle.rs), finds degrees of freedom 0 and one redundant
    // constraint, k0000003, which decision 5 keeps. Now, diagnosed again from the solution
    // the solver reached anyway, it's that (Translation::solve).
    assert!(broke.is_empty(), "{broke:#?}");
}

#[test]
fn arcs_drawn_anywhere_with_ends_symmetric_about_a_line_through_their_centre_are_mirrored() {
    // The same arc, its end drawn from 100° to 330° (its start at 30°): wherever it's drawn,
    // an Ok solve must leave its ends mirrored about the y axis (a.x = -b.x, a.y = b.y).
    let mut w = worker();
    let mut unmet = Vec::new();
    for end in (100..=330).step_by(10) {
        let doc = checked(&half_circle_symmetric_about_y(end as f64));
        if let Ok(s) = solved(&doc, &mut w) {
            let (a, b) = (
                s.point(&id("pa000000")).unwrap(),
                s.point(&id("pb000000")).unwrap(),
            );
            if (a[0] + b[0]).abs() > CLOSE || (a[1] - b[1]).abs() > CLOSE {
                unmet.push(format!("drawn to {end}°: a {a:?}, b {b:?}"));
            }
        }
    }
    // Found by break-it, fixed: every arc drawn to 180°–320° (a sweep of 150° to 290°)
    // solved to a half circle about its chord's midpoint, its ends unmirrored, and said Ok;
    // only arcs drawn under a half turn came out symmetric. FreeCAD 1.1.3 gives the same
    // points (adversarial_sketch_oracle.rs, drawn to 210° and 270°); Silico now refuses it,
    // naming the symmetry.
    assert!(unmet.is_empty(), "{unmet:#?}");
}

// --- Unsigned distances started exactly on their line ----------------------------------------

/// Line n from (0, 0) to (10, 0), held; point p drawn at (5, `y`), held 3 to the `side`.
fn point_from_line(y: f64, side: &str) -> String {
    text(
        &[
            point("pa000000", 0.0, 0.0),
            point("pb000000", 10.0, 0.0),
            point("pp000000", 5.0, y),
        ],
        &[line("na000000", "pa000000", "pb000000")],
        &[],
        &[],
        &[
            c(
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "pa000000", "value": 0, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "pa000000", "value": 0, "driving": true}"#,
            ),
            c(r#"{"id": "k0000003", "type": "horizontal", "line": "na000000"}"#),
            c(
                r#"{"id": "k0000004", "type": "length", "line": "na000000", "value": 10, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000005", "type": "distance_x", "a": "origin", "b": "pp000000", "value": 5, "driving": true}"#,
            ),
            format!(
                r#"{{"id": "k0000006", "type": "point_line_distance", "point": "pp000000", "line": "na000000", "value": 3, "side": "{side}", "driving": true}}"#
            ),
        ],
    )
}

#[test]
fn a_point_drawn_on_its_line_held_off_it_to_a_side_lands_on_that_side_or_says_which() {
    // Drawn exactly on the line (y = 0, and -0): the unsigned distance's gradient there has
    // no side. The answer is p at (5, 3) for left, (5, -3) for right (analytic), or a
    // refusal naming k0000006 with a suggestion; never an unnamed failure.
    let mut w = worker();
    for y in [0.0, -0.0, 1e-300, -1e-300] {
        for (side, want) in [("left", 3.0), ("right", -3.0)] {
            match solved(&checked(&point_from_line(y, side)), &mut w) {
                Ok(s) => {
                    let p = s.point(&id("pp000000")).unwrap();
                    assert!(
                        (p[0] - 5.0).abs() <= CLOSE && (p[1] - want).abs() <= CLOSE,
                        "{y} {side}: {p:?}"
                    );
                }
                Err(e) => {
                    assert!(
                        e.elements().iter().any(|x| x == "k0000006"),
                        "{y} {side}: {e}"
                    );
                    assert!(e.suggestion().is_some(), "{e}");
                }
            }
        }
    }
}
