//! A hint over the canvas (docs/design/m5-ui: styles.css's `.canvas-hint`, app.js's
//! `renderCanvasUI`): a chip centred across the canvas, of an icon, a line of words with
//! their emphasis, and one button, on one line (`white-space: nowrap`), in the floating
//! colours with the navigation bar's shadow. [`show`] draws one.
//!
//! The prototype draws two. The empty part's ([`empty_part`]) stands 64 above the canvas's
//! bottom while the part has no feature: "Start with a sketch: **Create Sketch**, then pick a
//! plane.", its Create Sketch opening Create Sketch's planes as the toolbar's button does.
//! Its plane pick's, "Pick an origin plane or a flat face to sketch on" with Cancel at the
//! canvas's top, the app hasn't: its Create Sketch opens the planes as a menu
//! (`crate::toolbar::planes`), with no pick to cancel, so [`show`] is the one chip either
//! would be drawn with, each at its own height.
//!
//! A hint takes the pointer over all of it, so a click or a drag on it never reaches the
//! viewport under it, and none outside it.

use std::sync::Arc;

use eframe::egui::text::LayoutJob;
use eframe::egui::{
    self, Rect, RectAlign, Response, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2,
};
use silico_core::command::Sessions;
use silico_core::kernel::Clients;

use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::modal::{self, Button, Kind};
use crate::tab::Tab;
use crate::theme;

/// What a screen reader calls a hint's chip, a group of its words and its button.
pub(crate) const HINT_LABEL: &str = "Hint";

/// The empty part's words, in parts, each emphasised (`.strong`) or not.
pub(crate) const EMPTY_PART_WORDS: [(&str, bool); 3] = [
    ("Start with a sketch: ", false),
    ("Create Sketch", true),
    (", then pick a plane.", false),
];

/// What a hint shows: its icon, its words in parts (each emphasised or not), and its button.
pub(crate) struct Hint<'a> {
    pub icon: Icon,
    pub words: &'a [(&'a str, bool)],
    pub button: Button<'a>,
}

/// `hint` over `canvas`, clipped to it, under `id_salt`, centred across it with its top
/// where `top` puts it for its height (the plane pick's 18 below the canvas's top, the empty
/// part's bottom [`theme::CANVAS_HINT_BOTTOM`] above the canvas's): its button's response.
/// It takes the pointer over its rect alone (its padding and border too), so a press there
/// never reaches the canvas, and one beside it does.
pub(crate) fn show(
    ui: &mut Ui,
    canvas: Rect,
    id_salt: &str,
    hint: &Hint<'_>,
    top: impl FnOnce(f32) -> f32,
) -> Response {
    let mut words = LayoutJob::default();
    for (part, strong) in hint.words {
        let colour = if *strong {
            theme::CANVAS_HINT_STRONG
        } else {
            theme::CANVAS_HINT_TEXT
        };
        words.append(part, 0.0, modal::format(theme::BODY_SIZE, colour));
    }
    let words = ui.painter().layout_job(words);
    let button = Vec2::new(modal::natural_width(ui, &hint.button), theme::BUTTON_HEIGHT);
    let pad = theme::CANVAS_HINT_PADDING;
    let gap = theme::CANVAS_HINT_GAP;
    // Its items centred in a row (`align-items: center`) as high as the highest.
    let row = ICON_SIZE.max(words.size().y).max(button.y);
    let size = Vec2::new(
        2.0 * theme::BORDER_WIDTH
            + pad.leftf()
            + ICON_SIZE
            + gap
            + words.size().x
            + gap
            + button.x
            + pad.rightf(),
        2.0 * theme::BORDER_WIDTH + pad.topf() + row + pad.bottomf(),
    );
    let rect = Rect::from_min_size(pos2(canvas.center().x - size.x / 2.0, top(size.y)), size);
    let clip = canvas.intersect(ui.clip_rect());
    ui.scope_builder(
        UiBuilder::new()
            .id_salt(id_salt)
            .max_rect(rect)
            // Under its button, over the canvas: its padding and border absorb a press.
            .sense(Sense::click_and_drag()),
        |ui| {
            ui.set_clip_rect(clip);
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Group);
                node.set_label(HINT_LABEL);
            });
            let corner = theme::NORMAL_CORNER_RADIUS;
            let painter = ui.painter();
            painter.add(theme::CANVAS_HINT_SHADOW.as_shape(rect, corner));
            painter.rect(
                rect,
                corner,
                theme::CANVAS_HINT_BG,
                Stroke::new(theme::BORDER_WIDTH, theme::CANVAS_HINT_BORDER),
                StrokeKind::Inside,
            );
            let middle = rect.top() + theme::BORDER_WIDTH + pad.topf() + row / 2.0;
            let icon_left = rect.left() + theme::BORDER_WIDTH + pad.leftf();
            hint.icon
                .image(IconSize::Small)
                .tint(theme::CANVAS_HINT_TEXT)
                .paint_at(
                    ui,
                    Rect::from_min_size(
                        pos2(icon_left, middle - ICON_SIZE / 2.0),
                        Vec2::splat(ICON_SIZE),
                    ),
                );
            let words_left = icon_left + ICON_SIZE + gap;
            let words_height = words.size().y;
            modal::label_at(
                ui,
                pos2(words_left, middle - words_height / 2.0),
                Arc::clone(&words),
            );
            let button_left = rect.right() - theme::BORDER_WIDTH - pad.rightf() - button.x;
            let place = Rect::from_min_size(pos2(button_left, middle - button.y / 2.0), button);
            let response = modal::paint(ui, place, &hint.button);
            // Its whole place its own (egui senses a scope by the rect it ended with), and
            // its bounds that place.
            ui.expand_to_include_rect(rect);
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_bounds(crate::panes::bounds(rect));
            });
            response
        },
    )
    .inner
}

/// Whether the empty part's hint shows on `tab`: its document read, with no feature, and
/// Create Sketch able to open (no sketch open, no dialog open or being read, the tab not
/// closing: [`Tab::can_sketch`]), nor its planes open elsewhere (the prototype's plane pick).
fn empty_part_shown(ctx: &egui::Context, tab: &Tab) -> bool {
    let empty = tab
        .timeline
        .as_ref()
        .and_then(|timeline| timeline.features.as_ref())
        .is_some_and(Vec::is_empty);
    empty && tab.can_sketch() && !crate::toolbar::planes_open(ctx)
}

/// The empty part's hint (app.js's `renderCanvasUI`, its `bottom:64px`), over `canvas`,
/// while it shows: its Create Sketch opens Create Sketch's planes, above it, and a plane
/// chosen there starts a sketch on it through `sessions` as the toolbar's does
/// ([`Tab::create_sketch`]). Its own planes open, it stays, the menu's anchor.
pub(crate) fn empty_part<P: Clients + 'static>(
    ui: &mut Ui,
    canvas: Rect,
    tab: &mut Tab,
    sessions: &Arc<Sessions<P>>,
) {
    if !empty_part_shown(ui.ctx(), tab) {
        return;
    }
    let hint = Hint {
        icon: Icon::Sketch,
        words: &EMPTY_PART_WORDS,
        button: Button::new(crate::toolbar::CREATE_SKETCH_WORDS, Kind::Primary),
    };
    let bottom = canvas.bottom() - theme::CANVAS_HINT_BOTTOM;
    let response = show(ui, canvas, "empty-part-hint", &hint, |height| {
        bottom - height
    });
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_has_popup(egui::accesskit::HasPopup::Menu);
    });
    let popup = egui::Popup::menu(&response)
        .align(RectAlign::TOP_START)
        .gap(theme::NAVIGATION_BAR_MENU_GAP);
    let shown = crate::bar::as_menu(popup).show(crate::toolbar::planes);
    if shown.is_some() {
        // Its own planes: not the pick it hides from.
        crate::toolbar::planes_not_open(ui.ctx());
    }
    if let Some(plane) = shown.and_then(|shown| shown.inner) {
        tab.create_sketch(sessions, plane);
        ui.ctx().request_repaint();
    }
}
