//! A client that goes away, and lines no client should send (docs/design/m4.9-mcp.md,
//! *Running a call* and *Exact JSON both ways*): the input's end, a write that fails, or
//! `Stop::stop` cancels every call in flight and drops the sessions, rolling back their
//! transactions, and `serve` ends; a line past the most read is skipped, answered by its ID
//! if that's in what was read; garbage gets an error and the server reads on. Expected
//! values: the note's rules, the document's bytes as written, and JSON-RPC's codes.

use serde_json::{Value, json};
use silico_mcp::{Ended, FRAME_ROOM, MAX_LINE};

use crate::client::{BOX, Client, HANG, is_box, rpc_error, running};

/// A session on [`BOX`] with a transaction open and an edit in it, and a check held on the
/// kernel's box.
fn mid_call() -> (Client, silico_core::fake::Hold) {
    let mut c = Client::initialized();
    c.file("part.silico", BOX);
    c.open(1, "part.silico");
    assert_eq!(c.envelope(2, "begin", r#"{"session": "s1"}"#)["ok"], true);
    let set = c.envelope(
        3,
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
    );
    assert_eq!(set["ok"], true, "{set}");
    let hold = c.kernel.hold(is_box);
    c.send_call(
        4,
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    running(&c.kernel, 1);
    (c, hold)
}

/// The server's end, as a mid-call client's leaving left it: the check never answered,
/// its rebuild stopped though its hold is held, the transaction rolled back.
fn left(mut c: Client, hold: silico_core::fake::Hold) {
    assert!(c.kernel.wait_idle(HANG));
    assert_eq!(c.read_file("part.silico"), BOX);
    while let Some(line) = c.line_within(std::time::Duration::ZERO) {
        assert!(!line.contains(r#""id":4"#), "{line}");
    }
    drop(hold);
}

#[test]
fn the_inputs_end_mid_call_cancels_it_rolls_back_and_ends_serve() {
    let (mut c, hold) = mid_call();
    c.close_input();
    assert_eq!(c.ended(), Ended::Input);
    left(c, hold);
}

#[test]
fn stop_mid_call_with_the_input_still_open_ends_serve() {
    let (mut c, hold) = mid_call();
    let stop = c.stop.clone();
    std::thread::spawn(move || stop.stop());
    assert_eq!(c.ended(), Ended::Stopped);
    left(c, hold);
}

#[test]
fn a_write_that_fails_mid_call_ends_serve() {
    let (mut c, hold) = mid_call();
    c.close_output();
    c.send(r#"{"jsonrpc": "2.0", "id": 9, "method": "ping"}"#);
    assert_eq!(c.ended(), Ended::Output);
    left(c, hold);
}

#[test]
fn replies_queued_before_the_inputs_end_are_still_written() {
    let mut c = Client::initialized();
    for id in 10..20 {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "ping"}}"#
        ));
    }
    c.close_input();
    assert_eq!(c.ended(), Ended::Input);
    for id in 10..20 {
        assert_eq!(
            c.line(),
            format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#)
        );
    }
}

#[test]
fn replies_queued_while_the_client_isnt_reading_as_the_input_ends_are_all_written() {
    let mut c = Client::initialized();
    c.stall_output();
    for id in 10..20 {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "ping"}}"#
        ));
    }
    c.wait_stalled();
    c.close_input();
    // The client reads again a while after its input closed.
    let release = c.releaser();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        release();
    });
    assert_eq!(c.ended(), Ended::Input);
    // Every pong written before `serve` returned.
    let mut written = Vec::new();
    while let Some(line) = c.line_within(std::time::Duration::ZERO) {
        written.push(line);
    }
    let pongs: Vec<String> = (10..20)
        .map(|id| format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#))
        .collect();
    assert_eq!(written, pongs);
}

/// A reader that gives `parts` in turn, each some bytes or an error, then the input's end.
struct Parts(
    std::collections::VecDeque<Result<Vec<u8>, std::io::ErrorKind>>,
    Vec<u8>,
);

impl std::io::Read for Parts {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = std::io::BufRead::fill_buf(self)?.len().min(out.len());
        out[..n].copy_from_slice(&self.1[..n]);
        std::io::BufRead::consume(self, n);
        Ok(n)
    }
}

impl std::io::BufRead for Parts {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.1.is_empty() {
            match self.0.pop_front() {
                Some(Ok(bytes)) => self.1 = bytes,
                Some(Err(kind)) => return Err(kind.into()),
                None => {}
            }
        }
        Ok(&self.1)
    }

    fn consume(&mut self, n: usize) {
        self.1.drain(..n);
    }
}

#[test]
fn a_read_interrupted_is_read_again_and_a_read_that_fails_ends_serve() {
    use std::io::ErrorKind;
    let parts = [
        Ok(br#"{"jsonrpc": "2.0", "id": 1, "#.to_vec()),
        // A signal mid-line: read again, the line whole.
        Err(ErrorKind::Interrupted),
        Ok(b"\"method\": \"ping\"}\n".to_vec()),
        Err(ErrorKind::Interrupted),
        Err(ErrorKind::PermissionDenied),
        // Never read: the input failed.
        Ok(b"{\"jsonrpc\": \"2.0\", \"id\": 2, \"method\": \"ping\"}\n".to_vec()),
    ];
    let mut c = Client::reading(Parts(parts.into_iter().collect(), Vec::new()));
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
    match c.ended() {
        Ended::InputFailed(why) => assert_eq!(
            why,
            std::io::Error::from(ErrorKind::PermissionDenied).to_string()
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(c.line_within(std::time::Duration::ZERO), None);
}

#[test]
fn an_answer_being_written_as_the_input_ends_is_written_whole_before_serve_ends() {
    let mut c = Client::initialized();
    c.file("part.silico", BOX);
    c.open(1, "part.silico");
    // The document's text, answered, its write held by a client that's stopped reading;
    // then the input ends.
    c.stall_output();
    c.send_call(2, "text", r#"{"session": "s1", "feature": null}"#);
    c.wait_stalled();
    c.close_input();
    // `serve` waits for it, however long the client takes to read (a while, here).
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!c.has_ended(), "serve ended with an answer half written");
    c.release_output();
    let (line, _) = c.reply_line("2");
    let reply: Value = serde_json::from_str(&line).unwrap();
    let envelope = &reply["result"]["structuredContent"];
    assert_eq!(envelope["ok"], true, "{line}");
    let text = envelope["result"]["text"].as_str().unwrap();
    assert!(text.contains(r#""id": "k7f2qx3m""#), "{text}");
    assert_eq!(c.ended(), Ended::Input);
}

#[test]
fn a_stop_ends_serve_at_once_though_an_answer_is_being_written() {
    let mut c = Client::initialized();
    c.file("part.silico", BOX);
    c.open(1, "part.silico");
    c.stall_output();
    c.send_call(2, "text", r#"{"session": "s1", "feature": null}"#);
    c.wait_stalled();
    c.close_input();
    // From the review (round 2): stopped at once, the stop reached serve before the reader
    // saw stdin's end, so the wait at its end (for the answer being written) was never
    // tested. Given time to reach that wait, serve is still in it: the client hasn't read.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!c.has_ended());
    // A stop ends it there, as SIGTERM must when a client closed stdin and stopped reading.
    // (Either way it ended: should the reader be slower than that on a loaded machine, the
    // stop comes first, and serve ends Stopped; a stop the wait ignored would hang.)
    c.stop.stop();
    assert!(matches!(c.ended(), Ended::Input | Ended::Stopped));
    c.release_output();
}

#[test]
fn a_line_past_the_most_read_is_skipped_and_answered_by_its_id_if_it_was_read() {
    let mut c = Client::initialized();
    let most = MAX_LINE;
    assert_eq!(most, (64 << 20) + FRAME_ROOM);
    // One byte past: refused by its ID, at the front of the line.
    let head = r#"{"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "schema", "arguments": {"of": ""#;
    let tail = r#""}}}"#;
    let line = format!(
        "{head}{}{tail}",
        "x".repeat(most + 1 - head.len() - tail.len())
    );
    assert_eq!(line.len(), most + 1);
    c.send(&line);
    let (code, id, error) = rpc_error(&c.message());
    assert_eq!((code, id), (-32600, json!(5)));
    assert_eq!(
        error["message"],
        format!(
            "the message is {} bytes, more than the {most} a message may be",
            most + 1
        )
    );
    // Its ID past what was read: answered with none.
    let tail = r#""}}, "id": 6}"#;
    let line = format!(
        r#"{{"jsonrpc": "2.0", "method": "x", "params": "{}{tail}"#,
        "x".repeat(most)
    );
    c.send(&line);
    let (code, id, _) = rpc_error(&c.message());
    assert_eq!((code, id), (-32600, Value::Null));
    // The next line is read whole.
    c.send(r#"{"jsonrpc": "2.0", "id": 7, "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":7,"result":{}}"#);
    // Arguments one byte past what the core reads, in a line within the most: the core's
    // refusal, for their size.
    let core = silico_core::document::json::MAX_INPUT_LEN;
    let arguments = format!(r#"{{"of": "{}"}}"#, "x".repeat(core - 10 + 1));
    assert_eq!(arguments.len(), core + 1);
    let refused = c.envelope(8, "schema", &arguments);
    assert_eq!(
        refused["errors"][0]["message"],
        format!(
            "the request is too large for Silico to read: the text is {} bytes, more than the {core} Silico reads",
            core + 1
        )
    );
}

#[test]
fn garbage_gets_an_error_and_the_server_reads_on() {
    let mut c = Client::initialized();
    let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    let garbage: Vec<(Vec<u8>, i64)> = vec![
        (b"{".to_vec(), -32700),
        (b"}".to_vec(), -32700),
        (b"\x00\x00\x00".to_vec(), -32700),
        (b"\xff\xfe{}".to_vec(), -32700),
        (b"\xef\xbb\xbf{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"ping\"}".to_vec(), -32700),
        (deep.into_bytes(), -32600),
        (b"[1, 2, 3]".to_vec(), -32600),
        (b"\"just a string\"".to_vec(), -32600),
        (b"true".to_vec(), -32600),
        (b"{}".to_vec(), -32600),
        (br#"{"jsonrpc": "2.0"}"#.to_vec(), -32600),
        (br#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "open", "arguments": {"path": "../../etc/passwd", "create": false}}}"#.to_vec(), 0),
    ];
    for (line, code) in garbage {
        c.send_bytes(&line);
        let reply = c.message();
        if code == 0 {
            // A tool the core refuses: a path out of the workspace.
            assert_eq!(reply["result"]["isError"], true, "{reply}");
            assert_eq!(
                reply["result"]["structuredContent"]["errors"][0]["parameter"],
                "path"
            );
        } else {
            assert_eq!(reply["error"]["code"], code, "{reply}");
        }
    }
    // Arguments nested far past the core's 64 reach it, and it refuses them as it reads them.
    let deep = format!(r#"{{"of": {}{}}}"#, "[".repeat(10_000), "]".repeat(10_000));
    let refused = c.envelope(3, "schema", &deep);
    assert_eq!(
        refused["errors"][0]["message"],
        "the request isn't valid JSON: arrays and objects nest more than 64 deep, at line 1, column 71"
    );
    // Blank lines are skipped, unanswered.
    c.send("");
    c.send("   \t");
    c.send(r#"{"jsonrpc": "2.0", "id": 2, "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":2,"result":{}}"#);
}

#[test]
fn calls_that_wait_on_nothing_sent_just_before_stdins_end_are_answered_in_order() {
    // From the review (round 2): a `version` queued when stdin ended was cancelled with the
    // calls in flight, and so answered or not by timing, while a `ping` after it always was.
    // It's answered as it's read, its reply queued with the rest.
    for _ in 0..20 {
        let mut c = Client::initialized();
        c.send_call(1, "version", "{}");
        c.send_call(2, "status", r#"{"session": "s9"}"#);
        c.send(r#"{"jsonrpc": "2.0", "id": 3, "method": "ping"}"#);
        c.close_input();
        let ids: Vec<u64> = (0..3)
            .map(|_| c.message()["id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, [1, 2, 3]);
        c.ended();
    }
}

#[test]
fn slot_free_answers_queued_when_stdins_end_is_read_are_written_and_the_call_in_flight_never() {
    // The note (*A client that goes away*): on stdin's end "the replies queued" are still
    // written, and every call in flight is cancelled, so never answered. `status`, `cancel`,
    // `read_logs`, `version`, and `schema` take no slot: each is answered on serve's thread
    // as it's read, its reply queued for the replies' thread. Here each is read and queued
    // behind a pong the client isn't reading, stdin's end is read after them, and only then
    // is the output let go: the order is the protocol's, never the clock's.
    let (mut c, hold) = mid_call();
    c.stall_output();
    c.send(r#"{"jsonrpc": "2.0", "id": 10, "method": "ping"}"#);
    c.wait_stalled();
    c.send_call(11, "version", "{}");
    c.send_call(12, "status", r#"{"session": "s1"}"#);
    c.send_call(13, "schema", r#"{"of": "close"}"#);
    c.send_call(
        14,
        "read_logs",
        r#"{"session": "s1", "level": "error", "feature": null, "after": null, "limit": 10}"#,
    );
    c.send_call(15, "cancel", r#"{"session": "s9", "command": "check"}"#);
    c.close_input();
    // Stdin's end was read, after every line before it: the check in flight (4) was
    // cancelled, so the rebuild it waits on stopped, its hold still held. Only that end
    // cancels it.
    assert!(c.kernel.wait_idle(HANG), "stdin's end was never read");
    // And serve waits for the replies queued, the client still not reading.
    assert!(
        !c.has_ended(),
        "serve ended with replies queued and unwritten"
    );
    c.release_output();
    assert_eq!(c.ended(), Ended::Input);
    // Every one written before serve returned, once, in the order read; the check never.
    let mut written = Vec::new();
    while let Some(line) = c.line_within(std::time::Duration::ZERO) {
        written.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    let ids: Vec<u64> = written.iter().map(|m| m["id"].as_u64().unwrap()).collect();
    assert_eq!(ids, [10, 11, 12, 13, 14, 15], "{written:?}");
    assert_eq!(
        written[0],
        json!({"jsonrpc": "2.0", "id": 10, "result": {}})
    );
    // Each a tool's answer (a refusal is one too, never dropped): `version`, `status`, and
    // `schema` answered as asked, and the cancel of a session that isn't open refused.
    // `read_logs` is answered either way: refused here, where the process's tracing keeps
    // no session's events.
    let refused: Vec<Option<bool>> = written[1..]
        .iter()
        .map(|m| m["result"]["isError"].as_bool())
        .collect();
    assert_eq!(refused[..3], [Some(false); 3], "{written:?}");
    assert!(refused[3].is_some(), "{written:?}");
    assert_eq!(refused[4], Some(true), "{written:?}");
    // And the transaction the check's session held, rolled back.
    assert_eq!(c.read_file("part.silico"), BOX);
    drop(hold);
}
