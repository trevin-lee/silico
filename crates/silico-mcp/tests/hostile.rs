//! A client that misbehaves (docs/design/m4.9-mcp.md, *Running a call*): one that stops
//! reading its output while it sends, one that calls and cancels against a hung kernel
//! request. The server neither hangs nor grows without bound: the reader waits once its
//! queue of replies is full, `Stop::stop` still ends it, and calls past the bound are
//! refused. Expected values: the note's rules, and the counts sent here.

use std::io::Write;

use silico_mcp::{Ended, MAX_IN_FLIGHT, MAX_QUEUED_REPLIES};

use crate::client::{BOX, Client, is_box, running};

#[test]
fn a_client_that_stops_reading_holds_the_reader_and_stop_still_ends_serve() {
    let mut c = Client::initialized();
    c.stall_output();
    // Far more pings than the replies queued (and more bytes than a pipe holds), from a
    // thread of their own, which blocks once the reader does: the reader waits on the
    // queue, the queue on the output.
    let mut input = c.take_input();
    let pings = 100 * MAX_QUEUED_REPLIES as u64;
    std::thread::spawn(move || {
        for id in 0..pings {
            let ping = format!("{{\"jsonrpc\": \"2.0\", \"id\": {id}, \"method\": \"ping\"}}\n");
            if input.write_all(ping.as_bytes()).is_err() {
                return;
            }
        }
    });
    c.wait_stalled();
    c.stop.stop();
    assert_eq!(c.ended(), Ended::Stopped);
}

#[test]
fn calls_cancelled_as_they_come_against_a_hung_request_are_bounded_and_settle() {
    let mut c = Client::initialized();
    c.file("part.silico", BOX);
    c.open(1, "part.silico");
    let hold = c.kernel.hold(is_box);
    c.send_call(
        2,
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    running(&c.kernel, 1);
    // Four times the bound, each cancelled at once: a cancelled call keeps its slot until
    // the core answers it, so those past the bound are refused, never left to grow.
    let sent = 4 * MAX_IN_FLIGHT as u64;
    for id in 100..100 + sent {
        c.send_call(
            id,
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        );
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {{"requestId": {id}}}}}"#
        ));
    }
    c.send(r#"{"jsonrpc": "2.0", "id": 9, "method": "ping"}"#);
    let (_, before) = c.reply_line("9");
    // Every reply before the pong is a refusal at the bound: no cancelled call answered.
    for reply in &before {
        let id = reply["id"].as_u64().unwrap();
        assert!((100..100 + sent).contains(&id), "{reply}");
        assert_eq!(
            reply["result"]["structuredContent"]["errors"][0]["message"],
            "64 calls are already running in this server, the most it runs at once"
        );
    }
    // The session's thread is held on check 2, so no cancelled call behind it is answered
    // yet, and none gives its slot back: 63 took the slots left, and the rest were refused.
    assert_eq!(before.len() as u64, sent - (MAX_IN_FLIGHT as u64 - 1));
    // The hung request let go, everything settles: the check held answers, and a call
    // after runs.
    hold.release();
    let (line, _) = c.reply_line("2");
    assert!(line.contains(r#""isError":false"#), "{line}");
    // The cancelled calls' slots come free as the core answers each: called until one
    // isn't refused at the bound, which a call can't be once they have.
    let deadline = std::time::Instant::now() + crate::client::HANG;
    for id in 1000.. {
        let outline = c.call(id, "outline", r#"{"session": "s1"}"#);
        if outline["isError"] == false {
            break;
        }
        assert_eq!(
            outline["structuredContent"]["errors"][0]["message"],
            "64 calls are already running in this server, the most it runs at once"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the slots never came free"
        );
    }
}

#[test]
fn replies_past_the_queue_while_the_client_isnt_reading_wait_for_room_and_none_is_dropped() {
    // Found by the M4 milestone's mutation run: dropping a reply that finds the queue full,
    // as if the server were ending, passed every test. JSON-RPC 2.0 (section 4): every
    // request is answered; the note's backpressure: a reader with a full queue of replies
    // waits for room, never drops one. One pong held in the stalled write, 64 queued behind
    // it, and as many again past them: once the client reads, every pong is written, once,
    // in the order asked (MCP's ping: an empty result). Whenever the client reads again,
    // every pong is written; reading again only after the reader has filled the queue (it
    // reads each ping in microseconds) is what tells a dropped one.
    let mut c = Client::initialized();
    c.stall_output();
    let pings = 2 * MAX_QUEUED_REPLIES as u64 + 2;
    for id in 10..10 + pings {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "ping"}}"#
        ));
    }
    c.wait_stalled();
    c.close_input();
    let release = c.releaser();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        release();
    });
    assert_eq!(c.ended(), Ended::Input);
    let mut written = Vec::new();
    while let Some(line) = c.line_within(std::time::Duration::ZERO) {
        written.push(line);
    }
    let pongs: Vec<String> = (10..10 + pings)
        .map(|id| format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#))
        .collect();
    assert_eq!(written.len(), pongs.len(), "pongs written");
    assert_eq!(written, pongs);
}
