//! The timeline along the window's bottom (docs/design/m5.0-app.md, section 8, *The
//! timeline*; the approved prototype, docs/design/m5-ui/DESIGN.md, *The idea in one line*
//! and *egui mapping*, and its `.tl-*` rules): a panel [`theme::TIMELINE_HEIGHT`] high,
//! its label, each feature in order as its kind's icon on a 30 point item, and at its right
//! how many errors and features there are, the errors a button that opens What's Wrong
//! (`crate::whats_wrong`). The rebuild runs visibly along it: the running
//! step's item spins, the items not yet known built are dimmed, and once it ends each item
//! shows its health, a failed one red ([`step_of_each`]). A click selects a feature, a
//! double-click (or Edit Feature in its menu) opens its dialog (a sketch's, Edit Sketch,
//! sketch mode), a drag moves it
//! (`move_feature`), and hovering it tints the faces it made in the viewport (`View::linked`).
//!
//! Left out, each for a reason: the rollback bar (the core has no rollback; Fusion's marker
//! is out of M5, DESIGN.md flags Onshape's as a proposal), the gear's timeline options (none
//! exist), and the reverse of
//! the hover linking, a face hovered outlining its feature here (`.hovered-from-canvas`):
//! SEAM, the outline's look, from `tab.selection().hovered_feature()` (M5.7: the feature
//! that made what the pointer is over in the canvas).

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Rect, Sense, Shape, Stroke, Ui, UiBuilder, Vec2,
    WidgetInfo, WidgetType, pos2, vec2,
};
use silico_core::command::{Rebuild, RebuildState, Revision, Sessions};
use silico_core::document::FeatureId;
use silico_core::kernel::Clients;
use silico_core::timeline::{Entry, Health};

use crate::icons::{Icon, IconSize};
use crate::panes::{self, MarkLook};
use crate::tab::Tab;
use crate::theme;

/// The pane's name, as the accessibility tree labels it.
pub const LABEL: &str = "Timeline";

/// The track's name, as the accessibility tree labels it (the prototype's `aria-label`).
pub const TRACK_LABEL: &str = "Features in order";

/// What a feature's item shows of the rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// No rebuild running (or the feature suppressed): its health, as the newest timeline
    /// marks it.
    Settled(Health),
    /// Built in the running rebuild: an input, however far back, of the step running, which
    /// can't have started before it settled.
    Built,
    /// The step running, as `status` names it.
    Running,
    /// Not known to be built: after the running step in its lineage, or off it.
    Queued,
}

impl Step {
    /// Its words, after the feature's type (the item's description, and its tooltip's).
    pub fn words(self) -> &'static str {
        match self {
            Step::Settled(Health::Healthy) => "Healthy",
            Step::Settled(Health::Warning) => "Warning",
            Step::Settled(Health::Error) => "Error",
            Step::Settled(Health::Suppressed) => "Suppressed",
            Step::Settled(Health::Unknown) => "Not built",
            Step::Built => "Built",
            Step::Running => "Rebuilding",
            Step::Queued => "Queued",
        }
    }
}

/// A feature's type in words, as the prototype says it (`FEATURE_KIND` in `js/app.js`) for
/// the types it lists, and as the toolbar's commands name the rest (the primitives, and
/// Union, Subtract, and Intersect for the booleans); the type as written for one neither
/// names.
pub fn kind_words(kind: &str) -> &str {
    match kind {
        "sketch" => "Sketch",
        "pad" => "Pad",
        "pocket" => "Pocket",
        "fillet" => "Fillet",
        "chamfer" => "Chamfer",
        "revolve" => "Revolve",
        "groove" => "Groove",
        "box" => "Box",
        "cylinder" => "Cylinder",
        "sphere" => "Sphere",
        "cone" => "Cone",
        "torus" => "Torus",
        "fuse" => "Union",
        "cut" => "Subtract",
        "common" => "Intersect",
        other => other,
    }
}

/// What each of `entries`, the document's features at revision `listed` (the timeline's
/// `revision`), shows of `rebuild`, the session's, as `status` gives it. Nothing is shown
/// built unless it certainly is.
///
/// While it runs, `status` names one feature, the first of the step started last of those
/// running (`Progress::running`, `4kq2m7vd pad`), and counts the rest: that feature runs.
/// If the rebuild is of the revision `entries` list, every input the running feature names
/// there, however far back, is built (a step starts only once its inputs settle). If it's
/// another revision's (a move or an edit answered, its timeline not read yet), the inputs
/// listed may not be the ones rebuilt, so none is shown built. Every other feature is
/// queued until the rebuild ends: one on another branch may have built already, but
/// `status` doesn't say which. SEAM: with the features each settled step was planned for
/// (or each step's state) in `Progress`, every built item would show so, and every running
/// one spin, the parallel branches too.
///
/// A suppressed feature is never rebuilt: it stays suppressed throughout, if `entries` are
/// the revision rebuilt.
pub fn step_of_each(entries: &[Entry], listed: Revision, rebuild: Option<&Rebuild>) -> Vec<Step> {
    let settled = || {
        entries
            .iter()
            .map(|entry| Step::Settled(entry.health))
            .collect()
    };
    let Some(rebuild) = rebuild.filter(|rebuild| rebuild.state == RebuildState::Running) else {
        return settled();
    };
    let place: BTreeMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(at, entry)| (entry.id.as_str(), at))
        .collect();
    let running = rebuild
        .progress
        .as_ref()
        .and_then(|progress| progress.running.as_deref())
        .and_then(|label| label.split_whitespace().next())
        .and_then(|id| place.get(id).copied());
    let same = rebuild.revision == listed;
    let mut built = vec![false; entries.len()];
    if let Some(at) = running.filter(|_| same) {
        let mut inputs: Vec<&FeatureId> = entries[at].inputs.iter().collect();
        while let Some(input) = inputs.pop() {
            if let Some(&input_at) = place.get(input.as_str())
                && !built[input_at]
            {
                built[input_at] = true;
                inputs.extend(&entries[input_at].inputs);
            }
        }
    }
    entries
        .iter()
        .enumerate()
        .map(|(at, entry)| {
            if same && (entry.suppressed || entry.health == Health::Suppressed) {
                Step::Settled(Health::Suppressed)
            } else if running == Some(at) {
                Step::Running
            } else if built[at] {
                Step::Built
            } else {
                Step::Queued
            }
        })
        .collect()
}

/// How an item is drawn (`.tl-item` and its states, in styles.css's order, each later rule
/// winning over an earlier one): its fill, its icon's colour, the line along its bottom, the
/// opacity of the whole, a suppressed one's strike, the spinner, and the mark at its corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub fill: Option<Color32>,
    pub icon: Color32,
    pub line: Option<Color32>,
    pub opacity: f32,
    pub strike: bool,
    pub spinner: bool,
    pub badge: Option<Health>,
}

/// An item's look, of a feature at `step`, a sketch or not, hovered or not, and selected or
/// not: `:hover`, then `.selected`, then `.sketch`, then `.err` and `.warn`, as styles.css
/// gives them, so a selected sketch's icon is the sketch's grey on the selection's blue, and
/// a failed feature's icon is red whatever else it is.
pub fn look(step: Step, sketch: bool, hovered: bool, selected: bool) -> Look {
    let mut look = Look {
        fill: None,
        icon: theme::TIMELINE_ITEM_ICON,
        line: None,
        opacity: 1.0,
        strike: false,
        spinner: false,
        badge: None,
    };
    if hovered {
        look.fill = Some(theme::TIMELINE_ITEM_HOVER_BG);
        look.icon = theme::TIMELINE_ITEM_HOVER_ICON;
    }
    if selected {
        look.fill = Some(theme::TIMELINE_ITEM_SELECTED_BG);
        look.icon = theme::TIMELINE_ITEM_SELECTED_ICON;
    }
    if sketch {
        look.icon = theme::TIMELINE_SKETCH_ICON;
    }
    match step {
        Step::Settled(Health::Error) => {
            look.icon = theme::TIMELINE_ERROR;
            look.line = Some(theme::TIMELINE_ERROR);
            look.badge = Some(Health::Error);
        }
        Step::Settled(Health::Warning) => {
            look.icon = theme::TIMELINE_WARNING;
            look.line = Some(theme::TIMELINE_WARNING);
            look.badge = Some(Health::Warning);
        }
        Step::Settled(Health::Suppressed) => {
            look.opacity = theme::TIMELINE_SUPPRESSED_OPACITY;
            look.strike = true;
        }
        // Not built by any model the app has read (a refused document, or one never
        // rebuilt): dimmed as a queued one is.
        Step::Settled(Health::Unknown) | Step::Queued => {
            look.opacity = theme::TIMELINE_QUEUED_OPACITY;
        }
        Step::Settled(Health::Healthy) | Step::Built => {}
        Step::Running => look.spinner = true,
    }
    look
}

/// The summary at the timeline's right (`.tl-right`): the errors, if there are any (the
/// gate's and each feature's problems), and the features.
pub fn summary(errors: u64, features: usize) -> (Option<String>, String) {
    let plural = |n: u64, one: &str, many: &str| {
        if n == 1 {
            format!("{n} {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let errors = (errors > 0).then(|| plural(errors, "error", "errors"));
    let count = u64::try_from(features).unwrap_or(u64::MAX);
    (errors, plural(count, "feature", "features"))
}

/// Where a feature dragged from place `from` would go, the pointer at `x` over items whose
/// centres are `centres`, in order: before the first whose centre is right of the pointer,
/// or at the end. `None` for where it is (before itself, or before the next).
pub fn drop_at(centres: &[f32], from: usize, x: f32) -> Option<usize> {
    let to = centres
        .iter()
        .position(|centre| x < *centre)
        .unwrap_or(centres.len());
    (to != from && to != from + 1).then_some(to)
}

/// The timeline's panel, of `tab`, the tab shown: what it asks is done to `tab` through
/// `sessions` (a selection, a dialog opened, a feature moved), `started` set if a command
/// was started; and the feature hovered given to the viewport, which draws after it.
pub(crate) fn show<P: Clients + 'static>(
    ui: &mut Ui,
    tab: Option<&mut Tab>,
    sessions: &Arc<Sessions<P>>,
    started: &mut bool,
) {
    egui::Panel::bottom("timeline")
        .exact_size(theme::TIMELINE_HEIGHT)
        .resizable(false)
        .frame(egui::Frame::NONE.fill(theme::BOTTOM_BAR_BG))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Pane);
                node.set_label(LABEL);
                node.set_bounds(crate::panes::bounds(rect));
            });
            let inner = rect.shrink2(vec2(theme::TIMELINE_PADDING, 0.0));
            title(ui, inner);
            let left = inner.left() + theme::TIMELINE_LABEL_WIDTH + theme::TIMELINE_GAP;
            let rest = Rect::from_x_y_ranges(left..=inner.right(), inner.y_range());
            let Some(tab) = tab else {
                return;
            };
            features(ui, rest, tab, sessions, started);
        });
}

/// The label at the timeline's left (`.tl-label`), in capitals.
fn title(ui: &Ui, inner: Rect) {
    let job = egui::text::LayoutJob::single_section(
        "TIMELINE".to_owned(),
        egui::TextFormat {
            font_id: egui::FontId::proportional(theme::HEADER_SIZE),
            extra_letter_spacing: theme::CAPITALS_LETTER_SPACING,
            color: theme::TIMELINE_LABEL,
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    let at = pos2(inner.left(), inner.center().y - galley.size().y / 2.0);
    ui.painter().galley(at, galley, theme::TIMELINE_LABEL);
}

/// Words in the track's place (`.tl-empty`, and what's said with no timeline yet).
fn words(ui: &mut Ui, rect: Rect, words: &str) {
    let at = Rect::from_x_y_ranges(
        rect.left() + theme::TIMELINE_EMPTY_PADDING..=rect.right(),
        rect.y_range(),
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(at)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.label(
                egui::RichText::new(words)
                    .size(theme::HEADER_SIZE)
                    .color(theme::TIMELINE_EMPTY),
            );
        },
    );
}

/// What the items asked this frame, done once they're drawn.
#[derive(Default)]
struct Asked {
    selected: Option<FeatureId>,
    edit: Option<FeatureId>,
    hovered: Option<FeatureId>,
    moved: Option<(FeatureId, Option<FeatureId>)>,
}

/// The track and the summary, in `rect`, for `tab`.
fn features<P: Clients + 'static>(
    ui: &mut Ui,
    rect: Rect,
    tab: &mut Tab,
    sessions: &Arc<Sessions<P>>,
    started: &mut bool,
) {
    let Some(timeline) = &tab.timeline else {
        words(ui, rect, panes::building(tab));
        tab.view.linked = None;
        return;
    };
    let Some(entries) = &timeline.features else {
        words(
            ui,
            rect,
            "The document doesn't read as a document: its problems are in Properties",
        );
        tab.view.linked = None;
        return;
    };
    let (track, summary_rect, said, errors) =
        summary_at(ui, rect, summary(tab.errors(), entries.len()));
    let rebuild = tab.status.as_ref().map(|status| &status.rebuild);
    let steps = step_of_each(entries, timeline.revision, rebuild);
    let mut asked = Asked::default();
    if entries.is_empty() {
        words(
            ui,
            track,
            "Features appear here, in order, as you add them.",
        );
    } else {
        ui.scope_builder(UiBuilder::new().max_rect(track), |ui| {
            // The track's items apart from the summary's button (`.tl-track`; a group, the
            // toolbar's role being the toolbar's panels').
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Group);
                node.set_label(TRACK_LABEL);
            });
            egui::ScrollArea::horizontal()
                .id_salt("timeline-track")
                .scroll_source(egui::scroll_area::ScrollSource {
                    drag: egui::scroll_area::DragScroll::Never,
                    ..Default::default()
                })
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing = vec2(theme::TIMELINE_ITEM_GAP, 0.0);
                        items(ui, tab, entries, &steps, &mut asked);
                    });
                });
        });
    }
    // After the items, so they come first in the pane.
    if let Some((button, words)) = errors
        && errors_button(ui, button, &words)
    {
        tab.whats_wrong = true;
    }
    summary_label(ui, summary_rect, &said);
    // Scrolled to once, whether its item was there or not.
    tab.timeline_scrolled();
    if steps.contains(&Step::Running) {
        // The spinner turns only while a step runs.
        ui.ctx().request_repaint();
    }
    tab.view.linked = asked.hovered;
    if let Some(id) = asked.selected {
        tab.selection_mut().select_feature(id);
    }
    if let Some((feature, before)) = asked.moved {
        tab.move_feature(sessions, feature, before);
        *started = true;
    }
    if let Some(id) = asked.edit {
        // Edit Sketch for a sketch, a feature's dialog for any other.
        tab.edit_sketch_or_feature(sessions, &id);
        *started = true;
    }
}

/// Where the summary's parts go, from the right (`.tl-right`, `gap: 2px`): the count's
/// left, `features` wide, flush with `right`; the errors' (`button.st-item`, `padding: 0
/// 8px; gap: 6px`), `errors` wide, if there are any, their text's left and their icon's;
/// and the summary's left edge, its button's padding included.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SummaryPlaces {
    pub features: f32,
    pub errors: Option<(f32, f32)>,
    pub left: f32,
}

/// The summary's places, `right` its right edge, its count `features` wide and its errors
/// `errors` wide, if there are any.
pub fn summary_places(right: f32, errors: Option<f32>, features: f32) -> SummaryPlaces {
    let count = right - features;
    let Some(errors) = errors else {
        return SummaryPlaces {
            features: count,
            errors: None,
            left: count,
        };
    };
    let text = count - theme::TIMELINE_SUMMARY_SPACING - theme::TIMELINE_SUMMARY_PADDING - errors;
    let icon = text - theme::TIMELINE_SUMMARY_GAP - crate::icons::ICON_SIZE;
    SummaryPlaces {
        features: count,
        errors: Some((text, icon)),
        left: icon - theme::TIMELINE_SUMMARY_PADDING,
    }
}

/// The summary drawn at `rect`'s right (`.tl-right`): its errors in the error's red with the
/// error's icon, a button (`button.st-item`, Gray.300 under it and white while the pointer
/// is over it) that opens What's Wrong, then its features: the rect left for the track, the
/// summary's, what it says, for its label in the accessibility tree ([`summary_label`]),
/// added after the items so it's last in the pane, and the errors' button's place and
/// words, if there are errors ([`errors_button`]).
fn summary_at(
    ui: &mut Ui,
    rect: Rect,
    (errors, features): (Option<String>, String),
) -> (Rect, Rect, String, Option<(Rect, String)>) {
    let font = egui::FontId::proportional(theme::HEADER_SIZE);
    let painter = ui.painter().clone();
    let centre = rect.center().y;
    let count = painter.layout_no_wrap(features.clone(), font.clone(), theme::TIMELINE_SUMMARY);
    let wrong = errors
        .as_ref()
        .map(|errors| painter.layout_no_wrap(errors.clone(), font, theme::TIMELINE_SUMMARY_ERRORS));
    let places = summary_places(
        rect.right(),
        wrong.as_ref().map(|galley| galley.size().x),
        count.size().x,
    );
    let mut spans = vec![(places.features, count, theme::TIMELINE_SUMMARY)];
    let mut button = None;
    if let (Some(galley), Some((text, icon))) = (wrong, places.errors) {
        let side = crate::icons::ICON_SIZE;
        let place = Rect::from_x_y_ranges(
            places.left..=text + galley.size().x + theme::TIMELINE_SUMMARY_PADDING,
            centre - theme::STATUS_ITEM_HEIGHT / 2.0..=centre + theme::STATUS_ITEM_HEIGHT / 2.0,
        );
        let ink = if ui.rect_contains_pointer(place) {
            painter.rect_filled(
                place,
                theme::SMALL_CORNER_RADIUS,
                theme::STATUS_ITEM_HOVER_BG,
            );
            theme::STATUS_ITEM_HOVER_TEXT
        } else {
            theme::TIMELINE_SUMMARY_ERRORS
        };
        Icon::Error.image(IconSize::Small).tint(ink).paint_at(
            ui,
            Rect::from_min_size(pos2(icon, centre - side / 2.0), Vec2::splat(side)),
        );
        button = Some((place, galley.text().to_owned()));
        spans.push((text, galley, ink));
    }
    let mut whole = Rect::NOTHING;
    for (x, galley, colour) in spans {
        let at = pos2(x, centre - galley.size().y / 2.0);
        whole = whole.union(Rect::from_min_size(at, galley.size()));
        // In the colour given, the errors' white while hovered.
        painter.galley_with_override_text_color(at, galley, colour);
    }
    let said = match errors {
        Some(errors) => format!("{errors} · {features}"),
        None => features,
    };
    let track = Rect::from_x_y_ranges(
        rect.left()..=(places.left - theme::TIMELINE_GAP).max(rect.left()),
        rect.y_range(),
    );
    (track, whole, said, button)
}

/// The summary's errors at `place`, saying `words`, as a button for the accessibility tree
/// and the pointer, with What's wrong's tooltip from the registry: whether it was clicked.
fn errors_button(ui: &Ui, place: Rect, words: &str) -> bool {
    let response = ui.interact(place, egui::Id::new("timeline-errors"), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, words));
    theme::focus_ring(
        ui,
        &response,
        place,
        theme::SMALL_CORNER_RADIUS,
        egui::StrokeKind::Outside,
    );
    let command = crate::shortcuts::Command::WhatsWrong;
    crate::bar::tooltip(response, command.label(), None, command.entry().description).clicked()
}

/// The summary's label for the accessibility tree, at `rect`, saying `said`.
fn summary_label(ui: &Ui, rect: Rect, said: &str) {
    let response = ui.interact(rect, egui::Id::new("timeline-summary"), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, said));
}

/// Each feature's item, in order, and a drag's drop line.
fn items(ui: &mut Ui, tab: &Tab, entries: &[Entry], steps: &[Step], asked: &mut Asked) {
    let mut rects = Vec::with_capacity(entries.len());
    let mut dragged = None;
    let mut dropped = None;
    for (at, (entry, step)) in entries.iter().zip(steps).enumerate() {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::splat(theme::TIMELINE_ITEM_SIZE), Sense::hover());
        rects.push(rect);
        let id = egui::Id::new(("timeline-item", entry.id.as_str()));
        let response = ui.interact(rect, id, Sense::click_and_drag());
        let selected = tab.selection().has_feature(&entry.id);
        let description = format!("{} · {}", kind_words(entry.kind), step.words());
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, &entry.name)
        });
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_description(description.clone());
        });
        let hovered = response.hovered();
        if hovered {
            asked.hovered = Some(entry.id.clone());
        }
        if tab.timeline_scroll() == Some(&entry.id) {
            // An error card's Show in timeline: its item brought into the track's view.
            ui.scroll_to_rect(rect, Some(Align::Center));
        }
        let sketch = entry.kind == "sketch";
        let look = look(*step, sketch, hovered, selected);
        paint(ui, rect, Icon::of_kind(entry.kind), &look);
        theme::focus_ring(
            ui,
            &response,
            rect,
            theme::TIMELINE_ITEM_CORNER_RADIUS,
            egui::StrokeKind::Inside,
        );
        if let Some(health) = look.badge {
            let said = tab.said.get(entry.id.as_str());
            let corner = pos2(
                rect.right() - theme::TIMELINE_BADGE_INSET - theme::TIMELINE_BADGE_SIZE,
                rect.top() + theme::TIMELINE_BADGE_INSET,
            );
            panes::health_mark(
                ui,
                id.with("mark"),
                Rect::from_min_size(corner, Vec2::splat(theme::TIMELINE_BADGE_SIZE)),
                health,
                said.map(|said| said.message.as_str()),
                MarkLook::Badge,
            );
        } else if let Some(text) = tab.outside.get(&entry.id) {
            // Changed by another program, with no health mark (`.badge.outside`).
            let side = theme::TIMELINE_BADGE_OUTSIDE_SIZE;
            let corner = pos2(
                rect.right() - theme::TIMELINE_BADGE_INSET - side,
                rect.top() + theme::TIMELINE_BADGE_INSET,
            );
            panes::outside_mark(
                ui,
                id.with("outside"),
                Rect::from_min_size(corner, Vec2::splat(side)),
                text,
                Some(theme::TIMELINE_BADGE_RING),
            );
        }
        if response.dragged() {
            dragged = Some(at);
        }
        if response.drag_stopped() {
            dropped = Some(at);
        }
        // A sketch's is Edit Sketch (fusion.md, *Sketch mode*, [esk]).
        let (word, editable) = match entry.kind {
            "sketch" => ("Edit Sketch", tab.can_sketch()),
            kind => ("Edit Feature", crate::dialog::Kind::of(kind).is_some()),
        };
        if response.clicked() {
            asked.selected = Some(entry.id.clone());
        }
        if response.double_clicked() {
            asked.edit = Some(entry.id.clone());
        }
        response.context_menu(|ui| {
            if ui.add_enabled(editable, egui::Button::new(word)).clicked() {
                asked.edit = Some(entry.id.clone());
                ui.close();
            }
        });
        let said = tab.said.get(entry.id.as_str());
        let mut tip = description;
        if let Some(said) = said {
            tip = format!("{tip} — {}", said.message);
        }
        if let Some(text) = tab.outside.get(&entry.id) {
            tip = format!("{tip} — changed on disk: {text}");
        }
        if editable {
            tip.push_str(". Double-click to edit.");
        }
        crate::bar::tooltip(response, &entry.name, None, Some(&tip));
    }
    let centres: Vec<f32> = rects.iter().map(|rect| rect.center().x).collect();
    let pointer = ui.ctx().pointer_latest_pos().map(|at| at.x);
    if let (Some(from), Some(x)) = (dragged.or(dropped), pointer)
        && let Some(to) = drop_at(&centres, from, x)
    {
        if dropped.is_some() {
            // SEAM: nothing is reordered here. The dropped item shows where it was until
            // `move_feature` answers and the timeline is read again of the revision it made
            // (the document's order, read beside the rebuild it starts, not after it); a
            // move queued behind other commands waits for them first.
            let before = entries.get(to).map(|entry| entry.id.clone());
            asked.moved = Some((entries[from].id.clone(), before));
        } else {
            drop_line(ui, &rects, to);
        }
    }
}

/// The line where a dragged feature would drop, before the item at `to` (`.drop-before`),
/// or after the last.
fn drop_line(ui: &Ui, rects: &[Rect], to: usize) {
    let x = match rects.get(to) {
        Some(rect) => rect.left() - theme::TIMELINE_DROP_OFFSET,
        None => match rects.last() {
            Some(rect) => rect.right(),
            None => return,
        },
    };
    let Some(item) = rects.get(to).or(rects.last()) else {
        return;
    };
    let line = Rect::from_x_y_ranges(
        x..=x + theme::TIMELINE_DROP_WIDTH,
        item.top() + theme::TIMELINE_DROP_INSET..=item.bottom() - theme::TIMELINE_DROP_INSET,
    );
    ui.painter().rect_filled(line, 0.0, theme::TIMELINE_DROP);
}

/// An item's fill, its bottom line, its icon, its strike, and its spinner, at `rect`.
fn paint(ui: &Ui, rect: Rect, icon: Icon, look: &Look) {
    let fade = |colour: Color32| colour.gamma_multiply(look.opacity);
    let painter = ui.painter();
    let radius = theme::TIMELINE_ITEM_CORNER_RADIUS;
    if let Some(fill) = look.fill {
        painter.rect_filled(rect, radius, fade(fill));
    }
    if let Some(line) = look.line {
        // `box-shadow: inset 0 -2px 0`: along the bottom, inside the item's corners.
        let band = Rect::from_x_y_ranges(
            rect.x_range(),
            rect.bottom() - theme::TIMELINE_MARK_LINE_WIDTH..=rect.bottom(),
        );
        let corners = CornerRadius {
            nw: 0,
            ne: 0,
            sw: radius,
            se: radius,
        };
        painter.rect_filled(band, corners, fade(line));
    }
    let side = IconSize::Toolbar.points();
    icon.image(IconSize::Toolbar)
        .tint(fade(look.icon))
        .paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(side)));
    if look.strike {
        // `::after`: a bar `5px` in from each side, turned `-35deg` about the centre.
        let half = rect.width() / 2.0 - theme::TIMELINE_STRIKE_INSET;
        let turn = theme::TIMELINE_STRIKE_DEGREES.to_radians();
        let along = vec2(libm::cosf(turn), libm::sinf(turn)) * half;
        painter.line_segment(
            [rect.center() - along, rect.center() + along],
            Stroke::new(theme::TIMELINE_STRIKE_WIDTH, fade(look.icon)),
        );
    }
    if look.spinner {
        spinner(ui, rect);
    }
}

/// The running step's spinner (`.building::before`): a ring `inset` in from the item, its
/// top and right borders drawn (half of it, from the upper left to the lower right), turning
/// clockwise once every [`theme::TIMELINE_SPINNER_TURN_SECONDS`].
fn spinner(ui: &Ui, rect: Rect) {
    let radius =
        rect.width() / 2.0 - theme::TIMELINE_SPINNER_INSET - theme::TIMELINE_SPINNER_WIDTH / 2.0;
    let time = ui.input(|input| input.time);
    let turns = (time / f64::from(theme::TIMELINE_SPINNER_TURN_SECONDS)).fract() as f32;
    // On screen, y down: an angle grows clockwise. The top border starts at the upper left,
    // 225 degrees round, and the right one ends at the lower right, 45 degrees round.
    let start = (-135.0f32).to_radians() + turns * std::f32::consts::TAU;
    let points: Vec<_> = (0..=32)
        .map(|n| {
            let angle = start + std::f32::consts::PI * n as f32 / 32.0;
            rect.center() + vec2(libm::cosf(angle), libm::sinf(angle)) * radius
        })
        .collect();
    ui.painter().add(Shape::line(
        points,
        Stroke::new(theme::TIMELINE_SPINNER_WIDTH, theme::TIMELINE_SPINNER),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use silico_core::command::{FeatureStates, Progress};

    fn entry(id: &str, kind: &'static str, inputs: &[&str], health: Health) -> Entry {
        Entry {
            id: FeatureId::new(id).unwrap(),
            kind,
            name: id.to_owned(),
            suppressed: health == Health::Suppressed,
            inputs: inputs.iter().map(|i| FeatureId::new(i).unwrap()).collect(),
            health,
            problems: u64::from(health == Health::Error),
            warnings: u64::from(health == Health::Warning),
        }
    }

    fn rebuild(state: RebuildState, running: Option<&str>) -> Rebuild {
        Rebuild {
            revision: Revision::OPENED,
            state,
            progress: Some(Progress {
                done: 0,
                steps: 0,
                running: running.map(str::to_owned),
                features: FeatureStates::default(),
            }),
        }
    }

    const LISTED: Revision = Revision::OPENED;

    /// A plate, a sketch on its own, a pin off to the side, a pad of the sketch on the
    /// plate, a cut of the pad by the pin, and a suppressed fillet of the cut. The pin is
    /// before the pad but off its lineage: a pad running doesn't make it built.
    fn part() -> Vec<Entry> {
        vec![
            entry("b0000001", "box", &[], Health::Healthy),
            entry("s0000001", "sketch", &[], Health::Healthy),
            entry("c0000001", "cylinder", &[], Health::Warning),
            entry(
                "p0000001",
                "pad",
                &["b0000001", "s0000001"],
                Health::Healthy,
            ),
            entry("m0000001", "cut", &["p0000001", "c0000001"], Health::Error),
            entry("f0000001", "fillet", &["m0000001"], Health::Suppressed),
        ]
    }

    /// `part`'s steps with `rebuild` of the revision it lists.
    fn steps(rebuild: Option<&Rebuild>) -> Vec<Step> {
        step_of_each(&part(), LISTED, rebuild)
    }

    #[test]
    fn with_no_rebuild_running_each_item_is_its_health() {
        use Health::*;
        let want = [Healthy, Healthy, Warning, Healthy, Error, Suppressed].map(Step::Settled);
        for state in [
            RebuildState::Idle,
            RebuildState::Done,
            RebuildState::Stopped,
            RebuildState::Faulted,
        ] {
            assert_eq!(
                steps(Some(&rebuild(state, Some("p0000001 pad")))),
                want,
                "{state:?}"
            );
        }
        assert_eq!(steps(None), want);
    }

    #[test]
    fn the_running_step_spins_its_inputs_are_built_and_the_rest_are_queued() {
        use Step::*;
        let suppressed = Settled(Health::Suppressed);
        // The pad running: the plate and the sketch are its inputs, so built; the pin, before
        // it but off its lineage, and the cut after it are queued; the fillet stays
        // suppressed.
        assert_eq!(
            steps(Some(&rebuild(RebuildState::Running, Some("p0000001 pad")))),
            [Built, Built, Queued, Running, Queued, suppressed]
        );
        // The cut running: every input back to the plate built, the pin too.
        assert_eq!(
            steps(Some(&rebuild(RebuildState::Running, Some("m0000001 cut")))),
            [Built, Built, Built, Built, Running, suppressed]
        );
        // Nothing running yet (its sketches solving, before its plan), or a feature the
        // timeline doesn't list (a newer revision's): every unsuppressed one queued.
        for running in [None, Some("z0000009 box"), Some("")] {
            assert_eq!(
                steps(Some(&rebuild(RebuildState::Running, running))),
                [Queued, Queued, Queued, Queued, Queued, suppressed],
                "{running:?}"
            );
        }
        let no_progress = Rebuild {
            progress: None,
            ..rebuild(RebuildState::Running, None)
        };
        assert_eq!(
            steps(Some(&no_progress)),
            [Queued, Queued, Queued, Queued, Queued, suppressed]
        );
    }

    #[test]
    fn a_rebuild_of_another_revision_shows_nothing_built() {
        use Step::*;
        // The timeline lists revision 1; the rebuild runs revision 2 (a move answered, its
        // timeline not read yet), whose inputs may not be these: the cut runs, every other
        // feature queued, the fillet too (revision 2 may have unsuppressed it).
        let newer = Rebuild {
            revision: LISTED.next().unwrap(),
            ..rebuild(RebuildState::Running, Some("m0000001 cut"))
        };
        assert_eq!(
            steps(Some(&newer)),
            [Queued, Queued, Queued, Queued, Running, Queued]
        );
        // And the other way: a rebuild of an older revision than the one listed.
        let older = Rebuild {
            revision: LISTED,
            ..rebuild(RebuildState::Running, Some("m0000001 cut"))
        };
        assert_eq!(
            step_of_each(&part(), LISTED.next().unwrap(), Some(&older)),
            [Queued, Queued, Queued, Queued, Running, Queued]
        );
        // Ended, each item is its health whichever revision it was.
        let done = Rebuild {
            state: RebuildState::Done,
            ..newer
        };
        assert_eq!(
            steps(Some(&done)),
            [
                Health::Healthy,
                Health::Healthy,
                Health::Warning,
                Health::Healthy,
                Health::Error,
                Health::Suppressed
            ]
            .map(Settled)
        );
    }

    #[test]
    fn an_input_cycle_or_an_unknown_input_ends_the_walk() {
        // Never in a checked document, but the walk mustn't loop on one.
        let entries = [
            entry(
                "a0000001",
                "box",
                &["b0000001", "x0000009"],
                Health::Healthy,
            ),
            entry("b0000001", "box", &["a0000001"], Health::Healthy),
        ];
        assert_eq!(
            step_of_each(
                &entries,
                LISTED,
                Some(&rebuild(RebuildState::Running, Some("a0000001 box")))
            ),
            [Step::Running, Step::Built]
        );
    }

    fn rgb(hex: u32) -> Color32 {
        let [_, r, g, b] = hex.to_be_bytes();
        Color32::from_rgb(r, g, b)
    }

    #[test]
    fn an_items_look_follows_styles_css() {
        let healthy = Step::Settled(Health::Healthy);
        // `.tl-item`: `--g750`, no fill.
        let rest = look(healthy, false, false, false);
        assert_eq!((rest.fill, rest.icon), (None, rgb(0xcfcfcf)));
        assert_eq!((rest.line, rest.opacity, rest.badge), (None, 1.0, None));
        // `:hover`: `--g1000` on `--g300`.
        let hover = look(healthy, false, true, false);
        assert_eq!(
            (hover.fill, hover.icon),
            (Some(rgb(0x2c2b2b)), rgb(0xffffff))
        );
        // `.selected`, after `:hover`: `--blue-text` on `--blue`, hovered or not.
        for hovered in [false, true] {
            let chosen = look(healthy, false, hovered, true);
            assert_eq!(
                (chosen.fill, chosen.icon),
                (Some(rgb(0x186add)), rgb(0xf0f2ff))
            );
        }
        // `.sketch`, after `.selected`: `--g600`, on the blue when selected.
        let sketch = look(healthy, true, false, true);
        assert_eq!(
            (sketch.fill, sketch.icon),
            (Some(rgb(0x186add)), rgb(0x9f9f9f))
        );
        // `.err`: `--red`, its line `--red`, its badge; selected too, on the blue.
        let failed = look(Step::Settled(Health::Error), true, false, true);
        assert_eq!(failed.icon, rgb(0xff3865));
        assert_eq!(failed.line, Some(rgb(0xff3865)));
        assert_eq!(failed.fill, Some(rgb(0x186add)));
        assert_eq!(failed.badge, Some(Health::Error));
        // `.warn`: `--orange`, its line and badge.
        let warned = look(Step::Settled(Health::Warning), false, false, false);
        assert_eq!(
            (warned.icon, warned.line, warned.badge),
            (rgb(0xff7a0c), Some(rgb(0xff7a0c)), Some(Health::Warning))
        );
        // `.suppressed`: `.35`, struck; `.queued`: `.45`; `.building`: the spinner.
        let suppressed = look(Step::Settled(Health::Suppressed), false, false, false);
        assert_eq!((suppressed.opacity, suppressed.strike), (0.35, true));
        assert_eq!(look(Step::Queued, false, false, false).opacity, 0.45);
        let running = look(Step::Running, false, false, false);
        assert!(running.spinner);
        assert_eq!((running.opacity, running.badge), (1.0, None));
        // Built in the running rebuild: as at rest, no mark until it ends.
        assert_eq!(look(Step::Built, false, false, false), rest);
    }

    #[test]
    fn the_summary_counts_errors_and_features() {
        assert_eq!(summary(0, 7), (None, "7 features".to_owned()));
        assert_eq!(
            summary(1, 7),
            (Some("1 error".to_owned()), "7 features".to_owned())
        );
        assert_eq!(
            summary(3, 1),
            (Some("3 errors".to_owned()), "1 feature".to_owned())
        );
        assert_eq!(summary(0, 0), (None, "0 features".to_owned()));
    }

    #[test]
    fn a_drop_goes_before_the_first_item_whose_centre_is_right_of_the_pointer() {
        // Four items 32 apart, centred at 15, 47, 79, 111.
        let centres = [15.0, 47.0, 79.0, 111.0];
        // The second dragged: left of the first's centre, before it; past the last's, to
        // the end; over itself or the next's left half, where it is.
        assert_eq!(drop_at(&centres, 1, 0.0), Some(0));
        assert_eq!(drop_at(&centres, 1, 14.9), Some(0));
        assert_eq!(drop_at(&centres, 1, 15.0), None);
        assert_eq!(drop_at(&centres, 1, 60.0), None);
        assert_eq!(drop_at(&centres, 1, 80.0), Some(3));
        assert_eq!(drop_at(&centres, 1, 500.0), Some(4));
        // The last dragged past the end: where it is.
        assert_eq!(drop_at(&centres, 3, 500.0), None);
        assert_eq!(drop_at(&centres, 3, 0.0), Some(0));
    }
    #[test]
    fn the_summarys_errors_are_10_from_its_count_and_its_icon_6_from_its_errors() {
        // `.tl-right { gap: 2px }`, `button.st-item { padding: 0 8px; gap: 6px }`: the count
        // flush right; the errors' text 8 + 2 left of it; the icon (14) 6 left of that;
        // the button's left 8 further.
        let places = summary_places(500.0, Some(40.0), 60.0);
        assert_eq!(places.features, 440.0);
        assert_eq!(places.errors, Some((390.0, 370.0)));
        assert_eq!(places.left, 362.0);
        let (text, _) = places.errors.unwrap();
        assert_eq!(places.features - (text + 40.0), 10.0);
        assert_eq!(
            summary_places(500.0, None, 60.0),
            SummaryPlaces {
                features: 440.0,
                errors: None,
                left: 440.0
            }
        );
    }

    #[test]
    fn a_features_type_is_said_as_the_prototype_says_it() {
        // `FEATURE_KIND` in js/app.js; the toolbar's names for the rest.
        for (kind, words) in [
            ("sketch", "Sketch"),
            ("pad", "Pad"),
            ("pocket", "Pocket"),
            ("fillet", "Fillet"),
            ("chamfer", "Chamfer"),
            ("revolve", "Revolve"),
            ("groove", "Groove"),
            ("box", "Box"),
            ("cylinder", "Cylinder"),
            ("sphere", "Sphere"),
            ("cone", "Cone"),
            ("torus", "Torus"),
            ("fuse", "Union"),
            ("cut", "Subtract"),
            ("common", "Intersect"),
            ("helix", "helix"),
        ] {
            assert_eq!(kind_words(kind), words, "{kind}");
        }
    }

    #[test]
    fn a_feature_marked_suppressed_stays_suppressed_whatever_its_health_says() {
        // The timeline marks a feature `suppressed` from the document, its health from the
        // model read: either one says it's never rebuilt.
        let flagged = Entry {
            suppressed: true,
            ..entry("a0000001", "box", &[], Health::Healthy)
        };
        let marked = Entry {
            suppressed: false,
            ..entry("b0000001", "box", &[], Health::Suppressed)
        };
        let running = rebuild(RebuildState::Running, Some("c0000001 box"));
        assert_eq!(
            step_of_each(
                &[
                    flagged,
                    marked,
                    entry("c0000001", "box", &[], Health::Healthy)
                ],
                LISTED,
                Some(&running)
            ),
            [
                Step::Settled(Health::Suppressed),
                Step::Settled(Health::Suppressed),
                Step::Running
            ]
        );
    }

    // ---- The timeline as drawn, against the prototype ----
    //
    // Each value below is typed from the approved prototype's stylesheet
    // (docs/design/m5-ui/styles.css: `:root`'s tokens, `#timeline`, `.tl-*`, `.st-item`,
    // `svg.i`), never from theme.rs, and compared with what the pane painted: each
    // rectangle, circle, line, icon, and text from egui's shapes.

    use eframe::egui::epaint::{CircleShape, ClippedShape, PathShape, RectShape, TextShape};
    use eframe::egui::{Event, FontId, Modifiers, PointerButton, Pos2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use silico_core::command::{Problems, SessionId, Status, TimelineAnswer};
    use silico_core::fake::FakeKernel;

    /// The prototype's tokens and geometry.
    mod css {
        use eframe::egui::Color32;

        // `:root`'s greys and colours.
        pub const G150: Color32 = Color32::from_rgb(0x17, 0x17, 0x17);
        pub const G250: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
        pub const G300: Color32 = Color32::from_rgb(0x2c, 0x2b, 0x2b);
        pub const G550: Color32 = Color32::from_rgb(0x93, 0x90, 0x90);
        pub const G600: Color32 = Color32::from_rgb(0x9f, 0x9f, 0x9f);
        pub const G750: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);
        pub const G1000: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
        pub const BLUE: Color32 = Color32::from_rgb(0x18, 0x6a, 0xdd);
        pub const BLUE_HI: Color32 = Color32::from_rgb(0x46, 0x76, 0xff);
        pub const RED: Color32 = Color32::from_rgb(0xff, 0x38, 0x65);
        pub const RED_FILL: Color32 = Color32::from_rgb(0xab, 0x01, 0x16);
        pub const ORANGE: Color32 = Color32::from_rgb(0xff, 0x7a, 0x0c);
        /// `--g750` at `.tl-item.queued`'s `opacity: .45` (207 × .45 = 93.15, 255 × .45 =
        /// 114.75) and `.suppressed`'s `.35` (72.45, 89.25), premultiplied as egui paints.
        pub const G750_QUEUED: Color32 = Color32::from_rgba_premultiplied(93, 93, 93, 115);
        pub const G750_SUPPRESSED: Color32 = Color32::from_rgba_premultiplied(72, 72, 72, 89);

        /// `--fs-small`, `--icon`, and `svg.i.lg`.
        pub const FS_SMALL: f32 = 11.0;
        pub const ICON: f32 = 14.0;
        pub const ICON_LG: f32 = 20.0;
        /// `#timeline`: `height: var(--timeline-h)` with its `border-top: 1px` inside it
        /// (`* { box-sizing: border-box }`), `padding: 0 8px`, `gap: 8px`.
        pub const TIMELINE_H: f32 = 40.0;
        pub const BORDER: f32 = 1.0;
        pub const PADDING: f32 = 8.0;
        pub const GAP: f32 = 8.0;
        /// `.tl-label`: `width: 68px`, `letter-spacing: .5px`.
        pub const LABEL_W: f32 = 68.0;
        pub const CAPITALS_SPACING: f32 = 0.5;
        /// `.tl-track`'s `gap: 2px`; `.tl-item`: 30 square, `border-radius: var(--r-sm)`.
        pub const TRACK_GAP: f32 = 2.0;
        pub const ITEM: f32 = 30.0;
        pub const R_SM: u8 = 4;
        /// `.err`, `.warn`: `box-shadow: inset 0 -2px 0`.
        pub const LINE: f32 = 2.0;
        /// `.suppressed::after`: `left: 5px; right: 5px; height: 1.5px; rotate(-35deg)`.
        pub const STRIKE_INSET: f32 = 5.0;
        pub const STRIKE_W: f32 = 1.5;
        /// cos 35° and sin 35°.
        pub const COS_35: f32 = 0.819_152;
        pub const SIN_35: f32 = 0.573_576;
        /// `.building::before`: `inset: 2px; border: 2px`, `spin .7s`.
        pub const SPIN_INSET: f32 = 2.0;
        pub const SPIN_BORDER: f32 = 2.0;
        pub const SPIN_SECONDS: f64 = 0.7;
        /// `.badge`: `top: 1px; right: 1px; width: 9px; height: 9px`, `box-shadow: 0 0 0
        /// 1.5px var(--g150)`.
        pub const BADGE_INSET: f32 = 1.0;
        pub const BADGE: f32 = 9.0;
        pub const BADGE_RING: f32 = 1.5;
        /// `.drop-before::before`: `left: -2px; top: 4px; bottom: 4px; width: 2px`.
        pub const DROP_LEFT: f32 = -2.0;
        pub const DROP_INSET: f32 = 4.0;
        pub const DROP_W: f32 = 2.0;
        /// `.tl-empty`: `padding-left: 4px`.
        pub const EMPTY_PADDING: f32 = 4.0;
        /// `.tl-right`: `gap: 2px`; `.st-item`: `padding: 0 8px; gap: 6px`.
        pub const RIGHT_GAP: f32 = 2.0;
        pub const ST_PADDING: f32 = 8.0;
        pub const ST_GAP: f32 = 6.0;
        /// `.st-item { height: 20px }`, centred in `.tl-right` (`align-items: center`).
        pub const ST_H: f32 = 20.0;
    }

    /// Where the timeline's scope starts in the harness's window (away from the origin, so a
    /// sum with it never reads as a product), and its size: the panel is its bottom.
    const PANE: Pos2 = Pos2::new(40.0, 30.0);
    const W: f32 = 800.0;
    const HIGH: f32 = 100.0;

    type Fake = Arc<FakeKernel>;

    struct Shown {
        tab: Option<Tab>,
        sessions: Arc<Sessions<Fake>>,
        started: bool,
    }

    /// `#timeline`'s box; below, its content's middle and its track's left edge, as the
    /// prototype lays them out.
    fn panel() -> Rect {
        Rect::from_min_size(
            PANE + Vec2::new(0.0, HIGH - css::TIMELINE_H),
            Vec2::new(W, css::TIMELINE_H),
        )
    }

    fn middle() -> f32 {
        panel().top() + css::BORDER + (css::TIMELINE_H - css::BORDER) / 2.0
    }

    fn track_left() -> f32 {
        panel().left() + css::PADDING + css::LABEL_W + css::GAP
    }

    /// The `at`th item's box on the track.
    fn item(at: usize) -> Rect {
        Rect::from_min_size(
            Pos2::new(
                track_left() + at as f32 * (css::ITEM + css::TRACK_GAP),
                middle() - css::ITEM / 2.0,
            ),
            Vec2::splat(css::ITEM),
        )
    }

    /// The tab of a document whose timeline lists `features`, with `problems` of its own.
    fn tab_of(features: Option<&[Entry]>, problems: u64) -> Tab {
        let none = Problems::from(silico_core::Error::new(
            silico_core::ErrorCode::Internal,
            "unused",
        ));
        let mut tab = Tab::opened(
            SessionId::parse("s1").unwrap(),
            "/models/Part.silico".to_owned(),
            &none,
        );
        tab.timeline = Some(TimelineAnswer {
            revision: LISTED,
            model: Some(LISTED),
            problems,
            features: features.map(<[Entry]>::to_vec),
            bodies: Vec::new(),
        });
        tab
    }

    /// `tab`'s session running `rebuild`.
    fn running(tab: &mut Tab, rebuild: Rebuild) {
        tab.status = Some(Status {
            features: None,
            gated: true,
            transaction: None,
            running: None,
            reads: Vec::new(),
            rebuild,
            drag: None,
            disk: None,
        });
    }

    fn drawn(tab: Tab) -> Harness<'static, Shown> {
        let shown = Shown {
            tab: Some(tab),
            sessions: Arc::new(Sessions::with_cache_opener(
                Arc::new(FakeKernel::new(1).unwrap()),
                |dir| silico_core::cache::Cache::unsealed(dir),
            )),
            started: false,
        };
        let mut h = Harness::builder()
            .with_size(Vec2::new(PANE.x + W + 40.0, PANE.y + HIGH + 40.0))
            .build_ui_state(
                |ui, shown: &mut Shown| {
                    let scope = Rect::from_min_size(PANE, Vec2::new(W, HIGH));
                    ui.scope_builder(UiBuilder::new().max_rect(scope), |ui| {
                        show(ui, shown.tab.as_mut(), &shown.sessions, &mut shown.started);
                    });
                },
                shown,
            );
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    /// Every shape the last frame painted, nested ones flattened, with its clip.
    fn clipped<S>(h: &Harness<'_, S>) -> Vec<ClippedShape> {
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
        for shape in &h.output().shapes {
            flatten(shape.clip_rect, &shape.shape, &mut out);
        }
        out
    }

    fn painted<S>(h: &Harness<'_, S>) -> Vec<Shape> {
        clipped(h).into_iter().map(|shape| shape.shape).collect()
    }

    fn rects<S>(h: &Harness<'_, S>) -> Vec<RectShape> {
        painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Rect(rect) => Some(rect),
                _ => None,
            })
            .collect()
    }

    fn circles<S>(h: &Harness<'_, S>, within: Rect) -> Vec<CircleShape> {
        painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Circle(circle) if within.contains(circle.center) => Some(circle),
                _ => None,
            })
            .collect()
    }

    /// The one text `text` painted.
    #[track_caller]
    fn text<S>(h: &Harness<'_, S>, text: &str) -> TextShape {
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

    /// A text's colour as painted: its override, else its first section's, else the shape's
    /// own for a placeholder.
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

    /// The tint of the icon painted at `rect` (a rectangle filled with its texture), to half
    /// a point: egui puts an image on the pixel grid.
    fn icon_at<S>(h: &Harness<'_, S>, rect: Rect) -> Option<Color32> {
        rects(h)
            .into_iter()
            .find(|shape| {
                shape.brush.is_some()
                    && (shape.rect.min - rect.min).abs().max_elem() <= 0.5
                    && (shape.rect.max - rect.max).abs().max_elem() <= 0.5
            })
            .map(|shape| shape.fill)
    }

    /// The fills painted over the whole of `rect`, but transparent ones, and their corners.
    fn fills_at<S>(h: &Harness<'_, S>, rect: Rect) -> Vec<(Color32, CornerRadius)> {
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

    /// Within egui's rounding of a widget's place, a 32nd of a point.
    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() <= egui::emath::GUI_ROUNDING
    }

    fn near_pos(a: Pos2, b: Pos2) -> bool {
        near(a.x, b.x) && near(a.y, b.y)
    }

    fn near_rect(a: Rect, b: Rect) -> bool {
        near_pos(a.min, b.min) && near_pos(a.max, b.max)
    }

    #[track_caller]
    fn at(got: Pos2, want: Pos2, what: &str) {
        assert!(near_pos(got, want), "{what}: {got:?}, not {want:?}");
    }

    /// The text `words` drawn from `left`, its line's middle the content's, `--fs-small`,
    /// in `ink`.
    #[track_caller]
    fn said_at<S>(h: &Harness<'_, S>, words: &str, left: f32, ink: Color32) -> Rect {
        let shown = text(h, words);
        let size = shown.galley.size();
        at(shown.pos, Pos2::new(left, middle() - size.y / 2.0), words);
        assert_eq!(
            shown.galley.job.sections[0].format.font_id,
            FontId::proportional(css::FS_SMALL),
            "{words}"
        );
        assert_eq!(colour(&shown), ink, "{words}'s ink");
        Rect::from_min_size(shown.pos, size)
    }

    #[test]
    fn the_label_and_an_empty_track_are_drawn_as_tl_label_and_tl_empty() {
        let h = drawn(tab_of(Some(&[]), 0));
        // `#timeline`: `--g150` under its `border-top: 1px solid var(--g250)`.
        let body = Rect::from_min_max(
            Pos2::new(panel().left(), panel().top() + css::BORDER),
            panel().max,
        );
        assert_eq!(fills_at(&h, body), [(css::G150, CornerRadius::ZERO)]);
        let border: Vec<([Pos2; 2], Stroke)> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::LineSegment { points, stroke } => Some((points, stroke)),
                _ => None,
            })
            .collect();
        let y = panel().top() + css::BORDER / 2.0;
        assert_eq!(
            border,
            [(
                [Pos2::new(panel().left(), y), Pos2::new(panel().right(), y)],
                Stroke::new(css::BORDER, css::G250)
            )]
        );
        // `.tl-label`: `--pad` in, its line's middle the content's, 11 points in capitals .5
        // apart, `--g550`.
        let label = text(&h, "TIMELINE");
        let size = label.galley.size();
        at(
            label.pos,
            Pos2::new(panel().left() + css::PADDING, middle() - size.y / 2.0),
            "the label",
        );
        let format = &label.galley.job.sections[0].format;
        assert_eq!(format.font_id, FontId::proportional(css::FS_SMALL));
        assert_eq!(format.extra_letter_spacing, css::CAPITALS_SPACING);
        assert_eq!(colour(&label), css::G550);
        // `.tl-empty`: on the track, 4 in.
        said_at(
            &h,
            "Features appear here, in order, as you add them.",
            track_left() + css::EMPTY_PADDING,
            css::G550,
        );
    }

    /// A block, a sketch, a pad of the sketch on the block (failed), a cylinder (warned),
    /// and a fillet of the pad (suppressed).
    fn features() -> Vec<Entry> {
        vec![
            entry("b0000001", "box", &[], Health::Healthy),
            entry("s0000001", "sketch", &[], Health::Healthy),
            entry("p0000001", "pad", &["b0000001", "s0000001"], Health::Error),
            entry("c0000001", "cylinder", &[], Health::Warning),
            entry("f0000001", "fillet", &["p0000001"], Health::Suppressed),
        ]
    }

    /// The icon's place in an item: `svg.i.lg`, centred (`place-items: center`).
    fn icon_of(item: Rect) -> Rect {
        Rect::from_center_size(item.center(), Vec2::splat(css::ICON_LG))
    }

    /// The badge at `item`'s corner: its ring `--g150` 1.5 outside it, then its dot, `fill`.
    #[track_caller]
    fn badge(h: &Harness<'_, Shown>, item: Rect, fill: Color32, what: &str) {
        let centre = Pos2::new(
            item.right() - css::BADGE_INSET - css::BADGE / 2.0,
            item.top() + css::BADGE_INSET + css::BADGE / 2.0,
        );
        let drawn: Vec<(Pos2, f32, Color32)> = circles(h, item)
            .into_iter()
            .map(|circle| (circle.center, circle.radius, circle.fill))
            .collect();
        assert_eq!(drawn.len(), 2, "{what}'s badge: {drawn:?}");
        let ring = (css::BADGE / 2.0 + css::BADGE_RING, css::G150);
        let dot = (css::BADGE / 2.0, fill);
        for (got, (radius, colour)) in drawn.iter().zip([ring, dot]) {
            assert!(
                near_pos(got.0, centre) && near(got.1, radius) && got.2 == colour,
                "{what}'s badge: {drawn:?}, not {radius} at {centre:?} in {colour:?}"
            );
        }
    }

    #[test]
    fn a_feature_another_program_changed_has_the_purple_badge_where_it_has_no_health_mark() {
        // `.tl-item .badge.outside`: `--purple` (#b780ff), 7 square, 1 in from the item's
        // top right, in its `--g150` ring 1.5 outside it; labelled with what changed. A
        // feature with a health mark shows that mark alone.
        const PURPLE: Color32 = Color32::from_rgb(0xb7, 0x80, 0xff);
        const OUTSIDE: f32 = 7.0;
        let mut tab = tab_of(Some(&features()), 0);
        for id in ["b0000001", "p0000001"] {
            tab.outside
                .insert(FeatureId::new(id).unwrap(), "height 10 → 11".to_owned());
        }
        let h = drawn(tab);
        let block = item(0);
        let centre = Pos2::new(
            block.right() - css::BADGE_INSET - OUTSIDE / 2.0,
            block.top() + css::BADGE_INSET + OUTSIDE / 2.0,
        );
        let drawn: Vec<(Pos2, f32, Color32)> = circles(&h, block)
            .into_iter()
            .map(|circle| (circle.center, circle.radius, circle.fill))
            .collect();
        assert_eq!(drawn.len(), 2, "the block's badge: {drawn:?}");
        let ring = (OUTSIDE / 2.0 + css::BADGE_RING, css::G150);
        let dot = (OUTSIDE / 2.0, PURPLE);
        for (got, (radius, colour)) in drawn.iter().zip([ring, dot]) {
            assert!(
                near_pos(got.0, centre) && near(got.1, radius) && got.2 == colour,
                "the block's badge: {drawn:?}, not {radius} at {centre:?} in {colour:?}"
            );
        }
        let said = h.get_by_label("changed on disk: height 10 → 11").rect();
        assert!(
            near_pos(said.center(), centre) && near(said.width(), OUTSIDE),
            "{said:?}"
        );
        // The failed pad: its error's badge only.
        badge(&h, item(2), css::RED_FILL, "the pad");
    }

    #[test]
    fn the_items_are_drawn_as_tl_item_in_each_state() {
        let mut tab = tab_of(Some(&features()), 0);
        tab.selection_mut()
            .select_feature(FeatureId::new("p0000001").unwrap());
        let mut h = drawn(tab);
        // `:hover`: the block hovered.
        h.input_mut()
            .events
            .push(Event::PointerMoved(item(0).center()));
        h.run_steps(2);

        // `.tl-item`: 30 square, 2 apart along the track from its left, centred on the
        // content's middle; its icon `svg.i.lg` at its centre. Hovered: `--g1000` on `--g300`.
        assert_eq!(
            fills_at(&h, item(0)),
            [(css::G300, CornerRadius::same(css::R_SM))],
            "the hovered block's fill"
        );
        assert_eq!(icon_at(&h, icon_of(item(0))), Some(css::G1000), "the block");
        // `.sketch`: `--g600`, no fill.
        assert_eq!(icon_at(&h, icon_of(item(1))), Some(css::G600), "the sketch");
        assert_eq!(fills_at(&h, item(1)), [], "the sketch's fill");
        // `.selected.err`: `--red` on `--blue`, its line along the bottom inside its
        // corners, and its badge.
        assert_eq!(
            fills_at(&h, item(2)),
            [(css::BLUE, CornerRadius::same(css::R_SM))],
            "the selected pad's fill"
        );
        assert_eq!(icon_at(&h, icon_of(item(2))), Some(css::RED), "the pad");
        let line =
            |at: Rect| Rect::from_min_max(Pos2::new(at.left(), at.bottom() - css::LINE), at.max);
        let under = CornerRadius {
            nw: 0,
            ne: 0,
            sw: css::R_SM,
            se: css::R_SM,
        };
        assert_eq!(fills_at(&h, line(item(2))), [(css::RED, under)]);
        badge(&h, item(2), css::RED_FILL, "the pad");
        // `.warn`: `--orange`, its line and its badge.
        assert_eq!(
            icon_at(&h, icon_of(item(3))),
            Some(css::ORANGE),
            "the cylinder"
        );
        assert_eq!(fills_at(&h, line(item(3))), [(css::ORANGE, under)]);
        badge(&h, item(3), css::ORANGE, "the cylinder");
        // `.suppressed`: `.35` of the whole, struck through: 5 in from each side, 1.5 wide,
        // turned 35 degrees anticlockwise about the centre, in its icon's colour.
        assert_eq!(
            icon_at(&h, icon_of(item(4))),
            Some(css::G750_SUPPRESSED),
            "the fillet"
        );
        let half = css::ITEM / 2.0 - css::STRIKE_INSET;
        let along = Vec2::new(css::COS_35, -css::SIN_35) * half;
        let centre = item(4).center();
        let strikes: Vec<([Pos2; 2], Stroke)> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::LineSegment { points, stroke } => Some((points, stroke)),
                _ => None,
            })
            .filter(|(points, _)| item(4).contains(points[0]))
            .collect();
        assert_eq!(strikes.len(), 1, "the fillet's strike: {strikes:?}");
        let ([from, to], stroke) = strikes[0];
        let ends = [centre - along, centre + along];
        assert!(
            (from - ends[0]).length() < 1e-3 && (to - ends[1]).length() < 1e-3,
            "the strike: {from:?} to {to:?}, not {ends:?}"
        );
        assert_eq!(stroke, Stroke::new(css::STRIKE_W, css::G750_SUPPRESSED));
        // No other item has a badge, and none a line.
        for at in [0, 1, 4] {
            assert_eq!(circles(&h, item(at)).len(), 0, "item {at}'s badge");
            assert_eq!(fills_at(&h, line(item(at))), [], "item {at}'s line");
        }

        // The pad rebuilding: `.building`'s ring on it, its inputs as at rest, the rest
        // `.queued` (`.45`), and no badge while it runs.
        let mut tab = tab_of(Some(&features()), 0);
        running(
            &mut tab,
            Rebuild {
                revision: LISTED,
                state: RebuildState::Running,
                progress: Some(Progress {
                    done: 2,
                    steps: 4,
                    running: Some("p0000001 pad".to_owned()),
                    features: FeatureStates::default(),
                }),
            },
        );
        let h = drawn(tab);
        assert_eq!(icon_at(&h, icon_of(item(0))), Some(css::G750), "the block");
        assert_eq!(icon_at(&h, icon_of(item(1))), Some(css::G600), "the sketch");
        assert_eq!(icon_at(&h, icon_of(item(2))), Some(css::G750), "the pad");
        assert_eq!(
            icon_at(&h, icon_of(item(3))),
            Some(css::G750_QUEUED),
            "the cylinder"
        );
        for at in 0..5 {
            assert_eq!(circles(&h, item(at)).len(), 0, "item {at}'s badge");
        }
        // The ring: `inset: 2px`, its `2px` border's middle 13 - 1 = 12 from the centre; its
        // top and right borders, from the upper left (225 degrees round, y down) half a turn
        // clockwise to the lower right, turned `t / .7s` of a turn on.
        let time = h.ctx.input(|input| input.time);
        assert!(time > 0.0, "the harness's clock: {time}");
        let turned = (time / css::SPIN_SECONDS).fract() as f32 * std::f32::consts::TAU;
        let radius = css::ITEM / 2.0 - css::SPIN_INSET - css::SPIN_BORDER / 2.0;
        let rings: Vec<PathShape> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Path(path) => Some(path),
                _ => None,
            })
            .filter(|path| path.points.iter().all(|point| item(2).contains(*point)))
            .collect();
        assert_eq!(rings.len(), 1, "the pad's spinner");
        let ring = &rings[0];
        assert!(!ring.closed);
        assert_eq!(ring.stroke.width, css::SPIN_BORDER);
        assert_eq!(
            ring.stroke.color,
            egui::epaint::ColorMode::Solid(css::G1000)
        );
        let centre = item(2).center();
        let start = (-135.0f32).to_radians() + turned;
        let point = |angle: f32| centre + Vec2::new(libm::cosf(angle), libm::sinf(angle)) * radius;
        let first = ring.points[0];
        let last = ring.points[ring.points.len() - 1];
        assert!(
            (first - point(start)).length() < 1e-3,
            "the ring starts at {first:?}, not {:?}",
            point(start)
        );
        assert!(
            (last - point(start + std::f32::consts::PI)).length() < 1e-3,
            "the ring ends at {last:?}, not {:?}",
            point(start + std::f32::consts::PI)
        );
        for p in &ring.points {
            assert!(
                ((*p - centre).length() - radius).abs() < 1e-3,
                "{p:?} is off the ring"
            );
        }
        // Half a turn, evenly: each point a 32nd of it on from the last.
        for pair in ring.points.windows(2) {
            let step = (pair[1] - centre).angle() - (pair[0] - centre).angle();
            let step = step.rem_euclid(std::f32::consts::TAU);
            assert!(
                (step - std::f32::consts::PI / 32.0).abs() < 1e-3,
                "a step of {step}"
            );
        }
    }

    #[test]
    fn the_summary_is_drawn_as_tl_right_and_the_track_ends_a_gap_before_it() {
        // Two errors, one of the document's own and the pad's, so 32 items overflow the track.
        let mut many = features();
        for n in 0..27 {
            many.push(entry(&format!("x{n:07}"), "box", &[], Health::Healthy));
        }
        let h = drawn(tab_of(Some(&many), 1));
        // `.tl-right`: flush with the content's right (the gear's left out); its count
        // `--g550`, and 2 before it the errors' `button.st-item.err-text`, 8 of padding, the
        // icon 14 square and 6 from its words, all `--red`.
        let right = panel().right() - css::PADDING;
        let count = text(&h, "32 features");
        let count = said_at(&h, "32 features", right - count.galley.size().x, css::G550);
        let errors = text(&h, "2 errors");
        let errors = said_at(
            &h,
            "2 errors",
            count.left() - css::RIGHT_GAP - css::ST_PADDING - errors.galley.size().x,
            css::RED,
        );
        let icon = Rect::from_min_size(
            Pos2::new(
                errors.left() - css::ST_GAP - css::ICON,
                middle() - css::ICON / 2.0,
            ),
            Vec2::splat(css::ICON),
        );
        assert_eq!(icon_at(&h, icon), Some(css::RED), "the errors' icon");
        assert!(h.query_by_label("2 errors · 32 features").is_some());
        // `.tl-track`: from after the label to `gap: 8px` before the summary's button, its
        // items clipped to it.
        let end = icon.left() - css::ST_PADDING - css::GAP;
        let clips: Vec<Rect> = clipped(&h)
            .into_iter()
            .filter(|shape| match &shape.shape {
                Shape::Rect(rect) => rect.brush.is_some() && item(0).contains(rect.rect.center()),
                _ => false,
            })
            .map(|shape| shape.clip_rect)
            .collect();
        assert_eq!(clips.len(), 1, "the block's icon");
        assert!(
            near(clips[0].left(), track_left()) && near(clips[0].right(), end),
            "the track: {:?}, not from {} to {end}",
            clips[0],
            track_left()
        );
    }

    #[test]
    fn the_summarys_errors_are_a_button_that_opens_whats_wrong() {
        use eframe::egui::accesskit::Role;
        // The prototype's `.tl-right button.st-item.err-text` (`data-act="whatsWrong"`): one
        // of the document's own and the pad's.
        let mut h = drawn(tab_of(Some(&features()), 1));
        assert!(!h.state().tab.as_ref().unwrap().whats_wrong);
        h.get_by_role_and_label(Role::Button, "2 errors").click();
        h.run_steps(2);
        assert!(h.state().tab.as_ref().unwrap().whats_wrong);
        // Not one of the track's items, which come first.
        let track = h.get_by_role_and_label(Role::Group, TRACK_LABEL);
        assert!(track.query_by_label("2 errors").is_none());
        assert!(track.query_by_label("p0000001").is_some());
        // With none, no button.
        let clean = [entry("b0000001", "box", &[], Health::Healthy)];
        let h = drawn(tab_of(Some(&clean), 0));
        assert!(h.query_by_label_contains("error").is_none());
    }

    #[test]
    fn the_summarys_errors_button_is_st_item_20_high_padded_8_and_lit_when_hovered() {
        use eframe::egui::accesskit::Role;
        let mut h = drawn(tab_of(Some(&features()), 1));
        // `button.st-item`: from 8 before its icon to 8 after its words, 20 high on the
        // content's middle.
        let count = text(&h, "5 features");
        let right = panel().right() - css::PADDING;
        assert!(near(count.pos.x + count.galley.size().x, right));
        let words = text(&h, "2 errors");
        let words = Rect::from_min_size(words.pos, words.galley.size());
        let icon_left = words.left() - css::ST_GAP - css::ICON;
        let place = Rect::from_min_max(
            Pos2::new(icon_left - css::ST_PADDING, middle() - css::ST_H / 2.0),
            Pos2::new(words.right() + css::ST_PADDING, middle() + css::ST_H / 2.0),
        );
        let button = h.get_by_role_and_label(Role::Button, "2 errors").rect();
        assert!(near_rect(button, place), "{button:?}, not {place:?}");
        // Not hovered: no fill.
        assert!(fills_at(&h, place).is_empty());
        // `button.st-item:hover`: `--g300` under it, `--r-sm` round, its icon and words
        // `--g1000`.
        h.input_mut()
            .events
            .push(Event::PointerMoved(place.center()));
        h.run_steps(2);
        assert_eq!(
            fills_at(&h, place),
            [(css::G300, CornerRadius::same(css::R_SM))]
        );
        let icon = Rect::from_min_size(
            Pos2::new(icon_left, middle() - css::ICON / 2.0),
            Vec2::splat(css::ICON),
        );
        assert_eq!(icon_at(&h, icon), Some(css::G1000), "the icon, hovered");
        assert_eq!(
            colour(&text(&h, "2 errors")),
            css::G1000,
            "the words, hovered"
        );
        // Off it, by its edge: red again, unlit.
        h.input_mut().events.push(Event::PointerMoved(Pos2::new(
            place.center().x,
            place.top() - 2.0,
        )));
        h.run_steps(2);
        assert!(fills_at(&h, place).is_empty());
        assert_eq!(colour(&text(&h, "2 errors")), css::RED);
    }

    #[test]
    fn show_in_timeline_selects_the_feature_and_scrolls_its_item_into_the_track() {
        use egui_kittest::kittest::NodeT;
        // 40 items, 40 × 30 and 39 gaps of 4, about 1356 wide: past the 800 wide panel, so
        // the last starts out of view at its right.
        let mut many = features();
        for n in 0..35 {
            many.push(entry(&format!("x{n:07}"), "box", &[], Health::Healthy));
        }
        let last = many[39].id.clone();
        let mut h = drawn(tab_of(Some(&many), 0));
        let shown = |h: &Harness<'_, Shown>| h.get_by_label(last.as_str()).rect();
        let before = shown(&h);
        assert!(
            before.left() > panel().right(),
            "{before:?} in view already"
        );
        let toggled = |h: &Harness<'_, Shown>| {
            h.get_by_label(last.as_str())
                .accesskit_node()
                .toggled()
                .map(|toggled| toggled == eframe::egui::accesskit::Toggled::True)
        };
        assert_eq!(toggled(&h), Some(false));
        h.state_mut()
            .tab
            .as_mut()
            .unwrap()
            .show_in_timeline(last.clone());
        h.run_steps(4);
        // Selected, and on the track, between its left and the panel's right.
        let tab = h.state().tab.as_ref().unwrap();
        assert_eq!(tab.selection().feature(), Some(&last));
        assert_eq!(tab.timeline_scroll(), None, "scrolled to once");
        let after = shown(&h);
        assert!(
            after.left() >= track_left() && after.right() <= panel().right() - css::PADDING,
            "{after:?} out of the track"
        );
        assert_eq!(toggled(&h), Some(true));
    }

    /// A drag from `from` to `to`, held: pressed at `from`, moved past the drag threshold,
    /// then to `to`.
    fn drag(h: &mut Harness<'_, Shown>, from: Pos2, to: Pos2) {
        let input = h.input_mut();
        input.events.push(Event::PointerMoved(from));
        input.events.push(Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        h.run();
        h.input_mut()
            .events
            .push(Event::PointerMoved(from + Vec2::new(0.0, 10.0)));
        h.run();
        h.input_mut().events.push(Event::PointerMoved(to));
        h.run_steps(2);
    }

    #[test]
    fn a_dragged_item_shows_where_it_drops_as_drop_before() {
        let mut h = drawn(tab_of(Some(&features()), 0));
        // The block dragged right of the pad's centre: before the cylinder.
        drag(
            &mut h,
            item(0).center(),
            item(2).center() + Vec2::new(4.0, 0.0),
        );
        let before = item(3);
        let line = Rect::from_min_max(
            Pos2::new(
                before.left() + css::DROP_LEFT,
                before.top() + css::DROP_INSET,
            ),
            Pos2::new(
                before.left() + css::DROP_LEFT + css::DROP_W,
                before.bottom() - css::DROP_INSET,
            ),
        );
        assert_eq!(
            fills_at(&h, line),
            [(css::BLUE_HI, CornerRadius::ZERO)],
            "the drop line"
        );
        // Nothing is moved until it's dropped.
        assert!(!h.state().started);
    }
}
