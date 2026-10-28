//! break-it on M3.6 step (i): pad, pocket, revolve, and groove in the document (reader,
//! canonical writer, schema), the gate's rules for them, and `sweep::check_axis_length`
//! (docs/design/m3.6-sketch-features.md, S1 and S6). Hostile inputs: extreme and
//! negative numbers, duplicate keys, huge strings, near-miss axis spellings, the reader
//! and the schema disagreeing, every in-memory value through write then read, odd
//! references, a gate run on the largest document the reader takes, and recompute's
//! refusal's effect on what's stored. Expected values are from the design's text.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::model::WAIT;
use super::sweeps;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use proptest::prelude::*;
use silico_core::document::{
    Angle, Axis, Checked, Coordinate, Document, ElementId, Feature, FeatureVersion, FormatVersion,
    ID_ALPHABET, Length, Name, NamingScheme, Operation, PadExtent, PocketExtent, TurnExtent, Units,
    read, schema, validate, write,
};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::sketch::SketchSolution;
use silico_core::sweep::check_axis_length;
use silico_core::{ErrorCode, command};
use sweeps::*;

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0xb4ea_c1e5),
        ..ProptestConfig::default()
    }
}

/// A box, a sketch, and `feature` (a JSON object's text) after them.
fn with(feature: &str) -> String {
    document(&[&block("k7f2qx3m"), PROFILE, feature])
}

// ---------------------------------------------------------------------------------------
// The reader: numbers at and past the edges of what the file's integers hold.

#[test]
fn length_past_i64_nanometres_is_refused_by_the_reader_naming_the_field() {
    // i64::MAX nm is 9223372036854.775807 mm, the most a length is either way; one nm more
    // doesn't fit.
    for text in [
        "9223372036854.775808",
        "-9223372036854.775808",
        "-9223372036854.775809",
        "1e400",
        "1e13",
    ] {
        let err =
            read(with(&pad("p0000001", None, "s0000003", &one_side(text))).as_bytes()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{text}: {err}");
        assert_eq!(err.feature(), Some("p0000001"), "{text}: {err}");
        assert_eq!(err.parameter(), Some("extent.length"), "{text}: {err}");
        assert!(err.suggestion().is_some(), "{text}: {err}");
    }
}

#[test]
fn length_at_i64_nanometres_reads_and_the_gate_refuses_it() {
    let text = with(&pad(
        "p0000001",
        None,
        "s0000003",
        &one_side("9223372036854.775807"),
    ));
    let document = read(text.as_bytes()).unwrap();
    let invalid = validate(&document).unwrap_err();
    assert_eq!(invalid.total(), 1, "{:#?}", invalid.errors());
    assert_eq!(invalid.errors()[0].parameter(), Some("extent.length"));
    // And it writes back as it was written.
    assert_eq!(
        read(write(&document).unwrap().as_bytes()).unwrap(),
        document
    );
}

#[test]
fn angle_past_i64_nanodegrees_is_refused_by_the_reader_naming_the_field() {
    // i64::MAX ndeg is 9223372036.854775807°.
    for text in ["9223372036.854775808", "-9223372036.854775809", "1e300"] {
        let err = read(
            with(&turned(
                "r0000001",
                "revolve",
                None,
                "s0000003",
                "x_axis",
                &angle(text),
            ))
            .as_bytes(),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{text}: {err}");
        assert_eq!(err.parameter(), Some("extent.angle"), "{text}: {err}");
    }
}

#[test]
fn negative_zero_is_refused_by_the_reader_and_zero_by_the_gate() {
    // A minus sign on zero is refused where it's written, naming the field.
    for (extent, parameter) in [
        (one_side("-0"), "extent.length"),
        (one_side("-0.0"), "extent.length"),
        (one_side("-0e-5"), "extent.length"),
        (angle("-0"), "extent.angle"),
    ] {
        let feature = if parameter == "extent.angle" {
            turned("r0000001", "revolve", None, "s0000003", "x_axis", &extent)
        } else {
            pad("r0000001", None, "s0000003", &extent)
        };
        let err = read(with(&feature).as_bytes()).unwrap_err();
        assert_eq!(err.feature(), Some("r0000001"), "{extent}: {err}");
        assert_eq!(err.parameter(), Some(parameter), "{extent}: {err}");
        assert_eq!(err.suggestion(), Some("write 0"), "{extent}: {err}");
    }
    // Zero by any other spelling reads, and the gate refuses it.
    for extent in [one_side("0e5"), one_side("0.000000"), one_side("0E-3")] {
        let error = problem(&[PROFILE, &pad("p0000001", None, "s0000003", &extent)]);
        assert_refused(
            &error,
            "p0000001",
            "extent.length",
            "pad extent.length must be a length above 0 mm and at most 1000000 mm (a kilometre), got 0",
            "give the pad a valid extent.length",
        );
    }
    let error = problem(&[
        PROFILE,
        &turned(
            "r0000001",
            "revolve",
            None,
            "s0000003",
            "x_axis",
            &angle("0.0"),
        ),
    ]);
    assert_refused(
        &error,
        "r0000001",
        "extent.angle",
        "revolve extent.angle must be an angle above 0° and at most 360°, got 0°",
        "give the revolve a valid extent.angle",
    );
}

#[test]
fn a_whole_turn_spelled_with_an_exponent_and_trailing_zeros_passes_and_one_ndeg_more_doesnt() {
    for (text, ok) in [
        ("3.6e2", true),
        ("360.000000000", true),
        ("36000000000e-8", true),
        ("360.000000001", false),
        ("3.60000000001e2", false),
        ("0.000000001", true),
        ("1e-9", true),
    ] {
        let result = checked(&[
            PROFILE,
            &turned(
                "r0000001",
                "revolve",
                None,
                "s0000003",
                "x_axis",
                &angle(text),
            ),
        ]);
        assert_eq!(result.is_ok(), ok, "{text}: {result:?}");
    }
}

// ---------------------------------------------------------------------------------------
// The reader: keys given twice, and strings huge or hostile.

#[test]
fn an_extent_given_twice_is_refused() {
    let text = with(
        r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000003", "extent": {"type": "one_side", "length": 10, "reversed": false}, "extent": {"type": "one_side", "length": 20, "reversed": false}}"#,
    );
    // Refused as JSON, before any feature is read: by line and column.
    let err = read(text.as_bytes()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(
        err.message()
            .starts_with("the document isn't valid JSON: the key \"extent\" is written twice"),
        "{err}"
    );
}

#[test]
fn an_extent_type_given_twice_is_refused() {
    let text = with(&pad(
        "p0000001",
        None,
        "s0000003",
        r#"{"type": "one_side", "type": "symmetric", "length": 10, "reversed": false}"#,
    ));
    let err = read(text.as_bytes()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(
        err.message()
            .starts_with("the document isn't valid JSON: the key \"type\" is written twice"),
        "{err}"
    );
}

#[test]
fn a_megabyte_axis_or_extent_type_gives_a_short_error() {
    let huge = "x".repeat(1 << 20);
    for text in [
        with(&turned(
            "r0000001", "revolve", None, "s0000003", &huge, TURN,
        )),
        with(&pad(
            "p0000001",
            None,
            "s0000003",
            &format!(r#"{{"type": "{huge}", "length": 1, "reversed": false}}"#),
        )),
        with(&pad(
            "p0000001",
            None,
            "s0000003",
            &format!(r#"{{"type": "one_side", "length": 1, "reversed": false, "{huge}": 1}}"#),
        )),
    ] {
        let err = read(text.as_bytes()).unwrap_err();
        let size = err.message().len()
            + err.suggestion().map_or(0, str::len)
            + err.parameter().map_or(0, str::len);
        assert!(size < 4096, "an error of {size} bytes");
    }
}

#[test]
fn axis_spellings_near_an_axis_are_refused_by_reader_and_schema_alike() {
    for axis in [
        "x_axis ",
        " x_axis",
        "X_AXIS",
        "x-axis",
        "xaxis",
        "z_axis",
        "x_axis\n",
        "x_axis\u{0}",
        "\u{feff}y_axis",
        "na00000",   // 7 characters
        "na0000000", // 9 characters
        "na00000o",  // o isn't in the alphabet
        "NA000000",
        "",
    ] {
        let escaped = serde_json::to_string(axis).unwrap();
        let feature = format!(
            r#"{{"id": "r0000001", "type": "revolve", "v": 1, "name": "R", "sketch": "s0000003", "axis": {escaped}, "extent": {TURN}}}"#
        );
        let text = schema_doc(&feature);
        let err = read(text.as_bytes()).unwrap_err();
        assert_eq!(err.parameter(), Some("axis"), "{axis:?}: {err}");
        assert!(!schema_valid(&text), "{axis:?}: the schema takes it");
    }
}

// ---------------------------------------------------------------------------------------
// The schema against the reader.

static VALIDATOR: std::sync::LazyLock<jsonschema::Validator> = std::sync::LazyLock::new(|| {
    let schema: serde_json::Value = serde_json::from_str(&schema().unwrap()).unwrap();
    jsonschema::validator_for(&schema).unwrap()
});

fn schema_valid(text: &str) -> bool {
    VALIDATOR.is_valid(&serde_json::from_str(text).unwrap())
}

/// A document of these features with the header the schema requires (`naming`, M4.0), so
/// the schema refuses one only for what its feature says. Checked against a revolve the
/// reader and the schema both take.
fn schema_doc(feature: &str) -> String {
    let doc =
        |feature: &str| with(feature).replacen(r#""units": "#, r#""naming": 5, "units": "#, 1);
    let good = doc(&format!(
        r#"{{"id": "r0000001", "type": "revolve", "v": 1, "name": "R", "sketch": "s0000003", "axis": "x_axis", "extent": {TURN}}}"#
    ));
    assert!(read(good.as_bytes()).is_ok(), "the reader refuses {good}");
    assert!(schema_valid(&good), "the schema refuses {good}");
    doc(feature)
}

#[test]
fn shapes_the_reader_refuses_the_schema_refuses_too() {
    for feature in [
        // A pad's base as null.
        r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "P", "base": null, "sketch": "s0000003", "extent": {"type": "one_side", "length": 1, "reversed": false}}"#,
        // A revolve's base as null.
        r#"{"id": "r0000001", "type": "revolve", "v": 1, "name": "R", "base": null, "sketch": "s0000003", "axis": "x_axis", "extent": {"type": "one_side", "angle": 1, "reversed": false}}"#,
        // A groove through all.
        r#"{"id": "g0000001", "type": "groove", "v": 1, "name": "G", "base": "k7f2qx3m", "sketch": "s0000003", "axis": "x_axis", "extent": {"type": "through_all", "reversed": false}}"#,
        // A groove with no base.
        r#"{"id": "g0000001", "type": "groove", "v": 1, "name": "G", "sketch": "s0000003", "axis": "x_axis", "extent": {"type": "one_side", "angle": 1, "reversed": false}}"#,
        // A pocket through all with no reversed.
        r#"{"id": "q0000001", "type": "pocket", "v": 1, "name": "Q", "base": "k7f2qx3m", "sketch": "s0000003", "extent": {"type": "through_all"}}"#,
        // A pocket two sides with only one length.
        r#"{"id": "q0000001", "type": "pocket", "v": 1, "name": "Q", "base": "k7f2qx3m", "sketch": "s0000003", "extent": {"type": "two_sides", "length": 1}}"#,
        // A pad symmetric by angle.
        r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "P", "sketch": "s0000003", "extent": {"type": "symmetric", "angle": 1}}"#,
        // A revolve symmetric with reversed.
        r#"{"id": "r0000001", "type": "revolve", "v": 1, "name": "R", "sketch": "s0000003", "axis": "x_axis", "extent": {"type": "symmetric", "angle": 1, "reversed": true}}"#,
        // An extent whose type is null.
        r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "P", "sketch": "s0000003", "extent": {"type": null, "length": 1, "reversed": false}}"#,
        // An extent that's an empty object.
        r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "P", "sketch": "s0000003", "extent": {}}"#,
        // An extent that's an array.
        r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "P", "sketch": "s0000003", "extent": [{"type": "one_side", "length": 1, "reversed": false}]}"#,
        // reversed as 0.
        r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "P", "sketch": "s0000003", "extent": {"type": "one_side", "length": 1, "reversed": 0}}"#,
        // A pocket with a pad's form and a pocket-only key.
        r#"{"id": "q0000001", "type": "pocket", "v": 1, "name": "Q", "base": "k7f2qx3m", "sketch": "s0000003", "extent": {"type": "symmetric", "length": 1, "length2": 1}}"#,
        // A pad with a tool.
        r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "P", "sketch": "s0000003", "tool": "k7f2qx3m", "extent": {"type": "one_side", "length": 1, "reversed": false}}"#,
    ] {
        let text = schema_doc(feature);
        assert!(read(text.as_bytes()).is_err(), "the reader takes {feature}");
        assert!(!schema_valid(&text), "the schema takes {feature}");
    }
}

/// One field of a generated sweep: left out, or written as this text.
fn field(name: &'static str, values: Vec<&'static str>) -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        1 => Just(None),
        6 => proptest::sample::select(values).prop_map(move |v| Some(format!("\"{name}\": {v}"))),
    ]
}

fn extent_member(
    name: &'static str,
    values: Vec<&'static str>,
) -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        1 => Just(None),
        2 => proptest::sample::select(values).prop_map(move |v| Some(format!("\"{name}\": {v}"))),
    ]
}

/// A sweep made of fields each present or not, and each well or badly typed, with plain
/// integers for numbers and valid IDs, so the reader refuses it only for its shape.
fn sweep_text() -> impl Strategy<Value = String> {
    (
        proptest::sample::select(vec!["pad", "pocket", "revolve", "groove"]),
        field("base", vec!["\"k7f2qx3m\"", "null", "1"]),
        field("sketch", vec!["\"s0000003\"", "null"]),
        field(
            "axis",
            vec![
                "\"x_axis\"",
                "\"y_axis\"",
                "\"na000000\"",
                "\"origin\"",
                "3",
            ],
        ),
        extent_member(
            "type",
            vec![
                "\"one_side\"",
                "\"symmetric\"",
                "\"two_sides\"",
                "\"through_all\"",
                "\"up_to_face\"",
                "7",
            ],
        ),
        extent_member("length", vec!["5", "\"5\"", "true"]),
        extent_member("length2", vec!["6", "null"]),
        extent_member("angle", vec!["30", "[30]"]),
        extent_member("reversed", vec!["true", "false", "1"]),
        prop_oneof![5 => Just(true), 1 => Just(false)],
    )
        .prop_map(
            |(kind, base, sketch, axis, ty, length, length2, angle, reversed, has_extent)| {
                let mut members: Vec<String> = Vec::new();
                for member in [ty, length, length2, angle, reversed].into_iter().flatten() {
                    members.push(member);
                }
                let mut fields = vec![
                    "\"id\": \"x0000001\"".to_owned(),
                    format!("\"type\": \"{kind}\""),
                    "\"v\": 1".to_owned(),
                    "\"name\": \"X\"".to_owned(),
                ];
                fields.extend([base, sketch, axis].into_iter().flatten());
                if has_extent {
                    fields.push(format!("\"extent\": {{{}}}", members.join(", ")));
                }
                format!("{{{}}}", fields.join(", "))
            },
        )
}

proptest! {
    #![proptest_config(config(4000))]

    /// Every sweep the reader takes, the schema takes; every one the schema takes, the
    /// reader takes (numbers are plain integers and IDs valid, so only shape decides).
    #[test]
    fn the_schema_and_the_reader_agree_on_a_sweeps_shape(feature in sweep_text()) {
        // With the header the schema requires (`naming`, M4.0): the reader takes a document
        // without it too, as written before it, so only a sweep's shape decides here.
        let text = with(&feature).replacen(r#""units": "#, r#""naming": 5, "units": "#, 1);
        prop_assert!(text.contains(r#""naming": 5"#));
        let read_ok = read(text.as_bytes()).is_ok();
        let schema_ok = schema_valid(&text);
        prop_assert_eq!(read_ok, schema_ok, "{}", feature);
    }
}

// ---------------------------------------------------------------------------------------
// The writer: every value the in-memory types hold writes a file that reads back as it.

/// A length as far as a document holds it: at most i64::MAX nm either way (i64::MIN, which
/// only code can make, the reader refuses).
fn length_nm() -> impl Strategy<Value = i64> {
    prop_oneof![
        Just(-i64::MAX),
        Just(i64::MAX),
        Just(-1),
        Just(0),
        Just(1),
        Just(1_000_000_000_000),
        Just(1_000_000_000_001),
        any::<i64>().prop_filter("at most i64::MAX either way", |&n| n != i64::MIN),
        -10_000_000i64..10_000_000,
    ]
}

fn axis_value() -> impl Strategy<Value = Axis> {
    prop_oneof![
        Just(Axis::XAxis),
        Just(Axis::YAxis),
        proptest::collection::vec(
            proptest::sample::select(ID_ALPHABET.chars().collect::<Vec<_>>()),
            8
        )
        .prop_map(|c| Axis::Line(ElementId::new(&c.into_iter().collect::<String>()).unwrap())),
    ]
}

fn sweep_operation() -> impl Strategy<Value = Operation> {
    let base = || prop_oneof![Just(None), Just(Some(id("k7f2qx3m")))];
    let sketch = id("s0000003");
    let pad_extent = prop_oneof![
        (length_nm(), any::<bool>()).prop_map(|(l, r)| PadExtent::OneSide {
            length: Length::from_nm(l),
            reversed: r
        }),
        length_nm().prop_map(|l| PadExtent::Symmetric {
            length: Length::from_nm(l)
        }),
        (length_nm(), length_nm()).prop_map(|(l, l2)| PadExtent::TwoSides {
            length: Length::from_nm(l),
            length2: Length::from_nm(l2)
        }),
    ];
    let pocket_extent = prop_oneof![
        (length_nm(), any::<bool>()).prop_map(|(l, r)| PocketExtent::OneSide {
            length: Length::from_nm(l),
            reversed: r
        }),
        length_nm().prop_map(|l| PocketExtent::Symmetric {
            length: Length::from_nm(l)
        }),
        (length_nm(), length_nm()).prop_map(|(l, l2)| PocketExtent::TwoSides {
            length: Length::from_nm(l),
            length2: Length::from_nm(l2)
        }),
        any::<bool>().prop_map(|r| PocketExtent::ThroughAll { reversed: r }),
    ];
    let turn = || {
        prop_oneof![
            (length_nm(), any::<bool>()).prop_map(|(a, r)| TurnExtent::OneSide {
                angle: Angle::from_ndeg(a),
                reversed: r
            }),
            length_nm().prop_map(|a| TurnExtent::Symmetric {
                angle: Angle::from_ndeg(a)
            }),
        ]
    };
    let (s1, s2, s3, s4) = (sketch.clone(), sketch.clone(), sketch.clone(), sketch);
    prop_oneof![
        (base(), pad_extent).prop_map(move |(base, extent)| Operation::Pad {
            base,
            sketch: s1.clone(),
            extent
        }),
        pocket_extent.prop_map(move |extent| Operation::Pocket {
            base: id("k7f2qx3m"),
            sketch: s2.clone(),
            extent
        }),
        (base(), axis_value(), turn()).prop_map(move |(base, axis, extent)| Operation::Revolve {
            base,
            sketch: s3.clone(),
            axis,
            extent
        }),
        (axis_value(), turn()).prop_map(move |(axis, extent)| Operation::Groove {
            base: id("k7f2qx3m"),
            sketch: s4.clone(),
            axis,
            extent
        }),
    ]
}

proptest! {
    #![proptest_config(config(3000))]

    /// `read(write(d)) == d` for every sweep the types hold, valid to the gate or not, and
    /// writing again gives the same bytes.
    #[test]
    fn every_constructible_sweep_writes_and_reads_back_as_itself(operation in sweep_operation()) {
        let document = Document {
            silico: FormatVersion,
            units: Units,
            naming: NamingScheme,
            features: vec![Feature {
                id: id("x0000001"),
                operation,
                version: FeatureVersion,
                name: Name::new("X").unwrap(),
                note: None,
                fingerprint: None,
            }],
        };
        let text = write(&document).unwrap();
        let back = read(text.as_bytes());
        prop_assert!(back.is_ok(), "{}: {:?}", text, back);
        let back = back.unwrap();
        prop_assert_eq!(&back, &document);
        prop_assert_eq!(write(&back).unwrap(), text);
    }
}

// ---------------------------------------------------------------------------------------
// The gate: references at odd places.

#[test]
fn an_axis_naming_a_constraint_of_the_sketch_is_refused_as_a_constraint() {
    let error = problem(&[
        PROFILE,
        &turned("r0000001", "revolve", None, "s0000003", "ka000000", TURN),
    ]);
    assert_refused(
        &error,
        "r0000001",
        "axis",
        "axis ka000000 is a constraint of sketch s0000003: a revolve turns about a line",
        "name a line of the sketch, x_axis, or y_axis",
    );
}

#[test]
fn an_axis_naming_a_line_of_another_sketch_is_refused() {
    let other = empty_sketch("s0000001", "Other", "xy");
    let error = problem(&[
        PROFILE,
        &other,
        &turned("r0000001", "revolve", None, "s0000001", "na000000", TURN),
    ]);
    assert_refused(
        &error,
        "r0000001",
        "axis",
        "axis na000000 isn't an element of sketch s0000001: a revolve turns about a line of its sketch, x_axis, or y_axis",
        "name a line of the sketch, x_axis, or y_axis",
    );
}

#[test]
fn a_pad_whose_base_is_its_own_sketch_is_refused_once_on_base() {
    let error = problem(&[PROFILE, &pad("p0000001", Some("s0000003"), "s0000003", TEN)]);
    assert_eq!(error.parameter(), Some("base"), "{error:?}");
    assert_eq!(
        error.message(),
        "base s0000003 is s0000003 (\"Profile\"), a sketch, which makes no solid for a pad to use"
    );
}

#[test]
fn a_pocket_that_names_itself_as_base_and_sketch_is_refused_on_each() {
    let invalid = checked(&[pocket("q0000001", "q0000001", "q0000001", TEN).as_str()]).unwrap_err();
    let parameters: Vec<_> = invalid.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(
        parameters,
        [Some("base"), Some("sketch")],
        "{:#?}",
        invalid.errors()
    );
    for error in invalid.errors() {
        assert!(
            error
                .message()
                .ends_with("is this feature: a feature can't use itself"),
            "{error:?}"
        );
    }
}

#[test]
fn a_pad_used_by_a_pocket_and_a_fuse_is_consumed_once() {
    let error = problem(&[
        PROFILE,
        &pad("p0000001", None, "s0000003", TEN),
        &pocket("q0000001", "p0000001", "s0000003", TEN),
        &block("a91cw0de"),
        r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "F", "base": "p0000001", "tool": "a91cw0de"}"#,
    ]);
    assert_eq!(error.feature(), Some("f0000001"));
    assert_eq!(error.parameter(), Some("base"));
    assert!(
        error
            .message()
            .contains("solid was already consumed by q0000001"),
        "{error:?}"
    );
}

#[test]
fn a_revolve_on_a_sketch_whose_id_a_box_has_first_is_refused_on_sketch_and_id() {
    // Two features share s0000003: a box first, then the sketch. The revolve's sketch is
    // the box (the ID's first feature): refused as not a sketch, and the axis isn't
    // checked against the second.
    let shared_box = r#"{"id": "s0000003", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#;
    let invalid = checked(&[
        shared_box,
        PROFILE,
        &turned("r0000001", "revolve", None, "s0000003", "na000000", TURN),
    ])
    .unwrap_err();
    let parameters: Vec<_> = invalid
        .errors()
        .iter()
        .map(|e| (e.feature(), e.parameter()))
        .collect();
    assert_eq!(
        parameters,
        [
            (Some("features[1]"), Some("id")),
            (Some("r0000001"), Some("sketch")),
        ],
        "{:#?}",
        invalid.errors()
    );
}

/// An 8-character ID from `n`.
fn nth_id(prefix: char, n: usize) -> String {
    let alphabet: Vec<char> = ID_ALPHABET.chars().collect();
    let mut out = vec![prefix];
    let mut n = n;
    for _ in 0..7 {
        out.push(alphabet[n % 32]);
        n /= 32;
    }
    out.into_iter().collect()
}

/// The document `the_gate_on_many_revolves_*` checks: a sketch of `points` points, and
/// 20,000 revolves about `axis`: under the reader's 16 MB written canonically.
fn many_revolves(points: usize, axis: &str) -> (Document, usize) {
    let points: Vec<String> = (0..points)
        .map(|n| {
            format!(
                r#"{{"id": "{}", "x": {n}, "y": 0, "construction": false}}"#,
                nth_id('p', n)
            )
        })
        .collect();
    let sketch = format!(
        r#"{{"id": "s0000003", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", ")
    );
    let n = 20_000;
    let mut features = vec![sketch];
    for i in 0..n {
        features.push(turned(
            &nth_id('r', i),
            "revolve",
            None,
            "s0000003",
            axis,
            TURN,
        ));
    }
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    (read(document(&refs).as_bytes()).unwrap(), n)
}

fn gate_time(document: &Document) -> (Duration, u64) {
    let start = Instant::now();
    let total = validate(document).map_or_else(|invalid| invalid.total(), |()| 0);
    (start.elapsed(), total)
}

// Found by break-it: the gate found each revolve's axis by scanning its sketch's elements
// from the start, so its time was revolves × elements: 20,000 revolves on one 10,000-point
// sketch, each naming an axis the sketch hasn't, took over a hundred times as long to check
// as the same revolves about x_axis (measured: 50,000 of them, 1.17 s against 11 ms in a
// release build, 11 s against 69 ms in debug); the largest document the reader takes holds
// about 85,000. Each sketch's elements are now sorted once and searched.
#[test]
fn the_gate_on_many_revolves_about_a_missing_line_takes_about_as_long_as_about_x_axis() {
    let (about_x, n) = many_revolves(10_000, "x_axis");
    let (about_missing, m) = many_revolves(10_000, "zzzzzzzz");
    assert_eq!(n, m);
    let (x_took, x_total) = gate_time(&about_x);
    let (missing_took, missing_total) = gate_time(&about_missing);
    // The sketch is refused for its size either way; each revolve about a missing line is
    // one problem more.
    assert!(x_total > 0);
    assert_eq!(missing_total, x_total + n as u64);
    assert!(
        missing_took < x_took * 4 + Duration::from_millis(500),
        "{n} revolves on a 10,000-point sketch: {x_took:?} about x_axis, {missing_took:?} about a missing line"
    );
}

// ---------------------------------------------------------------------------------------
// check_axis_length: solutions at the edges.

fn solution_with(points: &[(&str, [f64; 2])]) -> SketchSolution {
    let mut map = BTreeMap::new();
    for (point, [x, y]) in points {
        map.insert(
            ElementId::new(point).unwrap(),
            [Coordinate::new(*x).unwrap(), Coordinate::new(*y).unwrap()],
        );
    }
    SketchSolution {
        points: map,
        radii: BTreeMap::new(),
        references: BTreeMap::new(),
        dofs: 0,
        free: BTreeSet::new(),
        redundant: BTreeMap::new(),
        partially_redundant: BTreeMap::new(),
        algorithm: silico_protocol::Algorithm::DogLeg,
    }
}

fn revolve_on_profile(axis: &str) -> Checked {
    checked_document(&[
        PROFILE,
        &turned("r0000001", "revolve", None, "s0000003", axis, TURN),
    ])
}

#[test]
fn an_axis_on_the_profiles_own_line_is_checked_by_its_ends() {
    // nb000000 runs pc to pd. A solve that brings pd onto pc (a far place) is refused;
    // one that leaves them apart passes.
    let document = revolve_on_profile("nb000000");
    let far = 1e6;
    let solution = solution_with(&[
        ("pa000000", [0.0, 0.0]),
        ("pb000000", [0.0, 10.0]),
        ("pc000000", [far, -far]),
        ("pd000000", [far, -far]),
        ("pe000000", [5.0, 6.0]),
    ]);
    let error = check_axis_length(&document, &id("r0000001"), &solution).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.feature(), Some("r0000001"));
    assert_eq!(error.parameter(), Some("axis"));
    assert_eq!(
        error.message(),
        "axis nb000000, a line of sketch s0000003, has its ends at one point: it gives no direction to turn about"
    );
    assert_eq!(
        error.suggestion(),
        Some("give the line some length, or turn about another line")
    );
}

#[test]
fn checking_an_axis_twice_gives_the_same_answer_and_leaves_the_solution_alone() {
    let document = revolve_on_profile("na000000");
    let solution = solution_with(&[
        ("pa000000", [1.0, 1.0]),
        ("pb000000", [1.0, 1.0]),
        ("pc000000", [5.0, 5.0]),
        ("pd000000", [6.0, 5.0]),
        ("pe000000", [5.0, 6.0]),
    ]);
    let before = solution.clone();
    let first = check_axis_length(&document, &id("r0000001"), &solution);
    let second = check_axis_length(&document, &id("r0000001"), &solution);
    assert!(first.is_err());
    assert_eq!(first, second);
    assert_eq!(solution, before);
}

// ---------------------------------------------------------------------------------------
// Recompute and format: what a sweep it can't build leaves behind.

fn published(engine: &Recompute<FakeKernel>, document: Checked) -> std::sync::Arc<Model> {
    match engine.start(document).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

#[test]
fn a_pocket_on_a_pad_is_blocked_on_the_pad() {
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let model = published(
        &engine,
        checked_document(&[
            &empty_sketch("s0000001", "S", "xy"),
            &pad("p0000001", None, "s0000001", TEN),
            &pocket("q0000001", "p0000001", "s0000001", TEN),
        ]),
    );
    // An empty sketch makes no profile (M3.5): the pad fails with one error naming it, the
    // sketch's refusal its cause (design S3), and the pocket on it is blocked on the pad.
    let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("p0000001")) else {
        panic!("{:?}", model.outcome(&id("p0000001")));
    };
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.feature(), Some("p0000001"));
    assert_eq!(
        error.cause(),
        Some(
            "the sketch has no curves to make a profile of (draw a closed outline of lines, arcs, or circles that aren't construction geometry)"
        )
    );
    assert_eq!((error.elements(), error.elements_total()), (&[][..], 0));
    assert!(model.errors().all(|e| e.feature() != Some("s0000001")));
    assert_eq!(
        model.outcome(&id("q0000001")),
        Some(&FeatureOutcome::Blocked { on: id("p0000001") })
    );
}

const FINGERPRINT: &str = r#""fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}"#;

#[test]
fn format_keeps_a_sweeps_stored_fingerprint_and_says_why() {
    let pad_text = format!(
        r#"{{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {TEN}, {FINGERPRINT}}}"#
    );
    let sketch = empty_sketch("s0000001", "S", "xy");
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let model = published(&engine, checked_document(&[&sketch, &pad_text]));
    let formatted = command::format(&model).unwrap();
    // The stored fingerprint is kept, byte for byte, and nothing was refingerprinted.
    assert!(
        formatted
            .text
            .contains(r#""fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}"#),
        "{}",
        formatted.text
    );
    assert!(formatted.refingerprinted.is_empty(), "{formatted:?}");
    // The pad's refusal is among the problems: its empty sketch makes no profile, which
    // names the pad (design S3).
    let errors = formatted.problems.errors();
    assert!(
        errors
            .iter()
            .any(|e| e.feature() == Some("p0000001") && e.code() == ErrorCode::InvalidInput),
        "{errors:#?}"
    );
    // And what's written reads back as the document it was.
    assert_eq!(read(formatted.text.as_bytes()).unwrap(), *model.document());
    // The engine still builds a valid document after it.
    let model = published(&engine, checked_document(&[&block("k7f2qx3m")]));
    assert!(matches!(
        model.outcome(&id("k7f2qx3m")),
        Some(FeatureOutcome::Built { .. })
    ));
}
