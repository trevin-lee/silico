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

/// The number of cuts `engine`'s kernel was asked for.
fn cuts(engine: &Recompute<FakeKernel>) -> usize {
    engine
        .kernel()
        .calls()
        .iter()
        .filter(|call| call.op.name() == "Cut")
        .count()
}

/// Whether the cache under `dir` holds an entry whose shape's description has `word`
/// (`"\ncut "`, `"\nbox "`). A feature's solid is written to the cache only once it's
/// measured, after its measure's verdict is decided: so an entry there means the scheduler
/// has judged it.
fn cached(dir: &std::path::Path, word: &[u8]) -> bool {
    let Ok(dirs) = std::fs::read_dir(dir) else {
        return false;
    };
    dirs.flatten()
        .filter(|e| e.path().is_dir())
        .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().flatten())
        .filter_map(|e| std::fs::read(e.path()).ok())
        .any(|bytes| bytes.windows(word.len()).any(|w| w == word))
}

/// Waits up to 30 s for `ready`.
fn wait_for(what: &str, ready: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
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
    wait_for("the cube and the first cut weren't measured", || {
        cached(&dir.join("cache"), b"\ncut ") && cached(&dir.join("cache"), b"\nbox ")
    });
    drop(second);
    let Ok(Ok(RunOutcome::Published(model))) = run.wait_timeout(Duration::from_secs(60)) else {
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
fn a_step_a_live_feature_shares_with_a_dead_one_is_run() {
    // The council's round 4: boxes B and C alike above XY, sketch S, a baseless reversed pad
    // P (the pocket's prism), the M2 cut K = cut(C, P) (built: M2 checks only its solids),
    // and the pocket Q on B (refused: it removes nothing; K's step). Then, on the same
    // engine, the same reversed pocket G on Q and F on K: one cut step, which F (alive)
    // needs and G (dead) doesn't. It's run once; G is blocked on Q, F built.
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
    assert!(matches!(
        model.outcome(&id("qqqqqqqq")),
        Some(FeatureOutcome::Failed(_))
    ));
    let before = cuts(&engine);
    let g = pocket("gggggggg", "qqqqqqqq", "ssssssss", REVERSED);
    let f = pocket("ffffffff", "kkkkkkkk", "ssssssss", REVERSED);
    let mut more = refs.clone();
    more.extend([g.as_str(), f.as_str()]);
    let model = published(&engine, &more);
    assert_eq!(cuts(&engine) - before, 1, "the shared cut, once");
    assert_eq!(
        model.outcome(&id("gggggggg")),
        Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
    );
    assert!(
        matches!(
            model.outcome(&id("ffffffff")),
            Some(FeatureOutcome::Built { .. })
        ),
        "{:?}",
        model.outcome(&id("ffffffff"))
    );
}
