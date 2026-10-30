//! M5.1's timeline commands on the wire (docs/design/m5.0-app.md, section 7):
//! `add_feature`'s `before`, `move_feature`, `remove_feature`'s `rewired`, and `timeline`.
//! The design's examples of them, byte for byte: each file the design's own block; each
//! request read, written back to it, and held to its snapshot, every field it gives
//! required and any other refused; and each result and refusal the session's own, made by
//! running the command from the request's text on the M4.9 bracket (tests/common/bracket.rs)
//! on the fake kernel, inside a transaction at revision 12, as the design sets them up,
//! written to its file and held to its snapshot or the error contract. Then the design's
//! M5.1 test plan where it's a command's: the inverses (a removal undone by adding the
//! feature back `before` the one that followed it, the file byte for byte after the commit,
//! and a pad root's way back), a move past unrelated features rewiring nothing and writing
//! no fingerprint, a move to where it is changing nothing, a move to the end, a tool chain's
//! move rewiring its boolean's `tool`, and the lineage rule's refusals (open item 2b). Each
//! expected value is the design's text or worked out by hand from the chain's rules; the
//! volumes are the real kernel's, in silico-cli's tests/serve.rs.

use std::path::PathBuf;
use std::sync::Arc;

use silico_core::cache::Cache;
use silico_core::command::wire::edits::{AddFeatureRequest, MoveFeatureRequest};
use silico_core::command::wire::reads::TimelineRequest;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{
    ModelChoice, Problems, Response, Revision, Running, SessionId, Sessions,
};
use silico_core::document::chain::Rewired;
use silico_core::document::json::{Node, read};
use silico_core::document::{self, FeatureId};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_core::timeline::Health;
use silico_core::{Error, ErrorCode};
use tempfile::TempDir;

use super::bracket::*;

const HANG: std::time::Duration = std::time::Duration::from_secs(60);

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

/// The example `name` (`move_feature/request`), as committed.
fn example(name: &str) -> String {
    committed(&format!("commands/examples/{name}.json"))
}

/// The M5.0 design's JSON code blocks between the headings `from` and `to`, in order.
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

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

/// The command's answer, failing if it hangs.
fn answer<T>(running: Running<T>) -> Response<T> {
    match running.wait_timeout(HANG) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in {HANG:?}"),
    }
}

/// The one refusal of `response`, written.
fn the_refusal<T: std::fmt::Debug>(response: &Response<T>) -> String {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{:?}", problems.errors());
    written(&problems.errors()[0])
}

/// The one error of `response`.
fn the_error<T: std::fmt::Debug>(response: &Response<T>) -> Error {
    let problems: &Problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{:?}", problems.errors());
    problems.errors()[0].clone()
}

fn rewired(feature: &str, field: &'static str, from: Option<&str>, to: Option<&str>) -> Rewired {
    Rewired {
        feature: id(feature),
        field,
        from: from.map(id),
        to: to.map(id),
    }
}

/// A document of `features`, each a feature's JSON, as the reader reads it and the
/// canonical form writes it.
fn text_of(features: &[&str]) -> String {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    );
    document::write(&document::read(text.as_bytes()).unwrap()).unwrap()
}

/// The M4.9 bracket, its seven features, as the session added them: no fingerprints.
fn bracket() -> String {
    text_of(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001,
    ])
}

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
}

/// Sessions on the fake kernel and the fake sketch worker, with `text` in `bracket.silico`
/// open as s1.
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

/// The bracket, saved by `format` with the fake kernel's fingerprints: a document whose
/// every built feature stores one, so no check of it warns.
fn fingerprinted() -> String {
    let f = opened(&bracket());
    let formatted = answer(f.sessions.format(&f.id, None, true)).result.unwrap();
    assert!(formatted.written);
    f.file()
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("bracket.silico")
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.path()).unwrap()
    }

    fn revision(&self) -> Revision {
        self.sessions.status(&self.id).session.unwrap().revision
    }

    fn begin(&self) {
        answer(self.sessions.begin(&self.id)).result.unwrap();
    }

    /// Begun and rolled back until the session is at revision `n`, then begun: the design's
    /// examples are inside a transaction, at revision 12.
    fn inside_at(&self, n: u64) {
        while self.revision() < revision(n) {
            self.begin();
            answer(self.sessions.rollback(&self.id)).result.unwrap();
        }
        assert_eq!(self.revision(), revision(n));
        self.begin();
        assert_eq!(self.revision(), revision(n));
    }

    /// `add_feature` of the request `text`, its revision the session's.
    fn add(&self, text: &str) -> Response<silico_core::command::Added> {
        let r = AddFeatureRequest::read(&parsed(text)).unwrap();
        answer(
            self.sessions
                .add_feature(&r.session, self.revision(), r.before, r.feature),
        )
    }

    /// `move_feature` of `feature` before `before`, at the session's revision.
    fn moved(&self, feature: &str, before: Option<&str>) -> Response<silico_core::command::Moved> {
        answer(
            self.sessions
                .move_feature(&self.id, self.revision(), id(feature), before.map(id)),
        )
    }

    fn remove(&self, feature: &str) -> Response<silico_core::command::Removed> {
        answer(
            self.sessions
                .remove_feature(&self.id, self.revision(), id(feature)),
        )
    }

    /// The session's document's features' IDs, in timeline order, each with its base.
    fn chain(&self) -> Vec<(String, Option<String>)> {
        let text = answer(self.sessions.text(&self.id, None))
            .result
            .unwrap()
            .text;
        document::read(text.as_bytes())
            .unwrap()
            .features
            .iter()
            .map(|f| {
                let base = f
                    .operation
                    .inputs()
                    .into_iter()
                    .find(|(field, _)| *field == "base")
                    .map(|(_, b)| b.to_string());
                (f.id.to_string(), base)
            })
            .collect()
    }
}

/// The M5.0 design's sections of these commands, each block's file in order.
const SECTIONS: [(&str, &str, &str, &[&str]); 4] = [
    (
        "### `add_feature`'s `before` (M5.1)",
        "### `move_feature` (M5.1)",
        "add_feature",
        &[
            "request-before",
            "result-before",
            "refused-no-before",
            "refused-consumed",
        ],
    ),
    (
        "### `move_feature` (M5.1)",
        "### `remove_feature`'s `rewired`",
        "move_feature",
        &["request", "result", "refused-sketch", "refused-root"],
    ),
    (
        "### `remove_feature`'s `rewired`",
        "### `suppress_feature`",
        "remove_feature",
        &["result-rewired"],
    ),
    (
        "### `timeline` (M5.1)",
        "### `undo` and `redo`",
        "timeline",
        &["request", "result"],
    ),
];

#[test]
fn the_examples_are_the_m5_designs_own_text() {
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
    }
    // The new commands' directories hold these files and no other (add_feature's and
    // remove_feature's are listed whole by wire_session_commands.rs).
    for (dir, want) in [
        (
            "move_feature",
            &["refused-root", "refused-sketch", "request", "result"][..],
        ),
        ("timeline", &["request", "result"][..]),
    ] {
        let mut files: Vec<String> =
            std::fs::read_dir(contract(&format!("commands/examples/{dir}")))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
        files.sort();
        let want: Vec<String> = want.iter().map(|n| format!("{n}.json")).collect();
        assert_eq!(files, want, "{dir}");
    }
}

#[test]
fn the_designs_requests_are_read_written_back_byte_for_byte_and_hold_to_their_schemas() {
    let text = example("add_feature/request-before");
    let request = AddFeatureRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        (request.session, request.revision, request.before.clone()),
        (s1(), revision(12), Some(id("h0000002")))
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("add_feature", "request"), &text, "add_feature");

    let text = example("move_feature/request");
    let request = MoveFeatureRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        MoveFeatureRequest {
            session: s1(),
            revision: revision(12),
            feature: id("h0000001"),
            before: Some(id("f0000001")),
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("move_feature", "request"), &text, "move_feature");
    // To the end: `before` null, read and written back.
    let end = text.replace(r#""f0000001""#, "null");
    let request = MoveFeatureRequest::read(&parsed(&end)).unwrap();
    assert_eq!(request.before, None);
    assert_eq!(written(&request), end);
    holds(&schema_of("move_feature", "request"), &end, "to the end");

    let text = example("timeline/request");
    let request = TimelineRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        TimelineRequest {
            session: s1(),
            model: ModelChoice::Published,
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("timeline", "request"), &text, "timeline");
    let current = text.replace("published", "current");
    assert_eq!(
        TimelineRequest::read(&parsed(&current)).unwrap().model,
        ModelChoice::Current
    );
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

/// `text`, an object's JSON, without its member `key`.
fn without(text: &str, key: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let serde_json::Value::Object(mut members) = value else {
        panic!("{text}");
    };
    assert!(members.remove(key).is_some(), "{key}");
    serde_json::Value::Object(members).to_string()
}

/// `text`, an object's JSON, with its member `key` set to `value`, JSON's text.
fn with(text: &str, key: &str, value: &str) -> String {
    let mut object: serde_json::Value = serde_json::from_str(text).unwrap();
    object[key] = serde_json::from_str(value).unwrap();
    object.to_string()
}

#[test]
fn every_new_request_requires_each_of_its_fields_and_refuses_any_other() {
    type Reader = fn(&Node) -> Result<(), Problems>;
    let requests: [(&str, &str, &[&str], Reader); 3] = [
        (
            "add_feature",
            "add_feature/request-before",
            &["session", "revision", "before", "feature"],
            |n| AddFeatureRequest::read(n).map(drop),
        ),
        (
            "move_feature",
            "move_feature/request",
            &["session", "revision", "feature", "before"],
            |n| MoveFeatureRequest::read(n).map(drop),
        ),
        ("timeline", "timeline/request", &["session", "model"], |n| {
            TimelineRequest::read(n).map(drop)
        }),
    ];
    for (of, name, keys, read) in requests {
        let text = example(name);
        let mut given: Vec<String> = serde_json::from_str::<serde_json::Value>(&text)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        given.sort();
        let mut taken: Vec<String> = keys.iter().map(|k| (*k).to_owned()).collect();
        taken.sort();
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
        let extra = with(&text, "extra", "1");
        assert_eq!(
            said(&read(&parsed(&extra)).unwrap_err()),
            [(
                "extra".to_owned(),
                "extra isn't a field of this request".to_owned()
            )],
            "{of}"
        );
        fails(&schema_of(of, "request"), &extra, "an extra key");
    }
    // `before` is a feature's ID or null: never another value, in the words a nullable ID
    // is refused in, and refused by the schema too.
    for (of, name) in [
        ("add_feature", "add_feature/request-before"),
        ("move_feature", "move_feature/request"),
    ] {
        let text = example(name);
        for (value, got) in [
            ("7", "7"),
            (r#""H0000002""#, r#""H0000002""#),
            ("[]", "an array"),
        ] {
            let bad = with(&text, "before", value);
            let refused = if of == "add_feature" {
                AddFeatureRequest::read(&parsed(&bad)).unwrap_err()
            } else {
                MoveFeatureRequest::read(&parsed(&bad)).unwrap_err()
            };
            assert_eq!(
                said(&refused),
                [(
                    "before".to_owned(),
                    format!("before must be a feature's ID or null, got {got}")
                )],
                "{of}: {value}"
            );
            fails(&schema_of(of, "request"), &bad, value);
        }
    }
    // A move's feature is an ID, never null; a timeline's model one of the two.
    let refused = MoveFeatureRequest::read(&parsed(&with(
        &example("move_feature/request"),
        "feature",
        "null",
    )))
    .unwrap_err();
    assert_eq!(
        said(&refused),
        [(
            "feature".to_owned(),
            "feature must be a feature's ID, got null".to_owned()
        )]
    );
    let bad = with(&example("timeline/request"), "model", r#""newest""#);
    let refused = TimelineRequest::read(&parsed(&bad)).unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("model"));
    fails(
        &schema_of("timeline", "request"),
        &bad,
        "a model of neither",
    );
}

#[test]
fn the_designs_insert_and_its_refusals_are_the_sessions_own() {
    let error = error_schema();
    let f = opened(&bracket());
    f.inside_at(12);
    let request = example("add_feature/request-before");

    // Before h0000001's consumer, h0000002: rewired to the chamfer, at place 5.
    let added = f.add(&request);
    let text = written(added.result.as_ref().unwrap());
    assert_eq!(text, example("add_feature/result-before"));
    holds(&schema_of("add_feature", "result"), &text, "inserted");
    assert_eq!(f.revision(), revision(13));
    assert_eq!(
        f.chain(),
        [
            ("s0000001", None),
            ("p0000001", None),
            ("s0000002", None),
            ("h0000001", Some("p0000001")),
            ("s0000003", None),
            ("c0000002", Some("h0000001")),
            ("h0000002", Some("c0000002")),
            ("f0000001", Some("h0000002")),
        ]
        .map(|(f, b)| (f.to_owned(), b.map(str::to_owned)))
    );
    answer(f.sessions.rollback(&f.id)).result.unwrap();

    // Before a feature the document hasn't.
    f.begin();
    let at = f.revision();
    let missing = f.add(&with(&request, "before", r#""z0000001""#));
    let got = the_refusal(&missing);
    assert_eq!(got, example("add_feature/refused-no-before"));
    holds(&error, &got, "no feature to add before");
    // After h0000002, which consumes h0000001: before f0000001, the test plan's.
    let consumed = f.add(&with(&request, "before", r#""f0000001""#));
    let got = the_refusal(&consumed);
    assert_eq!(got, example("add_feature/refused-consumed"));
    holds(&error, &got, "consumed before the place");
    // Neither changed the session.
    assert_eq!(f.revision(), at);

    // A boolean whose two inputs both had consumers: refused, naming both consumers.
    let fuse = r#"{"session": "s1", "revision": 12, "before": "f0000001", "feature": {"id": "j0000001", "type": "fuse", "name": "Both", "base": "h0000001", "tool": "h0000002"}}"#;
    let twice = the_error(&f.add(fuse));
    assert_eq!(
        twice.message(),
        "j0000001 would be consumed twice: its base h0000001's solid is consumed by h0000002, and its tool h0000002's by f0000001"
    );
    assert_eq!(twice.feature(), Some("j0000001"));
    assert_eq!(twice.parameter(), Some("feature"));
    // At the end, a consumed input is the gate's to refuse, as M4 adds a feature.
    let end = f.add(&with(&request, "before", "null"));
    let gate = the_error(&end);
    assert_eq!(
        (gate.message(), gate.feature(), gate.parameter()),
        (
            r#"base h0000001's solid was already consumed by h0000002 ("Upright holes"): a solid can be used once"#,
            Some("c0000002"),
            Some("base")
        )
    );
    assert_eq!(f.revision(), at);
}

#[test]
fn the_designs_move_and_its_refusals_are_the_sessions_own() {
    let error = error_schema();
    let f = opened(&bracket());
    f.inside_at(12);
    let request = MoveFeatureRequest::read(&parsed(&example("move_feature/request"))).unwrap();
    let moved = answer(f.sessions.move_feature(
        &request.session,
        f.revision(),
        request.feature,
        request.before,
    ));
    let text = written(moved.result.as_ref().unwrap());
    assert_eq!(text, example("move_feature/result"));
    holds(&schema_of("move_feature", "result"), &text, "moved");
    // The design's timeline: each pocket still after its sketch.
    assert_eq!(
        f.chain(),
        [
            ("s0000001", None),
            ("p0000001", None),
            ("s0000002", None),
            ("s0000003", None),
            ("h0000002", Some("p0000001")),
            ("h0000001", Some("h0000002")),
            ("f0000001", Some("h0000001")),
        ]
        .map(|(f, b)| (f.to_owned(), b.map(str::to_owned)))
    );
    answer(f.sessions.rollback(&f.id)).result.unwrap();

    f.begin();
    let at = f.revision();
    // h0000002 before h0000001: before its sketch, s0000003.
    let got = the_refusal(&f.moved("h0000002", Some("h0000001")));
    assert_eq!(got, example("move_feature/refused-sketch"));
    holds(&error, &got, "before its sketch");
    // p0000001, the root, past h0000001, which needs it.
    let got = the_refusal(&f.moved("p0000001", Some("s0000003")));
    assert_eq!(got, example("move_feature/refused-root"));
    holds(&error, &got, "the root");
    // A feature, or a place, the document hasn't: each named by its field.
    let unknown = the_error(&f.moved("z0000001", None));
    assert_eq!(
        (unknown.message(), unknown.parameter(), unknown.suggestion()),
        (
            "the document has no feature z0000001",
            Some("feature"),
            Some("name a feature by its ID, as the document holds it")
        )
    );
    let nowhere = the_error(&f.moved("h0000001", Some("z0000001")));
    assert_eq!(
        (nowhere.message(), nowhere.parameter(), nowhere.suggestion()),
        (
            "the document has no feature z0000001 to move this one before",
            Some("before"),
            Some("name a feature by its ID, or give null to move it to the end")
        )
    );
    assert_eq!(f.revision(), at);
}

#[test]
fn the_designs_removal_is_the_sessions_own() {
    // The bracket's h0000001 stores no fingerprint, so the removed feature is the example's
    // whole: the design's block, which leaves the recording's fingerprint out.
    let f = opened(&bracket());
    f.inside_at(12);
    let removed = f.remove("h0000001");
    let text = written(removed.result.as_ref().unwrap());
    assert_eq!(text, example("remove_feature/result-rewired"));
    holds(&schema_of("remove_feature", "result"), &text, "removed");
    let feature = &removed.result.as_ref().unwrap().feature;
    assert_eq!(
        document::write_feature(feature).unwrap(),
        document::write_feature(&document::read(bracket().as_bytes()).unwrap().features[3])
            .unwrap()
    );
    // Removing p0000001, the root under h0000001 and whose elements the fillet names, or
    // s0000002, h0000001's sketch: refused, naming both.
    answer(f.sessions.rollback(&f.id)).result.unwrap();
    f.begin();
    let root = the_error(&f.remove("p0000001"));
    assert_eq!(
        (root.message(), root.feature(), root.parameter()),
        (
            "f0000001 names p0000001's elements, so p0000001 can't be removed",
            Some("p0000001"),
            Some("feature")
        )
    );
    let sketch = the_error(&f.remove("s0000002"));
    assert_eq!(
        (sketch.message(), sketch.feature(), sketch.suggestion()),
        (
            "h0000001 uses s0000002 as its sketch, so s0000002 can't be removed",
            Some("s0000002"),
            Some("remove h0000001 first")
        )
    );
}

#[test]
fn the_designs_timeline_is_the_sessions_own_and_its_healths_are_the_snapshots_words() {
    let f = opened(&fingerprinted());
    f.inside_at(12);
    // The current model of revision 12, built and published, then read as published.
    let current = answer(f.sessions.timeline(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    assert_eq!(current.model, Some(revision(12)));
    let request = TimelineRequest::read(&parsed(&example("timeline/request"))).unwrap();
    let response = answer(f.sessions.timeline(&request.session, request.model));
    assert_eq!(
        response.model.as_ref().map(|m| m.revision),
        Some(revision(12))
    );
    let text = written(response.result.as_ref().unwrap());
    assert_eq!(text, example("timeline/result"));
    holds(&schema_of("timeline", "result"), &text, "timeline");

    // Every health the core writes is one of the snapshot's words, in its order.
    let snapshot = schema_of("timeline", "result");
    let words: Vec<&str> = snapshot["$defs"]["Health"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert_eq!(
        words,
        ["healthy", "warning", "error", "suppressed", "unknown"]
    );
    assert_eq!(Health::ALL.map(Health::word), words.as_slice());
    // An entry of each health holds to the snapshot, and one of another word doesn't.
    let entry = example("timeline/result");
    for word in &words {
        holds(
            &snapshot,
            &entry.replace(r#""health": "healthy""#, &format!(r#""health": "{word}""#)),
            word,
        );
    }
    fails(
        &snapshot,
        &entry.replace(r#""health": "healthy""#, r#""health": "failed""#),
        "a health of no state's",
    );
}

#[test]
fn a_refused_documents_timeline_lists_it_as_read_every_health_unknown() {
    // A pad of no length, which the gate refuses: listed, every feature `unknown`, no model
    // named and no bodies, the gate's one problem counted.
    let flat = bracket().replace(r#""length": 40"#, r#""length": 0"#);
    let f = opened(&flat);
    let response = answer(f.sessions.timeline(&f.id, ModelChoice::Current));
    let timeline = response.result.as_ref().unwrap();
    let value: serde_json::Value = serde_json::from_str(&written(timeline)).unwrap();
    assert_eq!(value["revision"], 1);
    assert_eq!(value["model_revision"], serde_json::Value::Null);
    assert_eq!(value["problems"], 1);
    assert_eq!(value["bodies"], serde_json::json!([]));
    let features = value["features"].as_array().unwrap();
    assert_eq!(features.len(), 7);
    assert!(features.iter().all(|e| e["health"] == "unknown"), "{value}");
    holds(
        &schema_of("timeline", "result"),
        &written(timeline),
        "refused",
    );
    // Text that isn't a document: no features, the reader's one problem.
    let f = opened("{}");
    let timeline = answer(f.sessions.timeline(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    let text = written(&timeline);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["features"], serde_json::Value::Null);
    assert!(value["problems"].as_u64().unwrap() >= 1, "{value}");
    holds(&schema_of("timeline", "result"), &text, "unread");
}

// ---------------------------------------------------------------------------------------
// The design's M5.1 test plan, where it's a command's.

/// `feature`'s canonical text without its fingerprint: what a client adds back.
fn unprinted(feature: &silico_core::document::Feature) -> String {
    let mut back = feature.clone();
    assert!(back.fingerprint.is_some(), "{feature:?}");
    back.fingerprint = None;
    document::write_feature(&back).unwrap()
}

#[test]
fn a_removal_is_undone_by_adding_the_feature_back_before_the_one_that_followed_it() {
    // Removing h0000001, then adding it back with its recorded ID, its fingerprint left
    // out, before s0000003 (place 3): h0000002's base rewired back, and, each edit its own
    // commit, the file byte for byte as it was, h0000001's fingerprint written fresh.
    let file = fingerprinted();
    let f = opened(&file);
    let removed = f.remove("h0000001").result.unwrap();
    assert_eq!(removed.index, 3);
    assert_eq!(
        removed.rewired,
        [rewired(
            "h0000002",
            "base",
            Some("h0000001"),
            Some("p0000001")
        )]
    );
    assert!(removed.commit.unwrap().written);
    assert_ne!(f.file(), file);
    let feature = unprinted(&removed.feature);
    let added = f
        .add(&format!(
            r#"{{"session": "s1", "revision": 1, "before": "s0000003", "feature": {feature}}}"#
        ))
        .result
        .unwrap();
    assert_eq!(added.index, 3);
    assert_eq!(
        added.rewired,
        [rewired(
            "h0000002",
            "base",
            Some("p0000001"),
            Some("h0000001")
        )]
    );
    let commit = added.commit.unwrap();
    assert!(commit.written);
    assert_eq!(
        commit.refingerprinted,
        [id("h0000001"), id("h0000002"), id("f0000001")]
    );
    assert_eq!(f.file(), file);
}

/// `file` with h0000001's stored fingerprint one face off what it builds: drift, as a hand
/// edit leaves it.
fn drifted(file: &str) -> String {
    let mut document = document::read(file.as_bytes()).unwrap();
    let pocket = &mut document.features[3];
    assert_eq!(pocket.id, id("h0000001"));
    pocket.fingerprint.as_mut().unwrap().faces += 1;
    document::write(&document).unwrap()
}

/// `text` opened, a transaction begun, `edit` made in it, and committed: the commit, and the
/// file it left.
fn committed_after(
    text: &str,
    edit: impl Fn(&Fixture),
) -> (silico_core::command::Committed, String) {
    let f = opened(text);
    f.begin();
    edit(&f);
    let commit = answer(f.sessions.commit(&f.id)).result.unwrap();
    (commit, f.file())
}

/// h0000001 removed and added back as it was, its fingerprint left out, before s0000003.
fn removed_and_added_back(f: &Fixture, feature: &str) {
    f.remove("h0000001").result.unwrap();
    let added = f
        .add(&format!(
            r#"{{"session": "s1", "revision": 1, "before": "s0000003", "feature": {feature}}}"#
        ))
        .result
        .unwrap();
    assert_eq!(added.index, 3);
}

/// An edit of the fillet that reaches nothing (its note), so a commit writes the file.
fn noted(f: &Fixture) {
    answer(f.sessions.set_field(
        &f.id,
        f.revision(),
        id("f0000001"),
        "note".to_owned(),
        parsed(r#""Checked""#),
    ))
    .result
    .unwrap();
}

#[test]
fn removed_and_added_back_as_it_was_in_a_transaction_saves_what_never_touching_it_saves() {
    // The saved file is a function of the document's state, not of the edits' path: inside
    // a transaction, h0000001 added back with begin's fields keeps begin's fingerprint, so
    // the transaction's document is begin's, byte for byte, and its commit writes nothing.
    let file = fingerprinted();
    let pocket = unprinted(&document::read(file.as_bytes()).unwrap().features[3]);
    let f = opened(&file);
    f.begin();
    let text = answer(f.sessions.text(&f.id, None)).result.unwrap().text;
    removed_and_added_back(&f, &pocket);
    assert_eq!(
        answer(f.sessions.text(&f.id, None)).result.unwrap().text,
        text
    );
    let commit = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(!commit.written, "{commit:?}");
    assert_eq!(f.file(), file);
    // Beside an edit that writes the file: the same file, and the same commit, as that edit
    // alone.
    let (alone, by_itself) = committed_after(&file, noted);
    let (with, round_trip) = committed_after(&file, |f| {
        removed_and_added_back(f, &pocket);
        noted(f);
    });
    assert!(alone.written);
    assert_eq!(round_trip, by_itself);
    assert_eq!(
        (&with.refingerprinted, &with.problems),
        (&alone.refingerprinted, &alone.problems)
    );
}

#[test]
fn removed_and_added_back_drifted_it_keeps_its_stale_record_and_its_drift_as_untouched() {
    // h0000001 drifted at begin (its stored faces one off): added back as it was, the commit
    // reports the same drift as leaving it untouched does, and the file holds the stale
    // record exactly.
    let file = drifted(&fingerprinted());
    let pocket = unprinted(&document::read(file.as_bytes()).unwrap().features[3]);
    let (alone, by_itself) = committed_after(&file, noted);
    let (with, round_trip) = committed_after(&file, |f| {
        removed_and_added_back(f, &pocket);
        noted(f);
    });
    let drift: Vec<(ErrorCode, Option<&str>)> = alone
        .problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature()))
        .collect();
    assert_eq!(drift, [(ErrorCode::Drift, Some("h0000001"))]);
    assert_eq!(with.problems, alone.problems);
    assert_eq!(with.refingerprinted, alone.refingerprinted);
    assert_eq!(round_trip, by_itself);
    let stored = document::read(round_trip.as_bytes()).unwrap().features[3]
        .fingerprint
        .clone();
    assert_eq!(
        stored,
        document::read(file.as_bytes()).unwrap().features[3]
            .fingerprint
            .clone()
    );
}

#[test]
fn added_back_with_other_values_it_is_reached_and_written_fresh() {
    // h0000001 added back under its ID with another value (its extent, 5 deep): no
    // record of begin's is restored; it's reached, so `check` reports no drift on it, and
    // the commit writes it fresh, as setting the value in place does.
    let file = fingerprinted();
    let mut other = document::read(file.as_bytes()).unwrap().features[3].clone();
    other.fingerprint = None;
    let text = document::write_feature(&other).unwrap().replace(
        r#""extent": {"type": "through_all", "reversed": true}"#,
        r#""extent": {"type": "one_side", "length": 5, "reversed": true}"#,
    );
    assert!(text.contains(r#""length": 5"#), "{text}");
    let f = opened(&file);
    f.begin();
    f.remove("h0000001").result.unwrap();
    let added = f
        .add(&format!(
            r#"{{"session": "s1", "revision": 1, "before": "s0000003", "feature": {text}}}"#
        ))
        .result
        .unwrap();
    assert_eq!(added.feature.fingerprint, None);
    let check = answer(f.sessions.check(
        &f.id,
        silico_core::command::CheckModel::Current { fresh: false },
    ))
    .result
    .unwrap();
    assert!(check.problems.is_empty(), "{:?}", check.problems.errors());
    let commit = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(commit.written);
    assert_eq!(
        commit.refingerprinted,
        [id("h0000001"), id("h0000002"), id("f0000001")]
    );
    // As the value set in place, never removed.
    let (set, in_place) = committed_after(&file, |f| {
        answer(f.sessions.set_field(
            &f.id,
            f.revision(),
            id("h0000001"),
            "extent".to_owned(),
            parsed(r#"{"type": "one_side", "length": 5, "reversed": true}"#),
        ))
        .result
        .unwrap();
    });
    assert_eq!(f.file(), in_place);
    assert_eq!(commit.refingerprinted, set.refingerprinted);
}

#[test]
fn a_pad_roots_way_back_is_its_insert_then_the_pads_base_set() {
    // A pad root under a pad: removed, the pad begins the body (`to` null); added again at
    // its own place, nothing rewired (FreeCAD's insert would rewire the pad), the pad still
    // with no base; then the pad's base set to it: the file byte for byte, each edit its
    // own commit.
    let second = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "More", "base": "p0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let f = opened(&text_of(&[S0000001, P0000001, second]));
    answer(f.sessions.format(&f.id, None, true)).result.unwrap();
    let file = f.file();
    let removed = f.remove("p0000001").result.unwrap();
    assert_eq!(
        removed.rewired,
        [rewired("p0000002", "base", Some("p0000001"), None)]
    );
    let feature = unprinted(&removed.feature);
    let added = f
        .add(&format!(
            r#"{{"session": "s1", "revision": 1, "before": "p0000002", "feature": {feature}}}"#
        ))
        .result
        .unwrap();
    assert_eq!(added.index, 1);
    assert!(added.rewired.is_empty(), "{:?}", added.rewired);
    assert_eq!(
        f.chain()[2],
        ("p0000002".to_owned(), None),
        "the pad still begins its body"
    );
    assert_ne!(f.file(), file);
    let set = answer(f.sessions.set_field(
        &f.id,
        f.revision(),
        id("p0000002"),
        "base".to_owned(),
        parsed(r#""p0000001""#),
    ))
    .result
    .unwrap();
    assert!(set.changed);
    assert!(set.commit.unwrap().written);
    assert_eq!(f.file(), file);
}

#[test]
fn a_move_past_unrelated_features_rewires_nothing_and_writes_no_fingerprint() {
    // h0000001 past s0000003, still between p0000001 and h0000002: outside a transaction,
    // its own commit, which writes the new order and no fingerprint.
    let file = fingerprinted();
    let f = opened(&file);
    let moved = f.moved("h0000001", Some("h0000002")).result.unwrap();
    assert_eq!((moved.from, moved.to, moved.changed), (3, 4, true));
    assert!(moved.rewired.is_empty());
    let commit = moved.commit.unwrap();
    assert!(commit.written);
    assert!(commit.refingerprinted.is_empty(), "{commit:?}");
    assert!(commit.resolved.is_empty(), "{commit:?}");
    // The file: the features as they were, fingerprints and all, h0000001 after s0000003.
    let mut want = document::read(file.as_bytes()).unwrap();
    want.features.swap(3, 4);
    assert_eq!(f.file(), document::write(&want).unwrap());
}

#[test]
fn a_move_to_where_a_feature_is_changes_nothing_and_a_move_to_the_end_takes_the_last_as_base() {
    let f = opened(&bracket());
    f.inside_at(12);
    for before in [Some("s0000003")] {
        let moved = f.moved("h0000001", before).result.unwrap();
        assert_eq!(
            (moved.from, moved.to, moved.changed, moved.commit),
            (3, 3, false, None),
            "{before:?}"
        );
        assert!(moved.rewired.is_empty());
        assert_eq!(f.revision(), revision(12));
    }
    let value: serde_json::Value =
        serde_json::from_str(&written(&f.moved("f0000001", None).result.unwrap())).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"feature": "f0000001", "from": 6, "to": 6, "rewired": [], "changed": false, "commit": null})
    );
    // h0000001 to the end (from 3 to 6): h0000002 takes p0000001, and h0000001 builds on
    // the fillet, the chain's last.
    let moved = f.moved("h0000001", None).result.unwrap();
    assert_eq!((moved.from, moved.to), (3, 6));
    assert_eq!(
        moved.rewired,
        [
            rewired("h0000002", "base", Some("h0000001"), Some("p0000001")),
            rewired("h0000001", "base", Some("p0000001"), Some("f0000001")),
        ]
    );
    assert_eq!(f.revision(), revision(13));
}

/// A block b0000001, the tool chain p0000002 (a 20 mm cube), c0000002 (a chamfer of its
/// Edge1), and c0000003 (a chamfer, on c0000002, of the cube's Edge3, which shares no vertex
/// with Edge1), and the fuse w0000001 of the block and that chain, as its tool: the design's
/// test plan's (its `u1`: `u` isn't among an ID's letters).
fn tool_chain() -> String {
    text_of(&[
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 40, "height": 10}"#,
        r#"{"id": "p0000002", "type": "box", "v": 1, "name": "Cube", "position": [10, 10, 10], "length": 20, "width": 20, "height": 20}"#,
        r#"{"id": "c0000002", "type": "chamfer", "v": 1, "name": "One", "base": "p0000002", "edges": ["p0000002.box.Edge1"], "size": {"distance": 1}}"#,
        r#"{"id": "c0000003", "type": "chamfer", "v": 1, "name": "Two", "base": "c0000002", "edges": ["p0000002.box.Edge3"], "size": {"distance": 1}}"#,
        r#"{"id": "w0000001", "type": "fuse", "v": 1, "name": "Both", "base": "b0000001", "tool": "c0000003"}"#,
    ])
}

#[test]
fn a_tool_chains_move_rewires_its_booleans_tool_and_one_past_the_boolean_is_refused() {
    let f = opened(&tool_chain());
    f.begin();
    // c0000003 before c0000002: rewired, in timeline order after the move, c0000003's base
    // from c0000002 to p0000002, c0000002's from p0000002 to c0000003, and the fuse's tool
    // from c0000003 to c0000002.
    let moved = f.moved("c0000003", Some("c0000002")).result.unwrap();
    assert_eq!((moved.from, moved.to), (3, 2));
    assert_eq!(
        moved.rewired,
        [
            rewired("c0000003", "base", Some("c0000002"), Some("p0000002")),
            rewired("c0000002", "base", Some("p0000002"), Some("c0000003")),
            rewired("w0000001", "tool", Some("c0000003"), Some("c0000002")),
        ]
    );
    let value: serde_json::Value = serde_json::from_str(&written(&moved)).unwrap();
    assert_eq!(
        value["rewired"][2],
        serde_json::json!({"feature": "w0000001", "field": "tool", "from": "c0000003", "to": "c0000002"})
    );
    let ids: Vec<String> = f.chain().into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        ids,
        ["b0000001", "p0000002", "c0000003", "c0000002", "w0000001"]
    );
    // The model builds: each feature healthy but for its missing fingerprint.
    let timeline = answer(f.sessions.timeline(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    for entry in timeline.features.unwrap() {
        assert_eq!(entry.problems, 0, "{entry:?}");
    }
    answer(f.sessions.rollback(&f.id)).result.unwrap();
    f.begin();
    // c0000003 past the fuse: refused, naming the fuse.
    let error = the_error(&f.moved("c0000003", None));
    assert_eq!(
        error.message(),
        "c0000003 would follow c0000002, the end of the chain w0000001 consumes as its tool: a move never makes a feature another's tool"
    );
    assert_eq!(error.feature(), Some("c0000003"));
    // c0000002 before p0000002, whose elements it names: before its chain's root, which
    // nothing there can stand in for.
    let error = the_error(&f.moved("c0000002", Some("p0000002")));
    assert_eq!(
        error.message(),
        "c0000002, a chamfer, can't be placed before p0000002, its chain's root: it needs a base"
    );
}

#[test]
fn a_root_moved_under_a_feature_that_needs_it_lists_the_earlier_solids_nothing_consumes() {
    // Two earlier boxes nothing consumes, whose IDs run opposite to their places (z first,
    // then a), beside an earlier box another feature consumes (the fuse's tool) and an
    // earlier sketch: the root's move past the fuse that needs it refused, its candidates
    // the two in timeline order, its suggestion the set_field then the move.
    let f = opened(&text_of(&[
        r#"{"id": "z0000001", "type": "box", "v": 1, "name": "Z", "position": [100, 0, 0], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "a0000001", "type": "box", "v": 1, "name": "A", "position": [200, 0, 0], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "m0000001", "type": "box", "v": 1, "name": "M", "position": [5, 5, 5], "length": 1, "width": 1, "height": 10}"#,
        S0000001,
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "Root", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        r#"{"id": "x0000001", "type": "fuse", "v": 1, "name": "X", "base": "b0000001", "tool": "m0000001"}"#,
    ]));
    f.begin();
    let at = f.revision();
    let error = the_error(&f.moved("b0000001", None));
    assert_eq!(
        error.message(),
        "b0000001 is the root of x0000001's chain, and x0000001, a fuse, can't be left without a base"
    );
    assert_eq!(
        (error.feature(), error.parameter()),
        (Some("b0000001"), Some("feature"))
    );
    assert_eq!(
        error.candidates(),
        ["z0000001".to_owned(), "a0000001".to_owned()]
    );
    assert_eq!(
        error.suggestion(),
        Some("set x0000001's base to one of the candidates first (set_field), then move b0000001")
    );
    assert_eq!(f.revision(), at);
    // A primitive given a base: a box moved after the pad on it would take the pad as its
    // base. Refused, naming both.
    let pad = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "More", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let f = opened(&text_of(&[
        S0000001,
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "Root", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        pad,
    ]));
    f.begin();
    let error = the_error(&f.moved("b0000001", None));
    assert_eq!(
        (error.message(), error.feature()),
        (
            "b0000001, a box, takes no base, and the place asked for comes after p0000002 in its chain",
            Some("b0000001")
        )
    );
}

#[test]
fn the_lineage_rule_refuses_a_fuses_move_past_and_its_removal_under_a_fillet_of_its_tool() {
    // Open item 2b: boxes b0000001 and t0000001, their fuse j0000001, and a fillet
    // r0000001 on it naming t0000001.box.Edge1. Moving the fuse after the fillet, and
    // removing it, are each refused, naming the fillet and the tool; removing the fillet
    // first, then the fuse, goes ahead.
    let f = opened(&text_of(&[
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        r#"{"id": "t0000001", "type": "box", "v": 1, "name": "Tool", "position": [5, 5, 5], "length": 10, "width": 10, "height": 10}"#,
        r#"{"id": "j0000001", "type": "fuse", "v": 1, "name": "Join", "base": "b0000001", "tool": "t0000001"}"#,
        r#"{"id": "r0000001", "type": "fillet", "v": 1, "name": "Round", "base": "j0000001", "edges": ["t0000001.box.Edge1"], "radius": 0.5}"#,
    ]));
    f.begin();
    let at = f.revision();
    let error = the_error(&f.moved("j0000001", None));
    assert_eq!(
        error.message(),
        "r0000001 names elements of t0000001, which its base would no longer be built from once j0000001 moves there"
    );
    assert_eq!(
        (error.feature(), error.parameter()),
        (Some("j0000001"), Some("before"))
    );
    let error = the_error(&f.remove("j0000001"));
    assert_eq!(
        error.message(),
        "r0000001 names elements of t0000001, which its base would no longer be built from without j0000001"
    );
    assert_eq!(
        (error.feature(), error.parameter(), error.suggestion()),
        (
            Some("j0000001"),
            Some("feature"),
            Some("remove r0000001 first")
        )
    );
    assert_eq!(f.revision(), at);
    let fillet = f.remove("r0000001").result.unwrap();
    assert!(fillet.rewired.is_empty());
    let fuse = f.remove("j0000001").result.unwrap();
    assert!(fuse.rewired.is_empty());
    let ids: Vec<String> = f.chain().into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["b0000001", "t0000001"]);
}

#[test]
fn a_move_leaving_a_name_already_broken_after_its_maker_is_the_gates_to_refuse() {
    // The fillet on b0000001 names t0000001's edge, which its base was never built from:
    // broken already, so not the lineage rule's. Moving t0000001 to the end leaves the name
    // a maker the fillet comes before, which the gate refuses in its own words, naming the
    // fillet; the session as it was.
    let f = opened(&text_of(&[
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        r#"{"id": "t0000001", "type": "box", "v": 1, "name": "Tool", "position": [20, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        r#"{"id": "r0000001", "type": "fillet", "v": 1, "name": "Round", "base": "b0000001", "edges": ["t0000001.box.Edge1"], "radius": 1}"#,
    ]));
    f.begin();
    let at = f.revision();
    let text = answer(f.sessions.text(&f.id, None)).result.unwrap().text;
    let error = the_error(&f.moved("t0000001", None));
    assert_eq!(
        (error.code(), error.feature(), error.parameter()),
        (ErrorCode::InvalidInput, Some("r0000001"), Some("edges.0"))
    );
    assert_eq!(
        error.message(),
        r#"edges.0, "t0000001.box.Edge1", isn't an edge's name: no feature t0000001 before this one, at column 1"#
    );
    assert_eq!(f.revision(), at);
    assert_eq!(
        answer(f.sessions.text(&f.id, None)).result.unwrap().text,
        text
    );
}

#[test]
fn an_inserted_features_reader_errors_name_its_place_in_the_timeline() {
    // A chamfer with an ID that isn't one, inserted before h0000002: the reader names it by
    // the place it would take, features[5], at the request's feature.id.
    let f = opened(&bracket());
    f.begin();
    let request =
        example("add_feature/request-before").replace(r#""id": "c0000002""#, r#""id": "C0000002""#);
    assert!(request.contains("C0000002"));
    let error = the_error(&f.add(&request));
    assert_eq!(
        (error.feature(), error.parameter()),
        (Some("features[5]"), Some("feature.id"))
    );
}

#[test]
fn a_timelines_counts_are_each_its_own_and_an_older_models_revision_is_named() {
    // The bracket, no fingerprints stored, and a fillet f0000002 on the end of its chain
    // whose edge name nothing makes. By hand: every solid that builds warns of its missing
    // fingerprint (problems 0, warnings 1), sketches neither; f0000002 fails on its broken
    // reference (problems 1) and, failed, isn't warned of (warnings 0).
    let broken = r#"{"id": "f0000002", "type": "fillet", "v": 1, "name": "Broken", "base": "f0000001", "edges": ["p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000001, G s0000001.sketch.Vertex.pa000003)"], "radius": 1}"#;
    let f = opened(&text_of(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001, broken,
    ]));
    f.begin();
    let current = answer(f.sessions.timeline(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    let built = current.model.unwrap();
    // An edit the model needn't rebuild for: the session moves on, the published model
    // stays the older revision's.
    answer(f.sessions.set_field(
        &f.id,
        f.revision(),
        id("s0000002"),
        "note".to_owned(),
        parsed(r#""Holes""#),
    ))
    .result
    .unwrap();
    let response = answer(f.sessions.timeline(&f.id, ModelChoice::Published));
    let text = written(response.result.as_ref().unwrap());
    holds(&schema_of("timeline", "result"), &text, "counts");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["revision"], built.number() + 1);
    assert_eq!(value["model_revision"], built.number());
    let marks: Vec<(String, String, u64, u64)> = value["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_owned(),
                e["health"].as_str().unwrap().to_owned(),
                e["problems"].as_u64().unwrap(),
                e["warnings"].as_u64().unwrap(),
            )
        })
        .collect();
    let want: Vec<(String, String, u64, u64)> = [
        ("s0000001", "healthy", 0, 0),
        ("p0000001", "warning", 0, 1),
        ("s0000002", "healthy", 0, 0),
        ("h0000001", "warning", 0, 1),
        ("s0000003", "healthy", 0, 0),
        ("h0000002", "warning", 0, 1),
        ("f0000001", "warning", 0, 1),
        ("f0000002", "error", 1, 0),
    ]
    .iter()
    .map(|(i, h, p, w)| ((*i).to_owned(), (*h).to_owned(), *p, *w))
    .collect();
    assert_eq!(marks, want);
    // On the wire, each count under its own key, literally.
    assert!(
        text.contains(r#""id": "f0000002", "type": "fillet", "name": "Broken", "suppressed": false, "inputs": ["f0000001"], "health": "error", "problems": 1, "warnings": 0}"#),
        "{text}"
    );
    assert!(
        text.contains(r#""id": "h0000001", "type": "pocket", "name": "Base holes", "suppressed": false, "inputs": ["p0000001", "s0000002"], "health": "warning", "problems": 0, "warnings": 1}"#),
        "{text}"
    );
}

#[test]
fn a_before_the_document_hasnt_is_refused_with_the_features_own_refusals_at_once() {
    // A `before` the document hasn't, and a feature that gives a fingerprint: both refused
    // in one answer, `before` first, as every edit gives every reason at once.
    let f = opened(&bracket());
    f.begin();
    let at = f.revision();
    let request = with(
        &example("add_feature/request-before"),
        "before",
        r#""z0000001""#,
    )
    .replace(
        r#""name":"Base edge","#,
        r#""name":"Base edge","fingerprint":{"volume":1,"area":1,"faces":1,"edges":1,"vertices":1},"#,
    );
    assert!(request.contains("fingerprint"), "{request}");
    let refused = f.add(&request);
    let errors = refused.result.as_ref().unwrap_err();
    let said: Vec<(Option<&str>, &str)> = errors
        .errors()
        .iter()
        .map(|e| (e.parameter(), e.message()))
        .collect();
    assert_eq!(
        said,
        [
            (
                Some("before"),
                "the document has no feature z0000001 to add this one before"
            ),
            (
                Some("feature.fingerprint"),
                "a new feature has no fingerprint: a commit writes it"
            ),
        ]
    );
    assert_eq!(errors.total(), 2);
    assert_eq!(f.revision(), at);
}
