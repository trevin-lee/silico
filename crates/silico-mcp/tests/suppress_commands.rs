//! M5.1's `suppress_feature` as an MCP tool (docs/design/m5.0-app.md, section 7): called by
//! its tool and answered with the core's one-line envelope byte for byte, each answer
//! holding to its tool's `outputSchema` in the committed `tools.json`, and each refusal a
//! result with `isError`, as the core words it. Expected values: the same calls on the
//! core's `Sessions::call`, in a workspace of their own; and the lists by hand, from the
//! design's rules.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};

use crate::client::{Client, HANG, Reply};
use crate::contract::holds;

/// A block, a chamfer of its Edge3, a fillet of its Edge1 on the chamfer, and a lug nothing
/// uses.
const PARTS: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "size": {"distance": 1}}, {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "c0000001", "edges": ["k7f2qx3m.box.Edge1"], "radius": 1}, {"id": "b0000002", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 5, "width": 5, "height": 5}]}"#;

/// The calls, each `(name, arguments)`.
const CALLS: [(&str, &str); 10] = [
    ("open", r#"{"path": "part.silico", "create": false}"#),
    ("begin", r#"{"session": "s1"}"#),
    // The chamfer alone: the fillet names the block's edge, and builds on the block
    // through it.
    (
        "suppress_feature",
        r#"{"session": "s1", "revision": 1, "feature": "c0000001", "suppressed": true}"#,
    ),
    // The block: the fillet names its edge, so it goes too.
    (
        "suppress_feature",
        r#"{"session": "s1", "revision": 2, "feature": "k7f2qx3m", "suppressed": true}"#,
    ),
    ("timeline", r#"{"session": "s1", "model": "current"}"#),
    // The fillet back: the block, the maker of its name and its chain's root, with it; the
    // chamfer stays suppressed.
    (
        "suppress_feature",
        r#"{"session": "s1", "revision": 3, "feature": "f0000001", "suppressed": false}"#,
    ),
    ("timeline", r#"{"session": "s1", "model": "current"}"#),
    // Unsuppressed already: nothing changes.
    (
        "suppress_feature",
        r#"{"session": "s1", "revision": 4, "feature": "f0000001", "suppressed": false}"#,
    ),
    // A feature the document hasn't; and a stale revision.
    (
        "suppress_feature",
        r#"{"session": "s1", "revision": 4, "feature": "z0000001", "suppressed": true}"#,
    ),
    (
        "suppress_feature",
        r#"{"session": "s1", "revision": 1, "feature": "b0000002", "suppressed": true}"#,
    ),
];

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
fn suppress_feature_answers_as_the_core_does_and_holds_to_its_tool() {
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
    let ran: Vec<bool> = answers.iter().map(|a| a["ok"] == true).collect();
    assert_eq!(
        ran,
        [true, true, true, true, true, true, true, true, false, false]
    );
    assert_eq!(
        answers[2]["result"],
        json!({"feature": "c0000001", "suppressed": ["c0000001"], "unsuppressed": [], "changed": true, "commit": null})
    );
    assert_eq!(
        answers[3]["result"],
        json!({"feature": "k7f2qx3m", "suppressed": ["k7f2qx3m", "f0000001"], "unsuppressed": [], "changed": true, "commit": null})
    );
    // Everything but the lug suppressed: the lug the one body.
    let marks = |at: usize| -> Vec<(String, bool, String)> {
        answers[at]["result"]["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["id"].as_str().unwrap().to_owned(),
                    e["suppressed"].as_bool().unwrap(),
                    e["health"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let entry =
        |id: &str, suppressed: bool, health: &str| (id.to_owned(), suppressed, health.to_owned());
    assert_eq!(
        marks(4),
        [
            entry("k7f2qx3m", true, "suppressed"),
            entry("c0000001", true, "suppressed"),
            entry("f0000001", true, "suppressed"),
            // Saved with no fingerprint, and no edit reached it: warned of.
            entry("b0000002", false, "warning"),
        ]
    );
    assert_eq!(answers[4]["result"]["bodies"], json!(["b0000002"]));
    assert_eq!(
        answers[5]["result"],
        json!({"feature": "f0000001", "suppressed": [], "unsuppressed": ["k7f2qx3m", "f0000001"], "changed": true, "commit": null})
    );
    // The fillet on the block through the chamfer: two bodies, the fillet and the lug. Net
    // of the edits, the block is as `begin` saw it and uses nothing that changed, so it's
    // unreached and its missing fingerprint warned of; the fillet uses the chamfer, which
    // is suppressed now, so it's reached (A10).
    assert_eq!(
        marks(6),
        [
            entry("k7f2qx3m", false, "warning"),
            entry("c0000001", true, "suppressed"),
            entry("f0000001", false, "healthy"),
            entry("b0000002", false, "warning"),
        ]
    );
    assert_eq!(
        answers[6]["result"]["bodies"],
        json!(["f0000001", "b0000002"])
    );
    assert_eq!(
        answers[7]["result"],
        json!({"feature": "f0000001", "suppressed": [], "unsuppressed": [], "changed": false, "commit": null})
    );
    assert_eq!(answers[7]["session"]["revision"], 4);
    let refusal = |at: usize| answers[at]["errors"][0].clone();
    assert_eq!(
        (
            refusal(8)["parameter"].clone(),
            refusal(8)["message"].clone()
        ),
        (
            json!("feature"),
            json!("the document has no feature z0000001")
        )
    );
    assert_eq!(refusal(9)["code"], "conflict");
}
