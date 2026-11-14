//! M5.4's core (docs/design/m5.0-app.md, sections 3, 4, and 6, A3; section 13's M5.4
//! tests): `status`'s `disk`, a comparison started by `status` off the caller's thread at
//! most once each interval, by an injected clock; the session's own reload of a file an
//! agent changed with no transaction open, named `reload` while it runs, queued once, and
//! making one revision; the reload's rules inside a transaction; and the client's waker,
//! told only once what changed can be read, with no lock held. Expected values: the bytes
//! the tests write, the revisions counted by hand from the commands sent, and the design's
//! rules.

use super::*;
use crate::disk::{Clock, DISK_CHECK_INTERVAL, Found, Reader};
use crate::document::json::MAX_INPUT_LEN;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

/// A clock a test moves by hand.
struct TestClock(Mutex<Instant>);

impl TestClock {
    fn new() -> Arc<Self> {
        Arc::new(TestClock(Mutex::new(Instant::now())))
    }

    fn advance(&self, by: Duration) {
        let mut now = lock(&self.0);
        *now += by;
    }

    fn clock(self: &Arc<Self>) -> Clock {
        let clock = Arc::clone(self);
        Arc::new(move || *lock(&clock.0))
    }
}

/// The session's file read as it is (a regular file's bytes, gone if nothing's there), each
/// read counted as it starts and as it ends, and held, while the test holds it, before it
/// reads.
#[derive(Default)]
struct HeldReader {
    held: Mutex<bool>,
    opened: Condvar,
    started: AtomicUsize,
    ended: AtomicUsize,
}

impl HeldReader {
    fn reader(self: &Arc<Self>) -> Reader {
        let me = Arc::clone(self);
        Arc::new(move |path: &Path| {
            me.started.fetch_add(1, Ordering::SeqCst);
            let mut held = lock(&me.held);
            while *held {
                held = me.opened.wait(held).unwrap();
            }
            drop(held);
            let found = match std::fs::read(path) {
                Ok(bytes) if bytes.len() > MAX_INPUT_LEN => Found::Unreadable,
                Ok(bytes) => Found::Bytes(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Found::Gone,
                Err(_) => Found::Unreadable,
            };
            me.ended.fetch_add(1, Ordering::SeqCst);
            found
        })
    }

    fn hold(&self) {
        *lock(&self.held) = true;
    }

    fn release(&self) {
        *lock(&self.held) = false;
        self.opened.notify_all();
    }

    fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }

    fn ended(&self) -> usize {
        self.ended.load(Ordering::SeqCst)
    }
}

/// [`opened`]'s sessions, but with each file compared through a [`HeldReader`] at most once
/// each [`DISK_CHECK_INTERVAL`] by a [`TestClock`].
struct Watched {
    f: Fixture,
    clock: Arc<TestClock>,
    reader: Arc<HeldReader>,
}

fn watched(text: &str) -> Watched {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("a.silico");
    std::fs::write(&path, text).unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let clock = TestClock::new();
    let reader = Arc::new(HeldReader::default());
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        || Ok(Box::new(crate::fake::FakeSketches::new()) as Box<dyn SketchSolver>),
    )
    .watching_disk(DISK_CHECK_INTERVAL, reader.reader(), clock.clock())
    .unwrap();
    let id = sessions.open(&path, false).wait().session.unwrap().id;
    Watched {
        f: Fixture {
            _dir: dir,
            kernel,
            sessions,
            id,
            path,
        },
        clock,
        reader,
    }
}

impl Watched {
    /// How many of the watch's comparisons have ended, each with its telling done.
    fn compared(&self) -> usize {
        self.f.session().disk.get().unwrap().compared()
    }

    /// One comparison, which reads the file as it is now: every comparison started before
    /// it waited for until it has ended, then the clock moved a whole interval and `status`
    /// asked, which starts one, waited for until it has ended and told what it tells.
    #[track_caller]
    fn compare(&self) {
        let deadline = Instant::now() + WAIT;
        while self.compared() != self.reader.started() {
            assert!(Instant::now() < deadline, "a comparison never ended");
            std::thread::sleep(Duration::from_millis(1));
        }
        let before = self.compared();
        self.clock.advance(DISK_CHECK_INTERVAL);
        self.f.status();
        while self.compared() == before {
            assert!(Instant::now() < deadline, "no comparison ran");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(self.compared(), before + 1, "more than one comparison ran");
    }
}

impl Fixture {
    fn status(&self) -> Status {
        self.sessions.status(&self.id).result.unwrap()
    }

    /// The session's revision, as `status` gives it.
    fn at(&self) -> Revision {
        self.sessions.status(&self.id).session.unwrap().revision
    }

    /// The session's text, as `text` gives it.
    fn held_text(&self) -> String {
        self.sessions
            .text(&self.id, None)
            .wait()
            .result
            .unwrap()
            .text
    }

    /// `status` asked until `want` holds of it and the session's revision, within [`WAIT`]:
    /// each asking starts a comparison, as the app's frames do (these sessions compare with
    /// no interval).
    #[track_caller]
    fn status_reaches(&self, want: impl Fn(&Status, Revision) -> bool) -> (Status, Revision) {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            let response = self.sessions.status(&self.id);
            let revision = response.session.unwrap().revision;
            let status = response.result.unwrap();
            if want(&status, revision) {
                return (status, revision);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "never: last {status:?} at {revision:?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// `status` asked for `long`, each answer one `allowed` allows.
    #[track_caller]
    fn status_stays(&self, long: Duration, allowed: impl Fn(&Status, Revision) -> bool) {
        let deadline = std::time::Instant::now() + long;
        while std::time::Instant::now() < deadline {
            let response = self.sessions.status(&self.id);
            let revision = response.session.unwrap().revision;
            let status = response.result.unwrap();
            assert!(allowed(&status, revision), "{status:?} at {revision:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// A gate shut: what's held at it says so on the first end, and goes on once the second is
/// sent a word (or dropped).
fn shut(gate: &Gate) -> (Receiver<()>, Sender<()>) {
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    gate.shut(started, wait);
    (was_started, through)
}

fn open_gate(gate: &Gate) {
    *lock(&gate.0) = None;
}

/// Waits until a comparison that found the file changed has told the session (and so
/// queued its reload, if one was wanted): it does so after its finding is kept, so a
/// finding of `changed` alone doesn't say it has, and only once a second comparison has
/// found the same bytes (#479), so `status` is asked meanwhile, each asking starting a
/// comparison in these sessions (with no interval; a session with a test's clock compares
/// only as the test moves it).
#[track_caller]
fn told(f: &Fixture) {
    let deadline = Instant::now() + WAIT;
    while f.session().disk.get().unwrap().changed_told() == 0 {
        f.status();
        assert!(Instant::now() < deadline, "no comparison told the session");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Waits until the session's own reload, its gate shut, has reached the gate, asking `status`
/// meanwhile, each asking starting a comparison (these sessions compare with no interval): a
/// change is reloaded only once a second comparison finds the same bytes (#479).
#[track_caller]
fn reload_reaches_its_gate(f: &Fixture, started: &Receiver<()>) {
    let deadline = Instant::now() + WAIT;
    while started.try_recv().is_err() {
        f.status();
        assert!(Instant::now() < deadline, "the reload never started");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn length(value: &str) -> Node {
    crate::document::json::read(value.as_bytes()).unwrap()
}

fn reloading(status: &Status) -> bool {
    status.running == Some(RunningCommand::Reload)
}

// ---- `status.disk`, by an injected clock ----

#[test]
fn disk_is_null_until_a_comparison_ends_and_one_runs_at_a_time_each_a_second_after_the_last() {
    let w = watched(BOX);
    w.reader.hold();
    // The first status starts one, and has none to say.
    assert_eq!(w.f.status().disk, None);
    let deadline = std::time::Instant::now() + WAIT;
    while w.reader.started() < 1 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    // One at a time: a second status, the first held, starts none.
    assert_eq!(w.f.status().disk, None);
    // Held while the clock moves 0.8 s, then let go: it ends at 0.8 s.
    w.clock.advance(Duration::from_millis(800));
    w.reader.release();
    w.f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    assert_eq!(w.reader.started(), 1);
    // Its end plus 0.5 s: none due.
    w.clock.advance(Duration::from_millis(500));
    w.f.status();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(w.reader.started(), 1);
    // Its end plus 1 s: one starts.
    w.clock.advance(Duration::from_millis(500));
    w.f.status();
    let deadline = std::time::Instant::now() + WAIT;
    while w.reader.started() < 2 {
        assert!(std::time::Instant::now() < deadline, "none started at 1 s");
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(w.reader.started(), 2);
}

#[test]
fn the_sessions_own_commit_landing_during_a_comparison_is_unchanged() {
    let w = watched(STORED);
    w.f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    w.f.begin();
    w.f.edit(&longer());
    // A comparison held before it reads, the clock past its interval.
    w.reader.hold();
    w.clock.advance(DISK_CHECK_INTERVAL);
    let before = w.reader.started();
    w.f.status();
    let deadline = std::time::Instant::now() + WAIT;
    while w.reader.started() == before {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    // The commit lands while it's held; the comparison then reads the saved file, against
    // the bytes the session held when it started, and its finding is dropped.
    assert!(w.f.sessions.commit(&w.f.id).wait().result.unwrap().written);
    assert_eq!(w.f.status().disk, Some(Disk::Unchanged));
    // Counted before it's let go, so its end is never missed.
    let ended = w.reader.ended();
    w.reader.release();
    let deadline = std::time::Instant::now() + WAIT;
    while w.reader.ended() == ended {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    // The next comparison held as it starts, which it does only once the held one has
    // ended, its finding kept or dropped: dropped, so the finding is still the commit's.
    w.reader.hold();
    let next = w.reader.started();
    let deadline = std::time::Instant::now() + WAIT;
    while w.reader.started() == next {
        w.clock.advance(DISK_CHECK_INTERVAL);
        assert_eq!(w.f.status().disk, Some(Disk::Unchanged));
        assert!(std::time::Instant::now() < deadline, "no next comparison");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(w.f.status().disk, Some(Disk::Unchanged));
    w.reader.release();
}

#[test]
fn a_change_keeping_the_files_size_and_time_is_changed() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    // Inside a transaction, so it isn't reloaded and stays to be read.
    f.begin();
    let before = std::fs::metadata(&f.path).unwrap();
    let other = BOX.replace("Block", "Brick");
    assert_eq!(other.len(), BOX.len());
    std::fs::write(&f.path, &other).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&f.path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(before.modified().unwrap())
                .set_accessed(before.accessed().unwrap()),
        )
        .unwrap();
    let after = std::fs::metadata(&f.path).unwrap();
    assert_eq!(
        (after.len(), after.modified().unwrap()),
        (before.len(), before.modified().unwrap())
    );
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
}

#[test]
fn a_file_gone_is_gone_and_one_this_user_cant_read_as_a_document_is_unreadable() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    f.begin();
    std::fs::remove_file(&f.path).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Gone));
    std::fs::create_dir(&f.path).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Unreadable));
    std::fs::remove_dir(&f.path).unwrap();
    std::fs::write(&f.path, BOX).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
}

#[test]
fn a_file_of_the_most_silico_reads_is_compared_and_one_byte_more_is_unreadable() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    f.begin();
    std::fs::write(&f.path, vec![b' '; MAX_INPUT_LEN]).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    std::fs::write(&f.path, vec![b' '; MAX_INPUT_LEN + 1]).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Unreadable));
}

// ---- The session's own reload (A3) ----

#[test]
fn an_agents_commit_is_reloaded_with_no_command_named_reload_while_it_runs() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let (was_started, through) = shut(&f.session().reloading);
    std::fs::write(&f.path, SPHERE).unwrap();
    // Found by status's comparison, and reloaded by the session once the next finds it
    // again: no command sent.
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    reload_reaches_its_gate(&f, &was_started);
    let (status, revision) = f.status_reaches(|status, _| reloading(status));
    assert_eq!(revision, Revision::OPENED);
    assert_eq!(status.running.map(RunningCommand::as_str), Some("reload"));
    through.send(()).unwrap();
    let (status, revision) = f
        .status_reaches(|status, revision| revision > Revision::OPENED && status.running.is_none());
    assert_eq!(revision, Revision(2));
    assert_eq!(status.disk, Some(Disk::Unchanged));
    // Its text is the agent's, and the next command reloads nothing.
    let text = f.sessions.text(&f.id, None).wait();
    let session = text.session.unwrap();
    assert!(!session.reloaded);
    assert_eq!(session.revision, Revision(2));
    assert_eq!(text.result.unwrap().text, SPHERE);
    assert_eq!(f.file(), SPHERE);
}

#[test]
fn changes_found_while_the_reload_waits_queue_no_second_and_make_one_revision() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let (reload_started, reload_through) = shut(&f.session().reloading);
    // A command held ahead of it.
    let (started, through) = shut(&f.session().gate);
    let text = f.sessions.text(&f.id, None);
    started.recv_timeout(WAIT).unwrap();
    std::fs::write(&f.path, SPHERE).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    // Many comparisons find it while the reload waits: each queues nothing more.
    for _ in 0..50 {
        assert_eq!(f.status().disk, Some(Disk::Changed));
        std::thread::sleep(Duration::from_millis(1));
    }
    // The reload queued before the command ahead runs (and reads the file itself).
    told(&f);
    open_gate(&f.session().gate);
    through.send(()).unwrap();
    // The command ahead reloads it itself (decision 2): revision 2.
    let read = text.wait();
    assert!(read.session.as_ref().unwrap().reloaded);
    assert_eq!(read.session.unwrap().revision, Revision(2));
    // Then the one reload queued, finding it held, makes none; no second was queued (one
    // would start at the gate, still shut).
    reload_started.recv_timeout(WAIT).unwrap();
    reload_through.send(()).unwrap();
    assert!(
        reload_started
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "a second reload was queued"
    );
    open_gate(&f.session().reloading);
    drop(reload_through);
    f.status_reaches(|status, _| status.running.is_none());
    assert_eq!(f.at(), Revision(2));
    // The app's next edit, against the revision it read, meets no conflict.
    let set = f
        .sessions
        .set_field(
            &f.id,
            Revision(2),
            id("a91cw0de"),
            "radius".to_owned(),
            length("6"),
        )
        .wait();
    assert!(set.result.unwrap().changed);
}

#[test]
fn a_second_agent_commit_after_the_first_reload_is_reloaded_too() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    std::fs::write(&f.path, SPHERE).unwrap();
    f.status_reaches(|status, revision| {
        revision == Revision(2) && status.disk == Some(Disk::Unchanged)
    });
    std::fs::write(&f.path, BOX).unwrap();
    f.status_reaches(|status, revision| {
        revision == Revision(3) && status.disk == Some(Disk::Unchanged)
    });
    assert_eq!(f.held_text(), BOX);
}

#[test]
fn an_edit_queued_as_the_change_is_found_meets_a_conflict_and_the_agents_commit_stands() {
    let w = watched(BOX);
    let f = &w.f;
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    // The comparison held until the edit is queued.
    w.reader.hold();
    w.clock.advance(DISK_CHECK_INTERVAL);
    let before = w.reader.started();
    f.status();
    let deadline = std::time::Instant::now() + WAIT;
    while w.reader.started() == before {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    std::fs::write(&f.path, SPHERE).unwrap();
    // The edit waits behind a command held, so the reload is found while it's queued.
    let (started, through) = shut(&f.session().gate);
    let held = f.sessions.text(&f.id, None);
    started.recv_timeout(WAIT).unwrap();
    let set = f.sessions.set_field(
        &f.id,
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        length("41"),
    );
    w.reader.release();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    open_gate(&f.session().gate);
    through.send(()).unwrap();
    held.wait().result.unwrap();
    let refused = set.wait();
    let problems = refused.result.unwrap_err();
    let conflict = &problems.errors()[0];
    assert_eq!(conflict.code(), ErrorCode::Conflict);
    assert!(
        conflict
            .cause()
            .is_some_and(|cause| cause.contains("was reloaded")),
        "{conflict:?}"
    );
    // One new revision, the agent's text, never written over.
    f.status_reaches(|status, _| status.running.is_none());
    assert_eq!(f.at(), Revision(2));
    assert_eq!(f.held_text(), SPHERE);
    assert_eq!(f.file(), SPHERE);
}

#[test]
fn with_edits_held_nothing_is_reloaded_and_the_commit_is_a_conflict_until_rolled_back() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    f.begin();
    let set = f
        .sessions
        .set_field(
            &f.id,
            Revision::OPENED,
            id("k7f2qx3m"),
            "length".to_owned(),
            length("41"),
        )
        .wait();
    let edited = set.session.unwrap().revision;
    std::fs::write(&f.path, SPHERE).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    f.status_stays(Duration::from_millis(200), |status, revision| {
        revision == edited && status.disk == Some(Disk::Changed) && status.running.is_none()
    });
    let refused = f.sessions.commit(&f.id).wait().result.unwrap_err();
    assert_eq!(refused.errors()[0].code(), ErrorCode::Conflict);
    assert_eq!(f.file(), SPHERE);
    // Reload: the rollback, then the session's own reload, with no command after it.
    let rolled = f.sessions.rollback(&f.id).wait();
    let at = rolled.session.unwrap().revision;
    f.status_reaches(|status, revision| revision > at && status.disk == Some(Disk::Unchanged));
    assert_eq!(f.held_text(), SPHERE);
}

// ---- The reload's own rules ----

#[test]
fn a_reload_queued_behind_a_begin_that_reloaded_the_file_makes_no_revision() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let (reload_started, reload_through) = shut(&f.session().reloading);
    let (started, through) = shut(&f.session().gate);
    let begin = f.sessions.begin(&f.id);
    started.recv_timeout(WAIT).unwrap();
    std::fs::write(&f.path, SPHERE).unwrap();
    // Found, its reload queued behind the begin.
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    told(&f);
    open_gate(&f.session().gate);
    through.send(()).unwrap();
    // The begin reloads the file itself, at revision 2, then opens its transaction.
    let begun = begin.wait();
    assert!(begun.session.as_ref().unwrap().reloaded);
    assert_eq!(begun.result.unwrap().base, Revision(2));
    // The reload queued runs inside it: no revision, and the finding stale, so unchanged.
    reload_started.recv_timeout(WAIT).unwrap();
    open_gate(&f.session().reloading);
    reload_through.send(()).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged) && status.running.is_none());
    f.status_stays(Duration::from_millis(200), |status, revision| {
        revision == Revision(2) && status.transaction.is_some()
    });
}

#[test]
fn a_file_changed_again_after_that_begin_stays_changed() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let (reload_started, reload_through) = shut(&f.session().reloading);
    let (started, through) = shut(&f.session().gate);
    let begin = f.sessions.begin(&f.id);
    started.recv_timeout(WAIT).unwrap();
    std::fs::write(&f.path, SPHERE).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    // Its reload queued (behind the begin) before the begin runs: the comparison tells the
    // session after its finding is kept, and once the begin has read the file the
    // comparison is the session's no longer, and queues nothing.
    told(&f);
    open_gate(&f.session().gate);
    through.send(()).unwrap();
    assert_eq!(begin.wait().result.unwrap().base, Revision(2));
    // Changed again before the reload queued behind the begin runs.
    reload_started.recv_timeout(WAIT).unwrap();
    std::fs::write(&f.path, BOX.replace("Block", "Other")).unwrap();
    open_gate(&f.session().reloading);
    reload_through.send(()).unwrap();
    f.status_reaches(|status, _| status.running.is_none() && status.disk == Some(Disk::Changed));
    f.status_stays(Duration::from_millis(200), |status, revision| {
        revision == Revision(2) && status.disk == Some(Disk::Changed)
    });
}

#[test]
fn a_reload_that_finds_the_file_as_the_session_holds_it_makes_none() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let (started, through) = shut(&f.session().reloading);
    std::fs::write(&f.path, SPHERE).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    reload_reaches_its_gate(&f, &started);
    // Put back before it reads.
    std::fs::write(&f.path, BOX).unwrap();
    open_gate(&f.session().reloading);
    through.send(()).unwrap();
    f.status_reaches(|status, _| status.running.is_none() && status.disk == Some(Disk::Unchanged));
    assert_eq!(f.at(), Revision::OPENED);
}

#[test]
fn a_cancel_naming_reload_is_refused_and_reload_is_never_a_candidate() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let (started, through) = shut(&f.session().reloading);
    std::fs::write(&f.path, SPHERE).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    reload_reaches_its_gate(&f, &started);
    f.status_reaches(|status, _| reloading(status));
    // By name, as a client sends it: the reader refuses it, naming `command`.
    let crate::command::Call::Running(calling) = f
        .sessions
        .call("cancel", r#"{"session": "s1", "command": "reload"}"#)
    else {
        panic!("cancel is a command");
    };
    let answered = calling.wait();
    assert!(!answered.ok);
    assert!(
        answered.text.contains(r#""parameter": "command""#),
        "{}",
        answered.text
    );
    // A cancel of another command, while it runs: nothing of that name runs, and it isn't
    // offered.
    let refused = f
        .sessions
        .cancel(&f.id, SessionCommand::Text)
        .result
        .unwrap_err();
    let refusal = &refused.errors()[0];
    assert_eq!(refusal.parameter(), Some("command"));
    assert!(refusal.candidates().is_empty(), "{refusal:?}");
    assert_eq!(refusal.message(), "no text is running in session s1");
    open_gate(&f.session().reloading);
    through.send(()).unwrap();
    f.status_reaches(|_, revision| revision == Revision(2));
}

#[test]
fn a_reload_its_file_refused_is_queued_again_only_for_other_bytes() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    // Another session's document put at this one's path, through a link.
    let other = f.path.with_file_name("b.silico");
    std::fs::write(&other, SPHERE).unwrap();
    let second = f.sessions.open(&other, false).wait().session.unwrap().id;
    let (started, through) = shut(&f.session().reloading);
    std::fs::remove_file(&f.path).unwrap();
    std::os::unix::fs::symlink(&other, &f.path).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    reload_reaches_its_gate(&f, &started);
    through.send(()).unwrap();
    // Refused (it's s2's), and the same bytes, found again and again, queue no other.
    f.status_reaches(|status, _| status.running.is_none());
    for _ in 0..100 {
        f.status();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        started.try_recv().is_err(),
        "the refused reload was queued again"
    );
    assert_eq!(f.at(), Revision::OPENED);
    // Other bytes there are another reload.
    let edited = f
        .sessions
        .set_field(
            &second,
            Revision::OPENED,
            id("a91cw0de"),
            "radius".to_owned(),
            length("6"),
        )
        .wait();
    assert!(edited.result.unwrap().changed);
    f.status();
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        f.status();
        if started.try_recv().is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the new bytes queued none"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    open_gate(&f.session().reloading);
    through.send(()).unwrap();
}

// ---- The waker (section 3) ----

/// A waker that counts what it's told.
fn counting(sessions: &Sessions<Arc<FakeKernel>>) -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&count);
    sessions.on_change(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    count
}

/// Waits until the client has been woken `want` times, at least: the wake the test waits
/// for has come. Whether any more came is for [`settled`] to say, exactly.
#[track_caller]
fn woken(count: &AtomicUsize, want: usize) {
    let deadline = Instant::now() + WAIT;
    while count.load(Ordering::SeqCst) < want {
        assert!(
            Instant::now() < deadline,
            "woken {} times, never {want}",
            count.load(Ordering::SeqCst)
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The client's wakes so far, exactly, none still to come of what came before: the
/// session's rebuilds' watchers and its published reads' threads joined (each wakes before
/// it ends), then a `text` held as it starts, after every queued command and reload before
/// it has answered and woken on the session's thread. Counted then, and the `text` let
/// through: its own wake counted too, which the count returned includes.
#[track_caller]
fn settled(f: &Fixture, count: &AtomicUsize) -> usize {
    settled_in(&f.sessions, &f.id, count)
}

/// [`settled`], of the session `id` of `sessions`.
#[track_caller]
fn settled_in(sessions: &Sessions<Arc<FakeKernel>>, id: &SessionId, count: &AtomicUsize) -> usize {
    let session = sessions.find(id).unwrap();
    let watchers = std::mem::replace(&mut lock(&session.rebuilds).watchers, Threads::new());
    watchers.join();
    let reads = lock(&session.reads)
        .threads
        .as_mut()
        .map(|threads| std::mem::replace(threads, Threads::new()));
    if let Some(reads) = reads {
        reads.join();
    }
    let (started, through) = shut(&session.gate);
    let text = sessions.text(id, None);
    started.recv_timeout(WAIT).unwrap();
    let before = count.load(Ordering::SeqCst);
    through.send(()).unwrap();
    // The gate open again for every command after.
    drop(through);
    open_gate(&session.gate);
    text.wait().result.unwrap();
    woken(count, before + 1);
    before + 1
}

#[test]
fn a_waker_takes_the_answer_that_woke_it_and_asks_status_with_no_lock_held() {
    let f = Arc::new(opened(BOX));
    let hold = f.kernel.hold(is_box);
    let slot: Arc<Mutex<Option<Running<CheckAnswer>>>> = Arc::default();
    let taken: Arc<Mutex<Option<Response<CheckAnswer>>>> = Arc::default();
    let deadlocked = Arc::new(AtomicUsize::new(0));
    {
        let (slot, taken, deadlocked) = (
            Arc::clone(&slot),
            Arc::clone(&taken),
            Arc::clone(&deadlocked),
        );
        let weak = Arc::downgrade(&f);
        f.sessions.on_change(move || {
            let Some(f) = weak.upgrade() else {
                return;
            };
            // `status` from inside it, under a timeout: a lock the core held as it called
            // would keep it waiting.
            let (said, answer) = mpsc::channel();
            let asker = Arc::clone(&f);
            std::thread::spawn(move || {
                let _ = said.send(asker.sessions.status(&asker.id).result.is_ok());
            });
            if answer.recv_timeout(Duration::from_secs(10)) != Ok(true) {
                deadlocked.fetch_add(1, Ordering::SeqCst);
            }
            let running = lock(&slot).take();
            if let Some(running) = running {
                match running.try_take() {
                    Ok(response) => *lock(&taken) = Some(response),
                    Err(running) => *lock(&slot) = Some(running),
                }
            }
        });
    }
    *lock(&slot) = Some(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false }),
    );
    assert!(f.kernel.wait_running(1, WAIT));
    hold.release();
    let deadline = std::time::Instant::now() + WAIT;
    while lock(&taken).is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "no wake found the answer: it was told before it could be taken"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(lock(&taken).take().unwrap().result.is_ok());
    assert_eq!(deadlocked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_held_command_wakes_once_as_it_answers_and_nothing_wakes_while_nothing_is_pending() {
    let f = opened(BOX);
    let count = counting(&f.sessions);
    // Counted from a point no wake is still to come of (the open's, which follows its
    // answer, among them): every wake after it is this test's.
    let base = settled(&f, &count);
    let (started, through) = shut(&f.session().gate);
    let text = f.sessions.text(&f.id, None);
    started.recv_timeout(WAIT).unwrap();
    // Held: nothing woke.
    assert_eq!(count.load(Ordering::SeqCst), base);
    through.send(()).unwrap();
    woken(&count, base + 1);
    text.try_take().ok().unwrap().result.unwrap();
    // Held, nothing woke; answered, once; and nothing since.
    assert_eq!(settled(&f, &count), base + 2);
}

/// Two boxes, neither using the other, each at the origin (so neither is moved): a step
/// each, each held by its own size.
const TWO_BOXES: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}]}"#;

#[test]
fn each_step_of_a_held_rebuild_wakes_once_as_it_settles_then_its_end_and_its_answer() {
    // Two boxes, one step each, each held by its own size; the rebuild's watcher held
    // before its first look. Its plan made and the block's step settled before it looks:
    // that look tells both, once (the progress moved since the rebuild began); then the
    // lug's step, the rebuild's end, and the check's answer, once each.
    let f = opened(TWO_BOXES);
    let count = counting(&f.sessions);
    let base = settled(&f, &count);
    let (watching, watched) = shut(&f.session().watching);
    let block = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 20.0));
    let lug = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx < 20.0));
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    assert!(f.kernel.wait_running(2, WAIT));
    watching.recv_timeout(WAIT).unwrap();
    block.release();
    let deadline = Instant::now() + WAIT;
    while check.progress().unwrap().done < 1 {
        assert!(Instant::now() < deadline, "the block's step never settled");
        std::thread::sleep(Duration::from_millis(1));
    }
    // Settled before the watcher looked: nothing told yet.
    assert_eq!(count.load(Ordering::SeqCst), base);
    watched.send(()).unwrap();
    drop(watched);
    open_gate(&f.session().watching);
    woken(&count, base + 1);
    lug.release();
    woken(&count, base + 2);
    check.wait().result.unwrap();
    // The first look's, the lug's step, the rebuild's end (its model published), and the
    // check's answer; then `settled`'s own.
    assert_eq!(settled(&f, &count), base + 5);
}

#[test]
fn rebuild_told_counts_a_change_told_only_once_its_wake_has_returned() {
    // `Sessions::rebuild_told`, for a test to wait on by an event: the watcher's count of
    // the changes it told is behind the count's while the client's waker is still being
    // woken for them, and equal once it has returned; then, the box held, it stays equal.
    let f = opened(BOX);
    let count = counting(&f.sessions);
    settled(&f, &count);
    // From here, the first wake (the rebuild's watcher's, for the check's rebuild's first
    // changes) is held until the test lets it go.
    let (entered, waiting) = std::sync::mpsc::channel();
    let (go, held) = std::sync::mpsc::channel::<()>();
    let held = Mutex::new(held);
    let first = AtomicBool::new(true);
    f.sessions.on_change(move || {
        if first.swap(false, Ordering::SeqCst) {
            let _ = entered.send(());
            let _ = lock(&held).recv();
        }
    });
    let hold = f.kernel.hold(is_box);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    waiting.recv_timeout(WAIT).unwrap();
    let (changes, told) = f.sessions.rebuild_told(&f.id).unwrap();
    assert!(
        changes > 0 && told < changes,
        "({changes}, {told}) while its wake is held"
    );
    go.send(()).unwrap();
    assert!(f.kernel.wait_running(1, WAIT));
    let deadline = Instant::now() + WAIT;
    while f
        .sessions
        .rebuild_told(&f.id)
        .is_none_or(|(changes, told)| changes != told)
    {
        assert!(Instant::now() < deadline, "never all told");
        std::thread::sleep(Duration::from_millis(1));
    }
    let all = f.sessions.rebuild_told(&f.id);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(
        f.sessions.rebuild_told(&f.id),
        all,
        "a change while the box is held"
    );
    hold.release();
    check.wait().result.unwrap();
}

#[test]
fn a_finding_wakes_only_as_it_changes_and_the_sessions_own_reload_once() {
    let w = watched(BOX);
    let count = counting(&w.f.sessions);
    let base = settled(&w.f, &count);
    // The first comparison's finding, from none to unchanged.
    w.f.status();
    woken(&count, base + 1);
    // Unchanged again: nothing.
    w.clock.advance(DISK_CHECK_INTERVAL);
    let ended = w.reader.ended();
    w.f.status();
    let deadline = std::time::Instant::now() + WAIT;
    while w.reader.ended() == ended {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    // An agent's commit: the finding changed, then, once it settles, the session's reload.
    // Asked, a full interval later each time, until the comparison starts: the one before
    // may still be ending as the reader says it ended, and then stamps its end at the
    // clock's time after this advance, so a next is due only an interval after that.
    std::fs::write(&w.f.path, SPHERE).unwrap();
    let before = w.reader.started();
    let deadline = std::time::Instant::now() + WAIT;
    while w.reader.started() == before {
        w.clock.advance(DISK_CHECK_INTERVAL);
        w.f.status();
        assert!(
            std::time::Instant::now() < deadline,
            "no comparison started"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    woken(&count, base + 2);
    // The next comparison finds the same bytes: the change settled, and the session's
    // reload queued (#479), the finding as it was.
    w.compare();
    woken(&count, base + 3);
    assert_eq!(w.f.at(), Revision(2));
    // Three, and none besides (the second comparison, unchanged, and the fourth, changed
    // still, told nothing); then `settled`'s own.
    assert_eq!(settled(&w.f, &count), base + 4);
}

// ---- #297's review ----

#[test]
fn a_part_written_file_reads_changed_and_is_never_reloaded() {
    // The session's own save left it part written: the next save says where the old text
    // is, so no reload takes the half-written file.
    let f = opened(STORED);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let old = f.path.with_file_name(".a.silico.fmt-1");
    *f.session().held.session_changing().part_written_mut() = Some(old);
    std::fs::write(&f.path, "part written").unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    f.status_stays(Duration::from_millis(300), |status, revision| {
        revision == Revision::OPENED
            && status.disk == Some(Disk::Changed)
            && status.running.is_none()
    });
}

#[test]
fn a_published_read_wakes_the_client_once_as_it_answers() {
    let f = opened(BOX);
    // Published once.
    f.sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap();
    let count = counting(&f.sessions);
    let base = settled(&f, &count);
    let read = f.sessions.check(&f.id, CheckModel::Published);
    woken(&count, base + 1);
    read.try_take().ok().unwrap().result.unwrap();
    // Once, and nothing since; then `settled`'s own.
    assert_eq!(settled(&f, &count), base + 2);
}

#[test]
fn an_open_wakes_the_client_as_it_answers_and_so_does_one_refused() {
    // Counted from before the first open, so each open's wake is counted, whenever it
    // comes: the wake follows the answer on the open's own thread, so a count installed
    // after an open answered could meet that open's wake late (a loaded machine) and count
    // it as the next one's. At the end the sessions dropped, which joins every session's
    // thread: no wake can come after, so the count is exact.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let sessions = Sessions::with_stand_ins(
        Arc::new(FakeKernel::new(2).unwrap()),
        move |cache| Cache::with_secret(cache, &secret),
        || Ok(Box::new(crate::fake::FakeSketches::new()) as Box<dyn SketchSolver>),
    )
    .checking_disk_every(Duration::ZERO);
    let count = counting(&sessions);
    // Each open's wake comes on its own session's thread before any command it runs: a
    // `text` held there counts exactly what came, one wake for the open and, after, one
    // for the `text` itself.
    for (n, name) in ["a.silico", "b.silico"].into_iter().enumerate() {
        let path = dir.path().join(name);
        std::fs::write(&path, BOX).unwrap();
        let opened = sessions.open(&path, false).wait();
        let id = opened.session.unwrap().id;
        opened.result.unwrap();
        assert_eq!(settled_in(&sessions, &id, &count), 2 * n + 2);
    }
    let refused = sessions.open(dir.path().join("none.silico"), false).wait();
    assert!(refused.result.is_err());
    woken(&count, 5);
    drop(sessions);
    // One wake an open, the refused one's too, and none besides.
    assert_eq!(count.load(Ordering::SeqCst), 5);
}

/// A cache opener held, while it's held, as a session opens: the open waits in it.
#[derive(Default)]
struct HeldOpen {
    held: Mutex<bool>,
    opened: Condvar,
    entered: AtomicUsize,
}

#[test]
fn a_read_sent_while_its_session_opens_is_answered_before_the_wake_that_tells_of_it() {
    // The open cancelled while held, so the read sent meanwhile is refused as naming no
    // session: answered, then the client woken once, with the registry's book let go, from
    // inside which the waker can take the read's answer.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("a.silico");
    std::fs::write(&path, BOX).unwrap();
    let gate = Arc::new(HeldOpen::default());
    *lock(&gate.held) = true;
    let opener = Arc::clone(&gate);
    let sessions = Arc::new(Sessions::with_stand_ins(
        Arc::new(FakeKernel::new(1).unwrap()),
        move |cache| {
            opener.entered.fetch_add(1, Ordering::SeqCst);
            let mut held = lock(&opener.held);
            while *held {
                held = opener.opened.wait(held).unwrap();
            }
            Cache::with_secret(cache, &secret)
        },
        || Ok(Box::new(crate::fake::FakeSketches::new()) as Box<dyn SketchSolver>),
    ));
    let slot: Arc<Mutex<Option<Running<CheckAnswer>>>> = Arc::default();
    let taken = Arc::new(AtomicUsize::new(0));
    let wakes = Arc::new(AtomicUsize::new(0));
    {
        let (slot, taken, wakes) = (Arc::clone(&slot), Arc::clone(&taken), Arc::clone(&wakes));
        sessions.on_change(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
            let running = lock(&slot).take();
            if let Some(running) = running {
                match running.try_take() {
                    Ok(_) => {
                        taken.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(running) => *lock(&slot) = Some(running),
                }
            }
        });
    }
    let opening = sessions.open(&path, false);
    let deadline = std::time::Instant::now() + WAIT;
    while gate.entered.load(Ordering::SeqCst) == 0 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    *lock(&slot) = Some(sessions.check(&SessionId(1), CheckModel::Published));
    assert!(opening.cancel());
    *lock(&gate.held) = false;
    gate.opened.notify_all();
    let deadline = std::time::Instant::now() + WAIT;
    while taken.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "no wake found the read's answer"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
}

#[test]
fn a_reload_that_finds_the_file_as_held_moves_the_finding_itself() {
    // With the clock held, no comparison runs after the two that found the change (the
    // second settling it, #479): only the reload, finding the file put back, can say it's
    // unchanged.
    let w = watched(BOX);
    let f = &w.f;
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let (started, through) = shut(&f.session().reloading);
    std::fs::write(&f.path, SPHERE).unwrap();
    w.compare();
    assert_eq!(f.status().disk, Some(Disk::Changed));
    w.compare();
    started.recv_timeout(WAIT).unwrap();
    let reads = w.reader.started();
    std::fs::write(&f.path, BOX).unwrap();
    open_gate(&f.session().reloading);
    through.send(()).unwrap();
    f.status_reaches(|status, _| status.running.is_none() && status.disk == Some(Disk::Unchanged));
    assert_eq!(w.reader.started(), reads, "a comparison ran");
    assert_eq!(f.at(), Revision::OPENED);
}

#[test]
fn a_refused_reload_is_tried_again_once_the_claim_that_refused_it_is_let_go() {
    // s1's path a link to s2's document: refused while s2 holds it; once s2 closes, the
    // same bytes are reloaded.
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let other = f.path.with_file_name("b.silico");
    std::fs::write(&other, SPHERE).unwrap();
    let second = f.sessions.open(&other, false).wait().session.unwrap().id;
    std::fs::remove_file(&f.path).unwrap();
    std::os::unix::fs::symlink(&other, &f.path).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    f.status_stays(Duration::from_millis(200), |_, revision| {
        revision == Revision::OPENED
    });
    f.sessions.close(&second).wait().result.unwrap();
    f.status_reaches(|status, revision| {
        revision == Revision(2) && status.disk == Some(Disk::Unchanged)
    });
    assert_eq!(f.held_text(), SPHERE);
}

#[test]
fn bytes_a_reload_refused_come_back_after_another_reload_and_are_tried_again() {
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let other = f.path.with_file_name("b.silico");
    std::fs::write(&other, SPHERE).unwrap();
    let _second = f.sessions.open(&other, false).wait().session.unwrap().id;
    let link = || {
        let _ = std::fs::remove_file(&f.path);
        std::os::unix::fs::symlink(&other, &f.path).unwrap();
    };
    // Asked until a reload starts at the gate: a finding of `changed` from before (the
    // refused bytes still there) answers at once, and a comparison running then may have
    // read the bytes before, so each `status` may start the comparison that finds them.
    let reload_starts = |started: &Receiver<()>| {
        let deadline = Instant::now() + WAIT;
        loop {
            f.status();
            if started.try_recv().is_ok() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    // A reload let through, then the gate opened until nothing runs (a stale comparison's
    // reload, asked for after one took its bytes, finds them as held), and shut again.
    let through_then_settle = |through: Sender<()>, want: &dyn Fn(&Status, Revision) -> bool| {
        through.send(()).unwrap();
        drop(through);
        open_gate(&f.session().reloading);
        f.status_reaches(|status, revision| want(status, revision) && status.running.is_none());
    };
    let (started, through) = shut(&f.session().reloading);
    link();
    assert!(
        reload_starts(&started),
        "the link to s2's file queued no reload"
    );
    through_then_settle(through, &|_, _| true);
    // Other bytes of its own: reloaded.
    let (started, through) = shut(&f.session().reloading);
    std::fs::remove_file(&f.path).unwrap();
    std::fs::write(&f.path, BOX.replace("Block", "Brick")).unwrap();
    assert!(reload_starts(&started), "the new bytes queued no reload");
    through_then_settle(through, &|status, revision| {
        revision == Revision(2) && status.disk == Some(Disk::Unchanged)
    });
    // Any reload taken forgets the refusal.
    assert!(lock(&f.session().reloads).refused.is_none());
    // The refused bytes back: tried again, not passed over.
    let (started, through) = shut(&f.session().reloading);
    link();
    assert!(
        reload_starts(&started),
        "the bytes refused before weren't tried again"
    );
    open_gate(&f.session().reloading);
    drop(through);
}

#[test]
fn a_comparison_stuck_reading_keeps_no_waker_alive_past_the_sessions() {
    let w = watched(BOX);
    w.f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let token = Arc::new(());
    let held = Arc::clone(&token);
    w.f.sessions.on_change(move || {
        let _ = &held;
    });
    assert_eq!(Arc::strong_count(&token), 2);
    w.reader.hold();
    w.clock.advance(DISK_CHECK_INTERVAL);
    let before = w.reader.started();
    w.f.status();
    let deadline = std::time::Instant::now() + WAIT;
    while w.reader.started() == before {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let Watched { f, reader, .. } = w;
    drop(f);
    assert_eq!(
        Arc::strong_count(&token),
        1,
        "the stuck comparison holds the waker"
    );
    reader.release();
}

#[test]
fn a_refused_reload_isnt_tried_again_while_the_session_holding_its_file_only_reads_it() {
    // The claim that refused it unchanged (its holder's own commands claim the file it
    // holds again, which changes no hands): no reload is queued again, however many
    // comparisons find the file changed.
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let other = f.path.with_file_name("b.silico");
    std::fs::write(&other, SPHERE).unwrap();
    let second = f.sessions.open(&other, false).wait().session.unwrap().id;
    let (started, through) = shut(&f.session().reloading);
    std::fs::remove_file(&f.path).unwrap();
    std::os::unix::fs::symlink(&other, &f.path).unwrap();
    f.status_reaches(|status, _| status.disk == Some(Disk::Changed));
    reload_reaches_its_gate(&f, &started);
    through.send(()).unwrap();
    f.status_reaches(|status, _| status.running.is_none());
    for _ in 0..20 {
        // The holder busy: each command reads its file, claiming it again.
        f.sessions.text(&second, None).wait().result.unwrap();
        f.status();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        started.try_recv().is_err(),
        "the refused reload was tried again"
    );
    open_gate(&f.session().reloading);
    drop(through);
    assert_eq!(f.at(), Revision::OPENED);
}

#[test]
fn a_comparison_the_session_read_past_before_it_told_queues_no_reload() {
    // A comparison finds the agent's bytes, keeps its finding, and is held before it tells
    // the session; meanwhile a command reads the file and reloads it (the count of reads
    // moves on). Let go, the comparison is the session's no longer: dropped, never reported
    // (the design's section 4), so no second reload is queued for bytes already taken.
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let (at_hold, held_there) = mpsc::channel::<()>();
    let (release, released) = mpsc::channel::<()>();
    let released = Arc::new(Mutex::new(released));
    let at_hold = Mutex::new(at_hold);
    f.session()
        .disk
        .get()
        .unwrap()
        .before_changed(Arc::new(move || {
            let _ = lock(&at_hold).send(());
            // Until the test lets it go (its sender dropped).
            let _ = lock(&released).recv();
        }));
    // A reload, if one is queued, held where the test can see it start.
    let (reload_started, through) = shut(&f.session().reloading);
    std::fs::write(&f.path, SPHERE).unwrap();
    let deadline = Instant::now() + WAIT;
    while held_there.try_recv().is_err() {
        f.status();
        assert!(Instant::now() < deadline, "no comparison found the change");
        std::thread::sleep(Duration::from_millis(2));
    }
    // The command reloads the file before it runs: revision 2, the agent's text.
    let text = f.sessions.text(&f.id, None).wait();
    let session = text.session.unwrap();
    assert!(session.reloaded);
    assert_eq!(session.revision, Revision(2));
    assert_eq!(text.result.unwrap().text, SPHERE);
    // Let go: its telling done, it queued nothing.
    drop(release);
    let deadline = Instant::now() + WAIT;
    while f.session().disk.get().unwrap().changed_told() == 0 {
        assert!(
            Instant::now() < deadline,
            "the comparison never told the session"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!lock(&f.session().reloads).queued);
    assert!(reload_started.try_recv().is_err());
    assert_eq!(f.at(), Revision(2));
    open_gate(&f.session().reloading);
    drop(through);
}

// ---- #354's mutants: each guard of `reload_wanted` alone ----

/// `f`'s file written as `bytes` by another program, and `status` asked until a comparison
/// has found it and told the session (its own reload's gate shut first): whether a reload
/// was queued for it, read at once (a queued reload waits at the gate, `queued` still
/// set), and whether one reached the gate. The gate is opened again after.
fn queued_for(f: &Fixture, bytes: &str) -> (bool, bool) {
    let (reload_started, through) = shut(&f.session().reloading);
    std::fs::write(&f.path, bytes).unwrap();
    let deadline = Instant::now() + WAIT;
    while f.session().disk.get().unwrap().changed_told() == 0 {
        f.status();
        assert!(Instant::now() < deadline, "no comparison told the session");
        std::thread::sleep(Duration::from_millis(1));
    }
    let queued = lock(&f.session().reloads).queued;
    let started = reload_started.try_recv().is_ok();
    open_gate(&f.session().reloading);
    drop(through);
    (queued, started)
}

#[test]
fn a_change_found_inside_a_transaction_with_nothing_part_written_queues_no_reload() {
    // Inside a transaction, only `commit` reads the file (decision 2): a change found on
    // disk queues no reload of its own, whatever else holds.
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    f.begin();
    assert_eq!(f.session().held.read().part_written(), None);
    assert_eq!(queued_for(&f, SPHERE), (false, false));
    assert_eq!(f.at(), Revision::OPENED);
    assert_eq!(f.held_text(), BOX);
}

#[test]
fn a_change_to_a_part_written_file_with_no_transaction_queues_no_reload() {
    // The session's own save left the file part written: the next save says where its old
    // text is, so no reload takes the half-written file, even with no transaction open.
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    *f.session().held.session_changing().part_written_mut() =
        Some(f.path.with_file_name(".a.silico.fmt-1"));
    assert!(f.session().held.read().transaction.is_none());
    assert_eq!(queued_for(&f, "part written"), (false, false));
    assert_eq!(f.at(), Revision::OPENED);
}

#[test]
fn a_change_with_neither_queues_the_sessions_own_reload() {
    // The two above's control: with no transaction and nothing part written, the same
    // finding queues the reload, which waits at the gate.
    let f = opened(BOX);
    f.status_reaches(|status, _| status.disk == Some(Disk::Unchanged));
    let (reload_started, through) = shut(&f.session().reloading);
    std::fs::write(&f.path, SPHERE).unwrap();
    let deadline = Instant::now() + WAIT;
    while f.session().disk.get().unwrap().changed_told() == 0 {
        f.status();
        assert!(Instant::now() < deadline, "no comparison told the session");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(lock(&f.session().reloads).queued);
    reload_started.recv_timeout(WAIT).unwrap();
    open_gate(&f.session().reloading);
    drop(through);
    f.status_reaches(|status, revision| {
        revision == Revision(2) && status.disk == Some(Disk::Unchanged)
    });
    assert_eq!(f.held_text(), SPHERE);
}

mod in_place;
