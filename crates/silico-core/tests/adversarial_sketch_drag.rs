//! break-it, M5.3: `silico_core::drag::sketch` on the fake sketch solver: ends whose
//! settling fails (a file the gate refuses, a solver that crashes, loads that never
//! settle), frames the worker can't be trusted for, a worker that fails mid-drag, a cancel,
//! and a held solve that isn't the sketch's. Expected values are the drag's own contract:
//! the begin state kept and why said, naming the sketch, or the error, named; and the pool
//! still drags afterwards.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use silico_core::document::{Checked, ElementId, FeatureId, Operation, Sketch, read};
use silico_core::drag::Kept;
use silico_core::drag::sketch::{FrameKind, Held, Opened, begin};
use silico_core::fake::{DragCall, FakeSketches};
use silico_core::recompute::SketchSolvers;
use silico_core::sketch::{SketchSolver, solve};
use silico_core::{Error, ErrorCode};
use silico_protocol::{Moved, SolveStatus};

/// The bracket's L profile, as sketch_drag.rs has it. Its points: pa000001 (0, 0),
/// pa000002 (60, 0), pa000003 (60, 5), pa000004 (5, 5), pa000005 (5, 50), pa000006 (0, 50);
/// values 6 to 17 in that order, after the origin's and the axes' six.
const PROFILE: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 60, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}, {"id": "pa000005", "x": 5, "y": 50, "construction": false}, {"id": "pa000006", "x": 0, "y": 50, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "horizontal", "line": "na000005"}, {"id": "ka000006", "type": "vertical", "line": "na000006"}, {"id": "ka000007", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000008", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, {"id": "ka00000b", "type": "length", "line": "na000005", "value": 5, "driving": true}, {"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}]}"#;

const HOLES: &str = r#"{"id": "s0000002", "type": "sketch", "v": 1, "name": "Holes", "plane": "xy", "points": [{"id": "pb000001", "x": 20, "y": 10, "construction": false}, {"id": "pb000002", "x": 36, "y": 10, "construction": false}, {"id": "pb000003", "x": 38.75, "y": 10, "construction": false}], "lines": [], "circles": [{"id": "cb000001", "center": "pb000001", "radius": 2.75, "construction": false}, {"id": "cb000002", "center": "pb000002", "radius": 2.75, "construction": false}], "arcs": [], "constraints": [{"id": "kb000001", "type": "radius", "curve": "cb000001", "value": 2.75, "driving": true}, {"id": "kb000002", "type": "on", "point": "pb000003", "curve": "cb000002"}]}"#;

fn document() -> Checked {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{PROFILE}, {HOLES}]}}"#
    );
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn feature(s: &str) -> FeatureId {
    FeatureId::new(s).unwrap()
}

fn profile() -> Sketch {
    match &document().feature(&feature("s0000001")).unwrap().operation {
        Operation::Sketch(sketch) => sketch.clone(),
        _ => panic!("not a sketch"),
    }
}

/// Solvers that are all `fake`.
fn on(fake: &FakeSketches) -> SketchSolvers {
    let fake = fake.clone();
    SketchSolvers::starting(move || Ok(Box::new(fake.clone()) as Box<dyn SketchSolver>))
}

fn opened(solvers: &SketchSolvers) -> Opened {
    begin(
        solvers,
        None,
        &document(),
        &feature("s0000001"),
        &id("pa000003"),
        None,
    )
    .unwrap()
}

/// A full drag of pa000003 on a fresh fake, to (62.5, 7): it ends with the cursor kept.
fn a_drag_runs(solvers: &SketchSolvers) {
    let Opened { drag, .. } = opened(solvers);
    let ended = drag.end(Some([62.5, 7.0])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor, "{:?}", ended.why);
}

#[test]
fn an_end_at_a_line_of_no_length_keeps_the_begin_state_saying_why() {
    // The move puts pa000003 exactly on pa000004 (5, 5): na000003 between them has no
    // length, which the system's own check refuses to solve from, so the end's move is
    // taken as invalid, the begin state kept, and why says the result can't be built,
    // naming the sketch (not the line: an invalid frame's values are withheld, M3.7).
    let fake = FakeSketches::new();
    fake.answer_moves(|begin, _| {
        let mut values = begin.system.values.clone();
        (values[10], values[11]) = (5.0, 5.0);
        Ok(Moved {
            status: SolveStatus::Success,
            values: Some(values),
        })
    });
    let solvers = on(&fake);
    let Opened { drag, .. } = opened(&solvers);
    let ended = drag.end(Some([5.0, 5.0])).unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Begin, false));
    assert_eq!(ended.sketch, profile());
    let why = ended.why.unwrap();
    assert_eq!(why.code(), ErrorCode::InvalidResult, "{why:?}");
    assert_eq!(why.feature(), Some("s0000001"));
    assert!(why.message().contains("a line of no length"), "{why:?}");
}

#[test]
fn a_solver_that_crashes_settling_the_end_is_the_error_and_the_next_drag_runs() {
    // Solves 1 (the begin's) and 2 (the end's whole sketch) answer; 3, the settle's, is a
    // crash: the end is that error, naming the sketch; the next drag on the pool runs.
    let fake = FakeSketches::new();
    let solves = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&solves);
    fake.answer(move |system| {
        if count.fetch_add(1, Ordering::SeqCst) + 1 == 3 {
            return Err(Error::new(
                ErrorCode::WorkerCrashed,
                "the fake sketch solver crashed settling",
            ));
        }
        Ok(FakeSketches::as_sent(system))
    });
    let solvers = on(&fake);
    let Opened { drag, .. } = opened(&solvers);
    let Err(e) = drag.end(Some([62.5, 7.0])) else {
        panic!("ended")
    };
    assert_eq!(e.code(), ErrorCode::WorkerCrashed);
    assert_eq!(e.feature(), Some("s0000001"));
    a_drag_runs(&solvers);
}

#[test]
fn an_end_whose_loads_never_settle_keeps_the_begin_state_saying_so() {
    // From the settle on (solve 3), every solve moves every unknown by a micrometre more
    // than the last: no load gives its file back, so the settle refuses, and the begin
    // state is kept, why a `solve_failed` naming the sketch.
    let fake = FakeSketches::new();
    let solves = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&solves);
    fake.answer(move |system| {
        let n = count.fetch_add(1, Ordering::SeqCst) + 1;
        let mut solved = FakeSketches::as_sent(system);
        if n >= 3 {
            for v in &system.unknowns {
                solved.values[v.0 as usize] += 1e-3 * n as f64;
            }
        }
        Ok(solved)
    });
    let solvers = on(&fake);
    let Opened { drag, .. } = opened(&solvers);
    let ended = drag.end(Some([62.5, 7.0])).unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Begin, false));
    assert_eq!(ended.sketch, profile());
    let why = ended.why.unwrap();
    assert_eq!(
        (why.code(), why.feature()),
        (ErrorCode::SolveFailed, Some("s0000001")),
        "{why:?}"
    );
}

#[test]
fn a_worker_that_crashes_on_a_move_ends_the_drag_and_an_end_with_a_cursor_says_so() {
    let fake = FakeSketches::new();
    fake.answer_moves(|_, _| {
        Err(Error::new(
            ErrorCode::WorkerCrashed,
            "the fake sketch solver crashed moving",
        ))
    });
    let solvers = on(&fake);
    let Opened { drag, .. } = opened(&solvers);
    let Err(e) = drag.move_to([62.5, 7.0]).unwrap().wait() else {
        panic!("a frame")
    };
    assert_eq!(e.code(), ErrorCode::WorkerCrashed);
    assert!(!drag.is_open());
    assert_eq!(drag.closed().unwrap().code(), ErrorCode::WorkerCrashed);
    // A move after it is refused as the drag having ended.
    assert!(drag.move_to([62.0, 7.0]).unwrap().wait().is_err());
    let Err(e) = drag.end(Some([62.5, 7.0])) else {
        panic!("ended")
    };
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::WorkerCrashed, Some("s0000001"))
    );
    // Esc after the same failure ends with the file's sketch.
    let Opened { drag, .. } = opened(&solvers);
    assert!(drag.move_to([62.5, 7.0]).unwrap().wait().is_err());
    let ended = drag.end(None).unwrap();
    assert_eq!(
        (ended.kept, ended.changed, ended.why),
        (Kept::Begin, false, None)
    );
    assert_eq!(ended.sketch, profile());
}

#[test]
fn a_frame_with_a_value_that_isnt_a_number_is_the_workers_protocol_error() {
    // The fake answers pa000003 at (NaN, 7): no frame is read from it, and the drag ends.
    let fake = FakeSketches::new();
    fake.answer_moves(|begin, _| {
        let mut values = begin.system.values.clone();
        (values[10], values[11]) = (f64::NAN, 7.0);
        Ok(Moved {
            status: SolveStatus::Success,
            values: Some(values),
        })
    });
    let solvers = on(&fake);
    let Opened { drag, .. } = opened(&solvers);
    let Err(e) = drag.move_to([62.5, 7.0]).unwrap().wait() else {
        panic!("a frame")
    };
    assert_eq!(e.code(), ErrorCode::ProtocolError, "{e:?}");
    assert!(!drag.is_open());
}

#[test]
fn a_cancelled_move_answers_cancelled_and_the_end_keeps_the_file() {
    let fake = FakeSketches::new();
    let solvers = on(&fake);
    let Opened { drag, .. } = opened(&solvers);
    // Held from the move on: the begin's solve isn't.
    fake.hold();
    let ticket = drag.move_to([62.5, 7.0]).unwrap();
    assert!(fake.wait_held(Duration::from_secs(30)));
    assert!(drag.cancel_move());
    let frame = ticket.wait().unwrap();
    assert_eq!(frame.kind, FrameKind::Cancelled);
    assert!(frame.points.is_empty() && frame.radii.is_empty());
    fake.release();
    let ended = drag.end(None).unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Begin, false));
    assert_eq!(ended.sketch, profile());
    a_drag_runs(&solvers);
}

#[test]
fn a_held_solve_of_another_sketch_isnt_the_begin_state_and_the_drag_solves_its_own() {
    // The holes' solve, passed as the profile's: it solved another sketch, so it's not
    // taken; the drag solves the profile on its own solver (the fake, where it's drawn).
    let doc = document();
    let holes = solve(&doc, &feature("s0000002"), &mut FakeSketches::new()).unwrap();
    let holes_sketch = match &doc.feature(&feature("s0000002")).unwrap().operation {
        Operation::Sketch(sketch) => sketch.clone(),
        _ => panic!("not a sketch"),
    };
    let fake = FakeSketches::new();
    let solvers = on(&fake);
    let Opened { drag, .. } = begin(
        &solvers,
        None,
        &doc,
        &feature("s0000001"),
        &id("pa000003"),
        Some(Held {
            sketch: &holes_sketch,
            solution: &holes,
        }),
    )
    .unwrap();
    assert_eq!(fake.requests().len(), 1);
    let Some(DragCall::Begin(sent)) = fake.drag_calls().first().cloned() else {
        panic!("no begin")
    };
    assert_eq!(
        &sent.system.values[6..18],
        &[
            0.0, 0.0, 60.0, 0.0, 60.0, 5.0, 5.0, 5.0, 5.0, 50.0, 0.0, 50.0
        ]
    );
    let ended = drag.end(None).unwrap();
    assert_eq!(ended.sketch, profile());
}

#[test]
fn a_held_solve_of_an_older_revision_of_the_sketch_isnt_the_begin_state() {
    // Found by the architecture review: a model's solve of the profile as an earlier
    // revision held it (its base 70 long, pa000003 solved to (70, 9)) was taken for this
    // revision's, under this revision's constraints. It solved another sketch, so the drag
    // solves this one itself, from the file's (60, 5).
    let older_text = PROFILE.replace(
        r#""line": "na000001", "value": 60"#,
        r#""line": "na000001", "value": 70"#,
    );
    assert_ne!(older_text, PROFILE);
    let older_doc = Checked::new(
        read(
            format!(
                r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{older_text}]}}"#
            )
            .as_bytes(),
        )
        .unwrap(),
    )
    .unwrap();
    let older = match &older_doc.feature(&feature("s0000001")).unwrap().operation {
        Operation::Sketch(sketch) => sketch.clone(),
        _ => panic!("not a sketch"),
    };
    let mut held = solve(&older_doc, &feature("s0000001"), &mut FakeSketches::new()).unwrap();
    held.points.insert(
        id("pa000003"),
        [
            silico_core::document::Coordinate::new(70.0).unwrap(),
            silico_core::document::Coordinate::new(9.0).unwrap(),
        ],
    );
    let fake = FakeSketches::new();
    let solvers = on(&fake);
    let Opened { drag, .. } = begin(
        &solvers,
        None,
        &document(),
        &feature("s0000001"),
        &id("pa000003"),
        Some(Held {
            sketch: &older,
            solution: &held,
        }),
    )
    .unwrap();
    assert_eq!(fake.requests().len(), 1);
    let Some(DragCall::Begin(sent)) = fake.drag_calls().first().cloned() else {
        panic!("no begin")
    };
    // pa000003's values, 10 and 11: the file's, not the held solve's.
    assert_eq!(&sent.system.values[10..12], &[60.0, 5.0]);
    drop(drag);
}

#[test]
fn many_drags_at_once_on_one_pool_each_end_as_alone() {
    // Eight threads each drag pa000003 to their own cursor on one pool: each end is the
    // file with pa000003 at its cursor, nudged as the fake nudges (a thousandth of (60, 5)).
    let solvers =
        SketchSolvers::starting(|| Ok(Box::new(FakeSketches::new()) as Box<dyn SketchSolver>));
    let ends: Arc<Mutex<Vec<(usize, Sketch)>>> = Arc::default();
    std::thread::scope(|scope| {
        for i in 0..8 {
            let (solvers, ends) = (solvers.clone(), Arc::clone(&ends));
            scope.spawn(move || {
                let Opened { drag, .. } = opened(&solvers);
                let to = [61.0 + i as f64, 6.0 + i as f64];
                drag.move_to(to).unwrap().wait().unwrap();
                let ended = drag.end(Some(to)).unwrap();
                assert_eq!(ended.kept, Kept::Cursor);
                ends.lock().unwrap().push((i, ended.sketch));
            });
        }
    });
    for (i, sketch) in ends.lock().unwrap().iter() {
        let p = sketch
            .points
            .iter()
            .find(|p| p.id.as_str() == "pa000003")
            .unwrap();
        let want = [61.0 + *i as f64 + 0.06, 6.0 + *i as f64 + 0.005];
        assert_eq!([p.x.mm(), p.y.mm()], want, "drag {i}");
    }
}
