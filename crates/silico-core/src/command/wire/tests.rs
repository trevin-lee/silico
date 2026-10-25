//! The envelope's snapshot and the design's examples of it (section 2), byte for byte; and
//! the strict request reader. Expected texts are the design's own (docs/design/
//! m4.1-command-api.md), copied into `tests/contract/commands/examples/`, which is protected:
//! the writer must give them as they are.
//!
//! **Recording a snapshot.** Run these tests with `SILICO_CONTRACT_RECORD=1` set, and the
//! envelope's schema, `envelope.json`, is written first from what the code gives, by its own
//! test alone: only for a change the maintainer has approved, since `tests/contract/` is
//! protected. The examples are the design's text, which no recording writes.

use std::path::PathBuf;

use super::{Json, Layout, RequestFields, ToWire, response, snapshot, snapshot_value, write};
use crate::command::{
    ModelChoice, ModelRead, Problems, Response, Revision, SessionId, SessionInfo,
};
use crate::document::json::read;
use crate::{Error, ErrorCode};

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract/commands")
        .join(name)
}

/// The committed file `name`, or, with `SILICO_CONTRACT_RECORD` set, `text`, what the code
/// gives, written to it first (the maintainer's to approve: the directory is protected; see
/// the module's doc). Only a snapshot's own test records it.
fn recorded(name: &str, text: &str) -> String {
    let path = contract(name);
    if std::env::var_os("SILICO_CONTRACT_RECORD").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    committed(name)
}

/// The committed file `name`, read and never written: the design's examples, and a
/// snapshot read by any test but its own.
fn committed(name: &str) -> String {
    let path = contract(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn revision(n: u64) -> Revision {
    let mut r = Revision::OPENED;
    for _ in 1..n {
        r = r.next().unwrap();
    }
    r
}

fn session(revision_number: u64, transaction: bool) -> SessionInfo {
    SessionInfo {
        id: SessionId::parse("s1").unwrap(),
        path: "bracket.silico".to_owned(),
        revision: revision(revision_number),
        transaction,
        reloaded: false,
    }
}

/// `begin`'s result, as the design's first example shows it.
struct Began(Revision);

impl ToWire for Began {
    fn to_wire(&self) -> Json<'_> {
        Json::object([("base", self.0.to_wire())])
    }
}

/// `check`'s result with no problems and no warnings, as the second example shows it.
struct Clean;

impl ToWire for Clean {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("problems", Json::array([])),
            ("problems_total", Json::whole(0)),
            ("warnings", Json::array([])),
            ("warnings_total", Json::whole(0)),
        ])
    }
}

/// The design's three envelopes: a command that ran, one that read a model, and one
/// refused.
fn examples() -> [(&'static str, String); 3] {
    let ran = Response {
        session: Some(session(4, false)),
        model: None,
        result: Ok(Began(revision(4))),
    };
    let read_a_model = Response {
        session: Some(session(7, true)),
        model: Some(ModelRead {
            revision: revision(6),
            of: ModelChoice::Published,
            fault: None,
        }),
        result: Ok(Clean),
    };
    let refused: Response<Clean> = Response {
        session: Some(session(4, true)),
        model: None,
        result: Err(Error::new(
            ErrorCode::InvalidInput,
            "a transaction is already open in this session",
        )
        .with_parameter("session")
        .with_suggestion("commit or roll back the open transaction, then begin again")
        .into()),
    };
    [
        (
            "examples/envelope/ran.json",
            write(&response(&ran), Layout::Canonical).unwrap(),
        ),
        (
            "examples/envelope/read-a-model.json",
            write(&response(&read_a_model), Layout::Canonical).unwrap(),
        ),
        (
            "examples/envelope/refused.json",
            write(&response(&refused), Layout::Canonical).unwrap(),
        ),
    ]
}

#[test]
fn the_envelopes_schema_is_the_committed_snapshot() {
    let text = snapshot("envelope").unwrap().unwrap();
    assert_eq!(text, recorded("envelope.json", &text));
    assert!(snapshot("no_such_command").is_none());
}

#[test]
fn the_designs_envelopes_are_written_byte_for_byte_and_hold_to_the_snapshot() {
    let schema: serde_json::Value = serde_json::from_str(&committed("envelope.json")).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for (name, text) in examples() {
        // The file holds the response and a newline, as the error contract's files do.
        let file = committed(name);
        assert_eq!(format!("{text}\n"), file, "{name}");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&value)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "{name}: {errors:?}");
    }
}

#[test]
fn on_one_line_an_envelope_has_the_same_members_in_the_same_order() {
    let response = Response {
        session: Some(session(4, false)),
        model: None,
        result: Ok(Began(revision(4))),
    };
    assert_eq!(
        write(&super::response(&response), Layout::OneLine).unwrap(),
        r#"{"ok": true, "session": {"id": "s1", "path": "bracket.silico", "revision": 4, "transaction": false, "reloaded": false}, "model": null, "result": {"base": 4}}"#
    );
}

#[test]
fn a_refusal_lists_its_first_hundred_errors_and_counts_them_all() {
    let errors: Vec<Error> = (0..150)
        .map(|i| Error::new(ErrorCode::InvalidInput, format!("problem {i}")))
        .collect();
    let refused: Response<Clean> = Response {
        session: None,
        model: None,
        result: Err(Problems::of(errors)),
    };
    let value: serde_json::Value =
        serde_json::from_str(&write(&response(&refused), Layout::Canonical).unwrap()).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["session"], serde_json::Value::Null);
    assert_eq!(value["total"], 150);
    // The first hundred, in order.
    let listed = value["errors"].as_array().unwrap();
    assert_eq!(listed.len(), 100);
    assert_eq!(listed[0]["message"], "problem 0");
    assert_eq!(listed[99]["message"], "problem 99");
}

#[test]
fn a_session_id_is_s_and_a_number_from_1() {
    assert_eq!(SessionId::parse("s1").map(SessionId::number), Some(1));
    assert_eq!(SessionId::parse("s42").map(SessionId::number), Some(42));
    for bad in [
        "",
        "s",
        "s0",
        "s01",
        "1",
        "S1",
        "s-1",
        "s1.0",
        "s 1",
        "s18446744073709551616",
    ] {
        assert_eq!(SessionId::parse(bad), None, "{bad:?}");
    }
}

/// What a request's fields refuse: each error's parameter and message.
fn refusals(problems: &Problems) -> Vec<(String, String)> {
    problems
        .errors()
        .iter()
        .map(|e| {
            assert_eq!(e.code(), ErrorCode::InvalidInput);
            (
                e.parameter().unwrap_or_default().to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_request_is_read_strictly_and_every_reason_is_said_in_order() {
    let node =
        read(br#"{"session": "s0", "model": "latest", "limit": 1.5, "feature": 7, "extra": true}"#)
            .unwrap();
    let refused = RequestFields::read(&node, |fields| {
        assert_eq!(fields.session(), None);
        assert_eq!(fields.model(), None);
        assert_eq!(fields.u32("limit"), None);
        assert_eq!(fields.nullable_feature("feature"), None);
        assert_eq!(fields.bool("write"), None);
        Some(())
    })
    .unwrap_err();
    assert_eq!(
        refusals(&refused),
        [
            (
                "session".to_owned(),
                r#"session must be a session's ID, s1, s2, ..., got "s0""#.to_owned()
            ),
            (
                "model".to_owned(),
                "model must be current or published, got latest".to_owned()
            ),
            (
                "limit".to_owned(),
                "limit must be a whole number from 0 to 4294967295, got 1.5".to_owned()
            ),
            (
                "feature".to_owned(),
                "feature must be a feature's ID or null, got 7".to_owned()
            ),
            ("write".to_owned(), "write is missing".to_owned()),
            (
                "extra".to_owned(),
                "extra isn't a field of this request".to_owned()
            ),
        ]
    );
}

/// A request read as `begin`'s might be: its fields, every one taken.
#[derive(Debug, PartialEq)]
struct Begin<'a> {
    session: SessionId,
    model: ModelChoice,
    limit: u32,
    feature: Option<crate::document::FeatureId>,
    features: Option<Vec<crate::document::FeatureId>>,
    write: bool,
    name: Option<&'a str>,
}

#[test]
fn fields_of_their_types_are_read_and_null_only_where_the_shape_allows_it() {
    let node = read(
        br#"{"session": "s3", "model": "published", "limit": 1000, "feature": null, "features": ["k7f2qx3m", "a91cw0de"], "write": false, "name": null}"#,
    )
    .unwrap();
    let begin = RequestFields::read(&node, |fields| {
        Some(Begin {
            session: fields.session()?,
            model: fields.model()?,
            limit: fields.u32("limit")?,
            feature: fields.nullable_feature("feature")?,
            features: fields.nullable_feature_ids("features")?,
            write: fields.bool("write")?,
            name: fields.nullable_string("name")?,
        })
    })
    .unwrap();
    assert_eq!(begin.session.number(), 3);
    assert_eq!(begin.model, ModelChoice::Published);
    assert_eq!(begin.limit, 1000);
    assert_eq!(begin.feature, None);
    assert_eq!(
        begin
            .features
            .unwrap()
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        ["k7f2qx3m", "a91cw0de"]
    );
    assert!(!begin.write);
    assert_eq!(begin.name, None);
    // A bool isn't null.
    let node = read(br#"{"write": null}"#).unwrap();
    let refused = RequestFields::read(&node, |fields| fields.bool("write")).unwrap_err();
    assert_eq!(
        refusals(&refused),
        [(
            "write".to_owned(),
            "write must be true or false, got null".to_owned()
        )]
    );
}

#[test]
fn each_reader_gives_a_value_it_accepts_unchanged_and_refuses_another_kind() {
    use crate::document::{FeatureId, Length};
    let node = read(
        br#"{"write": true, "model": "current", "feature": "k7f2qx3m", "features": null, "facing": null, "name": "bracket", "title": "plate", "radius": 123456.123456, "base": 1, "last": 18446744073709551615}"#,
    )
    .unwrap();
    let got = RequestFields::read(&node, |fields| {
        Some((
            fields.bool("write")?,
            fields.model()?,
            fields.nullable_feature("feature")?,
            fields.nullable_feature_ids("features")?,
            fields.nullable_object("facing", |facing| facing.u32("within"))?,
            fields.string("name")?,
            fields.nullable_string("title")?,
            fields.length("radius")?,
            fields.revision("base")?.number(),
            fields.revision("last")?.number(),
        ))
    })
    .unwrap();
    assert_eq!(
        got,
        (
            true,
            ModelChoice::Current,
            Some(FeatureId::new("k7f2qx3m").unwrap()),
            None,
            None,
            "bracket",
            Some("plate"),
            // 123456.123456 mm, to the nanometre.
            Length::from_nm(123_456_123_456),
            1,
            u64::MAX
        )
    );
    // Another kind of each, or a value past what it takes.
    let node = read(br#"{"name": null, "title": 5, "radius": 1.0000001, "base": 0}"#).unwrap();
    let refused = RequestFields::read(&node, |fields| {
        assert_eq!(fields.string("name"), None);
        assert_eq!(fields.nullable_string("title"), None);
        assert_eq!(fields.length("radius"), None);
        assert_eq!(fields.revision("base"), None);
        Some(())
    })
    .unwrap_err();
    assert_eq!(
        refusals(&refused),
        [
            ("name", "name must be a string, got null"),
            ("title", "title must be a string or null, got 5"),
            (
                "radius",
                "radius must be a length in millimetres, got 1.0000001, which has more than 6 decimals"
            ),
            (
                "base",
                "base must be a whole number from 1 to 18446744073709551615, got 0"
            ),
        ]
        .map(|(at, message)| (at.to_owned(), message.to_owned()))
    );
}

#[test]
fn a_session_reloaded_outside_a_transaction_says_both() {
    // `transaction` and `reloaded` each written from its own field, so neither is a
    // constant or the other.
    let mut reloaded = session(2, false);
    reloaded.reloaded = true;
    let answer = Response {
        session: Some(reloaded),
        model: None,
        result: Ok(Began(revision(2))),
    };
    assert_eq!(
        write(&response(&answer), Layout::OneLine).unwrap(),
        r#"{"ok": true, "session": {"id": "s1", "path": "bracket.silico", "revision": 2, "transaction": false, "reloaded": true}, "model": null, "result": {"base": 2}}"#
    );
}

#[test]
fn a_value_is_had_only_from_a_request_with_nothing_refused() {
    // Every field read, but a key not taken: refused, though the reader gave a value.
    let node = read(br#"{"write": true, "extra": 1}"#).unwrap();
    let refused = RequestFields::read(&node, |fields| fields.bool("write")).unwrap_err();
    assert_eq!(
        refusals(&refused),
        [(
            "extra".to_owned(),
            "extra isn't a field of this request".to_owned()
        )]
    );
    // A reader that gives nothing, refusing nothing, is a bug, said as one.
    let node = read(br#"{"write": true}"#).unwrap();
    let bug = RequestFields::read(&node, |fields| {
        fields.bool("write")?;
        None::<()>
    })
    .unwrap_err();
    assert_eq!(bug.total(), 1);
    assert_eq!(bug.errors()[0].code(), ErrorCode::Internal);
    assert_eq!(
        bug.errors()[0].suggestion(),
        Some(crate::worker::SILICO_BUG)
    );
}

#[test]
fn a_whole_number_is_digits_only_from_0_to_u32_max() {
    // (A leading zero, `01`, isn't JSON: the reader refuses it before a field is read.)
    for (text, want) in [
        ("0", Some(0)),
        ("1", Some(1)),
        ("4294967295", Some(u32::MAX)),
        ("4294967296", None),
        ("-1", None),
        ("1e3", None),
        ("1.0", None),
        ("-0", None),
    ] {
        let doc = format!(r#"{{"limit": {text}}}"#);
        let node = read(doc.as_bytes()).unwrap();
        assert_eq!(
            RequestFields::read(&node, |fields| fields.u32("limit")).ok(),
            want,
            "{text}"
        );
    }
}

#[test]
fn a_list_member_and_a_nested_field_are_named_by_their_paths() {
    let node =
        read(br#"{"features": ["k7f2qx3m", 5, "NOT AN ID"], "facing": {"within": 1, "extra": 2}}"#)
            .unwrap();
    let refused = RequestFields::read(&node, |fields| {
        assert_eq!(fields.nullable_feature_ids("features"), None);
        // `within` is read, but `extra` isn't taken: the object is refused.
        let mut within = None;
        let facing = fields.nullable_object("facing", |facing| {
            within = facing.u32("within");
            within
        });
        assert_eq!((facing, within), (None, Some(1)));
        Some(())
    })
    .unwrap_err();
    assert_eq!(
        refusals(&refused),
        [
            (
                "features.1".to_owned(),
                "features.1 must be a feature's ID, got 5".to_owned()
            ),
            (
                "features.2".to_owned(),
                r#"features.2 must be a feature's ID, got "NOT AN ID""#.to_owned()
            ),
            (
                "facing.extra".to_owned(),
                "facing.extra isn't a field of this request".to_owned()
            ),
        ]
    );
    // The request itself must be an object.
    let node = read(b"[1]").unwrap();
    let refused = RequestFields::read(&node, |_| Some(())).unwrap_err();
    assert_eq!(
        refused.errors()[0].message(),
        "the request must be an object, got an array"
    );
}

#[test]
fn a_nested_object_read_whole_gives_its_value() {
    let node = read(br#"{"facing": {"within": 1}, "around": null, "near": {"by": 2}}"#).unwrap();
    let got = RequestFields::read(&node, |fields| {
        Some((
            fields.object("facing", |facing| facing.u32("within"))?,
            fields.nullable_object("around", |around| around.u32("by"))?,
            fields.nullable_object("near", |near| near.u32("by"))?,
        ))
    });
    assert_eq!(got.unwrap(), (1, None, Some(2)));
    // `object` takes no null.
    let node = read(br#"{"facing": null}"#).unwrap();
    let refused =
        RequestFields::read(&node, |fields| fields.object("facing", |_| Some(()))).unwrap_err();
    assert_eq!(
        refusals(&refused),
        [(
            "facing".to_owned(),
            "facing must be an object, got null".to_owned()
        )]
    );
}

/// The design's JSON code blocks between the headings `from` and `to`, in order.
fn design_examples(from: &str, to: &str) -> Vec<String> {
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

#[test]
fn the_envelopes_examples_are_the_designs_own_text() {
    // The contract's examples and the design's are one text: section 2's three envelopes,
    // in order, each file the block and a newline.
    let design = design_examples("### The response", "### Layout and key order");
    let files = ["ran", "read-a-model", "refused"].map(|name| {
        std::fs::read_to_string(contract(&format!("examples/envelope/{name}.json"))).unwrap()
    });
    assert_eq!(design.len(), files.len());
    for (block, file) in design.iter().zip(&files) {
        assert_eq!(format!("{block}\n"), *file);
    }
}

/// A wire type holding a type of its own that isn't one, as a result holds a document's
/// feature.
#[derive(schemars::JsonSchema)]
#[schemars(transform = super::wire_shape)]
struct Wire {
    optional: Option<u32>,
    held: Held,
}

#[derive(schemars::JsonSchema)]
struct Held {
    note: Option<String>,
    id: String,
}

#[test]
fn a_wire_type_requires_its_own_fields_and_a_type_it_holds_keeps_its_own() {
    // (A value of them, read, as a schema mirror's never is.)
    let value = Wire {
        optional: None,
        held: Held {
            note: None,
            id: "k7f2qx3m".to_owned(),
        },
    };
    assert_eq!(
        (value.optional, value.held.note, value.held.id.len()),
        (None, None, 8)
    );
    let schema = crate::document::schema_value_of::<Wire>();
    assert_eq!(schema["required"], serde_json::json!(["held", "optional"]));
    // Held's `note` may still be left out; its `id` may not.
    assert_eq!(
        schema["$defs"]["Held"]["required"],
        serde_json::json!(["id"])
    );
}

/// Every object schema in `schema` with `properties` that isn't closed to them
/// (`"additionalProperties": false`) or doesn't require each of them: where it is.
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

/// A mirror whose author left out `wire_shape`.
#[derive(schemars::JsonSchema)]
struct Unshaped {
    note: Option<String>,
}

#[test]
fn every_object_of_the_envelopes_snapshot_is_closed_and_requires_its_fields() {
    let schema = snapshot_value("envelope").unwrap();
    let mut found = Vec::new();
    open_shapes(&schema, "", &mut found);
    assert!(found.is_empty(), "{found:?}");
    // Its parts by the names an agent reads, the root `Response`.
    let defs: Vec<&String> = schema["$defs"].as_object().unwrap().keys().collect();
    assert_eq!(
        defs,
        ["Error", "Model", "ModelOf", "Ran", "Refused", "Session"]
    );
    assert_eq!(schema["title"], "Response");
    // Five of them are objects, each found and checked.
    for name in ["Error", "Model", "Ran", "Refused", "Session"] {
        assert_eq!(
            schema["$defs"][name]["additionalProperties"],
            serde_json::json!(false),
            "{name}"
        );
    }
    // The walk can fail: a mirror without `wire_shape`, and a type a shaped one holds.
    let mut found = Vec::new();
    open_shapes(
        &crate::document::schema_value_of::<Unshaped>(),
        "",
        &mut found,
    );
    assert_eq!(found, [""]);
    let mut found = Vec::new();
    open_shapes(&crate::document::schema_value_of::<Wire>(), "", &mut found);
    assert_eq!(found, ["/$defs/Held"]);
    // (A value of it, read, as a schema mirror's never is.)
    assert_eq!(Unshaped { note: None }.note, None);
}

/// A mirror enum of struct variants, tagged internally: each variant an inline object.
#[derive(schemars::JsonSchema)]
#[schemars(tag = "kind", transform = super::wire_shape)]
enum Tagged {
    Point { x: u32, note: Option<String> },
    Line { from: u32 },
}

/// One untagged.
#[derive(schemars::JsonSchema)]
#[schemars(untagged, transform = super::wire_shape)]
enum Untagged {
    A { a: Option<u32> },
    B { b: String },
}

/// One whose author left out `wire_shape`.
#[derive(schemars::JsonSchema)]
#[schemars(tag = "kind")]
enum TaggedUnshaped {
    Point { note: Option<String> },
}

#[test]
fn an_enums_variants_written_inline_are_closed_and_require_their_fields() {
    // (Values of them, read, as a schema mirror's never are.)
    let values = [Tagged::Point { x: 1, note: None }, Tagged::Line { from: 2 }];
    assert!(matches!(values[1], Tagged::Line { from: 2 }));
    assert!(matches!(&values[0], Tagged::Point { x: 1, note: None }));
    let values = [Untagged::A { a: None }, Untagged::B { b: "b".to_owned() }];
    assert!(matches!(&values[1], Untagged::B { b } if b == "b"));
    assert!(matches!(values[0], Untagged::A { a: None }));
    let TaggedUnshaped::Point { note } = TaggedUnshaped::Point { note: None };
    assert_eq!(note, None);

    let tagged = crate::document::schema_value_of::<Tagged>();
    let variants = tagged["oneOf"].as_array().unwrap();
    assert_eq!(variants.len(), 2);
    assert_eq!(
        variants[0]["required"],
        serde_json::json!(["kind", "note", "x"])
    );
    assert_eq!(variants[1]["required"], serde_json::json!(["from", "kind"]));
    let untagged = crate::document::schema_value_of::<Untagged>();
    let variants = untagged["anyOf"].as_array().unwrap();
    assert_eq!(variants[0]["required"], serde_json::json!(["a"]));
    assert_eq!(variants[1]["required"], serde_json::json!(["b"]));
    for schema in [&tagged, &untagged] {
        let mut found = Vec::new();
        open_shapes(schema, "", &mut found);
        assert!(found.is_empty(), "{found:?}: {schema}");
    }
    // Without it, each variant is open.
    let mut found = Vec::new();
    open_shapes(
        &crate::document::schema_value_of::<TaggedUnshaped>(),
        "",
        &mut found,
    );
    assert_eq!(found, ["/oneOf/0"]);
}

#[test]
fn a_snapshots_text_is_its_value_pretty_with_its_keys_sorted() {
    let value = snapshot_value("envelope").unwrap();
    let text = snapshot("envelope").unwrap().unwrap();
    assert_eq!(
        text,
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap())
    );
    assert!(snapshot_value("no_such_command").is_none());
}

#[test]
fn a_json_value_is_written_its_keys_sorted_and_escaped_as_every_string_is() {
    // By hand: keys sorted at every depth, numbers as serde_json writes them (`-0.0` for an
    // `f64` minus zero), the newline in a string escaped.
    let value = serde_json::json!({"b": [1, 2.5, -0.0], "a": {"z": null, "y": "x\ny"}, "c": true});
    assert_eq!(
        write(&Json::value(&value), Layout::OneLine).unwrap(),
        r#"{"a": {"y": "x\ny", "z": null}, "b": [1, 2.5, -0.0], "c": true}"#
    );
    assert_eq!(
        write(&Json::value(&value), Layout::Canonical).unwrap(),
        "{\n  \"a\": {\"y\": \"x\\ny\", \"z\": null},\n  \"b\": [1, 2.5, -0.0],\n  \"c\": true\n}"
    );
    // Inside a result, as `schema`'s `{"schema": S}` holds the envelope's.
    let schema = snapshot_value("envelope").unwrap();
    let result = Json::object([("schema", Json::value(&schema))]);
    let one = write(&result, Layout::OneLine).unwrap();
    assert!(!one.contains('\n'));
    let back: serde_json::Value = serde_json::from_str(&one).unwrap();
    assert_eq!(back["schema"], schema);
    let canonical = write(&result, Layout::Canonical).unwrap();
    let back: serde_json::Value = serde_json::from_str(&canonical).unwrap();
    assert_eq!(back["schema"], schema);
}

#[test]
fn numbers_are_written_as_the_document_writes_them() {
    use crate::document::{Angle, Length, Measure};
    let cases = [
        (Json::length(Length::from_nm(2_500_000)), "2.5"),
        (Json::length(Length::from_nm(1)), "0.000001"),
        (Json::length(Length::from_nm(-10_000_000)), "-10"),
        (Json::angle(Angle::from_ndeg(45_000_000_000)), "45"),
        (Json::angle(Angle::from_ndeg(1)), "0.000000001"),
        (
            Json::measure(Measure::from_millionths(780_365_046)),
            "780.365046",
        ),
        // A sketch's coordinate: the shortest decimal, never an exponent, `-0` as itself.
        (Json::coordinate(0.1), "0.1"),
        (Json::coordinate(1.0 / 3.0), "0.3333333333333333"),
        (Json::coordinate(1e-7), "0.0000001"),
        (Json::coordinate(-0.0), "-0"),
        (Json::coordinate(f64::NAN), "null"),
        (Json::coordinate(f64::INFINITY), "null"),
        // A direction's `-0` is `0`.
        (Json::direction([-0.0, 0.0, -1.0]), "[0, 0, -1]"),
        (
            Json::direction([0.6, -0.8, 1e-17]),
            "[0.6, -0.8, 0.00000000000000001]",
        ),
        (
            Json::point([
                Length::from_nm(1_000_000),
                Length::from_nm(-2_500_000),
                Length::from_nm(0),
            ]),
            "[1, -2.5, 0]",
        ),
    ];
    for (json, want) in cases {
        assert_eq!(write(&json, Layout::OneLine).unwrap(), want);
    }
}

/// `field`'s value read from `{"<field>": <text>}` by `read`, or every refusal's parameter,
/// message, and suggestion.
fn one<T>(
    field: &str,
    text: &str,
    read: impl FnOnce(&mut RequestFields<'_>) -> Option<T>,
) -> Result<T, Vec<(String, String, Option<String>)>> {
    let node = crate::document::json::read(format!(r#"{{"{field}": {text}}}"#).as_bytes()).unwrap();
    RequestFields::read(&node, read).map_err(|refused| {
        refused
            .errors()
            .iter()
            .map(|e| {
                (
                    e.parameter().unwrap_or_default().to_owned(),
                    e.message().to_owned(),
                    e.suggestion().map(str::to_owned),
                )
            })
            .collect()
    })
}

#[test]
fn a_length_is_read_exactly_and_refused_as_the_document_refuses_one() {
    use crate::document::Length;
    let length = |text: &str| one("radius", text, |f| f.length("radius"));
    // From its digits: 0.1 is 100,000 nm exactly, and zeros past the sixth place add none.
    assert_eq!(length("10.5"), Ok(Length::from_nm(10_500_000)));
    assert_eq!(length("0.1"), Ok(Length::from_nm(100_000)));
    assert_eq!(length("0.1000000000"), Ok(Length::from_nm(100_000)));
    assert_eq!(length("1e3"), Ok(Length::from_nm(1_000_000_000)));
    assert_eq!(length("0"), Ok(Length::from_nm(0)));
    let refused = |message: &str, suggestion: &str| {
        Err(vec![(
            "radius".to_owned(),
            message.to_owned(),
            Some(suggestion.to_owned()),
        )])
    };
    assert_eq!(
        length("0.0000001"),
        refused(
            "radius must be a length in millimetres, got 0.0000001, which has more than 6 decimals",
            "round it to 6 decimals: lengths are whole nanometres"
        )
    );
    // The design's own case: more digits than an `f64` holds are refused, not rounded.
    assert_eq!(
        length("0.10000000000000000001"),
        refused(
            "radius must be a length in millimetres, got 0.10000000000000000001, which has more than 6 decimals",
            "round it to 6 decimals: lengths are whole nanometres"
        )
    );
    assert_eq!(
        length("-0"),
        refused(
            "radius must be a length in millimetres, got -0, which is zero written with a minus sign",
            "write 0"
        )
    );
    assert_eq!(
        length("1e13"),
        refused(
            "radius must be a length in millimetres, got 1e13, which is too large",
            "a length is at most 9223372036854.775807 mm either way"
        )
    );
    assert_eq!(
        length(r#""10""#),
        Err(vec![(
            "radius".to_owned(),
            r#"radius must be a length in millimetres, got "10""#.to_owned(),
            None
        )])
    );
    assert_eq!(
        length("null"),
        Err(vec![(
            "radius".to_owned(),
            "radius must be a length in millimetres, got null".to_owned(),
            None
        )])
    );
    // Null only where the shape allows it.
    let nullable = |text: &str| one("radius", text, |f| f.nullable_length("radius"));
    assert_eq!(nullable("null"), Ok(None));
    assert_eq!(nullable("2"), Ok(Some(Length::from_nm(2_000_000))));
    assert_eq!(
        nullable("true"),
        Err(vec![(
            "radius".to_owned(),
            "radius must be a length in millimetres or null, got true or false".to_owned(),
            None
        )])
    );
}

#[test]
fn an_angle_is_read_exactly_to_nine_decimals() {
    use crate::document::Angle;
    let angle = |text: &str| one("value", text, |f| f.angle("value"));
    assert_eq!(angle("45"), Ok(Angle::from_ndeg(45_000_000_000)));
    assert_eq!(angle("45.123456789"), Ok(Angle::from_ndeg(45_123_456_789)));
    assert_eq!(
        angle("45.1234567891"),
        Err(vec![(
            "value".to_owned(),
            "value must be an angle in degrees, got 45.1234567891, which has more than 9 decimals"
                .to_owned(),
            Some("round it to 9 decimals: angles are whole nanodegrees".to_owned())
        )])
    );
    assert_eq!(
        angle("-0.0"),
        Err(vec![(
            "value".to_owned(),
            "value must be an angle in degrees, got -0.0, which is zero written with a minus sign"
                .to_owned(),
            Some("write 0".to_owned())
        )])
    );
}

#[test]
fn a_revision_is_a_whole_number_from_1() {
    let revision = |text: &str| one("base", text, |f| f.revision("base").map(Revision::number));
    assert_eq!(revision("1"), Ok(1));
    assert_eq!(revision("18446744073709551615"), Ok(u64::MAX));
    for bad in ["0", "18446744073709551616", "-1", "1.0", "1e0", r#""4""#] {
        assert_eq!(
            revision(bad),
            Err(vec![(
                "base".to_owned(),
                format!("base must be a whole number from 1 to 18446744073709551615, got {bad}"),
                None
            )]),
            "{bad}"
        );
    }
}

#[test]
fn a_direction_is_three_finite_numbers_each_at_fault_named_by_its_place() {
    let direction = |text: &str| one("normal", text, |f| f.direction("normal"));
    assert_eq!(direction("[0, 0, 1]"), Ok([0.0, 0.0, 1.0]));
    // Not all zero: a zero vector points nowhere. A tiny or huge one still has a direction.
    assert!(direction("[0, 0, 0]").is_err());
    assert!(direction("[0, -0, 0]").is_err());
    assert_eq!(direction("[1e-300, 0, 0]"), Ok([1e-300, 0.0, 0.0]));
    assert_eq!(direction("[0, 1e300, 0]"), Ok([0.0, 1e300, 0.0]));
    // Correctly rounded, as a sketch's coordinate is read, and `-0` kept.
    let [x, y, z] = direction("[0.1, -0, 1e-3]").unwrap();
    assert_eq!((x, y.to_bits(), z), (0.1, (-0.0f64).to_bits(), 0.001));
    let refused = |faults: &[(&str, &str)]| {
        Err(faults
            .iter()
            .map(|(at, message)| ((*at).to_owned(), (*message).to_owned(), None))
            .collect::<Vec<_>>())
    };
    assert_eq!(
        direction(r#"[1, "a", 1e400]"#),
        refused(&[
            ("normal.1", r#"normal.1 must be a finite number, got "a""#),
            ("normal.2", "normal.2 must be a finite number, got 1e400"),
        ])
    );
    assert_eq!(
        direction("[1, 2]"),
        refused(&[(
            "normal",
            "normal must be a direction, three numbers [x, y, z], got 2 of them"
        )])
    );
    assert_eq!(
        direction(r#""up""#),
        refused(&[(
            "normal",
            r#"normal must be a direction, three numbers [x, y, z], got "up""#
        )])
    );
}

/// A result holding a long text.
struct Long(String);

impl ToWire for Long {
    fn to_wire(&self) -> Json<'_> {
        Json::object([("text", Json::str(&self.0))])
    }
}

#[test]
fn a_response_past_its_limit_is_answered_with_a_refusal_saying_so() {
    use super::response_text_within;
    let answer = Response {
        session: Some(session(4, false)),
        model: Some(ModelRead {
            revision: revision(3),
            of: ModelChoice::Current,
            fault: None,
        }),
        result: Ok(Long("x".repeat(2000))),
    };
    let whole = write(&response(&answer), Layout::OneLine).unwrap();
    // At its limit, written as it is; one byte past, refused.
    assert_eq!(
        response_text_within(&answer, Layout::OneLine, whole.len()),
        whole
    );
    let limit = whole.len() - 1;
    assert_eq!(
        response_text_within(&answer, Layout::OneLine, limit),
        format!(
            r#"{{"ok": false, "session": {{"id": "s1", "path": "bracket.silico", "revision": 4, "transaction": false, "reloaded": false}}, "model": {{"revision": 3, "of": "current", "fault": null}}, "errors": [{{"code": "invalid_input", "message": "the response would be more than the {limit} bytes a response may be", "feature": null, "elements": [], "elements_total": 0, "parameter": null, "cause": "a command that changes the document may have done its work before its answer was written: read the session's revision before asking again", "suggestion": "ask for less at once", "candidates": []}}], "total": 1}}"#
        )
    );
    // A refusal too long itself is written without its session and model.
    let mut long_path = answer.session.clone().unwrap();
    long_path.path = "p".repeat(2000);
    let answer = Response {
        session: Some(long_path),
        ..answer
    };
    let text = response_text_within(&answer, Layout::Canonical, 1000);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["session"], serde_json::Value::Null);
    assert_eq!(value["model"], serde_json::Value::Null);
    assert_eq!(value["errors"][0]["code"], "invalid_input");
    assert_eq!(value["total"], 1);
    // A response within the real limit is its envelope.
    assert_eq!(
        super::response_text(&answer, Layout::OneLine),
        write(&response(&answer), Layout::OneLine).unwrap()
    );
}
