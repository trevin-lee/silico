//! break-it on the `sketch` tool: a feature that isn't a sketch, an unknown one, a sketch
//! whose solver may not be found beside the test's executable, and a document with no
//! sketch, each answered through MCP as the core's envelope: a structured refusal naming
//! `sketch` or the sketch, never a JSON-RPC error, a hang, or a refusal with no advice.

use serde_json::{Value, json};

use crate::client::Client;

const LINE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "b7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 3, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}]}"#;

#[test]
fn the_sketch_tool_refuses_a_box_and_an_unknown_feature_naming_sketch_with_the_sketches() {
    let mut c = Client::initialized();
    c.file("part.silico", LINE);
    c.open(1, "part.silico");
    for (n, (sketch, message)) in [
        ("b7f2qx3m", "b7f2qx3m is a box, not a sketch"),
        ("z7f2qx3m", "the document has no feature z7f2qx3m"),
    ]
    .into_iter()
    .enumerate()
    {
        let result = c.call(
            10 + n as u64,
            "sketch",
            &json!({"session": "s1", "model": "current", "sketch": sketch}).to_string(),
        );
        assert_eq!(result["isError"], true, "{result}");
        let error = &result["structuredContent"]["errors"][0];
        assert_eq!(
            (
                &error["code"],
                &error["message"],
                &error["parameter"],
                &error["candidates"],
            ),
            (
                &json!("invalid_input"),
                &json!(message),
                &json!("sketch"),
                &json!(["k7f2qx3m"]),
            ),
            "{result}"
        );
    }
}

#[test]
fn the_sketch_tool_answers_a_sketch_or_says_why_its_solver_isnt_there() {
    // The server's sessions solve on sketch workers found beside the executable: here
    // there may be none. Either way the answer is the core's envelope, never an RPC error:
    // solved (four degrees free, both points under defined), or refused with the rebuild's
    // failure, naming the sketch, with advice.
    let mut c = Client::initialized();
    c.file("part.silico", LINE);
    c.open(1, "part.silico");
    let result = c.call(
        2,
        "sketch",
        r#"{"session": "s1", "model": "current", "sketch": "k7f2qx3m"}"#,
    );
    let envelope = &result["structuredContent"];
    if envelope["ok"] == true {
        assert_eq!(envelope["result"]["dofs"], 4, "{result}");
        assert_eq!(envelope["result"]["state"], "under_defined");
    } else {
        let error = &envelope["errors"][0];
        assert_eq!(error["feature"], "k7f2qx3m", "{result}");
        assert!(error["suggestion"].is_string(), "{result}");
        assert_ne!(error["code"], Value::Null);
    }
    // The session goes on: a ping and a check after it answer.
    let check = c.envelope(3, "check", crate::client::CHECK);
    assert_eq!(check["ok"], true, "{check}");
}

#[test]
fn the_sketch_tool_on_a_document_with_no_sketch_doesnt_say_to_name_one_of_its_sketches() {
    // Found by break-it: the document has no sketch, so there's none to name, but the
    // refusal's only advice was "name a sketch of the document, by its ID", with no
    // candidates: an agent told to pick from an empty list (the catalog's "not found with no
    // candidates, its suggestion still pointing at candidates"). It now says there's none.
    let mut c = Client::opened();
    let result = c.call(
        2,
        "sketch",
        r#"{"session": "s1", "model": "current", "sketch": "k7f2qx3m"}"#,
    );
    let error = &result["structuredContent"]["errors"][0];
    assert_eq!(
        (&error["message"], &error["candidates"]),
        (&json!("k7f2qx3m is a box, not a sketch"), &json!([])),
        "{result}"
    );
    assert_eq!(
        error["suggestion"],
        json!("the document has no sketch: add one, and read it by its ID"),
        "advice to name one of no sketches"
    );
}
