//! Hostile runs of `undo` and `redo` read back through `status` (break-it, PR #300), where
//! only this crate can bound a session's undo stack: thousands of seeded edits, undos, and
//! redos (`to` any revision, the session's own and ones never made among them) at bounds
//! from nothing to none, and after each, `status`'s transaction checked whole against a
//! model of the stack kept here: its states, the revisions each was the session's at, the
//! session's state, the edits dropped, `bytes` recounted from A2's rule (`begin`'s file,
//! and each state but `begin`'s and the session's, 24 and 8 for its one chunk's list and 16
//! and its 126 bytes for the chunk, none shared since each edit changes the one feature),
//! and `changed` from whether the session's state is `begin`'s (the only state 40 long).
//! Each step's own result, and each refusal's kind, are checked against the same model.
//! And `status` answering while the kernel holds a rebuild of a state undone and redone.

use super::*;
use crate::document::json::Node;

/// What A2 counts for `begin`'s state: [`BOX`]'s bytes.
fn begin_bytes() -> usize {
    BOX.len()
}

/// What A2 counts for a state of [`canonical`] with a two-digit length: its list of one
/// chunk (24 and 8), and the chunk (16, its four spaces of indent and 122 bytes of feature).
const STATE: usize = 24 + 8 + 16 + 4 + 122;

/// [`BOX`] `length` long, in canonical form, written by hand from M2.1's layout.
fn canonical(length: u32) -> String {
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 6,\n  \"features\": [\n    {{\"id\": \"k7f2qx3m\", \"type\": \"box\", \"v\": 1, \"name\": \"Block\", \"position\": [0, 0, 0], \"length\": {length}, \"width\": 20, \"height\": 10}}\n  ]\n}}\n"
    )
}

fn bounded_box(bound: usize) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("a.silico");
    std::fs::write(&path, BOX).unwrap();
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

/// A state of the model: its length (40 is `begin`'s alone), and every revision it was the
/// session's at, the one it was made at first.
#[derive(Debug, Clone)]
struct ModelState {
    length: u32,
    revisions: Vec<u64>,
}

/// The undo stack as A2 and section 7 describe it, kept by hand.
#[derive(Debug)]
struct Stack {
    states: Vec<ModelState>,
    at: usize,
    dropped: usize,
    bound: usize,
    revision: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum Refused {
    Nothing,
    NotHeld,
    WrongSide,
}

impl Stack {
    fn new(bound: usize) -> Self {
        Stack {
            states: vec![ModelState {
                length: 40,
                revisions: vec![1],
            }],
            at: 0,
            dropped: 0,
            bound,
            revision: 1,
        }
    }

    /// A2's count: `begin`'s bytes always, and each other state but the session's.
    fn bytes(&self) -> usize {
        begin_bytes() + (1..self.states.len()).filter(|&i| i != self.at).count() * STATE
    }

    fn lowest(&self) -> usize {
        usize::from(self.dropped > 0)
    }

    fn undo_left(&self) -> usize {
        self.at - self.lowest().min(self.at)
    }

    fn redo_left(&self) -> usize {
        self.states.len() - self.at - 1
    }

    /// Past the bound, the oldest state after `begin`'s dropped while the session's is at
    /// `least` or later: 2 for the first drop, 1 for each after.
    fn drop_past_bound(&mut self, least: usize) -> usize {
        let mut more = 0;
        while self.bytes() > self.bound && self.at >= least {
            self.states.remove(1);
            self.at -= 1;
            let n = if self.dropped == 0 { 2 } else { 1 };
            self.dropped += n;
            more += n;
        }
        more
    }

    fn edit(&mut self, length: u32) {
        self.revision += 1;
        self.states.truncate(self.at + 1);
        self.states.push(ModelState {
            length,
            revisions: vec![self.revision],
        });
        self.at += 1;
        self.drop_past_bound(3);
    }

    fn find(&self, to: u64) -> Option<usize> {
        self.states
            .iter()
            .position(|s| s.revisions.contains(&to))
            .filter(|&found| found >= self.lowest())
    }

    /// The step `undo` (or redo), `to`: what it restores and says, or why it's refused.
    fn step(&mut self, undo: bool, to: Option<u64>) -> Result<Stepped, Refused> {
        let target = match (undo, to) {
            (true, None) if self.at <= self.lowest() => return Err(Refused::Nothing),
            (true, None) => self.at - 1,
            (false, None) if self.at + 1 >= self.states.len() => return Err(Refused::Nothing),
            (false, None) => self.at + 1,
            (_, Some(to)) => match self.find(to) {
                Some(found) if undo && found < self.at => found,
                Some(found) if !undo && found > self.at => found,
                Some(_) => return Err(Refused::WrongSide),
                None => return Err(Refused::NotHeld),
            },
        };
        self.revision += 1;
        self.at = target;
        let dropped = self.drop_past_bound(2);
        let state = &mut self.states[self.at];
        state.revisions.push(self.revision);
        Ok(Stepped {
            restored: Revision(state.revisions[0]),
            undo: self.undo_left(),
            redo: self.redo_left(),
            dropped,
        })
    }

    fn length(&self) -> u32 {
        self.states[self.at].length
    }

    fn text(&self) -> String {
        match self.length() {
            40 => BOX.to_owned(),
            length => canonical(length),
        }
    }

    fn transaction(&self) -> Transaction {
        Transaction {
            base: Revision(1),
            changed: self.length() != 40,
            undo: self.undo_left(),
            redo: self.redo_left(),
            dropped: self.dropped,
            bytes: self.bytes(),
        }
    }
}

fn refusal_kind(problems: &Problems) -> Refused {
    let message = problems.errors()[0].message();
    if message.starts_with("nothing to ") {
        Refused::Nothing
    } else if message.contains("isn't a state of") {
        Refused::NotHeld
    } else if message.contains("isn't before") || message.contains("isn't after") {
        Refused::WrongSide
    } else {
        panic!("{problems:?}")
    }
}

/// `status`'s revision and transaction.
fn status_of(f: &Fixture) -> (Revision, Transaction) {
    let status = f.sessions.status(&f.id);
    (
        status.session.unwrap().revision,
        status.result.unwrap().transaction.unwrap(),
    )
}

fn run(bound: usize, seed: u64, steps: usize) {
    let f = bounded_box(bound);
    f.sessions.begin(&f.id).wait().result.unwrap();
    let mut model = Stack::new(bound);
    let mut seed = seed;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for step in 0..steps {
        let at = Revision(model.revision);
        match next() % 5 {
            0 | 1 => {
                // Two digits, never 40 (`begin`'s alone), never the session's own.
                let mut length = 10 + u32::try_from(next() % 90).unwrap();
                if length == 40 || length == model.length() {
                    length = if model.length() == 98 { 97 } else { 98 };
                }
                let value: Node =
                    crate::document::json::read(length.to_string().as_bytes()).unwrap();
                let response = f
                    .sessions
                    .set_field(&f.id, at, id("k7f2qx3m"), "length".to_owned(), value)
                    .wait();
                assert!(response.result.unwrap().changed, "step {step}");
                model.edit(length);
            }
            pick => {
                let undo = pick < 4;
                let to = match next() % 4 {
                    0 => None,
                    // Any revision, the session's own and some never made among them.
                    _ => Some(next() % (model.revision + 3)),
                };
                let response = if undo {
                    f.sessions.undo(&f.id, at, to.map(Revision)).wait()
                } else {
                    f.sessions.redo(&f.id, at, to.map(Revision)).wait()
                };
                let want = model.step(undo, to);
                match (response.result, want) {
                    (Ok(got), Ok(want)) => assert_eq!(got, want, "step {step}: {undo} {to:?}"),
                    (Err(problems), Err(want)) => {
                        assert_eq!(refusal_kind(&problems), want, "step {step}: {problems:?}");
                        assert_eq!(
                            problems.errors()[0].parameter(),
                            Some(if want == Refused::Nothing {
                                "session"
                            } else {
                                "to"
                            }),
                            "step {step}"
                        );
                    }
                    (got, want) => panic!("step {step} {undo} {to:?}: {got:?}, not {want:?}"),
                }
            }
        }
        let (revision, transaction) = status_of(&f);
        assert_eq!(revision, Revision(model.revision), "step {step}");
        assert_eq!(
            transaction,
            model.transaction(),
            "step {step} at bound {bound}: {model:?}"
        );
        assert_eq!(
            f.sessions.text(&f.id, None).wait().result.unwrap().text,
            model.text(),
            "step {step}"
        );
    }
    // The commit writes exactly when the transaction says it changed.
    let changed = model.length() != 40;
    let committed = f.sessions.commit(&f.id).wait().result.unwrap();
    assert_eq!(committed.written, changed);
    if !changed {
        assert_eq!(f.file(), BOX);
    }
}

#[test]
fn status_counts_follow_a_model_of_the_stack_through_seeded_runs_at_every_kind_of_bound() {
    let b = begin_bytes();
    for (bound, seed) in [
        (0, 0x9e37_79b9_7f4a_7c15),
        (b + STATE, 0x2545_f491_4f6c_dd1d),
        (b + 2 * STATE, 0xd1b5_4a32_d192_ed03),
        (b + 3 * STATE + 7, 0x8cb9_2ba7_2f3d_8dd7),
        (usize::MAX, 0x6a09_e667_f3bc_c909),
    ] {
        run(bound, seed, 600);
    }
}

#[test]
fn status_answers_while_the_kernel_holds_the_rebuild_of_a_state_undone_and_redone() {
    // The rebuild of revision 2's state held in the kernel: an undo and a redo each answer,
    // and `status` between them gives each step's counts, never waiting on the rebuild.
    let f = bounded_box(usize::MAX);
    f.sessions.begin(&f.id).wait().result.unwrap();
    let value: Node = crate::document::json::read(b"41").unwrap();
    f.sessions
        .set_field(
            &f.id,
            Revision(1),
            id("k7f2qx3m"),
            "length".to_owned(),
            value,
        )
        .wait()
        .result
        .unwrap();
    let hold = f.kernel.hold(is_box);
    let _ = f.sessions.published(&f.id);
    assert!(f.kernel.wait_running(1, WAIT));
    let (revision, transaction) = status_of(&f);
    assert_eq!(revision, Revision(2));
    assert_eq!(
        (transaction.changed, transaction.undo, transaction.redo),
        (true, 1, 0)
    );
    f.sessions
        .undo(&f.id, Revision(2), None)
        .wait()
        .result
        .unwrap();
    let (revision, transaction) = status_of(&f);
    assert_eq!(revision, Revision(3));
    assert_eq!(
        transaction,
        Transaction {
            base: Revision(1),
            changed: false,
            undo: 0,
            redo: 1,
            dropped: 0,
            bytes: begin_bytes() + STATE,
        }
    );
    f.sessions
        .redo(&f.id, Revision(3), None)
        .wait()
        .result
        .unwrap();
    let (revision, transaction) = status_of(&f);
    assert_eq!(revision, Revision(4));
    assert_eq!(
        transaction,
        Transaction {
            base: Revision(1),
            changed: true,
            undo: 1,
            redo: 0,
            dropped: 0,
            bytes: begin_bytes(),
        }
    );
    drop(hold);
}
