//! break-it: hostile input for the M2.2 validation gate (`document::validate`, and the
//! `Operation` accessors and `Primitive::faults` it's built on). Expected values come
//! from the gate's stated contract (its doc comment, docs/roadmap/m2-document-naming.md,
//! the design's D2) and from hand calculation: lengths above zero and at most a kilometre
//! (10^12 nm), positions at most a kilometre either way, and a solid consumed once.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/within.rs"]
mod within;

use std::collections::BTreeSet;
use std::time::Duration;

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::document::{
    Document, Feature, FeatureId, FeatureVersion, FormatVersion, Invalid, Length, Name, Operation,
    Units, read, validate, write,
};
use silico_protocol::Primitive;
use within::within;

const KM: i64 = 1_000_000_000_000;

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn nm(n: i64) -> Length {
    Length::from_nm(n)
}

/// `mm` whole millimetres.
fn mm(mm: i64) -> Length {
    Length::from_nm(mm * 1_000_000)
}

fn feature(identity: &str, name: &str, operation: Operation) -> Feature {
    Feature {
        id: id(identity),
        operation,
        version: FeatureVersion,
        name: Name::new(name).unwrap(),
        note: None,
        fingerprint: None,
    }
}

fn document(features: Vec<Feature>) -> Document {
    Document {
        silico: FormatVersion,
        units: Units,
        features,
    }
}

fn unit_box() -> Operation {
    Operation::Box {
        position: [nm(0); 3],
        length: mm(1),
        width: mm(1),
        height: mm(1),
    }
}

fn cut(base: &str, tool: &str) -> Operation {
    Operation::Cut {
        base: id(base),
        tool: id(tool),
    }
}

fn found(invalid: &Invalid) -> Vec<(String, String)> {
    invalid
        .errors()
        .iter()
        .map(|e| {
            (
                e.feature().unwrap_or("<none>").to_owned(),
                e.parameter().unwrap_or("<none>").to_owned(),
            )
        })
        .collect()
}

/// The kernel's names for parameters, which a document never uses.
const KERNEL_NAMES: [&str; 5] = ["dx", "dy", "dz", "major_radius", "minor_radius"];

/// The value fields each primitive type has in the file (docs/design/m2.1-document-format.md).
fn fields_of(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "box" => &["length", "width", "height"],
        "cylinder" => &["radius", "height"],
        "sphere" => &["radius"],
        "cone" => &["radius1", "radius2", "height"],
        "torus" => &["radius1", "radius2"],
        other => panic!("not a primitive: {other}"),
    }
}

// ---------------------------------------------------------------- the accessors

#[test]
fn each_primitive_maps_its_fields_to_the_kernels_parameters_by_hand() {
    // Distinct values in every field, so a swap of any two shows.
    let p = [mm(7), mm(8), mm(9)];
    let cases = [
        (
            Operation::Box {
                position: p,
                length: mm(1),
                width: mm(2),
                height: mm(3),
            },
            Primitive::Box {
                dx: 1.0,
                dy: 2.0,
                dz: 3.0,
            },
        ),
        (
            Operation::Cylinder {
                position: p,
                radius: mm(4),
                height: mm(5),
            },
            Primitive::Cylinder {
                radius: 4.0,
                height: 5.0,
            },
        ),
        (
            Operation::Sphere {
                position: p,
                radius: nm(1),
            },
            Primitive::Sphere { radius: 1e-6 },
        ),
        (
            Operation::Cone {
                position: p,
                radius1: mm(1),
                radius2: mm(2),
                height: mm(3),
            },
            Primitive::Cone {
                radius1: 1.0,
                radius2: 2.0,
                height: 3.0,
            },
        ),
        (
            Operation::Torus {
                position: p,
                radius1: mm(10),
                radius2: mm(2),
            },
            Primitive::Torus {
                major_radius: 10.0,
                minor_radius: 2.0,
            },
        ),
    ];
    for (operation, primitive) in cases {
        assert_eq!(operation.primitive(), Some(primitive), "{operation:?}");
        assert_eq!(operation.position(), Some(&p));
        assert_eq!(operation.inputs(), None);
    }
    for boolean in [
        Operation::Fuse {
            base: id("aaaaaaaa"),
            tool: id("bbbbbbbb"),
        },
        cut("aaaaaaaa", "bbbbbbbb"),
        Operation::Common {
            base: id("aaaaaaaa"),
            tool: id("bbbbbbbb"),
        },
    ] {
        assert_eq!(boolean.primitive(), None);
        assert_eq!(boolean.position(), None);
        let (a, b) = (id("aaaaaaaa"), id("bbbbbbbb"));
        assert_eq!(boolean.inputs(), Some([("base", &a), ("tool", &b)]));
    }
}

#[test]
fn every_kernel_parameter_of_every_primitive_is_named_as_a_field_its_type_has() {
    let origin = [nm(0); 3];
    let one = nm(1);
    let kernel: [(Operation, &[&str]); 5] = [
        (
            Operation::Box {
                position: origin,
                length: one,
                width: one,
                height: one,
            },
            &["dx", "dy", "dz"],
        ),
        (
            Operation::Cylinder {
                position: origin,
                radius: one,
                height: one,
            },
            &["radius", "height"],
        ),
        (
            Operation::Sphere {
                position: origin,
                radius: one,
            },
            &["radius"],
        ),
        (
            Operation::Cone {
                position: origin,
                radius1: one,
                radius2: one,
                height: one,
            },
            &["radius1", "radius2", "height"],
        ),
        (
            Operation::Torus {
                position: origin,
                radius1: one,
                radius2: one,
            },
            &["major_radius", "minor_radius"],
        ),
    ];
    for (operation, parameters) in kernel {
        let type_name = operation.type_name();
        let named: Vec<&str> = parameters
            .iter()
            .map(|p| operation.document_field(p))
            .collect();
        assert_eq!(named, fields_of(type_name), "{type_name}");
    }
}

#[test]
fn each_type_renames_only_its_own_kernel_parameters() {
    let torus = Operation::Torus {
        position: [nm(0); 3],
        radius1: nm(2),
        radius2: nm(1),
    };
    let cylinder = Operation::Cylinder {
        position: [nm(0); 3],
        radius: nm(1),
        height: nm(1),
    };
    assert_eq!(unit_box().document_field("major_radius"), "major_radius");
    assert_eq!(torus.document_field("dz"), "dz");
    assert_eq!(cylinder.document_field("dx"), "dx");
}

// ---------------------------------------------------------------- every problem at once

#[test]
fn every_bad_value_of_one_primitive_is_reported_not_only_the_first() {
    // Once: `validate` promises "every problem with them" and reports a far position once
    // per axis, but the kernel's rule stopped at the first bad value, so a box with three
    // zero lengths was refused for its `length` alone: an agent fixed it, validated again,
    // and only then learnt of `width`, then `height`.
    let origin = [nm(0); 3];
    let cases = [
        (
            Operation::Box {
                position: origin,
                length: nm(0),
                width: nm(0),
                height: nm(-1),
            },
            vec!["length", "width", "height"],
        ),
        (
            Operation::Cylinder {
                position: origin,
                radius: nm(0),
                height: nm(KM + 1),
            },
            vec!["radius", "height"],
        ),
        (
            Operation::Cone {
                position: origin,
                radius1: nm(-1),
                radius2: nm(-1),
                height: nm(0),
            },
            vec!["radius1", "radius2", "height"],
        ),
        (
            Operation::Torus {
                position: origin,
                radius1: nm(0),
                radius2: nm(0),
            },
            vec!["radius1", "radius2"],
        ),
        // Both radii zero is a fault of its own, beside the height's.
        (
            Operation::Cone {
                position: origin,
                radius1: nm(0),
                radius2: nm(0),
                height: nm(0),
            },
            vec!["radius1", "height"],
        ),
        // A tube wider than a ring that isn't valid itself: only the ring is at fault.
        (
            Operation::Torus {
                position: origin,
                radius1: nm(0),
                radius2: nm(5),
            },
            vec!["radius1"],
        ),
    ];
    let mut failures = Vec::new();
    for (operation, expected) in cases {
        let what = operation.type_name();
        let invalid = validate(&document(vec![feature("aaaaaaaa", "P", operation)])).unwrap_err();
        let parameters: Vec<String> = found(&invalid).into_iter().map(|(_, p)| p).collect();
        if parameters != expected || invalid.total() != expected.len() as u64 {
            failures.push(format!(
                "{what}: reported {parameters:?} (total {}), expected {expected:?}",
                invalid.total()
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// ---------------------------------------------------------------- numbers at every edge

#[test]
fn every_length_at_i64_extremes_is_refused_naming_the_field_without_panicking() {
    for extreme in [i64::MIN, i64::MIN + 1, -1, 0, KM + 1, i64::MAX] {
        let e = nm(extreme);
        let one = nm(1_000_000);
        let operations = [
            (
                Operation::Box {
                    position: [nm(0); 3],
                    length: e,
                    width: one,
                    height: one,
                },
                "length",
            ),
            (
                Operation::Box {
                    position: [nm(0); 3],
                    length: one,
                    width: one,
                    height: e,
                },
                "height",
            ),
            (
                Operation::Cylinder {
                    position: [nm(0); 3],
                    radius: one,
                    height: e,
                },
                "height",
            ),
            (
                Operation::Sphere {
                    position: [nm(0); 3],
                    radius: e,
                },
                "radius",
            ),
            (
                Operation::Torus {
                    position: [nm(0); 3],
                    radius1: e,
                    radius2: nm(1),
                },
                "radius1",
            ),
            (
                Operation::Torus {
                    position: [nm(0); 3],
                    radius1: one,
                    radius2: e,
                },
                "radius2",
            ),
        ];
        for (operation, field) in operations {
            let what = operation.type_name();
            let invalid = validate(&document(vec![feature("aaaaaaaa", "P", operation)]))
                .expect_err(&format!("{what} {field} {extreme} nm"));
            assert_eq!(invalid.total(), 1);
            let error = &invalid.errors()[0];
            assert_eq!(error.code(), ErrorCode::InvalidInput);
            assert_eq!(error.feature(), Some("aaaaaaaa"));
            assert_eq!(error.parameter(), Some(field), "{error:?}");
            assert!(
                error
                    .message()
                    .starts_with(&format!("{what} {field} must be")),
                "{}",
                error.message()
            );
            assert_eq!(
                error.suggestion(),
                Some(format!("give the {what} a valid {field}").as_str())
            );
        }
        // A cone's radius may be zero, so only the non-zero extremes are refused.
        let cone = Operation::Cone {
            position: [nm(0); 3],
            radius1: one,
            radius2: e,
            height: one,
        };
        let result = validate(&document(vec![feature("aaaaaaaa", "K", cone)]));
        assert_eq!(result.is_ok(), extreme == 0, "{result:?}");
    }
}

#[test]
fn positions_at_i64_extremes_are_refused_per_axis_with_the_written_value() {
    let position = [nm(i64::MIN), nm(i64::MAX), nm(-KM)];
    let sphere = Operation::Sphere {
        position,
        radius: mm(1),
    };
    let invalid = validate(&document(vec![feature("aaaaaaaa", "Far", sphere)])).unwrap_err();
    assert_eq!(
        found(&invalid),
        [
            ("aaaaaaaa".to_owned(), "position.0".to_owned()),
            ("aaaaaaaa".to_owned(), "position.1".to_owned())
        ]
    );
    // The value as the file writes it, exactly: i64's limits in nanometres, as millimetres.
    assert_eq!(
        invalid.errors()[0].message(),
        "position.0 is -9223372036854.775808 mm, farther from the origin than the 1000000 mm (a kilometre) a sphere may be placed"
    );
    assert_eq!(
        invalid.errors()[1].message(),
        "position.1 is 9223372036854.775807 mm, farther from the origin than the 1000000 mm (a kilometre) a sphere may be placed"
    );
}

#[test]
fn a_torus_one_nanometre_either_side_of_a_horn_at_a_kilometre_is_judged_exactly() {
    // At a kilometre an f64 step is about 1.2e-10 mm, far below a nanometre, so the
    // 1e-7 mm tolerance can't swallow one: a horn torus passes, a nanometre more fails.
    let torus = |r1: i64, r2: i64| Operation::Torus {
        position: [nm(0); 3],
        radius1: nm(r1),
        radius2: nm(r2),
    };
    for (r1, r2, ok) in [
        (KM, KM, true),
        (KM - 1, KM, false),
        (KM, KM - 1, true),
        (1, 1, true),
        (1, 2, false),
    ] {
        let result = validate(&document(vec![feature("aaaaaaaa", "T", torus(r1, r2))]));
        assert_eq!(result.is_ok(), ok, "{r1} {r2}: {result:?}");
        if let Err(invalid) = result {
            assert_eq!(invalid.errors()[0].parameter(), Some("radius2"));
        }
    }
}

// ---------------------------------------------------------------- references

#[test]
fn a_boolean_of_a_consumed_solid_with_itself_is_refused_twice_both_naming_the_consumer() {
    let invalid = validate(&document(vec![
        feature("aaaaaaaa", "A", unit_box()),
        feature("bbbbbbbb", "B", unit_box()),
        feature("cccccccc", "First", cut("aaaaaaaa", "bbbbbbbb")),
        feature("dddddddd", "Again", cut("aaaaaaaa", "aaaaaaaa")),
    ]))
    .unwrap_err();
    assert_eq!(invalid.total(), 2, "{:#?}", invalid.errors());
    for (error, field) in invalid.errors().iter().zip(["base", "tool"]) {
        assert_eq!(error.feature(), Some("dddddddd"));
        assert_eq!(error.parameter(), Some(field));
        assert_eq!(
            error.message(),
            format!(
                "{field} aaaaaaaa's solid was already consumed by cccccccc (\"First\"): a solid can be used once"
            )
        );
    }
}

#[test]
fn a_boolean_of_itself_twice_is_refused_for_each_field() {
    let invalid = validate(&document(vec![feature(
        "aaaaaaaa",
        "Loop",
        cut("aaaaaaaa", "aaaaaaaa"),
    )]))
    .unwrap_err();
    assert_eq!(
        found(&invalid),
        [
            ("aaaaaaaa".to_owned(), "base".to_owned()),
            ("aaaaaaaa".to_owned(), "tool".to_owned())
        ]
    );
    assert!(
        invalid
            .errors()
            .iter()
            .all(|e| e.message().ends_with("a feature can't use itself"))
    );
}

#[test]
fn a_reference_to_a_shared_id_resolves_to_its_first_feature_and_says_so() {
    // The ID's first feature is after the boolean, its second before: the boolean is
    // refused for a forward reference to the first, named by place.
    let invalid = validate(&document(vec![
        feature("bbbbbbbb", "B", unit_box()),
        feature("cccccccc", "Cut", cut("bbbbbbbb", "aaaaaaaa")),
        feature("aaaaaaaa", "Late", unit_box()),
        feature("aaaaaaaa", "Later", unit_box()),
    ]))
    .unwrap_err();
    assert_eq!(
        found(&invalid),
        [
            ("cccccccc".to_owned(), "tool".to_owned()),
            ("features[3]".to_owned(), "id".to_owned())
        ]
    );
    assert_eq!(
        invalid.errors()[0].message(),
        "tool aaaaaaaa is features[2] (\"Late\"), which comes after this feature: a feature may use only features before it"
    );
    assert_eq!(
        invalid.errors()[1].message(),
        "id aaaaaaaa is already the ID of features[2] (\"Late\")"
    );
}

// ---------------------------------------------------------------- names in messages

#[test]
fn hostile_names_reach_messages_escaped_and_bounded() {
    let huge = "x".repeat(1 << 20);
    let names = [
        "\u{1b}[31mred\u{1b}[0m",
        "line\nbreak\r\n",
        "right\u{202e}to-left",
        "zero\u{200b}width",
        "nul\u{0}byte",
        "bidi\u{2066}isolate\u{2069}",
        huge.as_str(),
    ];
    for name in names {
        // A consumed solid's consumer is named in the second consumer's error, and a
        // shared ID's first holder in the second's.
        let invalid = validate(&document(vec![
            feature("aaaaaaaa", "A", unit_box()),
            feature("bbbbbbbb", "B", unit_box()),
            feature("cccccccc", name, cut("aaaaaaaa", "bbbbbbbb")),
            feature("dddddddd", "D", cut("aaaaaaaa", "bbbbbbbb")),
            feature("cccccccc", "Dup", unit_box()),
        ]))
        .unwrap_err();
        assert!(invalid.total() >= 3, "{:#?}", invalid.errors());
        for error in invalid.errors() {
            for text in [error.message(), error.suggestion().unwrap()] {
                assert!(text.len() < 1000, "{} bytes", text.len());
                let raw: Vec<char> = text
                    .chars()
                    .filter(|&c| {
                        c.is_control()
                            || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                    })
                    .collect();
                assert!(raw.is_empty(), "{name:?} reached {text:?} raw: {raw:?}");
            }
        }
    }
}

// ---------------------------------------------------------------- refusals in the document's words

fn near(limit: i64) -> impl Strategy<Value = i64> {
    prop_oneof![limit - 2..=limit + 2, -limit - 2..=-limit + 2]
}

fn length_values() -> impl Strategy<Value = i64> {
    prop_oneof![near(0), near(KM), 1..KM, Just(i64::MIN), Just(i64::MAX)]
}

fn any_primitive() -> impl Strategy<Value = Operation> {
    let position = || [length_values(), length_values(), length_values()];
    let l = length_values;
    prop_oneof![
        (position(), l(), l(), l()).prop_map(|(p, a, b, c)| Operation::Box {
            position: p.map(nm),
            length: nm(a),
            width: nm(b),
            height: nm(c),
        }),
        (position(), l(), l()).prop_map(|(p, a, b)| Operation::Cylinder {
            position: p.map(nm),
            radius: nm(a),
            height: nm(b),
        }),
        (position(), l()).prop_map(|(p, a)| Operation::Sphere {
            position: p.map(nm),
            radius: nm(a),
        }),
        (position(), l(), l(), l()).prop_map(|(p, a, b, c)| Operation::Cone {
            position: p.map(nm),
            radius1: nm(a),
            radius2: nm(b),
            height: nm(c),
        }),
        (position(), l(), l()).prop_map(|(p, a, b)| Operation::Torus {
            position: p.map(nm),
            radius1: nm(a),
            radius2: nm(b),
        }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4000))]

    #[test]
    fn refusals_name_only_the_documents_fields_never_the_kernels(operation in any_primitive()) {
        let what = operation.type_name();
        let fields = fields_of(what);
        if let Err(invalid) = validate(&document(vec![feature("aaaaaaaa", "P", operation)])) {
            for error in invalid.errors() {
                prop_assert_eq!(error.code(), ErrorCode::InvalidInput);
                prop_assert_eq!(error.feature(), Some("aaaaaaaa"));
                let parameter = error.parameter().unwrap();
                let positional = ["position.0", "position.1", "position.2"].contains(&parameter);
                prop_assert!(positional || fields.contains(&parameter), "{} {}", what, parameter);
                for text in [error.message(), error.suggestion().unwrap()] {
                    for word in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
                        prop_assert!(!KERNEL_NAMES.contains(&word), "{:?}", text);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- random timelines

const IDS: [&str; 5] = ["aaaaaaaa", "bbbbbbbb", "cccccccc", "dddddddd", "eeeeeeee"];
/// An ID no feature has.
const MISSING: &str = "zzzzzzzz";

#[derive(Debug, Clone)]
enum Spec {
    Sphere([i64; 3]),
    Boolean(u8, usize, usize),
}

fn spec() -> impl Strategy<Value = (usize, Spec)> {
    let axis = prop_oneof![4 => Just(0i64), 1 => near(KM)];
    (
        0..IDS.len(),
        prop_oneof![
            [axis.clone(), axis.clone(), axis].prop_map(Spec::Sphere),
            (0..3u8, 0..=IDS.len(), 0..=IDS.len()).prop_map(|(t, b, u)| Spec::Boolean(t, b, u)),
        ],
    )
}

fn build(specs: &[(usize, Spec)]) -> Document {
    let reference = |i: usize| id(IDS.get(i).copied().unwrap_or(MISSING));
    document(
        specs
            .iter()
            .enumerate()
            .map(|(index, (identity, spec))| {
                let operation = match *spec {
                    Spec::Sphere(position) => Operation::Sphere {
                        position: position.map(nm),
                        radius: mm(1),
                    },
                    Spec::Boolean(t, base, tool) => {
                        let (base, tool) = (reference(base), reference(tool));
                        match t {
                            0 => Operation::Fuse { base, tool },
                            1 => Operation::Cut { base, tool },
                            _ => Operation::Common { base, tool },
                        }
                    }
                };
                feature(IDS[*identity], &format!("F{index}"), operation)
            })
            .collect(),
    )
}

/// The problems the gate's contract says these features have, worked out from the rules
/// as stated (not from how `validate` does it): an ID's later holders are refused; each
/// position axis past a kilometre; a reference to no feature, to the feature itself, or to
/// a later one; and a reference to a solid an earlier reference already took, in timeline
/// order, base before tool.
fn expected(specs: &[(usize, Spec)]) -> Vec<(String, String)> {
    let holders = |identity: usize| -> Vec<usize> {
        (0..specs.len())
            .filter(|&i| specs[i].0 == identity)
            .collect()
    };
    let label = |index: usize| {
        if holders(specs[index].0).len() == 1 {
            IDS[specs[index].0].to_owned()
        } else {
            format!("features[{index}]")
        }
    };
    // Every reference that takes a solid, as (taker, field, target), in timeline order.
    let mut taken: Vec<usize> = Vec::new();
    let mut out = Vec::new();
    for (index, (identity, spec)) in specs.iter().enumerate() {
        if holders(*identity)[0] != index {
            out.push((label(index), "id".to_owned()));
        }
        match spec {
            Spec::Sphere(position) => {
                for (axis, v) in position.iter().enumerate() {
                    if v.abs() > KM {
                        out.push((label(index), format!("position.{axis}")));
                    }
                }
            }
            Spec::Boolean(_, base, tool) => {
                for (field, target) in [("base", base), ("tool", tool)] {
                    let Some(&target) = IDS.get(*target).and(holders(*target).first()) else {
                        out.push((label(index), field.to_owned()));
                        continue;
                    };
                    if target >= index || taken.contains(&target) {
                        out.push((label(index), field.to_owned()));
                    } else {
                        taken.push(target);
                    }
                }
            }
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5000))]

    #[test]
    fn random_timelines_are_refused_exactly_as_the_rules_say(
        specs in prop::collection::vec(spec(), 0..14),
    ) {
        let doc = build(&specs);
        let want = expected(&specs);
        let result = validate(&doc);
        match &result {
            Ok(()) => prop_assert!(want.is_empty(), "passed, expected {:?}", want),
            Err(invalid) => {
                prop_assert_eq!(found(invalid), want.clone());
                prop_assert_eq!(invalid.total(), want.len() as u64);
                for error in invalid.errors() {
                    prop_assert_eq!(error.code(), ErrorCode::InvalidInput);
                    prop_assert!(error.suggestion().is_some_and(|s| !s.is_empty()));
                    prop_assert!(error.elements().is_empty());
                }
            }
        }
        // The same on the document read back from its own text, and on a second look.
        let again = read(write(&doc).unwrap().as_bytes()).unwrap();
        prop_assert_eq!(&validate(&again), &result);
        prop_assert_eq!(&validate(&doc), &result);
    }
}

// ---------------------------------------------------------------- scale

#[test]
fn a_million_features_all_refused_validate_promptly() {
    // Every feature a boolean of the first box with a missing tool, all sharing one ID:
    // each refused three ways (its ID, base consumed, tool missing), named by place.
    let mut features = vec![feature("aaaaaaaa", "A", unit_box())];
    features.extend((0..1_000_000).map(|_| feature("aaaaaaaa", "F", cut("aaaaaaaa", MISSING))));
    let doc = document(features);
    let invalid = within(Duration::from_secs(5), move || validate(&doc).unwrap_err());
    // The first boolean takes the box; every later one is refused for that too. Each is
    // refused for its ID and its tool.
    assert_eq!(invalid.total(), 1_000_000 * 3 - 1);
    assert_eq!(invalid.errors().len(), 100);
    assert_eq!(invalid.errors()[0].feature(), Some("features[1]"));
}

#[test]
fn a_million_valid_features_validate_promptly() {
    let alphabet = silico_core::document::ID_ALPHABET.as_bytes();
    let ids: Vec<FeatureId> = (0..1_000_000u64)
        .map(|n| {
            let text: String = (0..8)
                .map(|k| alphabet[((n >> (5 * k)) & 31) as usize] as char)
                .collect();
            id(&text)
        })
        .collect();
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
    let features: Vec<Feature> = ids
        .iter()
        .enumerate()
        .map(|(i, identity)| Feature {
            id: identity.clone(),
            // Every third feature fuses the two before it, which no one else takes.
            operation: if i % 3 == 2 {
                Operation::Fuse {
                    base: ids[i - 2].clone(),
                    tool: ids[i - 1].clone(),
                }
            } else {
                unit_box()
            },
            version: FeatureVersion,
            name: Name::new("F").unwrap(),
            note: None,
            fingerprint: None,
        })
        .collect();
    let doc = document(features);
    let result = within(Duration::from_secs(5), move || validate(&doc));
    assert_eq!(result, Ok(()));
}
