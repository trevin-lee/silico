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

/// M3.5's refusal of an open end of the line n0000001, at `point`, as the error contract
/// writes it.
fn open_end(point: &str) -> Value {
    json!({
        "code": "invalid_input",
        "message": format!(
            "point {point} ends line n0000001 and no other profile curve, so the profile is open there"
        ),
        "feature": "k7f2qx3m",
        "elements": [point, "n0000001"],
        "elements_total": 2,
        "parameter": null,
        "cause": null,
        "suggestion": "end another curve at the point to close the outline, or make the curve construction geometry",
        "candidates": [],
        "advice": null,
    })
}

#[test]
fn the_sketch_tool_answers_a_sketch_or_says_why_its_solver_isnt_there() {
    // The server's sessions solve on sketch workers found beside the executable: here
    // there may be none. Either way the answer is the core's envelope, never an RPC error.
    // Solved, the whole result by hand: no equations, two points of two values each, all
    // free, 4 left and every member under defined; the line open at both ends, M3.5's
    // refusal for each in point order, 2 in all. Or refused with the
    // rebuild's failure: no sketch worker where Silico looks for one, a fault of no
    // feature (as every read words a worker that couldn't be had), saying how to build it.
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
        assert_eq!(
            envelope["result"],
            json!({
                "sketch": "k7f2qx3m",
                "plane": {"origin": [0, 0, 0], "x": [1, 0, 0], "y": [0, 1, 0], "normal": [0, 0, 1]},
                "state": "under_defined",
                "dofs": 4,
                "points": [
                    {"id": "p0000001", "x": 0, "y": 0, "construction": false, "state": "under_defined"},
                    {"id": "p0000002", "x": 10, "y": 3, "construction": false, "state": "under_defined"},
                ],
                "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false, "state": "under_defined"}],
                "circles": [],
                "arcs": [],
                "constraints": [],
                "regions": [],
                "profile_problems": [open_end("p0000001"), open_end("p0000002")],
                "profile_problems_total": 2,
                "solve_problem": null,
                "plane_problem": null,
            }),
            "{result}"
        );
    } else {
        let error = &envelope["errors"][0];
        assert_eq!(
            (&error["code"], &error["feature"], &envelope["total"]),
            (&json!("worker_unavailable"), &Value::Null, &json!(1)),
            "{result}"
        );
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|m| m.contains("silico-sketch-worker")),
            "{result}"
        );
        assert!(
            error["suggestion"]
                .as_str()
                .is_some_and(|s| s.starts_with("build it")),
            "{result}"
        );
    }
    // The session goes on: a check after it answers.
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
