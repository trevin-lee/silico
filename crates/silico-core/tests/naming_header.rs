//! M4.0: the document's header records the naming scheme its names were written under
//! (`naming`). The canonical writer always writes it; the reader takes the scheme Silico
//! names by, reads a document without it (saved before M4.0) as scheme 5, and refuses every
//! other value, naming the field. Expected texts, messages, and places are written by hand
//! (places counted with Python's `str.index` on the text, not by this reader).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::ErrorCode;
use silico_core::document::{Checked, NamingScheme, read, schema, write};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::naming::{NAMING_SCHEME, NAMING_SCHEME_NUMBER, UNRECORDED_NAMING_SCHEME};
use silico_core::recompute::{Recompute, RunOutcome};

/// The document saved under scheme 5 (protected, with the maintainer): names of each form
/// scheme 5 writes, a piece (`I`), a kept primitive's edge, and an edge a pad generated from
/// a sketch's line.
fn fixture() -> String {
    let path = format!(
        "{}/tests/contract/naming-scheme-5.silico",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// One line: the header with `naming` written as `value`, and no features.
fn with_naming(value: &str) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": {value}, "features": []}}"#
    )
}

fn refused(text: &str) -> silico_core::Error {
    let err = read(text.as_bytes()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    err
}

#[test]
fn the_schemes_number_is_the_one_its_bytes_hold() {
    assert_eq!(NAMING_SCHEME, b"silico naming 5\n");
    assert_eq!(NAMING_SCHEME_NUMBER, 5);
    assert_eq!(
        NAMING_SCHEME,
        format!("silico naming {NAMING_SCHEME_NUMBER}\n").as_bytes()
    );
    assert_eq!(NamingScheme::NUMBER, NAMING_SCHEME_NUMBER);
}

#[test]
fn a_document_that_records_no_scheme_is_read_as_scheme_5_forever() {
    // Only documents saved before M4.0 lack the field, and their names are scheme 5's:
    // this stays 5 when the scheme moves on.
    assert_eq!(UNRECORDED_NAMING_SCHEME, 5);
}

#[test]
fn the_fixture_reads_and_writes_back_byte_for_byte() {
    let text = fixture();
    let document = read(text.as_bytes()).unwrap();
    assert_eq!(document.naming, NamingScheme);
    assert_eq!(document.features.len(), 8);
    assert_eq!(write(&document).unwrap(), text);
}

#[test]
fn the_fixtures_names_pass_the_gate_and_each_finds_its_edge() {
    let document = Checked::new(read(fixture().as_bytes()).unwrap()).unwrap();
    assert_eq!(document.document().naming, NamingScheme);
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let Ok(RunOutcome::Published(model)) = engine.start(document).wait() else {
        panic!("not published");
    };
    // A name that found no edge would be a `reference_broken` error of its feature.
    let errors: Vec<_> = model.errors().collect();
    assert!(errors.is_empty(), "{errors:?}");
    // Each feature built: the three that name edges among them.
    let built: Vec<String> = model
        .features()
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    assert_eq!(
        built,
        [
            "k7f2qx3m", "a91cw0de", "m3tq8v0z", "f0000001", "c0000001", "s0000001", "p0000001",
            "f0000002"
        ]
    );
}

#[test]
fn a_document_without_naming_is_read_as_scheme_5_and_written_with_it() {
    // Saved before M4.0: keys in another order, spaced otherwise, no `naming`.
    let old = r#"{"units": {"resolution": "1nm", "length": "mm"}, "silico": 1, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
    let document = read(old.as_bytes()).unwrap();
    assert_eq!(document.naming, NamingScheme);
    let want = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;
    assert_eq!(write(&document).unwrap(), want);
    // And what's written reads back as the same document, and writes the same bytes.
    let again = read(want.as_bytes()).unwrap();
    assert_eq!(again, document);
    assert_eq!(write(&again).unwrap(), want);
}

#[test]
fn naming_is_read_wherever_the_header_writes_it_and_written_after_units() {
    let first = r#"{"naming": 5, "features": [], "silico": 1, "units": {"length": "mm", "resolution": "1nm"}}"#;
    let want = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": []\n}\n";
    assert_eq!(write(&read(first.as_bytes()).unwrap()).unwrap(), want);
}

#[test]
fn a_scheme_this_silico_doesnt_read_is_refused_naming_it() {
    for value in ["4", "6", "0", "99999999999999999999"] {
        let err = refused(&with_naming(value));
        assert_eq!(err.parameter(), Some("naming"), "{value}: {err}");
        assert_eq!(err.feature(), None, "{value}: {err}");
        assert_eq!(
            err.message(),
            format!(
                "the document's names were written under naming scheme {value}, which this \
                 Silico doesn't read, at line 1, column 73"
            ),
            "{value}"
        );
        assert_eq!(
            err.suggestion(),
            Some("this Silico reads naming scheme 5: open it in the Silico that wrote it"),
            "{value}"
        );
    }
}

#[test]
fn a_scheme_of_enormous_digits_is_refused_its_digits_cut_short() {
    let digits = "9".repeat(100_000);
    let err = refused(&with_naming(&digits));
    assert_eq!(err.parameter(), Some("naming"), "{err}");
    // The first 40 digits, then an ellipsis.
    assert_eq!(
        err.message(),
        format!(
            "the document's names were written under naming scheme {}…, which this Silico \
             doesn't read, at line 1, column 73",
            "9".repeat(40)
        )
    );
}

#[test]
fn a_scheme_that_isnt_a_whole_number_is_refused_as_silico_would_be() {
    // Numbers that aren't plain non-negative digits, though some equal 5.
    for value in ["5.5", "-5", "5e0", "5.0", "-0", "5E0", "50e-1"] {
        let err = refused(&with_naming(value));
        assert_eq!(err.parameter(), Some("naming"), "{value}: {err}");
        assert_eq!(err.feature(), None, "{value}: {err}");
        assert_eq!(
            err.message(),
            format!("naming is {value}, not a whole number, at line 1, column 73"),
            "{value}"
        );
        assert_eq!(
            err.suggestion(),
            Some("write naming as a whole number, such as 5"),
            "{value}"
        );
    }
    // Values that aren't numbers.
    for (value, kind) in [
        ("\"5\"", "a string"),
        ("true", "true or false"),
        ("null", "null"),
        ("{}", "an object"),
        ("[]", "an array"),
        ("[5]", "an array"),
        ("{\"scheme\": 5}", "an object"),
    ] {
        let err = refused(&with_naming(value));
        assert_eq!(err.parameter(), Some("naming"), "{value}: {err}");
        assert_eq!(err.feature(), None, "{value}: {err}");
        assert_eq!(
            err.message(),
            format!("naming is {kind}, not a whole number, at line 1, column 73"),
            "{value}"
        );
        assert_eq!(
            err.suggestion(),
            Some("write naming as a whole number"),
            "{value}"
        );
    }
}

#[test]
fn the_scheme_this_silico_reads_is_read() {
    let document = read(with_naming("5").as_bytes()).unwrap();
    assert_eq!(document.naming, NamingScheme);
    assert!(document.features.is_empty());
}

#[test]
fn a_missing_format_version_or_units_is_still_refused_naming_it() {
    for (text, field) in [
        (
            r#"{"units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": []}"#,
            "silico",
        ),
        (r#"{"silico": 1, "naming": 5, "features": []}"#, "units"),
        (
            r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5}"#,
            "features",
        ),
    ] {
        let err = refused(text);
        assert_eq!(err.parameter(), Some(field), "{text}: {err}");
        assert_eq!(
            err.message(),
            format!("the document has no {field}, at line 1, column 1"),
            "{text}"
        );
        assert_eq!(
            err.suggestion(),
            Some(format!("add {field} to the document").as_str()),
            "{text}"
        );
    }
}

#[test]
fn a_bad_format_version_is_refused_before_the_scheme_is_read() {
    // The format's version says how the rest is read, the scheme among it.
    let text = r#"{"silico": 2, "units": {"length": "mm", "resolution": "1nm"}, "naming": 4, "features": []}"#;
    let err = refused(text);
    assert_eq!(err.parameter(), Some("silico"), "{err}");
}

#[test]
fn naming_written_twice_or_inside_units_is_refused() {
    let twice = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "naming": 5, "features": []}"#;
    let err = refused(twice);
    assert!(
        err.message()
            .contains("the key \"naming\" is written twice in one object"),
        "{err}"
    );
    let inside = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm", "naming": 5}, "features": []}"#;
    let err = refused(inside);
    assert_eq!(err.parameter(), Some("units.naming"), "{err}");
    assert!(
        err.message()
            .starts_with("units has a key it can't have: \"naming\""),
        "{err}"
    );
    // In a feature, it's a key a feature can't have.
    let in_feature = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "B", "naming": 5, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}]}"#;
    let err = refused(in_feature);
    assert_eq!(err.feature(), Some("k7f2qx3m"), "{err}");
    assert_eq!(err.parameter(), Some("naming"), "{err}");
}

#[test]
fn a_byte_order_mark_before_a_scheme_5_document_is_refused() {
    let text = format!("\u{feff}{}", with_naming("5"));
    let err = refused(&text);
    assert!(err.message().contains("byte-order mark"), "{err}");
}

#[test]
fn the_schema_requires_naming_and_takes_only_5() {
    let schema: serde_json::Value = serde_json::from_str(&schema().unwrap()).unwrap();
    assert_eq!(
        schema["required"],
        serde_json::json!(["silico", "units", "naming", "features"])
    );
    assert_eq!(
        schema["properties"]["naming"],
        serde_json::json!({
            "$ref": "#/$defs/NamingScheme",
            "description": "The naming scheme the document's element names were written under."
        })
    );
    assert_eq!(
        schema["$defs"]["NamingScheme"],
        serde_json::json!({
            "const": 5,
            "description": "The naming scheme the document's element names were written under: 5. A document without it was saved before Silico recorded it, and is read as scheme 5."
        })
    );
    let validator = jsonschema::validator_for(&schema).unwrap();
    let valid = |text: &str| validator.is_valid(&serde_json::from_str(text).unwrap());
    assert!(valid(&with_naming("5")));
    assert!(valid(&fixture()));
    for value in ["4", "6", "\"5\"", "5.5", "null"] {
        assert!(!valid(&with_naming(value)), "{value}");
    }
    // The schema asks new documents to say their scheme; the reader still reads one saved
    // before the field (the migration), which the schema alone refuses.
    let old = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": []}"#;
    assert!(!valid(old));
    assert!(read(old.as_bytes()).is_ok());
}
