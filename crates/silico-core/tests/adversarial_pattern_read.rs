//! /break-it on M6.5's `pattern` read, on the fake kernel: references that break, name the
//! wrong kind of element, or run parallel, each said once among the problems and never
//! twice; the identity, and every copy skipped; a skip past a count; the cap's thousand instances; a base whose description was
//! refused; a turn a nanodegree short of whole, rounded once; a base suppressed, read up its
//! chain as the rebuild built it; and a copies step refused, alone or the second of two.
//! Every expected value is worked by hand from the document's integers.

use std::sync::Arc;
use std::time::Duration;

use silico_core::command::pattern::{InstanceState, PatternRead, read};
use silico_core::document::{FeatureId, Length};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{ElementKind, Op};
use silico_core::recompute::{FeatureOutcome, Model, Recompute};
use silico_core::{Error, ErrorCode};

use super::pattern_docs::{
    HOLE, SOLID, THROUGH, direction, linear_as, map, mirror, plate_with_hole_at, polar, with,
};
use super::sweeps::{circles, pocket};
use super::within::within;

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

fn published(engine: &Recompute<FakeKernel>, features: &[String]) -> Arc<Model> {
    super::pattern_docs::published(engine, features, Duration::from_secs(60))
}

fn plate() -> Vec<String> {
    plate_with_hole_at([-40.0, -20.0])
}

fn linear(directions: &[String], skip: &str) -> String {
    linear_as("t0000001", "q0000002", HOLE, directions, skip)
}

fn read_of(model: &Model) -> PatternRead {
    read(model, &id("t0000001")).unwrap_or_else(|p| panic!("{:?}", p.errors()))
}

fn mm(value: f64) -> Length {
    Length::rounded(value).unwrap()
}

/// The name of an edge of the plate's top face in the hole's pocket's map: one the fake
/// describes as no line (its curves are untyped).
fn an_edge(model: &Model) -> String {
    map(model, "q0000002")
        .iter()
        .find(|(element, names)| {
            element.kind == ElementKind::Edge
                && names[0].to_string().starts_with("p0000001.pad.Edge(G ")
        })
        .map(|(_, names)| names[0].to_string())
        .unwrap()
}

#[test]
fn a_broken_reference_leaves_the_geometry_null_and_is_said_once() {
    // A name that finds nothing: the step's reference_broken, and the read's own, are one
    // error, said once; no direction, no instance.
    let found = an_edge(&published(&engine(), &plate()));
    let missing = found.replacen("(G ", "(M1/2 ", 1);
    let model = published(
        &engine(),
        &with(
            plate(),
            &[linear(&[direction(&missing, "spacing", "10", 3)], "[]")],
        ),
    );
    assert!(matches!(
        model.outcome(&id("t0000001")),
        Some(FeatureOutcome::Broken { .. })
    ));
    let read = read_of(&model);
    assert_eq!((read.directions.as_ref(), read.instances.len()), (None, 0));
    assert_eq!(read.problems_total, 1, "{:?}", read.problems);
    assert_eq!(read.problems[0].code(), ErrorCode::ReferenceBroken);
    assert_eq!(read.problems[0].parameter(), Some("directions.0.along"));
    assert!(read.problems[0].candidates().contains(&found));
    // The anchor needs no reference: it's the hole's centre all the same.
    assert_eq!(read.anchor, Some([mm(-40.0), mm(-20.0), mm(0.0)]));
}

#[test]
fn a_reference_of_the_wrong_kind_and_parallel_directions_are_said_once() {
    // An edge the fake describes as no line, as a direction: the step's refusal, once.
    let found = an_edge(&published(&engine(), &plate()));
    let model = published(
        &engine(),
        &with(
            plate(),
            &[linear(&[direction(&found, "spacing", "10", 3)], "[]")],
        ),
    );
    let read = read_of(&model);
    assert_eq!(read.problems_total, 1, "{:?}", read.problems);
    assert_eq!(read.problems[0].code(), ErrorCode::InvalidInput);
    assert!(read.directions.is_none() && read.instances.is_empty());
    // x and x reversed: parallel, once, though both resolve.
    let reverse =
        direction("x", "spacing", "10", 2).replace("\"reversed\": false", "\"reversed\": true");
    let model = published(
        &engine(),
        &with(
            plate(),
            &[linear(&[direction("x", "spacing", "10", 2), reverse], "[]")],
        ),
    );
    let read = read_of(&model);
    assert_eq!(read.problems_total, 1, "{:?}", read.problems);
    assert!(
        read.problems[0]
            .message()
            .contains("directions are parallel")
    );
    assert!(read.directions.is_none() && read.instances.is_empty());
}

#[test]
fn the_identity_and_every_copy_skipped_read_with_nothing_built_and_no_problem() {
    // A count of 1: no instance. Every copy skipped: each listed skipped. And a skip past
    // the count, kept by the document, lists nothing it doesn't place.
    let model = published(
        &engine(),
        &with(
            plate(),
            &[linear(&[direction("x", "spacing", "10", 1)], "[]")],
        ),
    );
    let read = read_of(&model);
    assert!(read.instances.is_empty());
    assert_eq!(read.problems_total, 0);
    assert_eq!(read.directions.unwrap()[0].step, Some(mm(10.0)));
    let model = published(
        &engine(),
        &with(
            plate(),
            &[linear(
                &[direction("x", "spacing", "10", 3)],
                "[[1], [2], [7]]",
            )],
        ),
    );
    let read = read_of(&model);
    let states: Vec<(String, Option<InstanceState>)> = read
        .instances
        .iter()
        .map(|i| (i.index.to_string(), i.state))
        .collect();
    assert_eq!(
        states,
        [
            ("1".to_owned(), Some(InstanceState::Skipped)),
            ("2".to_owned(), Some(InstanceState::Skipped)),
        ]
    );
    assert_eq!(read.problems_total, 0);
}

#[test]
fn a_thousand_instances_are_read_whole_in_order_and_promptly() {
    // The cap: 40 by 25, 999 instances besides the original, in skip's order, each placed
    // from its index: (i, j) at (−40 + 0.5i, −20 + 0.5j, 0), j > 0 written as a pair.
    let model = published(
        &engine(),
        &with(
            plate(),
            &[linear(
                &[
                    direction("x", "spacing", "0.5", 40),
                    direction("y", "spacing", "0.5", 25),
                ],
                "[]",
            )],
        ),
    );
    let read = within(Duration::from_secs(5), move || read_of(&model));
    assert_eq!(read.instances.len(), 999);
    let mut want = Vec::new();
    for i in 0..40u32 {
        for j in 0..25u32 {
            if (i, j) != (0, 0) {
                want.push((i, j));
            }
        }
    }
    for (instance, (i, j)) in read.instances.iter().zip(want) {
        let offsets: Vec<u32> = if j == 0 { vec![i] } else { vec![i, j] };
        assert_eq!(instance.index.offsets(), offsets.as_slice());
        assert_eq!(
            instance.anchor,
            Some([
                mm(-40.0 + 0.5 * f64::from(i)),
                mm(-20.0 + 0.5 * f64::from(j)),
                mm(0.0)
            ])
        );
    }
}

#[test]
fn a_base_whose_description_was_refused_says_so_for_a_named_reference() {
    // Every description of a whole solid refused: a pattern whose direction is an edge's
    // name can't be read from its base's description, so the read says why, as `elements`
    // would of that solid, beside the step's own refusal; one along x needs none.
    let engine = engine();
    engine.kernel().refuse(|op| {
        matches!(op, Op::DescribeAll { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "OCCT couldn't describe it"))
    });
    let found = an_edge(&published(&self::engine(), &plate()));
    let model = published(
        &engine,
        &with(
            plate(),
            &[linear(&[direction(&found, "spacing", "10", 3)], "[]")],
        ),
    );
    let read = read_of(&model);
    assert!(read.directions.is_none() && read.instances.is_empty());
    let said: Vec<(&str, Option<&str>)> = read
        .problems
        .iter()
        .map(|e| (e.message(), e.feature()))
        .collect();
    assert!(
        said.iter()
            .any(|(m, f)| m.contains("OCCT couldn't describe it") && *f == Some("q0000002")),
        "{said:?}"
    );
    assert_eq!(read.problems_total, read.problems.len() as u64);
    let model = published(
        &engine,
        &with(
            plate(),
            &[linear(&[direction("x", "spacing", "10", 3)], "[]")],
        ),
    );
    let read = read_of(&model);
    assert_eq!(read.instances.len(), 2);
    assert_eq!(read.problems_total, 0, "{:?}", read.problems);
}

#[test]
fn a_polar_pattern_turned_by_a_nanodegree_short_of_a_whole_turn_reads_its_steps_exactly() {
    // 359.999999999° over 4, not a whole turn: steps of a third of it, k · 359999999999 / 3
    // nanodegrees, each rounded once at the end: 119999999999.667 to 120°, 239999999999.333
    // to 239.999999999°, and 359.999999999° exactly.
    let model = published(
        &engine(),
        &with(
            plate(),
            &[polar("t0000001", "q0000002", HOLE, "z", "359.999999999", 4)],
        ),
    );
    let read = read_of(&model);
    let angles: Vec<i64> = read
        .instances
        .iter()
        .map(|i| i.angle.unwrap().ndeg())
        .collect();
    assert_eq!(angles, [120_000_000_000, 239_999_999_999, 359_999_999_999]);
    assert_eq!(
        read.axis.unwrap().step.map(|a| a.ndeg()),
        Some(120_000_000_000)
    );
}

#[test]
fn a_pattern_whose_base_is_suppressed_is_read_on_the_base_up_its_chain() {
    // A whole-solid mirror in yz on a suppressed pocket builds on the corner hole's pocket,
    // as the rebuild takes it: read alike, its anchor the same centre of mass, with no
    // problem; and alike to the mirror on that pocket itself.
    let mut features = plate();
    features.push(circles("s0000005", "xy", &[([0.0, 0.0], 1.0)]));
    features.push(pocket("q0000005", "q0000002", "s0000005", THROUGH).replace(
        r#""name": "Pocket", "#,
        r#""name": "Pocket", "suppressed": true, "#,
    ));
    let on_suppressed = read_of(&published(
        &engine(),
        &with(features, &[mirror("t0000001", "q0000005", SOLID, "yz")]),
    ));
    let direct = read_of(&published(
        &engine(),
        &with(plate(), &[mirror("t0000001", "q0000002", SOLID, "yz")]),
    ));
    assert_eq!(
        on_suppressed.problems_total, 0,
        "{:?}",
        on_suppressed.problems
    );
    assert!(on_suppressed.anchor.is_some());
    assert_eq!(on_suppressed.anchor, direct.anchor);
    assert_eq!(on_suppressed.instances, direct.instances);
    assert_eq!(on_suppressed.instances[0].state, Some(InstanceState::Built));
}

#[test]
fn a_copies_step_refused_whole_or_one_of_two_gives_its_copies_no_state() {
    // The kernel refuses the copies: no step's verdict, so no copy has a state, and the
    // refusal is the problem.
    let engine = engine();
    engine.kernel().refuse(|op| {
        matches!(op, Op::Copies { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "OCCT couldn't copy it"))
    });
    let model = published(
        &engine,
        &with(
            plate(),
            &[linear(&[direction("x", "spacing", "10", 3)], "[]")],
        ),
    );
    let read = read_of(&model);
    let states: Vec<(String, Option<InstanceState>)> = read
        .instances
        .iter()
        .map(|i| (i.index.to_string(), i.state))
        .collect();
    assert_eq!(states, [("1".to_owned(), None), ("2".to_owned(), None)]);
    assert!(
        read.problems
            .iter()
            .any(|e| e.message().contains("OCCT couldn't copy it")),
        "{:?}",
        read.problems
    );
    // Two originals, the second's step refused: the first's verdict kept, but one step of
    // two never gave its own, so no copy has a state.
    let engine = self::engine();
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&seen);
    engine.kernel().refuse(move |op| {
        (matches!(op, Op::Copies { .. })
            && counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1)
            .then(|| Error::new(ErrorCode::KernelFailed, "OCCT couldn't copy the second"))
    });
    let mut features = plate();
    features.push(circles("s0000005", "xy", &[([0.0, 20.0], 1.0)]));
    features.push(pocket("q0000005", "q0000002", "s0000005", THROUGH));
    let both = r#"{"type": "features", "features": ["q0000002", "q0000005"]}"#;
    let model = published(
        &engine,
        &with(
            features,
            &[linear_as(
                "t0000001",
                "q0000005",
                both,
                &[direction("x", "spacing", "10", 3)],
                "[]",
            )],
        ),
    );
    assert_eq!(
        model.pattern_verdicts(&id("t0000001")).map(<[_]>::len),
        Some(1)
    );
    let read = read_of(&model);
    let states: Vec<(String, Option<InstanceState>)> = read
        .instances
        .iter()
        .map(|i| (i.index.to_string(), i.state))
        .collect();
    assert_eq!(states, [("1".to_owned(), None), ("2".to_owned(), None)]);
    let said = read
        .problems
        .iter()
        .filter(|e| e.message().contains("OCCT couldn't copy the second"))
        .count();
    assert_eq!(said, 1, "{:?}", read.problems);
}
