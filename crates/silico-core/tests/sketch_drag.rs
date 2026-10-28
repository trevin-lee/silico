//! `silico_core::drag::sketch` on the fake sketch solver (docs/design/m5.0-app.md, the drag
//! commands, decision 9): a document's sketch point dragged by its ID, its begin state the
//! model's solve or the drag's own, its frames read back by ID, and its end the sketch as
//! settled. Expected values are by hand: the fake solves a sketch where it's sent, and
//! answers a move from the begin state alone, the dragged point at the cursor nudged by a
//! thousandth of where it began.

use std::sync::{Arc, Mutex};

use silico_core::document::{Checked, ElementId, FeatureId, Operation, Sketch, read};
use silico_core::drag::Kept;
use silico_core::drag::sketch::{Frame, FrameKind, Opened, begin};
use silico_core::fake::{DragCall, FakeSketches};
use silico_core::recompute::SketchSolvers;
use silico_core::sketch::{SketchSolver, solve};
use silico_core::{Error, ErrorCode};
use silico_protocol::{Moved, PointOf, SolveStatus, Tag};

/// The bracket's L profile (M4.1's example) with its base leg's thickness, `ka00000a`,
/// removed: one freedom, that thickness. Its points: pa000001 (0, 0), pa000002 (60, 0),
/// pa000003 (60, 5), pa000004 (5, 5), pa000005 (5, 50), pa000006 (0, 50).
const PROFILE: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 60, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}, {"id": "pa000005", "x": 5, "y": 50, "construction": false}, {"id": "pa000006", "x": 0, "y": 50, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "horizontal", "line": "na000005"}, {"id": "ka000006", "type": "vertical", "line": "na000006"}, {"id": "ka000007", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000008", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, {"id": "ka00000b", "type": "length", "line": "na000005", "value": 5, "driving": true}, {"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}]}"#;

/// Two holes' circles about pb000001 (20, 10) and pb000002 (36, 10), radius 2.75, the
/// first's radius held; and pb000003 (38.75, 10) held on the second, so the second's
/// centre and radius are in pb000003's part.
const HOLES: &str = r#"{"id": "s0000002", "type": "sketch", "v": 1, "name": "Holes", "plane": "xy", "points": [{"id": "pb000001", "x": 20, "y": 10, "construction": false}, {"id": "pb000002", "x": 36, "y": 10, "construction": false}, {"id": "pb000003", "x": 38.75, "y": 10, "construction": false}], "lines": [], "circles": [{"id": "cb000001", "center": "pb000001", "radius": 2.75, "construction": false}, {"id": "cb000002", "center": "pb000002", "radius": 2.75, "construction": false}], "arcs": [], "constraints": [{"id": "kb000001", "type": "radius", "curve": "cb000001", "value": 2.75, "driving": true}, {"id": "kb000002", "type": "on", "point": "pb000003", "curve": "cb000002"}]}"#;

const PAD: &str = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Legs", "sketch": "s0000001", "extent": {"type": "one_side", "length": 40, "reversed": true}}"#;

fn document() -> Checked {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{PROFILE}, {PAD}, {HOLES}]}}"#
    );
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn feature(s: &str) -> FeatureId {
    FeatureId::new(s).unwrap()
}

fn sketch_of(document: &Checked, f: &str) -> Sketch {
    match &document.feature(&feature(f)).unwrap().operation {
        Operation::Sketch(sketch) => sketch.clone(),
        _ => panic!("{f} isn't a sketch"),
    }
}

/// Solvers that start fakes, and every fake started, in order.
fn pool() -> (SketchSolvers, Arc<Mutex<Vec<FakeSketches>>>) {
    let started: Arc<Mutex<Vec<FakeSketches>>> = Arc::default();
    let list = Arc::clone(&started);
    let solvers = SketchSolvers::starting(move || {
        let fake = FakeSketches::new();
        list.lock().unwrap().push(fake.clone());
        Ok(Box::new(fake) as Box<dyn SketchSolver>)
    });
    (solvers, started)
}

fn opened(solvers: &SketchSolvers, f: &str, point: &str) -> Opened {
    begin(solvers, None, &document(), &feature(f), &id(point), None).unwrap()
}

fn refused(f: &str, point: &str) -> Error {
    let (solvers, _) = pool();
    match begin(&solvers, None, &document(), &feature(f), &id(point), None) {
        Ok(_) => panic!("{f}'s {point} began a drag"),
        Err(e) => e,
    }
}

/// The profile's points as they began, by ID, but `moved` at its place.
fn profile_with(moved: (&str, [f64; 2])) -> Vec<(ElementId, [f64; 2])> {
    [
        ("pa000001", [0.0, 0.0]),
        ("pa000002", [60.0, 0.0]),
        ("pa000003", [60.0, 5.0]),
        ("pa000004", [5.0, 5.0]),
        ("pa000005", [5.0, 50.0]),
        ("pa000006", [0.0, 50.0]),
    ]
    .into_iter()
    .map(|(p, at)| (id(p), if p == moved.0 { moved.1 } else { at }))
    .collect()
}

#[test]
fn a_drag_of_a_sketch_point_moves_it_by_its_id_and_reads_every_point_of_its_part() {
    let (solvers, started) = pool();
    let Opened { drag, dofs, moves } = opened(&solvers, "s0000001", "pa000003");
    // The fake's degrees of freedom; every point of the profile is in the one part.
    assert_eq!(dofs, 1);
    let all: Vec<ElementId> = profile_with(("", [0.0; 2]))
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert_eq!(moves, all);
    // pa000003 is the sketch's point by its own geometry: the origin and two axes first
    // (0, 1, 2), then the points by ID, pa000003 the third (5).
    let fake = started.lock().unwrap()[0].clone();
    let Some(DragCall::Begin(sent)) = fake.drag_calls().first().cloned() else {
        panic!("{:?}", fake.drag_calls())
    };
    assert_eq!(sent.point, PointOf::Point(silico_protocol::Geometry(5)));
    // (62.5, 7), nudged by a thousandth of (60, 5).
    let frame = drag.move_to([62.5, 7.0]).unwrap().wait().unwrap();
    let want = [62.5 + 60.0 / 1000.0, 7.0 + 5.0 / 1000.0];
    assert_eq!(
        frame,
        Frame {
            to: [62.5, 7.0],
            kind: FrameKind::Solved,
            points: profile_with(("pa000003", want)),
            radii: vec![],
        }
    );
    // The end at the same cursor takes that frame's values; the fake solves them where
    // they're sent, and the load of the file they make gives it back, so the sketch is
    // the file's with pa000003 moved, and nothing else changed.
    let ended = drag.end(Some([62.5, 7.0])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor);
    assert!(ended.changed);
    assert_eq!(ended.why, None);
    let mut expected = sketch_of(&document(), "s0000001");
    for p in &mut expected.points {
        if p.id.as_str() == "pa000003" {
            (p.x, p.y) = (
                silico_core::document::Coordinate::new(want[0]).unwrap(),
                silico_core::document::Coordinate::new(want[1]).unwrap(),
            );
        }
    }
    assert_eq!(ended.sketch, expected);
}

#[test]
fn an_end_with_no_cursor_keeps_the_sketch_as_its_file_holds_it() {
    let (solvers, _) = pool();
    let Opened { drag, .. } = opened(&solvers, "s0000001", "pa000003");
    drag.move_to([62.5, 7.0]).unwrap().wait().unwrap();
    let ended = drag.end(None).unwrap();
    assert_eq!(ended.kept, Kept::Begin);
    assert!(!ended.changed);
    assert_eq!(ended.why, None);
    assert_eq!(ended.sketch, sketch_of(&document(), "s0000001"));
}

#[test]
fn a_held_solve_is_the_begin_state_and_none_is_solved_again() {
    // The model's solve, its pa000003 at (60, 9): the drag begins there, and the fake is
    // sent no solve before it.
    let doc = document();
    let mut held = solve(&doc, &feature("s0000001"), &mut FakeSketches::new()).unwrap();
    held.points.insert(
        id("pa000003"),
        [
            silico_core::document::Coordinate::new(60.0).unwrap(),
            silico_core::document::Coordinate::new(9.0).unwrap(),
        ],
    );
    let (solvers, started) = pool();
    let Opened { drag, .. } = begin(
        &solvers,
        None,
        &doc,
        &feature("s0000001"),
        &id("pa000003"),
        Some(&held),
    )
    .unwrap();
    let fake = started.lock().unwrap()[0].clone();
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
    // From (60, 9): the cursor nudged by a thousandth of it.
    let frame = drag.move_to([61.0, 8.0]).unwrap().wait().unwrap();
    assert_eq!(
        frame.points[2],
        (id("pa000003"), [61.0 + 60.0 / 1000.0, 8.0 + 9.0 / 1000.0])
    );
}

#[test]
fn with_no_held_solve_the_drag_solves_the_sketch_once_on_its_own_solver() {
    let (solvers, started) = pool();
    let _opened = opened(&solvers, "s0000001", "pa000003");
    let all = started.lock().unwrap().clone();
    assert_eq!(all.len(), 1, "one solver, the drag's");
    // The sketch solved as its file draws it: the fake solves it there, so its load needs
    // no second solve.
    let requests = all[0].requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    let Some(DragCall::Begin(sent)) = all[0].drag_calls().first().cloned() else {
        panic!("no begin")
    };
    assert_eq!(sent.system.values, requests[0].values);
}

#[test]
fn a_sketch_that_fails_to_solve_begins_where_its_file_draws_it() {
    // Open item 15: a solve that fails for the sketch (not the solver) leaves the drag to
    // begin from the file's coordinates.
    let fake = FakeSketches::new();
    fake.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.status = SolveStatus::Failed;
        solved.algorithm = None;
        // Where the failed solve left its unknowns, which the drag mustn't begin from.
        for v in &system.unknowns {
            solved.values[v.0 as usize] += 1.0;
        }
        Ok(solved)
    });
    let solvers = SketchSolvers::starting({
        let fake = fake.clone();
        move || Ok(Box::new(fake.clone()) as Box<dyn SketchSolver>)
    });
    let Opened { drag, .. } = opened(&solvers, "s0000001", "pa000003");
    let Some(DragCall::Begin(sent)) = fake.drag_calls().first().cloned() else {
        panic!("no begin")
    };
    // The file's (60, 5) where pa000003's values are, 10 and 11: after the origin's and
    // the axes' six, and pa000001's and pa000002's four.
    assert_eq!(&sent.system.values[10..12], &[60.0, 5.0]);
    drop(drag);
}

#[test]
fn a_solver_that_fails_the_begins_solve_refuses_the_drag_with_its_error() {
    let fake = FakeSketches::new();
    fake.answer(|_| {
        Err(Error::new(
            ErrorCode::WorkerCrashed,
            "the fake sketch solver crashed",
        ))
    });
    let solvers = SketchSolvers::starting({
        let fake = fake.clone();
        move || Ok(Box::new(fake.clone()) as Box<dyn SketchSolver>)
    });
    let Err(e) = begin(
        &solvers,
        None,
        &document(),
        &feature("s0000001"),
        &id("pa000003"),
        None,
    ) else {
        panic!("began")
    };
    assert_eq!(e.code(), ErrorCode::WorkerCrashed);
    assert_eq!(e.feature(), Some("s0000001"));
    assert!(fake.drag_calls().is_empty());
}

#[test]
fn constraints_in_conflict_refuse_the_drag_naming_them_by_id() {
    // Tags are the constraints' places, sorted by ID, from 1: ka000001 to ka000009 are 1
    // to 9, then ka00000b 10 and ka00000c 11 (ka00000a was removed).
    let fake = FakeSketches::new();
    fake.conflicting_drags(vec![Tag(9), Tag(11)]);
    let solvers = SketchSolvers::starting({
        let fake = fake.clone();
        move || Ok(Box::new(fake.clone()) as Box<dyn SketchSolver>)
    });
    let Err(e) = begin(
        &solvers,
        None,
        &document(),
        &feature("s0000001"),
        &id("pa000003"),
        None,
    ) else {
        panic!("began")
    };
    assert_eq!(e.code(), ErrorCode::OverConstrained);
    assert_eq!(
        e.message(),
        "the sketch's constraints conflict: these 2 can't all hold"
    );
    assert_eq!(e.elements(), ["ka000009", "ka00000c"]);
    assert_eq!(e.feature(), Some("s0000001"));
}

#[test]
fn a_point_the_sketch_hasnt_or_a_feature_that_isnt_a_sketch_is_refused_naming_it() {
    let e = refused("s0000001", "pb000001");
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (ErrorCode::InvalidInput, Some("point"), Some("s0000001"))
    );
    let e = refused("p0000001", "pa000003");
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (ErrorCode::InvalidInput, Some("sketch"), Some("p0000001"))
    );
    let e = refused("s0000009", "pa000003");
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("sketch"))
    );
}

#[test]
fn a_frame_reads_the_free_radii_of_its_part_and_only_its_part() {
    // pb000003, held on cb000002, joins that circle's centre and radius to its part;
    // pb000001 and its circle, whose radius is held, are another part.
    let (solvers, _) = pool();
    let Opened { drag, moves, .. } = opened(&solvers, "s0000002", "pb000003");
    assert_eq!(moves, [id("pb000002"), id("pb000003")]);
    // The fake moves the dragged point alone: (40, 12) nudged by a thousandth of
    // (38.75, 10); the centre and radius as they began.
    let frame = drag.move_to([40.0, 12.0]).unwrap().wait().unwrap();
    assert_eq!(
        frame,
        Frame {
            to: [40.0, 12.0],
            kind: FrameKind::Solved,
            points: vec![
                (id("pb000002"), [36.0, 10.0]),
                (
                    id("pb000003"),
                    [40.0 + 38.75 / 1000.0, 12.0 + 10.0 / 1000.0]
                ),
            ],
            radii: vec![(id("cb000002"), 2.75)],
        }
    );
}

#[test]
fn a_move_that_isnt_solved_answers_its_kind_with_no_values() {
    let (solvers, started) = pool();
    let Opened { drag, .. } = opened(&solvers, "s0000001", "pa000003");
    let fake = started.lock().unwrap()[0].clone();
    fake.answer_moves(|_, to| {
        Ok(Moved {
            status: if to[0] > 100.0 {
                SolveStatus::SolvedInvalid
            } else {
                SolveStatus::Failed
            },
            values: None,
        })
    });
    for (to, kind) in [
        ([70.0, 7.0], FrameKind::Failed),
        ([200.0, 7.0], FrameKind::Invalid),
    ] {
        let frame = drag.move_to(to).unwrap().wait().unwrap();
        assert_eq!(
            frame,
            Frame {
                to,
                kind,
                points: vec![],
                radii: vec![],
            }
        );
    }
    // A move the worker refuses is a failed frame, and the drag goes on.
    fake.answer_moves(|_, _| Err(Error::new(ErrorCode::InvalidInput, "out of reach")));
    let frame = drag.move_to([70.0, 7.0]).unwrap().wait().unwrap();
    assert_eq!(frame.kind, FrameKind::Failed);
    assert!(drag.is_open());
    // An end whose final move the worker refuses keeps the begin state, and says why: the
    // worker's refusal, naming the sketch.
    let ended = drag.end(Some([70.0, 7.0])).unwrap();
    assert_eq!(ended.kept, Kept::Begin);
    assert!(!ended.changed);
    assert_eq!(ended.sketch, sketch_of(&document(), "s0000001"));
    let why = ended.why.unwrap();
    assert_eq!(
        (why.code(), why.message(), why.feature()),
        (ErrorCode::InvalidInput, "out of reach", Some("s0000001"))
    );
}
