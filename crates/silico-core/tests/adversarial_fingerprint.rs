//! /break-it on M2.6 fingerprints: hostile fingerprints in a file, measures at the edges of
//! their rounding and range, documents the gate admits whose solids can't be fingerprinted,
//! drift against loose spellings, and a recompute superseded between a step and its
//! summary. Expected values come from an exact rounding written here from an f64's bits (no
//! formatting), from hand calculation (a primitive's formulas), or from the documented
//! contract (ARCHITECTURE.md, the module docs), never from the code under test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/model.rs"]
mod model;

use std::f64::consts::PI;

use model::{WAIT, checked, described, id, made, published};
use proptest::prelude::*;
use silico_core::document::{
    Checked, Document, Feature, FeatureId, FeatureVersion, Fingerprint, FormatVersion, Length,
    Measure, Name, NamingScheme, Operation, Units, read, schema, write,
};
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;
use silico_core::recompute::{Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};

const HEAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": ["#;

fn doc(features: &[&str]) -> String {
    format!("{HEAD}{}]}}", features.join(", "))
}

const BLOCK: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10"#;
const GOOD: &str = r#""volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8"#;

/// The block with `fingerprint` as its fingerprint's text (the object, braces and all).
fn block_with(fingerprint: &str) -> String {
    doc(&[&format!(r#"{BLOCK}, "fingerprint": {fingerprint}}}"#)])
}

fn refused(text: &str) -> Error {
    let err = read(text.as_bytes()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(err.suggestion().is_some(), "{err}");
    err
}

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap())
}

fn rebuilt(text: &str) -> (Document, std::sync::Arc<silico_core::recompute::Model>) {
    let document = read(text.as_bytes()).unwrap();
    let model = published(&engine(), Checked::new(document.clone()).unwrap());
    (document, model)
}

// ---------------------------------------------------------------------------------------
// Rounding, against an exact rounding from the float's bits.

/// `value` × 10^6 rounded to the nearest whole number, ties to even, computed exactly from
/// the float's sign, exponent, and significand in integers; none if it isn't finite or the
/// result isn't an i128. A result of zero is zero whatever the sign.
fn exact_millionths(value: f64) -> Option<i128> {
    if !value.is_finite() {
        return None;
    }
    let bits = value.to_bits();
    let negative = bits >> 63 == 1;
    let biased = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    let (significand, exponent) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), biased - 1075)
    };
    let n = u128::from(significand) * 1_000_000;
    let magnitude: u128 = if exponent >= 0 {
        if n == 0 {
            return Some(0);
        }
        // n < 2^(128 - z), at least 2^(127 - z): shifted by e, under 2^127 (an i128)
        // exactly when e < z.
        let e = exponent as u32;
        if e >= n.leading_zeros() {
            return None;
        }
        n << e
    } else {
        let k = (-exponent) as u32;
        if k >= 120 {
            0 // n < 2^74, so n / 2^k < 1/2: rounds to zero
        } else {
            let q = n >> k;
            let r = n & ((1u128 << k) - 1);
            let half = 1u128 << (k - 1);
            if r > half || (r == half && q & 1 == 1) {
                q + 1
            } else {
                q
            }
        }
    };
    if magnitude == 0 {
        return Some(0);
    }
    let signed = i128::try_from(magnitude).ok()?;
    Some(if negative { -signed } else { signed })
}

#[test]
fn the_exact_rounding_here_agrees_with_hand_worked_values() {
    // The oracle itself, against values worked by hand.
    assert_eq!(exact_millionths(0.0078125), Some(7_812)); // 7812.5: tie, to even
    assert_eq!(exact_millionths(0.0234375), Some(23_438)); // 23437.5: tie, to even
    assert_eq!(exact_millionths(-0.0078125), Some(-7_812));
    assert_eq!(exact_millionths(8000.0), Some(8_000_000_000));
    assert_eq!(exact_millionths(-1e-12), Some(0));
    assert_eq!(exact_millionths(f64::MIN_POSITIVE), Some(0));
    assert_eq!(exact_millionths(1e13), Some(10_000_000_000_000_000_000));
    // 2^127 millionths and up don't fit (i128::MAX as f64 is 2^127); half that does.
    assert_eq!(exact_millionths(i128::MAX as f64 / 1e6 * 1.0000001), None);
    assert!(exact_millionths(i128::MAX as f64 / 2e6).is_some());
    assert_eq!(exact_millionths(f64::NAN), None);
}

fn edge_value() -> impl Strategy<Value = f64> {
    let tie = (any::<i32>()).prop_map(|m| f64::from(m | 1) / 128.0);
    let near_tie = (any::<i32>(), prop::bool::ANY).prop_map(|(m, up)| {
        let t = f64::from(m | 1) / 128.0;
        if up { t.next_up() } else { t.next_down() }
    });
    // Odd multiples of 2^-7 times a power of two above: ties far from zero.
    let far_tie = (1i64..(1 << 52)).prop_map(|m| (m | 1) as f64 / 128.0);
    let limit = i128::MAX as f64 / 1e6;
    let range_edge = (0u32..64, prop::bool::ANY).prop_map(move |(steps, negative)| {
        let mut v = limit;
        for _ in 0..steps / 2 {
            v = v.next_down();
        }
        for _ in 0..steps % 8 {
            v = v.next_up();
        }
        if negative { -v } else { v }
    });
    let half_millionth = (0u32..8, prop::bool::ANY).prop_map(|(steps, negative)| {
        let mut v = 5e-7f64;
        for _ in 0..steps {
            v = if steps % 2 == 0 {
                v.next_up()
            } else {
                v.next_down()
            };
        }
        if negative { -v } else { v }
    });
    let bits = any::<u64>().prop_map(f64::from_bits);
    let plain = -1e7f64..1e7;
    prop_oneof![
        tie,
        near_tie,
        far_tie,
        range_edge,
        half_millionth,
        bits,
        plain,
        Just(0.0),
        Just(-0.0),
        Just(f64::MIN_POSITIVE),
        Just(-f64::MIN_POSITIVE),
        Just(5e-324),
        Just(f64::MAX),
        Just(f64::MIN),
        Just(f64::NAN),
        Just(f64::INFINITY),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 20_000,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x6_f1_9e),
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_measure_rounds_as_exact_arithmetic_does(value in edge_value()) {
        let got = Measure::rounded(value).map(Measure::millionths);
        prop_assert_eq!(got, exact_millionths(value), "{:e}", value);
        // And it writes text that reads back as itself.
        if let Some(m) = Measure::rounded(value) {
            prop_assert_eq!(Measure::parse(&m.to_string()), Ok(m));
        }
    }
}

// ---------------------------------------------------------------------------------------
// Every fingerprint the type holds is saved and read back as itself.

fn any_fingerprint() -> impl Strategy<Value = Fingerprint> {
    let measure = prop_oneof![
        Just(i128::MIN + 1),
        Just(i128::MAX),
        Just(0i128),
        Just(-1i128),
        Just(1i128),
        any::<i128>().prop_filter("i128::MIN has no magnitude", |&m| m != i128::MIN),
        any::<i64>().prop_map(i128::from)
    ];
    let wide = prop_oneof![
        Just(i64::MIN),
        Just(i64::MAX),
        Just(0i64),
        Just(-1i64),
        Just(1i64),
        any::<i64>()
    ];
    let count = prop_oneof![Just(0u32), Just(u32::MAX), any::<u32>()];
    (
        measure.clone(),
        measure,
        prop::option::of(prop::array::uniform6(wide)),
        count.clone(),
        count.clone(),
        count,
    )
        .prop_map(|(volume, area, bbox, faces, edges, vertices)| Fingerprint {
            volume: Measure::from_millionths(volume),
            area: Measure::from_millionths(area),
            bbox: bbox.map(|b| b.map(Length::from_nm)),
            faces,
            edges,
            vertices,
        })
}

fn with_fingerprint(fingerprint: Option<Fingerprint>) -> Document {
    Document {
        silico: FormatVersion,
        units: Units,
        naming: NamingScheme,
        features: vec![Feature {
            id: FeatureId::new("k7f2qx3m").unwrap(),
            operation: Operation::Sphere {
                position: [Length::from_nm(0); 3],
                radius: Length::from_nm(1_000_000),
            },
            version: FeatureVersion,
            name: Name::new("S").unwrap(),
            note: None,
            fingerprint,
        }],
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2_000,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x6_f1_9f),
        ..ProptestConfig::default()
    })]

    #[test]
    fn every_fingerprint_the_type_holds_reads_back_as_written(fingerprint in any_fingerprint()) {
        let document = with_fingerprint(Some(fingerprint));
        let text = write(&document).unwrap();
        prop_assert_eq!(read(text.as_bytes()).unwrap(), document, "{}", text);
    }
}

// ---------------------------------------------------------------------------------------
// Hostile fingerprints in a file.

#[test]
fn a_fingerprint_spelled_loosely_reads_as_the_canonical_one_and_isnt_drift() {
    let loose = r#"{"vertices": 8, "edges": 1.2e1, "faces": 6, "bbox": [0e5, 0.0, -0, 4e1, 2.000000e1, 100e-1], "area": 2.8E3, "volume": 8000.000000}"#;
    // -0 in the box: zero written with a minus sign, as for every length (the box is read
    // before the counts).
    let err = refused(&block_with(loose));
    assert_eq!(err.parameter(), Some("fingerprint.bbox.2"), "{err}");
    let loose = loose.replace("-0,", "0,");
    // Counts are whole numbers without an exponent, as the format's other counts are.
    let err = refused(&block_with(&loose));
    assert_eq!(err.parameter(), Some("fingerprint.edges"), "{err}");
    let loose = loose.replace("1.2e1", "12");
    let (document, model) = rebuilt(&block_with(&loose));
    assert!(model.drift().is_empty(), "{:?}", model.drift());
    let canonical = write(&document).unwrap();
    assert!(canonical.contains(GOOD), "{canonical}");
}

#[test]
fn a_fingerprint_that_isnt_an_object_is_refused_naming_it() {
    for (value, kind) in [
        ("null", "null"),
        ("true", "true or false"),
        ("8000", "a number"),
        (r#""8000""#, "a string"),
    ] {
        let err = refused(&block_with(value));
        assert_eq!(err.feature(), Some("k7f2qx3m"), "{value}: {err}");
        assert_eq!(err.parameter(), Some("fingerprint"), "{value}: {err}");
        assert!(
            err.message().starts_with(&format!("fingerprint is {kind}")),
            "{value}: {err}"
        );
    }
}

#[test]
fn an_empty_fingerprint_is_refused_naming_the_first_field_it_lacks() {
    let err = refused(&block_with("{}"));
    assert_eq!(err.parameter(), Some("fingerprint.volume"), "{err}");
    assert!(
        err.message().starts_with("fingerprint has no volume"),
        "{err}"
    );
}

#[test]
fn a_fingerprint_field_written_twice_is_refused() {
    // Refused by the JSON reader, before any feature is read: it names the key and place.
    let err = refused(&block_with(&format!(r#"{{{GOOD}, "faces": 6}}"#)));
    assert!(
        err.message()
            .contains(r#"the key "faces" is written twice"#),
        "{err}"
    );
}

#[test]
fn a_box_of_the_wrong_shape_is_refused_naming_it() {
    for (bbox, parameter, says) in [
        (
            "[]",
            "fingerprint.bbox",
            "fingerprint.bbox has 0 values, not 6",
        ),
        (
            "[0, 0, 0, 40, 20, 10, 0]",
            "fingerprint.bbox",
            "fingerprint.bbox has 7 values, not 6",
        ),
        (
            "[[0, 0, 0], [40, 20, 10]]",
            "fingerprint.bbox",
            "fingerprint.bbox has 2 values, not 6",
        ),
        (
            r#"[0, 0, 0, 40, 20, "10"]"#,
            "fingerprint.bbox.5",
            "fingerprint.bbox.5 is a string",
        ),
        ("null", "fingerprint.bbox", "fingerprint.bbox is null"),
        (
            "[0, 0, 0, 40, 20, 9223372036854.775808]",
            "fingerprint.bbox.5",
            "fingerprint.bbox.5 9223372036854.775808 is too large",
        ),
    ] {
        let text = block_with(&format!(
            r#"{{"volume": 8000, "area": 2800, "bbox": {bbox}, "faces": 6, "edges": 12, "vertices": 8}}"#
        ));
        let err = refused(&text);
        assert_eq!(err.feature(), Some("k7f2qx3m"), "{bbox}: {err}");
        assert_eq!(err.parameter(), Some(parameter), "{bbox}: {err}");
        assert!(err.message().starts_with(says), "{bbox}: {err}");
    }
}

#[test]
fn a_huge_number_or_key_in_a_fingerprint_makes_a_short_error() {
    let digits = "9".repeat(1 << 20);
    for fingerprint in [
        format!(r#"{{"volume": {digits}, "area": 2800, "faces": 6, "edges": 12, "vertices": 8}}"#),
        format!(
            r#"{{"volume": 1e{digits}, "area": 2800, "faces": 6, "edges": 12, "vertices": 8}}"#
        ),
        format!(
            r#"{{"volume": 1e-{digits}, "area": 2800, "faces": 6, "edges": 12, "vertices": 8}}"#
        ),
        format!(
            r#"{{"volume": 8000, "area": 2800, "faces": {digits}, "edges": 12, "vertices": 8}}"#
        ),
        format!(r#"{{{GOOD}, "{}": 1}}"#, "k".repeat(1 << 20)),
        format!(
            r#"{{"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, {digits}], "faces": 6, "edges": 12, "vertices": 8}}"#
        ),
    ] {
        let err = refused(&block_with(&fingerprint));
        let size = err.message().len()
            + err.parameter().map_or(0, str::len)
            + err.suggestion().map_or(0, str::len);
        assert!(
            size < 2_000,
            "an error of {size} bytes: {}",
            &err.message()[..200]
        );
        assert_eq!(err.feature(), Some("k7f2qx3m"));
    }
}

#[test]
fn a_key_with_control_characters_in_a_fingerprint_isnt_echoed_raw() {
    let err = refused(&block_with(&format!(r#"{{{GOOD}, "\u001b[31mred\n": 1}}"#)));
    let all = format!(
        "{}{}{}",
        err.message(),
        err.parameter().unwrap_or(""),
        err.suggestion().unwrap_or("")
    );
    assert!(!all.contains('\u{1b}') && !all.contains('\n'), "{all:?}");
}

#[test]
fn a_negative_volume_out_of_range_says_how_small_not_how_large() {
    // Once: the suggestion said "a fingerprint's volume is at most 9223372036854.775807"
    // for a value far below it; a length's says "either way". Measures are i128 millionths
    // now: -2^127 millionths is past them, as -(i128::MAX + 1) has no magnitude in one.
    let err = refused(&block_with(
        r#"{"volume": -170141183460469231731687303715884.105728, "area": 2800, "faces": 6, "edges": 12, "vertices": 8}"#,
    ));
    assert_eq!(err.parameter(), Some("fingerprint.volume"));
    let suggestion = err.suggestion().unwrap();
    assert!(suggestion.contains("either way"), "{suggestion}");
    // Just inside, either way, it reads.
    for volume in [
        "-170141183460469231731687303715884.105727",
        "170141183460469231731687303715884.105727",
    ] {
        let text = block_with(&format!(
            r#"{{"volume": {volume}, "area": 2800, "faces": 6, "edges": 12, "vertices": 8}}"#
        ));
        read(text.as_bytes()).unwrap();
    }
}

/// The schema's verdict on `text`.
fn schema_valid(text: &str) -> bool {
    let schema: serde_json::Value = serde_json::from_str(&schema().unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    validator.is_valid(&serde_json::from_str(text).unwrap())
}

#[test]
fn the_schema_refuses_a_count_past_what_the_reader_takes() {
    // BREAKS: the schema gives faces, edges, and vertices `"format": "uint32"` and
    // `"minimum": 0` but no `"maximum"`, and formats are annotations by default, so an
    // agent checking against the schema is told 4294967296 faces is fine; the reader
    // refuses it as too large.
    // With the header the schema requires (`naming`, M4.0), so the schema refuses each only
    // for its count: the good fingerprint passes the reader and the schema.
    let with_naming = |text: String| text.replacen(r#""units": "#, r#""naming": 5, "units": "#, 1);
    let good = with_naming(block_with(&format!("{{{GOOD}}}")));
    assert!(read(good.as_bytes()).is_ok(), "{good}");
    assert!(schema_valid(&good), "the schema refuses {good}");
    for (field, was) in [("faces", 6), ("edges", 12), ("vertices", 8)] {
        let text = with_naming(block_with(&format!(
            "{{{}}}",
            GOOD.replacen(
                &format!(r#""{field}": {was}"#),
                &format!(r#""{field}": 4294967296"#),
                1
            )
        )));
        assert!(text.contains("4294967296"), "{text}");
        assert!(read(text.as_bytes()).is_err(), "{text}");
        assert!(
            !schema_valid(&text),
            "the schema takes {field} 4294967296: {text}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Documents the gate admits whose solids can't be fingerprinted.

#[test]
fn a_box_the_gate_admits_has_a_fingerprint_whatever_its_size() {
    // BREAKS: a box 200 m on a side (2e5 mm, well inside the gate's kilometre) is 8e15 mm³
    // = 8,000,000 m³, which the docs say a fingerprint holds ("a volume past about 9.2
    // million m³" doesn't fit); but a Measure is millionths of a mm³ in an i64, at most
    // 9.22e12 mm³ = 9,223 m³ (a cube 21 m on a side). The fingerprint is silently none,
    // so a save writes none, and drift is never checked for the part.
    let text = doc(&[
        r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Hall", "position": [0, 0, 0], "length": 200000, "width": 200000, "height": 200000}"#,
    ]);
    let (_, model) = rebuilt(&text);
    let fingerprint = model.fingerprint(&id("k7f2qx3m"));
    let fingerprinted = model.saved();
    assert!(
        fingerprinted.features[0].fingerprint.is_some(),
        "no fingerprint for a valid 200 m box: {fingerprint:?}"
    );
}

#[test]
fn a_sphere_of_radius_13_1_metres_has_a_fingerprint() {
    // BREAKS: as above, at a much smaller part: a ball of radius 13 m (13,000 mm) is
    // 4/3 π 13000³ = 9.203e12 mm³, just fits; radius 13.1 m is 9.417e12 mm³, past i64
    // millionths. Hand calculation: 4/3 × π × 13100³ = 9.4166e12.
    let text = doc(&[
        r#"{"id": "k7f2qx3m", "type": "sphere", "v": 1, "name": "Tank", "position": [0, 0, 0], "radius": 13100}"#,
    ]);
    let (_, model) = rebuilt(&text);
    assert!(model.fingerprint(&id("k7f2qx3m")).is_some());
}

#[test]
fn a_stored_fingerprint_on_a_200_metre_part_is_checked_like_any_other() {
    // Once a part this size (past i64 millionths of a mm³) had no fingerprint to check.
    let text = doc(&[&format!(
        r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Hall", "position": [0, 0, 0], "length": 200000, "width": 200000, "height": 200000, "fingerprint": {{{GOOD}}}}}"#
    )]);
    let (_, model) = rebuilt(&text);
    let errors = model.drift();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let e = &errors[0];
    assert_eq!(e.code(), ErrorCode::Drift);
    assert_eq!(e.feature(), Some("k7f2qx3m"));
    assert_eq!(e.parameter(), Some("fingerprint.volume"));
    // 200000³ and 6 · 200000², exactly.
    assert!(
        e.message().contains("→ 8000000000000000")
            && e.message().contains("area 2800 → 240000000000"),
        "{e}"
    );
    assert!(e.suggestion().is_some());
}

#[test]
fn a_box_at_the_gates_far_corner_has_its_box_to_the_nanometre() {
    // The smallest box the document writes (1 nm sides; the gate wants above 1e-7 mm), placed
    // 1 km out on every axis: its box is 1e6 to 1e6 + 1e-6 to the nanometre (an f64 at 1e6
    // resolves 1.2e-10 mm), and its volume and area round to 0.
    let text = doc(&[
        r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Speck", "position": [1000000, -1000000, 1000000], "length": 0.000001, "width": 0.000001, "height": 0.000001}"#,
    ]);
    let (_, model) = rebuilt(&text);
    let f = model.fingerprint(&id("k7f2qx3m")).unwrap();
    assert_eq!(f.volume, Measure::from_millionths(0));
    assert_eq!(f.area, Measure::from_millionths(0)); // 6e-12 mm²
    assert_eq!(
        f.bbox,
        Some(
            [
                1_000_000_000_000,
                -1_000_000_000_000,
                1_000_000_000_000,
                1_000_000_000_001,
                -999_999_999_999,
                1_000_000_000_001
            ]
            .map(Length::from_nm)
        )
    );
}

// ---------------------------------------------------------------------------------------
// The fake's summaries, by hand.

#[test]
fn the_fakes_primitives_measure_as_their_formulas_where_the_document_puts_them() {
    // Each placed at (1, 2, 3).
    let text = doc(&[
        r#"{"id": "s0000000", "type": "sphere", "v": 1, "name": "S", "position": [1, 2, 3], "radius": 3}"#,
        r#"{"id": "c0000000", "type": "cone", "v": 1, "name": "C", "position": [1, 2, 3], "radius1": 3, "radius2": 0, "height": 4}"#,
        r#"{"id": "d0000000", "type": "cone", "v": 1, "name": "D", "position": [1, 2, 3], "radius1": 1, "radius2": 2, "height": 3}"#,
        r#"{"id": "t0000000", "type": "torus", "v": 1, "name": "T", "position": [1, 2, 3], "radius1": 5, "radius2": 1}"#,
    ]);
    let (_, model) = rebuilt(&text);
    let m = |s: &str| model.fingerprint(&id(s)).unwrap();
    let nm = |v: [i64; 6]| Some(v.map(Length::from_nm));
    // Sphere r 3: 36π = 113.097336 for both.
    let s = m("s0000000");
    assert_eq!(s.volume.to_string(), "113.097336");
    assert_eq!(s.area.to_string(), "113.097336");
    assert_eq!(s.bbox, nm([-2, -1, 0, 4, 5, 6].map(|v| v * 1_000_000)));
    // Cone 3 to 0 over 4: V = π 9 4 / 3 = 12π = 37.699112; slant 5, A = 15π + 9π = 24π.
    let c = m("c0000000");
    assert_eq!(c.volume.to_string(), "37.699112");
    assert_eq!(c.area.to_string(), "75.398224");
    assert_eq!(c.bbox, nm([-2, -1, 3, 4, 5, 7].map(|v| v * 1_000_000)));
    // Frustum 1 to 2 over 3: V = π 3 (1 + 2 + 4) / 3 = 7π = 21.991149; slant √10,
    // A = 3π√10 + π + 4π = 29.802307 + 15.707963 = 45.510270 (π(3√10 + 5) = 45.51027...).
    let d = m("d0000000");
    assert_eq!(d.volume.to_string(), "21.991149");
    assert_eq!(
        d.area.millionths(),
        (PI * (3.0 * 10f64.sqrt() + 5.0) * 1e6).round() as i128
    );
    assert_eq!(d.bbox, nm([-1, 0, 3, 3, 4, 6].map(|v| v * 1_000_000)));
    // Torus 5, 1: V = 2π² 5 = 10π² = 98.696044; A = 4π² 5 = 20π² = 197.392088.
    let t = m("t0000000");
    assert_eq!(t.volume.to_string(), "98.696044");
    assert_eq!(t.area.to_string(), "197.392088");
    assert_eq!(t.bbox, nm([-5, -4, 2, 7, 8, 4].map(|v| v * 1_000_000)));
}

// ---------------------------------------------------------------------------------------
// Timing: a recompute superseded between a step and its summary.

#[test]
fn a_run_superseded_while_its_summaries_are_held_publishes_the_newer_runs_fingerprints() {
    let engine = engine();
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::Summarize { .. }));
    let first = engine.start(checked(&doc(&[&format!("{BLOCK}}}")])));
    assert!(engine.kernel().wait_running(1, WAIT));
    // The edit: a box 41 long.
    let edited = doc(&[&format!(
        "{}}}",
        BLOCK.replace(r#""length": 40"#, r#""length": 41"#)
    )]);
    let second = engine.start(checked(&edited));
    hold.release();
    match first.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Superseded)) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("hung"),
    }
    let Ok(Ok(RunOutcome::Published(model))) = second.wait_timeout(WAIT) else {
        panic!("not published");
    };
    // 41 × 20 × 10 = 8200; 2 (820 + 200 + 410) = 2860.
    let f = model.fingerprint(&id("k7f2qx3m")).unwrap();
    assert_eq!(f.volume.to_string(), "8200");
    assert_eq!(f.area.to_string(), "2860");
    assert_eq!(described(&model, "k7f2qx3m"), "box(41, 20, 10)");
}

#[test]
fn a_cancel_while_a_summary_is_held_leaves_the_next_run_whole() {
    let engine = engine();
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::Summarize { .. }));
    let text = doc(&[&format!("{BLOCK}}}")]);
    let first = engine.start(checked(&text));
    // Until the box's measure is asked, and held.
    let deadline = std::time::Instant::now() + WAIT;
    while !engine
        .kernel()
        .calls()
        .iter()
        .any(|c| c.op.name() == "Summarize")
    {
        assert!(std::time::Instant::now() < deadline, "never measured");
        std::thread::yield_now();
    }
    assert!(engine.kernel().wait_running(1, WAIT));
    engine.cancel();
    hold.release();
    assert!(first.wait_timeout(WAIT).is_ok(), "hung");
    let before = engine.kernel().calls().len();
    let again = published(&engine, checked(&text));
    assert_eq!(
        again
            .fingerprint(&id("k7f2qx3m"))
            .map(|f| f.volume.to_string()),
        Some("8000".to_owned())
    );
    // The box the cancelled run made is kept; its measure, stopped with that run, is asked
    // again by the next, once.
    let calls = engine.kernel().calls();
    assert!(made(&calls[before..]).is_empty(), "{:?}", made(&calls));
    let measures = calls[before..]
        .iter()
        .filter(|c| c.op.name() == "Summarize")
        .count();
    assert_eq!(measures, 1);
}

// ---------------------------------------------------------------------------------------
// Saving: a document that opens, with its fingerprints added.

#[test]
fn a_document_that_opens_near_the_size_limit_and_gains_fingerprints_is_refused_as_data() {
    // Decided (the maintainer, 2026-10-21): fingerprints count toward the limit, so a
    // document that opens just under it and gains them on a save is refused, as data
    // naming the feature.
    let limit = silico_core::document::json::MAX_DOCUMENT_LEN;
    let base = doc(&[&format!(r#"{BLOCK}, "note": ""}}"#)]);
    let pad = limit - write(&read(base.as_bytes()).unwrap()).unwrap().len() - 16;
    let text = doc(&[&format!(r#"{BLOCK}, "note": "{}"}}"#, "n".repeat(pad))]);
    let (document, model) = rebuilt(&text);
    write(&document).unwrap();
    let e = write(&model.saved()).unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.feature(), Some("k7f2qx3m"));
    assert!(e.suggestion().is_some());
}

#[test]
fn a_fingerprinted_document_opens_up_to_the_size_limit_and_not_a_byte_past() {
    // The maintainer, 2026-10-21: fingerprints count toward the limit, reading as writing.
    // A block with its fingerprint and a note sized so its canonical text is the limit less
    // one, the limit, and the limit and one.
    let limit = silico_core::document::json::MAX_DOCUMENT_LEN;
    let block = |note: usize, fingerprint: &str| {
        doc(&[&format!(
            r#"{BLOCK}, "note": "{}"{fingerprint}}}"#,
            "n".repeat(note)
        )])
    };
    let stored = format!(r#", "fingerprint": {{{GOOD}}}"#);
    let base = write(&read(block(0, &stored).as_bytes()).unwrap())
        .unwrap()
        .len();
    for size in [limit - 1, limit] {
        let text = block(size - base, &stored);
        let document = read(text.as_bytes()).unwrap_or_else(|e| panic!("{size}: {e}"));
        assert!(document.features[0].fingerprint.is_some());
        assert_eq!(write(&document).unwrap().len(), size);
    }
    let past = block(limit + 1 - base, &stored);
    let err = read(past.as_bytes()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(
        err.message().contains("too large for Silico to read"),
        "{err}"
    );
    assert_eq!(err.feature(), Some("k7f2qx3m"), "{err}");
    // It's the fingerprint that takes it past: the same document without it opens.
    let unprinted = block(limit + 1 - base, "");
    assert!(
        read(unprinted.as_bytes()).unwrap().features[0]
            .fingerprint
            .is_none()
    );
}

#[test]
fn a_save_is_refused_exactly_when_the_fingerprints_it_writes_take_the_document_past_the_limit() {
    // A block with no fingerprint stored, opening with room to spare; its save writes the
    // fingerprint its rebuild measured, and the note is sized so the saved text is the
    // limit less one, the limit, and the limit and one.
    let limit = silico_core::document::json::MAX_DOCUMENT_LEN;
    let block = |note: usize| doc(&[&format!(r#"{BLOCK}, "note": "{}"}}"#, "n".repeat(note))]);
    let (_, model) = rebuilt(&block(0));
    let base = write(&model.saved()).unwrap().len();
    for size in [limit - 1, limit] {
        let (document, model) = rebuilt(&block(size - base));
        assert!(document.features[0].fingerprint.is_none());
        let formatted = silico_core::command::format(&model)
            .unwrap()
            .unwrap_or_else(|p| panic!("{size}: {:?}", p.errors()));
        assert_eq!(formatted.text.len(), size);
        // And what it saved opens, fingerprint and all.
        let saved = read(formatted.text.as_bytes()).unwrap();
        assert_eq!(
            saved.features[0].fingerprint.unwrap().volume.to_string(),
            "8000"
        );
    }
    let (document, model) = rebuilt(&block(limit + 1 - base));
    assert!(write(&document).is_ok(), "as it was read, it fits");
    let problems = silico_core::command::format(&model).unwrap().unwrap_err();
    assert_eq!(problems.total(), 1);
    let err = &problems.errors()[0];
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(err.message().contains("too large to write"), "{err}");
    assert!(err.message().contains(&limit.to_string()), "{err}");
    assert_eq!(err.feature(), Some("k7f2qx3m"), "{err}");
    assert!(
        err.cause().is_some_and(|c| c.contains("fingerprints")),
        "{err}"
    );
    assert!(err.suggestion().is_some(), "{err}");
}

#[test]
fn a_save_of_a_document_already_past_the_limit_doesnt_blame_the_fingerprints() {
    // Built in memory, as an edit command will build one, never through `read`: too large
    // as it stands, so its save is refused for that, with no word of the fingerprints.
    let limit = silico_core::document::json::MAX_DOCUMENT_LEN;
    let mut document = read(doc(&[&format!(r#"{BLOCK}}}"#)]).as_bytes()).unwrap();
    document.features[0].note = Some("n".repeat(limit));
    assert!(write(&document).is_err());
    let model = published(&engine(), Checked::new(document).unwrap());
    let problems = silico_core::command::format(&model).unwrap().unwrap_err();
    let err = &problems.errors()[0];
    assert!(err.message().contains("too large to write"), "{err}");
    assert_eq!(err.feature(), Some("k7f2qx3m"), "{err}");
    assert!(err.cause().is_none(), "{err}");
}

#[test]
fn drift_is_checked_against_the_document_the_model_was_built_from() {
    // Once drift and fingerprinted took any document, and matched features by ID alone: a
    // model of one document stamped its measures onto another. Now a model checks and
    // writes only its own.
    let tampered = doc(&[&format!(
        r#"{BLOCK}, "fingerprint": {{{}}}}}"#,
        GOOD.replace("8000", "8001")
    )]);
    let (document, model) = rebuilt(&tampered);
    assert_eq!(model.document(), &document);
    let errors = model.drift();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].feature(), Some("k7f2qx3m"));
    assert_eq!(errors[0].parameter(), Some("fingerprint.volume"));
    // Its fingerprints written onto its own document: the block's, rebuilt.
    let written = model.saved();
    assert_eq!(written.features.len(), 1);
    assert_eq!(
        written.features[0].fingerprint.unwrap().volume.to_string(),
        "8000"
    );
}
