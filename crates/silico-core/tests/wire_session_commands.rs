//! M4.2's session commands on the wire (docs/design/m4.1-command-api.md, sections 2, 4, 5,
//! 7, and 8): `begin`, `commit`, `rollback`, `format`, `add_feature`, `set_field`,
//! `remove_feature`, and `replace_text`. The design's examples of them, byte for byte: each
//! file the design's own block; each request read, written back to it, and held to its
//! snapshot; each request read strictly, every key it doesn't take refused and every field
//! it gives required; and each refusal the session's own, made by running the command on
//! the fake kernel from the request's text, written to its file, and held to the error
//! contract. The results the fake can make as the design's are made so too; the rest (a
//! path as `open` was given it, a drift) are written by the crate's own tests
//! (src/command/wire/session_tests.rs), which can build a result's problems. The snapshots
//! themselves are recorded and checked with the others' (wire_commands.rs). The examples of
//! M4.2's other commands, `version`, `open`, `close`, `status`, `cancel`, `text`, `outline`,
//! and `check`, are each the design's own text here too, and every request but `version`'s
//! and `open`'s, which name no session, is read strictly here; the rest of their tests are in
//! wire_session_reads.rs.
//!
//! `check`'s broken reference is a real kernel's: silico-kernel's wire_examples.rs writes it
//! on kernel workers, and it's the design's own text here too.
//!
//! The design's examples not in the contract are two, each for its reason ([`LEFT_OUT`]):
//! `open` outside a workspace, whose example file is the maintainer's to add (its message
//! quotes the path, which the design's doesn't); and `commit`'s `io_failed`, a disk out of
//! room, which no test can make happen.

use std::path::PathBuf;
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::wire::edits::{
    AddFeatureRequest, RemoveFeatureRequest, ReplaceTextRequest, SetFieldRequest,
};
use silico_core::command::wire::reads::{CheckRequest, TextRequest};
use silico_core::command::wire::session::CancelRequest;
use silico_core::command::wire::transaction::{CommitRequest, FormatRequest, SessionRequest};
use silico_core::command::wire::{Layout, ToWire, snapshot_value, write};
use silico_core::command::{Problems, Response, Revision, SessionId, Sessions};
use silico_core::document::json::{Node, Plain, Value, read};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

use crate::sessions::{answer, id};

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract")
        .join(name)
}

/// The committed file `name`, read and never written.
fn committed(name: &str) -> String {
    let path = contract(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The example `name` (`begin/request`), as committed.
fn example(name: &str) -> String {
    committed(&format!("commands/examples/{name}.json"))
}

/// The design's JSON code blocks between the headings `from` and `to`, in order.
fn design(from: &str, to: &str) -> Vec<String> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m4.1-command-api.md");
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

/// The snapshot `of`'s `part`, `request` or `result`, as committed.
fn schema_of(of: &str, part: &str) -> serde_json::Value {
    let schema: serde_json::Value =
        serde_json::from_str(&committed(&format!("commands/{of}.json"))).unwrap();
    schema[part].clone()
}

/// The error contract's shape, the envelope's `Error`.
fn error_schema() -> serde_json::Value {
    let envelope: serde_json::Value =
        serde_json::from_str(&committed("commands/envelope.json")).unwrap();
    serde_json::json!({"$defs": envelope["$defs"], "$ref": "#/$defs/Error"})
}

/// Panics, naming `what`, unless `text` holds to `schema`.
fn holds(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

/// Panics, naming `what`, if `text` holds to `schema`.
fn fails(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(!validator.is_valid(&value), "{what} held");
}

fn revision(n: u64) -> Revision {
    let mut r = Revision::OPENED;
    for _ in 1..n {
        r = r.next().unwrap();
    }
    r
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

/// The one refusal of `response`, written.
fn the_refusal<T: std::fmt::Debug>(response: &Response<T>) -> String {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{:?}", problems.errors());
    written(&problems.errors()[0])
}

/// The bracket's profile, empty, a box with the pad's ID (for the tests, any solid is the
/// pad), and the holes' sketch, empty: what the design's pocket is added to, at place 3.
const BRACKET: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "L profile", "plane": "xz", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}, {"id": "p0000001", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, 0], "length": 40, "width": 20, "height": 6}, {"id": "s0000002", "type": "sketch", "v": 1, "name": "Holes", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}]}"#;

/// The design's pad, of an empty sketch: what `set_field` is refused on.
const PAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "L profile", "plane": "xz", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}, {"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 6, "reversed": false}}]}"#;

/// The bracket, its holes' sketch holding a hole of diameter 5: the design's diameter
/// constraint, `g4h5j6k7`, what `set_field` sets to 0.
const HOLES: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "p0000001", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, 0], "length": 40, "width": 20, "height": 6}, {"id": "s0000002", "type": "sketch", "v": 1, "name": "Holes", "plane": "xy", "points": [{"id": "pa000000", "x": 10, "y": 10, "construction": false}], "lines": [], "circles": [{"id": "ca000000", "center": "pa000000", "radius": 2.5, "construction": false}], "arcs": [], "constraints": [{"id": "g4h5j6k7", "type": "diameter", "curve": "ca000000", "value": 5, "driving": true}]}]}"#;

/// The design's document with no features, as `replace_text`'s example gives it.
const EMPTY: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": []\n}\n";

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
}

/// Sessions on the fake kernel, with `text` in `bracket.silico` open as s1.
fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        kernel,
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("bracket.silico");
    std::fs::write(&path, text).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    assert_eq!(id, s1());
    Fixture { dir, sessions, id }
}

impl Fixture {
    /// The session's file.
    fn path(&self) -> PathBuf {
        self.dir.path().join("bracket.silico")
    }

    /// `written`, a refusal naming the session's file as `open` was given it (by its whole
    /// path, in a directory of the test's own), as a session opened as `bracket.silico`, in
    /// the directory it's in, names it: the design's.
    fn here(&self, written: &str) -> String {
        let dir = format!("{}/", self.dir.path().display());
        assert!(written.contains(&dir), "{written}");
        written.replace(&dir, "")
    }

    fn revision(&self) -> Revision {
        self.sessions.status(&self.id).session.unwrap().revision
    }

    fn begin(&self) {
        answer(self.sessions.begin(&self.id)).result.unwrap();
    }

    /// Begun and rolled back until the session is at revision `n`: a rollback is a new
    /// revision, an edit to none.
    fn at(&self, n: u64) {
        while self.revision() < revision(n) {
            self.begin();
            answer(self.sessions.rollback(&self.id)).result.unwrap();
        }
        assert_eq!(self.revision(), revision(n));
    }

    /// The design's request `text` of `set_field`, run, its revision the session's.
    fn set(&self, text: &str) -> Response<silico_core::command::FieldSet> {
        let request = SetFieldRequest::read(&parsed(text)).unwrap();
        answer(self.sessions.set_field(
            &request.session,
            self.revision(),
            request.feature,
            request.field,
            request.value,
        ))
    }
}

#[test]
fn the_examples_are_the_designs_own_text() {
    // Each section's blocks, in order, each file the block and a newline; a block not yet in
    // the contract (`None`) is the one the module's doc says, and has no file.
    let sections: [(&str, &str, &str, &[Option<&str>]); 17] = [
        (
            "### `version` (M4.2)",
            "### `open` (M4.2)",
            "version",
            &[Some("request"), Some("result")],
        ),
        (
            "### `open` (M4.2)",
            "### `close` (M4.2)",
            "open",
            &[
                Some("request"),
                Some("result"),
                Some("refused-document"),
                Some("refused-missing"),
                Some("refused-exists"),
                Some("refused-open-already"),
                None,
                Some("refused-too-many"),
            ],
        ),
        (
            "### `close` (M4.2)",
            "### `status` (M4.2)",
            "close",
            &[Some("request"), Some("result"), Some("refused-transaction")],
        ),
        (
            "### `status` (M4.2)",
            "### `cancel` (M4.2)",
            "status",
            &[Some("request"), Some("result")],
        ),
        (
            "### `cancel` (M4.2)",
            "### `begin` (M4.2)",
            "cancel",
            &[
                Some("request"),
                Some("result"),
                Some("refused-not-running"),
                Some("cancelled"),
            ],
        ),
        (
            "### `begin` (M4.2)",
            "### `commit` (M4.2)",
            "begin",
            &[Some("request"), Some("result")],
        ),
        (
            "### `commit` (M4.2)",
            "### `rollback` (M4.2)",
            "commit",
            &[
                Some("request"),
                Some("result"),
                Some("drift-kept"),
                Some("unchanged"),
                Some("refused-no-transaction"),
                Some("refused-conflict"),
                None,
                Some("request-save-as"),
                Some("result-save-as"),
                Some("refused-exists"),
            ],
        ),
        (
            "### `rollback` (M4.2)",
            "### `format` (M4.2)",
            "rollback",
            &[Some("request"), Some("result")],
        ),
        (
            "### `format` (M4.2)",
            "### `add_feature` (M4.2)",
            "format",
            &[
                Some("request"),
                Some("result"),
                Some("refused-unknown-feature"),
            ],
        ),
        (
            "### `add_feature` (M4.2)",
            "### `set_field` (M4.2)",
            "add_feature",
            &[
                Some("request"),
                Some("result"),
                Some("refused-sketch"),
                Some("refused-fingerprint"),
                Some("refused-stale"),
            ],
        ),
        (
            "### `set_field` (M4.2)",
            "### `remove_feature` (M4.2)",
            "set_field",
            &[
                Some("request"),
                Some("result"),
                Some("request-sketch-member"),
                Some("refused-diameter"),
                Some("refused-extent-length"),
                Some("refused-no-field"),
                Some("refused-type"),
            ],
        ),
        (
            "### `remove_feature` (M4.2)",
            "### `replace_text` (M4.2)",
            "remove_feature",
            &[
                Some("request"),
                Some("result"),
                Some("request-used"),
                Some("refused-used"),
            ],
        ),
        (
            "### `replace_text` (M4.2)",
            "### `text` (M4.2; `revision` in M5)",
            "replace_text",
            &[
                Some("request"),
                Some("result"),
                Some("refused-syntax"),
                Some("refused-no-naming"),
            ],
        ),
        (
            "### `text` (M4.2; `revision` in M5)",
            "### `outline` (M4.2)",
            "text",
            &[Some("request"), Some("result"), Some("request-revision")],
        ),
        (
            "### `outline` (M4.2)",
            "### `check` (M4.2",
            "outline",
            &[Some("request"), Some("result")],
        ),
        (
            "### `check` (M4.2",
            "### `cache_clear` (M4.3)",
            "check",
            &[Some("request"), Some("result"), Some("reference-broken")],
        ),
        (
            "### `cache_clear` (M4.3)",
            "### `elements` (M4.4)",
            "cache_clear",
            &[Some("request"), Some("result"), Some("refused-leads-out")],
        ),
    ];
    let mut left_out = Vec::new();
    for (from, to, dir, names) in sections {
        let blocks = design(from, to);
        assert_eq!(blocks.len(), names.len(), "{dir}");
        let mut files: Vec<String> =
            std::fs::read_dir(contract(&format!("commands/examples/{dir}")))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
        files.sort();
        let mut want: Vec<String> = names
            .iter()
            .flatten()
            .chain(added_by_m5(dir))
            .map(|n| format!("{n}.json"))
            .collect();
        want.sort();
        // A file for each block taken, and no other.
        assert_eq!(files, want, "{dir}");
        for (block, name) in blocks.iter().zip(names) {
            match name {
                Some(name) => assert_eq!(
                    format!("{block}\n"),
                    example(&format!("{dir}/{name}")),
                    "{dir}/{name}"
                ),
                None => left_out.push((dir, block.clone())),
            }
        }
    }
    // The examples left out, by command and what each is (a refusal's code and parameter, a
    // result's one problem's, or a result itself), each for its reason.
    let left: Vec<(&str, &str, &str)> = left_out
        .iter()
        .map(|(dir, block)| {
            let value: serde_json::Value = serde_json::from_str(block).unwrap();
            let error = match value.get("problems") {
                Some(problems) => {
                    assert_eq!(problems.as_array().map(Vec::len), Some(1), "{block}");
                    Some(&problems[0])
                }
                None => value.get("code").map(|_| &value),
            };
            let (code, parameter) = match error {
                Some(error) => (
                    error["code"].as_str().unwrap(),
                    error["parameter"].as_str().unwrap(),
                ),
                None => ("result", ""),
            };
            LEFT_OUT
                .iter()
                .find(|(d, c, p, _)| (*d, *c, *p) == (*dir, code, parameter))
                .map(|(d, c, p, _)| (*d, *c, *p))
                .unwrap_or_else(|| panic!("{dir}'s {code} at {parameter:?} left out for no reason"))
        })
        .collect();
    assert_eq!(
        left,
        LEFT_OUT.map(|(dir, code, parameter, _)| (dir, code, parameter))
    );
}

/// The examples of M4.1's commands that M5.0's design added beside M4.1's, by command:
/// `add_feature`'s `before` and `remove_feature`'s `rewired` (M5.1), each the M5.0 design's
/// own text (wire_timeline_commands.rs).
const ADDED_BY_M5: [(&str, &[&str]); 2] = [
    (
        "add_feature",
        &[
            "request-before",
            "result-before",
            "refused-no-before",
            "refused-consumed",
        ],
    ),
    ("remove_feature", &["result-rewired"]),
];

/// The examples M5.0's design added to the command `dir`'s.
fn added_by_m5(dir: &str) -> impl Iterator<Item = &'static &'static str> {
    ADDED_BY_M5
        .iter()
        .filter(move |(of, _)| *of == dir)
        .flat_map(|(_, names)| names.iter())
}

/// The design's examples not in the contract, by command and what each is (a refusal's code
/// and parameter, or `result` for a result), each with why.
const LEFT_OUT: [(&str, &str, &str, &str); 2] = [
    (
        "open",
        "invalid_input",
        "path",
        "open of a path outside the workspace a server was started in: its example file is a \
         change to the protected contract, the maintainer's; and the design's message names \
         the path bare, where every message quotes one (session_workspace.rs pins the refusal \
         as the session gives it)",
    ),
    (
        "commit",
        "io_failed",
        "path",
        "commit's save failing for a disk out of room, which no test can make happen; and the \
         code says it with the step that failed and the file's path, which the design's leaves \
         out",
    ),
];

#[test]
fn the_designs_requests_are_read_written_back_byte_for_byte_and_hold_to_their_schemas() {
    // `begin`, `rollback`, and `cache_clear` take one shape: the session alone.
    for of in ["begin", "rollback", "cache_clear"] {
        let text = example(&format!("{of}/request"));
        let request = SessionRequest::read(&parsed(&text)).unwrap();
        assert_eq!(request, SessionRequest { session: s1() });
        assert_eq!(written(&request), text, "{of}");
        holds(&schema_of(of, "request"), &text, of);
    }

    // `commit`'s: to its own file, and Save As's (M5.4), to another.
    for (of, path, replace) in [
        ("commit/request", None, false),
        ("commit/request-save-as", Some("bracket-v2.silico"), false),
    ] {
        let text = example(of);
        let request = CommitRequest::read(&parsed(&text)).unwrap();
        assert_eq!(
            request,
            CommitRequest {
                session: s1(),
                path: path.map(str::to_owned),
                replace,
            }
        );
        assert_eq!(written(&request), text, "{of}");
        holds(&schema_of("commit", "request"), &text, of);
    }

    let text = example("format/request");
    let request = FormatRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        FormatRequest {
            session: s1(),
            features: Some(vec![id("k7f2qx3m")]),
            write: true,
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("format", "request"), &text, "format");

    let text = example("add_feature/request");
    let request = AddFeatureRequest::read(&parsed(&text)).unwrap();
    assert_eq!((request.session, request.revision), (s1(), revision(6)));
    // The feature as given, its members in their order.
    let Value::Object(members) = &request.feature.value else {
        panic!("{:?}", request.feature);
    };
    assert_eq!(
        members.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(),
        ["type", "name", "base", "sketch", "extent"]
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("add_feature", "request"), &text, "add_feature");

    for (name, at, feature, field) in [
        ("set_field/request", 7, "p0000001", "extent.length"),
        (
            "set_field/request-sketch-member",
            8,
            "s0000002",
            "constraints.g4h5j6k7.value",
        ),
    ] {
        let text = example(name);
        let request = SetFieldRequest::read(&parsed(&text)).unwrap();
        assert_eq!(
            (
                request.session,
                request.revision,
                request.feature.clone(),
                request.field.as_str()
            ),
            (s1(), revision(at), id(feature), field),
            "{name}"
        );
        assert_eq!(written(&request), text, "{name}");
        holds(&schema_of("set_field", "request"), &text, name);
    }

    for (name, feature) in [
        ("remove_feature/request", "c0000001"),
        ("remove_feature/request-used", "m3tq8v0z"),
    ] {
        let text = example(name);
        let request = RemoveFeatureRequest::read(&parsed(&text)).unwrap();
        assert_eq!(
            request,
            RemoveFeatureRequest {
                session: s1(),
                revision: revision(8),
                feature: id(feature),
            }
        );
        assert_eq!(written(&request), text, "{name}");
        holds(&schema_of("remove_feature", "request"), &text, name);
    }

    let text = example("replace_text/request");
    let request = ReplaceTextRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        ReplaceTextRequest {
            session: s1(),
            revision: revision(9),
            text: EMPTY.to_owned(),
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("replace_text", "request"), &text, "replace_text");
}

/// Each refusal's parameter and message, in order.
fn said(problems: &Problems) -> Vec<(String, String)> {
    problems
        .errors()
        .iter()
        .map(|e| {
            assert_eq!(e.code(), ErrorCode::InvalidInput);
            (
                e.parameter().unwrap_or("").to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

/// Each command's request reader, by name, giving only whether it read.
fn reader(of: &str) -> fn(&Node) -> Result<(), Problems> {
    match of {
        "begin" | "rollback" => |n| SessionRequest::read(n).map(drop),
        "commit" => |n| CommitRequest::read(n).map(drop),
        "format" => |n| FormatRequest::read(n).map(drop),
        "add_feature" => |n| AddFeatureRequest::read(n).map(drop),
        "set_field" => |n| SetFieldRequest::read(n).map(drop),
        "remove_feature" => |n| RemoveFeatureRequest::read(n).map(drop),
        "replace_text" => |n| ReplaceTextRequest::read(n).map(drop),
        "close" | "status" | "outline" | "cache_clear" => |n| SessionRequest::read(n).map(drop),
        "cancel" => |n| CancelRequest::read(n).map(drop),
        "text" => |n| TextRequest::read(n).map(drop),
        "check" => |n| CheckRequest::read(n).map(drop),
        _ => panic!("{of}"),
    }
}

/// Each command, and the keys its request has, in the shape's order.
const REQUESTS: [(&str, &[&str]); 15] = [
    ("close", &["session"]),
    ("status", &["session"]),
    ("cancel", &["session", "command"]),
    ("begin", &["session"]),
    ("commit", &["session", "path", "replace"]),
    ("rollback", &["session"]),
    ("format", &["session", "features", "write"]),
    ("add_feature", &["session", "revision", "before", "feature"]),
    (
        "set_field",
        &["session", "revision", "feature", "field", "value"],
    ),
    ("remove_feature", &["session", "revision", "feature"]),
    ("replace_text", &["session", "revision", "text"]),
    ("text", &["session", "feature", "revision"]),
    ("outline", &["session"]),
    ("check", &["session", "model", "fresh"]),
    ("cache_clear", &["session"]),
];

/// `text`, an object's JSON, without its member `key`.
fn without(text: &str, key: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let serde_json::Value::Object(mut members) = value else {
        panic!("{text}");
    };
    assert!(members.remove(key).is_some(), "{key}");
    serde_json::Value::Object(members).to_string()
}

#[test]
fn every_request_requires_each_of_its_fields_and_refuses_any_other() {
    for (of, keys) in REQUESTS {
        let read = reader(of);
        let text = example(&format!("{of}/request"));
        let request: serde_json::Value = serde_json::from_str(&text).unwrap();
        // The design's request has every key the reader takes, and no other.
        let mut given: Vec<&str> = request
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        given.sort_unstable();
        let mut taken = keys.to_vec();
        taken.sort_unstable();
        assert_eq!(given, taken, "{of}");
        assert!(read(&parsed(&text)).is_ok(), "{of}");
        for key in keys {
            let refused = read(&parsed(&without(&text, key))).unwrap_err();
            assert_eq!(
                said(&refused),
                [((*key).to_owned(), format!("{key} is missing"))],
                "{of}"
            );
            fails(&schema_of(of, "request"), &without(&text, key), key);
        }
        // A key the shape hasn't, at the top, with its every field there.
        let extra = text.replacen('{', r#"{"extra": 1, "#, 1);
        let refused = read(&parsed(&extra)).unwrap_err();
        assert_eq!(
            said(&refused),
            [(
                "extra".to_owned(),
                "extra isn't a field of this request".to_owned()
            )],
            "{of}"
        );
        fails(&schema_of(of, "request"), &extra, "an extra key");
        // The session, and only a session's ID.
        let session = text.replacen(r#""session": "s1""#, r#""session": 1"#, 1);
        assert_eq!(
            said(&read(&parsed(&session)).unwrap_err()),
            [(
                "session".to_owned(),
                "session must be a session's ID, s1, s2, ..., got 1".to_owned()
            )],
            "{of}"
        );
        fails(&schema_of(of, "request"), &session, "a numeric session");
        // Nor an ID SessionId::parse doesn't read: no session 0, and no leading zero.
        for bad in ["s0", "s01"] {
            let session = text.replacen(r#""session": "s1""#, &format!(r#""session": "{bad}""#), 1);
            assert_eq!(
                said(&read(&parsed(&session)).unwrap_err()),
                [(
                    "session".to_owned(),
                    format!(r#"session must be a session's ID, s1, s2, ..., got "{bad}""#)
                )],
                "{of}: {bad}"
            );
            fails(&schema_of(of, "request"), &session, bad);
        }
    }
}

/// `text`, an object's JSON, with its member `key` set to `value`, JSON's text.
fn with(text: &str, key: &str, value: &str) -> String {
    let mut object: serde_json::Value = serde_json::from_str(text).unwrap();
    object[key] = serde_json::from_str(value).unwrap();
    object.to_string()
}

#[test]
fn each_requests_fields_are_of_their_types_and_null_only_where_the_shape_allows_it() {
    let refused = |of: &str, text: &str| said(&reader(of)(&parsed(text)).unwrap_err());
    for of in ["add_feature", "set_field", "remove_feature", "replace_text"] {
        let text = example(&format!("{of}/request"));
        for (value, got) in [
            ("0", "0"),
            ("null", "null"),
            ("1.0", "1.0"),
            (r#""7""#, r#""7""#),
        ] {
            assert_eq!(
                refused(of, &with(&text, "revision", value)),
                [(
                    "revision".to_owned(),
                    format!(
                        "revision must be a whole number from 1 to 18446744073709551615, got {got}"
                    )
                )],
                "{of}: {value}"
            );
        }
        // Of the right type, but no revision: refused by the schema too.
        fails(
            &schema_of(of, "request"),
            &with(&text, "revision", "0"),
            "revision 0",
        );
    }
    // `format`'s features: a list of IDs or null, each member at fault by its place; its
    // write true or false.
    let format = example("format/request");
    assert!(reader("format")(&parsed(&format.replace(r#"["k7f2qx3m"]"#, "null"))).is_ok());
    assert_eq!(
        refused(
            "format",
            &format
                .replace(r#"["k7f2qx3m"]"#, r#"["k7f2qx3m", 7, "K"]"#)
                .replace("true", "null")
        ),
        [
            (
                "features.1".to_owned(),
                "features.1 must be a feature's ID, got 7".to_owned()
            ),
            (
                "features.2".to_owned(),
                r#"features.2 must be a feature's ID, got "K""#.to_owned()
            ),
            (
                "write".to_owned(),
                "write must be true or false, got null".to_owned()
            ),
        ]
    );
    // A feature by its ID, never null; a field's path and a text, strings.
    let set = example("set_field/request");
    assert_eq!(
        refused(
            "set_field",
            &set.replace(r#""p0000001""#, "null")
                .replace(r#""extent.length""#, r#"["extent", "length"]"#)
        ),
        [
            (
                "feature".to_owned(),
                "feature must be a feature's ID, got null".to_owned()
            ),
            (
                "field".to_owned(),
                "field must be a string, got an array".to_owned()
            ),
        ]
    );
    let remove = example("remove_feature/request");
    assert_eq!(
        refused(
            "remove_feature",
            &remove.replace(r#""c0000001""#, r#""Bevel""#)
        ),
        [(
            "feature".to_owned(),
            r#"feature must be a feature's ID, got "Bevel""#.to_owned()
        )]
    );
    let replace = example("replace_text/request");
    let (head, _) = replace.split_once(r#""text": "#).unwrap();
    assert_eq!(
        refused("replace_text", &format!(r#"{head}"text": null}}"#)),
        [(
            "text".to_owned(),
            "text must be a string, got null".to_owned()
        )]
    );
    // `set_field`'s value is any JSON, read by the document's reader in the session: null
    // and a list are each taken as they are.
    for value in ["null", "[1, 2]", r#"{"a": {"b": "c"}}"#, r#""text""#, "1e3"] {
        let text = set.replace(r#""value": 8"#, &format!(r#""value": {value}"#));
        let request = SetFieldRequest::read(&parsed(&text)).unwrap();
        assert_eq!(
            Plain::of(&request.value),
            Plain::of(&parsed(value)),
            "{value}"
        );
    }
    let add = example("add_feature/request");
    let (head, _) = add.split_once(r#""feature": "#).unwrap();
    // `add_feature`'s feature is an object, any object, read by the document's reader in the
    // session; anything else is refused by the wire, in its words.
    let request = AddFeatureRequest::read(&parsed(&format!(
        "{head}\"feature\": {{\"radius\": 7}}\n}}"
    )))
    .unwrap();
    assert_eq!(
        Plain::of(&request.feature),
        Plain::of(&parsed(r#"{"radius": 7}"#))
    );
    let f = opened(BRACKET);
    for (value, got) in [
        ("7", "a number"),
        ("null", "null"),
        ("[1]", "an array"),
        (r#""box""#, "a string"),
    ] {
        let want = [(
            "feature".to_owned(),
            format!("feature must be an object, got {got}"),
        )];
        assert_eq!(
            refused("add_feature", &format!("{head}\"feature\": {value}\n}}")),
            want,
            "{value}"
        );
        // A Rust caller, past the wire: the session refuses it in the same words.
        let added = answer(
            f.sessions
                .add_feature(&f.id, f.revision(), None, parsed(value)),
        );
        assert_eq!(said(added.result.as_ref().unwrap_err()), want, "{value}");
    }
}

#[test]
fn formats_request_keeps_null_features_and_a_false_write() {
    let text = "{\"session\": \"s1\", \"features\": null, \"write\": false}\n";
    let request = FormatRequest::read(&parsed(text)).unwrap();
    assert_eq!(
        request,
        FormatRequest {
            session: s1(),
            features: None,
            write: false,
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("format", "request"), text, "null and false");
    // No features is a list, none of them: not null.
    let text = "{\"session\": \"s1\", \"features\": [], \"write\": true}\n";
    let request = FormatRequest::read(&parsed(text)).unwrap();
    assert_eq!(request.features, Some(Vec::new()));
    assert_eq!(written(&request), text);
    holds(&schema_of("format", "request"), text, "no features");
}

#[test]
fn every_snapshot_of_these_commands_has_the_names_an_agent_reads() {
    let titles = [
        ("begin", "BeginRequest", "BeginResult"),
        ("commit", "CommitRequest", "CommitResult"),
        ("rollback", "RollbackRequest", "RollbackResult"),
        ("format", "FormatRequest", "FormatResult"),
        ("add_feature", "AddFeatureRequest", "AddFeatureResult"),
        ("set_field", "SetFieldRequest", "SetFieldResult"),
        (
            "remove_feature",
            "RemoveFeatureRequest",
            "RemoveFeatureResult",
        ),
        ("replace_text", "ReplaceTextRequest", "ReplaceTextResult"),
    ];
    for (of, request, result) in titles {
        let value = snapshot_value(of).unwrap();
        assert_eq!(value["request"]["title"], request, "{of}");
        assert_eq!(value["result"]["title"], result, "{of}");
    }
    // An edit's result holds a commit's, by the same name as `commit`'s own result; and the
    // document's feature, by its own schema.
    for of in ["add_feature", "set_field", "remove_feature", "replace_text"] {
        let value = snapshot_value(of).unwrap();
        let commit = &value["result"]["$defs"]["CommitResult"];
        let own: serde_json::Map<String, serde_json::Value> =
            snapshot_value("commit").unwrap()["result"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "$defs" | "$schema" | "title"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
        assert_eq!(*commit, serde_json::Value::Object(own), "{of}");
    }
    let added = snapshot_value("add_feature").unwrap();
    let document: serde_json::Value =
        serde_json::from_str(&committed("document-schema.json")).unwrap();
    assert_eq!(
        added["result"]["$defs"]["Feature"],
        document["$defs"]["Feature"]
    );
    // A new feature is the document's, its fingerprint only null, its id and v optional,
    // and each member the reader takes null for (its id, v, and note, and a pad's or
    // revolve's base) taking null.
    let new = &added["request"]["$defs"]["NewFeature"];
    assert_eq!(new["required"], serde_json::json!(["name"]));
    assert_eq!(new["properties"]["fingerprint"]["type"], "null");
    for (key, own) in [
        ("id", serde_json::json!({"$ref": "#/$defs/FeatureId"})),
        ("v", serde_json::json!({"$ref": "#/$defs/FeatureVersion"})),
        ("note", serde_json::json!({"type": "string"})),
    ] {
        assert_eq!(
            new["properties"][key]["anyOf"],
            serde_json::json!([own, {"type": "null"}]),
            "{key}"
        );
    }
    // A sketch's plane is the document's, but for its face plane, whose x may be left out
    // (M4.1 section 5).
    let mut types = document["$defs"]["Feature"]["oneOf"].clone();
    for variant in types.as_array_mut().unwrap() {
        match variant["properties"]["type"]["const"].as_str() {
            Some("pad" | "revolve") => {
                variant["properties"]["base"] = serde_json::json!({
                    "anyOf": [{"$ref": "#/$defs/FeatureId"}, {"type": "null"}],
                    "description": "Null is the same as left out.",
                });
            }
            Some("sketch") => {
                assert_eq!(
                    variant["properties"]["plane"],
                    serde_json::json!({"$ref": "#/$defs/Plane"})
                );
                variant["properties"]["plane"] = serde_json::json!({
                    "anyOf": [
                        {"$ref": "#/$defs/OriginPlane"},
                        {"$ref": "#/$defs/FacePlaneRequest"},
                    ],
                    "description": "The plane the sketch is drawn on: one of the origin's, by its word (D6), or a planar face of the model, by its name, with the axis the sketch's x follows, which may be left out for the face's default, and whether it's flipped (docs/ports/attacher.md, F1 and F2; M5.9).",
                });
            }
            _ => {}
        }
    }
    assert_eq!(new["oneOf"], types);
    assert_eq!(new["unevaluatedProperties"], false);
    // The request's face plane: the document's, closed to the same three members, its face
    // and reversed as the document's, its x the document's token or null, and not required.
    let request = &added["request"]["$defs"]["FacePlaneRequest"];
    let face = &document["$defs"]["FacePlane"];
    assert_eq!(request["type"], "object");
    assert_eq!(request["additionalProperties"], false);
    assert_eq!(request["required"], serde_json::json!(["face", "reversed"]));
    assert_eq!(
        face["required"],
        serde_json::json!(["face", "x", "reversed"])
    );
    assert_eq!(
        request["properties"]["reversed"],
        face["properties"]["reversed"]
    );
    assert_eq!(
        request["properties"]["face"]["$ref"],
        face["properties"]["face"]["$ref"]
    );
    assert_eq!(
        request["properties"]["x"]["anyOf"],
        serde_json::json!([{"$ref": "#/$defs/AxisToken"}, {"type": "null"}])
    );
    assert_eq!(
        added["request"]["$defs"]["AxisToken"],
        document["$defs"]["AxisToken"]
    );
    // The document's Plane and FacePlane aren't the request's.
    assert!(added["request"]["$defs"].get("Plane").is_none());
    assert!(added["request"]["$defs"].get("FacePlane").is_none());
}

/// Every definition `schema` refers to, by name.
fn references(schema: &serde_json::Value, found: &mut Vec<String>) {
    match schema {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                match value.as_str() {
                    Some(to) if key == "$ref" => found.push(to.replace("#/$defs/", "")),
                    _ => references(value, found),
                }
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|i| references(i, found)),
        _ => {}
    }
}

#[test]
fn every_definition_in_a_commands_snapshot_is_one_it_uses() {
    for (of, _) in silico_core::command::wire::COMMANDS {
        let snapshot = snapshot_value(of).unwrap();
        for part in ["request", "result"] {
            let root = &snapshot[part];
            let mut used = Vec::new();
            references(root, &mut used);
            let defined: Vec<&String> = root["$defs"]
                .as_object()
                .map(|defs| defs.keys().collect())
                .unwrap_or_default();
            let unused: Vec<&&String> = defined.iter().filter(|d| !used.contains(d)).collect();
            assert!(unused.is_empty(), "{of} {part}: {unused:?}");
        }
    }
    // The new feature's fingerprint is only null: the document's fingerprint isn't defined.
    let added = snapshot_value("add_feature").unwrap();
    assert!(added["request"]["$defs"].get("Fingerprint").is_none());
    assert!(added["result"]["$defs"].get("Fingerprint").is_some());
}

#[test]
fn a_new_feature_given_null_where_the_reader_takes_it_holds_to_its_schema_and_is_added() {
    let schema = schema_of("add_feature", "request");
    let pocket = example("add_feature/request").replace(
        r#""type": "pocket","#,
        r#""id": null, "v": null, "type": "pocket", "note": null, "fingerprint": null,"#,
    );
    let pad = r#"{"session": "s1", "revision": 6, "before": null, "feature": {"id": null, "type": "pad", "v": null, "name": "Pad", "note": null, "base": null, "sketch": "s0000002", "extent": {"type": "one_side", "length": 6, "reversed": false}, "fingerprint": null}}"#;
    let f = opened(BRACKET);
    f.begin();
    for (what, text) in [("a pocket", pocket.as_str()), ("a pad", pad)] {
        holds(&schema, text, what);
        let request = AddFeatureRequest::read(&parsed(text)).unwrap();
        let added =
            answer(
                f.sessions
                    .add_feature(&request.session, f.revision(), None, request.feature),
            )
            .result
            .unwrap();
        // Each null left out: a new ID, no note, no fingerprint.
        assert!(!["p0000001", "s0000002"].contains(&added.feature.id.as_str()));
        assert_eq!(
            (added.feature.note, added.feature.fingerprint),
            (None, None),
            "{what}"
        );
    }
    // Null where the reader refuses it, the schema refuses too: a pocket's base, which it
    // must have, and a name. Each refused by the document's reader, at its field.
    for (what, from, to, parameter) in [
        (
            "a pocket's null base",
            r#""base": "p0000001""#,
            r#""base": null"#,
            "base",
        ),
        (
            "a null name",
            r#""name": "Leg holes""#,
            r#""name": null"#,
            "name",
        ),
    ] {
        let text = example("add_feature/request").replace(from, to);
        assert_ne!(text, example("add_feature/request"), "{what}");
        fails(&schema, &text, what);
        let request = AddFeatureRequest::read(&parsed(&text)).unwrap();
        let refused =
            answer(
                f.sessions
                    .add_feature(&request.session, f.revision(), None, request.feature),
            );
        let problems = refused.result.as_ref().unwrap_err();
        let at: Vec<(ErrorCode, Option<&str>)> = problems
            .errors()
            .iter()
            .map(|e| (e.code(), e.parameter()))
            .collect();
        assert_eq!(at, [(ErrorCode::InvalidInput, Some(parameter))], "{what}");
    }
}

#[test]
fn a_new_features_schema_takes_the_designs_feature_and_refuses_a_fingerprint() {
    let request = schema_of("add_feature", "request");
    let text = example("add_feature/request");
    holds(&request, &text, "the design's");
    // With an ID and a version, as a client may give them.
    holds(
        &request,
        &text.replace(
            r#""type": "pocket","#,
            r#""id": "q7m2x9kd", "v": 1, "type": "pocket","#,
        ),
        "with an ID",
    );
    for (what, from, to) in [
        (
            "a fingerprint",
            r#""name": "Leg holes","#,
            r#""name": "Leg holes", "fingerprint": {"volume": 1, "area": 1, "faces": 1, "edges": 1, "vertices": 1},"#,
        ),
        ("no name", r#""name": "Leg holes","#, ""),
        (
            "a member a pocket hasn't",
            r#""name": "Leg holes","#,
            r#""name": "Leg holes", "radius": 1,"#,
        ),
        ("no type", r#""type": "pocket","#, ""),
    ] {
        assert!(text.contains(from), "{what}");
        fails(&request, &text.replace(from, to), what);
    }
}

#[test]
fn begin_and_rollback_answer_as_the_design_gives_them() {
    let f = opened(BRACKET);
    f.at(4);
    let request = SessionRequest::read(&parsed(&example("begin/request"))).unwrap();
    let begun = answer(f.sessions.begin(&request.session));
    let text = written(begun.result.as_ref().unwrap());
    assert_eq!(text, example("begin/result"));
    holds(&schema_of("begin", "result"), &text, "begin");
    // The transaction's edit, then the rollback: the new revision's state is the 4th's.
    let set =
        r#"{"session": "s1", "revision": 1, "feature": "p0000001", "field": "height", "value": 7}"#;
    assert!(f.set(set).result.as_ref().unwrap().changed);
    assert_eq!(f.revision(), revision(5));
    let request = SessionRequest::read(&parsed(&example("rollback/request"))).unwrap();
    let rolled = answer(f.sessions.rollback(&request.session));
    let text = written(rolled.result.as_ref().unwrap());
    assert_eq!(text, example("rollback/result"));
    holds(&schema_of("rollback", "result"), &text, "rollback");
    assert_eq!(rolled.session.unwrap().revision, revision(6));
    // A second begin, inside the first: the design's envelope refusal, section 2.
    f.begin();
    let again = answer(f.sessions.begin(&f.id));
    assert_eq!(
        the_refusal(&again),
        r#"{"code": "invalid_input", "message": "a transaction is already open in this session", "feature": null, "elements": [], "elements_total": 0, "parameter": "session", "cause": null, "suggestion": "commit or roll back the open transaction, then begin again", "candidates": [], "advice": null}
"#
    );
}

#[test]
fn the_designs_added_and_removed_features_are_the_sessions_own() {
    // The design's pocket, given an ID (a session makes one at random otherwise), added
    // inside a transaction: the feature as the document holds it, and no commit.
    let f = opened(BRACKET);
    f.begin();
    let mut request = AddFeatureRequest::read(&parsed(&example("add_feature/request"))).unwrap();
    let Value::Object(members) = &mut request.feature.value else {
        panic!();
    };
    let given = parsed(r#"{"id": "q7m2x9kd"}"#);
    let Value::Object(id_member) = given.value else {
        panic!();
    };
    members.extend(id_member);
    let added =
        answer(
            f.sessions
                .add_feature(&request.session, f.revision(), None, request.feature),
        );
    let text = written(added.result.as_ref().unwrap());
    assert_eq!(text, example("add_feature/result"));
    holds(&schema_of("add_feature", "result"), &text, "added");

    // The design's chamfer removed, inside a transaction, from the contract's document.
    let f = opened(&committed("naming-scheme-5.silico"));
    f.begin();
    let request = RemoveFeatureRequest::read(&parsed(&example("remove_feature/request"))).unwrap();
    let removed = answer(f.sessions.remove_feature(
        &request.session,
        f.revision(),
        request.feature,
    ));
    // The chamfer as the document holds it, its text the contract file's own, a level
    // less deep, its edges a name a line: the design's example.
    let file = committed("naming-scheme-5.silico");
    let start = file.find("    {\n      \"id\": \"c0000001\"").unwrap();
    let end = start + file[start..].find("\n    }").unwrap() + "\n    }".len();
    let chamfer: String = file[start..end]
        .lines()
        .map(|line| line.strip_prefix("  ").unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let text = written(removed.result.as_ref().unwrap());
    assert_eq!(
        text,
        format!(
            "{{\n  \"feature\": {},\n  \"index\": 4,\n  \"rewired\": [],\n  \"changed\": true,\n  \"commit\": null\n}}\n",
            chamfer.trim_start()
        )
    );
    assert_eq!(text, example("remove_feature/result"));
    holds(&schema_of("remove_feature", "result"), &text, "removed");
}

#[test]
fn the_designs_refusals_are_the_sessions_own_and_hold_to_the_error_contract() {
    let error = error_schema();
    let check = |name: &str, got: String| {
        let want = example(name);
        assert_eq!(got, want, "{name}");
        holds(&error, &want, name);
    };

    // `commit` with no transaction open.
    let f = opened(BRACKET);
    let request = CommitRequest::read(&parsed(&example("commit/request"))).unwrap();
    let commit = answer(f.sessions.commit(&request.session));
    check("commit/refused-no-transaction", the_refusal(&commit));
    // `rollback` with none, as for `commit`.
    let rollback = answer(f.sessions.rollback(&f.id));
    check("commit/refused-no-transaction", the_refusal(&rollback));

    // `format` accepting a feature's drift the document hasn't, the second named.
    let f = opened(&committed("naming-scheme-5.silico"));
    let request = FormatRequest::read(&parsed(
        &example("format/request").replace(r#"["k7f2qx3m"]"#, r#"["k7f2qx3m", "z0000001"]"#),
    ))
    .unwrap();
    let format = answer(
        f.sessions
            .format(&request.session, request.features, request.write),
    );
    check("format/refused-unknown-feature", the_refusal(&format));

    // `add_feature`: the design's pocket of a sketch the document hasn't, given its ID; and
    // one with a fingerprint.
    let f = opened(BRACKET);
    let add = |text: String| {
        let request = AddFeatureRequest::read(&parsed(&text)).unwrap();
        answer(
            f.sessions
                .add_feature(&request.session, f.revision(), None, request.feature),
        )
    };
    let pocket = example("add_feature/request");
    let fingerprinted = add(pocket.replace(
        r#""name": "Leg holes","#,
        r#""name": "Leg holes", "fingerprint": {"volume": 1, "area": 1, "faces": 1, "edges": 1, "vertices": 1},"#,
    ));
    check(
        "add_feature/refused-fingerprint",
        the_refusal(&fingerprinted),
    );
    let sketchless = pocket
        .replace(
            r#""type": "pocket","#,
            r#""id": "q7m2x9kd", "type": "pocket","#,
        )
        .replace(r#""sketch": "s0000002""#, r#""sketch": "s0000003""#);
    assert_ne!(sketchless, pocket);
    check("add_feature/refused-sketch", the_refusal(&add(sketchless)));

    // `add_feature` against revision 6, the design's, the file changed outside the session
    // since: reloaded before the edit, at revision 7, so the edit is stale.
    let f = opened(BRACKET);
    f.at(6);
    std::fs::write(f.path(), BRACKET.replace("Plate", "Base")).unwrap();
    let request = AddFeatureRequest::read(&parsed(&pocket)).unwrap();
    assert_eq!(request.revision, revision(6));
    let stale =
        answer(
            f.sessions
                .add_feature(&request.session, request.revision, None, request.feature),
        );
    check("add_feature/refused-stale", f.here(&the_refusal(&stale)));
    assert_eq!(f.revision(), revision(7));

    // `commit` of a transaction begun at revision 4, the file changed outside the session
    // since: a conflict, the transaction left open.
    let f = opened(BRACKET);
    f.at(4);
    f.begin();
    let height =
        r#"{"session": "s1", "revision": 1, "feature": "p0000001", "field": "height", "value": 7}"#;
    assert!(f.set(height).result.as_ref().unwrap().changed);
    std::fs::write(f.path(), BRACKET.replace("Plate", "Base")).unwrap();
    let commit = answer(f.sessions.commit(&f.id));
    check("commit/refused-conflict", f.here(&the_refusal(&commit)));
    assert!(
        f.sessions
            .status(&f.id)
            .result
            .unwrap()
            .transaction
            .is_some()
    );

    // `commit` with a path (Save As, M5.4) where a file is, with no `replace`.
    let f = opened(BRACKET);
    f.begin();
    let there = f.dir.path().join("bracket-v2.silico");
    std::fs::write(&there, "an older copy").unwrap();
    let request = CommitRequest::read(&parsed(&example("commit/request-save-as"))).unwrap();
    let to = f.dir.path().join(request.path.unwrap());
    let exists = answer(f.sessions.save_as(&f.id, to, request.replace));
    check("commit/refused-exists", f.here(&the_refusal(&exists)));
    assert_eq!(std::fs::read_to_string(&there).unwrap(), "an older copy");

    // `set_field` of the design's sketch member: its diameter to 0.
    let f = opened(HOLES);
    check(
        "set_field/refused-diameter",
        the_refusal(&f.set(&example("set_field/request-sketch-member"))),
    );

    // `set_field` on the pad: a length of 0, a field it hasn't, and its type.
    let f = opened(PAD);
    let set = example("set_field/request");
    check(
        "set_field/refused-extent-length",
        the_refusal(&f.set(&set.replace(r#""value": 8"#, r#""value": 0"#))),
    );
    check(
        "set_field/refused-no-field",
        the_refusal(&f.set(&set.replace("extent.length", "extent.depth"))),
    );
    check(
        "set_field/refused-type",
        the_refusal(
            &f.set(
                &set.replace("extent.length", "type")
                    .replace(r#""value": 8"#, r#""value": "box""#),
            ),
        ),
    );

    // `remove_feature` of the cut, whose elements the fillet names: refused before the gate,
    // naming both (M5.0's A9 amends the M4.1 example to this), the session as it was.
    let f = opened(&committed("naming-scheme-5.silico"));
    let request =
        RemoveFeatureRequest::read(&parsed(&example("remove_feature/request-used"))).unwrap();
    let used = answer(
        f.sessions
            .remove_feature(&request.session, f.revision(), request.feature),
    );
    check("remove_feature/refused-used", the_refusal(&used));
    assert_eq!(f.revision(), Revision::OPENED);

    // `replace_text` of text without naming.
    let f = opened(BRACKET);
    let replace = |text: &str| {
        let request = ReplaceTextRequest::read(&parsed(&with(
            &example("replace_text/request"),
            "text",
            &serde_json::Value::String(text.to_owned()).to_string(),
        )))
        .unwrap();
        answer(
            f.sessions
                .replace_text(&request.session, f.revision(), request.text),
        )
    };
    let unnamed = replace(&EMPTY.replacen("  \"naming\": 5,\n", "", 1));
    check("replace_text/refused-no-naming", the_refusal(&unnamed));
    // Text whose third line, the units, ends without its comma: the reader finds the next
    // member at line 4, column 3.
    let broken = EMPTY.replacen("\"1nm\"},\n", "\"1nm\"}\n", 1);
    assert_ne!(broken, EMPTY);
    check(
        "replace_text/refused-syntax",
        the_refusal(&replace(&broken)),
    );
}

#[test]
fn a_new_sketch_on_a_face_leaving_out_its_x_holds_to_the_request_schema_and_the_documents_doesnt() {
    let schema = schema_of("add_feature", "request");
    let document: serde_json::Value =
        serde_json::from_str(&committed("document-schema.json")).unwrap();
    let request = |plane: &str| {
        format!(
            r#"{{"session": "s1", "revision": 6, "before": null, "feature": {{"type": "sketch", "name": "On top", "plane": {plane}, "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}}}}"#
        )
    };
    let feature = |text: &str| -> serde_json::Value {
        let mut feature =
            serde_json::from_str::<serde_json::Value>(text).unwrap()["feature"].clone();
        feature["id"] = serde_json::json!("s0000009");
        feature["v"] = serde_json::json!(1);
        feature
    };
    // The document's own feature schema, the definitions its references reach.
    let feature_schema = {
        let mut root = document.clone();
        root["$ref"] = serde_json::json!("#/$defs/Feature");
        root.as_object_mut().unwrap().remove("properties");
        root.as_object_mut().unwrap().remove("required");
        root.as_object_mut().unwrap().remove("type");
        root.as_object_mut().unwrap().remove("additionalProperties");
        root
    };
    for (what, plane) in [
        (
            "x left out",
            r#"{"face": "b0000001.box.Face6", "reversed": false}"#,
        ),
        (
            "x null",
            r#"{"face": "b0000001.box.Face6", "x": null, "reversed": false}"#,
        ),
    ] {
        let text = request(plane);
        holds(&schema, &text, what);
        // The document never holds a face plane without its x.
        fails(&feature_schema, &feature(&text).to_string(), what);
    }
    // Given, it's the document's, and holds to both.
    let text = request(r#"{"face": "b0000001.box.Face6", "x": "+y", "reversed": false}"#);
    holds(&schema, &text, "x given");
    holds(&feature_schema, &feature(&text).to_string(), "x given");
    holds(&schema, &request(r#""xz""#), "an origin plane");
    for (what, plane) in [
        (
            "x not a token",
            r#"{"face": "b0000001.box.Face6", "x": "+w", "reversed": false}"#,
        ),
        (
            "a member it hasn't",
            r#"{"face": "b0000001.box.Face6", "reversed": false, "y": "+z"}"#,
        ),
        ("no reversed", r#"{"face": "b0000001.box.Face6"}"#),
        ("no face", r#"{"x": "+x", "reversed": false}"#),
        ("an empty face", r#"{"face": "", "reversed": false}"#),
    ] {
        fails(&schema, &request(plane), what);
    }
}
