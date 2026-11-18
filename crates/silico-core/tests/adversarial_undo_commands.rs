//! Hostile inputs for M5.2's `undo` and `redo` on the wire and `status`'s transaction
//! counts (break-it, PR #300): `changed` checked after every step of random runs of edits,
//! whole-text replacements (begin's own bytes, the same document laid out otherwise, text
//! that doesn't read, which is refused, another length), undos and redos, against an oracle
//! written here (the two texts parsed by serde_json and compared as values, or byte for
//! byte where either doesn't parse), and against what the commit at the end then does;
//! `status` read from another thread while undos and redos of a long document run, each
//! answer one state's, never half of one and half of the next; requests through
//! `Sessions::call` at their edges, `to` and `revision` at every one; each kind of edit
//! undone and redone byte for byte; `call` by name against the typed methods through random
//! runs; and steps racing edits and `status`, replayed on a model as one order. Expected
//! values: the oracle, the stack's rules (undo left is the states before the session's,
//! redo left those after it), and the documented contract (`changed` false means a commit
//! writes nothing).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use proptest::prelude::*;
use silico_core::cache::Cache;
use silico_core::command::{Call, Response, Revision, SessionId, Sessions, Stepped, Transaction};
use silico_core::document::json::Node;
use silico_core::fake::FakeKernel;
use tempfile::TempDir;

use crate::sessions::{answer, id};

const HEAD: &str =
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": ["#;

/// The block's feature on one line, `length` long, its fingerprint stored for 40.
fn block_line(length: u32) -> String {
    format!(
        r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": {length}, "width": 20, "height": 10, "fingerprint": {{"volume": 8000, "area": 2800, "center": [20, 10, 5], "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}}}"#
    )
}

/// The block `length` long on one line: begin's file at 40, not canonical form.
fn block(length: u32) -> String {
    format!("{HEAD}{}]}}", block_line(length))
}

/// The same document as [`block`], laid out otherwise: a newline after each comma of the
/// header and two spaces before the feature.
fn spread(length: u32) -> String {
    format!(
        "{}\n  {}\n]}}\n",
        HEAD.replace(", ", ",\n"),
        block_line(length)
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

fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let sessions = Sessions::with_cache_opener(Arc::new(FakeKernel::new(2).unwrap()), move |c| {
        Cache::with_secret(c, &secret)
    });
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
        self.sessions.status(&self.id).session.unwrap().revision
    }

    fn text(&self) -> String {
        answer(self.sessions.text(&self.id, None))
            .result
            .unwrap()
            .text
    }

    fn transaction(&self) -> Transaction {
        self.sessions
            .status(&self.id)
            .result
            .unwrap()
            .transaction
            .unwrap()
    }

    fn undo(&self, to: Option<u64>) -> Response<Stepped> {
        answer(self.sessions.undo(&self.id, self.revision(), to.map(rev)))
    }

    fn redo(&self, to: Option<u64>) -> Response<Stepped> {
        answer(self.sessions.redo(&self.id, self.revision(), to.map(rev)))
    }
}

/// Whether `now` is another document than `begun`, as the oracle sees it: both parsed by
/// serde_json and compared as values (layout and whitespace aside); byte for byte where
/// either doesn't parse.
fn differs(begun: &str, now: &str) -> bool {
    // The header's `naming` says which scheme wrote the names, not what the document is: 5
    // and 6 read as one document (6 reads 5's names as they are, N10), and every save
    // writes 6, so a file begun at 5 and written back is unchanged.
    let read = |text: &str| {
        serde_json::from_str::<serde_json::Value>(text).map(|mut value| {
            if let Some(header) = value.as_object_mut() {
                header.remove("naming");
            }
            value
        })
    };
    match (read(begun), read(now)) {
        (Ok(a), Ok(b)) => a != b,
        _ => begun != now,
    }
}

// ---------------------------------------------------------------------------------------
// `changed` through random runs, against the oracle and the commit.
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Set(u32),
    /// begin's own bytes, given back whole.
    ReplaceBegun,
    /// The block `length` long, laid out otherwise.
    ReplaceSpread(u32),
    /// Text that doesn't read: a merge's conflict marker before the block.
    ReplaceUnread,
    Undo,
    Redo,
    UndoTo(u64),
    RedoTo(u64),
    /// `format` inside the transaction: canonical form asked for, compared byte for byte
    /// from its state on (M5.0's A6), until a step restores a state from before it.
    Format,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (40u32..=42).prop_map(Op::Set),
        1 => Just(Op::ReplaceBegun),
        2 => (40u32..=42).prop_map(Op::ReplaceSpread),
        1 => Just(Op::ReplaceUnread),
        3 => Just(Op::Undo),
        2 => Just(Op::Redo),
        1 => (1u64..=20).prop_map(Op::UndoTo),
        1 => (1u64..=20).prop_map(Op::RedoTo),
        2 => Just(Op::Format),
    ]
}

/// A model of the stack's states, for the oracle: each one's revisions (the one it was
/// made at first) and whether a `format` had asked for canonical form by then (set on a
/// format's state, taken by each edit's after it, restored with the state by a step).
struct Model {
    states: Vec<(Vec<Revision>, bool)>,
    at: usize,
}

impl Model {
    fn new(begun: Revision) -> Self {
        Model {
            states: vec![(vec![begun], false)],
            at: 0,
        }
    }

    /// An edit's state, at `revision`: the redo states dropped; formatted if the state
    /// before was, or if it's a format's.
    fn edit(&mut self, revision: Revision, format: bool) {
        let formatted = self.formatted() || format;
        self.states.truncate(self.at + 1);
        self.states.push((vec![revision], formatted));
        self.at += 1;
    }

    /// A step that restored the state made at `restored`, at `revision`, a new one.
    fn step(&mut self, restored: Revision, revision: Revision) {
        self.at = self
            .states
            .iter()
            .position(|(revisions, _)| revisions.first() == Some(&restored))
            .unwrap();
        self.states[self.at].0.push(revision);
    }

    fn formatted(&self) -> bool {
        self.states[self.at].1
    }
}

/// Runs `ops` in a transaction begun on [`block`] 40 long, checking `status`'s transaction
/// after each: `changed` as the oracle has it (byte for byte once a format in force at the
/// session's state asked for canonical form, as the model has it), `undo` + `redo` the
/// states the stack holds beside the session's, and a step's counts the ones `status` then
/// gives. Then the commit: written exactly when `changed` said so (where the text reads).
fn run(ops: &[Op]) {
    let begun = block(40);
    let f = opened(&begun);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    let mut model = Model::new(f.revision());
    let unread = format!("<<<<<<< ours\n{begun}");
    // The states the stack holds: begin's, and one for each edit kept.
    let mut held = 1usize;
    for (i, op) in ops.iter().enumerate() {
        let at = f.revision();
        let step = match op {
            Op::Set(length) => {
                let set = answer(f.sessions.set_field(
                    &f.id,
                    at,
                    id("k7f2qx3m"),
                    "length".to_owned(),
                    number(&length.to_string()),
                ));
                // Refused where the text doesn't read; otherwise an edit, or none.
                if let Ok(set) = set.result
                    && set.changed
                {
                    let t = f.transaction();
                    held = t.undo + 1;
                    model.edit(f.revision(), false);
                }
                None
            }
            Op::Format => {
                // Refused where the text doesn't read; a change held only if the canonical
                // text isn't the transaction's already.
                if let Ok(formatted) = answer(f.sessions.format(&f.id, None, true)).result
                    && formatted.changed
                {
                    held = f.transaction().undo + 1;
                    model.edit(f.revision(), true);
                }
                None
            }
            Op::ReplaceBegun | Op::ReplaceSpread(_) | Op::ReplaceUnread => {
                let text = match op {
                    Op::ReplaceBegun => begun.clone(),
                    Op::ReplaceSpread(length) => spread(*length),
                    _ => unread.clone(),
                };
                let replaced = answer(f.sessions.replace_text(&f.id, at, text));
                if let Ok(replaced) = replaced.result
                    && replaced.changed
                {
                    held = f.transaction().undo + 1;
                    model.edit(f.revision(), false);
                }
                None
            }
            Op::Undo => Some(f.undo(None)),
            Op::Redo => Some(f.redo(None)),
            Op::UndoTo(to) => Some(f.undo(Some(*to))),
            Op::RedoTo(to) => Some(f.redo(Some(*to))),
        };
        if let Some(Ok(stepped)) = step.as_ref().map(|s| s.result.as_ref()) {
            model.step(stepped.restored, f.revision());
        }
        let t = f.transaction();
        let now = f.text();
        let changed = if model.formatted() {
            begun != now
        } else {
            differs(&begun, &now)
        };
        assert_eq!(
            t.changed, changed,
            "op {i}: {op:?}: status {t:?}, text {now:?}"
        );
        assert_eq!(t.undo + t.redo + 1, held, "op {i}: {op:?}: {t:?}");
        if let Some(Ok(stepped)) = step.as_ref().map(|s| s.result.as_ref()) {
            assert_eq!(
                (stepped.undo, stepped.redo, stepped.dropped),
                (t.undo, t.redo, t.dropped),
                "op {i}: {op:?}"
            );
        }
    }
    let t = f.transaction();
    let now = f.text();
    let committed = answer(f.sessions.commit(&f.id));
    match committed.result {
        Ok(committed) => assert_eq!(
            committed.written, t.changed,
            "status said changed {} of {now:?}",
            t.changed
        ),
        // A text that doesn't read is never committed; `changed` said it differs.
        Err(problems) => {
            assert!(serde_json::from_str::<serde_json::Value>(&now).is_err());
            assert!(t.changed, "{problems:?}");
        }
    }
    if !t.changed {
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), begun);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    #[test]
    fn changed_follows_the_document_through_random_edits_replacements_undos_and_redos(
        ops in prop::collection::vec(op(), 1..16)
    ) {
        run(&ops);
    }
}

#[test]
fn begins_document_laid_out_otherwise_by_replace_text_is_unchanged_and_commits_nothing() {
    run(&[Op::ReplaceSpread(40)]);
}

#[test]
fn text_that_doesnt_read_is_refused_and_changed_stays_as_it_was() {
    // replace_text refuses text that doesn't read: `changed` and the stack as they were,
    // before an edit and after one, undone or not.
    run(&[Op::ReplaceUnread, Op::ReplaceBegun]);
    run(&[Op::Set(41), Op::ReplaceUnread, Op::Undo]);
    run(&[Op::Set(41), Op::Undo, Op::ReplaceUnread, Op::Redo]);
}

#[test]
fn a_transaction_begun_on_text_that_doesnt_read_changes_by_its_bytes_alone() {
    // begin's own text doesn't read, so `changed` is the bytes' difference. `replace_text`
    // takes only text that reads, so the mend is a change; undone, begin's bytes come back,
    // unchanged; redone, changed again; and text that doesn't read is refused, the
    // transaction as it was.
    let begun = format!("<<<<<<< ours\n{}", block(40));
    let f = opened(&begun);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    assert!(!f.transaction().changed);
    let refused = answer(
        f.sessions
            .replace_text(&f.id, f.revision(), format!("{begun} ")),
    );
    let problems = refused.result.unwrap_err();
    assert_eq!(problems.errors()[0].parameter(), Some("text"));
    assert_eq!(f.transaction().undo, 0);
    assert!(!f.transaction().changed);
    answer(f.sessions.replace_text(&f.id, f.revision(), block(40)))
        .result
        .unwrap();
    assert!(f.transaction().changed);
    // The same document laid out otherwise is no edit, and still differs from begin's
    // bytes.
    let same = answer(f.sessions.replace_text(&f.id, f.revision(), spread(40)));
    assert!(!same.result.unwrap().changed);
    assert!(f.transaction().changed);
    let mut seen = Vec::new();
    while f.undo(None).result.is_ok() {
        seen.push(f.transaction().changed);
    }
    assert_eq!(seen, [false]);
    assert_eq!(f.text(), begun);
    f.redo(None).result.unwrap();
    assert!(f.transaction().changed);
    f.undo(None).result.unwrap();
    let committed = answer(f.sessions.commit(&f.id));
    // Unchanged, so it writes nothing, though the text doesn't read.
    if let Ok(committed) = committed.result {
        assert!(!committed.written);
    }
    assert_eq!(std::fs::read_to_string(&f.path).unwrap(), begun);
}

// ---------------------------------------------------------------------------------------
// `status` beside an undo: one state's, never half of each.
// ---------------------------------------------------------------------------------------

/// A document of `n` blocks, each 10 long, one beside the next, the first the block above.
fn long_document(n: usize) -> String {
    let mut text = HEAD.to_owned();
    text.push_str(&block_line(40));
    for i in 1..n {
        text.push_str(&format!(
            r#", {{"id": "b{i:07}", "type": "box", "v": 1, "name": "Block {i}", "position": [{}, 0, 0], "length": 10, "width": 10, "height": 10}}"#,
            100 + 20 * i
        ));
    }
    text.push_str("]}");
    text
}

#[test]
fn status_read_beside_undos_and_redos_of_a_long_document_is_always_one_states() {
    // One edit, then undo and redo in turn: at an even revision the edit stands (one to
    // undo, none to redo, changed); at an odd one past begin's, it's undone (none to undo,
    // one to redo, unchanged). A status read while a step runs is one revision's: its
    // counts and `changed` that revision's, never the stack's after the step with the
    // document and revision of before it.
    // Broke (break-it): `step` moved the stack under one lock, then gated the restored text
    // and compared it with begin's off the lock, then changed the document, the revision,
    // and `changed` under a second. A `status` between them gave the revision and `changed`
    // of before the step with the stack's counts of after it: at revision 2 (the edit
    // standing, changed), `undo: 0, redo: 1`. Now the step is planned under the first lock,
    // the stack left as it is, and applied with the rest under the second.
    let f = opened(&long_document(4000));
    answer(f.sessions.begin(&f.id)).result.unwrap();
    answer(f.sessions.set_field(
        &f.id,
        rev(1),
        id("k7f2qx3m"),
        "length".to_owned(),
        number("41"),
    ))
    .result
    .unwrap();
    let done = AtomicBool::new(false);
    let (torn, first): (usize, Option<(u64, Transaction)>) = std::thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            let (mut torn, mut first) = (0usize, None);
            while !done.load(Ordering::Relaxed) {
                let status = f.sessions.status(&f.id);
                let revision = status.session.unwrap().revision.number();
                let t = status.result.unwrap().transaction.unwrap();
                let want = if revision.is_multiple_of(2) {
                    (1, 0, true)
                } else {
                    (0, 1, false)
                };
                if (t.undo, t.redo, t.changed) != want {
                    torn += 1;
                    first = first.or(Some((revision, t)));
                }
            }
            (torn, first)
        });
        for i in 0..40 {
            let at = f.revision();
            let stepped = if i % 2 == 0 {
                answer(f.sessions.undo(&f.id, at, None))
            } else {
                answer(f.sessions.redo(&f.id, at, None))
            };
            stepped.result.unwrap();
        }
        done.store(true, Ordering::Relaxed);
        watcher.join().unwrap()
    });
    assert!(torn == 0, "{torn} torn statuses, the first {first:?}");
}

// ---------------------------------------------------------------------------------------
// By name, at the request's edges.
// ---------------------------------------------------------------------------------------

/// `name` called with `arguments` on `f`'s sessions: its envelope.
fn call(f: &Fixture, name: &str, arguments: &str) -> serde_json::Value {
    match f.sessions.call(name, arguments) {
        Call::Running(calling) => {
            let answered = calling
                .wait_timeout(std::time::Duration::from_secs(60))
                .ok()
                .unwrap();
            serde_json::from_str(&answered.text).unwrap()
        }
        Call::Unknown => panic!("{name} unknown"),
    }
}

/// The first error's code and parameter.
fn first(envelope: &serde_json::Value) -> (String, String) {
    let e = &envelope["errors"][0];
    (
        e["code"].as_str().unwrap_or("").to_owned(),
        e["parameter"].as_str().unwrap_or("").to_owned(),
    )
}

#[test]
fn hostile_step_requests_by_name_are_refused_at_their_fields_and_leave_the_stack_as_it_was() {
    let f = opened(&block(40));
    answer(f.sessions.begin(&f.id)).result.unwrap();
    for length in ["41", "42"] {
        answer(f.sessions.set_field(
            &f.id,
            f.revision(),
            id("k7f2qx3m"),
            "length".to_owned(),
            number(length),
        ))
        .result
        .unwrap();
    }
    assert_eq!(f.revision(), rev(3));
    let before = f.transaction();
    for name in ["undo", "redo"] {
        for (arguments, want) in [
            // Not an object.
            ("[]", ("invalid_input", "")),
            ("null", ("invalid_input", "")),
            // `to` missing, or given twice (refused by the reader, before any field).
            (
                r#"{"session": "s1", "revision": 3}"#,
                ("invalid_input", "to"),
            ),
            (
                r#"{"session": "s1", "revision": 3, "to": 1, "to": 2}"#,
                ("invalid_input", ""),
            ),
            // A revision of 0, or past u64.
            (
                r#"{"session": "s1", "revision": 0, "to": null}"#,
                ("invalid_input", "revision"),
            ),
            (
                r#"{"session": "s1", "revision": 18446744073709551616, "to": null}"#,
                ("invalid_input", "revision"),
            ),
            // At u64::MAX: a conflict, not a step.
            (
                r#"{"session": "s1", "revision": 18446744073709551615, "to": null}"#,
                ("conflict", "revision"),
            ),
            // `to` true, an object, a revision written as a float.
            (
                r#"{"session": "s1", "revision": 3, "to": true}"#,
                ("invalid_input", "to"),
            ),
            (
                r#"{"session": "s1", "revision": 3, "to": {}}"#,
                ("invalid_input", "to"),
            ),
            // A session that isn't open, and a session's ID that isn't one.
            (
                r#"{"session": "s2", "revision": 3, "to": null}"#,
                ("invalid_input", "session"),
            ),
            (
                r#"{"session": "s01", "revision": 3, "to": null}"#,
                ("invalid_input", "session"),
            ),
        ] {
            let envelope = call(&f, name, arguments);
            assert_eq!(envelope["ok"], false, "{name} {arguments}: {envelope}");
            assert_eq!(
                first(&envelope),
                (want.0.to_owned(), want.1.to_owned()),
                "{name} {arguments}: {envelope}"
            );
            assert_eq!(f.transaction(), before, "{name} {arguments}");
            assert_eq!(f.revision(), rev(3), "{name} {arguments}");
        }
    }
    // Still steps after all of it: 42 undone, then redone.
    let undone = call(
        &f,
        "undo",
        r#"{"session": "s1", "revision": 3, "to": null}"#,
    );
    assert_eq!(
        undone["result"],
        serde_json::json!({"restored": 2, "undo": 1, "redo": 1, "dropped": 0})
    );
    let redone = call(&f, "redo", r#"{"session": "s1", "revision": 4, "to": 3}"#);
    assert_eq!(
        redone["result"],
        serde_json::json!({"restored": 3, "undo": 2, "redo": 0, "dropped": 0})
    );
}

#[test]
fn a_step_after_rollback_or_commit_is_refused_as_no_transaction_and_status_has_none() {
    for (end, request) in [
        ("rollback", r#"{"session": "s1"}"#),
        (
            "commit",
            r#"{"session": "s1", "path": null, "replace": false}"#,
        ),
    ] {
        let f = opened(&block(40));
        answer(f.sessions.begin(&f.id)).result.unwrap();
        answer(f.sessions.set_field(
            &f.id,
            rev(1),
            id("k7f2qx3m"),
            "length".to_owned(),
            number("41"),
        ))
        .result
        .unwrap();
        let ended = call(&f, end, request);
        assert_eq!(ended["ok"], true, "{end}: {ended}");
        let at = f.revision().number();
        for name in ["undo", "redo"] {
            let envelope = call(
                &f,
                name,
                &format!(r#"{{"session": "s1", "revision": {at}, "to": null}}"#),
            );
            assert_eq!(
                envelope["errors"][0]["message"], "no transaction is open in session s1",
                "{end} {name}: {envelope}"
            );
        }
        let status = call(&f, "status", r#"{"session": "s1"}"#);
        assert_eq!(status["result"]["transaction"], serde_json::Value::Null);
        // A new transaction holds none of the old one's states.
        answer(f.sessions.begin(&f.id)).result.unwrap();
        assert_eq!(
            (
                f.transaction().undo,
                f.transaction().redo,
                f.transaction().changed
            ),
            (0, 0, false),
            "{end}"
        );
    }
}

#[test]
fn a_state_the_gate_refuses_is_undone_redone_and_counted_and_the_next_edit_after_it_works() {
    // A block 0 long reads but the gate refuses it (a length must be positive), and an
    // edit inside a transaction is refused if the gate refuses what it makes, so begin's
    // own file is the one such state. Mended, then undone back into it and redone out;
    // `status` says what each state is; and an edit after the undo drops the mend from
    // redo and is kept.
    let f = opened(&block(0));
    answer(f.sessions.begin(&f.id)).result.unwrap();
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!((status.features, status.gated), (Some(1), false));
    let mended = answer(f.sessions.replace_text(&f.id, rev(1), block(40)));
    assert!(mended.result.unwrap().changed);
    // As held: the edit's state, in canonical form, which a redo gives back byte for byte.
    let mend = f.text();
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!((status.features, status.gated), (Some(1), true));
    assert_eq!(
        (
            status.transaction.unwrap().undo,
            status.transaction.unwrap().changed
        ),
        (1, true)
    );
    // Undone: begin's, unchanged, refused by the gate again.
    let undone = f.undo(None).result.unwrap();
    assert_eq!((undone.restored, undone.undo, undone.redo), (rev(1), 0, 1));
    assert_eq!(f.text(), block(0));
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!((status.features, status.gated), (Some(1), false));
    assert!(!status.transaction.unwrap().changed);
    // An edit other than replace_text is refused here, the stack as it was.
    let set = answer(f.sessions.set_field(
        &f.id,
        f.revision(),
        id("k7f2qx3m"),
        "length".to_owned(),
        number("41"),
    ));
    assert!(set.result.is_err());
    let t = f.transaction();
    assert_eq!((t.undo, t.redo, t.changed), (0, 1, false));
    // Redone: the mend again.
    let redone = f.redo(None).result.unwrap();
    assert_eq!((redone.restored, redone.undo, redone.redo), (rev(2), 1, 0));
    assert_eq!(f.text(), mend);
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!(
        (status.gated, status.transaction.unwrap().changed),
        (true, true)
    );
    // Undone again, then mended otherwise: kept, the first mend gone from redo.
    f.undo(None).result.unwrap();
    answer(f.sessions.replace_text(&f.id, f.revision(), block(41)))
        .result
        .unwrap();
    let t = f.transaction();
    assert_eq!((t.undo, t.redo, t.changed), (1, 0, true));
    assert!(f.redo(None).result.is_err());
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(committed.written);
}

/// The block and a lug, two independent boxes, on one line.
fn two_boxes() -> String {
    format!(
        r#"{HEAD}{}, {{"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10}}]}}"#,
        block_line(40)
    )
}

#[test]
fn a_move_and_a_remove_each_edited_back_to_begins_order_is_unchanged_and_commits_nothing() {
    // The lug moved before the block, then back to the end: begin's document again, by
    // two edits and not by undo, so `changed` is false with two to undo. Then the lug
    // removed and the removal undone: unchanged again.
    let f = opened(&two_boxes());
    answer(f.sessions.begin(&f.id)).result.unwrap();
    let moved =
        answer(
            f.sessions
                .move_feature(&f.id, rev(1), id("b0000001"), Some(id("k7f2qx3m"))),
        );
    assert!(moved.result.unwrap().changed);
    assert!(f.transaction().changed);
    let back = answer(f.sessions.move_feature(&f.id, rev(2), id("b0000001"), None));
    assert!(back.result.unwrap().changed);
    let t = f.transaction();
    assert_eq!((t.undo, t.redo, t.changed), (2, 0, false));
    let removed = answer(f.sessions.remove_feature(&f.id, rev(3), id("b0000001")));
    removed.result.unwrap();
    assert!(f.transaction().changed);
    f.undo(None).result.unwrap();
    let t = f.transaction();
    assert_eq!((t.undo, t.redo, t.changed), (2, 1, false));
    // Undone `to` the move: changed; redone `to` the end: not.
    f.undo(Some(2)).result.unwrap();
    assert!(f.transaction().changed);
    f.redo(Some(4)).result.unwrap();
    let t = f.transaction();
    assert_eq!((t.undo, t.redo, t.changed), (3, 0, true));
    f.undo(None).result.unwrap();
    assert!(!f.transaction().changed);
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(!committed.written);
    assert_eq!(std::fs::read_to_string(&f.path).unwrap(), two_boxes());
}

// ---------------------------------------------------------------------------------------
// `to` at its edges, stale revisions, each kind of edit undone, by name against typed, and
// steps racing edits and `status`.
// ---------------------------------------------------------------------------------------

/// `name` called with `arguments` on `f`'s sessions: its error's code, parameter, and the
/// envelope.
fn refused_at(f: &Fixture, name: &str, arguments: &str) -> (String, String, serde_json::Value) {
    let envelope = call(f, name, arguments);
    assert_eq!(envelope["ok"], false, "{name} {arguments}: {envelope}");
    let (code, parameter) = first(&envelope);
    (code, parameter, envelope)
}

#[test]
fn a_hostile_to_or_revision_is_refused_at_its_field_and_leaves_the_stack_as_it_was() {
    // 41 then 42, then one undo: at revision 4, 41's state the session's (made at 2), undo
    // can reach `begin`'s (1), and redo 42's (3).
    let f = opened(&block(40));
    answer(f.sessions.begin(&f.id)).result.unwrap();
    for length in ["41", "42"] {
        answer(f.sessions.set_field(
            &f.id,
            f.revision(),
            id("k7f2qx3m"),
            "length".to_owned(),
            number(length),
        ))
        .result
        .unwrap();
    }
    f.undo(None).result.unwrap();
    assert_eq!(f.revision(), rev(4));
    let before = (f.transaction(), f.text());
    let unchanged = |what: &str| {
        assert_eq!((f.transaction(), f.text()), before, "{what}");
        assert_eq!(f.revision(), rev(4), "{what}");
    };
    // Not a revision at all: 0, negative, fractional, written with an exponent or a point,
    // a string, a list, past u64, and a number past f64.
    for name in ["undo", "redo"] {
        for to in [
            "0",
            "-1",
            "-0",
            "1.5",
            "2.0",
            "2e0",
            "1e400",
            r#""2""#,
            r#""""#,
            "[2]",
            "18446744073709551616",
        ] {
            let arguments = format!(r#"{{"session": "s1", "revision": 4, "to": {to}}}"#);
            let (code, parameter, envelope) = refused_at(&f, name, &arguments);
            assert_eq!(
                (code.as_str(), parameter.as_str()),
                ("invalid_input", "to"),
                "{name} {to}: {envelope}"
            );
            unchanged(&format!("{name} {to}"));
        }
    }
    // Revisions no state was the session's at: u64::MAX, one not yet made, one far ahead.
    for name in ["undo", "redo"] {
        for to in ["18446744073709551615", "5", "99"] {
            let arguments = format!(r#"{{"session": "s1", "revision": 4, "to": {to}}}"#);
            let (code, parameter, envelope) = refused_at(&f, name, &arguments);
            assert_eq!(
                (code.as_str(), parameter.as_str()),
                ("invalid_input", "to"),
                "{name} {to}: {envelope}"
            );
            assert!(
                envelope["errors"][0]["message"]
                    .as_str()
                    .unwrap()
                    .contains("isn't a state of"),
                "{name} {to}: {envelope}"
            );
            unchanged(&format!("{name} {to}"));
        }
    }
    // On the wrong side, or the session's own state by either revision it was made or
    // restored at (2 and 4): refused naming `to`.
    for (name, to) in [
        ("undo", 4),
        ("undo", 2),
        ("undo", 3),
        ("redo", 4),
        ("redo", 2),
        ("redo", 1),
    ] {
        let arguments = format!(r#"{{"session": "s1", "revision": 4, "to": {to}}}"#);
        let (code, parameter, envelope) = refused_at(&f, name, &arguments);
        assert_eq!(
            (code.as_str(), parameter.as_str()),
            ("invalid_input", "to"),
            "{name} {to}: {envelope}"
        );
        unchanged(&format!("{name} {to}"));
    }
    // A stale revision, or one not yet made, is a conflict at `revision`, even with a `to`
    // the stack holds on the right side.
    for (name, revision, to) in [
        ("undo", 3, "1"),
        ("undo", 1, "null"),
        ("undo", 5, "1"),
        ("redo", 2, "3"),
        ("redo", 5, "null"),
    ] {
        let arguments = format!(r#"{{"session": "s1", "revision": {revision}, "to": {to}}}"#);
        let (code, parameter, envelope) = refused_at(&f, name, &arguments);
        assert_eq!(
            (code.as_str(), parameter.as_str()),
            ("conflict", "revision"),
            "{name} {revision} {to}: {envelope}"
        );
        unchanged(&format!("{name} {revision} {to}"));
    }
    // And it still steps both ways.
    let undone = call(&f, "undo", r#"{"session": "s1", "revision": 4, "to": 1}"#);
    assert_eq!(
        undone["result"],
        serde_json::json!({"restored": 1, "undo": 0, "redo": 2, "dropped": 0})
    );
    let redone = call(&f, "redo", r#"{"session": "s1", "revision": 5, "to": 3}"#);
    assert_eq!(
        redone["result"],
        serde_json::json!({"restored": 3, "undo": 2, "redo": 0, "dropped": 0})
    );
}

/// The features' IDs, in order, as serde_json reads `text`.
fn ids(text: &str) -> Vec<String> {
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    value["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap().to_owned())
        .collect()
}

const PIN: &str = r#"{"id": "c0000001", "type": "cylinder", "name": "Pin", "position": [200, 0, 0], "radius": 2, "height": 5}"#;

/// An edit made against a revision: whether it changed the document.
type Edit<'a> = Box<dyn Fn(Revision) -> bool + 'a>;

#[test]
fn each_kind_of_edit_is_undone_and_redone_byte_for_byte_and_counted() {
    // On the block and the lug: a pin added before the lug, the lug moved before the block,
    // the pin's radius set, the block removed, and the whole text replaced, laid out by
    // hand. Each undone in turn gives the text before it byte for byte, each redone the one
    // after, and `status` counts each.
    let f = opened(&two_boxes());
    answer(f.sessions.begin(&f.id)).result.unwrap();
    let mut texts = vec![f.text()];
    assert_eq!(texts[0], two_boxes());
    let edits: Vec<(&str, Edit<'_>)> = vec![
        (
            "add_feature before",
            Box::new(|at| {
                answer(
                    f.sessions
                        .add_feature(&f.id, at, Some(id("b0000001")), number(PIN)),
                )
                .result
                .unwrap()
                .changed
            }),
        ),
        (
            "move_feature",
            Box::new(|at| {
                answer(
                    f.sessions
                        .move_feature(&f.id, at, id("b0000001"), Some(id("k7f2qx3m"))),
                )
                .result
                .unwrap()
                .changed
            }),
        ),
        (
            "set_field",
            Box::new(|at| {
                answer(f.sessions.set_field(
                    &f.id,
                    at,
                    id("c0000001"),
                    "radius".to_owned(),
                    number("3"),
                ))
                .result
                .unwrap()
                .changed
            }),
        ),
        (
            "remove_feature",
            Box::new(|at| {
                answer(f.sessions.remove_feature(&f.id, at, id("k7f2qx3m")))
                    .result
                    .unwrap();
                true
            }),
        ),
        (
            "replace_text",
            Box::new(|at| {
                answer(f.sessions.replace_text(&f.id, at, spread(45)))
                    .result
                    .unwrap()
                    .changed
            }),
        ),
    ];
    let orders: [&[&str]; 5] = [
        &["k7f2qx3m", "c0000001", "b0000001"],
        &["b0000001", "k7f2qx3m", "c0000001"],
        &["b0000001", "k7f2qx3m", "c0000001"],
        &["b0000001", "c0000001"],
        &["k7f2qx3m"],
    ];
    for (n, ((what, edit), order)) in edits.iter().zip(orders).enumerate() {
        assert!(edit(f.revision()), "{what}");
        let text = f.text();
        assert_eq!(ids(&text), order, "{what}");
        texts.push(text);
        let t = f.transaction();
        assert_eq!((t.undo, t.redo, t.changed), (n + 1, 0, true), "{what}");
    }
    let radius: serde_json::Value = serde_json::from_str(&texts[3]).unwrap();
    assert_eq!(radius["features"][2]["radius"], 3);
    // The replaced text is held as its document's canonical text (M5.0's A2: every edit,
    // `replace_text` included), not as it was laid out: the same document, other bytes.
    assert_ne!(texts[5], spread(45));
    assert!(!differs(&texts[5], &spread(45)));
    let edits = texts.len() - 1;
    for k in 1..=edits {
        let stepped = f.undo(None).result.unwrap();
        let left = edits - k;
        assert_eq!(
            (
                stepped.restored,
                stepped.undo,
                stepped.redo,
                stepped.dropped
            ),
            (rev(left as u64 + 1), left, k, 0),
            "undo {k}"
        );
        assert_eq!(f.text(), texts[left], "undo {k}");
        let t = f.transaction();
        assert_eq!(
            (t.undo, t.redo, t.changed),
            (left, k, left != 0),
            "undo {k}"
        );
    }
    for (k, text) in texts.iter().enumerate().skip(1) {
        let stepped = f.redo(None).result.unwrap();
        assert_eq!((stepped.undo, stepped.redo), (k, edits - k), "redo {k}");
        assert_eq!(&f.text(), text, "redo {k}");
        assert!(f.transaction().changed, "redo {k}");
    }
    // Undone `to` `begin`'s revision: the file's own bytes, unchanged, and nothing written.
    f.undo(Some(1)).result.unwrap();
    assert_eq!(f.text(), two_boxes());
    let t = f.transaction();
    assert_eq!((t.undo, t.redo, t.changed), (0, edits, false));
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(!committed.written);
    assert_eq!(std::fs::read_to_string(&f.path).unwrap(), two_boxes());
}

// By name against the typed methods, through random runs.

#[derive(Debug, Clone)]
enum Act {
    Set(u32),
    Step { undo: bool, to: Option<u64> },
    Status,
}

/// An act, and the revision it's made against, off by `-1`, `0`, or `1` from the session's.
fn act() -> impl Strategy<Value = (Act, i8)> {
    (
        prop_oneof![
            3 => (10u32..100).prop_map(Act::Set),
            4 => (any::<bool>(), proptest::option::of(0u64..12))
                .prop_map(|(undo, to)| Act::Step { undo, to }),
            1 => Just(Act::Status),
        ],
        prop_oneof![6 => Just(0i8), 1 => Just(-1i8), 1 => Just(1i8)],
    )
}

fn by_name_against_typed(acts: &[(Act, i8)]) {
    use crate::calls::fixture;
    use silico_core::command::wire::{Layout, response_text};
    let by_name = fixture();
    let typed = fixture();
    for f in [&by_name, &typed] {
        f.file("part.silico", &block(40));
        answer(f.sessions.open("part.silico", false))
            .result
            .unwrap();
        answer(f.sessions.begin(&id_s1())).result.unwrap();
    }
    let t = &typed.sessions;
    let s1 = id_s1();
    for (n, (act, off)) in acts.iter().enumerate() {
        let now = t.status(&s1).session.unwrap().revision.number();
        let at = now.saturating_add_signed(i64::from(*off)).max(1);
        let (name, arguments, expected) = match act {
            Act::Set(length) => (
                "set_field",
                format!(
                    r#"{{"session": "s1", "revision": {at}, "feature": "k7f2qx3m", "field": "length", "value": {length}}}"#
                ),
                response_text(
                    &answer(t.set_field(
                        &s1,
                        rev(at),
                        id("k7f2qx3m"),
                        "length".to_owned(),
                        number(&length.to_string()),
                    )),
                    Layout::OneLine,
                ),
            ),
            Act::Step { undo, to } => {
                let name = if *undo { "undo" } else { "redo" };
                let arguments = format!(
                    r#"{{"session": "s1", "revision": {at}, "to": {}}}"#,
                    to.map_or("null".to_owned(), |to| to.to_string())
                );
                // 0 can't be a `Revision`: the reader refuses it, so only by name.
                if *to == Some(0) {
                    let answered = by_name.call(name, &arguments);
                    let envelope: serde_json::Value = serde_json::from_str(&answered.text).unwrap();
                    assert_eq!(
                        first(&envelope),
                        ("invalid_input".to_owned(), "to".to_owned())
                    );
                    continue;
                }
                let to = to.map(rev);
                let response = if *undo {
                    answer(t.undo(&s1, rev(at), to))
                } else {
                    answer(t.redo(&s1, rev(at), to))
                };
                (name, arguments, response_text(&response, Layout::OneLine))
            }
            Act::Status => (
                "status",
                r#"{"session": "s1"}"#.to_owned(),
                response_text(&t.status(&s1), Layout::OneLine),
            ),
        };
        let got = by_name.call(name, &arguments);
        if name == "status" {
            // `disk` is when the background comparison of the file last ended, a diagnostic,
            // so two reads in a row may differ by it alone: the rest is compared.
            assert_eq!(
                without_disk(&got.text),
                without_disk(&expected),
                "act {n}: {name} {arguments}"
            );
        } else {
            assert_eq!(got.text, expected, "act {n}: {name} {arguments}");
        }
    }
    // Both end as one: the same status and text.
    assert_eq!(
        without_disk(&by_name.call("status", r#"{"session": "s1"}"#).text),
        without_disk(&response_text(&t.status(&s1), Layout::OneLine))
    );
}

fn id_s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn undo_redo_and_status_by_name_answer_as_their_typed_methods_through_random_runs(
        acts in proptest::collection::vec(act(), 1..30)
    ) {
        by_name_against_typed(&acts);
    }
}

// Steps racing edits and `status`, replayed against a model.

#[derive(Debug, Clone, Copy)]
enum Did {
    Set(u32),
    Step { undo: bool, to: Option<u64> },
}

#[derive(Debug)]
struct Record {
    did: Did,
    asked: u64,
    /// The envelope's revision: the one made, or the one the session was at when refused.
    at: u64,
    result: Result<Option<Stepped>, (String, String, String)>,
}

/// The unbounded stack as section 7 describes it: each state's length and the revisions it
/// was the session's at.
struct Replay {
    states: Vec<(u32, Vec<u64>)>,
    at: usize,
    revision: u64,
}

impl Replay {
    fn step(&mut self, undo: bool, to: Option<u64>) -> Result<Stepped, &'static str> {
        let target = match (undo, to) {
            (true, None) if self.at == 0 => return Err("nothing"),
            (true, None) => self.at - 1,
            (false, None) if self.at + 1 == self.states.len() => return Err("nothing"),
            (false, None) => self.at + 1,
            (_, Some(to)) => match self.states.iter().position(|s| s.1.contains(&to)) {
                Some(found) if undo && found < self.at => found,
                Some(found) if !undo && found > self.at => found,
                Some(_) => return Err("side"),
                None => return Err("held"),
            },
        };
        self.revision += 1;
        self.at = target;
        self.states[target].1.push(self.revision);
        Ok(Stepped {
            restored: rev(self.states[target].1[0]),
            undo: self.at,
            redo: self.states.len() - self.at - 1,
            dropped: 0,
        })
    }
}

fn kind(message: &str) -> &'static str {
    if message.starts_with("nothing to ") {
        "nothing"
    } else if message.contains("isn't a state of") {
        "held"
    } else if message.contains("isn't before") || message.contains("isn't after") {
        "side"
    } else {
        "other"
    }
}

fn record<T>(
    did: Did,
    asked: u64,
    response: Response<T>,
    stepped: fn(T) -> Option<Stepped>,
) -> Record {
    let at = response.session.as_ref().unwrap().revision.number();
    let result = match response.result {
        Ok(value) => Ok(stepped(value)),
        Err(problems) => {
            let e = &problems.errors()[0];
            Err((
                e.code().as_str().to_owned(),
                e.parameter().unwrap_or("").to_owned(),
                e.message().to_owned(),
            ))
        }
    };
    Record {
        did,
        asked,
        at,
        result,
    }
}

#[test]
fn steps_racing_edits_and_status_from_other_threads_replay_as_one_order() {
    // One thread undoes and redoes, `to` some revision or none, another sets the length
    // (each one never set before), each against the revision `status` last gave it, often
    // stale by then; a third reads `status` throughout. Every answer, sorted by the revision
    // its envelope gives, must replay on the model as one order: each success the model's
    // step, each refusal a conflict when asked against another revision than the
    // session's, else the model's own refusal; and every `status` read's `changed` that of
    // its revision's state, which it sets under the same lock as the revision.
    let f = opened(&block(40));
    answer(f.sessions.begin(&f.id)).result.unwrap();
    let done = AtomicBool::new(false);
    let (steps, sets, statuses) = std::thread::scope(|scope| {
        let stepper = scope.spawn(|| {
            let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
            let mut records = Vec::new();
            for _ in 0..300 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let asked = f.revision().number();
                let undo = seed.is_multiple_of(2);
                let to = seed
                    .is_multiple_of(3)
                    .then(|| 1 + (seed >> 8) % (asked + 1));
                let did = Did::Step { undo, to };
                let response = if undo {
                    answer(f.sessions.undo(&f.id, rev(asked), to.map(rev)))
                } else {
                    answer(f.sessions.redo(&f.id, rev(asked), to.map(rev)))
                };
                records.push(record(did, asked, response, Some));
            }
            records
        });
        let setter = scope.spawn(|| {
            let mut records = Vec::new();
            for i in 0..150u32 {
                let asked = f.revision().number();
                let length = 100 + i;
                let response = answer(f.sessions.set_field(
                    &f.id,
                    rev(asked),
                    id("k7f2qx3m"),
                    "length".to_owned(),
                    number(&length.to_string()),
                ));
                records.push(record(Did::Set(length), asked, response, |_| None));
            }
            records
        });
        let watcher = scope.spawn(|| {
            let mut seen = Vec::new();
            while !done.load(Ordering::Relaxed) {
                let status = f.sessions.status(&f.id);
                let revision = status.session.unwrap().revision.number();
                let transaction = status.result.unwrap().transaction.unwrap();
                seen.push((revision, transaction));
            }
            seen
        });
        let steps = stepper.join().unwrap();
        let sets = setter.join().unwrap();
        done.store(true, Ordering::Relaxed);
        (steps, sets, watcher.join().unwrap())
    });
    let mut all: Vec<Record> = steps.into_iter().chain(sets).collect();
    // A success at r made r, before any refusal while the session was at r.
    all.sort_by_key(|r| (r.at, u8::from(r.result.is_err())));
    let mut model = Replay {
        states: vec![(40, vec![1])],
        at: 0,
        revision: 1,
    };
    let mut changed_at = std::collections::BTreeMap::from([(1u64, false)]);
    let (mut made, mut refused) = (0, 0);
    for r in &all {
        match (&r.result, r.did) {
            (Ok(_), Did::Set(length)) => {
                made += 1;
                assert_eq!(r.asked + 1, r.at, "{r:?}");
                model.revision += 1;
                model.states.truncate(model.at + 1);
                model.states.push((length, vec![model.revision]));
                model.at += 1;
                assert_eq!(model.revision, r.at, "{r:?}");
            }
            (Ok(got), Did::Step { undo, to }) => {
                made += 1;
                assert_eq!(r.asked + 1, r.at, "{r:?}");
                let want = model.step(undo, to);
                assert_eq!(want.as_ref().ok(), got.as_ref(), "{r:?}");
                assert_eq!(model.revision, r.at, "{r:?}");
            }
            (Err((code, parameter, message)), did) => {
                refused += 1;
                assert_eq!(r.at, model.revision, "a refusal at another revision: {r:?}");
                if r.asked != r.at {
                    assert_eq!(
                        (code.as_str(), parameter.as_str()),
                        ("conflict", "revision"),
                        "{r:?}"
                    );
                } else {
                    let Did::Step { undo, to } = did else {
                        panic!("a fresh set refused: {r:?}");
                    };
                    let want = model.step(undo, to);
                    assert_eq!(want.err(), Some(kind(message)), "{r:?}");
                }
            }
        }
        changed_at.insert(model.revision, model.states[model.at].0 != 40);
    }
    assert!(made > 10 && refused > 0, "made {made}, refused {refused}");
    // Each `status` read's `changed` its revision's.
    for (revision, transaction) in &statuses {
        assert_eq!(transaction.base, rev(1));
        assert_eq!(
            Some(&transaction.changed),
            changed_at.get(revision),
            "status at {revision}: {transaction:?}"
        );
    }
    // The end: the model's counts, its text's length, and a commit that writes exactly when
    // it changed.
    let t = f.transaction();
    assert_eq!(
        (t.undo, t.redo, t.changed),
        (
            model.at,
            model.states.len() - model.at - 1,
            model.states[model.at].0 != 40
        )
    );
    let length = model.states[model.at].0;
    let text: serde_json::Value = serde_json::from_str(&f.text()).unwrap();
    assert_eq!(text["features"][0]["length"], length);
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert_eq!(committed.written, length != 40);
}

/// A status envelope's text, its result's `disk` taken out.
fn without_disk(text: &str) -> serde_json::Value {
    let mut value: serde_json::Value = serde_json::from_str(text).unwrap();
    if let Some(result) = value.get_mut("result").and_then(|r| r.as_object_mut()) {
        result.remove("disk");
    }
    value
}

// ---------------------------------------------------------------------------------------
// A `format` undone and redone: what it put into the document goes and comes back with it.

impl Fixture {
    fn set_length(&self, length: &str) {
        answer(self.sessions.set_field(
            &self.id,
            self.revision(),
            id("k7f2qx3m"),
            "length".to_owned(),
            number(length),
        ))
        .result
        .unwrap();
    }

    fn file(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }
}

#[test]
fn a_format_undone_then_edited_back_leaves_nothing_changed_and_the_commit_writes_nothing() {
    // W3's example (#300's follow-up): a file not in canonical form, formatted inside the
    // transaction (from then on compared byte for byte), the format undone, then the
    // length set to 41 and back to 40. The document is the file's again: unchanged, as
    // the oracle says, and the commit writes nothing.
    let f = opened(&block(40));
    answer(f.sessions.begin(&f.id)).result.unwrap();
    let formatted = answer(f.sessions.format(&f.id, None, true)).result.unwrap();
    assert!(formatted.changed);
    assert!(f.transaction().changed);
    assert!(f.undo(None).result.is_ok());
    assert_eq!(f.text(), block(40));
    assert!(!f.transaction().changed);
    f.set_length("41");
    assert!(f.transaction().changed);
    f.set_length("40");
    assert!(!differs(&block(40), &f.text()));
    assert!(!f.transaction().changed);
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(!committed.written);
    assert_eq!(f.file(), block(40));
}

#[test]
fn a_format_undone_then_redone_is_changed_again_and_the_commit_writes_its_text() {
    let f = opened(&block(40));
    answer(f.sessions.begin(&f.id)).result.unwrap();
    answer(f.sessions.format(&f.id, None, true)).result.unwrap();
    let canonical = f.text();
    // The same document, laid out otherwise: a change only a format's byte compare sees.
    assert_ne!(canonical, block(40));
    assert!(!differs(&block(40), &canonical));
    assert!(f.undo(None).result.is_ok());
    assert!(!f.transaction().changed);
    assert!(f.redo(None).result.is_ok());
    assert_eq!(f.text(), canonical);
    assert!(f.transaction().changed);
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(committed.written);
    assert_eq!(f.file(), canonical);
}

#[test]
fn an_edit_after_a_format_keeps_the_format_when_redone() {
    // Formatted, then the length set to 41 and back to 40: the file's document laid out
    // in canonical form, a change the format asked for. Undone to 41 and redone to 40, the
    // state is still the format's: changed, and the commit writes canonical form.
    let f = opened(&block(40));
    answer(f.sessions.begin(&f.id)).result.unwrap();
    answer(f.sessions.format(&f.id, None, true)).result.unwrap();
    f.set_length("41");
    f.set_length("40");
    let canonical = f.text();
    assert_ne!(canonical, block(40));
    assert!(f.transaction().changed);
    assert!(f.undo(None).result.is_ok());
    assert!(f.redo(None).result.is_ok());
    assert_eq!(f.text(), canonical);
    assert!(f.transaction().changed);
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(committed.written);
    assert_eq!(f.file(), canonical);
}

#[test]
fn an_edit_after_a_format_undone_back_to_the_format_is_changed_and_the_commit_writes() {
    // Formatted, then set to 41, then that undone: the format's own state, canonical form
    // of the file's document, which the format asked for: changed, and the commit writes it.
    let f = opened(&block(40));
    answer(f.sessions.begin(&f.id)).result.unwrap();
    answer(f.sessions.format(&f.id, None, true)).result.unwrap();
    let canonical = f.text();
    f.set_length("41");
    assert!(f.undo(None).result.is_ok());
    assert_eq!(f.text(), canonical);
    assert!(f.transaction().changed);
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(committed.written);
    assert_eq!(f.file(), canonical);
}

#[test]
fn a_step_to_a_revision_takes_the_format_in_force_there() {
    // Begun at r0; formatted at r1; set to 41 at r2 and 42 at r3. Undone to r1, the
    // format's: changed (canonical form, byte for byte). Undone to r0, begin's: unchanged,
    // the format no longer in force. Redone to r3: the format in force again, changed.
    let f = opened(&block(40));
    let r0 = f.revision().number();
    answer(f.sessions.begin(&f.id)).result.unwrap();
    answer(f.sessions.format(&f.id, None, true)).result.unwrap();
    let r1 = f.revision().number();
    let canonical = f.text();
    f.set_length("41");
    f.set_length("42");
    let r3 = f.revision().number();
    assert_eq!(r3, r1 + 2);
    assert!(f.undo(Some(r1)).result.is_ok());
    assert_eq!(f.text(), canonical);
    assert!(f.transaction().changed);
    assert!(f.undo(Some(r0)).result.is_ok());
    assert_eq!(f.text(), block(40));
    assert!(!f.transaction().changed);
    assert!(f.redo(Some(r3)).result.is_ok());
    assert!(f.transaction().changed);
    // At 42 either way; then set back to 40 there: the format still in force, so canonical
    // form of the file's document is a change.
    f.set_length("40");
    assert_eq!(f.text(), canonical);
    assert!(f.transaction().changed);
    assert!(answer(f.sessions.commit(&f.id)).result.unwrap().written);
    assert_eq!(f.file(), canonical);
}
