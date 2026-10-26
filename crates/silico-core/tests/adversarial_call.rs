//! Hostile calls by name (`Sessions::call`, docs/design/m4.9-mcp.md, *`Call`*): arguments
//! that aren't an object, that a strict reader refuses (a byte-order mark, a lone surrogate,
//! text after the object), or that are huge; names a command almost has; every command on a
//! session never opened or closed; a call polled with no wait, dropped unwaited, cancelled
//! by its stopper while queued, or raced from several threads; and `status` waited on after
//! the sessions are gone. Expected values: RFC 8259 (what's JSON), the design's words for a
//! session not open, the files' bytes as the tests wrote them, and counts worked out by hand.

use std::time::{Duration, Instant};

use silico_core::command::{Answered, Call, Calling, Command};
use silico_core::kernel::{Op, Primitive};

use crate::calls::{
    BOX, Fixture, REFUSAL_BOUND, answered, calling, envelope, fixture, fixture_with,
};
use crate::sessions::HANG;

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
    // Broke (break-it): the design says commands reach a session in the order the client
    // sent them, and "nothing in a result depends on thread scheduling". An `open` and then
    // a `check` of the session it is (s1: `open`s take IDs in call order), called back to
    // back as MCP's reader dispatches a client's pipelined lines: the check was refused, "no
    // session s1 is open", since the session was only found once the open's thread had
    // recorded it; the same two calls with the open done in between ran the check. The same
    // calls, in the same order, answered by how the threads ran. Now a session is opening
    // from its open's call, and a command naming it waits behind the open.
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

/// The commands sent back to back after `open` in the tests below, each naming s1: queued
/// reads and edits, a transaction, its commit, a close, and one after the close.
fn after_open() -> Vec<(&'static str, String)> {
    let s1 = r#"{"session": "s1"}"#.to_owned();
    vec![
        ("text", r#"{"session": "s1", "feature": null}"#.to_owned()),
        ("outline", s1.clone()),
        (
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#.to_owned(),
        ),
        (
            "elements",
            r#"{"session": "s1", "model": "current", "feature": null, "after": null, "limit": 3}"#
                .to_owned(),
        ),
        (
            "mass_properties",
            r#"{"session": "s1", "model": "current", "feature": null}"#.to_owned(),
        ),
        (
            "set_field",
            r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#
                .to_owned(),
        ),
        ("begin", s1.clone()),
        (
            "add_feature",
            r#"{"session": "s1", "revision": 2, "feature": {"id": "q0000001", "type": "box", "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#
                .to_owned(),
        ),
        ("commit", s1.clone()),
        (
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#.to_owned(),
        ),
        ("close", s1.clone()),
        ("text", r#"{"session": "s1", "feature": null}"#.to_owned()),
    ]
}

/// `open` of `part.silico` (the box), then [`after_open`]'s commands, in a new workspace:
/// each sent at once after the open (`pipelined`), or once the open has answered; and the
/// answers, the open's first, and the file as they left it.
fn open_then_commands(pipelined: bool) -> (Vec<Answered>, String) {
    let f = fixture();
    f.file("part.silico", BOX);
    let open = f.calling("open", r#"{"path": "part.silico", "create": false}"#);
    let open = if pipelined {
        Ok(open)
    } else {
        Err(answered(open))
    };
    let callings: Vec<Calling> = after_open()
        .iter()
        .map(|(name, arguments)| f.calling(name, arguments))
        .collect();
    let mut answers = vec![match open {
        Ok(open) => answered(open),
        Err(open) => open,
    }];
    answers.extend(callings.into_iter().map(answered));
    let file = std::fs::read_to_string(f.work.join("part.silico")).unwrap();
    (answers, file)
}

#[test]
fn commands_sent_while_their_session_opens_answer_as_if_sent_after_it_opened_every_run() {
    // The reference: each command sent once the open has answered. By hand: every command
    // runs but the last, after the close, which names no session open; the set_field and
    // the commit each save, the file ending with the box 41 long and the new box after it.
    let (reference, file) = open_then_commands(false);
    let names: Vec<&str> = std::iter::once("open")
        .chain(after_open().iter().map(|(name, _)| *name))
        .collect();
    for (i, (name, answer)) in names.iter().zip(&reference).enumerate() {
        assert_eq!(answer.ok, i + 1 < names.len(), "{name}: {}", answer.text);
    }
    let last = envelope(reference.last().unwrap());
    assert_eq!(
        last["errors"][0]["message"], "no session s1 is open",
        "{last}"
    );
    assert!(file.contains(r#""length": 41"#), "{file}");
    assert!(file.contains("q0000001"), "{file}");
    // The same calls sent back to back, the open still running: the same answers, byte for
    // byte, and the same file, run after run.
    for run in 0..40 {
        let (answers, after) = open_then_commands(true);
        for ((name, answer), expected) in names.iter().zip(&answers).zip(&reference) {
            assert_eq!(answer, expected, "run {run}, {name}");
        }
        assert_eq!(after, file, "run {run}");
    }
}

#[test]
fn a_published_read_sent_while_its_session_opens_reads_the_session_it_opens() {
    // A published read waits behind no queued command, but behind the open: it reads the
    // session, its model or none published yet, never "no session s1 is open".
    for run in 0..40 {
        let f = fixture();
        f.file("part.silico", BOX);
        let open = f.calling("open", r#"{"path": "part.silico", "create": false}"#);
        let read = f.calling(
            "check",
            r#"{"session": "s1", "model": "published", "fresh": false}"#,
        );
        assert!(answered(open).ok, "run {run}");
        let read = answered(read);
        let envelope = envelope(&read);
        assert_eq!(envelope["session"]["id"], "s1", "run {run}: {envelope}");
        if !read.ok {
            assert_eq!(
                envelope["errors"][0]["message"], "session s1 has published no model yet",
                "run {run}: {envelope}"
            );
        }
    }
}

#[test]
fn typed_commands_sent_while_their_session_opens_run_once_it_has() {
    use silico_core::command::{ModelChoice, SessionId};
    let id = SessionId::parse("s1").unwrap();
    for run in 0..40 {
        let f = fixture();
        f.file("part.silico", BOX);
        let open = f.sessions.open("part.silico", false);
        let text = f.sessions.text(&id, None);
        let check = f.sessions.check(&id, ModelChoice::Current);
        let close = f.sessions.close(&id);
        assert!(open.wait().result.is_ok(), "run {run}");
        assert_eq!(text.wait().result.unwrap().text, BOX, "run {run}");
        let check = check.wait().result.unwrap();
        assert!(check.problems.is_empty(), "run {run}");
        assert!(close.wait().result.is_ok(), "run {run}");
    }
}

/// The answer of a command sent to s1 while it opened, its open not opening it, whatever
/// refused the open: the design's refusal of a session not open (M4.1 section 6's example),
/// as a command sent after the refusal is answered, so which came first changes nothing.
fn assert_unopened(answer: &Answered, at: &str) {
    assert!(!answer.ok, "{at}: {}", answer.text);
    let envelope = envelope(answer);
    assert_eq!(
        envelope["session"],
        serde_json::Value::Null,
        "{at}: {envelope}"
    );
    assert_eq!(envelope["total"], 1, "{at}: {envelope}");
    let e = &envelope["errors"][0];
    assert_eq!(e["code"], "invalid_input", "{at}: {e}");
    assert_eq!(e["message"], "no session s1 is open", "{at}: {e}");
    assert_eq!(e["parameter"], "session", "{at}: {e}");
    assert_eq!(e["cause"], serde_json::Value::Null, "{at}: {e}");
    assert_eq!(
        e["suggestion"],
        "open the document again: a session ends when it's closed or its client goes away",
        "{at}: {e}"
    );
}

#[test]
fn commands_sent_while_an_open_that_is_refused_runs_are_each_refused_the_same_way() {
    // Opens refused on the session's thread, each taking s1: a file not there, a directory,
    // a path out of the workspace, one there that's out of it, and a new document where one
    // is. Every command sent behind it, of every kind (queued, published, close), answers
    // the same refusal, whatever refused the open, run after run, and the same as one sent
    // after the open's answer: whether the open's thread refused it before a command was
    // sent or after, the answers are one.
    let opens = [
        r#"{"path": "missing.silico", "create": false}"#,
        r#"{"path": "folder.silico", "create": false}"#,
        r#"{"path": "../part.silico", "create": false}"#,
        r#"{"path": "../nowhere/part.silico", "create": true}"#,
        r#"{"path": "part.silico", "create": true}"#,
    ];
    let mut answers = Vec::new();
    for run in 0..10 {
        for open in opens {
            let f = fixture();
            f.file("part.silico", BOX);
            std::fs::create_dir(f.work.join("folder.silico")).unwrap();
            std::fs::write(f.work.join("../part.silico"), BOX).unwrap();
            let opening = f.calling("open", open);
            let queued: Vec<(String, Calling)> = requests("s1")
                .into_iter()
                .filter(|(name, _)| !matches!(*name, "status" | "cancel"))
                .map(|(name, arguments)| (name.to_owned(), f.calling(name, &arguments)))
                .collect();
            let opening = answered(opening);
            assert!(!opening.ok, "{open}: {}", opening.text);
            for (name, calling) in queued {
                let answer = answered(calling);
                assert_unopened(&answer, &format!("run {run}, {open}, {name}"));
                answers.push(answer.text);
            }
            // Nothing made, nothing changed, and the next open takes s2.
            assert_eq!(
                std::fs::read_to_string(f.work.join("part.silico")).unwrap(),
                BOX
            );
            assert!(!f.work.join("../nowhere").exists());
            let again = f.call("open", r#"{"path": "part.silico", "create": false}"#);
            assert_eq!(envelope(&again)["session"]["id"], "s2", "{}", again.text);
            for (name, arguments) in requests("s1") {
                if !matches!(name, "status" | "cancel") {
                    let after = f.call(name, &arguments);
                    assert_unopened(&after, &format!("run {run}, {open}, {name} after"));
                    answers.push(after.text);
                }
            }
        }
    }
    // Byte for byte one refusal, of every command, every open, every run.
    answers.sort();
    answers.dedup();
    assert_eq!(answers.len(), 1, "{answers:#?}");
}

/// Sessions in a workspace of their own whose first open is held as it opens its
/// directory's cache, past every check of the file and before it's recorded open: told on
/// the first receiver as it's held, and let through by a word on the sender.
fn held_open() -> (
    Fixture,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    use std::sync::Mutex;
    use std::sync::mpsc;
    let (held_tx, held) = mpsc::channel();
    let (through, through_rx) = mpsc::channel::<()>();
    let first = Mutex::new(Some((held_tx, through_rx)));
    let f = fixture_with(move |_| {
        let hold = first.lock().unwrap().take();
        if let Some((held, through)) = hold {
            held.send(()).unwrap();
            let _ = through.recv();
        }
    });
    (f, held, through)
}

/// The commands sent to s1 while its open is held, in order.
fn sent_while_held() -> [(&'static str, &'static str); 6] {
    [
        ("text", r#"{"session": "s1", "feature": null}"#),
        (
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        ),
        (
            "check",
            r#"{"session": "s1", "model": "published", "fresh": false}"#,
        ),
        ("outline", r#"{"session": "s1"}"#),
        ("close", r#"{"session": "s1"}"#),
        ("text", r#"{"session": "s1", "feature": null}"#),
    ]
}

#[test]
fn commands_sent_while_an_open_is_held_wait_behind_it_and_run_once_it_opens() {
    let (f, held, through) = held_open();
    f.file("part.silico", BOX);
    let open = f.calling("open", r#"{"path": "part.silico", "create": false}"#);
    held.recv_timeout(HANG).unwrap();
    // Certainly opening: none of these can find s1 open.
    let sent: Vec<Calling> = sent_while_held()
        .iter()
        .map(|(name, arguments)| f.calling(name, arguments))
        .collect();
    // None answers while the open is held (the published read too: it waits for the open).
    let sent: Vec<Calling> = sent
        .into_iter()
        .map(
            |calling| match calling.wait_timeout(Duration::from_millis(20)) {
                Ok(answer) => panic!("answered while the open is held: {}", answer.text),
                Err(calling) => calling,
            },
        )
        .collect();
    through.send(()).unwrap();
    assert!(answered(open).ok);
    let answers: Vec<Answered> = sent.into_iter().map(answered).collect();
    // By hand: the text is the file's, the check finds no problem, the published read reads
    // s1 (its model, or none yet), the outline names the box, the close closes it, and the
    // text after it names no session open.
    assert_eq!(
        envelope(&answers[0])["result"]["text"],
        BOX,
        "{}",
        answers[0].text
    );
    assert!(answers[1].ok, "{}", answers[1].text);
    assert_eq!(envelope(&answers[1])["result"]["problems_total"], 0);
    let published = envelope(&answers[2]);
    assert_eq!(published["session"]["id"], "s1", "{published}");
    if !answers[2].ok {
        assert_eq!(
            published["errors"][0]["message"], "session s1 has published no model yet",
            "{published}"
        );
    }
    assert!(answers[3].ok, "{}", answers[3].text);
    assert!(answers[3].text.contains("Block"), "{}", answers[3].text);
    assert!(answers[4].ok, "{}", answers[4].text);
    assert_unopened(&answers[5], "text after the close");
}

#[test]
fn commands_sent_while_an_open_is_held_and_then_cancelled_are_each_told_no_session_is_open() {
    let (f, held, through) = held_open();
    f.file("part.silico", BOX);
    let open = f.calling("open", r#"{"path": "part.silico", "create": false}"#);
    held.recv_timeout(HANG).unwrap();
    let sent: Vec<Calling> = sent_while_held()
        .iter()
        .map(|(name, arguments)| f.calling(name, arguments))
        .collect();
    // Cancelled while held, before it's recorded open: it answers `cancelled`, opening
    // nothing.
    assert!(open.stopper().cancel());
    through.send(()).unwrap();
    let open = envelope(&answered(open));
    assert_eq!(open["errors"][0]["code"], "cancelled", "{open}");
    // Each command sent behind it: the refusal of a session not open, as one sent now is.
    let mut texts = Vec::new();
    for ((name, _), calling) in sent_while_held().iter().zip(sent) {
        let answer = answered(calling);
        assert_unopened(&answer, name);
        texts.push(answer.text);
    }
    for (name, arguments) in sent_while_held() {
        let answer = f.call(name, arguments);
        assert_unopened(&answer, &format!("{name} after"));
        texts.push(answer.text);
    }
    texts.dedup();
    assert_eq!(texts.len(), 1, "{texts:#?}");
    // The file free: it opens, as s2.
    let again = f.call("open", r#"{"path": "part.silico", "create": false}"#);
    assert_eq!(envelope(&again)["session"]["id"], "s2", "{}", again.text);
}

#[test]
fn a_command_queued_behind_an_open_gives_its_progress_and_is_stopped_by_its_stopper() {
    // Found by the review: a command queued behind an open read its progress from the
    // opening session's record of its newest rebuild, which the session takes as its own as
    // it opens; a session that made a record of its own instead passed every test.
    let (f, held, through) = held_open();
    f.file("part.silico", BOX);
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let open = f.calling("open", r#"{"path": "part.silico", "create": false}"#);
    held.recv_timeout(HANG).unwrap();
    // Certainly opening: none of these can find s1 open.
    let check = f.calling(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    let text = f.calling("text", r#"{"session": "s1", "feature": null}"#);
    let read = f.calling(
        "mass_properties",
        r#"{"session": "s1", "model": "published", "feature": null}"#,
    );
    // Stopped from another thread while the open is held, each before it ran.
    for calling in [&text, &read] {
        let stopper = calling.stopper();
        assert!(std::thread::spawn(move || stopper.cancel()).join().unwrap());
    }
    through.send(()).unwrap();
    assert!(answered(open).ok);
    // The check's rebuild, its one step the box's, held running and not done.
    assert!(f.kernel.wait_running(1, HANG));
    let deadline = Instant::now() + HANG;
    let progress = loop {
        if let Some(progress) = check.progress()
            && progress.steps == 1
        {
            break progress;
        }
        assert!(Instant::now() < deadline, "no progress");
        std::thread::yield_now();
    };
    assert_eq!((progress.done, progress.steps), (0, 1));
    assert_eq!(progress.running.as_deref(), Some("k7f2qx3m box"));
    hold.release();
    let check = answered(check);
    assert!(check.ok, "{}", check.text);
    for (what, calling) in [("text", text), ("published read", read)] {
        let answer = envelope(&answered(calling));
        assert_eq!(answer["errors"][0]["code"], "cancelled", "{what}: {answer}");
    }
    // Nothing of them done: the session at revision 1, as opened.
    let status = envelope(&f.call("status", r#"{"session": "s1"}"#));
    assert_eq!(status["session"]["revision"], 1, "{status}");
}

#[test]
fn published_reads_held_behind_an_open_are_as_many_as_an_open_session_runs() {
    // Found by the review: an opening session held every published read sent to it, without
    // bound, each with a kernel client of its own; an open one runs at most
    // MAX_PUBLISHED_READS at once, and refuses one past them.
    use silico_core::command::MAX_PUBLISHED_READS;
    let (f, held, through) = held_open();
    f.file("part.silico", BOX);
    let open = f.calling("open", r#"{"path": "part.silico", "create": false}"#);
    held.recv_timeout(HANG).unwrap();
    let read = r#"{"session": "s1", "model": "published", "fresh": false}"#;
    let reads: Vec<Calling> = (0..MAX_PUBLISHED_READS)
        .map(|_| f.calling("check", read))
        .collect();
    // One past them: refused at once, as an open session refuses it, the open still held.
    let past = match f.calling("check", read).wait_timeout(Duration::ZERO) {
        Ok(answer) => envelope(&answer),
        Err(_) => panic!("held behind the open, past the bound"),
    };
    let e = &past["errors"][0];
    assert_eq!(e["code"], "invalid_input", "{past}");
    assert_eq!(
        e["message"],
        format!(
            "{MAX_PUBLISHED_READS} published reads are running in session s1, the most one session runs at once"
        ),
        "{past}"
    );
    assert_eq!(e["suggestion"], "wait for one to answer, then read again");
    through.send(()).unwrap();
    assert!(answered(open).ok);
    // Those held each read s1, once it opened.
    for calling in reads {
        let answer = envelope(&answered(calling));
        assert_eq!(answer["session"]["id"], "s1", "{answer}");
    }
}

#[test]
fn a_close_sent_while_its_session_opens_closes_it_once_it_has() {
    for run in 0..40 {
        let f = fixture();
        f.file("part.silico", BOX);
        let open = f.calling("open", r#"{"path": "part.silico", "create": false}"#);
        let close = f.calling("close", r#"{"session": "s1"}"#);
        let after = f.calling(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        );
        assert!(answered(open).ok, "run {run}");
        let close = answered(close);
        assert!(close.ok, "run {run}: {}", close.text);
        assert_eq!(envelope(&close)["session"]["id"], "s1", "run {run}");
        // After the close, as any command after one: not open, and no more said.
        let after = envelope(&answered(after));
        assert_eq!(
            after["errors"][0]["message"], "no session s1 is open",
            "{after}"
        );
        assert_eq!(
            after["errors"][0]["cause"],
            serde_json::Value::Null,
            "{after}"
        );
        // The file free again, as the next session.
        let again = f.call("open", r#"{"path": "part.silico", "create": false}"#);
        assert_eq!(envelope(&again)["session"]["id"], "s2", "{}", again.text);
    }
}

#[test]
fn sessions_opening_count_toward_the_sixteen_and_each_runs_what_was_sent_behind_it() {
    // Sixteen opens sent back to back, each with a check behind it: a seventeenth open is
    // refused as too many, though none has opened yet, and each check runs on its session.
    let f = fixture();
    let mut sent = Vec::new();
    for i in 1..=16 {
        f.file(&format!("part{i}.silico"), BOX);
        let open = f.calling(
            "open",
            &format!(r#"{{"path": "part{i}.silico", "create": false}}"#),
        );
        let check = f.calling(
            "check",
            &format!(r#"{{"session": "s{i}", "model": "current", "fresh": false}}"#),
        );
        sent.push((i, open, check));
    }
    f.file("part17.silico", BOX);
    let refused = f.call("open", r#"{"path": "part17.silico", "create": false}"#);
    let e = &envelope(&refused)["errors"][0];
    assert_eq!(
        e["message"], "16 sessions are open, the most one process holds",
        "{e}"
    );
    for (i, open, check) in sent {
        let open = answered(open);
        assert_eq!(
            envelope(&open)["session"]["id"],
            format!("s{i}"),
            "{}",
            open.text
        );
        let check = answered(check);
        assert!(check.ok, "s{i}: {}", check.text);
        assert_eq!(envelope(&check)["session"]["id"], format!("s{i}"));
    }
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
        assert_eq!(
            e["message"], "this process's sessions are closing, so none is open",
            "{e}"
        );
        assert_eq!(e["parameter"], "session", "{e}");
        assert_eq!(
            e["suggestion"], "open the document again once the sessions are made again",
            "{e}"
        );
    }
    assert!(answered(version).ok);
    // The check, queued before the drop, answers (its result, or the session's end), never
    // hangs.
    let _ = answered(check);
}
