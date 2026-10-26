//! Running calls (docs/design/m4.9-mcp.md, *Running a call*): cancellation stops a call
//! and it gets no reply; progress only grows; at most 64 calls wait on sessions at once,
//! `status`, `cancel`, `version`, and `schema` outside that; calls back to back on a
//! session run in the order sent. Timing comes from the fake kernel's holds, never the
//! clock. Expected values: the note's rules and messages, written here by hand.

use serde_json::{Value, json};
use silico_core::kernel::{Op, Primitive};

use crate::client::{BOX, CHECK, Client, is_box, running};

#[test]
fn a_cancelled_call_is_stopped_and_never_answered() {
    let mut c = Client::opened();
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

/// A box and a cylinder: two steps, each its own primitive.
const TWO: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [100, 0, 0], "radius": 1, "height": 1}]}"#;

fn is_cylinder(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
}

/// The progress notification `message` is, its token and count; none if it's another
/// message.
fn progress(message: &Value) -> Option<(Value, u64)> {
    (message["method"] == "notifications/progress").then(|| {
        (
            message["params"]["progressToken"].clone(),
            message["params"]["progress"].as_u64().unwrap(),
        )
    })
}

#[test]
fn progress_is_sent_once_for_each_count_and_only_as_steps_are_done() {
    let mut c = Client::initialized();
    c.file("two.silico", TWO);
    c.open(1, "two.silico");
    let hold = c.kernel.hold(is_box);
    c.send(r#"{"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "check", "arguments": {"session": "s1", "model": "current", "fresh": false}, "_meta": {"progressToken": "t"}}}"#);
    let first = c.message();
    let (token, mut last) = progress(&first).unwrap();
    assert_eq!(token, "t");
    // Every step that can be done with the box held, as `status` counts them: the
    // notifications up to it each greater than the last, the last of them that count.
    c.send(r#"{"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "status", "arguments": {"session": "s1"}}}"#);
    let (line, before) = c.reply_line("3");
    let status: Value = serde_json::from_str(&line).unwrap();
    let done = status["result"]["structuredContent"]["result"]["rebuild"]["done"]
        .as_u64()
        .unwrap();
    for message in before {
        let (_, count) = progress(&message).unwrap();
        assert!(count > last, "{count} after {last}");
        last = count;
    }
    while last < done {
        let (_, count) = progress(&c.message()).unwrap();
        assert!(count > last, "{count} after {last}");
        last = count;
    }
    assert_eq!(last, done);
    // No step more done while the box is held: nothing more is sent, however often the
    // waiter looks (ten times over), neither the same count again nor a greater.
    assert_eq!(c.line_within(silico_mcp::PROGRESS_STEP * 10), None);
    hold.release();
    let (line, before) = c.reply_line("2");
    assert!(line.contains(r#""isError":false"#), "{line}");
    // The box's step at most, once.
    for message in before {
        let (_, count) = progress(&message).unwrap();
        assert!(count > last && count <= done + 1, "{count} after {last}");
        last = count;
    }
}

#[test]
fn a_cancelled_call_sends_no_progress_though_its_session_rebuilds_on() {
    let mut c = Client::initialized();
    c.file("two.silico", TWO);
    c.open(1, "two.silico");
    let boxes = c.kernel.hold(is_box);
    let cylinders = c.kernel.hold(is_cylinder);
    c.send_call(2, "check", CHECK);
    running(&c.kernel, 2);
    // Two checks queued behind it, each asking for progress, which is the rebuild the
    // first waits on; the first of them cancelled, so it's answered (`cancelled`, unsent)
    // only when it's taken from the queue, and its waiter looks at progress till then.
    for (id, token) in [(3, "gone"), (4, "seen")] {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "tools/call", "params": {{"name": "check", "arguments": {CHECK}, "_meta": {{"progressToken": "{token}"}}}}}}"#
        ));
    }
    c.send(
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 3}}"#,
    );
    c.send(r#"{"jsonrpc": "2.0", "id": 5, "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":5,"result":{}}"#);
    // A step done: the call still running is told, and the cancelled one, which has
    // looked as often since (three times over), isn't.
    cylinders.release();
    let mut seen = Vec::new();
    loop {
        let message = c.message();
        let (token, _) = progress(&message).unwrap();
        seen.push(token.clone());
        if token == "seen" {
            break;
        }
    }
    while let Some(line) = c.line_within(silico_mcp::PROGRESS_STEP * 3) {
        seen.push(progress(&serde_json::from_str(&line).unwrap()).unwrap().0);
    }
    boxes.release();
    let mut answered = Vec::new();
    while answered.len() < 2 {
        let message = c.message();
        match progress(&message) {
            Some((token, _)) => seen.push(token),
            None => answered.push(message["id"].as_u64().unwrap()),
        }
    }
    answered.sort_unstable();
    assert_eq!(answered, [2, 4]);
    assert!(seen.iter().all(|token| token == "seen"), "{seen:?}");
    // Nothing for the cancelled call after: the next line is the pong.
    c.send(r#"{"jsonrpc": "2.0", "id": 6, "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":6,"result":{}}"#);
}

#[test]
fn a_cancel_naming_the_call_by_another_spelling_of_its_id_cancels_it() {
    let mut c = Client::opened();
    let hold = c.kernel.hold(is_box);
    c.send_call_as(r#""ab""#, "check", CHECK);
    running(&c.kernel, 1);
    c.send(r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": "\u0061b"}}"#);
    // Stopped, though its hold is held, and never answered: the next line is the call
    // after it's.
    assert!(c.kernel.wait_idle(crate::client::HANG));
    c.send_call(3, "outline", r#"{"session": "s1"}"#);
    let reply = c.message();
    assert_eq!(reply["id"], 3, "{reply}");
    drop(hold);
}

/// The refusal a call past the bound gets, as the note words it.
fn past_the_bound() -> Value {
    json!({"ok": false, "session": null, "model": null, "errors": [{"code": "invalid_input", "message": "64 calls are already running in this server, the most it runs at once", "feature": null, "elements": [], "elements_total": 0, "parameter": null, "cause": null, "suggestion": "wait for one to answer, or cancel one, and call again", "candidates": []}], "total": 1})
}

#[test]
fn past_64_calls_waiting_a_call_is_refused_at_once_but_status_and_cancel_still_answer() {
    let mut c = Client::opened();
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
    // The cancel's reply is queued with the others, and the check it stops answers from
    // its own thread, so that answer may come first: kept, and counted, not passed over
    // (the review, round 4: a test that skipped it waited for an answer already read).
    c.send_call(204, "cancel", r#"{"session": "s1", "command": "check"}"#);
    let (line, before) = c.reply_line("204");
    let cancel: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        cancel["result"]["structuredContent"]["result"],
        json!({"command": "check", "stopped": true})
    );
    // The cancelled check answers `cancelled`; the rest run once the box is let go.
    hold.release();
    let mut early = before.into_iter();
    let mut answered = 0;
    while answered < 64 {
        let reply = early.next().unwrap_or_else(|| c.message());
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

#[test]
fn a_cancel_read_before_a_command_was_sent_never_stops_it_however_far_behind_the_client_reads() {
    // From the review (round 2): `cancel` was run when the replies' thread reached it, so
    // behind a backlog of replies a client hadn't read it could stop a command sent after
    // it (the one it meant having finished meanwhile). It's answered as it's read now.
    let mut c = Client::opened();
    let hold = c.kernel.hold(is_box);
    c.send_call(10, "check", CHECK);
    running(&c.kernel, 1);
    // A backlog: the client reads nothing, and pings' replies wait to be written.
    c.stall_output();
    for id in 20..25 {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "ping"}}"#
        ));
    }
    c.send_call(11, "cancel", r#"{"session": "s1", "command": "check"}"#);
    c.send_call(12, "check", CHECK);
    // The first check, cancelled as the cancel was read, ends before the second runs. Given
    // time to end before the output is free (as a cancel run late would let it), it still
    // answers cancelled: the cancel stopped it while it was held.
    hold.release();
    std::thread::sleep(std::time::Duration::from_millis(300));
    c.release_output();
    let mut seen = std::collections::BTreeMap::new();
    while seen.len() < 8 {
        let reply = c.message();
        seen.insert(reply["id"].as_u64().unwrap(), reply);
    }
    let envelope = |id: u64| seen[&id]["result"]["structuredContent"].clone();
    assert_eq!(
        envelope(10)["errors"][0]["code"],
        "cancelled",
        "{}",
        seen[&10]
    );
    assert_eq!(
        envelope(11)["result"],
        json!({"command": "check", "stopped": true})
    );
    assert_eq!(envelope(12)["ok"], true, "{}", seen[&12]);
}

#[test]
fn progress_dropped_while_the_output_is_busy_is_sent_once_it_is_free() {
    // From the review (round 2): a count that couldn't be sent while another write held the
    // output must stay to send, not be marked sent and lost.
    let mut c = Client::initialized();
    c.file("two.silico", TWO);
    c.open(1, "two.silico");
    let boxes = c.kernel.hold(is_box);
    let cylinders = c.kernel.hold(is_cylinder);
    c.send(r#"{"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "check", "arguments": {"session": "s1", "model": "current", "fresh": false}, "_meta": {"progressToken": "t"}}}"#);
    running(&c.kernel, 2);
    c.stall_output();
    c.send(r#"{"jsonrpc": "2.0", "id": 3, "method": "ping"}"#);
    c.wait_stalled();
    // A step done while the output is held: its count can't be sent yet.
    cylinders.release();
    std::thread::sleep(silico_mcp::PROGRESS_STEP * 5);
    c.release_output();
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":3,"result":{}}"#);
    let line = c
        .line_within(crate::client::HANG)
        .expect("no progress after the output was free");
    let message: Value = serde_json::from_str(&line).unwrap();
    let (token, count) = progress(&message).unwrap();
    assert_eq!(token, "t");
    assert!(count >= 1, "{message}");
    boxes.release();
    let (line, _) = c.reply_line("2");
    assert!(line.contains(r#""isError":false"#), "{line}");
}

#[test]
fn a_slot_free_call_never_holds_serve_while_the_output_is_busy() {
    // From the review (round 3): a slot-free call's reply written on serve's thread would
    // hold it on a client that isn't reading, so no later line (a call, a cancel) is read.
    // It's queued with the other replies.
    let mut c = Client::opened();
    let hold = c.kernel.hold(is_box);
    c.stall_output();
    c.send(r#"{"jsonrpc": "2.0", "id": 2, "method": "ping"}"#);
    c.wait_stalled();
    c.send_call(3, "version", "{}");
    c.send_call(4, "check", CHECK);
    // The check after it is read and runs while the output is still stalled.
    running(&c.kernel, 1);
    hold.release();
    c.release_output();
    let ids: Vec<u64> = (0..3)
        .map(|_| c.message()["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids[..2], [2, 3]);
}
