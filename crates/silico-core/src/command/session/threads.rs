//! The threads a session starts and joins later ([`Threads`]), and its published reads
//! ([`Reads`]), at most [`MAX_PUBLISHED_READS`] unanswered at once.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;

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
    /// How many have started and not answered: each counted until just before it answers
    /// (however it ends, by a panic too), so one just answered, its thread still ending,
    /// leaves room for the next.
    pub(super) unanswered: Arc<AtomicUsize>,
    /// Their threads: those ended joined as the next starts, the rest by
    /// `Sessions::drop`; none once the session is released (its reads then the book's, and
    /// a read asked for after it refused as of a session not open).
    pub(super) threads: Option<Threads>,
}

impl Reads {
    pub(super) fn new() -> Self {
        Reads {
            unanswered: Arc::new(AtomicUsize::new(0)),
            threads: Some(Threads::new()),
        }
    }
}

/// A published read counted unanswered, until it's dropped: just before the read answers,
/// or as its thread unwinds.
pub(super) struct Unanswered(Arc<AtomicUsize>);

impl Unanswered {
    pub(super) fn count(unanswered: &Arc<AtomicUsize>) -> Self {
        unanswered.fetch_add(1, Ordering::SeqCst);
        Unanswered(Arc::clone(unanswered))
    }
}

impl Drop for Unanswered {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
