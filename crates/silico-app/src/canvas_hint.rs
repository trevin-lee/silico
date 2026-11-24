//! The hints over the canvas (docs/design/m5-ui: styles.css's `.canvas-hint`, app.js's
//! `renderCanvasUI`), each a chip centred across the canvas of an icon, words, and a button,
//! on one line (`white-space: nowrap`), in the floating colours with the navigation bar's
//! shadow:
//! - the empty part's: while the part has no feature, its bottom 64 above the canvas's, the
//!   sketch icon, "Start with a sketch: **Create Sketch**, then pick a plane.", and a primary
//!   Create Sketch, which begins Create Sketch's pick as the toolbar's button does;
//! - Create Sketch's pick's (`crate::plane_pick`): while it's on, its top 18 below the
//!   canvas's (`.canvas-hint`'s `top: 18px`), the plane icon, "Pick an origin plane or a flat
//!   face to sketch on", and a ghost Cancel with its key's cap, Esc, which ends the pick, as
//!   Esc does.
//!
//! A hint takes the pointer over all of it, so a click or a drag on it never reaches the
//! viewport under it, and none outside it.

use std::sync::Arc;

use eframe::egui::text::LayoutJob;
use eframe::egui::{
    self, Galley, Rect, Response, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2,
};

use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::modal::{self, Button, Kind};
use crate::tab::Tab;
use crate::theme;

/// The empty part's hint's words, in parts, each emphasised (`.strong`) or not.
const WORDS: [(&str, bool); 3] = [
    ("Start with a sketch: ", false),
    ("Create Sketch", true),
    (", then pick a plane.", false),
];

/// The pick's hint's words, as the prototype has them.
pub const PICK_WORDS: &str = "Pick an origin plane or a flat face to sketch on";

/// The pick's hint's name in the accessibility tree (the empty part's is "Hint").
pub const PICK_HINT: &str = "Pick hint";

/// Whether the empty part's hint shows on `tab`: its document read, with no feature (a
/// failed one counts, as the prototype's `!d.features.length`), Create Sketch able to open
/// (no sketch open, no dialog open or being read, the tab not closing: [`Tab::can_sketch`]),
/// and no pick on (the prototype's `!S.pickPlane`).
fn shown(tab: &Tab) -> bool {
    let empty = tab
        .timeline
        .as_ref()
        .and_then(|timeline| timeline.features.as_ref())
        .is_some_and(Vec::is_empty);
    empty && tab.can_sketch() && tab.plane_pick.is_none()
}

/// The empty part's hint over `canvas`, clipped to it, while it shows: its Create Sketch
/// begins Create Sketch's pick ([`Tab::begin_plane_pick`]), as the toolbar's does.
pub(crate) fn empty_part(ui: &mut Ui, canvas: Rect, tab: &mut Tab) {
    if !shown(tab) {
        return;
    }
    let mut words = LayoutJob::default();
    for (part, strong) in WORDS {
        let colour = if strong {
            theme::CANVAS_HINT_STRONG
        } else {
            theme::CANVAS_HINT_TEXT
        };
        words.append(part, 0.0, modal::format(theme::BODY_SIZE, colour));
    }
    let words = ui.painter().layout_job(words);
    let create = Button::new(crate::toolbar::CREATE_SKETCH_WORDS, Kind::Primary);
    let clicked = chip(
        ui,
        canvas,
        Place::Bottom,
        ("empty-part-hint", "Hint"),
        Icon::Sketch,
        words,
        &create,
    )
    .clicked();
    if clicked {
        tab.begin_plane_pick();
        ui.ctx().request_repaint();
    }
}

/// Create Sketch's pick's hint over `canvas`, clipped to it, while the pick is on: its
/// Cancel ends the pick ([`Tab::end_plane_pick`]), as Esc does.
pub(crate) fn plane_pick(ui: &mut Ui, canvas: Rect, tab: &mut Tab) {
    if tab.plane_pick.is_none() {
        return;
    }
    let words = ui.painter().layout_job(modal::wrapped(
        PICK_WORDS,
        theme::BODY_SIZE,
        theme::CANVAS_HINT_TEXT,
        f32::INFINITY,
    ));
    let cap = crate::shortcuts::Command::Cancel.keys_text(ui.ctx());
    let cancel = Button::new("Cancel", Kind::Ghost).cap(&cap);
    let clicked = chip(
        ui,
        canvas,
        Place::Top,
        ("plane-pick-hint", PICK_HINT),
        Icon::Plane,
        words,
        &cancel,
    )
    .clicked();
    if clicked {
        tab.end_plane_pick();
        ui.ctx().request_repaint();
    }
}

/// Where a hint stands, centred across the canvas: its top [`theme::CANVAS_HINT_TOP`] below
/// the canvas's (`.canvas-hint`'s own), or its bottom [`theme::CANVAS_HINT_BOTTOM`] above it
/// (the empty part's `top:auto;bottom:64px`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Top,
    Bottom,
}

/// A hint (`.canvas-hint`) over `canvas`, clipped to it, at `place`: `icon` in the words'
/// colour, `words`, and `button`, centred in a row as high as the highest, inside the hint's
/// padding and border; `named` its scope's ID salt and its group's name in the accessibility
/// tree. Its button's response.
fn chip(
    ui: &mut Ui,
    canvas: Rect,
    place: Place,
    named: (&str, &str),
    icon: Icon,
    words: Arc<Galley>,
    button: &Button<'_>,
) -> Response {
    let size_of_button = Vec2::new(modal::natural_width(ui, button), theme::BUTTON_HEIGHT);
    let pad = theme::CANVAS_HINT_PADDING;
    let gap = theme::CANVAS_HINT_GAP;
    // Its items centred in a row (`align-items: center`) as high as the highest.
    let row = ICON_SIZE.max(words.size().y).max(size_of_button.y);
    let size = Vec2::new(
        2.0 * theme::BORDER_WIDTH
            + pad.leftf()
            + ICON_SIZE
            + gap
            + words.size().x
            + gap
            + size_of_button.x
            + pad.rightf(),
        2.0 * theme::BORDER_WIDTH + pad.topf() + row + pad.bottomf(),
    );
    let top = match place {
        Place::Top => canvas.top() + theme::CANVAS_HINT_TOP,
        Place::Bottom => canvas.bottom() - theme::CANVAS_HINT_BOTTOM - size.y,
    };
    let rect = Rect::from_min_size(pos2(canvas.center().x - size.x / 2.0, top), size);
    let clip = canvas.intersect(ui.clip_rect());
    ui.scope_builder(
        UiBuilder::new()
            .id_salt(named.0)
            .max_rect(rect)
            // Under its button, over the canvas: its padding and border absorb a press.
            .sense(Sense::click_and_drag()),
        |ui| {
            ui.set_clip_rect(clip);
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Group);
                node.set_label(named.1);
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
            icon.image(IconSize::Small)
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
            let button_left = rect.right() - theme::BORDER_WIDTH - pad.rightf() - size_of_button.x;
            let place = Rect::from_min_size(
                pos2(button_left, middle - size_of_button.y / 2.0),
                size_of_button,
            );
            let response = modal::paint(ui, place, button);
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
