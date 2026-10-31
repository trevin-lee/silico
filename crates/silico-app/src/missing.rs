//! What the app shows when it can't start: each worker it's missing and where it looked
//! (`silico_core::workers::find`), in a window of its own that needs no worker, so a person
//! who started the app from the Finder or the Dock, with no terminal to read, sees why. The
//! app then exits non-zero, as `silico serve` does.

use eframe::egui::{self, RichText, Ui};

/// The window listing why the app can't start.
pub struct Missing {
    lines: Vec<String>,
}

impl Missing {
    /// A window saying `lines`, each a missing worker's.
    pub fn new(lines: Vec<String>) -> Self {
        Missing { lines }
    }

    /// One frame: the reasons, and Quit.
    pub fn frame(&mut self, ui: &mut Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Silico can't start: its install is missing workers");
            for line in &self.lines {
                ui.label(RichText::new(line).color(ui.visuals().error_fg_color));
            }
            ui.label("Install Silico again, or build its workers beside it.");
            if ui.button("Quit").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

impl eframe::App for Missing {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
}
