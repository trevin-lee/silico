//! Hostile inputs for `text`'s `revision` and the restore built on it (break-it, Fable's
//! decision of 2026-11-04, the three core gaps, section 2): a restore of a document just
//! under 16 MiB through `begin`, `replace_text`, `undo`, and `redo`, held within the undo
//! stack's bound; requests wrong in every way the reader can be (zero, negative, fractional,
//! past `u64`, a string, a boolean, an object, a key given twice, missing); `u64::MAX`, read
//! and not held; a replaced text that isn't a document, or isn't UTF-8; and a session closed
//! or never opened. Expected values: sizes counted by hand from the stack's rule (`begin`'s
//! bytes, and each state but the session's), the reader's own message where the refusal
//! quotes it, and the rule that every answer is a result or a structured refusal naming the
//! field at fault, the session answering its next command after each.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::wire::{MAX_RESPONSE_LEN, Named};
use silico_core::command::{Command, Response, Revision, SessionId, Sessions, TextAnswer};
use silico_core::document;
use silico_core::document::json::MAX_DOCUMENT_LEN;
use silico_core::document::undo::{
    CHUNK, CHUNK_BYTES, ENTRY_BYTES, LIST_BYTES, UNDO_BYTES, WHOLE_BYTES,
};
use silico_core::fake::FakeKernel;
use tempfile::TempDir;

use crate::sessions::{answer, id, refusal};

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions =
        Sessions::with_cache_opener(kernel, move |cache| Cache::with_secret(cache, &secret));
    Fixture { dir, sessions }
}

fn rev(n: u64) -> Revision {
    Revision::from_number(n).unwrap()
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("a.silico")
    }

    fn write(&self, text: &[u8]) {
        std::fs::write(self.path(), text).unwrap();
    }

    fn open(&self, text: &[u8]) -> SessionId {
        self.write(text);
        answer(self.sessions.open(self.path(), false))
            .session
            .unwrap()
            .id
    }

    fn text(
        &self,
        s: &SessionId,
        feature: Option<&str>,
        revision: Option<u64>,
    ) -> Response<TextAnswer> {
        let revision = revision.map(|n| Named::Revision(rev(n)));
        answer(self.sessions.text_at(s, feature.map(id), revision))
    }

    /// `text` of the file on disk (`"disk"`), or its `feature`.
    fn disk_text(&self, s: &SessionId, feature: Option<&str>) -> Response<TextAnswer> {
        answer(self.sessions.text_at(s, feature.map(id), Some(Named::Disk)))
    }

    /// `bytes` written outside, then `text` of the document now, whose own reread reloads
    /// it, then `text` of revision 1, which reads nothing of the file: the text replaced.
    fn reloaded_text(&self, s: &SessionId, bytes: &[u8]) -> Response<TextAnswer> {
        self.write(bytes);
        let now = self.text(s, None, None);
        assert!(now.session.as_ref().unwrap().reloaded);
        let read = self.text(s, None, Some(1));
        assert!(!read.session.as_ref().unwrap().reloaded);
        read
    }

    /// `text` called by name with `arguments`, as MCP's server calls it: the envelope.
    fn called(&self, arguments: &str) -> Value {
        let answered = self.sessions.call_command(Command::Text, arguments).wait();
        serde_json::from_str(&answered.text).unwrap()
    }

    /// What the session's transaction holds: (changed, bytes, dropped).
    fn stack(&self, s: &SessionId) -> (bool, usize, usize) {
        let t = self.sessions.status(s).result.unwrap().transaction.unwrap();
        (t.changed, t.bytes, t.dropped)
    }
}

const HEADER: &str =
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": ["#;

/// The ID of feature `n`: 8 characters of the alphabet, distinct for each `n` below 32^7.
fn feature_id(n: usize) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut id = String::from("b");
    let mut rest = n;
    for _ in 0..7 {
        id.insert(1, char::from(ALPHABET[rest % 32]));
        rest /= 32;
    }
    id
}

/// `n` boxes named with `name_len` n's, the `changed`th 5 long and the others 4, in
/// canonical form.
fn boxes(n: usize, name_len: usize, changed: Option<usize>) -> String {
    let name = "n".repeat(name_len);
    let features: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id": "{}", "type": "box", "v": 1, "name": "{name}", "position": [0, 0, 0], "length": {}, "width": 2, "height": 1}}"#,
                feature_id(i),
                if Some(i) == changed { 5 } else { 4 }
            )
        })
        .collect();
    let text = format!("{HEADER}{}]}}", features.join(", "));
    document::write(&document::read(text.as_bytes()).unwrap()).unwrap()
}

/// [`boxes`] whose canonical text is as near [`MAX_DOCUMENT_LEN`] as names of one length
/// make it, without passing it.
fn near_the_limit(n: usize, changed: Option<usize>) -> String {
    let base = boxes(n, 1, changed).len() - n;
    let full = boxes(n, (MAX_DOCUMENT_LEN - base) / n, changed);
    assert!(
        full.len() <= MAX_DOCUMENT_LEN && full.len() > MAX_DOCUMENT_LEN - n,
        "{}",
        full.len()
    );
    full
}

#[test]
fn a_restore_of_just_under_sixteen_mib_fits_the_undo_bound_and_steps_back_and_forth() {
    const N: usize = 1000;
    let f = fixture();
    let original = near_the_limit(N, None);
    let agents = near_the_limit(N, Some(617));
    assert_eq!(original.len(), agents.len());
    let s = f.open(original.as_bytes());
    // Read back whole, past the reload that replaced it.
    let read = f.reloaded_text(&s, agents.as_bytes());
    assert_eq!(read.session.unwrap().revision, rev(2));
    assert!(read.result.unwrap().text == original, "revision 1's text");
    // By name too: the answer, a little over 16 MiB with its escapes, under the response's
    // bound.
    let envelope = f.called(&format!(
        r#"{{"session": "{s}", "feature": null, "revision": 1}}"#
    ));
    assert_eq!(envelope["ok"], true);
    assert!(envelope["result"]["text"].as_str() == Some(original.as_str()));
    const { assert!(MAX_RESPONSE_LEN > 2 * MAX_DOCUMENT_LEN) };

    answer(f.sessions.begin(&s)).result.unwrap();
    let replaced = answer(f.sessions.replace_text(&s, rev(2), original.clone()));
    assert_eq!(replaced.session.unwrap().revision, rev(3));
    assert!(replaced.result.unwrap().changed);
    // The stack holds `begin`'s bytes, the file's, whole; the session's own state isn't
    // counted while it's the session's.
    assert_eq!(f.stack(&s), (true, agents.len(), 0));
    assert!(f.text(&s, None, None).result.unwrap().text == original);

    // Undone: the agent's text back, the restore now held as its chunks: its features'
    // texts (fewer bytes than the document's, which adds the header and the separators), a
    // list of ceil(1000 / 64) = 16 entries, and each chunk's own count.
    let undone = answer(f.sessions.undo(&s, rev(3), None));
    assert_eq!(undone.result.unwrap().restored, rev(2));
    let chunks = N.div_ceil(CHUNK);
    assert_eq!(chunks, 16);
    let (changed, bytes, dropped) = f.stack(&s);
    assert!(!changed);
    assert_eq!(dropped, 0);
    let most = agents.len() + original.len() + LIST_BYTES + chunks * (ENTRY_BYTES + CHUNK_BYTES);
    assert!(bytes > agents.len() + original.len() - 64 * 1024, "{bytes}");
    assert!(bytes <= most, "{bytes} > {most}");
    // Well within the bound: `begin`'s bytes and one whole state.
    const { assert!(2 * (MAX_DOCUMENT_LEN + WHOLE_BYTES) < UNDO_BYTES) };
    assert!(f.text(&s, None, None).result.unwrap().text == agents);
    // Redone: the restore, byte for byte; rolled back, the file untouched.
    let four = f.sessions.status(&s).session.unwrap().revision;
    let redone = answer(f.sessions.redo(&s, four, None));
    assert_eq!(redone.result.unwrap().restored, rev(3));
    assert!(f.text(&s, None, None).result.unwrap().text == original);
    answer(f.sessions.rollback(&s)).result.unwrap();
    assert!(std::fs::read(f.path()).unwrap() == agents.as_bytes());
}

/// The one error a refused envelope gives, as (code, parameter, message).
fn refused(envelope: &Value) -> (String, String, String) {
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(
        envelope["errors"].as_array().unwrap().len(),
        1,
        "{envelope}"
    );
    let e = &envelope["errors"][0];
    (
        e["code"].as_str().unwrap().to_owned(),
        e["parameter"].as_str().unwrap_or("").to_owned(),
        e["message"].as_str().unwrap().to_owned(),
    )
}

#[test]
fn every_revision_the_reader_cant_take_is_refused_naming_revision() {
    let f = fixture();
    let s = f.open(boxes(1, 1, None).as_bytes());
    for value in [
        "0",
        "-1",
        "1.5",
        "1.0",
        "1e0",
        "18446744073709551616",
        "99999999999999999999999999999999",
        "\"1\"",
        "true",
        "{}",
        "[1]",
    ] {
        let envelope = f.called(&format!(
            r#"{{"session": "{s}", "feature": null, "revision": {value}}}"#
        ));
        let (code, parameter, _) = refused(&envelope);
        assert_eq!(
            (code.as_str(), parameter.as_str()),
            ("invalid_input", "revision"),
            "{value}"
        );
        // Nothing was run: the session's revision is as it was.
        assert!(
            envelope["session"].is_null() || envelope["session"]["revision"] == 1,
            "{value}"
        );
    }
    let missing = f.called(&format!(r#"{{"session": "{s}", "feature": null}}"#));
    assert_eq!(
        refused(&missing),
        (
            "invalid_input".to_owned(),
            "revision".to_owned(),
            "revision is missing".to_owned()
        )
    );
    let twice = f.called(&format!(
        r#"{{"session": "{s}", "feature": null, "revision": 1, "revision": 1}}"#
    ));
    // The reader refuses it before any field is read, so it names no parameter: its message
    // says where. By hand: `{"session": "` is 13 characters, then the session's ID, then
    // `", "feature": null, "revision": 1, ` is 35, so the second key starts at the column
    // after those, 51 for `s1`.
    let (code, _, message) = refused(&twice);
    assert_eq!(code, "invalid_input");
    assert!(twice["errors"][0]["parameter"].is_null(), "{twice}");
    let column = 13 + s.to_string().len() + 35 + 1;
    assert_eq!(
        message,
        format!(
            "the request isn't valid JSON: the key \"revision\" is written twice in one object, at line 1, column {column}"
        )
    );
    assert_eq!(
        twice["errors"][0]["suggestion"],
        "fix the request there: a request is strict JSON (RFC 8259), each key given once"
    );
    // `u64::MAX` reads, and isn't held: refused naming it, the one held its candidate.
    let most = f.called(&format!(
        r#"{{"session": "{s}", "feature": null, "revision": 18446744073709551615}}"#
    ));
    let (code, parameter, message) = refused(&most);
    assert_eq!(
        (code.as_str(), parameter.as_str()),
        ("invalid_input", "revision")
    );
    assert_eq!(
        message,
        format!(
            "session {s} doesn't hold the text of revision 18446744073709551615: it holds revision 1"
        )
    );
    assert_eq!(most["errors"][0]["candidates"], serde_json::json!(["1"]));
    // And the session answers after each.
    let fine = f.called(&format!(
        r#"{{"session": "{s}", "feature": null, "revision": 1}}"#
    ));
    assert_eq!(fine["ok"], true);
}

#[test]
fn a_replaced_text_that_isnt_a_document_is_given_whole_and_its_feature_refused_as_before() {
    let f = fixture();
    let s = f.open(b"not json");
    // While it's the session's: the text whole, a feature refused with the reader's error.
    let before = f.text(&s, Some("b0000000"), None).result.unwrap_err();
    let reader = document::read(b"not json").unwrap_err();
    assert_eq!(before.errors()[0].message(), reader.message());
    // Mended outside and reloaded: revision 1 is still that text, and its feature still
    // the same refusal, read from the text kept (the session keeps no finding of it).
    let read = f.reloaded_text(&s, boxes(1, 1, None).as_bytes());
    assert_eq!(read.result.unwrap().text, "not json");
    let after = f.text(&s, Some("b0000000"), Some(1)).result.unwrap_err();
    assert_eq!(after, before);
    // The mended revision has the feature.
    assert!(f.text(&s, Some("b0000000"), None).result.is_ok());
}

#[test]
fn a_replaced_text_that_isnt_utf8_is_refused_as_text_and_the_session_answers_after() {
    let f = fixture();
    let s = f.open(b"{\"silico\": 1, \xff}");
    // While it's the session's revision, named or not: the file is to be mended.
    let now = f.text(&s, None, None);
    let (code, message, parameter, suggestion) = refusal(&now);
    assert_eq!(code, ErrorCode::InvalidInput);
    // By hand: `{"silico": 1, ` is 14 bytes, so the 0xff is at byte 14.
    assert_eq!(
        message,
        "the document's text isn't UTF-8 (at byte 14), so it can't be given as text"
    );
    assert_eq!(parameter, None);
    assert_eq!(
        suggestion.as_deref(),
        Some("mend the file: a document is UTF-8 text")
    );
    assert_eq!(refusal(&f.text(&s, None, Some(1))), refusal(&now));
    // Mended outside and reloaded: the file was mended, so the refusal of revision 1 names
    // `revision`, whose text can't be given as text, and points to the document now.
    let mended = boxes(1, 1, None);
    f.write(mended.as_bytes());
    // Revision 1 by number reads nothing of the file: the document now's read reloads it.
    let replaced = f.text(&s, None, Some(1));
    assert!(!replaced.session.as_ref().unwrap().reloaded);
    assert_eq!(refusal(&replaced), refusal(&now));
    assert!(f.text(&s, None, None).session.unwrap().reloaded);
    let replaced = f.text(&s, None, Some(1));
    old_revision_not_utf8(&replaced, 1, 14);
    assert_eq!(f.text(&s, None, None).result.unwrap().text, mended);
    assert_eq!(f.text(&s, None, Some(2)).result.unwrap().text, mended);
}

/// The refusal of revision `n`, not the session's now, whose text isn't UTF-8 from byte
/// `at`: naming `revision`, with the document now as the way on.
#[track_caller]
fn old_revision_not_utf8(response: &Response<TextAnswer>, n: u64, at: usize) {
    let (code, message, parameter, suggestion) = refusal(response);
    assert_eq!(code, ErrorCode::InvalidInput);
    assert_eq!(
        message,
        format!("revision {n}'s text isn't UTF-8 (at byte {at}), so it can't be given as text")
    );
    assert_eq!(parameter.as_deref(), Some("revision"));
    assert_eq!(
        suggestion.as_deref(),
        Some("give revision null for the document now, or another revision the session holds")
    );
    let error = &response.result.as_ref().unwrap_err().errors()[0];
    assert_eq!(
        error.cause(),
        Some(
            format!(
                "revision {n}'s text is the bytes the session held then, and they aren't UTF-8"
            )
            .as_str()
        )
    );
}

#[test]
fn a_base_that_isnt_utf8_is_refused_naming_revision_once_replaced() {
    let f = fixture();
    // By hand: `not ` is 4 bytes, so the 0xfe is at byte 4.
    let s = f.open(b"not \xfe utf-8");
    answer(f.sessions.begin(&s)).result.unwrap();
    let mended = boxes(1, 1, None);
    let replaced = answer(f.sessions.replace_text(&s, rev(1), mended.clone()));
    assert_eq!(replaced.session.unwrap().revision, rev(2));
    // The base, `begin`'s bytes, isn't the session's revision now: refused naming it.
    old_revision_not_utf8(&f.text(&s, None, Some(1)), 1, 4);
    assert_eq!(f.text(&s, None, None).result.unwrap().text, mended);
}

#[test]
fn a_revision_of_a_session_closed_or_never_opened_is_refused_naming_session() {
    let f = fixture();
    let s = f.open(boxes(1, 1, None).as_bytes());
    answer(f.sessions.close(&s)).result.unwrap();
    for session in [s, SessionId::parse("s99").unwrap()] {
        let read = f.text(&session, None, Some(1));
        let (code, _, parameter, _) = refusal(&read);
        assert_eq!(code, ErrorCode::InvalidInput);
        assert_eq!(parameter.as_deref(), Some("session"));
    }
}

#[test]
fn a_revision_that_almost_says_disk_is_refused_by_the_reader_and_disk_itself_reads() {
    // Fable's 6d decision: `"disk"` exactly, or a revision, or null; each near miss refused
    // naming `revision`, said as written (a string quoted, a number bare).
    let f = fixture();
    let one = boxes(1, 1, None);
    let s = f.open(one.as_bytes());
    for (value, said) in [
        (r#""Disk""#, r#""Disk""#),
        (r#""disk ""#, r#""disk ""#),
        (r#"" disk""#, r#"" disk""#),
        (r#""DISK""#, r#""DISK""#),
        (r#""""#, r#""""#),
        (r#""1""#, r#""1""#),
        ("0", "0"),
        ("-1", "-1"),
    ] {
        let envelope = f.called(&format!(
            r#"{{"session": "{s}", "feature": null, "revision": {value}}}"#
        ));
        let (code, parameter, message) = refused(&envelope);
        assert_eq!(
            (code.as_str(), parameter.as_str()),
            ("invalid_input", "revision"),
            "{value}"
        );
        assert_eq!(
            message,
            format!(r#"revision must be an integer from 1, "disk", or null, got {said}"#)
        );
    }
    // `true`, by its kind.
    let envelope = f.called(&format!(
        r#"{{"session": "{s}", "feature": null, "revision": true}}"#
    ));
    assert_eq!(refused(&envelope).1, "revision");
    // The word: the file's bytes, outside a transaction reloading nothing (unchanged).
    let envelope = f.called(&format!(
        r#"{{"session": "{s}", "feature": null, "revision": "disk"}}"#
    ));
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["result"]["text"], one.as_str());
    assert_eq!(envelope["session"]["reloaded"], false);
}

#[test]
fn text_of_disk_just_under_sixteen_mib_inside_a_transaction_is_given_whole() {
    // The agent's text near the limit, read through `"disk"` inside a transaction (nothing
    // reloaded), whole; by name too, its answer under the response's bound.
    const N: usize = 1000;
    let f = fixture();
    let original = near_the_limit(N, None);
    let agents = near_the_limit(N, Some(617));
    let s = f.open(original.as_bytes());
    answer(f.sessions.begin(&s)).result.unwrap();
    f.write(agents.as_bytes());
    let read = f.disk_text(&s, None);
    assert_eq!(read.session.as_ref().unwrap().revision, rev(1));
    assert!(read.result.unwrap().text == agents, "the file's text");
    let envelope = f.called(&format!(
        r#"{{"session": "{s}", "feature": null, "revision": "disk"}}"#
    ));
    assert_eq!(envelope["ok"], true);
    assert!(envelope["result"]["text"].as_str() == Some(agents.as_str()));
    // A feature of it: its canonical text, the 617th box 5 long.
    let id = feature_id(617);
    let feature = f.disk_text(&s, Some(&id)).result.unwrap().text;
    assert!(feature.contains("\"length\": 5,"), "{}", &feature[..200]);
    // The session's own text is `begin`'s still.
    assert!(f.text(&s, None, None).result.unwrap().text == original);
}
