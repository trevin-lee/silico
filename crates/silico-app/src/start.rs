//! The start page: what the window shows with no document open, and the Home tab while
//! documents are (docs/design/m5-ui/DESIGN.md, *Start page or blank canvas?*, approved
//! 2026-11-01). New, Open, and Keyboard shortcuts, the documents opened lately
//! (`crate::recent`), each with when it was modified ([`when`]), and the note for agents
//! with Copy MCP config, laid out as the prototype's `.st-wrap` lays them out: a column of
//! its own at the left, Recent at the right, centred in the window, and under Recent the
//! note that a document dropped on the window opens. The warm start's state ([`Warm`]) is
//! the status bar's while no document is open.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Align, Align2, Color32, FontFamily, FontId, Id, Key, Modifiers, Pos2, Rect, Response,
    RichText, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType,
};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

use crate::icons::{Icon, IconSize};
use crate::recent::{Recent, Recents};
use crate::shortcuts::Command;
use crate::theme;
use crate::thumbs::Thumbs;
use crate::wake::Wake;

/// How the warm start (decision 10) stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warming {
    /// The kernel and sketch workers are starting.
    Starting,
    /// They've started.
    Ready,
    /// They couldn't be started ahead of time: the first rebuild starts them.
    Cold,
}

impl Warming {
    /// What the status bar says of it.
    pub fn said(self) -> &'static str {
        match self {
            Warming::Starting => "Starting the kernel and sketch solver…",
            Warming::Ready => "Kernel and sketch solver ready",
            Warming::Cold => "The kernel and sketch solver start with the first rebuild",
        }
    }
}

/// The warm start's state, shared between the thread that warms the workers and the
/// window, which draws it: [`Warming::Starting`] until that thread says otherwise.
#[derive(Clone, Default)]
pub struct Warm {
    state: Arc<AtomicU8>,
    wake: Wake,
}

impl Warm {
    /// A warm start under way.
    pub fn new() -> Self {
        Self::default()
    }

    /// The workers have started.
    pub fn ready(&self) {
        self.set(Warming::Ready);
    }

    /// The workers couldn't be started ahead of time.
    pub fn cold(&self) {
        self.set(Warming::Cold);
    }

    /// The warm start ended, having started `kernel` kernel workers and `sketches` sketch
    /// workers (`Sessions::warm`'s answer): ready if it started both kinds, else cold.
    pub fn started(&self, kernel: usize, sketches: usize) {
        if kernel > 0 && sketches > 0 {
            self.ready();
        } else {
            self.cold();
        }
    }

    fn set(&self, warming: Warming) {
        let state = match warming {
            Warming::Starting => 0,
            Warming::Ready => 1,
            Warming::Cold => 2,
        };
        self.state.store(state, Ordering::SeqCst);
        self.wake.wake();
    }

    /// How it stands.
    pub fn state(&self) -> Warming {
        match self.state.load(Ordering::SeqCst) {
            0 => Warming::Starting,
            1 => Warming::Ready,
            _ => Warming::Cold,
        }
    }

    /// A frame asked of `ctx` when it changes.
    pub(crate) fn waking(&self, ctx: &egui::Context) {
        self.wake.set(ctx);
    }
}

/// The warm start's state, as the status bar draws it: a spinner while it starts, a green
/// dot once ready, and its words.
pub(crate) fn warm_status(ui: &mut Ui, warming: Warming) {
    match warming {
        Warming::Starting => {
            ui.add(
                egui::Spinner::new()
                    .size(theme::SPINNER_SIZE)
                    .color(theme::TEXT_STRONG),
            );
        }
        Warming::Ready => {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::splat(theme::STATE_DOT_SIZE), Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), theme::STATE_DOT_SIZE / 2.0, theme::SUCCESS);
        }
        Warming::Cold => {}
    }
    ui.label(
        RichText::new(warming.said())
            .size(theme::HEADER_SIZE)
            .color(theme::TEXT_SUBDUED),
    );
}

/// The JSON an MCP client is given to run `silico serve` (docs/mcp.md, *Setup*): `command`
/// the `silico` beside the app, and the workspace, if one is known, the folder the server
/// takes paths in.
pub fn mcp_config(command: &str, workspace: Option<&str>) -> String {
    let args = match workspace {
        Some(workspace) => format!(r#"["serve", "--workspace", {}]"#, json_string(workspace)),
        None => r#"["serve"]"#.to_owned(),
    };
    format!(
        "{{\n  \"mcpServers\": {{\n    \"silico\": {{\n      \"command\": {},\n      \"args\": {args}\n    }}\n  }}\n}}\n",
        json_string(command)
    )
}

/// The folder an MCP client is told to serve (docs/mcp.md: `--workspace`): that of the
/// document `shown`, if one is; else of the newest recent document not found missing; else
/// `documents`, the platform's Documents folder. A file system's root is never one: a
/// server started there stops at once. None if none of them will do.
pub fn workspace(
    shown: Option<&str>,
    recents: &[Recent],
    documents: Option<&Path>,
) -> Option<PathBuf> {
    let folder_of = |path: &str| {
        let path = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path));
        path.parent().map(Path::to_path_buf)
    };
    let usable = |folder: &PathBuf| folder.parent().is_some();
    shown
        .and_then(folder_of)
        .filter(usable)
        .or_else(|| {
            recents
                .iter()
                .filter(|recent| !recent.missing)
                .filter_map(|recent| folder_of(&recent.path))
                .find(usable)
        })
        .or_else(|| documents.map(Path::to_path_buf).filter(usable))
}

/// What Copy MCP config says beside itself when it knows no workspace to name.
pub const NO_WORKSPACE_NOTE: &str =
    "Add \"--workspace\" and your models' folder to its args: a server needs one.";

/// `text` as a JSON string: quoted, its quotes, backslashes, and control characters escaped.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What the person did on the start page this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    None,
    New,
    Open,
    /// Keyboard shortcuts: the shortcut sheet, as `?` opens it.
    Shortcuts,
    /// A recent document clicked (or its row's Enter), by its path.
    Recent(String),
    /// A missing recent document's Remove from list, by its path.
    Remove(String),
    CopyMcpConfig,
}

/// What the start page shows besides its own controls.
pub(crate) struct Shown<'a> {
    /// The documents opened lately, if the app keeps a list.
    pub recents: Option<&'a Recents>,
    /// Their thumbnails, if the app keeps them.
    pub thumbs: Option<&'a Thumbs>,
    /// Whether Copy MCP config knows a workspace to name: if not, it says so.
    pub workspace_known: bool,
    /// Now, by the wall clock: what each recent document's time is said against ([`when`]).
    /// None if the system's clock can't be read as a time: then no time is said.
    pub now: Option<Timestamp>,
    /// Whether the page takes the arrow keys this frame (Recent's ↑ and ↓): not while a text
    /// field has the keyboard, nor with a modal or a menu open.
    pub keys: bool,
}

/// When a document was modified, `then`, as the start page's Recent says it at `now` (the
/// prototype's `.when`: `12 min ago`, `Yesterday`, `Oct 28`), in `zone`. The time between
/// them is the time elapsed (a day with a clock change counts its real hours); the days are
/// the calendar's in `zone`. In order, the first that holds:
///
/// - less than a minute between them, either way: `Just now` (a time a little ahead of the
///   clock is a file server's clock a little ahead of this one's);
/// - `then` a minute or more ahead of `now`: its date, as below (no time ago can be said);
/// - less than an hour ago: whole minutes, `1 min ago` to `59 min ago`;
/// - on `now`'s day: whole hours, `1 h ago` to `23 h ago` (`24 h ago` on a day of 25 hours,
///   the clocks going back);
/// - on the day before `now`'s: `Yesterday`;
/// - in `now`'s year: the month's abbreviation and the day, `Oct 28` (`Oct 5`, no leading
///   zero);
/// - else with the year too, `Oct 28, 2025`.
pub fn when(then: Timestamp, now: Timestamp, zone: &TimeZone) -> String {
    let ago = then.duration_until(now);
    if ago.abs() < SignedDuration::from_secs(60) {
        return "Just now".to_owned();
    }
    let (day, today) = (
        then.to_zoned(zone.clone()).date(),
        now.to_zoned(zone.clone()).date(),
    );
    if ago > SignedDuration::ZERO {
        if ago < SignedDuration::from_secs(60 * 60) {
            return format!("{} min ago", ago.as_secs() / 60);
        }
        if day == today {
            return format!("{} h ago", ago.as_secs() / (60 * 60));
        }
        if today.yesterday().is_ok_and(|yesterday| yesterday == day) {
            return "Yesterday".to_owned();
        }
    }
    let form = if day.year() == today.year() {
        "%b %-d"
    } else {
        "%b %-d, %Y"
    };
    // A date always has a month, a day, and a year: the ISO date only if jiff says not.
    jiff::fmt::strtime::format(form, day).unwrap_or_else(|_| day.to_string())
}

/// How many of the files a drop's toast names before it says how many more there are.
pub const NAMED: usize = 3;

/// `names` as a toast lists them: each of the first [`NAMED`], then how many more.
fn listed(names: &[String]) -> String {
    match names.get(NAMED..) {
        Some(more) if !more.is_empty() => {
            format!("{} and {} more", names[..NAMED].join(", "), more.len())
        }
        _ => names.join(", "),
    }
}

/// Whether `path` names a document by its extension, `.silico` in any case (as a Save As's
/// name keeps `.SILICO`, `crate::browse::document_path`): what a drop on the window opens.
pub fn names_a_document(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(crate::browse::EXTENSION))
}

/// What a toast says of the files dropped on the window that aren't documents, `names` (by
/// their file names, in the order dropped), none of which was opened: the first
/// [`NAMED`] named, and how many more.
pub fn not_documents(names: &[String]) -> String {
    match names {
        [one] => format!("{one} isn't a .silico document, so it wasn't opened"),
        names => format!(
            "{} aren't .silico documents, so they weren't opened",
            listed(names)
        ),
    }
}

/// What a toast says of the files dropped on the window while a quit closes the tabs,
/// `names`, none of which was opened.
pub fn not_opened_quitting(names: &[String]) -> String {
    format!("Quitting: {} not opened", listed(names))
}

/// What a toast says of the files dropped on the window while a question is asked over it
/// (a modal: New, Open, and Recent can't be reached then either), `names`, none of which
/// was opened.
pub fn not_opened_asking(names: &[String]) -> String {
    format!("Answer the question first: {} not opened", listed(names))
}

/// What a toast says of the files dropped on the window while `modal`, one that asks
/// nothing (the shortcut sheet, What's Wrong), is open over it, `names`, none of which was
/// opened.
pub fn not_opened_until_closed(modal: &str, names: &[String]) -> String {
    format!("Close {modal} first: {} not opened", listed(names))
}

/// What the drop note under Recent says (`.drop-note`), its middle part in `.mono`.
pub const DROP_NOTE: [&str; 3] = [
    "Drop a ",
    ".silico",
    " file anywhere in this window to open it.",
];

/// The start page, filling `ui` (the window's central panel): labelled `Start` for the
/// accessibility tree.
pub(crate) fn page(ui: &mut Ui, shown: &Shown<'_>) -> Act {
    let mut act = Act::None;
    ui.scope_builder(UiBuilder::new().id_salt("start"), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::Pane);
            node.set_label("Start");
        });
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let full = ui.available_width();
                let width = full.min(theme::START_MAX_WIDTH);
                let inner = (width - 2.0 * theme::START_PADDING_SIDE).max(0.0);
                let right = (inner - theme::START_SIDE_WIDTH - theme::START_COLUMN_GAP).max(0.0);
                ui.add_space(theme::START_PADDING_TOP);
                ui.horizontal_top(|ui| {
                    ui.add_space((full - width) / 2.0 + theme::START_PADDING_SIDE);
                    ui.vertical(|ui| {
                        ui.set_width(theme::START_SIDE_WIDTH);
                        side(ui, shown.workspace_known, &mut act);
                    });
                    ui.add_space(theme::START_COLUMN_GAP);
                    ui.vertical(|ui| {
                        ui.set_width(right);
                        recent(ui, shown, &mut act);
                        drop_note(ui);
                    });
                });
                ui.add_space(theme::START_PADDING_BOTTOM);
            });
    });
    act
}

/// The left column: the logo and name, New, Open, and Keyboard shortcuts, and the note for
/// agents. The prototype's Search commands (`S`) waits for the command search.
fn side(ui: &mut Ui, workspace_known: bool, act: &mut Act) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::START_BRAND_GAP;
        ui.add(
            Icon::Logo
                .image(IconSize::Toolbar)
                .fit_to_exact_size(Vec2::splat(theme::START_LOGO_SIZE))
                .tint(theme::TEXT_STRONG),
        );
        ui.label(
            RichText::new("Silico")
                .size(theme::START_BRAND_SIZE)
                .color(theme::TEXT_STRONG),
        );
    });
    ui.add_space(theme::START_BRAND_MARGIN);
    if action(ui, Icon::NewDoc, Command::New, true).clicked() {
        *act = Act::New;
    }
    ui.add_space(theme::START_ACTION_GAP);
    if action(ui, Icon::Open, Command::Open, false).clicked() {
        *act = Act::Open;
    }
    ui.add_space(theme::START_ACTION_GAP);
    if action(ui, Icon::Help, Command::Shortcuts, false).clicked() {
        *act = Act::Shortcuts;
    }
    ui.add_space(theme::START_NOTE_MARGIN);
    let width = ui.available_width();
    let (rule, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), Sense::hover());
    ui.painter().hline(
        rule.x_range(),
        rule.center().y,
        Stroke::new(1.0, theme::PANEL_BORDER),
    );
    ui.add_space(theme::START_NOTE_PADDING);
    let mut note = LayoutJob::default();
    let plain = TextFormat {
        font_id: FontId::proportional(theme::HEADER_SIZE),
        color: theme::TEXT_SUBDUED,
        line_height: Some(theme::START_NOTE_LINE_HEIGHT),
        ..Default::default()
    };
    let mono = TextFormat {
        font_id: FontId::monospace(theme::MONOSPACE_SIZE),
        color: theme::TEXT_DEFAULT,
        ..plain.clone()
    };
    for (text, format) in [
        ("Parts are plain ", &plain),
        (".silico", &mono),
        (" files. Agents edit the same files through ", &plain),
        ("silico serve", &mono),
        (
            " (MCP). When one changes a part you have open, Silico reloads it, or asks first \
             if you have unsaved edits.",
            &plain,
        ),
    ] {
        note.append(text, 0.0, format.clone());
    }
    note.wrap = TextWrapping::wrap_at_width(width);
    ui.label(note);
    ui.add_space(theme::START_COPY_GAP);
    if copy_button(ui).clicked() {
        *act = Act::CopyMcpConfig;
    }
    if !workspace_known {
        ui.add_space(theme::START_ACTION_GAP);
        ui.label(
            RichText::new(NO_WORKSPACE_NOTE)
                .size(theme::HEADER_SIZE)
                .color(theme::TEXT_SUBDUED),
        );
    }
}

/// One of the left column's actions, the column's width: its icon, its command's label, and
/// its shortcut's key caps (both from the registry), the primary one filled as
/// `button_primary` is.
fn action(ui: &mut Ui, icon: Icon, command: Command, primary: bool) -> Response {
    let label = command.label();
    let size = Vec2::new(ui.available_width(), theme::START_ACTION_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let lit = response.hovered() || response.has_focus();
    let (fill, text) = match (primary, lit) {
        (true, false) => (theme::PRIMARY_BUTTON_BG, theme::PRIMARY_BUTTON_TEXT),
        (true, true) => (
            theme::START_PRIMARY_HOVER_BG,
            theme::PRIMARY_BUTTON_HOVER_TEXT,
        ),
        (false, false) => (Color32::TRANSPARENT, theme::TEXT_DEFAULT),
        (false, true) => (theme::HOVER_BG, theme::TEXT_STRONG),
    };
    let painter = ui.painter();
    painter.rect_filled(rect, theme::NORMAL_CORNER_RADIUS, fill);
    focus_ring(ui, &response, rect);
    let icon_rect = Rect::from_center_size(
        Pos2::new(
            rect.left() + theme::START_ACTION_PADDING + crate::icons::ICON_SIZE / 2.0,
            rect.center().y,
        ),
        Vec2::splat(crate::icons::ICON_SIZE),
    );
    icon.image(IconSize::Small)
        .tint(text)
        .paint_at(ui, icon_rect);
    ui.painter().text(
        Pos2::new(
            icon_rect.right() + theme::START_ACTION_ICON_GAP,
            rect.center().y,
        ),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(theme::BODY_SIZE),
        text,
    );
    key_caps(
        ui,
        Pos2::new(rect.right() - theme::START_ACTION_PADDING, rect.center().y),
        &command.caps(ui.ctx()),
        primary,
    );
    response
}

/// A cap for each of `modifiers`, Command (Ctrl off macOS) first, then Shift, as the
/// platform names them.
pub(crate) fn modifier_caps(ctx: &egui::Context, modifiers: Modifiers) -> Vec<String> {
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let names = if mac {
        egui::ModifierNames::SYMBOLS
    } else {
        egui::ModifierNames::NAMES
    };
    [Modifiers::COMMAND, Modifiers::SHIFT]
        .into_iter()
        .filter(|modifier| modifiers.contains(*modifier))
        .map(|modifier| names.format(&modifier, mac))
        .collect()
}

/// The width `caps` take as [`key_caps`] paints them.
pub(crate) fn key_caps_width(ui: &Ui, caps: &[String]) -> f32 {
    key_caps_width_spaced(ui, caps, theme::KEY_CAP_GAP)
}

/// The width `caps` take as [`key_caps_spaced`] paints them, `gap` apart.
pub(crate) fn key_caps_width_spaced(ui: &Ui, caps: &[String], gap: f32) -> f32 {
    let gaps = caps.len().saturating_sub(1) as f32 * gap;
    caps.iter()
        .map(|cap| {
            let galley = ui.painter().layout_no_wrap(
                cap.clone(),
                FontId::proportional(theme::KEY_CAP_TEXT_SIZE),
                theme::KEY_CAP_TEXT,
            );
            key_cap_width(galley.size().x)
        })
        .sum::<f32>()
        + gaps
}

/// A key cap's width, for text `text` wide: its padding and its side border at each side.
fn key_cap_width(text: f32) -> f32 {
    (text + 2.0 * (theme::KEY_CAP_PADDING + theme::KEY_CAP_BORDER)).max(theme::KEY_CAP_MIN_WIDTH)
}

/// `caps` painted as key caps, the last ending at `right` (centred on its height).
pub(crate) fn key_caps(ui: &Ui, right: Pos2, caps: &[String], primary: bool) {
    key_caps_spaced(ui, right, caps, primary, theme::KEY_CAP_GAP);
}

/// [`key_caps`], `gap` apart.
pub(crate) fn key_caps_spaced(ui: &Ui, right: Pos2, caps: &[String], primary: bool, gap: f32) {
    let colours = if primary {
        (
            Color32::TRANSPARENT,
            theme::KEY_CAP_ON_PRIMARY_STROKE,
            theme::KEY_CAP_ON_PRIMARY_TEXT,
        )
    } else {
        (
            theme::KEY_CAP_BG,
            theme::KEY_CAP_STROKE,
            theme::KEY_CAP_TEXT,
        )
    };
    key_caps_coloured(ui, right, caps, colours, gap);
}

/// [`key_caps_spaced`], each cap filled, outlined, and lettered in `colours`.
pub(crate) fn key_caps_coloured(
    ui: &Ui,
    right: Pos2,
    caps: &[String],
    (fill, stroke, text): (Color32, Color32, Color32),
    gap: f32,
) {
    let painter = ui.painter();
    let mut x = right.x;
    for cap in caps.iter().rev() {
        let galley = painter.layout_no_wrap(
            cap.clone(),
            FontId::proportional(theme::KEY_CAP_TEXT_SIZE),
            text,
        );
        let width = key_cap_width(galley.size().x);
        let rect = Rect::from_min_max(
            Pos2::new(x - width, right.y - theme::KEY_CAP_HEIGHT / 2.0),
            Pos2::new(x, right.y + theme::KEY_CAP_HEIGHT / 2.0),
        );
        theme::key_cap_box(ui, rect, fill, stroke);
        painter.galley(
            theme::key_cap_text_centre(rect) - galley.size() / 2.0,
            galley,
            text,
        );
        x = rect.left() - gap;
    }
}

/// The focus ring round `rect`, outside it, if `response` has the keyboard's focus.
fn focus_ring(ui: &Ui, response: &Response, rect: Rect) {
    theme::focus_ring(
        ui,
        response,
        rect,
        theme::NORMAL_CORNER_RADIUS,
        StrokeKind::Outside,
    );
}

/// Copy MCP config, a ghost button whose icon lines up with the note's text above it.
fn copy_button(ui: &mut Ui) -> Response {
    const LABEL: &str = "Copy MCP config";
    // Laid out before it's known to be lit, so in the colour it's painted with.
    let galley = ui.painter().layout_no_wrap(
        LABEL.to_owned(),
        FontId::proportional(theme::BODY_SIZE),
        Color32::PLACEHOLDER,
    );
    let width = 2.0 * theme::GHOST_BUTTON_PADDING
        + crate::icons::ICON_SIZE
        + theme::GHOST_BUTTON_PADDING
        + galley.size().x;
    let (place, _) = ui.allocate_exact_size(Vec2::new(width, theme::BUTTON_HEIGHT), Sense::hover());
    let rect = place.translate(Vec2::new(-theme::GHOST_BUTTON_PADDING, 0.0));
    let response = ui.interact(rect, ui.id().with("copy_mcp_config"), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, LABEL));
    let response = response.on_hover_text(
        "Copies the JSON an MCP client is given to run silico serve, as docs/mcp.md shows it",
    );
    let lit = response.hovered() || response.has_focus();
    let text = if lit {
        theme::TEXT_STRONG
    } else {
        theme::TEXT_DEFAULT
    };
    if lit {
        ui.painter().rect_filled(
            rect,
            theme::SMALL_CORNER_RADIUS,
            theme::GHOST_BUTTON_HOVER_BG,
        );
    }
    focus_ring(ui, &response, rect);
    let icon_rect = Rect::from_center_size(
        Pos2::new(
            rect.left() + theme::GHOST_BUTTON_PADDING + crate::icons::ICON_SIZE / 2.0,
            rect.center().y,
        ),
        Vec2::splat(crate::icons::ICON_SIZE),
    );
    Icon::Copy
        .image(IconSize::Small)
        .tint(text)
        .paint_at(ui, icon_rect);
    ui.painter().galley(
        Pos2::new(
            icon_rect.right() + theme::GHOST_BUTTON_PADDING,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        text,
    );
    response
}

/// A section's header: its name in capitals, subdued, letters spaced; and at its right, if
/// `hint`, Recent's keys (`↑ ↓ ↵ opens`, app.js's `.st-h`).
fn header(ui: &mut Ui, name: &str, hint: bool) {
    let said = ui.label(
        RichText::new(name.to_uppercase())
            .size(theme::HEADER_SIZE)
            .color(theme::TEXT_SUBDUED)
            .extra_letter_spacing(theme::CAPITALS_LETTER_SPACING),
    );
    if hint {
        keys_hint(ui, Pos2::new(ui.max_rect().right(), said.rect.center().y));
    }
    ui.add_space(theme::START_HEADER_GAP);
}

/// What Recent's header says of its keys, after the caps (`.st-h`'s last span).
pub const KEYS_HINT: &str = "opens";

/// Recent's keys, ending at `right`, on its line's middle: the caps ↑ and ↓ side by side
/// (two `.keys`, no space between), a space, ↵'s cap, a space, and "opens", subdued, in the
/// header's size with no capitals' spacing. Painted only: the rows' focus and Enter are
/// what the accessibility tree has of it.
fn keys_hint(ui: &Ui, right: Pos2) {
    let painter = ui.painter();
    let font = FontId::proportional(theme::HEADER_SIZE);
    let laid =
        |text: &str| painter.layout_no_wrap(text.to_owned(), font.clone(), theme::TEXT_SUBDUED);
    let opens = laid(KEYS_HINT);
    // A space's width: " opens" less "opens", as the line lays it out.
    let space = laid(&format!(" {KEYS_HINT}")).size().x - opens.size().x;
    let opens_left = right.x - opens.size().x;
    painter.galley(
        Pos2::new(opens_left, right.y - opens.size().y / 2.0),
        opens,
        theme::TEXT_SUBDUED,
    );
    let enter = vec![crate::shortcuts::key_cap(Key::Enter)];
    let enter_right = Pos2::new(opens_left - space, right.y);
    key_caps(ui, enter_right, &enter, false);
    let arrows = vec![
        crate::shortcuts::key_cap(Key::ArrowUp),
        crate::shortcuts::key_cap(Key::ArrowDown),
    ];
    let arrows_right = Pos2::new(enter_right.x - key_caps_width(ui, &enter) - space, right.y);
    key_caps_spaced(ui, arrows_right, &arrows, false, 0.0);
}

/// What Recent says when the app keeps no list: the platform named no folder for it, or its
/// thread couldn't be started.
pub const NOT_KEPT: &str =
    "Recent documents aren't being kept: there's nowhere to keep their list.";

/// What Recent says when it lists nothing yet.
pub const NO_RECENTS: &str = "No recent documents yet: each you open or make is listed here.";

/// The right column: the recent documents, newest first, labelled `Recent` for the
/// accessibility tree, each a button by its file's name, with when it was modified (a
/// label) or, if it's missing, Remove from list (a link); ↑ and ↓ move the keyboard's focus
/// along them, and Enter opens the one focused, as its click does.
fn recent(ui: &mut Ui, shown: &Shown<'_>, act: &mut Act) {
    let recents = shown.recents;
    let said = match recents {
        None => Some(NOT_KEPT),
        Some(recents) if !recents.is_kept() => Some(NOT_KEPT),
        Some(recents) if !recents.is_read() => Some("Reading the list of recent documents…"),
        Some(recents) if !recents.list().is_empty() => None,
        Some(_) => Some(NO_RECENTS),
    };
    header(ui, "Recent", said.is_none());
    ui.scope_builder(UiBuilder::new().id_salt("recent"), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::List);
            node.set_label("Recent");
        });
        if let Some(said) = said {
            ui.label(RichText::new(said).color(theme::TEXT_SUBDUED));
            return;
        }
        let list = recents.map(Recents::list).unwrap_or_default();
        let utc = TimeZone::UTC;
        let zone = recents.map_or(&utc, Recents::zone);
        let ids: Vec<Id> = list
            .iter()
            .map(|recent| ui.id().with(("recent row", &recent.path)))
            .collect();
        if shown.keys {
            arrows(ui, &ids);
        }
        for (at, (recent, id)) in list.iter().zip(ids).enumerate() {
            // `.recent`'s gap, between rows only.
            if at > 0 {
                ui.add_space(theme::START_RECENT_ROW_GAP);
            }
            let picture = shown.thumbs.and_then(|thumbs| thumbs.picture(&recent.path));
            match row(ui, id, recent, picture, shown.now, zone) {
                Row::Opened => *act = Act::Recent(recent.path.clone()),
                Row::Removed => *act = Act::Remove(recent.path.clone()),
                Row::None => {}
            }
        }
    });
}

/// ↑ and ↓ pressed this frame, with no modifier, each taken in turn (interact.js's
/// `keydown` on the start page): the keyboard's focus moved to the next row down or up,
/// stopping at the first and the last, from the first row if no row has it. egui's own
/// arrow keys, which move the focus to the nearest widget that way, don't move it too.
fn arrows(ui: &mut Ui, rows: &[Id]) {
    let Some(last) = rows.len().checked_sub(1) else {
        return;
    };
    let downs: Vec<bool> = ui.input_mut(|input| {
        let mut downs = Vec::new();
        input.events.retain(|event| match event {
            egui::Event::Key {
                key: key @ (Key::ArrowUp | Key::ArrowDown),
                pressed: true,
                modifiers,
                ..
            } if modifiers.is_none() => {
                downs.push(*key == Key::ArrowDown);
                false
            }
            _ => true,
        });
        downs
    });
    if downs.is_empty() {
        return;
    }
    ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
    let mut at = ui
        .memory(|memory| memory.focused())
        .and_then(|focused| rows.iter().position(|row| *row == focused));
    for down in downs {
        at = Some(match at {
            None => 0,
            Some(at) if down => (at + 1).min(last),
            Some(at) => at.saturating_sub(1),
        });
    }
    if let Some(row) = at.and_then(|at| rows.get(at)) {
        ui.memory_mut(|memory| memory.request_focus(*row));
    }
}

/// The note under Recent that a document dropped on the window opens (`.drop-note`): the
/// column's width, below the list by `.recent`'s margin, in a dashed border, its words
/// centred.
fn drop_note(ui: &mut Ui) {
    ui.add_space(theme::START_DROP_NOTE_MARGIN);
    let width = ui.available_width();
    let inner = (width - 2.0 * (theme::START_DROP_NOTE_PADDING + theme::BORDER_WIDTH)).max(0.0);
    let plain = TextFormat {
        font_id: FontId::proportional(theme::BODY_SIZE),
        color: theme::TEXT_SUBDUED,
        line_height: Some(theme::BODY_LINE_HEIGHT),
        ..Default::default()
    };
    let mono = TextFormat {
        font_id: FontId::monospace(theme::MONOSPACE_SIZE),
        ..plain.clone()
    };
    let mut job = LayoutJob::default();
    for (at, text) in DROP_NOTE.iter().enumerate() {
        let format = if at == 1 { &mono } else { &plain };
        job.append(text, 0.0, format.clone());
    }
    job.wrap = TextWrapping::wrap_at_width(inner);
    job.halign = Align::Center;
    let galley = ui.painter().layout_job(job);
    let height = galley.size().y + 2.0 * (theme::START_DROP_NOTE_PADDING + theme::BORDER_WIDTH);
    let (_, rect) = ui.allocate_space(Vec2::new(width, height));
    // The stroke's middle, half its width inside the note's edge, its corners as much
    // tighter than the edge's.
    let outline = rounded_outline(
        rect.shrink(theme::BORDER_WIDTH / 2.0),
        f32::from(theme::NORMAL_CORNER_RADIUS) - theme::BORDER_WIDTH / 2.0,
    );
    ui.painter().extend(Shape::dashed_line(
        &outline,
        Stroke::new(theme::BORDER_WIDTH, theme::START_DROP_NOTE_BORDER),
        theme::START_DROP_NOTE_DASH,
        theme::START_DROP_NOTE_DASH_GAP,
    ));
    ui.painter().galley(
        Pos2::new(
            rect.center().x,
            rect.top() + theme::BORDER_WIDTH + theme::START_DROP_NOTE_PADDING,
        ),
        galley,
        theme::TEXT_SUBDUED,
    );
}

/// The outline of `rect` with corners `radius` round, closed: its corners in eight steps a
/// quarter turn.
fn rounded_outline(rect: Rect, radius: f32) -> Vec<Pos2> {
    const STEPS: usize = 8;
    let radius = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let corners = [
        // Each corner's centre, and the angle its arc starts at (y down: a quarter turn on is
        // clockwise on the screen).
        (rect.right_top() + Vec2::new(-radius, radius), -0.5),
        (rect.right_bottom() + Vec2::new(-radius, -radius), 0.0),
        (rect.left_bottom() + Vec2::new(radius, -radius), 0.5),
        (rect.left_top() + Vec2::new(radius, radius), 1.0),
    ];
    let mut points = Vec::with_capacity(4 * (STEPS + 1) + 1);
    for (centre, start) in corners {
        for step in 0..=STEPS {
            let turn = (start + 0.5 * step as f32 / STEPS as f32) * std::f32::consts::PI;
            points.push(centre + radius * Vec2::new(libm::cosf(turn), libm::sinf(turn)));
        }
    }
    if let Some(first) = points.first().copied() {
        points.push(first);
    }
    points
}

/// What a recent document's row was asked this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    None,
    /// Clicked, or Enter (or Space) with the keyboard's focus on it.
    Opened,
    /// Its Remove from list clicked, which opens nothing.
    Removed,
}

/// What a missing recent document's row offers in place of its time (`.link`).
pub const REMOVE_FROM_LIST: &str = "Remove from list";

/// A recent document's row, `id`: its thumbnail's place (`picture`, the part drawn, if it has
/// one and is there; else the document's icon, or a warning's if nothing was at its path), its
/// name and its folder, and "not found" if nothing was; at its right (`.when`), when it was
/// modified, said at `now` in `zone` ([`when`]; nothing if the clock gave no time), or, if
/// it's missing, [`REMOVE_FROM_LIST`], a link that takes its own click. Its name and folder are
/// cut short before that column and the grid's gap.
fn row(
    ui: &mut Ui,
    id: Id,
    recent: &Recent,
    picture: Option<&egui::TextureHandle>,
    now: Option<Timestamp>,
    zone: &TimeZone,
) -> Row {
    let size = Vec2::new(ui.available_width(), theme::START_RECENT_ROW_HEIGHT);
    let (_, rect) = ui.allocate_space(size);
    let response = ui.interact(rect, id, Sense::click());
    let name = recent.name().to_owned();
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
    let described = if recent.missing {
        format!("{}, not found", recent.path)
    } else {
        recent.path.clone()
    };
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_description(described);
    });
    let response = response.on_hover_text(&recent.path);
    // The right column: the link, or the time, if any, laid out first so the name and folder
    // are cut short before it.
    let right = rect.right() - theme::START_RECENT_PADDING_RIGHT;
    let when_format = |ink: Color32, underline: Stroke| TextFormat {
        font_id: FontId::proportional(theme::HEADER_SIZE),
        color: ink,
        underline,
        line_height: Some(theme::BODY_LINE_HEIGHT),
        ..Default::default()
    };
    let (when_text, link) = match (&recent.missing, &recent.modified) {
        (true, _) => (Some(REMOVE_FROM_LIST.to_owned()), true),
        (false, Some(modified)) => (now.map(|now| when(*modified, now, zone)), false),
        (false, None) => (None, false),
    };
    let when_size = when_text.as_ref().map_or(Vec2::ZERO, |text| {
        ui.painter()
            .layout_job(LayoutJob::single_section(
                text.clone(),
                when_format(theme::INFO, Stroke::NONE),
            ))
            .size()
    });
    let when_rect = Rect::from_min_size(
        Pos2::new(right - when_size.x, rect.center().y - when_size.y / 2.0),
        when_size,
    );
    // The row lit with the pointer on its link too, as a browser's `:hover` lights the
    // button the link is in.
    let over_link = link && ui.rect_contains_pointer(when_rect);
    let mut asked = if response.clicked() {
        Row::Opened
    } else {
        Row::None
    };
    if ui.is_rect_visible(rect) {
        paint_row(ui, &response, rect, recent, picture, over_link, when_size.x);
    }
    let Some(text) = when_text else {
        return asked;
    };
    if link {
        // After the row, so above it: its click is the link's alone.
        let removing = ui.interact(when_rect, id.with("remove"), Sense::click());
        removing.widget_info(|| WidgetInfo::labeled(WidgetType::Link, true, REMOVE_FROM_LIST));
        ui.ctx().accesskit_node_builder(removing.id, |node| {
            node.set_description(recent.path.clone());
        });
        let (ink, underline) = if removing.hovered() {
            (
                theme::TEXT_STRONG,
                Stroke::new(theme::BORDER_WIDTH, theme::TEXT_STRONG),
            )
        } else {
            (theme::INFO, Stroke::NONE)
        };
        let galley = ui
            .painter()
            .layout_job(LayoutJob::single_section(text, when_format(ink, underline)));
        ui.painter().galley(when_rect.min, galley, ink);
        theme::focus_ring(
            ui,
            &removing,
            when_rect,
            theme::SMALL_CORNER_RADIUS,
            StrokeKind::Outside,
        );
        let removing = removing.on_hover_cursor(egui::CursorIcon::PointingHand);
        if removing.clicked() {
            asked = Row::Removed;
        }
    } else {
        // A label in the tree, under the list, so the time is read out as it's shown.
        let shown = ui.interact(when_rect, id.with("when"), Sense::hover());
        shown.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &text));
        let galley = ui.painter().layout_job(LayoutJob::single_section(
            text,
            when_format(theme::TEXT_SUBDUED, Stroke::NONE),
        ));
        ui.painter()
            .galley(when_rect.min, galley, theme::TEXT_SUBDUED);
    }
    asked
}

/// A recent document's row, but its right column, `when_width` wide: its fill hovered or
/// focused (or `over_link`), the focus ring, the thumbnail's place (`picture`, or an icon),
/// and its name and folder.
fn paint_row(
    ui: &Ui,
    response: &Response,
    rect: Rect,
    recent: &Recent,
    picture: Option<&egui::TextureHandle>,
    over_link: bool,
    when_width: f32,
) {
    if response.hovered() || response.has_focus() || over_link {
        ui.painter().rect_filled(
            rect,
            theme::NORMAL_CORNER_RADIUS,
            theme::START_RECENT_HOVER_BG,
        );
    }
    focus_ring(ui, response, rect);
    let thumb = Rect::from_min_size(
        Pos2::new(
            rect.left() + theme::START_RECENT_PADDING_LEFT,
            rect.center().y - theme::START_THUMB_HEIGHT / 2.0,
        ),
        Vec2::new(theme::START_THUMB_WIDTH, theme::START_THUMB_HEIGHT),
    );
    ui.painter().rect(
        thumb,
        theme::SMALL_CORNER_RADIUS,
        theme::START_THUMB_BG,
        Stroke::new(1.0, theme::PANEL_BORDER),
        StrokeKind::Inside,
    );
    if let Some(picture) = picture.filter(|_| !recent.missing) {
        crate::thumbs::paint(ui.painter(), thumb, picture);
    } else {
        let (icon, tint) = if recent.missing {
            (Icon::Warn, theme::TEXT_DISABLED)
        } else {
            (Icon::Doc, theme::TEXT_SUBDUED)
        };
        icon.image(IconSize::Small).tint(tint).paint_at(
            ui,
            Rect::from_center_size(thumb.center(), Vec2::splat(crate::icons::ICON_SIZE)),
        );
    }
    let left = thumb.right() + theme::START_RECENT_COLUMN_GAP;
    // The grid's gap stands before the right column, even an empty one.
    let width = (rect.right()
        - theme::START_RECENT_PADDING_RIGHT
        - when_width
        - theme::START_RECENT_COLUMN_GAP
        - left)
        .max(0.0);
    let mut name_job = LayoutJob::single_section(
        recent.name().to_owned(),
        TextFormat {
            font_id: FontId::proportional(theme::BODY_SIZE),
            color: if recent.missing {
                theme::TEXT_SUBDUED
            } else {
                theme::TEXT_STRONG
            },
            strikethrough: if recent.missing {
                Stroke::new(1.0, theme::TEXT_SUBDUED)
            } else {
                Stroke::NONE
            },
            line_height: Some(theme::BODY_LINE_HEIGHT),
            ..Default::default()
        },
    );
    name_job.wrap = TextWrapping::truncate_at_width(width);
    let mut folder_job = LayoutJob::default();
    folder_job.append(
        recent.folder(),
        0.0,
        TextFormat {
            font_id: FontId::new(theme::MONOSPACE_SIZE, FontFamily::Monospace),
            color: theme::TEXT_SUBDUED,
            line_height: Some(theme::BODY_LINE_HEIGHT),
            ..Default::default()
        },
    );
    if recent.missing {
        folder_job.append(
            " · not found",
            0.0,
            TextFormat {
                font_id: FontId::proportional(theme::HEADER_SIZE),
                color: theme::ERROR_TEXT,
                line_height: Some(theme::BODY_LINE_HEIGHT),
                ..Default::default()
            },
        );
    }
    folder_job.wrap = TextWrapping::truncate_at_width(width);
    let painter = ui.painter();
    let name_galley = painter.layout_job(name_job);
    let folder_galley = painter.layout_job(folder_job);
    let top = rect.center().y - theme::BODY_LINE_HEIGHT;
    painter.galley(Pos2::new(left, top), name_galley, theme::TEXT_STRONG);
    painter.galley(
        Pos2::new(left, top + theme::BODY_LINE_HEIGHT),
        folder_galley,
        theme::TEXT_SUBDUED,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mcp_config_is_docs_mcp_mds_form() {
        // docs/mcp.md, *Setup*: written out here by hand.
        assert_eq!(
            mcp_config("/opt/silico/silico", Some("/home/me/models")),
            r#"{
  "mcpServers": {
    "silico": {
      "command": "/opt/silico/silico",
      "args": ["serve", "--workspace", "/home/me/models"]
    }
  }
}
"#
        );
        assert_eq!(
            mcp_config("silico", None),
            "{\n  \"mcpServers\": {\n    \"silico\": {\n      \"command\": \"silico\",\n      \"args\": [\"serve\"]\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn a_paths_quotes_backslashes_and_control_characters_are_escaped() {
        assert_eq!(
            json_string("a\"b\\c\nd\te\u{1}f\u{7f}é"),
            "\"a\\\"b\\\\c\\nd\\te\\u0001f\u{7f}é\""
        );
    }

    #[test]
    fn the_last_control_character_is_escaped_and_a_space_is_not() {
        // JSON (RFC 8259, 7): U+0000 to U+001F escaped; U+0020, the space, is itself.
        assert_eq!(json_string("\u{1f} \u{1f}"), "\"\\u001f \\u001f\"");
        assert_eq!(json_string("/My Models"), "\"/My Models\"");
    }

    #[test]
    fn a_warm_start_told_ready_off_the_ui_thread_asks_for_a_frame() {
        let ctx = egui::Context::default();
        let asked = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counted = Arc::clone(&asked);
        ctx.set_request_repaint_callback(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        let warm = Warm::new();
        warm.waking(&ctx);
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        let told = warm.clone();
        std::thread::spawn(move || told.ready()).join().unwrap();
        assert_eq!(asked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_warm_start_is_ready_only_if_both_kinds_of_worker_started() {
        for (kernel, sketches, want) in [
            (1, 1, Warming::Ready),
            (2, 1, Warming::Ready),
            (0, 1, Warming::Cold),
            (1, 0, Warming::Cold),
            (0, 0, Warming::Cold),
        ] {
            let warm = Warm::new();
            warm.started(kernel, sketches);
            assert_eq!(warm.state(), want, "{kernel} {sketches}");
        }
    }

    fn recent(path: &str, missing: bool) -> Recent {
        Recent {
            path: path.to_owned(),
            missing,
            modified: None,
        }
    }

    #[test]
    fn the_workspace_is_the_shown_documents_folder_then_a_recent_ones_then_documents() {
        let documents = Some(Path::new("/home/me/Documents"));
        let recents = [
            recent("/gone/a.silico", true),
            recent("/b.silico", false),
            recent("/models/c.silico", false),
            recent("/older/d.silico", false),
        ];
        let at = |shown, recents: &[Recent], documents| {
            workspace(shown, recents, documents).map(|folder| folder.display().to_string())
        };
        // The document shown.
        assert_eq!(
            at(Some("/parts/e.silico"), &recents, documents).as_deref(),
            Some("/parts")
        );
        // The newest recent one that isn't missing and isn't at the root.
        assert_eq!(at(None, &recents, documents).as_deref(), Some("/models"));
        // One shown at the root: the recent documents' instead.
        assert_eq!(
            at(Some("/e.silico"), &recents, documents).as_deref(),
            Some("/models")
        );
        // None that will do: Documents; then nothing.
        assert_eq!(
            at(None, &recents[..2], documents).as_deref(),
            Some("/home/me/Documents")
        );
        assert_eq!(at(None, &[], None), None);
        assert_eq!(at(None, &[], Some(Path::new("/"))), None);
    }

    #[test]
    fn the_warm_start_is_starting_until_told() {
        let warm = Warm::new();
        assert_eq!(warm.state(), Warming::Starting);
        let shared = warm.clone();
        shared.ready();
        assert_eq!(warm.state(), Warming::Ready);
        shared.cold();
        assert_eq!(warm.state(), Warming::Cold);
    }

    // ---- The page as drawn, against the prototype ----
    //
    // Each value below is typed from the approved prototype's stylesheet
    // (docs/design/m5-ui/styles.css: `:root`'s tokens, `.st-wrap`, `.st-brand`, `.st-action`,
    // `.keys`, `.kbd`, `.st-side-note`, `.btn`, `.btn-ghost`, `.st-h`, `.recent`, `.recent-row`,
    // `.state-dot`, `:focus-visible`) and js/app.js's renderStart, never from theme.rs, and
    // compared with what the page painted: each rectangle, line, and text from egui's shapes.

    use egui::epaint::{CircleShape, RectShape, TextShape};
    use egui::text::TextFormat as Format;
    use egui::{CornerRadius, Shape};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    /// The prototype's tokens and geometry.
    mod css {
        use eframe::egui::Color32;

        // `:root`'s greys and colours.
        pub const G0: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
        pub const G200: Color32 = Color32::from_rgb(0x21, 0x21, 0x21);
        pub const G250: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
        pub const G300: Color32 = Color32::from_rgb(0x2c, 0x2b, 0x2b);
        pub const G350: Color32 = Color32::from_rgb(0x52, 0x51, 0x51);
        pub const G400: Color32 = Color32::from_rgb(0x64, 0x63, 0x63);
        pub const G550: Color32 = Color32::from_rgb(0x93, 0x90, 0x90);
        pub const G750: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);
        pub const G800: Color32 = Color32::from_rgb(0xe0, 0xdf, 0xdf);
        pub const G1000: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
        /// `--blue-hi`: `:focus-visible`'s outline.
        pub const BLUE_HI: Color32 = Color32::from_rgb(0x46, 0x76, 0xff);
        /// `--red`: `.err-text`.
        pub const RED: Color32 = Color32::from_rgb(0xff, 0x38, 0x65);
        /// `--green`: the status bar's ready dot.
        pub const GREEN: Color32 = Color32::from_rgb(0x00, 0xda, 0x7e);
        /// `.st-action.primary .kbd`: `color: #444`, `border-color: rgba(0,0,0,.25)`.
        pub const KBD_ON_PRIMARY_TEXT: Color32 = Color32::from_rgb(0x44, 0x44, 0x44);
        pub const KBD_ON_PRIMARY_BORDER: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 64);
        /// `.st-action.primary:hover`: `background: #fff; color: #000`.
        pub const PRIMARY_HOVER_TEXT: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);

        /// `--fs`, `--lh`, `--fs-small`, `.mono`'s size, `--icon`, `--r`, `--r-sm`.
        pub const FS: f32 = 12.0;
        pub const LH: f32 = 16.0;
        pub const FS_SMALL: f32 = 11.0;
        pub const MONO: f32 = 11.0;
        pub const ICON: f32 = 14.0;
        pub const R: u8 = 6;
        pub const R_SM: u8 = 4;

        /// `.st-wrap`: `max-width: 1080px; padding: 44px 40px 40px; grid-template-columns:
        /// 240px 1fr; gap: 48px`.
        pub const WRAP_MAX: f32 = 1080.0;
        pub const WRAP_TOP: f32 = 44.0;
        pub const WRAP_SIDE: f32 = 40.0;
        pub const SIDE: f32 = 240.0;
        pub const GAP: f32 = 48.0;
        /// `.st-brand`: `gap: 10px; font-size: 18px; margin-bottom: 26px`, its logo 26.
        pub const BRAND_GAP: f32 = 10.0;
        pub const BRAND_SIZE: f32 = 18.0;
        pub const BRAND_MARGIN: f32 = 26.0;
        pub const LOGO: f32 = 26.0;
        /// `.st-actions { gap: 4px }`; `.st-action`: `gap: 10px; height: 34px; padding: 0
        /// 10px; border-radius: var(--r)`.
        pub const ACTIONS_GAP: f32 = 4.0;
        pub const ACTION_GAP: f32 = 10.0;
        pub const ACTION_HEIGHT: f32 = 34.0;
        pub const ACTION_PADDING: f32 = 10.0;
        /// `.keys { gap: 2px }`; `.kbd`: `min-width: 16px; padding: 0 4px; height: 16px;
        /// line-height: 16px; border-radius: 3px; border: 1px; border-bottom-width: 2px;
        /// font-size: 10.5px` (`border-box`, `*`'s).
        pub const KEYS_GAP: f32 = 2.0;
        pub const KBD_MIN_WIDTH: f32 = 16.0;
        pub const KBD_PADDING: f32 = 4.0;
        pub const KBD_BORDER: f32 = 1.0;
        pub const KBD_HEIGHT: f32 = 16.0;
        pub const KBD_LINE_HEIGHT: f32 = 16.0;
        pub const KBD_RADIUS: u8 = 3;
        pub const KBD_SIZE: f32 = 10.5;
        /// `.st-side-note`: `margin-top: 28px; padding-top: 16px; border-top: 1px solid
        /// var(--g250); font-size: var(--fs-small); line-height: 17px`.
        pub const NOTE_MARGIN: f32 = 28.0;
        pub const NOTE_PADDING: f32 = 16.0;
        pub const NOTE_LINE_HEIGHT: f32 = 17.0;
        /// Copy MCP config: `margin-top: 10px` above it; `.btn`'s `height: 22px; gap: 6px`,
        /// its padding `0 6px` and `margin-left: -6px`.
        pub const COPY_MARGIN: f32 = 10.0;
        pub const BTN_HEIGHT: f32 = 22.0;
        pub const BTN_GAP: f32 = 6.0;
        pub const BTN_PADDING: f32 = 6.0;
        /// `.st-h { margin: 0 0 8px }`; `.recent { gap: 1px }`.
        pub const HEADER_MARGIN: f32 = 8.0;
        pub const RECENT_GAP: f32 = 1.0;
        /// `.recent-row`: `grid-template-columns: 52px 1fr auto; gap: 14px; padding: 6px 10px
        /// 6px 6px`; its thumb 52 by 40.
        pub const ROW_PADDING_Y: f32 = 6.0;
        pub const ROW_PADDING_RIGHT: f32 = 10.0;
        pub const ROW_PADDING_LEFT: f32 = 6.0;
        pub const ROW_GAP: f32 = 14.0;
        pub const THUMB_WIDTH: f32 = 52.0;
        pub const THUMB_HEIGHT: f32 = 40.0;
        /// `.state-dot`: 8 by 8, `border-radius: 50%`.
        pub const STATE_DOT: f32 = 8.0;
    }

    /// The wall clock's now in these tests: 2026-11-15 14:30:00 UTC. From the epoch to
    /// 2026-01-01, 56 years of 365 days and 14 leap days (1972 to 2024), 20,454 days; then
    /// 304 to November (31 + 28 + 31 + 30 + 31 + 30 + 31 + 31 + 30 + 31) and 14 more:
    /// 20,772 days of 86,400 s, 1,794,700,800, and 14.5 h, 52,200.
    const NOW: Timestamp = Timestamp::constant(1_794_753_000, 0);

    /// The harness's window, and the margin egui_kittest's frame leaves round the page.
    const WINDOW: Vec2 = Vec2::new(1296.0, 900.0);
    const MARGIN: f32 = 8.0;

    /// `app` drawn in the app's theme, its icons loaded.
    fn drawn<'a>(app: impl FnMut(&mut Ui) + 'a) -> Harness<'a> {
        let mut h = Harness::builder().with_size(WINDOW).build_ui(app);
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    /// Every shape the last frame painted, nested ones flattened.
    fn painted(h: &Harness<'_>) -> Vec<Shape> {
        fn flatten(shape: &Shape, out: &mut Vec<Shape>) {
            match shape {
                Shape::Vec(shapes) => shapes.iter().for_each(|shape| flatten(shape, out)),
                shape => out.push(shape.clone()),
            }
        }
        let mut out = Vec::new();
        for clipped in &h.output().shapes {
            flatten(&clipped.shape, &mut out);
        }
        out
    }

    fn rects(h: &Harness<'_>) -> Vec<RectShape> {
        painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Rect(rect) => Some(rect),
                _ => None,
            })
            .collect()
    }

    /// The texts painted whose text is `text`.
    fn texts(h: &Harness<'_>, text: &str) -> Vec<TextShape> {
        painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Text(shown) if shown.galley.text() == text => Some(shown),
                _ => None,
            })
            .collect()
    }

    /// The one text painted whose text is `text`.
    #[track_caller]
    fn text(h: &Harness<'_>, text: &str) -> TextShape {
        let mut found = texts(h, text);
        assert_eq!(found.len(), 1, "{text:?} painted {} times", found.len());
        found.remove(0)
    }

    /// The icons painted (each a rectangle filled with its texture, tinted), by rectangle.
    fn icon_at(h: &Harness<'_>, rect: Rect) -> Option<Color32> {
        rects(h)
            .into_iter()
            .find(|shape| shape.brush.is_some() && near_rect(shape.rect, rect))
            .map(|shape| shape.fill)
    }

    /// The untextured rectangles painted at `rect`.
    fn plain_at(h: &Harness<'_>, rect: Rect) -> Vec<RectShape> {
        rects(h)
            .into_iter()
            .filter(|shape| shape.brush.is_none() && near_rect(shape.rect, rect))
            .collect()
    }

    /// The fill painted over the whole of `rect`, if any but a transparent one is.
    fn fill_at(h: &Harness<'_>, rect: Rect) -> Option<(Color32, CornerRadius)> {
        plain_at(h, rect)
            .into_iter()
            .find(|shape| shape.fill != Color32::TRANSPARENT)
            .map(|shape| (shape.fill, shape.corner_radius))
    }

    /// The focus ring round `rect`: `:focus-visible`'s 1-point outline of `--blue-hi`.
    #[track_caller]
    fn ringed(h: &Harness<'_>, rect: Rect) -> bool {
        rects(h).into_iter().any(|shape| {
            shape.stroke.color == css::BLUE_HI
                && shape.stroke.width == 1.0
                && shape.fill == Color32::TRANSPARENT
                && shape.stroke_kind == StrokeKind::Outside
                && near_rect(shape.rect, rect)
        })
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn near_rect(a: Rect, b: Rect) -> bool {
        near(a.min.x, b.min.x)
            && near(a.min.y, b.min.y)
            && near(a.max.x, b.max.x)
            && near(a.max.y, b.max.y)
    }

    #[track_caller]
    fn at(got: Pos2, want: Pos2, what: &str) {
        assert!(
            near(got.x, want.x) && near(got.y, want.y),
            "{what}: {got:?}, not {want:?}"
        );
    }

    #[track_caller]
    fn sized(got: Rect, want: Rect, what: &str) {
        assert!(near_rect(got, want), "{what}: {got:?}, not {want:?}");
    }

    /// A text's colour as painted: its section's, or the shape's own for a placeholder.
    fn colour(shown: &TextShape, section: usize) -> Color32 {
        let format = &shown.galley.job.sections[section].format;
        if format.color == Color32::PLACEHOLDER {
            shown.fallback_color
        } else {
            format.color
        }
    }

    fn format(shown: &TextShape, section: usize) -> &Format {
        &shown.galley.job.sections[section].format
    }

    /// A button's rectangle, from the accessibility tree.
    fn button(h: &Harness<'_>, label: &str) -> Rect {
        h.get_by_role_and_label(egui::accesskit::Role::Button, label)
            .rect()
    }

    /// The left edges of the page's two columns in a window `window` wide, with the right
    /// column's width: the page `min(1080, the window less the margins)` wide, centred,
    /// padded 40 at each side, its columns 240 and the rest, 48 apart.
    fn columns(window: f32) -> (f32, f32, f32) {
        let full = window - 2.0 * MARGIN;
        let width = full.min(css::WRAP_MAX);
        let side = MARGIN + (full - width) / 2.0 + css::WRAP_SIDE;
        let right = side + css::SIDE + css::GAP;
        (
            side,
            right,
            width - 2.0 * css::WRAP_SIDE - css::SIDE - css::GAP,
        )
    }

    #[test]
    fn the_page_and_its_side_column_are_laid_out_as_the_prototypes_rules() {
        // 1296 wide: the page 1080 of the 1280 between the margins, so the side column at
        // 8 + 100 + 40 = 148, the right at 148 + 240 + 48 = 436; 816 wide: the page all
        // 800, the side column at 48 and the right at 336.
        assert_eq!(columns(1296.0), (148.0, 436.0, 712.0));
        assert_eq!(columns(816.0), (48.0, 336.0, 432.0));
        let shown = Shown {
            recents: None,
            thumbs: None,
            workspace_known: true,
            now: Some(NOW),
            keys: true,
        };
        let mut h = drawn(|ui| {
            page(ui, &shown);
        });
        for window in [WINDOW.x, 816.0] {
            h.set_size(Vec2::new(window, WINDOW.y));
            h.run_steps(2);
            let (left, right, _) = columns(window);
            let top = MARGIN + css::WRAP_TOP;
            // The brand: the logo, 26 square, white, at the column's top left; the name 10
            // beyond it, 18 points, white.
            let logo = Rect::from_min_size(Pos2::new(left, top), Vec2::splat(css::LOGO));
            assert_eq!(icon_at(&h, logo), Some(css::G1000), "the logo at {logo:?}");
            let name = text(&h, "Silico");
            assert!(
                near(name.pos.x, logo.right() + css::BRAND_GAP),
                "{:?}",
                name.pos
            );
            assert_eq!(
                format(&name, 0).font_id,
                FontId::proportional(css::BRAND_SIZE)
            );
            assert_eq!(colour(&name, 0), css::G1000);
            // New part and Open…: the column's width, 34 high, 26 below the brand and 4 apart.
            let new = button(&h, "New part");
            let new_top = logo.bottom() + css::BRAND_MARGIN;
            sized(
                new,
                Rect::from_min_size(
                    Pos2::new(left, new_top),
                    Vec2::new(css::SIDE, css::ACTION_HEIGHT),
                ),
                "New part",
            );
            let open = button(&h, "Open…");
            sized(
                open,
                Rect::from_min_size(
                    Pos2::new(left, new.bottom() + css::ACTIONS_GAP),
                    Vec2::new(css::SIDE, css::ACTION_HEIGHT),
                ),
                "Open…",
            );
            // Keyboard shortcuts, 4 below Open… (Search commands, the prototype's third, waits
            // for the command search).
            let shortcuts = button(&h, "Keyboard shortcuts");
            sized(
                shortcuts,
                Rect::from_min_size(
                    Pos2::new(left, open.bottom() + css::ACTIONS_GAP),
                    Vec2::new(css::SIDE, css::ACTION_HEIGHT),
                ),
                "Keyboard shortcuts",
            );
            assert!(
                h.query_by_role_and_label(egui::accesskit::Role::Button, "Search commands")
                    .is_none()
            );
            // The note's rule, 28 below them, the column's width, 1 of Gray.250; its text 16
            // below the rule, wrapped at the column's width.
            let rule_top = shortcuts.bottom() + css::NOTE_MARGIN;
            let rule = painted(&h)
                .into_iter()
                .find_map(|shape| match shape {
                    Shape::LineSegment { points, stroke } if stroke.color == css::G250 => {
                        Some((points, stroke))
                    }
                    _ => None,
                })
                .expect("the note's rule");
            at(
                rule.0[0],
                Pos2::new(left, rule_top + 0.5),
                "the rule's start",
            );
            at(
                rule.0[1],
                Pos2::new(left + css::SIDE, rule_top + 0.5),
                "the rule's end",
            );
            assert_eq!(rule.1.width, 1.0);
            let note = painted(&h)
                .into_iter()
                .find_map(|shape| match shape {
                    Shape::Text(shown) if shown.galley.text().starts_with("Parts are plain ") => {
                        Some(shown)
                    }
                    _ => None,
                })
                .expect("the note");
            at(
                note.pos,
                Pos2::new(left, rule_top + 1.0 + css::NOTE_PADDING),
                "the note",
            );
            assert_eq!(note.galley.job.wrap.max_width, css::SIDE);
            // Its words subdued, 11 on 17; `.silico` and `silico serve` in `.mono`, Gray.750.
            let words: Vec<(&str, FontId, Color32, Option<f32>)> = note
                .galley
                .job
                .sections
                .iter()
                .map(|section| {
                    (
                        &note.galley.job.text[section.byte_range.start.0..section.byte_range.end.0],
                        section.format.font_id.clone(),
                        section.format.color,
                        section.format.line_height,
                    )
                })
                .collect();
            let plain = (
                FontId::proportional(css::FS_SMALL),
                css::G550,
                Some(css::NOTE_LINE_HEIGHT),
            );
            let mono = (
                FontId::monospace(css::MONO),
                css::G750,
                Some(css::NOTE_LINE_HEIGHT),
            );
            let want = [
                ("Parts are plain ", &plain),
                (".silico", &mono),
                (" files. Agents edit the same files through ", &plain),
                ("silico serve", &mono),
            ];
            for (n, (said, format)) in want.into_iter().enumerate() {
                assert_eq!(
                    words[n],
                    (said, format.0.clone(), format.1, format.2),
                    "the note's section {n}"
                );
            }
            assert_eq!(
                (&words[4].1, words[4].2, words[4].3),
                (&plain.0, plain.1, plain.2)
            );
            // Copy MCP config, 10 below the note, 6 left of the column.
            let copy = button(&h, "Copy MCP config");
            at(
                copy.min,
                Pos2::new(
                    left - css::BTN_PADDING,
                    note.pos.y + note.galley.size().y + css::COPY_MARGIN,
                ),
                "Copy MCP config",
            );
            // The right column: its header, 11 points, at its top left.
            let header = text(&h, "RECENT");
            at(header.pos, Pos2::new(right, top), "Recent's header");
            assert_eq!(
                format(&header, 0).font_id,
                FontId::proportional(css::FS_SMALL)
            );
            assert_eq!(colour(&header, 0), css::G550);
            let none = text(&h, NOT_KEPT);
            assert!(near(none.pos.x, right), "{:?}", none.pos);
        }
    }

    #[test]
    fn the_actions_and_their_key_caps_are_drawn_as_st_action_and_kbd() {
        let shown = Shown {
            recents: None,
            thumbs: None,
            workspace_known: true,
            now: Some(NOW),
            keys: true,
        };
        let mut h = drawn(|ui| {
            page(ui, &shown);
        });
        // egui_kittest's platform is neither macOS nor Windows: Command is Ctrl. Keyboard
        // shortcuts' one cap is `?` (app.js: `<span class="kbd">?</span>`).
        for (label, caps, primary) in [
            ("New part", &["N", "Ctrl"][..], true),
            ("Open…", &["O", "Ctrl"][..], false),
            ("Keyboard shortcuts", &["?"][..], false),
        ] {
            let rect = button(&h, label);
            let (fill, ink, cap_fill, cap_border, cap_ink) = if primary {
                (
                    Some(css::G800),
                    css::G250,
                    Color32::TRANSPARENT,
                    css::KBD_ON_PRIMARY_BORDER,
                    css::KBD_ON_PRIMARY_TEXT,
                )
            } else {
                (None, css::G750, css::G250, css::G350, css::G800)
            };
            assert_eq!(
                fill_at(&h, rect),
                fill.map(|fill| (fill, CornerRadius::same(css::R))),
                "{label}"
            );
            // The icon, 14 square, 10 in; the label 10 beyond it, 12 points.
            let icon = Rect::from_min_size(
                Pos2::new(
                    rect.left() + css::ACTION_PADDING,
                    rect.center().y - css::ICON / 2.0,
                ),
                Vec2::splat(css::ICON),
            );
            assert_eq!(icon_at(&h, icon), Some(ink), "{label}'s icon at {icon:?}");
            let named = text(&h, label);
            at(
                named.pos,
                Pos2::new(
                    icon.right() + css::ACTION_GAP,
                    rect.center().y - named.galley.size().y / 2.0,
                ),
                label,
            );
            assert_eq!(format(&named, 0).font_id, FontId::proportional(css::FS));
            assert_eq!(colour(&named, 0), ink);
            // Its caps, Ctrl and the key, the last 10 from the right, 2 apart: each 16 high
            // on the action's middle, its text's width with 4 of padding and 1 of border at
            // each side (16 at least), its border 1 and 2 at its bottom, its text centred
            // across and on its 16 line under its top border, 10.5 points.
            let mut end = rect.right() - css::ACTION_PADDING;
            for &cap in caps {
                let shown = texts(&h, cap)
                    .into_iter()
                    .find(|shown| rect.contains(shown.pos))
                    .unwrap_or_else(|| panic!("{label}'s {cap}"));
                let size = shown.galley.size();
                let width =
                    (size.x + 2.0 * (css::KBD_PADDING + css::KBD_BORDER)).max(css::KBD_MIN_WIDTH);
                let want = Rect::from_min_max(
                    Pos2::new(end - width, rect.center().y - css::KBD_HEIGHT / 2.0),
                    Pos2::new(end, rect.center().y + css::KBD_HEIGHT / 2.0),
                );
                let drawn = rects(&h)
                    .into_iter()
                    .find(|shape| {
                        shape.corner_radius == CornerRadius::same(css::KBD_RADIUS)
                            && near_rect(shape.rect, want)
                    })
                    .unwrap_or_else(|| panic!("{label}'s {cap} cap at {want:?}"));
                assert_eq!(drawn.fill, cap_fill, "{label}'s {cap}");
                assert!(
                    drawn.stroke.is_empty(),
                    "{label}'s {cap}: {:?}",
                    drawn.stroke
                );
                crate::modal::tests::kbd_border(
                    &crate::modal::tests::cap_border(&h, want),
                    want,
                    cap_border,
                    &format!("{label}'s {cap}"),
                );
                at(
                    shown.pos,
                    Pos2::new(
                        want.center().x,
                        want.top() + css::KBD_BORDER + css::KBD_LINE_HEIGHT / 2.0,
                    ) - size / 2.0,
                    cap,
                );
                assert_eq!(
                    format(&shown, 0).font_id,
                    FontId::proportional(css::KBD_SIZE)
                );
                assert_eq!(colour(&shown, 0), cap_ink, "{label}'s {cap}");
                end = want.left() - css::KEYS_GAP;
            }
            // "Ctrl" is wider than 6 points: its cap is wider than the least.
            let ctrl = texts(&h, "Ctrl").remove(0).galley.size().x;
            assert!(ctrl + 10.0 > css::KBD_MIN_WIDTH, "{ctrl}");
        }
        // Hovered: Open… and Keyboard shortcuts Gray.250 with white ink (`.st-action:hover`),
        // New part white with black (`.st-action.primary:hover`).
        for (label, fill, ink) in [
            ("Open…", css::G250, css::G1000),
            ("Keyboard shortcuts", css::G250, css::G1000),
            ("New part", css::G1000, css::PRIMARY_HOVER_TEXT),
        ] {
            h.get_by_role_and_label(egui::accesskit::Role::Button, label)
                .hover();
            h.run_steps(2);
            let rect = button(&h, label);
            assert_eq!(
                fill_at(&h, rect),
                Some((fill, CornerRadius::same(css::R))),
                "{label} hovered"
            );
            let icon = Rect::from_min_size(
                Pos2::new(
                    rect.left() + css::ACTION_PADDING,
                    rect.center().y - css::ICON / 2.0,
                ),
                Vec2::splat(css::ICON),
            );
            assert_eq!(icon_at(&h, icon), Some(ink), "{label}'s icon hovered");
            assert_eq!(colour(&text(&h, label), 0), ink, "{label} hovered");
            assert!(!ringed(&h, rect));
        }
        // Focused from the keyboard, the pointer gone: the focus ring round it.
        h.remove_cursor();
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Open…")
            .focus();
        h.run_steps(2);
        let open = button(&h, "Open…");
        assert!(ringed(&h, open));
        assert!(!ringed(&h, button(&h, "New part")));
    }

    #[test]
    fn caps_take_their_widths_and_one_gap_between_each_two() {
        // `.keys { gap: 2px }` between caps, none outside them; each `.kbd` its text's width
        // with 4 of padding and 1 of border at each side, 16 at least. One cap, two, and
        // four, so no gap count n − 1 times 2 shares a value with n − 1 plus 2 (as three
        // caps' would: 4 both ways).
        let sets: [&[&str]; 3] = [&["I"], &["Ctrl", "N"], &["Ctrl", "Shift", "Alt", "Z"]];
        let measured = std::cell::RefCell::new(Vec::new());
        // Drawn, then dropped: the harness holds the closure's borrow of `measured`.
        drop(drawn(|ui| {
            let mut found = measured.borrow_mut();
            found.clear();
            for set in sets {
                let caps: Vec<String> = set.iter().map(|cap| (*cap).to_owned()).collect();
                let texts: Vec<f32> = set
                    .iter()
                    .map(|cap| {
                        ui.painter()
                            .layout_no_wrap(
                                (*cap).to_owned(),
                                FontId::proportional(css::KBD_SIZE),
                                Color32::WHITE,
                            )
                            .size()
                            .x
                    })
                    .collect();
                found.push((texts, key_caps_width(ui, &caps)));
            }
        }));
        let measured = measured.into_inner();
        assert_eq!(measured.len(), sets.len());
        let cap =
            |text: f32| (text + 2.0 * (css::KBD_PADDING + css::KBD_BORDER)).max(css::KBD_MIN_WIDTH);
        for (set, (texts, got)) in sets.iter().zip(&measured) {
            let gaps = (set.len() - 1) as f32 * css::KEYS_GAP;
            let want = texts.iter().map(|&text| cap(text)).sum::<f32>() + gaps;
            assert!(near(*got, want), "{set:?}: {got}, not {want}");
        }
        // "I" is narrower than 6 points, so its cap is the least, 16, and one cap has no gap.
        assert!(
            measured[0].0[0] + 10.0 < css::KBD_MIN_WIDTH,
            "{:?}",
            measured[0].0
        );
        assert!(near(measured[0].1, css::KBD_MIN_WIDTH), "{}", measured[0].1);
    }

    #[test]
    fn the_shortcuts_caps_are_the_platforms_names_for_command() {
        // The prototype's KEYNAME: Mod is ⌘ on macOS, Ctrl elsewhere.
        let ctx = egui::Context::default();
        for (os, command) in [
            (egui::os::OperatingSystem::Mac, "⌘"),
            (egui::os::OperatingSystem::Nix, "Ctrl"),
            (egui::os::OperatingSystem::Windows, "Ctrl"),
        ] {
            ctx.set_os(os);
            assert_eq!(
                Command::New.caps(&ctx),
                [command.to_owned(), "N".to_owned()],
                "{os:?}"
            );
        }
    }

    #[test]
    fn copy_mcp_config_is_drawn_as_a_ghost_btn() {
        let shown = Shown {
            recents: None,
            thumbs: None,
            workspace_known: true,
            now: Some(NOW),
            keys: true,
        };
        let mut h = drawn(|ui| {
            page(ui, &shown);
        });
        let (left, _, _) = columns(WINDOW.x);
        for hovered in [false, true] {
            if hovered {
                h.get_by_role_and_label(egui::accesskit::Role::Button, "Copy MCP config")
                    .hover();
                h.run_steps(2);
            }
            let rect = button(&h, "Copy MCP config");
            let label = text(&h, "Copy MCP config");
            let size = label.galley.size();
            // 6 of padding, the icon, 6, the label, 6: 22 high, 6 left of the column.
            sized(
                rect,
                Rect::from_min_size(
                    Pos2::new(left - css::BTN_PADDING, rect.top()),
                    Vec2::new(
                        css::BTN_PADDING + css::ICON + css::BTN_GAP + size.x + css::BTN_PADDING,
                        css::BTN_HEIGHT,
                    ),
                ),
                "Copy MCP config",
            );
            let icon = Rect::from_min_size(
                Pos2::new(
                    rect.left() + css::BTN_PADDING,
                    rect.center().y - css::ICON / 2.0,
                ),
                Vec2::splat(css::ICON),
            );
            at(
                label.pos,
                Pos2::new(icon.right() + css::BTN_GAP, rect.center().y - size.y / 2.0),
                "Copy MCP config's label",
            );
            assert_eq!(format(&label, 0).font_id, FontId::proportional(css::FS));
            // `.btn-ghost`: Gray.750 at rest; hovered, Gray.300 behind white.
            let (fill, ink) = if hovered {
                (Some((css::G300, CornerRadius::same(css::R_SM))), css::G1000)
            } else {
                (None, css::G750)
            };
            assert_eq!(fill_at(&h, rect), fill, "hovered {hovered}");
            assert_eq!(icon_at(&h, icon), Some(ink), "its icon, hovered {hovered}");
            assert_eq!(colour(&label, 0), ink, "its label, hovered {hovered}");
        }
        h.remove_cursor();
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Copy MCP config")
            .focus();
        h.run_steps(2);
        assert!(ringed(&h, button(&h, "Copy MCP config")));
    }

    /// A list read of `paths`, each looked for: marked missing, or given its time.
    fn listing(paths: &[&str]) -> Recents {
        let bytes = crate::recent::text(paths).into_bytes();
        let dir = tempfile::tempdir().unwrap();
        let mut recents = Recents::reading(
            dir.path().join("recent-documents"),
            Box::new(move |_| Some(bytes)),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !(recents.list().len() == paths.len()
            && recents
                .list()
                .iter()
                .all(|recent| recent.missing || recent.modified.is_some()))
        {
            recents.poll();
            assert!(std::time::Instant::now() < deadline, "never told");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        recents
    }

    use crate::recent::tests::modified_at;

    /// Twelve minutes before [`NOW`].
    const TWELVE_MIN_AGO: Timestamp = Timestamp::constant(1_794_753_000 - 12 * 60, 0);

    #[test]
    fn recents_rows_are_drawn_as_recent_row() {
        let dir = tempfile::tempdir().unwrap();
        let bracket = dir.path().join("bracket.silico");
        std::fs::write(&bracket, b"").unwrap();
        modified_at(&bracket, TWELVE_MIN_AGO);
        let gone = dir.path().join("fixtures").join("vise-jaw.silico");
        let recents = listing(&[bracket.to_str().unwrap(), gone.to_str().unwrap()]);
        let shown = Shown {
            recents: Some(&recents),
            thumbs: None,
            workspace_known: true,
            now: Some(NOW),
            keys: true,
        };
        let mut h = drawn(|ui| {
            page(ui, &shown);
        });
        let (_, left, width) = columns(WINDOW.x);
        let header = text(&h, "RECENT");
        // Each row the column's width, 52 high (6 + the thumb's 40 + 6), 1 apart.
        let first = button(&h, "bracket.silico");
        let second = button(&h, "vise-jaw.silico");
        let height = css::ROW_PADDING_Y + css::THUMB_HEIGHT + css::ROW_PADDING_Y;
        sized(
            first,
            Rect::from_min_size(
                Pos2::new(
                    left,
                    header.pos.y + header.galley.size().y + css::HEADER_MARGIN,
                ),
                Vec2::new(width, height),
            ),
            "the first row",
        );
        sized(
            second,
            Rect::from_min_size(
                Pos2::new(left, first.bottom() + css::RECENT_GAP),
                Vec2::new(width, height),
            ),
            "the second row",
        );
        let folder = dir.path().to_str().unwrap();
        let fixtures = gone.parent().unwrap().to_str().unwrap();
        for (row, name, at_folder, missing) in [
            (first, "bracket.silico", folder, false),
            (second, "vise-jaw.silico", fixtures, true),
        ] {
            // The thumb, 52 by 40, 6 in: Gray.0, a border of Gray.250, `--r-sm`.
            let thumb = Rect::from_min_size(
                Pos2::new(
                    row.left() + css::ROW_PADDING_LEFT,
                    row.center().y - css::THUMB_HEIGHT / 2.0,
                ),
                Vec2::new(css::THUMB_WIDTH, css::THUMB_HEIGHT),
            );
            let drawn = plain_at(&h, thumb);
            assert_eq!(drawn.len(), 1, "{name}'s thumb at {thumb:?}");
            assert_eq!(
                (drawn[0].fill, drawn[0].stroke, drawn[0].corner_radius),
                (
                    css::G0,
                    Stroke::new(1.0, css::G250),
                    CornerRadius::same(css::R_SM)
                ),
                "{name}'s thumb"
            );
            let icon = Rect::from_center_size(thumb.center(), Vec2::splat(css::ICON));
            let tint = icon_at(&h, icon);
            if missing {
                // `.recent-row.missing .thumb { color: var(--g400) }`.
                assert_eq!(tint, Some(css::G400));
            } else {
                assert!(tint.is_some(), "{name}'s icon");
            }
            // Its name and folder in two lines of 16 on the row's middle, 14 beyond the
            // thumb, cut short 14 (the grid's gap) before its right column (`.when`: its
            // time, or Remove from list), which ends 10 from its right.
            let when = text(
                &h,
                if missing {
                    "Remove from list"
                } else {
                    "12 min ago"
                },
            );
            let text_left = thumb.right() + css::ROW_GAP;
            // Laid out at a whole point's width: epaint rounds a wrap's width (its
            // `Fonts::layout_job`, against re-wrapping from frame to frame).
            let cut = (row.right()
                - css::ROW_PADDING_RIGHT
                - when.galley.size().x
                - css::ROW_GAP
                - text_left)
                .round();
            let named = text(&h, name);
            at(
                named.pos,
                Pos2::new(text_left, row.center().y - css::LH),
                name,
            );
            assert_eq!(named.galley.job.wrap.max_width, cut, "{name}'s cut");
            let (ink, struck) = if missing {
                // `.recent-row.missing .nm`: Gray.550, struck through.
                (css::G550, Stroke::new(1.0, css::G550))
            } else {
                (css::G1000, Stroke::NONE)
            };
            let name_format = format(&named, 0);
            assert_eq!(
                (
                    &name_format.font_id,
                    name_format.color,
                    name_format.strikethrough,
                    name_format.line_height
                ),
                (&FontId::proportional(css::FS), ink, struck, Some(css::LH)),
                "{name}'s name"
            );
            let place = if missing {
                format!("{at_folder} · not found")
            } else {
                at_folder.to_owned()
            };
            let placed = text(&h, &place);
            at(
                placed.pos,
                Pos2::new(text_left, row.center().y),
                "its folder",
            );
            assert_eq!(
                placed.galley.job.wrap.max_width, cut,
                "{name}'s folder's cut"
            );
            let folder_format = format(&placed, 0);
            assert_eq!(
                (
                    &folder_format.font_id,
                    folder_format.color,
                    folder_format.line_height
                ),
                (&FontId::monospace(css::MONO), css::G550, Some(css::LH)),
                "{name}'s folder"
            );
            if missing {
                // `.err-text`, 11 points.
                let not_found = format(&placed, 1);
                assert_eq!(
                    (&not_found.font_id, not_found.color, not_found.line_height),
                    (
                        &FontId::proportional(css::FS_SMALL),
                        css::RED,
                        Some(css::LH)
                    ),
                );
            } else {
                assert_eq!(placed.galley.job.sections.len(), 1);
            }
            assert_eq!(fill_at(&h, row), None, "{name} at rest");
        }
        // Hovered: Gray.200 over the row (`.recent-row:hover`); focused, the ring too.
        h.get_by_role_and_label(egui::accesskit::Role::Button, "bracket.silico")
            .hover();
        h.run_steps(2);
        assert_eq!(
            fill_at(&h, first),
            Some((css::G200, CornerRadius::same(css::R)))
        );
        assert_eq!(fill_at(&h, second), None);
        h.remove_cursor();
        h.get_by_role_and_label(egui::accesskit::Role::Button, "vise-jaw.silico")
            .focus();
        h.run_steps(2);
        assert_eq!(
            fill_at(&h, second),
            Some((css::G200, CornerRadius::same(css::R)))
        );
        assert!(ringed(&h, second));
        assert!(!ringed(&h, first));
    }

    #[test]
    fn a_list_not_kept_says_so_and_not_that_it_is_empty() {
        let recents = Recents::unkept();
        let shown = Shown {
            recents: Some(&recents),
            thumbs: None,
            workspace_known: true,
            now: Some(NOW),
            keys: true,
        };
        let h = drawn(|ui| {
            page(ui, &shown);
        });
        assert_eq!(texts(&h, NOT_KEPT).len(), 1);
        assert!(
            texts(
                &h,
                "No recent documents yet: each you open or make is listed here."
            )
            .is_empty()
        );
    }

    #[test]
    fn the_ready_dot_is_state_dot_green() {
        let h = drawn(|ui| {
            ui.horizontal(|ui| warm_status(ui, Warming::Ready));
        });
        let dots: Vec<CircleShape> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Circle(circle) => Some(circle),
                _ => None,
            })
            .collect();
        assert_eq!(dots.len(), 1);
        assert_eq!(
            (dots[0].radius, dots[0].fill),
            (css::STATE_DOT / 2.0, css::GREEN)
        );
    }

    // ---- When a recent document was modified (`.when`) ----

    /// `text` as a time, from RFC 3339.
    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    /// [`when`] of `then` in `zone`, at `now`.
    fn said(zone: &TimeZone, then: &str, now: &str) -> String {
        when(ts(then), ts(now), zone)
    }

    #[test]
    fn a_time_is_said_as_the_prototypes_when_by_the_rule_written_down() {
        use jiff::tz::{TimeZone, offset};
        let utc = TimeZone::UTC;
        // Each worked by hand from the rule in `when`'s documentation.
        for (then, now, want) in [
            // Less than a minute: just now; a minute: minutes; an hour: hours, on the day.
            ("2026-11-15T14:30:00Z", "2026-11-15T14:30:00Z", "Just now"),
            (
                "2026-11-15T14:29:00.000000001Z",
                "2026-11-15T14:30:00Z",
                "Just now",
            ),
            ("2026-11-15T14:29:00Z", "2026-11-15T14:30:00Z", "1 min ago"),
            ("2026-11-15T13:30:01Z", "2026-11-15T14:30:00Z", "59 min ago"),
            ("2026-11-15T13:30:00Z", "2026-11-15T14:30:00Z", "1 h ago"),
            ("2026-11-15T00:00:00Z", "2026-11-15T14:30:00Z", "14 h ago"),
            ("2026-11-15T00:00:00Z", "2026-11-15T23:59:59Z", "23 h ago"),
            // The day before: yesterday, from its first second to its last.
            ("2026-11-14T23:59:59Z", "2026-11-15T14:30:00Z", "Yesterday"),
            ("2026-11-14T00:00:00Z", "2026-11-15T14:30:00Z", "Yesterday"),
            // Before that: the date, this year's without the year.
            ("2026-11-13T23:59:59Z", "2026-11-15T14:30:00Z", "Nov 13"),
            ("2026-10-05T12:00:00Z", "2026-11-15T14:30:00Z", "Oct 5"),
            ("2026-01-01T00:00:00Z", "2026-11-15T14:30:00Z", "Jan 1"),
            (
                "2025-12-31T23:59:59Z",
                "2026-11-15T14:30:00Z",
                "Dec 31, 2025",
            ),
            (
                "2001-02-03T12:00:00Z",
                "2026-11-15T14:30:00Z",
                "Feb 3, 2001",
            ),
            // Midnight: minutes across it, then yesterday, never hours.
            ("2026-11-14T23:45:00Z", "2026-11-15T00:30:00Z", "45 min ago"),
            ("2026-11-15T00:00:00Z", "2026-11-15T00:30:00Z", "30 min ago"),
            ("2026-11-14T23:30:00Z", "2026-11-15T00:30:00Z", "Yesterday"),
            // The year's end: yesterday is last year's, the day before it a date with its
            // year.
            ("2026-12-31T23:55:00Z", "2027-01-01T00:10:00Z", "15 min ago"),
            ("2026-12-31T23:00:00Z", "2027-01-01T00:10:00Z", "Yesterday"),
            (
                "2026-12-30T12:00:00Z",
                "2027-01-01T00:10:00Z",
                "Dec 30, 2026",
            ),
            // A leap day.
            ("2028-02-29T10:00:00Z", "2028-03-01T10:00:00Z", "Yesterday"),
            ("2028-02-28T23:00:00Z", "2028-03-01T10:00:00Z", "Feb 28"),
            // The clock behind the file's time: under a minute, just now; else its date.
            ("2026-11-15T14:30:59Z", "2026-11-15T14:30:00Z", "Just now"),
            ("2026-11-15T14:31:00Z", "2026-11-15T14:30:00Z", "Nov 15"),
            ("2026-11-16T09:00:00Z", "2026-11-15T14:30:00Z", "Nov 16"),
            (
                "2027-01-02T00:00:00Z",
                "2026-11-15T14:30:00Z",
                "Jan 2, 2027",
            ),
        ] {
            assert_eq!(said(&utc, then, now), want, "{then} at {now}");
        }
        // The days are the time zone's. At +09:00, 23:00Z on the 14th is 08:00 on the
        // 15th and 00:30Z on the 15th is 09:30: the same day, an hour and a half apart.
        let tokyo = TimeZone::fixed(offset(9));
        let (then, now) = ("2026-11-14T23:00:00Z", "2026-11-15T00:30:00Z");
        assert_eq!(said(&tokyo, then, now), "1 h ago");
        assert_eq!(said(&utc, then, now), "Yesterday");
        // At -05:00, 06:00Z on the 14th is 01:00 on the 14th and 03:00Z on the 15th is
        // 22:00 on the 14th: 21 hours on one day.
        let east = TimeZone::fixed(offset(-5));
        let (then, now) = ("2026-11-14T06:00:00Z", "2026-11-15T03:00:00Z");
        assert_eq!(said(&east, then, now), "21 h ago");
        assert_eq!(said(&utc, then, now), "Yesterday");
        // And so is the year: 03:00Z on 2026-01-01 is 22:00 on 2025-12-31 at -05:00, and
        // 12:00Z on 2025-12-29 is 07:00 that day.
        let (then, now) = ("2025-12-29T12:00:00Z", "2026-01-01T03:00:00Z");
        assert_eq!(said(&east, then, now), "Dec 29");
        assert_eq!(said(&utc, then, now), "Dec 29, 2025");
        // A day the clocks go back (New York's rule: 2026-11-01, a Sunday, at 02:00 EDT,
        // 06:00Z): from 00:30 EDT (04:30Z) to 12:00 EST (17:00Z) the clock moves 11.5 h but
        // 12.5 h pass, and the hours said are those that passed.
        let new_york = TimeZone::posix("EST5EDT,M3.2.0,M11.1.0").unwrap();
        assert_eq!(
            said(&new_york, "2026-11-01T04:30:00Z", "2026-11-01T17:00:00Z"),
            "12 h ago"
        );
        // That day has 25 hours: from 00:05 EDT (04:05Z) to 23:55 EST (04:55Z the next
        // day) is 24 h 50 min on one date.
        assert_eq!(
            said(&new_york, "2026-11-01T04:05:00Z", "2026-11-02T04:55:00Z"),
            "24 h ago"
        );
    }

    #[test]
    fn only_a_silico_file_is_a_document_to_drop_and_the_rest_are_named() {
        for (path, document) in [
            ("/a/part.silico", true),
            ("/a/part.SILICO", true),
            ("/a/part.v2.silico", true),
            ("/a/part.silico.txt", false),
            ("/a/part", false),
            ("/a/silico", false),
            ("/a/.silico", false),
            ("/a/photo.png", false),
        ] {
            assert_eq!(names_a_document(Path::new(path)), document, "{path}");
        }
        assert_eq!(
            not_documents(&["photo.png".to_owned()]),
            "photo.png isn't a .silico document, so it wasn't opened"
        );
        assert_eq!(
            not_documents(&["photo.png".to_owned(), "notes.txt".to_owned()]),
            "photo.png, notes.txt aren't .silico documents, so they weren't opened"
        );
        // Three named, then how many more: of 4, one more; of 1000, 997.
        let named =
            |count: usize| -> Vec<String> { (0..count).map(|n| format!("{n}.txt")).collect() };
        assert_eq!(
            not_documents(&named(3)),
            "0.txt, 1.txt, 2.txt aren't .silico documents, so they weren't opened"
        );
        assert_eq!(
            not_documents(&named(4)),
            "0.txt, 1.txt, 2.txt and 1 more aren't .silico documents, so they weren't opened"
        );
        assert_eq!(
            not_documents(&named(1000)),
            "0.txt, 1.txt, 2.txt and 997 more aren't .silico documents, so they weren't opened"
        );
        assert_eq!(
            not_opened_quitting(&named(1000)),
            "Quitting: 0.txt, 1.txt, 2.txt and 997 more not opened"
        );
        assert_eq!(
            not_opened_asking(&["b.silico".to_owned()]),
            "Answer the question first: b.silico not opened"
        );
        assert_eq!(
            not_opened_until_closed("What's Wrong", &named(2)),
            "Close What's Wrong first: 0.txt, 1.txt not opened"
        );
    }

    /// `.link`'s `--info`, and `.recent-row .when`'s size.
    const INFO: Color32 = Color32::from_rgb(0xad, 0xbb, 0xff);

    #[test]
    fn each_rows_time_or_its_remove_from_list_is_drawn_as_when_at_its_right() {
        let dir = tempfile::tempdir().unwrap();
        let bracket = dir.path().join("bracket.silico");
        let plate = dir.path().join("plate.silico");
        for (path, time) in [
            (&bracket, TWELVE_MIN_AGO),
            // 2001-02-03 12:00 UTC: 11,356 days (31 years of 365 and 8 leap days, then 33)
            // of 86,400 s, and 43,200. Feb 3, 2001 in any zone within 12 hours of UTC.
            (&plate, Timestamp::constant(981_201_600, 0)),
        ] {
            std::fs::write(path, b"").unwrap();
            modified_at(path, time);
        }
        let gone = dir.path().join("vise-jaw.silico");
        let recents = listing(&[
            bracket.to_str().unwrap(),
            plate.to_str().unwrap(),
            gone.to_str().unwrap(),
        ]);
        let acts = std::cell::RefCell::new(Vec::new());
        let mut h = drawn(|ui| {
            let shown = Shown {
                recents: Some(&recents),
                thumbs: None,
                workspace_known: true,
                now: Some(NOW),
                keys: true,
            };
            let act = page(ui, &shown);
            if act != Act::None {
                acts.borrow_mut().push(act);
            }
        });
        for (name, when, ink) in [
            ("bracket.silico", "12 min ago", css::G550),
            ("plate.silico", "Feb 3, 2001", css::G550),
            ("vise-jaw.silico", "Remove from list", INFO),
        ] {
            let row = button(&h, name);
            let shown = text(&h, when);
            // Right-aligned 10 from the row's right (its padding), on its middle, 11 points
            // on a 16 line.
            at(
                shown.pos + shown.galley.size(),
                Pos2::new(
                    row.right() - css::ROW_PADDING_RIGHT,
                    row.center().y + shown.galley.size().y / 2.0,
                ),
                when,
            );
            assert!(near(shown.galley.size().y, css::LH), "{when}");
            let format = format(&shown, 0);
            assert_eq!(
                (&format.font_id, colour(&shown, 0), format.underline),
                (&FontId::proportional(css::FS_SMALL), ink, Stroke::NONE),
                "{when}"
            );
        }
        // In the tree: each time a label, the missing one's link a link.
        for when in ["12 min ago", "Feb 3, 2001"] {
            assert!(
                h.query_by_role_and_label(egui::accesskit::Role::Label, when)
                    .is_some(),
                "{when}"
            );
        }
        let link = h.get_by_role_and_label(egui::accesskit::Role::Link, "Remove from list");
        assert_eq!(
            link.accesskit_node().description().as_deref(),
            Some(gone.to_str().unwrap())
        );
        // Hovered, the link is white and underlined (`.link:hover`), and its row is lit
        // under it, as the button it's in is.
        link.hover();
        h.run_steps(2);
        let hovered = text(&h, "Remove from list");
        assert_eq!(colour(&hovered, 0), css::G1000);
        assert_eq!(format(&hovered, 0).underline, Stroke::new(1.0, css::G1000));
        let missing_row = button(&h, "vise-jaw.silico");
        assert_eq!(
            fill_at(&h, missing_row),
            Some((css::G200, CornerRadius::same(css::R)))
        );
        // Its click removes the document and opens nothing; the row's own click opens it.
        h.get_by_role_and_label(egui::accesskit::Role::Link, "Remove from list")
            .click();
        h.run_steps(2);
        assert_eq!(
            *acts.borrow(),
            [Act::Remove(gone.to_str().unwrap().to_owned())]
        );
        acts.borrow_mut().clear();
        h.get_by_role_and_label(egui::accesskit::Role::Button, "vise-jaw.silico")
            .click();
        h.run_steps(2);
        assert_eq!(
            *acts.borrow(),
            [Act::Recent(gone.to_str().unwrap().to_owned())]
        );
    }

    /// The row labelled `name` has the keyboard's focus.
    fn focused(h: &Harness<'_>, name: &str) -> bool {
        h.get_by_role_and_label(egui::accesskit::Role::Button, name)
            .accesskit_node()
            .is_focused()
    }

    /// `key` pressed twice in one frame, then a frame more.
    fn twice_in_a_frame(h: &mut Harness<'_>, key: Key) {
        for pressed in [true, false, true, false] {
            h.input_mut().events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            });
        }
        h.run_steps(2);
    }

    #[test]
    fn up_and_down_move_the_focus_along_recent_stopping_at_its_ends_and_enter_opens() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["one.silico", "two.silico", "three.silico"];
        let paths: Vec<String> = names
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                std::fs::write(&path, b"").unwrap();
                path.to_str().unwrap().to_owned()
            })
            .collect();
        let listed: Vec<&str> = paths.iter().map(String::as_str).collect();
        let recents = listing(&listed);
        let keys = std::cell::Cell::new(true);
        let acts = std::cell::RefCell::new(Vec::new());
        let mut h = drawn(|ui| {
            let shown = Shown {
                recents: Some(&recents),
                thumbs: None,
                workspace_known: true,
                now: Some(NOW),
                keys: keys.get(),
            };
            let act = page(ui, &shown);
            if act != Act::None {
                acts.borrow_mut().push(act);
            }
        });
        #[track_caller]
        fn only(h: &Harness<'_>, name: Option<&str>) {
            for row in ["one.silico", "two.silico", "three.silico"] {
                assert_eq!(focused(h, row), Some(row) == name, "{row}, want {name:?}");
            }
        }
        only(&h, None);
        // Nothing focused: ↓ focuses the first row (interact.js: from -1, the next is 0);
        // then each ↓ the next, the last staying.
        for want in ["one.silico", "two.silico", "three.silico", "three.silico"] {
            h.key_press(Key::ArrowDown);
            h.run_steps(2);
            only(&h, Some(want));
        }
        // ↑ back, the first staying.
        for want in ["two.silico", "one.silico", "one.silico"] {
            h.key_press(Key::ArrowUp);
            h.run_steps(2);
            only(&h, Some(want));
        }
        // Drawn focused: the row's fill and its ring.
        let one = button(&h, "one.silico");
        assert_eq!(
            fill_at(&h, one),
            Some((css::G200, CornerRadius::same(css::R)))
        );
        assert!(ringed(&h, one));
        // Two presses in one frame (egui_kittest's key_press steps a frame for each event it
        // queues, so they're put in the frame's input together) move two rows.
        twice_in_a_frame(&mut h, Key::ArrowDown);
        only(&h, Some("three.silico"));
        // From the middle row, two in one frame stop at the end they run into: the last
        // (one row on, then held there) and the first (one row back, then held).
        for (press, want) in [
            (Key::ArrowDown, "three.silico"),
            (Key::ArrowUp, "one.silico"),
        ] {
            h.get_by_role_and_label(egui::accesskit::Role::Button, "two.silico")
                .focus();
            h.run_steps(2);
            only(&h, Some("two.silico"));
            twice_in_a_frame(&mut h, press);
            only(&h, Some(want));
        }
        // With a modifier, an arrow isn't Recent's.
        h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowDown);
        h.run_steps(2);
        only(&h, Some("one.silico"));
        // Enter opens the row focused, as its click does.
        assert!(acts.borrow().is_empty());
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert_eq!(*acts.borrow(), [Act::Recent(paths[0].clone())]);
        // From another of the page's controls, ↑ goes to Recent's first row, not to the
        // control above it.
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Open…")
            .focus();
        h.run_steps(2);
        h.key_press(Key::ArrowUp);
        h.run_steps(2);
        only(&h, Some("one.silico"));
        // Not the page's while it isn't given the keys (a field typed in, a modal, a menu):
        // from Open…, ↑ is egui's own, which moves to the widget above it, never to a row.
        keys.set(false);
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Open…")
            .focus();
        h.run_steps(2);
        h.key_press(Key::ArrowUp);
        h.run_steps(2);
        only(&h, None);
    }

    #[test]
    fn recents_header_says_its_keys_at_its_right_only_with_rows_to_move_along() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.silico");
        std::fs::write(&one, b"").unwrap();
        let recents = listing(&[one.to_str().unwrap()]);
        let shown = Shown {
            recents: Some(&recents),
            thumbs: None,
            workspace_known: true,
            now: Some(NOW),
            keys: true,
        };
        let h = drawn(|ui| {
            page(ui, &shown);
        });
        let (_, left, width) = columns(WINDOW.x);
        let header = text(&h, "RECENT");
        // "opens", at the column's right, 11 points, Gray.550, on the header's line.
        let opens = text(&h, KEYS_HINT);
        assert!(
            near(opens.pos.x + opens.galley.size().x, left + width),
            "{:?}",
            opens.pos
        );
        assert_eq!(
            (&format(&opens, 0).font_id, colour(&opens, 0)),
            (&FontId::proportional(css::FS_SMALL), css::G550)
        );
        let middle = header.pos.y + header.galley.size().y / 2.0;
        assert!(near(opens.pos.y + opens.galley.size().y / 2.0, middle));
        // A space's width before it (the inline text's), as the header's font lays one out.
        let space = {
            let font = FontId::proportional(css::FS_SMALL);
            let fonts = |text: &str| {
                h.ctx
                    .fonts_mut(|fonts| {
                        fonts.layout_no_wrap(text.to_owned(), font.clone(), css::G550)
                    })
                    .size()
                    .x
            };
            fonts(" opens") - fonts("opens")
        };
        assert!(space > 0.0);
        // The caps, `.kbd` at rest (Gray.250 in Gray.350, Gray.800 letters), each 16 high on
        // the header's middle: ↵ a space before "opens", then a space, then ↑ and ↓ side by
        // side, no gap between their `.keys`.
        let mut end = opens.pos.x - space;
        for (cap, gap_after) in [("↵", space), ("↓", 0.0), ("↑", 0.0)] {
            let shown = text(&h, cap);
            let size = shown.galley.size();
            let width =
                (size.x + 2.0 * (css::KBD_PADDING + css::KBD_BORDER)).max(css::KBD_MIN_WIDTH);
            let want = Rect::from_min_max(
                Pos2::new(end - width, middle - css::KBD_HEIGHT / 2.0),
                Pos2::new(end, middle + css::KBD_HEIGHT / 2.0),
            );
            let drawn = rects(&h)
                .into_iter()
                .find(|shape| {
                    shape.corner_radius == CornerRadius::same(css::KBD_RADIUS)
                        && near_rect(shape.rect, want)
                })
                .unwrap_or_else(|| panic!("{cap}'s cap at {want:?}"));
            assert_eq!(drawn.fill, css::G250, "{cap}");
            crate::modal::tests::kbd_border(
                &crate::modal::tests::cap_border(&h, want),
                want,
                css::G350,
                cap,
            );
            assert_eq!(colour(&shown, 0), css::G800, "{cap}");
            assert_eq!(
                format(&shown, 0).font_id,
                FontId::proportional(css::KBD_SIZE)
            );
            end = want.left() - gap_after;
        }
        // No rows: no keys to say.
        let empty = Recents::unkept();
        let shown = Shown {
            recents: Some(&empty),
            thumbs: None,
            workspace_known: true,
            now: Some(NOW),
            keys: true,
        };
        let h = drawn(|ui| {
            page(ui, &shown);
        });
        assert!(texts(&h, KEYS_HINT).is_empty());
        assert!(texts(&h, "↵").is_empty());
    }

    #[test]
    fn a_rounded_outlines_corners_are_never_rounder_than_half_its_width_or_height() {
        // A radius of 5.5 on a rect 10 wide and 4 high is 2 (half its height): its first
        // point, the top right corner's arc's start, a quarter turn before its right, is
        // (10 - 2, 0); on one 4 wide and 10 high, 2 (half its width), (4 - 2, 0). Every point
        // within the rect (each corner's arc stays inside its own corner).
        for (size, first) in [
            (Vec2::new(10.0, 4.0), Pos2::new(8.0, 0.0)),
            (Vec2::new(4.0, 10.0), Pos2::new(2.0, 0.0)),
        ] {
            let rect = Rect::from_min_size(Pos2::ZERO, size);
            let outline = rounded_outline(rect, 5.5);
            assert_eq!(outline.len(), 4 * 9 + 1);
            assert!(
                (outline[0] - first).length() < 1e-5,
                "{size:?}: {:?}",
                outline[0]
            );
            assert_eq!(outline.first(), outline.last());
            for point in &outline {
                assert!(
                    rect.expand(1e-5).contains(*point),
                    "{size:?}: {point:?} outside"
                );
            }
        }
    }

    #[test]
    fn the_drop_notes_words_wrap_inside_its_padding_and_border_at_every_width() {
        // `.drop-note`'s `padding: 14px` and 1 px border each side: its words wrap in its
        // width less 30. At each width from 120 to 420 a point apart, the widest of its
        // lines is no wider than that, so no line runs into the padding by even a point,
        // and the narrow ones take more than one line.
        const INSET: f32 = 2.0 * (14.0 + 1.0);
        let mut wrapped = 0;
        for width in 120..=420_u16 {
            let available = std::cell::Cell::new(0.0_f32);
            let mut h = Harness::builder()
                .with_size(Vec2::new(f32::from(width), 300.0))
                .build_ui(|ui| {
                    available.set(ui.available_width());
                    drop_note(ui);
                });
            theme::apply(&h.ctx);
            h.run_steps(2);
            let said = painted(&h)
                .into_iter()
                .find_map(|shape| match shape {
                    Shape::Text(shown) if shown.galley.text().starts_with("Drop a ") => Some(shown),
                    _ => None,
                })
                .expect("the drop note");
            let room = available.get() - INSET;
            assert!(
                said.galley.size().x <= room + 0.01,
                "at {width}: a line {} wide in {room}",
                said.galley.size().x
            );
            if said.galley.size().y > 1.5 * css::LH {
                wrapped += 1;
            }
        }
        assert!(wrapped > 50, "{wrapped} widths wrapped");
    }

    #[test]
    fn the_drop_note_is_drawn_as_drop_note_under_recent() {
        // `.drop-note`: `margin-top: 26px; padding: 14px; border: 1px dashed var(--g350);
        // border-radius: var(--r); color: var(--g550); text-align: center`, after `.recent`
        // and its `margin-bottom: 30px` (Samples, between them in the prototype, not built:
        // the two margins meet, and the larger stands).
        const NOTE_MARGIN: f32 = 30.0;
        const NOTE_PADDING: f32 = 14.0;
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.silico");
        std::fs::write(&one, b"").unwrap();
        let recents = listing(&[one.to_str().unwrap()]);
        let shown = Shown {
            recents: Some(&recents),
            thumbs: None,
            workspace_known: true,
            now: Some(NOW),
            keys: true,
        };
        let h = drawn(|ui| {
            page(ui, &shown);
        });
        let (_, left, width) = columns(WINDOW.x);
        let row = button(&h, "one.silico");
        // `.recent`'s gap is between its rows, none after the last.
        let top = row.bottom() + NOTE_MARGIN;
        let height = 1.0 + NOTE_PADDING + css::LH + NOTE_PADDING + 1.0;
        let note = Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, height));
        // Its words: "Drop a ", `.silico` in `.mono`, and the rest, 12 points on 16, Gray.550,
        // their line centred across it, 1 + 14 below its top.
        let said = painted(&h)
            .into_iter()
            .find_map(|shape| match shape {
                Shape::Text(shown) if shown.galley.text().starts_with("Drop a ") => Some(shown),
                _ => None,
            })
            .expect("the drop note");
        assert_eq!(
            said.galley.text(),
            "Drop a .silico file anywhere in this window to open it."
        );
        let sections: Vec<(&str, FontId, Color32)> = said
            .galley
            .job
            .sections
            .iter()
            .map(|section| {
                (
                    &said.galley.job.text[section.byte_range.start.0..section.byte_range.end.0],
                    section.format.font_id.clone(),
                    section.format.color,
                )
            })
            .collect();
        assert_eq!(
            sections,
            [
                ("Drop a ", FontId::proportional(css::FS), css::G550),
                (".silico", FontId::monospace(css::MONO), css::G550),
                (
                    " file anywhere in this window to open it.",
                    FontId::proportional(css::FS),
                    css::G550
                ),
            ]
        );
        // Its line as laid out (its advance, not its ink), centred: within half a point,
        // as egui starts a centred line on a whole point.
        let drawn = said.galley.rect.translate(said.pos.to_vec2());
        assert!(
            (drawn.center().x - note.center().x).abs() <= 0.5,
            "{drawn:?} in {note:?}"
        );
        assert!(near(said.pos.y, top + 1.0 + NOTE_PADDING), "{:?}", said.pos);
        assert!(near(said.galley.size().y, css::LH));
        // Its border: dashes of Gray.350, 1 wide, along the note's edge (its stroke's middle
        // half a point in), round its corners.
        let dashes: Vec<[Pos2; 2]> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::LineSegment { points, stroke } if stroke.color == css::G350 => {
                    assert_eq!(stroke.width, 1.0);
                    Some(points)
                }
                _ => None,
            })
            .collect();
        assert!(dashes.len() > 20, "{} dashes", dashes.len());
        // Each dash's ends on the stroke's middle: the note's edge half a point in, its
        // corners `--r` (6) less that half point round. A point's distance from that rounded
        // rectangle (its signed distance: the corner's centre box, then the radius) is
        // nothing, within the chords that stand for its corners' arcs (8 a quarter turn:
        // 5.5 × (1 − cos(π/32)), 0.03).
        let edge = note.shrink(0.5);
        let radius = f32::from(css::R) - 0.5;
        let off = |point: Pos2| {
            let half = edge.size() / 2.0 - Vec2::splat(radius);
            let q = (point - edge.center()).abs() - half;
            q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - radius
        };
        let mut reach = Rect::NOTHING;
        for point in dashes.iter().flatten() {
            assert!(
                off(*point).abs() < 0.05,
                "{point:?}: {} off {edge:?}",
                off(*point)
            );
            reach = reach.union(Rect::from_min_max(*point, *point));
        }
        // They go all the way round: their reach is within half a point of the edge each
        // way.
        for (got, want) in [
            (reach.left(), edge.left()),
            (reach.right(), edge.right()),
            (reach.top(), edge.top()),
            (reach.bottom(), edge.bottom()),
        ] {
            assert!((got - want).abs() < 0.5, "{reach:?} against {edge:?}");
        }
    }
}
