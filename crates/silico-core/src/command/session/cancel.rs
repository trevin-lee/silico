//! Stopping a command (docs/design/m4.1-command-api.md, section 6, decisions 10 and 34): each
//! command's [`Ticket`], which its [`Running`](super::Running) handle, the session's
//! `cancel`, and the command's own work share, so a command is either stopped before it
//! leaves a trace and answers `cancelled`, or has done its work and answers with its real
//! result, never half.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::worker::lock;
use crate::{Error, ErrorCode};

/// What a stopped command's work is told to stop: a wait it may be in (a session's, for its
/// rebuild), or, for a published read, the kernel work it asks for (its own client, which
/// M4.5's reads give each read and this retires).
pub(super) trait Interrupt: Send + Sync {
    fn interrupt(&self);
}

/// Running: neither stopped nor past the point where its work leaves a trace.
const PENDING: u8 = 0;
/// Its work done, or begun to be left (a save under way): a cancel comes too late.
const KEPT: u8 = 1;
/// Stopped before its work left a trace: it answers `cancelled`.
const CANCELLED: u8 = 2;

/// One command's stop, shared by its handle, its session's record of what's running, and its
/// work. A cancel and the work's [`keep`](Self::keep) race for it once: whichever comes
/// first decides, so a command stopped never leaves a trace, and one that left one is never
/// said to be stopped.
pub(super) struct Ticket {
    command: &'static str,
    state: AtomicU8,
    /// What a cancel interrupts, each told once it's cancelled (at once, if it's added after).
    interrupts: Mutex<Vec<Weak<dyn Interrupt>>>,
}

impl Ticket {
    /// The ticket of a command named `command`, as requests and `cancel` name it.
    pub(super) fn new(command: &'static str) -> Arc<Self> {
        Arc::new(Ticket {
            command,
            state: AtomicU8::new(PENDING),
            interrupts: Mutex::new(Vec::new()),
        })
    }

    /// The command's name.
    pub(super) fn command(&self) -> &'static str {
        self.command
    }

    /// Stops the command unless it has done its work, interrupting what it waits on:
    /// whether it was stopped before it had (so it answers `cancelled`), as `cancel`'s
    /// `stopped` says.
    pub(super) fn cancel(&self) -> bool {
        let stopped = match self.state.compare_exchange(
            PENDING,
            CANCELLED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => true,
            Err(now) => now == CANCELLED,
        };
        if stopped {
            let interrupts: Vec<_> = lock(&self.interrupts).clone();
            for interrupt in interrupts.iter().filter_map(Weak::upgrade) {
                interrupt.interrupt();
            }
        }
        stopped
    }

    /// Whether it was stopped before it had done its work.
    pub(super) fn cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == CANCELLED
    }

    /// `interrupt` told when the command is cancelled: at once, if it has been. M4.5's
    /// published reads add their kernel client here, so a cancel retires it.
    pub(super) fn interrupt_with(&self, interrupt: Weak<dyn Interrupt>) {
        lock(&self.interrupts).push(Weak::clone(&interrupt));
        if self.cancelled()
            && let Some(interrupt) = interrupt.upgrade()
        {
            interrupt.interrupt();
        }
    }

    /// The point past which the command's work leaves a trace (a save, a change of what the
    /// session holds), or its answer: from here on a cancel comes too late, and the command
    /// answers with its real result. Refused, `cancelled`, if it was stopped before.
    pub(super) fn keep(&self) -> Result<(), Error> {
        match self
            .state
            .compare_exchange(PENDING, KEPT, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => Ok(()),
            Err(KEPT) => Ok(()),
            Err(_) => Err(cancelled(self.command)),
        }
    }
}

/// The answer of `command`, stopped before it had done its work: the design's words
/// (section 7's `cancel`).
pub(super) fn cancelled(command: &str) -> Error {
    Error::new(
        ErrorCode::Cancelled,
        format!("{command} was cancelled before it finished"),
    )
    .with_suggestion(
        "run it again when you want its answer; every rebuild step that finished is kept in the cache",
    )
}

/// The refusal of a `cancel` of `command` in session `session`, which isn't running there:
/// what is (the queued command, then the published reads, each name once) said and given as
/// candidates, so a cancel that came late never stops the next command (decision 34).
pub(super) fn not_running(
    session: super::SessionId,
    command: &str,
    running: &[&'static str],
) -> Error {
    let named = plain(command);
    let message = match running {
        [] => format!("no {named} is running in session {session}"),
        [one] => format!("no {named} is running in session {session}: {one} is"),
        many => format!(
            "no {named} is running in session {session}: {} are",
            many.join(", ")
        ),
    };
    Error::new(ErrorCode::InvalidInput, message)
        .with_parameter("command")
        .with_suggestion("cancel names the command it means; read status to see what's running")
        .with_candidates(running.iter().copied())
}

/// A command's name as a message says it: as it is, if it's a name a command could have
/// (lowercase letters and `_`), or quoted, its control and invisible characters escaped.
fn plain(command: &str) -> String {
    if !command.is_empty() && command.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        command.to_owned()
    } else {
        crate::command::quoted(command)
    }
}

/// A kernel fault as the model read gives it (the envelope's `model.fault`, decision 32): kept
/// as the revision's outcome, so its suggestion says how to try again (a new revision; with
/// M4.3, `check` with `fresh` too), never the worker's own "try again", which reading again
/// wouldn't do. A worker that couldn't be had says nothing of the feature that asked for it,
/// so it names none.
pub(super) fn kept(fault: &Error) -> Error {
    const KEPT: &str = "this revision keeps the fault, so reading it again won't try again";
    let fault = fault.clone();
    match fault.code() {
        ErrorCode::WorkerUnavailable => fault.without_feature().with_suggestion(format!(
            "once a worker can be had (the log says why none could), edit the document to try again: {KEPT}"
        )),
        ErrorCode::ResourceExhausted => fault.with_suggestion(format!(
            "free what's short, then edit the document to try again: {KEPT}"
        )),
        _ => fault.with_suggestion(format!(
            "edit the document to try again: {KEPT}; if the same feature faults again, report it with the document"
        )),
    }
}
