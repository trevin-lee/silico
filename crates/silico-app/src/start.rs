//! The start page: what the window shows with no document open, and the Home tab while
//! documents are (docs/design/m5-ui/DESIGN.md, *Start page or blank canvas?*, approved
//! 2026-11-01). New and Open, the documents opened lately (`crate::recent`), and the note
//! for agents with Copy MCP config, laid out as the prototype's `.st-wrap` lays them out: a
//! column of its own at the left, Recent at the right, centred in the window. The warm
//! start's state ([`Warm`]) is the status bar's while no document is open.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Align2, Color32, FontFamily, FontId, Key, Modifiers, Pos2, Rect, Response, RichText,
    Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType,
};

use crate::icons::{Icon, IconSize};
use crate::recent::{Recent, Recents};
use crate::theme;
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
    /// A recent document clicked, by its path.
    Recent(String),
    CopyMcpConfig,
}

/// What the start page shows besides its own controls.
pub(crate) struct Shown<'a> {
    /// The documents opened lately, if the app keeps a list.
    pub recents: Option<&'a Recents>,
    /// Whether Copy MCP config knows a workspace to name: if not, it says so.
    pub workspace_known: bool,
}

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
                        recent(ui, shown.recents, &mut act);
                    });
                });
                ui.add_space(theme::START_PADDING_BOTTOM);
            });
    });
    act
}

/// The left column: the logo and name, New and Open, and the note for agents.
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
    if action(ui, Icon::NewDoc, "New part", Key::N, true).clicked() {
        *act = Act::New;
    }
    ui.add_space(theme::START_ACTION_GAP);
    if action(ui, Icon::Open, "Open…", Key::O, false).clicked() {
        *act = Act::Open;
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

/// One of the left column's actions, the column's width: its icon, its label, and its
/// shortcut's key caps, the primary one filled as `button_primary` is.
fn action(ui: &mut Ui, icon: Icon, label: &str, key: Key, primary: bool) -> Response {
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
        &shortcut_caps(ui.ctx(), key),
        primary,
    );
    response
}

/// The caps of Command (Ctrl off macOS) and `key`, as the platform names them.
fn shortcut_caps(ctx: &egui::Context, key: Key) -> [String; 2] {
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let names = if mac {
        egui::ModifierNames::SYMBOLS
    } else {
        egui::ModifierNames::NAMES
    };
    [
        names.format(&Modifiers::COMMAND, mac),
        key.symbol_or_name().to_owned(),
    ]
}

/// `caps` painted as key caps, the last ending at `right` (centred on its height).
fn key_caps(ui: &Ui, right: Pos2, caps: &[String], primary: bool) {
    let painter = ui.painter();
    let (fill, stroke, text) = if primary {
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
    let mut x = right.x;
    for cap in caps.iter().rev() {
        let galley = painter.layout_no_wrap(
            cap.clone(),
            FontId::proportional(theme::KEY_CAP_TEXT_SIZE),
            text,
        );
        let width = (galley.size().x + 2.0 * theme::KEY_CAP_PADDING).max(theme::KEY_CAP_MIN_WIDTH);
        let rect = Rect::from_min_max(
            Pos2::new(x - width, right.y - theme::KEY_CAP_HEIGHT / 2.0),
            Pos2::new(x, right.y + theme::KEY_CAP_HEIGHT / 2.0),
        );
        painter.rect(
            rect,
            theme::KEY_CAP_CORNER_RADIUS,
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Inside,
        );
        painter.galley(rect.center() - galley.size() / 2.0, galley, text);
        x = rect.left() - theme::KEY_CAP_GAP;
    }
}

/// The focus ring round `rect`, if `response` has the keyboard's focus.
fn focus_ring(ui: &Ui, response: &Response, rect: Rect) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            theme::NORMAL_CORNER_RADIUS,
            Stroke::new(theme::FOCUS_WIDTH, theme::FOCUS),
            StrokeKind::Outside,
        );
    }
}

/// Copy MCP config, a ghost button whose icon lines up with the note's text above it.
fn copy_button(ui: &mut Ui) -> Response {
    const LABEL: &str = "Copy MCP config";
    let galley = ui.painter().layout_no_wrap(
        LABEL.to_owned(),
        FontId::proportional(theme::BODY_SIZE),
        theme::TEXT_DEFAULT,
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

/// A section's header: its name in capitals, subdued, letters spaced.
fn header(ui: &mut Ui, name: &str) {
    ui.label(
        RichText::new(name.to_uppercase())
            .size(theme::HEADER_SIZE)
            .color(theme::TEXT_SUBDUED)
            .extra_letter_spacing(theme::START_HEADER_LETTER_SPACING),
    );
    ui.add_space(theme::START_HEADER_GAP);
}

/// What Recent says when the app keeps no list: the platform named no folder for it, or its
/// thread couldn't be started.
pub const NOT_KEPT: &str =
    "Recent documents aren't being kept: there's nowhere to keep their list.";

/// The right column: the recent documents, newest first, labelled `Recent` for the
/// accessibility tree, each a button by its file's name.
fn recent(ui: &mut Ui, recents: Option<&Recents>, act: &mut Act) {
    header(ui, "Recent");
    ui.scope_builder(UiBuilder::new().id_salt("recent"), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::List);
            node.set_label("Recent");
        });
        let said = match recents {
            None => Some(NOT_KEPT),
            Some(recents) if !recents.is_kept() => Some(NOT_KEPT),
            Some(recents) if !recents.is_read() => Some("Reading the list of recent documents…"),
            Some(recents) if !recents.list().is_empty() => None,
            Some(_) => Some("No recent documents yet: each you open or make is listed here."),
        };
        if let Some(said) = said {
            ui.label(RichText::new(said).color(theme::TEXT_SUBDUED));
            return;
        }
        for recent in recents.map(Recents::list).unwrap_or_default() {
            if row(ui, recent).clicked() {
                *act = Act::Recent(recent.path.clone());
            }
            ui.add_space(theme::START_RECENT_ROW_GAP);
        }
    });
}

/// A recent document's row: its thumbnail's place (the document's icon; a warning's if
/// nothing was at its path), its name and its folder, and "not found" if nothing was.
fn row(ui: &mut Ui, recent: &Recent) -> Response {
    let size = Vec2::new(ui.available_width(), theme::START_RECENT_ROW_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
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
    if !ui.is_rect_visible(rect) {
        return response;
    }
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(
            rect,
            theme::NORMAL_CORNER_RADIUS,
            theme::START_RECENT_HOVER_BG,
        );
    }
    focus_ring(ui, &response, rect);
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
    let (icon, tint) = if recent.missing {
        (Icon::Warn, theme::TEXT_DISABLED)
    } else {
        (Icon::Doc, theme::TEXT_SUBDUED)
    };
    icon.image(IconSize::Small).tint(tint).paint_at(
        ui,
        Rect::from_center_size(thumb.center(), Vec2::splat(crate::icons::ICON_SIZE)),
    );
    let left = thumb.right() + theme::START_RECENT_COLUMN_GAP;
    let width = (rect.right() - theme::START_RECENT_PADDING_RIGHT - left).max(0.0);
    let mut name_job = LayoutJob::single_section(
        name,
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
    response
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
}
