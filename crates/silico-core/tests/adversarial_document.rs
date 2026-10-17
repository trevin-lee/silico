//! break-it: hostile input for the M2.1 document reader and writer (`document::read`,
//! `document::write`, `document::json::read`, `Length`, `Angle`). Expected values come
//! from hand calculation, from RFC 8259 (via serde_json as an independent reader), or
//! from the reader's own stated contract (docs/design/m2.1-document-format.md). Tests
//! marked `Once:` failed on the code as it was first written: each records a bug fixed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;
#[path = "common/within.rs"]
mod within;

use std::time::Duration;

use example::EXAMPLE;
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::document::json::{self, Value};
use silico_core::document::{
    Angle, Document, Feature, FeatureId, FeatureVersion, FormatVersion, Length, Name, NumberError,
    Operation, Units, read, write,
};
use within::within;

const HEAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": ["#;

/// A document holding the features' JSON texts.
fn doc(features: &[&str]) -> String {
    format!("{HEAD}{}]}}", features.join(", "))
}

/// A box feature with `length` written as `text`.
fn box_with_length(text: &str) -> String {
    doc(&[&format!(
        r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": {text}, "width": 1, "height": 1}}"#
    )])
}

fn example_with(from: &str, to: &str) -> String {
    assert!(EXAMPLE.contains(from), "{from}");
    EXAMPLE.replacen(from, to, 1)
}

/// The error reading `text` gives: `invalid_input`, with a suggestion.
fn refused(text: &str) -> silico_core::Error {
    let err = match read(text.as_bytes()) {
        Ok(d) => panic!("read {text:?} as {d:?}"),
        Err(e) => e,
    };
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(err.suggestion().is_some(), "{err}");
    err
}

fn the_length(text: &str) -> Length {
    let document = read(box_with_length(text).as_bytes()).unwrap();
    match &document.features[0].operation {
        Operation::Box { length, .. } => *length,
        other => panic!("{other:?}"),
    }
}

fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

fn a_box(id: &str, name: &str) -> Feature {
    Feature {
        id: FeatureId::new(id).unwrap(),
        operation: Operation::Box {
            position: [Length::from_nm(0); 3],
            length: Length::from_nm(1),
            width: Length::from_nm(1),
            height: Length::from_nm(1),
        },
        version: FeatureVersion,
        name: Name::new(name).unwrap(),
        note: None,
        fingerprint: None,
    }
}

fn one_feature(feature: Feature) -> Document {
    Document {
        silico: FormatVersion,
        units: Units,
        features: vec![feature],
    }
}

// ---------------------------------------------------------------- numbers at every edge

#[test]
fn length_at_i64_bounds_reads_exactly_and_one_nanometre_past_is_refused() {
    assert_eq!(the_length("9223372036854.775807").nm(), i64::MAX);
    assert_eq!(the_length("-9223372036854.775808").nm(), i64::MIN);
    // Trailing zeros past the resolution don't change the bound.
    assert_eq!(the_length("9223372036854.775807000").nm(), i64::MAX);
    for past in [
        "9223372036854.775808",
        "-9223372036854.775809",
        "9223372036855",
        "18446744073709.551616",
        "170141183460469231731687303715884105728", // 2^127: past i128 too
    ] {
        let err = refused(&box_with_length(past));
        assert_eq!(err.parameter(), Some("length"), "{past}: {err}");
        assert!(err.message().contains("too large"), "{past}: {err}");
    }
}

#[test]
fn negative_zero_every_spelling_is_refused_and_signed_zero_never_reads() {
    for text in ["-0", "-0.0", "-0.000000", "-0.000000000000000000"] {
        let err = refused(&box_with_length(text));
        assert!(err.message().contains("minus sign"), "{text}: {err}");
        assert_eq!(err.suggestion(), Some("write 0"), "{text}");
    }
    // A negative value below the resolution is too precise, not a negative zero.
    let err = refused(&box_with_length("-0.0000001"));
    assert!(err.message().contains("more than 6 decimals"), "{err}");
}

#[test]
fn every_exponent_spelling_reads_exactly_as_its_decimal_promptly() {
    // The maintainer's choice (2026-10-14): a number with an exponent is read exactly, as
    // files saved by Python (`1e-05`) and JavaScript write them; Silico writes plain
    // decimals. Values worked out by hand.
    for (text, nm) in [
        ("0e0", 0),
        ("0E-0", 0),
        ("1e+0", 1_000_000),
        ("1E0", 1_000_000),
        ("1.5e1", 15_000_000),
        ("-2e-6", -2),
        ("1e-05", 10),
    ] {
        assert_eq!(the_length(text), Length::from_nm(nm), "{text}");
    }
    // Past i64, or past the resolution, refused saying so.
    let err = refused(&box_with_length("1e999999999999"));
    assert!(err.message().contains("is too large"), "{err}");
    let err = refused(&box_with_length("1e-999999999999"));
    assert!(err.message().contains("more than 6 decimals"), "{err}");
    // A million-digit mantissa and an exponent that brings it back: exact, promptly.
    let text = format!("1{}e-1000000", "0".repeat(1_000_000));
    let length = within(Duration::from_secs(10), move || the_length(&text));
    assert_eq!(length, Length::from_nm(1_000_000));
}

#[test]
fn a_million_trailing_zeros_read_exactly_and_promptly() {
    let text = format!("1.{}", "0".repeat(1_000_000));
    let length = within(Duration::from_secs(10), move || the_length(&text));
    assert_eq!(length, Length::from_nm(1_000_000));
    // And a non-zero digit a million places in is refused, not rounded away.
    let text = format!("1.{}1", "0".repeat(1_000_000));
    let err = within(Duration::from_secs(10), move || {
        read(box_with_length(&text).as_bytes()).unwrap_err()
    });
    let head: String = err.message().chars().take(200).collect();
    assert!(err.message().contains("more than 6 decimals"), "{head}");
}

#[test]
fn a_million_digit_length_is_refused_promptly() {
    let text = format!("1{}", "0".repeat(1_000_000));
    let err = within(Duration::from_secs(10), move || {
        read(box_with_length(&text).as_bytes()).unwrap_err()
    });
    assert_eq!(err.code(), ErrorCode::InvalidInput);
    assert_eq!(err.parameter(), Some("length"));
}

#[test]
fn a_huge_value_isnt_echoed_whole_into_the_error() {
    // Once: the message quotes the number's whole text (`length 1000...0 is too large`),
    // so a 1 MB number gives a 1 MB error, which an agent reading it over MCP can't use.
    // (Measured: a 10,000,000-digit length gives a 10,000,164-byte error.)
    let text = format!("1{}", "0".repeat(1_000_000));
    let err = refused(&box_with_length(&text));
    assert!(
        err.to_string().len() < 1_000,
        "an error of {} bytes",
        err.to_string().len()
    );
}

#[test]
fn a_huge_key_or_string_isnt_echoed_whole_into_the_error() {
    // Once: an unknown key is quoted three times (message, parameter, suggestion), and a
    // bad type, ID, or unit once each: a 1 MB key gives a 3 MB error.
    let big = "k".repeat(1_000_000);
    let texts = [
        format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [], "{big}": 0}}"#
        ),
        example_with("\"type\": \"cut\"", &format!("\"type\": \"{big}\"")),
        example_with("\"tool\": \"a91cw0de\"", &format!("\"tool\": \"{big}\"")),
        example_with("\"length\": \"mm\"", &format!("\"length\": \"{big}\"")),
    ];
    for text in &texts {
        let err = refused(text);
        let len = err.to_string().len();
        assert!(
            len < 1_000,
            "an error of {len} bytes: {}",
            &err.to_string()[..120]
        );
    }
}

#[test]
fn a_version_wrapping_past_u32_isnt_read_as_1() {
    // FreeCAD 913e409b24: a version read into a narrower unsigned type wrapped and was
    // taken for something else. 2^32 + 1 truncated to u32 is 1.
    for (from, to) in [
        ("\"silico\": 1", "\"silico\": 4294967297"),
        ("\"silico\": 1", "\"silico\": 18446744073709551617"),
        (
            "\"type\": \"box\", \"v\": 1",
            "\"type\": \"box\", \"v\": 4294967297",
        ),
        (
            "\"type\": \"box\", \"v\": 1",
            "\"type\": \"box\", \"v\": 18446744073709551617",
        ),
        (
            "\"type\": \"box\", \"v\": 1",
            "\"type\": \"box\", \"v\": -1",
        ),
        ("\"type\": \"box\", \"v\": 1", "\"type\": \"box\", \"v\": 0"),
    ] {
        let err = refused(&example_with(from, to));
        let want = if from.contains("silico") {
            "silico"
        } else {
            "v"
        };
        assert_eq!(err.parameter(), Some(want), "{to}: {err}");
    }
}

#[test]
fn a_version_too_large_for_u64_is_called_an_unknown_version_not_a_fraction() {
    // Once: `"silico": 18446744073709551616` (2^64) fails u64 parsing, and the message
    // says it's "not a whole number", which it is; 2^64 - 1 instead says "format version
    // 18446744073709551615, which this Silico doesn't read". The two should agree.
    let err = refused(&example_with(
        "\"silico\": 1",
        "\"silico\": 18446744073709551616",
    ));
    assert!(
        !err.message().contains("not a whole number"),
        "{}",
        err.message()
    );
}

#[test]
fn localized_digits_and_signs_are_refused_saying_which_character() {
    // FreeCAD 85a0d991e3: users type localized digits, signs, and decimal commas.
    for (text, shown) in [
        ("\u{ff11}\u{ff10}", "\u{ff11}"), // fullwidth 10
        ("\u{0661}\u{0660}", "\u{0661}"), // Arabic-Indic 10
        ("\u{2212}5", "\u{2212}"),        // minus sign
        ("+5", "+"),
        (".5", "."),
        ("Infinity", "I"),
        ("NaN", "N"),
    ] {
        let err = refused(&box_with_length(text));
        assert!(
            err.message().contains(&format!("`{shown}`")),
            "{text}: {err}"
        );
        assert!(err.message().contains("line 1"), "{text}: {err}");
    }
    // A decimal comma splits the value: `10,5` is the length 10 and then a stray `5`.
    let err = refused(&box_with_length("10,5"));
    assert!(err.message().contains("expected a key, found `5`"), "{err}");
}

#[test]
fn invisible_characters_are_named_by_code_point_in_errors() {
    // Once: a no-break space, zero-width space, or byte-order mark where whitespace may
    // go is reported as found ` ` or found `` (the character itself, invisible), so the
    // error points at what looks like an ordinary space or at nothing. Control characters
    // are already named as U+XXXX; these aren't, as `found` checks only `is_control`.
    for (text, code, c) in [
        ("{\"silico\":\u{a0}1}", "U+00A0", '\u{a0}'),
        ("{\"silico\":\u{200b}1}", "U+200B", '\u{200b}'),
        (
            "{\"silico\": 1,\u{feff}\"units\": {}}",
            "U+FEFF",
            '\u{feff}',
        ),
        ("{\"silico\": 1\u{2028}}", "U+2028", '\u{2028}'),
        ("{\"silico\":\u{202e}1}", "U+202E", '\u{202e}'),
        ("{\"silico\":\u{ad}1}", "U+00AD", '\u{ad}'),
    ] {
        let err = refused(text);
        assert!(err.message().contains(code), "{text:?}: {}", err.message());
        // Named by its code point alone: the character itself isn't in the message.
        assert!(!err.message().contains(c), "{text:?}: {}", err.message());
    }
    // A visible character outside ASCII is shown, with its code point.
    let err = refused("{\"silico\":\u{e9}1}");
    assert!(
        err.message().contains("`\u{e9}` (U+00E9)"),
        "{}",
        err.message()
    );
}

// ------------------------------------------------------------- unicode and escapes

#[test]
fn escaped_spellings_of_a_key_are_the_same_key() {
    // `\u0069d` is `id`: read as it, and a second spelling of it is a duplicate.
    let text = doc(&[
        r#"{"\u0069d": "k7f2qx3m", "\u0074ype": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
    ]);
    let document = read(text.as_bytes()).unwrap();
    assert_eq!(document.features[0].id.as_str(), "k7f2qx3m");
    let text = doc(&[
        r#"{"id": "k7f2qx3m", "\u0069d": "a91cw0de", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
    ]);
    let err = refused(&text);
    assert!(err.message().contains("written twice"), "{err}");
    assert!(err.message().contains("line 1, column 95"), "{err}");
}

#[test]
fn surrogate_escapes_every_wrong_way_are_refused() {
    for (escape, says) in [
        (r"\udc00", "no high half"),
        (r"\udfff", "no high half"),
        (r"\ud800", "low half"),
        (r"\ud800x", "low half"),
        (r"\ud800\n", "low half"),
        (r"\ud800\u0041", "low half"),
        (r"\ud800\ud800", "low half"),
        (r"\ud800\u", "hex digit"),
        (r"\u00", "hex digit"),
        (r"\u", "hex digit"),
        (r"\U0041", "followed by `U`"),
    ] {
        let err = refused(&example_with("\"Block\"", &format!("\"{escape}\"")));
        assert!(err.message().contains(says), "{escape}: {err}");
    }
    // Upper-case hex and the extremes of each range are characters.
    let document =
        read(example_with("\"Block\"", r#""\uD83D\uDE00\uDBFF\uDFFF\uFFFF\u0000""#).as_bytes())
            .unwrap();
    assert_eq!(
        document.features[0].name.as_str(),
        "\u{1f600}\u{10ffff}\u{ffff}\u{0}"
    );
}

#[test]
fn names_of_every_awkward_character_round_trip_byte_for_byte() {
    let names = [
        "\u{0}",
        "\u{7f}\u{80}\u{85}\u{9f}",
        "\u{2028}\u{2029}",
        "\u{feff}",
        "\u{200f}\u{202e}right-to-left\u{202c}",
        "e\u{301}\u{301}\u{301}",
        "\u{10ffff}\u{fffe}\u{ffff}",
        " ",
        "\\u0041",
        "\"}, {\"id\": \"00000000\"",
        "\r\n\r\n",
    ];
    for name in names {
        let document = one_feature(a_box("k7f2qx3m", name));
        let text = write(&document).unwrap();
        assert!(!text.contains('\r'), "{name:?}: {text}");
        assert_eq!(
            text.lines().count(),
            7,
            "{name:?}: one feature, one line: {text}"
        );
        let back = read(text.as_bytes()).unwrap_or_else(|e| panic!("{name:?}: {e}"));
        assert_eq!(back, document, "{name:?}");
        assert_eq!(write(&back).unwrap(), text, "{name:?}");
    }
}

#[test]
fn invalid_utf8_every_way_is_refused_at_the_byte() {
    // Overlong, surrogate as UTF-8, past U+10FFFF, lone continuation, truncated sequences.
    for bad in [
        &b"\xc0\xaf"[..],
        b"\xed\xa0\x80",
        b"\xf4\x90\x80\x80",
        b"\x80",
        b"\xe2\x82",
        b"\xf0\x9f\x98",
        b"\xff",
    ] {
        let mut bytes = b"{\n  \"silico\": \"\xc3\xa9".to_vec();
        bytes.extend_from_slice(bad);
        bytes.extend_from_slice(b"\"}");
        let err = read(&bytes).unwrap_err();
        assert!(
            err.message().contains("isn't UTF-8, at line 2, column 15"),
            "{bad:?}: {err}"
        );
    }
}

#[test]
fn unknown_keys_are_named_safely_in_errors() {
    // Once: unknown and duplicate keys are written into the message, parameter, and
    // suggestion raw: a key holding ESC sequences or newlines reaches a terminal or log
    // as control characters (`\u001b[2J` clears the screen of whoever reads the error),
    // and an empty key reads `has a key it can't have: , at line 1` with parameter "".
    // Types, IDs, and units are already quoted with `{:?}`.
    let keys = [r"\u001b[2J\u001b]0;pwned\u0007", r"a\nb", ""];
    for key in keys {
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [], "{key}": 0}}"#
        );
        let err = refused(&text);
        for part in [Some(err.message()), err.parameter(), err.suggestion()] {
            let part = part.unwrap_or("");
            assert!(!has_control(part), "{key:?}: {part:?}");
        }
        assert!(
            err.message().contains('"') || err.message().contains('`'),
            "{key:?}: the key isn't quoted: {:?}",
            err.message()
        );
        // The same key written twice.
        let twice = format!(r#"{{"{key}": 0, "{key}": 0}}"#);
        let err = refused(&twice);
        assert!(!has_control(err.message()), "{key:?}: {:?}", err.message());
    }
}

// ------------------------------------------------------------------ structure

#[test]
fn the_example_cut_at_every_byte_is_refused_saying_where_except_its_trailing_newline() {
    let bytes = EXAMPLE.as_bytes();
    let lines = EXAMPLE.lines().count() as u32;
    for end in 0..bytes.len() {
        let cut = &bytes[..end];
        match read(cut) {
            Ok(_) => assert_eq!(end, bytes.len() - 1, "a cut at {end} read"),
            Err(err) => {
                assert_eq!(err.code(), ErrorCode::InvalidInput);
                assert!(err.suggestion().is_some());
                let m = err.message();
                // The place is a line of the text.
                let line: u32 = m
                    .split("at line ")
                    .nth(1)
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| panic!("cut at {end}: no line in {m}"));
                assert!(line >= 1 && line <= lines, "cut at {end}: {m}");
            }
        }
    }
}

#[test]
fn every_byte_of_a_multibyte_name_cut_is_refused_as_not_utf8() {
    // `😀` is 4 bytes; cutting after 1, 2, or 3 of them leaves a truncated sequence.
    let text = example_with("\"Block\"", "\"😀\"");
    let start = text.find('😀').unwrap();
    for k in 1..4 {
        let mut bytes = text.as_bytes()[..start + k].to_vec();
        bytes.extend_from_slice(&text.as_bytes()[start + 4..]);
        let err = read(&bytes).unwrap_err();
        assert!(
            err.message().contains("isn't UTF-8, at line 5, column 56"),
            "{k}: {err}"
        );
    }
}

#[test]
fn wrong_kinds_of_every_document_field_are_refused_naming_the_field() {
    // (replace, with, feature, parameter)
    let cases: &[(&str, &str, Option<&str>, Option<&str>)] = &[
        ("\"silico\": 1", "\"silico\": \"1\"", None, Some("silico")),
        ("\"silico\": 1", "\"silico\": true", None, Some("silico")),
        ("\"silico\": 1", "\"silico\": 1.0", None, Some("silico")),
        (
            "\"length\": \"mm\"",
            "\"length\": 1",
            None,
            Some("units.length"),
        ),
        (
            "\"position\": [0, 0, 0]",
            "\"position\": {\"x\": 0}",
            Some("k7f2qx3m"),
            Some("position"),
        ),
        (
            "\"position\": [0, 0, 0]",
            "\"position\": [0, 0, 0, 0]",
            Some("k7f2qx3m"),
            Some("position"),
        ),
        (
            "\"position\": [0, 0, 0]",
            "\"position\": []",
            Some("k7f2qx3m"),
            Some("position"),
        ),
        (
            "\"position\": [0, 0, 0]",
            "\"position\": [0, [0], 0]",
            Some("k7f2qx3m"),
            Some("position.1"),
        ),
        (
            "\"position\": [0, 0, 0]",
            "\"position\": [0, 0, null]",
            Some("k7f2qx3m"),
            Some("position.2"),
        ),
        (
            "\"id\": \"k7f2qx3m\"",
            "\"id\": 12345678",
            Some("features[0]"),
            Some("id"),
        ),
        (
            "\"id\": \"k7f2qx3m\"",
            "\"id\": \"k7f2qx3\"",
            Some("features[0]"),
            Some("id"),
        ),
        (
            "\"id\": \"k7f2qx3m\"",
            "\"id\": \"k7f2qx3mm\"",
            Some("features[0]"),
            Some("id"),
        ),
        (
            "\"id\": \"k7f2qx3m\"",
            "\"id\": \"k7f2qx3\u{1d5c6}\"",
            Some("features[0]"),
            Some("id"),
        ),
        (
            "\"type\": \"box\"",
            "\"type\": \"Box\"",
            Some("k7f2qx3m"),
            Some("type"),
        ),
        (
            "\"type\": \"box\"",
            "\"type\": \"box \"",
            Some("k7f2qx3m"),
            Some("type"),
        ),
        (
            "\"type\": \"box\"",
            "\"type\": [\"box\"]",
            Some("k7f2qx3m"),
            Some("type"),
        ),
        (
            "\"type\": \"cut\"",
            "\"type\": \"box\"",
            Some("m3tq8v0z"),
            Some("position"),
        ),
        (
            "\"type\": \"box\"",
            "\"type\": \"fuse\"",
            Some("k7f2qx3m"),
            Some("base"),
        ),
        (
            "\"name\": \"Pin\"",
            "\"name\": 7",
            Some("a91cw0de"),
            Some("name"),
        ),
        (
            "\"base\": \"k7f2qx3m\"",
            "\"base\": null",
            Some("m3tq8v0z"),
            Some("base"),
        ),
        (
            "\"note\": \"Clearance for an M5 bolt\"",
            "\"note\": [\"x\"]",
            Some("m3tq8v0z"),
            Some("note"),
        ),
    ];
    let err =
        refused(r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": {}}"#);
    assert_eq!(err.parameter(), Some("features"), "{err}");
    for &(from, to, feature, parameter) in cases {
        let err = refused(&example_with(from, to));
        assert_eq!(err.feature(), feature, "{to}: {err}");
        assert_eq!(err.parameter(), parameter, "{to}: {err}");
        assert!(err.message().contains("at line "), "{to}: {err}");
    }
}

#[test]
fn units_of_the_wrong_kind_are_refused_naming_units() {
    // Once: `"units": "mm"` is refused with no parameter at all ("units is a string, not
    // an object"), while every other field of the wrong kind names itself: `Fields::of`
    // takes no parameter, and `units` is the only top-level object read through it.
    for units in ["\"mm\"", "null", "[\"mm\", \"1nm\"]", "1"] {
        let err = refused(&example_with(
            "{\"length\": \"mm\", \"resolution\": \"1nm\"}",
            units,
        ));
        assert_eq!(err.parameter(), Some("units"), "{units}: {err}");
    }
}

#[test]
fn a_feature_that_isnt_an_object_is_refused_naming_its_place() {
    for (item, kind) in [
        ("null", "null"),
        ("[]", "an array"),
        ("\"box\"", "a string"),
        ("0", "a number"),
    ] {
        let err = refused(&doc(&[
            r#"{"id": "k7f2qx3m", "type": "sphere", "v": 1, "name": "S", "position": [0, 0, 0], "radius": 1}"#,
            item,
        ]));
        assert_eq!(err.feature(), Some("features[1]"), "{item}: {err}");
        assert!(err.message().contains(kind), "{item}: {err}");
    }
}

#[test]
fn a_feature_written_with_its_fields_out_of_order_and_respaced_is_canonical() {
    let text = "{\"features\":[{\"height\":1,\"width\":1,\"length\":1,\"position\":[0,0,0],\"name\":\"B\",\
                \"v\":1,\"type\":\"box\",\"id\":\"k7f2qx3m\"}],\"units\":{\"resolution\":\"1nm\",\"length\":\"mm\"},\"silico\":1}";
    let want = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"features\": [\n    \
                {\"id\": \"k7f2qx3m\", \"type\": \"box\", \"v\": 1, \"name\": \"B\", \"position\": [0, 0, 0], \"length\": 1, \"width\": 1, \"height\": 1}\n  ]\n}\n";
    assert_eq!(write(&read(text.as_bytes()).unwrap()).unwrap(), want);
}

#[test]
fn whitespace_json_doesnt_allow_is_refused() {
    for ws in ["\u{b}", "\u{c}", "\u{85}", "\u{3000}"] {
        let text = format!("{{\"silico\":{ws}1}}");
        let err = refused(&text);
        assert!(err.message().contains("expected a value"), "{ws:?}: {err}");
    }
    // Every whitespace JSON allows, everywhere it may go, is read.
    let spaced = EXAMPLE
        .replace(", ", " \t\r\n,\r\n\t ")
        .replace(": ", "\r\n:\t");
    assert_eq!(write(&read(spaced.as_bytes()).unwrap()).unwrap(), EXAMPLE);
}

#[test]
fn nesting_inside_an_unknown_key_at_the_limit_is_an_unknown_key_past_it_a_depth_error() {
    // The document is depth 1; an unknown key may hold 63 more levels.
    let nested = |k: usize| {
        format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [], "x": {}{}}}"#,
            "[".repeat(k),
            "]".repeat(k)
        )
    };
    let err = refused(&nested(63));
    assert_eq!(err.parameter(), Some("x"), "{err}");
    let err = refused(&nested(64));
    assert!(err.message().contains("nest more than 64"), "{err}");
}

#[test]
fn a_hundred_thousand_features_read_and_write_promptly() {
    let feature = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#;
    let features = vec![feature; 100_000];
    let text = doc(&features);
    let (n, written) = within(Duration::from_secs(20), move || {
        let document = read(text.as_bytes()).unwrap();
        (document.features.len(), write(&document).unwrap())
    });
    assert_eq!(n, 100_000);
    assert_eq!(written.lines().count(), 100_000 + 6);
}

#[test]
fn an_unknown_key_holding_a_million_items_is_refused_promptly() {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [], "x": [{}0]}}"#,
        "0,".repeat(1_000_000)
    );
    let err = within(Duration::from_secs(20), move || {
        read(text.as_bytes()).unwrap_err()
    });
    assert_eq!(err.parameter(), Some("x"), "{err}");
}

#[test]
fn an_object_with_a_hundred_thousand_keys_is_refused_promptly() {
    let keys: Vec<String> = (0..100_000).map(|i| format!("\"k{i:06}\": 0")).collect();
    let text = format!("{{{}, \"silico\": 1}}", keys.join(", "));
    let err = within(Duration::from_secs(20), move || {
        read(text.as_bytes()).unwrap_err()
    });
    // `silico` and `units` are read first: `units` is missing.
    assert_eq!(err.parameter(), Some("units"), "{err}");
}

#[test]
fn duplicate_keys_that_differ_only_after_a_long_shared_prefix_are_found_promptly() {
    let prefix = "p".repeat(100_000);
    let text = format!("{{\"{prefix}a\": 0, \"{prefix}b\": 0, \"{prefix}a\": 0}}");
    let err = within(Duration::from_secs(10), move || {
        json::read(text.as_bytes()).unwrap_err()
    });
    assert!(err.message.contains("written twice"));
}

// ---------------------------------------------------------- writer invariants

#[test]
fn a_feature_with_an_empty_name_cant_be_made_to_be_saved() {
    // Once: `write` wrote any `Document`, but `read` refuses an empty name, so a feature
    // renamed to "" saved without complaint, and the file then wouldn't open (FreeCAD
    // 7e11c7bd50 is the same failure: a save that returns as if done). Now the type can't
    // hold an empty name, so what's in memory is what the file can hold.
    assert_eq!(Name::new(""), None);
    assert_eq!(
        Name::new(" ").map(|n| n.as_str().to_owned()),
        Some(" ".to_owned())
    );
    let document = one_feature(a_box("k7f2qx3m", "B"));
    let text = write(&document).unwrap();
    assert_eq!(read(text.as_bytes()).ok(), Some(document), "{text}");
}

#[test]
fn a_features_version_is_written_as_the_one_the_reader_reads() {
    // Once: `Feature::version` was any u32, and `write` wrote it; `read` refuses every
    // version but 1. Now it's `FeatureVersion`, which holds only 1 until a type has more.
    let text = write(&one_feature(a_box("k7f2qx3m", "B"))).unwrap();
    assert!(text.contains("\"v\": 1,"), "{text}");
}

// ------------------------------------------------------------ property tests

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x0b7e_ac17),
        ..ProptestConfig::default()
    }
}

/// i64 values biased to the edges: the bounds, zero, powers of ten and one either side.
fn edge_i64() -> impl Strategy<Value = i64> {
    let mut edges = vec![i64::MIN, i64::MIN + 1, i64::MAX, i64::MAX - 1, 0, 1, -1];
    for k in 0..19 {
        let p = 10i64.pow(k);
        edges.extend([p, p - 1, p + 1, -p, -(p - 1), -(p + 1)]);
    }
    prop_oneof![prop::sample::select(edges), any::<i64>()]
}

/// The value of a plain decimal in units of 10^-`decimals`, computed by padding the text
/// (independent of `parse_number`'s digit-by-digit accumulation), or `None` when the
/// text has a non-zero digit past the resolution or the value doesn't fit an i64.
fn reference_value(negative: bool, whole: &str, fraction: &str, decimals: usize) -> Option<i64> {
    let (kept, dropped) = if fraction.len() > decimals {
        fraction.split_at(decimals)
    } else {
        (fraction, "")
    };
    if dropped.chars().any(|c| c != '0') {
        return None;
    }
    let digits = format!("{whole}{kept:0<decimals$}");
    let digits = digits.trim_start_matches('0');
    let signed = if negative {
        format!("-{digits}")
    } else {
        digits.to_owned()
    };
    if digits.is_empty() {
        return Some(0);
    }
    signed.parse::<i64>().ok()
}

/// Strategy: a plain decimal's parts, digits biased toward 0 and 9.
fn decimal_parts() -> impl Strategy<Value = (bool, String, String)> {
    let digit = prop_oneof![3 => Just('0'), 3 => Just('9'), 4 => prop::char::range('0', '9')];
    let whole = prop_oneof![
        Just("0".to_owned()),
        (
            prop::char::range('1', '9'),
            prop::collection::vec(digit.clone(), 0..22)
        )
            .prop_map(|(first, rest)| std::iter::once(first).chain(rest).collect()),
    ];
    let fraction = prop::collection::vec(digit, 0..16).prop_map(|v| v.into_iter().collect());
    (any::<bool>(), whole, fraction)
}

fn any_text() -> impl Strategy<Value = String> {
    // Any characters, controls and noncharacters too, weighted toward the awkward ones.
    let awkward = prop::sample::select(vec![
        '\0',
        '\u{1}',
        '\u{1f}',
        '"',
        '\\',
        '/',
        '\n',
        '\r',
        '\t',
        '\u{7f}',
        '\u{85}',
        '\u{a0}',
        '\u{2028}',
        '\u{2029}',
        '\u{feff}',
        '\u{fffe}',
        '\u{ffff}',
        '\u{10ffff}',
        '\u{d7ff}',
        '\u{e000}',
        '\u{301}',
        '\u{202e}',
    ]);
    prop::collection::vec(prop_oneof![awkward, any::<char>()], 0..24)
        .prop_map(|chars| chars.into_iter().collect())
}

fn any_length() -> impl Strategy<Value = Length> {
    edge_i64().prop_map(Length::from_nm)
}

fn any_id() -> impl Strategy<Value = FeatureId> {
    let alphabet: Vec<char> = silico_core::document::ID_ALPHABET.chars().collect();
    prop::collection::vec(prop::sample::select(alphabet), 8)
        .prop_map(|chars| FeatureId::new(&chars.into_iter().collect::<String>()).unwrap())
}

fn any_operation() -> impl Strategy<Value = Operation> {
    let p = || [any_length(), any_length(), any_length()];
    prop_oneof![
        (p(), any_length(), any_length(), any_length()).prop_map(
            |(position, length, width, height)| {
                Operation::Box {
                    position,
                    length,
                    width,
                    height,
                }
            }
        ),
        (p(), any_length(), any_length()).prop_map(|(position, radius, height)| {
            Operation::Cylinder {
                position,
                radius,
                height,
            }
        }),
        (p(), any_length()).prop_map(|(position, radius)| Operation::Sphere { position, radius }),
        (p(), any_length(), any_length(), any_length()).prop_map(
            |(position, radius1, radius2, height)| Operation::Cone {
                position,
                radius1,
                radius2,
                height
            }
        ),
        (p(), any_length(), any_length()).prop_map(|(position, radius1, radius2)| {
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

fn any_document() -> impl Strategy<Value = Document> {
    let feature = (
        any_id(),
        any_operation(),
        any_text().prop_filter("a name isn't empty", |s| !s.is_empty()),
        prop::option::of(any_text()),
    )
        .prop_map(|(id, operation, name, note)| Feature {
            id,
            operation,
            version: FeatureVersion,
            name: Name::new(name).unwrap(),
            note,
            fingerprint: None,
        });
    prop::collection::vec(feature, 0..5).prop_map(|features| Document {
        silico: FormatVersion,
        units: Units,
        features,
    })
}

/// Our reader's tree as serde_json's, numbers through serde_json's own parsing.
fn as_serde(node: &json::Node) -> serde_json::Value {
    match &node.value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Number(text) => serde_json::from_str(text).unwrap(),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Array(items) => serde_json::Value::Array(items.iter().map(as_serde).collect()),
        Value::Object(members) => serde_json::Value::Object(
            members
                .iter()
                .map(|m| (m.key.clone(), as_serde(&m.value)))
                .collect(),
        ),
    }
}

/// Text from JSON's tokens and a few that aren't, glued at random.
fn token_soup() -> impl Strategy<Value = String> {
    let tokens = vec![
        "{", "}", "[", "]", ":", ",", " ", "\n", "\"", "\"a\"", "\"a", "\\", "\\u", "\\ud800",
        "\\udc00", "\\n", "\\x", "0", "1", "-", ".", "e", "E", "+", "01", "-0", "1.5", "true",
        "fals", "null", "\u{a0}", "\u{feff}", "\u{2028}", "\u{0}", "\u{1f}", "é", "😀", "/", "//",
        "'", "NaN", "\r",
    ];
    prop::collection::vec(prop::sample::select(tokens), 0..30).prop_map(|t| t.concat())
}

/// A valid JSON value's text, with random whitespace, and its serde_json value.
fn any_json() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        Just("null".to_owned()),
        Just("true".to_owned()),
        Just("false".to_owned()),
        any::<i64>().prop_map(|n| n.to_string()),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(|f| format!("{f:e}")),
        any_text().prop_map(|s| serde_json::to_string(&s).unwrap()),
        any_text().prop_map(|s| {
            // Every character escaped as \uXXXX (surrogate pairs past the BMP).
            let mut out = String::from("\"");
            for u in s.encode_utf16() {
                out.push_str(&format!("\\u{u:04X}"));
            }
            out.push('"');
            out
        }),
    ];
    leaf.prop_recursive(6, 64, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6)
                .prop_map(|v| format!("[ {} ]", v.join(" ,\n"))),
            prop::collection::btree_map(any_text(), inner, 0..6).prop_map(|m| {
                let members: Vec<String> = m
                    .into_iter()
                    .map(|(k, v)| format!("{}\t:\r\n{v}", serde_json::to_string(&k).unwrap()))
                    .collect();
                format!("{{{}}}", members.join(","))
            }),
        ]
    })
}

proptest! {
    #![proptest_config(config(2_000))]

    /// Any plain decimal reads as the value a hand calculation gives, or is refused as too
    /// precise or too large, never misread.
    #[test]
    fn any_plain_decimal_reads_as_its_exact_value((negative, whole, fraction) in decimal_parts()) {
        let text = format!(
            "{}{whole}{}{fraction}",
            if negative { "-" } else { "" },
            if fraction.is_empty() { "" } else { "." }
        );
        for (decimals, parsed) in [
            (6usize, Length::parse(&text).map(Length::nm)),
            (9, Angle::parse(&text).map(Angle::ndeg)),
        ] {
            let want = reference_value(negative, &whole, &fraction, decimals);
            match (want, parsed) {
                (Some(0), Err(NumberError::NegativeZero)) => prop_assert!(negative),
                (Some(v), Ok(got)) => prop_assert_eq!(v, got, "{}", text),
                (None, Err(NumberError::TooPrecise { .. } | NumberError::OutOfRange)) => {}
                (want, got) => prop_assert!(false, "{} at {}: want {:?}, got {:?}", text, decimals, want, got),
            }
        }
    }

    /// Any i64 writes as a decimal that reads back as it, in both quantities, and the text
    /// is the shortest: no trailing zero after a point, no point without a fraction.
    #[test]
    fn any_value_writes_as_its_shortest_decimal(n in edge_i64()) {
        let length = Length::from_nm(n).to_string();
        prop_assert_eq!(Length::parse(&length), Ok(Length::from_nm(n)));
        let angle = Angle::from_ndeg(n).to_string();
        prop_assert_eq!(Angle::parse(&angle), Ok(Angle::from_ndeg(n)));
        for text in [&length, &angle] {
            prop_assert!(!text.ends_with('.') && !(text.contains('.') && text.ends_with('0')), "{}", text);
            prop_assert!(!text.starts_with("-0") || text.starts_with("-0."), "{}", text);
        }
    }

    /// Any document with edge-value lengths and names of any characters writes, reads back
    /// as itself, and writes the same bytes again.
    #[test]
    fn any_hostile_document_round_trips_byte_for_byte(document in any_document()) {
        let text = write(&document).unwrap();
        let back = read(text.as_bytes());
        prop_assert!(back.is_ok(), "{:?}\n{}", back, text);
        let back = back.unwrap();
        prop_assert_eq!(&back, &document);
        prop_assert_eq!(write(&back).unwrap(), text.clone());
        // One feature per line, LF only, a trailing newline.
        let n = document.features.len();
        prop_assert_eq!(text.lines().count(), if n == 0 { 5 } else { 6 + n });
        let canonical = !text.contains('\r') && text.ends_with("}\n");
        prop_assert!(canonical);
    }

    /// Valid JSON with random whitespace and escapes reads as serde_json reads it.
    #[test]
    fn valid_json_reads_as_an_independent_reader_reads_it(text in any_json()) {
        let ours = json::read(text.as_bytes());
        let theirs: serde_json::Value = serde_json::from_str(&text).unwrap();
        prop_assert!(ours.is_ok(), "{:?}\n{}", ours, text);
        prop_assert_eq!(as_serde(&ours.unwrap()), theirs);
    }

    /// Token soup: whatever serde_json refuses, ours refuses; whatever ours refuses that
    /// serde_json reads is one of Silico's stated extra strictnesses.
    #[test]
    fn json_soup_is_refused_whenever_an_independent_reader_refuses_it(text in token_soup()) {
        let ours = json::read(text.as_bytes());
        let theirs = serde_json::from_str::<serde_json::Value>(&text);
        match (&ours, &theirs) {
            (Ok(node), Ok(value)) => prop_assert_eq!(&as_serde(node), value),
            (Ok(_), Err(e)) => prop_assert!(false, "{:?}: ours read it, serde_json: {}", text, e),
            (Err(e), Ok(_)) => prop_assert!(
                e.message.contains("written twice") || e.message.contains("byte-order mark") || e.message.contains("nest more"),
                "{:?}: refused only by ours: {:?}", text, e
            ),
            (Err(_), Err(_)) => {}
        }
    }

    /// Any bytes: read or refused within a second, and if read, canonical and stable.
    #[test]
    fn any_bytes_near_a_document_are_read_or_refused_and_canonical_if_read(
        start in 0usize..EXAMPLE.len(),
        cut in 0usize..8,
        insert in token_soup(),
    ) {
        let mut text = EXAMPLE.as_bytes().to_vec();
        let end = (start + cut).min(text.len());
        text.splice(start..end, insert.into_bytes());
        if let Ok(document) = read(&text) {
            let once = write(&document).unwrap();
            let back = read(once.as_bytes()).unwrap();
            prop_assert_eq!(&back, &document);
            prop_assert_eq!(write(&back).unwrap(), once);
        }
    }
}
