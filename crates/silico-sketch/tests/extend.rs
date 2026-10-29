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
    // Both ends moved, by ID.
    let ids: Vec<&str> = edited.modified.iter().map(ElementId::as_str).collect();
    assert_eq!(ids, ["p0000000", "p0000001"]);
    // The gate takes it, and the edit's solve leaves it where the drag did.
    let mut after = before.clone();
    after.features[0].operation = Operation::Sketch(edited.sketch.clone());
    validate(&after).unwrap();
    let solution = Translation::new(&edited.sketch)
        .unwrap()
        .solve(&mut worker)
        .unwrap()
        .into_outcome()
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
            .into_outcome()
            .unwrap();
        let p = |name: &str| solution.point(&ElementId::new(name).unwrap()).unwrap();
        close(p("e0000000"), [-5.0, h]);
        close(p("z0000000"), [0.0, 0.0]);
        close(p("s0000000"), [10.0, 0.0]);
        close(p("q0000000"), far);
    }
}

#[test]
fn a_lines_end_an_arc_starts_at_pins_only_that_end_as_freecads_line_extend_does() {
    // FreeCAD 1.1.3 (probed for #262's review): a horizontal line from the origin, where
    // distances hold its start, to (10, 0), where an arc about (10, 10) starts and runs a
    // quarter turn to (20, 10); the line's end on by 5. FreeCAD's `extend` drags the
    // line's end (`moveGeometry` on the line, so `initMove` pins that end alone): the end
    // reaches (15, 0) and the arc's centre goes where the solve takes it, (10.039692,
    // 9.216921), its end to (20.480038, 9.962213). Pinning the centre too, as a drag of the
    // arc's start does, would hold it near (10, 10).
    let text = document(&fields(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("z0000000", 10.0, 10.0),
            point("e0000000", 20.0, 10.0),
        ]
        .join(", "),
        &line("a0000000", "p0000000", "p0000001"),
        "",
        &super::fixture::arc("r0000000", "z0000000", "p0000001", "e0000000"),
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}, {"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000000", "value": 0, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000000", "value": 0, "driving": true}"#,
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
    // FreeCAD prints six places, so these hold to its last one.
    let near = |got: [f64; 2], want: [f64; 2]| {
        assert!(
            (got[0] - want[0]).abs() <= 1e-6 && (got[1] - want[1]).abs() <= 1e-6,
            "{got:?} against {want:?}"
        );
    };
    near(at(&edited.sketch, "p0000000"), [0.0, 0.0]);
    near(at(&edited.sketch, "p0000001"), [15.0, 0.0]);
    near(at(&edited.sketch, "z0000000"), [10.039692, 9.216921]);
    near(at(&edited.sketch, "e0000000"), [20.480038, 9.962213]);
}

#[test]
fn a_line_extended_to_the_next_curve_ends_on_it() {
    // A free line from (1, 2) to (4, 3) and a circle about (10, 7) of radius 4. Its line's
    // foot from the centre is (10.6, 5.2), √3.6 mm off, so it meets the circle √12.4 mm
    // either side: first at (7.2593413823019868, 4.0864471274339956), by hand to 40 places.
    // The end goes there through the worker's one move, so it touches the circle as the
    // gate and trim count it, which no whole-nanometre increment could promise.
    let text = document(&fields(
        &[
            point("p0000000", 1.0, 2.0),
            point("p0000001", 4.0, 3.0),
            point("z0000000", 10.0, 7.0),
        ]
        .join(", "),
        &line("a0000000", "p0000000", "p0000001"),
        &super::fixture::circle("c0000000", "z0000000", 4.0),
        "",
        "",
    ));
    let before = read(text.as_bytes()).unwrap();
    let Operation::Sketch(sketch) = &before.features[0].operation else {
        panic!("not a sketch")
    };
    let a = ElementId::new("a0000000").unwrap();
    let met = silico_core::extend::next_crossing(sketch, &a, End::End)
        .unwrap()
        .unwrap();
    assert_eq!(met.curve.as_str(), "c0000000");
    let want = [7.259_341_382_301_987, 4.086_447_127_433_996];
    let crossing = [met.at[0].mm(), met.at[1].mm()];
    assert!(
        (crossing[0] - want[0]).abs() <= 1e-12 && (crossing[1] - want[1]).abs() <= 1e-12,
        "{crossing:?}"
    );
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let edited = extend(sketch, &a, End::End, By::To(met.at), &mut worker).unwrap();
    let end = at(&edited.sketch, "p0000001");
    close(end, crossing);
    let off = (libm::hypot(end[0] - 10.0, end[1] - 7.0) - 4.0).abs();
    assert!(off <= 1e-7, "{off} mm off the circle");
    // The start stays: the line is free, and its one move takes only the end.
    close(at(&edited.sketch, "p0000000"), [1.0, 2.0]);
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
        .into_outcome()
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
