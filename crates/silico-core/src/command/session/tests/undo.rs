//! `undo` and `redo` (docs/design/m5.0-app.md, section 7, and its test plan's "Undo and
//! redo"), through the sessions' own commands: each state's text back byte for byte, its
//! model compared whole and rebuilt with no kernel step, what the edits reach worked out
//! again, `to`, the bound, emptying, and every refusal's words. Every expected value is by
//! hand: texts written out, fingerprints by formula, the stack's bytes by A2's count.

use super::*;
use crate::command::Command;
use crate::document::Fingerprint;
use crate::document::json::Node;
use crate::document::undo::UNDO_BYTES;
use crate::naming::ElementMap;

/// A number as a request gives it.
fn number(text: &str) -> Node {
    crate::document::json::read(text.as_bytes()).unwrap()
}

/// [`BOX`]'s canonical text with the block `length` long: its one feature on one line (it
/// holds no object), indented four spaces, between the header and the array's close.
fn box_text(length: u32) -> String {
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n    {{\"id\": \"k7f2qx3m\", \"type\": \"box\", \"v\": 1, \"name\": \"Block\", \"position\": [0, 0, 0], \"length\": {length}, \"width\": 20, \"height\": 10}}\n  ]\n}}\n"
    )
}

/// What the stack counts for [`BOX`]'s file, held whole as `begin` saw it: its 212 bytes.
const BOX_BYTES: usize = 212;

/// What the stack counts for a held state of [`box_text`] with a two-digit length, kept in
/// chunks (A2): its list, 24 and 8 for its one chunk; and the chunk, 16 and its feature's
/// text, 4 spaces of indent and the 122 characters of
/// `{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0],
/// "length": 41, "width": 20, "height": 10}`.
const BOX_STATE: usize = 24 + 8 + 16 + 4 + 122;

/// Every feature of `model`, with its element map and its fingerprint as rebuilt: the
/// model, compared whole.
fn whole(model: &Model) -> Vec<(FeatureId, Option<ElementMap>, Option<Fingerprint>)> {
    model
        .document()
        .features
        .iter()
        .map(|feature| {
            (
                feature.id.clone(),
                model.element_map(&feature.id).cloned(),
                model.fingerprint(&feature.id),
            )
        })
        .collect()
}

impl Fixture {
    fn revision(&self) -> Revision {
        self.session().held.read().revision
    }

    /// The session's text, as `text` gives it.
    fn text(&self) -> String {
        self.sessions
            .text(&self.id, None)
            .wait()
            .result
            .unwrap()
            .text
    }

    /// `feature`'s `field` set to `value` against the session's revision: the new revision.
    fn set(&self, feature: &str, field: &str, value: &str) -> Revision {
        let response = self
            .sessions
            .set_field(
                &self.id,
                self.revision(),
                id(feature),
                field.to_owned(),
                number(value),
            )
            .wait();
        assert!(response.result.unwrap().changed);
        response.session.unwrap().revision
    }

    fn undo(&self, to: Option<u64>) -> Response<Stepped> {
        self.sessions
            .undo(&self.id, self.revision(), to.map(Revision))
            .wait()
    }

    fn redo(&self, to: Option<u64>) -> Response<Stepped> {
        self.sessions
            .redo(&self.id, self.revision(), to.map(Revision))
            .wait()
    }

    /// What the open transaction's undo stack counts.
    fn stack_bytes(&self) -> usize {
        self.session().held.read().undo.as_ref().unwrap().bytes()
    }

    /// The current revision's model, waited for.
    fn model(&self) -> Arc<Model> {
        match self.session().current(&probe()) {
            Ok(current) => current.model,
            Err(_) => panic!("no model"),
        }
    }

    fn calls(&self) -> usize {
        self.kernel.calls().len()
    }
}

/// [`opened`], each transaction's undo stack bounded by `bound` bytes.
fn opened_bounded(text: &str, bound: usize) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("a.silico");
    std::fs::write(&path, text).unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let solver = crate::fake::FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    )
    .checking_disk_every(Duration::ZERO)
    .bounding_undo(bound);
    let id = sessions.open(&path, false).wait().session.unwrap().id;
    Fixture {
        _dir: dir,
        kernel,
        sessions,
        id,
        path,
    }
}

/// A step's result, and the revision its envelope says the session is at.
#[track_caller]
fn stepped(response: Response<Stepped>) -> (Stepped, Revision) {
    let revision = response.session.as_ref().unwrap().revision;
    (response.result.unwrap(), revision)
}

fn step(restored: u64, undo: usize, redo: usize, dropped: usize) -> Stepped {
    Stepped {
        restored: Revision(restored),
        undo,
        redo,
        dropped,
    }
}

/// An error's words: its code, message, parameter, cause, and suggestion.
type Words = (
    ErrorCode,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// The one error a refusal holds, every field of it but its feature and elements (none),
/// and the session's revision the envelope says.
#[track_caller]
fn refusal<T: std::fmt::Debug>(response: Response<T>) -> (Words, Vec<String>, Revision) {
    let revision = response.session.as_ref().unwrap().revision;
    let problems = response.result.unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    let error = &problems.errors()[0];
    assert_eq!((error.feature(), error.elements_total()), (None, 0));
    (
        (
            error.code(),
            error.message().to_owned(),
            error.parameter().map(str::to_owned),
            error.cause().map(str::to_owned),
            error.suggestion().map(str::to_owned),
        ),
        error.candidates().to_vec(),
        revision,
    )
}

fn words(
    code: ErrorCode,
    message: &str,
    parameter: &str,
    cause: Option<&str>,
    suggestion: &str,
) -> Words {
    (
        code,
        message.to_owned(),
        Some(parameter.to_owned()),
        cause.map(str::to_owned),
        Some(suggestion.to_owned()),
    )
}

/// The lug's and the fuse's volume and area as rebuilt in `model`.
fn lug_and_fuse(model: &Model) -> [(Measure, Measure); 2] {
    ["b0000001", "f0000001"].map(|feature| {
        let fingerprint = model.fingerprint(&id(feature)).unwrap();
        (fingerprint.volume, fingerprint.area)
    })
}

#[test]
fn edits_undone_one_by_one_give_each_state_back_byte_for_byte_its_model_whole_from_the_cache() {
    let f = opened(STORED);
    let first = f.model();
    // [`STORED`]'s lug, 10 × 10 × 10 (1000, area 600), and the fuse, both boxes as if
    // apart: 9000, area 3400.
    assert_eq!(
        lug_and_fuse(&first),
        [
            millionths(1_000_000_000, 600),
            millionths(9_000_000_000, 3400)
        ]
    );
    f.begin();
    assert_eq!(f.set("b0000001", "length", "20"), Revision(2));
    let (second_text, second) = (f.text(), f.model());
    // 20 × 10 × 10 = 2000, area 2(200 + 200 + 100) = 1000; the fuse 10000, 3800.
    assert_eq!(
        lug_and_fuse(&second),
        [
            millionths(2_000_000_000, 1000),
            millionths(10_000_000_000, 3800)
        ]
    );
    assert_eq!(f.set("b0000001", "length", "30"), Revision(3));
    let third_text = f.text();
    // 30 × 10 × 10 = 3000, area 2(300 + 300 + 100) = 1400; the fuse 11000, 4200.
    assert_eq!(
        lug_and_fuse(&f.model()),
        [
            millionths(3_000_000_000, 1400),
            millionths(11_000_000_000, 4200)
        ]
    );
    let built = f.calls();

    // Revision 2's state, as revision 4: its text, and its model, whole, made with no
    // kernel step.
    assert_eq!(stepped(f.undo(None)), (step(2, 1, 1, 0), Revision(4)));
    assert_eq!(f.text(), second_text);
    assert_eq!(whole(&f.model()), whole(&second));
    assert_eq!(
        f.calls(),
        built,
        "the restored state's rebuild asked the kernel"
    );
    // `begin`'s, as revision 5: the file's bytes, as they were (not canonical form).
    assert_eq!(stepped(f.undo(None)), (step(1, 0, 2, 0), Revision(5)));
    assert_eq!(f.text(), STORED);
    assert_eq!(whole(&f.model()), whole(&first));
    assert_eq!(
        f.calls(),
        built,
        "the restored state's rebuild asked the kernel"
    );
    // And forward again: revision 3's, as revision 7.
    assert_eq!(stepped(f.redo(None)), (step(2, 1, 1, 0), Revision(6)));
    assert_eq!(f.text(), second_text);
    assert_eq!(stepped(f.redo(None)), (step(3, 2, 0, 0), Revision(7)));
    assert_eq!(f.text(), third_text);
    assert_eq!(
        lug_and_fuse(&f.model()),
        [
            millionths(3_000_000_000, 1400),
            millionths(11_000_000_000, 4200)
        ]
    );
    assert_eq!(
        f.calls(),
        built,
        "the restored state's rebuild asked the kernel"
    );
    // The file was never touched.
    assert_eq!(f.file(), STORED);
}

#[test]
fn an_edit_after_an_undo_drops_the_redo_and_its_revisions_from_the_stack() {
    let f = opened(BOX);
    f.begin();
    f.set("k7f2qx3m", "length", "41");
    f.set("k7f2qx3m", "length", "42");
    assert_eq!(stepped(f.undo(None)), (step(2, 1, 1, 0), Revision(4)));
    assert_eq!(f.set("k7f2qx3m", "length", "43"), Revision(5));
    assert_eq!(f.text(), box_text(43));
    let (words_, candidates, revision) = refusal(f.redo(None));
    assert_eq!(
        words_,
        words(
            ErrorCode::InvalidInput,
            "nothing to redo in session s1's transaction",
            "session",
            Some("an edit after an undo drops what was undone"),
            "roll back to return to the state the transaction began from",
        )
    );
    assert!(candidates.is_empty());
    assert_eq!(revision, Revision(5));
    // Revision 3's state is gone, though the stack holds revisions on either side of it: 1
    // (`begin`'s), 2 and 4 (the first edit's, and its undo's), and 5.
    let (words_, candidates, _) = refusal(f.redo(Some(3)));
    assert_eq!(
        words_,
        words(
            ErrorCode::InvalidInput,
            "revision 3 isn't a state of session s1's undo stack, which holds revisions 1 to 5",
            "to",
            Some(
                "an edit after an undo drops what was undone, or the revision isn't one of the transaction's states the stack kept"
            ),
            "redo to a revision the stack holds, or roll back to the transaction's start",
        )
    );
    assert_eq!(candidates, ["5", "4", "2", "1"]);
    let (words_, candidates, _) = refusal(f.undo(Some(3)));
    assert_eq!(
        words_,
        words(
            ErrorCode::InvalidInput,
            "revision 3 isn't a state of session s1's undo stack, which holds revisions 1 to 5",
            "to",
            Some(
                "the stack dropped its oldest edits past its bound, or the revision is from before the transaction"
            ),
            "undo to a revision the stack holds, or roll back to the transaction's start",
        )
    );
    assert_eq!(candidates, ["5", "4", "2", "1"]);
    // The refusals changed nothing: two undos reach 41, then `begin`'s.
    assert_eq!(stepped(f.undo(None)), (step(2, 1, 1, 0), Revision(6)));
    assert_eq!(f.text(), box_text(41));
    assert_eq!(stepped(f.undo(None)), (step(1, 0, 2, 0), Revision(7)));
    assert_eq!(f.text(), BOX);
}

#[test]
fn an_edit_undone_reaches_nothing_so_a_commit_writes_only_what_the_standing_edits_reach() {
    // The block's stored fingerprint is 1e-6 over what it measures: an edit of it, undone,
    // must leave that drift said and its fingerprint as stored. The lug's edit, standing,
    // writes the lug's and the fuse's ([`COMMITTED`], by hand).
    let f = opened(STORED);
    f.begin();
    f.set("k7f2qx3m", "length", "50");
    assert_eq!(
        f.session().held.read().reach(),
        BTreeSet::from([id("f0000001"), id("k7f2qx3m")])
    );
    assert_eq!(stepped(f.undo(None)), (step(1, 0, 1, 0), Revision(3)));
    assert!(f.session().held.read().reach().is_empty());
    let replaced = f.sessions.replace_text(&f.id, Revision(3), longer()).wait();
    assert!(replaced.result.unwrap().changed);
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert!(committed.written);
    assert_eq!(committed.refingerprinted, [id("b0000001"), id("f0000001")]);
    the_blocks_drift(&committed.problems);
    assert_eq!(f.file(), COMMITTED);
}

#[test]
fn a_run_of_edits_undone_with_to_is_undone_as_one_and_redone_with_to() {
    // An inline edit (revision 2), then a dialog opened at 2 that edits twice (3, 4), then
    // its Cancel: undo to 2. The inline edit stays; both the dialog's go.
    let f = opened(STORED);
    f.begin();
    f.set("b0000001", "length", "20");
    let inline = f.text();
    f.set("b0000001", "length", "30");
    f.set("b0000001", "length", "40");
    let last = f.text();
    assert_eq!(stepped(f.undo(Some(2))), (step(2, 1, 2, 0), Revision(5)));
    assert_eq!(f.text(), inline);
    assert_eq!(
        lug_and_fuse(&f.model()),
        [
            millionths(2_000_000_000, 1000),
            millionths(10_000_000_000, 3800)
        ]
    );
    // Both redone at once.
    assert_eq!(stepped(f.redo(Some(4))), (step(4, 3, 0, 0), Revision(6)));
    assert_eq!(f.text(), last);
    // 40 × 10 × 10 = 4000, area 2(400 + 400 + 100) = 1800; the fuse 12000, 4600.
    assert_eq!(
        lug_and_fuse(&f.model()),
        [
            millionths(4_000_000_000, 1800),
            millionths(12_000_000_000, 4600)
        ]
    );
    // And all undone at once, to `begin`'s, by its revision: the file's bytes.
    assert_eq!(stepped(f.undo(Some(1))), (step(1, 0, 3, 0), Revision(7)));
    assert_eq!(f.text(), STORED);
}

#[test]
fn a_to_on_the_wrong_side_or_the_sessions_own_state_is_refused_naming_to() {
    let f = opened(BOX);
    f.begin();
    f.set("k7f2qx3m", "length", "41");
    f.set("k7f2qx3m", "length", "42");
    f.set("k7f2qx3m", "length", "43");
    // At revision 5, revision 2's state.
    assert_eq!(stepped(f.undo(Some(2))), (step(2, 1, 2, 0), Revision(5)));
    let undo_refused = |to: u64| {
        words(
            ErrorCode::InvalidInput,
            &format!("revision {to} isn't before session s1's state now, so undo can't restore it"),
            "to",
            Some(&format!(
                "undo restores an earlier state: revision {to} is the session's state now, or one after it"
            )),
            "undo to an earlier revision the stack holds, or redo to a later one",
        )
    };
    let redo_refused = |to: u64| {
        words(
            ErrorCode::InvalidInput,
            &format!("revision {to} isn't after session s1's state now, so redo can't restore it"),
            "to",
            Some(&format!(
                "redo restores a later state: revision {to} is the session's state now, or one before it"
            )),
            "redo to a later revision the stack holds, or undo to an earlier one",
        )
    };
    // A later state, the session's own revision, and the earlier revision whose state it
    // is: none is before it.
    for to in [3, 4, 5, 2] {
        let (said, candidates, revision) = refusal(f.undo(Some(to)));
        assert_eq!(said, undo_refused(to));
        assert!(candidates.is_empty());
        assert_eq!(revision, Revision(5));
    }
    // An earlier state, the session's own revision, and the earlier revision whose state it
    // is: none is after it.
    for to in [1, 5, 2] {
        let (said, candidates, revision) = refusal(f.redo(Some(to)));
        assert_eq!(said, redo_refused(to));
        assert!(candidates.is_empty());
        assert_eq!(revision, Revision(5));
    }
    assert_eq!(f.text(), box_text(41));
    // Nothing changed: one undo, and three redos, are left.
    assert_eq!(stepped(f.redo(Some(4))), (step(4, 3, 0, 0), Revision(6)));
    assert_eq!(f.text(), box_text(43));
}

#[test]
fn past_its_bound_the_stack_drops_its_oldest_states_counted_by_hand_and_undo_stops_at_the_first_kept()
 {
    // Each edit's state shares nothing with the one before (its one chunk changed), so
    // after n edits the stack counts the file's bytes and n − 1 states. 800 holds three:
    // 212 + 3 × 174 = 734, and a fourth would be 908.
    let f = opened_bounded(BOX, 800);
    f.begin();
    let mut counted = Vec::new();
    for length in 41..=46 {
        f.set("k7f2qx3m", "length", &length.to_string());
        let stack = f.session();
        let held = stack.held.read();
        let stack = held.undo.as_ref().unwrap();
        counted.push((stack.bytes(), stack.dropped(), stack.undo_left()));
    }
    assert_eq!(
        counted,
        [
            (BOX_BYTES, 0, 1),
            (BOX_BYTES + BOX_STATE, 0, 2),
            (BOX_BYTES + 2 * BOX_STATE, 0, 3),
            (BOX_BYTES + 3 * BOX_STATE, 0, 4),
            // The fifth edit passes it: the first edit's state (41) dropped, so neither it
            // nor `begin`'s can be reached: two edits out of reach.
            (BOX_BYTES + 3 * BOX_STATE, 2, 3),
            // The sixth drops the second's (42): one more.
            (BOX_BYTES + 3 * BOX_STATE, 3, 3),
        ]
    );
    // Revisions 1 (`begin`'s), 2 and 3 (41's and 42's) are out of reach; 4 to 7 are held.
    for to in [1, 3] {
        let (said, candidates, revision) = refusal(f.undo(Some(to)));
        assert_eq!(
            said,
            words(
                ErrorCode::InvalidInput,
                &format!(
                    "revision {to} isn't a state of session s1's undo stack, which holds revisions 4 to 7"
                ),
                "to",
                Some(
                    "the stack dropped its oldest edits past its bound, or the revision is from before the transaction"
                ),
                "undo to a revision the stack holds, or roll back to the transaction's start",
            )
        );
        assert_eq!(candidates, ["7", "6", "5", "4"]);
        assert_eq!(revision, Revision(7));
    }
    // Three undos land on the first kept state, 43's; a fourth is refused.
    assert_eq!(stepped(f.undo(None)), (step(6, 2, 1, 0), Revision(8)));
    assert_eq!(stepped(f.undo(None)), (step(5, 1, 2, 0), Revision(9)));
    assert_eq!(stepped(f.undo(None)), (step(4, 0, 3, 0), Revision(10)));
    assert_eq!(f.text(), box_text(43));
    assert_eq!(f.stack_bytes(), BOX_BYTES + 3 * BOX_STATE);
    let (said, _, _) = refusal(f.undo(None));
    assert_eq!(said.1, "nothing to undo in session s1's transaction");
    // A rollback still reaches `begin`'s.
    f.sessions.rollback(&f.id).wait().result.unwrap();
    assert_eq!(f.text(), BOX);
}

#[test]
fn a_session_bounds_its_undo_stack_by_the_designs_256_mib() {
    let f = opened(BOX);
    assert_eq!(f.session().undo_bytes, 256 * 1024 * 1024);
    assert_eq!(UNDO_BYTES, 256 * 1024 * 1024);
}

#[test]
fn a_commit_or_a_rollback_empties_the_stack() {
    let no_transaction = |way: &str| {
        words(
            ErrorCode::InvalidInput,
            "no transaction is open in session s1",
            "session",
            Some(&format!(
                "{way} steps through a transaction's edits: outside one, each edit was saved as it was made"
            )),
            &format!("edit inside a transaction to {way}; a saved file's history is git's"),
        )
    };
    let f = opened(BOX);
    f.begin();
    f.set("k7f2qx3m", "length", "41");
    assert!(f.sessions.commit(&f.id).wait().result.unwrap().written);
    assert!(f.session().held.read().undo.is_none());
    assert_eq!(refusal(f.undo(None)).0, no_transaction("undo"));
    assert_eq!(refusal(f.redo(None)).0, no_transaction("redo"));
    // A new transaction's stack starts at its own `begin`: the committed edit isn't in it.
    f.begin();
    let (said, _, _) = refusal(f.undo(None));
    assert_eq!(said.1, "nothing to undo in session s1's transaction");
    f.set("k7f2qx3m", "length", "42");
    f.sessions.rollback(&f.id).wait().result.unwrap();
    assert!(f.session().held.read().undo.is_none());
    assert_eq!(refusal(f.undo(None)).0, no_transaction("undo"));
    assert_eq!(refusal(f.redo(None)).0, no_transaction("redo"));
    // A rollback restores `begin`'s text, the committed one, with the block's fingerprint:
    // 41 × 20 × 10 = 8200, area 2(820 + 410 + 200) = 2860.
    assert_eq!(
        f.text(),
        "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 41,\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 8200, \"area\": 2860, \"bbox\": [0, 0, 0, 41, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n"
    );
}

#[test]
fn a_replace_text_of_text_not_in_canonical_form_comes_back_through_undo_and_redo_byte_for_byte() {
    // #261's first item, through the command layer's own choice: the edit holds its
    // document's canonical form (M4.1, section 2: the file's bytes until an edit, then the
    // canonical form), so the stack keeps it in chunks, and undone and redone it's back as
    // the session held it. `begin`'s state, the file's own bytes, isn't canonical form, and
    // comes back as it was.
    let f = opened(BOX);
    f.begin();
    let spaced = format!(
        "\n{}\n\n",
        BOX.replace("\"length\": 40", "\"length\":    41")
            .replace(", \"", ",\n  \"")
    );
    let replaced = f.sessions.replace_text(&f.id, Revision(1), spaced).wait();
    assert!(replaced.result.unwrap().changed);
    assert_eq!(f.text(), box_text(41));
    assert_eq!(stepped(f.undo(None)), (step(1, 0, 1, 0), Revision(3)));
    assert_eq!(f.text(), BOX);
    // Held for redo in chunks: its list and one chunk, not its 231 bytes whole and 24.
    assert_eq!(f.stack_bytes(), BOX_BYTES + BOX_STATE);
    assert_eq!(stepped(f.redo(None)), (step(2, 1, 0, 0), Revision(4)));
    assert_eq!(f.text(), box_text(41));
    assert_eq!(f.stack_bytes(), BOX_BYTES);
}

#[test]
fn undo_and_redo_are_refused_with_the_designs_words() {
    let f = opened(BOX);
    // No transaction.
    let (said, candidates, revision) = refusal(f.undo(None));
    assert_eq!(
        said,
        words(
            ErrorCode::InvalidInput,
            "no transaction is open in session s1",
            "session",
            Some(
                "undo steps through a transaction's edits: outside one, each edit was saved as it was made"
            ),
            "edit inside a transaction to undo; a saved file's history is git's",
        )
    );
    assert!(candidates.is_empty());
    assert_eq!(revision, Revision(1));
    // Nothing to undo, nor to redo.
    f.begin();
    let nothing = |way: &str, cause: Option<&str>| {
        words(
            ErrorCode::InvalidInput,
            &format!("nothing to {way} in session s1's transaction"),
            "session",
            cause,
            "roll back to return to the state the transaction began from",
        )
    };
    assert_eq!(refusal(f.undo(None)).0, nothing("undo", None));
    assert_eq!(
        refusal(f.redo(None)).0,
        nothing("redo", Some("an edit after an undo drops what was undone"))
    );
    // A stale revision: a conflict, each refused before the stack is asked.
    f.set("k7f2qx3m", "length", "41");
    for (way, response) in [
        ("undo", f.sessions.undo(&f.id, Revision(1), None).wait()),
        (
            "redo",
            f.sessions
                .redo(&f.id, Revision(1), Some(Revision(2)))
                .wait(),
        ),
    ] {
        let (said, candidates, revision) = refusal(response);
        assert_eq!(
            said,
            (
                ErrorCode::Conflict,
                format!("the {way} was made against revision 1, and the session is at revision 2"),
                Some("revision".to_owned()),
                None,
                Some(format!(
                    "read the document again (text or outline), then {way} against revision 2"
                )),
            )
        );
        assert!(candidates.is_empty());
        assert_eq!(revision, Revision(2));
    }
    assert_eq!(f.text(), box_text(41));
}

#[test]
fn a_not_held_refusal_lists_the_newest_hundred_revisions_newest_first() {
    // An edit, then 110 steps back and forth: every revision from 1 to 112 is a state the
    // stack holds, `begin`'s or the edit's.
    let f = opened(BOX);
    f.begin();
    f.set("k7f2qx3m", "length", "41");
    for _ in 0..55 {
        f.undo(None).result.unwrap();
        f.redo(None).result.unwrap();
    }
    assert_eq!(f.revision(), Revision(112));
    let (said, candidates, _) = refusal(f.undo(Some(500)));
    assert_eq!(
        said.1,
        "revision 500 isn't a state of session s1's undo stack, which holds revisions 1 to 112"
    );
    let newest: Vec<String> = (13..=112).rev().map(|n: u64| n.to_string()).collect();
    assert_eq!(candidates, newest);
}

#[test]
fn an_undo_is_named_by_status_and_stopped_by_cancel_before_it_changes_anything() {
    let f = opened(BOX);
    f.begin();
    f.set("k7f2qx3m", "length", "41");
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().gate.shut(started, wait);
    let undo = f.sessions.undo(&f.id, Revision(2), None);
    was_started.recv_timeout(WAIT).unwrap();
    assert_eq!(
        f.sessions.status(&f.id).result.unwrap().running,
        Some(SessionCommand::Undo)
    );
    assert_eq!(
        f.sessions
            .cancel(&f.id, SessionCommand::Undo)
            .result
            .unwrap(),
        Cancelled {
            command: SessionCommand::Undo,
            stopped: true
        }
    );
    through.send(()).unwrap();
    let (said, _, revision) = refusal(undo.wait());
    assert_eq!(said.0, ErrorCode::Cancelled);
    assert_eq!(revision, Revision(2));
    *lock(&f.session().gate.0) = None;
    // The stack is as it was: one undo left.
    assert_eq!(stepped(f.undo(None)), (step(1, 0, 1, 0), Revision(3)));
    assert_eq!(SessionCommand::Undo.as_str(), "undo");
    assert_eq!(SessionCommand::Redo.as_str(), "redo");
    assert_eq!(SessionCommand::Undo.command(), None);
    assert_eq!(SessionCommand::Check.command(), Some(Command::Check));
}
