//! Restore (Fable's decision of 2026-11-04, the three core gaps, section 2): `text` of a
//! revision the session holds (its revision now, the one its last reload replaced, inside a
//! transaction the one `begin` saw), then `begin` and `replace_text` of that text, an edit
//! like any other: `undo` takes it back, `commit` saves it, and another session holding the
//! file sees the save as any change outside it. Expected values: documents written by hand
//! in canonical form, their boxes' fingerprints by hand arithmetic (l × w × h, and
//! 2(lw + lh + wh)), the bytes on disk, and the revisions counted by hand.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::wire::Named;
use silico_core::command::{Command, Response, Revision, SessionId, Sessions, TextAnswer};
use silico_core::document::json;
use silico_core::fake::FakeKernel;
use tempfile::TempDir;

use crate::sessions::{answer, id, refusal};

/// A block, 40 × 20 × 10, in canonical form, with the fingerprint it rebuilds to: volume
/// 40 × 20 × 10 = 8000, area 2(800 + 400 + 200) = 2800, its centre its middle, (20, 10, 5),
/// to z = 10.
const ORIGINAL: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 6,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 8000, \"area\": 2800, \"center\": [20, 10, 5], \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n";

/// The agent's change: the block 11 high, in canonical form, with its fingerprint: volume
/// 40 × 20 × 11 = 8800, area 2(800 + 440 + 220) = 2920, its centre its middle, (20, 10,
/// 5.5), to z = 11.
const AGENTS: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 6,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 11,\n      \"fingerprint\": {\"volume\": 8800, \"area\": 2920, \"center\": [20, 10, 5.5], \"bbox\": [0, 0, 0, 40, 20, 11], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n";

/// The block's feature alone as canonical form writes it, `height` its height and the
/// fingerprint `fingerprint`: a member a line, too long for one line with its fingerprint.
fn block(height: u32, fingerprint: &str) -> String {
    format!(
        "{{\n  \"id\": \"k7f2qx3m\",\n  \"type\": \"box\",\n  \"v\": 1,\n  \"name\": \"Block\",\n  \"position\": [0, 0, 0],\n  \"length\": 40,\n  \"width\": 20,\n  \"height\": {height},\n  \"fingerprint\": {fingerprint}\n}}"
    )
}

const FINGERPRINT_10: &str = r#"{"volume": 8000, "area": 2800, "center": [20, 10, 5], "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}"#;
const FINGERPRINT_11: &str = r#"{"volume": 8800, "area": 2920, "center": [20, 10, 5.5], "bbox": [0, 0, 0, 40, 20, 11], "faces": 6, "edges": 12, "vertices": 8}"#;

/// Sessions on a fake kernel of their own, their cache sealed with `dir`'s secret: one
/// client's (the app's, or an agent's server's).
fn sessions(dir: &Path) -> Sessions<Arc<FakeKernel>> {
    let secret = dir.join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    Sessions::with_cache_opener(kernel, move |cache| Cache::with_secret(cache, &secret))
}

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(dir.path());
    Fixture { dir, sessions }
}

fn rev(n: u64) -> Revision {
    Revision::from_number(n).unwrap()
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("a.silico")
    }

    /// `text` written to the file, outside every session.
    fn write(&self, text: &str) {
        std::fs::write(self.path(), text).unwrap();
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.path()).unwrap()
    }

    /// The session on `text`, opened at revision 1.
    fn open(&self, text: &str) -> SessionId {
        self.write(text);
        let opened = answer(self.sessions.open(self.path(), false));
        assert_eq!(opened.session.as_ref().unwrap().revision, Revision::OPENED);
        opened.session.unwrap().id
    }

    fn revision(&self, s: &SessionId) -> Revision {
        self.sessions.status(s).session.unwrap().revision
    }

    /// `status` stepped until the session has reloaded past `from`: its new revision.
    #[track_caller]
    fn reloads(&self, s: &SessionId, from: Revision) -> Revision {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let revision = self.revision(s);
            if revision > from {
                return revision;
            }
            assert!(Instant::now() < deadline, "never reloaded past {from:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn text(
        &self,
        s: &SessionId,
        feature: Option<&str>,
        revision: Option<u64>,
    ) -> Response<TextAnswer> {
        answer(self.sessions.text_at(
            s,
            feature.map(id),
            revision.map(|n| Named::Revision(rev(n))),
        ))
    }

    /// `text` of the file on disk (`"disk"`), or its `feature`.
    fn disk_text(&self, s: &SessionId, feature: Option<&str>) -> Response<TextAnswer> {
        answer(self.sessions.text_at(s, feature.map(id), Some(Named::Disk)))
    }

    /// The text `text` gives, refused or not.
    #[track_caller]
    fn text_of(&self, s: &SessionId, feature: Option<&str>, revision: Option<u64>) -> String {
        self.text(s, feature, revision).result.unwrap().text
    }

    fn changed(&self, s: &SessionId) -> bool {
        self.sessions
            .status(s)
            .result
            .unwrap()
            .transaction
            .unwrap()
            .changed
    }

    /// `text` called by name, as MCP's server and the CLI call it: the envelope's text.
    fn called(&self, arguments: &str) -> String {
        self.sessions
            .call_command(Command::Text, arguments)
            .wait()
            .text
    }
}

/// The refusal of a revision not held: `invalid_input` naming `revision`, the held ones as
/// candidates.
#[track_caller]
fn not_held(response: &Response<TextAnswer>, asked: u64, candidates: &[&str]) {
    let (code, message, parameter, suggestion) = refusal(response);
    assert_eq!(code, ErrorCode::InvalidInput);
    assert_eq!(parameter.as_deref(), Some("revision"));
    let holds = match candidates {
        [one] => format!("revision {one}"),
        [first, second] => format!("revisions {first} and {second}"),
        more => panic!("{more:?}"),
    };
    let s = response.session.as_ref().unwrap().id.to_string();
    assert_eq!(
        message,
        format!("session {s} doesn't hold the text of revision {asked}: it holds {holds}")
    );
    assert_eq!(
        suggestion.as_deref(),
        Some("give revision a revision the session holds, or null for the document now")
    );
    let error = &response.result.as_ref().unwrap_err().errors()[0];
    assert_eq!(error.candidates(), candidates);
}

#[test]
fn text_of_the_revision_a_reload_replaced_is_the_original_bytes() {
    let f = fixture();
    let s = f.open(ORIGINAL);
    f.write(AGENTS);
    let two = f.reloads(&s, Revision::OPENED);
    assert_eq!(two, rev(2));
    // Revision 1: the bytes the reload replaced, exactly.
    let replaced = f.text(&s, None, Some(1));
    assert!(!replaced.session.as_ref().unwrap().reloaded);
    assert_eq!(replaced.session.as_ref().unwrap().revision, rev(2));
    assert_eq!(replaced.result.unwrap().text, ORIGINAL);
    // Now, by null and by its number: the agent's.
    assert_eq!(f.text_of(&s, None, None), AGENTS);
    assert_eq!(f.text_of(&s, None, Some(2)), AGENTS);
    // Its feature, at each: the block 10 high then, 11 now.
    assert_eq!(
        f.text_of(&s, Some("k7f2qx3m"), Some(1)),
        block(10, FINGERPRINT_10)
    );
    assert_eq!(
        f.text_of(&s, Some("k7f2qx3m"), None),
        block(11, FINGERPRINT_11)
    );
    // Called by name with the request's `revision`, the same text, in the core's envelope.
    let called = f.called(&format!(
        r#"{{"session": "{s}", "feature": null, "revision": 1}}"#
    ));
    let envelope: serde_json::Value = serde_json::from_str(&called).unwrap();
    assert_eq!(envelope["ok"], true, "{called}");
    assert_eq!(envelope["result"]["text"], ORIGINAL);
    assert_eq!(envelope["session"]["revision"], 2);
    let called = f.called(&format!(
        r#"{{"session": "{s}", "feature": "k7f2qx3m", "revision": 1}}"#
    ));
    let envelope: serde_json::Value = serde_json::from_str(&called).unwrap();
    assert_eq!(envelope["result"]["text"], block(10, FINGERPRINT_10));
}

#[test]
fn a_read_of_revisions_by_number_reads_nothing_of_the_file_and_null_reloads_it() {
    // Fable's 6d decision: a revision by number names a text the session holds, so its read
    // reads nothing of the file; only `null`, the document now, reads it first. No `status`
    // is asked, so the disk watch queues no reload of its own.
    let f = fixture();
    let s = f.open(ORIGINAL);
    f.write(AGENTS);
    let named = Named::Revision(Revision::OPENED);
    let one = Some(named);
    let same = answer(f.sessions.diff(&s, one, one));
    let session = same.session.unwrap();
    assert_eq!((session.revision, session.reloaded), (rev(1), false));
    let same = same.result.unwrap();
    assert_eq!((same.from, same.to), (named, named));
    assert_eq!(same.changes, []);
    let read = f.text(&s, None, Some(1));
    let session = read.session.unwrap();
    assert_eq!((session.revision, session.reloaded), (rev(1), false));
    assert_eq!(read.result.unwrap().text, ORIGINAL);
    // The document now reads the file first: reloaded, at revision 2, the agent's text.
    let now = f.text(&s, None, None);
    let session = now.session.unwrap();
    assert_eq!((session.revision, session.reloaded), (rev(2), true));
    assert_eq!(now.result.unwrap().text, AGENTS);
    // Revision 1 again: the text the reload replaced, read as it was.
    let replaced = f.text(&s, None, Some(1));
    let session = replaced.session.unwrap();
    assert_eq!((session.revision, session.reloaded), (rev(2), false));
    assert_eq!(replaced.result.unwrap().text, ORIGINAL);
    // The file gone: revisions by number answer from what's held; null is refused.
    std::fs::remove_file(f.path()).unwrap();
    assert_eq!(f.text_of(&s, None, Some(1)), ORIGINAL);
    assert_eq!(f.text_of(&s, None, Some(2)), AGENTS);
    let diffed = answer(f.sessions.diff(
        &s,
        Some(Named::Revision(rev(1))),
        Some(Named::Revision(rev(2))),
    ));
    // By hand: the height, 10 to 11, then the fingerprint's measures that follow it (the
    // volume, the area, the centre's z, 5 to 5.5, and the box's top), in the order the
    // document writes them.
    let fields: Vec<Option<String>> = diffed
        .result
        .unwrap()
        .changes
        .into_iter()
        .map(|c| c.field)
        .collect();
    assert_eq!(
        fields,
        [
            "height",
            "fingerprint.volume",
            "fingerprint.area",
            "fingerprint.center.2",
            "fingerprint.bbox.5"
        ]
        .map(|f| Some(f.to_owned()))
    );
    let gone = f.text(&s, None, None);
    let (code, _, parameter, _) = refusal(&gone);
    assert_eq!(code, ErrorCode::IoFailed);
    assert_eq!(parameter.as_deref(), Some("path"));
    assert_eq!(gone.session.unwrap().revision, rev(2));
}

#[test]
fn restored_by_begin_and_replace_text_undo_takes_it_back_and_commit_writes_the_original_bytes() {
    let f = fixture();
    let s = f.open(ORIGINAL);
    f.write(AGENTS);
    let two = f.reloads(&s, Revision::OPENED);
    let original = f.text_of(&s, None, Some(1));

    let begun = answer(f.sessions.begin(&s)).result.unwrap();
    assert_eq!(begun.base, two);
    assert!(!f.changed(&s));
    let replaced = answer(f.sessions.replace_text(&s, two, original.clone()));
    let three = replaced.session.as_ref().unwrap().revision;
    assert_eq!(three, rev(3));
    let replaced = replaced.result.unwrap();
    assert!(replaced.changed);
    // Inside a transaction, `rollback` restores what it replaced: no `before`, no commit.
    assert_eq!((replaced.before, replaced.commit), (None, None));
    assert!(f.changed(&s));
    assert_eq!(f.text_of(&s, None, None), ORIGINAL);
    // Nothing is written until the commit.
    assert_eq!(f.file(), AGENTS);

    // Undone: the agent's text, unchanged since `begin`.
    let undone = answer(f.sessions.undo(&s, three, None));
    assert_eq!(undone.result.unwrap().restored, two);
    assert!(!f.changed(&s));
    assert_eq!(f.text_of(&s, None, None), AGENTS);
    // Redone, then committed: the original bytes on disk.
    let four = f.revision(&s);
    let redone = answer(f.sessions.redo(&s, four, None));
    assert_eq!(redone.result.unwrap().restored, three);
    assert!(f.changed(&s));
    let committed = answer(f.sessions.commit(&s)).result.unwrap();
    assert!(committed.written);
    assert_eq!(committed.problems.total(), 0);
    assert_eq!(f.file(), ORIGINAL);
    assert_eq!(f.text_of(&s, None, None), ORIGINAL);
    // Saved by the session itself: its next command reloads nothing.
    let after = f.text(&s, None, None);
    assert!(!after.session.unwrap().reloaded);
}

#[test]
fn restored_then_undone_and_committed_writes_nothing() {
    let f = fixture();
    let s = f.open(ORIGINAL);
    f.write(AGENTS);
    let two = f.reloads(&s, Revision::OPENED);
    let original = f.text_of(&s, None, Some(1));
    answer(f.sessions.begin(&s)).result.unwrap();
    answer(f.sessions.replace_text(&s, two, original))
        .result
        .unwrap();
    answer(f.sessions.undo(&s, rev(3), None)).result.unwrap();
    let committed = answer(f.sessions.commit(&s)).result.unwrap();
    assert!(!committed.written);
    assert_eq!(f.file(), AGENTS);
}

/// The agent's session, on Sessions of its own (another process, as MCP's server is), on
/// the file `f` holds: opened at revision 1, its block raised to 11 by `set_field` outside
/// a transaction, which saves at once. The file is then [`AGENTS`], byte for byte.
fn agent_raises(f: &Fixture) -> (Sessions<Arc<FakeKernel>>, SessionId) {
    let agent = sessions(f.dir.path());
    let a = answer(agent.open(f.path(), false)).session.unwrap().id;
    let set = answer(agent.set_field(
        &a,
        Revision::OPENED,
        id("k7f2qx3m"),
        "height".to_owned(),
        json::read(b"11").unwrap(),
    ));
    assert!(set.result.unwrap().commit.unwrap().written);
    assert_eq!(f.file(), AGENTS);
    (agent, a)
}

/// The app's restore of revision 1 after the agent's save: reloaded, read, begun, replaced,
/// committed. The file is then [`ORIGINAL`] again.
fn app_restores(f: &Fixture, s: &SessionId) {
    let two = f.reloads(s, Revision::OPENED);
    let original = f.text_of(s, None, Some(1));
    assert_eq!(original, ORIGINAL);
    answer(f.sessions.begin(s)).result.unwrap();
    answer(f.sessions.replace_text(s, two, original))
        .result
        .unwrap();
    assert!(answer(f.sessions.commit(s)).result.unwrap().written);
    assert_eq!(f.file(), ORIGINAL);
}

#[test]
fn the_agent_holding_the_file_sees_the_restore_reloaded_on_its_next_command() {
    let f = fixture();
    let s = f.open(ORIGINAL);
    let (agent, a) = agent_raises(&f);
    app_restores(&f, &s);
    // The agent's next command reloads the app's save (its revision 2 the one it saved, 3
    // the one it reloads), and `text` of its revision 2 is its own change.
    let read = answer(agent.text_at(&a, None, None));
    let session = read.session.unwrap();
    assert!(session.reloaded);
    assert_eq!(session.revision, rev(3));
    assert_eq!(read.result.unwrap().text, ORIGINAL);
    let mine = answer(agent.text_at(&a, None, Some(Named::Revision(rev(2)))));
    assert_eq!(mine.result.unwrap().text, AGENTS);
}

#[test]
fn the_agent_inside_a_transaction_has_its_commit_refused_as_a_conflict() {
    let f = fixture();
    let s = f.open(ORIGINAL);
    let (agent, a) = agent_raises(&f);
    answer(agent.begin(&a)).result.unwrap();
    let set = answer(agent.set_field(
        &a,
        rev(2),
        id("k7f2qx3m"),
        "width".to_owned(),
        json::read(b"30").unwrap(),
    ));
    set.result.unwrap();
    app_restores(&f, &s);
    // Inside its transaction the agent reads no change; its commit is refused, and the
    // app's restore stays on disk.
    let committed = answer(agent.commit(&a));
    let (code, _, _, _) = refusal(&committed);
    assert_eq!(code, ErrorCode::Conflict);
    assert_eq!(f.file(), ORIGINAL);
}

#[test]
fn after_a_second_reload_the_first_revision_is_refused_with_the_two_held() {
    let f = fixture();
    let s = f.open(ORIGINAL);
    f.write(AGENTS);
    let two = f.reloads(&s, Revision::OPENED);
    f.write(&AGENTS.replace("\"width\": 20", "\"width\": 21"));
    assert_eq!(f.reloads(&s, two), rev(3));
    not_held(&f.text(&s, None, Some(1)), 1, &["2", "3"]);
    // A feature at it likewise: the revision is refused before the feature is looked for.
    not_held(&f.text(&s, Some("k7f2qx3m"), Some(1)), 1, &["2", "3"]);
    // Revision 2's text is now the one replaced.
    assert_eq!(f.text_of(&s, None, Some(2)), AGENTS);
    // Refused by name, as the wire writes the error.
    let called = f.called(&format!(
        r#"{{"session": "{s}", "feature": null, "revision": 1}}"#
    ));
    let envelope: serde_json::Value = serde_json::from_str(&called).unwrap();
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["errors"][0]["parameter"], "revision");
    assert_eq!(
        envelope["errors"][0]["candidates"],
        serde_json::json!(["2", "3"])
    );
}

#[test]
fn an_untitled_session_and_one_never_reloaded_refuse_every_revision_but_their_own() {
    let f = fixture();
    let untitled = answer(f.sessions.open_untitled()).session.unwrap().id;
    let s = f.open(ORIGINAL);
    for session in [&untitled, &s] {
        let own = f.text_of(session, None, Some(1));
        assert_eq!(own, f.text_of(session, None, None));
        for asked in [2, 7, u64::MAX] {
            not_held(&f.text(session, None, Some(asked)), asked, &["1"]);
        }
    }
    assert_eq!(f.text_of(&s, None, Some(1)), ORIGINAL);
    // The revision before is never held: nothing replaced it.
    let edited = answer(f.sessions.set_field(
        &s,
        Revision::OPENED,
        id("k7f2qx3m"),
        "height".to_owned(),
        json::read(b"11").unwrap(),
    ));
    assert_eq!(edited.session.unwrap().revision, rev(2));
    assert_eq!(f.text_of(&s, None, Some(2)), AGENTS);
    not_held(&f.text(&s, None, Some(1)), 1, &["2"]);
}

#[test]
fn inside_a_transaction_its_base_gives_begins_text() {
    let f = fixture();
    // Not canonical: `begin`'s text is the file's bytes, given back as they are.
    let one_line = ORIGINAL.replace("\n", "").replace("  ", "");
    let s = f.open(&one_line);
    answer(f.sessions.begin(&s)).result.unwrap();
    let set = answer(f.sessions.set_field(
        &s,
        Revision::OPENED,
        id("k7f2qx3m"),
        "height".to_owned(),
        json::read(b"11").unwrap(),
    ));
    assert_eq!(set.session.unwrap().revision, rev(2));
    assert_eq!(f.text_of(&s, None, Some(1)), one_line);
    // Its feature as `begin`'s document has it; now's, as the edit left it (the stored
    // fingerprint carried until the commit writes it anew).
    assert_eq!(
        f.text_of(&s, Some("k7f2qx3m"), Some(1)),
        block(10, FINGERPRINT_10)
    );
    assert_eq!(
        f.text_of(&s, Some("k7f2qx3m"), None),
        block(11, FINGERPRINT_10)
    );
    // A revision the transaction passed through isn't held: its base and now are.
    answer(f.sessions.set_field(
        &s,
        rev(2),
        id("k7f2qx3m"),
        "width".to_owned(),
        json::read(b"30").unwrap(),
    ))
    .result
    .unwrap();
    not_held(&f.text(&s, None, Some(2)), 2, &["1", "3"]);
    // Rolled back: the base is the revision now, held as it, the file's bytes.
    let rolled = answer(f.sessions.rollback(&s));
    let now = rolled.session.unwrap().revision;
    assert_eq!(f.text_of(&s, None, Some(now.number())), one_line);
    not_held(&f.text(&s, None, Some(1)), 1, &[&now.number().to_string()]);
    // Begun again, edited, and committed: the base, held while the transaction was open,
    // isn't after its commit, which leaves the session at a revision past it.
    answer(f.sessions.begin(&s)).result.unwrap();
    let base = now;
    let edited = answer(f.sessions.set_field(
        &s,
        base,
        id("k7f2qx3m"),
        "height".to_owned(),
        json::read(b"12").unwrap(),
    ));
    assert_eq!(edited.session.unwrap().revision.number(), base.number() + 1);
    assert_eq!(f.text_of(&s, None, Some(base.number())), one_line);
    let committed = answer(f.sessions.commit(&s));
    committed.result.unwrap();
    let after = committed.session.unwrap().revision;
    assert!(after > base, "{after:?} {base:?}");
    not_held(
        &f.text(&s, None, Some(base.number())),
        base.number(),
        &[&after.number().to_string()],
    );
}

#[test]
fn a_feature_the_revision_hasnt_is_refused_naming_feature() {
    let f = fixture();
    let s = f.open(ORIGINAL);
    // The agent's text adds a cylinder.
    let pin = r#"{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12}"#;
    let with_pin = AGENTS.replace(
        "\"vertices\": 8}\n    }\n  ]",
        &format!("\"vertices\": 8}}\n    }},\n    {pin}\n  ]"),
    );
    assert_ne!(with_pin, AGENTS);
    f.write(&with_pin);
    f.reloads(&s, Revision::OPENED);
    assert_eq!(f.text_of(&s, Some("a91cw0de"), None), pin);
    // Not in revision 1's document: the refusal says the revision is why, and where the
    // feature is.
    let (code, message, parameter, suggestion) = refusal(&f.text(&s, Some("a91cw0de"), Some(1)));
    assert_eq!(code, ErrorCode::InvalidInput);
    assert_eq!(message, "revision 1's document has no feature a91cw0de");
    assert_eq!(parameter.as_deref(), Some("feature"));
    assert_eq!(
        suggestion.as_deref(),
        Some(
            "name a feature revision 1's document has, or give revision null for the document now"
        )
    );
    // Not in the document now, its revision named or not: the document's refusal, as
    // `text` gives it with no revision.
    let now = f.revision(&s).number();
    for revision in [None, Some(now)] {
        let (code, message, parameter, suggestion) =
            refusal(&f.text(&s, Some("b0000000"), revision));
        assert_eq!(code, ErrorCode::InvalidInput);
        assert_eq!(message, "the document has no feature b0000000");
        assert_eq!(parameter.as_deref(), Some("feature"));
        assert_eq!(
            suggestion.as_deref(),
            Some("name a feature by its ID, as the document's outline lists them")
        );
    }
}

/// The agent's change written by hand: on one line, the block's keys in another order than
/// canonical form's (`type` before `id`, `height` before `position`), the reader taking any.
const AGENTS_BY_HAND: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"type": "box", "id": "k7f2qx3m", "v": 1, "name": "Block", "height": 11, "position": [0, 0, 0], "length": 40, "width": 20, "fingerprint": {"volume": 8800, "area": 2920, "center": [20, 10, 5.5], "bbox": [0, 0, 0, 40, 20, 11], "faces": 6, "edges": 12, "vertices": 8}}]}"#;

#[test]
fn text_of_disk_inside_a_transaction_is_the_files_bytes_exactly_and_its_features_canonical() {
    let f = fixture();
    let s = f.open(ORIGINAL);
    answer(f.sessions.begin(&s)).result.unwrap();
    f.write(AGENTS_BY_HAND);
    // The file's bytes exactly, not canonical form; nothing reloaded, nothing moved.
    let read = f.disk_text(&s, None);
    let session = read.session.unwrap();
    assert_eq!((session.revision, session.reloaded), (rev(1), false));
    assert_eq!(read.result.unwrap().text, AGENTS_BY_HAND);
    assert_eq!(f.text_of(&s, None, None), ORIGINAL);
    assert!(!f.changed(&s));
    // A feature: its canonical text, as the file's document has it.
    assert_eq!(
        f.disk_text(&s, Some("k7f2qx3m")).result.unwrap().text,
        block(11, FINGERPRINT_11)
    );
    // A feature the file hasn't: refused naming `feature`, saying it's the file's.
    let (code, message, parameter, suggestion) = refusal(&f.disk_text(&s, Some("a91cw0de")));
    assert_eq!(code, ErrorCode::InvalidInput);
    assert_eq!(message, "the file on disk has no feature a91cw0de");
    assert_eq!(parameter.as_deref(), Some("feature"));
    assert_eq!(
        suggestion.as_deref(),
        Some("name a feature the file on disk has, or give revision null for the document now")
    );
    // By name, as MCP's server calls it: the same bytes in the core's envelope.
    let called = f.called(&format!(
        r#"{{"session": "{s}", "feature": null, "revision": "disk"}}"#
    ));
    let envelope: serde_json::Value = serde_json::from_str(&called).unwrap();
    assert_eq!(envelope["ok"], true, "{called}");
    assert_eq!(envelope["result"]["text"], AGENTS_BY_HAND);
    assert_eq!(envelope["session"]["revision"], 1);
    // Taken into the transaction ("take theirs, keep my undo"): the document equals the
    // file, but the file isn't the transaction's own, so its commit is a conflict, loud.
    answer(
        f.sessions
            .replace_text(&s, rev(1), AGENTS_BY_HAND.to_owned()),
    )
    .result
    .unwrap();
    let committed = answer(f.sessions.commit(&s));
    let (code, _, parameter, _) = refusal(&committed);
    assert_eq!(code, ErrorCode::Conflict);
    assert_eq!(parameter.as_deref(), Some("path"));
    assert_eq!(f.file(), AGENTS_BY_HAND);
}

#[test]
fn text_of_disk_thats_not_utf8_is_refused_naming_revision() {
    let f = fixture();
    let s = f.open(ORIGINAL);
    answer(f.sessions.begin(&s)).result.unwrap();
    // By hand: `{"silico": 1, ` is 14 bytes, so the 0xff is at byte 14.
    std::fs::write(f.path(), b"{\"silico\": 1, \xff}").unwrap();
    let (code, message, parameter, suggestion) = refusal(&f.disk_text(&s, None));
    assert_eq!(code, ErrorCode::InvalidInput);
    assert_eq!(
        message,
        "the file on disk isn't UTF-8 (at byte 14), so it can't be given as text"
    );
    assert_eq!(parameter.as_deref(), Some("revision"));
    assert_eq!(
        suggestion.as_deref(),
        Some("mend the file: a document is UTF-8 text; or give revision null for the document now")
    );
    // The document now is the session's still.
    assert_eq!(f.text_of(&s, None, None), ORIGINAL);
}
