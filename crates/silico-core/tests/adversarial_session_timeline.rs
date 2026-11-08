//! Hostile inputs for a session's `timeline` (`Sessions::timeline`, break-it, PR #276):
//! sessions closed or never opened; a published read with nothing published; documents
//! the gate refuses for duplicate IDs, a fuse of itself, or hundreds of problems past the
//! listing cap; text that's empty, `null`, a list, bytes that aren't UTF-8, or another
//! version; a published model older than a document whose features were removed or given
//! a new consumer; undo, redo, rollback, and a commit that writes nothing; concurrent
//! timelines of both models against a stream of edits; fillets naming edges their box
//! hasn't, a fuse, a sketch and its pad, a kernel fault mid-chain, a crashed sketch worker,
//! and a sketch solved otherwise than its file, each marked as the session's check has it;
//! two thousand cubes and their reversal; features moved under an older published model;
//! a published timeline past eight held reads; a timeline cancelled with a close queued
//! behind it; and random runs of edits, transactions, undos, and changes on disk, each
//! step's timeline compared with the session's own `check` of the same model. Expected values: each feature's health by
//! hand from the fake kernel's formulas (a box L × W × H measures L·W·H, area
//! 2(LW + LH + WH); a fuse of two apart, the sums), the gate's one problem per bad value,
//! and, for random runs, the session's `check`, which the timeline's doc says it agrees
//! with feature by feature.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use proptest::prelude::*;
use silico_core::cache::Cache;
use silico_core::command::{
    CheckAnswer, CheckModel, ModelChoice, Response, Revision, SessionId, Sessions, TimelineAnswer,
};
use silico_core::document::FeatureId;
use silico_core::fake::FakeKernel;
use silico_core::timeline::{Entry, Health};
use silico_core::{Error, ErrorCode};
use silico_protocol::{Op, Primitive};
use tempfile::TempDir;

use crate::sessions::{answer, id, refusal};

const HEAD: &str =
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": ["#;

const BLOCK: &str = "k7f2qx3m";
const LUG: &str = "b0000001";
const FUSE: &str = "f0000001";

/// A block's stored fingerprint: its measures at 40 × 20 × 10 (volume 8000, area 2800),
/// its volume a millionth over them, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Print {
    Right,
    Stale,
    Missing,
}

fn block_line(length: u32, print: Print) -> String {
    let volume = match print {
        Print::Right => "8000",
        Print::Stale => "8000.000001",
        Print::Missing => "",
    };
    let print = if volume.is_empty() {
        String::new()
    } else {
        format!(
            r#", "fingerprint": {{"volume": {volume}, "area": 2800, "center": [20, 10, 5], "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}"#
        )
    };
    format!(
        r#"{{"id": "{BLOCK}", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": {length}, "width": 20, "height": 10{print}}}"#
    )
}

/// The lug, 10 wide and high at x = 100, `length` long, with its fingerprint at length 10
/// (volume 1000, area 600) or none.
fn lug_line(length: u32, printed: bool) -> String {
    let print = if printed {
        r#", "fingerprint": {"volume": 1000, "area": 600, "center": [105, 5, 5], "bbox": [100, 0, 0, 110, 10, 10], "faces": 6, "edges": 12, "vertices": 8}"#
    } else {
        ""
    };
    format!(
        r#"{{"id": "{LUG}", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": {length}, "width": 10, "height": 10{print}}}"#
    )
}

/// The fuse of the block and the lug, with its fingerprint for the lug 10 long (volume
/// 9000, area 3400) or none.
fn fuse_line(printed: bool) -> String {
    let print = if printed {
        r#", "fingerprint": {"volume": 9000, "area": 3400, "center": [29.444444, 9.444444, 5], "bbox": [0, 0, 0, 110, 20, 10], "faces": 12, "edges": 24, "vertices": 16}"#
    } else {
        ""
    };
    format!(
        r#"{{"id": "{FUSE}", "type": "fuse", "v": 1, "name": "Both", "base": "{BLOCK}", "tool": "{LUG}"{print}}}"#
    )
}

fn document(lines: &[String]) -> String {
    format!("{HEAD}{}]}}", lines.join(", "))
}

/// The block (its fingerprint `print`), the lug 10 long with its own, and their fuse with its.
fn three(print: Print, block_length: u32) -> String {
    document(&[
        block_line(block_length, print),
        lug_line(10, true),
        fuse_line(true),
    ])
}

struct Fixture {
    _dir: TempDir,
    kernel: Arc<FakeKernel>,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
    path: PathBuf,
}

/// A lug (10 wide) 30 long can't be made, and one 40 long crashes the worker: a failure of
/// the document's, and a kernel fault (decision 32), that never touch the block (20 wide).
fn refusing(kernel: &FakeKernel) {
    kernel.refuse(|op| match op {
        Op::MakePrimitive(Primitive::Box { dx, dy, .. }) if *dy < 15.0 => {
            if *dx > 25.0 && *dx < 35.0 {
                Some(Error::new(ErrorCode::KernelFailed, "the lug can't be made"))
            } else if *dx > 35.0 && *dx < 45.0 {
                Some(Error::new(
                    ErrorCode::WorkerCrashed,
                    "the lug crashed the worker",
                ))
            } else {
                None
            }
        }
        _ => None,
    });
}

fn opened_bytes(bytes: &[u8]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    refusing(&kernel);
    let sessions = Sessions::with_cache_opener(Arc::clone(&kernel), move |cache| {
        Cache::with_secret(cache, &secret)
    });
    let path = dir.path().join("a.silico");
    std::fs::write(&path, bytes).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    Fixture {
        _dir: dir,
        kernel,
        sessions,
        id,
        path,
    }
}

fn opened(text: &str) -> Fixture {
    opened_bytes(text.as_bytes())
}

impl Fixture {
    fn timeline(&self, model: ModelChoice) -> Response<TimelineAnswer> {
        answer(self.sessions.timeline(&self.id, model))
    }

    fn check(&self, model: CheckModel) -> Response<CheckAnswer> {
        answer(self.sessions.check(&self.id, model))
    }

    fn revision(&self) -> Revision {
        answer(self.sessions.text(&self.id, None))
            .session
            .unwrap()
            .revision
    }

    fn text(&self) -> String {
        answer(self.sessions.text(&self.id, None))
            .result
            .unwrap()
            .text
    }

    fn begin(&self) {
        answer(self.sessions.begin(&self.id)).result.unwrap();
    }

    fn replace(&self, text: &str) {
        answer(
            self.sessions
                .replace_text(&self.id, self.revision(), text.to_owned()),
        )
        .result
        .unwrap();
    }
}

/// Each feature's ID and mark (health, problems, warnings), in the timeline's order.
fn marks(features: &[Entry]) -> Vec<(String, Health, u64, u64)> {
    features
        .iter()
        .map(|e| (e.id.as_str().to_owned(), e.health, e.problems, e.warnings))
        .collect()
}

fn mark(feature: &str, health: Health, problems: u64, warnings: u64) -> (String, Health, u64, u64) {
    (feature.to_owned(), health, problems, warnings)
}

fn bodies(features: &[&str]) -> Vec<FeatureId> {
    features.iter().map(|f| id(f)).collect()
}

/// What the timeline of the current model says, checked to be of the session's revision,
/// its model that revision's and named so by the response.
#[track_caller]
fn current(f: &Fixture) -> TimelineAnswer {
    let response = f.timeline(ModelChoice::Current);
    let revision = response.session.as_ref().unwrap().revision;
    let answer = response.result.unwrap();
    assert_eq!(answer.revision, revision);
    assert_eq!(answer.model, response.model.map(|m| m.revision));
    answer
}

// --- Sessions that aren't there ----------------------------------------------------------

#[test]
fn a_closed_sessions_timeline_is_refused_as_not_open_for_both_models() {
    let f = opened(&three(Print::Right, 40));
    answer(f.sessions.close(&f.id)).result.unwrap();
    for choice in ModelChoice::ALL {
        let response = f.timeline(choice);
        assert!(response.session.is_none(), "{choice:?}");
        assert!(response.model.is_none(), "{choice:?}");
        let (code, message, parameter, suggestion) = refusal(&response);
        assert_eq!(code, ErrorCode::InvalidInput, "{choice:?}");
        assert_eq!(
            message,
            format!("no session {} is open", f.id),
            "{choice:?}"
        );
        assert_eq!(parameter.as_deref(), Some("session"), "{choice:?}");
        assert!(suggestion.is_some(), "{choice:?}");
    }
}

#[test]
fn a_session_never_opened_is_refused_as_not_open_for_both_models() {
    let f = opened(&three(Print::Right, 40));
    let never = SessionId::parse("s999").unwrap();
    for choice in ModelChoice::ALL {
        let response = answer(f.sessions.timeline(&never, choice));
        let (code, message, parameter, _) = refusal(&response);
        assert_eq!(code, ErrorCode::InvalidInput, "{choice:?}");
        assert_eq!(message, "no session s999 is open", "{choice:?}");
        assert_eq!(parameter.as_deref(), Some("session"), "{choice:?}");
    }
    // The open session is untouched.
    assert_eq!(current(&f).model, Some(Revision::OPENED));
}

// --- Nothing published ------------------------------------------------------------------

#[test]
fn a_published_timeline_before_any_rebuild_is_refused_naming_the_model_then_answers() {
    // Open builds nothing (decision 13); the first published read starts the rebuild and,
    // with nothing published, is refused as every published read is, naming `model`.
    let f = opened(&three(Print::Right, 40));
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let response = f.timeline(ModelChoice::Published);
    assert!(response.model.is_none());
    let (code, _, parameter, suggestion) = refusal(&response);
    assert_eq!(code, ErrorCode::InvalidInput);
    assert_eq!(parameter.as_deref(), Some("model"));
    assert!(suggestion.unwrap().contains("current model"));
    hold.release();
    // The current one waits for that rebuild: every feature healthy, the fuse the body.
    let answer = current(&f);
    assert_eq!(
        marks(answer.features.as_ref().unwrap()),
        [
            mark(BLOCK, Health::Healthy, 0, 0),
            mark(LUG, Health::Healthy, 0, 0),
            mark(FUSE, Health::Healthy, 0, 0),
        ]
    );
    assert_eq!(answer.bodies, bodies(&[FUSE]));
    // And the published one has it now.
    assert_eq!(f.timeline(ModelChoice::Published).result.unwrap(), answer);
}

// --- Documents the gate refuses --------------------------------------------------------------

/// The timeline of a refused document, of both models: no model, every feature unknown,
/// no bodies, and the count of problems the session's check of it gives.
#[track_caller]
fn refused_both_ways(f: &Fixture) -> TimelineAnswer {
    let check = f.check(CheckModel::Current { fresh: false });
    let total = check.result.unwrap().problems.total();
    assert!(total >= 1);
    let mut answers = Vec::new();
    for choice in ModelChoice::ALL {
        let response = f.timeline(choice);
        assert!(response.model.is_none(), "{choice:?}");
        let answer = response.result.unwrap();
        assert_eq!(answer.model, None, "{choice:?}");
        assert_eq!(answer.problems, total, "{choice:?}");
        assert!(answer.bodies.is_empty(), "{choice:?}");
        for e in answer.features.iter().flatten() {
            assert_eq!((e.health, e.problems, e.warnings), (Health::Unknown, 0, 0));
        }
        answers.push(answer);
    }
    assert_eq!(answers[0], answers[1]);
    assert!(f.kernel.calls().is_empty(), "nothing is built of it");
    answers.remove(0)
}

#[test]
fn a_document_with_one_id_twice_is_answered_as_read_both_listed() {
    let text = document(&[
        block_line(40, Print::Right),
        block_line(40, Print::Right).replace("Block", "Again"),
    ]);
    let f = opened(&text);
    let answer = refused_both_ways(&f);
    let features = answer.features.unwrap();
    assert_eq!(
        features
            .iter()
            .map(|e| (e.id.as_str(), e.name.as_str()))
            .collect::<Vec<_>>(),
        [(BLOCK, "Block"), (BLOCK, "Again")]
    );
}

#[test]
fn a_fuse_of_itself_and_a_cycle_are_answered_as_read_without_hanging() {
    // The fuse names itself as its base; a second fuse and the first use each other.
    let fused_with_itself = document(&[
        block_line(40, Print::Right),
        lug_line(10, true),
        fuse_line(false).replace(
            &format!(r#""base": "{BLOCK}""#),
            &format!(r#""base": "{FUSE}""#),
        ),
    ]);
    let f = opened(&fused_with_itself);
    let features = refused_both_ways(&f).features.unwrap();
    assert_eq!(features[2].inputs, bodies(&[FUSE, LUG]));
    let cycle = document(&[
        r#"{"id": "c0000001", "type": "fuse", "v": 1, "name": "A", "base": "c0000002", "tool": "c0000002"}"#
            .to_owned(),
        r#"{"id": "c0000002", "type": "cut", "v": 1, "name": "B", "base": "c0000001", "tool": "c0000001"}"#
            .to_owned(),
    ]);
    let f = opened(&cycle);
    let features = refused_both_ways(&f).features.unwrap();
    assert_eq!(features.len(), 2);
    assert_eq!(features[0].inputs, bodies(&["c0000002", "c0000002"]));
}

#[test]
fn three_hundred_problems_are_counted_past_the_hundred_listed() {
    // 300 boxes each 0 long: one `length` problem each, by the gate's rule.
    let lines: Vec<String> = (0..300)
        .map(|n| {
            format!(
                r#"{{"id": "z{n:07}", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 0, "width": 20, "height": 10}}"#
            )
        })
        .collect();
    let f = opened(&document(&lines));
    let answer = refused_both_ways(&f);
    assert_eq!(answer.problems, 300);
    assert_eq!(answer.features.unwrap().len(), 300);
}

#[test]
fn text_that_doesnt_read_has_no_features_whatever_it_is() {
    for bytes in [
        &b""[..],
        b"null",
        b"[]",
        b"{}",
        b"\xff\xfe\x00{",
        br#"{"silico": 2, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": []}"#,
        br#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [], "features": []}"#,
    ] {
        let f = opened_bytes(bytes);
        let answer = refused_both_ways(&f);
        assert_eq!(answer.features, None, "{:?}", String::from_utf8_lossy(bytes));
        assert_eq!(answer.revision, Revision::OPENED);
    }
}

#[test]
fn a_document_of_no_features_is_built_and_empty() {
    let f = opened(&document(&[]));
    let answer = current(&f);
    assert_eq!(answer.model, Some(Revision::OPENED));
    assert_eq!(answer.problems, 0);
    assert_eq!(answer.features, Some(vec![]));
    assert!(answer.bodies.is_empty());
}

// --- A published model older than the session's document ---------------------------------------

/// The block, 41 long and its fingerprint stale, made only once `hold` is released.
fn hold_the_long_block(f: &Fixture) -> silico_core::fake::Hold {
    f.kernel.hold(
        |op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, dy, .. }) if *dy > 15.0 && *dx > 40.5),
    )
}

#[test]
fn an_older_model_under_a_document_without_its_fuse_has_both_boxes_as_bodies() {
    // Revision 1: the block's fingerprint stale (an error, one problem), the fuse the body.
    let f = opened(&three(Print::Stale, 40));
    let first = current(&f);
    assert_eq!(
        marks(first.features.as_ref().unwrap()),
        [
            mark(BLOCK, Health::Error, 1, 0),
            mark(LUG, Health::Healthy, 0, 0),
            mark(FUSE, Health::Healthy, 0, 0),
        ]
    );
    assert_eq!(first.bodies, bodies(&[FUSE]));
    // Revision 2, in a transaction: the fuse removed and the block 41 long, its box held.
    f.begin();
    let hold = hold_the_long_block(&f);
    f.replace(&document(&[
        block_line(41, Print::Stale),
        lug_line(10, true),
    ]));
    let response = f.timeline(ModelChoice::Published);
    assert_eq!(response.model.as_ref().unwrap().revision, Revision::OPENED);
    let answer = response.result.unwrap();
    assert_eq!(answer.revision, response.session.unwrap().revision);
    assert_eq!(answer.model, Some(Revision::OPENED));
    // The block changed since: unknown. The lug, unchanged: as revision 1 had it. Both
    // built by revision 1, and no feature of the document consumes either: both bodies.
    assert_eq!(
        marks(answer.features.as_ref().unwrap()),
        [
            mark(BLOCK, Health::Unknown, 0, 0),
            mark(LUG, Health::Healthy, 0, 0),
        ]
    );
    assert_eq!(answer.bodies, bodies(&[BLOCK, LUG]));
    hold.release();
    // Revision 2's own: the block's drift is what the edit reached, which its commit
    // writes, so excepted, as check excepts it.
    let now = current(&f);
    assert_eq!(
        marks(now.features.as_ref().unwrap()),
        [
            mark(BLOCK, Health::Healthy, 0, 0),
            mark(LUG, Health::Healthy, 0, 0),
        ]
    );
    assert_eq!(now.bodies, bodies(&[BLOCK, LUG]));
    let check = f
        .check(CheckModel::Current { fresh: false })
        .result
        .unwrap();
    assert!(check.problems.is_empty(), "{:?}", check.problems.errors());
}

#[test]
fn an_older_model_under_a_document_whose_fuse_gained_a_consumer_has_no_body_yet() {
    let f = opened(&three(Print::Stale, 40));
    current(&f);
    f.begin();
    // A pin, 5 × 5 × 5 at x = 102, in the lug, held, and a cut of the fuse by it (one that
    // removes something: S7's rule refuses one that doesn't).
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx < 6.0));
    f.replace(&document(&[
        block_line(40, Print::Stale),
        lug_line(10, true),
        fuse_line(true),
        r#"{"id": "p0000001", "type": "box", "v": 1, "name": "Pin", "position": [102, 0, 0], "length": 5, "width": 5, "height": 5}"#.to_owned(),
        format!(r#"{{"id": "c0000001", "type": "cut", "v": 1, "name": "Cut", "base": "{FUSE}", "tool": "p0000001"}}"#),
    ]));
    let answer = f.timeline(ModelChoice::Published).result.unwrap();
    assert_eq!(answer.model, Some(Revision::OPENED));
    assert_eq!(
        marks(answer.features.as_ref().unwrap()),
        [
            mark(BLOCK, Health::Error, 1, 0),
            mark(LUG, Health::Healthy, 0, 0),
            mark(FUSE, Health::Healthy, 0, 0),
            mark("p0000001", Health::Unknown, 0, 0),
            mark("c0000001", Health::Unknown, 0, 0),
        ]
    );
    // The block and the lug the fuse consumes, the fuse the cut will, and the pin isn't
    // built: no body until the rebuild.
    assert_eq!(answer.bodies, bodies(&[]));
    hold.release();
    let now = current(&f);
    // The pin and the cut, added, reached: their missing fingerprints are what the commit
    // writes. The cut's the body.
    assert_eq!(
        marks(now.features.as_ref().unwrap()),
        [
            mark(BLOCK, Health::Error, 1, 0),
            mark(LUG, Health::Healthy, 0, 0),
            mark(FUSE, Health::Healthy, 0, 0),
            mark("p0000001", Health::Healthy, 0, 0),
            mark("c0000001", Health::Healthy, 0, 0),
        ]
    );
    assert_eq!(now.bodies, bodies(&["c0000001"]));
}

// --- Undo, redo, rollback, and a commit that writes nothing --------------------------------------

const DRIFTED: [(&str, Health, u64, u64); 3] = [
    (BLOCK, Health::Error, 1, 0),
    (LUG, Health::Healthy, 0, 0),
    (FUSE, Health::Healthy, 0, 0),
];
const ALL_HEALTHY: [(&str, Health, u64, u64); 3] = [
    (BLOCK, Health::Healthy, 0, 0),
    (LUG, Health::Healthy, 0, 0),
    (FUSE, Health::Healthy, 0, 0),
];

#[track_caller]
fn marked(f: &Fixture, expected: [(&str, Health, u64, u64); 3]) {
    let expected: Vec<_> = expected
        .iter()
        .map(|(feature, health, problems, warnings)| mark(feature, *health, *problems, *warnings))
        .collect();
    let now = current(f);
    assert_eq!(now.model, Some(now.revision));
    assert_eq!(marks(now.features.as_ref().unwrap()), expected, "current");
    assert_eq!(now.bodies, bodies(&[FUSE]));
    let published = f.timeline(ModelChoice::Published).result.unwrap();
    assert_eq!(published, now, "published");
}

#[test]
fn an_undone_edit_reaches_nothing_so_its_drift_is_said_again_and_a_redo_excepts_it_again() {
    let f = opened(&three(Print::Stale, 40));
    marked(&f, DRIFTED);
    f.begin();
    // The block 41 long: reached (and the fuse that uses it), the drift of both excepted
    // (the block's stored 8000.000001 against 8200; the fuse's 9000 against 9200).
    f.replace(&three(Print::Stale, 41));
    marked(&f, ALL_HEALTHY);
    let undone = answer(f.sessions.undo(&f.id, f.revision(), None));
    undone.result.unwrap();
    // Back at 40: no edit reaches anything, the stale fingerprint's drift said again.
    marked(&f, DRIFTED);
    answer(f.sessions.redo(&f.id, f.revision(), None))
        .result
        .unwrap();
    marked(&f, ALL_HEALTHY);
    // Edited back to 40 by hand: the same as the undo.
    f.replace(&three(Print::Stale, 40));
    marked(&f, DRIFTED);
    answer(f.sessions.rollback(&f.id)).result.unwrap();
    marked(&f, DRIFTED);
}

#[test]
fn a_commit_that_writes_nothing_keeps_the_model_at_the_new_revision() {
    let f = opened(&three(Print::Stale, 40));
    marked(&f, DRIFTED);
    f.begin();
    f.replace(&three(Print::Stale, 41));
    marked(&f, ALL_HEALTHY);
    // The same document in other bytes: canonically equal to what `begin` saw.
    f.replace(&three(Print::Stale, 40).replace(", ", ","));
    let before = f.revision();
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(!committed.written);
    assert!(f.revision() >= before);
    marked(&f, DRIFTED);
    // Not even the block's drift was written over.
    assert!(f.text().contains("8000.000001"));
}

// --- Concurrency ----------------------------------------------------------------------------

/// What every timeline answer holds, whatever was running: of the session's revision, its
/// model no newer and named by the response if it has one, every feature of a refused
/// document unknown (with no model, no bodies; under an older published model, its
/// bodies).
#[track_caller]
fn sound(response: &Response<TimelineAnswer>, choice: ModelChoice) {
    let Ok(answer) = &response.result else {
        return;
    };
    let session = response.session.as_ref().unwrap();
    assert_eq!(answer.revision, session.revision, "{choice:?}");
    match answer.model {
        Some(model) => {
            assert!(model <= answer.revision, "{choice:?}");
            assert_eq!(
                Some(model),
                response.model.as_ref().map(|m| m.revision),
                "{choice:?}"
            );
            if choice == ModelChoice::Current {
                assert_eq!(model, answer.revision);
            }
            // A refused document under the published model of an older revision: named,
            // its bodies given, nothing of the document marked.
            if answer.problems > 0 {
                assert_eq!(choice, ModelChoice::Published);
                assert!(model < answer.revision, "{choice:?}");
                for e in answer.features.iter().flatten() {
                    assert_eq!((e.health, e.problems, e.warnings), (Health::Unknown, 0, 0));
                }
            }
        }
        None => {
            assert!(answer.problems >= 1, "{choice:?}");
            assert!(answer.bodies.is_empty(), "{choice:?}");
            for e in answer.features.iter().flatten() {
                assert_eq!((e.health, e.problems, e.warnings), (Health::Unknown, 0, 0));
            }
        }
    }
    for e in answer.features.iter().flatten() {
        assert!([BLOCK, LUG, FUSE].contains(&e.id.as_str()), "{e:?}");
        if e.problems > 0 {
            assert_eq!(e.health, Health::Error, "{e:?}");
        }
    }
}

#[test]
fn timelines_of_both_models_from_four_threads_during_edits_and_reloads_are_each_sound() {
    let f = opened(&three(Print::Stale, 40));
    current(&f);
    f.begin();
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for n in 0..4 {
            let f = &f;
            let done = &done;
            scope.spawn(move || {
                let choice = ModelChoice::ALL[n % 2];
                let mut answered = 0;
                while !done.load(Ordering::Relaxed) {
                    let response = f.timeline(choice);
                    sound(&response, choice);
                    answered += u32::from(response.result.is_ok());
                }
                assert!(answered > 0, "{choice:?}");
            });
        }
        /// Stops the readers however the edits end.
        struct Done<'a>(&'a AtomicBool);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let _done = Done(&done);
        for step in 0..40u32 {
            let text = match step % 5 {
                0 => three(Print::Stale, 41),
                1 => document(&[
                    block_line(40, Print::Missing),
                    lug_line(30, false),
                    fuse_line(true),
                ]),
                2 => three(Print::Stale, 0),
                3 => r#"{"silico": 1, "units": "#.to_owned(),
                _ => document(&[block_line(40, Print::Right), lug_line(40, true)]),
            };
            // The gate's refusal of the 0-long block is the edit's: the session is as it was.
            drop(answer(f.sessions.replace_text(&f.id, f.revision(), text)));
        }
        // Then, the transaction rolled back, the file changed on disk each time, the gate
        // refusing every third, reloaded by the next command.
        answer(f.sessions.rollback(&f.id)).result.unwrap();
        let rolled_back = f.revision().number();
        for step in 0..60u32 {
            let text = match step % 3 {
                0 => three(Print::Missing, 0),
                1 => document(&[block_line(40, Print::Right), lug_line(20, false)]),
                _ => r#"{"silico": 1, "units": "#.to_owned(),
            };
            // Written whole, by a rename, so no reader's reload finds it part written.
            let next = f.path.with_extension("next");
            std::fs::write(&next, text).unwrap();
            std::fs::rename(&next, &f.path).unwrap();
            assert_eq!(f.revision().number(), rolled_back + u64::from(step) + 1);
        }
    });
    // Back to the first state: as it was.
    std::fs::write(&f.path, three(Print::Stale, 40)).unwrap();
    marked(&f, DRIFTED);
}

// --- Random runs, each step's timeline against the session's check -----------------------------

#[derive(Debug, Clone)]
enum Text {
    Parts {
        print: Print,
        lug: u32,
        lug_printed: bool,
        fuse: Option<bool>,
    },
    Refused,
    Unread,
}

impl Text {
    fn text(&self) -> String {
        match self {
            Text::Parts {
                print,
                lug,
                lug_printed,
                fuse,
            } => {
                let mut lines = vec![block_line(40, *print), lug_line(*lug, *lug_printed)];
                if let Some(printed) = fuse {
                    lines.push(fuse_line(*printed));
                }
                document(&lines)
            }
            Text::Refused => three(Print::Missing, 0),
            Text::Unread => r#"{"silico": 1, "units": "#.to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
enum Step {
    Begin,
    Commit,
    Rollback,
    Undo,
    Redo,
    Replace(Text),
    OnDisk(Text),
    /// `format`, writing, accepting every drift or none.
    Format(bool),
}

fn texts() -> impl Strategy<Value = Text> {
    prop_oneof![
        6 => (
            prop_oneof![Just(Print::Right), Just(Print::Stale), Just(Print::Missing)],
            prop_oneof![Just(10u32), Just(20), Just(30), Just(40)],
            any::<bool>(),
            prop_oneof![Just(None), Just(Some(true)), Just(Some(false))],
        )
            .prop_map(|(print, lug, lug_printed, fuse)| Text::Parts {
                print,
                lug,
                lug_printed,
                fuse,
            }),
        1 => Just(Text::Refused),
        1 => Just(Text::Unread),
    ]
}

fn steps() -> impl Strategy<Value = Step> {
    prop_oneof![
        2 => Just(Step::Begin),
        1 => Just(Step::Commit),
        1 => Just(Step::Rollback),
        2 => Just(Step::Undo),
        1 => Just(Step::Redo),
        4 => texts().prop_map(Step::Replace),
        1 => texts().prop_map(Step::OnDisk),
        1 => any::<bool>().prop_map(Step::Format),
    ]
}

/// A check's problems and warnings, counted by the feature each is on.
fn by_feature(check: &CheckAnswer) -> (BTreeMap<String, u64>, BTreeMap<String, u64>) {
    let mut problems = BTreeMap::new();
    for e in check.problems.errors() {
        *problems
            .entry(e.feature().unwrap_or("").to_owned())
            .or_insert(0) += 1;
    }
    let mut warnings = BTreeMap::new();
    for w in check.warnings.listed() {
        *warnings
            .entry(w.feature().unwrap_or("").to_owned())
            .or_insert(0) += 1;
    }
    (problems, warnings)
}

/// The timeline of `choice` and the session's check of the same model agree: of a model,
/// each feature's problems and warnings are the check's on it; of none, the gate's count.
fn agree(f: &Fixture, choice: ModelChoice, check_model: CheckModel) -> Result<(), TestCaseError> {
    let timeline = f.timeline(choice);
    let check = f.check(check_model);
    sound(&timeline, choice);
    let (t, c) = match (&timeline.result, &check.result) {
        (Ok(t), Ok(c)) => (t, c),
        (Err(_), Err(_)) => return Ok(()),
        // A published read races its rebuild: the check, after it, may find one published.
        _ if choice == ModelChoice::Published => return Ok(()),
        (t, c) => {
            return Err(TestCaseError::fail(format!(
                "the timeline and the check disagree on answering: {:?} / {:?}",
                t.as_ref().map(|_| ()).map_err(|p| p.errors().to_vec()),
                c.as_ref().map(|_| ()).map_err(|p| p.errors().to_vec()),
            )));
        }
    };
    let models = (
        timeline.model.as_ref().map(|m| m.revision),
        check.model.as_ref().map(|m| m.revision),
    );
    if models.0 != models.1 {
        // Not the same model: a published read raced a rebuild.
        prop_assert_eq!(choice, ModelChoice::Published);
        return Ok(());
    }
    // The features are the session's document's, in its order: read by `text`, which
    // reloads a change on disk a read beside the queue doesn't, so compared only at the
    // same revision.
    let read = answer(f.sessions.text(&f.id, None));
    let text = read.result.unwrap().text;
    if read.session.unwrap().revision == t.revision {
        let ids = silico_core::document::read(text.as_bytes()).ok().map(|d| {
            d.features
                .iter()
                .map(|f| f.id.as_str().to_owned())
                .collect::<Vec<_>>()
        });
        let listed = t.features.as_ref().map(|fs| {
            fs.iter()
                .map(|e| e.id.as_str().to_owned())
                .collect::<Vec<_>>()
        });
        prop_assert_eq!(&listed, &ids, "{}", text);
    }
    match t.model {
        None => {
            if models.1.is_none() {
                prop_assert_eq!(t.problems, c.problems.total());
            }
        }
        Some(model) if model == t.revision => {
            let (problems, warnings) = by_feature(c);
            prop_assert_eq!(c.problems.total(), c.problems.errors().len() as u64);
            for e in t.features.iter().flatten() {
                let id = e.id.as_str();
                prop_assert_eq!(
                    e.problems,
                    problems.get(id).copied().unwrap_or(0),
                    "problems of {} in {}: {:?}",
                    id,
                    text,
                    c.problems.errors()
                );
                prop_assert_eq!(
                    e.warnings,
                    warnings.get(id).copied().unwrap_or(0),
                    "warnings of {} in {}",
                    id,
                    text
                );
                match e.health {
                    Health::Healthy => prop_assert_eq!((e.problems, e.warnings), (0, 0)),
                    Health::Warning => prop_assert!(e.problems == 0 && e.warnings > 0),
                    Health::Unknown => prop_assert_eq!((e.problems, e.warnings), (0, 0)),
                    Health::Error => {}
                    Health::Suppressed => prop_assert!(false, "nothing is suppressed"),
                }
            }
            let sum: u64 = t.features.iter().flatten().map(|e| e.problems).sum();
            prop_assert_eq!(sum, c.problems.total());
            let warned: u64 = t.features.iter().flatten().map(|e| e.warnings).sum();
            prop_assert_eq!(warned, c.warnings.total());
        }
        // An older published model under a newer document: what changed is unknown, and
        // every other feature is marked as the check of that model has it.
        Some(_) => {
            let (problems, warnings) = by_feature(c);
            for e in t.features.iter().flatten() {
                if e.health == Health::Unknown {
                    prop_assert_eq!((e.problems, e.warnings), (0, 0));
                    continue;
                }
                let id = e.id.as_str();
                prop_assert_eq!(
                    e.problems,
                    problems.get(id).copied().unwrap_or(0),
                    "{}",
                    text
                );
                prop_assert_eq!(
                    e.warnings,
                    warnings.get(id).copied().unwrap_or(0),
                    "{}",
                    text
                );
            }
        }
    }
    // The same read twice, of the same model at the same revision, is the same answer.
    let again = f.timeline(choice);
    if again.model.as_ref().map(|m| m.revision) == models.0
        && again.session.as_ref().map(|s| s.revision) == Some(t.revision)
    {
        prop_assert_eq!(again.result.as_ref().ok(), Some(t));
    }
    Ok(())
}

fn run(f: &Fixture, step: &Step) {
    let revision = f.revision();
    match step {
        Step::Begin => drop(answer(f.sessions.begin(&f.id))),
        Step::Commit => drop(answer(f.sessions.commit(&f.id))),
        Step::Rollback => drop(answer(f.sessions.rollback(&f.id))),
        Step::Undo => drop(answer(f.sessions.undo(&f.id, revision, None))),
        Step::Redo => drop(answer(f.sessions.redo(&f.id, revision, None))),
        Step::Replace(text) => drop(answer(f.sessions.replace_text(
            &f.id,
            revision,
            text.text(),
        ))),
        Step::OnDisk(text) => std::fs::write(&f.path, text.text()).unwrap(),
        Step::Format(all) => drop(answer(f.sessions.format(
            &f.id,
            if *all { None } else { Some(vec![]) },
            true,
        ))),
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn every_steps_timeline_agrees_with_the_sessions_check_of_the_same_model(
        first in texts(),
        steps in prop::collection::vec((steps(), any::<bool>()), 1..7),
    ) {
        let f = opened(&first.text());
        agree(&f, ModelChoice::Current, CheckModel::Current { fresh: false })?;
        for (step, published_first) in &steps {
            run(&f, step);
            // The published model read first, before the current one's rebuild is waited
            // for, may be an older revision's.
            if *published_first {
                agree(&f, ModelChoice::Published, CheckModel::Published)?;
            }
            agree(&f, ModelChoice::Current, CheckModel::Current { fresh: false })?;
            agree(&f, ModelChoice::Published, CheckModel::Published)?;
        }
    }
}

// --- A published model older than a document the gate refuses --------------------------------------

#[test]
fn an_older_faulted_model_under_a_refused_document_marks_nothing_of_it() {
    // Revision 1: the lug 40 long crashes the worker (a kernel fault, decision 32), so the
    // lug and the fuse are unknown, the fault given beside the result.
    let f = opened(&document(&[
        block_line(40, Print::Right),
        lug_line(40, false),
        fuse_line(false),
    ]));
    let first = f.timeline(ModelChoice::Current);
    let fault = first.model.as_ref().unwrap().fault.as_ref().unwrap();
    assert_eq!(fault.code(), ErrorCode::WorkerCrashed);
    assert_eq!(fault.feature(), Some(LUG));
    assert_eq!(
        marks(first.result.unwrap().features.as_ref().unwrap()),
        [
            mark(BLOCK, Health::Healthy, 0, 0),
            mark(LUG, Health::Unknown, 0, 0),
            mark(FUSE, Health::Unknown, 0, 0),
        ]
    );
    // The file then changed outside the session to a block 0 long, which the gate refuses
    // (one problem): reloaded at revision 2, nothing built of it.
    std::fs::write(&f.path, three(Print::Missing, 0)).unwrap();
    let refused = f.timeline(ModelChoice::Current);
    assert!(refused.model.is_none());
    let refused = refused.result.unwrap();
    assert_eq!(refused.revision, Revision::from_number(2).unwrap());
    assert_eq!((refused.model, refused.problems), (None, 1));
    // Read beside the queue: revision 1's model is the newest published, but it isn't of
    // this document, so nothing of it is marked: the refused answer, but naming revision 1's
    // model, as the response does, and its body, the block (the lug faulted, and the fuse,
    // stopped by it, built nothing), which a client still draws.
    let published = f.timeline(ModelChoice::Published);
    let read = published.model.clone().unwrap();
    assert_eq!(read.revision, Revision::OPENED);
    assert_eq!(
        published.result.unwrap(),
        TimelineAnswer {
            model: Some(Revision::OPENED),
            bodies: bodies(&[BLOCK]),
            ..refused
        }
    );
    // SPEC QUESTION (not asserted either way): the response names revision 1's fault on the
    // lug (`read.fault`) beside an answer that marks nothing of it; and the session's
    // `check` of the published model gives revision 1's problems (none), where the timeline
    // gives the gate's count (1).
    let check = f.check(CheckModel::Published);
    assert_eq!(
        check.model.as_ref().map(|m| m.revision),
        Some(Revision::OPENED)
    );
    let _ = (read.fault, check.result.unwrap().problems.total());
}

// --- The sessions dropped while a timeline waits ------------------------------------------------

#[test]
fn a_current_timeline_waiting_on_a_rebuild_when_the_sessions_are_dropped_answers() {
    let f = opened(&three(Print::Right, 40));
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let waiting = f.sessions.timeline(&f.id, ModelChoice::Current);
    assert!(f.kernel.wait_running(1, crate::sessions::HANG));
    let Fixture { _dir, sessions, .. } = f;
    drop(sessions);
    let response = answer(waiting);
    let problems = response.result.unwrap_err();
    assert!(!problems.errors().is_empty());
    hold.release();
    drop(_dir);
}

// --- Sketches, fillets, booleans, thousands of features, and moves -----------------------------

/// Sessions on a fake kernel whose sketches a fake solves, with `text` in `a.silico` open:
/// the fixture and the solver, which answers each sketch as sent unless told otherwise.
fn opened_sketched(text: &str) -> (Fixture, silico_core::fake::FakeSketches) {
    use silico_core::sketch::SketchSolver;
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = silico_core::fake::FakeSketches::new();
    let solver = sketches.clone();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("a.silico");
    std::fs::write(&path, text).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    (
        Fixture {
            _dir: dir,
            kernel,
            sessions,
            id,
            path,
        },
        sketches,
    )
}

/// The timeline of `choice` read now, and the session's check of the same model: asserted
/// to be of the same model, every feature's problems and warnings the check's on it (but
/// for one the model hasn't built since, unknown), its health as its counts say, a feature
/// the check finds a problem on an error, and, of the session's revision's model, the
/// totals the check's. The timeline's answer and the check's.
#[track_caller]
fn agrees_with_check(f: &Fixture, choice: ModelChoice) -> (TimelineAnswer, CheckAnswer) {
    let response = f.timeline(choice);
    let check = f.check(match choice {
        ModelChoice::Current => CheckModel::Current { fresh: false },
        ModelChoice::Published => CheckModel::Published,
    });
    let t = response.result.unwrap();
    let c = check.result.unwrap();
    assert_eq!(
        t.model,
        response.model.as_ref().map(|m| m.revision),
        "{choice:?}"
    );
    assert_eq!(
        t.model,
        check.model.as_ref().map(|m| m.revision),
        "{choice:?}"
    );
    let (problems, warnings) = by_feature(&c);
    let latest = t.model == Some(t.revision);
    let all_problems_listed = c.problems.errors().len() as u64 == c.problems.total();
    let all_warnings_listed = c.warnings.listed().len() as u64 == c.warnings.total();
    for e in t.features.iter().flatten() {
        let id = e.id.as_str();
        match e.health {
            Health::Healthy => assert_eq!((e.problems, e.warnings), (0, 0), "{e:?}"),
            Health::Warning => assert!(e.problems == 0 && e.warnings > 0, "{e:?}"),
            Health::Unknown => assert_eq!((e.problems, e.warnings), (0, 0), "{e:?}"),
            Health::Error => {}
            Health::Suppressed => panic!("nothing is suppressed: {e:?}"),
        }
        if e.health == Health::Unknown && !latest {
            continue;
        }
        // Past the hundred a check lists, only what it lists is known by feature.
        let on = problems.get(id).copied().unwrap_or(0);
        let warned = warnings.get(id).copied().unwrap_or(0);
        if all_problems_listed {
            assert_eq!(
                e.problems,
                on,
                "{choice:?}: problems of {id}: {:?}",
                c.problems.errors()
            );
        } else {
            assert!(e.problems >= on, "{choice:?}: problems of {id}");
        }
        if all_warnings_listed {
            assert_eq!(e.warnings, warned, "{choice:?}: warnings of {id}");
        } else {
            assert!(e.warnings >= warned, "{choice:?}: warnings of {id}");
        }
        if on > 0 {
            assert_eq!(e.health, Health::Error, "{choice:?}: {e:?}");
        }
    }
    if latest {
        let sum: u64 = t.features.iter().flatten().map(|e| e.problems).sum();
        assert_eq!(sum, c.problems.total(), "{choice:?}");
        let warned: u64 = t.features.iter().flatten().map(|e| e.warnings).sum();
        assert_eq!(warned, c.warnings.total(), "{choice:?}");
    }
    (t, c)
}

/// Each feature's mark, by ID.
fn marks_by_id(answer: &TimelineAnswer) -> BTreeMap<String, (Health, u64, u64)> {
    answer
        .features
        .iter()
        .flatten()
        .map(|e| (e.id.as_str().to_owned(), (e.health, e.problems, e.warnings)))
        .collect()
}

fn ids_of(answer: &TimelineAnswer) -> Vec<String> {
    answer
        .features
        .iter()
        .flatten()
        .map(|e| e.id.as_str().to_owned())
        .collect()
}

/// A box `id` at x = `x`, 1 × 1 × 1 unless `length` says otherwise, with no fingerprint.
fn cube(id: &str, x: u32, length: u32) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Cube", "position": [{x}, 0, 0], "length": {length}, "width": 1, "height": 1}}"#
    )
}

fn boolean(kind: &str, id: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "Joined", "base": "{base}", "tool": "{tool}"}}"#
    )
}

fn fillet(id: &str, base: &str, edges: &[&str]) -> String {
    let edges: Vec<String> = edges.iter().map(|e| format!("\"{e}\"")).collect();
    format!(
        r#"{{"id": "{id}", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": [{}], "radius": 0.1}}"#,
        edges.join(", ")
    )
}

/// Three cubes, the first two fused and the third cut from that; a fillet naming two edges
/// the third cube hasn't (a box has twelve); a sketch's rectangle and its pad.
fn mixed() -> Vec<String> {
    vec![
        cube("aaaaaaaa", 0, 1),
        cube("cccccccc", 5, 1),
        cube("bbbbbbbb", 10, 1),
        boolean("fuse", "j0000001", "aaaaaaaa", "cccccccc"),
        fillet(
            "f0000002",
            "bbbbbbbb",
            &["bbbbbbbb.box.Edge90", "bbbbbbbb.box.Edge91"],
        ),
        crate::sweeps::rectangle("s0000001", "xy", [20.0, 0.0], 4.0, 2.0),
        crate::sweeps::pad("p0000001", None, "s0000001", crate::sweeps::TEN),
    ]
}

#[test]
fn broken_fillet_references_and_a_fuse_mark_as_check_has_them() {
    let (f, _sketches) = opened_sketched(&document(&mixed()));
    let (t, c) = agrees_with_check(&f, ModelChoice::Current);
    let m = marks_by_id(&t);
    // Two names the cube hasn't: two problems, each `reference_broken`, by hand.
    assert_eq!(m["f0000002"], (Health::Error, 2, 0));
    assert_eq!(
        c.problems
            .errors()
            .iter()
            .filter(|e| e.feature() == Some("f0000002"))
            .map(|e| e.code())
            .collect::<Vec<_>>(),
        [ErrorCode::ReferenceBroken, ErrorCode::ReferenceBroken]
    );
    // The fuse consumed both cubes: the fuse, the third cube (its fillet failed), and the
    // pad are the bodies, in timeline order.
    assert_eq!(t.bodies, bodies(&["bbbbbbbb", "j0000001", "p0000001"]));
    // The published model is the same one.
    let (p, _) = agrees_with_check(&f, ModelChoice::Published);
    assert_eq!(p, t);
}

#[test]
fn a_kernel_fault_mid_chain_is_unknown_with_what_it_alone_blocks() {
    let (f, _sketches) = opened_sketched(&document(&[
        cube("aaaaaaaa", 0, 1),
        cube("cccccccc", 5, 1),
        boolean("fuse", "j0000001", "aaaaaaaa", "cccccccc"),
        fillet("f0000001", "j0000001", &["aaaaaaaa.box.Edge1"]),
        cube("dddddddd", 30, 1),
    ]));
    f.kernel.refuse(|op| {
        matches!(op, Op::Boolean { .. })
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the fuse crashed the worker"))
    });
    let response = f.timeline(ModelChoice::Current);
    let fault = response.model.as_ref().unwrap().fault.clone().unwrap();
    assert_eq!(
        (fault.code(), fault.feature()),
        (ErrorCode::WorkerCrashed, Some("j0000001"))
    );
    let (t, c) = agrees_with_check(&f, ModelChoice::Current);
    let m = marks_by_id(&t);
    assert_eq!(m["j0000001"], (Health::Unknown, 0, 0));
    assert_eq!(m["f0000001"], (Health::Unknown, 0, 0));
    // Built, but with no fingerprint: a warning each.
    assert_eq!(m["aaaaaaaa"], (Health::Warning, 0, 1));
    assert_eq!(m["dddddddd"], (Health::Warning, 0, 1));
    assert_eq!(c.problems.total(), 0, "{:?}", c.problems.errors());
}

#[test]
fn a_sketch_solved_otherwise_than_its_file_is_an_error_as_check_has_it() {
    let (f, sketches) = opened_sketched(&document(&mixed()));
    // The rectangle's last value, its last corner's y (2), solved to 7: a solve, settled,
    // that the file doesn't hold (drift).
    sketches.answer(|system| {
        let mut solved = silico_core::fake::FakeSketches::as_sent(system);
        let n = solved.values.len();
        solved.values[n - 1] = 7.0;
        Ok(solved)
    });
    let (t, c) = agrees_with_check(&f, ModelChoice::Current);
    assert_eq!(marks_by_id(&t)["s0000001"].0, Health::Error);
    assert_eq!(
        c.problems
            .errors()
            .iter()
            .filter(|e| e.feature() == Some("s0000001"))
            .map(|e| e.code())
            .collect::<Vec<_>>(),
        [ErrorCode::Drift],
        "{:?}",
        c.problems.errors()
    );
    // In a transaction, an edit reaching the sketch excepts its drift, in both.
    f.begin();
    let moved = document(&mixed()).replace(r#""x": 20, "y": 0"#, r#""x": 20.5, "y": 0"#);
    assert_ne!(moved, document(&mixed()));
    f.replace(&moved);
    let (t, c) = agrees_with_check(&f, ModelChoice::Current);
    assert_ne!(
        marks_by_id(&t)["s0000001"].0,
        Health::Error,
        "{:#?}",
        c.problems.errors()
    );
}

#[test]
fn a_crashed_sketch_worker_is_unknown_with_its_pad_as_check_has_them() {
    let (f, sketches) = opened_sketched(&document(&mixed()));
    sketches.answer(|_| {
        Err(Error::new(
            ErrorCode::WorkerCrashed,
            "the sketch worker crashed",
        ))
    });
    let response = f.timeline(ModelChoice::Current);
    let read = response.model.clone().unwrap();
    let (t, _) = agrees_with_check(&f, ModelChoice::Current);
    let m = marks_by_id(&t);
    // A fault, not the document's: unknown, and the pad it alone blocks.
    let fault = read.fault.unwrap();
    assert_eq!(fault.code(), ErrorCode::WorkerCrashed);
    assert_eq!(fault.feature(), Some("s0000001"));
    assert_eq!(m["s0000001"], (Health::Unknown, 0, 0));
    assert_eq!(m["p0000001"], (Health::Unknown, 0, 0));
    // The fillet's broken names are still the document's.
    assert_eq!(m["f0000002"], (Health::Error, 2, 0));
    // The worker comes back: the next revision's rebuild solves it.
    sketches.answer(|system| Ok(silico_core::fake::FakeSketches::as_sent(system)));
    let next =
        document(&mixed()).replace(r#""length": 1, "width": 1"#, r#""length": 2, "width": 1"#);
    f.replace(&next);
    let (t, _) = agrees_with_check(&f, ModelChoice::Current);
    let m = marks_by_id(&t);
    assert_ne!(m["s0000001"].0, Health::Unknown);
    assert_ne!(m["p0000001"].0, Health::Unknown);
}

/// `n` cubes, each a step apart along x, none with a fingerprint, and a fillet for every
/// tenth naming an edge it hasn't.
fn many(n: u32) -> Vec<String> {
    let mut lines = Vec::new();
    for i in 0..n {
        let cube_id = format!("c{i:07}");
        lines.push(cube(&cube_id, 2 * i, 1));
        if i % 10 == 0 {
            lines.push(fillet(
                &format!("r{i:07}"),
                &cube_id,
                &[&format!("{cube_id}.box.Edge99")],
            ));
        }
    }
    lines
}

#[test]
fn two_thousand_features_and_their_reversal_mark_as_check_counts_them() {
    let lines = many(2000);
    let (f, _sketches) = opened_sketched(&document(&lines));
    let (t, c) = agrees_with_check(&f, ModelChoice::Current);
    // By hand: 200 fillets, each one broken name; 2000 cubes built, each warned once.
    assert_eq!(c.problems.total(), 200);
    assert_eq!(c.warnings.total(), 2000);
    let m = marks_by_id(&t);
    assert_eq!(m.len(), 2200);
    assert_eq!(m["r0000000"], (Health::Error, 1, 0));
    assert_eq!(m["c0001999"], (Health::Warning, 0, 1));
    // Every feature moved: the timeline reversed, a fillet before its cube is refused by the
    // gate, so the pairs keep their order, each pair's place reversed.
    let mut pairs: Vec<Vec<String>> = Vec::new();
    for line in lines {
        if line.contains(r#""type": "fillet""#) {
            pairs.last_mut().unwrap().push(line);
        } else {
            pairs.push(vec![line]);
        }
    }
    pairs.reverse();
    let reversed: Vec<String> = pairs.into_iter().flatten().collect();
    f.replace(&document(&reversed));
    let (r, _) = agrees_with_check(&f, ModelChoice::Current);
    let want: Vec<String> = reversed.iter().map(|l| l[8..16].to_owned()).collect();
    assert_eq!(ids_of(&r), want);
    // Nothing changed but the order: every mark as it was.
    assert_eq!(marks_by_id(&r), m);
}

#[test]
fn an_older_published_model_under_moved_features_marks_each_as_before_and_what_changed_unknown() {
    let (f, _sketches) = opened_sketched(&document(&mixed()));
    let (before, _) = agrees_with_check(&f, ModelChoice::Current);
    let old = marks_by_id(&before);
    // Revision 2: the sketch and its pad moved to the front, the third cube 3 long (its
    // rebuild held, so the published model stays revision 1's).
    let hold = f.kernel.hold(
        |op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 2.5 && *dx < 3.5),
    );
    let mut moved = mixed();
    let pad = moved.pop().unwrap();
    let sketch = moved.pop().unwrap();
    moved.insert(0, pad);
    moved.insert(0, sketch);
    moved[4] = cube("bbbbbbbb", 10, 3);
    // In a transaction, so the edit doesn't wait for its rebuild to save.
    f.begin();
    f.replace(&document(&moved));
    let response = f.timeline(ModelChoice::Published);
    let published = response.result.unwrap();
    assert_eq!(published.revision, Revision::from_number(2).unwrap());
    assert_eq!(published.model, Some(Revision::OPENED));
    assert_eq!(
        ids_of(&published),
        [
            "s0000001", "p0000001", "aaaaaaaa", "cccccccc", "bbbbbbbb", "j0000001", "f0000002"
        ]
    );
    let now = marks_by_id(&published);
    // The changed cube and the fillet on it unknown; every other feature as before.
    for (feature, mark) in &now {
        if feature == "bbbbbbbb" || feature == "f0000002" {
            assert_eq!(*mark, (Health::Unknown, 0, 0), "{feature}");
        } else {
            assert_eq!(*mark, old[feature], "{feature}");
        }
    }
    agrees_with_check(&f, ModelChoice::Published);
    hold.release();
    let (after, _) = agrees_with_check(&f, ModelChoice::Current);
    assert_eq!(after.model, Some(Revision::from_number(2).unwrap()));
}

// --- Published reads at the limit, cancel, and close -----------------------------------------------

#[test]
fn a_published_timeline_past_eight_held_reads_is_refused_and_answers_once_they_end() {
    use silico_core::command::{MAX_PUBLISHED_READS, ReadCommand};
    use std::sync::{Mutex, mpsc};
    let f = opened(&three(Print::Right, 40));
    let built = current(&f);
    let (release, released) = mpsc::channel::<()>();
    let released = Arc::new(Mutex::new(released));
    let (started, starts) = mpsc::channel::<()>();
    let held: Vec<_> = (0..MAX_PUBLISHED_READS)
        .map(|_| {
            let released = Arc::clone(&released);
            let started = started.clone();
            f.sessions.read_model(
                &f.id,
                ReadCommand::Check,
                ModelChoice::Published,
                move |_| {
                    started.send(()).unwrap();
                    let _ = released.lock().unwrap().recv();
                    Ok(())
                },
            )
        })
        .collect();
    for _ in 0..MAX_PUBLISHED_READS {
        starts.recv_timeout(crate::sessions::HANG).unwrap();
    }
    let past = f.timeline(ModelChoice::Published);
    let (code, message, _, suggestion) = refusal(&past);
    assert_eq!(code, ErrorCode::InvalidInput);
    assert_eq!(
        message,
        format!(
            "{MAX_PUBLISHED_READS} published reads are running in session {}, the most one session runs at once",
            f.id
        )
    );
    assert_eq!(
        suggestion.as_deref(),
        Some("wait for one to answer, then read again")
    );
    // The current model's, queued, isn't counted among them.
    assert_eq!(current(&f), built);
    drop(release);
    for read in held {
        answer(read).result.unwrap();
    }
    assert_eq!(f.timeline(ModelChoice::Published).result.unwrap(), built);
}

#[test]
fn a_current_timeline_cancelled_while_a_close_waits_behind_it_lets_the_close_through() {
    use silico_core::command::SessionCommand;
    let f = opened(&three(Print::Right, 40));
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let waiting = f.sessions.timeline(&f.id, ModelChoice::Current);
    assert!(f.kernel.wait_running(1, crate::sessions::HANG));
    let close = f.sessions.close(&f.id);
    let cancelled = f.sessions.cancel(&f.id, SessionCommand::Timeline);
    assert!(cancelled.result.unwrap().stopped);
    let timeline = answer(waiting);
    assert_eq!(
        timeline.result.unwrap_err().errors()[0].code(),
        ErrorCode::Cancelled
    );
    answer(close).result.unwrap();
    hold.release();
    for choice in ModelChoice::ALL {
        let (code, _, parameter, _) = refusal(&f.timeline(choice));
        assert_eq!(
            (code, parameter.as_deref()),
            (ErrorCode::InvalidInput, Some("session"))
        );
    }
}

// ---- `format` naming a sketch, inside and outside a transaction (#297's review) ----

/// A rectangle sketch and its pad, the pad's stored fingerprint not what it builds (a
/// volume of 1), opened with a solver that solves the rectangle's last corner's y to 7, so
/// the sketch is solved otherwise than its file (drift) and the pad builds otherwise than
/// its stored print (drift).
fn sketch_and_stale_pad() -> (Fixture, silico_core::fake::FakeSketches, String) {
    const STALE: &str = r#", "fingerprint": {"volume": 1, "area": 1, "bbox": [0, 0, 0, 1, 1, 1], "faces": 6, "edges": 12, "vertices": 8}}"#;
    let pad = crate::sweeps::pad("p0000001", None, "s0000001", crate::sweeps::TEN);
    let pad = format!("{}{STALE}", pad.strip_suffix('}').unwrap());
    let text = document(&[
        crate::sweeps::rectangle("s0000001", "xy", [20.0, 0.0], 4.0, 2.0),
        pad,
    ]);
    let (f, sketches) = opened_sketched(&text);
    sketches.answer(|system| {
        let mut solved = silico_core::fake::FakeSketches::as_sent(system);
        let n = solved.values.len();
        solved.values[n - 1] = 7.0;
        Ok(solved)
    });
    (f, sketches, STALE.trim_start_matches(", ").to_owned())
}

/// The pad's line, as the file holds it now.
fn pad_line(f: &Fixture) -> String {
    let file = std::fs::read_to_string(&f.path).unwrap();
    let at = file.find(r#""id": "p0000001""#).unwrap();
    file[at..].lines().take(20).collect::<Vec<_>>().join("\n")
}

#[test]
fn format_outside_a_transaction_naming_the_sketch_keeps_the_pads_stored_print() {
    let (f, _sketches, stale) = sketch_and_stale_pad();
    let formatted = answer(f.sessions.format(&f.id, Some(vec![id("s0000001")]), true))
        .result
        .unwrap();
    assert_eq!(formatted.accepted, [id("s0000001")]);
    assert_eq!(formatted.resolved, [id("s0000001")]);
    assert!(!formatted.refingerprinted.contains(&id("p0000001")));
    // Its drift isn't accepted: a problem, and its stored print kept.
    assert!(
        formatted
            .problems
            .errors()
            .iter()
            .any(|e| { e.feature() == Some("p0000001") && e.code() == ErrorCode::Drift })
    );
    let pad = pad_line(&f);
    assert!(pad.contains(r#""volume": 1,"#), "{pad} (was {stale})");
}

#[test]
fn format_inside_a_transaction_naming_the_sketch_keeps_the_pads_stored_print() {
    // As outside (the twin above): the sketch's solve, written by the format, is no edit of
    // it, so its pad isn't reached, and the commit keeps the pad's print, its drift a
    // problem, never accepted silently.
    let (f, _sketches, stale) = sketch_and_stale_pad();
    f.begin();
    let formatted = answer(f.sessions.format(&f.id, Some(vec![id("s0000001")]), true))
        .result
        .unwrap();
    assert!(formatted.changed);
    assert_eq!(formatted.accepted, [id("s0000001")]);
    assert!(!formatted.refingerprinted.contains(&id("p0000001")));
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(committed.written);
    assert!(
        !committed.refingerprinted.contains(&id("p0000001")),
        "{:?}",
        committed.refingerprinted
    );
    assert!(
        committed
            .problems
            .errors()
            .iter()
            .any(|e| { e.feature() == Some("p0000001") && e.code() == ErrorCode::Drift })
    );
    let pad = pad_line(&f);
    assert!(pad.contains(r#""volume": 1,"#), "{pad} (was {stale})");
    // The sketch's solve is the file's now.
    assert!(
        std::fs::read_to_string(&f.path)
            .unwrap()
            .contains(r#""y": 7"#)
    );
}

#[test]
fn an_edit_of_a_sketch_after_a_format_in_the_transaction_reaches_its_pad() {
    // What format wrote reaches nothing only while the sketch holds it: edited after, the
    // sketch is the edits', and so is its pad, which the commit refingerprints. The solver
    // gives back what it's sent from then on; the rectangle's top corners raised to y = 3
    // make it 4 × 3 from (20, 0): area 12, perimeter 14, so the pad, 10 high, measures 120,
    // area 2·12 + 14·10 = 164, from (20, 0, 0) to (24, 3, 10), 6 faces, 12 edges, 8 vertices.
    let (f, sketches, _) = sketch_and_stale_pad();
    f.begin();
    let formatted = answer(f.sessions.format(&f.id, Some(vec![id("s0000001")]), true))
        .result
        .unwrap();
    assert!(formatted.changed);
    sketches.answer(|system| Ok(silico_core::fake::FakeSketches::as_sent(system)));
    for point in ["pa000003", "pa000004"] {
        let three = silico_core::document::json::read(b"3").unwrap();
        answer(f.sessions.set_field(
            &f.id,
            f.revision(),
            id("s0000001"),
            format!("points.{point}.y"),
            three,
        ))
        .result
        .unwrap();
    }
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(committed.written);
    assert!(
        committed.refingerprinted.contains(&id("p0000001")),
        "{:?}",
        committed.refingerprinted
    );
    assert!(
        !committed
            .problems
            .errors()
            .iter()
            .any(|e| e.code() == ErrorCode::Drift),
        "{:?}",
        committed.problems.errors()
    );
    let pad = pad_line(&f);
    assert!(
        pad.contains(r#""fingerprint": {"volume": 120, "area": 164, "center": [22, 1.5, 5], "bbox": [20, 0, 0, 24, 3, 10], "faces": 6, "edges": 12, "vertices": 8}"#),
        "{pad}"
    );
}
