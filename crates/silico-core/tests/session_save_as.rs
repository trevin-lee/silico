//! Save As (M5.4; docs/design/m5.0-app.md, open item 22 and section 4): `commit` with a
//! `path` writes the transaction's document there, under decision 35's rules for a file a
//! command writes (as a document), and the session holds the new path from then on, its old
//! file let go. Expected values are the paths the requests give, the files' bytes written
//! by hand (M2.1's canonical layout, the box's fingerprint by formula: 40 × 20 × 10, volume
//! 8000, area 2800), and the refusals' words as the design and decision 35 give them.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;

use silico_core::ErrorCode;
use silico_core::command::SessionId;

use crate::calls::{BOX, Fixture, OUTSIDE, envelope, fixture};
use crate::sessions::{answer, refusal};

/// The box in canonical form, `length` long (20 wide, 10 high), with the fingerprint the
/// fake measures, by formula: volume 200·length, area 2·(20·length + 10·length + 200).
fn canonical(length: u32) -> String {
    let (volume, area) = (200 * length, 2 * (30 * length + 200));
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n    {{\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": {length},\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {{\"volume\": {volume}, \"area\": {area}, \"bbox\": [0, 0, 0, {length}, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}}\n    }}\n  ]\n}}\n"
    )
}

/// The box as given, in canonical form: on one line, as canonical form writes an object
/// that fits (M2.1), with no fingerprint (none stored, and no edit reached it).
const BOX_CANONICAL: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\"id\": \"k7f2qx3m\", \"type\": \"box\", \"v\": 1, \"name\": \"Block\", \"position\": [0, 0, 0], \"length\": 40, \"width\": 20, \"height\": 10}\n  ]\n}\n";

const S1: &str = "s1";

impl Fixture {
    /// The envelope of `status` of `session`.
    fn status_of(&self, session: &str) -> serde_json::Value {
        envelope(&self.call("status", &format!(r#"{{"session": "{session}"}}"#)))
    }

    /// `part.silico` opened as s1, a transaction begun, and its box made `length` long.
    fn lengthened(&self, length: u32) {
        self.opened();
        let begin = self.call("begin", r#"{"session": "s1"}"#);
        assert!(begin.ok, "{}", begin.text);
        let revision = &self.status_of(S1)["session"]["revision"];
        let set = self.call(
            "set_field",
            &format!(
                r#"{{"session": "s1", "revision": {revision}, "feature": "k7f2qx3m", "field": "length", "value": {length}}}"#
            ),
        );
        assert!(set.ok, "{}", set.text);
    }

    /// `commit` of s1 to `path` (a JSON value: a string or null), with `replace`.
    fn commit(&self, path: &str, replace: bool) -> serde_json::Value {
        envelope(&self.call(
            "commit",
            &format!(r#"{{"session": "s1", "path": {path}, "replace": {replace}}}"#),
        ))
    }

    /// The file `name` in the workspace, as text.
    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.work.join(name)).unwrap()
    }

    /// The one error of a refused envelope: code, message, parameter, and suggestion; the
    /// transaction still open, and s1 still at `part.silico`.
    fn refused(&self, answer: &serde_json::Value) -> (String, String, String, String) {
        assert_eq!(answer["ok"], false, "{answer}");
        let errors = answer["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1, "{answer}");
        let status = self.status_of(S1);
        assert_eq!(status["session"]["path"], "part.silico");
        assert_eq!(status["session"]["transaction"], true);
        let e = &errors[0];
        let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_owned();
        (
            text(&e["code"]),
            text(&e["message"]),
            text(&e["parameter"]),
            text(&e["suggestion"]),
        )
    }
}

#[test]
fn save_as_writes_the_document_at_the_new_path_and_the_session_holds_it() {
    let f = fixture();
    f.lengthened(50);
    let saved = f.commit(r#""copy.silico""#, false);
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(
        saved["result"],
        serde_json::json!({"written": true, "path": "copy.silico", "refingerprinted": ["k7f2qx3m"], "resolved": [], "problems": [], "problems_total": 0})
    );
    assert_eq!(saved["session"]["path"], "copy.silico");
    assert_eq!(saved["session"]["transaction"], false);
    // The new file, byte for byte; the old one as it was.
    assert_eq!(f.read("copy.silico"), canonical(50));
    assert_eq!(f.read("part.silico"), BOX);
    assert_eq!(f.status_of(S1)["session"]["path"], "copy.silico");

    // The session claims the new file and has let the old one go.
    let copy = f.open("copy.silico", false);
    assert_eq!(
        envelope(&copy)["errors"][0]["message"],
        "\"copy.silico\" is already open in session s1"
    );
    let old = f.open("part.silico", false);
    assert!(old.ok, "{}", old.text);

    // Its next save writes the new file, never the old.
    let begin = f.call("begin", r#"{"session": "s1"}"#);
    assert!(begin.ok, "{}", begin.text);
    let revision = &f.status_of(S1)["session"]["revision"];
    let set = f.call(
        "set_field",
        &format!(
            r#"{{"session": "s1", "revision": {revision}, "feature": "k7f2qx3m", "field": "length", "value": 60}}"#
        ),
    );
    assert!(set.ok, "{}", set.text);
    let own = f.commit("null", false);
    assert_eq!(own["result"]["path"], "copy.silico", "{own}");
    assert_eq!(f.read("copy.silico"), canonical(60));
    assert_eq!(f.read("part.silico"), BOX);
}

#[test]
fn save_as_with_nothing_changed_still_writes_the_new_file() {
    let f = fixture();
    f.opened();
    let begin = f.call("begin", r#"{"session": "s1"}"#);
    assert!(begin.ok, "{}", begin.text);
    let saved = f.commit(r#""same.silico""#, false);
    assert_eq!(
        saved["result"],
        serde_json::json!({"written": true, "path": "same.silico", "refingerprinted": [], "resolved": [], "problems": [], "problems_total": 0})
    );
    // In canonical form, no fingerprint written: no edit reached the box.
    assert_eq!(f.read("same.silico"), BOX_CANONICAL);
    assert_eq!(f.read("part.silico"), BOX);
    assert_eq!(f.status_of(S1)["session"]["path"], "same.silico");
}

#[test]
fn save_as_keeps_an_agents_change_to_the_old_file_and_saves_elsewhere() {
    // The file changed on disk with edits held: commit to it is a conflict; Save As saves
    // elsewhere and leaves the agent's file as it is (docs/design/m5.0-app.md, section 4).
    let f = fixture();
    f.lengthened(50);
    std::fs::write(f.work.join("part.silico"), OUTSIDE).unwrap();
    let own = f.commit("null", false);
    assert_eq!(own["errors"][0]["code"], "conflict", "{own}");
    let saved = f.commit(r#""mine.silico""#, false);
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(f.read("mine.silico"), canonical(50));
    assert_eq!(f.read("part.silico"), OUTSIDE);
}

#[test]
fn a_file_there_is_refused_unless_replace_and_then_keeps_its_permissions() {
    let f = fixture();
    f.lengthened(50);
    let there = f.file("copy.silico", "an old copy");
    std::fs::set_permissions(&there, std::fs::Permissions::from_mode(0o600)).unwrap();
    let refused = f.commit(r#""copy.silico""#, false);
    assert_eq!(
        f.refused(&refused),
        (
            "invalid_input".to_owned(),
            "\"copy.silico\" exists".to_owned(),
            "path".to_owned(),
            "give replace: true to replace it, or another path".to_owned()
        )
    );
    assert_eq!(f.read("copy.silico"), "an old copy");
    let replaced = f.commit(r#""copy.silico""#, true);
    assert_eq!(replaced["ok"], true, "{replaced}");
    assert_eq!(f.read("copy.silico"), canonical(50));
    let mode = std::fs::metadata(&there).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_path_outside_the_workspace_is_refused() {
    let f = fixture();
    f.lengthened(50);
    let refused = f.commit(r#""../outside/mine.silico""#, false);
    assert_eq!(
        f.refused(&refused),
        (
            "invalid_input".to_owned(),
            "\"../outside/mine.silico\" is outside the workspace this server was started in"
                .to_owned(),
            "path".to_owned(),
            "give a path inside the workspace; a link out of it is refused too".to_owned()
        )
    );
    assert!(!f.outside.join("mine.silico").exists());
    // Through a link out of it, too.
    std::os::unix::fs::symlink(&f.outside, f.work.join("out")).unwrap();
    let linked = f.commit(r#""out/mine.silico""#, false);
    assert_eq!(
        f.refused(&linked).1,
        "\"out/mine.silico\" is outside the workspace this server was started in"
    );
    assert!(!f.outside.join("mine.silico").exists());
    assert_eq!(f.theirs(), OUTSIDE);
}

#[test]
fn a_file_another_session_holds_is_refused_by_any_path_to_it() {
    let f = fixture();
    f.file("other.silico", BOX);
    f.lengthened(50);
    let other = f.open("other.silico", false);
    assert!(other.ok, "{}", other.text);
    let s2 = envelope(&other)["session"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(s2, "s2");
    let refused = f.commit(r#""other.silico""#, true);
    assert_eq!(
        f.refused(&refused),
        (
            "invalid_input".to_owned(),
            "\"other.silico\" is open in session s2".to_owned(),
            "path".to_owned(),
            "give another path, or close session s2 first".to_owned()
        )
    );
    // A hard link to it is the same file.
    std::fs::hard_link(f.work.join("other.silico"), f.work.join("alias.silico")).unwrap();
    let aliased = f.commit(r#""alias.silico""#, true);
    assert_eq!(
        f.refused(&aliased).1,
        "\"alias.silico\" is open in session s2"
    );
    assert_eq!(f.read("other.silico"), BOX);
}

#[test]
fn the_sessions_own_file_a_directory_and_another_extension_are_refused() {
    let f = fixture();
    f.lengthened(50);
    let own = f.commit(r#""part.silico""#, true);
    assert_eq!(
        f.refused(&own),
        (
            "invalid_input".to_owned(),
            "\"part.silico\" is this session's own file".to_owned(),
            "path".to_owned(),
            "give path null to save to it, or another path".to_owned()
        )
    );
    std::fs::create_dir(f.work.join("folder.silico")).unwrap();
    let directory = f.commit(r#""folder.silico""#, true);
    assert_eq!(
        f.refused(&directory),
        (
            "invalid_input".to_owned(),
            "path \"folder.silico\": is there, and isn't a file".to_owned(),
            "path".to_owned(),
            "give the path of a file to write".to_owned()
        )
    );
    let text = f.commit(r#""copy.json""#, false);
    assert_eq!(
        f.refused(&text),
        (
            "invalid_input".to_owned(),
            "path \"copy.json\": doesn't end in .silico".to_owned(),
            "path".to_owned(),
            "give a path ending in .silico".to_owned()
        )
    );
    assert!(!f.work.join("copy.json").exists());
    let cache = f.commit(r#""x/.silico-cache/copy.silico""#, false);
    assert_eq!(f.refused(&cache).0, "invalid_input");
    assert_eq!(f.read("part.silico"), BOX);
}

#[test]
fn a_path_that_isnt_utf8_is_refused_naming_path() {
    let f = fixture();
    f.lengthened(50);
    let id = SessionId::parse(S1).unwrap();
    let path = f.work.join(OsStr::from_bytes(b"copy-\xff.silico"));
    let refused = answer(f.sessions.save_as(&id, &path, false));
    let (code, message, parameter, suggestion) = refusal(&refused);
    assert_eq!(code, ErrorCode::InvalidInput);
    assert!(
        message.ends_with("isn't UTF-8, so the API can't name it exactly"),
        "{message}"
    );
    assert_eq!(parameter.as_deref(), Some("path"));
    assert_eq!(suggestion.as_deref(), Some("give a path that is UTF-8"));
    assert!(!path.exists());
    assert_eq!(f.status_of(S1)["session"]["transaction"], true);
}

#[test]
fn replace_without_a_path_and_a_save_as_with_no_transaction_are_refused() {
    let f = fixture();
    f.opened();
    let both = f.commit("null", true);
    assert_eq!(both["errors"][0]["parameter"], "replace", "{both}");
    assert_eq!(
        both["errors"][0]["message"],
        "replace must be false for a commit to the session's own file (path null)"
    );
    let none = f.commit(r#""copy.silico""#, false);
    assert_eq!(
        none["errors"][0]["message"], "no transaction is open in session s1",
        "{none}"
    );
    assert!(!f.work.join("copy.silico").exists());
}

#[test]
fn the_session_watches_its_new_file_on_disk_and_reloads_an_agents_change_to_it() {
    // After a Save As, `status` compares the new file (docs/design/m5.0-app.md, section 4):
    // an agent's change to it, with no transaction open, is reloaded by the session itself.
    let f = fixture();
    f.lengthened(50);
    let saved = f.commit(r#""copy.silico""#, false);
    assert_eq!(saved["ok"], true, "{saved}");
    let before = f.status_of(S1)["session"]["revision"].as_u64().unwrap();
    std::fs::write(f.work.join("copy.silico"), OUTSIDE).unwrap();
    let deadline = std::time::Instant::now() + crate::sessions::HANG;
    loop {
        let status = f.status_of(S1);
        if status["session"]["revision"].as_u64().unwrap() > before {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never reloaded: {status}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let text = f.call("text", r#"{"session": "s1", "feature": null}"#);
    assert_eq!(envelope(&text)["result"]["text"], OUTSIDE);
    assert_eq!(f.read("part.silico"), BOX);
}
