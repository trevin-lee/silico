//! Hostile inputs for a transaction's `undo` and `redo` (`Sessions::undo`, `Sessions::redo`,
//! break-it, PR #273): random runs of edits, undos and redos (to a revision or not, stale or
//! not) checked step by step against a model of the stack written here; a transaction begun
//! from text that didn't read, mended, undone back to it, and mended otherwise; undos raced
//! from many threads against one revision; edits that change nothing, refused edits and
//! refused steps between an undo and its redo; every kind of edit undone and redone; `to`
//! at 0, past the session, and from a transaction before; and sessions
//! unknown or closed. Expected values: the texts written out by hand, the box's
//! fingerprint by formula (L × 20 × 10: volume 200 L, area 60 L + 400), the design's rule
//! that an edit undone reaches nothing, and the stack's rules as the model here has them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{Response, Revision, SessionId, Sessions, Stepped};
use silico_core::document::json::Node;
use silico_core::fake::FakeKernel;
use tempfile::TempDir;

use crate::sessions::{answer, id};

const HEAD: &str =
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": ["#;

/// The block's feature on one line, `length` long, its fingerprint stored for a length of 40.
fn block_line(length: u32) -> String {
    format!(
        r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": {length}, "width": 20, "height": 10, "fingerprint": {{"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}}}"#
    )
}

/// A block 40 × 20 × 10, its fingerprint stored, on one line: not canonical form.
fn block() -> String {
    format!("{HEAD}{}]}}", block_line(40))
}

/// The block's fingerprint member for a length of `length`, by formula.
fn fingerprint(length: u32) -> String {
    format!(
        r#""fingerprint": {{"volume": {}, "area": {}, "bbox": [0, 0, 0, {length}, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}"#,
        200 * length,
        60 * length + 400
    )
}

/// The block `length` long in canonical form, with the fingerprint member `fingerprint`:
/// written by hand from M2.1's layout.
fn canonical(length: u32, fingerprint: &str) -> String {
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n    {{\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": {length},\n      \"width\": 20,\n      \"height\": 10,\n      {fingerprint}\n    }}\n  ]\n}}\n"
    )
}

fn number(text: &str) -> Node {
    silico_core::document::json::read(text.as_bytes()).unwrap()
}

fn rev(n: u64) -> Revision {
    Revision::from_number(n).unwrap()
}

struct Fixture {
    _dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
    path: PathBuf,
}

fn sessions(dir: &Path) -> Sessions<Arc<FakeKernel>> {
    let secret = dir.join("secret");
    Sessions::with_cache_opener(Arc::new(FakeKernel::new(2).unwrap()), move |cache| {
        Cache::with_secret(cache, &secret)
    })
}

fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(dir.path());
    let path = dir.path().join("a.silico");
    std::fs::write(&path, text).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    Fixture {
        _dir: dir,
        sessions,
        id,
        path,
    }
}

impl Fixture {
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

    fn file(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }

    fn begin(&self) {
        answer(self.sessions.begin(&self.id)).result.unwrap();
    }

    fn set(&self, length: u32) -> Response<silico_core::command::FieldSet> {
        answer(self.sessions.set_field(
            &self.id,
            self.revision(),
            id("k7f2qx3m"),
            "length".to_owned(),
            number(&length.to_string()),
        ))
    }

    fn replace(&self, text: &str) -> bool {
        answer(
            self.sessions
                .replace_text(&self.id, self.revision(), text.to_owned()),
        )
        .result
        .unwrap()
        .changed
    }

    fn undo(&self, to: Option<u64>) -> Response<Stepped> {
        answer(self.sessions.undo(&self.id, self.revision(), to.map(rev)))
    }

    fn redo(&self, to: Option<u64>) -> Response<Stepped> {
        answer(self.sessions.redo(&self.id, self.revision(), to.map(rev)))
    }
}

/// The one error a refused step gives: its code, message, parameter, cause, and candidates.
fn refused(response: &Response<Stepped>) -> (ErrorCode, String, Option<String>, Option<String>) {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    let error = &problems.errors()[0];
    (
        error.code(),
        error.message().to_owned(),
        error.parameter().map(str::to_owned),
        error.cause().map(str::to_owned),
    )
}

// ---------------------------------------------------------------------------------------
// A transaction begun from text that didn't read: the mend an undo left behind.
// ---------------------------------------------------------------------------------------

/// The lug, 10 × 10 × 10 at x = 100, its fingerprint by formula (volume 1000, area 600),
/// `length` long, as one feature's text, the length on a line of its own.
fn lug(length: &str) -> String {
    format!(
        r#"{{"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0],
{length}
"width": 10, "height": 10, "fingerprint": {{"volume": 1000, "area": 600, "bbox": [100, 0, 0, 110, 10, 10], "faces": 6, "edges": 12, "vertices": 8}}}}"#
    )
}

/// The block and the lug, the lug's length a merge's conflict: text that doesn't read.
fn conflicted() -> String {
    format!(
        "{HEAD}{}, {}]}}",
        block_line(40),
        lug("<<<<<<< ours\n\"length\": 10,\n=======\n\"length\": 20,\n>>>>>>> theirs")
    )
}

/// The conflict mended with the lug `length` long (its fingerprint stays the one stored for
/// 10).
fn mended(length: u32) -> String {
    format!(
        "{HEAD}{}, {}]}}",
        block_line(40),
        lug(&format!("\"length\": {length},"))
    )
}

/// What a transaction begun from [`conflicted`] commits: the commit's refingerprinted
/// features and the file it leaves, made by `edits`.
fn commit_after(edits: impl FnOnce(&Fixture)) -> (Vec<String>, bool, String) {
    let f = opened(&conflicted());
    f.begin();
    edits(&f);
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    (
        committed
            .refingerprinted
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect(),
        committed.written,
        f.file(),
    )
}

#[test]
fn a_mend_undone_then_mended_otherwise_reaches_only_what_the_new_mend_alone_reaches() {
    // BREAKS: the transaction keeps the first state that read as its mend for good
    // (`Held::change` sets it only while it's none), so a mend undone back to the text that
    // didn't read still decides what the next, different mend reaches. Mended as "theirs"
    // (the lug 20 long) straight away, the commit reaches nothing: both IDs are in the text,
    // and nothing changed since the mend, so the lug's stored fingerprint (for 10) is kept
    // and its drift said. Mended as "ours" first, undone, then mended as "theirs", the lug
    // reads as changed since the undone mend: the commit writes the lug a new fingerprint,
    // accepting its drift silently. The same edits standing, after an edit undone, must
    // commit the same file (an edit undone reaches nothing).
    let direct = commit_after(|f| assert!(f.replace(&mended(20))));
    assert!(direct.0.is_empty(), "{:?}", direct.0);
    let through_an_undo = commit_after(|f| {
        assert!(f.replace(&mended(10)));
        f.undo(None).result.unwrap();
        assert_eq!(f.text(), conflicted());
        assert!(f.replace(&mended(20)));
    });
    assert_eq!(through_an_undo, direct);
}

#[test]
fn a_mend_undone_and_redone_reaches_what_it_did() {
    // The control: the mend undone and redone is the mend kept, and reaches as it did.
    let direct = commit_after(|f| assert!(f.replace(&mended(20))));
    let redone = commit_after(|f| {
        assert!(f.replace(&mended(20)));
        f.undo(None).result.unwrap();
        f.redo(None).result.unwrap();
    });
    assert_eq!(redone, direct);
}

// ---------------------------------------------------------------------------------------
// What a step leaves for the next command.
// ---------------------------------------------------------------------------------------

#[test]
fn every_edit_undone_commits_nothing_and_leaves_the_file_byte_for_byte() {
    let f = opened(&block());
    f.begin();
    for length in [41, 42, 43] {
        assert!(f.set(length).result.unwrap().changed);
    }
    let undone = f.undo(Some(1)).result.unwrap();
    assert_eq!((undone.restored, undone.undo, undone.redo), (rev(1), 0, 3));
    assert_eq!(f.text(), block());
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(!committed.written);
    assert!(committed.refingerprinted.is_empty());
    assert_eq!(f.file(), block());
    assert_eq!(f.text(), block());
}

#[test]
fn an_undo_then_a_redo_then_a_commit_writes_the_redone_state_with_its_own_fingerprint() {
    let f = opened(&block());
    f.begin();
    f.set(41).result.unwrap();
    f.set(42).result.unwrap();
    f.undo(Some(1)).result.unwrap();
    f.redo(Some(2)).result.unwrap();
    assert_eq!(f.text(), canonical(41, &fingerprint(40)));
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(committed.written);
    assert_eq!(committed.refingerprinted, [id("k7f2qx3m")]);
    assert_eq!(f.file(), canonical(41, &fingerprint(41)));
}

#[test]
fn an_edit_that_changes_nothing_after_an_undo_keeps_the_redo() {
    // No new revision and no new state: nothing to drop the redo for.
    let f = opened(&block());
    f.begin();
    f.set(41).result.unwrap();
    f.undo(None).result.unwrap();
    let at = f.revision();
    let same = f.set(40);
    assert!(!same.result.unwrap().changed);
    assert_eq!(f.revision(), at);
    // The same document laid out otherwise: no change either.
    assert!(!f.replace(&canonical(40, &fingerprint(40))));
    let redone = f.redo(None).result.unwrap();
    assert_eq!((redone.restored, redone.undo, redone.redo), (rev(2), 1, 0));
    assert_eq!(f.text(), canonical(41, &fingerprint(40)));
}

#[test]
fn a_refused_edit_after_an_undo_keeps_the_redo_and_the_state() {
    let f = opened(&block());
    f.begin();
    f.set(41).result.unwrap();
    f.undo(None).result.unwrap();
    let at = f.revision();
    // A length of -1, a field the box hasn't, a feature the document hasn't, a stale
    // revision, and text that doesn't read: each refused, the session as it was.
    let refused_set = |feature: &str, field: &str, value: &str, revision: Revision| {
        let response = answer(f.sessions.set_field(
            &f.id,
            revision,
            id(feature),
            field.to_owned(),
            number(value),
        ));
        assert!(response.result.is_err(), "{response:?}");
    };
    refused_set("k7f2qx3m", "length", "-1", at);
    refused_set("k7f2qx3m", "radius", "3", at);
    refused_set("z0000009", "length", "3", at);
    refused_set("k7f2qx3m", "length", "45", rev(1));
    let garbage = answer(
        f.sessions
            .replace_text(&f.id, at, "{\"silico\": 1,".to_owned()),
    );
    assert!(garbage.result.is_err());
    assert_eq!(f.revision(), at);
    assert_eq!(f.text(), block());
    let redone = f.redo(None).result.unwrap();
    assert_eq!((redone.restored, redone.undo, redone.redo), (rev(2), 1, 0));
    assert_eq!(f.text(), canonical(41, &fingerprint(40)));
}

#[test]
fn a_feature_added_removed_and_replaced_comes_back_through_undo_and_redo_byte_for_byte() {
    let f = opened(&block());
    f.begin();
    let mut texts = vec![f.text()];
    let added = answer(f.sessions.add_feature(
        &f.id,
        f.revision(),
        None,
        number(r#"{"type": "sphere", "name": "Ball", "position": [0, 0, 50], "radius": 5}"#),
    ));
    let ball = added.result.unwrap().feature.id;
    texts.push(f.text());
    let removed = answer(
        f.sessions
            .remove_feature(&f.id, f.revision(), id("k7f2qx3m")),
    );
    assert!(removed.result.unwrap().changed);
    texts.push(f.text());
    let removed = answer(f.sessions.remove_feature(&f.id, f.revision(), ball));
    assert!(removed.result.unwrap().changed);
    texts.push(f.text());
    assert!(f.replace(&block().replace("\"length\": 40", "\"length\": 44")));
    texts.push(f.text());
    for back in (0..texts.len() - 1).rev() {
        f.undo(None).result.unwrap();
        assert_eq!(f.text(), texts[back], "undone to state {back}");
    }
    assert_eq!(refused(&f.undo(None)).0, ErrorCode::InvalidInput);
    for (forward, text) in texts.iter().enumerate().skip(1) {
        f.redo(None).result.unwrap();
        assert_eq!(f.text(), *text, "redone to state {forward}");
    }
    assert_eq!(refused(&f.redo(None)).0, ErrorCode::InvalidInput);
}

#[test]
fn a_rollback_after_undos_and_redos_restores_begins_state_and_the_next_transaction_holds_none_of_them()
 {
    let f = opened(&block());
    f.begin();
    f.set(41).result.unwrap();
    f.set(42).result.unwrap();
    f.undo(None).result.unwrap();
    f.redo(None).result.unwrap();
    f.undo(Some(1)).result.unwrap();
    let rolled = answer(f.sessions.rollback(&f.id)).result.unwrap();
    assert_eq!(rolled.restored, rev(1));
    assert_eq!(f.text(), block());
    assert_eq!(f.file(), block());
    // The session is at 7; the new transaction's stack holds its `begin`, 7, alone.
    assert_eq!(f.revision(), rev(7));
    f.begin();
    for to in [1, 2, 3, 6] {
        let (code, message, parameter, _) = refused(&f.redo(Some(to)));
        assert_eq!(code, ErrorCode::InvalidInput);
        assert_eq!(
            message,
            format!(
                "revision {to} isn't a state of session {}'s undo stack, which holds revision 7",
                f.id
            )
        );
        assert_eq!(parameter.as_deref(), Some("to"));
    }
    assert_eq!(f.text(), block());
}

// ---------------------------------------------------------------------------------------
// Revisions and `to` at and past their edges.
// ---------------------------------------------------------------------------------------

#[test]
fn a_step_made_against_a_revision_past_the_session_or_at_the_last_is_a_conflict() {
    let f = opened(&block());
    f.begin();
    f.set(41).result.unwrap();
    for made in [rev(3), rev(u64::MAX), rev(1)] {
        for response in [
            answer(f.sessions.undo(&f.id, made, None)),
            answer(f.sessions.redo(&f.id, made, None)),
        ] {
            assert_eq!(refused(&response).0, ErrorCode::Conflict);
            assert_eq!(response.session.unwrap().revision, rev(2));
        }
    }
    assert_eq!(f.text(), canonical(41, &fingerprint(40)));
    f.undo(None).result.unwrap();
    assert_eq!(f.text(), block());
}

#[test]
fn a_to_past_the_session_or_at_u64_max_is_not_held_and_names_to() {
    let f = opened(&block());
    f.begin();
    f.set(41).result.unwrap();
    for to in [3, 4, u64::MAX] {
        for response in [f.undo(Some(to)), f.redo(Some(to))] {
            let (code, message, parameter, _) = refused(&response);
            assert_eq!(code, ErrorCode::InvalidInput);
            assert_eq!(
                message,
                format!(
                    "revision {to} isn't a state of session {}'s undo stack, which holds revisions 1 to 2",
                    f.id
                )
            );
            assert_eq!(parameter.as_deref(), Some("to"));
        }
    }
    assert_eq!(f.revision(), rev(2));
}

#[test]
fn a_to_at_a_state_restored_at_several_revisions_restores_it_by_any_of_them() {
    // Begin 1; edit 2; undo 3 (begin's state again); redo 4 (2's); undo 5 (1's); redo 6.
    let f = opened(&block());
    f.begin();
    f.set(41).result.unwrap();
    f.undo(None).result.unwrap();
    f.redo(None).result.unwrap();
    f.undo(None).result.unwrap();
    f.redo(None).result.unwrap();
    assert_eq!(f.revision(), rev(6));
    // The state now was the session's at 2, 4 and 6: each is the wrong side for both ways.
    for to in [2, 4, 6] {
        assert!(refused(&f.undo(Some(to))).1.contains("isn't before"));
        assert!(refused(&f.redo(Some(to))).1.contains("isn't after"));
    }
    for to in [3, 5, 1] {
        let undone = f.undo(Some(to)).result.unwrap();
        assert_eq!(undone.restored, rev(1));
        assert_eq!(f.text(), block());
        let redone = f.redo(Some(4)).result.unwrap();
        assert_eq!(redone.restored, rev(2));
        assert_eq!(f.text(), canonical(41, &fingerprint(40)));
    }
}

// ---------------------------------------------------------------------------------------
// Sessions that aren't there, and racing steps.
// ---------------------------------------------------------------------------------------

#[test]
fn a_step_in_a_session_unknown_or_closed_is_refused_naming_it() {
    let f = opened(&block());
    answer(f.sessions.close(&f.id)).result.unwrap();
    for response in [
        answer(f.sessions.undo(&f.id, rev(1), None)),
        answer(f.sessions.redo(&f.id, rev(1), Some(rev(1)))),
    ] {
        let problems = response.result.unwrap_err();
        let error = &problems.errors()[0];
        assert_eq!(error.parameter(), Some("session"), "{error:?}");
        assert!(error.message().contains(&f.id.to_string()), "{error:?}");
    }
}

#[test]
fn undos_raced_from_many_threads_against_one_revision_step_once() {
    let f = opened(&block());
    f.begin();
    for length in 41..=48 {
        f.set(length).result.unwrap();
    }
    let at = f.revision();
    assert_eq!(at, rev(9));
    let answers: Vec<Response<Stepped>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| scope.spawn(|| answer(f.sessions.undo(&f.id, at, None))))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let stepped: Vec<&Stepped> = answers
        .iter()
        .filter_map(|r| r.result.as_ref().ok())
        .collect();
    assert_eq!(stepped.len(), 1, "{answers:?}");
    assert_eq!(
        (stepped[0].restored, stepped[0].undo, stepped[0].redo),
        (rev(8), 7, 1)
    );
    for refusal in answers.iter().filter(|r| r.result.is_err()) {
        assert_eq!(refused(refusal).0, ErrorCode::Conflict);
    }
    assert_eq!(f.revision(), rev(10));
    assert_eq!(f.text(), canonical(47, &fingerprint(40)));
}

#[test]
fn undos_and_redos_interleaved_from_threads_each_against_the_revision_it_read_end_consistent() {
    // Each thread reads the revision, then steps against it: whatever interleaving, every
    // accepted step is a step of the stack, so the text is always one of its states and
    // the counts always sum to the edits made.
    let f = opened(&block());
    f.begin();
    for length in 41..=44 {
        f.set(length).result.unwrap();
    }
    std::thread::scope(|scope| {
        for thread in 0..8 {
            let f = &f;
            scope.spawn(move || {
                for i in 0..40 {
                    let at = f.revision();
                    let response = if (thread + i) % 2 == 0 {
                        answer(f.sessions.undo(&f.id, at, None))
                    } else {
                        answer(f.sessions.redo(&f.id, at, None))
                    };
                    match &response.result {
                        Ok(stepped) => assert_eq!(stepped.undo + stepped.redo, 4),
                        Err(_) => {
                            let code = refused(&response).0;
                            assert!(
                                matches!(code, ErrorCode::Conflict | ErrorCode::InvalidInput),
                                "{response:?}"
                            );
                        }
                    }
                }
            });
        }
    });
    let text = f.text();
    let states: Vec<String> = std::iter::once(block())
        .chain((41..=44).map(|l| canonical(l, &fingerprint(40))))
        .collect();
    let at = states.iter().position(|s| *s == text).unwrap();
    // From state `at`, `at` undos and 4 − `at` redos are left.
    let undone = f.undo(Some(1));
    if at == 0 {
        assert!(refused(&undone).1.contains("isn't before"));
    } else {
        let undone = undone.result.unwrap();
        assert_eq!((undone.undo, undone.redo), (0, 4));
    }
    f.redo(Some(5)).result.unwrap();
    assert_eq!(f.text(), states[4]);
}

// ---------------------------------------------------------------------------------------
// Random runs against a model of the stack.
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Set(u32),
    Undo,
    Redo,
    UndoTo(u64),
    RedoTo(u64),
    /// An undo made against the revision before the session's.
    StaleUndo,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (40u32..=43).prop_map(Op::Set),
        3 => Just(Op::Undo),
        2 => Just(Op::Redo),
        2 => (0u64..=24).prop_map(Op::UndoTo),
        2 => (0u64..=24).prop_map(Op::RedoTo),
        1 => Just(Op::StaleUndo),
    ]
}

/// The stack as the design has it: each state's length (`None` for `begin`'s, the file's
/// own bytes), and every revision it was the session's at.
struct Stack {
    states: Vec<(Option<u32>, Vec<u64>)>,
    at: usize,
    revision: u64,
}

/// What the model says a step gives: restored, undo left, redo left; or a refusal's code
/// and the start of its message.
type Expected = Result<(u64, usize, usize), (ErrorCode, String)>;

impl Stack {
    fn length(&self) -> u32 {
        self.states[self.at].0.unwrap_or(40)
    }

    fn text(&self) -> String {
        match self.states[self.at].0 {
            None => block(),
            Some(length) => canonical(length, &fingerprint(40)),
        }
    }

    fn restore(&mut self, target: usize) -> Expected {
        self.revision += 1;
        self.at = target;
        self.states[target].1.push(self.revision);
        Ok((
            self.states[target].1[0],
            self.at,
            self.states.len() - self.at - 1,
        ))
    }

    fn find(&self, to: u64) -> Option<usize> {
        self.states.iter().position(|s| s.1.contains(&to))
    }

    fn step(&mut self, undo: bool, to: Option<u64>) -> Expected {
        let refuse = |message: String| Err((ErrorCode::InvalidInput, message));
        match (undo, to) {
            (true, None) if self.at == 0 => refuse("nothing to undo".to_owned()),
            (true, None) => self.restore(self.at - 1),
            (false, None) if self.at + 1 == self.states.len() => {
                refuse("nothing to redo".to_owned())
            }
            (false, None) => self.restore(self.at + 1),
            (_, Some(to)) => match self.find(to) {
                None => refuse(format!("revision {to} isn't a state")),
                Some(found) if undo && found < self.at => self.restore(found),
                Some(found) if !undo && found > self.at => self.restore(found),
                Some(_) if undo => refuse(format!("revision {to} isn't before")),
                Some(_) => refuse(format!("revision {to} isn't after")),
            },
        }
    }
}

fn run(f: &Fixture, ops: &[Op]) {
    let mut stack = Stack {
        states: vec![(None, vec![1])],
        at: 0,
        revision: 1,
    };
    for (i, op) in ops.iter().enumerate() {
        let (response, expected) = match op {
            Op::Set(length) => {
                let set = f.set(*length);
                let changed = set.result.unwrap().changed;
                assert_eq!(changed, *length != stack.length(), "op {i}: {op:?}");
                if changed {
                    stack.states.truncate(stack.at + 1);
                    stack.revision += 1;
                    stack.states.push((Some(*length), vec![stack.revision]));
                    stack.at += 1;
                }
                assert_eq!(f.revision().number(), stack.revision, "op {i}: {op:?}");
                assert_eq!(f.text(), stack.text(), "op {i}: {op:?}");
                continue;
            }
            Op::Undo => (f.undo(None), stack.step(true, None)),
            Op::Redo => (f.redo(None), stack.step(false, None)),
            Op::UndoTo(to) => match Revision::from_number(*to) {
                Some(_) => (f.undo(Some(*to)), stack.step(true, Some(*to))),
                None => continue,
            },
            Op::RedoTo(to) => match Revision::from_number(*to) {
                Some(_) => (f.redo(Some(*to)), stack.step(false, Some(*to))),
                None => continue,
            },
            Op::StaleUndo => {
                let Some(before) = Revision::from_number(stack.revision - 1) else {
                    continue;
                };
                (
                    answer(f.sessions.undo(&f.id, before, None)),
                    Err((ErrorCode::Conflict, "the undo was made against".to_owned())),
                )
            }
        };
        match (&response.result, &expected) {
            (Ok(stepped), Ok((restored, undo, redo))) => {
                assert_eq!(
                    (
                        stepped.restored.number(),
                        stepped.undo,
                        stepped.redo,
                        stepped.dropped
                    ),
                    (*restored, *undo, *redo, 0),
                    "op {i}: {op:?}"
                );
            }
            (Err(_), Err((code, start))) => {
                let (got, message, _, _) = refused(&response);
                assert_eq!(got, *code, "op {i}: {op:?}: {message}");
                assert!(message.starts_with(start), "op {i}: {op:?}: {message}");
            }
            _ => panic!("op {i}: {op:?}: got {response:?}, expected {expected:?}"),
        }
        assert_eq!(
            response.session.as_ref().unwrap().revision.number(),
            stack.revision,
            "op {i}: {op:?}"
        );
        assert_eq!(f.text(), stack.text(), "op {i}: {op:?}");
    }
    // The commit writes the state the session is at, its fingerprint as built, or nothing
    // if that's `begin`'s document.
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    let length = stack.length();
    if length == 40 {
        assert!(!committed.written);
        assert_eq!(f.file(), block());
    } else {
        assert!(committed.written);
        assert_eq!(committed.refingerprinted, [id("k7f2qx3m")]);
        assert_eq!(f.file(), canonical(length, &fingerprint(length)));
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    #[test]
    fn random_runs_of_edits_undos_and_redos_follow_the_stacks_rules(
        ops in prop::collection::vec(op(), 1..24)
    ) {
        let f = opened(&block());
        f.begin();
        run(&f, &ops);
    }
}

// ---------------------------------------------------------------------------------------
// Checks and commits around a step.
// ---------------------------------------------------------------------------------------

/// The block 40 long, its fingerprint stored 1e-6 mm³ over what it measures (8000), on one
/// line: drift, which a check says unless an edit reached the block.
fn drifted() -> String {
    block().replace("\"volume\": 8000,", "\"volume\": 8000.000001,")
}

/// A check of the current revision: the codes of its problems, and the revision its model is
/// of.
fn check(f: &Fixture) -> (Vec<ErrorCode>, Revision) {
    let response = answer(f.sessions.check(
        &f.id,
        silico_core::command::CheckModel::Current { fresh: false },
    ));
    let codes = response
        .result
        .unwrap()
        .problems
        .errors()
        .iter()
        .map(|e| e.code())
        .collect();
    (codes, response.model.unwrap().revision)
}

#[test]
fn a_check_after_an_undo_reads_the_restored_states_model_and_its_drift_again() {
    // The stored fingerprint drifts. Edited, the block is reached and its drift not said (a
    // commit writes it anew); every edit undone, it's reached no more, and the drift is
    // said again; redone, it isn't.
    let f = opened(&drifted());
    assert_eq!(check(&f), (vec![ErrorCode::Drift], rev(1)));
    f.begin();
    f.set(41).result.unwrap();
    assert_eq!(check(&f), (vec![], rev(2)));
    f.set(42).result.unwrap();
    f.undo(None).result.unwrap();
    assert_eq!(check(&f), (vec![], rev(4)));
    f.undo(None).result.unwrap();
    assert_eq!(f.text(), drifted());
    assert_eq!(check(&f), (vec![ErrorCode::Drift], rev(5)));
    f.redo(Some(3)).result.unwrap();
    assert_eq!(
        f.text(),
        canonical(42, &fingerprint(40).replace("8000", "8000.000001"))
    );
    assert_eq!(check(&f), (vec![], rev(6)));
}

#[test]
fn a_commit_refused_for_a_file_changed_outside_leaves_the_stack_to_step_through() {
    let f = opened(&block());
    f.begin();
    f.set(41).result.unwrap();
    f.set(42).result.unwrap();
    std::fs::write(&f.path, drifted()).unwrap();
    let committed = answer(f.sessions.commit(&f.id));
    assert!(committed.result.is_err(), "{committed:?}");
    assert_eq!(f.file(), drifted());
    // The transaction and its stack as they were.
    let undone = f.undo(None).result.unwrap();
    assert_eq!((undone.restored, undone.undo, undone.redo), (rev(2), 1, 1));
    assert_eq!(f.text(), canonical(41, &fingerprint(40)));
    let undone = f.undo(None).result.unwrap();
    assert_eq!((undone.restored, undone.undo, undone.redo), (rev(1), 0, 2));
    assert_eq!(f.text(), block());
    f.redo(Some(3)).result.unwrap();
    assert_eq!(f.text(), canonical(42, &fingerprint(40)));
    answer(f.sessions.rollback(&f.id)).result.unwrap();
    assert_eq!(f.file(), drifted());
}

#[test]
fn a_long_documents_hundred_edits_undone_and_redone_one_by_one_give_each_text_back() {
    // 2,000 boxes, each edit one box's length: every state but `begin`'s shares all its
    // chunks but one with the one before. 100 edits, then 100 undos and 100 redos, each
    // its text back as made.
    let boxes: Vec<String> = (0..2000)
        .map(|i| {
            format!(
                r#"{{"id": "b{i:07}", "type": "box", "v": 1, "name": "B{i}", "position": [{}, 0, 0], "length": 1, "width": 1, "height": 1}}"#,
                i * 2
            )
        })
        .collect();
    let f = opened(&format!("{HEAD}{}]}}", boxes.join(", ")));
    f.begin();
    let mut texts = vec![f.text()];
    for i in 0..100u32 {
        let feature = format!("b{:07}", (i * 37) % 2000);
        let response = answer(f.sessions.set_field(
            &f.id,
            f.revision(),
            id(&feature),
            "length".to_owned(),
            number(&(2 + i).to_string()),
        ));
        assert!(response.result.unwrap().changed);
        texts.push(f.text());
    }
    for back in (0..100).rev() {
        f.undo(None).result.unwrap();
        assert_eq!(f.text(), texts[back]);
    }
    for text in texts.iter().skip(1) {
        f.redo(None).result.unwrap();
        assert_eq!(f.text(), *text);
    }
}
