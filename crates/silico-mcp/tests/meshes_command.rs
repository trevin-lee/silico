//! The `meshes` tool (docs/design/m5.0-app.md, section 7, `meshes`, M5.5; open item 23,
//! JSON numbers with `held`): listed as a read, its input schema the core's snapshot, and
//! callable, each answer the core's one-line envelope byte for byte, holding to its
//! `outputSchema`; a re-read with `held` as the first answer gave it carries no geometry.
//! Expected values: the same calls on the core's `Sessions::call` directly, in a workspace
//! of their own, and the fake's mesh by hand (the box (0, 0, 0) to (40, 20, 10): three of its
//! corners, one triangle a face, each edge its diagonal).

use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};

use crate::client::{BOX, Client, HANG, Reply};

/// A request of s1's current model from the front, at 100 pixels square, holding `held`.
fn meshes(held: &str) -> String {
    format!(
        r#"{{"session": "s1", "model": "current", "view": "front", "camera": null, "width": 100, "height": 100, "features": null, "held": {held}}}"#
    )
}

/// The answer's `structuredContent` as written, checked to be `content`'s one text block's
/// text, byte for byte; and `isError`.
fn structured(line: &str) -> (String, bool) {
    let reply: Reply<'_> = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
    let answer = reply.result;
    assert_eq!(answer.content.len(), 1, "{line}");
    assert_eq!(answer.content[0].kind, "text");
    assert_eq!(
        answer.content[0].text.as_deref(),
        Some(answer.structured.get())
    );
    (answer.structured.get().to_owned(), answer.is_error)
}

#[test]
fn the_meshes_tool_is_a_read_whose_input_is_the_cores_request() {
    let mut c = Client::initialized();
    let listed = c.request(1, "tools/list", "{}");
    let tools = listed["result"]["tools"].as_array().unwrap();
    let tool = tools.iter().find(|t| t["name"] == "meshes").unwrap();
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    assert!(tool["annotations"].get("destructiveHint").is_none());
    assert_eq!(tool["title"], "Display meshes");
    assert!(
        tool["description"]
            .as_str()
            .unwrap()
            .contains("Pass back in `held`")
    );
    assert_eq!(tool["inputSchema"]["type"], "object");
    assert_eq!(
        tool["inputSchema"]["required"],
        json!([
            "camera", "features", "height", "held", "model", "session", "view", "width"
        ])
    );
    let snapshot: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../silico-core/tests/contract/commands/meshes.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(tool["inputSchema"], snapshot["request"]);
    // Listed after drag_end, before sketch, as the design's section 7 has it.
    let at = |name: &str| tools.iter().position(|t| t["name"] == name).unwrap();
    assert_eq!(at("meshes"), at("drag_end") + 1);
    assert_eq!(at("sketch"), at("meshes") + 1);
}

#[test]
fn meshes_through_mcp_is_the_cores_envelope_byte_for_byte_and_held_carries_no_geometry() {
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), BOX).unwrap();
    let secret = dir.path().join("secret");
    let sessions = Sessions::with_cache_opener(Arc::new(FakeKernel::new(2).unwrap()), move |c| {
        Cache::with_secret(c, &secret)
    })
    .rendering(FakeRenderer::new())
    .in_workspace(&work)
    .unwrap();
    let core = |name: &str, arguments: &str| match sessions.call(name, arguments) {
        Call::Running(calling) => {
            let answered = calling.wait_timeout(HANG).ok().unwrap();
            (answered.text, !answered.ok)
        }
        Call::Unknown => panic!("{name}"),
    };
    let mut c = Client::initialized();
    let listed = c.request(0, "tools/list", "{}");
    let tool = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "meshes")
        .unwrap()
        .clone();
    let validator = jsonschema::validator_for(&tool["outputSchema"]).unwrap();
    c.file("part.silico", BOX);
    let mut id = 0;
    let mut both = |c: &mut Client, name: &str, arguments: &str| {
        id += 1;
        let want = core(name, arguments);
        c.send_call(id, name, arguments);
        let (line, _) = c.reply_line(&id.to_string());
        let got = structured(&line);
        assert_eq!(got, want, "{name} {arguments}");
        if name == "meshes" {
            let value: Value = serde_json::from_str(&got.0).unwrap();
            let errors: Vec<String> = validator
                .iter_errors(&value)
                .map(|e| e.to_string())
                .collect();
            assert!(errors.is_empty(), "{arguments}: {errors:?}");
        }
        serde_json::from_str::<Value>(&got.0).unwrap()
    };
    both(
        &mut c,
        "open",
        r#"{"path": "part.silico", "create": false}"#,
    );
    let first = both(&mut c, "meshes", &meshes("[]"));
    assert_eq!(first["ok"], true, "{first}");
    let body = &first["result"]["bodies"][0];
    assert_eq!(body["feature"], "k7f2qx3m");
    assert_eq!(body["held"], false);
    assert_eq!(body["box"], json!([[0, 0, 0], [40, 20, 10]]));
    assert_eq!(
        body["positions"],
        json!([[0, 0, 0], [40, 0, 0], [0, 20, 10]])
    );
    assert_eq!(body["triangles"], json!(vec![[0, 1, 2]; 6]));
    assert_eq!(body["faces"].as_array().unwrap().len(), 6);
    assert_eq!(body["edges"].as_array().unwrap().len(), 12);
    // Given back as written: held, its geometry left out, its names given.
    let held = format!(
        r#"[{{"key": {}, "deflection": {}}}]"#,
        body["key"], body["deflection"]
    );
    let second = both(&mut c, "meshes", &meshes(&held));
    let again = &second["result"]["bodies"][0];
    assert_eq!(again["held"], true, "{second}");
    for array in ["positions", "normals", "triangles", "edge_points"] {
        assert_eq!(again[array], json!([]), "{array}");
    }
    assert_eq!(again["faces"][0]["name"], body["faces"][0]["name"]);
    assert_eq!(again["faces"][0]["triangles"], Value::Null);
    assert_eq!(again["edges"][11]["points"], Value::Null);
    // Refusals, the core's alike: a key not one, and a view too large.
    let refused = both(
        &mut c,
        "meshes",
        &meshes(r#"[{"key": "K", "deflection": 1}]"#),
    );
    assert_eq!(refused["errors"][0]["parameter"], "held.0.key", "{refused}");
    let refused = both(
        &mut c,
        "meshes",
        r#"{"session": "s1", "model": "current", "view": "front", "camera": null, "width": 4097, "height": 100, "features": null, "held": []}"#,
    );
    assert_eq!(refused["errors"][0]["parameter"], "width", "{refused}");
    // Cancel names it, as a command a session runs: here none is running.
    let cancelled = both(
        &mut c,
        "cancel",
        r#"{"session": "s1", "command": "meshes"}"#,
    );
    assert_eq!(
        cancelled["errors"][0]["message"],
        "no meshes is running in session s1"
    );
}
