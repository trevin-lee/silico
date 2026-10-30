//! A transaction's undo stack (`document::undo`: the M5.0 design's section 3, A2, section
//! 6's bound, and section 7's `undo` and `redo`; pending Trevin, decision 2): a run of
//! edits undone one by one gives back each earlier state's text, byte for byte (its
//! fingerprints, rebuilt from it, the ones it had: the same text's, which the command's
//! tests compare with a model rebuilt from the cache); redo gives back the later ones; an edit after an undo
//! drops them; `to` reaches a revision the stack holds and refuses any other; and the
//! bound counts what the states hold, worked out by hand, dropping the oldest past it.

use super::example::EXAMPLE;
use super::model::{checked, id, published};

use silico_core::document::json::MAX_DOCUMENT_LEN;
use silico_core::document::undo::{
    After, CHUNK_BYTES, ENTRY_BYTES, LIST_BYTES, Prepared, Pushed, Refusal, UNDO_BYTES, UndoStack,
    WHOLE_BYTES,
};
use silico_core::document::{Document, Name, read, write};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::Recompute;
use std::sync::Arc;

/// A stack begun on `bytes`.
fn begun(bytes: &[u8], revision: u64, bound: usize) -> UndoStack<u64> {
    UndoStack::new(Arc::new(bytes.to_vec()), revision, bound)
}

/// An edit's state, as these tests choose it: `document`'s features if `text` is its
/// canonical form, the text whole if not, or if there's no document. The stack keeps what
/// it's given (see `a_canonical_state_is_restored_as_its_features_write_it`); choosing is
/// the command layer's, whose own code the session's tests check.
fn after(text: &[u8], document: Option<&Document>) -> After {
    match document.and_then(|d| Prepared::of(d).ok()) {
        Some(prepared) if prepared.text().as_bytes() == text => After::Canonical(prepared),
        _ => After::Whole(Arc::new(text.to_vec())),
    }
}

/// An edit of `text`, as [`after`] gives its state.
trait EditText {
    fn edit_text(&mut self, text: &[u8], document: Option<&Document>, revision: u64) -> Pushed;
}

impl EditText for UndoStack<u64> {
    fn edit_text(&mut self, text: &[u8], document: Option<&Document>, revision: u64) -> Pushed {
        self.edit(after(text, document), revision)
    }
}

/// `text` with `from` put to `to`, once, read and written in canonical form: an edit's
/// text, as a typed edit writes it.
fn edited(text: &str, from: &str, to: &str) -> (String, Document) {
    assert_eq!(text.matches(from).count(), 1, "{from}");
    let document = read(text.replacen(from, to, 1).as_bytes()).unwrap();
    (write(&document).unwrap(), document)
}

/// Each feature's fingerprint, by ID, in `text`'s rebuild on the fake kernel.
fn fingerprints(text: &str) -> Vec<(String, String)> {
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let model = published(&engine, checked(text));
    let document = read(text.as_bytes()).unwrap();
    document
        .features
        .iter()
        .map(|f| {
            (
                f.id.to_string(),
                format!("{:?}", model.fingerprint(&id(f.id.as_str()))),
            )
        })
        .collect()
}

/// The example's begin, and three typed edits of it, each text in canonical form, and a
/// fourth text that reads as the third's document but isn't its canonical form (a
/// `replace_text` that spaced it out): five states, the revisions 10 to 14.
fn run() -> Vec<(String, Option<Document>)> {
    let begin = EXAMPLE.to_owned();
    let (one, d1) = edited(&begin, r#""length": 40"#, r#""length": 41"#);
    let (two, d2) = edited(&one, r#""radius": 2.5"#, r#""radius": 3"#);
    let (three, d3) = edited(&two, r#""name": "Hole""#, r#""name": "Bolt hole""#);
    let spaced = three.replace(r#""v": 1"#, r#""v":   1"#);
    let d4 = read(spaced.as_bytes()).unwrap();
    assert!(d4.canonically_eq(&d3) && spaced != three);
    vec![
        (begin, None),
        (one, Some(d1)),
        (two, Some(d2)),
        (three, Some(d3)),
        (spaced, Some(d4)),
    ]
}

/// A stack over `run()`'s states, each edit pushed at its revision with its text's index
/// as what the edits reached.
fn pushed(states: &[(String, Option<Document>)], bound: usize) -> UndoStack<u64> {
    let mut stack = begun(states[0].0.as_bytes(), 10, bound);
    for (i, (text, document)) in states.iter().enumerate().skip(1) {
        let pushed = stack.edit_text(text.as_bytes(), document.as_ref(), 10 + i as u64);
        assert_eq!(pushed, Pushed { dropped: 0 });
    }
    stack
}

#[test]
fn a_run_of_edits_undone_one_by_one_gives_back_each_earlier_state_byte_for_byte() {
    let states = run();
    // Each state's fingerprints as its model had them while it was the session's, before
    // anything is undone: the box's length and the pin's radius change them, so a restore
    // of a neighbouring state has others.
    let live: Vec<_> = states.iter().map(|(text, _)| fingerprints(text)).collect();
    assert_ne!(live[0], live[1]);
    assert_ne!(live[1], live[2]);
    let mut stack = pushed(&states, UNDO_BYTES);
    assert_eq!((stack.undo_left(), stack.redo_left()), (4, 0));
    let mut revision = 14;
    for back in (0..4).rev() {
        revision += 1;
        let restored = stack.undo(None, revision).unwrap();
        let (text, _) = &states[back];
        assert_eq!(restored.restored, 10 + back as u64);
        assert_eq!(String::from_utf8(restored.text.to_vec()).unwrap(), *text);
        assert_eq!((restored.undo, restored.redo), (back, 4 - back));
        // Rebuilt from the text restored, the model the state had. (Its element maps, and
        // that the rebuild runs no kernel step, its steps in the cache, are the command's
        // to show, which holds the session's cache.)
        assert_eq!(
            fingerprints(std::str::from_utf8(&restored.text).unwrap()),
            live[back],
            "state {back}"
        );
    }
    assert_eq!(stack.undo(None, 19), Err(Refusal::NothingToUndo));
    // And redone one by one, each later state again, the spaced-out text as it was.
    for (forward, (text, _)) in states.iter().enumerate().skip(1) {
        revision += 1;
        let restored = stack.redo(None, revision).unwrap();
        assert_eq!(restored.restored, 10 + forward as u64);
        assert_eq!(
            String::from_utf8(restored.text.to_vec()).unwrap(),
            *text,
            "state {forward}"
        );
        assert_eq!((restored.undo, restored.redo), (forward, 4 - forward));
    }
    assert_eq!(stack.redo(None, 24), Err(Refusal::NothingToRedo));
}

#[test]
fn an_edit_after_an_undo_drops_what_was_undone() {
    let states = run();
    let mut stack = pushed(&states, UNDO_BYTES);
    stack.undo(Some(12), 15).unwrap();
    assert_eq!((stack.undo_left(), stack.redo_left()), (2, 2));
    let (other, document) = edited(&states[2].0, r#""height": 12"#, r#""height": 13"#);
    stack.edit_text(other.as_bytes(), Some(&document), 16);
    assert_eq!((stack.undo_left(), stack.redo_left()), (3, 0));
    assert_eq!(stack.redo(None, 17), Err(Refusal::NothingToRedo));
    // The undone revisions are no state of the stack's now. Undo names those it can
    // restore: the states before the session's (16), 10, 11, and 12's (at 12 and 15).
    assert_eq!(
        stack.undo(Some(13), 17),
        Err(Refusal::NotHeld {
            to: 13,
            held: vec![10, 11, 12, 15]
        })
    );
    // Undone, the edit gives back the state it was made from: revision 12's, restored at
    // 15, which `to` reaches by either.
    let back = stack.undo(None, 17).unwrap();
    assert_eq!(
        (back.restored, back.text.as_slice()),
        (12, states[2].0.as_bytes())
    );
    let redone = stack.redo(Some(16), 18).unwrap();
    assert_eq!(redone.text.as_slice(), other.as_bytes());
    assert_eq!(stack.undo(Some(15), 19).unwrap().restored, 12);
}

#[test]
fn to_reaches_a_revision_the_stack_holds_and_refuses_any_other() {
    let states = run();
    let mut stack = pushed(&states, UNDO_BYTES);
    // An undo to a later revision, or to the session's own, is on the wrong side.
    assert_eq!(stack.undo(Some(14), 15), Err(Refusal::WrongSide { to: 14 }));
    // One the stack never held: undo names the states before the session's (14).
    assert_eq!(
        stack.undo(Some(9), 15),
        Err(Refusal::NotHeld {
            to: 9,
            held: vec![10, 11, 12, 13]
        })
    );
    let restored = stack.undo(Some(11), 15).unwrap();
    assert_eq!(
        (restored.restored, restored.undo, restored.redo),
        (11, 1, 3)
    );
    assert_eq!(restored.text.as_slice(), states[1].0.as_bytes());
    // A redo to an earlier one, or to the state the session is at (made at 11, restored at
    // 15), is too.
    assert_eq!(stack.redo(Some(10), 16), Err(Refusal::WrongSide { to: 10 }));
    assert_eq!(stack.redo(Some(15), 16), Err(Refusal::WrongSide { to: 15 }));
    assert_eq!(stack.redo(Some(11), 16), Err(Refusal::WrongSide { to: 11 }));
    let restored = stack.redo(Some(13), 16).unwrap();
    assert_eq!(
        (restored.restored, restored.undo, restored.redo),
        (13, 3, 1)
    );
    assert_eq!(restored.text.as_slice(), states[3].0.as_bytes());
    // Back to `begin`'s state: its bytes, as the file held them.
    let restored = stack.undo(Some(10), 17).unwrap();
    assert_eq!(restored.text.as_slice(), EXAMPLE.as_bytes());
}

// ---------------------------------------------------------------------------------------
// The bound, by hand.

#[test]
fn the_bound_and_what_it_counts_are_the_design_s_numbers() {
    // Section 6 and A2: 256 MiB; a state's list 24 bytes and 8 a chunk; a chunk 16 bytes
    // beyond its texts, of 64 features; a whole text counted as a list of none.
    assert_eq!(UNDO_BYTES, 268_435_456);
    assert_eq!((LIST_BYTES, ENTRY_BYTES, CHUNK_BYTES), (24, 8, 16));
    assert_eq!(silico_core::document::undo::CHUNK, 64);
    assert_eq!(WHOLE_BYTES, 24);
}

/// A box of the document `boxes` makes, as its canonical form places it: indented four
/// spaces, on one line, its length `length`.
fn placed(n: usize, length: &str) -> String {
    format!(
        r#"    {{"id": "b{n:07}", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": {length}, "width": 10, "height": 10}}"#
    )
}

/// A document of 130 boxes, in three chunks (64, 64, 2), every length 10 but those named,
/// 105 (a byte more), in canonical form, written out by hand.
fn boxes(longer: &[usize]) -> (String, Document) {
    let features: Vec<String> = (0..130)
        .map(|n| placed(n, if longer.contains(&n) { "105" } else { "10" }))
        .collect();
    let text = format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n{}\n  ]\n}}\n",
        features.join(",\n")
    );
    let document = read(text.as_bytes()).unwrap();
    assert_eq!(
        write(&document).unwrap(),
        text,
        "the fixture isn't canonical"
    );
    (text, document)
}

#[test]
fn the_bound_counts_each_state_s_list_and_each_chunk_once_by_hand() {
    let l = placed(0, "10").len();
    let list = LIST_BYTES + 3 * ENTRY_BYTES;
    // A chunk of `n` boxes, `longer` of them length 105, one byte more each.
    let chunk = |n: usize, longer: usize| n * l + longer + CHUNK_BYTES;
    let (begin, _) = boxes(&[]);
    let f = begin.len();
    let mut stack = begun(begin.as_bytes(), 1u64, UNDO_BYTES);
    assert_eq!(stack.bytes(), f);
    // Box 0 longer: the state before (begin's) was counted already.
    let (one, d1) = boxes(&[0]);
    stack.edit_text(one.as_bytes(), Some(&d1), 2);
    assert_eq!(stack.bytes(), f);
    // Box 100 longer: the state before, the first after begin's, is charged in full.
    let (two, d2) = boxes(&[0, 100]);
    stack.edit_text(two.as_bytes(), Some(&d2), 3);
    let first = list + chunk(64, 1) + chunk(64, 0) + chunk(2, 0);
    assert_eq!(stack.bytes(), f + first);
    // Box 129 longer: the state before shares its first and last chunks with the first's,
    // so it adds its list and its middle chunk.
    let (three, d3) = boxes(&[0, 100, 129]);
    stack.edit_text(three.as_bytes(), Some(&d3), 4);
    let second = list + chunk(64, 1);
    assert_eq!(stack.bytes(), f + first + second);
    // Undone: the session's state is the second's (no longer counted), and the third is
    // held for redo, its first chunk the first state's (counted already), its middle the
    // second's (not counted now), its last its own.
    stack.undo(None, 5).unwrap();
    assert_eq!(stack.bytes(), f + first + list + chunk(64, 1) + chunk(2, 1));
    // An edit drops the redo state, and holds the second again.
    let (other, d4) = boxes(&[0, 1, 100]);
    stack.edit_text(other.as_bytes(), Some(&d4), 6);
    assert_eq!(stack.bytes(), f + first + second);
    // A text held whole, not the canonical form: its bytes and a list's, once it's held.
    let spaced = other.replacen("\"v\": 1", "\"v\":  1", 1);
    let d5 = read(spaced.as_bytes()).unwrap();
    stack.edit_text(spaced.as_bytes(), Some(&d5), 7);
    let fourth = list + chunk(64, 2);
    assert_eq!(stack.bytes(), f + first + second + fourth);
    stack.edit_text(other.as_bytes(), Some(&d4), 8);
    assert_eq!(
        stack.bytes(),
        f + first + second + fourth + spaced.len() + WHOLE_BYTES
    );
}

#[test]
fn past_the_bound_the_oldest_state_after_begin_is_dropped_and_counted() {
    let l = placed(0, "10").len();
    let list = LIST_BYTES + 3 * ENTRY_BYTES;
    // Each state below changes box 0 alone (its length 101 to 106, a byte longer than 10), so
    // each after the first adds its list and its first chunk.
    let step = list + 64 * l + 1 + CHUNK_BYTES;
    let (begin, _) = boxes(&[]);
    let f = begin.len();
    let first = list + (64 * l + 1 + CHUNK_BYTES) + (64 * l + CHUNK_BYTES) + (2 * l + CHUNK_BYTES);
    // Room for begin, the first state, and two steps; not a third.
    let bound = f + first + 2 * step;
    let mut stack = begun(begin.as_bytes(), 1u64, bound);
    let length = |k: usize| {
        let text = begin.replacen(
            r#""id": "b0000000", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10"#,
            &format!(r#""id": "b0000000", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10{k}"#),
            1,
        );
        let document = read(text.as_bytes()).unwrap();
        (text, document)
    };
    for k in 1..=4 {
        let (text, document) = length(k);
        let pushed = stack.edit_text(text.as_bytes(), Some(&document), 1 + k as u64);
        assert_eq!(pushed, Pushed { dropped: 0 }, "edit {k}");
    }
    assert_eq!(stack.bytes(), f + first + 2 * step);
    assert_eq!(stack.undo_left(), 4);
    // The fifth edit passes the bound: the first state after begin's goes, and undo
    // reaches neither it nor begin's, two edits out of its reach.
    let (text, document) = length(5);
    assert_eq!(
        stack.edit_text(text.as_bytes(), Some(&document), 6),
        Pushed { dropped: 2 }
    );
    assert_eq!(stack.dropped(), 2);
    // The second state, now the oldest after begin's, shares no chunk with a state held
    // before it, so its middle and last chunks count now, which were the first's.
    let second_alone =
        list + (64 * l + 1 + CHUNK_BYTES) + (64 * l + CHUNK_BYTES) + (2 * l + CHUNK_BYTES);
    assert_eq!(stack.bytes(), f + second_alone + 2 * step);
    assert_eq!(stack.undo_left(), 3);
    // A sixth drops one more.
    let (text, document) = length(6);
    assert_eq!(
        stack.edit_text(text.as_bytes(), Some(&document), 7),
        Pushed { dropped: 1 }
    );
    assert_eq!(stack.dropped(), 3);
    assert_eq!(stack.bytes(), f + second_alone + 2 * step);
    assert_eq!(stack.undo_left(), 3);
    // A dropped revision is refused, as is begin's, naming what undo can restore: the
    // states kept before the session's (7).
    assert_eq!(
        stack.undo(Some(2), 8),
        Err(Refusal::NotHeld {
            to: 2,
            held: vec![4, 5, 6]
        })
    );
    assert_eq!(
        stack.undo(Some(1), 8),
        Err(Refusal::NotHeld {
            to: 1,
            held: vec![4, 5, 6]
        })
    );
    // Undo stops at the oldest kept, revision 4's state.
    let mut revision = 8;
    for _ in 0..3 {
        stack.undo(None, revision).unwrap();
        revision += 1;
    }
    assert_eq!(stack.undo(None, revision), Err(Refusal::NothingToUndo));
    assert_eq!(stack.undo_left(), 0);
    // And there, a dropped revision is still refused as not held, not as there being
    // nothing to undo: with nothing before the session's state, undo can restore none.
    assert_eq!(
        stack.undo(Some(2), revision),
        Err(Refusal::NotHeld {
            to: 2,
            held: vec![]
        })
    );
    // Redo can restore the three undone: 5's (at 5 and 9), 6's (6 and 8), and 7's.
    assert_eq!(
        stack.redo(Some(2), revision),
        Err(Refusal::NotHeld {
            to: 2,
            held: vec![5, 6, 7, 8, 9]
        })
    );
}

// ---------------------------------------------------------------------------------------
// Hostile input (break-it): the shapes a document's text, the documents passed with it,
// the bound, and the order of undo, redo, and edits can take. Every expected value is
// counted by hand from the text's lines, or by a model of the stack that counts what each
// state holds from scratch at every step.

use proptest::prelude::*;

/// The canonical text before a document's features, written by hand: the same for every
/// document.
const HEAD: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": ";

/// A box as the canonical form places it, by its ID's number and its length.
fn line(id: u32, length: u32) -> String {
    format!(
        r#"    {{"id": "b{id:07}", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": {length}, "width": 10, "height": 10}}"#
    )
}

/// The canonical text of a document of these boxes, written by hand.
fn text_of(boxes: &[(u32, u32)]) -> String {
    if boxes.is_empty() {
        return format!("{HEAD}[]\n}}\n");
    }
    let lines: Vec<String> = boxes.iter().map(|&(id, l)| line(id, l)).collect();
    format!("{HEAD}[\n{}\n  ]\n}}\n", lines.join(",\n"))
}

/// The document of these boxes, and its text, which is checked to be its canonical form.
fn of(boxes: &[(u32, u32)]) -> (String, Document) {
    let text = text_of(boxes);
    let document = read(text.as_bytes()).unwrap();
    assert_eq!(
        write(&document).unwrap(),
        text,
        "the fixture isn't canonical"
    );
    (text, document)
}

/// `n` boxes, IDs 0 to n − 1, each length 10.
fn tens(n: u32) -> Vec<(u32, u32)> {
    (0..n).map(|id| (id, 10)).collect()
}

/// What a chunk of these boxes counts for: its lines and [`CHUNK_BYTES`].
fn chunk_of(boxes: &[(u32, u32)]) -> usize {
    boxes
        .iter()
        .map(|&(id, l)| line(id, l).len())
        .sum::<usize>()
        + CHUNK_BYTES
}

/// What a state's list counts for, of `chunks` chunks.
fn list_of(chunks: usize) -> usize {
    LIST_BYTES + ENTRY_BYTES * chunks
}

#[test]
fn a_document_of_no_features_is_held_as_a_list_of_no_chunks_and_restored() {
    let (begin, _) = of(&tens(2));
    let f = begin.len();
    let (none, d0) = of(&[]);
    let (one, d1) = of(&tens(1));
    let mut stack = begun(begin.as_bytes(), 1u64, UNDO_BYTES);
    stack.edit_text(none.as_bytes(), Some(&d0), 2);
    stack.edit_text(one.as_bytes(), Some(&d1), 3);
    // The empty state is held now: a list of no chunks.
    assert_eq!(stack.bytes(), f + list_of(0));
    stack.edit_text(none.as_bytes(), Some(&d0), 4);
    assert_eq!(
        stack.bytes(),
        f + list_of(0) + list_of(1) + chunk_of(&tens(1))
    );
    for (want, text) in [(3, &one), (2, &none), (1, &begin)] {
        let back = stack.undo(None, 10 + want).unwrap();
        assert_eq!(back.restored, want);
        assert_eq!(String::from_utf8(back.text.to_vec()).unwrap(), **text);
    }
    let redone = stack.redo(Some(4), 20).unwrap();
    assert_eq!(redone.text.as_slice(), none.as_bytes());
    assert_eq!((redone.undo, redone.redo), (3, 0));
}

#[test]
fn chunks_at_64_65_128_and_129_features_are_shared_where_they_hold_the_same_texts() {
    // Begin's 63 boxes, then 64, 65, 128, and 129, each the one before with boxes added,
    // then the last box's length changed: the first chunk is shared by all, the second
    // is a box, then 64, the third a box, then another.
    let sizes = [63u32, 64, 65, 128, 129];
    let (begin, _) = of(&tens(sizes[0]));
    let f = begin.len();
    let mut stack = begun(begin.as_bytes(), 0u64, UNDO_BYTES);
    let mut texts = vec![begin.clone()];
    for (i, &n) in sizes.iter().enumerate().skip(1) {
        let (text, document) = of(&tens(n));
        stack.edit_text(text.as_bytes(), Some(&document), i as u64);
        texts.push(text);
    }
    let mut last = tens(129);
    last[128].1 = 1000;
    let (changed, d5) = of(&last);
    stack.edit_text(changed.as_bytes(), Some(&d5), 5);
    texts.push(changed);
    let a = chunk_of(&tens(64));
    let b = chunk_of(&[(64, 10)]);
    let c = chunk_of(&tens(128)[64..]);
    let d = chunk_of(&[(128, 10)]);
    // 64 (one chunk, A), 65 (A, B), 128 (A, C), 129 (A, C, D) held; 129 changed is the
    // session's.
    let held = f + list_of(1) + a + list_of(2) + b + list_of(2) + c + list_of(3) + d;
    assert_eq!(stack.bytes(), held);
    // Undone to begin's state, every later one held, the last's own chunk E with them.
    let e = chunk_of(&[(128, 1000)]);
    for back in (0..5).rev() {
        let restored = stack.undo(None, 10 + back).unwrap();
        assert_eq!(
            restored.text.as_slice(),
            texts[back as usize].as_bytes(),
            "{back}"
        );
    }
    assert_eq!(stack.bytes(), held + list_of(3) + e);
    for forward in 1..6u64 {
        let restored = stack.redo(None, 20 + forward).unwrap();
        assert_eq!(
            restored.text.as_slice(),
            texts[forward as usize].as_bytes(),
            "{forward}"
        );
    }
    assert_eq!(stack.bytes(), held);
}

#[test]
fn a_box_inserted_at_the_top_shifts_every_chunk_and_shares_none() {
    let (begin, _) = of(&tens(130));
    let f = begin.len();
    let mut stack = begun(begin.as_bytes(), 0u64, UNDO_BYTES);
    let mut first = tens(130);
    first[0].1 = 11;
    let (one, d1) = of(&first);
    stack.edit_text(one.as_bytes(), Some(&d1), 1);
    let mut second = first.clone();
    second.insert(0, (999, 10));
    let (two, d2) = of(&second);
    stack.edit_text(two.as_bytes(), Some(&d2), 2);
    let mut third = second.clone();
    third[0].1 = 12;
    let (three, d3) = of(&third);
    stack.edit_text(three.as_bytes(), Some(&d3), 3);
    let whole = |boxes: &[(u32, u32)]| {
        list_of(3) + chunk_of(&boxes[..64]) + chunk_of(&boxes[64..128]) + chunk_of(&boxes[128..])
    };
    assert_eq!(stack.bytes(), f + whole(&first) + whole(&second));
    assert_eq!(
        stack.undo(Some(1), 4).unwrap().text.as_slice(),
        one.as_bytes()
    );
}

#[test]
fn a_whole_text_whose_document_isnt_the_one_passed_is_held_and_restored_as_given() {
    let (begin, d0) = of(&tens(3));
    let f = begin.len();
    let mut lengths = tens(3);
    lengths[1].1 = 77;
    let (one, d1) = of(&lengths);
    let mut stack = begun(begin.as_bytes(), 0u64, UNDO_BYTES);
    // `one`'s text with begin's document: a caller's mistake; the text is what's held.
    stack.edit_text(one.as_bytes(), Some(&d0), 1);
    stack.edit_text(begin.as_bytes(), Some(&d1), 2);
    assert_eq!(stack.bytes(), f + one.len() + WHOLE_BYTES);
    stack.edit_text(one.as_bytes(), None, 3);
    assert_eq!(
        stack.bytes(),
        f + one.len() + WHOLE_BYTES + begin.len() + WHOLE_BYTES
    );
    assert_eq!(
        stack.undo(None, 4).unwrap().text.as_slice(),
        begin.as_bytes()
    );
    assert_eq!(stack.undo(None, 5).unwrap().text.as_slice(), one.as_bytes());
}

#[test]
fn whole_bytes_no_document_reads_as_are_held_and_restored_as_given() {
    let (canonical, document) = of(&tens(2));
    let bom = format!("\u{feff}{canonical}");
    let crlf = canonical.replace('\n', "\r\n");
    let mut invalid = canonical.clone().into_bytes();
    invalid[10] = 0xff;
    let nul = format!("{canonical}\0");
    let texts: Vec<Vec<u8>> = vec![
        Vec::new(),
        bom.into_bytes(),
        crlf.into_bytes(),
        invalid,
        nul.into_bytes(),
        canonical.as_bytes()[..canonical.len() - 1].to_vec(),
        canonical.as_bytes().to_vec(),
    ];
    // Begin's bytes empty, as a file of nothing is.
    let mut stack = begun(b"", 0u64, UNDO_BYTES);
    for (i, text) in texts.iter().enumerate() {
        stack.edit_text(text, Some(&document), 1 + i as u64);
    }
    let whole: usize = texts[..texts.len() - 1]
        .iter()
        .map(|t| t.len() + WHOLE_BYTES)
        .sum();
    assert_eq!(stack.bytes(), whole);
    for back in (0..texts.len() - 1).rev() {
        let restored = stack.undo(None, 100 + back as u64).unwrap();
        assert_eq!(restored.text.as_slice(), texts[back], "{back}");
    }
    assert_eq!(stack.undo(None, 200).unwrap().text.as_slice(), b"");
}

#[test]
fn a_sketch_s_lines_over_several_lines_are_held_in_chunks() {
    use super::fixture::{LINE, POINTS, document, fields, point};
    let raw = document(&fields(POINTS, LINE, "", "", ""));
    let d1 = read(raw.as_bytes()).unwrap();
    let one = write(&d1).unwrap();
    let moved = raw.replacen(
        &point("pb000000", 10.0, 0.0),
        &point("pb000000", 12.0, 0.0),
        1,
    );
    assert_ne!(moved, raw);
    let d2 = read(moved.as_bytes()).unwrap();
    let two = write(&d2).unwrap();
    assert!(
        one.lines().count() > 8,
        "the sketch isn't over several lines"
    );
    let (begin, _) = of(&tens(1));
    let f = begin.len();
    let mut stack = begun(begin.as_bytes(), 0u64, UNDO_BYTES);
    stack.edit_text(one.as_bytes(), Some(&d1), 1);
    stack.edit_text(two.as_bytes(), Some(&d2), 2);
    // One feature, in one chunk: its text is the document's but its head, the "[\n"
    // before it, and the "\n  ]\n}\n" after.
    assert!(one.starts_with(HEAD));
    let feature = one.len() - HEAD.len() - 2 - 7;
    assert_eq!(stack.bytes(), f + list_of(1) + feature + CHUNK_BYTES);
    assert_eq!(stack.undo(None, 3).unwrap().text.as_slice(), one.as_bytes());
    assert_eq!(stack.redo(None, 4).unwrap().text.as_slice(), two.as_bytes());
}

#[test]
fn a_bound_one_state_passes_still_keeps_the_state_before_the_edit() {
    // `edit` says the bound never drops "the one before this edit". Begin's state, then
    // two edits, under a bound of nothing: the first edit's state is the one before the
    // second.
    let (begin, _) = of(&tens(2));
    let mut lengths = tens(2);
    let mut stack = begun(begin.as_bytes(), 0u64, 0);
    let mut texts = vec![begin.clone()];
    for k in 1..=3u32 {
        lengths[0].1 = 10 + k;
        let (text, document) = of(&lengths);
        stack.edit_text(text.as_bytes(), Some(&document), u64::from(k));
        texts.push(text);
    }
    // Was (a break-it finding): past the bound, `edit` dropped the state at index 1 while `at >= 2`, which at
    // `at == 2` is the state before the edit just made: the edit can't be undone at all
    // (undo_left 0, NothingToUndo), contrary to `edit`'s doc ("never the one before this
    // edit") and the loop's own comment. Unreachable at UNDO_BYTES (two states of the
    // largest file fit), so low.
    assert_eq!(stack.undo_left(), 1);
    let back = stack.undo(None, 10).unwrap();
    assert_eq!(back.text.as_slice(), texts[2].as_bytes());
}

#[test]
fn a_bound_of_nothing_never_panics_and_keeps_begin_s_and_the_state_before_each_edit() {
    let (begin, _) = of(&tens(2));
    let f = begin.len();
    let mut lengths = tens(2);
    let mut stack = begun(begin.as_bytes(), 0u64, 0);
    for k in 1..=50u32 {
        lengths[1].1 = 10 + k;
        let (text, document) = of(&lengths);
        stack.edit_text(text.as_bytes(), Some(&document), u64::from(k));
        // Begin's bytes are counted and never dropped, nor the state before the edit: the
        // first edit holds begin's alone, the second the first's too (nothing droppable
        // yet), and from the third the bound drops all but the one before the edit: two
        // edits out of reach at the third, one more at each after.
        let mut before = tens(2);
        before[1].1 = 10 + k - 1;
        let (bytes, undo, dropped) = match k {
            1 => (f, 1, 0),
            2 => (f + list_of(1) + chunk_of(&before), 2, 0),
            _ => (f + list_of(1) + chunk_of(&before), 1, k as usize - 1),
        };
        assert_eq!(
            (stack.bytes(), stack.undo_left(), stack.dropped()),
            (bytes, undo, dropped),
            "{k}"
        );
        assert_eq!(stack.redo_left(), 0);
    }
    assert_eq!(stack.dropped(), 49);
    assert_eq!(stack.redo(None, 99), Err(Refusal::NothingToRedo));
}

#[test]
fn a_to_with_nothing_to_undo_or_redo_is_still_named() {
    // Section 7: "A `to` on the wrong side, or the session's own revision, is refused,
    // naming `to`." A fresh stack, begun at revision 12.
    let (begin, _) = of(&tens(1));
    let mut stack = begun(begin.as_bytes(), 12u64, UNDO_BYTES);
    // Was (a break-it finding): `undo` and `redo` checked for an empty side before `to`, so an undo or redo
    // to the session's own revision (a dialog Cancel that made no edit, or an agent's
    // stale `to`) is NothingToUndo / NothingToRedo, not naming `to`; the same holds for a
    // `to` the bound dropped once undo has reached the oldest kept, which is told
    // "nothing to undo" and not that its revision is gone.
    assert_eq!(stack.undo(Some(12), 13), Err(Refusal::WrongSide { to: 12 }));
    assert_eq!(stack.redo(Some(12), 13), Err(Refusal::WrongSide { to: 12 }));
}

#[test]
fn ten_thousand_undos_and_redos_leave_the_count_and_the_texts_as_they_were() {
    let states = run();
    let mut stack = pushed(&states, UNDO_BYTES);
    stack.undo(Some(10), 15).unwrap();
    let low = stack.bytes();
    stack.redo(Some(14), 16).unwrap();
    let high = stack.bytes();
    let mut revision = 16;
    for i in 0..5_000u64 {
        revision += 1;
        let back = stack.undo(Some(10 + i % 4), revision).unwrap();
        assert_eq!(back.text.as_slice(), states[(i % 4) as usize].0.as_bytes());
        revision += 1;
        let forward = stack.redo(Some(14), revision).unwrap();
        assert_eq!(forward.text.as_slice(), states[4].0.as_bytes());
        assert_eq!(stack.bytes(), high, "{i}");
    }
    stack.undo(Some(10), revision + 1).unwrap();
    assert_eq!(stack.bytes(), low);
}

/// A document of one box whose note makes its canonical text exactly `len` bytes, and the
/// document it is.
fn padded_to(len: usize) -> (String, Document) {
    let (small, mut document) = of(&tens(1));
    let prefix = line(0, 10);
    let short = small.replacen(&prefix, &prefix.replacen('}', r#", "note": ""}"#, 1), 1);
    let note = "n".repeat(len - short.len());
    let text = short.replacen(r#""note": """#, &format!(r#""note": "{note}""#), 1);
    assert_eq!(text.len(), len);
    document.features[0].note = Some(note);
    (text, document)
}

#[test]
fn a_document_past_the_limit_is_held_whole_and_one_at_it_in_chunks() {
    use silico_core::document::json::MAX_DOCUMENT_LEN;
    // Was (a break-it finding): `write::parts` counted each feature's "[\n" or ",\n" and
    // not the "\n  ]\n}\n" after the last, so a document 1 to 7 bytes past the limit was
    // split anyway, and held in chunks rather than whole. The text it gave back was the
    // same, so it cost only the bound's count. One byte past, and seven, are held whole;
    // one at the limit in chunks: its list and its one chunk, the box's text and
    // `CHUNK_BYTES` (the text less the head, "[\n", and the close).
    let (begin, _) = of(&tens(2));
    let f = begin.len();
    let (small, _) = of(&tens(1));
    for (len, whole) in [
        (MAX_DOCUMENT_LEN + 7, true),
        (MAX_DOCUMENT_LEN + 1, true),
        (MAX_DOCUMENT_LEN, false),
    ] {
        let (text, document) = padded_to(len);
        // The reader refuses one past the limit, as the writer does, and takes one at it.
        assert_eq!(read(text.as_bytes()).is_ok(), !whole, "{len}");
        assert_eq!(write(&document).is_ok(), !whole, "{len}");
        let mut stack = begun(begin.as_bytes(), 0u64, UNDO_BYTES);
        stack.edit_text(text.as_bytes(), Some(&document), 1);
        stack.edit_text(small.as_bytes(), None, 2);
        let held = if whole {
            text.len() + WHOLE_BYTES
        } else {
            list_of(1) + (text.len() - HEAD.len() - 2 - 7) + CHUNK_BYTES
        };
        assert_eq!(stack.bytes(), f + held, "{len}");
        assert_eq!(
            stack.undo(None, 3).unwrap().text.as_slice(),
            text.as_bytes(),
            "{len}"
        );
    }
}

#[test]
fn an_undo_that_holds_more_past_the_bound_drops_the_oldest_and_says_so() {
    // Section 6: the bound holds through `undo` and `redo` too. Four edits of box 0's
    // length (each its list and its first chunk), then a box put at the top, which shifts
    // every chunk, so the state undone from shares none: held for redo, it passes the
    // bound, and the oldest after begin's go until it's under.
    let with = |length: u32| {
        let mut boxes = tens(130);
        boxes[0].1 = length;
        boxes
    };
    let full = |boxes: &[(u32, u32)]| {
        list_of(boxes.chunks(64).count()) + boxes.chunks(64).map(chunk_of).sum::<usize>()
    };
    let (begin, _) = of(&tens(130));
    let f = begin.len();
    let first = full(&with(101));
    let step = list_of(3) + chunk_of(&with(101)[..64]);
    let mut stack = begun(begin.as_bytes(), 0, f + first + 3 * step);
    for k in 1..=4u32 {
        let (text, document) = of(&with(100 + k));
        assert_eq!(
            stack.edit_text(text.as_bytes(), Some(&document), u64::from(k)),
            Pushed { dropped: 0 }
        );
    }
    let mut top = with(104);
    top.insert(0, (999, 10));
    let (shifted, document) = of(&top);
    assert_eq!(
        stack.edit_text(shifted.as_bytes(), Some(&document), 5),
        Pushed { dropped: 0 }
    );
    assert_eq!(stack.bytes(), f + first + 3 * step);
    // Undone: the shifted state held (all its chunks), the fourth released (its list and
    // first chunk): over by the shifted state less a step. The first state after begin's
    // goes (the second's first chunk and list stay, its others the third's now), then the
    // second: the third alone, in full, and the shifted state.
    let restored = stack.undo(None, 6).unwrap();
    assert_eq!(restored.text.as_slice(), of(&with(104)).0.as_bytes());
    assert_eq!((restored.dropped, restored.undo, restored.redo), (3, 1, 1));
    assert_eq!(stack.dropped(), 3);
    assert_eq!(stack.bytes(), f + first + full(&top));
    // Redone, the shifted state is the session's again, the third and fourth held.
    let redone = stack.redo(None, 7).unwrap();
    assert_eq!(redone.text.as_slice(), shifted.as_bytes());
    assert_eq!((redone.dropped, redone.undo, redone.redo), (0, 2, 0));
    assert_eq!(stack.bytes(), f + first + step);
}

#[test]
fn not_held_names_every_revision_the_command_can_restore_gaps_and_all() {
    // From the architecture review: begin at 12, edits at 13 and 14, an undo at 15 (13's
    // state), and an edit at 16, which drops 14's: 14 lies between revisions undo can
    // restore, and the refusal says which those are (those before the session's state,
    // 16's), not a range that seems to hold it.
    let states = run();
    let mut stack = begun(states[0].0.as_bytes(), 12, UNDO_BYTES);
    stack.edit_text(states[1].0.as_bytes(), states[1].1.as_ref(), 13);
    stack.edit_text(states[2].0.as_bytes(), states[2].1.as_ref(), 14);
    stack.undo(None, 15).unwrap();
    stack.edit_text(states[3].0.as_bytes(), states[3].1.as_ref(), 16);
    assert_eq!(
        stack.undo(Some(14), 17),
        Err(Refusal::NotHeld {
            to: 14,
            held: vec![12, 13, 15]
        })
    );
}

#[test]
fn a_prepared_document_s_text_is_its_canonical_text() {
    // What the command layer takes as an edit's text, from the features it hands the
    // stack: `write`'s, byte for byte, for features on one line and none at all.
    let example = read(EXAMPLE.as_bytes()).unwrap();
    let (none, empty) = of(&[]);
    let (many, boxes) = of(&tens(130));
    for (document, text) in [
        (&example, EXAMPLE),
        (&empty, none.as_str()),
        (&boxes, many.as_str()),
    ] {
        let prepared = Prepared::of(document).unwrap();
        assert_eq!(prepared.text(), write(document).unwrap());
        assert_eq!(prepared.text(), text);
    }
    // A sketch, over several lines; and a name and a note not in ASCII, the note with a
    // line separator, which canonical form escapes.
    use super::fixture::{LINE, POINTS, document, fields};
    let sketch = read(document(&fields(POINTS, LINE, "", "", "")).as_bytes()).unwrap();
    let (_, mut named) = of(&tens(3));
    named.features[1].name = Name::new("Bügel — 角").unwrap();
    named.features[1].note = Some("Ø 6 mm ✓\u{2028}then".to_owned());
    for document in [&sketch, &named] {
        let written = write(document).unwrap();
        assert!(written.lines().count() > 3);
        assert_eq!(Prepared::of(document).unwrap().text(), written);
    }
    assert!(write(&named).unwrap().contains("Bügel — 角"));
    assert!(write(&named).unwrap().contains("\\u2028"));
}

#[test]
fn a_prepared_document_at_exactly_the_limit_is_its_canonical_text_and_one_byte_more_is_refused() {
    // One box with a note of `n` letters: its canonical text is the text with an empty note
    // and `n` bytes more, so `n` is chosen to make it exactly `MAX_DOCUMENT_LEN`.
    let (_, mut noted) = of(&tens(1));
    noted.features[0].note = Some(String::new());
    let empty = write(&noted).unwrap().len();
    noted.features[0].note = Some("x".repeat(MAX_DOCUMENT_LEN - empty));
    let written = write(&noted).unwrap();
    assert_eq!(written.len(), MAX_DOCUMENT_LEN);
    assert_eq!(Prepared::of(&noted).unwrap().text(), written);
    noted.features[0].note = Some("x".repeat(MAX_DOCUMENT_LEN - empty + 1));
    let refused = Prepared::of(&noted).unwrap_err();
    assert_eq!(
        refused.message(),
        write(&noted).unwrap_err().message(),
        "refused otherwise than write refuses it"
    );
}

#[test]
fn a_canonical_state_is_restored_as_its_features_write_it() {
    // The stack keeps an edit's state as it's given: a `Canonical` one is restored as its
    // features write it, whatever bytes the session held, so the caller must give it only
    // for a text that is that document's canonical form (and a text that isn't, whole).
    let states = run();
    let (spaced, document) = (&states[4].0, states[4].1.as_ref().unwrap());
    let prepared = Prepared::of(document).unwrap();
    assert_ne!(prepared.text(), *spaced);
    let mut stack = begun(states[0].0.as_bytes(), 1, UNDO_BYTES);
    stack.edit(After::Canonical(prepared.clone()), 2);
    stack.edit(After::Whole(Arc::new(spaced.clone().into_bytes())), 3);
    assert_eq!(
        stack.undo(None, 4).unwrap().text.as_slice(),
        prepared.text().as_bytes()
    );
    assert_eq!(
        stack.redo(None, 5).unwrap().text.as_slice(),
        spaced.as_bytes()
    );
}

/// 130 boxes of length 10 but box 0's, `length`.
fn longer(length: u32) -> Vec<(u32, u32)> {
    let mut boxes = tens(130);
    boxes[0].1 = length;
    boxes
}

/// What a state of `boxes` holds when it shares no chunk: its list and every chunk.
fn full(boxes: &[(u32, u32)]) -> usize {
    list_of(boxes.chunks(64).count()) + boxes.chunks(64).map(chunk_of).sum::<usize>()
}

/// A stack of 130 boxes, and what its states count for: begin's bytes, the first state's,
/// a step's (an edit of box 0's length), and the shifted state's.
struct Shifted {
    stack: UndoStack<u64>,
    f: usize,
    first: usize,
    step: usize,
    top: usize,
}

/// A stack of 130 boxes bounded at begin's bytes, the first state's, and three steps: four
/// edits of box 0's length (101 to 104, revisions 1 to 4), then a box put at the top
/// (revision 5), which shifts every chunk: at the bound exactly.
fn shifted() -> Shifted {
    let (begin, _) = of(&tens(130));
    let f = begin.len();
    let first = full(&longer(101));
    let step = list_of(3) + chunk_of(&longer(101)[..64]);
    let mut stack = begun(begin.as_bytes(), 0, f + first + 3 * step);
    for k in 1..=4u32 {
        let (text, document) = of(&longer(100 + k));
        stack.edit_text(text.as_bytes(), Some(&document), u64::from(k));
    }
    let mut top = longer(104);
    top.insert(0, (999, 10));
    let (text, document) = of(&top);
    stack.edit_text(text.as_bytes(), Some(&document), 5);
    assert_eq!(stack.bytes(), f + first + 3 * step);
    Shifted {
        stack,
        f,
        first,
        step,
        top: full(&top),
    }
}

#[test]
fn an_undo_to_the_oldest_state_after_begin_s_drops_nothing_however_far_past_the_bound() {
    // Undone to revision 1's state, the first after begin's, while the shifted state is
    // held: over the bound, but the only state the bound could drop is the session's own,
    // which it never drops.
    let Shifted {
        mut stack,
        f,
        first,
        step,
        top,
    } = shifted();
    let restored = stack.undo(Some(1), 6).unwrap();
    assert_eq!(restored.text.as_slice(), of(&longer(101)).0.as_bytes());
    assert_eq!((restored.dropped, restored.undo, restored.redo), (0, 1, 4));
    // The second state now holds the middle and last chunks the first did, in full; the
    // third and fourth a step each; the shifted state all its own.
    assert_eq!(stack.bytes(), f + first + 2 * step + top);
    assert!(stack.bytes() > f + first + 3 * step);
}

#[test]
fn an_undo_to_the_second_state_after_begin_s_drops_the_first_alone() {
    // Undone to revision 2's state while the shifted state is held: over the bound, the
    // first after begin's goes (two edits out of undo's reach), and then the session's
    // state is the oldest after begin's, which the bound never drops, over it still.
    let Shifted {
        mut stack,
        f,
        first,
        step,
        top,
    } = shifted();
    let restored = stack.undo(Some(2), 6).unwrap();
    assert_eq!(restored.text.as_slice(), of(&longer(102)).0.as_bytes());
    assert_eq!((restored.dropped, restored.undo, restored.redo), (2, 0, 3));
    assert_eq!(stack.dropped(), 2);
    // The third state now holds the middle and last chunks in full; the fourth a step; the
    // shifted state all its own.
    assert_eq!(stack.bytes(), f + first + step + top);
}

// ---------------------------------------------------------------------------------------
// A model of the stack: what each state holds counted from scratch, against the stack's
// running count, over random runs of edits, undos, and redos. What it checks on its own is
// the count (each chunk by its texts, from scratch at every step, not the stack's running
// total or its pointers) and the texts (written by hand, held whole); its rules for undo,
// redo, `to`, and drops are the stack's, copied, so those rest on the tests by hand above.

#[derive(Debug, Clone)]
enum Step {
    /// A box's length set (the index taken modulo the boxes there are).
    Set(usize, u32),
    Insert(usize),
    Remove(usize),
    /// The session's document again, its text spaced out: held whole.
    Spaced,
    /// The canonical text passed with begin's document.
    Mismatched,
    /// The canonical text passed with no document.
    Untyped,
    /// An undo or redo, to `None` or a revision picked from those so far and a few after.
    Undo(Option<u16>),
    Redo(Option<u16>),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        4 => (any::<usize>(), prop_oneof![Just(10u32), Just(105), Just(7), 1u32..100_000]).prop_map(|(i, l)| Step::Set(i, l)),
        1 => any::<usize>().prop_map(Step::Insert),
        1 => any::<usize>().prop_map(Step::Remove),
        1 => Just(Step::Spaced),
        1 => Just(Step::Mismatched),
        1 => Just(Step::Untyped),
        3 => proptest::option::of(any::<u16>()).prop_map(Step::Undo),
        2 => proptest::option::of(any::<u16>()).prop_map(Step::Redo),
    ]
}

struct Held {
    text: String,
    boxes: Vec<(u32, u32)>,
    /// Each chunk's ID in the model's table, or `None` held whole.
    chunks: Option<Vec<usize>>,
    revisions: Vec<u64>,
}

struct Model {
    states: Vec<Held>,
    table: Vec<Vec<String>>,
    at: usize,
    dropped: usize,
    bound: usize,
}

impl Model {
    fn bytes(&self) -> usize {
        let mut total = self.states[0].text.len();
        let mut counted = std::collections::BTreeSet::new();
        for (i, state) in self.states.iter().enumerate() {
            if i == 0 || i == self.at {
                continue;
            }
            match &state.chunks {
                None => total += state.text.len() + WHOLE_BYTES,
                Some(ids) => {
                    total += list_of(ids.len());
                    for &id in ids {
                        if counted.insert(id) {
                            total +=
                                self.table[id].iter().map(String::len).sum::<usize>() + CHUNK_BYTES;
                        }
                    }
                }
            }
        }
        total
    }

    fn lowest(&self) -> usize {
        usize::from(self.dropped > 0)
    }

    fn edit(
        &mut self,
        text: String,
        boxes: Vec<(u32, u32)>,
        chunked: bool,
        revision: u64,
    ) -> usize {
        self.states.truncate(self.at + 1);
        let chunks = chunked.then(|| {
            let lines: Vec<String> = boxes.iter().map(|&(id, l)| line(id, l)).collect();
            let before = self.states[self.at].chunks.clone().unwrap_or_default();
            lines
                .chunks(64)
                .enumerate()
                .map(|(i, texts)| match before.get(i) {
                    Some(&id) if self.table[id] == texts => id,
                    _ => {
                        self.table.push(texts.to_vec());
                        self.table.len() - 1
                    }
                })
                .collect()
        });
        self.states.push(Held {
            text,
            boxes,
            chunks,
            revisions: vec![revision],
        });
        self.at += 1;
        // The oldest after `begin`'s, never the state before the edit.
        self.drop_past_bound(3)
    }

    fn drop_past_bound(&mut self, least: usize) -> usize {
        let mut dropped = 0;
        while self.bytes() > self.bound && self.at >= least {
            self.states.remove(1);
            self.at -= 1;
            let more = if self.dropped == 0 { 2 } else { 1 };
            self.dropped += more;
            dropped += more;
        }
        dropped
    }

    fn find(&self, to: u64) -> Option<usize> {
        self.states
            .iter()
            .position(|s| s.revisions.contains(&to))
            .filter(|&found| found >= self.lowest())
    }

    /// The refusal of `to`, naming the revisions of the states at `side`, those the
    /// command can restore.
    fn not_held(&self, to: u64, side: std::ops::Range<usize>) -> Refusal<u64> {
        let held: std::collections::BTreeSet<u64> = self.states[side]
            .iter()
            .flat_map(|s| s.revisions.iter().copied())
            .collect();
        Refusal::NotHeld {
            to,
            held: held.into_iter().collect(),
        }
    }

    fn restore(&mut self, target: usize, revision: u64) -> (u64, String, usize, usize, usize) {
        self.at = target;
        // Past the bound, the oldest after `begin`'s, never the session's.
        let dropped = self.drop_past_bound(2);
        let at = self.at;
        self.states[at].revisions.push(revision);
        let undo = self.at - self.lowest();
        let redo = self.states.len() - self.at - 1;
        let state = &self.states[at];
        (state.revisions[0], state.text.clone(), undo, redo, dropped)
    }

    fn undo(
        &mut self,
        to: Option<u64>,
        revision: u64,
    ) -> Result<(u64, String, usize, usize, usize), Refusal<u64>> {
        let target = match to {
            None if self.at <= self.lowest() => return Err(Refusal::NothingToUndo),
            None => self.at - 1,
            Some(to) => match self.find(to) {
                Some(found) if found < self.at => found,
                Some(_) => return Err(Refusal::WrongSide { to }),
                None => return Err(self.not_held(to, self.lowest()..self.at)),
            },
        };
        Ok(self.restore(target, revision))
    }

    fn redo(
        &mut self,
        to: Option<u64>,
        revision: u64,
    ) -> Result<(u64, String, usize, usize, usize), Refusal<u64>> {
        let target = match to {
            None if self.at + 1 >= self.states.len() => return Err(Refusal::NothingToRedo),
            None => self.at + 1,
            Some(to) => match self.find(to) {
                Some(found) if found > self.at => found,
                Some(_) => return Err(Refusal::WrongSide { to }),
                None => return Err(self.not_held(to, self.at + 1..self.states.len())),
            },
        };
        Ok(self.restore(target, revision))
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    #[test]
    fn the_stack_s_count_and_texts_match_a_model_counted_from_scratch(
        n in prop_oneof![Just(0u32), Just(1), Just(63), Just(64), Just(65), Just(128), 0u32..140],
        bound_steps in 0usize..8,
        steps in proptest::collection::vec(step(), 1..24),
    ) {
        let begin_boxes = tens(n);
        let begin = text_of(&begin_boxes);
        let begin_document = read(begin.as_bytes()).unwrap();
        let f = begin.len();
        let bound = if bound_steps == 7 { UNDO_BYTES } else { f + bound_steps * (f / 2 + 64) };
        let mut stack = begun(begin.as_bytes(), 0u64, bound);
        let mut model = Model {
            states: vec![Held { text: begin.clone(), boxes: begin_boxes.clone(), chunks: None, revisions: vec![0] }],
            table: Vec::new(),
            at: 0,
            dropped: 0,
            bound,
        };
        let mut next_id = n;
        let mut revision = 0u64;
        for (i, step) in steps.into_iter().enumerate() {
            revision += 1;
            let mut boxes = model.states[model.at].boxes.clone();
            let pick = |sel: u16| u64::from(sel) % (revision + 3);
            let edit: Option<(String, Option<Document>, bool)> = match step {
                Step::Set(k, l) => {
                    if boxes.is_empty() { None } else {
                        let k = k % boxes.len();
                        boxes[k].1 = l;
                        let text = text_of(&boxes);
                        let document = read(text.as_bytes()).unwrap();
                        Some((text, Some(document), true))
                    }
                }
                Step::Insert(k) => {
                    let k = k % (boxes.len() + 1);
                    boxes.insert(k, (next_id, 10));
                    next_id += 1;
                    let text = text_of(&boxes);
                    let document = read(text.as_bytes()).unwrap();
                    Some((text, Some(document), true))
                }
                Step::Remove(k) => {
                    if boxes.is_empty() { None } else {
                        boxes.remove(k % boxes.len());
                        let text = text_of(&boxes);
                        let document = read(text.as_bytes()).unwrap();
                        Some((text, Some(document), true))
                    }
                }
                Step::Spaced => {
                    let canonical = text_of(&boxes);
                    let text = canonical.replacen("\"v\": 1", "\"v\":  1", 1);
                    let document = read(canonical.as_bytes()).unwrap();
                    let chunked = text == canonical;
                    Some((text, Some(document), chunked))
                }
                Step::Mismatched => {
                    let text = text_of(&boxes);
                    let chunked = text == begin;
                    Some((text, Some(begin_document.clone()), chunked))
                }
                Step::Untyped => Some((text_of(&boxes), None, false)),
                Step::Undo(to) => {
                    let to = to.map(pick);
                    let got = stack.undo(to, revision).map(|r| (r.restored, String::from_utf8(r.text.to_vec()).unwrap(), r.undo, r.redo, r.dropped));
                    prop_assert_eq!(got, model.undo(to, revision), "undo {:?} at {}", to, i);
                    None
                }
                Step::Redo(to) => {
                    let to = to.map(pick);
                    let got = stack.redo(to, revision).map(|r| (r.restored, String::from_utf8(r.text.to_vec()).unwrap(), r.undo, r.redo, r.dropped));
                    prop_assert_eq!(got, model.redo(to, revision), "redo {:?} at {}", to, i);
                    None
                }
            };
            if let Some((text, document, chunked)) = edit {
                let pushed = stack.edit_text(text.as_bytes(), document.as_ref(), revision);
                let dropped = model.edit(text, boxes, chunked, revision);
                prop_assert_eq!(pushed, Pushed { dropped }, "edit at {}", i);
            }
            prop_assert_eq!(stack.bytes(), model.bytes(), "bytes at {}", i);
            prop_assert_eq!(stack.dropped(), model.dropped);
            prop_assert_eq!(stack.undo_left(), model.at - model.lowest());
            prop_assert_eq!(stack.redo_left(), model.states.len() - model.at - 1);
        }
    }
}
