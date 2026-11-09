//! Sketch mode's own panels in the canvas, as the approved prototype lays them out
//! (docs/design/m5-ui/, `js/sketch.js`'s `renderCanvasUI` and `statusText`): the sketch bar at
//! the canvas's top (the sketch, its plane, its state, Cancel, and Finish Sketch), the
//! Sketch Palette at its right, and what the status bar says of the sketch. Each is laid
//! out by a small function of its own, given what it shows and giving back what was asked,
//! apart from sketch mode's state and actions.

use eframe::egui::{
    self, Color32, CornerRadius, Margin, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2,
};
use silico_core::sketch_tools::ToolKind;

use super::style::*;
use super::tools::{Choice, TrimMode};
use crate::shortcuts::Command;

/// The palette's layout: its title row's height; a section's heading's text size and its
/// small text's; and a state's dot's radius, in points.
const TITLE_HEIGHT: f32 = 26.0;
const HEAD_SIZE: f32 = 10.0;
const SMALL_SIZE: f32 = 11.0;
const DOT_RADIUS: f32 = 4.0;
/// The palette's border's width, inside its width as the prototype's `box-sizing` has it.
const BORDER: f32 = 1.0;

/// What the sketch bar was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarAction {
    None,
    Finish,
    Cancel,
}

/// A sketch's state, as sketch mode shows it: its words and its colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateShown {
    pub words: String,
    pub colour: Color32,
}

/// What the bar shows: the sketch's name, its plane's, and its state (none until read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarView {
    pub name: String,
    pub plane: String,
    pub state: Option<StateShown>,
    /// Off while the tab closes.
    pub enabled: bool,
}

/// The sketch bar (the prototype's `.sketch-bar`): a tag, the sketch and its plane, its
/// state, Cancel, and Finish Sketch with its key.
pub fn sketch_bar(ui: &mut Ui, view: &BarView) -> BarAction {
    let mut action = BarAction::None;
    dark(ui);
    let (fill, accent, strong, weak) = (FLOATING_BG, SELECTION_BG, TEXT_STRONG, TEXT_SUBDUED);
    // One row, whatever room the canvas leaves: its words are never wrapped away.
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    let shown = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, accent))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin {
            left: 10,
            right: 4,
            top: 4,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Toolbar);
                node.set_label("Sketch bar");
            });
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(accent)
                    .corner_radius(CornerRadius::same(3))
                    .inner_margin(Margin::symmetric(6, 1))
                    .show(ui, |ui| {
                        ui.label(RichText::new("SKETCH").size(HEAD_SIZE).color(ON_SELECTION));
                    });
                ui.label(RichText::new(&view.name).color(strong));
                ui.label(RichText::new(format!("on {}", view.plane)).color(weak));
                if let Some(state) = &view.state {
                    state_dot(ui, state.colour);
                    ui.label(RichText::new(&state.words).color(state.colour));
                }
                if ui
                    .add_enabled(
                        view.enabled,
                        egui::Button::new("Cancel").fill(Color32::TRANSPARENT),
                    )
                    .clicked()
                {
                    action = BarAction::Cancel;
                }
                // Its name, keys, and line the registry's, as the toolbar's Finish Sketch
                // and the shortcut sheet give them.
                let command = Command::FinishSketch;
                let finish = egui::Button::new(RichText::new(command.label()).color(ON_SELECTION))
                    .fill(accent);
                let response = ui.add_enabled(view.enabled, finish);
                let keys = command.shown(ui.ctx());
                let line = command.entry().description;
                if crate::bar::tooltip(response, command.label(), keys, line).clicked() {
                    action = BarAction::Finish;
                }
            });
            ui.unique_id()
        });
    bounded(ui, shown.inner, shown.response.rect);
    action
}

/// The group `group`'s node (a ui's, which egui gives no bounds of its own) bounded by
/// `rect`, its frame's, in points as egui bounds its widgets: so the accessibility tree says
/// where the group is, around what it holds.
fn bounded(ui: &Ui, group: egui::Id, rect: egui::Rect) {
    ui.ctx().accesskit_node_builder(group, |node| {
        node.set_bounds(crate::panes::bounds(rect));
    });
}

/// The dark look the bar and the palette are drawn in, whatever theme the window shows:
/// egui's dark visuals, their widgets' text in the dark tokens, so text stands out from the
/// dark panels the prototype draws them as (a light theme's dark text would vanish there).
pub fn panel_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = FLOATING_BG;
    visuals.panel_fill = FLOATING_BG;
    visuals.window_stroke = Stroke::new(1.0, PANEL_BORDER);
    visuals.selection.bg_fill = SELECTION_BG;
    visuals.selection.stroke = Stroke::new(1.0, ON_SELECTION);
    visuals.error_fg_color = ERROR_TEXT;
    visuals.widgets.noninteractive.fg_stroke.color = TEXT_DEFAULT;
    visuals.widgets.inactive.fg_stroke.color = TEXT_DEFAULT;
    visuals.widgets.hovered.fg_stroke.color = TEXT_STRONG;
    visuals.widgets.active.fg_stroke.color = TEXT_STRONG;
    visuals.widgets.open.fg_stroke.color = TEXT_DEFAULT;
    visuals
}

/// `ui` and what's laid out in it drawn in [`panel_visuals`].
fn dark(ui: &mut Ui) {
    *ui.visuals_mut() = panel_visuals();
}

/// The palette's settings: what's shown; Construction, the drawing tools' curves drawn as
/// construction; *Automatic relations*, the relations the pointer shows added; and how trim
/// trims.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub show_profile: bool,
    pub show_points: bool,
    pub show_dimensions: bool,
    pub show_constraints: bool,
    pub construction: bool,
    pub automatic_relations: bool,
    pub trim: TrimMode,
}

impl Default for Settings {
    /// Everything shown, constraints among it (fusion.md, *Sketch mode*), and *Automatic
    /// relations* on (docs/ux/solidworks.md, §2).
    fn default() -> Self {
        Settings {
            show_profile: true,
            show_points: true,
            show_dimensions: true,
            show_constraints: true,
            construction: false,
            automatic_relations: true,
            trim: TrimMode::Power,
        }
    }
}

/// What the palette shows of Smart Dimension while it's on: how many are picked, whether a
/// right-click locked the kind, and why nothing is offered for what's picked, if nothing is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DimensionShown {
    pub picks: usize,
    pub locked: bool,
    pub unavailable: Option<String>,
}

/// What the palette shows: the sketch's state, and a word on what it means; the tool on,
/// and what it last said; Smart Dimension's picks and lock; whether a drag is under way;
/// the read's refusal, as said; and why the sketch's face wasn't found (#347), with its
/// suggestion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteView {
    pub state: Option<StateShown>,
    pub hint: Option<&'static str>,
    pub tool: Option<Choice>,
    pub tool_said: Option<String>,
    pub dimension: Option<DimensionShown>,
    pub dragging: bool,
    pub said: Option<String>,
    pub plane_problem: Option<(String, Option<String>)>,
}

/// A tool's name, as the palette and the toolbar say it.
pub fn tool_name(tool: ToolKind) -> &'static str {
    match tool {
        ToolKind::Line => "Line",
        ToolKind::CornerRectangle => "Corner Rectangle",
        ToolKind::CenterRectangle => "Center Rectangle",
        ToolKind::Circle => "Circle",
        ToolKind::CenterpointArc => "Center Point Arc",
        ToolKind::ThreePointArc => "3-Point Arc",
        ToolKind::TangentArc => "Tangent Arc",
        ToolKind::Point => "Point",
    }
}

/// A choice's name, as the palette says it.
pub fn choice_name(choice: Choice) -> &'static str {
    match choice {
        Choice::Draw(tool) => tool_name(tool),
        Choice::Dimension => "Smart Dimension",
        Choice::Trim => "Trim",
        Choice::Split => "Split",
    }
}

/// How a tool is used, as the palette says it (the prototype's words for the line; the
/// help's for the rest, docs/ux/solidworks.md, §4).
fn usage(choice: Choice, trim: TrimMode) -> &'static str {
    match (choice, trim) {
        (Choice::Draw(ToolKind::Line), _) => {
            "Click, click: a chain. Double-click or Enter ends it. Drag: one line."
        }
        (Choice::Draw(ToolKind::CornerRectangle), _) => {
            "Drag from corner to corner, or click each. Shift: a square."
        }
        (Choice::Draw(ToolKind::CenterRectangle), _) => {
            "Click the centre, then a corner, or drag from one to the other."
        }
        (Choice::Draw(ToolKind::Circle), _) => "Click the centre, then a point on the circle.",
        (Choice::Draw(ToolKind::CenterpointArc), _) => {
            "Click the centre, then the start, then the end."
        }
        (Choice::Draw(ToolKind::ThreePointArc), _) => {
            "Click the start, then the end, then a point on the arc."
        }
        (Choice::Draw(ToolKind::TangentArc), _) => "Press on the end of a line or an arc and drag.",
        (Choice::Draw(ToolKind::Point), _) => "Click to place a point.",
        (Choice::Trim, TrimMode::Power) => {
            "Drag across curves to trim them. Drag along one to extend it."
        }
        (Choice::Trim, TrimMode::Closest) => {
            "Click a curve to trim it to its closest cuts. Drag from one to extend it."
        }
        (Choice::Split, _) => "Click a curve to split it there. A circle takes two clicks.",
        (Choice::Dimension, _) => {
            "Click a line, a circle, or an arc, or two points or lines, then where the dimension goes. Right-click locks its kind. Esc undoes the last pick."
        }
    }
}

/// The Sketch Palette (the prototype's `.palette`): its title, then Status, Tool, Show,
/// and the colours' legend, each a section; Look At off until the viewport's camera. At
/// most `max_height` tall: its title stays, and its sections scroll beneath it when they
/// don't fit.
pub fn palette(ui: &mut Ui, view: &PaletteView, settings: &mut Settings, max_height: f32) {
    dark(ui);
    let shown = egui::Frame::new()
        .fill(FLOATING_BG)
        .stroke(Stroke::new(BORDER, PANEL_BORDER))
        .corner_radius(CornerRadius::same(6))
        .shadow(egui::Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(140),
        })
        .show(ui, |ui| {
            // The prototype's width, its border inside it.
            let width = SKETCH_PALETTE_WIDTH - 2.0 * BORDER;
            ui.set_width(width);
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Group);
                node.set_label("Sketch Palette");
            });
            ui.spacing_mut().item_spacing.y = 2.0;
            let (title, _) = ui.allocate_exact_size(Vec2::new(width, TITLE_HEIGHT), Sense::hover());
            ui.put(
                title.shrink2(Vec2::new(10.0, 0.0)),
                egui::Label::new(
                    RichText::new("SKETCH PALETTE")
                        .size(SMALL_SIZE)
                        .color(TEXT_STRONG),
                ),
            );
            // What's left beneath the title and the space after it, and inside the border:
            // at least nothing.
            let spacing = ui.spacing().item_spacing.y;
            let body = (max_height - 2.0 * BORDER - TITLE_HEIGHT - spacing).max(0.0);
            egui::ScrollArea::vertical()
                .id_salt("sketch_palette_body")
                .max_height(body)
                // Its height the cap's whenever its sections pass it, not the area's last
                // (an area's room is its size the frame before, so it would never grow).
                .min_scrolled_height(body)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    section(ui, "Status", |ui| status_section(ui, view));
                    section(ui, "Tool", |ui| tool_section(ui, view, settings));
                    section(ui, "Show", |ui| {
                        ui.checkbox(&mut settings.show_profile, "Profile");
                        ui.checkbox(&mut settings.show_points, "Points");
                        ui.checkbox(&mut settings.show_dimensions, "Dimensions");
                        ui.checkbox(&mut settings.show_constraints, "Constraints");
                        ui.add_enabled(
                            false,
                            egui::Button::new("Look At").fill(Color32::TRANSPARENT),
                        );
                    });
                    section(ui, "Colours", legend);
                });
            ui.unique_id()
        });
    bounded(ui, shown.inner, shown.response.rect);
}

/// The palette's Tool: the tool on, how it's used, and what it last said, or the keys that
/// choose one; trim's two ways; and the drawing tools' settings, *Automatic relations*
/// (Ctrl held does as it off, for one click) and Construction.
fn tool_section(ui: &mut Ui, view: &PaletteView, settings: &mut Settings) {
    match view.tool {
        Some(choice) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(choice_name(choice)).color(TEXT_STRONG));
                ui.label(small("Esc leaves").color(TEXT_SUBDUED));
            });
            ui.label(small(usage(choice, settings.trim)).color(TEXT_SUBDUED));
            if choice == Choice::Trim {
                ui.radio_value(&mut settings.trim, TrimMode::Power, "Power trim");
                ui.radio_value(&mut settings.trim, TrimMode::Closest, "Trim to closest");
            }
            if let Some(shown) = &view.dimension {
                let picked = match shown.picks {
                    0 => "Nothing picked".to_owned(),
                    1 => "1 picked".to_owned(),
                    n => format!("{n} picked"),
                };
                let locked = if shown.locked { " · kind locked" } else { "" };
                ui.label(small(&format!("{picked}{locked}")).color(TEXT_DEFAULT));
                if let Some(unavailable) = &shown.unavailable {
                    ui.label(small(unavailable).color(WARNING));
                }
            }
            if let Some(said) = &view.tool_said {
                ui.label(small(said).color(WARNING));
            }
        }
        None if view.dragging => {
            ui.label(RichText::new("Dragging").color(TEXT_STRONG));
            ui.label(small("Release to place it, Esc to put it back.").color(TEXT_SUBDUED));
        }
        None => {
            ui.label(
                small("None. Line L, Rectangle R, Circle C, Arc A, Point P, Trim T, Dimension D")
                    .color(TEXT_SUBDUED),
            );
            ui.label(
                small(
                    "Drag a blue point or curve to move it: what's fully defined stays put. Double-click a dimension to change it.",
                )
                .color(TEXT_SUBDUED),
            );
        }
    }
    let held = match ui.ctx().os() {
        egui::os::OperatingSystem::Mac | egui::os::OperatingSystem::IOS => "⌘",
        _ => "Ctrl",
    };
    ui.checkbox(&mut settings.automatic_relations, "Automatic relations")
        .on_hover_text(format!("Hold {held} to add none for one click"));
    ui.checkbox(&mut settings.construction, "Construction (X)");
}

/// What the canvas's right-click menu in sketch mode was asked (the prototype's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    None,
    EndChain,
    /// Smart Dimension: its tool chosen, as the bar's button and D choose it.
    Dimension,
    Finish,
    Cancel,
}

/// Why End chain is off: no tool's gesture is under way.
pub const NOTHING_BEING_DRAWN: &str = "Nothing is being drawn";

/// An item of the canvas's right-click menu in sketch mode: what it asks, its icon, its
/// words, its keys' caps (the registry's, as the platform names them), why it's off if it
/// is, and whether a separator comes before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub action: MenuAction,
    pub icon: Option<crate::icons::Icon>,
    pub label: &'static str,
    pub caps: Vec<String>,
    pub off: Option<&'static str>,
    pub separated: bool,
}

impl MenuItem {
    /// It as every menu draws an item (`crate::menu`), lit by nothing but the pointer.
    pub fn line(&self) -> crate::menu::Item<'_> {
        crate::menu::Item {
            icon: self.icon,
            label: self.label,
            sub: None,
            caps: &self.caps,
            off: self.off,
            danger: false,
            lit: false,
        }
    }
}

/// The canvas's right-click menu in sketch mode, as the prototype's `onContext` has it
/// (js/interact.js), in its order: *End chain* (Esc), off but while a tool's gesture is
/// under way (`busy`); Smart Dimension (D), with its icon; a separator; Finish Sketch (its
/// Ctrl+↵, ⌘↵ on macOS), with its icon; and Cancel sketch. Each cap is the registry's
/// (`crate::shortcuts`): Smart Dimension's and Finish Sketch's their own commands', and End
/// chain's Esc, which the app takes as the registry's Stop and gives sketch mode, where it
/// ends the gesture (`Tab::escape`).
pub fn menu_items(ctx: &egui::Context, busy: bool) -> Vec<MenuItem> {
    use crate::icons::Icon;
    vec![
        MenuItem {
            action: MenuAction::EndChain,
            icon: None,
            label: "End chain",
            caps: Command::Stop.caps(ctx),
            off: (!busy).then_some(NOTHING_BEING_DRAWN),
            separated: false,
        },
        MenuItem {
            action: MenuAction::Dimension,
            icon: Some(Icon::Dimension),
            label: Command::Dimension.label(),
            caps: Command::Dimension.caps(ctx),
            off: None,
            separated: false,
        },
        MenuItem {
            action: MenuAction::Finish,
            icon: Some(Icon::Check),
            label: Command::FinishSketch.label(),
            caps: Command::FinishSketch.caps(ctx),
            off: None,
            separated: true,
        },
        MenuItem {
            action: MenuAction::Cancel,
            icon: None,
            label: "Cancel sketch",
            caps: Vec::new(),
            off: None,
            separated: false,
        },
    ]
}

/// The canvas's right-click menu in sketch mode ([`menu_items`]), drawn as every menu over
/// the window is (`crate::menu`, as the prototype's one `menu(...)` draws both of its
/// canvas's menus): each item as wide as the widest, a separator before Finish Sketch. The
/// pane opens it; what was asked is given back.
pub fn canvas_menu(ui: &mut Ui, busy: bool) -> MenuAction {
    let items = menu_items(ui.ctx(), busy);
    let lines: Vec<crate::menu::Item<'_>> = items.iter().map(MenuItem::line).collect();
    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
    let width = crate::menu::width(ui, None, &lines, 0.0);
    let mut action = MenuAction::None;
    for (item, line) in items.iter().zip(&lines) {
        if item.separated {
            crate::menu::separator(ui, width);
        }
        if crate::menu::item(ui, line, width).clicked() {
            action = item.action;
        }
    }
    if action != MenuAction::None {
        ui.close();
    }
    action
}

/// Where the canvas menu opened by a right-click at `at` stands: the top left of the
/// physical pixel the click is in, at `pixels_per_point`, so the menu's first pixel is the
/// click's (egui would round a corner between pixels to the nearer, which may be past it).
pub fn menu_corner(at: Pos2, pixels_per_point: f32) -> Pos2 {
    let pixel = |c: f32| (c * pixels_per_point).floor() / pixels_per_point;
    Pos2::new(pixel(at.x), pixel(at.y))
}

/// Whether a press at `at` is on the menu `menu`, both in physical pixels at
/// `pixels_per_point`: the pixel the press is in, among those the menu covers (from the
/// pixel its edge rounds to, as egui draws an area, to the one before its far edge's).
pub fn on_menu(at: Pos2, menu: Rect, pixels_per_point: f32) -> bool {
    let pixel = |c: f32| (c * pixels_per_point).floor();
    let edge = |c: f32| (c * pixels_per_point).round();
    let across = |c: f32, low: f32, high: f32| (edge(low)..edge(high)).contains(&pixel(c));
    across(at.x, menu.left(), menu.right()) && across(at.y, menu.top(), menu.bottom())
}

/// Whether this frame's `events` press a button off the menu `menu` ([`on_menu`]), which
/// closes it. In the frame the menu opens (`opening`), every press before the last right
/// release (the click that opened it, wherever in its click distance it was pressed, and
/// any press of any button a slow frame took before it, a tool's click among them) came
/// before the menu was there, and closes nothing; only one after it does.
pub fn pressed_off_menu(
    events: &[egui::Event],
    menu: Rect,
    pixels_per_point: f32,
    opening: bool,
) -> bool {
    use egui::{Event, PointerButton};
    let released = events.iter().rposition(|event| {
        matches!(
            event,
            Event::PointerButton {
                button: PointerButton::Secondary,
                pressed: false,
                ..
            }
        )
    });
    events.iter().enumerate().any(|(place, event)| match event {
        Event::PointerButton {
            pos, pressed: true, ..
        } => {
            let before_it = opening && released.is_some_and(|released| place < released);
            !before_it && !on_menu(*pos, menu, pixels_per_point)
        }
        _ => false,
    })
}

/// The palette's Status: the state, with its dot, a word on it, the read's refusal, and the
/// plane's problem.
fn status_section(ui: &mut Ui, view: &PaletteView) {
    match &view.state {
        Some(state) => {
            ui.horizontal(|ui| {
                state_dot(ui, state.colour);
                ui.label(RichText::new(&state.words).color(state.colour));
            });
        }
        None => {
            ui.label(small("Reading the sketch…").color(TEXT_SUBDUED));
        }
    }
    if let Some(hint) = view.hint {
        ui.label(small(hint).color(TEXT_SUBDUED));
    }
    if let Some(said) = &view.said {
        ui.label(small(said).color(ERROR_TEXT));
    }
    if let Some((message, suggestion)) = &view.plane_problem {
        ui.label(small(&format!("The sketch's plane: {message}")).color(ERROR_TEXT));
        if let Some(suggestion) = suggestion {
            ui.label(small(suggestion).color(TEXT_SUBDUED));
        }
    }
}

/// The colours' legend: each state's colour beside its words, construction dashed.
fn legend(ui: &mut Ui) {
    for (colour, words) in [
        (SKETCH_UNDER_DEFINED, "Under defined"),
        (SKETCH_FULLY_DEFINED, "Fully defined"),
        (SKETCH_OVER_DEFINED, "Over defined, unsolvable"),
        (SKETCH_INVALID, "Invalid (zero length)"),
        (SKETCH_DANGLING, "Dangling reference"),
        (SKETCH_DRIVEN, "Driven dimension"),
    ] {
        ui.horizontal(|ui| {
            let (swatch, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(
                egui::Rect::from_center_size(swatch.center(), Vec2::new(10.0, 3.0)),
                CornerRadius::same(2),
                colour,
            );
            ui.label(small(words));
        });
    }
    ui.horizontal(|ui| {
        let (swatch, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
        let [dash, gap] = SKETCH_CONSTRUCTION_DASH;
        ui.painter().extend(egui::Shape::dashed_line(
            &[swatch.left_center(), swatch.right_center()],
            Stroke::new(2.0, TEXT_SUBDUED),
            dash / 2.0,
            gap / 2.0,
        ));
        ui.label(small("Construction"));
    });
}

/// A palette section: a rule above, its heading, then `contents`.
fn section(ui: &mut Ui, heading: &str, contents: impl FnOnce(&mut Ui)) {
    ui.separator();
    egui::Frame::new()
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(
                RichText::new(heading.to_uppercase())
                    .size(HEAD_SIZE)
                    .color(TEXT_SUBDUED),
            );
            contents(ui);
        });
}

fn small(text: &str) -> RichText {
    RichText::new(text).size(SMALL_SIZE)
}

/// A state's dot, in `colour`.
fn state_dot(ui: &mut Ui, colour: Color32) {
    let side = 2.0 * DOT_RADIUS;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), DOT_RADIUS, colour);
}

/// What the status bar says of an open sketch (docs/ux/solidworks.md, §2: the pointer's
/// coordinates, the state, and "Editing"): what's edited, the pointer on the plane by its
/// axes' letters (none off the canvas), and the state. The status bar draws it
/// (`crate::status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub editing: String,
    pub coordinates: Option<String>,
    pub state: Option<StateShown>,
}
