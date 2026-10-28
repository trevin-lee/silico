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

#[test]
fn an_arcs_extended_end_takes_a_free_neighbour_with_it_as_freecads_arc_first_solve_does() {
    // FreeCAD 1.1.3 (the port review's probe9): an arc about (0, 0), radius 10, from 0° to
    // 90°, and a line from its end to (−10, 10); the end on by 30°, then solved. With the
    // arc listed first, its end lands at (10 cos 120°, 10 sin 120°) = (−5, 5√3), the radius
    // stays 10, and the free line follows, its far end where it was. Made horizontal, the
    // line's far end goes to (−10, 5√3). (Listed after the line, FreeCAD undoes most of the
    // turn: Silico writes the point, so it's the arc-first result whatever the order.)
    let h = 5.0 * 3f64.sqrt();
    for (constraint, far) in [
        ("", [-10.0, 10.0]),
        (
            r#"{"id": "k0000000", "type": "horizontal", "line": "b0000000"}"#,
            [-10.0, h],
        ),
    ] {
        let text = document(&fields(
            &[
                point("z0000000", 0.0, 0.0),
                point("s0000000", 10.0, 0.0),
                point("e0000000", 0.0, 10.0),
                point("q0000000", -10.0, 10.0),
            ]
            .join(", "),
            &line("b0000000", "e0000000", "q0000000"),
            "",
            &super::fixture::arc("r0000000", "z0000000", "s0000000", "e0000000"),
            constraint,
        ));
        let before = read(text.as_bytes()).unwrap();
        let Operation::Sketch(sketch) = &before.features[0].operation else {
            panic!("not a sketch")
        };
        let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
        let edited = extend(
            sketch,
            &ElementId::new("r0000000").unwrap(),
            End::End,
            By::Angle(silico_core::document::Angle::rounded(30.0).unwrap()),
            &mut worker,
        )
        .unwrap();
        let solution = Translation::new(&edited.sketch)
            .unwrap()
            .solve(&mut worker)
            .unwrap()
            .outcome
            .unwrap();
        let p = |name: &str| solution.point(&ElementId::new(name).unwrap()).unwrap();
        close(p("e0000000"), [-5.0, h]);
        close(p("z0000000"), [0.0, 0.0]);
        close(p("s0000000"), [10.0, 0.0]);
        close(p("q0000000"), far);
    }
}

#[test]
fn a_line_in_a_part_past_a_drags_unknowns_extends_as_one_solve_does() {
    // 140 lines end to end along x, p0 at 0 to p140 at 1,400, each held 10 mm long: one
    // part of 141 free points, 282 unknowns, past the 256 a drag takes. An extend isn't a
    // drag: the last line's end on by 5 reaches (1405, 0), the chain sliding along, and
    // every line is 10 mm after the edit's solve.
    let n = 140;
    let points: Vec<String> = (0..=n)
        .map(|i| point(&format!("p{i:07}"), 10.0 * i as f64, 0.0))
        .collect();
    let lines: Vec<String> = (0..n)
        .map(|i| {
            line(
                &format!("a{i:07}"),
                &format!("p{i:07}"),
                &format!("p{:07}", i + 1),
            )
        })
        .collect();
    let lengths: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id": "k{i:07}", "type": "length", "line": "a{i:07}", "value": 10, "driving": true}}"#
            )
        })
        .collect();
    let text = document(&fields(
        &points.join(", "),
        &lines.join(", "),
        "",
        "",
        &lengths.join(", "),
    ));
    let before = read(text.as_bytes()).unwrap();
    let Operation::Sketch(sketch) = &before.features[0].operation else {
        panic!("not a sketch")
    };
    assert_eq!(Translation::new(sketch).unwrap().system.unknowns.len(), 282);
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let last = format!("a{:07}", n - 1);
    let edited = extend(
        sketch,
        &ElementId::new(&last).unwrap(),
        End::End,
        By::Length(Length::rounded(5.0).unwrap()),
        &mut worker,
    )
    .unwrap();
    let end = format!("p{n:07}");
    close(at(&edited.sketch, &end), [1405.0, 0.0]);
    let solution = Translation::new(&edited.sketch)
        .unwrap()
        .solve(&mut worker)
        .unwrap()
        .outcome
        .unwrap();
    for i in 0..n {
        let p = |j: usize| {
            solution
                .point(&ElementId::new(&format!("p{j:07}")).unwrap())
                .unwrap()
        };
        let (a, b) = (p(i), p(i + 1));
        let length = libm::hypot(b[0] - a[0], b[1] - a[1]);
        assert!((length - 10.0).abs() <= CLOSE, "line {i}: {length}");
    }
}
