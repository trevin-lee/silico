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
    Answered, Call, Cancellable, Cancelled, DragEnded, DragFrame, DragGrab, DragOpen, FrameEnd,
    Response, Revision, SessionCommand, SessionId, Sessions,
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

/// [`chain`]'s three points and two lines with `n0000001`'s length held twice: `k0000009`
/// repeats `k0000001`, a redundancy whatever the sketch's coordinates, which PlaneGCS
/// flags on the later (FreeCAD's redundant list). Its tags by the translation's rule:
/// `k0000001` 1, `k0000002` 2, `k0000009` 3.
fn chain_held_twice() -> String {
    chain(3, false).replace(
        r#""driving": true}]}"#,
        r#""driving": true}, {"id": "k0000009", "type": "length", "line": "n0000001", "value": 1, "driving": true}]}"#,
    )
}

/// `p0000003`'s coordinates in the document `text`.
fn p3_of(text: &str) -> (f64, f64) {
    let document: Value = serde_json::from_str(text).unwrap();
    let point = document["features"][0]["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "p0000003")
        .unwrap()
        .clone();
    (point["x"].as_f64().unwrap(), point["y"].as_f64().unwrap())
}

#[test]
fn a_drag_in_a_sketch_whose_solve_had_a_redundancy_already_ends_and_is_written() {
    // Every solve flags k0000009 (tag 3) redundant, as it is wherever the points are: the
    // sketch's solve before the drag ranks a redundancy, as its solve after does, and flags
    // nothing the one before didn't, so the drag's end is no worse (a problem the sketch
    // had already isn't the edit's) and stands.
    let f = opened(&chain_held_twice());
    f.sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.redundant = vec![Tag(3)];
        Ok(solved)
    });
    // A redundancy isn't a conflict: the drag begins.
    f.begin_point("p0000003").result.unwrap();
    let ended = f.ended(Some([5.0, 4.0])).result.unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    assert!(ended.commit.unwrap().written);
    assert_eq!(f.revision(), revision(2));
    // Written where the fake's move put it: the cursor, nudged by a thousandth of where
    // p0000003 began, (3, 0); to the document's resolution, 1 nm.
    let (x, y) = p3_of(&f.file());
    assert!((x - (5.0 + 3.0 / 1000.0)).abs() < 1e-9, "{x}");
    assert_eq!(y, 4.0);
    assert_eq!(f.text(), f.file());
}

#[test]
fn a_drag_whose_end_the_solve_finds_worse_is_refused_as_the_drags_the_document_as_it_was() {
    // The solve flags k0000001 (tag 1) redundant only where p0000003 is off the x axis:
    // PlaneGCS's diagnosis reads the constraints' Jacobian where the solve starts, so a
    // constraint can repeat others in one configuration and not another (two point-on-line
    // constraints of one point, on lines through one point, once the lines are collinear).
    // The sketch as opened, p0000003 at (3, 0), solves clean; the drag leaves it at y = 4,
    // where the solve flags k0000001, a redundancy the solve before didn't: worse, so the
    // drag's end is refused, as every edit that writes geometry is, the document as it was.
    // k0000001 is met there (p0000001 and p0000002 don't move: n0000001 is still 1 long), so
    // the redundancy agrees with the others (decision 5) and the drag's own solve keeps the
    // cursor: its judgment is what refuses it.
    let f = opened(&chain(3, false));
    f.sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        // p0000003 is the sketch's third geometry after the origin and the two axes (5).
        let p3 = system.point_values(PointOf::Point(Geometry(5))).unwrap();
        if system.values[p3.y.0 as usize] != 0.0 {
            solved.redundant = vec![Tag(1)];
        }
        Ok(solved)
    });
    let before = f.text();
    assert_eq!(p3_of(&before), (3.0, 0.0));
    f.begin_point("p0000003").result.unwrap();
    let response = f.ended(Some([5.0, 4.0]));
    let error = the_error(&response);
    assert_eq!(error.code(), ErrorCode::OverConstrained, "{error:?}");
    assert_eq!(error.feature(), Some("s0000001"));
    assert_eq!(error.elements(), ["k0000001"]);
    assert!(
        error.message().starts_with("after the drag, "),
        "{}",
        error.message()
    );
    // Nothing changed: no new revision, the text and the file as they were, and the drag
    // over.
    assert_eq!(f.revision(), revision(1));
    assert_eq!(f.text(), before);
    assert_eq!(f.file(), chain(3, false));
    assert_eq!(f.drag(), None);
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
            command: Cancellable::Command(SessionCommand::DragMove),
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
    let moved = moving
        .try_take()
        .unwrap_or_else(|_| panic!("the move hadn't answered"));
    // No client stopped it: it answers as a command of a session that isn't open does (the
    // design's `close`), never `cancelled`.
    let error = the_error(&moved);
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
    assert_eq!(error.message(), "no session s1 is open");
    assert_eq!(error.parameter(), Some("session"));
    f.sketches.release();
    // Closed: the session's commands are refused as of no session.
    assert_eq!(
        the_error(&f.moved([1.0, 1.0])).message(),
        "no session s1 is open"
    );
}

#[test]
fn a_reload_of_the_file_changed_on_disk_ends_the_drag() {
    let profile = document(&[&S0000001.replace(
        r#"{"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, "#,
        "",
    )]);
    let f = opened(&profile);
    f.begin_point("pa000003").result.unwrap();
    // Another client renames the profile; the release, queued, reloads the file first, a
    // new revision, which ends the drag before the release runs.
    let renamed = profile.replace("Bracket profile", "Profile");
    std::fs::write(f.dir.path().join("bracket.silico"), &renamed).unwrap();
    let release = f.ended(Some([62.5, 7.0]));
    assert!(release.session.as_ref().unwrap().reloaded);
    assert_eq!(
        the_error(&release).message(),
        "no drag is open in session s1"
    );
    assert_eq!(f.file(), renamed);
    // And a drag begun at the new revision is ended by a command's reload alike.
    f.begin_point("pa000003").result.unwrap();
    std::fs::write(f.dir.path().join("bracket.silico"), &profile).unwrap();
    let text = answer(f.sessions.text(&s1(), None));
    assert!(text.session.unwrap().reloaded);
    assert_eq!(f.drag(), None);
    assert_eq!(f.sketches.drag_calls().last(), Some(&DragCall::End));
}

/// The bracket's profile alone, its base leg's thickness free.
fn freed_profile() -> String {
    document(&[&S0000001.replace(
        r#"{"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, "#,
        "",
    )])
}

#[test]
fn a_move_the_solver_fails_or_solves_to_what_cant_be_built_answers_its_kind_with_no_values() {
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    for (status, end) in [
        (SolveStatus::Failed, FrameEnd::Failed),
        (SolveStatus::SolvedInvalid, FrameEnd::Invalid),
    ] {
        f.sketches.answer_moves(move |_, _| {
            Ok(Moved {
                status,
                values: None,
            })
        });
        let frame = f.moved([62.5, 7.0]).result.unwrap();
        assert_eq!(
            frame,
            DragFrame {
                to: [62.5, 7.0],
                frame: end,
                points: vec![],
                radii: vec![],
            },
            "{status:?}"
        );
        // The drag goes on.
        assert!(f.drag().is_some(), "{status:?}");
    }
}

#[test]
fn a_frame_gives_each_free_radius_of_its_part_by_id() {
    // Two points, a line of held length between them, and a circle about the second with
    // the first held on it: the circle's radius is in the part.
    let f = opened(&chain(2, true));
    f.begin_point("p0000001").result.unwrap();
    // The fake's answer: the circle's radius 2.5, every other value as it began.
    f.sketches.answer_moves(|begin, _| {
        let mut values = begin.system.values.clone();
        let radius = begin
            .system
            .geometry
            .iter()
            .find_map(|g| match g {
                silico_protocol::SketchGeometry::Circle { radius, .. } => Some(*radius),
                _ => None,
            })
            .unwrap();
        values[radius.0 as usize] = 2.5;
        Ok(Moved {
            status: SolveStatus::Success,
            values: Some(values),
        })
    });
    let frame = f.moved([1.0, 1.0]).result.unwrap();
    assert_eq!(frame.radii, [(eid("c0000001"), 2.5)]);
    // Both points, where they began: (1, 0) and (2, 0).
    assert_eq!(
        frame.points,
        [(eid("p0000001"), [1.0, 0.0]), (eid("p0000002"), [2.0, 0.0])]
    );
}

#[test]
fn a_moves_reason_reaches_the_sessions_logs() {
    use silico_core::command::logs::{LogLevel, LogQuery};
    use tracing_subscriber::layer::SubscriberExt;
    // Alone in its process (nextest runs each test in its own): the core's log layer.
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(silico_core::command::logs::layer()),
    );
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    // A move the worker refuses: a failed frame, its reason at info.
    f.sketches.answer_moves(|_, _| {
        Err(Error::new(
            ErrorCode::InvalidInput,
            "the fake refused the move",
        ))
    });
    assert_eq!(f.moved([62.5, 7.0]).result.unwrap().frame, FrameEnd::Failed);
    // And one that doesn't reach its cursor.
    f.sketches.answer_moves(|_, _| {
        Ok(Moved {
            status: SolveStatus::Failed,
            values: None,
        })
    });
    assert_eq!(f.moved([62.5, 8.0]).result.unwrap().frame, FrameEnd::Failed);
    // And one the worker fails, which ends the drag.
    f.sketches.answer_moves(|_, _| {
        Err(Error::new(
            ErrorCode::WorkerCrashed,
            "the fake worker crashed",
        ))
    });
    assert_eq!(
        the_error(&f.moved([62.5, 9.0])).code(),
        ErrorCode::WorkerCrashed
    );
    let logs = f
        .sessions
        .read_logs(
            &s1(),
            &LogQuery {
                level: LogLevel::Info,
                feature: None,
                after: None,
                limit: 1000,
            },
        )
        .result
        .unwrap();
    let said: Vec<(&str, &str)> = logs
        .events
        .iter()
        .filter(|e| e.message.starts_with("the drag's move"))
        .map(|e| (e.spans[0].as_str(), e.message.as_str()))
        .collect();
    // Each filed under the session's command that began the drag, whose thread solves it.
    assert_eq!(
        said,
        [
            (
                "drag_begin",
                "the drag's move was refused: the fake refused the move"
            ),
            ("drag_begin", "the drag's move didn't reach its cursor"),
            (
                "drag_begin",
                "the drag's move failed: the fake worker crashed code=worker_crashed"
            ),
        ]
    );
}

#[test]
fn a_moves_reason_is_filed_under_the_drags_begin_when_its_own_drag_move_answers_it() {
    // A move's frame is answered by whichever thread finds it: the drag's own, as the frame
    // comes, or the move's `drag_move`, if the frame came before the move was kept among
    // the drag's moves. Its events are filed alike either way, under the command that began
    // the drag (unfixed, the `drag_move` that answered it filed them under itself, so the
    // test before this one flaked on a gate). Forced here: a move that replaces one waiting
    // wakes the client before it's kept, and is held in that wake until its frame has come.
    use silico_core::command::logs::{LogLevel, LogQuery};
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use tracing_subscriber::layer::SubscriberExt;
    // Alone in its process (nextest runs each test in its own): the core's log layer.
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(silico_core::command::logs::layer()),
    );
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    // Each move's solve says it's solving and waits to be let go; the worker then refuses
    // the replacing move, and solves the others.
    const SOLVING: [f64; 2] = [62.5, 7.0];
    const REPLACED: [f64; 2] = [66.0, 7.0];
    const REPLACING: [f64; 2] = [70.0, 7.0];
    const AFTER: [f64; 2] = [74.0, 7.0];
    let (solves, solving) = channel::<[f64; 2]>();
    let solves = Mutex::new(solves);
    let (lets, gates): (Vec<Sender<()>>, Vec<Mutex<Receiver<()>>>) = (0..3)
        .map(|_| {
            let (go, gate) = channel::<()>();
            (go, Mutex::new(gate))
        })
        .unzip();
    f.sketches.answer_moves(move |begin, to| {
        let Some(at) = [SOLVING, REPLACING, AFTER].iter().position(|&c| c == to) else {
            panic!("a move to {to:?} was solved: it should have been dropped");
        };
        let _ = solves.lock().unwrap().send(to);
        let _ = gates[at].lock().unwrap().recv_timeout(HANG);
        if to == REPLACING {
            Err(Error::new(
                ErrorCode::InvalidInput,
                "the fake refused the move",
            ))
        } else {
            Ok(FakeSketches::dragged(begin, to))
        }
    });
    let first = f.sessions.drag_move(&s1(), SOLVING);
    assert_eq!(solving.recv_timeout(HANG).unwrap(), SOLVING);
    // Waiting behind the move solving: the one the next replaces.
    let replaced = f.sessions.drag_move(&s1(), REPLACED);
    let mover = Arc::new(Mutex::new(None::<std::thread::ThreadId>));
    let (let_go, gate) = channel::<()>();
    let gate = Mutex::new(gate);
    let (held, in_waker) = channel::<()>();
    let held = Mutex::new(held);
    {
        let mover = Arc::clone(&mover);
        f.sessions.on_change(move || {
            // Only the replacing move's own wake, on its own thread, and only once.
            let mut mover = mover.lock().unwrap();
            if mover.as_ref() == Some(&std::thread::current().id()) {
                *mover = None;
                drop(mover);
                let _ = held.lock().unwrap().send(());
                let _ = gate.lock().unwrap().recv_timeout(HANG);
            }
        });
    }
    let sessions = &f.sessions;
    let (replacing, after) = std::thread::scope(|scope| {
        let moving = scope.spawn(|| {
            *mover.lock().unwrap() = Some(std::thread::current().id());
            sessions.drag_move(&s1(), REPLACING)
        });
        // The replacing move, in its wake, not yet kept.
        in_waker
            .recv_timeout(HANG)
            .expect("the replacing move didn't wake the client before it was kept");
        // The move solving let go: the drag's thread takes the replacing move next.
        lets[0].send(()).unwrap();
        assert_eq!(solving.recv_timeout(HANG).unwrap(), REPLACING);
        // One sent behind it, then the replacing move refused: the drag's thread found no
        // move kept to answer with its frame, and took the next, so its frame is there.
        let after = sessions.drag_move(&s1(), AFTER);
        lets[1].send(()).unwrap();
        assert_eq!(solving.recv_timeout(HANG).unwrap(), AFTER);
        // The replacing move kept, its frame there already: answered by its own drag_move,
        // on its own thread, before it returns, while the drag's thread waits (so it can be
        // no other thread's answer).
        let_go.send(()).unwrap();
        let replacing = answer(moving.join().unwrap());
        lets[2].send(()).unwrap();
        (replacing, after)
    });
    assert_eq!(answer(first).result.unwrap().frame, FrameEnd::Solved);
    assert_eq!(answer(replaced).result.unwrap().frame, FrameEnd::Dropped);
    assert_eq!(replacing.result.unwrap().frame, FrameEnd::Failed);
    assert_eq!(answer(after).result.unwrap().frame, FrameEnd::Solved);
    let logs = f
        .sessions
        .read_logs(
            &s1(),
            &LogQuery {
                level: LogLevel::Info,
                feature: None,
                after: None,
                limit: 1000,
            },
        )
        .result
        .unwrap();
    let said: Vec<(&str, &str)> = logs
        .events
        .iter()
        .filter(|e| e.message.starts_with("the drag's move"))
        .map(|e| (e.spans[0].as_str(), e.message.as_str()))
        .collect();
    assert_eq!(
        said,
        [(
            "drag_begin",
            "the drag's move was refused: the fake refused the move"
        )]
    );
}

#[test]
fn the_sessions_own_reload_of_its_file_ends_the_drag() {
    let profile = freed_profile();
    let f = opened(&profile);
    f.begin_point("pa000003").result.unwrap();
    let began = f.revision();
    std::fs::write(
        f.dir.path().join("bracket.silico"),
        profile.replace("Bracket profile", "Profile"),
    )
    .unwrap();
    // No command sent: `status` compares the file, at most once a second, and the session
    // reloads it itself, a new revision, which ends the drag.
    let deadline = std::time::Instant::now() + HANG;
    loop {
        let ended = f.revision() > began
            && f.drag().is_none()
            && f.sketches.drag_calls().last() == Some(&DragCall::End);
        if ended {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the reload didn't end the drag"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        the_error(&f.moved([1.0, 1.0])).message(),
        "no drag is open in session s1"
    );
}

#[test]
fn a_cancel_of_drag_begin_or_drag_end_mid_solve_stops_its_solve() {
    let f = opened(&freed_profile());
    // The begin's solve of the sketch (no model holds it), held.
    f.sketches.hold();
    let begin = f
        .sessions
        .drag_begin(&s1(), fid("s0000001"), DragGrab::Point(eid("pa000003")));
    assert!(f.sketches.wait_held(HANG));
    let stops = f.sketches.stops();
    assert!(
        f.sessions
            .cancel(&s1(), SessionCommand::DragBegin)
            .result
            .unwrap()
            .stopped
    );
    let begun = answer(begin);
    f.sketches.release();
    assert_eq!(the_error(&begun).code(), ErrorCode::Cancelled);
    assert_eq!(f.sketches.stops(), stops + 1);
    assert_eq!(f.drag(), None);
    // The end's whole solve, after a frame at its cursor, held.
    f.begin_point("pa000003").result.unwrap();
    f.moved([62.5, 7.0]).result.unwrap();
    f.sketches.hold();
    let end = f.sessions.drag_end(&s1(), Some([62.5, 7.0]));
    assert!(f.sketches.wait_held(HANG));
    let stops = f.sketches.stops();
    assert!(
        f.sessions
            .cancel(&s1(), SessionCommand::DragEnd)
            .result
            .unwrap()
            .stopped
    );
    let ended = answer(end);
    f.sketches.release();
    assert_eq!(the_error(&ended).code(), ErrorCode::Cancelled);
    assert_eq!(f.sketches.stops(), stops + 1);
    // Nothing changed, no drag open.
    assert_eq!(f.revision(), revision(1));
    assert_eq!(f.file(), freed_profile());
    assert_eq!(f.drag(), None);
}

/// `running`'s answer once `f`'s sessions are dropped, on a thread of their own, while its
/// solve is held: every session closed with them, as a client that goes away closes its
/// own; the held solve stopped by the close, then released.
fn closed_mid_solve<T: std::fmt::Debug>(
    f: Fixture,
    running: silico_core::command::Running<T>,
) -> Response<T> {
    assert!(f.sketches.wait_held(HANG), "the solve never came");
    let Fixture {
        dir,
        sketches,
        sessions,
    } = f;
    let dropper = std::thread::spawn(move || drop(sessions));
    let answered = answer(running);
    assert!(sketches.stops() >= 1);
    dropper.join().unwrap();
    sketches.release();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("bracket.silico")).unwrap(),
        freed_profile()
    );
    answered
}

#[test]
fn a_begin_or_an_end_whose_solve_its_sessions_close_stops_answers_not_open_never_cancelled() {
    // No client stopped either: each answers as a command of a session that isn't open
    // does (the design's `close`: only a client's cancel makes a command answer
    // `cancelled`), not with the drag's words for a stopped solve.
    let not_open = |error: Error| {
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
        assert_eq!(error.message(), "no session s1 is open");
        assert_eq!(error.parameter(), Some("session"));
    };
    // The begin's solve of the sketch, held.
    let f = opened(&freed_profile());
    f.sketches.hold();
    let begin = f
        .sessions
        .drag_begin(&s1(), fid("s0000001"), DragGrab::Point(eid("pa000003")));
    not_open(the_error(&closed_mid_solve(f, begin)));
    // The end's whole solve, after a frame at its cursor, held.
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    f.moved([62.5, 7.0]).result.unwrap();
    f.sketches.hold();
    let end = f.sessions.drag_end(&s1(), Some([62.5, 7.0]));
    not_open(the_error(&closed_mid_solve(f, end)));
}

#[test]
fn a_move_the_close_missed_as_it_was_being_kept_answers_not_open_never_cancelled() {
    // A move that replaces one waiting wakes the client before it's kept among the
    // drag's moves (the waker runs once the drag's lock is let go). A close that ends the
    // drag in that window tells the moves it finds, not this one: the move must learn the
    // session closed as it's kept, and answer as a command of a session that isn't open
    // does (the design's `close`; the robustness review of #307): not with a frame of the
    // drag the close ended (unfixed, it answered `dropped`), nor `cancelled`.
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let solving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    // Waiting behind the move solving: the one the next replaces.
    let first = f.sessions.drag_move(&s1(), [66.0, 7.0]);
    let mover = Arc::new(std::sync::Mutex::new(None::<std::thread::ThreadId>));
    let (let_go, gate) = std::sync::mpsc::channel::<()>();
    let gate = std::sync::Mutex::new(gate);
    let (held, in_waker) = std::sync::mpsc::channel::<()>();
    let held = std::sync::Mutex::new(held);
    {
        let mover = Arc::clone(&mover);
        f.sessions.on_change(move || {
            // Only the replacing move's own wake, on its own thread, and only once.
            let mut mover = mover.lock().unwrap();
            if mover.as_ref() == Some(&std::thread::current().id()) {
                *mover = None;
                drop(mover);
                let _ = held.lock().unwrap().send(());
                let _ = gate.lock().unwrap().recv();
            }
        });
    }
    let sessions = &f.sessions;
    let second = std::thread::scope(|scope| {
        let moving = scope.spawn(|| {
            *mover.lock().unwrap() = Some(std::thread::current().id());
            sessions.drag_move(&s1(), [70.0, 7.0])
        });
        // The replacing move, in its wake, not yet kept: the session closes now.
        in_waker
            .recv_timeout(HANG)
            .expect("the replacing move didn't wake the client before it was kept");
        answer(sessions.close(&s1())).result.unwrap();
        let_go.send(()).unwrap();
        moving.join().unwrap()
    });
    f.sketches.release();
    assert_eq!(
        answer(first).result.unwrap().frame,
        FrameEnd::Dropped,
        "the replaced move"
    );
    // The move solving was told by the close itself.
    assert_eq!(
        the_error(&answer(solving)).message(),
        "no session s1 is open"
    );
    let error = the_error(&answer(second));
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
    assert_eq!(error.message(), "no session s1 is open");
    assert_eq!(error.parameter(), Some("session"));
}

#[test]
fn after_a_cancelled_move_a_release_with_a_cursor_is_told_no_drag_is_open() {
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    f.sessions
        .cancel(&s1(), SessionCommand::DragMove)
        .result
        .unwrap();
    assert_eq!(the_error(&answer(moving)).code(), ErrorCode::Cancelled);
    f.sketches.release();
    let error = the_error(&f.ended(Some([62.5, 7.0])));
    assert_eq!(
        (error.code(), error.message()),
        (ErrorCode::InvalidInput, "no drag is open in session s1")
    );
}

#[test]
fn a_release_at_another_cursor_than_the_last_moves_moves_once_more_from_the_begin_state() {
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    f.moved([62.5, 7.0]).result.unwrap();
    let ended = f.ended(Some([61.0, 9.0])).result.unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    // One move more, to the release's cursor.
    let moves: Vec<DragCall> = f
        .sketches
        .drag_calls()
        .into_iter()
        .filter(|c| matches!(c, DragCall::Move(_)))
        .collect();
    assert_eq!(
        moves,
        [DragCall::Move([62.5, 7.0]), DragCall::Move([61.0, 9.0])]
    );
    // From the begin state, as the fake answers it: the cursor, nudged by a thousandth of
    // where pa000003 began, (60, 5); not of the last frame's (62.56, 7.005).
    let text = f.file();
    let document: Value = serde_json::from_str(&text).unwrap();
    let point = document["features"][0]["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "pa000003")
        .unwrap()
        .clone();
    assert_eq!(
        (point["x"].as_f64().unwrap(), point["y"].as_f64().unwrap()),
        (61.0 + 60.0 / 1000.0, 9.0 + 5.0 / 1000.0)
    );
}

/// A sketch `s0000001` on xy of an arc about `p0000001` at the origin, counter-clockwise
/// from `start` to `end`, each a point of the arc's circle of radius 10, its points free.
fn arc_sketch(start: [i32; 2], end: [i32; 2]) -> String {
    let point = |id: &str, [x, y]: [i32; 2]| {
        format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
    };
    document(&[&format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Bend", "plane": "xy", "points": [{}, {}, {}], "lines": [], "circles": [], "arcs": [{{"id": "a0000001", "center": "p0000001", "start": "p0000002", "end": "p0000003", "construction": false}}], "constraints": []}}"#,
        point("p0000001", [0, 0]),
        point("p0000002", start),
        point("p0000003", end),
    )])
}

/// An edge drag of `curve` pressed at `press` on `f`'s sketch, ended at once if it began.
fn pressed(f: &Fixture, curve: &str, press: [f64; 2]) -> Result<(), Error> {
    let begun = answer(f.sessions.drag_begin(
        &s1(),
        fid("s0000001"),
        DragGrab::Edge {
            curve: eid(curve),
            press,
        },
    ));
    match begun.result {
        Ok(_) => {
            f.ended(None).result.unwrap();
            Ok(())
        }
        Err(_) => Err(the_error(&begun)),
    }
}

/// The refusal of a press `off` mm from the edge, at `press`, as the begin says it.
fn off_by(error: &Error, press: &str, off: &str) {
    assert_eq!(
        (error.code(), error.parameter(), error.message()),
        (
            ErrorCode::InvalidInput,
            Some("press"),
            format!(
                "the drag's press at {press} is {off} mm off the edge it drags: a press lands on its edge"
            )
            .as_str()
        )
    );
}

#[test]
fn an_arcs_press_is_taken_on_its_sweep_and_refused_off_it_by_its_nearer_end() {
    // A quarter, from (10, 0) to (0, 10): (6, 8) is on it (6² + 8² = 10², at 53°, within
    // the sweep); (-6, -8) is on its circle but past its ends, √320 from (10, 0), nearer
    // than √360 from (0, 10).
    let f = opened(&arc_sketch([10, 0], [0, 10]));
    pressed(&f, "a0000001", [6.0, 8.0]).unwrap();
    off_by(
        &pressed(&f, "a0000001", [-6.0, -8.0]).unwrap_err(),
        "(-6, -8)",
        "17.88854381999832",
    );
    // A half, from (0, -10) to (0, 10) through (10, 0), across angle 0: its start's angle
    // below 0 or past a turn, the sweep reckoned round. (10, 0) is on it; (-10, 0) past
    // it, √200 from either end.
    let f = opened(&arc_sketch([0, -10], [0, 10]));
    pressed(&f, "a0000001", [10.0, 0.0]).unwrap();
    off_by(
        &pressed(&f, "a0000001", [-10.0, 0.0]).unwrap_err(),
        "(-10, 0)",
        "14.142135623730951",
    );
}

#[test]
fn a_press_the_length_tolerance_off_its_line_is_taken_and_one_past_it_refused() {
    // na000001, from (0, 0) to (60, 0): 1e-7 mm above it is on it, inclusive; the next
    // number up isn't.
    let f = opened(&freed_profile());
    pressed(&f, "na000001", [30.0, 1e-7]).unwrap();
    let past = 1e-7f64.next_up();
    let error = pressed(&f, "na000001", [30.0, past]).unwrap_err();
    assert_eq!(
        (error.code(), error.parameter()),
        (ErrorCode::InvalidInput, Some("press"))
    );
    assert!(
        error.message().contains("mm off the edge it drags"),
        "{}",
        error.message()
    );
    assert_eq!(f.drag(), None);
}

#[test]
fn a_cancel_of_drag_move_returns_without_waiting_on_the_drags_thread() {
    // The drag's thread held in the client's waker as it answers the stopped move: a cancel
    // that waited for that thread (to join it) would never return, since the test lets the
    // waker go only once the cancel has.
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (let_go, gate) = std::sync::mpsc::channel::<()>();
    let gate = Arc::new(std::sync::Mutex::new(gate));
    let (held, in_waker) = std::sync::mpsc::channel::<()>();
    let held = std::sync::Mutex::new(held);
    {
        let armed = Arc::clone(&armed);
        // Only the drag's own thread, once: any other wake meanwhile (the opened document's
        // rebuild publishing, a read answering) passes, since one held here too would wait
        // for a second let-go that never comes, and the test would time out for nothing the
        // cancel did (#345's gate).
        f.sessions.on_change(move || {
            if std::thread::current().name() == Some("silico-drag")
                && armed.swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                let _ = held.lock().unwrap().send(());
                let _ = gate.lock().unwrap().recv_timeout(HANG);
            }
        });
    }
    // On a thread of its own, so a cancel that waits fails the test rather than hanging it.
    let sessions = &f.sessions;
    let cancelled = std::thread::scope(|scope| {
        let cancel = scope.spawn(|| sessions.cancel(&s1(), SessionCommand::DragMove));
        // The drag's thread reaches the waker, answering the stopped move.
        in_waker.recv_timeout(HANG).unwrap();
        let (returned, cancelled) = std::sync::mpsc::channel();
        scope.spawn(move || {
            let _ = returned.send(cancel.join().unwrap());
        });
        // The cancel returns while the drag's thread is still held in the waker.
        let answer = cancelled.recv_timeout(HANG);
        armed.store(false, std::sync::atomic::Ordering::SeqCst);
        let_go.send(()).unwrap();
        answer.expect("the cancel waited on the drag's thread")
    });
    assert_eq!(
        cancelled.result.unwrap(),
        Cancelled {
            command: Cancellable::Command(SessionCommand::DragMove),
            stopped: true,
        }
    );
    assert_eq!(the_error(&answer(moving)).code(), ErrorCode::Cancelled);
    f.sketches.release();
    assert_eq!(f.drag(), None);
    // The closed drag is joined by what comes next: a new begin works.
    f.begin_point("pa000003").result.unwrap();
    assert!(f.drag().is_some());
}

#[test]
fn a_move_wakes_the_client_once_its_frame_has_come_and_never_as_it_is_sent() {
    // The client's waker is told what changed once it can be read: a move's frame, on the
    // drag's thread as it answers. Sending a move that answers nothing (none replaced, its
    // own frame held in the solver) changes nothing a client reads, and wakes no one.
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    let sender = std::thread::current().id();
    let on_sender = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (woke, wakes) = std::sync::mpsc::channel::<()>();
    let woke = std::sync::Mutex::new(woke);
    {
        let on_sender = Arc::clone(&on_sender);
        f.sessions.on_change(move || {
            let current = std::thread::current();
            if current.id() == sender {
                on_sender.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            } else if current.name() == Some("silico-drag") {
                let _ = woke.lock().unwrap().send(());
            }
        });
    }
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    assert_eq!(
        on_sender.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a move that answered nothing woke the client as it was sent"
    );
    f.sketches.release();
    assert_eq!(answer(moving).result.unwrap().frame, FrameEnd::Solved);
    wakes
        .recv_timeout(HANG)
        .expect("the move's frame didn't wake the client");
    assert_eq!(on_sender.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// The cause a move or a release is told with when no drag is open and no `drag_begin` is
/// queued.
const ENDED: &str =
    "a drag ends at drag_end, at a new revision of the session, or when its worker restarts";

/// The cause it's told with when a `drag_begin` sent to the session hasn't answered.
const QUEUED: &str = "a drag_begin sent to the session hasn't answered yet: it may still be queued, and its drag opens when it runs";

#[test]
fn a_move_after_its_drag_was_cancelled_is_told_why_by_whether_a_begin_is_queued() {
    // A cancelled move leaves its drag the session's, closed, until what comes next takes
    // its place: a move then is refused as no drag being open, its cause saying whether a
    // begin is on its way.
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    f.sketches.hold();
    let moving = f.sessions.drag_move(&s1(), [62.5, 7.0]);
    assert!(f.sketches.wait_held(HANG));
    f.sessions
        .cancel(&s1(), SessionCommand::DragMove)
        .result
        .unwrap();
    assert_eq!(the_error(&answer(moving)).code(), ErrorCode::Cancelled);
    f.sketches.release();
    // No begin sent: the plain cause.
    let error = the_error(&f.moved([1.0, 1.0]));
    assert_eq!(
        (error.message(), error.cause()),
        ("no drag is open in session s1", Some(ENDED))
    );
    // A begin queued behind a fresh check, its sketch's solve held: the begin's cause.
    f.sketches.hold();
    let check = f.sessions.check(
        &s1(),
        silico_core::command::CheckModel::Current { fresh: true },
    );
    assert!(f.sketches.wait_held(HANG));
    let begin = f
        .sessions
        .drag_begin(&s1(), fid("s0000001"), DragGrab::Point(eid("pa000003")));
    let error = the_error(&f.moved([1.0, 1.0]));
    assert_eq!(
        (error.message(), error.cause()),
        ("no drag is open in session s1", Some(QUEUED))
    );
    f.sketches.release();
    answer(check).result.unwrap();
    answer(begin).result.unwrap();
    assert!(f.drag().is_some());
}

#[test]
fn a_release_run_before_a_begin_sent_after_it_is_told_the_begin_may_be_queued() {
    // A release and then a begin, sent while a check holds the queue: the release runs
    // first, with no drag open, and is told a begin is on its way; once that begin has
    // answered, a release ends its drag, and the next is told plainly.
    let f = opened(&bracket());
    f.sketches.hold();
    let check = f.sessions.check(
        &s1(),
        silico_core::command::CheckModel::Current { fresh: false },
    );
    assert!(f.sketches.wait_held(HANG));
    let end = f.sessions.drag_end(&s1(), None);
    let begin = f
        .sessions
        .drag_begin(&s1(), fid("s0000001"), DragGrab::Point(eid("pa000003")));
    f.sketches.release();
    answer(check).result.unwrap();
    let error = the_error(&answer(end));
    assert_eq!(
        (error.message(), error.cause()),
        ("no drag is open in session s1", Some(QUEUED))
    );
    answer(begin).result.unwrap();
    f.ended(None).result.unwrap();
    let error = the_error(&f.ended(None));
    assert_eq!(
        (error.message(), error.cause()),
        ("no drag is open in session s1", Some(ENDED))
    );
}

#[test]
fn a_drag_whose_thread_panicked_says_its_end_failed_in_the_sessions_logs_as_the_next_begin_ends_it()
{
    use silico_core::command::logs::{LogLevel, LogQuery};
    use tracing_subscriber::layer::SubscriberExt;
    // Alone in its process (nextest runs each test in its own): the core's log layer.
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(silico_core::command::logs::layer()),
    );
    let f = opened(&freed_profile());
    f.begin_point("pa000003").result.unwrap();
    // A bug in the solver under a move: the drag's thread unwinds, the move failed and the
    // drag closed, its end's answer the panic's.
    f.sketches
        .answer_moves(|_, _| panic!("a bug in the sketch solver"));
    let error = the_error(&f.moved([62.5, 7.0]));
    assert_eq!(
        (error.code(), error.message()),
        (
            ErrorCode::Internal,
            "the drag's thread panicked under its solver"
        )
    );
    f.sketches
        .answer_moves(|begin, to| Ok(FakeSketches::dragged(begin, to)));
    // The next begin takes the closed drag's place, ending it: its end failed, as the
    // session's logs say, at warn, under the end's own span.
    f.begin_point("pa000003").result.unwrap();
    let logs = f
        .sessions
        .read_logs(
            &s1(),
            &LogQuery {
                level: LogLevel::Warn,
                feature: None,
                after: None,
                limit: 1000,
            },
        )
        .result
        .unwrap();
    let ends: Vec<&Vec<String>> = logs
        .events
        .iter()
        .filter(|e| e.message.starts_with("the drag's end failed"))
        .map(|e| &e.spans)
        .collect();
    assert_eq!(ends.len(), 1, "{:?}", logs.events);
    assert_eq!(
        ends[0].last().map(String::as_str),
        Some("drag_ended sketch=s0000001"),
        "{:?}",
        ends[0]
    );
}
