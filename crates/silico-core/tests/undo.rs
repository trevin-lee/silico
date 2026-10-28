//! A transaction's undo stack (`document::undo`: the M5.0 design's section 3, A2, section
//! 6's bound, and section 7's `undo` and `redo`; pending Trevin, decision 2): a run of
//! edits undone one by one gives back each earlier state's text, byte for byte, and its
//! fingerprints on the fake kernel; redo gives back the later ones; an edit after an undo
//! drops them; `to` reaches a revision the stack holds and refuses any other; and the
//! bound counts what the states hold, worked out by hand, dropping the oldest past it.

use super::example::EXAMPLE;
use super::model::{checked, id, published};

use silico_core::document::undo::{
    CHUNK_BYTES, ENTRY_BYTES, LIST_BYTES, Pushed, Refusal, UNDO_BYTES, UndoStack, WHOLE_BYTES,
};
use silico_core::document::{Document, read, write};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::Recompute;

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
fn pushed(states: &[(String, Option<Document>)], bound: usize) -> UndoStack<u64, usize> {
    let mut stack = UndoStack::new(states[0].0.as_bytes(), 10, 0, bound);
    for (i, (text, document)) in states.iter().enumerate().skip(1) {
        let pushed = stack.edit(text.as_bytes(), document.as_ref(), 10 + i as u64, i);
        assert_eq!(pushed, Pushed { dropped: 0 });
    }
    stack
}

#[test]
fn a_run_of_edits_undone_one_by_one_gives_back_each_earlier_state_and_its_fingerprints() {
    let states = run();
    let mut stack = pushed(&states, UNDO_BYTES);
    assert_eq!((stack.undo_left(), stack.redo_left()), (4, 0));
    let mut revision = 14;
    for back in (0..4).rev() {
        revision += 1;
        let restored = stack.undo(None, revision).unwrap();
        let (text, _) = &states[back];
        assert_eq!(restored.restored, 10 + back as u64);
        assert_eq!(String::from_utf8(restored.text.clone()).unwrap(), *text);
        assert_eq!(restored.reached, back);
        assert_eq!((restored.undo, restored.redo), (back, 4 - back));
        assert_eq!(
            fingerprints(&String::from_utf8(restored.text).unwrap()),
            fingerprints(text),
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
            String::from_utf8(restored.text).unwrap(),
            *text,
            "state {forward}"
        );
        assert_eq!(restored.reached, forward);
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
    stack.edit(other.as_bytes(), Some(&document), 16, 9);
    assert_eq!((stack.undo_left(), stack.redo_left()), (3, 0));
    assert_eq!(stack.redo(None, 17), Err(Refusal::NothingToRedo));
    // The undone revisions are no state of the stack's now.
    assert_eq!(
        stack.undo(Some(13), 17),
        Err(Refusal::NotHeld {
            to: 13,
            held: (10, 16)
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
    assert_eq!(redone.text, other.as_bytes());
    assert_eq!(stack.undo(Some(15), 19).unwrap().restored, 12);
}

#[test]
fn to_reaches_a_revision_the_stack_holds_and_refuses_any_other() {
    let states = run();
    let mut stack = pushed(&states, UNDO_BYTES);
    // An undo to a later revision, or to the session's own, is on the wrong side.
    assert_eq!(stack.undo(Some(14), 15), Err(Refusal::WrongSide { to: 14 }));
    assert_eq!(
        stack.undo(Some(9), 15),
        Err(Refusal::NotHeld {
            to: 9,
            held: (10, 14)
        })
    );
    let restored = stack.undo(Some(11), 15).unwrap();
    assert_eq!(
        (restored.restored, restored.undo, restored.redo),
        (11, 1, 3)
    );
    assert_eq!(restored.text, states[1].0.as_bytes());
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
    assert_eq!(restored.text, states[3].0.as_bytes());
    // Back to `begin`'s state: its bytes, as the file held them.
    let restored = stack.undo(Some(10), 17).unwrap();
    assert_eq!(restored.text, EXAMPLE.as_bytes());
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
    let mut stack = UndoStack::new(begin.as_bytes(), 1u64, (), UNDO_BYTES);
    assert_eq!(stack.bytes(), f);
    // Box 0 longer: the state before (begin's) was counted already.
    let (one, d1) = boxes(&[0]);
    stack.edit(one.as_bytes(), Some(&d1), 2, ());
    assert_eq!(stack.bytes(), f);
    // Box 100 longer: the state before, the first after begin's, is charged in full.
    let (two, d2) = boxes(&[0, 100]);
    stack.edit(two.as_bytes(), Some(&d2), 3, ());
    let first = list + chunk(64, 1) + chunk(64, 0) + chunk(2, 0);
    assert_eq!(stack.bytes(), f + first);
    // Box 129 longer: the state before shares its first and last chunks with the first's,
    // so it adds its list and its middle chunk.
    let (three, d3) = boxes(&[0, 100, 129]);
    stack.edit(three.as_bytes(), Some(&d3), 4, ());
    let second = list + chunk(64, 1);
    assert_eq!(stack.bytes(), f + first + second);
    // Undone: the session's state is the second's (no longer counted), and the third is
    // held for redo, its first chunk the first state's (counted already), its middle the
    // second's (not counted now), its last its own.
    stack.undo(None, 5).unwrap();
    assert_eq!(stack.bytes(), f + first + list + chunk(64, 1) + chunk(2, 1));
    // An edit drops the redo state, and holds the second again.
    let (other, d4) = boxes(&[0, 1, 100]);
    stack.edit(other.as_bytes(), Some(&d4), 6, ());
    assert_eq!(stack.bytes(), f + first + second);
    // A text held whole, not the canonical form: its bytes and a list's, once it's held.
    let spaced = other.replacen("\"v\": 1", "\"v\":  1", 1);
    let d5 = read(spaced.as_bytes()).unwrap();
    stack.edit(spaced.as_bytes(), Some(&d5), 7, ());
    let fourth = list + chunk(64, 2);
    assert_eq!(stack.bytes(), f + first + second + fourth);
    stack.edit(other.as_bytes(), Some(&d4), 8, ());
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
    let mut stack = UndoStack::new(begin.as_bytes(), 1u64, (), bound);
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
        let pushed = stack.edit(text.as_bytes(), Some(&document), 1 + k as u64, ());
        assert_eq!(pushed, Pushed { dropped: 0 }, "edit {k}");
    }
    assert_eq!(stack.bytes(), f + first + 2 * step);
    assert_eq!(stack.undo_left(), 4);
    // The fifth edit passes the bound: the first state after begin's goes, and undo
    // reaches neither it nor begin's, two edits out of its reach.
    let (text, document) = length(5);
    assert_eq!(
        stack.edit(text.as_bytes(), Some(&document), 6, ()),
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
        stack.edit(text.as_bytes(), Some(&document), 7, ()),
        Pushed { dropped: 1 }
    );
    assert_eq!(stack.dropped(), 3);
    assert_eq!(stack.bytes(), f + second_alone + 2 * step);
    assert_eq!(stack.undo_left(), 3);
    // A dropped revision is refused, as is begin's, naming what's held.
    assert_eq!(
        stack.undo(Some(2), 8),
        Err(Refusal::NotHeld {
            to: 2,
            held: (4, 7)
        })
    );
    assert_eq!(
        stack.undo(Some(1), 8),
        Err(Refusal::NotHeld {
            to: 1,
            held: (4, 7)
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
}
