//! Hostile inputs for `silico_core::timeline` (M5.1's timeline core) and
//! `warnings::per_feature`: a timeline read of a session with an open transaction whose
//! edit left a fingerprint stale, a kernel fault on a built feature's measure, a feature
//! blocked by a real failure and a fault at once (both operand orders), 20,000 broken
//! names, thousands of warned features, and models of other documents. Expected values
//! come from what `check` (the session's and `command::check`) and `warnings::of` say of
//! the same model, and from the timeline's own doc: a fault, and what it stopped, is
//! unknown; a feature that can't compute for a document problem is an error.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::cache::Cache;
use silico_core::command::{self, CheckModel, ModelChoice, ReadCommand, Revision, Sessions};
use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeKernel;
use silico_core::recompute::{FeatureOutcome, Model};
use silico_core::timeline::{Entry, Excepted, Health, timeline};
use silico_core::{Error, ErrorCode};
use silico_protocol::{BooleanOp, Op};
use tempfile::TempDir;

use crate::commands::engine;
use crate::model::published;
use crate::sessions::answer;
use crate::sweeps::{block, box_at, document};

fn id(s: &str) -> FeatureId {
    FeatureId::new(s).unwrap()
}

fn boolean(kind: &str, id: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "Op", "base": "{base}", "tool": "{tool}"}}"#
    )
}

fn entry<'a>(timeline: &'a [Entry], f: &str) -> &'a Entry {
    timeline.iter().find(|e| e.id == id(f)).unwrap()
}

fn mark(e: &Entry) -> (Health, u64, u64) {
    (e.health, e.problems, e.warnings)
}

fn rebuilt_with(
    text: &str,
    refuse: impl Fn(&Op) -> Option<Error> + Send + Sync + 'static,
) -> Arc<Model> {
    let engine = engine();
    engine.kernel().refuse(refuse);
    published(
        &engine,
        Checked::new(read(text.as_bytes()).unwrap()).unwrap(),
    )
}

/// A box 7 × 1 × 1 stored with volume 8: it measures 7 (by formula), so it drifts.
const STALE_BOX: &str = r#"{"id": "dddddddd", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 7, "width": 1, "height": 1, "fingerprint": {"volume": 8, "area": 30, "bbox": [0, 0, 0, 7, 1, 1], "faces": 6, "edges": 12, "vertices": 8}}"#;

// --- Drift excepted by the session's check, reported by the timeline -----------------------

/// A block whose fingerprint is its 40 × 20 × 10 measures (by formula: volume 8000, area
/// 2800), length `length`.
fn stored_block(length: u32) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": {length}, "width": 20, "height": 10, "fingerprint": {{"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}}}]}}"#
    )
}

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let sessions =
        Sessions::with_cache_opener(Arc::new(FakeKernel::new(2).unwrap()), move |cache| {
            Cache::with_secret(cache, &secret)
        });
    Fixture { dir, sessions }
}

impl Fixture {
    fn file(&self, text: &str) -> PathBuf {
        let path = self.dir.path().join("a.silico");
        std::fs::write(&path, text).unwrap();
        path
    }
}

// Found by break-it, fixed: in an open transaction whose edit reached the block (its length 40 → 41, the
// stored fingerprint now the old solid's), the session's `check` excepts the block's drift
// (decision 28: the commit writes its new fingerprint) and answers no problems, but the
// timeline of the same model, given the features the edits reached, marks the block
// `error` with one problem: `marks` reads `Model::drifted()`, never `drift_except`, so
// every feature an open transaction edits turns red in the timeline while `check` says
// the document has no problem. `timeline` has no way to be told which drift is excepted.
#[test]
fn a_feature_an_open_transaction_edited_is_no_error_where_the_sessions_check_has_no_problem() {
    let f = fixture();
    let path = f.file(&stored_block(40));
    let opened = answer(f.sessions.open(&path, false));
    let s1 = opened.session.unwrap().id;
    answer(f.sessions.begin(&s1)).result.unwrap();
    answer(
        f.sessions
            .replace_text(&s1, Revision::OPENED, stored_block(41)),
    )
    .result
    .unwrap();
    let checked = answer(f.sessions.check(&s1, CheckModel::Current { fresh: false }));
    let problems = checked.result.unwrap().problems;
    assert!(
        problems.is_empty(),
        "the session's check says no problem: {:?}",
        problems.errors()
    );
    let read =
        answer(
            f.sessions
                .read_model(&s1, ReadCommand::Check, ModelChoice::Current, |read| {
                    // What the reach is, beside the timeline the command would answer with.
                    let reached = read.reached().clone();
                    let model = read.model();
                    Ok((
                        reached.clone(),
                        timeline(
                            model.document(),
                            Some(model),
                            &Excepted {
                                spared: reached.clone(),
                                drift: reached.clone(),
                            },
                        ),
                    ))
                }),
        );
    let (reached, got) = read.result.unwrap();
    assert!(reached.contains(&id("k7f2qx3m")), "{reached:?}");
    let block = entry(&got.features, "k7f2qx3m");
    assert_eq!(
        mark(block),
        (Health::Healthy, 0, 0),
        "the timeline contradicts the session's check"
    );
}

// Found by break-it, fixed: the same at the library: a model whose box drifts from its stored fingerprint,
// read with that box in `spared` (the features an open transaction's edits reached), has
// no drift left once excepted (`Model::drift_except`, what the session's check reports),
// but the timeline marks it `error`, one problem.
#[test]
fn a_spared_features_drift_is_excepted_as_drift_except_excepts_it() {
    let text = document(&[STALE_BOX]);
    let model = rebuilt_with(&text, |_| None);
    let spared = BTreeSet::from([id("dddddddd")]);
    assert_eq!(model.drift().len(), 1, "it drifts");
    assert!(
        model.drift_except(&spared).is_empty(),
        "excepted, as check excepts it"
    );
    let got = timeline(
        model.document(),
        Some(&model),
        &Excepted {
            drift: spared,
            ..Excepted::default()
        },
    );
    assert_eq!(
        mark(entry(&got.features, "dddddddd")),
        (Health::Healthy, 0, 0)
    );
}

// --- Kernel faults ---------------------------------------------------------------------------

// Found by break-it, fixed: a box that builds but whose measure the worker crashes on has its drift unknown
// (its fingerprint, stored wrong here, is never compared): `check` sets the fault aside and
// `Model::fault` holds it, and a fault-free rebuild would say drift. The timeline's own
// doc says a feature a kernel fault stopped is `unknown`, but it marks the box `healthy`:
// `marks` drops a kernel-fault drift entry instead of marking its feature unknown.
#[test]
fn a_built_feature_whose_measure_faulted_is_unknown_not_healthy() {
    let text = document(&[STALE_BOX]);
    // Without the fault: drift, an error.
    let clean = rebuilt_with(&text, |_| None);
    let got = timeline(clean.document(), Some(&clean), &Excepted::default());
    assert_eq!(
        mark(entry(&got.features, "dddddddd")),
        (Health::Error, 1, 0)
    );
    // With every measure crashing.
    let model = rebuilt_with(&text, |op| {
        matches!(op, Op::Summarize { .. })
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the worker crashed"))
    });
    assert!(matches!(
        model.outcome(&id("dddddddd")),
        Some(FeatureOutcome::Built { .. })
    ));
    assert!(model.fault().is_some(), "the model holds the fault");
    assert!(command::check(&model).is_empty());
    let got = timeline(model.document(), Some(&model), &Excepted::default());
    assert_eq!(
        mark(entry(&got.features, "dddddddd")),
        (Health::Unknown, 0, 0),
        "its drift wasn't checked: unknown, as the fault is"
    );
}

/// Four pairs of boxes; a fuse of a pair the kernel crashes on (a fault), a cut of a pair it
/// refuses (the document's problem); then fuses of the two, each order.
fn blocked_both_ways() -> Arc<Model> {
    let text = document(&[
        &box_at("a0000001", [0.0, 0.0, 0.0], 1.0, 1.0, 1.0),
        &box_at("a0000002", [0.0, 0.0, 0.0], 2.0, 1.0, 1.0),
        &box_at("a0000003", [0.0, 0.0, 0.0], 3.0, 1.0, 1.0),
        &box_at("a0000004", [0.0, 0.0, 0.0], 4.0, 1.0, 1.0),
        &box_at("a0000005", [0.0, 0.0, 0.0], 5.0, 1.0, 1.0),
        &box_at("a0000006", [0.0, 0.0, 0.0], 6.0, 1.0, 1.0),
        &box_at("a0000007", [0.0, 0.0, 0.0], 7.0, 1.0, 1.0),
        &box_at("a0000008", [0.0, 0.0, 0.0], 8.0, 1.0, 1.0),
        &boolean("fuse", "j0000001", "a0000001", "a0000002"),
        &boolean("cut", "k0000001", "a0000003", "a0000004"),
        &boolean("fuse", "j0000002", "a0000005", "a0000006"),
        &boolean("cut", "k0000002", "a0000007", "a0000008"),
        // The fault first, the failure second.
        &boolean("fuse", "x0000001", "j0000001", "k0000001"),
        // The failure first, the fault second.
        &boolean("fuse", "y0000001", "k0000002", "j0000002"),
    ]);
    rebuilt_with(&text, |op| match op {
        Op::Boolean {
            op: BooleanOp::Fuse,
            ..
        } => Some(Error::new(ErrorCode::WorkerCrashed, "the worker crashed")),
        Op::Boolean {
            op: BooleanOp::Cut, ..
        } => Some(Error::new(ErrorCode::KernelFailed, "the cut failed")),
        _ => None,
    })
}

// Found by break-it, fixed: a fuse whose base is blocked on a kernel fault and whose tool failed for the
// document (a cut the kernel refuses) can't compute until the document changes, whatever
// the fault; but its health follows the operands' order: `unknown` with the fault as base,
// `error` with it as tool. `Blocked { on }` names only the first root, and `marks` reads
// only it. A feature with a document problem upstream is an error either way.
#[test]
fn a_feature_blocked_by_a_failure_and_a_fault_is_an_error_whichever_comes_first() {
    let model = blocked_both_ways();
    let got = timeline(model.document(), Some(&model), &Excepted::default());
    let f = &got.features;
    assert_eq!(mark(entry(f, "j0000001")), (Health::Unknown, 0, 0));
    assert_eq!(mark(entry(f, "k0000001")), (Health::Error, 1, 0));
    // The failure first: an error.
    assert_eq!(mark(entry(f, "y0000001")), (Health::Error, 0, 0));
    // The fault first: should be the same.
    assert_eq!(
        mark(entry(f, "x0000001")),
        mark(entry(f, "y0000001")),
        "health depends on the operands' order"
    );
}

/// Every feature's problems add up to `check`'s total, and its warnings to `warnings::of`'s.
#[track_caller]
fn assert_totals_agree(model: &Model, spared: &BTreeSet<FeatureId>) {
    let got = timeline(
        model.document(),
        Some(model),
        &Excepted {
            spared: spared.clone(),
            ..Excepted::default()
        },
    );
    let problems: u64 = got.features.iter().map(|e| e.problems).sum();
    let warnings: u64 = got.features.iter().map(|e| e.warnings).sum();
    assert_eq!(problems, command::check(model).total(), "problems");
    assert_eq!(
        warnings,
        silico_core::warnings::of(model, spared).total(),
        "warnings"
    );
    // Health agrees with the counts: an entry with a problem is an error, one with a
    // warning and no problem a warning or error, one with neither healthy, error (blocked),
    // or unknown.
    for e in &got.features {
        match e.health {
            Health::Healthy => assert_eq!((e.problems, e.warnings), (0, 0), "{e:?}"),
            Health::Warning => assert!(e.problems == 0 && e.warnings > 0, "{e:?}"),
            Health::Unknown => assert_eq!((e.problems, e.warnings), (0, 0), "{e:?}"),
            Health::Error | Health::Suppressed => {}
        }
        if e.problems > 0 {
            assert_eq!(e.health, Health::Error, "{e:?}");
        }
    }
}

#[test]
fn the_blocked_both_ways_models_counts_add_up_to_checks() {
    assert_totals_agree(&blocked_both_ways(), &BTreeSet::new());
}

// --- Size --------------------------------------------------------------------------------------

#[test]
fn twenty_thousand_broken_names_are_twenty_thousand_problems_as_check_counts_them() {
    let edges: Vec<String> = (13..20_013)
        .map(|n| format!("\"bbbbbbbb.box.Edge{n}\""))
        .collect();
    let text = document(&[
        &block("bbbbbbbb"),
        &format!(
            r#"{{"id": "f0000002", "type": "fillet", "v": 1, "name": "Round", "base": "bbbbbbbb", "edges": [{}], "radius": 0.1}}"#,
            edges.join(", ")
        ),
        r#"{"id": "f0000003", "type": "fillet", "v": 1, "name": "Round", "base": "f0000002", "edges": ["bbbbbbbb.box.Edge1"], "radius": 0.1}"#,
    ]);
    let model = rebuilt_with(&text, |_| None);
    let at = Instant::now();
    let got = timeline(model.document(), Some(&model), &Excepted::default());
    let took = at.elapsed();
    assert_eq!(
        mark(entry(&got.features, "f0000002")),
        (Health::Error, 20_000, 0)
    );
    // Blocked by it: an error with none of its own.
    assert_eq!(
        mark(entry(&got.features, "f0000003")),
        (Health::Error, 0, 0)
    );
    assert_eq!(command::check(&model).total(), 20_000);
    assert_totals_agree(&model, &BTreeSet::new());
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[test]
fn five_thousand_features_warned_each_count_one_and_add_up_to_warnings_of() {
    let blocks: Vec<String> = (0..5_000).map(|n| block(&format!("b{n:07}"))).collect();
    let refs: Vec<&str> = blocks.iter().map(String::as_str).collect();
    let model = rebuilt_with(&document(&refs), |_| None);
    let at = Instant::now();
    let got = timeline(model.document(), Some(&model), &Excepted::default());
    let took = at.elapsed();
    let all = silico_core::warnings::of(&model, &BTreeSet::new());
    assert_eq!(all.total(), 5_000);
    assert_eq!(all.listed().len(), 100);
    assert!(
        got.features
            .iter()
            .all(|e| mark(e) == (Health::Warning, 0, 1)),
        "every block warned of once"
    );
    assert_totals_agree(&model, &BTreeSet::new());
    // Spared all: none warned.
    let every: BTreeSet<FeatureId> = got.features.iter().map(|e| e.id.clone()).collect();
    let spared = timeline(
        model.document(),
        Some(&model),
        &Excepted {
            spared: every,
            ..Excepted::default()
        },
    );
    assert!(
        spared
            .features
            .iter()
            .all(|e| mark(e) == (Health::Healthy, 0, 0))
    );
    assert!(took < Duration::from_secs(5), "{took:?}");
}

// --- Models of other documents -------------------------------------------------------------------

#[test]
fn a_model_of_a_document_whose_features_were_removed_and_reordered_marks_what_both_have() {
    let text = document(&[&block("aaaaaaaa"), &block("bbbbbbbb"), &block("cccccccc")]);
    let model = rebuilt_with(&text, |_| None);
    // The session's document: bbbbbbbb removed, cccccccc before aaaaaaaa, a new one.
    let newer =
        read(document(&[&block("cccccccc"), &block("aaaaaaaa"), &block("zzzzzzzz")]).as_bytes())
            .unwrap();
    let got = timeline(&newer, Some(&model), &Excepted::default());
    let ids: Vec<&str> = got.features.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["cccccccc", "aaaaaaaa", "zzzzzzzz"]);
    assert_eq!(mark(&got.features[0]), (Health::Warning, 0, 1));
    assert_eq!(mark(&got.features[1]), (Health::Warning, 0, 1));
    assert_eq!(mark(&got.features[2]), (Health::Unknown, 0, 0));
}

#[test]
fn an_empty_document_and_an_empty_model_have_an_empty_timeline() {
    let model = rebuilt_with(&document(&[]), |_| None);
    let got = timeline(model.document(), Some(&model), &Excepted::default());
    assert!(got.features.is_empty());
    assert!(got.bodies.is_empty());
    // A model of a document with features, read for an empty one.
    let model = rebuilt_with(&document(&[&block("aaaaaaaa")]), |_| None);
    let empty = read(document(&[]).as_bytes()).unwrap();
    let got = timeline(&empty, Some(&model), &Excepted::default());
    assert!(got.features.is_empty());
}

#[test]
fn a_timeline_is_the_same_read_twice_and_from_two_threads() {
    let model = blocked_both_ways();
    let once = timeline(model.document(), Some(&model), &Excepted::default());
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let model = Arc::clone(&model);
            std::thread::spawn(move || {
                timeline(model.document(), Some(&model), &Excepted::default())
            })
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), once);
    }
}
