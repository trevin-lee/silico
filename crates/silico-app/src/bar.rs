//! The application bar's look (docs/design/m5-ui/DESIGN.md, *Layout* and *egui mapping*;
//! styles.css's `#appbar`, `.ab-*`, and `.doc-tab`): its buttons, the separator, the open
//! documents' tabs, and the tooltips that name a button's shortcut. What each does is the
//! app's (`app`); this draws them, each with the label a test or a screen reader finds it by.
//!
//! On macOS the window's title bar is the app bar, as Rerun's is: the window's content runs
//! under the title bar ([`TITLE_BAR_IS_APP_BAR`]), the bar leaves room at its left for the
//! window's buttons ([`inset`]), and its empty space moves the window ([`move_window`]).

use std::sync::Arc;

use eframe::egui::{
    self, FontId, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
    ViewportCommand, WidgetInfo, WidgetType, text::LayoutJob, text::TextWrapping,
};

use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::shortcuts::Chord;
use crate::theme;

/// Whether the window's content runs under its title bar, the app bar drawn there (macOS,
/// where eframe can: `with_fullsize_content_view`).
pub const TITLE_BAR_IS_APP_BAR: bool = cfg!(target_os = "macos");

/// The bar's room at its left: for macOS's window buttons where the title bar is the app
/// bar and the window isn't full screen (which hides them), else the bar's own padding.
pub(crate) fn inset(ui: &mut Ui) {
    let full_screen = ui.input(|input| input.viewport().fullscreen.unwrap_or(false));
    let room = if TITLE_BAR_IS_APP_BAR && !full_screen {
        theme::TRAFFIC_LIGHTS_WIDTH
    } else {
        theme::APP_BAR_PADDING
    };
    // `add_space` takes no item spacing after it, where `.traffic` has the bar's gap.
    ui.add_space(room + theme::APP_BAR_ITEM_SPACING);
}

/// The bar's empty space at `rect`, where the title bar is the app bar: dragged, it moves
/// the window, and double-clicked, it zooms it, as a title bar does. Called before the
/// bar's items, so they take the pointer first. Never focused: the keyboard's first stop is
/// Home.
pub(crate) fn move_window(ui: &Ui, rect: Rect) {
    if !TITLE_BAR_IS_APP_BAR {
        return;
    }
    // `Sense::CLICK`, not `Sense::click()`, which is focusable too.
    let background = ui.interact(rect, ui.id().with("move_window"), Sense::CLICK);
    if background.double_clicked() {
        let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if background.is_pointer_button_down_on() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// The bar's border below it, along `rect`'s bottom: painted first, so the shown
/// document's tab covers it.
pub(crate) fn border(ui: &Ui, rect: Rect) {
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - theme::BORDER_WIDTH / 2.0,
        Stroke::new(theme::BORDER_WIDTH, theme::APP_BAR_BORDER),
    );
}

/// A bar button (`.ab-btn`): `icon`, and `text` after it if given, labelled `label`; `on`
/// for one that shows a state, as Home does. Off unless `enabled`.
pub(crate) fn button(
    ui: &mut Ui,
    icon: Icon,
    text: Option<&str>,
    label: &str,
    enabled: bool,
    on: Option<bool>,
) -> Response {
    // Painted at the bar's opacity, not faded further when off: `.ab-btn:disabled` is
    // Gray.400 itself.
    let opacity = ui.opacity();
    ui.add_enabled(enabled, |ui: &mut Ui| {
        ui.set_opacity(opacity);
        let galley = text.map(|text| {
            ui.painter().layout_no_wrap(
                text.to_owned(),
                FontId::proportional(theme::BODY_SIZE),
                theme::TEXT_DEFAULT,
            )
        });
        let content = ICON_SIZE
            + galley
                .as_ref()
                .map_or(0.0, |galley| theme::APP_BAR_BUTTON_GAP + galley.size().x);
        let size = Vec2::new(
            (content + 2.0 * theme::APP_BAR_BUTTON_PADDING).max(theme::APP_BAR_BUTTON_SIZE),
            theme::APP_BAR_BUTTON_SIZE,
        );
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| match on {
            Some(on) => WidgetInfo::selected(WidgetType::Button, enabled, on, label),
            None => WidgetInfo::labeled(WidgetType::Button, enabled, label),
        });
        if !ui.is_rect_visible(rect) {
            return response;
        }
        // `.ab-btn:hover:not(:disabled)` outweighs `.ab-btn.on` (a class and two
        // pseudo-classes to two classes), so a hovered button takes hovered's fill.
        let (fill, color) = if !enabled {
            (None, theme::TEXT_DISABLED)
        } else if response.hovered() {
            (Some(theme::APP_BAR_BUTTON_HOVER_BG), theme::TEXT_STRONG)
        } else if on == Some(true) {
            (Some(theme::APP_BAR_BUTTON_ON_BG), theme::TEXT_STRONG)
        } else {
            (None, theme::TEXT_DEFAULT)
        };
        if let Some(fill) = fill {
            ui.painter()
                .rect_filled(rect, theme::SMALL_CORNER_RADIUS, fill);
        }
        theme::focus_ring(
            ui,
            &response,
            rect,
            theme::SMALL_CORNER_RADIUS,
            StrokeKind::Outside,
        );
        let left = rect.center().x - content / 2.0;
        let icon_rect = Rect::from_min_size(
            Pos2::new(left, rect.center().y - ICON_SIZE / 2.0),
            Vec2::splat(ICON_SIZE),
        );
        icon.image(IconSize::Small)
            .tint(color)
            .paint_at(ui, icon_rect);
        if let Some(galley) = galley {
            let at = Pos2::new(
                icon_rect.right() + theme::APP_BAR_BUTTON_GAP,
                rect.center().y - galley.size().y / 2.0,
            );
            ui.painter()
                .galley_with_override_text_color(at, galley, color);
        }
        response
    })
}

/// The drop-down menu `add` fills, opened and closed by clicks on `response` (`.menu`):
/// the File menu, and the toolbar's.
pub(crate) fn menu(response: &Response, add: impl FnOnce(&mut Ui)) {
    let config = egui::containers::menu::MenuConfig::new();
    egui::Popup::menu(response)
        .close_behavior(config.close_behavior)
        .style(config.style.clone())
        .info(
            egui::UiStackInfo::new(egui::UiKind::Menu)
                .with_tag_value(egui::containers::menu::MenuConfig::MENU_CONFIG_TAG, config),
        )
        .show(add);
}

/// The separator between the bar's buttons and the documents' tabs (`.ab-sep`).
pub(crate) fn separator(ui: &mut Ui) {
    let size = Vec2::new(
        theme::BORDER_WIDTH + 2.0 * theme::APP_BAR_SEPARATOR_MARGIN,
        theme::APP_BAR_SEPARATOR_HEIGHT,
    );
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(theme::BORDER_WIDTH, theme::APP_BAR_SEPARATOR),
    );
}

/// `response` with a tooltip (`.tooltip`), on or off: `title`, the caps of `shortcut` after
/// it if given (as the registry names them: `crate::shortcuts::caps`), and `description`
/// under it if given.
pub(crate) fn tooltip(
    response: Response,
    title: &str,
    shortcut: Option<Chord>,
    description: Option<&str>,
) -> Response {
    let show = |ui: &mut Ui| tooltip_ui(ui, title, shortcut, description);
    response.on_hover_ui(show).on_disabled_hover_ui(show)
}

fn tooltip_ui(ui: &mut Ui, title: &str, shortcut: Option<Chord>, description: Option<&str>) {
    ui.spacing_mut().item_spacing.y = theme::TOOLTIP_DESCRIPTION_GAP;
    // As high as its text and its caps, not the height egui's `horizontal` assumes for a
    // row of controls.
    let row = Vec2::new(ui.available_size_before_wrap().x, theme::KEY_CAP_HEIGHT);
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(row, layout, |ui| {
        ui.spacing_mut().item_spacing.x = theme::TOOLTIP_TITLE_GAP;
        ui.label(RichText::new(title).color(theme::TEXT_STRONG));
        if let Some(shortcut) = shortcut {
            let caps = crate::shortcuts::caps(ui.ctx(), shortcut);
            let width = crate::start::key_caps_width(ui, &caps);
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(width, theme::KEY_CAP_HEIGHT), Sense::hover());
            crate::start::key_caps(ui, rect.right_center(), &caps, false);
        }
    });
    if let Some(description) = description {
        ui.label(
            RichText::new(description)
                .size(theme::HEADER_SIZE)
                .color(theme::TEXT_SUBDUED),
        );
    }
}

/// What an untitled part's tab says when hovered, with no edits: it has no file yet.
pub(crate) const NOT_SAVED_YET: &str = "Not saved to a file yet";

/// An open document's tab, as [`doc_tab`] draws it.
pub(crate) struct DocTab<'a> {
    /// The document's file name.
    pub name: &'a str,
    /// Its path, which the tab's tooltip gives: none for an untitled part, whose tooltip
    /// gives its name and that it has no file yet.
    pub path: Option<&'a str>,
    /// Whether it holds edits not yet saved: an asterisk after its name.
    pub unsaved: bool,
    /// Whether it's the document shown (not while Home is).
    pub active: bool,
    /// Whether it's the first tab, which has a border at its left too.
    pub first: bool,
}

/// What a tab answered: a click on it (show the document), and on its close button; and
/// where it is, its borders and all.
pub(crate) struct DocTabResponse {
    pub tab: Response,
    pub close: Response,
    pub rect: Rect,
}

impl DocTabResponse {
    /// Whether the whole tab is in the strip's view, or as much of it as fits, if it's
    /// wider than the strip; else asked to scroll there. Within half a point, as egui
    /// places widgets on a grid finer than that and a scroll may stop short by a step.
    pub(crate) fn in_view(&self, ui: &Ui) -> bool {
        let view = ui.clip_rect().x_range().expand(0.5);
        let seen = view.contains(self.rect.left()) && view.contains(self.rect.right())
            || self.rect.width() >= view.span();
        if !seen {
            ui.scroll_to_rect(self.rect, None);
        }
        seen
    }
}

/// A document's tab (`.doc-tab`), the bar's height less its border, which the shown one's
/// fill covers: its name, ending in an ellipsis past [`theme::DOC_TAB_MAX_WIDTH`]; an
/// asterisk if it holds edits; and its close button, at its right, drawn while the tab is
/// hovered or shown (`.x`'s `opacity`), or either has the keyboard's focus, though there,
/// and clickable, always, as the prototype's is. A click anywhere else on the tab shows
/// the document. Labelled
/// with its name, and the asterisk, and selected while shown; its close button labelled
/// "Close" and its name.
pub(crate) fn doc_tab(ui: &mut Ui, tab: &DocTab<'_>) -> DocTabResponse {
    let font = FontId::proportional(theme::BODY_SIZE);
    let dirty = tab.unsaved.then(|| {
        ui.painter()
            .layout_no_wrap(" *".to_owned(), font.clone(), theme::DOC_TAB_DIRTY)
    });
    let borders = theme::BORDER_WIDTH * if tab.first { 2.0 } else { 1.0 };
    let fixed = borders
        + theme::DOC_TAB_PADDING_LEFT
        + dirty.as_ref().map_or(0.0, |dirty| dirty.size().x)
        + theme::DOC_TAB_GAP
        + theme::DOC_TAB_CLOSE_SIZE
        + theme::DOC_TAB_PADDING_RIGHT;
    let room = (theme::DOC_TAB_MAX_WIDTH - fixed).max(0.0);
    let mut job = LayoutJob::simple_singleline(tab.name.to_owned(), font, theme::DOC_TAB_TEXT);
    job.wrap = TextWrapping::truncate_at_width(room);
    let name = ui.painter().layout_job(job);
    // A name cut short takes all its room, so the tab is its widest.
    let width = if name.elided { room } else { name.size().x };
    let size = Vec2::new(fixed + width, theme::TOP_BAR_HEIGHT - theme::BORDER_WIDTH);
    let (rect, area) = ui.allocate_exact_size(size, Sense::hover());
    let close_rect = Rect::from_center_size(
        Pos2::new(
            rect.right()
                - theme::BORDER_WIDTH
                - theme::DOC_TAB_PADDING_RIGHT
                - theme::DOC_TAB_CLOSE_SIZE / 2.0,
            rect.center().y,
        ),
        Vec2::splat(theme::DOC_TAB_CLOSE_SIZE),
    );
    // The whole tab shows its document but for its close button, which, added after it,
    // takes its own clicks.
    let response = ui.interact(rect, area.id.with("show"), Sense::click());
    let close = ui.interact(close_rect, area.id.with("close"), Sense::click());
    let label = if tab.unsaved {
        format!("{} *", tab.name)
    } else {
        tab.name.to_owned()
    };
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, tab.active, &label));
    // Told to a screen reader (and a test) as its name's side, up to the close button, so
    // the tab and its close button don't overlap and a click at the tab's centre is never
    // the close button's.
    let named = Rect::from_min_max(rect.min, Pos2::new(close_rect.left(), rect.bottom()));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_bounds(egui::accesskit::Rect {
            x0: named.min.x.into(),
            y0: named.min.y.into(),
            x1: named.max.x.into(),
            y1: named.max.y.into(),
        });
    });
    let closing = format!("Close {}", tab.name);
    close.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &closing));
    if ui.is_rect_visible(rect) {
        paint_tab(ui, tab, &area, &response, &close, name, dirty);
    }
    let description = match (tab.unsaved, tab.path) {
        (true, _) => Some("Unsaved edits"),
        (false, None) => Some(NOT_SAVED_YET),
        (false, Some(_)) => None,
    };
    let response = tooltip(response, tab.path.unwrap_or(tab.name), None, description);
    DocTabResponse {
        tab: response,
        close,
        rect,
    }
}

/// `tab` painted over its `area`.
fn paint_tab(
    ui: &Ui,
    tab: &DocTab<'_>,
    area: &Response,
    response: &Response,
    close: &Response,
    name: Arc<egui::Galley>,
    dirty: Option<Arc<egui::Galley>>,
) {
    let rect = area.rect;
    let painter = ui.painter();
    // Hovered anywhere on it, its close button too (`.doc-tab:hover` holds over its
    // children).
    let hovered = area.contains_pointer();
    // `.doc-tab.active` comes after `:hover`, so the shown tab keeps its look hovered.
    let text = if tab.active {
        // Down over the bar's border (`.doc-tab.active::after`).
        let fill = Rect::from_min_max(
            rect.min,
            Pos2::new(rect.right(), rect.bottom() + theme::BORDER_WIDTH),
        );
        painter.rect_filled(fill, 0.0, theme::DOC_TAB_ACTIVE_BG);
        theme::DOC_TAB_ACTIVE_TEXT
    } else if hovered {
        painter.rect_filled(rect, 0.0, theme::DOC_TAB_HOVER_BG);
        theme::DOC_TAB_HOVER_TEXT
    } else {
        theme::DOC_TAB_TEXT
    };
    let half = theme::BORDER_WIDTH / 2.0;
    let border = Stroke::new(theme::BORDER_WIDTH, theme::DOC_TAB_BORDER);
    painter.vline(rect.right() - half, rect.y_range(), border);
    let mut left = rect.left();
    if tab.first {
        painter.vline(left + half, rect.y_range(), border);
        left += theme::BORDER_WIDTH;
    }
    // Inside its borders, where the strip's clip leaves it whole.
    theme::focus_ring(
        ui,
        response,
        rect.shrink(theme::BORDER_WIDTH),
        0,
        StrokeKind::Inside,
    );
    let at = Pos2::new(
        left + theme::DOC_TAB_PADDING_LEFT,
        rect.center().y - name.size().y / 2.0,
    );
    let after = at.x + name.size().x;
    painter.galley_with_override_text_color(at, name, text);
    if let Some(dirty) = dirty {
        let y = rect.center().y - dirty.size().y / 2.0;
        painter.galley(Pos2::new(after, y), dirty, theme::DOC_TAB_DIRTY);
    }
    if !(hovered || tab.active || response.has_focus() || close.has_focus()) {
        return;
    }
    let icon = if close.hovered() {
        painter.rect_filled(
            close.rect,
            theme::DOC_TAB_CLOSE_CORNER_RADIUS,
            theme::DOC_TAB_CLOSE_HOVER_BG,
        );
        theme::DOC_TAB_CLOSE_HOVER
    } else {
        theme::DOC_TAB_CLOSE
    };
    theme::focus_ring(
        ui,
        close,
        close.rect,
        theme::DOC_TAB_CLOSE_CORNER_RADIUS,
        StrokeKind::Inside,
    );
    Icon::Close.image(IconSize::Small).tint(icon).paint_at(
        ui,
        Rect::from_center_size(close.rect.center(), Vec2::splat(ICON_SIZE)),
    );
}

/// The open documents' tabs, laid out by `add` in a row that scrolls across, by the wheel
/// or a trackpad, when they're wider than the room left in the bar, rather than squeezing
/// them; the shown tab's fill may run one point below the row, over the bar's border.
pub(crate) fn tab_strip<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let room = Vec2::new(
        (ui.available_width() - theme::APP_BAR_PADDING).max(0.0),
        ui.available_height(),
    );
    let top = egui::Layout::left_to_right(egui::Align::Min);
    ui.allocate_ui_with_layout(room, top, |ui| {
        // The row scrolls one way only, so the wheel's up and down scroll it across.
        ui.style_mut().always_scroll_the_only_direction = true;
        ui.spacing_mut().item_spacing.x = 0.0;
        egui::ScrollArea::horizontal()
            .id_salt("document_tabs")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                let clip = ui.clip_rect();
                ui.set_clip_rect(clip.with_max_y(clip.max.y + theme::BORDER_WIDTH));
                add(ui)
            })
            .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use eframe::egui::epaint::{ClippedShape, RectShape, TextShape};
    use eframe::egui::{Color32, CornerRadius, Key, Modifiers, Shape};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// The prototype's tokens and geometry (docs/design/m5-ui/styles.css).
    mod css {
        use eframe::egui::Color32;

        // `:root`'s greys and colours.
        pub const G150: Color32 = Color32::from_rgb(0x17, 0x17, 0x17);
        pub const G200: Color32 = Color32::from_rgb(0x21, 0x21, 0x21);
        pub const G250: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
        pub const G300: Color32 = Color32::from_rgb(0x2c, 0x2b, 0x2b);
        pub const G350: Color32 = Color32::from_rgb(0x52, 0x51, 0x51);
        pub const G400: Color32 = Color32::from_rgb(0x64, 0x63, 0x63);
        pub const G550: Color32 = Color32::from_rgb(0x93, 0x90, 0x90);
        pub const G750: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);
        pub const G800: Color32 = Color32::from_rgb(0xe0, 0xdf, 0xdf);
        pub const G1000: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
        /// `--blue-hi`: `:focus-visible`'s 1px outline.
        pub const BLUE_HI: Color32 = Color32::from_rgb(0x46, 0x76, 0xff);

        /// `--fs`, `--fs-small`, `--icon`, `--r-sm`, `--btn`, `--top-bar-h`.
        pub const FS: f32 = 12.0;
        pub const FS_SMALL: f32 = 11.0;
        pub const ICON: f32 = 14.0;
        pub const R_SM: u8 = 4;
        pub const BTN: f32 = 22.0;
        pub const TOP_BAR_H: f32 = 30.0;
        /// `#appbar`: `border-bottom: 1px`, `gap: 2px`; `.doc-tab`'s borders, 1px.
        pub const BORDER: f32 = 1.0;
        pub const BAR_GAP: f32 = 2.0;
        /// `.ab-btn`: `padding: 0 5px; gap: 4px`.
        pub const BTN_PADDING: f32 = 5.0;
        pub const BTN_GAP: f32 = 4.0;
        /// `.ab-sep`: `width: 1px; height: 16px; margin: 0 6px`.
        pub const SEP_WIDTH: f32 = 1.0;
        pub const SEP_HEIGHT: f32 = 16.0;
        pub const SEP_MARGIN: f32 = 6.0;
        /// `.doc-tab`: `padding: 0 6px 0 10px; gap: 6px`; `.x`: 16 square, `border-radius:
        /// 3px`.
        pub const TAB_PADDING_LEFT: f32 = 10.0;
        pub const TAB_PADDING_RIGHT: f32 = 6.0;
        pub const TAB_GAP: f32 = 6.0;
        pub const X: f32 = 16.0;
        pub const X_RADIUS: u8 = 3;
        /// `.tt-title`'s `gap: 8px`; `.tt-desc`'s `margin-top: 2px`; `.kbd`: `height: 16px;
        /// border-radius: 3px`, Gray.350 1 round and 2 at its bottom; `.keys`' `gap: 2px`.
        pub const TT_TITLE_GAP: f32 = 8.0;
        pub const TT_DESC_MARGIN: f32 = 2.0;
        pub const KBD_HEIGHT: f32 = 16.0;
        pub const KBD_RADIUS: u8 = 3;
        pub const KEYS_GAP: f32 = 2.0;
    }

    /// `app` drawn in the app's theme, its icons loaded.
    fn drawn<'a>(app: impl FnMut(&mut Ui) + 'a) -> Harness<'a> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(800.0, 200.0))
            .build_ui(app);
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    /// Every shape the last frame painted, nested ones flattened, each with its clip.
    fn clipped(h: &Harness<'_>) -> Vec<ClippedShape> {
        fn flatten(clip: Rect, shape: &Shape, out: &mut Vec<ClippedShape>) {
            match shape {
                Shape::Vec(shapes) => shapes.iter().for_each(|shape| flatten(clip, shape, out)),
                shape => out.push(ClippedShape {
                    clip_rect: clip,
                    shape: shape.clone(),
                }),
            }
        }
        let mut out = Vec::new();
        for shown in &h.output().shapes {
            flatten(shown.clip_rect, &shown.shape, &mut out);
        }
        out
    }

    fn rects(h: &Harness<'_>) -> Vec<(Rect, RectShape)> {
        clipped(h)
            .into_iter()
            .filter_map(|shown| match shown.shape {
                Shape::Rect(rect) => Some((shown.clip_rect, rect)),
                _ => None,
            })
            .collect()
    }

    /// The one text painted whose text is `text`.
    #[track_caller]
    fn text(h: &Harness<'_>, text: &str) -> TextShape {
        let mut found: Vec<TextShape> = clipped(h)
            .into_iter()
            .filter_map(|shown| match shown.shape {
                Shape::Text(shown) if shown.galley.text() == text => Some(shown),
                _ => None,
            })
            .collect();
        assert_eq!(found.len(), 1, "{text:?} painted {} times", found.len());
        found.remove(0)
    }

    /// A text's colour as painted: its override, else its first section's, else the
    /// shape's own for a placeholder.
    fn colour(shown: &TextShape) -> Color32 {
        let format = shown.galley.job.sections[0].format.color;
        shown
            .override_text_color
            .unwrap_or(if format == Color32::PLACEHOLDER {
                shown.fallback_color
            } else {
                format
            })
    }

    fn font(shown: &TextShape) -> FontId {
        shown.galley.job.sections[0].format.font_id.clone()
    }

    /// The tint of the icon painted at `rect` (a rectangle filled with its texture), to
    /// half a point: egui puts an image on the pixel grid.
    fn icon_at(h: &Harness<'_>, rect: Rect) -> Option<Color32> {
        rects(h)
            .into_iter()
            .find(|(_, shape)| {
                shape.brush.is_some()
                    && (shape.rect.min - rect.min).abs().max_elem() <= 0.5
                    && (shape.rect.max - rect.max).abs().max_elem() <= 0.5
            })
            .map(|(_, shape)| shape.fill)
    }

    /// The icons painted inside `rect`.
    fn icons_in(h: &Harness<'_>, rect: Rect) -> usize {
        rects(h)
            .into_iter()
            .filter(|(_, shape)| shape.brush.is_some() && rect.contains_rect(shape.rect))
            .count()
    }

    /// The fill painted over the whole of `rect`, if any but a transparent one is, its
    /// corners, and its clip.
    fn fill_at(h: &Harness<'_>, rect: Rect) -> Option<(Color32, CornerRadius, Rect)> {
        rects(h)
            .into_iter()
            .find(|(_, shape)| {
                shape.brush.is_none()
                    && shape.fill != Color32::TRANSPARENT
                    && near_rect(shape.rect, rect)
            })
            .map(|(clip, shape)| (shape.fill, shape.corner_radius, clip))
    }

    /// The focus ring at `rect`: `:focus-visible`'s 1 point outline of `--blue-hi`.
    fn ring_at(h: &Harness<'_>, rect: Rect) -> Option<(CornerRadius, StrokeKind)> {
        rects(h)
            .into_iter()
            .find(|(_, shape)| {
                shape.stroke == Stroke::new(1.0, css::BLUE_HI)
                    && shape.fill == Color32::TRANSPARENT
                    && near_rect(shape.rect, rect)
            })
            .map(|(_, shape)| (shape.corner_radius, shape.stroke_kind))
    }

    fn rings(h: &Harness<'_>) -> usize {
        rects(h)
            .into_iter()
            .filter(|(_, shape)| shape.stroke.color == css::BLUE_HI)
            .count()
    }

    /// The lines painted, each from one end to the other, with its stroke.
    fn lines(h: &Harness<'_>) -> Vec<([Pos2; 2], Stroke)> {
        clipped(h)
            .into_iter()
            .filter_map(|shown| match shown.shape {
                Shape::LineSegment { points, stroke } => Some((points, stroke)),
                _ => None,
            })
            .collect()
    }

    /// The vertical lines painted at `x`.
    fn lines_at_x(h: &Harness<'_>, x: f32) -> Vec<([Pos2; 2], Stroke)> {
        lines(h)
            .into_iter()
            .filter(|(points, _)| near(points[0].x, x) && near(points[1].x, x))
            .collect()
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

    fn widget(h: &Harness<'_>, label: &str) -> Rect {
        h.get_by_role_and_label(egui::accesskit::Role::Button, label)
            .rect()
    }

    /// A 14 point icon centred on `center`.
    fn icon_rect(center: Pos2) -> Rect {
        Rect::from_center_size(center, Vec2::splat(css::ICON))
    }

    #[test]
    fn the_bars_buttons_separator_and_border_are_drawn_as_ab_btn_ab_sep_and_appbar() {
        // A bar 400 wide, its border along its bottom; Home (on), File, Save (off), and
        // Undo in its row above the border, 2 apart as `#appbar`'s `gap`, then `.ab-sep`.
        let top = Rc::new(Cell::new(Pos2::ZERO));
        let seen = Rc::clone(&top);
        let mut h = drawn(move |ui| {
            let bar = Rect::from_min_size(ui.max_rect().min, Vec2::new(400.0, css::TOP_BAR_H));
            seen.set(bar.min);
            border(ui, bar);
            let row =
                Rect::from_min_max(bar.min, Pos2::new(bar.right(), bar.bottom() - css::BORDER));
            let layout = egui::Layout::left_to_right(egui::Align::Center);
            ui.scope_builder(egui::UiBuilder::new().max_rect(row).layout(layout), |ui| {
                ui.spacing_mut().item_spacing.x = css::BAR_GAP;
                button(ui, Icon::Home, None, "Home", true, Some(true));
                button(ui, Icon::Menu, Some("File"), "File", true, None);
                button(ui, Icon::Save, None, "Save", false, None);
                button(ui, Icon::Undo, None, "Undo", true, None);
                separator(ui);
            });
        });
        let top = top.get();
        // `#appbar`'s `border-bottom`: 1 of Gray.250 along the bar's last point.
        let border: Vec<_> = lines(&h)
            .into_iter()
            .filter(|(points, _)| near(points[0].y, points[1].y))
            .collect();
        assert_eq!(border.len(), 1, "{border:?}");
        let y = top.y + css::TOP_BAR_H - css::BORDER / 2.0;
        at(border[0].0[0], Pos2::new(top.x, y), "the border's start");
        at(
            border[0].0[1],
            Pos2::new(top.x + 400.0, y),
            "the border's end",
        );
        assert_eq!(border[0].1, Stroke::new(css::BORDER, css::G250));
        // The row's middle, above the border.
        let middle = top.y + (css::TOP_BAR_H - css::BORDER) / 2.0;
        // Each button 22 high on the row's middle; an icon alone 14 in 5 of padding at
        // each side; File's icon, 4, then its text, 12 points.
        let file = text(&h, "File");
        let file_width = css::ICON + css::BTN_GAP + file.galley.size().x;
        let mut left = top.x;
        let mut want = std::collections::BTreeMap::new();
        for (label, content) in [
            ("Home", css::ICON),
            ("File", file_width),
            ("Save", css::ICON),
            ("Undo", css::ICON),
        ] {
            let width = (content + 2.0 * css::BTN_PADDING).max(css::BTN);
            let rect = Rect::from_min_size(
                Pos2::new(left, middle - css::BTN / 2.0),
                Vec2::new(width, css::BTN),
            );
            sized(widget(&h, label), rect, label);
            want.insert(label, rect);
            left = rect.right() + css::BAR_GAP;
        }
        // At rest: Home on, Gray.250 behind white (`.ab-btn.on`); File and Undo Gray.750 on
        // nothing (`.ab-btn`); Save off, Gray.400 (`.ab-btn:disabled`). Each icon centred,
        // File's with its text, 4 beyond the icon, on the row's middle.
        let check = |h: &Harness<'_>, label: &str, fill: Option<Color32>, ink: Color32| {
            let rect = want[label];
            assert_eq!(
                fill_at(h, rect).map(|(fill, corners, _)| (fill, corners)),
                fill.map(|fill| (fill, CornerRadius::same(css::R_SM))),
                "{label}'s fill"
            );
            let content = if label == "File" {
                file_width
            } else {
                css::ICON
            };
            let icon = Rect::from_min_size(
                Pos2::new(
                    rect.center().x - content / 2.0,
                    rect.center().y - css::ICON / 2.0,
                ),
                Vec2::splat(css::ICON),
            );
            assert_eq!(icon_at(h, icon), Some(ink), "{label}'s icon at {icon:?}");
            if label == "File" {
                let file = text(h, "File");
                at(
                    file.pos,
                    Pos2::new(
                        icon.right() + css::BTN_GAP,
                        rect.center().y - file.galley.size().y / 2.0,
                    ),
                    "File's text",
                );
                assert_eq!(font(&file), FontId::proportional(css::FS));
                assert_eq!(colour(&file), ink, "File's text");
            }
        };
        check(&h, "Home", Some(css::G250), css::G1000);
        check(&h, "File", None, css::G750);
        check(&h, "Save", None, css::G400);
        check(&h, "Undo", None, css::G750);
        assert_eq!(rings(&h), 0);
        // `.ab-sep`: 1 of Gray.250, 16 high on the row's middle, 6 beyond Undo's gap.
        let x = want["Undo"].right() + css::BAR_GAP + css::SEP_MARGIN + css::SEP_WIDTH / 2.0;
        let separator = lines_at_x(&h, x);
        assert_eq!(separator.len(), 1, "the separator at {x}: {:?}", lines(&h));
        at(
            separator[0].0[0],
            Pos2::new(x, middle - css::SEP_HEIGHT / 2.0),
            "the separator's top",
        );
        at(
            separator[0].0[1],
            Pos2::new(x, middle + css::SEP_HEIGHT / 2.0),
            "the separator's bottom",
        );
        assert_eq!(separator[0].1, Stroke::new(css::SEP_WIDTH, css::G250));
        // Hovered: Gray.300 behind white (`.ab-btn:hover:not(:disabled)`), Home's too, over
        // `.on`; Save, off, unchanged.
        for (label, fill, ink) in [
            ("File", Some(css::G300), css::G1000),
            ("Home", Some(css::G300), css::G1000),
            ("Save", None, css::G400),
        ] {
            h.get_by_role_and_label(egui::accesskit::Role::Button, label)
                .hover();
            h.run_steps(2);
            check(&h, label, fill, ink);
        }
        // Focused from the keyboard, the pointer gone: the focus ring round Undo, its
        // corners `--r-sm`.
        h.remove_cursor();
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Undo")
            .focus();
        h.run_steps(2);
        assert_eq!(
            ring_at(&h, want["Undo"]),
            Some((CornerRadius::same(css::R_SM), StrokeKind::Outside))
        );
        assert_eq!(rings(&h), 1);
        check(&h, "Undo", None, css::G750);
    }

    #[test]
    fn the_tabs_are_drawn_as_doc_tab() {
        // A row the bar's height less its border, clipped there: bracket.silico, first,
        // shown, and unsaved; vise-jaw.silico after it.
        let top = Rc::new(Cell::new(Pos2::ZERO));
        let seen = Rc::clone(&top);
        let mut h = drawn(move |ui| {
            let row = Rect::from_min_size(
                ui.max_rect().min,
                Vec2::new(600.0, css::TOP_BAR_H - css::BORDER),
            );
            seen.set(row.min);
            let layout = egui::Layout::left_to_right(egui::Align::Center);
            ui.scope_builder(egui::UiBuilder::new().max_rect(row).layout(layout), |ui| {
                ui.set_clip_rect(row);
                tab_strip(ui, |ui| {
                    for tab in [
                        DocTab {
                            name: "bracket.silico",
                            path: Some("/parts/bracket.silico"),
                            unsaved: true,
                            active: true,
                            first: true,
                        },
                        DocTab {
                            name: "vise-jaw.silico",
                            path: Some("/parts/vise-jaw.silico"),
                            unsaved: false,
                            active: false,
                            first: false,
                        },
                    ] {
                        doc_tab(ui, &tab);
                    }
                });
            });
        });
        let top = top.get();
        let height = css::TOP_BAR_H - css::BORDER;
        let middle = top.y + height / 2.0;
        // Each tab: its borders (the first's at both sides, the rest's at their right), 10
        // of padding, its name and asterisk, 6, the 16 point close button, and 6.
        let bracket = text(&h, "bracket.silico");
        let dirty = text(&h, " *");
        let vise = text(&h, "vise-jaw.silico");
        let tail = css::TAB_GAP + css::X + css::TAB_PADDING_RIGHT;
        let first = Rect::from_min_size(
            top,
            Vec2::new(
                css::BORDER
                    + css::TAB_PADDING_LEFT
                    + bracket.galley.size().x
                    + dirty.galley.size().x
                    + tail
                    + css::BORDER,
                height,
            ),
        );
        let second = Rect::from_min_size(
            first.right_top(),
            Vec2::new(
                css::TAB_PADDING_LEFT + vise.galley.size().x + tail + css::BORDER,
                height,
            ),
        );
        // Each close button 16 square on the tab's middle, 6 and the border from its right;
        // each tab known by its name up to its close button.
        let close = |tab: Rect| {
            Rect::from_center_size(
                Pos2::new(
                    tab.right() - css::BORDER - css::TAB_PADDING_RIGHT - css::X / 2.0,
                    middle,
                ),
                Vec2::splat(css::X),
            )
        };
        for (label, tab) in [("bracket.silico *", first), ("vise-jaw.silico", second)] {
            let x = close(tab);
            sized(
                widget(&h, &format!("Close {}", label.trim_end_matches(" *"))),
                x,
                label,
            );
            sized(
                widget(&h, label),
                Rect::from_min_max(tab.min, Pos2::new(x.left(), tab.bottom())),
                label,
            );
        }
        // Borders: 1 of Gray.250 at each tab's right, and at the first's left, its height.
        for (x, tab) in [
            (first.left() + css::BORDER / 2.0, first),
            (first.right() - css::BORDER / 2.0, first),
            (second.right() - css::BORDER / 2.0, second),
        ] {
            let found = lines_at_x(&h, x);
            assert_eq!(found.len(), 1, "a border at {x}: {:?}", lines(&h));
            at(found[0].0[0], Pos2::new(x, tab.top()), "a border's top");
            at(
                found[0].0[1],
                Pos2::new(x, tab.bottom()),
                "a border's bottom",
            );
            assert_eq!(found[0].1, Stroke::new(css::BORDER, css::G250));
        }
        assert!(lines_at_x(&h, second.left() + css::BORDER / 2.0).is_empty());
        // The shown tab (`.doc-tab.active`): Gray.200, down over the bar's border below it
        // (`::after`, the strip's clip letting it past the row's); its name white, 10 inside its border, and the
        // asterisk white after it (`.dirty`), each on the tab's middle in 12 points; its
        // close button's icon Gray.550 (`.x`).
        let (fill, corners, clip) = fill_at(
            &h,
            Rect::from_min_max(first.min, first.max + Vec2::new(0.0, css::BORDER)),
        )
        .expect("the shown tab's fill");
        assert_eq!((fill, corners), (css::G200, CornerRadius::ZERO));
        assert!(
            clip.bottom() >= first.bottom() + css::BORDER - 1e-3,
            "{clip:?}"
        );
        at(
            bracket.pos,
            Pos2::new(
                first.left() + css::BORDER + css::TAB_PADDING_LEFT,
                middle - bracket.galley.size().y / 2.0,
            ),
            "bracket.silico",
        );
        at(
            dirty.pos,
            Pos2::new(
                bracket.pos.x + bracket.galley.size().x,
                middle - dirty.galley.size().y / 2.0,
            ),
            "its asterisk",
        );
        for shown in [&bracket, &dirty, &vise] {
            assert_eq!(font(shown), FontId::proportional(css::FS));
        }
        assert_eq!(colour(&bracket), css::G1000);
        assert_eq!(colour(&dirty), css::G1000);
        assert_eq!(
            icon_at(&h, icon_rect(close(first).center())),
            Some(css::G550)
        );
        // The other at rest (`.doc-tab`): no fill, its name Gray.550 10 in from its left,
        // its close button not drawn (`.x`'s `opacity: 0`).
        assert_eq!(fill_at(&h, second), None);
        at(
            vise.pos,
            Pos2::new(
                second.left() + css::TAB_PADDING_LEFT,
                middle - vise.galley.size().y / 2.0,
            ),
            "vise-jaw.silico",
        );
        assert_eq!(colour(&vise), css::G550);
        assert_eq!(icons_in(&h, close(second)), 0);
        assert_eq!(rings(&h), 0);
        // Hovered (`.doc-tab:hover`): Gray.150 behind Gray.800, its close button drawn.
        h.get_by_role_and_label(egui::accesskit::Role::Button, "vise-jaw.silico")
            .hover();
        h.run_steps(2);
        assert_eq!(
            fill_at(&h, second).map(|(fill, corners, _)| (fill, corners)),
            Some((css::G150, CornerRadius::ZERO))
        );
        assert_eq!(colour(&text(&h, "vise-jaw.silico")), css::G800);
        assert_eq!(
            icon_at(&h, icon_rect(close(second).center())),
            Some(css::G550)
        );
        // Its close button hovered (`.x:hover`): Gray.350 behind white, corners 3.
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Close vise-jaw.silico")
            .hover();
        h.run_steps(2);
        assert_eq!(
            fill_at(&h, close(second)).map(|(fill, corners, _)| (fill, corners)),
            Some((css::G350, CornerRadius::same(css::X_RADIUS)))
        );
        assert_eq!(
            icon_at(&h, icon_rect(close(second).center())),
            Some(css::G1000)
        );
        // Focused from the keyboard, the pointer gone: the ring inside its borders, where
        // the strip's clip (`#tabs`'s `overflow: hidden`) leaves it whole, and its close
        // button drawn.
        h.remove_cursor();
        h.get_by_role_and_label(egui::accesskit::Role::Button, "vise-jaw.silico")
            .focus();
        h.run_steps(2);
        assert_eq!(
            ring_at(&h, second.shrink(css::BORDER)),
            Some((CornerRadius::ZERO, StrokeKind::Inside))
        );
        assert_eq!(rings(&h), 1);
        assert_eq!(fill_at(&h, second), None);
        assert_eq!(
            icon_at(&h, icon_rect(close(second).center())),
            Some(css::G550)
        );
    }

    #[test]
    fn the_tooltip_is_drawn_as_tooltip() {
        // Redo's: its title, its caps (egui_kittest's platform is neither macOS nor
        // Windows: Ctrl, Shift, and Z), and a description under them.
        let top = Rc::new(Cell::new(Pos2::ZERO));
        let seen = Rc::clone(&top);
        let h = drawn(move |ui| {
            seen.set(ui.max_rect().min);
            tooltip_ui(
                ui,
                "Redo",
                Some(Chord::Key(egui::KeyboardShortcut::new(
                    Modifiers::COMMAND | Modifiers::SHIFT,
                    Key::Z,
                ))),
                Some("Unsaved edits"),
            );
        });
        // `.tt-title`: white, 12 points, on its row's middle, the row as high as its text
        // and its caps (`align-items: center`), at the tooltip's top left.
        let top = top.get();
        let title = text(&h, "Redo");
        let row_height = title.galley.size().y.max(css::KBD_HEIGHT);
        at(
            title.pos,
            Pos2::new(top.x, top.y + (row_height - title.galley.size().y) / 2.0),
            "the title",
        );
        assert_eq!(font(&title), FontId::proportional(css::FS));
        assert_eq!(colour(&title), css::G1000);
        let title_middle = title.pos.y + title.galley.size().y / 2.0;
        // Its caps 8 beyond it (`.tt-title`'s `gap`, the caps' `margin-left: auto` taking
        // no more in a tooltip as wide as its content), 16 high on its middle, left to
        // right.
        let shapes: Vec<RectShape> = rects(&h)
            .into_iter()
            .filter(|(_, shape)| shape.corner_radius == CornerRadius::same(css::KBD_RADIUS))
            .map(|(_, shape)| shape)
            .collect();
        for shape in &shapes {
            assert!(shape.stroke.is_empty(), "{shape:?}");
        }
        let mut caps: Vec<Rect> = shapes.iter().map(|shape| shape.rect).collect();
        caps.sort_by(|a, b| a.left().total_cmp(&b.left()));
        assert_eq!(caps.len(), 3, "{caps:?}");
        assert!(
            near(
                caps[0].left(),
                title.pos.x + title.galley.size().x + css::TT_TITLE_GAP
            ),
            "the first cap at {:?}, the title {:?} wide at {:?}",
            caps[0],
            title.galley.size(),
            title.pos
        );
        for cap in &caps {
            assert!(near(cap.height(), css::KBD_HEIGHT), "{cap:?}");
            assert!(near(cap.center().y, title_middle), "{cap:?}");
            crate::modal::tests::kbd_border(
                &crate::modal::tests::cap_border(&h, *cap),
                *cap,
                css::G350,
                "the tooltip's cap",
            );
        }
        // Each 2 beyond the one before it (`.keys`' `gap`).
        for pair in caps.windows(2) {
            assert!(
                near(pair[1].left(), pair[0].right() + css::KEYS_GAP),
                "{pair:?}"
            );
        }
        for (cap, said) in caps.iter().zip(["Ctrl", "Shift", "Z"]) {
            assert!(cap.contains(text(&h, said).pos), "{said} in {cap:?}");
        }
        // `.tt-desc`: 2 below the title's row, at its left, Gray.550, 11 points.
        let description = text(&h, "Unsaved edits");
        at(
            description.pos,
            Pos2::new(top.x, top.y + row_height + css::TT_DESC_MARGIN),
            "the description",
        );
        assert_eq!(font(&description), FontId::proportional(css::FS_SMALL));
        assert_eq!(colour(&description), css::G550);
    }

    /// The viewport commands `move_window` sends over the bar's empty space as the pointer
    /// goes down on it and comes up, twice, the window `maximized` or not: those of each
    /// frame.
    fn moved(maximized: bool) -> Vec<Vec<ViewportCommand>> {
        let ctx = egui::Context::default();
        let space = Rect::from_min_size(Pos2::new(100.0, 0.0), Vec2::new(200.0, 30.0));
        let on = space.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: on,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let frames = [
            vec![egui::Event::PointerMoved(on)],
            vec![button(true)],
            vec![button(false)],
            vec![button(true)],
            vec![button(false)],
        ];
        frames
            .into_iter()
            .enumerate()
            .map(|(n, events)| {
                let mut input = egui::RawInput {
                    time: Some(n as f64 * 0.05),
                    events,
                    ..Default::default()
                };
                input.viewports.insert(
                    egui::ViewportId::ROOT,
                    egui::ViewportInfo {
                        maximized: Some(maximized),
                        ..Default::default()
                    },
                );
                let mut drawn = ctx.run_ui(input, |ui| move_window(ui, space));
                drawn.textures_delta.clear();
                // The window's own: not the theme the first frame sets.
                drawn
                    .viewport_output
                    .remove(&egui::ViewportId::ROOT)
                    .map(|output| output.commands)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|sent| {
                        matches!(
                            sent,
                            ViewportCommand::StartDrag | ViewportCommand::Maximized(_)
                        )
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_bars_empty_space_moves_and_zooms_the_window_where_it_is_the_title_bar() {
        for maximized in [false, true] {
            let sent = moved(maximized);
            if TITLE_BAR_IS_APP_BAR {
                // Down on it, the window follows the pointer; a double click zooms it, or
                // unzooms it.
                assert_eq!(
                    sent,
                    [
                        vec![],
                        vec![ViewportCommand::StartDrag],
                        vec![],
                        vec![ViewportCommand::StartDrag],
                        vec![ViewportCommand::Maximized(!maximized)],
                    ],
                    "maximized {maximized}"
                );
            } else {
                // Where the title bar isn't the bar, it moves nothing.
                assert!(sent.iter().all(Vec::is_empty), "{sent:?}");
            }
        }
    }

    /// The room [`inset`] leaves, in a window full screen or not.
    fn inset_in(full_screen: bool) -> f32 {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                fullscreen: Some(full_screen),
                ..Default::default()
            },
        );
        let mut room = 0.0;
        let mut drawn = ctx.run_ui(input, |ui| {
            ui.horizontal(|ui| {
                let before = ui.cursor().left();
                inset(ui);
                room = ui.cursor().left() - before;
            });
        });
        // Nothing here draws: the font atlas's upload is let go.
        drawn.textures_delta.clear();
        room
    }

    #[test]
    fn the_inset_is_the_window_buttons_room_or_the_bars_padding() {
        // `.traffic` (12 of padding, three 12 point lights 8 apart, and 14 of padding) and
        // the bar's 2 point gap after it, on macOS; elsewhere the bar's 8 of padding.
        let expected = if cfg!(target_os = "macos") {
            12.0 + 36.0 + 16.0 + 14.0 + 2.0
        } else {
            8.0 + 2.0
        };
        assert_eq!(inset_in(false), expected);
        // Full screen, macOS hides its window buttons: the bar's own padding everywhere.
        assert_eq!(inset_in(true), 8.0 + 2.0);
    }
}
