//! M3.6, step 1: pad, pocket, revolve, and groove in the document
//! (docs/design/m3.6-sketch-features.md, S1): read exactly, written canonically, and in the
//! schema; outlined; refused by recompute until their steps are built, and their elements'
//! names refused until they're named. Expected text and messages are written out by hand
//! from the design. (The gate's rules: tests/sketch_features_gate.rs.)
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::model::WAIT;
use super::sweeps;

use silico_core::document::{
    Angle, Axis, Checked, Document, ElementId, Feature, FeatureVersion, FormatVersion, Length,
    Name, NamingScheme, Operation, PadExtent, PocketExtent, TurnExtent, Units, read, schema,
    validate, write,
};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode, command};
use sweeps::*;

/// What `engine` published for `document`, and the names of the kernel calls that made
/// shapes (every call but the summaries) since it started.
fn published(engine: &Recompute<FakeKernel>, document: Checked) -> std::sync::Arc<Model> {
    match engine.start(document).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

fn made(engine: &Recompute<FakeKernel>) -> Vec<&'static str> {
    engine
        .kernel()
        .calls()
        .iter()
        .map(|call| call.op.name())
        .filter(|name| !matches!(*name, "Summarize" | "Describe"))
        .collect()
}

fn mm(nm: i64) -> Length {
    Length::from_nm(nm)
}

/// S1's example in canonical form, with its three sketches (empty) before the features
/// that use them: written by hand from the design.
const EXAMPLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 6,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Outline", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []},
    {
      "id": "p0000001",
      "type": "pad",
      "v": 1,
      "name": "Block",
      "sketch": "s0000001",
      "extent": {"type": "one_side", "length": 10, "reversed": false}
    },
    {"id": "s0000002", "type": "sketch", "v": 1, "name": "Hole", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []},
    {
      "id": "q0000001",
      "type": "pocket",
      "v": 1,
      "name": "Bore",
      "base": "p0000001",
      "sketch": "s0000002",
      "extent": {"type": "through_all", "reversed": true}
    },
    {"id": "s0000003", "type": "sketch", "v": 1, "name": "Boss profile", "plane": "xz", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []},
    {
      "id": "r0000001",
      "type": "revolve",
      "v": 1,
      "name": "Boss",
      "base": "q0000001",
      "sketch": "s0000003",
      "axis": "y_axis",
      "extent": {"type": "one_side", "angle": 360, "reversed": false}
    }
  ]
}
"#;

fn feature(id_text: &str, name: &str, operation: Operation) -> Feature {
    Feature {
        id: id(id_text),
        operation,
        version: FeatureVersion,
        name: Name::new(name).unwrap(),
        suppressed: false,
        note: None,
        fingerprint: None,
    }
}

#[test]
fn the_designs_example_reads_as_its_values_and_writes_back_byte_for_byte() {
    let document = read(EXAMPLE.as_bytes()).unwrap();
    let operations: Vec<&Operation> = document
        .features
        .iter()
        .filter(|f| !matches!(f.operation, Operation::Sketch(_)))
        .map(|f| &f.operation)
        .collect();
    assert_eq!(
        operations,
        [
            &Operation::Pad {
                base: None,
                sketch: id("s0000001"),
                extent: PadExtent::OneSide {
                    length: mm(10_000_000),
                    reversed: false,
                },
            },
            &Operation::Pocket {
                base: id("p0000001"),
                sketch: id("s0000002"),
                extent: PocketExtent::ThroughAll { reversed: true },
            },
            &Operation::Revolve {
                base: Some(id("q0000001")),
                sketch: id("s0000003"),
                axis: Axis::YAxis,
                extent: TurnExtent::OneSide {
                    angle: Angle::from_ndeg(360_000_000_000),
                    reversed: false,
                },
            },
        ]
    );
    assert_eq!(write(&document).unwrap(), EXAMPLE);
    // And it passes the gate: each sketch is before its feature, each base consumed once.
    validate(&document).unwrap();
}

/// A document of one feature after the example's first sketch and pad, as canonical text
/// writes it (the feature over lines).
fn with_feature(lines: &str) -> String {
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 6,\n  \"features\": [\n    {}\n  ]\n}}\n",
        lines
    )
}

#[test]
fn every_form_reads_as_its_values_and_writes_back_byte_for_byte() {
    // (canonical text of the feature, inside the features array; its operation)
    let cases: Vec<(&str, Operation)> = vec![
        (
            "{\n      \"id\": \"p0000002\",\n      \"type\": \"pad\",\n      \"v\": 1,\n      \"name\": \"P\",\n      \"base\": \"p0000001\",\n      \"sketch\": \"s0000001\",\n      \"extent\": {\"type\": \"one_side\", \"length\": 0.000001, \"reversed\": true}\n    }",
            Operation::Pad {
                base: Some(id("p0000001")),
                sketch: id("s0000001"),
                extent: PadExtent::OneSide {
                    length: mm(1),
                    reversed: true,
                },
            },
        ),
        (
            "{\n      \"id\": \"p0000002\",\n      \"type\": \"pad\",\n      \"v\": 1,\n      \"name\": \"P\",\n      \"sketch\": \"s0000001\",\n      \"extent\": {\"type\": \"symmetric\", \"length\": 20.5}\n    }",
            Operation::Pad {
                base: None,
                sketch: id("s0000001"),
                extent: PadExtent::Symmetric {
                    length: mm(20_500_000),
                },
            },
        ),
        (
            "{\n      \"id\": \"p0000002\",\n      \"type\": \"pad\",\n      \"v\": 1,\n      \"name\": \"P\",\n      \"sketch\": \"s0000001\",\n      \"extent\": {\"type\": \"two_sides\", \"length\": 10, \"length2\": 5}\n    }",
            Operation::Pad {
                base: None,
                sketch: id("s0000001"),
                extent: PadExtent::TwoSides {
                    length: mm(10_000_000),
                    length2: mm(5_000_000),
                },
            },
        ),
        (
            "{\n      \"id\": \"q0000002\",\n      \"type\": \"pocket\",\n      \"v\": 1,\n      \"name\": \"Q\",\n      \"base\": \"p0000001\",\n      \"sketch\": \"s0000001\",\n      \"extent\": {\"type\": \"one_side\", \"length\": 1000000, \"reversed\": false}\n    }",
            Operation::Pocket {
                base: id("p0000001"),
                sketch: id("s0000001"),
                extent: PocketExtent::OneSide {
                    length: mm(1_000_000_000_000),
                    reversed: false,
                },
            },
        ),
        (
            "{\n      \"id\": \"q0000002\",\n      \"type\": \"pocket\",\n      \"v\": 1,\n      \"name\": \"Q\",\n      \"base\": \"p0000001\",\n      \"sketch\": \"s0000001\",\n      \"extent\": {\"type\": \"symmetric\", \"length\": 3}\n    }",
            Operation::Pocket {
                base: id("p0000001"),
                sketch: id("s0000001"),
                extent: PocketExtent::Symmetric {
                    length: mm(3_000_000),
                },
            },
        ),
        (
            "{\n      \"id\": \"q0000002\",\n      \"type\": \"pocket\",\n      \"v\": 1,\n      \"name\": \"Q\",\n      \"base\": \"p0000001\",\n      \"sketch\": \"s0000001\",\n      \"extent\": {\"type\": \"two_sides\", \"length\": 2, \"length2\": 1}\n    }",
            Operation::Pocket {
                base: id("p0000001"),
                sketch: id("s0000001"),
                extent: PocketExtent::TwoSides {
                    length: mm(2_000_000),
                    length2: mm(1_000_000),
                },
            },
        ),
        (
            "{\n      \"id\": \"q0000002\",\n      \"type\": \"pocket\",\n      \"v\": 1,\n      \"name\": \"Q\",\n      \"base\": \"p0000001\",\n      \"sketch\": \"s0000001\",\n      \"extent\": {\"type\": \"through_all\", \"reversed\": false}\n    }",
            Operation::Pocket {
                base: id("p0000001"),
                sketch: id("s0000001"),
                extent: PocketExtent::ThroughAll { reversed: false },
            },
        ),
        (
            "{\n      \"id\": \"r0000002\",\n      \"type\": \"revolve\",\n      \"v\": 1,\n      \"name\": \"R\",\n      \"sketch\": \"s0000001\",\n      \"axis\": \"x_axis\",\n      \"extent\": {\"type\": \"symmetric\", \"angle\": 90.000000001}\n    }",
            Operation::Revolve {
                base: None,
                sketch: id("s0000001"),
                axis: Axis::XAxis,
                extent: TurnExtent::Symmetric {
                    angle: Angle::from_ndeg(90_000_000_001),
                },
            },
        ),
        (
            "{\n      \"id\": \"g0000002\",\n      \"type\": \"groove\",\n      \"v\": 1,\n      \"name\": \"G\",\n      \"base\": \"p0000001\",\n      \"sketch\": \"s0000001\",\n      \"axis\": \"6t7v8w9x\",\n      \"extent\": {\"type\": \"one_side\", \"angle\": 0.000000001, \"reversed\": true}\n    }",
            Operation::Groove {
                base: id("p0000001"),
                sketch: id("s0000001"),
                axis: Axis::Line(ElementId::new("6t7v8w9x").unwrap()),
                extent: TurnExtent::OneSide {
                    angle: Angle::from_ndeg(1),
                    reversed: true,
                },
            },
        ),
    ];
    for (text, operation) in cases {
        let text = with_feature(text);
        let document = read(text.as_bytes()).unwrap();
        assert_eq!(document.features[0].operation, operation, "{text}");
        assert_eq!(write(&document).unwrap(), text);
        assert!(
            VALIDATOR.is_valid(&serde_json::from_str(&text).unwrap()),
            "{text}"
        );
    }
}

#[test]
fn any_spelling_of_a_sweep_writes_the_same_canonical_bytes() {
    // Keys in any order, the extent's too, exponents, and trailing zeros: the same pocket.
    let messy = r#"{"features": [{"extent": {"reversed": false, "length": 1e1, "type": "one_side"}, "sketch": "s0000002", "base": "p0000001", "name": "Bore", "v": 1, "type": "pocket", "id": "q0000001"}], "units": {"resolution": "1nm", "length": "mm"}, "silico": 1}"#;
    let tidy = with_feature(
        "{\n      \"id\": \"q0000001\",\n      \"type\": \"pocket\",\n      \"v\": 1,\n      \"name\": \"Bore\",\n      \"base\": \"p0000001\",\n      \"sketch\": \"s0000002\",\n      \"extent\": {\"type\": \"one_side\", \"length\": 10, \"reversed\": false}\n    }",
    );
    assert_eq!(write(&read(messy.as_bytes()).unwrap()).unwrap(), tidy);
}

/// The reader's refusal of `text`: `invalid_input`, with a suggestion.
fn refused(text: &str) -> Error {
    let err = read(text.as_bytes()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(err.suggestion().is_some(), "{err}");
    err
}

/// The example with `from` replaced by `to` (which must be in it).
fn example_with(from: &str, to: &str) -> String {
    assert!(EXAMPLE.contains(from), "{from}");
    EXAMPLE.replacen(from, to, 1)
}

#[test]
fn a_sweep_that_doesnt_fit_its_type_is_refused_naming_its_feature_and_field() {
    // (replace, with, feature, parameter, message before ", at", suggestion)
    let pad_extent = r#""extent": {"type": "one_side", "length": 10, "reversed": false}"#;
    let pocket_extent = r#""extent": {"type": "through_all", "reversed": true}"#;
    let revolve_extent = r#""extent": {"type": "one_side", "angle": 360, "reversed": false}"#;
    let cases: &[(&str, &str, &str, &str, &str, &str)] = &[
        // Unknown fields, in the feature and in its extent.
        (
            "\"sketch\": \"s0000001\",",
            "\"sketch\": \"s0000001\", \"length\": 10,",
            "p0000001",
            "length",
            "feature p0000001 has a key it can't have: \"length\"",
            "remove \"length\" (or check its spelling)",
        ),
        (
            "\"axis\": \"y_axis\",",
            "\"axis\": \"y_axis\", \"profile\": \"s0000003\",",
            "r0000001",
            "profile",
            "feature r0000001 has a key it can't have: \"profile\"",
            "remove \"profile\" (or check its spelling)",
        ),
        (
            "\"sketch\": \"s0000001\",",
            "\"sketch\": \"s0000001\", \"axis\": \"x_axis\",",
            "p0000001",
            "axis",
            "feature p0000001 has a key it can't have: \"axis\"",
            "remove \"axis\" (or check its spelling)",
        ),
        (
            pad_extent,
            r#""extent": {"type": "one_side", "length": 10, "reversed": false, "taper": 0}"#,
            "p0000001",
            "extent.taper",
            "extent has a key it can't have: \"taper\"",
            "remove \"taper\" (or check its spelling)",
        ),
        (
            pad_extent,
            r#""extent": {"type": "symmetric", "length": 10, "reversed": false}"#,
            "p0000001",
            "extent.reversed",
            "extent has a key it can't have: \"reversed\"",
            "remove \"reversed\" (or check its spelling)",
        ),
        (
            pocket_extent,
            r#""extent": {"type": "through_all", "reversed": true, "length": 1}"#,
            "q0000001",
            "extent.length",
            "extent has a key it can't have: \"length\"",
            "remove \"length\" (or check its spelling)",
        ),
        (
            revolve_extent,
            r#""extent": {"type": "one_side", "angle": 360, "reversed": false, "length": 1}"#,
            "r0000001",
            "extent.length",
            "extent has a key it can't have: \"length\"",
            "remove \"length\" (or check its spelling)",
        ),
        // Missing fields.
        (
            "\"sketch\": \"s0000001\",",
            "",
            "p0000001",
            "sketch",
            "feature p0000001 has no sketch",
            "add sketch to feature p0000001",
        ),
        (
            ",\n      \"extent\": {\"type\": \"through_all\", \"reversed\": true}",
            "",
            "q0000001",
            "extent",
            "feature q0000001 has no extent",
            "add extent to feature q0000001",
        ),
        (
            "\"base\": \"p0000001\",",
            "",
            "q0000001",
            "base",
            "feature q0000001 has no base",
            "add base to feature q0000001",
        ),
        (
            "\"axis\": \"y_axis\",",
            "",
            "r0000001",
            "axis",
            "feature r0000001 has no axis",
            "add axis to feature r0000001",
        ),
        (
            pad_extent,
            r#""extent": {"type": "one_side", "length": 10}"#,
            "p0000001",
            "extent.reversed",
            "extent has no reversed",
            "add reversed to extent",
        ),
        (
            pad_extent,
            r#""extent": {"type": "two_sides", "length": 10}"#,
            "p0000001",
            "extent.length2",
            "extent has no length2",
            "add length2 to extent",
        ),
        (
            pad_extent,
            r#""extent": {"length": 10, "reversed": false}"#,
            "p0000001",
            "extent.type",
            "extent has no type",
            "add type to extent",
        ),
        (
            revolve_extent,
            r#""extent": {"type": "one_side", "reversed": false}"#,
            "r0000001",
            "extent.angle",
            "extent has no angle",
            "add angle to extent",
        ),
        // Wrong types.
        (
            "\"sketch\": \"s0000001\",",
            "\"sketch\": 1,",
            "p0000001",
            "sketch",
            "sketch is a number, not a string",
            "write sketch as a string",
        ),
        (
            "\"base\": \"p0000001\",",
            "\"base\": null,",
            "q0000001",
            "base",
            "base is null, not a string",
            "write base as a string",
        ),
        (
            pad_extent,
            r#""extent": "one_side""#,
            "p0000001",
            "extent",
            "extent is a string, not an object",
            "write extent as an object",
        ),
        (
            pad_extent,
            r#""extent": {"type": "one_side", "length": 10, "reversed": "no"}"#,
            "p0000001",
            "extent.reversed",
            "extent.reversed is a string, not true or false",
            "write extent.reversed as true or false",
        ),
        (
            pad_extent,
            r#""extent": {"type": "one_side", "length": "=depth", "reversed": false}"#,
            "p0000001",
            "extent.length",
            "extent.length is a string, not a length in millimetres",
            "write extent.length as a number, such as 10.5: strings are reserved for expressions, which this Silico doesn't read yet",
        ),
        (
            pad_extent,
            r#""extent": {"type": 1, "length": 10, "reversed": false}"#,
            "p0000001",
            "extent.type",
            "extent.type is a number, not a string",
            "write extent.type as a string",
        ),
        (
            revolve_extent,
            r#""extent": {"type": "one_side", "angle": "=a", "reversed": false}"#,
            "r0000001",
            "extent.angle",
            "extent.angle is a string, not an angle in degrees",
            "write extent.angle as a number, such as 45: strings are reserved for expressions, which this Silico doesn't read yet",
        ),
        (
            "\"axis\": \"y_axis\",",
            "\"axis\": [0, 1],",
            "r0000001",
            "axis",
            "axis is an array, not a string",
            "write axis as a string",
        ),
        // Values the schema refuses too.
        (
            pad_extent,
            r#""extent": {"type": "through_all", "reversed": false}"#,
            "p0000001",
            "extent.type",
            "extent.type through_all is a pocket's: a pad goes a length from its sketch",
            "write extent.type as one_side, symmetric, or two_sides",
        ),
        // A pad's up-to-face and a revolve's through-all are neither of theirs, not a
        // pocket's through-all (found by the mutation run: either, not both, read as that).
        (
            pad_extent,
            r#""extent": {"type": "up_to_face", "length": 10}"#,
            "p0000001",
            "extent.type",
            "extent.type \"up_to_face\" isn't one of a pad's, one_side, symmetric, two_sides",
            "write extent.type as one of one_side, symmetric, two_sides",
        ),
        (
            revolve_extent,
            r#""extent": {"type": "through_all", "reversed": false}"#,
            "r0000001",
            "extent.type",
            "extent.type \"through_all\" isn't one of a revolve's, one_side, symmetric",
            "write extent.type as one of one_side, symmetric",
        ),
        (
            pocket_extent,
            r#""extent": {"type": "up_to_face", "reversed": true}"#,
            "q0000001",
            "extent.type",
            "extent.type \"up_to_face\" isn't one of a pocket's, one_side, symmetric, two_sides, through_all",
            "write extent.type as one of one_side, symmetric, two_sides, through_all",
        ),
        (
            revolve_extent,
            r#""extent": {"type": "two_sides", "length": 1, "length2": 2}"#,
            "r0000001",
            "extent.type",
            "extent.type \"two_sides\" isn't one of a revolve's, one_side, symmetric",
            "write extent.type as one of one_side, symmetric",
        ),
        (
            "\"axis\": \"y_axis\",",
            "\"axis\": \"origin\",",
            "r0000001",
            "axis",
            "axis origin is the sketch's origin, a point: a revolve turns about a line",
            "name a line of the sketch by its ID, or x_axis or y_axis",
        ),
        (
            "\"axis\": \"y_axis\",",
            "\"axis\": \"Y\",",
            "r0000001",
            "axis",
            "axis \"Y\" isn't x_axis, y_axis, or a sketch element's ID",
            "name a line of the sketch by its ID, or x_axis or y_axis",
        ),
        (
            "\"sketch\": \"s0000001\",",
            "\"sketch\": \"S0000001\",",
            "p0000001",
            "sketch",
            "sketch \"S0000001\" isn't a feature ID",
            "an ID is 8 characters of digits and lower-case letters but i, l, o, and u",
        ),
        (
            pad_extent,
            r#""extent": {"type": "one_side", "length": 10.0000001, "reversed": false}"#,
            "p0000001",
            "extent.length",
            "extent.length 10.0000001 has more than 6 decimals",
            "round it to 6 decimals: lengths are whole nanometres",
        ),
        (
            revolve_extent,
            r#""extent": {"type": "one_side", "angle": 1.0000000001, "reversed": false}"#,
            "r0000001",
            "extent.angle",
            "extent.angle 1.0000000001 has more than 9 decimals",
            "round it to 9 decimals: angles are whole nanodegrees",
        ),
    ];
    for &(from, to, feature, parameter, message, suggestion) in cases {
        let text = example_with(from, to);
        let err = refused(&text);
        assert_eq!(err.feature(), Some(feature), "{to}: {err}");
        assert_eq!(err.parameter(), Some(parameter), "{to}: {err}");
        let said = err.message().split(", at line").next().unwrap();
        assert_eq!(said, message, "{to}: {err}");
        assert!(err.message().contains(", at line "), "{err}");
        assert_eq!(err.suggestion(), Some(suggestion), "{to}: {err}");
        assert!(err.elements().is_empty(), "{err}");
    }
}

#[test]
fn a_groove_refuses_the_origin_as_a_groove() {
    let text = with_feature(
        r#"{"id": "g0000001", "type": "groove", "v": 1, "name": "G", "base": "p0000001", "sketch": "s0000001", "axis": "origin", "extent": {"type": "symmetric", "angle": 10}}"#,
    );
    let err = refused(&text);
    assert!(
        err.message().starts_with(
            "axis origin is the sketch's origin, a point: a groove turns about a line, at line 6"
        ),
        "{err}"
    );
}

/// The published schema, as an agent's validator compiles it.
static VALIDATOR: std::sync::LazyLock<jsonschema::Validator> = std::sync::LazyLock::new(|| {
    let schema: serde_json::Value = serde_json::from_str(&schema().unwrap()).unwrap();
    jsonschema::validator_for(&schema).unwrap()
});

fn valid(text: &str) -> bool {
    VALIDATOR.is_valid(&serde_json::from_str(text).unwrap())
}

#[test]
fn the_schema_takes_the_example_and_refuses_what_the_reader_refuses_by_shape() {
    assert!(valid(EXAMPLE));
    let pad_extent = r#""extent": {"type": "one_side", "length": 10, "reversed": false}"#;
    let pocket_extent = r#""extent": {"type": "through_all", "reversed": true}"#;
    let revolve_extent = r#""extent": {"type": "one_side", "angle": 360, "reversed": false}"#;
    for (from, to) in [
        // through_all on a pad: by the schema, not only the reader.
        (
            pad_extent,
            r#""extent": {"type": "through_all", "reversed": false}"#,
        ),
        (
            pad_extent,
            r#""extent": {"type": "one_side", "length": 10}"#,
        ),
        (
            pad_extent,
            r#""extent": {"type": "symmetric", "length": 10, "reversed": false}"#,
        ),
        (
            pocket_extent,
            r#""extent": {"type": "through_all", "reversed": true, "length": 1}"#,
        ),
        (
            revolve_extent,
            r#""extent": {"type": "one_side", "length": 360, "reversed": false}"#,
        ),
        ("\"base\": \"p0000001\",", ""),
        ("\"base\": \"p0000001\",", "\"base\": null,"),
        ("\"axis\": \"y_axis\",", ""),
        ("\"axis\": \"y_axis\",", "\"axis\": \"origin\","),
        ("\"sketch\": \"s0000001\",", ""),
        (
            "\"sketch\": \"s0000001\",",
            "\"sketch\": \"s0000001\", \"axis\": \"x_axis\",",
        ),
    ] {
        let text = example_with(from, to);
        assert!(read(text.as_bytes()).is_err(), "{to}");
        assert!(!valid(&text), "{to}");
    }
}

#[test]
fn a_fillet_naming_a_sweeps_element_says_the_sweeps_type_and_reads_its_names() {
    // M3.6, step v: a pad's, pocket's, revolve's, or groove's elements are named (design
    // S4), each saying its type: a name without it is refused, saying what to write, and
    // one with it reads.
    let sketch = rectangle("s0000001", "xy", [0.0, 0.0], 4.0, 2.0);
    let invalid = checked(&[
        &sketch,
        &pad("p0000001", None, "s0000001", TEN),
        r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "base": "p0000001", "edges": ["p0000001.Edge(G s0000001.Edge1)"], "radius": 1}"#,
    ])
    .unwrap_err();
    assert_eq!(invalid.total(), 1);
    assert!(
        invalid.errors()[0]
            .message()
            .contains("as `p0000001.pad.Edge(G s0000001.Edge1)`"),
        "{}",
        invalid.errors()[0].message()
    );
    checked(&[
        &sketch,
        &pad("p0000001", None, "s0000001", TEN),
        r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "base": "p0000001", "edges": ["p0000001.pad.Edge(G s0000001.sketch.Edge.na000003)"], "radius": 1}"#,
    ])
    .unwrap();
    // Each type says what it is.
    for (feature, word) in [
        (pocket("q0000001", "k7f2qx3m", "s0000001", TEN), "pocket"),
        (
            turned(
                "q0000001",
                "groove",
                Some("k7f2qx3m"),
                "s0000001",
                "x_axis",
                TURN,
            ),
            "groove",
        ),
    ] {
        let invalid = checked(&[
            &sketch,
            &block("k7f2qx3m"),
            &feature,
            r#"{"id": "f0000001", "type": "chamfer", "v": 1, "name": "F", "base": "q0000001", "edges": ["k7f2qx3m.box.Edge1", "q0000001.Edge2"], "size": {"distance": 1}}"#,
        ])
        .unwrap_err();
        assert_eq!(invalid.total(), 1, "{:#?}", invalid.errors());
        assert_eq!(invalid.errors()[0].parameter(), Some("edges.1"));
        assert!(
            invalid.errors()[0]
                .message()
                .contains(&format!("as `q0000001.{word}.Edge2`")),
            "{}",
            invalid.errors()[0].message()
        );
    }
}

// Recompute and the outline.

#[test]
fn a_fillet_naming_an_edge_made_from_a_sketchs_only_region_is_admitted() {
    // Design S4: a sketch's only region, `s.sketch.Face1`, is stable by design. An edge
    // where a box's side meets a pocket's floor names it inside, and passes the gate; the
    // same edge named from the pocket's face by number doesn't (found by the mutation run).
    let sketch = rectangle("s0000001", "xy", [0.0, 0.0], 4.0, 2.0);
    let fillet = |edge: &str| {
        format!(
            r#"{{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "base": "q0000001", "edges": ["{edge}"], "radius": 1}}"#
        )
    };
    let features = |edge: &str| {
        [
            sketch.clone(),
            block("k7f2qx3m"),
            pocket("q0000001", "k7f2qx3m", "s0000001", TEN),
            fillet(edge),
        ]
    };
    let floor = features(
        "q0000001.pocket.Edge(G k7f2qx3m.box.Face1, G q0000001.pocket.Face(G s0000001.sketch.Face1))",
    );
    checked(&floor.iter().map(String::as_str).collect::<Vec<_>>()).unwrap();
    let numbered = features("q0000001.pocket.Edge(G k7f2qx3m.box.Face1, G q0000001.pocket.Face3)");
    let invalid = checked(&numbered.iter().map(String::as_str).collect::<Vec<_>>()).unwrap_err();
    assert_eq!(invalid.total(), 1, "{:#?}", invalid.errors());
    assert!(
        invalid.errors()[0]
            .message()
            .contains("a duplicate (~n) or an operation's element by its number"),
        "{}",
        invalid.errors()[0]
    );
}

#[test]
fn recompute_builds_a_sweep_from_its_sketch_and_what_uses_it() {
    // M3.6, step iv (design S3): a pad onto a base is its faces, its prism, and its fuse,
    // and a feature using it builds on it. Each sweep feature alike, with its own steps.
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let sketch = rectangle("s0000001", "xy", [0.0, 0.0], 4.0, 2.0);
    // A 10 mm cube under the sketch's plane: a pad goes up from its top, a pocket down into
    // it, and a turn about the sketch's x axis passes through it.
    let base = box_at("k7f2qx3m", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0);
    let model = published(
        &engine,
        checked_document(&[
            &sketch,
            &base,
            &pad("p0000001", Some("k7f2qx3m"), "s0000001", TEN),
            &block("a91cw0de"),
            r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "F", "base": "p0000001", "tool": "a91cw0de"}"#,
        ]),
    );
    for feature in ["k7f2qx3m", "p0000001", "f0000001"] {
        assert!(
            matches!(
                model.outcome(&id(feature)),
                Some(FeatureOutcome::Built { .. })
            ),
            "{feature}: {:?}",
            model.outcome(&id(feature))
        );
    }
    assert!(matches!(
        model.outcome(&id("s0000001")),
        Some(FeatureOutcome::Sketch(_))
    ));
    assert_eq!(model.bodies(), [id("f0000001")]);
    // The cube made and moved, the small box made, the pad's three steps, and the fuse,
    // in whatever order they ran.
    let mut calls = made(&engine);
    calls.sort_unstable();
    assert_eq!(
        calls,
        [
            "Fuse",
            "Fuse",
            "MakeBox",
            "MakeBox",
            "MakeFaces",
            "Prism",
            "Translate"
        ],
        "{calls:?}"
    );
    // Each type alike: a pocket and a groove cut, a revolve with no base is its faces and
    // its revolution alone.
    for (feature, word, steps) in [
        (
            pocket("x0000001", "k7f2qx3m", "s0000001", TEN),
            "pocket",
            &["MakeFaces", "Prism", "Cut"][..],
        ),
        (
            turned("x0000001", "revolve", None, "s0000001", "x_axis", TURN),
            "revolve",
            &["MakeFaces", "Revolution"][..],
        ),
        (
            turned(
                "x0000001",
                "groove",
                Some("k7f2qx3m"),
                "s0000001",
                "x_axis",
                TURN,
            ),
            "groove",
            &["MakeFaces", "Revolution", "Cut"][..],
        ),
    ] {
        let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
        let model = published(&engine, checked_document(&[&sketch, &base, &feature]));
        assert!(
            matches!(
                model.outcome(&id("x0000001")),
                Some(FeatureOutcome::Built { .. })
            ),
            "{word}: {:?}",
            model.outcome(&id("x0000001"))
        );
        // The feature's own steps, which run one after another.
        let made = made(&engine);
        let own: Vec<&str> = made
            .iter()
            .copied()
            .filter(|m| !["MakeBox", "Translate"].contains(m))
            .collect();
        assert_eq!(own, steps, "{word}");
    }
}

#[test]
fn a_sweep_whose_description_is_refused_or_fails_still_builds_and_so_does_what_uses_it() {
    // A pad's check reads its solid's summary, never its description (M4.4's decision 4):
    // a description refused for the solid's values, or a worker crashing on it, leaves every
    // summary standing, so the pad builds, nothing that uses it is blocked, drift is empty,
    // and only its description is missing, the reason kept beside it.
    let refused = silico_protocol::Failure::new(silico_protocol::Refusal::ResultInvalid {
        operation: silico_protocol::Operation::Other,
        edges: Vec::new(),
    });
    for code in [ErrorCode::WorkerCrashed, ErrorCode::InvalidResult] {
        let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
        match code {
            ErrorCode::InvalidResult => {
                let refused = refused.clone();
                engine
                    .kernel()
                    .refuse_as_worker(move |op| (op.name() == "Describe").then(|| refused.clone()));
            }
            _ => engine.kernel().refuse(|op| {
                (op.name() == "Describe")
                    .then(|| Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed"))
            }),
        }
        let model = published(
            &engine,
            checked_document(&[
                &rectangle("s0000001", "xy", [0.0, 0.0], 4.0, 2.0),
                &box_at("k7f2qx3m", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0),
                &pad("p0000001", Some("k7f2qx3m"), "s0000001", TEN),
                &block("a91cw0de"),
                r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "F", "base": "p0000001", "tool": "a91cw0de"}"#,
            ]),
        );
        assert!(model.drift().is_empty(), "{code:?}: {:?}", model.drift());
        assert_eq!(model.errors().count(), 0, "{code:?}");
        for feature in ["k7f2qx3m", "p0000001", "a91cw0de", "f0000001"] {
            let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&id(feature)) else {
                panic!("{code:?}: {feature}: {:?}", model.outcome(&id(feature)));
            };
            assert!(built.summary().is_some(), "{code:?}: {feature}");
            assert!(
                model.description(&id(feature)).is_none(),
                "{code:?}: {feature}"
            );
            assert_eq!(
                built.why_undescribed().map(Error::code),
                Some(code),
                "{code:?}: {feature}"
            );
        }
        assert_eq!(model.bodies(), [id("f0000001")]);
    }
}

#[test]
fn a_sweep_whose_base_fails_is_blocked_on_the_base() {
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    engine
        .kernel()
        .refuse(|_| Some(Error::new(ErrorCode::KernelFailed, "no")));
    let model = published(
        &engine,
        checked_document(&[
            &empty_sketch("s0000001", "S", "xy"),
            &block("k7f2qx3m"),
            &pocket("q0000001", "k7f2qx3m", "s0000001", TEN),
        ]),
    );
    assert!(matches!(
        model.outcome(&id("k7f2qx3m")),
        Some(FeatureOutcome::Failed(_))
    ));
    assert_eq!(
        model.outcome(&id("q0000001")),
        Some(&FeatureOutcome::Blocked { on: id("k7f2qx3m") })
    );
}

#[test]
fn the_outline_says_each_sweeps_fields() {
    let outline = command::outline(&read(EXAMPLE.as_bytes()).unwrap());
    assert_eq!(
        outline,
        "s0000001 \"Outline\" sketch plane xy\n\
         p0000001 \"Block\" pad sketch s0000001, extent one_side, extent.length 10, extent.reversed false\n\
         s0000002 \"Hole\" sketch plane xy\n\
         q0000001 \"Bore\" pocket base p0000001, sketch s0000002, extent through_all, extent.reversed true\n\
         s0000003 \"Boss profile\" sketch plane xz\n\
         r0000001 \"Boss\" revolve base q0000001, sketch s0000003, axis y_axis, extent one_side, extent.angle 360, extent.reversed false\n\
         bodies r0000001\n"
    );
}

#[test]
fn a_document_built_in_memory_round_trips() {
    let document = Document {
        silico: FormatVersion,
        units: Units,
        naming: NamingScheme,
        features: vec![feature(
            "g0000001",
            "Groove",
            Operation::Groove {
                base: id("p0000001"),
                sketch: id("s0000001"),
                axis: Axis::Line(ElementId::new("0123abcd").unwrap()),
                extent: TurnExtent::Symmetric {
                    angle: Angle::from_ndeg(45_500_000_000),
                },
            },
        )],
    };
    let text = write(&document).unwrap();
    assert_eq!(
        text,
        with_feature(
            "{\n      \"id\": \"g0000001\",\n      \"type\": \"groove\",\n      \"v\": 1,\n      \"name\": \"Groove\",\n      \"base\": \"p0000001\",\n      \"sketch\": \"s0000001\",\n      \"axis\": \"0123abcd\",\n      \"extent\": {\"type\": \"symmetric\", \"angle\": 45.5}\n    }"
        )
    );
    assert_eq!(read(text.as_bytes()).unwrap(), document);
}
