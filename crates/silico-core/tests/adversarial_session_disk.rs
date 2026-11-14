//! Hostile inputs for a session's disk watch (`Sessions::disk`, break-it, PR #269): what the
//! session's own reads and writes leave (a created file, a non-canonical one, saves by edit,
//! commit and format, a save back to the text opened, a save refused, a save that can't be
//! written), what another leaves at its path (the same bytes again, nothing, a directory, a
//! pipe, a file past the limit, a link out of the workspace), and when (asked while a
//! command waits on its rebuild, asked from another thread through many saves, asked of a
//! session closed and of many sessions). The sessions here compare at most once a second
//! (`DISK_CHECK_INTERVAL`), as the app's do. Expected values: the bytes the tests wrote,
//! and the rule that the session's own saves and reloads never read as changes, while what
//! the next command will find (a reload, a refusal) is what `disk` said.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{Revision, SessionId, Sessions};
use silico_core::disk::Disk;
use silico_core::document::json::{MAX_INPUT_LEN, Node};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Op, Primitive};
use tempfile::TempDir;

use crate::sessions::{answer, id};

/// A block, 40 × 20 × 10, on one line: not canonical form.
const BLOCK: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "fingerprint": {"volume": 8000, "area": 2800, "center": [20, 10, 5], "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}]}"#;

/// Longer than one comparison's interval (a second) and its read, with room on a loaded
/// machine: what a test waits for a finding it expects.
const WAIT: Duration = Duration::from_secs(20);

/// Long enough for at least two comparisons at the one-second interval.
const TWO_COMPARISONS: Duration = Duration::from_millis(2600);

struct Fixture {
    dir: TempDir,
    kernel: Arc<FakeKernel>,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
    path: PathBuf,
}

fn sessions(dir: &Path, kernel: &Arc<FakeKernel>) -> Sessions<Arc<FakeKernel>> {
    let secret = dir.join("secret");
    Sessions::with_cache_opener(Arc::clone(kernel), move |cache| {
        Cache::with_secret(cache, &secret)
    })
}

/// Sessions on a fake kernel, with `text` in `a.silico` open.
fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = sessions(dir.path(), &kernel);
    let path = dir.path().join("a.silico");
    std::fs::write(&path, text).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    Fixture {
        dir,
        kernel,
        sessions,
        id,
        path,
    }
}

impl Fixture {
    fn disk(&self) -> Option<Disk> {
        self.sessions.disk(&self.id).unwrap()
    }

    /// The finding once it's `want`, within [`WAIT`].
    #[track_caller]
    fn reaches(&self, want: Disk) {
        let deadline = Instant::now() + WAIT;
        loop {
            let found = self.disk();
            if found == Some(want) {
                return;
            }
            assert!(Instant::now() < deadline, "never {want:?}: last {found:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Asked for `long`, every finding one `allowed` allows.
    #[track_caller]
    fn stays(&self, long: Duration, allowed: impl Fn(Option<Disk>) -> bool) {
        let deadline = Instant::now() + long;
        while Instant::now() < deadline {
            let found = self.disk();
            assert!(allowed(found), "found {found:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn file(&self) -> Vec<u8> {
        std::fs::read(&self.path).unwrap()
    }

    /// Once the session has reloaded its file itself (M5.0's A3), with no command sent:
    /// its revision past `from` and the file unchanged, within [`WAIT`]. Its new revision.
    #[track_caller]
    fn reloads_itself(&self, from: Revision) -> Revision {
        let deadline = Instant::now() + WAIT;
        loop {
            // `status` starts the comparison that finds the change, as the app's frames do.
            let status = self.sessions.status(&self.id);
            let revision = status.session.unwrap().revision;
            if revision > from && status.result.unwrap().disk == Some(Disk::Unchanged) {
                return revision;
            }
            assert!(Instant::now() < deadline, "never reloaded past {from:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn set_length(
        &self,
        revision: Revision,
        length: &str,
    ) -> silico_core::command::Response<silico_core::command::FieldSet> {
        answer(self.sessions.set_field(
            &self.id,
            revision,
            id("k7f2qx3m"),
            "length".to_owned(),
            value(length),
        ))
    }

    fn revision(&self) -> Revision {
        answer(self.sessions.text(&self.id, None))
            .session
            .unwrap()
            .revision
    }
}

fn value(text: &str) -> Node {
    silico_core::document::json::read(text.as_bytes()).unwrap()
}

fn is_box(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
}

fn unchanged(found: Option<Disk>) -> bool {
    found == Some(Disk::Unchanged)
}

fn root() -> bool {
    // A directory made read-only still takes root's writes.
    let dir = tempfile::tempdir().unwrap();
    let probe = dir.path().join("sub");
    std::fs::create_dir(&probe).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writes = std::fs::write(probe.join("p"), "").is_ok();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
    writes
}

/// Makes `path` `mode` until dropped, then 0o755.
struct Mode(PathBuf);

impl Mode {
    fn of(path: &Path, mode: u32) -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        Mode(path.to_owned())
    }
}

impl Drop for Mode {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

// ---- The session's own reads and writes ----

#[test]
fn a_document_the_session_made_is_unchanged_and_never_reads_as_changed() {
    let dir = tempfile::tempdir().unwrap();
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let sessions = sessions(dir.path(), &kernel);
    let path = dir.path().join("new.silico");
    let opened = answer(sessions.open(&path, true));
    assert!(opened.result.is_ok(), "{opened:?}");
    let session = opened.session.unwrap().id;
    let deadline = Instant::now() + TWO_COMPARISONS;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        seen.push(sessions.disk(&session).unwrap());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(seen.contains(&Some(Disk::Unchanged)), "{seen:?}");
    assert!(
        seen.iter()
            .all(|found| found.is_none() || unchanged(*found)),
        "{seen:?}"
    );
}

#[test]
fn a_file_not_in_canonical_form_is_unchanged_as_opened() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    f.stays(TWO_COMPARISONS, unchanged);
}

#[test]
fn an_edits_own_save_is_unchanged_at_once_and_after_every_comparison() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    let set = f.set_length(Revision::OPENED, "41");
    assert!(set.result.unwrap().commit.unwrap().written);
    assert_ne!(f.file(), BLOCK.as_bytes());
    assert_eq!(f.disk(), Some(Disk::Unchanged));
    f.stays(TWO_COMPARISONS, unchanged);
}

#[test]
fn formats_own_save_is_unchanged_at_once_and_after_every_comparison() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    let formatted = answer(f.sessions.format(&f.id, None, true));
    assert!(formatted.result.unwrap().written);
    assert_eq!(f.disk(), Some(Disk::Unchanged));
    f.stays(TWO_COMPARISONS, unchanged);
}

#[test]
fn saves_back_to_the_text_the_session_wrote_before_are_unchanged_and_others_text_is_changed() {
    let f = opened(BLOCK);
    assert!(
        answer(f.sessions.format(&f.id, None, true))
            .result
            .unwrap()
            .written
    );
    let canonical = f.file();
    let set = f.set_length(f.revision(), "41");
    assert!(set.result.unwrap().commit.unwrap().written);
    let longer = f.file();
    // A, then B, then A again, by bytes.
    let set = f.set_length(f.revision(), "40");
    assert!(set.result.unwrap().commit.unwrap().written);
    assert_eq!(f.file(), canonical);
    assert_eq!(f.disk(), Some(Disk::Unchanged));
    f.stays(TWO_COMPARISONS, unchanged);
    // B again, by another: a change, though the session wrote it once. Inside a
    // transaction, which reloads nothing, so the change stays for the watch to find.
    answer(f.sessions.begin(&f.id)).result.unwrap();
    std::fs::write(&f.path, &longer).unwrap();
    f.reaches(Disk::Changed);
}

#[test]
fn asked_from_another_thread_through_many_saves_the_sessions_own_never_reads_as_changed() {
    let f = Arc::new(opened(BLOCK));
    f.reaches(Disk::Unchanged);
    let done = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let asker = {
        let (f, done, seen) = (Arc::clone(&f), Arc::clone(&done), Arc::clone(&seen));
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let found = f.disk();
                if found != Some(Disk::Unchanged) {
                    seen.lock().unwrap().push(found);
                }
            }
        })
    };
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut saves = 0;
    while Instant::now() < deadline {
        let length = if saves % 2 == 0 { "41" } else { "40" };
        let set = f.set_length(f.revision(), length);
        assert!(set.result.unwrap().commit.unwrap().written);
        saves += 1;
    }
    done.store(true, Ordering::Relaxed);
    asker.join().unwrap();
    assert!(saves > 4, "{saves}");
    let seen = seen.lock().unwrap();
    // Only "unknown" mid-save: never changed, gone, or unreadable.
    assert!(seen.iter().all(Option::is_none), "{seen:?}");
}

#[test]
fn a_commit_refused_as_a_conflict_leaves_the_file_changed_and_a_rollback_and_reload_unchanged() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    f.set_length(Revision::OPENED, "41").result.unwrap();
    let theirs = BLOCK.replace(r#""height": 10"#, r#""height": 11"#);
    std::fs::write(&f.path, &theirs).unwrap();
    f.reaches(Disk::Changed);
    let commit = answer(f.sessions.commit(&f.id));
    assert_eq!(
        commit.result.unwrap_err().errors()[0].code(),
        ErrorCode::Conflict
    );
    assert_eq!(f.file(), theirs.as_bytes());
    // Their text is still a change: never unchanged, nor gone.
    f.stays(TWO_COMPARISONS, |found| {
        found.is_none() || found == Some(Disk::Changed)
    });
    f.reaches(Disk::Changed);
    let rolled = answer(f.sessions.rollback(&f.id));
    rolled.result.unwrap();
    // Rolled back, the file isn't read by the rollback; with no transaction open, the next
    // comparison's change is the session's own reload, which no command asks for.
    let reloaded = f.reloads_itself(rolled.session.unwrap().revision);
    let text = answer(f.sessions.text(&f.id, None));
    let session = text.session.unwrap();
    assert!(!session.reloaded);
    assert_eq!(session.revision, reloaded);
    assert_eq!(text.result.unwrap().text, theirs);
    f.stays(TWO_COMPARISONS, unchanged);
}

#[test]
fn a_commit_over_a_changed_file_that_writes_nothing_leaves_it_for_the_sessions_own_reload() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    let theirs = BLOCK.replace("Block", "Other");
    std::fs::write(&f.path, &theirs).unwrap();
    f.reaches(Disk::Changed);
    let commit = answer(f.sessions.commit(&f.id));
    let at = commit.session.unwrap().revision;
    assert!(!commit.result.unwrap().written);
    // Their text, never written over: the transaction ended, the session reloads it.
    f.reloads_itself(at);
    assert_eq!(f.file(), theirs.as_bytes());
    f.stays(TWO_COMPARISONS, unchanged);
}

#[test]
fn a_reload_of_text_the_gate_refuses_is_unchanged_at_once() {
    // The empty file is never the session's own reload (#479): the test after this one.
    // One byte, the least that settles, as few as a broken file may hold.
    for text in ["{", "not json", "{\"silico\": 1}", "\u{feff}{}", "\0\0\0"] {
        let f = opened(BLOCK);
        f.reaches(Disk::Unchanged);
        std::fs::write(&f.path, text).unwrap();
        // The session's own reload holds it, its problems with it (decision 21).
        f.reloads_itself(Revision::OPENED);
        let read = answer(f.sessions.text(&f.id, None));
        assert!(!read.session.unwrap().reloaded, "{text:?}");
        // The text the gate refuses is the session's, as the file holds it.
        assert_eq!(read.result.unwrap().text, text, "{text:?}");
        let status = f.sessions.status(&f.id).result.unwrap();
        assert!(!status.gated, "{text:?}");
        assert_eq!(f.disk(), Some(Disk::Unchanged), "{text:?}");
        f.stays(TWO_COMPARISONS, unchanged);
    }
}

#[test]
fn an_empty_file_is_changed_never_reloaded_by_the_session_and_a_command_reads_it() {
    // Never a document, and what a write in place leaves until it writes (#479): the
    // session's own reload never takes it, however many comparisons find it, and `disk`
    // says changed all the while; the next command reads it as it is, its problems with it
    // (decision 21), and the session then holds it as the file does.
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    std::fs::write(&f.path, "").unwrap();
    f.reaches(Disk::Changed);
    let deadline = Instant::now() + TWO_COMPARISONS;
    while Instant::now() < deadline {
        let status = f.sessions.status(&f.id);
        assert_eq!(status.session.unwrap().revision, Revision::OPENED);
        assert_eq!(status.result.unwrap().disk, Some(Disk::Changed));
        std::thread::sleep(Duration::from_millis(5));
    }
    let read = answer(f.sessions.text(&f.id, None));
    let session = read.session.unwrap();
    assert!(session.reloaded);
    assert!(session.revision > Revision::OPENED);
    assert_eq!(read.result.unwrap().text, "");
    let status = f.sessions.status(&f.id).result.unwrap();
    assert!(!status.gated);
    assert_eq!(f.disk(), Some(Disk::Unchanged));
    f.stays(TWO_COMPARISONS, unchanged);
}

#[test]
fn the_same_bytes_written_again_by_another_or_renamed_over_are_unchanged() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    std::fs::write(&f.path, BLOCK).unwrap();
    f.stays(TWO_COMPARISONS, unchanged);
    // An editor's safe save: a new file, renamed over.
    let beside = f.dir.path().join(".a.silico.tmp");
    std::fs::write(&beside, BLOCK).unwrap();
    std::fs::rename(&beside, &f.path).unwrap();
    f.stays(TWO_COMPARISONS, unchanged);
    let text = answer(f.sessions.text(&f.id, None));
    assert!(!text.session.unwrap().reloaded);
}

// ---- What another leaves at the path ----

#[test]
fn a_file_past_the_limit_is_unreadable_and_the_refused_reload_leaves_it_so() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    std::fs::write(&f.path, vec![b' '; MAX_INPUT_LEN + 1]).unwrap();
    f.reaches(Disk::Unreadable);
    let text = answer(f.sessions.text(&f.id, None));
    assert!(text.result.is_err());
    assert!(!text.session.unwrap().reloaded);
    f.stays(TWO_COMPARISONS, |found| found == Some(Disk::Unreadable));
    std::fs::write(&f.path, BLOCK).unwrap();
    f.reaches(Disk::Unchanged);
}

#[test]
fn a_file_at_the_limit_is_a_change_and_its_reload_unchanged() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    // Inside a transaction first, which reloads nothing: at the limit it's a change, never
    // unreadable.
    answer(f.sessions.begin(&f.id)).result.unwrap();
    std::fs::write(&f.path, vec![b' '; MAX_INPUT_LEN]).unwrap();
    f.reaches(Disk::Changed);
    let rolled = answer(f.sessions.rollback(&f.id));
    // Then, rolled back, the session reloads it itself, a file it may hold whole.
    f.reloads_itself(rolled.session.unwrap().revision);
    let text = answer(f.sessions.text(&f.id, None));
    assert!(!text.session.unwrap().reloaded);
    assert_eq!(text.result.unwrap().text.len(), MAX_INPUT_LEN);
    f.stays(TWO_COMPARISONS, unchanged);
}

#[test]
fn a_directory_at_the_path_is_unreadable_and_the_next_command_refused() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    std::fs::remove_file(&f.path).unwrap();
    std::fs::create_dir(&f.path).unwrap();
    f.reaches(Disk::Unreadable);
    assert!(answer(f.sessions.text(&f.id, None)).result.is_err());
    f.stays(TWO_COMPARISONS, |found| found == Some(Disk::Unreadable));
    std::fs::remove_dir(&f.path).unwrap();
    f.reaches(Disk::Gone);
    std::fs::write(&f.path, BLOCK).unwrap();
    f.reaches(Disk::Unchanged);
}

#[test]
fn a_pipe_at_the_path_is_unreadable_and_neither_disk_nor_the_command_waits_on_it() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    std::fs::remove_file(&f.path).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&f.path)
        .status()
        .unwrap();
    assert!(made.success());
    let deadline = Instant::now() + WAIT;
    loop {
        // A disk that waited on the pipe would never return: no clock needed.
        let found = f.disk();
        if found == Some(Disk::Unreadable) {
            break;
        }
        assert!(Instant::now() < deadline, "never unreadable: {found:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(answer(f.sessions.text(&f.id, None)).result.is_err());
    std::fs::remove_file(&f.path).unwrap();
    std::fs::write(&f.path, BLOCK).unwrap();
    f.reaches(Disk::Unchanged);
}

#[test]
fn a_file_gone_and_its_directory_gone_are_gone_until_put_back() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    std::fs::remove_file(&f.path).unwrap();
    f.reaches(Disk::Gone);
    assert!(answer(f.sessions.text(&f.id, None)).result.is_err());
    f.stays(TWO_COMPARISONS, |found| found == Some(Disk::Gone));
    std::fs::write(&f.path, BLOCK).unwrap();
    f.reaches(Disk::Unchanged);
    // Its directory moved away whole.
    let away = f.dir.path().with_extension("away");
    std::fs::rename(f.dir.path(), &away).unwrap();
    f.reaches(Disk::Gone);
    std::fs::rename(&away, f.dir.path()).unwrap();
    f.reaches(Disk::Unchanged);
}

#[test]
fn a_file_this_user_cant_read_is_unreadable_and_a_save_over_it_leaves_it_so() {
    if root() {
        return;
    }
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    f.set_length(Revision::OPENED, "41").result.unwrap();
    let unreadable = Mode::of(&f.path, 0o000);
    f.reaches(Disk::Unreadable);
    assert!(answer(f.sessions.commit(&f.id)).result.is_err());
    // Not stuck unknown: compared again, as it is.
    f.reaches(Disk::Unreadable);
    drop(unreadable);
    f.reaches(Disk::Unchanged);
    // The next valid save still works, and is its own.
    assert!(answer(f.sessions.commit(&f.id)).result.unwrap().written);
    assert_eq!(f.disk(), Some(Disk::Unchanged));
    f.stays(TWO_COMPARISONS, unchanged);
}

#[test]
fn a_save_that_cant_be_written_is_unknown_then_unchanged_and_the_next_save_its_own() {
    if root() {
        return;
    }
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    f.set_length(Revision::OPENED, "41").result.unwrap();
    let read_only = Mode::of(f.dir.path(), 0o555);
    assert!(answer(f.sessions.commit(&f.id)).result.is_err());
    assert_eq!(f.file(), BLOCK.as_bytes());
    f.reaches(Disk::Unchanged);
    f.stays(TWO_COMPARISONS, unchanged);
    drop(read_only);
    assert!(answer(f.sessions.commit(&f.id)).result.unwrap().written);
    assert_eq!(f.disk(), Some(Disk::Unchanged));
    f.stays(TWO_COMPARISONS, unchanged);
}

// ---- When ----

#[test]
fn disk_answers_at_once_while_a_command_waits_on_its_rebuild_and_finds_a_change_meanwhile() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    let hold = f.kernel.hold(is_box);
    let running = f.sessions.set_field(
        &f.id,
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        value("41"),
    );
    // Past the command's own read of the file (so the change below is no reload of
    // its), waiting on the kernel; disk never waits on it.
    let deadline = Instant::now() + WAIT;
    while !f.kernel.calls().iter().any(|call| is_box(&call.op)) {
        assert!(Instant::now() < deadline, "the edit never rebuilt");
        std::thread::sleep(Duration::from_millis(5));
    }
    // A disk that waited on the held rebuild would never return: no clock needed.
    for _ in 0..50 {
        assert_eq!(f.disk(), Some(Disk::Unchanged));
    }
    let theirs = BLOCK.replace(r#""height": 10"#, r#""height": 11"#);
    std::fs::write(&f.path, &theirs).unwrap();
    f.reaches(Disk::Changed);
    hold.release();
    let set = answer(running);
    let at = set.session.unwrap().revision;
    assert_eq!(
        set.result.unwrap_err().errors()[0].code(),
        ErrorCode::Conflict
    );
    // Their change, never written over, and reloaded by the session itself, queued behind
    // the edit as the comparison found it.
    f.reloads_itself(at);
    assert_eq!(f.file(), theirs.as_bytes());
    f.stays(TWO_COMPARISONS, unchanged);
}

#[test]
fn the_disk_of_a_session_closed_is_refused_naming_it_and_a_reopen_watches_afresh() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    answer(f.sessions.close(&f.id)).result.unwrap();
    let refused = f.sessions.disk(&f.id).unwrap_err();
    assert_eq!(
        (refused.code(), refused.parameter()),
        (ErrorCode::InvalidInput, Some("session"))
    );
    assert!(
        refused.message().contains(&f.id.to_string()),
        "{}",
        refused.message()
    );
    assert!(refused.suggestion().is_some());
    // Changed while closed: a new session holds the new bytes.
    std::fs::write(&f.path, BLOCK.replace("Block", "Other")).unwrap();
    let again = answer(f.sessions.open(&f.path, false)).session.unwrap().id;
    assert_ne!(again, f.id);
    let deadline = Instant::now() + WAIT;
    while f.sessions.disk(&again).unwrap() != Some(Disk::Unchanged) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(f.sessions.disk(&f.id).is_err());
}

#[test]
fn many_sessions_watch_their_own_files_each() {
    let dir = tempfile::tempdir().unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = sessions(dir.path(), &kernel);
    let opened: Vec<(SessionId, PathBuf)> = (0..16)
        .map(|n| {
            let path = dir.path().join(format!("p{n}.silico"));
            std::fs::write(&path, BLOCK).unwrap();
            let response = answer(sessions.open(&path, false));
            assert!(response.result.is_ok(), "{n}: {response:?}");
            (response.session.unwrap().id, path)
        })
        .collect();
    let reach = |session: &SessionId, want: Disk| {
        let deadline = Instant::now() + WAIT;
        while sessions.disk(session).unwrap() != Some(want) {
            assert!(Instant::now() < deadline, "{session}: never {want:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    for (session, _) in &opened {
        reach(session, Disk::Unchanged);
        // Inside a transaction, which reloads nothing, so each change stays to be found.
        answer(sessions.begin(session)).result.unwrap();
    }
    // Every other file changed: only theirs read as changed.
    for (_, path) in opened.iter().step_by(2) {
        std::fs::write(path, BLOCK.replace("Block", "Other")).unwrap();
    }
    for (n, (session, _)) in opened.iter().enumerate() {
        if n % 2 == 0 {
            reach(session, Disk::Changed);
        }
    }
    std::thread::sleep(TWO_COMPARISONS);
    for (n, (session, _)) in opened.iter().enumerate() {
        if n % 2 == 1 {
            assert_eq!(
                sessions.disk(session).unwrap(),
                Some(Disk::Unchanged),
                "{session}"
            );
        }
    }
}

// ---- In a workspace ----

#[test]
fn in_a_workspace_a_link_out_to_the_same_bytes_is_unreadable_never_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let sessions = sessions(dir.path(), &kernel).in_workspace(&root).unwrap();
    let path = root.join("a.silico");
    std::fs::write(&path, BLOCK).unwrap();
    let session = answer(sessions.open("a.silico", false)).session.unwrap().id;
    let reach = |want: Disk| {
        let deadline = Instant::now() + WAIT;
        loop {
            let found = sessions.disk(&session).unwrap();
            if found == Some(want) {
                return;
            }
            assert!(Instant::now() < deadline, "never {want:?}: {found:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    reach(Disk::Unchanged);
    // The same bytes outside, linked in its place: what's there is never said.
    let outside = dir.path().join("outside.silico");
    std::fs::write(&outside, BLOCK).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    reach(Disk::Unreadable);
    assert!(answer(sessions.text(&session, None)).result.is_err());
    // Its directory, the workspace's root, swapped for a link out: the same.
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, BLOCK).unwrap();
    reach(Disk::Unchanged);
    let kept = dir.path().join("kept");
    std::fs::rename(&root, &kept).unwrap();
    std::os::unix::fs::symlink(&kept, &root).unwrap();
    reach(Disk::Unreadable);
    std::fs::remove_file(&root).unwrap();
    std::fs::rename(&kept, &root).unwrap();
    reach(Disk::Unchanged);
}

#[test]
fn in_a_workspace_a_link_inside_to_the_same_bytes_agrees_with_the_next_command() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let sessions = sessions(dir.path(), &kernel).in_workspace(&root).unwrap();
    let path = root.join("a.silico");
    std::fs::write(&path, BLOCK).unwrap();
    let session = answer(sessions.open("a.silico", false)).session.unwrap().id;
    let other = root.join("b.silico");
    std::fs::write(&other, BLOCK).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&other, &path).unwrap();
    let deadline = Instant::now() + WAIT;
    let found = loop {
        if let Some(found) = sessions.disk(&session).unwrap() {
            break found;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    std::thread::sleep(TWO_COMPARISONS);
    let found = sessions.disk(&session).unwrap().unwrap_or(found);
    // What the next command does is what disk said: unchanged, no reload and no refusal.
    let text = answer(sessions.text(&session, None));
    match found {
        Disk::Unchanged => {
            assert!(text.result.is_ok(), "{text:?}");
            assert!(!text.session.unwrap().reloaded);
        }
        other => panic!("the same bytes through a link in the workspace read as {other:?}"),
    }
}

#[test]
fn a_commit_cancelled_before_its_save_leaves_the_file_unchanged_never_unknown() {
    let f = opened(BLOCK);
    f.reaches(Disk::Unchanged);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    f.set_length(Revision::OPENED, "41").result.unwrap();
    let hold = f.kernel.hold(is_box);
    let running = f.sessions.commit(&f.id);
    let deadline = Instant::now() + WAIT;
    while !f.kernel.calls().iter().any(|call| is_box(&call.op)) {
        assert!(Instant::now() < deadline, "the commit never rebuilt");
        std::thread::sleep(Duration::from_millis(5));
    }
    let cancelled = f
        .sessions
        .cancel(&f.id, silico_core::command::SessionCommand::Commit);
    assert!(cancelled.result.is_ok(), "{cancelled:?}");
    hold.release();
    let commit = answer(running);
    assert_eq!(
        commit.result.unwrap_err().errors()[0].code(),
        ErrorCode::Cancelled
    );
    assert_eq!(f.file(), BLOCK.as_bytes());
    assert_eq!(f.disk(), Some(Disk::Unchanged));
    f.stays(TWO_COMPARISONS, unchanged);
    // The next commit saves, and it's its own.
    assert!(answer(f.sessions.commit(&f.id)).result.unwrap().written);
    assert_eq!(f.disk(), Some(Disk::Unchanged));
    f.stays(TWO_COMPARISONS, unchanged);
}

#[test]
fn in_a_workspace_whose_root_is_moved_away_the_file_is_gone_as_the_next_command_finds_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let sessions = sessions(dir.path(), &kernel).in_workspace(&root).unwrap();
    std::fs::write(root.join("a.silico"), BLOCK).unwrap();
    let session = answer(sessions.open("a.silico", false)).session.unwrap().id;
    let reach = |want: Disk| {
        let deadline = Instant::now() + WAIT;
        loop {
            let found = sessions.disk(&session).unwrap();
            if found == Some(want) {
                return;
            }
            assert!(Instant::now() < deadline, "never {want:?}: {found:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    reach(Disk::Unchanged);
    let kept = dir.path().join("kept");
    std::fs::rename(&root, &kept).unwrap();
    reach(Disk::Gone);
    let text = answer(sessions.text(&session, None));
    let error = &text.result.as_ref().unwrap_err().errors()[0];
    // The next command finds it gone too: put it back.
    assert!(
        error
            .suggestion()
            .is_some_and(|s| s.starts_with("put the file back")),
        "{error:?}"
    );
    std::fs::rename(&kept, &root).unwrap();
    reach(Disk::Unchanged);
}

/// The latest `Instant` this platform adds to `start` without overflow, from a thousand
/// years down.
fn far_future(start: Instant) -> Instant {
    let mut ahead = Duration::from_secs(1000 * 365 * 24 * 3600);
    loop {
        if let Some(time) = start.checked_add(ahead) {
            return time;
        }
        ahead /= 2;
    }
}

#[test]
fn a_watch_clock_that_goes_back_or_jumps_far_ahead_never_panics_or_sticks() {
    use silico_core::disk::{Clock, DiskWatch, Found, Reader};
    use std::sync::atomic::AtomicUsize;

    // A clock the test sets, and a reader that counts its reads and finds "B" (another's
    // bytes) once told to.
    let start = Instant::now();
    let time = Arc::new(Mutex::new(start));
    let set = |to: Instant| *time.lock().unwrap() = to;
    let clock: Clock = {
        let time = Arc::clone(&time);
        Arc::new(move || *time.lock().unwrap())
    };
    let reads = Arc::new(AtomicUsize::new(0));
    let other = Arc::new(AtomicBool::new(false));
    let read: Reader = {
        let (reads, other) = (Arc::clone(&reads), Arc::clone(&other));
        Arc::new(move |_: &Path| {
            reads.fetch_add(1, Ordering::SeqCst);
            Found::Bytes(if other.load(Ordering::SeqCst) {
                b"B".to_vec()
            } else {
                b"A".to_vec()
            })
        })
    };
    let w = DiskWatch::clocked(
        "p",
        Arc::new(b"A".to_vec()),
        Duration::from_secs(1),
        None,
        read,
        clock,
    );
    let settle = |w: &DiskWatch, reads_before: usize| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let found = w.finding();
            if reads.load(Ordering::SeqCst) > reads_before && found.is_some() {
                // Let the comparison's end land before the next ask.
                std::thread::sleep(Duration::from_millis(20));
                return w.finding();
            }
            assert!(Instant::now() < deadline, "no comparison ended");
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    assert_eq!(settle(&w, 0), Some(Disk::Unchanged));
    let after_first = reads.load(Ordering::SeqCst);

    // Back an hour: no comparison is due (the last ended "after" now), and none panics.
    set(start
        .checked_sub(Duration::from_secs(3600))
        .unwrap_or(start));
    other.store(true, Ordering::SeqCst);
    for _ in 0..50 {
        assert_eq!(w.finding(), Some(Disk::Unchanged));
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(reads.load(Ordering::SeqCst), after_first);
    // The session's own reload still lands at once on the backward clock.
    w.held(Arc::new(b"B".to_vec()));
    assert_eq!(w.finding(), Some(Disk::Unchanged));

    // Far ahead: a comparison is due at once, and finds the file as held.
    other.store(false, Ordering::SeqCst);
    let far = far_future(start);
    set(far);
    let before = reads.load(Ordering::SeqCst);
    assert_eq!(settle(&w, before), Some(Disk::Changed));

    // Back to the start after the far jump: the last comparison ended "in the future", so
    // none is due until the clock passes it; then one is.
    set(start);
    let before = reads.load(Ordering::SeqCst);
    for _ in 0..50 {
        assert_eq!(w.finding(), Some(Disk::Changed));
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(reads.load(Ordering::SeqCst), before);
    set(far.checked_add(Duration::from_secs(1)).unwrap_or(far));
    w.held(Arc::new(b"A".to_vec()));
    assert_eq!(settle(&w, before), Some(Disk::Unchanged));
}

#[test]
fn the_disk_of_a_session_never_opened_is_refused_naming_it() {
    let f = opened(BLOCK);
    let max = format!("s{}", u64::MAX);
    for text in ["s1000", "s0", max.as_str()] {
        let Some(never) = SessionId::parse(text) else {
            continue;
        };
        if never == f.id {
            continue;
        }
        let refused = f.sessions.disk(&never).unwrap_err();
        assert_eq!(
            (refused.code(), refused.parameter()),
            (ErrorCode::InvalidInput, Some("session")),
            "{text}"
        );
        assert!(refused.message().contains(text), "{}", refused.message());
    }
    // The open one is untouched by the asks.
    f.reaches(Disk::Unchanged);
}
