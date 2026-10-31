//! break-it for `sketch_split`, `sketch_trim`, and `sketch_merge` on PlaneGCS
//! (docs/design/m5.0-app.md, section 7, *The sketch edits*): through the sessions' own
//! command called by name, on the fake kernel, each sketch solved by this crate's worker.
//! What an edit writes must be what the sketch then solves to, so the file it saves reads
//! back with no drift: a merge whose kept point leaves a constraint of the other point's
//! line off by 4e-8 mm, which the solve then meets by moving a point; a split of an arc at a
//! position whose projection is irrational, its new point on the arc only to rounding.
//! Expected values by hand: a horizontal line is horizontal (both ends at one y), and a
//! sketch that solves to its file's values has no `drift` problem (M3.4).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, Sessions};
use silico_core::fake::FakeKernel;
use silico_core::sketch::{SketchSolver, SketchWorker};
use tempfile::TempDir;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

const HANG: Duration = Duration::from_secs(60);

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
    /// The open session's ID, as `open` gave it.
    session: std::cell::RefCell<String>,
}

fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = Sessions::with_stand_ins(
        kernel,
        |cache: &Path| Cache::unsealed(cache),
        || {
            SketchWorker::start_worker(Path::new(WORKER))
                .map(|worker| Box::new(worker) as Box<dyn SketchSolver>)
        },
    );
    let f = Fixture {
        dir,
        sessions,
        session: std::cell::RefCell::new(String::new()),
    };
    std::fs::write(f.path(), text).unwrap();
    f.open();
    f
}

fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("part.silico")
    }

    fn open(&self) {
        let opened = self.call(
            "open",
            &json!({"path": self.path().to_str().unwrap(), "create": false}).to_string(),
        );
        assert_eq!(opened["ok"], true, "{opened}");
        *self.session.borrow_mut() = opened["result"]["session"]["id"]
            .as_str()
            .or_else(|| opened["session"]["id"].as_str())
            .unwrap()
            .to_owned();
    }

    /// `arguments` with the open session's ID for its `session`.
    fn with_session(&self, arguments: &str) -> String {
        let mut value: Value = serde_json::from_str(arguments).unwrap();
        if value.get("session").is_some() {
            value["session"] = json!(self.session.borrow().as_str());
        }
        value.to_string()
    }

    fn close(&self) {
        let closed = self.call("close", r#"{"session": "s1"}"#);
        assert_eq!(closed["ok"], true, "{closed}");
    }

    fn call(&self, name: &str, arguments: &str) -> Value {
        let arguments = &self.with_session(arguments);
        let answer: Answered = match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}")),
            Call::Unknown => panic!("no command {name}"),
        };
        let envelope: Value = serde_json::from_str(&answer.text).unwrap();
        assert_eq!(envelope["ok"], answer.ok, "{}", answer.text);
        envelope
    }

    fn ran(&self, name: &str, arguments: &str) -> Value {
        let envelope = self.call(name, arguments);
        assert_eq!(envelope["ok"], true, "{envelope}");
        envelope["result"].clone()
    }

    fn problems(&self) -> Value {
        self.ran(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        )["problems"]
            .clone()
    }

    /// The sketch `s0000001` as the file on disk holds it.
    fn saved_sketch(&self) -> Value {
        let text = std::fs::read_to_string(self.path()).unwrap();
        let document: Value = serde_json::from_str(&text).unwrap();
        document["features"][0].clone()
    }
}

fn member<'a>(sketch: &'a Value, list: &str, id: &str) -> &'a Value {
    sketch[list]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {list}: {sketch}"))
}

/// A vertical line `na` from pa000001 (0, 0) to pa000002 (0, 10), its start locked at the
/// origin and its length 10; and a horizontal line `nb` from pb000001 (0, 10.00000004),
/// 4e-8 mm above pa000002, to pb000002 (20, 10.00000004): every constraint met exactly as
/// the file holds it, and the two ends one cluster ends_not_joined warns of.
fn touching_corner() -> String {
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Corner", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 0, "y": 10, "construction": false}, {"id": "pb000001", "x": 0, "y": 10.00000004, "construction": false}, {"id": "pb000002", "x": 20, "y": 10.00000004, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000002", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000003", "type": "vertical", "line": "na000001"}, {"id": "ka000004", "type": "length", "line": "na000001", "value": 10, "driving": true}, {"id": "kb000001", "type": "horizontal", "line": "nb000001"}]}"#;
    document(&[sketch])
}

#[test]
fn a_merge_whose_kept_point_leaves_a_line_off_by_4e_8_saves_what_the_sketch_solves_to() {
    // Merged keeping pa000002 (0, 10): nb then runs from (0, 10) to (20, 10.00000004), off
    // horizontal by 4e-8 mm, so the solve moves pb000002 to y = 10. What the commit saves
    // must be that solve: reopened, the sketch has no drift, and nb is horizontal in the
    // file.
    let f = opened(&touching_corner());
    assert_eq!(f.problems(), json!([]));
    let merged = f.ran(
        "sketch_merge",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "points": ["pa000002", "pb000001"]}).to_string(),
    );
    assert_eq!(merged["removed"], json!(["pb000001"]), "{merged}");
    assert_eq!(merged["commit"]["written"], true, "{merged}");
    assert_eq!(f.problems(), json!([]));
    f.close();
    f.open();
    assert_eq!(f.problems(), json!([]), "reopened");
    let saved = f.saved_sketch();
    assert_eq!(member(&saved, "points", "pa000002")["y"], json!(10));
    assert_eq!(
        member(&saved, "points", "pb000002")["y"],
        member(&saved, "points", "pa000002")["y"],
        "{saved}"
    );
}

#[test]
fn a_merge_inside_a_transaction_committed_saves_what_the_sketch_solves_to() {
    let f = opened(&touching_corner());
    f.ran("begin", r#"{"session": "s1"}"#);
    f.ran(
        "sketch_merge",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "points": ["pa000002", "pb000001"]}).to_string(),
    );
    f.ran(
        "commit",
        r#"{"session": "s1", "path": null, "replace": false}"#,
    );
    f.close();
    f.open();
    assert_eq!(f.problems(), json!([]), "reopened");
}

/// An arc `aa000001` about pa000001 at the origin, from pa000002 (radius, 0) to pa000003
/// at `angle` radians, of `radius`, its end's coordinates rounded as f64 holds them: the
/// sketch is free, so it solves where it's drawn.
fn arc_sketch(radius: f64, angle: f64) -> String {
    let (x, y) = (radius * libm::cos(angle), radius * libm::sin(angle));
    let sketch = format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Arc", "plane": "xy", "points": [{{"id": "pa000001", "x": 0, "y": 0, "construction": false}}, {{"id": "pa000002", "x": {radius:?}, "y": 0, "construction": false}}, {{"id": "pa000003", "x": {x:?}, "y": {y:?}, "construction": false}}], "lines": [], "circles": [], "arcs": [{{"id": "aa000001", "center": "pa000001", "start": "pa000002", "end": "pa000003", "construction": false}}], "constraints": []}}"#
    );
    document(&[&sketch])
}

#[test]
fn an_arc_split_at_an_irrational_position_saves_what_the_sketch_solves_to() {
    // The new point is the position's projection onto the arc, on it to rounding; its join
    // constraints hold it there. Whatever the solve then does, the file saved is that solve:
    // reopened, no drift. Radii from a micron to a kilometre, angles that aren't round.
    for (radius, angle, at) in [
        (7.0, 1.0, 0.5_f64),
        (1234.5, 2.5, 1.1),
        (1e-3, 1.3, 0.7),
        (1e6, 0.3, 0.123_456_789),
    ] {
        let f = opened(&arc_sketch(radius, angle));
        assert_eq!(f.problems(), json!([]), "{radius} {angle}");
        let position = [radius * libm::cos(at), radius * libm::sin(at)];
        let split = f.call(
            "sketch_split",
            &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "curve": "aa000001", "at": [position], "ids": null}).to_string(),
        );
        assert_eq!(split["ok"], true, "{radius} {angle}: {split}");
        assert_eq!(f.problems(), json!([]), "{radius} {angle}: after the split");
        f.close();
        f.open();
        assert_eq!(f.problems(), json!([]), "{radius} {angle}: reopened");
    }
}

#[test]
fn a_trim_of_a_circle_between_two_lines_saves_what_the_sketch_solves_to() {
    // A circle of radius 5 about the origin, crossed by the x axis' line from (-10, 0) to
    // (10, 0); picked at its top, the upper half goes, the lower arc kept, its ends where
    // the line cuts it, held on the line. Reopened: no drift.
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Cut circle", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": -10, "y": 0, "construction": false}, {"id": "pa000003", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "na000001", "from": "pa000002", "to": "pa000003", "construction": false}], "circles": [{"id": "ca000001", "center": "pa000001", "radius": 5, "construction": false}], "arcs": [], "constraints": []}"#;
    let f = opened(&document(&[sketch]));
    let trimmed = f.call(
        "sketch_trim",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "curve": "ca000001", "pick": [0, 5], "ids": null}).to_string(),
    );
    assert_eq!(trimmed["ok"], true, "{trimmed}");
    assert_eq!(f.problems(), json!([]), "after the trim");
    f.close();
    f.open();
    assert_eq!(f.problems(), json!([]), "reopened");
}

#[test]
fn a_merge_in_a_sketch_already_redundant_elsewhere_is_no_worse_and_is_made() {
    // The touching corner with na000001's vertical given twice (ka000003 and ka000005):
    // the solve before the merge flags one of the two redundant. Merging the corner's ends
    // adds nothing the solve finds redundant or in conflict (by hand: pb000001's two
    // unknowns go, and the shared point is fixed by na's lock, vertical, and length as
    // before; nb's horizontal then fixes pb000002's y), so FreeCAD's rollback keeps it:
    // the same redundancy after as before.
    let text = touching_corner().replacen(
        r#"{"id": "kb000001", "type": "horizontal", "line": "nb000001"}"#,
        r#"{"id": "kb000001", "type": "horizontal", "line": "nb000001"}, {"id": "ka000005", "type": "vertical", "line": "na000001"}"#,
        1,
    );
    let f = opened(&text);
    let checked = f.ran(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    let redundant: Vec<&Value> = checked["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "constraints_redundant")
        .collect();
    assert_eq!(redundant.len(), 1, "{checked}");
    let merged = f.call(
        "sketch_merge",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "points": ["pa000002", "pb000001"]}).to_string(),
    );
    assert_eq!(merged["ok"], true, "before: {}; {merged}", redundant[0]);
}
