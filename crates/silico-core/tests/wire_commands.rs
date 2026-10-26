//! M4.4's wire types (docs/design/m4.1-command-api.md, sections 2, 7, and 8): `elements`',
//! `query`'s, and `schema`'s snapshots, as committed, every object in them closed; and the
//! design's examples of them, byte for byte: each file the design's own block, each request
//! read and written back to it, each refusal the command's own, written to it, and each
//! validated against its snapshot. The entries of real solids, the design's box with its
//! hole, are written to their examples by silico-kernel's tests/wire_examples.rs, on kernel
//! workers; the fake kernel measures nothing as a solid would.
//!
//! **Recording a snapshot.** Run these tests with `SILICO_CONTRACT_RECORD=1` set, and each
//! command's snapshot is written first from what the code gives: only for a change the
//! maintainer has approved, since `tests/contract/` is protected. The examples are the
//! design's text, which no recording writes.

use super::listing::model_of;

use std::path::PathBuf;

use silico_core::ErrorCode;
use silico_core::command::Revision;
use silico_core::command::elements::{
    Alias, AxisToken, Cursor, ElementType, EntryAxis, Facing, MeasureName, Order, Quantity,
    Queried, Query, Range, Tied, entries, list, query,
};
use silico_core::command::schema::schema;
use silico_core::command::wire::elements::{ElementsRequest, QueryRequest};
use silico_core::command::wire::measure::MassPropertiesRequest;
use silico_core::command::wire::schema::SchemaRequest;
use silico_core::command::wire::{COMMANDS, Layout, ToWire, snapshot, snapshot_value, write};
use silico_core::command::{ModelChoice, Problems, SessionId};
use silico_core::document::json::{Node, read};
use silico_core::document::{Angle, FeatureId, Length, Measure};
use silico_core::kernel::ElementKind;

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract")
        .join(name)
}

/// The committed file `name`, read and never written.
fn committed(name: &str) -> String {
    let path = contract(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The committed file `name`, or, with `SILICO_CONTRACT_RECORD` set, `text`, what the code
/// gives, written to it first (the maintainer's to approve: the directory is protected).
/// Only a snapshot's own test records it; every other reads it ([`committed`]).
fn recorded(name: &str, text: &str) -> String {
    if std::env::var_os("SILICO_CONTRACT_RECORD").is_some() {
        std::fs::write(contract(name), text).unwrap();
    }
    committed(name)
}

/// The example `name` (`elements/request`), as committed.
fn example(name: &str) -> String {
    let path = contract(&format!("commands/examples/{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The design's JSON code blocks between the headings `from` and `to`, in order.
fn design(from: &str, to: &str) -> Vec<String> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m4.1-command-api.md");
    let doc = std::fs::read_to_string(&path).unwrap();
    let section = &doc[doc.find(from).unwrap()..doc.find(to).unwrap()];
    section
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

/// `value` as the API writes it, and a newline, as each example file holds it.
fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

fn parsed(text: &str) -> Node {
    read(text.as_bytes()).unwrap()
}

/// The snapshot `of`'s `part` (`request` or `result`), or its definition `def` in it.
fn schema_of(of: &str, part: &str, def: Option<&str>) -> serde_json::Value {
    let schema: serde_json::Value =
        serde_json::from_str(&committed(&format!("commands/{of}.json"))).unwrap();
    let root = &schema[part];
    match def {
        None => root.clone(),
        Some(def) => serde_json::json!({
            "$defs": root["$defs"],
            "$ref": format!("#/$defs/{def}"),
        }),
    }
}

/// The error contract's shape, the envelope's `Error`.
fn error_schema() -> serde_json::Value {
    let envelope: serde_json::Value =
        serde_json::from_str(&committed("commands/envelope.json")).unwrap();
    serde_json::json!({"$defs": envelope["$defs"], "$ref": "#/$defs/Error"})
}

/// Panics, naming `what`, unless `text` holds to `schema`.
fn holds(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

/// Panics, naming `what`, if `text` holds to `schema`.
fn fails(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(!validator.is_valid(&value), "{what} held");
}

fn revision(n: u64) -> Revision {
    let mut r = Revision::OPENED;
    for _ in 1..n {
        r = r.next().unwrap();
    }
    r
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

/// The first refusal of `problems`, which must hold just one, written.
fn the_refusal(problems: &Problems) -> String {
    assert_eq!(problems.total(), 1, "{:?}", problems.errors());
    written(&problems.errors()[0])
}

/// The query of the design's request: "the largest planar face facing +Z".
fn largest_face_up() -> Query {
    Query {
        feature: None,
        kind: ElementKind::Face,
        r#type: Some(ElementType::Plane),
        stable: Some(true),
        facing: Some(Facing {
            direction: [0.0, 0.0, 1.0],
            within: Angle::from_ndeg(1_000_000_000),
        }),
        measures: Vec::new(),
        order: Some(Order {
            by: MeasureName::Area,
            descending: true,
        }),
        limit: 1,
    }
}

/// A sketch, which makes no solid, and a box.
const SKETCH_AND_BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Outline", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}, {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

#[test]
fn each_commands_schemas_are_the_committed_snapshot() {
    for (of, _) in COMMANDS {
        let text = snapshot(of).unwrap().unwrap();
        assert_eq!(
            text,
            recorded(&format!("commands/{of}.json"), &text),
            "{of}"
        );
        let value = snapshot_value(of).unwrap();
        // Two standalone roots, each with its own definitions, as MCP takes them.
        assert_eq!(
            value.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["request", "result"],
            "{of}"
        );
    }
    assert!(snapshot("version").is_none());
}

/// Every object of `schema` with properties that isn't closed to them or doesn't require
/// them all, by its path.
fn open_shapes(schema: &serde_json::Value, at: &str, found: &mut Vec<String>) {
    match schema {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Object(properties)) = map.get("properties") {
                let mut keys: Vec<&str> = properties.keys().map(String::as_str).collect();
                keys.sort_unstable();
                let mut required: Vec<&str> = map
                    .get("required")
                    .and_then(serde_json::Value::as_array)
                    .map(|r| r.iter().filter_map(serde_json::Value::as_str).collect())
                    .unwrap_or_default();
                required.sort_unstable();
                let closed = map.get("additionalProperties") == Some(&serde_json::json!(false));
                if !closed || required != keys {
                    found.push(at.to_owned());
                }
            }
            for (key, value) in map {
                open_shapes(value, &format!("{at}/{key}"), found);
            }
        }
        serde_json::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                open_shapes(item, &format!("{at}/{i}"), found);
            }
        }
        _ => {}
    }
}

#[test]
fn every_object_of_the_commands_snapshots_is_closed_and_requires_its_fields() {
    let document: serde_json::Value =
        serde_json::from_str(&committed("document-schema.json")).unwrap();
    for (of, _) in COMMANDS {
        let mut value = snapshot_value(of).unwrap();
        // A document type a request or result holds keeps the document's own schema
        // (`wire_shape`): the document's definitions, as the document's schema has them, and
        // a new feature, the document's feature less its fingerprint
        // (wire_session_commands.rs checks it), aren't the wire's to close.
        for part in ["request", "result"] {
            if let Some(defs) = value[part]["$defs"].as_object_mut() {
                defs.retain(|name, def| {
                    name != "NewFeature" && document["$defs"].get(name) != Some(def)
                });
            }
        }
        let mut found = Vec::new();
        open_shapes(&value, "", &mut found);
        assert!(found.is_empty(), "{of}: {found:?}");
    }
    // Their parts by the names an agent reads.
    let elements = snapshot_value("elements").unwrap();
    let defs = |root: &serde_json::Value| -> Vec<String> {
        root["$defs"].as_object().unwrap().keys().cloned().collect()
    };
    assert_eq!(elements["request"]["title"], "ElementsRequest");
    assert_eq!(
        defs(&elements["request"]),
        ["Cursor", "FeatureId", "ModelChoice"]
    );
    assert_eq!(elements["result"]["title"], "ElementsResult");
    assert_eq!(
        defs(&elements["result"]),
        [
            "Alias",
            "Angle",
            "AxisToken",
            "Cursor",
            "ElementKind",
            "ElementType",
            "Entry",
            "EntryAxis",
            "FeatureId",
            "Length",
            "Measure"
        ]
    );
    let query = snapshot_value("query").unwrap();
    assert_eq!(query["request"]["title"], "QueryRequest");
    assert_eq!(
        defs(&query["request"]),
        [
            "Angle",
            "Bound",
            "ElementKind",
            "ElementType",
            "Facing",
            "FeatureId",
            "MeasureName",
            "MeasureRange",
            "ModelChoice",
            "Order"
        ]
    );
    assert_eq!(query["result"]["title"], "QueryResult");
    let mass = snapshot_value("mass_properties").unwrap();
    assert_eq!(mass["request"]["title"], "MassPropertiesRequest");
    assert_eq!(mass["result"]["title"], "MassPropertiesResult");
    assert_eq!(
        defs(&mass["result"]),
        ["FeatureId", "Length", "MassProperties", "Measure"]
    );
    let schema = snapshot_value("schema").unwrap();
    assert_eq!(schema["request"]["title"], "SchemaRequest");
    assert_eq!(schema["result"]["title"], "SchemaResult");
}

#[test]
fn the_word_lists_in_the_snapshots_are_the_commands_own() {
    // Each enum's schema is built from the command's own words, so a word the reader takes
    // is one the schema lists, and no other.
    let elements = snapshot_value("elements").unwrap();
    let defs = &elements["result"]["$defs"];
    let words = |def: &serde_json::Value| -> Vec<String> {
        def["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(words(&defs["ElementKind"]), ["face", "edge", "vertex"]);
    assert_eq!(
        words(&defs["ElementType"]),
        ElementType::ALL.map(|t| t.word().to_owned())
    );
    assert_eq!(words(&defs["ElementType"]).len(), 17);
    assert_eq!(
        words(&defs["AxisToken"]),
        ["+x", "-x", "+y", "-y", "+z", "-z"]
    );
    let query = snapshot_value("query").unwrap();
    assert_eq!(
        words(&query["request"]["$defs"]["MeasureName"]),
        [
            "area",
            "length",
            "radius",
            "diameter",
            "major_radius",
            "minor_radius",
            "half_angle"
        ]
    );
    // The model's choices, each described: a `oneOf` of constants.
    let choices: Vec<&str> = query["request"]["$defs"]["ModelChoice"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|choice| choice["const"].as_str().unwrap())
        .collect();
    assert_eq!(choices, ["current", "published"]);
}

#[test]
fn the_examples_are_the_designs_own_text() {
    // Each section's blocks, in order, each file the block and a newline.
    let sections: [(&str, &str, &str, &[&str]); 6] = [
        (
            "### Element entries",
            "### Warnings",
            "entry",
            &["plane-face", "cylinder-face"],
        ),
        (
            "### `elements` (M4.4)",
            "### `query` (M4.4)",
            "elements",
            &[
                "request",
                "first-page",
                "refused-sketch",
                "refused-limit",
                "stale-cursor",
            ],
        ),
        (
            "### `query` (M4.4)",
            "### `schema` (M4.4)",
            "query",
            &[
                "largest-face-up",
                "largest-face-up-found",
                "tie-not-included",
                "refused-kind",
                "refused-facing-edges",
                "refused-zero-direction",
            ],
        ),
        (
            "### `schema` (M4.4)",
            "### `mass_properties` (M4.5)",
            "schema",
            &["request-document"],
        ),
        (
            "### `mass_properties` (M4.5)",
            "### `distance` (M4.5)",
            "mass_properties",
            &["every-body", "every-body-measured"],
        ),
        (
            "### `render` (M4.6)",
            "### `read_logs` (M4.7)",
            "render",
            &["request", "result", "refused-width", "refused-eye-inside"],
        ),
    ];
    for (from, to, dir, names) in sections {
        let blocks = design(from, to);
        assert_eq!(blocks.len(), names.len(), "{dir}");
        for (block, name) in blocks.iter().zip(names) {
            assert_eq!(
                format!("{block}\n"),
                example(&format!("{dir}/{name}")),
                "{dir}/{name}"
            );
        }
    }
}

#[test]
fn the_designs_requests_are_read_written_back_byte_for_byte_and_hold_to_their_schemas() {
    let text = example("elements/request");
    let request = ElementsRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        ElementsRequest {
            session: s1(),
            model: ModelChoice::Current,
            feature: None,
            after: None,
            limit: 2,
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("elements", "request", None), &text, "elements");

    let text = example("query/largest-face-up");
    let request = QueryRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        QueryRequest {
            session: s1(),
            model: ModelChoice::Current,
            query: largest_face_up(),
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("query", "request", None), &text, "query");

    let text = example("mass_properties/every-body");
    let request = MassPropertiesRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        MassPropertiesRequest {
            session: s1(),
            model: ModelChoice::Current,
            feature: None,
        }
    );
    assert_eq!(written(&request), text);
    holds(
        &schema_of("mass_properties", "request", None),
        &text,
        "mass_properties",
    );

    let text = example("schema/request-document");
    let request = SchemaRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        SchemaRequest {
            of: "document".to_owned()
        }
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("schema", "request", None), &text, "schema");
}

#[test]
fn a_cursor_and_ranges_of_each_quantity_are_read_and_written_back() {
    let text = r#"{
  "session": "s3",
  "model": "published",
  "feature": "m3tq8v0z",
  "after": {"revision": 6, "feature": "m3tq8v0z", "name": "k7f2qx3m.box.Face2"},
  "limit": 1000
}
"#;
    let request = ElementsRequest::read(&parsed(text)).unwrap();
    assert_eq!(
        request.after,
        Some(Cursor {
            revision: revision(6),
            feature: id("m3tq8v0z"),
            name: "k7f2qx3m.box.Face2".to_owned(),
        })
    );
    assert_eq!(request.feature, Some(id("m3tq8v0z")));
    assert_eq!(written(&request), text);
    holds(&schema_of("elements", "request", None), text, "a cursor");

    // An area to 6 places, a length to 6, an angle to 9, each read exactly.
    let text = r#"{
  "session": "s1",
  "model": "current",
  "feature": null,
  "kind": "face",
  "type": null,
  "stable": false,
  "facing": null,
  "measures": [
    {"measure": "area", "at_least": 100.000001, "at_most": null},
    {"measure": "radius", "at_least": null, "at_most": 2.500001},
    {"measure": "half_angle", "at_least": -33.690067526, "at_most": 0}
  ],
  "order": null,
  "limit": 1000
}
"#;
    let request = QueryRequest::read(&parsed(text)).unwrap();
    assert_eq!(
        request.query.measures,
        [
            Range {
                measure: MeasureName::Area,
                at_least: Some(Quantity::Area(Measure::from_millionths(100_000_001))),
                at_most: None,
            },
            Range {
                measure: MeasureName::Radius,
                at_least: None,
                at_most: Some(Quantity::Length(Length::from_nm(2_500_001))),
            },
            Range {
                measure: MeasureName::HalfAngle,
                at_least: Some(Quantity::Angle(Angle::from_ndeg(-33_690_067_526))),
                at_most: Some(Quantity::Angle(Angle::from_ndeg(0))),
            },
        ]
    );
    assert_eq!(request.query.stable, Some(false));
    assert_eq!(written(&request), text);
    holds(&schema_of("query", "request", None), text, "ranges");
}

#[test]
fn the_designs_refusals_are_the_commands_own_and_hold_to_the_error_contract() {
    let error = error_schema();
    let model = model_of(SKETCH_AND_BOX);
    let check = |name: &str, got: String| {
        let want = example(name);
        assert_eq!(got, want, "{name}");
        holds(&error, &want, name);
    };

    // `elements` of a sketch, with a limit past what a call lists, and with a cursor of an
    // older revision.
    let sketch = list(&model, Revision::OPENED, Some(&id("s0000001")), None, 2).unwrap_err();
    check("elements/refused-sketch", the_refusal(&sketch));
    let limit = list(&model, Revision::OPENED, None, None, 5000).unwrap_err();
    check("elements/refused-limit", the_refusal(&limit));
    let older = Cursor {
        revision: revision(6),
        feature: id("k7f2qx3m"),
        name: "k7f2qx3m.box.Face2".to_owned(),
    };
    let stale = list(&model, revision(7), None, Some(&older), 2).unwrap_err();
    assert_eq!(stale.errors()[0].code(), ErrorCode::Conflict);
    check("elements/stale-cursor", the_refusal(&stale));

    // `query` of solids, the request's reader's refusal and the command's alike.
    let request =
        example("query/largest-face-up").replace(r#""kind": "face""#, r#""kind": "solid""#);
    let kind = QueryRequest::read(&parsed(&request)).unwrap_err();
    check("query/refused-kind", the_refusal(&kind));
    let solids = query(
        &model,
        &Query {
            kind: ElementKind::Solid,
            ..Query::of(ElementKind::Face, 1)
        },
    )
    .unwrap_err();
    check("query/refused-kind", the_refusal(&solids));
    // Edges facing a direction: the command's.
    let edges = query(
        &model,
        &Query {
            facing: largest_face_up().facing,
            ..Query::of(ElementKind::Edge, 1)
        },
    )
    .unwrap_err();
    check("query/refused-facing-edges", the_refusal(&edges));
    // A zero direction: the reader's.
    let request = example("query/largest-face-up").replace("[0, 0, 1]", "[0, 0, 0]");
    let zero = QueryRequest::read(&parsed(&request)).unwrap_err();
    check("query/refused-zero-direction", the_refusal(&zero));
}

#[test]
fn a_tie_too_large_to_return_is_written_as_the_design_gives_it() {
    let tied = Queried {
        entries: Vec::new(),
        matched: 5000,
        tied: Some(Tied {
            count: 5000,
            included: false,
        }),
    };
    let text = written(&tied);
    assert_eq!(text, example("query/tie-not-included"));
    let result = schema_of("query", "result", None);
    holds(&result, &text, "a tie");
    // The schema holds a result to its shape: a tie without `included`, an unknown field,
    // and more entries than a call returns fail it.
    fails(
        &result,
        r#"{"elements": [], "matched": 5000, "tied": {"count": 5000}}"#,
        "a tie without included",
    );
    fails(
        &result,
        r#"{"elements": [], "matched": 0, "tied": null, "more": 1}"#,
        "an unknown field",
    );
    // More entries than a call returns.
    let entry = example("entry/plane-face");
    let entries = vec![entry.trim_end(); 1001].join(", ");
    let too_many = format!(r#"{{"elements": [{entries}], "matched": 1001, "tied": null}}"#);
    fails(&result, &too_many, "1001 entries");
    let at_most = too_many.replacen(&format!("{}, ", entry.trim_end()), "", 1);
    holds(&result, &at_most.replace("1001", "1000"), "1000 entries");
    // A tie returned whole, past the limit.
    let whole = Queried {
        entries: Vec::new(),
        matched: 12,
        tied: Some(Tied {
            count: 4,
            included: true,
        }),
    };
    let text = written(&whole);
    assert_eq!(
        text,
        "{\n  \"elements\": [],\n  \"matched\": 12,\n  \"tied\": {\"count\": 4, \"included\": true}\n}\n"
    );
    holds(&result, &text, "a tie returned whole");
}

#[test]
fn the_entries_examples_hold_to_the_entrys_schema_and_a_wrong_one_doesnt() {
    let entry = schema_of("elements", "result", Some("Entry"));
    for name in ["entry/plane-face", "entry/cylinder-face"] {
        holds(&entry, &example(name), name);
    }
    let plane = example("entry/plane-face");
    for (what, from, to) in [
        ("a field left out", r#""default_x": "+x""#, r#""other": 1"#),
        (
            "a kind there's no such thing as",
            r#""kind": "face""#,
            r#""kind": "solid""#,
        ),
        (
            "a type there's no such thing as",
            r#""type": "plane""#,
            r#""type": "nurbs""#,
        ),
        (
            "a default x there's no such thing as",
            r#""default_x": "+x""#,
            r#""default_x": "x""#,
        ),
        ("a normal of two numbers", "[0, 0, 1]", "[0, 1]"),
    ] {
        assert!(plane.contains(from), "{what}");
        fails(&entry, &plane.replace(from, to), what);
    }
    let first_page = example("elements/first-page");
    holds(
        &schema_of("elements", "result", None),
        &first_page,
        "first page",
    );
    let found = example("query/largest-face-up-found");
    holds(&schema_of("query", "result", None), &found, "found");
}

#[test]
fn schema_answers_the_document_and_each_command_as_committed() {
    let document: serde_json::Value =
        serde_json::from_str(&committed("document-schema.json")).unwrap();
    assert_eq!(schema("document").unwrap().schema, document);
    for (of, _) in COMMANDS {
        let committed: serde_json::Value =
            serde_json::from_str(&committed(&format!("commands/{of}.json"))).unwrap();
        let described = schema(of).unwrap();
        assert_eq!(described.schema, committed, "{of}");
        // Written as `{"schema": S}`, S as committed.
        let text = written(&described);
        let back: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back, serde_json::json!({ "schema": committed }), "{of}");
        holds(&schema_of("schema", "result", None), &text, of);
    }
    // Neither the document nor a command: the envelope's isn't a command's, and a name is
    // matched exactly.
    for of in ["documents", "envelope", "Elements", "", "query "] {
        let refused = schema(of).unwrap_err();
        assert_eq!(refused.total(), 1);
        let error = &refused.errors()[0];
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{of:?}");
        assert_eq!(error.parameter(), Some("of"));
        assert_eq!(
            error.candidates(),
            [
                "document",
                "begin",
                "commit",
                "rollback",
                "format",
                "add_feature",
                "set_field",
                "remove_feature",
                "replace_text",
                "elements",
                "query",
                "schema",
                "mass_properties",
                "render"
            ]
        );
    }
    assert_eq!(
        schema("documents").unwrap_err().errors()[0].message(),
        "of must be document or the name of a command with a schema, got documents"
    );
    assert_eq!(
        schema("query ").unwrap_err().errors()[0].message(),
        r#"of must be document or the name of a command with a schema, got "query ""#
    );
}

/// Each refusal's parameter and message, in order.
fn said(problems: &Problems) -> Vec<(String, String)> {
    problems
        .errors()
        .iter()
        .map(|e| {
            (
                e.parameter().unwrap_or("").to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

fn query_refused(text: &str) -> Vec<(String, String)> {
    said(&QueryRequest::read(&parsed(text)).unwrap_err())
}

const QUERY: &str = r#"{"session": "s1", "model": "current", "feature": null, "kind": "edge", "type": null, "stable": null, "facing": null, "measures": [], "order": null, "limit": 10}"#;

#[test]
fn a_query_requests_words_are_its_words_and_another_is_refused_with_them_as_candidates() {
    assert!(QueryRequest::read(&parsed(QUERY)).is_ok());
    let kind = QueryRequest::read(&parsed(&QUERY.replace(r#""edge""#, r#""Edge""#))).unwrap_err();
    let error = &kind.errors()[0];
    assert_eq!(
        error.message(),
        "kind must be face, edge, or vertex, got Edge"
    );
    assert_eq!(error.suggestion(), Some("give one of the kinds"));
    assert_eq!(error.candidates(), ["face", "edge", "vertex"]);
    // A value that isn't a plain word is quoted and escaped; one that isn't a string, said
    // as what it is.
    assert_eq!(
        query_refused(&QUERY.replace(r#""edge""#, r#""an edge""#)),
        [(
            "kind".to_owned(),
            r#"kind must be face, edge, or vertex, got "an edge""#.to_owned()
        )]
    );
    assert_eq!(
        query_refused(&QUERY.replace(r#""edge""#, "3")),
        [(
            "kind".to_owned(),
            "kind must be face, edge, or vertex, got 3".to_owned()
        )]
    );
    let typed = QueryRequest::read(&parsed(
        &QUERY.replace(r#""type": null"#, r#""type": "nurbs""#),
    ))
    .unwrap_err();
    assert_eq!(typed.errors()[0].parameter(), Some("type"));
    assert_eq!(typed.errors()[0].candidates().len(), 17);
    // An order by a measure there's no such thing as, and one not a bool.
    assert_eq!(
        query_refused(&QUERY.replace(
            r#""order": null"#,
            r#""order": {"by": "volume", "descending": 1}"#
        )),
        [
            (
                "order.by".to_owned(),
                "order.by must be area, length, radius, diameter, major_radius, minor_radius, \
                 or half_angle, got volume"
                    .to_owned()
            ),
            (
                "order.descending".to_owned(),
                "order.descending must be true or false, got 1".to_owned()
            ),
        ]
    );
    // `stable` is true, false, or null.
    assert_eq!(
        query_refused(&QUERY.replace(r#""stable": null"#, r#""stable": "yes""#)),
        [(
            "stable".to_owned(),
            r#"stable must be true, false, or null, got "yes""#.to_owned()
        )]
    );
}

#[test]
fn a_ranges_bounds_are_read_as_its_measures_quantity() {
    let with = |measures: &str| {
        QUERY.replace(r#""measures": []"#, &format!(r#""measures": [{measures}]"#))
    };
    // A seventh decimal is past an area's and a length's places; an angle takes nine.
    let refused = query_refused(&with(
        r#"{"measure": "area", "at_least": 1.0000001, "at_most": null}, {"measure": "length", "at_least": null, "at_most": 1.0000001}"#,
    ));
    assert_eq!(
        refused.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
        ["measures.0.at_least", "measures.1.at_most"]
    );
    assert!(
        refused[0].1.starts_with(
            "measures.0.at_least must be an area in square millimetres or null, got 1.0000001"
        ),
        "{}",
        refused[0].1
    );
    assert!(
        refused[1].1.starts_with(
            "measures.1.at_most must be a length in millimetres or null, got 1.0000001"
        ),
        "{}",
        refused[1].1
    );
    let angle = QueryRequest::read(&parsed(&with(
        r#"{"measure": "half_angle", "at_least": 1.000000001, "at_most": null}"#,
    )))
    .unwrap();
    assert_eq!(
        angle.query.measures[0].at_least,
        Some(Quantity::Angle(Angle::from_ndeg(1_000_000_001)))
    );
    // A measure there's no such thing as: refused, with the measures as candidates, its
    // bounds taken unread, whatever they are.
    let unknown = QueryRequest::read(&parsed(&with(
        r#"{"measure": "volume", "at_least": "big", "at_most": [1]}"#,
    )))
    .unwrap_err();
    assert_eq!(
        said(&unknown),
        [(
            "measures.0.measure".to_owned(),
            "measures.0.measure must be area, length, radius, diameter, major_radius, \
             minor_radius, or half_angle, got volume"
                .to_owned()
        )]
    );
    assert_eq!(unknown.errors()[0].candidates().len(), 7);
    // But a bound left out is still missing, and a key not a range's still refused.
    assert_eq!(
        query_refused(&with(
            r#"{"measure": "volume", "at_most": null, "extra": 0}"#
        )),
        [
            (
                "measures.0.measure".to_owned(),
                "measures.0.measure must be area, length, radius, diameter, major_radius, \
                 minor_radius, or half_angle, got volume"
                    .to_owned()
            ),
            (
                "measures.0.at_least".to_owned(),
                "measures.0.at_least is missing".to_owned()
            ),
            (
                "measures.0.extra".to_owned(),
                "measures.0.extra isn't a field of this request".to_owned()
            ),
        ]
    );
}

#[test]
fn more_ranges_than_there_are_measures_are_one_refusal_and_none_of_them_is_read() {
    // Eight, each wrong in every field: one reason, the command's own, and no other.
    let bad = [r#"{"measure": 1, "at_least": "x", "at_most": [], "extra": 0}"#; 8].join(", ");
    let text = QUERY.replace(r#""measures": []"#, &format!(r#""measures": [{bad}]"#));
    let refused = QueryRequest::read(&parsed(&text)).unwrap_err();
    assert_eq!(
        said(&refused),
        [(
            "measures".to_owned(),
            "measures holds at most 7 ranges, as many as there are measures, got 8".to_owned()
        )]
    );
    // A million, read no further than their count.
    let many = vec!["0"; 1_000_000].join(", ");
    let text = QUERY.replace(r#""measures": []"#, &format!(r#""measures": [{many}]"#));
    let refused = QueryRequest::read(&parsed(&text)).unwrap_err();
    assert_eq!(refused.total(), 1);
    // Seven are read, each member at fault by its place.
    let seven = ["0"; 7].join(", ");
    let text = QUERY.replace(r#""measures": []"#, &format!(r#""measures": [{seven}]"#));
    let refused = QueryRequest::read(&parsed(&text)).unwrap_err();
    assert_eq!(
        said(&refused)
            .iter()
            .map(|(p, _)| p.clone())
            .collect::<Vec<_>>(),
        (0..7).map(|i| format!("measures.{i}")).collect::<Vec<_>>()
    );
    assert_eq!(
        said(&refused)[0].1,
        "measures.0 must be an object, got a number"
    );
}

#[test]
fn an_elements_requests_cursor_is_read_strictly() {
    let with = |after: &str| {
        format!(
            r#"{{"session": "s1", "model": "current", "feature": null, "after": {after}, "limit": 2}}"#
        )
    };
    let refused = |after: &str| said(&ElementsRequest::read(&parsed(&with(after))).unwrap_err());
    assert_eq!(
        refused(r#"{"revision": 0, "feature": "k7f2qx3m", "name": "x"}"#),
        [(
            "after.revision".to_owned(),
            "after.revision must be a whole number from 1 to 18446744073709551615, got 0"
                .to_owned()
        )]
    );
    assert_eq!(
        refused(r#"{"revision": 6, "feature": null, "name": "x"}"#),
        [(
            "after.feature".to_owned(),
            "after.feature must be a feature's ID, got null".to_owned()
        )]
    );
    assert_eq!(
        refused(r#"{"revision": 6, "feature": "k7f2qx3m"}"#),
        [("after.name".to_owned(), "after.name is missing".to_owned())]
    );
    assert_eq!(
        refused(r#""k7f2qx3m.box.Face2""#),
        [(
            "after".to_owned(),
            "after must be an object, got a string".to_owned()
        )]
    );
    // Its every field wrong, and the request's, each said in the shape's order.
    let text = r#"{"session": "t1", "model": "latest", "feature": 7, "after": {"revision": -1, "feature": "K", "name": 2}, "limit": 1.5}"#;
    let all = said(&ElementsRequest::read(&parsed(text)).unwrap_err());
    assert_eq!(
        all.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
        [
            "session",
            "model",
            "feature",
            "after.revision",
            "after.feature",
            "after.name",
            "limit"
        ]
    );
}

#[test]
fn every_field_of_an_entry_is_written_from_its_own_value() {
    // An entry with every field set, each to a value of its own (wider than any one
    // element's, as the writer takes each field as it is), written as the design's layout
    // gives it: one member a line, since it holds objects.
    let model = model_of(SKETCH_AND_BOX);
    let mut entry = entries(&model, &id("k7f2qx3m"))
        .unwrap()
        .iter()
        .find(|e| e.kind == ElementKind::Edge)
        .unwrap()
        .clone();
    let name = entry.name.to_string();
    let mm = |nm: i64| Length::from_nm(nm * 1_000_000);
    entry.aliases = vec![Alias {
        name: entry.name.clone(),
        stable: true,
    }];
    entry.stable = false;
    entry.r#type = ElementType::Ellipse;
    entry.area = Some(Measure::from_millionths(1_500_000));
    entry.length = Some(Length::from_nm(2_250_000));
    entry.position = Some([mm(1), mm(2), mm(3)]);
    entry.normal = Some([0.0, 0.6, -0.8]);
    entry.centroid = Some([mm(4), mm(5), mm(6)]);
    entry.start = Some([mm(7), mm(8), mm(9)]);
    entry.end = Some([mm(10), mm(11), mm(12)]);
    entry.midpoint = Some([mm(13), mm(14), mm(15)]);
    entry.straight = Some(false);
    entry.axis = Some(EntryAxis {
        point: [mm(16), mm(17), mm(18)],
        direction: [0.0, -1.0, 0.0],
    });
    entry.radius = Some(mm(19));
    entry.diameter = Some(mm(38));
    entry.major_radius = Some(mm(20));
    entry.minor_radius = Some(mm(21));
    entry.half_angle = Some(Angle::from_ndeg(-22_500_000_000));
    entry.apex = Some([mm(23), mm(24), mm(25)]);
    entry.default_x = Some(AxisToken::MinusZ);
    let want = format!(
        r#"{{
  "name": "{name}",
  "aliases": [
    {{"name": "{name}", "stable": true}}
  ],
  "stable": false,
  "feature": "k7f2qx3m",
  "kind": "edge",
  "type": "ellipse",
  "area": 1.5,
  "length": 2.25,
  "position": [1, 2, 3],
  "normal": [0, 0.6, -0.8],
  "centroid": [4, 5, 6],
  "start": [7, 8, 9],
  "end": [10, 11, 12],
  "midpoint": [13, 14, 15],
  "straight": false,
  "axis": {{"point": [16, 17, 18], "direction": [0, -1, 0]}},
  "radius": 19,
  "diameter": 38,
  "major_radius": 20,
  "minor_radius": 21,
  "half_angle": -22.5,
  "apex": [23, 24, 25],
  "default_x": "-z"
}}
"#
    );
    assert_eq!(written(&entry), want);
    holds(
        &schema_of("elements", "result", Some("Entry")),
        &want,
        "every field",
    );
    // A vertex's kind is its own word.
    entry.kind = ElementKind::Vertex;
    assert!(written(&entry).contains(r#""kind": "vertex","#));
}

// ---- render (M4.6) ----

/// The design's block, `k7f2qx3m` the 40 x 20 x 10 block alone (its box the one the
/// design's camera is fitted to: the fake measures a box as a box), on the fake kernel.
const BLOCK: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

/// `request` rendered on `text`'s model, on the fake kernel and the fake renderer.
fn rendered(
    text: &str,
    request: &silico_core::command::render::RenderRequest,
) -> Result<silico_core::command::render::Rendering, Problems> {
    use silico_core::command::render::{RenderOn, render};
    use silico_core::fake::{FakeKernel, FakeRenderer};
    use silico_core::kernel::Kernel;
    use silico_core::meshes::{MESH_CACHE_BYTES, MeshCache};
    use silico_core::recompute::Recompute;
    let kernel = std::sync::Arc::new(FakeKernel::new(1).unwrap());
    let engine = Recompute::new(std::sync::Arc::clone(&kernel));
    let model = silico_core::command::rebuild(
        &engine,
        silico_core::command::gate(text.as_bytes()).unwrap(),
    )
    .unwrap()
    .unwrap();
    let meshes = MeshCache::new(MESH_CACHE_BYTES);
    let renderer = FakeRenderer::new();
    render(
        &model,
        request,
        &RenderOn {
            kernel: &kernel,
            generation: kernel.generation(),
            meshes: &meshes,
            renderer: &renderer,
            workspace: None,
            held: &[],
        },
    )
}

#[test]
fn the_designs_render_request_is_read_and_written_back_and_holds_to_its_schema() {
    use silico_core::command::render::RenderRequest;
    use silico_core::command::wire::render::RenderCall;
    let text = example("render/request");
    let call = RenderCall::read(&parsed(&text)).unwrap();
    assert_eq!(
        call,
        RenderCall {
            session: s1(),
            model: ModelChoice::Current,
            render: RenderRequest {
                view: Some("top_front_right".to_owned()),
                camera: None,
                width: 800,
                height: 600,
                highlight: vec!["k7f2qx3m.box.Face6".to_owned()],
                to: None,
                replace: false,
            },
        }
    );
    assert_eq!(written(&call), text);
    holds(&schema_of("render", "request", None), &text, "render");
}

#[test]
fn the_designs_render_result_is_the_commands_own_but_for_its_machines_png_bytes() {
    // The block's top_front_right at 800 x 600, its top highlighted, as the design's request
    // asks. The PNG's size is the machine's (the fake's stand-in here): it alone is set to
    // the design's before comparing, as the design says it isn't a result.
    use silico_core::command::wire::render::RenderCall;
    let call = RenderCall::read(&parsed(&example("render/request"))).unwrap();
    let done = rendered(BLOCK, &call.render).unwrap();
    let mut result = done.result.clone();
    result.png_bytes = 48_213;
    let want = example("render/result");
    assert_eq!(written(&result), want);
    holds(&schema_of("render", "result", None), &want, "render");
    // Exactly M4.1's five members, so the ID buffer can't drift into it.
    let value: serde_json::Value = serde_json::from_str(&want).unwrap();
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["camera", "height", "path", "png_bytes", "width"]);
}

#[test]
fn the_designs_render_refusals_are_the_commands_own_and_hold_to_the_error_contract() {
    use silico_core::command::wire::render::RenderCall;
    use silico_protocol::{Camera, Projection};
    let error = error_schema();
    let check = |name: &str, got: String| {
        let want = example(name);
        assert_eq!(got, want, "{name}");
        holds(&error, &want, name);
    };
    let mut call = RenderCall::read(&parsed(&example("render/request"))).unwrap();
    call.render.width = 0;
    check(
        "render/refused-width",
        the_refusal(&rendered(BLOCK, &call.render).unwrap_err()),
    );
    // A perspective eye inside the design's part (its block, the hole aside).
    let mut call = RenderCall::read(&parsed(&example("render/request"))).unwrap();
    call.render.view = None;
    call.render.highlight = vec![];
    call.render.camera = Some(Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        eye: [5.0, 5.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    });
    check(
        "render/refused-eye-inside",
        the_refusal(&rendered(super::example::EXAMPLE, &call.render).unwrap_err()),
    );
}
