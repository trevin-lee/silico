//! Untitled parts (docs/design/m5.0-app.md, open item 30, overturned by Trevin 2026-11-01;
//! M4.1's as-built notes on `open`, `commit`, `status`, `format`, and `cache_clear`): `open`
//! with `path` null and `create` true holds the empty document and no file, and nothing is
//! written until a `commit` gives it a path, its first save, after which it's as a session
//! opened on that file. Expected values are the design's words for each refusal, the empty
//! document's canonical bytes (tests/session.rs's `EMPTY`, M2.1's layout), the box's
//! canonical text with its fingerprint by formula (session_save_as.rs's `canonical`: 40 ×
//! 20 × 10, volume 8000, area 2800), directory listings made by hand, and, where the
//! untitled session should answer as any session does, a file session's answer to the same
//! requests.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Value, json};
use silico_protocol::Reply;

use silico_core::kernel::Op;

use crate::calls::{BOX, Fixture, OUTSIDE, call, envelope, fixture, sessions_on};
use crate::example::EXAMPLE;
use crate::export_report::tetra;
use crate::session_save_as::canonical;
use crate::sessions::HANG;

/// The empty document in canonical form: what `open` with `create` writes, and what an
/// untitled session holds (tests/session.rs's `EMPTY`).
const EMPTY: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": []\n}\n";

/// `open`'s request for an untitled document.
const UNTITLED: &str = r#"{"path": null, "create": true}"#;

/// The words of every refusal of a save an untitled session can't make outside a
/// transaction (an edit, `format` writing): the design's §2.
const NO_FILE: &str = "it has no file to save to";
const SAVED_AS_MADE: &str = "outside a transaction an edit is saved as it's made";
const BEGIN_FIRST: &str =
    "begin a transaction first; its commit, given a path, saves every edit it holds (Save As)";

/// The 40 × 20 × 10 box, as `add_feature` takes it.
const THE_BOX: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;

impl Fixture {
    /// The envelope of the command `name` called on `arguments`.
    fn env(&self, name: &str, arguments: &str) -> Value {
        envelope(&self.call(name, arguments))
    }

    /// An untitled session opened: its ID.
    fn untitled(&self) -> String {
        let open = self.env("open", UNTITLED);
        assert_eq!(open["ok"], true, "{open}");
        open["session"]["id"].as_str().unwrap().to_owned()
    }

    /// The session's document's text.
    fn text_of(&self, s: &str) -> String {
        let text = self.env("text", &format!(r#"{{"session": "{s}", "feature": null}}"#));
        assert_eq!(text["ok"], true, "{text}");
        text["result"]["text"].as_str().unwrap().to_owned()
    }

    /// `status` of the session.
    fn status(&self, s: &str) -> Value {
        self.env("status", &format!(r#"{{"session": "{s}"}}"#))
    }

    /// The session's revision, as `status` gives it.
    fn revision(&self, s: &str) -> u64 {
        self.status(s)["session"]["revision"].as_u64().unwrap()
    }

    /// `begin` in the session, which must run.
    fn begun(&self, s: &str) {
        let begin = self.env("begin", &format!(r#"{{"session": "{s}"}}"#));
        assert_eq!(begin["ok"], true, "{begin}");
    }

    /// The box added to the session's document, at its revision.
    fn add_box(&self, s: &str) -> Value {
        let revision = self.revision(s);
        self.env(
            "add_feature",
            &format!(
                r#"{{"session": "{s}", "revision": {revision}, "before": null, "feature": {THE_BOX}}}"#
            ),
        )
    }

    /// `commit` of the session to `path` (a JSON value: a string or null), with `replace`.
    fn commit_to(&self, s: &str, path: &str, replace: bool) -> Value {
        self.env(
            "commit",
            &format!(r#"{{"session": "{s}", "path": {path}, "replace": {replace}}}"#),
        )
    }

    /// The file `name` in the workspace, as text.
    fn on_disk(&self, name: &str) -> String {
        std::fs::read_to_string(self.work.join(name)).unwrap()
    }

    /// A file session on `text`, written at `name` and opened: its ID.
    fn file_session(&self, name: &str, text: &str) -> String {
        self.file(name, text);
        let open = self.env("open", &format!(r#"{{"path": "{name}", "create": false}}"#));
        assert_eq!(open["ok"], true, "{open}");
        open["session"]["id"].as_str().unwrap().to_owned()
    }

    /// The cache directories the sessions have opened, in order.
    fn caches_opened(&self) -> Vec<PathBuf> {
        self.caches.lock().unwrap().clone()
    }
}

/// Every file and directory under `dir`, by its path from `dir`, sorted.
fn listing(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, at: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            found.push(path.strip_prefix(root).unwrap().display().to_string());
            if path.is_dir() && !path.is_symlink() {
                walk(root, &path, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    found.sort();
    found
}

/// The entries of the rebuild cache at `cache`: files named by 62 hex digits in a
/// subdirectory named by two, as the cache writes them (M4.1's `cache_clear`, as built).
fn entries(cache: &Path) -> usize {
    let hex =
        |name: &str, len: usize| name.len() == len && name.bytes().all(|b| b.is_ascii_hexdigit());
    let Ok(subdirectories) = std::fs::read_dir(cache) else {
        return 0;
    };
    subdirectories
        .flatten()
        .filter(|sub| hex(&sub.file_name().to_string_lossy(), 2))
        .flat_map(|sub| std::fs::read_dir(sub.path()).unwrap().flatten())
        .filter(|entry| hex(&entry.file_name().to_string_lossy(), 62))
        .count()
}

/// The one error of a refused envelope, whole.
fn the_error(answer: &Value) -> &Value {
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["total"], 1, "{answer}");
    &answer["errors"][0]
}

/// `error` is `invalid_input` with these words, every other field as a plain refusal's.
fn refused_as(
    error: &Value,
    message: &str,
    parameter: &str,
    cause: Option<&str>,
    suggestion: &str,
) {
    assert_eq!(
        error,
        &json!({
            "code": "invalid_input",
            "message": message,
            "feature": null,
            "elements": [],
            "elements_total": 0,
            "parameter": parameter,
            "cause": cause,
            "suggestion": suggestion,
            "candidates": [],
            "advice": null
        })
    );
}

// ---- 1, 2: opening ----

#[test]
fn an_untitled_open_answers_as_the_design_gives_it_holding_the_empty_document() {
    let f = fixture();
    let open = f.env("open", UNTITLED);
    assert_eq!(
        open,
        json!({
            "ok": true,
            "session": {"id": "s1", "path": null, "revision": 1, "transaction": false, "reloaded": false},
            "model": null,
            "result": {"path": null, "created": true, "features": 0, "bodies": [], "problems": [], "problems_total": 0}
        })
    );
    // Its text, byte for byte the empty document, which `create` writes to a file.
    assert_eq!(f.text_of("s1"), EMPTY);
    let made = f.env("open", r#"{"path": "made.silico", "create": true}"#);
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(f.on_disk("made.silico"), EMPTY);
    assert_eq!(f.text_of("s1"), f.on_disk("made.silico"));
    // `status` as any session's, its disk `null`: no file to compare.
    let status = f.status("s1");
    assert_eq!(status["session"]["path"], Value::Null, "{status}");
    assert_eq!(status["result"]["features"], 0);
    assert_eq!(status["result"]["gated"], true);
    assert_eq!(status["result"]["transaction"], Value::Null);
    assert_eq!(status["result"]["disk"], Value::Null);
    // It claims nothing and opened no cache: only `made.silico`'s directory's was opened.
    assert_eq!(f.caches_opened(), [f.work.join(".silico-cache")]);
    let reopened = f.env("open", r#"{"path": "made.silico", "create": false}"#);
    assert_eq!(
        the_error(&reopened)["message"],
        "\"made.silico\" is already open in session s2"
    );
}

#[test]
fn a_null_path_without_create_is_refused_and_opens_nothing() {
    let f = fixture();
    let refused = f.env("open", r#"{"path": null, "create": false}"#);
    assert_eq!(refused["session"], Value::Null, "{refused}");
    refused_as(
        the_error(&refused),
        "path null opens nothing",
        "path",
        None,
        "give a path to open, or create: true for an untitled document",
    );
    // No session took an ID: the next open is s1. Nothing made.
    assert_eq!(f.untitled(), "s1");
    assert!(listing(&f.work).is_empty(), "{:?}", listing(&f.work));
}

// ---- 3, 4: what would write the file ----

#[test]
fn an_edit_outside_a_transaction_is_refused_the_session_as_it_was() {
    let f = fixture();
    let s = f.untitled();
    let added = f.add_box(&s);
    refused_as(
        the_error(&added),
        &format!("session {s} is untitled: {NO_FILE}"),
        "session",
        Some(SAVED_AS_MADE),
        BEGIN_FIRST,
    );
    assert_eq!(added["session"]["revision"], 1, "{added}");
    assert_eq!(added["session"]["transaction"], false, "{added}");
    assert_eq!(added["model"], Value::Null, "{added}");
    // Every edit alike: `replace_text`, with the box's whole document.
    let replaced = f.env(
        "replace_text",
        &format!(
            r#"{{"session": "{s}", "revision": 1, "text": {}}}"#,
            serde_json::to_string(BOX).unwrap()
        ),
    );
    assert_eq!(
        the_error(&replaced)["message"],
        format!("session {s} is untitled: {NO_FILE}")
    );
    assert_eq!(f.revision(&s), 1);
    assert_eq!(f.text_of(&s), EMPTY);
    assert!(listing(&f.work).is_empty(), "{:?}", listing(&f.work));
    // An edit that changes nothing is no commit: it answers as anywhere.
    let same = f.env(
        "replace_text",
        &format!(
            r#"{{"session": "{s}", "revision": 1, "text": {}}}"#,
            serde_json::to_string(EMPTY).unwrap()
        ),
    );
    assert_eq!(same["ok"], true, "{same}");
    assert_eq!(same["result"]["changed"], false, "{same}");
    assert_eq!(f.revision(&s), 1);
}

#[test]
fn a_commit_with_no_path_is_refused_naming_path_the_transaction_left_open() {
    let f = fixture();
    let s = f.untitled();
    f.begun(&s);
    let added = f.add_box(&s);
    assert_eq!(added["ok"], true, "{added}");
    let held = f.text_of(&s);
    let revision = f.revision(&s);
    let refused = f.commit_to(&s, "null", false);
    refused_as(
        the_error(&refused),
        &format!("session {s} is untitled: {NO_FILE}"),
        "path",
        None,
        "give path: where to save it (Save As)",
    );
    let status = f.status(&s);
    assert_eq!(status["session"]["transaction"], true, "{status}");
    assert_eq!(status["session"]["revision"], revision);
    assert_eq!(status["session"]["path"], Value::Null);
    assert_eq!(f.text_of(&s), held);
    assert!(listing(&f.work).is_empty(), "{:?}", listing(&f.work));
}

// ---- 5, 6: the first save ----

#[test]
fn the_first_save_writes_the_file_and_the_session_is_then_as_one_opened_on_it() {
    let f = fixture();
    let s = f.untitled();
    f.begun(&s);
    assert_eq!(f.add_box(&s)["ok"], true);
    let saved = f.commit_to(&s, r#""part.silico""#, false);
    assert_eq!(
        saved,
        json!({
            "ok": true,
            "session": {"id": s, "path": "part.silico", "revision": 3, "transaction": false, "reloaded": false},
            "model": {"revision": 2, "of": "current", "fault": null},
            "result": {"written": true, "path": "part.silico", "refingerprinted": ["k7f2qx3m"], "resolved": [], "problems": [], "problems_total": 0}
        })
    );
    assert_eq!(f.on_disk("part.silico"), canonical(40));
    // The cache beside it opened at the save, and nothing kept in it from before.
    let cache = f.work.join(".silico-cache");
    assert_eq!(f.caches_opened(), std::slice::from_ref(&cache));
    assert_eq!(entries(&cache), 0);
    // The session claims the file, as `open` would have.
    let again = f.env("open", r#"{"path": "part.silico", "create": false}"#);
    assert_eq!(
        the_error(&again)["message"],
        format!("\"part.silico\" is already open in session {s}")
    );
    // An edit outside a transaction now saves the file as it's made; its rebuild's new step
    // kept in the cache beside it.
    let revision = f.revision(&s);
    let set = f.env(
        "set_field",
        &format!(
            r#"{{"session": "{s}", "revision": {revision}, "feature": "k7f2qx3m", "field": "length", "value": 50}}"#
        ),
    );
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(f.on_disk("part.silico"), canonical(50));
    assert!(entries(&cache) > 0, "{:?}", listing(&f.work));
    // A hand edit of the file is reloaded by the next command, as any session's is.
    std::fs::write(f.work.join("part.silico"), OUTSIDE).unwrap();
    let text = f.env("text", &format!(r#"{{"session": "{s}", "feature": null}}"#));
    assert_eq!(text["session"]["reloaded"], true, "{text}");
    assert_eq!(text["result"]["text"], OUTSIDE);
    // And its file is watched on disk: `status.disk` comes to say it's unchanged.
    let start = Instant::now();
    loop {
        let disk = f.status(&s)["result"]["disk"].clone();
        if disk == "unchanged" {
            break;
        }
        assert!(start.elapsed() < HANG, "disk stayed {disk}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // Only the file and its cache are on disk.
    let listed = listing(&f.work);
    assert!(
        listed
            .iter()
            .all(|p| p == "part.silico" || p.starts_with(".silico-cache")),
        "{listed:?}"
    );
}

#[test]
fn a_first_save_with_no_edits_writes_the_empty_document() {
    let f = fixture();
    let s = f.untitled();
    f.begun(&s);
    let saved = f.commit_to(&s, r#""empty.silico""#, false);
    assert_eq!(
        saved["result"],
        json!({"written": true, "path": "empty.silico", "refingerprinted": [], "resolved": [], "problems": [], "problems_total": 0})
    );
    assert_eq!(saved["session"]["path"], "empty.silico");
    assert_eq!(f.on_disk("empty.silico"), EMPTY);
}

// ---- 7, 8: dropped unsaved ----

#[test]
fn close_is_refused_in_a_transaction_and_after_rollback_closes_leaving_nothing() {
    let f = fixture();
    let s = f.untitled();
    f.begun(&s);
    assert_eq!(f.add_box(&s)["ok"], true);
    let close = f.env("close", &format!(r#"{{"session": "{s}"}}"#));
    refused_as(
        the_error(&close),
        &format!("a transaction is open in session {s}"),
        "session",
        None,
        "commit or roll back the transaction, then close the session",
    );
    let rollback = f.env("rollback", &format!(r#"{{"session": "{s}"}}"#));
    assert_eq!(rollback["ok"], true, "{rollback}");
    assert_eq!(f.text_of(&s), EMPTY);
    let close = f.env("close", &format!(r#"{{"session": "{s}"}}"#));
    assert_eq!(close["ok"], true, "{close}");
    assert!(listing(&f.work).is_empty(), "{:?}", listing(&f.work));
    assert!(listing(&f.root.join("outside")) == ["part.silico"]);
    assert!(f.caches_opened().is_empty());
}

#[test]
fn sessions_dropped_mid_transaction_write_nothing() {
    let f = fixture();
    let s = f.untitled();
    f.begun(&s);
    assert_eq!(f.add_box(&s)["ok"], true);
    let Fixture {
        _dir,
        work,
        sessions,
        caches,
        ..
    } = f;
    drop(sessions);
    assert!(listing(&work).is_empty(), "{:?}", listing(&work));
    assert!(caches.lock().unwrap().is_empty());
}

// ---- 9, 10: the cache and format ----

#[test]
fn cache_clear_is_refused_and_removes_nothing() {
    let f = fixture();
    // Another document's cache in the workspace's root, where a clear that took the
    // session's path for "" would look.
    let entry = format!("ab/{}", "c".repeat(62));
    f.file(&format!(".silico-cache/{entry}"), "an entry");
    let before = listing(&f.work);
    let s = f.untitled();
    let cleared = f.env("cache_clear", &format!(r#"{{"session": "{s}"}}"#));
    refused_as(
        the_error(&cleared),
        &format!("session {s} is untitled: it has no file, so no rebuild cache beside one"),
        "session",
        None,
        "save it first (commit with a path), then clear the cache beside the file",
    );
    assert_eq!(listing(&f.work), before);
    assert_eq!(entries(&f.work.join(".silico-cache")), 1);
    // Inside a transaction too.
    f.begun(&s);
    let cleared = f.env("cache_clear", &format!(r#"{{"session": "{s}"}}"#));
    assert_eq!(the_error(&cleared)["parameter"], "session");
    assert_eq!(listing(&f.work), before);
}

#[test]
fn format_writing_is_refused_outside_a_transaction_and_answers_inside_one() {
    let f = fixture();
    let s = f.untitled();
    let refused = f.env(
        "format",
        &format!(r#"{{"session": "{s}", "features": null, "write": true}}"#),
    );
    refused_as(
        the_error(&refused),
        &format!("session {s} is untitled: {NO_FILE}"),
        "session",
        Some(SAVED_AS_MADE),
        BEGIN_FIRST,
    );
    // Before any rebuild: it read no model.
    assert_eq!(refused["model"], Value::Null, "{refused}");
    // Writing nothing, it answers, comparing with the empty document: unchanged.
    let checked = f.env(
        "format",
        &format!(r#"{{"session": "{s}", "features": null, "write": false}}"#),
    );
    assert_eq!(checked["ok"], true, "{checked}");
    assert_eq!(
        checked["result"],
        json!({"changed": false, "written": false, "path": null, "refingerprinted": [], "resolved": [], "accepted": [], "problems": [], "problems_total": 0})
    );
    assert_eq!(f.revision(&s), 1);
    // Inside a transaction, as a file session's format answers, but its path null.
    f.begun(&s);
    assert_eq!(f.add_box(&s)["ok"], true);
    let inside = f.env(
        "format",
        &format!(r#"{{"session": "{s}", "features": null, "write": true}}"#),
    );
    assert_eq!(inside["ok"], true, "{inside}");
    let file = f.env("open", r#"{"path": "file.silico", "create": true}"#);
    let t = file["session"]["id"].as_str().unwrap().to_owned();
    f.begun(&t);
    assert_eq!(f.add_box(&t)["ok"], true);
    let theirs = f.env(
        "format",
        &format!(r#"{{"session": "{t}", "features": null, "write": true}}"#),
    );
    assert_eq!(theirs["ok"], true, "{theirs}");
    assert_eq!(theirs["result"]["path"], "file.silico");
    let mut expected = theirs["result"].clone();
    expected["path"] = Value::Null;
    assert_eq!(inside["result"], expected);
    assert_eq!(inside["result"]["written"], false);
    assert_eq!(f.text_of(&s), f.text_of(&t));
}

// ---- 11, 12: where the first save may go ----

#[test]
fn in_a_workspace_the_first_save_is_held_inside_it() {
    let f = fixture();
    let s = f.untitled();
    f.begun(&s);
    let out = f.commit_to(&s, r#""../out.silico""#, false);
    refused_as(
        the_error(&out),
        "\"../out.silico\" is outside the workspace this server was started in",
        "path",
        None,
        "give a path inside the workspace; a link out of it is refused too",
    );
    assert!(!f.root.join("out.silico").exists());
    let status = f.status(&s);
    assert_eq!(status["session"]["path"], Value::Null);
    assert_eq!(status["session"]["transaction"], true);
    std::fs::create_dir(f.work.join("a")).unwrap();
    let saved = f.commit_to(&s, r#""a/b.silico""#, false);
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(saved["session"]["path"], "a/b.silico");
    assert_eq!(f.on_disk("a/b.silico"), EMPTY);
    assert_eq!(f.caches_opened(), [f.work.join("a/.silico-cache")]);
}

#[test]
fn two_untitled_sessions_claim_nothing_until_one_saves_and_then_its_file_is_its_own() {
    let f = fixture();
    let (s1, s2) = (f.untitled(), f.untitled());
    assert_eq!((s1.as_str(), s2.as_str()), ("s1", "s2"));
    f.begun(&s1);
    f.begun(&s2);
    assert_eq!(f.add_box(&s2)["ok"], true);
    assert_eq!(f.commit_to(&s1, r#""one.silico""#, false)["ok"], true);
    for replace in [false, true] {
        let refused = f.commit_to(&s2, r#""one.silico""#, replace);
        refused_as(
            the_error(&refused),
            "\"one.silico\" is open in session s1",
            "path",
            None,
            "give another path, or close session s1 first",
        );
        assert_eq!(refused["session"]["transaction"], true);
        assert_eq!(refused["session"]["path"], Value::Null);
    }
    // Nor by another name for the same file (a hard link): s1 claims the file itself, as
    // `open` claims one, not only the path it gave.
    std::fs::hard_link(f.work.join("one.silico"), f.work.join("hard.silico")).unwrap();
    let linked = f.commit_to(&s2, r#""hard.silico""#, true);
    refused_as(
        the_error(&linked),
        "\"hard.silico\" is open in session s1",
        "path",
        None,
        "give another path, or close session s1 first",
    );
    assert_eq!(f.on_disk("one.silico"), EMPTY);
    // Elsewhere, it saves.
    assert_eq!(f.commit_to(&s2, r#""two.silico""#, false)["ok"], true);
    assert_eq!(f.on_disk("two.silico"), canonical(40));
}

// ---- 13, 14, 15: inside a transaction, as any session ----

/// A file session on the box (`part.silico`), and an untitled one with the box added
/// inside a transaction, each rebuilt: their IDs, the file's first.
fn twins(f: &Fixture) -> (String, String) {
    let file = f.file_session("part.silico", BOX);
    let untitled = f.untitled();
    f.begun(&untitled);
    assert_eq!(f.add_box(&untitled)["ok"], true);
    (file, untitled)
}

#[test]
fn export_and_render_answer_from_an_untitled_transaction_as_from_a_file() {
    let f = fixture();
    f.kernel.answer_instead(|op| match op {
        Op::ExportMesh { .. } => Some(Reply::ExportMesh(Box::new(tetra()))),
        _ => None,
    });
    let (file, untitled) = twins(&f);
    let export = |s: &str, to: &str| {
        f.env(
            "export",
            &format!(
                r#"{{"session": "{s}", "format": "3mf", "path": "{to}", "replace": false, "features": null, "tolerance": 0.1}}"#
            ),
        )
    };
    let theirs = export(&file, "file.3mf");
    let ours = export(&untitled, "untitled.3mf");
    assert_eq!(theirs["ok"], true, "{theirs}");
    assert_eq!(ours["ok"], true, "{ours}");
    assert_eq!(ours["result"]["path"], "untitled.3mf");
    assert_eq!(ours["result"]["objects"], theirs["result"]["objects"]);
    assert_eq!(ours["result"]["objects"][0]["feature"], "k7f2qx3m");
    assert_eq!(ours["result"]["bytes"], theirs["result"]["bytes"]);
    assert_eq!(
        std::fs::metadata(f.work.join("untitled.3mf"))
            .unwrap()
            .len(),
        ours["result"]["bytes"].as_u64().unwrap()
    );
    let render = |s: &str| {
        f.env(
            "render",
            &format!(
                r#"{{"session": "{s}", "model": "current", "view": "front", "camera": null, "width": 40, "height": 30, "highlight": ["k7f2qx3m.box.Face1"], "to": null, "replace": false}}"#
            ),
        )
    };
    let (theirs, ours) = (render(&file), render(&untitled));
    assert_eq!(ours["ok"], true, "{ours}");
    assert_eq!(ours["result"], theirs["result"]);
    // Neither wrote a document.
    assert_eq!(f.on_disk("part.silico"), BOX);
    assert!(!f.work.join("untitled.silico").exists());
}

#[test]
fn undo_and_redo_step_an_untitled_transaction_as_a_file_sessions() {
    let f = fixture();
    let made = f.env("open", r#"{"path": "file.silico", "create": true}"#);
    let file = made["session"]["id"].as_str().unwrap().to_owned();
    let untitled = f.untitled();
    let mut answers = Vec::new();
    for s in [&file, &untitled] {
        f.begun(s);
        assert_eq!(f.add_box(s)["ok"], true);
        let boxed = f.text_of(s);
        let undo = f.env(
            "undo",
            &format!(r#"{{"session": "{s}", "revision": 2, "to": null}}"#),
        );
        assert_eq!(undo["ok"], true, "{undo}");
        assert_eq!(f.text_of(s), EMPTY);
        let redo = f.env(
            "redo",
            &format!(r#"{{"session": "{s}", "revision": 3, "to": null}}"#),
        );
        assert_eq!(redo["ok"], true, "{redo}");
        assert_eq!(f.text_of(s), boxed);
        answers.push((
            undo["result"].clone(),
            redo["result"].clone(),
            f.revision(s),
        ));
    }
    assert_eq!(
        answers[0],
        (
            json!({"restored": 1, "undo": 0, "redo": 1, "dropped": 0}),
            json!({"restored": 2, "undo": 1, "redo": 0, "dropped": 0}),
            4
        )
    );
    assert_eq!(answers[1], answers[0]);
    assert_eq!(f.on_disk("file.silico"), EMPTY);
}

#[test]
fn an_untitled_boxs_names_and_fingerprint_are_a_file_sessions() {
    let f = fixture();
    let (file, untitled) = twins(&f);
    let elements = |s: &str| {
        let read = f.env(
            "elements",
            &format!(
                r#"{{"session": "{s}", "model": "current", "feature": null, "after": null, "limit": 100}}"#
            ),
        );
        assert_eq!(read["ok"], true, "{read}");
        read["result"].clone()
    };
    let ours = elements(&untitled);
    assert_eq!(ours, elements(&file));
    assert_eq!(ours["total"], 26, "6 faces, 12 edges, 8 vertices: {ours}");
    // Saved, its fingerprint is the one `format` writes for the file's.
    assert_eq!(
        f.commit_to(&untitled, r#""untitled.silico""#, false)["ok"],
        true
    );
    let formatted = f.env(
        "format",
        &format!(r#"{{"session": "{file}", "features": null, "write": true}}"#),
    );
    assert_eq!(formatted["ok"], true, "{formatted}");
    assert_eq!(f.on_disk("untitled.silico"), f.on_disk("part.silico"));
    assert_eq!(f.on_disk("untitled.silico"), canonical(40));
}

// ---- After the review (#354) ----

/// An untitled session opened on `sessions`, a transaction begun, and its document made
/// `text` (inside the transaction, as `replace_text` makes one): its ID.
fn untitled_holding(
    sessions: &silico_core::command::Sessions<std::sync::Arc<silico_core::fake::FakeKernel>>,
    text: &str,
) -> String {
    let open = envelope(&call(sessions, "open", UNTITLED));
    assert_eq!(open["ok"], true, "{open}");
    let s = open["session"]["id"].as_str().unwrap().to_owned();
    let begin = envelope(&call(
        sessions,
        "begin",
        &format!(r#"{{"session": "{s}"}}"#),
    ));
    assert_eq!(begin["ok"], true, "{begin}");
    let replaced = envelope(&call(
        sessions,
        "replace_text",
        &format!(
            r#"{{"session": "{s}", "revision": 1, "text": {}}}"#,
            serde_json::to_string(text).unwrap()
        ),
    ));
    assert_eq!(replaced["ok"], true, "{replaced}");
    s
}

#[test]
fn an_untitled_parts_step_header_and_assembly_are_named_from_the_file_written() {
    // With no document file, the header's FILE_NAME is the exported file's last component
    // and the assembly its stem (docs/ports/step-export.md, as built), as the fake STEP
    // writer writes them back line by line: line 1 the file name, line 2 the assembly of
    // the two parts (the cut, and the box it consumed, named).
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (kernel, sessions, _) = sessions_on(&root, |_| {});
    let sessions = sessions.exporting_step(std::sync::Arc::clone(&kernel));
    let s = untitled_holding(&sessions, EXAMPLE);
    let other = root.join("elsewhere");
    std::fs::create_dir(&other).unwrap();
    for (to, file_name, assembly) in [
        (root.join("out.step"), "out.step", "out"),
        (other.join("part-b.stp"), "part-b.stp", "part-b"),
    ] {
        let exported = envelope(&call(
            &sessions,
            "export",
            &format!(
                r#"{{"session": "{s}", "format": "step", "path": {}, "replace": false, "features": ["m3tq8v0z", "k7f2qx3m"], "tolerance": null}}"#,
                serde_json::to_string(to.to_str().unwrap()).unwrap()
            ),
        ));
        assert_eq!(exported["ok"], true, "{exported}");
        let text = std::fs::read_to_string(&to).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], format!("FILE_NAME('{file_name}')"), "{text}");
        assert_eq!(lines[2], format!("ASSEMBLY('{assembly}')"), "{text}");
    }
}

#[test]
fn a_drag_is_refused_as_it_begins_in_an_untitled_session_outside_a_transaction() {
    // Its end would be an edit saved as it's made, which the session has no file for:
    // refused at `drag_begin`, before any sketch worker is started (the review of #354).
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&started);
    let sessions = silico_core::command::Sessions::with_stand_ins(
        std::sync::Arc::new(silico_core::fake::FakeKernel::new(2).unwrap()),
        move |cache| silico_core::cache::Cache::with_secret(cache, &secret),
        move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::new(silico_core::fake::FakeSketches::new())
                as Box<dyn silico_core::sketch::SketchSolver>)
        },
    );
    let open = envelope(&call(&sessions, "open", UNTITLED));
    let s = open["session"]["id"].as_str().unwrap().to_owned();
    let drag = format!(
        r#"{{"session": "{s}", "sketch": "s0000001", "point": "pa000003", "curve": null, "press": null}}"#
    );
    let refused = envelope(&call(&sessions, "drag_begin", &drag));
    refused_as(
        the_error(&refused),
        &format!("session {s} is untitled: {NO_FILE}"),
        "session",
        Some(SAVED_AS_MADE),
        BEGIN_FIRST,
    );
    assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 0);
    let status = envelope(&call(
        &sessions,
        "status",
        &format!(r#"{{"session": "{s}"}}"#),
    ));
    assert_eq!(status["result"]["drag"], Value::Null, "{status}");
    // Inside a transaction, it's judged as any session's drag: here, of no such sketch.
    let begin = envelope(&call(
        &sessions,
        "begin",
        &format!(r#"{{"session": "{s}"}}"#),
    ));
    assert_eq!(begin["ok"], true);
    let inside = envelope(&call(&sessions, "drag_begin", &drag));
    assert_eq!(the_error(&inside)["parameter"], "sketch", "{inside}");
}

#[test]
fn status_straight_after_the_first_save_finds_the_file_unchanged() {
    // The watch is made holding what the save wrote: `status`, the first command after the
    // commit, reads `unchanged`, not `null` before a first comparison has ended.
    let f = fixture();
    let s = f.untitled();
    f.begun(&s);
    assert_eq!(f.commit_to(&s, r#""part.silico""#, false)["ok"], true);
    assert_eq!(f.status(&s)["result"]["disk"], "unchanged");
}

#[test]
fn the_apps_first_save_writes_an_absolute_path_over_a_file_there() {
    // The app's own save: no workspace, the path the system's save panel gives (absolute),
    // a file already there, and `replace: true` (the panel asked).
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (_kernel, sessions, caches) = sessions_on(&root, |_| {});
    let target = root.join("x.silico");
    std::fs::write(&target, "an old file").unwrap();
    let s = untitled_holding(&sessions, BOX);
    let path = target.to_str().unwrap();
    let saved = envelope(&call(
        &sessions,
        "commit",
        &format!(
            r#"{{"session": "{s}", "path": {}, "replace": true}}"#,
            serde_json::to_string(path).unwrap()
        ),
    ));
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(saved["session"]["path"], path);
    assert_eq!(saved["result"]["path"], path);
    // The box's text in canonical form, fingerprinted: it was reached in the transaction.
    assert_eq!(std::fs::read_to_string(&target).unwrap(), canonical(40));
    assert_eq!(caches.lock().unwrap().clone(), [root.join(".silico-cache")]);
}
