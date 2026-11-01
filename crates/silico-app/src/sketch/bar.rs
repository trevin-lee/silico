//! The sketch bar and the Sketch Palette's layout (fusion.md, *Sketch mode*; docs/design/
//! m5.0-app.md, section 8): plain egui widgets, each laid out by a small function of its
//! own, given what it shows and giving back what was asked, so the look's port replaces
//! these and nothing of sketch mode's state or actions.

use eframe::egui::{self, RichText, Ui};

use crate::theme;

/// What the sketch bar was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarAction {
    None,
    Finish,
    Cancel,
}

/// The tools the bar shows, by panel, off until M5.11b gives them their behaviour.
pub const TOOLS: [(&str, &[&str]); 3] = [
    (
        "CREATE",
        &[
            "Line",
            "Corner Rectangle",
            "Center Rectangle",
            "Circle",
            "Center Point Arc",
            "3-Point Arc",
            "Tangent Arc",
            "Point",
        ],
    ),
    ("MODIFY", &["Trim", "Split"]),
    ("CONSTRAINTS", &["Sketch Dimension"]),
];

/// The contextual Sketch tab's row (fusion.md, *The toolbar*, *Sketch mode*): its panels'
/// tools, off for now, then Finish Sketch, highlighted, and Cancel; both off while
/// `enabled` isn't (the tab closing).
pub fn sketch_bar(ui: &mut Ui, enabled: bool) -> BarAction {
    let mut action = BarAction::None;
    ui.label(RichText::new("DESIGN").strong());
    ui.separator();
    ui.label("SOLID");
    ui.separator();
    ui.label(RichText::new("SKETCH").strong());
    ui.scope_builder(egui::UiBuilder::new().id_salt("sketch_bar"), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::Toolbar);
            node.set_label("SKETCH");
        });
        ui.horizontal(|ui| {
            for (panel, tools) in TOOLS {
                ui.separator();
                ui.label(RichText::new(panel).small());
                for tool in tools {
                    ui.add_enabled(false, egui::Button::new(*tool));
                }
            }
            ui.separator();
            let finish = egui::Button::new(RichText::new("Finish Sketch").strong())
                .fill(theme::SKETCH_FINISH_FILL);
            if ui.add_enabled(enabled, finish).clicked() {
                action = BarAction::Finish;
            }
            if ui
                .add_enabled(enabled, egui::Button::new("Cancel"))
                .clicked()
            {
                action = BarAction::Cancel;
            }
        });
    });
    action
}

/// What the palette shows of the sketch: its state's words, or why there's none, and why its
/// plane wasn't found, if it wasn't.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteView {
    /// "Under Defined", "Fully Defined", and so on; none until a read has answered.
    pub status: Option<&'static str>,
    /// The read's refusal, as said, or what's awaited.
    pub said: Option<String>,
    /// Why the sketch's face wasn't found (#347), and its suggestion.
    pub plane_problem: Option<(String, Option<String>)>,
}

/// The palette's settings: what's shown, and *Automatic relations* (kept for M5.11b's tools).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub show_dimensions: bool,
    pub show_constraints: bool,
    pub automatic_relations: bool,
}

impl Default for Settings {
    /// Constraints shown by default (fusion.md, *Sketch mode*), dimensions too, and
    /// *Automatic relations* on (docs/ux/solidworks.md, §2).
    fn default() -> Self {
        Settings {
            show_dimensions: true,
            show_constraints: true,
            automatic_relations: true,
        }
    }
}

/// The Sketch Palette's contents (fusion.md, *Sketch mode*, [pal]): Status, then the
/// plane's problem if there's one, Look At (off until the viewport's camera), and the
/// settings.
pub fn palette(ui: &mut Ui, view: &PaletteView, settings: &mut Settings) {
    let status = view.status.unwrap_or("Reading the sketch…");
    ui.label(format!("Status: {status}"));
    if let Some(said) = &view.said {
        ui.colored_label(ui.visuals().error_fg_color, said);
    }
    if let Some((message, suggestion)) = &view.plane_problem {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("The sketch's plane: {message}"),
        );
        if let Some(suggestion) = suggestion {
            ui.weak(suggestion);
        }
    }
    ui.separator();
    ui.add_enabled(false, egui::Button::new("Look At"));
    ui.checkbox(&mut settings.show_dimensions, "Show Dimensions");
    ui.checkbox(&mut settings.show_constraints, "Show Constraints");
    ui.checkbox(&mut settings.automatic_relations, "Automatic relations");
}
