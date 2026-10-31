//! `drag_begin` and its session's close (docs/design/m5.0-app.md, `drag_begin`: a drag ends
//! when its session closes), at the one place the close can miss it: the drag begun and the
//! begin settled, but not yet held by the session, so the close ends no drag. The begin
//! must then end its own drag before it answers. Expected values: the design's `close` (a
//! command stopped by its session's close answers as of a session not open) and the fake
//! solver's record of what it was sent.

use super::*;
use crate::command::DragGrab;
use crate::document::ElementId;
use crate::fake::{DragCall, FakeSketches};

/// A sketch of one free point, `p0000001`, at (1000, 2000).
const DOT: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Dot", "plane": "xy", "points": [{"id": "p0000001", "x": 1000, "y": 2000, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}]}"#;

#[test]
fn a_begin_whose_session_closed_before_it_held_its_drag_ends_the_drag_and_answers_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("a.silico");
    std::fs::write(&path, DOT).unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let fake = FakeSketches::new();
    let solver = fake.clone();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    let id = sessions.open(&path, false).wait().session.unwrap().id;
    let session = sessions.find(&id).unwrap();
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    session.dragged.shut(started, wait);
    let begin = sessions.drag_begin(
        &id,
        FeatureId::new("s0000001").unwrap(),
        DragGrab::Point(ElementId::new("p0000001").unwrap()),
    );
    // Begun on the worker, not yet the session's: the close finds no drag to end.
    was_started.recv_timeout(WAIT).unwrap();
    assert_eq!(fake.drag_calls().len(), 1);
    assert!(matches!(fake.drag_calls()[0], DragCall::Begin(_)));
    session.close();
    through.send(()).unwrap();
    let begun = begin.wait();
    assert_eq!(begun.result.unwrap_err().errors(), [not_open(id)]);
    // By the time the begin answered, it had ended the drag the close missed: the worker's
    // drag ended, and the session holds none.
    assert_eq!(fake.drag_calls().last(), Some(&DragCall::End));
    assert_eq!(fake.drag_calls().len(), 2);
    assert_eq!(session.drag_open(), None);
}
