//! M4.2's sessions (docs/design/m4.1-command-api.md, section 3): a document held open
//! between commands, each session's commands run in turn on its own thread, a file open in
//! one session a process, and one cache a directory. Expected values are the file's bytes as
//! the test wrote them, the design's texts and messages, and counts by hand.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{Response, Revision, Running, SessionId, Sessions};
use silico_core::document::FeatureId;
use silico_core::fake::FakeKernel;
use tempfile::TempDir;

/// A box document, not in canonical form (keys on one line, `40.0` for 40): what `text`
/// gives must be these bytes, not the canonical form's.
const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40.0, "width": 20, "height": 10}]}"#;

/// [`BOX`] with a length of 0, which the gate refuses.
const FLAT_BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 0, "width": 20, "height": 10}]}"#;

/// An empty document in canonical form, as the design's `text` example gives it.
const EMPTY: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": []\n}\n";

/// Longer than any command here takes on a loaded machine: past it, a command hangs.
const HANG: Duration = Duration::from_secs(60);

/// Sessions on a fake kernel, with documents in a scratch directory and each cache's
/// secret outside it; and the cache directories opened, in order.
struct Fixture {
    dir: TempDir,
    sessions: Sessions<FakeKernel>,
    opened: Arc<Mutex<Vec<PathBuf>>>,
}

fn fixture() -> Fixture {
    fixture_with(|_| {})
}

/// As [`fixture`], `before` run as each cache is opened, on the opening session's thread.
fn fixture_with(before: impl Fn(&Path) + Send + Sync + 'static) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&opened);
    let sessions = Sessions::with_cache_opener(FakeKernel::new(1).unwrap(), move |cache| {
        before(cache);
        seen.lock().unwrap().push(cache.to_owned());
        Cache::with_secret(cache, &secret)
    });
    Fixture {
        dir,
        sessions,
        opened,
    }
}

impl Fixture {
    /// `text` written to `name` in the scratch directory.
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The session `path` opened as.
    fn open(&self, path: &Path) -> SessionId {
        let response = answer(self.sessions.open(path, false));
        assert!(response.result.is_ok(), "{response:?}");
        response.session.unwrap().id
    }
}

/// The command's answer, failing if it hangs.
fn answer<T>(running: Running<T>) -> Response<T> {
    match running.wait_timeout(HANG) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in {HANG:?}"),
    }
}

/// The one error a refused response gives: its code, message, parameter, and suggestion.
fn refusal<T: std::fmt::Debug>(
    response: &Response<T>,
) -> (ErrorCode, String, Option<String>, Option<String>) {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    let error = &problems.errors()[0];
    (
        error.code(),
        error.message().to_owned(),
        error.parameter().map(str::to_owned),
        error.suggestion().map(str::to_owned),
    )
}

fn not_open(id: &str) -> (ErrorCode, String, Option<String>, Option<String>) {
    (
        ErrorCode::InvalidInput,
        format!("no session {id} is open"),
        Some("session".to_owned()),
        Some(
            "open the document again: a session ends when it's closed or its client goes away"
                .to_owned(),
        ),
    )
}

#[test]
fn session_ids_count_opens_and_a_closed_ones_is_never_given_again() {
    let f = fixture();
    let ids: Vec<SessionId> = ["a", "b", "c"]
        .iter()
        .map(|name| f.open(&f.file(&format!("{name}.silico"), BOX)))
        .collect();
    let names: Vec<String> = ids.iter().map(SessionId::to_string).collect();
    assert_eq!(names, ["s1", "s2", "s3"]);
    assert_eq!(answer(f.sessions.close(&ids[1])).result, Ok(()));
    // Two open, s2 closed: the next is s4, never s2 again, nor s3 (one more than those open).
    assert_eq!(f.open(&f.file("d.silico", BOX)).to_string(), "s4");
}

#[test]
fn a_document_opens_at_revision_1_holding_its_path_as_given() {
    let f = fixture();
    let path = f.file("block.silico", BOX);
    let response = answer(f.sessions.open(&path, false));
    let opened = response.result.unwrap();
    assert_eq!(opened.path, path);
    assert!(!opened.created);
    assert_eq!(opened.features, Some(1));
    assert_eq!(opened.bodies, [FeatureId::new("k7f2qx3m").unwrap()]);
    assert!(opened.problems.is_empty());
    let session = response.session.unwrap();
    assert_eq!(session.id.to_string(), "s1");
    assert_eq!(session.path, path);
    assert_eq!(session.revision, Revision::OPENED);
    assert_eq!(session.revision.number(), 1);
    assert!(!session.transaction);
    assert!(!session.reloaded);
    let status = f.sessions.status(&session.id);
    assert_eq!(status.session.as_ref(), Some(&session));
    let status = status.result.unwrap();
    assert_eq!(status.features, Some(1));
    assert!(status.gated);
}

#[test]
fn text_is_the_files_bytes_exactly_and_a_feature_its_canonical_text() {
    let f = fixture();
    let s1 = f.open(&f.file("block.silico", BOX));
    let text = answer(f.sessions.text(&s1, None));
    assert_eq!(text.result.unwrap(), BOX);
    assert_eq!(text.session.unwrap().id, s1);
    // One feature as canonical form writes it, not as the file does: `40`, not `40.0`.
    let feature = answer(f.sessions.text(&s1, FeatureId::new("k7f2qx3m")));
    assert_eq!(
        feature.result.unwrap(),
        r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#
    );
    let missing = answer(f.sessions.text(&s1, FeatureId::new("a91cw0de")));
    let (code, message, parameter, _) = refusal(&missing);
    assert_eq!(code, ErrorCode::InvalidInput);
    assert_eq!(message, "the document has no feature a91cw0de");
    assert_eq!(parameter.as_deref(), Some("feature"));
    // A refusal still says the session's state: the command took one.
    assert_eq!(missing.session.unwrap().id, s1);
}

#[test]
fn the_outline_is_silico_shows() {
    let f = fixture();
    let s1 = f.open(&f.file("block.silico", BOX));
    let outline = answer(f.sessions.outline(&s1));
    // As docs/design/m4.1-command-api.md's `outline` example gives the block's line.
    assert_eq!(
        outline.result.unwrap(),
        "k7f2qx3m \"Block\" box length 40, width 20, height 10, at (0, 0, 0)\nbodies k7f2qx3m\n"
    );
}

#[test]
fn a_document_the_gate_refuses_opens_with_its_problems() {
    let f = fixture();
    let response = answer(f.sessions.open(f.file("flat.silico", FLAT_BOX), false));
    let opened = response.result.unwrap();
    assert_eq!(opened.features, Some(1));
    assert_eq!(opened.bodies, [FeatureId::new("k7f2qx3m").unwrap()]);
    assert_eq!(opened.problems.total(), 1);
    let problem = &opened.problems.errors()[0];
    assert_eq!(problem.code(), ErrorCode::InvalidInput);
    assert_eq!(problem.feature(), Some("k7f2qx3m"));
    assert_eq!(problem.parameter(), Some("length"));
    let s1 = response.session.unwrap().id;
    let status = f.sessions.status(&s1).result.unwrap();
    assert_eq!(status.features, Some(1));
    assert!(!status.gated);
    // It reads as a document, so its text and outline are there to mend it from.
    assert_eq!(answer(f.sessions.text(&s1, None)).result.unwrap(), FLAT_BOX);
    assert!(
        answer(f.sessions.outline(&s1))
            .result
            .unwrap()
            .starts_with("k7f2qx3m \"Block\" box length 0,")
    );
}

#[test]
fn text_that_isnt_a_document_opens_with_the_readers_error_which_outline_gives_too() {
    let f = fixture();
    let response = answer(f.sessions.open(f.file("junk.silico", "not json"), false));
    let opened = response.result.unwrap();
    assert_eq!(opened.features, None);
    assert!(opened.bodies.is_empty());
    assert_eq!(opened.problems.total(), 1);
    assert_eq!(opened.problems.errors()[0].code(), ErrorCode::InvalidInput);
    let s1 = response.session.unwrap().id;
    let status = f.sessions.status(&s1).result.unwrap();
    assert_eq!(status.features, None);
    assert!(!status.gated);
    assert_eq!(
        answer(f.sessions.text(&s1, None)).result.unwrap(),
        "not json"
    );
    let outline = answer(f.sessions.outline(&s1));
    assert_eq!(outline.result.unwrap_err(), opened.problems);
    let feature = answer(f.sessions.text(&s1, FeatureId::new("k7f2qx3m")));
    assert_eq!(feature.result.unwrap_err(), opened.problems);
}

#[test]
fn text_that_isnt_utf8_opens_and_text_refuses_to_give_it() {
    let f = fixture();
    let path = f.dir.path().join("latin1.silico");
    std::fs::write(&path, b"{\"silico\": 1, \xe9}").unwrap();
    let s1 = f.open(&path);
    let text = answer(f.sessions.text(&s1, None));
    let (code, message, _, _) = refusal(&text);
    assert_eq!(code, ErrorCode::InvalidInput);
    // The bad byte is the 15th: `{"silico": 1, ` is 14.
    assert_eq!(
        message,
        "the document's text isn't UTF-8 (at byte 14), so it can't be given as text"
    );
}

#[test]
fn a_file_is_open_in_one_session_by_any_of_its_paths_until_it_closes() {
    let f = fixture();
    let path = f.file("block.silico", BOX);
    let s1 = f.open(&path);
    let link = f.dir.path().join("link.silico");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    std::fs::create_dir(f.dir.path().join("sub")).unwrap();
    let dotted = f.dir.path().join("sub/../block.silico");
    // A hard link is another name of the same file, which no path through links reaches.
    let hard = f.dir.path().join("hard.silico");
    std::fs::hard_link(&path, &hard).unwrap();
    let mut paths = vec![path.clone(), link.clone(), dotted, hard];
    // macOS names a file of the data volume twice (/private/var/... and
    // /System/Volumes/Data/private/var/...), each its own path through links.
    let data = Path::new("/System/Volumes/Data").join(
        std::fs::canonicalize(&path)
            .unwrap()
            .strip_prefix("/")
            .unwrap(),
    );
    if cfg!(target_os = "macos") {
        assert!(data.exists(), "{}", data.display());
        assert_eq!(std::fs::canonicalize(&data).unwrap(), data);
        paths.push(data);
    }
    for again in &paths {
        let response = answer(f.sessions.open(again, false));
        assert_eq!(response.session, None);
        assert_eq!(
            refusal(&response),
            (
                ErrorCode::InvalidInput,
                format!("\"{}\" is already open in session s1", again.display()),
                Some("path".to_owned()),
                Some("use session s1, which this client opened, or close it first".to_owned()),
            )
        );
    }
    assert_eq!(answer(f.sessions.close(&s1)).result, Ok(()));
    // Closed, it opens again, through the link too; each refused open took a number.
    let again = f.open(&link);
    assert_eq!(again.to_string(), format!("s{}", 2 + paths.len()));
}

#[test]
fn a_seventeenth_session_is_refused_until_one_closes() {
    let f = fixture();
    let ids: Vec<SessionId> = (0..16)
        .map(|n| f.open(&f.file(&format!("p{n}.silico"), BOX)))
        .collect();
    let path = f.file("q.silico", BOX);
    let response = answer(f.sessions.open(&path, false));
    assert_eq!(response.session, None);
    assert_eq!(
        refusal(&response),
        (
            ErrorCode::InvalidInput,
            "16 sessions are open, the most one process holds".to_owned(),
            None,
            Some("close a session you're done with, then open this one".to_owned()),
        )
    );
    assert_eq!(answer(f.sessions.close(&ids[3])).result, Ok(()));
    // The refused open took no number: the 16 had s1 to s16.
    assert_eq!(f.open(&path).to_string(), "s17");
}

#[test]
fn a_missing_file_or_a_directory_or_a_pipe_isnt_opened() {
    let f = fixture();
    let missing = answer(f.sessions.open(f.dir.path().join("none.silico"), false));
    assert_eq!(missing.session, None);
    assert_eq!(
        refusal(&missing),
        (
            ErrorCode::IoFailed,
            "couldn't be read: No such file or directory (os error 2)".to_owned(),
            Some("path".to_owned()),
            Some(
                "check the file's name, and that it's a regular file this user may read".to_owned()
            ),
        )
    );
    let directory = answer(f.sessions.open(f.dir.path(), false));
    let (code, message, parameter, _) = refusal(&directory);
    assert_eq!(code, ErrorCode::IoFailed);
    assert_eq!(message, "couldn't be read: it isn't a regular file");
    assert_eq!(parameter.as_deref(), Some("path"));
    let pipe = f.dir.path().join("pipe.silico");
    let made = std::process::Command::new("mkfifo")
        .arg(&pipe)
        .status()
        .unwrap();
    assert!(made.success());
    let piped = answer(f.sessions.open(&pipe, false));
    assert_eq!(
        refusal(&piped).1,
        "couldn't be read: it isn't a regular file"
    );
    // Nothing failed holds its file: each opens once made a document.
    std::fs::write(f.dir.path().join("none.silico"), BOX).unwrap();
    f.open(&f.dir.path().join("none.silico"));
}

#[test]
fn create_writes_an_empty_document_in_canonical_form_and_never_over_a_file() {
    let f = fixture();
    let path = f.dir.path().join("new.silico");
    let response = answer(f.sessions.open(&path, true));
    let opened = response.result.unwrap();
    assert!(opened.created);
    assert_eq!(opened.features, Some(0));
    assert!(opened.bodies.is_empty());
    assert!(opened.problems.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EMPTY);
    let s1 = response.session.unwrap().id;
    assert_eq!(answer(f.sessions.text(&s1, None)).result.unwrap(), EMPTY);
    assert!(f.sessions.status(&s1).result.unwrap().gated);

    let existing = f.file("old.silico", BOX);
    let refused = answer(f.sessions.open(&existing, true));
    assert_eq!(refused.session, None);
    assert_eq!(
        refusal(&refused),
        (
            ErrorCode::InvalidInput,
            format!("\"{}\" already exists", existing.display()),
            Some("create".to_owned()),
            Some("open it without create, or give another path".to_owned()),
        )
    );
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), BOX);
    // Nor through a link to nothing, which would write where it points.
    let dangling = f.dir.path().join("dangling.silico");
    let target = f.dir.path().join("target.silico");
    std::os::unix::fs::symlink(&target, &dangling).unwrap();
    let refused = answer(f.sessions.open(&dangling, true));
    assert_eq!(refusal(&refused).2.as_deref(), Some("create"));
    assert!(!target.exists());
    // Refused, it holds no claim on the file: opened without create, it opens.
    f.open(&existing);

    let nowhere = answer(
        f.sessions
            .open(f.dir.path().join("no/dir/new.silico"), true),
    );
    let (code, _, parameter, _) = refusal(&nowhere);
    assert_eq!(code, ErrorCode::IoFailed);
    assert_eq!(parameter.as_deref(), Some("path"));
    let up = f.dir.path().join("..");
    let nameless = answer(f.sessions.open(&up, true));
    let (code, message, parameter, _) = refusal(&nameless);
    assert_eq!(
        message,
        format!("\"{}\" names no file to make", up.display())
    );
    assert_eq!(code, ErrorCode::InvalidInput);
    assert_eq!(parameter.as_deref(), Some("path"));
}

#[test]
fn sessions_in_one_directory_share_one_cache_opened_once() {
    let f = fixture();
    f.open(&f.file("a.silico", BOX));
    f.open(&f.file("b.silico", BOX));
    f.open(&f.file("other/c.silico", BOX));
    let dir = std::fs::canonicalize(f.dir.path()).unwrap();
    // Two directories, two caches: a and b share the first.
    assert_eq!(
        *f.opened.lock().unwrap(),
        [
            dir.join(".silico-cache"),
            dir.join("other").join(".silico-cache")
        ]
    );
}

#[test]
fn a_command_queued_behind_close_is_told_its_session_closed_and_none_hangs() {
    let f = fixture();
    for n in 0..20 {
        let s = f.open(&f.file("block.silico", BOX));
        // Queued in turn, without waiting: the outline before the close runs, the text
        // after it doesn't, whether it was queued before the close ran or after.
        let outline = f.sessions.outline(&s);
        let close = f.sessions.close(&s);
        let text = f.sessions.text(&s, None);
        assert!(answer(outline).result.is_ok(), "round {n}");
        assert_eq!(answer(close).result, Ok(()), "round {n}");
        let text = answer(text);
        assert_eq!(text.session, None, "round {n}");
        assert_eq!(refusal(&text), not_open(&s.to_string()), "round {n}");
        assert_eq!(refusal(&f.sessions.status(&s)), not_open(&s.to_string()));
        let again = answer(f.sessions.close(&s));
        assert_eq!(refusal(&again), not_open(&s.to_string()));
    }
}

#[test]
fn a_commands_work_is_off_the_callers_thread_and_other_sessions_answer_meanwhile() {
    let (release, released) = mpsc::channel::<()>();
    let released = Mutex::new(released);
    // The cache of the directory `slow` is opened only once released, so its open waits
    // inside, on its session's thread.
    let f = fixture_with(move |cache| {
        if cache.parent().is_some_and(|p| p.ends_with("slow")) {
            released.lock().unwrap().recv().unwrap();
        }
    });
    let s1 = f.open(&f.file("a.silico", BOX));
    let mut slow = f.sessions.open(f.file("slow/b.silico", BOX), false);
    // Returned before its open was done, which is still waiting.
    for _ in 0..5 {
        slow = match slow.try_take() {
            Ok(response) => panic!("answered before the cache was opened: {response:?}"),
            Err(running) => running,
        };
        std::thread::sleep(Duration::from_millis(20));
    }
    // Another session answers meanwhile.
    assert!(f.sessions.status(&s1).result.unwrap().gated);
    assert_eq!(answer(f.sessions.text(&s1, None)).result.unwrap(), BOX);
    release.send(()).unwrap();
    let response = answer(slow);
    assert_eq!(response.session.unwrap().id.to_string(), "s2");
    assert_eq!(response.result.unwrap().features, Some(1));
}

#[test]
fn dropping_the_sessions_closes_them_and_ends_their_threads() {
    let f = fixture();
    let path = f.file("block.silico", BOX);
    let s1 = f.open(&path);
    let text = f.sessions.text(&s1, None);
    let Fixture { dir, sessions, .. } = f;
    drop(sessions);
    // Queued before the drop: answered, run or told the session closed, never left waiting.
    let text = answer(text);
    assert!(
        text.result.is_ok() || refusal(&text) == not_open("s1"),
        "{text:?}"
    );
    drop(dir);
}

#[test]
fn a_file_longer_than_silico_reads_isnt_opened_and_one_at_the_limit_is_held_whole() {
    // 64 MiB, four times the 16 MiB a document may be: what Silico reads of one.
    const READ: u64 = 4 * 16 * 1024 * 1024;
    let f = fixture();
    let long = f.dir.path().join("long.silico");
    std::fs::File::create(&long)
        .unwrap()
        .set_len(READ + 1)
        .unwrap();
    let response = answer(f.sessions.open(&long, false));
    assert_eq!(response.session, None);
    assert_eq!(
        refusal(&response),
        (
            ErrorCode::InvalidInput,
            format!(
                "\"{}\" is more than the 67108864 bytes Silico reads of a document",
                long.display()
            ),
            Some("path".to_owned()),
            Some("check it's the document you mean: no document is this long".to_owned()),
        )
    );
    // At the limit, it opens (the reader refusing it) and its text is all of it.
    let at = f.dir.path().join("at.silico");
    std::fs::File::create(&at).unwrap().set_len(READ).unwrap();
    let s = f.open(&at);
    let text = answer(f.sessions.text(&s, None)).result.unwrap();
    assert_eq!(text.len() as u64, READ);
    assert!(text.bytes().all(|b| b == 0));
}

#[test]
fn an_open_whose_thread_panics_holds_neither_its_file_nor_a_place() {
    let panicked = AtomicBool::new(false);
    // The first cache opened in `boom` panics, as a bug in the open would.
    let f = fixture_with(move |cache| {
        if cache.parent().is_some_and(|p| p.ends_with("boom"))
            && !panicked.swap(true, Ordering::SeqCst)
        {
            panic!("a bug in an open");
        }
    });
    // Made by the open, as `create` makes it, before the panic.
    std::fs::create_dir(f.dir.path().join("boom")).unwrap();
    let path = f.dir.path().join("boom/a.silico");
    let response = answer(f.sessions.open(&path, true));
    assert_eq!(response.session, None);
    assert_eq!(
        refusal(&response),
        (
            ErrorCode::Internal,
            "the session stopped before the command answered".to_owned(),
            None,
            Some("this is a bug in Silico, not in the request: report it".to_owned()),
        )
    );
    // What it made is gone, its file isn't held, and its place isn't taken: it's made and
    // opens again, and 15 more beside it.
    assert!(!path.exists());
    let response = answer(f.sessions.open(&path, true));
    assert!(response.result.unwrap().created);
    for n in 0..15 {
        f.open(&f.file(&format!("p{n}.silico"), BOX));
    }
}

#[test]
fn text_of_a_feature_whose_id_a_refused_document_repeats_is_refused_naming_it() {
    let twice = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "k7f2qx3m", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}]}"#;
    let f = fixture();
    let response = answer(f.sessions.open(f.file("twice.silico", twice), false));
    assert!(!response.result.unwrap().problems.is_empty());
    let s = response.session.unwrap().id;
    let text = answer(f.sessions.text(&s, FeatureId::new("k7f2qx3m")));
    assert_eq!(
        refusal(&text),
        (
            ErrorCode::InvalidInput,
            "2 features in the document have the ID k7f2qx3m".to_owned(),
            Some("feature".to_owned()),
            Some(
                "give each feature an ID of its own: the gate refuses an ID used twice".to_owned()
            ),
        )
    );
}

#[test]
fn a_feature_over_several_lines_is_written_as_the_document_writes_it_not_indented() {
    // A box with its fingerprint (an object inside it), so one member a line; the canonical
    // document holds it four spaces in.
    let feature = "{\n  \"id\": \"k7f2qx3m\",\n  \"type\": \"box\",\n  \"v\": 1,\n  \"name\": \"Block\",\n  \"position\": [0, 0, 0],\n  \"length\": 40,\n  \"width\": 20,\n  \"height\": 10,\n  \"fingerprint\": {\"volume\": 8000, \"area\": 2800, \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n}";
    let document = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 8000, \"area\": 2800, \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n";
    let f = fixture();
    let s = f.open(&f.file("block.silico", document));
    let text = answer(f.sessions.text(&s, FeatureId::new("k7f2qx3m")))
        .result
        .unwrap();
    assert_eq!(text, feature);
    assert!(document.contains(&format!("    {}\n", text.replace('\n', "\n    "))));
    // The document is in canonical form, so `text` of the whole is the same bytes.
    assert_eq!(answer(f.sessions.text(&s, None)).result.unwrap(), document);
}

#[test]
fn a_cache_without_the_users_secret_isnt_kept_so_the_next_open_tries_again() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("secret");
    // Not 32 bytes: the cache keeps nothing.
    let bad = dir.path().join("bad-secret");
    std::fs::write(&bad, "bad").unwrap();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&opened);
    let sessions = Sessions::with_cache_opener(FakeKernel::new(1).unwrap(), move |cache| {
        let unsealed = cache.parent().is_some_and(|p| p.ends_with("unsealed"));
        seen.lock().unwrap().push(unsealed);
        Cache::with_secret(cache, if unsealed { &bad } else { &good })
    });
    for name in ["unsealed/a", "unsealed/b", "sealed/c", "sealed/d"] {
        let path = dir.path().join(format!("{name}.silico"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, BOX).unwrap();
        assert!(answer(sessions.open(&path, false)).result.is_ok());
    }
    // The unsealed directory's opened for each session, the sealed one's once.
    assert_eq!(*opened.lock().unwrap(), [true, true, false]);
}

#[test]
fn a_file_replaced_at_its_path_as_a_save_by_rename_does_is_still_open_in_its_session() {
    // An editor's, git's, or Silico's own save writes a copy beside the file and renames it
    // over: the path's file is then another inode, and still s1's document.
    let f = fixture();
    let path = f.file("block.silico", BOX);
    let s1 = f.open(&path);
    let refused_naming_s1 = |response: &Response<_>| {
        assert_eq!(response.session, None);
        assert_eq!(
            refusal(response),
            (
                ErrorCode::InvalidInput,
                format!("\"{}\" is already open in session s1", path.display()),
                Some("path".to_owned()),
                Some("use session s1, which this client opened, or close it first".to_owned()),
            )
        );
    };
    refused_naming_s1(&answer(f.sessions.open(&path, false)));
    let copy = f.file(".block.silico.new", BOX);
    std::fs::rename(&copy, &path).unwrap();
    refused_naming_s1(&answer(f.sessions.open(&path, false)));
    assert_eq!(answer(f.sessions.close(&s1)).result, Ok(()));
    // Closed, the new file opens, as s4: the two refused opens took s2 and s3.
    assert_eq!(f.open(&path).to_string(), "s4");
}
