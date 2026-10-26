//! M4.2's edits (docs/design/m4.1-command-api.md, sections 2, 4, 5 and 7; decisions 1, 3,
//! 12, 13, 15, 20 and 21): `add_feature`, `set_field`, `remove_feature`, and
//! `replace_text`, each against a revision, inside a transaction only changing what the
//! session holds and outside one saved as its own transaction. Expected values are the
//! design's messages, documents written by hand and their exact bytes, and boxes'
//! fingerprints by hand arithmetic (l × w × h, and 2(lw + lh + wh)).

use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{ModelChoice, ModelRead, Response, Revision, SessionId, Sessions};
use silico_core::document::json::{Node, Plain};
use silico_core::document::{self, Length, Measure, Operation};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Op, Primitive};
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

use crate::sessions::{answer, id, refusal};

/// A block, 40 × 20 × 10, its fingerprint stored as the fake measures it (volume 8000,
/// area 2(800 + 400 + 200) = 2800), on one line: not canonical form.
const BLOCK: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}]}"#;

/// [`BLOCK`] 41 long, saved: in canonical form, by hand, its fingerprint the new box's:
/// 41 × 20 × 10 = 8200, area 2(820 + 410 + 200) = 2860, to x = 41.
const BLOCK_41: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 41,\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 8200, \"area\": 2860, \"bbox\": [0, 0, 0, 41, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n";

/// A block, a lug clear of it, and their fuse, none with a fingerprint.
const FUSED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10}, {"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "k7f2qx3m", "tool": "b0000001", "note": "Kept together"}]}"#;

/// A sketch whose lists aren't in ID order: its points pb (10, 0) then pa (0, 0), the
/// line na from pa to pb, and its constraints k0000002 (na 10 long) then k0000001 (na
/// horizontal).
const SKETCHED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [{"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "k0000002", "type": "length", "line": "na000000", "value": 10, "driving": true}, {"id": "k0000001", "type": "horizontal", "line": "na000000"}]}]}"#;

/// The design's `replace_text` example: a document of no features, in canonical form.
const EMPTY: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": []\n}\n";

struct Fixture {
    _dir: TempDir,
    kernel: Arc<FakeKernel>,
    /// What solves the sessions' sketches: each as sent, unless a test says otherwise.
    sketches: FakeSketches,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
    path: PathBuf,
}

/// Sessions on a fake kernel, their sketches solved by a fake, with `text` in `a.silico`
/// open as s1.
fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("a.silico");
    std::fs::write(&path, text).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    Fixture {
        _dir: dir,
        kernel,
        sketches,
        sessions,
        id,
        path,
    }
}

impl Fixture {
    fn file(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }

    fn revision(&self) -> Revision {
        self.sessions.status(&self.id).session.unwrap().revision
    }

    fn begin(&self) {
        answer(self.sessions.begin(&self.id)).result.unwrap();
    }

    fn text(&self) -> String {
        answer(self.sessions.text(&self.id, None))
            .result
            .unwrap()
            .text
    }

    fn set(
        &self,
        feature: &str,
        field: &str,
        to: &str,
    ) -> Response<silico_core::command::FieldSet> {
        answer(self.sessions.set_field(
            &self.id,
            self.revision(),
            id(feature),
            field.to_owned(),
            value(to),
        ))
    }

    /// How many boxes the kernel has been asked to make.
    fn boxes(&self) -> usize {
        self.kernel
            .calls()
            .iter()
            .filter(|call| matches!(call.op, Op::MakePrimitive(Primitive::Box { .. })))
            .count()
    }
}

/// The reader's tree of `text`, as a request's value is.
fn value(text: &str) -> Node {
    document::json::read(text.as_bytes()).unwrap()
}

/// `value` as compact JSON, its numbers as written, its members in order.
fn json(value: &Plain) -> String {
    match value {
        Plain::Null => "null".to_owned(),
        Plain::Bool(b) => b.to_string(),
        Plain::Number(text) => text.clone(),
        Plain::String(text) => serde_json::to_string(text).unwrap(),
        Plain::Array(items) => format!(
            "[{}]",
            items.iter().map(json).collect::<Vec<_>>().join(", ")
        ),
        Plain::Object(members) => format!(
            "{{{}}}",
            members
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    serde_json::to_string(key).unwrap(),
                    json(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The one error a refusal gives, whole.
fn the_error<T: std::fmt::Debug>(response: &Response<T>) -> silico_core::Error {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    problems.errors()[0].clone()
}

#[test]
fn a_field_set_outside_a_transaction_is_saved_with_the_fingerprints_it_reached() {
    let f = opened(BLOCK);
    let response = f.set("k7f2qx3m", "length", "41");
    let session = response.session.as_ref().unwrap();
    // Its own transaction: one revision on, none open.
    assert_eq!((session.revision.number(), session.transaction), (2, false));
    assert_eq!(
        response.model,
        Some(ModelRead {
            revision: session.revision,
            of: ModelChoice::Current,
            fault: None,
        })
    );
    let set = response.result.unwrap();
    assert_eq!(set.feature, id("k7f2qx3m"));
    assert_eq!(set.field, "length");
    assert_eq!(
        (json(&set.before), json(&set.after)),
        ("40".into(), "41".into())
    );
    assert!(set.changed);
    let commit = set.commit.unwrap();
    assert!(commit.written);
    assert_eq!(commit.path, f.path.to_str().unwrap());
    assert_eq!(commit.refingerprinted, [id("k7f2qx3m")]);
    assert!(commit.resolved.is_empty());
    assert!(commit.problems.is_empty(), "{:?}", commit.problems);
    assert_eq!(f.file(), BLOCK_41);
    assert_eq!(f.text(), BLOCK_41);
    // The saved revision's check finds no drift: its model is the edit's, rebound to the
    // saved document, never the edit's own, which holds the fingerprint as it was.
    let check = answer(f.sessions.check(&f.id, ModelChoice::Current));
    assert!(check.result.unwrap().problems.is_empty());
    assert_eq!(check.model.unwrap().revision.number(), 2);
}

#[test]
fn a_field_set_outside_a_transaction_publishes_its_model_over_an_older_one() {
    let f = opened(BLOCK);
    // Revision 1's model published first: the edit's must take its place.
    let check = answer(f.sessions.check(&f.id, ModelChoice::Current));
    assert!(check.result.unwrap().problems.is_empty());
    assert_eq!(
        f.sessions.published(&f.id).model.unwrap().revision,
        Revision::OPENED
    );
    f.set("k7f2qx3m", "length", "41").result.unwrap();
    let published = f.sessions.published(&f.id).result.unwrap();
    assert_eq!(published.revision.number(), 2);
    // The edit's model rebound to the saved document: its fingerprint the one written, by
    // hand 41 × 20 × 10 = 8200, area 2(820 + 410 + 200) = 2860, and measured so.
    let block = id("k7f2qx3m");
    let stored = published.model.document().features[0].fingerprint.unwrap();
    let measured = published.model.fingerprint(&block).unwrap();
    for fingerprint in [stored, measured] {
        assert_eq!(fingerprint.volume, Measure::from_millionths(8_200_000_000));
        assert_eq!(fingerprint.area, Measure::from_millionths(2_860_000_000));
    }
    // The saved revision's check takes it: nothing is rebuilt.
    let calls = f.kernel.calls().len();
    let check = answer(f.sessions.check(&f.id, ModelChoice::Current));
    assert!(check.result.unwrap().problems.is_empty());
    assert_eq!(check.model.unwrap().revision.number(), 2);
    assert_eq!(f.kernel.calls().len(), calls);
}

#[test]
fn a_field_set_to_the_value_it_has_changes_nothing() {
    let f = opened(BLOCK);
    let inode = std::fs::metadata(&f.path).unwrap().ino();
    // 40.0 is 40: the same length, read exactly.
    for to in ["40", "40.0"] {
        let response = f.set("k7f2qx3m", "length", to);
        assert_eq!(response.session.unwrap().revision, Revision::OPENED);
        assert_eq!(response.model, None);
        let set = response.result.unwrap();
        assert!(!set.changed);
        assert_eq!(set.commit, None);
        assert_eq!(
            (json(&set.before), json(&set.after)),
            ("40".into(), "40".into())
        );
    }
    assert_eq!(f.file(), BLOCK);
    assert_eq!(std::fs::metadata(&f.path).unwrap().ino(), inode);
    assert!(f.kernel.calls().is_empty());
}

#[test]
fn an_edit_against_another_revision_is_a_conflict() {
    let f = opened(BLOCK);
    f.begin();
    f.set("k7f2qx3m", "length", "41").result.unwrap();
    let stale = answer(f.sessions.set_field(
        &f.id,
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        value("42"),
    ));
    assert_eq!(
        refusal(&stale),
        (
            ErrorCode::Conflict,
            "the edit was made against revision 1, and the session is at revision 2".to_owned(),
            Some("revision".to_owned()),
            Some(
                "read the document again (text or outline), then make the edit against revision 2"
                    .to_owned()
            )
        )
    );
    // Every edit takes it: each refused the same way, the session left at 2.
    let removed = answer(
        f.sessions
            .remove_feature(&f.id, Revision::OPENED, id("k7f2qx3m")),
    );
    assert_eq!(refusal(&removed).0, ErrorCode::Conflict);
    let added = answer(f.sessions.add_feature(&f.id, Revision::OPENED, value("{}")));
    assert_eq!(refusal(&added).0, ErrorCode::Conflict);
    let replaced = answer(
        f.sessions
            .replace_text(&f.id, Revision::OPENED, EMPTY.to_owned()),
    );
    assert_eq!(refusal(&replaced).0, ErrorCode::Conflict);
    assert_eq!(f.revision().number(), 2);
    assert!(f.text().contains("\"length\": 41,"));
}

#[test]
fn inside_a_transaction_an_edit_changes_only_what_the_session_holds() {
    let f = opened(BLOCK);
    f.begin();
    let response = f.set("k7f2qx3m", "length", "41");
    assert_eq!(response.model, None);
    let set = response.result.unwrap();
    assert!(set.changed);
    assert_eq!(set.commit, None);
    assert_eq!(f.file(), BLOCK);
    // The session's text is the edited document in canonical form, its fingerprint as
    // stored until a commit writes it.
    assert_eq!(
        f.text(),
        BLOCK_41.replace(
            r#""volume": 8200, "area": 2860, "bbox": [0, 0, 0, 41, 20, 10]"#,
            r#""volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10]"#
        )
    );
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert_eq!(committed.refingerprinted, [id("k7f2qx3m")]);
    assert_eq!(f.file(), BLOCK_41);
}

#[test]
fn ten_edits_in_a_transaction_rebuild_nothing_and_the_first_check_rebuilds_once() {
    let f = opened(BLOCK);
    f.begin();
    for length in 41..=50 {
        assert!(
            f.set("k7f2qx3m", "length", &length.to_string())
                .result
                .unwrap()
                .changed
        );
    }
    assert!(f.kernel.calls().is_empty(), "{:?}", f.kernel.calls());
    let check = answer(f.sessions.check(&f.id, ModelChoice::Current));
    // Revision 1 at open, and one for each edit: the tenth is 11.
    assert_eq!(check.model.unwrap().revision.number(), 11);
    let boxes: Vec<f64> = f
        .kernel
        .calls()
        .iter()
        .filter_map(|call| match call.op {
            Op::MakePrimitive(Primitive::Box { dx, .. }) => Some(dx),
            _ => None,
        })
        .collect();
    assert_eq!(boxes, [50.0]);
}

#[test]
fn a_refused_commit_of_an_edit_outside_a_transaction_leaves_the_session_as_it_was() {
    let f = opened(BLOCK);
    // The file changed while the edit's commit rebuilds (a change before the command is
    // reloaded first, docs/design/m4.1-command-api.md decision 2): found by its save.
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let running = f.sessions.set_field(
        &f.id,
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        value("41"),
    );
    assert!(f.kernel.wait_running(1, crate::sessions::HANG));
    std::fs::write(&f.path, "edited elsewhere").unwrap();
    hold.release();
    let response = answer(running);
    let session = response.session.as_ref().unwrap();
    assert_eq!(
        (session.revision, session.transaction, session.reloaded),
        (Revision::OPENED, false, false)
    );
    // No model is read: the candidate's isn't the session's.
    assert_eq!(response.model, None);
    let error = the_error(&response);
    assert_eq!(
        (error.code(), error.parameter()),
        (ErrorCode::Conflict, Some("path"))
    );
    assert_eq!(f.file(), "edited elsewhere");
    assert_eq!(
        f.sessions.status(&f.id).session.unwrap().revision,
        Revision::OPENED
    );
    // And nothing of the edit is published: the session's published model is none yet
    // (the candidate's isn't), and the next command reloads the file.
    assert!(f.sessions.published(&f.id).result.is_err());
    let text = answer(f.sessions.text(&f.id, None));
    assert!(text.session.unwrap().reloaded);
    assert_eq!(text.result.unwrap().text, "edited elsewhere");
}

#[test]
fn a_field_is_read_exactly_as_the_document_reader_reads_it() {
    let f = opened(BLOCK);
    f.begin();
    let set = f.set("k7f2qx3m", "length", "0.1").result.unwrap();
    assert_eq!(json(&set.after), "0.1");
    let held = document::read(f.text().as_bytes()).unwrap();
    let Operation::Box { length, .. } = held.features[0].operation else {
        panic!("not a box");
    };
    assert_eq!(length, Length::from_nm(100_000));
    // A seventh decimal is refused as the document refuses it, naming the field.
    let response = f.set("k7f2qx3m", "length", "0.0000001");
    let error = the_error(&response);
    assert_eq!(
        (error.code(), error.feature(), error.parameter()),
        (ErrorCode::InvalidInput, Some("k7f2qx3m"), Some("length"))
    );
    assert!(
        error.message().starts_with("length 0.0000001 "),
        "{error:?}"
    );
    assert_eq!(response.session.unwrap().revision.number(), 2);
    // A length the gate refuses: refused whole, the session as it was.
    let response = f.set("k7f2qx3m", "length", "0");
    let error = the_error(&response);
    assert_eq!(
        (error.feature(), error.parameter()),
        (Some("k7f2qx3m"), Some("length"))
    );
    assert_eq!(response.session.unwrap().revision.number(), 2);
    // Any other array by place, from 0.
    let set = f.set("k7f2qx3m", "position.2", "5").result.unwrap();
    assert_eq!(
        (json(&set.before), json(&set.after)),
        ("0".into(), "5".into())
    );
    assert!(f.text().contains("\"position\": [0, 0, 5],"));
}

#[test]
fn a_feature_s_id_type_version_and_fingerprint_cant_be_set() {
    let f = opened(BLOCK);
    for (field, message, suggestion) in [
        (
            "type",
            "a feature's type can't be set",
            "add a feature of the type you want, and remove this one",
        ),
        (
            "id",
            "a feature's id can't be set",
            "add a feature with the ID you want, and remove this one",
        ),
        (
            "v",
            "a feature's version can't be set",
            "leave v as it is: a feature's version changes only by an upgrade",
        ),
        (
            "fingerprint",
            "a feature's fingerprint can't be set: a commit writes it",
            "leave fingerprint to the commit; format accepts a fingerprint's drift by name",
        ),
        (
            "fingerprint.volume",
            "a feature's fingerprint can't be set: a commit writes it",
            "leave fingerprint to the commit; format accepts a fingerprint's drift by name",
        ),
    ] {
        let response = f.set("k7f2qx3m", field, "1");
        let error = the_error(&response);
        assert_eq!(
            (
                error.code(),
                error.message(),
                error.feature(),
                error.parameter(),
                error.suggestion()
            ),
            (
                ErrorCode::InvalidInput,
                message,
                Some("k7f2qx3m"),
                Some("field"),
                Some(suggestion)
            ),
            "{field}"
        );
    }
    assert_eq!(f.file(), BLOCK);
    assert_eq!(f.revision(), Revision::OPENED);
}

#[test]
fn a_field_a_feature_hasnt_is_refused_with_every_field_it_has() {
    let f = opened(BLOCK);
    // A place is digits only: Rust's `usize` parse takes a leading `+`, which would make
    // `position.+1` the box's y.
    for field in [
        "depth",
        "position.3",
        "position.01",
        "position.+1",
        "position.-0",
        "position. 1",
        "length.x",
        "",
    ] {
        let error = the_error(&f.set("k7f2qx3m", field, "1"));
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{field}");
        assert_eq!(error.parameter(), Some("field"), "{field}");
        assert_eq!(error.suggestion(), Some("name one of the box's fields"));
        assert_eq!(
            error.candidates(),
            ["name", "note", "position", "length", "width", "height"],
            "{field}"
        );
    }
    assert_eq!(
        the_error(&f.set("k7f2qx3m", "depth", "1")).message(),
        "a box has no field depth"
    );
    // A plain path is shown as written up to 80 characters (two names' worth), and quoted,
    // cut to its first 40, past that.
    let at_most =
        "counterbore.depth_under_the_mounting_face.of_the_block_as_drawn_on_the_sheet_abc";
    let past = "counterbore.depth_under_the_mounting_face.of_the_block_as_drawn_on_the_sheet_abcd";
    assert_eq!((at_most.len(), past.len()), (80, 81));
    assert_eq!(
        the_error(&f.set("k7f2qx3m", at_most, "1")).message(),
        "a box has no field counterbore.depth_under_the_mounting_face.of_the_block_as_drawn_on_the_sheet_abc"
    );
    assert_eq!(
        the_error(&f.set("k7f2qx3m", past, "1")).message(),
        "a box has no field \"counterbore.depth_under_the_mounting_fac\"…"
    );
    assert_eq!(f.file(), BLOCK);
    assert_eq!(f.revision(), Revision::OPENED);
    // The design's pad, which has no base here: a base can be given it.
    let pad = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000003", "type": "sketch", "v": 1, "name": "Profile", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}, {"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000003", "extent": {"type": "one_side", "length": 6, "reversed": false}}]}"#;
    let f = opened(pad);
    let error = the_error(&f.set("p0000001", "extent.depth", "1"));
    assert_eq!(error.message(), "a pad has no field extent.depth");
    assert_eq!(error.feature(), Some("p0000001"));
    assert_eq!(
        error.candidates(),
        [
            "name",
            "note",
            "base",
            "sketch",
            "extent",
            "extent.type",
            "extent.length",
            "extent.reversed"
        ]
    );
    // A feature the document hasn't.
    assert_eq!(
        refusal(&f.set("z0000009", "name", "\"Other\"")),
        (
            ErrorCode::InvalidInput,
            "the document has no feature z0000009".to_owned(),
            Some("feature".to_owned()),
            Some("name a feature by its ID, as the document holds it".to_owned())
        )
    );
}

#[test]
fn a_note_is_added_set_and_removed_by_null_and_reaches_nothing() {
    let f = opened(BLOCK);
    let set = f.set("k7f2qx3m", "note", "\"Mount face\"").result.unwrap();
    assert_eq!(
        (json(&set.before), json(&set.after)),
        ("null".into(), "\"Mount face\"".into())
    );
    // A note reaches nothing: its commit writes no fingerprint.
    assert!(set.commit.unwrap().refingerprinted.is_empty());
    assert!(f.file().contains("      \"note\": \"Mount face\",\n"));
    let removed = f.set("k7f2qx3m", "note", "null").result.unwrap();
    assert_eq!(
        (json(&removed.before), json(&removed.after)),
        ("\"Mount face\"".into(), "null".into())
    );
    assert!(!f.file().contains("note"));
    // Removing a note the feature hasn't changes nothing.
    let again = f.set("k7f2qx3m", "note", "null").result.unwrap();
    assert!(!again.changed);
    assert_eq!(f.revision().number(), 3);
}

#[test]
fn a_sketch_member_is_set_by_its_id_and_refused_by_the_same_path() {
    let f = opened(SKETCHED);
    f.begin();
    let set = f
        .set("s0000001", "constraints.k0000002.value", "12")
        .result
        .unwrap();
    assert_eq!(
        (json(&set.before), json(&set.after)),
        ("10".into(), "12".into())
    );
    let error = the_error(&f.set("s0000001", "constraints.k0000002.value", "0"));
    assert_eq!(
        (
            error.code(),
            error.feature(),
            error.parameter(),
            error.elements()
        ),
        (
            ErrorCode::InvalidInput,
            Some("s0000001"),
            Some("constraints.k0000002.value"),
            &["k0000002".to_owned()][..]
        )
    );
    // A whole member, its fields in its kind's order, as the document writes it.
    let set = f
        .set(
            "s0000001",
            "points.pb000000",
            r#"{"construction": false, "y": 0, "x": 20, "id": "pb000000"}"#,
        )
        .result
        .unwrap();
    assert_eq!(
        json(&set.after),
        r#"{"id": "pb000000", "x": 20, "y": 0, "construction": false}"#
    );
    // A member's ID changed: what it holds after is found by its new ID.
    let set = f
        .set("s0000001", "constraints.k0000001.id", "\"k0000003\"")
        .result
        .unwrap();
    assert_eq!(
        (json(&set.before), json(&set.after)),
        ("\"k0000001\"".into(), "\"k0000003\"".into())
    );
}

#[test]
fn a_member_named_by_place_is_refused_with_its_id_path_counted_as_the_text_shows_it() {
    let f = opened(SKETCHED);
    f.begin();
    let place = |field: &str| the_error(&f.set("s0000001", field, "12"));
    // The file's order: k0000002, then k0000001.
    let error = place("constraints.1.value");
    assert_eq!(
        (
            error.code(),
            error.message(),
            error.parameter(),
            error.feature()
        ),
        (
            ErrorCode::InvalidInput,
            "constraints.1.value names a constraint by its place: a request names a sketch's members by ID",
            Some("field"),
            Some("s0000001")
        )
    );
    assert_eq!(error.candidates(), ["constraints.k0000001.value"]);
    assert_eq!(place("points.0.x").candidates(), ["points.pb000000.x"]);
    // An arc, with its article, though the sketch has none.
    assert_eq!(
        place("arcs.0.radius").message(),
        "arcs.0.radius names an arc by its place: a request names a sketch's members by ID"
    );
    // No member at that place: no candidate.
    assert!(place("constraints.2.value").candidates().is_empty());
    // After an edit, the session's text is canonical, its lists in ID order.
    f.set("s0000001", "name", "\"Renamed\"").result.unwrap();
    assert_eq!(
        place("constraints.1.value").candidates(),
        ["constraints.k0000002.value"]
    );
    assert_eq!(place("points.0.x").candidates(), ["points.pa000000.x"]);
    // An ID no member of the list holds; one another list holds is its candidate.
    let error = place("circles.pa000000.radius");
    assert_eq!(
        error.message(),
        "sketch s0000001 has no circle \"pa000000\""
    );
    assert_eq!(error.candidates(), ["points.pa000000.radius"]);
    assert!(place("circles.zz000000.radius").candidates().is_empty());
    assert_eq!(
        place("arcs.zz000000.radius").suggestion(),
        Some("name an arc of this sketch by its ID")
    );
}

#[test]
fn an_edit_that_repeats_an_id_is_refused_at_the_field_it_named() {
    let f = opened(SKETCHED);
    f.begin();
    // The point given the line's ID: the gate would blame the line, the later holder, by
    // its place; the edit is refused at the point's field, naming the line.
    let error = the_error(&f.set("s0000001", "points.pb000000.id", "\"na000000\""));
    assert_eq!(
        (
            error.message(),
            error.parameter(),
            error.elements(),
            error.suggestion()
        ),
        (
            "id na000000 is already the ID of line na000000 (lines.na000000)",
            Some("points.pb000000.id"),
            &["na000000".to_owned()][..],
            Some("give this point an ID of its own")
        )
    );
    // A list set whole that gives one ID twice.
    let circle = r#"{"id": "ca000000", "center": "pa000000", "radius": 1, "construction": false}"#;
    let error = the_error(&f.set("s0000001", "circles", &format!("[{circle}, {circle}]")));
    assert_eq!(
        (error.message(), error.parameter(), error.elements()),
        (
            "id ca000000 is given to more than one circle of circles",
            Some("circles"),
            &["ca000000".to_owned()][..]
        )
    );
    assert_eq!(f.revision(), Revision::OPENED);
}

#[test]
fn a_removed_feature_comes_back_with_its_place_and_one_others_use_is_refused() {
    let f = opened(FUSED);
    f.begin();
    // The block, which the fuse uses: refused, the fuse naming it.
    let error = the_error(&answer(f.sessions.remove_feature(
        &f.id,
        Revision::OPENED,
        id("k7f2qx3m"),
    )));
    assert_eq!(
        (error.message(), error.feature(), error.parameter()),
        (
            "base k7f2qx3m isn't the ID of any feature",
            Some("f0000001"),
            Some("base")
        )
    );
    let removed = answer(
        f.sessions
            .remove_feature(&f.id, Revision::OPENED, id("f0000001")),
    )
    .result
    .unwrap();
    assert_eq!(removed.index, 2);
    assert!(removed.changed);
    assert_eq!(removed.commit, None);
    let held = document::read(FUSED.as_bytes()).unwrap();
    assert_eq!(removed.feature, held.features[2]);
    // Put back with replace_text: the document as it was.
    let without = f.text();
    let back = without.replacen(
        "\n  ]\n}\n",
        &format!(
            ",\n    {}\n  ]\n}}\n",
            document::write_feature(&removed.feature).unwrap()
        ),
        1,
    );
    let replaced = answer(f.sessions.replace_text(&f.id, f.revision(), back))
        .result
        .unwrap();
    assert_eq!(replaced.features, 3);
    assert_eq!(f.text(), document::write(&held).unwrap());
    let missing = answer(
        f.sessions
            .remove_feature(&f.id, f.revision(), id("z0000009")),
    );
    assert_eq!(refusal(&missing).1, "the document has no feature z0000009");
}

#[test]
fn a_feature_added_without_an_id_is_given_a_new_one_and_with_one_keeps_it() {
    let f = opened(FUSED);
    f.begin();
    let ball = r#"{"type": "sphere", "name": "Ball", "position": [0, 0, 50], "radius": 5}"#;
    let added = answer(f.sessions.add_feature(&f.id, Revision::OPENED, value(ball)))
        .result
        .unwrap();
    assert!(added.changed);
    assert_eq!(added.commit, None);
    let new = added.feature.id.clone();
    assert!(!["k7f2qx3m", "b0000001", "f0000001"].contains(&new.as_str()));
    let held = document::read(f.text().as_bytes()).unwrap();
    assert_eq!(held.features.len(), 4);
    assert_eq!(held.features[3], added.feature);
    assert_eq!(
        document::write_feature(&added.feature).unwrap(),
        format!(
            r#"{{"id": "{new}", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 50], "radius": 5}}"#
        )
    );
    let named = r#"{"id": "z0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 80], "radius": 5}"#;
    let added = answer(f.sessions.add_feature(&f.id, f.revision(), value(named)))
        .result
        .unwrap();
    assert_eq!(added.feature.id, id("z0000001"));
}

#[test]
fn a_feature_added_is_refused_with_what_the_request_may_not_give_and_the_gate_s_errors() {
    let f = opened(FUSED);
    let fingerprinted = r#"{"type": "box", "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1, "fingerprint": {"volume": 1, "area": 6, "faces": 6, "edges": 12, "vertices": 8}}"#;
    assert_eq!(
        refusal(&answer(f.sessions.add_feature(
            &f.id,
            Revision::OPENED,
            value(fingerprinted)
        ))),
        (
            ErrorCode::InvalidInput,
            "a new feature has no fingerprint: a commit writes it".to_owned(),
            Some("feature.fingerprint".to_owned()),
            Some("leave fingerprint out".to_owned())
        )
    );
    assert_eq!(
        refusal(&answer(f.sessions.add_feature(
            &f.id,
            Revision::OPENED,
            value("[]")
        )))
        .1,
        "feature must be an object, got an array"
    );
    // The gate's error, its feature the new one: a pocket of a sketch the document hasn't.
    let pocket = r#"{"id": "q7m2x9kd", "type": "pocket", "name": "Leg holes", "base": "f0000001", "sketch": "s0000003", "extent": {"type": "through_all", "reversed": false}}"#;
    let error = the_error(&answer(f.sessions.add_feature(
        &f.id,
        Revision::OPENED,
        value(pocket),
    )));
    assert_eq!(
        (error.message(), error.feature(), error.parameter()),
        (
            "sketch s0000003 isn't the ID of any feature",
            Some("q7m2x9kd"),
            Some("sketch")
        )
    );
    // An ID the document holds already: the request's field, the holder by ID and name.
    let again = r#"{"id": "k7f2qx3m", "type": "sphere", "name": "Ball", "position": [0, 0, 50], "radius": 5}"#;
    let error = the_error(&answer(f.sessions.add_feature(
        &f.id,
        Revision::OPENED,
        value(again),
    )));
    assert_eq!(
        (
            error.code(),
            error.message(),
            error.feature(),
            error.parameter(),
            error.elements(),
            error.suggestion()
        ),
        (
            ErrorCode::InvalidInput,
            "id k7f2qx3m is already the ID of feature k7f2qx3m (\"Block\")",
            None,
            Some("feature.id"),
            &["k7f2qx3m".to_owned()][..],
            Some("give the feature an ID the document doesn't hold, or leave id out for a new one")
        )
    );
    assert_eq!(f.file(), FUSED);
    assert_eq!(f.revision(), Revision::OPENED);
}

#[test]
fn replace_text_outside_a_transaction_gives_back_the_text_it_replaced() {
    let f = opened(BLOCK);
    let response = answer(
        f.sessions
            .replace_text(&f.id, Revision::OPENED, EMPTY.to_owned()),
    );
    assert_eq!(response.session.unwrap().revision.number(), 2);
    let replaced = response.result.unwrap();
    assert!(replaced.changed);
    assert_eq!(replaced.features, 0);
    assert_eq!(replaced.before.as_deref(), Some(BLOCK));
    let commit = replaced.commit.unwrap();
    assert!(commit.written);
    assert!(commit.refingerprinted.is_empty());
    assert_eq!(f.file(), EMPTY);
    // The same document in another layout changes nothing.
    let unchanged = answer(f.sessions.replace_text(
        &f.id,
        f.revision(),
        r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": []}"#.to_owned(),
    ));
    assert_eq!(unchanged.session.unwrap().revision.number(), 2);
    let unchanged = unchanged.result.unwrap();
    assert!(!unchanged.changed);
    assert_eq!(unchanged.commit, None);
    assert_eq!(f.file(), EMPTY);
}

#[test]
fn replace_text_inside_a_transaction_gives_back_nothing() {
    let f = opened(BLOCK);
    f.begin();
    let replaced = answer(
        f.sessions
            .replace_text(&f.id, Revision::OPENED, EMPTY.to_owned()),
    )
    .result
    .unwrap();
    assert!(replaced.changed);
    assert_eq!((replaced.before, replaced.commit), (None, None));
    assert_eq!(f.file(), BLOCK);
    assert_eq!(f.text(), EMPTY);
}

#[test]
fn replace_text_refuses_text_without_naming_and_what_check_refuses() {
    let f = opened(BLOCK);
    let unnamed =
        r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": []}"#;
    let response = answer(
        f.sessions
            .replace_text(&f.id, Revision::OPENED, unnamed.to_owned()),
    );
    let error = the_error(&response);
    assert_eq!(
        (
            error.code(),
            error.message(),
            error.parameter(),
            error.cause(),
            error.suggestion()
        ),
        (
            ErrorCode::InvalidInput,
            "the document's header has no naming",
            Some("naming"),
            Some(
                "only a document saved before the header recorded its naming scheme is read without it"
            ),
            Some("add \"naming\": 5 after units")
        )
    );
    // Not JSON: the reader's error, with where, naming text.
    let error = the_error(&answer(f.sessions.replace_text(
        &f.id,
        Revision::OPENED,
        "{\"silico\": 1,".to_owned(),
    )));
    assert_eq!(error.parameter(), Some("text"));
    assert!(error.message().contains("at line 1, column "), "{error:?}");
    // A box the gate refuses, its error the gate's.
    let error = the_error(&answer(f.sessions.replace_text(
        &f.id,
        Revision::OPENED,
        BLOCK.replace(r#""length": 40"#, r#""length": 0"#),
    )));
    assert_eq!(
        (error.feature(), error.parameter()),
        (Some("k7f2qx3m"), Some("length"))
    );
    assert_eq!(f.file(), BLOCK);
    assert_eq!(f.text(), BLOCK);
    assert_eq!(f.revision(), Revision::OPENED);
}

#[test]
fn a_refused_document_is_mended_by_replace_text_alone() {
    let flat = BLOCK.replace(r#""length": 40"#, r#""length": 0"#);
    let f = opened(&flat);
    let refused = (
        ErrorCode::InvalidInput,
        "session s1's document is one the gate refuses, so only replace_text edits it".to_owned(),
        Some("session".to_owned()),
        Some("read its problems (check), then mend its text with replace_text".to_owned()),
    );
    assert_eq!(refusal(&f.set("k7f2qx3m", "length", "40")), refused);
    assert_eq!(
        refusal(&answer(f.sessions.remove_feature(
            &f.id,
            Revision::OPENED,
            id("k7f2qx3m")
        ))),
        refused
    );
    assert_eq!(
        refusal(&answer(f.sessions.add_feature(
            &f.id,
            Revision::OPENED,
            value("{}")
        ))),
        refused
    );
    let replaced = answer(
        f.sessions
            .replace_text(&f.id, Revision::OPENED, BLOCK.to_owned()),
    )
    .result
    .unwrap();
    assert_eq!(replaced.before, Some(flat));
    assert!(replaced.commit.unwrap().written);
    assert!(f.sessions.status(&f.id).result.unwrap().gated);
    // Mended, so typed edits are taken again.
    assert!(f.set("k7f2qx3m", "length", "41").result.unwrap().changed);
    assert_eq!(f.file(), BLOCK_41);
}

#[test]
fn a_commit_of_an_edit_outside_a_transaction_rebuilds_it_on_the_kernel_once() {
    let f = opened(BLOCK);
    f.set("k7f2qx3m", "length", "41").result.unwrap();
    assert_eq!(f.boxes(), 1);
    // Its check rebuilds the saved document from the steps already built.
    answer(f.sessions.check(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    assert_eq!(f.boxes(), 1);
}

#[test]
fn a_text_that_isnt_utf8_is_replaced_only_inside_a_transaction() {
    let f = opened("{}");
    std::fs::write(&f.path, b"{\xff}").unwrap();
    // Opened again on the bytes that aren't UTF-8: the session holds them, unread.
    drop(f.sessions);
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let secret = f._dir.path().join("secret");
    let sessions =
        Sessions::with_cache_opener(kernel, move |cache| Cache::with_secret(cache, &secret));
    let s1 = answer(sessions.open(&f.path, false)).session.unwrap().id;
    let outside = answer(sessions.replace_text(&s1, Revision::OPENED, EMPTY.to_owned()));
    assert_eq!(
        refusal(&outside),
        (
            ErrorCode::InvalidInput,
            "session s1's text isn't UTF-8 (at byte 1), so replace_text couldn't give it back".to_owned(),
            Some("session".to_owned()),
            Some("replace it inside a transaction (begin, replace_text, commit), where no text is given back".to_owned())
        )
    );
    assert_eq!(outside.session.unwrap().revision, Revision::OPENED);
    assert_eq!(std::fs::read(&f.path).unwrap(), b"{\xff}");
    answer(sessions.begin(&s1)).result.unwrap();
    let inside = answer(sessions.replace_text(&s1, Revision::OPENED, EMPTY.to_owned()))
        .result
        .unwrap();
    assert!(inside.changed);
    assert!(answer(sessions.commit(&s1)).result.unwrap().written);
    assert_eq!(std::fs::read_to_string(&f.path).unwrap(), EMPTY);
}

#[test]
fn hostile_paths_and_ids_are_refused_saying_what_they_are() {
    let f = opened(BLOCK);
    // A long path with a bidirectional override: quoted, escaped, and cut short.
    let field = format!("a\u{202e}b{}", "x".repeat(100));
    let error = the_error(&f.set("k7f2qx3m", &field, "1"));
    assert_eq!(error.parameter(), Some("field"));
    assert!(
        error
            .message()
            .starts_with("a box has no field \"a\\u{202e}bxxx"),
        "{error:?}"
    );
    assert!(error.message().ends_with("…"), "{error:?}");
    assert!(!error.message().contains('\u{202e}'));
    // A feature's own ID that isn't one: the reader's error, the feature named by its
    // place in the timeline, where it would go.
    let bad =
        r#"{"id": "BAD", "type": "sphere", "name": "Ball", "position": [0, 0, 50], "radius": 5}"#;
    let error = the_error(&answer(f.sessions.add_feature(
        &f.id,
        Revision::OPENED,
        value(bad),
    )));
    assert_eq!(
        (error.feature(), error.parameter(), error.message()),
        (
            Some("features[1]"),
            Some("feature.id"),
            "id \"BAD\" isn't a feature ID"
        )
    );
    // A sketch member named by an empty segment: no member has it.
    let f = opened(SKETCHED);
    let error = the_error(&f.set("s0000001", "points..x", "1"));
    assert_eq!(error.message(), "sketch s0000001 has no point \"\"");
    assert!(error.candidates().is_empty());
    assert_eq!(f.file(), SKETCHED);
}

/// The bytes of [`BLOCK`] in canonical form, by hand: [`BLOCK_41`] 40 long, its fingerprint
/// the one stored.
fn block_canonical() -> String {
    BLOCK_41
        .replace("\"length\": 41,", "\"length\": 40,")
        .replace(
            r#""volume": 8200, "area": 2860, "bbox": [0, 0, 0, 41, 20, 10]"#,
            r#""volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10]"#,
        )
}

/// pb solved to x = 12.25 (the solver's values: the origin and the axes' ends, 0 to 5,
/// then the points by ID, pa's x and y at 6 and 7, pb's at 8 and 9): its stored x is 10,
/// so the solve is drift unless an edit reaches the sketch.
fn moving_pb(sketches: &FakeSketches) {
    sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.values[8] = 12.25;
        Ok(solved)
    });
}

#[test]
fn a_name_set_beside_a_sketch_whose_lists_arent_in_id_order_reaches_nothing() {
    let f = opened(SKETCHED);
    moving_pb(&f.sketches);
    let set = f.set("s0000001", "name", "\"Renamed\"").result.unwrap();
    let commit = set.commit.unwrap();
    assert!(commit.written);
    // Its lists put in ID order changed none of its values: nothing reached, so the stored
    // solve is kept and its drift said.
    assert!(commit.refingerprinted.is_empty());
    assert!(commit.resolved.is_empty(), "{:?}", commit.resolved);
    let drift: Vec<_> = commit
        .problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature()))
        .collect();
    assert_eq!(drift, [(ErrorCode::Drift, Some("s0000001"))]);
    assert!(f.file().contains(r#""id": "pb000000", "x": 10, "y": 0"#));
}

#[test]
fn an_edit_and_its_undo_beside_a_sketch_whose_lists_arent_in_id_order_write_nothing() {
    let f = opened(SKETCHED);
    let inode = std::fs::metadata(&f.path).unwrap().ino();
    f.begin();
    assert!(
        f.set("s0000001", "name", "\"Renamed\"")
            .result
            .unwrap()
            .changed
    );
    assert!(
        f.set("s0000001", "name", "\"Sketch\"")
            .result
            .unwrap()
            .changed
    );
    let committed = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(!committed.written);
    assert_eq!(f.file(), SKETCHED);
    assert_eq!(std::fs::metadata(&f.path).unwrap().ino(), inode);
}

#[test]
fn an_edit_outside_a_transaction_publishes_its_model_as_the_saved_revisions() {
    let f = opened(BLOCK);
    f.set("k7f2qx3m", "length", "41").result.unwrap();
    let calls = f.kernel.calls().len();
    // Published as revision 2's at once, its document the saved one, fingerprint and all.
    let published = f.sessions.published(&f.id).result.unwrap();
    assert_eq!(published.revision.number(), 2);
    assert_eq!(
        published.model.document(),
        &document::read(BLOCK_41.as_bytes()).unwrap()
    );
    // Its check finds no drift and starts no rebuild: the model is the one published.
    let check = answer(f.sessions.check(&f.id, ModelChoice::Current));
    assert!(check.result.unwrap().problems.is_empty());
    assert_eq!(check.model.unwrap().revision.number(), 2);
    assert_eq!(f.kernel.calls().len(), calls);
    let after = f.sessions.published(&f.id).result.unwrap();
    assert!(Arc::ptr_eq(&after.model, &published.model));
}

#[test]
fn a_member_named_by_place_or_a_path_too_deep_repeats_nothing_raw() {
    let f = opened(SKETCHED);
    let error = the_error(&f.set("s0000001", "points.0.\u{202e}x", "1"));
    assert_eq!(
        error.message(),
        r#""points.0.\u{202e}x" names a point by its place: a request names a sketch's members by ID"#
    );
    // The rest of the path isn't plain: the candidate is the member's path alone.
    assert_eq!(error.candidates(), ["points.pb000000"]);
    let error = the_error(&f.set("s0000001", "circles.pa000000.\u{202e}", "1"));
    assert_eq!(error.candidates(), ["points.pa000000"]);
    // A path of a million dots, deeper than any tree: refused as a field the sketch hasn't.
    let error = the_error(&f.set("s0000001", &format!("points{}", ".".repeat(1_000_000)), "1"));
    assert_eq!(error.parameter(), Some("field"));
    assert!(
        error
            .message()
            .starts_with("a sketch has no field \"points......"),
        "{error:?}"
    );
}

#[test]
fn a_feature_added_with_a_null_id_or_version_is_given_them() {
    let f = opened(BLOCK);
    f.begin();
    for ball in [
        r#"{"id": null, "type": "sphere", "name": "Ball", "position": [0, 0, 50], "radius": 5}"#,
        r#"{"id": "z0000001", "type": "sphere", "v": null, "name": "Ball", "position": [0, 0, 50], "radius": 5}"#,
    ] {
        let added = answer(f.sessions.add_feature(&f.id, f.revision(), value(ball)))
            .result
            .unwrap();
        let new = added.feature.id.clone();
        assert_ne!(new, id("k7f2qx3m"));
        assert_eq!(
            document::write_feature(&added.feature).unwrap(),
            format!(
                r#"{{"id": "{new}", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 50], "radius": 5}}"#
            )
        );
    }
}

/// A box, an empty sketch, and a pad of it with no base.
const PAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "s0000003", "type": "sketch", "v": 1, "name": "Profile", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}, {"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000003", "extent": {"type": "one_side", "length": 6, "reversed": false}}]}"#;

/// The held document's pad's base.
fn pad_base(f: &Fixture) -> Option<document::FeatureId> {
    let held = document::read(f.text().as_bytes()).unwrap();
    match &held.features[2].operation {
        Operation::Pad { base, .. } => base.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_pads_base_is_given_and_taken_away_by_null() {
    let f = opened(PAD);
    f.begin();
    let set = f.set("p0000001", "base", "\"k7f2qx3m\"").result.unwrap();
    assert!(set.changed);
    assert_eq!(
        (json(&set.before), json(&set.after)),
        ("null".into(), "\"k7f2qx3m\"".into())
    );
    assert_eq!(pad_base(&f), Some(id("k7f2qx3m")));
    let set = f.set("p0000001", "base", "null").result.unwrap();
    assert!(set.changed);
    assert_eq!(
        (json(&set.before), json(&set.after)),
        ("\"k7f2qx3m\"".into(), "null".into())
    );
    assert_eq!(pad_base(&f), None);
    assert!(!f.text().contains("\"base\""));
}

#[test]
fn an_edited_feature_s_errors_name_no_place_in_a_text() {
    let f = opened(BLOCK);
    let error = the_error(&f.set("k7f2qx3m", "length", "\"long\""));
    assert_eq!(error.parameter(), Some("length"));
    assert!(!error.message().contains(", at line"), "{error:?}");
    let ball = r#"{"type": "sphere", "name": "Ball", "position": [0, 0, 50], "radius": "big"}"#;
    let error = the_error(&answer(f.sessions.add_feature(
        &f.id,
        Revision::OPENED,
        value(ball),
    )));
    assert_eq!(error.parameter(), Some("radius"));
    assert!(!error.message().contains(", at line"), "{error:?}");
}

#[test]
fn an_id_repeated_within_one_list_names_the_member_that_holds_it() {
    let f = opened(SKETCHED);
    f.begin();
    let error = the_error(&f.set("s0000001", "points.pb000000.id", "\"pa000000\""));
    assert_eq!(
        (
            error.message(),
            error.parameter(),
            error.elements(),
            error.suggestion()
        ),
        (
            "id pa000000 is already the ID of point pa000000 (points.pa000000)",
            Some("points.pb000000.id"),
            &["pa000000".to_owned()][..],
            Some("give this point an ID of its own")
        )
    );
}

#[test]
fn a_member_set_whole_under_a_new_id_reads_back_after_by_that_id() {
    let f = opened(SKETCHED);
    f.begin();
    let set = f
        .set(
            "s0000001",
            "constraints.k0000001",
            r#"{"id": "k0000003", "type": "horizontal", "line": "na000000"}"#,
        )
        .result
        .unwrap();
    assert_eq!(
        (json(&set.before), json(&set.after)),
        (
            r#"{"id": "k0000001", "type": "horizontal", "line": "na000000"}"#.into(),
            r#"{"id": "k0000003", "type": "horizontal", "line": "na000000"}"#.into()
        )
    );
}

#[test]
fn a_member_whose_id_is_eight_digits_is_named_by_it() {
    let sketch = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [{"id": "12345678", "x": 1, "y": 2, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}]}"#;
    let f = opened(sketch);
    f.begin();
    let set = f.set("s0000001", "points.12345678.x", "5").result.unwrap();
    assert_eq!(
        (json(&set.before), json(&set.after)),
        ("1".into(), "5".into())
    );
}

/// Three boxes: the block, a lug at x 100, a cap at x 200.
const THREE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10}, {"id": "b0000002", "type": "box", "v": 1, "name": "Cap", "position": [200, 0, 0], "length": 5, "width": 5, "height": 5}]}"#;

#[test]
fn a_feature_removed_from_the_middle_gives_its_place() {
    let f = opened(THREE);
    f.begin();
    let removed = answer(
        f.sessions
            .remove_feature(&f.id, Revision::OPENED, id("b0000001")),
    )
    .result
    .unwrap();
    assert_eq!(removed.index, 1);
    let held = document::read(f.text().as_bytes()).unwrap();
    let ids: Vec<&str> = held.features.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["k7f2qx3m", "b0000002"]);
}

#[test]
fn a_feature_added_and_removed_outside_a_transaction_each_save_their_commit() {
    let f = opened(BLOCK);
    let lug = r#"{"id": "b0000001", "type": "box", "name": "Lug", "position": [100, 0, 0], "length": 1, "width": 2, "height": 3}"#;
    let response = answer(f.sessions.add_feature(&f.id, Revision::OPENED, value(lug)));
    assert_eq!(response.session.unwrap().revision.number(), 2);
    let commit = response.result.unwrap().commit.unwrap();
    assert!(commit.written);
    assert_eq!(commit.refingerprinted, [id("b0000001")]);
    // 1 × 2 × 3 = 6, area 2(2 + 3 + 6) = 22, to (101, 2, 3).
    let with_lug = block_canonical().replace(
        "\n    }\n  ]\n}\n",
        "\n    },\n    {\n      \"id\": \"b0000001\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Lug\",\n      \"position\": [100, 0, 0],\n      \"length\": 1,\n      \"width\": 2,\n      \"height\": 3,\n      \"fingerprint\": {\"volume\": 6, \"area\": 22, \"bbox\": [100, 0, 0, 101, 2, 3], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n",
    );
    assert_eq!(f.file(), with_lug);
    let removed = answer(
        f.sessions
            .remove_feature(&f.id, f.revision(), id("b0000001")),
    )
    .result
    .unwrap();
    assert_eq!(removed.index, 1);
    let commit = removed.commit.unwrap();
    assert!(commit.written);
    assert!(commit.refingerprinted.is_empty());
    assert_eq!(f.file(), block_canonical());
}

#[test]
fn a_sketch_added_is_given_back_as_the_document_holds_it() {
    let f = opened(BLOCK);
    f.begin();
    let sketch = r#"{"id": "s0000009", "type": "sketch", "name": "S", "plane": "xy", "points": [{"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}"#;
    let added = answer(
        f.sessions
            .add_feature(&f.id, Revision::OPENED, value(sketch)),
    )
    .result
    .unwrap();
    let held = document::read(f.text().as_bytes()).unwrap();
    assert_eq!(added.feature, held.features[1]);
    let Operation::Sketch(sketch) = &added.feature.operation else {
        panic!("not a sketch");
    };
    let points: Vec<&str> = sketch.points.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(points, ["pa000000", "pb000000"]);
}

/// A circle about pa, radius 1, under `id`.
fn circle(id: &str) -> String {
    format!(r#"{{"id": "{id}", "center": "pa000000", "radius": 1, "construction": false}}"#)
}

#[test]
fn a_circle_given_three_times_is_one_refusal() {
    let f = opened(SKETCHED);
    f.begin();
    let three = format!("[{0}, {0}, {0}]", circle("ca000000"));
    let response = f.set("s0000001", "circles", &three);
    let error = the_error(&response);
    assert_eq!(
        error.message(),
        "id ca000000 is given to more than one circle of circles"
    );
}

#[test]
fn a_hundred_and_one_ids_given_twice_list_a_hundred_and_count_them_all() {
    let f = opened(SKETCHED);
    f.begin();
    let circles: Vec<String> = (0..101)
        .flat_map(|i| {
            let c = circle(&format!("c{i:07}"));
            [c.clone(), c]
        })
        .collect();
    let response = f.set("s0000001", "circles", &format!("[{}]", circles.join(", ")));
    let problems = response.result.unwrap_err();
    assert_eq!((problems.errors().len(), problems.total()), (100, 101));
}

#[test]
fn replace_text_that_changes_only_a_stored_fingerprint_changes_the_document() {
    let f = opened(BLOCK);
    // 7999 stored, 8000 measured: the text is another document, saved as given, its
    // fingerprint's drift said (format accepts it by name).
    let text = BLOCK.replace(r#""volume": 8000"#, r#""volume": 7999"#);
    let replaced = answer(f.sessions.replace_text(&f.id, Revision::OPENED, text))
        .result
        .unwrap();
    assert!(replaced.changed);
    let commit = replaced.commit.unwrap();
    assert!(commit.written);
    assert!(commit.refingerprinted.is_empty());
    let drift: Vec<_> = commit
        .problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature()))
        .collect();
    assert_eq!(drift, [(ErrorCode::Drift, Some("k7f2qx3m"))]);
    assert_eq!(
        f.file(),
        block_canonical().replace("\"volume\": 8000", "\"volume\": 7999")
    );
}

#[test]
fn an_edit_whose_save_writes_a_solve_leaves_its_check_to_rebuild_the_saved_document() {
    let f = opened(SKETCHED);
    moving_pb(&f.sketches);
    // pb sent at 11, solved to 12.25: the save writes the solve.
    let set = f.set("s0000001", "points.pb000000.x", "11").result.unwrap();
    let commit = set.commit.unwrap();
    assert_eq!(commit.resolved, [id("s0000001")]);
    assert!(f.file().contains(r#""id": "pb000000", "x": 12.25, "y": 0"#));
    let sent = f.sketches.requests().len();
    // The edit's model was solved from 11, not from what the file now holds: its check
    // solves the saved document again, sending pb's x as saved.
    answer(f.sessions.check(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    let requests = f.sketches.requests();
    assert_eq!(requests.len(), sent + 1);
    assert_eq!(requests[sent].values[8], 12.25);
}

/// A sketch whose points are pa (0, 0), pz (1, 0), and pa again (2, 0): its third repeats
/// the first's ID, which its canonical text, in ID order, has second.
const REPEATED: &str = r#"{"id": "s0000009", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pz000000", "x": 1, "y": 0, "construction": false}, {"id": "pa000000", "x": 2, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}"#;

#[test]
fn an_edit_refused_counts_places_in_its_own_order_not_the_canonical_text_s() {
    // As silico check counts the text's: the repeat is the third point given, points.2.
    let f = opened(BLOCK);
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{REPEATED}]}}"#
    );
    let error = the_error(&answer(f.sessions.replace_text(
        &f.id,
        Revision::OPENED,
        text,
    )));
    assert_eq!(
        (error.feature(), error.parameter()),
        (Some("s0000009"), Some("points.2.id"))
    );
    let error = the_error(&answer(f.sessions.add_feature(
        &f.id,
        Revision::OPENED,
        value(REPEATED),
    )));
    assert_eq!(
        (error.feature(), error.parameter()),
        (Some("s0000009"), Some("points.2.id"))
    );
    assert_eq!(f.file(), BLOCK);
    assert_eq!(f.revision(), Revision::OPENED);
}

#[test]
fn dropping_the_sessions_while_an_edit_rebuilds_refuses_it_and_writes_nothing() {
    let f = opened(SKETCHED);
    // Settled first, so the solve held is the edit's.
    answer(f.sessions.check(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    f.sketches.hold();
    let running = f.sessions.set_field(
        &f.id,
        Revision::OPENED,
        id("s0000001"),
        "points.pb000000.x".to_owned(),
        value("11"),
    );
    assert!(f.sketches.wait_held(std::time::Duration::from_secs(10)));
    let Fixture {
        _dir,
        sessions,
        sketches,
        path,
        ..
    } = f;
    let dropper = std::thread::spawn(move || drop(sessions));
    let response = answer(running);
    assert_eq!(
        refusal(&response),
        (
            ErrorCode::InvalidInput,
            "no session s1 is open".to_owned(),
            Some("session".to_owned()),
            Some(
                "open the document again: a session ends when it's closed or its client goes away"
                    .to_owned()
            )
        )
    );
    assert_eq!(response.session.unwrap().revision, Revision::OPENED);
    dropper.join().unwrap();
    sketches.release();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SKETCHED);
}

#[test]
fn a_feature_added_with_a_null_fingerprint_note_or_base_has_none() {
    let f = opened(PAD);
    f.begin();
    for given in [
        r#"{"type": "sphere", "name": "Ball", "position": [0, 0, 50], "radius": 5, "fingerprint": null}"#,
        r#"{"type": "sphere", "name": "Ball", "position": [0, 0, 50], "radius": 5, "note": null}"#,
    ] {
        let added = answer(f.sessions.add_feature(&f.id, f.revision(), value(given)))
            .result
            .unwrap();
        let new = added.feature.id.clone();
        assert_eq!(
            document::write_feature(&added.feature).unwrap(),
            format!(
                r#"{{"id": "{new}", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 50], "radius": 5}}"#
            )
        );
    }
    let pad = r#"{"type": "pad", "name": "Pad", "sketch": "s0000003", "extent": {"type": "one_side", "length": 6, "reversed": false}, "base": null}"#;
    let added = answer(f.sessions.add_feature(&f.id, f.revision(), value(pad)))
        .result
        .unwrap();
    match &added.feature.operation {
        Operation::Pad { base, .. } => assert_eq!(*base, None),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        (added.feature.note, added.feature.fingerprint),
        (None, None)
    );
}

#[test]
fn replace_text_that_changes_only_a_feature_s_id_changes_the_document() {
    // The fuse, which nothing uses, renamed: its ID the only difference.
    let f = opened(FUSED);
    let text = FUSED.replace(r#""id": "f0000001""#, r#""id": "f0000002""#);
    let replaced = answer(f.sessions.replace_text(&f.id, Revision::OPENED, text))
        .result
        .unwrap();
    assert!(replaced.changed);
    assert!(replaced.commit.unwrap().written);
    assert!(f.file().contains(r#""id": "f0000002""#));
    assert!(!f.file().contains("f0000001"));
}

#[test]
fn a_path_below_a_member_the_feature_hasnt_is_a_field_it_hasnt() {
    // A box has no note, and a pad here no base: neither has anything below it to set.
    let f = opened(PAD);
    for (feature, field, what) in [
        ("k7f2qx3m", "note.text", "box"),
        ("p0000001", "base.id", "pad"),
    ] {
        let error = the_error(&f.set(feature, field, "\"k7f2qx3m\""));
        assert_eq!(
            (error.message(), error.feature(), error.parameter()),
            (
                format!("a {what} has no field {field}").as_str(),
                Some(feature),
                Some("field")
            )
        );
    }
    assert_eq!(f.file(), PAD);
}

#[test]
fn a_feature_added_outside_a_transaction_comes_back_as_saved_fingerprint_and_all() {
    let f = opened(BLOCK);
    let lug = r#"{"id": "b0000002", "type": "box", "name": "Lug", "position": [100, 0, 0], "length": 1, "width": 2, "height": 3}"#;
    let added = answer(f.sessions.add_feature(&f.id, Revision::OPENED, value(lug)))
        .result
        .unwrap();
    assert!(added.commit.unwrap().written);
    // Its fingerprint, by hand: 1 × 2 × 3 = 6, area 2(2 + 3 + 6) = 22, from (100, 0, 0)
    // to (101, 2, 3); as the saved file holds it.
    assert_eq!(
        document::write_feature(&added.feature).unwrap(),
        "{\n  \"id\": \"b0000002\",\n  \"type\": \"box\",\n  \"v\": 1,\n  \"name\": \"Lug\",\n  \"position\": [100, 0, 0],\n  \"length\": 1,\n  \"width\": 2,\n  \"height\": 3,\n  \"fingerprint\": {\"volume\": 6, \"area\": 22, \"bbox\": [100, 0, 0, 101, 2, 3], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n}"
    );
    let saved = document::read(f.file().as_bytes()).unwrap();
    assert_eq!(saved.features[1], added.feature);
}

#[test]
fn a_sketch_member_set_outside_a_transaction_comes_back_where_its_solve_put_it() {
    let f = opened(SKETCHED);
    // pb set to x = 11, and solved to 12.25: the commit writes the solve.
    moving_pb(&f.sketches);
    let set = f.set("s0000001", "points.pb000000.x", "11").result.unwrap();
    let commit = set.commit.unwrap();
    assert_eq!(commit.resolved, [id("s0000001")]);
    assert_eq!(
        (json(&set.before), json(&set.after)),
        ("10".into(), "12.25".into())
    );
    assert!(
        f.file().contains(r#""id": "pb000000", "x": 12.25, "y": 0"#),
        "{}",
        f.file()
    );
}

#[test]
fn a_member_renamed_outside_a_transaction_comes_back_by_its_new_id_as_saved() {
    // A constraint's ID set: after names it by the new ID, which the saved file holds, the
    // old one gone.
    let f = opened(SKETCHED);
    let set = f
        .set("s0000001", "constraints.k0000001.id", r#""k0000003""#)
        .result
        .unwrap();
    assert!(set.commit.unwrap().written);
    assert_eq!(
        (json(&set.before), json(&set.after)),
        (r#""k0000001""#.into(), r#""k0000003""#.into())
    );
    let file = f.file();
    assert!(
        file.contains(r#"{"id": "k0000003", "type": "horizontal", "line": "na000000"}"#),
        "{file}"
    );
    assert!(!file.contains("k0000001"), "{file}");

    // A point nothing uses, set whole under a new ID: after is the new point, as saved.
    let free = SKETCHED.replace(
        r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}"#,
        r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pf000000", "x": 3, "y": 4, "construction": true}"#,
    );
    assert_ne!(free, SKETCHED);
    let f = opened(&free);
    let point = r#"{"id": "pg000000", "x": 5, "y": 6, "construction": false}"#;
    let set = f.set("s0000001", "points.pf000000", point).result.unwrap();
    assert!(set.commit.unwrap().written);
    assert_eq!(
        (json(&set.before), json(&set.after)),
        (
            r#"{"id": "pf000000", "x": 3, "y": 4, "construction": true}"#.into(),
            point.into()
        )
    );
    let file = f.file();
    assert!(file.contains(point), "{file}");
    assert!(!file.contains("pf000000"), "{file}");
}
