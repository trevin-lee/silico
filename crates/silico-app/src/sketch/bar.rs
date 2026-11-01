//! Sketch mode's own panels in the canvas, as the approved prototype lays them out
//! (docs/design/m5-ui/, `js/sketch.js`'s `renderCanvasUI` and `statusText`): the sketch bar at
//! the canvas's top (the sketch, its plane, its state, Cancel, and Finish Sketch), the
//! Sketch Palette at its right, and what the status bar says of the sketch. Each is laid
//! out by a small function of its own, given what it shows and giving back what was asked,
//! apart from sketch mode's state and actions.

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Sense, Stroke, Ui, Vec2};
use silico_core::sketch_tools::ToolKind;

use super::style::*;

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
    // One row, whatever room the canvas leaves: its words are never wrapped away.
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    egui::Frame::new()
        .fill(FLOATING_BG)
        .stroke(Stroke::new(1.0, SELECTION))
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
                node.set_label("Sketch");
            });
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(SELECTION)
                    .corner_radius(CornerRadius::same(3))
                    .inner_margin(Margin::symmetric(6, 1))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("SKETCH")
                                .size(PALETTE_HEAD_SIZE)
                                .color(TEXT_STRONG),
                        );
                    });
                ui.label(RichText::new(&view.name).color(TEXT_STRONG));
                ui.label(RichText::new(format!("on {}", view.plane)).color(TEXT_SUBDUED));
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
                let finish = egui::Button::new(RichText::new("Finish Sketch").color(TEXT_STRONG))
                    .fill(SELECTION);
                let key = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::Enter,
                ));
                if ui
                    .add_enabled(view.enabled, finish)
                    .on_hover_text(format!(
                        "Finish Sketch ({key}): keeps your changes and rebuilds"
                    ))
                    .clicked()
                {
                    action = BarAction::Finish;
                }
            });
        });
    action
}

/// The palette's settings: what's shown, Construction, and *Automatic relations* (the last
/// two kept for M5.11b's tools).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub show_profile: bool,
    pub show_points: bool,
    pub show_dimensions: bool,
    pub show_constraints: bool,
    pub construction: bool,
    pub automatic_relations: bool,
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
        }
    }
}

/// What the palette shows: the sketch's state, and a word on what it means; the tool on;
/// the read's refusal, as said; and why the sketch's face wasn't found (#347), with its
/// suggestion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteView {
    pub state: Option<StateShown>,
    pub hint: Option<&'static str>,
    pub tool: Option<ToolKind>,
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

/// The Sketch Palette (the prototype's `.palette`): its title, then Status, Tool, Show,
/// and the colours' legend, each a section; Look At off until the viewport's camera.
pub fn palette(ui: &mut Ui, view: &PaletteView, settings: &mut Settings) {
    egui::Frame::new()
        .fill(FLOATING_BG)
        .stroke(Stroke::new(1.0, PANEL_BORDER))
        .corner_radius(CornerRadius::same(6))
        .shadow(egui::Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(140),
        })
        .show(ui, |ui| {
            ui.set_width(PALETTE_WIDTH);
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Group);
                node.set_label("Sketch Palette");
            });
            ui.spacing_mut().item_spacing.y = 2.0;
            let (title, _) = ui.allocate_exact_size(
                Vec2::new(PALETTE_WIDTH, PALETTE_TITLE_HEIGHT),
                Sense::hover(),
            );
            ui.put(
                title.shrink2(Vec2::new(10.0, 0.0)),
                egui::Label::new(
                    RichText::new("SKETCH PALETTE")
                        .size(PALETTE_SMALL_SIZE)
                        .color(TEXT_STRONG),
                ),
            );
            section(ui, "Status", |ui| status_section(ui, view));
            section(ui, "Tool", |ui| {
                match view.tool {
                    Some(tool) => {
                        ui.label(RichText::new(tool_name(tool)).color(TEXT_STRONG));
                        ui.checkbox(&mut settings.automatic_relations, "Automatic relations");
                    }
                    None => {
                        ui.label(small("None. Line L, Smart Dimension D").color(TEXT_SUBDUED));
                    }
                }
                ui.checkbox(&mut settings.construction, "Construction");
            });
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
        ui.label(small(said).color(SKETCH_OVER_DEFINED));
    }
    if let Some((message, suggestion)) = &view.plane_problem {
        ui.label(small(&format!("The sketch's plane: {message}")).color(SKETCH_OVER_DEFINED));
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
                    .size(PALETTE_HEAD_SIZE)
                    .color(TEXT_SUBDUED),
            );
            contents(ui);
        });
}

fn small(text: &str) -> RichText {
    RichText::new(text).size(PALETTE_SMALL_SIZE)
}

/// A state's dot, in `colour`.
fn state_dot(ui: &mut Ui, colour: Color32) {
    let side = 2.0 * STATE_DOT_RADIUS;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), STATE_DOT_RADIUS, colour);
}

/// What the status bar says of an open sketch (docs/ux/solidworks.md, §2: the pointer's
/// coordinates, the state, and "Editing"): what's edited, the pointer on the plane by its
/// axes' letters (none off the canvas), and the state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub editing: String,
    pub coordinates: Option<String>,
    pub state: Option<StateShown>,
}

/// The status bar's sketch content, for the status bar to place: what's edited, the
/// coordinates (a dash without them), and the state, in its colour.
pub fn status_ui(ui: &mut Ui, line: &StatusLine) {
    ui.label(&line.editing);
    ui.separator();
    ui.label(line.coordinates.as_deref().unwrap_or("—"));
    if let Some(state) = &line.state {
        ui.separator();
        ui.label(RichText::new(&state.words).color(state.colour));
    }
}
