//! `Sessions::call` (docs/design/m4.9-mcp.md, *`Call`*): every command by its name, its
//! request's text read by Silico's own reader and its command's, its typed method run, and
//! its envelope written on one line, `render`'s PNG beside it. Expected values: the committed
//! snapshots (each command's request's required fields, which `{}` must be refused for, each
//! named), the typed methods' own responses on sessions of their own, written as
//! `response_text` writes them (call adds nothing and drops nothing), the reader's refusals
//! as written here by hand, and lengths in nanometres worked out by hand.

use crate::model::WAIT;
use std::path::PathBuf;

use silico_core::command::render::Render;
use silico_core::command::wire::{COMMANDS, Layout, response_text};
use silico_core::command::{
    Answered, Call, Calling, CheckModel, Command, ModelChoice, Response, Revision, SessionId,
    VERSION,
};
use silico_core::document::json::Node;
use silico_core::document::{Length, Operation};
use silico_core::kernel::{Op, Primitive};
use silico_core::views::NamedView;

use crate::calls::{BOX, Fixture, answered, envelope, fixture};
use crate::example::EXAMPLE;
use crate::sessions::{HANG, answer, id};

/// What `call` must answer for the typed method's `response`: its envelope on one line, and
/// whether it ran.
fn as_written<T: silico_core::command::wire::ToWire>(response: &Response<T>) -> Answered {
    Answered {
        ok: response.result.is_ok(),
        text: response_text(response, Layout::OneLine),
        png: None,
    }
}

fn value(text: &str) -> Node {
    silico_core::document::json::read(text.as_bytes()).unwrap()
}

fn s(n: &str) -> SessionId {
    SessionId::parse(n).unwrap()
}

/// The committed snapshot of `command`'s request.
fn request_schema(command: &str) -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract/commands")
        .join(format!("{command}.json"));
    let text = std::fs::read_to_string(&path).unwrap();
    serde_json::from_str::<serde_json::Value>(&text).unwrap()["request"].clone()
}

#[test]
fn the_commands_are_one_list_in_one_order() {
    let names: Vec<&str> = Command::ALL.iter().map(|c| c.as_str()).collect();
    let listed: Vec<&str> = COMMANDS.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, listed);
    // The design's section 7, in its order, written out by hand (read_logs comes with its
    // command), then M5.0's section 7's, in its order (M5.1's, then M5.10's).
    assert_eq!(
        names,
        [
            "version",
            "open",
            "close",
            "status",
            "cancel",
            "begin",
            "commit",
            "rollback",
            "format",
            "add_feature",
            "set_field",
            "remove_feature",
            "replace_text",
            "text",
            "outline",
            "check",
            "cache_clear",
            "elements",
            "query",
            "schema",
            "mass_properties",
            "distance",
            "angle",
            "render",
            "read_logs",
            "export",
            "move_feature",
            "timeline",
            "sketch",
            "sketch_add",
            "sketch_remove",
            "sketch_split",
            "sketch_trim",
            "sketch_unjoin",
            "sketch_merge",
            "undo",
            "redo",
        ]
    );
    for command in Command::ALL {
        assert_eq!(Command::named(command.as_str()), Some(command));
        assert_eq!(command.to_string(), command.as_str());
    }
}

#[test]
fn every_command_dispatches_by_its_name_to_its_own_request_reader() {
    // `{}` is every command's request with each field missing: refused for each field its
    // snapshot requires, each named, so the reader that refused it is the command's own.
    // `version`'s takes none, so it runs.
    let f = fixture();
    for (name, _) in COMMANDS {
        let answer = f.call(name, "{}");
        assert_eq!(answer.png, None, "{name}");
        if name == "version" {
            let version: Response<_> = Response {
                session: None,
                model: None,
                result: Ok(VERSION),
            };
            assert_eq!(answer, as_written(&version));
            continue;
        }
        assert!(!answer.ok, "{name}: {}", answer.text);
        let envelope = envelope(&answer);
        let mut named: Vec<String> = envelope["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                assert_eq!(e["code"], "invalid_input", "{name}");
                assert_eq!(
                    e["message"],
                    format!("{} is missing", e["parameter"].as_str().unwrap()),
                    "{name}"
                );
                e["parameter"].as_str().unwrap().to_owned()
            })
            .collect();
        named.sort();
        let required: Vec<String> = request_schema(name)["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(named, required, "{name}");
        assert_eq!(envelope["total"], required.len(), "{name}");
        assert_eq!(envelope["session"], serde_json::Value::Null, "{name}");
    }
}

#[test]
fn a_name_no_command_has_is_unknown() {
    let f = fixture();
    for name in [
        "",
        "Check",
        "check ",
        " check",
        "tools/call",
        "Export",
        "read_log",
        "Read_logs",
        "cache-clear",
        "cache clear",
        "versions",
        "check\0",
    ] {
        assert!(
            matches!(f.sessions.call(name, "{}"), Call::Unknown),
            "{name:?}"
        );
    }
}

#[test]
fn a_call_answers_as_its_typed_method_does_written_on_one_line() {
    // Two sessions of their own, each in a workspace holding the same `part.silico`: one
    // driven by name, one by the typed methods, command for command.
    let by_name = fixture();
    let typed = fixture();
    for f in [&by_name, &typed] {
        f.file("part.silico", EXAMPLE);
    }
    let t = &typed.sessions;
    let s1 = s("s1");
    let pairs: Vec<(&str, &str, Answered)> = vec![
        (
            "open",
            r#"{"path": "part.silico", "create": false}"#,
            as_written(&answer(t.open("part.silico", false))),
        ),
        ("status", r#"{"session": "s1"}"#, as_written(&t.status(&s1))),
        (
            "text",
            r#"{"session": "s1", "feature": null}"#,
            as_written(&answer(t.text(&s1, None))),
        ),
        (
            "text",
            r#"{"session": "s1", "feature": "a91cw0de"}"#,
            as_written(&answer(t.text(&s1, Some(id("a91cw0de"))))),
        ),
        (
            "outline",
            r#"{"session": "s1"}"#,
            as_written(&answer(t.outline(&s1))),
        ),
        (
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
            as_written(&answer(t.check(&s1, CheckModel::Current { fresh: false }))),
        ),
        (
            "mass_properties",
            r#"{"session": "s1", "model": "current", "feature": null}"#,
            as_written(&answer(t.mass_properties(&s1, ModelChoice::Current, None))),
        ),
        (
            "elements",
            r#"{"session": "s1", "model": "current", "feature": null, "after": null, "limit": 3}"#,
            as_written(&answer(t.elements(
                &s1,
                ModelChoice::Current,
                None,
                None,
                3,
            ))),
        ),
        (
            "begin",
            r#"{"session": "s1"}"#,
            as_written(&answer(t.begin(&s1))),
        ),
        (
            "set_field",
            r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
            as_written(&answer(t.set_field(
                &s1,
                Revision::OPENED,
                id("k7f2qx3m"),
                "length".to_owned(),
                value("41"),
            ))),
        ),
        (
            "commit",
            r#"{"session": "s1"}"#,
            as_written(&answer(t.commit(&s1))),
        ),
        (
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
            as_written(&answer(t.check(&s1, CheckModel::Current { fresh: false }))),
        ),
        (
            "schema",
            r#"{"of": "close"}"#,
            as_written(&Response {
                session: None,
                model: None,
                result: silico_core::command::schema::schema("close"),
            }),
        ),
        (
            "schema",
            r#"{"of": "nothing"}"#,
            as_written(&Response {
                session: None,
                model: None,
                result: silico_core::command::schema::schema("nothing"),
            }),
        ),
        (
            "close",
            r#"{"session": "s1"}"#,
            as_written(&answer(t.close(&s1))),
        ),
        (
            "close",
            r#"{"session": "s1"}"#,
            as_written(&answer(t.close(&s1))),
        ),
    ];
    for (name, arguments, expected) in pairs {
        // Each ran but the second close, of a session closed.
        assert!(
            expected.ok || name == "close" || arguments.contains("nothing"),
            "{name}"
        );
        assert_eq!(
            by_name.call(name, arguments),
            expected,
            "{name} {arguments}"
        );
    }
    // The edit was saved alike.
    assert_eq!(
        std::fs::read(by_name.work.join("part.silico")).unwrap(),
        std::fs::read(typed.work.join("part.silico")).unwrap()
    );
}

#[test]
fn the_measures_queries_and_the_rest_of_the_edits_answer_as_their_typed_methods_do() {
    use silico_core::command::SessionCommand;
    use silico_core::command::wire::elements::QueryRequest;
    let by_name = fixture();
    let typed = fixture();
    for f in [&by_name, &typed] {
        f.file("part.silico", BOX);
        answer(f.sessions.open("part.silico", false))
            .result
            .unwrap();
    }
    let t = &typed.sessions;
    let s1 = s("s1");
    // Two of the box's faces, by name, as `elements` gives them.
    let listed = answer(t.elements(&s1, ModelChoice::Current, None, None, 2))
        .result
        .unwrap();
    let [a, b] = [0, 1].map(|i| listed.entries[i].name.to_string());
    let pair = serde_json::to_string(&[&a, &b]).unwrap();
    let query = r#"{"session": "s1", "model": "current", "feature": null, "kind": "face", "type": null, "stable": null, "facing": null, "measures": [], "order": null, "limit": 4}"#;
    let added = r#"{"id": "q0000001", "type": "box", "name": "Second", "position": [50, 0, 0], "length": 1, "width": 1, "height": 1}"#;
    let pairs: Vec<(&str, String, Answered)> = vec![
        (
            "distance",
            format!(
                r#"{{"session": "s1", "model": "current", "elements": {pair}, "feature": null}}"#
            ),
            as_written(&answer(t.distance(
                &s1,
                ModelChoice::Current,
                [a.clone(), b.clone()],
                None,
            ))),
        ),
        (
            "angle",
            format!(
                r#"{{"session": "s1", "model": "current", "elements": {pair}, "feature": null}}"#
            ),
            as_written(&answer(t.angle(
                &s1,
                ModelChoice::Current,
                [a.clone(), b.clone()],
                None,
            ))),
        ),
        (
            "query",
            query.to_owned(),
            as_written(&answer(t.query(
                &s1,
                ModelChoice::Current,
                QueryRequest::read(&value(query)).unwrap().query,
            ))),
        ),
        (
            "cancel",
            r#"{"session": "s1", "command": "check"}"#.to_owned(),
            as_written(&t.cancel(&s1, SessionCommand::Check)),
        ),
        (
            "format",
            r#"{"session": "s1", "features": null, "write": false}"#.to_owned(),
            as_written(&answer(t.format(&s1, None, false))),
        ),
        (
            "begin",
            r#"{"session": "s1"}"#.to_owned(),
            as_written(&answer(t.begin(&s1))),
        ),
        (
            "add_feature",
            format!(r#"{{"session": "s1", "revision": 1, "before": null, "feature": {added}}}"#),
            as_written(&answer(t.add_feature(
                &s1,
                Revision::OPENED,
                None,
                value(added),
            ))),
        ),
        (
            "rollback",
            r#"{"session": "s1"}"#.to_owned(),
            as_written(&answer(t.rollback(&s1))),
        ),
        (
            "add_feature",
            format!(r#"{{"session": "s1", "revision": 3, "before": null, "feature": {added}}}"#),
            as_written(&answer(t.add_feature(
                &s1,
                Revision::OPENED.next().unwrap().next().unwrap(),
                None,
                value(added),
            ))),
        ),
        (
            "remove_feature",
            r#"{"session": "s1", "revision": 4, "feature": "q0000001"}"#.to_owned(),
            as_written(&answer(t.remove_feature(
                &s1,
                Revision::from_number(4).unwrap(),
                id("q0000001"),
            ))),
        ),
        (
            "replace_text",
            format!(
                r#"{{"session": "s1", "revision": 5, "text": {}}}"#,
                serde_json::to_string(BOX).unwrap()
            ),
            as_written(&answer(t.replace_text(
                &s1,
                Revision::from_number(5).unwrap(),
                BOX.to_owned(),
            ))),
        ),
    ];
    for (name, arguments, expected) in pairs {
        // Each ran but the cancel, which nothing was running for, and the angle: the fake
        // kernel's faces are on no plane, which an angle refuses as its own.
        assert_eq!(
            expected.ok,
            !matches!(name, "cancel" | "angle"),
            "{name}: {}",
            expected.text
        );
        assert_eq!(
            by_name.call(name, &arguments),
            expected,
            "{name} {arguments}"
        );
    }
    // Each command's own answer, not another's: the distance's result is a distance's, the
    // angle's an angle's.
    let distance = envelope(&by_name.call(
        "distance",
        &format!(r#"{{"session": "s1", "model": "current", "elements": {pair}, "feature": null}}"#),
    ));
    let angle = envelope(&by_name.call(
        "angle",
        &format!(r#"{{"session": "s1", "model": "current", "elements": {pair}, "feature": null}}"#),
    ));
    assert_ne!(distance, angle);
}

fn front(to: Option<&str>) -> Render {
    Render {
        view: Some(NamedView::Front),
        camera: None,
        width: 40,
        height: 30,
        highlight: vec![],
        to: to.map(PathBuf::from),
        replace: false,
    }
}

#[test]
fn a_renders_png_comes_beside_its_envelope_never_in_it() {
    let (by_name, typed) = (fixture(), fixture());
    for f in [&by_name, &typed] {
        f.file("part.silico", EXAMPLE);
        answer(f.sessions.open("part.silico", false))
            .result
            .unwrap();
    }
    let rendered = answer(
        typed
            .sessions
            .render(&s("s1"), ModelChoice::Current, front(None)),
    );
    let Response {
        session,
        model,
        result,
    } = rendered;
    let rendering = result.unwrap();
    let expected = response_text(
        &Response {
            session,
            model,
            result: Ok(rendering.result),
        },
        Layout::OneLine,
    );
    let answer = by_name.call(
        "render",
        r#"{"session": "s1", "model": "current", "view": "front", "camera": null, "width": 40, "height": 30, "highlight": [], "to": null, "replace": false}"#,
    );
    assert!(answer.ok);
    assert_eq!(answer.text, expected);
    // The fake renderer's PNG, `PNG 40x30`: its 9 bytes counted in the result, not in it.
    assert_eq!(answer.png.as_deref(), Some(&b"PNG 40x30"[..]));
    assert_eq!(rendering.png, b"PNG 40x30");
    assert_eq!(envelope(&answer)["result"]["png_bytes"], 9);
    assert!(!answer.text.contains("PNG 40x30"));
    // Refused (its `to` out of the workspace), it has none.
    let refused = by_name.call(
        "render",
        r#"{"session": "s1", "model": "current", "view": "front", "camera": null, "width": 40, "height": 30, "highlight": [], "to": "../view.png", "replace": false}"#,
    );
    assert!(!refused.ok);
    assert_eq!(refused.png, None);
    assert_eq!(envelope(&refused)["errors"][0]["parameter"], "to");
    // And with a `to`, written in the workspace, the PNG beside the envelope too.
    let written = by_name.call(
        "render",
        r#"{"session": "s1", "model": "current", "view": "front", "camera": null, "width": 40, "height": 30, "highlight": [], "to": "view.png", "replace": false}"#,
    );
    assert_eq!(written.png.as_deref(), Some(&b"PNG 40x30"[..]));
    assert_eq!(
        std::fs::read(by_name.work.join("view.png")).unwrap(),
        b"PNG 40x30"
    );
}

/// A refusal of a request no session was found for: `ok` false, `session` and `model`
/// null, and the one error, as the error contract writes it (written out by hand).
fn refusal(message: &str, parameter: Option<&str>, suggestion: Option<&str>) -> String {
    let quoted = |text: Option<&str>| text.map_or("null".to_owned(), |t| format!("\"{t}\""));
    let (parameter, suggestion) = (quoted(parameter), quoted(suggestion));
    format!(
        r#"{{"ok": false, "session": null, "model": null, "errors": [{{"code": "invalid_input", "message": "{message}", "feature": null, "elements": [], "elements_total": 0, "parameter": {parameter}, "cause": null, "suggestion": {suggestion}, "candidates": []}}], "total": 1}}"#
    )
}

const NOT_JSON: Option<&str> =
    Some("fix the request there: a request is strict JSON (RFC 8259), each key given once");

#[test]
fn a_request_the_reader_refuses_is_answered_with_the_cores_refused_envelope() {
    let f = fixture();
    for (name, arguments, expected) in [
        (
            "status",
            r#"{"session": }"#,
            refusal(
                "the request isn't valid JSON: expected a value, found `}`, at line 1, column 13",
                None,
                NOT_JSON,
            ),
        ),
        (
            "status",
            r#"{"session": "s1", "session": "s1"}"#,
            refusal(
                "the request isn't valid JSON: the key \\\"session\\\" is written twice in one object, at line 1, column 19",
                None,
                NOT_JSON,
            ),
        ),
        (
            "version",
            "",
            refusal(
                "the request isn't valid JSON: expected a value, found the end of the text, at line 1, column 1",
                None,
                NOT_JSON,
            ),
        ),
        (
            "version",
            "[]",
            refusal("the request must be an object, got an array", None, None),
        ),
        (
            "status",
            r#"{"session": "s1", "extra": 1}"#,
            refusal(
                "extra isn't a field of this request",
                Some("extra"),
                Some("leave it out: a request has only the fields its shape gives"),
            ),
        ),
    ] {
        let answer = f.call(name, arguments);
        assert!(!answer.ok, "{arguments}");
        assert_eq!(answer.text, expected, "{arguments}");
        assert_eq!(answer.png, None);
    }
}

#[test]
fn a_request_nested_past_the_limit_or_longer_than_it_is_refused_unread() {
    let f = fixture();
    // 64 deep is read (and refused for its shape); 65 is refused by the reader.
    let nested = |depth: usize| {
        format!(
            r#"{{"of": {}{}}}"#,
            "[".repeat(depth - 1),
            "]".repeat(depth - 1)
        )
    };
    let at_most = envelope(&f.call("schema", &nested(64)));
    assert_eq!(at_most["errors"][0]["parameter"], "of");
    let past = f.call("schema", &nested(65));
    assert_eq!(
        envelope(&past)["errors"][0]["message"],
        "the request isn't valid JSON: arrays and objects nest more than 64 deep, at line 1, column 71"
    );
    assert_eq!(
        envelope(&past)["errors"][0]["parameter"],
        serde_json::Value::Null
    );
    // One byte past what's read: refused for its size, its text never parsed.
    let most = silico_core::document::json::MAX_INPUT_LEN;
    let long = format!(r#"{{"of": "{}"}}"#, "x".repeat(most - 10 + 1));
    assert_eq!(long.len(), most + 1);
    let past = envelope(&f.call("schema", &long));
    assert_eq!(
        past["errors"][0]["message"],
        format!(
            "the request is too large for Silico to read: the text is {} bytes, more than the {most} Silico reads",
            most + 1
        )
    );
    assert_eq!(past["errors"][0]["parameter"], serde_json::Value::Null);
    assert_eq!(
        past["errors"][0]["suggestion"],
        format!("send less at once: a request is at most {most} bytes")
    );
}

#[test]
fn a_number_is_read_from_its_digits_never_rounded() {
    let f = fixture();
    f.file("part.silico", BOX);
    assert!(
        f.call("open", r#"{"path": "part.silico", "create": false}"#)
            .ok
    );
    // A length with more decimals than a nanometre's: refused for them, not rounded to 0.1.
    let refused = f.call(
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 0.10000000000000000001}"#,
    );
    assert!(!refused.ok);
    let error = &envelope(&refused)["errors"][0];
    assert_eq!(error["code"], "invalid_input");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("has more than 6 decimals"),
        "{error}"
    );
    // And a request's own length field alike (a camera's view height).
    let camera = f.call(
        "render",
        r#"{"session": "s1", "model": "current", "view": null, "camera": {"projection": "orthographic", "eye": [0, -100, 0], "target": [0, 0, 0], "up": [0, 0, 1], "view_height": 0.10000000000000000001, "field_of_view": null}, "width": 40, "height": 30, "highlight": [], "to": null, "replace": false}"#,
    );
    let error = &envelope(&camera)["errors"][0];
    assert_eq!(error["parameter"], "camera.view_height");
    assert_eq!(
        error["message"],
        "camera.view_height must be a length in millimetres or null, got 0.10000000000000000001, which has more than 6 decimals"
    );
    assert_eq!(camera.png, None);
    // `0.1` is 100,000 nm exactly, as the document holds it and the kernel is sent it.
    let set = f.call(
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 0.1}"#,
    );
    assert!(set.ok, "{}", set.text);
    assert_eq!(envelope(&set)["result"]["after"], 0.1);
    let published = f.sessions.published(&s("s1")).result.unwrap();
    let Operation::Box { length, .. } = &published.model.document().features[0].operation else {
        panic!("not a box");
    };
    assert_eq!(*length, Length::from_nm(100_000));
    let sent: Vec<f64> = f
        .kernel
        .calls()
        .iter()
        .filter_map(|call| match call.op {
            Op::MakePrimitive(Primitive::Box { dx, .. }) => Some(dx),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [0.1]);
}

#[test]
fn calls_to_one_session_run_in_the_order_sent_and_opens_take_ids_in_that_order() {
    let f = fixture();
    for name in ["a", "b", "c", "d", "e"] {
        f.file(&format!("{name}.silico"), BOX);
    }
    // Each queued as it's called; waited on last first, so no answer orders the next.
    let opens: Vec<Calling> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|name| {
            f.calling(
                "open",
                &format!(r#"{{"path": "{name}.silico", "create": false}}"#),
            )
        })
        .collect();
    let mut ids: Vec<(String, String)> = opens
        .into_iter()
        .rev()
        .map(|calling| {
            let envelope = envelope(&answered(calling));
            (
                envelope["result"]["path"].as_str().unwrap().to_owned(),
                envelope["session"]["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    ids.reverse();
    assert_eq!(
        ids,
        [
            ("a.silico", "s1"),
            ("b.silico", "s2"),
            ("c.silico", "s3"),
            ("d.silico", "s4"),
            ("e.silico", "s5"),
        ]
        .map(|(p, s)| (p.to_owned(), s.to_owned()))
    );
    // An edit then a check, called back to back, each time: the check reads the edit's
    // revision, and the text its length.
    for (at, length) in (1..=10).zip(41..) {
        let edit = f.calling(
            "set_field",
            &format!(
                r#"{{"session": "s1", "revision": {at}, "feature": "k7f2qx3m", "field": "length", "value": {length}}}"#
            ),
        );
        let check = f.calling(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        );
        let text = f.calling("text", r#"{"session": "s1", "feature": "k7f2qx3m"}"#);
        let (text, check, edit) = (answered(text), answered(check), answered(edit));
        assert!(edit.ok, "{}", edit.text);
        let check = envelope(&check);
        assert_eq!(check["session"]["revision"], at + 1);
        assert_eq!(check["model"]["revision"], at + 1);
        let text = envelope(&text);
        assert!(
            text["result"]["text"]
                .as_str()
                .unwrap()
                .contains(&format!("\"length\": {length},")),
            "{text}"
        );
    }
}

#[test]
fn status_and_cancel_are_answered_when_waited_on_not_when_called() {
    let f = fixture();
    f.file("part.silico", BOX);
    // Called before s1 is open, waited on after: it answers of s1 open, so it ran then.
    let status = f.calling("status", r#"{"session": "s1"}"#);
    assert!(status.progress().is_none());
    assert!(!status.stopper().cancel());
    answer(f.sessions.open("part.silico", false))
        .result
        .unwrap();
    let status = envelope(&answered(status));
    assert_eq!(status["ok"], true);
    assert_eq!(status["session"]["id"], "s1");
    // A cancel called before the check it names is queued, waited on while that check
    // waits on the kernel: it finds it running, and stops it.
    let cancel = f.calling("cancel", r#"{"session": "s1", "command": "check"}"#);
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let check = f.calling(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    assert!(f.kernel.wait_running(1, HANG));
    let cancel = envelope(&answered(cancel));
    assert_eq!(cancel["result"]["command"], "check");
    assert_eq!(cancel["result"]["stopped"], true);
    hold.release();
    let check = envelope(&answered(check));
    assert_eq!(check["errors"][0]["code"], "cancelled");
    // `version` and `schema` likewise: nothing to stop, and no progress.
    for (name, arguments) in [("version", "{}"), ("schema", r#"{"of": "document"}"#)] {
        let calling = f.calling(name, arguments);
        assert!(calling.progress().is_none());
        assert!(!calling.stopper().cancel());
        assert!(answered(calling).ok);
    }
}

#[test]
fn each_commands_calling_is_answered_when_waited_on_exactly_as_the_command_says() {
    // The five that take no place in a session's queue, written out by hand (the design's
    // *Running a call*): the rest run off the caller's thread, and are waited on.
    let later: Vec<Command> = Command::ALL
        .into_iter()
        .filter(|c| c.answered_when_waited())
        .collect();
    assert_eq!(
        later,
        [
            Command::Version,
            Command::Status,
            Command::Cancel,
            Command::Schema,
            Command::ReadLogs
        ]
    );
    // Each command's own example request (the contract's, which its reader takes), on a
    // session open as s1: the call's kind is the command's.
    let f = fixture();
    f.opened();
    for command in Command::ALL {
        let name = command.as_str();
        let example = match command {
            Command::Angle => "wedge.json",
            Command::Distance => "box-bottom-and-top.json",
            Command::MassProperties => "every-body.json",
            Command::Query => "largest-face-up.json",
            Command::Schema => "request-document.json",
            _ => "request.json",
        };
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/contract/commands/examples")
            .join(name)
            .join(example);
        let request = std::fs::read_to_string(&path).unwrap();
        let calling = f.calling(name, &request);
        assert_eq!(
            calling.waits_on_nothing(),
            command.answered_when_waited(),
            "{name}"
        );
        let answer = answered(calling);
        // Read by its reader, so it isn't a refusal of the request: it ran, or its typed
        // method refused it (a feature the box hasn't, say).
        let envelope = envelope(&answer);
        if !answer.ok {
            let code = envelope["errors"][0]["code"].as_str().unwrap();
            assert!(
                !envelope["errors"][0]["message"]
                    .as_str()
                    .unwrap()
                    .ends_with(" is missing"),
                "{name}: {}",
                answer.text
            );
            assert_ne!(code, "internal", "{name}: {}", answer.text);
        }
    }
    // A request refused before it ran is answered when it's waited on, whatever its
    // command; `version`'s `{}` is its whole request, and runs, as it's answered.
    for command in Command::ALL {
        let calling = f.calling(command.as_str(), "{}");
        assert!(calling.waits_on_nothing(), "{command}");
        let _ = answered(calling);
        let calling = f.calling(command.as_str(), "not json");
        assert!(calling.waits_on_nothing(), "{command}");
        assert!(!answered(calling).ok, "{command}");
    }
}

#[test]
fn a_command_called_as_one_answers_as_it_does_called_by_its_name() {
    let f = fixture();
    f.opened();
    for (command, arguments) in [
        (Command::Version, "{}"),
        (Command::Outline, r#"{"session": "s1"}"#),
        (Command::Check, "{}"),
        (Command::Text, r#"{"session": "s9", "feature": null}"#),
    ] {
        let by_name = answered(f.calling(command.as_str(), arguments));
        let typed = answered(f.sessions.call_command(command, arguments));
        assert_eq!(typed, by_name, "{command}");
    }
}

#[test]
fn a_refused_answer_is_the_cores_refused_envelope_on_one_line() {
    let problems = silico_core::command::Problems::from(silico_core::Error::new(
        silico_core::ErrorCode::InvalidInput,
        "no",
    ));
    let answer = Answered::refused(problems);
    assert!(!answer.ok);
    assert_eq!(answer.png, None);
    assert_eq!(answer.text, refusal("no", None, None));
}

#[test]
fn a_call_running_gives_its_progress_waits_again_and_is_stopped_by_its_stopper() {
    let f = fixture();
    f.file("part.silico", BOX);
    answer(f.sessions.open("part.silico", false))
        .result
        .unwrap();
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let mut check = f.calling(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    assert!(f.kernel.wait_running(1, HANG));
    // One step, the box's, running and not done.
    let deadline = std::time::Instant::now() + HANG;
    let progress = loop {
        if let Some(progress) = check.progress()
            && progress.steps == 1
        {
            break progress;
        }
        assert!(std::time::Instant::now() < deadline, "no progress");
        std::thread::yield_now();
    };
    assert_eq!((progress.done, progress.steps), (0, 1));
    assert_eq!(progress.running.as_deref(), Some("k7f2qx3m box"));
    // Not answered yet: the call comes back to be waited on again.
    check = match check.wait_timeout(std::time::Duration::from_millis(20)) {
        Err(check) => check,
        Ok(answered) => panic!("answered while held: {}", answered.text),
    };
    let stopper = check.stopper();
    assert!(stopper.cancel());
    hold.release();
    let answered = check.wait();
    assert!(!answered.ok);
    assert_eq!(envelope(&answered)["errors"][0]["code"], "cancelled");
    // A request refused before it ran has nothing to stop.
    let refused = f.calling("check", "{}");
    assert!(refused.progress().is_none());
    assert!(!refused.stopper().cancel());
}

#[test]
fn a_call_is_waited_on_by_another_thread_than_the_one_that_called() {
    // As MCP's server waits: the reader calls, a waiter waits.
    let f = fixture();
    f.file("part.silico", BOX);
    let open = f.calling("open", r#"{"path": "part.silico", "create": false}"#);
    let status = f.calling("status", r#"{"session": "s1"}"#);
    let (open, status) = std::thread::scope(|scope| {
        let open = scope.spawn(move || answered(open));
        let opened = open.join().unwrap();
        let status = scope.spawn(move || answered(status));
        (opened, status.join().unwrap())
    });
    assert!(open.ok && status.ok, "{} {}", open.text, status.text);
}

/// `a`, and every other answer in `others`, told apart: so a field `call` dropped (each of
/// `others` its request with that field at its default) can't answer as `a` does.
fn apart(a: &Answered, others: &[&Answered], what: &str) {
    for other in others {
        assert_ne!(
            a.text, other.text,
            "{what}: a field's default answers alike"
        );
    }
}

#[test]
fn every_field_of_a_request_reaches_its_typed_method() {
    // Found by the review: rows where each field was at its default passed with `call`
    // dropping it (`format(.., None, false)`, `check(.., Current)`, `elements(.., None, None,
    // ..)`, `mass_properties(.., None)`, `distance(.., None)`). Each field here differs from
    // its default, and the answer with it at its default is told apart from it.
    let s1 = s("s1");
    let (by_name, typed) = (fixture(), fixture());
    for f in [&by_name, &typed] {
        f.file("part.silico", EXAMPLE);
        answer(f.sessions.open("part.silico", false))
            .result
            .unwrap();
    }
    let t = &typed.sessions;
    let block = || Some(id("k7f2qx3m"));
    // `elements` of the block alone, after its first entry: the cut's bodies, and from the
    // start, list otherwise.
    let first = answer(t.elements(&s1, ModelChoice::Current, block(), None, 1))
        .result
        .unwrap();
    let cursor = envelope(&by_name.call(
        "elements",
        r#"{"session": "s1", "model": "current", "feature": "k7f2qx3m", "after": null, "limit": 1}"#,
    ))["result"]["next"]
        .clone();
    assert!(cursor.is_object(), "{cursor}");
    let elements = |feature: &str, after: &str| {
        by_name.call(
            "elements",
            &format!(
                r#"{{"session": "s1", "model": "current", "feature": {feature}, "after": {after}, "limit": 2}}"#
            ),
        )
    };
    let cursor = cursor.to_string();
    let listed = elements(r#""k7f2qx3m""#, &cursor);
    assert_eq!(
        listed,
        as_written(&answer(t.elements(
            &s1,
            ModelChoice::Current,
            block(),
            first.next,
            2
        )))
    );
    apart(
        &listed,
        &[
            &elements("null", &cursor),
            &elements(r#""k7f2qx3m""#, "null"),
        ],
        "elements",
    );
    // `mass_properties` of the block alone, not of the cut, the one body.
    let mass = |feature: &str| {
        by_name.call(
            "mass_properties",
            &format!(r#"{{"session": "s1", "model": "current", "feature": {feature}}}"#),
        )
    };
    let block_mass = mass(r#""k7f2qx3m""#);
    assert!(block_mass.ok, "{}", block_mass.text);
    assert_eq!(
        block_mass,
        as_written(&answer(t.mass_properties(
            &s1,
            ModelChoice::Current,
            block()
        )))
    );
    apart(&block_mass, &[&mass("null")], "mass_properties");
    // `distance` and `angle` between two of the block's faces, looked for in the pin's
    // element map, which hasn't them: the bodies' (the cut's) has.
    let faces = answer(t.elements(&s1, ModelChoice::Current, block(), None, 2))
        .result
        .unwrap();
    let [a, b] = [0, 1].map(|i| faces.entries[i].name.to_string());
    let pair = serde_json::to_string(&[&a, &b]).unwrap();
    for name in ["distance", "angle"] {
        let measured = |feature: &str| {
            by_name.call(
                name,
                &format!(
                    r#"{{"session": "s1", "model": "current", "elements": {pair}, "feature": {feature}}}"#
                ),
            )
        };
        let expected = if name == "distance" {
            as_written(&answer(t.distance(
                &s1,
                ModelChoice::Current,
                [a.clone(), b.clone()],
                Some(id("a91cw0de")),
            )))
        } else {
            as_written(&answer(t.angle(
                &s1,
                ModelChoice::Current,
                [a.clone(), b.clone()],
                Some(id("a91cw0de")),
            )))
        };
        let of_pin = measured(r#""a91cw0de""#);
        assert_eq!(of_pin, expected, "{name}");
        apart(&of_pin, &[&measured("null")], name);
    }
}

#[test]
fn a_read_of_the_published_model_reaches_its_typed_method_as_one() {
    // The published model at revision 1, the session at 2, its rebuild held: a read of the
    // published model answers of revision 1 at once; of the current one, it waits.
    let s1 = s("s1");
    let (by_name, typed) = (fixture(), fixture());
    let (mut holds, mut edits) = (Vec::new(), Vec::new());
    for f in [&by_name, &typed] {
        f.file("part.silico", BOX);
        answer(f.sessions.open("part.silico", false))
            .result
            .unwrap();
        answer(f.sessions.check(&s1, CheckModel::Current { fresh: false }))
            .result
            .unwrap();
        holds.push(
            f.kernel
                .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. }))),
        );
        // The edit waits on its rebuild, held: sent, not waited on.
        edits.push(f.calling(
            "set_field",
            r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
        ));
        assert!(f.kernel.wait_running(1, HANG));
    }
    let t = &typed.sessions;
    let promptly = |calling: Calling, what: &str| match calling.wait_timeout(WAIT) {
        Ok(answer) => answer,
        Err(_) => panic!("{what} waited on the held rebuild: read the current model"),
    };
    let check = promptly(
        by_name.calling(
            "check",
            r#"{"session": "s1", "model": "published", "fresh": false}"#,
        ),
        "check",
    );
    assert_eq!(
        check,
        as_written(&answer(t.check(&s1, CheckModel::Published)))
    );
    let elements = promptly(
        by_name.calling(
            "elements",
            r#"{"session": "s1", "model": "published", "feature": null, "after": null, "limit": 3}"#,
        ),
        "elements",
    );
    assert_eq!(
        elements,
        as_written(&answer(t.elements(
            &s1,
            ModelChoice::Published,
            None,
            None,
            3
        )))
    );
    // Of revision 1, the edit's rebuild held.
    for answer in [&check, &elements] {
        let envelope = envelope(answer);
        assert!(answer.ok, "{envelope}");
        assert_eq!(envelope["model"]["revision"], 1, "{envelope}");
        assert_eq!(envelope["model"]["of"], "published", "{envelope}");
    }
    for (hold, edit) in holds.into_iter().zip(edits) {
        hold.release();
        let edit = answered(edit);
        assert!(edit.ok, "{}", edit.text);
    }
}

#[test]
fn a_format_calls_features_and_write_reach_its_typed_method() {
    // Three workspaces holding the same document, saved with its fingerprints, then the
    // block and the pin edited by hand, so both drift: one formatted by name, accepting the
    // block's drift alone and writing; one by the typed method alike; one accepting all.
    let fixtures = [fixture(), fixture(), fixture()];
    let saved = |f: &Fixture| std::fs::read_to_string(f.work.join("part.silico")).unwrap();
    for f in &fixtures {
        f.file("part.silico", EXAMPLE);
        let s1 = answer(f.sessions.open("part.silico", false))
            .session
            .unwrap()
            .id;
        answer(f.sessions.format(&s1, None, true)).result.unwrap();
        answer(f.sessions.close(&s1)).result.unwrap();
    }
    let fingerprinted = saved(&fixtures[0]);
    assert!(fingerprinted.contains("fingerprint"), "{fingerprinted}");
    let drifted = fingerprinted
        .replacen("\"length\": 40,", "\"length\": 41,", 1)
        .replacen("\"radius\": 2.5,", "\"radius\": 3,", 1);
    assert_eq!(
        drifted.matches("41").count(),
        1 + fingerprinted.matches("41").count()
    );
    assert!(drifted.contains("\"radius\": 3,"));
    for f in &fixtures {
        f.file("part.silico", &drifted);
        answer(f.sessions.open("part.silico", false))
            .result
            .unwrap();
    }
    let s2 = s("s2");
    let [by_name, typed, all] = &fixtures;
    let formatted = by_name.call(
        "format",
        r#"{"session": "s2", "features": ["k7f2qx3m"], "write": true}"#,
    );
    assert_eq!(
        formatted,
        as_written(&answer(typed.sessions.format(
            &s2,
            Some(vec![id("k7f2qx3m")]),
            true
        )))
    );
    assert_eq!(saved(by_name), saved(typed));
    // Written: `write` reached it.
    assert_ne!(saved(by_name), drifted);
    // Only the block's drift accepted: `features` reached it.
    answer(all.sessions.format(&s2, None, true)).result.unwrap();
    assert_ne!(saved(by_name), saved(all));
}

#[test]
fn every_reads_model_reaches_its_typed_method_the_published_answering_past_a_held_rebuild() {
    // Found by the review (round 2): a read's `model` could be dropped to `current` and
    // every test passed. A read of the published model answers while the current one's
    // rebuild is held; one of the current model would wait on it.
    let f = fixture();
    f.file("part.silico", BOX);
    answer(f.sessions.open("part.silico", false))
        .result
        .unwrap();
    answer(
        f.sessions
            .check(&s("s1"), CheckModel::Current { fresh: false }),
    )
    .result
    .unwrap();
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let edit = f.calling(
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
    );
    assert!(f.kernel.wait_running(1, HANG));
    let pair = r#"["k7f2qx3m.Face1", "k7f2qx3m.Face2"]"#;
    for (name, args) in [
        ("mass_properties", r#"{"session": "s1", "model": "published", "feature": null}"#.to_owned()),
        ("distance", format!(r#"{{"session": "s1", "model": "published", "elements": {pair}, "feature": null}}"#)),
        ("angle", format!(r#"{{"session": "s1", "model": "published", "elements": {pair}, "feature": null}}"#)),
        ("query", r#"{"session": "s1", "model": "published", "feature": null, "kind": "face", "type": null, "stable": null, "facing": null, "measures": [], "order": null, "limit": 4}"#.to_owned()),
        ("render", r#"{"session": "s1", "model": "published", "view": "front", "camera": null, "width": 40, "height": 30, "highlight": [], "to": null, "replace": false}"#.to_owned()),
    ] {
        let answer = match f.calling(name, &args).wait_timeout(HANG) {
            Ok(a) => a,
            Err(_) => panic!("{name} waited on the held rebuild: read the current model"),
        };
        let e = envelope(&answer);
        assert_eq!(e["model"]["revision"], 1, "{name}: {e}");
        assert_eq!(e["model"]["of"], "published", "{name}: {e}");
    }
    hold.release();
    assert!(answered(edit).ok);
}

#[test]
fn cache_clear_and_a_fresh_check_answer_as_their_typed_methods_do() {
    // Two workspaces holding the same box, whose kernel crashes making it until told not to:
    // the open's check keeps the fault as its revision's outcome (decision 32). Then, by name
    // and by the typed methods alike: a plain check gives the kept fault, asking the kernel
    // nothing; a fresh one tries the box again and builds it, its model the fault's no more;
    // and cache_clear removes the one entry the fresh check kept (the box, the document's
    // one step), then none.
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let s1 = s("s1");
    let (by_name, typed) = (fixture(), fixture());
    let crashing = Arc::new(AtomicBool::new(true));
    for f in [&by_name, &typed] {
        let crashes = Arc::clone(&crashing);
        f.kernel.refuse(move |op| {
            (op.name() == "MakeBox" && crashes.load(Ordering::SeqCst))
                .then(|| silico_core::Error::new(silico_core::ErrorCode::WorkerCrashed, "crashed"))
        });
        f.file("part.silico", BOX);
        answer(f.sessions.open("part.silico", false))
            .result
            .unwrap();
        let faulted = answer(f.sessions.check(&s1, CheckModel::Current { fresh: false }));
        assert!(faulted.model.unwrap().fault.is_some());
    }
    crashing.store(false, Ordering::SeqCst);
    let t = &typed.sessions;
    let boxes = |f: &Fixture| {
        f.kernel
            .calls()
            .iter()
            .filter(|call| call.op.name() == "MakeBox")
            .count()
    };
    let before = boxes(&by_name);
    let kept = by_name.call(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    assert_eq!(
        kept,
        as_written(&answer(t.check(&s1, CheckModel::Current { fresh: false })))
    );
    assert!(
        envelope(&kept)["model"]["fault"].is_object(),
        "{}",
        kept.text
    );
    assert_eq!(boxes(&by_name), before);
    let fresh = by_name.call(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": true}"#,
    );
    assert_eq!(
        fresh,
        as_written(&answer(t.check(&s1, CheckModel::Current { fresh: true })))
    );
    // `fresh` reached it: it built the box again, and answers otherwise than the plain check.
    assert!(fresh.ok, "{}", fresh.text);
    assert_eq!(envelope(&fresh)["model"]["fault"], serde_json::Value::Null);
    assert_ne!(fresh, kept);
    assert_eq!(boxes(&by_name), before + 1);
    for removed in [1, 0] {
        let cleared = by_name.call("cache_clear", r#"{"session": "s1"}"#);
        assert_eq!(cleared, as_written(&answer(t.cache_clear(&s1))));
        assert!(cleared.ok, "{}", cleared.text);
        assert_eq!(
            envelope(&cleared)["result"],
            serde_json::json!({"directory": ".silico-cache", "removed": removed})
        );
    }
}

#[test]
fn the_timeline_commands_answer_by_name_as_their_typed_methods_do() {
    // A block, a fillet of its Edge1, and a lug nothing uses: the lug moved first, a chamfer
    // inserted under the fillet, removed again, and the timeline of each model, each by
    // name and by its typed method, on sessions of their own; each answer the same, byte
    // for byte, and each ran.
    let parts = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge1"], "radius": 1}, {"id": "b0000002", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 5, "width": 5, "height": 5}]}"#;
    let chamfer = r#"{"id": "c0000001", "type": "chamfer", "name": "Bevel", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "size": {"distance": 1}}"#;
    let by_name = fixture();
    let typed = fixture();
    for f in [&by_name, &typed] {
        f.file("part.silico", parts);
        answer(f.sessions.open("part.silico", false))
            .result
            .unwrap();
    }
    let t = &typed.sessions;
    let s1 = s("s1");
    let at = |n: u64| Revision::from_number(n).unwrap();
    let pairs: Vec<(&str, String, Answered)> = vec![
        (
            "begin",
            r#"{"session": "s1"}"#.to_owned(),
            as_written(&answer(t.begin(&s1))),
        ),
        (
            "move_feature",
            r#"{"session": "s1", "revision": 1, "feature": "b0000002", "before": "k7f2qx3m"}"#
                .to_owned(),
            as_written(&answer(t.move_feature(
                &s1,
                at(1),
                id("b0000002"),
                Some(id("k7f2qx3m")),
            ))),
        ),
        (
            "add_feature",
            format!(
                r#"{{"session": "s1", "revision": 2, "before": "f0000001", "feature": {chamfer}}}"#
            ),
            as_written(&answer(t.add_feature(
                &s1,
                at(2),
                Some(id("f0000001")),
                value(chamfer),
            ))),
        ),
        (
            "remove_feature",
            r#"{"session": "s1", "revision": 3, "feature": "c0000001"}"#.to_owned(),
            as_written(&answer(t.remove_feature(&s1, at(3), id("c0000001")))),
        ),
        (
            "timeline",
            r#"{"session": "s1", "model": "current"}"#.to_owned(),
            as_written(&answer(t.timeline(&s1, ModelChoice::Current))),
        ),
        (
            "timeline",
            r#"{"session": "s1", "model": "published"}"#.to_owned(),
            as_written(&answer(t.timeline(&s1, ModelChoice::Published))),
        ),
    ];
    for (name, arguments, expected) in pairs {
        assert!(expected.ok, "{name}: {}", expected.text);
        assert_eq!(
            by_name.call(name, &arguments),
            expected,
            "{name} {arguments}"
        );
    }
    // What each did, by hand: the lug moved from 2 to 0, rewiring nothing; the chamfer at
    // 2, the fillet rewired to it; its removal giving the fillet the block back.
    let moved = envelope(&by_name.call(
        "move_feature",
        r#"{"session": "s1", "revision": 4, "feature": "b0000002", "before": "k7f2qx3m"}"#,
    ));
    assert_eq!(
        moved["result"],
        serde_json::json!({"feature": "b0000002", "from": 0, "to": 0, "rewired": [], "changed": false, "commit": null})
    );
    let text = envelope(&by_name.call("text", r#"{"session": "s1", "feature": null}"#));
    let document =
        silico_core::document::read(text["result"]["text"].as_str().unwrap().as_bytes()).unwrap();
    let ids: Vec<&str> = document.features.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["b0000002", "k7f2qx3m", "f0000001"]);
}

#[test]
fn undo_and_redo_answer_by_name_as_their_typed_methods_do() {
    // The box, its length set twice in a transaction, then undone, redone, undone `to` the
    // revision `begin` came at, and a refusal of each, by name and by its typed method, on
    // sessions of their own; each answer the same, byte for byte, `status` among them.
    let by_name = fixture();
    let typed = fixture();
    for f in [&by_name, &typed] {
        f.file("part.silico", BOX);
        answer(f.sessions.open("part.silico", false))
            .result
            .unwrap();
    }
    let t = &typed.sessions;
    let s1 = s("s1");
    let at = |n: u64| Revision::from_number(n).unwrap();
    let length = |r: u64, value: &str| {
        (
            "set_field",
            format!(
                r#"{{"session": "s1", "revision": {r}, "feature": "k7f2qx3m", "field": "length", "value": {value}}}"#
            ),
        )
    };
    let step = |r: u64, to: &str| format!(r#"{{"session": "s1", "revision": {r}, "to": {to}}}"#);
    let set = |r: u64, value: &str| {
        as_written(&answer(t.set_field(
            &s1,
            at(r),
            id("k7f2qx3m"),
            "length".to_owned(),
            silico_core::document::json::read(value.as_bytes()).unwrap(),
        )))
    };
    let pairs: Vec<(&str, String, Answered)> = vec![
        (
            "begin",
            r#"{"session": "s1"}"#.to_owned(),
            as_written(&answer(t.begin(&s1))),
        ),
        {
            let (name, args) = length(1, "41");
            (name, args, set(1, "41"))
        },
        {
            let (name, args) = length(2, "42");
            (name, args, set(2, "42"))
        },
        (
            "undo",
            step(3, "null"),
            as_written(&answer(t.undo(&s1, at(3), None))),
        ),
        (
            "redo",
            step(4, "null"),
            as_written(&answer(t.redo(&s1, at(4), None))),
        ),
        (
            "undo",
            step(5, "1"),
            as_written(&answer(t.undo(&s1, at(5), Some(at(1))))),
        ),
        (
            "status",
            r#"{"session": "s1"}"#.to_owned(),
            as_written(&t.status(&s1)),
        ),
        // Refused: nothing left to undo, and a redo `to` the session's own revision.
        (
            "undo",
            step(6, "null"),
            as_written(&answer(t.undo(&s1, at(6), None))),
        ),
        (
            "redo",
            step(6, "6"),
            as_written(&answer(t.redo(&s1, at(6), Some(at(6))))),
        ),
    ];
    let ran: Vec<bool> = pairs.iter().map(|(_, _, answer)| answer.ok).collect();
    assert_eq!(
        ran,
        [true, true, true, true, true, true, true, false, false]
    );
    for (name, arguments, expected) in pairs {
        assert_eq!(
            by_name.call(name, &arguments),
            expected,
            "{name} {arguments}"
        );
    }
    // What each did, by hand: 42 undone (41's state, revision 2's), redone (3's), then both
    // undone to `begin`'s, at revision 6, with both to redo and the document unchanged.
    let undone = envelope(&by_name.call("undo", &step(6, "null")));
    assert_eq!(
        undone["errors"][0]["message"],
        "nothing to undo in session s1's transaction"
    );
    let status = envelope(&by_name.call("status", r#"{"session": "s1"}"#));
    assert_eq!(status["session"]["revision"], 6);
    let file = BOX.len();
    assert_eq!(
        status["result"]["transaction"],
        serde_json::json!({"base": 1, "changed": false, "undo": 0, "redo": 2, "dropped": 0,
            // `begin`'s file, and the two states held for redo, each its list (24 and 8) and
            // its chunk (16, 4 spaces of indent and the block's 122 characters).
            "bytes": file + 2 * (24 + 8 + 16 + 4 + 122)})
    );
}
