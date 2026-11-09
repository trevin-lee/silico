//! `sketch_extend` on PlaneGCS (docs/design/m5.0-app.md, section 7, `sketch_extend`; open
//! item 12c): through the sessions' own command called by name, as a transport calls it
//! (`Sessions::call`), on the fake kernel, each sketch solved by this crate's worker.
//!
//! Expected values: the design's examples, byte for byte (each file the design's own block),
//! on the bracket's L profile with the base leg's thickness `ka00000a` removed, as the
//! design's drag example has it; and by hand. With `ka00000a` gone, the leg's outer edge
//! `na000002`, from (60, 0) to (60, 5), is free in length: its end, lengthened 2 mm, goes to
//! (60, 7), and the leg's top, `na000003`, held horizontal, takes `pa000004` up to (5, 7);
//! nothing else is free (the upright's thickness and height, and the base line, are held).
//! Arcs and the next curve on sketches of their own, worked by hand where they're said.

use super::bracket::{P0000001, S0000001, S0000002};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, SessionId, Sessions};
use silico_core::fake::FakeKernel;
use silico_core::sketch::{SketchSolver, SketchWorker};
use tempfile::TempDir;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// Long enough for any command here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(60);

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
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
    let f = Fixture { dir, sessions };
    std::fs::write(f.path(), text).unwrap();
    let opened = f.call(
        "open",
        &json!({"path": f.path().to_str().unwrap(), "create": false}).to_string(),
    );
    assert_eq!(opened["ok"], true, "{opened}");
    f
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

/// The contract's example `name` (`sketch_extend/request`), read and never written.
fn example(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../silico-core/tests/contract/commands/examples")
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn example_value(name: &str) -> Value {
    serde_json::from_str(&example(name)).unwrap()
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("bracket.silico")
    }

    fn call(&self, name: &str, arguments: &str) -> Value {
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

    fn refused(&self, name: &str, arguments: &str) -> Value {
        let envelope = self.call(name, arguments);
        assert_eq!(envelope["ok"], false, "{envelope}");
        envelope["errors"].clone()
    }

    fn revision(&self) -> u64 {
        self.sessions
            .status(&s1())
            .session
            .unwrap()
            .revision
            .number()
    }

    fn text(&self) -> String {
        self.ran(
            "text",
            r#"{"session": "s1", "feature": null, "revision": null}"#,
        )["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Begun and rolled back until the session is at revision `n`, then begun.
    fn inside_at(&self, n: u64) {
        while self.revision() < n {
            self.ran("begin", r#"{"session": "s1"}"#);
            self.ran("rollback", r#"{"session": "s1"}"#);
        }
        assert_eq!(self.revision(), n);
        self.ran("begin", r#"{"session": "s1"}"#);
    }

    /// Point `id` of sketch `sketch` as the session's text holds it.
    fn point(&self, sketch: &str, id: &str) -> [f64; 2] {
        let document: Value = serde_json::from_str(&self.text()).unwrap();
        let feature = document["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == sketch)
            .unwrap()
            .clone();
        let p = feature["points"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()
            .clone();
        [p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap()]
    }

    /// The sketch `id` as the current model solved it, by `sketch`'s read.
    fn read(&self, id: &str) -> Value {
        self.ran(
            "sketch",
            &json!({"session": "s1", "model": "current", "sketch": id}).to_string(),
        )
    }
}

/// The bracket, inside a transaction at 12, with `ka00000a` removed (revision 13): the
/// design's example's state.
fn freed_leg() -> Fixture {
    let f = opened(&document(&[S0000001, P0000001, S0000002]));
    f.inside_at(12);
    let removed = f.ran(
        "sketch_remove",
        r#"{"session": "s1", "revision": 12, "sketch": "s0000001", "members": ["ka00000a"]}"#,
    );
    assert_eq!(removed["removed"], json!(["ka00000a"]));
    assert_eq!(f.revision(), 13);
    f
}

#[test]
fn the_designs_extend_is_its_result_and_its_refusals_are_its_text() {
    let f = freed_leg();
    let text = f.text();
    for (request, refusal) in [
        (example("sketch_extend/request-next-curve"), "refused-no-next-curve"),
        (
            r#"{"session": "s1", "revision": 13, "sketch": "s0000002", "curve": "cb000001", "end": "end", "by": {"type": "length", "length": 1}}"#.to_owned(),
            "refused-not-a-line-or-arc",
        ),
        (
            r#"{"session": "s1", "revision": 13, "sketch": "s0000001", "curve": "na000002", "end": "end", "by": {"type": "length", "length": 0}}"#.to_owned(),
            "refused-zero",
        ),
        (
            r#"{"session": "s1", "revision": 13, "sketch": "s0000001", "curve": "na000002", "end": "end", "by": {"type": "length", "length": -5}}"#.to_owned(),
            "refused-to-nothing",
        ),
        (
            r#"{"session": "s1", "revision": 13, "sketch": "s0000001", "curve": "na000002", "end": "end", "by": {"type": "point", "point": [61, 10]}}"#.to_owned(),
            "refused-off-line",
        ),
        (
            r#"{"session": "s1", "revision": 13, "sketch": "s0000001", "curve": "na000001", "end": "end", "by": {"type": "length", "length": 5}}"#.to_owned(),
            "refused-held",
        ),
    ] {
        assert_eq!(
            f.refused("sketch_extend", &request),
            json!([example_value(&format!("sketch_extend/{refusal}"))]),
            "{refusal}"
        );
        assert_eq!((f.revision(), f.text()), (13, text.clone()), "{refusal}");
    }
    let envelope = f.call("sketch_extend", &example("sketch_extend/request"));
    assert_eq!(
        envelope["result"],
        example_value("sketch_extend/result"),
        "{envelope}"
    );
    assert_eq!(envelope["session"]["revision"], 14);
    // The end written exactly where the request named; the leg's top risen with it.
    assert_eq!(f.point("s0000001", "pa000003"), [60.0, 7.0]);
    let top = f.point("s0000001", "pa000004");
    assert!(
        (top[0] - 5.0).abs() < 1e-9 && (top[1] - 7.0).abs() < 1e-9,
        "{top:?}"
    );
    // The sketch solves as written: as the read has it, the end still at (60, 7).
    let read = f.read("s0000001");
    let solved = read["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "pa000003")
        .unwrap()
        .clone();
    assert_eq!(
        (&solved["x"], &solved["y"]),
        (&json!(60), &json!(7)),
        "{read}"
    );
    assert_eq!(read["state"], "under_defined", "{read}");
}

/// A free line `a` from (0, 0) to (10, 0) and a line `b` across x = 15, from (15, -5) to
/// (15, 5), and an arc `r` about (0, 20), radius 10, from (10, 20) a quarter turn to (0, 30),
/// nothing constraining any of them.
fn free_curves() -> Fixture {
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Free", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000003", "x": 15, "y": -5, "construction": false}, {"id": "p0000004", "x": 15, "y": 5, "construction": false}, {"id": "p0000005", "x": 0, "y": 20, "construction": false}, {"id": "p0000006", "x": 10, "y": 20, "construction": false}, {"id": "p0000007", "x": 0, "y": 30, "construction": false}], "lines": [{"id": "n000000a", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n000000b", "from": "p0000003", "to": "p0000004", "construction": false}], "circles": [], "arcs": [{"id": "r000000a", "center": "p0000005", "start": "p0000006", "end": "p0000007", "construction": false}], "constraints": []}"#;
    let f = opened(&document(&[sketch]));
    f.ran("begin", r#"{"session": "s1"}"#);
    f
}

fn extend(revision: u64, curve: &str, end: &str, by: Value) -> String {
    json!({"session": "s1", "revision": revision, "sketch": "s0000001", "curve": curve, "end": end, "by": by}).to_string()
}

#[test]
fn a_free_line_extends_by_a_length_to_a_point_and_to_the_next_curve_its_end_exactly_there() {
    let f = free_curves();
    // By 2.5: (10, 0) to (12.5, 0), by hand.
    let result = f.ran(
        "sketch_extend",
        &extend(
            1,
            "n000000a",
            "end",
            json!({"type": "length", "length": 2.5}),
        ),
    );
    assert_eq!(
        result,
        json!({"sketch": "s0000001", "added": [], "modified": ["p0000002"], "removed": [], "changed": true, "commit": null})
    );
    assert_eq!(f.point("s0000001", "p0000002"), [12.5, 0.0]);
    // To the next curve: b, crossing y = 0 at x = 15, exactly.
    f.ran(
        "sketch_extend",
        &extend(2, "n000000a", "end", json!({"type": "next_curve"})),
    );
    assert_eq!(f.point("s0000001", "p0000002"), [15.0, 0.0]);
    // Back at its start, to a point on its line: (-3, 0), exactly.
    f.ran(
        "sketch_extend",
        &extend(
            3,
            "n000000a",
            "start",
            json!({"type": "point", "point": [-3, 0]}),
        ),
    );
    assert_eq!(f.point("s0000001", "p0000001"), [-3.0, 0.0]);
    // The line's start, extended to the next curve: nothing lies left along y = 0.
    let refused = f.refused(
        "sketch_extend",
        &extend(4, "n000000a", "start", json!({"type": "next_curve"})),
    );
    assert_eq!(
        refused[0]["message"],
        "line n000000a meets no other curve past its start"
    );
    // An angle for a line, and a length for the arc: the wrong kind.
    let refused = f.refused(
        "sketch_extend",
        &extend(4, "n000000a", "end", json!({"type": "angle", "angle": 10})),
    );
    assert_eq!(
        (&refused[0]["message"], &refused[0]["parameter"]),
        (
            &json!("line n000000a extends by a length, not an angle"),
            &json!("by")
        )
    );
    let refused = f.refused(
        "sketch_extend",
        &extend(4, "r000000a", "end", json!({"type": "length", "length": 1})),
    );
    assert_eq!(
        refused[0]["message"],
        "arc r000000a extends by an angle, not a length"
    );
    // A line c across x = -8, from (-8, -5) to (-8, 5): the line's start, extended to the
    // next curve, now meets it at (-8, 0), exactly; its end stays on b at (15, 0).
    f.ran(
        "sketch_add",
        r#"{"session": "s1", "revision": 4, "sketch": "s0000001", "points": [{"id": "p0000008", "x": -8, "y": -5, "construction": false}, {"id": "p0000009", "x": -8, "y": 5, "construction": false}], "lines": [{"id": "n000000c", "from": "p0000008", "to": "p0000009", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#,
    );
    let result = f.ran(
        "sketch_extend",
        &extend(5, "n000000a", "start", json!({"type": "next_curve"})),
    );
    assert_eq!(result["modified"], json!(["p0000001"]), "{result}");
    assert_eq!(f.point("s0000001", "p0000001"), [-8.0, 0.0]);
    assert_eq!(f.point("s0000001", "p0000002"), [15.0, 0.0]);
}

#[test]
fn a_point_short_of_the_end_shortens_the_line_and_the_arc_its_end_exactly_there() {
    // a from (0, 0) to (10, 0), its end to (4, 0): 4 mm long. r about (0, 20), radius 10,
    // from 0° to 90°: its end to (6, 28) (6² + 8² = 10², about 53.13°), and its start to
    // (8, 26) (8² + 6² = 10², about 36.87°), each short of where it was. By hand.
    let f = free_curves();
    let result = f.ran(
        "sketch_extend",
        &extend(
            1,
            "n000000a",
            "end",
            json!({"type": "point", "point": [4, 0]}),
        ),
    );
    assert_eq!(
        result,
        json!({"sketch": "s0000001", "added": [], "modified": ["p0000002"], "removed": [], "changed": true, "commit": null})
    );
    assert_eq!(f.point("s0000001", "p0000002"), [4.0, 0.0]);
    assert_eq!(f.point("s0000001", "p0000001"), [0.0, 0.0]);
    let result = f.ran(
        "sketch_extend",
        &extend(
            2,
            "r000000a",
            "end",
            json!({"type": "point", "point": [6, 28]}),
        ),
    );
    assert_eq!(result["modified"], json!(["p0000007"]), "{result}");
    assert_eq!(f.point("s0000001", "p0000007"), [6.0, 28.0]);
    let result = f.ran(
        "sketch_extend",
        &extend(
            3,
            "r000000a",
            "start",
            json!({"type": "point", "point": [8, 26]}),
        ),
    );
    assert_eq!(result["modified"], json!(["p0000006"]), "{result}");
    assert_eq!(f.point("s0000001", "p0000006"), [8.0, 26.0]);
    assert_eq!(f.point("s0000001", "p0000005"), [0.0, 20.0]);
}

#[test]
fn an_arc_extends_by_an_angle_its_end_turned_about_its_centre() {
    // r about (0, 20), radius 10, from 0° to 90°: its end on by 90° goes to 180°, (-10, 20);
    // its start back by 90° from there to -90°, (0, 10). Each by hand, to the solver's
    // rounding (the turn is written in floating point, sin and cos of a right angle).
    let f = free_curves();
    let result = f.ran(
        "sketch_extend",
        &extend(1, "r000000a", "end", json!({"type": "angle", "angle": 90})),
    );
    assert_eq!(result["modified"], json!(["p0000007"]));
    let end = f.point("s0000001", "p0000007");
    assert!(
        (end[0] + 10.0).abs() < 1e-12 && (end[1] - 20.0).abs() < 1e-12,
        "{end:?}"
    );
    f.ran(
        "sketch_extend",
        &extend(
            2,
            "r000000a",
            "start",
            json!({"type": "angle", "angle": 90}),
        ),
    );
    let start = f.point("s0000001", "p0000006");
    assert!(
        (start[0]).abs() < 1e-12 && (start[1] - 10.0).abs() < 1e-12,
        "{start:?}"
    );
    // A turn of 270° from a half turn's sweep: past a whole turn.
    let refused = f.refused(
        "sketch_extend",
        &extend(3, "r000000a", "end", json!({"type": "angle", "angle": 270})),
    );
    assert!(
        refused[0]["message"]
            .as_str()
            .unwrap()
            .contains("a whole turn or more"),
        "{refused}"
    );
    // To the next curve, around the circle: from 180° (its end at (-10, 20)) on towards
    // 270°, nothing on its circle is met.
    let refused = f.refused(
        "sketch_extend",
        &extend(3, "r000000a", "end", json!({"type": "next_curve"})),
    );
    assert_eq!(
        refused[0]["message"],
        "arc r000000a meets no other curve past its end"
    );
    assert_eq!(
        refused[0]["suggestion"],
        "extend it by an angle, or to a point on its circle"
    );
}

#[test]
fn an_extend_to_a_neighbours_end_lands_on_it_and_fmt_merge_touching_can_join_them() {
    // #299's behaviour, through the command: a free line along y = 0 extended to the next
    // curve, b, which starts on its line at (18.066, 0) (Worker 2's value): the end lands on
    // b's start's stored coordinates bit for bit, two points at one place, which check warns
    // of as ends_not_joined and a merge joins.
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Free", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000003", "x": 18.066, "y": 0, "construction": false}, {"id": "p0000004", "x": 18.066, "y": 5, "construction": false}], "lines": [{"id": "n000000a", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n000000b", "from": "p0000003", "to": "p0000004", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
    let f = opened(&document(&[sketch]));
    f.ran("begin", r#"{"session": "s1"}"#);
    f.ran(
        "sketch_extend",
        &extend(1, "n000000a", "end", json!({"type": "next_curve"})),
    );
    assert_eq!(f.point("s0000001", "p0000002"), [18.066, 0.0]);
    let checked = f.ran(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    let warned: Vec<&Value> = checked["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "ends_not_joined")
        .collect();
    assert_eq!(warned.len(), 1, "{checked}");
    assert_eq!(warned[0]["elements"], json!(["p0000002", "p0000003"]));
    let merged = f.ran(
        "sketch_merge",
        r#"{"session": "s1", "revision": 2, "sketch": "s0000001", "points": ["p0000002", "p0000003"]}"#,
    );
    assert_eq!(merged["removed"], json!(["p0000003"]));
}

#[test]
fn a_lines_extend_in_a_sketch_whose_constraints_conflict_is_refused_as_a_drag_of_one_is_and_an_arcs_is_made()
 {
    // The over-defined L profile (a second length of the base line, 61 where ka000009
    // holds 60): the line's end moves through a drag, which begins only from a sketch whose
    // constraints agree (FreeCAD refuses to drag one with conflicts), so the move finds the
    // conflict and is refused naming the two, inside a transaction and out. (The judgment
    // of the edit's solve, where a conflict the sketch had isn't the edit's, never comes.)
    // An arc's end isn't moved by a drag, so its extend is judged by that rule, and made.
    for inside in [true, false] {
        let f = opened(&document(&[S0000001, P0000001, S0000002]));
        if inside {
            f.ran("begin", r#"{"session": "s1"}"#);
        }
        let at = f.revision();
        // An arc too, about (100, 100), radius 10, from (110, 100) a quarter turn to
        // (100, 110), free.
        let added = f.call(
            "sketch_add",
            &format!(
                r#"{{"session": "s1", "revision": {at}, "sketch": "s0000001", "points": [{{"id": "pa000007", "x": 100, "y": 100, "construction": false}}, {{"id": "pa000008", "x": 110, "y": 100, "construction": false}}, {{"id": "pa000009", "x": 100, "y": 110, "construction": false}}], "lines": [], "circles": [], "arcs": [{{"id": "ra000001", "center": "pa000007", "start": "pa000008", "end": "pa000009", "construction": false}}], "constraints": [{{"id": "ka00000d", "type": "length", "line": "na000001", "value": 61, "driving": true}}]}}"#
            ),
        );
        assert_eq!(added["ok"], true, "{added}");
        let at = f.revision();
        let text = f.text();
        let file = std::fs::read(f.path()).unwrap();
        let refused = f.refused(
            "sketch_extend",
            &json!({"session": "s1", "revision": at, "sketch": "s0000001", "curve": "na000002", "end": "end", "by": {"type": "length", "length": 2}}).to_string(),
        );
        assert_eq!(
            refused,
            json!([{"code": "over_constrained", "message": "the sketch's constraints conflict, so line na000002 can't be extended", "feature": "s0000001", "elements": ["ka000009", "ka00000d"], "elements_total": 2, "parameter": null, "cause": null, "suggestion": "remove or change one of the constraints named", "candidates": [], "ranking": [], "advice": null}]),
            "inside: {inside}"
        );
        assert_eq!((f.revision(), f.text()), (at, text));
        assert_eq!(std::fs::read(f.path()).unwrap(), file);
        // The arc's end, turned on 90° in the same sketch, is made: its end written at
        // (90, 100) (by hand, to the turn's rounding) and a new revision, since the
        // conflict the sketch had already isn't the edit's (the common rules).
        let result = f.ran(
            "sketch_extend",
            &json!({"session": "s1", "revision": at, "sketch": "s0000001", "curve": "ra000001", "end": "end", "by": {"type": "angle", "angle": 90}}).to_string(),
        );
        assert_eq!(result["modified"], json!(["pa000009"]), "{result}");
        assert_eq!(result["changed"], true);
        assert_eq!(result["commit"].is_null(), inside, "{result}");
        assert_eq!(f.revision(), at + 1);
        let end = f.point("s0000001", "pa000009");
        assert!(
            (end[0] - 90.0).abs() < 1e-12 && (end[1] - 100.0).abs() < 1e-12,
            "inside {inside}: {end:?}"
        );
    }
}
