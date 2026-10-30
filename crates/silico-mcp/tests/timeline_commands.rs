//! M5.1's timeline commands as MCP tools (docs/design/m5.0-app.md, section 7): `move_feature`,
//! `add_feature` with `before`, `remove_feature` with `rewired`, and `timeline`, each called
//! by its tool and answered with the core's one-line envelope byte for byte, each answer
//! holding to its tool's `outputSchema` in the committed `tools.json`, and a refusal of each
//! a result with `isError`, as the core words it. Expected values: the same calls on the
//! core's `Sessions::call`, in a workspace of their own; and each chain's rewiring by hand.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};

use crate::client::{Client, HANG, Reply};
use crate::contract::holds;

/// A block, a fillet of its Edge1, and a lug nothing uses.
const PARTS: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge1"], "radius": 1}, {"id": "b0000002", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 5, "width": 5, "height": 5}]}"#;

/// The calls, each `(name, arguments)`: the lug moved first, a chamfer inserted under the
/// fillet, its removal, a timeline of each model, and a refusal of each new command.
const CALLS: [(&str, &str); 11] = [
    ("open", r#"{"path": "part.silico", "create": false}"#),
    ("begin", r#"{"session": "s1"}"#),
    (
        "move_feature",
        r#"{"session": "s1", "revision": 1, "feature": "b0000002", "before": "k7f2qx3m"}"#,
    ),
    (
        "add_feature",
        r#"{"session": "s1", "revision": 2, "before": "f0000001", "feature": {"id": "c0000001", "type": "chamfer", "name": "Bevel", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "size": {"distance": 1}}}"#,
    ),
    (
        "remove_feature",
        r#"{"session": "s1", "revision": 3, "feature": "c0000001"}"#,
    ),
    ("timeline", r#"{"session": "s1", "model": "current"}"#),
    ("timeline", r#"{"session": "s1", "model": "published"}"#),
    // The block, root of the fillet's chain, past it: refused, naming both.
    (
        "move_feature",
        r#"{"session": "s1", "revision": 4, "feature": "k7f2qx3m", "before": null}"#,
    ),
    // Before a feature the document hasn't.
    (
        "add_feature",
        r#"{"session": "s1", "revision": 4, "before": "z0000001", "feature": {"type": "box", "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#,
    ),
    // The block, whose elements the fillet names.
    (
        "remove_feature",
        r#"{"session": "s1", "revision": 4, "feature": "k7f2qx3m"}"#,
    ),
    ("timeline", r#"{"session": "s9", "model": "current"}"#),
];

/// The answer's `structuredContent` as written, checked to be `content`'s one text block's
/// text, byte for byte; and `isError`.
fn structured(line: &str) -> (String, bool) {
    let reply: Reply<'_> = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
    let answer = reply.result;
    assert_eq!(answer.content.len(), 1, "{line}");
    assert_eq!(
        answer.content[0].text.as_deref(),
        Some(answer.structured.get())
    );
    (answer.structured.get().to_owned(), answer.is_error)
}

/// The committed `tools.json`'s tools, by name.
fn tool(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/contract/tools.json");
    let tools: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no tool {name}"))
        .clone()
}

#[test]
fn each_timeline_command_answers_as_the_core_does_and_holds_to_its_tool() {
    // The core's own answers, on sessions of their own.
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), PARTS).unwrap();
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
    c.file("part.silico", PARTS);
    let mut answers = Vec::new();
    for (at, ((name, arguments), core)) in CALLS.iter().zip(&core).enumerate() {
        let id = at as u64 + 1;
        c.send_call(id, name, arguments);
        let (line, _) = c.reply_line(&id.to_string());
        let (text, is_error) = structured(&line);
        assert_eq!((&text, is_error), (&core.0, core.1), "{name} {arguments}");
        let envelope: Value = serde_json::from_str(&text).unwrap();
        holds(&tool(name)["outputSchema"], &envelope, name);
        holds(
            &tool(name)["inputSchema"],
            &serde_json::from_str(arguments).unwrap(),
            name,
        );
        answers.push(envelope);
    }
    // Ran, each but the refusals, by hand.
    let ran: Vec<bool> = answers.iter().map(|a| a["ok"] == true).collect();
    assert_eq!(
        ran,
        [
            true, true, true, true, true, true, true, false, false, false, false
        ]
    );
    // The lug moved from 2 to 0, past features it's no part of: nothing rewired.
    assert_eq!(
        answers[2]["result"],
        json!({"feature": "b0000002", "from": 2, "to": 0, "rewired": [], "changed": true, "commit": null})
    );
    // The chamfer at 2 (after the move: lug, block, chamfer, fillet), the fillet on it.
    assert_eq!(answers[3]["result"]["index"], 2);
    assert_eq!(
        answers[3]["result"]["rewired"],
        json!([{"feature": "f0000001", "field": "base", "from": "k7f2qx3m", "to": "c0000001"}])
    );
    // Removed, the fillet takes the block back.
    assert_eq!(
        answers[4]["result"]["rewired"],
        json!([{"feature": "f0000001", "field": "base", "from": "c0000001", "to": "k7f2qx3m"}])
    );
    // The timeline: the lug, the block, the fillet; the bodies the lug and the fillet.
    let timeline = &answers[5]["result"];
    let entries: Vec<(&str, &str, Value)> = timeline["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap(),
                e["type"].as_str().unwrap(),
                e["inputs"].clone(),
            )
        })
        .collect();
    assert_eq!(
        entries,
        [
            ("b0000002", "box", json!([])),
            ("k7f2qx3m", "box", json!([])),
            ("f0000001", "fillet", json!(["k7f2qx3m"])),
        ]
    );
    assert_eq!(timeline["revision"], 4);
    assert_eq!(timeline["model_revision"], 4);
    assert_eq!(timeline["bodies"], json!(["b0000002", "f0000001"]));
    // The published model is the current one just built.
    assert_eq!(answers[6]["result"], *timeline);
    // The refusals, each the core's words, at their fields.
    let refusal = |at: usize| answers[at]["errors"][0].clone();
    assert_eq!(
        (refusal(7)["message"].clone(), refusal(7)["feature"].clone()),
        (
            json!(
                "k7f2qx3m is the root of f0000001's chain, and f0000001, a fillet, can't be left without a base"
            ),
            json!("k7f2qx3m")
        )
    );
    assert_eq!(refusal(8)["parameter"], "before");
    assert_eq!(
        refusal(8)["message"],
        "the document has no feature z0000001 to add this one before"
    );
    assert_eq!(
        refusal(9)["message"],
        "f0000001 names k7f2qx3m's elements, so k7f2qx3m can't be removed"
    );
    assert_eq!(refusal(10)["message"], "no session s9 is open");
}
