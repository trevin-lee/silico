//! What's Wrong (docs/design/m5.0-app.md, section 9, *Showing a problem*: "a click on the
//! root lists every problem"; docs/ux/solidworks.md, §5, *Errors and warnings*; the
//! prototype's `whatswrong` modal in `js/app.js`'s `renderModal`, and its `.modal`,
//! `.m-head`, and `.m-body` rules): a modal over the window listing every problem in the
//! shown document, the gate's and each feature's errors and warnings, in the document's
//! order, each an error card (`crate::properties::card`) with Edit, Show in timeline, and
//! Copy as JSON. The status bar's errors, the timeline's, and a click on the browser's
//! document row open it (the registry's What's wrong, `crate::shortcuts`); Esc, its ×, or a
//! click on its backdrop closes it, as do Edit and Show in timeline.
//!
//! What it lists is a `check` of the model the timeline shows, which the tab reads while
//! it's open (`Tab::listed`), never waited on: until it answers, it says so.
//!
//! Left out: SolidWorks' columns, sorting, and *Show errors* and *Show warnings* (§5), which
//! the prototype's list of cards doesn't have.

use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{self, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};

use crate::bar;
use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::modal;
use crate::properties::{self, CardAct, CardOut};
use crate::tab::Tab;
use crate::theme;

/// Its name in the accessibility tree (the prototype's `aria-label`).
pub(crate) const TITLE: &str = "What's wrong";

/// What its list says with nothing wrong (the prototype's), and while it's being read.
pub const NOTHING_WRONG: &str = "No problems. Every feature built.";
pub(crate) const READING: &str = "Reading what's wrong…";

/// What the person asked of it this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Asked {
    None,
    /// Its ×, or a click on its backdrop.
    Close,
    /// A card's Edit or Show in timeline.
    Card(CardAct),
}

/// What's Wrong over the window, of `tab`: what was asked of it, and the broken
/// reference's candidate whose row the pointer is over, if it's over one.
pub(crate) fn show(ctx: &egui::Context, tab: &Tab) -> (Asked, Option<String>) {
    let window = ctx.content_rect().width();
    let width = theme::WHATS_WRONG_WIDTH.min(window - theme::WHATS_WRONG_WINDOW_MARGIN);
    let (asked, backdrop) = modal::show_sized(ctx, egui::Id::new("whats-wrong"), width, |ui| {
        ui.scope_builder(UiBuilder::new().id_salt("whats-wrong"), |ui| {
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Dialog);
                node.set_label(TITLE);
            });
            if head(ui, tab.name()) {
                return (Asked::Close, None);
            }
            let out = body(ui, tab);
            (out.asked.map_or(Asked::None, Asked::Card), out.hovered)
        })
        .inner
    });
    match asked {
        (Asked::None, hovered) if backdrop => (Asked::Close, hovered),
        asked => asked,
    }
}

/// The head (`.m-head`): the error icon and "What's wrong in" the document's name at its
/// left, and its × (`.ab-btn`, labelled Close) at its right, centred on its middle.
/// Whether the × was clicked.
fn head(ui: &mut Ui, name: &str) -> bool {
    let pad = theme::MODAL_HEAD_PADDING;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(
        vec2(width, pad.sum().y + theme::APP_BAR_BUTTON_SIZE),
        Sense::hover(),
    );
    // Its parts placed in a child, so none moves the body's cursor back up.
    let ui = &mut ui.new_child(UiBuilder::new().max_rect(rect));
    let middle = rect.top() + pad.topf() + theme::APP_BAR_BUTTON_SIZE / 2.0;
    let left = rect.left() + pad.leftf();
    let right = rect.right() - pad.rightf();
    let close_width =
        (ICON_SIZE + 2.0 * theme::APP_BAR_BUTTON_PADDING).max(theme::APP_BAR_BUTTON_SIZE);
    let close = Rect::from_min_size(
        pos2(
            right - close_width,
            middle - theme::APP_BAR_BUTTON_SIZE / 2.0,
        ),
        vec2(close_width, theme::APP_BAR_BUTTON_SIZE),
    );
    let closed = ui
        .scope_builder(UiBuilder::new().max_rect(close), |ui| {
            bar::button(ui, Icon::Close, None, "Close", true, None)
        })
        .inner
        .clicked();
    Icon::Error
        .image(IconSize::Small)
        .tint(theme::MODAL_TITLE)
        .paint_at(
            ui,
            Rect::from_min_size(pos2(left, middle - ICON_SIZE / 2.0), Vec2::splat(ICON_SIZE)),
        );
    let at = left + ICON_SIZE + theme::MODAL_HEAD_GAP;
    let mut job = LayoutJob::single_section(
        format!("What's wrong in {name}"),
        modal::format(theme::MODAL_TITLE_SIZE, theme::MODAL_TITLE),
    );
    job.wrap =
        TextWrapping::truncate_at_width((close.left() - theme::MODAL_HEAD_GAP - at).max(1.0));
    let galley = ui.painter().layout_job(job);
    modal::label_at(ui, pos2(at, middle - galley.size().y / 2.0), galley);
    closed
}

/// The body (`.m-body`, its own padding: `0 4px 8px`), scrolling where the modal would be
/// taller than the window lets it: a card for each problem `tab` lists, or, with none, the
/// prototype's hint that nothing's wrong, or, while they're read, that they are. What a
/// card's button asked, if one was.
fn body(ui: &mut Ui, tab: &Tab) -> CardOut {
    egui::ScrollArea::vertical()
        .id_salt("whats-wrong-body")
        .max_height(ui.available_height().max(0.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Frame::NONE
                .inner_margin(theme::WHATS_WRONG_BODY_PADDING)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    cards(ui, tab)
                })
                .inner
        })
        .inner
}

/// Each problem's card (`errCard(p)`: Edit, Show in timeline, and Copy as JSON), by the
/// name of the feature it names, then how many errors and warnings the core counted past
/// those it listed, as a card's "the first N of M" counts its elements; or the hint.
fn cards(ui: &mut Ui, tab: &Tab) -> CardOut {
    let Some(listed) = tab.listed() else {
        properties::hint(ui, &[(READING, false)]);
        return CardOut::default();
    };
    if listed.is_empty() {
        properties::hint(ui, &[(NOTHING_WRONG, false)]);
        return CardOut::default();
    }
    let asked = properties::document_cards(ui, tab, listed, "whats-wrong");
    let unlisted = tab.unlisted();
    for (count, one, many) in [
        (unlisted.errors, "error", "errors"),
        (unlisted.warnings, "warning", "warnings"),
    ] {
        if let Some(more) = more(count, one, many) {
            properties::hint(ui, &[(&more, false)]);
        }
    }
    asked
}

/// What's said of `count` more problems than are listed, `one` or `many` of them ("and 30
/// more warnings"): nothing for none.
fn more(count: u64, one: &str, many: &str) -> Option<String> {
    match count {
        0 => None,
        1 => Some(format!("and 1 more {one}")),
        count => Some(format!("and {count} more {many}")),
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::accesskit::Role;
    use eframe::egui::epaint::RectShape;
    use eframe::egui::{Event, Modifiers, PointerButton, Pos2};
    use egui_kittest::Harness;
    use silico_core::command::{Problems, Revision, SessionId, TimelineAnswer};

    use super::*;
    use crate::modal::tests::{at, font, icon_at, near, rects, sized, text_shape, widget};

    /// The prototype's tokens and geometry (docs/design/m5-ui/styles.css, and `renderModal`'s
    /// `whatswrong` in js/app.js).
    mod css {
        use eframe::egui::Color32;

        /// `--g200`, `--g250`, `--g1000`.
        pub const G200: Color32 = Color32::from_rgb(0x21, 0x21, 0x21);
        pub const G250: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
        pub const G1000: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
        /// `<div class="modal" style="width:560px">`, `border-box`, its border 1;
        /// `.modal { max-width: calc(100vw - 40px) }`.
        pub const WIDTH: f32 = 560.0;
        pub const WINDOW_MARGIN: f32 = 40.0;
        pub const BORDER: f32 = 1.0;
        /// `.m-head`: `display: flex; align-items: center; gap: 8px; padding: 12px 12px 8px
        /// 16px; font-size: 14px`, on `body`'s 16 px line.
        pub const HEAD_TOP: f32 = 12.0;
        pub const HEAD_RIGHT: f32 = 12.0;
        pub const HEAD_LEFT: f32 = 16.0;
        pub const HEAD_GAP: f32 = 8.0;
        pub const HEAD_FS: f32 = 14.0;
        pub const LH: f32 = 16.0;
        /// `svg.i`: `--icon`, 14 square.
        pub const ICON: f32 = 14.0;
        /// `.ab-btn`: `height: var(--btn); min-width: var(--btn); padding: 0 5px`: 22 high,
        /// and with its icon alone, 14 + 2 × 5 = 24 wide.
        pub const AB_BTN_H: f32 = 22.0;
        pub const AB_BTN_W: f32 = 14.0 + 2.0 * 5.0;
    }

    /// What's Wrong of `tab`, as it's shown, and everything it was asked.
    struct Shown {
        tab: Tab,
        asked: Vec<Asked>,
    }

    /// The tab of `path`, its timeline listing nothing wrong.
    fn clean(path: &str) -> Tab {
        let none = Problems::from(silico_core::Error::new(
            silico_core::ErrorCode::Internal,
            "unused",
        ));
        let mut tab = Tab::opened(SessionId::parse("s1").unwrap(), path.to_owned(), &none);
        tab.timeline = Some(TimelineAnswer {
            revision: Revision::OPENED,
            model: Some(Revision::OPENED),
            problems: 0,
            features: Some(Vec::new()),
            bodies: Vec::new(),
        });
        tab
    }

    /// What's Wrong of `tab` in a window `window` in size.
    fn shown(tab: Tab, window: Vec2) -> Harness<'static, Shown> {
        let mut h = Harness::builder().with_size(window).build_ui_state(
            |ui, shown: &mut Shown| {
                let (asked, _hovered) = show(ui.ctx(), &shown.tab);
                if asked != Asked::None {
                    shown.asked.push(asked);
                }
            },
            Shown {
                tab,
                asked: Vec::new(),
            },
        );
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    /// The modal's frame: its fill's shape.
    fn frame(h: &Harness<'_, Shown>) -> RectShape {
        let mut found: Vec<RectShape> = rects(h)
            .into_iter()
            .filter(|shape| shape.fill == css::G200 && shape.stroke.color == css::G250)
            .collect();
        assert_eq!(found.len(), 1, "the modal's frames");
        found.remove(0)
    }

    /// A click of the primary button at `at`.
    fn click_at(h: &mut Harness<'_, Shown>, at: Pos2) {
        for pressed in [true, false] {
            let input = h.input_mut();
            input.events.push(Event::PointerMoved(at));
            input.events.push(Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            h.run();
        }
        h.run_steps(2);
    }

    #[test]
    fn whats_wrong_is_560_wide_its_head_the_icon_its_title_and_its_close_ab_btn() {
        let window = Vec2::new(800.0, 600.0);
        let h = shown(clean("/models/Part.silico"), window);
        let frame = frame(&h).rect;
        assert!(near(frame.width(), css::WIDTH), "{frame:?}");
        assert!(near(frame.center().x, window.x / 2.0), "{frame:?}");
        let inside = frame.shrink(css::BORDER);
        // `align-items: center` on the head's tallest item, the × 22 high.
        let middle = inside.top() + css::HEAD_TOP + css::AB_BTN_H / 2.0;
        let icon = Rect::from_min_size(
            Pos2::new(inside.left() + css::HEAD_LEFT, middle - css::ICON / 2.0),
            Vec2::splat(css::ICON),
        );
        assert_eq!(icon_at(&h, icon), Some(css::G1000), "the error icon");
        let title = text_shape(&h, "What's wrong in Part.silico");
        font(&title, css::HEAD_FS, "the title");
        assert!(near(title.galley.size().y, css::LH), "one 16 line");
        assert_eq!(
            title
                .override_text_color
                .unwrap_or(title.galley.job.sections[0].format.color),
            css::G1000
        );
        at(
            title.pos,
            Pos2::new(icon.right() + css::HEAD_GAP, middle - css::LH / 2.0),
            "the title",
        );
        sized(
            widget(&h, Role::Button, "Close"),
            Rect::from_min_size(
                Pos2::new(
                    inside.right() - css::HEAD_RIGHT - css::AB_BTN_W,
                    middle - css::AB_BTN_H / 2.0,
                ),
                Vec2::new(css::AB_BTN_W, css::AB_BTN_H),
            ),
            "the close button",
        );
    }

    #[test]
    fn in_a_window_narrower_than_600_whats_wrong_is_the_window_less_40_wide() {
        // `max-width: calc(100vw - 40px)`: 500 - 40.
        let h = shown(clean("/models/Part.silico"), Vec2::new(500.0, 600.0));
        let frame = frame(&h).rect;
        assert!(near(frame.width(), 500.0 - css::WINDOW_MARGIN), "{frame:?}");
    }

    #[test]
    fn a_name_too_long_for_the_head_is_cut_short_a_gap_before_the_close_button() {
        let name = "a-bracket-for-the-left-hand-mount-of-the-second-prototype-run-revised.silico";
        let h = shown(clean(&format!("/models/{name}")), Vec2::new(800.0, 600.0));
        let title = text_shape(&h, &format!("What's wrong in {name}"));
        let close = widget(&h, Role::Button, "Close");
        // Never under the ×, nor within `.m-head`'s gap of it; one line, cut short with an
        // ellipsis no more than a glyph and the ellipsis short of the room it has.
        let room = close.left() - css::HEAD_GAP;
        let right = title.pos.x + title.galley.size().x;
        assert!(right <= room + 0.5, "{right} past {room}");
        assert!(
            right > room - 2.0 * css::HEAD_FS,
            "{right}, short of {room}"
        );
        assert_eq!(title.galley.rows.len(), 1);
        assert!(title.galley.rows[0].text().ends_with('…'));
    }

    #[test]
    fn a_click_on_the_backdrop_closes_whats_wrong_and_one_inside_it_doesnt() {
        let mut h = shown(clean("/models/Part.silico"), Vec2::new(800.0, 600.0));
        let frame = frame(&h).rect;
        // Inside: on its hint.
        click_at(&mut h, frame.center());
        assert!(h.state().asked.is_empty(), "{:?}", h.state().asked);
        // Outside it, on the backdrop (`.backdrop`'s `closeModal`).
        click_at(&mut h, Pos2::new(frame.left() / 2.0, frame.top() / 2.0));
        assert_eq!(h.state().asked, [Asked::Close]);
    }

    #[test]
    fn what_is_said_of_those_unlisted_counts_them_in_the_singular_or_plural() {
        assert_eq!(more(0, "error", "errors"), None);
        assert_eq!(
            more(1, "error", "errors").as_deref(),
            Some("and 1 more error")
        );
        assert_eq!(
            more(30, "warning", "warnings").as_deref(),
            Some("and 30 more warnings")
        );
    }
}
