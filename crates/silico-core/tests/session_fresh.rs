//! M4.3: cache control through a session (docs/design/m4.1-command-api.md, section 7, and
//! section 12's M4.3 line). `check` with `fresh` rebuilds the current revision with every
//! step built anew, reading neither the session's engine (its steps' results, the refusals
//! among them, its sketches' solves) nor the cache on disk, yet keeping what it builds in
//! both, so a later rebuild hits it; it tries again a kernel fault the revision kept
//! (decision 32), and its model is the revision's from then on. `cache_clear` removes the
//! entries of the cache beside the session's document, and says how many. Expected values
//! are the fake kernel's call record and the fake solver's, counted by hand from the
//! documents: a box at the origin is one `MakeBox`; a pad of a sketch with no base is its
//! `MakeFaces` and its `Prism`, after one solve of the sketch; a sphere at the origin is one
//! `MakeSphere`; each feature's solid that's made is summarized once.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use silico_core::ErrorCode;
use silico_core::command::{
    CacheCleared, CheckAnswer, CheckModel, ModelChoice, ModelRead, RebuildState, Response,
    Revision, SessionCommand, SessionId,
};

use crate::cache_control::{Fixture, fixture};
use crate::sessions::{HANG, answer};

/// A sketch of a 4 by 2 rectangle on xy, with no constraints, and its pad, 10 long.
const PADDED: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Outline", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 4, "y": 0, "construction": false}, {"id": "pa000003", "x": 4, "y": 2, "construction": false}, {"id": "pa000004", "x": 0, "y": 2, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}, {"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}}"#;

/// A box, 40 by 20 by 10, at the origin.
const BLOCK: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;

/// A sphere of radius 5 at the origin.
const BALL: &str = r#"{"id": "b0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}"#;

/// A document of `features`.
fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

/// The kernel's and the solver's work so far: how far into each record a step begins.
#[derive(Clone, Copy)]
struct Mark {
    calls: usize,
    solves: usize,
}

impl Fixture {
    fn mark(&self) -> Mark {
        Mark {
            calls: self.kernel.calls().len(),
            solves: self.sketches.requests().len(),
        }
    }

    /// The shapes the kernel was asked to make since `mark`, by operation, in order of
    /// name (requests run at once, in any order), and how many solids it summarized.
    fn made_since(&self, mark: Mark) -> (Vec<&'static str>, usize) {
        let calls = self.kernel.calls();
        let since = &calls[mark.calls..];
        let mut made: Vec<&'static str> = since
            .iter()
            .map(|call| call.op.name())
            .filter(|name| !matches!(*name, "Summarize" | "Describe"))
            .collect();
        made.sort_unstable();
        let summarized = since
            .iter()
            .filter(|call| call.op.name() == "Summarize")
            .count();
        (made, summarized)
    }

    /// How many sketches were solved since `mark`.
    fn solved_since(&self, mark: Mark) -> usize {
        self.sketches.requests().len() - mark.solves
    }

    fn check(&self, id: &SessionId) -> Response<CheckAnswer> {
        answer(
            self.sessions
                .check(id, CheckModel::Current { fresh: false }),
        )
    }

    fn fresh(&self, id: &SessionId) -> Response<CheckAnswer> {
        answer(self.sessions.check(id, CheckModel::Current { fresh: true }))
    }
}

/// Each problem a check found: its code and feature.
fn problems(response: &Response<CheckAnswer>) -> Vec<(ErrorCode, Option<String>)> {
    response
        .result
        .as_ref()
        .unwrap()
        .problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature().map(str::to_owned)))
        .collect()
}

/// The model a response read, of the session's first revision, with `fault`'s code and
/// feature if it met one.
fn read_fault(response: &Response<CheckAnswer>) -> Option<(ErrorCode, Option<String>)> {
    let model = response.model.as_ref().unwrap();
    assert_eq!(
        (model.revision, model.of),
        (Revision::OPENED, ModelChoice::Current)
    );
    model
        .fault
        .as_ref()
        .map(|fault| (fault.code(), fault.feature().map(str::to_owned)))
}

#[test]
fn a_fresh_check_builds_every_step_reading_nothing_the_engine_or_the_disk_kept() {
    let f = fixture();
    // The sphere's step is refused for its values (`kernel_failed`): a refusal the engine
    // keeps for the session, and never on disk.
    f.kernel.refuse(|op| {
        (op.name() == "MakeSphere").then(|| {
            silico_core::Error::new(ErrorCode::KernelFailed, "OCCT couldn't make the sphere")
        })
    });
    let text = document(&[PADDED, BLOCK, BALL]);
    let every = ["MakeBox", "MakeFaces", "MakeSphere", "Prism"];
    let failed = vec![(ErrorCode::KernelFailed, Some("b0000001".to_owned()))];
    let a = f.open("a.silico", &text);
    let start = f.mark();
    let first = f.check(&a);
    assert_eq!(problems(&first), failed);
    assert_eq!(read_fault(&first), None);
    // Every step, the pad's and the box's solids summarized, the sketch solved once.
    assert_eq!(f.made_since(start), (every.to_vec(), 2));
    assert_eq!(f.solved_since(start), 1);
    // The same revision again: its model, nothing asked.
    let kept = f.mark();
    assert_eq!(f.check(&a), first);
    assert_eq!(f.made_since(kept), (vec![], 0));
    assert_eq!(f.solved_since(kept), 0);
    // Fresh: every step again, the sphere's kept refusal asked again too, the sketch solved
    // again, though the engine keeps them all and the disk the pad's and the box's.
    let fresh = f.mark();
    let again = f.fresh(&a);
    assert_eq!(problems(&again), failed);
    assert_eq!(read_fault(&again), None);
    assert_eq!(f.made_since(fresh), (every.to_vec(), 2));
    assert_eq!(f.solved_since(fresh), 1);

    // A session of its own on the same steps, in the same directory: a plain check takes
    // all but the sphere's from the disk (its refusal isn't kept there)...
    let b = f.open("b.silico", &text);
    let plain = f.mark();
    assert_eq!(problems(&f.check(&b)), failed);
    assert_eq!(f.made_since(plain), (vec!["MakeSphere"], 0));
    // ...and a fresh check, first in a session with nothing in memory, reads none of them.
    let c = f.open("c.silico", &text);
    let first_fresh = f.mark();
    assert_eq!(problems(&f.fresh(&c)), failed);
    assert_eq!(f.made_since(first_fresh), (every.to_vec(), 2));
    assert_eq!(f.solved_since(first_fresh), 1);
}

#[test]
fn what_a_fresh_check_builds_a_later_rebuild_hits() {
    let f = fixture();
    let text = document(&[PADDED, BLOCK]);
    let a = f.open("a.silico", &text);
    let start = f.mark();
    let fresh = f.fresh(&a);
    assert_eq!(problems(&fresh), []);
    assert_eq!(
        f.made_since(start),
        (vec!["MakeBox", "MakeFaces", "Prism"], 2)
    );
    // In the session: a plain check runs nothing on the kernel.
    let after = f.mark();
    assert_eq!(f.check(&a), fresh);
    assert_eq!(f.made_since(after), (vec![], 0));
    // In another, on the disk alone: nothing on the kernel either, the sketch solved (a
    // solve isn't kept on disk).
    let b = f.open("b.silico", &text);
    let other = f.mark();
    assert_eq!(problems(&f.check(&b)), []);
    assert_eq!(f.made_since(other), (vec![], 0));
    assert_eq!(f.solved_since(other), 1);
}

#[test]
fn a_fresh_check_tries_again_a_fault_the_revision_kept_and_its_model_is_the_revisions() {
    let f = fixture();
    let crashing = Arc::new(AtomicBool::new(true));
    let crashes = Arc::clone(&crashing);
    f.kernel.refuse(move |op| {
        (op.name() == "MakeBox" && crashes.load(Ordering::SeqCst)).then(|| {
            silico_core::Error::new(ErrorCode::WorkerCrashed, "crashed")
                .with_suggestion("change the operation's values")
        })
    });
    let a = f.open("a.silico", &document(&[BLOCK]));
    let start = f.mark();
    let faulted = f.check(&a);
    assert_eq!(problems(&faulted), []);
    assert_eq!(
        read_fault(&faulted),
        Some((ErrorCode::WorkerCrashed, Some("k7f2qx3m".to_owned())))
    );
    assert_eq!(f.made_since(start), (vec!["MakeBox"], 0));
    // Kept: a plain check of the revision asks nothing, and gives the same fault.
    let kept = f.mark();
    assert_eq!(f.check(&a), faulted);
    assert_eq!(f.made_since(kept), (vec![], 0));
    assert_eq!(
        f.sessions.status(&a).result.unwrap().rebuild.state,
        RebuildState::Faulted
    );
    // The worker would build it now: fresh tries it again, and builds it.
    crashing.store(false, Ordering::SeqCst);
    let retried = f.mark();
    let fresh = f.fresh(&a);
    assert_eq!(problems(&fresh), []);
    assert_eq!(
        fresh.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Current,
            fault: None,
        })
    );
    assert_eq!(f.made_since(retried), (vec!["MakeBox"], 1));
    // Its model is the revision's: status, a published read, and a plain check say so,
    // asking nothing more.
    let after = f.mark();
    assert_eq!(
        f.sessions.status(&a).result.unwrap().rebuild.state,
        RebuildState::Done
    );
    let published = answer(f.sessions.check(&a, CheckModel::Published));
    assert_eq!(
        published.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Published,
            fault: None,
        })
    );
    assert_eq!(f.check(&a), fresh);
    assert_eq!(f.made_since(after), (vec![], 0));
}

#[test]
fn a_fresh_check_of_a_document_the_gate_refused_gives_the_gates_problems_and_builds_nothing() {
    let f = fixture();
    let refused = document(&[&BLOCK.replace("\"length\": 40", "\"length\": 0")]);
    let a = f.open("a.silico", &refused);
    let start = f.mark();
    let fresh = f.fresh(&a);
    assert_eq!(fresh.model, None);
    let found = problems(&fresh);
    assert_eq!(
        found,
        [(ErrorCode::InvalidInput, Some("k7f2qx3m".to_owned()))]
    );
    assert_eq!(
        fresh.result.as_ref().unwrap().problems.errors()[0].parameter(),
        Some("length")
    );
    assert_eq!(f.made_since(start), (vec![], 0));
}

#[test]
fn a_fresh_check_is_cancelled_by_name_and_its_rebuild_stopped() {
    let f = fixture();
    let a = f.open("a.silico", &document(&[BLOCK]));
    let hold = f.kernel.hold(|op| op.name() == "MakeBox");
    let running = f.sessions.check(&a, CheckModel::Current { fresh: true });
    assert!(f.kernel.wait_running(1, HANG));
    let status = f.sessions.status(&a).result.unwrap();
    assert_eq!(status.running, Some(SessionCommand::Check));
    assert_eq!(status.rebuild.state, RebuildState::Running);
    let cancelled = f.sessions.cancel(&a, SessionCommand::Check).result.unwrap();
    assert!(cancelled.stopped);
    let answered = answer(running);
    assert_eq!(
        answered.result.unwrap_err().errors()[0].code(),
        ErrorCode::Cancelled
    );
    assert_eq!(
        f.sessions.status(&a).result.unwrap().rebuild.state,
        RebuildState::Stopped
    );
    drop(hold);
    // The next check rebuilds it, and answers.
    assert_eq!(problems(&f.check(&a)), []);
}

#[test]
fn a_fresh_check_supersedes_a_rebuild_running_that_reads_what_was_kept() {
    // A published read starts the revision's rebuild, reading what's kept; a fresh check
    // then starts its own, which answers: the box asked twice, once a rebuild.
    let f = fixture();
    let a = f.open("a.silico", &document(&[BLOCK]));
    let hold = f.kernel.hold(|op| op.name() == "MakeBox");
    let read = answer(f.sessions.check(&a, CheckModel::Published));
    assert_eq!(
        read.result.unwrap_err().errors()[0].parameter(),
        Some("model")
    );
    assert!(f.kernel.wait_running(1, HANG));
    let start = f.mark();
    let fresh = f.sessions.check(&a, CheckModel::Current { fresh: true });
    // The fresh one's box asked, and held in turn; the first superseded (the fake ends a
    // held request of a superseded generation).
    let deadline = std::time::Instant::now() + HANG;
    while boxes(&f) < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "the fresh rebuild asked no box"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    drop(hold);
    let fresh = answer(fresh);
    assert_eq!(problems(&fresh), []);
    assert_eq!(read_fault(&fresh), None);
    assert_eq!(f.made_since(start), (vec!["MakeBox"], 1));
    assert_eq!(boxes(&f), 2);
}

/// How many boxes the kernel has started making.
fn boxes(f: &Fixture) -> usize {
    f.kernel
        .calls()
        .iter()
        .filter(|call| call.op.name() == "MakeBox")
        .count()
}

#[test]
fn cache_clear_removes_the_entries_beside_the_document_and_a_second_removes_none() {
    let f = fixture();
    let a = f.open("a.silico", &document(&[PADDED, BLOCK]));
    assert_eq!(problems(&f.check(&a)), []);
    // An entry a step: the faces, the prism, and the box.
    assert_eq!(f.entries().len(), 3);
    let stray = f.cache().join("notes.txt");
    std::fs::write(&stray, b"mine").unwrap();
    let directory = f.path(".silico-cache").to_str().unwrap().to_owned();
    let cleared = answer(f.sessions.cache_clear(&a));
    assert_eq!(cleared.model, None);
    assert_eq!(cleared.session.as_ref().unwrap().revision, Revision::OPENED);
    assert_eq!(
        cleared.result.unwrap(),
        CacheCleared {
            directory: directory.clone(),
            removed: 3
        }
    );
    assert!(f.entries().is_empty());
    assert_eq!(std::fs::read(&stray).unwrap(), b"mine");
    assert_eq!(
        answer(f.sessions.cache_clear(&a)).result.unwrap(),
        CacheCleared {
            directory,
            removed: 0
        }
    );
    // The session's own model stands: a check of its revision asks nothing.
    let kept = f.mark();
    assert_eq!(problems(&f.check(&a)), []);
    assert_eq!(f.made_since(kept), (vec![], 0));
    // Another session's rebuild misses each entry, and makes it again.
    let b = f.open("b.silico", &document(&[PADDED, BLOCK]));
    let missed = f.mark();
    assert_eq!(problems(&f.check(&b)), []);
    assert_eq!(
        f.made_since(missed),
        (vec!["MakeBox", "MakeFaces", "Prism"], 2)
    );
    assert_eq!(f.entries().len(), 3);
}

#[test]
fn cache_clear_names_the_directory_as_the_sessions_path_does() {
    // The design's example: `bracket.silico`, opened as named from its directory, has its
    // cache at `.silico-cache`. (The working directory is the process's: nextest runs each
    // test in a process of its own.)
    let f = fixture();
    let docs = f.path("");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("bracket.silico"), document(&[BLOCK])).unwrap();
    std::env::set_current_dir(&docs).unwrap();
    let opened = answer(f.sessions.open(Path::new("bracket.silico"), false));
    let s = opened.session.unwrap().id;
    assert_eq!(problems(&f.check(&s)), []);
    assert_eq!(
        answer(f.sessions.cache_clear(&s)).result.unwrap(),
        CacheCleared {
            directory: ".silico-cache".to_owned(),
            removed: 1
        }
    );
    // A path into a subdirectory names it beside that.
    std::fs::create_dir_all(docs.join("parts")).unwrap();
    std::fs::write(docs.join("parts").join("plate.silico"), document(&[BALL])).unwrap();
    let opened = answer(f.sessions.open(Path::new("parts/plate.silico"), false));
    let p = opened.session.unwrap().id;
    assert_eq!(problems(&f.check(&p)), []);
    assert_eq!(
        answer(f.sessions.cache_clear(&p)).result.unwrap(),
        CacheCleared {
            directory: "parts/.silico-cache".to_owned(),
            removed: 1
        }
    );
}

#[test]
fn a_fresh_check_clears_nothing_kept_on_disk_or_in_its_engine() {
    // The pad's prism is refused for its values (`kernel_failed`): a refusal the engine
    // keeps and the disk never does, so only the engine can spare the kernel it; and the
    // sketch's solve is kept only in the engine too. The faces, the box, and the ball are
    // kept on disk.
    let f = fixture();
    f.kernel.refuse(|op| {
        (op.name() == "Prism").then(|| {
            silico_core::Error::new(ErrorCode::KernelFailed, "OCCT couldn't sweep the face")
        })
    });
    let (padded, ball) = (document(&[PADDED, BLOCK]), document(&[BALL]));
    let refused = vec![(ErrorCode::KernelFailed, Some("p0000001".to_owned()))];
    let a = f.open("a.silico", &padded);
    let b = f.open("b.silico", &ball);
    let start = f.mark();
    assert_eq!(problems(&f.check(&a)), refused);
    assert_eq!(problems(&f.check(&b)), []);
    assert_eq!(
        f.made_since(start),
        (vec!["MakeBox", "MakeFaces", "MakeSphere", "Prism"], 2)
    );
    assert_eq!(f.solved_since(start), 1);
    // In a transaction, so its edits are rebuilt only when read: a holds the ball, checked
    // fresh; its engine has published the padded document and the ball, the two models it
    // keeps results for.
    assert!(answer(f.sessions.begin(&a)).result.is_ok());
    let revision = f.sessions.status(&a).session.unwrap().revision;
    let replaced = answer(f.sessions.replace_text(&a, revision, ball.clone()));
    assert!(replaced.result.is_ok(), "{replaced:?}");
    let fresh = f.mark();
    assert_eq!(problems(&f.fresh(&a)), []);
    assert_eq!(f.made_since(fresh), (vec!["MakeSphere"], 1));
    // The disk keeps what every session wrote: a session of its own on the ball, and one on
    // the padded document, take every entry there (but the pad's refusal, never on disk,
    // and the sketch's solve).
    let c = f.open("c.silico", &ball);
    let d = f.open("d.silico", &padded);
    let other = f.mark();
    assert_eq!(problems(&f.check(&c)), []);
    assert_eq!(problems(&f.check(&d)), refused);
    assert_eq!(f.made_since(other), (vec!["Prism"], 0));
    assert_eq!(f.solved_since(other), 1);
    // And a's engine keeps what it built of the padded document: back to it, nothing is
    // asked of the kernel or the solver, the pad's refusal and the solve kept in memory.
    let revision = f.sessions.status(&a).session.unwrap().revision;
    let back = answer(f.sessions.replace_text(&a, revision, padded.clone()));
    assert!(back.result.is_ok(), "{back:?}");
    let kept = f.mark();
    assert_eq!(problems(&f.check(&a)), refused);
    assert_eq!(f.made_since(kept), (vec![], 0));
    assert_eq!(f.solved_since(kept), 0);
    assert!(answer(f.sessions.rollback(&a)).result.is_ok());
}

#[test]
fn a_fresh_check_after_a_rollback_or_a_commit_answers_with_its_own_rebuilds_model() {
    // A rollback and a commit publish a model no rebuild of the session's made (the one
    // `begin` kept, and the commit's, rebound to the saved document), labelled by none. A
    // fresh check of the revision they leave rebuilds it, and its model takes theirs: the
    // kernel asked once, and the published read gives the fresh model. Each published model
    // holds a crash, the fresh one none, so which one a read gives is seen.
    let f = fixture();
    let crashing = Arc::new(AtomicBool::new(true));
    let crashes = Arc::clone(&crashing);
    f.kernel.refuse(move |op| {
        (matches!(op.name(), "MakeBox" | "MakeSphere") && crashes.load(Ordering::SeqCst)).then(
            || {
                silico_core::Error::new(ErrorCode::WorkerCrashed, "crashed")
                    .with_suggestion("change the operation's values")
            },
        )
    });
    let a = f.open("a.silico", &document(&[BLOCK]));
    let faulted = f.check(&a);
    assert_eq!(
        read_fault(&faulted),
        Some((ErrorCode::WorkerCrashed, Some("k7f2qx3m".to_owned())))
    );
    let published = |revision: u64, fault: Option<(ErrorCode, &str)>| {
        let read = answer(f.sessions.check(&a, CheckModel::Published))
            .model
            .unwrap();
        assert_eq!(
            (read.revision, read.of),
            (
                Revision::from_number(revision).unwrap(),
                ModelChoice::Published
            )
        );
        assert_eq!(
            read.fault
                .map(|fault| (fault.code(), fault.feature().unwrap().to_owned())),
            fault.map(|(code, feature)| (code, feature.to_owned()))
        );
    };
    let current = |response: &Response<CheckAnswer>, revision: u64| {
        assert_eq!(
            response.model,
            Some(ModelRead {
                revision: Revision::from_number(revision).unwrap(),
                of: ModelChoice::Current,
                fault: None,
            })
        );
    };

    // Begun and rolled back: revision 2, the model `begin` kept (the crashed box) published.
    assert!(answer(f.sessions.begin(&a)).result.is_ok());
    let rolled = answer(f.sessions.rollback(&a)).result.unwrap();
    assert_eq!(rolled.restored, Revision::OPENED);
    published(2, Some((ErrorCode::WorkerCrashed, "k7f2qx3m")));
    crashing.store(false, Ordering::SeqCst);
    let start = f.mark();
    let fresh = f.fresh(&a);
    assert_eq!(problems(&fresh), []);
    current(&fresh, 2);
    assert_eq!(f.made_since(start), (vec!["MakeBox"], 1));
    published(2, None);
    assert_eq!(
        f.sessions.status(&a).result.unwrap().rebuild.state,
        RebuildState::Done
    );

    // Begun, the ball put in its place, and committed: revision 4, the commit's model (the
    // crashed ball) published.
    crashing.store(true, Ordering::SeqCst);
    assert!(answer(f.sessions.begin(&a)).result.is_ok());
    let replaced = answer(f.sessions.replace_text(
        &a,
        Revision::from_number(2).unwrap(),
        document(&[BALL]),
    ));
    assert!(replaced.result.is_ok(), "{replaced:?}");
    let committed = answer(f.sessions.commit(&a)).result.unwrap();
    assert!(committed.written);
    published(4, Some((ErrorCode::WorkerCrashed, "b0000001")));
    crashing.store(false, Ordering::SeqCst);
    let start = f.mark();
    let fresh = f.fresh(&a);
    assert_eq!(problems(&fresh), []);
    current(&fresh, 4);
    assert_eq!(f.made_since(start), (vec!["MakeSphere"], 1));
    published(4, None);
    assert_eq!(
        f.sessions.status(&a).result.unwrap().rebuild.state,
        RebuildState::Done
    );
}
