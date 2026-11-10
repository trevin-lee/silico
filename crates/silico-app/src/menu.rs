//! A menu's lines as the approved prototype draws them (docs/design/m5-ui: styles.css's
//! `.menu`, `.menu .mi`, `.menu .mh`, `.menu .msep`; app.js's `renderFloating`, which every
//! menu of the prototype goes through): one way to draw them, which every menu the app draws
//! over the window takes, a feature's (`crate::feature_menu`), the canvas's
//! (`crate::canvas_menu`, `crate::select_other`), and sketch mode's on the canvas
//! (`crate::sketch::bar::canvas_menu`), so their widths and their look can't part. Each item
//! is named for a screen reader by its words and dim words, its keys its keyboard shortcut.
//!
//! An item (`.mi`, 24 high, `padding: 0 8px`, `gap: 8px`): its icon, or a spacer as wide
//! (`<span style="width:14px">`); its words; its dim words after them (the `subdued` span,
//! 11 high, `margin-left: 8px`), if it has them; and its keys' caps at its right (`.sc`,
//! `margin-left: auto; padding-left: 24px`, there with no keys too, so it always takes its
//! gap and its padding). Every item of a menu is as wide as its widest line (`width: 100%`,
//! `white-space: nowrap`, the menu as wide as its lines), and that at least `.menu`'s
//! `min-width: 200px` less its border and its padding (`box-sizing: border-box`).

use eframe::egui::{
    self, Color32, FontId, Pos2, Rect, Response, Sense, Ui, Vec2, WidgetInfo, WidgetType, vec2,
};

use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::theme;

/// An item as a menu draws it: its icon, its words, its dim words after them, its keys'
/// caps, why it's off if it is, whether its words are red (`.danger`), and whether it's lit
/// by the keys (`.kfocus`) as a hovered item is.
#[derive(Debug, Clone, Copy)]
pub struct Item<'a> {
    pub icon: Option<Icon>,
    pub label: &'a str,
    pub sub: Option<&'a str>,
    pub caps: &'a [String],
    pub off: Option<&'a str>,
    pub danger: bool,
    pub lit: bool,
}

impl Item<'_> {
    /// What a screen reader (and a test) is told it is, its name: its words, then its dim
    /// words. Its keys aren't in it: [`item`] gives them as its keyboard shortcut
    /// ([`Item::shortcut`]), which a screen reader announces as one, apart from the name, as
    /// every menu of the app does (sketch mode's too), so an item is found by its words.
    pub fn said(&self) -> String {
        let mut said = self.label.to_owned();
        if let Some(sub) = self.sub {
            said.push(' ');
            said.push_str(sub);
        }
        said
    }

    /// Its keys, as the platform joins them, for a screen reader's keyboard shortcut; none
    /// for an item with no keys.
    pub fn shortcut(&self, ctx: &egui::Context) -> Option<String> {
        (!self.caps.is_empty()).then(|| crate::shortcuts::joined(ctx, self.caps))
    }
}

/// A menu's frame (`.menu`): egui's, as the theme sets it for every menu (the feature
/// menus', the toolbar's drop-downs, the canvas's in selection and in sketch mode): Gray.200
/// inside a Gray.250 border, `--r`'s corners, the popups' shadow, padded
/// [`theme::MENU_PADDING`].
pub fn frame(ui: &Ui) -> egui::Frame {
    egui::Frame::menu(ui.style())
}

/// The least a menu's lines are wide: `.menu`'s `min-width: 200px`, less its border and its
/// padding inside it.
pub fn least_width() -> f32 {
    theme::MENU_MIN_WIDTH - 2.0 * (f32::from(theme::MENU_PADDING) + theme::BORDER_WIDTH)
}

/// The width `text` takes at `size`, on one line.
pub fn text_width(ui: &Ui, text: &str, size: f32) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            FontId::proportional(size),
            theme::MENU_ITEM_TEXT,
        )
        .size()
        .x
}

/// The width `item` takes on its own: its padding, its icon or spacer, its words, its dim
/// words after a gap and their margin, and `.sc` (a gap, its padding, and its caps, 2 apart).
pub fn item_width(ui: &Ui, item: &Item<'_>) -> f32 {
    let sub = item.sub.map_or(0.0, |sub| {
        theme::MENU_ITEM_GAP
            + theme::MENU_ITEM_SUB_MARGIN
            + text_width(ui, sub, theme::MENU_ITEM_SUB_SIZE)
    });
    2.0 * theme::MENU_ITEM_PADDING
        + ICON_SIZE
        + theme::MENU_ITEM_GAP
        + text_width(ui, item.label, theme::BODY_SIZE)
        + sub
        + theme::MENU_ITEM_GAP
        + theme::MENU_KEYS_PADDING
        + crate::start::key_caps_width_spaced(ui, item.caps, theme::KEY_CAP_GAP)
}

/// A head's words laid out (`.menu .mh`: `font-size: 10px`, Gray.550, `text-transform:
/// uppercase; letter-spacing: .5px`).
pub fn head_galley(ui: &Ui, text: &str) -> std::sync::Arc<egui::Galley> {
    let job = egui::text::LayoutJob::single_section(
        text.to_uppercase(),
        egui::TextFormat {
            font_id: FontId::proportional(theme::TOOLBAR_LABEL_SIZE),
            color: theme::TOOLBAR_LABEL_TEXT,
            extra_letter_spacing: theme::CAPITALS_LETTER_SPACING,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

/// The width a head of `text` takes: its words, 8 in at each side (`padding: 6px 8px 2px`).
pub fn head_width(ui: &Ui, text: &str) -> f32 {
    head_galley(ui, text).size().x + 2.0 * theme::MENU_ITEM_PADDING
}

/// A menu's lines' width: the widest of its head's, its items', and `more` (its other lines',
/// as Select Other's foot), and [`least_width`] at least.
pub fn width<'a>(
    ui: &Ui,
    head: Option<&str>,
    items: impl IntoIterator<Item = &'a Item<'a>>,
    more: f32,
) -> f32 {
    items
        .into_iter()
        .map(|item| item_width(ui, item))
        .chain(head.map(|text| head_width(ui, text)))
        .fold(least_width().max(more), f32::max)
}

/// A head (`.menu .mh`), `width` wide: `text` in capitals, 6 above its line and 2 under it,
/// 8 in from the left; labelled with them as drawn.
pub fn head(ui: &mut Ui, text: &str, width: f32) {
    let galley = head_galley(ui, text);
    let height = theme::MENU_HEAD_TOP + theme::MENU_LINE_HEIGHT + theme::MENU_HEAD_BOTTOM;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let said = galley.text().to_owned();
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &said));
    let line = rect.top() + theme::MENU_HEAD_TOP;
    let at = Pos2::new(
        rect.left() + theme::MENU_ITEM_PADDING,
        line + (theme::MENU_LINE_HEIGHT - galley.size().y) / 2.0,
    );
    ui.painter().galley(at, galley, theme::TOOLBAR_LABEL_TEXT);
}

/// A separator (`.menu .msep`: 1 high, Gray.250, 4 above and below), `width` wide.
pub fn separator(ui: &mut Ui, width: f32) {
    let height = theme::BORDER_WIDTH + 2.0 * theme::MENU_SEPARATOR_MARGIN;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let line = Rect::from_min_size(
        Pos2::new(rect.left(), rect.top() + theme::MENU_SEPARATOR_MARGIN),
        vec2(width, theme::BORDER_WIDTH),
    );
    ui.painter().rect_filled(line, 0.0, theme::MENU_SEPARATOR);
}

/// One item (`.menu .mi`), `width` wide, on or off: its icon or its spacer, its words, its
/// dim words after them, and its caps at its right. Hovered, focused, or lit by the keys
/// (`.kfocus`) it's filled blue, its words, icon, and caps the blue's text; off, its words
/// are Gray.400, and its reason is its hover text and its description for a screen reader;
/// `.danger`'s words are red, off too. Its response, labelled as [`Item::said`] says, its
/// keys its keyboard shortcut ([`Item::shortcut`]).
pub fn item(ui: &mut Ui, item: &Item<'_>, width: f32) -> Response {
    let said = item.said();
    let shortcut = item.shortcut(ui.ctx());
    // Painted at the menu's opacity, not faded further when off: `.menu .mi:disabled` is
    // Gray.400 itself.
    let opacity = ui.opacity();
    let response = ui.add_enabled(item.off.is_none(), |ui: &mut Ui| {
        ui.set_opacity(opacity);
        let (rect, response) =
            ui.allocate_exact_size(vec2(width, theme::MENU_ITEM_HEIGHT), Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &said));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let lit = enabled && (item.lit || response.hovered() || response.has_focus());
        // `.menu .mi:hover:not(:disabled)` outweighs `.danger` (a class and two
        // pseudo-classes to two classes), so a hovered red item is lit as the others are;
        // `.danger` comes after `.menu .mi:disabled`, of the same weight, so an off one is
        // red still.
        let ink = if lit {
            theme::MENU_ITEM_HOVER_TEXT
        } else if item.danger {
            theme::ERROR_TEXT
        } else if !enabled {
            theme::TEXT_DISABLED
        } else {
            theme::MENU_ITEM_TEXT
        };
        if lit {
            ui.painter()
                .rect_filled(rect, theme::SMALL_CORNER_RADIUS, theme::MENU_ITEM_HOVER_BG);
        }
        let middle = rect.center().y;
        let icon_rect = Rect::from_min_size(
            Pos2::new(
                rect.left() + theme::MENU_ITEM_PADDING,
                middle - ICON_SIZE / 2.0,
            ),
            Vec2::splat(ICON_SIZE),
        );
        if let Some(icon) = item.icon {
            icon.image(IconSize::Small)
                .tint(ink)
                .paint_at(ui, icon_rect);
        }
        let label = ui.painter().layout_no_wrap(
            item.label.to_owned(),
            FontId::proportional(theme::BODY_SIZE),
            ink,
        );
        let label_at = Pos2::new(
            icon_rect.right() + theme::MENU_ITEM_GAP,
            middle - label.size().y / 2.0,
        );
        let label_right = label_at.x + label.size().x;
        ui.painter()
            .galley_with_override_text_color(label_at, label, ink);
        if let Some(sub) = item.sub {
            let sub = ui.painter().layout_no_wrap(
                sub.to_owned(),
                FontId::proportional(theme::MENU_ITEM_SUB_SIZE),
                theme::MENU_ITEM_SUB_TEXT,
            );
            let at = Pos2::new(
                label_right + theme::MENU_ITEM_GAP + theme::MENU_ITEM_SUB_MARGIN,
                middle - sub.size().y / 2.0,
            );
            ui.painter().galley(at, sub, theme::MENU_ITEM_SUB_TEXT);
        }
        if !item.caps.is_empty() {
            // `.menu .mi:hover .kbd` (and `.kfocus .kbd`): no fill, a faint white outline,
            // the item's words' colour; else `.kbd`'s own, off or on. 2 apart, as `.keys`
            // sets them.
            let colours = if lit {
                (
                    Color32::TRANSPARENT,
                    theme::MENU_KEY_CAP_HOVER_STROKE,
                    theme::MENU_ITEM_HOVER_TEXT,
                )
            } else {
                (
                    theme::KEY_CAP_BG,
                    theme::KEY_CAP_STROKE,
                    theme::KEY_CAP_TEXT,
                )
            };
            crate::start::key_caps_coloured(
                ui,
                Pos2::new(rect.right() - theme::MENU_ITEM_PADDING, middle),
                item.caps,
                colours,
                theme::KEY_CAP_GAP,
            );
        }
        response
    });
    if let Some(shortcut) = shortcut {
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_keyboard_shortcut(shortcut);
        });
    }
    match item.off {
        Some(off) => {
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_description(off.to_owned());
            });
            response.on_disabled_hover_text(off)
        }
        None => response,
    }
}
