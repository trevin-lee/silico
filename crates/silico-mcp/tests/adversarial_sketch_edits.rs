//! Hostile `sketch_add` and `sketch_remove` tool calls over MCP: a batch of a hundred
//! thousand points in one line, arguments of every JSON type, and two edits at one revision
//! sent together. Each is answered as data, once, and the server reads on. Expected values:
//! the design's rules (an edit against a revision the session has left is a `conflict`; the
//! solver's limits refused at the list that passes them), by hand.

use serde_json::{Value, json};

use crate::client::Client;

/// A sketch of a line from the origin, and nothing else.
const LINE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 3, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}]}"#;

fn begun() -> Client {
    let mut c = Client::initialized();
    c.file("part.silico", LINE);
    c.open(1, "part.silico");
    let begun = c.envelope(2, "begin", r#"{"session": "s1"}"#);
    assert_eq!(begun["ok"], true, "{begun}");
    c
}

fn add_point(revision: u64, id: &str) -> String {
    format!(
        r#"{{"session": "s1", "revision": {revision}, "sketch": "k7f2qx3m", "points": [{{"id": "{id}", "x": 1, "y": 1, "construction": true}}], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#
    )
}

#[test]
fn a_batch_of_a_hundred_thousand_points_in_one_line_is_refused_at_points_and_the_server_reads_on() {
    let mut c = begun();
    let points: Vec<String> = (0..100_000)
        .map(|i| format!(r#"{{"id": "q{i:07}", "x": 1, "y": 1, "construction": false}}"#))
        .collect();
    let arguments = format!(
        r#"{{"session": "s1", "revision": 1, "sketch": "k7f2qx3m", "points": [{}], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", ")
    );
    let envelope = c.envelope(3, "sketch_add", &arguments);
    assert_eq!(envelope["ok"], false, "{envelope}");
    let at: Vec<&str> = envelope["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["parameter"].as_str().unwrap())
        .collect();
    assert!(at.contains(&"points"), "{envelope}");
    for e in envelope["errors"].as_array().unwrap() {
        assert_eq!(e["feature"], "k7f2qx3m", "{e}");
    }
    // The session is as it was, and takes the next edit.
    let next = c.envelope(4, "sketch_add", &add_point(1, "p0000003"));
    assert_eq!(next["ok"], true, "{next}");
    assert_eq!(next["result"]["added"], json!(["p0000003"]));
}

#[test]
fn arguments_of_every_json_type_but_an_object_are_refused_as_data() {
    let mut c = begun();
    for (n, arguments) in ["[]", "null", "\"s1\"", "7", "true", "[{}]"]
        .iter()
        .enumerate()
    {
        for (m, tool) in ["sketch_add", "sketch_remove"].iter().enumerate() {
            let id = 10 + 2 * n as u64 + m as u64;
            c.send_call(id, tool, arguments);
            let (line, _) = c.reply_line(&id.to_string());
            let reply: Value = serde_json::from_str(&line).unwrap();
            // Either the tool's refusal as its envelope, or JSON-RPC's invalid params:
            // never a result that ran.
            let ran = reply["result"]["structuredContent"]["ok"] == true;
            assert!(!ran, "{tool} {arguments}: {line}");
        }
    }
    let next = c.envelope(99, "sketch_add", &add_point(1, "p0000003"));
    assert_eq!(next["ok"], true, "{next}");
}

#[test]
fn an_add_and_a_remove_at_one_revision_sent_together_are_one_edit_and_one_conflict() {
    let mut c = begun();
    c.send_call(3, "sketch_add", &add_point(1, "p0000003"));
    c.send_call(
        4,
        "sketch_remove",
        r#"{"session": "s1", "revision": 1, "sketch": "k7f2qx3m", "members": ["n0000001"]}"#,
    );
    // The session runs its commands in turn, in the order they came, but each call's reply
    // is written when its call is done, so the remove's may come first: read both, in any
    // order (reading 3's alone would drop a 4 written before it, and wait for it forever).
    let envelopes: Vec<Value> = c
        .replies(&["3", "4"])
        .iter()
        .map(|line| {
            let reply: Value = serde_json::from_str(line).unwrap();
            reply["result"]["structuredContent"].clone()
        })
        .collect();
    let (added, removed) = (&envelopes[0], &envelopes[1]);
    // The add first.
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(removed["ok"], false, "{removed}");
    assert_eq!(removed["errors"][0]["code"], "conflict", "{removed}");
    let text = c.envelope(5, "text", r#"{"session": "s1", "feature": null}"#);
    let document: Value = serde_json::from_str(text["result"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        document["features"][0]["lines"][0]["id"], "n0000001",
        "the conflicting remove reached the document"
    );
    // Nothing reached the file: the transaction holds the edit.
    assert_eq!(c.read_file("part.silico"), LINE);
}

#[test]
fn replies_sent_together_are_read_whichever_the_server_wrote_first() {
    // The reader the test above relies on, with the remove's reply (4) written before the
    // add's (3), a notification between: each is given its own, and nothing waits.
    let lines = [
        r#"{"jsonrpc": "2.0", "id": 4, "result": {"which": "remove"}}"#,
        r#"{"jsonrpc": "2.0", "method": "notifications/message", "params": {}}"#,
        r#"{"jsonrpc": "2.0", "id": 3, "result": {"which": "add"}}"#,
    ];
    let mut next = lines.iter().map(|line| (*line).to_owned());
    let got = crate::client::replies_in(|| next.next().unwrap(), &["3", "4"]);
    assert_eq!(got, [lines[2], lines[0]]);
}
