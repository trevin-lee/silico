//! Calls in flight (docs/design/m4.9-mcp.md, *Running a call*): each by its ID until it's
//! answered or cancelled, the slots the calls that wait on a session hold until their
//! waiters end, and the answers being written; and the waiter that waits for a call's
//! answer, sends its progress, and writes it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use silico_core::command::{Answered, Calling, Stopper};

use crate::frame::Id;
use crate::reply::{self, Output};
use crate::{Control, MAX_IN_FLIGHT, PROGRESS_STEP, lock};

/// The calls in flight: by their IDs, those not yet answered or cancelled; how many hold a
/// slot, until their waiters end; and how many answers are being written.
pub(crate) struct Flights {
    table: Mutex<Table>,
    control: Arc<Control>,
}

struct Table {
    calls: BTreeMap<Id, Arc<Flight>>,
    slotted: usize,
    /// Calls answered whose answers aren't written yet.
    writing: usize,
}

/// A call in flight.
struct Flight {
    fate: Mutex<Fate>,
    stopper: Mutex<Option<Stopper>>,
    slotted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fate {
    Running,
    Answered,
    Cancelled,
}

impl Flights {
    /// No calls in flight; `control` told when the last answer being written is written.
    pub(crate) fn new(control: Arc<Control>) -> Self {
        Flights {
            table: Mutex::new(Table {
                calls: BTreeMap::new(),
                slotted: 0,
                writing: 0,
            }),
            control,
        }
    }

    pub(crate) fn contains(&self, id: &Id) -> bool {
        lock(&self.table).calls.contains_key(id)
    }

    /// A new call `id`, holding a slot if `slotted`: none if every slot is held.
    pub(crate) fn take(self: &Arc<Self>, id: &Id, slotted: bool) -> Option<Landing> {
        let mut table = lock(&self.table);
        if slotted {
            if table.slotted >= MAX_IN_FLIGHT {
                return None;
            }
            table.slotted += 1;
        }
        let flight = Arc::new(Flight {
            fate: Mutex::new(Fate::Running),
            stopper: Mutex::new(None),
            slotted,
        });
        table.calls.insert(id.clone(), Arc::clone(&flight));
        Some(Landing {
            flights: Arc::clone(self),
            id: id.clone(),
            flight,
        })
    }

    /// Cancels the call `id`, if it's in flight and not answered (MCP's cancellation): it
    /// gets no reply. Whether it was.
    pub(crate) fn cancel(&self, id: &Id) -> bool {
        let flight = lock(&self.table).calls.get(id).cloned();
        match flight {
            Some(flight) => {
                let cancelled = flight.cancel();
                forget(&mut lock(&self.table), id, &flight);
                cancelled
            }
            None => false,
        }
    }

    /// Cancels every call in flight: from now, no call is answered but those answered
    /// already, whose answers are being written ([`writing`](Self::writing)).
    pub(crate) fn cancel_all(&self) {
        let calls = std::mem::take(&mut lock(&self.table).calls);
        for flight in calls.values() {
            flight.cancel();
        }
    }

    /// How many calls are answered whose answers aren't written yet.
    pub(crate) fn writing(&self) -> usize {
        lock(&self.table).writing
    }

    /// Marks `flight`, the call `id`, answered, unless it's cancelled: if it is, what counts
    /// its answer as being written until it's dropped. Under the table's lock, so
    /// [`cancel_all`](Self::cancel_all) comes either before (and the call isn't answered)
    /// or after (and its answer is counted).
    fn answer(&self, id: &Id, flight: &Arc<Flight>) -> Option<Writing<'_>> {
        let mut table = lock(&self.table);
        if !flight.answer() {
            return None;
        }
        // No longer in flight by its ID before its answer is written, so the client may
        // give the ID again as soon as it reads it.
        forget(&mut table, id, flight);
        table.writing += 1;
        Some(Writing { flights: self })
    }
}

/// `flight` no longer in flight by `id`, if it's still the call `id`.
fn forget(table: &mut Table, id: &Id, flight: &Arc<Flight>) {
    if table.calls.get(id).is_some_and(|f| Arc::ptr_eq(f, flight)) {
        table.calls.remove(id);
    }
}

/// A call's answer being written, until it's dropped (a waiter that panics mid-write
/// included): the last tells `serve`, which may be waiting for it.
struct Writing<'a> {
    flights: &'a Flights,
}

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        let mut table = lock(&self.flights.table);
        table.writing = table.writing.saturating_sub(1);
        if table.writing == 0 {
            self.flights.control.written();
        }
    }
}

impl Flight {
    /// Marks it cancelled and stops it, unless it's answered: whether it was cancelled.
    fn cancel(&self) -> bool {
        {
            let mut fate = lock(&self.fate);
            if *fate != Fate::Running {
                return false;
            }
            *fate = Fate::Cancelled;
        }
        if let Some(stopper) = lock(&self.stopper).as_ref() {
            stopper.cancel();
        }
        true
    }

    /// Marks it answered, unless it's cancelled: whether its answer is to be written.
    fn answer(&self) -> bool {
        let mut fate = lock(&self.fate);
        if *fate != Fate::Running {
            return false;
        }
        *fate = Fate::Answered;
        true
    }

    fn cancelled(&self) -> bool {
        *lock(&self.fate) == Fate::Cancelled
    }
}

/// A call's place in flight, given up when it's dropped (its waiter ends): its slot, and
/// its ID if it's still in flight.
pub(crate) struct Landing {
    flights: Arc<Flights>,
    id: Id,
    flight: Arc<Flight>,
}

impl Landing {
    /// What cancels the call: `stopper`, its command's, from now.
    pub(crate) fn stopped_by(&self, stopper: Stopper) {
        *lock(&self.flight.stopper) = Some(stopper);
    }
}

impl Drop for Landing {
    fn drop(&mut self) {
        let mut table = lock(&self.flights.table);
        forget(&mut table, &self.id, &self.flight);
        if self.flight.slotted {
            table.slotted = table.slotted.saturating_sub(1);
        }
    }
}

/// What waits for a call's answer and writes it.
pub(crate) struct Waiter {
    pub(crate) id: Id,
    pub(crate) landing: Landing,
    /// The call's progress token, if it asked for progress.
    pub(crate) token: Option<Id>,
    pub(crate) output: Arc<Output>,
}

impl Waiter {
    pub(crate) fn run(self, calling: Calling) {
        let _span = tracing::info_span!("tools/call", id = self.id.raw()).entered();
        let answered = match &self.token {
            None => calling.wait(),
            Some(token) => self.with_progress(calling, token),
        };
        self.deliver(&answered);
    }

    /// Writes `answered`, the call's answer, unless the call was cancelled first.
    pub(crate) fn deliver(&self, answered: &Answered) {
        let landing = &self.landing;
        match landing.flights.answer(&self.id, &landing.flight) {
            Some(_writing) => {
                self.output.send(reply::answer(&self.id, answered));
            }
            None => tracing::debug!("cancelled: its answer isn't sent"),
        }
    }

    /// The call's answer, waited for in steps, its progress sent between ([`Count`]), with
    /// the step running as its message, only while the call isn't cancelled and when the
    /// output is free.
    fn with_progress(&self, mut calling: Calling, token: &Id) -> Answered {
        let mut count = Count::default();
        loop {
            match calling.wait_timeout(PROGRESS_STEP) {
                Ok(answered) => return answered,
                Err(still) => calling = still,
            }
            let Some(progress) = calling.progress() else {
                continue;
            };
            if let Some(grown) = count.seen(progress.done)
                && !self.landing.flight.cancelled()
                && self.output.send_if_free(reply::progress(
                    token,
                    grown,
                    progress.running.as_deref(),
                ))
            {
                count.sent(grown);
            }
        }
    }
}

/// A call's progress, as MCP has it, every notification's greater than the last: a count
/// that grows by each step seen done and never falls (the session's newest rebuild may be
/// another than the call's own, whose `done` can fall), sent only when it has grown past
/// the last sent.
#[derive(Debug, Default)]
struct Count {
    count: u64,
    sent: u64,
    /// The newest rebuild's `done` when last seen.
    last: usize,
}

impl Count {
    /// The newest rebuild's steps done are now `done`: the count to send, if it has grown
    /// past the last sent.
    fn seen(&mut self, done: usize) -> Option<u64> {
        self.count = grown(self.count, self.last, done);
        self.last = done;
        (self.count > self.sent).then_some(self.count)
    }

    /// `count` was sent.
    fn sent(&mut self, count: u64) {
        self.sent = count;
    }
}

/// A call's progress `count`, its newest rebuild's `done` steps having been `last` and now
/// `done`: grown by each step done since, never fallen (a `done` that falls is another
/// rebuild's, begun since, whose steps are counted from it).
fn grown(count: u64, last: usize, done: usize) -> u64 {
    count.saturating_add(done.saturating_sub(last) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_counts_each_step_seen_done_and_never_falls() {
        // A check behind an edit: the edit's 9 of 9, then its own from 0 to 12.
        let mut count = 0;
        let mut last = 0;
        let mut counts = Vec::new();
        for done in [3, 9, 9, 0, 1, 1, 5, 12] {
            count = grown(count, last, done);
            last = done;
            counts.push(count);
        }
        assert_eq!(counts, [3, 9, 9, 9, 10, 10, 14, 21]);
    }

    #[test]
    fn progress_is_sent_only_as_steps_are_done_and_never_twice() {
        let mut count = Count::default();
        // Nothing done: nothing to send, however often it's seen.
        assert_eq!(count.seen(0), None);
        assert_eq!(count.seen(0), None);
        // Two steps done, sent; seen again with none more done, nothing more.
        assert_eq!(count.seen(2), Some(2));
        count.sent(2);
        assert_eq!(count.seen(2), None);
        assert_eq!(count.seen(2), None);
        // A third, not sent (the output busy): still to send, at the same count, until it is.
        assert_eq!(count.seen(3), Some(3));
        assert_eq!(count.seen(3), Some(3));
        count.sent(3);
        assert_eq!(count.seen(3), None);
        // Another rebuild, from none done: nothing until one of its steps is.
        assert_eq!(count.seen(0), None);
        assert_eq!(count.seen(1), Some(4));
        count.sent(4);
        // Two done between looks: grown by both.
        assert_eq!(count.seen(3), Some(6));
    }
}
