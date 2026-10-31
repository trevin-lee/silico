//! M5.2's `undo` and `redo` as MCP tools (docs/design/m5.0-app.md, section 7), and
//! `status`'s transaction counts: each called by its tool and answered with the core's
//! one-line envelope byte for byte, each answer holding to its tool's `outputSchema` in the
//! committed `tools.json` and its arguments to its `inputSchema`, and a refusal of each a
//! result with `isError`, as the core words it; and their annotations, edits of what the
//! server holds that never write a file. Expected values: the same calls on the core's
//! `Sessions::call`, in a workspace of their own; and each step's counts by hand.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};

use crate::client::{BOX, Client, HANG, Reply};
use crate::contract::holds;

/// The calls, each `(name, arguments)`: the box's length set twice in a transaction, one
/// undo, one redo, an undo `to` the revision `begin` came at, the status then, and a
/// refusal of each command.
const CALLS: [(&str, &str); 11] = [
    ("open", r#"{"path": "part.silico", "create": false}"#),
    ("begin", r#"{"session": "s1"}"#),
    (
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
    ),
    (
        "set_field",
        r#"{"session": "s1", "revision": 2, "feature": "k7f2qx3m", "field": "length", "value": 42}"#,
    ),
    ("undo", r#"{"session": "s1", "revision": 3, "to": null}"#),
    ("redo", r#"{"session": "s1", "revision": 4, "to": null}"#),
    ("undo", r#"{"session": "s1", "revision": 5, "to": 1}"#),
    ("status", r#"{"session": "s1"}"#),
    // Nothing left to undo; a redo `to` a revision the stack never held.
    ("undo", r#"{"session": "s1", "revision": 6, "to": null}"#),
    ("redo", r#"{"session": "s1", "revision": 6, "to": 99}"#),
    ("undo", r#"{"session": "s9", "revision": 1, "to": null}"#),
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
fn undo_and_redo_answer_as_the_core_does_and_hold_to_their_tools() {
    // The core's own answers, on sessions of their own.
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), BOX).unwrap();
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
    c.file("part.silico", BOX);
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
            true, true, true, true, true, true, true, true, false, false, false
        ]
    );
    // 42 undone: 41's state (revision 2's), one to undo and one to redo, at revision 4.
    assert_eq!(
        answers[4]["result"],
        json!({"restored": 2, "undo": 1, "redo": 1, "dropped": 0})
    );
    assert_eq!(answers[4]["session"]["revision"], 4);
    // Redone: 42's (revision 3's), at 5.
    assert_eq!(
        answers[5]["result"],
        json!({"restored": 3, "undo": 2, "redo": 0, "dropped": 0})
    );
    // Both undone, `to` 1: `begin`'s, at 6.
    assert_eq!(
        answers[6]["result"],
        json!({"restored": 1, "undo": 0, "redo": 2, "dropped": 0})
    );
    // The status: back at `begin`'s document, unchanged; the stack holds the file's bytes and
    // the two states for redo, each its list (24 and 8) and its chunk (16, 4 spaces of
    // indent, and the block's 122 characters).
    assert_eq!(answers[7]["session"]["revision"], 6);
    assert_eq!(
        answers[7]["result"]["transaction"],
        json!({"base": 1, "changed": false, "undo": 0, "redo": 2, "dropped": 0,
            "bytes": BOX.len() + 2 * (24 + 8 + 16 + 4 + 122)})
    );
    // The refusals, each the core's words, at their fields.
    let refusal = |at: usize| answers[at]["errors"][0].clone();
    assert_eq!(
        (
            refusal(8)["message"].clone(),
            refusal(8)["parameter"].clone()
        ),
        (
            json!("nothing to undo in session s1's transaction"),
            json!("session")
        )
    );
    assert_eq!(
        (
            refusal(9)["message"].clone(),
            refusal(9)["parameter"].clone()
        ),
        (
            json!(
                "revision 99 isn't a state of session s1's undo stack that redo can restore: it can restore revisions 2 to 5"
            ),
            json!("to")
        )
    );
    // The revisions redo can restore, newest first: the session is at `begin`'s state (1
    // and 6), so those of the states after it, 41's at 2 and 4 and 42's at 3 and 5.
    assert_eq!(refusal(9)["candidates"], json!(["5", "4", "3", "2"]));
    assert_eq!(refusal(10)["message"], "no session s9 is open");
}

#[test]
fn undo_and_redo_are_edits_of_what_the_server_holds_never_read_only_nor_destructive() {
    // What the server builds, and what's committed, the same.
    let built = silico_mcp::tools::list().unwrap();
    for name in ["undo", "redo"] {
        let sent = built["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap();
        assert_eq!(*sent, tool(name), "{name}");
        let annotations = &sent["annotations"];
        assert_eq!(annotations["readOnlyHint"], false, "{name}");
        assert_eq!(annotations["destructiveHint"], false, "{name}");
        assert_eq!(annotations["openWorldHint"], false, "{name}");
        // Its description tells an agent where it works and what to give.
        let description = tool(name)["description"].as_str().unwrap().to_owned();
        assert!(
            description.contains("Inside a transaction"),
            "{name}: {description}"
        );
        assert!(description.contains("`to`"), "{name}: {description}");
        assert!(description.contains("`model: \"current\"`"), "{name}");
    }
    // `status` says what it gives now.
    let status = tool("status")["description"].as_str().unwrap().to_owned();
    assert!(status.contains("`undo` and `redo`"), "{status}");
}
