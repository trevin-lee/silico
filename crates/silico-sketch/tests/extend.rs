//! `silico_core::extend` through the sketch worker (docs/ports/sketch-edit.md, §7): a
//! line's end dragged once from the sketch as stored, as FreeCAD's `extend` drags it, its
//! constraints winning. Expected values from FreeCAD 1.1.3 (the note's probe) and by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture::{document, fields, line, point};

use std::path::Path;

use silico_core::document::{ElementId, Length, Operation, Sketch, read, validate};
use silico_core::extend::{By, End, extend};
use silico_core::sketch::{SketchWorker, Translation};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// The solver meets each constraint to 1e-10; points land within this of where they must.
const CLOSE: f64 = 1e-8;

fn at(sketch: &Sketch, name: &str) -> [f64; 2] {
    let p = sketch
        .points
        .iter()
        .find(|p| p.id.as_str() == name)
        .unwrap();
    [p.x.mm(), p.y.mm()]
}

fn close(got: [f64; 2], want: [f64; 2]) {
    assert!(
        (got[0] - want[0]).abs() <= CLOSE && (got[1] - want[1]).abs() <= CLOSE,
        "{got:?} against {want:?}"
    );
}

#[test]
fn a_horizontal_line_with_its_length_slides_when_extended_as_freecads_drag_moves_it() {
    // FreeCAD 1.1.3 (docs/ports/sketch-edit.md, §7): a 10 mm horizontal line with its length,
    // from (0, 0) to (10, 0), extended at its end by 5: the drag pulls the end to (15, 0),
    // the length holds, so the line slides to (5, 0)–(15, 0). Writing the end there and
    // solving gives (3.5706, 0)–(13.5706, 0) instead, which the drag isn't.
    let text = document(&fields(
        &[point("p0000000", 0.0, 0.0), point("p0000001", 10.0, 0.0)].join(", "),
        &line("a0000000", "p0000000", "p0000001"),
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}, {"id": "k0000001", "type": "length", "line": "a0000000", "value": 10, "driving": true}"#,
    ));
    let before = read(text.as_bytes()).unwrap();
    let Operation::Sketch(sketch) = &before.features[0].operation else {
        panic!("not a sketch")
    };
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let edited = extend(
        sketch,
        &ElementId::new("a0000000").unwrap(),
        End::End,
        By::Length(Length::rounded(5.0).unwrap()),
        &mut worker,
    )
    .unwrap();
    close(at(&edited.sketch, "p0000000"), [5.0, 0.0]);
    close(at(&edited.sketch, "p0000001"), [15.0, 0.0]);
    let mut ids: Vec<&str> = edited.modified.iter().map(ElementId::as_str).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["p0000000", "p0000001"]);
    // The gate takes it, and the edit's solve leaves it where the drag did.
    let mut after = before.clone();
    after.features[0].operation = Operation::Sketch(edited.sketch.clone());
    validate(&after).unwrap();
    let solution = Translation::new(&edited.sketch)
        .unwrap()
        .solve(&mut worker)
        .unwrap()
        .outcome
        .unwrap();
    close(
        solution
            .point(&ElementId::new("p0000000").unwrap())
            .unwrap(),
        [5.0, 0.0],
    );
    close(
        solution
            .point(&ElementId::new("p0000001").unwrap())
            .unwrap(),
        [15.0, 0.0],
    );
}
