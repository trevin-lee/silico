//! Errors on the wire (docs/design/m4.9-mcp.md, *Errors on the wire*), every row of its
//! table: JSON-RPC's codes, `id` null where none could be read, and `data` a refused
//! envelope in the core's contract (checked against the committed envelope schema), naming
//! the field at fault. Expected values: JSON-RPC 2.0's codes and the note's table, written
//! here by hand.

use serde_json::{Value, json};

use crate::client::rpc_error as parts;
use crate::client::{Client, is_box, running};
use crate::contract::{committed_core, holds};

/// The error a reply carries, failing unless it's one, with `code` and `id`, whose `data`
/// is a refused envelope of one error: that error.
fn refused_rpc_error(reply: &Value, code: i64, id: Value) -> Value {
    assert_eq!(reply["jsonrpc"], "2.0", "{reply}");
    let (was, with, _) = parts(reply);
    assert_eq!((was, with), (code, id), "{reply}");
    let error = &reply["error"];
    // The error's own message, JSON-RPC's generic one never.
    assert_eq!(
        error["message"], error["data"]["errors"][0]["message"],
        "{reply}"
    );
    let data = &error["data"];
    let envelope: Value = serde_json::from_str(&committed_core("commands/envelope.json")).unwrap();
    holds(&envelope, data, "the error's data");
    assert_eq!(data["ok"], false);
    assert_eq!(data["session"], Value::Null);
    assert_eq!(data["model"], Value::Null);
    assert_eq!(data["total"], 1, "{reply}");
    let error = data["errors"][0].clone();
    assert_eq!(error["code"], "invalid_input", "{reply}");
    error
}

#[test]
fn a_line_that_isnt_json_is_a_parse_error_with_a_null_id_saying_where() {
    let mut c = Client::initialized();
    for (line, said) in [
        (
            &b"{\"jsonrpc\": \"2.0\", \"id\": 1,"[..],
            "EOF while parsing an object at line 1 column 27",
        ),
        (b"not json", "expected ident at line 1 column 2"),
        (
            b"{\"id\": 1} trailing",
            "trailing characters at line 1 column 11",
        ),
        (b"{\"a\xff\": 1}", "isn't UTF-8: at byte 3"),
    ] {
        c.send_bytes(line);
        let error = refused_rpc_error(&c.message(), -32700, Value::Null);
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(said), "{message}");
        assert_eq!(error["parameter"], Value::Null);
    }
}

#[test]
fn json_that_isnt_a_request_or_a_notification_is_an_invalid_request() {
    let mut c = Client::initialized();
    for (line, id, said) in [
        (
            r#"[{"jsonrpc": "2.0", "id": 1, "method": "ping"}]"#,
            Value::Null,
            "a batch",
        ),
        ("[]", Value::Null, "a batch"),
        ("null", Value::Null, "isn't an object"),
        ("12", Value::Null, "isn't an object"),
        (
            r#"{"jsonrpc": "1.0", "id": 3, "method": "ping"}"#,
            json!(3),
            "jsonrpc isn't \"2.0\"",
        ),
        (
            r#"{"id": "x", "method": "ping"}"#,
            json!("x"),
            "jsonrpc isn't \"2.0\"",
        ),
        (
            r#"{"jsonrpc": 2.0, "id": 4, "method": "ping"}"#,
            json!(4),
            "jsonrpc isn't \"2.0\"",
        ),
        (
            r#"{"jsonrpc": "2.0", "id": null, "method": "ping"}"#,
            Value::Null,
            "neither a string nor a number",
        ),
        (
            r#"{"jsonrpc": "2.0", "id": true, "method": "ping"}"#,
            Value::Null,
            "neither a string nor a number",
        ),
        (
            r#"{"jsonrpc": "2.0", "id": {"a": 1}, "method": "ping"}"#,
            Value::Null,
            "neither a string nor a number",
        ),
        (r#"{"jsonrpc": "2.0", "id": 5}"#, json!(5), "has no method"),
        (
            r#"{"jsonrpc": "2.0", "id": 6, "method": 6}"#,
            json!(6),
            "method isn't a string",
        ),
        (
            r#"{"jsonrpc": "2.0", "id": 7, "id": 8, "method": "ping"}"#,
            Value::Null,
            "duplicate field `id`",
        ),
        (
            r#"{"jsonrpc": "2.0", "method": "ping", "method": "ping"}"#,
            Value::Null,
            "duplicate field `method`",
        ),
    ] {
        c.send(line);
        let error = refused_rpc_error(&c.message(), -32600, id);
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(said), "{line}: {message}");
    }
}

#[test]
fn a_tools_call_before_initialize_is_refused_and_after_it_runs() {
    let mut c = Client::start();
    let refused = c.request(1, "tools/call", r#"{"name": "version"}"#);
    let error = refused_rpc_error(&refused, -32600, json!(1));
    assert_eq!(error["message"], "tools/call came before initialize");
    // tools/list needs no initialize; the call does.
    let listed = c.request(2, "tools/list", "{}");
    assert_eq!(
        listed["result"]["tools"].as_array().unwrap().len(),
        TOOLS.len()
    );
    c.request(3, "initialize", "{}");
    let ran = c.request(4, "tools/call", r#"{"name": "version"}"#);
    assert_eq!(ran["result"]["isError"], false, "{ran}");
}

#[test]
fn an_unknown_method_is_method_not_found_naming_the_methods() {
    let mut c = Client::initialized();
    for method in [
        "resources/list",
        "prompts/list",
        "tools/Call",
        "notifications/cancelled",
    ] {
        let reply = c.request(1, method, "{}");
        let error = refused_rpc_error(&reply, -32601, json!(1));
        assert_eq!(error["message"], format!("no method is named \"{method}\""));
        assert_eq!(error["parameter"], "method");
        assert_eq!(
            error["candidates"],
            json!(["initialize", "ping", "tools/list", "tools/call"])
        );
    }
}

/// Every tool's name, in the list's order, written out by hand.
pub const TOOLS: [&str; 29] = [
    "version",
    "open",
    "close",
    "status",
    "cancel",
    "begin",
    "commit",
    "rollback",
    "format",
    "add_feature",
    "set_field",
    "remove_feature",
    "replace_text",
    "text",
    "outline",
    "check",
    "cache_clear",
    "elements",
    "query",
    "schema",
    "mass_properties",
    "distance",
    "angle",
    "render",
    "read_logs",
    "export",
    "move_feature",
    "timeline",
    "sketch",
];

#[test]
fn an_unknown_tool_or_arguments_that_arent_an_object_are_invalid_params_naming_the_field() {
    let mut c = Client::initialized();
    for (params, parameter, said) in [
        (
            r#"{"name": "Open", "arguments": {}}"#,
            "name",
            "no tool is named \"Open\"",
        ),
        (
            r#"{"name": "", "arguments": {}}"#,
            "name",
            "no tool is named \"\"",
        ),
        (r#"{"arguments": {}}"#, "name", "name is missing"),
        (r#"{"name": 1}"#, "name", "name must be a string"),
        (r#"{"name": null}"#, "name", "name must be a string"),
        (
            r#"{"name": "version", "arguments": []}"#,
            "arguments",
            "arguments must be an object, got \"[]\"",
        ),
        (
            r#"{"name": "version", "arguments": null}"#,
            "arguments",
            "arguments must be an object, got \"null\"",
        ),
        (
            r#"{"name": "version", "arguments": "{}"}"#,
            "arguments",
            "arguments must be an object",
        ),
        (
            r#"{"name": "version", "_meta": 1}"#,
            "_meta",
            "_meta must be an object",
        ),
        (
            r#"{"name": "version", "_meta": {"progressToken": null}}"#,
            "_meta.progressToken",
            "must be a string or a number",
        ),
        (
            r#"{"name": "version", "_meta": {"progressToken": [1]}}"#,
            "_meta.progressToken",
            "must be a string or a number",
        ),
        (
            r#"{"name": "version", "name": "open"}"#,
            "params",
            "duplicate field `name`",
        ),
        (r#"[1]"#, "params", "params must be an object"),
        (r#"["version"]"#, "params", "params must be an object"),
        (r#""version""#, "params", "params must be an object"),
        (
            r#"{"name": "version", "_meta": [7]}"#,
            "_meta",
            "_meta must be an object",
        ),
    ] {
        let reply = c.request(1, "tools/call", params);
        let error = refused_rpc_error(&reply, -32602, json!(1));
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(said), "{params}: {message}");
        assert_eq!(error["parameter"], parameter, "{params}");
        if parameter == "name" {
            assert_eq!(error["candidates"], json!(TOOLS), "{params}");
        }
    }
    // A tools/call with no params names no tool.
    c.send(r#"{"jsonrpc": "2.0", "id": 2, "method": "tools/call"}"#);
    let error = refused_rpc_error(&c.message(), -32602, json!(2));
    assert_eq!(error["message"], "name is missing");
    // A name too long to quote whole is quoted cut.
    let long = "x".repeat(10_000);
    let reply = c.request(3, "tools/call", &format!(r#"{{"name": "{long}"}}"#));
    let error = refused_rpc_error(&reply, -32602, json!(3));
    assert_eq!(
        error["message"],
        format!("no tool is named \"{}\"...", "x".repeat(200))
    );
}

#[test]
fn a_known_tool_whose_arguments_the_core_refuses_is_a_result_with_the_cores_refusal() {
    let mut c = Client::initialized();
    let result = c.call(1, "close", "{}");
    assert_eq!(result["isError"], true);
    assert_eq!(
        result["structuredContent"],
        json!({"ok": false, "session": null, "model": null, "errors": [{"code": "invalid_input", "message": "session is missing", "feature": null, "elements": [], "elements_total": 0, "parameter": "session", "cause": null, "suggestion": "give every field the request's shape has; null where it allows none", "candidates": []}], "total": 1})
    );
}

#[test]
fn a_request_id_in_flight_is_refused_and_free_once_answered() {
    let mut c = Client::initialized();
    c.file("part.silico", crate::client::BOX);
    c.open(1, "part.silico");
    let hold = c.kernel.hold(is_box);
    c.send_call(
        2,
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    running(&c.kernel, 1);
    // Any request, by the ID in flight.
    for line in [
        r#"{"jsonrpc": "2.0", "id": 2, "method": "ping"}"#,
        r#"{"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "version"}}"#,
    ] {
        c.send(line);
        let error = refused_rpc_error(&c.message(), -32600, json!(2));
        assert_eq!(
            error["message"],
            "the ID \"2\" is a request's still in flight"
        );
    }
    // The same number written otherwise, and the string "2", are other IDs.
    c.send(r#"{"jsonrpc": "2.0", "id": "2", "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":"2","result":{}}"#);
    // A string in flight is the same ID however it's escaped: refused by its own spelling.
    c.send_call_as(
        r#""ab""#,
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    for spelling in [r#""\u0061b""#, r#""a\u0062""#, r#""ab""#] {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {spelling}, "method": "ping"}}"#
        ));
        let error = refused_rpc_error(&c.message(), -32600, json!("ab"));
        let message = error["message"].as_str().unwrap();
        assert!(message.starts_with("the ID "), "{spelling}: {message}");
        assert!(
            message.ends_with(" is a request's still in flight"),
            "{spelling}: {message}"
        );
    }
    hold.release();
    // Each answered once.
    let mut answered = Vec::new();
    while answered.len() < 2 {
        let reply = c.message();
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        answered.push(reply["id"].clone());
    }
    answered.sort_by_key(|id| id.to_string());
    assert_eq!(answered, [json!("ab"), json!(2)]);
    // Answered, its ID is free.
    c.send(r#"{"jsonrpc": "2.0", "id": 2, "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":2,"result":{}}"#);
}
