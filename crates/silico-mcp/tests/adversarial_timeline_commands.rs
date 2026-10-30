//! Hostile calls of M5.1's timeline tools (PR #295): `move_feature`, `add_feature` with
//! `before`, `remove_feature`, and `timeline`, each with fields missing, null, of the wrong
//! type, extra, or naming what can't be: the feature itself, a sketch, a consumed feature,
//! one after it, none at all. Each must be answered, never hang or end the server, by the
//! core's own envelope byte for byte (the same calls on `Sessions::call`, in a workspace of
//! its own), every answer holding to its tool's `outputSchema` in the committed
//! `tools.json`, a refusal flagged `isError`, and the session still working after.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};

use crate::client::{Client, HANG, Reply};
use crate::contract::holds;

/// A block, a fillet of its Edge1 on a chamfer of its Edge3, and a lug nothing uses.
const PARTS: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "size": {"distance": 1}}, {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "c0000001", "edges": ["k7f2qx3m.box.Edge1"], "radius": 1}, {"id": "b0000002", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 5, "width": 5, "height": 5}]}"#;

fn calls() -> Vec<(&'static str, String)> {
    let big = "a".repeat(100_000);
    let mut calls: Vec<(&'static str, String)> = vec![(
        "open",
        r#"{"path": "part.silico", "create": false}"#.to_owned(),
    )];
    let hostile_moves = [
        r#"{"session": "s1", "revision": 1, "feature": "b0000002"}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "before": null}"#.to_owned(),
        r#"{"session": null, "revision": 1, "feature": "b0000002", "before": null}"#.to_owned(),
        r#"{"session": "s1", "revision": null, "feature": "b0000002", "before": null}"#.to_owned(),
        r#"{"session": "s1", "revision": "1", "feature": "b0000002", "before": null}"#.to_owned(),
        r#"{"session": "s1", "revision": 1.5, "feature": "b0000002", "before": null}"#.to_owned(),
        r#"{"session": "s1", "revision": -1, "feature": "b0000002", "before": null}"#.to_owned(),
        r#"{"session": "s1", "revision": 0, "feature": "b0000002", "before": null}"#.to_owned(),
        r#"{"session": "s1", "revision": 18446744073709551616, "feature": "b0000002", "before": null}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": null, "before": null}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": 7, "before": null}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": "b0000002", "before": 7}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": "b0000002", "before": ["k7f2qx3m"]}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": "b0000002", "before": null, "extra": 1}"#.to_owned(),
        format!(r#"{{"session": "s1", "revision": 1, "feature": "b0000002", "before": "{big}"}}"#),
        format!(r#"{{"session": "s1", "revision": 1, "feature": "{big}", "before": null}}"#),
        // The chamfer before the block it's on; the block past the fillet; none such; a stale
        // revision; a session not open.
        r#"{"session": "s1", "revision": 1, "feature": "c0000001", "before": "k7f2qx3m"}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "before": null}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": "z0000009", "before": "k7f2qx3m"}"#.to_owned(),
        r#"{"session": "s1", "revision": 9, "feature": "b0000002", "before": "k7f2qx3m"}"#.to_owned(),
        r#"{"session": "s7", "revision": 1, "feature": "b0000002", "before": "k7f2qx3m"}"#.to_owned(),
    ];
    calls.extend(hostile_moves.into_iter().map(|a| ("move_feature", a)));
    // Each with its ID: one the core makes is the OS's randomness (M4.1, decision 20).
    let lug = r#"{"id": "l0000002", "type": "box", "name": "L2", "position": [200, 0, 0], "length": 1, "width": 1, "height": 1}"#;
    let on_block = r#"{"id": "c0000005", "type": "chamfer", "name": "B2", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge5"], "size": {"distance": 1}}"#;
    let hostile_adds = [
        format!(r#"{{"session": "s1", "revision": 1, "feature": {lug}}}"#),
        format!(r#"{{"session": "s1", "revision": 1, "before": 3, "feature": {lug}}}"#),
        format!(r#"{{"session": "s1", "revision": 1, "before": {{}}, "feature": {lug}}}"#),
        format!(r#"{{"session": "s1", "revision": 1, "before": "", "feature": {lug}}}"#),
        format!(r#"{{"session": "s1", "revision": 1, "before": "{big}", "feature": {lug}}}"#),
        r#"{"session": "s1", "revision": 1, "before": "k7f2qx3m", "feature": null}"#.to_owned(),
        // A chamfer on the block before the block; a box whose ID is its own `before`.
        format!(r#"{{"session": "s1", "revision": 1, "before": "k7f2qx3m", "feature": {on_block}}}"#),
        r#"{"session": "s1", "revision": 1, "before": "q0000001", "feature": {"id": "q0000001", "type": "box", "name": "Q", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#.to_owned(),
        // On the block, which the chamfer consumes, after the chamfer: refused as consumed.
        format!(r#"{{"session": "s1", "revision": 1, "before": "b0000002", "feature": {on_block}}}"#),
        format!(r#"{{"session": "s1", "revision": 1, "before": "k7f2qx3m", "feature": {lug}, "x": 0}}"#),
    ];
    calls.extend(hostile_adds.into_iter().map(|a| ("add_feature", a)));
    let hostile_removes = [
        r#"{"session": "s1", "revision": 1}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": null}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m"}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": "z0000009"}"#.to_owned(),
        r#"{"session": "s1", "revision": 1, "feature": "b0000002", "before": null}"#.to_owned(),
    ];
    calls.extend(hostile_removes.into_iter().map(|a| ("remove_feature", a)));
    let hostile_timelines = [
        r#"{"session": "s1"}"#.to_owned(),
        r#"{"session": "s1", "model": null}"#.to_owned(),
        r#"{"session": "s1", "model": "Current"}"#.to_owned(),
        r#"{"session": "s1", "model": "current", "fresh": false}"#.to_owned(),
        r#"{"session": "", "model": "current"}"#.to_owned(),
        r#"{"model": "current"}"#.to_owned(),
        format!(r#"{{"session": "{big}", "model": "current"}}"#),
    ];
    calls.extend(hostile_timelines.into_iter().map(|a| ("timeline", a)));
    // The session still works: the lug first, the chamfer removed, the timeline.
    calls.extend([
        (
            "move_feature",
            r#"{"session": "s1", "revision": 1, "feature": "b0000002", "before": "k7f2qx3m"}"#
                .to_owned(),
        ),
        (
            "remove_feature",
            r#"{"session": "s1", "revision": 2, "feature": "c0000001"}"#.to_owned(),
        ),
        (
            "timeline",
            r#"{"session": "s1", "model": "current"}"#.to_owned(),
        ),
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
fn hostile_timeline_calls_are_answered_as_the_core_answers_them_and_hold_to_their_tools() {
    let calls = calls();
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
    let core: Vec<(String, bool)> = calls
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
    let mut refused = Vec::new();
    for (at, ((name, arguments), core)) in calls.iter().zip(&core).enumerate() {
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
        assert!(
            text.len() < 20_000,
            "call {at} echoes its input: {} bytes",
            text.len()
        );
        refused.push(answer.is_error);
    }
    // Every hostile call refused; the open and the last three ran.
    let n = calls.len();
    let mut want = vec![true; n];
    want[0] = false;
    for w in &mut want[n - 3..] {
        *w = false;
    }
    let differ: Vec<(usize, &str, &str)> = refused
        .iter()
        .zip(&want)
        .enumerate()
        .filter(|(_, (got, want))| got != want)
        .map(|(at, _)| (at, calls[at].0, calls[at].1.as_str()))
        .collect();
    assert!(
        differ.is_empty(),
        "ran or refused otherwise than by hand: {differ:?}"
    );
}
