//! The `drag_begin`, `drag_move`, and `drag_end` tools (docs/design/m5.0-app.md, section 7,
//! M5.3): each listed, callable, and answering with the core's one-line envelope byte for
//! byte, its runs and its refusals alike, each answer holding to its tool's `outputSchema`.
//! The drags run on PlaneGCS itself, the sketch worker, in the server and in the core alike
//! (each waited for before the next move, so every frame is solved, none dropped). Expected
//! values: the same calls on the core's `Sessions::call` directly, in a workspace of their
//! own; and the frames by hand: the design's profile with its base leg's thickness free
//! (`ka00000a` removed), so dragging `pa000003` to (62.5, 7) keeps it on its vertical line at
//! x = 60 and takes the leg's top, `pa000003` and `pa000004`, to y = 7; and, once released
//! there, that top's line pressed at its midpoint (32.5, 7) and dragged to (32.5, 10) moves
//! whole to y = 10.

use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};

use crate::client::{Client, HANG, Reply};

/// The bracket's L profile with its base leg's thickness, `ka00000a`, removed: one freedom.
const PROFILE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 60, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}, {"id": "pa000005", "x": 5, "y": 50, "construction": false}, {"id": "pa000006", "x": 0, "y": 50, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "horizontal", "line": "na000005"}, {"id": "ka000006", "type": "vertical", "line": "na000006"}, {"id": "ka000007", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000008", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, {"id": "ka00000b", "type": "length", "line": "na000005", "value": 5, "driving": true}, {"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}]}]}"#;

const POINT: &str =
    r#"{"session": "s1", "sketch": "s0000001", "point": "pa000003", "curve": null, "press": null}"#;

/// The calls, each `(name, arguments)`.
const CALLS: [(&str, &str); 13] = [
    ("open", r#"{"path": "part.silico", "create": false}"#),
    ("begin", r#"{"session": "s1"}"#),
    ("drag_begin", POINT),
    // A second begin: one drag a session.
    ("drag_begin", POINT),
    ("drag_move", r#"{"session": "s1", "to": [62.5, 7]}"#),
    ("drag_end", r#"{"session": "s1", "to": [62.5, 7]}"#),
    // Ended: a move refused.
    ("drag_move", r#"{"session": "s1", "to": [62.5, 7]}"#),
    (
        "drag_begin",
        r#"{"session": "s1", "sketch": "s0000001", "point": null, "curve": "na000003", "press": [32.5, 7]}"#,
    ),
    ("drag_move", r#"{"session": "s1", "to": [32.5, 10]}"#),
    // A cursor no number reaches, at the drag's own check.
    ("drag_move", r#"{"session": "s1", "to": [1e999, 0]}"#),
    ("drag_end", r#"{"session": "s1", "to": null}"#),
    ("drag_end", r#"{"session": "s1", "to": null}"#),
    // Both a point and a curve.
    (
        "drag_begin",
        r#"{"session": "s1", "sketch": "s0000001", "point": "pa000003", "curve": "na000003", "press": [32.5, 7]}"#,
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

/// Within a nanometre of `want`: PlaneGCS's own solve, not this test's arithmetic.
fn near(got: &Value, want: f64, what: &str) {
    let got = got.as_f64().unwrap_or_else(|| panic!("{what}: {got}"));
    assert!((got - want).abs() < 1e-6, "{what}: {got}, not {want}");
}

/// The frame's point `id`, `[x, y]`.
fn point(frame: &Value, id: &str) -> (Value, Value) {
    let p = frame["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("{id}: {frame}"));
    (p["x"].clone(), p["y"].clone())
}

#[test]
fn each_drag_command_is_a_tool_whose_answer_is_the_cores_envelope_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), PROFILE).unwrap();
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
    let listed = c.request(0, "tools/list", "{}");
    let tools = listed["result"]["tools"].as_array().unwrap().clone();
    c.file("part.silico", PROFILE);
    for (at, ((name, arguments), core)) in CALLS.iter().zip(&core).enumerate() {
        let id = at as u64 + 1;
        c.send_call(id, name, arguments);
        let (line, _) = c.reply_line(&id.to_string());
        let (text, is_error) = structured(&line);
        assert_eq!((&text, is_error), (&core.0, core.1), "{name} {arguments}");
        // And it holds to its tool's output schema.
        let tool = tools.iter().find(|t| t["name"] == *name).unwrap();
        let validator = jsonschema::validator_for(&tool["outputSchema"]).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&value)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "{name} {arguments}: {errors:?}");
    }
    // What each said, by hand.
    let envelope = |n: usize| serde_json::from_str::<Value>(&core[n].0).unwrap();
    assert_eq!(
        envelope(2)["result"],
        json!({"sketch": "s0000001", "point": "pa000003", "curve": null, "press": null, "dofs": 1, "moves": ["pa000001", "pa000002", "pa000003", "pa000004", "pa000005", "pa000006"]})
    );
    let code = |n: usize| {
        let e = envelope(n);
        assert_eq!(e["ok"], false, "{e}");
        (
            e["errors"][0]["code"].as_str().unwrap().to_owned(),
            e["errors"][0]["parameter"]
                .as_str()
                .unwrap_or("")
                .to_owned(),
            e["errors"][0]["message"].as_str().unwrap().to_owned(),
        )
    };
    assert_eq!(
        code(3),
        (
            "invalid_input".to_owned(),
            "session".to_owned(),
            "a drag is already open in session s1, of pa000003 in s0000001".to_owned()
        )
    );
    // The design's frame, on PlaneGCS: pa000003 kept at x = 60, the leg's top at y = 7.
    let frame = envelope(4)["result"].clone();
    assert_eq!(frame["frame"], "solved", "{frame}");
    assert_eq!(frame["to"], json!([62.5, 7]));
    for (id, x, y) in [
        ("pa000001", 0.0, 0.0),
        ("pa000002", 60.0, 0.0),
        ("pa000003", 60.0, 7.0),
        ("pa000004", 5.0, 7.0),
        ("pa000005", 5.0, 50.0),
        ("pa000006", 0.0, 50.0),
    ] {
        let (gx, gy) = point(&frame, id);
        near(&gx, x, id);
        near(&gy, y, id);
    }
    assert_eq!(frame["radii"], json!([]));
    assert_eq!(
        envelope(5)["result"],
        json!({"sketch": "s0000001", "kept": "cursor", "changed": true, "commit": null})
    );
    assert_eq!(envelope(5)["session"]["revision"], 2);
    assert_eq!(
        code(6),
        (
            "invalid_input".to_owned(),
            "session".to_owned(),
            "no drag is open in session s1".to_owned()
        )
    );
    // The top's line pressed at its midpoint, now at y = 7, and dragged up 3: it moves whole.
    assert_eq!(envelope(7)["result"]["curve"], "na000003");
    let frame = envelope(8)["result"].clone();
    assert_eq!(frame["frame"], "solved", "{frame}");
    for (id, x, y) in [("pa000003", 60.0, 10.0), ("pa000004", 5.0, 10.0)] {
        let (gx, gy) = point(&frame, id);
        near(&gx, x, id);
        near(&gy, y, id);
    }
    assert_eq!(code(9).1, "to");
    assert_eq!(
        envelope(10)["result"],
        json!({"sketch": "s0000001", "kept": "begin", "changed": false, "commit": null})
    );
    assert_eq!(envelope(10)["session"]["revision"], 2);
    assert_eq!(code(11).2, "no drag is open in session s1");
    assert_eq!(code(12).1, "curve");
    // Nothing reached the file: the transaction holds the release.
    assert_eq!(c.read_file("part.silico"), PROFILE);
}

#[test]
fn the_drag_tools_are_listed_with_what_they_change() {
    let mut c = Client::initialized();
    let listed = c.request(1, "tools/list", "{}");
    let tools = listed["result"]["tools"].as_array().unwrap();
    for (name, destructive, required) in [
        (
            "drag_begin",
            false,
            json!(["curve", "point", "press", "session", "sketch"]),
        ),
        ("drag_move", false, json!(["session", "to"])),
        ("drag_end", true, json!(["session", "to"])),
    ] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        assert_eq!(
            (
                &tool["annotations"]["readOnlyHint"],
                &tool["annotations"]["destructiveHint"]
            ),
            (&json!(false), &json!(destructive)),
            "{name}"
        );
        // One object at the root, as agents' clients take an input schema.
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        assert!(tool["inputSchema"].get("anyOf").is_none(), "{name}");
        assert_eq!(tool["inputSchema"]["required"], required, "{name}");
    }
}
