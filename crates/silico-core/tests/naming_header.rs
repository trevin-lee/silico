//! M4.0: the document's header records the naming scheme its names were written under
//! (`naming`). The canonical writer always writes it, as the scheme Silico names by (6 since
//! M6.1); the reader takes 5 and 6 (6 a strict superset of 5, N10), reads a document without
//! it (saved before M4.0) as scheme 5, and refuses every other value, naming the field.
//! Inputs stay at 5 where they were; what Silico writes says 6. Expected texts, messages,
//! and places are written by hand (places counted with Python's `str.index` on the text, not
//! by this reader).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::ErrorCode;
use silico_core::document::{Checked, NamingScheme, read, schema, write};
use silico_core::naming::{
    NAMING_SCHEME, NAMING_SCHEME_NUMBER, NAMING_SCHEMES_READ, UNRECORDED_NAMING_SCHEME,
};

/// The document saved under scheme 5 (protected, with the maintainer), by `silico fmt` on
/// the real kernel, fingerprints and all: names of each form scheme 5 writes, a hole's rim
/// the cut generated from the block's top and the pin's side, a kept primitive's edge, and
/// an edge a pad generated from a sketch's line. That every name finds its edge, and every
/// feature rebuilds to its fingerprint, is silico-cli's tests/golden.rs, on the real kernel:
/// the fake makes no section edges, so it can't build the rim.
fn fixture() -> String {
    let path = format!(
        "{}/tests/contract/naming-scheme-5.silico",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The document saved under scheme 6 (M6.4, protected, with the maintainer), by `silico
/// fmt` on the real kernel, fingerprints and all: a linear pattern with a skip, a grid, a
/// polar pattern, a mirror, a whole-solid mirror of a solid holding a pattern, and fillets on
/// a `C2` edge, a `C2,1` edge, a mirror's `C1` edge, and a whole-solid mirror's `S1` edge.
/// That every name finds its edge, and every feature rebuilds to its fingerprint, is
/// silico-cli's tests/golden.rs, on the real kernel.
fn fixture_6() -> String {
    let path = format!(
        "{}/tests/contract/naming-scheme-6.silico",
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
    assert_eq!(NAMING_SCHEME, b"silico naming 6\n");
    assert_eq!(NAMING_SCHEME_NUMBER, 6);
    // 6 reads 5's names as they are (N10).
    assert_eq!(NAMING_SCHEMES_READ, [5, 6]);
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
fn the_fixture_reads_and_writes_back_differing_in_its_naming_line_alone() {
    // Saved under 5 and read under 6, which reads 5's names as they are: written back, the
    // save says 6, and no other byte moves.
    let text = fixture();
    assert_eq!(text.matches("\n  \"naming\": 5,\n").count(), 1);
    let document = read(text.as_bytes()).unwrap();
    assert_eq!(document.naming, NamingScheme);
    assert_eq!(document.features.len(), 8);
    let written = write(&document).unwrap();
    assert_eq!(
        written,
        text.replacen("\n  \"naming\": 5,\n", "\n  \"naming\": 6,\n", 1)
    );
    // And the save reads back as the same document, written alike.
    let again = read(written.as_bytes()).unwrap();
    assert_eq!(again, document);
    assert_eq!(write(&again).unwrap(), written);
}

#[test]
fn the_fixtures_names_pass_the_gate() {
    // The gate parses every name under the document's scheme (M2.8): each is one scheme 5
    // writes.
    let document = Checked::new(read(fixture().as_bytes()).unwrap()).unwrap();
    assert_eq!(document.document().features.len(), 8);
}

#[test]
fn a_document_without_naming_is_read_as_scheme_5_and_written_as_6() {
    // Saved before M4.0: keys in another order, spaced otherwise, no `naming`.
    let old = r#"{"units": {"resolution": "1nm", "length": "mm"}, "silico": 1, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
    let document = read(old.as_bytes()).unwrap();
    assert_eq!(document.naming, NamingScheme);
    let want = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 6,
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
    let want = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 6,\n  \"features\": []\n}\n";
    for scheme in ["5", "6"] {
        let first = format!(
            r#"{{"naming": {scheme}, "features": [], "silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}}}"#
        );
        assert_eq!(
            write(&read(first.as_bytes()).unwrap()).unwrap(),
            want,
            "{scheme}"
        );
    }
}

#[test]
fn a_scheme_this_silico_doesnt_read_is_refused_naming_it() {
    for value in ["4", "7", "0", "99999999999999999999"] {
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
            Some("this Silico reads naming schemes 5 and 6: open it in the Silico that wrote it"),
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
            Some("write naming as a whole number, such as 6"),
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
fn the_schemes_this_silico_reads_are_read() {
    for scheme in ["5", "6"] {
        let document = read(with_naming(scheme).as_bytes()).unwrap();
        assert_eq!(document.naming, NamingScheme, "{scheme}");
        assert!(document.features.is_empty(), "{scheme}");
    }
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
fn the_schema_requires_naming_and_takes_5_or_6() {
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
            "enum": [5, 6],
            "description": "The naming scheme the document's element names were written under: 5 or 6, and every save writes 6. A document without it was saved before Silico recorded it, and is read as scheme 5."
        })
    );
    let validator = jsonschema::validator_for(&schema).unwrap();
    let valid = |text: &str| validator.is_valid(&serde_json::from_str(text).unwrap());
    assert!(valid(&with_naming("5")));
    assert!(valid(&with_naming("6")));
    assert!(valid(&fixture()));
    for value in ["4", "7", "\"5\"", "\"6\"", "5.5", "null"] {
        assert!(!valid(&with_naming(value)), "{value}");
    }
    // The schema asks new documents to say their scheme; the reader still reads one saved
    // before the field (the migration), which the schema alone refuses.
    let old = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": []}"#;
    assert!(!valid(old));
    assert!(read(old.as_bytes()).is_ok());
}

#[test]
fn the_scheme_6_fixture_reads_and_writes_back_byte_for_byte_and_passes_the_gate_and_the_schema() {
    // Saved under 6 and read under 6: written back, no byte moves. Its four copy names, as
    // N10 writes them (written here by hand), each parse under the gate (a copy of a pad's
    // tool `C`, a grid's instance `C2,1`, a whole solid's `S` around a pattern's `C`), and
    // the schema takes it.
    let text = fixture_6();
    assert_eq!(text.matches("\n  \"naming\": 6,\n").count(), 1);
    let document = read(text.as_bytes()).unwrap();
    assert_eq!(document.features.len(), 30);
    assert_eq!(write(&document).unwrap(), text);
    for name in [
        "t0000001.linear_pattern.Edge(C2 p0000002.pad.Edge(G s0000002.sketch.Edge.na000002))",
        "t0000002.linear_pattern.Edge(C2,1 p0000004.pad.Edge(G s0000004.sketch.Edge.na000002))",
        "m0000001.mirror.Edge(C1 p0000008.pad.Edge(G s0000008.sketch.Edge.na000002))",
        "m0000002.mirror.Edge(S1 t0000004.linear_pattern.Edge(C1 p0000010.pad.Edge(G s0000010.sketch.Edge.na000002)))",
    ] {
        assert_eq!(
            text.matches(&format!("\n        \"{name}\"\n")).count(),
            1,
            "{name}"
        );
    }
    for (kind, count) in [
        ("linear_pattern", 3),
        ("polar_pattern", 1),
        ("mirror", 2),
        ("fillet", 4),
    ] {
        assert_eq!(
            document
                .features
                .iter()
                .filter(|f| f.operation.type_name() == kind)
                .count(),
            count,
            "{kind}"
        );
    }
    assert!(text.contains("\"skip\": [\n        [1]\n      ],"));
    assert!(text.contains("\"copies\": {\"type\": \"solid\"},"));
    Checked::new(document).unwrap();
    let schema: serde_json::Value = serde_json::from_str(&schema().unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&serde_json::from_str(&text).unwrap()));
}
