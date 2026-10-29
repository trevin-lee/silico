//! The sketch edits' results through the sketch worker: a merge that makes an edge tangency
//! a `tangent_at` (docs/design/m5.0-app.md, open item 13) writes the `direction` the drawing
//! shows, so PlaneGCS solves the merged sketch without moving it; the other direction moves
//! it. Expected values by hand: a line along y = 0 into (10, 0), an arc about (10, 10).

use super::fixture::{document, fields, line, point};

use std::path::Path;

use silico_core::document::sketch_edit::merge;
use silico_core::document::{Direction, ElementId, Form, Operation, Sketch, read};
use silico_core::sketch::{SketchWorker, Translation};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// The solver meets each constraint to 1e-10; points land within this of where they must.
const CLOSE: f64 = 1e-8;

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

/// Line a0 from (0, 0) to p1 (10, 0), and arc r0 about q0 (10, 10) from `start` to `end`, p9
/// at (10, 4e-8) one of them, edge tangent to a0; merged at p1.
fn merged(start: &str, end: &str) -> Sketch {
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
    merge(sketch, &[id("p0000001"), id("p0000009")])
        .unwrap()
        .sketch
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
