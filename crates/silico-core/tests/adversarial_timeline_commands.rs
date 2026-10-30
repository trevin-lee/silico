//! Hostile uses of M5.1's timeline edits (PR #295): `add_feature`'s `before`, `move_feature`,
//! and `remove_feature`'s rewiring, through the session, on the fake kernel. Each test aims
//! at an assumption the commands make: that a rewired chain's fingerprints are written anew
//! by the commit (so the saved file reopens without drift), that a move that changes nothing
//! still checks its revision, that an insert whose input comes after its place is refused
//! by the gate, naming the new feature, that a `tool` is rewired as a `tool`, that a move's
//! undo restores the text byte for byte, that a refused edit leaves the session working, that
//! racing edits at one revision conflict, and that a long chain moves in bounded time.
//! Expected values are worked out by hand from the chain's rules (docs/design/m5.0-app.md,
//! section 7, and A9) and the gate's (every reference to a feature before it).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::cache::Cache;
use silico_core::command::{
    CheckModel, ModelChoice, Response, Revision, Running, SessionId, Sessions,
};
use silico_core::document::chain::Rewired;
use silico_core::document::json::read;
use silico_core::document::{self, FeatureId};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};
use tempfile::TempDir;

use super::bracket::*;

const HANG: Duration = Duration::from_secs(60);

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn answer<T>(running: Running<T>) -> Response<T> {
    match running.wait_timeout(HANG) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in {HANG:?}"),
    }
}

fn errors<T: std::fmt::Debug>(response: &Response<T>) -> Vec<Error> {
    response.result.as_ref().unwrap_err().errors().to_vec()
}

fn rewired(feature: &str, field: &'static str, from: Option<&str>, to: Option<&str>) -> Rewired {
    Rewired {
        feature: id(feature),
        field,
        from: from.map(id),
        to: to.map(id),
    }
}

fn text_of(features: &[&str]) -> String {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    );
    document::write(&document::read(text.as_bytes()).unwrap()).unwrap()
}

fn bracket() -> String {
    text_of(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001,
    ])
}

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
}

fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        kernel,
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("part.silico");
    std::fs::write(&path, text).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    Fixture { dir, sessions, id }
}

/// The bracket saved by `format` with the fake kernel's fingerprints.
fn fingerprinted() -> String {
    let f = opened(&bracket());
    assert!(
        answer(f.sessions.format(&f.id, None, true))
            .result
            .unwrap()
            .written
    );
    f.file()
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("part.silico")
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.path()).unwrap()
    }

    fn revision(&self) -> Revision {
        self.sessions.status(&self.id).session.unwrap().revision
    }

    fn begin(&self) {
        answer(self.sessions.begin(&self.id)).result.unwrap();
    }

    fn text(&self) -> String {
        answer(self.sessions.text(&self.id, None))
            .result
            .unwrap()
            .text
    }

    fn add(&self, before: Option<&str>, feature: &str) -> Response<silico_core::command::Added> {
        answer(self.sessions.add_feature(
            &self.id,
            self.revision(),
            before.map(id),
            read(feature.as_bytes()).unwrap(),
        ))
    }

    fn moved(&self, feature: &str, before: Option<&str>) -> Response<silico_core::command::Moved> {
        answer(
            self.sessions
                .move_feature(&self.id, self.revision(), id(feature), before.map(id)),
        )
    }

    fn remove(&self, feature: &str) -> Response<silico_core::command::Removed> {
        answer(
            self.sessions
                .remove_feature(&self.id, self.revision(), id(feature)),
        )
    }

    fn ids(&self) -> Vec<String> {
        document::read(self.text().as_bytes())
            .unwrap()
            .features
            .iter()
            .map(|f| f.id.to_string())
            .collect()
    }
}

/// The problems a fresh session finds in `file`, rebuilt from nothing: what a user who
/// reopens the saved file sees.
fn reopened_problems(file: &str) -> Vec<(ErrorCode, Option<String>, String)> {
    let f = opened(file);
    let check = answer(f.sessions.check(&f.id, CheckModel::Current { fresh: true }))
        .result
        .unwrap();
    check
        .problems
        .errors()
        .iter()
        .map(|e| {
            (
                e.code(),
                e.feature().map(str::to_owned),
                e.message().to_owned(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// The saved file after a rewiring edit: every fingerprint the rewiring changed written anew.

#[test]
fn a_rewiring_move_committed_reopens_without_drift() {
    // h0000001 to the end: h0000002 takes p0000001 as its base, h0000001 builds on the
    // fillet. h0000002's, f0000001's, and h0000001's solids all differ from before (each
    // built on another solid), so the commit must write all three fingerprints again.
    let file = fingerprinted();
    assert!(reopened_problems(&file).is_empty());
    let f = opened(&file);
    let moved = f.moved("h0000001", None).result.unwrap();
    assert_eq!(
        moved.rewired,
        [
            rewired("h0000002", "base", Some("h0000001"), Some("p0000001")),
            rewired("h0000001", "base", Some("p0000001"), Some("f0000001")),
        ]
    );
    let commit = moved.commit.unwrap();
    assert!(commit.written);
    assert_eq!(
        commit.refingerprinted,
        [id("h0000002"), id("f0000001"), id("h0000001")]
    );
    assert_eq!(reopened_problems(&f.file()), []);
}

#[test]
fn a_rewiring_removal_committed_reopens_without_drift() {
    // h0000001 removed: h0000002 on p0000001, f0000001 on that; both rebuilt otherwise.
    let f = opened(&fingerprinted());
    let removed = f.remove("h0000001").result.unwrap();
    let commit = removed.commit.unwrap();
    assert!(commit.written);
    assert_eq!(commit.refingerprinted, [id("h0000002"), id("f0000001")]);
    assert_eq!(reopened_problems(&f.file()), []);
}

const AGAIN: &str = r#"{"id": "h0000009", "type": "pocket", "v": 1, "name": "Again", "base": "h0000002", "sketch": "s0000003", "extent": {"type": "through_all", "reversed": true}}"#;

#[test]
fn a_rewiring_insert_committed_reopens_without_drift() {
    // A pocket inserted before f0000001 on h0000002: the fillet rewired to it, its solid
    // the fillet of another, so both the new pocket's and the fillet's fingerprints written.
    let f = opened(&fingerprinted());
    let added = f.add(Some("f0000001"), AGAIN).result.unwrap();
    assert_eq!(added.index, 6);
    assert_eq!(
        added.rewired,
        [rewired(
            "f0000001",
            "base",
            Some("h0000002"),
            Some("h0000009")
        )]
    );
    let commit = added.commit.unwrap();
    assert!(commit.written);
    assert_eq!(commit.refingerprinted, [id("h0000009"), id("f0000001")]);
    assert_eq!(reopened_problems(&f.file()), []);
}

#[test]
fn a_rewiring_move_inside_a_transaction_commits_without_drift() {
    // The same move inside a transaction, with an unrelated undo-able edit around it.
    let f = opened(&fingerprinted());
    f.begin();
    f.moved("h0000001", None).result.unwrap();
    f.moved("h0000002", Some("h0000001")).result.unwrap();
    let commit = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(commit.written);
    assert_eq!(reopened_problems(&f.file()), []);
}

// ---------------------------------------------------------------------------------------
// A move that changes nothing.

#[test]
fn a_no_op_move_against_a_stale_revision_is_a_conflict() {
    // Revision 1 is stale once an edit made revision 2: a move to where the feature is,
    // made against it, is refused as every stale edit is, not answered "changed: false".
    let f = opened(&bracket());
    f.begin();
    f.moved("h0000001", None).result.unwrap();
    assert_eq!(f.revision(), Revision::OPENED.next().unwrap());
    let stale = answer(
        f.sessions
            .move_feature(&f.id, Revision::OPENED, id("f0000001"), None),
    );
    let errors = errors(&stale);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code(), ErrorCode::Conflict);
}

#[test]
fn a_no_op_move_outside_a_transaction_leaves_the_file_untouched() {
    let file = fingerprinted();
    let f = opened(&file);
    let before = std::fs::metadata(f.path()).unwrap().modified().unwrap();
    for (feature, place) in [
        ("h0000001", Some("s0000003")),
        ("f0000001", None),
        ("s0000001", Some("p0000001")),
    ] {
        let moved = f.moved(feature, place).result.unwrap();
        assert!(!moved.changed, "{feature} before {place:?}");
        assert_eq!(moved.commit, None);
        assert!(moved.rewired.is_empty());
        assert_eq!(moved.from, moved.to);
    }
    // Before itself names no place: refused, the session as it was.
    let itself = f.moved("h0000001", Some("h0000001"));
    let refused = itself.result.unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("before"));
    assert_eq!(f.revision(), Revision::OPENED);
    assert_eq!(f.file(), file);
    assert_eq!(
        std::fs::metadata(f.path()).unwrap().modified().unwrap(),
        before
    );
}

// ---------------------------------------------------------------------------------------
// Undo, rollback, and refused edits leave the session as it was.

#[test]
fn a_move_and_an_insert_undone_restore_the_text_byte_for_byte() {
    let f = opened(&bracket());
    f.begin();
    let start = f.text();
    let at = f.revision();
    f.moved("h0000001", None).result.unwrap();
    let after_move = f.text();
    f.add(Some("f0000001"), AGAIN).result.unwrap();
    let after_add = f.text();
    assert_ne!(after_move, start);
    assert_ne!(after_add, after_move);
    answer(f.sessions.undo(&f.id, f.revision(), None))
        .result
        .unwrap();
    assert_eq!(f.text(), after_move);
    answer(f.sessions.undo(&f.id, f.revision(), None))
        .result
        .unwrap();
    assert_eq!(f.text(), start);
    answer(f.sessions.redo(&f.id, f.revision(), None))
        .result
        .unwrap();
    assert_eq!(f.text(), after_move);
    answer(f.sessions.rollback(&f.id)).result.unwrap();
    assert_eq!(f.text(), start);
    assert!(f.revision() > at);
}

#[test]
fn every_refused_timeline_edit_leaves_the_session_and_the_next_edit_working() {
    // Each refusal by the chain, worked out by hand, then a move that goes ahead.
    let f = opened(&bracket());
    f.begin();
    let start = f.text();
    let at = f.revision();
    let refusals: Vec<(&str, Vec<Error>)> = vec![
        // Before its sketch.
        (
            "pad before its sketch",
            errors(&f.moved("p0000001", Some("s0000001"))),
        ),
        // After a feature that uses it as its sketch.
        ("sketch after its user", errors(&f.moved("s0000003", None))),
        // A fillet before its chain's root.
        (
            "fillet before the root",
            errors(&f.moved("f0000001", Some("p0000001"))),
        ),
        // Removing the sketch of a pocket.
        ("sketch removed", errors(&f.remove("s0000002"))),
        // An insert whose base is consumed before the place.
        ("consumed before", errors(&f.add(None, AGAIN))),
    ];
    for (what, errors) in &refusals {
        assert_eq!(errors.len(), 1, "{what}: {errors:?}");
        assert_eq!(errors[0].code(), ErrorCode::InvalidInput, "{what}");
        assert!(errors[0].feature().is_some(), "{what}: {errors:?}");
        assert!(errors[0].suggestion().is_some(), "{what}: {errors:?}");
    }
    assert_eq!(f.text(), start);
    assert_eq!(f.revision(), at);
    let moved = f.moved("h0000001", Some("h0000002")).result.unwrap();
    assert!(moved.changed);
}

// ---------------------------------------------------------------------------------------
// An insert whose inputs come after its place.

#[test]
fn an_insert_whose_base_comes_after_its_place_is_refused_naming_the_new_feature() {
    // The pocket on h0000002 inserted before p0000001: the chain rewires f0000001 (at 6,
    // after the place) to it, but the pocket at place 1 would use h0000002 and s0000003,
    // both after it. The gate refuses that, naming the pocket, and the session is as it was.
    let f = opened(&bracket());
    f.begin();
    let start = f.text();
    let response = f.add(Some("p0000001"), AGAIN);
    let errors = errors(&response);
    assert!(
        errors.iter().any(|e| e.feature() == Some("h0000009")
            && e.parameter() == Some("base")
            && e.message().contains("comes after this feature")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.feature() == Some("h0000009") && e.parameter() == Some("sketch")),
        "{errors:?}"
    );
    assert_eq!(f.text(), start);
}

#[test]
fn an_insert_before_the_feature_it_builds_on_is_refused() {
    // A pocket on h0000002 inserted before h0000002 itself: its base would be after it.
    let f = opened(&bracket());
    f.begin();
    let start = f.text();
    let errors = errors(&f.add(Some("h0000002"), AGAIN));
    assert!(
        errors
            .iter()
            .any(|e| e.feature() == Some("h0000009") && e.parameter() == Some("base")),
        "{errors:?}"
    );
    assert_eq!(f.text(), start);
}

#[test]
fn an_insert_with_an_id_the_document_holds_is_refused_before_any_rewire() {
    let f = opened(&bracket());
    f.begin();
    let start = f.text();
    let twin = AGAIN.replace("h0000009", "h0000001");
    let errors = errors(&f.add(Some("f0000001"), &twin));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].parameter(), Some("feature.id"), "{errors:?}");
    assert_eq!(f.text(), start);
}

// ---------------------------------------------------------------------------------------
// Tools.

fn tool_chain() -> String {
    text_of(&[
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 40, "height": 10}"#,
        r#"{"id": "p0000002", "type": "box", "v": 1, "name": "Cube", "position": [10, 10, 10], "length": 20, "width": 20, "height": 20}"#,
        r#"{"id": "c0000002", "type": "chamfer", "v": 1, "name": "One", "base": "p0000002", "edges": ["p0000002.box.Edge1"], "size": {"distance": 1}}"#,
        r#"{"id": "c0000003", "type": "chamfer", "v": 1, "name": "Two", "base": "c0000002", "edges": ["p0000002.box.Edge3"], "size": {"distance": 1}}"#,
        r#"{"id": "w0000001", "type": "fuse", "v": 1, "name": "Both", "base": "b0000001", "tool": "c0000003"}"#,
    ])
}

#[test]
fn an_insert_on_a_booleans_tool_rewires_its_tool_field() {
    // A chamfer on c0000003 (the fuse's tool) before the fuse: the fuse's tool, not its
    // base, now names the chamfer.
    let f = opened(&tool_chain());
    f.begin();
    let added = f
        .add(
            Some("w0000001"),
            r#"{"id": "c0000004", "type": "chamfer", "v": 1, "name": "Three", "base": "c0000003", "edges": ["p0000002.box.Edge5"], "size": {"distance": 1}}"#,
        )
        .result
        .unwrap();
    assert_eq!(added.index, 4);
    assert_eq!(
        added.rewired,
        [rewired(
            "w0000001",
            "tool",
            Some("c0000003"),
            Some("c0000004")
        )]
    );
    let doc = document::read(f.text().as_bytes()).unwrap();
    let fuse = doc
        .features
        .iter()
        .find(|x| x.id == id("w0000001"))
        .unwrap();
    let inputs: Vec<(String, String)> = fuse
        .operation
        .inputs()
        .into_iter()
        .map(|(field, input)| (field.to_owned(), input.to_string()))
        .collect();
    assert_eq!(
        inputs,
        [
            ("base".to_owned(), "b0000001".to_owned()),
            ("tool".to_owned(), "c0000004".to_owned())
        ]
    );
}

#[test]
fn an_insert_of_a_boolean_with_one_solid_twice_is_worded_as_the_gate_words_it() {
    // A fuse of a0000001 with itself: refused before a feature by the chain and at the end
    // by the gate; the chain's doc says "as the gate words it", so the two say the same.
    let fuse = r#"{"id": "w0000002", "type": "fuse", "v": 1, "name": "Self", "base": "a0000001", "tool": "a0000001"}"#;
    let f = opened(&text_of(&[
        r#"{"id": "a0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        r#"{"id": "b0000002", "type": "box", "v": 1, "name": "B", "position": [50, 0, 0], "length": 10, "width": 10, "height": 10}"#,
    ]));
    f.begin();
    let before = errors(&f.add(Some("b0000002"), fuse));
    let end = errors(&f.add(None, fuse));
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(!end.is_empty());
    let end = end
        .iter()
        .find(|e| e.parameter().is_some_and(|p| p.ends_with("tool")))
        .unwrap_or_else(|| panic!("{end:?}"));
    assert_eq!(before[0].message(), end.message());
    assert_eq!(before[0].feature(), Some("w0000002"));
}

// ---------------------------------------------------------------------------------------
// Moves through the bracket's chain, worked out by hand.

#[test]
fn a_fillet_moved_before_its_base_swaps_them_in_the_chain() {
    // f0000001 (on h0000002, naming p0000001's edge) before h0000002: the chain left is
    // p0000001, h0000001, h0000002; before h0000002's place the last is h0000001, so the
    // fillet takes h0000001, and h0000002 takes the fillet. Its names' maker, p0000001, is
    // still what its base is built from.
    let f = opened(&bracket());
    f.begin();
    let moved = f.moved("f0000001", Some("h0000002")).result.unwrap();
    assert_eq!((moved.from, moved.to), (6, 5));
    assert_eq!(
        moved.rewired,
        [
            rewired("f0000001", "base", Some("h0000002"), Some("h0000001")),
            rewired("h0000002", "base", Some("h0000001"), Some("f0000001")),
        ]
    );
    assert_eq!(
        f.ids(),
        [
            "s0000001", "p0000001", "s0000002", "h0000001", "s0000003", "f0000001", "h0000002"
        ]
    );
    let timeline = answer(f.sessions.timeline(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    for entry in timeline.features.unwrap() {
        assert_eq!(entry.problems, 0, "{entry:?}");
    }
    assert_eq!(timeline.bodies, [id("h0000002")]);
}

#[test]
fn a_sketch_moved_to_the_start_rewires_nothing_and_one_after_its_user_is_refused() {
    let f = opened(&bracket());
    f.begin();
    let moved = f.moved("s0000003", Some("s0000001")).result.unwrap();
    assert_eq!((moved.from, moved.to, moved.changed), (4, 0, true));
    assert!(moved.rewired.is_empty());
    let refused = errors(&f.moved("s0000003", None));
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(
        refused[0].message(),
        "s0000003 would come after h0000002, which uses s0000003 as its sketch"
    );
    assert_eq!(refused[0].feature(), Some("s0000003"));
    assert_eq!(refused[0].parameter(), Some("before"));
}

#[test]
fn a_pad_root_moved_past_the_pad_on_it_reverses_them() {
    // p0000001 (no base) and p0000002 on it: p0000001 to the end. p0000002 begins the body
    // (base null), and p0000001 takes it as its base.
    let second = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "More", "base": "p0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let f = opened(&text_of(&[S0000001, P0000001, second]));
    f.begin();
    let moved = f.moved("p0000001", None).result.unwrap();
    assert_eq!((moved.from, moved.to), (1, 2));
    assert_eq!(
        moved.rewired,
        [
            rewired("p0000002", "base", Some("p0000001"), None),
            rewired("p0000001", "base", None, Some("p0000002")),
        ]
    );
    // And back: p0000002 to the end restores the first chain.
    let back = f.moved("p0000002", None).result.unwrap();
    assert_eq!(
        back.rewired,
        [
            rewired("p0000001", "base", Some("p0000002"), None),
            rewired("p0000002", "base", None, Some("p0000001")),
        ]
    );
    assert_eq!(
        f.text(),
        text_of(&[S0000001, P0000001, second]),
        "a move and its inverse"
    );
}

#[test]
fn a_maker_moved_after_a_feature_already_naming_it_out_of_lineage_is_refused_by_the_gate() {
    // r0000001 fillets b0000002 but names a0000001's edge (outside its lineage: broken
    // already, the rebuild's to say). Moving a0000001 after r0000001 isn't the lineage
    // rule's, so the gate must refuse it: r0000001 would name a feature after it.
    let f = opened(&text_of(&[
        r#"{"id": "a0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        r#"{"id": "b0000002", "type": "box", "v": 1, "name": "B", "position": [50, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        r#"{"id": "r0000001", "type": "fillet", "v": 1, "name": "R", "base": "b0000002", "edges": ["a0000001.box.Edge1"], "radius": 1}"#,
    ]));
    f.begin();
    let start = f.text();
    let errors = errors(&f.moved("a0000001", None));
    assert!(!errors.is_empty());
    assert!(
        errors.iter().all(|e| e.code() == ErrorCode::InvalidInput),
        "{errors:?}"
    );
    assert_eq!(f.text(), start);
}

// ---------------------------------------------------------------------------------------
// Races, determinism, and scale.

#[test]
fn two_moves_raced_at_one_revision_one_goes_ahead_and_the_other_conflicts() {
    let f = opened(&bracket());
    f.begin();
    let at = f.revision();
    let a = f
        .sessions
        .move_feature(&f.id, at, id("h0000001"), Some(id("h0000002")));
    let b = f
        .sessions
        .move_feature(&f.id, at, id("s0000003"), Some(id("s0000001")));
    let (a, b) = (answer(a), answer(b));
    let ok = [a.result.is_ok(), b.result.is_ok()];
    assert_eq!(ok.iter().filter(|x| **x).count(), 1, "{ok:?}");
    let refused = if a.result.is_err() { &a } else { &b };
    assert_eq!(errors(refused)[0].code(), ErrorCode::Conflict);
    assert_eq!(f.revision(), at.next().unwrap());
}

#[test]
fn the_same_moves_in_two_sessions_give_the_same_bytes() {
    let run = || {
        let f = opened(&fingerprinted());
        f.moved("h0000001", None).result.unwrap();
        f.moved("f0000001", Some("h0000002")).result.unwrap();
        f.add(
            Some("s0000003"),
            &AGAIN
                .replace("h0000002", "h0000001")
                .replace("\"Again\"", "\"Mid\""),
        )
        .result
        .ok();
        f.file()
    };
    assert_eq!(run(), run());
}

/// A box, then `n` boxes each fused onto the chain as its tool: 2n + 1 features.
fn long_chain(n: usize) -> String {
    let mut features = vec![
        r#"{"id": "b0000000", "type": "box", "v": 1, "name": "Root", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
    ];
    let mut base = "b0000000".to_owned();
    for i in 1..=n {
        let tool = format!("t{i:07x}");
        let fuse = format!("j{i:07x}");
        features.push(format!(
            r#"{{"id": "{tool}", "type": "box", "v": 1, "name": "T", "position": [{i}, 0, 0], "length": 1, "width": 1, "height": 1}}"#
        ));
        features.push(format!(
            r#"{{"id": "{fuse}", "type": "fuse", "v": 1, "name": "J", "base": "{base}", "tool": "{tool}"}}"#
        ));
        base = fuse;
    }
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    text_of(&refs)
}

#[test]
fn a_long_chains_first_fuse_moved_to_the_end_answers_in_bounded_time() {
    // 4001 features. j0000001 (on the root, with t0000001) to the end: j0000002 takes the
    // root, and j0000001 builds on the last fuse. Refused or not, a few seconds at most.
    let n = 2000;
    let f = opened(&long_chain(n));
    f.begin();
    let started = Instant::now();
    let moved = f.moved("j0000001", None);
    let took = started.elapsed();
    assert!(took < Duration::from_secs(10), "{took:?}");
    let moved = moved.result.unwrap();
    let last = format!("j{n:07x}");
    assert_eq!(
        moved.rewired,
        [
            rewired("j0000002", "base", Some("j0000001"), Some("b0000000")),
            rewired("j0000001", "base", Some("b0000000"), Some(&last)),
        ]
    );
    // And a refused one, the root past its consumer, as fast.
    let started = Instant::now();
    let refused = f.moved("b0000000", None);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(refused.result.is_err());
}

// ---------------------------------------------------------------------------------------
// The timeline read around the edits.

#[test]
fn the_timeline_inside_a_transaction_lists_the_moved_order_and_its_inputs() {
    let f = opened(&tool_chain());
    f.begin();
    f.moved("c0000003", Some("c0000002")).result.unwrap();
    let timeline = answer(f.sessions.timeline(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    let features = timeline.features.unwrap();
    let listed: Vec<(String, Vec<String>)> = features
        .iter()
        .map(|e| {
            (
                e.id.to_string(),
                e.inputs.iter().map(ToString::to_string).collect(),
            )
        })
        .collect();
    let own = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        listed,
        [
            ("b0000001".to_owned(), own(&[])),
            ("p0000002".to_owned(), own(&[])),
            ("c0000003".to_owned(), own(&["p0000002"])),
            ("c0000002".to_owned(), own(&["c0000003"])),
            ("w0000001".to_owned(), own(&["b0000001", "c0000002"])),
        ]
    );
    assert_eq!(timeline.bodies, [id("w0000001")]);
    assert_eq!(timeline.revision, f.revision());
}

#[test]
fn a_timeline_of_the_published_model_after_a_move_names_the_older_revision() {
    // Published before the move (revision 1 built), the move makes revision 2 inside a
    // transaction, which starts no rebuild: the published timeline says the model is 1's.
    let f = opened(&bracket());
    answer(f.sessions.timeline(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    f.begin();
    f.moved("h0000001", None).result.unwrap();
    let timeline = answer(f.sessions.timeline(&f.id, ModelChoice::Published))
        .result
        .unwrap();
    assert_eq!(timeline.revision, f.revision());
    assert_eq!(timeline.model, Some(Revision::OPENED));
    // Listed in the document's new order, h0000001 last.
    let ids: Vec<String> = timeline
        .features
        .unwrap()
        .iter()
        .map(|e| e.id.to_string())
        .collect();
    assert_eq!(ids.last().map(String::as_str), Some("h0000001"));
}

// ---------------------------------------------------------------------------------------
// Random runs of timeline edits through the session: every run committed edit by edit
// reopens with no drift (each rewired chain's fingerprints written anew), and the same run
// inside a transaction is undone, edit by edit, to the text it began with, byte for byte.

mod runs {
    use super::*;

    use proptest::prelude::*;
    use std::sync::OnceLock;

    fn config(cases: u32) -> ProptestConfig {
        ProptestConfig {
            cases,
            failure_persistence: None,
            rng_seed: proptest::test_runner::RngSeed::Fixed(0x7173_e295),
            ..ProptestConfig::default()
        }
    }

    /// The bracket and the tool chain, each saved with the fake kernel's fingerprints.
    fn starts() -> &'static [String; 2] {
        static STARTS: OnceLock<[String; 2]> = OnceLock::new();
        STARTS.get_or_init(|| {
            let tools = opened(&tool_chain());
            assert!(
                answer(tools.sessions.format(&tools.id, None, true))
                    .result
                    .unwrap()
                    .written
            );
            [fingerprinted(), tools.file()]
        })
    }

    #[derive(Debug, Clone, Copy)]
    enum Edit {
        Move(u8, u8),
        Remove(u8),
        /// A pocket (the bracket) or a chamfer (the tool chain) on a solid, before a place.
        Insert(u8, u8),
    }

    fn edit() -> impl Strategy<Value = Edit> {
        prop_oneof![
            4 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| Edit::Move(a, b)),
            1 => any::<u8>().prop_map(Edit::Remove),
            2 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| Edit::Insert(a, b)),
        ]
    }

    /// `edit` applied to the session's document as it stands; whether it was taken and
    /// changed the document (a move to where a feature is changes nothing).
    fn apply(f: &Fixture, edit: Edit, n: usize) -> bool {
        attempt(f, edit, n).is_some()
    }

    /// `edit` applied as [`apply`] applies it: what it said it rewired, if it was taken and
    /// changed the document.
    fn attempt(f: &Fixture, edit: Edit, n: usize) -> Option<Vec<Rewired>> {
        let doc = document::read(f.text().as_bytes()).unwrap();
        let ids: Vec<String> = doc.features.iter().map(|x| x.id.to_string()).collect();
        let place = |b: u8| -> Option<String> { ids.get(b as usize % (ids.len() + 1)).cloned() };
        match edit {
            Edit::Move(a, b) => {
                let feature = &ids[a as usize % ids.len()];
                let before = place(b);
                f.moved(feature, before.as_deref())
                    .result
                    .ok()
                    .filter(|m| m.changed)
                    .map(|m| m.rewired)
            }
            Edit::Remove(a) => f
                .remove(&ids[a as usize % ids.len()])
                .result
                .ok()
                .map(|r| r.rewired),
            Edit::Insert(a, b) => {
                let solids: Vec<&str> = doc
                    .features
                    .iter()
                    .filter(|x| x.operation.makes_solid())
                    .map(|x| x.id.as_str())
                    .collect();
                let base = solids.get(a as usize % solids.len().max(1))?;
                // A chamfer of nothing named would be refused by the gate: a pocket needs a
                // sketch, which only the bracket has; the tool chain takes a box fused on.
                let new = format!("e{n:07}");
                let feature = if ids.iter().any(|i| i == "s0000003") {
                    format!(
                        r#"{{"id": "{new}", "type": "pocket", "v": 1, "name": "New", "base": "{base}", "sketch": "s0000002", "extent": {{"type": "through_all", "reversed": true}}}}"#
                    )
                } else {
                    format!(
                        r#"{{"id": "{new}", "type": "chamfer", "v": 1, "name": "New", "base": "{base}", "edges": ["p0000002.box.Edge7"], "size": {{"distance": 0.5}}}}"#
                    )
                };
                f.add(place(b).as_deref(), &feature)
                    .result
                    .ok()
                    .map(|a| a.rewired)
            }
        }
    }

    /// Each feature of `text`'s `base` and `tool`, by ID, in the document's order, read by
    /// serde_json, not Silico's reader.
    fn inputs(text: &str) -> Vec<(String, [Option<String>; 2])> {
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        v["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let field = |k: &str| f.get(k).and_then(|x| x.as_str()).map(str::to_owned);
                (
                    f["id"].as_str().unwrap().to_owned(),
                    [field("base"), field("tool")],
                )
            })
            .collect()
    }

    type Change = (String, &'static str, Option<String>, Option<String>);

    /// Each `base` and `tool` that differs between `before` and `after`, of the features
    /// both hold, in `after`'s order: what `rewired` must list, worked out from the texts.
    fn changed(before: &str, after: &str) -> Vec<Change> {
        let old: std::collections::BTreeMap<String, [Option<String>; 2]> =
            inputs(before).into_iter().collect();
        let mut out = Vec::new();
        for (id, now) in inputs(after) {
            let Some(was) = old.get(&id) else { continue };
            for (k, field) in ["base", "tool"].into_iter().enumerate() {
                if was[k] != now[k] {
                    out.push((id.clone(), field, was[k].clone(), now[k].clone()));
                }
            }
        }
        out
    }

    fn said(rewired: &[Rewired]) -> Vec<Change> {
        rewired
            .iter()
            .map(|r| {
                (
                    r.feature.to_string(),
                    r.field,
                    r.from.as_ref().map(ToString::to_string),
                    r.to.as_ref().map(ToString::to_string),
                )
            })
            .collect()
    }

    fn drift(file: &str) -> Vec<String> {
        reopened_problems(file)
            .into_iter()
            .filter(|(code, _, _)| *code == ErrorCode::Drift)
            .map(|(_, feature, message)| format!("{feature:?}: {message}"))
            .collect()
    }

    proptest! {
        #![proptest_config(config(250))]

        #[test]
        fn every_run_committed_edit_by_edit_reopens_without_drift(
            which in 0usize..2,
            edits in proptest::collection::vec(edit(), 1..5),
        ) {
            let f = opened(&starts()[which]);
            let mut taken = 0;
            for (n, edit) in edits.iter().enumerate() {
                if apply(&f, *edit, n) {
                    taken += 1;
                }
            }
            let drifted = drift(&f.file());
            prop_assert!(drifted.is_empty(), "{} taken of {:?}: {:?}\n{}", taken, edits, drifted, f.file());
        }

        #[test]
        fn every_run_inside_a_transaction_undoes_to_its_start_and_commits_without_drift(
            which in 0usize..2,
            edits in proptest::collection::vec(edit(), 1..5),
        ) {
            let f = opened(&starts()[which]);
            f.begin();
            let start = f.text();
            let mut texts = vec![start.clone()];
            for (n, edit) in edits.iter().enumerate() {
                let at = f.revision();
                if apply(&f, *edit, n) {
                    texts.push(f.text());
                } else {
                    prop_assert_eq!(f.revision(), at, "a refused or empty edit moved the revision");
                    prop_assert_eq!(texts.last().unwrap(), &f.text(), "a refused or empty edit changed the text");
                }
            }
            let end = f.text();
            // Undone edit by edit, each state as it was.
            for want in texts.iter().rev().skip(1) {
                answer(f.sessions.undo(&f.id, f.revision(), None)).result.unwrap();
                prop_assert_eq!(&f.text(), want);
            }
            // Redone to the end, then committed.
            for want in texts.iter().skip(1) {
                answer(f.sessions.redo(&f.id, f.revision(), None)).result.unwrap();
                prop_assert_eq!(&f.text(), want);
            }
            prop_assert_eq!(&f.text(), &end);
            answer(f.sessions.commit(&f.id)).result.unwrap();
            let drifted = drift(&f.file());
            prop_assert!(drifted.is_empty(), "{:?}: {:?}\n{}", edits, drifted, f.file());
        }

        #[test]
        fn every_taken_edits_rewired_is_exactly_what_changed_and_the_gate_takes_its_text(
            which in 0usize..2,
            inside in any::<bool>(),
            edits in proptest::collection::vec(edit(), 1..5),
        ) {
            let f = opened(&starts()[which]);
            if inside {
                f.begin();
            }
            for (n, edit) in edits.iter().enumerate() {
                let before = f.text();
                if let Some(rewired) = attempt(&f, *edit, n) {
                    let after = f.text();
                    prop_assert_eq!(
                        said(&rewired),
                        changed(&before, &after),
                        "{:?} of {:?}\n{}\n{}", edit, edits, before, after
                    );
                    let read = document::read(after.as_bytes());
                    prop_assert!(read.is_ok(), "{:?}: the text isn't read", edit);
                    let gated = document::validate(&read.unwrap());
                    prop_assert!(gated.is_ok(), "{:?}: the gate refuses the result: {:?}\n{}", edit, gated.err(), after);
                }
            }
        }

        #[test]
        fn a_move_and_its_way_back_restore_the_text_and_the_file_byte_for_byte(
            which in 0usize..2,
            inside in any::<bool>(),
            a in any::<u8>(),
            b in any::<u8>(),
        ) {
            let f = opened(&starts()[which]);
            let file = f.file();
            if inside {
                f.begin();
            }
            let start = f.text();
            let ids = f.ids();
            let at = a as usize % ids.len();
            let feature = ids[at].clone();
            let after = ids.get(at + 1).cloned();
            let to = ids.get(b as usize % (ids.len() + 1)).cloned();
            let Ok(there) = f.moved(&feature, to.as_deref()).result else {
                return Ok(());
            };
            if !there.changed {
                return Ok(());
            }
            let mut back = f.moved(&feature, after.as_deref());
            // A move never makes a feature another's tool (section 7, step 3), so a member
            // of a tool's chain moved off its end can't be moved back onto it; when it
            // swapped with its neighbour, the neighbour moved before it is the way back.
            if back.result.as_ref().is_err_and(|e| {
                e.errors()[0].message().contains("a move never makes a feature another's tool")
            }) && there.to + 1 == at
            {
                let neighbour = f.ids()[at].clone();
                back = f.moved(&neighbour, Some(&feature));
            }
            prop_assert!(
                back.result.is_ok(),
                "{} moved before {:?} can't go back before {:?}: {:?}",
                feature, to, after, back.result.as_ref().err()
            );
            let back = back.result.unwrap();
            prop_assert_eq!(&f.text(), &start, "{} before {:?} and back", feature, to);
            // The way back undoes each rewiring, and nothing more.
            let mut undone: Vec<Change> = said(&there.rewired)
                .into_iter()
                .map(|(i, field, from, to)| (i, field, to, from))
                .collect();
            let mut redone = said(&back.rewired);
            undone.sort();
            redone.sort();
            prop_assert_eq!(redone, undone);
            if inside {
                answer(f.sessions.commit(&f.id)).result.unwrap();
            }
            prop_assert!(f.file() == file, "{} before {:?} and back, saved:\n{}\nnot\n{}", feature, to, f.file(), file);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Hostile requests on the wire: each refused naming its field, the rest of the request's
// problems with it.

mod wire {
    use super::*;

    use silico_core::command::wire::edits::{AddFeatureRequest, MoveFeatureRequest};
    use silico_core::command::wire::reads::TimelineRequest;
    use silico_core::document::json::Node;

    fn node(text: &str) -> Result<Node, String> {
        read(text.as_bytes()).map_err(|e| format!("{e:?}"))
    }

    /// Each refusal's parameter, in order.
    fn parameters(problems: &silico_core::command::Problems) -> Vec<String> {
        problems
            .errors()
            .iter()
            .map(|e| e.parameter().unwrap_or("").to_owned())
            .collect()
    }

    #[test]
    fn a_before_given_twice_is_refused_by_the_reader_or_the_request() {
        // RFC 8259 leaves a repeated name's meaning open; a strict reader refuses it. Either
        // the text isn't read, or the request names `before`: never one of the two taken.
        let text = r#"{"session": "s1", "revision": 1, "feature": "h0000001", "before": null, "before": "f0000001"}"#;
        match node(text) {
            Err(_) => {}
            Ok(n) => {
                let refused = MoveFeatureRequest::read(&n);
                assert!(refused.is_err(), "taken: {refused:?}");
            }
        }
    }

    #[test]
    fn before_values_no_feature_has_are_refused_naming_before() {
        let move_of = |before: &str| {
            format!(
                r#"{{"session": "s1", "revision": 1, "feature": "h0000001", "before": {before}}}"#
            )
        };
        let add_of = |before: &str| {
            format!(
                r#"{{"session": "s1", "revision": 1, "before": {before}, "feature": {{"type": "box", "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}}}"#
            )
        };
        let long = format!("\"{}\"", "a".repeat(100_000));
        for before in [
            r#""""#,
            r#"" h0000001""#,
            r#""h0000001 ""#,
            r#""h000000１""#,
            r#""h00000001""#,
            r#""h000000""#,
            r#"{"id": "h0000001"}"#,
            "true",
            "1e400",
            long.as_str(),
        ] {
            let refused = MoveFeatureRequest::read(&node(&move_of(before)).unwrap()).unwrap_err();
            assert_eq!(parameters(&refused), ["before"], "move: {before:.40}");
            let refused = AddFeatureRequest::read(&node(&add_of(before)).unwrap()).unwrap_err();
            assert_eq!(parameters(&refused), ["before"], "add: {before:.40}");
            // The refusal doesn't echo a huge value whole.
            for e in refused.errors() {
                assert!(e.message().len() < 1_000, "{}", e.message().len());
            }
        }
    }

    #[test]
    fn every_bad_field_of_a_move_is_refused_at_once() {
        let text = r#"{"session": "S1", "revision": 0, "feature": null, "before": 7}"#;
        let refused = MoveFeatureRequest::read(&node(text).unwrap()).unwrap_err();
        assert_eq!(
            parameters(&refused),
            ["session", "revision", "feature", "before"]
        );
    }

    #[test]
    fn a_timeline_of_a_model_by_another_case_or_a_missing_session_is_refused() {
        for (text, field) in [
            (r#"{"session": "s1", "model": "Current"}"#, "model"),
            (r#"{"session": "s1", "model": null}"#, "model"),
            (r#"{"session": "", "model": "current"}"#, "session"),
            (
                r#"{"session": "s1", "model": "current", "fresh": false}"#,
                "fresh",
            ),
        ] {
            let refused = TimelineRequest::read(&node(text).unwrap()).unwrap_err();
            assert_eq!(parameters(&refused), [field], "{text}");
        }
    }

    #[test]
    fn a_move_and_a_timeline_of_a_session_never_opened_or_closed_say_so() {
        let f = opened(&bracket());
        let other = SessionId::parse("s9").unwrap();
        let moved = answer(
            f.sessions
                .move_feature(&other, Revision::OPENED, id("h0000001"), None),
        );
        let errors = errors(&moved);
        assert_eq!(errors[0].parameter(), Some("session"), "{errors:?}");
        answer(f.sessions.close(&f.id)).result.unwrap();
        let timeline = answer(f.sessions.timeline(&f.id, ModelChoice::Current));
        let errors = super::errors(&timeline);
        assert_eq!(errors[0].parameter(), Some("session"), "{errors:?}");
    }

    #[test]
    fn a_move_of_a_document_the_gate_refuses_is_refused_and_says_replace_text_mends_it() {
        // Even one to where the feature is: only replace_text mends a refused document.
        let flat = bracket().replace(r#""length": 40"#, r#""length": 0"#);
        let f = opened(&flat);
        for before in [None, Some("h0000002")] {
            let refused = errors(&f.moved("h0000001", before));
            assert_eq!(refused.len(), 1, "{refused:?}");
            assert!(
                refused[0]
                    .suggestion()
                    .is_some_and(|s| s.contains("replace_text")),
                "{refused:?}"
            );
        }
        let refused = errors(&f.add(Some("f0000001"), AGAIN));
        assert!(
            refused[0]
                .suggestion()
                .is_some_and(|s| s.contains("replace_text")),
            "{refused:?}"
        );
        assert_eq!(f.revision(), Revision::OPENED);
    }
}

// ---------------------------------------------------------------------------------------
// The undo stack holds a state as chunks of 64 features' texts, shared between states while
// none of a chunk's features changes. An insert, a move, or a removal near the top shifts
// every feature after it across the chunks' bounds: each state must still be restored byte
// for byte.

/// An edit of a fixture's session, and whether it was taken.
type Step = Box<dyn Fn(&Fixture) -> bool>;

#[test]
fn shifting_edits_of_a_long_document_are_undone_and_redone_byte_for_byte() {
    let f = opened(&long_chain(150));
    f.begin();
    let mut texts = vec![f.text()];
    let front = r#"{"id": "a0000001", "type": "box", "v": 1, "name": "Front", "position": [0, 9, 0], "length": 1, "width": 1, "height": 1}"#;
    let edits: Vec<Step> = vec![
        Box::new(|f| f.add(Some("b0000000"), front).result.is_ok()),
        Box::new(|f| f.moved("j0000001", None).result.is_ok()),
        Box::new(|f| f.remove("j0000032").result.is_ok()),
        Box::new(|f| f.moved("t0000050", Some("a0000001")).result.is_ok()),
        Box::new(|f| f.moved("a0000001", None).result.is_ok()),
        Box::new(|f| f.moved("t0000090", Some("t0000005")).result.is_ok()),
    ];
    for (n, edit) in edits.iter().enumerate() {
        assert!(edit(&f), "edit {n}");
        let text = f.text();
        assert_ne!(Some(&text), texts.last(), "edit {n} changed nothing");
        texts.push(text);
    }
    for (n, want) in texts.iter().enumerate().rev().skip(1) {
        answer(f.sessions.undo(&f.id, f.revision(), None))
            .result
            .unwrap();
        assert!(f.text() == *want, "undone to state {n}: not byte for byte");
    }
    for (n, want) in texts.iter().enumerate().skip(1) {
        answer(f.sessions.redo(&f.id, f.revision(), None))
            .result
            .unwrap();
        assert!(f.text() == *want, "redone to state {n}: not byte for byte");
    }
    // Undone again to the start, then committed: the file as it was, byte for byte.
    let start = texts[0].clone();
    for _ in 0..edits.len() {
        answer(f.sessions.undo(&f.id, f.revision(), None))
            .result
            .unwrap();
    }
    assert!(f.text() == start);
    answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(f.file() == long_chain(150));
}

#[test]
fn a_published_timeline_after_a_rewiring_move_marks_each_rewired_feature_and_its_users_unknown() {
    // Revision 1 built and published; inside a transaction, h0000001 moved to the end:
    // h0000002 (rebased on p0000001), f0000001 (on h0000002), and h0000001 (on f0000001)
    // are each built otherwise now, so revision 1's model can't say how they are: unknown.
    // The sketches and the pad, untouched, keep the marks revision 1 gave them.
    use silico_core::timeline::Health;
    let f = opened(&fingerprinted());
    let before = answer(f.sessions.timeline(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    let marks = |t: &silico_core::command::TimelineAnswer| -> Vec<(String, Health)> {
        t.features
            .as_ref()
            .unwrap()
            .iter()
            .map(|e| (e.id.to_string(), e.health))
            .collect()
    };
    assert!(
        marks(&before).iter().all(|(_, h)| *h == Health::Healthy),
        "{:?}",
        marks(&before)
    );
    f.begin();
    f.moved("h0000001", None).result.unwrap();
    let after = answer(f.sessions.timeline(&f.id, ModelChoice::Published))
        .result
        .unwrap();
    assert_eq!(after.model, Some(Revision::OPENED));
    let own = |pairs: &[(&str, Health)]| -> Vec<(String, Health)> {
        pairs.iter().map(|(i, h)| ((*i).to_owned(), *h)).collect()
    };
    assert_eq!(
        marks(&after),
        own(&[
            ("s0000001", Health::Healthy),
            ("p0000001", Health::Healthy),
            ("s0000002", Health::Healthy),
            ("s0000003", Health::Healthy),
            ("h0000002", Health::Unknown),
            ("f0000001", Health::Unknown),
            ("h0000001", Health::Unknown),
        ])
    );
    // A move that rewires nothing leaves every mark as it was.
    answer(f.sessions.rollback(&f.id)).result.unwrap();
    f.begin();
    f.moved("h0000001", Some("h0000002")).result.unwrap();
    let after = answer(f.sessions.timeline(&f.id, ModelChoice::Published))
        .result
        .unwrap();
    assert!(
        marks(&after).iter().all(|(_, h)| *h == Health::Healthy),
        "{:?}",
        marks(&after)
    );
}

// ---------------------------------------------------------------------------------------
// `call` by name, as MCP and the CLI's `serve` reach it, against the typed methods, on the
// refusals and the edge cases as well as the happy path: each answer the same envelope,
// byte for byte, on two sessions opened on the same file, so each step leaves both alike.

/// A step: the command's name, its arguments given the revision the session stands at, and
/// the typed method's envelope, on one line, at that revision.
type Pair = (
    &'static str,
    Box<dyn Fn(u64) -> String>,
    Box<dyn Fn(&Fixture, Revision) -> (bool, String)>,
);

fn written<T: silico_core::command::wire::ToWire>(response: &Response<T>) -> (bool, String) {
    use silico_core::command::wire::{Layout, response_text};
    (
        response.result.is_ok(),
        response_text(response, Layout::OneLine),
    )
}

#[test]
fn the_timeline_commands_refusals_and_edge_cases_by_name_are_the_typed_methods_byte_for_byte() {
    use silico_core::command::Call;
    let start = fingerprinted();
    let by_name = opened(&start);
    let typed = opened(&start);
    assert_eq!(by_name.id, typed.id);
    let s = by_name.id.to_string();
    let pocket = AGAIN;
    let mv = |feature: &'static str, before: Option<&'static str>| -> Pair {
        let s = s.clone();
        let quoted = before.map_or("null".to_owned(), |b| format!("\"{b}\""));
        (
            "move_feature",
            Box::new(move |r| {
                format!(
                    r#"{{"session": "{s}", "revision": {r}, "feature": "{feature}", "before": {quoted}}}"#
                )
            }),
            Box::new(move |f, r| {
                written(&answer(f.sessions.move_feature(
                    &f.id,
                    r,
                    id(feature),
                    before.map(id),
                )))
            }),
        )
    };
    let rm = |feature: &'static str| -> Pair {
        let s = s.clone();
        (
            "remove_feature",
            Box::new(move |r| {
                format!(r#"{{"session": "{s}", "revision": {r}, "feature": "{feature}"}}"#)
            }),
            Box::new(move |f, r| {
                written(&answer(f.sessions.remove_feature(&f.id, r, id(feature))))
            }),
        )
    };
    let add = |before: Option<&'static str>, feature: &'static str| -> Pair {
        let s = s.clone();
        let quoted = before.map_or("null".to_owned(), |b| format!("\"{b}\""));
        (
            "add_feature",
            Box::new(move |r| {
                format!(
                    r#"{{"session": "{s}", "revision": {r}, "before": {quoted}, "feature": {feature}}}"#
                )
            }),
            Box::new(move |f, r| {
                written(&answer(f.sessions.add_feature(
                    &f.id,
                    r,
                    before.map(id),
                    read(feature.as_bytes()).unwrap(),
                )))
            }),
        )
    };
    let tl = |model: &'static str, choice: ModelChoice| -> Pair {
        let s = s.clone();
        (
            "timeline",
            Box::new(move |_| format!(r#"{{"session": "{s}", "model": "{model}"}}"#)),
            Box::new(move |f, _| written(&answer(f.sessions.timeline(&f.id, choice)))),
        )
    };
    let stale = {
        let s = s.clone();
        let p: Pair = (
            "move_feature",
            Box::new(move |r| {
                format!(
                    r#"{{"session": "{s}", "revision": {}, "feature": "h0000001", "before": null}}"#,
                    r + 5
                )
            }),
            Box::new(move |f, r| {
                written(&answer(f.sessions.move_feature(
                    &f.id,
                    Revision::from_number(r.number() + 5).unwrap(),
                    id("h0000001"),
                    None,
                )))
            }),
        );
        p
    };
    let steps: Vec<Pair> = vec![
        mv("h0000001", Some("h0000001")),
        mv("s0000002", None),
        mv("h0000001", Some("z0000001")),
        mv("z0000001", None),
        mv("f0000001", Some("s0000001")),
        mv("p0000001", None),
        stale,
        rm("p0000001"),
        rm("s0000003"),
        add(Some("s0000001"), pocket),
        add(Some("f0000001"), pocket),
        tl("current", ModelChoice::Current),
        tl("published", ModelChoice::Published),
        mv("h0000001", None),
        tl("current", ModelChoice::Current),
        tl("published", ModelChoice::Published),
        rm("h0000009"),
        mv("h0000001", Some("h0000002")),
        tl("current", ModelChoice::Current),
    ];
    let mut ran = Vec::new();
    for (n, (name, arguments, method)) in steps.iter().enumerate() {
        let at = typed.revision();
        assert_eq!(by_name.revision(), at, "step {n}");
        let text = arguments(at.number());
        let want = method(&typed, at);
        let got = match by_name.sessions.call(name, &text) {
            Call::Running(calling) => match calling.wait_timeout(HANG) {
                Ok(answered) => (answered.ok, answered.text),
                Err(_) => panic!("{name} didn't answer"),
            },
            Call::Unknown => panic!("{name} unknown"),
        };
        // The session's path, each fixture's own directory, aside.
        let at_path = |f: &Fixture, (ok, text): (bool, String)| {
            (ok, text.replace(&*f.path().to_string_lossy(), "PATH"))
        };
        assert_eq!(
            at_path(&by_name, got),
            at_path(&typed, want.clone()),
            "step {n}: {name} {text}"
        );
        ran.push(want.0);
    }
    // The file each saved, the same.
    assert!(by_name.file() == typed.file());
    // Which ran, by hand: not a move before itself, which names no place; the pocket
    // inserted under the fillet (and not before the first sketch); the timelines; h0000001
    // to the end; the pocket removed; h0000001 back.
    assert_eq!(
        ran,
        [
            false, false, false, false, false, false, false, false, false, false, true, true, true,
            true, true, true, true, true, true
        ]
    );
}
