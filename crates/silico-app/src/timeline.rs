//! The timeline along the window's bottom (docs/design/m5.0-app.md, section 8, *The
//! timeline*; the approved prototype, docs/design/m5-ui/DESIGN.md, *The idea in one line*
//! and *egui mapping*, and its `.tl-*` rules): a panel [`theme::TIMELINE_HEIGHT`] high,
//! its label, each feature in order as its kind's icon on a 30 point item, and at its right
//! how many errors and features there are. The rebuild runs visibly along it: the running
//! step's item spins, the items not yet known built are dimmed, and once it ends each item
//! shows its health, a failed one red ([`step_of_each`]). A click selects a feature, a
//! double-click (or Edit Feature in its menu) opens its dialog, a drag moves it
//! (`move_feature`), and hovering it tints the faces it made in the viewport (`View::linked`).
//!
//! Left out, each for a reason: the rollback bar (the core has no rollback; Fusion's marker
//! is out of M5, DESIGN.md flags Onshape's as a proposal), the gear's timeline options (none
//! exist), a click on the errors opening What's Wrong (not built yet), and the reverse of
//! the hover linking, a face hovered outlining its feature here (`.hovered-from-canvas`):
//! SEAM(M5.7), picking's, once it names the face under the pointer and its feature.

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Rect, Sense, Shape, Stroke, Ui, UiBuilder, Vec2,
    WidgetInfo, WidgetType, pos2, vec2,
};
use silico_core::command::{Rebuild, RebuildState, Sessions};
use silico_core::document::FeatureId;
use silico_core::kernel::Clients;
use silico_core::timeline::{Entry, Health};

use crate::icons::{Icon, IconSize};
use crate::panes::{self, MarkLook};
use crate::tab::Tab;
use crate::theme;

/// The pane's name, as the accessibility tree labels it.
pub const LABEL: &str = "Timeline";

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

/// What each of `entries` shows of `rebuild`, the session's, as `status` gives it.
///
/// While it runs, `status` names one feature, the first of the step started last of those
/// running (`Progress::running`, `4kq2m7vd pad`), and counts the rest: that feature runs,
/// and every input it names, however far back, is built (a step starts only once its inputs
/// settle). Every other feature is queued, until the rebuild ends: one on another branch may
/// have built already, but `status` doesn't say which. SEAM: with the features each settled
/// step was planned for (or each step's state) in `Progress`, every built item would show
/// so, and every running one spin, the parallel branches too.
///
/// A suppressed feature is never rebuilt: it stays suppressed throughout.
pub fn step_of_each(entries: &[Entry], rebuild: Option<&Rebuild>) -> Vec<Step> {
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
    let mut built = vec![false; entries.len()];
    if let Some(at) = running {
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
            if entry.suppressed || entry.health == Health::Suppressed {
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
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Pane);
                node.set_label(LABEL);
            });
            let rect = ui.max_rect();
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
    let errors = timeline.problems
        + entries
            .iter()
            .map(|entry| entry.problems)
            .fold(0u64, u64::saturating_add);
    let (track, summary_rect, said) = summary_at(ui, rect, summary(errors, entries.len()));
    let rebuild = tab.status.as_ref().map(|status| &status.rebuild);
    let steps = step_of_each(entries, rebuild);
    let mut asked = Asked::default();
    if entries.is_empty() {
        words(
            ui,
            track,
            "Features appear here, in order, as you add them.",
        );
    } else {
        ui.scope_builder(UiBuilder::new().max_rect(track), |ui| {
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
    summary_label(ui, summary_rect, &said);
    if steps.contains(&Step::Running) {
        // The spinner turns only while a step runs.
        ui.ctx().request_repaint();
    }
    tab.view.linked = asked.hovered;
    if let Some(id) = asked.selected {
        tab.selected = Some(id);
    }
    if let Some((feature, before)) = asked.moved {
        tab.move_feature(sessions, feature, before);
        *started = true;
    }
    if let Some(id) = asked.edit {
        // SEAM(#351): `tab.edit_sketch_or_feature(sessions, &id)`, Edit Sketch for a sketch;
        // until then a sketch has no dialog, and this opens none.
        tab.edit_feature(sessions, &id);
        *started = true;
    }
}

/// The summary drawn at `rect`'s right (`.tl-right`): its errors in the error's red with the
/// error's icon, then its features: the rect left for the track, the summary's, and what
/// it says, for its label in the accessibility tree ([`summary_label`]), added after the
/// items so it's last in the pane.
fn summary_at(
    ui: &mut Ui,
    rect: Rect,
    (errors, features): (Option<String>, String),
) -> (Rect, Rect, String) {
    let font = egui::FontId::proportional(theme::HEADER_SIZE);
    let painter = ui.painter().clone();
    let pad = theme::TIMELINE_SUMMARY_PADDING;
    let centre = rect.center().y;
    let galley = painter.layout_no_wrap(features.clone(), font.clone(), theme::TIMELINE_SUMMARY);
    let mut right = rect.right() - pad - galley.size().x;
    let mut spans = vec![(right, galley, theme::TIMELINE_SUMMARY)];
    right -= pad;
    if let Some(errors) = &errors {
        let galley = painter.layout_no_wrap(errors.clone(), font, theme::TIMELINE_SUMMARY_ERRORS);
        right -= pad + galley.size().x;
        let icon_at = right - theme::TIMELINE_SUMMARY_GAP - crate::icons::ICON_SIZE;
        Icon::Error
            .image(IconSize::Small)
            .tint(theme::TIMELINE_SUMMARY_ERRORS)
            .paint_at(
                ui,
                Rect::from_center_size(
                    pos2(icon_at + crate::icons::ICON_SIZE / 2.0, centre),
                    Vec2::splat(crate::icons::ICON_SIZE),
                ),
            );
        spans.push((right, galley, theme::TIMELINE_SUMMARY_ERRORS));
        right = icon_at - pad;
    }
    let mut whole = Rect::NOTHING;
    for (x, galley, colour) in spans {
        let at = pos2(x, centre - galley.size().y / 2.0);
        whole = whole.union(Rect::from_min_size(at, galley.size()));
        painter.galley(at, galley, colour);
    }
    let said = match errors {
        Some(errors) => format!("{errors} · {features}"),
        None => features,
    };
    let track = Rect::from_x_y_ranges(
        rect.left()..=(right - theme::TIMELINE_GAP).max(rect.left()),
        rect.y_range(),
    );
    (track, whole, said)
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
        let selected = tab.selected.as_ref() == Some(&entry.id);
        let description = format!("{} · {}", entry.kind, step.words());
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
        let sketch = entry.kind == "sketch";
        paint(
            ui,
            rect,
            Icon::of_kind(entry.kind),
            &look(*step, sketch, hovered, selected),
        );
        theme::focus_ring(
            ui,
            &response,
            rect,
            theme::TIMELINE_ITEM_CORNER_RADIUS,
            egui::StrokeKind::Inside,
        );
        if let Step::Settled(health) = step {
            let said = tab.said.get(entry.id.as_str());
            let corner = pos2(
                rect.right() - theme::TIMELINE_BADGE_INSET - theme::TIMELINE_BADGE_SIZE,
                rect.top() + theme::TIMELINE_BADGE_INSET,
            );
            panes::health_mark(
                ui,
                id.with("mark"),
                Rect::from_min_size(corner, Vec2::splat(theme::TIMELINE_BADGE_SIZE)),
                *health,
                said.map(|said| said.message.as_str()),
                MarkLook::Badge,
            );
        }
        if response.dragged() {
            dragged = Some(at);
        }
        if response.drag_stopped() {
            dropped = Some(at);
        }
        let editable = crate::dialog::Kind::of(entry.kind).is_some();
        if response.clicked() {
            asked.selected = Some(entry.id.clone());
        }
        if response.double_clicked() {
            asked.edit = Some(entry.id.clone());
        }
        response.context_menu(|ui| {
            if ui
                .add_enabled(editable, egui::Button::new("Edit Feature"))
                .clicked()
            {
                asked.edit = Some(entry.id.clone());
                ui.close();
            }
        });
        let said = tab.said.get(entry.id.as_str());
        let mut tip = description;
        if let Some(said) = said {
            tip = format!("{tip} — {}", said.message);
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
    use silico_core::command::{Progress, Revision};

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
            }),
        }
    }

    /// A plate, a sketch on its own, a pad of the sketch on the plate, a pin off to the
    /// side, a cut of the pad by the pin, and a suppressed fillet of the cut.
    fn part() -> Vec<Entry> {
        vec![
            entry("b0000001", "box", &[], Health::Healthy),
            entry("s0000001", "sketch", &[], Health::Healthy),
            entry(
                "p0000001",
                "pad",
                &["b0000001", "s0000001"],
                Health::Healthy,
            ),
            entry("c0000001", "cylinder", &[], Health::Warning),
            entry("m0000001", "cut", &["p0000001", "c0000001"], Health::Error),
            entry("f0000001", "fillet", &["m0000001"], Health::Suppressed),
        ]
    }

    #[test]
    fn with_no_rebuild_running_each_item_is_its_health() {
        use Health::*;
        let want = [Healthy, Healthy, Healthy, Warning, Error, Suppressed].map(Step::Settled);
        for state in [
            RebuildState::Idle,
            RebuildState::Done,
            RebuildState::Stopped,
            RebuildState::Faulted,
        ] {
            assert_eq!(
                step_of_each(&part(), Some(&rebuild(state, Some("p0000001 pad")))),
                want,
                "{state:?}"
            );
        }
        assert_eq!(step_of_each(&part(), None), want);
    }

    #[test]
    fn the_running_step_spins_its_inputs_are_built_and_the_rest_are_queued() {
        use Step::*;
        let suppressed = Settled(Health::Suppressed);
        // The pad running: the plate and the sketch are its inputs, so built; the pin, off
        // its lineage, and the cut after it are queued; the fillet stays suppressed.
        assert_eq!(
            step_of_each(
                &part(),
                Some(&rebuild(RebuildState::Running, Some("p0000001 pad")))
            ),
            [Built, Built, Running, Queued, Queued, suppressed]
        );
        // The cut running: every input back to the plate built.
        assert_eq!(
            step_of_each(
                &part(),
                Some(&rebuild(RebuildState::Running, Some("m0000001 cut")))
            ),
            [Built, Built, Built, Built, Running, suppressed]
        );
        // Nothing running yet (its sketches solving, before its plan), or a feature the
        // timeline doesn't list (a newer revision's): every unsuppressed one queued.
        for running in [None, Some("z0000009 box"), Some("")] {
            assert_eq!(
                step_of_each(&part(), Some(&rebuild(RebuildState::Running, running))),
                [Queued, Queued, Queued, Queued, Queued, suppressed],
                "{running:?}"
            );
        }
        let no_progress = Rebuild {
            progress: None,
            ..rebuild(RebuildState::Running, None)
        };
        assert_eq!(
            step_of_each(&part(), Some(&no_progress)),
            [Queued, Queued, Queued, Queued, Queued, suppressed]
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
}
