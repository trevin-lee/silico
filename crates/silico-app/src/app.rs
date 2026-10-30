//! The app (docs/design/m5.0-app.md, sections 2 to 4): one `Sessions`, a tab for each open
//! document, and the frame loop. Each frame polls only what answers at once (`status`,
//! `published`, and each command's `try_take`), starts the commands the person asks for,
//! and draws; it never waits on a command (invariant 4). The core's waker asks for the
//! frame after anything changes, and with nothing pending one frame a second compares the
//! files on disk.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, RichText, Ui};
use silico_core::command::{Opened, RebuildState, Running, RunningCommand, SessionId, Sessions};
use silico_core::disk::{DISK_CHECK_INTERVAL, Disk};
use silico_core::kernel::Clients;

use crate::panes::{self, Pane, Panes};
use crate::tab::{DiskChoice, Tab, said};
use crate::theme;

/// How long past [`DISK_CHECK_INTERVAL`] the idle frame comes: a comparison is due once the
/// last one ended a whole interval ago, so a frame exactly an interval after the one that
/// started it would come a little early, and the next comparison a whole interval late
/// (#267's idle cadence). This much covers a comparison's own read on any likely file.
pub const IDLE_MARGIN: Duration = Duration::from_millis(100);

/// The delay of the frame the app asks for when nothing it waits on has answered: one each
/// [`DISK_CHECK_INTERVAL`], whose `status` starts the next comparison (section 3, *Idle*).
pub const IDLE_FRAME: Duration = DISK_CHECK_INTERVAL.saturating_add(IDLE_MARGIN);

/// What the app is asking the person.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asking {
    /// A path to open, or (`create`) to make a new document at (open item 30).
    Path { create: bool },
    /// Whether to save a changed document before closing it (docs/ux/solidworks.md, §6),
    /// by its session, whatever opens or closes meanwhile.
    Close(SessionId),
    /// Whether to save the changed documents before quitting.
    Quit,
}

/// The app's state: everything the window draws, and what it's waiting for.
pub struct App<P: Clients + 'static> {
    sessions: Arc<Sessions<P>>,
    tabs: Vec<Tab>,
    /// The tab shown, by its session.
    active: Option<SessionId>,
    opening: Vec<(String, Running<Opened>)>,
    asking: Option<Asking>,
    /// The path field's text, while a path is asked for.
    path: String,
    tree: egui_tiles::Tree<Pane>,
    /// The property panel's name field, while it's typed in.
    name: Option<panes::NameField>,
    /// The last refusal not of a tab's (an open's), as said to the person.
    message: Option<String>,
    /// Quitting: once every tab has closed, the window does.
    quitting: bool,
    /// Frames drawn: what the tests watch climb while commands are held.
    frames: u64,
    /// Times the core's waker asked for a frame.
    wakes: Arc<AtomicU64>,
    waker_set: bool,
}

impl<P: Clients + 'static> App<P> {
    /// An app on `sessions`, with no document open.
    pub fn new(sessions: Arc<Sessions<P>>) -> Self {
        App {
            sessions,
            tabs: Vec::new(),
            active: None,
            opening: Vec::new(),
            asking: None,
            path: String::new(),
            tree: panes::layout(),
            name: None,
            message: None,
            quitting: false,
            frames: 0,
            wakes: Arc::new(AtomicU64::new(0)),
            waker_set: false,
        }
    }

    /// Frames drawn so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Times the core's waker asked for a frame.
    pub fn wakes(&self) -> u64 {
        self.wakes.load(Ordering::SeqCst)
    }

    /// The sessions the app runs its commands on.
    pub fn sessions(&self) -> &Arc<Sessions<P>> {
        &self.sessions
    }

    /// Each open document's tab, in the order they opened.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Each open document's tab, to act on as the window's controls do (a client driving
    /// the app without its window, a test).
    pub fn tabs_mut(&mut self) -> &mut [Tab] {
        &mut self.tabs
    }

    /// Opens the document at `path`, as the Open dialog does: a tab once it has opened.
    pub fn open(&mut self, path: &str, create: bool) {
        self.opening
            .push((path.to_owned(), self.sessions.open(path, create)));
    }

    /// One frame: every tab polled, the person's input taken, and the window drawn.
    pub fn frame(&mut self, ui: &mut Ui) {
        self.frames += 1;
        if !self.waker_set {
            // The core's waker asks egui for a frame, off the UI thread, never drawing.
            let ctx = ui.ctx().clone();
            let wakes = Arc::clone(&self.wakes);
            self.sessions.on_change(move || {
                wakes.fetch_add(1, Ordering::SeqCst);
                ctx.request_repaint();
            });
            self.waker_set = true;
        }
        self.poll();
        let mut started = self.keys(ui);
        self.app_bar(ui, &mut started);
        self.toolbar(ui);
        self.status_bar(ui, &mut started);
        self.disk_banner(ui, &mut started);
        egui::CentralPanel::default().show(ui, |ui| {
            let active = self.active_place();
            let App {
                tree,
                tabs,
                sessions,
                name,
                ..
            } = self;
            let mut panes = Panes {
                tab: active.and_then(|at| tabs.get_mut(at)),
                sessions,
                name,
                started: &mut started,
            };
            tree.ui(&mut panes, ui);
        });
        self.ask(ui, &mut started);
        if self.quitting && self.tabs.is_empty() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // The window never closes while a tab holds edits: the person is asked, unless a
        // quit is already closing them, which closes the window once every tab has gone.
        if ui.ctx().input(|input| input.viewport().close_requested())
            && self.tabs.iter().any(Tab::unsaved)
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if !self.quitting {
                self.asking = Some(Asking::Quit);
            }
        }
        if started {
            // Polled next frame, even if it answered at once.
            ui.ctx().request_repaint();
        }
        // With nothing pending, a frame each interval, whose `status` compares the files on
        // disk; with something pending, the same, the waker telling of each answer.
        ui.ctx().request_repaint_after(IDLE_FRAME);
    }

    /// The opens under way and every tab polled; tabs whose sessions closed dropped.
    fn poll(&mut self) {
        for (path, running) in std::mem::take(&mut self.opening) {
            match running.try_take() {
                Ok(response) => match response.result {
                    Ok(opened) => {
                        self.tabs
                            .push(Tab::opened(opened.session, path, &opened.problems));
                        self.active = Some(opened.session);
                        self.message = None;
                    }
                    Err(problems) => self.message = Some(said(&problems)),
                },
                Err(running) => self.opening.push((path, running)),
            }
        }
        for tab in &mut self.tabs {
            tab.poll(&self.sessions);
            if tab.take_close_refused() && self.quitting {
                // A tab's save or revert was refused: it stays, holding its edits, and the
                // quit stops, said.
                self.quitting = false;
                self.message = Some(format!("Quit stopped: {} wasn't closed", tab.name()));
            }
        }
        self.tabs.retain(|tab| !tab.closed);
        if self.active_place().is_none() {
            self.active = self.tabs.last().map(|tab| tab.id);
        }
    }

    /// The place among the tabs of the one shown, if one is.
    fn active_place(&self) -> Option<usize> {
        let active = self.active?;
        self.tabs.iter().position(|tab| tab.id == active)
    }

    /// Ctrl+S saves, Ctrl+W closes, Ctrl+Z and Ctrl+Y undo and redo (docs/ux/solidworks.md,
    /// §6), and Esc stops what's running (§5, *Rebuild*): whether a command was started.
    fn keys(&mut self, ui: &mut Ui) -> bool {
        if self.asking.is_some() {
            return false;
        }
        let pressed = |key| {
            ui.input_mut(|input| {
                input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, key))
            })
        };
        let (save, close, undo, redo) = (
            pressed(Key::S),
            pressed(Key::W),
            pressed(Key::Z),
            pressed(Key::Y),
        );
        let stop = ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
            return false;
        };
        let sessions = &self.sessions;
        if save && tab.unsaved() {
            tab.save(sessions, false);
        }
        if undo && tab.transaction {
            tab.step(sessions, true);
        }
        if redo && tab.transaction {
            tab.step(sessions, false);
        }
        if stop {
            tab.stop(sessions);
        }
        if close && let Some(id) = self.active {
            self.close(id);
        }
        save || undo || redo || stop || close
    }

    /// Close the tab of session `id`: asking first if it holds edits (decision 1).
    fn close(&mut self, id: SessionId) {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        if tab.unsaved() {
            self.asking = Some(Asking::Close(id));
        } else {
            tab.close(&self.sessions);
        }
    }

    /// The application bar (section 8): the File menu, Save, Undo and Redo, and the open
    /// documents' tabs, an asterisk on a changed one's (docs/ux/solidworks.md, §6).
    fn app_bar(&mut self, ui: &mut Ui, started: &mut bool) {
        egui::Panel::top("app_bar")
            .exact_size(theme::TOP_BAR_HEIGHT)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let open = !self.tabs.is_empty();
                    let transaction = self
                        .active_place()
                        .and_then(|at| self.tabs.get(at))
                        .is_some_and(|tab| tab.transaction);
                    // Revert is off while the tab closes: its close drops the edits already.
                    let revertible = transaction
                        && self
                            .active_place()
                            .and_then(|at| self.tabs.get(at))
                            .is_some_and(|tab| !tab.closing());
                    ui.menu_button("File", |ui| {
                        if ui.button("New…").clicked() {
                            self.asking = Some(Asking::Path { create: true });
                            ui.close();
                        }
                        if ui.button("Open…").clicked() {
                            self.asking = Some(Asking::Path { create: false });
                            ui.close();
                        }
                        if ui
                            .add_enabled(revertible, egui::Button::new("Revert"))
                            .clicked()
                        {
                            if let Some(tab) =
                                self.active_place().and_then(|at| self.tabs.get_mut(at))
                            {
                                tab.revert(&self.sessions, false);
                                *started = true;
                            }
                            ui.close();
                        }
                        if ui.add_enabled(open, egui::Button::new("Close")).clicked() {
                            if let Some(id) = self.active {
                                self.close(id);
                            }
                            *started = true;
                            ui.close();
                        }
                    });
                    let tab = self.active_place().and_then(|at| self.tabs.get_mut(at));
                    let unsaved = tab.as_ref().is_some_and(|tab| tab.unsaved());
                    if ui.add_enabled(unsaved, egui::Button::new("Save")).clicked()
                        && let Some(tab) = tab
                    {
                        tab.save(&self.sessions, false);
                        *started = true;
                    }
                    for (undo, word) in [(true, "Undo"), (false, "Redo")] {
                        if ui
                            .add_enabled(transaction, egui::Button::new(word))
                            .clicked()
                            && let Some(tab) =
                                self.active_place().and_then(|at| self.tabs.get_mut(at))
                        {
                            tab.step(&self.sessions, undo);
                            *started = true;
                        }
                    }
                    ui.separator();
                    let mut close = None;
                    for tab in &self.tabs {
                        let title = if tab.unsaved() {
                            format!("{} *", tab.name())
                        } else {
                            tab.name().to_owned()
                        };
                        if ui
                            .selectable_label(Some(tab.id) == self.active, &title)
                            .clicked()
                        {
                            self.active = Some(tab.id);
                        }
                        let x = ui.add(egui::Button::new("×").frame(false));
                        let closing = format!("Close {}", tab.name());
                        x.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &closing)
                        });
                        if x.clicked() {
                            close = Some(tab.id);
                        }
                    }
                    if let Some(id) = close {
                        self.close(id);
                        *started = true;
                    }
                });
            });
    }

    /// The toolbar (section 8): the Design workspace's SOLID tab, its panels named; their
    /// tools come with the feature dialogs (M5.8).
    fn toolbar(&mut self, ui: &mut Ui) {
        egui::Panel::top("toolbar")
            .exact_size(theme::TITLE_BAR_HEIGHT * 2.0)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("DESIGN").strong());
                    ui.separator();
                    ui.label(RichText::new("SOLID").strong());
                    for panel in ["CREATE", "MODIFY", "INSPECT", "SELECT"] {
                        ui.separator();
                        ui.label(RichText::new(panel).small());
                    }
                });
            });
    }

    /// The status bar: the rebuild (`status.rebuild`) or the session's own reload, the stop
    /// button (`cancel`, off while only a reload runs, section 3), and the last refusal.
    fn status_bar(&mut self, ui: &mut Ui, started: &mut bool) {
        egui::Panel::bottom("status_bar")
            .exact_size(theme::TITLE_BAR_HEIGHT)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let message = self.message.clone();
                    let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
                        if let Some(message) = message {
                            ui.colored_label(ui.visuals().error_fg_color, message);
                        }
                        return;
                    };
                    let status = tab.status.clone();
                    let running = status.as_ref().and_then(|status| status.running);
                    let rebuild = status.as_ref().map(|status| &status.rebuild);
                    let said = match (running, rebuild) {
                        (Some(RunningCommand::Reload), _) => {
                            format!("Reloading {} from disk", tab.name())
                        }
                        (_, Some(rebuild)) => match (rebuild.state, &rebuild.progress) {
                            (RebuildState::Running, Some(progress)) => {
                                format!("Rebuilding: {} of {} steps", progress.done, progress.steps)
                            }
                            (RebuildState::Running, None) => "Rebuilding".to_owned(),
                            (RebuildState::Done, _) => "Rebuilt".to_owned(),
                            (RebuildState::Faulted, _) => "The kernel faulted".to_owned(),
                            (RebuildState::Stopped, _) => "Rebuild stopped".to_owned(),
                            (RebuildState::Idle, _) => String::new(),
                        },
                        (_, None) => String::new(),
                    };
                    ui.label(said);
                    let stoppable = tab.stoppable();
                    if ui
                        .add_enabled(stoppable, egui::Button::new("Stop"))
                        .clicked()
                    {
                        tab.stop(&self.sessions);
                        *started = true;
                    }
                    for message in [message, tab.message.clone()].into_iter().flatten() {
                        ui.colored_label(ui.visuals().error_fg_color, message);
                    }
                });
            });
    }

    /// A file changed on disk while edits are held (section 4): Reload (`rollback`, after
    /// which the session reloads it) or keep the edits (a commit is then a conflict).
    fn disk_banner(&mut self, ui: &mut Ui, started: &mut bool) {
        let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
            return;
        };
        let DiskChoice::Offered(disk) = tab.disk_choice else {
            return;
        };
        egui::Panel::top("disk_banner").show(ui, |ui| {
            ui.horizontal(|ui| {
                let what = match disk {
                    Disk::Gone => "is gone from disk",
                    _ => "changed on disk",
                };
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("{} {what} while you have unsaved edits.", tab.name()),
                );
                // Off while the tab closes: its close drops the edits already.
                if ui
                    .add_enabled(!tab.closing(), egui::Button::new("Reload"))
                    .clicked()
                {
                    tab.revert(&self.sessions, false);
                    *started = true;
                }
                if ui.button("Keep my edits").clicked() {
                    tab.disk_choice = DiskChoice::Kept;
                }
            });
        });
    }

    /// What the app is asking, as a modal: a path, or whether to save.
    fn ask(&mut self, ui: &mut Ui, started: &mut bool) {
        let Some(asking) = self.asking.clone() else {
            return;
        };
        let mut done = false;
        egui::Modal::new(egui::Id::new("asking")).show(ui.ctx(), |ui| match asking {
            Asking::Path { create } => {
                ui.heading(if create {
                    "New document"
                } else {
                    "Open document"
                });
                ui.horizontal(|ui| {
                    let label = ui.label("Path");
                    ui.add(egui::TextEdit::singleline(&mut self.path))
                        .labelled_by(label.id);
                });
                ui.horizontal(|ui| {
                    let word = if create { "Create" } else { "Open" };
                    if ui.button(word).clicked() && !self.path.is_empty() {
                        let path = std::mem::take(&mut self.path);
                        self.open(&path, create);
                        *started = true;
                        done = true;
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
            }
            Asking::Close(id) => {
                let name = self
                    .tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .map(|tab| tab.name().to_owned());
                ui.label(format!(
                    "Save changes to {} before closing?",
                    name.as_deref().unwrap_or("the document")
                ));
                ui.horizontal(|ui| {
                    let sessions = &self.sessions;
                    if ui.button("Save").clicked() {
                        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                            tab.save(sessions, true);
                        }
                        *started = true;
                        done = true;
                    }
                    if ui.button("Don't save").clicked() {
                        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                            tab.revert(sessions, true);
                        }
                        *started = true;
                        done = true;
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
            }
            Asking::Quit => {
                let names: Vec<&str> = self
                    .tabs
                    .iter()
                    .filter(|tab| tab.unsaved())
                    .map(Tab::name)
                    .collect();
                ui.label(format!(
                    "Save changes before quitting? Unsaved: {}",
                    names.join(", ")
                ));
                ui.horizontal(|ui| {
                    let sessions = &self.sessions;
                    let save = ui.button("Save all").clicked();
                    let drop = ui.button("Don't save").clicked();
                    if save || drop {
                        for tab in &mut self.tabs {
                            match (tab.unsaved(), save) {
                                (true, true) => tab.save(sessions, true),
                                (true, false) => tab.revert(sessions, true),
                                (false, _) => tab.close(sessions),
                            }
                        }
                        self.quitting = true;
                        *started = true;
                        done = true;
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
            }
        });
        if done {
            self.asking = None;
        }
    }
}

impl<P: Clients + 'static> eframe::App for App<P> {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
}
