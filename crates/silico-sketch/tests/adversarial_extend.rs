//! Hostile inputs for `silico_core::extend` through the sketch worker, from break-it: a
//! line's end its constraints hold, a drag onto a neighbour's far end, a drag that grows a
//! circle or re-measures a reference dimension, and the worker after each refusal.
//! Expected values by hand: where PlaneGCS must put each point, or the gate's verdict.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture::{circle, document, fields, line, point};

use std::path::Path;

use silico_core::ErrorCode;
use silico_core::document::{Document, ElementId, Form, Length, Operation, Sketch, read, validate};
use silico_core::extend::{By, End, extend};
use silico_core::sketch::SketchWorker;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// The solver meets each constraint to 1e-10; points land within this of where they must.
const CLOSE: f64 = 1e-8;

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn doc(points: &[String], lines: &[String], circles: &str, constraints: &str) -> Document {
    read(
        document(&fields(
            &points.join(", "),
            &lines.join(", "),
            circles,
            "",
            constraints,
        ))
        .as_bytes(),
    )
    .unwrap()
}

fn sketch_of(document: &Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

fn gate(document: &Document, sketch: &Sketch) -> Vec<(String, Vec<String>)> {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(sketch.clone());
    match validate(&document) {
        Ok(()) => Vec::new(),
        Err(invalid) => invalid
            .errors()
            .iter()
            .map(|e| (e.message().to_owned(), e.elements().to_vec()))
            .collect(),
    }
}

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

fn mm(v: f64) -> By {
    By::Length(Length::rounded(v).unwrap())
}

fn worker() -> SketchWorker {
    SketchWorker::start_worker(Path::new(WORKER)).unwrap()
}

/// A free line a0 (0, 0)–(10, 0): the edit that shows the worker still works.
fn free_line() -> Document {
    doc(
        &[point("p0000000", 0.0, 0.0), point("p0000001", 10.0, 0.0)],
        &[line("a0000000", "p0000000", "p0000001")],
        "",
        "",
    )
}

#[track_caller]
fn still_works(worker: &mut SketchWorker) {
    let before = free_line();
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        worker,
    )
    .unwrap();
    close(at(&edited.sketch, "p0000001"), [15.0, 0.0]);
    close(at(&edited.sketch, "p0000000"), [0.0, 0.0]);
}

#[test]
fn extend_a_line_whose_end_its_constraints_hold_is_refused_and_the_worker_goes_on() {
    // p1 held at (10, 0) by its x and y from the origin: the drag can't move it. Refused
    // as the module says, naming `by`, the line and its end; the same worker then extends a
    // free line to (15, 0).
    let before = doc(
        &[point("p0000000", 0.0, 0.0), point("p0000001", 10.0, 0.0)],
        &[line("a0000000", "p0000000", "p0000001")],
        "",
        r#"{"id": "k0000000", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 10, "driving": true}, {"id": "k0000001", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
    );
    assert!(gate(&before, sketch_of(&before)).is_empty());
    let mut worker = worker();
    let got = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut worker,
    );
    // The worker first, so its state is checked whatever the edit said.
    still_works(&mut worker);
    // BREAKS: Ok, p1 at (10, 0) where it was, `modified` empty: the drag's frame is a
    // Success that moved nothing, so the edit says it extended a line it left as it was.
    // (FreeCAD's extend returns its drag's success here too; the module's notes promise the
    // refusal.)
    match got {
        Ok(edited) => panic!(
            "Ok: p1 at {:?}, modified {:?}",
            at(&edited.sketch, "p0000001"),
            edited.modified
        ),
        Err(e) => {
            assert_eq!(e.code(), ErrorCode::InvalidResult, "{e}");
            assert_eq!(e.parameter(), Some("by"), "{e}");
            assert_eq!(e.elements(), ["a0000000", "p0000001"], "{e}");
        }
    }
}

#[test]
fn extend_a_line_onto_its_neighbours_far_end_names_the_neighbour() {
    // Lines a0 (0, 0)–(10, 0) and b0 (10, 0)–(12, 0) sharing p1, nothing else: a0 on by 2
    // drags p1 onto p2, so b0 would have no length. Either the edit refuses naming b0, or
    // its sketch is one the gate refuses naming b0; either way the worker goes on.
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000002", 12.0, 0.0),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "p0000001", "p0000002"),
        ],
        "",
        "",
    );
    let mut worker = worker();
    let got = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(2.0),
        &mut worker,
    );
    still_works(&mut worker);
    match got {
        Ok(edited) => {
            let refusals = gate(&before, &edited.sketch);
            assert!(
                refusals
                    .iter()
                    .any(|(_, elements)| elements.iter().any(|e| e == "b0000000")),
                "{refusals:?}"
            );
        }
        Err(e) => {
            // BREAKS: the worker withholds the frame as `SolvedInvalid`,
            // and the edit says "the drag's solve ended SolvedInvalid", naming a0 and p1,
            // with "remove or change the constraints that hold the line's end": there are
            // none; b0, which would collapse, goes unnamed.
            assert!(
                e.elements().iter().any(|e| e == "b0000000"),
                "{e}: suggestion {:?}",
                e.suggestion()
            );
        }
    }
}

#[test]
fn extend_a_line_whose_end_is_on_a_free_circle_lists_the_grown_circle() {
    // a0's end p1 on circle c0 (about z0, held at (5, 0) by its x and y; radius 5, free):
    // the drag to (15, 0) takes p1 there and the circle with it, radius 10, as PlaneGCS's
    // least move does (the circle's centre is held). FreeCAD's extend moves its circle too.
    // BREAKS: `modified` names the points moved alone, [p1]: c0, whose radius the edit
    // changed from 5 to 10, isn't listed.
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("z0000000", 5.0, 0.0),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &circle("c0000000", "z0000000", 5.0),
        r#"{"id": "k0000000", "type": "on", "point": "p0000001", "curve": "c0000000"}, {"id": "k0000001", "type": "distance_x", "a": "origin", "b": "z0000000", "value": 5, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "z0000000", "value": 0, "driving": true}"#,
    );
    assert!(gate(&before, sketch_of(&before)).is_empty());
    let mut worker = worker();
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut worker,
    )
    .unwrap();
    close(at(&edited.sketch, "p0000001"), [15.0, 0.0]);
    let radius = edited.sketch.circles[0].radius.mm();
    assert!((radius - 10.0).abs() <= CLOSE, "{radius}");
    // By kind, as `Edited` lists them: the point, then the circle.
    assert_eq!(edited.modified, [id("p0000001"), id("c0000000")]);
}

#[test]
fn extend_a_line_with_a_reference_length_lists_the_dimension_re_measured() {
    // a0 with a reference (not driving) length of 10, on by 5 at its end: the drag takes
    // p1 to (15, 0), and the edited sketch's dimension reads 15, as a save writes it.
    // BREAKS: `modified` is [p1]: k0, whose value the edit changed in place, isn't listed.
    let before = doc(
        &[point("p0000000", 0.0, 0.0), point("p0000001", 10.0, 0.0)],
        &[line("a0000000", "p0000000", "p0000001")],
        "",
        r#"{"id": "k0000000", "type": "length", "line": "a0000000", "value": 10, "driving": false}"#,
    );
    let mut worker = worker();
    let edited = extend(
        sketch_of(&before),
        &id("a0000000"),
        End::End,
        mm(5.0),
        &mut worker,
    )
    .unwrap();
    close(at(&edited.sketch, "p0000001"), [15.0, 0.0]);
    let value = match &edited.sketch.constraints[0].form {
        Form::Length { value, .. } => *value,
        other => panic!("{other:?}"),
    };
    if value != Length::rounded(10.0).unwrap() {
        assert_eq!(
            edited.modified,
            // By kind, as `Edited` lists them: the point, then the constraint.
            [id("p0000001"), id("k0000000")],
            "k0 now {value}"
        );
    }
}

#[test]
fn extend_through_fresh_workers_is_bit_for_bit_the_same() {
    // A rectangle with horizontals, verticals and a driving width: its bottom's end on by 3.
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000002", 10.0, 5.0),
            point("p0000003", 0.0, 5.0),
        ],
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("a0000001", "p0000001", "p0000002"),
            line("a0000002", "p0000002", "p0000003"),
            line("a0000003", "p0000003", "p0000000"),
        ],
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}, {"id": "k0000001", "type": "vertical", "line": "a0000001"}, {"id": "k0000002", "type": "horizontal", "line": "a0000002"}, {"id": "k0000003", "type": "vertical", "line": "a0000003"}"#,
    );
    let run = || {
        let mut worker = worker();
        extend(
            sketch_of(&before),
            &id("a0000000"),
            End::End,
            mm(3.0),
            &mut worker,
        )
        .unwrap()
    };
    let (one, two) = (run(), run());
    assert_eq!(one, two);
    assert!(gate(&before, &one.sketch).is_empty());
    // The bottom's end goes to (13, 0), the right side with it (its vertical), the top's end
    // follows by its horizontal: a 13 × 5 rectangle, p0 and p3 where they were.
    close(at(&one.sketch, "p0000001"), [13.0, 0.0]);
    close(at(&one.sketch, "p0000002"), [13.0, 5.0]);
    close(at(&one.sketch, "p0000000"), [0.0, 0.0]);
    close(at(&one.sketch, "p0000003"), [0.0, 5.0]);
}
