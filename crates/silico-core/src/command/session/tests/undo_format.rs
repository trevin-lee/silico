//! A `format` inside a transaction on its undo stack (#311's review): what it put into the
//! document (that it asked for canonical form, and the solves it wrote) kept with each
//! state, given back by an undo or a redo, dropped with the state past the stack's bound,
//! and counted in its bytes once however many states share it. Expected values: the
//! design's rules (a format reaches nothing; `changed` false means a commit writes
//! nothing; A2's count), worked by hand from [`BOX`] and its canonical text.

use super::*;
use crate::document::json::Node;

/// [`BOX`]'s feature as a format writes it (its fingerprint with it, by formula: 40 ×
/// 20 × 10), with a two-digit `length` (an edit after the format keeps that fingerprint, as
/// only a commit writes one anew), placed as canonical form places it: written by hand from
/// M2.1's layout.
fn feature(length: u32) -> String {
    format!(
        "    {{\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": {length},\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {{\"volume\": 8000, \"area\": 2800, \"center\": [20, 10, 5], \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}}\n    }}"
    )
}

/// What A2 counts for a state after the format: its list of one chunk (24 and 8), and the
/// chunk (16 and its feature's bytes, the same for every two-digit length).
fn state() -> usize {
    24 + 8 + 16 + feature(40).len()
}

/// What a format that wrote no solve counts for on the stack: its own fields alone.
const FORMAT: usize = 32;

fn bounded(text: &str, bound: usize, sketches: &crate::fake::FakeSketches) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("a.silico");
    std::fs::write(&path, text).unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let solver = sketches.clone();
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

impl Fixture {
    fn revision_now(&self) -> Revision {
        self.sessions.status(&self.id).session.unwrap().revision
    }

    fn transaction_now(&self) -> Transaction {
        self.sessions
            .status(&self.id)
            .result
            .unwrap()
            .transaction
            .unwrap()
    }

    fn set_length(&self, length: u32) {
        let value: Node = crate::document::json::read(length.to_string().as_bytes()).unwrap();
        let set = self
            .sessions
            .set_field(
                &self.id,
                self.revision_now(),
                id("k7f2qx3m"),
                "length".to_owned(),
                value,
            )
            .wait();
        assert!(set.result.unwrap().changed);
    }

    fn undo_once(&self) -> Response<Stepped> {
        self.sessions
            .undo(&self.id, self.revision_now(), None)
            .wait()
    }

    fn redo_once(&self) -> Response<Stepped> {
        self.sessions
            .redo(&self.id, self.revision_now(), None)
            .wait()
    }

    fn text_now(&self) -> String {
        String::from_utf8(self.session().held.read().text.to_vec()).unwrap()
    }
}

#[test]
fn a_solve_a_format_wrote_goes_with_its_undo_and_comes_back_with_its_redo() {
    // The sketch's pb, stored at 10, solves to 11: the format writes the solve, which
    // reaches nothing (A6). Undone, the document is the file's again: nothing to commit.
    // Redone, the solve is the format's again: its commit writes it, reaching nothing, so
    // re-solving nothing (`resolved` empty) and refingerprinting nothing.
    let sketches = crate::fake::FakeSketches::new();
    sketches.answer(|system| {
        let mut solved = crate::fake::FakeSketches::as_sent(system);
        if solved.values[8] == 10.0 {
            solved.values[8] += 1.0;
        }
        Ok(solved)
    });
    let file = sketch_and_box("10");
    let f = bounded(&file, crate::document::undo::UNDO_BYTES, &sketches);
    f.begin();
    let formatted = f.sessions.format(&f.id, None, true).wait().result.unwrap();
    assert!(formatted.changed);
    assert_eq!(formatted.resolved, [id("s0000001")]);
    let after_format = f.text_now();
    assert_eq!(pb_x(&after_format, "s0000001"), 11.0);
    assert!(f.undo_once().result.is_ok());
    assert_eq!(f.text_now(), file);
    assert!(!f.transaction_now().changed);
    assert!(f.redo_once().result.is_ok());
    assert_eq!(f.text_now(), after_format);
    assert!(f.transaction_now().changed);
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert!(committed.written);
    assert_eq!(committed.resolved, Vec::<FeatureId>::new());
    assert_eq!(committed.refingerprinted, Vec::<FeatureId>::new());
    assert_eq!(pb_x(&f.file(), "s0000001"), 11.0);
}

#[test]
fn a_format_is_counted_once_on_the_stack_and_its_edits_keep_it_past_its_states_drop() {
    // BOX on one line, so its canonical form is a change only a format's byte compare
    // sees. Formatted (F), then 41, 42, 41, and 40: canonical form of the file's document
    // again, the format still in force. Counted: `begin`'s bytes, each state but the
    // session's, and the format once, however many of them share it.
    let begin = BOX.len();
    let f = bounded(
        BOX,
        crate::document::undo::UNDO_BYTES,
        &crate::fake::FakeSketches::new(),
    );
    f.begin();
    f.sessions.format(&f.id, None, true).wait().result.unwrap();
    // F the session's: not counted, nor its format.
    assert_eq!(f.transaction_now().bytes, begin);
    f.set_length(41);
    // F held, its format with it.
    assert_eq!(f.transaction_now().bytes, begin + state() + FORMAT);
    f.set_length(42);
    // F and 41's held, sharing the format: counted once.
    assert_eq!(f.transaction_now().bytes, begin + 2 * state() + FORMAT);
    // Undone to `begin`'s (always counted, holding no format): the three states after it
    // held for redo, sharing the format, counted once.
    assert!(
        f.sessions
            .undo(&f.id, f.revision_now(), Some(Revision(1)))
            .wait()
            .result
            .is_ok()
    );
    assert!(!f.transaction_now().changed);
    assert_eq!(f.transaction_now().bytes, begin + 3 * state() + FORMAT);
    // An edit there drops the redo states, and the format with them.
    f.set_length(43);
    assert_eq!(f.transaction_now().bytes, begin);
}

#[test]
fn a_format_dropped_past_the_bound_stays_in_force_in_the_edits_after_it() {
    // Bounded so the stack holds `begin`'s and two states besides: formatted, then 41,
    // 42, 41, 40, the oldest states (the format's among them) dropped as each comes. The
    // format is still in force in those left: undone to the oldest reachable, 42 or 41,
    // and redone to 40, canonical form of the file's document, it's a change, which the
    // commit writes.
    let begin = BOX.len();
    let bound = begin + 2 * state() + FORMAT;
    let f = bounded(BOX, bound, &crate::fake::FakeSketches::new());
    f.begin();
    f.sessions.format(&f.id, None, true).wait().result.unwrap();
    let canonical = f.text_now();
    assert_ne!(canonical, BOX);
    for length in [41, 42, 41, 40] {
        f.set_length(length);
    }
    let t = f.transaction_now();
    assert!(t.dropped > 0, "{t:?}");
    assert!(t.bytes <= bound, "{t:?}");
    assert_eq!(f.text_now(), canonical);
    assert!(t.changed);
    while f.undo_once().result.is_ok() {}
    assert!(f.transaction_now().changed);
    while f.redo_once().result.is_ok() {}
    assert_eq!(f.text_now(), canonical);
    assert!(f.transaction_now().changed);
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert!(committed.written);
    assert_eq!(f.file(), canonical);
}

#[test]
fn two_formats_on_the_stack_are_counted_apart() {
    // Formatted (its record A), the text replaced on one line again (41 long, taking A),
    // formatted again (a record of its own, B: A is shared with the states before, so the
    // format's change makes a copy), then set to 42: the second format's state held, its
    // record with it, counted beside A, not as A.
    let f = bounded(
        BOX,
        crate::document::undo::UNDO_BYTES,
        &crate::fake::FakeSketches::new(),
    );
    f.begin();
    f.sessions.format(&f.id, None, true).wait().result.unwrap();
    let one_line = BOX.replace(r#""length": 40"#, r#""length": 41"#);
    f.sessions
        .replace_text(&f.id, f.revision_now(), one_line)
        .wait()
        .result
        .unwrap();
    let again = f.sessions.format(&f.id, None, true).wait().result.unwrap();
    assert!(again.changed);
    let before = f.transaction_now().bytes;
    f.set_length(42);
    // The second format's state (41 long, its fingerprint as rebuilt: 8200, 2860, and its
    // box, each as many digits as 40's, and its centre's x 20.5, two more than 20's) held,
    // and its record: a second FORMAT.
    assert_eq!(f.transaction_now().bytes, before + state() + 2 + FORMAT);
}

#[test]
fn the_unit_extra_shares_nothing_and_costs_nothing() {
    use crate::document::undo::Extra as _;
    assert_eq!(().shared(), None);
    assert_eq!(().cost(), 0);
}
