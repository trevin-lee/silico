//! M5.3: a document's sketch point dragged through the sketch worker (silico_core's
//! `drag::sketch`), by its ID: the design's example (docs/design/m5.0-app.md, `drag_begin`,
//! `drag_move`, `drag_end`), and an arc dragged by the point it ends at. Where each point
//! lands is worked out by hand, each where it's asserted.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use silico_core::document::{Checked, ElementId, FeatureId, Operation, Sketch, read};
use silico_core::drag::Kept;
use silico_core::drag::sketch::{DragStop, EdgeGrab, FrameKind, Opened, begin, begin_edge};
use silico_core::recompute::SketchSolvers;
use silico_core::sketch::{SketchSolver, SketchWorker, solve};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// The solver stops when its step is under 1e-10: a point lands within this of where it
/// should.
const CLOSE: f64 = 1e-9;

/// The bracket's L profile (M4.1's example) with its base leg's thickness, `ka00000a`,
/// removed: its one freedom is that thickness, the height of pa000003 and pa000004.
const PROFILE: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 60, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}, {"id": "pa000005", "x": 5, "y": 50, "construction": false}, {"id": "pa000006", "x": 0, "y": 50, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "horizontal", "line": "na000005"}, {"id": "ka000006", "type": "vertical", "line": "na000006"}, {"id": "ka000007", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000008", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, {"id": "ka00000b", "type": "length", "line": "na000005", "value": 5, "driving": true}, {"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}]}"#;

/// An arc about pc000001 (1, 2), counter-clockwise from pc000002 (6, 2) to pc000003
/// (1, 7), its radius held at 5.
const ARC: &str = r#"{"id": "s0000003", "type": "sketch", "v": 1, "name": "Arc", "plane": "xy", "points": [{"id": "pc000001", "x": 1, "y": 2, "construction": false}, {"id": "pc000002", "x": 6, "y": 2, "construction": false}, {"id": "pc000003", "x": 1, "y": 7, "construction": false}], "lines": [], "circles": [], "arcs": [{"id": "ac000001", "center": "pc000001", "start": "pc000002", "end": "pc000003", "construction": false}], "constraints": [{"id": "kc000001", "type": "radius", "curve": "ac000001", "value": 5, "driving": true}]}"#;

fn document(feature: &str) -> Checked {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{feature}]}}"#
    );
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

fn solvers() -> SketchSolvers {
    SketchSolvers::starting(|| {
        Ok(Box::new(SketchWorker::start_worker(Path::new(WORKER))?) as Box<dyn SketchSolver>)
    })
}

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn assert_near(got: [f64; 2], want: [f64; 2], what: &str) {
    let off = libm::hypot(got[0] - want[0], got[1] - want[1]);
    assert!(
        off <= CLOSE,
        "{what}: got {got:?}, want {want:?} ({off:e} off)"
    );
}

fn point_of(sketch: &Sketch, p: &str) -> [f64; 2] {
    let p = sketch.points.iter().find(|q| q.id.as_str() == p).unwrap();
    [p.x.mm(), p.y.mm()]
}

/// `document` with its one feature's sketch replaced by `sketch`.
fn with_sketch(document: &Checked, sketch: Sketch) -> Checked {
    let mut edited = document.document().clone();
    edited.features[0].operation = Operation::Sketch(sketch);
    Checked::new(edited).unwrap()
}

#[test]
fn the_designs_drag_moves_the_bracket_legs_top_and_ends_on_a_sketch_its_file_gives_back() {
    // The cursor off to the right of the leg's end, at (62.5, 7): pa000003 stays on its
    // vertical line at x = 60 (the base's length from the corner at the origin), and goes
    // as near the cursor as that lets it, to y = 7; the leg's top, horizontal, takes
    // pa000004, held at x = 5 by the upright's width, with it. Every other point stays.
    let doc = document(PROFILE);
    let sketch = FeatureId::new("s0000001").unwrap();
    let solvers = solvers();
    let Opened {
        drag,
        dofs,
        moves,
        unsolved,
    } = begin(&solvers, None, &doc, &sketch, &id("pa000003"), None).unwrap();
    assert_eq!(unsolved, None);
    assert_eq!(dofs, 1);
    let profile = [
        ("pa000001", [0.0, 0.0]),
        ("pa000002", [60.0, 0.0]),
        ("pa000003", [60.0, 7.0]),
        ("pa000004", [5.0, 7.0]),
        ("pa000005", [5.0, 50.0]),
        ("pa000006", [0.0, 50.0]),
    ];
    assert_eq!(
        moves,
        profile.iter().map(|(p, _)| id(p)).collect::<Vec<_>>()
    );
    let frame = drag.move_to([62.5, 7.0]).unwrap().wait().unwrap();
    assert_eq!(frame.kind, FrameKind::Solved);
    assert!(frame.radii.is_empty());
    assert_eq!(frame.points.len(), profile.len());
    for ((got, at), (want, place)) in frame.points.iter().zip(profile) {
        assert_eq!(got.as_str(), want);
        assert_near(*at, place, want);
    }
    let ended = drag.end(Some([62.5, 7.0])).unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    assert_eq!(ended.why, None);
    for (p, place) in profile {
        assert_near(point_of(&ended.sketch, p), place, p);
    }
    // Settled: solved again from its file, the sketch is written back as it is, bit for
    // bit, so the next rebuild finds no drift.
    let edited = with_sketch(&doc, ended.sketch.clone());
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let again = solve(&edited, &sketch, &mut worker).unwrap();
    assert_eq!(again.written(&ended.sketch), ended.sketch);
}

#[test]
fn an_arc_dragged_by_the_point_it_ends_at_moves_its_end_before_its_centre() {
    // The core drags pc000003, the sketch's point, not the arc's end: its centre is still
    // pinned where it began, a hundred times more weakly (FreeCAD 4d7a9435c1). With the
    // radius held at 5, a cursor d from the centre along u moves the centre δ along u,
    // where (d - 5 - δ)² + 1e-4 δ² is least: δ = (d - 5) / (1 + 1e-4). Unpinned, the centre
    // would follow by d - 5, and the end reach the cursor: 1e-3 mm apart at d = 15. Only
    // the 1e-4-weighted pin holds the centre, so the solver stops with it, and the end on
    // its circle, less close than a pinned point: within 1e-7 (9.4e-9 seen), still far
    // finer than that.
    let doc = document(ARC);
    let solvers = solvers();
    let Opened { drag, moves, .. } = begin(
        &solvers,
        None,
        &doc,
        &FeatureId::new("s0000003").unwrap(),
        &id("pc000003"),
        None,
    )
    .unwrap();
    assert_eq!(moves, [id("pc000001"), id("pc000002"), id("pc000003")]);
    for cursor in [[1.0, 17.0], [-11.0, -7.0], [4.0, 6.0]] {
        let frame = drag.move_to(cursor).unwrap().wait().unwrap();
        assert_eq!(frame.kind, FrameKind::Solved, "{cursor:?}");
        // The arc's radius is the part's to solve, held at 5 by its constraint.
        assert_eq!(frame.radii.len(), 1);
        assert_eq!(frame.radii[0].0, id("ac000001"));
        assert!((frame.radii[0].1 - 5.0).abs() <= CLOSE, "{:?}", frame.radii);
        let (dx, dy) = (cursor[0] - 1.0, cursor[1] - 2.0);
        let d = libm::hypot(dx, dy);
        let u = [dx / d, dy / d];
        let delta = (d - 5.0) / (1.0 + 1e-4);
        let c = [1.0 + delta * u[0], 2.0 + delta * u[1]];
        for (what, got, want) in [
            ("centre", frame.points[0].1, c),
            (
                "end",
                frame.points[2].1,
                [c[0] + 5.0 * u[0], c[1] + 5.0 * u[1]],
            ),
        ] {
            let off = libm::hypot(got[0] - want[0], got[1] - want[1]);
            assert!(
                off <= 1e-7,
                "{what}, cursor {cursor:?}: got {got:?}, want {want:?} ({off:e} off)"
            );
        }
    }
    drop(drag);
}

// --- Dragging an edge (docs/ports/sketcher.md, *Dragging an edge*) ------------------------

/// A free line from pe000001 (0,0) to pe000002 (10,0), and a circle about pe000003 (20,0) of
/// radius 5: nothing held.
const FREE: &str = r#"{"id": "s0000005", "type": "sketch", "v": 1, "name": "Free", "plane": "xy", "points": [{"id": "pe000001", "x": 0, "y": 0, "construction": false}, {"id": "pe000002", "x": 10, "y": 0, "construction": false}, {"id": "pe000003", "x": 20, "y": 0, "construction": false}], "lines": [{"id": "ne000001", "from": "pe000001", "to": "pe000002", "construction": false}], "circles": [{"id": "ce000001", "center": "pe000003", "radius": 5, "construction": false}], "arcs": [], "constraints": []}"#;

fn edge(doc: &Checked, curve: &str, press: [f64; 2]) -> Result<Opened, silico_core::Error> {
    begin_edge(
        &solvers(),
        None,
        doc,
        &FeatureId::new("s0000005").unwrap(),
        EdgeGrab {
            curve: &id(curve),
            press,
        },
        None,
        &DragStop::new(),
    )
}

#[test]
fn a_lines_edge_pressed_anywhere_moves_whole_by_the_cursors_offset_from_the_press() {
    // Pressed at (2,0), a fifth of the way along; the cursor at (5,4), the offset (3,4):
    // (3,4)-(13,4), as 1.1.3's relative drag. The frame and the end agree; the circle, in a
    // part of its own, stays.
    let doc = document(FREE);
    let Opened { drag, moves, .. } = edge(&doc, "ne000001", [2.0, 0.0]).unwrap();
    assert_eq!(moves, [id("pe000001"), id("pe000002")]);
    let frame = drag.move_to([5.0, 4.0]).unwrap().wait().unwrap();
    assert_eq!(frame.kind, FrameKind::Solved);
    assert_eq!(frame.to, [5.0, 4.0]);
    assert_near(frame.points[0].1, [3.0, 4.0], "start");
    assert_near(frame.points[1].1, [13.0, 4.0], "end");
    let ended = drag.end(Some([5.0, 4.0])).unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    assert_near(point_of(&ended.sketch, "pe000001"), [3.0, 4.0], "start");
    assert_near(point_of(&ended.sketch, "pe000002"), [13.0, 4.0], "end");
    assert_near(
        point_of(&ended.sketch, "pe000003"),
        [20.0, 0.0],
        "the circle's centre",
    );
}

#[test]
fn a_circles_edge_dragged_is_sized_through_the_cursor_and_saved_so() {
    // Its edge pressed at its top (20,5), released at (26,8): radius 10, the centre staying,
    // as 1.1.3's; the frame reads the radius free, the file writes it.
    let doc = document(FREE);
    let Opened { drag, .. } = edge(&doc, "ce000001", [20.0, 5.0]).unwrap();
    let frame = drag.move_to([26.0, 8.0]).unwrap().wait().unwrap();
    assert_eq!(frame.kind, FrameKind::Solved);
    assert_eq!(frame.radii.len(), 1);
    assert!(
        (frame.radii[0].1 - 10.0).abs() <= CLOSE,
        "{:?}",
        frame.radii
    );
    let ended = drag.end(Some([26.0, 8.0])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor);
    let radius = ended.sketch.circles[0].radius.mm();
    assert!((radius - 10.0).abs() <= CLOSE, "radius {radius}");
    assert_near(point_of(&ended.sketch, "pe000003"), [20.0, 0.0], "centre");
}

#[test]
fn an_edges_release_at_its_press_is_a_click_and_a_curve_the_sketch_hasnt_is_refused() {
    let doc = document(FREE);
    let Opened { drag, .. } = edge(&doc, "ne000001", [2.0, 0.0]).unwrap();
    drag.move_to([5.0, 4.0]).unwrap().wait().unwrap();
    let ended = drag.end(Some([2.0, 0.0])).unwrap();
    assert_eq!(
        (ended.kept, ended.changed, &ended.why),
        (Kept::Begin, false, &None)
    );
    let Err(e) = edge(&doc, "ne000009", [0.0, 0.0]) else {
        panic!("began")
    };
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (
            silico_core::ErrorCode::InvalidInput,
            Some("curve"),
            Some("s0000005")
        )
    );
    // A point's ID isn't a curve's.
    let Err(e) = edge(&doc, "pe000001", [0.0, 0.0]) else {
        panic!("began")
    };
    assert_eq!(e.parameter(), Some("curve"));
}
