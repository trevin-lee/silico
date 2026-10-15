//! M2.2: the validation gate. Each kind of invalid document is refused with an error naming
//! the feature and the field, every problem at once. Expected messages are written out by
//! hand, and the value ranges come from the kernel's limits as numbers: a length above
//! zero and at most a kilometre (10^12 nm), a position at most a kilometre either way.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;

use example::EXAMPLE;
use proptest::prelude::*;
use silico_core::document::{Invalid, PROBLEMS_LISTED, read, validate};
use silico_core::{Error, ErrorCode};

/// A document of these features, each a JSON object's text.
fn document(features: &[&str]) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}]}}",
        features.join(", ")
    )
}

fn checked(features: &[&str]) -> Result<(), Invalid> {
    validate(&read(document(features).as_bytes()).unwrap())
}

fn problems(features: &[&str]) -> Invalid {
    checked(features).unwrap_err()
}

/// The one problem these features have.
fn problem(features: &[&str]) -> Error {
    let invalid = problems(features);
    assert_eq!(invalid.total(), 1, "{:#?}", invalid.errors());
    invalid.errors()[0].clone()
}

#[track_caller]
fn assert_refused(error: &Error, feature: &str, parameter: &str, message: &str, suggestion: &str) {
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.feature(), Some(feature), "{error:?}");
    assert_eq!(error.parameter(), Some(parameter), "{error:?}");
    assert_eq!(error.message(), message);
    assert_eq!(error.suggestion(), Some(suggestion));
    assert!(error.elements().is_empty());
}

fn block(id: &str, name: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "{name}", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#
    )
}

fn boolean(id: &str, type_name: &str, name: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "{type_name}", "v": 1, "name": "{name}", "base": "{base}", "tool": "{tool}"}}"#
    )
}

#[test]
fn valid_documents_pass() {
    validate(&read(EXAMPLE.as_bytes()).unwrap()).unwrap();
    checked(&[]).unwrap();
    // Every primitive, each at its limits, and booleans of booleans.
    checked(&[
        r#"{"id": "00000000", "type": "box", "v": 1, "name": "B", "position": [1000000, -1000000, 0], "length": 1000000, "width": 0.000001, "height": 1}"#,
        r#"{"id": "11111111", "type": "cylinder", "v": 1, "name": "C", "position": [0, 0, 0], "radius": 0.000001, "height": 1000000}"#,
        r#"{"id": "22222222", "type": "sphere", "v": 1, "name": "S", "position": [0, 0, 0], "radius": 1000000}"#,
        r#"{"id": "33333333", "type": "cone", "v": 1, "name": "K", "position": [0, 0, 0], "radius1": 0, "radius2": 1000000, "height": 1}"#,
        r#"{"id": "44444444", "type": "cone", "v": 1, "name": "K", "position": [0, 0, 0], "radius1": 5, "radius2": 0, "height": 1}"#,
        r#"{"id": "55555555", "type": "torus", "v": 1, "name": "T", "position": [0, 0, 0], "radius1": 10, "radius2": 10}"#,
        &boolean("66666666", "fuse", "F", "00000000", "11111111"),
        &boolean("77777777", "cut", "U", "66666666", "22222222"),
        &boolean("88888888", "common", "M", "33333333", "77777777"),
        &boolean("99999999", "fuse", "G", "88888888", "44444444"),
    ])
    .unwrap();
}

#[test]
fn an_id_used_twice_is_refused_at_its_second_feature_named_by_place() {
    let error = problem(&[&block("aaaaaaaa", "First"), &block("aaaaaaaa", "Second")]);
    assert_refused(
        &error,
        "features[1]",
        "id",
        "id aaaaaaaa is already the ID of features[0] (\"First\")",
        "give this feature an ID of its own",
    );
}

#[test]
fn a_reference_to_no_feature_is_refused() {
    let error = problem(&[
        &block("aaaaaaaa", "A"),
        &boolean("cccccccc", "cut", "Hole", "aaaaaaaa", "zzzzzzzz"),
    ]);
    assert_refused(
        &error,
        "cccccccc",
        "tool",
        "tool zzzzzzzz isn't the ID of any feature",
        "use the ID of a feature before this one",
    );
}

#[test]
fn a_missing_id_sorting_between_present_ones_is_refused_not_matched_to_a_neighbour() {
    let error = problem(&[
        &block("aaaaaaaa", "A"),
        &block("cccccccc", "C"),
        &boolean("dddddddd", "cut", "Hole", "aaaaaaaa", "bbbbbbbb"),
    ]);
    assert_refused(
        &error,
        "dddddddd",
        "tool",
        "tool bbbbbbbb isn't the ID of any feature",
        "use the ID of a feature before this one",
    );
}

#[test]
fn ids_differing_in_one_character_anywhere_are_different_features() {
    for at in 0..8 {
        let mut other = *b"aaaaaaaa";
        other[at] = b'b';
        let other = std::str::from_utf8(&other).unwrap();
        assert_eq!(
            checked(&[
                &block("aaaaaaaa", "A"),
                &block(other, "B"),
                &boolean("zzzzzzzz", "fuse", "F", "aaaaaaaa", other),
            ]),
            Ok(()),
            "{other}"
        );
    }
}

#[test]
fn a_feature_using_itself_is_refused() {
    let error = problem(&[
        &block("aaaaaaaa", "A"),
        &boolean("cccccccc", "fuse", "Loop", "cccccccc", "aaaaaaaa"),
    ]);
    assert_refused(
        &error,
        "cccccccc",
        "base",
        "base cccccccc is this feature: a feature can't use itself",
        "use the ID of a feature before this one",
    );
}

#[test]
fn a_reference_forward_is_refused_so_no_cycle_can_be_written() {
    // Two booleans using each other: the first's reference is forward.
    let invalid = problems(&[
        &block("aaaaaaaa", "A"),
        &block("bbbbbbbb", "B"),
        &boolean("cccccccc", "fuse", "One", "aaaaaaaa", "dddddddd"),
        &boolean("dddddddd", "fuse", "Two", "bbbbbbbb", "cccccccc"),
    ]);
    assert_eq!(invalid.total(), 1, "{:#?}", invalid.errors());
    assert_refused(
        &invalid.errors()[0],
        "cccccccc",
        "tool",
        "tool dddddddd is dddddddd (\"Two\"), which comes after this feature: a feature may use only features before it",
        "move this feature after the one it uses, or use one before it",
    );
}

#[test]
fn a_solid_consumed_twice_is_refused_naming_its_consumer() {
    let error = problem(&[
        &block("aaaaaaaa", "Block"),
        &block("bbbbbbbb", "Pin"),
        &block("cccccccc", "Peg"),
        &boolean("dddddddd", "cut", "Hole", "aaaaaaaa", "bbbbbbbb"),
        &boolean("eeeeeeee", "cut", "Slot", "aaaaaaaa", "cccccccc"),
    ]);
    assert_refused(
        &error,
        "eeeeeeee",
        "base",
        "base aaaaaaaa's solid was already consumed by dddddddd (\"Hole\"): a solid can be used once",
        "use dddddddd's result, or another feature's",
    );
}

#[test]
fn a_consumer_sharing_its_id_is_named_by_place_and_not_offered_by_id() {
    // Its ID names the box before it, so "use its result" by ID would take the box.
    let invalid = problems(&[
        &block("aaaaaaaa", "Block"),
        &block("dddddddd", "Spacer"),
        &block("bbbbbbbb", "Pin"),
        &boolean("dddddddd", "fuse", "Join", "aaaaaaaa", "bbbbbbbb"),
        &block("cccccccc", "Peg"),
        &boolean("eeeeeeee", "cut", "Slot", "aaaaaaaa", "cccccccc"),
    ]);
    assert_eq!(invalid.total(), 2, "{:#?}", invalid.errors());
    assert_refused(
        &invalid.errors()[0],
        "features[3]",
        "id",
        "id dddddddd is already the ID of features[1] (\"Spacer\")",
        "give this feature an ID of its own",
    );
    assert_refused(
        &invalid.errors()[1],
        "eeeeeeee",
        "base",
        "base aaaaaaaa's solid was already consumed by features[3] (\"Join\"): a solid can be used once",
        "give features[3] an ID of its own and use its result, or use another feature",
    );
}

#[test]
fn a_consumer_first_with_its_id_is_offered_by_that_id() {
    // A later box shares the consumer's ID, but the ID names the consumer: the box is the
    // one to rename, as its own error says, so the advice for the consumer agrees.
    let invalid = problems(&[
        &block("aaaaaaaa", "Block"),
        &block("bbbbbbbb", "Pin"),
        &boolean("dddddddd", "fuse", "Join", "aaaaaaaa", "bbbbbbbb"),
        &block("dddddddd", "Spacer"),
        &block("cccccccc", "Peg"),
        &boolean("eeeeeeee", "cut", "Slot", "aaaaaaaa", "cccccccc"),
    ]);
    assert_eq!(invalid.total(), 2, "{:#?}", invalid.errors());
    assert_refused(
        &invalid.errors()[0],
        "features[3]",
        "id",
        "id dddddddd is already the ID of features[2] (\"Join\")",
        "give this feature an ID of its own",
    );
    assert_refused(
        &invalid.errors()[1],
        "eeeeeeee",
        "base",
        "base aaaaaaaa's solid was already consumed by features[2] (\"Join\"): a solid can be used once",
        "use dddddddd's result, or another feature's",
    );
}

#[test]
fn a_boolean_of_a_solid_with_itself_is_refused() {
    let error = problem(&[
        &block("aaaaaaaa", "A"),
        &boolean("cccccccc", "common", "Same", "aaaaaaaa", "aaaaaaaa"),
    ]);
    assert_refused(
        &error,
        "cccccccc",
        "tool",
        "tool aaaaaaaa is base too: a common takes two different solids",
        "give the common a different tool",
    );
}

#[test]
fn a_value_the_kernel_refuses_is_refused_in_the_documents_words() {
    let cases: [(&str, &str, &str, &str); 7] = [
        (
            r#""type": "box", "v": 1, "name": "N", "position": [0, 0, 0], "length": 0, "width": 1, "height": 1"#,
            "length",
            "box length must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got 0",
            "give the box a valid length",
        ),
        (
            r#""type": "box", "v": 1, "name": "N", "position": [0, 0, 0], "length": 1, "width": 1000000.000001, "height": 1"#,
            "width",
            "box width must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got 1000000.000001",
            "give the box a valid width",
        ),
        (
            r#""type": "cylinder", "v": 1, "name": "N", "position": [0, 0, 0], "radius": 1, "height": -5"#,
            "height",
            "cylinder height must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got -5",
            "give the cylinder a valid height",
        ),
        (
            r#""type": "sphere", "v": 1, "name": "N", "position": [0, 0, 0], "radius": -0.5"#,
            "radius",
            "sphere radius must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got -0.5",
            "give the sphere a valid radius",
        ),
        (
            r#""type": "cone", "v": 1, "name": "N", "position": [0, 0, 0], "radius1": 2, "radius2": -1, "height": 1"#,
            "radius2",
            "cone radius2 must be zero, or a length above 1e-7 mm and at most 1e6 mm (a kilometre), got -1",
            "give the cone a valid radius2",
        ),
        (
            r#""type": "cone", "v": 1, "name": "N", "position": [0, 0, 0], "radius1": 0, "radius2": 0, "height": 1"#,
            "radius1",
            "cone radius1 must be above zero where radius2 is zero: a cone with no radius has no volume",
            "give the cone a valid radius1",
        ),
        (
            r#""type": "torus", "v": 1, "name": "N", "position": [0, 0, 0], "radius1": 10, "radius2": 10.000001"#,
            "radius2",
            "torus radius2 must be at most radius1 (10), got 10.000001: the tube would cross the axis",
            "give the torus a valid radius2",
        ),
    ];
    for (fields, parameter, message, suggestion) in cases {
        let error = problem(&[&format!(r#"{{"id": "aaaaaaaa", {fields}}}"#)]);
        assert_refused(&error, "aaaaaaaa", parameter, message, suggestion);
    }
}

#[test]
fn a_position_past_a_kilometre_is_refused_for_each_axis_past_it() {
    let invalid = problems(&[
        r#"{"id": "aaaaaaaa", "type": "sphere", "v": 1, "name": "Far", "position": [1000000.000001, 1000000, -1000000.000001], "radius": 1}"#,
    ]);
    assert_eq!(invalid.total(), 2, "{:#?}", invalid.errors());
    for (error, axis, value) in [
        (&invalid.errors()[0], 0, "1000000.000001"),
        (&invalid.errors()[1], 2, "-1000000.000001"),
    ] {
        assert_refused(
            error,
            "aaaaaaaa",
            &format!("position.{axis}"),
            &format!(
                "position.{axis} is {value} mm, farther from the origin than the 1000000 mm (a kilometre) a sphere may be placed"
            ),
            "place the sphere within a kilometre of the origin",
        );
    }
}

#[test]
fn every_problem_is_reported_at_once_in_timeline_order() {
    let invalid = problems(&[
        &boolean("cccccccc", "cut", "Early", "aaaaaaaa", "qqqqqqqq"),
        r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [0, 0, 2000000], "length": 0, "width": 1, "height": 1}"#,
        &block("aaaaaaaa", "Again"),
        &block("bbbbbbbb", "B"),
        &boolean("dddddddd", "fuse", "Late", "bbbbbbbb", "bbbbbbbb"),
    ]);
    let found: Vec<_> = invalid
        .errors()
        .iter()
        .map(|e| (e.feature().unwrap(), e.parameter().unwrap()))
        .collect();
    assert_eq!(
        found,
        [
            // A reference forward to an ID two features share: named by place.
            ("cccccccc", "base"),
            ("cccccccc", "tool"),
            // Its ID shared, the first feature with it is named by place too.
            ("features[1]", "position.2"),
            ("features[1]", "length"),
            ("features[2]", "id"),
            ("dddddddd", "tool"),
        ]
    );
    assert_eq!(invalid.total(), 6);
    assert_eq!(
        invalid.errors()[0].message(),
        "base aaaaaaaa is features[1] (\"A\"), which comes after this feature: a feature may use only features before it"
    );
}

#[test]
fn past_a_hundred_problems_the_rest_are_counted() {
    assert_eq!(PROBLEMS_LISTED, 100);
    let mut features = vec![block("aaaaaaaa", "A"), block("bbbbbbbb", "B")];
    // 150 booleans, each referring to an ID no feature has.
    let ids: Vec<String> = (0..150).map(|n| format!("c{n:07}")).collect();
    for id in &ids {
        features.push(boolean(id, "fuse", "F", "zzzzzzzz", "aaaaaaaa"));
    }
    let features: Vec<&str> = features.iter().map(String::as_str).collect();
    let invalid = problems(&features);
    // Each refers to the missing ID; the first also consumes aaaaaaaa, so each later one is
    // refused for that too.
    assert_eq!(invalid.total(), 150 + 149);
    assert_eq!(invalid.errors().len(), 100);
    assert_eq!(invalid.errors()[0].feature(), Some("c0000000"));
    assert_eq!(invalid.errors()[0].parameter(), Some("base"));
    assert_eq!(invalid.errors()[99].feature(), Some("c0000050"));
    assert_eq!(invalid.errors()[99].parameter(), Some("base"));
}

/// A document of one feature with these fields, valid or not.
fn one(fields: String) -> Result<(), Invalid> {
    checked(&[&format!(
        r#"{{"id": "aaaaaaaa", "name": "N", "v": 1, {fields}}}"#
    )])
}

/// Written as a length: nanometres as millimetres to 6 decimals.
fn written(nm: i64) -> String {
    let sign = if nm < 0 { "-" } else { "" };
    let nm = nm.unsigned_abs();
    format!("{sign}{}.{:06}", nm / 1_000_000, nm % 1_000_000)
}

const KM: i64 = 1_000_000_000_000;

fn near_limits() -> impl Strategy<Value = i64> {
    prop_oneof![-3..3i64, KM - 3..KM + 3, -KM - 3..-KM + 3, -2 * KM..2 * KM,]
}

proptest! {
    #[test]
    fn a_boxs_lengths_pass_exactly_when_above_zero_and_at_most_a_kilometre(
        dims in [near_limits(), near_limits(), near_limits()],
    ) {
        let [l, w, h] = dims.map(written);
        let valid = dims.iter().all(|&nm| nm > 0 && nm <= KM);
        let result = one(format!(
            r#""type": "box", "position": [0, 0, 0], "length": {l}, "width": {w}, "height": {h}"#
        ));
        prop_assert_eq!(result.is_ok(), valid, "{:?}", result);
    }

    #[test]
    fn a_position_passes_exactly_when_within_a_kilometre_either_way(
        position in [near_limits(), near_limits(), near_limits()],
    ) {
        let [x, y, z] = position.map(written);
        let valid = position.iter().all(|&nm| nm.abs() <= KM);
        let result = one(format!(
            r#""type": "sphere", "position": [{x}, {y}, {z}], "radius": 1"#
        ));
        prop_assert_eq!(result.is_ok(), valid, "{:?}", result);
    }

    #[test]
    fn a_cones_radii_pass_exactly_when_each_is_zero_or_a_length_and_not_both_zero(
        radius1 in near_limits(), radius2 in near_limits(),
    ) {
        let in_range = |nm: i64| nm == 0 || (nm > 0 && nm <= KM);
        let valid = in_range(radius1) && in_range(radius2) && (radius1, radius2) != (0, 0);
        let result = one(format!(
            r#""type": "cone", "position": [0, 0, 0], "radius1": {}, "radius2": {}, "height": 1"#,
            written(radius1), written(radius2)
        ));
        prop_assert_eq!(result.is_ok(), valid, "{:?}", result);
    }

    #[test]
    fn a_torus_passes_exactly_when_its_tube_is_no_wider_than_its_ring(
        radius1 in prop_oneof![1..20i64, KM - 3..=KM],
        tube in -3..3i64,
    ) {
        // The tube's radius within a few nanometres of the ring's.
        let radius2 = radius1 + tube;
        let valid = radius2 > 0 && radius2 <= radius1;
        let result = one(format!(
            r#""type": "torus", "position": [0, 0, 0], "radius1": {}, "radius2": {}"#,
            written(radius1), written(radius2)
        ));
        prop_assert_eq!(result.is_ok(), valid, "{:?}", result);
    }
}

#[test]
fn the_checked_type_refuses_what_the_gate_refuses_and_holds_what_it_passes() {
    let text = document(&[&boolean("cccccccc", "cut", "C", "aaaaaaaa", "bbbbbbbb")]);
    let refused = read(text.as_bytes()).unwrap();
    let invalid = silico_core::document::Checked::new(refused.clone()).unwrap_err();
    assert_eq!(invalid, validate(&refused).unwrap_err());
    assert_eq!(invalid.total(), 2);
    let passed = read(EXAMPLE.as_bytes()).unwrap();
    let checked = silico_core::document::Checked::new(passed.clone()).unwrap();
    assert_eq!(checked.document(), &passed);
    assert_eq!(checked.into_document(), passed);
}
