//! The council's round 3 (architecture's and robustness's proofs): a step's verdict gate
//! was its first sharer's, but an M2 boolean and a sweep feature can share one step (step
//! keys hold no IDs: a baseless reversed pad makes the pocket's prism). Each feature's
//! verdict is its own, in its own words, and a step only refused features' descendants need
//! isn't started; every outcome is the same fresh and warm, and nothing is ever reported as
//! a step that wasn't run. Since M2 booleans take S7's rule, the two checks agree on a
//! shared step: an M2 cut sharing a refused pocket's step is refused too, and what's made
//! from either is blocked.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::model::WAIT;
use super::sweeps;

use std::sync::Arc;
use std::time::Duration;

use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use sweeps::*;

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

/// An engine whose kernel can't measure a cut of a base that isn't itself a cut (the shared
/// cut of the cube by the prism), saying so, as a kernel refuses a measure for its values:
/// the pocket sharing that step fails unmeasured, and an M2 cut of it builds, its change
/// unjudged (`checked`): so a shared step's features come to different outcomes, and the
/// pocket's users are blocked on it while the cut's build.
fn unmeasured_engine() -> Recompute<FakeKernel> {
    let kernel = FakeKernel::new(2).unwrap();
    kernel.refuse(|op| match op {
        silico_protocol::Op::Summarize { shape } => {
            let made = FakeKernel::describe(shape)?;
            (made.starts_with("cut(") && made.matches("cut(").count() == 1).then(|| {
                silico_core::Error::new(
                    silico_core::ErrorCode::KernelFailed,
                    "the cut couldn't be measured",
                )
            })
        }
        _ => None,
    });
    Recompute::new(kernel).with_sketches(FakeSketches::new())
}

/// That `model`'s pocket `qqqqqqqq` failed, unmeasured, naming itself.
fn pocket_unmeasured(model: &Model) {
    let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("qqqqqqqq")) else {
        panic!("pocket: {:?}", model.outcome(&id("qqqqqqqq")));
    };
    assert_eq!(error.feature(), Some("qqqqqqqq"), "{error:?}");
    assert!(
        error.message().contains("the cut couldn't be measured"),
        "{error:?}"
    );
}

/// That `feature` built in `model`.
fn built(model: &Model, feature: &str) {
    assert!(
        matches!(
            model.outcome(&id(feature)),
            Some(FeatureOutcome::Built { .. })
        ),
        "{feature}: {:?}",
        model.outcome(&id(feature))
    );
}

#[test]
fn a_valid_m2_cut_sharing_a_refused_pockets_step_still_lets_its_fillet_build() {
    // The pocket first: it fails, unmeasured; the M2 cut sharing its step builds, its change
    // unjudged, and so does its fillet. Fresh and warm.
    let engine = unmeasured_engine();
    let f = features(false);
    let refs: Vec<&str> = f.iter().map(String::as_str).collect();
    for _ in ["fresh", "warm"] {
        let model = published(&engine, &refs);
        pocket_unmeasured(&model);
        built(&model, "kkkkkkkk");
        built(&model, "ffffffff");
    }
}

#[test]
fn a_valid_m2_cut_and_its_fillet_build_with_the_cut_first() {
    let engine = unmeasured_engine();
    let f = features(true);
    let refs: Vec<&str> = f.iter().map(String::as_str).collect();
    for _ in ["fresh", "warm"] {
        let model = published(&engine, &refs);
        pocket_unmeasured(&model);
        built(&model, "kkkkkkkk");
        built(&model, "ffffffff");
    }
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

fn features(cut_first: bool) -> Vec<String> {
    let cube_b = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let cube_c = box_at("cccccccc", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    // A baseless pad, reversed: the same prism as a pocket of the sketch (downward).
    let pad = pad("pppppppp", None, "ssssssss", REVERSED);
    // The pocket misses its cube: refused, removes nothing.
    let pocket = pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN);
    // An M2 cut of the twin cube by the pad: the same step as the pocket's cut, refused as
    // it is, removing nothing (S7's rule, for M2 booleans too).
    let cut = r#"{"id": "kkkkkkkk", "type": "cut", "v": 1, "name": "K", "base": "cccccccc", "tool": "pppppppp"}"#.to_owned();
    let fillet = r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "kkkkkkkk", "edges": ["cccccccc.box.Edge1"], "radius": 0.1}"#.to_owned();
    if cut_first {
        vec![cube_b, cube_c, sketch, pad, cut, pocket, fillet]
    } else {
        vec![cube_b, cube_c, sketch, pad, pocket, cut, fillet]
    }
}

/// That `model`'s pocket and M2 cut, sharing a step, are each refused in its own words,
/// naming itself, and the fillet of the cut is blocked on it.
fn both_refused_and_the_fillet_blocked(model: &Model) {
    for (feature, words) in [
        (
            "qqqqqqqq",
            "pocket qqqqqqqq removes nothing from its base bbbbbbbb",
        ),
        (
            "kkkkkkkk",
            "cut kkkkkkkk removes nothing from its base cccccccc",
        ),
    ] {
        let Some(FeatureOutcome::Failed(error)) = model.outcome(&id(feature)) else {
            panic!("{feature}: {:?}", model.outcome(&id(feature)));
        };
        assert_eq!(error.feature(), Some(feature), "{error:?}");
        assert!(error.message().starts_with(words), "{error:?}");
    }
    assert_eq!(
        model.outcome(&id("ffffffff")),
        Some(&FeatureOutcome::Blocked { on: id("kkkkkkkk") })
    );
}

#[test]
fn an_m2_cut_sharing_a_refused_pockets_step_is_refused_too_and_its_fillet_blocked() {
    let engine = engine();
    let f = features(false);
    let refs: Vec<&str> = f.iter().map(String::as_str).collect();
    both_refused_and_the_fillet_blocked(&published(&engine, &refs));
    // Warm, every step kept: the same.
    both_refused_and_the_fillet_blocked(&published(&engine, &refs));
}

#[test]
fn an_m2_cut_and_its_fillet_with_the_cut_first_are_refused_and_blocked_alike() {
    let engine = engine();
    let f = features(true);
    let refs: Vec<&str> = f.iter().map(String::as_str).collect();
    both_refused_and_the_fillet_blocked(&published(&engine, &refs));
    both_refused_and_the_fillet_blocked(&published(&engine, &refs));
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
    // Fresh, the pocket on the refused one may start before the verdict lands (its users
    // start as soon as the shared cut is built, beside its measure: round 3), so how many
    // cuts ran isn't pinned here; a_verdict_that_lands_mid_recompute_stops_what_hasnt_started
    // pins it with the order held. With the verdict known before the plan (the same document, kept), a pocket on the
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
/// C = cut(B, P) (Q's step: refused too, S7's rule), and E = cut(C, F). E is blocked on C,
/// fresh and warm, in either order of Q and C: each user of the shared step sees its own
/// base's verdict.
#[test]
fn robust_proof_an_m2_cuts_user_is_blocked_on_it_whatever_pocket_shares_its_step() {
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
            for (refused, words) in [
                ("qqqqqqqq", "pocket qqqqqqqq removes nothing"),
                ("cccccccc", "cut cccccccc removes nothing"),
            ] {
                let Some(FeatureOutcome::Failed(error)) = model.outcome(&id(refused)) else {
                    panic!(
                        "{run}, q first {q_first}, {refused}: {:?}",
                        model.outcome(&id(refused))
                    );
                };
                assert!(error.message().starts_with(words), "{run}: {error:?}");
            }
            assert_eq!(
                model.outcome(&id("eeeeeeee")),
                Some(&FeatureOutcome::Blocked { on: id("cccccccc") }),
                "{run}, q first {q_first}"
            );
        }
    }
}

/// Robustness's proof, its premise reached by an unmeasured shared step: boxes B and D
/// alike above the XY plane, a small box F inside B, sketch S, a baseless pad P reversed, a
/// pocket Q on D (failing, unmeasured), the cut C = cut(B, P) (Q's step: built, its change
/// unjudged), and E = cut(C, F). E builds, fresh and warm, in either order of Q and C.
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
        let engine = unmeasured_engine();
        for _ in ["fresh", "warm"] {
            let model = published(&engine, &refs);
            pocket_unmeasured(&model);
            built(&model, "cccccccc");
            built(&model, "eeeeeeee");
        }
    }
}

/// The number of cuts `engine`'s kernel was asked for.
fn cuts(engine: &Recompute<FakeKernel>) -> usize {
    engine
        .kernel()
        .calls()
        .iter()
        .filter(|call| call.op.name() == "Cut")
        .count()
}

/// Whether the cache under `dir` holds an entry whose shape has a step `word` (`"\ncut "`,
/// `"\ntranslate "`: the fake writes a shape as its steps, a line each, a boolean's with
/// all of both its inputs') and none `without`. A
/// feature's solid is written to the cache only once it's measured, after its measure's
/// verdict is decided: so its entry there means the scheduler has judged it. A step that's
/// no feature's solid (the box a moved box is made from) is kept as soon as it's made.
fn cached(dir: &std::path::Path, word: &[u8], without: Option<&[u8]>) -> bool {
    let has = |bytes: &[u8], word: &[u8]| bytes.windows(word.len()).any(|w| w == word);
    let Ok(dirs) = std::fs::read_dir(dir) else {
        return false;
    };
    dirs.flatten()
        .filter(|e| e.path().is_dir())
        .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().flatten())
        .filter_map(|e| std::fs::read(e.path()).ok())
        .any(|bytes| has(&bytes, word) && without.is_none_or(|w| !has(&bytes, w)))
}

/// Waits up to [`WAIT`] for `ready`.
fn wait_for(what: &str, ready: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + WAIT;
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_verdict_that_lands_mid_recompute_stops_what_hasnt_started() {
    // The council's round 4: three pockets chained on the cube above the XY plane, each
    // going down and missing it. The first's cut is made; its users start at once, so the
    // second's cut starts (held) while the first's measure is held. The measure is let go,
    // and once the cube's and the first cut's entries are on disk (written after their
    // measures are judged), the first pocket is refused and the second's cut let go: the
    // third's cut, which only refused features need, is never started. Exactly two cuts.
    let temp = tempfile::Builder::new()
        .prefix("silico-verdict-mid-recompute-")
        .tempdir()
        .unwrap();
    let dir = temp.path();
    let kernel = FakeKernel::new(3).unwrap();
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let second = kernel.hold(move |op| {
        op.name() == "Cut" && seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
    });
    let measure = kernel.hold(|op| match op {
        silico_protocol::Op::Summarize { shape } => {
            FakeKernel::describe(shape).is_some_and(|made| made.starts_with("cut("))
        }
        _ => false,
    });
    let engine = Recompute::with_cache(
        kernel,
        silico_core::cache::Cache::with_secret(dir.join("cache"), &dir.join("secret")),
    )
    .with_sketches(FakeSketches::new());
    let cube_above = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let s = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    let q = pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN);
    let r = pocket("rrrrrrrr", "qqqqqqqq", "ssssssss", TEN);
    let t = pocket("tttttttt", "rrrrrrrr", "ssssssss", TEN);
    let run = engine.start(checked_document(&[&cube_above, &s, &q, &r, &t]));
    wait_for("the second cut didn't start", || cuts(&engine) == 2);
    drop(measure);
    // The cube's own entry, its box moved into place and no cut. "A box" was met before the
    // cube was measured, twice over: by the box made at the origin, a step of its own kept
    // as soon as it's made, and by the first cut's entry, which holds its base's steps too.
    // With the cube's measure late, the first pocket wasn't yet refused when the second cut
    // was let go: the third started (run 40185654927, its first attempt).
    wait_for("the cube and the first cut weren't measured", || {
        cached(&dir.join("cache"), b"\ncut ", None)
            && cached(&dir.join("cache"), b"\ntranslate ", Some(b"\ncut "))
    });
    drop(second);
    let Ok(Ok(RunOutcome::Published(model))) = run.wait_timeout(WAIT) else {
        panic!("not published");
    };
    assert!(matches!(
        model.outcome(&id("qqqqqqqq")),
        Some(FeatureOutcome::Failed(_))
    ));
    for blocked in ["rrrrrrrr", "tttttttt"] {
        assert_eq!(
            model.outcome(&id(blocked)),
            Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
        );
    }
    assert_eq!(cuts(&engine), 2, "the third cut started after the verdict");
}

#[test]
fn a_step_only_dead_features_share_is_never_run() {
    // The council's round 4, its premise moved by S7's rule for M2 booleans: boxes B and C
    // alike above XY, sketch S, a baseless reversed pad P (the pocket's prism), the M2 cut
    // K = cut(C, P) and the pocket Q on B, sharing one step, both refused (each removes
    // nothing). Then, on the same engine, the same reversed pocket G on Q and F on K: one cut
    // step, which both need and both are dead for, their bases refused. It's never run; G is
    // blocked on Q, F on K. (A live feature sharing a step with a dead one no longer arises:
    // a shared step's verdict is the same for every feature.)
    let engine = engine();
    let base = [
        box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0),
        box_at("cccccccc", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0),
        rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0),
        pad("pppppppp", None, "ssssssss", REVERSED),
        r#"{"id": "kkkkkkkk", "type": "cut", "v": 1, "name": "K", "base": "cccccccc", "tool": "pppppppp"}"#.to_owned(),
        pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
    ];
    let refs: Vec<&str> = base.iter().map(String::as_str).collect();
    let model = published(&engine, &refs);
    for refused in ["qqqqqqqq", "kkkkkkkk"] {
        assert!(
            matches!(model.outcome(&id(refused)), Some(FeatureOutcome::Failed(_))),
            "{refused}: {:?}",
            model.outcome(&id(refused))
        );
    }
    let before = cuts(&engine);
    let g = pocket("gggggggg", "qqqqqqqq", "ssssssss", REVERSED);
    let f = pocket("ffffffff", "kkkkkkkk", "ssssssss", REVERSED);
    let mut more = refs.clone();
    more.extend([g.as_str(), f.as_str()]);
    let model = published(&engine, &more);
    assert_eq!(cuts(&engine) - before, 0, "the shared cut, never");
    assert_eq!(
        model.outcome(&id("gggggggg")),
        Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
    );
    assert_eq!(
        model.outcome(&id("ffffffff")),
        Some(&FeatureOutcome::Blocked { on: id("kkkkkkkk") })
    );
}

#[test]
fn a_step_a_live_feature_shares_with_a_dead_one_is_run() {
    // The council's round 4's outcomes, reached by an unmeasured shared step: boxes B and C
    // alike above XY, sketch S, a baseless reversed pad P (the pocket's prism), the M2 cut
    // K = cut(C, P) (built, its change unjudged), and the pocket Q on B (failed: unmeasured;
    // K's step). Then, on the same engine, the reversed pocket G on Q and the M2 cut F of K
    // by V, a baseless pad of the sketch up 10 (G's prism): one cut step, F's and G's. It's
    // run once; G is blocked on Q, F built. Q failed unmeasured, not refused by its check,
    // so G isn't dead to the scheduler (only a verdict kills): the step a live feature shares
    // with a dead one is `a_step_a_live_pad_shares_with_a_dead_pocket_is_run_once`'s.
    let engine = unmeasured_engine();
    let base = [
        box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0),
        box_at("cccccccc", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0),
        rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0),
        pad("pppppppp", None, "ssssssss", REVERSED),
        pad("vvvvvvvv", None, "ssssssss", &one_side("10")),
        r#"{"id": "kkkkkkkk", "type": "cut", "v": 1, "name": "K", "base": "cccccccc", "tool": "pppppppp"}"#.to_owned(),
        pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
    ];
    let refs: Vec<&str> = base.iter().map(String::as_str).collect();
    let model = published(&engine, &refs);
    pocket_unmeasured(&model);
    built(&model, "kkkkkkkk");
    let before = cuts(&engine);
    let g = pocket("gggggggg", "qqqqqqqq", "ssssssss", REVERSED);
    let f = r#"{"id": "ffffffff", "type": "cut", "v": 1, "name": "F", "base": "kkkkkkkk", "tool": "vvvvvvvv"}"#;
    let mut more = refs.clone();
    more.extend([g.as_str(), f]);
    let model = published(&engine, &more);
    assert_eq!(cuts(&engine) - before, 1, "the shared cut, once");
    assert_eq!(
        model.outcome(&id("gggggggg")),
        Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
    );
    built(&model, "ffffffff");
}

#[test]
fn a_step_a_live_pad_shares_with_a_dead_pocket_is_run_once() {
    // A step a dead feature and a live one share is run, for the live one (`needed`'s
    // any-owner): on a fresh engine, the cube B above XY, sketch S, and the pocket Q of S
    // down 10 into B, which misses it: refused, removing nothing. Then, on the same engine,
    // the reversed pocket G on Q (up 10: dead, made from a refused feature) and the baseless
    // pad W of S up 10 (live): a sweep feature owns its faces and prism steps, which carry no
    // check, and the reversed pocket's prism is the pad's, one step. It's run once; W is
    // built, G blocked on Q.
    let engine = engine();
    let first = [
        box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0),
        rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0),
        pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
    ];
    let refs: Vec<&str> = first.iter().map(String::as_str).collect();
    let model = published(&engine, &refs);
    assert!(
        matches!(
            model.outcome(&id("qqqqqqqq")),
            Some(FeatureOutcome::Failed(_))
        ),
        "{:?}",
        model.outcome(&id("qqqqqqqq"))
    );
    let prisms = |engine: &Recompute<FakeKernel>| {
        engine
            .kernel()
            .calls()
            .iter()
            .filter(|call| matches!(call.op, silico_protocol::Op::Prism { .. }))
            .count()
    };
    let before = prisms(&engine);
    let g = pocket("gggggggg", "qqqqqqqq", "ssssssss", REVERSED);
    let w = pad("wwwwwwww", None, "ssssssss", &one_side("10"));
    let mut more = refs.clone();
    more.extend([g.as_str(), w.as_str()]);
    let model = published(&engine, &more);
    assert_eq!(prisms(&engine) - before, 1, "the shared prism, once");
    built(&model, "wwwwwwww");
    assert_eq!(
        model.outcome(&id("gggggggg")),
        Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
    );
}
