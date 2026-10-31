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

use crate::dialog::{Action, Kind};
use crate::export::{self, ExportForm, Shown};
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

/// Opens, each by the path it was asked with, in the order asked.
type ByPath<T> = Vec<(String, T)>;

/// The opens under way that have answered, taken in the order they were asked, each waiting
/// for those before it, and those still to take: sessions open on threads of their own, so
/// the order they answer in isn't the person's, and a tab's place is the order it was opened.
fn in_order<R, T>(
    opening: ByPath<R>,
    mut try_take: impl FnMut(R) -> Result<T, R>,
) -> (ByPath<T>, ByPath<R>) {
    let mut answered = Vec::new();
    let mut waiting = Vec::new();
    for (path, running) in opening {
        if !waiting.is_empty() {
            waiting.push((path, running));
            continue;
        }
        match try_take(running) {
            Ok(answer) => answered.push((path, answer)),
            Err(running) => waiting.push((path, running)),
        }
    }
    (answered, waiting)
}

/// Whether Enter, pressed this frame, is an open dialog's OK, taking it if so: while typing
/// in a field or with nothing focused (a focused button's or checkbox's Enter is its own
/// click), and only a fresh press, never the system's repeat of one held down (an Enter held
/// on the menu item that opened the dialog would otherwise go on to be its OK).
fn enter_is_ok(ui: &mut Ui) -> bool {
    // Only a focus inside the dialog counts: one left on a widget no longer drawn (the
    // menu item that opened it), or behind a modal (the menu button it went back to), takes
    // no Enter of its own.
    let focused = ui.memory(|memory| memory.focused()).filter(|id| {
        ui.ctx().read_response(*id).is_some_and(|response| {
            ui.memory(|memory| memory.allows_interaction(response.layer_id))
        })
    });
    if !focused.is_none_or(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some()) {
        return false;
    }
    ui.input_mut(|input| {
        let mut pressed = false;
        input.events.retain(|event| {
            let fresh = matches!(
                event,
                egui::Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if modifiers.matches_logically(Modifiers::NONE)
            );
            pressed |= fresh;
            !fresh
        });
        pressed
    })
}

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
    /// A path to save session's document to (Save As), by its session.
    SaveAs(SessionId),
    /// Whether to replace the file a Save As of session's document found at its path.
    Replace(SessionId, String),
    /// What to export of session's document, and where (M5.12): the form is `export`.
    Export(SessionId),
}

/// The app's state: everything the window draws, and what it's waiting for.
pub struct App<P: Clients + 'static> {
    sessions: Arc<Sessions<P>>,
    tabs: Vec<Tab>,
    /// The tab shown, by its session.
    active: Option<SessionId>,
    opening: Vec<(String, Running<Opened>)>,
    asking: Option<Asking>,
    /// The Open (and New) dialog's path field, kept from the last time it was open; and
    /// Save As's, empty each time it opens.
    open_path: String,
    save_as_path: String,
    /// The export dialog's form, while it's open.
    export: Option<ExportForm>,
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
    /// Enter or Esc pressed this frame with a modal already open at its start: the modal's
    /// OK or Cancel, taken before anything else could read it.
    modal_key: Option<Key>,
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
            open_path: String::new(),
            save_as_path: String::new(),
            export: None,
            tree: panes::layout(),
            name: None,
            message: None,
            quitting: false,
            frames: 0,
            wakes: Arc::new(AtomicU64::new(0)),
            waker_set: false,
            modal_key: None,
        }
    }

    /// The last refusal said that isn't a tab's (an open's).
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
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
        let (mut started, dialog_key) = self.keys(ui);
        self.app_bar(ui, &mut started);
        self.toolbar(ui, &mut started);
        self.status_bar(ui, &mut started);
        self.disk_banner(ui, &mut started);
        self.dialog_panel(ui, &mut started, dialog_key);
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
                // An export dialog it replaces is cancelled, as its Cancel would.
                if let Some(Asking::Export(id)) = self.asking
                    && let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
                {
                    tab.stop_export();
                }
                self.export = None;
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
        let (answered, opening) = in_order(std::mem::take(&mut self.opening), Running::try_take);
        self.opening = opening;
        for (path, response) in answered {
            match response.result {
                Ok(opened) => {
                    self.tabs
                        .push(Tab::opened(opened.session, path, &opened.problems));
                    self.active = Some(opened.session);
                    self.message = None;
                }
                Err(problems) => self.message = Some(said(&problems)),
            }
        }
        for tab in &mut self.tabs {
            tab.poll(&self.sessions);
            // Asked once nothing else is: kept on the tab meanwhile.
            if self.asking.is_none()
                && let Some(path) = tab.take_replace_asked()
            {
                self.asking = Some(Asking::Replace(tab.id, path));
            }
            // An export written: its dialog closes.
            if tab.take_exported() && self.asking == Some(Asking::Export(tab.id)) {
                self.asking = None;
                self.export = None;
            }
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

    /// Whether Save As is offered: a tab shown, not closing (its edits go with it), and no
    /// dialog open on it (its OK or Cancel ends its edits first, as for Save).
    fn save_as_offered(&self) -> bool {
        self.active_place()
            .and_then(|at| self.tabs.get(at))
            .is_some_and(|tab| !tab.closing() && !tab.in_dialog())
    }

    /// The place among the tabs of the one shown, if one is.
    fn active_place(&self) -> Option<usize> {
        let active = self.active?;
        self.tabs.iter().position(|tab| tab.id == active)
    }

    /// Ctrl+S saves and Ctrl+Shift+S asks where to save as (neither while a dialog is
    /// open), Ctrl+W closes, Ctrl+Z and Ctrl+Y undo and redo (docs/ux/solidworks.md, §6),
    /// Esc stops what's running and Ctrl+B rebuilds what was stopped (§5, *Rebuild*). With a dialog open, Enter and Esc are
    /// the dialog's, taken here and given to it once its inputs have taken this frame's
    /// typing ([`dialog_panel`](Self::dialog_panel)). Whether a command was started, and
    /// the dialog's key.
    fn keys(&mut self, ui: &mut Ui) -> (bool, Option<Key>) {
        self.modal_key = None;
        if self.asking.is_some() {
            // A modal open since the last frame: Esc is its Cancel, and Enter its OK unless
            // a button or a checkbox of its has focus, whose own click Enter then is. Taken
            // only now, so an Enter that opened the modal (on a focused menu item) isn't
            // also its OK.
            let escape = ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
            let enter = !escape && enter_is_ok(ui);
            self.modal_key = match (escape, enter) {
                (true, _) => Some(Key::Escape),
                (false, true) => Some(Key::Enter),
                (false, false) => None,
            };
            return (escape || enter, None);
        }
        // Before Ctrl+S, which a Ctrl+Shift+S also is, its Shift aside.
        let save_as = ui.input_mut(|input| {
            input.consume_shortcut(&KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::S,
            ))
        });
        if save_as && self.save_as_offered() {
            self.save_as_path.clear();
            self.asking = self.active.map(Asking::SaveAs);
            return (false, None);
        }
        let in_dialog = self
            .active_place()
            .and_then(|at| self.tabs.get(at))
            .is_some_and(Tab::in_dialog);
        // A feature dialog's OK, as a modal's is (a focused button's Enter its own click).
        let enter = in_dialog && enter_is_ok(ui);
        let pressed = |key| {
            ui.input_mut(|input| {
                input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, key))
            })
        };
        let (save, close, undo, redo, rebuild) = (
            pressed(Key::S),
            pressed(Key::W),
            pressed(Key::Z),
            pressed(Key::Y),
            pressed(Key::B),
        );
        let stop = ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
            return (false, None);
        };
        let sessions = &self.sessions;
        if save && tab.unsaved() && !tab.in_dialog() {
            tab.save(sessions, false);
        }
        if undo && tab.undoable() {
            tab.step(sessions, true);
        }
        if redo && tab.redoable() {
            tab.step(sessions, false);
        }
        if rebuild {
            tab.rebuild(sessions);
        }
        let dialog_key = match (enter, stop && in_dialog) {
            (true, _) => Some(Key::Enter),
            (false, true) => Some(Key::Escape),
            (false, false) => None,
        };
        // Esc, with no dialog open: the selection cleared first, then a rebuild running
        // stopped, as the stop does (section 3's order); with neither, nothing.
        if stop && !in_dialog {
            tab.escape(sessions);
        }
        if close && let Some(id) = self.active {
            self.close(id);
        }
        (
            save || undo || redo || stop || close || enter || rebuild,
            dialog_key,
        )
    }

    /// The open dialog, docked at the right (open item 27): drawn, what's changed in it sent,
    /// and its OK or Cancel taken, or `key`: Enter is OK, if OK is on; Esc goes to the first
    /// of these there is (section 3), a dialog's preview rebuilding, which it stops, the
    /// dialog left open, or the dialog, which it cancels (one still being read too).
    fn dialog_panel(&mut self, ui: &mut Ui, started: &mut bool, key: Option<Key>) {
        let sessions = &self.sessions;
        let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
            return;
        };
        if tab.dialog.is_none() {
            if key == Some(Key::Escape) && tab.in_dialog() {
                tab.close_dialog(sessions, false);
            }
            return;
        }
        let refused: Vec<&'static str> = tab
            .dialog
            .iter()
            .flat_map(|dialog| dialog.inputs.iter().map(|input| input.part.field()))
            .filter(|field| tab.field_refused(field))
            .collect();
        let said = tab.dialog_said();
        let Some(mut dialog) = tab.dialog.take() else {
            return;
        };
        let enabled = !tab.closing();
        let mut action = Action::None;
        egui::Panel::right("dialog")
            .resizable(false)
            .default_size(280.0)
            .show(ui, |ui| {
                let title = dialog.kind.title();
                ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                    node.set_role(egui::accesskit::Role::Dialog);
                    node.set_label(title);
                });
                let rows = &tab.browser.sketches;
                let named = |id: &silico_core::document::FeatureId| {
                    rows.iter()
                        .find(|row| &row.id == id)
                        .map(|row| row.name.clone())
                };
                let refused = |field: &str| refused.contains(&field);
                action = dialog.ui(ui, &said, &named, &refused, enabled);
            });
        tab.dialog = Some(dialog);
        *started |= tab.dialog_changed(sessions);
        match key {
            Some(Key::Enter) if tab.can_ok() => action = Action::Ok,
            Some(Key::Escape) if tab.preview_rebuilding() => tab.stop_quietly(sessions),
            Some(Key::Escape) => action = Action::Cancel,
            _ => {}
        }
        match action {
            Action::None => {}
            Action::Ok => tab.close_dialog(sessions, true),
            Action::Cancel => tab.close_dialog(sessions, false),
        }
        *started |= action != Action::None;
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
                            .add_enabled(self.save_as_offered(), egui::Button::new("Save As…"))
                            .clicked()
                        {
                            if let Some(id) = self.active {
                                self.save_as_path.clear();
                                self.asking = Some(Asking::SaveAs(id));
                            }
                            ui.close();
                        }
                        // Off while the tab closes, as Save As is.
                        if ui
                            .add_enabled(self.save_as_offered(), egui::Button::new("Export…"))
                            .clicked()
                        {
                            if let Some(tab) =
                                self.active_place().and_then(|at| self.tabs.get_mut(at))
                            {
                                tab.export_opened();
                                self.export = Some(ExportForm::new(tab.id, &tab.path));
                                self.asking = Some(Asking::Export(tab.id));
                            }
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
                    // Off while a dialog is open: its OK or Cancel ends its edits first.
                    let unsaved = tab
                        .as_ref()
                        .is_some_and(|tab| tab.unsaved() && !tab.in_dialog());
                    if ui.add_enabled(unsaved, egui::Button::new("Save")).clicked()
                        && let Some(tab) = tab
                    {
                        tab.save(&self.sessions, false);
                        *started = true;
                    }
                    let (undoable, redoable) = self
                        .active_place()
                        .and_then(|at| self.tabs.get(at))
                        .map_or((false, false), |tab| (tab.undoable(), tab.redoable()));
                    for (undo, word, enabled) in
                        [(true, "Undo", undoable), (false, "Redo", redoable)]
                    {
                        if ui.add_enabled(enabled, egui::Button::new(word)).clicked()
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

    /// The toolbar (section 8): the Design workspace's SOLID tab, its panels named, and
    /// CREATE's tools, each opening its feature's dialog (M5.8): the primitives, and the
    /// features that sweep a sketch's profile (those that cut, only with a body to cut).
    fn toolbar(&mut self, ui: &mut Ui, started: &mut bool) {
        let sessions = &self.sessions;
        let mut tab = self
            .active
            .and_then(|active| self.tabs.iter_mut().find(|tab| tab.id == active));
        egui::Panel::top("toolbar")
            .exact_size(theme::TITLE_BAR_HEIGHT * 2.0)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("DESIGN").strong());
                    ui.separator();
                    ui.label(RichText::new("SOLID").strong());
                    ui.separator();
                    ui.label(RichText::new("CREATE").small());
                    ui.scope_builder(egui::UiBuilder::new().id_salt("create"), |ui| {
                        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                            node.set_role(egui::accesskit::Role::Toolbar);
                            node.set_label("CREATE");
                        });
                        ui.horizontal(|ui| {
                            for kind in Kind::ALL {
                                let enabled = tab.as_ref().is_some_and(|tab| tab.can_open(kind));
                                if ui
                                    .add_enabled(enabled, egui::Button::new(kind.title()))
                                    .clicked()
                                    && let Some(tab) = tab.as_mut()
                                {
                                    tab.open_dialog(sessions, kind);
                                    *started = true;
                                }
                            }
                        });
                    });
                    for panel in ["MODIFY", "INSPECT", "SELECT"] {
                        ui.separator();
                        ui.label(RichText::new(panel).small());
                    }
                });
            });
    }

    /// The status bar: the rebuild (`status.rebuild`) or the session's own reload, the stop
    /// button (`cancel` naming the rebuild, on while it runs, whatever command runs: section
    /// 3, *Stop*), and the last refusal.
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
                    // Rebuild (Ctrl+B), on while the tab's revision's rebuild is stopped.
                    if ui
                        .add_enabled(tab.can_rebuild(), egui::Button::new("Rebuild"))
                        .clicked()
                    {
                        tab.rebuild(&self.sessions);
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
        // Enter and Esc are the dialog's own, as keys took them: its OK and its Cancel
        // (section 3).
        let key = self.modal_key.take();
        let (enter, escape) = (key == Some(Key::Enter), key == Some(Key::Escape));
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
                    ui.add(egui::TextEdit::singleline(&mut self.open_path))
                        .labelled_by(label.id);
                });
                ui.horizontal(|ui| {
                    let word = if create { "Create" } else { "Open" };
                    // The path as typed; only an empty one (spaces aside) opens nothing.
                    if (ui.button(word).clicked() || enter) && !self.open_path.trim().is_empty() {
                        let path = std::mem::take(&mut self.open_path);
                        self.open(&path, create);
                        *started = true;
                        done = true;
                    }
                    if ui.button("Cancel").clicked() || escape {
                        done = true;
                    }
                });
            }
            Asking::SaveAs(id) => {
                let name = self
                    .tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .map(|tab| tab.name().to_owned());
                ui.heading(format!(
                    "Save {} as",
                    name.as_deref().unwrap_or("the document")
                ));
                ui.horizontal(|ui| {
                    let label = ui.label("Path");
                    ui.add(egui::TextEdit::singleline(&mut self.save_as_path))
                        .labelled_by(label.id);
                });
                ui.horizontal(|ui| {
                    if (ui.button("Save").clicked() || enter)
                        && !self.save_as_path.trim().is_empty()
                    {
                        let typed = std::mem::take(&mut self.save_as_path);
                        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                            // A relative path is in the document's folder.
                            let path = export::beside(&tab.path, &typed)
                                .to_string_lossy()
                                .into_owned();
                            tab.save_as(&self.sessions, path, false);
                        }
                        *started = true;
                        done = true;
                    }
                    if ui.button("Cancel").clicked() || escape {
                        done = true;
                    }
                });
            }
            Asking::Export(id) => {
                let sessions = &self.sessions;
                let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
                    done = true;
                    return;
                };
                let Some(form) = self.export.as_mut().filter(|form| form.session == id) else {
                    done = true;
                    return;
                };
                ui.heading(format!("Export {}", tab.name()));
                let replacing = tab
                    .export_replacing
                    .as_ref()
                    .map(|asked| asked.path.clone());
                let said = tab.export_said.clone();
                let shown = Shown {
                    running: tab.exporting(),
                    replacing: replacing.as_deref(),
                    said: said.as_deref(),
                };
                let action = match form.ui(ui, &tab.browser.bodies, &shown) {
                    export::Action::None if escape => export::Action::Cancel,
                    export::Action::None if enter => form.enter(&tab.browser.bodies, &shown),
                    action => action,
                };
                match action {
                    export::Action::None => {}
                    export::Action::Export(request) => {
                        tab.export(sessions, request);
                        *started = true;
                    }
                    export::Action::Replace => {
                        tab.replace_export(sessions);
                        *started = true;
                    }
                    export::Action::Cancel => {
                        tab.stop_export();
                        *started = true;
                        done = true;
                    }
                }
            }
            Asking::Replace(id, path) => {
                ui.label(format!("{path} already exists. Replace it?"));
                ui.horizontal(|ui| {
                    if ui.button("Replace").clicked() {
                        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                            tab.save_as(&self.sessions, path.clone(), true);
                        }
                        *started = true;
                        done = true;
                    }
                    if ui.button("Cancel").clicked() || escape {
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
                    if ui.button("Cancel").clicked() || escape {
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
                    if ui.button("Cancel").clicked() || escape {
                        done = true;
                    }
                });
            }
        });
        if done {
            if matches!(self.asking, Some(Asking::Export(_))) {
                self.export = None;
            }
            self.asking = None;
        }
    }
}

impl<P: Clients + 'static> eframe::App for App<P> {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::in_order;

    /// An open's answer as the test gives it: `Some` once answered, `None` while not.
    fn take(running: Option<u32>) -> Result<u32, Option<u32>> {
        running.ok_or(None)
    }

    fn opening(answers: &[Option<u32>]) -> Vec<(String, Option<u32>)> {
        answers
            .iter()
            .enumerate()
            .map(|(at, answer)| (format!("{at}.silico"), *answer))
            .collect()
    }

    #[test]
    fn opens_are_taken_in_the_order_asked_each_waiting_for_those_before_it() {
        // The second and third answered, the first not: none is taken, the order kept.
        let (answered, waiting) = in_order(opening(&[None, Some(2), Some(3)]), take);
        assert!(answered.is_empty());
        let paths: Vec<&str> = waiting.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(paths, ["0.silico", "1.silico", "2.silico"]);
        assert_eq!(waiting[1].1, Some(2));
        // The first answered: all three taken, in the order asked.
        let (answered, waiting) = in_order(opening(&[Some(1), Some(2), Some(3)]), take);
        assert!(waiting.is_empty());
        assert_eq!(
            answered,
            [
                ("0.silico".to_owned(), 1),
                ("1.silico".to_owned(), 2),
                ("2.silico".to_owned(), 3)
            ]
        );
        // The first answered, the second not: the first taken, the third waits behind it.
        let (answered, waiting) = in_order(opening(&[Some(1), None, Some(3)]), take);
        assert_eq!(answered, [("0.silico".to_owned(), 1)]);
        let paths: Vec<&str> = waiting.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(paths, ["1.silico", "2.silico"]);
        assert_eq!(waiting[1].1, Some(3));
    }

    #[test]
    fn nothing_opening_takes_nothing() {
        let (answered, waiting) = in_order(Vec::<(String, Option<u32>)>::new(), take);
        assert!(answered.is_empty() && waiting.is_empty());
    }
}
