//! `cancel` naming `rebuild` (decision 34 as amended 2026-10-31, docs/design/m4.1-command-api.md
//! section 6): whatever `status` reports running, `cancel` names by the key `status` reports it
//! under, and a rebuild and the one command waiting on it stop together, whichever is named.
//! Expected values come from that rule and the design's words: what `status` says, the
//! fake kernel's holds and its record of the boxes asked for, the file's bytes, and the
//! design's messages. Where a race decides the outcome (a rebuild's watcher recording what
//! it came to before or after its waiter wakes), the session is driven by hand so each
//! order runs every time.

use super::*;

/// What `cancel` naming `rebuild` answers when it stops one.
const STOPPED: Cancelled = Cancelled {
    command: Cancellable::Rebuild,
    stopped: true,
};

/// A refused cancel's one error, as its message and candidates: `invalid_input`, naming
/// `command`, as every refused cancel is.
fn refused(response: Response<Cancelled>) -> (String, Vec<String>) {
    let problems = response.result.unwrap_err();
    assert_eq!(problems.total(), 1);
    let error = &problems.errors()[0];
    assert_eq!(
        (error.code(), error.parameter(), error.suggestion()),
        (
            ErrorCode::InvalidInput,
            Some("command"),
            Some("cancel names the command it means; read status to see what's running")
        )
    );
    (error.message().to_owned(), error.candidates().to_vec())
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|&name| name.to_owned()).collect()
}

fn status(f: &Fixture) -> Status {
    f.sessions.status(&f.id).result.unwrap()
}

/// The session's revision, as `status` gives it.
fn revision(f: &Fixture) -> Revision {
    f.sessions.status(&f.id).session.unwrap().revision
}

/// Waits until every rebuild's watcher has ended, so what each came to is recorded.
fn watchers_ended(f: &Fixture) {
    let session = f.session();
    ended(|| all_ended(&lock(&session.rebuilds).watchers));
}

fn is_longer_box(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 40.5)
}

/// The block's length set to 41, against revision `at`.
fn longer_by_field(f: &Fixture, at: Revision) -> Running<FieldSet> {
    f.sessions.set_field(
        &f.id,
        at,
        id("k7f2qx3m"),
        "length".to_owned(),
        crate::document::json::read(b"41").unwrap(),
    )
}

/// The published read the app makes each frame, refused with `message`, nothing published.
fn frame_refused(f: &Fixture, message: &str) {
    let refused = f.sessions.published(&f.id).result.unwrap_err();
    assert_eq!(refused.errors()[0].message(), message);
}

#[test]
fn a_rebuild_a_published_read_started_stops_by_name_and_no_frame_starts_it_again() {
    let f = opened(BOX);
    let hold = f.kernel.hold(is_box);
    // The app's frame: nothing published, but the revision's rebuild started, its box held.
    frame_refused(&f, "session s1 has published no model yet");
    assert!(f.kernel.wait_running(1, WAIT));
    // Nothing waits on it: no command running, no read, the rebuild running.
    let before = status(&f);
    assert_eq!(
        (
            before.running,
            before.reads,
            before.rebuild.revision,
            before.rebuild.state
        ),
        (None, vec![], Revision::OPENED, RebuildState::Running)
    );
    assert_eq!(
        f.sessions
            .cancel(&f.id, Cancellable::Rebuild)
            .result
            .unwrap(),
        STOPPED
    );
    // Its box let go by the stop, never by the test; and once its watcher has recorded what
    // it came to, it's still `stopped`.
    assert!(
        f.kernel.wait_idle(WAIT),
        "the stopped rebuild's box still runs"
    );
    watchers_ended(&f);
    assert_eq!(status(&f).rebuild.state, RebuildState::Stopped);
    // The next frame doesn't start it again: the box asked for once.
    frame_refused(
        &f,
        "session s1 has published no model yet: its rebuild was cancelled",
    );
    assert_eq!(f.made(is_box), 1);
    assert_eq!(status(&f).rebuild.state, RebuildState::Stopped);
    // A check of the current model does, and answers.
    hold.release();
    let checked = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert!(checked.result.unwrap().problems.is_empty());
    assert_eq!(f.made(is_box), 2);
    assert_eq!(status(&f).rebuild.state, RebuildState::Done);
}

#[test]
fn a_check_waiting_on_the_rebuild_cancel_names_answers_cancelled() {
    let f = opened(BOX);
    let _hold = f.kernel.hold(is_box);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    assert!(f.kernel.wait_running(1, WAIT));
    let before = status(&f);
    assert_eq!(
        (before.running, before.rebuild.state),
        (
            Some(RunningCommand::Command(SessionCommand::Check)),
            RebuildState::Running
        )
    );
    assert_eq!(
        f.sessions
            .cancel(&f.id, Cancellable::Rebuild)
            .result
            .unwrap(),
        STOPPED
    );
    // As if it had been named.
    let answered = check.wait();
    assert_eq!(answered.result.unwrap_err().errors(), [cancelled("check")]);
    assert_eq!(answered.model, None);
    assert!(
        f.kernel.wait_idle(WAIT),
        "the stopped rebuild's box still runs"
    );
    watchers_ended(&f);
    assert_eq!(f.made(is_box), 1);
    let after = status(&f);
    assert_eq!(
        (after.running, after.rebuild.state),
        (None, RebuildState::Stopped)
    );
}

#[test]
fn a_commit_waiting_on_the_rebuild_cancel_names_leaves_no_trace_and_the_transaction_open() {
    let f = opened(BOX);
    f.begin();
    // Inside the transaction an edit starts no rebuild.
    let edited = longer_by_field(&f, Revision::OPENED).wait();
    assert_eq!(edited.result.unwrap().commit, None);
    let two = Revision::OPENED.next().unwrap();
    let hold = f.kernel.hold(is_box);
    let commit = f.sessions.commit(&f.id);
    assert!(f.kernel.wait_running(1, WAIT));
    let before = status(&f);
    assert_eq!(
        (
            before.running,
            before.rebuild.revision,
            before.rebuild.state
        ),
        (
            Some(RunningCommand::Command(SessionCommand::Commit)),
            two,
            RebuildState::Running
        )
    );
    assert_eq!(
        f.sessions
            .cancel(&f.id, Cancellable::Rebuild)
            .result
            .unwrap(),
        STOPPED
    );
    let answered = commit.wait();
    assert_eq!(answered.result.unwrap_err().errors(), [cancelled("commit")]);
    assert!(answered.session.unwrap().transaction);
    // Nothing written: the file as it was, the transaction open at the edit's revision.
    assert_eq!(f.file(), BOX);
    let after = status(&f);
    assert_eq!(
        after.transaction.map(|open| open.base),
        Some(Revision::OPENED)
    );
    assert_eq!(revision(&f), two);
    assert!(
        f.kernel.wait_idle(WAIT),
        "the stopped rebuild's box still runs"
    );
    assert_eq!(f.made(is_box), 1);
    // The next commit starts it again, and saves.
    hold.release();
    let committed = f.sessions.commit(&f.id).wait();
    assert!(committed.result.unwrap().written);
    assert!(f.file().contains("\"length\": 41"), "{}", f.file());
    assert_eq!(f.made(is_box), 2);
}

#[test]
fn an_edit_outside_a_transaction_waiting_on_its_candidate_cancel_names_answers_cancelled() {
    let f = opened(BOX);
    // Revision 1 built first, so its own state is `done`.
    let checked = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert!(checked.result.unwrap().problems.is_empty());
    let _hold = f.kernel.hold(is_longer_box);
    let edit = longer_by_field(&f, Revision::OPENED);
    assert!(f.kernel.wait_running(1, WAIT));
    // Its candidate's rebuild is the session's revision's running, as status says.
    let before = status(&f);
    assert_eq!(
        (
            before.running,
            before.rebuild.revision,
            before.rebuild.state
        ),
        (
            Some(RunningCommand::Command(SessionCommand::SetField)),
            Revision::OPENED,
            RebuildState::Running
        )
    );
    assert_eq!(
        f.sessions
            .cancel(&f.id, Cancellable::Rebuild)
            .result
            .unwrap(),
        STOPPED
    );
    let answered = edit.wait();
    assert_eq!(
        answered.result.unwrap_err().errors(),
        [cancelled("set_field")]
    );
    assert_eq!(answered.session.unwrap().revision, Revision::OPENED);
    assert!(
        f.kernel.wait_idle(WAIT),
        "the stopped candidate's box still runs"
    );
    watchers_ended(&f);
    // Never started again: the longer box asked for once. Revision 1 and its own state.
    assert_eq!(f.made(is_longer_box), 1);
    assert_eq!(f.file(), BOX);
    let after = status(&f);
    assert_eq!(
        (
            after.running,
            after.rebuild.revision,
            after.rebuild.state,
            revision(&f)
        ),
        (None, Revision::OPENED, RebuildState::Done, Revision::OPENED)
    );
}

#[test]
fn a_cancel_of_a_rebuild_none_running_or_of_a_command_with_only_the_rebuild_is_refused() {
    let f = opened(BOX);
    // Nothing running at all.
    assert_eq!(
        refused(f.sessions.cancel(&f.id, Cancellable::Rebuild)),
        ("no rebuild is running in session s1".to_owned(), vec![])
    );
    // Only the rebuild a published read started: a cancel of `check` names it.
    let hold = f.kernel.hold(is_box);
    frame_refused(&f, "session s1 has published no model yet");
    assert!(f.kernel.wait_running(1, WAIT));
    assert_eq!(
        refused(f.sessions.cancel(&f.id, SessionCommand::Check)),
        (
            "no check is running in session s1: rebuild is".to_owned(),
            names(&["rebuild"])
        )
    );
    // Let through, it publishes: a cancel that comes then is refused, naming nothing.
    hold.release();
    let checked = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert!(checked.result.unwrap().problems.is_empty());
    assert_eq!(f.made(is_box), 1);
    assert_eq!(status(&f).rebuild.state, RebuildState::Done);
    assert_eq!(
        refused(f.sessions.cancel(&f.id, Cancellable::Rebuild)),
        ("no rebuild is running in session s1".to_owned(), vec![])
    );
}

#[test]
fn a_cancel_of_the_rebuild_while_a_command_runs_with_none_is_refused_naming_the_command() {
    // `text` held as it starts, no rebuild running: refused, naming `text`. Then, a
    // published read's rebuild running beside it: both, as status lists them, the queued
    // command first.
    let f = opened(BOX);
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().gate.shut(started, wait);
    let text = f.sessions.text(&f.id, None);
    was_started.recv_timeout(WAIT).unwrap();
    assert_eq!(
        refused(f.sessions.cancel(&f.id, Cancellable::Rebuild)),
        (
            "no rebuild is running in session s1: text is".to_owned(),
            names(&["text"])
        )
    );
    let _hold = f.kernel.hold(is_box);
    frame_refused(&f, "session s1 has published no model yet");
    assert!(f.kernel.wait_running(1, WAIT));
    assert_eq!(
        refused(f.sessions.cancel(&f.id, SessionCommand::Check)),
        (
            "no check is running in session s1: text, rebuild are".to_owned(),
            names(&["text", "rebuild"])
        )
    );
    // The rebuild stops; `text`, which waits on nothing, is left alone and answers.
    assert_eq!(
        f.sessions
            .cancel(&f.id, Cancellable::Rebuild)
            .result
            .unwrap(),
        STOPPED
    );
    through.send(()).unwrap();
    assert_eq!(text.wait().result.unwrap().text, BOX);
}

#[test]
fn a_published_distance_held_in_the_kernel_survives_a_cancel_of_the_rebuild_and_answers() {
    let f = opened(BOX);
    let faces = || {
        [
            "k7f2qx3m.box.Face1".to_owned(),
            "k7f2qx3m.box.Face2".to_owned(),
        ]
    };
    let checked = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert!(checked.result.unwrap().problems.is_empty());
    // What the fake measures for the pair, unheld: the answer the held read must give.
    let measured = f
        .sessions
        .distance(&f.id, ModelChoice::Published, faces(), None)
        .wait()
        .result
        .unwrap();
    // Revision 2, inside a transaction: no rebuild yet.
    f.begin();
    longer_by_field(&f, Revision::OPENED).wait().result.unwrap();
    let measuring = f.kernel.hold(|op| op.name() == "Distance");
    let distance = f
        .sessions
        .distance(&f.id, ModelChoice::Published, faces(), None);
    assert!(f.kernel.wait_running(1, WAIT));
    // The app's frame starts revision 2's rebuild, its box held; it answers revision 1's.
    let _boxes = f.kernel.hold(is_box);
    let frame = f.sessions.published(&f.id).result.unwrap();
    assert_eq!(frame.revision, Revision::OPENED);
    assert!(f.kernel.wait_running(2, WAIT));
    let before = status(&f);
    assert_eq!(
        (
            before.running,
            before.reads,
            before.rebuild.revision,
            before.rebuild.state
        ),
        (
            None,
            vec![ReadCommand::Distance],
            Revision::OPENED.next().unwrap(),
            RebuildState::Running
        )
    );
    assert_eq!(
        f.sessions
            .cancel(&f.id, Cancellable::Rebuild)
            .result
            .unwrap(),
        STOPPED
    );
    // The read, on a kernel client of its own, is left alone: let through, it answers.
    measuring.release();
    let answered = distance.wait();
    assert_eq!(answered.model.unwrap().revision, Revision::OPENED);
    assert_eq!(answered.result.unwrap(), measured);
    assert!(
        f.kernel.wait_idle(WAIT),
        "the stopped rebuild's box still runs"
    );
    let after = status(&f);
    assert_eq!(
        (after.reads, after.rebuild.state),
        (vec![], RebuildState::Stopped)
    );
    // The box of revision 2 asked for once, the box of revision 1 once before it.
    assert_eq!(f.made(is_longer_box), 1);
}

#[test]
fn the_designs_refusal_is_a_cancel_of_check_while_an_edit_runs_before_its_rebuild_starts() {
    // The design's example (section 7), byte for byte: `set_field` held as it starts, so no
    // rebuild runs, and status reports the edit alone.
    let f = opened(BOX);
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().gate.shut(started, wait);
    let edit = longer_by_field(&f, Revision::OPENED);
    was_started.recv_timeout(WAIT).unwrap();
    let before = status(&f);
    assert_eq!(
        (before.running, before.rebuild.state),
        (
            Some(RunningCommand::Command(SessionCommand::SetField)),
            RebuildState::Idle
        )
    );
    let refused = f
        .sessions
        .cancel(&f.id, SessionCommand::Check)
        .result
        .unwrap_err();
    assert_eq!(refused.total(), 1);
    let text = crate::command::wire::write(
        &crate::command::wire::ToWire::to_wire(&refused.errors()[0]),
        crate::command::wire::Layout::Canonical,
    )
    .unwrap()
        + "\n";
    let designed = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/contract/commands/examples/cancel/refused-not-running.json"),
    )
    .unwrap();
    assert_eq!(text, designed);
    // Stopped before it did anything.
    assert!(
        f.sessions
            .cancel(&f.id, SessionCommand::SetField)
            .result
            .unwrap()
            .stopped
    );
    through.send(()).unwrap();
    assert_eq!(
        edit.wait().result.unwrap_err().errors(),
        [cancelled("set_field")]
    );
    assert_eq!(f.file(), BOX);
}

#[test]
fn a_client_names_the_rebuild_by_its_word_and_reads_the_answer_as_the_decision_writes_it() {
    // Through `call`, as MCP's server sends it: the request's text, the envelope's.
    let f = opened(BOX);
    let cancel = |text: &str| {
        let crate::command::Call::Running(calling) = f.sessions.call("cancel", text) else {
            panic!("cancel is a command");
        };
        calling.wait()
    };
    let request = r#"{"session": "s1", "command": "rebuild"}"#;
    let refused = cancel(request);
    assert!(!refused.ok);
    assert!(
        refused
            .text
            .contains(r#""message": "no rebuild is running in session s1""#),
        "{}",
        refused.text
    );
    let _hold = f.kernel.hold(is_box);
    frame_refused(&f, "session s1 has published no model yet");
    assert!(f.kernel.wait_running(1, WAIT));
    let stopped = cancel(request);
    assert!(stopped.ok, "{}", stopped.text);
    assert!(
        stopped
            .text
            .contains(r#""result": {"command": "rebuild", "stopped": true}"#),
        "{}",
        stopped.text
    );
    assert!(
        f.kernel.wait_idle(WAIT),
        "the stopped rebuild's box still runs"
    );
    assert_eq!(status(&f).rebuild.state, RebuildState::Stopped);
}

#[test]
fn the_command_queued_behind_a_stopped_waiter_starts_the_rebuild_again_and_answers() {
    // Nothing of the stop reaches the commands queued behind the one waiting: the next that
    // needs the current model starts the rebuild again, and a second cancel, with nothing
    // running, is refused.
    let f = opened(BOX);
    let hold = f.kernel.hold(is_box);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    let listed = f
        .sessions
        .elements(&f.id, ModelChoice::Current, None, None, 10);
    assert!(f.kernel.wait_running(1, WAIT));
    assert_eq!(
        f.sessions
            .cancel(&f.id, Cancellable::Rebuild)
            .result
            .unwrap(),
        STOPPED
    );
    assert_eq!(
        check.wait().result.unwrap_err().errors(),
        [cancelled("check")]
    );
    // The elements read's own rebuild, its box held again, then let through.
    let deadline = std::time::Instant::now() + WAIT;
    while f.made(is_box) < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "the next command never rebuilt"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(status(&f).rebuild.state, RebuildState::Running);
    hold.release();
    let answered = listed.wait();
    assert_eq!(answered.model.unwrap().revision, Revision::OPENED);
    assert!(answered.result.is_ok());
    assert_eq!(f.made(is_box), 2);
    assert_eq!(status(&f).rebuild.state, RebuildState::Done);
    assert_eq!(
        refused(f.sessions.cancel(&f.id, Cancellable::Rebuild)),
        ("no rebuild is running in session s1".to_owned(), vec![])
    );
}

#[test]
fn an_older_revisions_rebuild_status_doesnt_report_is_not_the_rebuild_cancel_names() {
    // The rule is what status reports: revision 1's rebuild runs (a frame started it, its
    // box held), then an edit inside a transaction makes revision 2, whose rebuild nothing
    // has started. Status reports revision 2's, `none`, so a cancel of the rebuild is
    // refused; the next frame starts revision 2's, which supersedes revision 1's, and that
    // one is the rebuild running.
    let f = opened(BOX);
    let _hold = f.kernel.hold(is_box);
    frame_refused(&f, "session s1 has published no model yet");
    assert!(f.kernel.wait_running(1, WAIT));
    f.begin();
    longer_by_field(&f, Revision::OPENED).wait().result.unwrap();
    let two = Revision::OPENED.next().unwrap();
    let before = status(&f);
    assert_eq!(
        (before.rebuild.revision, before.rebuild.state),
        (two, RebuildState::Idle)
    );
    assert_eq!(
        refused(f.sessions.cancel(&f.id, Cancellable::Rebuild)),
        ("no rebuild is running in session s1".to_owned(), vec![])
    );
    frame_refused(&f, "session s1 has published no model yet");
    assert_eq!(status(&f).rebuild.state, RebuildState::Running);
    assert_eq!(
        f.sessions
            .cancel(&f.id, Cancellable::Rebuild)
            .result
            .unwrap(),
        STOPPED
    );
    assert!(
        f.kernel.wait_idle(WAIT),
        "a stopped rebuild's box still runs"
    );
    let after = status(&f);
    assert_eq!(
        (after.rebuild.revision, after.rebuild.state),
        (two, RebuildState::Stopped)
    );
}

// ---- Races, each order run by hand ----

/// Revision 1's rebuild started on `session`'s engine, as `start_unless_newer` starts it,
/// but with no thread watching it: the test says when it's recorded ([`watch`]).
fn started_unwatched(
    session: &Arc<Session<FakeClient>>,
    text: &str,
) -> (Label, crate::recompute::Run) {
    let mut rebuilds = lock(&session.rebuilds);
    let label = rebuilds.numbered(Label::new(Revision::OPENED, Reached::default()));
    let run = rebuilds
        .engine
        .begin(checked(text), crate::recompute::Reads::Kept);
    rebuilds.running = Some(label.clone());
    (label, run)
}

#[test]
fn a_rebuild_stopped_after_its_engine_published_but_before_its_watcher_recorded_publishes_nothing()
{
    // The cancel finds it running (its watcher hasn't recorded), so it says `stopped`; its
    // engine had published. Recorded after, its model must not make it `done`: the cancel
    // said it stopped, and status, and a published read, must agree.
    let kernel = FakeKernel::new(2).unwrap();
    let (session, _jobs) = session(&kernel);
    let (label, run) = started_unwatched(&session, BOX);
    let progress = run.progress();
    let deadline = std::time::Instant::now() + WAIT;
    while !progress.now().ended {
        assert!(
            std::time::Instant::now() < deadline,
            "the rebuild never ended"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(lock(&session.rebuilds).stop_running(Revision::OPENED));
    watch(&Arc::downgrade(&session), &label, run);
    let rebuilds = lock(&session.rebuilds);
    assert_eq!(
        rebuilds.state(Revision::OPENED).state,
        RebuildState::Stopped
    );
    assert!(rebuilds.published.is_none());
}

#[test]
fn a_waiter_whose_rebuild_was_stopped_and_recorded_before_it_woke_answers_cancelled_never_starting_it_again()
 {
    // A check of the current model waits on revision 1's rebuild, its box held. The cancel
    // stops it under the lock but tells no one; the watcher records the end and tells the
    // waiter, who wakes to find no rebuild running and none failed: it must answer
    // `cancelled`, not start the rebuild again (which would undo the stop silently).
    let kernel = FakeKernel::new(2).unwrap();
    let (session, _jobs) = session(&kernel);
    let hold = kernel.hold(is_box);
    let (said, answered) = mpsc::channel();
    let waiter = {
        let session = Arc::clone(&session);
        std::thread::spawn(move || {
            let stop = Ticket::new(SessionCommand::Check);
            let model = session.model(&stop, Revision::OPENED, &checked(BOX), &Reached::default());
            let _ = said.send(model.map(|_| ()));
        })
    };
    assert!(kernel.wait_running(1, WAIT));
    // Taken once the waiter waits: it holds the lock from the start until its wait.
    assert!(lock(&session.rebuilds).stop_running(Revision::OPENED));
    let answer = answered.recv_timeout(WAIT).unwrap();
    assert_eq!(answer.unwrap_err().errors(), [cancelled("check")]);
    waiter.join().unwrap();
    assert!(
        kernel.wait_idle(WAIT),
        "the stopped rebuild's box still runs"
    );
    assert_eq!(
        kernel
            .calls()
            .iter()
            .filter(|call| is_box(&call.op))
            .count(),
        1
    );
    assert_eq!(
        lock(&session.rebuilds).state(Revision::OPENED).state,
        RebuildState::Stopped
    );
    drop(hold);
}

#[test]
fn an_edits_candidate_stopped_and_recorded_before_its_commit_woke_answers_cancelled_never_starting_it_again()
 {
    // As above, for an edit outside a transaction: its candidate's outcome, superseded, is
    // taken before the commit wakes to look for a stop, so it must pass through the stop
    // before it's matched, or the candidate starts again.
    let kernel = FakeKernel::new(2).unwrap();
    let (session, _jobs) = session(&kernel);
    let hold = kernel.hold(is_box);
    let (said, answered) = mpsc::channel();
    let committing = {
        let session = Arc::clone(&session);
        std::thread::spawn(move || {
            let stop = Ticket::new(SessionCommand::SetField);
            let longer = BOX.replace("\"length\": 40", "\"length\": 41");
            let model =
                session.candidate_model(&stop, Revision::OPENED.next().unwrap(), &checked(&longer));
            let _ = said.send(model.map(|_| ()));
        })
    };
    assert!(kernel.wait_running(1, WAIT));
    // The candidate counts as the session's revision's rebuild running.
    assert!(lock(&session.rebuilds).stop_running(Revision::OPENED));
    let answer = answered.recv_timeout(WAIT).unwrap();
    assert_eq!(answer.unwrap_err().errors(), [cancelled("set_field")]);
    committing.join().unwrap();
    assert!(
        kernel.wait_idle(WAIT),
        "the stopped candidate's box still runs"
    );
    assert_eq!(
        kernel
            .calls()
            .iter()
            .filter(|call| is_box(&call.op))
            .count(),
        1
    );
    drop(hold);
}

#[test]
fn a_cancel_of_the_rebuild_after_a_newer_one_superseded_it_stops_the_newer_one() {
    // A fresh check supersedes the rebuild a published read started; the cancel stops the
    // one running then, the fresh check's, and the check answers `cancelled`. The one it
    // superseded, recorded late, never hides the stop.
    let f = opened(BOX);
    let _hold = f.kernel.hold(is_box);
    frame_refused(&f, "session s1 has published no model yet");
    assert!(f.kernel.wait_running(1, WAIT));
    let fresh = f.sessions.check(&f.id, CheckModel::Current { fresh: true });
    // The first box let go as superseded, the fresh one held.
    let deadline = std::time::Instant::now() + WAIT;
    while f.made(is_box) < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "the fresh check never asked"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(f.kernel.wait_running(1, WAIT));
    assert_eq!(
        f.sessions
            .cancel(&f.id, Cancellable::Rebuild)
            .result
            .unwrap(),
        STOPPED
    );
    assert_eq!(
        fresh.wait().result.unwrap_err().errors(),
        [cancelled("check")]
    );
    assert!(
        f.kernel.wait_idle(WAIT),
        "a stopped rebuild's box still runs"
    );
    watchers_ended(&f);
    assert_eq!(f.made(is_box), 2);
    assert_eq!(status(&f).rebuild.state, RebuildState::Stopped);
    frame_refused(
        &f,
        "session s1 has published no model yet: its rebuild was cancelled",
    );
}
