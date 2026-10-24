//! M3.6, step 1: the gate's rules for pad, pocket, revolve, and groove
//! (docs/design/m3.6-sketch-features.md, S1). Messages and limits are written out by hand
//! from the design: a length above 0 and at most a kilometre (10^12 nm, and a two-sided
//! prism's whole length too), an angle above 0° and at most 360° (360 × 10^9 nanodegrees),
//! the sketch a sketch before the feature, used and not consumed, the base consumed once,
//! the axis an axis or a line of the sketch. Every problem at once, as M2.2's.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::sweeps;

use proptest::prelude::*;
use silico_core::document::{Angle, Invalid, Length, PROBLEMS_LISTED};
use sweeps::*;

#[test]
fn valid_sweeps_pass_and_one_sketch_may_be_used_by_several() {
    let sketch = empty_sketch("s0000001", "S", "xy");
    checked(&[
        &sketch,
        PROFILE,
        // Two pads of one sketch, the second on the first; a pocket of the same sketch.
        &pad("p0000001", None, "s0000001", TEN),
        &pad("p0000002", Some("p0000001"), "s0000001", TEN),
        &pocket("q0000001", "p0000002", "s0000001", TEN),
        // Revolves and a groove of one sketch, about each kind of axis.
        &turned("r0000001", "revolve", None, "s0000003", "x_axis", TURN),
        &turned("r0000002", "revolve", Some("r0000001"), "s0000003", "y_axis", TURN),
        // About a construction line, and a line that's part of the profile.
        &turned("g0000001", "groove", Some("q0000001"), "s0000003", "na000000", TURN),
        &turned("g0000002", "groove", Some("g0000001"), "s0000003", "nb000000", TURN),
        // A boolean consuming what the sweeps made.
        r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "F", "base": "g0000002", "tool": "r0000002"}"#,
    ])
    .unwrap();
}

#[test]
fn a_sketch_that_isnt_one_before_the_feature_is_refused() {
    let advice = "use the ID of a sketch before this feature";
    assert_refused(
        &problem(&[&pad("p0000001", None, "zzzzzzzz", TEN)]),
        "p0000001",
        "sketch",
        "sketch zzzzzzzz isn't the ID of any feature",
        advice,
    );
    assert_refused(
        &problem(&[&block("k7f2qx3m"), &pad("p0000001", None, "k7f2qx3m", TEN)]),
        "p0000001",
        "sketch",
        "sketch k7f2qx3m is k7f2qx3m (\"Block\"), a box, not a sketch",
        advice,
    );
    assert_refused(
        &problem(&[&pad("p0000001", None, "p0000001", TEN)]),
        "p0000001",
        "sketch",
        "sketch p0000001 is this feature: a feature can't use itself",
        advice,
    );
    assert_refused(
        &problem(&[
            &pad("p0000001", None, "s0000001", TEN),
            &empty_sketch("s0000001", "Later", "xy"),
        ]),
        "p0000001",
        "sketch",
        "sketch s0000001 is s0000001 (\"Later\"), which comes after this feature: a feature may use only features before it",
        "move this feature after the sketch it uses, or use a sketch before it",
    );
    // A pad that sweeps a pad, not a sketch.
    let sketch = empty_sketch("s0000001", "S", "xy");
    assert_refused(
        &problem(&[
            &sketch,
            &pad("p0000001", None, "s0000001", TEN),
            &pad("p0000002", None, "p0000001", TEN),
        ]),
        "p0000002",
        "sketch",
        "sketch p0000001 is p0000001 (\"Pad\"), a pad, not a sketch",
        advice,
    );
}

#[test]
fn a_base_follows_the_booleans_rules_and_is_consumed_once() {
    let sketch = empty_sketch("s0000001", "S", "xy");
    // Consumed twice: by a pad, then by a pocket.
    assert_refused(
        &problem(&[
            &sketch,
            &block("k7f2qx3m"),
            &pad("p0000001", Some("k7f2qx3m"), "s0000001", TEN),
            &pocket("q0000001", "k7f2qx3m", "s0000001", TEN),
        ]),
        "q0000001",
        "base",
        "base k7f2qx3m's solid was already consumed by p0000001 (\"Pad\"): a solid can be used once",
        "use p0000001's result, or another feature's",
    );
    // Consumed by a boolean, then a revolve.
    assert_refused(
        &problem(&[
            PROFILE,
            &block("k7f2qx3m"),
            &block("a91cw0de"),
            r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "F", "base": "k7f2qx3m", "tool": "a91cw0de"}"#,
            &turned(
                "r0000001",
                "revolve",
                Some("a91cw0de"),
                "s0000003",
                "x_axis",
                TURN,
            ),
        ]),
        "r0000001",
        "base",
        "base a91cw0de's solid was already consumed by f0000001 (\"F\"): a solid can be used once",
        "use f0000001's result, or another feature's",
    );
    // A sketch makes no solid to be a base.
    assert_refused(
        &problem(&[&sketch, &pocket("q0000001", "s0000001", "s0000001", TEN)]),
        "q0000001",
        "base",
        "base s0000001 is s0000001 (\"S\"), a sketch, which makes no solid for a pocket to use",
        "use a feature that makes a solid",
    );
    assert_refused(
        &problem(&[
            &sketch,
            &turned(
                "g0000001",
                "groove",
                Some("zzzzzzzz"),
                "s0000001",
                "x_axis",
                TURN,
            ),
        ]),
        "g0000001",
        "base",
        "base zzzzzzzz isn't the ID of any feature",
        "use the ID of a feature before this one",
    );
    assert_refused(
        &problem(&[
            &sketch,
            &pad("p0000001", Some("k7f2qx3m"), "s0000001", TEN),
            &block("k7f2qx3m"),
        ]),
        "p0000001",
        "base",
        "base k7f2qx3m is k7f2qx3m (\"Block\"), which comes after this feature: a feature may use only features before it",
        "move this feature after the one it uses, or use one before it",
    );
}

/// The problems of a pad of `extent` from an empty sketch.
fn pad_problems(extent: &str) -> Result<(), Invalid> {
    checked(&[
        &empty_sketch("s0000001", "S", "xy"),
        &pad("p0000001", None, "s0000001", extent),
    ])
}

fn turn_problems(type_name: &str, extent: &str) -> Result<(), Invalid> {
    let base = (type_name == "groove").then_some("k7f2qx3m");
    checked(&[
        &empty_sketch("s0000001", "S", "xy"),
        &block("k7f2qx3m"),
        &turned("r0000001", type_name, base, "s0000001", "x_axis", extent),
    ])
}

#[test]
fn lengths_are_above_0_and_at_most_a_kilometre_exactly() {
    // At the bounds, each form: 1 nm and 10^12 nm pass.
    for length in ["0.000001", "1000000"] {
        for extent in [
            one_side(length),
            format!(r#"{{"type": "symmetric", "length": {length}}}"#),
            format!(r#"{{"type": "two_sides", "length": {length}, "length2": 0.000001}}"#),
            format!(r#"{{"type": "two_sides", "length": 0.000001, "length2": {length}}}"#),
        ] {
            // (Two sides of a kilometre and a nanometre are past a kilometre together:
            // `two_sides_together_are_at_most_a_kilometre`.)
            if extent.contains("two_sides") && length == "1000000" {
                continue;
            }
            assert_eq!(pad_problems(&extent), Ok(()), "{extent}");
        }
        // A pocket's lengths alike.
        assert_eq!(
            checked(&[
                &empty_sketch("s0000001", "S", "xy"),
                &block("k7f2qx3m"),
                &pocket("q0000001", "k7f2qx3m", "s0000001", &one_side(length)),
            ]),
            Ok(())
        );
    }
    let range = |what: &str, parameter: &str, got: &str| {
        (
            format!(
                "{what} {parameter} must be a length above 0 mm and at most 1000000 mm (a kilometre), got {got}"
            ),
            format!("give the {what} a valid {parameter}"),
        )
    };
    // Just outside: 0, a nanometre past a kilometre, and below 0.
    for (written, got) in [
        ("0", "0"),
        ("1000000.000001", "1000000.000001"),
        ("-0.000001", "-0.000001"),
        ("-10", "-10"),
    ] {
        for (extent, parameter) in [
            (one_side(written), "extent.length"),
            (
                format!(r#"{{"type": "symmetric", "length": {written}}}"#),
                "extent.length",
            ),
            (
                format!(r#"{{"type": "two_sides", "length": 1, "length2": {written}}}"#),
                "extent.length2",
            ),
        ] {
            let invalid = pad_problems(&extent).unwrap_err();
            assert_eq!(invalid.total(), 1, "{extent}: {:#?}", invalid.errors());
            let (message, suggestion) = range("pad", parameter, got);
            assert_refused(
                &invalid.errors()[0],
                "p0000001",
                parameter,
                &message,
                &suggestion,
            );
        }
        // A pocket's too.
        let invalid = checked(&[
            &empty_sketch("s0000001", "S", "xy"),
            &block("k7f2qx3m"),
            &pocket("q0000001", "k7f2qx3m", "s0000001", &one_side(written)),
        ])
        .unwrap_err();
        let (message, suggestion) = range("pocket", "extent.length", got);
        assert_eq!(invalid.total(), 1);
        assert_refused(
            &invalid.errors()[0],
            "q0000001",
            "extent.length",
            &message,
            &suggestion,
        );
    }
}

#[test]
fn two_sides_together_are_at_most_a_kilometre() {
    // 600 + 400 m is a kilometre: passes. A nanometre more, either side: refused, as a
    // whole, naming the extent.
    let two =
        |a: &str, b: &str| format!(r#"{{"type": "two_sides", "length": {a}, "length2": {b}}}"#);
    pad_problems(&two("600000", "400000")).unwrap();
    pad_problems(&two("999999.999999", "0.000001")).unwrap();
    for (a, b) in [
        ("600000.000001", "400000"),
        ("600000", "400000.000001"),
        ("1000000", "1000000"),
    ] {
        let invalid = pad_problems(&two(a, b)).unwrap_err();
        assert_eq!(invalid.total(), 1, "{:#?}", invalid.errors());
        let sum = match (a, b) {
            ("1000000", "1000000") => "2000000",
            _ => "1000000.000001",
        };
        assert_refused(
            &invalid.errors()[0],
            "p0000001",
            "extent",
            &format!(
                "pad extent.length and extent.length2 together must be at most 1000000 mm (a kilometre), the prism's length, got {sum}"
            ),
            "shorten extent.length or extent.length2",
        );
    }
    // Each wrong alone: each named, and no sum, which means nothing then.
    let invalid = pad_problems(&two("0", "1000001")).unwrap_err();
    let parameters: Vec<_> = invalid.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(parameters, [Some("extent.length"), Some("extent.length2")]);
    // And one wrong, the other a kilometre: the wrong one only.
    let invalid = pad_problems(&two("-1", "1000000")).unwrap_err();
    assert_eq!(invalid.total(), 1);
    assert_eq!(invalid.errors()[0].parameter(), Some("extent.length"));
}

#[test]
fn angles_are_above_0_and_at_most_360_degrees_exactly() {
    for type_name in ["revolve", "groove"] {
        for written in ["0.000000001", "360", "180"] {
            for extent in [
                angle(written),
                format!(r#"{{"type": "symmetric", "angle": {written}}}"#),
            ] {
                assert_eq!(turn_problems(type_name, &extent), Ok(()), "{extent}");
            }
        }
        for written in ["0", "360.000000001", "-0.000000001", "720"] {
            for extent in [
                angle(written),
                format!(r#"{{"type": "symmetric", "angle": {written}}}"#),
            ] {
                let invalid = turn_problems(type_name, &extent).unwrap_err();
                assert_eq!(invalid.total(), 1, "{:#?}", invalid.errors());
                assert_refused(
                    &invalid.errors()[0],
                    "r0000001",
                    "extent.angle",
                    &format!(
                        "{type_name} extent.angle must be an angle above 0° and at most 360°, got {written}°"
                    ),
                    &format!("give the {type_name} a valid extent.angle"),
                );
            }
        }
    }
}

#[test]
fn an_axis_is_an_axis_or_a_line_of_the_sketch() {
    let axis_problem = |type_name: &str, axis: &str| {
        let base = (type_name == "groove").then_some("k7f2qx3m");
        problem(&[
            PROFILE,
            &block("k7f2qx3m"),
            &turned("r0000001", type_name, base, "s0000003", axis, TURN),
        ])
    };
    let advice = "name a line of the sketch, x_axis, or y_axis";
    for (axis, kind) in [
        ("pa000000", "point"),
        ("ca000000", "circle"),
        ("aa000000", "arc"),
        ("ka000000", "constraint"),
    ] {
        for type_name in ["revolve", "groove"] {
            assert_refused(
                &axis_problem(type_name, axis),
                "r0000001",
                "axis",
                &format!(
                    "axis {axis} is a {kind} of sketch s0000003: a {type_name} turns about a line"
                ),
                advice,
            );
        }
    }
    assert_refused(
        &axis_problem("revolve", "zz000000"),
        "r0000001",
        "axis",
        "axis zz000000 isn't an element of sketch s0000003: a revolve turns about a line of its sketch, x_axis, or y_axis",
        advice,
    );
    // A line of another sketch isn't one of this one's.
    let invalid = checked(&[
        PROFILE,
        &empty_sketch("s0000001", "S", "xy"),
        &turned("r0000001", "revolve", None, "s0000001", "na000000", TURN),
    ])
    .unwrap_err();
    assert_eq!(invalid.total(), 1);
    assert_eq!(
        invalid.errors()[0].message(),
        "axis na000000 isn't an element of sketch s0000001: a revolve turns about a line of its sketch, x_axis, or y_axis"
    );
    // An axis of a feature that isn't a sketch: only the sketch is refused.
    let invalid = checked(&[
        &block("k7f2qx3m"),
        &turned("r0000001", "revolve", None, "k7f2qx3m", "na000000", TURN),
    ])
    .unwrap_err();
    assert_eq!(invalid.total(), 1);
    assert_eq!(invalid.errors()[0].parameter(), Some("sketch"));
}

#[test]
fn every_problem_is_listed_at_once_in_the_order_of_the_fields() {
    // Values, then base, sketch, axis; feature by feature.
    let invalid = checked(&[
        PROFILE,
        &turned(
            "r0000001",
            "groove",
            Some("zzzzzzzz"),
            "s0000003",
            "pa000000",
            &angle("0"),
        ),
        &pad("p0000001", Some("s0000003"), "nnnnnnnn", &one_side("0")),
    ])
    .unwrap_err();
    let got: Vec<(Option<&str>, Option<&str>)> = invalid
        .errors()
        .iter()
        .map(|e| (e.feature(), e.parameter()))
        .collect();
    assert_eq!(
        got,
        [
            (Some("r0000001"), Some("extent.angle")),
            (Some("r0000001"), Some("base")),
            (Some("r0000001"), Some("axis")),
            (Some("p0000001"), Some("extent.length")),
            (Some("p0000001"), Some("base")),
            (Some("p0000001"), Some("sketch")),
        ]
    );
    assert_eq!(invalid.total(), 6);
}

#[test]
fn past_a_hundred_problems_the_first_hundred_are_listed_with_the_total() {
    let sketch = empty_sketch("s0000001", "S", "xy");
    let pads: Vec<String> = (0..150)
        .map(|i| {
            pad(
                &format!("p{i:07}"),
                None,
                "s0000001",
                // Each two problems: its length, and its sketch.
                &one_side("0"),
            )
            .replace("\"s0000001\"", "\"zzzzzzzz\"")
        })
        .collect();
    let mut features: Vec<&str> = vec![&sketch];
    features.extend(pads.iter().map(String::as_str));
    let invalid = checked(&features).unwrap_err();
    assert_eq!(invalid.total(), 300);
    assert_eq!(invalid.errors().len(), PROBLEMS_LISTED);
    assert_eq!(invalid.errors()[99].feature(), Some("p0000049"));
    assert_eq!(invalid.errors()[99].parameter(), Some("sketch"));
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 300,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5ee_7ea7),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// Any pad length passes the gate exactly when it's 1 to 10^12 nm.
    #[test]
    fn a_pad_length_passes_exactly_in_range(
        nm in prop_oneof![
            any::<i64>(),
            -10i64..10,
            999_999_999_990i64..1_000_000_000_010,
        ],
    ) {
        let extent = one_side(&Length::from_nm(nm).to_string());
        let passes = pad_problems(&extent).is_ok();
        prop_assert_eq!(passes, (1..=1_000_000_000_000).contains(&nm), "{}", extent);
    }

    /// Two sides pass exactly when each is 1 to 10^12 nm and their sum at most 10^12.
    #[test]
    fn two_sides_pass_exactly_in_range(
        a in prop_oneof![-2i64..3, 0i64..1_000_000_000_002, 499_999_999_990i64..500_000_000_010],
        b in prop_oneof![-2i64..3, 0i64..1_000_000_000_002, 499_999_999_990i64..500_000_000_010],
    ) {
        let extent = format!(
            r#"{{"type": "two_sides", "length": {}, "length2": {}}}"#,
            Length::from_nm(a),
            Length::from_nm(b)
        );
        let max = 1_000_000_000_000i64;
        let want = (1..=max).contains(&a) && (1..=max).contains(&b) && a + b <= max;
        prop_assert_eq!(pad_problems(&extent).is_ok(), want, "{}", extent);
    }

    /// Any angle passes exactly when it's 1 to 360 × 10^9 nanodegrees.
    #[test]
    fn an_angle_passes_exactly_in_range(
        ndeg in prop_oneof![
            any::<i64>(),
            -10i64..10,
            359_999_999_990i64..360_000_000_010,
        ],
    ) {
        let extent = format!(
            r#"{{"type": "symmetric", "angle": {}}}"#,
            Angle::from_ndeg(ndeg)
        );
        let passes = turn_problems("revolve", &extent).is_ok();
        prop_assert_eq!(passes, (1..=360_000_000_000).contains(&ndeg), "{}", extent);
    }
}
