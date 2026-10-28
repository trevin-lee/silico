//! A transaction's undo stack (the M5.0 design, section 3's A2, section 6's bound, and
//! section 7's `undo` and `redo`; pending Trevin, decision 2): every state a transaction's
//! edits left, so `undo` and `redo` can restore any of them, byte for byte, with what the
//! edits had reached then. The command layer holds the stack and words its refusals; this
//! is what it holds, so a change of the commands (decision 2 vetoed, or worded otherwise)
//! leaves it as it is.
//!
//! A state is held as its features' canonical texts, in chunks of [`CHUNK`] features, each
//! chunk shared with the state before while none of its features changes: a state costs
//! its list of chunks and the chunks its edit changed. A text that isn't a document's
//! canonical form (one `replace_text` wrote, say) is held whole, so an undo gives it back
//! as it was. The state `begin` saw is the file's bytes, held whole, and never dropped (it's
//! `rollback`'s). Past the bound, the oldest state after it is dropped and counted, and
//! `undo` then reaches neither it nor `begin`.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::Document;
use super::json::MAX_DOCUMENT_LEN;
use super::write;

/// How much a transaction's undo stack holds at most, as [`UndoStack::bytes`] counts it:
/// 256 MiB (section 6).
pub const UNDO_BYTES: usize = 256 * 1024 * 1024;

/// How many features a chunk holds: a state's list of chunks has one entry for each.
pub const CHUNK: usize = 64;

/// What a state's list of chunks counts for, beyond its chunks: the list itself, and each
/// of its entries (a constant for each, not a measured size).
pub const LIST_BYTES: usize = 24;
pub const ENTRY_BYTES: usize = 8;

/// What a chunk counts for beyond its features' texts.
pub const CHUNK_BYTES: usize = 16;

/// What a state held whole counts for beyond its text's bytes, as a list of no chunks does.
pub const WHOLE_BYTES: usize = LIST_BYTES;

/// Up to [`CHUNK`] features' canonical texts, as the document places them.
#[derive(Debug, PartialEq, Eq)]
struct Chunk {
    texts: Vec<Box<str>>,
}

impl Chunk {
    /// What it counts for: its texts' bytes and [`CHUNK_BYTES`].
    fn cost(&self) -> usize {
        self.texts.iter().map(|t| t.len()).sum::<usize>() + CHUNK_BYTES
    }
}

/// A state's text, as the stack holds it.
#[derive(Debug, Clone)]
enum Text {
    /// Its bytes, as they were: `begin`'s, and any text that isn't a document's canonical
    /// form.
    Whole(Arc<[u8]>),
    /// A document's canonical text, its features in chunks after the stack's head.
    Chunked(Vec<Arc<Chunk>>),
}

/// A state the stack holds: its text, what the edits had reached then (`R`, the command
/// layer's), and every revision it was the session's state at, the one it was made at first.
#[derive(Debug, Clone)]
struct State<K, R> {
    text: Text,
    reached: R,
    revisions: Vec<K>,
}

/// A state restored by [`UndoStack::undo`] or [`UndoStack::redo`]: the revision it was
/// made at, its text, what the edits had reached then, and what's left to undo and redo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored<K, R> {
    pub restored: K,
    pub text: Vec<u8>,
    pub reached: R,
    pub undo: usize,
    pub redo: usize,
}

/// Why [`UndoStack::undo`] or [`UndoStack::redo`] restored nothing, for the command layer to
/// word (section 7's errors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal<K> {
    /// No edit to undo: none was made, or every one is undone, or the bound dropped those
    /// left.
    NothingToUndo,
    /// No edit to redo: none was undone, or an edit since dropped what was.
    NothingToRedo,
    /// `to` isn't a revision any state the stack holds was the session's state at: the
    /// bound dropped it, or it's from before the transaction, or an edit dropped it from
    /// redo. `held` is the oldest and newest revisions the stack's states hold.
    NotHeld { to: K, held: (K, K) },
    /// `to` is the session's state now, or a revision on the other side of it (an undo to
    /// a later state, a redo to an earlier one).
    WrongSide { to: K },
}

/// What an edit pushed off the stack: how many more of the transaction's edits undo can
/// no longer reach, for the edit's caller to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pushed {
    pub dropped: usize,
}

/// A transaction's states, from the one `begin` saw to the newest an edit made, the
/// session's current one among them, and what they hold (section 3's A2).
#[derive(Debug)]
pub struct UndoStack<K, R> {
    /// The canonical form's text before a document's features: the same for every
    /// document, so every chunked state shares it, and it's counted for none.
    head: Arc<str>,
    /// Oldest first: `begin`'s, then each edit's, the redo states after the current one.
    states: Vec<State<K, R>>,
    /// The session's state now, in `states`.
    at: usize,
    /// How many of the transaction's edits undo can no longer reach.
    dropped: usize,
    bound: usize,
    /// What the states other than the current one hold, as [`UndoStack::bytes`] says.
    bytes: usize,
    /// Each chunk those states hold, by where it is, with how many of them hold it and what
    /// it counts for: counted once, however many share it.
    held: BTreeMap<usize, (usize, usize)>,
}

impl<K: Copy + Ord, R: Clone> UndoStack<K, R> {
    /// The stack of a transaction `begin` opened at `revision`, on a file holding `bytes`,
    /// with what the session's edits had reached then; bounded by `bound` bytes as
    /// [`UndoStack::bytes`] counts them ([`UNDO_BYTES`] for a session's).
    pub fn new(bytes: &[u8], revision: K, reached: R, bound: usize) -> Self {
        let head = write::parts(&empty_document(), MAX_DOCUMENT_LEN)
            .map(|parts| parts.head)
            .unwrap_or_default();
        UndoStack {
            head: Arc::from(head),
            states: vec![State {
                text: Text::Whole(Arc::from(bytes)),
                reached,
                revisions: vec![revision],
            }],
            at: 0,
            dropped: 0,
            bound,
            bytes: bytes.len(),
            held: BTreeMap::new(),
        }
    }

    /// An edit's state, at `revision`: its text, the document it reads as if it does, and
    /// what the edits had reached then. The redo states are dropped, and past the bound,
    /// the oldest states after `begin`'s, but never the one before this edit.
    pub fn edit(
        &mut self,
        text: &[u8],
        document: Option<&Document>,
        revision: K,
        reached: R,
    ) -> Pushed {
        while self.states.len() > self.at + 1 {
            if let Some(state) = self.states.pop() {
                self.release(&state.text);
            }
        }
        let held = self.held_text(text, document);
        // The state before this edit is held from now: counted.
        if let Some(before) = self.states.get(self.at).map(|s| s.text.clone())
            && self.at > 0
        {
            self.hold(&before);
        }
        self.states.push(State {
            text: held,
            reached,
            revisions: vec![revision],
        });
        self.at += 1;
        let mut pushed = Pushed { dropped: 0 };
        // The oldest after `begin`, while it isn't the state before this edit.
        while self.bytes > self.bound && self.at >= 2 {
            let dropped = self.states.remove(1);
            self.release(&dropped.text);
            self.at -= 1;
            let more = if self.dropped == 0 { 2 } else { 1 };
            self.dropped += more;
            pushed.dropped += more;
        }
        pushed
    }

    /// The state before the session's, or with `to`, the state at that revision; the
    /// session's state now held for redo, at `revision`, a new one.
    pub fn undo(&mut self, to: Option<K>, revision: K) -> Result<Restored<K, R>, Refusal<K>> {
        let lowest = self.lowest();
        if self.at <= lowest {
            return Err(Refusal::NothingToUndo);
        }
        let target = match to {
            None => self.at - 1,
            Some(to) => match self.find(to) {
                Some(found) if found < self.at => found,
                Some(_) => return Err(Refusal::WrongSide { to }),
                None => return Err(self.not_held(to)),
            },
        };
        Ok(self.restore(target, revision))
    }

    /// The state after the session's, or with `to`, the state at that revision.
    pub fn redo(&mut self, to: Option<K>, revision: K) -> Result<Restored<K, R>, Refusal<K>> {
        if self.at + 1 >= self.states.len() {
            return Err(Refusal::NothingToRedo);
        }
        let target = match to {
            None => self.at + 1,
            Some(to) => match self.find(to) {
                Some(found) if found > self.at => found,
                Some(_) => return Err(Refusal::WrongSide { to }),
                None => return Err(self.not_held(to)),
            },
        };
        Ok(self.restore(target, revision))
    }

    /// How many edits [`UndoStack::undo`] can undo.
    pub fn undo_left(&self) -> usize {
        self.at - self.lowest().min(self.at)
    }

    /// How many edits [`UndoStack::redo`] can redo.
    pub fn redo_left(&self) -> usize {
        self.states.len() - self.at - 1
    }

    /// How many of the transaction's edits undo can no longer reach, the bound having
    /// dropped their states: 2 at the first drop (the edits before and after the state
    /// dropped), one more at each after.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// What the stack holds, as its bound counts it: `begin`'s bytes; and for each state but
    /// the session's, its list of chunks ([`LIST_BYTES`] and [`ENTRY_BYTES`] a chunk), and
    /// each chunk no other such state already counted ([`CHUNK_BYTES`] and its texts'
    /// bytes), or its bytes and [`WHOLE_BYTES`] if it's held whole.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The oldest state undo can reach: `begin`'s, until the bound drops a state.
    fn lowest(&self) -> usize {
        usize::from(self.dropped > 0)
    }

    /// The state the session was at at `to`, if the stack holds it and undo can reach it.
    fn find(&self, to: K) -> Option<usize> {
        let lowest = self.lowest();
        self.states
            .iter()
            .position(|state| state.revisions.contains(&to))
            .filter(|&found| found >= lowest)
    }

    fn not_held(&self, to: K) -> Refusal<K> {
        let oldest = self
            .states
            .get(self.lowest())
            .and_then(|state| state.revisions.first().copied())
            .unwrap_or(to);
        let newest = self
            .states
            .iter()
            .flat_map(|state| state.revisions.iter().copied())
            .max()
            .unwrap_or(to);
        Refusal::NotHeld {
            to,
            held: (oldest, newest),
        }
    }

    /// The state at `target` made the session's, at `revision`: the one it was before held
    /// from now, and `target`'s no longer.
    fn restore(&mut self, target: usize, revision: K) -> Restored<K, R> {
        if let Some(now) = self.states.get(self.at).map(|s| s.text.clone())
            && self.at > 0
        {
            self.hold(&now);
        }
        if let Some(then) = self.states.get(target).map(|s| s.text.clone())
            && target > 0
        {
            self.release(&then);
        }
        self.at = target;
        let undo = self.undo_left();
        let redo = self.redo_left();
        let head = Arc::clone(&self.head);
        let state = &mut self.states[target];
        state.revisions.push(revision);
        Restored {
            restored: state.revisions[0],
            text: written(&head, &state.text),
            reached: state.reached.clone(),
            undo,
            redo,
        }
    }

    /// `text` as the stack holds it: in chunks if it's `document`'s canonical form, each
    /// shared with the session's state now where its features' texts are the same; whole
    /// otherwise.
    fn held_text(&self, text: &[u8], document: Option<&Document>) -> Text {
        let whole = || Text::Whole(Arc::from(text));
        let Some(document) = document else {
            return whole();
        };
        let Ok(parts) = write::parts(document, MAX_DOCUMENT_LEN) else {
            return whole();
        };
        if *parts.head != *self.head
            || write::assemble(&parts.head, parts.features.iter().map(String::as_str)).as_bytes()
                != text
        {
            return whole();
        }
        let before = match self.states.get(self.at).map(|s| &s.text) {
            Some(Text::Chunked(chunks)) => chunks.as_slice(),
            _ => &[],
        };
        let chunks = parts
            .features
            .chunks(CHUNK)
            .enumerate()
            .map(|(i, texts)| {
                let chunk = Chunk {
                    texts: texts.iter().map(|t| Box::from(t.as_str())).collect(),
                };
                match before.get(i) {
                    Some(same) if **same == chunk => Arc::clone(same),
                    _ => Arc::new(chunk),
                }
            })
            .collect();
        Text::Chunked(chunks)
    }

    /// `text` counted as held by one more state.
    fn hold(&mut self, text: &Text) {
        match text {
            Text::Whole(bytes) => self.bytes += bytes.len() + WHOLE_BYTES,
            Text::Chunked(chunks) => {
                self.bytes += LIST_BYTES + ENTRY_BYTES * chunks.len();
                for chunk in chunks {
                    let (refs, cost) = self
                        .held
                        .entry(Arc::as_ptr(chunk) as usize)
                        .or_insert((0, chunk.cost()));
                    if *refs == 0 {
                        self.bytes += *cost;
                    }
                    *refs += 1;
                }
            }
        }
    }

    /// `text` counted as held by one state fewer.
    fn release(&mut self, text: &Text) {
        match text {
            Text::Whole(bytes) => {
                self.bytes = self.bytes.saturating_sub(bytes.len() + WHOLE_BYTES);
            }
            Text::Chunked(chunks) => {
                self.bytes = self
                    .bytes
                    .saturating_sub(LIST_BYTES + ENTRY_BYTES * chunks.len());
                for chunk in chunks {
                    let key = Arc::as_ptr(chunk) as usize;
                    if let Some((refs, cost)) = self.held.get_mut(&key) {
                        *refs = refs.saturating_sub(1);
                        if *refs == 0 {
                            self.bytes = self.bytes.saturating_sub(*cost);
                            self.held.remove(&key);
                        }
                    }
                }
            }
        }
    }
}

/// A state's text, as the session held it.
fn written(head: &str, text: &Text) -> Vec<u8> {
    match text {
        Text::Whole(bytes) => bytes.to_vec(),
        Text::Chunked(chunks) => {
            let texts: Vec<&str> = chunks
                .iter()
                .flat_map(|chunk| chunk.texts.iter().map(|text| &**text))
                .collect();
            write::assemble(head, texts.into_iter()).into_bytes()
        }
    }
}

fn empty_document() -> Document {
    Document {
        silico: super::FormatVersion,
        units: super::Units,
        naming: super::NamingScheme,
        features: Vec::new(),
    }
}
