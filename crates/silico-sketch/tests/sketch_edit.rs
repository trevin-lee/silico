//! The sketch edits' results through the sketch worker: a merge that makes an edge tangency
//! a `tangent_at` (docs/design/m5.0-app.md, open item 13) writes the `direction` that keeps
//! the tangent's side. Where the merged points touch, PlaneGCS solves the merged sketch
//! without moving it, and the other direction moves it; merged from apart, the solve holds
//! the side, and the other direction would flip it. Expected values by hand: a line along
//! y = 0 into (10, 0), an arc about (10, 10). And an add the gate takes, whose solve then
//! finds the conflict it makes, which refuses the edit (docs/ports/sketch-edit.md, §2).

use super::fixture::{document, fields, line, point};

use std::path::Path;

use silico_core::ErrorCode;
use silico_core::document::sketch_edit::{Members, add, merge};
use silico_core::document::{Direction, ElementId, Form, Operation, Sketch, read};
use silico_core::sketch::{SketchWorker, Translation};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// The solver meets each constraint to 1e-10; points land within this of where they must.
const CLOSE: f64 = 1e-8;

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

/// Line a0 from (0, 0) to p1 (10, 0), and arc r0 about q0 (10, 10) from `start` to `end`, p9
/// at (10, 4e-8) one of them, edge tangent to a0, `left`; merged at p1.
fn merged(start: &str, end: &str) -> Sketch {
    merged_at(start, end, &["p0000001", "p0000009"])
}

/// The sketch of [`merged`], `points` merged.
fn merged_at(start: &str, end: &str, points: &[&str]) -> Sketch {
    let text = document(&fields(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000009", 10.0, 0.00000004),
            point("q0000000", 10.0, 10.0),
            point("p0000007", 0.0, 10.0),
            point("p0000008", 20.0, 10.0),
        ]
        .join(", "),
        &line("a0000000", "p0000000", "p0000001"),
        "",
        &format!(
            r#"{{"id": "r0000000", "center": "q0000000", "start": "{start}", "end": "{end}", "construction": false}}"#
        ),
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "r0000000", "side": "left"}"#,
    ));
    let before = read(text.as_bytes()).unwrap();
    let Operation::Sketch(sketch) = &before.features[0].operation else {
        panic!("not a sketch")
    };
    let points: Vec<ElementId> = points.iter().map(|p| id(p)).collect();
    merge(sketch, &points).unwrap().sketch
}

/// `sketch` solved, each point's move from where it's stored, at most; none if it doesn't
/// solve.
fn moved(sketch: &Sketch, worker: &mut SketchWorker) -> Option<f64> {
    let solution = Translation::new(sketch)
        .unwrap()
        .solve(worker)
        .unwrap()
        .outcome
        .ok()?;
    Some(
        sketch
            .points
            .iter()
            .map(|p| {
                let [x, y] = solution.point(&p.id).unwrap();
                libm::hypot(x - p.x.mm(), y - p.y.mm())
            })
            .fold(0.0, f64::max),
    )
}

/// `sketch` with k0's direction the other way.
fn flipped(sketch: &Sketch) -> Sketch {
    let mut sketch = sketch.clone();
    for c in &mut sketch.constraints {
        if let Form::TangentAt { direction, .. } = &mut c.form {
            *direction = match direction {
                Direction::Same => Direction::Opposite,
                Direction::Opposite => Direction::Same,
            };
        }
    }
    sketch
}

#[test]
fn a_merges_tangent_at_holds_the_drawing_as_it_is_and_the_other_direction_doesnt() {
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    // An arc leaving p1 (smooth, `same`) and one arriving at it (a cusp, `opposite`).
    for (start, end, direction) in [
        ("p0000009", "p0000008", Direction::Same),
        ("p0000007", "p0000009", Direction::Opposite),
    ] {
        let sketch = merged(start, end);
        let written = sketch.constraints.iter().find_map(|c| match &c.form {
            Form::TangentAt { direction, .. } => Some(*direction),
            _ => None,
        });
        assert_eq!(written, Some(direction), "{start} to {end}");
        let as_written = moved(&sketch, &mut worker);
        assert!(
            as_written.is_some_and(|d| d <= CLOSE),
            "{start} to {end}: moved {as_written:?}"
        );
        // The other direction can't hold as drawn: the solve turns a curve about, or fails.
        let other = moved(&flipped(&sketch), &mut worker);
        assert!(
            other.is_none_or(|d| d > 1e-3),
            "{start} to {end}: the flipped one moved {other:?}"
        );
    }
}

/// Where `sketch` solves its points a0's ends p0 and p7 and r0's centre q0 and start p7:
/// q0's side of a0, facing from p0 to p7 (the cross product's sign, + on its left), and how
/// far a0's line misses being tangent to r0's circle (q0's distance from it against the
/// radius); none if it doesn't solve.
fn side_held(sketch: &Sketch, worker: &mut SketchWorker) -> Option<(f64, f64)> {
    let solution = Translation::new(sketch)
        .unwrap()
        .solve(worker)
        .unwrap()
        .outcome
        .ok()?;
    let at = |p: &str| solution.point(&id(p)).unwrap();
    let (p0, p7, q0) = (at("p0000000"), at("p0000007"), at("q0000000"));
    let along = [p7[0] - p0[0], p7[1] - p0[1]];
    let length = libm::hypot(along[0], along[1]);
    let cross = along[0] * (q0[1] - p0[1]) - along[1] * (q0[0] - p0[0]);
    let radius = libm::hypot(q0[0] - p7[0], q0[1] - p7[1]);
    Some((cross.signum(), (cross.abs() / length - radius).abs()))
}

#[test]
fn a_merge_from_apart_solves_with_the_arc_still_on_the_lines_left() {
    // Robustness's line case: r0 about q0 (10, 10) from p7 (0, 10) at 180° to p9 at 270°,
    // k0 `left` (q0 on a0's left, facing +x). Merged at p7, a0 runs from (0, 0) up to (0, 10)
    // and r0 starts there: drawn tangent, but with q0 on a0's right (x = 10, facing +y). The
    // side is an angle of 0 between their own directions at the contact, `same` where p7
    // ends a0 and starts r0. Solved, q0 must lie on a0's left, the line tangent to the circle.
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let sketch = merged_at("p0000007", "p0000009", &["p0000007", "p0000001"]);
    let written = sketch.constraints.iter().find_map(|c| match &c.form {
        Form::TangentAt { direction, .. } => Some(*direction),
        _ => None,
    });
    assert_eq!(written, Some(Direction::Same));
    let held = side_held(&sketch, &mut worker);
    assert!(
        held.is_some_and(|(side, miss)| side > 0.0 && miss <= CLOSE),
        "kept: {held:?}"
    );
    // The direction the drawing shows, `opposite`, solves with q0 on the right (as drawn),
    // or doesn't solve: the check above can tell the two apart.
    let other = side_held(&flipped(&sketch), &mut worker);
    assert!(
        other.is_none_or(|(side, _)| side < 0.0),
        "flipped: {other:?}"
    );
}

#[test]
fn an_added_conflicting_length_is_found_by_the_solve_naming_it() {
    // Line na from (0, 0) to (10, 0), its length 10; a second length of 12 added. FreeCAD
    // 1.1.3 keeps such a constraint and its solve reports the conflict (status -3, the port
    // note's probe); Silico's add takes it, the gate having nothing against it, and the
    // edit's solve refuses it, naming it.
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let sketch_in = |constraints: &str| {
        let text = document(&fields(
            &[point("pa000000", 0.0, 0.0), point("pb000000", 10.0, 0.0)].join(", "),
            &line("na000000", "pa000000", "pb000000"),
            "",
            "",
            constraints,
        ));
        let read = read(text.as_bytes()).unwrap();
        let Operation::Sketch(sketch) = &read.features[0].operation else {
            panic!("not a sketch")
        };
        sketch.clone()
    };
    let before = sketch_in(
        r#"{"id": "ka000001", "type": "length", "line": "na000000", "value": 10, "driving": true}"#,
    );
    let batch = Members {
        constraints: sketch_in(
            r#"{"id": "kz000001", "type": "length", "line": "na000000", "value": 12, "driving": true}"#,
        )
        .constraints,
        ..Members::default()
    };
    let solve = |sketch: &Sketch, worker: &mut SketchWorker| {
        Translation::new(sketch)
            .unwrap()
            .solve(worker)
            .unwrap()
            .outcome
    };
    // Before the add, it solves.
    assert!(solve(&before, &mut worker).is_ok());
    let edited = add(&before, &batch).unwrap();
    let error = solve(&edited.sketch, &mut worker).unwrap_err();
    assert_eq!(error.code(), ErrorCode::OverConstrained, "{error}");
    assert!(
        error.elements().iter().any(|e| e == "kz000001"),
        "{:?}",
        error.elements()
    );
}
