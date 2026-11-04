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
pub(crate) fn shortcut_caps(ctx: &egui::Context, key: Key) -> Vec<String> {
    caps(ctx, Modifiers::COMMAND, key)
}

/// A cap for each of `modifiers`, Command (Ctrl off macOS) first, then Shift, then `key`'s,
/// as the platform names them (the prototype's `kbd`, in its `Mod Shift` order).
pub(crate) fn caps(ctx: &egui::Context, modifiers: Modifiers, key: Key) -> Vec<String> {
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
        .chain(std::iter::once(key.symbol_or_name().to_owned()))
        .collect()
}

/// The width `caps` take as [`key_caps`] paints them.
pub(crate) fn key_caps_width(ui: &Ui, caps: &[String]) -> f32 {
    let gaps = caps.len().saturating_sub(1) as f32 * theme::KEY_CAP_GAP;
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

/// A key cap's width, for text `text` wide: its padding and its outline at each side.
fn key_cap_width(text: f32) -> f32 {
    (text + 2.0 * (theme::KEY_CAP_PADDING + theme::KEY_CAP_BORDER)).max(theme::KEY_CAP_MIN_WIDTH)
}

/// `caps` painted as key caps, the last ending at `right` (centred on its height).
pub(crate) fn key_caps(ui: &Ui, right: Pos2, caps: &[String], primary: bool) {
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
        let width = key_cap_width(galley.size().x);
        let rect = Rect::from_min_max(
            Pos2::new(x - width, right.y - theme::KEY_CAP_HEIGHT / 2.0),
            Pos2::new(x, right.y + theme::KEY_CAP_HEIGHT / 2.0),
        );
        painter.rect(
            rect,
            theme::KEY_CAP_CORNER_RADIUS,
            fill,
            Stroke::new(theme::KEY_CAP_BORDER, stroke),
            StrokeKind::Inside,
        );
        painter.galley(rect.center() - galley.size() / 2.0, galley, text);
        x = rect.left() - theme::KEY_CAP_GAP;
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
    use egui_kittest::kittest::Queryable;

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
        /// border-radius: 3px; border: 1px; font-size: 10.5px` (`border-box`, `*`'s).
        pub const KEYS_GAP: f32 = 2.0;
        pub const KBD_MIN_WIDTH: f32 = 16.0;
        pub const KBD_PADDING: f32 = 4.0;
        pub const KBD_BORDER: f32 = 1.0;
        pub const KBD_HEIGHT: f32 = 16.0;
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
            workspace_known: true,
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
            // The note's rule, 28 below them, the column's width, 1 of Gray.250; its text 16
            // below the rule, wrapped at the column's width.
            let rule_top = open.bottom() + css::NOTE_MARGIN;
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
            workspace_known: true,
        };
        let mut h = drawn(|ui| {
            page(ui, &shown);
        });
        // egui_kittest's platform is neither macOS nor Windows: Command is Ctrl.
        for (label, key, primary) in [("New part", "N", true), ("Open…", "O", false)] {
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
            // each side (16 at least), its text centred, 10.5 points.
            let mut end = rect.right() - css::ACTION_PADDING;
            for cap in [key, "Ctrl"] {
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
                assert_eq!(
                    drawn.stroke,
                    Stroke::new(css::KBD_BORDER, cap_border),
                    "{label}'s {cap}"
                );
                assert_eq!(drawn.stroke_kind, StrokeKind::Inside);
                at(shown.pos, want.center() - size / 2.0, cap);
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
        // Hovered: Open… Gray.250 with white ink (`.st-action:hover`), New part white with
        // black (`.st-action.primary:hover`).
        for (label, fill, ink) in [
            ("Open…", css::G250, css::G1000),
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
                shortcut_caps(&ctx, Key::N),
                [command.to_owned(), "N".to_owned()],
                "{os:?}"
            );
        }
    }

    #[test]
    fn copy_mcp_config_is_drawn_as_a_ghost_btn() {
        let shown = Shown {
            recents: None,
            workspace_known: true,
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

    /// A list read of `paths`, each looked for.
    fn listing(paths: &[&str]) -> Recents {
        let bytes = crate::recent::text(paths).into_bytes();
        let dir = tempfile::tempdir().unwrap();
        let mut recents = Recents::reading(
            dir.path().join("recent-documents"),
            Box::new(move |_| Some(bytes)),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        // Read, then each looked for: told again if any is missing.
        while !recents.list().iter().any(|recent| recent.missing) {
            recents.poll();
            assert!(std::time::Instant::now() < deadline, "never told");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        recents
    }

    #[test]
    fn recents_rows_are_drawn_as_recent_row() {
        let dir = tempfile::tempdir().unwrap();
        let bracket = dir.path().join("bracket.silico");
        std::fs::write(&bracket, b"").unwrap();
        let gone = dir.path().join("fixtures").join("vise-jaw.silico");
        let recents = listing(&[bracket.to_str().unwrap(), gone.to_str().unwrap()]);
        let shown = Shown {
            recents: Some(&recents),
            workspace_known: true,
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
            // thumb, cut short 10 from its right.
            let text_left = thumb.right() + css::ROW_GAP;
            let cut = row.right() - css::ROW_PADDING_RIGHT - text_left;
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
            workspace_known: true,
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
}
