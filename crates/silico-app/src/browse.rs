//! Native file dialogs: Open's own (File > Open…, Command+O, and the start page's Open…,
//! which open the file picked), Save As's (File > Save As…, Command+Shift+S, and an untitled
//! part's Save, which save the document as the file picked), and the Browse… button beside a
//! path field (Export's path, and the typed Open and Save As the app falls back to), which
//! asks the platform's own dialog for a path and fills the field with it.
//!
//! A dialog is a future ([`Answer`]) that the frame polls, never waits on (invariant 4): it
//! answers through a waker that asks egui for a frame ([`Repaint`]), and the frame after
//! takes its answer. On macOS, rfd's future begins the panel as a sheet on the app's window
//! when it's made, on the UI thread (where the frame runs), and the sheet's completion
//! answers it; on Linux, rfd asks the XDG desktop portal on a thread of its own, which it
//! starts only when the future is first polled, so [`Browsing::new`] polls it at once,
//! and the portal's answer wakes it. Behind [`Browse`], so tests drive a fake that answers
//! when they say.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use eframe::egui;
use wgpu::rwh::{HasDisplayHandle, HasWindowHandle};

/// Which dialog: one picking a file that is there (Open), or one naming a file to write
/// (Save As, Export), which asks before replacing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Open,
    Save,
}

/// A filter of the dialog's: its name, and the extensions it shows (no dot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}

/// What a dialog is asked: which dialog, its title, its filters in order (the first chosen),
/// the folder it starts in (the platform's own choice if none), and the file name a save
/// dialog starts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub pick: Pick,
    pub title: String,
    pub filters: Vec<Filter>,
    pub folder: Option<PathBuf>,
    pub file_name: Option<String>,
}

/// A dialog's answer: the path picked, or none if it was cancelled.
pub type Answer = Pin<Box<dyn Future<Output = Option<PathBuf>>>>;

/// What opens a file dialog: the platform's ([`Native`]), or a test's.
pub trait Browse {
    /// The dialog `ask` says, opened, its answer to come; none if no dialog can open. Never
    /// waits for it.
    fn browse(&mut self, ask: &Ask) -> Option<Answer>;
}

/// A window a dialog can be the sheet of: one with a handle, as rfd takes its parent.
trait Window: HasWindowHandle + HasDisplayHandle {}

impl<T: HasWindowHandle + HasDisplayHandle + ?Sized> Window for T {}

/// The platform's own dialogs, through rfd's async API, each the app window's.
///
/// On macOS, rfd begins each panel as a sheet on the window it's given. Given none, it takes
/// `NSApp`'s main window, which is none while the app isn't the active one (a click sent by
/// another process, such as System Events', doesn't make it so), and then the first window
/// in `NSApp`'s list, which needn't be the one drawn (the app has several, the drawn one
/// last): a panel begun on a window not shown is never seen. And with no window at all it
/// runs the panel's own modal loop, which would hold the UI thread until the panel closed.
/// So the app's window, held here, is given as each panel's parent, and with no handle for
/// it no panel opens (the app asks for a typed path instead). A panel asked for while the
/// window isn't focused brings it, and so its sheet, to the front, the app made the active
/// one.
pub struct Native {
    /// The app's window, held for as long as its dialogs are: none if eframe gave none.
    window: Option<Arc<dyn Window>>,
    /// The app's egui context, to bring its window to the front.
    ctx: egui::Context,
}

impl Native {
    /// The platform's dialogs, each on the window eframe made for the app (`creation`'s).
    pub fn on(creation: &eframe::CreationContext<'_>) -> Self {
        let window = creation
            .winit_window()
            .map(|window| Arc::clone(window) as Arc<dyn Window>);
        if window.is_none() && cfg!(target_os = "macos") {
            tracing::warn!(
                "eframe gave no window: Open and Save As ask for a typed path, and Browse… opens nothing"
            );
        }
        Native {
            window,
            ctx: creation.egui_ctx.clone(),
        }
    }
}

impl Browse for Native {
    fn browse(&mut self, ask: &Ask) -> Option<Answer> {
        let mut dialog = rfd::AsyncFileDialog::new().set_title(&ask.title);
        // On macOS, a sheet on the app's window, or no panel. Linux's portal dialogs keep
        // their own place, as they have: a parent there is a handle exported for another
        // process, and rfd's export of one isn't needed.
        if cfg!(target_os = "macos") {
            let Some(window) = self
                .window
                .as_deref()
                .filter(|window| window.window_handle().is_ok())
            else {
                tracing::warn!("the window has no handle: no file dialog opens");
                return None;
            };
            dialog = dialog.set_parent(window);
        }
        for filter in &ask.filters {
            dialog = dialog.add_filter(filter.name, filter.extensions);
        }
        if let Some(folder) = &ask.folder {
            dialog = dialog.set_directory(folder);
        }
        if let Some(name) = &ask.file_name {
            dialog = dialog.set_file_name(name);
        }
        // Made here, on the UI thread: on macOS this begins the sheet, which returns at once.
        let picked = match ask.pick {
            Pick::Open => Box::pin(dialog.pick_file()) as Pin<Box<dyn Future<Output = _>>>,
            Pick::Save => Box::pin(dialog.save_file()),
        };
        // A window not focused (the app not the active one), brought to the front and made
        // key, its sheet with it, once this frame ends. A focused one is left alone: its
        // sheet is key already, and a focus asked for now could take that from it.
        let focused = self.ctx.input(|input| input.viewport().focused);
        if cfg!(target_os = "macos") && focused != Some(true) {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        Some(Box::pin(async move {
            picked.await.map(|handle| handle.path().to_path_buf())
        }))
    }
}

/// The waker a dialog's answer wakes: it asks egui for a frame, which takes the answer.
pub struct Repaint(pub egui::Context);

impl Wake for Repaint {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.request_repaint();
    }
}

/// A dialog open, for `T` (what its answer goes to), polled each frame.
pub struct Browsing<T> {
    pub what: T,
    answer: Answer,
    /// The answer its first poll gave, if it came then, kept for the frame that takes it.
    answered: Option<Option<PathBuf>>,
    waker: Waker,
}

impl<T> Browsing<T> {
    /// `answer`, for `what`, woken through `ctx`: polled once now, which starts rfd's portal
    /// thread on Linux, so its dialog shows this frame, not the next.
    pub fn new(what: T, answer: Answer, ctx: &egui::Context) -> Self {
        let mut browsing = Browsing {
            what,
            answer,
            answered: None,
            waker: Waker::from(Arc::new(Repaint(ctx.clone()))),
        };
        browsing.answered = browsing.poll_answer();
        if browsing.answered.is_some() {
            // Answered at once, so no waker will ask for the frame that takes it.
            ctx.request_repaint();
        }
        browsing
    }

    /// The answer, if it has come (`Some(None)`: cancelled); never waits for it.
    pub fn poll(&mut self) -> Option<Option<PathBuf>> {
        if let Some(answer) = self.answered.take() {
            return Some(answer);
        }
        self.poll_answer()
    }

    /// The future polled.
    fn poll_answer(&mut self) -> Option<Option<PathBuf>> {
        match self
            .answer
            .as_mut()
            .poll(&mut Context::from_waker(&self.waker))
        {
            Poll::Ready(answer) => Some(answer),
            Poll::Pending => None,
        }
    }
}

/// What a Browse… button that's off says when hovered.
pub const ALREADY_OPEN: &str = "A file dialog is already open";

/// The Browse… button beside a path field (`.btn-secondary`): off unless `on` (off while
/// a dialog is open).
pub(crate) fn button(on: bool) -> crate::modal::Button<'static> {
    crate::modal::Button::new("Browse…", crate::modal::Kind::Secondary).enabled(on)
}

/// The Browse… button as drawn: saying why it's off when hovered. Whether it was clicked.
pub(crate) fn clicked(response: egui::Response) -> bool {
    let response = if response.enabled() {
        response
    } else {
        response.on_disabled_hover_text(ALREADY_OPEN)
    };
    response.clicked()
}

/// A Silico document's filter.
pub const DOCUMENTS: Filter = Filter {
    name: "Silico document",
    extensions: &["silico"],
};

/// Export's filters: the format chosen's first.
pub fn export_filters(step: bool) -> Vec<Filter> {
    let three_mf = Filter {
        name: "3MF",
        extensions: &["3mf"],
    };
    let step_filter = Filter {
        name: "STEP",
        extensions: &["step", "stp"],
    };
    if step {
        vec![step_filter, three_mf]
    } else {
        vec![three_mf, step_filter]
    }
}

/// The folder of the document at `document`, if its path names one (a bare name doesn't),
/// made absolute against the working directory: the XDG portal, another process, can't read
/// a relative one. As given if it can't be (the working directory unknown).
pub fn folder_of(document: &str) -> Option<PathBuf> {
    let folder = Path::new(document)
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())?;
    Some(std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf()))
}

/// The file name of `path`, if it has one.
pub fn file_name_of(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_documents_folder_is_its_paths_and_a_bare_name_has_none() {
        assert_eq!(
            folder_of("/work/bracket.silico"),
            Some(PathBuf::from("/work"))
        );
        assert_eq!(folder_of("bracket.silico"), None);
        assert_eq!(folder_of(""), None);
    }

    #[test]
    fn a_relative_documents_folder_is_made_absolute_against_the_working_directory() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(folder_of("sub/bracket.silico"), Some(here.join("sub")));
        assert_eq!(
            folder_of("sub/deeper/bracket.silico"),
            Some(here.join("sub").join("deeper"))
        );
        assert!(folder_of("sub/bracket.silico").unwrap().is_absolute());
    }

    #[test]
    fn exports_filters_put_the_format_chosen_first() {
        let names = |step| -> Vec<&str> {
            export_filters(step)
                .iter()
                .map(|filter| filter.name)
                .collect()
        };
        assert_eq!(names(false), ["3MF", "STEP"]);
        assert_eq!(names(true), ["STEP", "3MF"]);
        assert_eq!(export_filters(true)[0].extensions, ["step", "stp"]);
    }

    /// A context drawn until it asks for no frame, and the delay of each frame asked of it
    /// from then on.
    fn settled() -> (
        egui::Context,
        Arc<std::sync::Mutex<Vec<std::time::Duration>>>,
    ) {
        let ctx = egui::Context::default();
        for _ in 0..10 {
            // Its textures dropped unpainted, as no painter takes them here.
            ctx.run_ui(egui::RawInput::default(), |_| {})
                .textures_delta
                .clear();
            if !ctx.has_requested_repaint() {
                let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
                let recorded = Arc::clone(&asked);
                ctx.set_request_repaint_callback(move |info| {
                    recorded.lock().unwrap().push(info.delay);
                });
                return (ctx, asked);
            }
        }
        panic!("the context never stopped asking for frames");
    }

    #[test]
    fn a_dialogs_answer_woken_by_reference_asks_for_a_frame_at_once() {
        let (ctx, asked) = settled();
        let waker = Waker::from(Arc::new(Repaint(ctx.clone())));
        waker.wake_by_ref();
        assert!(ctx.has_requested_repaint());
        assert_eq!(*asked.lock().unwrap(), [std::time::Duration::ZERO]);
    }

    #[test]
    fn a_dialogs_answer_woken_by_value_asks_for_a_frame_at_once() {
        let (ctx, asked) = settled();
        let waker = Waker::from(Arc::new(Repaint(ctx.clone())));
        waker.wake();
        assert!(ctx.has_requested_repaint());
        assert_eq!(*asked.lock().unwrap(), [std::time::Duration::ZERO]);
    }
}
