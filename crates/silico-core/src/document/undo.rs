//! A transaction's undo stack (the M5.0 design, section 3's A2, section 6's bound, and
//! section 7's `undo` and `redo`; pending Trevin, decision 2): every state a transaction's
//! edits left, so `undo` and `redo` can restore any of them, byte for byte. The command
//! layer holds the stack, words its refusals, and works out again what the edits had
//! reached from the text restored (a function of it, and of what the transaction holds
//! fixed); this is what it holds, so a change of the commands (decision 2 vetoed, or
//! worded otherwise) leaves it as it is.
//!
//! A state is held as its features' canonical texts, in chunks of [`CHUNK`] features, each
//! chunk shared with the state before while none of its features changes: a state costs
//! its list of chunks and the chunks its edit changed. A text that isn't a document's
//! canonical form (one `replace_text` wrote, say) is held whole, sharing nothing, so an
//! undo gives it back as it was. The state `begin` saw is the file's bytes, held whole,
//! and never dropped (it's `rollback`'s). Past the bound, the oldest state after it is
//! dropped and counted, and `undo` then reaches neither it nor `begin`.
//!
//! An edit's state is made in two steps, so the costly one runs outside whatever lock the
//! stack is held under: [`Prepared::of`] writes a document's features' canonical texts
//! (and gives the text they make, which the edit keeps as its own), and
//! [`UndoStack::edit`] keeps them, comparing each with the state before's before it
//! copies any.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use super::json::MAX_DOCUMENT_LEN;
use super::{Document, too_long, write};
use crate::Error;

/// How much a transaction's undo stack holds at most, as [`UndoStack::bytes`] counts it:
/// 256 MiB (section 6).
pub const UNDO_BYTES: usize = 256 * 1024 * 1024;

/// How many features a chunk holds: a state's list of chunks has one entry for each.
pub const CHUNK: usize = 64;

/// What a state's list of chunks counts for, beyond its chunks: the list itself, and each
/// of its entries (constants, not measured sizes, as A2 has them; a state's own small
/// fixed size beside them isn't counted).
pub const LIST_BYTES: usize = 24;
pub const ENTRY_BYTES: usize = 8;

/// What a chunk counts for beyond its features' texts.
pub const CHUNK_BYTES: usize = 16;

/// What a state held whole counts for beyond its text's bytes, as a list of no chunks does.
pub const WHOLE_BYTES: usize = LIST_BYTES;

/// A document's features' canonical texts, as its canonical form places them: an edit's
/// state, made before the stack is touched, and the edit's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    features: Vec<String>,
}

impl Prepared {
    /// `document`'s features' canonical texts; refused, as `write` refuses it, if its text
    /// would be more than a document may be.
    pub fn of(document: &Document) -> Result<Prepared, Error> {
        let _span =
            tracing::debug_span!("document_prepare", features = document.features.len()).entered();
        write::placed(document, MAX_DOCUMENT_LEN)
            .map(|features| Prepared { features })
            .map_err(|write::TooLong| too_long(document, "the document is too large to write"))
    }

    /// The document's canonical text: what `write` writes for it, byte for byte.
    pub fn text(&self) -> String {
        write::assemble(self.features.iter().map(String::as_str))
    }
}

/// An edit's state, as [`UndoStack::edit`] keeps it: a document's canonical text, by its
/// features' ([`Prepared`]), or a text held whole, as it is.
#[derive(Debug, Clone)]
pub enum After {
    /// What a session's edits push: each holds its document's canonical text.
    Canonical(Prepared),
    /// Kept for a caller that holds the bytes it was given, not a document's canonical form
    /// (none of a session's edits does today): held as they are, sharing nothing.
    Whole(Arc<Vec<u8>>),
}

/// Up to [`CHUNK`] features' canonical texts, as the document places them, one after
/// another, each ending where `ends` says.
#[derive(Debug)]
struct Chunk {
    text: Box<str>,
    ends: Box<[usize]>,
}

impl Chunk {
    fn of(features: &[String]) -> Chunk {
        let mut text = String::with_capacity(features.iter().map(String::len).sum());
        let mut ends = Vec::with_capacity(features.len());
        for feature in features {
            text.push_str(feature);
            ends.push(text.len());
        }
        Chunk {
            text: text.into_boxed_str(),
            ends: ends.into_boxed_slice(),
        }
    }

    /// Its features' texts, in order.
    fn features(&self) -> impl Iterator<Item = &str> {
        let starts = std::iter::once(0).chain(self.ends.iter().copied());
        starts
            .zip(self.ends.iter().copied())
            .map(|(start, end)| self.text.get(start..end).unwrap_or_default())
    }

    /// Whether it holds exactly `features`' texts.
    fn holds(&self, features: &[String]) -> bool {
        self.ends.len() == features.len()
            && self
                .features()
                .zip(features)
                .all(|(held, text)| held == text)
    }

    /// What it counts for: its texts' bytes and [`CHUNK_BYTES`].
    fn cost(&self) -> usize {
        self.text.len() + CHUNK_BYTES
    }
}

/// A state's text, as the stack holds it.
#[derive(Debug, Clone)]
enum Text {
    /// Its bytes, as they were: `begin`'s, and any text that isn't a document's canonical
    /// form, shared with whoever else holds them.
    Whole(Arc<Vec<u8>>),
    /// A document's canonical text, its features in chunks.
    Chunked(Vec<Arc<Chunk>>),
}

/// A state the stack holds: its text, and every revision it was the session's state at,
/// the one it was made at first.
#[derive(Debug, Clone)]
struct State<K> {
    text: Text,
    revisions: Vec<K>,
}

/// A state restored by [`UndoStack::undo`] or [`UndoStack::redo`]: the revision it was
/// made at, its text, what's left to undo and redo, and how many more edits the bound left
/// out of undo's reach as it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored<K> {
    pub restored: K,
    pub text: Arc<Vec<u8>>,
    pub undo: usize,
    pub redo: usize,
    pub dropped: usize,
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
    /// `to` isn't a revision any state undo or redo can reach was the session's state at:
    /// the bound dropped it, or it's from before the transaction, or an edit dropped it
    /// from redo. `held` is every revision such a state was, in order, for the refusal to
    /// name.
    NotHeld { to: K, held: Vec<K> },
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
pub struct UndoStack<K> {
    /// Oldest first: `begin`'s, then each edit's, the redo states after the current one.
    states: VecDeque<State<K>>,
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

impl<K: Copy + Ord> UndoStack<K> {
    /// The stack of a transaction `begin` opened at `revision`, on a file holding `bytes`,
    /// bounded by `bound` bytes as [`UndoStack::bytes`] counts them ([`UNDO_BYTES`] for a
    /// session's).
    pub fn new(bytes: Arc<Vec<u8>>, revision: K, bound: usize) -> Self {
        let counted = bytes.len();
        UndoStack {
            states: VecDeque::from([State {
                text: Text::Whole(bytes),
                revisions: vec![revision],
            }]),
            at: 0,
            dropped: 0,
            bound,
            bytes: counted,
            held: BTreeMap::new(),
        }
    }

    /// An edit's state, at `revision`. The redo states are dropped, and past the bound,
    /// the oldest states after `begin`'s, but never the one before this edit.
    pub fn edit(&mut self, after: After, revision: K) -> Pushed {
        while self.states.len() > self.at + 1 {
            if let Some(state) = self.states.pop_back() {
                self.release(&state.text);
            }
        }
        let text = match after {
            After::Whole(bytes) => Text::Whole(bytes),
            After::Canonical(prepared) => Text::Chunked(self.chunked(&prepared)),
        };
        // The state before this edit is held from now: counted.
        self.hold_at(self.at);
        self.states.push_back(State {
            text,
            revisions: vec![revision],
        });
        self.at += 1;
        Pushed {
            // The oldest after `begin`'s, at 1, while it isn't the one before this edit, at
            // `at - 1`.
            dropped: self.drop_past_bound(3),
        }
    }

    /// The state before the session's, or with `to`, the state at that revision; the
    /// session's state now held for redo, at `revision`, a new one.
    pub fn undo(&mut self, to: Option<K>, revision: K) -> Result<Restored<K>, Refusal<K>> {
        let target = match to {
            None if self.at <= self.lowest() => return Err(Refusal::NothingToUndo),
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
    pub fn redo(&mut self, to: Option<K>, revision: K) -> Result<Restored<K>, Refusal<K>> {
        let target = match to {
            None if self.at + 1 >= self.states.len() => return Err(Refusal::NothingToRedo),
            None => self.at + 1,
            Some(to) => match self.find(to) {
                Some(found) if found > self.at => found,
                Some(_) => return Err(Refusal::WrongSide { to }),
                None => return Err(self.not_held(to)),
            },
        };
        Ok(self.restore(target, revision))
    }

    /// Whether a state after the session's, one [`UndoStack::redo`] can restore, was the
    /// session's state at `revision`: so one an edit would drop.
    pub fn redo_holds(&self, revision: K) -> bool {
        self.states
            .iter()
            .skip(self.at + 1)
            .any(|state| state.revisions.contains(&revision))
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
        let mut held: Vec<K> = self
            .states
            .iter()
            .skip(self.lowest())
            .flat_map(|state| state.revisions.iter().copied())
            .collect();
        held.sort_unstable();
        held.dedup();
        Refusal::NotHeld { to, held }
    }

    /// The state at `target` made the session's, at `revision`: the one it was before held
    /// from now, and `target`'s no longer; then, past the bound, the oldest states after
    /// `begin`'s dropped, but never the session's.
    fn restore(&mut self, target: usize, revision: K) -> Restored<K> {
        self.hold_at(self.at);
        if target > 0
            && let Some(then) = self.states.get(target).map(|s| s.text.clone())
        {
            self.release(&then);
        }
        self.at = target;
        let dropped = self.drop_past_bound(2);
        let (restored, text) = match self.states.get_mut(self.at) {
            Some(state) => {
                state.revisions.push(revision);
                (
                    state.revisions.first().copied().unwrap_or(revision),
                    written(&state.text),
                )
            }
            None => (revision, Arc::new(Vec::new())),
        };
        Restored {
            restored,
            text,
            undo: self.undo_left(),
            redo: self.redo_left(),
            dropped,
        }
    }

    /// Past the bound, the oldest state after `begin`'s dropped, while the session's is at
    /// `least` or later: how many more edits undo can no longer reach.
    fn drop_past_bound(&mut self, least: usize) -> usize {
        let mut dropped = 0;
        while self.bytes > self.bound && self.at >= least {
            let Some(state) = self.states.remove(1) else {
                break;
            };
            self.release(&state.text);
            self.at -= 1;
            let more = if self.dropped == 0 { 2 } else { 1 };
            self.dropped += more;
            dropped += more;
        }
        dropped
    }

    /// `prepared`'s features in chunks, each shared with the session's state now where its
    /// features' texts are the same, compared before any is copied.
    fn chunked(&self, prepared: &Prepared) -> Vec<Arc<Chunk>> {
        let before = match self.states.get(self.at).map(|s| &s.text) {
            Some(Text::Chunked(chunks)) => chunks.as_slice(),
            _ => &[],
        };
        prepared
            .features
            .chunks(CHUNK)
            .enumerate()
            .map(|(i, features)| match before.get(i) {
                Some(same) if same.holds(features) => Arc::clone(same),
                _ => Arc::new(Chunk::of(features)),
            })
            .collect()
    }

    /// The state at `at` counted as held, unless it's `begin`'s, counted always.
    fn hold_at(&mut self, at: usize) {
        if at > 0
            && let Some(text) = self.states.get(at).map(|s| s.text.clone())
        {
            self.hold(&text);
        }
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
fn written(text: &Text) -> Arc<Vec<u8>> {
    match text {
        Text::Whole(bytes) => Arc::clone(bytes),
        Text::Chunked(chunks) => {
            let features: Vec<&str> = chunks.iter().flat_map(|c| c.features()).collect();
            Arc::new(write::assemble(features.into_iter()).into_bytes())
        }
    }
}
