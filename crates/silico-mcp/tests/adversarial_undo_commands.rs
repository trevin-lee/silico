//! Hostile calls of M5.2's `undo` and `redo` tools and `status`'s transaction counts (PR
//! #300): `to` that's 0, negative, fractional, a string, a list, an object, a boolean, past
//! u64, u64::MAX, never made, or the session's own revision; `revision` stale, ahead,
//! missing, or of the wrong type; `session` missing, empty, or not open; fields extra or
//! given twice; and steps after the transaction's commit, when `status` gives none. Each
//! must be answered, never hang or end the server, with the core's own envelope byte for
//! byte (the same calls on `Sessions::call`, in a workspace of its own), every answer
//! holding to its tool's `outputSchema` in the committed `tools.json` and every call's
//! arguments that hold to its `inputSchema` run or refused as the core does, a refusal
//! flagged `isError`, and the session still stepping after.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};

use crate::client::{BOX, Client, HANG, Reply};
use crate::contract::holds;

/// The calls, and whether each is refused, by hand.
fn calls() -> Vec<(&'static str, String, bool)> {
    let set = |revision: u64, length: u32| {
        format!(
            r#"{{"session": "s1", "revision": {revision}, "feature": "k7f2qx3m", "field": "length", "value": {length}}}"#
        )
    };
    let step = |revision: &str, to: &str| {
        format!(r#"{{"session": "s1", "revision": {revision}, "to": {to}}}"#)
    };
    let mut calls: Vec<(&'static str, String, bool)> = vec![
        (
            "open",
            r#"{"path": "part.silico", "create": false}"#.to_owned(),
            false,
        ),
        // Outside a transaction: refused.
        ("undo", step("1", "null"), true),
        ("redo", step("1", "null"), true),
        ("begin", r#"{"session": "s1"}"#.to_owned(), false),
        ("set_field", set(1, 41), false),
        ("set_field", set(2, 42), false),
    ];
    // At revision 3: undo can reach 1 and 2, nothing to redo.
    for name in ["undo", "redo"] {
        for to in [
            "0",
            "-1",
            "1.5",
            "2.0",
            "1e400",
            r#""2""#,
            "[2]",
            "{}",
            "true",
            "18446744073709551616",
            "18446744073709551615",
            "99",
            "3",
        ] {
            calls.push((name, step("3", to), true));
        }
        for revision in [
            "2",
            "4",
            "0",
            "-3",
            "3.5",
            r#""3""#,
            "null",
            "18446744073709551616",
        ] {
            calls.push((name, step(revision, "null"), true));
        }
        for arguments in [
            r#"{"session": "s1", "revision": 3}"#,
            r#"{"session": "s1", "to": null}"#,
            r#"{"revision": 3, "to": null}"#,
            r#"{"session": "", "revision": 3, "to": null}"#,
            r#"{"session": "s9", "revision": 3, "to": null}"#,
            r#"{"session": null, "revision": 3, "to": null}"#,
            r#"{"session": "s1", "revision": 3, "to": null, "extra": 1}"#,
            r#"{"session": "s1", "revision": 3, "to": null, "to": 1}"#,
            "{}",
        ] {
            calls.push((name, arguments.to_owned(), true));
        }
    }
    calls.extend([
        ("redo", step("3", "null"), true),
        ("redo", step("3", "1"), true),
        (
            "status",
            r#"{"session": "s1", "extra": 1}"#.to_owned(),
            true,
        ),
        ("status", r#"{"session": 1}"#.to_owned(), true),
        // Still steps: one undo, a redo, an undo `to` 1, a redo `to` 3, then an edit after an
        // undo, which drops the redo.
        ("undo", step("3", "null"), false),
        ("status", r#"{"session": "s1"}"#.to_owned(), false),
        ("redo", step("4", "null"), false),
        ("undo", step("5", "1"), false),
        ("status", r#"{"session": "s1"}"#.to_owned(), false),
        ("redo", step("6", "3"), false),
        ("undo", step("7", "null"), false),
        ("set_field", set(8, 43), false),
        ("redo", step("9", "null"), true),
        ("status", r#"{"session": "s1"}"#.to_owned(), false),
        ("commit", r#"{"session": "s1"}"#.to_owned(), false),
        ("status", r#"{"session": "s1"}"#.to_owned(), false),
        ("undo", step("9", "null"), true),
    ]);
    calls
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
fn hostile_undo_redo_and_status_calls_are_answered_as_the_core_answers_them_and_hold_to_their_tools()
 {
    let calls = calls();
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
    let core: Vec<(String, bool)> = calls
        .iter()
        .map(
            |(name, arguments, _)| match sessions.call(name, arguments) {
                Call::Running(calling) => {
                    let answered = calling.wait_timeout(HANG).ok().unwrap();
                    (answered.text, !answered.ok)
                }
                Call::Unknown => panic!("{name}"),
            },
        )
        .collect();
    let mut c = Client::initialized();
    c.file("part.silico", BOX);
    let mut envelopes = Vec::new();
    let mut differ = Vec::new();
    for (at, ((name, arguments, refused), core)) in calls.iter().zip(&core).enumerate() {
        let id = at as u64 + 1;
        c.send_call(id, name, arguments);
        let (line, _) = c.reply_line(&id.to_string());
        let reply: Reply<'_> = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("{name} {arguments}: {e}: {line}"));
        let answer = reply.result;
        assert_eq!(answer.content.len(), 1, "{line}");
        assert_eq!(
            answer.content[0].text.as_deref(),
            Some(answer.structured.get()),
            "{name} {arguments}"
        );
        let text = answer.structured.get().to_owned();
        assert_eq!(
            (&text, answer.is_error),
            (&core.0, core.1),
            "call {at}: {name} {arguments}"
        );
        let envelope: Value = serde_json::from_str(&text).unwrap();
        holds(
            &tool(name)["outputSchema"],
            &envelope,
            &format!("call {at}: {name} {arguments}"),
        );
        if answer.is_error != *refused {
            differ.push((at, *name, arguments.clone(), text.clone()));
        }
        envelopes.push(envelope);
    }
    assert!(
        differ.is_empty(),
        "ran or refused otherwise than by hand: {differ:#?}"
    );
    // The statuses, by hand: after one undo, 41's state, one each way; after undoing `to`
    // 1, `begin`'s, unchanged, two to redo; after the edit that dropped the redo, 43's, two
    // to undo; after the commit, none.
    let statuses: Vec<&Value> = calls
        .iter()
        .zip(&envelopes)
        .filter(|((name, arguments, _), _)| {
            *name == "status" && arguments == r#"{"session": "s1"}"#
        })
        .map(|(_, envelope)| &envelope["result"]["transaction"])
        .collect();
    let each = 24 + 8 + 16 + 4 + 122;
    assert_eq!(
        statuses,
        [
            &serde_json::json!({"base": 1, "changed": true, "undo": 1, "redo": 1, "dropped": 0, "bytes": BOX.len() + each}),
            &serde_json::json!({"base": 1, "changed": false, "undo": 0, "redo": 2, "dropped": 0, "bytes": BOX.len() + 2 * each}),
            &serde_json::json!({"base": 1, "changed": true, "undo": 2, "redo": 0, "dropped": 0, "bytes": BOX.len() + each}),
            &Value::Null,
        ]
    );
}
