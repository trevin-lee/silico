//! `silico_core::drag::sketch` on the fake sketch solver (docs/design/m5.0-app.md, the drag
//! commands, decision 9): a document's sketch point dragged by its ID, its begin state the
//! model's solve or the drag's own, its frames read back by ID, and its end the sketch as
//! settled. Expected values are by hand: the fake solves a sketch where it's sent, and
//! answers a move from the begin state alone, the dragged point at the cursor nudged by a
//! thousandth of where it began.

use std::sync::{Arc, Mutex};

use silico_core::document::{Checked, ElementId, FeatureId, Operation, Sketch, read};
use silico_core::drag::Kept;
use silico_core::drag::sketch::{
    DragStop, EdgeGrab, Frame, FrameKind, Held, Opened, begin, begin_edge, begin_stoppable,
};
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
    let Opened {
        drag,
        dofs,
        moves,
        unsolved,
    } = opened(&solvers, "s0000001", "pa000003");
    assert_eq!(unsolved, None);
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
    assert_eq!(
        sent.dragged,
        PointOf::Point(silico_protocol::Geometry(5)).into()
    );
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
            why: None,
        }
    );
    // The end at the same cursor takes that frame's values; the fake solves them where
    // they're sent, and the load of the file they make gives it back, so the sketch is
    // the file's with pa000003 moved, and nothing else changed.
    let ended = drag.end(Some([62.5, 7.0])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor);
    assert!(ended.changed);
    // Solves: the begin's, then the end's one whole solve and the load that settles it,
    // and none more (the drag's own release solves nothing whole).
    assert_eq!(fake.requests().len(), 3, "{:?}", fake.requests());
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
        Some(Held {
            sketch: &sketch_of(&doc, "s0000001"),
            solution: &held,
        }),
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
fn a_suppressed_sketch_is_refused_naming_sketch_a_point_and_an_edge_alike() {
    // M5.0's `drag_begin`: "A suppressed sketch is `invalid_input`, naming `sketch`." It's
    // never solved, so there's no begin state; refused before any solver is taken.
    let holes = HOLES.replacen(
        r#""name": "Holes", "#,
        r#""name": "Holes", "suppressed": true, "#,
        1,
    );
    assert_ne!(holes, HOLES);
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{PROFILE}, {PAD}, {holes}]}}"#
    );
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    let (solvers, started) = pool();
    let point = begin(
        &solvers,
        None,
        &document,
        &feature("s0000002"),
        &id("pb000001"),
        None,
    );
    let edge = begin_edge(
        &solvers,
        None,
        &document,
        &feature("s0000002"),
        EdgeGrab {
            curve: &id("cb000001"),
            press: [22.75, 10.0],
        },
        None,
        &DragStop::new(),
    );
    for (grab, begun) in [("point", point), ("edge", edge)] {
        let Err(e) = begun else {
            panic!("a suppressed sketch's {grab} began a drag")
        };
        assert_eq!(
            (
                e.code(),
                e.parameter(),
                e.feature(),
                e.message(),
                e.suggestion()
            ),
            (
                ErrorCode::InvalidInput,
                Some("sketch"),
                Some("s0000002"),
                "s0000002 is suppressed, so it isn't solved",
                Some(
                    "unsuppress s0000002 first (suppress_feature with suppressed false), or \
                     name a sketch that isn't suppressed"
                )
            ),
            "{grab}"
        );
    }
    assert!(started.lock().unwrap().is_empty());
    // The same sketch unsuppressed begins a drag of the same point.
    opened(&solvers, "s0000002", "pb000001");
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
            why: None,
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
                why: None,
            }
        );
    }
    // A move the worker refuses is a failed frame, and the drag goes on.
    fake.answer_moves(|_, _| Err(Error::new(ErrorCode::InvalidInput, "out of reach")));
    let frame = drag.move_to([70.0, 7.0]).unwrap().wait().unwrap();
    assert_eq!(frame.kind, FrameKind::Failed);
    let why = frame.why.unwrap();
    assert_eq!(
        (why.code(), why.message()),
        (ErrorCode::InvalidInput, "out of reach")
    );
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

/// Solvers that are all `fake`.
fn on(fake: &FakeSketches) -> SketchSolvers {
    let fake = fake.clone();
    SketchSolvers::starting(move || Ok(Box::new(fake.clone()) as Box<dyn SketchSolver>))
}

#[test]
fn a_stop_refuses_the_begins_whole_solve_and_the_ends_as_cancelled() {
    // The begin's solve held in the fake, and stopped: the begin is refused, `cancelled`,
    // naming the sketch.
    let fake = FakeSketches::new();
    let solvers = on(&fake);
    let stop = DragStop::new();
    fake.hold();
    let begun = {
        let (solvers, stop) = (solvers.clone(), stop.clone());
        on_a_thread(move || {
            begin_stoppable(
                &solvers,
                None,
                &document(),
                &feature("s0000001"),
                &id("pa000003"),
                None,
                &stop,
            )
            .err()
        })
    };
    assert!(fake.wait_held(std::time::Duration::from_secs(10)));
    stop.stop();
    let e = begun
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the stop didn't stop the begin")
        .expect("the begin went on");
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::Cancelled, Some("s0000001"))
    );
    fake.release();
    // The end's solve, held after a move to the final cursor (so the end needs no move of
    // its own), and stopped through the drag's own stop: the end is refused, `cancelled`.
    let Opened { drag, .. } = opened(&solvers, "s0000001", "pa000003");
    drag.move_to([62.5, 7.0]).unwrap().wait().unwrap();
    let stop = drag.stop();
    fake.hold();
    let ended = on_a_thread(move || drag.end(Some([62.5, 7.0])).err());
    assert!(fake.wait_held(std::time::Duration::from_secs(10)));
    stop.stop();
    let e = ended
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the stop didn't stop the end")
        .expect("the end went on");
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::Cancelled, Some("s0000001"))
    );
    fake.release();
}

#[test]
fn a_sketch_that_didnt_solve_when_the_drag_began_says_so_when_it_keeps_the_begin_state() {
    // Found by the port review: every solve fails, so the drag begins where the file draws
    // it, and its release, solved as a whole, fails alike. Why is the begin's own failure,
    // not advice to release nearer, which no cursor could follow.
    let fake = FakeSketches::new();
    fake.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.status = SolveStatus::Failed;
        solved.algorithm = None;
        Ok(solved)
    });
    let solvers = on(&fake);
    let Opened { drag, unsolved, .. } = opened(&solvers, "s0000001", "pa000003");
    let unsolved = unsolved.expect("the begin solved");
    assert_eq!(unsolved.feature(), Some("s0000001"));
    let ended = drag.end(Some([62.5, 7.0])).unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Begin, false));
    assert_eq!(ended.why, Some(unsolved));
}

#[test]
fn a_settle_the_worker_refuses_keeps_the_begin_state_saying_why() {
    // Found by the robustness review: the begin's solve answers; the end's, the worker
    // refuses (as it refuses values its own check won't take): the begin kept, the
    // refusal its why, naming the sketch, never the end's error.
    use std::sync::atomic::{AtomicUsize, Ordering};
    let fake = FakeSketches::new();
    let solves = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&solves);
    fake.answer(move |system| {
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(FakeSketches::as_sent(system))
        } else {
            Err(Error::new(ErrorCode::InvalidInput, "refused by its check"))
        }
    });
    let solvers = on(&fake);
    let Opened { drag, .. } = opened(&solvers, "s0000001", "pa000003");
    let ended = drag.end(Some([62.5, 7.0])).unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Begin, false));
    let why = ended.why.unwrap();
    assert_eq!(
        (why.code(), why.message(), why.feature()),
        (
            ErrorCode::InvalidInput,
            "refused by its check",
            Some("s0000001")
        )
    );
}

#[test]
fn a_stop_before_the_begin_returns_refuses_the_begin_rather_than_leaving_the_drag_open() {
    // A stop given before the begin (from a held solve, so nothing is solved to stop):
    // the begin is refused, `cancelled`, not opened for every release to be cancelled.
    let doc = document();
    let held = solve(&doc, &feature("s0000001"), &mut FakeSketches::new()).unwrap();
    let (solvers, _) = pool();
    let stop = DragStop::new();
    stop.stop();
    let Err(e) = begin_stoppable(
        &solvers,
        None,
        &doc,
        &feature("s0000001"),
        &id("pa000003"),
        Some(Held {
            sketch: &sketch_of(&doc, "s0000001"),
            solution: &held,
        }),
        &stop,
    ) else {
        panic!("began")
    };
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::Cancelled, Some("s0000001"))
    );
}

#[test]
fn the_begin_state_is_the_sketchs_solve_not_where_its_file_draws_it() {
    // Found by the tests review: every fake solved where it was sent, so the solve and the
    // drawing were one. Here the solve raises the leg's top to y = 6 (pa000003's and
    // pa000004's y, values 11 and 13): the drag begins there, not at the drawn 5.
    let fake = FakeSketches::new();
    fake.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.values[11] = 6.0;
        solved.values[13] = 6.0;
        Ok(solved)
    });
    let solvers = on(&fake);
    let Opened { drag, .. } = opened(&solvers, "s0000001", "pa000003");
    let Some(DragCall::Begin(sent)) = fake.drag_calls().first().cloned() else {
        panic!("no begin")
    };
    assert_eq!(&sent.system.values[10..14], &[60.0, 6.0, 5.0, 6.0]);
    drop(drag);
}

#[test]
fn a_replaced_move_is_a_dropped_frame_and_a_polled_ticket_answers_its_frame() {
    // From the tests review: the first move held in the fake, the second replaced by the
    // third before it's sent: the second answers `Dropped`, with its cursor and no values;
    // a poll of the first answers nothing while it's held, and its frame once released.
    let (solvers, started) = pool();
    let Opened { drag, .. } = opened(&solvers, "s0000001", "pa000003");
    let fake = started.lock().unwrap()[0].clone();
    fake.hold();
    let first = drag.move_to([61.0, 6.0]).unwrap();
    assert!(fake.wait_held(std::time::Duration::from_secs(10)));
    assert!(first.try_take().is_none());
    let second = drag.move_to([62.0, 6.0]).unwrap();
    let third = drag.move_to([63.0, 6.0]).unwrap();
    assert_eq!(
        second.wait().unwrap(),
        Frame {
            to: [62.0, 6.0],
            kind: FrameKind::Dropped,
            points: vec![],
            radii: vec![],
            why: None,
        }
    );
    fake.release();
    assert_eq!(third.wait().unwrap().kind, FrameKind::Solved);
    let took = first.try_take().unwrap().unwrap();
    assert_eq!((took.to, took.kind), ([61.0, 6.0], FrameKind::Solved));
}

#[test]
fn a_frames_radii_are_in_the_order_of_their_ids_arcs_and_circles_together() {
    // An arc, ag000001, about pg000001 from pg000002 (5, 0) to pg000003 (0, 5), and a
    // circle, cg000001, about the same centre, radius 5, with pg000002 held on it: one
    // part, both radii free. By ID, the arc's first, though circles are translated first.
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "s0000007", "type": "sketch", "v": 1, "name": "Rounds", "plane": "xy", "points": [{"id": "pg000001", "x": 0, "y": 0, "construction": false}, {"id": "pg000002", "x": 5, "y": 0, "construction": false}, {"id": "pg000003", "x": 0, "y": 5, "construction": false}], "lines": [], "circles": [{"id": "cg000001", "center": "pg000001", "radius": 5, "construction": false}], "arcs": [{"id": "ag000001", "center": "pg000001", "start": "pg000002", "end": "pg000003", "construction": false}], "constraints": [{"id": "kg000001", "type": "on", "point": "pg000002", "curve": "cg000001"}]}]}"#;
    let doc = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    let (solvers, _) = pool();
    let Opened { drag, .. } = begin(
        &solvers,
        None,
        &doc,
        &feature("s0000007"),
        &id("pg000003"),
        None,
    )
    .unwrap();
    let frame = drag.move_to([1.0, 6.0]).unwrap().wait().unwrap();
    assert_eq!(frame.kind, FrameKind::Solved);
    assert_eq!(frame.radii, [(id("ag000001"), 5.0), (id("cg000001"), 5.0)]);
}

/// A solver whose drag begin (the worker's diagnosis of the whole sketch) runs until its
/// stopper is called, or ten seconds, and is then stopped, as a restarted worker's is.
struct SlowBegin {
    inner: FakeSketches,
    stopped: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl SketchSolver for SlowBegin {
    fn solve(
        &mut self,
        system: silico_protocol::SketchSystem,
    ) -> Result<silico_protocol::Solved, Error> {
        self.inner.solve(system)
    }

    fn stopper(&mut self) -> Result<Option<silico_core::sketch::SolveStopper>, Error> {
        let stopped = Arc::clone(&self.stopped);
        Ok(Some(silico_core::sketch::SolveStopper::new(move || {
            *stopped.0.lock().unwrap() = true;
            stopped.1.notify_all();
        })))
    }

    fn drag_begin(
        &mut self,
        begin: silico_protocol::DragBegin,
    ) -> Result<silico_core::sketch::DragStarted, Error> {
        let mut stopped = self.stopped.0.lock().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !*stopped && std::time::Instant::now() < deadline {
            stopped = self
                .stopped
                .1
                .wait_timeout(stopped, std::time::Duration::from_millis(50))
                .unwrap()
                .0;
        }
        if *stopped {
            return Err(Error::new(ErrorCode::WorkerCrashed, "stopped mid-begin"));
        }
        drop(stopped);
        self.inner.drag_begin(begin)
    }
}

#[test]
fn a_stop_during_the_workers_begin_stops_its_diagnosis() {
    // Found by the robustness review of the race fix: the worker's begin, its diagnosis of
    // the whole sketch, ran outside the drag's stop, so a stop waited for it (up to the
    // solver's time). Here the begin runs until stopped: a stop a moment in refuses the
    // begin, `cancelled`, at once, not after the ten seconds.
    let doc = document();
    let held = solve(&doc, &feature("s0000001"), &mut FakeSketches::new()).unwrap();
    let solvers = SketchSolvers::starting(|| {
        Ok(Box::new(SlowBegin {
            inner: FakeSketches::new(),
            stopped: Arc::default(),
        }) as Box<dyn SketchSolver>)
    });
    let stop = DragStop::new();
    let started = std::time::Instant::now();
    let begun = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let doc = document();
            let sketch = sketch_of(&doc, "s0000001");
            begin_stoppable(
                &solvers,
                None,
                &doc,
                &feature("s0000001"),
                &id("pa000003"),
                Some(Held {
                    sketch: &sketch,
                    solution: &held,
                }),
                &stop,
            )
            .err()
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(200));
    stop.stop();
    let e = begun.join().unwrap().expect("the begin went on");
    assert_eq!(e.code(), ErrorCode::Cancelled);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the stop waited for the begin: {:?}",
        started.elapsed()
    );
}

/// `f` run on a thread of its own, its result sent when it's done: a test waits for it with
/// a bound, so a stop that doesn't stop fails it rather than hanging it.
fn on_a_thread<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> std::sync::mpsc::Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx
}

#[test]
fn a_models_held_solve_is_its_sketchs_and_none_for_what_isnt_a_solved_sketch() {
    // Held::of reads the model's sketch and its solve together: the profile's and the
    // holes' each their own; a pad, or a feature the model hasn't, has none.
    use silico_core::recompute::{FeatureOutcome, Recompute};
    let engine = Recompute::new(silico_core::fake::FakeKernel::new(2).unwrap())
        .with_sketches(FakeSketches::new());
    let model = crate::model::published(&engine, document());
    for f in ["s0000001", "s0000002"] {
        let held = Held::of(&model, &feature(f)).expect("a sketch's solve");
        assert_eq!(held.sketch, &sketch_of(&document(), f), "{f}");
        let Some(FeatureOutcome::Sketch(solution)) = model.outcome(&feature(f)) else {
            panic!("{f} unsolved")
        };
        assert!(std::ptr::eq(held.solution, solution.as_ref()), "{f}");
    }
    assert!(Held::of(&model, &feature("p0000001")).is_none());
    assert!(Held::of(&model, &feature("s0000009")).is_none());
}
