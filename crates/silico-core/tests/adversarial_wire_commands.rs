//! break-it for M4.4's wire types (`command::wire::elements`, `command::wire::schema`, and
//! the readers they're built on): requests wrong in every field at once, floods of
//! refusals, words that differ from the command's by case, whitespace, look-alikes, and
//! invisible or control characters, numbers at and past each quantity's places and range,
//! directions of tiny, huge, and subnormal components, `schema` names that aren't names,
//! and results of unusual but valid values written and held to the committed snapshots.
//! Expected values come from the design (docs/design/m4.1-command-api.md, sections 2 and 7,
//! and m4.4-describe.md's *On the wire*), the quantities' integer types (an area is an
//! `i128` of millionths, a length an `i64` of nanometres, an angle an `i64` of nanodegrees),
//! IEEE 754, and the snapshots themselves.

use super::listing::{BOX, model_of};

use std::path::PathBuf;

use silico_core::ErrorCode;
use silico_core::command::elements::{
    Alias, AxisToken, Cursor, ElementType, EntryAxis, Facing, Listed, Quantity, Queried, Query,
    Tied, list, query,
};
use silico_core::command::schema::schema;
use silico_core::command::wire::elements::{ElementsRequest, QueryRequest};
use silico_core::command::wire::schema::SchemaRequest;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{Problems, Revision};
use silico_core::document::json::{Node, read};
use silico_core::document::{Angle, ElementName, Length, Measure};
use silico_core::kernel::ElementKind;

fn parsed(text: &str) -> Node {
    read(text.as_bytes()).unwrap()
}

fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap()
}

/// Each refusal's parameter (empty for none) and message, in order.
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

fn params(problems: &Problems) -> Vec<String> {
    said(problems).into_iter().map(|(p, _)| p).collect()
}

/// A query request with nothing wrong; each test replaces a field of it.
const QUERY: &str = r#"{"session": "s1", "model": "current", "feature": null, "kind": "face", "type": null, "stable": null, "facing": null, "measures": [], "order": null, "limit": 10}"#;

fn query_with(from: &str, to: &str) -> String {
    assert!(QUERY.contains(from), "{from}");
    QUERY.replace(from, to)
}

fn query_refused(text: &str) -> Problems {
    QueryRequest::read(&parsed(text)).unwrap_err()
}

fn with_measures(ranges: &str) -> String {
    query_with(r#""measures": []"#, &format!(r#""measures": [{ranges}]"#))
}

/// A character a message must never show raw: a control, a line or paragraph separator,
/// any space but the plain one, or one Unicode draws as nothing (General_Category=Cf: the
/// soft hyphen, zero-width characters, bidi controls and isolates, the BOM, tags).
fn unseen(c: char) -> bool {
    c.is_control()
        || (c.is_whitespace() && c != ' ')
        || matches!(c,
            '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}'
            | '\u{e0000}'..='\u{e007f}')
}

fn printable(text: &str) -> bool {
    !text.chars().any(unseen)
}

/// The snapshot `of`'s `part`, or its definition `def` in it, as committed.
fn snapshot_part(of: &str, part: &str, def: Option<&str>) -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract/commands")
        .join(format!("{of}.json"));
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let root = &schema[part];
    match def {
        None => root.clone(),
        Some(def) => serde_json::json!({
            "$defs": root["$defs"],
            "$ref": format!("#/$defs/{def}"),
        }),
    }
}

/// Panics, naming `what`, unless `text` holds to `schema`.
fn holds(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}\n{text}");
}

// --- Every field wrong at once ----------------------------------------------------------

#[test]
fn a_query_wrong_in_every_field_says_each_reason_in_the_shapes_order() {
    // Every field of the request wrong, each nested object's too, and a range of each kind
    // of wrong: one reason each, none hidden behind another, in the order the shape gives.
    let text = r#"{"session": "t1", "model": "latest", "feature": 7, "kind": "solid", "type": "nurbs", "stable": "yes",
        "facing": {"direction": [0, 0, "z"], "within": -0, "x": 1},
        "measures": [
            {"measure": "volume", "at_least": "big", "at_most": [1]},
            {"measure": "area", "at_least": 1.0000001, "at_most": -0},
            {"measure": "half_angle", "at_least": 1.0000000001, "at_most": 9223372036.854775808},
            {"measure": "radius", "at_least": "1", "at_most": null, "y": 0},
            5
        ],
        "order": {"by": 1, "descending": null},
        "limit": -1, "extra": 0}"#;
    let refused = query_refused(text);
    assert_eq!(
        params(&refused),
        [
            "session",
            "model",
            "feature",
            "kind",
            "type",
            "stable",
            "facing.direction.2",
            "facing.within",
            "facing.x",
            "measures.0.measure",
            "measures.1.at_least",
            "measures.1.at_most",
            "measures.2.at_least",
            "measures.2.at_most",
            "measures.3.at_least",
            "measures.3.y",
            "measures.4",
            "order.by",
            "order.descending",
            "limit",
            "extra",
        ]
    );
    assert_eq!(refused.total(), 21);
    assert!(
        refused
            .errors()
            .iter()
            .all(|e| e.code() == ErrorCode::InvalidInput)
    );
    let messages: Vec<String> = said(&refused).into_iter().map(|(_, m)| m).collect();
    assert_eq!(
        messages[7],
        "facing.within must be an angle in degrees, got -0, which is zero written with a minus \
         sign"
    );
    assert_eq!(
        refused.errors()[7].suggestion(),
        Some("write 0"),
        "an angle's -0"
    );
    assert_eq!(
        messages[11],
        "measures.1.at_most must be an area in square millimetres or null, got -0, which is \
         zero written with a minus sign"
    );
    assert_eq!(
        messages[13],
        "measures.2.at_most must be an angle in degrees or null, got 9223372036.854775808, \
         which is too large"
    );
    assert_eq!(
        refused.errors()[13].suggestion(),
        Some("an angle is at most 9223372036.854775807 degrees either way")
    );
    assert_eq!(
        messages[14],
        r#"measures.3.at_least must be a length in millimetres or null, got "1""#
    );
    assert_eq!(messages[16], "measures.4 must be an object, got a number");
    assert_eq!(
        messages[18],
        "order.descending must be true or false, got null"
    );
}

#[test]
fn too_many_ranges_among_other_wrong_fields_is_one_reason_in_its_place() {
    // Eight ranges among other faults: the ranges are one reason, between the fields read
    // before and after them, and none of their members is read.
    let bad = [r#"{"measure": 1, "at_least": "x", "at_most": [], "extra": 0}"#; 8].join(", ");
    let text = with_measures(&bad)
        .replace(r#""kind": "face""#, r#""kind": "Face""#)
        .replace(r#""limit": 10"#, r#""limit": 1.5"#);
    let refused = query_refused(&text);
    assert_eq!(params(&refused), ["kind", "measures", "limit"]);
    assert_eq!(
        said(&refused)[1].1,
        "measures holds at most 7 ranges, as many as there are measures, got 8"
    );
    // Seven, each wrong in every way: every reason of each, by its place.
    let seven = [r#"{"measure": 1, "at_least": "x", "at_most": [], "extra": 0}"#; 7].join(", ");
    let refused = query_refused(&with_measures(&seven));
    assert_eq!(refused.total(), 7 * 2);
    assert_eq!(
        params(&refused)[..2],
        ["measures.0.measure", "measures.0.extra"]
    );
    assert_eq!(params(&refused)[13], "measures.6.extra");
}

#[test]
fn a_flood_of_refusals_in_nested_objects_lists_the_first_hundred_and_counts_them_all() {
    // 20 unknown keys in `facing`, 20 in each of seven ranges, 50 at the top: 210 reasons,
    // the first hundred listed in the order found (facing's, then the ranges' in turn).
    let keys = |n: usize| -> String {
        (0..n)
            .map(|i| format!(r#""k{i}": 0"#))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let range = format!(
        r#"{{"measure": "area", "at_least": null, "at_most": null, {}}}"#,
        keys(20)
    );
    let text = with_measures(&vec![range; 7].join(", "))
        .replace(
            r#""facing": null"#,
            &format!(
                r#""facing": {{"direction": [0, 0, 1], "within": 1, {}}}"#,
                keys(20)
            ),
        )
        .replace(r#""limit": 10"#, &format!(r#""limit": 10, {}"#, keys(50)));
    let refused = query_refused(&text);
    assert_eq!(refused.total(), 210);
    assert_eq!(refused.errors().len(), 100);
    let listed = params(&refused);
    assert_eq!(listed[0], "facing.k0");
    assert_eq!(listed[19], "facing.k19");
    assert_eq!(listed[20], "measures.0.k0");
    assert_eq!(listed[99], "measures.3.k19");
    // The same flood in an `elements` request's cursor, and at its top.
    let text = format!(
        r#"{{"session": "s1", "model": "current", "feature": null, "after": {{"revision": 1, "feature": "k7f2qx3m", "name": "x", {}}}, "limit": 2, {}}}"#,
        keys(150),
        keys(150)
    );
    let refused = ElementsRequest::read(&parsed(&text)).unwrap_err();
    assert_eq!(refused.total(), 300);
    assert_eq!(refused.errors().len(), 100);
    assert_eq!(params(&refused)[99], "after.k99");
}

#[test]
fn every_request_that_isnt_an_object_is_one_reason_naming_no_field() {
    for text in ["[]", "null", "\"s1\"", "0", "true"] {
        let node = parsed(text);
        for refused in [
            QueryRequest::read(&node).unwrap_err(),
            ElementsRequest::read(&node).unwrap_err(),
            SchemaRequest::read(&node).unwrap_err(),
        ] {
            assert_eq!(refused.total(), 1, "{text}");
            let error = &refused.errors()[0];
            assert_eq!(error.parameter(), None, "{text}");
            assert!(
                error
                    .message()
                    .starts_with("the request must be an object, got "),
                "{text}: {}",
                error.message()
            );
        }
    }
}

// --- Words ------------------------------------------------------------------------------

/// Words that differ from `face` only by case, whitespace, a look-alike, or a character
/// drawn as nothing or acted on by a terminal.
fn near_faces() -> Vec<String> {
    [
        "Face",
        "FACE",
        " face",
        "face ",
        "face\n",
        "face\t",
        "face\r",
        "face\u{a0}",
        "fac\u{435}",
        "face\u{200b}",
        "\u{feff}face",
        "\u{ff46}\u{ff41}\u{ff43}\u{ff45}",
        "face\u{0}",
        "\u{1b}[31mface",
        "face\u{2028}",
        "face\u{2029}",
        "\u{202e}ecaf",
        "\u{2066}face",
        "face\u{e0041}",
        "fa\u{ad}ce",
        "face\u{301}",
        "face\u{7f}",
        "face\u{85}",
        "",
        "faces",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[test]
fn a_word_near_one_of_the_commands_is_refused_safe_to_print_and_told_apart() {
    let mut messages: Vec<String> = Vec::new();
    for word in near_faces() {
        let text = query_with(
            r#""kind": "face""#,
            &format!(r#""kind": {}"#, serde_json::to_string(&word).unwrap()),
        );
        let refused = query_refused(&text);
        assert_eq!(refused.total(), 1, "{word:?}");
        let error = &refused.errors()[0];
        assert_eq!(error.parameter(), Some("kind"), "{word:?}");
        assert_eq!(error.candidates(), ["face", "edge", "vertex"], "{word:?}");
        assert_eq!(error.suggestion(), Some("give one of the kinds"));
        assert!(
            printable(error.message()),
            "{word:?}: {:?}",
            error.message()
        );
        assert!(
            error
                .message()
                .starts_with("kind must be face, edge, or vertex, got "),
            "{word:?}: {}",
            error.message()
        );
        // The error as the wire writes it, on one line and canonically, holds nothing raw
        // either.
        let line = write(&error.to_wire(), Layout::OneLine).unwrap();
        assert!(!line.contains('\n'), "{word:?}");
        messages.push(error.message().to_owned());
    }
    // No two of them read alike.
    for (i, a) in messages.iter().enumerate() {
        for b in &messages[i + 1..] {
            assert_ne!(a, b);
        }
    }
    // A look-alike and an invisible character are quoted, never bare, so they don't read
    // as the word itself.
    let shown = |word: &str| {
        let text = query_with(
            r#""kind": "face""#,
            &format!(r#""kind": {}"#, serde_json::to_string(word).unwrap()),
        );
        said(&query_refused(&text))[0].1.clone()
    };
    assert_eq!(
        shown("fac\u{435}"),
        "kind must be face, edge, or vertex, got \"fac\u{435}\""
    );
    assert_eq!(
        shown("face\u{200b}"),
        r#"kind must be face, edge, or vertex, got "face\u{200b}""#
    );
    assert_eq!(
        shown("\u{1b}[31mface"),
        r#"kind must be face, edge, or vertex, got "\u{1b}[31mface""#
    );
    assert_eq!(shown(""), r#"kind must be face, edge, or vertex, got """#);
    // The word itself, spelled with JSON's escapes, is the word.
    let spelled = r#""kind": "\u0066a\u0063e""#;
    assert!(spelled.contains("\\u0066"), "{spelled}");
    let escaped = QueryRequest::read(&parsed(&query_with(r#""kind": "face""#, spelled))).unwrap();
    assert_eq!(escaped.query.kind, ElementKind::Face);
}

#[test]
fn a_long_word_is_cut_short_in_every_word_field() {
    // A megabyte of word in each field read as one: each refusal short, and a plain word
    // just past what's shown bare quoted and cut.
    let long = "x".repeat(1 << 20);
    let json = serde_json::to_string(&long).unwrap();
    let text = query_with(r#""kind": "face""#, &format!(r#""kind": {json}"#))
        .replace(r#""type": null"#, &format!(r#""type": {json}"#))
        .replace(
            r#""order": null"#,
            &format!(r#""order": {{"by": {json}, "descending": true}}"#),
        );
    let text = text.replace(
        r#""measures": []"#,
        &format!(r#""measures": [{{"measure": {json}, "at_least": null, "at_most": null}}]"#),
    );
    let refused = query_refused(&text);
    assert_eq!(
        params(&refused),
        ["kind", "type", "measures.0.measure", "order.by"]
    );
    for error in refused.errors() {
        assert!(
            error.message().len() < 400,
            "{}: {} bytes",
            error.parameter().unwrap_or(""),
            error.message().len()
        );
        assert!(error.message().ends_with("\"…"), "{}", error.message());
    }
    // 40 characters are shown bare; 41 are quoted and cut.
    let at = |n: usize| {
        let text = query_with(
            r#""kind": "face""#,
            &format!(r#""kind": "{}""#, "y".repeat(n)),
        );
        said(&query_refused(&text))[0].1.clone()
    };
    assert!(
        at(40).ends_with(&format!("got {}", "y".repeat(40))),
        "{}",
        at(40)
    );
    assert!(
        at(41).ends_with(&format!("got \"{}\"…", "y".repeat(40))),
        "{}",
        at(41)
    );
}

#[test]
fn a_word_refused_as_a_string_is_told_apart_from_the_json_value_it_spells() {
    // The string "null" and the value null are refused in other words, and the string "3"
    // and the number 3: a refused word that spells a JSON value is quoted, so `"type":
    // "null"` isn't told it can't be null, which is what `type` takes.
    for field in ["kind", "type"] {
        for (string, value) in [
            (r#""null""#, "null"),
            (r#""3""#, "3"),
            (r#""true""#, "true"),
            (r#""false""#, "false"),
            (r#""-1""#, "-1"),
        ] {
            let from = if field == "kind" {
                r#""kind": "face""#
            } else {
                r#""type": null"#
            };
            let as_string = said(&query_refused(&query_with(
                from,
                &format!(r#""{field}": {string}"#),
            )))[0]
                .1
                .clone();
            let as_value = match QueryRequest::read(&parsed(&query_with(
                from,
                &format!(r#""{field}": {value}"#),
            ))) {
                Ok(_) => String::new(),
                Err(refused) => said(&refused)[0].1.clone(),
            };
            assert_ne!(
                as_string, as_value,
                "{field}: {string} and {value} are refused alike"
            );
            assert!(
                !as_string.ends_with(&format!("got {value}")),
                "{field}: {string} is said as {value}: {as_string}"
            );
        }
    }
}

#[test]
fn a_nullable_words_refusal_says_null_is_one_of_its_values() {
    // `type` takes null, as `stable`, `feature`, and a range's bounds do, and its refusal
    // says so, as theirs do ("true, false, or null"), so an agent isn't told a type is
    // required.
    let stable = said(&query_refused(&query_with(
        r#""stable": null"#,
        r#""stable": "maybe""#,
    )))[0]
        .1
        .clone();
    assert!(stable.contains("or null"), "{stable}");
    let typed = said(&query_refused(&query_with(
        r#""type": null"#,
        r#""type": "nurbs""#,
    )))[0]
        .1
        .clone();
    assert!(typed.contains("or null"), "{typed}");
}

#[test]
fn a_nullable_field_given_each_wrong_kind_says_what_it_was() {
    for (from, to, want) in [
        (
            r#""stable": null"#,
            r#""stable": 0"#,
            "stable must be true, false, or null, got 0",
        ),
        (
            r#""stable": null"#,
            r#""stable": "true""#,
            r#"stable must be true, false, or null, got "true""#,
        ),
        (
            r#""stable": null"#,
            r#""stable": []"#,
            "stable must be true, false, or null, got an array",
        ),
        (
            r#""feature": null"#,
            r#""feature": "K7F2QX3M""#,
            r#"feature must be a feature's ID or null, got "K7F2QX3M""#,
        ),
        (
            r#""facing": null"#,
            r#""facing": []"#,
            "facing must be an object, got an array",
        ),
        (
            r#""order": null"#,
            r#""order": "area""#,
            "order must be an object, got a string",
        ),
        (
            r#""measures": []"#,
            r#""measures": null"#,
            "measures must be a list, got null",
        ),
        (
            r#""measures": []"#,
            r#""measures": {}"#,
            "measures must be a list, got an object",
        ),
    ] {
        assert_eq!(
            said(&query_refused(&query_with(from, to))),
            [(want.split(' ').next().unwrap().to_owned(), want.to_owned())],
            "{to}"
        );
    }
}

// --- Numbers ----------------------------------------------------------------------------

/// The range `{"measure": measure, "at_least": bound, "at_most": null}` read: its bound, or
/// its one refusal's message and suggestion.
fn bound(measure: &str, bound: &str) -> Result<Quantity, (String, String)> {
    let text = with_measures(&format!(
        r#"{{"measure": "{measure}", "at_least": {bound}, "at_most": null}}"#
    ));
    match QueryRequest::read(&parsed(&text)) {
        Ok(request) => Ok(request.query.measures[0].at_least.unwrap()),
        Err(refused) => {
            assert_eq!(refused.total(), 1, "{bound}");
            let error = &refused.errors()[0];
            assert_eq!(error.parameter(), Some("measures.0.at_least"));
            Err((
                error.message().to_owned(),
                error.suggestion().unwrap_or("").to_owned(),
            ))
        }
    }
}

const AREA_MOST: &str = "170141183460469231731687303715884.105727";

#[test]
fn an_areas_bound_is_read_exactly_to_the_edge_of_an_i128_of_millionths_and_written_back() {
    let area = |millionths: i128| Ok(Quantity::Area(Measure::from_millionths(millionths)));
    assert_eq!(bound("area", AREA_MOST), area(i128::MAX));
    assert_eq!(bound("area", &format!("-{AREA_MOST}")), area(-i128::MAX));
    // Exponents, read exactly, either case, either sign.
    assert_eq!(bound("area", "1e3"), area(1_000_000_000));
    assert_eq!(bound("area", "1.5E+2"), area(150_000_000));
    assert_eq!(bound("area", "1e-6"), area(1));
    assert_eq!(bound("area", "1000000e-12"), area(1));
    assert_eq!(bound("area", "100e-2"), area(1_000_000));
    assert_eq!(bound("area", "1e32"), area(10i128.pow(38)));
    assert_eq!(bound("area", "0e99999999999999999999"), area(0));
    assert_eq!(bound("area", "0.000000000"), area(0));
    // Past the edge either way, or past its places: refused, saying the range and the
    // places.
    let range = format!("an area is at most {AREA_MOST} mm² either way");
    for past in [
        "170141183460469231731687303715884.105728",
        "-170141183460469231731687303715884.105728",
        "1e33",
        "1e99999999999999999999",
        &"9".repeat(100_000),
    ] {
        let (message, suggestion) = bound("area", past).unwrap_err();
        assert!(message.ends_with("which is too large"), "{message}");
        assert!(message.len() < 300, "{} bytes", message.len());
        assert_eq!(suggestion, range, "{past}");
    }
    for fine in [
        "1e-7",
        "1.0000001",
        "0.0000001",
        "-0.0000001",
        "1e-99999999999999999999",
    ] {
        let (message, suggestion) = bound("area", fine).unwrap_err();
        assert!(
            message.ends_with("which has more than 6 decimals"),
            "{fine}: {message}"
        );
        assert_eq!(suggestion, "round it to 6 decimals", "{fine}");
    }
    for zero in ["-0", "-0.0", "-0e5", "-0.000000000"] {
        let (message, suggestion) = bound("area", zero).unwrap_err();
        assert!(
            message.ends_with("which is zero written with a minus sign"),
            "{zero}"
        );
        assert_eq!(suggestion, "write 0");
    }
    // Each edge written back as read, and read again to itself.
    let text = with_measures(&format!(
        r#"{{"measure": "area", "at_least": -{AREA_MOST}, "at_most": {AREA_MOST}}}"#
    ));
    let request = QueryRequest::read(&parsed(&text)).unwrap();
    let back = written(&request);
    assert!(back.contains(&format!("-{AREA_MOST}")), "{back}");
    assert_eq!(QueryRequest::read(&parsed(&back)).unwrap(), request);
    holds(
        &snapshot_part("query", "request", None),
        &back,
        "area's edges",
    );
}

#[test]
fn a_length_and_an_angle_bound_are_read_to_their_places_and_range() {
    let length = |nm: i64| Ok(Quantity::Length(Length::from_nm(nm)));
    let angle = |ndeg: i64| Ok(Quantity::Angle(Angle::from_ndeg(ndeg)));
    for measure in [
        "length",
        "radius",
        "diameter",
        "major_radius",
        "minor_radius",
    ] {
        assert_eq!(bound(measure, "9223372036854.775807"), length(i64::MAX));
        assert_eq!(bound(measure, "-9223372036854.775807"), length(-i64::MAX));
        assert_eq!(bound(measure, "1e-6"), length(1));
        // One nanometre past the bound, either way.
        for past in ["9223372036854.775808", "-9223372036854.775808"] {
            let (message, suggestion) = bound(measure, past).unwrap_err();
            assert!(message.ends_with("which is too large"), "{message}");
            assert_eq!(
                suggestion,
                "a length is at most 9223372036854.775807 mm either way"
            );
        }
        let (_, suggestion) = bound(measure, "1e-7").unwrap_err();
        assert_eq!(
            suggestion,
            "round it to 6 decimals: lengths are whole nanometres"
        );
        // An angle's places aren't a length's.
        assert!(bound(measure, "0.000000001").is_err());
    }
    assert_eq!(bound("half_angle", "9223372036.854775807"), angle(i64::MAX));
    assert_eq!(
        bound("half_angle", "-9223372036.854775807"),
        angle(-i64::MAX)
    );
    assert_eq!(bound("half_angle", "1e-9"), angle(1));
    assert_eq!(bound("half_angle", "-90"), angle(-90_000_000_000));
    let (_, suggestion) = bound("half_angle", "1e-10").unwrap_err();
    assert_eq!(
        suggestion,
        "round it to 9 decimals: angles are whole nanodegrees"
    );
    for past in ["9223372036.854775808", "-9223372036.854775808"] {
        let (_, suggestion) = bound("half_angle", past).unwrap_err();
        assert_eq!(
            suggestion,
            "an angle is at most 9223372036.854775807 degrees either way"
        );
    }
    // A bound of the wrong kind says the measure's quantity.
    let (message, _) = bound("half_angle", "\"30\"").unwrap_err();
    assert_eq!(
        message,
        r#"measures.0.at_least must be an angle in degrees or null, got "30""#
    );
}

#[test]
fn a_range_whose_measure_is_refused_takes_bounds_of_every_kind_unread() {
    // Whatever the bounds are, a range of no measure is one reason, or two with a bound
    // missing; deep, huge, and past every range alike.
    let deep = format!("{}{}", "[".repeat(60), "]".repeat(60));
    let huge = "9".repeat(100_000);
    for bad_measure in [r#""volume""#, "null", "1", r#"{"area": 1}"#, r#""AREA""#] {
        for bounds in [
            format!(r#""at_least": {deep}, "at_most": {huge}"#),
            r#""at_least": -0, "at_most": 1e99999999999999999999"#.to_owned(),
            r#""at_least": {"x": [1, 2, 3]}, "at_most": "∞""#.to_owned(),
        ] {
            let text = with_measures(&format!(r#"{{"measure": {bad_measure}, {bounds}}}"#));
            assert_eq!(
                params(&query_refused(&text)),
                ["measures.0.measure"],
                "{bad_measure}"
            );
        }
    }
    // The measure missing: its own reason, the bounds still unread.
    let text = with_measures(r#"{"at_least": "x", "at_most": "y"}"#);
    assert_eq!(params(&query_refused(&text)), ["measures.0.measure"]);
    assert_eq!(
        said(&query_refused(&text))[0].1,
        "measures.0.measure is missing"
    );
    // A range that's empty: no measure and no bounds.
    let text = with_measures("{}");
    assert_eq!(
        params(&query_refused(&text)),
        [
            "measures.0.measure",
            "measures.0.at_least",
            "measures.0.at_most"
        ]
    );
}

// --- Directions and within -------------------------------------------------------------

/// Whether `text` writes a number with an exponent: a digit followed by `e` or `E`.
fn has_exponent(text: &str) -> bool {
    text.as_bytes()
        .windows(2)
        .any(|w| w[0].is_ascii_digit() && matches!(w[1], b'e' | b'E'))
}

fn facing(direction: &str, within: &str) -> String {
    query_with(
        r#""facing": null"#,
        &format!(r#""facing": {{"direction": {direction}, "within": {within}}}"#),
    )
}

#[test]
fn a_direction_of_tiny_huge_and_subnormal_components_is_read_to_its_bits_and_written_back() {
    let schema = snapshot_part("query", "request", None);
    for (text, want) in [
        ("[5e-324, 0, 0]", [5e-324, 0.0, 0.0]),
        ("[0, -5e-324, 0]", [0.0, -5e-324, 0.0]),
        (
            "[2.2250738585072014e-308, 0, 0]",
            [f64::MIN_POSITIVE, 0.0, 0.0],
        ),
        (
            "[1.7976931348623157e308, -1.7976931348623157e308, 0]",
            [f64::MAX, -f64::MAX, 0.0],
        ),
        ("[1e-300, -1e300, 2.5]", [1e-300, -1e300, 2.5]),
        ("[1E-160, 1e-200, -1e-200]", [1e-160, 1e-200, -1e-200]),
        (
            "[0.1, 0.2, 0.30000000000000004]",
            [0.1, 0.2, 0.30000000000000004],
        ),
    ] {
        let request = QueryRequest::read(&parsed(&facing(text, "1"))).unwrap();
        let got = request.query.facing.as_ref().unwrap().direction;
        assert_eq!(got.map(f64::to_bits), want.map(f64::to_bits), "{text}");
        // Written as plain decimals, read back to the same bits, and held to the schema.
        let back = written(&request);
        assert!(!has_exponent(&back), "{back}");
        let again = QueryRequest::read(&parsed(&back)).unwrap();
        assert_eq!(
            again.query.facing.unwrap().direction.map(f64::to_bits),
            want.map(f64::to_bits),
            "{text}"
        );
        holds(&schema, &back, text);
    }
    // -0 reads as -0, and is written 0: a direction keeps no sign on a zero.
    let request = QueryRequest::read(&parsed(&facing("[-0, 0, -1]", "1"))).unwrap();
    assert!(request.query.facing.as_ref().unwrap().direction[0].is_sign_negative());
    assert!(written(&request).contains("[0, 0, -1]"));
}

#[test]
fn a_direction_past_a_float_or_of_the_wrong_shape_is_refused_by_its_place() {
    // Past the largest f64 (it rounds to infinity): refused at its place, cut short.
    let refused = query_refused(&facing("[1.7976931348623159e308, 0, 0]", "1"));
    assert_eq!(
        said(&refused),
        [(
            "facing.direction.0".to_owned(),
            "facing.direction.0 must be a finite number, got 1.7976931348623159e308".to_owned()
        )]
    );
    let refused = query_refused(&facing(&format!("[0, 1{}, 0]", "0".repeat(100_000)), "1"));
    assert_eq!(params(&refused), ["facing.direction.1"]);
    assert!(said(&refused)[0].1.len() < 300);
    // Every member at fault, each said.
    let refused = query_refused(&facing(r#"[null, "1", 1e999]"#, "1"));
    assert_eq!(
        params(&refused),
        [
            "facing.direction.0",
            "facing.direction.1",
            "facing.direction.2"
        ]
    );
    // Not three: said how many, however many.
    for (members, n) in [("[]", 0), ("[1]", 1), ("[1, 0]", 2), ("[1, 0, 0, 0]", 4)] {
        assert_eq!(
            said(&query_refused(&facing(members, "1")))[0].1,
            format!(
                "facing.direction must be a direction, three numbers [x, y, z], got {n} of them"
            )
        );
    }
    let million = format!("[{}]", vec!["1"; 1_000_000].join(", "));
    assert_eq!(
        said(&query_refused(&facing(&million, "1")))[0].1,
        "facing.direction must be a direction, three numbers [x, y, z], got 1000000 of them"
    );
    // Zeros of every spelling, and components that underflow to zero: a zero direction.
    for zero in [
        "[0, 0, 0]",
        "[-0, 0.0, 0e10]",
        "[1e-400, 0, 0]",
        "[1e-400, -1e-400, 1e-999]",
    ] {
        let refused = query_refused(&facing(zero, "1"));
        assert_eq!(params(&refused), ["facing.direction"], "{zero}");
        assert_eq!(
            refused.errors()[0].suggestion(),
            Some("give a direction that isn't zero")
        );
    }
}

#[test]
fn within_is_read_as_any_angle_and_the_command_holds_it_from_0_to_180() {
    let model = model_of(BOX);
    let run = |within: &str| -> Result<(), Vec<String>> {
        let request = QueryRequest::read(&parsed(&facing("[0, 0, 1]", within)))
            .map_err(|refused| params(&refused))?;
        query(&model, &request.query)
            .map(|_| ())
            .map_err(|refused| params(&refused))
    };
    for fine in ["0", "180", "180.000000000", "1e-9", "1.8e2", "90"] {
        assert_eq!(run(fine), Ok(()), "{fine}");
    }
    for past in [
        "180.000000001",
        "-0.000000001",
        "-180",
        "360",
        "9223372036.854775807",
        "-9223372036.854775807",
    ] {
        // Read (an angle the reader holds), then held to 0 to 180° by the command.
        assert!(
            QueryRequest::read(&parsed(&facing("[0, 0, 1]", past))).is_ok(),
            "{past}"
        );
        assert_eq!(run(past), Err(vec!["facing.within".to_owned()]), "{past}");
    }
    // Past an angle's range (at most 9223372036.854775807 either way) or places, or -0: the
    // reader's.
    for refused in [
        "9223372036.854775808",
        "-9223372036.854775808",
        "1e-10",
        "-0",
        "\"90\"",
        "null",
    ] {
        let problems = query_refused(&facing("[0, 0, 1]", refused));
        assert_eq!(params(&problems), ["facing.within"], "{refused}");
    }
}

// --- elements' request ------------------------------------------------------------------

#[test]
fn an_elements_limit_and_cursor_revision_are_read_to_their_types_edges() {
    let text = |limit: &str, revision: &str| {
        format!(
            r#"{{"session": "s1", "model": "current", "feature": "k7f2qx3m", "after": {{"revision": {revision}, "feature": "k7f2qx3m", "name": ""}}, "limit": {limit}}}"#
        )
    };
    let request =
        ElementsRequest::read(&parsed(&text("4294967295", "18446744073709551615"))).unwrap();
    assert_eq!(request.limit, u32::MAX);
    assert_eq!(request.after.as_ref().unwrap().revision.number(), u64::MAX);
    assert_eq!(request.after.as_ref().unwrap().name.as_str(), "");
    let back = written(&request);
    assert_eq!(ElementsRequest::read(&parsed(&back)).unwrap(), request);
    for (limit, revision, want) in [
        ("4294967296", "1", vec!["limit"]),
        ("0", "18446744073709551616", vec!["after.revision"]),
        ("-0", "-1", vec!["after.revision", "limit"]),
        ("1e3", "6.0", vec!["after.revision", "limit"]),
    ] {
        let refused = ElementsRequest::read(&parsed(&text(limit, revision))).unwrap_err();
        assert_eq!(params(&refused), want, "{limit} {revision}");
    }
    // The command, not the reader, refuses a limit of 0 or past 1,000.
    let model = model_of(BOX);
    for limit in [0, 1001, u32::MAX] {
        let refused = list(&model, Revision::OPENED, None, None, limit).unwrap_err();
        assert_eq!(params(&refused), ["limit"], "{limit}");
    }
}

#[test]
fn a_cursor_passed_back_as_the_result_wrote_it_is_read_to_itself() {
    // `next` is passed back as it was: whatever its name holds, the cursor a result writes
    // reads back, in `after`, to the same cursor, in both layouts.
    let names = [
        String::new(),
        "k7f2qx3m.box.Face2".to_owned(),
        "\"quoted\" \\ back\\slash".to_owned(),
        "nul\u{0} esc\u{1b}[2J line\u{2028}para\u{2029} bom\u{feff} rtl\u{202e}".to_owned(),
        "emoji \u{1f600} tag\u{e0041} combining e\u{301}".to_owned(),
        "n".repeat(100_000),
    ];
    let request = ElementsRequest::read(&parsed(
        r#"{"session": "s1", "model": "current", "feature": null, "after": {"revision": 18446744073709551615, "feature": "k7f2qx3m", "name": ""}, "limit": 2}"#,
    ))
    .unwrap();
    let top = request.after.unwrap().revision;
    for name in names {
        let cursor = Cursor {
            revision: top,
            feature: silico_core::document::FeatureId::new("k7f2qx3m").unwrap(),
            name: ElementName::as_sent(name.as_str()),
        };
        for layout in [Layout::Canonical, Layout::OneLine] {
            let next = write(&cursor.to_wire(), layout).unwrap();
            let text = format!(
                r#"{{"session": "s1", "model": "current", "feature": null, "after": {next}, "limit": 2}}"#
            );
            let back = ElementsRequest::read(&parsed(&text)).unwrap();
            assert_eq!(back.after.as_ref(), Some(&cursor), "{layout:?}");
        }
        holds(
            &snapshot_part("elements", "result", Some("Cursor")),
            &written(&cursor),
            "a cursor",
        );
    }
}

// --- schema -----------------------------------------------------------------------------

#[test]
fn schema_refuses_names_that_arent_names_safely_and_briefly() {
    let long = "d".repeat(1 << 20);
    for of in [
        "document\n",
        "document\0",
        "\u{1b}[2Jdocument",
        "document\u{200b}",
        "\u{feff}document",
        "\u{202e}tnemucod",
        "DOCUMENT",
        "Document",
        "d\u{3bf}cument",
        "../envelope",
        "./document",
        "commands/elements",
        "elements.json",
        "document.json",
        "/document",
        "query\t",
        long.as_str(),
    ] {
        let refused = schema(of).unwrap_err();
        assert_eq!(refused.total(), 1, "{of:?}");
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
                "meshes",
                "sketch",
                "sketch_add",
                "sketch_remove",
                "sketch_split",
                "sketch_trim",
                "sketch_unjoin",
                "sketch_merge",
                "undo",
                "redo"
            ]
        );
        assert!(printable(error.message()), "{of:?}: {:?}", error.message());
        assert!(error.message().len() < 300, "{of:?}");
        assert!(
            error
                .message()
                .starts_with("of must be document or the name of a command with a schema, got "),
            "{}",
            error.message()
        );
    }
}

#[test]
fn a_schema_request_is_read_strictly() {
    let refused = |text: &str| said(&SchemaRequest::read(&parsed(text)).unwrap_err());
    assert_eq!(
        refused(r#"{"of": null}"#),
        [("of".to_owned(), "of must be a string, got null".to_owned())]
    );
    assert_eq!(
        refused(r#"{"of": ["document"]}"#),
        [(
            "of".to_owned(),
            "of must be a string, got an array".to_owned()
        )]
    );
    assert_eq!(
        refused("{}"),
        [("of".to_owned(), "of is missing".to_owned())]
    );
    assert_eq!(
        refused(r#"{"of": "document", "Of": "query"}"#),
        [(
            "Of".to_owned(),
            "Of isn't a field of this request".to_owned()
        )]
    );
    // A key given twice never reaches the reader: the JSON is refused.
    assert!(read(br#"{"of": "document", "of": "query"}"#).is_err());
    // A string of any name is the request's; whether it names a schema is the command's.
    let request = SchemaRequest::read(&parsed(r#"{"of": "\u001b[2J"}"#)).unwrap();
    assert_eq!(request.of, "\u{1b}[2J");
    assert!(!written(&request).contains('\u{1b}'));
    assert_eq!(
        SchemaRequest::read(&parsed(&written(&request))).unwrap(),
        request
    );
    assert!(schema(&request.of).is_err());
}

// --- Results of unusual values, held to the snapshots ----------------------------------

/// The fake box's entries, as `elements` lists them.
fn box_entries() -> Listed {
    list(&model_of(BOX), Revision::OPENED, None, None, 1000).unwrap()
}

#[test]
fn entries_of_unusual_but_valid_values_hold_to_the_entrys_schema() {
    let schema = snapshot_part("elements", "result", Some("Entry"));
    let listed = box_entries();
    let face = listed
        .entries
        .iter()
        .find(|e| e.kind == ElementKind::Face)
        .unwrap()
        .clone();
    // Every field that may be null, null.
    let mut bare = face.clone();
    bare.aliases.clear();
    bare.stable = false;
    bare.area = None;
    bare.length = None;
    bare.position = None;
    bare.normal = None;
    bare.centroid = None;
    bare.start = None;
    bare.end = None;
    bare.midpoint = None;
    bare.straight = None;
    bare.axis = None;
    bare.radius = None;
    bare.diameter = None;
    bare.major_radius = None;
    bare.minor_radius = None;
    bare.half_angle = None;
    bare.apex = None;
    bare.default_x = None;
    let text = written(&bare);
    holds(&schema, &text, "every field null");
    assert_eq!(text.matches("null").count(), 17, "{text}");

    // A cone whose half-angle is negative, its axis pointing down with a -0 in it, and its
    // apex at the ends of what a length holds.
    let mut cone = bare.clone();
    cone.r#type = ElementType::Cone;
    cone.area = Some(Measure::from_millionths(i128::MAX));
    cone.centroid = Some([
        Length::from_nm(i64::MIN),
        Length::from_nm(0),
        Length::from_nm(i64::MAX),
    ]);
    cone.axis = Some(EntryAxis {
        point: [Length::from_nm(-1); 3],
        direction: [-0.0, 0.0, -1.0],
    });
    cone.half_angle = Some(Angle::from_ndeg(-33_690_067_526));
    cone.apex = Some([Length::from_nm(i64::MIN); 3]);
    let text = written(&cone);
    holds(&schema, &text, "a cone");
    assert!(text.contains("\"half_angle\": -33.690067526"), "{text}");
    assert!(text.contains("[0, 0, -1]"), "{text}");
    assert!(!text.contains("-0,"), "{text}");
    assert!(text.contains(&format!("\"area\": {AREA_MOST}")), "{text}");
    assert!(text.contains("-9223372036854.775808"), "{text}");

    // A planar face of every default x, its normal of subnormal, huge, and -0 components.
    for axis in AxisToken::ALL {
        let mut plane = face.clone();
        plane.default_x = Some(axis);
        plane.normal = Some([5e-324, -f64::MAX, -0.0]);
        let text = written(&plane);
        holds(&schema, &text, axis.word());
        assert!(text.contains(&format!("\"default_x\": \"{}\"", axis.word())));
    }

    // Aliases, stable and not, in the order given, the name unstable.
    let mut aliased = face.clone();
    let others: Vec<_> = listed.entries.iter().skip(1).take(3).collect();
    aliased.stable = false;
    aliased.aliases = others
        .iter()
        .enumerate()
        .map(|(i, e)| Alias {
            name: e.name.clone(),
            stable: i % 2 == 0,
        })
        .collect();
    let text = written(&aliased);
    holds(&schema, &text, "aliases");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let aliases: Vec<(String, bool)> = value["aliases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["name"].as_str().unwrap().to_owned(),
                a["stable"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        aliases,
        others
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.to_string(), i % 2 == 0))
            .collect::<Vec<_>>()
    );
    // Every type the entry's schema lists, written.
    for t in ElementType::ALL {
        let mut typed = bare.clone();
        typed.r#type = t;
        holds(&schema, &written(&typed), t.word());
    }
}

#[test]
fn results_at_their_counts_edges_hold_to_their_snapshots() {
    let listed = box_entries();
    let elements = snapshot_part("elements", "result", None);
    let queried = snapshot_part("query", "result", None);
    let request = ElementsRequest::read(&parsed(
        r#"{"session": "s1", "model": "current", "feature": null, "after": {"revision": 18446744073709551615, "feature": "k7f2qx3m", "name": "\u0000 \""}, "limit": 2}"#,
    ))
    .unwrap();
    let page = Listed {
        entries: listed.entries.clone(),
        total: u64::MAX,
        next: request.after.clone(),
    };
    let text = written(&page);
    holds(&elements, &text, "a page at the counts' edges");
    let last = Listed {
        entries: Vec::new(),
        total: 0,
        next: None,
    };
    holds(&elements, &written(&last), "an empty last page");
    for tied in [
        None,
        Some(Tied {
            count: u64::MAX,
            included: true,
        }),
        Some(Tied {
            count: 0,
            included: false,
        }),
    ] {
        let result = Queried {
            entries: listed.entries.clone(),
            matched: u64::MAX,
            tied,
        };
        holds(&queried, &written(&result), "a query's result");
    }
    // A facing of the largest angle a request holds (at most i64::MAX nanodegrees either
    // way), and every order, written and read back.
    let q = Query {
        facing: Some(Facing {
            direction: [f64::MAX, -5e-324, 0.0],
            within: Angle::from_ndeg(-i64::MAX),
        }),
        ..Query::of(ElementKind::Face, 0)
    };
    let request = QueryRequest {
        session: silico_core::command::SessionId::parse("s18446744073709551615").unwrap(),
        model: silico_core::command::ModelChoice::Published,
        query: q,
    };
    let text = written(&request);
    assert_eq!(QueryRequest::read(&parsed(&text)).unwrap(), request);
}
