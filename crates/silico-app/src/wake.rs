//! Who to ask for a frame when something changes off the UI thread: the window's context,
//! once its first frame has given it ([`Wake::set`]). Shared by the threads that keep the
//! recent documents (`crate::recent`), warm the workers (`crate::start::Warm`), and make
//! Select Other's boxes (`crate::boxes`).

use std::sync::{Arc, Mutex, PoisonError};

use eframe::egui;

/// The window's context, once given; something told before then is seen by the first frame,
/// which gives it.
#[derive(Clone, Default)]
pub(crate) struct Wake(Arc<Mutex<Option<egui::Context>>>);

impl Wake {
    /// `ctx` asked from now on.
    pub(crate) fn set(&self, ctx: &egui::Context) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(ctx.clone());
    }

    /// A frame asked for, if the window has given its context.
    pub(crate) fn wake(&self) {
        if let Some(ctx) = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            ctx.request_repaint();
        }
    }
}
