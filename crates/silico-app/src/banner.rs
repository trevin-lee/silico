//! The banner over the canvas (docs/design/m5-ui: its `styles.css`'s `.canvas-banner`
//! rules and `js/app.js`'s `renderBanner`; DESIGN.md's *Disk changed* rows, 6c and 6d): at
//! the canvas's top, clear of the ViewCube, its icon, its title over a line that says more,
//! and its buttons at its right, in its tone's colours. What the file on disk did is told
//! here: reloaded (purple), changed while edits are held (amber), or gone (red).
//!
//! Its words are labels, so the accessibility tree has them as they're drawn. It takes the
//! pointer over all of it, so a click or a drag on it never reaches the viewport under it.

use eframe::egui::epaint::RectShape;
use eframe::egui::text::LayoutJob;
use eframe::egui::{Color32, FontFamily, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2};

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

/// What a banner says and offers.
pub(crate) struct Banner<'a> {
    pub tone: Tone,
    /// Its title (`.b-text .t`), Gray.1000.
    pub title: &'a str,
    /// Its second line (`.b-text .s`), in parts, each monospace (`.mono`) or not.
    pub sub: &'a [(&'a str, bool)],
    /// Its buttons, by their words, each on or not.
    pub buttons: &'a [(&'a str, bool)],
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

/// `banner`, drawn over `canvas`: what was clicked on it.
pub(crate) fn show(ui: &mut Ui, canvas: Rect, banner: &Banner<'_>) -> Clicked {
    let (fill, border, ink) = banner.tone.colours();
    let pad = theme::BANNER_PADDING;
    let left = canvas.left() + theme::BANNER_MARGIN;
    let width = (canvas.right() - theme::BANNER_MARGIN_RIGHT - left).max(0.0);
    let top = canvas.top() + theme::BANNER_MARGIN;
    let inner_left = left + theme::BORDER_WIDTH + pad.leftf();
    let inner_right = left + width - theme::BORDER_WIDTH - pad.rightf();
    let inner_top = top + theme::BORDER_WIDTH + pad.topf();
    // The buttons' places, at the right, in a row (`.b-actions`).
    let buttons: Vec<Button<'_>> = banner
        .buttons
        .iter()
        .map(|(text, on)| Button::new(text, Kind::Banner(ink)).enabled(*on))
        .collect();
    let mut widths: Vec<f32> = buttons
        .iter()
        .map(|button| modal::natural_width(ui, button))
        .collect();
    if banner.dismiss {
        // `.ab-btn`: its icon padded, 22 at least.
        widths.push(
            (ICON_SIZE + 2.0 * theme::APP_BAR_BUTTON_PADDING).max(theme::APP_BAR_BUTTON_SIZE),
        );
    }
    let actions = widths.iter().sum::<f32>()
        + widths.len().saturating_sub(1) as f32 * theme::BANNER_ACTION_GAP;
    // The words between the icon and the buttons, wrapped there.
    let text_left = inner_left + ICON_SIZE + theme::BANNER_GAP;
    let gap = if widths.is_empty() {
        0.0
    } else {
        theme::BANNER_GAP
    };
    let text_width = (inner_right - actions - gap - text_left).max(1.0);
    let title = ui.painter().layout_job(modal::wrapped(
        banner.title,
        theme::BODY_SIZE,
        theme::BANNER_TITLE,
        text_width,
    ));
    let sub_ink = ink.gamma_multiply(theme::BANNER_SUB_OPACITY);
    let mut sub = LayoutJob::default();
    for (part, mono) in banner.sub {
        let mut format = modal::format(theme::BODY_SIZE, sub_ink);
        if *mono {
            format.font_id = FontId::new(theme::MONOSPACE_SIZE, FontFamily::Monospace);
        }
        sub.append(part, 0.0, format);
    }
    sub.wrap.max_width = text_width;
    let sub = ui.painter().layout_job(sub);
    let words = title.size().y + theme::BANNER_SUB_MARGIN_TOP + sub.size().y;
    let content = words
        .max(theme::BUTTON_HEIGHT)
        .max(theme::BANNER_ICON_MARGIN_TOP + ICON_SIZE);
    let height = 2.0 * theme::BORDER_WIDTH + pad.sum().y + content;
    let rect = Rect::from_min_size(pos2(left, top), Vec2::new(width, height));
    // Under its buttons, so it takes the pointer only where they don't.
    ui.interact(
        rect,
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
    modal::label_at(
        ui,
        pos2(
            text_left,
            inner_top + title_height + theme::BANNER_SUB_MARGIN_TOP,
        ),
        sub,
    );
    let mut clicked = Clicked::None;
    let mut x = inner_right - actions;
    for (place, (button, width)) in buttons.iter().zip(&widths).enumerate() {
        let at = Rect::from_min_size(pos2(x, inner_top), Vec2::new(*width, theme::BUTTON_HEIGHT));
        if modal::paint(ui, at, button).clicked() {
            clicked = Clicked::Button(place);
        }
        x += width + theme::BANNER_ACTION_GAP;
    }
    if banner.dismiss {
        let at = Rect::from_min_size(
            pos2(x, inner_top),
            Vec2::new(widths[widths.len() - 1], theme::APP_BAR_BUTTON_SIZE),
        );
        let mut child = ui.new_child(eframe::egui::UiBuilder::new().max_rect(at));
        if bar::button(&mut child, Icon::Close, None, DISMISS, true, None).clicked() {
            clicked = Clicked::Dismiss;
        }
    }
    clicked
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
    const SUB: [(&str, bool); 3] = [
        ("Another program wrote the file: an agent through ", false),
        ("silico serve", true),
        (", git, or an editor.", false),
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
        let mut h = Harness::builder()
            .with_size(Vec2::new(CANVAS.right() + 40.0, CANVAS.bottom() + 40.0))
            .build_ui_state(
                move |ui, drawn: &mut Drawn| {
                    let canvas = ui.allocate_rect(CANVAS, Sense::click_and_drag());
                    if canvas.clicked() {
                        drawn.under += 1;
                    }
                    let buttons: &[(&str, bool)] = match tone {
                        Tone::Info => &[],
                        _ => &[("Reload", true), ("Keep my edits", false)],
                    };
                    let banner = Banner {
                        tone,
                        title: TITLE,
                        sub: &SUB,
                        buttons,
                        dismiss: tone == Tone::Info,
                    };
                    let clicked = show(ui, CANVAS, &banner);
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
            let reload = widget(&h, Role::Button, "Reload");
            let keep = widget(&h, Role::Button, "Keep my edits");
            assert!(near(keep.right(), inner_right));
            assert!(near(reload.right(), keep.left() - css::BANNER_ACTIONS_GAP));
            for (rect, label) in [(reload, "Reload"), (keep, "Keep my edits")] {
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
            assert_eq!(colour(&text_shape(&h, "Reload")), ink, "Reload's words");
            // Off, Keep my edits' words are the disabled grey.
            assert_eq!(colour(&text_shape(&h, "Keep my edits")), css::G400);
            assert!(title.pos.x + title.galley.size().x <= reload.left() - css::BANNER_GAP);
        }
    }

    #[test]
    fn a_banner_button_hovered_is_white_on_white_at_16() {
        let mut h = drawn(Tone::Warning);
        h.get_by_role_and_label(Role::Button, "Reload").hover();
        h.run_steps(2);
        let reload = widget(&h, Role::Button, "Reload");
        assert_eq!(
            fills_at(&h, reload),
            [(rgba(css::BANNER_BTN_HOVER), CornerRadius::same(css::R_SM))]
        );
        assert_eq!(colour(&text_shape(&h, "Reload")), css::G1000);
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
        h.get_by_role_and_label(Role::Button, "Reload").click();
        h.run_steps(2);
        assert_eq!(h.state().clicked, [Clicked::Button(0)]);
        // An off button takes no click.
        h.get_by_role_and_label(Role::Button, "Keep my edits")
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
}
