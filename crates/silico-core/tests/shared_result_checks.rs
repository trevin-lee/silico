//! The council's round-1 finding (proven): a feature's result check belonged to a step's
//! key, so a step shared by features alike (its key holds no ID), or had from memory or the
//! cache, was checked for the wrong feature or not at all. Each check is now the feature's,
//! decided when its outcome is, from its solid's stored measures and its base's. Its two
//! proofs, then each path (fresh, warm in memory, the disk cache) for a sweep feature's
//! no-op refusal: the same refusal, in the feature's own words, once.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::model::WAIT;
use super::sweeps;

use std::sync::Arc;

use silico_core::cache::Cache;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use sweeps::*;

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

fn published(engine: &Recompute<FakeKernel>, features: &[&str]) -> Arc<Model> {
    let document = checked_document(features);
    match engine.start(document).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

const REVERSED: &str = r#"{"type": "one_side", "length": 10, "reversed": true}"#;

fn cut(id: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "cut", "v": 1, "name": "C", "base": "{base}", "tool": "{tool}"}}"#
    )
}

/// The same document's pocket is refused by a fresh engine, but built by one that ran an
/// M2 cut of the same step first: the check never runs on a result already kept.
#[test]
fn a_pockets_check_holds_whatever_ran_before() {
    let cube_above = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    let pocket_doc = [
        cube_above.as_str(),
        sketch.as_str(),
        &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
    ];
    let fresh = engine();
    let model = published(&fresh, &pocket_doc);
    let fresh_outcome = model.outcome(&id("qqqqqqqq")).cloned();
    assert!(
        matches!(fresh_outcome, Some(FeatureOutcome::Failed(_))),
        "{fresh_outcome:?}"
    );

    let warm = engine();
    let first = published(
        &warm,
        &[
            &cube_above,
            &sketch,
            &pad("pppppppp", None, "ssssssss", REVERSED),
            &cut("cccccccc", "bbbbbbbb", "pppppppp"),
        ],
    );
    assert!(
        matches!(
            first.outcome(&id("cccccccc")),
            Some(FeatureOutcome::Built { .. })
        ),
        "{:?}",
        first.features()
    );
    let model = published(&warm, &pocket_doc);
    let warm_outcome = model.outcome(&id("qqqqqqqq")).cloned();
    assert!(
        matches!(warm_outcome, Some(FeatureOutcome::Failed(_))),
        "same document, fresh engine: {fresh_outcome:?}; warm engine: {warm_outcome:?}"
    );
}

/// An M2 cut that removes nothing builds, unless a pocket elsewhere in the document shares
/// its step, when it fails in the pocket's words.
#[test]
fn an_m2_cut_isnt_refused_by_an_unrelated_pockets_check() {
    let cube_above = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let cube_twin = box_at("dddddddd", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    let alone = published(
        &engine(),
        &[
            &cube_above,
            &sketch,
            &pad("pppppppp", None, "ssssssss", REVERSED),
            &cut("cccccccc", "bbbbbbbb", "pppppppp"),
        ],
    );
    assert!(matches!(
        alone.outcome(&id("cccccccc")),
        Some(FeatureOutcome::Built { .. })
    ));
    let with_pocket = published(
        &engine(),
        &[
            &cube_above,
            &cube_twin,
            &sketch,
            &pad("pppppppp", None, "ssssssss", REVERSED),
            &cut("cccccccc", "bbbbbbbb", "pppppppp"),
            &pocket("qqqqqqqq", "dddddddd", "ssssssss", TEN),
        ],
    );
    let outcome = with_pocket.outcome(&id("cccccccc")).cloned();
    assert!(
        matches!(outcome, Some(FeatureOutcome::Built { .. })),
        "{outcome:?}"
    );
    // The premise: the pocket sharing the cut's step is refused, removing nothing.
    let Some(FeatureOutcome::Failed(error)) = with_pocket.outcome(&id("qqqqqqqq")) else {
        panic!("{:?}", with_pocket.outcome(&id("qqqqqqqq")));
    };
    assert!(error.message().contains("removes nothing"), "{error:?}");
}

/// The pocket of the proofs, missing its cube: refused, its error said once, naming it.
fn refused_once(model: &Model) -> String {
    let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("qqqqqqqq")) else {
        panic!("{:?}", model.outcome(&id("qqqqqqqq")));
    };
    assert_eq!(error.feature(), Some("qqqqqqqq"));
    assert!(
        error
            .message()
            .starts_with("pocket qqqqqqqq removes nothing from its base bbbbbbbb"),
        "{}",
        error.message()
    );
    assert_eq!(
        model.errors().count(),
        1,
        "{:?}",
        model.errors().collect::<Vec<_>>()
    );
    error.message().to_owned()
}

#[test]
fn a_pockets_no_op_refusal_is_the_same_fresh_warm_and_from_the_cache() {
    let cube_above = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    let document = [
        cube_above.as_str(),
        sketch.as_str(),
        &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
    ];
    // The cache and its secret beside it (not in it, as a user's is), removed however the
    // test ends.
    let temp = tempfile::Builder::new()
        .prefix("silico-shared-checks-")
        .tempdir()
        .unwrap();
    let dir = temp.path().join("cache");
    let secret = temp.path().join("secret");
    let session = || {
        Recompute::with_cache(
            FakeKernel::new(2).unwrap(),
            Cache::with_secret(&dir, &secret),
        )
        .with_sketches(FakeSketches::new())
    };
    // Fresh: everything made.
    let first = session();
    let fresh = refused_once(&published(&first, &document));
    // Warm: every step kept in memory, nothing asked again.
    let calls = first.kernel().calls().len();
    assert_eq!(refused_once(&published(&first, &document)), fresh);
    assert_eq!(first.kernel().calls().len(), calls);
    drop(first);
    // From the disk cache: a new session, every step read back, no kernel call.
    let second = session();
    assert_eq!(refused_once(&published(&second, &document)), fresh);
    assert_eq!(second.kernel().calls(), []);
}
