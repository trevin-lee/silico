//! Break-it tests of `drag_begin`, `drag_move`, and `drag_end` as session commands (PR #307):
//! moves racing ends, edits, and cancels; many threads moving at once; cancels of a begin's
//! and an end's solve; clicks; a release's independence of the moves before it (A5);
//! hostile wire shapes; a random sequence of every command, each answering and the drag's
//! state as a model of it says; and, on PlaneGCS itself, the degenerate drags an agent can
//! ask for (a circle pressed at its centre or off its edge, a line's end dragged onto its
//! other, a circle's edge dragged to its centre, a point with no freedom).
//!
//! Expected values: the fake sketch solver's rule by hand (a dragged point at the cursor,
//! nudged by a thousandth of where it began); the drag's own contract (every move answered,
//! a drag either open and movable or refused as none); and, on PlaneGCS, geometry (a
//! circle's edge dragged to a point passes through it; a sketch's lines keep a length and
//! its circles a radius above zero, as the gate requires of a file).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use proptest::prelude::*;
use serde_json::Value;
use silico_core::cache::Cache;
use silico_core::command::{
    Call, DragFrame, DragGrab, FrameEnd, Response, Revision, SessionCommand, SessionId, Sessions,
};
use silico_core::document::json::read;
use silico_core::document::{ElementId, FeatureId};
use silico_core::drag::Kept;
use silico_core::fake::{DragCall, FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};
use tempfile::TempDir;

use crate::bracket::S0000001;

const HANG: Duration = Duration::from_secs(60);

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

fn fid(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn eid(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn answer<T>(running: silico_core::command::Running<T>) -> Response<T> {
    match running.wait_timeout(HANG) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in {HANG:?}"),
    }
}

fn the_error<T: std::fmt::Debug>(response: &Response<T>) -> Error {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{:?}", problems.errors());
    problems.errors()[0].clone()
}

fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

/// The bracket's L profile with its base leg's thickness, `ka00000a`, removed: `pa000003`
/// (60, 5) and `pa000004` (5, 5) free to move up and down together.
fn profile() -> String {
    document(&[&S0000001.replace(
        r#"{"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, "#,
        "",
    )])
}

struct Fixture {
    dir: TempDir,
    sketches: FakeSketches,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let sessions = Sessions::with_stand_ins(
        kernel,
        move |cache: &Path| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("part.silico");
    std::fs::write(&path, text).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    assert_eq!(id, s1());
    Fixture {
        dir,
        sketches,
        sessions,
    }
}

impl Fixture {
    fn file(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("part.silico")).unwrap()
    }

    fn revision(&self) -> Revision {
        self.sessions.status(&s1()).session.unwrap().revision
    }

    fn open(&self) -> bool {
        self.sessions.status(&s1()).result.unwrap().drag.is_some()
    }

    fn text(&self) -> String {
        answer(self.sessions.text(&s1(), None)).result.unwrap().text
    }

    fn begin_point(&self, point: &str) -> Response<silico_core::command::DragBegun> {
        answer(
            self.sessions
                .drag_begin(&s1(), fid("s0000001"), DragGrab::Point(eid(point))),
        )
    }

    fn moved(&self, to: [f64; 2]) -> Response<DragFrame> {
        answer(self.sessions.drag_move(&s1(), to))
    }

    fn ended(&self, to: Option<[f64; 2]>) -> Response<silico_core::command::DragEnded> {
        answer(self.sessions.drag_end(&s1(), to))
    }

    fn rename(&self, name: &str) -> Response<silico_core::command::FieldSet> {
        answer(self.sessions.set_field(
            &s1(),
            self.revision(),
            fid("s0000001"),
            "name".to_owned(),
            read(format!("\"{name}\"").as_bytes()).unwrap(),
        ))
    }

    fn call(&self, name: &str, arguments: &str) -> Value {
        match self.sessions.call(name, arguments) {
            Call::Running(calling) => {
                let answered = calling
                    .wait_timeout(HANG)
                    .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}"));
                serde_json::from_str(&answered.text).unwrap()
            }
            Call::Unknown => panic!("no command {name}"),
        }
    }

    /// After whatever came before: the drag's commands still work, from a new begin to a
    /// release that's one edit.
    fn still_works(&self) {
        if self.open() {
            self.ended(None).result.unwrap();
        }
        let before = self.revision();
        self.begin_point("pa000003").result.unwrap();
        assert_eq!(
            self.moved([62.5, 7.0]).result.unwrap().frame,
            FrameEnd::Solved
        );
        let ended = self.ended(Some([62.5, 7.0])).result.unwrap();
        assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
        assert_eq!(self.revision(), before.next().unwrap());
        assert!(!self.open());
    }
}

/// The point `id`'s `(x, y)` in `text`'s first sketch.
fn point_in(text: &str, id: &str) -> (f64, f64) {
    let document: Value = serde_json::from_str(text).unwrap();
    let point = document["features"][0]["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("{id}: {text}"))
        .clone();
    (point["x"].as_f64().unwrap(), point["y"].as_f64().unwrap())
}

// ---------------------------------------------------------------------------------------
// Moves racing ends, edits, and cancels.
// ---------------------------------------------------------------------------------------

#[test]
fn esc_while_a_move_is_held_solving_answers_the_move_and_ends_the_drag() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    let esc = f.ended(None).result.unwrap();
    assert_eq!((esc.kept, esc.changed), (Kept::Begin, false));
    // The move held solving was stopped: answered, not left waiting.
    let moved = answer(moving);
    assert_eq!(the_error(&moved).code(), ErrorCode::Cancelled);
    f.sketches.release();
    assert!(!f.open());
    assert_eq!(f.revision(), Revision::OPENED);
    assert_eq!(f.file(), profile());
    f.still_works();
}

#[test]
fn a_move_waiting_behind_a_held_one_is_answered_when_esc_ends_the_drag() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let first = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    let waiting = f.sessions.drag_move(&s1(), [70.0, 9.0]);
    f.ended(None).result.unwrap();
    // Neither left unanswered: the first stopped, the second dropped or refused, never
    // solved (the drag ended before it ran).
    assert_eq!(the_error(&answer(first)).code(), ErrorCode::Cancelled);
    let second = answer(waiting);
    match &second.result {
        Ok(frame) => assert_eq!(frame.frame, FrameEnd::Dropped, "{frame:?}"),
        Err(problems) => assert!(
            matches!(
                problems.errors()[0].code(),
                ErrorCode::Cancelled | ErrorCode::InvalidInput
            ),
            "{problems:?}"
        ),
    }
    f.sketches.release();
    assert!(!f.open());
    f.still_works();
}

#[test]
fn a_release_with_a_cursor_while_a_move_is_held_waits_for_it_then_keeps_the_cursor() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [70.0, 9.0]);
    assert!(f.sketches.wait_held(HANG));
    let release = f.sessions.drag_end(&s1(), Some([62.5, 7.0]));
    f.sketches.release();
    let moved = answer(moving);
    // The move ran to its end (or was dropped for the release): never an error.
    let frame = moved.result.unwrap();
    assert!(
        matches!(frame.frame, FrameEnd::Solved | FrameEnd::Dropped),
        "{frame:?}"
    );
    let ended = answer(release).result.unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    // The fake's rule: the dragged point at the cursor, nudged by a thousandth of where it
    // began: (62.5 + 0.06, 7 + 0.005).
    assert_eq!(
        point_in(&f.file(), "pa000003"),
        (62.5 + 60.0 / 1000.0, 7.0 + 5.0 / 1000.0)
    );
    assert!(!f.open());
}

/// [`profile`] and a block after it, whose name an edit can change without touching the
/// sketch.
fn profile_and_block() -> String {
    profile().replace(
        "]}]}",
        r#"]}, {"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}]}"#,
    )
}

#[test]
fn an_edit_while_a_move_is_held_ends_the_drag_and_answers_the_move() {
    let f = opened(&profile_and_block());
    // Inside a transaction: the edit is held, no rebuild waited for.
    answer(f.sessions.begin(&s1())).result.unwrap();
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    // Another client's edit: a new revision, which ends the drag as it answers.
    let renamed = f.sessions.set_field(
        &s1(),
        Revision::OPENED,
        fid("b0000001"),
        "name".to_owned(),
        read(br#""Renamed""#).unwrap(),
    );
    let renamed = answer(renamed);
    renamed.result.unwrap();
    // By then the move has answered: the drag it belonged to has ended.
    let moved = moving
        .try_take()
        .unwrap_or_else(|_| panic!("the move hadn't answered when the edit did"));
    assert!(moved.result.is_err(), "{:?}", moved.result);
    f.sketches.release();
    assert!(!f.open());
    assert_eq!(
        the_error(&f.moved([1.0, 1.0])).message(),
        "no drag is open in session s1"
    );
    f.still_works();
}

#[test]
fn eight_threads_moving_at_once_each_move_answers_solved_or_dropped() {
    let f = Arc::new(opened(&profile()));
    f.begin_point("pa000003").result.unwrap();
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let f = Arc::clone(&f);
            std::thread::spawn(move || {
                let running: Vec<_> = (0..60)
                    .map(|i| {
                        let to = [60.0 + f64::from(t), 5.0 + f64::from(i) / 10.0];
                        (to, f.sessions.drag_move(&s1(), to))
                    })
                    .collect();
                running
                    .into_iter()
                    .map(|(to, r)| (to, answer(r)))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut solved = 0;
    for thread in threads {
        for (to, response) in thread.join().unwrap() {
            let frame = response.result.unwrap();
            assert_eq!(frame.to, to);
            match frame.frame {
                FrameEnd::Solved => {
                    solved += 1;
                    // The fake's rule, the frame its own move's.
                    let p = frame
                        .points
                        .iter()
                        .find(|(id, _)| id.as_str() == "pa000003")
                        .unwrap()
                        .1;
                    assert_eq!(p, [to[0] + 0.06, to[1] + 0.005], "{to:?}");
                }
                FrameEnd::Dropped => assert!(frame.points.is_empty()),
                other => panic!("{other:?}"),
            }
        }
    }
    assert!(solved >= 1);
    assert!(f.open());
    f.still_works();
}

#[test]
fn moves_racing_a_release_each_answer_and_the_release_is_one_edit() {
    let f = Arc::new(opened(&profile()));
    f.begin_point("pa000003").result.unwrap();
    let mover = {
        let f = Arc::clone(&f);
        std::thread::spawn(move || {
            (0..200)
                .map(|i| {
                    answer(
                        f.sessions
                            .drag_move(&s1(), [61.0, 5.0 + f64::from(i) / 100.0]),
                    )
                })
                .collect::<Vec<_>>()
        })
    };
    let ended = f.ended(Some([62.5, 7.0])).result.unwrap();
    let moves = mover.join().unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    assert_eq!(f.revision(), Revision::OPENED.next().unwrap());
    // The release is a function of the begin state and its cursor alone (A5).
    assert_eq!(
        point_in(&f.file(), "pa000003"),
        (62.5 + 60.0 / 1000.0, 7.0 + 5.0 / 1000.0)
    );
    // Every move answered: solved or dropped before the release, refused after it.
    for moved in &moves {
        match &moved.result {
            Ok(frame) => assert!(
                matches!(frame.frame, FrameEnd::Solved | FrameEnd::Dropped),
                "{frame:?}"
            ),
            Err(problems) => {
                let error = &problems.errors()[0];
                assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
                assert_eq!(error.parameter(), Some("session"), "{error:?}");
            }
        }
    }
}

#[test]
fn a_cancel_of_drag_begin_while_its_solve_is_held_leaves_no_drag_and_the_next_begins() {
    let f = opened(&profile());
    f.sketches.hold();
    let begin = f
        .sessions
        .drag_begin(&s1(), fid("s0000001"), DragGrab::Point(eid("pa000003")));
    assert!(f.sketches.wait_held(HANG));
    let cancelled = f
        .sessions
        .cancel(&s1(), SessionCommand::DragBegin)
        .result
        .unwrap();
    assert!(cancelled.stopped);
    let begun = answer(begin);
    assert_eq!(the_error(&begun).code(), ErrorCode::Cancelled);
    f.sketches.release();
    assert!(!f.open());
    // No drag was left with the worker: every begin it saw was ended.
    let calls = f.sketches.drag_calls();
    let begins = calls
        .iter()
        .filter(|c| matches!(c, DragCall::Begin(_)))
        .count();
    let ends = calls.iter().filter(|c| matches!(c, DragCall::End)).count();
    assert_eq!(begins, ends, "{calls:?}");
    f.still_works();
}

#[test]
fn a_cancel_of_drag_end_while_its_move_is_held_changes_nothing_and_the_next_begins() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    // No move before: the release moves to its cursor, held.
    let release = f.sessions.drag_end(&s1(), Some([62.5, 7.0]));
    assert!(f.sketches.wait_held(HANG));
    let cancelled = f
        .sessions
        .cancel(&s1(), SessionCommand::DragEnd)
        .result
        .unwrap();
    assert!(cancelled.stopped);
    // BREAKS: the cancel says it stopped the end, but only the end's whole solve is under
    // its stop (`StopDrag(drag.stop())`, session/drag.rs `end_drag_with`): the release's
    // move to its cursor runs on the drag's thread, which nothing the cancel reaches stops,
    // so the end waits for that move, up to DRAG_MOVE_TIMEOUT (10 s) on a real worker,
    // with the session's queue held behind it. The fake holds it until released: the end
    // never answers the cancel.
    let prompt = release.wait_timeout(Duration::from_secs(20));
    f.sketches.release();
    let ended = match prompt {
        Ok(ended) => ended,
        Err(release) => {
            let late = answer(release);
            panic!(
                "a cancelled drag_end didn't answer until its move was let go: {:?}",
                late.result
            );
        }
    };
    // Cancelled: nothing changed, no drag open.
    assert_eq!(the_error(&ended).code(), ErrorCode::Cancelled);
    assert_eq!(f.revision(), Revision::OPENED);
    assert_eq!(f.file(), profile());
    assert!(!f.open());
    f.still_works();
}

#[test]
fn a_cancel_of_drag_end_while_its_whole_solve_is_held_answers_cancelled_with_nothing_changed() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    // The newest frame at the release's cursor: the release moves no more, and solves the
    // sketch as a whole, held.
    assert_eq!(f.moved([62.5, 7.0]).result.unwrap().frame, FrameEnd::Solved);
    f.sketches.hold();
    let release = f.sessions.drag_end(&s1(), Some([62.5, 7.0]));
    assert!(f.sketches.wait_held(HANG));
    assert!(
        f.sessions
            .cancel(&s1(), SessionCommand::DragEnd)
            .result
            .unwrap()
            .stopped
    );
    let prompt = release.wait_timeout(Duration::from_secs(20));
    f.sketches.release();
    let ended = prompt.unwrap_or_else(|_| panic!("a cancelled whole solve didn't answer"));
    assert_eq!(the_error(&ended).code(), ErrorCode::Cancelled);
    assert_eq!(f.revision(), Revision::OPENED);
    assert_eq!(f.file(), profile());
    assert!(!f.open());
    f.still_works();
}

#[test]
fn a_close_while_a_release_moves_answers_and_ends_the_drag() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let release = f.sessions.drag_end(&s1(), Some([62.5, 7.0]));
    assert!(f.sketches.wait_held(HANG));
    // BREAKS: the close waits on the release's move: its stop reaches only the end's whole
    // solve, as a cancel's does (session/drag.rs `end_drag_with`), so the close answers only
    // once the move does (up to DRAG_MOVE_TIMEOUT on a real worker; the fake holds it until
    // released).
    let close = f.sessions.close(&s1());
    let closed = close.wait_timeout(Duration::from_secs(20));
    let released = release.wait_timeout(Duration::from_secs(20));
    f.sketches.release();
    let closed = closed.unwrap_or_else(|_| panic!("the close waited on the release's move"));
    closed.result.unwrap();
    let released =
        released.unwrap_or_else(|_| panic!("the release didn't answer its session's close"));
    assert!(released.result.is_err(), "{:?}", released.result);
    assert_eq!(f.file(), profile());
}

#[test]
fn a_cancel_of_drag_move_with_a_move_waiting_answers_both_and_ends_the_drag() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let first = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    let second = f.sessions.drag_move(&s1(), [70.0, 9.0]);
    f.sessions
        .cancel(&s1(), SessionCommand::DragMove)
        .result
        .unwrap();
    let first = answer(first);
    let second = answer(second);
    f.sketches.release();
    assert_eq!(the_error(&first).code(), ErrorCode::Cancelled);
    assert!(
        second.result.is_err() || second.result.as_ref().unwrap().frame == FrameEnd::Dropped,
        "{:?}",
        second.result
    );
    assert!(!f.open());
    f.still_works();
}

// ---------------------------------------------------------------------------------------
// Clicks, A5, and transactions.
// ---------------------------------------------------------------------------------------

#[test]
fn a_release_where_the_point_began_is_a_click_and_changes_nothing() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    f.moved([70.0, 9.0]).result.unwrap();
    let ended = f.ended(Some([60.0, 5.0])).result.unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Begin, false));
    assert!(ended.commit.is_none());
    assert_eq!(f.revision(), Revision::OPENED);
    assert_eq!(f.file(), profile());
}

#[test]
fn a_release_at_negative_zero_of_a_point_at_the_origin_is_a_click() {
    let f = opened(&profile());
    f.begin_point("pa000001").result.unwrap();
    let ended = f.ended(Some([-0.0, -0.0])).result.unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Begin, false));
    assert_eq!(f.file(), profile());
    // On the wire alike.
    f.begin_point("pa000001").result.unwrap();
    let ended = f.call("drag_end", r#"{"session": "s1", "to": [-0, -0.0e0]}"#);
    assert_eq!(ended["result"]["kept"], "begin", "{ended}");
    assert_eq!(f.file(), profile());
}

#[test]
fn a_release_is_the_same_whatever_moves_came_before_it() {
    // A5: a function of the begin state and the cursor alone.
    let plain = opened(&profile());
    plain.begin_point("pa000003").result.unwrap();
    plain.ended(Some([62.5, 7.0])).result.unwrap();
    let moved = opened(&profile());
    moved.begin_point("pa000003").result.unwrap();
    for to in [[70.0, 9.0], [80.0, -3.0], [62.5, 7.0], [10.0, 10.0]] {
        moved.moved(to).result.unwrap();
    }
    moved.ended(Some([62.5, 7.0])).result.unwrap();
    assert_eq!(plain.file(), moved.file());
    // And a release at the newest frame's cursor takes its values: the same again.
    let newest = opened(&profile());
    newest.begin_point("pa000003").result.unwrap();
    newest.moved([10.0, 10.0]).result.unwrap();
    newest.moved([62.5, 7.0]).result.unwrap();
    newest.ended(Some([62.5, 7.0])).result.unwrap();
    assert_eq!(plain.file(), newest.file());
}

#[test]
fn a_drag_begun_in_a_transaction_that_commits_is_either_open_and_releasable_or_ended() {
    let f = opened(&profile());
    answer(f.sessions.begin(&s1())).result.unwrap();
    f.rename("Renamed");
    f.begin_point("pa000003").result.unwrap();
    answer(f.sessions.commit(&s1())).result.unwrap();
    let open = f.open();
    let ended = f.ended(Some([62.5, 7.0]));
    if open {
        // Open after the commit: the release is an edit, its own commit, written.
        let ended = ended.result.unwrap();
        assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
        assert!(ended.commit.unwrap().written);
        assert_eq!(point_in(&f.file(), "pa000003"), (62.56, 7.005));
    } else {
        assert_eq!(the_error(&ended).message(), "no drag is open in session s1");
    }
    assert!(f.file().contains("Renamed"));
    f.still_works();
}

#[test]
fn a_drag_begun_in_a_transaction_that_rolls_back_has_ended() {
    let f = opened(&profile());
    answer(f.sessions.begin(&s1())).result.unwrap();
    f.rename("Renamed");
    f.begin_point("pa000003").result.unwrap();
    answer(f.sessions.rollback(&s1())).result.unwrap();
    assert!(!f.open());
    assert_eq!(
        the_error(&f.ended(Some([62.5, 7.0]))).message(),
        "no drag is open in session s1"
    );
    assert_eq!(f.file(), profile());
    f.still_works();
}

#[test]
fn a_begin_of_a_document_the_gate_refuses_is_refused_naming_the_session() {
    // A pad of no sketch: the gate refuses it.
    let broken = profile().replace(
        r#""features": ["#,
        r#""features": [{"id": "p0000001", "type": "pad", "v": 1, "name": "Legs", "sketch": "s9999999", "extent": {"type": "one_side", "length": 40, "reversed": true}}, "#,
    );
    let f = opened(&broken);
    let error = the_error(&f.begin_point("pa000003"));
    assert_eq!(
        (error.code(), error.parameter()),
        (ErrorCode::InvalidInput, Some("session"))
    );
    assert!(error.suggestion().is_some());
    assert!(!f.open());
    assert!(f.sketches.drag_calls().is_empty());
}

#[test]
fn a_second_begin_queued_behind_the_first_is_refused_and_the_first_stays_open() {
    let f = opened(&profile());
    let first = f
        .sessions
        .drag_begin(&s1(), fid("s0000001"), DragGrab::Point(eid("pa000003")));
    let second = f
        .sessions
        .drag_begin(&s1(), fid("s0000001"), DragGrab::Point(eid("pa000004")));
    answer(first).result.unwrap();
    let error = the_error(&answer(second));
    assert_eq!(error.parameter(), Some("session"));
    assert_eq!(
        error.message(),
        "a drag is already open in session s1, of pa000003 in s0000001"
    );
    // The first's drag still moves.
    let frame = f.moved([62.5, 7.0]).result.unwrap();
    assert_eq!(frame.frame, FrameEnd::Solved);
}

// ---------------------------------------------------------------------------------------
// Hostile wire shapes.
// ---------------------------------------------------------------------------------------

/// The envelope's errors' `(parameter, code)`.
fn wire_errors(envelope: &Value) -> Vec<(String, String)> {
    assert_eq!(envelope["ok"], false, "{envelope}");
    envelope["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["parameter"].as_str().unwrap_or("").to_owned(),
                e["code"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

#[test]
fn hostile_cursors_on_the_wire_are_refused_naming_their_place_and_the_drag_goes_on() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    let cases: [(&str, &[&str]); 14] = [
        (r#"{"session": "s1", "to": []}"#, &["to"]),
        (r#"{"session": "s1", "to": [1]}"#, &["to"]),
        (r#"{"session": "s1", "to": [1, 2, 3]}"#, &["to"]),
        (r#"{"session": "s1", "to": [[1, 2]]}"#, &["to"]),
        (r#"{"session": "s1", "to": [[1], [2]]}"#, &["to.0", "to.1"]),
        (
            r#"{"session": "s1", "to": [null, null]}"#,
            &["to.0", "to.1"],
        ),
        (r#"{"session": "s1", "to": ["1", 2]}"#, &["to.0"]),
        (r#"{"session": "s1", "to": {"x": 1, "y": 2}}"#, &["to"]),
        (r#"{"session": "s1", "to": "62.5, 7"}"#, &["to"]),
        (r#"{"session": "s1"}"#, &["to"]),
        (r#"{"session": "s1", "to": [1, 2], "extra": 1}"#, &["extra"]),
        // Within an `f64`, out of the sketch's reach: the drag's own check.
        (
            r#"{"session": "s1", "to": [1.7976931348623157e308, 0]}"#,
            &["to"],
        ),
        (r#"{"session": "s1", "to": [0, -1e999]}"#, &["to"]),
        (
            r#"{"session": "s1", "to": [1000000.000000001, 0]}"#,
            &["to"],
        ),
    ];
    for (arguments, want) in cases {
        let envelope = f.call("drag_move", arguments);
        let got = wire_errors(&envelope);
        let parameters: Vec<&str> = got.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(parameters, want, "{arguments}: {envelope}");
        assert!(
            got.iter().all(|(_, code)| code == "invalid_input"),
            "{arguments}: {envelope}"
        );
        assert!(f.open(), "{arguments}");
    }
    // Numbers JSON allows that read as plain values: taken.
    for arguments in [
        r#"{"session": "s1", "to": [4.9e-324, -4.9e-324]}"#,
        r#"{"session": "s1", "to": [1e-400, 0]}"#,
        r#"{"session": "s1", "to": [999999, -999999]}"#,
        r#"{"session": "s1", "to": [6.25E1, 7e+0]}"#,
    ] {
        let envelope = f.call("drag_move", arguments);
        assert_eq!(envelope["ok"], true, "{arguments}: {envelope}");
    }
    // A thousand-digit number: read correctly rounded, not refused.
    let long = format!(
        r#"{{"session": "s1", "to": [62.{}5, 7]}}"#,
        "0".repeat(1000)
    );
    let envelope = f.call("drag_move", &long);
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["result"]["to"][0], 62.0, "{envelope}");
    f.still_works();
}

#[test]
fn a_release_with_no_to_field_is_refused_and_the_drag_stays_open() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    // Every field required: an end with no `to` isn't an Esc.
    let envelope = f.call("drag_end", r#"{"session": "s1"}"#);
    assert_eq!(
        wire_errors(&envelope),
        [("to".to_owned(), "invalid_input".to_owned())]
    );
    assert!(f.open());
    // A duplicate key, the second an Esc: refused, not read as either.
    let envelope = f.call(
        "drag_end",
        r#"{"session": "s1", "to": [62.5, 7], "to": null}"#,
    );
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert!(f.open());
    assert_eq!(f.file(), profile());
}

#[test]
fn hostile_begins_on_the_wire_are_refused_naming_each_field_at_fault() {
    let f = opened(&profile());
    let begin = |rest: &str| format!(r#"{{"session": "s1", "sketch": "s0000001", {rest}}}"#);
    let cases: [(String, &[&str]); 9] = [
        (begin(r#""point": 1, "curve": 2, "press": 3"#), &["point", "curve", "press"]),
        (begin(r#""point": "", "curve": null, "press": null"#), &["point"]),
        (begin(r#""point": "pa0000031", "curve": null, "press": null"#), &["point"]),
        (begin(r#""point": "pa00000\u0000", "curve": null, "press": null"#), &["point"]),
        (begin(r#""point": ["pa000003"], "curve": null, "press": null"#), &["point"]),
        (begin(r#""point": null, "curve": "na000001", "press": [1e999, 0]"#), &["press"]),
        (begin(r#""point": null, "curve": "na000001", "press": [0, 0, 0]"#), &["press"]),
        (
            r#"{"session": "s1", "sketch": null, "point": "pa000003", "curve": null, "press": null}"#
                .to_owned(),
            &["sketch"],
        ),
        (
            r#"{"session": "S1", "sketch": "s0000001", "point": "pa000003", "curve": null, "press": null}"#
                .to_owned(),
            &["session"],
        ),
    ];
    for (arguments, want) in cases {
        let envelope = f.call("drag_begin", &arguments);
        let got = wire_errors(&envelope);
        let parameters: Vec<&str> = got.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(parameters, want, "{arguments}: {envelope}");
        assert!(!f.open(), "{arguments}");
    }
    f.still_works();
}

// ---------------------------------------------------------------------------------------
// A random sequence of commands, against a model of the drag's state.
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Begin(&'static str),
    Edge([f64; 2]),
    Move([f64; 2]),
    End(Option<[f64; 2]>),
    Rename,
    Undo,
    CancelMove,
}

fn cursor() -> impl Strategy<Value = [f64; 2]> {
    let coordinate = prop_oneof![
        Just(60.0),
        Just(5.0),
        Just(0.0),
        Just(-0.0),
        // Near the reach's edge, but not so near that the fake's frame passes it (its nudge
        // of a thousandth of where a point began, an edge's offset from a press near it):
        // a frame past the reach is the subject of its own test below.
        Just(999_000.0),
        Just(-999_000.0),
        Just(1e6 + 1.0),
        Just(f64::NAN),
        Just(f64::INFINITY),
        Just(f64::MIN_POSITIVE),
        -100.0..100.0f64,
    ];
    (coordinate.clone(), coordinate).prop_map(|(x, y)| [x, y])
}

/// A press near the line it's on, or one the drag refuses.
fn press() -> impl Strategy<Value = [f64; 2]> {
    let coordinate = prop_oneof![
        Just(5.0),
        Just(-0.0),
        Just(1e6 + 1.0),
        Just(f64::NAN),
        Just(f64::NEG_INFINITY),
        -100.0..100.0f64,
    ];
    (coordinate.clone(), coordinate).prop_map(|(x, y)| [x, y])
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => prop_oneof![
            Just("pa000003"),
            Just("pa000004"),
            Just("pa000001"),
            Just("pa00000z"),
        ]
        .prop_map(Op::Begin),
        1 => press().prop_map(Op::Edge),
        5 => cursor().prop_map(Op::Move),
        2 => proptest::option::of(cursor()).prop_map(Op::End),
        1 => Just(Op::Rename),
        1 => Just(Op::Undo),
        1 => Just(Op::CancelMove),
    ]
}

/// A frame past the reach, refused as the worker's fault.
fn past_reach(problems: &silico_core::command::Problems) -> bool {
    let error = &problems.errors()[0];
    error.code() == ErrorCode::ProtocolError && error.message().ends_with("out of reach")
}

fn reachable(to: [f64; 2]) -> bool {
    to.iter().all(|v| v.is_finite() && v.abs() <= 1e6)
}

/// Runs `ops`, checking each against the model: the file's text at the end.
fn run(ops: &[Op]) -> String {
    let f = opened(&profile());
    // Inside a transaction, so undo undoes.
    answer(f.sessions.begin(&s1())).result.unwrap();
    // The model: the drag open, and where its point began; the edits not undone.
    let mut open: Option<[f64; 2]> = None;
    let mut undoable = 0usize;
    let starts = |id: &str| match id {
        "pa000003" => [60.0, 5.0],
        "pa000004" => [5.0, 5.0],
        _ => [0.0, 0.0],
    };
    for (at, op) in ops.iter().enumerate() {
        let before = f.revision();
        match op {
            Op::Begin(point) => {
                let begun = f.begin_point(point);
                match (open, *point) {
                    (Some(_), _) => {
                        assert_eq!(the_error(&begun).parameter(), Some("session"), "{at}")
                    }
                    (None, "pa00000z") => {
                        assert_eq!(the_error(&begun).parameter(), Some("point"), "{at}")
                    }
                    (None, point) => {
                        begun.result.unwrap();
                        open = Some(starts(point));
                    }
                }
            }
            Op::Edge(press) => {
                let begun = answer(f.sessions.drag_begin(
                    &s1(),
                    fid("s0000001"),
                    DragGrab::Edge {
                        curve: eid("na000003"),
                        press: *press,
                    },
                ));
                match open {
                    Some(_) => {
                        assert_eq!(the_error(&begun).parameter(), Some("session"), "{at}")
                    }
                    None if !reachable(*press) => {
                        assert_eq!(the_error(&begun).parameter(), Some("press"), "{at}")
                    }
                    None => {
                        begun.result.unwrap();
                        open = Some(*press);
                    }
                }
            }
            Op::Move(to) => {
                let moved = f.moved(*to);
                match open {
                    None => assert_eq!(the_error(&moved).parameter(), Some("session"), "{at}"),
                    Some(_) if !reachable(*to) => {
                        assert_eq!(the_error(&moved).parameter(), Some("to"), "{at}")
                    }
                    // Sequential: never dropped. Invalid where the cursor lands a line's end
                    // on its other (the fake's frame isn't checked for that, the drag's is).
                    Some(_) => match &moved.result {
                        Ok(frame) => assert!(
                            matches!(frame.frame, FrameEnd::Solved | FrameEnd::Invalid),
                            "{at}: {frame:?}"
                        ),
                        // The fake's frame past the reach (its nudge of a point far out):
                        // refused as the worker's fault, which ends the drag.
                        Err(problems) => {
                            assert!(past_reach(problems), "{at}: {problems:?}");
                            open = None;
                        }
                    },
                }
            }
            Op::End(to) => {
                let ended = f.ended(*to);
                match (open.take(), to) {
                    (None, _) => {
                        assert_eq!(the_error(&ended).parameter(), Some("session"), "{at}")
                    }
                    (Some(_), Some(to)) if !reachable(*to) => {
                        assert_eq!(the_error(&ended).parameter(), Some("to"), "{at}")
                    }
                    (Some(_), None) => {
                        let ended = ended.result.unwrap();
                        assert_eq!((ended.kept, ended.changed), (Kept::Begin, false), "{at}");
                    }
                    (Some(_), Some(_)) if ended.result.as_ref().is_err_and(past_reach) => {}
                    (Some(origin), Some(to)) => {
                        let ended = ended.result.unwrap();
                        if *to == origin {
                            assert_eq!((ended.kept, ended.changed), (Kept::Begin, false), "{at}");
                        } else if ended.changed {
                            assert_eq!(f.revision(), before.next().unwrap(), "{at}");
                            undoable += 1;
                        }
                    }
                }
            }
            Op::Rename => {
                f.rename(&format!("Profile {at}")).result.unwrap();
                open = None;
                undoable += 1;
            }
            Op::Undo => {
                let undone = answer(f.sessions.undo(&s1(), before, None));
                if undoable > 0 {
                    undone.result.unwrap();
                    open = None;
                    undoable -= 1;
                } else {
                    assert!(undone.result.is_err(), "{at}");
                }
            }
            Op::CancelMove => {
                // Nothing is moving between commands: refused, the drag left as it is.
                let cancelled = f.sessions.cancel(&s1(), SessionCommand::DragMove);
                assert!(cancelled.result.is_err(), "{at}");
            }
        }
        assert_eq!(f.open(), open.is_some(), "{at}: {op:?} in {ops:?}");
        assert!(f.revision() >= before, "{at}");
    }
    // Whatever came before, a drag still begins and ends.
    if open.is_some() {
        f.ended(None).result.unwrap();
    }
    f.begin_point("pa000004").result.unwrap();
    f.ended(None).result.unwrap();
    assert!(!f.open());
    f.text()
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn any_sequence_of_drag_commands_answers_each_as_the_model_says_and_twice_alike(
        ops in proptest::collection::vec(op(), 0..16)
    ) {
        let first = run(&ops);
        let second = run(&ops);
        prop_assert_eq!(first, second);
    }
}

// ---------------------------------------------------------------------------------------
// On PlaneGCS itself: the degenerate drags an agent can ask for.
// ---------------------------------------------------------------------------------------

struct Real {
    _dir: TempDir,
    path: std::path::PathBuf,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn real(text: &str) -> Real {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let sessions = Sessions::with_cache_opener(Arc::new(FakeKernel::new(2).unwrap()), move |c| {
        Cache::with_secret(c, &secret)
    });
    let path = dir.path().join("part.silico");
    std::fs::write(&path, text).unwrap();
    answer(sessions.open(&path, false)).result.unwrap();
    Real {
        _dir: dir,
        path,
        sessions,
    }
}

impl Real {
    fn file(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(&self.path).unwrap()).unwrap()
    }

    fn edge(&self, curve: &str, press: [f64; 2]) -> Response<silico_core::command::DragBegun> {
        answer(self.sessions.drag_begin(
            &s1(),
            fid("s0000001"),
            DragGrab::Edge {
                curve: eid(curve),
                press,
            },
        ))
    }

    fn point(&self, point: &str) -> Response<silico_core::command::DragBegun> {
        answer(
            self.sessions
                .drag_begin(&s1(), fid("s0000001"), DragGrab::Point(eid(point))),
        )
    }

    fn moved(&self, to: [f64; 2]) -> Response<DragFrame> {
        answer(self.sessions.drag_move(&s1(), to))
    }

    fn ended(&self, to: Option<[f64; 2]>) -> Response<silico_core::command::DragEnded> {
        answer(self.sessions.drag_end(&s1(), to))
    }

    /// The file's sketch is one the gate takes: every line with length, every circle a
    /// radius above zero, every number finite.
    fn sound(&self) {
        let file = self.file();
        let sketch = &file["features"][0];
        let at = |id: &Value| {
            let p = sketch["points"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == *id)
                .unwrap();
            (p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap())
        };
        for line in sketch["lines"].as_array().unwrap() {
            let (a, b) = (at(&line["from"]), at(&line["to"]));
            assert!(a != b, "{line}: {file}");
        }
        for circle in sketch["circles"].as_array().unwrap() {
            assert!(circle["radius"].as_f64().unwrap() > 0.0, "{circle}");
        }
        assert!(
            answer(self.sessions.check(
                &s1(),
                silico_core::command::CheckModel::Current { fresh: false }
            ))
            .result
            .is_ok()
        );
    }
}

fn circle_sketch(constraints: &str) -> String {
    document(&[&format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Ring", "plane": "xy", "points": [{{"id": "p0000001", "x": 0, "y": 0, "construction": false}}], "lines": [], "circles": [{{"id": "c0000001", "center": "p0000001", "radius": 10, "construction": false}}], "arcs": [], "constraints": [{constraints}]}}"#
    )])
}

/// The circle's centre and radius in `frame`: the centre `(0, 0)` and the radius 10, as
/// they began, where the frame doesn't hold them (not in the part it moves).
fn circle_of(frame: &DragFrame) -> ([f64; 2], f64) {
    let centre = frame
        .points
        .iter()
        .find(|(id, _)| id.as_str() == "p0000001")
        .map_or([0.0, 0.0], |p| p.1);
    let radius = frame
        .radii
        .iter()
        .find(|(id, _)| id.as_str() == "c0000001")
        .map_or(10.0, |r| r.1);
    (centre, radius)
}

/// The centre fixed at the origin: only the radius free.
const CENTRED: &str = r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#;

#[test]
fn planegcs_a_circles_edge_pressed_at_its_centre_follows_the_cursor() {
    let r = real(&circle_sketch(CENTRED));
    r.edge("c0000001", [0.0, 0.0]).result.unwrap();
    let frame = r.moved([5.0, 0.0]).result.unwrap();
    assert_eq!(frame.frame, FrameEnd::Solved, "{frame:?}");
    let (centre, radius) = circle_of(&frame);
    // The edge through the cursor: |centre - cursor| = radius.
    assert!(
        (((centre[0] - 5.0) * (centre[0] - 5.0) + centre[1] * centre[1]).sqrt() - radius).abs()
            < 1e-6,
        "{frame:?}"
    );
    let ended = r.ended(Some([5.0, 0.0])).result.unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    let radius = r.file()["features"][0]["circles"][0]["radius"]
        .as_f64()
        .unwrap();
    assert!((radius - 5.0).abs() < 1e-6, "{radius}");
    r.sound();
}

#[test]
fn planegcs_a_circles_edge_dragged_to_its_centre_is_never_saved_with_no_radius() {
    let r = real(&circle_sketch(CENTRED));
    r.edge("c0000001", [10.0, 0.0]).result.unwrap();
    let frame = r.moved([0.0, 0.0]).result.unwrap();
    // A circle of no radius can't be built: never a solved frame of radius 0.
    if frame.frame == FrameEnd::Solved {
        assert!(circle_of(&frame).1 > 0.0, "{frame:?}");
    }
    let ended = r.ended(Some([0.0, 0.0]));
    match &ended.result {
        Ok(ended) => {
            if ended.changed {
                r.sound();
            }
        }
        Err(problems) => {
            let error = &problems.errors()[0];
            assert!(error.suggestion().is_some(), "{error:?}");
        }
    }
    r.sound();
}

#[test]
fn planegcs_a_release_at_an_off_curve_press_keeps_what_the_frame_there_showed() {
    // An agent's press needn't be on the curve. A move to it puts the circle's edge there;
    // a release at the same cursor should keep that, as a release at a newest frame's
    // cursor takes its values.
    let r = real(&circle_sketch(CENTRED));
    r.edge("c0000001", [30.0, 40.0]).result.unwrap();
    let frame = r.moved([30.0, 40.0]).result.unwrap();
    assert_eq!(frame.frame, FrameEnd::Solved, "{frame:?}");
    let (_, radius) = circle_of(&frame);
    // The edge through (30, 40): radius 50.
    assert!((radius - 50.0).abs() < 1e-6, "{frame:?}");
    let ended = r.ended(Some([30.0, 40.0])).result.unwrap();
    // BREAKS (or a spec question: should a press off its curve be refused?): the release at the press is taken as a click (`to != origin`, the origin of
    // an edge's drag being its press), so the begin state is kept, radius 10, though the
    // newest frame, at that same cursor, showed radius 50.
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    let saved = r.file()["features"][0]["circles"][0]["radius"]
        .as_f64()
        .unwrap();
    assert!((saved - 50.0).abs() < 1e-6, "{saved}");
}

#[test]
fn planegcs_a_lines_end_dragged_onto_its_other_end_never_saves_a_line_of_no_length() {
    let r = real(&document(&[
        r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Stick", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}]}"#,
    ]));
    r.point("p0000002").result.unwrap();
    let frame = r.moved([0.0, 0.0]).result.unwrap();
    if frame.frame == FrameEnd::Solved {
        let p = frame
            .points
            .iter()
            .find(|(id, _)| id.as_str() == "p0000002")
            .unwrap()
            .1;
        assert!(
            p != [0.0, 0.0],
            "a solved frame of a line of no length: {frame:?}"
        );
    }
    let ended = r.ended(Some([0.0, 0.0]));
    match &ended.result {
        Ok(ended) => assert!(!ended.changed || ended.kept == Kept::Cursor),
        Err(problems) => assert!(problems.errors()[0].suggestion().is_some()),
    }
    r.sound();
}

#[test]
fn planegcs_a_point_with_no_freedom_is_dragged_and_released_with_nothing_changed() {
    let r = real(&circle_sketch(CENTRED));
    let before = r.file();
    r.point("p0000001").result.unwrap();
    let frame = r.moved([20.0, 20.0]).result.unwrap();
    // Held at the origin by its two distances, whatever the cursor.
    if frame.frame == FrameEnd::Solved {
        let (centre, radius) = circle_of(&frame);
        assert!(
            centre[0].abs() < 1e-9 && centre[1].abs() < 1e-9,
            "{frame:?}"
        );
        assert!((radius - 10.0).abs() < 1e-9, "{frame:?}");
    }
    let ended = r.ended(Some([20.0, 20.0])).result.unwrap();
    assert!(!ended.changed, "{ended:?}");
    assert_eq!(r.file(), before);
}

#[test]
fn planegcs_a_reachable_cursor_whose_solve_leaves_reach_is_a_frame_not_a_worker_fault() {
    // A horizontal line 100 long: its start dragged to x = 999 990 (within reach) puts its
    // end at 1 000 090, past it. That's the sketch's answer, not the worker's fault.
    let r = real(&document(&[
        r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Stick", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 100, "y": 0, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "k0000001", "type": "length", "line": "n0000001", "value": 100, "driving": true}, {"id": "k0000002", "type": "horizontal", "line": "n0000001"}]}"#,
    ]));
    r.point("p0000001").result.unwrap();
    let moved = r.moved([999_990.0, 0.0]);
    // A reachable cursor the sketch can't follow there is a frame that didn't solve (or
    // `to` refused), the drag going on: not a worker's fault.
    match &moved.result {
        Ok(frame) => assert_ne!(frame.frame, FrameEnd::Solved, "{frame:?}"),
        Err(problems) => {
            let error = &problems.errors()[0];
            assert_eq!(
                (error.code(), error.parameter()),
                (ErrorCode::InvalidInput, Some("to")),
                "{error:?}"
            );
        }
    }
    let back = r.moved([5.0, 0.0]).result.unwrap();
    assert_eq!(back.frame, FrameEnd::Solved, "{back:?}");
    r.ended(None).result.unwrap();
}

#[test]
fn planegcs_a_lines_edge_dragged_so_its_far_end_leaves_reach_is_a_frame_not_a_worker_fault() {
    // A free line 100 long pressed at its start and dragged whole to x = 999 950 (within
    // reach): its end's target, the cursor's offset from the press, is 1 000 050, past it.
    let r = real(&document(&[
        r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Stick", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 100, "y": 0, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "k0000001", "type": "length", "line": "n0000001", "value": 100, "driving": true}]}"#,
    ]));
    r.edge("n0000001", [0.0, 0.0]).result.unwrap();
    let moved = r.moved([999_950.0, 0.0]);
    // A reachable cursor the sketch can't follow there is a frame that didn't solve, or `to`
    // refused, and the drag goes on: never `protocol_error` (drag.rs `checked_frame`), the
    // worker restarted and the drag ended as if PlaneGCS had misbehaved.
    match &moved.result {
        Ok(frame) => assert_ne!(frame.frame, FrameEnd::Solved, "{frame:?}"),
        Err(problems) => {
            let error = &problems.errors()[0];
            assert_eq!(
                (error.code(), error.parameter()),
                (ErrorCode::InvalidInput, Some("to")),
                "{error:?}"
            );
        }
    }
    let back = r.moved([5.0, 0.0]).result.unwrap();
    assert_eq!(back.frame, FrameEnd::Solved, "{back:?}");
    // And a release there: the begin kept or `to` refused, never a file past the reach.
    let ended = r.ended(Some([999_950.0, 0.0]));
    if let Ok(ended) = &ended.result {
        assert!(!ended.changed, "{ended:?}");
    }
}

#[test]
fn planegcs_a_drag_to_the_edge_of_reach_keeps_every_point_within_it_or_refuses() {
    // A free line 100 long, its end dragged to the edge of the reach: the other end held
    // 100 away, past it.
    let r = real(&document(&[
        r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Stick", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 100, "y": 0, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "k0000001", "type": "length", "line": "n0000001", "value": 100, "driving": true}, {"id": "k0000002", "type": "horizontal", "line": "n0000001"}]}"#,
    ]));
    r.point("p0000001").result.unwrap();
    // Dragging p1 to x = -1e6 puts p2 at about -1e6 + 100 (inside), but p1 at x = 1e6 puts
    // p2 at 1e6 + 100, out of reach.
    let ended = r.ended(Some([1e6, 0.0]));
    match &ended.result {
        Ok(ended) if ended.changed => {
            let file = r.file();
            for p in file["features"][0]["points"].as_array().unwrap() {
                assert!(p["x"].as_f64().unwrap().abs() <= 1e6, "{p}");
            }
        }
        Ok(_) => {}
        Err(problems) => {
            let error = &problems.errors()[0];
            assert!(error.suggestion().is_some(), "{error:?}");
            assert_eq!(error.feature(), Some("s0000001"), "{error:?}");
        }
    }
    // Whatever came of it, the next drag works.
    r.point("p0000001").result.unwrap();
    assert_eq!(r.moved([5.0, 0.0]).result.unwrap().frame, FrameEnd::Solved);
    r.ended(None).result.unwrap();
}

// ---------------------------------------------------------------------------------------
// A burst of moves, other threads' reads, undo, and a crash mid-drag.
// ---------------------------------------------------------------------------------------

fn moves_sent(sketches: &FakeSketches) -> Vec<[f64; 2]> {
    sketches
        .drag_calls()
        .into_iter()
        .filter_map(|call| match call {
            DragCall::Move(to) => Some(to),
            _ => None,
        })
        .collect()
}

#[test]
fn a_burst_of_10000_moves_behind_a_held_one_solves_only_the_newest_and_the_release_ignores_them() {
    // The reference: the same begin and release with no moves at all (A5).
    let reference = opened(&profile());
    reference.begin_point("pa000003").result.unwrap();
    reference.ended(Some([62.5, 7.0])).result.unwrap();

    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let first = f.sessions.drag_move(&s1(), [70.0, 9.0]);
    assert!(f.sketches.wait_held(HANG));
    let burst: Vec<_> = (0..10_000)
        .map(|i| {
            f.sessions
                .drag_move(&s1(), [40.0 + f64::from(i) / 1000.0, 3.0])
        })
        .collect();
    // While the burst waits, status says the drag is open, of the point it began on.
    let open = f.sessions.status(&s1()).result.unwrap().drag.unwrap();
    assert_eq!(open.grab, DragGrab::Point(eid("pa000003")));
    f.sketches.release();
    assert_eq!(answer(first).result.unwrap().frame, FrameEnd::Solved);
    let last = burst.len() - 1;
    for (i, moving) in burst.into_iter().enumerate() {
        let frame = answer(moving).result.unwrap();
        let expected = if i == last {
            FrameEnd::Solved
        } else {
            FrameEnd::Dropped
        };
        assert_eq!(frame.frame, expected, "move {i}: {frame:?}");
    }
    // The worker was sent the held move and the newest, nothing between.
    assert_eq!(moves_sent(&f.sketches), [[70.0, 9.0], [49.999, 3.0]]);
    let ended = f.ended(Some([62.5, 7.0])).result.unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    assert!(!f.open());
    assert_eq!(f.file(), reference.file());
}

#[test]
fn status_from_another_thread_while_a_move_is_held_answers_and_names_the_drag() {
    let f = Arc::new(opened(&profile()));
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    let (send, receive) = std::sync::mpsc::channel();
    let other = Arc::clone(&f);
    std::thread::spawn(move || {
        let _ = send.send(other.sessions.status(&s1()));
    });
    let status = receive
        .recv_timeout(HANG)
        .unwrap_or_else(|_| panic!("status waited on the held move"));
    let drag = status.result.unwrap().drag.unwrap();
    assert_eq!(
        (drag.sketch, drag.grab),
        (fid("s0000001"), DragGrab::Point(eid("pa000003")))
    );
    f.sketches.release();
    assert_eq!(answer(moving).result.unwrap().frame, FrameEnd::Solved);
    assert!(f.open());
    f.still_works();
}

#[test]
fn an_undo_while_a_move_is_held_ends_the_drag_and_answers_the_move() {
    let f = opened(&profile_and_block());
    answer(f.sessions.begin(&s1())).result.unwrap();
    let renamed = answer(f.sessions.set_field(
        &s1(),
        Revision::OPENED,
        fid("b0000001"),
        "name".to_owned(),
        read(br#""Renamed""#).unwrap(),
    ));
    renamed.result.unwrap();
    let edited = f.revision();
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    answer(f.sessions.undo(&s1(), edited, None)).result.unwrap();
    // The undo is a new revision: the drag has ended by the time it answers.
    let moved = moving
        .try_take()
        .unwrap_or_else(|_| panic!("the move hadn't answered when the undo did"));
    assert!(moved.result.is_err(), "{:?}", moved.result);
    f.sketches.release();
    assert!(!f.open());
    assert_eq!(
        the_error(&f.ended(Some([62.5, 7.0]))).message(),
        "no drag is open in session s1"
    );
    f.still_works();
}

#[test]
fn a_crash_in_the_releases_move_keeps_the_begin_state_and_ends_the_drag() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches
        .answer_moves(|_, _| Err(Error::new(ErrorCode::WorkerCrashed, "the fake crashed")));
    let ended = f.ended(Some([62.5, 7.0]));
    // Either refused with the worker's code or answered as the begin state kept: never an
    // edit, and the drag over either way.
    match &ended.result {
        Ok(ended) => assert_eq!((ended.kept, ended.changed), (Kept::Begin, false)),
        Err(problems) => assert_eq!(
            problems.errors()[0].code(),
            ErrorCode::WorkerCrashed,
            "{problems:?}"
        ),
    }
    assert!(!f.open());
    assert_eq!(f.revision(), Revision::OPENED);
    assert_eq!(f.file(), profile());
    assert_eq!(
        the_error(&f.ended(None)).message(),
        "no drag is open in session s1"
    );
    f.sketches
        .answer_moves(|begin, to| Ok(FakeSketches::dragged(begin, to)));
    f.still_works();
}

#[test]
fn after_close_every_drag_command_is_refused_as_of_no_session() {
    let f = opened(&profile());
    f.begin_point("pa000003").result.unwrap();
    answer(f.sessions.close(&s1())).result.unwrap();
    assert_eq!(
        the_error(&f.moved([1.0, 1.0])).message(),
        "no session s1 is open"
    );
    assert_eq!(the_error(&f.ended(None)).message(), "no session s1 is open");
    assert_eq!(
        the_error(&f.begin_point("pa000003")).message(),
        "no session s1 is open"
    );
    assert_eq!(f.file(), profile());
}
