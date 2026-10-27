//! break-it: STEP's string encoding (`step_encoded`, `step_decoded`, `is_step_encoded`) and
//! `ExportStep::check`, with hostile text: every string a document can hold through the
//! encoder, and strings built from the pieces of directives (backslashes, `X`, `0`, `2`,
//! `4`, hex digits of either case, apostrophes, non-ASCII) through the decoder and the form
//! check. Expected values are ISO 10303-21's own (§6.4.3.3), worked by hand: a `\X2\` run
//! is four uppercase hex digits a UCS-2 character, `\X4\` eight, closed by `\X0\`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use silico_protocol::{
    Blob, ExportStep, ShapeData, StepPart, is_step_encoded, step_decoded, step_encoded,
};

/// A decoder written from ISO 10303-21's rules alone, for comparison: the text, or none if
/// any piece isn't printable ASCII outside a run, a run isn't closed or holds anything but
/// whole groups of uppercase hex, or a value isn't a scalar value.
fn by_the_standard(encoded: &str) -> Option<String> {
    let bytes = encoded.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' {
            if !(0x20..=0x7E).contains(&b) {
                return None;
            }
            out.push(b as char);
            i += 1;
            continue;
        }
        let width = match bytes.get(i..i + 4) {
            Some(b"\\X2\\") => 4,
            Some(b"\\X4\\") => 8,
            _ => return None,
        };
        i += 4;
        let mut groups = 0;
        loop {
            if bytes.get(i..i + 4) == Some(b"\\X0\\") {
                i += 4;
                break;
            }
            let group = bytes.get(i..i + width)?;
            if !group
                .iter()
                .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(c))
            {
                return None;
            }
            let value = u32::from_str_radix(std::str::from_utf8(group).ok()?, 16).ok()?;
            out.push(char::from_u32(value)?);
            groups += 1;
            i += width;
        }
        if groups == 0 {
            return None;
        }
    }
    Some(out)
}

/// Text from the pieces directives are made of, so the decoder meets near-misses.
fn pieces() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("\\".to_owned()),
            Just("\\X2\\".to_owned()),
            Just("\\X4\\".to_owned()),
            Just("\\X0\\".to_owned()),
            Just("\\X\\".to_owned()),
            Just("\\N\\".to_owned()),
            Just("X".to_owned()),
            Just("0".to_owned()),
            Just("2".to_owned()),
            Just("00FC".to_owned()),
            Just("00fc".to_owned()),
            Just("D800".to_owned()),
            Just("0001F600".to_owned()),
            Just("0010FFFF".to_owned()),
            Just("00110000".to_owned()),
            Just("0041".to_owned()),
            Just("'".to_owned()),
            Just("a".to_owned()),
            Just("ü".to_owned()),
            Just("\u{0}".to_owned()),
            "[0-9A-Fa-f]{1,9}",
        ],
        0..12,
    )
    .prop_map(|parts| parts.concat())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 20_000, ..ProptestConfig::default() })]

    #[test]
    fn any_text_encodes_to_printable_ascii_that_decodes_to_it_and_is_in_form(text in any::<String>()) {
        let encoded = step_encoded(&text);
        prop_assert!(encoded.bytes().all(|b| (0x20..=0x7E).contains(&b)), "{}", encoded);
        let decoded = step_decoded(&encoded);
        prop_assert_eq!(decoded.as_deref(), Some(text.as_str()));
        let standard = by_the_standard(&encoded);
        prop_assert_eq!(standard.as_deref(), Some(text.as_str()));
        prop_assert!(is_step_encoded(&encoded));
        // No raw backslash outside a directive: every `\` begins `\X2\`, `\X4\`, or `\X0\`.
        for (at, _) in encoded.match_indices('\\') {
            let rest = &encoded[at..];
            let directive = ["\\X2\\", "\\X4\\", "\\X0\\"].iter().any(|d| rest.starts_with(d));
            let closing = encoded[..at].ends_with("\\X2") || encoded[..at].ends_with("\\X4")
                || encoded[..at].ends_with("\\X0");
            prop_assert!(directive || closing, "{}", encoded);
        }
    }

}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4_000, ..ProptestConfig::default() })]

    #[test]
    fn hostile_text_decodes_as_the_standard_reads_it_or_not_at_all(text in pieces()) {
        let decoded = step_decoded(&text);
        let standard = by_the_standard(&text);
        // The decoder reads what the standard reads, or refuses: never reads otherwise.
        if let Some(d) = &decoded {
            prop_assert_eq!(Some(d), standard.as_ref(), "{:?}", text);
        }
        // The form check is exactly "the encoder's own output".
        prop_assert_eq!(
            is_step_encoded(&text),
            decoded.as_ref().is_some_and(|d| step_encoded(d) == text),
            "{:?}", text
        );
        if is_step_encoded(&text) {
            // A string in form is all printable ASCII, so OCCT changes it only by doubling `'`.
            prop_assert!(text.bytes().all(|b| (0x20..=0x7E).contains(&b)), "{:?}", text);
        }
    }

    #[test]
    fn any_byte_soup_never_panics_the_decoder(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        let text = String::from_utf8_lossy(&bytes);
        let _ = step_decoded(&text);
        let _ = is_step_encoded(&text);
    }
}

#[test]
fn a_run_whose_hex_is_split_by_a_line_end_is_not_in_form() {
    // What OCCT's writer makes of a long run (it wraps at 72 columns): never accepted as
    // a string to write, and never read as the text it was.
    for text in [
        "\\X2\\00\nFC\\X0\\",
        "\\X2\\00FC\\\nX0\\",
        "\\\nX2\\00FC\\X0\\",
    ] {
        assert!(!is_step_encoded(text), "{:?}", text);
        assert_eq!(step_decoded(text), None, "{:?}", text);
    }
}

#[test]
fn a_check_of_a_huge_export_is_linear_and_refuses_the_first_bad_field() {
    // A hundred parts of 10 KiB names each, the last name not in form: refused naming
    // `name`, within seconds (linear in the request's size).
    let good = step_encoded(&"ü".repeat(10 << 10));
    let mut parts: Vec<StepPart> = (0..100)
        .map(|n| StepPart {
            shape: ShapeData::unchecked(Blob::inline(vec![1])),
            id: format!("f{n:07}"),
            name: good.clone(),
        })
        .collect();
    parts.last_mut().unwrap().name.push('\u{F6}');
    let export = ExportStep {
        parts,
        assembly: "a".to_owned(),
        file_name: "a.silico".to_owned(),
    };
    let start = std::time::Instant::now();
    let refused = export.check().unwrap_err();
    assert_eq!(refused.parameter, "name");
    assert!(start.elapsed().as_secs_f64() < 5.0, "{:?}", start.elapsed());
}
