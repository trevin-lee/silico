//! What a session holds, behind its lock ([`Holding`]): read by anyone, at once, and
//! changed in only two ways, each named: by the queued command running, once its stop is
//! settled ([`Reading::changing`]), so a command cancelled first never changes it and one
//! that changed it is never said to be stopped (decision 34); or by the session itself
//! ([`Holding::session_changing`]), whatever its command comes to.

use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};

use super::Problems;
use super::cancel::Ticket;
use super::transaction::Held;
use crate::worker::lock;

/// A session's [`Held`], under its lock: nothing changes it but through [`Changing`].
pub(super) struct Holding {
    held: Mutex<Held>,
}

impl Holding {
    pub(super) fn new(held: Held) -> Self {
        Holding {
            held: Mutex::new(held),
        }
    }

    /// What's held, to read, the lock held until it's dropped.
    pub(super) fn read(&self) -> Reading<'_> {
        Reading(lock(&self.held))
    }

    /// What's held, to change as the session's own work, not a command's, so whatever the
    /// command comes to, cancelled or not, the change stands: the reload of a file changed
    /// outside the session, before the command (decision 2), whose response then says
    /// `reloaded`; and the mark of a file the session's own save left part written, set as
    /// the save fails and cleared by a reload that finds the old text back.
    pub(super) fn session_changing(&self) -> Changing<'_> {
        Changing(lock(&self.held))
    }
}

/// What a session holds, read under its lock.
pub(super) struct Reading<'a>(MutexGuard<'a, Held>);

impl<'a> Reading<'a> {
    /// What's held, to change as the work of the command `stop` stops, under the same lock:
    /// its stop settled first ([`Ticket::settle`]), so from here a cancel comes too late
    /// and the command answers with its real result. Refused, `cancelled`, nothing changed,
    /// if it was cancelled before. Settling is once: a command that has settled changes what's
    /// held again without being refused.
    pub(super) fn changing(self, stop: &Ticket) -> Result<Changing<'a>, Problems> {
        stop.settle()?;
        Ok(Changing(self.0))
    }
}

impl Deref for Reading<'_> {
    type Target = Held;

    fn deref(&self) -> &Held {
        &self.0
    }
}

/// What a session holds, to change, under its lock: had through
/// [`Reading::changing`] or [`Holding::session_changing`] alone.
pub(super) struct Changing<'a>(MutexGuard<'a, Held>);

impl Deref for Changing<'_> {
    type Target = Held;

    fn deref(&self) -> &Held {
        &self.0
    }
}

impl DerefMut for Changing<'_> {
    fn deref_mut(&mut self) -> &mut Held {
        &mut self.0
    }
}
