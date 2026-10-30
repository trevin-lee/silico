//! `sketch_add` and `sketch_remove` (docs/design/m5.0-app.md, section 7, *The sketch edits*),
//! through the sessions' own command called by name, as a transport calls it
//! (`Sessions::call`), on the fake kernel and the fake sketch solver. Expected values: the
//! design's examples, byte for byte (each file the design's own block); the bracket's members
//! and what the edits do to them, by hand from the design's rules and the port note's
//! (docs/ports/sketch-edit.md, decision 7); and the refusals, written here by hand as exact
//! JSON. The solver's verdict (SD7: an over-defining edit applied, and reported) is checked
//! on PlaneGCS, in silico-sketch's tests/session_sketch_edits.rs; here, that the edit itself
//! asks the solver nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::wire::sketch_edits::{SketchAddRequest, SketchRemoveRequest};
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{
    Answered, Call, NewMembers, Revision, SessionId, Sessions, SketchEdited,
};
use silico_core::document::json::{Node, read};
use silico_core::document::{FeatureId, Reference};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

use crate::bracket::{H0000001, P0000001, S0000001, S0000002, S0000003};
use crate::sessions::{HANG, answer};

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract")
        .join(name)
}

/// The committed example `name` (`sketch_add/request`), read and never written.
fn example(name: &str) -> String {
    let path = contract(&format!("commands/examples/{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
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
    let text = std::fs::read_to_string(contract(&format!("commands/{of}.json"))).unwrap();
    serde_json::from_str::<Value>(&text).unwrap()[part].clone()
}

/// The error contract's shape, the envelope's `Error`.
fn error_schema() -> Value {
    let text = std::fs::read_to_string(contract("commands/envelope.json")).unwrap();
    let envelope: Value = serde_json::from_str(&text).unwrap();
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

fn revision(n: u64) -> Revision {
    let mut r = Revision::OPENED;
    for _ in 1..n {
        r = r.next().unwrap();
    }
    r
}

/// The bracket's first five features (M4.9's, as its session added them): the L profile on
/// xz, the pad, the base's holes on xy, the pocket through them, and the upright's holes.
fn bracket() -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        [S0000001, P0000001, S0000002, H0000001, S0000003].join(", ")
    )
}

/// Sessions on the fake kernel and the fake sketch solver, with the bracket open as s1 from
/// `bracket.silico` in a scratch directory.
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
    fn path(&self) -> PathBuf {
        self.dir.path().join("bracket.silico")
    }

    /// The command `name`'s answer to `arguments`, as a transport has it.
    fn answered(&self, name: &str, arguments: &str) -> Answered {
        match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}")),
            Call::Unknown => panic!("no command {name}"),
        }
    }

    /// The envelope the command `name` answers to `arguments`.
    fn call(&self, name: &str, arguments: &str) -> Value {
        let answer = self.answered(name, arguments);
        let envelope: Value = serde_json::from_str(&answer.text).unwrap();
        assert_eq!(envelope["ok"], answer.ok, "{}", answer.text);
        envelope
    }

    /// The result of a command that ran.
    fn ran(&self, name: &str, arguments: &str) -> Value {
        let envelope = self.call(name, arguments);
        assert_eq!(envelope["ok"], true, "{envelope}");
        envelope["result"].clone()
    }

    /// The errors of a command refused.
    fn refused(&self, name: &str, arguments: &str) -> Value {
        let envelope = self.call(name, arguments);
        assert_eq!(envelope["ok"], false, "{envelope}");
        envelope["errors"].clone()
    }

    fn revision(&self) -> Revision {
        self.sessions.status(&s1()).session.unwrap().revision
    }

    fn begin(&self) {
        answer(self.sessions.begin(&s1())).result.unwrap();
    }

    /// Begun and rolled back until the session is at revision `n`, then begun: inside a
    /// transaction at `n`, as the design's examples are.
    fn inside_at(&self, n: u64) {
        while self.revision() < revision(n) {
            self.begin();
            answer(self.sessions.rollback(&s1())).result.unwrap();
        }
        assert_eq!(self.revision(), revision(n));
        self.begin();
    }

    /// The session's text.
    fn text(&self) -> String {
        answer(self.sessions.text(&s1(), None)).result.unwrap().text
    }

    /// The sketch `id` as the session's document holds it, as JSON.
    fn sketch(&self, id: &str) -> Value {
        let document: Value = serde_json::from_str(&self.text()).unwrap();
        document["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == id)
            .cloned()
            .unwrap()
    }

    /// The current model checked, its rebuild waited for: so no solve is still to come.
    fn checked(&self) -> Value {
        self.ran(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        )
    }
}

/// The IDs of `sketch`'s list `list`, in order.
fn ids(sketch: &Value, list: &str) -> Vec<String> {
    sketch[list]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect()
}

/// The request `sketch_add` takes, at `revision`, of `sketch`, with these lists.
fn add_request(revision: u64, sketch: &str, lists: [&str; 5]) -> String {
    let [points, lines, circles, arcs, constraints] = lists;
    format!(
        r#"{{"session": "s1", "revision": {revision}, "sketch": "{sketch}", "points": [{points}], "lines": [{lines}], "circles": [{circles}], "arcs": [{arcs}], "constraints": [{constraints}]}}"#
    )
}

/// The request `sketch_remove` takes, at `revision`, of `sketch`'s `members`.
fn remove_request(revision: u64, sketch: &str, members: &[&str]) -> String {
    json!({"session": "s1", "revision": revision, "sketch": sketch, "members": members}).to_string()
}

/// The error contract's object, every field given.
fn error(
    message: &str,
    feature: Option<&str>,
    elements: &[&str],
    parameter: &str,
    cause: Option<&str>,
    suggestion: &str,
    candidates: &[&str],
) -> Value {
    json!({
        "code": "invalid_input",
        "message": message,
        "feature": feature,
        "elements": elements,
        "elements_total": elements.len(),
        "parameter": parameter,
        "cause": cause,
        "suggestion": suggestion,
        "candidates": candidates,
    })
}

#[test]
fn the_sketch_edits_examples_are_the_designs_own_text() {
    let add = design("#### `sketch_add`", "#### `sketch_remove`");
    assert_eq!(add.len(), 2);
    for (block, name) in add.iter().zip(["request", "result"]) {
        assert_eq!(format!("{block}\n"), example(&format!("sketch_add/{name}")));
    }
    let remove = design("#### `sketch_remove`", "#### `sketch_split`");
    assert_eq!(remove.len(), 4);
    for (block, name) in
        remove
            .iter()
            .zip(["request", "result", "refused-point-used", "refused-origin"])
    {
        assert_eq!(
            format!("{block}\n"),
            example(&format!("sketch_remove/{name}"))
        );
    }
    // A file for each block, and no other.
    for (dir, want) in [
        ("sketch_add", &["request.json", "result.json"][..]),
        (
            "sketch_remove",
            &[
                "refused-origin.json",
                "refused-point-used.json",
                "request.json",
                "result.json",
            ],
        ),
    ] {
        let mut files: Vec<String> =
            std::fs::read_dir(contract(&format!("commands/examples/{dir}")))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
        files.sort();
        assert_eq!(files, want, "{dir}");
    }
}

#[test]
fn each_request_is_read_and_written_back_and_holds_to_its_schema_and_its_result_too() {
    // sketch_add's: its lists taken whole, as the document's reader will read them.
    let text = example("sketch_add/request");
    let request = SketchAddRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        (request.session, request.revision, request.sketch.as_str()),
        (s1(), revision(12), "s0000002")
    );
    assert_eq!(written(&request), text);
    holds(
        &schema_of("sketch_add", "request"),
        &text,
        "sketch_add request",
    );
    // sketch_remove's: each member by ID.
    let text = example("sketch_remove/request");
    let request = SketchRemoveRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        SketchRemoveRequest {
            session: s1(),
            revision: revision(12),
            sketch: fid("s0000001"),
            members: vec![Reference::new("na000003").unwrap()],
        }
    );
    assert_eq!(written(&request), text);
    holds(
        &schema_of("sketch_remove", "request"),
        &text,
        "sketch_remove request",
    );
    // Each result, and what isn't one; each refusal, the error contract's.
    for of in ["sketch_add", "sketch_remove"] {
        let result = example(&format!("{of}/result"));
        let schema = schema_of(of, "result");
        holds(&schema, &result, of);
        let result_value: Value = serde_json::from_str(&result).unwrap();
        for (key, bad) in [
            ("added", json!([1])),
            ("removed", json!(["Na000003"])),
            ("changed", json!("true")),
            ("commit", json!({})),
            ("sketch", json!(null)),
        ] {
            let mut wrong = result_value.clone();
            wrong[key] = bad;
            fails(&schema, &wrong.to_string(), &format!("{of}'s {key}"));
        }
        let mut extra = result_value.clone();
        extra["state"] = json!("over_defined");
        fails(&schema, &extra.to_string(), &format!("{of} with a state"));
        let mut missing = result_value.as_object().unwrap().clone();
        missing.remove("modified");
        fails(
            &schema,
            &Value::Object(missing).to_string(),
            &format!("{of} without modified"),
        );
    }
    for name in ["refused-point-used", "refused-origin"] {
        holds(
            &error_schema(),
            &example(&format!("sketch_remove/{name}")),
            name,
        );
    }
    // The two share one result's shape, as the design gives them one.
    assert_eq!(
        schema_of("sketch_add", "result"),
        schema_of("sketch_remove", "result")
    );
    assert_eq!(
        schema_of("sketch_add", "result")["title"],
        "SketchEditResult"
    );
    assert_eq!(
        schema_of("sketch_add", "request")["title"],
        "SketchAddRequest"
    );
    assert_eq!(
        schema_of("sketch_remove", "request")["title"],
        "SketchRemoveRequest"
    );
}

#[test]
fn the_designs_sketch_add_on_the_bracket_is_its_result_and_asks_the_solver_nothing() {
    let f = opened(&bracket());
    f.checked();
    let solves = f.sketches.requests().len();
    f.inside_at(12);
    let before = f.sketch("s0000002");
    let envelope = f.call("sketch_add", &example("sketch_add/request"));
    assert_eq!(envelope["ok"], true, "{envelope}");
    let result: Value = serde_json::from_str(&example("sketch_add/result")).unwrap();
    assert_eq!(envelope["result"], result);
    // A new revision, the transaction still open, no model read.
    assert_eq!(
        envelope["session"],
        json!({"id": "s1", "path": f.path().to_str().unwrap(), "revision": 13, "transaction": true, "reloaded": false})
    );
    assert_eq!(envelope["model"], Value::Null);
    // The sketch holds the three, each as given, after its own (each list by ID).
    let after = f.sketch("s0000002");
    assert_eq!(ids(&after, "points"), ["pb000001", "pb000002", "pb000003"]);
    assert_eq!(ids(&after, "circles"), ["cb000001", "cb000002", "cb000003"]);
    assert_eq!(
        after["points"][2],
        json!({"id": "pb000003", "x": 40, "y": 20, "construction": false})
    );
    assert_eq!(
        after["circles"][2],
        json!({"id": "cb000003", "center": "pb000003", "radius": 2.75, "construction": false})
    );
    assert_eq!(
        after["constraints"][6],
        json!({"id": "kb000007", "type": "equal", "a": "cb000001", "b": "cb000003"})
    );
    assert_eq!(ids(&after, "constraints").len(), 7);
    assert_eq!(after["lines"], before["lines"]);
    assert_eq!(after["arcs"], before["arcs"]);
    // The edit solved nothing, inside a transaction (no rebuild starts, decision 13).
    assert_eq!(f.sketches.requests().len(), solves);
    // The typed method's result, written as the API writes it, is the design's own text.
    let typed = answer(f.sessions.sketch_add(
        &s1(),
        revision(13),
        fid("s0000002"),
        NewMembers {
            points: parsed(r#"[{"id": "pb000004", "x": 30, "y": 20, "construction": false}]"#),
            lines: parsed("[]"),
            circles: parsed("[]"),
            arcs: parsed("[]"),
            constraints: parsed("[]"),
        },
    ))
    .result
    .unwrap();
    assert_eq!(
        written(&typed),
        "{\"sketch\": \"s0000002\", \"added\": [\"pb000004\"], \"modified\": [], \"removed\": [], \"changed\": true, \"commit\": null}\n"
    );
    assert_eq!(f.sketches.requests().len(), solves);
}

#[test]
fn the_designs_sketch_remove_and_its_refusals_are_its_text() {
    let f = opened(&bracket());
    f.inside_at(12);
    // The point two lines end at, and the origin: each refused, the session as it was.
    let text = f.text();
    let refused = f.refused(
        "sketch_remove",
        &remove_request(12, "s0000001", &["pa000004"]),
    );
    let want: Value = serde_json::from_str(&example("sketch_remove/refused-point-used")).unwrap();
    assert_eq!(refused, json!([want]));
    let refused = f.refused(
        "sketch_remove",
        &remove_request(12, "s0000001", &["origin"]),
    );
    let want: Value = serde_json::from_str(&example("sketch_remove/refused-origin")).unwrap();
    assert_eq!(refused, json!([want]));
    assert_eq!((f.text(), f.revision()), (text, revision(12)));
    // The design's request: the line, with its horizontal; both its points stay, each an end
    // of a neighbour, with every constraint on them.
    let envelope = f.call("sketch_remove", &example("sketch_remove/request"));
    let result: Value = serde_json::from_str(&example("sketch_remove/result")).unwrap();
    assert_eq!(envelope["result"], result, "{envelope}");
    assert_eq!(envelope["session"]["revision"], 13);
    let after = f.sketch("s0000001");
    assert_eq!(
        ids(&after, "lines"),
        ["na000001", "na000002", "na000004", "na000005", "na000006"]
    );
    assert_eq!(
        ids(&after, "points"),
        [
            "pa000001", "pa000002", "pa000003", "pa000004", "pa000005", "pa000006"
        ]
    );
    let constraints = ids(&after, "constraints");
    assert_eq!(constraints.len(), 11);
    assert!(!constraints.contains(&"ka000003".to_owned()));
    // With the line gone, the point is only na000004's end: refused for that one line.
    let refused = f.refused(
        "sketch_remove",
        &remove_request(13, "s0000001", &["pa000004"]),
    );
    assert_eq!(
        refused,
        json!([error(
            "point pa000004 is used by line na000004",
            Some("s0000001"),
            &["pa000004", "na000004"],
            "members.0",
            Some("a point that curves use is those curves' end"),
            "remove the curves that use it with it, or unjoin it first",
            &[]
        )])
    );
}

#[test]
fn a_point_in_use_is_said_to_be_the_end_or_the_centre_it_is() {
    // The base's hole centres are their circles' centres; a point both an arc's centre and
    // a line's end is either.
    let f = opened(&bracket());
    let refused = f.refused(
        "sketch_remove",
        &remove_request(1, "s0000002", &["pb000001"]),
    );
    assert_eq!(
        refused,
        json!([error(
            "point pb000001 is used by circle cb000001",
            Some("s0000002"),
            &["pb000001", "cb000001"],
            "members.0",
            Some("a point that curves use is those curves' centre"),
            "remove the curves that use it with it, or unjoin it first",
            &[]
        )])
    );
    let added = f.ran(
        "sketch_add",
        &add_request(
            1,
            "s0000002",
            [
                r#"{"id": "pb000005", "x": 50, "y": 12, "construction": false}, {"id": "pb000006", "x": 50, "y": 2, "construction": false}"#,
                r#"{"id": "nb000001", "from": "pb000001", "to": "pb000005", "construction": true}"#,
                "",
                r#"{"id": "ab000001", "center": "pb000005", "start": "pb000006", "end": "pb000001", "construction": true}"#,
                "",
            ],
        ),
    );
    assert_eq!(
        added["added"],
        json!(["pb000005", "pb000006", "nb000001", "ab000001"])
    );
    let refused = f.refused(
        "sketch_remove",
        &remove_request(2, "s0000002", &["pb000005"]),
    );
    assert_eq!(
        refused,
        json!([error(
            "point pb000005 is used by curves nb000001 and ab000001",
            Some("s0000002"),
            &["pb000005", "nb000001", "ab000001"],
            "members.0",
            Some("a point that curves use is those curves' end or centre"),
            "remove the curves that use it with it, or unjoin it first",
            &[]
        )])
    );
}

#[test]
fn outside_a_transaction_each_edit_is_its_own_commit_saved_at_once() {
    let f = opened(&bracket());
    let envelope = f.call(
        "sketch_add",
        &add_request(
            1,
            "s0000002",
            [
                r#"{"id": "pb000003", "x": 40, "y": 20, "construction": false}"#,
                "",
                r#"{"id": "cb000003", "center": "pb000003", "radius": 2.75, "construction": false}"#,
                "",
                r#"{"id": "kb000007", "type": "equal", "a": "cb000001", "b": "cb000003"}"#,
            ],
        ),
    );
    assert_eq!(envelope["ok"], true, "{envelope}");
    let result = &envelope["result"];
    assert_eq!(
        (&result["added"], &result["changed"]),
        (&json!(["pb000003", "cb000003", "kb000007"]), &json!(true))
    );
    // The commit wrote the file. The fake solver gives back the coordinates it was sent, so
    // no solve differs from what's stored, and none is written anew.
    assert_eq!(result["commit"]["written"], true, "{result}");
    assert_eq!(result["commit"]["path"], f.path().to_str().unwrap());
    assert_eq!(result["commit"]["resolved"], json!([]));
    assert_eq!(
        (
            &envelope["session"]["revision"],
            &envelope["session"]["transaction"]
        ),
        (&json!(2), &json!(false))
    );
    let file: Value = serde_json::from_str(&std::fs::read_to_string(f.path()).unwrap()).unwrap();
    assert_eq!(
        ids(&file["features"][2], "circles"),
        ["cb000001", "cb000002", "cb000003"]
    );
    // sketch_remove likewise, the file without them again.
    let envelope = f.call(
        "sketch_remove",
        &remove_request(2, "s0000002", &["cb000003", "kb000007"]),
    );
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(
        envelope["result"]["removed"],
        json!(["pb000003", "cb000003", "kb000007"])
    );
    assert_eq!(envelope["result"]["commit"]["written"], true);
    let file: Value = serde_json::from_str(&std::fs::read_to_string(f.path()).unwrap()).unwrap();
    assert_eq!(
        ids(&file["features"][2], "circles"),
        ["cb000001", "cb000002"]
    );
    assert_eq!(
        ids(&file["features"][2], "points"),
        ["pb000001", "pb000002"]
    );
    assert_eq!(f.revision(), revision(3));
}

#[test]
fn an_edit_of_nothing_changes_nothing_and_writes_nothing() {
    // The only edits that change nothing: an empty batch, and an empty list (the core adds or
    // removes nothing, and every other request that runs changes the sketch).
    let f = opened(&bracket());
    let file = std::fs::read(f.path()).unwrap();
    let nothing = json!({"sketch": "s0000002", "added": [], "modified": [], "removed": [], "changed": false, "commit": null});
    assert_eq!(
        f.ran(
            "sketch_add",
            &add_request(1, "s0000002", ["", "", "", "", ""])
        ),
        nothing
    );
    assert_eq!(
        f.ran("sketch_remove", &remove_request(1, "s0000002", &[])),
        nothing
    );
    assert_eq!(f.revision(), revision(1));
    assert_eq!(std::fs::read(f.path()).unwrap(), file);
    f.begin();
    assert_eq!(
        f.ran(
            "sketch_add",
            &add_request(1, "s0000002", ["", "", "", "", ""])
        ),
        nothing
    );
    // Nothing pushed on the undo stack: there's nothing to undo (session/undo.rs's words).
    let undone = answer(f.sessions.undo(&s1(), revision(1), None));
    let refused = undone.result.unwrap_err();
    let [error] = refused.errors() else {
        panic!("{refused:?}")
    };
    assert_eq!(
        (
            error.code(),
            error.message(),
            error.parameter(),
            error.cause()
        ),
        (
            ErrorCode::InvalidInput,
            "nothing to undo in session s1's transaction",
            Some("session"),
            None
        )
    );
    assert_eq!(f.revision(), revision(1));
}

#[test]
fn an_edit_against_a_stale_revision_is_a_conflict() {
    let f = opened(&bracket());
    f.begin();
    f.ran(
        "sketch_remove",
        &remove_request(1, "s0000002", &["kb000006"]),
    );
    let text = f.text();
    let conflict = |command: &str, request: &str| {
        let errors = f.refused(command, request);
        assert_eq!(
            errors,
            json!([{
                "code": "conflict",
                "message": "the edit was made against revision 1, and the session is at revision 2",
                "feature": null,
                "elements": [],
                "elements_total": 0,
                "parameter": "revision",
                "cause": null,
                "suggestion": "read the document again (text or outline), then make the edit against revision 2",
                "candidates": []
            }]),
            "{command}"
        );
    };
    conflict(
        "sketch_remove",
        &remove_request(1, "s0000002", &["kb000005"]),
    );
    conflict(
        "sketch_add",
        &add_request(1, "s0000002", ["", "", "", "", ""]),
    );
    assert_eq!((f.text(), f.revision()), (text, revision(2)));
}

#[test]
fn each_edit_is_undone_and_redone_byte_for_byte() {
    let f = opened(&bracket());
    f.begin();
    let opened = f.text();
    let added = f.ran(
        "sketch_add",
        &add_request(
            1,
            "s0000002",
            [
                r#"{"id": "pb000003", "x": 40, "y": 20, "construction": false}"#,
                "",
                r#"{"id": "cb000003", "center": "pb000003", "radius": 2.75, "construction": false}"#,
                "",
                "",
            ],
        ),
    );
    assert_eq!(added["commit"], Value::Null);
    let after_add = f.text();
    let removed = f.ran(
        "sketch_remove",
        &remove_request(2, "s0000001", &["na000003"]),
    );
    assert_eq!(removed["removed"], json!(["na000003", "ka000003"]));
    let after_remove = f.text();
    let step = |undo: bool, at: u64| {
        let running = if undo {
            f.sessions.undo(&s1(), revision(at), None)
        } else {
            f.sessions.redo(&s1(), revision(at), None)
        };
        answer(running).result.unwrap()
    };
    step(true, 3);
    assert_eq!(f.text(), after_add);
    step(true, 4);
    assert_eq!(f.text(), opened);
    step(false, 5);
    assert_eq!(f.text(), after_add);
    step(false, 6);
    assert_eq!(f.text(), after_remove);
    assert_eq!(f.revision(), revision(7));
    // Undone, an edit at the new revision drops the redo state, as any edit does.
    step(true, 7);
    f.ran(
        "sketch_remove",
        &remove_request(8, "s0000002", &["kb000006"]),
    );
    let stepped = answer(f.sessions.redo(&s1(), revision(9), None));
    let refused = stepped.result.unwrap_err();
    let [error] = refused.errors() else {
        panic!("{refused:?}")
    };
    assert_eq!(
        (
            error.code(),
            error.message(),
            error.parameter(),
            error.cause()
        ),
        (
            ErrorCode::InvalidInput,
            "nothing to redo in session s1's transaction",
            Some("session"),
            Some("an edit after an undo drops what was undone")
        )
    );
    // Rolled back, the file as it was opened.
    answer(f.sessions.rollback(&s1())).result.unwrap();
    assert_eq!(f.text(), opened);
}

#[test]
fn the_edits_own_refusals_name_their_members_and_the_sketch() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    // An unknown member, each named once at its first place; a duplicate taken once. Its
    // candidates, by the gate's rule for a reference to an ID no member holds (at most two
    // characters off, the nearest first, then by ID, at most five), by hand from the L
    // profile's IDs (pa000001 to pa000006, na000001 to na000006, ka000001 to ka00000c):
    // pa000009 is one character from ka000009 and from each of the six points, the first
    // five by ID taken;
    // kb000001 one from ka000001, and two from every other ka, na, and pa, of which ka000002
    // to ka000005 come first by ID. zzzzzzzz is near none.
    let near = |id: &str| {
        format!(
            "name a point, curve, or constraint of this sketch by its ID: the candidates are this sketch's IDs a character or two from {id}, none taken for it"
        )
    };
    assert_eq!(
        f.refused(
            "sketch_remove",
            &remove_request(
                1,
                "s0000001",
                &["na000003", "pa000009", "pa000009", "kb000001", "zzzzzzzz"]
            )
        ),
        json!([
            error(
                "the sketch has no member pa000009",
                Some("s0000001"),
                &[],
                "members.1",
                None,
                &near("pa000009"),
                &["ka000009", "pa000001", "pa000002", "pa000003", "pa000004"]
            ),
            error(
                "the sketch has no member kb000001",
                Some("s0000001"),
                &[],
                "members.3",
                None,
                &near("kb000001"),
                &["ka000001", "ka000002", "ka000003", "ka000004", "ka000005"]
            ),
            error(
                "the sketch has no member zzzzzzzz",
                Some("s0000001"),
                &[],
                "members.4",
                None,
                "name a point, curve, or constraint of this sketch by its ID",
                &[]
            )
        ])
    );
    // Each fixed reference, refused at its place, in one answer with every other problem,
    // each said once, at its first place.
    assert_eq!(
        f.refused(
            "sketch_remove",
            &remove_request(
                1,
                "s0000001",
                &["pa000009", "x_axis", "y_axis", "x_axis", "pa000005"]
            )
        ),
        json!([
            error(
                "the sketch has no member pa000009",
                Some("s0000001"),
                &[],
                "members.0",
                None,
                &near("pa000009"),
                &["ka000009", "pa000001", "pa000002", "pa000003", "pa000004"]
            ),
            error(
                "x_axis is one of the sketch's fixed references, which can't be removed",
                Some("s0000001"),
                &[],
                "members.1",
                None,
                "remove the constraints that name it instead",
                &[]
            ),
            error(
                "y_axis is one of the sketch's fixed references, which can't be removed",
                Some("s0000001"),
                &[],
                "members.2",
                None,
                "remove the constraints that name it instead",
                &[]
            ),
            error(
                "point pa000005 is used by lines na000004 and na000005",
                Some("s0000001"),
                &["pa000005", "na000004", "na000005"],
                "members.4",
                Some("a point that curves use is those curves' end"),
                "remove the curves that use it with it, or unjoin it first",
                &[]
            )
        ])
    );
    assert_eq!(f.text(), text);
    // A line named twice, with its own constraint named too: each once.
    assert_eq!(
        f.ran(
            "sketch_remove",
            &remove_request(1, "s0000001", &["na000003", "ka000003", "na000003"])
        )["removed"],
        json!(["na000003", "ka000003"])
    );
}

#[test]
fn an_arcs_start_or_end_alone_is_said_to_be_its_end() {
    // An arc's points are its centre, then its ends (`curves`' order): a point only an arc's
    // start or end is that arc's end, its centre its centre (point_in_use's words, by hand).
    let f = opened(&bracket());
    f.begin();
    f.ran(
        "sketch_add",
        &add_request(
            1,
            "s0000002",
            [
                r#"{"id": "pz000001", "x": 50, "y": 10, "construction": true}, {"id": "pz000002", "x": 55, "y": 10, "construction": true}, {"id": "pz000003", "x": 50, "y": 15, "construction": true}"#,
                "",
                "",
                r#"{"id": "az000001", "center": "pz000001", "start": "pz000002", "end": "pz000003", "construction": true}"#,
                "",
            ],
        ),
    );
    let text = f.text();
    for (point, role) in [
        ("pz000002", "end"),
        ("pz000003", "end"),
        ("pz000001", "centre"),
    ] {
        assert_eq!(
            f.refused("sketch_remove", &remove_request(2, "s0000002", &[point])),
            json!([error(
                &format!("point {point} is used by arc az000001"),
                Some("s0000002"),
                &[point, "az000001"],
                "members.0",
                Some(&format!("a point that curves use is those curves' {role}")),
                "remove the curves that use it with it, or unjoin it first",
                &[]
            )]),
            "{point}"
        );
    }
    assert_eq!((f.text(), f.revision()), (text, revision(2)));
}

#[test]
fn a_removals_modified_and_removed_are_by_kind_then_by_id_whatever_the_requests_order() {
    // Two lines whose ends their neighbours keep, each with a midpoint, the lower ID on the
    // line named first, and a lone point between them in the request: each midpoint becomes
    // a symmetric of its line's ends, in `modified` by ID; `removed` holds the point, then
    // the lines, then the constraints naming them (ka000001 and ka000003, their horizontals,
    // and ka000009, na000001's length), each kind by ID.
    let f = opened(&bracket());
    f.begin();
    f.ran(
        "sketch_add",
        &add_request(
            1,
            "s0000001",
            [
                r#"{"id": "pa000007", "x": 30, "y": 0, "construction": false}, {"id": "pa000008", "x": 32.5, "y": 5, "construction": false}, {"id": "pa000009", "x": 20, "y": 20, "construction": true}"#,
                "",
                "",
                "",
                r#"{"id": "ka00000e", "type": "midpoint", "point": "pa000007", "line": "na000001"}, {"id": "ka00000d", "type": "midpoint", "point": "pa000008", "line": "na000003"}"#,
            ],
        ),
    );
    assert_eq!(
        f.ran(
            "sketch_remove",
            &remove_request(2, "s0000001", &["na000003", "pa000009", "na000001"])
        ),
        json!({
            "sketch": "s0000001",
            "added": [],
            "modified": ["ka00000d", "ka00000e"],
            "removed": ["pa000009", "na000001", "na000003", "ka000001", "ka000003", "ka000009"],
            "changed": true,
            "commit": null
        })
    );
}

#[test]
fn an_unknown_sketch_in_a_document_of_six_has_the_first_five_sketches_as_candidates() {
    // The candidates are the document's first sketches in timeline order, at most five
    // (`CANDIDATES`): of six, out of ID order, the sixth (s0000003) is left out.
    let order = [
        "s0000006", "s0000002", "s0000005", "s0000001", "s0000004", "s0000003",
    ];
    let sketches: Vec<String> = order
        .iter()
        .map(|id| {
            format!(
                r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#
            )
        })
        .collect();
    let f = opened(&format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        sketches.join(", ")
    ));
    let want = json!([error(
        "the document has no feature s0000009",
        None,
        &[],
        "sketch",
        None,
        "name a sketch of the document, by its ID",
        &order[..5]
    )]);
    assert_eq!(
        f.refused("sketch_remove", &remove_request(1, "s0000009", &[])),
        want
    );
    assert_eq!(
        f.refused(
            "sketch_add",
            &add_request(1, "s0000009", ["", "", "", "", ""])
        ),
        want
    );
}

#[test]
fn an_add_is_refused_for_an_id_the_sketch_holds_and_by_the_gate_at_each_members_path() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    // An ID the sketch holds, at the field the request named; one the request gives twice,
    // at the later's place.
    assert_eq!(
        f.refused(
            "sketch_add",
            &add_request(
                1,
                "s0000002",
                [
                    r#"{"id": "pb000001", "x": 1, "y": 1, "construction": false}, {"id": "pb000009", "x": 2, "y": 2, "construction": false}, {"id": "pb000009", "x": 3, "y": 3, "construction": false}"#,
                    "",
                    "",
                    "",
                    "",
                ]
            )
        ),
        json!([
            error(
                "id pb000001 is already the ID of point pb000001 (points.pb000001)",
                Some("s0000002"),
                &["pb000001"],
                "points.pb000001.id",
                None,
                "give this point an ID of its own",
                &[]
            ),
            error(
                "id pb000009 is already the ID of point pb000009 (points.pb000009), earlier in this request",
                Some("s0000002"),
                &["pb000009"],
                "points.2.id",
                None,
                "give this point an ID of its own",
                &[]
            )
        ])
    );
    // The gate's, by ID path, the request's path for it too.
    assert_eq!(
        f.refused(
            "sketch_add",
            &add_request(
                1,
                "s0000002",
                [
                    r#"{"id": "pb000003", "x": 40, "y": 20, "construction": false}"#,
                    "",
                    r#"{"id": "cb000003", "center": "pb000003", "radius": 0, "construction": false}"#,
                    "",
                    "",
                ]
            )
        ),
        json!([error(
            "circle cb000003's radius is 0 mm: arcs and circles have a radius above 1e-7 mm, as the kernel measures them",
            Some("s0000002"),
            &["cb000003"],
            "circles.cb000003.radius",
            None,
            "draw the circle larger",
            &[]
        )])
    );
    // A member the document's reader refuses: named at its path, with the sketch.
    assert_eq!(
        f.refused(
            "sketch_add",
            &add_request(
                1,
                "s0000002",
                [
                    r#"{"id": "pb000003", "x": "40", "y": 20, "construction": false}"#,
                    "",
                    "",
                    "",
                    "",
                ]
            )
        ),
        json!([error(
            "points.pb000003.x is a string, not a number in millimetres",
            Some("s0000002"),
            &[],
            "points.pb000003.x",
            None,
            "write points.pb000003.x as a number in millimetres",
            &[]
        )])
    );
    assert_eq!((f.text(), f.revision()), (text, revision(1)));
}

#[test]
fn a_sketch_named_that_isnt_one_is_refused_naming_sketch_with_the_documents_sketches() {
    let f = opened(&bracket());
    let sketches = ["s0000001", "s0000002", "s0000003"];
    for (sketch, message) in [
        ("p0000001", "p0000001 is a pad, not a sketch"),
        ("h0000001", "h0000001 is a pocket, not a sketch"),
        ("zzzzzzzz", "the document has no feature zzzzzzzz"),
    ] {
        let want = json!([error(
            message,
            None,
            &[],
            "sketch",
            None,
            "name a sketch of the document, by its ID",
            &sketches
        )]);
        assert_eq!(
            f.refused("sketch_remove", &remove_request(1, sketch, &["na000003"])),
            want
        );
        assert_eq!(
            f.refused("sketch_add", &add_request(1, sketch, ["", "", "", "", ""])),
            want
        );
    }
    // A document with no sketch says so.
    let f = opened(&format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#
    ));
    assert_eq!(
        f.refused("sketch_remove", &remove_request(1, "k7f2qx3m", &[])),
        json!([error(
            "k7f2qx3m is a box, not a sketch",
            None,
            &[],
            "sketch",
            None,
            "the document has no sketch: add one, and edit it by its ID",
            &[]
        )])
    );
}

#[test]
fn malformed_requests_are_refused_by_the_wire_each_field_named() {
    let f = opened(&bracket());
    let invalid = |message: &str, parameter: &str, suggestion: Option<&str>| {
        json!({
            "code": "invalid_input",
            "message": message,
            "feature": null,
            "elements": [],
            "elements_total": 0,
            "parameter": parameter,
            "cause": null,
            "suggestion": suggestion,
            "candidates": []
        })
    };
    let missing = |field: &str| {
        invalid(
            &format!("{field} is missing"),
            field,
            Some("give every field the request's shape has; null where it allows none"),
        )
    };
    // Every field missing, each named, in the shape's order.
    assert_eq!(
        f.refused("sketch_add", "{}"),
        json!(
            [
                "session",
                "revision",
                "sketch",
                "points",
                "lines",
                "circles",
                "arcs",
                "constraints"
            ]
            .map(missing)
        )
    );
    assert_eq!(
        f.refused("sketch_remove", "{}"),
        json!(["session", "revision", "sketch", "members"].map(missing))
    );
    // A field it hasn't.
    let mut request: Value = serde_json::from_str(&example("sketch_remove/request")).unwrap();
    request["ids"] = json!(null);
    assert_eq!(
        f.refused("sketch_remove", &request.to_string()),
        json!([invalid(
            "ids isn't a field of this request",
            "ids",
            Some("leave it out: a request has only the fields its shape gives")
        )])
    );
    // Lists that aren't, and members that aren't IDs.
    assert_eq!(
        f.refused(
            "sketch_add",
            r#"{"session": "s1", "revision": 1, "sketch": "s0000002", "points": {}, "lines": [], "circles": "c", "arcs": [], "constraints": null}"#
        ),
        json!([
            invalid("points must be a list of points, got an object", "points", None),
            invalid("circles must be a list of circles, got \"c\"", "circles", None),
            invalid("constraints must be a list of constraints, got null", "constraints", None)
        ])
    );
    assert_eq!(
        f.refused(
            "sketch_remove",
            r#"{"session": "s1", "revision": 1, "sketch": "s0000002", "members": ["na000003", 5, "NA000003", "", null]}"#
        ),
        json!([1, 2, 3, 4].map(|i| {
            let got = ["5", "\"NA000003\"", "\"\"", "null"][i - 1];
            invalid(
                &format!("members.{i} must be a sketch member's ID, got {got}"),
                &format!("members.{i}"),
                Some("name a point, curve, or constraint of the sketch by its ID"),
            )
        }))
    );
    assert_eq!(
        f.refused(
            "sketch_remove",
            r#"{"session": "s1", "revision": 1, "sketch": "s0000002", "members": "na000003"}"#
        ),
        json!([invalid(
            "members must be a list of a sketch's members' IDs, got \"na000003\"",
            "members",
            None
        )])
    );
    // The edit's revision as every edit's.
    assert_eq!(
        f.refused(
            "sketch_remove",
            r#"{"session": "s1", "revision": 0, "sketch": "s0000002", "members": []}"#
        )[0]["parameter"],
        "revision"
    );
    assert_eq!(f.revision(), revision(1));
}

#[test]
fn a_document_the_gate_refuses_is_edited_by_replace_text_alone() {
    // A pad whose sketch is missing: the gate refuses the document, which opens, and only
    // replace_text edits it.
    let f = opened(&format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{S0000002}, {P0000001}]}}"#
    ));
    let want = json!([{
        "code": "invalid_input",
        "message": "session s1's document is one the gate refuses, so only replace_text edits it",
        "feature": null,
        "elements": [],
        "elements_total": 0,
        "parameter": "session",
        "cause": null,
        "suggestion": "read its problems (check), then mend its text with replace_text",
        "candidates": []
    }]);
    assert_eq!(
        f.refused(
            "sketch_remove",
            &remove_request(1, "s0000002", &["kb000006"])
        ),
        want
    );
    assert_eq!(
        f.refused(
            "sketch_add",
            &add_request(1, "s0000002", ["", "", "", "", ""])
        ),
        want
    );
}

#[test]
fn the_results_typed_value_is_what_the_wire_writes() {
    // A removed line's midpoint becomes a symmetric of its ends, in `modified`.
    let f = opened(&bracket());
    let added = answer(f.sessions.sketch_add(
        &s1(),
        revision(1),
        fid("s0000001"),
        NewMembers {
            points: parsed(r#"[{"id": "pa000007", "x": 30, "y": 0, "construction": false}]"#),
            lines: parsed("[]"),
            circles: parsed("[]"),
            arcs: parsed("[]"),
            constraints: parsed(
                r#"[{"id": "ka00000d", "type": "midpoint", "point": "pa000007", "line": "na000001"}]"#,
            ),
        },
    ))
    .result
    .unwrap();
    assert_eq!(added.added.len(), 2);
    let removed: SketchEdited = answer(f.sessions.sketch_remove(
        &s1(),
        revision(2),
        fid("s0000001"),
        vec![Reference::new("na000001").unwrap()],
    ))
    .result
    .unwrap();
    assert_eq!(
        (
            removed.added.is_empty(),
            removed
                .modified
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>(),
            removed
                .removed
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>()
        ),
        (
            true,
            vec!["ka00000d"],
            vec!["na000001", "ka000001", "ka000009"]
        )
    );
    let after = f.sketch("s0000001");
    let symmetric = after["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "ka00000d")
        .unwrap()
        .clone();
    assert_eq!(
        symmetric,
        json!({"id": "ka00000d", "type": "symmetric", "a": "pa000001", "b": "pa000002", "about": "pa000007"})
    );
}
