//! M4.2's transactions and `format` (docs/design/m4.1-command-api.md, sections 4 and 7,
//! decisions 1, 22, 28 and 31), as far as a client reaches them before edits come: one
//! transaction at a time, `close` and `format` refused inside one, a commit of nothing
//! changed writing nothing, and `format` accepting only the drift it names. Expected values
//! are the design's messages, the box's analytic fingerprint (40 × 20 × 10: volume 8000,
//! area 2800), and the files' bytes, written by hand.

use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{
    Begun, ModelChoice, Published, Response, Revision, SessionId, Sessions,
};
use silico_core::fake::FakeKernel;
use tempfile::TempDir;

use crate::lateness::{Lateness, allowance};
use crate::sessions::{answer, id, refusal};

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

#[test]
fn format_refuses_past_a_hundred_unknown_features_listing_a_hundred_and_counting_all() {
    let f = fixture();
    let text = stored("8000");
    let (s1, path) = f.open(&text);
    // The block, then 150 IDs the document hasn't (z0000000, z0000001, ...).
    let mut named = vec![id("k7f2qx3m")];
    named.extend((0..150).map(|n| id(&format!("z{n:07}"))));
    let response = answer(f.sessions.format(&s1, Some(named), true));
    let problems = response.result.unwrap_err();
    assert_eq!(problems.total(), 150);
    assert_eq!(problems.errors().len(), 100);
    assert_eq!(problems.errors()[0].parameter(), Some("features.1"));
    assert_eq!(problems.errors()[99].parameter(), Some("features.100"));
    assert_eq!(
        problems.errors()[99].message(),
        "the document has no feature z0000099 to accept the drift of"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn a_published_read_while_begin_takes_a_large_text_that_didnt_read_answers_promptly() {
    // 48 MB of strings each of five escaped newlines (four million of them, twelve bytes
    // each): text that didn't read, whose every string a scan for the IDs it holds must
    // decode. `begin` keeps the text without scanning it, so the session's held state is
    // never locked for the scan, and a published read (a UI's, each frame) never waits.
    let f = fixture();
    let text = r#""\n\n\n\n\n""#.repeat(4_000_000);
    assert_eq!(text.len(), 48_000_000);
    let (s1, _) = f.open(&text);
    let lateness = Lateness::start();
    let mut begin = f.sessions.begin(&s1);
    let mut slowest = Duration::ZERO;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let at = Instant::now();
        let read = f.sessions.published(&s1);
        slowest = slowest.max(at.elapsed());
        // Nothing published: the reader's problems.
        assert!(read.result.is_err());
        match begin.wait_timeout(Duration::from_millis(1)) {
            Ok(begun) => {
                assert_eq!(
                    begun.result.unwrap(),
                    Begun {
                        base: Revision::OPENED
                    }
                );
                break;
            }
            Err(running) => begin = running,
        }
        assert!(Instant::now() < deadline, "begin never answered");
    }
    // At once: a second, and ten times the machine's lateness on a loaded one.
    let most = allowance(Duration::from_secs(1), lateness.most());
    assert!(
        slowest < most,
        "a published read took {slowest:?}, past {most:?}"
    );
}

/// `session`'s published read, as the app's frame asks for it, timed: `slowest` the longest
/// one has taken.
fn published_timed(
    f: &Fixture,
    session: &SessionId,
    slowest: &mut Duration,
) -> Response<Published> {
    let at = Instant::now();
    let read = f.sessions.published(session);
    *slowest = (*slowest).max(at.elapsed());
    read
}

#[test]
fn a_published_read_never_waits_on_the_scan_of_a_large_begun_text_that_didnt_read() {
    // As above, 48 MB of text that didn't read, begun from, and replaced in the transaction
    // by a block whose stored volume drifts: revision 2, whose edits reach every feature
    // with an ID the begun text hasn't (a scan of its every string), the block among them.
    // A published read starts revision 2's rebuild, keeping what its edits reach with it
    // unworked; a published check of its model works it out, on the check's own thread. A
    // published read each frame waits on neither.
    let f = fixture();
    let text = r#""\n\n\n\n\n""#.repeat(4_000_000);
    assert_eq!(text.len(), 48_000_000);
    let (s1, _) = f.open(&text);
    answer(f.sessions.begin(&s1)).result.unwrap();
    let second = Revision::OPENED.next().unwrap();
    let replaced = answer(
        f.sessions
            .replace_text(&s1, Revision::OPENED, stored("8000.000001")),
    );
    assert_eq!(replaced.session.unwrap().revision, second);
    let lateness = Lateness::start();
    let mut slowest = Duration::ZERO;
    let deadline = Instant::now() + Duration::from_secs(120);
    // Revision 2's model, published.
    loop {
        if let Ok(published) = published_timed(&f, &s1, &mut slowest).result {
            assert_eq!(published.revision, second);
            break;
        }
        assert!(Instant::now() < deadline, "revision 2 was never published");
        std::thread::sleep(Duration::from_millis(1));
    }
    // Its check, the scan its own: the block reached, so its drift isn't said.
    let mut check = f.sessions.check_model(&s1, ModelChoice::Published);
    loop {
        assert!(published_timed(&f, &s1, &mut slowest).result.is_ok());
        match check.wait_timeout(Duration::from_millis(1)) {
            Ok(checked) => {
                assert_eq!(checked.model.unwrap().revision, second);
                let problems = checked.result.unwrap();
                assert!(problems.is_empty(), "{:?}", problems.errors());
                break;
            }
            Err(running) => check = running,
        }
        assert!(Instant::now() < deadline, "the check never answered");
    }
    // At once: a second, and ten times the machine's lateness on a loaded one.
    let most = allowance(Duration::from_secs(1), lateness.most());
    assert!(
        slowest < most,
        "a published read took {slowest:?}, past {most:?}"
    );
}
