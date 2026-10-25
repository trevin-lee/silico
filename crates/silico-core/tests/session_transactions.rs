//! M4.2's transactions and `format` (docs/design/m4.1-command-api.md, sections 4 and 7,
//! decisions 1, 22, 28 and 31), as far as a client reaches them before edits come: one
//! transaction at a time, `close` and `format` refused inside one, a commit of nothing
//! changed writing nothing, and `format` accepting only the drift it names. Expected values
//! are the design's messages, the box's analytic fingerprint (40 × 20 × 10: volume 8000,
//! area 2800), and the files' bytes, written by hand.

use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{Begun, Response, Revision, Running, SessionId, Sessions};
use silico_core::document::FeatureId;
use silico_core::fake::FakeKernel;
use tempfile::TempDir;

/// Longer than any command here takes on a loaded machine: past it, a command hangs.
const HANG: Duration = Duration::from_secs(60);

/// The box's fingerprint as the fake measures it, by formula.
const FINGERPRINT: &str = r#""fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}"#;

/// A block, its fingerprint stored with its volume `volume`, and a lug clear of it with
/// none stored, on one line each: not canonical form.
fn stored(volume: &str) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "fingerprint": {{"volume": {volume}, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}}}, {{"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10}}]}}"#
    )
}

/// [`stored`]'s document in canonical form, the block's fingerprint `block` (a member's
/// text) and the lug's `lug` (one, or none): written by hand from M2.1's layout.
fn canonical(block: &str, lug: Option<&str>) -> String {
    let lug = match lug {
        None => r#"    {"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10}"#.to_owned(),
        Some(lug) => format!(
            "    {{\n      \"id\": \"b0000001\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Lug\",\n      \"position\": [100, 0, 0],\n      \"length\": 10,\n      \"width\": 10,\n      \"height\": 10,\n      {lug}\n    }}"
        ),
    };
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n    {{\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 10,\n      {block}\n    }},\n{lug}\n  ]\n}}\n"
    )
}

/// The lug's fingerprint, by formula: 10 × 10 × 10 = 1000, area 600, at x = 100.
const LUG: &str = r#""fingerprint": {"volume": 1000, "area": 600, "bbox": [100, 0, 0, 110, 10, 10], "faces": 6, "edges": 12, "vertices": 8}"#;

/// The block's fingerprint stored 1e-6 mm³ over what it measures.
const DRIFTED: &str = r#""fingerprint": {"volume": 8000.000001, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}"#;

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let sessions =
        Sessions::with_cache_opener(Arc::new(FakeKernel::new(2).unwrap()), move |cache| {
            Cache::with_secret(cache, &secret)
        });
    Fixture { dir, sessions }
}

impl Fixture {
    fn file(&self, text: &str) -> PathBuf {
        let path = self.dir.path().join("a.silico");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn open(&self, text: &str) -> (SessionId, PathBuf) {
        let path = self.file(text);
        let response = answer(self.sessions.open(&path, false));
        assert!(response.result.is_ok(), "{response:?}");
        (response.session.unwrap().id, path)
    }
}

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

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

#[test]
fn begin_gives_its_base_and_a_second_begin_is_refused() {
    let f = fixture();
    let (s1, _) = f.open(&stored("8000"));
    let begun = answer(f.sessions.begin(&s1));
    assert_eq!(
        begun.result.as_ref().unwrap(),
        &Begun {
            base: Revision::OPENED
        }
    );
    let session = begun.session.unwrap();
    assert!(session.transaction);
    assert_eq!(session.revision, Revision::OPENED);
    assert!(f.sessions.status(&s1).session.unwrap().transaction);
    // The design's section 2 example, word for word.
    assert_eq!(
        refusal(&answer(f.sessions.begin(&s1))),
        (
            ErrorCode::InvalidInput,
            "a transaction is already open in this session".to_owned(),
            Some("session".to_owned()),
            Some("commit or roll back the open transaction, then begin again".to_owned())
        )
    );
}

#[test]
fn commit_and_rollback_with_no_transaction_open_are_refused() {
    let f = fixture();
    let (s1, _) = f.open(&stored("8000"));
    let refused = (
        ErrorCode::InvalidInput,
        "no transaction is open in session s1".to_owned(),
        Some("session".to_owned()),
        Some("begin a transaction first; outside one, every edit is saved as it's made".to_owned()),
    );
    let commit = answer(f.sessions.commit(&s1));
    assert_eq!(refusal(&commit), refused);
    assert_eq!(commit.session.unwrap().revision, Revision::OPENED);
    assert_eq!(refusal(&answer(f.sessions.rollback(&s1))), refused);
}

#[test]
fn close_is_refused_while_a_transaction_is_open_and_then_closes() {
    let f = fixture();
    let (s1, path) = f.open(&stored("8000"));
    answer(f.sessions.begin(&s1)).result.unwrap();
    let close = answer(f.sessions.close(&s1));
    assert_eq!(
        refusal(&close),
        (
            ErrorCode::InvalidInput,
            "a transaction is open in session s1".to_owned(),
            Some("session".to_owned()),
            Some("commit or roll back the transaction, then close the session".to_owned())
        )
    );
    // Still open: its file isn't free.
    assert!(answer(f.sessions.open(&path, false)).result.is_err());
    let rolled = answer(f.sessions.rollback(&s1));
    assert_eq!(rolled.result.unwrap().restored, Revision::OPENED);
    // A new revision, whose state is the first's.
    assert_eq!(
        rolled.session.unwrap().revision,
        Revision::OPENED.next().unwrap()
    );
    answer(f.sessions.close(&s1)).result.unwrap();
    assert!(answer(f.sessions.open(&path, false)).result.is_ok());
}

#[test]
fn format_is_refused_inside_a_transaction() {
    let f = fixture();
    let text = stored("8000");
    let (s1, path) = f.open(&text);
    answer(f.sessions.begin(&s1)).result.unwrap();
    assert_eq!(
        refusal(&answer(f.sessions.format(&s1, None, true))),
        (
            ErrorCode::InvalidInput,
            "a transaction is open in session s1".to_owned(),
            Some("session".to_owned()),
            Some("commit or roll back the transaction, then format".to_owned())
        )
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn a_commit_with_nothing_changed_writes_nothing() {
    let f = fixture();
    // Not in canonical form, and the lug has no fingerprint: anything written would change
    // the bytes.
    let text = stored("8000");
    let (s1, path) = f.open(&text);
    let inode = std::fs::metadata(&path).unwrap().ino();
    answer(f.sessions.begin(&s1)).result.unwrap();
    let response = answer(f.sessions.commit(&s1));
    let committed = response.result.unwrap();
    assert!(!committed.written);
    assert!(committed.refingerprinted.is_empty());
    assert!(committed.problems.is_empty(), "{:?}", committed.problems);
    let session = response.session.unwrap();
    assert_eq!(
        (session.revision, session.transaction),
        (Revision::OPENED, false)
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
}

#[test]
fn format_writes_every_fingerprint_but_the_drift_it_doesnt_accept() {
    let f = fixture();
    let (s1, path) = f.open(&stored("8000.000001"));
    // Accepting the lug's drift only (it has none): the block's is kept as stored and said,
    // and the lug gets its first fingerprint.
    let response = answer(f.sessions.format(&s1, Some(vec![id("b0000001")]), true));
    assert_eq!(response.session.as_ref().unwrap().revision.number(), 2);
    let formatted = response.result.unwrap();
    assert!(formatted.changed && formatted.written);
    assert_eq!(formatted.path, path);
    assert_eq!(formatted.refingerprinted, [id("b0000001")]);
    assert!(formatted.accepted.is_empty());
    let drift = &formatted.problems.errors()[0];
    assert_eq!(formatted.problems.total(), 1);
    assert_eq!(
        (drift.code(), drift.feature()),
        (ErrorCode::Drift, Some("k7f2qx3m"))
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        canonical(DRIFTED, Some(LUG))
    );
    // Accepting the block's, by name: written, and named as accepted.
    let response = answer(f.sessions.format(&s1, Some(vec![id("k7f2qx3m")]), true));
    assert_eq!(response.session.as_ref().unwrap().revision.number(), 3);
    let formatted = response.result.unwrap();
    assert!(formatted.changed && formatted.written);
    assert_eq!(formatted.refingerprinted, [id("k7f2qx3m")]);
    assert_eq!(formatted.accepted, [id("k7f2qx3m")]);
    assert!(formatted.problems.is_empty(), "{:?}", formatted.problems);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        canonical(FINGERPRINT, Some(LUG))
    );
    // And again: nothing differs, so nothing is written.
    let response = answer(f.sessions.format(&s1, None, true));
    let formatted = response.result.unwrap();
    assert!(!formatted.changed && !formatted.written);
    assert!(formatted.refingerprinted.is_empty());
    assert_eq!(response.session.unwrap().revision.number(), 3);
}

#[test]
fn format_accepts_every_drift_with_no_features_named() {
    let f = fixture();
    let (s1, path) = f.open(&stored("8000.000001"));
    let formatted = answer(f.sessions.format(&s1, None, true)).result.unwrap();
    assert_eq!(formatted.accepted, [id("k7f2qx3m")]);
    assert_eq!(formatted.refingerprinted, [id("k7f2qx3m"), id("b0000001")]);
    assert!(formatted.problems.is_empty(), "{:?}", formatted.problems);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        canonical(FINGERPRINT, Some(LUG))
    );
}

#[test]
fn format_without_write_says_what_it_would_and_writes_nothing() {
    let f = fixture();
    let text = stored("8000");
    let (s1, path) = f.open(&text);
    let response = answer(f.sessions.format(&s1, None, false));
    let formatted = response.result.unwrap();
    assert!(formatted.changed);
    assert!(!formatted.written);
    assert_eq!(formatted.refingerprinted, [id("b0000001")]);
    assert_eq!(response.session.unwrap().revision, Revision::OPENED);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn format_refuses_a_feature_the_document_hasnt_writing_nothing() {
    let f = fixture();
    let text = stored("8000.000001");
    let (s1, path) = f.open(&text);
    let response = answer(
        f.sessions
            .format(&s1, Some(vec![id("k7f2qx3m"), id("zzzzzzzz")]), true),
    );
    assert_eq!(
        refusal(&response),
        (
            ErrorCode::InvalidInput,
            "the document has no feature zzzzzzzz to accept the drift of".to_owned(),
            Some("features.1".to_owned()),
            Some("name features by their IDs, as the document holds them".to_owned())
        )
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn format_of_a_document_the_gate_refuses_is_refused_with_its_problems() {
    let f = fixture();
    let text = stored("8000").replace(r#""length": 40"#, r#""length": 0"#);
    let (s1, path) = f.open(&text);
    let response = answer(f.sessions.format(&s1, None, true));
    let problems = response.result.unwrap_err();
    assert_eq!(problems.errors()[0].parameter(), Some("length"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn a_file_a_save_replaced_is_still_claimed_by_its_session() {
    // A save renames a new file over the old: another inode, which a hard link made after it
    // reaches. The session claims it, so the link doesn't open it a second time.
    let f = fixture();
    let (s1, path) = f.open(&stored("8000"));
    let inode = std::fs::metadata(&path).unwrap().ino();
    assert!(
        answer(f.sessions.format(&s1, None, true))
            .result
            .unwrap()
            .written
    );
    assert_ne!(std::fs::metadata(&path).unwrap().ino(), inode);
    let link = f.dir.path().join("link.silico");
    std::fs::hard_link(&path, &link).unwrap();
    let response = answer(f.sessions.open(&link, false));
    let (code, message, parameter, _) = refusal(&response);
    assert_eq!(code, ErrorCode::InvalidInput);
    assert!(
        message.ends_with("is already open in session s1"),
        "{message}"
    );
    assert_eq!(parameter.as_deref(), Some("path"));
}
