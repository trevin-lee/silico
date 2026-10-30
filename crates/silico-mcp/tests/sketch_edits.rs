//! The `sketch_add` and `sketch_remove` tools (docs/design/m5.0-app.md, section 7, *The
//! sketch edits*): each listed, callable, and answering with the core's one-line envelope
//! byte for byte, its edits and its refusals alike. Expected values: the same calls on the
//! core's `Sessions::call` directly, in a workspace of their own; and the members added and
//! removed, by hand from the design's rules. Inside a transaction, so neither the server
//! nor the core solves (neither edit asks the solver anything, and no rebuild starts).

use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};

use crate::client::{Client, HANG, Reply};

/// A sketch of a line from the origin, and a box: what the edits edit.
const LINE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "b7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 3, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}]}"#;

/// The calls, each `(name, arguments)`: a line added from the line's end, with a
/// coincidence-free horizontal; the first line removed, its start with it (no other curve
/// uses it) and its end kept (the new line's); and refusals: the origin, a point a line
/// uses, a box named as the sketch, a member that isn't an ID, and a stale revision.
const CALLS: [(&str, &str); 9] = [
    ("open", r#"{"path": "part.silico", "create": false}"#),
    ("begin", r#"{"session": "s1"}"#),
    (
        "sketch_add",
        r#"{"session": "s1", "revision": 1, "sketch": "k7f2qx3m", "points": [{"id": "p0000003", "x": 20, "y": 3, "construction": false}], "lines": [{"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "c0000001", "type": "horizontal", "line": "n0000002"}]}"#,
    ),
    (
        "sketch_remove",
        r#"{"session": "s1", "revision": 2, "sketch": "k7f2qx3m", "members": ["n0000001"]}"#,
    ),
    (
        "sketch_remove",
        r#"{"session": "s1", "revision": 3, "sketch": "k7f2qx3m", "members": ["origin"]}"#,
    ),
    (
        "sketch_remove",
        r#"{"session": "s1", "revision": 3, "sketch": "k7f2qx3m", "members": ["p0000002"]}"#,
    ),
    (
        "sketch_add",
        r#"{"session": "s1", "revision": 3, "sketch": "b7f2qx3m", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}"#,
    ),
    (
        "sketch_remove",
        r#"{"session": "s1", "revision": 3, "sketch": "k7f2qx3m", "members": [7]}"#,
    ),
    (
        "sketch_add",
        r#"{"session": "s1", "revision": 1, "sketch": "k7f2qx3m", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}"#,
    ),
];

/// The answer's `structuredContent` as written, checked to be `content`'s one text block's
/// text, byte for byte; and `isError`.
fn structured(line: &str) -> (String, bool) {
    let reply: Reply<'_> = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
    let answer = reply.result;
    assert_eq!(answer.content.len(), 1, "{line}");
    assert_eq!(answer.content[0].kind, "text");
    assert_eq!(
        answer.content[0].text.as_deref(),
        Some(answer.structured.get())
    );
    (answer.structured.get().to_owned(), answer.is_error)
}

#[test]
fn each_sketch_edit_is_a_tool_whose_answer_is_the_cores_envelope_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), LINE).unwrap();
    let secret = dir.path().join("secret");
    let sessions = Sessions::with_cache_opener(Arc::new(FakeKernel::new(2).unwrap()), move |c| {
        Cache::with_secret(c, &secret)
    })
    .rendering(FakeRenderer::new())
    .in_workspace(&work)
    .unwrap();
    let core: Vec<(String, bool)> = CALLS
        .iter()
        .map(|(name, arguments)| match sessions.call(name, arguments) {
            Call::Running(calling) => {
                let answered = calling.wait_timeout(HANG).ok().unwrap();
                (answered.text, !answered.ok)
            }
            Call::Unknown => panic!("{name}"),
        })
        .collect();
    let mut c = Client::initialized();
    c.file("part.silico", LINE);
    for (at, ((name, arguments), core)) in CALLS.iter().zip(&core).enumerate() {
        let id = at as u64 + 1;
        c.send_call(id, name, arguments);
        let (line, _) = c.reply_line(&id.to_string());
        let (text, is_error) = structured(&line);
        assert_eq!((&text, is_error), (&core.0, core.1), "{name} {arguments}");
    }
    // What each said, by hand.
    let envelope = |n: usize| serde_json::from_str::<Value>(&core[n].0).unwrap();
    assert_eq!(
        envelope(2)["result"],
        json!({"sketch": "k7f2qx3m", "added": ["p0000003", "n0000002", "c0000001"], "modified": [], "removed": [], "changed": true, "commit": null})
    );
    assert_eq!(
        envelope(3)["result"],
        json!({"sketch": "k7f2qx3m", "added": [], "modified": [], "removed": ["p0000001", "n0000001"], "changed": true, "commit": null})
    );
    let first = |n: usize| {
        let e = envelope(n);
        assert_eq!(e["ok"], false, "{e}");
        (
            e["errors"][0]["code"].as_str().unwrap().to_owned(),
            e["errors"][0]["parameter"].as_str().unwrap().to_owned(),
            e["errors"][0]["message"].as_str().unwrap().to_owned(),
        )
    };
    let refused = |code: &str, parameter: &str, message: &str| {
        (code.to_owned(), parameter.to_owned(), message.to_owned())
    };
    assert_eq!(
        first(4),
        refused(
            "invalid_input",
            "members.0",
            "origin is one of the sketch's fixed references, which can't be removed"
        )
    );
    assert_eq!(
        first(5),
        refused(
            "invalid_input",
            "members.0",
            "point p0000002 is used by line n0000002"
        )
    );
    assert_eq!(
        first(6),
        refused("invalid_input", "sketch", "b7f2qx3m is a box, not a sketch")
    );
    assert_eq!(
        first(7),
        refused(
            "invalid_input",
            "members.0",
            "members.0 must be a sketch member's ID, got 7"
        )
    );
    assert_eq!(
        first(8),
        refused(
            "conflict",
            "revision",
            "the edit was made against revision 1, and the session is at revision 3"
        )
    );
    // Nothing reached the file: the transaction holds the edits.
    assert_eq!(c.read_file("part.silico"), LINE);
}

#[test]
fn the_sketch_edit_tools_are_listed_as_edits() {
    let mut c = Client::initialized();
    let listed = c.request(1, "tools/list", "{}");
    let tools = listed["result"]["tools"].as_array().unwrap();
    for name in ["sketch_add", "sketch_remove"] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        assert_eq!(
            (
                &tool["annotations"]["readOnlyHint"],
                &tool["annotations"]["destructiveHint"]
            ),
            (&json!(false), &json!(true)),
            "{name}"
        );
        assert_eq!(
            tool["inputSchema"]["required"],
            match name {
                "sketch_add" => json!([
                    "arcs",
                    "circles",
                    "constraints",
                    "lines",
                    "points",
                    "revision",
                    "session",
                    "sketch"
                ]),
                _ => json!(["members", "revision", "session", "sketch"]),
            },
            "{name}"
        );
    }
}
