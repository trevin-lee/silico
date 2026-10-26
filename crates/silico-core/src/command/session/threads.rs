//! The threads a session starts and joins later ([`Threads`]), and its published reads
//! ([`Reads`]), at most [`MAX_PUBLISHED_READS`] unanswered at once, each with its own stop.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::cancel::Ticket;
use crate::command::ReadCommand;
use crate::worker::lock;

/// The most published reads one session runs at once: each holds a thread, and the session,
/// until it ends.
pub const MAX_PUBLISHED_READS: usize = 8;

/// Threads started and joined later (a session's, its rebuilds' watchers, its published
/// reads): those that have ended joined as more start, the rest when what holds them is
/// dropped, so none outlives it.
pub(super) struct Threads {
    pub(super) running: Vec<JoinHandle<()>>,
}

impl Threads {
    pub(super) fn new() -> Self {
        Threads {
            running: Vec::new(),
        }
    }

    pub(super) fn push(&mut self, thread: JoinHandle<()>) {
        self.running.push(thread);
    }

    pub(super) fn extend(&mut self, other: Threads) {
        self.running.extend(other.running);
    }

    /// Those that have ended, joined; the rest kept.
    pub(super) fn reap(&mut self) {
        let (ended, running): (Vec<_>, Vec<_>) = std::mem::take(&mut self.running)
            .into_iter()
            .partition(JoinHandle::is_finished);
        self.running = running;
        for thread in ended {
            let _ = thread.join();
        }
    }

    /// Every one joined, waiting for each to end.
    pub(super) fn join(self) {
        for thread in self.running {
            let _ = thread.join();
        }
    }
}

/// A session's published reads, each on a thread of its own, at most
/// [`MAX_PUBLISHED_READS`] unanswered at once.
pub(super) struct Reads {
    /// Those that have started and not answered, each with its stop: each kept until just
    /// before it answers (however it ends, by a panic too), so one just answered, its thread
    /// still ending, leaves room for the next, and `cancel` and `status` see only those
    /// running. Taken under no other of the session's locks but `reads`, and with none under
    /// it.
    pub(super) unanswered: Arc<Mutex<Unanswered>>,
    /// Their threads: those ended joined as the next starts, the rest by
    /// `Sessions::drop`; none once the session is released (its reads then the book's, and
    /// a read asked for after it refused as of a session not open).
    pub(super) threads: Option<Threads>,
}

impl Reads {
    pub(super) fn new() -> Self {
        Reads {
            unanswered: Arc::new(Mutex::new(Unanswered {
                next: 0,
                reads: BTreeMap::new(),
                closed: false,
            })),
            threads: Some(Threads::new()),
        }
    }
}

/// A session's published reads started and not answered, in the order they started.
pub(super) struct Unanswered {
    /// The number the next read takes.
    next: u64,
    /// Each one's command and stop, by its number. Each read keeps its kernel client with
    /// its stop (through [`Ticket::interrupt_with`]), so a cancel or the session's close
    /// retires the read's own client.
    reads: BTreeMap<u64, (ReadCommand, Arc<Ticket>)>,
    /// Whether the session closed ([`close`](Self::close)): kept, so a read counted after
    /// is closed at once.
    closed: bool,
}

impl Unanswered {
    /// How many there are.
    pub(super) fn len(&self) -> usize {
        self.reads.len()
    }

    /// Each one's stop, in the order they started.
    pub(super) fn tickets(&self) -> impl Iterator<Item = &Arc<Ticket>> {
        self.reads.values().map(|(_, ticket)| ticket)
    }

    /// Each one's command, in the order they started.
    pub(super) fn commands(&self) -> impl Iterator<Item = ReadCommand> + '_ {
        self.reads.values().map(|(read, _)| *read)
    }

    /// The session closed: so it stays, and each read's stop running now given back, for
    /// the caller to close ([`Ticket::close`]) once this lock is let go.
    pub(super) fn close(&mut self) -> Vec<Arc<Ticket>> {
        self.closed = true;
        self.tickets().cloned().collect()
    }
}

/// A published read counted unanswered, its stop kept with the session's reads, until it's
/// dropped: just before the read answers, or as its thread unwinds.
pub(super) struct Counted {
    unanswered: Arc<Mutex<Unanswered>>,
    number: u64,
}

impl Counted {
    /// The read `read` counted in `unanswered`, with its stop, `ticket`, which is made named
    /// by `read` (`Ticket::new(read.into())`, so the command `status` lists and the one
    /// `cancel` stops are one name): closed at once ([`Ticket::close`]) if the session has
    /// closed.
    pub(super) fn count(
        unanswered: &Arc<Mutex<Unanswered>>,
        read: ReadCommand,
        ticket: &Arc<Ticket>,
    ) -> Self {
        let (number, closed) = {
            let mut reads = lock(unanswered);
            let number = reads.next;
            reads.next = number.wrapping_add(1);
            reads.reads.insert(number, (read, Arc::clone(ticket)));
            (number, reads.closed)
        };
        if closed {
            ticket.close();
        }
        Counted {
            unanswered: Arc::clone(unanswered),
            number,
        }
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        lock(&self.unanswered).reads.remove(&self.number);
    }
}
