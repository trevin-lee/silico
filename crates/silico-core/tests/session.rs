//! M4.2's sessions (docs/design/m4.1-command-api.md, section 3): a document held open
//! between commands, each session's commands run in turn on its own thread, a file open in
//! one session a process, and one cache a directory. Expected values are the file's bytes as
//! the test wrote them, the design's texts and messages, and counts by hand.

use std::path::{Path, PathBuf};
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
    for again in [&path, &link, &dotted] {
        let response = answer(f.sessions.open(again, false));
        assert_eq!(response.session, None);
        assert_eq!(
            refusal(&response),
            (
                ErrorCode::InvalidInput,
                format!("{} is already open in session s1", again.display()),
                Some("path".to_owned()),
                Some("use session s1, which this client opened, or close it first".to_owned()),
            )
        );
    }
    assert_eq!(answer(f.sessions.close(&s1)).result, Ok(()));
    // Closed, it opens again, through the link too.
    let s5 = f.open(&link);
    assert_eq!(s5.to_string(), "s5");
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
            format!("{} already exists", existing.display()),
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
    let nameless = answer(f.sessions.open(f.dir.path().join(".."), true));
    let (code, _, parameter, _) = refusal(&nameless);
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
