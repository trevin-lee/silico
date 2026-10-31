//! M4.2's other session commands on the wire (docs/design/m4.1-command-api.md, sections 3,
//! 6, and 7): `version`, `open`, `close`, `status`, `cancel`, `text`, `outline`, and `check`.
//! Each design request read, written back byte for byte, and held to its snapshot; each
//! result and refusal the design gives made by the session, on the fake kernel, from the
//! request's text (a path as `open` was given it, in a directory of the test's own, said as
//! the design's `bracket.silico`), written to its example file, and held to its schema;
//! `status`'s, which names a read no command has yet, built by hand. `version`'s and
//! `status`'s results aren't the design's yet (its limits of commands not built, its
//! progress), so each is pinned here as written by hand. Expected values are the design's
//! text (each file its own block: wire_session_commands.rs checks that), and, for values the
//! design hasn't, written by hand. A published `check` answers at once from the newest model
//! published, while the current revision's rebuild is held.

use std::path::PathBuf;
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::wire::reads::{CheckRequest, TextRequest};
use silico_core::command::wire::session::{
    CANCELLABLE, CancelRequest, OpenRequest, PUBLISHED_READS, SESSION_COMMANDS, VersionRequest,
};
use silico_core::command::wire::transaction::SessionRequest;
use silico_core::command::wire::{Layout, ToWire, response, response_text, snapshot_value, write};
use silico_core::command::{
    Cancellable, Cancelled, CheckModel, Limits, MAX_SESSIONS, ModelChoice, Opened, OutlineAnswer,
    Problems, Progress, ReadCommand, Rebuild, RebuildState, Response, Revision, RunningCommand,
    SessionCommand, SessionId, Sessions, Status, TextAnswer, Transaction, VERSION, Version,
};
use silico_core::disk::Disk;
use silico_core::document::json::{Node, read};
use silico_core::fake::{FakeKernel, FakeSketches, Hold};
use silico_core::kernel::{Op, Primitive};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, document};
use tempfile::TempDir;

use crate::sessions::{HANG, answer, id};

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

/// The example `name` (`open/request`), as committed.
fn example(name: &str) -> String {
    committed(&format!("commands/examples/{name}.json"))
}

/// `value` as the API writes it, and a newline, as each example file holds it.
fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

/// `value` as the API writes it on one line.
fn one_line<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::OneLine).unwrap()
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

/// The design's document (M2.1's block with a hole through it): the contract's first three
/// features, the box, the pin, and the cut, with their fingerprints and the cut's note, in
/// canonical form.
fn block() -> String {
    let mut document = document::read(committed("naming-scheme-5.silico").as_bytes()).unwrap();
    document.features.truncate(3);
    document::write(&document).unwrap()
}

/// A box, with no fingerprint: a document the fake kernel finds no problem with.
const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

/// The design's document with no features, as `text`'s example gives it.
const EMPTY: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": []\n}\n";

struct Fixture {
    dir: TempDir,
    kernel: Arc<FakeKernel>,
    sessions: Sessions<Arc<FakeKernel>>,
}

/// Sessions on the fake kernel, in a directory of their own, nothing open.
fn sessions() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    Fixture {
        dir,
        kernel,
        sessions,
    }
}

/// Sessions on the fake kernel, with `text` in `bracket.silico` open as s1.
fn opened(text: &str) -> Fixture {
    let f = sessions();
    std::fs::write(f.path(), text).unwrap();
    let id = answer(f.sessions.open(f.path(), false)).session.unwrap().id;
    assert_eq!(id, s1());
    f
}

fn is_box(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
}

impl Fixture {
    /// The session's file, `bracket.silico` in the test's directory.
    fn path(&self) -> PathBuf {
        self.dir.path().join("bracket.silico")
    }

    /// `written`, which names the test's directory, as the design's: as a session opened as
    /// `bracket.silico`, in the directory it's in, names it.
    fn here(&self, written: &str) -> String {
        let dir = format!("{}/", self.dir.path().display());
        assert!(written.contains(&dir), "{written}");
        written.replace(&dir, "")
    }

    /// The design's `open` request `text`, run, its path the session's file.
    fn open(&self, text: &str) -> Response<Opened> {
        let request = OpenRequest::read(&parsed(text)).unwrap();
        assert_eq!(request.path, "bracket.silico");
        answer(
            self.sessions
                .open(self.dir.path().join(&request.path), request.create),
        )
    }

    /// The fake's box held until the hold is dropped; and `running` started and waited for
    /// until the box is held.
    fn running_held<T>(&self, running: impl FnOnce() -> T) -> (Hold, T) {
        let hold = self.kernel.hold(is_box);
        let running = running();
        assert!(self.kernel.wait_running(1, HANG));
        (hold, running)
    }
}

#[test]
fn the_designs_requests_are_read_written_back_byte_for_byte_and_hold_to_their_schemas() {
    let text = example("version/request");
    assert_eq!(text, "{}\n");
    let request = VersionRequest::read(&parsed(&text)).unwrap();
    assert_eq!(written(&request), text);
    holds(&schema_of("version", "request"), &text, "version");

    let text = example("open/request");
    let request = OpenRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        OpenRequest {
            path: "bracket.silico".to_owned(),
            create: false,
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("open", "request"), &text, "open");

    for of in ["close", "status", "outline"] {
        let text = example(&format!("{of}/request"));
        let request = SessionRequest::read(&parsed(&text)).unwrap();
        assert_eq!(request, SessionRequest { session: s1() });
        assert_eq!(written(&request), text, "{of}");
        holds(&schema_of(of, "request"), &text, of);
    }

    let text = example("cancel/request");
    let request = CancelRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        CancelRequest {
            session: s1(),
            command: SessionCommand::Check.into(),
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("cancel", "request"), &text, "cancel");

    let text = example("text/request");
    let request = TextRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        TextRequest {
            session: s1(),
            feature: None,
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("text", "request"), &text, "text");

    let text = example("check/request");
    let request = CheckRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        CheckRequest {
            session: s1(),
            model: CheckModel::Current { fresh: false },
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("check", "request"), &text, "check");
}

#[test]
fn requests_with_their_other_values_are_read_and_written_back() {
    // By hand: each value the design's requests don't give.
    for (text, want) in [
        (
            "{\"path\": \"a/b c.silico\", \"create\": true}\n",
            OpenRequest {
                path: "a/b c.silico".to_owned(),
                create: true,
            },
        ),
        (
            "{\"path\": \"\\u00e9.silico\", \"create\": false}\n",
            OpenRequest {
                path: "\u{e9}.silico".to_owned(),
                create: false,
            },
        ),
    ] {
        let request = OpenRequest::read(&parsed(text)).unwrap();
        assert_eq!(request, want);
        assert_eq!(
            OpenRequest::read(&parsed(&written(&request))).unwrap(),
            want
        );
        holds(&schema_of("open", "request"), text, text);
    }
    let text = "{\"session\": \"s12\", \"feature\": \"k7f2qx3m\"}\n";
    let request = TextRequest::read(&parsed(text)).unwrap();
    assert_eq!(
        request,
        TextRequest {
            session: SessionId::parse("s12").unwrap(),
            feature: Some(id("k7f2qx3m")),
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("text", "request"), text, "a feature");
    let text = "{\"session\": \"s2\", \"model\": \"published\", \"fresh\": false}\n";
    let request = CheckRequest::read(&parsed(text)).unwrap();
    assert_eq!(request.model, CheckModel::Published);
    assert_eq!(written(&request), text);
    holds(&schema_of("check", "request"), text, "published");
    // A fresh check, of the current model (M4.3).
    let text = "{\"session\": \"s3\", \"model\": \"current\", \"fresh\": true}\n";
    let request = CheckRequest::read(&parsed(text)).unwrap();
    assert_eq!(
        request,
        CheckRequest {
            session: SessionId::parse("s3").unwrap(),
            model: CheckModel::Current { fresh: true },
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("check", "request"), text, "fresh");
    // Every command that runs in a session can be named, and the rebuild, each written back
    // as named.
    for name in CANCELLABLE {
        let text = format!("{{\"session\": \"s1\", \"command\": \"{name}\"}}\n");
        let request = CancelRequest::read(&parsed(&text)).unwrap();
        assert_eq!(request.command.as_str(), name);
        assert_eq!(written(&request), text);
        holds(&schema_of("cancel", "request"), &text, name);
    }
    // The rebuild (decision 34 as amended): read as itself, no command, and its result
    // written as the decision gives it.
    let text = "{\"session\": \"s1\", \"command\": \"rebuild\"}\n";
    assert_eq!(
        CancelRequest::read(&parsed(text)).unwrap(),
        CancelRequest {
            session: s1(),
            command: Cancellable::Rebuild,
        }
    );
    let stopped = Cancelled {
        command: Cancellable::Rebuild,
        stopped: true,
    };
    assert_eq!(
        one_line(&stopped),
        r#"{"command": "rebuild", "stopped": true}"#
    );
    holds(
        &schema_of("cancel", "result"),
        &written(&stopped),
        "rebuild",
    );
}

#[test]
fn version_and_open_take_only_their_fields_and_require_each() {
    let refused = |read: fn(&Node) -> Result<(), Problems>, text: &str| {
        said(&read(&parsed(text)).unwrap_err())
    };
    let version: fn(&Node) -> Result<(), Problems> = |n| VersionRequest::read(n).map(drop);
    let open: fn(&Node) -> Result<(), Problems> = |n| OpenRequest::read(n).map(drop);
    // `version` has no fields: any key is refused, and so is anything but an object.
    assert_eq!(
        refused(version, r#"{"session": "s1"}"#),
        [(
            "session".to_owned(),
            "session isn't a field of this request".to_owned()
        )]
    );
    fails(
        &schema_of("version", "request"),
        r#"{"session": "s1"}"#,
        "a key",
    );
    assert_eq!(
        refused(version, "[]"),
        [(
            String::new(),
            "the request must be an object, got an array".to_owned()
        )]
    );
    fails(&schema_of("version", "request"), "[]", "an array");
    // `open`: each field required, of its type, and no other.
    let text = example("open/request");
    for (changed, want) in [
        (
            r#"{"create": false}"#.to_owned(),
            vec![("path", "path is missing".to_owned())],
        ),
        (
            r#"{"path": "bracket.silico"}"#.to_owned(),
            vec![("create", "create is missing".to_owned())],
        ),
        (
            text.replacen('{', r#"{"session": "s1", "#, 1),
            vec![(
                "session",
                "session isn't a field of this request".to_owned(),
            )],
        ),
        (
            text.replace(r#""bracket.silico""#, "null")
                .replace("false", r#""false""#),
            vec![
                ("path", "path must be a string, got null".to_owned()),
                (
                    "create",
                    r#"create must be true or false, got "false""#.to_owned(),
                ),
            ],
        ),
    ] {
        let want: Vec<(String, String)> =
            want.into_iter().map(|(p, m)| (p.to_owned(), m)).collect();
        assert_eq!(refused(open, &changed), want, "{changed}");
        fails(&schema_of("open", "request"), &changed, &changed);
    }
}

#[test]
fn cancel_names_only_a_command_and_any_other_name_is_refused_with_every_command_as_candidates() {
    let text = example("cancel/request");
    let names: Vec<String> = CANCELLABLE.map(str::to_owned).to_vec();
    // Every command that runs in a session, then the rebuild: 33 and one, counted by hand
    // from the design's lists (M4.1's 20, M5.0's 13, sketch_join the last of them).
    assert_eq!(names.len(), 34);
    assert_eq!(names[..33], SESSION_COMMANDS.map(str::to_owned));
    assert_eq!(names[33], "rebuild");
    // A name close to one, the session's own probe, commands that never run in a session
    // (`read_logs` among them, answered at once), and other types.
    for (value, got) in [
        (r#""Check""#, "Check"),
        (r#""check ""#, r#""check ""#),
        (r#""read_logs""#, "read_logs"),
        (r#""probe""#, "probe"),
        // The session's own reload, which nothing cancels, and the rebuild misspelt.
        (r#""reload""#, "reload"),
        (r#""Rebuild""#, "Rebuild"),
        (r#""status""#, "status"),
        (r#""open""#, "open"),
        (r#""version""#, "version"),
        (r#""""#, r#""""#),
        ("null", "null"),
        ("1", "1"),
    ] {
        let changed = text.replace(r#""check""#, value);
        let problems = CancelRequest::read(&parsed(&changed)).unwrap_err();
        assert_eq!(problems.total(), 1, "{value}");
        let error = &problems.errors()[0];
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(error.parameter(), Some("command"));
        assert_eq!(
            error.message(),
            format!(
                "command must be {}, or {}, got {got}",
                names[..names.len() - 1].join(", "),
                names[names.len() - 1]
            ),
            "{value}"
        );
        assert_eq!(error.suggestion(), Some("give one of the commands"));
        assert_eq!(error.candidates(), names.as_slice(), "{value}");
        fails(&schema_of("cancel", "request"), &changed, value);
    }
    // The schema's names are those, in order, and its result names one too.
    let request = schema_of("cancel", "request");
    assert_eq!(
        request["$defs"]["SessionCommand"]["enum"],
        serde_json::json!(names)
    );
    let result = schema_of("cancel", "result");
    assert_eq!(
        result["$defs"]["SessionCommand"]["enum"],
        serde_json::json!(names)
    );
    fails(
        &result,
        r#"{"command": "probe", "stopped": true}"#,
        "a result naming no command",
    );
}

#[test]
fn checks_fresh_is_refused_with_the_published_model_and_taken_with_the_current() {
    // The design's section 7: "fresh" true only with current. With published, refused as
    // fields that must agree are (a camera's projection's), naming `fresh`; the snapshot
    // takes either value, since which goes with which is the reader's rule.
    let text = example("check/request")
        .replace("current", "published")
        .replace("false", "true");
    assert_eq!(
        text,
        "{\"session\": \"s1\", \"model\": \"published\", \"fresh\": true}\n"
    );
    let problems = CheckRequest::read(&parsed(&text)).unwrap_err();
    assert_eq!(problems.total(), 1);
    let error = &problems.errors()[0];
    assert_eq!(
        (
            error.code(),
            error.message(),
            error.parameter(),
            error.cause(),
            error.suggestion()
        ),
        (
            ErrorCode::InvalidInput,
            "fresh must be false for a read of the published model",
            Some("fresh"),
            Some(
                "a read of the published model answers from the newest model as it is, \
                 rebuilding nothing"
            ),
            Some(
                "give model current with fresh true to rebuild the current revision anew, or \
                 fresh false to read the published model"
            )
        )
    );
    // The same with current is read, and true.
    let current = example("check/request").replace("false", "true");
    assert_eq!(
        CheckRequest::read(&parsed(&current)).unwrap().model,
        CheckModel::Current { fresh: true }
    );
    // A session that isn't one is refused beside it, every reason at once.
    let both = text.replace("s1", "s0");
    assert_eq!(
        said(&CheckRequest::read(&parsed(&both)).unwrap_err()),
        [
            (
                "session".to_owned(),
                r#"session must be a session's ID, s1, s2, ..., got "s0""#.to_owned()
            ),
            (
                "fresh".to_owned(),
                "fresh must be false for a read of the published model".to_owned()
            ),
        ]
    );
    // Its type, and the others', refused with every reason.
    assert_eq!(
        said(
            &CheckRequest::read(&parsed(
                r#"{"session": "s0", "model": "newest", "fresh": "false"}"#
            ))
            .unwrap_err()
        ),
        [
            (
                "session".to_owned(),
                r#"session must be a session's ID, s1, s2, ..., got "s0""#.to_owned()
            ),
            (
                "model".to_owned(),
                "model must be current or published, got newest".to_owned()
            ),
            (
                "fresh".to_owned(),
                r#"fresh must be true or false, got "false""#.to_owned()
            ),
        ]
    );
}

#[test]
fn version_is_answered_with_every_limit_as_the_design_gives_them_and_no_session() {
    // The design's own result, byte for byte, every limit at its value (section 6).
    let text = written(&VERSION);
    let designed = &design("### `version` (M4.2)", "### `open` (M4.2)")[1];
    assert_eq!(text, format!("{designed}\n"));
    assert_eq!(text, example("version/result"));
    holds(&schema_of("version", "result"), &text, "version");
    // Its envelope: no session, no model.
    let ran = Response {
        session: None,
        model: None,
        result: Ok(VERSION),
    };
    let envelope = response_text(&ran, Layout::OneLine);
    assert_eq!(
        envelope,
        format!(
            r#"{{"ok": true, "session": null, "model": null, "result": {}}}"#,
            one_line(&VERSION)
        )
    );
    holds(
        &serde_json::from_str(&committed("commands/envelope.json")).unwrap(),
        &envelope,
        "version's envelope",
    );
    assert_eq!(
        write(&response(&ran), Layout::Canonical).unwrap(),
        format!(
            "{{\n  \"ok\": true,\n  \"session\": null,\n  \"model\": null,\n  \"result\": {{\n    \"api\": 1,\n    \"format\": 1,\n    \"naming\": 5,\n    \"limits\": {}\n  }}\n}}",
            one_line(&VERSION.limits)
        )
    );
    // Each field from its own value, none another's.
    let other = Version {
        api: 2,
        format: 3,
        naming: 4,
        limits: Limits {
            document_bytes: 11,
            request_bytes: 12,
            request_depth: 13,
            response_bytes: 14,
            listed: 15,
            sessions: 16,
            published_reads: 17,
            elements: 18,
            query: 19,
            query_measures: 20,
            highlight: 23,
            render_side: 24,
            render_seconds: 25,
            log_read_events: 26,
            log_read_bytes: 27,
            log_events: 28,
            log_bytes: 29,
            export_tolerance: [
                document::Length::from_nm(1),
                document::Length::from_nm(30_000_000),
            ],
            export_triangles: 31,
            export_bytes: 32,
            kernel_seconds: 21,
            sketch_seconds: 22,
            undo_bytes: 33,
            disk_check_ms: 34,
        },
    };
    assert_eq!(
        one_line(&other),
        r#"{"api": 2, "format": 3, "naming": 4, "limits": {"document_bytes": 11, "request_bytes": 12, "request_depth": 13, "response_bytes": 14, "listed": 15, "sessions": 16, "published_reads": 17, "elements": 18, "query": 19, "query_measures": 20, "highlight": 23, "render_side": 24, "render_seconds": 25, "log_read_events": 26, "log_read_bytes": 27, "log_events": 28, "log_bytes": 29, "export_tolerance": [0.000001, 30], "export_triangles": 31, "export_bytes": 32, "kernel_seconds": 21, "sketch_seconds": 22, "undo_bytes": 33, "disk_check_ms": 34}}"#
    );
    holds(&schema_of("version", "result"), &written(&other), "another");
}

#[test]
fn open_answers_and_refuses_as_the_design_gives_it() {
    let error = error_schema();
    let check = |name: &str, got: String| {
        assert_eq!(got, example(name), "{name}");
        holds(&error, &got, name);
    };
    // The design's block, opened: its three features, the cut its one body.
    let f = sessions();
    std::fs::write(f.path(), block()).unwrap();
    let opened = f.open(&example("open/request"));
    assert_eq!(opened.session.as_ref().unwrap().id, s1());
    let text = f.here(&written(opened.result.as_ref().unwrap()));
    assert_eq!(text, example("open/result"));
    holds(&schema_of("open", "result"), &text, "opened");

    // The same file again: open already, in s1.
    let again = f.open(&example("open/request"));
    assert_eq!(again.session, None);
    check("open/refused-open-already", f.here(&the_refusal(&again)));
    // Made, where it is.
    let made = f.open(&example("open/request").replace("false", "true"));
    assert_eq!(made.session, None);
    check("open/refused-exists", f.here(&the_refusal(&made)));

    // A box of no length: the document opens, the gate's problem listed.
    let f = sessions();
    let zero = block().replacen("\"length\": 40", "\"length\": 0", 1);
    assert_ne!(zero, block());
    std::fs::write(f.path(), &zero).unwrap();
    let opened = f.open(&example("open/request"));
    assert!(opened.session.is_some());
    let text = f.here(&written(opened.result.as_ref().unwrap()));
    assert_eq!(text, example("open/refused-document"));
    holds(&schema_of("open", "result"), &text, "refused document");

    // No file there.
    let f = sessions();
    let missing = f.open(&example("open/request"));
    assert_eq!(missing.session, None);
    check("open/refused-missing", the_refusal(&missing));

    // As many sessions open as a process holds: the next refused.
    let f = sessions();
    for i in 0..MAX_SESSIONS {
        let path = f.dir.path().join(format!("{i}.silico"));
        std::fs::write(&path, BOX).unwrap();
        assert!(answer(f.sessions.open(path, false)).result.is_ok(), "{i}");
    }
    std::fs::write(f.path(), BOX).unwrap();
    let many = f.open(&example("open/request"));
    assert_eq!(many.session, None);
    check("open/refused-too-many", the_refusal(&many));
}

#[test]
fn opens_result_is_written_from_each_of_its_own_values() {
    let opened = Opened {
        session: SessionId::parse("s1").unwrap(),
        path: "new.silico".to_owned(),
        created: true,
        features: None,
        bodies: vec![id("a0000001"), id("b0000001")],
        problems: Problems::from(Error::new(ErrorCode::InvalidInput, "p")),
    };
    let text = written(&opened);
    holds(&schema_of("open", "result"), &text, "by hand");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        (
            &value["path"],
            &value["created"],
            &value["features"],
            &value["bodies"],
            &value["problems"][0]["message"],
            &value["problems_total"]
        ),
        (
            &serde_json::json!("new.silico"),
            &serde_json::json!(true),
            &serde_json::Value::Null,
            &serde_json::json!(["a0000001", "b0000001"]),
            &serde_json::json!("p"),
            &serde_json::json!(1)
        )
    );
}

#[test]
fn close_answers_and_refuses_as_the_design_gives_it() {
    let f = opened(BOX);
    answer(f.sessions.begin(&s1())).result.unwrap();
    let request = SessionRequest::read(&parsed(&example("close/request"))).unwrap();
    let refused = answer(f.sessions.close(&request.session));
    let text = the_refusal(&refused);
    assert_eq!(text, example("close/refused-transaction"));
    holds(&error_schema(), &text, "close refused");
    answer(f.sessions.rollback(&s1())).result.unwrap();
    let closed = answer(f.sessions.close(&request.session));
    let text = written(closed.result.as_ref().unwrap());
    assert_eq!(text, example("close/result"));
    assert_eq!(text, "{}\n");
    holds(&schema_of("close", "result"), &text, "closed");
    fails(
        &schema_of("close", "result"),
        r#"{"closed": true}"#,
        "a field close's result hasn't",
    );
}

#[test]
fn status_is_written_as_the_design_gives_it_and_from_each_of_its_own_values() {
    // The design's: a check running, a published read, a transaction begun at 4 with three
    // edits to undo (M5.0's section 6).
    let status = Status {
        features: Some(7),
        gated: true,
        transaction: Some(Transaction {
            base: revision(4),
            changed: true,
            undo: 3,
            redo: 0,
            dropped: 0,
            bytes: 61530,
        }),
        running: Some(RunningCommand::Command(SessionCommand::Check)),
        reads: vec![ReadCommand::Distance],
        rebuild: Rebuild {
            revision: revision(6),
            state: RebuildState::Running,
            progress: Some(Progress {
                done: 3,
                steps: 9,
                running: Some("f0000001 fillet".to_owned()),
            }),
        },
        disk: Some(Disk::Unchanged),
    };
    // By hand: the design's result, its rebuild 3 steps of 9 in; the step running, which
    // the design's `status` doesn't give, isn't written.
    let text = written(&status);
    assert_eq!(
        text,
        "{\n  \"features\": 7,\n  \"gated\": true,\n  \"transaction\": {\"base\": 4, \"changed\": true, \"undo\": 3, \"redo\": 0, \"dropped\": 0, \"bytes\": 61530},\n  \"running\": \"check\",\n  \"reads\": [\"distance\"],\n  \"rebuild\": {\"revision\": 6, \"state\": \"running\", \"done\": 3, \"steps\": 9},\n  \"disk\": \"unchanged\"\n}\n"
    );
    // Its read, `distance`, a read of the published model (M4.5's): it holds.
    holds(&schema_of("status", "result"), &text, "the design's");
    let designed = &design("### `status` (M4.2)", "### `cancel` (M4.2)")[1];
    assert_eq!(designed.clone() + "\n", text, "the design's");
    assert_eq!(text, example("status/result"));
    // Progress is a count: a fraction, or a negative, isn't one.
    for (from, to) in [
        ("\"done\": 3", "\"done\": 2.5"),
        ("\"steps\": 9", "\"steps\": -1"),
    ] {
        fails(&schema_of("status", "result"), &text.replace(from, to), to);
    }
    // A session's own, on the fake kernel: a box, nothing running, nothing rebuilt, and no
    // comparison of its file ended before this first status, which starts one.
    let f = opened(BOX);
    let request = SessionRequest::read(&parsed(&example("status/request"))).unwrap();
    let text = written(f.sessions.status(&request.session).result.as_ref().unwrap());
    assert_eq!(
        text,
        "{\n  \"features\": 1,\n  \"gated\": true,\n  \"transaction\": null,\n  \"running\": null,\n  \"reads\": [],\n  \"rebuild\": {\"revision\": 1, \"state\": \"none\", \"done\": null, \"steps\": null},\n  \"disk\": null\n}\n"
    );
    holds(&schema_of("status", "result"), &text, "a box's");
    // A document the reader refuses: no features, not gated.
    let other = Status {
        features: None,
        gated: false,
        transaction: None,
        running: None,
        reads: vec![ReadCommand::Elements, ReadCommand::Query],
        rebuild: Rebuild {
            revision: revision(9),
            state: RebuildState::Idle,
            progress: None,
        },
        disk: None,
    };
    assert_eq!(
        one_line(&other),
        r#"{"features": null, "gated": false, "transaction": null, "running": null, "reads": ["elements", "query"], "rebuild": {"revision": 9, "state": "none", "done": null, "steps": null}, "disk": null}"#
    );
    holds(&schema_of("status", "result"), &written(&other), "unread");
    // The session's own reload (M5.0's A3), and each finding on disk, each the word the
    // design gives it (section 6), and each one the schema holds.
    for (disk, word) in [
        (Disk::Unchanged, "unchanged"),
        (Disk::Changed, "changed"),
        (Disk::Gone, "gone"),
        (Disk::Unreadable, "unreadable"),
    ] {
        let reloading = Status {
            running: Some(RunningCommand::Reload),
            disk: Some(disk),
            ..other.clone()
        };
        let line = one_line(&reloading);
        assert!(
            line.contains(r#""running": "reload""#)
                && line.ends_with(&format!(r#""disk": "{word}"}}"#)),
            "{line}"
        );
        holds(&schema_of("status", "result"), &written(&reloading), word);
    }
    // A word that's neither a command nor `reload`, or a finding not among the four, isn't.
    let reloading = written(&Status {
        running: Some(RunningCommand::Reload),
        disk: Some(Disk::Changed),
        ..other.clone()
    });
    fails(
        &schema_of("status", "result"),
        &reloading.replace("\"reload\"", "\"reloading\""),
        "reloading",
    );
    fails(
        &schema_of("status", "result"),
        &reloading.replace("\"changed\"", "\"stale\""),
        "stale",
    );
    // Every state, as the schema lists them, each the session's own word.
    let states = [
        RebuildState::Idle,
        RebuildState::Running,
        RebuildState::Done,
        RebuildState::Faulted,
        RebuildState::Stopped,
    ];
    let result = schema_of("status", "result");
    let words: Vec<&str> = result["$defs"]["RebuildState"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|state| state["const"].as_str().unwrap())
        .collect();
    assert_eq!(words, ["none", "running", "done", "faulted", "stopped"]);
    for (state, word) in states.into_iter().zip(&words) {
        let status = Status {
            rebuild: Rebuild {
                revision: revision(2),
                state,
                progress: None,
            },
            ..other.clone()
        };
        let value: serde_json::Value = serde_json::from_str(&written(&status)).unwrap();
        assert_eq!(value["rebuild"]["state"], *word);
        holds(&schema_of("status", "result"), &written(&status), word);
    }
    fails(
        &schema_of("status", "result"),
        &text.replace("\"none\"", "\"waiting\""),
        "a state status hasn't",
    );
}

#[test]
fn cancel_answers_and_refuses_as_the_design_gives_it() {
    let error = error_schema();
    // A check waits for its rebuild, its box held: cancelled by the design's request.
    let f = opened(BOX);
    let (_hold, check) = f.running_held(|| {
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false })
    });
    let request = CancelRequest::read(&parsed(&example("cancel/request"))).unwrap();
    let cancelled = f.sessions.cancel(&request.session, request.command);
    let text = written(cancelled.result.as_ref().unwrap());
    assert_eq!(text, example("cancel/result"));
    holds(&schema_of("cancel", "result"), &text, "cancelled");
    // The check's own answer, having left no trace.
    let stopped = answer(check);
    let text = the_refusal(&stopped);
    assert_eq!(text, example("cancel/cancelled"));
    holds(&error, &text, "the check's answer");

    // An edit, outside a transaction, waits for its commit's rebuild: a cancel of check is
    // refused, naming what status reports running, the edit and the rebuild (decision 34 as
    // amended). The design's example is that refusal with the edit alone running, before
    // its rebuild starts, which only the crate's own tests can hold a command at; they give
    // it byte for byte (`session::tests::cancel_rebuild`).
    let f = opened(BOX);
    let (_hold, edit) = f.running_held(|| {
        f.sessions.set_field(
            &s1(),
            Revision::OPENED,
            id("k7f2qx3m"),
            "length".to_owned(),
            parsed("41"),
        )
    });
    let refused = f.sessions.cancel(&request.session, request.command);
    let text = the_refusal(&refused);
    let designed = example("cancel/refused-not-running");
    let both = designed
        .replace("set_field is", "set_field, rebuild are")
        .replace(r#"["set_field"]"#, r#"["set_field", "rebuild"]"#);
    assert_ne!(both, designed);
    assert_eq!(text, both);
    holds(&error, &text, "not running");
    holds(&error, &designed, "the design's");
    assert!(edit.cancel());
    // A result by hand: not stopped, another command.
    let late = Cancelled {
        command: SessionCommand::SetField.into(),
        stopped: false,
    };
    assert_eq!(
        one_line(&late),
        r#"{"command": "set_field", "stopped": false}"#
    );
    holds(&schema_of("cancel", "result"), &written(&late), "late");
}

#[test]
fn text_outline_and_check_answer_as_the_design_gives_them() {
    // The whole document's text: the file's bytes.
    let f = opened(EMPTY);
    let request = TextRequest::read(&parsed(&example("text/request"))).unwrap();
    let text = answer(f.sessions.text(&request.session, request.feature))
        .result
        .unwrap()
        .text;
    let written_text = written(&TextAnswer { text });
    assert_eq!(written_text, example("text/result"));
    holds(&schema_of("text", "result"), &written_text, "text");

    // The design's block's outline.
    let f = opened(&block());
    let request = SessionRequest::read(&parsed(&example("outline/request"))).unwrap();
    let outline = answer(f.sessions.outline(&request.session))
        .result
        .unwrap()
        .outline;
    let text = written(&OutlineAnswer { outline });
    assert_eq!(text, example("outline/result"));
    holds(&schema_of("outline", "result"), &text, "outline");

    // A box with nothing wrong, saved with its fingerprint: no problems, no warnings.
    let f = opened(&saved_box());
    let request = CheckRequest::read(&parsed(&example("check/request"))).unwrap();
    assert_eq!(request.model, CheckModel::Current { fresh: false });
    let checked = answer(
        f.sessions
            .check(&request.session, CheckModel::Current { fresh: false }),
    );
    assert_eq!(checked.model.as_ref().unwrap().of, ModelChoice::Current);
    let text = written(&checked.result.unwrap());
    assert_eq!(text, example("check/result"));
    holds(&schema_of("check", "result"), &text, "check");
    // The same result, in its envelope, is the design's model read.
    assert_eq!(
        text,
        "{\"problems\": [], \"problems_total\": 0, \"warnings\": [], \"warnings_total\": 0}\n"
    );
    // A fillet whose edge an edit removed, a real kernel's (silico-kernel's wire_examples.rs
    // writes it on kernel workers), holds to the result's snapshot too.
    holds(
        &schema_of("check", "result"),
        &example("check/reference-broken"),
        "reference broken",
    );
}

#[test]
fn a_text_and_an_outline_are_written_as_they_are_escaped_never_cut() {
    let text = TextAnswer {
        text: "a\n\"b\"\t\u{1b}".to_owned(),
    };
    assert_eq!(one_line(&text), r#"{"text": "a\n\"b\"\t\u001b"}"#);
    let outline = OutlineAnswer {
        outline: "x\n".to_owned(),
    };
    assert_eq!(one_line(&outline), r#"{"outline": "x\n"}"#);
}

#[test]
fn every_snapshot_of_these_commands_has_the_names_an_agent_reads() {
    let defs = |root: &serde_json::Value| -> Vec<String> {
        root["$defs"]
            .as_object()
            .map(|defs| defs.keys().cloned().collect())
            .unwrap_or_default()
    };
    for (of, request, request_defs, result, result_defs) in [
        (
            "version",
            "VersionRequest",
            &[][..],
            "VersionResult",
            &["Length", "Limits"][..],
        ),
        (
            "open",
            "OpenRequest",
            &[],
            "OpenResult",
            &["Error", "FeatureId"],
        ),
        ("close", "CloseRequest", &[], "CloseResult", &[]),
        (
            "status",
            "StatusRequest",
            &[],
            "StatusResult",
            &[
                "Disk",
                "PublishedRead",
                "Rebuild",
                "RebuildState",
                "RunningCommand",
                "Transaction",
            ],
        ),
        (
            "cancel",
            "CancelRequest",
            &["SessionCommand"],
            "CancelResult",
            &["SessionCommand"],
        ),
        ("text", "TextRequest", &["FeatureId"], "TextResult", &[]),
        ("outline", "OutlineRequest", &[], "OutlineResult", &[]),
        (
            "check",
            "CheckRequest",
            &["ModelChoice"],
            "CheckResult",
            &["Error", "Warning"],
        ),
    ] {
        let value = snapshot_value(of).unwrap();
        assert_eq!(value["request"]["title"], request, "{of}");
        assert_eq!(defs(&value["request"]), request_defs, "{of}");
        assert_eq!(value["result"]["title"], result, "{of}");
        assert_eq!(defs(&value["result"]), result_defs, "{of}");
    }
    // An object with no fields is closed to every key, as one with fields is to others.
    for (of, part) in [("version", "request"), ("close", "result")] {
        let schema = schema_of(of, part);
        assert_eq!(schema["type"], "object", "{of}");
        assert_eq!(schema["additionalProperties"], false, "{of}");
        assert!(schema.get("properties").is_none(), "{of}");
        holds(&schema, "{}", of);
        fails(&schema, r#"{"x": 1}"#, of);
    }
    // `check`'s fresh either boolean (M4.3: true with current; the reader refuses it with
    // published); `status`'s running a command that runs in a session, by name, or null,
    // and its reads each one; `cancel`'s command, asked and answered, one.
    let check = schema_of("check", "request");
    assert_eq!(
        check["properties"]["fresh"]["const"],
        serde_json::Value::Null
    );
    assert_eq!(check["properties"]["fresh"]["type"], "boolean");
    for fresh in ["true", "false"] {
        holds(
            &check,
            &format!(r#"{{"session": "s1", "model": "current", "fresh": {fresh}}}"#),
            fresh,
        );
    }
    fails(
        &check,
        r#"{"session": "s1", "model": "current", "fresh": "true"}"#,
        "fresh as a string",
    );
    let command = serde_json::json!({"$ref": "#/$defs/SessionCommand"});
    let status = schema_of("status", "result");
    // What runs on the session's thread: a command, or its own reload (M5.0's A3), a type of
    // its own, so `cancel`'s `command` never takes `reload`.
    assert_eq!(
        status["properties"]["running"]["anyOf"],
        serde_json::json!([{"$ref": "#/$defs/RunningCommand"}, {"type": "null"}])
    );
    assert_eq!(
        status["properties"]["reads"]["items"],
        serde_json::json!({"$ref": "#/$defs/PublishedRead"})
    );
    assert_eq!(
        status["$defs"]["PublishedRead"]["enum"],
        serde_json::json!(PUBLISHED_READS)
    );
    let running: Vec<&str> = SESSION_COMMANDS.iter().copied().chain(["reload"]).collect();
    assert_eq!(
        status["$defs"]["RunningCommand"]["enum"],
        serde_json::json!(running)
    );
    // `cancel` names what status reports running by its key: each command, and `rebuild`
    // (decision 34 as amended), never `reload`.
    let cancellable: Vec<&str> = SESSION_COMMANDS
        .iter()
        .copied()
        .chain(["rebuild"])
        .collect();
    assert_eq!(CANCELLABLE.to_vec(), cancellable);
    for part in ["request", "result"] {
        let cancel = schema_of("cancel", part);
        assert_eq!(cancel["properties"]["command"]["$ref"], command["$ref"]);
        assert_eq!(
            cancel["$defs"]["SessionCommand"]["enum"],
            serde_json::json!(cancellable)
        );
    }
    let base = |running: &str, reads: &str| {
        format!(
            r#"{{"features": 1, "gated": true, "transaction": null, "running": {running}, "reads": [{reads}], "rebuild": {{"revision": 1, "state": "none", "done": null, "steps": null}}, "disk": null}}"#
        )
    };
    holds(&status, &base(r#""check""#, ""), "check running");
    holds(
        &status,
        &base(r#""reload""#, ""),
        "the session's own reload running",
    );
    holds(&status, &base("null", ""), "nothing running");
    for never in [r#""status""#, r#""version""#, r#""probe""#, "1"] {
        fails(&status, &base(never, ""), never);
    }
    // Each published read a read; a command that never reads the published model, none.
    for read in PUBLISHED_READS {
        holds(&status, &base("null", &format!("\"{read}\"")), read);
    }
    for never in [r#""commit""#, r#""text""#, r#""outline""#, r#""close""#] {
        fails(&status, &base("null", never), never);
    }
}

/// [`BOX`] as `format` saves it, with its fingerprint: a document saved through Silico, which
/// a check finds nothing to warn of (one without would warn, `fingerprint_missing`).
fn saved_box() -> String {
    let f = opened(BOX);
    let formatted = answer(f.sessions.format(&s1(), None, true)).result.unwrap();
    assert_eq!(formatted.refingerprinted, [id("k7f2qx3m")]);
    std::fs::read_to_string(f.path()).unwrap()
}

#[test]
fn a_published_check_answers_at_once_from_the_older_revision_while_the_current_rebuilds() {
    let f = opened(&saved_box());
    // Revision 1's model, built and published.
    assert!(
        answer(
            f.sessions
                .check(&s1(), CheckModel::Current { fresh: false })
        )
        .result
        .unwrap()
        .problems
        .is_empty()
    );
    // Revision 2, an edit in a transaction, its rebuild held by a current check.
    answer(f.sessions.begin(&s1())).result.unwrap();
    let edit = answer(f.sessions.set_field(
        &s1(),
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        parsed("41"),
    ));
    assert_eq!(edit.session.unwrap().revision, revision(2));
    let (hold, current) = f.running_held(|| {
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false })
    });
    // The design's request, reading the published model: answered while the rebuild is held,
    // from revision 1's model.
    let text = example("check/request").replace("current", "published");
    let request = CheckRequest::read(&parsed(&text)).unwrap();
    assert_eq!(request.model, CheckModel::Published);
    let published = answer(f.sessions.check(&request.session, request.model));
    let model = published.model.as_ref().unwrap();
    assert_eq!(
        (model.revision, model.of, &model.fault),
        (Revision::OPENED, ModelChoice::Published, &None)
    );
    assert_eq!(published.session.as_ref().unwrap().revision, revision(2));
    let text = written(&published.result.unwrap());
    assert_eq!(text, example("check/result"));
    // The current check still waits on its rebuild, then answers of revision 2.
    let Err(current) = current.try_take() else {
        panic!("the current check answered with its rebuild held");
    };
    hold.release();
    let current = answer(current);
    let model = current.model.as_ref().unwrap();
    assert_eq!(
        (model.revision, model.of),
        (revision(2), ModelChoice::Current)
    );
    assert!(current.result.unwrap().problems.is_empty());
    // The current choice is the current check's.
    let again = answer(
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false }),
    );
    assert_eq!(again.model.unwrap().of, ModelChoice::Current);
}

#[test]
fn a_published_check_of_a_document_the_gate_refused_follows_the_published_rule() {
    // Nothing published: refused with the gate's problems, where the current check gives them
    // as its result.
    let zero = BOX.replace("\"length\": 40", "\"length\": 0");
    assert_ne!(zero, BOX);
    let f = opened(&zero);
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    assert_eq!(published.model, None);
    let refused = published.result.unwrap_err();
    let current = answer(
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false }),
    );
    assert_eq!(current.model, None);
    let problems = current.result.unwrap().problems;
    assert_eq!(refused, problems);
    assert_eq!(
        problems
            .errors()
            .iter()
            .map(|e| (e.code(), e.parameter()))
            .collect::<Vec<_>>(),
        [(ErrorCode::InvalidInput, Some("length"))]
    );
    // Something published, then the file changed to a document the gate refuses, reloaded at
    // revision 2: the published check answers from revision 1's model.
    let f = opened(BOX);
    assert!(
        answer(
            f.sessions
                .check(&s1(), CheckModel::Current { fresh: false })
        )
        .result
        .unwrap()
        .problems
        .is_empty()
    );
    std::fs::write(f.path(), &zero).unwrap();
    assert!(
        answer(f.sessions.text(&s1(), None))
            .session
            .unwrap()
            .reloaded
    );
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    let model = published.model.as_ref().unwrap();
    assert_eq!(
        (model.revision, model.of),
        (Revision::OPENED, ModelChoice::Published)
    );
    assert_eq!(published.session.as_ref().unwrap().revision, revision(2));
    assert!(published.result.unwrap().problems.is_empty());
    let current = answer(
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false }),
    );
    assert_eq!(current.result.unwrap().problems.total(), 1);
}

#[test]
fn a_published_check_leaves_out_the_drift_its_own_revisions_edits_reached() {
    // A box whose stored fingerprint is the fake kernel's, by formula: no drift as it is.
    let stored = BOX.replace(
        r#""height": 10}"#,
        r#""height": 10, "fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}"#,
    );
    assert_ne!(stored, BOX);
    let f = opened(&stored);
    let length = |at: u64, value: &str| {
        let set = answer(f.sessions.set_field(
            &s1(),
            revision(at),
            id("k7f2qx3m"),
            "length".to_owned(),
            parsed(value),
        ));
        assert_eq!(set.session.unwrap().revision, revision(at + 1));
    };
    // Revision 2: the box made longer in a transaction, which reaches it, so its stored
    // fingerprint, the shorter box's, isn't drift; its model published.
    answer(f.sessions.begin(&s1())).result.unwrap();
    length(1, "41");
    let checked = answer(
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false }),
    );
    assert_eq!(checked.model.unwrap().revision, revision(2));
    assert!(checked.result.unwrap().problems.is_empty());
    // Revision 3: the edit undone, so nothing is reached, its rebuild held by a current check.
    length(2, "40");
    let (hold, current) = f.running_held(|| {
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false })
    });
    // The published check reads revision 2's model, as revision 2's edits reached it: no
    // drift, though nothing is reached now.
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    let model = published.model.as_ref().unwrap();
    assert_eq!(
        (model.revision, model.of),
        (revision(2), ModelChoice::Published)
    );
    assert_eq!(published.session.as_ref().unwrap().revision, revision(3));
    let problems = published.result.unwrap().problems;
    assert!(problems.is_empty(), "{:?}", problems.errors());
    // The current check, released, is of revision 3: the box as stored, no drift.
    hold.release();
    let current = answer(current);
    assert_eq!(current.model.as_ref().unwrap().revision, revision(3));
    let problems = current.result.unwrap().problems;
    assert!(problems.is_empty(), "{:?}", problems.errors());
    // And revision 3's model, published, checks as its own: still nothing reached, no drift.
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    assert_eq!(published.model.unwrap().revision, revision(3));
    assert!(published.result.unwrap().problems.is_empty());
}

/// [`BOX`] with the fingerprint of a box 41 long stored (by formula: 41 × 20 × 10 = 8200,
/// area 2 × (820 + 410 + 200) = 2860): drift of the box 40 long, no edit reaching it.
fn stale() -> String {
    let stale = BOX.replace(
        r#""height": 10}"#,
        r#""height": 10, "fingerprint": {"volume": 8200, "area": 2860, "bbox": [0, 0, 0, 41, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}"#,
    );
    assert_ne!(stale, BOX);
    stale
}

/// Each problem's code and feature, in order.
fn codes(problems: &Problems) -> Vec<(ErrorCode, Option<&str>)> {
    problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature()))
        .collect()
}

/// The box's `field` set to `value` at `at`, which moves s1 to `at + 1`.
fn set(f: &Fixture, at: u64, field: &str, value: &str) {
    let set = answer(f.sessions.set_field(
        &s1(),
        revision(at),
        id("k7f2qx3m"),
        field.to_owned(),
        parsed(value),
    ));
    assert_eq!(set.session.unwrap().revision, revision(at + 1));
}

#[test]
fn a_published_check_says_the_drift_no_edit_reached() {
    // The box as stored drifts, and no edit reaches it: a current check says so, and
    // publishes its model; a published check of that model says the same drift.
    let f = opened(&stale());
    let current = answer(
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false }),
    );
    let current = current.result.unwrap();
    assert_eq!(
        codes(&current.problems),
        [(ErrorCode::Drift, Some("k7f2qx3m"))]
    );
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    let model = published.model.as_ref().unwrap();
    assert_eq!(
        (model.revision, model.of),
        (Revision::OPENED, ModelChoice::Published)
    );
    assert_eq!(published.result.unwrap(), current);
}

#[test]
fn after_a_rollback_a_published_check_says_the_drift_the_transaction_reached() {
    // The box's drift said at revision 1, whose model is published; in a transaction, its
    // width 21, reached, its drift not said at revision 2. Rolled back, revision 3 is
    // revision 1's state, revision 1's model published as its own: nothing reached, so a
    // published check of it says the drift again.
    let f = opened(&stale());
    let first = answer(
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false }),
    );
    assert_eq!(
        codes(&first.result.unwrap().problems),
        [(ErrorCode::Drift, Some("k7f2qx3m"))]
    );
    answer(f.sessions.begin(&s1())).result.unwrap();
    set(&f, 1, "width", "21");
    let reached = answer(
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false }),
    );
    assert_eq!(reached.model.unwrap().revision, revision(2));
    let problems = reached.result.unwrap().problems;
    assert!(problems.is_empty(), "{:?}", problems.errors());
    let rolled = answer(f.sessions.rollback(&s1()));
    assert_eq!(rolled.session.unwrap().revision, revision(3));
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    let model = published.model.as_ref().unwrap();
    assert_eq!(
        (model.revision, model.of),
        (revision(3), ModelChoice::Published)
    );
    assert_eq!(
        codes(&published.result.unwrap().problems),
        [(ErrorCode::Drift, Some("k7f2qx3m"))]
    );
}

#[test]
fn a_rebuild_a_published_check_starts_keeps_its_revisions_reach() {
    // A box whose stored fingerprint is the fake kernel's, made longer in a transaction:
    // revision 2, the box reached, its rebuild never started. A published check finds
    // nothing published and starts revision 2's rebuild; its model, published, is checked
    // as revision 2's edits reached it: the stored fingerprint, the shorter box's, not drift.
    let stored = BOX.replace(
        r#""height": 10}"#,
        r#""height": 10, "fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}"#,
    );
    let f = opened(&stored);
    answer(f.sessions.begin(&s1())).result.unwrap();
    set(&f, 1, "length", "41");
    let deadline = std::time::Instant::now() + HANG;
    let published = loop {
        let published = answer(f.sessions.check(&s1(), CheckModel::Published));
        if published.result.is_ok() {
            break published;
        }
        assert_eq!(
            said(&published.result.unwrap_err()),
            [(
                "model".to_owned(),
                "session s1 has published no model yet".to_owned()
            )]
        );
        assert!(std::time::Instant::now() < deadline, "nothing published");
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    let model = published.model.as_ref().unwrap();
    assert_eq!(
        (model.revision, model.of),
        (revision(2), ModelChoice::Published)
    );
    let problems = published.result.unwrap().problems;
    assert!(problems.is_empty(), "{:?}", problems.errors());
}

#[test]
fn a_published_check_lists_problems_beside_a_kernel_fault() {
    // A worker that crashes on every request: the current check's problems none, the fault
    // in its model read; a published check of that model the same, never refused for it.
    let f = opened(BOX);
    f.kernel
        .refuse(|_| Some(Error::new(ErrorCode::WorkerCrashed, "crashed")));
    let current = answer(
        f.sessions
            .check(&s1(), CheckModel::Current { fresh: false }),
    );
    let fault = current.model.as_ref().unwrap().fault.as_ref().unwrap();
    assert_eq!(fault.code(), ErrorCode::WorkerCrashed);
    assert!(current.result.unwrap().problems.is_empty());
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    let model = published.model.as_ref().unwrap();
    assert_eq!(
        (model.revision, model.of),
        (Revision::OPENED, ModelChoice::Published)
    );
    assert_eq!(
        model.fault.as_ref().map(Error::code),
        Some(ErrorCode::WorkerCrashed)
    );
    let checked = published.result.unwrap();
    assert!(
        checked.problems.is_empty(),
        "{:?}",
        checked.problems.errors()
    );
    assert!(checked.warnings.is_empty());
}
