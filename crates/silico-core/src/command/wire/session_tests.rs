//! The session commands' results on the wire (docs/design/m4.1-command-api.md, section 7):
//! each of the design's results written from a value built by hand, byte for byte as its
//! example file (the design's own text, `tests/contract/commands/examples/`, protected), and
//! held to its command's snapshot; and the builders they're written with, a document's value
//! ([`Json::plain`]), a request's ([`Json::node`]), and a feature ([`Json::feature`]), and
//! the reader that takes a request's value whole ([`RequestFields::node`]). The results a
//! session on the fake kernel makes as the design's (`begin`, `rollback`, a feature added and
//! one removed) are made so in tests/wire_session_commands.rs, which also holds the
//! requests and the refusals.

use std::path::PathBuf;

use super::{Json, Layout, RequestFields, ToWire, write};
use crate::command::{
    Added, Begun, CacheCleared, CheckAnswer, Committed, FieldSet, Opened, Problems, Reformatted,
    Removed, Replaced, Revision, RolledBack, SessionId, Warnings,
};
use crate::document::json::{Plain, read};
use crate::document::{Feature, FeatureId};
use crate::{Error, ErrorCode, Warning, WarningCode};

fn example(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract/commands/examples")
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn revision(n: u64) -> Revision {
    let mut r = Revision::OPENED;
    for _ in 1..n {
        r = r.next().unwrap();
    }
    r
}

fn ids(ids: &[&str]) -> Vec<FeatureId> {
    ids.iter().map(|id| FeatureId::new(id).unwrap()).collect()
}

/// `value` written, and a newline, as an example file holds it.
fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

/// Panics unless `text` holds to the result schema of the command `of`'s committed
/// snapshot.
fn holds(of: &str, text: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/contract/commands/{of}.json"));
    let snapshot: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let validator = jsonschema::validator_for(&snapshot["result"]).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{of}: {errors:?}");
}

/// `value` written as the example `name`, a result of `of`, and held to `of`'s snapshot;
/// on one line, the same value.
fn as_designed<T: ToWire>(of: &str, name: &str, value: &T) {
    let text = written(value);
    assert_eq!(text, example(&format!("{of}/{name}")), "{of}/{name}");
    holds(of, &text);
    let one = write(&value.to_wire(), Layout::OneLine).unwrap();
    assert!(!one.contains('\n'), "{of}/{name}");
    let back: serde_json::Value = serde_json::from_str(&one).unwrap();
    assert_eq!(
        back,
        serde_json::from_str::<serde_json::Value>(&text).unwrap()
    );
}

/// The feature `text` holds, read as the document's reader reads it.
fn feature(text: &str) -> Feature {
    let document = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{text}]}}"#
    );
    crate::document::read(document.as_bytes())
        .unwrap()
        .features
        .remove(0)
}

/// The commit the design's examples hold: written, refingerprinting `refingerprinted` and
/// re-solving `resolved`, with no problems.
fn committed(refingerprinted: &[&str], resolved: &[&str]) -> Committed {
    Committed {
        written: true,
        path: "bracket.silico".to_owned(),
        refingerprinted: ids(refingerprinted),
        resolved: ids(resolved),
        problems: Problems::of(Vec::new()),
    }
}

#[test]
fn cache_clears_result_is_written_as_the_design_gives_it() {
    // The cache beside `bracket.silico`, as the session's path names it, and 214 entries
    // removed: the design's numbers, by hand.
    as_designed(
        "cache_clear",
        "result",
        &CacheCleared {
            directory: ".silico-cache".to_owned(),
            removed: 214,
        },
    );
}

#[test]
fn begin_rollback_and_commit_results_are_written_as_the_design_gives_them() {
    as_designed("begin", "result", &Begun { base: revision(4) });
    as_designed(
        "rollback",
        "result",
        &RolledBack {
            restored: revision(4),
        },
    );
    as_designed(
        "commit",
        "result",
        &committed(&["p0000001", "q7m2x9kd"], &["s0000002"]),
    );
    // A drift kept: one member a line, since it holds the error's object.
    let drift = Error::new(
        ErrorCode::Drift,
        "feature k7f2qx3m rebuilt to other measures than its fingerprint: volume 8000 → 8000.000002",
    )
    .with_feature("k7f2qx3m")
    .with_parameter("fingerprint.volume")
    .with_suggestion(
        "if this feature or one it uses was edited by hand or in a merge since it was saved, \
         run format (or silico fmt) to accept its new fingerprint; if not, this is a \
         determinism bug or a change in the kernel: find which before accepting it",
    );
    as_designed(
        "commit",
        "drift-kept",
        &Committed {
            problems: Problems::of(vec![drift]),
            ..committed(&["p0000001"], &[])
        },
    );
    as_designed(
        "commit",
        "unchanged",
        &Committed {
            written: false,
            ..committed(&[], &[])
        },
    );
}

#[test]
fn a_commits_problems_are_its_first_hundred_and_all_counted() {
    let many: Vec<Error> = (0..150)
        .map(|i| Error::new(ErrorCode::Drift, format!("drift {i}")))
        .collect();
    let commit = Committed {
        problems: Problems::of(many),
        ..committed(&[], &[])
    };
    let text = written(&commit);
    holds("commit", &text);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["problems"].as_array().unwrap().len(), 100);
    assert_eq!(value["problems"][99]["message"], "drift 99");
    assert_eq!(value["problems_total"], 150);
}

#[test]
fn opens_and_checks_problems_are_their_first_hundred_and_all_counted() {
    let many = || {
        Problems::of(
            (0..150)
                .map(|i| Error::new(ErrorCode::Drift, format!("drift {i}")))
                .collect(),
        )
    };
    let opened = Opened {
        session: SessionId::parse("s1").unwrap(),
        path: "p.silico".to_owned(),
        created: false,
        features: Some(1),
        bodies: Vec::new(),
        problems: many(),
    };
    let check = CheckAnswer {
        problems: many(),
        warnings: Warnings::none(),
    };
    for (of, text) in [("open", written(&opened)), ("check", written(&check))] {
        holds(of, &text);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["problems"].as_array().unwrap().len(), 100, "{of}");
        assert_eq!(value["problems"][0]["message"], "drift 0", "{of}");
        assert_eq!(value["problems"][99]["message"], "drift 99", "{of}");
        assert_eq!(value["problems_total"], 150, "{of}");
    }
    // A check's warnings: none, whatever its problems; and, given some, its own, listed and
    // counted as problems are, its problems none.
    let value: serde_json::Value = serde_json::from_str(&written(&check)).unwrap();
    assert_eq!(
        (&value["warnings"], &value["warnings_total"]),
        (&serde_json::json!([]), &serde_json::json!(0))
    );
    let warned = CheckAnswer {
        problems: Problems::none(),
        warnings: Warnings::of(
            (0..150).map(|i| Warning::new(WarningCode::FingerprintMissing, format!("missing {i}"))),
        ),
    };
    let text = written(&warned);
    holds("check", &text);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["warnings"].as_array().unwrap().len(), 100);
    assert_eq!(value["warnings"][0]["message"], "missing 0");
    assert_eq!(value["warnings"][0]["code"], "fingerprint_missing");
    assert_eq!(value["warnings"][99]["message"], "missing 99");
    assert_eq!(value["warnings_total"], 150);
    assert_eq!(
        (&value["problems"], &value["problems_total"]),
        (&serde_json::json!([]), &serde_json::json!(0))
    );
}

#[test]
fn formats_result_is_written_as_the_design_gives_it_each_list_its_own() {
    as_designed(
        "format",
        "result",
        &Reformatted {
            changed: true,
            written: true,
            path: "bracket.silico".to_owned(),
            refingerprinted: ids(&["k7f2qx3m", "p0000001"]),
            resolved: Vec::new(),
            accepted: ids(&["k7f2qx3m"]),
            problems: Problems::of(Vec::new()),
        },
    );
    // Each field from its own value: none a constant or another's.
    let other = Reformatted {
        changed: false,
        written: false,
        path: "p.silico".to_owned(),
        refingerprinted: ids(&["a0000001"]),
        resolved: ids(&["s0000001"]),
        accepted: ids(&["b0000001"]),
        problems: Problems::from(Error::new(ErrorCode::Drift, "d")),
    };
    let value: serde_json::Value = serde_json::from_str(&written(&other)).unwrap();
    assert_eq!(
        (
            &value["changed"],
            &value["written"],
            &value["path"],
            &value["refingerprinted"],
            &value["resolved"],
            &value["accepted"],
            &value["problems_total"]
        ),
        (
            &serde_json::json!(false),
            &serde_json::json!(false),
            &serde_json::json!("p.silico"),
            &serde_json::json!(["a0000001"]),
            &serde_json::json!(["s0000001"]),
            &serde_json::json!(["b0000001"]),
            &serde_json::json!(1)
        )
    );
    assert_eq!(value["problems"][0]["message"], "d");
    holds("format", &written(&other));
}

#[test]
fn the_edits_results_are_written_as_the_design_gives_them() {
    as_designed(
        "add_feature",
        "result",
        &Added {
            feature: feature(
                r#"{"id": "q7m2x9kd", "type": "pocket", "v": 1, "name": "Leg holes", "base": "p0000001", "sketch": "s0000002", "extent": {"type": "through_all", "reversed": false}}"#,
            ),
            changed: true,
            commit: None,
        },
    );
    as_designed(
        "set_field",
        "result",
        &FieldSet {
            feature: FeatureId::new("p0000001").unwrap(),
            field: "extent.length".to_owned(),
            before: Plain::Number("6".to_owned()),
            after: Plain::Number("8".to_owned()),
            changed: true,
            commit: Some(committed(&["p0000001", "q7m2x9kd", "f0000003"], &[])),
        },
    );
    as_designed(
        "replace_text",
        "result",
        &Replaced {
            changed: true,
            features: 0,
            before: Some(
                "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\"id\": \"k7f2qx3m\", \"type\": \"box\", \"v\": 1, \"name\": \"Block\", \"position\": [0, 0, 0], \"length\": 40, \"width\": 20, \"height\": 10}\n  ]\n}\n"
                    .to_owned(),
            ),
            commit: Some(committed(&[], &[])),
        },
    );
}

#[test]
fn inside_a_transaction_an_edits_commit_and_replaced_text_are_null() {
    let set = FieldSet {
        feature: FeatureId::new("p0000001").unwrap(),
        field: "note".to_owned(),
        before: Plain::String("old".to_owned()),
        after: Plain::Null,
        changed: false,
        commit: None,
    };
    assert_eq!(
        write(&set.to_wire(), Layout::OneLine).unwrap(),
        r#"{"feature": "p0000001", "field": "note", "before": "old", "after": null, "changed": false, "commit": null}"#
    );
    holds("set_field", &written(&set));
    let replaced = Replaced {
        changed: false,
        features: 3,
        before: None,
        commit: None,
    };
    assert_eq!(
        write(&replaced.to_wire(), Layout::OneLine).unwrap(),
        r#"{"changed": false, "features": 3, "before": null, "commit": null}"#
    );
    holds("replace_text", &written(&replaced));
    // A removed feature's place and a feature added's change, each its own.
    let removed = Removed {
        feature: feature(
            r#"{"id": "b0000001", "type": "sphere", "v": 1, "name": "S", "position": [0, 0, 0], "radius": 1}"#,
        ),
        index: 17,
        changed: false,
        commit: Some(committed(&[], &[])),
    };
    let value: serde_json::Value = serde_json::from_str(&written(&removed)).unwrap();
    assert_eq!(
        (&value["index"], &value["changed"]),
        (&serde_json::json!(17), &serde_json::json!(false))
    );
    assert_eq!(value["commit"]["written"], true);
    holds("remove_feature", &written(&removed));
}

#[test]
fn an_added_features_commit_is_written_outside_a_transaction() {
    let added = Added {
        feature: feature(
            r#"{"id": "b0000001", "type": "sphere", "v": 1, "name": "S", "position": [0, 0, 0], "radius": 1}"#,
        ),
        changed: true,
        commit: Some(committed(&["b0000001"], &[])),
    };
    let value: serde_json::Value = serde_json::from_str(&written(&added)).unwrap();
    assert_eq!(
        value["commit"]["refingerprinted"],
        serde_json::json!(["b0000001"])
    );
    assert_eq!(value["commit"]["written"], true);
    holds("add_feature", &written(&added));
}

#[test]
fn a_boolean_fields_before_and_after_are_each_its_own() {
    let set = FieldSet {
        feature: FeatureId::new("p0000001").unwrap(),
        field: "extent.reversed".to_owned(),
        before: Plain::Bool(false),
        after: Plain::Bool(true),
        changed: true,
        commit: None,
    };
    assert_eq!(
        write(&set.to_wire(), Layout::OneLine).unwrap(),
        r#"{"feature": "p0000001", "field": "extent.reversed", "before": false, "after": true, "changed": true, "commit": null}"#
    );
    holds("set_field", &written(&set));
}

#[test]
fn a_documents_value_is_written_in_its_order_with_its_numbers_as_written() {
    // By hand: members as given, never sorted; numbers' text as it is (`1e3`, `2.50`), and
    // strings escaped as every string is.
    let plain = Plain::Object(vec![
        ("z".to_owned(), Plain::Number("1e3".to_owned())),
        (
            "a".to_owned(),
            Plain::Array(vec![
                Plain::Number("2.50".to_owned()),
                Plain::Bool(true),
                Plain::Null,
            ]),
        ),
        ("m".to_owned(), Plain::String("x\ny".to_owned())),
    ]);
    let one = r#"{"z": 1e3, "a": [2.50, true, null], "m": "x\ny"}"#;
    assert_eq!(write(&Json::plain(&plain), Layout::OneLine).unwrap(), one);
    // An object of scalars is on one line in canonical form too; one holding an object is
    // written a member a line, its inner object on one.
    assert_eq!(write(&Json::plain(&plain), Layout::Canonical).unwrap(), one);
    let nested = Plain::Object(vec![(
        "extent".to_owned(),
        Plain::Object(vec![
            ("type".to_owned(), Plain::String("one_side".to_owned())),
            ("length".to_owned(), Plain::Number("6".to_owned())),
        ]),
    )]);
    assert_eq!(
        write(&Json::plain(&nested), Layout::Canonical).unwrap(),
        "{\n  \"extent\": {\"type\": \"one_side\", \"length\": 6}\n}"
    );
    assert_eq!(
        write(&Json::plain(&nested), Layout::OneLine).unwrap(),
        r#"{"extent": {"type": "one_side", "length": 6}}"#
    );
    // A request's value, as read, is written as its plain value is.
    let node = read(br#"{"z": 1e3, "a": [2.50, true, null], "m": "x\ny"}"#).unwrap();
    assert_eq!(Plain::of(&node), plain);
    assert_eq!(write(&Json::node(&node), Layout::OneLine).unwrap(), one);
    let node = read(br#"{"extent": {"type": "one_side", "length": 6}}"#).unwrap();
    assert_eq!(
        write(&Json::node(&node), Layout::Canonical).unwrap(),
        write(&Json::plain(&nested), Layout::Canonical).unwrap()
    );
}

#[test]
fn a_feature_is_written_as_the_documents_canonical_form_writes_it() {
    // Members in schema order, whatever order they were read in; a feature holding objects
    // a member a line.
    let read_in_another_order = feature(
        r#"{"extent": {"reversed": false, "type": "through_all"}, "sketch": "s0000002", "base": "p0000001", "name": "Leg holes", "v": 1, "type": "pocket", "id": "q7m2x9kd"}"#,
    );
    let text = write(&Json::feature(&read_in_another_order), Layout::Canonical).unwrap();
    assert_eq!(
        text,
        crate::document::write_feature(&read_in_another_order).unwrap()
    );
    assert_eq!(
        text,
        "{\n  \"id\": \"q7m2x9kd\",\n  \"type\": \"pocket\",\n  \"v\": 1,\n  \"name\": \"Leg holes\",\n  \"base\": \"p0000001\",\n  \"sketch\": \"s0000002\",\n  \"extent\": {\"type\": \"through_all\", \"reversed\": false}\n}"
    );
    assert_eq!(
        write(&Json::feature(&read_in_another_order), Layout::OneLine).unwrap(),
        r#"{"id": "q7m2x9kd", "type": "pocket", "v": 1, "name": "Leg holes", "base": "p0000001", "sketch": "s0000002", "extent": {"type": "through_all", "reversed": false}}"#
    );
}

#[test]
fn set_fields_edges_are_laid_out_as_the_feature_writer_lays_out_a_fillets() {
    let fillet = feature(
        r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge5"], "radius": 2}"#,
    );
    let names = |text: &str| Plain::of(&read(text.as_bytes()).unwrap());
    let set = FieldSet {
        feature: FeatureId::new("f0000001").unwrap(),
        field: "edges".to_owned(),
        before: names(r#"["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge5"]"#),
        after: names(r#"["k7f2qx3m.box.Edge1"]"#),
        changed: true,
        commit: None,
    };
    let result = written(&set);
    // By hand: a name a line, as the document holds a fillet's edges.
    assert_eq!(
        result,
        "{\n  \"feature\": \"f0000001\",\n  \"field\": \"edges\",\n  \"before\": [\n    \"k7f2qx3m.box.Edge1\",\n    \"k7f2qx3m.box.Edge5\"\n  ],\n  \"after\": [\n    \"k7f2qx3m.box.Edge1\"\n  ],\n  \"changed\": true,\n  \"commit\": null\n}\n"
    );
    holds("set_field", &result);
    // The fillet's edges as its feature writes them, a member of an object at the same depth
    // as before is: the same bytes.
    let feature = write(&Json::feature(&fillet), Layout::Canonical).unwrap();
    let member = |text: &str, key: &str| {
        let start = text.find(&format!("\n  \"{key}\": ")).unwrap() + key.len() + 7;
        let end = start + text[start..].find("\n  ]").unwrap() + "\n  ]".len();
        text[start..end].to_owned()
    };
    assert_eq!(member(&result, "before"), member(&feature, "edges"));
    // Any other list on one line, as the feature writer writes a box's position, and so the
    // whole result, holding no object; one edge, a string; and on one line, the same names.
    let position = FieldSet {
        field: "position".to_owned(),
        before: names("[0, 0, 0]"),
        after: names("[0, 0, 5]"),
        ..set.clone()
    };
    assert_eq!(
        written(&position),
        "{\"feature\": \"f0000001\", \"field\": \"position\", \"before\": [0, 0, 0], \"after\": [0, 0, 5], \"changed\": true, \"commit\": null}\n"
    );
    let one = FieldSet {
        field: "edges.0".to_owned(),
        before: names(r#""k7f2qx3m.box.Edge1""#),
        after: names(r#""k7f2qx3m.box.Edge2""#),
        ..set.clone()
    };
    assert_eq!(
        written(&one),
        "{\"feature\": \"f0000001\", \"field\": \"edges.0\", \"before\": \"k7f2qx3m.box.Edge1\", \"after\": \"k7f2qx3m.box.Edge2\", \"changed\": true, \"commit\": null}\n"
    );
    assert_eq!(
        write(&set.to_wire(), Layout::OneLine).unwrap(),
        r#"{"feature": "f0000001", "field": "edges", "before": ["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge5"], "after": ["k7f2qx3m.box.Edge1"], "changed": true, "commit": null}"#
    );
}

#[test]
fn a_requests_value_is_taken_whole_whatever_it_is_and_required() {
    let node = read(br#"{"value": null, "feature": {"type": "box", "extra": [1]}}"#).unwrap();
    let (value, feature) = RequestFields::read(&node, |f| {
        let value = f.node("value")?;
        let feature = f.node("feature")?;
        Some((Plain::of(value), Plain::of(feature)))
    })
    .unwrap();
    assert_eq!(value, Plain::Null);
    // Inside it, nothing is read, so nothing is refused: the document's reader reads it.
    assert_eq!(
        feature,
        Plain::Object(vec![
            ("type".to_owned(), Plain::String("box".to_owned())),
            (
                "extra".to_owned(),
                Plain::Array(vec![Plain::Number("1".to_owned())])
            ),
        ])
    );
    let node = read(br#"{"feature": 1}"#).unwrap();
    let refused = RequestFields::read(&node, |f| f.node("value")).unwrap_err();
    let said: Vec<(Option<&str>, &str)> = refused
        .errors()
        .iter()
        .map(|e| (e.parameter(), e.message()))
        .collect();
    assert_eq!(
        said,
        [
            (Some("value"), "value is missing"),
            (Some("feature"), "feature isn't a field of this request"),
        ]
    );
}

#[test]
fn the_commands_a_session_runs_are_the_wires_in_the_designs_order() {
    // Each command that runs in a session is a command of the wire's, named as the wire
    // names it, once, in the wire's order: every one but those answered at once and `open`.
    use super::session::{PUBLISHED_READS, SESSION_COMMANDS};
    let ordered: Vec<&str> = super::COMMANDS
        .map(|(name, _)| name)
        .into_iter()
        .filter(|name| SESSION_COMMANDS.contains(name))
        .collect();
    assert_eq!(ordered, SESSION_COMMANDS);
    let others: Vec<&str> = super::COMMANDS
        .map(|(name, _)| name)
        .into_iter()
        .filter(|name| !SESSION_COMMANDS.contains(name))
        .collect();
    assert_eq!(others, ["version", "open", "status", "cancel", "schema"]);
    // The design's reads of the published model (section 2: those that take `model`).
    assert_eq!(
        PUBLISHED_READS,
        [
            "check",
            "elements",
            "query",
            "mass_properties",
            "distance",
            "angle",
            "render"
        ]
    );
}
