//! M4.2: stopping a command (docs/design/m4.1-command-api.md, section 6 and `cancel` in
//! section 7; decisions 10 and 34). `cancel` names the command it means and is refused,
//! what's running its candidate, if that isn't what's running; a command stopped before
//! its work left a trace answers `cancelled`, the rebuild it waited on stopped through the
//! pool's superseded path (the fake kernel's held request let go without its hold being
//! released); a command's own handle stops it the same way, even queued. Expected values
//! are the design's messages, the fake kernel's holds and call record, and the file's bytes.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{
    Cancelled, ModelChoice, Progress, PublishedRead, Rebuild, RebuildState, Response, Revision,
    SessionCommand, SessionId, Sessions,
};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Op, Primitive};
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

use crate::sessions::{HANG, answer, id};

const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

/// The design's answer of a command `cancel` stopped (section 7), but for its name.
fn cancelled(command: &str) -> (ErrorCode, String, Option<String>, Option<String>) {
    (
        ErrorCode::Cancelled,
        format!("{command} was cancelled before it finished"),
        None,
        Some(
            "run it again when you want its answer; every rebuild step that finished is kept in the cache"
                .to_owned(),
        ),
    )
}

/// The one error a refused response gives, as [`cancelled`] gives it.
fn refusal<T: std::fmt::Debug>(
    response: &Response<T>,
) -> (ErrorCode, String, Option<String>, Option<String>) {
    crate::sessions::refusal(response)
}

struct Fixture {
    _dir: TempDir,
    kernel: Arc<FakeKernel>,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
    path: PathBuf,
}

/// Sessions on a fake kernel, with [`BOX`] in `a.silico` open as s1.
fn opened() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("a.silico");
    std::fs::write(&path, BOX).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    Fixture {
        _dir: dir,
        kernel,
        sessions,
        id,
        path,
    }
}

fn is_box(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
}

impl Fixture {
    fn boxes(&self) -> usize {
        self.kernel.calls().iter().filter(|c| is_box(&c.op)).count()
    }

    fn file(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }

    fn revision(&self) -> Revision {
        self.sessions.status(&self.id).session.unwrap().revision
    }

    fn rebuild(&self) -> RebuildState {
        let rebuild = self.sessions.status(&self.id).result.unwrap().rebuild;
        assert_eq!(rebuild.revision, self.revision());
        rebuild.state
    }

    /// The block's length set to 41, against revision 1: outside a transaction, saved.
    fn longer(&self) -> silico_core::command::Running<silico_core::command::FieldSet> {
        self.sessions.set_field(
            &self.id,
            Revision::OPENED,
            id("k7f2qx3m"),
            "length".to_owned(),
            silico_core::document::json::read(b"41").unwrap(),
        )
    }

    /// The fake's box held until the hold is dropped; and `running` started and waited for
    /// until the box is held.
    fn running_held<T>(&self, running: impl FnOnce() -> T) -> (silico_core::fake::Hold, T) {
        let hold = self.kernel.hold(is_box);
        let running = running();
        assert!(self.kernel.wait_running(1, HANG));
        (hold, running)
    }
}

#[test]
fn cancel_naming_the_running_check_stops_its_rebuild_and_the_check_answers_cancelled() {
    let f = opened();
    let (hold, check) = f.running_held(|| f.sessions.check(&f.id, ModelChoice::Current));
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!(status.running, Some(SessionCommand::Check));
    // One box, one step, held: none done, the box's running.
    assert_eq!(
        status.rebuild,
        Rebuild {
            revision: Revision::OPENED,
            state: RebuildState::Running,
            progress: Some(Progress {
                done: 0,
                steps: 1,
                running: Some("k7f2qx3m box".to_owned()),
            }),
        }
    );
    let stop = f.sessions.cancel(&f.id, SessionCommand::Check);
    assert_eq!(stop.session.unwrap().id, f.id);
    assert_eq!(
        stop.result.unwrap(),
        Cancelled {
            command: SessionCommand::Check,
            stopped: true
        }
    );
    // Its box never let go by the test: the cancel stopped it, as a superseded request is.
    let stopped = answer(check);
    assert!(
        f.kernel.wait_idle(HANG),
        "the cancelled check's box still runs"
    );
    assert_eq!(refusal(&stopped), cancelled("check"));
    assert_eq!(stopped.model, None);
    assert_eq!(stopped.session.unwrap().revision, Revision::OPENED);
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!(status.running, None);
    assert_eq!(status.rebuild.state, RebuildState::Stopped);
    // The app's frame doesn't start it again: nothing's published, and nothing asked.
    let (code, message, parameter, _) = refusal(&f.sessions.published(&f.id));
    assert_eq!(
        (code, message.as_str(), parameter.as_deref()),
        (
            ErrorCode::InvalidInput,
            "session s1 has published no model yet: its rebuild was cancelled",
            Some("model")
        )
    );
    assert_eq!(f.boxes(), 1);
    assert_eq!(f.rebuild(), RebuildState::Stopped);
    // The next check does, the box made again (a stopped step isn't kept).
    hold.release();
    let checked = answer(f.sessions.check(&f.id, ModelChoice::Current));
    assert!(checked.result.unwrap().problems.is_empty());
    assert_eq!(f.boxes(), 2);
    assert_eq!(f.rebuild(), RebuildState::Done);
}

#[test]
fn a_checks_own_handle_stops_it_as_cancel_does() {
    // MCP's cancellation (decision 10), the Rust handle's.
    let f = opened();
    let (_hold, check) = f.running_held(|| f.sessions.check(&f.id, ModelChoice::Current));
    assert!(check.cancel());
    // Asked again, it's still stopped.
    assert!(check.cancel());
    assert_eq!(refusal(&answer(check)), cancelled("check"));
    assert_eq!(f.rebuild(), RebuildState::Stopped);
}

#[test]
fn cancel_naming_another_command_is_refused_with_whats_running_as_the_candidate() {
    let f = opened();
    // The edit, outside a transaction, waits for its commit's rebuild, its box held.
    let (_hold, edit) = f.running_held(|| f.longer());
    // The design's example, word for word.
    let refused = f.sessions.cancel(&f.id, SessionCommand::Check);
    let problems = refused.result.unwrap_err();
    assert_eq!(problems.total(), 1);
    let error = &problems.errors()[0];
    assert_eq!(
        (
            error.code(),
            error.message(),
            error.parameter(),
            error.suggestion(),
            error.candidates()
        ),
        (
            ErrorCode::InvalidInput,
            "no check is running in session s1: set_field is",
            Some("command"),
            Some("cancel names the command it means; read status to see what's running"),
            &["set_field".to_owned()][..]
        )
    );
    // The edit runs on, its commit's rebuild of the block 41 long, one step, held: none
    // done, the block's running, shown as the session's revision's until the edit saves.
    // Then it's stopped by name: it saved nothing, so it left no trace.
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!(status.running, Some(SessionCommand::SetField));
    assert_eq!(status.rebuild.revision, Revision::OPENED);
    assert_eq!(
        status.rebuild.progress,
        Some(Progress {
            done: 0,
            steps: 1,
            running: Some("k7f2qx3m box".to_owned()),
        })
    );
    assert!(
        f.sessions
            .cancel(&f.id, SessionCommand::SetField)
            .result
            .unwrap()
            .stopped
    );
    let stopped = answer(edit);
    assert_eq!(refusal(&stopped), cancelled("set_field"));
    assert_eq!(f.file(), BOX);
    assert_eq!(f.revision(), Revision::OPENED);
    assert_eq!(
        answer(f.sessions.text(&f.id, None)).result.unwrap().text,
        BOX
    );
}

#[test]
fn a_cancel_of_nothing_running_or_that_comes_late_is_refused_and_stops_nothing_after() {
    let f = opened();
    let (code, message, parameter, _) = refusal(&f.sessions.cancel(&f.id, SessionCommand::Check));
    assert_eq!(
        (code, message.as_str(), parameter.as_deref()),
        (
            ErrorCode::InvalidInput,
            "no check is running in session s1",
            Some("command")
        )
    );
    // The check runs to its end; the edit queued after it runs, its box (41 long) held; a
    // cancel of the check that comes now is refused, naming the edit, and leaves it be.
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 40.5));
    let check = f.sessions.check(&f.id, ModelChoice::Current);
    let edit = f.longer();
    assert!(answer(check).result.unwrap().problems.is_empty());
    assert!(f.kernel.wait_running(1, HANG));
    let late = f.sessions.cancel(&f.id, SessionCommand::Check);
    assert_eq!(
        late.result.unwrap_err().errors()[0].candidates(),
        ["set_field".to_owned()]
    );
    hold.release();
    // Saved, 41 long.
    let edited = answer(edit);
    assert!(
        edited.result.unwrap().commit.unwrap().written,
        "{:?}",
        f.file()
    );
    assert!(f.file().contains("\"length\": 41"));
}

#[test]
fn a_cancel_stops_the_running_check_and_never_the_edit_queued_after_it() {
    let f = opened();
    let (hold, check) = f.running_held(|| f.sessions.check(&f.id, ModelChoice::Current));
    let edit = f.longer();
    assert!(
        f.sessions
            .cancel(&f.id, SessionCommand::Check)
            .result
            .unwrap()
            .stopped
    );
    assert_eq!(refusal(&answer(check)), cancelled("check"));
    // The edit's turn: its own rebuild's box held, then let go, and it's saved.
    hold.release();
    let edited = answer(edit);
    assert!(edited.result.unwrap().commit.unwrap().written);
    assert_eq!(f.revision(), Revision::OPENED.next().unwrap());
}

#[test]
fn an_edit_cancelled_by_its_handle_while_queued_never_runs() {
    let f = opened();
    let (hold, check) = f.running_held(|| f.sessions.check(&f.id, ModelChoice::Current));
    let edit = f.longer();
    assert!(edit.cancel());
    // Changed outside the session meanwhile: a command that ran would reload it first.
    let outside = BOX.replace("\"Block\"", "\"Plate\"");
    std::fs::write(&f.path, &outside).unwrap();
    hold.release();
    assert!(answer(check).result.unwrap().problems.is_empty());
    let stopped = answer(edit);
    assert_eq!(refusal(&stopped), cancelled("set_field"));
    let session = stopped.session.unwrap();
    assert_eq!(
        (session.revision, session.reloaded),
        (Revision::OPENED, false)
    );
    assert_eq!(f.revision(), Revision::OPENED);
    assert_eq!(f.file(), outside);
    // Only the check's box was made: the edit's commit never rebuilt.
    assert_eq!(f.boxes(), 1);
}

#[test]
fn a_commit_cancelled_while_it_waits_for_its_rebuild_leaves_the_transaction_open() {
    let f = opened();
    answer(f.sessions.begin(&f.id)).result.unwrap();
    let edited = answer(f.longer());
    assert_eq!(edited.result.unwrap().commit, None);
    let (_hold, commit) = f.running_held(|| f.sessions.commit(&f.id));
    assert!(
        f.sessions
            .cancel(&f.id, SessionCommand::Commit)
            .result
            .unwrap()
            .stopped
    );
    let stopped = answer(commit);
    assert_eq!(refusal(&stopped), cancelled("commit"));
    // Nothing written, the transaction still open at the edit's revision.
    assert!(stopped.session.unwrap().transaction);
    assert_eq!(f.file(), BOX);
    let status = f.sessions.status(&f.id);
    assert_eq!(
        status.result.unwrap().transaction.map(|begun| begun.base),
        Some(Revision::OPENED)
    );
    assert_eq!(
        status.session.unwrap().revision,
        Revision::OPENED.next().unwrap()
    );
}

#[test]
fn a_published_read_is_stopped_by_name_and_answers_cancelled() {
    let f = opened();
    assert!(
        answer(f.sessions.check(&f.id, ModelChoice::Current))
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let (go, wait) = mpsc::channel::<()>();
    let (started, was_started) = mpsc::channel::<bool>();
    let (seen, saw) = mpsc::channel::<bool>();
    let read = f.sessions.read_model(
        &f.id,
        PublishedRead::Distance,
        ModelChoice::Published,
        move |read| {
            started.send(read.stopped()).unwrap();
            wait.recv().unwrap();
            seen.send(read.stopped()).unwrap();
            Ok(read.model_read().revision)
        },
    );
    // Running beside the queue: listed by status until it answers.
    assert!(!was_started.recv_timeout(HANG).unwrap());
    let reads = || f.sessions.status(&f.id).result.unwrap().reads;
    assert_eq!(reads(), [PublishedRead::Distance]);
    let stop = f
        .sessions
        .cancel(&f.id, SessionCommand::Distance)
        .result
        .unwrap();
    assert_eq!(
        stop,
        Cancelled {
            command: SessionCommand::Distance,
            stopped: true
        }
    );
    go.send(()).unwrap();
    // The read saw it was stopped; whatever it gave, it answers cancelled.
    assert!(saw.recv_timeout(HANG).unwrap());
    let stopped = answer(read);
    assert_eq!(refusal(&stopped), cancelled("distance"));
    assert_eq!(stopped.model, None);
    assert!(reads().is_empty());
}

#[test]
fn a_stopper_cancels_the_command_from_another_thread_while_its_handle_waits() {
    // MCP's server waits on a request's handle, and is told it was cancelled elsewhere.
    fn shared<T: Clone + Send + Sync + 'static>(_: &T) {}
    let f = opened();
    let (_hold, check) = f.running_held(|| f.sessions.check(&f.id, ModelChoice::Current));
    let stopper = check.stopper();
    shared(&stopper);
    let cancelling = std::thread::spawn(move || {
        // Most likely once the handle is waiting: either way, the cancel stops the check.
        std::thread::sleep(std::time::Duration::from_millis(20));
        stopper.cancel()
    });
    let stopped = answer(check);
    assert!(cancelling.join().unwrap());
    assert_eq!(refusal(&stopped), cancelled("check"));
    assert!(
        f.kernel.wait_idle(HANG),
        "the cancelled check's box still runs"
    );
    assert_eq!(f.rebuild(), RebuildState::Stopped);
}

#[test]
fn close_waits_for_the_command_ahead_of_it_and_cancels_nothing() {
    // Commands run in turn, each to its end (section 3): stopping one is said by name.
    let f = opened();
    let (hold, check) = f.running_held(|| f.sessions.check(&f.id, ModelChoice::Current));
    let close = f.sessions.close(&f.id);
    let close = match close.wait_timeout(std::time::Duration::from_millis(50)) {
        Ok(response) => panic!("close answered before the check: {response:?}"),
        Err(close) => close,
    };
    hold.release();
    assert!(answer(check).result.unwrap().problems.is_empty());
    answer(close).result.unwrap();
}

#[test]
fn an_edit_cancelled_while_its_commit_rebuilds_stops_that_rebuild() {
    // An edit outside a transaction rebuilds its candidate for its commit, which status
    // gives as the current revision's rebuild running; a cancel stops it, its box let go
    // without the test releasing its hold, and, the commit's own, it leaves no end behind.
    let f = opened();
    let (_hold, edit) = f.running_held(|| f.longer());
    assert_eq!(f.rebuild(), RebuildState::Running);
    assert!(
        f.sessions
            .cancel(&f.id, SessionCommand::SetField)
            .result
            .unwrap()
            .stopped
    );
    assert_eq!(refusal(&answer(edit)), cancelled("set_field"));
    assert!(
        f.kernel.wait_idle(HANG),
        "the cancelled edit's box still runs"
    );
    assert_eq!(f.rebuild(), RebuildState::Idle);
    assert_eq!((f.revision(), f.file()), (Revision::OPENED, BOX.to_owned()));
}

#[test]
fn cancel_stops_every_read_of_its_name_and_its_refusal_names_each_command_running() {
    let f = opened();
    assert!(
        answer(f.sessions.check(&f.id, ModelChoice::Current))
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    // Two published reads of one name, each waiting for a go.
    let (go, wait) = mpsc::channel::<()>();
    let wait = Arc::new(Mutex::new(wait));
    let (started, was_started) = mpsc::channel::<()>();
    let read = || {
        let (wait, started) = (Arc::clone(&wait), started.clone());
        f.sessions.read_model(
            &f.id,
            PublishedRead::Distance,
            ModelChoice::Published,
            move |read| {
                started.send(()).unwrap();
                wait.lock().unwrap().recv().unwrap();
                Ok(read.model_read().revision)
            },
        )
    };
    let (one, two) = (read(), read());
    was_started.recv_timeout(HANG).unwrap();
    was_started.recv_timeout(HANG).unwrap();
    // And the queued command, an edit whose commit's box is held.
    let (hold, edit) = f.running_held(|| f.longer());
    // A command none is: the queued command, then each read, as status lists them.
    let refused = f.sessions.cancel(&f.id, SessionCommand::Check);
    let problems = refused.result.unwrap_err();
    let error = &problems.errors()[0];
    assert_eq!(
        (error.message(), error.candidates()),
        (
            "no check is running in session s1: set_field, distance, distance are",
            // Names to pick from: each once, the queued command first.
            &["set_field".to_owned(), "distance".to_owned()][..]
        )
    );
    // One cancel stops both reads, and not the edit.
    assert_eq!(
        f.sessions
            .cancel(&f.id, SessionCommand::Distance)
            .result
            .unwrap(),
        Cancelled {
            command: SessionCommand::Distance,
            stopped: true
        }
    );
    go.send(()).unwrap();
    go.send(()).unwrap();
    assert_eq!(refusal(&answer(one)), cancelled("distance"));
    assert_eq!(refusal(&answer(two)), cancelled("distance"));
    hold.release();
    assert!(answer(edit).result.unwrap().commit.unwrap().written);
}

#[test]
fn status_says_none_of_a_revision_nothing_rebuilt_or_whose_document_the_gate_refused() {
    let f = opened();
    assert_eq!(f.rebuild(), RebuildState::Idle);
    let flat = f.path.with_file_name("flat.silico");
    std::fs::write(&flat, BOX.replace("\"length\": 40", "\"length\": 0")).unwrap();
    let s = answer(f.sessions.open(&flat, false)).session.unwrap().id;
    // Its check gives the gate's problems, and rebuilds nothing.
    assert!(
        !answer(f.sessions.check(&s, ModelChoice::Current))
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let status = f.sessions.status(&s).result.unwrap();
    assert!(!status.gated);
    assert_eq!(status.rebuild.state, RebuildState::Idle);
    // The design's words for each.
    assert_eq!(
        [
            RebuildState::Idle,
            RebuildState::Running,
            RebuildState::Done,
            RebuildState::Faulted,
            RebuildState::Stopped
        ]
        .map(RebuildState::as_str),
        ["none", "running", "done", "faulted", "stopped"]
    );
}

#[test]
fn a_command_that_has_answered_is_past_a_cancel_whether_it_ran_or_was_refused() {
    // Its answer settles its stop: a check, which leaves no trace of its own, and an open
    // refused before it opened, are each past a cancel once they've answered.
    let f = opened();
    let check = f.sessions.check(&f.id, ModelChoice::Current);
    let stopper = check.stopper();
    assert!(answer(check).result.unwrap().problems.is_empty());
    assert!(!stopper.cancel());
    let opening = f
        .sessions
        .open(f.path.with_file_name("missing.silico"), false);
    let stopper = opening.stopper();
    assert!(answer(opening).result.is_err());
    assert!(!stopper.cancel());
}
