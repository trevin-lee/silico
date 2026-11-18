//! Hostile requests against untitled parts (docs/design/m5.0-app.md, open item 30,
//! overturned by Trevin 2026-11-01): `open` with a null path and odd `create`s; a first save
//! to a directory, a link out of the workspace, a path that can't be written, two sessions
//! racing for one path; a Save As raced by a `close`; the cache after the first save; and
//! more untitled sessions than a process holds. Expected values: the design's §2 words,
//! decision 35's rules for a file a command writes, POSIX (what's on disk after), and the
//! files' bytes as the tests wrote them.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use serde_json::{Value, json};
use silico_core::command::MAX_SESSIONS;

use crate::calls::{Fixture, OUTSIDE, answered, envelope, fixture};
use crate::session_save_as::canonical;

const EMPTY: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 6,\n  \"features\": []\n}\n";

const THE_BOX: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;

/// The envelope of `name` called on `arguments` in `f`.
fn env(f: &Fixture, name: &str, arguments: &str) -> Value {
    envelope(&f.call(name, arguments))
}

/// An untitled session opened in `f`, a transaction begun in it: its ID.
fn untitled_begun(f: &Fixture) -> String {
    let open = env(f, "open", r#"{"path": null, "create": true}"#);
    assert_eq!(open["ok"], true, "{open}");
    let s = open["session"]["id"].as_str().unwrap().to_owned();
    let begin = env(f, "begin", &format!(r#"{{"session": "{s}"}}"#));
    assert_eq!(begin["ok"], true, "{begin}");
    s
}

/// The box added to session `s`'s document at its revision.
fn add_box(f: &Fixture, s: &str) {
    let revision =
        env(f, "status", &format!(r#"{{"session": "{s}"}}"#))["session"]["revision"].clone();
    let added = env(
        f,
        "add_feature",
        &format!(
            r#"{{"session": "{s}", "revision": {revision}, "before": null, "feature": {THE_BOX}}}"#
        ),
    );
    assert_eq!(added["ok"], true, "{added}");
}

/// `commit`'s request for session `s` to `path` (JSON text), with `replace`.
fn commit(s: &str, path: &str, replace: bool) -> String {
    format!(r#"{{"session": "{s}", "path": {path}, "replace": {replace}}}"#)
}

/// The one error of a refused envelope: code, message, parameter.
fn error(answer: &Value) -> (String, String, Value) {
    assert_eq!(answer["ok"], false, "{answer}");
    let e = &answer["errors"][0];
    (
        e["code"].as_str().unwrap().to_owned(),
        e["message"].as_str().unwrap().to_owned(),
        e["parameter"].clone(),
    )
}

/// Session `s` still untitled, its transaction open, and its document the box.
fn still_untitled(f: &Fixture, s: &str) {
    let status = env(f, "status", &format!(r#"{{"session": "{s}"}}"#));
    assert_eq!(status["session"]["path"], Value::Null, "{status}");
    assert_eq!(status["session"]["transaction"], true, "{status}");
    assert_eq!(status["result"]["disk"], Value::Null, "{status}");
    assert_eq!(status["result"]["features"], 1, "{status}");
}

#[test]
fn a_null_path_with_any_create_but_true_is_refused_and_takes_no_id() {
    let f = fixture();
    for (arguments, parameter, message) in [
        (
            r#"{"path": null, "create": null}"#,
            "create",
            "create must be true or false, got null",
        ),
        (
            r#"{"path": null, "create": "true"}"#,
            "create",
            r#"create must be true or false, got "true""#,
        ),
        (
            r#"{"path": null, "create": 1}"#,
            "create",
            "create must be true or false, got 1",
        ),
        (r#"{"path": null}"#, "create", "create is missing"),
        (r#"{"create": true}"#, "path", "path is missing"),
        (
            r#"{"path": null, "create": false}"#,
            "path",
            "path null opens nothing",
        ),
    ] {
        let refused = env(&f, "open", arguments);
        assert_eq!(refused["session"], Value::Null, "{arguments}: {refused}");
        assert_eq!(refused["total"], 1, "{arguments}: {refused}");
        let (code, said, named) = error(&refused);
        assert_eq!(
            (code.as_str(), said.as_str(), named),
            ("invalid_input", message, json!(parameter)),
            "{arguments}"
        );
    }
    // An unknown field beside them is refused too, never ignored into an untitled open.
    let extra = env(
        &f,
        "open",
        r#"{"path": null, "create": true, "untitled": true}"#,
    );
    assert_eq!(error(&extra).2, "untitled");
    // None took an ID, nor made anything.
    let open = env(&f, "open", r#"{"path": null, "create": true}"#);
    assert_eq!(open["session"]["id"], "s1", "{open}");
    assert!(std::fs::read_dir(&f.work).unwrap().next().is_none());
}

#[test]
fn a_first_save_to_what_isnt_a_writable_document_path_is_refused_leaving_it_untitled() {
    let f = fixture();
    let s = untitled_begun(&f);
    add_box(&f, &s);
    std::fs::create_dir(f.work.join("folder.silico")).unwrap();
    for (path, replace, code, parameter) in [
        // A directory there, even with replace.
        (r#""folder.silico""#, true, "invalid_input", "path"),
        // Not a document's name; nothing to name; the workspace itself.
        (r#""part.txt""#, false, "invalid_input", "path"),
        (r#""""#, false, "invalid_input", "path"),
        (r#""""#, true, "invalid_input", "path"),
        (r#"".""#, false, "invalid_input", "path"),
        (r#""part.silico/""#, false, "invalid_input", "path"),
        // In a cache's directory.
        (
            r#"".silico-cache/part.silico""#,
            false,
            "invalid_input",
            "path",
        ),
        // Its directory isn't there: refused before anything's written.
        (r#""nowhere/part.silico""#, false, "invalid_input", "path"),
        // Out of the workspace, absolutely.
        (
            r#""/tmp/silico-untitled-break.silico""#,
            false,
            "invalid_input",
            "path",
        ),
    ] {
        let refused = env(&f, "commit", &commit(&s, path, replace));
        let (got, _, named) = error(&refused);
        assert_eq!(
            (got.as_str(), named),
            (code, json!(parameter)),
            "{path}: {refused}"
        );
        still_untitled(&f, &s);
    }
    assert!(!Path::new("/tmp/silico-untitled-break.silico").exists());
    assert_eq!(
        std::fs::read_dir(&f.work).unwrap().count(),
        1,
        "only the directory the test made"
    );
    // The edits are all still there: it saves where it may.
    let saved = env(&f, "commit", &commit(&s, r#""part.silico""#, false));
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(
        std::fs::read_to_string(f.work.join("part.silico")).unwrap(),
        canonical(40)
    );
}

#[test]
fn a_first_save_through_a_link_out_of_the_workspace_writes_nothing_outside() {
    let f = fixture();
    let s = untitled_begun(&f);
    add_box(&f, &s);
    // A link to the outside file, and a directory link out.
    std::os::unix::fs::symlink(f.outside.join("part.silico"), f.work.join("link.silico")).unwrap();
    std::os::unix::fs::symlink(&f.outside, f.work.join("out")).unwrap();
    for path in [
        r#""link.silico""#,
        r#""out/part.silico""#,
        r#""out/new.silico""#,
    ] {
        for replace in [false, true] {
            let refused = env(&f, "commit", &commit(&s, path, replace));
            let (code, message, named) = error(&refused);
            assert_eq!(
                (code.as_str(), named),
                ("invalid_input", json!("path")),
                "{path}"
            );
            assert!(
                message.contains("outside the workspace"),
                "{path}: {message}"
            );
            still_untitled(&f, &s);
        }
    }
    assert_eq!(f.theirs(), OUTSIDE);
    assert!(!f.outside.join("new.silico").exists());
    assert!(f.work.join("link.silico").is_symlink());
}

#[test]
fn a_first_save_that_cant_be_written_leaves_the_session_untitled_and_unwatched() {
    let f = fixture();
    let s = untitled_begun(&f);
    add_box(&f, &s);
    let locked = f.work.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let refused = env(&f, "commit", &commit(&s, r#""locked/part.silico""#, false));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (code, _, named) = error(&refused);
    assert_eq!(
        (code.as_str(), named),
        ("io_failed", json!("path")),
        "{refused}"
    );
    still_untitled(&f, &s);
    assert!(std::fs::read_dir(&locked).unwrap().next().is_none());
    assert!(
        f.caches.lock().unwrap().is_empty(),
        "no cache opened for a save that failed"
    );
    // Still refused outside a transaction afterwards, as untitled: nothing half-adopted.
    let cleared = env(&f, "cache_clear", &format!(r#"{{"session": "{s}"}}"#));
    assert_eq!(error(&cleared).2, "session");
    // And the path the failed save reserved is free again: it saves there now.
    let saved = env(&f, "commit", &commit(&s, r#""locked/part.silico""#, false));
    assert_eq!(saved["ok"], true, "{saved}");
}

#[test]
fn a_save_as_with_a_close_sent_behind_it_saves_then_closes_and_frees_the_file() {
    let f = fixture();
    let s = untitled_begun(&f);
    add_box(&f, &s);
    // Sent back to back, neither waited for: the close runs after the save, in order.
    let saving = f.calling("commit", &commit(&s, r#""raced.silico""#, false));
    let closing = f.calling("close", &format!(r#"{{"session": "{s}"}}"#));
    let saved = envelope(&answered(saving));
    let closed = envelope(&answered(closing));
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(closed["ok"], true, "{closed}");
    assert_eq!(
        std::fs::read_to_string(f.work.join("raced.silico")).unwrap(),
        canonical(40)
    );
    // Closed, its file is free: another session opens it.
    let open = env(&f, "open", r#"{"path": "raced.silico", "create": false}"#);
    assert_eq!(open["ok"], true, "{open}");
    // A close sent first is refused (the transaction), and the save behind it still runs.
    let t = untitled_begun(&f);
    let closing = f.calling("close", &format!(r#"{{"session": "{t}"}}"#));
    let saving = f.calling("commit", &commit(&t, r#""second.silico""#, false));
    assert_eq!(error(&envelope(&answered(closing))).2, "session");
    assert_eq!(envelope(&answered(saving))["ok"], true);
    assert_eq!(
        std::fs::read_to_string(f.work.join("second.silico")).unwrap(),
        EMPTY
    );
}

#[test]
fn after_the_first_save_cache_clear_clears_the_cache_beside_the_file() {
    let f = fixture();
    let s = untitled_begun(&f);
    add_box(&f, &s);
    assert_eq!(
        env(&f, "commit", &commit(&s, r#""part.silico""#, false))["ok"],
        true
    );
    // An edit outside a transaction, saved, its new step kept in the cache.
    let revision =
        env(&f, "status", &format!(r#"{{"session": "{s}"}}"#))["session"]["revision"].clone();
    let set = env(
        &f,
        "set_field",
        &format!(
            r#"{{"session": "{s}", "revision": {revision}, "feature": "k7f2qx3m", "field": "length", "value": 50}}"#
        ),
    );
    assert_eq!(set["ok"], true, "{set}");
    let cache = f.work.join(".silico-cache");
    let kept = count_entries(&cache);
    assert!(kept > 0);
    let cleared = env(&f, "cache_clear", &format!(r#"{{"session": "{s}"}}"#));
    assert_eq!(
        cleared["result"],
        json!({"directory": ".silico-cache", "removed": kept}),
        "{cleared}"
    );
    assert_eq!(count_entries(&cache), 0);
}

/// The cache's entries at `cache`: 62 hex digits in a directory of two.
fn count_entries(cache: &Path) -> usize {
    let hex =
        |name: &str, len: usize| name.len() == len && name.bytes().all(|b| b.is_ascii_hexdigit());
    std::fs::read_dir(cache)
        .map(|subs| {
            subs.flatten()
                .filter(|sub| hex(&sub.file_name().to_string_lossy(), 2))
                .flat_map(|sub| std::fs::read_dir(sub.path()).unwrap().flatten())
                .filter(|e| hex(&e.file_name().to_string_lossy(), 62))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn two_untitled_sessions_saving_to_one_path_at_once_write_it_once() {
    for replace in [false, true] {
        let f = fixture();
        let (a, b) = (untitled_begun(&f), untitled_begun(&f));
        add_box(&f, &b);
        // Sent at once, neither waited for: exactly one writes, the other is refused.
        let first = f.calling("commit", &commit(&a, r#""one.silico""#, replace));
        let second = f.calling("commit", &commit(&b, r#""one.silico""#, replace));
        let answers = [envelope(&answered(first)), envelope(&answered(second))];
        let won: Vec<usize> = (0..2).filter(|i| answers[*i]["ok"] == true).collect();
        assert_eq!(won.len(), 1, "{answers:?}");
        let (winner, loser) = if won[0] == 0 { (&a, &b) } else { (&b, &a) };
        let refused = &answers[1 - won[0]];
        let (code, message, named) = error(refused);
        assert_eq!(
            (code.as_str(), named),
            ("invalid_input", json!("path")),
            "{refused}"
        );
        // The other session holds it, or a file was put there since this one looked.
        assert!(
            message == format!("\"one.silico\" is open in session {winner}")
                || (!replace && message == "\"one.silico\" exists"),
            "{message}"
        );
        let text = std::fs::read_to_string(f.work.join("one.silico")).unwrap();
        assert_eq!(
            text,
            if winner == &a {
                EMPTY.to_owned()
            } else {
                canonical(40)
            }
        );
        // The loser is still untitled, its transaction open; the winner holds the file.
        let status = env(&f, "status", &format!(r#"{{"session": "{loser}"}}"#));
        assert_eq!(status["session"]["path"], Value::Null);
        assert_eq!(status["session"]["transaction"], true);
        let status = env(&f, "status", &format!(r#"{{"session": "{winner}"}}"#));
        assert_eq!(status["session"]["path"], "one.silico");
    }
}

#[test]
fn a_formerly_untitled_session_saves_as_again_keeping_its_first_cache_and_freeing_its_file() {
    let f = fixture();
    let s = untitled_begun(&f);
    add_box(&f, &s);
    assert_eq!(
        env(&f, "commit", &commit(&s, r#""first.silico""#, false))["ok"],
        true
    );
    std::fs::create_dir(f.work.join("a")).unwrap();
    assert_eq!(
        env(&f, "begin", &format!(r#"{{"session": "{s}"}}"#))["ok"],
        true
    );
    // Its own file now: refused as such.
    let own = env(&f, "commit", &commit(&s, r#""first.silico""#, true));
    assert_eq!(error(&own).1, "\"first.silico\" is this session's own file");
    let again = env(&f, "commit", &commit(&s, r#""a/second.silico""#, false));
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(again["session"]["path"], "a/second.silico");
    // The first file is free; the cache is the first save's, as a file session's Save As
    // keeps the one it opened with.
    let open = env(&f, "open", r#"{"path": "first.silico", "create": false}"#);
    assert_eq!(open["ok"], true, "{open}");
    assert_eq!(
        f.caches.lock().unwrap().clone(),
        [f.work.join(".silico-cache")]
    );
}

#[test]
fn as_many_untitled_sessions_as_a_process_holds_and_no_more() {
    let f = fixture();
    for n in 1..=MAX_SESSIONS {
        let open = env(&f, "open", r#"{"path": null, "create": true}"#);
        assert_eq!(open["session"]["id"], format!("s{n}"), "{open}");
    }
    let past = env(&f, "open", r#"{"path": null, "create": true}"#);
    assert_eq!(
        error(&past).1,
        format!("{MAX_SESSIONS} sessions are open, the most one process holds")
    );
    // One closed, one more opens.
    assert_eq!(env(&f, "close", r#"{"session": "s1"}"#)["ok"], true);
    assert_eq!(
        env(&f, "open", r#"{"path": null, "create": true}"#)["ok"],
        true
    );
    assert!(std::fs::read_dir(&f.work).unwrap().next().is_none());
}

#[test]
fn replace_true_with_no_path_is_refused_as_for_any_session() {
    let f = fixture();
    let s = untitled_begun(&f);
    let refused = env(&f, "commit", &commit(&s, "null", true));
    assert_eq!(
        error(&refused),
        (
            "invalid_input".to_owned(),
            "replace must be false for a commit to the session's own file (path null)".to_owned(),
            json!("replace")
        )
    );
}

#[test]
fn a_path_ending_in_a_slash_names_a_directory_for_a_save_and_an_export_alike() {
    // `part.silico/` and `out.3mf/` name directories, as open(2) reads them: refused, never
    // written as `part.silico` (a session that then held the path with its slash).
    let f = fixture();
    let s = untitled_begun(&f);
    add_box(&f, &s);
    for path in [r#""part.silico/""#, r#""part.silico/.""#] {
        let refused = env(&f, "commit", &commit(&s, path, false));
        let (code, message, named) = error(&refused);
        assert_eq!(
            (code.as_str(), named),
            ("invalid_input", json!("path")),
            "{refused}"
        );
        assert!(message.ends_with("has no file name"), "{message}");
        still_untitled(&f, &s);
    }
    for path in ["out.3mf/", "out.3mf/."] {
        let refused = env(
            &f,
            "export",
            &format!(
                r#"{{"session": "{s}", "format": "3mf", "path": "{path}", "replace": false, "features": null, "tolerance": 0.1}}"#
            ),
        );
        let (code, message, named) = error(&refused);
        assert_eq!(
            (code.as_str(), named),
            ("invalid_input", json!("path")),
            "{refused}"
        );
        assert!(message.ends_with("has no file name"), "{message}");
    }
    assert!(std::fs::read_dir(&f.work).unwrap().next().is_none());
}
