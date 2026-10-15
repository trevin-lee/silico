//! M2.1: a `.silico` document read exactly from strict JSON and written back in canonical
//! form (docs/design/m2.1-document-format.md). Expected values are written out by hand:
//! nanometres as integers, canonical text as text.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;

use example::EXAMPLE;
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::document::{
    Document, Feature, FeatureId, FeatureVersion, FormatVersion, ID_ALPHABET, Length, Name,
    Operation, Units, read, schema, write,
};

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
        version: FeatureVersion,
        name: Name::new(name).unwrap(),
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
    assert_eq!(write(&document).unwrap(), EXAMPLE);
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
    assert_eq!(write(&document).unwrap(), text);
}

#[test]
fn a_document_with_no_features_writes_an_empty_array() {
    let text = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"features\": []\n}\n";
    let document = read(text.as_bytes()).unwrap();
    assert!(document.features.is_empty());
    assert_eq!(write(&document).unwrap(), text);
}

#[test]
fn a_length_written_with_an_exponent_is_written_back_plain() {
    // As a script's JSON library may write it (Python: `1e-05`; JavaScript: `1e-7`):
    // read exactly, written as the plain decimal.
    let text = EXAMPLE
        .replacen("\"length\": 40", "\"length\": 4e1", 1)
        .replacen("\"radius\": 2.5", "\"radius\": 25E-1", 1)
        .replacen("[20, 10, -1]", "[2e1, 1e-05, -1e0]", 1);
    let document = read(text.as_bytes()).unwrap();
    let Operation::Cylinder { position, .. } = &document.features[1].operation else {
        panic!()
    };
    assert_eq!(*position, [mm(20_000_000), mm(10), mm(-1_000_000)]);
    let written = write(&document).unwrap();
    assert_eq!(
        written,
        EXAMPLE.replacen("[20, 10, -1]", "[20, 0.00001, -1]", 1)
    );
    // A string is reserved for expressions, which aren't read yet: refused, saying so.
    let err = read(
        EXAMPLE
            .replacen("\"length\": 40", "\"length\": \"40\"", 1)
            .as_bytes(),
    )
    .unwrap_err();
    assert!(
        err.suggestion()
            .unwrap()
            .contains("reserved for expressions"),
        "{err}"
    );
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
    assert_eq!(write(&read(messy.as_bytes()).unwrap()).unwrap(), tidy);
    let numbers = EXAMPLE
        .replace("\"length\": 40", "\"length\": 40.000")
        .replace("\"radius\": 2.5", "\"radius\": 2.5000000000");
    assert_eq!(write(&read(numbers.as_bytes()).unwrap()).unwrap(), EXAMPLE);
}

#[test]
fn strings_keep_every_character_and_escape_only_what_json_requires() {
    let mut document = read(EXAMPLE.as_bytes()).unwrap();
    let name = "Quote \" back\\slash\ttab\nline \u{1} é 😀 \u{2028} </tag>";
    document.features[0].name = Name::new(name).unwrap();
    document.features[0].note = Some(String::new());
    let text = write(&document).unwrap();
    // Quote, backslash, control characters, and the line separator escaped; the rest as
    // written.
    let want = "\"name\": \"Quote \\\" back\\\\slash\\ttab\\nline \\u0001 é 😀 \\u2028 </tag>\"";
    assert!(text.contains(want), "{text}");
    // An empty note is a note, kept.
    assert!(text.contains(r#""note": """#), "{text}");
    let back = read(text.as_bytes()).unwrap();
    assert_eq!(back, document);
    assert_eq!(write(&back).unwrap(), text);
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
            "[20, 10, -1.0000001]",
            Some("a91cw0de"),
            Some("position.2"),
            "position.2 -1.0000001 has more than 6 decimals",
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
            "feature a91cw0de has a key it can't have: \"depth\"",
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
            "units has a key it can't have: \"angle\"",
            "line 3",
        ),
        (
            "\"silico\": 1,",
            "\"silico\": 1, \"comment\": \"x\",",
            None,
            Some("comment"),
            "the document has a key it can't have: \"comment\"",
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
fn a_document_past_the_size_or_memory_limits_is_refused_before_it_costs_much() {
    use silico_core::document::json::{MAX_DOCUMENT_LEN, MAX_INPUT_LEN, MAX_TREE_BYTES};
    // One byte past what Silico reads: refused before anything is read.
    let big = vec![b' '; MAX_INPUT_LEN + 1];
    let err = read(&big).unwrap_err();
    assert!(
        err.message()
            .contains(&format!("more than the {MAX_INPUT_LEN}")),
        "{err}"
    );
    // Said as too large, not as bad JSON: there's nothing to fix in the text.
    assert!(
        err.message().contains("too large for Silico to read"),
        "{err}"
    );
    assert!(!err.message().contains("valid JSON"), "{err}");
    assert!(
        err.suggestion().unwrap().contains("make it smaller"),
        "{err}"
    );
    // At it, read (here, refused as JSON for being only spaces).
    let err = read(&big[1..]).unwrap_err();
    assert!(err.message().contains("expected a value"), "{err}");
    // The text that costs most for its size: a one-member object per 8 bytes. Within the
    // size, but its tree would pass the memory limit: refused when it would.
    let objects = format!(
        "{{\"x\": [{}{{\"a\":0}}]}}",
        "{\"a\":0},".repeat((MAX_DOCUMENT_LEN - 64) / 8)
    );
    assert!(objects.len() <= MAX_INPUT_LEN);
    let err = read(objects.as_bytes()).unwrap_err();
    assert!(
        err.message()
            .contains(&format!("more than the {MAX_TREE_BYTES} bytes of memory")),
        "{err}"
    );
    assert!(
        err.message().contains("too large for Silico to read"),
        "{err}"
    );
    assert!(!err.message().contains("valid JSON"), "{err}");
    // A real document near the size reads: about seven times its size in tree.
    let feature = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;
    let count = (MAX_DOCUMENT_LEN - 200) / (feature.len() + 6);
    let many = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}]}}",
        vec![feature; count].join(",\n    ")
    );
    assert!(many.len() <= MAX_DOCUMENT_LEN, "{}", many.len());
    assert_eq!(read(many.as_bytes()).unwrap().features.len(), count);
}

/// The published schema, as an agent's validator compiles it.
fn validator() -> jsonschema::Validator {
    let schema: serde_json::Value = serde_json::from_str(&schema().unwrap()).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}

/// Whether the validator accepts `text`, as JSON.
fn valid(validator: &jsonschema::Validator, text: &str) -> bool {
    validator.is_valid(&serde_json::from_str(text).unwrap())
}

#[test]
fn the_schema_accepts_what_the_reader_reads() {
    // Once, the schema said lengths were a `multipleOf` 1e-6, and validators, dividing in
    // floating point, refused 0.1 mm (0.1 / 1e-6 isn't whole to them) and 27 more of
    // 0.1 to 9.9: a published schema that refuses documents Silico reads.
    // This validator computes `multipleOf` exactly, so it wouldn't see that bug; python's
    // jsonschema and ajv, like most, divide in floating point. So: no number in the schema
    // says a multiple.
    let schema: serde_json::Value = serde_json::from_str(&schema().unwrap()).unwrap();
    assert!(schema["$defs"]["Length"].is_object(), "{schema}");
    assert!(!schema.to_string().contains("multipleOf"), "{schema}");
    let validator = &*VALIDATOR;
    assert!(valid(validator, EXAMPLE));
    for tenths in 1..100 {
        let radius = format!("{}.{}", tenths / 10, tenths % 10);
        let text = EXAMPLE.replacen("\"radius\": 2.5", &format!("\"radius\": {radius}"), 1);
        read(text.as_bytes()).unwrap();
        assert!(valid(validator, &text), "{radius}");
    }
    for value in ["100.123456", "0.000001", "-9223372036854.775808", "1.1"] {
        let text = EXAMPLE.replacen("\"length\": 40", &format!("\"length\": {value}"), 1);
        read(text.as_bytes()).unwrap();
        assert!(valid(validator, &text), "{value}");
    }
}

#[test]
fn the_schema_refuses_what_the_reader_refuses_by_shape() {
    // So the validator above is checking something: each of these the reader refuses too.
    let validator = &*VALIDATOR;
    for (from, to) in [
        ("\"height\": 12}", "\"height\": 12, \"depth\": 3}"),
        ("\"radius\": 2.5, ", ""),
        ("\"type\": \"cut\"", "\"type\": \"pocket\""),
        ("\"type\": \"box\", \"v\": 1", "\"type\": \"box\", \"v\": 2"),
        ("\"name\": \"Pin\"", "\"name\": \"\""),
        ("\"id\": \"k7f2qx3m\"", "\"id\": \"k7f2qx3l\""),
        ("[20, 10, -1]", "[20, 10]"),
        ("\"length\": 40", "\"length\": \"40\""),
        ("\"silico\": 1", "\"silico\": 2"),
        ("\"length\": \"mm\"", "\"length\": \"in\""),
    ] {
        let text = EXAMPLE.replacen(from, to, 1);
        assert!(read(text.as_bytes()).is_err(), "{to}");
        assert!(!valid(validator, &text), "{to}");
    }
}

#[test]
fn every_feature_type_is_read_written_and_in_the_schema() {
    // FEATURE_TYPES, the writer's `type`, the reader's arms, and the schema all agree: a
    // type in one but not another fails here.
    use silico_core::document::FEATURE_TYPES;
    let zero = [mm(0), mm(0), mm(0)];
    let one = mm(1_000_000);
    let samples = [
        Operation::Box {
            position: zero,
            length: one,
            width: one,
            height: one,
        },
        Operation::Cylinder {
            position: zero,
            radius: one,
            height: one,
        },
        Operation::Sphere {
            position: zero,
            radius: one,
        },
        Operation::Cone {
            position: zero,
            radius1: one,
            radius2: one,
            height: one,
        },
        Operation::Torus {
            position: zero,
            radius1: one,
            radius2: one,
        },
        Operation::Fuse {
            base: id("00000000"),
            tool: id("11111111"),
        },
        Operation::Cut {
            base: id("00000000"),
            tool: id("11111111"),
        },
        Operation::Common {
            base: id("00000000"),
            tool: id("11111111"),
        },
    ];
    let names: Vec<&str> = samples.iter().map(Operation::type_name).collect();
    assert_eq!(names, FEATURE_TYPES);
    let validator = &*VALIDATOR;
    for operation in samples {
        let document = Document {
            silico: FormatVersion,
            units: Units,
            features: vec![feature("22222222", "F", operation)],
        };
        let text = write(&document).unwrap();
        assert_eq!(read(text.as_bytes()).unwrap(), document, "{text}");
        assert!(valid(validator, &text), "{text}");
    }
}

#[test]
fn bidirectional_controls_are_written_as_escapes() {
    // A name holding U+202E would reorder the rest of its feature's line in a diff (Trojan
    // Source): escaped, it can't, and reads back as the same name.
    let mut document = read(EXAMPLE.as_bytes()).unwrap();
    let name = "A\u{202e}\u{2066}\u{2069}\u{200e}\u{200f}\u{61c}\u{202a}";
    document.features[0].name = Name::new(name).unwrap();
    let text = write(&document).unwrap();
    for c in name.chars().skip(1) {
        assert!(!text.contains(c), "U+{:04X} written raw: {text}", c as u32);
    }
    assert!(
        text.contains(r#""name": "A\u202e\u2066\u2069\u200e\u200f\u061c\u202a""#),
        "{text}"
    );
    assert_eq!(read(text.as_bytes()).unwrap(), document);
}

#[test]
fn strings_escape_exactly_what_canonical_form_names() {
    // D7: `"`, `\`, C0 (short escapes where JSON has them), DEL and C1, the line and
    // paragraph separators, and the bidirectional controls are escaped; the characters
    // either side of each range are written as they are. The text here is written out by
    // hand.
    let mut document = read(EXAMPLE.as_bytes()).unwrap();
    let name = "\u{1f} \u{8}\u{c}\u{7e}\u{7f}\u{80}\u{85}\u{9f}\u{a0}\u{2027}\u{2028}\u{2029}\u{202a}\
                \u{61b}\u{61c}\u{200d}\u{200e}\u{200f}\u{2010}\u{202e}\u{202f}\
                \u{2065}\u{2066}\u{2069}\u{206a}";
    document.features[0].name = Name::new(name).unwrap();
    let text = write(&document).unwrap();
    // Escaped where `\\` is written; the characters either side of each range raw.
    let want = [
        "\"name\": \"",
        "\\u001f \\b\\f~\\u007f\\u0080\\u0085\\u009f",
        "\u{a0}\u{2027}",
        "\\u2028\\u2029\\u202a",
        "\u{61b}",
        "\\u061c",
        "\u{200d}",
        "\\u200e\\u200f",
        "\u{2010}",
        "\\u202e",
        "\u{202f}\u{2065}",
        "\\u2066\\u2069",
        "\u{206a}",
        "\"",
    ]
    .concat();
    assert!(text.contains(&want), "{text}");
    assert_eq!(read(text.as_bytes()).unwrap(), document);
}

#[test]
fn ids_with_the_letters_crockford_leaves_out_are_refused() {
    // Crockford's base32, lower case, written out here, not taken from the code.
    assert_eq!(ID_ALPHABET, "0123456789abcdefghjkmnpqrstvwxyz");
    for bad in [
        "iiiiiiii", "oooooooo", "uuuuuuuu", "llllllll", "k7f2qx3I", "k7f2qx3",
    ] {
        assert_eq!(FeatureId::new(bad), None, "{bad}");
        let text = EXAMPLE.replace("k7f2qx3m", bad);
        let err = read(text.as_bytes()).unwrap_err();
        assert!(err.message().contains("isn't a feature ID"), "{bad}: {err}");
    }
    for good in ["00000000", "zzzzzzzz", "abcdefgh", "jkmnpqrs"] {
        assert!(FeatureId::new(good).is_some(), "{good}");
    }
}

#[test]
fn a_number_is_shown_whole_up_to_forty_characters_then_cut() {
    // 40 digits: shown whole. 41: cut, with `…`. (Version numbers are compared as text.)
    let forty = "9".repeat(40);
    let err = read(
        EXAMPLE
            .replacen("\"silico\": 1", &format!("\"silico\": {forty}"), 1)
            .as_bytes(),
    )
    .unwrap_err();
    assert!(
        err.message().contains(&format!("version {forty},")),
        "{err}"
    );
    let more = "9".repeat(41);
    let err = read(
        EXAMPLE
            .replacen("\"silico\": 1", &format!("\"silico\": {more}"), 1)
            .as_bytes(),
    )
    .unwrap_err();
    assert!(
        err.message().contains(&format!("version {forty}…,")),
        "{err}"
    );
}

#[test]
fn what_the_schema_cant_say_the_reader_refuses() {
    // JSON Schema has no way to refuse a whole number written with a fraction, or a
    // length past the resolution: the schema accepts these, and the reader, the authority,
    // refuses them.
    let validator = &*VALIDATOR;
    for (from, to) in [
        ("\"length\": 40", "\"length\": 40.0000001"),
        ("\"silico\": 1", "\"silico\": 1.0"),
        (
            "\"type\": \"box\", \"v\": 1",
            "\"type\": \"box\", \"v\": 1.0",
        ),
    ] {
        let text = EXAMPLE.replacen(from, to, 1);
        assert!(valid(validator, &text), "{to}");
        assert!(read(text.as_bytes()).is_err(), "{to}");
    }
}

#[test]
fn every_bidirectional_control_is_escaped() {
    // Every code point of each range, not only its ends: a writer missing any one passes
    // a test of the ends (RLE, PDF, LRO, RLI, and FSI are inside them).
    let mut document = read(EXAMPLE.as_bytes()).unwrap();
    for cp in [
        0x61c, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066, 0x2067, 0x2068,
        0x2069,
    ] {
        let c = char::from_u32(cp).unwrap();
        document.features[0].name = Name::new(format!("A{c}")).unwrap();
        let text = write(&document).unwrap();
        assert!(!text.contains(c), "U+{cp:04X} written raw");
        assert!(text.contains(&format!("\"A\\u{cp:04x}\"")), "{text}");
    }
}

#[test]
fn the_first_duplicate_in_written_order_is_the_one_reported() {
    // "b" is written again (column 18) before "a" is (column 26): "b" is reported, though
    // "a" sorts first.
    let err =
        silico_core::document::json::read(br#"{"b": 0, "a": 0, "b": 0, "a": 0}"#).unwrap_err();
    assert!(
        err.message.contains("\"b\" is written twice"),
        "{}",
        err.message
    );
    assert_eq!((err.at.line, err.at.column), (1, 18), "{}", err.message);
}

#[test]
fn a_document_opens_exactly_when_it_can_be_saved() {
    // The size is measured on canonical text, by `read` and `write` alike: compact text
    // within the size whose canonical text (with its spacing) would pass it is refused by
    // both, so no document opens that can't be saved, and none is saved that won't open.
    use silico_core::document::json::MAX_DOCUMENT_LEN;
    let f =
        r#"{"id":"00000000","type":"fuse","v":1,"name":"a","base":"00000001","tool":"00000002"}"#;
    let count = (MAX_DOCUMENT_LEN - 100) / (f.len() + 1);
    let compact = format!(
        "{{\"silico\":1,\"units\":{{\"length\":\"mm\",\"resolution\":\"1nm\"}},\"features\":[{}]}}",
        vec![f; count].join(",")
    );
    assert!(compact.len() <= MAX_DOCUMENT_LEN);
    let err = read(compact.as_bytes()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(
        err.message().contains("too large for Silico to read"),
        "{err}"
    );
    assert!(err.message().contains("more than the 16777216"), "{err}");
    assert!(
        err.suggestion().unwrap().contains("make it smaller"),
        "{err}"
    );
    // Many small features: none takes most of the text, so none is named.
    assert_eq!(err.feature(), None, "{err}");
    // Fewer features, whose canonical text fits: opened, and saved.
    let fits = (MAX_DOCUMENT_LEN - 200) / (f.len() + 30);
    let compact = format!(
        "{{\"silico\":1,\"units\":{{\"length\":\"mm\",\"resolution\":\"1nm\"}},\"features\":[{}]}}",
        vec![f; fits].join(",")
    );
    let document = read(compact.as_bytes()).unwrap();
    let text = write(&document).unwrap();
    assert!(text.len() <= MAX_DOCUMENT_LEN);
    assert_eq!(read(text.as_bytes()).unwrap(), document);
    // A note escaped to six times its size, on a feature not the last: refused, naming the
    // feature that holds it, by `write` and by `read` alike.
    let mut document = read(EXAMPLE.as_bytes()).unwrap();
    document.features[1].note = Some("\u{1}".repeat(3 << 20));
    let err = write(&document).unwrap_err();
    assert!(err.message().contains("too large to write"), "{err}");
    assert_eq!(err.feature(), Some("a91cw0de"), "{err}");
    let raw = EXAMPLE.replacen(
        "\"height\": 12}",
        &format!(
            "\"height\": 12, \"note\": \"{}\"}}",
            "\\u0001".repeat(3 << 20)
        ),
        1,
    );
    let err = read(raw.as_bytes()).unwrap_err();
    assert!(
        err.message().contains("too large for Silico to read"),
        "{err}"
    );
    assert_eq!(err.feature(), Some("a91cw0de"), "{err}");
    assert!(
        err.suggestion()
            .unwrap()
            .contains("this feature's name or note"),
        "{err}"
    );
    // Under the size, written and read back.
    document.features[1].note = Some("\u{1}".repeat(1 << 20));
    let text = write(&document).unwrap();
    assert_eq!(read(text.as_bytes()).unwrap(), document);
}

/// `id`'s feature as a refusal for size measures it: its line at indent 0, without the
/// comma. (Counted from the written text, not by the code under test.)
fn feature_len(text: &str, id: &str) -> usize {
    let line = text
        .lines()
        .find(|l| l.contains(&format!("\"id\": \"{id}\"")))
        .unwrap();
    line.trim_start().trim_end_matches(',').len()
}

#[test]
fn a_feature_is_named_exactly_when_it_takes_more_than_half_the_size() {
    use silico_core::document::json::MAX_DOCUMENT_LEN;
    let half = MAX_DOCUMENT_LEN / 2;
    let mut document = read(EXAMPLE.as_bytes()).unwrap();
    document.features[0].note = Some(String::new());
    document.features[1].note = Some(String::new());
    let text = write(&document).unwrap();
    let base0 = feature_len(&text, "k7f2qx3m");
    let base1 = feature_len(&text, "a91cw0de");
    // Two features of exactly half each: too large, but neither takes more than half.
    document.features[0].note = Some("a".repeat(half - base0));
    document.features[1].note = Some("a".repeat(half - base1));
    assert_eq!(write(&document).unwrap_err().feature(), None);
    // One byte more on the second: it takes more than half, and is named, by `write` and
    // by `read` alike.
    document.features[1].note = Some("a".repeat(half - base1 + 1));
    assert_eq!(write(&document).unwrap_err().feature(), Some("a91cw0de"));
    let mut fits = document.clone();
    fits.features[0].note = Some(String::new());
    let mut text = write(&fits).unwrap();
    let at = text.find("\"note\": \"\"").unwrap() + 9;
    text.insert_str(at, &"a".repeat(half - base0));
    let err = read(text.as_bytes()).unwrap_err();
    assert!(
        err.message().contains("too large for Silico to read"),
        "{err}"
    );
    assert_eq!(err.feature(), Some("a91cw0de"), "{err}");
}

#[test]
fn the_first_and_last_features_are_named_exactly_when_they_take_more_than_half_too() {
    use silico_core::document::json::MAX_DOCUMENT_LEN;
    let half = MAX_DOCUMENT_LEN / 2;
    let ids = ["k7f2qx3m", "a91cw0de", "m3tq8v0z"];
    let mut empty = read(EXAMPLE.as_bytes()).unwrap();
    for feature in &mut empty.features {
        feature.note = Some(String::new());
    }
    let text = write(&empty).unwrap();
    let base: Vec<usize> = ids.iter().map(|id| feature_len(&text, id)).collect();
    // Each end in turn takes one byte more than half, the middle exactly half.
    for (large, other) in [(0, 1), (2, 1)] {
        let mut document = empty.clone();
        document.features[large].note = Some("a".repeat(half - base[large] + 1));
        document.features[other].note = Some("a".repeat(half - base[other]));
        let err = write(&document).unwrap_err();
        assert_eq!(err.feature(), Some(ids[large]), "{}", ids[large]);
    }
}

#[test]
fn a_document_whose_text_is_exactly_the_size_is_written_and_read() {
    use silico_core::document::json::MAX_DOCUMENT_LEN;
    let mut document = read(EXAMPLE.as_bytes()).unwrap();
    document.features[0].note = Some(String::new());
    let base = write(&document).unwrap().len();
    document.features[0].note = Some("a".repeat(MAX_DOCUMENT_LEN - base));
    let text = write(&document).unwrap();
    assert_eq!(text.len(), MAX_DOCUMENT_LEN);
    assert_eq!(read(text.as_bytes()).unwrap(), document);
    // One byte more: refused by both, for its size.
    document.features[0].note = Some("a".repeat(MAX_DOCUMENT_LEN - base + 1));
    let err = write(&document).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(err.message().contains("too large to write"), "{err}");
    let mut text = text;
    text.insert(text.find("\"note\": \"").unwrap() + 9, 'a');
    let err = read(text.as_bytes()).unwrap_err();
    assert!(
        err.message().contains("too large for Silico to read"),
        "{err}"
    );
}

#[test]
fn a_loosely_spelled_document_past_the_canonical_size_opens() {
    // Silico reads up to four times the size, so a document spelled more loosely than
    // canonical form (here, 20 MiB of spacing) opens if its canonical text fits.
    use silico_core::document::json::{MAX_DOCUMENT_LEN, MAX_INPUT_LEN};
    assert_eq!(MAX_DOCUMENT_LEN, 16 << 20);
    assert_eq!(MAX_INPUT_LEN, 64 << 20);
    let at = EXAMPLE.rfind('}').unwrap();
    let loose = format!(
        "{}{}{}",
        &EXAMPLE[..at],
        " ".repeat(20 << 20),
        &EXAMPLE[at..]
    );
    assert!(loose.len() > MAX_DOCUMENT_LEN);
    let document = read(loose.as_bytes()).unwrap();
    assert_eq!(write(&document).unwrap(), EXAMPLE);
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
            version: FeatureVersion,
            name: Name::new(name).unwrap(),
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

static VALIDATOR: std::sync::LazyLock<jsonschema::Validator> = std::sync::LazyLock::new(validator);

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
        let text = write(&document).unwrap();
        let closed = text.ends_with("}\n");
        prop_assert!(closed);
        prop_assert!(!text.contains('\r'));
        let back = read(text.as_bytes());
        prop_assert!(back.is_ok(), "{:?}\n{}", back, text);
        let back = back.unwrap();
        prop_assert_eq!(&back, &document);
        prop_assert_eq!(write(&back).unwrap(), text.clone());
        // And the published schema accepts it, as an agent's validator would.
        prop_assert!(valid(&VALIDATOR, &text), "{}", text);
    }

    /// Any length written with its point moved and an exponent to match (`1.5e-5`,
    /// `150e-7`, with zeros after or not) reads as the same length.
    #[test]
    fn any_exponent_spelling_reads_as_the_length_it_writes(
        nm in prop_oneof![
            any::<i64>(),
            -10_000_000i64..10_000_000,
            prop::sample::select(vec![i64::MIN, i64::MAX, 0, 1, -1, 10, 999_999, 1_000_000]),
        ],
        moved in -30i32..30,
        zeros in 0usize..4,
    ) {
        let plain = Length::from_nm(nm).to_string();
        let negative = plain.starts_with('-');
        let digits_and_point = plain.trim_start_matches('-');
        let (whole, fraction) = digits_and_point.split_once('.').unwrap_or((digits_and_point, ""));
        // The digits with the point after `whole.len() + moved` of them, and e{-moved}.
        let digits = format!("{whole}{fraction}{}", "0".repeat(zeros));
        let point = whole.len() as i32 + moved;
        let (mantissa, exponent) = if point <= 0 {
            (format!("0.{}{digits}", "0".repeat((-point) as usize)), -moved)
        } else if point as usize >= digits.len() {
            (format!("{digits}{}", "0".repeat(point as usize - digits.len())), -moved)
        } else {
            let (a, b) = digits.split_at(point as usize);
            (format!("{a}.{b}"), -moved)
        };
        let sign = if negative { "-" } else { "" };
        let text = format!("{sign}{mantissa}e{exponent}");
        prop_assert_eq!(Length::parse(&text), Ok(Length::from_nm(nm)), "{}", text);
        // A tenth of a nanometre more, spelled with an exponent: the digits of nm and a 1,
        // times 10^-(7 + zeros), refused as too precise however the zeros fall.
        let tenths = format!("{}1{}", nm.unsigned_abs(), "0".repeat(zeros));
        let finer = format!("{sign}{tenths}e-{}", 7 + zeros);
        prop_assert_eq!(
            Length::parse(&finer),
            Err(silico_core::document::NumberError::TooPrecise { decimals: 6 }),
            "{}",
            finer
        );
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
            let text = write(&document).unwrap();
            prop_assert_eq!(read(text.as_bytes()).unwrap(), document);
        }
    }
}
