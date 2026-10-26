//! Running calls (docs/design/m4.9-mcp.md, *Running a call*): cancellation stops a call
//! and it gets no reply; progress only grows; at most 64 calls wait on sessions at once,
//! `status`, `cancel`, `version`, and `schema` outside that; calls back to back on a
//! session run in the order sent. Timing comes from the fake kernel's holds, never the
//! clock. Expected values: the note's rules and messages, written here by hand.

use serde_json::{Value, json};

use crate::client::{BOX, Client, is_box, running};

const CHECK: &str = r#"{"session": "s1", "model": "current", "fresh": false}"#;

fn opened() -> Client {
    let mut c = Client::initialized();
    c.file("part.silico", BOX);
    c.open(1, "part.silico");
    c
}

#[test]
fn a_cancelled_call_is_stopped_and_never_answered() {
    let mut c = opened();
    let hold = c.kernel.hold(is_box);
    c.send_call(2, "check", CHECK);
    running(&c.kernel, 1);
    c.send(r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 2, "reason": "changed my mind"}}"#);
    // The rebuild it waited on is stopped through the pool's superseded path: the held
    // request ends though its hold is never released.
    assert!(c.kernel.wait_idle(crate::client::HANG));
    // A call sent after it, on the same session, answers after it would have: its reply is
    // the next line, so the cancelled call's never came.
    c.send_call(3, "outline", r#"{"session": "s1"}"#);
    let reply = c.message();
    assert_eq!(reply["id"], 3, "{reply}");
    assert_eq!(reply["result"]["isError"], false);
    drop(hold);
    // A cancel of a call answered already, or of none, is ignored.
    c.send(
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 3}}"#,
    );
    c.send(r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": "nobody"}}"#);
    c.send(r#"{"jsonrpc": "2.0", "id": 4, "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":4,"result":{}}"#);
    // The cancelled call's ID may be given again at once: it's no longer in flight.
    let again = c.call(2, "version", "{}");
    assert_eq!(again["isError"], false);
}

#[test]
fn progress_only_grows_carries_its_token_and_comes_before_the_answer() {
    let mut c = Client::initialized();
    // Two bodies, two steps: the cylinder's built while the box's is held.
    c.file(
        "two.silico",
        r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [100, 0, 0], "radius": 1, "height": 1}]}"#,
    );
    c.open(1, "two.silico");
    let hold = c.kernel.hold(is_box);
    c.send(r#"{"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "check", "arguments": {"session": "s1", "model": "current", "fresh": false}, "_meta": {"progressToken": "tok-1"}}}"#);
    // The cylinder's steps done, the box's running: the count is the steps done, as
    // `status` says them while the box is held, sent with no total.
    let first = c.message();
    let done = c.envelope(3, "status", r#"{"session": "s1"}"#)["result"]["rebuild"]["done"].clone();
    assert!(done.as_u64().unwrap() >= 1, "{done}");
    assert_eq!(
        first,
        json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": "tok-1", "progress": done, "message": "k7f2qx3m box"}})
    );
    hold.release();
    let (line, before) = c.reply_line("2");
    assert!(line.contains(r#""isError":false"#), "{line}");
    let mut last = first["params"]["progress"].as_u64().unwrap();
    for notification in before {
        assert_eq!(notification["method"], "notifications/progress");
        assert_eq!(notification["params"]["progressToken"], "tok-1");
        assert!(notification["params"].get("total").is_none());
        let progress = notification["params"]["progress"].as_u64().unwrap();
        assert!(progress > last, "{progress} after {last}");
        last = progress;
    }
    // A call that gives no token gets none, and a numeric token is carried as written.
    c.send_call(3, "check", CHECK);
    let (_, before) = c.reply_line("3");
    assert!(before.is_empty(), "{before:?}");
    // Another document, so neither step comes from the cache.
    c.file(
        "again.silico",
        &c.read_file("two.silico")
            .replace(r#""length": 40"#, r#""length": 41"#)
            .replace(r#""radius": 1"#, r#""radius": 2"#),
    );
    c.open(4, "again.silico");
    let hold = c.kernel.hold(is_box);
    c.send(r#"{"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "check", "arguments": {"session": "s2", "model": "current", "fresh": false}, "_meta": {"progressToken": 7.0}}}"#);
    let line = c.line();
    assert!(
        line.contains(r#""progressToken":7.0,"progress":"#),
        "{line}"
    );
    hold.release();
    c.reply_line("5");
}

/// The refusal a call past the bound gets, as the note words it.
fn past_the_bound() -> Value {
    json!({"ok": false, "session": null, "model": null, "errors": [{"code": "invalid_input", "message": "64 calls are already running in this server, the most it runs at once", "feature": null, "elements": [], "elements_total": 0, "parameter": null, "cause": null, "suggestion": "wait for one to answer, or cancel one, and call again", "candidates": []}], "total": 1})
}

#[test]
fn past_64_calls_waiting_a_call_is_refused_at_once_but_status_and_cancel_still_answer() {
    let mut c = opened();
    let hold = c.kernel.hold(is_box);
    // 64 checks: the first held on the kernel, the rest queued behind it.
    for id in 100..164 {
        c.send_call(id, "check", CHECK);
    }
    running(&c.kernel, 1);
    let refused = c.call(200, "check", CHECK);
    assert_eq!(refused["isError"], true);
    assert_eq!(refused["structuredContent"], past_the_bound());
    // Never started: nothing more was asked of the kernel.
    let asked = c.kernel.asked().len();
    // Calls that never wait on a session take no slot.
    let status = c.envelope(201, "status", r#"{"session": "s1"}"#);
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["result"]["running"], "check");
    assert_eq!(c.envelope(202, "version", "{}")["ok"], true);
    assert_eq!(c.envelope(203, "schema", r#"{"of": "close"}"#)["ok"], true);
    assert_eq!(c.kernel.asked().len(), asked);
    let cancel = c.envelope(204, "cancel", r#"{"session": "s1", "command": "check"}"#);
    assert_eq!(
        cancel["result"],
        json!({"command": "check", "stopped": true})
    );
    // The cancelled check answers `cancelled`; the rest run once the box is let go.
    hold.release();
    let mut answered = 0;
    while answered < 64 {
        let reply = c.message();
        let id = reply["id"].as_u64().unwrap();
        assert!((100..164).contains(&id), "{reply}");
        let envelope = &reply["result"]["structuredContent"];
        if id == 100 {
            assert_eq!(envelope["errors"][0]["code"], "cancelled", "{reply}");
        } else {
            assert_eq!(envelope["ok"], true, "{reply}");
        }
        answered += 1;
    }
    // Each slot is free once its call answered: the next call runs.
    c.file("other.silico", BOX);
    assert_eq!(c.open(300, "other.silico"), "s2");
}

#[test]
fn calls_back_to_back_on_one_session_run_in_the_order_sent() {
    let mut c = Client::initialized();
    for name in ["a", "b", "c"] {
        c.file(&format!("{name}.silico"), BOX);
    }
    // Sent without waiting: opens take IDs in the order sent.
    for (id, name) in [(1, "a"), (2, "b"), (3, "c")] {
        c.send_call(
            id,
            "open",
            &format!(r#"{{"path": "{name}.silico", "create": false}}"#),
        );
    }
    let mut opened = std::collections::BTreeMap::new();
    while opened.len() < 3 {
        let reply = c.message();
        let envelope = &reply["result"]["structuredContent"];
        opened.insert(
            reply["id"].as_u64().unwrap(),
            (
                envelope["result"]["path"].as_str().unwrap().to_owned(),
                envelope["session"]["id"].as_str().unwrap().to_owned(),
            ),
        );
    }
    let opened: Vec<(u64, (String, String))> = opened.into_iter().collect();
    assert_eq!(
        opened,
        [
            (1, ("a.silico".to_owned(), "s1".to_owned())),
            (2, ("b.silico".to_owned(), "s2".to_owned())),
            (3, ("c.silico".to_owned(), "s3".to_owned())),
        ]
    );
    // A transaction's edits, a read, and the commit, back to back: each sees the one
    // before.
    let calls = [
        (10, "begin", r#"{"session": "s1"}"#.to_owned()),
        (11, "set_field", r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#.to_owned()),
        (12, "set_field", r#"{"session": "s1", "revision": 2, "feature": "k7f2qx3m", "field": "length", "value": 42}"#.to_owned()),
        (13, "text", r#"{"session": "s1", "feature": "k7f2qx3m"}"#.to_owned()),
        (14, "commit", r#"{"session": "s1"}"#.to_owned()),
    ];
    for (id, name, arguments) in &calls {
        c.send_call(*id, name, arguments);
    }
    let mut answers = std::collections::BTreeMap::new();
    while answers.len() < calls.len() {
        let reply = c.message();
        answers.insert(
            reply["id"].as_u64().unwrap(),
            reply["result"]["structuredContent"].clone(),
        );
    }
    for (id, revision) in [(10, 1), (11, 2), (12, 3), (13, 3)] {
        assert_eq!(answers[&id]["ok"], true, "{}", answers[&id]);
        assert_eq!(answers[&id]["session"]["revision"], revision, "{id}");
    }
    let text = answers[&13]["result"]["text"].as_str().unwrap();
    assert!(text.contains("\"length\": 42,"), "{text}");
    assert_eq!(answers[&14]["ok"], true, "{}", answers[&14]);
    assert_eq!(answers[&14]["session"]["transaction"], false);
    assert!(c.read_file("a.silico").contains("\"length\": 42,"));
}
