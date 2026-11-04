//! Untitled parts over MCP (docs/design/m5.0-app.md, open item 30, overturned by Trevin
//! 2026-11-01): the same command as every client's (invariant 3), `open` with `path` null
//! and `create` true, an agent's scratch part with no file, saved where its first `commit`
//! says. Expected values: the design's §2 shapes and words, and the box's canonical text
//! with its fingerprint by formula (40 × 20 × 10: volume 8000, area 2·(800 + 400 + 200) =
//! 2800), as the core's own tests write it.

use serde_json::{Value, json};

use crate::client::Client;
use crate::contract::holds;

/// The box, in canonical form, with the fingerprint a commit writes for it.
const SAVED: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 8000, \"area\": 2800, \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n";

const THE_BOX: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;

/// The tool `name` as `tools/list` gives it.
fn tool(list: &Value, name: &str) -> Value {
    list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("no tool {name}"))
}

#[test]
fn an_agent_opens_an_untitled_part_edits_it_in_a_transaction_and_saves_it_with_a_path() {
    let mut c = Client::initialized();
    let list = c.request(1, "tools/list", "{}")["result"].clone();
    let open_tool = tool(&list, "open");
    let described = open_tool["description"].as_str().unwrap();
    assert!(
        described.ends_with(silico_mcp::tools::UNTITLED),
        "{described}"
    );
    // Opened untitled: §2's result and envelope, holding to the tool's output schema.
    let opened = c.call(2, "open", r#"{"path": null, "create": true}"#);
    assert_eq!(opened["isError"], false, "{opened}");
    let envelope = &opened["structuredContent"];
    assert_eq!(
        envelope,
        &json!({
            "ok": true,
            "session": {"id": "s1", "path": null, "revision": 1, "transaction": false, "reloaded": false},
            "model": null,
            "result": {"path": null, "created": true, "features": 0, "bodies": [], "problems": [], "problems_total": 0}
        })
    );
    holds(&open_tool["outputSchema"], envelope, "an untitled open");
    // An edit outside a transaction would be saved as it's made: refused, naming `session`.
    let add =
        format!(r#"{{"session": "s1", "revision": 1, "before": null, "feature": {THE_BOX}}}"#);
    let refused = c.call(3, "add_feature", &add);
    assert_eq!(refused["isError"], true, "{refused}");
    let error = &refused["structuredContent"]["errors"][0];
    assert_eq!(
        (&error["parameter"], &error["message"], &error["suggestion"]),
        (
            &json!("session"),
            &json!("session s1 is untitled: it has no file to save to"),
            &json!(
                "begin a transaction first; its commit, given a path, saves every edit it holds (Save As)"
            )
        )
    );
    holds(
        &tool(&list, "add_feature")["outputSchema"],
        &refused["structuredContent"],
        "a refused edit of an untitled session",
    );
    // Inside a transaction, it's made; a commit with no path is refused, naming `path`.
    assert_eq!(c.envelope(4, "begin", r#"{"session": "s1"}"#)["ok"], true);
    let added = c.envelope(5, "add_feature", &add);
    assert_eq!(added["ok"], true, "{added}");
    let own = c.envelope(
        6,
        "commit",
        r#"{"session": "s1", "path": null, "replace": false}"#,
    );
    assert_eq!(own["errors"][0]["parameter"], "path", "{own}");
    assert_eq!(own["session"]["transaction"], true);
    assert!(std::fs::read_dir(&c.work).unwrap().next().is_none());
    // Saved with a path: written there, and the session holds it from then on.
    let saved = c.call(
        7,
        "commit",
        r#"{"session": "s1", "path": "bracket.silico", "replace": false}"#,
    );
    let envelope = &saved["structuredContent"];
    assert_eq!(envelope["ok"], true, "{saved}");
    assert_eq!(
        envelope["result"],
        json!({"written": true, "path": "bracket.silico", "refingerprinted": ["k7f2qx3m"], "resolved": [], "problems": [], "problems_total": 0})
    );
    assert_eq!(envelope["session"]["path"], "bracket.silico");
    holds(
        &tool(&list, "commit")["outputSchema"],
        envelope,
        "an untitled session's first save",
    );
    assert_eq!(c.read_file("bracket.silico"), SAVED);
    let status = c.envelope(8, "status", r#"{"session": "s1"}"#);
    assert_eq!(status["session"]["path"], "bracket.silico", "{status}");
}

#[test]
fn a_null_path_without_create_is_refused_and_the_instructions_say_how_to_make_one() {
    let mut c = Client::start();
    let reply = c.request(
        1,
        "initialize",
        r#"{"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}"#,
    );
    let instructions = reply["result"]["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("open with `path: null` and `create: true`: an untitled part"),
        "{instructions}"
    );
    c.send(r#"{"jsonrpc": "2.0", "method": "notifications/initialized"}"#);
    let refused = c.call(2, "open", r#"{"path": null, "create": false}"#);
    assert_eq!(refused["isError"], true, "{refused}");
    let envelope = &refused["structuredContent"];
    assert_eq!(envelope["session"], Value::Null);
    let error = &envelope["errors"][0];
    assert_eq!(
        (&error["parameter"], &error["message"], &error["suggestion"]),
        (
            &json!("path"),
            &json!("path null opens nothing"),
            &json!("give a path to open, or create: true for an untitled document")
        )
    );
    // No session opened: the next is s1.
    let opened = c.envelope(3, "open", r#"{"path": null, "create": true}"#);
    assert_eq!(opened["session"]["id"], "s1", "{opened}");
}
