//! The app (docs/design/m5.0-app.md, sections 2 to 4): one `Sessions`, a tab for each open
//! document, and the frame loop. Each frame polls only what answers at once (`status`,
//! `published`, and each command's `try_take`), starts the commands the person asks for,
//! and draws; it never waits on a command (invariant 4). The core's waker asks for the
//! frame after anything changes, and with nothing pending one frame a second compares the
//! files on disk.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use eframe::egui::{self, Key, Modifiers, Ui};
use silico_core::command::{Opened, Response, Running, SessionId, Sessions};
use silico_core::disk::{DISK_CHECK_INTERVAL, Disk};
use silico_core::kernel::Clients;
use silico_core::views::NamedView;

use crate::banner::{self, Banner, Style, Tone};
use crate::bar;
use crate::browse::{self, Ask, Browse, Browsing, Pick};
use crate::changes::{self, Changes};
use crate::export::{self, ExportForm, Shown};
use crate::icons::Icon;
use crate::modal::{self, Button, Kind};
use crate::panes::{self, Pane, Panes};
use crate::properties;
use crate::recent::Recents;
use crate::sheet::{self, Sheet};
use crate::shortcuts::{self, Command};
use crate::sketch::{self, bar::BarAction};
use crate::start::{self, Warm};
use crate::tab::{DiskChoice, Tab, said};
use crate::theme::{self, FieldKind};
use crate::toolbar;
use crate::viewport::ViewAsk;

/// SolidWorks' isometric view (Ctrl+7), Z up: its front, right, and top toward the viewer,
/// as the ViewCube's top front right corner gives it.
const ISOMETRIC: NamedView = NamedView::TopFrontRight;

/// How long past [`DISK_CHECK_INTERVAL`] the idle frame comes: a comparison is due once the
/// last one ended a whole interval ago, so a frame exactly an interval after the one that
/// started it would come a little early, and the next comparison a whole interval late
/// (#267's idle cadence). This much covers a comparison's own read on any likely file.
pub const IDLE_MARGIN: Duration = Duration::from_millis(100);

/// The delay of the frame the app asks for when nothing it waits on has answered: one each
/// [`DISK_CHECK_INTERVAL`], whose `status` starts the next comparison (section 3, *Idle*).
pub const IDLE_FRAME: Duration = DISK_CHECK_INTERVAL.saturating_add(IDLE_MARGIN);

/// What F5 says (the prototype's toast, interact.js): SolidWorks' key for its filter
/// toolbar, which the app has none of (DESIGN.md, open question 2).
pub const FILTERS_SAID: &str =
    "F5 shows the selection filters. They live in the toolbar, under SELECT.";

/// The selection filters' keys pressed this frame, X, E, and V (`crate::shortcuts`), each
/// taken and its kind given, in that order; none taken while a sketch is open (`in_sketch`),
/// whose X is Construction and whose picking is its own.
pub fn filter_keys(ui: &mut Ui, in_sketch: bool) -> Vec<toolbar::FilterKind> {
    if in_sketch {
        return Vec::new();
    }
    [
        (Command::FilterFace, toolbar::FilterKind::Faces),
        (Command::FilterEdge, toolbar::FilterKind::Edges),
        (Command::FilterVertex, toolbar::FilterKind::Vertices),
    ]
    .into_iter()
    .filter(|(command, _)| shortcuts::take(ui, *command))
    .map(|(_, kind)| kind)
    .collect()
}

/// A toast saying `text` over `canvas`'s bottom right (the prototype's `.toast`): drawn in
/// its own area, above the panes, taking no input. SEAM: the prototype's info icon and its
/// fade, which the UI port adds.
fn toast(ctx: &egui::Context, canvas: egui::Rect, text: &str) {
    let corner =
        canvas.right_bottom() - egui::vec2(theme::TOAST_MARGIN_RIGHT, theme::TOAST_MARGIN_BOTTOM);
    egui::Area::new(egui::Id::new("toast"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fixed_pos(corner)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::TOAST_BG)
                .stroke(egui::Stroke::new(theme::BORDER_WIDTH, theme::TOAST_BORDER))
                .corner_radius(theme::TOAST_CORNER_RADIUS)
                .shadow(theme::TOAST_SHADOW)
                .inner_margin(theme::TOAST_PADDING)
                .show(ui, |ui| {
                    ui.set_max_width(theme::TOAST_MAX_WIDTH);
                    ui.label(egui::RichText::new(text).color(theme::TOAST_TEXT));
                });
        });
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

/// Where sketch mode's panels stand in the canvas, as the prototype's: the sketch bar below
/// its top, centred; the palette at its right, below the ViewCube.
const SKETCH_BAR_FROM_TOP: f32 = 10.0;
const PALETTE_FROM_CORNER: egui::Vec2 =
    egui::Vec2::new(-12.0 - sketch::style::SKETCH_PALETTE_WIDTH, 118.0);

/// What the app is asking the person.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asking {
    /// A path to open, typed: Open's fallback when its panel won't open.
    Open,
    /// Whether to save a changed document before closing it (docs/ux/solidworks.md, §6),
    /// by its session, whatever opens or closes meanwhile.
    Close(SessionId),
    /// Whether to save the changed documents before quitting.
    Quit,
    /// A path to save `session`'s document to, typed (Save As, and an untitled part's
    /// Save): the Save panel's fallback when it won't open. Then closed, if `then_close`
    /// (Close's or Quit's Save of an untitled part), whose Cancel refuses that close.
    SaveAs {
        session: SessionId,
        then_close: bool,
    },
    /// Whether to replace the file a typed Save As of `session`'s document found at
    /// `path`; then closed, if `then_close`.
    Replace {
        session: SessionId,
        path: String,
        then_close: bool,
    },
    /// What to export of session's document, and where (M5.12): the form is `export`.
    Export(SessionId),
    /// What changed on disk while `session`'s edits are held (DESIGN.md, 6d's Show
    /// changes): the tab's `held_changes`.
    Changes(SessionId),
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
    /// The Save panel of `session`'s document (Save As, and an untitled part's Save): its
    /// document saved as the file picked, if its tab is still open and not closing, then
    /// closed if `then_close`; a cancel, if it was to close, refuses the close.
    SaveAs {
        session: SessionId,
        then_close: bool,
    },
}

/// What an open was asked for: the document at a path, or an untitled part, shown by the
/// name it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    File(String),
    Untitled(String),
}

impl Part {
    /// The path asked for: none for an untitled part.
    fn path(&self) -> Option<&str> {
        match self {
            Part::File(path) => Some(path),
            Part::Untitled(_) => None,
        }
    }
}

/// An open under way: its number, the order it was asked in, what it was asked for, and its
/// command, running.
struct Opening<R = Running<Opened>> {
    number: u64,
    part: Part,
    running: R,
}

/// An open that answered: its number, what it was asked for, and its answer.
type Answered<T = Response<Opened>> = (u64, Part, T);

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
        part,
        running,
    } in opening
    {
        match try_take(running) {
            Ok(answer) => answered.push((number, part, answer)),
            Err(running) => pending.push(Opening {
                number,
                part,
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
    /// How many untitled parts New has asked for in this run: the next is named by one
    /// more, so no name is given twice in a run, whatever has closed since (macOS's rule).
    untitled: u64,
    /// The untitled parts whose Save panel a quit's Save all has still to open, in turn,
    /// one at a time: let go if the quit stops.
    quit_saves: VecDeque<SessionId>,
    /// Each tab's open number, by its session: the tabs stand in that order, the order their
    /// opens were asked in, whatever order they answered in (each session opens on a thread
    /// of its own); and an open answering after one asked later than it, whose tab is still
    /// open, isn't shown over that one.
    open_numbers: BTreeMap<SessionId, u64>,
    asking: Option<Asking>,
    /// The typed-path Open's field, kept from the last time it was open; and the typed Save
    /// As's, empty each time it opens.
    open_path: String,
    save_as_path: String,
    /// The export dialog's form, while it's open.
    export: Option<ExportForm>,
    /// The shortcut sheet, while it's open (DESIGN.md, state 7).
    sheet: Option<Sheet>,
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
    /// The values each kind of dialog was last OK'd with in this run, in any tab: a new
    /// one of that kind opens with them (SolidWorks' PropertyManager remembers its last
    /// used state for the session).
    remembered: crate::dialog::Remembered,
    /// Times the core's waker asked for a frame.
    wakes: Arc<AtomicU64>,
    waker_set: bool,
    /// Enter or Esc pressed this frame with a modal already open at its start: the modal's
    /// OK or Cancel, taken before anything else could read it.
    modal_key: Option<Key>,
    /// The canvas's place in the window, as the panes last laid it out: where sketch mode's
    /// palette floats.
    canvas: Option<egui::Rect>,
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
    /// The toast shown over the canvas's bottom right (DESIGN.md's *Toasts*), and when it
    /// was shown, by egui's clock: F5's word on where the filters are, or a box's.
    toast: Option<(String, f64)>,
}

/// A row of Show changes' "Their change (on disk)": the changed feature's name in the key
/// column, purple (`.k`, `var(--purple)`), and the field and its values beside it.
fn theirs_row(row: &changes::Row) -> (String, egui::Color32, String) {
    (
        row.feature.clone(),
        theme::CHANGED_OUTSIDE,
        format!("{} · {}", row.field, row.text),
    )
}

/// A row of Show changes' "Your unsaved edits": "Edit" in the key column, and the feature,
/// the field, and its values beside it.
fn mine_row(row: &changes::Row) -> (String, egui::Color32, String) {
    (
        "Edit".to_owned(),
        theme::PROPERTY_KEY,
        format!("{} · {} · {}", row.feature, row.field, row.text),
    )
}

/// One of Show changes' lists (DESIGN.md, 6d), as its own diff answered: "Reading what
/// changed…" while it's asked; its rows (`.prop`, unpadded), each `row` giving its key, the
/// key's colour, and its value, then how many more; or its refusal, said, with its cause
/// under it (the reader's message, for a file on disk that doesn't read as a document).
/// `list` names its rows apart from the other list's.
fn held_list(
    ui: &mut egui::Ui,
    list: &str,
    changes: Option<&Changes>,
    row: fn(&changes::Row) -> (String, egui::Color32, String),
) {
    match changes {
        None | Some(Changes::Asking) => {
            modal::text(ui, "Reading what changed…", theme::MODAL_TEXT);
        }
        Some(Changes::Listed(listed)) => {
            if listed.rows.is_empty() {
                modal::text(ui, "No feature changed.", theme::MODAL_TEXT);
            }
            for (at, change) in listed.rows.iter().enumerate() {
                let (key, colour, value) = row(change);
                properties::list_row(ui, (list, at), &key, colour, &value);
            }
            if let Some(more) = &listed.more {
                modal::text(ui, more, theme::MODAL_TEXT);
            }
        }
        Some(Changes::Refused { said, cause }) => {
            modal::text(ui, said, theme::ERROR_TEXT);
            if let Some(cause) = cause {
                modal::text(ui, cause, theme::ERROR_TEXT);
            }
        }
    }
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
            untitled: 0,
            quit_saves: VecDeque::new(),
            open_numbers: BTreeMap::new(),
            asking: None,
            open_path: String::new(),
            save_as_path: String::new(),
            export: None,
            sheet: None,
            tree: panes::layout(),
            name: None,
            message: None,
            message_from: None,
            quitting: false,
            frames: 0,
            remembered: crate::dialog::Remembered::default(),
            wakes: Arc::new(AtomicU64::new(0)),
            waker_set: false,
            modal_key: None,
            canvas: None,
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
            toast: None,
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

    /// The tab shown, if What's Wrong is open over it (not under the start page).
    fn whats_wrong_shown(&mut self) -> Option<&mut Tab> {
        if self.start_shown() {
            return None;
        }
        let at = self.active_place()?;
        self.tabs.get_mut(at).filter(|tab| tab.whats_wrong)
    }

    /// The documents opened lately, if the app keeps a list.
    pub fn recents(&self) -> Option<&Recents> {
        self.recents.as_ref()
    }

    /// The workspace Copy MCP config names ([`start::workspace`]): the folder of the
    /// document shown, else (none shown, or an untitled part, which has no folder) of the
    /// newest recent one found, else the Documents folder.
    fn workspace(&self) -> Option<PathBuf> {
        let shown = self
            .active_place()
            .and_then(|at| self.tabs.get(at))
            .and_then(|tab| tab.path.as_deref());
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
        let running = self.sessions.open(path, create);
        self.opening(Part::File(path.to_owned()), running);
    }

    /// A new, untitled part (File > New, Command+N, the start page's New part): opened at
    /// once, asking nothing, with no file until its first save (`open` with no path). Named
    /// "Untitled", then "Untitled 2", "Untitled 3", and so on, one more each time, never a
    /// name given before in this run.
    pub fn new_part(&mut self) {
        self.untitled += 1;
        let name = match self.untitled {
            1 => "Untitled".to_owned(),
            n => format!("Untitled {n}"),
        };
        let running = self.sessions.open_untitled();
        self.opening(Part::Untitled(name), running);
    }

    /// The open of `part`, `running`, numbered in the order asked: a tab once it opens.
    fn opening(&mut self, part: Part, running: Running<Opened>) {
        let number = self.next_open;
        self.next_open += 1;
        self.opening.push(Opening {
            number,
            part,
            running,
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
        // The candidate a card's row was hovered on last frame, lit in the canvas from now.
        for tab in &mut self.tabs {
            if tab.take_candidate_hover() {
                ui.ctx().request_repaint();
            }
        }
        // A name being typed, clicked away from: sent before any control the click lands on
        // acts, so it comes first in the tab's queue.
        let renamed = self.rename_clicked_away(ui.ctx());
        let (mut started, dialog_key) = self.keys(ui);
        started |= renamed;
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
        // Over the status bar, across the window's width, under a docked dialog too; drawn
        // before the viewport, which highlights the faces of the feature hovered in it.
        let active = self.active_place();
        crate::timeline::show(
            ui,
            active.and_then(|at| self.tabs.get_mut(at)),
            &self.sessions,
            &mut started,
        );
        // The panes reach the bars and the window's sides, as `#main` fills its grid area.
        let main = egui::Frame::NONE.fill(theme::PANEL_BG);
        egui::CentralPanel::default().frame(main).show(ui, |ui| {
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
                dialog_key,
            };
            panes::fit(tree, ui.available_width());
            tree.ui(&mut panes, ui);
            // Over the canvas, drawn after it, so it takes the pointer over its place.
            let canvas = tree
                .tiles
                .find_pane(&Pane::Viewport)
                .and_then(|viewport| tree.tiles.rect(viewport))
                .unwrap_or_else(|| ui.max_rect());
            // Where sketch mode's panels float.
            self.canvas = Some(canvas);
            self.disk_banner(ui, canvas, &mut started);
            self.show_toast(ui, canvas);
        });
        self.sketch_canvas(ui, &mut started);
        // A toast the tab shown asked for this frame (a box's), drawn from the next; one a tab
        // not shown asked for (its drag ended as it was left) dropped, never shown later out
        // of its context.
        let active = self.active_place();
        let mut asked = None;
        for (at, tab) in self.tabs.iter_mut().enumerate() {
            let toast = tab.take_toast();
            if Some(at) == active {
                asked = toast;
            }
        }
        if let Some(text) = asked {
            self.toast = Some((text, ui.input(|input| input.time)));
            ui.ctx().request_repaint();
        }
        self.after_drawing(ui, started);
    }

    /// The active tab's rename field, if a press of the pointer this frame lands away from
    /// it: its name finished there and then, before the window's controls are drawn, so
    /// what the click does (a dialog opened, a save, an undo) comes after it. Whether the
    /// name was sent.
    fn rename_clicked_away(&mut self, ctx: &egui::Context) -> bool {
        self.send_rename_if(|renaming| crate::feature_menu::clicked_away(ctx, renaming))
    }

    /// The active tab's name being typed in its own field, if `now` says so of it: sent as
    /// Enter sends it ([`Tab::finish_rename`]), its field closed. Whether it was sent.
    fn send_rename_if(&mut self, now: impl FnOnce(&crate::feature_menu::Renaming) -> bool) -> bool {
        let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
            return false;
        };
        if !tab.renaming.as_ref().is_some_and(now) {
            return false;
        }
        let Some(renaming) = tab.renaming.take() else {
            return false;
        };
        tab.finish_rename(&self.sessions, renaming.feature, &renaming.text)
    }

    /// What the start page's `act` asks: a new untitled part, Open's panel, a recent
    /// document opened (or its tab shown, if it's open), or the MCP config copied.
    fn start_act(&mut self, ui: &Ui, act: start::Act, started: &mut bool) {
        match act {
            start::Act::None => {}
            start::Act::New => {
                self.new_part();
                *started = true;
            }
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
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.path.as_deref().is_some_and(same))
        {
            self.active = Some(tab.id);
            self.home = false;
            return false;
        }
        if self
            .opening
            .iter()
            .any(|opening| opening.part.path().is_some_and(same))
        {
            return false;
        }
        self.open(path, false);
        true
    }

    /// The frame's end, the window drawn: what's asked, as a modal; the window closed once
    /// a quit has closed every tab, or kept open while one holds edits; and the next frame
    /// asked for.
    fn after_drawing(&mut self, ui: &mut Ui, mut started: bool) {
        // The dialog OK'd this frame, if one was (one tab's panes draw a frame), remembered.
        for tab in &mut self.tabs {
            if let Some(oked) = tab.take_oked() {
                self.remembered.keep(&oked);
            }
        }
        // The sheet under a question asked over it meanwhile (a quit, a replace).
        if let Some(open) = self.sheet.as_mut()
            && sheet::show(ui.ctx(), open)
        {
            self.sheet = None;
        }
        // What's Wrong, the shown tab's, the same.
        let sessions = Arc::clone(&self.sessions);
        if let Some(tab) = self.whats_wrong_shown() {
            let (asked, hovered) = crate::whats_wrong::show(ui.ctx(), tab);
            tab.hover_candidate(hovered);
            match asked {
                crate::whats_wrong::Asked::None => {}
                crate::whats_wrong::Asked::Close => tab.whats_wrong = false,
                crate::whats_wrong::Asked::Card(asked) => {
                    // Edit and Show in timeline each close it, as the prototype's do; Use and
                    // Remove the edge leave it open (`useCand` and `removeRef` close nothing).
                    if matches!(
                        asked,
                        crate::properties::CardAct::Edit(_) | crate::properties::CardAct::Show(_)
                    ) {
                        tab.whats_wrong = false;
                    }
                    started |= tab.card_asked(&sessions, asked);
                }
            }
        }
        self.ask(ui, &mut started);
        started |= self.next_quit_save(ui.ctx());
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
        let mut refused = Vec::new();
        for tab in &mut self.tabs {
            let path = tab.path.clone();
            tab.poll(&self.sessions);
            // Saved as another file (an untitled part's first save among them): listed in
            // Recent as an open is.
            if tab.path != path
                && let Some(path) = &tab.path
            {
                if let Some(recents) = &self.recents {
                    recents.opened(path);
                }
                // An export form opened while it was untitled: a relative path is in its
                // folder from now on.
                if let Some(form) = self.export.as_mut().filter(|form| form.session == tab.id) {
                    form.saved_as(path);
                }
            }
            // Asked once nothing else is: kept on the tab meanwhile.
            if self.asking.is_none()
                && let Some((path, then_close)) = tab.take_replace_asked()
            {
                self.asking = Some(Asking::Replace {
                    session: tab.id,
                    path,
                    then_close,
                });
            }
            // An export written: its dialog closes.
            if tab.take_exported() && self.asking == Some(Asking::Export(tab.id)) {
                self.asking = None;
                self.export = None;
            }
            if tab.take_close_refused() {
                refused.push(tab.name().to_owned());
            }
        }
        // A tab's save or revert was refused: it stays, holding its edits, and a quit stops.
        for name in refused {
            self.close_refused(&name);
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

    /// A tab's close refused (its save or revert refused, or its Save panel cancelled): a
    /// quit under way stops, said, and the Save panels it had still to open are let go; a
    /// Save panel open is to close its part no more, so a later pick saves it and leaves it
    /// open.
    fn close_refused(&mut self, name: &str) {
        if self.quitting {
            self.quitting = false;
            self.quit_saves.clear();
            if let Some(Browsing {
                what: Browsed::SaveAs { then_close, .. },
                ..
            }) = &mut self.browsing
            {
                *then_close = false;
            }
            self.message = Some(format!("Quit stopped: {name} wasn't closed"));
            self.message_from = None;
        }
    }

    /// The next of a quit's untitled parts to save, once nothing is asked and no panel is
    /// open: its Save panel opened, the part closed once it's saved. One whose Save As (a
    /// panel's pick from before the quit) is queued or running is waited for, and then
    /// closed if that left it with no edits, saved (and closed) if it gave it a file and
    /// edits were made after, or asked again if it's still untitled (its Save As refused,
    /// and said). One gone or already closing is passed over. Whether a command was
    /// started.
    fn next_quit_save(&mut self, ctx: &egui::Context) -> bool {
        while self.quitting && self.asking.is_none() && self.browsing.is_none() {
            let Some(&id) = self.quit_saves.front() else {
                return false;
            };
            let Some(tab) = self
                .tabs
                .iter_mut()
                .find(|tab| tab.id == id && !tab.closing())
            else {
                self.quit_saves.pop_front();
                continue;
            };
            if tab.saving_as() {
                return false;
            }
            self.quit_saves.pop_front();
            if !tab.unsaved() {
                tab.close(&self.sessions);
                return true;
            }
            if !tab.is_untitled() {
                tab.save(&self.sessions, true);
                return true;
            }
            self.ask_save_as(id, true, ctx);
        }
        false
    }

    /// The opens that answered since the last poll, `(number, part, response)` each, taken in
    /// the order they were asked, so what's said comes out the same whether they answered in
    /// one poll or in several.
    fn take_opens(&mut self, answered: Vec<Answered>) {
        for (number, part, response) in answered {
            self.take_open(number, part, response);
        }
    }

    /// An open that answered, taken at once, whatever is still opening: its refusal said, or
    /// its tab put after every tab whose open was asked before it, and shown unless the tab of
    /// an open asked after it is open (a late answer never takes the tab shown). An open asked
    /// after the one whose refusal is said clears it (as one does any other message); one
    /// asked before it, answering late, never does.
    fn take_open(&mut self, number: u64, part: Part, response: Response<Opened>) {
        let opened = match response.result {
            Ok(opened) => opened,
            Err(problems) => {
                self.message = Some(said(&problems));
                self.message_from = Some(number);
                // A recent document's: dropped from the list if nothing is at its path.
                if let Some(path) = part.path()
                    && let Some(recents) =
                        self.recents.as_ref().filter(|recents| recents.holds(path))
                {
                    recents.check(path);
                }
                return;
            }
        };
        // An untitled part isn't listed until its first save gives it a file.
        if let Some(recents) = &self.recents
            && let Some(path) = part.path()
        {
            recents.opened(path);
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
        let tab = match part {
            Part::File(path) => Tab::opened(id, path, &opened.problems),
            Part::Untitled(name) => Tab::untitled(id, name, &opened.problems),
        };
        self.tabs.insert(at, tab);
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
            .is_some_and(|tab| !tab.closing() && !tab.editing())
    }

    /// The place among the tabs of the one shown, if one is.
    fn active_place(&self) -> Option<usize> {
        let active = self.active?;
        self.tabs.iter().position(|tab| tab.id == active)
    }

    /// The keyboard's commands, each by its keys in the registry (`crate::shortcuts`), which
    /// the sheet, the tooltips, and the buttons' caps read too. Ctrl+S saves and
    /// Ctrl+Shift+S asks where to save as (neither while a dialog is open), Ctrl+W closes,
    /// Ctrl+Z undoes and Ctrl+Y or Ctrl+Shift+Z redoes (docs/ux/solidworks.md, §6;
    /// DESIGN.md, *Keyboard*), Esc stops what's running and Ctrl+B rebuilds what was
    /// stopped (§5, *Rebuild*), and H hides or shows the selected body or sketch, outside a
    /// text field. Outside a text field too, `?` opens the shortcut sheet; F6 fits the shown
    /// tab's view, Home gives its home view, and Ctrl+1 to Ctrl+7 its standard views
    /// (SolidWorks' numbering); Ctrl+Shift+B rebuilds everything, ignoring the cache; and,
    /// with a feature selected, F2 types its name over in its own field and Del (or ⌫)
    /// deletes it (the prototype's `keydown`), Esc dropping a name being typed and the
    /// document's commands (Save, Close, Undo, and the rest) sending it before they act.
    /// In sketch mode, Del (or ⌫) deletes the sketch's selection instead, never a feature,
    /// F2 says why it renames nothing (as its menu item is off), and Ctrl+Enter finishes
    /// the sketch.
    /// With a modal open (the sheet too), none acts: Esc is the modal's Cancel (the sheet's
    /// close), and Enter its OK. With a dialog open, Enter and Esc are the dialog's, taken
    /// here and given to it once its inputs have taken this frame's typing (`Panes::dialog`,
    /// in `crate::panes`). Whether a command was started, and the dialog's key.
    fn keys(&mut self, ui: &mut Ui) -> (bool, Option<Key>) {
        self.modal_key = None;
        if self.asking.is_some() {
            // A modal open since the last frame: Esc is its Cancel, and Enter its OK unless
            // a button or a checkbox of its has focus, whose own click Enter then is. Taken
            // only now, so an Enter that opened the modal (on a focused menu item) isn't
            // also its OK.
            let escape = shortcuts::take(ui, Command::Cancel);
            let enter = !escape && enter_is_ok(ui);
            self.modal_key = match (escape, enter) {
                (true, _) => Some(Key::Escape),
                (false, true) => Some(Key::Enter),
                (false, false) => None,
            };
            return (escape || enter, None);
        }
        if self.sheet.is_some() {
            // The sheet is a modal too: Esc closes it, and no other command acts under it.
            if shortcuts::take(ui, Command::Cancel) {
                self.sheet = None;
            }
            return (false, None);
        }
        if let Some(tab) = self.whats_wrong_shown() {
            // What's Wrong too: Esc closes it, and no other command acts under it.
            if shortcuts::take(ui, Command::Cancel) {
                tab.whats_wrong = false;
            }
            return (false, None);
        }
        let renamed = self.rename_before_command(ui.ctx());
        let (started, dialog_key) = self.command_keys(ui);
        (started || renamed, dialog_key)
    }

    /// The commands' shortcuts that act on a document, which send a name being typed in
    /// its own field first, as a click away does (SolidWorks commits a name typed in place
    /// when another command is invoked): Esc drops it instead, and the rest never act
    /// while it's typed.
    const COMMANDS_AFTER_RENAME: [Command; 9] = [
        Command::New,
        Command::Open,
        Command::Save,
        Command::SaveAs,
        Command::Close,
        Command::Undo,
        Command::Redo,
        Command::Rebuild,
        Command::RebuildAll,
    ];

    /// The active tab's rename field, which has had the focus, if a shortcut of
    /// [`COMMANDS_AFTER_RENAME`](Self::COMMANDS_AFTER_RENAME) was pressed this frame: its
    /// name sent there and then, through the path Enter takes, so the command acts on the
    /// renamed document. Whether the name was sent.
    fn rename_before_command(&mut self, ctx: &egui::Context) -> bool {
        let asked = Self::COMMANDS_AFTER_RENAME
            .iter()
            .any(|&command| shortcuts::pressed(ctx, command));
        asked && self.send_rename_if(crate::feature_menu::Renaming::focused)
    }

    /// The keyboard's commands with no modal open: [`keys`](Self::keys)'s, a name being
    /// typed already sent where one asks it.
    fn command_keys(&mut self, ui: &mut Ui) -> (bool, Option<Key>) {
        // Never a letter typed, nor a key a text field moves its cursor with (Home).
        let typing = ui.ctx().text_edit_focused();
        if !typing && shortcuts::take(ui, Command::Shortcuts) {
            self.sheet = Some(Sheet::default());
            return (false, None);
        }
        // Ctrl+N and Ctrl+O: a new untitled part and Open's panel, as the File menu's and
        // the start page's.
        for command in [Command::New, Command::Open] {
            if shortcuts::take(ui, command) {
                if command == Command::New {
                    self.new_part();
                    return (true, None);
                }
                let ctx = ui.ctx().clone();
                self.ask_open(&ctx);
                return (false, None);
            }
        }
        // The start page over the open documents (Home): no document's shortcut acts on the
        // one hidden; Esc and Ctrl+W leave Home for it.
        if self.home {
            let leave = shortcuts::take(ui, Command::Stop) || shortcuts::take(ui, Command::Close);
            if leave {
                self.home = false;
            }
            return (false, None);
        }
        // Before Ctrl+S, which a Ctrl+Shift+S also is, its Shift aside.
        let save_as = shortcuts::take(ui, Command::SaveAs);
        if save_as
            && self.save_as_offered()
            && let Some(id) = self.active
        {
            let ctx = ui.ctx().clone();
            self.ask_save_as(id, false, &ctx);
            return (false, None);
        }
        let in_dialog = self
            .active_place()
            .and_then(|at| self.tabs.get(at))
            .is_some_and(Tab::in_dialog);
        // A feature dialog's OK, as a modal's is (a focused button's Enter its own click).
        let enter = in_dialog && enter_is_ok(ui);
        // Redo before Undo (its Shift+Ctrl+Z first, which egui would match as Ctrl+Z with
        // the Shift aside): a Redo, as Ctrl+Y is, never an Undo. Rebuild everything before
        // Rebuild for the same reason, taken while typing too, so it's never a Rebuild; but
        // done only outside a text field.
        let redo = shortcuts::take(ui, Command::Redo);
        let (save, close, undo) = (
            shortcuts::take(ui, Command::Save),
            shortcuts::take(ui, Command::Close),
            shortcuts::take(ui, Command::Undo),
        );
        let rebuild_all = shortcuts::take(ui, Command::RebuildAll) && !typing;
        let rebuild = shortcuts::take(ui, Command::Rebuild);
        // Esc with a menu open (a feature's right-click menu, a drop-down) closes the menu
        // alone, as the prototype's `keydown` has it: left for the menu to take.
        let stop = !egui::Popup::is_any_open(ui.ctx()) && shortcuts::take(ui, Command::Stop);
        // H hides or shows the selection (DESIGN.md, *Keyboard*), never a letter typed.
        let hide = !typing && shortcuts::take(ui, Command::Hide);
        // F2 renames the selected feature, and Del (or ⌫) deletes it: never a field's keys.
        // In sketch mode Del (or ⌫) is the sketch's, below.
        let rename = !typing && shortcuts::take(ui, Command::Rename);
        let delete = !typing && shortcuts::take(ui, Command::Delete);
        // The view's keys, never a field's (Home moves its cursor).
        let view = if typing {
            None
        } else {
            [
                (Command::Fit, ViewAsk::Fit),
                (Command::Home, ViewAsk::Named(crate::navigate::HOME)),
                (Command::Front, ViewAsk::Named(NamedView::Front)),
                (Command::Back, ViewAsk::Named(NamedView::Back)),
                (Command::Left, ViewAsk::Named(NamedView::Left)),
                (Command::Right, ViewAsk::Named(NamedView::Right)),
                (Command::Top, ViewAsk::Named(NamedView::Top)),
                (Command::Bottom, ViewAsk::Named(NamedView::Bottom)),
                (Command::Isometric, ViewAsk::Named(ISOMETRIC)),
            ]
            .into_iter()
            // Each taken, the last pressed given.
            .fold(None, |asked, (command, ask)| {
                if shortcuts::take(ui, command) {
                    Some(ask)
                } else {
                    asked
                }
            })
        };
        let in_sketch = self
            .active_place()
            .and_then(|at| self.tabs.get(at))
            .is_some_and(Tab::in_sketch);
        // Ctrl+Enter (⌘↵) finishes the sketch (the prototype's key, as the toolbar's Finish
        // Sketch names it).
        let finish = in_sketch
            && ui.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter))
            });
        // Del (or ⌫) in sketch mode deletes the sketch's selection, never a feature (the
        // prototype's `keydown` gives a sketch its own keys and goes no further).
        let (sketch_delete, delete) = (delete && in_sketch, delete && !in_sketch);
        // SEAM(#351): `in_sketch` once the sketch's own keys are in (the wiring's X is
        // Construction); until then the filters are taken in a sketch too.
        let filters = if typing {
            Vec::new()
        } else {
            filter_keys(ui, false)
        };
        // F5: where the filters are (the prototype's, no command of the registry's).
        let filters_said =
            !typing && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F5));
        let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
            return (false, None);
        };
        if hide {
            tab.toggle_selected_shown();
        }
        for kind in filters {
            let selection = tab.selection_mut();
            selection.filter = kind.toggled(selection.filter);
        }
        if filters_said {
            // The toolbar shows SOLID, whose SELECT panel holds the filters, whenever no
            // sketch is open: there's no other tab to switch from (a sketch's SKETCH is its
            // mode's, left by Finish or Cancel).
            self.toast = Some((FILTERS_SAID.to_owned(), ui.input(|input| input.time)));
        }
        let selected = tab.selection().feature().cloned();
        // Each only where its menu item would be on: else its reason is said, as the item's
        // hover says it.
        let place = tab.selected_in;
        let can = |tab: &Tab, id: &silico_core::document::FeatureId, item| {
            let entry = tab.entry(id.as_str())?;
            Some(crate::feature_menu::off(tab, entry, item, place))
        };
        if rename && let Some(id) = selected.clone() {
            match can(tab, &id, crate::feature_menu::Item::Rename) {
                Some(None) => tab.start_rename(id, place),
                Some(Some(why)) => tab.message = Some(why),
                None => {}
            }
        }
        let mut deleted = false;
        if delete && let Some(id) = selected {
            match can(tab, &id, crate::feature_menu::Item::Delete) {
                Some(None) => {
                    tab.remove_feature(&self.sessions, id);
                    deleted = true;
                }
                Some(Some(why)) => tab.message = Some(why),
                None => {}
            }
        }
        // Esc with a feature's name being typed: the name dropped, nothing sent, and nothing
        // further down Esc's chain done (the field lost its focus to the Esc already).
        let stop = if stop && tab.renaming.is_some() {
            tab.renaming = None;
            false
        } else {
            stop
        };
        if let Some(ask) = view {
            tab.view.ask(ask);
        }
        let sessions = &self.sessions;
        // An untitled part's Save asks where, through the Save panel, once the tab is let go:
        // with no edits too, as its first save writes the document all the same.
        let mut save_panel = None;
        if save && tab.is_untitled() {
            // Not while a Save As of its runs: its answer gives it its file.
            if !tab.editing() && !tab.closing() && !tab.saving_as() {
                save_panel = Some(tab.id);
            }
        } else if save && tab.unsaved() && !tab.editing() {
            tab.save(sessions, false);
        }
        if sketch_delete {
            tab.sketch_delete(sessions);
        }
        if finish {
            tab.finish_sketch(sessions, true);
        }
        if undo && tab.undoable() {
            tab.step(sessions, true);
        }
        if redo && tab.redoable() {
            tab.step(sessions, false);
        }
        if rebuild_all {
            tab.rebuild_all(sessions);
        }
        if rebuild {
            tab.rebuild(sessions);
        }
        let dialog_key = match (enter, stop && in_dialog) {
            (true, _) => Some(Key::Enter),
            (false, true) => Some(Key::Escape),
            (false, false) => None,
        };
        // Esc, with no dialog open: the reload's banner dismissed first, then the selection
        // cleared, then a rebuild running stopped, as the stop does (section 3's order).
        if stop && !in_dialog {
            tab.escape(sessions);
        }
        if let Some(id) = save_panel {
            let ctx = ui.ctx().clone();
            self.ask_save_as(id, false, &ctx);
        }
        if close && let Some(id) = self.active {
            self.close(id);
        }
        (
            save || undo
                || redo
                || stop
                || close
                || enter
                || rebuild
                || rebuild_all
                || deleted
                || sketch_delete
                || finish,
            dialog_key,
        )
    }

    /// The toast, if one is shown, over `canvas`'s bottom right, until
    /// [`theme::TOAST_SECONDS`] have passed since it was shown; a frame asked for then, to
    /// take it down.
    fn show_toast(&mut self, ui: &Ui, canvas: egui::Rect) {
        let Some((text, at)) = &self.toast else {
            return;
        };
        let now = ui.input(|input| input.time);
        let left = theme::TOAST_SECONDS - (now - at);
        if left.is_nan() || left <= 0.0 {
            self.toast = None;
            return;
        }
        toast(ui.ctx(), canvas, text);
        ui.ctx()
            .request_repaint_after(Duration::from_secs_f64(left));
    }

    /// The toast shown now, if one is, and when it was shown, by egui's clock.
    pub fn toast(&self) -> Option<(&str, f64)> {
        self.toast.as_ref().map(|(text, at)| (text.as_str(), *at))
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
    /// opened; the Save panel's pick saved as ([`saved_as`](Self::saved_as)); a path picked
    /// through Browse… filling the path field of the question it was asked from (the
    /// typed-path Open's opens it), if that question is still asked; a cancel, or a question
    /// no longer asked, changes nothing (but a cancelled Save panel that was to close its
    /// tab refuses the close), nor does Export's while its export runs or asks whether to
    /// replace. Whether a command was started.
    fn take_browsed(&mut self) -> bool {
        let Some(answer) = self.browsing.as_mut().and_then(Browsing::poll) else {
            return false;
        };
        let Some(Browsing { what, .. }) = self.browsing.take() else {
            return false;
        };
        let Some(path) = answer else {
            if let Browsed::SaveAs {
                session,
                then_close: true,
            } = what
            {
                self.save_panel_refused(session);
            }
            return false;
        };
        if let Some(folder) = path
            .parent()
            .filter(|folder| !folder.as_os_str().is_empty())
        {
            self.last_folder = Some(folder.to_path_buf());
        }
        let path = match path.into_os_string().into_string() {
            Ok(path) => path,
            Err(path) => {
                self.message = Some(format!(
                    "{} can't be used: its path isn't UTF-8",
                    std::path::Path::new(&path).display()
                ));
                self.message_from = None;
                if let Browsed::SaveAs {
                    session,
                    then_close: true,
                } = what
                {
                    self.save_panel_refused(session);
                }
                return false;
            }
        };
        let what = match what {
            Browsed::Field(what) if self.asking.as_ref() == Some(&what) => what,
            Browsed::Field(_) => return false,
            Browsed::Open => {
                // Open's panel: the file picked opened, as Browse…'s pick in the typed-path
                // Open is, its tab and the recent documents following once it answers.
                self.open(&path, false);
                return true;
            }
            Browsed::SaveAs {
                session,
                then_close,
            } => return self.saved_as(session, path, then_close),
        };
        match what {
            Asking::Open => {
                self.open(&path, false);
                // As a typed path's open takes it.
                self.open_path.clear();
                self.asking = None;
                return true;
            }
            Asking::SaveAs { .. } => self.save_as_path = path,
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
            Asking::Close(_) | Asking::Quit | Asking::Replace { .. } | Asking::Changes(_) => {}
        }
        false
    }

    /// The Save panel's pick, `path`, for `session`'s document: saved as it, `.silico`
    /// added if its name hasn't it ([`browse::document_path`]) (`commit` with the path, and
    /// `replace` if the panel asked about a file there: if it was picked as it is, but not
    /// if `.silico` was added, so a file by that name is asked about), then closed if
    /// `then_close`, if its tab is still open and not closing (Don't save chosen while a
    /// panel of the portal's was open); else nothing. Its own file picked is a Save (the core
    /// refuses a Save As to it, and the tab saves instead). Whether a command was started.
    fn saved_as(&mut self, session: SessionId, path: String, then_close: bool) -> bool {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == session && !tab.closing())
        else {
            return false;
        };
        let (path, added) = browse::document_path(path);
        // Its own file picked (by any path to it) is refused by the core as its own, which
        // the tab then takes as a Save.
        tab.save_as(&self.sessions, path, !added, then_close);
        true
    }

    /// `session`'s Save panel, which was to close its tab, cancelled (or its pick unusable):
    /// the close refused, so a quit stops.
    fn save_panel_refused(&mut self, session: SessionId) {
        if let Some(name) = self
            .tabs
            .iter()
            .find(|tab| tab.id == session)
            .map(|tab| tab.name().to_owned())
        {
            self.close_refused(&name);
        }
    }

    /// The Browse… button beside a path field, if the app has native dialogs: off while one
    /// is open.
    fn browse_button(&self) -> Option<modal::Button<'static>> {
        self.browser
            .as_ref()
            .map(|_| browse::button(self.browsing.is_none()))
    }

    /// Open (File > Open…, Command+O, the start page's Open…): the platform's own open panel
    /// at once, the file picked opened when it answers ([`take_browsed`](Self::take_browsed)),
    /// a cancel doing nothing. With no native dialogs, or one that won't open, or while
    /// another native dialog is open (a Browse…'s), the typed-path Open asked instead; while
    /// Open's own panel is open, nothing.
    fn ask_open(&mut self, ctx: &egui::Context) {
        let typed = Asking::Open;
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

    /// Save As (File > Save As…, Command+Shift+S), and an untitled part's Save (Command+S,
    /// the app bar's Save, and Close's and Quit's, which then close it, `then_close`): the
    /// platform's own Save panel for `session`'s document at once, which saves it as the
    /// file picked when it answers ([`saved_as`](Self::saved_as)). With no native dialogs,
    /// or one that won't open, or while another native dialog is open, the typed Save As
    /// asked instead; while this document's own Save panel is open, nothing more (but a
    /// close asked meanwhile is taken on by it).
    fn ask_save_as(&mut self, session: SessionId, then_close: bool, ctx: &egui::Context) {
        let typed = Asking::SaveAs {
            session,
            then_close,
        };
        match &mut self.browsing {
            Some(Browsing {
                what:
                    Browsed::SaveAs {
                        session: open,
                        then_close: closes,
                    },
                ..
            }) if *open == session => {
                *closes |= then_close;
                return;
            }
            Some(_) => {
                self.save_as_path.clear();
                self.asking = Some(typed);
                return;
            }
            None => {}
        }
        let Some(ask) = self.ask_of(&typed) else {
            return;
        };
        let panel = Browsed::SaveAs {
            session,
            then_close,
        };
        if !self.start_browsing(panel, &ask, ctx) {
            self.save_as_path.clear();
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
            Asking::Open => {
                let shown = self.active_place().and_then(|at| self.tabs.get(at));
                Ask {
                    pick: Pick::Open,
                    title: "Open document".to_owned(),
                    filters: vec![browse::DOCUMENTS],
                    folder: folder(shown.and_then(|tab| tab.path.as_deref())),
                    file_name: None,
                }
            }
            Asking::SaveAs { session, .. } => {
                let tab = tab(*session)?;
                Ask {
                    pick: Pick::Save,
                    title: format!("Save {} as", tab.name()),
                    filters: vec![browse::DOCUMENTS],
                    folder: folder(tab.path.as_deref()),
                    // An untitled part's: its name, a document's.
                    file_name: match &tab.path {
                        Some(path) => browse::file_name_of(path),
                        None => Some(format!("{}.silico", tab.name())),
                    },
                }
            }
            Asking::Export(id) => {
                let tab = tab(*id)?;
                let form = self.export.as_ref().filter(|form| form.session == *id)?;
                // An untitled part's: the folder of the path in its field, if that's a full
                // one (it has no folder of its own).
                let typed = std::path::Path::new(&form.path)
                    .is_absolute()
                    .then_some(form.path.as_str());
                Ask {
                    pick: Pick::Save,
                    title: format!("Export {}", tab.name()),
                    filters: browse::export_filters(form.step),
                    folder: folder(tab.path.as_deref().or(typed)),
                    file_name: browse::file_name_of(&form.path),
                }
            }
            Asking::Close(_) | Asking::Quit | Asking::Replace { .. } | Asking::Changes(_) => {
                return None;
            }
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
        // Off while a dialog or a sketch is open: its OK or Finish, or its Cancel, ends its
        // edits first. On for an untitled part with no edits too, its first save writing it,
        // but not while it closes or a Save As of its runs.
        let unsaved = tab.as_ref().is_some_and(|tab| {
            !tab.editing()
                && if tab.is_untitled() {
                    !tab.closing() && !tab.saving_as()
                } else {
                    tab.unsaved()
                }
        });
        let said = tab.as_ref().filter(|_| unsaved).map(|tab| match tab.path {
            Some(_) => format!("Writes your edits to {}", tab.name()),
            None => format!("Asks where to save {}, then writes it", tab.name()),
        });
        let save = bar::button(ui, Icon::Save, None, Command::Save.label(), unsaved, None);
        let shortcut = Command::Save.shown(ui.ctx());
        if bar::tooltip(save, Command::Save.label(), shortcut, said.as_deref()).clicked()
            && let Some(tab) = tab
        {
            // An untitled part's asks where, through the Save panel.
            if tab.is_untitled() {
                let (id, ctx) = (tab.id, ui.ctx().clone());
                self.ask_save_as(id, false, &ctx);
            } else {
                tab.save(&self.sessions, false);
                *started = true;
            }
        }
        let (undoable, redoable) = self
            .active_place()
            .filter(|_| !start_shown)
            .and_then(|at| self.tabs.get(at))
            .map_or((false, false), |tab| (tab.undoable(), tab.redoable()));
        // Redo's shortcut as the prototype names it: Shift+Command+Z on macOS, Ctrl+Y
        // elsewhere (each works on both): the registry's.
        for (undo, icon, command, enabled) in [
            (true, Icon::Undo, Command::Undo, undoable),
            (false, Icon::Redo, Command::Redo, redoable),
        ] {
            let word = command.label();
            let button = bar::button(ui, icon, None, word, enabled, None);
            if bar::tooltip(button, word, command.shown(ui.ctx()), None).clicked()
                && let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at))
            {
                tab.step(&self.sessions, undo);
                *started = true;
            }
        }
    }

    /// The File menu's items: New, Open…, Save As…, Export…, Revert, and Close.
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
        // New asks nothing: an untitled part at once.
        if ui.button("New").clicked() {
            self.new_part();
            *started = true;
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
                let ctx = ui.ctx().clone();
                self.ask_save_as(id, false, &ctx);
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
                self.export = Some(match &tab.path {
                    Some(path) => ExportForm::new(tab.id, path),
                    None => ExportForm::untitled(tab.id, tab.name(), self.documents.as_deref()),
                });
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
                        path: tab.path.as_deref(),
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
                    in_sketch: shown.is_some_and(Tab::in_sketch),
                    create_sketch: Some(shown.is_some_and(Tab::can_sketch)),
                    can_open: &can_open,
                    selected: shown
                        .is_some_and(|tab| !tab.selection().is_empty() && !tab.in_dialog()),
                    filter: shown.map_or_else(crate::selection::Filter::default, |tab| {
                        tab.selection().filter
                    }),
                },
            )
        };
        let Some(tab) = tab.as_mut() else {
            return;
        };
        match act {
            Some(toolbar::Act::Dialog(kind)) => {
                tab.open_dialog(&self.sessions, kind, &self.remembered);
                *started = true;
            }
            Some(toolbar::Act::ClearSelection) => tab.selection_mut().clear(),
            Some(toolbar::Act::Filter(kind)) => {
                let selection = tab.selection_mut();
                selection.filter = kind.toggled(selection.filter);
            }
            Some(toolbar::Act::AllFilters) => {
                tab.selection_mut().filter = crate::selection::Filter::default();
            }
            Some(toolbar::Act::CreateSketch(plane)) => {
                tab.create_sketch(&self.sessions, plane);
                *started = true;
            }
            Some(toolbar::Act::FinishSketch) => {
                tab.finish_sketch(&self.sessions, true);
                *started = true;
            }
            None => {}
        }
    }

    /// The status bar (`crate::status`): the rebuild (`status.rebuild`) or the session's own
    /// reload, the stop button (`cancel` naming the rebuild, on while it runs, whatever
    /// command runs: section 3, *Stop*), Rebuild, the last refusal, the file on disk (its
    /// chip offering again the choice kept), whether the document is saved, and Shortcuts,
    /// which opens the shortcut sheet.
    fn status_bar(&mut self, ui: &mut Ui, started: &mut bool) {
        let shortcuts = egui::Panel::bottom("status_bar")
            .exact_size(theme::STATUS_BAR_HEIGHT)
            .resizable(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let message = self.message.clone();
                let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
                    // No document: how the warm start stands (decision 10).
                    let warming = self.warm.as_ref().map(Warm::state);
                    let asked = crate::status::bar(ui, &crate::status::unopened(warming, message));
                    return asked.shortcuts;
                };
                let asked = crate::status::bar(ui, &crate::status::of(tab, message));
                if asked.stop {
                    tab.stop(&self.sessions);
                    *started = true;
                }
                if asked.rebuild {
                    tab.rebuild(&self.sessions);
                    *started = true;
                }
                if asked.whats_wrong {
                    tab.whats_wrong = true;
                }
                if asked.disk
                    && tab.disk_choice == DiskChoice::Kept
                    && let Some(disk) = tab.status.as_ref().and_then(|status| status.disk)
                {
                    // The edits kept over a file changed on disk: the choice offered again.
                    tab.disk_choice = DiskChoice::Offered(disk);
                }
                asked.shortcuts
            })
            .inner;
        if shortcuts {
            self.sheet = Some(Sheet::default());
        }
    }

    /// Sketch mode's panels in the canvas while a sketch is open in the tab shown (the
    /// prototype's): the sketch bar at the canvas's top centre, its Finish Sketch and Cancel
    /// taken, and the Sketch Palette at its right.
    fn sketch_canvas(&mut self, ui: &mut Ui, started: &mut bool) {
        let sessions = &self.sessions;
        let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
            return;
        };
        let name = tab.sketch_name();
        let (revision, enabled, id) = (tab.revision, !tab.closing(), tab.id);
        let Some(mode) = tab.sketch.as_mut() else {
            return;
        };
        let canvas = self.canvas.unwrap_or_else(|| ui.ctx().content_rect());
        let bar = mode.bar_view(&name, enabled);
        let view = mode.palette_view(revision);
        let mut action = BarAction::None;
        egui::Area::new(egui::Id::new(("sketch_bar", id.to_string())))
            .fixed_pos(canvas.center_top() + egui::vec2(0.0, SKETCH_BAR_FROM_TOP))
            .pivot(egui::Align2::CENTER_TOP)
            .show(ui.ctx(), |ui| action = sketch::bar::sketch_bar(ui, &bar));
        // Pinned at the canvas's top right, under the ViewCube, as the prototype's is.
        egui::Area::new(egui::Id::new(("sketch_palette", id.to_string())))
            .fixed_pos(canvas.right_top() + PALETTE_FROM_CORNER)
            .show(ui.ctx(), |ui| {
                sketch::bar::palette(ui, &view, &mut mode.settings)
            });
        match action {
            BarAction::None => {}
            BarAction::Finish => tab.finish_sketch(sessions, true),
            BarAction::Cancel => tab.finish_sketch(sessions, false),
        }
        *started |= action != BarAction::None;
    }

    /// The banner over the canvas (section 4; DESIGN.md's 6c and 6d): a file changed on
    /// disk while edits are held, amber (gone, red), offering Reload (`rollback`, after which
    /// the session reloads it) or to keep the edits (a commit is then a conflict); or else,
    /// once the session has reloaded its file with nothing held, purple, saying so until
    /// it's dismissed.
    fn disk_banner(&mut self, ui: &mut Ui, canvas: egui::Rect, started: &mut bool) {
        let Some(tab) = self.active_place().and_then(|at| self.tabs.get_mut(at)) else {
            return;
        };
        if let DiskChoice::Offered(disk) = tab.disk_choice {
            // The newer news: the file changed again, with edits held now.
            tab.reloaded = None;
            let (tone, what, sub) = match disk {
                Disk::Gone => (
                    Tone::Error,
                    "is gone from disk",
                    &[(
                        "The part is still open here, with your edits: keep them, or reload, \
                         which drops them.",
                        Style::Plain,
                    )][..],
                ),
                _ => (
                    Tone::Warning,
                    "changed on disk",
                    &[
                        (
                            "Another program wrote the file: an agent through ",
                            Style::Plain,
                        ),
                        ("silico serve", Style::Mono),
                        (
                            ", git, or an editor. Your edits are safe here, and Save won't \
                             write over the change.",
                            Style::Plain,
                        ),
                    ][..],
                ),
            };
            let title = format!("{} {what} while you have unsaved edits.", tab.name());
            // Reload is off while the tab closes: its close drops the edits already. Show
            // changes goes first, as the prototype has it, for a file changed, not gone.
            let reload = ("Reload, discard my edits", !tab.closing());
            let keep = ("Keep my edits…", true);
            let changed = [("Show changes", tab.transaction().is_some()), reload, keep];
            let gone = [reload, keep];
            let buttons: &[(&str, bool)] = match disk {
                Disk::Gone => &gone,
                _ => &changed,
            };
            let clicked = banner::show(
                ui,
                canvas,
                &Banner {
                    tone,
                    title: &title,
                    sub,
                    list: None,
                    buttons,
                    tips: &[],
                    dismiss: false,
                },
            );
            let clicked = match clicked {
                banner::Clicked::Button(at) => buttons.get(at).map(|(words, _)| *words),
                banner::Clicked::None | banner::Clicked::Dismiss => None,
            };
            match clicked {
                Some("Show changes") => {
                    tab.show_changes(&self.sessions);
                    self.asking = Some(Asking::Changes(tab.id));
                    *started = true;
                }
                Some("Reload, discard my edits") => {
                    tab.revert(&self.sessions, false);
                    *started = true;
                }
                Some(_) => tab.disk_choice = DiskChoice::Kept,
                None => {}
            }
            return;
        }
        let Some(reloaded) = tab.reloaded.clone() else {
            return;
        };
        let title = format!(
            "Reloaded {}: another program changed it on disk",
            tab.name()
        );
        let sub = format!(
            "Revision {} → {}. You had no unsaved edits, so nothing of yours was lost. \
             Changed features are marked ",
            reloaded.from.number(),
            reloaded.to.number()
        );
        // What changed (`.diff`), as the reload's diff answered: nothing while it's asked;
        // its refusal, said; then the earlier reloads whose changes it replaced unlisted;
        // and Restore's refusal, if it was refused. A line each.
        let (rows, more): (&[crate::changes::Row], Option<&str>) = match &reloaded.changes {
            Changes::Asking => (&[], None),
            Changes::Listed(listed) => (&listed.rows, listed.more.as_deref()),
            Changes::Refused { said, .. } => (&[], Some(said.as_str())),
        };
        let unlisted = changes::unlisted(&reloaded.unlisted);
        let after: Vec<&str> = [more, unlisted.as_deref(), reloaded.refused.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        let list = (!rows.is_empty() || !after.is_empty()).then_some(banner::List {
            rows,
            after: &after,
        });
        // Named by the revision it puts back (Fable's decision of 2026-11-04, section 2:
        // never "my version", which may be another program's earlier save). Off while a
        // dialog or a sketch is open, the tab closes, a Restore is under way, once it was
        // refused, or once the session is known not to hold that revision (said, as
        // Restore's read could only be refused).
        let restore = format!("Restore revision {}", reloaded.from.number());
        let gone = format!(
            "Silico no longer holds revision {}: the file changed again before what changed \
             was read, so it can't be put back",
            reloaded.from.number()
        );
        let on = !tab.editing()
            && !tab.closing()
            && !tab.restoring()
            && reloaded.refused.is_none()
            && reloaded.from_held;
        let clicked = banner::show(
            ui,
            canvas,
            &Banner {
                tone: Tone::Info,
                title: &title,
                sub: &[
                    (&sub, Style::Plain),
                    ("●", Style::Dot(theme::CHANGED_OUTSIDE)),
                    (" until you look at them.", Style::Plain),
                ],
                list,
                buttons: &[(&restore, on)],
                tips: &[(!reloaded.from_held).then_some(gone.as_str())],
                dismiss: true,
            },
        );
        match clicked {
            banner::Clicked::Dismiss => tab.reloaded = None,
            banner::Clicked::Button(_) => {
                tab.restore(&self.sessions, reloaded.from);
                *started = true;
            }
            banner::Clicked::None => {}
        }
    }

    /// What the app is asking, as a modal (`modal`): a path, or whether to save.
    fn ask(&mut self, ui: &mut Ui, started: &mut bool) {
        let Some(asking) = self.asking.clone() else {
            return;
        };
        // Enter and Esc are the dialog's own, as keys took them: its OK and its Cancel
        // (section 3).
        let key = self.modal_key.take();
        let (enter, escape) = (key == Some(Key::Enter), key == Some(Key::Escape));
        // OK's key cap, as the registry names it.
        let enter_cap = Command::Ok.keys_text(ui.ctx());
        let mut done = false;
        let mut browse = false;
        // Show changes' Save mine as…: a Save As of the edits held, once the modal has gone.
        let mut save_mine = None;
        // An untitled part's Save, asked by Close or Quit, through its Save panel once the
        // question has gone; and a close refused (a typed Save As cancelled that was to close).
        let mut save_panel = None;
        let mut refused = None;
        modal::show(ui.ctx(), egui::Id::new("asking"), |ui| match asking {
            Asking::Open => {
                modal::head(ui, Some(Icon::Open), "Open document");
                let beside = self.browse_button();
                modal::body(ui, |ui| {
                    let label = modal::label(ui, "Path");
                    let edit = egui::TextEdit::singleline(&mut self.open_path);
                    let edit = theme::field(ui, edit, "open-path", FieldKind::Modify, false);
                    let (_, button) = modal::field(ui, edit, label.id, beside);
                    browse = button.is_some_and(browse::clicked);
                });
                let ([], [cancel, ok]) = modal::foot(
                    ui,
                    [],
                    [
                        Button::new("Cancel", Kind::Secondary),
                        Button::new("Open", Kind::Primary).cap(&enter_cap),
                    ],
                );
                // The path as typed; only an empty one (spaces aside) opens nothing.
                if (ok || enter) && !self.open_path.trim().is_empty() {
                    let path = std::mem::take(&mut self.open_path);
                    self.open(&path, false);
                    *started = true;
                    done = true;
                }
                if cancel || escape {
                    done = true;
                }
            }
            Asking::SaveAs {
                session: id,
                then_close,
            } => {
                let name = self
                    .tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .map(|tab| tab.name().to_owned());
                // An untitled part's path must be a full one: it has no folder.
                let document = self
                    .tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .map(|tab| tab.path.clone());
                let typed = self.save_as_path.trim();
                let no_folder = !typed.is_empty()
                    && document.as_ref().is_some_and(|document| {
                        export::beside(document.as_deref(), &self.save_as_path).is_none()
                    });
                let title = format!("Save {} as", name.as_deref().unwrap_or("the document"));
                modal::head(ui, Some(Icon::Save), &title);
                let beside = self.browse_button();
                modal::body(ui, |ui| {
                    let label = modal::label(ui, "Path");
                    let edit = egui::TextEdit::singleline(&mut self.save_as_path);
                    let edit = theme::field(ui, edit, "save-as-path", FieldKind::Modify, false);
                    let (_, button) = modal::field(ui, edit, label.id, beside);
                    browse = button.is_some_and(browse::clicked);
                    if no_folder {
                        ui.add_space(theme::MODAL_LABEL_MARGIN_TOP);
                        modal::text(ui, export::NO_FOLDER, theme::ERROR_TEXT);
                    }
                });
                let ([], [cancel, save]) = modal::foot(
                    ui,
                    [],
                    [
                        Button::new("Cancel", Kind::Secondary),
                        Button::new("Save", Kind::Primary).cap(&enter_cap),
                    ],
                );
                if (save || enter) && !self.save_as_path.trim().is_empty() && !no_folder {
                    let typed = std::mem::take(&mut self.save_as_path);
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
                        // A relative path is in the document's folder.
                        && let Some(path) = export::beside(tab.path.as_deref(), &typed)
                    {
                        // A document's name: `.silico` added if it hasn't it. A file there
                        // by that name is asked about.
                        let (path, _) = browse::document_path(path.to_string_lossy().into_owned());
                        tab.save_as(&self.sessions, path, false, then_close);
                    }
                    *started = true;
                    done = true;
                }
                if cancel || escape {
                    done = true;
                    if then_close {
                        refused = Some(id);
                    }
                }
            }
            Asking::Changes(id) => {
                let sessions = &self.sessions;
                let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
                    done = true;
                    return;
                };
                modal::head(ui, Some(Icon::Diff), "What changed on disk");
                let held = tab.held_changes.as_ref();
                modal::body(ui, |ui| {
                    // The prototype's rows (`.prop`, unpadded), each list from its own diff.
                    modal::label(ui, "Their change (on disk)");
                    held_list(ui, "theirs", held.map(|held| &held.theirs), theirs_row);
                    modal::label(ui, "Your unsaved edits");
                    held_list(ui, "mine", held.map(|held| &held.mine), mine_row);
                    // `.hint-block`, in Silico's words: the prototype's names `silico
                    // textconv`, but these rows are the core's `diff`'s.
                    properties::hint_at(
                        ui,
                        &[(
                            "A feature-level summary of the two documents, as Silico's diff \
                             reads them.",
                            false,
                        )],
                        theme::MODAL_HINT_MARGIN,
                    );
                });
                let ([], [close, reload, save_as]) = modal::foot(
                    ui,
                    [],
                    [
                        Button::new("Close", Kind::Secondary),
                        Button::new("Reload theirs", Kind::Secondary).enabled(!tab.closing()),
                        Button::new("Save mine as…", Kind::Primary),
                    ],
                );
                if reload {
                    tab.revert(sessions, false);
                    *started = true;
                    done = true;
                }
                if save_as {
                    save_mine = Some(id);
                    done = true;
                }
                if close || escape {
                    done = true;
                }
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
                modal::head(ui, None, &format!("Export {}", tab.name()));
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
            Asking::Replace {
                session: id,
                path,
                then_close,
            } => {
                modal::head(ui, Some(Icon::Warn), "Replace the file?");
                modal::body(ui, |ui| {
                    modal::text(
                        ui,
                        &format!("{path} already exists. Replace it?"),
                        theme::MODAL_TEXT,
                    );
                });
                let ([], [cancel, replace]) = modal::foot(
                    ui,
                    [],
                    [
                        Button::new("Cancel", Kind::Secondary),
                        Button::new("Replace", Kind::Danger),
                    ],
                );
                if replace {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                        tab.save_as(&self.sessions, path.clone(), true, then_close);
                    }
                    *started = true;
                    done = true;
                }
                if cancel || escape {
                    done = true;
                    if then_close {
                        refused = Some(id);
                    }
                }
            }
            Asking::Close(id) => {
                let name = self
                    .tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .map(|tab| tab.name().to_owned());
                let title = format!(
                    "Save changes to {} before closing?",
                    name.as_deref().unwrap_or("the document")
                );
                modal::head(ui, Some(Icon::Save), &title);
                modal::body(ui, |ui| {
                    modal::text(
                        ui,
                        "It has unsaved edits. Closing without saving drops them.",
                        theme::MODAL_TEXT,
                    );
                });
                let ([drop], [cancel, save]) = modal::foot(
                    ui,
                    [Button::new("Don't save", Kind::Danger)],
                    [
                        Button::new("Cancel", Kind::Secondary),
                        Button::new("Save", Kind::Primary),
                    ],
                );
                let sessions = &self.sessions;
                if save {
                    match self.tabs.iter_mut().find(|tab| tab.id == id) {
                        // An untitled part's asks where first.
                        Some(tab) if tab.is_untitled() => save_panel = Some(id),
                        Some(tab) => tab.save(sessions, true),
                        None => {}
                    }
                    *started = true;
                    done = true;
                }
                if drop {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                        tab.revert(sessions, true);
                    }
                    *started = true;
                    done = true;
                }
                if cancel || escape {
                    done = true;
                }
            }
            Asking::Quit => {
                let names: Vec<&str> = self
                    .tabs
                    .iter()
                    .filter(|tab| tab.unsaved())
                    .map(Tab::name)
                    .collect();
                modal::head(ui, Some(Icon::Save), "Save changes before quitting?");
                modal::body(ui, |ui| {
                    modal::text(
                        ui,
                        &format!(
                            "Unsaved: {}. Quitting without saving drops their edits.",
                            names.join(", ")
                        ),
                        theme::MODAL_TEXT,
                    );
                });
                let ([drop], [cancel, save]) = modal::foot(
                    ui,
                    [Button::new("Don't save", Kind::Danger)],
                    [
                        Button::new("Cancel", Kind::Secondary),
                        Button::new("Save all", Kind::Primary),
                    ],
                );
                let sessions = &self.sessions;
                if save || drop {
                    self.quit_saves.clear();
                    for tab in &mut self.tabs {
                        match (tab.unsaved(), save) {
                            // Its own Save panel open already (Command+S, the portal's
                            // leaving the window usable): that panel closes it, as Close's
                            // Save would have it.
                            (true, true)
                                if tab.is_untitled()
                                    && matches!(
                                        &self.browsing,
                                        Some(Browsing {
                                            what: Browsed::SaveAs { session, .. },
                                            ..
                                        }) if *session == tab.id
                                    ) =>
                            {
                                if let Some(Browsing {
                                    what: Browsed::SaveAs { then_close, .. },
                                    ..
                                }) = &mut self.browsing
                                {
                                    *then_close = true;
                                }
                            }
                            // An untitled part's Save panel, each in turn, after this
                            // question has gone ([`next_quit_save`]); one whose Save As
                            // runs waits for it.
                            (true, true) if tab.is_untitled() => {
                                self.quit_saves.push_back(tab.id);
                            }
                            (true, true) => tab.save(sessions, true),
                            (true, false) => tab.revert(sessions, true),
                            (false, _) => tab.close(sessions),
                        }
                    }
                    self.quitting = true;
                    *started = true;
                    done = true;
                }
                if cancel || escape {
                    done = true;
                }
            }
        });
        if browse
            && !done
            && let Some(asking) = self.asking.clone()
        {
            self.browse(asking, ui.ctx());
        }
        if done {
            match self.asking {
                Some(Asking::Export(_)) => self.export = None,
                Some(Asking::Changes(id)) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                        tab.hide_changes();
                    }
                }
                _ => {}
            }
            self.asking = None;
        }
        if let Some(id) = save_mine {
            self.ask_save_as(id, false, ui.ctx());
        }
        if let Some(id) = refused {
            self.save_panel_refused(id);
        }
        if let Some(id) = save_panel {
            self.ask_save_as(id, true, ui.ctx());
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

    use super::{Answered, App, Opening, Part, held_list, mine_row, split_answered, theirs_row};
    use crate::changes::{Changes, Listed, Row};
    use crate::tab::said;
    use crate::theme;

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
        (number, Part::File(path), response)
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
                part: Part::File(format!("{number}.silico")),
                running,
            })
            .collect();
        let (answered, pending) = split_answered(opening, |running| running.ok_or(None));
        assert_eq!(
            answered,
            [
                (1, Part::File("1.silico".to_owned()), 10),
                (3, Part::File("3.silico".to_owned()), 30)
            ]
        );
        let pending: Vec<(u64, Option<&str>)> = pending
            .iter()
            .map(|opening| (opening.number, opening.part.path()))
            .collect();
        assert_eq!(pending, [(2, Some("2.silico"))]);
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

    #[test]
    fn a_commands_shortcut_before_the_rename_field_has_had_the_focus_leaves_the_field_open() {
        use eframe::egui::{Context, Event, Key, Modifiers, RawInput};
        use silico_core::command::{Revision, TimelineAnswer};
        use silico_core::document::FeatureId;
        use silico_core::timeline::{Entry, Health};

        use crate::feature_menu::Place;

        let (mut app, dir) = app();
        let one = answered(&app, &dir, 1, "one.silico", true);
        app.take_opens(vec![one]);
        let block = FeatureId::new("k7f2qx3m").unwrap();
        let tab = &mut app.tabs[0];
        tab.timeline = Some(TimelineAnswer {
            revision: Revision::OPENED,
            model: Some(Revision::OPENED),
            problems: 0,
            features: Some(vec![Entry {
                id: block.clone(),
                kind: "box",
                name: "Block".to_owned(),
                suppressed: false,
                inputs: Vec::new(),
                health: Health::Healthy,
                problems: 0,
                warnings: 0,
            }]),
            bodies: Vec::new(),
        });
        // F2 on the block: its field asked for, not yet drawn, so it hasn't had the focus and
        // nothing can have been typed in it.
        tab.start_rename(block.clone(), Place::Timeline);
        assert!(!tab.renaming.as_ref().unwrap().focused());
        // Ctrl+S in the next frame, before the field is drawn: nothing typed to send, so the
        // field stays open for the name F2 asked for, and the save goes on.
        let ctrl_s = Event::Key {
            key: Key::S,
            physical_key: Some(Key::S),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::CTRL | Modifiers::COMMAND,
        };
        let ctx = Context::default();
        let mut sent = None;
        ctx.run_ui(
            RawInput {
                events: vec![ctrl_s],
                ..Default::default()
            },
            |ui| sent = Some(app.rename_before_command(ui.ctx())),
        )
        .textures_delta
        .clear();
        assert_eq!(sent, Some(false));
        let renaming = app.tabs[0].renaming.as_ref();
        assert_eq!(
            renaming.map(|renaming| (&renaming.feature, renaming.text.as_str())),
            Some((&block, "Block")),
            "the field closed"
        );
    }

    #[test]
    fn x_e_and_v_are_the_filters_outside_a_sketch_and_left_for_it_inside_one() {
        use eframe::egui::{Event, Key, Modifiers};
        use egui_kittest::Harness;

        use crate::toolbar::FilterKind;

        /// The filters taken in a step's passes, and whether X was left in one for what
        /// comes after (sketch mode's Construction).
        #[derive(Default)]
        struct Seen {
            in_sketch: bool,
            taken: Vec<FilterKind>,
            x_left: bool,
        }
        let mut h = Harness::builder().build_ui_state(
            |ui, seen: &mut Seen| {
                let taken = super::filter_keys(ui, seen.in_sketch);
                let x = ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::X));
                seen.taken.extend(taken);
                seen.x_left |= x;
            },
            Seen::default(),
        );
        h.run();
        let press = |h: &mut Harness<'_, Seen>, key: Key, modifiers: Modifiers| {
            h.state_mut().taken.clear();
            h.state_mut().x_left = false;
            // Pressed and let go, so the next press isn't egui's repeat of a key held down.
            for pressed in [true, false] {
                h.event(Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                });
            }
            h.run();
            (h.state().taken.clone(), h.state().x_left)
        };
        // Outside a sketch: X faces, E edges, V vertices, each taken, so X is no one else's.
        assert_eq!(
            press(&mut h, Key::X, Modifiers::NONE),
            (vec![FilterKind::Faces], false)
        );
        assert_eq!(
            press(&mut h, Key::E, Modifiers::NONE).0,
            [FilterKind::Edges]
        );
        assert_eq!(
            press(&mut h, Key::V, Modifiers::NONE).0,
            [FilterKind::Vertices]
        );
        // Exactly the key: Shift+X or Ctrl+E is no filter's.
        assert!(press(&mut h, Key::X, Modifiers::SHIFT).0.is_empty());
        assert!(press(&mut h, Key::E, Modifiers::COMMAND).0.is_empty());
        // In a sketch: none of them, and X left for sketch mode.
        h.state_mut().in_sketch = true;
        for key in [Key::E, Key::V] {
            assert!(press(&mut h, key, Modifiers::NONE).0.is_empty(), "{key:?}");
        }
        assert_eq!(press(&mut h, Key::X, Modifiers::NONE), (Vec::new(), true));
    }

    /// What [`held_list`] paints for `changes`, as Show changes draws its list `list` with
    /// `row`: each text, and its colour.
    fn held_list_painted(
        list: &'static str,
        changes: Option<Changes>,
        row: fn(&Row) -> (String, eframe::egui::Color32, String),
    ) -> Vec<(String, eframe::egui::Color32)> {
        use eframe::egui::Shape;
        fn flatten(shape: &Shape, out: &mut Vec<(String, eframe::egui::Color32)>) {
            match shape {
                Shape::Vec(shapes) => shapes.iter().for_each(|shape| flatten(shape, out)),
                Shape::Text(shown) => {
                    let colour = shown
                        .override_text_color
                        .unwrap_or(shown.galley.job.sections[0].format.color);
                    out.push((shown.galley.text().to_owned(), colour));
                }
                _ => {}
            }
        }
        let mut h = egui_kittest::Harness::builder()
            .with_size(eframe::egui::Vec2::new(520.0, 300.0))
            .build_ui(move |ui| held_list(ui, list, changes.as_ref(), row));
        h.run_steps(2);
        let mut out = Vec::new();
        for clipped in &h.output().shapes {
            flatten(&clipped.shape, &mut out);
        }
        out
    }

    /// The colour `text` was painted in, painted once.
    #[track_caller]
    fn painted_in(
        painted: &[(String, eframe::egui::Color32)],
        text: &str,
    ) -> eframe::egui::Color32 {
        let found: Vec<_> = painted.iter().filter(|(shown, _)| shown == text).collect();
        assert_eq!(
            found.len(),
            1,
            "{text:?} painted {} times: {painted:?}",
            found.len()
        );
        found[0].1
    }

    fn one_row(feature: &str, field: &str, text: &str) -> Option<Changes> {
        Some(Changes::Listed(Listed {
            rows: vec![Row {
                feature: feature.to_owned(),
                field: field.to_owned(),
                text: text.to_owned(),
            }],
            more: None,
            changes: 1,
            features: 1,
        }))
    }

    #[test]
    fn show_changes_lists_their_features_purple_and_your_edits_as_edits() {
        // The prototype's 6d rows: under "Their change (on disk)", the feature's name in
        // the key column in purple (`.k`, `var(--purple)`, #b780ff), the field and its
        // values beside it; under "Your unsaved edits", "Edit" in the key's grey.
        let theirs = held_list_painted("theirs", one_row("Pin", "radius", "5 → 3"), theirs_row);
        assert_eq!(
            painted_in(&theirs, "Pin"),
            eframe::egui::Color32::from_rgb(0xb7, 0x80, 0xff)
        );
        assert_eq!(painted_in(&theirs, "radius · 5 → 3"), theme::PROPERTY_VALUE);
        let mine = held_list_painted("mine", one_row("Block", "height", "10 → 12"), mine_row);
        assert_eq!(painted_in(&mine, "Edit"), theme::PROPERTY_KEY);
        assert_eq!(
            painted_in(&mine, "Block · height · 10 → 12"),
            theme::PROPERTY_VALUE
        );
        assert_ne!(theme::PROPERTY_KEY, theme::CHANGED_OUTSIDE);
    }

    #[test]
    fn a_list_says_it_is_reading_has_nothing_or_was_refused_with_its_cause() {
        for asking in [None, Some(Changes::Asking)] {
            let painted = held_list_painted("theirs", asking, theirs_row);
            painted_in(&painted, "Reading what changed…");
        }
        let empty = Some(Changes::Listed(Listed {
            rows: Vec::new(),
            more: None,
            changes: 0,
            features: 0,
        }));
        painted_in(
            &held_list_painted("mine", empty, mine_row),
            "No feature changed.",
        );
        // A refusal, red: what the core said, and under it its cause, the reader's message.
        let refused = Some(Changes::Refused {
            said: "to, the file on disk, doesn't read as a document".to_owned(),
            cause: Some("expected a key".to_owned()),
        });
        let painted = held_list_painted("theirs", refused, theirs_row);
        assert_eq!(
            painted_in(&painted, "to, the file on disk, doesn't read as a document"),
            theme::ERROR_TEXT
        );
        assert_eq!(painted_in(&painted, "expected a key"), theme::ERROR_TEXT);
        assert!(
            !painted
                .iter()
                .any(|(text, _)| text == "Reading what changed…")
        );
    }

    // ---- #351's mutants: what sketch mode's panels say of the frame ----

    #[test]
    fn sketch_modes_panels_asked_nothing_leave_whether_a_command_was_started_as_it_was() {
        // `started` says whether the frame started a command, so its next frame is asked for
        // at once: the sketch bar and the palette, Finish Sketch and Cancel unclicked, start
        // none, and never take back one started before them.
        let (mut app, dir) = app();
        let open = answered(&app, &dir, 1, "one.silico", true);
        let id = session(&open);
        app.take_opens(vec![open]);
        app.tabs_mut()[0].sketch = Some(crate::sketch::SketchMode::new(1, None));
        app.active = Some(id);
        let ctx = eframe::egui::Context::default();
        ctx.set_fonts(theme::fonts());
        // A first pass, in which the panels' areas only take their sizes.
        let mut output = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            app.sketch_canvas(ui, &mut false);
        });
        output.textures_delta.clear();
        for before in [false, true] {
            let mut after = None;
            let mut output = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
                let mut started = before;
                app.sketch_canvas(ui, &mut started);
                after = Some(started);
            });
            output.textures_delta.clear();
            assert_eq!(after, Some(before), "started {before}");
            // Drawn, so not passed over: the sketch bar's Finish Sketch among what's painted.
            fn texts(shape: &eframe::egui::Shape, into: &mut Vec<String>) {
                match shape {
                    eframe::egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, into)),
                    eframe::egui::Shape::Text(text) => into.push(text.galley.text().to_owned()),
                    _ => {}
                }
            }
            let mut painted = Vec::new();
            for clipped in &output.shapes {
                texts(&clipped.shape, &mut painted);
            }
            assert!(
                painted.iter().any(|text| text == "Finish Sketch"),
                "{painted:?}"
            );
        }
    }
}
