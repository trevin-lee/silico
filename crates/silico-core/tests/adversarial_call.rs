//! Hostile calls by name (`Sessions::call`, docs/design/m4.9-mcp.md, *`Call`*): arguments
//! that aren't an object, that a strict reader refuses (a byte-order mark, a lone surrogate,
//! text after the object), or that are huge; names a command almost has; every command on a
//! session never opened or closed; a call polled with no wait, dropped unwaited, cancelled
//! by its stopper while queued, or raced from several threads; and `status` waited on after
//! the sessions are gone. Expected values: RFC 8259 (what's JSON), the design's words for a
//! session not open, the files' bytes as the tests wrote them, and counts worked out by hand.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, Calling, Command, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};
use silico_core::kernel::{Op, Primitive};
use tempfile::TempDir;

use crate::sessions::HANG;

/// A 40 × 20 × 10 box.
const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

/// The most an error's envelope may reasonably be: every value quoted in a message is cut
/// to a few dozen characters (`json::shown`), so a refusal is some hundreds of bytes.
const REFUSAL_BOUND: usize = 16 << 10;

/// Sessions on a fake kernel, drawing with a fake renderer, in a workspace of their own.
struct Fixture {
    _dir: TempDir,
    work: PathBuf,
    kernel: Arc<FakeKernel>,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = Sessions::with_cache_opener(Arc::clone(&kernel), move |cache| {
        Cache::with_secret(cache, &secret)
    })
    .rendering(FakeRenderer::new())
    .in_workspace(&work)
    .unwrap();
    Fixture {
        _dir: dir,
        work,
        kernel,
        sessions,
    }
}

impl Fixture {
    fn file(&self, name: &str, text: &str) {
        std::fs::write(self.work.join(name), text).unwrap();
    }

    fn calling(&self, name: &str, arguments: &str) -> Calling {
        calling(&self.sessions, name, arguments)
    }

    fn call(&self, name: &str, arguments: &str) -> Answered {
        answered(self.calling(name, arguments))
    }

    /// `part.silico` (the box) written and opened as s1.
    fn opened(&self) {
        self.file("part.silico", BOX);
        let open = self.call("open", r#"{"path": "part.silico", "create": false}"#);
        assert!(open.ok, "{}", open.text);
    }
}

fn calling(sessions: &Sessions<Arc<FakeKernel>>, name: &str, arguments: &str) -> Calling {
    match sessions.call(name, arguments) {
        Call::Running(calling) => calling,
        Call::Unknown => panic!("{name} is no command"),
    }
}

fn answered(calling: Calling) -> Answered {
    match calling.wait_timeout(HANG) {
        Ok(answered) => answered,
        Err(_) => panic!("the call didn't answer in {HANG:?}"),
    }
}

fn envelope(answered: &Answered) -> serde_json::Value {
    serde_json::from_str(&answered.text).unwrap()
}

/// A request of every command, sound in shape, naming session `session`.
fn requests(session: &str) -> Vec<(&'static str, String)> {
    let s = session;
    vec![
        ("close", format!(r#"{{"session": "{s}"}}"#)),
        ("status", format!(r#"{{"session": "{s}"}}"#)),
        (
            "cancel",
            format!(r#"{{"session": "{s}", "command": "check"}}"#),
        ),
        ("begin", format!(r#"{{"session": "{s}"}}"#)),
        ("commit", format!(r#"{{"session": "{s}"}}"#)),
        ("rollback", format!(r#"{{"session": "{s}"}}"#)),
        (
            "format",
            format!(r#"{{"session": "{s}", "features": null, "write": false}}"#),
        ),
        (
            "add_feature",
            format!(
                r#"{{"session": "{s}", "revision": 1, "feature": {{"id": "q0000001", "type": "box", "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}}}"#
            ),
        ),
        (
            "set_field",
            format!(
                r#"{{"session": "{s}", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}}"#
            ),
        ),
        (
            "remove_feature",
            format!(r#"{{"session": "{s}", "revision": 1, "feature": "k7f2qx3m"}}"#),
        ),
        (
            "replace_text",
            format!(
                r#"{{"session": "{s}", "revision": 1, "text": {}}}"#,
                serde_json::to_string(BOX).unwrap()
            ),
        ),
        ("text", format!(r#"{{"session": "{s}", "feature": null}}"#)),
        ("outline", format!(r#"{{"session": "{s}"}}"#)),
        (
            "check",
            format!(r#"{{"session": "{s}", "model": "current", "fresh": false}}"#),
        ),
        (
            "elements",
            format!(
                r#"{{"session": "{s}", "model": "current", "feature": null, "after": null, "limit": 3}}"#
            ),
        ),
        (
            "query",
            format!(
                r#"{{"session": "{s}", "model": "current", "feature": null, "kind": "face", "type": null, "stable": null, "facing": null, "measures": [], "order": null, "limit": 4}}"#
            ),
        ),
        (
            "mass_properties",
            format!(r#"{{"session": "{s}", "model": "current", "feature": null}}"#),
        ),
        (
            "distance",
            format!(
                r#"{{"session": "{s}", "model": "current", "elements": ["k7f2qx3m.Face1", "k7f2qx3m.Face2"], "feature": null}}"#
            ),
        ),
        (
            "angle",
            format!(
                r#"{{"session": "{s}", "model": "published", "elements": ["k7f2qx3m.Face1", "k7f2qx3m.Face2"], "feature": null}}"#
            ),
        ),
        (
            "render",
            format!(
                r#"{{"session": "{s}", "model": "current", "view": "front", "camera": null, "width": 40, "height": 30, "highlight": [], "to": null, "replace": false}}"#
            ),
        ),
    ]
}

#[test]
fn arguments_that_arent_one_strict_json_object_are_refused_as_data_by_every_command() {
    let f = fixture();
    f.opened();
    // Whitespace about the object is JSON (RFC 8259, section 2): `version` runs on it.
    let spaced = f.call("version", " \t\r\n{}\n ");
    assert!(spaced.ok, "{}", spaced.text);
    let hostile = [
        "null",
        "3",
        "-0",
        "1e999",
        "\"{}\"",
        "true",
        "[]",
        "[{}]",
        "{} {}",
        "{}x",
        "{}\u{0}",
        "\u{feff}{}",
        "{\"\u{feff}session\": \"s1\"}",
        r#"{"session": "\ud800"}"#,
        r#"{"session": "\udc00\ud800"}"#,
        r#"{"session": "\ud800A"}"#,
        r#"{"session": "s1\u0000"}"#,
        "{\"session\": \"s1\u{0}\"}",
        r#"{"session": "S1"}"#,
        r#"{"session": "s01"}"#,
        r#"{"session": "s1 "}"#,
        r#"{"session": "s0"}"#,
        r#"{"session": "s18446744073709551616"}"#,
        r#"{"session": 1}"#,
        r#"{"session": null}"#,
        r#"{"session": ["s1"]}"#,
        r#"{"": 1}"#,
        r#"{"session": "s1", "session": "s2"}"#,
        "{'session': 's1'}",
        "{\"session\": \"s1\",}",
        "/* */ {}",
        "{\"session\": \"s1\"",
    ];
    for command in Command::ALL {
        for arguments in hostile {
            let answer = f.call(command.as_str(), arguments);
            let at = format!("{command} {arguments:?}");
            assert!(!answer.ok, "{at}: {}", answer.text);
            assert_eq!(answer.png, None, "{at}");
            assert!(!answer.text.contains('\n'), "{at}: not on one line");
            assert!(answer.text.len() < REFUSAL_BOUND, "{at}");
            let envelope = envelope(&answer);
            let errors = envelope["errors"].as_array().unwrap();
            assert!(!errors.is_empty(), "{at}");
            assert_eq!(envelope["total"], errors.len(), "{at}");
            for e in errors {
                assert_eq!(e["code"], "invalid_input", "{at}: {e}");
                // Said as text a terminal shows as it is: no raw control characters.
                let message = e["message"].as_str().unwrap();
                assert!(!message.chars().any(char::is_control), "{at}: {message:?}");
            }
        }
    }
    // Nothing above reached the session: it's as it was opened.
    let status = envelope(&f.call("status", r#"{"session": "s1"}"#));
    assert_eq!(status["session"]["revision"], 1, "{status}");
}

#[test]
fn a_huge_value_is_quoted_cut_in_the_refusal_never_whole() {
    let f = fixture();
    let mb = "x".repeat(1 << 20);
    for (command, arguments) in [
        ("status", format!(r#"{{"session": "{mb}"}}"#)),
        ("schema", format!(r#"{{"of": "{mb}"}}"#)),
        (
            "cancel",
            format!(r#"{{"session": "s1", "command": "{mb}"}}"#),
        ),
        (
            "check",
            format!(r#"{{"session": "s1", "model": "{mb}", "fresh": false}}"#),
        ),
        ("status", format!(r#"{{"session": "s1", "{mb}": 1}}"#)),
    ] {
        let answer = f.call(command, &arguments);
        assert!(!answer.ok);
        assert!(
            answer.text.len() < REFUSAL_BOUND,
            "{command}: a refusal of {} bytes",
            answer.text.len()
        );
    }
}

#[test]
fn a_huge_path_is_quoted_cut_in_opens_refusal_never_whole() {
    // Broke (break-it): `open`'s refusals quoted the path whole (`file::shown_path` cut
    // nothing), so a path of 1 MiB (well under the 64 MiB a request may be) was answered with
    // a refusal of 1 MiB: "outside the workspace" for a path out of it through `..`, and
    // "names no file to make" for one ending in `..`. A path is now shown whole up to 4096
    // characters (Linux's PATH_MAX: every path the system can name), and cut past it.
    let f = fixture();
    let dots = "./".repeat(1 << 19);
    for (why, path, create) in [
        ("outside", format!("../{dots}outside.silico"), false),
        ("outside, made", format!("../{dots}outside.silico"), true),
        (
            "no file to make",
            format!("{}/..", "x".repeat(1 << 20)),
            true,
        ),
    ] {
        let answer = f.call(
            "open",
            &format!(r#"{{"path": "{path}", "create": {create}}}"#),
        );
        assert!(!answer.ok, "{why}");
        assert!(
            answer.text.len() < REFUSAL_BOUND,
            "{why}: a refusal of {} bytes: {}…",
            answer.text.len(),
            &answer.text[..300]
        );
    }
    // 4096 characters, shown whole; 4097, cut at 4096 and marked.
    let at_most = format!("../{}o1.silico", "./".repeat(2042));
    let past = format!("../{}o12.silico", "./".repeat(2042));
    assert_eq!((at_most.len(), past.len()), (4096, 4097));
    let message = |path: &str| {
        let answer = f.call("open", &format!(r#"{{"path": "{path}", "create": false}}"#));
        envelope(&answer)["errors"][0]["message"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        message(&at_most),
        format!("\"{at_most}\" is outside the workspace this server was started in")
    );
    assert_eq!(
        message(&past),
        format!(
            "\"{}\"… is outside the workspace this server was started in",
            &past[..4096]
        )
    );
}

#[test]
fn names_a_command_almost_has_are_unknown() {
    let f = fixture();
    let long = "status".repeat(200_000);
    let mut names: Vec<String> = vec![
        long,
        "status\u{0}".to_owned(),
        "\u{0}status".to_owned(),
        "stat\u{0}us".to_owned(),
        "status\n".to_owned(),
        "status\u{200b}".to_owned(),
        "\u{feff}status".to_owned(),
        // Cyrillic `ѕ`, and fullwidth letters.
        "\u{455}tatus".to_owned(),
        "ｓｔａｔｕｓ".to_owned(),
        "add-feature".to_owned(),
        "addFeature".to_owned(),
        "add_feature ".to_owned(),
        "massproperties".to_owned(),
    ];
    for command in Command::ALL {
        names.push(command.as_str().to_uppercase());
        names.push(format!("{command} "));
        names.push(format!("{command}\u{0}"));
        names.push(format!("silico.{command}"));
    }
    for name in &names {
        assert!(
            matches!(f.sessions.call(name, "{}"), Call::Unknown),
            "{:?}",
            &name[..name.len().min(40)]
        );
        assert_eq!(Command::named(name), None);
    }
}

#[test]
fn every_command_on_a_session_never_opened_or_closed_is_refused_naming_it() {
    let f = fixture();
    let not_open = |answer: &Answered, id: &str, at: &str| {
        assert!(!answer.ok, "{at}: {}", answer.text);
        let envelope = envelope(answer);
        assert_eq!(envelope["total"], 1, "{at}: {envelope}");
        let e = &envelope["errors"][0];
        assert_eq!(e["code"], "invalid_input", "{at}: {e}");
        assert_eq!(e["message"], format!("no session {id} is open"), "{at}");
        assert_eq!(e["parameter"], "session", "{at}");
        assert_eq!(envelope["session"], serde_json::Value::Null, "{at}");
    };
    for (command, arguments) in requests("s7") {
        not_open(&f.call(command, &arguments), "s7", command);
    }
    // And once s1 is open and closed: every command after its close, as if it never was.
    f.opened();
    let close = f.call("close", r#"{"session": "s1"}"#);
    assert!(close.ok, "{}", close.text);
    for (command, arguments) in requests("s1") {
        not_open(&f.call(command, &arguments), "s1", command);
    }
    // The file is free again, and opens as the next session.
    let again = envelope(&f.call("open", r#"{"path": "part.silico", "create": false}"#));
    assert_eq!(again["session"]["id"], "s2", "{again}");
}

#[test]
fn a_command_called_right_after_the_open_of_its_session_answers_as_one_called_later() {
    // BREAKS (with a spec question): the design says commands reach a session in the order
    // the client sent them, and "nothing in a result depends on thread scheduling". An
    // `open` and then a `check` of the session it is (s1: `open`s take IDs in call order),
    // called back to back as MCP's reader dispatches a client's pipelined lines: the check
    // is refused, "no session s1 is open", since the session is only found once the open's
    // thread has recorded it; the same two calls with the open done in between run the
    // check. The same calls, in the same order, answered by how the threads ran.
    let pipelined = {
        let f = fixture();
        f.file("part.silico", BOX);
        let open = f.calling("open", r#"{"path": "part.silico", "create": false}"#);
        let check = f.calling(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        );
        let (open, check) = (answered(open), answered(check));
        assert!(open.ok, "{}", open.text);
        check
    };
    let after = {
        let f = fixture();
        f.file("part.silico", BOX);
        let open = f.calling("open", r#"{"path": "part.silico", "create": false}"#);
        // The open's thread done before the check is called; its answer still unread.
        std::thread::sleep(Duration::from_millis(200));
        let check = f.calling(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        );
        let (open, check) = (answered(open), answered(check));
        assert!(open.ok, "{}", open.text);
        check
    };
    assert!(after.ok, "{}", after.text);
    assert_eq!(pipelined.ok, after.ok, "{}", pipelined.text);
}

#[test]
fn a_call_polled_with_no_wait_answers_once_and_as_waiting_would() {
    let f = fixture();
    f.opened();
    // Held on the kernel: thousands of polls of no wait each give the call back.
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let mut check = f.calling(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    assert!(f.kernel.wait_running(1, HANG));
    for _ in 0..10_000 {
        check = match check.wait_timeout(Duration::ZERO) {
            Err(check) => check,
            Ok(answered) => panic!("answered while held: {}", answered.text),
        };
    }
    hold.release();
    let deadline = Instant::now() + HANG;
    let answer = loop {
        match check.wait_timeout(Duration::ZERO) {
            Ok(answer) => break answer,
            Err(again) => check = again,
        }
        assert!(Instant::now() < deadline, "never answered");
        std::thread::yield_now();
    };
    assert!(answer.ok, "{}", answer.text);
    // The same check, waited on: the same answer.
    let waited = f.call(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    assert_eq!(answer, waited);
    // Answered later ones, and refused ones, answer on the first poll of no wait.
    for (name, arguments) in [
        ("status", r#"{"session": "s1"}"#),
        ("version", "{}"),
        ("schema", r#"{"of": "document"}"#),
        ("cancel", r#"{"session": "s1", "command": "check"}"#),
        ("check", "{}"),
        ("check", "nonsense"),
    ] {
        assert!(
            f.calling(name, arguments)
                .wait_timeout(Duration::ZERO)
                .is_ok(),
            "{name} {arguments}"
        );
    }
}

#[test]
fn a_call_dropped_unwaited_still_runs_and_the_session_after_it_runs_on() {
    let f = fixture();
    f.opened();
    // An edit and an open, dropped as they're called: each still runs (the command runs on,
    // its answer unread), and nothing waits on them.
    drop(f.calling(
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
    ));
    f.file("other.silico", BOX);
    drop(f.calling("open", r#"{"path": "other.silico", "create": false}"#));
    // Queued after the edit: it reads the edit's revision.
    let check = envelope(&f.call(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    ));
    assert_eq!(check["session"]["revision"], 2, "{check}");
    assert!(
        std::fs::read_to_string(f.work.join("part.silico"))
            .unwrap()
            .contains("\"length\": 41,")
    );
    // The open dropped took s2, which opens.
    let deadline = Instant::now() + HANG;
    loop {
        let status = f.call("status", r#"{"session": "s2"}"#);
        if status.ok {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "s2 never opened: {}",
            status.text
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // And a call dropped while it's held on the kernel: the sessions drop with it unwaited,
    // and nothing hangs.
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    drop(f.calling(
        "set_field",
        r#"{"session": "s1", "revision": 2, "feature": "k7f2qx3m", "field": "length", "value": 42}"#,
    ));
    let check = f.calling(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    assert!(f.kernel.wait_running(1, HANG));
    drop(check);
    let Fixture {
        _dir,
        sessions,
        kernel,
        ..
    } = f;
    let (done, dropped) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(sessions);
        let _ = done.send(());
    });
    // The held op released after the sessions began to drop: they don't wait on it forever.
    std::thread::sleep(Duration::from_millis(50));
    hold.release();
    assert!(
        dropped.recv_timeout(HANG).is_ok(),
        "the sessions' drop hung"
    );
    drop(kernel);
}

#[test]
fn a_queued_call_cancelled_by_its_stopper_answers_cancelled_and_leaves_no_trace() {
    let f = fixture();
    f.opened();
    let before = std::fs::read(f.work.join("part.silico")).unwrap();
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let check = f.calling(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    assert!(f.kernel.wait_running(1, HANG));
    // Queued behind the held check: stopped before it ran, from another thread.
    let edit = f.calling(
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
    );
    let stopper = edit.stopper();
    let stopped = std::thread::spawn(move || stopper.cancel()).join().unwrap();
    assert!(stopped);
    // Cancelled twice: the second stops nothing more.
    assert!(!edit.stopper().cancel() || edit.stopper().cancel());
    hold.release();
    let edit = envelope(&answered(edit));
    assert_eq!(edit["errors"][0]["code"], "cancelled", "{edit}");
    assert!(answered(check).ok);
    assert_eq!(std::fs::read(f.work.join("part.silico")).unwrap(), before);
    let status = envelope(&f.call("status", r#"{"session": "s1"}"#));
    assert_eq!(status["session"]["revision"], 1, "{status}");
    // A stopper used after its call answered stops nothing.
    let done = f.calling("text", r#"{"session": "s1", "feature": null}"#);
    let late = done.stopper();
    assert!(answered(done).ok);
    assert!(!late.cancel());
}

#[test]
fn edits_of_one_revision_raced_from_many_threads_take_exactly_one() {
    let f = fixture();
    f.opened();
    let threads = 8;
    let answers: Vec<Answered> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let sessions = &f.sessions;
                scope.spawn(move || {
                    // Status and cancel, raced alongside, never stop the edits.
                    let status = calling(sessions, "status", r#"{"session": "s1"}"#);
                    let edit = calling(
                        sessions,
                        "set_field",
                        &format!(
                            r#"{{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": {}}}"#,
                            50 + i
                        ),
                    );
                    let cancel = calling(
                        sessions,
                        "cancel",
                        r#"{"session": "s1", "command": "outline"}"#,
                    );
                    assert!(answered(status).ok);
                    let _ = answered(cancel);
                    answered(edit)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let taken: Vec<&Answered> = answers.iter().filter(|a| a.ok).collect();
    assert_eq!(taken.len(), 1, "{answers:#?}");
    for refused in answers.iter().filter(|a| !a.ok) {
        let envelope = envelope(refused);
        assert_eq!(envelope["errors"][0]["parameter"], "revision", "{envelope}");
    }
    // The file holds the one edit taken, at revision 2.
    let length = envelope(taken[0])["result"]["after"].as_i64().unwrap();
    assert!(
        std::fs::read_to_string(f.work.join("part.silico"))
            .unwrap()
            .contains(&format!("\"length\": {length},"))
    );
    let status = envelope(&f.call("status", r#"{"session": "s1"}"#));
    assert_eq!(status["session"]["revision"], 2, "{status}");
}

#[test]
fn opens_of_one_file_raced_from_many_threads_open_it_once() {
    let f = fixture();
    f.file("part.silico", BOX);
    // Spelled differently by each thread, all the same file.
    let spellings = [
        "part.silico",
        "./part.silico",
        "../work/part.silico",
        ".//part.silico",
    ];
    let work = f.work.clone();
    let answers: Vec<Answered> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let sessions = &f.sessions;
                let path = if i == 15 {
                    work.join("part.silico").display().to_string()
                } else {
                    spellings[i % spellings.len()].to_owned()
                };
                scope.spawn(move || {
                    answered(calling(
                        sessions,
                        "open",
                        &format!(r#"{{"path": "{path}", "create": false}}"#),
                    ))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let opened: Vec<&Answered> = answers.iter().filter(|a| a.ok).collect();
    assert_eq!(opened.len(), 1, "{answers:#?}");
    for refused in answers.iter().filter(|a| !a.ok) {
        let envelope = envelope(refused);
        assert_eq!(envelope["errors"][0]["parameter"], "path", "{envelope}");
    }
}

#[test]
fn status_and_cancel_waited_on_after_the_sessions_are_dropped_are_refused_as_closing() {
    let f = fixture();
    f.opened();
    let status = f.calling("status", r#"{"session": "s1"}"#);
    let cancel = f.calling("cancel", r#"{"session": "s1", "command": "check"}"#);
    let version = f.calling("version", "{}");
    let check = f.calling(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    let Fixture { _dir, sessions, .. } = f;
    drop(sessions);
    for calling in [status, cancel] {
        let answer = answered(calling);
        assert!(!answer.ok, "{}", answer.text);
        let e = &envelope(&answer)["errors"][0];
        assert_eq!(e["code"], "invalid_input", "{e}");
        assert!(e["message"].as_str().unwrap().contains("closing"), "{e}");
    }
    assert!(answered(version).ok);
    // The check, queued before the drop, answers (its result, or the session's end), never
    // hangs.
    let _ = answered(check);
}
