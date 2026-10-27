//! The handshake (docs/design/m4.9-mcp.md, *The protocol*): `initialize` answered with
//! MCP 2025-06-18 whatever the client asks, tools only, `serverInfo` and instructions;
//! `ping`; notifications never answered; IDs carried back as written. Expected values: the
//! note's and MCP 2025-06-18's shapes, written here by hand.

use serde_json::json;

use crate::client::Client;

#[test]
fn initialize_answers_with_the_pinned_revision_whatever_the_client_asks() {
    let mut c = Client::start();
    c.send(r#"{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {"sampling": {}}, "clientInfo": {"name": "old", "version": "0"}}}"#);
    let (line, _) = c.reply_line("1");
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    let result = &reply["result"];
    assert_eq!(reply["jsonrpc"], "2.0");
    assert_eq!(reply["id"], 1);
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(
        result["capabilities"],
        json!({"tools": {"listChanged": false}})
    );
    assert_eq!(
        result["serverInfo"],
        json!({"name": "silico", "title": "Silico", "version": env!("CARGO_PKG_VERSION")})
    );
    // In MCP's order, as written: read from the line's text, since a `Value`'s keys come
    // out in an order set by serde_json's features, not by what was sent.
    let at = |key: &str| line.find(&format!("\"{key}\":")).unwrap();
    assert!(
        at("protocolVersion") < at("capabilities")
            && at("capabilities") < at("serverInfo")
            && at("serverInfo") < at("instructions"),
        "{line}"
    );
    // The workspace, as relative paths resolve against it, and the three things an agent
    // most needs.
    let instructions = result["instructions"].as_str().unwrap();
    let root = c.work.display().to_string();
    for said in [
        format!("The workspace is {root}:"),
        "relative to it".to_owned(),
        "Edit inside a transaction".to_owned(),
        "`model: \"current\"`".to_owned(),
        "only as `elements` or `query` gives it".to_owned(),
    ] {
        assert!(instructions.contains(&said), "{said}: {instructions}");
    }
    // A second initialize is answered as the first was.
    let again = c.request(2, "initialize", "{}");
    assert_eq!(again["result"], *result);
    // And one with no params at all.
    c.send(r#"{"jsonrpc": "2.0", "id": 3, "method": "initialize"}"#);
    assert_eq!(c.message()["result"], *result);
}

#[test]
fn ping_is_answered_with_an_empty_result_before_and_after_initialize() {
    let mut c = Client::start();
    c.send(r#"{"jsonrpc": "2.0", "id": 7, "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":7,"result":{}}"#);
    c.request(8, "initialize", "{}");
    c.send(r#"{"jsonrpc": "2.0", "id": "p", "method": "ping", "params": {"_meta": {}}}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":"p","result":{}}"#);
}

#[test]
fn notifications_are_never_answered_known_or_not() {
    let mut c = Client::initialized();
    for notification in [
        r#"{"jsonrpc": "2.0", "method": "notifications/initialized"}"#,
        r#"{"jsonrpc": "2.0", "method": "notifications/roots/list_changed"}"#,
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 99}}"#,
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": "garbage"}"#,
        r#"{"jsonrpc": "2.0", "method": "no/such/thing", "params": [1, 2]}"#,
        r#"{"jsonrpc": "2.0", "method": "ping"}"#,
        // Responses, to nothing the server asked, whatever their IDs (an error's may be
        // null, JSON-RPC says) and their versions: never answered, an error least of all.
        r#"{"jsonrpc": "2.0", "id": 4, "result": {}}"#,
        r#"{"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "no"}}"#,
        r#"{"jsonrpc": "2.0", "id": true, "result": 1}"#,
        r#"{"jsonrpc": "2.0", "result": null}"#,
        r#"{"id": 4, "error": "no"}"#,
    ] {
        c.send(notification);
    }
    // Answered in line order, so the pong is the next line: nothing came before it.
    c.send(r#"{"jsonrpc": "2.0", "id": 5, "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":5,"result":{}}"#);
}

#[test]
fn a_tools_call_sent_as_a_notification_never_runs() {
    let mut c = Client::initialized();
    c.send(r#"{"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "open", "arguments": {"path": "made.silico", "create": true}}}"#);
    c.send(r#"{"jsonrpc": "2.0", "id": 1, "method": "ping"}"#);
    assert_eq!(c.line(), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
    // An open sent after it, made the same file: so the first never made it.
    let made = c.envelope(2, "open", r#"{"path": "made.silico", "create": true}"#);
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(made["result"]["created"], true);
}

#[test]
fn an_id_is_carried_back_as_the_client_wrote_it() {
    let mut c = Client::initialized();
    for (id, written) in [
        (r#""ab""#, r#""ab""#),
        ("1.50", "1.50"),
        ("-0", "-0"),
        ("1e3", "1e3"),
        (r#""""#, r#""""#),
    ] {
        c.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "ping"}}"#
        ));
        assert_eq!(
            c.line(),
            format!(r#"{{"jsonrpc":"2.0","id":{written},"result":{{}}}}"#)
        );
    }
}

#[test]
fn unknown_keys_in_a_message_and_in_tools_calls_params_are_ignored_and_absent_arguments_are_empty()
{
    let mut c = Client::initialized();
    c.send(r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "extra": [1], "params": {"name": "version", "later": {"x": 1}}}"#);
    let (line, _) = c.reply_line("1");
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["result"]["isError"], false, "{line}");
    assert_eq!(reply["result"]["structuredContent"]["result"]["api"], 1);
    // `arguments: {}` answers the same, byte for byte.
    c.send(r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "version", "arguments": {}}}"#);
    let (again, _) = c.reply_line("1");
    assert_eq!(again, line);
}
