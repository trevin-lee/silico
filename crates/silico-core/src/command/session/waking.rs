//! The client's waker (docs/design/m5.0-app.md, section 3, *Waking without input*): what a
//! client that polls (the app, once a frame) is told when something it would draw changed,
//! so it draws a frame then and not at the display's rate.

use std::sync::Mutex;

use crate::notify::Notify;
use crate::worker::lock;

/// The waker [`Sessions::on_change`](super::Sessions::on_change) was given, if one was,
/// shared by every session. Its lock is taken alone, and let go before the waker is called.
#[derive(Default)]
pub(super) struct Waker(Mutex<Option<Notify>>);

impl Waker {
    /// From now on, `notify` is told of each change (in place of any given before).
    pub(super) fn set(&self, notify: Notify) {
        *lock(&self.0) = Some(notify);
    }

    /// The client told something changed: called once what changed can be read (a
    /// command's answer sent, a model published, a finding kept), with no lock of the
    /// core's held, so the waker may ask anything of the sessions.
    pub(super) fn wake(&self) {
        let notify = lock(&self.0).clone();
        if let Some(notify) = notify {
            notify();
        }
    }
}
