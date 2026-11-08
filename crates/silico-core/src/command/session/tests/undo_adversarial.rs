//! Hostile runs of `undo` and `redo` through a session's own commands (break-it, PR #273),
//! where only this crate can reach: the stack's bound at nothing, so every edit and every
//! step past the third state drops one; thousands of edits, undos and redos at a bound of
//! two states, the stack's count checked after each against a recount from what it says
//! is left; an undo while the rebuild of the state it undoes is held in the kernel, and
//! published reads from another thread racing undos and redos; and cancels and `status`
//! from other threads during each step. Expected values: the texts written out by hand, the
//! stack's bytes by A2's count (the box's file 212, each state of it 174), and each
//! revision's text as the session was given it when the revision was made.

use super::*;
use crate::document::json::Node;
use std::collections::BTreeMap;

/// The box's file, as [`BOX`] holds it: what the stack counts for `begin`'s state.
const BEGIN_BYTES: usize = 212;

/// What the stack counts for a state of [`canonical`] with a two-digit length: its list of
/// one chunk (24 and 8) and the chunk (16, and its four spaces of indent and 122 bytes of
/// feature).
const STATE_BYTES: usize = 24 + 8 + 16 + 4 + 122;

/// [`BOX`] `length` long, in canonical form: its one feature on one line, written by hand
/// from M2.1's layout.
fn canonical(length: u32) -> String {
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n    {{\"id\": \"k7f2qx3m\", \"type\": \"box\", \"v\": 1, \"name\": \"Block\", \"position\": [0, 0, 0], \"length\": {length}, \"width\": 20, \"height\": 10}}\n  ]\n}}\n"
    )
}

/// [`BOX`] opened, each transaction's undo stack bounded by `bound` bytes.
fn bounded(bound: usize) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("a.silico");
    std::fs::write(&path, BOX).unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let solver = crate::fake::FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    )
    .checking_disk_every(Duration::ZERO)
    .bounding_undo(bound);
    let id = sessions.open(&path, false).wait().session.unwrap().id;
    Fixture {
        _dir: dir,
        kernel,
        sessions,
        id,
        path,
    }
}

fn revision_of(f: &Fixture) -> Revision {
    f.session().held.read().revision
}

fn text_of(f: &Fixture) -> String {
    f.sessions.text(&f.id, None).wait().result.unwrap().text
}

/// The open transaction's stack: its bytes, the edits it says are out of undo's reach, and
/// those left to undo and to redo.
fn stack_of(f: &Fixture) -> (usize, usize, usize, usize) {
    let session = f.session();
    let held = session.held.read();
    let stack = &held.transaction.as_ref().unwrap().undo;
    (
        stack.bytes(),
        stack.dropped(),
        stack.undo_left(),
        stack.redo_left(),
    )
}

/// The block's length set to `length` against the session's revision: the new revision.
fn set_length(f: &Fixture, length: u32) -> Revision {
    let value: Node = crate::document::json::read(length.to_string().as_bytes()).unwrap();
    let response = f
        .sessions
        .set_field(
            &f.id,
            revision_of(f),
            id("k7f2qx3m"),
            "length".to_owned(),
            value,
        )
        .wait();
    assert!(response.result.unwrap().changed);
    response.session.unwrap().revision
}

fn undo_of(f: &Fixture, to: Option<u64>) -> Response<Stepped> {
    f.sessions
        .undo(&f.id, revision_of(f), to.map(Revision))
        .wait()
}

fn redo_of(f: &Fixture, to: Option<u64>) -> Response<Stepped> {
    f.sessions
        .redo(&f.id, revision_of(f), to.map(Revision))
        .wait()
}

/// A step's result and the revision its envelope gives.
#[track_caller]
fn stepped_of(response: Response<Stepped>) -> (Stepped, Revision) {
    let revision = response.session.as_ref().unwrap().revision;
    (response.result.unwrap(), revision)
}

fn stepped_as(restored: u64, undo: usize, redo: usize, dropped: usize) -> Stepped {
    Stepped {
        restored: Revision(restored),
        undo,
        redo,
        dropped,
    }
}

/// A refused step's one error's message and cause, and the revision its envelope gives.
#[track_caller]
fn refused_with(response: Response<Stepped>) -> (String, Option<String>, Revision) {
    let revision = response.session.as_ref().unwrap().revision;
    let problems = response.result.unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    let error = &problems.errors()[0];
    (
        error.message().to_owned(),
        error.cause().map(str::to_owned),
        revision,
    )
}

/// The canonical text of the document `model` was built from.
fn built_text(model: &Model) -> String {
    crate::document::write(model.document()).unwrap()
}

#[test]
fn at_a_bound_of_nothing_every_step_past_the_third_state_drops_the_oldest_even_a_redo() {
    // A2's rules at a bound nothing fits under: `begin`'s bytes are counted and never
    // dropped, an edit never drops the state before it, and no step drops the session's.
    let f = bounded(0);
    f.sessions.begin(&f.id).wait().result.unwrap();
    assert_eq!(set_length(&f, 41), Revision(2));
    assert_eq!(stack_of(&f), (BEGIN_BYTES, 0, 1, 0));
    assert_eq!(set_length(&f, 42), Revision(3));
    // 41's state held now: past the bound, but it's the one before the edit.
    assert_eq!(stack_of(&f), (BEGIN_BYTES + STATE_BYTES, 0, 2, 0));
    assert_eq!(set_length(&f, 43), Revision(4));
    // 41's dropped (the first drop: 2), 42's kept (the one before 43's).
    assert_eq!(stack_of(&f), (BEGIN_BYTES + STATE_BYTES, 2, 1, 0));
    // One undo, to 42's: 43's held for redo, 42's no longer counted.
    assert_eq!(
        stepped_of(undo_of(&f, None)),
        (stepped_as(3, 0, 1, 0), Revision(5))
    );
    assert_eq!(text_of(&f), canonical(42));
    assert_eq!(stack_of(&f), (BEGIN_BYTES + STATE_BYTES, 2, 0, 1));
    let (message, _, revision) = refused_with(undo_of(&f, None));
    assert_eq!(message, "nothing to undo in session s1's transaction");
    assert_eq!(revision, Revision(5));
    // The redo makes 43's the session's and 42's held again, past the bound: 42's, the
    // oldest after `begin`'s, is dropped (one more), so nothing is left to undo.
    assert_eq!(
        stepped_of(redo_of(&f, None)),
        (stepped_as(4, 0, 0, 1), Revision(6))
    );
    assert_eq!(text_of(&f), canonical(43));
    assert_eq!(stack_of(&f), (BEGIN_BYTES, 3, 0, 0));
    let (message, _, _) = refused_with(redo_of(&f, None));
    assert_eq!(message, "nothing to redo in session s1's transaction");
    // An edit: 43's kept, now at revisions 4 and 6, so undo finds it by either.
    assert_eq!(set_length(&f, 44), Revision(7));
    assert_eq!(stack_of(&f), (BEGIN_BYTES + STATE_BYTES, 3, 1, 0));
    let (message, _, _) = refused_with(undo_of(&f, Some(1)));
    assert_eq!(
        message,
        "revision 1 isn't a state of session s1's undo stack that undo can restore: it can restore revisions 4 to 6"
    );
    assert_eq!(
        stepped_of(undo_of(&f, Some(6))),
        (stepped_as(4, 0, 1, 0), Revision(8))
    );
    assert_eq!(text_of(&f), canonical(43));
    // The rollback still reaches `begin`'s, byte for byte, and writes nothing.
    f.sessions.rollback(&f.id).wait().result.unwrap();
    assert_eq!(text_of(&f), BOX);
    assert_eq!(f.file(), BOX);
}

#[test]
fn thousands_of_edits_undos_and_redos_at_a_bound_of_two_states_keep_the_count_and_each_text() {
    // A bound of `begin`'s bytes and two states: every step past it drops. After each step,
    // the stack's count is recounted from what it says it holds (A2: `begin`'s bytes, and
    // each state but `begin`'s and the session's, sharing nothing here, since each edit
    // changes the one chunk); and a step's text is the one its restored revision was made
    // with, as recorded here when it was made.
    let f = bounded(BEGIN_BYTES + 2 * STATE_BYTES);
    f.sessions.begin(&f.id).wait().result.unwrap();
    let mut made: BTreeMap<u64, String> = BTreeMap::from([(1, BOX.to_owned())]);
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut length = 10;
    let mut steps = 0;
    for _ in 0..2_000 {
        let pick = next() % 6;
        let before = revision_of(&f);
        let text_before = text_of(&f);
        let (_, _, undo_before, redo_before) = stack_of(&f);
        match pick {
            0 | 1 => {
                length = 10 + (length + 1 + u32::try_from(next() % 80).unwrap()) % 90;
                if text_before == canonical(length) || (text_before == BOX && length == 40) {
                    continue;
                }
                let revision = set_length(&f, length);
                made.insert(revision.0, canonical(length));
            }
            _ => {
                let undo = pick < 4;
                let to = if next() % 3 == 0 {
                    Some(1 + next() % (before.0 + 2))
                } else {
                    None
                };
                let response = if undo {
                    undo_of(&f, to)
                } else {
                    redo_of(&f, to)
                };
                let revision = response.session.as_ref().unwrap().revision;
                match response.result {
                    Ok(stepped) => {
                        steps += 1;
                        assert_eq!(revision.0, before.0 + 1);
                        let restored = made.get(&stepped.restored.0).unwrap().clone();
                        assert_eq!(text_of(&f), restored, "restored {:?}", stepped.restored);
                        made.insert(revision.0, restored);
                        let (_, _, undo_now, redo_now) = stack_of(&f);
                        assert_eq!((stepped.undo, stepped.redo), (undo_now, redo_now));
                        if to.is_none() {
                            // One step: one fewer to undo (and one more to redo), or the other
                            // way, less any the bound dropped.
                            if undo {
                                assert_eq!(redo_now, redo_before + 1);
                                assert!(undo_now < undo_before);
                            } else {
                                assert_eq!(redo_now + 1, redo_before);
                                assert!(undo_now <= undo_before + 1);
                            }
                        }
                    }
                    Err(problems) => {
                        assert_eq!(problems.errors()[0].code(), ErrorCode::InvalidInput);
                        assert_eq!(revision, before);
                        assert_eq!(text_of(&f), text_before);
                        assert_eq!(stack_of(&f).2, undo_before);
                        assert_eq!(stack_of(&f).3, redo_before);
                    }
                }
            }
        }
        let (bytes, dropped, undo, redo) = stack_of(&f);
        // The states but `begin`'s: those below the session's (the one undo can't reach once
        // any's dropped, and those it can), the session's, and those above it.
        let lowest = usize::from(dropped > 0);
        let at = undo + lowest;
        let others = (at + redo).saturating_sub(usize::from(at > 0));
        assert_eq!(
            bytes,
            BEGIN_BYTES + others * STATE_BYTES,
            "dropped {dropped}, undo {undo}, redo {redo}"
        );
        // Never more than the bound, but the states it may not drop: the session's and,
        // after an edit, the one before it.
        assert!(others <= 2, "{others} states held at a bound of two");
    }
    // The run stepped, and the bound dropped.
    assert!(steps > 100, "{steps}");
    assert!(stack_of(&f).1 > 100, "{:?}", stack_of(&f));
    // A commit writes the session's state, its fingerprint by formula (200 L, 60 L + 400).
    let text = text_of(&f);
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    if text == BOX {
        assert!(!committed.written);
    } else {
        let length: u32 = (10..100).find(|l| canonical(*l) == text).unwrap();
        let file = f.file();
        assert!(
            file.contains(&format!(
                "\"fingerprint\": {{\"volume\": {}, \"area\": {}, \"center\": [{}, 10, 5], \"bbox\": [0, 0, 0, {length}, 20, 10]",
                200 * length,
                60 * length + 400,
                f64::from(length) / 2.0
            )),
            "{file}"
        );
    }
}

#[test]
fn an_undo_while_the_undone_states_rebuild_is_held_answers_and_reads_its_own_model() {
    let f = opened(BOX);
    f.sessions.begin(&f.id).wait().result.unwrap();
    assert_eq!(set_length(&f, 41), Revision(2));
    let hold = f.kernel.hold(is_box);
    // A published read starts revision 2's rebuild (decision 13), which the kernel holds.
    let _ = f.sessions.published(&f.id);
    assert!(f.kernel.wait_running(1, WAIT));
    // The undo neither waits for it nor is refused for it.
    assert_eq!(
        stepped_of(undo_of(&f, None)),
        (stepped_as(1, 0, 1, 0), Revision(3))
    );
    assert_eq!(text_of(&f), BOX);
    // Published reads while it's held: never of a revision past the session's, and never
    // revision 3's yet (nothing built it).
    for _ in 0..50 {
        let response = f.sessions.published(&f.id);
        if let Some(model) = response.model {
            assert!(model.revision <= Revision(3));
            assert_ne!(model.revision, Revision(3));
        }
    }
    drop(hold);
    // The session's model is the box 40 long, not the undone 41.
    let current = f.session().current(&probe()).ok().unwrap();
    assert_eq!(built_text(&current.model), canonical(40));
    // And a published read now answers with revision 3's, the box 40 long; if revision 2's
    // came, it's labelled 2 and is the box 41 long.
    let published = f.sessions.published(&f.id);
    let read = published.model.unwrap();
    let model = published.result.unwrap().model;
    match read.revision {
        Revision(3) => assert_eq!(built_text(&model), canonical(40)),
        Revision(2) => assert_eq!(built_text(&model), canonical(41)),
        other => panic!("published revision {other:?}"),
    }
    // A commit of `begin`'s state writes nothing.
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert!(!committed.written);
    assert_eq!(f.file(), BOX);
}

#[test]
fn published_reads_racing_undos_and_redos_each_read_the_model_of_the_revision_they_name() {
    // A reader thread reads the published model over and over while the session's thread
    // undoes and redoes: every model a read answers with is built from the text its
    // revision was made with (each recorded before the step that makes it is asked for,
    // so a read of it finds it).
    let f = Arc::new(opened(BOX));
    f.sessions.begin(&f.id).wait().result.unwrap();
    let made: Arc<Mutex<BTreeMap<u64, String>>> =
        Arc::new(Mutex::new(BTreeMap::from([(1, canonical(40))])));
    for length in 41..=45 {
        let revision = set_length(&f, length);
        lock(&made).insert(revision.0, canonical(length));
    }
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let f = Arc::clone(&f);
        let made = Arc::clone(&made);
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                let response = f.sessions.published(&f.id);
                if let (Some(read), Ok(published)) = (response.model, response.result) {
                    let expected = lock(&made).get(&read.revision.0).cloned();
                    let built = built_text(&published.model);
                    if expected.as_deref() != Some(built.as_str()) {
                        seen.push((read.revision, built));
                    }
                }
            }
            seen
        })
    };
    // Revisions 7 on: five undos, five redos, again and again. Each step's revision's text
    // is recorded before the step: the one it restores.
    let lengths: Vec<u32> = (40..=45).collect();
    let mut at = 5;
    for round in 0..40 {
        let undo = round % 2 == 0;
        for _ in 0..5 {
            at = if undo { at - 1 } else { at + 1 };
            let next = revision_of(&f).0 + 1;
            lock(&made).insert(next, canonical(lengths[at]));
            let (stepped, revision) = stepped_of(if undo {
                undo_of(&f, None)
            } else {
                redo_of(&f, None)
            });
            assert_eq!(revision.0, next);
            assert_eq!(stepped.undo, at);
        }
        // The current model, waited for, is the state's own.
        let current = f.session().current(&probe()).ok().unwrap();
        assert_eq!(built_text(&current.model), canonical(lengths[at]));
    }
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(reader.join().unwrap(), []);
}

#[test]
fn cancels_and_status_from_other_threads_during_steps_leave_each_step_whole_or_undone() {
    // Another thread cancels whatever undo or redo runs and asks `status`, over and over,
    // while this one steps. Each step answers whole (a new revision, the text its restored
    // revision was made with) or cancelled (the session, its text and its stack as they
    // were); the revisions `status` sees never go back.
    let f = Arc::new(opened(BOX));
    f.sessions.begin(&f.id).wait().result.unwrap();
    let mut made: BTreeMap<u64, String> = BTreeMap::from([(1, BOX.to_owned())]);
    for length in 41..=48 {
        let revision = set_length(&f, length);
        made.insert(revision.0, canonical(length));
    }
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let other = {
        let f = Arc::clone(&f);
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut last = Revision(0);
            let mut backwards = Vec::new();
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                for command in [SessionCommand::Undo, SessionCommand::Redo] {
                    let _ = f.sessions.cancel(&f.id, command);
                }
                let status = f.sessions.status(&f.id);
                let revision = status.session.unwrap().revision;
                if revision < last {
                    backwards.push((last, revision));
                }
                last = revision;
            }
            backwards
        })
    };
    for round in 0..400 {
        let undo = (round / 8) % 2 == 0;
        let before = revision_of(&f);
        let text_before = text_of(&f);
        let stack_before = stack_of(&f);
        let response = if undo {
            undo_of(&f, None)
        } else {
            redo_of(&f, None)
        };
        let revision = response.session.as_ref().unwrap().revision;
        match response.result {
            Ok(stepped) => {
                assert_eq!(revision.0, before.0 + 1);
                let restored = made.get(&stepped.restored.0).unwrap().clone();
                assert_eq!(text_of(&f), restored);
                made.insert(revision.0, restored);
                let (_, _, undo_now, redo_now) = stack_of(&f);
                assert_eq!((stepped.undo, stepped.redo), (undo_now, redo_now));
            }
            Err(problems) => {
                let code = problems.errors()[0].code();
                assert!(
                    matches!(code, ErrorCode::Cancelled | ErrorCode::InvalidInput),
                    "{problems:?}"
                );
                assert_eq!(revision, before);
                assert_eq!(revision_of(&f), before);
                assert_eq!(text_of(&f), text_before);
                assert_eq!(stack_of(&f), stack_before);
            }
        }
    }
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(other.join().unwrap(), []);
    // Whatever was cancelled, a rollback reaches `begin`'s.
    f.sessions.rollback(&f.id).wait().result.unwrap();
    assert_eq!(text_of(&f), BOX);
}

#[test]
fn a_to_from_before_begin_or_from_a_transaction_before_is_not_held() {
    // Revision 2 is an edit committed outside a transaction; 3 to 4 a transaction committed;
    // the transaction begun at 5 holds 5 and its own edits only.
    let f = opened(BOX);
    set_length(&f, 41);
    f.sessions.begin(&f.id).wait().result.unwrap();
    set_length(&f, 42);
    f.sessions.commit(&f.id).wait().result.unwrap();
    let base = revision_of(&f);
    f.sessions.begin(&f.id).wait().result.unwrap();
    let first = set_length(&f, 43);
    set_length(&f, 44);
    for to in [1, 2, 3, first.0 - 1] {
        if to == base.0 {
            continue;
        }
        let (message, cause, revision) = refused_with(undo_of(&f, Some(to)));
        assert_eq!(
            message,
            format!(
                "revision {to} isn't a state of session s1's undo stack that undo can restore: it can restore revisions {} to {}",
                base.0, first.0
            )
        );
        assert_eq!(
            cause.as_deref(),
            Some(
                "the stack dropped its oldest edits past its bound, or the revision is from before the transaction"
            )
        );
        assert_eq!(revision, Revision(first.0 + 1));
    }
    assert_eq!(
        stepped_of(undo_of(&f, Some(base.0))).0,
        stepped_as(base.0, 0, 2, 0)
    );
    assert_eq!(text_of(&f), f.file());
}
