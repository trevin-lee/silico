use super::cancel::{Interrupt, Ticket, Why, cancelled};
use super::holding::Holding;
use super::rebuilds::*;
use super::transaction::*;
use super::types::*;
use super::*;
use crate::command::wire::Named;
use crate::document;
use crate::document::Checked;
use crate::document::Measure;
use crate::fake::{FakeClient, FakeKernel};
use crate::file;
use crate::kernel::{Kernel, KernelRequest, Op, Primitive};
use crate::pool::Outcome;
use crate::recompute::{FeatureOutcome, Model, Recompute, RunOutcome, StepState};
use std::collections::BTreeSet;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::Duration;

const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
const SPHERE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "a91cw0de", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}]}"#;
const WAIT: Duration = Duration::from_secs(60);

/// The stop of a command no one cancels: as a queued command's work is handed its own.
fn probe() -> Arc<Ticket> {
    Ticket::new(SessionCommand::Text)
}

fn checked(text: &str) -> Arc<Checked> {
    Arc::new(Checked::new(document::read(text.as_bytes()).unwrap()).unwrap())
}

/// A session as `start` makes one, at revision 1 of [`BOX`], on its own client of
/// `kernel`; its queue open, though nothing serves it. Edits, which move a session to a
/// new revision, come later: these tests drive `Session::model` at revisions 1 and 2.
fn session(kernel: &FakeKernel) -> (Arc<Session<FakeClient>>, Receiver<Job<FakeClient>>) {
    let (sender, jobs) = mpsc::channel();
    let session = Arc::new(Session {
        id: SessionId(1),
        workspace: None,
        undo_bytes: crate::document::undo::UNDO_BYTES,
        held: Holding::new(Held {
            file: Some(HeldFile {
                path: "a.silico".to_owned(),
                resolved: PathBuf::from("a.silico"),
                part_written: None,
            }),
            bytes: Arc::new(BOX.as_bytes().to_vec()),
            text: Arc::new(BOX.as_bytes().to_vec()),
            gated: Arc::new(Gated::of(BOX.as_bytes())),
            revision: Revision::OPENED,
            carried: Arc::new(BTreeSet::new()),
            transaction: None,
            last_reload: None,
            reloaded_over: None,
        }),
        queue: Mutex::new(Some(sender)),
        rebuilds: Mutex::new(Rebuilds {
            engine: Recompute::new(kernel.client()),
            running: None,
            published: None,
            ended: None,
            kept: None,
            floor: Revision::OPENED,
            candidate: None,
            watchers: Threads::new(),
            last_number: 0,
            stopped_for: None,
        }),
        rebuilt: Condvar::new(),
        reads: Mutex::new(Reads::new()),
        command: Mutex::new(None),
        newest: Arc::default(),
        logs: crate::command::logs::LogBuffer::new(),
        disk: OnceLock::from(DiskWatch::new(
            "a.silico",
            None,
            Arc::new(BOX.as_bytes().to_vec()),
            Duration::ZERO,
            None,
        )),
        reloads: Mutex::default(),
        drags: Mutex::default(),
        waker: Arc::default(),
        gate: Gate::default(),
        settled: Gate::default(),
        writing: Gate::default(),
        reloading: Gate::default(),
        watching: Gate::default(),
        dragged: Gate::default(),
    });
    (session, jobs)
}

/// The ID of the one feature `model` was built from.
fn built_from(model: &Model) -> String {
    model.document().features[0].id.to_string()
}

#[test]
fn a_fresh_read_answers_with_its_own_rebuilds_model_never_one_recorded_after_it_started() {
    // A rebuild of the revision that reads what's kept publishes in its engine before a
    // fresh read's start supersedes it, and its watcher records that model only after the
    // fresh rebuild has started: of the right revision, not the model published before the
    // fresh read, yet not the fresh rebuild's. The fresh read waits for its own. (The late
    // model is another engine's sphere, so the two are told apart by what they're built
    // from; the fresh one's box is held until the late one is recorded.)
    let kernel = FakeKernel::new(2).unwrap();
    let (session, _jobs) = session(&kernel);
    let hold = kernel.hold(|op| op.name() == "MakeBox");
    let (said, answered) = mpsc::channel();
    let reader = {
        let session = Arc::clone(&session);
        std::thread::spawn(move || {
            let model = session.model_with(
                &probe(),
                Revision::OPENED,
                &checked(BOX),
                &Reached::default(),
                crate::recompute::Reads::Nothing,
            );
            let _ = said.send(model.map(|model| built_from(&model)));
        })
    };
    assert!(kernel.wait_running(1, WAIT));
    let other = FakeKernel::new(1).unwrap();
    let late = Recompute::new(other.client());
    watch(
        &Arc::downgrade(&session),
        &Label::new(Revision::OPENED, Reached::default()),
        late.start(checked(SPHERE)),
    );
    // Recorded as the revision's newest, and told to the waiting read...
    let recorded = lock(&session.rebuilds).published.clone().unwrap();
    assert_eq!(recorded.revision, Revision::OPENED);
    assert_eq!(built_from(&recorded.model), "a91cw0de");
    // ...which doesn't take it: it answers only once its own rebuild is let through.
    assert!(
        answered.recv_timeout(Duration::from_millis(500)).is_err(),
        "a fresh read answered before its own rebuild ended"
    );
    drop(hold);
    assert_eq!(
        answered.recv_timeout(WAIT).unwrap().unwrap(),
        "k7f2qx3m".to_owned()
    );
    reader.join().unwrap();
    // Its model is the revision's from then on.
    let published = lock(&session.rebuilds).published.clone().unwrap();
    assert_eq!(built_from(&published.model), "k7f2qx3m");
}

#[test]
fn a_rebuild_started_before_a_fresh_one_never_replaces_what_the_fresh_one_came_to() {
    // A rebuild of revision 1 that reads what's kept (another engine's sphere, here) starts
    // before a fresh read's; the fresh rebuild (the box) publishes first, and the first's
    // watcher records its model only after. The revision's model stays the fresh one's:
    // the read after it gets the box, never the model of before the fresh check.
    let kernel = FakeKernel::new(2).unwrap();
    let (session, _jobs) = session(&kernel);
    let before = lock(&session.rebuilds).numbered(Label::new(Revision::OPENED, Reached::default()));
    let other = FakeKernel::new(1).unwrap();
    let late = Recompute::new(other.client());
    let run = late.start(checked(SPHERE));
    let fresh = session
        .model_with(
            &probe(),
            Revision::OPENED,
            &checked(BOX),
            &Reached::default(),
            crate::recompute::Reads::Nothing,
        )
        .unwrap();
    assert_eq!(built_from(&fresh), "k7f2qx3m");
    watch(&Arc::downgrade(&session), &before, run);
    let published = lock(&session.rebuilds).published.clone().unwrap();
    assert_eq!(published.revision, Revision::OPENED);
    assert_eq!(built_from(&published.model), "k7f2qx3m");
    assert!(Arc::ptr_eq(&published.model, &fresh));
    // A read of what's kept answers with it, starting nothing.
    let read = session
        .model(
            &probe(),
            Revision::OPENED,
            &checked(BOX),
            &Reached::default(),
        )
        .unwrap();
    assert!(Arc::ptr_eq(&read, &fresh));
    assert_eq!(
        kernel
            .calls()
            .iter()
            .filter(|call| call.op.name() == "MakeBox")
            .count(),
        1
    );
    // Nor does one started before a rebuild of the revision that ended unpublished (a
    // stop, here) take that one's place with its late model.
    let earlier =
        lock(&session.rebuilds).numbered(Label::new(Revision::OPENED, Reached::default()));
    let later = lock(&session.rebuilds).numbered(Label::new(Revision::OPENED, Reached::default()));
    lock(&session.rebuilds).end(&later, Ending::Cancelled);
    watch(
        &Arc::downgrade(&session),
        &earlier,
        late.start(checked(SPHERE)),
    );
    let published = lock(&session.rebuilds).published.clone().unwrap();
    assert!(Arc::ptr_eq(&published.model, &fresh));
    // One started after both publishes over them.
    let last = lock(&session.rebuilds).numbered(Label::new(Revision::OPENED, Reached::default()));
    watch(
        &Arc::downgrade(&session),
        &last,
        late.start(checked(SPHERE)),
    );
    let published = lock(&session.rebuilds).published.clone().unwrap();
    assert_eq!(built_from(&published.model), "a91cw0de");
}

#[test]
fn a_revisions_model_is_its_own_and_a_late_one_of_an_older_never_replaces_it() {
    let kernel = FakeKernel::new(2).unwrap();
    let (session, _jobs) = session(&kernel);
    assert_eq!(
        built_from(
            &session
                .model(
                    &probe(),
                    Revision::OPENED,
                    &checked(BOX),
                    &Reached::default()
                )
                .unwrap()
        ),
        "k7f2qx3m"
    );
    // Revision 2's read is never revision 1's model, published as it is.
    let second = session
        .model(
            &probe(),
            Revision::OPENED.next().unwrap(),
            &checked(SPHERE),
            &Reached::default(),
        )
        .unwrap();
    assert_eq!(built_from(&second), "a91cw0de");
    // A model of revision 1 arriving late (another engine's, here) leaves 2's in place.
    let other = Recompute::new(kernel.client());
    watch(
        &Arc::downgrade(&session),
        &Label::new(Revision::OPENED, Reached::default()),
        other.start(checked(BOX)),
    );
    let published = lock(&session.rebuilds).published.clone().unwrap();
    assert_eq!(published.revision, Revision::OPENED.next().unwrap());
    assert!(Arc::ptr_eq(&published.model, &second));
    // Nor is revision 1's rebuild started again now that 2's has published.
    let mut rebuilds = lock(&session.rebuilds);
    assert!(
        !session
            .start_unless_newer(
                &mut rebuilds,
                Revision::OPENED,
                &checked(BOX),
                &Reached::default()
            )
            .unwrap()
    );
    assert_eq!(rebuilds.running(), None);
}

#[test]
fn a_revision_has_a_next_until_there_are_none_left() {
    assert_eq!(Revision::OPENED.next(), Some(Revision(2)));
    assert_eq!(Revision(u64::MAX - 1).next(), Some(Revision(u64::MAX)));
    assert_eq!(Revision(u64::MAX).next(), None);
}

/// Sessions on a fake kernel with [`BOX`] open as s1, checked so revision 1's model is
/// published, then moved to revision 2 with `text` as its document, as an edit would: so a
/// read can be told of which revision it is.
fn at_two(
    text: &str,
) -> (
    tempfile::TempDir,
    Arc<FakeKernel>,
    Sessions<Arc<FakeKernel>>,
    SessionId,
) {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("a.silico");
    std::fs::write(&path, BOX).unwrap();
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let sessions = Sessions::with_cache_opener(Arc::clone(&kernel), move |cache| {
        Cache::with_secret(cache, &secret)
    });
    let id = sessions.open(&path, false).wait().session.unwrap().id;
    assert!(
        sessions
            .check(&id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let session = sessions.find(&id).unwrap();
    let mut held = session.held.session_changing();
    held.revision = Revision::OPENED.next().unwrap();
    held.text = Arc::new(text.as_bytes().to_vec());
    held.gated = Arc::new(Gated::of(text.as_bytes()));
    drop(held);
    (dir, kernel, sessions, id)
}

/// A read that gives the volume of the one feature its model was built from, and what it
/// read.
fn volume<C>(read: Read<'_, C>) -> Result<(Measure, ModelRead), Problems> {
    let feature = &read.model().document().features[0].id;
    let fingerprint = read.model().fingerprint(feature).unwrap();
    Ok((fingerprint.volume, read.model_read().clone()))
}

#[test]
fn a_published_read_says_the_older_revision_it_read() {
    let (_dir, kernel, sessions, id) = at_two(SPHERE);
    // Revision 2's sphere held, so its rebuild is seen running.
    let _hold = kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
    let response = sessions
        .read_model(&id, ReadCommand::Check, ModelChoice::Published, volume)
        .wait();
    let read = ModelRead {
        revision: Revision::OPENED,
        of: ModelChoice::Published,
        fault: None,
    };
    // The box, 40 × 20 × 10: 8000 mm³, of revision 1, while the session is at 2.
    assert_eq!(
        response.result.unwrap(),
        (Measure::from_millionths(8_000_000_000), read.clone())
    );
    assert_eq!(response.model, Some(read));
    let session = response.session.unwrap();
    assert_eq!(session.revision, Revision::OPENED.next().unwrap());
    // And it started revision 2's rebuild.
    let found = sessions.find(&id).unwrap();
    assert_eq!(
        lock(&found.rebuilds).running(),
        Some(Revision::OPENED.next().unwrap())
    );
}

#[test]
fn a_current_read_says_the_newer_revision_it_waited_for() {
    let (_dir, _kernel, sessions, id) = at_two(SPHERE);
    let response = sessions
        .read_model(&id, ReadCommand::Check, ModelChoice::Current, volume)
        .wait();
    let read = ModelRead {
        revision: Revision::OPENED.next().unwrap(),
        of: ModelChoice::Current,
        fault: None,
    };
    // The sphere of radius 5: 4/3 π 5³ = 523.598776 mm³, of revision 2.
    assert_eq!(
        response.result.unwrap(),
        (Measure::from_millionths(523_598_776), read.clone())
    );
    assert_eq!(response.model, Some(read));
    assert_eq!(
        response.session.unwrap().revision,
        Revision::OPENED.next().unwrap()
    );
}

#[test]
fn a_published_read_of_a_refused_revision_gives_the_older_model_and_check_its_problems() {
    let flat = BOX.replace("\"length\": 40", "\"length\": 0");
    let (_dir, kernel, sessions, id) = at_two(&flat);
    let calls = kernel.calls().len();
    let read = ModelRead {
        revision: Revision::OPENED,
        of: ModelChoice::Published,
        fault: None,
    };
    // The newest published model, revision 1's, its revision saying it's older.
    let published = sessions.published(&id);
    assert_eq!(published.model, Some(read.clone()));
    assert_eq!(published.result.unwrap().revision, Revision::OPENED);
    let response = sessions
        .read_model(&id, ReadCommand::Check, ModelChoice::Published, volume)
        .wait();
    assert_eq!(
        response.result.unwrap(),
        (Measure::from_millionths(8_000_000_000), read)
    );
    // The gate's problems are check's; nothing is rebuilt.
    let problems = sessions
        .check(&id, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap()
        .problems;
    assert_eq!(problems.errors()[0].parameter(), Some("length"));
    assert_eq!(kernel.calls().len(), calls);
}

#[test]
fn a_published_read_racing_a_close_is_refused_as_not_open() {
    // Closed (its queue ended) but not yet gone from the registry, as a close is for a
    // moment: the read refuses it, though a model is published.
    let (_dir, _kernel, sessions, id) = at_two(SPHERE);
    sessions.find(&id).unwrap().close();
    let response = sessions.published(&id);
    // Of no session, as every refusal of a session not open is.
    assert_eq!(response.session, None);
    assert_eq!(response.model, None);
    assert_eq!(response.result.unwrap_err().errors()[0], not_open(id));
}

#[test]
fn a_rebuild_is_refused_only_under_a_newer_revisions() {
    let kernel = FakeKernel::new(1).unwrap();
    let (session, _jobs) = session(&kernel);
    let _hold = kernel.hold(|_| true);
    let mut rebuilds = lock(&session.rebuilds);
    // Revision 2's rebuild running (as recorded): revision 1's isn't started under it.
    rebuilds.running = Some(Label::new(
        Revision::OPENED.next().unwrap(),
        Reached::default(),
    ));
    assert!(
        !session
            .start_unless_newer(
                &mut rebuilds,
                Revision::OPENED,
                &checked(BOX),
                &Reached::default()
            )
            .unwrap()
    );
    assert_eq!(rebuilds.running(), Some(Revision::OPENED.next().unwrap()));
    // Its own revision's running: started (as a check after a fault starts it again).
    rebuilds.running = Some(Label::new(Revision::OPENED, Reached::default()));
    assert!(
        session
            .start_unless_newer(
                &mut rebuilds,
                Revision::OPENED,
                &checked(BOX),
                &Reached::default()
            )
            .unwrap()
    );
    assert_eq!(rebuilds.running(), Some(Revision::OPENED));
    drop(rebuilds);
    assert!(kernel.wait_running(1, WAIT));
}

#[test]
fn a_superseded_rebuilds_watcher_leaves_the_newer_one_running() {
    let kernel = FakeKernel::new(2).unwrap();
    let (session, _jobs) = session(&kernel);
    let boxes = kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let spheres = kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
    let second = Revision::OPENED.next().unwrap();
    // Revision 1's rebuild, its box held.
    let mut rebuilds = lock(&session.rebuilds);
    assert!(
        session
            .start_unless_newer(
                &mut rebuilds,
                Revision::OPENED,
                &checked(BOX),
                &Reached::default()
            )
            .unwrap()
    );
    drop(rebuilds);
    assert!(kernel.wait_running(1, WAIT));
    // Revision 2's supersedes it, its sphere held.
    let mut rebuilds = lock(&session.rebuilds);
    assert!(
        session
            .start_unless_newer(&mut rebuilds, second, &checked(SPHERE), &Reached::default())
            .unwrap()
    );
    assert_eq!(rebuilds.watchers.running.len(), 2);
    drop(rebuilds);
    // Revision 1's step let go (if its supersession hadn't stopped it), so its watcher ends.
    boxes.release();
    ended(|| lock(&session.rebuilds).watchers.running[0].is_finished());
    // Revision 2's is still the rebuild running, and nothing is published of revision 1.
    let rebuilds = lock(&session.rebuilds);
    assert_eq!(rebuilds.running(), Some(second));
    assert!(rebuilds.published.is_none());
    drop(rebuilds);
    // Its model, when its sphere is let go, is revision 2's, from its one rebuild.
    spheres.release();
    let model = session
        .model(&probe(), second, &checked(SPHERE), &Reached::default())
        .unwrap();
    assert_eq!(built_from(&model), "a91cw0de");
    let published = lock(&session.rebuilds).published.clone().unwrap();
    assert_eq!(published.revision, second);
    assert!(Arc::ptr_eq(&published.model, &model));
    let spheres_made = kernel
        .calls()
        .iter()
        .filter(|c| matches!(c.op, Op::MakePrimitive(Primitive::Sphere { .. })))
        .count();
    assert_eq!(spheres_made, 1);
}

#[test]
fn a_published_read_starts_its_revisions_rebuild_only_while_an_older_is_published() {
    let kernel = FakeKernel::new(1).unwrap();
    let (session, _jobs) = session(&kernel);
    let first = session
        .model(
            &probe(),
            Revision::OPENED,
            &checked(BOX),
            &Reached::default(),
        )
        .unwrap();
    // At revision 1, with revision 1 published: that model, and nothing started.
    let read = session
        .published_read(
            Revision::OPENED,
            &checked(BOX),
            session.held.read().reaching(),
        )
        .unwrap();
    assert_eq!(read.revision, Revision::OPENED);
    assert!(Arc::ptr_eq(&read.model, &first));
    assert_eq!(lock(&session.rebuilds).running(), None);
    // At revision 2, with only revision 1 published: revision 1's model, at once, and
    // revision 2's rebuild started (held, so it's seen running).
    let _hold = kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
    let read = session
        .published_read(
            Revision::OPENED.next().unwrap(),
            &checked(SPHERE),
            session.held.read().reaching(),
        )
        .unwrap();
    assert_eq!(read.revision, Revision::OPENED);
    assert!(Arc::ptr_eq(&read.model, &first));
    assert_eq!(
        lock(&session.rebuilds).running(),
        Some(Revision::OPENED.next().unwrap())
    );
}

#[test]
fn a_rebuild_stopped_under_an_open_session_is_started_again() {
    let kernel = FakeKernel::new(1).unwrap();
    let (session, _jobs) = session(&kernel);
    let hold = kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let boxes = || {
        kernel
            .calls()
            .iter()
            .filter(|c| matches!(c.op, Op::MakePrimitive(Primitive::Box { .. })))
            .count()
    };
    std::thread::scope(|scope| {
        let read = scope.spawn(|| {
            session.model(
                &probe(),
                Revision::OPENED,
                &checked(BOX),
                &Reached::default(),
            )
        });
        assert!(kernel.wait_running(1, WAIT));
        lock(&session.rebuilds).engine.cancel();
        // The box asked for again, by the read, still waiting.
        let deadline = std::time::Instant::now() + WAIT;
        while boxes() < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "the rebuild wasn't started again"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        hold.release();
        let model = read.join().unwrap().unwrap();
        assert_eq!(built_from(&model), "k7f2qx3m");
    });
    assert_eq!(boxes(), 2);
}

#[test]
fn a_published_read_never_gives_a_model_newer_than_the_sessions_revision() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("a.silico");
    std::fs::write(&path, BOX).unwrap();
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let sessions = Sessions::with_cache_opener(Arc::clone(&kernel), move |cache| {
        Cache::with_secret(cache, &secret)
    });
    let id = sessions.open(&path, false).wait().session.unwrap().id;
    // A model of revision 2, which this session (at 1) hasn't reached.
    let newer = match Recompute::new(FakeKernel::new(1).unwrap())
        .start(checked(SPHERE))
        .wait()
        .unwrap()
    {
        RunOutcome::Published(model) => model,
        RunOutcome::Superseded => panic!("superseded"),
    };
    let session = sessions.find(&id).unwrap();
    lock(&session.rebuilds).published = Some(Published {
        revision: Revision::OPENED.next().unwrap(),
        model: newer,
        reached: Reached::default(),
        by: None,
    });
    let read = sessions.published(&id);
    assert_eq!(read.result.unwrap_err().errors()[0], nothing_published(id));
    // Nor does it start revision 1's rebuild, whose model would be older than one there.
    assert_eq!(lock(&session.rebuilds).running(), None);
}

#[test]
fn a_published_read_each_frame_while_an_edits_rebuild_runs_works_out_no_reach() {
    // An edit outside a transaction, its candidate's box held: the session's rebuild
    // running is the candidate's, of the revision its save would give, nothing published.
    // A published read each frame finds its revision's model wanted and none newer
    // published, but starts nothing under the candidate: what the session's edits reach,
    // which may scan a long text, is never worked out on the caller's thread for it.
    let f = opened(BOX);
    let hold = f.kernel.hold(is_box);
    let renamed = BOX.replace("\"Block\"", "\"Plate\"");
    let edit = f.sessions.replace_text(&f.id, Revision::OPENED, renamed);
    assert!(f.kernel.wait_running(1, WAIT));
    assert!(
        lock(&f.session().rebuilds)
            .running
            .as_ref()
            .is_some_and(|running| running.revision() == Revision(2))
    );
    let before = REACHES.with(std::cell::Cell::get);
    for _ in 0..100 {
        let read = f.sessions.published(&f.id);
        assert_eq!(read.result.unwrap_err().errors(), [nothing_published(f.id)]);
    }
    assert_eq!(REACHES.with(std::cell::Cell::get), before);
    hold.release();
    assert!(edit.wait().result.unwrap().changed);
}

#[test]
fn a_document_made_for_a_session_the_sessions_closing_refuse_is_removed() {
    // The sessions began closing while the open made its file: it's refused, and what it
    // made is gone; a file it didn't make is left.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let sessions = Sessions::with_cache_opener(FakeKernel::new(1).unwrap(), move |cache| {
        Cache::with_secret(cache, &secret)
    });
    lock(&sessions.registry.book).stopping = true;
    let (reply, answer) = mpsc::sync_channel(1);
    let made = dir.path().join("new.silico");
    let started = start(
        Arc::clone(&sessions.registry),
        SessionId(1),
        made.to_str(),
        true,
        &crate::command::session::cancel::Ticket::opening(),
        &reply,
        unqueued(),
    );
    assert!(started.is_none());
    let response = answer.recv().unwrap();
    assert_eq!(response.result.unwrap_err().errors()[0], closing());
    assert!(!made.exists());
    let kept = dir.path().join("kept.silico");
    std::fs::write(&kept, "{}").unwrap();
    assert!(
        start(
            Arc::clone(&sessions.registry),
            SessionId(2),
            kept.to_str(),
            false,
            &crate::command::session::cancel::Ticket::opening(),
            &reply,
            unqueued(),
        )
        .is_none()
    );
    assert!(kept.exists());
    let book = lock(&sessions.registry.book);
    assert!(book.files.is_empty());
    assert!(book.paths.is_empty());
}

#[test]
fn a_workspace_is_refused_once_an_open_was_asked_for_its_thread_ended() {
    // Found by the review: the integration test's refused open may still have its thread,
    // and so the registry shared, when the workspace is given, so it's refused for that and
    // never reaches the check that no open was asked for. Here the thread has certainly
    // ended (joined), the registry is the sessions' alone, and still it's refused, for the
    // open asked for (`next` past 1).
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let made = || {
        let secret = secret.clone();
        Sessions::with_cache_opener(FakeKernel::new(1).unwrap(), move |cache| {
            Cache::with_secret(cache, &secret)
        })
    };
    let sessions = made();
    let refused = sessions
        .open(dir.path().join("none.silico"), false)
        .wait_timeout(WAIT)
        .ok()
        .unwrap();
    // Refused as open refuses a file that isn't there (tests/session.rs's
    // `a_missing_file_or_a_directory_or_a_pipe_isnt_opened`).
    let Err(problems) = &refused.result else {
        panic!("a file that isn't there was opened");
    };
    let e = &problems.errors()[0];
    assert_eq!(
        (problems.total(), e.code(), e.message(), e.parameter()),
        (
            1,
            crate::ErrorCode::IoFailed,
            "couldn't be read: No such file or directory (os error 2)",
            Some("path")
        )
    );
    let threads = std::mem::replace(&mut lock(&sessions.registry.book).threads, Threads::new());
    threads.join();
    assert_eq!(Arc::strong_count(&sessions.registry), 1);
    assert_eq!(lock(&sessions.registry.book).next, 2);
    let e = match sessions.in_workspace(dir.path()) {
        Ok(_) => panic!("given a workspace after an open was asked for"),
        Err(e) => e,
    };
    assert_eq!(e.message(), "these sessions can't be given a workspace now");
    assert_eq!(
        e.cause(),
        Some("a workspace is given once, as the sessions are made, before any document opens")
    );
    // The same sessions, no open asked for: given it.
    assert!(made().in_workspace(dir.path()).is_ok());
}

/// A session's queue with no command in it, and none to come: its sender gone before
/// it's given (a temporary's lives to the end of its statement, so `start` would wait on
/// it).
fn unqueued() -> Receiver<Job<FakeClient>> {
    mpsc::channel().1
}

impl Held {
    /// What the session's edits reach now ([`Reaching::reach`]).
    fn reach(&self) -> BTreeSet<FeatureId> {
        self.reaching().reach()
    }

    /// Its file's path, as `open` was given it: none if it's untitled.
    fn path(&self) -> Option<&str> {
        self.file.as_ref().map(|file| file.path.as_str())
    }

    /// Where its file's old text is, if its own save left the file part written.
    fn part_written(&self) -> Option<&PathBuf> {
        self.file
            .as_ref()
            .and_then(|file| file.part_written.as_ref())
    }

    /// Its file's mark of a save left part written, to set (the file there, as every test
    /// of it opens one).
    fn part_written_mut(&mut self) -> &mut Option<PathBuf> {
        match &mut self.file {
            Some(file) => &mut file.part_written,
            None => panic!("the session is untitled: it has no file to mark"),
        }
    }
}

impl<C: Kernel + 'static> Session<C> {
    /// The document replaced by `text` as `replace_text` replaces it inside a
    /// transaction, whether or not one is open: the production edit's candidate
    /// (`edit::replace_text`) held as an edit inside a transaction holds it
    /// (`Session::hold`), at a new revision unless it's the same document, starting no
    /// rebuild and saving nothing (what it reaches is the session's to work out,
    /// `Reaching::reach`); refused whole, the session as it was, as the edit is.
    fn applied(&self, stop: &Ticket, text: &str) -> Result<Revision, Problems> {
        let (held, gated) = self.document();
        match edit::replace_text(&gated, &held, text.as_bytes())?.1 {
            Some(candidate) => self.hold(stop, candidate),
            None => Ok(self.held.read().revision),
        }
    }
}

/// Sessions on a fake kernel of two threads, with a document in `a.silico` open as s1.
struct Fixture {
    _dir: tempfile::TempDir,
    kernel: Arc<FakeKernel>,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
    path: PathBuf,
}

fn opened(text: &str) -> Fixture {
    opened_solving(text, &crate::fake::FakeSketches::new())
}

/// [`opened`], its sketches solved by `sketches`.
fn opened_solving(text: &str, sketches: &crate::fake::FakeSketches) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("a.silico");
    std::fs::write(&path, text).unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let solver = sketches.clone();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    )
    .checking_disk_every(Duration::ZERO);
    let id = sessions.open(&path, false).wait().session.unwrap().id;
    Fixture {
        _dir: dir,
        kernel,
        sessions,
        id,
        path,
    }
}

impl Fixture {
    fn session(&self) -> Arc<Session<FakeClient>> {
        self.sessions.find(&self.id).unwrap()
    }

    fn edit(&self, text: &str) -> Revision {
        self.session().applied(&probe(), text).unwrap()
    }

    fn file(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }

    fn begin(&self) -> Begun {
        self.sessions.begin(&self.id).wait().result.unwrap()
    }

    /// How many requests of the kind `pick` picks the kernel has started.
    fn made(&self, pick: impl Fn(&Op) -> bool) -> usize {
        self.kernel.calls().iter().filter(|c| pick(&c.op)).count()
    }
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// Each feature's state in a rebuild, as `status` lists them: `pairs`, by ID, in order.
fn states(pairs: &[(&str, StepState)]) -> crate::recompute::FeatureStates {
    pairs
        .iter()
        .map(|(feature, state)| (id(feature), *state))
        .collect()
}

fn is_box(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
}

fn is_cylinder(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
}

/// A block, a lug clear of it, and their fuse, each with the fingerprint the fake
/// measures: the boxes' by formula (40 × 20 × 10 = 8000, area 2800; 10 × 10 × 10 =
/// 1000, area 600), and the fuse's both together, as the fake fuses two solids as if
/// they didn't touch (and these don't): 9000, area 3400. But the block's volume is
/// stored 1e-6 mm³ over what it measures: drift, which no edit below reaches.
const STORED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [
  {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "fingerprint": {"volume": 8000.000001, "area": 2800, "center": [20, 10, 5], "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}},
  {"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10, "fingerprint": {"volume": 1000, "area": 600, "center": [105, 5, 5], "bbox": [100, 0, 0, 110, 10, 10], "faces": 6, "edges": 12, "vertices": 8}},
  {"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "k7f2qx3m", "tool": "b0000001", "fingerprint": {"volume": 9000, "area": 3400, "center": [29.444444, 9.444444, 5], "bbox": [0, 0, 0, 110, 20, 10], "faces": 12, "edges": 24, "vertices": 16}}
]}
"#;

/// [`STORED`] with the lug 20 long: an edit that reaches the lug and the fuse.
fn longer() -> String {
    let longer = STORED.replace(
        r#""length": 10, "width": 10"#,
        r#""length": 20, "width": 10"#,
    );
    assert_ne!(longer, STORED);
    longer
}

/// [`longer`] committed: the block's fingerprint as stored, drift and all; the lug's
/// and the fuse's new, by hand: the lug 20 × 10 × 10 = 2000, area 2(200 + 200 + 100) =
/// 1000, to x = 120; the fuse 8000 + 2000 = 10000, area 2800 + 1000 = 3800.
const COMMITTED: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 6,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 8000.000001, \"area\": 2800, \"center\": [20, 10, 5], \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    },\n    {\n      \"id\": \"b0000001\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Lug\",\n      \"position\": [100, 0, 0],\n      \"length\": 20,\n      \"width\": 10,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 2000, \"area\": 1000, \"center\": [110, 5, 5], \"bbox\": [100, 0, 0, 120, 10, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    },\n    {\n      \"id\": \"f0000001\",\n      \"type\": \"fuse\",\n      \"v\": 1,\n      \"name\": \"Both\",\n      \"base\": \"k7f2qx3m\",\n      \"tool\": \"b0000001\",\n      \"fingerprint\": {\"volume\": 10000, \"area\": 3800, \"center\": [38, 9, 5], \"bbox\": [0, 0, 0, 120, 20, 10], \"faces\": 12, \"edges\": 24, \"vertices\": 16}\n    }\n  ]\n}\n";

/// The one drift `problems` hold: the block's volume, stored 1e-6 over.
#[track_caller]
fn the_blocks_drift(problems: &Problems) {
    assert_eq!(problems.total(), 1, "{problems:?}");
    let drift = &problems.errors()[0];
    assert_eq!(
        (drift.code(), drift.feature(), drift.parameter()),
        (
            ErrorCode::Drift,
            Some("k7f2qx3m"),
            Some("fingerprint.volume")
        ),
        "{drift:?}"
    );
    // Stored → rebuilt, as design section 7's commit example writes it.
    assert_eq!(
        drift.message(),
        "feature k7f2qx3m rebuilt to other measures than its fingerprint: volume 8000.000001 → 8000"
    );
    // The design's words (section 7's commit example), which name format (decision 31).
    assert_eq!(
        drift.suggestion(),
        Some(
            "if this feature or one it uses was edited by hand or in a merge since it was saved, run format (or silico fmt) to accept its new fingerprint; if not, this is a determinism bug or a change in the kernel: find which before accepting it"
        )
    );
}

#[test]
fn an_edits_saved_model_is_named_by_the_background_with_no_client_asking() {
    // An edit outside a transaction saves, and publishes its rebuild's model rebound to the
    // saved document: a copy, not the model the engine published and the background names.
    // Its names settle all the same, with no client asking: read as the app's selection
    // reads them, never naming anything (`named_element_map`, `selection::resolve`), a
    // pick of the block's face by name is found once the background has named the engine's
    // model, and is pending till then.
    let f = opened(BOX);
    let session = f.session();
    lock(&session.rebuilds).engine.hold_naming();
    let revision = session.held.read().revision;
    let set = f
        .sessions
        .set_field(
            &f.id,
            revision,
            id("k7f2qx3m"),
            "length".to_owned(),
            crate::document::json::read(b"41").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    assert!(set.commit.unwrap().written);
    let published = f.sessions.published(&f.id).result.unwrap();
    assert_eq!(published.revision.number(), 2);
    let saved = published.model;
    let built = lock(&session.rebuilds).engine.published().unwrap();
    // The saved revision's model is the edit's, rebound: another model, of the saved text.
    assert!(!Arc::ptr_eq(&saved, &built));
    assert_eq!(saved.generation(), built.generation());
    assert_eq!(
        saved.document().features[0].fingerprint.unwrap().volume,
        Measure::from_millionths(8_200_000_000)
    );
    let block = id("k7f2qx3m");
    let pick = ["k7f2qx3m.box.Face1"];
    assert!(saved.named_element_map(&block).is_none());
    assert!(saved.named_so_far(&block).is_none());
    let resolved = crate::selection::resolve(&saved, &pick, "selection").unwrap();
    assert_eq!(resolved.pending.len(), 1, "{resolved:?}");
    assert!(resolved.kept.is_empty(), "{resolved:?}");
    lock(&session.rebuilds).engine.release_naming();
    // Waited for on the engine's model, the one it names; never on the saved copy. (The
    // background's naming takes no lock of the session's.)
    assert!(
        lock(&session.rebuilds)
            .engine
            .wait_naming_ended(&built, WAIT)
    );
    assert!(std::ptr::eq(
        saved.named_element_map(&block).unwrap(),
        built.named_element_map(&block).unwrap()
    ));
    let resolved = crate::selection::resolve(&saved, &pick, "selection").unwrap();
    assert!(resolved.pending.is_empty(), "{resolved:?}");
    assert!(resolved.gone.is_empty(), "{resolved:?}");
    assert_eq!(resolved.kept.len(), 1, "{resolved:?}");
    assert_eq!(resolved.kept[0].feature, block);
    assert_eq!(
        resolved.kept[0].element,
        silico_protocol::ElementRef::face(1)
    );
}

#[test]
fn a_commit_writes_new_fingerprints_only_for_what_edits_reached_and_keeps_drift_as_stored() {
    let f = opened(STORED);
    assert_eq!(
        f.begin(),
        Begun {
            base: Revision::OPENED
        }
    );
    assert_eq!(f.edit(&longer()), Revision(2));
    let response = f.sessions.commit(&f.id).wait();
    let session = response.session.unwrap();
    assert_eq!(
        (session.revision, session.transaction),
        (Revision(3), false)
    );
    assert_eq!(
        response.model,
        Some(ModelRead {
            revision: Revision(2),
            of: ModelChoice::Current,
            fault: None,
        })
    );
    let committed = response.result.unwrap();
    assert!(committed.written);
    assert_eq!(committed.path, f.path.to_str().unwrap());
    assert_eq!(committed.refingerprinted, [id("b0000001"), id("f0000001")]);
    assert!(committed.resolved.is_empty());
    the_blocks_drift(&committed.problems);
    assert_eq!(f.file(), COMMITTED);
    // The session holds what it wrote, as text and as the file's bytes, and nothing is
    // reached since the save.
    let session = f.session();
    let held = session.held.read();
    assert_eq!(held.text.as_slice(), COMMITTED.as_bytes());
    assert_eq!(held.bytes.as_slice(), COMMITTED.as_bytes());
    assert!(held.reach().is_empty());
    drop(held);
    // So a check of the saved revision finds the block's drift alone.
    the_blocks_drift(
        &f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems,
    );
}

#[test]
fn a_check_says_no_drift_of_what_edits_reached() {
    // The lug's stored 1000 and the fuse's 9000 were measured from the old length: the
    // edit reached them, so only the block's drift is said.
    let f = opened(STORED);
    f.begin();
    f.edit(&longer());
    let response = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    the_blocks_drift(&response.result.unwrap().problems);
    assert_eq!(response.model.unwrap().revision, Revision(2));
    assert_eq!(
        f.session().held.read().reach(),
        BTreeSet::from([id("b0000001"), id("f0000001")])
    );
}

#[test]
fn a_notes_edit_reaches_nothing_and_its_commit_writes_no_fingerprint() {
    // The lug has no fingerprint stored: a commit that reached it would write one.
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "fingerprint": {"volume": 8000, "area": 2800, "center": [20, 10, 5], "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}, {"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10}]}"#;
    let noted = text.replace(
        r#""name": "Block", "#,
        r#""name": "Block", "note": "Mount face", "#,
    );
    let f = opened(text);
    f.begin();
    f.edit(&noted);
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert!(committed.written);
    assert!(committed.refingerprinted.is_empty());
    assert!(committed.problems.is_empty(), "{:?}", committed.problems);
    assert_eq!(
        f.file(),
        "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 6,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 10,\n      \"note\": \"Mount face\",\n      \"fingerprint\": {\"volume\": 8000, \"area\": 2800, \"center\": [20, 10, 5], \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    },\n    {\"id\": \"b0000001\", \"type\": \"box\", \"v\": 1, \"name\": \"Lug\", \"position\": [100, 0, 0], \"length\": 10, \"width\": 10, \"height\": 10}\n  ]\n}\n"
    );
}

#[test]
fn a_commit_of_edits_undone_writes_nothing_and_reaches_what_begin_saw() {
    let f = opened(STORED);
    f.begin();
    f.edit(&longer());
    // Back as it was, though in canonical form now: the same document.
    f.edit(&document::write(&document::read(STORED.as_bytes()).unwrap()).unwrap());
    let response = f.sessions.commit(&f.id).wait();
    let committed = response.result.unwrap();
    assert!(!committed.written);
    assert!(committed.refingerprinted.is_empty());
    // The block's drift is said: the commit's check is begin's.
    the_blocks_drift(&committed.problems);
    assert_eq!(f.file(), STORED);
    // The session's text is the file's again (not the canonical form the second edit
    // left): a new revision, 4, after the two edits'.
    let session = response.session.unwrap();
    assert_eq!(
        (session.revision, session.transaction),
        (Revision(4), false)
    );
    // The commit's model, revision 3's, is revision 4's: the same document, so nothing
    // is rebuilt for it.
    assert_eq!(
        f.sessions.published(&f.id).result.unwrap().revision,
        Revision(4)
    );
    assert_eq!(lock(&f.session().rebuilds).running(), None);
    assert_eq!(
        f.sessions.text(&f.id, None).wait().result.unwrap().text,
        STORED
    );
    assert!(f.session().held.read().reach().is_empty());
}

#[test]
fn a_commit_over_a_file_changed_outside_is_a_conflict_writing_nothing_and_leaving_it_open() {
    let f = opened(STORED);
    f.begin();
    f.edit(&longer());
    std::fs::write(&f.path, "edited elsewhere").unwrap();
    let response = f.sessions.commit(&f.id).wait();
    let session = response.session.unwrap();
    // Inside the transaction, nothing reloaded.
    assert_eq!(
        (session.revision, session.transaction, session.reloaded),
        (Revision(2), true, false)
    );
    let problems = response.result.unwrap_err();
    assert_eq!(problems.total(), 1);
    let conflict = &problems.errors()[0];
    assert_eq!(conflict.code(), ErrorCode::Conflict);
    assert_eq!(conflict.parameter(), Some("path"));
    assert_eq!(
        conflict.message(),
        format!(
            "{} changed outside this session since it was read",
            shown_path(&f.path)
        )
    );
    assert_eq!(
        conflict.cause(),
        Some(
            "it changed after the transaction began, from revision 1, and a transaction doesn't reload its file"
        )
    );
    // The design's words (section 7's commit example).
    assert_eq!(
        conflict.suggestion(),
        Some(
            "nothing was written: roll back, then begin again from the file as it is now; or save elsewhere with path"
        )
    );
    // Found before the command, so the commit rebuilt nothing.
    assert!(f.kernel.calls().is_empty(), "{:?}", f.kernel.calls());
    assert_eq!(f.file(), "edited elsewhere");
    // Rolled back, as the advice says (inside the transaction still, so not reloaded),
    // then begun again: from the file as it is now, which begin reloads first. Revision
    // 1 at open, 2 the edit, 3 the rollback, 4 the reload.
    let rolled = f.sessions.rollback(&f.id).wait();
    assert_eq!(
        rolled.result.unwrap(),
        RolledBack {
            restored: Revision::OPENED
        }
    );
    assert!(!rolled.session.unwrap().reloaded);
    let begun = f.sessions.begin(&f.id).wait();
    assert_eq!(begun.result.unwrap(), Begun { base: Revision(4) });
    assert!(begun.session.unwrap().reloaded);
    assert_eq!(
        f.sessions.text(&f.id, None).wait().result.unwrap().text,
        "edited elsewhere"
    );
}

#[test]
fn a_reached_feature_that_failed_stays_reached_after_its_commit() {
    let f = opened(STORED);
    // The lug, 20 long, can't be made: it fails, and the fuse is blocked on it.
    f.kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 15.0 && *dx < 25.0)
            .then(|| Error::new(ErrorCode::KernelFailed, "the lug can't be made"))
    });
    f.begin();
    f.edit(&longer());
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert!(committed.written);
    assert!(committed.refingerprinted.is_empty());
    let found: Vec<(ErrorCode, Option<&str>)> = committed
        .problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature()))
        .collect();
    assert_eq!(
        found,
        [
            (ErrorCode::KernelFailed, Some("b0000001")),
            (ErrorCode::Drift, Some("k7f2qx3m"))
        ]
    );
    // Neither's fingerprint written, so both still reached; and both saved with none,
    // since what's stored was measured from the lug 10 long (decision 28, amended): the
    // block, which no edit reached, keeps its own, drift and all.
    assert_eq!(
        *f.session().held.read().carried,
        BTreeSet::from([id("b0000001"), id("f0000001")])
    );
    assert_eq!(
        stored_fingerprints(&f.file()),
        [Some(millionths(8_000_000_001, 2800)), None, None]
    );
    // A feature without an object written on one line (M2.1).
    assert_eq!(f.file(), REACHED_FAILED);
}

/// [`STORED`] committed with the lug 20 long, which failed: the block as stored, the lug
/// and the fuse, which the edit reached, saved with no fingerprint.
const REACHED_FAILED: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 6,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 8000.000001, \"area\": 2800, \"center\": [20, 10, 5], \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    },\n    {\"id\": \"b0000001\", \"type\": \"box\", \"v\": 1, \"name\": \"Lug\", \"position\": [100, 0, 0], \"length\": 20, \"width\": 10, \"height\": 10},\n    {\"id\": \"f0000001\", \"type\": \"fuse\", \"v\": 1, \"name\": \"Both\", \"base\": \"k7f2qx3m\", \"tool\": \"b0000001\"}\n  ]\n}\n";

/// Each feature's stored volume and area in the document `text`, in timeline order: none
/// for one saved without a fingerprint.
fn stored_fingerprints(text: &str) -> Vec<Option<(Measure, Measure)>> {
    document::read(text.as_bytes())
        .unwrap()
        .features
        .iter()
        .map(|feature| feature.fingerprint.map(|f| (f.volume, f.area)))
        .collect()
}

/// A volume in millionths of a mm³, and a whole area in mm².
fn millionths(volume: i128, area: i128) -> (Measure, Measure) {
    (
        Measure::from_millionths(volume),
        Measure::from_millionths(area * 1_000_000),
    )
}

/// The code and feature of each of `problems`.
fn said(problems: &Problems) -> Vec<(ErrorCode, Option<&str>)> {
    problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature()))
        .collect()
}

/// [`STORED`], its lug refused as stored (10 long), the block's note edited and committed:
/// an edit that reaches nothing.
fn the_unreached_lug_failed() -> (Fixture, Committed) {
    let f = opened(STORED);
    f.kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 5.0 && *dx < 15.0)
            .then(|| Error::new(ErrorCode::KernelFailed, "the lug can't be made"))
    });
    f.begin();
    f.edit(&STORED.replace(
        r#""name": "Block", "#,
        r#""name": "Block", "note": "Mount face", "#,
    ));
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    (f, committed)
}

#[test]
fn a_failed_feature_no_edit_reached_keeps_its_stored_fingerprint() {
    // The lug fails as stored, and the fuse is blocked on it; the edit, a note, reaches
    // nothing: all three keep what's stored, measured from the values the file holds.
    let (f, committed) = the_unreached_lug_failed();
    assert!(committed.written);
    assert!(committed.refingerprinted.is_empty());
    assert_eq!(
        said(&committed.problems),
        [
            (ErrorCode::KernelFailed, Some("b0000001")),
            (ErrorCode::Drift, Some("k7f2qx3m"))
        ]
    );
    assert!(f.session().held.read().carried.is_empty());
    assert_eq!(
        stored_fingerprints(&f.file()),
        [
            Some(millionths(8_000_000_001, 2800)),
            Some(millionths(1_000_000_000, 600)),
            Some(millionths(9_000_000_000, 3400))
        ]
    );
}

#[test]
fn a_reached_feature_saved_with_no_fingerprint_is_no_drift_in_a_new_session_and_a_reaching_commit_writes_one()
 {
    // Saved as `a_reached_feature_that_failed_stays_reached_after_its_commit` saves it,
    // then opened again: the lug's failure and the block's drift are said, and nothing of
    // the fuse, which has no fingerprint to hold. The lug made 30 long, which builds: its
    // commit writes both records. By hand: the lug 30 × 10 × 10 = 3000, area
    // 2(300 + 100 + 300) = 1400; the fuse 8000 + 3000 = 11000, area 2800 + 1400 = 4200.
    let f = opened(STORED);
    refuse_the_longer_lug(&f, ErrorCode::KernelFailed);
    f.begin();
    f.edit(&longer());
    assert!(f.sessions.commit(&f.id).wait().result.unwrap().written);
    assert_eq!(f.file(), REACHED_FAILED);
    f.sessions.close(&f.id).wait().result.unwrap();
    let again = f.sessions.open(&f.path, false).wait().session.unwrap().id;
    let checked = f
        .sessions
        .check(&again, CheckModel::Current { fresh: false })
        .wait();
    let problems = checked.result.unwrap().problems;
    assert_eq!(
        said(&problems),
        [
            (ErrorCode::KernelFailed, Some("b0000001")),
            (ErrorCode::Drift, Some("k7f2qx3m"))
        ]
    );
    let revision = checked.session.unwrap().revision;
    let set = f
        .sessions
        .set_field(
            &again,
            revision,
            id("b0000001"),
            "length".to_owned(),
            crate::document::json::read(b"30").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    let commit = set.commit.unwrap();
    assert!(commit.written);
    assert_eq!(commit.refingerprinted, [id("b0000001"), id("f0000001")]);
    the_blocks_drift(&commit.problems);
    assert_eq!(
        stored_fingerprints(&f.file()),
        [
            Some(millionths(8_000_000_001, 2800)),
            Some(millionths(3_000_000_000, 1400)),
            Some(millionths(11_000_000_000, 4200))
        ]
    );
}

#[test]
fn a_sessions_format_saves_a_reached_failure_with_no_fingerprint_and_an_unreached_one_as_stored() {
    // After the reached lug's commit: format accepts the block's drift (8000, as the fake
    // measures 40 × 20 × 10), and the lug and the fuse, reached and failed still, stay
    // with none, carried.
    let f = opened(STORED);
    refuse_the_longer_lug(&f, ErrorCode::KernelFailed);
    f.begin();
    f.edit(&longer());
    assert!(f.sessions.commit(&f.id).wait().result.unwrap().written);
    let formatted = f.sessions.format(&f.id, None, true).wait().result.unwrap();
    assert!(formatted.written);
    assert_eq!(formatted.accepted, [id("k7f2qx3m")]);
    assert_eq!(formatted.refingerprinted, [id("k7f2qx3m")]);
    assert_eq!(
        said(&formatted.problems),
        [(ErrorCode::KernelFailed, Some("b0000001"))]
    );
    assert_eq!(
        stored_fingerprints(&f.file()),
        [Some(millionths(8_000_000_000, 2800)), None, None]
    );
    assert_eq!(
        *f.session().held.read().carried,
        BTreeSet::from([id("b0000001"), id("f0000001")])
    );
    // The lug failing as stored, reached by no edit: format keeps its fingerprint and the
    // fuse's, as stored, and accepts the block's drift.
    let (f, _) = the_unreached_lug_failed();
    let formatted = f.sessions.format(&f.id, None, true).wait().result.unwrap();
    assert!(formatted.written);
    assert_eq!(formatted.refingerprinted, [id("k7f2qx3m")]);
    assert_eq!(
        stored_fingerprints(&f.file()),
        [
            Some(millionths(8_000_000_000, 2800)),
            Some(millionths(1_000_000_000, 600)),
            Some(millionths(9_000_000_000, 3400))
        ]
    );
}

#[test]
fn a_rollback_restores_the_text_byte_for_byte_and_the_same_model_at_a_new_revision() {
    // BOX isn't in canonical form: it comes back as written.
    let f = opened(BOX);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let model = f.sessions.published(&f.id).result.unwrap().model;
    f.begin();
    f.edit(SPHERE);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let calls = f.kernel.calls().len();
    let response = f.sessions.rollback(&f.id).wait();
    assert_eq!(
        response.result.unwrap(),
        RolledBack {
            restored: Revision::OPENED
        }
    );
    let session = response.session.unwrap();
    assert_eq!(
        (session.revision, session.transaction),
        (Revision(3), false)
    );
    assert_eq!(
        f.sessions.text(&f.id, None).wait().result.unwrap().text,
        BOX
    );
    // The model begin saw, the same one, as revision 3's.
    let published = f.sessions.published(&f.id).result.unwrap();
    assert_eq!(published.revision, Revision(3));
    assert!(Arc::ptr_eq(&published.model, &model));
    // A check reads it, and asks the kernel nothing.
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert!(check.result.unwrap().problems.is_empty());
    assert_eq!(
        check.model,
        Some(ModelRead {
            revision: Revision(3),
            of: ModelChoice::Current,
            fault: None,
        })
    );
    assert_eq!(f.kernel.calls().len(), calls);
    assert_eq!(f.file(), BOX);
}

/// [`BOX`] and a cylinder clear of it, of radius 2 and height 5.
const BOX_AND_PIN: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [100, 0, 0], "radius": 2, "height": 5}]}"#;

#[test]
fn a_rollback_whose_begin_came_mid_rebuild_and_whose_rebuild_was_superseded_starts_one() {
    let f = opened(BOX);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    f.edit(BOX_AND_PIN);
    // Revision 2's rebuild started by a published read, its pin held.
    let hold = f.kernel.hold(is_cylinder);
    assert_eq!(
        f.sessions.published(&f.id).result.unwrap().revision,
        Revision::OPENED
    );
    assert!(f.kernel.wait_running(1, WAIT));
    assert_eq!(f.begin().base, Revision(2));
    // A check in the transaction supersedes revision 2's rebuild: its model never comes.
    f.edit(SPHERE);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let response = f.sessions.rollback(&f.id).wait();
    assert_eq!(response.result.unwrap().restored, Revision(2));
    // The rollback started the restored document's rebuild, as revision 4's.
    assert_eq!(lock(&f.session().rebuilds).running(), Some(Revision(4)));
    hold.release();
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert!(check.result.unwrap().problems.is_empty());
    assert_eq!(check.model.unwrap().revision, Revision(4));
    // The block, built before, is never asked for again; the pin, stopped, is.
    assert_eq!(f.made(is_box), 1);
    assert_eq!(f.made(is_cylinder), 2);
}

#[test]
fn a_rollback_restores_the_model_begins_rebuild_published_inside_the_transaction() {
    let f = opened(BOX);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    f.edit(BOX_AND_PIN);
    let hold = f.kernel.hold(is_cylinder);
    let _ = f.sessions.published(&f.id);
    assert!(f.kernel.wait_running(1, WAIT));
    f.begin();
    // Revision 2's rebuild publishes after begin, before anything supersedes it.
    hold.release();
    let deadline = std::time::Instant::now() + WAIT;
    let model = loop {
        let published = f.sessions.published(&f.id).result.unwrap();
        if published.revision == Revision(2) {
            break published.model;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "revision 2 never published"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    f.edit(SPHERE);
    let calls = f.kernel.calls().len();
    assert_eq!(
        f.sessions.rollback(&f.id).wait().result.unwrap().restored,
        Revision(2)
    );
    // That model, as revision 4's, with no rebuild started.
    let published = f.sessions.published(&f.id).result.unwrap();
    assert_eq!(published.revision, Revision(4));
    assert!(Arc::ptr_eq(&published.model, &model));
    assert_eq!(lock(&f.session().rebuilds).running(), None);
    assert_eq!(f.kernel.calls().len(), calls);
}

#[test]
fn dropping_the_sessions_mid_transaction_leaves_the_file_as_it_was() {
    let f = opened(STORED);
    f.begin();
    f.edit(&longer());
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .is_ok()
    );
    let Fixture {
        _dir,
        sessions,
        path,
        ..
    } = f;
    drop(sessions);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), STORED);
}

/// Two sketches, each a line from `pa` at (0, 0) to `pb` at (10, 0) on xy, held
/// horizontal; the second's `pb` at `x` instead.
fn two_sketches(x: &str) -> String {
    let sketch = |id: &str, name: &str, x: &str| {
        format!(
            r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "{name}", "plane": "xy", "points": [{{"id": "pa000000", "x": 0, "y": 0, "construction": false}}, {{"id": "pb000000", "x": {x}, "y": 0, "construction": false}}], "lines": [{{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}}], "circles": [], "arcs": [], "constraints": [{{"id": "k0000001", "type": "horizontal", "line": "na000000"}}]}}"#
        )
    };
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}, {}]}}"#,
        sketch("s0000001", "First", "10"),
        sketch("s0000002", "Second", x)
    )
}

/// Where `pb`, the second point, of sketch `sketch` in `text` is along x.
fn pb_x(text: &str, sketch: &str) -> f64 {
    let document = document::read(text.as_bytes()).unwrap();
    match &document.feature(&id(sketch)).unwrap().operation {
        document::Operation::Sketch(found) => found.points[1].x.mm(),
        _ => panic!("{sketch} isn't a sketch"),
    }
}

#[test]
fn a_commit_writes_the_solves_only_of_the_sketches_edits_reached() {
    // A solve from `pb` at 10 or 12 puts it 1 to the right, and from anywhere else
    // leaves it, so a solve loaded again stays (the values sent are the origin and the
    // axes' ends, 0 to 5, then `pa`'s x and y, 6 and 7, then `pb`'s, 8 and 9): both
    // sketches solve to other values than they store.
    let sketches = crate::fake::FakeSketches::new();
    sketches.answer(|system| {
        let mut solved = crate::fake::FakeSketches::as_sent(system);
        if solved.values[8] == 10.0 || solved.values[8] == 12.0 {
            solved.values[8] += 1.0;
        }
        Ok(solved)
    });
    let f = opened_solving(&two_sketches("10"), &sketches);
    f.begin();
    // The second's `pb` moved to 12: the edit reaches it alone.
    f.edit(&two_sketches("12"));
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert!(committed.written);
    assert_eq!(committed.resolved, [id("s0000002")]);
    assert!(committed.refingerprinted.is_empty());
    // The first's drift kept as stored, and said; the second's solve, 12 + 1, written.
    assert_eq!(committed.problems.total(), 1, "{:?}", committed.problems);
    let drift = &committed.problems.errors()[0];
    assert_eq!(
        (drift.code(), drift.feature(), drift.elements()),
        (
            ErrorCode::Drift,
            Some("s0000001"),
            &["pb000000".to_owned()][..]
        )
    );
    // Decision 31's words for a sketch: format, beside silico fmt, accepts its solve.
    assert_eq!(
        drift.suggestion(),
        Some(
            "if this sketch was edited by hand or in a merge since it was saved, run format (or silico fmt) to accept its new solve; if not, this is a determinism bug or a change in the sketch solver: find which before accepting it"
        )
    );
    // Its solve written, the second sketch isn't reached after the commit.
    assert!(f.session().held.read().reach().is_empty());
    let saved = f.file();
    assert_eq!(pb_x(&saved, "s0000001"), 10.0);
    assert_eq!(pb_x(&saved, "s0000002"), 13.0);
}

#[test]
fn a_published_read_answered_but_its_thread_still_ending_leaves_room_for_another() {
    // Eight reads whose threads haven't ended, each having answered (counted no more):
    // a ninth runs, since what's bounded is reads unanswered, not threads.
    let (_dir, _kernel, sessions, id) = at_two(BOX);
    let (release, released) = mpsc::channel::<()>();
    let released = Arc::new(Mutex::new(released));
    {
        let found = sessions.find(&id).unwrap();
        let mut reads = lock(&found.reads);
        let threads = reads.threads.as_mut().unwrap();
        for _ in 0..MAX_PUBLISHED_READS {
            let released = Arc::clone(&released);
            threads.push(std::thread::spawn(move || {
                let _ = lock(&released).recv();
            }));
        }
    }
    let ninth = sessions
        .read_model(&id, ReadCommand::Check, ModelChoice::Published, |read| {
            Ok(read.model_read().revision)
        })
        .wait();
    assert_eq!(ninth.result.unwrap(), Revision::OPENED);
    drop(release);
}

#[test]
fn a_rollback_restores_the_checked_form() {
    let f = opened(BOX);
    f.begin();
    f.edit(BOX_AND_PIN);
    f.sessions.rollback(&f.id).wait().result.unwrap();
    assert_eq!(f.sessions.status(&f.id).result.unwrap().features, Some(1));
    assert!(
        f.sessions
            .text(&f.id, Some(id("c0000001")))
            .wait()
            .result
            .is_err()
    );
}

#[test]
fn a_rollback_restores_what_edits_reached_so_drift_is_said_again() {
    let f = opened(STORED);
    f.begin();
    // Reaches the block and the fuse.
    f.edit(&STORED.replace(r#""length": 40"#, r#""length": 41"#));
    assert_eq!(
        f.session().held.read().reach(),
        BTreeSet::from([id("k7f2qx3m"), id("f0000001")])
    );
    f.sessions.rollback(&f.id).wait().result.unwrap();
    assert!(f.session().held.read().reach().is_empty());
    the_blocks_drift(
        &f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems,
    );
}

/// The lug, 20 long, can't be made, as `f`'s kernel is told.
fn refuse_the_longer_lug(f: &Fixture, code: ErrorCode) {
    f.kernel.refuse(move |op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 15.0 && *dx < 25.0)
            .then(|| Error::new(code, "the lug can't be made"))
    });
}

#[test]
fn a_rollback_restores_what_was_reached_before_begin() {
    let f = opened(STORED);
    refuse_the_longer_lug(&f, ErrorCode::KernelFailed);
    f.begin();
    f.edit(&longer());
    f.sessions.commit(&f.id).wait().result.unwrap();
    let before = BTreeSet::from([id("b0000001"), id("f0000001")]);
    assert_eq!(*f.session().held.read().carried, before);
    f.begin();
    f.edit(&f.file().replace("\"length\": 40,", "\"length\": 41,"));
    f.sessions.rollback(&f.id).wait().result.unwrap();
    assert_eq!(*f.session().held.read().carried, before);
    assert_eq!(f.session().held.read().reach(), before);
}

#[test]
fn format_of_no_features_accepts_no_drift() {
    let f = opened(STORED);
    let formatted = f
        .sessions
        .format(&f.id, Some(vec![]), false)
        .wait()
        .result
        .unwrap();
    assert!(formatted.accepted.is_empty());
    the_blocks_drift(&formatted.problems);
}

#[test]
fn a_commits_new_file_is_claimed() {
    let f = opened(STORED);
    f.begin();
    f.edit(&longer());
    assert!(f.sessions.commit(&f.id).wait().result.unwrap().written);
    let link = f.path.with_file_name("link.silico");
    std::fs::hard_link(&f.path, &link).unwrap();
    let response = f.sessions.open(&link, false).wait();
    let problems = response.result.unwrap_err();
    let refusal = &problems.errors()[0];
    assert!(
        refusal.message().ends_with("is already open in session s1"),
        "{refusal:?}"
    );
}

/// The code and feature of the fault `response`'s model read gives.
fn the_fault<T>(response: &Response<T>) -> (ErrorCode, Option<String>) {
    let fault = response.model.as_ref().unwrap().fault.as_ref().unwrap();
    (fault.code(), fault.feature().map(str::to_owned))
}

#[test]
fn a_commit_saves_around_a_kernel_fault_given_once_beside_its_result() {
    // Decision 32: the lug, 20 long, crashes the worker; the commit saves all the same, as
    // it saves around a failed feature: the lug and the fuse it stopped, which the edit
    // reached, with no fingerprint (decision 28, amended), and the fault given once, in the
    // model read, not among the problems.
    let f = opened(STORED);
    refuse_the_longer_lug(&f, ErrorCode::WorkerCrashed);
    f.begin();
    f.edit(&longer());
    let response = f.sessions.commit(&f.id).wait();
    assert_eq!(
        the_fault(&response),
        (ErrorCode::WorkerCrashed, Some("b0000001".to_owned()))
    );
    assert!(!response.session.as_ref().unwrap().transaction);
    let committed = response.result.clone().unwrap();
    assert!(committed.written);
    assert!(committed.refingerprinted.is_empty());
    the_blocks_drift(&committed.problems);
    assert_eq!(f.file(), REACHED_FAILED);
    // Their fingerprints unwritten, the lug and the fuse are still reached.
    assert_eq!(
        *f.session().held.read().carried,
        BTreeSet::from([id("b0000001"), id("f0000001")])
    );
    // The save wrote no solve, so its model, fault and all, is the saved revision's: a
    // check of it asks the kernel nothing, and gives the same fault.
    let saved = response.session.unwrap().revision;
    let asked = f.kernel.calls().len();
    let checked = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert_eq!(f.kernel.calls().len(), asked);
    assert_eq!(checked.model.as_ref().unwrap().revision, saved);
    assert_eq!(
        the_fault(&checked),
        (ErrorCode::WorkerCrashed, Some("b0000001".to_owned()))
    );
    the_blocks_drift(&checked.result.unwrap().problems);
}

#[test]
fn a_format_saves_around_a_kernel_fault_accepting_the_drift_it_can() {
    // The lug, 10 long as stored, times out: format writes the block's new fingerprint
    // (its drift accepted) and keeps the lug's and the fuse's as stored.
    let f = opened(STORED);
    f.kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 5.0 && *dx < 15.0)
            .then(|| Error::new(ErrorCode::WorkerTimeout, "the lug timed out"))
    });
    let response = f.sessions.format(&f.id, None, true).wait();
    assert_eq!(
        the_fault(&response),
        (ErrorCode::WorkerTimeout, Some("b0000001".to_owned()))
    );
    let formatted = response.result.clone().unwrap();
    assert!(formatted.written);
    assert_eq!(formatted.refingerprinted, [id("k7f2qx3m")]);
    assert_eq!(formatted.accepted, [id("k7f2qx3m")]);
    assert!(formatted.problems.is_empty(), "{:?}", formatted.problems);
    // The block's as rebuilt (a box's area by hand, 2 * (800 + 400 + 200)); the lug's
    // and the fuse's as stored.
    assert_eq!(
        stored_fingerprints(&f.file()),
        [
            Some(millionths(8_000_000_000, 2800)),
            Some(millionths(1_000_000_000, 600)),
            Some(millionths(9_000_000_000, 3400))
        ]
    );
    // As a commit's: the model, the block's drift accepted, is the saved revision's, so a
    // check of it asks the kernel nothing and finds no drift.
    let saved = response.session.unwrap().revision;
    assert_eq!(saved, Revision(2));
    let asked = f.kernel.calls().len();
    let checked = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert_eq!(f.kernel.calls().len(), asked);
    assert_eq!(checked.model.as_ref().unwrap().revision, saved);
    assert_eq!(
        the_fault(&checked),
        (ErrorCode::WorkerTimeout, Some("b0000001".to_owned()))
    );
    assert!(checked.result.unwrap().problems.is_empty());
}

#[test]
fn a_one_edit_commits_fault_is_the_saved_revisions_and_never_met_again() {
    // An edit outside a transaction, whose candidate's lug crashes the worker: saved around
    // it, and its model, fault and all, the saved revision's, so a check of that revision
    // asks the kernel nothing (decision 32: a fault isn't paid twice for one state).
    let f = opened(STORED);
    refuse_the_longer_lug(&f, ErrorCode::WorkerCrashed);
    let edited = f
        .sessions
        .replace_text(&f.id, Revision::OPENED, longer())
        .wait();
    assert_eq!(
        the_fault(&edited),
        (ErrorCode::WorkerCrashed, Some("b0000001".to_owned()))
    );
    let saved = edited.session.as_ref().unwrap().revision;
    assert!(edited.result.unwrap().commit.unwrap().written);
    let asked = f.kernel.calls().len();
    let checked = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert_eq!(f.kernel.calls().len(), asked);
    assert_eq!(checked.model.as_ref().unwrap().revision, saved);
    assert_eq!(
        the_fault(&checked),
        (ErrorCode::WorkerCrashed, Some("b0000001".to_owned()))
    );
    the_blocks_drift(&checked.result.unwrap().problems);
    // The candidate's rebuild, the saved revision's: its four steps (the block; the lug's
    // box and its move to x = 100; the fuse) all settled, the lug's crash and the steps it
    // blocked among them.
    assert_eq!(
        clockless_rebuild(f.sessions.status(&f.id).result.unwrap().rebuild),
        Rebuild {
            revision: saved,
            state: RebuildState::Faulted,
            progress: Some(Progress {
                done: 4,
                steps: 4,
                running: None,
                features: states(&[
                    ("k7f2qx3m", StepState::Built),
                    ("b0000001", StepState::Faulted),
                    ("f0000001", StepState::Blocked)
                ]),
                elapsed: Duration::ZERO,
            }),
        }
    );
}

/// A sketch, a line from `pa` at (0, 0) to `pb` at (`x`, 0) on xy, held horizontal; and a
/// box beside it, using nothing.
fn sketch_and_box(x: &str) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{{"id": "pa000000", "x": 0, "y": 0, "construction": false}}, {{"id": "pb000000", "x": {x}, "y": 0, "construction": false}}], "lines": [{{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}}], "circles": [], "arcs": [], "constraints": [{{"id": "k0000001", "type": "horizontal", "line": "na000000"}}]}}, {{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}}]}}"#
    )
}

#[test]
fn a_one_edit_commits_model_isnt_the_saved_revisions_once_it_writes_a_solve_fault_or_not() {
    // As above, but the save writes the sketch's solve (pb at 12 solves to 13): the saved
    // document is solved from the values it wrote, so the candidate's model, fault and all,
    // isn't the saved revision's, as a fault-free one isn't, and the saved revision's first
    // read rebuilds it, meeting the fault again.
    let sketches = crate::fake::FakeSketches::new();
    sketches.answer(|system| {
        let mut solved = crate::fake::FakeSketches::as_sent(system);
        if solved.values[8] == 12.0 {
            solved.values[8] += 1.0;
        }
        Ok(solved)
    });
    let f = opened_solving(&sketch_and_box("10"), &sketches);
    f.kernel
        .refuse(|op| is_box(op).then(|| Error::new(ErrorCode::WorkerCrashed, "crashed")));
    let edited = f
        .sessions
        .replace_text(&f.id, Revision::OPENED, sketch_and_box("12"))
        .wait();
    assert_eq!(
        the_fault(&edited),
        (ErrorCode::WorkerCrashed, Some("k7f2qx3m".to_owned()))
    );
    let commit = edited.result.unwrap().commit.unwrap();
    assert_eq!(commit.resolved, [id("s0000001")]);
    assert!(commit.problems.is_empty(), "{:?}", commit.problems);
    assert_eq!(pb_x(&f.file(), "s0000001"), 13.0);
    let (asked, solved) = (f.kernel.calls().len(), sketches.requests().len());
    let checked = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    // Rebuilt: the box asked for again, and the sketch solved again from 13.
    assert_eq!(
        (f.kernel.calls().len(), sketches.requests().len()),
        (asked + 1, solved + 1)
    );
    assert_eq!(
        the_fault(&checked),
        (ErrorCode::WorkerCrashed, Some("k7f2qx3m".to_owned()))
    );
    // The solve the save wrote holds: no drift of it.
    assert!(checked.result.unwrap().problems.is_empty());
}

#[test]
fn a_rollback_stops_the_transactions_rebuild() {
    let f = opened(BOX);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    f.begin();
    f.edit(SPHERE);
    let _hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })));
    let _ = f.sessions.published(&f.id);
    assert!(f.kernel.wait_running(1, WAIT));
    assert_eq!(lock(&f.session().rebuilds).running(), Some(Revision(2)));
    f.sessions.rollback(&f.id).wait().result.unwrap();
    assert_eq!(lock(&f.session().rebuilds).running(), None);
    // Stopped: the kernel's held sphere ends, though its hold is never released.
    let deadline = std::time::Instant::now() + WAIT;
    while f.kernel.wait_running(1, Duration::ZERO) {
        assert!(
            std::time::Instant::now() < deadline,
            "the transaction's rebuild still runs"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// [`BOX`] with a length of 0, which the gate refuses.
const FLAT: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 0, "width": 20, "height": 10}]}"#;

#[test]
fn a_commit_of_a_refused_document_unchanged_writes_nothing_and_ends() {
    let f = opened(FLAT);
    f.begin();
    let response = f.sessions.commit(&f.id).wait();
    let committed = response.result.unwrap();
    assert!(!committed.written);
    assert_eq!(committed.problems.errors()[0].parameter(), Some("length"));
    assert!(!response.session.unwrap().transaction);
    assert_eq!(f.file(), FLAT);
}

#[test]
fn a_commit_writes_over_no_drift_of_an_edit_undone() {
    // The block made 41 long, then 40 again, and a note put on the lug: the block's
    // stored fingerprint (1e-6 over) is kept and its drift said; only the note is new.
    let f = opened(STORED);
    f.begin();
    f.edit(&STORED.replace(r#""length": 40"#, r#""length": 41"#));
    f.edit(&STORED.replace(
        r#""name": "Lug", "#,
        r#""name": "Lug", "note": "Lifting lug", "#,
    ));
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert!(committed.written);
    assert!(committed.refingerprinted.is_empty());
    the_blocks_drift(&committed.problems);
    let saved = document::read(f.file().as_bytes()).unwrap();
    assert_eq!(
        saved.features[0].fingerprint.unwrap().volume,
        Measure::from_millionths(8_000_000_001)
    );
    assert_eq!(saved.features[1].note.as_deref(), Some("Lifting lug"));
}

#[test]
fn from_text_that_didnt_read_a_commit_reaches_only_features_with_new_ids() {
    // A merge's conflict markers around the lug's length: the text doesn't read. Mended
    // with the lug 20 long and a ball added, only the ball, whose ID the text hadn't, is
    // reached: the rest keep their stored fingerprints, and their drift is said.
    let markers = STORED.replace(
        r#""length": 10, "width": 10"#,
        "\n<<<<<<< ours\n\"length\": 10, \"width\": 10\n=======\n\"length\": 20, \"width\": 10\n>>>>>>> theirs\n",
    );
    let f = opened(&markers);
    assert!(matches!(*f.session().held.read().gated, Gated::Unread(_)));
    f.begin();
    let ball = r#", {"id": "z0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 50], "radius": 5}"#;
    let mended = longer().replacen("\n]}", &format!("{ball}\n]}}"), 1);
    assert_ne!(mended, longer());
    f.edit(&mended);
    assert_eq!(
        f.session().held.read().reach(),
        BTreeSet::from([id("z0000001")])
    );
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert!(committed.written);
    assert_eq!(committed.refingerprinted, [id("z0000001")]);
    let drifted: Vec<Option<&str>> = committed
        .problems
        .errors()
        .iter()
        .map(|e| e.feature())
        .collect();
    assert_eq!(
        drifted,
        [Some("k7f2qx3m"), Some("b0000001"), Some("f0000001")]
    );
}

#[test]
fn an_edit_after_text_that_didnt_read_was_mended_reaches_what_it_changed() {
    // The merge's markers around the lug's length, mended with the lug 20 long: every ID
    // was in the text, so the mend reaches nothing. Then an edit of the mended document,
    // the block 60 long: it reaches the block and the fuse, against the mend.
    let markers = STORED.replace(
        r#""length": 10, "width": 10"#,
        "\n<<<<<<< ours\n\"length\": 10, \"width\": 10\n=======\n\"length\": 20, \"width\": 10\n>>>>>>> theirs\n",
    );
    let f = opened(&markers);
    f.begin();
    f.edit(&longer());
    assert_eq!(f.session().held.read().reach(), BTreeSet::new());
    let block_60 = longer().replace(r#""length": 40"#, r#""length": 60"#);
    assert_ne!(block_60, longer());
    f.edit(&block_60);
    assert_eq!(
        f.session().held.read().reach(),
        BTreeSet::from([id("k7f2qx3m"), id("f0000001")])
    );
    // Undone, the edit after the mend reaches nothing; made again, what it changed.
    f.edit(&longer());
    assert_eq!(f.session().held.read().reach(), BTreeSet::new());
    f.edit(&block_60);
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert!(committed.written);
    assert_eq!(committed.refingerprinted, [id("k7f2qx3m"), id("f0000001")]);
    // Only the lug's drift, which the merge made and format accepts by name: the block and
    // the fuse were measured again.
    let drifted: Vec<(ErrorCode, Option<&str>)> = committed
        .problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature()))
        .collect();
    assert_eq!(drifted, [(ErrorCode::Drift, Some("b0000001"))]);
    // By hand: the block 60 × 20 × 10 = 12000, area 2(60·20 + 60·10 + 20·10) = 4000; the
    // fuse, the fake's two solids apart, 12000 + 2000 = 14000, area 4000 + 1000 = 5000,
    // to x = 120; the lug as stored.
    let saved = f.file();
    for fingerprint in [
        r#""fingerprint": {"volume": 12000, "area": 4000, "center": [30, 10, 5], "bbox": [0, 0, 0, 60, 20, 10], "faces": 6, "edges": 12, "vertices": 8}"#,
        r#""fingerprint": {"volume": 1000, "area": 600, "center": [105, 5, 5], "bbox": [100, 0, 0, 110, 10, 10], "faces": 6, "edges": 12, "vertices": 8}"#,
        r#""fingerprint": {"volume": 14000, "area": 5000, "center": [41.428571, 9.285714, 5], "bbox": [0, 0, 0, 120, 20, 10], "faces": 12, "edges": 24, "vertices": 16}"#,
    ] {
        assert!(saved.contains(fingerprint), "{fingerprint} in {saved}");
    }
}

#[test]
fn a_rollback_gives_begins_still_running_rebuild_to_the_new_revision() {
    // Revision 2's rebuild running at begin, its pin held; an edit, then a rollback: the
    // rebuild goes on, as revision 4's, and the pin is asked for once.
    let f = opened(BOX);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    f.edit(BOX_AND_PIN);
    let hold = f.kernel.hold(is_cylinder);
    let _ = f.sessions.published(&f.id);
    assert!(f.kernel.wait_running(1, WAIT));
    assert_eq!(f.begin().base, Revision(2));
    f.edit(SPHERE);
    f.sessions.rollback(&f.id).wait().result.unwrap();
    assert_eq!(lock(&f.session().rebuilds).running(), Some(Revision(4)));
    hold.release();
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert!(check.result.unwrap().problems.is_empty());
    assert_eq!(check.model.unwrap().revision, Revision(4));
    assert_eq!(f.made(is_cylinder), 1);
}

/// `f`'s check, which must answer within [`WAIT`]: if it doesn't (a revision left
/// running for good), the sessions are leaked rather than dropped, since their drop would
/// wait on the same rebuild, and the test fails rather than hangs.
fn check_in_time(f: Fixture) -> (Fixture, Response<CheckAnswer>) {
    match f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait_timeout(WAIT)
    {
        Ok(check) => (f, check),
        Err(_) => {
            std::mem::forget(f);
            panic!("the check never answered: a revision is left running");
        }
    }
}

/// Revision 2's rebuild running, its pin held, when a transaction begins; then `rollbacks`
/// transactions rolled back in a row, each begun from the one before's new revision,
/// before the pin is released: the rebuild is each new revision's in turn, and its model
/// the last's (1 + `rollbacks` + 1, the edit's 2 and a revision for each rollback).
fn rollbacks_while_begins_rebuild_runs(rollbacks: u64) {
    let f = opened(BOX);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    f.edit(BOX_AND_PIN);
    let hold = f.kernel.hold(is_cylinder);
    let _ = f.sessions.published(&f.id);
    assert!(f.kernel.wait_running(1, WAIT));
    for rolled in 1..=rollbacks {
        assert_eq!(f.begin().base, Revision(1 + rolled));
        f.sessions.rollback(&f.id).wait().result.unwrap();
        assert_eq!(
            lock(&f.session().rebuilds).running(),
            Some(Revision(2 + rolled))
        );
    }
    hold.release();
    let (f, check) = check_in_time(f);
    assert!(check.result.unwrap().problems.is_empty());
    assert_eq!(check.model.unwrap().revision, Revision(2 + rollbacks));
    // The one rebuild, never started again.
    assert_eq!(f.made(is_cylinder), 1);
}

#[test]
fn two_rollbacks_while_begins_rebuild_runs_still_end() {
    rollbacks_while_begins_rebuild_runs(2);
}

#[test]
fn three_rollbacks_while_begins_rebuild_runs_still_end() {
    rollbacks_while_begins_rebuild_runs(3);
}

#[test]
fn a_rollback_after_the_rebuild_a_rollback_was_given_published_restores_its_model() {
    // Revision 2's rebuild given to revision 3 by a rollback, then published as 3's; a
    // transaction from 3 rolled back: that model is revision 5's, nothing rebuilt.
    let f = opened(BOX);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    f.edit(BOX_AND_PIN);
    let hold = f.kernel.hold(is_cylinder);
    let _ = f.sessions.published(&f.id);
    assert!(f.kernel.wait_running(1, WAIT));
    f.begin();
    f.sessions.rollback(&f.id).wait().result.unwrap();
    hold.release();
    let deadline = std::time::Instant::now() + WAIT;
    let model = loop {
        let published = f.sessions.published(&f.id).result.unwrap();
        if published.revision == Revision(3) {
            break published.model;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "revision 3 never published"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(f.begin().base, Revision(3));
    f.edit(SPHERE);
    let calls = f.kernel.calls().len();
    f.sessions.rollback(&f.id).wait().result.unwrap();
    let published = f.sessions.published(&f.id).result.unwrap();
    assert_eq!(published.revision, Revision(5));
    assert!(Arc::ptr_eq(&published.model, &model));
    assert_eq!(lock(&f.session().rebuilds).running(), None);
    let (f, check) = check_in_time(f);
    assert!(check.result.unwrap().problems.is_empty());
    assert_eq!(check.model.unwrap().revision, Revision(5));
    assert_eq!(f.kernel.calls().len(), calls);
    assert_eq!(f.made(is_cylinder), 1);
}

#[test]
fn a_rollback_of_a_transaction_that_built_nothing_starts_nothing() {
    let f = opened(BOX);
    f.begin();
    f.edit(SPHERE);
    f.sessions.rollback(&f.id).wait().result.unwrap();
    assert_eq!(lock(&f.session().rebuilds).running(), None);
    assert!(f.kernel.calls().is_empty());
}

#[test]
fn a_rollback_restores_what_begin_saw_published_and_no_transaction_model_comes_after() {
    // Refused at begin, nothing published; the transaction's box built and published;
    // rolled back: nothing published again, and a late model of the transaction's
    // revision isn't either.
    let f = opened(FLAT);
    f.begin();
    f.edit(BOX);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    assert_eq!(
        f.sessions.published(&f.id).result.unwrap().revision,
        Revision(2)
    );
    f.sessions.rollback(&f.id).wait().result.unwrap();
    let refused = f.sessions.published(&f.id).result.unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("length"));
    let other = Recompute::new(f.kernel.client());
    watch(
        &Arc::downgrade(&f.session()),
        &Label::new(Revision(2), Reached::default()),
        other.start(checked(BOX)),
    );
    assert!(lock(&f.session().rebuilds).published.is_none());
}

#[test]
fn a_save_this_session_left_part_written_refuses_the_next_saying_where_the_text_is() {
    let f = opened(STORED);
    let old = f.path.with_file_name(".a.silico.fmt-1");
    *f.session().held.session_changing().part_written_mut() = Some(old.clone());
    // What a write in place cut short leaves: the first half of the new text.
    let half = &COMMITTED[..COMMITTED.len() / 2];
    std::fs::write(&f.path, half).unwrap();
    let response = f.sessions.format(&f.id, None, true).wait();
    let problems = response.result.unwrap_err();
    assert_eq!(problems.total(), 1);
    assert_eq!(
        problems.errors()[0],
        part_written(f.path.to_str().unwrap(), &old)
    );
    assert_eq!(problems.errors()[0].code(), ErrorCode::IoFailed);
    assert!(
        problems.errors()[0]
            .message()
            .ends_with(&format!("its text before is in {}", shown_path(&old))),
        "{:?}",
        problems.errors()[0]
    );
    assert_eq!(
        problems.errors()[0].suggestion(),
        Some(
            "nothing was written: put the old text back in the file, then run the command again, or close the session and open the file again"
        )
    );
    assert_eq!(f.file(), half);
    // As the advice says: the old text put back, the command run again writes.
    std::fs::write(&f.path, STORED).unwrap();
    let again = f.sessions.format(&f.id, None, true).wait();
    assert!(!again.session.unwrap().reloaded);
    assert!(again.result.unwrap().written);
    assert_eq!(f.session().held.read().part_written(), None);
    assert_ne!(f.file(), STORED);
}

#[test]
fn a_save_as_from_a_file_left_part_written_writes_and_says_where_its_old_text_is() {
    // The session's own file left part written: a Save As writes elsewhere all the same,
    // and, the session holding the new file now, lists the old one's mark among its
    // problems, so where the old text is isn't lost.
    let f = opened(STORED);
    let old = f.path.with_file_name(".a.silico.fmt-1");
    // Marked inside the transaction, where nothing reads the file again (a read that finds
    // it as the session wrote it clears the mark).
    f.sessions.begin(&f.id).wait().result.unwrap();
    *f.session().held.session_changing().part_written_mut() = Some(old.clone());
    let to = f.path.with_file_name("b.silico");
    let response = f.sessions.save_as(&f.id, &to, false).wait();
    let committed = response.result.unwrap();
    assert!(committed.written);
    assert!(to.exists());
    // Beside STORED's drift, one: the old file's, last.
    let io: Vec<_> = committed
        .problems
        .errors()
        .iter()
        .filter(|e| e.code() == ErrorCode::IoFailed)
        .collect();
    assert_eq!(io.len(), 1);
    let problem = committed.problems.errors().last().unwrap();
    assert_eq!(problem.code(), ErrorCode::IoFailed);
    assert_eq!(problem.parameter(), Some("path"));
    assert_eq!(
        problem.message(),
        format!(
            "{} was left part written by this session's own save, which couldn't put its text back: its text before is in {}",
            shown_path(&f.path),
            shown_path(&old)
        )
    );
    assert_eq!(f.session().held.read().part_written(), None);
}

/// `f`'s Save As to `to` started and held just before it writes, `meanwhile` done, then
/// let through: its answer.
fn save_as_held(
    f: &Fixture,
    to: &Path,
    replace: bool,
    meanwhile: impl FnOnce(),
) -> Response<Committed> {
    f.sessions.begin(&f.id).wait().result.unwrap();
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().writing.shut(started, wait);
    let saving = f.sessions.save_as(&f.id, to, replace);
    was_started.recv_timeout(WAIT).unwrap();
    meanwhile();
    through.send(()).unwrap();
    saving.wait()
}

#[test]
fn a_save_as_claims_its_path_before_it_writes_so_no_session_opens_it_meanwhile() {
    // Held after its path is claimed and before it writes: an open of the path, making the
    // file there, is refused as the Save As's session's; let go, the Save As writes it and
    // holds it.
    let f = opened(STORED);
    let to = f.path.with_file_name("b.silico");
    let mut refused = None;
    let response = save_as_held(&f, &to, false, || {
        refused = Some(f.sessions.open(&to, true).wait());
    });
    let refused = refused.unwrap();
    let problems = refused.result.unwrap_err();
    assert_eq!(problems.errors()[0].code(), ErrorCode::InvalidInput);
    assert_eq!(
        problems.errors()[0].message(),
        format!("{} is already open in session {}", shown_path(&to), f.id)
    );
    let committed = response.result.unwrap();
    assert!(committed.written);
    assert_eq!(f.session().held.read().path(), to.to_str());
    assert_eq!(
        std::fs::read(&to).unwrap(),
        f.session().held.read().text.as_slice()
    );
}

#[test]
fn a_save_as_whose_path_a_link_is_put_on_after_its_check_writes_nothing() {
    let f = opened(STORED);
    let (to, elsewhere) = (
        f.path.with_file_name("b.silico"),
        f.path.with_file_name("c.silico"),
    );
    std::fs::write(&elsewhere, "another file").unwrap();
    let response = save_as_held(&f, &to, true, || {
        std::os::unix::fs::symlink(&elsewhere, &to).unwrap();
    });
    let problems = response.result.unwrap_err();
    assert_eq!(problems.errors()[0].code(), ErrorCode::Conflict);
    assert_eq!(problems.errors()[0].parameter(), Some("path"));
    assert_eq!(
        problems.errors()[0].suggestion(),
        Some("nothing was written: commit again")
    );
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "another file");
    assert_eq!(f.session().held.read().path(), f.path.to_str());
}

#[test]
fn a_save_as_whose_path_becomes_another_sessions_document_after_its_check_writes_nothing() {
    let f = opened(STORED);
    let theirs = f.path.with_file_name("theirs.silico");
    std::fs::write(&theirs, BOX).unwrap();
    f.sessions.open(&theirs, false).wait().result.unwrap();
    let to = f.path.with_file_name("b.silico");
    let response = save_as_held(&f, &to, true, || {
        std::fs::hard_link(&theirs, &to).unwrap();
    });
    let problems = response.result.unwrap_err();
    assert_eq!(problems.errors()[0].code(), ErrorCode::InvalidInput);
    assert!(
        problems.errors()[0]
            .message()
            .ends_with("is a document a session holds: a commit never writes over one"),
        "{:?}",
        problems.errors()[0]
    );
    assert_eq!(std::fs::read_to_string(&to).unwrap(), BOX);
    assert_eq!(f.session().held.read().path(), f.path.to_str());
}

#[test]
fn a_file_this_sessions_own_save_left_part_written_isnt_reloaded() {
    let f = opened(STORED);
    let old = f.path.with_file_name(".a.silico.fmt-1");
    *f.session().held.session_changing().part_written_mut() = Some(old.clone());
    // What its save left: neither what the session wrote nor what was there.
    std::fs::write(&f.path, "part written").unwrap();
    let text = f.sessions.text(&f.id, None).wait();
    let session = text.session.unwrap();
    assert_eq!(
        (session.revision, session.reloaded),
        (Revision::OPENED, false)
    );
    assert_eq!(text.result.unwrap().text, STORED);
    // And the next save says where the old text is.
    let problems = f
        .sessions
        .format(&f.id, None, true)
        .wait()
        .result
        .unwrap_err();
    assert_eq!(
        problems.errors()[0],
        part_written(f.path.to_str().unwrap(), &old)
    );
    assert_eq!(f.file(), "part written");
}

#[test]
fn a_read_needs_the_strongest_of_its_sides() {
    // Fable's 6d decision, by hand: a revision by number reads nothing; null, outside a
    // transaction; "disk", inside one too; and a read of several sides, the strongest.
    assert!(Rereads::Never < Rereads::OutsideTransaction);
    assert!(Rereads::OutsideTransaction < Rereads::Always);
    let one = Some(Named::Revision(Revision::OPENED));
    let disk = Some(Named::Disk);
    for (sides, need) in [
        (vec![], Rereads::Never),
        (vec![one], Rereads::Never),
        (vec![one, one], Rereads::Never),
        (vec![None], Rereads::OutsideTransaction),
        (vec![one, None], Rereads::OutsideTransaction),
        (vec![None, one], Rereads::OutsideTransaction),
        (vec![disk], Rereads::Always),
        (vec![one, disk], Rereads::Always),
        (vec![disk, None], Rereads::Always),
        (vec![None, disk], Rereads::Always),
        (vec![disk, disk], Rereads::Always),
    ] {
        assert_eq!(Rereads::needed(sides.clone()), need, "{sides:?}");
    }
}

#[test]
fn a_disk_side_of_a_file_left_part_written_is_refused_as_the_next_save_would_be() {
    // Fable's 6d decision: the session knows the file isn't what it holds, so its held
    // bytes never stand in for it; the refusal is the one the next save gives, naming the
    // side, outside a transaction and inside one alike.
    let f = opened(STORED);
    let old = f.path.with_file_name(".a.silico.fmt-1");
    *f.session().held.session_changing().part_written_mut() = Some(old.clone());
    std::fs::write(&f.path, "part written").unwrap();
    let path = f.path.to_str().unwrap();
    let one = Some(Named::Revision(Revision::OPENED));
    for transaction in [false, true] {
        if transaction {
            f.sessions.begin(&f.id).wait().result.unwrap();
        }
        let response = f.sessions.diff(&f.id, one, Some(Named::Disk)).wait();
        let session = response.session.unwrap();
        assert_eq!(
            (session.revision, session.reloaded),
            (Revision::OPENED, false)
        );
        assert_eq!(
            response.result.unwrap_err().errors(),
            [part_written(path, &old).with_parameter("to")]
        );
        let text = f.sessions.text_at(&f.id, None, Some(Named::Disk)).wait();
        assert_eq!(
            text.result.unwrap_err().errors(),
            [part_written(path, &old).with_parameter("revision")]
        );
        // A revision by number answers, the file unread.
        assert_eq!(
            f.sessions
                .diff(&f.id, one, one)
                .wait()
                .result
                .unwrap()
                .changes,
            []
        );
    }
    // The old text put back: the read clears the mark, and `"disk"` is the file.
    std::fs::write(&f.path, STORED).unwrap();
    let response = f.sessions.diff(&f.id, one, Some(Named::Disk)).wait();
    assert_eq!(response.result.unwrap().changes, []);
    assert_eq!(f.session().held.read().part_written(), None);
}

/// Makes `dir` read-only until dropped: a save there can't make its file beside.
struct ReadOnly(PathBuf);

impl ReadOnly {
    fn of(dir: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        ReadOnly(dir.to_owned())
    }
}

impl Drop for ReadOnly {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn a_save_that_cant_be_written_names_path_for_commit_and_format() {
    let f = opened(STORED);
    let dir = f.path.parent().unwrap().to_owned();
    f.begin();
    f.edit(&longer());
    let _read_only = ReadOnly::of(&dir);
    // Root (in a container, say) writes in a read-only directory all the same: then no
    // save here fails, and the test has nothing to show, so it ends.
    let probe = dir.join("probe");
    if std::fs::write(&probe, "").is_ok() {
        let _ = std::fs::remove_file(&probe);
        return;
    }
    let commit = f.sessions.commit(&f.id).wait();
    let problems = commit.result.unwrap_err();
    let refused = &problems.errors()[0];
    assert_eq!(
        (refused.code(), refused.parameter()),
        (ErrorCode::IoFailed, Some("path")),
        "{refused:?}"
    );
    assert!(commit.session.unwrap().transaction);
    f.sessions.rollback(&f.id).wait().result.unwrap();
    let format = f.sessions.format(&f.id, None, true).wait();
    let problems = format.result.unwrap_err();
    let refused = &problems.errors()[0];
    assert_eq!(
        (refused.code(), refused.parameter()),
        (ErrorCode::IoFailed, Some("path")),
        "{refused:?}"
    );
    assert_eq!(f.file(), STORED);
}

#[test]
fn a_rollback_of_a_closed_session_starts_no_rebuild() {
    // A rebuild started in the transaction, so a rollback would start the restored
    // document's; but the session has closed (its sessions dropped, say): none starts.
    let f = opened(BOX);
    f.begin();
    f.edit(SPHERE);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let session = f.session();
    session.close();
    let calls = f.kernel.calls().len();
    session.rollback(&probe()).unwrap();
    assert_eq!(lock(&session.rebuilds).running(), None);
    assert_eq!(f.kernel.calls().len(), calls);
}

/// A 10 mm cube, its fingerprint stored as the fake measures it: 10³ = 1000 mm³, area
/// 6 × 100 = 600.
const CUBE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Cube", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10, "fingerprint": {"volume": 1000, "area": 600, "center": [5, 5, 5], "bbox": [0, 0, 0, 10, 10, 10], "faces": 6, "edges": 12, "vertices": 8}}]}"#;

/// `f`'s model of the revision it's at, built by a check and published, put back with
/// the cube measured 1e40 mm³: 1e46 millionths, past the 1.7e38 an i128 holds, so no
/// fingerprint can hold it.
fn plant_unholdable(f: &Fixture) {
    let revision = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait()
        .model
        .unwrap()
        .revision;
    let session = f.session();
    let mut rebuilds = lock(&session.rebuilds);
    let published = rebuilds.published.as_mut().unwrap();
    assert_eq!(published.revision, revision);
    let cube = id("k7f2qx3m");
    let Some(FeatureOutcome::Built { built, .. }) = published.model.outcome(&cube) else {
        panic!("the cube didn't build");
    };
    let mut summary = built.summary().unwrap().clone();
    summary.volume = 1e40;
    published.model = Arc::new(published.model.measured_as(&cube, summary));
    assert!(!published.model.records(&cube));
}

/// The one problem `problems` holds: the cube's drift, measured past a fingerprint.
#[track_caller]
fn the_cubes_unholdable_drift(problems: &Problems) {
    assert_eq!(problems.total(), 1, "{problems:?}");
    let drift = &problems.errors()[0];
    assert_eq!(
        (drift.code(), drift.feature(), drift.parameter()),
        (ErrorCode::Drift, Some("k7f2qx3m"), Some("fingerprint")),
        "{drift:?}"
    );
}

#[test]
fn measures_no_fingerprint_holds_are_drift_however_reached_or_accepted() {
    // Nothing takes the stored record's place: format accepts nothing, and an edit that
    // reaches the cube still has its drift said.
    let f = opened(CUBE);
    plant_unholdable(&f);
    let formatted = f.sessions.format(&f.id, None, false).wait().result.unwrap();
    assert!(formatted.accepted.is_empty(), "{:?}", formatted.accepted);
    the_cubes_unholdable_drift(&formatted.problems);
    f.begin();
    f.edit(&CUBE.replace(r#""length": 10"#, r#""length": 11"#));
    assert_eq!(
        f.session().held.read().reach(),
        BTreeSet::from([id("k7f2qx3m")])
    );
    plant_unholdable(&f);
    the_cubes_unholdable_drift(
        &f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems,
    );
}

#[test]
fn a_save_that_left_the_file_part_written_is_marked_for_the_next() {
    let f = opened(STORED);
    let session = f.session();
    let failed = || Error::new(ErrorCode::IoFailed, "couldn't be written");
    // Its old text put back: nothing marked.
    let problems = session.unsaved(
        "a.silico",
        file::Unsaved {
            error: failed(),
            old_text_in: None,
        },
        Saver::Format,
    );
    assert_eq!(problems.errors()[0].parameter(), Some("path"));
    assert_eq!(session.held.read().part_written(), None);
    // Left part written, its old text beside it: marked, so the next save says where.
    let old = f.path.with_file_name(".a.silico.fmt-1");
    let problems = session.unsaved(
        "a.silico",
        file::Unsaved {
            error: failed(),
            old_text_in: Some(old.clone()),
        },
        Saver::Format,
    );
    assert_eq!(
        (
            problems.errors()[0].code(),
            problems.errors()[0].parameter()
        ),
        (ErrorCode::IoFailed, Some("path"))
    );
    assert_eq!(session.held.read().part_written(), Some(&old));
}

#[test]
fn a_format_after_a_reached_feature_builds_writes_its_record_and_carries_it_no_more() {
    // As `a_reached_feature_that_failed_stays_reached_after_its_commit`: the lug and the
    // fuse carried after the commit. Then the lug made 30 long, which builds: a format
    // writes both records, so neither is carried. By hand: the lug 30 × 10 × 10 = 3000,
    // area 2(300 + 100 + 300) = 1400; the fuse 8000 + 3000 = 11000, area 2800 + 1400 =
    // 4200.
    let f = opened(STORED);
    refuse_the_longer_lug(&f, ErrorCode::KernelFailed);
    f.begin();
    f.edit(&longer());
    assert!(f.sessions.commit(&f.id).wait().result.unwrap().written);
    assert_eq!(
        *f.session().held.read().carried,
        BTreeSet::from([id("b0000001"), id("f0000001")])
    );
    f.edit(&f.file().replace("\"length\": 20,", "\"length\": 30,"));
    let formatted = f.sessions.format(&f.id, None, true).wait().result.unwrap();
    assert!(formatted.written);
    assert!(f.session().held.read().carried.is_empty());
    assert!(f.session().held.read().reach().is_empty());
    let saved = document::read(f.file().as_bytes()).unwrap();
    let measures: Vec<(Measure, Measure)> = saved.features[1..]
        .iter()
        .map(|feature| {
            let fingerprint = feature.fingerprint.unwrap();
            (fingerprint.volume, fingerprint.area)
        })
        .collect();
    let mm = |whole: i128| Measure::from_millionths(whole * 1_000_000);
    assert_eq!(measures, [(mm(3000), mm(1400)), (mm(11000), mm(4200))]);
}

#[test]
fn a_reload_carries_nothing_so_a_hand_edits_drift_is_said_and_not_written_over() {
    // As `a_reached_feature_that_failed_stays_reached_after_its_commit`: the lug and the
    // fuse carried after the commit, saved with no fingerprint. Then the lug made 30 long
    // by hand, which builds (30 × 10 × 10 = 3000; the fuse 11000). A hand edit reaches
    // nothing, and a reload is as a fresh open: the block's drift (stored 1e-6 over) is
    // said, and nothing of the lug and the fuse, which have no fingerprint to hold; and
    // nothing is carried, so a commit that reaches nothing writes neither's.
    let f = opened(STORED);
    refuse_the_longer_lug(&f, ErrorCode::KernelFailed);
    f.begin();
    f.edit(&longer());
    assert!(f.sessions.commit(&f.id).wait().result.unwrap().written);
    assert_eq!(
        *f.session().held.read().carried,
        BTreeSet::from([id("b0000001"), id("f0000001")])
    );
    let by_hand = f.file().replace("\"length\": 20,", "\"length\": 30,");
    assert_ne!(by_hand, f.file());
    std::fs::write(&f.path, &by_hand).unwrap();
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    let session = check.session.unwrap();
    assert!(session.reloaded);
    let drift: Vec<(ErrorCode, Option<&str>)> = check
        .result
        .as_ref()
        .unwrap()
        .problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature()))
        .collect();
    assert_eq!(drift, [(ErrorCode::Drift, Some("k7f2qx3m"))]);
    assert!(f.session().held.read().carried.is_empty());
    // A note's edit reaches nothing: its commit writes no fingerprint over the drift.
    let note = f
        .sessions
        .set_field(
            &f.id,
            session.revision,
            id("k7f2qx3m"),
            "note".to_owned(),
            crate::document::json::read(br#""by hand""#).unwrap(),
        )
        .wait()
        .result
        .unwrap();
    let commit = note.commit.unwrap();
    assert!(commit.written);
    assert!(commit.refingerprinted.is_empty(), "{commit:?}");
    assert_eq!(
        stored_fingerprints(&f.file()),
        [Some(millionths(8_000_000_001, 2800)), None, None]
    );
}

#[test]
fn a_save_moves_only_its_own_sessions_claim() {
    // Two sessions, s1 on a.silico and s2 on b.silico; s1's format writes a.silico anew:
    // b.silico is still s2's, by its file too, so a hard link to it is refused.
    let f = opened(STORED);
    let other = f.path.with_file_name("b.silico");
    std::fs::write(&other, BOX).unwrap();
    let second = f.sessions.open(&other, false).wait().session.unwrap().id;
    assert_eq!(second, SessionId(2));
    assert!(
        f.sessions
            .format(&f.id, None, true)
            .wait()
            .result
            .unwrap()
            .written
    );
    let link = f.path.with_file_name("link.silico");
    std::fs::hard_link(&other, &link).unwrap();
    let problems = f.sessions.open(&link, false).wait().result.unwrap_err();
    let refusal = &problems.errors()[0];
    assert_eq!(refusal.parameter(), Some("path"), "{refusal:?}");
    assert!(
        refusal.message().ends_with("is already open in session s2"),
        "{refusal:?}"
    );
}

#[test]
fn a_commit_from_text_that_still_doesnt_read_writes_nothing_and_ends() {
    let f = opened("{");
    f.begin();
    let response = f.sessions.commit(&f.id).wait();
    let committed = response.result.unwrap();
    assert!(!committed.written);
    assert!(!response.session.unwrap().transaction);
    assert_eq!(f.file(), "{");
}

#[test]
fn a_commit_of_a_document_the_gate_took_at_begin_and_refuses_now_is_refused_and_left_open() {
    // Taken at begin; then held refused, set here: no command holds one inside a
    // transaction (each edit goes through the gate, and a file changed outside isn't
    // reloaded inside one, decision 2), but the commit mustn't rely on it. The commit is
    // refused with the gate's problems, the document stays held as it is, and the
    // transaction open.
    let f = opened(STORED);
    f.begin();
    let session = f.session();
    {
        let mut held = session.held.session_changing();
        let next = held.next().unwrap();
        let carried = Arc::clone(&held.carried);
        held.change(
            next,
            Arc::new(FLAT.as_bytes().to_vec()),
            Arc::new(Gated::of(FLAT.as_bytes())),
            carried,
        );
    }
    let response = f.sessions.commit(&f.id).wait();
    let problems = response.result.unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    assert_eq!(
        (
            problems.errors()[0].feature(),
            problems.errors()[0].parameter()
        ),
        (Some("k7f2qx3m"), Some("length"))
    );
    let after = response.session.unwrap();
    assert_eq!((after.revision, after.transaction), (Revision(2), true));
    assert_eq!(
        f.sessions.text(&f.id, None).wait().result.unwrap().text,
        FLAT
    );
    assert_eq!(f.file(), STORED);
}

/// The bytes that map to `k7f2qx3m`, by hand: its characters' places in the alphabet,
/// 19, 7, 15, 2, 23, 29, 3, 20, five bits each (10011 00111 01111 00010 10111 11101 00011
/// 10100), eight at a time.
const BLOCKS_ID: [u8; 5] = [0x99, 0xde, 0x2b, 0xf4, 0x74];

/// The bytes that map to `12345678`: 00001 00010 ... 01000.
const DIGITS_ID: [u8; 5] = [0x08, 0x86, 0x42, 0x98, 0xe8];

#[test]
fn a_feature_added_without_an_id_takes_the_next_id_the_document_doesnt_hold() {
    let f = opened(BOX);
    *lock(&f.sessions.registry.feature_ids) = FeatureIds::given([BLOCKS_ID, DIGITS_ID]);
    f.begin();
    let ball = crate::document::json::read(
        br#"{"type": "sphere", "name": "Ball", "position": [0, 0, 50], "radius": 5}"#,
    )
    .unwrap();
    let added = f
        .sessions
        .add_feature(&f.id, Revision::OPENED, None, ball)
        .wait()
        .result
        .unwrap();
    // The block's ID first, which the document holds: passed over.
    assert_eq!(added.feature.id, id("12345678"));
}

/// Waits, up to [`WAIT`], until `done` says the threads it looks at have ended.
fn ended(done: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + WAIT;
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "the threads never ended"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn all_ended(threads: &Threads) -> bool {
    threads.running.iter().all(JoinHandle::is_finished)
}

#[test]
fn published_reads_that_ended_are_joined_as_the_next_starts() {
    let f = opened(BOX);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let read = || {
        f.sessions
            .read_model(&f.id, ReadCommand::Check, ModelChoice::Published, |read| {
                Ok(read.model_read().revision)
            })
            .wait()
            .result
            .unwrap()
    };
    let session = f.session();
    let threads = || {
        lock(&session.reads)
            .threads
            .as_ref()
            .map(|t| t.running.len())
    };
    for _ in 0..4 {
        assert_eq!(read(), Revision::OPENED);
    }
    ended(|| all_ended(lock(&session.reads).threads.as_ref().unwrap()));
    // The four ended, joined as the fifth starts: its thread alone is held.
    assert_eq!(read(), Revision::OPENED);
    assert_eq!(threads(), Some(1));
}

#[test]
fn session_threads_that_ended_are_joined_as_the_next_opens() {
    let f = opened(BOX);
    let other = f.path.with_file_name("b.silico");
    std::fs::write(&other, BOX).unwrap();
    for _ in 0..3 {
        let id = f.sessions.open(&other, false).wait().session.unwrap().id;
        f.sessions.close(&id).wait().result.unwrap();
    }
    let book = || lock(&f.sessions.registry.book);
    // Each closed session's thread ends (those not joined yet); s1's runs on.
    ended(|| {
        book()
            .threads
            .running
            .iter()
            .filter(|thread| !thread.is_finished())
            .count()
            == 1
    });
    f.sessions.open(&other, false).wait().session.unwrap();
    // The three ended, joined as the next opens: s1's thread and the new one's.
    assert_eq!(book().threads.running.len(), 2);
}

#[test]
fn a_cancel_after_the_command_kept_its_work_says_so_and_it_answers_its_result() {
    // Decision 34: a command past the point where its work leaves a trace (an edit saved)
    // isn't stopped: `cancel` says `stopped: false`, and the command answers as it would.
    let f = opened(BOX);
    let (go, wait) = mpsc::channel::<()>();
    let (kept, was_kept) = mpsc::channel::<()>();
    let running = f
        .sessions
        .queued(&f.id, SessionCommand::Text, move |_, stop, _| {
            stop.settle()?;
            kept.send(()).unwrap();
            wait.recv().unwrap();
            Ok(7)
        });
    was_kept.recv_timeout(WAIT).unwrap();
    assert_eq!(
        f.sessions
            .cancel(&f.id, SessionCommand::Text)
            .result
            .unwrap(),
        Cancelled {
            command: SessionCommand::Text.into(),
            stopped: false
        }
    );
    assert!(!running.cancel());
    go.send(()).unwrap();
    assert_eq!(running.wait().result.unwrap(), 7);
}

#[test]
fn a_command_cancelled_before_it_kept_its_work_answers_cancelled_whatever_it_gives() {
    let f = opened(BOX);
    let (go, wait) = mpsc::channel::<()>();
    let (started, was_started) = mpsc::channel::<()>();
    let running = f
        .sessions
        .queued(&f.id, SessionCommand::Text, move |_, _, _| {
            started.send(()).unwrap();
            wait.recv().unwrap();
            Ok(7)
        });
    was_started.recv_timeout(WAIT).unwrap();
    assert!(
        f.sessions
            .cancel(&f.id, SessionCommand::Text)
            .result
            .unwrap()
            .stopped
    );
    go.send(()).unwrap();
    let response = running.wait();
    assert_eq!(response.result.unwrap_err().errors(), [cancelled("text")]);
    assert_eq!(response.session.unwrap().id, f.id);
}

#[test]
fn an_open_cancelled_before_it_opens_leaves_no_session_and_no_file_it_made() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let (go, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let (asked, was_asked) = mpsc::channel::<()>();
    // The cache is opened once the file is made and read: held there.
    let sessions = Sessions::with_cache_opener(FakeKernel::new(1).unwrap(), move |cache| {
        asked.send(()).unwrap();
        lock(&wait).recv().unwrap();
        Cache::with_secret(cache, &secret)
    });
    let made = dir.path().join("new.silico");
    let opening = sessions.open(&made, true);
    was_asked.recv_timeout(WAIT).unwrap();
    assert!(made.exists());
    assert!(opening.cancel());
    go.send(()).unwrap();
    let response = opening.wait();
    assert_eq!(response.session, None);
    assert_eq!(response.result.unwrap_err().errors(), [cancelled("open")]);
    assert!(!made.exists());
    let book = lock(&sessions.registry.book);
    assert!(book.sessions.is_empty() && book.opening.is_empty() && book.files.is_empty());
}

/// `f`'s session, and the stop of a command cancelled already: as the work of a command
/// whose cancel came before it reached the point where it leaves a trace.
fn cancelled_meanwhile(f: &Fixture) -> (Arc<Session<FakeClient>>, Arc<Ticket>) {
    let ticket = probe();
    assert!(ticket.cancel());
    (f.session(), ticket)
}

#[test]
fn a_transactions_work_a_cancel_came_before_leaves_no_trace() {
    // Its revision's model published, so the commit reaches its save without waiting: there,
    // and at each change of what the session holds, the cancel is found first.
    let f = opened(STORED);
    f.begin();
    let edited = f.edit(&longer());
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .is_ok()
    );
    let (session, stop) = cancelled_meanwhile(&f);
    let refused = |result: Result<(), Problems>| {
        assert_eq!(result.unwrap_err().errors(), [cancelled("text")]);
    };
    let mut read = None;
    refused(session.commit(&stop, None, &mut read, &|_| {}).map(|_| ()));
    assert_eq!(f.file(), STORED);
    refused(session.applied(&stop, STORED).map(|_| ()));
    refused(session.rollback(&stop).map(|_| ()));
    let held = session.held.read();
    assert_eq!(held.revision, edited);
    assert!(held.transaction.is_some());
}

#[test]
fn a_commit_writing_nothing_a_cancel_came_before_leaves_the_transaction_open() {
    let f = opened(STORED);
    f.begin();
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .is_ok()
    );
    let (session, stop) = cancelled_meanwhile(&f);
    let mut read = None;
    let refused = session.commit(&stop, None, &mut read, &|_| {});
    assert_eq!(refused.unwrap_err().errors(), [cancelled("text")]);
    assert!(session.held.read().transaction.is_some());
}

#[test]
fn work_outside_a_transaction_a_cancel_came_before_leaves_no_trace() {
    let f = opened(STORED);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .is_ok()
    );
    let (session, stop) = cancelled_meanwhile(&f);
    let mut read = None;
    let format = session.format(&stop, None, true, &mut read, &|_| {});
    assert_eq!(format.unwrap_err().errors(), [cancelled("text")]);
    assert_eq!(f.file(), STORED);
    assert_eq!(
        session.begin(&stop).unwrap_err().errors(),
        [cancelled("text")]
    );
    assert!(session.held.read().transaction.is_none());
    assert_eq!(
        session.shut(&stop).unwrap_err().errors(),
        [cancelled("text")]
    );
    assert!(!session.closed());
    assert_eq!(session.held.read().revision, Revision::OPENED);
}

#[test]
fn a_cancel_that_comes_during_a_save_is_too_late_and_the_save_stands() {
    // Decision 34: the save settles the command's stop before it writes, so a cancel that
    // comes once the file's written (here, as the new file is claimed) says it came too
    // late, and the save, and the session's move to the saved revision, stand.
    let f = opened(STORED);
    assert!(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .is_ok()
    );
    let session = f.session();
    let stop = probe();
    let late = std::cell::Cell::new(None);
    let mut read = None;
    let formatted = session
        .format(&stop, None, true, &mut read, &|_| {
            late.set(Some(stop.cancel()))
        })
        .unwrap();
    assert_eq!(late.get(), Some(false));
    assert!(!stop.cancelled());
    assert!(formatted.written);
    assert_ne!(f.file(), STORED);
    assert_eq!(f.file().as_bytes(), &**session.held.read().bytes);
    assert_eq!(session.held.read().revision, Revision(2));
}

#[test]
fn each_change_of_what_the_session_holds_is_past_a_cancel() {
    // Each command's change of what the session holds settles its stop as it changes it,
    // so a cancel after says it came too late: begin, an edit held in a transaction, a
    // rollback, a commit that writes nothing, and close.
    let f = opened(STORED);
    let session = f.session();
    let settled = |stop: &Ticket| assert!(!stop.cancel(), "{}'s stop wasn't settled", stop.name());
    let stop = Ticket::new(SessionCommand::Begin);
    session.begin(&stop).unwrap();
    settled(&stop);
    let stop = Ticket::new(SessionCommand::ReplaceText);
    assert_eq!(session.applied(&stop, &longer()).unwrap(), Revision(2));
    settled(&stop);
    let stop = Ticket::new(SessionCommand::Rollback);
    session.rollback(&stop).unwrap();
    settled(&stop);
    session.begin(&probe()).unwrap();
    let stop = Ticket::new(SessionCommand::Commit);
    let mut read = None;
    let committed = session.commit(&stop, None, &mut read, &|_| {}).unwrap();
    assert!(!committed.written);
    assert!(session.held.read().transaction.is_none());
    settled(&stop);
    let stop = Ticket::new(SessionCommand::Close);
    session.shut(&stop).unwrap();
    assert!(session.closed());
    settled(&stop);
}

#[test]
fn the_cli_words_each_fault_as_a_session_does_trying_again_by_running_its_command() {
    // The design's section 6: the CLI words a fault with the session's function, trying
    // again by running its command, said once; a timeout never says "allow it more time"; a
    // crash on the request's values is the worker's advice alone. Each expected suggestion
    // is the design's own quote of the CLI's wording, for `silico check`.
    use crate::worker::{Reached, WorkerKind, crash_advice, timeout_advice};
    use std::os::unix::process::ExitStatusExt;
    let signalled = |signal| Some(std::process::ExitStatus::from_raw(signal));
    let cli = |fault: &Error| fault_suggestion(fault, TryAgainBy::Running("check"));
    let crashed = |signal, reached| {
        Error::new(ErrorCode::WorkerCrashed, "the worker crashed")
            .with_feature("f0000001")
            .with_advice(crash_advice(WorkerKind::Kernel, signalled(signal), reached))
    };
    assert_eq!(
        cli(&crashed(libc::SIGSEGV, Reached::Request)),
        "change the operation's values: these crashed the kernel, and the same request would \
         crash it again"
    );
    assert_eq!(
        cli(&crashed(libc::SIGTERM, Reached::Request)),
        "run `silico check` to try it again: the kernel worker was stopped from outside, \
         stopped responding, or was gone before the request reached it, not by these values"
    );
    assert_eq!(
        cli(&crashed(libc::SIGKILL, Reached::Request)),
        "run `silico check` to try it once more: the kernel worker was killed, most likely by \
         the system for running out of memory; if it's killed again, the request needs too much \
         memory: simplify it"
    );
    let unstarted = Error::new(ErrorCode::WorkerTimeout, "the worker took too long")
        .with_advice(timeout_advice(WorkerKind::Kernel, false));
    assert_eq!(
        cli(&unstarted),
        "run `silico check` to try it again: the kernel worker hadn't started in the time \
         allowed (a new worker binary's first launch can be slow while the system checks it); if \
         it never starts, check Silico's install"
    );
    let timed_out = Error::new(ErrorCode::WorkerTimeout, "the worker took too long")
        .with_advice(timeout_advice(WorkerKind::Kernel, true));
    assert_eq!(
        cli(&timed_out),
        "simplify the operation, or run `silico check` to try it again"
    );
    // Any other advice of the worker's own (its install, here), then trying again.
    let exited =
        Error::new(ErrorCode::WorkerCrashed, "the worker crashed").with_advice(crash_advice(
            WorkerKind::Kernel,
            Some(std::process::ExitStatus::from_raw(1 << 8)),
            Reached::Request,
        ));
    let own = exited.suggestion().unwrap().to_owned();
    assert_eq!(
        cli(&exited),
        format!("{own}; then run `silico check` to try it again")
    );
    // With none of its own.
    let bare = Error::new(ErrorCode::WorkerCrashed, "the worker crashed").with_feature("f0000001");
    assert_eq!(
        cli(&bare),
        "run `silico check` to try it again; if the same feature crashes the worker again, \
         report it with the document"
    );
    let bare = Error::new(ErrorCode::WorkerTimeout, "the worker took too long");
    assert_eq!(cli(&bare), "run `silico check` to try it again");
    // A command names itself; the retry is said once in every case.
    assert_eq!(
        fault_suggestion(&bare, TryAgainBy::Running("export")),
        "run `silico export` to try it again"
    );
    for fault in [crashed(libc::SIGTERM, Reached::Request), timed_out, exited] {
        let words = cli(&fault);
        assert_eq!(words.matches("try it").count(), 1, "{words}");
        assert!(!words.contains("allow it more time"), "{words}");
    }
}

#[test]
fn a_kept_fault_rewords_the_workers_advice_for_a_session_then_says_to_check_with_fresh() {
    // Decision 32 and the design's section 6: reading the revision again won't run it
    // again, so the worker's own advice, reworded for a session, and how to try again. The
    // workers' advice as worker.rs makes it (`crash_advice`, `timeout_advice`), its words
    // written out as it gives them; the session's, as the design's note gives them.
    use crate::worker::{Reached, WorkerKind, crash_advice, timeout_advice};
    use std::os::unix::process::ExitStatusExt;
    let signalled = |signal| Some(std::process::ExitStatus::from_raw(signal));
    let said = |error: &Error| {
        (
            error.code(),
            error.message().to_owned(),
            error.feature().map(str::to_owned),
            error.suggestion().map(str::to_owned),
        )
    };
    // A crash on the request's values, which the same values would crash again: edit them,
    // which rebuilds anyway, never check with fresh.
    let crashed = Error::new(ErrorCode::WorkerCrashed, "the worker crashed")
        .with_feature("f0000001")
        .with_advice(crash_advice(
            WorkerKind::Kernel,
            signalled(libc::SIGSEGV),
            Reached::Request,
        ));
    assert_eq!(
        crashed.suggestion(),
        Some(
            "change the operation's values: these crashed the kernel, and the same request \
             would crash it again"
        )
    );
    // Worded for a session, it keeps the advice its words come from: a client of the
    // session's refusal (an `elements` of the feature) reads that it would crash again.
    assert!(kept_fault(&crashed).would_crash_again());
    assert_eq!(
        said(&kept_fault(&crashed)),
        (
            ErrorCode::WorkerCrashed,
            "the worker crashed".to_owned(),
            Some("f0000001".to_owned()),
            Some(
                "edit the feature's values: these crashed the kernel, and the same request \
                 would crash it again; an edit makes a new revision, which rebuilds it"
                    .to_owned()
            )
        )
    );
    // The sketch solver's (`WorkerKind::runs`), likewise.
    let solver = Error::new(ErrorCode::WorkerCrashed, "the worker crashed")
        .with_feature("s0000001")
        .with_advice(crash_advice(
            WorkerKind::Sketch,
            signalled(libc::SIGBUS),
            Reached::Request,
        ));
    assert_eq!(
        kept_fault(&solver).suggestion(),
        Some(
            "edit the feature's values: these crashed the sketch solver, and the same request \
             would crash it again; an edit makes a new revision, which rebuilds it"
        )
    );
    // With no feature named, the worker's advice alone: there's no feature to edit, and
    // the same request would crash it again.
    let unnamed_values =
        Error::new(ErrorCode::WorkerCrashed, "the worker crashed").with_advice(crash_advice(
            WorkerKind::Kernel,
            signalled(libc::SIGSEGV),
            Reached::Request,
        ));
    assert_eq!(
        kept_fault(&unnamed_values).suggestion(),
        Some(
            "change the operation's values: these crashed the kernel, and the same request \
             would crash it again"
        )
    );
    // Advice in the same words that the worker didn't give as a crash's is no crash's: the
    // advice as data decides, not the words (a kernel's refusal, worker.rs's
    // `KernelFailed`, here; and a crash's words given as a suggestion of their own).
    let refused = Error::new(ErrorCode::KernelFailed, "refused")
        .with_feature("f0000001")
        .with_suggestion("change the operation's values: the kernel can't build it from these");
    assert_eq!(
        kept_fault(&refused).suggestion(),
        Some(
            "change the operation's values: the kernel can't build it from these; then check \
             with fresh to try it again"
        )
    );
    let worded = crashed.clone().with_suggestion(
        "change the operation's values: these crashed the kernel, and the same request would \
         crash it again",
    );
    assert_eq!(
        kept_fault(&worded).suggestion(),
        Some(
            "change the operation's values: these crashed the kernel, and the same request \
             would crash it again; then check with fresh to try it again"
        )
    );
    // A timeout once the worker had started: the time allowed isn't the command API's to
    // give, so not said.
    let timed_out = Error::new(ErrorCode::WorkerTimeout, "timed out")
        .with_feature("f0000001")
        .with_advice(timeout_advice(WorkerKind::Kernel, true));
    assert_eq!(
        timed_out.suggestion(),
        Some("simplify the operation, or allow it more time")
    );
    assert_eq!(
        kept_fault(&timed_out).suggestion(),
        Some("simplify the operation, or check with fresh to try it again")
    );
    // One that hadn't started: check with fresh in the place of the worker's retry, so a
    // retry is said once.
    let unstarted = Error::new(ErrorCode::WorkerTimeout, "timed out")
        .with_feature("f0000001")
        .with_advice(timeout_advice(WorkerKind::Kernel, false));
    assert_eq!(
        unstarted.suggestion(),
        Some(
            "try the request again: the kernel worker hadn't started in the time allowed (a \
             new worker binary's first launch can be slow while the system checks it); if it \
             never starts, check Silico's install"
        )
    );
    assert_eq!(
        kept_fault(&unstarted).suggestion(),
        Some(
            "check with fresh to try it again: the kernel worker hadn't started in the time \
             allowed (a new worker binary's first launch can be slow while the system checks \
             it); if it never starts, check Silico's install"
        )
    );
    // A worker stopped from outside, and one killed (most likely for memory): likewise,
    // once more for the kill.
    let outside = Error::new(ErrorCode::WorkerCrashed, "the worker crashed")
        .with_feature("f0000001")
        .with_advice(crash_advice(
            WorkerKind::Kernel,
            signalled(libc::SIGTERM),
            Reached::Request,
        ));
    assert_eq!(
        kept_fault(&outside).suggestion(),
        Some(
            "check with fresh to try it again: the kernel worker was stopped from outside, \
             stopped responding, or was gone before the request reached it, not by these values"
        )
    );
    assert!(!kept_fault(&outside).would_crash_again());
    let killed = Error::new(ErrorCode::WorkerCrashed, "the worker crashed")
        .with_feature("f0000001")
        .with_advice(crash_advice(
            WorkerKind::Kernel,
            signalled(libc::SIGKILL),
            Reached::Request,
        ));
    assert_eq!(
        kept_fault(&killed).suggestion(),
        Some(
            "check with fresh to try it once more: the kernel worker was killed, most likely \
             by the system for running out of memory; if it's killed again, the request needs \
             too much memory: simplify it"
        )
    );
    // Killed before it started (worker.rs gives a worker that never started this code):
    // a retry, said once, as check with fresh's, then what to do if it happens again.
    let killed_unstarted = Error::new(ErrorCode::WorkerUnavailable, "the worker was killed")
        .with_advice(crash_advice(
            WorkerKind::Kernel,
            signalled(libc::SIGKILL),
            Reached::NotStarted,
        ));
    assert_eq!(
        kept_fault(&killed_unstarted).suggestion(),
        Some(
            "check with fresh to try it again: the kernel worker was killed before it started; \
             if it's killed before starting again, check Silico's install (macOS kills a \
             worker binary whose code signature is broken)"
        )
    );
    // The worker's install: its advice, then check with fresh.
    let exited = Error::new(ErrorCode::WorkerCrashed, "the worker crashed")
        .with_feature("f0000001")
        .with_advice(crash_advice(
            WorkerKind::Kernel,
            Some(std::process::ExitStatus::from_raw(3 << 8)),
            Reached::Request,
        ));
    assert_eq!(
        kept_fault(&exited).suggestion(),
        Some(
            "check Silico's install: the kernel worker exited on its own (see its last \
             output), which no change to the request will help; report it if it persists; \
             then check with fresh to try it again"
        )
    );
    // Out of memory (silico-protocol's `OUT_OF_MEMORY`).
    let short = Error::new(ErrorCode::ResourceExhausted, "out of memory")
        .with_feature("f0000001")
        .with_suggestion("free some memory, or make the part or sketch simpler, and try again");
    assert_eq!(
        said(&kept_fault(&short)),
        (
            ErrorCode::ResourceExhausted,
            "out of memory".to_owned(),
            Some("f0000001".to_owned()),
            Some(
                "free some memory, or make the part or sketch simpler, and try again; then \
                 check with fresh to try it again"
                    .to_owned()
            )
        )
    );
    // A crash with no advice of its own: the design's example's words, exactly.
    let bare_crash =
        Error::new(ErrorCode::WorkerCrashed, "the worker crashed").with_feature("f0000001");
    assert_eq!(
        kept_fault(&bare_crash).suggestion(),
        Some(
            "check with fresh to try it again; if the same feature crashes the worker again, \
             report it with the document"
        )
    );
    // Any other with none: check with fresh alone.
    let bare_timeout = Error::new(ErrorCode::WorkerTimeout, "timed out").with_feature("f0000001");
    assert_eq!(
        kept_fault(&bare_timeout).suggestion(),
        Some("check with fresh to try it again")
    );
    // A worker that couldn't be had says nothing of the feature that asked for one: its
    // own suggestion, then check with fresh; with none of its own, check with fresh alone,
    // never the design's crash words (it names no feature).
    let unavailable = Error::new(ErrorCode::WorkerUnavailable, "no worker")
        .with_feature("f0000001")
        .with_suggestion("restart Silico");
    assert_eq!(
        said(&kept_fault(&unavailable)),
        (
            ErrorCode::WorkerUnavailable,
            "no worker".to_owned(),
            None,
            Some("restart Silico; then check with fresh to try it again".to_owned())
        )
    );
    let bare = Error::new(ErrorCode::WorkerUnavailable, "no worker");
    assert_eq!(
        kept_fault(&bare).suggestion(),
        Some("check with fresh to try it again")
    );
    let unnamed_crash = Error::new(ErrorCode::WorkerCrashed, "the worker crashed");
    assert_eq!(
        kept_fault(&unnamed_crash).suggestion(),
        Some("check with fresh to try it again")
    );
}

#[test]
fn a_rebuild_a_cancel_stopped_stays_stopped_once_its_watcher_has_ended() {
    // The watcher records the stopped run's end, superseded, after the check has answered:
    // joined here, so what it recorded is what status and the published read see.
    let f = opened(BOX);
    let _hold = f.kernel.hold(is_box);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    assert!(f.kernel.wait_running(1, WAIT));
    assert!(
        f.sessions
            .cancel(&f.id, SessionCommand::Check)
            .result
            .unwrap()
            .stopped
    );
    assert_eq!(
        check.wait().result.unwrap_err().errors(),
        [cancelled("check")]
    );
    let watchers = std::mem::replace(&mut lock(&f.session().rebuilds).watchers, Threads::new());
    assert!(!watchers.running.is_empty());
    watchers.join();
    assert_eq!(
        f.sessions.status(&f.id).result.unwrap().rebuild.state,
        RebuildState::Stopped
    );
    let published = f.sessions.published(&f.id).result.unwrap_err();
    assert_eq!(
        published.errors()[0].message(),
        "session s1 has published no model yet: its rebuild was cancelled"
    );
    assert_eq!(f.made(is_box), 1);
}

#[test]
fn a_command_cancelled_after_its_sessions_reload_says_it_reloaded() {
    // The reload is the session's own work, not the command's: it stands, and the
    // command's `cancelled` answer gives the new revision and says it reloaded.
    let f = opened(BOX);
    let outside = BOX.replace("\"Block\"", "\"Plate\"");
    std::fs::write(&f.path, &outside).unwrap();
    let (go, wait) = mpsc::channel::<()>();
    let (started, was_started) = mpsc::channel::<()>();
    let running = f
        .sessions
        .queued(&f.id, SessionCommand::Text, move |_, _, _| {
            started.send(()).unwrap();
            wait.recv().unwrap();
            Ok(7)
        });
    was_started.recv_timeout(WAIT).unwrap();
    assert!(
        f.sessions
            .cancel(&f.id, SessionCommand::Text)
            .result
            .unwrap()
            .stopped
    );
    go.send(()).unwrap();
    let response = running.wait();
    assert_eq!(response.result.unwrap_err().errors(), [cancelled("text")]);
    let session = response.session.unwrap();
    assert_eq!((session.revision, session.reloaded), (Revision(2), true));
    assert_eq!(&**f.session().held.read().text, outside.as_bytes());
}

/// A stand-in for what a cancel or a close interrupts (M4.5's read client): each why it's
/// told, in order.
struct Told(Mutex<Vec<Why>>);

impl Interrupt for Told {
    fn interrupt(&self, why: Why) {
        lock(&self.0).push(why);
    }
}

impl Told {
    fn new() -> Arc<Self> {
        Arc::new(Told(Mutex::new(Vec::new())))
    }

    fn whys(&self) -> Vec<Why> {
        lock(&self.0).clone()
    }

    fn weak(self: &Arc<Self>) -> Weak<dyn Interrupt> {
        Arc::downgrade(self) as Weak<dyn Interrupt>
    }
}

#[test]
fn a_sessions_close_interrupts_its_reads_as_closed_and_each_counted_after_at_once() {
    let f = opened(BOX);
    let session = f.session();
    let unanswered = Arc::clone(&lock(&session.reads).unanswered);
    let running = Ticket::new(ReadCommand::Check.into());
    let _counted = Counted::count(&unanswered, ReadCommand::Check, &running);
    let before = Told::new();
    // Given twice, told once.
    running.interrupt_with(before.weak());
    running.interrupt_with(before.weak());
    assert!(before.whys().is_empty());
    session.close();
    assert_eq!(before.whys(), [Why::Closed]);
    // Closed isn't cancelled: the read answers as of a session that closed.
    assert!(!running.cancelled());
    // Sticky: a read counted after the close is closed at once, so what it adds is told at
    // once.
    let late = Ticket::new(ReadCommand::Check.into());
    let _late = Counted::count(&unanswered, ReadCommand::Check, &late);
    let after = Told::new();
    late.interrupt_with(after.weak());
    assert_eq!(after.whys(), [Why::Closed]);
    // A cancel says it was cancelled.
    assert!(running.cancel());
    assert_eq!(before.whys(), [Why::Closed, Why::Cancelled]);
}

#[test]
fn a_format_that_writes_a_solve_publishes_no_model_and_the_next_check_solves_again() {
    // The sketch's pb, stored at 10, solves to 11: format writes the solve, so its model,
    // solved from the stored values, isn't the saved revision's, and the check after it
    // solves again, from 11, which holds.
    let sketches = crate::fake::FakeSketches::new();
    sketches.answer(|system| {
        let mut solved = crate::fake::FakeSketches::as_sent(system);
        if solved.values[8] == 10.0 {
            solved.values[8] += 1.0;
        }
        Ok(solved)
    });
    let f = opened_solving(&sketch_and_box("10"), &sketches);
    let response = f.sessions.format(&f.id, None, true).wait();
    let saved = response.session.unwrap().revision;
    let formatted = response.result.unwrap();
    assert!(formatted.written);
    assert_eq!(formatted.resolved, [id("s0000001")]);
    assert_eq!(pb_x(&f.file(), "s0000001"), 11.0);
    let solved = sketches.requests().len();
    let checked = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert_eq!(sketches.requests().len(), solved + 1);
    assert_eq!(checked.model.unwrap().revision, saved);
    assert!(checked.result.unwrap().problems.is_empty());
}

#[test]
fn a_reads_client_interrupted_by_its_stop_is_superseded_and_one_added_after_at_once() {
    // M4.5's distance: a read's own kernel client, given to its stop, is retired when the
    // read is cancelled or its session closes, and one given after either at once (found by
    // the tests review: a published read can register its client after its session's close
    // has stopped the rest). One given before answers until then.
    let kernel = FakeKernel::new(1).unwrap();
    let make = || {
        KernelRequest::make(Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        })
        .unwrap()
    };
    for why in [Why::Closed, Why::Cancelled] {
        let stop = Ticket::new(SessionCommand::Distance);
        let before = Arc::new(ReadClient::new(kernel.client()));
        stop.interrupt_with(Arc::downgrade(&before) as Weak<dyn Interrupt>);
        let asked = before.submit(before.generation(), make()).wait();
        assert!(matches!(asked, Outcome::Done(Ok(_))), "{why:?} {asked:?}");
        match why {
            Why::Closed => stop.close(),
            Why::Cancelled => assert!(stop.cancel()),
        }
        let asked = before.submit(before.generation(), make()).wait();
        assert!(matches!(asked, Outcome::Superseded), "{why:?} {asked:?}");
        let after = Arc::new(ReadClient::new(kernel.client()));
        stop.interrupt_with(Arc::downgrade(&after) as Weak<dyn Interrupt>);
        let asked = after.submit(after.generation(), make()).wait();
        assert!(matches!(asked, Outcome::Superseded), "{why:?} {asked:?}");
    }
}

#[test]
fn a_reads_client_is_its_kernels_and_advances_it() {
    // The mutation run: a read's client's identity and advance went untested. It names the
    // kernel it's a client of, and advancing it supersedes what it asks for after, at the
    // generation it began at.
    let kernel = FakeKernel::new(1).unwrap();
    let client = ReadClient::new(kernel.client());
    assert_eq!(client.identity(), kernel.identity());
    client.advance(crate::kernel::Generation(u64::MAX));
    let asked = client
        .submit(
            client.generation(),
            KernelRequest::make(Primitive::Box {
                dx: 1.0,
                dy: 1.0,
                dz: 1.0,
            })
            .unwrap(),
        )
        .wait();
    assert!(matches!(asked, Outcome::Superseded), "{asked:?}");
}

/// `progress` but its clock, which no test can pin: its `elapsed` zero.
fn clockless(progress: Option<Progress>) -> Option<Progress> {
    progress.map(|progress| Progress {
        elapsed: Duration::ZERO,
        ..progress
    })
}

/// `rebuild` but its progress's clock ([`clockless`]).
fn clockless_rebuild(rebuild: Rebuild) -> Rebuild {
    Rebuild {
        progress: clockless(rebuild.progress),
        ..rebuild
    }
}

/// Polls `progress` until it's `want` but its clock ([`clockless`]), for at most [`WAIT`].
#[track_caller]
fn until_progress(progress: impl Fn() -> Option<Progress>, want: Option<Progress>) {
    let started = std::time::Instant::now();
    while clockless(progress()) != want {
        assert!(started.elapsed() < WAIT, "{:?}, not {want:?}", progress());
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// [`STORED`]'s lug's box (10 long), and [`longer`]'s (20 long): never the block (40).
fn is_lug(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx < 25.0)
}

#[test]
fn a_held_rebuild_shows_its_steps_done_below_its_steps_then_all_of_them() {
    // STORED's steps, by hand: the block at the origin (one), the lug at x = 100 (its box
    // and its move: two), and the fuse (one): four. The lug's box held: the block's step
    // settles, and the move and the fuse wait on the lug.
    let f = opened(STORED);
    let hold = f.kernel.hold(is_lug);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    let one_of_four = Some(Progress {
        done: 1,
        steps: 4,
        running: Some("b0000001 box".to_owned()),
        features: states(&[
            ("k7f2qx3m", StepState::Built),
            ("b0000001", StepState::Running),
            ("f0000001", StepState::Queued),
        ]),
        elapsed: Duration::ZERO,
    });
    until_progress(|| check.progress(), one_of_four.clone());
    assert_eq!(
        clockless_rebuild(f.sessions.status(&f.id).result.unwrap().rebuild),
        Rebuild {
            revision: Revision::OPENED,
            state: RebuildState::Running,
            progress: one_of_four.clone(),
        }
    );
    hold.release();
    check.wait().result.unwrap();
    assert_eq!(
        clockless_rebuild(f.sessions.status(&f.id).result.unwrap().rebuild),
        Rebuild {
            revision: Revision::OPENED,
            state: RebuildState::Done,
            progress: Some(Progress {
                done: 4,
                steps: 4,
                running: None,
                features: states(&[
                    ("k7f2qx3m", StepState::Built),
                    ("b0000001", StepState::Built),
                    ("f0000001", StepState::Built)
                ]),
                elapsed: Duration::ZERO,
            }),
        }
    );
    // Nothing running now: a command's handle has no progress to give.
    let text = f.sessions.text(&f.id, None);
    assert_eq!(text.progress(), None);
    text.wait().result.unwrap();
}

#[test]
fn counting_a_rebuilds_steps_never_keeps_status_or_a_published_read_waiting() {
    // Revision 1 built and published; then, in a transaction, the lug 20 long, its
    // rebuild's lug held, its steps being counted. Status, the check's handle, and a read of
    // the published model each answer while the step is held, none waiting for it.
    let f = opened(STORED);
    f.sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap();
    f.begin();
    f.edit(&longer());
    let hold = f.kernel.hold(is_lug);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    // Revision 2's: the block and the fuse unchanged, but the fuse uses the lug, so only
    // the block's step is kept; the lug's box is held.
    until_progress(
        || check.progress(),
        Some(Progress {
            done: 1,
            steps: 4,
            running: Some("b0000001 box".to_owned()),
            features: states(&[
                ("k7f2qx3m", StepState::Built),
                ("b0000001", StepState::Running),
                ("f0000001", StepState::Queued),
            ]),
            elapsed: Duration::ZERO,
        }),
    );
    // A step counts as running as its job starts, before its request reaches the kernel
    // (recompute's `serve`): held only once the kernel has it, so wait for that, or the
    // answers below could come before the lug was ever held, not while it was.
    assert!(
        f.kernel.wait_running(1, WAIT),
        "the lug never reached the kernel"
    );
    let quick = Duration::from_secs(5);
    let (said, heard) = mpsc::channel();
    let answered = std::thread::scope(|scope| {
        scope.spawn(|| {
            let status = f.sessions.status(&f.id).result.map(|status| status.rebuild);
            let read = f
                .sessions
                .read_model(&f.id, ReadCommand::Check, ModelChoice::Published, |read| {
                    Ok(read.model_read().revision)
                })
                .wait()
                .result;
            let _ = said.send((status, read, f.kernel.wait_running(1, Duration::ZERO)));
        });
        let answered = heard.recv_timeout(quick);
        // Let go either way, so a call that waited on the step ends, and the test with it.
        hold.release();
        answered
    });
    let (status, read, still_held) = answered.unwrap();
    assert!(still_held, "answered while the lug was held");
    assert_eq!(
        clockless_rebuild(status.unwrap()),
        Rebuild {
            revision: Revision(2),
            state: RebuildState::Running,
            progress: Some(Progress {
                done: 1,
                steps: 4,
                running: Some("b0000001 box".to_owned()),
                features: states(&[
                    ("k7f2qx3m", StepState::Built),
                    ("b0000001", StepState::Running),
                    ("f0000001", StepState::Queued)
                ]),
                elapsed: Duration::ZERO,
            }),
        }
    );
    assert_eq!(read.unwrap(), Revision::OPENED);
    check.wait().result.unwrap();
}

#[test]
fn a_published_reads_handle_reports_no_rebuilds_progress() {
    // Revision 1 built and published; then, in a transaction, the lug 20 long, its rebuild's
    // lug held. The check waits on that rebuild and reports it; a read of the published
    // model, running while the step is held, waits on no rebuild and reports none.
    let f = opened(STORED);
    f.sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap();
    f.begin();
    f.edit(&longer());
    let hold = f.kernel.hold(is_lug);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    let lug_held = Some(Progress {
        done: 1,
        steps: 4,
        running: Some("b0000001 box".to_owned()),
        features: states(&[
            ("k7f2qx3m", StepState::Built),
            ("b0000001", StepState::Running),
            ("f0000001", StepState::Queued),
        ]),
        elapsed: Duration::ZERO,
    });
    until_progress(|| check.progress(), lug_held.clone());
    let (go, held) = mpsc::channel::<()>();
    let read = f.sessions.read_model(
        &f.id,
        ReadCommand::Check,
        ModelChoice::Published,
        move |read| {
            let _ = held.recv_timeout(WAIT);
            Ok(read.model_read().revision)
        },
    );
    assert_eq!(read.progress(), None);
    assert_eq!(clockless(check.progress()), lug_held);
    go.send(()).unwrap();
    assert_eq!(read.wait().result.unwrap(), Revision::OPENED);
    hold.release();
    check.wait().result.unwrap();
}

#[test]
fn an_open_with_no_session_ids_left_is_refused_taking_none() {
    // The next number u64::MAX, with none after it to count on to: the open is refused,
    // taking no ID, the book as it was.
    let f = opened(STORED);
    lock(&f.sessions.registry.book).next = u64::MAX;
    let other = f.path.with_file_name("b.silico");
    std::fs::write(&other, STORED).unwrap();
    let response = f.sessions.open(&other, false).wait();
    assert!(response.session.is_none());
    let problems = response.result.unwrap_err();
    assert_eq!(problems.total(), 1);
    let refused = &problems.errors()[0];
    assert_eq!(
        (refused.code(), refused.message(), refused.suggestion()),
        (
            ErrorCode::ResourceExhausted,
            "this process has no session IDs left to give",
            Some("start Silico again")
        )
    );
    let book = lock(&f.sessions.registry.book);
    assert_eq!(book.next, u64::MAX);
    assert!(book.opening.is_empty());
    assert_eq!(book.sessions.keys().copied().collect::<Vec<_>>(), [f.id]);
    drop(book);
    // Its file isn't held: once an ID is free to give, it opens.
    lock(&f.sessions.registry.book).next = 2;
    assert!(f.sessions.open(&other, false).wait().result.is_ok());
}

#[test]
fn a_closed_sessions_rebuild_watchers_are_the_books_to_join() {
    // A session that rebuilt once, closed: the thread that watched its rebuild is moved
    // into the book with its reads, which the drop joins, and the session keeps none.
    let f = opened(BOX);
    let other = f.path.with_file_name("b.silico");
    std::fs::write(&other, BOX).unwrap();
    let id = f.sessions.open(&other, false).wait().session.unwrap().id;
    f.sessions
        .check(&id, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap();
    let session = f.sessions.find(&id).unwrap();
    assert_eq!(lock(&session.rebuilds).watchers.running.len(), 1);
    assert!(lock(&f.sessions.registry.book).released.running.is_empty());
    f.sessions.close(&id).wait().result.unwrap();
    assert!(lock(&session.rebuilds).watchers.running.is_empty());
    assert_eq!(lock(&f.sessions.registry.book).released.running.len(), 1);
}

/// Starts the command and gives a wait for its result (its answer's result alone, whatever
/// it answers).
type Start<'a> = Box<dyn Fn() -> Box<dyn FnOnce() -> Result<(), Problems>> + 'a>;

/// What a command's handle answers, its value dropped.
fn erased<T: Send + 'static>(running: Running<T>) -> Box<dyn FnOnce() -> Result<(), Problems>> {
    Box::new(move || running.wait().result.map(|_| ()))
}

#[test]
fn status_names_each_command_running_and_cancel_by_that_name_stops_it() {
    // Every command that runs in a session, started by its own call and held at the gate
    // as it starts: status names it (the queued command running, or, read of the published
    // model, among the reads), and cancel by that name stops it before it has done
    // anything, so it answers cancelled. The reads, queued, read the current model.
    let f = opened(BOX);
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().gate.shut(started, wait);
    let s = &f.sessions;
    let at = &f.id;
    let length = || crate::document::json::read(b"41").unwrap();
    let pair = || ["Face1".to_owned(), "Face2".to_owned()];
    let front = || crate::command::render::Render {
        view: Some(crate::views::NamedView::Front),
        camera: None,
        width: 100,
        height: 100,
        highlight: vec![],
        to: None,
        replace: false,
    };
    // Drawn by a fake renderer, as the sessions' render tests draw.
    let renderer = Arc::new(crate::fake::FakeRenderer::new());
    *lock(&f.sessions.registry.renderer) = Some(Arc::new(Arc::clone(&renderer)));
    let reads = |model: ModelChoice| -> Vec<(ReadCommand, Start<'_>)> {
        vec![
            (
                ReadCommand::Check,
                Box::new(move || {
                    erased(s.check(
                        at,
                        match model {
                            ModelChoice::Current => CheckModel::Current { fresh: false },
                            ModelChoice::Published => CheckModel::Published,
                        },
                    ))
                }),
            ),
            (
                ReadCommand::Elements,
                Box::new(move || erased(s.elements(at, model, None, None, 10))),
            ),
            (
                ReadCommand::Query,
                Box::new(move || {
                    erased(s.query(
                        at,
                        model,
                        crate::command::elements::Query::of(crate::kernel::ElementKind::Face, 10),
                    ))
                }),
            ),
            (
                ReadCommand::MassProperties,
                Box::new(move || erased(s.mass_properties(at, model, None))),
            ),
            (
                ReadCommand::Distance,
                Box::new(move || erased(s.distance(at, model, pair(), None))),
            ),
            (
                ReadCommand::Angle,
                Box::new(move || erased(s.angle(at, model, pair(), None))),
            ),
            (
                ReadCommand::Render,
                Box::new(move || erased(s.render(at, model, front()))),
            ),
            (
                ReadCommand::Timeline,
                Box::new(move || erased(s.timeline(at, model))),
            ),
            (
                ReadCommand::Meshes,
                Box::new(move || {
                    erased(s.meshes(
                        at,
                        model,
                        crate::command::meshes::Meshes {
                            view: Some(crate::views::NamedView::Front),
                            camera: None,
                            width: 100,
                            height: 100,
                            features: None,
                            held: vec![],
                        },
                    ))
                }),
            ),
            (
                ReadCommand::Sketch,
                Box::new(move || erased(s.sketch(at, model, id("k7f2qx3m")))),
            ),
            (
                ReadCommand::Pattern,
                Box::new(move || erased(s.pattern(at, model, id("k7f2qx3m")))),
            ),
        ]
    };
    let mut queued: Vec<(SessionCommand, Start<'_>)> = vec![
        (SessionCommand::Close, Box::new(|| erased(s.close(at)))),
        (SessionCommand::Begin, Box::new(|| erased(s.begin(at)))),
        (SessionCommand::Commit, Box::new(|| erased(s.commit(at)))),
        (
            SessionCommand::Rollback,
            Box::new(|| erased(s.rollback(at))),
        ),
        (
            SessionCommand::Format,
            Box::new(|| erased(s.format(at, None, true))),
        ),
        (
            SessionCommand::AddFeature,
            Box::new(|| erased(s.add_feature(at, Revision::OPENED, None, length()))),
        ),
        (
            SessionCommand::SetField,
            Box::new(|| {
                erased(s.set_field(
                    at,
                    Revision::OPENED,
                    id("k7f2qx3m"),
                    "length".to_owned(),
                    length(),
                ))
            }),
        ),
        (
            SessionCommand::RemoveFeature,
            Box::new(|| erased(s.remove_feature(at, Revision::OPENED, id("k7f2qx3m")))),
        ),
        (
            SessionCommand::ReplaceText,
            Box::new(|| erased(s.replace_text(at, Revision::OPENED, BOX.to_owned()))),
        ),
        (SessionCommand::Text, Box::new(|| erased(s.text(at, None)))),
        (SessionCommand::Outline, Box::new(|| erased(s.outline(at)))),
    ];
    // The reads M4.1 lists; `timeline`, `meshes`, and `sketch`, M5.0's, after `export` and
    // `move_feature`.
    let (m4_reads, m5_reads): (Vec<_>, Vec<_>) =
        reads(ModelChoice::Current)
            .into_iter()
            .partition(|(read, _)| {
                !matches!(
                    read,
                    ReadCommand::Timeline
                        | ReadCommand::Meshes
                        | ReadCommand::Sketch
                        | ReadCommand::Pattern
                )
            });
    queued.extend(
        m4_reads
            .into_iter()
            .map(|(read, start)| (SessionCommand::from(read), start)),
    );
    // Stopped before it meshes anything, so its path is never written.
    let never = std::env::temp_dir().join(format!("silico-never-{}.3mf", std::process::id()));
    queued.push((
        SessionCommand::Export,
        Box::new(move || {
            erased(s.export(
                at,
                crate::command::export::Export {
                    format: crate::command::export::ExportFormat::ThreeMf {
                        tolerance: crate::document::Length::from_nm(100_000),
                    },
                    path: never.clone(),
                    replace: false,
                    features: None,
                },
            ))
        }),
    ));
    queued.push((
        SessionCommand::MoveFeature,
        Box::new(|| erased(s.move_feature(at, Revision::OPENED, id("k7f2qx3m"), None))),
    ));
    queued.push((
        SessionCommand::SuppressFeature,
        Box::new(|| erased(s.suppress_feature(at, Revision::OPENED, id("k7f2qx3m"), true))),
    ));
    // M6.5's `pattern`, after `diff`, last.
    let (m6_reads, m5_reads): (Vec<_>, Vec<_>) = m5_reads
        .into_iter()
        .partition(|(read, _)| *read == ReadCommand::Pattern);
    queued.extend(
        m5_reads
            .into_iter()
            .map(|(read, start)| (SessionCommand::from(read), start)),
    );
    // M5.0's sketch edits, after `timeline`, as the design lists them.
    let empty = || crate::document::json::read(b"[]").unwrap();
    queued.push((
        SessionCommand::SketchAdd,
        Box::new(move || {
            erased(s.sketch_add(
                at,
                Revision::OPENED,
                id("k7f2qx3m"),
                crate::command::NewMembers {
                    points: empty(),
                    lines: empty(),
                    circles: empty(),
                    arcs: empty(),
                    constraints: empty(),
                },
            ))
        }),
    ));
    queued.push((
        SessionCommand::SketchRemove,
        Box::new(move || erased(s.sketch_remove(at, Revision::OPENED, id("k7f2qx3m"), vec![]))),
    ));
    let member = |text: &str| crate::document::ElementId::new(text).unwrap();
    queued.push((
        SessionCommand::SketchSplit,
        Box::new(move || {
            erased(s.sketch_split(
                at,
                Revision::OPENED,
                id("k7f2qx3m"),
                member("na000001"),
                vec![[0.0, 0.0]],
                None,
            ))
        }),
    ));
    queued.push((
        SessionCommand::SketchTrim,
        Box::new(move || {
            erased(s.sketch_trim(
                at,
                Revision::OPENED,
                id("k7f2qx3m"),
                member("na000001"),
                [0.0, 0.0],
                None,
            ))
        }),
    ));
    queued.push((
        SessionCommand::SketchUnjoin,
        Box::new(move || {
            erased(s.sketch_unjoin(
                at,
                Revision::OPENED,
                id("k7f2qx3m"),
                member("pa000001"),
                member("na000001"),
                None,
            ))
        }),
    ));
    queued.push((
        SessionCommand::SketchMerge,
        Box::new(move || {
            erased(s.sketch_merge(
                at,
                Revision::OPENED,
                id("k7f2qx3m"),
                vec![member("pa000001"), member("pa000002")],
            ))
        }),
    ));
    queued.push((
        SessionCommand::SketchExtend,
        Box::new(move || {
            erased(s.sketch_extend(
                at,
                Revision::OPENED,
                id("k7f2qx3m"),
                member("na000001"),
                crate::extend::End::End,
                crate::command::ExtendBy::NextCurve,
            ))
        }),
    ));
    queued.push((
        SessionCommand::SketchJoin,
        Box::new(move || {
            erased(s.sketch_join(at, Revision::OPENED, id("k7f2qx3m"), member("pa000002")))
        }),
    ));
    // M5.2's, after the sketch edits.
    queued.push((
        SessionCommand::Undo,
        Box::new(|| erased(s.undo(at, Revision::OPENED, None))),
    ));
    queued.push((
        SessionCommand::Redo,
        Box::new(|| erased(s.redo(at, Revision::OPENED, None))),
    ));
    // `diff`, Fable's of 2026-11-04, last.
    queued.push((
        SessionCommand::Diff,
        Box::new(|| erased(s.diff(at, None, None))),
    ));
    queued.extend(
        m6_reads
            .into_iter()
            .map(|(read, start)| (SessionCommand::from(read), start)),
    );
    // M5.3's queued drag commands, after `timeline`, as the design lists them: `drag_move`
    // runs beside the queue, on the drag, and its cancel is the drag's tests'.
    let after_timeline = queued
        .iter()
        .position(|(command, _)| *command == SessionCommand::Timeline)
        .unwrap()
        + 1;
    queued.insert(
        after_timeline,
        (
            SessionCommand::DragBegin,
            Box::new(|| {
                erased(s.drag_begin(
                    at,
                    id("k7f2qx3m"),
                    crate::command::DragGrab::Point(
                        crate::document::ElementId::new("pa000001").unwrap(),
                    ),
                ))
            }),
        ),
    );
    queued.insert(
        after_timeline + 1,
        (
            SessionCommand::DragEnd,
            Box::new(|| erased(s.drag_end(at, None))),
        ),
    );
    // `cache_clear`, which reads no model, after `check`, as the design lists it.
    let after_check = queued
        .iter()
        .position(|(command, _)| *command == SessionCommand::Check)
        .unwrap()
        + 1;
    queued.insert(
        after_check,
        (
            SessionCommand::CacheClear,
            Box::new(|| erased(s.cache_clear(at))),
        ),
    );
    // Every one, in the design's order, but `drag_move`, which isn't queued.
    assert_eq!(
        queued
            .iter()
            .map(|(command, _)| *command)
            .collect::<Vec<_>>(),
        SessionCommand::ALL
            .into_iter()
            .filter(|command| *command != SessionCommand::DragMove)
            .collect::<Vec<_>>()
    );
    let stops = |command: SessionCommand, start: &Start<'_>, running, reads: Vec<_>| {
        let answer = start();
        was_started.recv_timeout(WAIT).unwrap();
        let status = s.status(at).result.unwrap();
        assert_eq!(
            (status.running, status.reads),
            (running, reads),
            "{command}"
        );
        assert_eq!(
            s.cancel(at, command).result.unwrap(),
            Cancelled {
                command: command.into(),
                stopped: true
            },
            "{command}"
        );
        through.send(()).unwrap();
        assert_eq!(
            answer().unwrap_err().errors(),
            [cancelled(command.as_str())],
            "{command}"
        );
        let status = s.status(at).result.unwrap();
        assert_eq!((status.running, status.reads), (None, vec![]), "{command}");
    };
    for (command, start) in &queued {
        stops(
            *command,
            start,
            Some(RunningCommand::Command(*command)),
            vec![],
        );
    }
    for (read, start) in &reads(ModelChoice::Published) {
        stops(SessionCommand::from(*read), start, None, vec![*read]);
    }
    // And the rebuild, which status reports by its key, `rebuild`: started by a published
    // read (which passes no gate and waits on nothing), its box held, no command running and
    // no read, and cancel by that name stops it.
    let hold = f.kernel.hold(is_box);
    assert!(s.published(at).result.is_err());
    assert!(f.kernel.wait_running(1, WAIT));
    let status = s.status(at).result.unwrap();
    assert_eq!(
        (status.running, status.reads, status.rebuild.state),
        (None, vec![], RebuildState::Running)
    );
    assert_eq!(
        s.cancel(at, Cancellable::Rebuild).result.unwrap(),
        Cancelled {
            command: Cancellable::Rebuild,
            stopped: true
        }
    );
    assert!(
        f.kernel.wait_idle(WAIT),
        "the stopped rebuild's box still runs"
    );
    let status = s.status(at).result.unwrap();
    assert_eq!(
        (status.running, status.reads, status.rebuild.state),
        (None, vec![], RebuildState::Stopped)
    );
    drop(hold);
    // Nothing was done: the file is as it was, the session open at its first revision, and
    // nothing drawn.
    assert_eq!(f.file(), BOX);
    assert_eq!(f.session().held.read().revision, Revision::OPENED);
    assert!(renderer.sent().is_empty());
}

#[test]
fn a_cancel_after_cache_clear_settles_comes_too_late_and_the_clear_answers_with_its_result() {
    // Held once it has settled, before it removes anything: a cancel by name stops nothing,
    // and the clear, let through, removes the box's one entry and answers so.
    let f = opened(BOX);
    let checked = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    assert!(checked.result.unwrap().problems.is_empty());
    let cache = f.path.parent().unwrap().join(".silico-cache");
    // Its entries, as the cache finds its own: a regular file named by 62 lowercase hex
    // digits in a subdirectory named by two.
    let hex = |name: &std::ffi::OsStr, digits: usize| {
        name.to_str().is_some_and(|name| {
            name.len() == digits && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
    };
    let entries = || {
        std::fs::read_dir(&cache)
            .unwrap()
            .flatten()
            .filter(|sub| hex(&sub.file_name(), 2) && sub.file_type().unwrap().is_dir())
            .flat_map(|sub| std::fs::read_dir(sub.path()).unwrap().flatten())
            .filter(|file| hex(&file.file_name(), 62) && file.file_type().unwrap().is_file())
            .count()
    };
    assert_eq!(entries(), 1);
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().settled.shut(started, wait);
    let clearing = f.sessions.cache_clear(&f.id);
    was_started.recv_timeout(WAIT).unwrap();
    let cancelled = f
        .sessions
        .cancel(&f.id, SessionCommand::CacheClear)
        .result
        .unwrap();
    assert_eq!(
        (cancelled.command, cancelled.stopped),
        (SessionCommand::CacheClear.into(), false)
    );
    assert_eq!(entries(), 1);
    through.send(()).unwrap();
    let cleared = clearing.wait().result.unwrap();
    assert_eq!(cleared.removed, 1);
    assert_eq!(entries(), 0);
}

// ---- The session's file on disk (`Sessions::disk`) ----

/// `f`'s session's disk finding once it's `want`, within [`WAIT`].
fn disk_reaches(f: &Fixture, want: Disk) {
    let deadline = std::time::Instant::now() + WAIT;
    while f.sessions.disk(&f.id).unwrap() != Some(want) {
        assert!(std::time::Instant::now() < deadline, "never {want:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_sessions_file_is_unchanged_as_it_opened_it_and_changed_or_gone_as_another_leaves_it() {
    let f = opened(BOX);
    disk_reaches(&f, Disk::Unchanged);
    // Inside a transaction, where nothing's reloaded, so each finding stays to be read.
    f.begin();
    std::fs::write(&f.path, SPHERE).unwrap();
    disk_reaches(&f, Disk::Changed);
    std::fs::remove_file(&f.path).unwrap();
    disk_reaches(&f, Disk::Gone);
    std::fs::write(&f.path, BOX).unwrap();
    disk_reaches(&f, Disk::Unchanged);
}

#[test]
fn a_sessions_own_commit_is_what_its_file_is_compared_with_from_then_on() {
    let f = opened(STORED);
    disk_reaches(&f, Disk::Unchanged);
    f.begin();
    f.edit(&longer());
    assert!(f.sessions.commit(&f.id).wait().result.unwrap().written);
    let saved = f.file();
    assert_ne!(saved, STORED);
    // At once: what it wrote is what it holds.
    assert_eq!(f.sessions.disk(&f.id).unwrap(), Some(Disk::Unchanged));
    // The text it opened, written back by another, is a change; what it saved isn't. (In a
    // transaction again, where the change isn't reloaded.)
    f.begin();
    std::fs::write(&f.path, STORED).unwrap();
    disk_reaches(&f, Disk::Changed);
    std::fs::write(&f.path, &saved).unwrap();
    disk_reaches(&f, Disk::Unchanged);
}

#[test]
fn a_reload_is_what_the_sessions_file_is_compared_with_from_then_on() {
    let f = opened(BOX);
    disk_reaches(&f, Disk::Unchanged);
    // Held behind a command, so the change is found before the session reloads it.
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().gate.shut(started, wait);
    let text = f.sessions.text(&f.id, None);
    was_started.recv_timeout(WAIT).unwrap();
    std::fs::write(&f.path, SPHERE).unwrap();
    disk_reaches(&f, Disk::Changed);
    *lock(&f.session().gate.0) = None;
    through.send(()).unwrap();
    assert!(text.wait().session.unwrap().reloaded);
    // At once, though the comparisons before it found the file changed.
    assert_eq!(f.sessions.disk(&f.id).unwrap(), Some(Disk::Unchanged));
    f.begin();
    std::fs::write(&f.path, BOX).unwrap();
    disk_reaches(&f, Disk::Changed);
    std::fs::write(&f.path, SPHERE).unwrap();
    disk_reaches(&f, Disk::Unchanged);
}

#[test]
fn a_save_that_fails_leaves_the_sessions_file_unknown_until_it_is_compared_again() {
    let f = opened(STORED);
    disk_reaches(&f, Disk::Unchanged);
    let dir = f.path.parent().unwrap().to_owned();
    f.begin();
    f.edit(&longer());
    let read_only = ReadOnly::of(&dir);
    // Root writes in a read-only directory all the same: no save here fails.
    let probe = dir.join("probe");
    if std::fs::write(&probe, "").is_ok() {
        let _ = std::fs::remove_file(&probe);
        return;
    }
    let problems = f.sessions.commit(&f.id).wait().result.unwrap_err();
    let refused = &problems.errors()[0];
    assert_eq!(
        (refused.code(), refused.parameter()),
        (ErrorCode::IoFailed, Some("path"))
    );
    assert_eq!(f.sessions.disk(&f.id).unwrap(), None);
    // Nothing was written: the next comparison finds the file as the session opened it.
    disk_reaches(&f, Disk::Unchanged);
    drop(read_only);
}

#[test]
fn the_disk_of_a_session_not_open_is_refused_naming_it() {
    let f = opened(BOX);
    let refused = f.sessions.disk(&SessionId(99)).unwrap_err();
    assert_eq!(
        (refused.code(), refused.parameter()),
        (ErrorCode::InvalidInput, Some("session"))
    );
    f.sessions.close(&f.id).wait().result.unwrap();
    let refused = f.sessions.disk(&f.id).unwrap_err();
    assert_eq!(
        (refused.code(), refused.parameter()),
        (ErrorCode::InvalidInput, Some("session"))
    );
}

#[test]
fn a_sessions_own_save_is_unknown_on_disk_from_before_its_file_is_written() {
    let f = opened(STORED);
    disk_reaches(&f, Disk::Unchanged);
    f.begin();
    f.edit(&longer());
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().writing.shut(started, wait);
    let committing = f.sessions.commit(&f.id);
    was_started.recv_timeout(WAIT).unwrap();
    // The file still as the session opened it, the save about to write it: already no
    // finding, and no comparison starts to find one, so none reads it mid-save.
    assert_eq!(f.file(), STORED);
    for _ in 0..20 {
        assert_eq!(f.sessions.disk(&f.id).unwrap(), None);
        std::thread::sleep(Duration::from_millis(1));
    }
    through.send(()).unwrap();
    assert!(committing.wait().result.unwrap().written);
    assert_eq!(f.sessions.disk(&f.id).unwrap(), Some(Disk::Unchanged));
}

#[test]
fn a_session_opened_through_a_link_watches_the_file_it_read_not_where_the_link_leads_later() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, link) = (
        dir.path().join("a.silico"),
        dir.path().join("b.silico"),
        dir.path().join("link.silico"),
    );
    std::fs::write(&a, BOX).unwrap();
    std::fs::write(&b, BOX).unwrap();
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let secret = dir.path().join("secret");
    let sessions = Sessions::with_stand_ins(
        kernel,
        move |cache| Cache::with_secret(cache, &secret),
        || Ok(Box::new(crate::fake::FakeSketches::new()) as Box<dyn SketchSolver>),
    )
    .checking_disk_every(Duration::ZERO);
    let id = sessions.open(&link, false).wait().session.unwrap().id;
    // Inside a transaction, where nothing's reloaded, so a's change stays to be found.
    sessions.begin(&id).wait().result.unwrap();
    // The link leads to b, as the session first read a; a changed. Read where the session
    // reads and saves, a, it's changed; where the link leads now, b, it would be unchanged.
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&b, &link).unwrap();
    std::fs::write(&a, SPHERE).unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while sessions.disk(&id).unwrap() != Some(Disk::Changed) {
        assert!(
            std::time::Instant::now() < deadline,
            "a's change never found"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn each_sessions_disk_is_its_own_files() {
    let f = opened(BOX);
    let other = f.path.with_file_name("b.silico");
    std::fs::write(&other, SPHERE).unwrap();
    let second = f.sessions.open(&other, false).wait().session.unwrap().id;
    disk_reaches(&f, Disk::Unchanged);
    f.begin();
    std::fs::write(&f.path, SPHERE).unwrap();
    disk_reaches(&f, Disk::Changed);
    let deadline = std::time::Instant::now() + WAIT;
    let found = loop {
        if let Some(found) = f.sessions.disk(&second).unwrap() {
            break found;
        }
        assert!(std::time::Instant::now() < deadline, "no comparison ended");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(found, Disk::Unchanged);
}

#[test]
fn a_sessions_disk_is_answered_while_its_command_waits() {
    let f = opened(BOX);
    disk_reaches(&f, Disk::Unchanged);
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().gate.shut(started, wait);
    let text = f.sessions.text(&f.id, None);
    was_started.recv_timeout(WAIT).unwrap();
    // A disk that waited on the command would never return.
    std::fs::write(&f.path, SPHERE).unwrap();
    disk_reaches(&f, Disk::Changed);
    through.send(()).unwrap();
    text.wait().result.unwrap();
}

// ---- Break-it (PR #269): the session's own saves, compared with no interval ----

#[test]
fn asked_without_pause_through_many_commits_the_sessions_own_saves_never_read_as_changed() {
    let f = Arc::new(opened(STORED));
    disk_reaches(&f, Disk::Unchanged);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let askers: Vec<_> = (0..3)
        .map(|_| {
            let (f, done) = (Arc::clone(&f), Arc::clone(&done));
            std::thread::spawn(move || {
                let mut seen = Vec::new();
                let mut asked = 0_u64;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    let found = f.sessions.disk(&f.id).unwrap();
                    asked += 1;
                    if found.is_some_and(|found| found != Disk::Unchanged) {
                        seen.push(found);
                    }
                }
                (asked, seen)
            })
        })
        .collect();
    for n in 0..60 {
        f.begin();
        f.edit(&if n % 2 == 0 {
            longer()
        } else {
            STORED.to_owned()
        });
        let committed = f.sessions.commit(&f.id).wait().result.unwrap();
        assert!(committed.written, "{n}");
        // At once, whatever comparison was running.
        assert_eq!(
            f.sessions.disk(&f.id).unwrap(),
            Some(Disk::Unchanged),
            "{n}"
        );
    }
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    for asker in askers {
        let (asked, seen) = asker.join().unwrap();
        assert!(asked > 0);
        assert!(seen.is_empty(), "{seen:?}");
    }
    // And after: the file is what the last commit wrote.
    disk_reaches(&f, Disk::Unchanged);
}

mod cancel_rebuild;
mod disk_reload;
mod drag;
mod elapsed;
mod rebuild_features;
#[test]
fn a_diff_cancelled_as_it_compares_a_feature_stops_there_and_answers_cancelled() {
    // Held at the gate as it starts, let through, and held again as it comes to compare its
    // first feature (`diff`'s stop is asked there, the gate first): cancelled there, it
    // compares no other. Of two features, so a diff that went on past its stop would come
    // to the second, saying so on `started`.
    let f = opened(BOX_AND_PIN);
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().gate.shut(started, wait);
    let running = f.sessions.diff(&f.id, None, None);
    // The command's start, then its first feature.
    was_started.recv_timeout(WAIT).unwrap();
    through.send(()).unwrap();
    was_started.recv_timeout(WAIT).unwrap();
    assert!(
        f.sessions
            .cancel(&f.id, SessionCommand::Diff)
            .result
            .unwrap()
            .stopped
    );
    // Every wait at the gate from here let through at once.
    drop(through);
    let response = running.wait();
    assert_eq!(response.result.unwrap_err().errors(), [cancelled("diff")]);
    // Answered once its work ended: nothing came to the gate after the first feature.
    assert_eq!(was_started.try_iter().count(), 0);
    // And the next diff runs whole.
    *lock(&f.session().gate.0) = None;
    let next = f.sessions.diff(&f.id, None, None).wait().result.unwrap();
    assert_eq!(next.changes, []);
}

mod timeline;
mod undo;
mod undo_adversarial;
mod undo_format;
mod undo_status;
