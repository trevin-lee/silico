//! M2.1: a `.silico` document read exactly from strict JSON and written back in canonical
//! form (docs/design/m2.1-document-format.md). Expected values are written out by hand:
//! nanometres as integers, canonical text as text.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::document::{
    Document, Feature, FeatureId, FormatVersion, ID_ALPHABET, Length, Operation, Units, read,
    schema, write,
};

/// The design's example, in canonical form.
const EXAMPLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
    {"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12},
    {"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de", "note": "Clearance for an M5 bolt"}
  ]
}
"#;

fn mm(nm: i64) -> Length {
    Length::from_nm(nm)
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn feature(id_text: &str, name: &str, operation: Operation) -> Feature {
    Feature {
        id: id(id_text),
        operation,
        version: 1,
        name: name.to_owned(),
        note: None,
    }
}

#[test]
fn the_example_reads_as_its_values_and_writes_back_byte_for_byte() {
    let document = read(EXAMPLE.as_bytes()).unwrap();
    let mut hole = feature(
        "m3tq8v0z",
        "Hole",
        Operation::Cut {
            base: id("k7f2qx3m"),
            tool: id("a91cw0de"),
        },
    );
    hole.note = Some("Clearance for an M5 bolt".to_owned());
    let want = Document {
        silico: FormatVersion,
        units: Units,
        features: vec![
            feature(
                "k7f2qx3m",
                "Block",
                Operation::Box {
                    position: [mm(0), mm(0), mm(0)],
                    length: mm(40_000_000),
                    width: mm(20_000_000),
                    height: mm(10_000_000),
                },
            ),
            feature(
                "a91cw0de",
                "Pin",
                Operation::Cylinder {
                    position: [mm(20_000_000), mm(10_000_000), mm(-1_000_000)],
                    radius: mm(2_500_000),
                    height: mm(12_000_000),
                },
            ),
            hole,
        ],
    };
    assert_eq!(document, want);
    assert_eq!(write(&document), EXAMPLE);
}

#[test]
fn every_feature_type_reads_and_writes_on_one_line() {
    let text = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "00000000", "type": "sphere", "v": 1, "name": "S", "position": [1, 2, 3], "radius": 0.000001},
    {"id": "11111111", "type": "cone", "v": 1, "name": "C", "position": [0, 0, 0], "radius1": 5, "radius2": 0, "height": 7.25},
    {"id": "22222222", "type": "torus", "v": 1, "name": "T", "position": [-0.5, 0, 0], "radius1": 10, "radius2": 2},
    {"id": "33333333", "type": "fuse", "v": 1, "name": "F", "base": "00000000", "tool": "11111111"},
    {"id": "44444444", "type": "common", "v": 1, "name": "M", "base": "33333333", "tool": "22222222"}
  ]
}
"#;
    let document = read(text.as_bytes()).unwrap();
    let operations: Vec<&Operation> = document.features.iter().map(|f| &f.operation).collect();
    assert_eq!(
        operations,
        [
            &Operation::Sphere {
                position: [mm(1_000_000), mm(2_000_000), mm(3_000_000)],
                radius: mm(1),
            },
            &Operation::Cone {
                position: [mm(0), mm(0), mm(0)],
                radius1: mm(5_000_000),
                radius2: mm(0),
                height: mm(7_250_000),
            },
            &Operation::Torus {
                position: [mm(-500_000), mm(0), mm(0)],
                radius1: mm(10_000_000),
                radius2: mm(2_000_000),
            },
            &Operation::Fuse {
                base: id("00000000"),
                tool: id("11111111"),
            },
            &Operation::Common {
                base: id("33333333"),
                tool: id("22222222"),
            },
        ]
    );
    assert_eq!(write(&document), text);
}

#[test]
fn a_document_with_no_features_writes_an_empty_array() {
    let text = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"features\": []\n}\n";
    let document = read(text.as_bytes()).unwrap();
    assert!(document.features.is_empty());
    assert_eq!(write(&document), text);
}

#[test]
fn any_spelling_of_the_same_document_writes_the_same_canonical_bytes() {
    // Keys in another order, other whitespace, CRLF, trailing zeros, `\/`, and a note
    // escaped every way JSON allows: the same document.
    let messy = "{\"features\":[{\"note\":\"Clearance for an \\u004d5 bolt\",\"tool\":\"a91cw0de\",\
        \"base\":\"k7f2qx3m\",\"name\":\"Hole\",\"v\":1,\"type\":\"cut\",\"id\":\"m3tq8v0z\"}],\r\n\
        \"units\" : { \"resolution\":\"1nm\" , \"length\":\"mm\" },\t\"silico\":1 }";
    let tidy = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de", "note": "Clearance for an M5 bolt"}
  ]
}
"#;
    assert_eq!(write(&read(messy.as_bytes()).unwrap()), tidy);
    let numbers = EXAMPLE
        .replace("\"length\": 40", "\"length\": 40.000")
        .replace("\"radius\": 2.5", "\"radius\": 2.5000000000");
    assert_eq!(write(&read(numbers.as_bytes()).unwrap()), EXAMPLE);
}

#[test]
fn strings_keep_every_character_and_escape_only_what_json_requires() {
    let mut document = read(EXAMPLE.as_bytes()).unwrap();
    let name = "Quote \" back\\slash\ttab\nline \u{1} é 😀 \u{2028} </tag>";
    document.features[0].name = name.to_owned();
    document.features[0].note = Some(String::new());
    let text = write(&document);
    // Quote, backslash, and control characters escaped; the rest, U+2028 too, as written.
    let want = "\"name\": \"Quote \\\" back\\\\slash\\ttab\\nline \\u0001 é 😀 \u{2028} </tag>\"";
    assert!(text.contains(want), "{text}");
    // An empty note is a note, kept.
    assert!(text.contains(r#""note": """#), "{text}");
    let back = read(text.as_bytes()).unwrap();
    assert_eq!(back, document);
    assert_eq!(write(&back), text);
}

/// The error reading `text` gives, which must be `invalid_input`.
fn refused(text: &str) -> silico_core::Error {
    let err = read(text.as_bytes()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(err.suggestion().is_some(), "{err}");
    err
}

/// A refusal case: replace, with, the feature and parameter named, what the message says,
/// and the place it gives.
type Refusal = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
    &'static str,
);

/// The example with `from` replaced by `to` (which must be in it).
fn example_with(from: &str, to: &str) -> String {
    assert!(EXAMPLE.contains(from), "{from}");
    EXAMPLE.replacen(from, to, 1)
}

#[test]
fn a_value_that_doesnt_fit_is_refused_naming_its_feature_field_and_place() {
    // (replace, with, feature, parameter, says, line and column of the value: counted
    // with Python's str.index on the edited example, not by this reader)
    let cases: &[Refusal] = &[
        (
            "\"length\": 40",
            "\"length\": 1e-3",
            Some("k7f2qx3m"),
            Some("length"),
            "length 1e-3 is written with an exponent",
            "line 5, column 97",
        ),
        (
            "\"length\": 40",
            "\"length\": 0.0000001",
            Some("k7f2qx3m"),
            Some("length"),
            "length 0.0000001 has more than 6 decimals",
            "line 5, column 97",
        ),
        (
            "\"length\": 40",
            "\"length\": -0",
            Some("k7f2qx3m"),
            Some("length"),
            "is zero written with a minus sign",
            "line 5",
        ),
        (
            "\"length\": 40",
            "\"length\": 9223372036854.775808",
            Some("k7f2qx3m"),
            Some("length"),
            "is too large",
            "line 5",
        ),
        (
            "\"length\": 40",
            "\"length\": \"40\"",
            Some("k7f2qx3m"),
            Some("length"),
            "length is a string, not a length in millimetres",
            "line 5",
        ),
        (
            "[20, 10, -1]",
            "[20, 10, -1e0]",
            Some("a91cw0de"),
            Some("position.2"),
            "position.2 -1e0 is written with an exponent",
            "line 6, column 88",
        ),
        (
            "[20, 10, -1]",
            "[20, 10]",
            Some("a91cw0de"),
            Some("position"),
            "position has 2 values, not 3",
            "line 6",
        ),
        (
            "\"radius\": 2.5, ",
            "",
            Some("a91cw0de"),
            Some("radius"),
            "feature a91cw0de has no radius",
            "line 6, column 5",
        ),
        (
            "\"height\": 12}",
            "\"height\": 12, \"depth\": 3}",
            Some("a91cw0de"),
            Some("depth"),
            "feature a91cw0de has a key it can't have: depth",
            "line 6",
        ),
        (
            "\"type\": \"cut\"",
            "\"type\": \"pocket\"",
            Some("m3tq8v0z"),
            Some("type"),
            "\"pocket\" isn't a feature type",
            "line 7",
        ),
        (
            "\"type\": \"box\", \"v\": 1",
            "\"type\": \"box\", \"v\": 2",
            Some("k7f2qx3m"),
            Some("v"),
            "this Silico has no box of version 2",
            "line 5",
        ),
        (
            "\"v\": 1, \"name\": \"Block\"",
            "\"v\": 1.0, \"name\": \"Block\"",
            Some("k7f2qx3m"),
            Some("v"),
            "v is 1.0, not a whole number",
            "line 5",
        ),
        (
            "\"name\": \"Pin\"",
            "\"name\": \"\"",
            Some("a91cw0de"),
            Some("name"),
            "name is empty",
            "line 6",
        ),
        (
            "\"tool\": \"a91cw0de\"",
            "\"tool\": \"A91CW0DE\"",
            Some("m3tq8v0z"),
            Some("tool"),
            "tool \"A91CW0DE\" isn't a feature ID",
            "line 7",
        ),
        (
            "\"id\": \"k7f2qx3m\"",
            "\"id\": \"k7f2qx3l\"",
            Some("features[0]"),
            Some("id"),
            "id \"k7f2qx3l\" isn't a feature ID",
            "line 5, column 12",
        ),
        (
            "\"note\": \"Clearance for an M5 bolt\"",
            "\"note\": null",
            Some("m3tq8v0z"),
            Some("note"),
            "note is null, not a string",
            "line 7",
        ),
        (
            "\"silico\": 1",
            "\"silico\": 2",
            None,
            Some("silico"),
            "format version 2, which this Silico doesn't read",
            "line 2, column 13",
        ),
        (
            "\"length\": \"mm\"",
            "\"length\": \"in\"",
            None,
            Some("units.length"),
            "the document's length is \"in\", which format version 1 doesn't read",
            "line 3",
        ),
        (
            "\"resolution\": \"1nm\"",
            "\"resolution\": \"1nm\", \"angle\": \"rad\"",
            None,
            Some("units.angle"),
            "units has a key it can't have: angle",
            "line 3",
        ),
        (
            "\"silico\": 1,",
            "\"silico\": 1, \"comment\": \"x\",",
            None,
            Some("comment"),
            "the document has a key it can't have: comment",
            "line 2, column 16",
        ),
        (
            "\"silico\": 1,",
            "",
            None,
            Some("silico"),
            "the document has no silico",
            "line 1, column 1",
        ),
    ];
    for &(from, to, feature, parameter, says, place) in cases {
        let err = refused(&example_with(from, to));
        assert_eq!(err.feature(), feature, "{to}: {err}");
        assert_eq!(err.parameter(), parameter, "{to}: {err}");
        assert!(err.message().contains(says), "{to}: {err}");
        assert!(err.message().contains(place), "{to}: {err}");
    }
}

#[test]
fn text_that_isnt_a_documents_json_is_refused_saying_where() {
    let cases: &[(&[u8], &str)] = &[
        (b"", "line 1, column 1"),
        (b"\xef\xbb\xbf{}", "byte-order mark"),
        (b"{\"silico\": 1, \"silico\": 1}", "written twice"),
        (b"{\"silico\": NaN}", "found `N`"),
        (b"{\"silico\": Infinity}", "found `I`"),
        (b"{\"silico\": 1,}", "expected a key"),
        (
            b"{\n  \"silico\": \"\xc3\x28\"}",
            "isn't UTF-8, at line 2, column 14",
        ),
        (b"[1, 2]", "the document is an array, not an object"),
    ];
    for &(bytes, says) in cases {
        let err = read(bytes).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
        assert!(err.message().contains(says), "{bytes:?}: {err}");
        assert!(err.suggestion().is_some(), "{err}");
    }
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let err = read(deep.as_bytes()).unwrap_err();
    assert!(err.message().contains("nest more than 64"), "{err}");
}

#[test]
fn the_schema_is_the_committed_contract() {
    // Agents rely on this shape: changing it means changing the committed file, with the
    // maintainer (tests/contract/ is protected).
    let path = format!(
        "{}/tests/contract/document-schema.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(schema().unwrap(), committed);
}

/// An ID from eight picks of the alphabet.
fn any_id() -> impl Strategy<Value = FeatureId> {
    let alphabet: Vec<char> = ID_ALPHABET.chars().collect();
    prop::collection::vec(prop::sample::select(alphabet), 8)
        .prop_map(|chars| FeatureId::new(&chars.into_iter().collect::<String>()).unwrap())
}

fn any_length() -> impl Strategy<Value = Length> {
    prop_oneof![
        any::<i64>().prop_map(Length::from_nm),
        (-1_000_000_000_000i64..=1_000_000_000_000).prop_map(Length::from_nm),
        (-1000i64..=1000).prop_map(|mm| Length::from_nm(mm * 1_000_000)),
    ]
}

fn any_position() -> impl Strategy<Value = [Length; 3]> {
    [any_length(), any_length(), any_length()]
}

fn any_operation() -> impl Strategy<Value = Operation> {
    prop_oneof![
        (any_position(), any_length(), any_length(), any_length()).prop_map(
            |(position, length, width, height)| Operation::Box {
                position,
                length,
                width,
                height
            }
        ),
        (any_position(), any_length(), any_length()).prop_map(|(position, radius, height)| {
            Operation::Cylinder {
                position,
                radius,
                height,
            }
        }),
        (any_position(), any_length())
            .prop_map(|(position, radius)| Operation::Sphere { position, radius }),
        (any_position(), any_length(), any_length(), any_length()).prop_map(
            |(position, radius1, radius2, height)| Operation::Cone {
                position,
                radius1,
                radius2,
                height
            }
        ),
        (any_position(), any_length(), any_length()).prop_map(|(position, radius1, radius2)| {
            Operation::Torus {
                position,
                radius1,
                radius2,
            }
        }),
        (any_id(), any_id()).prop_map(|(base, tool)| Operation::Fuse { base, tool }),
        (any_id(), any_id()).prop_map(|(base, tool)| Operation::Cut { base, tool }),
        (any_id(), any_id()).prop_map(|(base, tool)| Operation::Common { base, tool }),
    ]
}

fn any_feature() -> impl Strategy<Value = Feature> {
    (any_id(), any_operation(), ".+", prop::option::of(".*")).prop_map(
        |(id, operation, name, note)| Feature {
            id,
            operation,
            version: 1,
            name,
            note,
        },
    )
}

fn any_document() -> impl Strategy<Value = Document> {
    prop::collection::vec(any_feature(), 0..6).prop_map(|features| Document {
        silico: FormatVersion,
        units: Units,
        features,
    })
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 500,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x51_11c0),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// Any document written reads back as itself, and writes the same bytes again.
    #[test]
    fn any_document_round_trips_byte_for_byte(document in any_document()) {
        let text = write(&document);
        let closed = text.ends_with("}\n");
        prop_assert!(closed);
        prop_assert!(!text.contains('\r'));
        let back = read(text.as_bytes());
        prop_assert!(back.is_ok(), "{:?}\n{}", back, text);
        let back = back.unwrap();
        prop_assert_eq!(&back, &document);
        prop_assert_eq!(write(&back), text);
    }

    /// Bytes of any kind are read or refused, never panicked on.
    #[test]
    fn any_bytes_are_read_or_refused(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        let _ = read(&bytes);
    }

    /// The example with a stretch of it replaced by other bytes: read or refused, and if
    /// read, written back as a document that reads as itself.
    #[test]
    fn a_damaged_document_is_read_or_refused(
        start in 0usize..EXAMPLE.len(),
        cut in 0usize..40,
        insert in prop::collection::vec(any::<u8>(), 0..12),
    ) {
        let mut bytes = EXAMPLE.as_bytes().to_vec();
        let end = (start + cut).min(bytes.len());
        bytes.splice(start..end, insert);
        if let Ok(document) = read(&bytes) {
            let text = write(&document);
            prop_assert_eq!(read(text.as_bytes()).unwrap(), document);
        }
    }
}
