//! /break-it on M6.3's pattern rebuild, on the fake kernel: counts at the cap, every copy
//! skipped, skips past a count, turns at a whole turn and a nanodegree short, placements a
//! kilometre out, parallel directions, hostile references, nested whole-solid patterns and
//! their names, a cache round trip, two engines, a superseded rebuild, an original that
//! fails, a suppressed base, and a second original's step that changes nothing. Every
//! expected value is worked by hand from the document's integers.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::document::{Checked, read};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{ElementKind, Op, Placement};
use silico_core::naming::{Maker, Name};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{ErrorCode, document::FeatureId};
use silico_protocol::CopiesOp;

use super::cache::{Scratch, session};
use super::pattern_docs::{
    FIVE, HOLE, SOLID, THROUGH, checked, direction, document, failure, linear_as, map, mirror,
    names, plate_with_hole_at, with,
};
use super::sweeps::{circles, pad, pocket, rectangle};

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn plate() -> Vec<String> {
    plate_with_hole_at([-40.0, -20.0])
}

fn linear(base: &str, copies: &str, directions: &[String], skip: &str) -> String {
    linear_as("t0000001", base, copies, directions, skip)
}

fn polar(axis: &str, kind: &str, angle: &str, count: u32, skip: &str) -> String {
    format!(
        r#"{{"id": "t0000002", "type": "polar_pattern", "v": 1, "name": "Ring", "base": "q0000002", "copies": {HOLE}, "axis": "{axis}", "reversed": false, "distribution": {{"type": "{kind}", "angle": {angle}}}, "count": {count}, "skip": {skip}}}"#
    )
}

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

fn published(engine: &Recompute<FakeKernel>, features: &[String]) -> Arc<Model> {
    super::pattern_docs::published(engine, features, Duration::from_secs(120))
}

fn copies(kernel: &FakeKernel) -> Vec<(CopiesOp, Vec<Placement>)> {
    kernel
        .calls()
        .into_iter()
        .filter_map(|call| match call.op {
            Op::Copies { op, placements, .. } => Some((op, placements)),
            _ => None,
        })
        .collect()
}

fn outcome(model: &Model, feature: &str) -> FeatureOutcome {
    model.outcome(&id(feature)).cloned().unwrap()
}

/// Every name in `feature`'s map parses (with the document's makers) and finds the element
/// it's held for, and no name is held for two elements.
fn names_round_trip(model: &Model, feature: &str) -> usize {
    let map = map(model, feature);
    let makers = |f: &FeatureId| {
        model
            .document()
            .features
            .iter()
            .find(|x| &x.id == f)
            .map(|x| Maker::of(&x.operation))
    };
    let mut seen = BTreeSet::new();
    let mut count = 0;
    for (element, held) in map.iter() {
        for name in held {
            let text = name.to_string();
            assert!(seen.insert(text.clone()), "{text} is held twice");
            let parsed = Name::parse(&text, &makers)
                .unwrap_or_else(|e| panic!("{text} doesn't parse: {e:?}"));
            assert_eq!(map.element(&parsed), Some(element), "{text}");
            count += 1;
        }
    }
    count
}

// ---------------------------------------------------------------------------------------
// Counts at the cap

#[test]
fn a_linear_pattern_of_exactly_1000_instances_sends_999_copies_each_at_its_exact_offset() {
    // 1000 instances 0.5 mm apart: copy k at k · 0.5, exactly (k · 500000 nm over 1e6).
    let kernel_engine = engine();
    let started = Instant::now();
    let model = published(
        &kernel_engine,
        &with(
            plate(),
            &[linear(
                "q0000002",
                HOLE,
                &[direction("x", "spacing", "0.5", 1000)],
                "[]",
            )],
        ),
    );
    let elapsed = started.elapsed();
    assert!(matches!(
        outcome(&model, "t0000001"),
        FeatureOutcome::Built { .. }
    ));
    let sent = copies(kernel_engine.kernel());
    assert_eq!(sent.len(), 1);
    let (op, placements) = &sent[0];
    assert_eq!(*op, CopiesOp::Cut);
    assert_eq!(placements.len(), 999);
    for (k, placement) in placements.iter().enumerate() {
        let k = (k + 1) as f64;
        assert_eq!(
            *placement,
            Placement::Translation {
                offset: [k * 0.5, 0.0, 0.0]
            }
        );
    }
    // 999 copies' names, each the sweep's wrapped once: the hole's own names times 999,
    // with the base's.
    let base = names(&map(&model, "q0000002")).len();
    let held = names(&map(&model, "t0000001")).len();
    let sweep = names(&map(&model, "q0000002"))
        .into_iter()
        .filter(|(_, n)| n.starts_with("q0000002."))
        .count();
    assert_eq!(held, base + 999 * sweep);
    assert!(elapsed < Duration::from_secs(60), "{elapsed:?}");
}

#[test]
fn a_grid_of_8_by_125_sends_its_999_copies_direction_1_outer_each_by_hand() {
    // 8 along x 10 apart, 125 along y 0.1 apart: copy (i, j) at (10 i, j / 10), j / 10
    // the correctly rounded tenth (j · 100000 nm over 1e6, both exact).
    let kernel_engine = engine();
    let model = published(
        &kernel_engine,
        &with(
            plate(),
            &[linear(
                "q0000002",
                HOLE,
                &[
                    direction("x", "spacing", "10", 8),
                    direction("y", "spacing", "0.1", 125),
                ],
                "[]",
            )],
        ),
    );
    assert!(matches!(
        outcome(&model, "t0000001"),
        FeatureOutcome::Built { .. }
    ));
    let sent = copies(kernel_engine.kernel());
    let placements = &sent[0].1;
    assert_eq!(placements.len(), 999);
    let mut want = Vec::new();
    for i in 0..8u32 {
        for j in 0..125u32 {
            if (i, j) == (0, 0) {
                continue;
            }
            want.push(Placement::Translation {
                offset: [f64::from(i) * 10.0, f64::from(j) / 10.0, 0.0],
            });
        }
    }
    assert_eq!(*placements, want);
}

#[test]
fn every_one_of_999_copies_skipped_is_the_identity_asking_the_kernel_nothing() {
    let skip: Vec<String> = (1..1000).map(|k| format!("[{k}]")).collect();
    let kernel_engine = engine();
    let model = published(
        &kernel_engine,
        &with(
            plate(),
            &[linear(
                "q0000002",
                HOLE,
                &[direction("x", "spacing", "0.5", 1000)],
                &format!("[{}]", skip.join(", ")),
            )],
        ),
    );
    assert!(copies(kernel_engine.kernel()).is_empty());
    assert_eq!(
        names(&map(&model, "t0000001")),
        names(&map(&model, "q0000002"))
    );
    // The identity places nothing, so keeps no verdict.
    assert!(model.pattern_verdicts(&id("t0000001")).is_none());
}

#[test]
fn a_skip_past_every_count_takes_nothing_away_and_names_as_with_no_skip() {
    // [5] and [0, 3] past counts of 3 and 2: ignored, so five copies, named as with none.
    let directions = [
        direction("x", "spacing", "10", 3),
        direction("y", "spacing", "10", 2),
    ];
    let kernel_engine = engine();
    let skipped = published(
        &kernel_engine,
        &with(
            plate(),
            &[linear("q0000002", HOLE, &directions, "[[5], [0, 3]]")],
        ),
    );
    let none = published(
        &engine(),
        &with(plate(), &[linear("q0000002", HOLE, &directions, "[]")]),
    );
    assert_eq!(copies(kernel_engine.kernel())[0].1.len(), 5);
    assert_eq!(
        names(&map(&skipped, "t0000001")),
        names(&map(&none, "t0000001"))
    );
}

// ---------------------------------------------------------------------------------------
// Turns and reaches

#[test]
fn a_whole_turn_of_1000_spreads_copies_0_36_degrees_apart_the_last_short_of_the_original() {
    let kernel_engine = engine();
    let model = published(
        &kernel_engine,
        &with(plate(), &[polar("z", "extent", "360", 1000, "[]")]),
    );
    assert!(matches!(
        outcome(&model, "t0000002"),
        FeatureOutcome::Built { .. }
    ));
    let sent = copies(kernel_engine.kernel());
    let placements = &sent[0].1;
    assert_eq!(placements.len(), 999);
    for (k, placement) in placements.iter().enumerate() {
        let k = (k + 1) as f64;
        // k · 360 / 1000 degrees, the integer product exact and divided once.
        let degrees = k * 360.0 / 1000.0;
        assert_eq!(
            *placement,
            Placement::Rotation {
                axis: silico_protocol::Axis {
                    origin: [0.0; 3],
                    direction: [0.0, 0.0, 1.0]
                },
                angle: silico_protocol::radians(degrees)
            }
        );
    }
}

#[test]
fn an_extent_a_nanodegree_short_of_a_turn_places_its_one_copy_there_not_on_the_original() {
    // 359.999999999° over one gap: the copy at 359.999999999°, 1.7e-11 rad short of a
    // turn, more than OCCT's 1e-12, so it's sent (and the kernel's rule accepts it).
    let kernel_engine = engine();
    let model = published(
        &kernel_engine,
        &with(plate(), &[polar("z", "extent", "359.999999999", 2, "[]")]),
    );
    assert!(matches!(
        outcome(&model, "t0000002"),
        FeatureOutcome::Built { .. }
    ));
    let sent = copies(kernel_engine.kernel());
    assert_eq!(
        sent[0].1,
        [Placement::Rotation {
            axis: silico_protocol::Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 1.0]
            },
            angle: silico_protocol::radians(359.999999999)
        }]
    );
}

#[test]
fn two_directions_each_a_kilometre_place_the_far_corner_at_a_kilometre_on_each_axis() {
    // Each direction reaches exactly 1e6 mm, which the gate takes; the corner copy is
    // (1e6, 1e6, 0), each component at most a kilometre, which the kernel's rule takes.
    let kernel_engine = engine();
    let model = published(
        &kernel_engine,
        &with(
            plate(),
            &[linear(
                "q0000002",
                HOLE,
                &[
                    direction("x", "spacing", "1000000", 2),
                    direction("y", "spacing", "1000000", 2),
                ],
                "[]",
            )],
        ),
    );
    let sent = copies(kernel_engine.kernel());
    assert_eq!(
        sent[0].1,
        [[0.0, 1e6, 0.0], [1e6, 0.0, 0.0], [1e6, 1e6, 0.0]]
            .map(|offset| Placement::Translation { offset })
            .to_vec(),
        "{:?}",
        outcome(&model, "t0000001")
    );
}

#[test]
fn a_reversed_direction_sends_negative_offsets_and_no_negative_zero() {
    let kernel_engine = engine();
    let mut x = direction("x", "spacing", "10", 3);
    x = x.replace("\"reversed\": false", "\"reversed\": true");
    published(
        &kernel_engine,
        &with(plate(), &[linear("q0000002", HOLE, &[x], "[]")]),
    );
    let sent = copies(kernel_engine.kernel());
    for (k, placement) in sent[0].1.iter().enumerate() {
        let Placement::Translation { offset } = placement else {
            panic!("{placement:?}");
        };
        assert_eq!(offset[0], -10.0 * (k + 1) as f64);
        // +0.0, bit for bit: a −0.0 would make a different request, and step, for the
        // same copy.
        assert_eq!(offset[1].to_bits(), 0, "{offset:?}");
        assert_eq!(offset[2].to_bits(), 0, "{offset:?}");
    }
}

#[test]
fn two_directions_along_one_axis_either_way_are_refused_naming_direction_1() {
    for second in [
        direction("x", "spacing", "10", 2),
        direction("x", "spacing", "10", 2).replace("\"reversed\": false", "\"reversed\": true"),
    ] {
        let kernel_engine = engine();
        let model = published(
            &kernel_engine,
            &with(
                plate(),
                &[linear(
                    "q0000002",
                    HOLE,
                    &[direction("x", "spacing", "20", 2), second.clone()],
                    "[]",
                )],
            ),
        );
        let error = failure(&model, "t0000001");
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{second}");
        assert_eq!(error.parameter(), Some("directions.1.along"));
        assert_eq!(error.feature(), Some("t0000001"));
        assert!(copies(kernel_engine.kernel()).is_empty());
    }
}

// ---------------------------------------------------------------------------------------
// Hostile references

/// A rebuild's verdict on `features`' pattern t0000001: its gate's refusal or its outcome.
fn verdict(features: &[String]) -> Result<FeatureOutcome, silico_core::Error> {
    let document = read(document(features).as_bytes()).unwrap();
    match Checked::new(document) {
        Err(invalid) => Err(invalid.errors().first().cloned().unwrap()),
        Ok(checked) => match engine()
            .start(checked)
            .wait_timeout(Duration::from_secs(60))
        {
            Ok(Ok(RunOutcome::Published(model))) => Ok(outcome(&model, "t0000001")),
            Ok(other) => panic!("not published: {other:?}"),
            Err(_) => panic!("the rebuild didn't finish"),
        },
    }
}

#[test]
fn a_direction_naming_the_patterns_own_copy_or_a_later_feature_is_refused_never_a_bug() {
    // The pattern's own copy of the hole's wall, and a later pad's edge: neither is in its
    // base's solid when it rebuilds.
    let wall = "q0000002.pocket.Face(G s0000003.sketch.Edge.ca000001)";
    for name in [
        format!("t0000001.linear_pattern.Face(C1 {wall})"),
        "p0000009.pad.Edge(G s0000001.sketch.Edge.na000001)".to_owned(),
    ] {
        let mut features = with(
            plate(),
            &[linear(
                "q0000002",
                HOLE,
                &[direction(&name, "spacing", "10", 2)],
                "[]",
            )],
        );
        features.push(pad("p0000009", Some("t0000001"), "s0000001", FIVE));
        match verdict(&features) {
            Err(error) => {
                assert_eq!(error.code(), ErrorCode::InvalidInput, "{name}: {error}");
                assert_eq!(error.parameter(), Some("directions.0.along"), "{name}");
            }
            Ok(FeatureOutcome::Broken { errors, .. }) => {
                assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken, "{name}");
                assert_eq!(errors[0].parameter(), Some("directions.0.along"));
            }
            Ok(other) => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn a_direction_naming_another_bodys_edge_is_refused_never_a_bug() {
    // A second body, padded apart, before the pattern: its edge is in no solid of the
    // pattern's base's chain.
    let mut features = plate();
    features.push(rectangle("s0000008", "xy", [200.0, 0.0], 10.0, 10.0));
    features.push(pad("p0000008", None, "s0000008", FIVE));
    features.push(linear(
        "q0000002",
        HOLE,
        &[direction(
            "p0000008.pad.Edge(G s0000008.sketch.Edge.na000001)",
            "spacing",
            "10",
            2,
        )],
        "[]",
    ));
    match verdict(&features) {
        Err(error) => assert_eq!(error.parameter(), Some("directions.0.along"), "{error}"),
        Ok(FeatureOutcome::Broken { errors, .. }) => {
            assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
            assert_eq!(errors[0].parameter(), Some("directions.0.along"));
        }
        Ok(other) => panic!("{other:?}"),
    }
}

#[test]
fn a_mirror_plane_naming_a_face_the_fake_doesnt_type_is_refused_for_its_kind_not_a_bug() {
    // The fake describes every face as a surface OCCT doesn't type: refused as no planar
    // face, invalid_input, naming the field.
    let plane = "p0000001.pad.Face(M s0000001.sketch.Face1)";
    let model = published(
        &engine(),
        &with(plate(), &[mirror("m0000001", "q0000002", HOLE, plane)]),
    );
    let error = failure(&model, "m0000001");
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert_eq!(error.parameter(), Some("plane"));
    assert!(
        error.message().contains("a mirror plane is a planar face"),
        "{error}"
    );
}

// ---------------------------------------------------------------------------------------
// Nesting, determinism, the cache, and supersession

/// A pattern t0000001 of the hole, a mirror m0000001 of the whole result about yz, and a
/// whole-solid linear pattern t0000003 of that along z.
fn nested() -> Vec<String> {
    with(
        plate(),
        &[
            linear(
                "q0000002",
                HOLE,
                &[direction("x", "spacing", "20", 2)],
                "[]",
            ),
            mirror("m0000001", "t0000001", SOLID, "yz"),
            linear_as(
                "t0000003",
                "m0000001",
                SOLID,
                &[direction("z", "spacing", "5", 2)],
                "[]",
            ),
        ],
    )
}

#[test]
fn patterns_of_patterns_nest_their_names_each_parsing_to_its_one_element() {
    let model = published(&engine(), &nested());
    for feature in ["t0000001", "m0000001", "t0000003"] {
        assert!(names_round_trip(&model, feature) > 0, "{feature}");
    }
    // The hole's copy, mirrored, then copied up: three wraps, innermost the pocket's own.
    let deep: Vec<String> = names(&map(&model, "t0000003"))
        .into_iter()
        .map(|(_, n)| n)
        .filter(|n| {
            n.contains("t0000003.linear_pattern.")
                && n.contains("(S1 m0000001.mirror.")
                && n.contains("(S1 t0000001.linear_pattern.")
                && n.contains("(C1 q0000002.pocket.")
        })
        .collect();
    assert!(!deep.is_empty());
}

#[test]
fn a_chamfer_on_a_thrice_nested_copys_edge_resolves() {
    let model = published(&engine(), &nested());
    let edge = names(&map(&model, "t0000003"))
        .into_iter()
        .find(|(kind, n)| {
            *kind == ElementKind::Edge
                && n.starts_with("t0000003.linear_pattern.Edge(S1 m0000001.mirror.Edge(S1 t0000001.linear_pattern.Edge(C1 q0000002.pocket.")
                && !n.contains('~')
        })
        .map(|(_, n)| n)
        .unwrap();
    let chamfer = format!(
        r#"{{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Rim", "base": "t0000003", "edges": ["{edge}"], "size": {{"distance": 0.5}}}}"#
    );
    let model = published(&engine(), &with(nested(), &[chamfer]));
    assert!(
        matches!(outcome(&model, "c0000001"), FeatureOutcome::Built { .. }),
        "{edge}: {:?}",
        outcome(&model, "c0000001")
    );
}

#[test]
fn six_whole_solid_patterns_deep_every_name_is_unique_and_parses() {
    let mut features = plate();
    let mut base = "q0000002".to_owned();
    for level in 1..=6u32 {
        let feature = format!("t000000{level}");
        features.push(linear_as(
            &feature,
            &base,
            SOLID,
            &[direction("z", "spacing", &format!("{}", 5u32 << level), 2)],
            "[]",
        ));
        base = feature;
    }
    let model = published(&engine(), &features);
    let held = names_round_trip(&model, "t0000006");
    // Each level doubles the solid's elements.
    let at_base = names(&map(&model, "q0000002")).len();
    assert!(held >= at_base * 64, "{held} < 64 × {at_base}");
}

#[test]
fn a_pattern_rebuilt_twice_and_on_two_engines_names_and_places_alike() {
    let features = nested();
    let one = engine();
    let two = engine();
    let a = published(&one, &features);
    let b = published(&two, &features);
    let again = published(&one, &features);
    for feature in ["t0000001", "m0000001", "t0000003"] {
        assert_eq!(
            names(&map(&a, feature)),
            names(&map(&b, feature)),
            "{feature}"
        );
        assert_eq!(
            names(&map(&a, feature)),
            names(&map(&again, feature)),
            "{feature}"
        );
    }
    assert_eq!(copies(one.kernel()), copies(two.kernel()));
}

#[test]
fn originals_listed_in_either_order_build_one_step_and_one_set_of_names() {
    let both = |list: &str| {
        vec![
            rectangle("s0000001", "xy", [-50.0, -30.0], 100.0, 60.0),
            pad("p0000001", None, "s0000001", FIVE),
            circles("s0000003", "xy", &[([-40.0, -20.0], 2.75)]),
            pocket("z0000001", "p0000001", "s0000003", THROUGH),
            circles("s0000004", "xy", &[([-40.0, 0.0], 1.5)]),
            pocket("a0000002", "z0000001", "s0000004", THROUGH),
            linear(
                "a0000002",
                &format!(r#"{{"type": "features", "features": [{list}]}}"#),
                &[direction("x", "spacing", "20", 2)],
                "[]",
            ),
        ]
    };
    let kernel_engine = engine();
    let first = published(&kernel_engine, &both(r#""z0000001", "a0000002""#));
    let asked = copies(kernel_engine.kernel()).len();
    let second = published(&kernel_engine, &both(r#""a0000002", "z0000001""#));
    assert_eq!(
        names(&map(&first, "t0000001")),
        names(&map(&second, "t0000001"))
    );
    // The order is the timeline's, so the second is the first's steps, built already.
    assert_eq!(copies(kernel_engine.kernel()).len(), asked);
}

#[test]
fn a_cache_round_trip_rebuilds_the_patterns_steps_and_names_without_asking_the_kernel() {
    let scratch = Scratch::new("adversarial-pattern-cache");
    let features = with(
        nested(),
        &[linear_as(
            "t0000009",
            "t0000003",
            SOLID,
            &[direction("y", "spacing", "1", 1)],
            "[]",
        )],
    );
    let first = session(&scratch.0).with_sketches(FakeSketches::new());
    let a = published(&first, &features);
    assert!(!copies(first.kernel()).is_empty());
    let second = session(&scratch.0).with_sketches(FakeSketches::new());
    let b = published(&second, &features);
    assert!(copies(second.kernel()).is_empty(), "the cache held them");
    for feature in ["t0000001", "m0000001", "t0000003", "t0000009"] {
        assert_eq!(
            names(&map(&a, feature)),
            names(&map(&b, feature)),
            "{feature}"
        );
        let (
            Some(FeatureOutcome::Built { built: x, .. }),
            Some(FeatureOutcome::Built { built: y, .. }),
        ) = (a.outcome(&id(feature)), b.outcome(&id(feature)))
        else {
            panic!("{feature}");
        };
        assert_eq!(x.shape, y.shape, "{feature}");
    }
    assert_eq!(
        a.pattern_verdicts(&id("t0000003")),
        b.pattern_verdicts(&id("t0000003"))
    );
}

#[test]
fn a_rebuild_superseded_mid_pattern_leaves_the_next_one_whole() {
    let kernel_engine = engine();
    let first = with(
        plate(),
        &[linear(
            "q0000002",
            HOLE,
            &[direction("x", "spacing", "0.5", 1000)],
            "[]",
        )],
    );
    let second = with(
        plate(),
        &[linear(
            "q0000002",
            HOLE,
            &[direction("x", "spacing", "20", 3)],
            "[]",
        )],
    );
    let _superseded = kernel_engine.start(checked(&first));
    let model = published(&kernel_engine, &second);
    let held = names(&map(&model, "t0000001"));
    assert!(held.iter().any(|(_, n)| n.contains("(C2 ")));
    assert!(!held.iter().any(|(_, n)| n.contains("(C3 ")));
    let last = copies(kernel_engine.kernel()).pop().unwrap();
    assert_eq!(last.1.len(), 2);
}

// ---------------------------------------------------------------------------------------
// Failures upstream and down

#[test]
fn a_pattern_of_an_original_that_changes_nothing_is_blocked_not_failed() {
    // The hole at (200, 200), off the plate: the pocket removes nothing and fails (S7);
    // the pattern built on it is blocked on it.
    let model = published(
        &engine(),
        &with(
            plate_with_hole_at([200.0, 200.0]),
            &[linear(
                "q0000002",
                HOLE,
                &[direction("x", "spacing", "20", 2)],
                "[]",
            )],
        ),
    );
    assert!(matches!(
        outcome(&model, "q0000002"),
        FeatureOutcome::Failed(_)
    ));
    assert_eq!(
        outcome(&model, "t0000001"),
        FeatureOutcome::Blocked { on: id("q0000002") }
    );
    assert_eq!(model.errors().count(), 1);
}

#[test]
fn copies_that_all_miss_the_base_are_refused_as_changing_nothing_naming_the_pattern() {
    // Copies 200 and 400 along x, off a 100 mm plate: the fake removes nothing for them.
    let model = published(
        &engine(),
        &with(
            plate(),
            &[linear(
                "q0000002",
                HOLE,
                &[direction("x", "spacing", "200", 3)],
                "[]",
            )],
        ),
    );
    let error = failure(&model, "t0000001");
    assert_eq!(error.code(), ErrorCode::KernelFailed, "{error}");
    assert_eq!(error.feature(), Some("t0000001"));
    // One verdict, its step built.
    assert_eq!(model.pattern_verdicts(&id("t0000001")).unwrap().len(), 1);
}

#[test]
fn a_second_originals_step_that_changes_nothing_fails_the_pattern_keeping_both_verdicts() {
    // z0000001's hole at (−40, −20) copied 20 along x lands on the plate; a0000002's at
    // (40, 20) copied 20 along x lands at (60, 20), off it, and removes nothing.
    let features = vec![
        rectangle("s0000001", "xy", [-50.0, -30.0], 100.0, 60.0),
        pad("p0000001", None, "s0000001", FIVE),
        circles("s0000003", "xy", &[([-40.0, -20.0], 2.75)]),
        pocket("z0000001", "p0000001", "s0000003", THROUGH),
        circles("s0000004", "xy", &[([40.0, 20.0], 2.75)]),
        pocket("a0000002", "z0000001", "s0000004", THROUGH),
        linear(
            "a0000002",
            r#"{"type": "features", "features": ["z0000001", "a0000002"]}"#,
            &[direction("x", "spacing", "20", 2)],
            "[]",
        ),
    ];
    let model = published(&engine(), &features);
    let error = failure(&model, "t0000001");
    assert_eq!(error.code(), ErrorCode::KernelFailed, "{error}");
    let verdicts = model.pattern_verdicts(&id("t0000001")).unwrap();
    assert_eq!(verdicts.len(), 2);
    assert_eq!(verdicts[0].original, Some(id("z0000001")));
    assert_eq!(verdicts[1].original, Some(id("a0000002")));
}

#[test]
fn a_pattern_whose_base_is_suppressed_builds_on_the_base_up_its_chain() {
    // q0000005, another hole, suppressed: the pattern's effective base is q0000002.
    let mut features = plate();
    features.push(circles("s0000005", "xy", &[([0.0, 0.0], 1.0)]));
    features.push(pocket("q0000005", "q0000002", "s0000005", THROUGH).replace(
        r#""name": "Pocket", "#,
        r#""name": "Pocket", "suppressed": true, "#,
    ));
    features.push(linear(
        "q0000005",
        HOLE,
        &[direction("x", "spacing", "20", 2)],
        "[]",
    ));
    let kernel_engine = engine();
    let model = published(&kernel_engine, &features);
    assert_eq!(outcome(&model, "q0000005"), FeatureOutcome::Suppressed);
    let pattern = map(&model, "t0000001");
    let base = map(&model, "q0000002");
    for name in names(&base) {
        assert!(names(&pattern).contains(&name), "{name:?}");
    }
    assert_eq!(
        copies(kernel_engine.kernel()),
        [(
            CopiesOp::Cut,
            vec![Placement::Translation {
                offset: [20.0, 0.0, 0.0]
            }]
        )]
    );
}

#[test]
fn an_identity_pattern_passes_a_chamfers_names_through_and_a_copys_name_then_breaks() {
    // A count of 1: the identity. A chamfer on the base's edge through it builds; one on
    // copy 1's edge, which the identity hasn't, is reference_broken, never a bug.
    let base = published(&engine(), &plate());
    let edge = names(&map(&base, "q0000002"))
        .into_iter()
        .find(|(kind, n)| {
            *kind == ElementKind::Edge && n.starts_with("q0000002.") && !n.contains('~')
        })
        .map(|(_, n)| n)
        .unwrap();
    let chamfer = |edge: &str| {
        format!(
            r#"{{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Rim", "base": "t0000001", "edges": ["{edge}"], "size": {{"distance": 0.5}}}}"#
        )
    };
    let identity = linear(
        "q0000002",
        HOLE,
        &[direction("x", "spacing", "20", 1)],
        "[]",
    );
    let model = published(
        &engine(),
        &with(plate(), &[identity.clone(), chamfer(&edge)]),
    );
    assert!(matches!(
        outcome(&model, "c0000001"),
        FeatureOutcome::Built { .. }
    ));
    let copy = format!("t0000001.linear_pattern.Edge(C1 {edge})");
    let model = published(&engine(), &with(plate(), &[identity, chamfer(&copy)]));
    match outcome(&model, "c0000001") {
        FeatureOutcome::Broken { errors, .. } => {
            assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken, "{copy}")
        }
        other => panic!("{copy}: {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------
// Hangs and the cache

#[test]
fn a_copies_hang_is_kept_for_the_session_only_never_on_disk_for_the_next() {
    let scratch = Scratch::new("adversarial-pattern-hang");
    let features = with(
        plate(),
        &[linear(
            "q0000002",
            HOLE,
            &[direction("x", "spacing", "20", 2)],
            "[]",
        )],
    );
    let hung = session(&scratch.0).with_sketches(FakeSketches::new());
    hung.kernel().refuse(|op| {
        matches!(op, Op::Copies { .. }).then(|| {
            silico_core::Error::new(ErrorCode::WorkerTimeout, "the kernel worker timed out")
        })
    });
    let model = published(&hung, &features);
    assert_eq!(failure(&model, "t0000001").code(), ErrorCode::WorkerTimeout);
    // Another session on the same cache asks again, and builds.
    let next = session(&scratch.0).with_sketches(FakeSketches::new());
    let model = published(&next, &features);
    assert!(matches!(
        outcome(&model, "t0000001"),
        FeatureOutcome::Built { .. }
    ));
    assert_eq!(copies(next.kernel()).len(), 1);
}

#[test]
fn a_reference_whose_description_hangs_is_asked_again_by_the_next_session_not_kept_on_disk() {
    // The mirror's plane by name: its one-element describe times out in the first session;
    // the next, on the same cache, describes it and refuses it for its kind (the fake's
    // faces are of no surface OCCT types), never the first's timeout.
    let scratch = Scratch::new("adversarial-pattern-describe-hang");
    let plane = "p0000001.pad.Face(M s0000001.sketch.Face1)";
    let features = with(plate(), &[mirror("m0000001", "q0000002", HOLE, plane)]);
    let hung = session(&scratch.0).with_sketches(FakeSketches::new());
    hung.kernel().refuse(|op| {
        matches!(
            op,
            Op::Describe {
                count: 1,
                kind: ElementKind::Face,
                ..
            }
        )
        .then(|| silico_core::Error::new(ErrorCode::WorkerTimeout, "the kernel worker timed out"))
    });
    let model = published(&hung, &features);
    let error = failure(&model, "m0000001");
    assert_eq!(error.code(), ErrorCode::WorkerTimeout, "{error}");
    assert_eq!(error.feature(), Some("m0000001"));
    let next = session(&scratch.0).with_sketches(FakeSketches::new());
    let model = published(&next, &features);
    let error = failure(&model, "m0000001");
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert_eq!(error.parameter(), Some("plane"));
}
