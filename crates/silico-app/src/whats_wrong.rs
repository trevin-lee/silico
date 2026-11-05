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
use crate::properties::{self, CardAct};
use crate::tab::Tab;
use crate::theme;

/// Its name in the accessibility tree (the prototype's `aria-label`).
pub(crate) const TITLE: &str = "What's wrong";

/// What its list says with nothing wrong (the prototype's), and while it's being read.
pub(crate) const NOTHING_WRONG: &str = "No problems. Every feature built.";
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

/// What's Wrong over the window, of `tab`: what was asked of it.
pub(crate) fn show(ctx: &egui::Context, tab: &Tab) -> Asked {
    let window = ctx.content_rect().width();
    let width = theme::WHATS_WRONG_WIDTH.min(window - theme::WHATS_WRONG_WINDOW_MARGIN);
    let (asked, backdrop) = modal::show_sized(ctx, egui::Id::new("whats-wrong"), width, |ui| {
        ui.scope_builder(UiBuilder::new().id_salt("whats-wrong"), |ui| {
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Dialog);
                node.set_label(TITLE);
            });
            if head(ui, tab.name()) {
                return Asked::Close;
            }
            body(ui, tab).map_or(Asked::None, Asked::Card)
        })
        .inner
    });
    match asked {
        Asked::None if backdrop => Asked::Close,
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
fn body(ui: &mut Ui, tab: &Tab) -> Option<CardAct> {
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
fn cards(ui: &mut Ui, tab: &Tab) -> Option<CardAct> {
    let Some(listed) = tab.listed() else {
        properties::hint(ui, &[(READING, false)]);
        return None;
    };
    if listed.is_empty() {
        properties::hint(ui, &[(NOTHING_WRONG, false)]);
        return None;
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
    use super::more;

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
