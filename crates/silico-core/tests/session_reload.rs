//! M4.2: a file changed outside the session (docs/design/m4.1-command-api.md, section 4's
//! "A file changed outside the session", sections 2, 3 and 7, decisions 2, 12, 21 and 22).
//! Before each queued command the session reads its file and compares its bytes with those it
//! last read or wrote: outside a transaction a change is reloaded at a new revision
//! (`reloaded: true`), and a file gone, unreadable, or another session's refuses the
//! command; inside one only `commit` reads the file, a change its conflict; `close`,
//! `status`, and published reads never read the file.
//! Expected values are the design's words, documents written by hand and their bytes, and
//! boxes' fingerprints by hand arithmetic (l × w × h, and 2(lw + lh + wh)).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{
    Begun, CheckModel, Closed, ModelChoice, ReadCommand, Response, Revision, SessionId, Sessions,
    TextAnswer,
};
use silico_core::document::json::Node;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Op, Primitive};
use tempfile::TempDir;

use crate::sessions::{HANG, answer, id, refusal};

/// A block, 40 × 20 × 10, its fingerprint stored as the fake measures it (volume 8000,
/// area 2(800 + 400 + 200) = 2800), on one line: not canonical form.
const BLOCK: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}]}"#;

/// [`BLOCK`] 41 long, as a hand edit leaves it: its fingerprint as stored, and the file the
/// same size.
fn longer() -> String {
    let longer = BLOCK.replace(r#""length": 40"#, r#""length": 41"#);
    assert_ne!(longer, BLOCK);
    assert_eq!(longer.len(), BLOCK.len());
    longer
}

/// [`BLOCK`] 11 high, as another hand edit leaves it.
fn higher() -> String {
    let higher = BLOCK.replace(r#""height": 10"#, r#""height": 11"#);
    assert_ne!(higher, BLOCK);
    higher
}

/// [`higher`] in canonical form, by hand, with the fingerprint it rebuilds to: 40 × 20 × 11
/// = 8800, area 2(800 + 440 + 220) = 2920, to z = 11.
const HIGHER_FORMATTED: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 11,\n      \"fingerprint\": {\"volume\": 8800, \"area\": 2920, \"bbox\": [0, 0, 0, 40, 20, 11], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n";

struct Fixture {
    _dir: TempDir,
    kernel: Arc<FakeKernel>,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
    path: PathBuf,
}

/// Sessions on a fake kernel, with `text` in `a.silico` open as s1.
fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = Sessions::with_cache_opener(Arc::clone(&kernel), move |cache| {
        Cache::with_secret(cache, &secret)
    });
    let path = dir.path().join("a.silico");
    std::fs::write(&path, text).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    Fixture {
        _dir: dir,
        kernel,
        sessions,
        id,
        path,
    }
}

impl Fixture {
    fn file(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }

    /// The file written outside the session.
    fn write(&self, text: &str) {
        std::fs::write(&self.path, text).unwrap();
    }

    fn text(&self) -> Response<TextAnswer> {
        answer(self.sessions.text(&self.id, None))
    }

    /// The session's revision and whether it reloaded, as `response` gives them.
    fn after<T>(response: &Response<T>) -> (u64, bool) {
        let session = response.session.as_ref().unwrap();
        (session.revision.number(), session.reloaded)
    }

    fn set_length(
        &self,
        revision: Revision,
        length: &str,
    ) -> Response<silico_core::command::FieldSet> {
        answer(self.sessions.set_field(
            &self.id,
            revision,
            id("k7f2qx3m"),
            "length".to_owned(),
            value(length),
        ))
    }

    fn boxes(&self) -> usize {
        self.kernel
            .calls()
            .iter()
            .filter(|call| is_box(&call.op))
            .count()
    }
}

fn value(text: &str) -> Node {
    silico_core::document::json::read(text.as_bytes()).unwrap()
}

fn is_box(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
}

/// A path as the session's messages quote it.
fn quoted(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// Why a command was refused over a file the session couldn't take as it is now (gone,
/// unreadable, too long): the design's words (section 4).
const REREAD: &str =
    "the session reads its file before each command to find changes made outside it";

/// What to do about a file at `path` the session couldn't take as it is now.
fn put_back(path: &Path) -> String {
    format!(
        "put the file back at {}, or close the session and open the file where it is now",
        quoted(path)
    )
}

/// The refusal of a command outside a transaction whose session's file at `path` is gone.
fn gone(path: &Path) -> (ErrorCode, String, Option<String>, Option<String>) {
    (
        ErrorCode::IoFailed,
        format!(
            "{} couldn't be read: No such file or directory (os error 2)",
            quoted(path)
        ),
        Some("path".to_owned()),
        Some(put_back(path)),
    )
}

/// The conflict of a transaction's commit, begun from revision `base`, over the file at
/// `path`: the design's words (section 7's commit).
fn commit_conflict(
    path: &Path,
    base: u64,
) -> (
    ErrorCode,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    (
        ErrorCode::Conflict,
        format!(
            "{} changed outside this session since it was read",
            quoted(path)
        ),
        Some("path".to_owned()),
        Some(format!(
            "it changed after the transaction began, from revision {base}, and a transaction doesn't reload its file"
        )),
        Some(
            "nothing was written: roll back, then begin again from the file as it is now"
                .to_owned(),
        ),
    )
}

/// The refusal of a command whose session found at `path` a file session s2 holds.
fn held_by_s2(
    path: &Path,
) -> (
    ErrorCode,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    (
        ErrorCode::InvalidInput,
        format!("{} is already open in session s2", quoted(path)),
        Some("path".to_owned()),
        Some(REREAD.to_owned()),
        Some(format!(
            "put this session's file back at {}: the file there now is session s2's",
            quoted(path)
        )),
    )
}

/// `response`'s one error: its code, message, parameter, cause, and suggestion.
fn the_error<T: std::fmt::Debug>(
    response: &Response<T>,
) -> (
    ErrorCode,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    let error = &problems.errors()[0];
    (
        error.code(),
        error.message().to_owned(),
        error.parameter().map(str::to_owned),
        error.cause().map(str::to_owned),
        error.suggestion().map(str::to_owned),
    )
}

#[test]
fn a_change_keeping_the_files_size_and_time_is_reloaded_before_the_next_command() {
    let f = opened(BLOCK);
    let before = std::fs::metadata(&f.path).unwrap();
    // Changed in place, then its modification and access times put back: only its bytes
    // say it changed.
    f.write(&longer());
    std::fs::File::options()
        .write(true)
        .open(&f.path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(before.modified().unwrap())
                .set_accessed(before.accessed().unwrap()),
        )
        .unwrap();
    let after = std::fs::metadata(&f.path).unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    // Published reads never read the file. (`status` compares it, off its caller's thread,
    // and the session then reloads it itself: M5.0's A3, the disk tests'.)
    assert_eq!(Fixture::after(&f.sessions.published(&f.id)), (1, false));
    let read = answer(f.sessions.read_model(
        &f.id,
        ReadCommand::Check,
        ModelChoice::Published,
        |_| Ok(()),
    ));
    assert_eq!(Fixture::after(&read), (1, false));
    // The next queued command reloads it: a new revision, whose model is the file's.
    let check = answer(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false }),
    );
    assert_eq!(Fixture::after(&check), (2, true));
    assert_eq!(check.model.unwrap().revision.number(), 2);
    // A hand edit reaches nothing (section 5): its stale fingerprint is drift, 41 × 20 × 10
    // = 8000 → 8200, area 2800 → 2(820 + 410 + 200) = 2860.
    let problems = check.result.unwrap().problems;
    assert_eq!(problems.total(), 1, "{problems:?}");
    let drift = &problems.errors()[0];
    assert_eq!(
        (drift.code(), drift.feature(), drift.parameter()),
        (
            ErrorCode::Drift,
            Some("k7f2qx3m"),
            Some("fingerprint.volume")
        )
    );
    assert!(
        drift.message().starts_with(
            "feature k7f2qx3m rebuilt to other measures than its fingerprint: volume 8000 → 8200, area 2800 → 2860"
        ),
        "{drift:?}"
    );
    // Once: the file is what the session now holds.
    let text = f.text();
    assert_eq!(Fixture::after(&text), (2, false));
    assert_eq!(text.result.unwrap().text, longer());
}

#[test]
fn a_reloaded_document_the_gate_refuses_is_held_with_its_problems() {
    let f = opened(BLOCK);
    let flat = BLOCK.replace(r#""height": 10"#, r#""height": 0"#);
    f.write(&flat);
    let check = answer(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false }),
    );
    assert_eq!(Fixture::after(&check), (2, true));
    assert_eq!(check.model, None);
    let problems = check.result.unwrap().problems;
    assert_eq!(problems.total(), 1, "{problems:?}");
    assert_eq!(
        (
            problems.errors()[0].feature(),
            problems.errors()[0].parameter()
        ),
        (Some("k7f2qx3m"), Some("height"))
    );
    assert!(!f.sessions.status(&f.id).result.unwrap().gated);
    assert_eq!(f.text().result.unwrap().text, flat);
}

#[test]
fn inside_a_transaction_a_change_isnt_reloaded_and_the_commit_is_a_conflict() {
    let f = opened(BLOCK);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    f.set_length(Revision::OPENED, "41").result.unwrap();
    f.write(&higher());
    // The transaction keeps its state: its edit, at revision 2, unreloaded.
    let text = f.text();
    assert_eq!(Fixture::after(&text), (2, false));
    assert!(text.result.unwrap().text.contains("\"length\": 41,"));
    let commit = answer(f.sessions.commit(&f.id));
    assert_eq!(Fixture::after(&commit), (2, false));
    assert!(commit.session.as_ref().unwrap().transaction);
    let problems = commit.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    let conflict = &problems.errors()[0];
    assert_eq!(
        (
            conflict.code(),
            conflict.message(),
            conflict.parameter(),
            conflict.cause(),
            conflict.suggestion()
        ),
        (
            ErrorCode::Conflict,
            format!(
                "{} changed outside this session since it was read",
                quoted(&f.path)
            )
            .as_str(),
            Some("path"),
            Some(
                "it changed after the transaction began, from revision 1, and a transaction doesn't reload its file"
            ),
            // The design's words (section 7's commit).
            Some("nothing was written: roll back, then begin again from the file as it is now")
        )
    );
    // Found before the commit's rebuild: nothing built, nothing written.
    assert_eq!(f.boxes(), 0);
    assert_eq!(f.file(), higher());
    // As the advice says: rolled back (still inside, so not reloaded), then begun again
    // from the file as it is now, which begin reloads. 1 at open, 2 the edit, 3 the
    // rollback, 4 the reload.
    let rolled = answer(f.sessions.rollback(&f.id));
    assert_eq!(Fixture::after(&rolled), (3, false));
    let begun = answer(f.sessions.begin(&f.id));
    assert_eq!(Fixture::after(&begun), (4, true));
    assert_eq!(begun.result.unwrap().base.number(), 4);
    assert_eq!(f.text().result.unwrap().text, higher());
}

#[test]
fn an_edit_made_before_a_reload_is_a_conflict_saying_the_file_was_reloaded() {
    let f = opened(BLOCK);
    f.write(&higher());
    let stale = f.set_length(Revision::OPENED, "41");
    assert_eq!(Fixture::after(&stale), (2, true));
    let problems = stale.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1);
    let conflict = &problems.errors()[0];
    // The design's add_feature example, word for word but the path and the revisions.
    assert_eq!(
        (
            conflict.code(),
            conflict.message(),
            conflict.parameter(),
            conflict.cause(),
            conflict.suggestion()
        ),
        (
            ErrorCode::Conflict,
            "the edit was made against revision 1, and the session is at revision 2",
            Some("revision"),
            Some(
                format!(
                    "{} was reloaded: it changed outside this session",
                    quoted(&f.path)
                )
                .as_str()
            ),
            Some(
                "read the document again (text or outline), then make the edit against revision 2"
            )
        )
    );
    assert_eq!(f.file(), higher());
    // Made against the reloaded revision, it's saved, the outside change kept.
    let set = f.set_length(Revision::OPENED.next().unwrap(), "41");
    assert_eq!(Fixture::after(&set), (3, false));
    assert!(set.result.unwrap().commit.unwrap().written);
    assert!(f.file().contains("\"height\": 11,"));
    assert!(f.file().contains("\"length\": 41,"));
    // Revision 1 is still older than the reload: said so again.
    assert!(
        f.set_length(Revision::OPENED, "42")
            .result
            .unwrap_err()
            .errors()[0]
            .cause()
            .is_some()
    );
    // Revision 2, the reload's own, is stale only because of the session's own save.
    let after_save = f.set_length(Revision::OPENED.next().unwrap(), "42");
    let error = &after_save.result.as_ref().unwrap_err().errors()[0];
    assert_eq!(
        (error.code(), error.cause()),
        (ErrorCode::Conflict, None),
        "{error:?}"
    );
}

#[test]
fn a_file_changed_while_a_one_edit_commit_rebuilds_is_a_conflict_and_then_reloaded() {
    let f = opened(BLOCK);
    let hold = f.kernel.hold(is_box);
    let running = f.sessions.set_field(
        &f.id,
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        value("41"),
    );
    assert!(f.kernel.wait_running(1, HANG));
    f.write(&higher());
    hold.release();
    let set = answer(running);
    // Refused whole: the session as it was, nothing written, and no model read.
    assert_eq!(Fixture::after(&set), (1, false));
    assert_eq!(set.model, None);
    let (code, message, parameter, suggestion) = refusal(&set);
    assert_eq!(
        (code, message, parameter, suggestion),
        (
            ErrorCode::Conflict,
            format!(
                "{} changed outside this session since it was read",
                quoted(&f.path)
            ),
            Some("path".to_owned()),
            Some(
                "nothing was written: read it again and make the edit against the new revision"
                    .to_owned()
            )
        )
    );
    assert_eq!(f.file(), higher());
    // Read again, it's the new revision, the file's.
    let text = f.text();
    assert_eq!(Fixture::after(&text), (2, true));
    assert_eq!(text.result.unwrap().text, higher());
}

#[test]
fn a_file_changed_while_format_rebuilds_is_a_conflict_and_format_run_again_writes_it() {
    let f = opened(BLOCK);
    let hold = f.kernel.hold(is_box);
    let running = f.sessions.format(&f.id, None, true);
    assert!(f.kernel.wait_running(1, HANG));
    f.write(&higher());
    hold.release();
    let format = answer(running);
    assert_eq!(Fixture::after(&format), (1, false));
    let (code, _, parameter, suggestion) = refusal(&format);
    assert_eq!(
        (code, parameter, suggestion),
        (
            ErrorCode::Conflict,
            Some("path".to_owned()),
            Some("nothing was written: run format again, on the file as it is now".to_owned())
        )
    );
    assert_eq!(f.file(), higher());
    let again = answer(f.sessions.format(&f.id, None, true));
    // Reloaded at 2, written at 3.
    assert_eq!(Fixture::after(&again), (3, true));
    let reformatted = again.result.unwrap();
    assert!(reformatted.written);
    assert_eq!(reformatted.accepted, [id("k7f2qx3m")]);
    assert_eq!(f.file(), HIGHER_FORMATTED);
}

#[test]
fn a_file_thats_gone_refuses_each_command_but_status_and_close_leaving_the_session_as_it_was() {
    let f = opened(BLOCK);
    std::fs::remove_file(&f.path).unwrap();
    let gone = gone(&f.path);
    let text = f.text();
    assert_eq!(refusal(&text), gone);
    assert_eq!(
        text.result.as_ref().unwrap_err().errors()[0].cause(),
        Some(REREAD)
    );
    assert_eq!(Fixture::after(&text), (1, false));
    assert_eq!(
        refusal(&answer(
            f.sessions
                .check(&f.id, CheckModel::Current { fresh: false })
        )),
        gone
    );
    assert_eq!(refusal(&answer(f.sessions.begin(&f.id))), gone);
    assert_eq!(
        f.set_length(Revision::OPENED, "41")
            .result
            .unwrap_err()
            .errors()[0]
            .code(),
        ErrorCode::IoFailed
    );
    assert_eq!(f.boxes(), 0);
    let status = f.sessions.status(&f.id);
    assert_eq!(Fixture::after(&status), (1, false));
    assert_eq!(status.result.unwrap().transaction, None);
    // Put back as it was: nothing changed, so nothing reloads.
    f.write(BLOCK);
    let text = f.text();
    assert_eq!(Fixture::after(&text), (1, false));
    assert_eq!(text.result.unwrap().text, BLOCK);
    // Gone again, the session still closes.
    std::fs::remove_file(&f.path).unwrap();
    assert_eq!(answer(f.sessions.close(&f.id)).result, Ok(Closed));
}

#[test]
fn a_file_grown_past_what_silico_reads_refuses_the_command_and_isnt_held() {
    let f = opened(BLOCK);
    // MAX_INPUT_LEN, 4 × 16 MiB, and one more byte.
    f.write(&" ".repeat(4 * 16 * 1024 * 1024 + 1));
    let check = answer(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false }),
    );
    assert_eq!(Fixture::after(&check), (1, false));
    assert_eq!(
        refusal(&check),
        (
            ErrorCode::InvalidInput,
            format!(
                "{} is more than the 67108864 bytes Silico reads of a document",
                quoted(&f.path)
            ),
            Some("path".to_owned()),
            Some(put_back(&f.path))
        )
    );
    assert_eq!(
        check.result.as_ref().unwrap_err().errors()[0].cause(),
        Some(REREAD)
    );
    f.write(BLOCK);
    assert_eq!(f.text().result.unwrap().text, BLOCK);
}

#[test]
fn status_gives_the_open_transactions_base() {
    let f = opened(BLOCK);
    let status = |f: &Fixture| f.sessions.status(&f.id).result.unwrap().transaction;
    assert_eq!(status(&f), None);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    assert_eq!(
        status(&f),
        Some(Begun {
            base: Revision::OPENED
        })
    );
    // An edit moves the session on, not the transaction's base.
    f.set_length(Revision::OPENED, "41").result.unwrap();
    assert_eq!(
        f.sessions.status(&f.id).session.unwrap().revision.number(),
        2
    );
    assert_eq!(
        status(&f),
        Some(Begun {
            base: Revision::OPENED
        })
    );
    answer(f.sessions.rollback(&f.id)).result.unwrap();
    assert_eq!(status(&f), None);
    // Begun again at revision 3, the rollback's.
    answer(f.sessions.begin(&f.id)).result.unwrap();
    assert_eq!(status(&f).map(|begun| begun.base.number()), Some(3));
}

#[test]
fn a_reloaded_file_put_at_its_path_by_rename_is_claimed_by_its_session() {
    let f = opened(BLOCK);
    let dir = f.path.parent().unwrap();
    let copy = dir.join(".a.silico.new");
    std::fs::write(&copy, higher()).unwrap();
    std::fs::rename(&copy, &f.path).unwrap();
    assert_eq!(Fixture::after(&f.text()), (2, true));
    // Another name for the file now at the path: still s1's.
    let link = dir.join("c.silico");
    std::fs::hard_link(&f.path, &link).unwrap();
    let refused = answer(f.sessions.open(&link, false));
    assert_eq!(
        refusal(&refused).1,
        format!("{} is already open in session s1", quoted(&link))
    );
}

#[test]
fn a_commit_that_writes_nothing_over_a_file_changed_outside_ends_and_the_next_command_reloads() {
    let f = opened(BLOCK);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    f.write(&higher());
    // The transaction's document is the one begin saw: nothing to write, nothing lost, and
    // no rebuild waited for.
    let commit = answer(f.sessions.commit(&f.id));
    assert!(!commit.result.unwrap().written);
    assert_eq!(f.boxes(), 0);
    let session = commit.session.unwrap();
    assert_eq!(
        (
            session.revision.number(),
            session.transaction,
            session.reloaded
        ),
        (1, false, false)
    );
    assert_eq!(f.file(), higher());
    let text = f.text();
    assert_eq!(Fixture::after(&text), (2, true));
    assert_eq!(text.result.unwrap().text, higher());
}

#[test]
fn inside_a_transaction_a_gone_file_is_only_the_commits_conflict() {
    let f = opened(BLOCK);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    f.set_length(Revision::OPENED, "41").result.unwrap();
    std::fs::remove_file(&f.path).unwrap();
    // Every command but commit runs on what the session holds: the file isn't read.
    let text = f.text();
    assert_eq!(Fixture::after(&text), (2, false));
    assert!(text.result.unwrap().text.contains("\"length\": 41,"));
    assert!(answer(f.sessions.outline(&f.id)).result.is_ok());
    let check = answer(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false }),
    );
    assert_eq!(Fixture::after(&check), (2, false));
    assert!(check.result.is_ok(), "{:?}", check.result);
    let set = f.set_length(Revision::OPENED.next().unwrap(), "42");
    assert_eq!(Fixture::after(&set), (3, false));
    // The commit's conflict, naming the file.
    let commit = answer(f.sessions.commit(&f.id));
    assert_eq!(the_error(&commit), commit_conflict(&f.path, 1));
    assert!(commit.session.unwrap().transaction);
    assert!(!f.path.exists());
    // Rolled back, as the advice says; then, outside the transaction, the file's gone.
    let rolled = answer(f.sessions.rollback(&f.id));
    assert_eq!(Fixture::after(&rolled), (4, false));
    assert_eq!(refusal(&answer(f.sessions.begin(&f.id))), gone(&f.path));
    assert_eq!(answer(f.sessions.close(&f.id)).result, Ok(Closed));
}

#[test]
fn inside_a_transaction_an_unreadable_file_is_the_commits_io_failed() {
    let f = opened(BLOCK);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    f.set_length(Revision::OPENED, "41").result.unwrap();
    // A directory where the file was: there, but no regular file.
    std::fs::remove_file(&f.path).unwrap();
    std::fs::create_dir(&f.path).unwrap();
    assert_eq!(Fixture::after(&f.text()), (2, false));
    let commit = answer(f.sessions.commit(&f.id));
    assert_eq!(
        the_error(&commit),
        (
            ErrorCode::IoFailed,
            format!("{} couldn't be read: it isn't a regular file", quoted(&f.path)),
            Some("path".to_owned()),
            None,
            Some(
                "nothing was written: make it a regular file this user may read again, then commit; or roll back"
                    .to_owned()
            ),
        )
    );
    assert!(commit.session.unwrap().transaction);
    assert_eq!(f.boxes(), 0);
    assert!(f.path.is_dir());
}

#[test]
fn a_commit_that_writes_nothing_over_a_gone_file_ends() {
    let f = opened(BLOCK);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    std::fs::remove_file(&f.path).unwrap();
    let commit = answer(f.sessions.commit(&f.id));
    assert!(!commit.result.unwrap().written);
    assert!(!commit.session.unwrap().transaction);
    assert_eq!(f.boxes(), 0);
    assert!(!f.path.exists());
}

#[test]
fn close_never_reads_the_file() {
    let f = opened(BLOCK);
    f.write(&higher());
    let close = answer(f.sessions.close(&f.id));
    assert_eq!(close.result, Ok(Closed));
    // Not reloaded first: the session closed at the revision it was at.
    assert_eq!(Fixture::after(&close), (1, false));
    assert_eq!(f.file(), higher());
}

/// s1 on `a.silico` (the fixture's) and s2 on `b.silico`, holding [`higher`]; then
/// `a.silico` replaced by `link` made to `b.silico`: s1's next command is refused, naming
/// s2, as `open` refuses; s2's claim and s1's state are as they were.
fn a_link_to_another_sessions_file_is_refused_naming_it(link: fn(&Path, &Path)) {
    let f = opened(BLOCK);
    let dir = f.path.parent().unwrap().to_owned();
    let other = dir.join("b.silico");
    std::fs::write(&other, higher()).unwrap();
    let s2 = answer(f.sessions.open(&other, false)).session.unwrap().id;
    assert_eq!(s2.to_string(), "s2");
    std::fs::remove_file(&f.path).unwrap();
    link(&other, &f.path);
    assert_eq!(f.file(), higher());
    let text = f.text();
    assert_eq!(the_error(&text), held_by_s2(&f.path));
    assert_eq!(Fixture::after(&text), (1, false));
    // s2's claim intact: another name for its file is still s2's.
    let third = dir.join("c.silico");
    std::fs::hard_link(&other, &third).unwrap();
    assert_eq!(
        refusal(&answer(f.sessions.open(&third, false))).1,
        format!("{} is already open in session s2", quoted(&third))
    );
    let s2_text = answer(f.sessions.text(&s2, None));
    assert_eq!(Fixture::after(&s2_text), (1, false));
    assert_eq!(s2_text.result.unwrap().text, higher());
    // s1 as it was: its file put back, nothing changed, so nothing reloads.
    std::fs::remove_file(&f.path).unwrap();
    f.write(BLOCK);
    let text = f.text();
    assert_eq!(Fixture::after(&text), (1, false));
    assert_eq!(text.result.unwrap().text, BLOCK);
}

#[test]
fn a_symbolic_link_to_another_sessions_file_put_at_the_path_is_refused_naming_it() {
    a_link_to_another_sessions_file_is_refused_naming_it(|to, at| {
        std::os::unix::fs::symlink(to, at).unwrap();
    });
}

#[test]
fn a_hard_link_to_another_sessions_file_put_at_the_path_is_refused_naming_it() {
    a_link_to_another_sessions_file_is_refused_naming_it(|to, at| {
        std::fs::hard_link(to, at).unwrap();
    });
}

#[test]
fn a_transaction_begun_after_a_conflict_rolled_back_commits() {
    let f = opened(BLOCK);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    f.set_length(Revision::OPENED, "41").result.unwrap();
    f.write(&higher());
    assert_eq!(
        refusal(&answer(f.sessions.commit(&f.id))).0,
        ErrorCode::Conflict
    );
    answer(f.sessions.rollback(&f.id)).result.unwrap();
    // 1 at open, 2 the edit, 3 the rollback, 4 the reload.
    let begun = answer(f.sessions.begin(&f.id));
    assert_eq!(Fixture::after(&begun), (4, true));
    let base = begun.result.unwrap().base;
    f.set_length(base, "42").result.unwrap();
    let commit = answer(f.sessions.commit(&f.id));
    assert!(commit.result.unwrap().written);
    assert!(f.file().contains("\"height\": 11,"));
    assert!(f.file().contains("\"length\": 42,"));
}

#[test]
fn a_file_changed_while_a_transactions_commit_rebuilds_has_the_commits_advice() {
    let f = opened(BLOCK);
    // Saved outside a transaction: revision 2, the file the session's.
    f.set_length(Revision::OPENED, "41").result.unwrap();
    let base = answer(f.sessions.begin(&f.id)).result.unwrap().base;
    assert_eq!(base.number(), 2);
    f.set_length(base, "42").result.unwrap();
    let hold = f.kernel.hold(is_box);
    let running = f.sessions.commit(&f.id);
    assert!(f.kernel.wait_running(1, HANG));
    f.write(&higher());
    hold.release();
    let commit = answer(running);
    assert_eq!(Fixture::after(&commit), (3, false));
    assert!(commit.session.as_ref().unwrap().transaction);
    assert_eq!(the_error(&commit), commit_conflict(&f.path, 2));
    assert_eq!(f.file(), higher());
}

#[test]
fn inside_a_transaction_a_file_grown_too_long_is_a_change_like_any_other() {
    let f = opened(BLOCK);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    f.set_length(Revision::OPENED, "41").result.unwrap();
    // MAX_INPUT_LEN, 4 × 16 MiB, and one more byte.
    f.write(&" ".repeat(4 * 16 * 1024 * 1024 + 1));
    let text = f.text();
    assert_eq!(Fixture::after(&text), (2, false));
    assert!(text.result.unwrap().text.contains("\"length\": 41,"));
    let commit = answer(f.sessions.commit(&f.id));
    assert_eq!(the_error(&commit), commit_conflict(&f.path, 1));
    assert_eq!(f.boxes(), 0);
}

#[test]
fn a_file_of_exactly_the_bytes_silico_reads_is_reloaded() {
    let f = opened(BLOCK);
    // MAX_INPUT_LEN, 4 × 16 MiB, exactly: not a document, but not too long to hold.
    f.write(&" ".repeat(4 * 16 * 1024 * 1024));
    let check = answer(
        f.sessions
            .check(&f.id, CheckModel::Current { fresh: false }),
    );
    assert_eq!(Fixture::after(&check), (2, true));
    assert!(!f.sessions.status(&f.id).result.unwrap().gated);
}

#[test]
fn an_edit_older_than_the_latest_reload_says_it_was_reloaded() {
    let f = opened(BLOCK);
    f.write(&higher());
    assert_eq!(Fixture::after(&f.text()), (2, true));
    f.write(&longer());
    assert_eq!(Fixture::after(&f.text()), (3, true));
    // Made against revision 2, the first reload's: older than the latest, so stale because
    // the file changed.
    let stale = f.set_length(Revision::OPENED.next().unwrap(), "42");
    assert_eq!(Fixture::after(&stale), (3, false));
    let (code, message, parameter, cause, _) = the_error(&stale);
    assert_eq!(
        (code, message.as_str(), parameter.as_deref(), cause),
        (
            ErrorCode::Conflict,
            "the edit was made against revision 2, and the session is at revision 3",
            Some("revision"),
            Some(format!(
                "{} was reloaded: it changed outside this session",
                quoted(&f.path)
            ))
        )
    );
}

#[test]
fn a_link_to_another_sessions_file_saved_by_rename_is_refused_naming_it() {
    let f = opened(BLOCK);
    let dir = f.path.parent().unwrap().to_owned();
    let other = dir.join("b.silico");
    std::fs::write(&other, BLOCK).unwrap();
    let s2 = answer(f.sessions.open(&other, false)).session.unwrap().id;
    // b.silico saved by another editor, by rename: a new file at s2's path, which s2 hasn't
    // read (nor claimed) yet.
    let new = dir.join(".b.silico.new");
    std::fs::write(&new, higher()).unwrap();
    std::fs::rename(&new, &other).unwrap();
    std::fs::remove_file(&f.path).unwrap();
    std::os::unix::fs::symlink(&other, &f.path).unwrap();
    let text = f.text();
    assert_eq!(the_error(&text), held_by_s2(&f.path));
    assert_eq!(Fixture::after(&text), (1, false));
    // s2 isn't locked out of its own file: it reloads it.
    let s2_text = answer(f.sessions.text(&s2, None));
    assert_eq!(Fixture::after(&s2_text), (2, true));
    assert_eq!(s2_text.result.unwrap().text, higher());
}

/// A file with `BLOCK`'s bytes put at `path` by rename: another file, the same bytes.
fn put_unchanged_by_rename(path: &Path) {
    let new = path.with_file_name(".a.silico.new");
    std::fs::write(&new, BLOCK).unwrap();
    std::fs::rename(&new, path).unwrap();
}

/// After s1's file at `f.path` was put back unchanged by rename, its old file still linked
/// at `old`: the new file is s1's (another name for it is refused, naming s1), and the old
/// one let go (another name for it opens).
fn claimed_anew_and_the_old_let_go(f: &Fixture, old: &Path) {
    let link = f.path.with_file_name("c.silico");
    std::fs::hard_link(&f.path, &link).unwrap();
    assert_eq!(
        refusal(&answer(f.sessions.open(&link, false))).1,
        format!("{} is already open in session s1", quoted(&link))
    );
    let opened = answer(f.sessions.open(old, false));
    assert!(opened.result.is_ok(), "{:?}", opened.result);
}

#[test]
fn a_file_put_back_unchanged_is_claimed_and_the_one_it_replaced_let_go() {
    let f = opened(BLOCK);
    let old = f.path.with_file_name("old.silico");
    std::fs::hard_link(&f.path, &old).unwrap();
    put_unchanged_by_rename(&f.path);
    // The same bytes: nothing reloaded.
    assert_eq!(Fixture::after(&f.text()), (1, false));
    claimed_anew_and_the_old_let_go(&f, &old);
}

#[test]
fn a_file_put_back_unchanged_inside_a_transaction_is_claimed_by_its_commit() {
    let f = opened(BLOCK);
    answer(f.sessions.begin(&f.id)).result.unwrap();
    let old = f.path.with_file_name("old.silico");
    std::fs::hard_link(&f.path, &old).unwrap();
    put_unchanged_by_rename(&f.path);
    // A commit that writes nothing: only its read of the file claims the new one.
    let commit = answer(f.sessions.commit(&f.id));
    assert!(!commit.result.unwrap().written);
    claimed_anew_and_the_old_let_go(&f, &old);
}

#[test]
fn outside_a_transaction_an_unreadable_file_refuses_the_command_saying_how_to_go_on() {
    let f = opened(BLOCK);
    // A directory where the file was: there, but no regular file.
    std::fs::remove_file(&f.path).unwrap();
    std::fs::create_dir(&f.path).unwrap();
    let text = f.text();
    assert_eq!(
        the_error(&text),
        (
            ErrorCode::IoFailed,
            format!(
                "{} couldn't be read: it isn't a regular file",
                quoted(&f.path)
            ),
            Some("path".to_owned()),
            Some(REREAD.to_owned()),
            Some(format!(
                "make the file at {} a regular file this user may read, or close the session and open the file where it is now",
                quoted(&f.path)
            )),
        )
    );
    assert_eq!(Fixture::after(&text), (1, false));
}
