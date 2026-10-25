//! Stopping a command (docs/design/m4.1-command-api.md, section 6, decisions 10 and 34): each
//! command's [`Ticket`], which its [`Running`](super::Running) handle, the session's
//! `cancel`, and the command's own work share, so a command is either stopped before it
//! leaves a trace and answers `cancelled`, or has done its work and answers with its real
//! result, never half.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::{Problems, Response, SessionInfo};
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// What a stopped command's work is told to stop: a wait it may be in (a session's, for its
/// rebuild), or, for a published read, the kernel work it asks for (its own client, which
/// M4.5's reads give each read and this retires), told why. It may be told more than once (a
/// cancel and a close, or a cancel racing the interrupt's adding), so telling it again must
/// do nothing more.
pub(super) trait Interrupt: Send + Sync {
    fn interrupt(&self, why: Why);
}

/// Why a command's work is interrupted, which says how it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Why {
    /// It was cancelled (decision 34): it answers `cancelled`, unless it had settled.
    Cancelled,
    /// Its session closed: a published read answers as of a session that closed, never
    /// `cancelled`, since no client stopped it.
    Closed,
}

/// Running: neither stopped nor past the point where its work leaves a trace.
const PENDING: u8 = 0;
/// Settled: its work done, or begun to be left (a save under way), so a cancel comes too
/// late.
const SETTLED: u8 = 1;
/// Stopped before its work left a trace: it answers `cancelled`.
const CANCELLED: u8 = 2;

/// One command's stop, shared by its handle, its session's record of what's running, and its
/// work. A cancel and the work's [`settle`](Self::settle) race for it once: whichever comes
/// first decides, so a command stopped never leaves a trace, and one that left one is never
/// said to be stopped.
pub(super) struct Ticket {
    command: &'static str,
    state: AtomicU8,
    /// Whether its session closed ([`close`](Self::close)): kept, so what's interrupted
    /// after is interrupted at once.
    closed: AtomicBool,
    /// What it interrupts, each told when it's cancelled or its session closes (at once, if
    /// it's added after): each added once, and none whose owner has gone kept, though one may
    /// be told more than once ([`Interrupt`]).
    interrupts: Mutex<Vec<Weak<dyn Interrupt>>>,
}

impl Ticket {
    /// The ticket of a command named `command`, as requests and `cancel` name it.
    pub(super) fn new(command: &'static str) -> Arc<Self> {
        Arc::new(Ticket {
            command,
            state: AtomicU8::new(PENDING),
            closed: AtomicBool::new(false),
            interrupts: Mutex::new(Vec::new()),
        })
    }

    /// The command's name.
    pub(super) fn command(&self) -> &'static str {
        self.command
    }

    /// Stops the command unless it has settled, interrupting what it waits on: whether it
    /// was stopped before it had (so it answers `cancelled`), as `cancel`'s `stopped` says.
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
            self.tell(Why::Cancelled);
        }
        stopped
    }

    /// Its session closed: what it interrupts told so, and, from here, what's added after
    /// at once. Its state is left as it is: a command its session's close interrupts isn't
    /// cancelled.
    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.tell(Why::Closed);
    }

    /// Whether it was stopped before it had settled.
    pub(super) fn cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == CANCELLED
    }

    /// `interrupt` told when the command is cancelled or its session closes: at once, if
    /// either has come. Added once, however often it's given, and those whose owners have
    /// gone let go. M4.5's published reads add their kernel client here, so a cancel or a
    /// close retires it.
    pub(super) fn interrupt_with(&self, interrupt: Weak<dyn Interrupt>) {
        {
            let mut interrupts = lock(&self.interrupts);
            interrupts.retain(|kept| kept.strong_count() > 0);
            if !interrupts.iter().any(|kept| Weak::ptr_eq(kept, &interrupt)) {
                interrupts.push(Weak::clone(&interrupt));
            }
        }
        let why = if self.cancelled() {
            Some(Why::Cancelled)
        } else if self.closed.load(Ordering::SeqCst) {
            Some(Why::Closed)
        } else {
            None
        };
        if let Some(why) = why
            && let Some(interrupt) = interrupt.upgrade()
        {
            interrupt.interrupt(why);
        }
    }

    /// Each of what it interrupts told `why`, with its list let go first.
    fn tell(&self, why: Why) {
        let interrupts: Vec<_> = lock(&self.interrupts).clone();
        for interrupt in interrupts.iter().filter_map(Weak::upgrade) {
            interrupt.interrupt(why);
        }
    }

    /// The point past which the command's work leaves a trace (a save, a change of what the
    /// session holds), or its answer: from here on a cancel comes too late, and the command
    /// answers with its real result. Refused, `cancelled`, if it was stopped before. Once
    /// settled, it stays so.
    pub(super) fn settle(&self) -> Result<(), Error> {
        match self
            .state
            .compare_exchange(PENDING, SETTLED, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) | Err(SETTLED) => Ok(()),
            Err(_) => Err(cancelled(self.command)),
        }
    }

    /// The command's response, as its stop leaves it: `run`'s, unless the command was
    /// cancelled before it settled, then `cancelled`, the session as `run` left it; or, if
    /// it was cancelled before it ran, `run` never called, and the session as `unrun` gives
    /// it. Settled either way, so a cancel after it's answered says it came too late.
    pub(super) fn answer<T>(
        &self,
        unrun: impl FnOnce() -> Option<SessionInfo>,
        run: impl FnOnce() -> Response<T>,
    ) -> Response<T> {
        let response = if self.cancelled() {
            Response {
                session: unrun(),
                model: None,
                result: Err(Problems::from(cancelled(self.command))),
            }
        } else {
            run()
        };
        match self.settle() {
            Ok(()) => response,
            Err(cancelled) => Response {
                session: response.session,
                model: None,
                result: Err(Problems::from(cancelled)),
            },
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
/// what is (the queued command, then each published read, as `status` lists them) said, and
/// given as candidates, each name once, in the order first seen, since a candidate is a name
/// to pick: so a cancel that came late never stops the next command (decision 34).
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
        .with_candidates(running.iter().fold(Vec::new(), |mut once, &name| {
            if !once.contains(&name) {
                once.push(name);
            }
            once
        }))
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
