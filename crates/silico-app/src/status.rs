//! The status bar along the window's bottom (docs/design/m5-ui/DESIGN.md, *The idea in one
//! line*, *Layout*, and *egui mapping*; its `#status` and `.st-*` rules and `renderStatus`):
//! the rebuild in words, with a dot or a spinner in its state's colour (rebuilt with errors,
//! a button that opens What's Wrong), and Stop and Rebuild beside it; then, in sketch mode,
//! the sketch edited, the pointer's coordinates on its plane, and its state
//! (`js/sketch.js`'s `statusText`), or what the person points at; then, at the right, the
//! file changed on disk, whether the document is saved, its units, and the shortcut
//! sheet's button (`crate::sheet`). With no document open, how the warm start stands
//! (decision 10) in the rebuild's place.
//!
//! Left out, each for a reason, each a seam:
//! - the rebuild's time (`· 84 ms`): `status.rebuild` gives no duration, and the app's own
//!   clock would time its polls, not the rebuild. SEAM: a duration in the core's `Rebuild`.
//! - the edits' count (`Unsaved · 2 edits`): the core's undo stack counts edits, but the
//!   app's Undo steps over a dialog's edits as one, so neither count is the person's. SEAM:
//!   the tab's count of its own undo steps.
//! - the hovered face's surface type and feature in the middle (`Face6 · Planar · Pad`):
//!   SEAM, an `elements` read of it.

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    Align, Color32, FontId, Layout, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};
use silico_core::command::{RebuildState, RunningCommand};
use silico_core::disk::Disk;
use silico_core::document::Units;

use crate::icons::{Icon, IconSize};
use crate::shortcuts::Command;
use crate::sketch::bar::StatusLine;
use crate::start::{self, Warming};
use crate::tab::Tab;
use crate::theme;

/// What the rebuild's place says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// No document: how the warm start stands.
    Warm(Warming),
    /// Nothing: no rebuild has started (or the gate refused the document).
    Nothing,
    /// The session reloads its file, changed on disk: the file's name.
    Reloading(String),
    /// A rebuild runs: a dialog's preview or not, and its steps settled of its plan's, once
    /// it has one.
    Rebuilding {
        preview: bool,
        progress: Option<(usize, usize)>,
    },
    /// The rebuild was stopped, as far as it had got.
    Stopped(Option<(usize, usize)>),
    /// Rebuilt: the revision, and its errors (the gate's and its features').
    Rebuilt { revision: u64, errors: u64 },
    /// The kernel faulted.
    Faulted,
}

impl Said {
    /// Its words.
    pub fn words(&self) -> String {
        let plural = |n: u64| if n == 1 { "" } else { "s" };
        match self {
            Said::Warm(warming) => warming.said().to_owned(),
            Said::Nothing => String::new(),
            Said::Reloading(name) => format!("Reloading {name} from disk"),
            Said::Rebuilding { preview, progress } => {
                let what = if *preview {
                    "Rebuilding the preview"
                } else {
                    "Rebuilding"
                };
                match progress {
                    Some((done, steps)) => format!("{what} · {done} of {steps}"),
                    None => what.to_owned(),
                }
            }
            Said::Stopped(Some((done, steps))) => format!("Rebuild stopped at {done} of {steps}"),
            Said::Stopped(None) => "Rebuild stopped".to_owned(),
            Said::Rebuilt {
                revision,
                errors: 0,
            } => format!("Rebuilt · revision {revision}"),
            Said::Rebuilt { errors, .. } => {
                format!("Rebuilt with {errors} error{}", plural(*errors))
            }
            Said::Faulted => "The kernel faulted".to_owned(),
        }
    }
}

/// Whether the document is saved, as the bar says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    Saved,
    Unsaved,
    /// Unsaved, with a dialog open, whose edits are its own until OK.
    DialogOpen,
}

/// What the bar shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bar {
    pub said: Said,
    /// Stop and Rebuild, each on or off; none with no document.
    pub buttons: Option<(bool, bool)>,
    /// Refusals said to the person (the window's, then the tab's).
    pub messages: Vec<String>,
    /// What the selection says (M5.7): what the pointer is over in the canvas, or how many
    /// elements are selected; and what the last change to it dropped, and why.
    pub selection: Vec<String>,
    /// The last comparison of the file on disk, as `status.disk` gives it.
    pub disk: Option<Disk>,
    /// Whether the document is saved; none with no document.
    pub saved: Option<Saved>,
    /// In sketch mode, what the bar says of the sketch.
    pub sketch: Option<StatusLine>,
}

/// What the person asked of the bar this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Asked {
    pub stop: bool,
    pub rebuild: bool,
    /// The Changed on disk chip: the choice it stands for offered again.
    pub disk: bool,
    /// Shortcuts: the shortcut sheet opened.
    pub shortcuts: bool,
    /// Rebuilt with errors: What's Wrong opened.
    pub whats_wrong: bool,
}

/// The bar for `tab`, the tab shown, and `message`, the window's refusal, if it has one.
pub fn of(tab: &Tab, message: Option<String>) -> Bar {
    let status = tab.status.as_ref();
    let progress = status
        .and_then(|status| status.rebuild.progress.as_ref())
        .map(|progress| (progress.done, progress.steps));
    let said = match status {
        None => Said::Nothing,
        Some(status) if status.running == Some(RunningCommand::Reload) => {
            Said::Reloading(tab.name().to_owned())
        }
        Some(status) => match status.rebuild.state {
            RebuildState::Running => Said::Rebuilding {
                preview: tab.preview_rebuilding(),
                progress,
            },
            RebuildState::Stopped => Said::Stopped(progress),
            RebuildState::Done => Said::Rebuilt {
                revision: status.rebuild.revision.number(),
                errors: tab.errors(),
            },
            RebuildState::Faulted => Said::Faulted,
            RebuildState::Idle => Said::Nothing,
        },
    };
    let saved = if !tab.unsaved() {
        Saved::Saved
    } else if tab.in_dialog() {
        Saved::DialogOpen
    } else {
        Saved::Unsaved
    };
    Bar {
        said,
        buttons: Some((tab.stoppable(), tab.can_rebuild())),
        messages: message.into_iter().chain(tab.message.clone()).collect(),
        selection: selection_words(tab.selection()),
        disk: status.and_then(|status| status.disk),
        saved: Some(saved),
        sketch: tab
            .sketch
            .as_ref()
            .map(|mode| mode.status(&tab.sketch_name())),
    }
}

/// The bar with no document open: the warm start, if there is one, and `message`.
pub fn unopened(warming: Option<Warming>, message: Option<String>) -> Bar {
    Bar {
        said: warming.map_or(Said::Nothing, Said::Warm),
        buttons: None,
        messages: message.into_iter().collect(),
        selection: Vec::new(),
        disk: None,
        saved: None,
        sketch: None,
    }
}

/// What the status bar says of `selection`: what the pointer is over (its name, as an agent
/// writes it, and Unstable if a store wouldn't take it), or else how many elements are
/// selected; then what its last change said (a name a rebuild left gone, and why).
pub fn selection_words(selection: &crate::selection::Selection) -> Vec<String> {
    let pointed = match selection.hovered() {
        Some(hovered) => {
            let stable = hovered
                .found
                .as_ref()
                .map_or(hovered.stable, silico_core::selection::Selected::stable);
            let name = hovered
                .found
                .as_ref()
                .map_or_else(|| hovered.name.to_string(), |found| found.name.to_string());
            Some(if stable {
                name
            } else {
                format!("{name} · Unstable")
            })
        }
        None => match selection.elements().count() {
            0 => None,
            1 => Some("1 selected".to_owned()),
            n => Some(format!("{n} selected")),
        },
    };
    pointed
        .into_iter()
        .chain(selection.said().map(str::to_owned))
        .collect()
}

/// The bar drawn across `ui`'s whole rectangle: what the person asked of it.
pub fn bar(ui: &mut Ui, bar: &Bar) -> Asked {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0, theme::STATUS_BAR_BG);
    ui.painter().hline(
        rect.x_range(),
        rect.top() + theme::BORDER_WIDTH / 2.0,
        Stroke::new(theme::BORDER_WIDTH, theme::STATUS_BAR_BORDER),
    );
    // `border-box`: its parts are centred in what's under the border.
    let inner = Rect::from_min_max(
        pos2(
            rect.left() + theme::STATUS_BAR_PADDING_LEFT,
            rect.top() + theme::BORDER_WIDTH,
        ),
        pos2(
            rect.right() - theme::STATUS_BAR_PADDING_RIGHT,
            rect.bottom(),
        ),
    );
    let mut asked = Asked::default();
    // The right's parts first, from the right, so the left's know where to stop.
    let right = ui
        .scope_builder(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing = vec2(theme::STATUS_BAR_GAP, 0.0);
                right_parts(ui, bar, &mut asked);
                ui.min_rect().left()
            },
        )
        .inner;
    let left = Rect::from_min_max(inner.min, pos2(right.min(inner.right()), inner.bottom()));
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(left)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing = vec2(theme::STATUS_BAR_GAP, 0.0);
            ui.set_clip_rect(left.intersect(ui.clip_rect()));
            left_parts(ui, bar, &mut asked);
        },
    );
    asked
}

/// The rebuild, Stop and Rebuild, and the middle: from the left.
fn left_parts(ui: &mut Ui, bar: &Bar, asked: &mut Asked) {
    let words = bar.said.words();
    match &bar.said {
        Said::Warm(warming) => {
            ui.add_space(theme::STATUS_ITEM_PADDING);
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = vec2(theme::STATUS_ITEM_GAP, 0.0);
                start::warm_status(ui, *warming);
            });
            ui.add_space(theme::STATUS_ITEM_PADDING);
        }
        Said::Nothing => {}
        Said::Reloading(_) => {
            item(ui, Some(Mark::Spinner), &words, theme::TEXT_STRONG);
        }
        Said::Rebuilding { progress, .. } => {
            item(ui, Some(Mark::Spinner), &words, theme::TEXT_STRONG);
            if let Some((done, steps)) = progress {
                bar_of(ui, *done, *steps);
            }
        }
        Said::Stopped(_) => {
            item(ui, Some(Mark::Icon(Icon::Stop)), &words, theme::WARNING);
        }
        Said::Rebuilt { errors: 0, .. } => {
            item(
                ui,
                Some(Mark::Dot(theme::SUCCESS)),
                &words,
                theme::STATUS_TEXT,
            );
        }
        Said::Rebuilt { .. } => {
            // A button (`button.st-item`), as the prototype's `whatsWrong`.
            let errors = item_with(
                ui,
                Some(Mark::Icon(Icon::Error)),
                plain(&words, theme::ERROR_TEXT),
                true,
            );
            asked.whats_wrong = crate::bar::tooltip(
                errors,
                Command::WhatsWrong.label(),
                None,
                Some("Lists every problem, with what to do"),
            )
            .clicked();
        }
        Said::Faulted => {
            item(ui, Some(Mark::Icon(Icon::Error)), &words, theme::ERROR_TEXT);
        }
    }
    if let Some((stop, rebuild)) = bar.buttons {
        asked.stop = button(ui, Icon::Stop, "Stop", stop, true).clicked();
        // Rebuild (Ctrl+B), on while the tab's revision's rebuild is stopped.
        asked.rebuild = button(ui, Icon::Rebuild, "Rebuild", rebuild, false).clicked();
        separator(ui, theme::STATUS_SEPARATOR_MARGIN);
    }
    middle(ui, bar);
}

/// The middle: in sketch mode, the sketch's line; then the refusals said.
///
/// Outside sketch mode, the selection's words ([`selection_words`]) after them: a sketch's
/// line says what's being edited in their place.
fn middle(ui: &mut Ui, bar: &Bar) {
    if let Some(line) = &bar.sketch {
        sketch_line(ui, line);
    }
    for message in &bar.messages {
        item(ui, None, message, theme::ERROR_TEXT);
    }
    if bar.sketch.is_none() {
        for words in &bar.selection {
            item(ui, None, words, theme::STATUS_TEXT);
        }
    }
}

/// The sketch's line (`statusText`): `Editing` the sketch, a separator, the pointer's
/// coordinates on its plane (a dash off it) in an item at least
/// [`theme::STATUS_COORDINATES_WIDTH`] wide, so the state after it holds still as they
/// change, and, once the sketch is read, a separator and its state, in its colour.
fn sketch_line(ui: &mut Ui, line: &StatusLine) {
    item(ui, None, &line.editing, theme::STATUS_TEXT);
    separator(ui, 0.0);
    // SEAM: the prototype's figures are tabular (`font-variant-numeric: tabular-nums`), so
    // the coordinates' digits hold still as they change; egui lays out no OpenType
    // features, so Inter's proportional figures are drawn, within the least width.
    let coordinates = line.coordinates.as_deref().unwrap_or("—");
    let shown = item(ui, None, coordinates, theme::STATUS_TEXT);
    let short = theme::STATUS_COORDINATES_WIDTH - shown.rect.width();
    if short > 0.0 {
        ui.add_space(short);
    }
    if let Some(state) = &line.state {
        separator(ui, 0.0);
        item(ui, None, &state.words, state.colour);
    }
}

/// The right's parts, from the right: the shortcut sheet, the units, whether it's saved,
/// and the file on disk.
fn right_parts(ui: &mut Ui, bar: &Bar, asked: &mut Asked) {
    asked.shortcuts = shortcuts(ui).clicked();
    let Some(saved) = bar.saved else {
        return;
    };
    separator(ui, 0.0);
    item(ui, None, Units::LENGTH, theme::STATUS_TEXT);
    separator(ui, 0.0);
    let job = match saved {
        Saved::Saved => plain("Saved", theme::STATUS_TEXT),
        Saved::Unsaved => plain("Unsaved", theme::TEXT_STRONG),
        Saved::DialogOpen => {
            let mut job = plain("Unsaved", theme::TEXT_STRONG);
            job.append(" · dialog open", 0.0, format(theme::STATUS_TEXT));
            job
        }
    };
    item_job(ui, None, job);
    match bar.disk {
        Some(Disk::Changed) => {
            separator(ui, 0.0);
            asked.disk = chip(ui).clicked();
        }
        Some(Disk::Gone) => {
            separator(ui, 0.0);
            item(
                ui,
                Some(Mark::Icon(Icon::Error)),
                "File gone",
                theme::ERROR_TEXT,
            );
        }
        Some(Disk::Unreadable) => {
            separator(ui, 0.0);
            item(
                ui,
                Some(Mark::Icon(Icon::Error)),
                "File unreadable",
                theme::ERROR_TEXT,
            );
        }
        Some(Disk::Unchanged) | None => {}
    }
}

/// What an item shows before its words.
#[derive(Debug, Clone, Copy)]
enum Mark {
    /// A state's dot (`.state-dot`), in its colour.
    Dot(Color32),
    /// The spinner (`.spinner`).
    Spinner,
    /// An icon, in the words' colour.
    Icon(Icon),
}

impl Mark {
    fn width(self) -> f32 {
        match self {
            Mark::Dot(_) => theme::STATE_DOT_SIZE,
            Mark::Spinner => theme::SPINNER_SIZE,
            Mark::Icon(_) => IconSize::Small.points(),
        }
    }
}

/// The bar's text format, in `colour`.
fn format(colour: Color32) -> TextFormat {
    TextFormat {
        font_id: FontId::proportional(theme::HEADER_SIZE),
        color: colour,
        ..Default::default()
    }
}

/// `text` in the bar's type, in `colour`.
fn plain(text: &str, colour: Color32) -> LayoutJob {
    LayoutJob::single_section(text.to_owned(), format(colour))
}

/// An item (`.st-item`): its mark, if it has one, and `text` in `colour`.
fn item(ui: &mut Ui, mark: Option<Mark>, text: &str, colour: Color32) -> Response {
    item_job(ui, mark, plain(text, colour))
}

/// An item of `job`'s text, labelled for the accessibility tree with it.
fn item_job(ui: &mut Ui, mark: Option<Mark>, job: LayoutJob) -> Response {
    item_with(ui, mark, job, false)
}

/// [`item_job`], a button if `button` (`button.st-item`): Gray.300 under it and its words
/// and mark Gray.1000 while hovered.
fn item_with(ui: &mut Ui, mark: Option<Mark>, job: LayoutJob, button: bool) -> Response {
    let text = job.text.clone();
    let marked = mark.map_or(0.0, |mark| mark.width() + theme::STATUS_ITEM_GAP);
    let width = ui.painter().layout_job(job.clone()).size().x;
    let size = vec2(
        2.0 * theme::STATUS_ITEM_PADDING + marked + width,
        theme::STATUS_ITEM_HEIGHT,
    );
    let sense = if button {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let kind = if button {
        WidgetType::Button
    } else {
        WidgetType::Label
    };
    response.widget_info(|| WidgetInfo::labeled(kind, true, &text));
    let mut job = job;
    if button && response.hovered() {
        ui.painter().rect_filled(
            rect,
            theme::SMALL_CORNER_RADIUS,
            theme::STATUS_ITEM_HOVER_BG,
        );
        for section in &mut job.sections {
            section.format.color = theme::STATUS_ITEM_HOVER_TEXT;
        }
    }
    if button {
        theme::focus_ring(
            ui,
            &response,
            rect,
            theme::SMALL_CORNER_RADIUS,
            StrokeKind::Outside,
        );
    }
    let colour = job
        .sections
        .first()
        .map_or(theme::STATUS_TEXT, |section| section.format.color);
    let galley = ui.painter().layout_job(job);
    let left = rect.left() + theme::STATUS_ITEM_PADDING;
    if let Some(mark) = mark {
        paint_mark(
            ui,
            mark,
            pos2(left + mark.width() / 2.0, rect.center().y),
            colour,
        );
    }
    ui.painter().galley(
        pos2(left + marked, rect.center().y - galley.size().y / 2.0),
        galley,
        colour,
    );
    response
}

/// `mark` painted centred on `centre`, an icon in `colour`.
fn paint_mark(ui: &Ui, mark: Mark, centre: Pos2, colour: Color32) {
    match mark {
        Mark::Dot(fill) => {
            ui.painter()
                .circle_filled(centre, theme::STATE_DOT_SIZE / 2.0, fill);
        }
        Mark::Spinner => spinner(ui, centre),
        Mark::Icon(icon) => {
            let side = IconSize::Small.points();
            icon.image(IconSize::Small)
                .tint(colour)
                .paint_at(ui, Rect::from_center_size(centre, Vec2::splat(side)));
        }
    }
}

/// The spinner (`.spinner`): a ring, its top quarter turning clockwise with the frames that
/// come. It asks for none of its own: a rebuild held with nothing answering draws at the
/// idle frame's rate, never the display's (the timeline asks for frames while a step runs,
/// and turns it then).
pub(crate) fn spinner(ui: &Ui, centre: Pos2) {
    let radius = theme::SPINNER_SIZE / 2.0 - theme::SPINNER_WIDTH / 2.0;
    ui.painter().circle_stroke(
        centre,
        radius,
        Stroke::new(theme::SPINNER_WIDTH, theme::SPINNER_TRACK),
    );
    let time = ui.input(|input| input.time);
    let turns = (time / f64::from(theme::SPINNER_TURN_SECONDS)).fract() as f32;
    // The top border: from the upper left, 225 degrees round (y down), a quarter on.
    let start = (-135.0f32).to_radians() + turns * std::f32::consts::TAU;
    let points: Vec<Pos2> = (0..=16)
        .map(|n| {
            let angle = start + std::f32::consts::FRAC_PI_2 * n as f32 / 16.0;
            centre + vec2(libm::cosf(angle), libm::sinf(angle)) * radius
        })
        .collect();
    ui.painter().add(Shape::line(
        points,
        Stroke::new(theme::SPINNER_WIDTH, theme::SPINNER_HEAD),
    ));
}

/// The rebuild's progress (`.progress`): `done` of `steps`.
fn bar_of(ui: &mut Ui, done: usize, steps: usize) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(theme::STATUS_PROGRESS_WIDTH, theme::STATUS_PROGRESS_HEIGHT),
        Sense::hover(),
    );
    let corner = theme::STATUS_PROGRESS_CORNER_RADIUS;
    ui.painter()
        .rect_filled(rect, corner, theme::STATUS_PROGRESS_BG);
    let part = done.min(steps) as f32 / steps.max(1) as f32;
    if part > 0.0 {
        let full = Rect::from_min_size(rect.min, vec2(rect.width() * part, rect.height()));
        // `.progress { overflow: hidden }`: the bar clipped to the track's corners.
        ui.painter()
            .with_clip_rect(rect)
            .rect_filled(full, corner, theme::STATUS_PROGRESS_FILL);
    }
}

/// A separator (`.st-sep`), `margin` at each side.
fn separator(ui: &mut Ui, margin: f32) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            theme::BORDER_WIDTH + 2.0 * margin,
            theme::STATUS_SEPARATOR_HEIGHT,
        ),
        Sense::hover(),
    );
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(theme::BORDER_WIDTH, theme::STATUS_SEPARATOR),
    );
}

/// A button (`.st-btn`): `icon` and `text`, labelled `text`; Stop's colours while it's on
/// if `stop`. Off unless `enabled`.
fn button(ui: &mut Ui, icon: Icon, text: &str, enabled: bool, stop: bool) -> Response {
    // Painted at the bar's opacity, not faded further when off: `.st-btn:disabled` is
    // Gray.400 itself.
    let opacity = ui.opacity();
    ui.add_enabled(enabled, |ui: &mut Ui| {
        ui.set_opacity(opacity);
        let galley = ui.painter().layout_no_wrap(
            text.to_owned(),
            FontId::proportional(theme::HEADER_SIZE),
            theme::STATUS_BUTTON_TEXT,
        );
        let side = IconSize::Small.points();
        let content = side + theme::STATUS_BUTTON_GAP + galley.size().x;
        let size = vec2(
            content + 2.0 * theme::STATUS_BUTTON_PADDING,
            theme::STATUS_BUTTON_HEIGHT,
        );
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
        let hovered = response.hovered();
        let (fill, ink) = match (enabled, stop, hovered) {
            (false, _, _) => (Color32::TRANSPARENT, theme::STATUS_BUTTON_DISABLED_TEXT),
            (true, true, false) => (theme::STATUS_STOP_BG, theme::STATUS_STOP_TEXT),
            (true, true, true) => (theme::STATUS_STOP_HOVER_BG, theme::STATUS_STOP_HOVER_TEXT),
            (true, false, false) => (theme::STATUS_BUTTON_BG, theme::STATUS_BUTTON_TEXT),
            (true, false, true) => (
                theme::STATUS_BUTTON_HOVER_BG,
                theme::STATUS_BUTTON_HOVER_TEXT,
            ),
        };
        ui.painter()
            .rect_filled(rect, theme::SMALL_CORNER_RADIUS, fill);
        theme::focus_ring(
            ui,
            &response,
            rect,
            theme::SMALL_CORNER_RADIUS,
            StrokeKind::Outside,
        );
        let left = rect.left() + theme::STATUS_BUTTON_PADDING;
        icon.image(IconSize::Small).tint(ink).paint_at(
            ui,
            Rect::from_min_size(pos2(left, rect.center().y - side / 2.0), Vec2::splat(side)),
        );
        ui.painter().galley_with_override_text_color(
            pos2(
                left + side + theme::STATUS_BUTTON_GAP,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            ink,
        );
        response
    })
}

/// An item that's a button (`button.st-item`): `mark`, then `text` in `colour`, white on
/// Gray.300 hovered, then `caps` as key caps, if any. Off unless `enabled`, its mark and
/// words then Gray.400, as a `.st-btn:disabled`'s.
fn item_button(
    ui: &mut Ui,
    mark: Mark,
    text: &str,
    colour: Color32,
    caps: &[String],
    enabled: bool,
) -> Response {
    // Painted at the bar's opacity, not faded further when off.
    let opacity = ui.opacity();
    ui.add_enabled(enabled, |ui: &mut Ui| {
        ui.set_opacity(opacity);
        let galley = ui.painter().layout_job(plain(text, colour));
        let keys = if caps.is_empty() {
            0.0
        } else {
            theme::STATUS_ITEM_GAP + start::key_caps_width(ui, caps)
        };
        let size = vec2(
            2.0 * theme::STATUS_ITEM_PADDING
                + mark.width()
                + theme::STATUS_ITEM_GAP
                + galley.size().x
                + keys,
            theme::STATUS_ITEM_HEIGHT,
        );
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
        let ink = if !enabled {
            theme::STATUS_BUTTON_DISABLED_TEXT
        } else if response.hovered() {
            ui.painter().rect_filled(
                rect,
                theme::SMALL_CORNER_RADIUS,
                theme::STATUS_ITEM_HOVER_BG,
            );
            theme::STATUS_ITEM_HOVER_TEXT
        } else {
            colour
        };
        theme::focus_ring(
            ui,
            &response,
            rect,
            theme::SMALL_CORNER_RADIUS,
            StrokeKind::Outside,
        );
        let left = rect.left() + theme::STATUS_ITEM_PADDING;
        paint_mark(
            ui,
            mark,
            pos2(left + mark.width() / 2.0, rect.center().y),
            ink,
        );
        let at = pos2(
            left + mark.width() + theme::STATUS_ITEM_GAP,
            rect.center().y - galley.size().y / 2.0,
        );
        let words_right = at.x + galley.size().x;
        ui.painter()
            .galley_with_override_text_color(at, galley, ink);
        if !caps.is_empty() {
            start::key_caps(ui, pos2(words_right + keys, rect.center().y), caps, false);
        }
        response
    })
}

/// The Changed on disk chip (`button.st-item.warn-text`).
fn chip(ui: &mut Ui) -> Response {
    item_button(
        ui,
        Mark::Icon(Icon::Warn),
        "Changed on disk",
        theme::WARNING,
        &[],
        true,
    )
}

/// The shortcut sheet's button (`button.st-item`): the sheet's key (`?`, from the
/// registry) in a key cap after its words.
fn shortcuts(ui: &mut Ui) -> Response {
    let caps = Command::Shortcuts.caps(ui.ctx());
    item_button(
        ui,
        Mark::Icon(Icon::Help),
        "Shortcuts",
        theme::STATUS_TEXT,
        &caps,
        true,
    )
}

#[cfg(test)]
mod tests {
    // ---- The bar as drawn, against the prototype ----
    //
    // Each value below is typed from the approved prototype's stylesheet
    // (docs/design/m5-ui/styles.css: `:root`'s tokens, `#status`, `.st-item`,
    // `button.st-item:hover`, `.st-sep`, `.st-btn`, `.st-btn.stop`, `.st-btn:disabled`,
    // `.progress`, `.state-dot`, `.kbd`, `.ok-text`, `.warn-text`, `.err-text`, `.strong`) and
    // js/app.js's `renderStatus`, never from theme.rs, and compared with what the bar painted:
    // each rectangle, line, circle, icon, and text from egui's shapes, and each widget's
    // bounds from the accessibility tree.

    use eframe::egui::epaint::{CircleShape, RectShape, TextShape};
    use eframe::egui::{self, CornerRadius, FontFamily, FontId};
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;

    /// The prototype's tokens and geometry.
    mod css {
        use eframe::egui::Color32;

        // `:root`'s greys and colours.
        pub const G150: Color32 = Color32::from_rgb(0x17, 0x17, 0x17);
        pub const G250: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
        pub const G300: Color32 = Color32::from_rgb(0x2c, 0x2b, 0x2b);
        pub const G350: Color32 = Color32::from_rgb(0x52, 0x51, 0x51);
        pub const G400: Color32 = Color32::from_rgb(0x64, 0x63, 0x63);
        pub const G550: Color32 = Color32::from_rgb(0x93, 0x90, 0x90);
        pub const G800: Color32 = Color32::from_rgb(0xe0, 0xdf, 0xdf);
        pub const G1000: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
        /// `--green` (the Rebuilt dot), `--orange` (`.warn-text`), `--red` (`.err-text`).
        pub const GREEN: Color32 = Color32::from_rgb(0x00, 0xda, 0x7e);
        pub const ORANGE: Color32 = Color32::from_rgb(0xff, 0x7a, 0x0c);
        pub const RED: Color32 = Color32::from_rgb(0xff, 0x38, 0x65);
        /// `.st-btn.stop:not(:disabled)`: `background: #5c001a; color: #ffd1d6`.
        pub const STOP_BG: Color32 = Color32::from_rgb(0x5c, 0x00, 0x1a);
        pub const STOP_TEXT: Color32 = Color32::from_rgb(0xff, 0xd1, 0xd6);

        /// `--status-h`, and `#status`'s `border-top: 1px` within it (`border-box`).
        pub const STATUS_H: f32 = 24.0;
        pub const BORDER: f32 = 1.0;
        /// `#status`: `padding: 0 6px 0 8px; gap: 2px; font-size: var(--fs-small)`.
        pub const PADDING_LEFT: f32 = 8.0;
        pub const PADDING_RIGHT: f32 = 6.0;
        pub const GAP: f32 = 2.0;
        pub const FS_SMALL: f32 = 11.0;
        /// `.st-item`: `gap: 6px; padding: 0 8px; height: 20px`.
        pub const ITEM_GAP: f32 = 6.0;
        pub const ITEM_PADDING: f32 = 8.0;
        pub const ITEM_H: f32 = 20.0;
        /// `--icon`, `--r-sm`.
        pub const ICON: f32 = 14.0;
        pub const R_SM: u8 = 4;
        /// `.st-sep`: 1 by 12; the one after the buttons `margin: 0 6px`.
        pub const SEP_H: f32 = 12.0;
        pub const SEP_MARGIN: f32 = 6.0;
        /// `.st-btn`: `height: 18px; padding: 0 7px; gap: 5px`.
        pub const BTN_H: f32 = 18.0;
        pub const BTN_PADDING: f32 = 7.0;
        pub const BTN_GAP: f32 = 5.0;
        /// `.progress`: 70 by 4, `border-radius: 2px`.
        pub const PROGRESS_W: f32 = 70.0;
        pub const PROGRESS_H: f32 = 4.0;
        pub const PROGRESS_R: u8 = 2;
        /// `.state-dot`: 8 by 8, round.
        pub const DOT: f32 = 8.0;
        /// `.kbd`: `min-width: 16px; height: 16px; border-radius: 3px`, Gray.250 on Gray.350,
        /// its border 1 and 2 at its bottom (`border-bottom-width: 2px`).
        pub const KBD_H: f32 = 16.0;
        pub const KBD_MIN_W: f32 = 16.0;
        pub const KBD_R: u8 = 3;
    }

    /// The bar's place in the harness's window: away from the origin, so a sum with its
    /// corner never reads as a product.
    const BAR: Rect = Rect::from_min_max(Pos2::new(40.0, 30.0), Pos2::new(1000.0, 54.0));

    /// The middle of what's under the bar's border, where its parts are centred.
    fn middle() -> f32 {
        BAR.top() + css::BORDER + (css::STATUS_H - css::BORDER) / 2.0
    }

    fn drawn(shown: Bar) -> Harness<'static, Bar> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(BAR.right() + 40.0, BAR.bottom() + 40.0))
            .build_ui_state(
                |ui, shown: &mut Bar| {
                    ui.scope_builder(UiBuilder::new().max_rect(BAR), |ui| {
                        bar(ui, shown);
                    });
                },
                shown,
            );
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    fn painted<S>(h: &Harness<'_, S>) -> Vec<Shape> {
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

    fn rects<S>(h: &Harness<'_, S>) -> Vec<RectShape> {
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

    /// A text's colour as painted: its override, else its first section's.
    fn colour(shown: &TextShape) -> Color32 {
        shown
            .override_text_color
            .unwrap_or(shown.galley.job.sections[0].format.color)
    }

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

    /// The vertical lines painted at `x`: their ends and stroke.
    fn vlines<S>(h: &Harness<'_, S>, x: f32) -> Vec<([Pos2; 2], Stroke)> {
        painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::LineSegment { points, stroke } if near(points[0].x, x) => {
                    Some((points, Stroke::new(stroke.width, stroke.color)))
                }
                _ => None,
            })
            .collect()
    }

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

    #[track_caller]
    fn sized(got: Rect, want: Rect, what: &str) {
        assert!(near_rect(got, want), "{what}: {got:?}, not {want:?}");
    }

    fn widget<S>(h: &Harness<'_, S>, role: Role, label: &str) -> Rect {
        h.get_by_role_and_label(role, label).rect()
    }

    /// `words` drawn from `left`, centred on the bar's middle, `--fs-small`, in `ink`: the
    /// text's right edge.
    #[track_caller]
    fn said_at<S>(h: &Harness<'_, S>, words: &str, left: f32, ink: Color32) -> f32 {
        let shown = text(h, words);
        let size = shown.galley.size();
        at(shown.pos, Pos2::new(left, middle() - size.y / 2.0), words);
        assert_eq!(
            shown.galley.job.sections[0].format.font_id,
            FontId::new(css::FS_SMALL, FontFamily::Proportional),
            "{words}"
        );
        assert_eq!(colour(&shown), ink, "{words}'s ink");
        left + size.x
    }

    /// A box `width` wide and `height` high from `left`, centred on the bar's middle.
    fn centred(left: f32, width: f32, height: f32) -> Rect {
        Rect::from_min_size(
            Pos2::new(left, middle() - height / 2.0),
            Vec2::new(width, height),
        )
    }

    /// A button (`.st-btn`) from `left` labelled `label`, its icon and words in `ink` on
    /// `fill` (none when off): its right edge.
    #[track_caller]
    fn button_at<S>(
        h: &Harness<'_, S>,
        label: &str,
        left: f32,
        ink: Color32,
        fill: Option<Color32>,
    ) -> f32 {
        let icon = centred(left + css::BTN_PADDING, css::ICON, css::ICON);
        assert_eq!(icon_at(h, icon), Some(ink), "{label}'s icon");
        let right = said_at(h, label, icon.right() + css::BTN_GAP, ink) + css::BTN_PADDING;
        let rect = Rect::from_min_max(
            Pos2::new(left, middle() - css::BTN_H / 2.0),
            Pos2::new(right, middle() + css::BTN_H / 2.0),
        );
        sized(widget(h, Role::Button, label), rect, label);
        match fill {
            Some(fill) => assert_eq!(
                fills_at(h, rect),
                [(fill, CornerRadius::same(css::R_SM))],
                "{label}'s fill"
            ),
            None => assert!(fills_at(h, rect).is_empty(), "{label} filled"),
        }
        right
    }

    /// A separator (`.st-sep`) from `left`, `margin` at each side: its right edge.
    #[track_caller]
    fn separator_at<S>(h: &Harness<'_, S>, left: f32, margin: f32) -> f32 {
        let x = left + margin + css::BORDER / 2.0;
        let lines = vlines(h, x);
        assert_eq!(
            lines,
            [(
                [
                    Pos2::new(x, middle() - css::SEP_H / 2.0),
                    Pos2::new(x, middle() + css::SEP_H / 2.0)
                ],
                Stroke::new(css::BORDER, css::G250)
            )],
            "the separator at {x}"
        );
        left + 2.0 * margin + css::BORDER
    }

    fn bar_of(said: Said, buttons: (bool, bool), saved: Saved, disk: Option<Disk>) -> Bar {
        Bar {
            said,
            buttons: Some(buttons),
            messages: Vec::new(),
            selection: Vec::new(),
            disk,
            saved: Some(saved),
            sketch: None,
        }
    }

    /// The bar of `said` drawn, what it asked kept (each frame's added).
    fn asking(said: Said) -> Harness<'static, (Bar, Asked)> {
        let shown = bar_of(said, (false, false), Saved::Saved, None);
        let mut h = Harness::builder()
            .with_size(Vec2::new(BAR.right() + 40.0, BAR.bottom() + 40.0))
            .build_ui_state(
                |ui, (shown, asked): &mut (Bar, Asked)| {
                    ui.scope_builder(UiBuilder::new().max_rect(BAR), |ui| {
                        asked.whats_wrong |= bar(ui, shown).whats_wrong;
                    });
                },
                (shown, Asked::default()),
            );
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    #[test]
    fn rebuilt_with_errors_is_a_button_that_opens_whats_wrong() {
        // The prototype's `button.st-item.err-text` (`data-act="whatsWrong"`).
        let mut h = asking(Said::Rebuilt {
            revision: 4,
            errors: 2,
        });
        assert!(!h.state().1.whats_wrong);
        h.get_by_role_and_label(Role::Button, "Rebuilt with 2 errors")
            .click();
        h.run_steps(2);
        assert!(h.state().1.whats_wrong);
        // Rebuilt with none, and a fault: words alone.
        for said in [
            Said::Rebuilt {
                revision: 4,
                errors: 0,
            },
            Said::Faulted,
        ] {
            let words = said.words();
            let h = asking(said);
            assert!(h.query_by_role_and_label(Role::Label, &words).is_some());
            assert!(h.query_by_role_and_label(Role::Button, &words).is_none());
        }
    }

    #[test]
    fn in_sketch_mode_the_middle_is_editing_the_pointers_coordinates_and_the_state() {
        use crate::sketch::bar::StateShown;
        // `statusText` (js/sketch.js): `.st-item` Editing, `.st-sep`, `.st-item` the
        // coordinates, `min-width: 150px`, `.st-sep`, and `.st-item` the state in its colour;
        // after the separator Stop and Rebuild end with, each part 2 from the last.
        let under = Color32::from_rgb(0x2e, 0x9c, 0xff);
        let mut shown = bar_of(
            Said::Rebuilt {
                revision: 3,
                errors: 0,
            },
            (false, false),
            Saved::Unsaved,
            None,
        );
        shown.messages = vec!["The sketch couldn't be written".to_owned()];
        // The model's selection, which the sketch's line stands in place of.
        shown.selection = vec!["2 selected".to_owned()];
        shown.sketch = Some(StatusLine {
            editing: "Editing Sketch1".to_owned(),
            coordinates: Some("x 12.500 · y -3.000 mm".to_owned()),
            state: Some(StateShown {
                words: "Under defined · 6 DOF".to_owned(),
                colour: under,
            }),
        });
        let mut h = drawn(shown);
        let after_buttons = |h: &Harness<'_, Bar>| {
            let rebuild = widget(h, Role::Button, "Rebuild");
            rebuild.right() + css::GAP + css::SEP_MARGIN + css::BORDER + css::SEP_MARGIN + css::GAP
        };
        let left = after_buttons(&h);
        let editing = said_at(&h, "Editing Sketch1", left + css::ITEM_PADDING, css::G550);
        let sep = editing + css::ITEM_PADDING + css::GAP;
        let coordinates = separator_at(&h, sep, 0.0) + css::GAP;
        let words = said_at(
            &h,
            "x 12.500 · y -3.000 mm",
            coordinates + css::ITEM_PADDING,
            css::G550,
        );
        // The coordinates' item no narrower than 150, however short its words.
        let item = (words + css::ITEM_PADDING - coordinates).max(150.0);
        let sep = coordinates + item + css::GAP;
        let state = separator_at(&h, sep, 0.0) + css::GAP;
        let state_words = said_at(
            &h,
            "Under defined · 6 DOF",
            state + css::ITEM_PADDING,
            under,
        );
        // Then the refusals said, after the sketch's line.
        said_at(
            &h,
            "The sketch couldn't be written",
            state_words + css::ITEM_PADDING + css::GAP + css::ITEM_PADDING,
            css::RED,
        );
        // The selection's words aren't said while the sketch's line is.
        assert!(!painted(&h).into_iter().any(
            |shape| matches!(shape, Shape::Text(shown) if shown.galley.text() == "2 selected")
        ));
        // Off the canvas, a dash in the same 150; before the sketch is read, no state and
        // no separator for it.
        if let Some(line) = h.state_mut().sketch.as_mut() {
            line.coordinates = None;
            line.state = None;
        }
        h.run_steps(2);
        let editing = said_at(&h, "Editing Sketch1", left + css::ITEM_PADDING, css::G550);
        let coordinates = separator_at(&h, editing + css::ITEM_PADDING + css::GAP, 0.0) + css::GAP;
        said_at(&h, "—", coordinates + css::ITEM_PADDING, css::G550);
        assert!(vlines(&h, coordinates + 150.0 + css::GAP + css::BORDER / 2.0).is_empty());
        said_at(
            &h,
            "The sketch couldn't be written",
            coordinates + 150.0 + css::GAP + css::ITEM_PADDING,
            css::RED,
        );
        // Outside sketch mode, none of it.
        h.state_mut().sketch = None;
        h.run_steps(2);
        let painted_text = |h: &Harness<'_, Bar>, words: &str| {
            painted(h)
                .into_iter()
                .any(|shape| matches!(shape, Shape::Text(shown) if shown.galley.text() == words))
        };
        assert!(!painted_text(&h, "Editing Sketch1"));
        assert!(!painted_text(&h, "—"));
        let refusal = said_at(
            &h,
            "The sketch couldn't be written",
            after_buttons(&h) + css::ITEM_PADDING,
            css::RED,
        );
        // And the selection's words after the refusals, as they are with no sketch open.
        said_at(
            &h,
            "2 selected",
            refusal + css::ITEM_PADDING + css::GAP + css::ITEM_PADDING,
            css::G550,
        );
    }

    #[test]
    fn the_bar_is_drawn_as_status_with_its_items_buttons_separators_and_chip() {
        let h = drawn(bar_of(
            Said::Rebuilt {
                revision: 12,
                errors: 0,
            },
            (false, false),
            Saved::Saved,
            Some(Disk::Changed),
        ));
        // `#status`: Gray.150 across the whole 24, a 1 px Gray.250 border along its top.
        assert_eq!(fills_at(&h, BAR), [(css::G150, CornerRadius::ZERO)]);
        let border = painted(&h).into_iter().any(|shape| {
            matches!(shape, Shape::LineSegment { points, stroke }
                if near(points[0].y, BAR.top() + css::BORDER / 2.0)
                    && near(points[1].y, BAR.top() + css::BORDER / 2.0)
                    && near(points[0].x, BAR.left())
                    && near(points[1].x, BAR.right())
                    && stroke.width == css::BORDER
                    && stroke.color == css::G250)
        });
        assert!(border, "the border along its top");
        // The rebuilt item (`.st-item`): its green dot 8 across at its padding, then its
        // words 6 after, in the bar's Gray.550.
        let item = BAR.left() + css::PADDING_LEFT;
        let dots: Vec<CircleShape> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Circle(circle) => Some(circle),
                _ => None,
            })
            .collect();
        assert_eq!(dots.len(), 1);
        at(
            dots[0].center,
            Pos2::new(item + css::ITEM_PADDING + css::DOT / 2.0, middle()),
            "the dot",
        );
        assert_eq!((dots[0].radius, dots[0].fill), (css::DOT / 2.0, css::GREEN));
        let words = said_at(
            &h,
            "Rebuilt · revision 12",
            item + css::ITEM_PADDING + css::DOT + css::ITEM_GAP,
            css::G550,
        );
        let item_rect = Rect::from_min_max(
            Pos2::new(item, middle() - css::ITEM_H / 2.0),
            Pos2::new(words + css::ITEM_PADDING, middle() + css::ITEM_H / 2.0),
        );
        sized(
            widget(&h, Role::Label, "Rebuilt · revision 12"),
            item_rect,
            "the item",
        );
        // Stop and Rebuild, off (`.st-btn:disabled`: no fill, Gray.400), 2 apart, then the
        // separator with its 6 of margin.
        let stop = button_at(&h, "Stop", item_rect.right() + css::GAP, css::G400, None);
        let rebuild = button_at(&h, "Rebuild", stop + css::GAP, css::G400, None);
        separator_at(&h, rebuild + css::GAP, css::SEP_MARGIN);
        // From the right, 6 in: Shortcuts (`button.st-item`: help, its words, `?` in a key
        // cap 6 after; on, in the bar's Gray.550), a separator, mm, a separator, Saved, a
        // separator, and the chip.
        let shortcuts = widget(&h, Role::Button, "Shortcuts");
        assert!(
            !h.get_by_role_and_label(Role::Button, "Shortcuts")
                .accesskit_node()
                .is_disabled()
        );
        assert!(near(shortcuts.right(), BAR.right() - css::PADDING_RIGHT));
        assert!(near(shortcuts.height(), css::ITEM_H));
        let help = centred(shortcuts.left() + css::ITEM_PADDING, css::ICON, css::ICON);
        assert_eq!(icon_at(&h, help), Some(css::G550));
        let words = said_at(&h, "Shortcuts", help.right() + css::ITEM_GAP, css::G550);
        let caps: Vec<RectShape> = rects(&h)
            .into_iter()
            .filter(|shape| {
                near(shape.rect.left(), words + css::ITEM_GAP)
                    && near(shape.rect.height(), css::KBD_H)
            })
            .collect();
        assert_eq!(caps.len(), 1, "the ? cap");
        assert!(caps[0].rect.width() >= css::KBD_MIN_W);
        assert!(near(
            caps[0].rect.right(),
            shortcuts.right() - css::ITEM_PADDING
        ));
        assert_eq!(
            (caps[0].fill, caps[0].corner_radius),
            (css::G250, CornerRadius::same(css::KBD_R))
        );
        assert!(caps[0].stroke.is_empty(), "the ? cap: {:?}", caps[0].stroke);
        crate::modal::tests::kbd_border(
            &crate::modal::tests::cap_border(&h, caps[0].rect),
            caps[0].rect,
            css::G350,
            "the ? cap",
        );
        let sep = shortcuts.left() - css::GAP - css::BORDER;
        separator_at(&h, sep, 0.0);
        let mm = widget(&h, Role::Label, "mm");
        assert!(near(mm.right(), sep - css::GAP));
        said_at(&h, "mm", mm.left() + css::ITEM_PADDING, css::G550);
        let sep = mm.left() - css::GAP - css::BORDER;
        separator_at(&h, sep, 0.0);
        let saved = widget(&h, Role::Label, "Saved");
        assert!(near(saved.right(), sep - css::GAP));
        said_at(&h, "Saved", saved.left() + css::ITEM_PADDING, css::G550);
        let sep = saved.left() - css::GAP - css::BORDER;
        separator_at(&h, sep, 0.0);
        // The chip (`button.st-item.warn-text`): its warning icon and words orange.
        let chip = widget(&h, Role::Button, "Changed on disk");
        assert!(near(chip.right(), sep - css::GAP));
        sized(
            chip,
            centred(chip.left(), chip.width(), css::ITEM_H),
            "the chip",
        );
        let warn = centred(chip.left() + css::ITEM_PADDING, css::ICON, css::ICON);
        assert_eq!(icon_at(&h, warn), Some(css::ORANGE));
        let end = said_at(
            &h,
            "Changed on disk",
            warn.right() + css::ITEM_GAP,
            css::ORANGE,
        );
        assert!(near(end + css::ITEM_PADDING, chip.right()));
    }

    #[test]
    fn a_rebuild_running_stopped_or_failed_is_drawn_in_its_states_colours() {
        // Running: its words strong (`.st-item.strong`), the spinner's ring, the progress
        // bar 2 after the item, 3 of 7 of its 70 filled Gray.800 on Gray.300; Stop on in
        // its own colours; Unsaved strong.
        let h = drawn(bar_of(
            Said::Rebuilding {
                preview: false,
                progress: Some((3, 7)),
            },
            (true, false),
            Saved::Unsaved,
            None,
        ));
        let item = BAR.left() + css::PADDING_LEFT;
        let words = said_at(
            &h,
            "Rebuilding · 3 of 7",
            item + css::ITEM_PADDING + 11.0 + css::ITEM_GAP,
            css::G1000,
        );
        let ring = painted(&h).into_iter().find_map(|shape| match shape {
            Shape::Circle(circle) => Some(circle),
            _ => None,
        });
        // `.spinner`: 11 across, its 1.5 border Gray.350, inside its box.
        let ring = ring.unwrap();
        at(
            ring.center,
            Pos2::new(item + css::ITEM_PADDING + 11.0 / 2.0, middle()),
            "the spinner",
        );
        assert_eq!(
            (ring.radius, ring.stroke.width, ring.stroke.color),
            (11.0 / 2.0 - 1.5 / 2.0, 1.5, css::G350)
        );
        let track = centred(
            words + css::ITEM_PADDING + css::GAP,
            css::PROGRESS_W,
            css::PROGRESS_H,
        );
        assert_eq!(
            fills_at(&h, track),
            [(css::G300, CornerRadius::same(css::PROGRESS_R))]
        );
        let done = Rect::from_min_size(
            track.min,
            Vec2::new(css::PROGRESS_W * 3.0 / 7.0, css::PROGRESS_H),
        );
        assert_eq!(
            fills_at(&h, done),
            [(css::G800, CornerRadius::same(css::PROGRESS_R))]
        );
        let stop = button_at(
            &h,
            "Stop",
            track.right() + css::GAP,
            css::STOP_TEXT,
            Some(css::STOP_BG),
        );
        button_at(&h, "Rebuild", stop + css::GAP, css::G400, None);
        let unsaved = text(&h, "Unsaved");
        assert_eq!(colour(&unsaved), css::G1000);

        // Stopped: `.warn-text`, its stop icon orange; Rebuild on (`.st-btn`: Gray.800 on
        // Gray.300).
        let h = drawn(bar_of(
            Said::Stopped(Some((2, 7))),
            (false, true),
            Saved::Saved,
            None,
        ));
        let icon = centred(item + css::ITEM_PADDING, css::ICON, css::ICON);
        assert_eq!(icon_at(&h, icon), Some(css::ORANGE));
        let words = said_at(
            &h,
            "Rebuild stopped at 2 of 7",
            icon.right() + css::ITEM_GAP,
            css::ORANGE,
        );
        let stop = button_at(
            &h,
            "Stop",
            words + css::ITEM_PADDING + css::GAP,
            css::G400,
            None,
        );
        button_at(&h, "Rebuild", stop + css::GAP, css::G800, Some(css::G300));

        // Rebuilt with errors: `.err-text`, its error icon red.
        let h = drawn(bar_of(
            Said::Rebuilt {
                revision: 4,
                errors: 2,
            },
            (false, false),
            Saved::Saved,
            Some(Disk::Gone),
        ));
        assert_eq!(icon_at(&h, icon), Some(css::RED));
        said_at(
            &h,
            "Rebuilt with 2 errors",
            icon.right() + css::ITEM_GAP,
            css::RED,
        );
        // A file gone: said in red, no chip.
        assert_eq!(colour(&text(&h, "File gone")), css::RED);
        assert!(h.query_by_label("Changed on disk").is_none());
    }

    #[test]
    fn the_words_are_the_prototypes() {
        let rebuilding = |preview, progress| Said::Rebuilding { preview, progress }.words();
        assert_eq!(rebuilding(false, Some((3, 7))), "Rebuilding · 3 of 7");
        assert_eq!(
            rebuilding(true, Some((0, 2))),
            "Rebuilding the preview · 0 of 2"
        );
        assert_eq!(rebuilding(false, None), "Rebuilding");
        assert_eq!(
            Said::Stopped(Some((1, 4))).words(),
            "Rebuild stopped at 1 of 4"
        );
        assert_eq!(Said::Stopped(None).words(), "Rebuild stopped");
        let rebuilt = |errors| {
            Said::Rebuilt {
                revision: 9,
                errors,
            }
            .words()
        };
        assert_eq!(rebuilt(0), "Rebuilt · revision 9");
        assert_eq!(rebuilt(1), "Rebuilt with 1 error");
        assert_eq!(rebuilt(3), "Rebuilt with 3 errors");
        assert_eq!(Said::Nothing.words(), "");
    }
}
