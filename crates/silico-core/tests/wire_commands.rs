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
use silico_core::command::measure::DistanceAnswer;
use silico_core::command::schema::schema;
use silico_core::command::wire::elements::{ElementsRequest, QueryRequest};
use silico_core::command::wire::measure::{MassPropertiesRequest, PairRequest};
use silico_core::command::wire::schema::SchemaRequest;
use silico_core::command::wire::sketch::SketchRequest;
use silico_core::command::wire::{COMMANDS, Layout, ToWire, snapshot, snapshot_value, write};
use silico_core::command::{ModelChoice, Problems, SessionId};
use silico_core::document::json::{Node, read};
use silico_core::document::{Angle, ElementName, FeatureId, Length, Measure};
use silico_core::kernel::ElementKind;
use silico_core::sketch_states::{ConstraintState, MemberState, SketchState};
use silico_core::views::NamedView;

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
    // A name that isn't a command's (one a near spelling of one) has none.
    assert!(snapshot("read_log").is_none());
    assert!(snapshot("readlogs").is_none());
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
    let sections: [(&str, &str, &str, &[&str]); 11] = [
        (
            "### Element entries",
            "### Warnings",
            "entry",
            &["plane-face", "cylinder-face"],
        ),
        (
            "### Warnings",
            "## 3. Sessions",
            "warning",
            &["ends-not-joined"],
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
            "### `distance` (M4.5)",
            "### `angle` (M4.5)",
            "distance",
            &[
                "box-bottom-and-top",
                "box-bottom-and-top-measured",
                "rims-measured",
                "rim-and-top-measured",
                "refused-no-such-face",
                "refused-kernel-failed",
            ],
        ),
        (
            "### `angle` (M4.5)",
            "### `render` (M4.6)",
            "angle",
            &["wedge", "wedge-measured", "refused-circle"],
        ),
        (
            "### `render` (M4.6)",
            "### `read_logs` (M4.7)",
            "render",
            &["request", "result", "refused-width", "refused-eye-inside"],
        ),
        (
            "### `read_logs` (M4.7)",
            "### `export` (M4.8)",
            "read_logs",
            &["request", "result", "refused-level"],
        ),
        (
            "### `export` (M4.8)",
            "## 8. Versioning",
            "export",
            &[
                "request",
                "request-step",
                "result",
                "result-step",
                "refused-unclosed",
                "refused-tolerance",
                "refused-tolerance-null",
                "refused-tolerance-step",
                "refused-raised-face",
                "refused-exists",
                "refused-document",
                "refused-format",
            ],
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
            name: ElementName::as_sent("k7f2qx3m.box.Face2"),
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
        name: ElementName::as_sent("k7f2qx3m.box.Face2"),
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
                "suppress_feature",
                "timeline",
                "drag_begin",
                "drag_move",
                "drag_end",
                "sketch",
                "sketch_add",
                "sketch_remove",
                "sketch_split",
                "sketch_trim",
                "sketch_unjoin",
                "sketch_merge",
                "sketch_extend",
                "undo",
                "redo"
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

#[test]
fn distance_and_angle_requests_are_read_and_written_back_byte_for_byte() {
    for (name, of, first, second) in [
        (
            "distance/box-bottom-and-top",
            "distance",
            "k7f2qx3m.box.Face5",
            "k7f2qx3m.box.Face6",
        ),
        (
            "angle/wedge",
            "angle",
            "w5mb4c2n.pad.Face(G s0000003.sketch.Edge.ra000001)",
            "w5mb4c2n.pad.Face(G s0000003.sketch.Edge.ra000002)",
        ),
    ] {
        let text = example(name);
        let request = PairRequest::read(&parsed(&text)).unwrap();
        assert_eq!(
            request,
            PairRequest {
                session: s1(),
                model: ModelChoice::Current,
                elements: [first.to_owned(), second.to_owned()],
                feature: None,
            },
            "{name}"
        );
        assert_eq!(written(&request), text, "{name}");
        holds(&schema_of(of, "request", None), &text, name);
    }
    // Two names exactly, each a string, said by its place.
    let with = |elements: &str| {
        format!(
            r#"{{"session": "s1", "model": "current", "elements": {elements}, "feature": null}}"#
        )
    };
    let said = |elements: &str| -> Vec<(String, String)> {
        PairRequest::read(&parsed(&with(elements)))
            .unwrap_err()
            .errors()
            .iter()
            .map(|e| (e.parameter().unwrap().to_owned(), e.message().to_owned()))
            .collect()
    };
    assert_eq!(
        said(r#"["a"]"#),
        [(
            "elements".to_owned(),
            "elements must be a list of 2 names, got 1 of them".to_owned()
        )]
    );
    assert_eq!(
        said(r#"["a", 2]"#),
        [(
            "elements.1".to_owned(),
            "elements.1 must be a name, got 2".to_owned()
        )]
    );
    assert_eq!(
        said(r#""a""#),
        [(
            "elements".to_owned(),
            r#"elements must be a list of 2 names, got "a""#.to_owned()
        )]
    );
}

#[test]
fn distances_refusals_are_the_commands_own_as_the_design_gives_them() {
    use silico_core::command::measure::distance;
    use silico_core::fake::FakeKernel;
    let error = error_schema();
    let model = model_of(super::example::EXAMPLE);
    let kernel = FakeKernel::new(1).unwrap();
    let pair = |a: &str, b: &str| [a.to_owned(), b.to_owned()];
    // A face the box hasn't: the nearest it has as the candidate.
    let broken = distance(
        &model,
        &kernel,
        &pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face7"),
        None,
    )
    .unwrap_err();
    let text = the_refusal(&broken);
    assert_eq!(text, example("distance/refused-no-such-face"));
    holds(&error, &text, "no such face");
    // OCCT finding no distance, as a worker refuses it.
    kernel.refuse_as_worker(|op| {
        matches!(op, silico_core::kernel::Op::Distance { .. }).then(|| silico_protocol::Failure {
            refusal: silico_protocol::Refusal::DistanceNotDone,
            cause: Some("BRepExtrema_DistShapeShape: not done".to_owned()),
        })
    });
    let failed = distance(
        &model,
        &kernel,
        &pair(
            "k7f2qx3m.box.Face5",
            "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)",
        ),
        None,
    )
    .unwrap_err();
    let text = the_refusal(&failed);
    assert_eq!(text, example("distance/refused-kernel-failed"));
    holds(&error, &text, "kernel failed");
}

#[test]
fn the_measures_examples_hold_to_their_results_schemas() {
    for (name, of) in [
        ("mass_properties/every-body-measured", "mass_properties"),
        ("distance/box-bottom-and-top-measured", "distance"),
        ("distance/rims-measured", "distance"),
        ("distance/rim-and-top-measured", "distance"),
        ("angle/wedge-measured", "angle"),
    ] {
        holds(&schema_of(of, "result", None), &example(name), name);
    }
    // And a vertex named, which no example writes; a field left out fails.
    let angle = schema_of("angle", "result", None);
    holds(
        &angle,
        r#"{"angle": 135, "vertex": "w5mb4c2n.pad.Vertex(G s0000001.sketch.Vertex.pb000000)"}"#,
        "a vertex",
    );
    fails(&angle, r#"{"angle": 135}"#, "no vertex");
    fails(
        &schema_of("distance", "result", None),
        r#"{"distance": 1, "from": [0, 0, 0], "to": [0, 0, 1], "center_distance": null}"#,
        "no normal distance",
    );
    fails(
        &schema_of("distance", "result", None),
        r#"{"distance": 1, "from": [0, 0, 0], "to": [0, 0, 1], "center_distance": null, "normal_distance": null}"#,
        "no centre-to-element distance",
    );
}

#[test]
fn a_distance_from_a_centre_is_written_whole() {
    // The design's third example: M2.1's part's hole, of radius 2.5 about (20, 10), its top
    // rim against the top it bounds. They touch, so 0 apart, met at a point of the rim, on
    // both (a witness: OCCT's, the rim's start, (22.5, 10, 10)); no centres' or normal
    // distance (a circle and a plane); and the rim's centre, (20, 10, 10), in the hole, off
    // the face, so the face's nearest point is on the rim, 2.5 away: every field written, in
    // the result's order.
    let mm = |text: &str| Length::parse(text).unwrap();
    let rim = [mm("22.5"), mm("10"), mm("10")];
    let answer = DistanceAnswer {
        distance: mm("0"),
        from: rim,
        to: rim,
        center_distance: None,
        normal_distance: None,
        center_to_element: Some(mm("2.5")),
    };
    let text = example("distance/rim-and-top-measured");
    assert_eq!(
        text,
        "{\"distance\": 0, \"from\": [22.5, 10, 10], \"to\": [22.5, 10, 10], \"center_distance\": \
         null, \"normal_distance\": null, \"center_to_element\": 2.5}\n"
    );
    assert_eq!(written(&answer), text);
    // Its centre-to-element distance alone changed, the rest as written.
    let farther = DistanceAnswer {
        center_to_element: Some(mm("10.307764")),
        ..answer
    };
    assert_eq!(
        written(&farther),
        text.replace(
            "\"center_to_element\": 2.5",
            "\"center_to_element\": 10.307764"
        )
    );
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
    request: &silico_core::command::render::Render,
) -> Result<silico_core::command::render::Rendering, Problems> {
    use silico_core::command::render::{RenderOn, render};
    use silico_core::fake::{FakeKernel, FakeRenderer};
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
            meshes: &meshes,
            renderer: &renderer,
            workspace: None,
            held: &[],
            settle: &|| Ok(()),
        },
    )
}

#[test]
fn the_designs_render_request_is_read_and_written_back_and_holds_to_its_schema() {
    use silico_core::command::render::Render;
    use silico_core::command::wire::render::RenderRequest;
    let text = example("render/request");
    let call = RenderRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        call,
        RenderRequest {
            session: s1(),
            model: ModelChoice::Current,
            render: Render {
                view: NamedView::named("top_front_right"),
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
    use silico_core::command::wire::render::RenderRequest;
    let call = RenderRequest::read(&parsed(&example("render/request"))).unwrap();
    let done = rendered(BLOCK, &call.render).unwrap();
    let mut result = done.result.clone();
    result.png_bytes = 48_213;
    let want = example("render/result");
    assert_eq!(written(&result), want);
    holds(&schema_of("render", "result", None), &want, "render");
    // Exactly M4.1's five members, so the ID buffer can't drift into it; their order is the
    // example's, compared byte for byte above (serde_json's map order depends on the
    // features a build unifies, so it's sorted here).
    let value: serde_json::Value = serde_json::from_str(&want).unwrap();
    let mut keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["camera", "height", "path", "png_bytes", "width"]);
}

#[test]
fn the_designs_render_refusals_are_the_commands_own_and_hold_to_the_error_contract() {
    use silico_core::command::wire::render::RenderRequest;
    use silico_protocol::{Camera, Projection};
    let error = error_schema();
    let check = |name: &str, got: String| {
        let want = example(name);
        assert_eq!(got, want, "{name}");
        holds(&error, &want, name);
    };
    let mut call = RenderRequest::read(&parsed(&example("render/request"))).unwrap();
    call.render.width = 0;
    check(
        "render/refused-width",
        the_refusal(&rendered(BLOCK, &call.render).unwrap_err()),
    );
    // A perspective eye inside the design's part (its block, the hole aside).
    let mut call = RenderRequest::read(&parsed(&example("render/request"))).unwrap();
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

/// `request` exported from `text`'s model on the fake kernel, whose export meshes are
/// `mesh`'s, in the workspace `workspace`.
fn exported(
    text: &str,
    request: &silico_core::command::export::Export,
    workspace: &std::path::Path,
    mesh: Option<silico_protocol::ExportMesh>,
) -> Result<silico_core::command::export::Exported, Problems> {
    use silico_core::command::export::{ExportOn, export};
    use silico_core::fake::FakeKernel;
    use silico_core::recompute::Recompute;
    let kernel = std::sync::Arc::new(FakeKernel::new(1).unwrap());
    if let Some(mesh) = mesh {
        kernel.answer_instead(move |op| match op {
            silico_core::kernel::Op::ExportMesh { .. } => {
                Some(silico_protocol::Reply::ExportMesh(Box::new(mesh.clone())))
            }
            _ => None,
        });
    }
    let engine = Recompute::new(std::sync::Arc::clone(&kernel));
    let model = silico_core::command::rebuild(
        &engine,
        silico_core::command::gate(text.as_bytes()).unwrap(),
    )
    .unwrap()
    .unwrap();
    export(
        &model,
        request,
        &ExportOn {
            kernel: &kernel,
            step: &*kernel,
            document: "bracket.silico",
            workspace: Some(&silico_core::file::Workspace::new(workspace).unwrap()),
            held: &[],
            turns: &silico_core::command::export::MeshTurns::new(),
            stopped: &|| false,
            settle: &|| Ok(()),
        },
    )
}

#[test]
fn the_designs_export_request_is_read_and_written_back_and_holds_to_its_schema() {
    use silico_core::command::export::{Export, ExportFormat};
    use silico_core::command::wire::export::ExportRequest;
    let text = example("export/request");
    let call = ExportRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        call.export,
        Export {
            format: ExportFormat::ThreeMf {
                tolerance: Length::from_nm(100_000),
            },
            path: std::path::PathBuf::from("out/bracket.3mf"),
            replace: false,
            features: None,
        }
    );
    assert_eq!(written(&call), text);
    holds(&schema_of("export", "request", None), &text, "export");
}

#[test]
fn an_export_request_naming_features_and_replacing_is_read_as_it_says_and_written_back() {
    // The design's example gives `false` and `null`; one with `replace: true`, two features
    // (the second before the first, as the file's order), and another tolerance is read
    // as given, each field its own, and written back byte for byte.
    use silico_core::command::export::{Export, ExportFormat};
    use silico_core::command::wire::export::ExportRequest;
    let text = r#"{"session": "s1", "format": "3mf", "path": "out/two.3mf", "replace": true, "features": ["b2b2b2b2", "a1a1a1a1"], "tolerance": 0.025}"#;
    let call = ExportRequest::read(&parsed(text)).unwrap();
    assert_eq!(
        call.export,
        Export {
            format: ExportFormat::ThreeMf {
                tolerance: Length::from_nm(25_000),
            },
            path: std::path::PathBuf::from("out/two.3mf"),
            replace: true,
            features: Some(vec![
                silico_core::document::FeatureId::new("b2b2b2b2").unwrap(),
                silico_core::document::FeatureId::new("a1a1a1a1").unwrap(),
            ]),
        }
    );
    assert_eq!(written(&call), format!("{text}\n"));
    holds(&schema_of("export", "request", None), text, "export");
}

#[test]
fn an_export_results_faces_left_out_are_written_by_name_as_the_example_writes_none() {
    // The design's example with names in `left_out`: each written as a JSON string, its quote
    // escaped, in order, and every other byte the example's.
    use silico_core::command::export::{Exported, ExportedObject};
    let named = |text: &str| silico_core::document::ElementName::new(text).unwrap();
    let result = Exported {
        path: std::path::PathBuf::from("out/bracket.3mf"),
        bytes: 48_213,
        objects: vec![ExportedObject {
            feature: silico_core::document::FeatureId::new("f0000003").unwrap(),
            name: "Bracket".to_owned(),
            mesh: Some(silico_core::command::export::MeshWritten {
                triangles: 1204,
                vertices: 596,
                volume: silico_core::document::Measure::parse("15187.442817").unwrap(),
            }),
            left_out: vec![named("k7f2qx3m.cut.Face5"), named("Face7"), named("a\"b")],
        }],
    };
    let want = example("export/result").replace(
        r#""left_out": []"#,
        r#""left_out": ["k7f2qx3m.cut.Face5", "Face7", "a\"b"]"#,
    );
    assert_ne!(want, example("export/result"));
    assert_eq!(written(&result), want);
}

#[test]
fn the_designs_export_result_is_the_writers_and_holds_to_its_schema() {
    use silico_core::command::export::{Exported, ExportedObject};
    let want = example("export/result");
    let result = Exported {
        path: std::path::PathBuf::from("out/bracket.3mf"),
        bytes: 48_213,
        objects: vec![ExportedObject {
            feature: silico_core::document::FeatureId::new("f0000003").unwrap(),
            name: "Bracket".to_owned(),
            mesh: Some(silico_core::command::export::MeshWritten {
                triangles: 1204,
                vertices: 596,
                volume: silico_core::document::Measure::parse("15187.442817").unwrap(),
            }),
            left_out: vec![],
        }],
    };
    assert_eq!(written(&result), want);
    holds(&schema_of("export", "result", None), &want, "export");
    // Exactly M4.1's three members, and an object's six, checked as sets: their order is
    // the example's, compared byte for byte above.
    let value: serde_json::Value = serde_json::from_str(&want).unwrap();
    let mut keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["bytes", "objects", "path"]);
    let mut keys: Vec<&String> = value["objects"][0].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "feature",
            "left_out",
            "name",
            "triangles",
            "vertices",
            "volume"
        ]
    );
}

#[test]
fn the_designs_export_refusals_are_the_commands_own_and_hold_to_the_error_contract() {
    use silico_core::command::wire::export::ExportRequest;
    let error = error_schema();
    let check = |name: &str, got: String| {
        let want = example(name);
        assert_eq!(got, want, "{name}");
        holds(&error, &want, name);
    };
    let workspace = std::env::temp_dir().join(format!("silico-wire-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(workspace.join("out")).unwrap();
    let request = || ExportRequest::read(&parsed(&example("export/request"))).unwrap();
    // A tolerance of 0.
    let mut call = request();
    call.export.format = silico_core::command::export::ExportFormat::ThreeMf {
        tolerance: Length::from_nm(0),
    };
    check(
        "export/refused-tolerance",
        the_refusal(&exported(BLOCK, &call.export, &workspace, None).unwrap_err()),
    );
    // A file at the path, not to be replaced.
    std::fs::write(workspace.join("out/bracket.3mf"), b"someone's").unwrap();
    check(
        "export/refused-exists",
        the_refusal(&exported(BLOCK, &request().export, &workspace, None).unwrap_err()),
    );
    // A document as the path.
    let mut call = request();
    call.export.path = std::path::PathBuf::from("bracket.silico");
    check(
        "export/refused-document",
        the_refusal(&exported(BLOCK, &call.export, &workspace, None).unwrap_err()),
    );
    // A format it doesn't write, refused as the request is read.
    let text = example("export/request").replace("\"3mf\"", "\"stl\"");
    check(
        "export/refused-format",
        the_refusal(&ExportRequest::read(&parsed(&text)).unwrap_err()),
    );
    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
fn the_designs_step_request_and_result_are_read_and_written_back_and_hold_to_their_schemas() {
    // Fable's decision (2026-10-26): STEP takes a null tolerance, and its objects' three
    // mesh fields are null.
    use silico_core::command::export::{Export, ExportFormat, Exported, ExportedObject};
    use silico_core::command::wire::export::ExportRequest;
    let text = example("export/request-step");
    let call = ExportRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        call.export,
        Export {
            format: ExportFormat::Step,
            path: std::path::PathBuf::from("out/bracket.step"),
            replace: false,
            features: None,
        }
    );
    assert_eq!(written(&call), text);
    holds(&schema_of("export", "request", None), &text, "export");
    let want = example("export/result-step");
    let result = Exported {
        path: std::path::PathBuf::from("out/bracket.step"),
        bytes: 27_418,
        objects: vec![ExportedObject {
            feature: silico_core::document::FeatureId::new("f0000003").unwrap(),
            name: "Bracket".to_owned(),
            mesh: None,
            left_out: vec![],
        }],
    };
    assert_eq!(written(&result), want);
    holds(&schema_of("export", "result", None), &want, "export");
}

#[test]
fn a_tolerance_that_doesnt_fit_the_format_is_refused_as_the_request_is_read() {
    use silico_core::command::wire::export::ExportRequest;
    let error = error_schema();
    let read = |text: &str| ExportRequest::read(&parsed(text)).unwrap_err();
    // 3MF with null, and STEP with a length: the design's refusals, byte for byte.
    let null = example("export/request").replace("\"tolerance\": 0.1", "\"tolerance\": null");
    let want = example("export/refused-tolerance-null");
    assert_eq!(the_refusal(&read(&null)), want);
    holds(&error, &want, "refused-tolerance-null");
    let given = example("export/request-step").replace("\"tolerance\": null", "\"tolerance\": 0.1");
    let want = example("export/refused-tolerance-step");
    assert_eq!(the_refusal(&read(&given)), want);
    holds(&error, &want, "refused-tolerance-step");
    // A tolerance of the wrong type is the shape's own refusal, whatever the format.
    for text in [
        example("export/request-step").replace("\"tolerance\": null", "\"tolerance\": \"0.1\""),
        example("export/request").replace("\"tolerance\": 0.1", "\"tolerance\": true"),
    ] {
        let problems = read(&text);
        assert_eq!(problems.total(), 1, "{:?}", problems.errors());
        let e = &problems.errors()[0];
        assert_eq!(e.parameter(), Some("tolerance"), "{e:?}");
        assert!(
            e.message()
                .starts_with("tolerance must be a length in millimetres or null, got"),
            "{e:?}"
        );
    }
    // An unknown key and a mismatch: both said.
    let both = given.replace("\"replace\": false", "\"replace\": false, \"angle\": 1");
    let problems = read(&both);
    let parameters: Vec<Option<&str>> = problems.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(problems.total(), 2, "{:?}", problems.errors());
    assert!(parameters.contains(&Some("tolerance")), "{parameters:?}");
    assert!(parameters.contains(&Some("angle")), "{parameters:?}");
}

#[test]
fn the_designs_export_refusals_of_meshes_say_the_commands_words_and_hold_to_the_contract() {
    // A face the kernel didn't mesh: the command's refusal is the example's but for which
    // feature and face it names (the example's are illustrative: a pocket's face of
    // another feature than the one named). The raised face's is pinned on the real sphere
    // the example is of (silico-kernel's tests/adversarial_export.rs).
    let error = error_schema();
    for name in ["export/refused-unclosed", "export/refused-raised-face"] {
        holds(&error, &example(name), name);
    }
    let workspace =
        std::env::temp_dir().join(format!("silico-wire-export-mesh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(workspace.join("out")).unwrap();
    let mut open = super::export_report::tetra();
    open.triangles.pop();
    open.face_triangles[3] = 0;
    let call = silico_core::command::wire::export::ExportRequest::read(&parsed(&example(
        "export/request",
    )))
    .unwrap();
    let got = the_refusal(&exported(BLOCK, &call.export, &workspace, Some(open)).unwrap_err());
    let mut got: serde_json::Value = serde_json::from_str(&got).unwrap();
    let mut want: serde_json::Value =
        serde_json::from_str(&example("export/refused-unclosed")).unwrap();
    assert_eq!(got["feature"], "k7f2qx3m");
    assert_eq!(got["elements"].as_array().unwrap().len(), 1);
    for value in [&mut got, &mut want] {
        let fields = value.as_object_mut().unwrap();
        fields.remove("feature");
        fields.remove("elements");
    }
    assert_eq!(got, want);
    assert!(!workspace.join("out/bracket.3mf").exists());
    let _ = std::fs::remove_dir_all(&workspace);
}

/// A render request around `camera`, a JSON object's text.
fn with_camera(camera: &str) -> String {
    format!(
        r#"{{"session": "s1", "model": "current", "view": null, "camera": {camera}, "width": 10, "height": 10, "highlight": [], "to": null, "replace": false}}"#
    )
}

#[test]
fn a_perspective_camera_is_read_from_its_digits_and_written_back_as_given() {
    use silico_core::command::wire::render::RenderRequest;
    use silico_protocol::{Camera, Projection};
    // Eye and target as lengths, to the nanometre; the angle to the nanodegree; up as a
    // direction. Read and written back: the same text.
    let camera = r#"{"projection": "perspective", "eye": [1, 2.000001, -3], "target": [4, 5, 6], "up": [0, 0, 1], "view_height": null, "field_of_view": 37.123456789}"#;
    let text = with_camera(camera);
    let call = RenderRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        call.render.camera,
        Some(Camera {
            projection: Projection::Perspective {
                field_of_view: 37.123_456_789,
            },
            eye: [1.0, 2.000_001, -3.0],
            target: [4.0, 5.0, 6.0],
            up: [0.0, 0.0, 1.0],
        })
    );
    let back: serde_json::Value = serde_json::from_str(&written(&call)).unwrap();
    let given: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(back, given);
    holds(&schema_of("render", "request", None), &text, "perspective");
}

#[test]
fn a_cameras_fields_that_dont_fit_its_projection_or_aren_t_numbers_are_refused_by_name() {
    use silico_core::command::wire::render::RenderRequest;
    let field = |camera: &str| {
        let refused = RenderRequest::read(&parsed(&with_camera(camera))).unwrap_err();
        refused
            .errors()
            .iter()
            .map(|e| e.parameter().unwrap_or_default().to_owned())
            .collect::<Vec<_>>()
    };
    let ortho = |rest: &str| {
        format!(
            r#"{{"projection": "orthographic", "eye": [0, -10, 0], "target": [0, 0, 0], "up": [0, 0, 1], {rest}}}"#
        )
    };
    let persp = |rest: &str| {
        format!(
            r#"{{"projection": "perspective", "eye": [0, -10, 0], "target": [0, 0, 0], "up": [0, 0, 1], {rest}}}"#
        )
    };
    assert_eq!(
        field(&ortho(r#""view_height": 10, "field_of_view": 40"#)),
        ["camera.field_of_view"]
    );
    assert_eq!(
        field(&ortho(r#""view_height": null, "field_of_view": null"#)),
        ["camera.view_height"]
    );
    assert_eq!(
        field(&persp(r#""view_height": 10, "field_of_view": 40"#)),
        ["camera.view_height"]
    );
    assert_eq!(
        field(&persp(r#""view_height": null, "field_of_view": null"#)),
        ["camera.field_of_view"]
    );
    // A seventh decimal of a millimetre, and a member that isn't a number, each named.
    assert_eq!(
        field(
            r#"{"projection": "orthographic", "eye": [0, -10, 0.0000001], "target": [0, "0", 0], "up": [0, 0, 1], "view_height": 10, "field_of_view": null}"#
        ),
        ["camera.eye.2", "camera.target.1"]
    );
    // The article read as the projection is: "an orthographic camera".
    let refused = RenderRequest::read(&parsed(&with_camera(&ortho(
        r#""view_height": null, "field_of_view": null"#,
    ))))
    .unwrap_err();
    assert!(
        refused.errors()[0]
            .message()
            .contains("for an orthographic camera"),
        "{refused:?}"
    );
}

#[test]
fn every_reason_a_render_request_is_refused_is_found() {
    use silico_core::command::wire::render::RenderRequest;
    let text = r#"{"session": "s1", "model": "current", "view": "isometric", "camera": null, "width": 1.5, "height": 10, "highlight": ["ok", 3], "to": null, "replace": false}"#;
    let refused = RenderRequest::read(&parsed(text)).unwrap_err();
    let fields: Vec<Option<&str>> = refused.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(fields, [Some("view"), Some("width"), Some("highlight.1")]);
    assert_eq!(refused.errors()[0].candidates().len(), 14);
}

#[test]
fn a_render_request_of_1000_highlights_is_read_and_one_of_1001_refused_as_it_is_read() {
    // M4.1 §7: at most 1,000 elements highlighted. The reader takes exactly that many, and
    // refuses one more itself, naming `highlight`, before any is read as a string.
    use silico_core::command::wire::render::RenderRequest;
    let request = |count: usize| {
        let names = vec![r#""k7f2qx3m.box.Face1""#; count].join(", ");
        format!(
            r#"{{"session": "s1", "model": "current", "view": "front", "camera": null, "width": 10, "height": 10, "highlight": [{names}], "to": null, "replace": false}}"#
        )
    };
    let call = RenderRequest::read(&parsed(&request(1000))).unwrap();
    assert_eq!(call.render.highlight.len(), 1000);
    let refused = RenderRequest::read(&parsed(&request(1001))).unwrap_err();
    assert_eq!(refused.total(), 1, "{refused:?}");
    let e = &refused.errors()[0];
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("highlight")),
        "{e:?}"
    );
    assert!(
        e.message().starts_with("1001 elements highlighted"),
        "{e:?}"
    );
}

#[test]
fn a_perspective_result_writes_its_angle_and_no_height() {
    use silico_core::command::render::{AnsweredCamera, RenderResult};
    use silico_protocol::{Camera, Projection};
    let camera = AnsweredCamera::of(&Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        eye: [0.0, -10.0, 0.0],
        target: [0.0; 3],
        up: [0.0, 0.0, 1.0],
    })
    .unwrap();
    let out = written(&RenderResult {
        png_bytes: 1,
        path: None,
        width: 10,
        height: 10,
        camera,
    });
    assert!(
        out.contains(r#""view_height": null, "field_of_view": 40"#),
        "{out}"
    );
    holds(
        &schema_of("render", "result", None),
        &out,
        "perspective result",
    );
}

#[test]
fn the_designs_read_logs_request_is_read_and_written_back_and_holds_to_its_schema() {
    use silico_core::command::logs::{LogLevel, LogQuery};
    use silico_core::command::wire::logs::ReadLogsRequest;
    let text = example("read_logs/request");
    let call = ReadLogsRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        call,
        ReadLogsRequest {
            session: s1(),
            query: LogQuery {
                level: LogLevel::Info,
                feature: Some(id("f0000001")),
                after: Some(40),
                limit: 200,
            },
        }
    );
    assert_eq!(written(&call), text);
    holds(&schema_of("read_logs", "request", None), &text, "read_logs");
    // From the oldest, of every feature: both null, and read so.
    let every =
        r#"{"session": "s1", "level": "trace", "feature": null, "after": null, "limit": 1}"#;
    let call = ReadLogsRequest::read(&parsed(every)).unwrap();
    assert_eq!(
        call.query,
        LogQuery {
            level: LogLevel::Trace,
            feature: None,
            after: None,
            limit: 1,
        }
    );
    assert_eq!(written(&call), format!("{every}\n"));
    holds(&schema_of("read_logs", "request", None), every, "every");
}

#[test]
fn the_designs_read_logs_result_is_the_writers_and_holds_to_its_schema() {
    use silico_core::command::logs::{LogEvent, LogLevel, Logs};
    let spans = || {
        [
            "check",
            "rebuild",
            "step feature=f0000001",
            "kernel op=fillet",
        ]
        .map(str::to_owned)
        .to_vec()
    };
    let logs = Logs {
        events: vec![
            LogEvent {
                seq: 41,
                level: LogLevel::Info,
                at_ms: 1518,
                spans: spans(),
                feature: Some(id("f0000001")),
                message: "fillet of 1 edge, radius 6".to_owned(),
            },
            LogEvent {
                seq: 42,
                level: LogLevel::Warn,
                at_ms: 1520,
                spans: spans(),
                feature: Some(id("f0000001")),
                message: "fillet failed: StdFail_NotDone: BRepFilletAPI_MakeFillet".to_owned(),
            },
        ],
        next: 42,
        dropped: 0,
    };
    let want = example("read_logs/result");
    assert_eq!(written(&logs), want);
    holds(&schema_of("read_logs", "result", None), &want, "read_logs");
    // An event of no feature, as null; each other field from its own value.
    let other = Logs {
        events: vec![LogEvent {
            seq: 7,
            level: LogLevel::Trace,
            at_ms: 3,
            spans: vec!["open".to_owned()],
            feature: None,
            message: "m".to_owned(),
        }],
        next: 9,
        dropped: 5,
    };
    let text = written(&other);
    assert_eq!(
        text,
        "{\n  \"events\": [\n    {\"seq\": 7, \"level\": \"trace\", \"at_ms\": 3, \"spans\": [\"open\"], \"feature\": null, \"message\": \"m\"}\n  ],\n  \"next\": 9,\n  \"dropped\": 5\n}\n"
    );
    holds(&schema_of("read_logs", "result", None), &text, "other");
    // A level that isn't one fails the schema, as the reader refuses it.
    fails(
        &schema_of("read_logs", "result", None),
        &text.replace("\"trace\"", "\"verbose\""),
        "a verbose event",
    );
}

#[test]
fn the_designs_read_logs_refusal_is_the_readers_own_and_holds_to_the_error_contract() {
    use silico_core::command::wire::logs::ReadLogsRequest;
    let text = example("read_logs/request").replace("\"info\"", "\"verbose\"");
    let refused = ReadLogsRequest::read(&parsed(&text)).unwrap_err();
    let want = example("read_logs/refused-level");
    assert_eq!(the_refusal(&refused), want);
    holds(&error_schema(), &want, "read_logs/refused-level");
}

#[test]
fn a_read_logs_cursor_is_a_whole_number_or_null_and_its_other_fields_are_read_strictly() {
    use silico_core::command::wire::logs::ReadLogsRequest;
    let with = |field: &str, value: &str| {
        let text = example("read_logs/request");
        let mut value_node: serde_json::Value = serde_json::from_str(&text).unwrap();
        value_node[field] = serde_json::from_str(value).unwrap();
        ReadLogsRequest::read(&parsed(&value_node.to_string()))
    };
    let said = |problems: Problems| -> Vec<(Option<String>, String)> {
        problems
            .errors()
            .iter()
            .map(|e| (e.parameter().map(str::to_owned), e.message().to_owned()))
            .collect()
    };
    assert_eq!(
        said(with("after", "-1").unwrap_err()),
        [(
            Some("after".to_owned()),
            "after must be a whole number from 0 to 18446744073709551615 or null, got -1"
                .to_owned()
        )]
    );
    assert_eq!(
        said(with("after", "\"40\"").unwrap_err()),
        [(
            Some("after".to_owned()),
            "after must be a whole number from 0 to 18446744073709551615 or null, got \"40\""
                .to_owned()
        )]
    );
    assert_eq!(with("after", "0").unwrap().query.after, Some(0));
    assert_eq!(
        with("after", "18446744073709551615").unwrap().query.after,
        Some(u64::MAX)
    );
    assert_eq!(
        said(with("feature", "\"F0000001\"").unwrap_err())[0]
            .0
            .as_deref(),
        Some("feature")
    );
    // A limit's range is the command's to say (it says the bound); its shape is the reader's.
    assert_eq!(with("limit", "0").unwrap().query.limit, 0);
    assert_eq!(
        said(with("limit", "1.5").unwrap_err())[0].0.as_deref(),
        Some("limit")
    );
    // Every field required, and no other taken.
    let missing = r#"{"session": "s1", "level": "info", "feature": null, "limit": 1}"#;
    assert_eq!(
        said(ReadLogsRequest::read(&parsed(missing)).unwrap_err()),
        [(Some("after".to_owned()), "after is missing".to_owned())]
    );
    let extra = r#"{"session": "s1", "level": "info", "feature": null, "after": null, "limit": 1, "span": "x"}"#;
    assert_eq!(
        said(ReadLogsRequest::read(&parsed(extra)).unwrap_err())[0]
            .0
            .as_deref(),
        Some("span")
    );
}

/// M5.0's design's JSON code blocks between the headings `from` and `to`, in order.
fn m5_design(from: &str, to: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m5.0-app.md");
    let doc = std::fs::read_to_string(&path).unwrap();
    let section = &doc[doc.find(from).unwrap()..doc.find(to).unwrap()];
    section
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

#[test]
fn the_sketch_examples_are_the_designs_own_text() {
    // docs/design/m5.0-app.md's `sketch` section: its request, and its result, the bracket's
    // base holes.
    let blocks = m5_design("### `sketch` (M5.10)", "### The sketch edits (M5.10)");
    assert_eq!(blocks.len(), 2);
    assert_eq!(format!("{}\n", blocks[0]), example("sketch/request"));
    assert_eq!(format!("{}\n", blocks[1]), example("sketch/result"));
}

#[test]
fn a_sketch_request_is_read_written_back_and_holds_to_its_schema_and_its_result_too() {
    let text = example("sketch/request");
    let request = SketchRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        SketchRequest {
            session: s1(),
            model: ModelChoice::Current,
            sketch: id("s0000002"),
        }
    );
    assert_eq!(written(&request), text);
    holds(
        &schema_of("sketch", "request", None),
        &text,
        "sketch request",
    );
    let published = r#"{"session": "s4", "model": "published", "sketch": "k7f2qx3m"}"#;
    assert_eq!(
        SketchRequest::read(&parsed(published)).unwrap(),
        SketchRequest {
            session: SessionId::parse("s4").unwrap(),
            model: ModelChoice::Published,
            sketch: id("k7f2qx3m"),
        }
    );
    // The result, and what isn't one: a member's state not a state's word, a constraint
    // with a field its type hasn't, a member with no state, a region's hole not a loop.
    let result = example("sketch/result");
    let schema = schema_of("sketch", "result", None);
    holds(&schema, &result, "sketch result");
    for (from, to, what) in [
        (
            r#""x": 40, "y": 12, "construction": false, "state": "fully_defined""#,
            r#""x": 40, "y": 12, "construction": false, "state": "satisfied""#,
            "a point satisfied",
        ),
        (
            r#""type": "equal", "a": "cb000001", "b": "cb000002", "state": "satisfied""#,
            r#""type": "equal", "a": "cb000001", "b": "cb000002", "value": 1, "state": "satisfied""#,
            "an equal with a value",
        ),
        (
            r#""type": "equal", "a": "cb000001", "b": "cb000002", "state": "satisfied""#,
            r#""type": "equal", "a": "cb000001", "b": "cb000002""#,
            "a constraint with no state",
        ),
        (
            r#""center": "pb000002", "radius": 2.75, "construction": false, "state": "fully_defined"}"#,
            r#""center": "pb000002", "radius": 2.75, "construction": false}"#,
            "a circle with no state",
        ),
        (
            r#""state": "fully_defined",
  "dofs": 0"#,
            r#""state": "satisfied",
  "dofs": 0"#,
            "a sketch satisfied",
        ),
        (
            r#"{"outer": ["cb000002"], "holes": []}"#,
            r#"{"outer": ["cb000002"], "holes": ["cb000001"]}"#,
            "a hole of one curve, not a loop",
        ),
        (r#""dofs": 0"#, r#""dofs": 0.5"#, "half a degree of freedom"),
        (
            r#"  "profile_problems_total": 0,
"#,
            "",
            "no profile problems' total",
        ),
    ] {
        assert_eq!(result.matches(from).count(), 1, "{what}");
        fails(&schema, &result.replacen(from, to, 1), what);
    }
    // Every member's state's word, and a sketch's, and negative degrees of freedom.
    for (from, to) in [
        (
            r#""state": "fully_defined",
  "dofs": 0"#,
            r#""state": "over_defined",
  "dofs": -1"#,
        ),
        (
            r#""cb000002", "state": "satisfied""#,
            r#""cb000002", "state": "partially_redundant""#,
        ),
    ] {
        holds(&schema, &result.replacen(from, to, 1), to);
    }
}

#[test]
fn the_sketch_states_words_in_its_snapshot_are_the_states_own() {
    let sketch = snapshot_value("sketch").unwrap();
    let defs = &sketch["result"]["$defs"];
    let words = |def: &serde_json::Value| -> Vec<String> {
        def["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap().to_owned())
            .collect()
    };
    // SolidWorks' five, its three for an entity, and item 32's five for a constraint, by
    // hand, and each the core's own word for it.
    assert_eq!(
        words(&defs["SketchState"]),
        [
            "under_defined",
            "fully_defined",
            "over_defined",
            "no_solution",
            "invalid"
        ]
    );
    assert_eq!(
        words(&defs["SketchState"]),
        SketchState::ALL.map(|s| s.as_str().to_owned())
    );
    assert_eq!(
        words(&defs["MemberState"]),
        ["under_defined", "fully_defined", "over_defined"]
    );
    assert_eq!(
        words(&defs["MemberState"]),
        MemberState::ALL.map(|s| s.as_str().to_owned())
    );
    assert_eq!(
        words(&defs["ConstraintState"]),
        [
            "satisfied",
            "conflicting",
            "redundant",
            "partially_redundant",
            "unsolved"
        ]
    );
    assert_eq!(
        words(&defs["ConstraintState"]),
        ConstraintState::ALL.map(|s| s.as_str().to_owned())
    );
    // A constraint read is each of the document's forms, its ID and state beside its type's
    // fields: as many as the document has types.
    let forms = defs["SketchConstraintRead"]["oneOf"].as_array().unwrap();
    let types: Vec<&str> = forms
        .iter()
        .map(|f| f["properties"]["type"]["const"].as_str().unwrap())
        .collect();
    assert_eq!(types, silico_core::document::CONSTRAINT_TYPES);
    assert_eq!(sketch["request"]["title"], "SketchRequest");
    assert_eq!(sketch["result"]["title"], "SketchResult");
}
