//! The council's round 3 (architecture's and robustness's proofs): a step's verdict gate
//! was its first sharer's, but an M2 boolean and a sweep feature can share one step with
//! different checks (step keys hold no IDs: a baseless reversed pad makes the pocket's
//! prism). Each feature's verdict is now its own, and a step only refused features'
//! descendants need isn't started; every outcome is the same fresh and warm, and nothing
//! is ever reported as a step that wasn't run.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/sweeps.rs"]
mod sweeps;

use std::sync::Arc;
use std::time::Duration;

use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use sweeps::*;

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

fn published(engine: &Recompute<FakeKernel>, features: &[&str]) -> Arc<Model> {
    let document = checked_document(features);
    match engine.start(document).wait_timeout(Duration::from_secs(60)) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

const REVERSED: &str = r#"{"type": "one_side", "length": 10, "reversed": true}"#;

fn features(cut_first: bool) -> Vec<String> {
    let cube_b = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let cube_c = box_at("cccccccc", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    // A baseless pad, reversed: the same prism as a pocket of the sketch (downward).
    let pad = pad("pppppppp", None, "ssssssss", REVERSED);
    // The pocket misses its cube: refused, removes nothing.
    let pocket = pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN);
    // An M2 cut of the twin cube by the pad: the same step as the pocket's cut, one solid,
    // which M2 builds.
    let cut = r#"{"id": "kkkkkkkk", "type": "cut", "v": 1, "name": "K", "base": "cccccccc", "tool": "pppppppp"}"#.to_owned();
    let fillet = r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "kkkkkkkk", "edges": ["cccccccc.box.Edge1"], "radius": 0.1}"#.to_owned();
    if cut_first {
        vec![cube_b, cube_c, sketch, pad, cut, pocket, fillet]
    } else {
        vec![cube_b, cube_c, sketch, pad, pocket, cut, fillet]
    }
}

#[test]
fn a_valid_m2_cut_sharing_a_refused_pockets_step_still_lets_its_fillet_build() {
    let engine = engine();
    let f = features(false);
    let refs: Vec<&str> = f.iter().map(String::as_str).collect();
    let model = published(&engine, &refs);
    assert!(
        matches!(
            model.outcome(&id("qqqqqqqq")),
            Some(FeatureOutcome::Failed(_))
        ),
        "pocket: {:?}",
        model.outcome(&id("qqqqqqqq"))
    );
    assert!(
        matches!(
            model.outcome(&id("kkkkkkkk")),
            Some(FeatureOutcome::Built { .. })
        ),
        "cut: {:?}",
        model.outcome(&id("kkkkkkkk"))
    );
    assert!(
        matches!(
            model.outcome(&id("ffffffff")),
            Some(FeatureOutcome::Built { .. })
        ),
        "fillet of a built cut: {:?}",
        model.outcome(&id("ffffffff"))
    );
}

#[test]
fn a_valid_m2_cut_and_its_fillet_build_with_the_cut_first() {
    let engine = engine();
    let f = features(true);
    let refs: Vec<&str> = f.iter().map(String::as_str).collect();
    let model = published(&engine, &refs);
    assert!(
        matches!(
            model.outcome(&id("kkkkkkkk")),
            Some(FeatureOutcome::Built { .. })
        ),
        "cut: {:?}",
        model.outcome(&id("kkkkkkkk"))
    );
    assert!(
        matches!(
            model.outcome(&id("ffffffff")),
            Some(FeatureOutcome::Built { .. })
        ),
        "fillet of a built cut: {:?}",
        model.outcome(&id("ffffffff"))
    );
}

#[test]
fn nothing_is_built_on_a_refused_pocket_sharing_an_m2_cuts_step() {
    let engine = engine();
    let mut f = features(true);
    f.push(pocket("rrrrrrrr", "qqqqqqqq", "ssssssss", TEN));
    let refs: Vec<&str> = f.iter().map(String::as_str).collect();
    let model = published(&engine, &refs);
    assert!(matches!(
        model.outcome(&id("qqqqqqqq")),
        Some(FeatureOutcome::Failed(_))
    ));
    assert_eq!(
        model.outcome(&id("rrrrrrrr")),
        Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
    );
    let cuts = |engine: &Recompute<FakeKernel>| {
        engine
            .kernel()
            .calls()
            .into_iter()
            .filter(|call| call.op.name() == "Cut")
            .count()
    };
    // The shared step, and perhaps the pocket on the refused pocket, which may start before
    // the verdict lands (its users start as soon as the shared cut is built, beside its
    // measure: round 3); none after it.
    assert!((1..=2).contains(&cuts(&engine)));
    // With the verdict known before the plan (the same document, kept), a pocket on the
    // refused one, another step, is never started.
    let warm = self::engine();
    let f = features(true);
    let refs: Vec<&str> = f.iter().map(String::as_str).collect();
    published(&warm, &refs);
    assert_eq!(cuts(&warm), 1);
    let mut g = features(true);
    g.push(pocket("rrrrrrrr", "qqqqqqqq", "ssssssss", &one_side("3")));
    let refs: Vec<&str> = g.iter().map(String::as_str).collect();
    let model = published(&warm, &refs);
    assert_eq!(
        model.outcome(&id("rrrrrrrr")),
        Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
    );
    assert_eq!(cuts(&warm), 1);
}

/// Robustness's proof: boxes B and D alike above the XY plane, a small box F inside B,
/// sketch S, a baseless pad P reversed, a pocket Q on D (missing it: refused), the cut
/// C = cut(B, P) (Q's step), and E = cut(C, F). E builds, fresh and warm, in either order of
/// Q and C.
#[test]
fn robust_proof_an_m2_cuts_user_runs_whatever_pocket_shares_its_step() {
    for q_first in [true, false] {
        let mut f = vec![
            box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0),
            box_at("dddddddd", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0),
            box_at("ffffffff", [2.0, 2.0, 3.0], 1.0, 1.0, 1.0),
            rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0),
            pad("pppppppp", None, "ssssssss", REVERSED),
        ];
        let pocket = pocket("qqqqqqqq", "dddddddd", "ssssssss", TEN);
        let cut = r#"{"id": "cccccccc", "type": "cut", "v": 1, "name": "C", "base": "bbbbbbbb", "tool": "pppppppp"}"#.to_owned();
        if q_first {
            f.extend([pocket, cut]);
        } else {
            f.extend([cut, pocket]);
        }
        f.push(r#"{"id": "eeeeeeee", "type": "cut", "v": 1, "name": "E", "base": "cccccccc", "tool": "ffffffff"}"#.to_owned());
        let refs: Vec<&str> = f.iter().map(String::as_str).collect();
        let engine = engine();
        for run in ["fresh", "warm"] {
            let model = published(&engine, &refs);
            assert!(
                matches!(
                    model.outcome(&id("qqqqqqqq")),
                    Some(FeatureOutcome::Failed(_))
                ),
                "{run}, q first {q_first}: {:?}",
                model.outcome(&id("qqqqqqqq"))
            );
            for built in ["cccccccc", "eeeeeeee"] {
                assert!(
                    matches!(
                        model.outcome(&id(built)),
                        Some(FeatureOutcome::Built { .. })
                    ),
                    "{run}, q first {q_first}, {built}: {:?}",
                    model.outcome(&id(built))
                );
            }
        }
    }
}
