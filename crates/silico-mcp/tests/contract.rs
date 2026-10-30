//! The tool list (docs/design/m4.9-mcp.md, *Tools*; M4.1 decision 24): `tools.json`, the
//! `tools/list` reply written canonically, equal to what the server sends; each tool's
//! `inputSchema` the core's committed request snapshot, and its `outputSchema` what the
//! committed envelope and result snapshots build, put together here from the files; and
//! every example in the core's contract validating against its tool's schemas. Expected
//! values: the committed snapshots (protected), the note's construction done by hand on
//! them, and the annotations, titles' presence, and order written out here.
//!
//! **Recording.** Run these tests with `SILICO_CONTRACT_RECORD=1` set, and `tools.json` is
//! written first from what the server sends: only for a change the maintainer has
//! approved, since `tests/contract/` is protected.

use std::path::PathBuf;

use serde_json::{Map, Value, json};
use silico_core::command::Command;
use silico_mcp::tools;

use crate::client::Client;
use crate::errors::TOOLS;

fn core_contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../silico-core/tests/contract")
        .join(name)
}

/// The core's committed contract file `name`.
pub fn committed_core(name: &str) -> String {
    let path = core_contract(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn snapshot(name: &str) -> Value {
    serde_json::from_str(&committed_core(&format!("commands/{name}.json"))).unwrap()
}

/// Panics, naming `what`, unless `value` holds to `schema`.
pub fn holds(schema: &Value, value: &Value, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

/// Panics, naming `what`, if `value` holds to `schema`.
fn fails(schema: &Value, value: &Value, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    assert!(!validator.is_valid(value), "{what} held");
}

/// The tools the server sends, in its `tools/list` reply.
fn sent() -> Value {
    let mut c = Client::start();
    c.request(1, "tools/list", "{}")["result"].clone()
}

/// The committed `tools.json`, or, with `SILICO_CONTRACT_RECORD` set, `value` written to it
/// first.
fn recorded(value: &Value) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/contract/tools.json");
    if std::env::var_os("SILICO_CONTRACT_RECORD").is_some() {
        std::fs::write(&path, tools::canonical(value).unwrap()).unwrap();
    }
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn tool<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no tool {name}"))
}

#[test]
fn tools_json_is_what_the_server_sends_written_canonically() {
    let sent = sent();
    let committed = recorded(&sent);
    let file: Value = serde_json::from_str(&committed).unwrap();
    assert_eq!(file, sent);
    // Canonical: sorted keys, two-space indents, a trailing newline (as the core's
    // snapshots are), so its text is its value's one writing.
    assert_eq!(committed, tools::canonical(&file).unwrap());
    assert!(committed.ends_with("}\n"));
    assert!(committed.starts_with("{\n  \"tools\": [\n    {\n      \"annotations\": {\n"));
    // What `tools::list` builds is what's sent.
    assert_eq!(tools::list().unwrap(), sent);
}

#[test]
fn the_tools_are_the_commands_in_their_order_each_with_mcps_fields() {
    let sent = sent();
    let names: Vec<&str> = sent["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, TOOLS);
    let reads = [
        "version",
        "status",
        "text",
        "outline",
        "check",
        "elements",
        "query",
        "schema",
        "mass_properties",
        "distance",
        "angle",
        "read_logs",
        "timeline",
    ];
    let destructive = [
        "commit",
        "format",
        "add_feature",
        "set_field",
        "remove_feature",
        "replace_text",
        "render",
        "export",
        "move_feature",
        "sketch_add",
        "sketch_remove",
    ];
    for tool in sent["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let keys: Vec<&String> = tool.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "annotations",
                "description",
                "inputSchema",
                "name",
                "outputSchema",
                "title"
            ],
            "{name}"
        );
        assert!(!tool["title"].as_str().unwrap().is_empty(), "{name}");
        assert!(tool["description"].as_str().unwrap().len() > 40, "{name}");
        let annotations = &tool["annotations"];
        assert_eq!(annotations["title"], tool["title"], "{name}");
        assert_eq!(annotations["openWorldHint"], false, "{name}");
        if reads.contains(&name) {
            assert_eq!(annotations["readOnlyHint"], true, "{name}");
            assert!(annotations.get("destructiveHint").is_none(), "{name}");
        } else {
            assert_eq!(annotations["readOnlyHint"], false, "{name}");
            assert_eq!(
                annotations["destructiveHint"],
                destructive.contains(&name),
                "{name}"
            );
        }
    }
    // The tips are in the descriptions of the tools they're about.
    let said = |name: &str, what: &str| {
        tool(&sent, name)["description"]
            .as_str()
            .unwrap()
            .contains(what)
    };
    assert!(said("set_field", "Edit inside a transaction"));
    assert!(said("add_feature", "`model: \"current\"`"));
    assert!(said("query", "only as `elements` or `query` gives it"));
    assert!(said("sketch_add", "Edit inside a transaction"));
    assert!(said("sketch_remove", "`model: \"current\"`"));
    assert!(!said("version", "transaction"));
}

/// `result`'s schema, as the note says it's inlined: without its `$schema`, `title`, and
/// `$defs`.
fn inlined(result: &Value) -> Value {
    let mut result = result.as_object().unwrap().clone();
    for key in ["$schema", "title", "$defs"] {
        result.remove(key);
    }
    Value::Object(result)
}

#[test]
fn each_tools_schemas_are_what_the_committed_snapshots_build() {
    let sent = sent();
    let envelope = snapshot("envelope");
    let envelope_defs = envelope["$defs"].as_object().unwrap();
    for name in TOOLS {
        let snapshot = snapshot(name);
        let tool = tool(&sent, name);
        assert_eq!(tool["inputSchema"], snapshot["request"], "{name}");
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        // The root: the dialect, an object, the envelope's two branches.
        let output = &tool["outputSchema"];
        let keys: Vec<&String> = output.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["$defs", "$schema", "anyOf", "type"], "{name}");
        assert_eq!(
            output["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(output["type"], "object");
        let mut ran = envelope_defs["Ran"].clone();
        ran["properties"]["result"] = inlined(&snapshot["result"]);
        assert_eq!(
            output["anyOf"],
            json!([ran, {"$ref": "#/$defs/Refused"}]),
            "{name}"
        );
        // The definitions: the envelope's but Ran, and the result's.
        let mut defs: Map<String, Value> = envelope_defs.clone();
        defs.remove("Ran");
        if let Some(lifted) = snapshot["result"].get("$defs") {
            for (def, schema) in lifted.as_object().unwrap() {
                if let Some(same) = defs.get(def) {
                    assert_eq!(same, schema, "{name}: {def}");
                }
                defs.insert(def.clone(), schema.clone());
            }
        }
        assert_eq!(output["$defs"], Value::Object(defs), "{name}");
        // `$schema` only at the root, and every reference resolves there.
        let text = serde_json::to_string(&output["$defs"]).unwrap()
            + &serde_json::to_string(&output["anyOf"]).unwrap();
        assert!(!text.contains("\"$schema\""), "{name}");
        for reference in text.split("\"$ref\":\"#/$defs/").skip(1) {
            let def = &reference[..reference.find('"').unwrap()];
            assert!(output["$defs"].get(def).is_some(), "{name}: {def}");
        }
    }
}

#[test]
fn a_definition_both_schemas_give_must_be_the_same_in_both() {
    let envelope = snapshot("envelope");
    let error = envelope["$defs"]["Error"].clone();
    let with = |defs: Value| json!({"$schema": tools::DIALECT, "title": "R", "type": "object", "properties": {"e": {"$ref": "#/$defs/Error"}}, "$defs": defs});
    // The same Error: built, the result's definition the envelope's.
    let built = tools::output_schema(
        &envelope,
        &with(json!({"Error": error, "Extra": {"type": "integer"}})),
    )
    .unwrap();
    assert_eq!(built["$defs"]["Error"], error);
    assert_eq!(built["$defs"]["Extra"], json!({"type": "integer"}));
    assert!(built["$defs"].get("Ran").is_none());
    // Another Error: refused, naming it, never one picked.
    let mut other = error.clone();
    other["description"] = json!("another");
    let refused = tools::output_schema(&envelope, &with(json!({"Error": other}))).unwrap_err();
    assert_eq!(refused.code(), silico_core::ErrorCode::Internal);
    assert_eq!(
        refused.message(),
        "the result's definition Error differs from the envelope's"
    );
    // And a tool built from one names its command.
    let refused = tools::tool(
        Command::Close,
        &envelope,
        &json!({"request": {"type": "object"}, "result": with(json!({"Session": {"type": "string"}}))}),
    )
    .unwrap_err();
    assert_eq!(
        refused.message(),
        "close's output schema couldn't be built: the result's definition Session differs from the envelope's"
    );
}

/// The requests among the examples whose names don't say so: the measures' and the
/// query's, named for what they ask.
const REQUESTS: [&str; 5] = [
    "angle/wedge",
    "distance/box-bottom-and-top",
    "mass_properties/every-body",
    "query/largest-face-up",
    "remove_feature/request-used",
];

/// An envelope of a command that ran with `result`.
fn ran(result: Value) -> Value {
    json!({"ok": true, "session": null, "model": null, "result": result})
}

/// An envelope of a refusal for `error`.
fn refusal(error: Value) -> Value {
    json!({"ok": false, "session": null, "model": null, "errors": [error], "total": 1})
}

#[test]
fn every_example_holds_to_its_tools_schemas() {
    let sent = sent();
    let examples = core_contract("commands/examples");
    let mut seen = 0;
    for dir in std::fs::read_dir(&examples).unwrap() {
        let dir = dir.unwrap().path();
        let command = dir.file_name().unwrap().to_str().unwrap().to_owned();
        for file in std::fs::read_dir(&dir).unwrap() {
            let file = file.unwrap().path();
            let stem = file.file_stem().unwrap().to_str().unwrap().to_owned();
            let what = format!("{command}/{stem}");
            let value: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap())
                .unwrap_or_else(|e| panic!("{what}: {e}"));
            seen += 1;
            match command.as_str() {
                // Whole envelopes: `begin`'s result, a published `check`, a `begin` refused.
                "envelope" => {
                    let of = match stem.as_str() {
                        "ran" | "refused" => "begin",
                        "read-a-model" => "check",
                        other => panic!("an envelope example not known here: {other}"),
                    };
                    holds(&tool(&sent, of)["outputSchema"], &value, &what);
                }
                // An element's entry, as `elements` and `query` list them.
                "entry" => {
                    let page = json!({"elements": [value], "matched": 1, "tied": null});
                    holds(&tool(&sent, "query")["outputSchema"], &ran(page), &what);
                }
                // A warning, as `check` lists it.
                "warning" => {
                    let checked = json!({"problems": [], "problems_total": 0,
                        "warnings": [value], "warnings_total": 1});
                    holds(&tool(&sent, "check")["outputSchema"], &ran(checked), &what);
                }
                _ => {
                    let tool = tool(&sent, &command);
                    if stem.starts_with("request") || REQUESTS.contains(&what.as_str()) {
                        holds(&tool["inputSchema"], &value, &what);
                        // A request isn't a result.
                        fails(&tool["outputSchema"], &ran(value), &what);
                    } else if value.get("code").is_some() {
                        holds(&tool["outputSchema"], &refusal(value.clone()), &what);
                        fails(&tool["outputSchema"], &ran(value), &what);
                    } else {
                        holds(&tool["outputSchema"], &ran(value.clone()), &what);
                        fails(&tool["outputSchema"], &refusal(value), &what);
                    }
                }
            }
        }
    }
    // Every file in the contract's examples, counted by hand: 89 on M4.9's branch,
    // export's 8 (#182), cache_clear's 2 (#183), STEP export's 4 (#186), check's
    // warning (#189), and read_logs' 3 and version's result (#194); M5.1's 11:
    // add_feature's 4 for `before`, remove_feature's result with `rewired`, move_feature's
    // 4, and timeline's 2; and M5.10's 6: sketch_add's 2 and sketch_remove's 4.
    assert_eq!(seen, 125);
}

#[test]
fn an_output_schema_tells_its_commands_result_from_anothers() {
    let sent = sent();
    let close = &tool(&sent, "close")["outputSchema"];
    let begin = &tool(&sent, "begin")["outputSchema"];
    holds(close, &ran(json!({})), "close's result");
    fails(close, &ran(json!({"base": 4})), "begin's result as close's");
    holds(begin, &ran(json!({"base": 4})), "begin's result");
    fails(begin, &ran(json!({})), "close's result as begin's");
    // Neither branch half of each.
    fails(
        close,
        &json!({"ok": true, "session": null, "model": null, "errors": [], "total": 0}),
        "ran with errors",
    );
    fails(
        close,
        &json!({"ok": false, "session": null, "model": null, "result": {}}),
        "refused with a result",
    );
    fails(close, &json!([]), "an array");
}

/// Every `format` keyword a schema in `value` gives, with where. A property named `format`
/// (export's) is a schema, an object, so never taken for the keyword, a string.
fn formats(value: &Value, at: &str, found: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(format)) = map.get("format") {
                found.push((at.to_owned(), format.clone()));
            }
            for (key, value) in map {
                formats(value, &format!("{at}/{key}"), found);
            }
        }
        Value::Array(items) => {
            for (i, value) in items.iter().enumerate() {
                formats(value, &format!("{at}/{i}"), found);
            }
        }
        _ => {}
    }
}

#[test]
fn no_tools_schema_gives_a_format() {
    // JSON Schema's formats are all strings', and Silico's strings have none: a `format` a
    // schema gave would be schemars' name of a Rust number's type (`uint64`, `double`),
    // which a strict validator (Ajv, the MCP SDKs') warns of, once each place it's given.
    let sent = sent();
    let mut found = Vec::new();
    formats(&sent, "", &mut found);
    assert!(found.is_empty(), "{found:?}");
    // Each bound is said where its field is declared: `elements`' `limit` keeps its 1,000,
    // and render's result its width's and height's 4,096, the request's.
    let elements = &tool(&sent, "elements")["inputSchema"]["properties"]["limit"];
    assert_eq!(elements["maximum"], 1000, "{elements}");
    let render = tool(&sent, "render")["outputSchema"].to_string();
    for side in ["width", "height"] {
        let bounded = format!(
            r#""{side}":{{"description":"The image's {side} in pixels.","maximum":4096,"minimum":1,"type":"integer"}}"#
        );
        assert!(render.contains(&bounded), "{side}: {render}");
    }
}
