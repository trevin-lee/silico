//! `/break-it` on the MCP server (docs/design/m4.9-mcp.md; M4.1 sections 8–9, decisions 7,
//! 10, 23): lines no client should send (a carriage return at the most read, a lone one,
//! NULs), IDs and tokens of every JSON type, a member given twice, cancellations of every
//! kind and in storms, ten thousand pings, a client that stops reading while it cancels, a
//! stop mid-commit, and a document that's a pipe. Each gets a structured answer, or none
//! where JSON-RPC and MCP say none; the server never crashes or hangs, and every line it
//! writes is one JSON-RPC message whose `structuredContent` is its text block's text byte
//! for byte and whose `isError` is its envelope's `!ok`. Expected values: JSON-RPC 2.0's and
//! MCP 2025-06-18's rules, RFC 8259's grammar, the note's rules, and the files as written
//! here. Timing comes from the fake kernel's holds, never the clock.

use std::io::Write;

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use silico_mcp::{Ended, MAX_LINE};

use crate::client::{BOX, Client, HANG, is_box, running};

#[derive(Deserialize)]
struct Answer<'a> {
    content: Vec<Block>,
    #[serde(borrow, rename = "structuredContent")]
    structured: &'a RawValue,
    #[serde(rename = "isError")]
    is_error: bool,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

/// `line`, checked to be one JSON-RPC 2.0 message as MCP has the server write them: a
/// progress notification, a result, or an error with a refused envelope as its data; a
/// tool's answer's `structuredContent` its text block's text byte for byte, and its
/// `isError` the envelope's `!ok`. The message, parsed.
fn checked(line: &str) -> Value {
    assert!(!line.contains('\r'), "a carriage return in {line:?}");
    let message: Value =
        serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"));
    assert_eq!(message["jsonrpc"], "2.0", "{line}");
    if let Some(method) = message.get("method") {
        assert_eq!(method, "notifications/progress", "{line}");
        assert!(message.get("id").is_none(), "{line}");
        let params = &message["params"];
        assert!(
            params["progressToken"].is_string() || params["progressToken"].is_number(),
            "{line}"
        );
        assert!(params["progress"].is_u64(), "{line}");
        return message;
    }
    let id = &message["id"];
    assert!(id.is_string() || id.is_number() || id.is_null(), "{line}");
    match (message.get("result"), message.get("error")) {
        (Some(result), None) => {
            if result.get("structuredContent").is_some() {
                #[derive(Deserialize)]
                struct Reply<'a> {
                    #[serde(borrow)]
                    result: Answer<'a>,
                }
                let reply: Reply<'_> = serde_json::from_str(line).unwrap();
                let answer = reply.result;
                assert!(!answer.content.is_empty(), "{line}");
                assert_eq!(answer.content[0].kind, "text", "{line}");
                assert_eq!(
                    answer.content[0].text.as_deref(),
                    Some(answer.structured.get()),
                    "{line}"
                );
                for block in &answer.content[1..] {
                    assert_eq!(block.kind, "image", "{line}");
                }
                let envelope: Value = serde_json::from_str(answer.structured.get()).unwrap();
                assert_eq!(envelope["ok"], json!(!answer.is_error), "{line}");
            }
        }
        (None, Some(error)) => {
            assert!(error["code"].is_i64(), "{line}");
            assert!(error["message"].is_string(), "{line}");
            assert_eq!(error["data"]["ok"], false, "{line}");
            assert!(error["data"]["total"].as_u64().unwrap() >= 1, "{line}");
        }
        _ => panic!("neither a result nor an error, or both: {line}"),
    }
    message
}

/// The next line, checked.
fn next(c: &mut Client) -> Value {
    let line = c.line();
    checked(&line)
}

/// The next line, checked to be the pong of `id` (JSON text): so nothing was written
/// before it.
fn pong(c: &mut Client, id: &str) {
    c.send(&format!(
        r#"{{"jsonrpc": "2.0", "id": {id}, "method": "ping"}}"#
    ));
    assert_eq!(
        c.line(),
        format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#)
    );
}

/// A JSON-RPC error's code, ID, and its data's first error.
fn rpc_error(message: &Value) -> (i64, Value, Value) {
    assert!(message.get("result").is_none(), "{message}");
    (
        message["error"]["code"].as_i64().unwrap(),
        message["id"].clone(),
        message["error"]["data"]["errors"][0].clone(),
    )
}

const CHECK: &str = r#"{"session": "s1", "model": "current", "fresh": false}"#;

fn opened() -> Client {
    let mut c = Client::initialized();
    c.file("part.silico", BOX);
    c.open(1, "part.silico");
    c
}

/// A ping of `id` padded to exactly `length` bytes, in a `params` the server ignores.
fn ping_of_length(id: u64, length: usize) -> String {
    let head = format!(r#"{{"jsonrpc": "2.0", "id": {id}, "method": "ping", "params": {{"pad": ""#);
    let tail = r#""}}"#;
    let line = format!(
        "{head}{}{tail}",
        "x".repeat(length - head.len() - tail.len())
    );
    assert_eq!(line.len(), length);
    line
}

#[test]
fn a_line_exactly_at_the_most_read_is_read_whole_with_or_without_a_carriage_return() {
    let mut c = Client::initialized();
    // Exactly the most, ended by a newline: read whole.
    c.send(&ping_of_length(1, MAX_LINE));
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
    // The same, ended by CRLF, as some clients end lines: the carriage return isn't the
    // message's, so it's read whole too.
    // Broken once (/break-it): the carriage return was counted in the line's length, so a
    // message of exactly MAX_LINE bytes sent with CRLF was refused as one byte too long
    // (-32600), while the same bytes with LF were read.
    let mut line = ping_of_length(2, MAX_LINE).into_bytes();
    line.push(b'\r');
    c.send_bytes(&line);
    let reply = next(&mut c);
    assert_eq!(reply, json!({"jsonrpc": "2.0", "id": 2, "result": {}}));
    // One past the most, with CRLF: refused for its own length, not its carriage return's.
    let mut line = ping_of_length(3, MAX_LINE + 1).into_bytes();
    line.push(b'\r');
    c.send_bytes(&line);
    let (code, id, error) = rpc_error(&next(&mut c));
    assert_eq!((code, id), (-32600, json!(3)));
    assert_eq!(
        error["message"],
        format!(
            "the message is {} bytes, more than the {MAX_LINE} a message may be",
            MAX_LINE + 1
        )
    );
    pong(&mut c, "4");
}

#[test]
fn a_lone_carriage_return_or_a_nul_in_a_line_is_a_parse_error_and_the_server_reads_on() {
    let mut c = Client::initialized();
    // Two messages split by a lone CR are one line, not two: JSON with trailing characters.
    c.send(
        "{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"ping\"}\r{\"jsonrpc\": \"2.0\", \"id\": 2, \"method\": \"ping\"}",
    );
    let (code, id, error) = rpc_error(&next(&mut c));
    assert_eq!((code, id), (-32700, Value::Null));
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("trailing characters"),
        "{error}"
    );
    // A line of carriage returns alone is blank: unanswered.
    c.send("\r");
    c.send("\r\r");
    // A raw NUL in a string isn't JSON (RFC 8259: control characters are escaped).
    c.send_bytes(b"{\"jsonrpc\": \"2.0\", \"id\": 3, \"method\": \"pi\x00ng\"}");
    let (code, id, _) = rpc_error(&next(&mut c));
    assert_eq!((code, id), (-32700, Value::Null));
    // An escaped one is a method's name like any other, quoted as the core quotes.
    c.send(r#"{"jsonrpc": "2.0", "id": 4, "method": "ping\u0000"}"#);
    let (code, id, error) = rpc_error(&next(&mut c));
    assert_eq!((code, id), (-32601, json!(4)));
    assert_eq!(error["message"], "no method is named \"ping\\u{0}\"");
    // A tab or form feed before the message: JSON's whitespace is space, tab, CR, LF.
    c.send("\t{\"jsonrpc\": \"2.0\", \"id\": 5, \"method\": \"ping\"}");
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":5,"result":{}}"#);
    c.send_bytes(b"\x0c{\"jsonrpc\": \"2.0\", \"id\": 6, \"method\": \"ping\"}");
    let (code, id, _) = rpc_error(&next(&mut c));
    assert_eq!((code, id), (-32700, Value::Null));
    pong(&mut c, "7");
}

#[test]
fn ids_of_every_size_and_spelling_are_carried_back_as_written() {
    let mut c = Client::initialized();
    let long = format!("\"{}\"", "i".repeat(1 << 20));
    for id in [
        "123456789012345678901234567890123456789",
        "-123456789012345678901234567890123456789",
        "1e400",
        "-1.5e-300",
        "0",
        r#""é\n\"""#,
        r#""é""#,
        long.as_str(),
    ] {
        pong(&mut c, id);
    }
}

#[test]
fn an_id_holding_a_lone_surrogate_is_carried_back_as_written() {
    let mut c = Client::initialized();
    // RFC 8259's grammar allows any \uXXXX escape in a string, a lone surrogate included
    // (section 8.2: what software does with one is unpredictable); the server needn't
    // decode an ID, only carry it back, as it does every other.
    // Broken once (/break-it): refused as -32600 "the message's id is neither a string nor
    // a number", with a null ID, though it's a string: serde_json won't decode it.
    c.send(r#"{"jsonrpc": "2.0", "id": "\ud800", "method": "ping"}"#);
    let reply = c.line();
    assert_eq!(reply, r#"{"jsonrpc":"2.0","id":"\ud800","result":{}}"#);
    // And distinct from another lone surrogate, and from the same text decoded otherwise.
    c.send(r#"{"jsonrpc": "2.0", "id": "\udc00", "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":"\udc00","result":{}}"#);
    // As a progress token, a string too.
    let version = c.call(2, "version", r#"{}, "_meta": {"progressToken": "\ud800"}"#);
    assert_eq!(version["isError"], false, "{version}");
}

#[test]
fn a_request_with_a_member_given_twice_is_refused_by_its_id_when_that_is_given_once() {
    let mut c = Client::initialized();
    // JSON-RPC: the ID is null only if it couldn't be found. Here it's given once, so a
    // client waiting on it would wait forever for an answer under a null ID.
    // Broken once (/break-it): every frame refused for a member given twice was answered
    // with a null ID, though the ID was read, so request 5 was never answered by its ID.
    for (line, id, member) in [
        (
            r#"{"jsonrpc": "2.0", "id": 5, "method": "ping", "params": {}, "params": {}}"#,
            json!(5),
            "params",
        ),
        (
            r#"{"jsonrpc": "2.0", "jsonrpc": "2.0", "id": "a", "method": "ping"}"#,
            json!("a"),
            "jsonrpc",
        ),
        (
            r#"{"jsonrpc": "2.0", "id": 6, "method": "tools/call", "method": "ping"}"#,
            json!(6),
            "method",
        ),
        // Two IDs: which to answer by can't be told, so none (null).
        (
            r#"{"jsonrpc": "2.0", "id": 7, "id": 7, "method": "ping"}"#,
            Value::Null,
            "id",
        ),
    ] {
        c.send(line);
        let (code, got, error) = rpc_error(&next(&mut c));
        assert_eq!((code, &got), (-32600, &id), "{line}");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains(&format!("duplicate field `{member}`")),
            "{line}: {error}"
        );
    }
    pong(&mut c, "8");
}

#[test]
fn every_refusal_of_a_messages_shape_says_what_to_do() {
    let mut c = Client::initialized();
    // CLAUDE.md: every error carries a suggestion an agent can act on.
    // Broken once (/break-it): tools/call's params that aren't an object, a _meta that
    // isn't, and a progress token that's neither a string nor a number were refused with
    // no suggestion.
    for (line, parameter) in [
        (
            r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": [1]}"#,
            Value::from("params"),
        ),
        (
            r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": null}"#,
            json!("params"),
        ),
        (
            r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "version", "_meta": null}}"#,
            json!("_meta"),
        ),
        (
            r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "version", "_meta": {"progressToken": 1, "progressToken": 2}}}"#,
            json!("_meta"),
        ),
        (
            r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "version", "_meta": {"progressToken": true}}}"#,
            json!("_meta.progressToken"),
        ),
        (
            r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": true}}"#,
            json!("name"),
        ),
        (
            r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "version", "arguments": 3}}"#,
            json!("arguments"),
        ),
        (
            r#"{"jsonrpc": "2.0", "id": 1, "method": "nope"}"#,
            json!("method"),
        ),
        (
            r#"{"jsonrpc": "1.0", "id": 1, "method": "ping"}"#,
            Value::Null,
        ),
        (r#"{"#, Value::Null),
    ] {
        c.send(line);
        let (_, _, error) = rpc_error(&next(&mut c));
        assert_eq!(error["parameter"], parameter, "{line}");
        let suggestion = error["suggestion"].as_str().unwrap_or_default();
        assert!(!suggestion.is_empty(), "{line}: no suggestion in {error}");
    }
}

#[test]
fn progress_tokens_of_every_json_type_are_taken_if_a_string_or_a_number() {
    let mut c = Client::initialized();
    let mut id = 10;
    for token in [
        r#""""#,
        r#""tok""#,
        "0",
        "-1",
        "2.5",
        "1e3",
        "123456789012345678901234567890",
    ] {
        id += 1;
        let answer = c.call(
            id,
            "version",
            &format!(r#"{{}}, "_meta": {{"progressToken": {token}}}"#),
        );
        assert_eq!(answer["isError"], false, "{token}: {answer}");
    }
    for token in ["null", "true", "false", "[]", "{}", r#"{"a": 1}"#] {
        id += 1;
        let reply = c.request(
            id,
            "tools/call",
            &format!(r#"{{"name": "version", "_meta": {{"progressToken": {token}}}}}"#),
        );
        checked(&reply.to_string());
        let (code, _, error) = rpc_error(&reply);
        assert_eq!(code, -32602, "{token}");
        assert_eq!(error["parameter"], "_meta.progressToken", "{token}");
    }
    // A `_meta` of every other type is refused naming it; one with no token gets none.
    for meta in ["null", "1", r#""x""#, "[]"] {
        id += 1;
        let reply = c.request(
            id,
            "tools/call",
            &format!(r#"{{"name": "version", "_meta": {meta}}}"#),
        );
        let (code, _, error) = rpc_error(&reply);
        assert_eq!(
            (code, error["parameter"].clone()),
            (-32602, json!("_meta")),
            "{meta}"
        );
    }
    let answer = c.call(99, "version", r#"{}, "_meta": {"other": [1, 2]}"#);
    assert_eq!(answer["isError"], false);
}

#[test]
fn progress_on_a_call_that_answers_at_once_sends_none() {
    let mut c = opened();
    for (id, name, arguments) in [
        (2, "version", "{}"),
        (3, "schema", r#"{"of": "close"}"#),
        (4, "outline", r#"{"session": "s1"}"#),
        (5, "close", r#"{"session": "nobody"}"#),
    ] {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "tools/call", "params": {{"name": "{name}", "arguments": {arguments}, "_meta": {{"progressToken": "p{id}"}}}}}}"#
        ));
        // The answer is the next line: no progress came before it.
        let reply = next(&mut c);
        assert_eq!(reply["id"], id, "{reply}");
        assert!(reply["result"]["structuredContent"].is_object(), "{reply}");
    }
}

#[test]
fn tools_calls_name_of_every_json_type_is_refused_but_a_string_however_escaped() {
    let mut c = Client::initialized();
    for name in [
        "true",
        "false",
        "1.5",
        "[]",
        r#"["version"]"#,
        "{}",
        r#"{"name": "version"}"#,
    ] {
        let reply = c.request(1, "tools/call", &format!(r#"{{"name": {name}}}"#));
        let (code, id, error) = rpc_error(&reply);
        assert_eq!((code, id), (-32602, json!(1)), "{name}");
        assert_eq!(error["message"], "name must be a string", "{name}");
        assert_eq!(error["parameter"], "name");
    }
    // Escaped, the same name.
    let answer = c.call(2, "v\\u0065rsion", "{}");
    assert_eq!(answer["isError"], false, "{answer}");
    // A NUL after it: another name, quoted escaped.
    let reply = c.request(3, "tools/call", r#"{"name": "version\u0000"}"#);
    let (code, _, error) = rpc_error(&reply);
    assert_eq!(code, -32602);
    assert_eq!(error["message"], "no tool is named \"version\\u{0}\"");
}

#[test]
fn initialize_with_params_of_every_type_is_answered_as_the_first_was() {
    let mut c = Client::start();
    let first = {
        c.send(r#"{"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}"#);
        let (line, _) = c.reply_line("0");
        line.replacen(r#""id":0"#, r#""id":ID"#, 1)
    };
    for (id, params) in [
        (1, r#"{"protocolVersion": "1999-01-01"}"#),
        (2, r#"{"protocolVersion": 20250618}"#),
        (3, r#"{"protocolVersion": null, "capabilities": "all"}"#),
        (4, "[]"),
        (5, r#""x""#),
        (6, "null"),
    ] {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "initialize", "params": {params}}}"#
        ));
        let (line, _) = c.reply_line(&id.to_string());
        assert_eq!(
            line,
            first.replacen(r#""id":ID"#, &format!(r#""id":{id}"#), 1),
            "{params}"
        );
    }
}

#[test]
fn cancellations_of_nothing_in_every_shape_and_in_storms_are_ignored() {
    let mut c = Client::initialized();
    for params in [
        "",
        r#", "params": null"#,
        r#", "params": []"#,
        r#", "params": [1]"#,
        r#", "params": "1""#,
        r#", "params": {}"#,
        r#", "params": {"requestId": null}"#,
        r#", "params": {"requestId": true}"#,
        r#", "params": {"requestId": {"id": 1}}"#,
        r#", "params": {"requestId": [1]}"#,
        r#", "params": {"requestId": 1e999}"#,
        r#", "params": {"requestId": "\ud800"}"#,
        r#", "params": {"requestId": 1, "requestId": 2}"#,
        r#", "params": {"requestId": 0}"#,
    ] {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "method": "notifications/cancelled"{params}}}"#
        ));
    }
    // None of them answered: the pong is the next line.
    pong(&mut c, "1");
    // A storm of them, every ID new, from a thread of its own, then a ping.
    let mut input = c.take_input();
    let writer = std::thread::spawn(move || {
        for id in 0..5_000u64 {
            writeln!(
                input,
                r#"{{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {{"requestId": {id}, "reason": "storm"}}}}"#
            )
            .unwrap();
        }
        writeln!(input, r#"{{"jsonrpc": "2.0", "id": 2, "method": "ping"}}"#).unwrap();
        input
    });
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":2,"result":{}}"#);
    drop(writer.join().unwrap());
    assert_eq!(c.ended(), Ended::Input);
}

#[test]
fn a_cancel_that_comes_before_its_call_or_after_its_answer_changes_nothing() {
    let mut c = opened();
    // Before: the call that comes after it runs and answers.
    c.send(
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 2}}"#,
    );
    let outline = c.call(2, "outline", r#"{"session": "s1"}"#);
    assert_eq!(outline["isError"], false, "{outline}");
    // After its answer: ignored, and the ID free again at once.
    c.send(
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 2}}"#,
    );
    let again = c.call(2, "outline", r#"{"session": "s1"}"#);
    assert_eq!(again, outline);
}

#[test]
fn an_id_given_again_right_after_its_cancel_is_answered_once_by_the_new_call() {
    let mut c = opened();
    let hold = c.kernel.hold(is_box);
    c.send_call(2, "check", CHECK);
    running(&c.kernel, 1);
    c.send(
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 2}}"#,
    );
    // The same ID at once, for a call of another command, and again for the first's.
    c.send_call(2, "outline", r#"{"session": "s1"}"#);
    let (line, before) = c.reply_line("2");
    assert!(before.is_empty(), "{before:?}");
    let reply = checked(&line);
    assert_eq!(reply["result"]["structuredContent"]["ok"], true, "{line}");
    assert!(
        reply["result"]["structuredContent"]["result"]
            .get("outline")
            .is_some(),
        "the outline's answer, not the check's: {line}"
    );
    c.send_call(2, "check", CHECK);
    hold.release();
    let (line, before) = c.reply_line("2");
    assert!(before.is_empty(), "{before:?}");
    let reply = checked(&line);
    assert_eq!(reply["result"]["isError"], false, "{line}");
    // Nothing more under 2: the cancelled check's answer never comes.
    pong(&mut c, "3");
    assert_eq!(c.line_within(std::time::Duration::ZERO), None);
}

#[test]
fn ten_thousand_pings_are_each_answered_once_and_in_order() {
    let mut c = Client::initialized();
    let mut input = c.take_input();
    let count = 10_000u64;
    let writer = std::thread::spawn(move || {
        for id in 0..count {
            writeln!(input, r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#).unwrap();
        }
        input
    });
    for id in 0..count {
        assert_eq!(
            c.line(),
            format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#)
        );
    }
    drop(writer.join().unwrap());
    assert_eq!(c.ended(), Ended::Input);
    assert_eq!(c.line_within(std::time::Duration::ZERO), None);
}

#[test]
fn a_cancel_is_read_while_the_client_has_stopped_reading() {
    let mut c = opened();
    let hold = c.kernel.hold(is_box);
    c.send_call(2, "check", CHECK);
    running(&c.kernel, 1);
    // The client stops reading: the next reply blocks the writer.
    c.stall_output();
    c.send(r#"{"jsonrpc": "2.0", "id": 3, "method": "ping"}"#);
    c.wait_stalled();
    // Fewer pings than the replies queued, then the cancel: the reader isn't held, so the
    // cancel is read, and the held rebuild stops though its hold is never released.
    for id in 10..20 {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "ping"}}"#
        ));
    }
    c.send(
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 2}}"#,
    );
    assert!(c.kernel.wait_idle(HANG), "the cancel wasn't read");
    c.stop.stop();
    assert_eq!(c.ended(), Ended::Stopped);
    drop(hold);
}

/// The names in the workspace but the cache's.
fn listed(c: &Client) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(&c.work)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != ".silico-cache")
        .collect();
    names.sort();
    names
}

#[test]
fn a_stop_mid_commit_leaves_the_document_as_it_was_and_no_partial_file() {
    let mut c = opened();
    assert_eq!(c.envelope(2, "begin", r#"{"session": "s1"}"#)["ok"], true);
    let set = c.envelope(
        3,
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
    );
    assert_eq!(set["ok"], true, "{set}");
    let hold = c.kernel.hold(is_box);
    // The commit waits on the edit's rebuild, held: it's saved nothing yet.
    c.send_call(4, "commit", r#"{"session": "s1"}"#);
    running(&c.kernel, 1);
    c.stop.stop();
    assert_eq!(c.ended(), Ended::Stopped);
    assert!(c.kernel.wait_idle(HANG));
    assert_eq!(c.read_file("part.silico"), BOX);
    assert_eq!(listed(&c), ["part.silico"]);
    while let Some(line) = c.line_within(std::time::Duration::ZERO) {
        assert!(!line.contains(r#""id":4"#), "the commit answered: {line}");
    }
    drop(hold);
}

#[test]
fn a_stop_racing_a_commit_leaves_the_document_whole_either_way() {
    let mut c = opened();
    assert_eq!(c.envelope(2, "begin", r#"{"session": "s1"}"#)["ok"], true);
    let set = c.envelope(
        3,
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
    );
    assert_eq!(set["ok"], true, "{set}");
    let hold = c.kernel.hold(is_box);
    c.send_call(4, "commit", r#"{"session": "s1"}"#);
    running(&c.kernel, 1);
    // Let go as the server's told to stop: whichever wins, the file is the old document or
    // the committed one, whole, and nothing is left beside it.
    let stop = c.stop.clone();
    let stopping = std::thread::spawn(move || stop.stop());
    hold.release();
    stopping.join().unwrap();
    assert_eq!(c.ended(), Ended::Stopped);
    assert!(c.kernel.wait_idle(HANG));
    let text = c.read_file("part.silico");
    if text != BOX {
        let document: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(document["features"][0]["length"], 41, "{text}");
    }
    assert_eq!(listed(&c), ["part.silico"]);
}

#[test]
fn a_document_that_is_a_pipe_is_refused_and_the_server_reads_on() {
    let mut c = Client::initialized();
    let made = std::process::Command::new("mkfifo")
        .arg(c.work.join("pipe.silico"))
        .status()
        .unwrap();
    assert!(made.success());
    c.send_call(1, "open", r#"{"path": "pipe.silico", "create": false}"#);
    let reply = next(&mut c);
    assert_eq!(reply["id"], 1);
    let envelope = &reply["result"]["structuredContent"];
    assert_eq!(envelope["ok"], false, "{reply}");
    assert!(
        envelope["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("isn't a regular file"),
        "{reply}"
    );
    pong(&mut c, "2");
}

#[test]
fn a_battery_of_garbage_and_calls_is_answered_one_json_rpc_line_each() {
    let mut c = opened();
    let lines: Vec<&[u8]> = vec![
        b"{\"jsonrpc\": \"2.0\", \"id\": 1}",
        b"\xff",
        b"[]",
        b"{\"jsonrpc\": \"2.0\", \"id\": 2, \"method\": \"tools/call\", \"params\": {\"name\": \"elements\", \"arguments\": {\"session\": \"s1\", \"model\": \"current\", \"feature\": \"k7f2qx3m\", \"after\": null, \"limit\": 5}}}",
        b"{\"jsonrpc\": \"2.0\", \"id\": 3, \"method\": \"tools/call\", \"params\": {\"name\": \"check\", \"arguments\": {\"session\": \"s1\", \"model\": \"published\", \"fresh\": true}}}",
        b"{\"jsonrpc\": \"2.0\", \"id\": 4, \"method\": \"tools/call\", \"params\": {\"name\": \"render\", \"arguments\": {\"session\": \"s1\"}}}",
        b"{\"jsonrpc\": \"2.0\", \"id\": 5, \"method\": \"tools/call\", \"params\": {\"name\": \"query\", \"arguments\": {\"session\": 5}}}",
        b"{\"jsonrpc\": \"2.0\", \"id\": 6, \"method\": \"tools/call\", \"params\": {\"name\": \"status\", \"arguments\": {\"session\": \"s1\"}}}",
        b"{\"jsonrpc\": \"2.0\", \"id\": 7, \"method\": \"tools/call\", \"params\": {\"name\": \"cancel\", \"arguments\": {\"session\": \"s9\", \"command\": \"check\"}}}",
        b"{\"jsonrpc\": \"2.0\", \"id\": 8, \"method\": \"resources/list\"}",
        b"{\"jsonrpc\": \"2.0\", \"id\": 9, \"method\": \"tools/list\", \"params\": [1]}",
    ];
    for line in &lines {
        c.send_bytes(line);
    }
    // Each line gets one answer (no call here asked for progress), in whatever order the
    // calls answer, each by its ID or, unread, a null one.
    let mut ids = Vec::new();
    for _ in &lines {
        ids.push(next(&mut c)["id"].clone());
    }
    ids.sort_by_key(Value::to_string);
    let mut expected: Vec<Value> = vec![json!(1), Value::Null, Value::Null];
    expected.extend((2..=9).map(Value::from));
    expected.sort_by_key(Value::to_string);
    assert_eq!(ids, expected);
    pong(&mut c, "10");
}
