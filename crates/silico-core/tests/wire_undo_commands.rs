//! M5.2's `undo` and `redo` on the wire (docs/design/m5.0-app.md, section 7), and `status`'s
//! transaction counts (its section 6). The design's examples of them, byte for byte: each
//! file the design's own block (and the `to` request its prose gives); each request read,
//! written back to it, and held to its snapshot, every field it gives required and any other
//! refused; and each result and refusal the session's own, made by running the command from
//! the request's text on the fake kernel, inside a transaction begun at revision 12 with
//! three edits (13, 14, and 15), as the design sets them up, written to its file and held to
//! its snapshot or the error contract. `redo`'s examples, which the design gives in prose,
//! are written here by hand from it. Then the counts `status` gives through edits, undos,
//! and redos, each worked out by hand from A2's rule (the file's bytes, then for each state
//! but the session's its list, 24 and 8 a chunk, and each chunk it doesn't share, 16 and its
//! features' texts); `changed` false after editing back to `begin`'s document (40, 41, 40)
//! and after undoing back to it; and a dialog's run of edits undone as one, `to` the
//! revision it opened at. The bound's drop is counted in the crate's own tests
//! (`command/session/tests/undo.rs`), which can bound a session's stack.

use std::path::PathBuf;
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::wire::transaction::StepRequest;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{
    Problems, Response, Revision, SessionId, Sessions, Stepped, Transaction,
};
use silico_core::document::json::{Node, read};
use silico_core::fake::FakeKernel;
use tempfile::TempDir;

use crate::sessions::{answer, id};

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract")
        .join(name)
}

/// The committed file `name`, read and never written.
fn committed(name: &str) -> String {
    let path = contract(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The example `name` (`undo/request`), as committed.
fn example(name: &str) -> String {
    committed(&format!("commands/examples/{name}.json"))
}

/// The M5.0 design's text between the headings `from` and `to`.
fn design_section(from: &str, to: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m5.0-app.md");
    let doc = std::fs::read_to_string(&path).unwrap();
    doc[doc.find(from).unwrap()..doc.find(to).unwrap()].to_owned()
}

/// The JSON code blocks of `section`, in order.
fn blocks(section: &str) -> Vec<String> {
    section
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

const UNDO_SECTION: (&str, &str) = (
    "### `undo` and `redo` (M5.2; decision 2)",
    "### `drag_begin`, `drag_move`, `drag_end`",
);

/// `value` as the API writes it, and a newline, as each example file holds it.
fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

/// `value` as the API writes it on one line.
fn one_line<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::OneLine).unwrap()
}

fn parsed(text: &str) -> Node {
    read(text.as_bytes()).unwrap()
}

/// The snapshot `of`'s `part`, `request` or `result`, as committed.
fn schema_of(of: &str, part: &str) -> serde_json::Value {
    let schema: serde_json::Value =
        serde_json::from_str(&committed(&format!("commands/{of}.json"))).unwrap();
    schema[part].clone()
}

/// The error contract's shape, the envelope's `Error`.
fn error_schema() -> serde_json::Value {
    let envelope: serde_json::Value =
        serde_json::from_str(&committed("commands/envelope.json")).unwrap();
    serde_json::json!({"$defs": envelope["$defs"], "$ref": "#/$defs/Error"})
}

/// Panics, naming `what`, unless `text` holds to `schema`.
fn holds(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

/// Panics, naming `what`, if `text` holds to `schema`.
fn fails(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(!validator.is_valid(&value), "{what} held");
}

fn revision(n: u64) -> Revision {
    Revision::from_number(n).unwrap()
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

/// The one refusal of `response`, written.
fn the_refusal<T: std::fmt::Debug>(response: &Response<T>) -> String {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{:?}", problems.errors());
    written(&problems.errors()[0])
}

/// The block's line in canonical form, `length` long: as a chunk holds it, with its four
/// spaces of indent.
fn block_line(length: &str) -> String {
    format!(
        r#"    {{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": {length}, "width": 20, "height": 10}}"#
    )
}

/// A 40 × 20 × 10 block `length` long in canonical form, written by hand from M2.1's
/// layout: its one feature on one line (it holds no object).
fn block(length: &str) -> String {
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n{}\n  ]\n}}\n",
        block_line(length)
    )
}

/// What A2 counts for a state of [`block`] held in chunks, `length` long: its list (24, and
/// 8 for its one chunk) and its chunk (16, and the block's line).
fn state(length: &str) -> usize {
    24 + 8 + 16 + block_line(length).len()
}

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
}

/// Sessions on the fake kernel, with [`block`] 40 long in `block.silico` open as s1.
fn opened() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let sessions = Sessions::with_cache_opener(Arc::new(FakeKernel::new(2).unwrap()), move |c| {
        Cache::with_secret(c, &secret)
    });
    let path = dir.path().join("block.silico");
    std::fs::write(&path, block("40")).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    assert_eq!(id, s1());
    Fixture { dir, sessions, id }
}

impl Fixture {
    fn revision(&self) -> Revision {
        self.sessions.status(&self.id).session.unwrap().revision
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("block.silico")).unwrap()
    }

    fn text(&self) -> String {
        answer(self.sessions.text(&self.id, None))
            .result
            .unwrap()
            .text
    }

    fn begin(&self) {
        answer(self.sessions.begin(&self.id)).result.unwrap();
    }

    /// Begun and rolled back until the session is at revision `n`, then begun: the design's
    /// examples are of a transaction begun at revision 12.
    fn inside_at(&self, n: u64) {
        while self.revision() < revision(n) {
            self.begin();
            answer(self.sessions.rollback(&self.id)).result.unwrap();
        }
        assert_eq!(self.revision(), revision(n));
        self.begin();
    }

    /// The block's length set to `length`, at the session's revision: an edit, at a new one.
    fn set(&self, length: &str) {
        let before = self.revision();
        let set = answer(self.sessions.set_field(
            &self.id,
            before,
            id("k7f2qx3m"),
            "length".to_owned(),
            parsed(length),
        ));
        assert!(set.result.unwrap().changed);
        assert_eq!(self.revision(), before.next().unwrap());
    }

    /// The request `text` run as `undo`.
    fn undo(&self, text: &str) -> Response<Stepped> {
        let r = StepRequest::read(&parsed(text)).unwrap();
        answer(self.sessions.undo(&r.session, r.revision, r.to))
    }

    /// The request `text` run as `redo`.
    fn redo(&self, text: &str) -> Response<Stepped> {
        let r = StepRequest::read(&parsed(text)).unwrap();
        answer(self.sessions.redo(&r.session, r.revision, r.to))
    }

    /// A one-step `undo` (`way` "undo") or `redo` at the session's revision.
    fn step(&self, way: &str) -> Stepped {
        let text = format!(
            r#"{{"session": "s1", "revision": {}, "to": null}}"#,
            self.revision().number()
        );
        let stepped = match way {
            "undo" => self.undo(&text),
            _ => self.redo(&text),
        };
        stepped.result.unwrap()
    }

    /// The transaction `status` gives.
    fn transaction(&self) -> Option<Transaction> {
        self.sessions.status(&self.id).result.unwrap().transaction
    }
}

/// The design's three edits inside a transaction begun at 12: lengths 41, 42, and 43, at
/// revisions 13, 14, and 15.
fn three_edits() -> Fixture {
    let f = opened();
    f.inside_at(12);
    for length in ["41", "42", "43"] {
        f.set(length);
    }
    assert_eq!(f.revision(), revision(15));
    f
}

/// Each file of the design's section, in order.
const UNDO_FILES: [&str; 5] = [
    "request",
    "result",
    "refused-no-transaction",
    "refused-nothing",
    "refused-not-held",
];

/// `redo`'s examples, by hand from the design's prose: after the design's undo (revision 16,
/// 14's state), a redo restores 15's, three edits to undo and none to redo; with nothing to
/// redo it's refused "likewise", its cause "an edit after an undo drops what was undone".
const REDO_REQUEST: &str = r#"{"session": "s1", "revision": 16, "to": null}"#;
const REDO_RESULT: &str = r#"{"restored": 15, "undo": 3, "redo": 0, "dropped": 0}"#;
const REDO_NOTHING: &str = r#"{"code": "invalid_input", "message": "nothing to redo in session s1's transaction", "feature": null, "elements": [], "elements_total": 0, "parameter": "session", "cause": "an edit after an undo drops what was undone", "suggestion": "roll back to return to the state the transaction began from", "candidates": []}"#;

#[test]
fn the_examples_are_the_designs_own_text_and_redos_by_hand_from_its_words() {
    let section = design_section(UNDO_SECTION.0, UNDO_SECTION.1);
    let found = blocks(&section);
    assert_eq!(found.len(), UNDO_FILES.len());
    for (block, name) in found.iter().zip(UNDO_FILES) {
        assert_eq!(
            format!("{block}\n"),
            example(&format!("undo/{name}")),
            "{name}"
        );
    }
    // A dialog's Cancel, in the design's prose.
    let to = r#"{"session": "s1", "revision": 16, "to": 13}"#;
    assert!(section.contains(&format!("`{to}`")));
    assert_eq!(example("undo/request-to"), format!("{to}\n"));
    // Redo's "likewise": the design's words.
    assert!(section.contains(
        "`redo` with nothing to redo is refused likewise, its cause \"an edit after an undo drops what was undone\""
    ));
    for (name, text) in [
        ("request", REDO_REQUEST),
        ("result", REDO_RESULT),
        ("refused-nothing", REDO_NOTHING),
    ] {
        assert_eq!(
            example(&format!("redo/{name}")),
            format!("{text}\n"),
            "{name}"
        );
    }
    // Each directory holds these files and no other.
    for (dir, want) in [
        (
            "undo",
            &[
                "refused-no-transaction",
                "refused-not-held",
                "refused-nothing",
                "request-to",
                "request",
                "result",
            ][..],
        ),
        ("redo", &["refused-nothing", "request", "result"][..]),
    ] {
        let mut files: Vec<String> =
            std::fs::read_dir(contract(&format!("commands/examples/{dir}")))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
        files.sort();
        let want: Vec<String> = want.iter().map(|n| format!("{n}.json")).collect();
        assert_eq!(files, want, "{dir}");
    }
}

/// Each refusal's parameter and message, in order.
fn said(problems: &Problems) -> Vec<(String, String)> {
    problems
        .errors()
        .iter()
        .map(|e| {
            assert_eq!(e.code(), ErrorCode::InvalidInput);
            (
                e.parameter().unwrap_or("").to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

/// `text`, an object's JSON, without its member `key`.
fn without(text: &str, key: &str) -> String {
    let mut object: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(
        object.as_object_mut().unwrap().remove(key).is_some(),
        "{key}"
    );
    object.to_string()
}

/// `text`, an object's JSON, with its member `key` set to `value`, JSON's text.
fn with(text: &str, key: &str, value: &str) -> String {
    let mut object: serde_json::Value = serde_json::from_str(text).unwrap();
    object[key] = serde_json::from_str(value).unwrap();
    object.to_string()
}

#[test]
fn the_requests_are_read_written_back_byte_for_byte_and_hold_to_their_schemas() {
    for (of, name, want) in [
        ("undo", "undo/request", (15, None)),
        ("undo", "undo/request-to", (16, Some(13))),
        ("redo", "redo/request", (16, None)),
    ] {
        let text = example(name);
        let request = StepRequest::read(&parsed(&text)).unwrap();
        assert_eq!(
            request,
            StepRequest {
                session: s1(),
                revision: revision(want.0),
                to: want.1.map(revision),
            },
            "{name}"
        );
        assert_eq!(written(&request), text, "{name}");
        holds(&schema_of(of, "request"), &text, name);
        // Each field is required, and no other is taken.
        for key in ["session", "revision", "to"] {
            let refused = StepRequest::read(&parsed(&without(&text, key))).unwrap_err();
            assert_eq!(
                said(&refused),
                [(key.to_owned(), format!("{key} is missing"))],
                "{name}"
            );
            fails(&schema_of(of, "request"), &without(&text, key), key);
        }
        let extra = with(&text, "extra", "1");
        assert_eq!(
            said(&StepRequest::read(&parsed(&extra)).unwrap_err()),
            [(
                "extra".to_owned(),
                "extra isn't a field of this request".to_owned()
            )]
        );
        fails(&schema_of(of, "request"), &extra, "an extra key");
        // `to` is a revision, from 1, or null: never another value, in the words a revision
        // is refused in, and refused by the schema too.
        for (value, got) in [
            ("0", "0"),
            ("-1", "-1"),
            ("13.5", "13.5"),
            (r#""13""#, r#""13""#),
            ("[]", "an array"),
        ] {
            let bad = with(&text, "to", value);
            assert_eq!(
                said(&StepRequest::read(&parsed(&bad)).unwrap_err()),
                [(
                    "to".to_owned(),
                    format!(
                        "to must be a whole number from 1 to 18446744073709551615 or null, got {got}"
                    )
                )],
                "{value}"
            );
            fails(&schema_of(of, "request"), &bad, value);
        }
        // One past the largest revision is refused (the message shows it as the reader
        // shows a number that long); the largest is one.
        let past = with(&text, "to", "18446744073709551615")
            .replace("18446744073709551615", "18446744073709551616");
        let refused = StepRequest::read(&parsed(&past)).unwrap_err();
        assert_eq!(said(&refused)[0].0, "to");
        assert!(
            said(&refused)[0].1.starts_with(
                "to must be a whole number from 1 to 18446744073709551615 or null, got "
            ),
            "{refused:?}"
        );
        // The largest revision is one.
        let last = with(&text, "to", "18446744073709551615");
        assert_eq!(
            StepRequest::read(&parsed(&last)).unwrap().to,
            Revision::from_number(u64::MAX)
        );
    }
}

#[test]
fn each_result_and_refusal_is_the_sessions_own_as_the_design_sets_them_up() {
    let error = error_schema();
    let check = |name: &str, got: String| {
        assert_eq!(got, example(name), "{name}");
        holds(&error, &got, name);
    };
    // The design's: three edits after `begin` at 12; one undo restores 14's state, as 16.
    let f = three_edits();
    let undone = f.undo(&example("undo/request"));
    assert_eq!(undone.session.as_ref().unwrap().revision, revision(16));
    let text = written(undone.result.as_ref().unwrap());
    assert_eq!(text, example("undo/result"));
    holds(&schema_of("undo", "result"), &text, "undo's result");
    assert_eq!(f.text(), block("42"));
    // A redo restores 15's, as 17.
    let redone = f.redo(&example("redo/request"));
    assert_eq!(redone.session.as_ref().unwrap().revision, revision(17));
    let text = written(redone.result.as_ref().unwrap());
    assert_eq!(text, example("redo/result"));
    holds(&schema_of("redo", "result"), &text, "redo's result");
    assert_eq!(f.text(), block("43"));
    // Nothing more to redo: refused, the session as it was.
    let refused = f.redo(r#"{"session": "s1", "revision": 17, "to": null}"#);
    check("redo/refused-nothing", the_refusal(&refused));
    assert_eq!(refused.session.as_ref().unwrap().revision, revision(17));

    // At 16, after the one undo: a `to` from before the transaction, refused, the stack's
    // revisions its candidates; then a dialog's Cancel, `to` 13, restores 13's state.
    let f = three_edits();
    f.undo(&example("undo/request")).result.unwrap();
    let refused = f.undo(r#"{"session": "s1", "revision": 16, "to": 3}"#);
    check("undo/refused-not-held", the_refusal(&refused));
    assert_eq!(refused.session.as_ref().unwrap().revision, revision(16));
    let cancelled = f.undo(&example("undo/request-to"));
    assert_eq!(cancelled.session.as_ref().unwrap().revision, revision(17));
    assert_eq!(
        one_line(cancelled.result.as_ref().unwrap()),
        r#"{"restored": 13, "undo": 1, "redo": 2, "dropped": 0}"#
    );
    assert_eq!(f.text(), block("41"));
    // A redo `to` 15 (16's state was 14's, so 15 is the newest) redoes both.
    let redone = f.redo(r#"{"session": "s1", "revision": 17, "to": 15}"#);
    assert_eq!(
        one_line(redone.result.as_ref().unwrap()),
        r#"{"restored": 15, "undo": 3, "redo": 0, "dropped": 0}"#
    );
    assert_eq!(f.text(), block("43"));

    // Begun at 12 with no edit: nothing to undo.
    let f = opened();
    f.inside_at(12);
    let refused = f.undo(r#"{"session": "s1", "revision": 12, "to": null}"#);
    check("undo/refused-nothing", the_refusal(&refused));

    // No transaction open: undo's words, and redo's in its own name.
    let f = opened();
    let refused = f.undo(r#"{"session": "s1", "revision": 1, "to": null}"#);
    check("undo/refused-no-transaction", the_refusal(&refused));
    let refused = f.redo(r#"{"session": "s1", "revision": 1, "to": null}"#);
    assert_eq!(
        the_refusal(&refused),
        example("undo/refused-no-transaction")
            .replace("undo steps", "redo steps")
            .replace("to undo;", "to redo;")
    );

    // Against a revision the session has moved on from: a conflict, naming `revision`.
    let f = three_edits();
    for (way, refused) in [
        (
            "undo",
            f.undo(r#"{"session": "s1", "revision": 14, "to": null}"#),
        ),
        (
            "redo",
            f.redo(r#"{"session": "s1", "revision": 14, "to": null}"#),
        ),
    ] {
        let problems = refused.result.as_ref().unwrap_err();
        let e = &problems.errors()[0];
        assert_eq!(
            (e.code(), e.parameter()),
            (ErrorCode::Conflict, Some("revision")),
            "{way}"
        );
        holds(&error, &written(e), way);
    }
    assert_eq!(f.text(), block("43"));
    // A `to` on the wrong side, or the session's own revision, names `to`.
    for (way, to) in [("undo", 15), ("redo", 15), ("redo", 14)] {
        let text = format!(r#"{{"session": "s1", "revision": 15, "to": {to}}}"#);
        let refused = if way == "undo" {
            f.undo(&text)
        } else {
            f.redo(&text)
        };
        let problems = refused.result.as_ref().unwrap_err();
        assert_eq!(
            (
                problems.errors()[0].code(),
                problems.errors()[0].parameter()
            ),
            (ErrorCode::InvalidInput, Some("to")),
            "{way} to {to}"
        );
    }
    // None of it touched the file.
    assert_eq!(f.file(), block("40"));
}

/// A transaction's counts, as `status` gives them.
fn counts(base: u64, changed: bool, undo: usize, redo: usize, bytes: usize) -> Transaction {
    Transaction {
        base: revision(base),
        changed,
        undo,
        redo,
        dropped: 0,
        bytes,
    }
}

#[test]
fn status_counts_a_transactions_undo_and_redo_by_hand_and_changed_follows_its_document() {
    let f = opened();
    let file = block("40").len();
    assert_eq!(f.transaction(), None);
    f.begin();
    // `begin`'s state is the file's bytes, counted always.
    assert_eq!(f.transaction(), Some(counts(1, false, 0, 0, file)));
    // 40 to 41: one edit; the session's own state isn't counted.
    f.set("41");
    assert_eq!(f.transaction(), Some(counts(1, true, 1, 0, file)));
    // Back to 40 (the design's 40 → 41 → 40): the document is `begin`'s again, so nothing
    // has changed, though two edits can be undone; 41's state is held.
    f.set("40");
    assert_eq!(
        f.transaction(),
        Some(counts(1, false, 2, 0, file + state("41")))
    );
    // Undone, 41's: changed again; the second 40's state held for redo, 41's no longer.
    assert_eq!(f.step("undo").restored, revision(2));
    assert_eq!(
        f.transaction(),
        Some(counts(1, true, 1, 1, file + state("40")))
    );
    // Undone to `begin`'s: unchanged, both held for redo (`begin`'s bytes counted once).
    assert_eq!(f.step("undo").restored, revision(1));
    assert_eq!(f.text(), block("40"));
    assert_eq!(
        f.transaction(),
        Some(counts(1, false, 0, 2, file + state("40") + state("41")))
    );
    // Redone, 41's again.
    assert_eq!(f.step("redo").restored, revision(2));
    assert_eq!(
        f.transaction(),
        Some(counts(1, true, 1, 1, file + state("40")))
    );
    // An edit drops the redo (the second 40), and 41's state is held: 42 is the session's.
    f.set("42");
    assert_eq!(
        f.transaction(),
        Some(counts(1, true, 2, 0, file + state("41")))
    );
    // `status` writes them in the design's order (section 6), each from its own field.
    let status = f.sessions.status(&f.id).result.unwrap();
    let wrote: serde_json::Value = serde_json::from_str(&one_line(&status)).unwrap();
    assert_eq!(
        wrote["transaction"],
        serde_json::json!({"base": 1, "changed": true, "undo": 2, "redo": 0, "dropped": 0, "bytes": file + state("41")})
    );
    holds(&schema_of("status", "result"), &written(&status), "status");
    // A commit ends it: no transaction, and nothing to undo.
    answer(f.sessions.commit(&f.id)).result.unwrap();
    assert_eq!(f.transaction(), None);
    // Saved with the fingerprint of the block 42 long, by formula: 42 × 20 × 10 = 8400, area
    // 2(840 + 420 + 200) = 2920.
    assert_eq!(
        f.file(),
        "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 42,\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 8400, \"area\": 2920, \"bbox\": [0, 0, 0, 42, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n"
    );
}

#[test]
fn a_transaction_undone_back_to_begin_is_unchanged_and_its_commit_writes_nothing() {
    let f = opened();
    f.begin();
    f.set("41");
    f.set("42");
    f.step("undo");
    f.step("undo");
    assert!(!f.transaction().unwrap().changed);
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(!committed.written);
    assert_eq!(f.file(), block("40"));
}

#[test]
fn a_transaction_is_written_from_each_of_its_own_values() {
    // Every count apart, so none is written from another's field.
    let transaction = Transaction {
        base: revision(4),
        changed: false,
        undo: 5,
        redo: 6,
        dropped: 7,
        bytes: 8,
    };
    assert_eq!(
        one_line(&transaction),
        r#"{"base": 4, "changed": false, "undo": 5, "redo": 6, "dropped": 7, "bytes": 8}"#
    );
    let stepped = Stepped {
        restored: revision(9),
        undo: 10,
        redo: 11,
        dropped: 12,
    };
    let text = one_line(&stepped);
    assert_eq!(
        text,
        r#"{"restored": 9, "undo": 10, "redo": 11, "dropped": 12}"#
    );
    for of in ["undo", "redo"] {
        holds(&schema_of(of, "result"), &text, of);
        // A count is a whole number, never negative; the revision is from 1.
        for (from, to) in [
            ("\"undo\": 10", "\"undo\": -1"),
            ("\"dropped\": 12", "\"dropped\": 1.5"),
            ("\"restored\": 9", "\"restored\": 0"),
        ] {
            fails(&schema_of(of, "result"), &text.replace(from, to), to);
        }
    }
    // In a status, it holds; a count missing or negative doesn't.
    let status = format!(
        r#"{{"features": 1, "gated": true, "transaction": {}, "running": "undo", "reads": [], "rebuild": {{"revision": 9, "state": "none", "done": null, "steps": null}}}}"#,
        one_line(&transaction)
    );
    holds(&schema_of("status", "result"), &status, "a transaction");
    fails(
        &schema_of("status", "result"),
        &status.replace(", \"bytes\": 8", ""),
        "no bytes",
    );
    fails(
        &schema_of("status", "result"),
        &status.replace("\"redo\": 6", "\"redo\": -6"),
        "a negative redo",
    );
    // `running` takes `redo` too.
    holds(
        &schema_of("status", "result"),
        &status.replace("\"running\": \"undo\"", "\"running\": \"redo\""),
        "redo running",
    );
}

#[test]
fn a_dialogs_edits_are_undone_as_one_and_the_inline_edit_before_it_kept() {
    // The design's test plan: an inline edit of the length (revision 2), then a dialog
    // opened at 2 that changes it twice (3, 4), then its Cancel, `to` 2.
    let f = opened();
    f.begin();
    f.set("41");
    let opened_at = f.revision();
    f.set("42");
    f.set("43");
    let cancel = format!(
        r#"{{"session": "s1", "revision": {}, "to": {}}}"#,
        f.revision().number(),
        opened_at.number()
    );
    let stepped = f.undo(&cancel).result.unwrap();
    assert_eq!(
        one_line(&stepped),
        r#"{"restored": 2, "undo": 1, "redo": 2, "dropped": 0}"#
    );
    assert_eq!(f.text(), block("41"));
    assert!(f.transaction().unwrap().changed);
}
