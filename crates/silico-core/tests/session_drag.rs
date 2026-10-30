//! `drag_begin`, `drag_move`, and `drag_end` (docs/design/m5.0-app.md, section 7, M5.3;
//! decision 9) as commands: through the sessions' typed methods and their call by name, as a
//! transport calls them (`Sessions::call`), on the fake kernel and the fake sketch solver.
//!
//! Expected values: the design's examples, byte for byte (each file the design's own block,
//! each request read and written back, each result and refusal the session's own); the fake
//! solver's answers, by hand from its rule (a move's dragged point at the cursor, nudged by
//! a thousandth of where it began; a line's ends moved by the cursor's offset from the
//! press, nudged alike), or, for the design's frame, the values the hand calculation gives
//! (`pa000003` held on its vertical line at x = 60, the leg's top following the cursor's y),
//! which silico-mcp's tests/drag_commands.rs checks against PlaneGCS itself; tags by the
//! translation's rule (a constraint's place among the sketch's, sorted by ID, plus one); and
//! the unknowns of a part by hand (two for each free point, one for each free radius). No
//! assertion holds to a wall clock: a move past its time is the fake's answer, and the time
//! it was given is read back.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::wire::drag::{DragBeginRequest, DragEndRequest, DragMoveRequest};
use silico_core::command::wire::{Layout, ToWire, response_text, write};
use silico_core::command::{
    Answered, Call, Cancelled, DragEnded, DragFrame, DragGrab, DragOpen, FrameEnd, Response,
    Revision, SessionCommand, SessionId, Sessions,
};
use silico_core::document::json::{Node, read};
use silico_core::document::{ElementId, FeatureId, Reference};
use silico_core::drag::{DRAG_MOVE_TIMEOUT, Kept};
use silico_core::fake::{DragCall, FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};
use silico_protocol::{Geometry, Moved, PointOf, SolveStatus, Tag};
use tempfile::TempDir;

use crate::bracket::{H0000001, P0000001, S0000001, S0000002, S0000003};

const HANG: std::time::Duration = std::time::Duration::from_secs(60);

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract")
        .join(name)
}

fn committed(name: &str) -> String {
    let path = contract(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The committed example `name` (`drag_move/request`).
fn example(name: &str) -> String {
    committed(&format!("commands/examples/{name}.json"))
}

/// M5.0's design's JSON code blocks between the headings `from` and `to`, in order.
fn design(from: &str, to: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m5.0-app.md");
    let doc = std::fs::read_to_string(&path).unwrap();
    let section = &doc[doc.find(from).unwrap()..doc.find(to).unwrap()];
    section
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

/// `value` as the API writes it, and a newline, as each example file holds it.
fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

fn parsed(text: &str) -> Node {
    read(text.as_bytes()).unwrap()
}

/// The committed snapshot `of`'s `part`, `request` or `result`.
fn schema_of(of: &str, part: &str) -> Value {
    serde_json::from_str::<Value>(&committed(&format!("commands/{of}.json"))).unwrap()[part].clone()
}

/// The error contract's shape, the envelope's `Error`.
fn error_schema() -> Value {
    let envelope: Value = serde_json::from_str(&committed("commands/envelope.json")).unwrap();
    json!({"$defs": envelope["$defs"], "$ref": "#/$defs/Error"})
}

fn holds(schema: &Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: Value = serde_json::from_str(text).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

fn fails(schema: &Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: Value = serde_json::from_str(text).unwrap();
    assert!(!validator.is_valid(&value), "{what} held");
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

fn fid(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn eid(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn revision(n: u64) -> Revision {
    let mut r = Revision::OPENED;
    for _ in 1..n {
        r = r.next().unwrap();
    }
    r
}

/// The command's answer, failing if it hangs.
fn answer<T>(running: silico_core::command::Running<T>) -> Response<T> {
    match running.wait_timeout(HANG) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in {HANG:?}"),
    }
}

/// The one error of `response`.
fn the_error<T: std::fmt::Debug>(response: &Response<T>) -> Error {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{:?}", problems.errors());
    problems.errors()[0].clone()
}

/// The bracket's first five features (M4.9's, as its session added them): the L profile on
/// xz, the pad, the base's holes on xy, the pocket through them, and the upright's holes.
fn bracket() -> String {
    document(&[S0000001, P0000001, S0000002, H0000001, S0000003])
}

/// A document of `features`, each a feature's JSON.
fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

/// A sketch `s0000001` on xy of `points` free points, `p0000001` on, chained by lines,
/// each line's length held (so its ends are one part: an unconstrained line joins nothing in
/// the solver's equations); and, with `circle`, a circle about the last point with the one
/// before held on it, joining its radius to the part. A part of two unknowns a point, and
/// the circle's radius one more.
fn chain(points: usize, circle: bool) -> String {
    let point =
        |n: usize| format!(r#"{{"id": "p{n:07}", "x": {n}, "y": 0, "construction": false}}"#);
    let line = |n: usize| {
        format!(
            r#"{{"id": "n{n:07}", "from": "p{n:07}", "to": "p{:07}", "construction": false}}"#,
            n + 1
        )
    };
    let length = |n: usize| {
        format!(
            r#"{{"id": "k{n:07}", "type": "length", "line": "n{n:07}", "value": 1, "driving": true}}"#
        )
    };
    let mut constraints: Vec<String> = (1..points).map(length).collect();
    let circles = if circle {
        constraints.push(format!(
            r#"{{"id": "k{points:07}", "type": "on", "point": "p{:07}", "curve": "c0000001"}}"#,
            points - 1
        ));
        format!(
            r#"{{"id": "c0000001", "center": "p{points:07}", "radius": 1, "construction": false}}"#
        )
    } else {
        String::new()
    };
    let sketch = format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Chain", "plane": "xy", "points": [{}], "lines": [{}], "circles": [{circles}], "arcs": [], "constraints": [{}]}}"#,
        (1..=points).map(point).collect::<Vec<_>>().join(", "),
        (1..points).map(line).collect::<Vec<_>>().join(", "),
        constraints.join(", ")
    );
    document(&[&sketch])
}

/// Sessions on the fake kernel and one fake sketch solver every worker shares, with `text`
/// open as s1 from `bracket.silico` in a scratch directory.
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
    let path = dir.path().join("bracket.silico");
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
        std::fs::read_to_string(self.dir.path().join("bracket.silico")).unwrap()
    }

    /// The command `name`'s answer to `arguments`, as a transport has it.
    fn call(&self, name: &str, arguments: &str) -> Answered {
        match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}")),
            Call::Unknown => panic!("no command {name}"),
        }
    }

    /// The envelope `name` answers to `arguments`.
    fn envelope(&self, name: &str, arguments: &str) -> Value {
        let answered = self.call(name, arguments);
        let envelope: Value = serde_json::from_str(&answered.text).unwrap();
        assert_eq!(envelope["ok"], answered.ok, "{}", answered.text);
        envelope
    }

    fn revision(&self) -> Revision {
        self.sessions.status(&s1()).session.unwrap().revision
    }

    fn drag(&self) -> Option<DragOpen> {
        self.sessions.status(&s1()).result.unwrap().drag
    }

    fn text(&self) -> String {
        answer(self.sessions.text(&s1(), None)).result.unwrap().text
    }

    /// Begun and rolled back until the session is at revision `n`, then begun: inside a
    /// transaction at `n`, as the design's examples are.
    fn inside_at(&self, n: u64) {
        while self.revision() < revision(n) {
            answer(self.sessions.begin(&s1())).result.unwrap();
            answer(self.sessions.rollback(&s1())).result.unwrap();
        }
        assert_eq!(self.revision(), revision(n));
        answer(self.sessions.begin(&s1())).result.unwrap();
    }

    /// The design's setting: inside a transaction begun at 12, the profile's base leg's
    /// thickness, `ka00000a`, removed, at 13.
    fn freed(&self) {
        self.inside_at(12);
        answer(self.sessions.sketch_remove(
            &s1(),
            revision(12),
            fid("s0000001"),
            vec![Reference::new("ka00000a").unwrap()],
        ))
        .result
        .unwrap();
        assert_eq!(self.revision(), revision(13));
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

    fn ended(&self, to: Option<[f64; 2]>) -> Response<DragEnded> {
        answer(self.sessions.drag_end(&s1(), to))
    }
}

/// The fake's moves answered as PlaneGCS answers the design's drag of `pa000003` (the base
/// leg's free thickness): `pa000003` held on its vertical line, at x = 60, and the leg's
/// top, `pa000003` and `pa000004`, at the cursor's y; every other value as it began. The
/// points are the sketch's own geometries after the origin and the two axes (0 to 2), by ID:
/// `pa000003` the sixth (5), `pa000004` the seventh (6).
fn as_planegcs(fake: &FakeSketches) {
    fake.answer_moves(|begin, to| {
        let mut values = begin.system.values.clone();
        for g in [5, 6] {
            let p = begin
                .system
                .point_values(PointOf::Point(Geometry(g)))
                .unwrap();
            values[p.y.0 as usize] = to[1];
        }
        Ok(Moved {
            status: SolveStatus::Success,
            values: Some(values),
        })
    });
}

/// The design's sections of these commands, each block's file in order.
const SECTIONS: [(&str, &str, &str, &[&str]); 3] = [
    (
        "#### `drag_begin`",
        "#### `drag_move`",
        "drag_begin",
        &[
            "request",
            "request-edge",
            "result",
            "refused-conflict",
            "refused-unknowns",
            "refused-open",
        ],
    ),
    (
        "#### `drag_move`",
        "#### `drag_end`",
        "drag_move",
        &["request", "result", "result-dropped", "refused-no-drag"],
    ),
    (
        "#### `drag_end`",
        "### `meshes`",
        "drag_end",
        &["request", "result-cursor", "result-begin"],
    ),
];

#[test]
fn the_drag_examples_are_the_designs_own_text_and_their_directories_hold_no_other() {
    for (from, to, dir, names) in SECTIONS {
        let blocks = design(from, to);
        assert_eq!(blocks.len(), names.len(), "{dir}");
        for (block, name) in blocks.iter().zip(names) {
            assert_eq!(
                format!("{block}\n"),
                example(&format!("{dir}/{name}")),
                "{dir}/{name}"
            );
        }
        let mut files: Vec<String> =
            std::fs::read_dir(contract(&format!("commands/examples/{dir}")))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
        files.sort();
        let mut want: Vec<String> = names.iter().map(|n| format!("{n}.json")).collect();
        want.sort();
        assert_eq!(files, want, "{dir}");
    }
}

#[test]
fn the_designs_requests_are_read_written_back_byte_for_byte_and_hold_to_their_schemas() {
    let text = example("drag_begin/request");
    let request = DragBeginRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        DragBeginRequest {
            session: s1(),
            sketch: fid("s0000001"),
            grab: DragGrab::Point(eid("pa000003")),
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("drag_begin", "request"), &text, "a point's");

    let text = example("drag_begin/request-edge");
    let request = DragBeginRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request.grab,
        DragGrab::Edge {
            curve: eid("na000001"),
            press: [30.0, 0.0],
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("drag_begin", "request"), &text, "an edge's");

    let text = example("drag_move/request");
    let request = DragMoveRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        DragMoveRequest {
            session: s1(),
            to: [62.5, 7.0],
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("drag_move", "request"), &text, "drag_move");

    let text = example("drag_end/request");
    let request = DragEndRequest::read(&parsed(&text)).unwrap();
    assert_eq!(request.to, Some([62.5, 7.0]));
    assert_eq!(written(&request), text);
    holds(&schema_of("drag_end", "request"), &text, "drag_end");
    // Esc: `to` null, read and written back.
    let esc = r#"{"session": "s1", "to": null}"#;
    let request = DragEndRequest::read(&parsed(esc)).unwrap();
    assert_eq!(request.to, None);
    assert_eq!(written(&request), format!("{esc}\n"));
    holds(&schema_of("drag_end", "request"), esc, "Esc");
}

/// Each refusal of `text` as `read` reads it: its parameter and message, in order.
fn refusals<T: std::fmt::Debug>(
    read: impl Fn(&Node) -> Result<T, silico_core::command::Problems>,
    text: &str,
) -> Vec<(String, String)> {
    read(&parsed(text))
        .unwrap_err()
        .errors()
        .iter()
        .map(|e| {
            assert_eq!(e.code(), ErrorCode::InvalidInput, "{text}");
            (
                e.parameter().unwrap_or("").to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_begin_grabs_a_point_or_a_curve_and_its_press_and_is_refused_otherwise() {
    let begin = |rest: &str| format!(r#"{{"session": "s1", "sketch": "s0000001", {rest}}}"#);
    let said = |rest: &str| refusals(DragBeginRequest::read, &begin(rest));
    let one = |parameter: &str, message: &str| vec![(parameter.to_owned(), message.to_owned())];
    assert_eq!(
        said(r#""point": "pa000003", "curve": "na000001", "press": [1, 2]"#),
        one(
            "curve",
            "the request names both a point and a curve, and a drag grabs one"
        )
    );
    assert_eq!(
        said(r#""point": null, "curve": null, "press": null"#),
        one(
            "point",
            "the request names neither a point nor a curve, and a drag grabs one"
        )
    );
    assert_eq!(
        said(r#""point": "pa000003", "curve": null, "press": [1, 2]"#),
        one(
            "press",
            "press is given with a point, and only a curve's edge is pressed"
        )
    );
    assert_eq!(
        said(r#""point": null, "curve": "na000001", "press": null"#),
        one(
            "press",
            "press is null, and a drag of a curve's edge needs where the press on it landed"
        )
    );
    // Every field required (the design's own shapes of two fields, without the others,
    // aren't M4.1's strict requests).
    assert_eq!(
        said(r#""point": "pa000003""#),
        vec![
            ("curve".to_owned(), "curve is missing".to_owned()),
            ("press".to_owned(), "press is missing".to_owned()),
        ]
    );
    // Shapes: an ID that isn't one, a press of three, one not a number.
    assert_eq!(
        said(r#""point": "PA000003", "curve": null, "press": null"#),
        one(
            "point",
            r#"point must be a sketch member's ID (8 characters), or null, got "PA000003""#
        )
    );
    assert_eq!(
        said(r#""point": null, "curve": "na000001", "press": [1, 2, 3]"#),
        one(
            "press",
            "press must be two numbers [x, y], in the sketch's coordinates, got 3 of them"
        )
    );
    assert_eq!(
        said(r#""point": null, "curve": "na000001", "press": [1, "2"]"#),
        one("press.1", r#"press.1 must be a number, got "2""#)
    );
    // A move's cursor alike.
    assert_eq!(
        refusals(
            DragMoveRequest::read,
            r#"{"session": "s1", "to": [true, 1]}"#
        ),
        one("to.0", "to.0 must be a number, got true or false")
    );
    assert_eq!(
        refusals(DragMoveRequest::read, r#"{"session": "s1", "to": null}"#),
        one(
            "to",
            "to must be two numbers [x, y], in the sketch's coordinates, got null"
        )
    );
    // A number past the largest `f64` is read, as infinite, for the drag's own check.
    let far =
        DragMoveRequest::read(&parsed(r#"{"session": "s1", "to": [1e999, -1e999]}"#)).unwrap();
    assert_eq!(far.to, [f64::INFINITY, f64::NEG_INFINITY]);
    // The schemas hold the same: a request of both, or a press with a point, doesn't hold.
    let schema = schema_of("drag_begin", "request");
    fails(&schema, &begin(r#""point": "pa000003""#), "two fields");
    fails(
        &schema,
        &begin(r#""point": "pa000003", "curve": null, "press": [1]"#),
        "a press of one",
    );
}

#[test]
fn the_designs_results_and_refusals_are_written_as_the_design_gives_them() {
    // Each by hand from the design's values, written, and held to its snapshot.
    let begun = silico_core::command::DragBegun {
        sketch: fid("s0000001"),
        grab: DragGrab::Point(eid("pa000003")),
        dofs: 1,
        moves: [
            "pa000001", "pa000002", "pa000003", "pa000004", "pa000005", "pa000006",
        ]
        .map(eid)
        .to_vec(),
    };
    assert_eq!(written(&begun), example("drag_begin/result"));
    holds(
        &schema_of("drag_begin", "result"),
        &example("drag_begin/result"),
        "begun",
    );
    let frame = DragFrame {
        to: [62.5, 7.0],
        frame: FrameEnd::Solved,
        points: [
            ("pa000001", [0.0, 0.0]),
            ("pa000002", [60.0, 0.0]),
            ("pa000003", [60.0, 7.0]),
            ("pa000004", [5.0, 7.0]),
            ("pa000005", [5.0, 50.0]),
            ("pa000006", [0.0, 50.0]),
        ]
        .map(|(p, at)| (eid(p), at))
        .to_vec(),
        radii: vec![],
    };
    assert_eq!(written(&frame), example("drag_move/result"));
    holds(
        &schema_of("drag_move", "result"),
        &example("drag_move/result"),
        "solved",
    );
    let dropped = DragFrame {
        to: [70.0, 9.0],
        frame: FrameEnd::Dropped,
        points: vec![],
        radii: vec![],
    };
    assert_eq!(written(&dropped), example("drag_move/result-dropped"));
    // Each frame's word, and only those: `cancelled` is an error, never a frame.
    for (end, word) in [
        (FrameEnd::Solved, "solved"),
        (FrameEnd::Failed, "failed"),
        (FrameEnd::Invalid, "invalid"),
        (FrameEnd::Dropped, "dropped"),
    ] {
        let text = written(&DragFrame {
            frame: end,
            ..dropped.clone()
        });
        assert!(text.contains(&format!(r#""frame": "{word}""#)), "{text}");
        holds(&schema_of("drag_move", "result"), &text, word);
    }
    fails(
        &schema_of("drag_move", "result"),
        &example("drag_move/result-dropped").replace("dropped", "cancelled"),
        "cancelled",
    );
    // A radius, as the frame gives one.
    let sized = DragFrame {
        radii: vec![(eid("cb000002"), 3.25)],
        ..dropped.clone()
    };
    let text = written(&sized);
    assert_eq!(
        write(&sized.to_wire(), Layout::OneLine).unwrap(),
        r#"{"to": [70, 9], "frame": "dropped", "points": [], "radii": [{"id": "cb000002", "radius": 3.25}]}"#
    );
    holds(&schema_of("drag_move", "result"), &text, "a radius");
    for (kept, changed, name) in [
        (Kept::Cursor, true, "drag_end/result-cursor"),
        (Kept::Begin, false, "drag_end/result-begin"),
    ] {
        let ended = DragEnded {
            sketch: fid("s0000001"),
            kept,
            changed,
            commit: None,
        };
        assert_eq!(written(&ended), example(name));
        holds(&schema_of("drag_end", "result"), &example(name), name);
    }
    for name in [
        "drag_begin/refused-conflict",
        "drag_begin/refused-unknowns",
        "drag_begin/refused-open",
        "drag_move/refused-no-drag",
    ] {
        holds(&error_schema(), &example(name), name);
    }
}

#[test]
fn a_point_drag_begins_moves_and_ends_as_the_design_gives_it_one_undoable_edit() {
    let f = opened(&bracket());
    f.freed();
    as_planegcs(&f.sketches);
    let before = f.text();
    // The design's begin, by name: its result, byte for byte.
    let begun = f.envelope("drag_begin", &example("drag_begin/request"));
    assert_eq!(begun["ok"], true, "{begun}");
    assert_eq!(
        begun["result"],
        serde_json::from_str::<Value>(&example("drag_begin/result")).unwrap()
    );
    assert_eq!(begun["session"]["revision"], 13);
    // Nothing changed: a begin isn't an edit.
    assert_eq!(f.revision(), revision(13));
    // Open: status says so, written as the design gives it.
    let open = f.drag().unwrap();
    assert_eq!(
        open,
        DragOpen {
            sketch: fid("s0000001"),
            grab: DragGrab::Point(eid("pa000003")),
        }
    );
    assert_eq!(
        write(&open.to_wire(), Layout::OneLine).unwrap(),
        r#"{"sketch": "s0000001", "point": "pa000003", "curve": null, "press": null}"#
    );
    let status = written(&f.sessions.status(&s1()).result.unwrap());
    holds(&schema_of("status", "result"), &status, "a drag open");
    // A second begin: the design's refusal.
    let again = f.call("drag_begin", &example("drag_begin/request"));
    let errors: Value = serde_json::from_str::<Value>(&again.text).unwrap()["errors"].clone();
    assert_eq!(
        errors,
        json!([serde_json::from_str::<Value>(&example("drag_begin/refused-open")).unwrap()])
    );
    // The design's move, its frame byte for byte.
    let moved = f.call("drag_move", &example("drag_move/request"));
    let envelope: Value = serde_json::from_str(&moved.text).unwrap();
    assert_eq!(
        envelope["result"],
        serde_json::from_str::<Value>(&example("drag_move/result")).unwrap(),
        "{}",
        moved.text
    );
    assert_eq!(envelope["session"]["revision"], 13);
    // The design's release, at the same cursor: the frame's values taken, no move run
    // again, and one edit, at 14, inside the transaction.
    let moves = |fake: &FakeSketches| {
        fake.drag_calls()
            .iter()
            .filter(|c| matches!(c, DragCall::Move(_)))
            .count()
    };
    assert_eq!(moves(&f.sketches), 1);
    let ended = f.envelope("drag_end", &example("drag_end/request"));
    assert_eq!(
        ended["result"],
        serde_json::from_str::<Value>(&example("drag_end/result-cursor")).unwrap()
    );
    assert_eq!(moves(&f.sketches), 1);
    assert_eq!(f.revision(), revision(14));
    assert_eq!(ended["session"]["revision"], 14);
    // The sketch's points where the frame put them: pa000003 at (60, 7), pa000004 at
    // (5, 7), the rest as they were.
    let text = f.text();
    let after = |id: &str| {
        let document: Value = serde_json::from_str(&text).unwrap();
        let sketch = &document["features"][0];
        let point = sketch["points"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()
            .clone();
        (point["x"].as_f64().unwrap(), point["y"].as_f64().unwrap())
    };
    assert_eq!(after("pa000003"), (60.0, 7.0));
    assert_eq!(after("pa000004"), (5.0, 7.0));
    assert_eq!(after("pa000002"), (60.0, 0.0));
    assert_eq!(after("pa000005"), (5.0, 50.0));
    assert_eq!(
        text,
        before
            .replace(
                r#""pa000003", "x": 60, "y": 5,"#,
                r#""pa000003", "x": 60, "y": 7,"#
            )
            .replace(
                r#""pa000004", "x": 5, "y": 5,"#,
                r#""pa000004", "x": 5, "y": 7,"#
            )
    );
    // Ended: no drag open, a move refused as the design gives it.
    assert_eq!(f.drag(), None);
    let late = f.call("drag_move", &example("drag_move/request"));
    let errors: Value = serde_json::from_str::<Value>(&late.text).unwrap()["errors"].clone();
    assert_eq!(
        errors,
        json!([serde_json::from_str::<Value>(&example("drag_move/refused-no-drag")).unwrap()])
    );
    // One undoable edit: one undo restores revision 13's text, byte for byte, at 15.
    answer(f.sessions.undo(&s1(), revision(14), None))
        .result
        .unwrap();
    assert_eq!(f.revision(), revision(15));
    assert_eq!(f.text(), before);
    // Nothing reached the file: the transaction holds it.
    assert_eq!(f.file(), bracket());
    // A new begin works.
    assert!(f.begin_point("pa000003").result.is_ok());
    assert!(f.drag().is_some());
}

#[test]
fn outside_a_transaction_the_release_is_its_own_commit() {
    let f = opened(&document(&[&S0000001.replace(
        r#"{"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, "#,
        "",
    )]));
    as_planegcs(&f.sketches);
    f.begin_point("pa000003").result.unwrap();
    let ended = f.ended(Some([62.5, 7.0])).result.unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    let commit = ended.commit.unwrap();
    assert!(commit.written);
    assert_eq!(f.revision(), revision(2));
    // The file holds the release, with pa000003 at (60, 7).
    assert!(
        f.file()
            .contains(r#"{"id": "pa000003", "x": 60, "y": 7, "construction": false}"#),
        "{}",
        f.file()
    );
}

#[test]
fn an_edges_drag_moves_its_line_by_the_cursors_offset_from_the_press() {
    let f = opened(&bracket());
    f.freed();
    let begun = f.envelope("drag_begin", &example("drag_begin/request-edge"));
    assert_eq!(
        begun["result"],
        json!({"sketch": "s0000001", "point": null, "curve": "na000001", "press": [30, 0], "dofs": 1, "moves": ["pa000001", "pa000002", "pa000003", "pa000004", "pa000005", "pa000006"]})
    );
    assert_eq!(
        f.drag(),
        Some(DragOpen {
            sketch: fid("s0000001"),
            grab: DragGrab::Edge {
                curve: eid("na000001"),
                press: [30.0, 0.0],
            },
        })
    );
    // Pressed at (30, 0), dragged to (30, 2): each end moved up 2, nudged by a thousandth
    // of where it began (the fake's rule): pa000001 (0, 0) to (0, 2); pa000002 (60, 0) to
    // (60 + 0.06, 2).
    let frame = f.moved([30.0, 2.0]).result.unwrap();
    assert_eq!(frame.frame, FrameEnd::Solved);
    let at = |id: &str| {
        frame
            .points
            .iter()
            .find(|(p, _)| p.as_str() == id)
            .unwrap()
            .1
    };
    assert_eq!(at("pa000001"), [0.0, 2.0]);
    assert_eq!(at("pa000002"), [60.0 + 60.0 / 1000.0, 2.0]);
    assert_eq!(at("pa000003"), [60.0, 5.0]);
    // A release at the press is a click: nothing changes.
    let ended = f.ended(Some([30.0, 0.0])).result.unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Begin, false));
    assert_eq!(f.revision(), revision(13));
}

#[test]
fn an_esc_ends_the_drag_with_nothing_changed_and_a_move_after_is_refused() {
    let f = opened(&bracket());
    f.freed();
    let before = f.text();
    f.begin_point("pa000003").result.unwrap();
    f.moved([62.5, 7.0]).result.unwrap();
    let esc = f.envelope("drag_end", r#"{"session": "s1", "to": null}"#);
    assert_eq!(
        esc["result"],
        serde_json::from_str::<Value>(&example("drag_end/result-begin")).unwrap()
    );
    assert_eq!(esc["session"]["revision"], 13);
    assert_eq!((f.revision(), f.text()), (revision(13), before));
    assert_eq!(f.drag(), None);
    // The worker's session ended.
    assert_eq!(f.sketches.drag_calls().last(), Some(&DragCall::End));
    // A move, and an end, after it: no drag open.
    let refused = |error: Error| {
        assert_eq!(
            (error.code(), error.parameter(), error.message()),
            (
                ErrorCode::InvalidInput,
                Some("session"),
                "no drag is open in session s1"
            )
        );
        assert_eq!(
            error.cause(),
            Some(
                "a drag ends at drag_end, at a new revision of the session, or when its worker restarts"
            )
        );
    };
    refused(the_error(&f.moved([1.0, 1.0])));
    refused(the_error(&f.ended(None)));
    refused(the_error(&f.ended(Some([1.0, 1.0]))));
    // A new begin works.
    f.begin_point("pa000003").result.unwrap();
    assert!(f.drag().is_some());
}

#[test]
fn a_new_revision_ends_the_drag() {
    let f = opened(&bracket());
    f.freed();
    f.begin_point("pa000003").result.unwrap();
    let ends = f.sketches.drag_calls().len();
    // An edit of another feature: the drag ends as it answers.
    answer(f.sessions.set_field(
        &s1(),
        revision(13),
        fid("p0000001"),
        "name".to_owned(),
        parsed(r#""Legs, renamed""#),
    ))
    .result
    .unwrap();
    assert_eq!(f.drag(), None);
    assert_eq!(f.sketches.drag_calls()[ends..], [DragCall::End]);
    assert_eq!(
        the_error(&f.moved([1.0, 1.0])).message(),
        "no drag is open in session s1"
    );
    // An undo is a new revision too.
    f.begin_point("pa000003").result.unwrap();
    answer(f.sessions.undo(&s1(), revision(14), None))
        .result
        .unwrap();
    assert_eq!(f.drag(), None);
    assert_eq!(
        the_error(&f.ended(Some([1.0, 1.0]))).message(),
        "no drag is open in session s1"
    );
}

#[test]
fn a_replaced_move_answers_dropped_as_the_design_gives_it() {
    let f = opened(&bracket());
    f.freed();
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    // The first move held solving, the second waiting, the third replacing it.
    let first = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    let second = f.sessions.drag_move(&s1(), [70.0, 9.0]);
    let third = f.sessions.drag_move(&s1(), [71.0, 9.0]);
    let dropped = answer(second);
    assert_eq!(
        written(dropped.result.as_ref().unwrap()),
        example("drag_move/result-dropped")
    );
    f.sketches.release();
    assert_eq!(answer(first).result.unwrap().frame, FrameEnd::Solved);
    let newest = answer(third).result.unwrap();
    assert_eq!((newest.to, newest.frame), ([71.0, 9.0], FrameEnd::Solved));
    // Only the first and the third were sent.
    let moves: Vec<DragCall> = f
        .sketches
        .drag_calls()
        .into_iter()
        .filter(|c| matches!(c, DragCall::Move(_)))
        .collect();
    assert_eq!(
        moves,
        [DragCall::Move([62.5, 7.0]), DragCall::Move([71.0, 9.0])]
    );
}

#[test]
fn the_design_refusals_of_a_begin_are_the_sessions_own() {
    // Conflicting constraints: the profile with a second length of na000001, ka00000d,
    // 61 mm. Sorted by ID, ka000009 is the ninth constraint and ka00000d the twelfth
    // (ka00000a removed), so their tags are 9 and 12.
    let conflicting = S0000001
        .replace(
            r#"{"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, "#,
            "",
        )
        .replace(
            r#"{"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}"#,
            r#"{"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}, {"id": "ka00000d", "type": "length", "line": "na000001", "value": 61, "driving": true}"#,
        );
    let f = opened(&document(&[&conflicting]));
    f.sketches.conflicting_drags(vec![Tag(9), Tag(12)]);
    let refused = f.call("drag_begin", &example("drag_begin/request"));
    let errors: Value = serde_json::from_str::<Value>(&refused.text).unwrap()["errors"].clone();
    assert_eq!(
        errors,
        json!([serde_json::from_str::<Value>(&example("drag_begin/refused-conflict")).unwrap()])
    );
    assert_eq!(f.drag(), None);
    // 300 unknowns: a chain of 150 free points.
    let f = opened(&chain(150, false));
    let refused = f.call(
        "drag_begin",
        r#"{"session": "s1", "sketch": "s0000001", "point": "p0000001", "curve": null, "press": null}"#,
    );
    let errors: Value = serde_json::from_str::<Value>(&refused.text).unwrap()["errors"].clone();
    assert_eq!(
        errors,
        json!([serde_json::from_str::<Value>(&example("drag_begin/refused-unknowns")).unwrap()])
    );
    // No worker was asked to drag it.
    assert!(f.sketches.drag_calls().is_empty());
}

#[test]
fn a_part_of_256_unknowns_is_dragged_and_one_of_257_refused() {
    // 128 free points chained: 256 unknowns, taken.
    let f = opened(&chain(128, false));
    f.begin_point("p0000001").result.unwrap();
    assert_eq!(f.sketches.drag_calls().len(), 1);
    // A circle about the last point: its radius one more, 257, refused naming the point.
    let f = opened(&chain(128, true));
    let error = the_error(&f.begin_point("p0000001"));
    assert_eq!(
        (
            error.code(),
            error.parameter(),
            error.feature(),
            error.message()
        ),
        (
            ErrorCode::InvalidInput,
            Some("point"),
            Some("s0000001"),
            "the part of the sketch the drag moves has 257 unknowns, and a drag takes at most 256 there: each move of a larger part takes too long"
        )
    );
    assert!(f.sketches.drag_calls().is_empty());
    // An edge drag of the same part is refused naming the curve.
    let error = the_error(&answer(f.sessions.drag_begin(
        &s1(),
        fid("s0000001"),
        DragGrab::Edge {
            curve: eid("n0000001"),
            press: [1.5, 0.0],
        },
    )));
    assert_eq!(error.parameter(), Some("curve"));
}

#[test]
fn a_move_past_its_time_is_a_worker_timeout_and_ends_the_drag() {
    let f = opened(&bracket());
    f.freed();
    f.begin_point("pa000003").result.unwrap();
    // The fake's control: the worker answers as one stopped past its time does.
    f.sketches.answer_moves(|_, _| {
        Err(Error::new(
            ErrorCode::WorkerTimeout,
            "the sketch worker didn't answer the move within 10 s",
        ))
    });
    let error = the_error(&f.moved([62.5, 7.0]));
    assert_eq!(error.code(), ErrorCode::WorkerTimeout);
    // Given the drag's own time, not a solve's.
    assert_eq!(f.sketches.move_timeouts(), [DRAG_MOVE_TIMEOUT]);
    assert_eq!(DRAG_MOVE_TIMEOUT, std::time::Duration::from_secs(10));
    // Ended: status says none, and a later move and a release with a cursor are refused
    // with the worker's own code.
    assert_eq!(f.drag(), None);
    assert_eq!(
        the_error(&f.moved([1.0, 1.0])).code(),
        ErrorCode::WorkerTimeout
    );
    assert_eq!(
        the_error(&f.ended(Some([1.0, 1.0]))).code(),
        ErrorCode::WorkerTimeout
    );
    // That release took the drag: nothing changed, and a new begin works.
    assert_eq!(f.revision(), revision(13));
    f.sketches
        .answer_moves(|begin, to| Ok(FakeSketches::dragged(begin, to)));
    f.begin_point("pa000003").result.unwrap();
    assert_eq!(f.moved([62.5, 7.0]).result.unwrap().frame, FrameEnd::Solved);
    // An Esc after a failure keeps the begin state, as asked.
    f.sketches.answer_moves(|_, _| {
        Err(Error::new(
            ErrorCode::WorkerCrashed,
            "the sketch worker crashed",
        ))
    });
    assert_eq!(
        the_error(&f.moved([1.0, 1.0])).code(),
        ErrorCode::WorkerCrashed
    );
    let esc = f.ended(None).result.unwrap();
    assert_eq!((esc.kept, esc.changed), (Kept::Begin, false));
}

#[test]
fn a_cancel_of_drag_move_stops_the_move_solving_and_ends_the_drag() {
    let f = opened(&bracket());
    f.freed();
    // Nothing moving: refused, naming the command.
    let refused = f.sessions.cancel(&s1(), SessionCommand::DragMove);
    let error = refused.result.unwrap_err().errors()[0].clone();
    assert_eq!(
        (error.parameter(), error.message()),
        (Some("command"), "no drag_move is running in session s1")
    );
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    assert_eq!(
        f.sessions
            .cancel(&s1(), SessionCommand::DragMove)
            .result
            .unwrap(),
        Cancelled {
            command: SessionCommand::DragMove,
            stopped: true,
        }
    );
    let error = the_error(&answer(moving));
    assert_eq!(
        (error.code(), error.message()),
        (
            ErrorCode::Cancelled,
            "drag_move was cancelled before it finished"
        )
    );
    f.sketches.release();
    // The drag has ended: none open, a move refused as no drag being open.
    assert_eq!(f.drag(), None);
    assert_eq!(
        the_error(&f.moved([1.0, 1.0])).message(),
        "no drag is open in session s1"
    );
    // The handle's own cancel stops it alike.
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    assert!(moving.cancel());
    assert_eq!(the_error(&answer(moving)).code(), ErrorCode::Cancelled);
    f.sketches.release();
    assert_eq!(f.drag(), None);
    // A new begin works.
    f.begin_point("pa000003").result.unwrap();
    assert!(f.drag().is_some());
}

#[test]
fn a_cursor_that_isnt_finite_or_is_out_of_reach_is_refused_naming_to() {
    let f = opened(&bracket());
    f.freed();
    let before = f.text();
    f.begin_point("pa000003").result.unwrap();
    for to in [
        [f64::NAN, 0.0],
        [0.0, f64::INFINITY],
        [f64::NEG_INFINITY, 0.0],
        [2e6, 0.0],
    ] {
        let error = the_error(&f.moved(to));
        assert_eq!(
            (error.code(), error.parameter()),
            (ErrorCode::InvalidInput, Some("to")),
            "{to:?}"
        );
        // The drag goes on.
        assert!(f.drag().is_some(), "{to:?}");
    }
    // On the wire, a number past the largest `f64` alike.
    let far = f.envelope("drag_move", r#"{"session": "s1", "to": [1e999, 0]}"#);
    assert_eq!(far["errors"][0]["parameter"], "to", "{far}");
    assert!(f.drag().is_some());
    assert_eq!(f.moved([62.5, 7.0]).result.unwrap().frame, FrameEnd::Solved);
    // A release at one: refused, the drag ended with nothing changed.
    let error = the_error(&f.ended(Some([f64::NAN, 1.0])));
    assert_eq!(error.parameter(), Some("to"));
    assert_eq!(f.drag(), None);
    assert_eq!((f.revision(), f.text()), (revision(13), before));
    // A press alike, at the begin.
    let error = the_error(&answer(f.sessions.drag_begin(
        &s1(),
        fid("s0000001"),
        DragGrab::Edge {
            curve: eid("na000001"),
            press: [f64::NAN, 0.0],
        },
    )));
    assert_eq!(error.parameter(), Some("press"));
    assert_eq!(f.drag(), None);
}

#[test]
fn what_a_begin_names_is_refused_naming_its_field() {
    let f = opened(&bracket());
    let begin = |sketch: &str, grab: DragGrab| {
        the_error(&answer(f.sessions.drag_begin(&s1(), fid(sketch), grab)))
    };
    let point = |p: &str| DragGrab::Point(eid(p));
    for (error, parameter) in [
        (begin("s0000001", point("pa00000z")), "point"),
        (begin("p0000001", point("pa000003")), "sketch"),
        (begin("zzzzzzzz", point("pa000003")), "sketch"),
        (
            begin(
                "s0000001",
                DragGrab::Edge {
                    curve: eid("pa000003"),
                    press: [0.0, 0.0],
                },
            ),
            "curve",
        ),
    ] {
        assert_eq!(
            (error.code(), error.parameter()),
            (ErrorCode::InvalidInput, Some(parameter)),
            "{error:?}"
        );
    }
    assert_eq!(f.drag(), None);
    // A session that isn't open.
    let error = the_error(&f.moved([0.0, 0.0]));
    assert_eq!(error.message(), "no drag is open in session s1");
    let none = SessionId::parse("s9").unwrap();
    let error = the_error(&answer(f.sessions.drag_move(&none, [0.0, 0.0])));
    assert_eq!(error.message(), "no session s9 is open");
}

#[test]
fn a_move_sent_before_its_begin_answers_is_told_the_begin_may_be_queued() {
    let f = opened(&bracket());
    // A check of the current model holds the queue, its sketch's solve held.
    f.sketches.hold();
    let check = f.sessions.check(
        &s1(),
        silico_core::command::CheckModel::Current { fresh: false },
    );
    assert!(f.sketches.wait_held(HANG));
    let begin = f
        .sessions
        .drag_begin(&s1(), fid("s0000001"), DragGrab::Point(eid("pa000003")));
    let error = the_error(&f.moved([1.0, 1.0]));
    assert_eq!(
        (error.message(), error.cause()),
        (
            "no drag is open in session s1",
            Some(
                "a drag_begin sent to the session hasn't answered yet: it may still be queued, and its drag opens when it runs"
            )
        )
    );
    f.sketches.release();
    answer(check).result.unwrap();
    answer(begin).result.unwrap();
    // Answered: a move is sent.
    assert_eq!(f.moved([62.5, 7.0]).result.unwrap().frame, FrameEnd::Solved);
    // And once it's answered, the cause is the plain one again.
    f.ended(None).result.unwrap();
    assert_eq!(
        the_error(&f.moved([1.0, 1.0])).cause(),
        Some(
            "a drag ends at drag_end, at a new revision of the session, or when its worker restarts"
        )
    );
}

#[test]
fn a_begin_takes_the_published_models_solve_when_it_solved_the_sketch() {
    let f = opened(&bracket());
    // Rebuilt: the model holds the profile's solve.
    answer(f.sessions.check(
        &s1(),
        silico_core::command::CheckModel::Current { fresh: false },
    ))
    .result
    .unwrap();
    let solves = f.sketches.requests().len();
    f.begin_point("pa000003").result.unwrap();
    assert_eq!(f.sketches.requests().len(), solves, "solved again");
    f.ended(None).result.unwrap();
    // Changed since (its thickness removed): solved on the drag's own worker, once.
    f.freed();
    f.begin_point("pa000003").result.unwrap();
    let requests = f.sketches.requests();
    assert_eq!(requests.len(), solves + 1);
}

/// A typed method's call, and what it answers, as `call` must.
type Typed<'a> = Box<dyn Fn() -> Answered + 'a>;

/// What `call` must answer for the typed method's `response`: its envelope on one line,
/// and whether it ran, the session's directory `from` written as `to`.
fn as_written<T: ToWire>(response: &Response<T>, from: &str, to: &str) -> Answered {
    Answered {
        ok: response.result.is_ok(),
        text: response_text(response, Layout::OneLine).replace(from, to),
        png: None,
    }
}

#[test]
fn call_by_name_answers_as_each_typed_method_does() {
    // Two sessions of the same document, each in a directory of its own, driven alike: one
    // by name, one typed.
    let (named, typed) = (opened(&bracket()), opened(&bracket()));
    for f in [&named, &typed] {
        f.freed();
        as_planegcs(&f.sketches);
    }
    let from = typed.dir.path().to_str().unwrap().to_owned();
    let to = named.dir.path().to_str().unwrap().to_owned();
    let begin_text = example("drag_begin/request");
    let pairs: Vec<(&str, String, Typed<'_>)> = vec![
        (
            "drag_begin",
            begin_text.clone(),
            Box::new(|| as_written(&typed.begin_point("pa000003"), &from, &to)),
        ),
        (
            "drag_begin",
            begin_text.clone(),
            Box::new(|| as_written(&typed.begin_point("pa000003"), &from, &to)),
        ),
        (
            "drag_move",
            example("drag_move/request"),
            Box::new(|| as_written(&typed.moved([62.5, 7.0]), &from, &to)),
        ),
        (
            "drag_end",
            example("drag_end/request"),
            Box::new(|| as_written(&typed.ended(Some([62.5, 7.0])), &from, &to)),
        ),
        (
            "drag_move",
            example("drag_move/request"),
            Box::new(|| as_written(&typed.moved([62.5, 7.0]), &from, &to)),
        ),
        (
            "drag_end",
            r#"{"session": "s1", "to": null}"#.to_owned(),
            Box::new(|| as_written(&typed.ended(None), &from, &to)),
        ),
        (
            "drag_begin",
            example("drag_begin/request-edge"),
            Box::new(|| {
                as_written(
                    &answer(typed.sessions.drag_begin(
                        &s1(),
                        fid("s0000001"),
                        DragGrab::Edge {
                            curve: eid("na000001"),
                            press: [30.0, 0.0],
                        },
                    )),
                    &from,
                    &to,
                )
            }),
        ),
    ];
    let mut ran = 0;
    for (name, arguments, typed_answer) in pairs {
        let by_name = named.call(name, &arguments);
        let expected = typed_answer();
        ran += usize::from(expected.ok);
        assert_eq!(by_name, expected, "{name} {arguments}");
    }
    // The begins, the move, and the end ran; the second begin, the late move, and the late
    // Esc were refused.
    assert_eq!(ran, 4);
    assert_eq!(named.text(), typed.text());
}

#[test]
fn each_drag_snapshot_has_the_names_an_agent_reads() {
    let defs = |root: &Value| -> Vec<String> {
        root["$defs"]
            .as_object()
            .map(|defs| defs.keys().cloned().collect())
            .unwrap_or_default()
    };
    for (of, request, request_defs, result, result_defs) in [
        (
            "drag_begin",
            "DragBeginRequest",
            &["Coordinate", "ElementId", "FeatureId"][..],
            "DragBeginResult",
            &["Coordinate", "ElementId", "FeatureId"][..],
        ),
        (
            "drag_move",
            "DragMoveRequest",
            &["Coordinate"],
            "DragMoveResult",
            &[
                "Coordinate",
                "ElementId",
                "FrameEnd",
                "FramePoint",
                "FrameRadius",
            ],
        ),
        (
            "drag_end",
            "DragEndRequest",
            &["Coordinate"],
            "DragEndResult",
            &["CommitResult", "Error", "FeatureId", "Kept"],
        ),
    ] {
        let (req, res) = (schema_of(of, "request"), schema_of(of, "result"));
        assert_eq!(
            (req["title"].as_str(), defs(&req)),
            (
                Some(request),
                request_defs.iter().map(|d| d.to_string()).collect()
            ),
            "{of}"
        );
        assert_eq!(
            (res["title"].as_str(), defs(&res)),
            (
                Some(result),
                result_defs.iter().map(|d| d.to_string()).collect()
            ),
            "{of}"
        );
        // One object at each root, every field required: an MCP tool's input schema.
        assert_eq!(req["type"], "object", "{of}");
        assert!(
            req.get("anyOf").is_none() && req.get("oneOf").is_none(),
            "{of}"
        );
    }
    // Each request's fields, by hand: every one required, none optional.
    assert_eq!(
        schema_of("drag_begin", "request")["required"],
        json!(["curve", "point", "press", "session", "sketch"])
    );
    assert_eq!(
        schema_of("drag_move", "request")["required"],
        json!(["session", "to"])
    );
    assert_eq!(
        schema_of("drag_end", "request")["required"],
        json!(["session", "to"])
    );
    // The frame's words and the kept state's, the design's.
    assert_eq!(
        schema_of("drag_move", "result")["$defs"]["FrameEnd"]["enum"],
        json!(["solved", "failed", "invalid", "dropped"])
    );
    assert_eq!(
        schema_of("drag_end", "result")["$defs"]["Kept"]["enum"],
        json!(["cursor", "begin"])
    );
}

#[test]
fn a_session_closed_ends_its_drag_and_gives_its_worker_back() {
    let f = opened(&document(&[&S0000001.replace(
        r#"{"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, "#,
        "",
    )]));
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    answer(f.sessions.close(&s1())).result.unwrap();
    // By the time the close has answered, the move solving was stopped and has answered,
    // the worker's drag ended with it: the close ends the drag, not the session's last
    // reference going some time after.
    assert_eq!(f.sketches.stops(), 1);
    let moved = moving.try_take().unwrap_or_else(|_| panic!("the move hadn't answered"));
    assert_eq!(the_error(&moved).code(), ErrorCode::Cancelled);
    f.sketches.release();
    // Closed: the session's commands are refused as of no session.
    assert_eq!(
        the_error(&f.moved([1.0, 1.0])).message(),
        "no session s1 is open"
    );
}
