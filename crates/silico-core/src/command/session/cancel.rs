//! Stopping a command (docs/design/m4.1-command-api.md, section 6, decisions 10 and 34): each
//! command's [`Ticket`], which its [`Running`](super::Running) handle, the session's
//! `cancel`, and the command's own work share, so a command is either stopped before it
//! leaves a trace and answers `cancelled`, or has done its work and answers with its real
//! result, never half.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::{Problems, Response, SessionInfo};
use crate::command::{RunningCommand, SessionCommand};
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// What a stopped command's work is told to stop: a wait it may be in (a session's, for its
/// rebuild), or, for a read of a model, the kernel work it asks for (its own client, which
/// each read is given and this retires), told why. It may be told more than once (a
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
    /// What it stops: `open`'s, its handle's alone, which runs before its session is there
    /// for `cancel` to name it; a command, which `cancel` names; or the session's own
    /// reload, which nothing names to stop.
    of: Of,
    state: AtomicU8,
    /// Whether its session closed ([`close`](Self::close)): kept, so what's interrupted
    /// after is interrupted at once.
    closed: AtomicBool,
    /// What it interrupts, each told when it's cancelled or its session closes (at once, if
    /// it's added after): each added once, and none whose owner has gone kept, though one may
    /// be told more than once ([`Interrupt`]).
    interrupts: Mutex<Vec<Weak<dyn Interrupt>>>,
}

/// What a [`Ticket`] stops.
#[derive(Clone, Copy)]
enum Of {
    Open,
    Command(SessionCommand),
    Reload,
}

impl Ticket {
    /// The ticket of `command`, which `cancel` names.
    pub(super) fn new(command: SessionCommand) -> Arc<Self> {
        Self::of(Of::Command(command))
    }

    /// The ticket of `open`, which only its handle stops.
    pub(super) fn opening() -> Arc<Self> {
        Self::of(Of::Open)
    }

    /// The ticket of the session's own reload (docs/design/m5.0-app.md, A3): `status` names
    /// it `reload`, and nothing stops it but its session's close.
    pub(super) fn reloading() -> Arc<Self> {
        Self::of(Of::Reload)
    }

    fn of(of: Of) -> Arc<Self> {
        Arc::new(Ticket {
            of,
            state: AtomicU8::new(PENDING),
            closed: AtomicBool::new(false),
            interrupts: Mutex::new(Vec::new()),
        })
    }

    /// The command it stops, which `cancel` names: none for `open`'s or a reload's.
    pub(super) fn command(&self) -> Option<SessionCommand> {
        match self.of {
            Of::Command(command) => Some(command),
            Of::Open | Of::Reload => None,
        }
    }

    /// What it stops, as `status` says it's running: none for `open`'s, which runs before
    /// its session is there to ask.
    pub(super) fn running(&self) -> Option<RunningCommand> {
        match self.of {
            Of::Command(command) => Some(RunningCommand::Command(command)),
            Of::Reload => Some(RunningCommand::Reload),
            Of::Open => None,
        }
    }

    /// The command's name, as requests name it (`reload` for the session's own).
    pub(super) fn name(&self) -> &'static str {
        match self.of {
            Of::Open => "open",
            Of::Command(command) => command.as_str(),
            Of::Reload => RunningCommand::Reload.as_str(),
        }
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
    /// gone let go. Each read of a model adds its kernel client here, so a cancel or a
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
            Err(_) => Err(cancelled(self.name())),
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
                result: Err(Problems::from(cancelled(self.name()))),
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
    command: SessionCommand,
    running: &[SessionCommand],
) -> Error {
    let names: Vec<&str> = running.iter().map(|running| running.as_str()).collect();
    let message = match names.as_slice() {
        [] => format!("no {command} is running in session {session}"),
        [one] => format!("no {command} is running in session {session}: {one} is"),
        many => format!(
            "no {command} is running in session {session}: {} are",
            many.join(", ")
        ),
    };
    Error::new(ErrorCode::InvalidInput, message)
        .with_parameter("command")
        .with_suggestion("cancel names the command it means; read status to see what's running")
        .with_candidates(names.iter().fold(Vec::new(), |mut once, &name| {
            if !once.contains(&name) {
                once.push(name);
            }
            once
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a cancel interrupts, as a read's kernel client is: each why it's told.
    struct Told(Mutex<Vec<Why>>);

    impl Interrupt for Told {
        fn interrupt(&self, why: Why) {
            lock(&self.0).push(why);
        }
    }

    fn told() -> Arc<Told> {
        Arc::new(Told(Mutex::new(Vec::new())))
    }

    fn weak(told: &Arc<Told>) -> Weak<dyn Interrupt> {
        Arc::downgrade(told) as Weak<dyn Interrupt>
    }

    #[test]
    fn an_interrupt_whose_owner_has_gone_is_let_go_as_the_next_is_added_and_the_rest_are_told() {
        // `interrupt_with`'s rule: "none whose owner has gone kept". Three added, the second's
        // owner gone before the third: two kept, the first and the third, each told of the
        // cancel once.
        let ticket = Ticket::new(SessionCommand::Text);
        let first = told();
        let gone = told();
        ticket.interrupt_with(weak(&first));
        ticket.interrupt_with(weak(&gone));
        assert_eq!(lock(&ticket.interrupts).len(), 2);
        drop(gone);
        let third = told();
        ticket.interrupt_with(weak(&third));
        let kept = lock(&ticket.interrupts).clone();
        assert_eq!(kept.len(), 2);
        assert!(Weak::ptr_eq(&kept[0], &weak(&first)));
        assert!(Weak::ptr_eq(&kept[1], &weak(&third)));
        assert!(ticket.cancel());
        assert_eq!(lock(&first.0).as_slice(), [Why::Cancelled]);
        assert_eq!(lock(&third.0).as_slice(), [Why::Cancelled]);
    }
}
