//! The banner over the canvas (docs/design/m5-ui: its `styles.css`'s `.canvas-banner`
//! rules and `js/app.js`'s `renderBanner`; DESIGN.md's *Disk changed* rows, 6c and 6d): at
//! the canvas's top, clear of the ViewCube, its icon, its title over a line that says more,
//! and its buttons at its right, in its tone's colours, and under its words, if it has one,
//! its list of what changed (`.diff`). What the file on disk did is told here: reloaded
//! (purple), changed while edits are held (amber), or gone (red).
//!
//! Its words are labels, so the accessibility tree has them as they're drawn. It takes the
//! pointer over all of it, so a click or a drag on it never reaches the viewport under it.

use eframe::egui::epaint::RectShape;
use eframe::egui::text::LayoutJob;
use eframe::egui::{
    Color32, FontFamily, FontId, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2,
};

use crate::bar;
use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::modal::{self, Button, Kind};
use crate::theme;

/// The banner's tone (`.canvas-banner.info`, `.warn`, `.err`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Purple: the session reloaded its file, nothing of the person's lost (6c).
    Info,
    /// Amber: the file changed on disk while edits are held (6d).
    Warning,
    /// Red: the file is gone while edits are held.
    Error,
}

impl Tone {
    /// Its fill, its border, and its words.
    pub(crate) fn colours(self) -> (Color32, Color32, Color32) {
        match self {
            Tone::Info => (
                theme::BANNER_INFO_BG,
                theme::BANNER_INFO_BORDER,
                theme::BANNER_INFO_TEXT,
            ),
            Tone::Warning => (
                theme::BANNER_WARNING_BG,
                theme::BANNER_WARNING_BORDER,
                theme::BANNER_WARNING_TEXT,
            ),
            Tone::Error => (
                theme::BANNER_ERROR_BG,
                theme::BANNER_ERROR_BORDER,
                theme::BANNER_ERROR_TEXT,
            ),
        }
    }

    /// Its icon (`renderBanner`'s): changed, a warning, or an error.
    fn icon(self) -> Icon {
        match self {
            Tone::Info => Icon::Diff,
            Tone::Warning => Icon::Warn,
            Tone::Error => Icon::Error,
        }
    }
}

/// How a part of a banner's second line is written: in its words' type, monospace
/// (`.mono`), or as a dot of a colour (`.state-dot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    Plain,
    Mono,
    Dot(Color32),
}

/// The list under a banner's words (`.diff`): its rows, each the feature's name, the
/// field, and what it went from and to; then the lines after them (how many more, a
/// refusal), each a label of its own.
pub(crate) struct List<'a> {
    pub rows: &'a [crate::changes::Row],
    pub after: &'a [&'a str],
}

/// What a banner says and offers.
pub(crate) struct Banner<'a> {
    pub tone: Tone,
    /// Its title (`.b-text .t`), Gray.1000.
    pub title: &'a str,
    /// Its second line (`.b-text .s`), in parts, each in its style.
    pub sub: &'a [(&'a str, Style)],
    /// What changed, listed under its words, if it lists anything.
    pub list: Option<List<'a>>,
    /// Its buttons, by their words, each on or not.
    pub buttons: &'a [(&'a str, bool)],
    /// Why each button, by its place among them, is off, shown as its tooltip while it is:
    /// none for one with no reason given.
    pub tips: &'a [Option<&'a str>],
    /// Whether it has a close button, labelled Dismiss (`.ab-btn`).
    pub dismiss: bool,
}

/// What was clicked on a banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Clicked {
    None,
    /// The button at this place among its buttons.
    Button(usize),
    Dismiss,
}

/// The label of a banner's close button.
pub(crate) const DISMISS: &str = "Dismiss";

/// One of a banner's actions: a button, by its place among them, or Dismiss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Button(usize),
    Dismiss,
}

/// `banner`, drawn over `canvas` and clipped to it, the pointer taken only there: what was
/// clicked on it. At [`theme::BANNER_MARGIN`] from the canvas's left and top, and
/// [`theme::BANNER_MARGIN_RIGHT`] from its right, clear of the ViewCube, unless that leaves
/// it narrower than [`theme::BANNER_MIN_WIDTH`]; its buttons beside its words, or under
/// them where the words would have less than [`theme::BANNER_WORDS_MIN_WIDTH`].
pub(crate) fn show(ui: &mut Ui, canvas: Rect, banner: &Banner<'_>) -> Clicked {
    let clip = canvas.intersect(ui.clip_rect());
    let mut child = ui.new_child(UiBuilder::new().id_salt("canvas-banner").max_rect(canvas));
    child.set_clip_rect(clip);
    let ui = &mut child;
    let (fill, border, ink) = banner.tone.colours();
    let pad = theme::BANNER_PADDING;
    let left = canvas.left() + theme::BANNER_MARGIN;
    let beside_cube = canvas.right() - theme::BANNER_MARGIN_RIGHT - left;
    let right = if beside_cube >= theme::BANNER_MIN_WIDTH {
        canvas.right() - theme::BANNER_MARGIN_RIGHT
    } else {
        canvas.right() - theme::BANNER_MARGIN
    };
    let width = (right - left).max(0.0);
    let top = canvas.top() + theme::BANNER_MARGIN;
    let inner_left = left + theme::BORDER_WIDTH + pad.leftf();
    let inner_right = left + width - theme::BORDER_WIDTH - pad.rightf();
    let inner_top = top + theme::BORDER_WIDTH + pad.topf();
    // The actions (`.b-actions`) and their sizes: the buttons, `.btn` high, and Dismiss
    // (`.ab-btn`: its icon padded, 22 at least).
    let buttons: Vec<Button<'_>> = banner
        .buttons
        .iter()
        .map(|(text, on)| Button::new(text, Kind::Banner(ink)).enabled(*on))
        .collect();
    let mut actions: Vec<(Action, Vec2)> = buttons
        .iter()
        .enumerate()
        .map(|(place, button)| {
            (
                Action::Button(place),
                Vec2::new(modal::natural_width(ui, button), theme::BUTTON_HEIGHT),
            )
        })
        .collect();
    if banner.dismiss {
        actions.push((
            Action::Dismiss,
            Vec2::new(
                (ICON_SIZE + 2.0 * theme::APP_BAR_BUTTON_PADDING).max(theme::APP_BAR_BUTTON_SIZE),
                theme::APP_BAR_BUTTON_SIZE,
            ),
        ));
    }
    let row = actions.iter().map(|(_, size)| size.x).sum::<f32>()
        + actions.len().saturating_sub(1) as f32 * theme::BANNER_ACTION_GAP;
    // The words after the icon: beside the actions, or over them where that's too narrow.
    let text_left = inner_left + ICON_SIZE + theme::BANNER_GAP;
    let gap = if actions.is_empty() {
        0.0
    } else {
        theme::BANNER_GAP
    };
    let beside = inner_right - row - gap - text_left;
    let stacked = beside < theme::BANNER_WORDS_MIN_WIDTH;
    let text_width = if stacked {
        (inner_right - text_left).max(theme::BANNER_WORDS_MIN_WIDTH)
    } else {
        beside
    };
    let title = ui.painter().layout_job(modal::wrapped(
        banner.title,
        theme::BODY_SIZE,
        theme::BANNER_TITLE,
        text_width,
    ));
    let sub_ink = ink.gamma_multiply(theme::BANNER_SUB_OPACITY);
    let mut sub = LayoutJob::default();
    for (part, style) in banner.sub {
        let mut format = modal::format(theme::BODY_SIZE, sub_ink);
        match style {
            Style::Plain => {}
            Style::Mono => {
                format.font_id = FontId::new(theme::MONOSPACE_SIZE, FontFamily::Monospace);
            }
            Style::Dot(colour) => format.color = colour.gamma_multiply(theme::BANNER_SUB_OPACITY),
        }
        sub.append(part, 0.0, format);
    }
    sub.wrap.max_width = text_width;
    let sub = ui.painter().layout_job(sub);
    let list = banner
        .list
        .as_ref()
        .map(|list| laid_out(ui, list, ink, text_width));
    let words = title.size().y
        + theme::BANNER_SUB_MARGIN_TOP
        + sub.size().y
        + list
            .as_ref()
            .map_or(0.0, |list| theme::BANNER_LIST_MARGIN_TOP + list.height);
    // The actions' places: in a row at the right, at the words' top; or under the words,
    // from their left, a row after another as they fit (one alone in a row whatever its
    // width).
    let mut places = Vec::with_capacity(actions.len());
    if stacked {
        let mut at = pos2(text_left, inner_top + words + theme::BANNER_ACTION_GAP);
        let mut row_height: f32 = 0.0;
        for (action, size) in &actions {
            if at.x > text_left && at.x + size.x > inner_right {
                at = pos2(text_left, at.y + row_height + theme::BANNER_ACTION_GAP);
                row_height = 0.0;
            }
            places.push((*action, Rect::from_min_size(at, *size)));
            at.x += size.x + theme::BANNER_ACTION_GAP;
            row_height = row_height.max(size.y);
        }
    } else {
        let mut x = inner_right - row;
        for (action, size) in &actions {
            places.push((*action, Rect::from_min_size(pos2(x, inner_top), *size)));
            x += size.x + theme::BANNER_ACTION_GAP;
        }
    }
    let bottom = places
        .iter()
        .map(|(_, place)| place.bottom())
        .fold(inner_top + words, f32::max)
        .max(inner_top + theme::BANNER_ICON_MARGIN_TOP + ICON_SIZE);
    let rect = Rect::from_min_max(
        pos2(left, top),
        pos2(left + width, bottom + pad.bottomf() + theme::BORDER_WIDTH),
    );
    // Under its buttons, so it takes the pointer only where they don't, and only where it
    // shows.
    ui.interact(
        rect.intersect(clip),
        ui.id().with("canvas-banner"),
        Sense::CLICK | Sense::DRAG,
    );
    ui.painter().add(RectShape::new(
        rect,
        theme::BANNER_CORNER_RADIUS,
        fill,
        Stroke::new(theme::BORDER_WIDTH, border),
        StrokeKind::Inside,
    ));
    banner
        .tone
        .icon()
        .image(IconSize::Small)
        .tint(ink)
        .paint_at(
            ui,
            Rect::from_min_size(
                pos2(inner_left, inner_top + theme::BANNER_ICON_MARGIN_TOP),
                Vec2::splat(ICON_SIZE),
            ),
        );
    let title_height = title.size().y;
    modal::label_at(ui, pos2(text_left, inner_top), title);
    let sub_top = inner_top + title_height + theme::BANNER_SUB_MARGIN_TOP;
    let sub_height = sub.size().y;
    modal::label_at(ui, pos2(text_left, sub_top), sub);
    if let Some(list) = list {
        list.paint(
            ui,
            pos2(
                text_left,
                sub_top + sub_height + theme::BANNER_LIST_MARGIN_TOP,
            ),
        );
    }
    let mut clicked = Clicked::None;
    for (action, place) in places {
        match action {
            Action::Button(at) => {
                if let Some(button) = buttons.get(at) {
                    let mut response = modal::paint(ui, place, button);
                    if let Some(tip) = banner.tips.get(at).copied().flatten() {
                        response = response.on_disabled_hover_text(tip);
                    }
                    if response.clicked() {
                        clicked = Clicked::Button(at);
                    }
                }
            }
            Action::Dismiss => {
                let mut child = ui.new_child(UiBuilder::new().max_rect(place));
                if bar::button(&mut child, Icon::Close, None, DISMISS, true, None).clicked() {
                    clicked = Clicked::Dismiss;
                }
            }
        }
    }
    clicked
}

/// A banner's list laid out: each row's three cells, the line after them, and how high it
/// all is.
struct LaidOut {
    rows: Vec<[std::sync::Arc<eframe::egui::Galley>; 3]>,
    after: Vec<std::sync::Arc<eframe::egui::Galley>>,
    /// The first two columns' widths (`grid-template-columns: auto auto 1fr`).
    columns: [f32; 2],
    height: f32,
}

/// `list` laid out in `width`, in the small size: the names in [`theme::CHANGED_OUTSIDE`],
/// the rest in `ink`; the first two columns as wide as their widest cell, the third
/// wrapping in what's left.
fn laid_out(ui: &Ui, list: &List<'_>, ink: Color32, width: f32) -> LaidOut {
    let one_line = |text: &str, colour| {
        let mut job =
            LayoutJob::single_section(text.to_owned(), modal::format(theme::HEADER_SIZE, colour));
        job.wrap.max_width = f32::INFINITY;
        ui.painter().layout_job(job)
    };
    let cells: Vec<_> = list
        .rows
        .iter()
        .map(|row| {
            (
                one_line(&row.feature, theme::CHANGED_OUTSIDE),
                one_line(&row.field, ink),
            )
        })
        .collect();
    let columns = cells
        .iter()
        .fold([0.0_f32; 2], |[names, fields], (name, field)| {
            [names.max(name.size().x), fields.max(field.size().x)]
        });
    let third = (width - columns[0] - columns[1] - 2.0 * theme::BANNER_LIST_COLUMN_GAP).max(0.0);
    let mut height = 0.0;
    let rows: Vec<_> = cells
        .into_iter()
        .zip(list.rows)
        .map(|((feature, field), row)| {
            let text =
                ui.painter()
                    .layout_job(modal::wrapped(&row.text, theme::HEADER_SIZE, ink, third));
            if height > 0.0 {
                height += theme::BANNER_LIST_ROW_GAP;
            }
            height += feature.size().y.max(field.size().y).max(text.size().y);
            [feature, field, text]
        })
        .collect();
    let after = list
        .after
        .iter()
        .map(|after| {
            let galley =
                ui.painter()
                    .layout_job(modal::wrapped(after, theme::HEADER_SIZE, ink, width));
            if height > 0.0 {
                height += theme::BANNER_LIST_ROW_GAP;
            }
            height += galley.size().y;
            galley
        })
        .collect();
    LaidOut {
        rows,
        after,
        columns,
        height,
    }
}

impl LaidOut {
    /// Drawn from `at`, each cell a label, so the accessibility tree reads it.
    fn paint(self, ui: &mut Ui, at: eframe::egui::Pos2) {
        let mut y = at.y;
        let lefts = [
            at.x,
            at.x + self.columns[0] + theme::BANNER_LIST_COLUMN_GAP,
            at.x + self.columns[0] + self.columns[1] + 2.0 * theme::BANNER_LIST_COLUMN_GAP,
        ];
        let mut first = true;
        for cells in self.rows {
            if !first {
                y += theme::BANNER_LIST_ROW_GAP;
            }
            first = false;
            let height = cells.iter().map(|cell| cell.size().y).fold(0.0, f32::max);
            for (cell, left) in cells.into_iter().zip(lefts) {
                modal::label_at(ui, pos2(left, y), cell);
            }
            y += height;
        }
        for after in self.after {
            if !first {
                y += theme::BANNER_LIST_ROW_GAP;
            }
            first = false;
            let height = after.size().y;
            modal::label_at(ui, pos2(at.x, y), after);
            y += height;
        }
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Vec2};
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::modal::tests::{
        at, at_opacity, colour, css, fills_at, font, icon_at, near, rects, rgba, same_colour,
        sized, text_shape, widget,
    };

    /// The canvas the banner is over, away from the window's corner.
    const CANVAS: Rect = Rect::from_min_max(Pos2::new(40.0, 30.0), Pos2::new(1000.0, 600.0));

    const TITLE: &str = "a.silico changed on disk while you have unsaved edits.";
    const SUB: [(&str, Style); 3] = [
        (
            "Another program wrote the file: an agent through ",
            Style::Plain,
        ),
        ("silico serve", Style::Mono),
        (", git, or an editor.", Style::Plain),
    ];

    /// What the harness keeps: what was clicked on the banner, and how many clicks reached
    /// the canvas under it.
    #[derive(Default)]
    struct Drawn {
        clicked: Vec<Clicked>,
        under: usize,
    }

    /// The banner of `tone` drawn over the canvas, which takes clicks as the viewport does;
    /// with the amber banner's buttons, or else only Dismiss.
    fn drawn(tone: Tone) -> Harness<'static, Drawn> {
        drawn_over(tone, CANVAS)
    }

    /// [`drawn`], over `canvas`, in a window wider than the default's.
    fn drawn_over(tone: Tone, canvas: Rect) -> Harness<'static, Drawn> {
        let buttons: &[(&str, bool)] = match tone {
            Tone::Info => &[],
            _ => &[
                ("Reload, discard my edits", true),
                ("Keep my edits…", false),
            ],
        };
        drawn_with(tone, canvas, buttons, tone == Tone::Info)
    }

    /// The banner of `tone` with `buttons`, and Dismiss if `dismiss`, drawn over `canvas`,
    /// which takes clicks as the viewport does, in a window wider than the default's.
    fn drawn_with(
        tone: Tone,
        canvas: Rect,
        buttons: &'static [(&'static str, bool)],
        dismiss: bool,
    ) -> Harness<'static, Drawn> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(CANVAS.right() + 40.0, CANVAS.bottom() + 40.0))
            .build_ui_state(
                move |ui, drawn: &mut Drawn| {
                    let canvas = ui.allocate_rect(canvas, Sense::click_and_drag());
                    if canvas.clicked() {
                        drawn.under += 1;
                    }
                    let banner = Banner {
                        tone,
                        title: TITLE,
                        sub: &SUB,
                        list: None,
                        buttons,
                        tips: &[],
                        dismiss,
                    };
                    let clicked = show(ui, canvas.rect, &banner);
                    if clicked != Clicked::None {
                        drawn.clicked.push(clicked);
                    }
                },
                Drawn::default(),
            );
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    /// The banner's place: its fill's shape, which is its tone's.
    fn banner_rect(h: &Harness<'_, Drawn>, fill: Color32) -> Rect {
        let found: Vec<Rect> = rects(h)
            .into_iter()
            .filter(|shape| shape.fill == fill)
            .map(|shape| shape.rect)
            .collect();
        assert_eq!(found.len(), 1, "the banner's fills");
        found[0]
    }

    #[test]
    fn the_banner_sits_over_the_canvas_clear_of_the_cube_in_its_tones_colours() {
        for (tone, (fill, border, ink)) in [
            (Tone::Warning, css::WARN),
            (Tone::Info, css::INFO),
            (Tone::Error, css::ERR),
        ] {
            let h = drawn(tone);
            let rect = banner_rect(&h, fill);
            let shape = rects(&h)
                .into_iter()
                .find(|shape| shape.fill == fill)
                .unwrap();
            assert_eq!(shape.corner_radius, CornerRadius::same(css::R), "{tone:?}");
            assert_eq!(
                shape.stroke,
                egui::epaint::Stroke::new(css::BORDER, border),
                "{tone:?}"
            );
            assert_eq!(shape.stroke_kind, egui::StrokeKind::Inside);
            assert!(near(rect.left(), CANVAS.left() + css::BANNER_LEFT));
            assert!(near(rect.top(), CANVAS.top() + css::BANNER_TOP));
            assert!(near(rect.right(), CANVAS.right() - css::BANNER_RIGHT));
            let inner_left = rect.left() + css::BORDER + css::BANNER_PAD_LEFT;
            let inner_top = rect.top() + css::BORDER + css::BANNER_PAD_TOP;
            // `.b-icon`: in the banner's words' colour, 1 lower than the words.
            let icon = Rect::from_min_size(
                Pos2::new(inner_left, inner_top + css::BANNER_ICON_TOP),
                Vec2::splat(css::ICON),
            );
            assert_eq!(icon_at(&h, icon), Some(ink), "{tone:?}'s icon");
            // `.b-text .t`: 10 after the icon, Gray.1000 on 16 lines.
            let title = text_shape(&h, TITLE);
            font(&title, css::FS, "the title");
            assert_eq!(colour(&title), css::G1000);
            at(
                title.pos,
                Pos2::new(icon.right() + css::BANNER_GAP, inner_top),
                "the title",
            );
            // `.b-text .s`: 2 under it, the banner's words at 85%, `silico serve` monospace.
            let sub = text_shape(&h, &SUB.map(|(part, _)| part).concat());
            at(
                sub.pos,
                Pos2::new(
                    title.pos.x,
                    title.pos.y + title.galley.size().y + css::BANNER_SUB_TOP,
                ),
                "the second line",
            );
            let sections = &sub.galley.job.sections;
            for section in sections {
                same_colour(
                    section.format.color,
                    at_opacity(ink, css::BANNER_SUB_OPACITY),
                    "the second line",
                );
            }
            assert_eq!(
                sections[1].format.font_id.family,
                FontFamily::Monospace,
                "silico serve"
            );
            assert_eq!(
                sections[0].format.font_id,
                FontId::new(css::FS, FontFamily::Proportional)
            );
            // As high as its words, padded, and its border.
            let words = title.galley.size().y + css::BANNER_SUB_TOP + sub.galley.size().y;
            assert!(near(
                rect.bottom(),
                inner_top + words.max(css::BTN_H) + css::BANNER_PAD_BOTTOM + css::BORDER
            ));
            let inner_right = rect.right() - css::BORDER - css::BANNER_PAD_RIGHT;
            if tone == Tone::Info {
                // `.ab-btn`: Dismiss, 22 square, at the right, its × Gray.750.
                let dismiss = widget(&h, Role::Button, DISMISS);
                sized(
                    dismiss,
                    Rect::from_min_size(
                        Pos2::new(inner_right - css::AB_BTN_W, inner_top),
                        Vec2::new(css::AB_BTN_W, css::AB_BTN),
                    ),
                    "Dismiss",
                );
                let cross = Rect::from_center_size(dismiss.center(), Vec2::splat(css::ICON));
                assert_eq!(icon_at(&h, cross), Some(css::G750));
                // The words end 10 before it.
                assert!(title.pos.x + title.galley.size().x <= dismiss.left() - css::BANNER_GAP);
                continue;
            }
            // `.b-actions`: Reload, 6, Keep my edits, at the right, at the words' top, each
            // `.btn` high and padded, white at 8% with the banner's words.
            let reload = widget(&h, Role::Button, "Reload, discard my edits");
            let keep = widget(&h, Role::Button, "Keep my edits…");
            assert!(near(keep.right(), inner_right));
            assert!(near(reload.right(), keep.left() - css::BANNER_ACTIONS_GAP));
            for (rect, label) in [
                (reload, "Reload, discard my edits"),
                (keep, "Keep my edits…"),
            ] {
                assert!(near(rect.top(), inner_top) && near(rect.height(), css::BTN_H));
                let words = text_shape(&h, label);
                assert!(near(
                    rect.width(),
                    words.galley.size().x + 2.0 * css::BTN_PADDING
                ));
                assert_eq!(
                    fills_at(&h, rect),
                    [(rgba(css::BANNER_BTN), CornerRadius::same(css::R_SM))],
                    "{label}'s fill"
                );
            }
            assert_eq!(
                colour(&text_shape(&h, "Reload, discard my edits")),
                ink,
                "Reload's words"
            );
            // Off, Keep my edits' words are the disabled grey.
            assert_eq!(colour(&text_shape(&h, "Keep my edits…")), css::G400);
            assert!(title.pos.x + title.galley.size().x <= reload.left() - css::BANNER_GAP);
        }
    }

    #[test]
    fn a_banner_button_hovered_is_white_on_white_at_16() {
        let mut h = drawn(Tone::Warning);
        h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
            .hover();
        h.run_steps(2);
        let reload = widget(&h, Role::Button, "Reload, discard my edits");
        assert_eq!(
            fills_at(&h, reload),
            [(rgba(css::BANNER_BTN_HOVER), CornerRadius::same(css::R_SM))]
        );
        assert_eq!(
            colour(&text_shape(&h, "Reload, discard my edits")),
            css::G1000
        );
    }

    #[test]
    fn a_click_on_the_banner_is_its_own_never_the_canvas_under_it() {
        let mut h = drawn(Tone::Warning);
        let banner = banner_rect(&h, css::WARN.0);
        // On its words: nothing, and not the canvas.
        h.get_by_label(TITLE).click();
        h.run_steps(2);
        // Its corner, past its words and buttons.
        let corner = banner.left_bottom() + Vec2::new(4.0, -4.0);
        h.event(egui::Event::PointerMoved(corner));
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: corner,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        h.run_steps(2);
        assert_eq!(
            h.state().under,
            0,
            "a click on the banner reached the canvas"
        );
        assert!(h.state().clicked.is_empty());
        // Its button: its own click.
        h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
            .click();
        h.run_steps(2);
        assert_eq!(h.state().clicked, [Clicked::Button(0)]);
        // An off button takes no click.
        h.get_by_role_and_label(Role::Button, "Keep my edits…")
            .click();
        h.run_steps(2);
        assert_eq!(h.state().clicked, [Clicked::Button(0)]);
        // Under the banner, the canvas has its clicks.
        let below = Pos2::new(banner.center().x, banner.bottom() + 20.0);
        h.event(egui::Event::PointerMoved(below));
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: below,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        h.run_steps(2);
        assert_eq!(h.state().under, 1);
        // The purple banner's Dismiss.
        let mut h = drawn(Tone::Info);
        h.get_by_role_and_label(Role::Button, DISMISS).click();
        h.run_steps(2);
        assert_eq!(h.state().clicked, [Clicked::Dismiss]);
        assert_eq!(h.state().under, 0);
    }

    #[test]
    fn on_a_narrow_canvas_the_banner_stays_inside_it_and_its_height_bounded() {
        // The prototype's canvas is never narrow: the banner takes the canvas's width but
        // 10 at each side, its buttons go under its words, and nothing of it is drawn, or
        // takes the pointer, outside the canvas, however narrow.
        for (width, inside) in [(300.0, true), (60.0, false)] {
            let canvas = Rect::from_min_size(CANVAS.min, Vec2::new(width, CANVAS.height()));
            for tone in [Tone::Warning, Tone::Info] {
                let mut h = drawn_over(tone, canvas);
                let fill = tone.colours().0;
                let banner = banner_rect(&h, fill);
                assert!(near(banner.left(), canvas.left() + css::BANNER_LEFT));
                if inside {
                    assert!(
                        near(banner.right(), canvas.right() - css::BANNER_LEFT),
                        "{tone:?} at {width}: {banner:?}"
                    );
                }
                assert!(
                    banner.height() < 240.0,
                    "{tone:?} at {width}: {} high",
                    banner.height()
                );
                // What shows of every shape the banner's, inside the canvas (the harness's
                // own panel, behind it all in the panel's fill, aside).
                for clipped in &h.output().shapes {
                    if matches!(&clipped.shape, egui::Shape::Rect(panel) if panel.fill == theme::PANEL_BG)
                    {
                        continue;
                    }
                    let bounds = clipped.shape.visual_bounding_rect();
                    let shown = clipped.clip_rect.intersect(bounds);
                    assert!(
                        !shown.is_positive() || canvas.expand(0.5).contains_rect(shown),
                        "{tone:?} at {width}: a shape shows at {shown:?}"
                    );
                }
                let labels: &[&str] = match tone {
                    Tone::Info => &[DISMISS],
                    _ => &["Reload, discard my edits", "Keep my edits…"],
                };
                for label in labels {
                    let rect = widget(&h, Role::Button, label);
                    if inside {
                        assert!(
                            canvas.contains_rect(rect),
                            "{tone:?} at {width}: {label} at {rect:?}"
                        );
                        // Under the words, not beside them.
                        let title = text_shape(&h, TITLE);
                        assert!(rect.top() >= title.pos.y + title.galley.size().y);
                    }
                }
                // A click past the canvas, at either side, level with the banner's top,
                // reaches nothing of the banner's.
                for past in [
                    Pos2::new(canvas.left() - 20.0, banner.top() + 12.0),
                    Pos2::new(canvas.right() + 20.0, banner.top() + 12.0),
                ] {
                    h.event(egui::Event::PointerMoved(past));
                    for pressed in [true, false] {
                        h.event(egui::Event::PointerButton {
                            pos: past,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        });
                    }
                    h.run_steps(2);
                }
                assert!(h.state().clicked.is_empty(), "{tone:?} at {width}");
            }
        }
    }

    /// What the prototype never shows (its canvas is never narrow), as theme.rs proposes
    /// it: the least width a banner keeps beside the ViewCube, and the least its words keep
    /// beside its buttons.
    const BESIDE_CUBE_MIN: f32 = 320.0;
    const WORDS_MIN: f32 = 200.0;

    /// A canvas at [`CANVAS`]'s corner, `width` wide.
    fn canvas_of(width: f32) -> Rect {
        Rect::from_min_size(CANVAS.min, Vec2::new(width, CANVAS.height()))
    }

    /// The banner's insides, from its drawn rect: its padding's top, the words' left (after
    /// the icon and its gap), and the right of its padding.
    fn insides(banner: Rect) -> (f32, f32, f32) {
        let inner_top = banner.top() + css::BORDER + css::BANNER_PAD_TOP;
        let text_left =
            banner.left() + css::BORDER + css::BANNER_PAD_LEFT + css::ICON + css::BANNER_GAP;
        let inner_right = banner.right() - css::BORDER - css::BANNER_PAD_RIGHT;
        (inner_top, text_left, inner_right)
    }

    /// An action's width (`.btn`: its words and `padding: 0 10px`; `.ab-btn`: 24), its
    /// words' advance as the font measures them where they're painted.
    fn action_width(h: &Harness<'_, Drawn>, label: &str) -> f32 {
        if label == DISMISS {
            css::AB_BTN_W
        } else {
            text_shape(h, label).galley.size().x + 2.0 * css::BTN_PADDING
        }
    }

    /// The words wrap at `width`: the title and the second line.
    #[track_caller]
    fn words_wrap_at(h: &Harness<'_, Drawn>, width: f32, what: &str) {
        let title = text_shape(h, TITLE);
        let sub = text_shape(h, &SUB.map(|(part, _)| part).concat());
        for (shown, which) in [(&title, "title"), (&sub, "second line")] {
            assert_eq!(
                shown.galley.job.wrap.max_width, width,
                "{what}: the {which}'s wrap"
            );
            assert!(shown.galley.size().x <= width, "{what}: the {which}");
        }
    }

    /// The actions under the words (`.b-actions` wrapped), in `rows` by label: the first
    /// row 6 under the words, each next 6 under the last (the actions 22 high), each from
    /// the words' left, its actions 6 apart; each row holding what fits in the words' room
    /// as flex-wrap fills it, one at least; and the banner ending under the last, padded.
    #[track_caller]
    fn under_the_words(h: &Harness<'_, Drawn>, banner: Rect, rows: &[&[&str]], what: &str) {
        let (inner_top, text_left, inner_right) = insides(banner);
        let room = inner_right - text_left;
        let title = text_shape(h, TITLE);
        let sub = text_shape(h, &SUB.map(|(part, _)| part).concat());
        let words = title.galley.size().y + css::BANNER_SUB_TOP + sub.galley.size().y;
        let mut top = inner_top + words + css::BANNER_ACTIONS_GAP;
        for (at, row) in rows.iter().enumerate() {
            let widths: Vec<f32> = row.iter().map(|label| action_width(h, label)).collect();
            let used =
                widths.iter().sum::<f32>() + (widths.len() - 1) as f32 * css::BANNER_ACTIONS_GAP;
            assert!(
                widths.len() == 1 || used <= room,
                "{what}: row {at} is {used} wide in {room}"
            );
            if let Some(next) = rows.get(at + 1) {
                let more = used + css::BANNER_ACTIONS_GAP + action_width(h, next[0]);
                assert!(more > room, "{what}: {} fits in row {at}", next[0]);
            }
            let mut left = text_left;
            for (label, width) in row.iter().zip(&widths) {
                sized(
                    widget(h, Role::Button, label),
                    Rect::from_min_size(Pos2::new(left, top), Vec2::new(*width, css::BTN_H)),
                    &format!("{what}: {label}"),
                );
                left += width + css::BANNER_ACTIONS_GAP;
            }
            top += css::BTN_H + css::BANNER_ACTIONS_GAP;
        }
        let last = top - css::BANNER_ACTIONS_GAP;
        assert!(
            near(banner.bottom(), last + css::BANNER_PAD_BOTTOM + css::BORDER),
            "{what}: the banner ends at {}, not under its last row at {last}",
            banner.bottom()
        );
    }

    #[test]
    fn the_banner_keeps_clear_of_the_cube_only_with_320_beside_it() {
        // The canvas from x 40: the banner from 10 in, to 148 from the canvas's right while
        // that leaves it 320 wide, or else to 10 from it.
        for (width, right) in [
            // 1040 - 148: the prototype's place.
            (1000.0, 892.0),
            // 518 - 148: 320 wide beside the cube, exactly.
            (478.0, 370.0),
            // 517 - 10: 319 beside it would be too narrow.
            (477.0, 507.0),
            // 440 - 10.
            (400.0, 430.0),
        ] {
            for tone in [Tone::Warning, Tone::Info] {
                let h = drawn_over(tone, canvas_of(width));
                let banner = banner_rect(&h, tone.colours().0);
                assert!(near(banner.left(), 50.0), "{tone:?} at {width}: {banner:?}");
                assert!(
                    near(banner.right(), right),
                    "{tone:?} at {width}: {banner:?}, not to {right}"
                );
            }
        }
        assert!(near(
            478.0 - css::BANNER_LEFT - css::BANNER_RIGHT,
            BESIDE_CUBE_MIN
        ));
    }

    #[test]
    fn the_buttons_go_under_the_words_only_where_the_words_would_have_less_than_200() {
        // Dismiss alone, on a canvas 302 wide: the banner from 50 to 332, its padding's
        // right at 321, its words from 87 (50, its border, 12, the icon, 10); Dismiss's 24
        // and its 10 leave the words 321 - 24 - 10 - 87 = 200, so it stays beside them.
        let h = drawn_over(Tone::Info, canvas_of(302.0));
        let banner = banner_rect(&h, css::INFO.0);
        assert!(near(banner.right(), 332.0), "{banner:?}");
        let (inner_top, text_left, inner_right) = insides(banner);
        assert!(near(
            inner_right - css::AB_BTN_W - css::BANNER_GAP - text_left,
            WORDS_MIN
        ));
        sized(
            widget(&h, Role::Button, DISMISS),
            Rect::from_min_size(
                Pos2::new(inner_right - css::AB_BTN_W, inner_top),
                Vec2::new(css::AB_BTN_W, css::AB_BTN),
            ),
            "Dismiss beside 200",
        );
        words_wrap_at(&h, WORDS_MIN, "beside 200");
        // One narrower, 199: Dismiss goes under, and the words take the room to its
        // padding's right, 320 - 87.
        let h = drawn_over(Tone::Info, canvas_of(301.0));
        let banner = banner_rect(&h, css::INFO.0);
        under_the_words(&h, banner, &[&[DISMISS]], "beside 199");
        words_wrap_at(&h, 233.0, "beside 199");
    }

    #[test]
    fn under_the_words_the_buttons_fill_rows_as_they_fit() {
        // 300 wide: the banner 280, its words' room 280 - 1 - 10 - 1 - 12 - 14 - 10 = 232,
        // Reload and Keep my edits too wide for one row.
        let h = drawn_over(Tone::Warning, canvas_of(300.0));
        let banner = banner_rect(&h, css::WARN.0);
        assert!(near(banner.width(), 280.0), "{banner:?}");
        under_the_words(
            &h,
            banner,
            &[&["Reload, discard my edits"], &["Keep my edits…"]],
            "at 300",
        );
        words_wrap_at(&h, 232.0, "at 300");
        // 60 wide: no room at all, each alone in its row, the words wrapped at 200.
        for tone in [Tone::Warning, Tone::Info] {
            let h = drawn_over(tone, canvas_of(60.0));
            let banner = banner_rect(&h, tone.colours().0);
            let rows: &[&[&str]] = match tone {
                Tone::Info => &[&[DISMISS]],
                _ => &[&["Reload, discard my edits"], &["Keep my edits…"]],
            };
            under_the_words(&h, banner, rows, &format!("{tone:?} at 60"));
            words_wrap_at(&h, WORDS_MIN, &format!("{tone:?} at 60"));
        }
        // Two short buttons and Dismiss, the words' room exactly as wide as the three and
        // their gaps: one row, Dismiss ending at the padding. A 32nd narrower, Dismiss goes
        // to a row of its own.
        const SHORT: [(&str, bool); 2] = [("Reload", true), ("Keep", true)];
        let measured = drawn_with(Tone::Warning, CANVAS, &SHORT, true);
        let row = action_width(&measured, "Reload")
            + css::BANNER_ACTIONS_GAP
            + action_width(&measured, "Keep")
            + css::BANNER_ACTIONS_GAP
            + css::AB_BTN_W;
        // The canvas: the row, the words' 37 from the banner's left and 11 to its right,
        // and the banner's 10 at each side.
        let exact = row + 37.0 + 11.0 + 2.0 * css::BANNER_LEFT;
        for (width, rows, what) in [
            (exact, &[&["Reload", "Keep", DISMISS][..]][..], "exactly"),
            (
                exact - 1.0 / 32.0,
                &[&["Reload", "Keep"][..], &[DISMISS][..]][..],
                "a 32nd short",
            ),
        ] {
            let h = drawn_with(Tone::Warning, canvas_of(width), &SHORT, true);
            let banner = banner_rect(&h, css::WARN.0);
            let (_, text_left, inner_right) = insides(banner);
            if what == "exactly" {
                assert_eq!(inner_right - text_left, row, "the room");
            }
            under_the_words(&h, banner, rows, what);
            words_wrap_at(&h, WORDS_MIN, what);
        }
    }
}
