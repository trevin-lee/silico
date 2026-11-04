//! The app (docs/design/m5.0-app.md, sections 2 to 4): one `Sessions`, a tab for each open
//! document, and the frame loop. Each frame polls only what answers at once (`status`,
//! `published`, and each command's `try_take`), starts the commands the person asks for,
//! and draws; it never waits on a command (invariant 4). The core's waker asks for the
//! frame after anything changes, and with nothing pending one frame a second compares the
//! files on disk.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, Ui};
use silico_core::command::{
    Opened, RebuildState, Response, Running, RunningCommand, SessionId, Sessions,
};
use silico_core::disk::{DISK_CHECK_INTERVAL, Disk};
use silico_core::kernel::Clients;

use crate::bar;
use crate::browse::{self, Ask, Browse, Browsing, Pick};
use crate::dialog::Action;
use crate::export::{self, ExportForm, Shown};
use crate::icons::Icon;
use crate::panes::{self, Pane, Panes};
use crate::recent::Recents;
use crate::start::{self, Warm};
use crate::tab::{DiskChoice, Tab, said};
use crate::theme::{self, FieldKind};
use crate::toolbar;

/// How long past [`DISK_CHECK_INTERVAL`] the idle frame comes: a comparison is due once the
/// last one ended a whole interval ago, so a frame exactly an interval after the one that
/// started it would come a little early, and the next comparison a whole interval late
/// (#267's idle cadence). This much covers a comparison's own read on any likely file.
pub const IDLE_MARGIN: Duration = Duration::from_millis(100);

/// Redo's shortcut beside Ctrl+Y: Shift+Ctrl+Z (Shift+Command+Z on macOS).
const REDO_SHIFTED: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);

/// The delay of the frame the app asks for when nothing it waits on has answered: one each
/// [`DISK_CHECK_INTERVAL`], whose `status` starts the next comparison (section 3, *Idle*).
pub const IDLE_FRAME: Duration = DISK_CHECK_INTERVAL.saturating_add(IDLE_MARGIN);

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

/// Hide/Show's shortcut, H with no modifier (DESIGN.md, *Keyboard*: Fusion's V, moved to
/// H), shown in the browser's eyes' tooltips.
pub(crate) const HIDE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::H);

/// Whether `shortcut` was pressed this frame with exactly its modifiers (egui's own match
/// ignores Shift and Alt, so Shift+H and Alt+H stay free for other commands), and freshly,
/// never the system's repeat of a key held down: taking it if so.
fn take_bare(ui: &mut Ui, shortcut: KeyboardShortcut) -> bool {
    ui.input_mut(|input| {
        let mut pressed = false;
        input.events.retain(|event| {
            let fresh = matches!(
                event,
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if *key == shortcut.logical_key && *modifiers == shortcut.modifiers
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

/// What a native dialog's answer goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Browsed {
    /// The path field of the question it was asked from (its Browse…), if that question is
    /// still asked when it comes.
    Field(Asking),
    /// Open's own panel (File > Open…, Command+O, the start page's Open…): the file picked
    /// is opened, whatever is asked meanwhile.
    Open,
}

/// An open under way: its number, the order it was asked in, the path it was asked with, and
/// its command, running.
struct Opening<R = Running<Opened>> {
    number: u64,
    path: String,
    running: R,
}

/// An open that answered: its number, the path it was asked with, and its answer.
type Answered<T = Response<Opened>> = (u64, String, T);

/// The opens under way split into those that have answered, each with its answer, and those
/// still pending, both in the order asked: every answered one taken at once, none waiting on
/// another, so an open stuck reading its file (on a network mount that stopped answering,
/// say) holds back no open asked after it.
fn split_answered<R, T>(
    opening: Vec<Opening<R>>,
    mut try_take: impl FnMut(R) -> Result<T, R>,
) -> (Vec<Answered<T>>, Vec<Opening<R>>) {
    let mut answered = Vec::new();
    let mut pending = Vec::new();
    for Opening {
        number,
        path,
        running,
    } in opening
    {
        match try_take(running) {
            Ok(answer) => answered.push((number, path, answer)),
            Err(running) => pending.push(Opening {
                number,
                path,
                running,
            }),
        }
    }
    (answered, pending)
}

/// The app's state: everything the window draws, and what it's waiting for.
pub struct App<P: Clients + 'static> {
    sessions: Arc<Sessions<P>>,
    tabs: Vec<Tab>,
    /// The tab shown, by its session.
    active: Option<SessionId>,
    opening: Vec<Opening>,
    /// The number the next open is asked with.
    next_open: u64,
    /// Each tab's open number, by its session: the tabs stand in that order, the order their
    /// opens were asked in, whatever order they answered in (each session opens on a thread
    /// of its own); and an open answering after one asked later than it, whose tab is still
    /// open, isn't shown over that one.
    open_numbers: BTreeMap<SessionId, u64>,
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
    /// The number of the open whose refusal `message` says: none if it says something else,
    /// or nothing.
    message_from: Option<u64>,
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
    /// What the viewport's renderer holds, and how its frames reach it (M5.6).
    drawing: crate::viewport::Drawing,
    /// What opens the native file dialogs, the Browse… buttons beside the path fields: none
    /// shown without it.
    browser: Option<Box<dyn Browse>>,
    /// The native dialog open, for what its answer goes to: the question it was asked from,
    /// if it's still asked when it comes, or Open, which opens it.
    browsing: Option<Browsing<Browsed>>,
    /// The folder of the last path a native dialog picked: where the next starts, if its
    /// document's folder isn't known.
    last_folder: Option<PathBuf>,
    /// The Home tab shown, over the open documents' (the start page, which shows anyway
    /// while none is open).
    home: bool,
    /// The tab last scrolled into view in the app bar: a tab shown since, by a click or an
    /// open, is scrolled to until it's in view, once.
    tab_in_view: Option<SessionId>,
    /// The documents opened lately, if the app keeps a list.
    recents: Option<Recents>,
    /// The warm start's state, if the app warms its workers.
    warm: Option<Warm>,
    /// The `silico` an MCP client is told to run (Copy MCP config).
    silico: String,
    /// The platform's Documents folder: the workspace Copy MCP config names when no
    /// document is shown and none listed in Recent will do.
    documents: Option<PathBuf>,
}

impl<P: Clients + 'static> App<P> {
    /// An app on `sessions`, with no document open.
    pub fn new(sessions: Arc<Sessions<P>>) -> Self {
        App {
            sessions,
            tabs: Vec::new(),
            active: None,
            opening: Vec::new(),
            next_open: 0,
            open_numbers: BTreeMap::new(),
            asking: None,
            open_path: String::new(),
            save_as_path: String::new(),
            export: None,
            tree: panes::layout(),
            name: None,
            message: None,
            message_from: None,
            quitting: false,
            frames: 0,
            wakes: Arc::new(AtomicU64::new(0)),
            waker_set: false,
            modal_key: None,
            drawing: crate::viewport::Drawing::default(),
            browser: None,
            browsing: None,
            last_folder: None,
            home: false,
            tab_in_view: None,
            recents: None,
            warm: None,
            silico: "silico".to_owned(),
            documents: directories::UserDirs::new()
                .and_then(|dirs| dirs.document_dir().map(std::path::Path::to_path_buf)),
        }
    }

    /// The app, Copy MCP config falling back to `documents` as the workspace (else to the
    /// platform's Documents folder): a test's, or none.
    #[must_use]
    pub fn documents_folder(mut self, documents: Option<PathBuf>) -> Self {
        self.documents = documents;
        self
    }

    /// The app, keeping the documents it opens in `recents`, which its start page lists.
    #[must_use]
    pub fn remembering(mut self, recents: Recents) -> Self {
        self.recents = Some(recents);
        self
    }

    /// The app, its status bar saying how `warm`, its workers' warm start, stands while no
    /// document is open.
    #[must_use]
    pub fn warming(mut self, warm: Warm) -> Self {
        self.warm = Some(warm);
        self
    }

    /// The app, Copy MCP config naming `silico` as the command to run (else `silico`, as
    /// found on the client's `PATH`).
    #[must_use]
    pub fn serving_with(mut self, silico: String) -> Self {
        self.silico = silico;
        self
    }

    /// Whether the start page is shown: the Home tab, or no document open.
    pub fn start_shown(&self) -> bool {
        self.home || self.tabs.is_empty()
    }

    /// The documents opened lately, if the app keeps a list.
    pub fn recents(&self) -> Option<&Recents> {
        self.recents.as_ref()
    }

    /// The workspace Copy MCP config names ([`start::workspace`]): the folder of the
    /// document shown, else of the newest recent one found, else the Documents folder.
    fn workspace(&self) -> Option<PathBuf> {
        let shown = self
            .active_place()
            .and_then(|at| self.tabs.get(at))
            .map(|tab| tab.path.as_str());
        let recents = self.recents.as_ref().map(Recents::list).unwrap_or_default();
        start::workspace(shown, recents, self.documents.as_deref())
    }

    /// The MCP config Copy MCP config copies: the `silico` it was given, serving
    /// [`workspace`](Self::workspace)'s folder, if there is one.
    pub fn mcp_config(&self) -> String {
        let workspace = self.workspace();
        start::mcp_config(
            &self.silico,
            workspace.as_deref().and_then(std::path::Path::to_str),
        )
    }

    /// The app, a Browse… button beside each path field opening `browser`'s file dialog:
    /// the platform's own ([`browse::Native`]), or a test's.
    #[must_use]
    pub fn browsing(mut self, browser: Box<dyn Browse>) -> Self {
        self.browser = Some(browser);
        self
    }

    /// The app, its viewport's frames drawn through `present` (M5.6): egui's paint callback
    /// on the GPU (`renderer::OnGpu`), or a test's own.
    #[must_use]
    pub fn presenting(mut self, present: Box<dyn crate::renderer::Present>) -> Self {
        self.drawing = crate::viewport::Drawing::new(present);
        self
    }

    /// The app with no renderer, for `why`: its viewport draws nothing, and says why.
    #[must_use]
    pub fn not_drawing(mut self, why: String) -> Self {
        self.drawing = crate::viewport::Drawing::unavailable(why);
        self
    }

    /// [`presenting`](Self::presenting), with `clock` saying the time the viewport waits on
    /// (a refused read's retry): a test's, to move by hand.
    #[must_use]
    pub fn presenting_clocked(
        mut self,
        present: Box<dyn crate::renderer::Present>,
        clock: silico_core::disk::Clock,
    ) -> Self {
        self.drawing = crate::viewport::Drawing::clocked(present, clock);
        self
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

    /// Each open document's tab, in the order their opens were asked in.
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
        let number = self.next_open;
        self.next_open += 1;
        self.opening.push(Opening {
            number,
            path: path.to_owned(),
            running: self.sessions.open(path, create),
        });
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
            // And the threads that keep the recent documents and warm the workers.
            if let Some(recents) = &self.recents {
                recents.waking(ui.ctx());
            }
            if let Some(warm) = &self.warm {
                warm.waking(ui.ctx());
            }
            self.waker_set = true;
        }
        self.poll();
        let (mut started, dialog_key) = self.keys(ui);
        started |= self.take_browsed();
        self.app_bar(ui, &mut started);
        if self.start_shown() {
            // The start page alone between the bars: no toolbar, panes, or dialog.
            self.status_bar(ui, &mut started);
            let act = egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(theme::PANEL_BG))
                .show(ui, |ui| {
                    if self.tabs.is_empty() {
                        // Nothing is drawn: the renderer lets go of what it held.
                        self.drawing.nothing_shown(ui.painter());
                    }
                    let shown = start::Shown {
                        recents: self.recents.as_ref(),
                        workspace_known: self.workspace().is_some(),
                    };
                    start::page(ui, &shown)
                })
                .inner;
            self.start_act(ui, act, &mut started);
            self.after_drawing(ui, started);
            return;
        }
        self.toolbar(ui, &mut started);
        self.status_bar(ui, &mut started);
        self.disk_banner(ui, &mut started);
        self.dialog_panel(ui, &mut started, dialog_key);
        egui::CentralPanel::default().show(ui, |ui| {
            let active = self.active_place();
            let shown = active.and_then(|at| self.tabs.get(at)).map(|tab| tab.id);
            // A left drag held in a tab no longer shown ends there, before the pane draws.
            crate::viewport::end_unshown(
                ui.ctx(),
                &self.sessions,
                &mut self.drawing,
                shown,
                &mut self.tabs,
            );
            let App {
                tree,
                tabs,
                sessions,
                name,
                drawing,
                ..
            } = self;
            let mut panes = Panes {
                tab: active.and_then(|at| tabs.get_mut(at)),
                sessions,
                name,
                started: &mut started,
                drawing,
            };
            tree.ui(&mut panes, ui);
        });
        self.after_drawing(ui, started);
    }

    /// What the start page's `act` asks: New's and Open's dialogs, a recent document opened
    /// (or its tab shown, if it's open), or the MCP config copied.
    fn start_act(&mut self, ui: &Ui, act: start::Act, started: &mut bool) {
        match act {
            start::Act::None => {}
            start::Act::New => self.asking = Some(Asking::Path { create: true }),
            start::Act::Open => self.ask_open(ui.ctx()),
            start::Act::Recent(path) => *started |= self.open_recent(&path),
            start::Act::CopyMcpConfig => ui.ctx().copy_text(self.mcp_config()),
        }
    }

    /// The recent document at `path` opened: its tab shown if it's open, nothing done if its
    /// open is under way (a double-click, or a second click before the first answers), else
    /// opened. Both compared by their absolute paths. Whether an open was started.
    fn open_recent(&mut self, path: &str) -> bool {
        let same = |other: &str| {
            std::path::absolute(other).is_ok_and(|at| at == std::path::Path::new(path))
        };
        if let Some(tab) = self.tabs.iter().find(|tab| same(&tab.path)) {
            self.active = Some(tab.id);
            self.home = false;
            return false;
        }
        if self.opening.iter().any(|opening| same(&opening.path)) {
            return false;
        }
        self.open(path, false);
        true
    }

    /// The frame's end, the window drawn: what's asked, as a modal; the window closed once
    /// a quit has closed every tab, or kept open while one holds edits; and the next frame
    /// asked for.
    fn after_drawing(&mut self, ui: &mut Ui, mut started: bool) {
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

    /// The opens under way and every tab polled; tabs whose sessions closed dropped; and
    /// what the recent documents' thread has told, a document it dropped said.
    fn poll(&mut self) {
        let gone = self.recents.as_mut().map(Recents::poll).unwrap_or_default();
        for recent in gone {
            self.message = Some(format!(
                "{} isn't in {} any more, so it's no longer listed in Recent",
                recent.name(),
                recent.folder()
            ));
            self.message_from = None;
        }
        let (answered, pending) =
            split_answered(std::mem::take(&mut self.opening), Running::try_take);
        self.opening = pending;
        self.take_opens(answered);
        for tab in &mut self.tabs {
            let path = tab.path.clone();
            tab.poll(&self.sessions);
            // Saved as another file: listed in Recent as an open is.
            if tab.path != path
                && let Some(recents) = &self.recents
            {
                recents.opened(&tab.path);
            }
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
                self.message_from = None;
            }
        }
        let open_numbers = &mut self.open_numbers;
        self.tabs.retain(|tab| {
            if tab.closed {
                open_numbers.remove(&tab.id);
            }
            !tab.closed
        });
        if self.active_place().is_none() {
            self.active = self.tabs.last().map(|tab| tab.id);
        }
    }

    /// The opens that answered since the last poll, `(number, path, response)` each, taken in
    /// the order they were asked, so what's said comes out the same whether they answered in
    /// one poll or in several.
    fn take_opens(&mut self, answered: Vec<Answered>) {
        for (number, path, response) in answered {
            self.take_open(number, path, response);
        }
    }

    /// An open that answered, taken at once, whatever is still opening: its refusal said, or
    /// its tab put after every tab whose open was asked before it, and shown unless the tab of
    /// an open asked after it is open (a late answer never takes the tab shown). An open asked
    /// after the one whose refusal is said clears it (as one does any other message); one
    /// asked before it, answering late, never does.
    fn take_open(&mut self, number: u64, path: String, response: Response<Opened>) {
        let opened = match response.result {
            Ok(opened) => opened,
            Err(problems) => {
                self.message = Some(said(&problems));
                self.message_from = Some(number);
                // A recent document's: dropped from the list if nothing is at its path.
                if let Some(recents) = self.recents.as_ref().filter(|recents| recents.holds(&path))
                {
                    recents.check(&path);
                }
                return;
            }
        };
        if let Some(recents) = &self.recents {
            recents.opened(&path);
        }
        if self.message_from.is_none_or(|from| number > from) {
            self.message = None;
            self.message_from = None;
        }
        let id = opened.session;
        let at = self
            .tabs
            .iter()
            .position(|tab| {
                self.open_numbers
                    .get(&tab.id)
                    .is_some_and(|&asked| asked > number)
            })
            .unwrap_or(self.tabs.len());
        let shown = self
            .open_numbers
            .values()
            .max()
            .is_none_or(|&newest| number > newest);
        self.tabs
            .insert(at, Tab::opened(id, path, &opened.problems));
        self.open_numbers.insert(id, number);
        if shown {
            self.active = Some(id);
            self.home = false;
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
    /// open), Ctrl+W closes, Ctrl+Z undoes and Ctrl+Y or Ctrl+Shift+Z redoes
    /// (docs/ux/solidworks.md, §6; DESIGN.md, *Keyboard*), Esc stops what's running and
    /// Ctrl+B rebuilds what was stopped (§5, *Rebuild*), and H hides or shows the selected
    /// body or sketch, outside a text field. With a dialog open, Enter and Esc are the
    /// dialog's, taken here and given to it once its inputs have taken this frame's typing
    /// ([`dialog_panel`](Self::dialog_panel)). Whether a command was started, and the
    /// dialog's key.
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
        // Ctrl+N and Ctrl+O: New's dialog and Open's panel, as the File menu's and the
        // start page's.
        for (key, create) in [(Key::N, true), (Key::O, false)] {
            if ui.input_mut(|input| {
                input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, key))
            }) {
                if create {
                    self.asking = Some(Asking::Path { create });
                } else {
                    let ctx = ui.ctx().clone();
                    self.ask_open(&ctx);
                }
                return (false, None);
            }
        }
        // The start page over the open documents (Home): no document's shortcut acts on the
        // one hidden; Esc and Ctrl+W leave Home for it.
        if self.home {
            let leave = ui.input_mut(|input| {
                input.consume_key(Modifiers::NONE, Key::Escape)
                    || input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::W))
            });
            if leave {
                self.home = false;
            }
            return (false, None);
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
        // Shift+Ctrl+Z before Ctrl+Z, which egui would match with the Shift aside: Redo, as
        // Ctrl+Y is (DESIGN.md, *Keyboard*), never an Undo.
        let shift_redo = ui.input_mut(|input| input.consume_shortcut(&REDO_SHIFTED));
        let (save, close, undo, redo, rebuild) = (
            pressed(Key::S),
            pressed(Key::W),
            pressed(Key::Z),
            pressed(Key::Y) || shift_redo,
            pressed(Key::B),
        );
        let stop = ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        // H hides or shows the selection (DESIGN.md, *Keyboard*), never a letter typed.
        let hide = !ui.ctx().text_edit_focused() && take_bare(ui, HIDE);
        let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
            return (false, None);
        };
        if hide {
            tab.toggle_selected_shown();
        }
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
    /// dialog left open, or the dialog, which it cancels (one still being read too;
    /// [`Tab::dialog_escape`]).
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
            Some(Key::Escape) if tab.dialog_escape(sessions) => action = Action::Cancel,
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

    /// The native dialog's answer, if it has come, never waited for: Open's panel's pick
    /// opened; a path picked through Browse… filling the path field of the question it was
    /// asked from (the typed-path Open's opens it), if that question is still asked; a
    /// cancel, or a question no longer asked, changes nothing, nor does Export's while its
    /// export runs or asks whether to replace. Whether a command was started.
    fn take_browsed(&mut self) -> bool {
        let Some(answer) = self.browsing.as_mut().and_then(Browsing::poll) else {
            return false;
        };
        let Some(Browsing { what, .. }) = self.browsing.take() else {
            return false;
        };
        let Some(path) = answer else {
            return false;
        };
        if let Some(folder) = path
            .parent()
            .filter(|folder| !folder.as_os_str().is_empty())
        {
            self.last_folder = Some(folder.to_path_buf());
        }
        let what = match what {
            Browsed::Field(what) if self.asking.as_ref() == Some(&what) => Some(what),
            Browsed::Field(_) => return false,
            Browsed::Open => None,
        };
        let path = match path.into_os_string().into_string() {
            Ok(path) => path,
            Err(path) => {
                self.message = Some(format!(
                    "{} can't be used: its path isn't UTF-8",
                    std::path::Path::new(&path).display()
                ));
                self.message_from = None;
                return false;
            }
        };
        let Some(what) = what else {
            // Open's panel: the file picked opened, as Browse…'s pick in the typed-path Open
            // is, its tab and the recent documents following once it answers.
            self.open(&path, false);
            return true;
        };
        match what {
            Asking::Path { create: false } => {
                self.open(&path, false);
                // As a typed path's open takes it.
                self.open_path.clear();
                self.asking = None;
                return true;
            }
            Asking::Path { create: true } => self.open_path = path,
            Asking::SaveAs(_) => self.save_as_path = path,
            Asking::Export(id) => {
                // Its path field is off while the export runs or asks whether to replace.
                let idle = self
                    .tabs
                    .iter()
                    .any(|tab| tab.id == id && !tab.exporting() && tab.export_replacing.is_none());
                if let Some(form) = self
                    .export
                    .as_mut()
                    .filter(|form| idle && form.session == id)
                {
                    form.browsed(path);
                }
            }
            Asking::Close(_) | Asking::Quit | Asking::Replace(..) => {}
        }
        false
    }

    /// The Browse… button beside a path field, if the app has native dialogs: off while one
    /// is open. Whether it was clicked.
    fn browse_button(&self, ui: &mut Ui) -> bool {
        self.browser.is_some() && browse::button(ui, self.browsing.is_none())
    }

    /// Open (File > Open…, Command+O, the start page's Open…): the platform's own open panel
    /// at once, the file picked opened when it answers ([`take_browsed`](Self::take_browsed)),
    /// a cancel doing nothing. With no native dialogs, or one that won't open, or while
    /// another native dialog is open (a Browse…'s), the typed-path Open asked instead; while
    /// Open's own panel is open, nothing.
    fn ask_open(&mut self, ctx: &egui::Context) {
        let typed = Asking::Path { create: false };
        match &self.browsing {
            Some(browsing) if browsing.what == Browsed::Open => return,
            Some(_) => {
                self.asking = Some(typed);
                return;
            }
            None => {}
        }
        let Some(ask) = self.ask_of(&typed) else {
            return;
        };
        if !self.start_browsing(Browsed::Open, &ask, ctx) {
            self.asking = Some(typed);
        }
    }

    /// The native dialog of `asking`'s path field, opened, its answer polled each frame; if
    /// none will open, said.
    fn browse(&mut self, asking: Asking, ctx: &egui::Context) {
        if self.browsing.is_some() {
            return;
        }
        let Some(ask) = self.ask_of(&asking) else {
            return;
        };
        if !self.start_browsing(Browsed::Field(asking), &ask, ctx) {
            self.message = Some("The file dialog didn't open: type the path".to_owned());
            self.message_from = None;
        }
    }

    /// The native dialog `ask`, opened for `what`, its answer polled each frame: whether one
    /// opened (none without native dialogs).
    fn start_browsing(&mut self, what: Browsed, ask: &Ask, ctx: &egui::Context) -> bool {
        let Some(answer) = self
            .browser
            .as_mut()
            .and_then(|browser| browser.browse(ask))
        else {
            return false;
        };
        self.browsing = Some(Browsing::new(what, answer, ctx));
        true
    }

    /// What the native dialog of `asking`'s path field is asked: where it starts, its
    /// filters, and its title. None for a question with no path, or of a tab or form gone.
    fn ask_of(&self, asking: &Asking) -> Option<Ask> {
        let tab = |id: SessionId| self.tabs.iter().find(|tab| tab.id == id);
        // The document's folder, or else the last a dialog picked in (else the platform's).
        let folder = |document: Option<&str>| {
            document
                .and_then(browse::folder_of)
                .or_else(|| self.last_folder.clone())
        };
        let ask = match asking {
            Asking::Path { create } => {
                let shown = self.active_place().and_then(|at| self.tabs.get(at));
                Ask {
                    pick: if *create { Pick::Save } else { Pick::Open },
                    title: if *create {
                        "New document"
                    } else {
                        "Open document"
                    }
                    .to_owned(),
                    filters: vec![browse::DOCUMENTS],
                    folder: folder(shown.map(|tab| tab.path.as_str())),
                    file_name: None,
                }
            }
            Asking::SaveAs(id) => {
                let tab = tab(*id)?;
                Ask {
                    pick: Pick::Save,
                    title: format!("Save {} as", tab.name()),
                    filters: vec![browse::DOCUMENTS],
                    folder: folder(Some(&tab.path)),
                    file_name: browse::file_name_of(&tab.path),
                }
            }
            Asking::Export(id) => {
                let tab = tab(*id)?;
                let form = self.export.as_ref().filter(|form| form.session == *id)?;
                Ask {
                    pick: Pick::Save,
                    title: format!("Export {}", tab.name()),
                    filters: browse::export_filters(form.step),
                    folder: folder(Some(&tab.path)),
                    file_name: browse::file_name_of(&form.path),
                }
            }
            Asking::Close(_) | Asking::Quit | Asking::Replace(..) => return None,
        };
        Some(ask)
    }

    /// The application bar (section 8; docs/design/m5-ui/DESIGN.md, *Layout*): Home, the
    /// File menu, Save, Undo and Redo, and the open documents' tabs, an asterisk on a changed
    /// one's (docs/ux/solidworks.md, §6), scrolling across when they overflow. On macOS it's
    /// the window's title bar (`bar`).
    fn app_bar(&mut self, ui: &mut Ui, started: &mut bool) {
        egui::Panel::top("app_bar")
            .exact_size(theme::TOP_BAR_HEIGHT)
            .frame(egui::Frame::NONE.fill(theme::TOP_BAR_BG))
            .show_separator_line(false)
            .show(ui, |ui| {
                let bar = ui.max_rect();
                bar::move_window(ui, bar);
                bar::border(ui, bar);
                // The row above the border, its items centred in it.
                let row =
                    egui::Rect::from_min_max(bar.min, egui::pos2(bar.right(), bar.bottom() - 1.0));
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(row)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = theme::APP_BAR_ITEM_SPACING;
                        bar::inset(ui);
                        self.app_bar_buttons(ui, started);
                        bar::separator(ui);
                        self.document_tabs(ui, started);
                    },
                );
            });
    }

    /// The app bar's buttons: Home, at the far left (the start page, over the open
    /// documents), the File menu, Save, Undo, and Redo.
    fn app_bar_buttons(&mut self, ui: &mut Ui, started: &mut bool) {
        let start_shown = self.start_shown();
        let home = bar::button(ui, Icon::Home, None, "Home", true, Some(start_shown));
        let home = bar::tooltip(
            home,
            "Home",
            None,
            Some("New, Open, and the documents opened lately"),
        );
        if home.clicked() {
            self.home = true;
        }
        let file = bar::button(ui, Icon::Menu, Some("File"), "File", true, None);
        bar::menu(&file, |ui| self.file_menu(ui, started));
        // Off on Home, as the shortcuts are (the prototype's `S.view === 'doc'`): the
        // document is hidden.
        let tab = self
            .active_place()
            .filter(|_| !start_shown)
            .and_then(|at| self.tabs.get_mut(at));
        // Off while a dialog is open: its OK or Cancel ends its edits first.
        let unsaved = tab
            .as_ref()
            .is_some_and(|tab| tab.unsaved() && !tab.in_dialog());
        let said = tab
            .as_ref()
            .filter(|_| unsaved)
            .map(|tab| format!("Writes your edits to {}", tab.name()));
        let save = bar::button(ui, Icon::Save, None, "Save", unsaved, None);
        let shortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        if bar::tooltip(save, "Save", Some(shortcut), said.as_deref()).clicked()
            && let Some(tab) = tab
        {
            tab.save(&self.sessions, false);
            *started = true;
        }
        let (undoable, redoable) = self
            .active_place()
            .filter(|_| !start_shown)
            .and_then(|at| self.tabs.get(at))
            .map_or((false, false), |tab| (tab.undoable(), tab.redoable()));
        // Redo's shortcut as the prototype names it: Shift+Command+Z on macOS, Ctrl+Y
        // elsewhere (each works on both).
        let redo = if ui.ctx().os() == egui::os::OperatingSystem::Mac {
            REDO_SHIFTED
        } else {
            KeyboardShortcut::new(Modifiers::COMMAND, Key::Y)
        };
        for (undo, icon, word, shortcut, enabled) in [
            (
                true,
                Icon::Undo,
                "Undo",
                KeyboardShortcut::new(Modifiers::COMMAND, Key::Z),
                undoable,
            ),
            (false, Icon::Redo, "Redo", redo, redoable),
        ] {
            let button = bar::button(ui, icon, None, word, enabled, None);
            if bar::tooltip(button, word, Some(shortcut), None).clicked()
                && let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at))
            {
                tab.step(&self.sessions, undo);
                *started = true;
            }
        }
    }

    /// The File menu's items: New…, Open…, Save As…, Export…, Revert, and Close.
    fn file_menu(&mut self, ui: &mut Ui, started: &mut bool) {
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
        if ui.button("New…").clicked() {
            self.asking = Some(Asking::Path { create: true });
            ui.close();
        }
        if ui.button("Open…").clicked() {
            let ctx = ui.ctx().clone();
            self.ask_open(&ctx);
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
            if let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) {
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
            if let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) {
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
    }

    /// The open documents' tabs, after the separator: a click shows one's document, and its
    /// ×, shown while it's hovered or shown, closes it. A tab newly shown is scrolled into
    /// view.
    fn document_tabs(&mut self, ui: &mut Ui, started: &mut bool) {
        let start_shown = self.start_shown();
        let mut show = None;
        let mut close = None;
        let in_view = &mut self.tab_in_view;
        bar::tab_strip(ui, |ui| {
            for (at, tab) in self.tabs.iter().enumerate() {
                let active = Some(tab.id) == self.active && !start_shown;
                let drawn = bar::doc_tab(
                    ui,
                    &bar::DocTab {
                        name: tab.name(),
                        path: &tab.path,
                        unsaved: tab.unsaved(),
                        active,
                        first: at == 0,
                    },
                );
                // Scrolled to until it's in view; then left where the person scrolls.
                if active && *in_view != Some(tab.id) && drawn.in_view(ui) {
                    *in_view = Some(tab.id);
                }
                if drawn.close.clicked() {
                    close = Some(tab.id);
                } else if drawn.tab.clicked() {
                    show = Some(tab.id);
                }
            }
        });
        if let Some(id) = show {
            self.active = Some(id);
            self.home = false;
        }
        if let Some(id) = close {
            self.close(id);
            *started = true;
        }
    }

    /// The toolbar (section 8; `toolbar`): the Design workspace's SOLID tab and its panels,
    /// CREATE's tools each opening its feature's dialog (M5.8): the primitives, and the
    /// features that sweep a sketch's profile (those that cut, only with a body to cut); and
    /// SELECT's Clear selection.
    fn toolbar(&mut self, ui: &mut Ui, started: &mut bool) {
        let mut tab = self
            .active
            .and_then(|active| self.tabs.iter_mut().find(|tab| tab.id == active));
        let act = {
            let shown = tab.as_deref();
            let can_open = |kind| shown.is_some_and(|tab| tab.can_open(kind));
            toolbar::show(
                ui,
                &toolbar::Shown {
                    // Sketch mode (#351) wires these: `tab.in_sketch() -> bool`, and
                    // `Some(tab.can_sketch())` (`tab.can_sketch() -> bool`).
                    in_sketch: false,
                    create_sketch: None,
                    can_open: &can_open,
                    selected: shown.is_some_and(|tab| tab.selected.is_some() && !tab.in_dialog()),
                },
            )
        };
        let Some(tab) = tab.as_mut() else {
            return;
        };
        match act {
            Some(toolbar::Act::Dialog(kind)) => {
                tab.open_dialog(&self.sessions, kind);
                *started = true;
            }
            Some(toolbar::Act::ClearSelection) => tab.selected = None,
            // Sketch mode (#351) wires these: `CreateSketch(plane)` to
            // `tab.create_sketch(&self.sessions, plane)` and `*started = true`, and
            // `FinishSketch` to `tab.finish_sketch(&self.sessions, true)` (false is Cancel).
            Some(toolbar::Act::CreateSketch(_) | toolbar::Act::FinishSketch) | None => {}
        }
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
                        // No document: how the warm start stands (decision 10).
                        if let Some(warm) = &self.warm {
                            start::warm_status(ui, warm.state());
                        }
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
        let mut browse = false;
        egui::Modal::new(egui::Id::new("asking")).show(ui.ctx(), |ui| match asking {
            Asking::Path { create } => {
                ui.heading(if create {
                    "New document"
                } else {
                    "Open document"
                });
                ui.horizontal(|ui| {
                    let label = ui.label("Path");
                    let edit = egui::TextEdit::singleline(&mut self.open_path);
                    ui.add(theme::field(
                        ui,
                        edit,
                        "open-path",
                        FieldKind::Modify,
                        false,
                    ))
                    .labelled_by(label.id);
                    browse = self.browse_button(ui);
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
                    let edit = egui::TextEdit::singleline(&mut self.save_as_path);
                    ui.add(theme::field(
                        ui,
                        edit,
                        "save-as-path",
                        FieldKind::Modify,
                        false,
                    ))
                    .labelled_by(label.id);
                    browse = self.browse_button(ui);
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
                    browse: self.browser.as_ref().map(|_| self.browsing.is_none()),
                };
                let action = match form.ui(ui, &tab.browser.bodies, &shown) {
                    export::Action::None if escape => export::Action::Cancel,
                    export::Action::None if enter => form.enter(&tab.browser.bodies, &shown),
                    action => action,
                };
                match action {
                    export::Action::None => {}
                    export::Action::Browse => browse = true,
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
        if browse
            && !done
            && let Some(asking) = self.asking.clone()
        {
            self.browse(asking, ui.ctx());
        }
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
    use std::sync::Arc;

    use silico_core::cache::Cache;
    use silico_core::command::{SessionId, Sessions};
    use silico_core::fake::{FakeKernel, FakeSketches};
    use silico_core::sketch::SketchSolver;
    use tempfile::TempDir;

    use super::{Answered, App, Opening, split_answered};
    use crate::tab::said;

    /// A block: any document the gate takes.
    const BLOCK: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

    type Fake = Arc<FakeKernel>;

    /// The app on sessions of the fake kernel, as the app's tests have them, with nothing
    /// open; and the directory its files are made in.
    fn app() -> (App<Fake>, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        let sketches = FakeSketches::new();
        let sessions = Sessions::with_stand_ins(
            Arc::new(FakeKernel::new(2).unwrap()),
            move |cache| Cache::with_secret(cache, &secret),
            move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
        );
        (App::new(Arc::new(sessions)), dir)
    }

    /// The open of `name` in `dir`, asked as the app's `number`th and waited for: a block
    /// written there first if `write`, or no file at all, which `open` refuses.
    fn answered(app: &App<Fake>, dir: &TempDir, number: u64, name: &str, write: bool) -> Answered {
        let path = dir.path().join(name);
        if write {
            std::fs::write(&path, BLOCK).unwrap();
        }
        let answer = asked(app, number, &path);
        assert_eq!(answer.2.result.is_ok(), write, "{name}'s open");
        answer
    }

    /// The open of `path`, asked as the app's `number`th and waited for.
    fn asked(app: &App<Fake>, number: u64, path: &std::path::Path) -> Answered {
        let path = path.to_str().unwrap().to_owned();
        let response = app.sessions().open(&path, false).wait();
        (number, path, response)
    }

    /// What the app says once a refusal and an open, asked as its `refused`th and `opened`th,
    /// are taken, in one poll (in the order asked) or in two (the refusal's first); and what
    /// the refusal said.
    fn said_after(refused: u64, opened: u64, one_poll: bool) -> (Option<String>, String) {
        let (mut app, dir) = app();
        let refusal = answered(&app, &dir, refused, "missing.silico", false);
        let open = answered(&app, &dir, opened, "one.silico", true);
        let said_of_it = said(refusal.2.result.as_ref().unwrap_err());
        if one_poll {
            let mut poll = vec![refusal, open];
            poll.sort_by_key(|(number, ..)| *number);
            app.take_opens(poll);
        } else {
            app.take_opens(vec![refusal]);
            app.take_opens(vec![open]);
        }
        assert_eq!(names(&app), ["one.silico"]);
        (app.message().map(str::to_owned), said_of_it)
    }

    fn session(answered: &Answered) -> SessionId {
        answered.2.result.as_ref().unwrap().session
    }

    fn names(app: &App<Fake>) -> Vec<&str> {
        app.tabs().iter().map(|tab| tab.name()).collect()
    }

    #[test]
    fn tabs_stand_in_the_order_their_opens_were_asked_whatever_order_they_answer_in() {
        let (mut app, dir) = app();
        let one = answered(&app, &dir, 1, "one.silico", true);
        let two = answered(&app, &dir, 2, "two.silico", true);
        let three = answered(&app, &dir, 3, "three.silico", true);
        let shown = session(&three);
        // Three answers first, then one, then two, each in a poll of its own.
        app.take_opens(vec![three]);
        assert_eq!(names(&app), ["three.silico"]);
        app.take_opens(vec![one]);
        assert_eq!(names(&app), ["one.silico", "three.silico"]);
        app.take_opens(vec![two]);
        assert_eq!(names(&app), ["one.silico", "two.silico", "three.silico"]);
        // The last asked is shown, though it answered first.
        assert_eq!(app.active, Some(shown));
        assert_eq!(app.message(), None);
    }

    #[test]
    fn opens_answering_together_out_of_order_stand_in_the_order_asked() {
        let (mut app, dir) = app();
        let one = answered(&app, &dir, 1, "one.silico", true);
        let two = answered(&app, &dir, 2, "two.silico", true);
        let three = answered(&app, &dir, 3, "three.silico", true);
        let shown = session(&three);
        app.take_opens(vec![three, one, two]);
        assert_eq!(names(&app), ["one.silico", "two.silico", "three.silico"]);
        assert_eq!(app.active, Some(shown));
    }

    #[test]
    fn an_open_asked_after_a_refusal_clears_it_in_the_same_poll_or_a_later_one() {
        for one_poll in [true, false] {
            let (message, _) = said_after(1, 2, one_poll);
            assert_eq!(message, None, "in one poll: {one_poll}");
        }
    }

    #[test]
    fn an_open_asked_before_a_refusal_never_clears_it_in_the_same_poll_or_a_later_one() {
        for one_poll in [true, false] {
            let (message, said_of_it) = said_after(2, 1, one_poll);
            assert_eq!(message, Some(said_of_it), "in one poll: {one_poll}");
        }
    }

    #[test]
    fn of_two_refusals_in_one_poll_the_one_asked_last_is_said() {
        let (mut app, dir) = app();
        let first = answered(&app, &dir, 1, "missing.silico", false);
        // A directory: refused for what it is, not for being missing, so said otherwise.
        let folder = dir.path().join("folder.silico");
        std::fs::create_dir(&folder).unwrap();
        let third = asked(&app, 3, &folder);
        let said_of_first = said(first.2.result.as_ref().unwrap_err());
        let said_of_third = said(third.2.result.as_ref().unwrap_err());
        assert_ne!(said_of_first, said_of_third);
        app.take_opens(vec![first, third]);
        assert_eq!(app.message(), Some(said_of_third.as_str()));
        assert!(app.tabs().is_empty());
        // A poll in which nothing answers leaves it as it is.
        app.take_opens(Vec::new());
        assert_eq!(app.message(), Some(said_of_third.as_str()));
    }

    #[test]
    fn an_open_answering_after_one_asked_later_never_takes_the_tab_shown() {
        let (mut app, dir) = app();
        let one = answered(&app, &dir, 1, "one.silico", true);
        let two = answered(&app, &dir, 2, "two.silico", true);
        let shown = session(&two);
        app.take_opens(vec![two]);
        assert_eq!(app.active, Some(shown));
        app.take_opens(vec![one]);
        assert_eq!(app.active, Some(shown));
        assert_eq!(names(&app), ["one.silico", "two.silico"]);
    }

    #[test]
    fn a_refusal_stays_said_when_an_open_asked_before_it_answers_in_a_later_poll() {
        let (mut app, dir) = app();
        let one = answered(&app, &dir, 1, "one.silico", true);
        let refused = answered(&app, &dir, 2, "missing.silico", false);
        let said_of_it = said(refused.2.result.as_ref().unwrap_err());
        app.take_opens(vec![refused]);
        assert_eq!(app.message(), Some(said_of_it.as_str()));
        app.take_opens(vec![one]);
        assert_eq!(app.message(), Some(said_of_it.as_str()));
        assert_eq!(names(&app), ["one.silico"]);
        // One asked after it, opening, clears it.
        let two = answered(&app, &dir, 3, "two.silico", true);
        app.take_opens(vec![two]);
        assert_eq!(app.message(), None);
    }

    #[test]
    fn an_open_answering_after_a_later_one_whose_tab_has_closed_is_shown() {
        let (mut app, dir) = app();
        let one = answered(&app, &dir, 1, "one.silico", true);
        let two = answered(&app, &dir, 2, "two.silico", true);
        let three = answered(&app, &dir, 3, "three.silico", true);
        let (first, second, third) = (session(&one), session(&two), session(&three));
        app.take_opens(vec![one, three]);
        assert_eq!(app.active, Some(third));
        // The third's tab closed: the first is shown in its place.
        assert!(app.sessions().close(&third).wait().result.is_ok());
        app.poll();
        assert_eq!(app.active, Some(first));
        // The second, answering now, was asked after every open still open: it's shown.
        app.take_opens(vec![two]);
        assert_eq!(app.active, Some(second));
        assert_eq!(names(&app), ["one.silico", "two.silico"]);
    }

    #[test]
    fn answered_opens_are_taken_in_the_order_asked_around_one_still_pending() {
        // Each stub open answers `Some` at once, or `None`, pending (stuck reading its file).
        let opening: Vec<Opening<Option<u32>>> = [(1, Some(10)), (2, None), (3, Some(30))]
            .into_iter()
            .map(|(number, running)| Opening {
                number,
                path: format!("{number}.silico"),
                running,
            })
            .collect();
        let (answered, pending) = split_answered(opening, |running| running.ok_or(None));
        assert_eq!(
            answered,
            [
                (1, "1.silico".to_owned(), 10),
                (3, "3.silico".to_owned(), 30)
            ]
        );
        let pending: Vec<(u64, &str)> = pending
            .iter()
            .map(|opening| (opening.number, opening.path.as_str()))
            .collect();
        assert_eq!(pending, [(2, "2.silico")]);
    }

    #[test]
    fn an_open_answering_after_a_later_one_was_refused_is_shown() {
        // The later open made no tab, so the earlier one, answering after it, takes none
        // from it.
        let (mut app, dir) = app();
        let one = answered(&app, &dir, 1, "one.silico", true);
        let refused = answered(&app, &dir, 2, "missing.silico", false);
        let shown = session(&one);
        app.take_opens(vec![refused]);
        app.take_opens(vec![one]);
        assert_eq!(app.active, Some(shown));
        assert_eq!(names(&app), ["one.silico"]);
    }

    #[test]
    fn a_closed_tabs_open_number_goes_with_it() {
        let (mut app, dir) = app();
        let one = answered(&app, &dir, 1, "one.silico", true);
        let two = answered(&app, &dir, 2, "two.silico", true);
        let (first, second) = (session(&one), session(&two));
        app.take_opens(vec![one, two]);
        assert!(app.sessions().close(&first).wait().result.is_ok());
        app.poll();
        assert_eq!(names(&app), ["two.silico"]);
        assert_eq!(app.open_numbers.keys().collect::<Vec<_>>(), [&second]);
    }

    #[test]
    fn a_recent_document_clicked_again_before_its_open_answers_opens_once() {
        let (mut app, dir) = app();
        let path = dir.path().join("one.silico");
        std::fs::write(&path, BLOCK).unwrap();
        let path = path.to_str().unwrap();
        assert!(app.open_recent(path));
        // A double-click's second click, or another before the open answers: nothing more.
        assert!(!app.open_recent(path));
        assert_eq!(app.opening.len(), 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while app.tabs().is_empty() {
            assert!(std::time::Instant::now() < deadline, "never opened");
            app.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Opened: a click shows its tab, from Home, and opens nothing.
        app.home = true;
        assert!(!app.open_recent(path));
        assert!(app.opening.is_empty());
        assert_eq!(names(&app), ["one.silico"]);
        assert!(!app.home);
        assert_eq!(app.message(), None);
    }

    #[test]
    fn opens_are_numbered_in_the_order_asked() {
        let (mut app, dir) = app();
        for name in ["one.silico", "two.silico"] {
            let path = dir.path().join(name);
            std::fs::write(&path, BLOCK).unwrap();
            app.open(path.to_str().unwrap(), false);
        }
        let numbers: Vec<u64> = app.opening.iter().map(|opening| opening.number).collect();
        assert_eq!(numbers, [0, 1]);
    }
}
