//! `sketch_split`, `sketch_trim`, `sketch_unjoin`, and `sketch_merge` (docs/design/m5.0-app.md,
//! section 7, *The sketch edits*), through the sessions' own command called by name, as a
//! transport calls it (`Sessions::call`), on the fake kernel and the fake sketch solver.
//! Expected values: the design's examples, byte for byte (each file the design's own block);
//! what each edit does to the bracket's L profile and to the port note's trim example (its
//! §4, as sketch `s0000004`), by hand from the design's rules and the note's; the refusals,
//! written here by hand as exact JSON. These edits write geometry, so the sketch's solve
//! judges each (SD7): the fake solver's answers stand in for a conflict, a failure, and a
//! stop here; PlaneGCS's own verdicts are in silico-sketch's
//! tests/session_sketch_geometry_edits.rs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::wire::sketch_edits::{
    SketchMergeRequest, SketchSplitRequest, SketchTrimRequest, SketchUnjoinRequest,
};
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{Answered, Call, Revision, SessionCommand, SessionId, Sessions};
use silico_core::document::json::{Node, read};
use silico_core::document::{ElementId, FeatureId};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_protocol::{SketchGeometry, SketchSystem, SolveStatus, Solved};
use tempfile::TempDir;

use crate::bracket::{F0000001, H0000001, H0000002, P0000001, S0000001, S0000002, S0000003};
use crate::sessions::{HANG, answer};

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract")
        .join(name)
}

/// The committed example `name` (`sketch_split/request`), read and never written.
fn example(name: &str) -> String {
    let path = contract(&format!("commands/examples/{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn example_value(name: &str) -> Value {
    serde_json::from_str(&example(name)).unwrap()
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

/// A document of `features`.
fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

/// The bracket's first five features (M4.9's, as its session added them): the L profile on
/// xz, the pad, the base's holes on xy, the pocket through them, and the upright's holes.
fn bracket() -> String {
    document(&[S0000001, P0000001, S0000002, H0000001, S0000003])
}

/// The port note's trim example (§4, 1.1.3) as sketch `s0000004`, the design's
/// `sketch_trim` (its members listed there), with `more` constraints after its own.
fn trim_sketch(more: &str) -> String {
    let point = |id: &str, x: i32, y: i32| {
        format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
    };
    let line = |id: &str, from: &str, to: &str| {
        format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#)
    };
    let points = [
        point("pd000001", 0, 0),
        point("pd000002", 10, 0),
        point("pd000003", 3, -2),
        point("pd000004", 3, 2),
        point("pd000005", 7, 0),
        point("pd000006", 7, 3),
        point("pd000007", 5, 0),
        point("pd000008", 9, 0),
        point("pd00000a", 0, 5),
        point("pd00000b", 0, 10),
        point("pd00000c", 10, 10),
    ]
    .join(", ");
    let lines = [
        line("nd000001", "pd000001", "pd000002"),
        line("nd000002", "pd000003", "pd000004"),
        line("nd000003", "pd000005", "pd000006"),
        line("nd000004", "pd00000b", "pd00000c"),
        line("nd000005", "pd000001", "pd00000a"),
    ]
    .join(", ");
    let constraints = [
        r#"{"id": "kd000001", "type": "horizontal", "line": "nd000001"}"#,
        r#"{"id": "kd000002", "type": "length", "line": "nd000001", "value": 10, "driving": true}"#,
        r#"{"id": "kd000003", "type": "equal", "a": "nd000001", "b": "nd000004"}"#,
        r#"{"id": "kd000004", "type": "on", "point": "pd000005", "curve": "nd000001"}"#,
        r#"{"id": "kd000005", "type": "on", "point": "pd000007", "curve": "nd000001"}"#,
        r#"{"id": "kd000006", "type": "on", "point": "pd000008", "curve": "nd000001"}"#,
    ]
    .join(", ");
    let constraints = if more.is_empty() {
        constraints
    } else {
        format!("{constraints}, {more}")
    };
    format!(
        r#"{{"id": "s0000004", "type": "sketch", "v": 1, "name": "Trim example", "plane": "xy", "points": [{points}], "lines": [{lines}], "circles": [], "arcs": [], "constraints": [{constraints}]}}"#
    )
}

/// Sessions on the fake kernel and the fake sketch solver, with `text` open as s1 from
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

/// The member `id` of `sketch`'s list `list`.
fn member<'a>(sketch: &'a Value, list: &str, id: &str) -> &'a Value {
    sketch[list]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {list}: {sketch}"))
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

fn split(revision: u64, sketch: &str, curve: &str, at: Value, ids: Value) -> String {
    json!({"session": "s1", "revision": revision, "sketch": sketch, "curve": curve, "at": at, "ids": ids}).to_string()
}

fn trim(revision: u64, sketch: &str, curve: &str, pick: [f64; 2], ids: Value) -> String {
    json!({"session": "s1", "revision": revision, "sketch": sketch, "curve": curve, "pick": pick, "ids": ids}).to_string()
}

fn unjoin(revision: u64, sketch: &str, point: &str, curve: &str, ids: Value) -> String {
    json!({"session": "s1", "revision": revision, "sketch": sketch, "point": point, "curve": curve, "ids": ids}).to_string()
}

fn merge(revision: u64, sketch: &str, points: &[&str]) -> String {
    json!({"session": "s1", "revision": revision, "sketch": sketch, "points": points}).to_string()
}

/// The error contract's object, every field given but its candidates, which are none.
fn error(
    code: &str,
    message: &str,
    feature: Option<&str>,
    elements: &[&str],
    parameter: Option<&str>,
    cause: Option<&str>,
    suggestion: Option<&str>,
) -> Value {
    json!({
        "code": code,
        "message": message,
        "feature": feature,
        "elements": elements,
        "elements_total": elements.len(),
        "parameter": parameter,
        "cause": cause,
        "suggestion": suggestion,
        "candidates": [],
    })
}

/// `error` with `candidates`.
fn with_candidates(mut error: Value, candidates: &[&str]) -> Value {
    error["candidates"] = json!(candidates);
    error
}

/// How many points `system` sends, the origin among them.
fn points_sent(system: &SketchSystem) -> usize {
    system
        .geometry
        .iter()
        .filter(|g| matches!(g, SketchGeometry::Point(_)))
        .count()
}

/// A solve answered with the constraint first sent in conflict, as PlaneGCS answers one
/// that can't hold.
fn conflicting(system: &SketchSystem) -> Solved {
    let mut solved = FakeSketches::as_sent(system);
    let first = system.constraints.iter().find_map(|c| c.tag());
    solved.conflicting = first.into_iter().collect();
    solved.dofs = -1;
    solved
}

/// A solve no algorithm finished, as PlaneGCS answers one it can't meet.
fn failing(system: &SketchSystem) -> Solved {
    let mut solved = FakeSketches::as_sent(system);
    solved.status = SolveStatus::Failed;
    solved.algorithm = None;
    solved
}

#[test]
fn the_geometry_edits_examples_are_the_designs_own_text() {
    for (name, from, to, files) in [
        (
            "sketch_split",
            "#### `sketch_split`",
            "#### `sketch_trim`",
            &[
                "request",
                "result",
                "refused-off-curve",
                "refused-circle-one-position",
            ][..],
        ),
        (
            "sketch_trim",
            "#### `sketch_trim`",
            "#### `sketch_unjoin`",
            &["request", "result", "refused-symmetric"],
        ),
        (
            "sketch_unjoin",
            "#### `sketch_unjoin`",
            "#### `sketch_merge`",
            &["request", "result", "refused-not-shared"],
        ),
        (
            "sketch_merge",
            "#### `sketch_merge`",
            "## 8. The layout",
            &["request", "result", "refused-both-ends"],
        ),
    ] {
        let blocks = design(from, to);
        assert_eq!(blocks.len(), files.len(), "{name}");
        for (block, file) in blocks.iter().zip(files) {
            assert_eq!(
                format!("{block}\n"),
                example(&format!("{name}/{file}")),
                "{name}/{file}"
            );
        }
        // A file for each block, and no other.
        let mut found: Vec<String> =
            std::fs::read_dir(contract(&format!("commands/examples/{name}")))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
        found.sort();
        let mut want: Vec<String> = files.iter().map(|f| format!("{f}.json")).collect();
        want.sort();
        assert_eq!(found, want, "{name}");
    }
}

#[test]
fn each_request_is_read_and_written_back_and_holds_to_its_schema_and_its_result_too() {
    let text = example("sketch_split/request");
    let request = SketchSplitRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        SketchSplitRequest {
            session: s1(),
            revision: revision(12),
            sketch: fid("s0000001"),
            curve: eid("na000001"),
            at: vec![[30.0, 0.0]],
            ids: Some(vec![eid("pa000007"), eid("na000007"), eid("ka00000d")]),
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("sketch_split", "request"), &text, "split");
    let text = example("sketch_trim/request");
    let request = SketchTrimRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        SketchTrimRequest {
            session: s1(),
            revision: revision(12),
            sketch: fid("s0000004"),
            curve: eid("nd000001"),
            pick: [5.0, 0.0],
            ids: Some(
                ["pd000009", "nd000006", "kd00000a", "kd00000b"]
                    .map(eid)
                    .to_vec()
            ),
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("sketch_trim", "request"), &text, "trim");
    let text = example("sketch_unjoin/request");
    let request = SketchUnjoinRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        SketchUnjoinRequest {
            session: s1(),
            revision: revision(12),
            sketch: fid("s0000001"),
            point: eid("pa000004"),
            curve: eid("na000004"),
            ids: Some(vec![eid("pa000008")]),
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("sketch_unjoin", "request"), &text, "unjoin");
    let text = example("sketch_merge/request");
    let request = SketchMergeRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        SketchMergeRequest {
            session: s1(),
            revision: revision(13),
            sketch: fid("s0000001"),
            points: vec![eid("pa000004"), eid("pa000008")],
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("sketch_merge", "request"), &text, "merge");
    // `ids` null, written back as null, and held by the schema; a request without it isn't.
    let text = r#"{"session": "s1", "revision": 1, "sketch": "s0000001", "curve": "na000001", "at": [[30, 0]], "ids": null}"#;
    let request = SketchSplitRequest::read(&parsed(text)).unwrap();
    assert_eq!(request.ids, None);
    assert_eq!(written(&request), format!("{text}\n"));
    holds(
        &schema_of("sketch_split", "request"),
        text,
        "split, ids null",
    );
    fails(
        &schema_of("sketch_split", "request"),
        r#"{"session": "s1", "revision": 1, "sketch": "s0000001", "curve": "na000001", "at": [[30, 0]]}"#,
        "split without ids",
    );
    fails(
        &schema_of("sketch_trim", "request"),
        r#"{"session": "s1", "revision": 1, "sketch": "s0000004", "curve": "nd000001", "pick": [5], "ids": null}"#,
        "trim with a pick of one number",
    );
    // Each result, and what isn't one; each refusal, the error contract's.
    for of in [
        "sketch_split",
        "sketch_trim",
        "sketch_unjoin",
        "sketch_merge",
    ] {
        let result = example(&format!("{of}/result"));
        let schema = schema_of(of, "result");
        holds(&schema, &result, of);
        let value: Value = serde_json::from_str(&result).unwrap();
        for (key, bad) in [
            ("added", json!([1])),
            ("removed", json!(["Na000003"])),
            ("changed", json!("true")),
            ("commit", json!({})),
            ("sketch", json!(null)),
        ] {
            let mut wrong = value.clone();
            wrong[key] = bad;
            fails(&schema, &wrong.to_string(), &format!("{of}'s {key}"));
        }
        let mut extra = value.clone();
        extra["state"] = json!("over_defined");
        fails(&schema, &extra.to_string(), &format!("{of} with a state"));
        let mut missing = value.as_object().unwrap().clone();
        missing.remove("modified");
        fails(
            &schema,
            &Value::Object(missing).to_string(),
            &format!("{of} without modified"),
        );
        assert_eq!(
            schema["title"],
            match of {
                "sketch_split" => "SketchSplitResult",
                "sketch_trim" => "SketchTrimResult",
                "sketch_unjoin" => "SketchUnjoinResult",
                _ => "SketchMergeResult",
            }
        );
    }
    for of in ["sketch_split", "sketch_trim"] {
        let schema = schema_of(of, "result");
        let mut value = example_value(&format!("{of}/result"));
        value["references"] = json!(["not an id"]);
        fails(&schema, &value.to_string(), &format!("{of}'s references"));
        value["references"] = json!([]);
        value.as_object_mut().unwrap().remove("pieces");
        fails(&schema, &value.to_string(), &format!("{of} without pieces"));
    }
    for name in [
        "sketch_split/refused-off-curve",
        "sketch_split/refused-circle-one-position",
        "sketch_trim/refused-symmetric",
        "sketch_unjoin/refused-not-shared",
        "sketch_merge/refused-both-ends",
    ] {
        holds(&error_schema(), &example(name), name);
    }
}

#[test]
fn the_designs_split_on_the_bracket_is_its_result_and_its_refusals_are_its_text() {
    let f = opened(&bracket());
    f.checked();
    f.inside_at(12);
    let text = f.text();
    // A position 0.25 mm off the line, and a circle given one position: each the design's,
    // the session as it was.
    let refused = f.refused(
        "sketch_split",
        &split(12, "s0000001", "na000001", json!([[30, 0.25]]), json!(null)),
    );
    assert_eq!(
        refused,
        json!([example_value("sketch_split/refused-off-curve")])
    );
    let refused = f.refused(
        "sketch_split",
        &split(
            12,
            "s0000002",
            "cb000001",
            json!([[42.75, 12]]),
            json!(null),
        ),
    );
    assert_eq!(
        refused,
        json!([example_value("sketch_split/refused-circle-one-position")])
    );
    assert_eq!((f.text(), f.revision()), (text, revision(12)));
    let solves = f.sketches.requests().len();
    let envelope = f.call("sketch_split", &example("sketch_split/request"));
    assert_eq!(
        envelope["result"],
        example_value("sketch_split/result"),
        "{envelope}"
    );
    assert_eq!(
        envelope["session"],
        json!({"id": "s1", "path": f.path().to_str().unwrap(), "revision": 13, "transaction": true, "reloaded": false})
    );
    // The edit's solve, and only it: inside a transaction, no rebuild starts.
    assert_eq!(f.sketches.requests().len(), solves + 1);
    // What changes, by the design's list.
    let after = f.sketch("s0000001");
    assert_eq!(
        member(&after, "points", "pa000007"),
        &json!({"id": "pa000007", "x": 30, "y": 0, "construction": false})
    );
    assert_eq!(
        member(&after, "lines", "na000001"),
        &json!({"id": "na000001", "from": "pa000001", "to": "pa000007", "construction": false})
    );
    assert_eq!(
        member(&after, "lines", "na000007"),
        &json!({"id": "na000007", "from": "pa000007", "to": "pa000002", "construction": false})
    );
    assert_eq!(
        member(&after, "constraints", "ka000009"),
        &json!({"id": "ka000009", "type": "distance", "a": "pa000001", "b": "pa000002", "value": 60, "driving": true})
    );
    assert_eq!(
        member(&after, "constraints", "ka00000d"),
        &json!({"id": "ka00000d", "type": "tangent_at", "a": "na000001", "b": "na000007", "at": "pa000007", "direction": "same"})
    );
    assert_eq!(
        member(&after, "constraints", "ka000001"),
        &json!({"id": "ka000001", "type": "horizontal", "line": "na000001"})
    );
    assert_eq!(ids(&after, "constraints").len(), 13);
    assert_eq!(ids(&after, "points").len(), 7);
    assert_eq!(ids(&after, "lines").len(), 7);
}

#[test]
fn the_designs_trim_is_freecads_result_every_member_as_the_design_lists_it() {
    let f = opened(&document(&[&trim_sketch("")]));
    f.inside_at(12);
    let before = f.sketch("s0000004");
    let envelope = f.call("sketch_trim", &example("sketch_trim/request"));
    assert_eq!(
        envelope["result"],
        example_value("sketch_trim/result"),
        "{envelope}"
    );
    let after = f.sketch("s0000004");
    // Every member, by the design's list.
    assert_eq!(
        ids(&after, "points"),
        [
            "pd000001", "pd000002", "pd000003", "pd000004", "pd000005", "pd000006", "pd000007",
            "pd000008", "pd000009", "pd00000a", "pd00000b", "pd00000c"
        ]
    );
    assert_eq!(
        member(&after, "points", "pd000009"),
        &json!({"id": "pd000009", "x": 3, "y": 0, "construction": false})
    );
    for kept in [
        "pd000001", "pd000002", "pd000003", "pd000004", "pd000005", "pd000006", "pd000007",
        "pd000008", "pd00000a", "pd00000b", "pd00000c",
    ] {
        assert_eq!(
            member(&after, "points", kept),
            member(&before, "points", kept),
            "{kept}"
        );
    }
    assert_eq!(
        after["lines"],
        json!([
            {"id": "nd000001", "from": "pd000001", "to": "pd000009", "construction": false},
            {"id": "nd000002", "from": "pd000003", "to": "pd000004", "construction": false},
            {"id": "nd000003", "from": "pd000005", "to": "pd000006", "construction": false},
            {"id": "nd000004", "from": "pd00000b", "to": "pd00000c", "construction": false},
            {"id": "nd000005", "from": "pd000001", "to": "pd00000a", "construction": false},
            {"id": "nd000006", "from": "pd000005", "to": "pd000002", "construction": false}
        ])
    );
    assert_eq!(
        after["constraints"],
        json!([
            {"id": "kd000001", "type": "horizontal", "line": "nd000001"},
            {"id": "kd000002", "type": "distance", "a": "pd000001", "b": "pd000002", "value": 10, "driving": true},
            {"id": "kd000006", "type": "on", "point": "pd000008", "curve": "nd000006"},
            {"id": "kd00000a", "type": "horizontal", "line": "nd000006"},
            {"id": "kd00000b", "type": "on", "point": "pd000009", "curve": "nd000002"}
        ])
    );
}

#[test]
fn the_designs_trim_refusal_of_a_symmetry_about_the_line_is_its_text() {
    let symmetric = r#"{"id": "kd000007", "type": "symmetric", "a": "pd000003", "b": "pd000004", "about": "nd000001"}"#;
    let f = opened(&document(&[&trim_sketch(symmetric)]));
    f.inside_at(12);
    let text = f.text();
    let refused = f.refused("sketch_trim", &example("sketch_trim/request"));
    assert_eq!(
        refused,
        json!([example_value("sketch_trim/refused-symmetric")])
    );
    assert_eq!((f.text(), f.revision()), (text, revision(12)));
}

#[test]
fn the_designs_unjoin_then_merge_on_the_bracket_are_their_results_and_refusals() {
    let f = opened(&bracket());
    f.inside_at(12);
    let opened = f.text();
    // A hole's centre, used by its circle alone.
    let refused = f.refused(
        "sketch_unjoin",
        &unjoin(12, "s0000002", "pb000001", "cb000001", json!(null)),
    );
    assert_eq!(
        refused,
        json!([example_value("sketch_unjoin/refused-not-shared")])
    );
    // A curve that doesn't use the point.
    let refused = f.refused(
        "sketch_unjoin",
        &unjoin(12, "s0000001", "pa000004", "na000001", json!(null)),
    );
    assert_eq!(
        refused,
        json!([error(
            "invalid_input",
            "line na000001 doesn't use point pa000004",
            Some("s0000001"),
            &["pa000004", "na000001"],
            Some("curve"),
            None,
            Some("name a curve that ends at, or is centred on, the point")
        )])
    );
    // Merging na000004's two ends, before the unjoin: the design's refusal.
    let refused = f.refused(
        "sketch_merge",
        &merge(12, "s0000001", &["pa000004", "pa000005"]),
    );
    assert_eq!(
        refused,
        json!([example_value("sketch_merge/refused-both-ends")])
    );
    assert_eq!((f.text(), f.revision()), (opened.clone(), revision(12)));
    let envelope = f.call("sketch_unjoin", &example("sketch_unjoin/request"));
    assert_eq!(
        envelope["result"],
        example_value("sketch_unjoin/result"),
        "{envelope}"
    );
    let after = f.sketch("s0000001");
    assert_eq!(
        member(&after, "points", "pa000008"),
        &json!({"id": "pa000008", "x": 5, "y": 5, "construction": false})
    );
    assert_eq!(
        member(&after, "lines", "na000004"),
        &json!({"id": "na000004", "from": "pa000008", "to": "pa000005", "construction": false})
    );
    assert_eq!(
        member(&after, "lines", "na000003"),
        &json!({"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false})
    );
    // Afterwards, check warns of the two points as ends_not_joined.
    let warnings = f.checked()["warnings"].clone();
    let ends: Vec<&Value> = warnings
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "ends_not_joined")
        .collect();
    assert_eq!(ends.len(), 1, "{warnings}");
    assert_eq!(ends[0]["elements"], json!(["pa000004", "pa000008"]));
    // The design's merge: the unjoin undone, as the document was, byte for byte.
    let envelope = f.call("sketch_merge", &example("sketch_merge/request"));
    assert_eq!(
        envelope["result"],
        example_value("sketch_merge/result"),
        "{envelope}"
    );
    assert_eq!(envelope["session"]["revision"], 14);
    // The document is the one the unjoin began from, in canonical form.
    let text: Value = serde_json::from_str(&f.text()).unwrap();
    assert_eq!(text, serde_json::from_str::<Value>(&opened).unwrap());
}

#[test]
fn ids_given_null_or_of_the_wrong_length() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    // Too few, too many, one the sketch holds, and one given twice: refused, naming `ids`.
    for (ids, message, parameter) in [
        (
            json!(["pa000007", "na000007"]),
            "the split needs 3 new IDs, got 2",
            "ids",
        ),
        (
            json!(["pa000007", "na000007", "ka00000d", "ka00000e"]),
            "the split needs 3 new IDs, got 4",
            "ids",
        ),
        (
            json!(["pa000007", "na000002", "ka00000d"]),
            "the sketch already has a member na000002",
            "ids.1",
        ),
        (
            json!(["pa000007", "na000007", "pa000007"]),
            "ID pa000007 is given twice",
            "ids.2",
        ),
    ] {
        let refused = f.refused(
            "sketch_split",
            &split(1, "s0000001", "na000001", json!([[30, 0]]), ids),
        );
        assert_eq!(refused[0]["message"], message);
        assert_eq!(refused[0]["parameter"], parameter);
        assert_eq!(refused[0]["feature"], "s0000001");
        assert_eq!(refused.as_array().unwrap().len(), 1);
    }
    let refused = f.refused(
        "sketch_split",
        &split(1, "s0000001", "na000001", json!([[30, 0]]), json!([])),
    );
    assert_eq!(
        refused,
        json!([error(
            "invalid_input",
            "the split needs 3 new IDs, got 0",
            Some("s0000001"),
            &[],
            Some("ids"),
            None,
            Some(
                "give one ID for each member the split adds, in order: its new points along the curve, its new piece, then a line's join; or null, for the core to make them"
            )
        )])
    );
    let refused = f.refused(
        "sketch_unjoin",
        &unjoin(
            1,
            "s0000001",
            "pa000004",
            "na000004",
            json!(["pa000008", "pa000009"]),
        ),
    );
    assert_eq!(refused[0]["message"], "the unjoin needs 1 new IDs, got 2");
    assert_eq!(refused[0]["parameter"], "ids");
    let refused = f.refused(
        "sketch_trim",
        &trim(1, "s0000001", "na000001", [30.0, 0.0], json!(["pa000009"])),
    );
    assert_eq!(refused[0]["message"], "the trim needs 0 new IDs, got 1");
    assert_eq!((f.text(), f.revision()), (text.clone(), revision(1)));
    // Null: the core's IDs, fresh, distinct, none the sketch's, in added's order.
    let result = f.ran(
        "sketch_split",
        &split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
    );
    let added: Vec<String> = result["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(added.len(), 3, "{result}");
    let before: Value = serde_json::from_str(&text).unwrap();
    let held = &before["features"][0];
    for (i, id) in added.iter().enumerate() {
        assert!(ElementId::new(id).is_some(), "{id}");
        assert!(!added[..i].contains(id), "{added:?}");
        for list in ["points", "lines", "constraints"] {
            assert!(!ids(held, list).contains(id), "{id} in {list}");
        }
    }
    assert_eq!(result["pieces"], json!(["na000001", added[1]]));
    let after = f.sketch("s0000001");
    assert_eq!(member(&after, "points", &added[0])["x"], 30);
    assert_eq!(
        member(&after, "lines", &added[1])["from"],
        added[0].as_str()
    );
    assert_eq!(
        member(&after, "constraints", &added[2])["type"],
        "tangent_at"
    );
    let split_text = f.text();
    // A replay passing back `added` gives the recorded document byte for byte.
    let g = opened(&bracket());
    g.begin();
    let replayed = g.ran(
        "sketch_split",
        &split(1, "s0000001", "na000001", json!([[30, 0]]), json!(added)),
    );
    assert_eq!(replayed, result);
    assert_eq!(g.text(), split_text);
    // And an unjoin's one new point, null.
    let unjoined = f.ran(
        "sketch_unjoin",
        &unjoin(2, "s0000001", "pa000004", "na000004", json!(null)),
    );
    let point = unjoined["added"][0].as_str().unwrap().to_owned();
    assert!(ElementId::new(&point).is_some());
    assert!(!added.contains(&point));
    assert_eq!(
        member(&f.sketch("s0000001"), "lines", "na000004")["from"],
        point.as_str()
    );
}

#[test]
fn outside_a_transaction_each_edit_is_its_own_commit_and_inside_one_commit_is_null() {
    let f = opened(&bracket());
    let result = f.ran(
        "sketch_split",
        &split(
            1,
            "s0000001",
            "na000001",
            json!([[30, 0]]),
            json!(["pa000007", "na000007", "ka00000d"]),
        ),
    );
    assert_eq!(result["commit"]["written"], true, "{result}");
    assert_eq!(result["commit"]["path"], f.path().to_str().unwrap());
    let file: Value = serde_json::from_str(&std::fs::read_to_string(f.path()).unwrap()).unwrap();
    assert!(ids(&file["features"][0], "lines").contains(&"na000007".to_owned()));
    assert_eq!(f.revision(), revision(2));
    let result = f.ran(
        "sketch_unjoin",
        &unjoin(2, "s0000001", "pa000004", "na000004", json!(["pa000008"])),
    );
    assert_eq!(result["commit"]["written"], true);
    let result = f.ran(
        "sketch_merge",
        &merge(3, "s0000001", &["pa000004", "pa000008"]),
    );
    assert_eq!(result["commit"]["written"], true);
    let result = f.ran(
        "sketch_trim",
        &trim(4, "s0000001", "na000007", [45.0, 0.0], json!([])),
    );
    // Nothing cuts na000007 (its neighbours share its ends): removed whole.
    assert_eq!(
        (
            &result["pieces"],
            &result["removed"],
            &result["commit"]["written"]
        ),
        (&json!([]), &json!(["na000007", "ka00000d"]), &json!(true)),
        "{result}"
    );
    let file: Value = serde_json::from_str(&std::fs::read_to_string(f.path()).unwrap()).unwrap();
    assert!(!ids(&file["features"][0], "lines").contains(&"na000007".to_owned()));
    // Inside one, null, and nothing written until the commit.
    let saved = std::fs::read(f.path()).unwrap();
    f.begin();
    let result = f.ran(
        "sketch_split",
        &split(5, "s0000001", "na000003", json!([[30, 5]]), json!(null)),
    );
    assert_eq!(result["commit"], Value::Null);
    assert_eq!(std::fs::read(f.path()).unwrap(), saved);
}

#[test]
fn an_edit_against_a_stale_revision_is_a_conflict() {
    let f = opened(&bracket());
    f.begin();
    f.ran(
        "sketch_split",
        &split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
    );
    let text = f.text();
    let conflict = json!([{
        "code": "conflict",
        "message": "the edit was made against revision 1, and the session is at revision 2",
        "feature": null,
        "elements": [],
        "elements_total": 0,
        "parameter": "revision",
        "cause": null,
        "suggestion": "read the document again (text or outline), then make the edit against revision 2",
        "candidates": []
    }]);
    for (command, request) in [
        (
            "sketch_split",
            split(1, "s0000001", "na000003", json!([[30, 5]]), json!(null)),
        ),
        (
            "sketch_trim",
            trim(1, "s0000001", "na000003", [30.0, 5.0], json!(null)),
        ),
        (
            "sketch_unjoin",
            unjoin(1, "s0000001", "pa000004", "na000004", json!(null)),
        ),
        (
            "sketch_merge",
            merge(1, "s0000001", &["pa000004", "pa000005"]),
        ),
    ] {
        assert_eq!(f.refused(command, &request), conflict, "{command}");
    }
    assert_eq!((f.text(), f.revision()), (text, revision(2)));
}

#[test]
fn each_edit_is_undone_and_redone_byte_for_byte() {
    let f = opened(&bracket());
    f.begin();
    let mut states = vec![f.text()];
    f.ran(
        "sketch_split",
        &split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
    );
    states.push(f.text());
    f.ran(
        "sketch_unjoin",
        &unjoin(2, "s0000001", "pa000004", "na000004", json!(null)),
    );
    states.push(f.text());
    let point = f.sketch("s0000001")["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == "na000004")
        .unwrap()["from"]
        .as_str()
        .unwrap()
        .to_owned();
    f.ran("sketch_merge", &merge(3, "s0000001", &["pa000004", &point]));
    states.push(f.text());
    f.ran(
        "sketch_trim",
        &trim(4, "s0000001", "na000003", [30.0, 5.0], json!(null)),
    );
    states.push(f.text());
    // Undone one by one, then redone: each state back byte for byte, at a new revision.
    let mut at = 5;
    for state in states.iter().rev().skip(1) {
        answer(f.sessions.undo(&s1(), revision(at), None))
            .result
            .unwrap();
        at += 1;
        assert_eq!(&f.text(), state);
    }
    for state in states.iter().skip(1) {
        answer(f.sessions.redo(&s1(), revision(at), None))
            .result
            .unwrap();
        at += 1;
        assert_eq!(&f.text(), state);
    }
    assert_eq!(f.revision(), revision(at));
    // The merge undid the unjoin: those two states are one text.
    assert_eq!(states[3], states[1]);
}

/// The judgment's cause, as every refusal for a solve worse than before says it.
const WORSE: &str = "an edit that writes geometry is refused where the sketch's solve after it is worse than before it (a conflict the sketch had already isn't the edit's), as FreeCAD's Coincident command rolls one back";

/// The fake answering every solve of the L profile but its system as first sent (the
/// sketch before an edit) by `edited`, and every other as solved: so only an edit's solve
/// is `edited`'s.
fn answering_edits(f: &Fixture, edited: fn(&SketchSystem) -> Solved) {
    let base = f
        .sketches
        .requests()
        .into_iter()
        .find(|s| points_sent(s) == 7)
        .expect("the L profile solved");
    f.sketches.answer(move |system| {
        Ok(if points_sent(system) >= 5 && *system != base {
            edited(system)
        } else {
            FakeSketches::as_sent(system)
        })
    });
}

/// The first `n` constraints sent, by tag.
fn first_tags(system: &SketchSystem, n: usize) -> Vec<silico_protocol::Tag> {
    system
        .constraints
        .iter()
        .filter_map(|c| c.tag())
        .take(n)
        .collect()
}

/// Every unknown at 0: every line of no length, a solve the file can't hold.
fn collapsed(system: &SketchSystem) -> Solved {
    let mut solved = FakeSketches::as_sent(system);
    for v in &system.unknowns {
        solved.values[v.0 as usize] = 0.0;
    }
    solved
}

#[test]
fn a_split_trim_or_unjoin_whose_solve_conflicts_or_fails_where_the_sketch_didnt_is_refused() {
    for inside in [true, false] {
        let f = opened(&bracket());
        f.checked();
        if inside {
            f.begin();
        }
        let text = f.text();
        let file = std::fs::read(f.path()).unwrap();
        answering_edits(&f, conflicting);
        // The fake puts the first constraint sent in conflict: the first by ID the edited
        // sketch holds, ka000001, but after the trim, which removes it with na000001.
        for (command, request, named) in [
            (
                "sketch_split",
                split(
                    1,
                    "s0000001",
                    "na000001",
                    json!([[30, 0]]),
                    json!(["pa000007", "na000007", "ka00000d"]),
                ),
                "ka000001",
            ),
            (
                "sketch_trim",
                // Nothing cuts na000001 but its neighbours, at its ends: removed whole.
                trim(1, "s0000001", "na000001", [30.0, 0.0], json!(null)),
                "ka000002",
            ),
            (
                "sketch_unjoin",
                unjoin(1, "s0000001", "pa000004", "na000004", json!(null)),
                "ka000001",
            ),
        ] {
            let refused = f.refused(command, &request);
            let edit = command.trim_start_matches("sketch_");
            assert_eq!(
                refused,
                json!([error(
                    "over_constrained",
                    &format!(
                        "after the {edit}, the sketch's constraints conflict: these 1 can't all hold"
                    ),
                    Some("s0000001"),
                    &[named],
                    None,
                    Some(WORSE),
                    Some("remove or change one of the constraints named")
                )]),
                "{command}, inside: {inside}"
            );
            assert_eq!((f.text(), f.revision()), (text.clone(), revision(1)));
            assert_eq!(std::fs::read(f.path()).unwrap(), file);
        }
        answering_edits(&f, failing);
        let refused = f.refused(
            "sketch_split",
            &split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
        );
        assert_eq!(refused[0]["code"], "solve_failed", "{refused}");
        assert_eq!(
            refused[0]["message"],
            "after the split, no solver could meet the sketch's constraints"
        );
        assert_eq!(refused[0]["feature"], "s0000001");
        assert_eq!((f.text(), f.revision()), (text.clone(), revision(1)));
        assert_eq!(std::fs::read(f.path()).unwrap(), file);
        // A result the file can't hold: always refused, whatever the sketch before was.
        f.sketches.answer(|system| Ok(collapsed(system)));
        let refused = f.refused(
            "sketch_unjoin",
            &unjoin(1, "s0000001", "pa000004", "na000004", json!(null)),
        );
        assert_eq!(refused[0]["code"], "invalid_result", "{refused}");
        assert!(
            refused[0]["message"]
                .as_str()
                .unwrap()
                .starts_with("after the unjoin, "),
            "{refused}"
        );
        assert_eq!((f.text(), f.revision()), (text.clone(), revision(1)));
        // The solver well again, the same request is made.
        f.sketches
            .answer(|system| Ok(FakeSketches::as_sent(system)));
        f.ran(
            "sketch_split",
            &split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
        );
    }
}

#[test]
fn a_conflict_the_sketch_had_already_isnt_the_edits_but_one_it_draws_a_new_constraint_into_is() {
    // The fake puts every solve's first constraint sent in conflict, the sketch's before too:
    // a split and an unjoin leave that one, ka000001, in conflict, as before them (FreeCAD's
    // rollback: same rank, nothing newly flagged), and are made; the trim, which removes
    // ka000001 with its line, flags ka000002, which the solve before didn't: refused.
    let f = opened(&bracket());
    f.begin();
    f.sketches.answer(|system| Ok(conflicting(system)));
    let made = f.ran(
        "sketch_split",
        &split(
            1,
            "s0000001",
            "na000003",
            json!([[30, 5]]),
            json!(["pa000007", "na000007", "ka00000d"]),
        ),
    );
    assert_eq!(made["changed"], true);
    let unjoined = f.ran(
        "sketch_unjoin",
        &unjoin(2, "s0000001", "pa000004", "na000004", json!(null)),
    );
    assert_eq!(unjoined["changed"], true);
    let refused = f.refused(
        "sketch_trim",
        &trim(3, "s0000001", "na000001", [30.0, 0.0], json!(null)),
    );
    assert_eq!(refused[0]["code"], "over_constrained", "{refused}");
    assert_eq!(refused[0]["elements"], json!(["ka000002"]));
    assert_eq!(refused[0]["cause"], WORSE);
}

#[test]
fn a_conflict_that_leaves_no_freedom_ranks_worse_than_one_that_does() {
    // FreeCAD main's getCoincidentSolveStatus reads a negative DoF count first: the same
    // constraint in conflict, with DoF 0 before (ConflictingConstraints, 2) and -1 after
    // (Overconstrained, 3), is worse, and refused; the other way round, it's made.
    fn conflict_with(system: &SketchSystem, dofs: i32) -> Solved {
        let mut solved = FakeSketches::as_sent(system);
        solved.conflicting = first_tags(system, 1);
        solved.dofs = dofs;
        solved
    }
    fn free(system: &SketchSystem) -> Solved {
        conflict_with(system, 0)
    }
    fn bound(system: &SketchSystem) -> Solved {
        conflict_with(system, -1)
    }
    let judged = |before: fn(&SketchSystem) -> Solved, after: fn(&SketchSystem) -> Solved| {
        let f = opened(&bracket());
        f.begin();
        f.sketches.answer(move |system| {
            Ok(match points_sent(system) {
                7 => before(system),
                8 => after(system),
                _ => FakeSketches::as_sent(system),
            })
        });
        f.call(
            "sketch_split",
            &split(
                1,
                "s0000001",
                "na000003",
                json!([[30, 5]]),
                json!(["pa000007", "na000007", "ka00000d"]),
            ),
        )
    };
    let envelope = judged(free, bound);
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(
        envelope["errors"][0]["message"],
        "after the split, the sketch's constraints conflict: these 1 can't all hold"
    );
    let envelope = judged(bound, free);
    assert_eq!(envelope["ok"], true, "{envelope}");
}

#[test]
fn a_merge_is_refused_only_where_its_solve_is_worse_than_the_one_before() {
    // The fake answers the merged sketch (the L profile's six points, and the origin, sent)
    // as `after` says, and, `before_too`, the unjoined one before it (seven, and the
    // origin) too; every other (the holes' two, and the origin) as solved.
    let answering = |f: &Fixture, after: fn(&SketchSystem) -> Solved, before_too: bool| {
        f.sketches.answer(move |system| {
            let points = points_sent(system);
            Ok(if points == 7 || (before_too && points == 8) {
                after(system)
            } else {
                FakeSketches::as_sent(system)
            })
        });
    };
    let f = opened(&bracket());
    f.begin();
    f.ran(
        "sketch_unjoin",
        &unjoin(1, "s0000001", "pa000004", "na000004", json!(["pa000008"])),
    );
    let text = f.text();
    let request = merge(2, "s0000001", &["pa000004", "pa000008"]);
    // A conflict the solve before didn't find: refused, naming it.
    answering(&f, conflicting, false);
    let refused = f.refused("sketch_merge", &request);
    assert_eq!(
        refused,
        json!([error(
            "over_constrained",
            "after the merge, the sketch's constraints conflict: these 1 can't all hold",
            Some("s0000001"),
            &["ka000001"],
            None,
            Some(WORSE),
            Some("remove or change one of the constraints named")
        )])
    );
    assert_eq!(f.text(), text);
    // A solve that fails where the one before solved: refused, with the lever.
    answering(&f, failing, false);
    let refused = f.refused("sketch_merge", &request);
    assert_eq!(refused[0]["code"], "solve_failed", "{refused}");
    assert_eq!(
        refused[0]["suggestion"],
        "name first the point the constraints hold, or move the points together, then merge"
    );
    assert_eq!(f.text(), text);
    // The same conflict before it as after: not worse, so it's made, as FreeCAD's rollback
    // makes it.
    answering(&f, conflicting, true);
    let merged = f.ran("sketch_merge", &request);
    assert_eq!(merged["removed"], json!(["pa000008"]));
}

#[test]
fn cancel_naming_the_edit_stops_its_solve_and_leaves_the_session_as_it_was() {
    let f = opened(&bracket());
    f.checked();
    f.begin();
    let text = f.text();
    f.sketches.hold();
    let request = split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null));
    let running = match f.sessions.call("sketch_split", &request) {
        Call::Running(calling) => calling,
        Call::Unknown => panic!("no sketch_split"),
    };
    assert!(f.sketches.wait_held(HANG), "the edit's solve never came");
    let status = f.sessions.status(&s1()).result.unwrap();
    assert_eq!(
        status.running,
        Some(silico_core::command::RunningCommand::Command(
            SessionCommand::SketchSplit
        ))
    );
    let cancelled = f
        .sessions
        .cancel(&s1(), SessionCommand::SketchSplit)
        .result
        .unwrap();
    assert!(cancelled.stopped);
    let Ok(answered) = running.wait_timeout(HANG) else {
        panic!("sketch_split didn't answer in {HANG:?}")
    };
    let envelope: Value = serde_json::from_str(&answered.text).unwrap();
    assert_eq!(
        envelope["errors"],
        json!([error(
            "cancelled",
            "sketch_split was cancelled before it finished",
            None,
            &[],
            None,
            None,
            Some(
                "run it again when you want its answer; every rebuild step that finished is kept in the cache"
            )
        )])
    );
    assert!(f.sketches.stops() >= 1);
    f.sketches.release();
    assert_eq!((f.text(), f.revision()), (text, revision(1)));
    // The next edit runs.
    let result = f.ran("sketch_split", &request);
    assert_eq!(result["changed"], true);
}

#[test]
fn split_and_trim_list_the_features_storing_a_name_their_curve_reaches() {
    // The bracket's fillet stores p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000004): a
    // name made from pa000004, an end of na000003 and of na000004. A split of na000003, or
    // a trim of na000004, reaches it; a split of na000001, whose points are pa000001 and
    // pa000002, doesn't.
    let f = opened(&document(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001,
    ]));
    f.begin();
    let result = f.ran(
        "sketch_split",
        &split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
    );
    assert_eq!(result["references"], json!([]));
    let result = f.ran(
        "sketch_split",
        &split(2, "s0000001", "na000003", json!([[30, 5]]), json!(null)),
    );
    assert_eq!(result["references"], json!(["f0000001"]));
    // The name the fillet stores is as it was: never re-pointed.
    let fillet = f.sketch("f0000001");
    assert_eq!(
        fillet["edges"],
        json!(["p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000004)"])
    );
    // A trim of na000004, which nothing cuts but its neighbours at its ends: removed whole,
    // and the feature that named its end is listed.
    let result = f.ran(
        "sketch_trim",
        &trim(3, "s0000001", "na000004", [5.0, 30.0], json!(null)),
    );
    assert_eq!(result["pieces"], json!([]));
    assert_eq!(result["references"], json!(["f0000001"]));
}

#[test]
fn a_feature_that_isnt_a_sketch_an_unknown_sketch_and_malformed_requests_are_refused_exactly() {
    let f = opened(&bracket());
    let text = f.text();
    let sketches = ["s0000001", "s0000002", "s0000003"];
    let refused = f.refused(
        "sketch_split",
        &split(1, "p0000001", "na000001", json!([[30, 0]]), json!(null)),
    );
    assert_eq!(
        refused,
        json!([with_candidates(
            error(
                "invalid_input",
                "p0000001 is a pad, not a sketch",
                None,
                &[],
                Some("sketch"),
                None,
                Some("name a sketch of the document, by its ID")
            ),
            &sketches
        )])
    );
    let refused = f.refused(
        "sketch_merge",
        &merge(1, "s000000z", &["pa000001", "pa000002"]),
    );
    assert_eq!(
        refused,
        json!([with_candidates(
            error(
                "invalid_input",
                "the document has no feature s000000z",
                None,
                &[],
                Some("sketch"),
                None,
                Some("name a sketch of the document, by its ID")
            ),
            &sketches
        )])
    );
    // A curve the sketch hasn't; a point given as the curve to split.
    let refused = f.refused(
        "sketch_split",
        &split(1, "s0000001", "na00000z", json!([[30, 0]]), json!(null)),
    );
    assert_eq!(
        refused,
        json!([error(
            "invalid_input",
            "the sketch has no curve na00000z",
            Some("s0000001"),
            &[],
            Some("curve"),
            None,
            Some("name a curve of this sketch by its ID")
        )])
    );
    let refused = f.refused(
        "sketch_trim",
        &trim(1, "s0000001", "pa000001", [0.0, 0.0], json!(null)),
    );
    assert_eq!(refused[0]["parameter"], "curve");
    assert_eq!(
        refused[0]["message"],
        "point pa000001 isn't a curve: a line, an arc, or a circle trims"
    );
    // Shapes the reader refuses: each at its field, every one at once.
    let refused = f.refused(
        "sketch_split",
        r#"{"session": "s1", "revision": 1, "sketch": "s0000001", "curve": "origin", "at": [[30], [1, "2"], 3], "ids": "pa000007"}"#,
    );
    assert_eq!(
        refused,
        json!([
            error(
                "invalid_input",
                "curve must be a curve's ID, as the sketch holds it, got \"origin\"",
                None,
                &[],
                Some("curve"),
                None,
                None
            ),
            error(
                "invalid_input",
                "at.0 must be a position, two numbers [x, y] in millimetres, got 1 of them",
                None,
                &[],
                Some("at.0"),
                None,
                None
            ),
            error(
                "invalid_input",
                "at.1.1 must be a finite number, got \"2\"",
                None,
                &[],
                Some("at.1.1"),
                None,
                None
            ),
            error(
                "invalid_input",
                "at.2 must be a position, two numbers [x, y] in millimetres, got 3",
                None,
                &[],
                Some("at.2"),
                None,
                None
            ),
            error(
                "invalid_input",
                "ids must be a list of the new members' IDs, or null, got \"pa000007\"",
                None,
                &[],
                Some("ids"),
                None,
                None
            ),
        ])
    );
    let refused = f.refused(
        "sketch_merge",
        r#"{"session": "s1", "revision": 1, "sketch": "s0000001", "points": ["pa000001", 7, "PA000002"], "extra": 1}"#,
    );
    let messages: Vec<&str> = refused
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        messages[..2],
        [
            "points.1 must be a sketch member's ID, got 7",
            "points.2 must be a sketch member's ID, got \"PA000002\""
        ]
    );
    assert_eq!(refused.as_array().unwrap().len(), 3, "{refused}");
    assert_eq!(refused[2]["parameter"], "extra");
    let refused = f.refused(
        "sketch_unjoin",
        r#"{"session": "s1", "revision": 1, "sketch": "s0000001", "curve": "na000004", "ids": [null]}"#,
    );
    let parameters: Vec<&str> = refused
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["parameter"].as_str().unwrap())
        .collect();
    assert_eq!(parameters, ["point", "ids.0"], "{refused}");
    assert_eq!(f.text(), text);
}

#[test]
fn a_merge_holding_an_end_on_its_own_line_drops_the_on_as_implied_on_the_wire() {
    // Open item 13b's T: the upright's inside corner pa000004 at (5, 5), and a lone point
    // there, held on na000003. Merged, the on holds na000003's own end on it: removed as
    // implied, listed after the point.
    let profile = S0000001.replacen(
        r#"{"id": "pa000006", "x": 0, "y": 50, "construction": false}"#,
        r#"{"id": "pa000006", "x": 0, "y": 50, "construction": false}, {"id": "pa00000t", "x": 5, "y": 5, "construction": false}"#,
        1,
    );
    let profile = profile.replacen(
        r#"{"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}"#,
        r#"{"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}, {"id": "ka00000t", "type": "on", "point": "pa00000t", "curve": "na000003"}"#,
        1,
    );
    let f = opened(&document(&[&profile]));
    let result = f.ran(
        "sketch_merge",
        &merge(1, "s0000001", &["pa000004", "pa00000t"]),
    );
    assert_eq!(
        (&result["modified"], &result["removed"]),
        (&json!([]), &json!(["pa00000t", "ka00000t"])),
        "{result}"
    );
    let after = f.sketch("s0000001");
    assert!(!ids(&after, "constraints").contains(&"ka00000t".to_owned()));
    assert!(!ids(&after, "points").contains(&"pa00000t".to_owned()));
}

#[test]
fn the_solve_runs_on_the_edited_sketch_as_written_from_its_coordinates() {
    // Fable's seeding rule: the edit's solve is the sketch's ordinary one, from what the edit
    // wrote: here the split's new point at (30, 0) and the line's to it, and nothing else.
    let f = opened(&bracket());
    f.checked();
    f.begin();
    let solves = f.sketches.requests().len();
    f.ran(
        "sketch_split",
        &split(
            1,
            "s0000001",
            "na000001",
            json!([[30, 0]]),
            json!(["pa000007", "na000007", "ka00000d"]),
        ),
    );
    let requests = f.sketches.requests();
    assert_eq!(requests.len(), solves + 1);
    let sent = &requests[solves];
    // The profile's seven points, each at its stored place, the new one at (30, 0), after
    // the origin, sent first.
    let mut places: Vec<(f64, f64)> = sent
        .geometry
        .iter()
        .skip(1)
        .filter_map(|g| match g {
            SketchGeometry::Point(p) => {
                Some((sent.values[p.x.0 as usize], sent.values[p.y.0 as usize]))
            }
            _ => None,
        })
        .collect();
    places.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(
        places,
        [
            (0.0, 0.0),
            (0.0, 50.0),
            (5.0, 5.0),
            (5.0, 50.0),
            (30.0, 0.0),
            (60.0, 0.0),
            (60.0, 5.0)
        ]
    );
}

// ------------------------------------------------------------------ the review's cases

/// The bracket unjoined at pa000004 (eight points sent with the origin), then the merge
/// back, the fake answering the sketch before (8) by `before` and after (7) by `after`.
fn merge_judged(before: fn(&SketchSystem) -> Solved, after: fn(&SketchSystem) -> Solved) -> Value {
    let f = opened(&bracket());
    f.begin();
    f.ran(
        "sketch_unjoin",
        &unjoin(1, "s0000001", "pa000004", "na000004", json!(["pa000008"])),
    );
    f.sketches.answer(move |system| {
        Ok(match points_sent(system) {
            8 => before(system),
            7 => after(system),
            _ => FakeSketches::as_sent(system),
        })
    });
    f.call(
        "sketch_merge",
        &merge(2, "s0000001", &["pa000004", "pa000008"]),
    )
}

#[test]
fn a_merge_that_draws_another_constraint_into_the_conflict_is_refused() {
    // Same rank (in conflict, no freedom, both), but a constraint in conflict after that
    // wasn't before: FreeCAD's introducesDiagnostic, rejected.
    let envelope = merge_judged(
        |s| {
            let mut solved = FakeSketches::as_sent(s);
            solved.conflicting = first_tags(s, 1);
            solved.dofs = -1;
            solved
        },
        |s| {
            let mut solved = FakeSketches::as_sent(s);
            solved.conflicting = first_tags(s, 2);
            solved.dofs = -1;
            solved
        },
    );
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(
        envelope["errors"][0]["code"], "over_constrained",
        "{envelope}"
    );
    assert_eq!(
        envelope["errors"][0]["elements"],
        json!(["ka000001", "ka000002"])
    );
}

#[test]
fn a_merge_that_adds_a_redundancy_to_one_the_sketch_had_is_refused_naming_the_new_one() {
    let envelope = merge_judged(
        |s| {
            let mut solved = FakeSketches::as_sent(s);
            solved.redundant = first_tags(s, 1);
            solved
        },
        |s| {
            let mut solved = FakeSketches::as_sent(s);
            solved.redundant = first_tags(s, 2);
            solved
        },
    );
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(
        envelope["errors"][0]["elements"],
        json!(["ka000002"]),
        "{envelope}"
    );
    assert_eq!(
        envelope["errors"][0]["message"],
        "after the merge, these 1 constraints repeat others, as the sketch's solve before it found none of them do"
    );
}

#[test]
fn a_merge_whose_conflict_before_becomes_a_failed_solve_is_refused() {
    // Over-constrained (3) before, no solution (4) after: worse, though nothing new is
    // flagged.
    let envelope = merge_judged(conflicting, failing);
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(envelope["errors"][0]["code"], "solve_failed", "{envelope}");
}

#[test]
fn a_merge_whose_sketch_solves_unbuildable_before_and_after_is_refused() {
    // FreeCAD: InvalidGeometry after is always rejected, whatever before was.
    let envelope = merge_judged(collapsed, collapsed);
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(
        envelope["errors"][0]["code"], "invalid_result",
        "{envelope}"
    );
    // The solve's own advice: the merge's lever is for a solve that fails, not for one that
    // reaches what can't be built.
    assert_eq!(
        envelope["errors"][0]["suggestion"],
        "draw the geometry nearer its solution, or check the dimensions that move it past a kilometre or shrink it to nothing",
        "{envelope}"
    );
}

#[test]
fn a_split_of_an_arc_or_a_circle_lists_the_features_storing_a_name_made_from_it_or_its_points() {
    // A D (line ne000001 from pe000001 (-10, 0) to pe000002 (10, 0), arc ae000001 about the
    // origin back over the top, from pe000002 to pe000001), padded, its fillet naming the
    // arc's end's vertex; and a circle cc000001 of radius 5 about the origin, padded, its
    // fillet naming the circle's edge. Each split lists the fillet whose name it reaches.
    let d = r#"{"id": "s0000009", "type": "sketch", "v": 1, "name": "D", "plane": "xy", "points": [{"id": "pe000001", "x": -10, "y": 0, "construction": false}, {"id": "pe000002", "x": 10, "y": 0, "construction": false}, {"id": "pe000003", "x": 0, "y": 0, "construction": false}], "lines": [{"id": "ne000001", "from": "pe000001", "to": "pe000002", "construction": false}], "circles": [], "arcs": [{"id": "ae000001", "center": "pe000003", "start": "pe000002", "end": "pe000001", "construction": false}], "constraints": []}"#;
    let d_pad = r#"{"id": "p0000009", "type": "pad", "v": 1, "name": "D", "sketch": "s0000009", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let d_fillet = r#"{"id": "f0000009", "type": "fillet", "v": 1, "name": "F", "base": "p0000009", "edges": ["p0000009.pad.Edge(G s0000009.sketch.Vertex.pe000001)"], "radius": 1}"#;
    let o = r#"{"id": "s000000c", "type": "sketch", "v": 1, "name": "O", "plane": "xy", "points": [{"id": "pc000001", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [{"id": "cc000001", "center": "pc000001", "radius": 5, "construction": false}], "arcs": [], "constraints": []}"#;
    let o_pad = r#"{"id": "p000000c", "type": "pad", "v": 1, "name": "O", "sketch": "s000000c", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let o_fillet = r#"{"id": "f000000c", "type": "fillet", "v": 1, "name": "G", "base": "p000000c", "edges": ["p000000c.pad.Edge(G s000000c.sketch.Edge.cc000001)"], "radius": 1}"#;
    let f = opened(&document(&[d, d_pad, d_fillet, o, o_pad, o_fillet]));
    f.begin();
    let result = f.ran(
        "sketch_split",
        &split(1, "s0000009", "ae000001", json!([[0, 10]]), json!(null)),
    );
    assert_eq!(result["references"], json!(["f0000009"]), "{result}");
    let result = f.ran(
        "sketch_split",
        &split(
            2,
            "s000000c",
            "cc000001",
            json!([[5, 0], [-5, 0]]),
            json!(null),
        ),
    );
    assert_eq!(result["references"], json!(["f000000c"]), "{result}");
}

#[test]
fn a_clean_edit_asks_the_solver_once_never_for_the_sketchs_solve_before_it() {
    // Each edit whose solve is clean needs no judgment against the sketch before it: one
    // solve each, the edit's, on the fake (which solves everything cleanly).
    let f = opened(&bracket());
    f.checked();
    f.begin();
    for (command, request) in [
        (
            "sketch_split",
            split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
        ),
        (
            "sketch_unjoin",
            unjoin(2, "s0000001", "pa000004", "na000004", json!(["pa000008"])),
        ),
        (
            "sketch_merge",
            merge(3, "s0000001", &["pa000004", "pa000008"]),
        ),
        (
            "sketch_trim",
            trim(4, "s0000001", "na000003", [30.0, 5.0], json!(null)),
        ),
    ] {
        let solves = f.sketches.requests().len();
        f.ran(command, &request);
        assert_eq!(f.sketches.requests().len(), solves + 1, "{command}");
    }
}

/// The bracket's L profile with two construction circles: ca000001 about pa000004, the
/// corner whose vertex the fillet f0000001 names, and ca000002 about a point of its own,
/// pa000007, clear of the profile.
fn with_circles() -> String {
    let sketch = S0000001
        .replace(
            r#"{"id": "pa000006", "x": 0, "y": 50, "construction": false}]"#,
            r#"{"id": "pa000006", "x": 0, "y": 50, "construction": false}, {"id": "pa000007", "x": 30, "y": 25, "construction": true}]"#,
        )
        .replace(
            r#""circles": []"#,
            r#""circles": [{"id": "ca000001", "center": "pa000004", "radius": 2, "construction": true}, {"id": "ca000002", "center": "pa000007", "radius": 3, "construction": true}]"#,
        );
    assert_ne!(sketch, S0000001);
    document(&[
        &sketch, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001,
    ])
}

#[test]
fn a_circles_split_reaches_its_own_centre_never_another_circles() {
    // The fillet names a vertex made from pa000004, ca000001's centre: a split of ca000002,
    // about pa000007, reaches no name the document stores; one of ca000001 reaches the
    // fillet's.
    let f = opened(&with_circles());
    f.begin();
    let result = f.ran(
        "sketch_split",
        &split(
            1,
            "s0000001",
            "ca000002",
            json!([[33, 25], [27, 25]]),
            json!(null),
        ),
    );
    assert_eq!(result["references"], json!([]), "{result}");
    let result = f.ran(
        "sketch_split",
        &split(
            2,
            "s0000001",
            "ca000001",
            json!([[7, 5], [3, 5]]),
            json!(null),
        ),
    );
    assert_eq!(result["references"], json!(["f0000001"]), "{result}");
}

#[test]
fn an_edit_whose_result_the_file_cant_hold_asks_the_solver_once() {
    // A result the file can't hold is refused whatever the sketch's solve before it found:
    // so that solve is never asked for.
    let f = opened(&bracket());
    f.checked();
    f.begin();
    let text = f.text();
    f.sketches.answer(|system| Ok(collapsed(system)));
    for (command, request) in [
        (
            "sketch_split",
            split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
        ),
        (
            "sketch_unjoin",
            unjoin(1, "s0000001", "pa000004", "na000004", json!(null)),
        ),
    ] {
        let solves = f.sketches.requests().len();
        let refused = f.refused(command, &request);
        assert_eq!(refused[0]["code"], "invalid_result", "{refused}");
        assert_eq!(f.sketches.requests().len(), solves + 1, "{command}");
        assert_eq!((f.text(), f.revision()), (text.clone(), revision(1)));
    }
}

#[test]
fn a_split_whose_solve_fails_as_the_sketchs_did_before_it_is_made() {
    // No solver meets the sketch before the split nor after it, and neither flags a
    // constraint: the split's solve is no worse (FreeCAD's rollback, same rank), so it's
    // made, both solves asked.
    let f = opened(&bracket());
    f.checked();
    f.begin();
    f.sketches.answer(|system| Ok(failing(system)));
    let solves = f.sketches.requests().len();
    let result = f.ran(
        "sketch_split",
        &split(
            1,
            "s0000001",
            "na000001",
            json!([[30, 0]]),
            json!(["pa000007", "na000007", "ka00000d"]),
        ),
    );
    assert_eq!(result["changed"], true, "{result}");
    assert_eq!(
        result["pieces"],
        json!(["na000001", "na000007"]),
        "{result}"
    );
    assert_eq!(f.sketches.requests().len(), solves + 2);
    assert_eq!(f.revision(), revision(2));
    assert_eq!(
        member(&f.sketch("s0000001"), "lines", "na000007")["to"],
        "pa000002"
    );
}

#[test]
fn a_solver_that_crashes_in_an_edits_solve_refuses_it_as_the_crash() {
    let f = opened(&bracket());
    f.checked();
    f.begin();
    let text = f.text();
    f.sketches.answer(|_| {
        Err(silico_core::Error::new(
            silico_core::ErrorCode::WorkerCrashed,
            "the fake sketch worker crashed",
        ))
    });
    for (command, request) in [
        (
            "sketch_split",
            split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
        ),
        (
            "sketch_merge",
            merge(1, "s0000001", &["pa000001", "pa000003"]),
        ),
    ] {
        let refused = f.refused(command, &request);
        assert_eq!(
            refused,
            json!([error(
                "worker_crashed",
                "the fake sketch worker crashed",
                Some("s0000001"),
                &[],
                None,
                None,
                None
            )]),
            "{command}"
        );
        assert_eq!((f.text(), f.revision()), (text.clone(), revision(1)));
    }
}

#[test]
fn an_edit_whose_solve_its_sessions_close_stops_answers_cancelled_as_the_command() {
    // The sessions dropped while the split's solve is held: its session closes, which stops
    // the solve (not a cancel of the command), and the split answers as its own stop, never
    // as the drag's words for a stopped solve.
    let f = opened(&bracket());
    f.checked();
    f.begin();
    f.sketches.hold();
    let request = split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null));
    let running = match f.sessions.call("sketch_split", &request) {
        Call::Running(calling) => calling,
        Call::Unknown => panic!("no sketch_split"),
    };
    assert!(f.sketches.wait_held(HANG), "the edit's solve never came");
    let Fixture {
        dir,
        sketches,
        sessions,
    } = f;
    let dropper = std::thread::spawn(move || drop(sessions));
    let Ok(answered) = running.wait_timeout(HANG) else {
        panic!("sketch_split didn't answer in {HANG:?}")
    };
    let envelope: Value = serde_json::from_str(&answered.text).unwrap();
    assert_eq!(
        envelope["errors"],
        json!([error(
            "cancelled",
            "sketch_split was cancelled before it finished",
            None,
            &[],
            None,
            None,
            Some(
                "run it again when you want its answer; every rebuild step that finished is kept in the cache"
            )
        )]),
        "{envelope}"
    );
    assert!(sketches.stops() >= 1);
    dropper.join().unwrap();
    sketches.release();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("bracket.silico")).unwrap(),
        bracket()
    );
}
