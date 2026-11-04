//! The app's modals and buttons as the prototype draws them (docs/design/m5-ui: its
//! `styles.css`'s `.backdrop`, `.modal`, `.m-head`, `.m-body`, `.m-foot`, `.lbl`, `.field`,
//! and `.btn` rules, and `js/app.js`'s `renderModal`): the backdrop, the modal's frame, its
//! head, body, and foot, a field's label, a field 26 high with a button beside it, and a
//! button of each kind (`.btn-primary`, `.btn-secondary`, `.btn-danger`), with its key cap
//! (`.btn .kbd`). The docked dialog's footer (`crate::properties`) and the banners
//! (`crate::banner`) draw their buttons here too.
//!
//! Every text is a label, so the accessibility tree has it as it's drawn.

use std::sync::Arc;

use eframe::egui::text::LayoutJob;
use eframe::egui::{
    self, Align, Color32, FontId, Frame, Galley, Label, Layout, Rect, Response, Sense, Stroke,
    StrokeKind, TextFormat, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};

use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::properties;
use crate::theme;

/// Which of the prototype's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `.btn-primary`, the light button: what Enter does.
    Primary,
    /// `.btn-secondary`: Cancel, Browse….
    Secondary,
    /// `.btn-danger`: what drops the person's edits (Don't save) or writes over a file
    /// (Replace).
    Danger,
    /// `.canvas-banner .btn-secondary`: a button on a banner whose words are this colour.
    Banner(Color32),
}

/// A button: its words (its label), its kind, its key cap if it shows one, and whether it's
/// on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Button<'a> {
    pub text: &'a str,
    pub kind: Kind,
    pub cap: Option<&'a str>,
    pub enabled: bool,
}

impl<'a> Button<'a> {
    /// A button of `kind` saying `text`, on, with no key cap.
    pub(crate) fn new(text: &'a str, kind: Kind) -> Self {
        Button {
            text,
            kind,
            cap: None,
            enabled: true,
        }
    }

    /// With `cap` as its key cap: the key that does what it does.
    pub(crate) fn cap(self, cap: &'a str) -> Self {
        Button {
            cap: Some(cap),
            ..self
        }
    }

    /// On only if `enabled`.
    pub(crate) fn enabled(self, enabled: bool) -> Self {
        Button { enabled, ..self }
    }
}

/// Text in the body's type, `size` points on a line of the body's, in `colour`.
pub(crate) fn format(size: f32, colour: Color32) -> TextFormat {
    TextFormat {
        line_height: Some(theme::BODY_LINE_HEIGHT),
        ..properties::format(size, colour, false)
    }
}

/// `text` in [`format`], wrapped at `width`.
pub(crate) fn wrapped(text: &str, size: f32, colour: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::single_section(text.to_owned(), format(size, colour));
    job.wrap.max_width = width;
    job
}

/// `galley` as a label with its top left at `at`, in a child of `ui`'s: drawn, and in the
/// accessibility tree, without moving `ui`'s cursor.
pub(crate) fn label_at(ui: &mut Ui, at: egui::Pos2, galley: Arc<Galley>) -> Response {
    let rect = Rect::from_min_size(at, galley.size());
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Min)),
    );
    child.add(Label::new(galley).selectable(false))
}

/// The modal's frame (`.modal`): Gray.200, its 1 px Gray.250 border, `--r`, its shadow, and
/// nothing inside it but its parts' own padding.
pub(crate) fn frame() -> Frame {
    Frame::new()
        .fill(theme::MODAL_BG)
        .stroke(Stroke::new(theme::BORDER_WIDTH, theme::MODAL_BORDER))
        .corner_radius(theme::MODAL_CORNER_RADIUS)
        .shadow(theme::MODAL_SHADOW)
}

/// A modal named `id`, over the backdrop (`.backdrop`, black at 60%), centred in the
/// window, [`theme::MODAL_WIDTH`] wide, its border in that: `content` draws its head, body,
/// and foot ([`head`], [`body`], [`foot`]). What `content` gives.
pub(crate) fn show<R>(ctx: &egui::Context, id: egui::Id, content: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Modal::new(id)
        .backdrop_color(theme::MODAL_BACKDROP)
        .frame(frame())
        .show(ctx, |ui| {
            ui.set_width(theme::MODAL_WIDTH - 2.0 * theme::BORDER_WIDTH);
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            content(ui)
        })
        .inner
}

/// The modal's head (`.m-head`): `icon`, if it has one, and `title`, 14 high in Gray.1000,
/// wrapped where it doesn't fit, centred on the icon.
pub(crate) fn head(ui: &mut Ui, icon: Option<Icon>, title: &str) -> Response {
    let pad = theme::MODAL_HEAD_PADDING;
    let width = ui.available_width();
    let icon_room = icon.map_or(0.0, |_| ICON_SIZE + theme::MODAL_HEAD_GAP);
    let wrap = (width - pad.sum().x - icon_room).max(1.0);
    let galley = ui.painter().layout_job(wrapped(
        title,
        theme::MODAL_TITLE_SIZE,
        theme::MODAL_TITLE,
        wrap,
    ));
    let line = galley.size().y.max(theme::BODY_LINE_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(vec2(width, pad.sum().y + line), Sense::hover());
    let middle = rect.top() + pad.topf() + line / 2.0;
    let left = rect.left() + pad.leftf();
    if let Some(icon) = icon {
        icon.image(IconSize::Small)
            .tint(theme::MODAL_TITLE)
            .paint_at(
                ui,
                Rect::from_min_size(pos2(left, middle - ICON_SIZE / 2.0), Vec2::splat(ICON_SIZE)),
            );
    }
    label_at(
        ui,
        pos2(left + icon_room, middle - galley.size().y / 2.0),
        galley,
    )
}

/// The modal's body (`.m-body`): `add`'s parts, padded at its sides and under it.
pub(crate) fn body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::NONE
        .inner_margin(theme::MODAL_BODY_PADDING)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A field's label (`.modal .lbl`): `text`, `--fs-small` in Gray.550, 10 above it and 4
/// under. Its response, to label the field with.
pub(crate) fn label(ui: &mut Ui, text: &str) -> Response {
    ui.add_space(theme::MODAL_LABEL_MARGIN_TOP);
    let galley = ui.painter().layout_job(wrapped(
        text,
        theme::HEADER_SIZE,
        theme::MODAL_LABEL,
        ui.available_width(),
    ));
    let response = ui.add(Label::new(galley).selectable(false));
    ui.add_space(theme::MODAL_LABEL_MARGIN_BOTTOM);
    response
}

/// Words in the body (`.m-body p`): `text` in `colour` (Gray.750, [`theme::MODAL_TEXT`], or
/// red for what went wrong), wrapped to the body's width.
pub(crate) fn text(ui: &mut Ui, text: &str, colour: Color32) -> Response {
    let galley = ui.painter().layout_job(wrapped(
        text,
        theme::BODY_SIZE,
        colour,
        ui.available_width(),
    ));
    ui.add(Label::new(galley).selectable(false))
}

/// A field (`.modal .field`, 26 high) across the body's width, `edit` (in
/// [`theme::field`]'s frame) labelled by `label`, and, if `beside` is given, that button at
/// its right, `--gap` from it, as high: the field's response, and the button's.
pub(crate) fn field(
    ui: &mut Ui,
    edit: egui::TextEdit<'_>,
    label: egui::Id,
    beside: Option<Button<'_>>,
) -> (Response, Option<Response>) {
    let (row, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::MODAL_FIELD_HEIGHT),
        Sense::hover(),
    );
    let button = beside.map(|button| {
        let width = natural_width(ui, &button);
        (
            button,
            Rect::from_min_max(pos2(row.right() - width, row.top()), row.max),
        )
    });
    let field = match &button {
        Some((_, at)) => {
            Rect::from_min_max(row.min, pos2(at.left() - theme::ITEM_SPACING, row.bottom()))
        }
        None => row,
    };
    let response = ui.put(field, edit).labelled_by(label);
    let clicked = button.map(|(button, at)| paint(ui, at, &button));
    (response, clicked)
}

/// The modal's foot (`.m-foot`): `start`'s buttons from its left, and `end`'s at its right,
/// each [`theme::MODAL_BUTTON_HEIGHT`] high and [`theme::MODAL_BUTTON_MIN_WIDTH`] wide at
/// least, in that order for the keyboard. Which were clicked.
pub(crate) fn foot<const S: usize, const E: usize>(
    ui: &mut Ui,
    start: [Button<'_>; S],
    end: [Button<'_>; E],
) -> ([bool; S], [bool; E]) {
    let pad = theme::MODAL_FOOT_PADDING;
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            ui.available_width(),
            pad.sum().y + theme::MODAL_BUTTON_HEIGHT,
        ),
        Sense::hover(),
    );
    let top = rect.top() + pad.topf();
    let width =
        |ui: &Ui, button: &Button<'_>| natural_width(ui, button).max(theme::MODAL_BUTTON_MIN_WIDTH);
    let place = |left: &mut f32, width: f32| {
        let at = Rect::from_min_size(pos2(*left, top), vec2(width, theme::MODAL_BUTTON_HEIGHT));
        *left += width + theme::MODAL_FOOT_GAP;
        at
    };
    let mut left = rect.left() + pad.leftf();
    let start_at = start.map(|button| place(&mut left, width(ui, &button)));
    let widths = end.map(|button| width(ui, &button));
    let row: f32 = widths.iter().sum::<f32>() + E.saturating_sub(1) as f32 * theme::MODAL_FOOT_GAP;
    let mut left = rect.right() - pad.rightf() - row;
    let end_at = widths.map(|width| place(&mut left, width));
    let start_clicked = std::array::from_fn(|at| paint(ui, start_at[at], &start[at]).clicked());
    let end_clicked = std::array::from_fn(|at| paint(ui, end_at[at], &end[at]).clicked());
    (start_clicked, end_clicked)
}

/// A key cap's words (`.kbd`, 10.5 points) in `ink`.
fn cap_galley(ui: &Ui, cap: &str, ink: Color32) -> Arc<Galley> {
    ui.painter().layout_no_wrap(
        cap.to_owned(),
        FontId::proportional(theme::KEY_CAP_TEXT_SIZE),
        ink,
    )
}

/// A key cap's width: its words padded, and its border, 16 at least.
fn cap_width(galley: &Galley) -> f32 {
    (galley.size().x + 2.0 * (theme::KEY_CAP_PADDING + theme::KEY_CAP_BORDER))
        .max(theme::KEY_CAP_MIN_WIDTH)
}

/// The width `button` takes where nothing widens it (`.btn`): its words and its key cap,
/// [`theme::BUTTON_GAP`] apart, padded at each side.
pub(crate) fn natural_width(ui: &Ui, button: &Button<'_>) -> f32 {
    let words = ui
        .painter()
        .layout_job(properties::body(button.text, Color32::WHITE))
        .size()
        .x;
    let cap = button.cap.map_or(0.0, |cap| {
        theme::BUTTON_GAP + cap_width(&cap_galley(ui, cap, Color32::WHITE))
    });
    words + cap + 2.0 * theme::BUTTON_PADDING
}

/// A button's fill and words, as the prototype colours `kind`: on or off, hovered or not.
pub(crate) fn colours(kind: Kind, enabled: bool, hovered: bool) -> (Color32, Color32) {
    match (kind, enabled, hovered) {
        (Kind::Primary, false, _) => (
            theme::PRIMARY_BUTTON_DISABLED_BG,
            theme::PRIMARY_BUTTON_DISABLED_TEXT,
        ),
        (Kind::Primary, true, false) => (theme::PRIMARY_BUTTON_BG, theme::PRIMARY_BUTTON_TEXT),
        (Kind::Primary, true, true) => (
            theme::PRIMARY_BUTTON_HOVER_BG,
            theme::PRIMARY_BUTTON_HOVER_TEXT,
        ),
        (Kind::Secondary, true, true) => (
            theme::SECONDARY_BUTTON_HOVER_BG,
            theme::SECONDARY_BUTTON_HOVER_TEXT,
        ),
        (Kind::Secondary, _, _) => (theme::SECONDARY_BUTTON_BG, theme::SECONDARY_BUTTON_TEXT),
        (Kind::Danger, true, true) => (
            theme::DANGER_BUTTON_HOVER_BG,
            theme::DANGER_BUTTON_HOVER_TEXT,
        ),
        (Kind::Danger, _, _) => (theme::DANGER_BUTTON_BG, theme::DANGER_BUTTON_TEXT),
        // The prototype gives a banner's button no look when off: its words go the
        // disabled grey, as an off bar button's do.
        (Kind::Banner(_), false, _) => (theme::BANNER_BUTTON_BG, theme::TEXT_DISABLED),
        (Kind::Banner(_), true, true) => (
            theme::BANNER_BUTTON_HOVER_BG,
            theme::BANNER_BUTTON_HOVER_TEXT,
        ),
        (Kind::Banner(ink), true, false) => (theme::BANNER_BUTTON_BG, ink),
    }
}

/// `button` in `rect`: its fill, its words and its key cap centred in it, labelled with its
/// words; off unless it's enabled. Painted at `ui`'s opacity, not faded further when off:
/// the prototype gives an off button its own colours.
pub(crate) fn paint(ui: &mut Ui, rect: Rect, button: &Button<'_>) -> Response {
    let opacity = ui.opacity();
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.add_enabled_ui(button.enabled, |ui| {
            ui.set_opacity(opacity);
            let (rect, response) = ui.allocate_exact_size(rect.size(), Sense::click());
            let enabled = ui.is_enabled();
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, button.text));
            let (fill, ink) = colours(button.kind, enabled, response.hovered());
            ui.painter()
                .rect_filled(rect, theme::SMALL_CORNER_RADIUS, fill);
            theme::focus_ring(
                ui,
                &response,
                rect,
                theme::SMALL_CORNER_RADIUS,
                StrokeKind::Outside,
            );
            let words = ui.painter().layout_job(properties::body(button.text, ink));
            let cap = button.cap.map(|cap| cap_galley(ui, cap, ink));
            let cap_room = cap
                .as_ref()
                .map_or(0.0, |cap| theme::BUTTON_GAP + cap_width(cap));
            let content = words.size().x + cap_room;
            let left = rect.center().x - content / 2.0;
            let middle = rect.center().y;
            ui.painter()
                .galley(pos2(left, middle - words.size().y / 2.0), words, ink);
            if let Some(cap) = cap {
                let width = cap_width(&cap);
                let cap_rect = Rect::from_min_size(
                    pos2(left + content - width, middle - theme::KEY_CAP_HEIGHT / 2.0),
                    vec2(width, theme::KEY_CAP_HEIGHT),
                );
                // `.btn .kbd`: no fill, its border and words the button's, at 55%.
                let faded = ink.gamma_multiply(theme::BUTTON_KEY_CAP_OPACITY);
                ui.painter().rect_stroke(
                    cap_rect,
                    theme::KEY_CAP_CORNER_RADIUS,
                    Stroke::new(theme::KEY_CAP_BORDER, faded),
                    StrokeKind::Inside,
                );
                ui.painter().galley_with_override_text_color(
                    cap_rect.center() - cap.size() / 2.0,
                    cap,
                    faded,
                );
            }
            response
        })
        .inner
    })
    .inner
}

#[cfg(test)]
pub(crate) mod tests {
    use eframe::egui::epaint::{RectShape, TextShape};
    use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Shape, Vec2};
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// The prototype's tokens and geometry (`styles.css`), the modals' and the banners'.
    pub(crate) mod css {
        use eframe::egui::Color32;

        /// `:root`'s greys.
        pub const G200: Color32 = Color32::from_rgb(0x21, 0x21, 0x21);
        pub const G250: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
        pub const G300: Color32 = Color32::from_rgb(0x2c, 0x2b, 0x2b);
        pub const G350: Color32 = Color32::from_rgb(0x52, 0x51, 0x51);
        pub const G400: Color32 = Color32::from_rgb(0x64, 0x63, 0x63);
        pub const G500: Color32 = Color32::from_rgb(0x82, 0x7f, 0x7f);
        pub const G550: Color32 = Color32::from_rgb(0x93, 0x90, 0x90);
        pub const G750: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);
        pub const G800: Color32 = Color32::from_rgb(0xe0, 0xdf, 0xdf);
        pub const G850: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xe5);
        pub const G1000: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
        /// `--red`, `.err-text`.
        pub const RED: Color32 = Color32::from_rgb(0xff, 0x38, 0x65);

        /// `body`: `font: 12px/16px`; `--fs-small`; `--icon`; `--r-sm`, `--r`; `--gap`.
        pub const FS: f32 = 12.0;
        pub const LH: f32 = 16.0;
        pub const FS_SMALL: f32 = 11.0;
        pub const ICON: f32 = 14.0;
        pub const R_SM: u8 = 4;
        pub const R: u8 = 6;
        pub const GAP: f32 = 8.0;

        /// `.backdrop`: `rgba(0,0,0,.6)`.
        pub const BACKDROP: [u8; 4] = [0, 0, 0, 153];
        /// `.modal`: 440 wide (`border-box`), Gray.200, `1px solid` Gray.250, `--r`, and
        /// `box-shadow: 0 15px 50px rgba(0,0,0,.7)`.
        pub const MODAL_W: f32 = 440.0;
        pub const BORDER: f32 = 1.0;
        pub const SHADOW_Y: f32 = 15.0;
        pub const SHADOW_BLUR: f32 = 50.0;
        pub const SHADOW: [u8; 4] = [0, 0, 0, 179];
        /// `.m-head`: `gap: 8px; padding: 12px 12px 8px 16px; font-size: 14px`.
        pub const HEAD_TOP: f32 = 12.0;
        pub const HEAD_BOTTOM: f32 = 8.0;
        pub const HEAD_LEFT: f32 = 16.0;
        pub const HEAD_GAP: f32 = 8.0;
        pub const HEAD_FS: f32 = 14.0;
        /// `.m-body`: `padding: 0 16px 12px`; its `p`, Gray.750.
        pub const BODY_SIDE: f32 = 16.0;
        pub const BODY_BOTTOM: f32 = 12.0;
        /// `.m-foot`: `gap: 6px; padding: 10px 16px 14px`; `.m-foot .btn`: `height: 26px;
        /// min-width: 76px`.
        pub const FOOT_TOP: f32 = 10.0;
        pub const FOOT_SIDE: f32 = 16.0;
        pub const FOOT_BOTTOM: f32 = 14.0;
        pub const FOOT_GAP: f32 = 6.0;
        pub const FOOT_BTN_H: f32 = 26.0;
        pub const FOOT_BTN_MIN_W: f32 = 76.0;
        /// `.modal .lbl`: `--fs-small`, Gray.550, `margin: 10px 0 4px`; `.modal .field`: 26.
        pub const LBL_TOP: f32 = 10.0;
        pub const LBL_BOTTOM: f32 = 4.0;
        pub const FIELD_H: f32 = 26.0;

        /// `.btn`: `height: 22px; padding: 0 10px; gap: 6px`; `.btn .kbd`: 16 high, `opacity:
        /// .55`.
        pub const BTN_H: f32 = 22.0;
        pub const BTN_PADDING: f32 = 10.0;
        pub const BTN_GAP: f32 = 6.0;
        pub const KBD_H: f32 = 16.0;
        pub const KBD_OPACITY: f32 = 0.55;
        /// `.btn-primary`: Gray.800 on Gray.250; hovered `--g850` and `#000`; off Gray.300 and
        /// Gray.500.
        pub const PRIMARY: (Color32, Color32) = (G800, G250);
        pub const PRIMARY_HOVER: (Color32, Color32) = (G850, Color32::BLACK);
        pub const PRIMARY_OFF: (Color32, Color32) = (G300, G500);
        /// `.btn-secondary`: Gray.300 and Gray.750; hovered Gray.350 and Gray.1000.
        pub const SECONDARY: (Color32, Color32) = (G300, G750);
        pub const SECONDARY_HOVER: (Color32, Color32) = (G350, G1000);
        /// `.btn-danger`: `#5c001a` and `#ffd1d6`; hovered `#730022` and `#fff`.
        pub const DANGER: (Color32, Color32) = (
            Color32::from_rgb(0x5c, 0x00, 0x1a),
            Color32::from_rgb(0xff, 0xd1, 0xd6),
        );
        pub const DANGER_HOVER: (Color32, Color32) = (Color32::from_rgb(0x73, 0x00, 0x22), G1000);

        /// `.canvas-banner`: `left: 10px; top: 10px; right: 148px` (`renderBanner`), `gap:
        /// 10px; padding: 9px 10px 9px 12px; border-radius: var(--r); border: 1px solid`.
        pub const BANNER_LEFT: f32 = 10.0;
        pub const BANNER_TOP: f32 = 10.0;
        pub const BANNER_RIGHT: f32 = 148.0;
        pub const BANNER_GAP: f32 = 10.0;
        pub const BANNER_PAD_TOP: f32 = 9.0;
        pub const BANNER_PAD_RIGHT: f32 = 10.0;
        pub const BANNER_PAD_BOTTOM: f32 = 9.0;
        pub const BANNER_PAD_LEFT: f32 = 12.0;
        /// `.b-icon { margin-top: 1px }`; `.b-text .s { opacity: .85; margin-top: 2px }`;
        /// `.b-actions { gap: 6px }`.
        pub const BANNER_ICON_TOP: f32 = 1.0;
        pub const BANNER_SUB_TOP: f32 = 2.0;
        pub const BANNER_SUB_OPACITY: f32 = 0.85;
        pub const BANNER_ACTIONS_GAP: f32 = 6.0;
        /// `.canvas-banner.warn`, `.info`, `.err`: fill, border, words (`--alert-warn-text`).
        pub const WARN: (Color32, Color32, Color32) = (
            Color32::from_rgb(0x2a, 0x1d, 0x0b),
            Color32::from_rgb(0x6b, 0x43, 0x10),
            Color32::from_rgb(0xff, 0xd2, 0x8a),
        );
        pub const INFO: (Color32, Color32, Color32) = (
            Color32::from_rgb(0x17, 0x14, 0x2a),
            Color32::from_rgb(0x46, 0x33, 0x75),
            Color32::from_rgb(0xe4, 0xd6, 0xff),
        );
        pub const ERR: (Color32, Color32, Color32) = (
            Color32::from_rgb(0x2a, 0x0a, 0x12),
            Color32::from_rgb(0x6e, 0x12, 0x28),
            Color32::from_rgb(0xff, 0xd1, 0xd6),
        );
        /// `.canvas-banner .btn-secondary`: `rgba(255,255,255,.08)`, the banner's words;
        /// hovered `.16` and `#fff`.
        pub const BANNER_BTN: [u8; 4] = [255, 255, 255, 20];
        pub const BANNER_BTN_HOVER: [u8; 4] = [255, 255, 255, 41];
        /// `.ab-btn`: `height: var(--btn); min-width: var(--btn); padding: 0 5px`, its icon
        /// Gray.750: an icon's alone is 24 wide.
        pub const AB_BTN: f32 = 22.0;
        pub const AB_BTN_W: f32 = ICON + 2.0 * 5.0;
    }

    /// `[r, g, b, a]`, not premultiplied, as egui keeps it.
    pub(crate) fn rgba([r, g, b, a]: [u8; 4]) -> Color32 {
        Color32::from_rgba_unmultiplied(r, g, b, a)
    }

    /// `colour` at `opacity`, as CSS's `opacity` draws it.
    pub(crate) fn at_opacity(colour: Color32, opacity: f32) -> Color32 {
        Color32::from_rgba_unmultiplied(
            colour.r(),
            colour.g(),
            colour.b(),
            (opacity * 255.0).round() as u8,
        )
    }

    /// Whether two colours are the same but for a step of rounding in each channel.
    #[track_caller]
    pub(crate) fn same_colour(got: Color32, want: Color32, what: &str) {
        let close = got
            .to_array()
            .iter()
            .zip(want.to_array())
            .all(|(a, b)| a.abs_diff(b) <= 1);
        assert!(close, "{what}: {got:?}, not {want:?}");
    }

    pub(crate) fn painted<S>(h: &Harness<'_, S>) -> Vec<Shape> {
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

    pub(crate) fn rects<S>(h: &Harness<'_, S>) -> Vec<RectShape> {
        painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Rect(rect) => Some(rect),
                _ => None,
            })
            .collect()
    }

    /// The one text `text` painted.
    #[track_caller]
    pub(crate) fn text_shape<S>(h: &Harness<'_, S>, text: &str) -> TextShape {
        let mut found: Vec<TextShape> = painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Text(shown) if shown.galley.text() == text => Some(shown),
                _ => None,
            })
            .collect();
        assert_eq!(found.len(), 1, "{text:?} painted {} times", found.len());
        found.remove(0)
    }

    /// A text's colour as painted: its override, else its first section's.
    pub(crate) fn colour(shown: &TextShape) -> Color32 {
        shown
            .override_text_color
            .unwrap_or(shown.galley.job.sections[0].format.color)
    }

    /// The tint of the icon painted in `rect`, if one is.
    pub(crate) fn icon_at<S>(h: &Harness<'_, S>, rect: Rect) -> Option<Color32> {
        rects(h)
            .into_iter()
            .find(|shape| shape.brush.is_some() && near_rect(shape.rect, rect))
            .map(|shape| shape.fill)
    }

    /// The fills painted in `rect` (no icon's), and their corners.
    pub(crate) fn fills_at<S>(h: &Harness<'_, S>, rect: Rect) -> Vec<(Color32, CornerRadius)> {
        rects(h)
            .into_iter()
            .filter(|shape| {
                shape.brush.is_none()
                    && shape.fill != Color32::TRANSPARENT
                    && near_rect(shape.rect, rect)
            })
            .map(|shape| (shape.fill, shape.corner_radius))
            .collect()
    }

    pub(crate) fn near(a: f32, b: f32) -> bool {
        (a - b).abs() <= egui::emath::GUI_ROUNDING
    }

    pub(crate) fn near_pos(a: Pos2, b: Pos2) -> bool {
        near(a.x, b.x) && near(a.y, b.y)
    }

    pub(crate) fn near_rect(a: Rect, b: Rect) -> bool {
        near_pos(a.min, b.min) && near_pos(a.max, b.max)
    }

    #[track_caller]
    pub(crate) fn at(got: Pos2, want: Pos2, what: &str) {
        assert!(near_pos(got, want), "{what}: {got:?}, not {want:?}");
    }

    #[track_caller]
    pub(crate) fn sized(got: Rect, want: Rect, what: &str) {
        assert!(near_rect(got, want), "{what}: {got:?}, not {want:?}");
    }

    #[track_caller]
    pub(crate) fn font(shown: &TextShape, size: f32, what: &str) {
        assert_eq!(
            shown.galley.job.sections[0].format.font_id,
            FontId::new(size, FontFamily::Proportional),
            "{what}'s font"
        );
    }

    pub(crate) fn widget<S>(h: &Harness<'_, S>, role: Role, label: &str) -> Rect {
        h.get_by_role_and_label(role, label).rect()
    }

    /// The harness's window: the modal is centred in it.
    const WINDOW: Vec2 = Vec2::new(800.0, 600.0);

    /// The close question's parts, as the app draws them, with a key cap on its Save.
    const TITLE: &str = "Save changes to a.silico before closing?";
    const SAID: &str = "It has unsaved edits. Closing without saving drops them.";

    /// The close question in a modal, Save on unless `save_off`.
    fn question(save_off: bool) -> Harness<'static> {
        let mut h = Harness::builder().with_size(WINDOW).build_ui(move |ui| {
            show(ui.ctx(), egui::Id::new("question"), |ui| {
                head(ui, Some(Icon::Save), TITLE);
                body(ui, |ui| text(ui, SAID, theme::MODAL_TEXT));
                foot(
                    ui,
                    [Button::new("Don't save", Kind::Danger)],
                    [
                        Button::new("Cancel", Kind::Secondary),
                        Button::new("Save", Kind::Primary)
                            .cap("↵")
                            .enabled(!save_off),
                    ],
                );
            });
        });
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    /// The modal's frame: its fill's shape.
    fn frame_of<S>(h: &Harness<'_, S>) -> RectShape {
        let mut found: Vec<RectShape> = rects(h)
            .into_iter()
            .filter(|shape| shape.fill == css::G200 && shape.stroke.color == css::G250)
            .collect();
        assert_eq!(found.len(), 1, "the modal's frames");
        found.remove(0)
    }

    #[test]
    fn a_modal_is_the_prototypes_frame_over_its_backdrop() {
        let h = question(false);
        // `.backdrop`: the window, black at 60%.
        let window = Rect::from_min_size(Pos2::ZERO, WINDOW);
        assert!(
            rects(&h)
                .iter()
                .any(|shape| shape.fill == rgba(css::BACKDROP) && near_rect(shape.rect, window)),
            "no backdrop over the window"
        );
        // `.modal`: 440 wide, its border in that, centred.
        let frame = frame_of(&h);
        assert!(near(frame.rect.width(), css::MODAL_W), "{:?}", frame.rect);
        at(
            frame.rect.center(),
            (WINDOW / 2.0).to_pos2(),
            "the modal's middle",
        );
        assert_eq!(frame.stroke.width, css::BORDER);
        assert_eq!(frame.stroke_kind, egui::StrokeKind::Inside);
        assert_eq!(frame.corner_radius, CornerRadius::same(css::R));
        // Its shadow: 15 lower, blurred 50, black at 70%.
        let shadow = rects(&h)
            .into_iter()
            .find(|shape| shape.blur_width > 0.0)
            .expect("the modal's shadow");
        assert_eq!(shadow.blur_width, css::SHADOW_BLUR);
        assert_eq!(shadow.fill, rgba(css::SHADOW));
        sized(
            shadow.rect,
            frame.rect.translate(Vec2::new(0.0, css::SHADOW_Y)),
            "the shadow",
        );
    }

    #[test]
    fn a_modals_head_body_and_foot_are_laid_out_as_the_prototypes() {
        let h = question(false);
        let frame = frame_of(&h).rect;
        let inside = frame.shrink(css::BORDER);
        // `.m-head`: the icon, 8, the title on a 16 line, centred on each other.
        let middle = inside.top() + css::HEAD_TOP + css::LH / 2.0;
        let icon = Rect::from_min_size(
            Pos2::new(inside.left() + css::HEAD_LEFT, middle - css::ICON / 2.0),
            Vec2::splat(css::ICON),
        );
        assert_eq!(icon_at(&h, icon), Some(css::G1000), "the head's icon");
        let title = text_shape(&h, TITLE);
        font(&title, css::HEAD_FS, "the title");
        assert_eq!(colour(&title), css::G1000);
        assert!(near(title.galley.size().y, css::LH), "one line, 16 high");
        at(
            title.pos,
            Pos2::new(icon.right() + css::HEAD_GAP, middle - css::LH / 2.0),
            "the title",
        );
        let head_bottom = inside.top() + css::HEAD_TOP + css::LH + css::HEAD_BOTTOM;
        // `.m-body p`: from the head's foot, 16 in, Gray.750.
        let said = text_shape(&h, SAID);
        font(&said, css::FS, "the words");
        assert_eq!(colour(&said), css::G750);
        at(
            said.pos,
            Pos2::new(inside.left() + css::BODY_SIDE, head_bottom),
            "the words",
        );
        assert!(said.galley.size().x <= inside.width() - 2.0 * css::BODY_SIDE);
        let body_bottom = head_bottom + said.galley.size().y + css::BODY_BOTTOM;
        // `.m-foot`: Don't save at its left; Cancel, 6, then Save at its right; 26 high, 76
        // wide at least; the modal ending 14 under them.
        let top = body_bottom + css::FOOT_TOP;
        let save = widget(&h, Role::Button, "Save");
        let cancel = widget(&h, Role::Button, "Cancel");
        let drop = widget(&h, Role::Button, "Don't save");
        for (rect, what) in [(save, "Save"), (cancel, "Cancel"), (drop, "Don't save")] {
            assert!(near(rect.top(), top), "{what}'s top: {rect:?}, not {top}");
            assert!(near(rect.height(), css::FOOT_BTN_H), "{what}: {rect:?}");
            assert!(
                rect.width() >= css::FOOT_BTN_MIN_W - 0.01,
                "{what}: {rect:?}"
            );
        }
        assert!(near(save.right(), inside.right() - css::FOOT_SIDE));
        assert!(near(cancel.right(), save.left() - css::FOOT_GAP));
        assert!(near(drop.left(), inside.left() + css::FOOT_SIDE));
        assert!(near(
            frame.bottom(),
            save.bottom() + css::FOOT_BOTTOM + css::BORDER
        ));
        // Each as wide as its words padded, 76 at least.
        for (rect, label) in [(cancel, "Cancel"), (drop, "Don't save")] {
            let words = text_shape(&h, label).galley.size().x;
            let width = (words + 2.0 * css::BTN_PADDING).max(css::FOOT_BTN_MIN_W);
            assert!(near(rect.width(), width), "{label}: {rect:?}, not {width}");
        }
    }

    #[test]
    fn each_button_kind_has_the_prototypes_colours_and_the_default_its_key_cap() {
        let mut h = question(false);
        for (label, (fill, ink)) in [
            ("Save", css::PRIMARY),
            ("Cancel", css::SECONDARY),
            ("Don't save", css::DANGER),
        ] {
            let rect = widget(&h, Role::Button, label);
            assert_eq!(
                fills_at(&h, rect),
                [(fill, CornerRadius::same(css::R_SM))],
                "{label}'s fill"
            );
            let words = text_shape(&h, label);
            font(&words, css::FS, label);
            assert_eq!(colour(&words), ink, "{label}'s words");
        }
        // `.btn .kbd`: Save's ↵, 6 after its words, 16 high, in its words' colour at 55%;
        // the two centred in the button together.
        let save = widget(&h, Role::Button, "Save");
        let words = text_shape(&h, "Save");
        let cap = text_shape(&h, "↵");
        same_colour(
            colour(&cap),
            at_opacity(css::PRIMARY.1, css::KBD_OPACITY),
            "the cap's words",
        );
        let outline = rects(&h)
            .into_iter()
            .find(|shape| {
                shape.fill == Color32::TRANSPARENT
                    && shape.stroke.width > 0.0
                    && save.contains_rect(shape.rect)
                    && near(shape.rect.height(), css::KBD_H)
            })
            .expect("the cap's outline");
        same_colour(
            outline.stroke.color,
            at_opacity(css::PRIMARY.1, css::KBD_OPACITY),
            "the cap's outline",
        );
        assert!(near(outline.rect.center().y, save.center().y));
        assert!(near(
            outline.rect.left(),
            words.pos.x + words.galley.size().x + css::BTN_GAP
        ));
        assert!(near(
            outline.rect.right() - save.center().x,
            save.center().x - words.pos.x
        ));
        // Hovered, each takes its hovered colours.
        for (label, (fill, ink)) in [
            ("Save", css::PRIMARY_HOVER),
            ("Cancel", css::SECONDARY_HOVER),
            ("Don't save", css::DANGER_HOVER),
        ] {
            h.get_by_role_and_label(Role::Button, label).hover();
            h.run_steps(2);
            let rect = widget(&h, Role::Button, label);
            assert_eq!(
                fills_at(&h, rect),
                [(fill, CornerRadius::same(css::R_SM))],
                "{label} hovered"
            );
            assert_eq!(
                colour(&text_shape(&h, label)),
                ink,
                "{label}'s words hovered"
            );
        }
        // Off, the primary button is Gray.300 with Gray.500 words.
        let h = question(true);
        let rect = widget(&h, Role::Button, "Save");
        assert_eq!(
            fills_at(&h, rect),
            [(css::PRIMARY_OFF.0, CornerRadius::same(css::R_SM))]
        );
        assert_eq!(colour(&text_shape(&h, "Save")), css::PRIMARY_OFF.1);
    }

    #[test]
    fn a_fields_label_is_small_and_subdued_over_a_26_high_field_and_its_button() {
        let mut h = Harness::builder().with_size(WINDOW).build_ui_state(
            |ui, path: &mut String| {
                show(ui.ctx(), egui::Id::new("path"), |ui| {
                    head(ui, None, "Open document");
                    body(ui, |ui| {
                        let label = label(ui, "Path");
                        let edit = theme::field(
                            ui,
                            egui::TextEdit::singleline(path),
                            "path",
                            theme::FieldKind::Modify,
                            false,
                        );
                        field(
                            ui,
                            edit,
                            label.id,
                            Some(Button::new("Browse…", Kind::Secondary)),
                        );
                    });
                });
            },
            String::new(),
        );
        theme::apply(&h.ctx);
        h.run_steps(4);
        let inside = frame_of(&h).rect.shrink(css::BORDER);
        // No icon: the title at the head's padding.
        let title = text_shape(&h, "Open document");
        assert!(near(title.pos.x, inside.left() + css::HEAD_LEFT));
        let head_bottom = inside.top() + css::HEAD_TOP + css::LH + css::HEAD_BOTTOM;
        // `.lbl`: 10 under the head, 11 in Gray.550 on a 16 line, 4 over the field.
        let label = text_shape(&h, "Path");
        font(&label, css::FS_SMALL, "the label");
        assert_eq!(colour(&label), css::G550);
        assert!(near(label.galley.size().y, css::LH));
        at(
            label.pos,
            Pos2::new(inside.left() + css::BODY_SIDE, head_bottom + css::LBL_TOP),
            "the label",
        );
        // `.field`: 26 high, across the body but for its button, `--gap` from it, as high.
        let top = head_bottom + css::LBL_TOP + css::LH + css::LBL_BOTTOM;
        let field = widget(&h, Role::TextInput, "Path");
        let browse = widget(&h, Role::Button, "Browse…");
        assert!(near(browse.right(), inside.right() - css::BODY_SIDE));
        sized(
            field,
            Rect::from_min_max(
                Pos2::new(inside.left() + css::BODY_SIDE, top),
                Pos2::new(browse.left() - css::GAP, top + css::FIELD_H),
            ),
            "the field",
        );
        assert!(near(browse.top(), top) && near(browse.height(), css::FIELD_H));
        // Browse… is as wide as its words and its padding (`.btn`).
        let words = text_shape(&h, "Browse…");
        assert!(near(
            browse.width(),
            words.galley.size().x + 2.0 * css::BTN_PADDING
        ));
        assert_eq!(
            fills_at(&h, browse),
            [(css::SECONDARY.0, CornerRadius::same(css::R_SM))]
        );
        // Under the field, the body's padding, and the modal's end (no foot here).
        assert!(near(inside.bottom(), top + css::FIELD_H + css::BODY_BOTTOM));
        // Typed into, it reads as the field: it's labelled by the label.
        h.get_by_role_and_label(Role::TextInput, "Path").focus();
        h.run_steps(1);
        h.get_by_role_and_label(Role::TextInput, "Path")
            .type_text("a.silico");
        h.run_steps(2);
        assert_eq!(h.state(), "a.silico");
    }

    #[test]
    fn red_words_in_a_body_are_the_prototypes_red() {
        let mut h = Harness::builder().with_size(WINDOW).build_ui(|ui| {
            show(ui.ctx(), egui::Id::new("said"), |ui| {
                body(ui, |ui| text(ui, "The export failed", theme::ERROR_TEXT));
            });
        });
        theme::apply(&h.ctx);
        h.run_steps(4);
        assert_eq!(colour(&text_shape(&h, "The export failed")), css::RED);
    }
}
