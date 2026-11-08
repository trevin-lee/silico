//! The council's round 4 (robustness's proof): a refused sweep feature whose last step (its
//! cut) is in hand, kept or cached, without its earlier ones (its faces and its sweep: a
//! cache entry dropped as unusable, a write that failed, a process that died between
//! writes) is reported as its refusal, as a fresh recompute reports it, never as a step
//! that wasn't run. Its own steps are built again: the outcome is read from each of them.
//! Only the work its users alone need is left out.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::model::WAIT;
use super::sweeps;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use silico_core::cache::Cache;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use silico_protocol::Op;
use sweeps::*;

fn published(engine: &Recompute<FakeKernel>, features: &[&str]) -> Arc<Model> {
    match engine.start(checked_document(features)).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

/// A scratch directory: its cache and the cache's secret.
struct Scratch(PathBuf);

impl Scratch {
    fn new(case: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "silico-refused-partial-cache-{case}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn engine(&self, kernel: FakeKernel) -> Recompute<FakeKernel> {
        Recompute::with_cache(
            kernel,
            Cache::with_secret(self.0.join("cache"), &self.0.join("secret")),
        )
        .with_sketches(FakeSketches::new())
    }

    /// Every entry file in the cache.
    fn entries(&self) -> BTreeSet<PathBuf> {
        let Ok(dirs) = std::fs::read_dir(self.0.join("cache")) else {
            return BTreeSet::new();
        };
        dirs.flatten()
            .filter(|e| e.path().is_dir())
            .flat_map(|e| std::fs::read_dir(e.path()).unwrap().flatten())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether the cache entry at `path` holds a fake kernel's cut.
fn is_cut(path: &Path) -> bool {
    let bytes = std::fs::read(path).unwrap();
    let cut = b"\ncut ";
    bytes.windows(cut.len()).any(|w| w == cut)
}

fn refused(model: &Model, feature: &str) -> Error {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Failed(error)) => error.clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

/// The 10 mm cube above XY and the 4 by 2 rectangle on XY; with the pocket of it going
/// down, which misses the cube; and with a fillet of the pocket too.
fn documents() -> [Vec<String>; 3] {
    let cube = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    let pocket = pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN);
    let fillet = r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "qqqqqqqq", "edges": ["bbbbbbbb.box.Edge1"], "radius": 0.1}"#;
    [
        vec![cube.clone(), sketch.clone()],
        vec![cube.clone(), sketch.clone(), pocket.clone()],
        vec![cube, sketch, pocket, fillet.to_owned()],
    ]
}

/// Removes every entry the cache gained since `had` but the one cut, and how many.
fn drop_all_but_the_cut(scratch: &Scratch, had: &BTreeSet<PathBuf>) -> usize {
    let new: Vec<PathBuf> = scratch.entries().difference(had).cloned().collect();
    assert_eq!(new.iter().filter(|p| is_cut(p)).count(), 1, "{new:?}");
    let mut dropped = 0;
    for path in new.iter().filter(|p| !is_cut(p)) {
        std::fs::remove_file(path).unwrap();
        dropped += 1;
    }
    dropped
}

fn refs(features: &[String]) -> Vec<&str> {
    features.iter().map(String::as_str).collect()
}

/// The pocket's refusal and the fillet blocked on it: what a fresh recompute reports (and
/// never an internal error, a step that wasn't run).
fn check(model: &Model, fresh: &Model) {
    let error = refused(model, "qqqqqqqq");
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    assert!(
        error
            .message()
            .starts_with("pocket qqqqqqqq removes nothing from its base bbbbbbbb"),
        "{}",
        error.message()
    );
    assert_eq!(
        model.outcome(&id("qqqqqqqq")),
        fresh.outcome(&id("qqqqqqqq"))
    );
    assert_eq!(
        model.outcome(&id("ffffffff")),
        Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
    );
    assert_eq!(model.errors().count(), 1);
}

#[test]
fn a_refused_pocket_whose_cut_alone_is_cached_is_reported_as_fresh() {
    let [before, pocketed, filleted] = documents();
    let fresh = published(
        &Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new()),
        &refs(&filleted),
    );
    let scratch = Scratch::new("cut-alone");
    let first = scratch.engine(FakeKernel::new(2).unwrap());
    published(&first, &refs(&before));
    let had = scratch.entries();
    published(&first, &refs(&pocketed));
    drop(first);
    // The pocket's own entries are its faces', its sweep's, and its cut's.
    assert_eq!(drop_all_but_the_cut(&scratch, &had), 2);
    // The cut is read back measured: the pocket is refused before anything starts, and
    // its faces and sweep are built again, as fresh, so its outcome can be read; the
    // fillet, which only its user needs, isn't.
    let second = scratch.engine(FakeKernel::new(2).unwrap());
    check(&published(&second, &refs(&filleted)), &fresh);
    let ops: Vec<String> = second
        .kernel()
        .calls()
        .iter()
        .map(|call| format!("{:?}", call.op))
        .collect();
    assert!(ops.iter().any(|op| op.starts_with("MakeFaces")), "{ops:?}");
    assert!(ops.iter().any(|op| op.starts_with("Prism")), "{ops:?}");
    assert!(!ops.iter().any(|op| op.starts_with("Fillet")), "{ops:?}");
    assert!(!ops.iter().any(|op| op.starts_with("Boolean")), "{ops:?}");
}

#[test]
fn a_refused_pockets_faces_and_sweep_still_queued_when_its_cached_cuts_measure_lands_are_built() {
    // The mid-run variant: the cut comes from the cache unmeasured (its measure failed when
    // it was made, so it was kept unmeasured), its faces and sweep are missing. The next
    // session measures the cut while its faces are held: the pocket is refused while its
    // sweep is still to be started, and is still reported as its refusal.
    let [before, pocketed, filleted] = documents();
    let fresh = published(
        &Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new()),
        &refs(&filleted),
    );
    let scratch = Scratch::new("mid-run");
    let kernel = FakeKernel::new(2).unwrap();
    kernel.refuse(|op| match op {
        Op::Summarize { shape } if FakeKernel::describe(shape)?.starts_with("cut(") => Some(
            Error::new(ErrorCode::KernelFailed, "the cut couldn't be measured"),
        ),
        _ => None,
    });
    let first = scratch.engine(kernel);
    published(&first, &refs(&before));
    let had = scratch.entries();
    let unmeasured = published(&first, &refs(&pocketed));
    assert!(
        refused(&unmeasured, "qqqqqqqq")
            .message()
            .contains("the cut couldn't be measured"),
        "{:?}",
        unmeasured.outcome(&id("qqqqqqqq"))
    );
    drop(first);
    assert_eq!(drop_all_but_the_cut(&scratch, &had), 2);
    // The cut's entry, unmeasured: rewritten measured once its measure is judged.
    let cut: Vec<PathBuf> = scratch
        .entries()
        .into_iter()
        .filter(|p| is_cut(p))
        .collect();
    let [cut] = cut.as_slice() else {
        panic!("{cut:?}");
    };
    let unmeasured = std::fs::read(cut).unwrap();
    let kernel = FakeKernel::new(2).unwrap();
    let hold = kernel.hold(|op| matches!(op, Op::MakeFaces(_)));
    let second = scratch.engine(kernel);
    let pending = second.start(checked_document(&refs(&filleted)));
    // The faces are held; the cut's measure runs beside them and is judged, refusing the
    // pocket, and its entry is written measured (after that), before they're let go.
    let deadline = std::time::Instant::now() + WAIT;
    while std::fs::read(cut).is_ok_and(|bytes| bytes == unmeasured) {
        assert!(
            std::time::Instant::now() < deadline,
            "the cut wasn't measured"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        !second
            .kernel()
            .calls()
            .iter()
            .any(|call| matches!(call.op, Op::Prism { .. })),
        "the sweep started before the verdict"
    );
    hold.release();
    let model = match pending.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    };
    check(&model, &fresh);
}

/// Every entry in the cache gained since `had` that isn't a cut removed, and how many.
fn drop_all_but_the_cuts(scratch: &Scratch, had: &BTreeSet<PathBuf>) -> usize {
    let mut dropped = 0;
    for path in scratch.entries().difference(had).filter(|p| !is_cut(p)) {
        std::fs::remove_file(path).unwrap();
        dropped += 1;
    }
    dropped
}

/// Whether the cache holds a shape of `cuts` cuts, the fake writing each of a shape's steps
/// on a line of its own: a pocket's cut on `cuts − 1` pockets.
fn cached_cuts(scratch: &Scratch, cuts: usize) -> bool {
    let cut = b"\ncut ";
    scratch.entries().iter().any(|path| {
        std::fs::read(path)
            .is_ok_and(|bytes| bytes.windows(cut.len()).filter(|w| *w == cut).count() == cuts)
    })
}

/// A fake kernel that holds the measure of the first pocket's cut (a cut of the box alone),
/// so the pockets made from it are built, measured, and kept on disk before its verdict.
fn holding_the_first_cuts_measure() -> (FakeKernel, silico_core::fake::Hold) {
    let kernel = FakeKernel::new(2).unwrap();
    let hold = kernel.hold(|op| match op {
        Op::Summarize { shape } => FakeKernel::describe(shape)
            .is_some_and(|made| made.starts_with("cut(") && made.matches("cut(").count() == 1),
        _ => false,
    });
    (kernel, hold)
}

/// The council's round 5 (tests): a refused pocket made from another refused one, its cut
/// cached (its verdict known when the recompute starts) but its own sweep missing, is
/// blocked on the first, and none of its steps is built: they serve a feature made from a
/// refused one, which nothing reads (`needed`'s `made_from_dead`). The cut of the second
/// is cached by a run in which it was made and measured before the first's measure landed
/// (held here; a measure is a job of its own, and a pocket's users start as soon as its cut
/// is made): its premise once came from an M2 cut sharing the first's step and built, which
/// S7's rule for M2 booleans makes refused too.
#[test]
fn a_refused_pocket_on_a_refused_pocket_builds_none_of_its_own_missing_steps() {
    const REVERSED: &str = r#"{"type": "one_side", "length": 10, "reversed": true}"#;
    let scratch = Scratch::new("made-from-dead");
    let cube = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    // A baseless pad, reversed: the first pocket's faces and prism, kept.
    let pad = pad("pppppppp", None, "ssssssss", REVERSED);
    let first_pocket = pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN);
    let on_it = pocket("rrrrrrrr", "qqqqqqqq", "ssssssss", &one_side("3"));
    let first = scratch.engine(FakeKernel::new(2).unwrap());
    published(&first, &refs(&[cube.clone(), sketch.clone(), pad]));
    drop(first);
    let had = scratch.entries();
    // Run 1: the first pocket's measure held until the second's cut, made from it and
    // measured, is on disk; then both refused (each misses), the second dead too.
    let (kernel, hold) = holding_the_first_cuts_measure();
    let engine = scratch.engine(kernel);
    let document = vec![cube, sketch, first_pocket, on_it];
    let pending = engine.start(checked_document(&refs(&document)));
    let deadline = std::time::Instant::now() + WAIT;
    while !cached_cuts(&scratch, 2) {
        assert!(
            std::time::Instant::now() < deadline,
            "the second cut wasn't kept"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    hold.release();
    let Ok(Ok(RunOutcome::Published(model))) = pending.wait_timeout(WAIT) else {
        panic!("not published");
    };
    refused(&model, "qqqqqqqq");
    assert_eq!(
        model.outcome(&id("rrrrrrrr")),
        Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
    );
    drop(engine);
    // The second's sweep's entry lost; both cuts kept.
    assert_eq!(drop_all_but_the_cuts(&scratch, &had), 1);
    // Run 2: the first pocket refused before anything starts (its cut cached), the second
    // made from it, its cut cached and refused, its sweep missing: nothing is built.
    let second = scratch.engine(FakeKernel::new(2).unwrap());
    let model = published(&second, &refs(&document));
    assert_eq!(
        model.outcome(&id("rrrrrrrr")),
        Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
    );
    let asked: Vec<&str> = second
        .kernel()
        .calls()
        .iter()
        .map(|c| c.op.name())
        .collect();
    assert!(
        asked.is_empty(),
        "built for a feature made from a refused one: {asked:?}"
    );
}

/// The council's round 6 (tests): the same one level down. A refused pocket on a pocket
/// that is dead (made from a refused one) but not refused itself builds none of its own
/// steps either: `made_from_dead` asks whether a feature it's made from is dead, not only
/// refused. Its cuts cached as above, made and measured before the first pocket's measure
/// landed.
#[test]
fn a_refused_pocket_on_a_dead_but_unrefused_pocket_builds_none_of_its_own_missing_steps() {
    const REVERSED: &str = r#"{"type": "one_side", "length": 10, "reversed": true}"#;
    const UP3: &str = r#"{"type": "one_side", "length": 3, "reversed": true}"#;
    let scratch = Scratch::new("made-from-dead-transitive");
    let cube = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    let pad = pad("pppppppp", None, "ssssssss", REVERSED);
    let first_pocket = pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN);
    // A pocket up into the first, which removes some of it: its own check holds.
    let mid = pocket("dddddddd", "qqqqqqqq", "ssssssss", UP3);
    // A pocket down from it, which misses: refused.
    let on_mid = pocket("rrrrrrrr", "dddddddd", "ssssssss", &one_side("3"));
    let first = scratch.engine(FakeKernel::new(2).unwrap());
    published(&first, &refs(&[cube.clone(), sketch.clone(), pad]));
    drop(first);
    let had = scratch.entries();
    let (kernel, hold) = holding_the_first_cuts_measure();
    let engine = scratch.engine(kernel);
    let document = vec![cube, sketch, first_pocket, mid, on_mid];
    let pending = engine.start(checked_document(&refs(&document)));
    let deadline = std::time::Instant::now() + WAIT;
    while !cached_cuts(&scratch, 3) {
        assert!(
            std::time::Instant::now() < deadline,
            "the third cut wasn't kept"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    hold.release();
    let Ok(Ok(RunOutcome::Published(model))) = pending.wait_timeout(WAIT) else {
        panic!("not published");
    };
    refused(&model, "qqqqqqqq");
    drop(engine);
    // The two later pockets' sweeps' entries lost; the three cuts kept.
    assert_eq!(drop_all_but_the_cuts(&scratch, &had), 2);
    // The pocket of the cube refused; the pocket on it dead, not refused (its cached cut
    // holds); the pocket on that refused, its cut cached, its sweep lost.
    let second = scratch.engine(FakeKernel::new(2).unwrap());
    let model = published(&second, &refs(&document));
    for blocked in ["dddddddd", "rrrrrrrr"] {
        assert_eq!(
            model.outcome(&id(blocked)),
            Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") }),
            "{blocked}"
        );
    }
    let asked: Vec<&str> = second
        .kernel()
        .calls()
        .iter()
        .map(|c| c.op.name())
        .collect();
    assert!(
        asked.is_empty(),
        "built for a feature made from a dead one: {asked:?}"
    );
}
