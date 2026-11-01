//! The export dialog (M5.12; docs/design/m5.0-app.md, section 8's dialogs, and M4.1's
//! `export`): File > Export…, its format (3MF, meshed at a tolerance, or STEP), which bodies,
//! and the path, sent as `export` on the session, off the UI thread, as every command is.
//! While it runs the dialog says so, and its Cancel is `cancel export`: an export meshing is
//! its own dialog's to stop, not the status bar's. A file already at the path is asked
//! about, as SolidWorks asks before it writes over one, and replaced only if the person says
//! so (`replace`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Ui};
use silico_core::command::SessionId;
use silico_core::command::export::{DEFAULT_TOLERANCE, Export, ExportFormat};
use silico_core::document::{FeatureId, Length};

use crate::browse;
use crate::tab::Row;

/// What the dialog holds while it's open: the format, the 3MF's tolerance as typed, the
/// bodies, and the path, of the document of session `session`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportForm {
    pub session: SessionId,
    /// STEP, or else 3MF.
    pub step: bool,
    /// The 3MF's tolerance, in mm, as typed.
    pub tolerance: String,
    /// Every body (`features` null), or else only those `chosen`.
    pub all: bool,
    pub chosen: BTreeSet<FeatureId>,
    /// The path as typed: one that isn't absolute is in the document's folder.
    pub path: String,
    /// The document's path, whose folder a relative path is in.
    document: String,
}

/// What the person did in the dialog this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Export: the request.
    Export(Export),
    /// Replace the file the last export found at its path: the request again, replacing.
    Replace,
    /// Cancel: the export running stopped (`cancel export`), and the dialog closed.
    Cancel,
    /// Browse…: the native dialog asked for the path.
    Browse,
}

/// What the dialog shows of the tab's export: running, the file at the path to ask about,
/// and the core's refusal, as said to the person.
pub struct Shown<'a> {
    pub running: bool,
    pub replacing: Option<&'a Path>,
    pub said: Option<&'a str>,
    /// The Browse… button beside the path, if the app has native dialogs, and whether it's
    /// on (off while one is open).
    pub browse: Option<bool>,
}

impl ExportForm {
    /// The dialog of the document at `document`, of session `session`: a 3MF of every
    /// body, at FreeCAD's tolerance (the core's default), beside the document, named by it.
    pub fn new(session: SessionId, document: &str) -> Self {
        let mut form = ExportForm {
            session,
            step: false,
            tolerance: DEFAULT_TOLERANCE.to_string(),
            all: true,
            chosen: BTreeSet::new(),
            path: String::new(),
            document: document.to_owned(),
        };
        form.path = with_extension(document, form.extension());
        form
    }

    /// The extension the format writes, its own (STEP's `.step`; `.stp` is taken too).
    fn extension(&self) -> &'static str {
        if self.step { "step" } else { "3mf" }
    }

    /// The format chosen: the path's extension follows it, if it was the other's.
    fn choose(&mut self, step: bool) {
        if self.step == step {
            return;
        }
        let other = format_of(&self.path) == Some(!step);
        self.step = step;
        if other {
            self.path = with_extension(&self.path, self.extension());
        }
    }

    /// The path the native dialog picked, in the path field: the format follows its
    /// extension, if it's a format's (a `.step` picked while 3MF is chosen is a STEP).
    pub fn browsed(&mut self, path: String) {
        if let Some(step) = format_of(&path) {
            self.step = step;
        }
        self.path = path;
    }

    /// The tolerance typed, if it reads as a length: the core checks its range.
    fn tolerance(&self) -> Option<Length> {
        Length::parse(self.tolerance.trim()).ok()
    }

    /// The request the dialog asks for, of `bodies` (the model's, in the timeline's order):
    /// none while its tolerance doesn't read, its path is empty, or no body is chosen.
    pub fn request(&self, bodies: &[Row]) -> Option<Export> {
        let format = if self.step {
            ExportFormat::Step
        } else {
            ExportFormat::ThreeMf {
                tolerance: self.tolerance()?,
            }
        };
        if self.path.trim().is_empty() {
            return None;
        }
        let features = if self.all {
            None
        } else {
            let chosen: Vec<FeatureId> = bodies
                .iter()
                .filter(|row| self.chosen.contains(&row.id))
                .map(|row| row.id.clone())
                .collect();
            if chosen.is_empty() {
                return None;
            }
            Some(chosen)
        };
        Some(Export {
            format,
            path: beside(&self.document, &self.path),
            replace: false,
            features,
        })
    }

    /// What Enter does: Export, if the dialog would take it (its request valid, nothing
    /// running, no question asked).
    pub fn enter(&self, bodies: &[Row], shown: &Shown<'_>) -> Action {
        match self.request(bodies) {
            Some(request) if !shown.running && shown.replacing.is_none() => Action::Export(request),
            _ => Action::None,
        }
    }

    /// The dialog drawn into `ui`, of `bodies`, showing `shown` of the tab's export.
    pub fn ui(&mut self, ui: &mut Ui, bodies: &[Row], shown: &Shown<'_>) -> Action {
        let mut action = Action::None;
        let idle = !shown.running && shown.replacing.is_none();
        if let Some(path) = shown.replacing {
            ui.label(format!("{} already exists. Replace it?", path.display()));
            ui.horizontal(|ui| {
                if ui.button("Replace").clicked() {
                    action = Action::Replace;
                }
                if ui.button("Cancel").clicked() {
                    action = Action::Cancel;
                }
            });
            return action;
        }
        ui.add_enabled_ui(idle, |ui| {
            ui.horizontal(|ui| {
                ui.label("Format");
                for (step, word) in [(false, "3MF"), (true, "STEP")] {
                    if ui
                        .add(egui::Button::selectable(self.step == step, word))
                        .clicked()
                    {
                        self.choose(step);
                    }
                }
            });
            if !self.step {
                ui.horizontal(|ui| {
                    let label = ui.label("Tolerance");
                    let red = unreadable(&self.tolerance);
                    let mut edit = egui::TextEdit::singleline(&mut self.tolerance)
                        .id_salt("export-tolerance")
                        .desired_width(100.0);
                    if red {
                        edit = edit.text_color(ui.visuals().error_fg_color);
                    }
                    ui.add(edit).labelled_by(label.id);
                    ui.label("mm");
                });
            }
            ui.checkbox(&mut self.all, "All bodies");
            if !self.all {
                for row in bodies {
                    let mut on = self.chosen.contains(&row.id);
                    if ui.checkbox(&mut on, &row.name).changed() {
                        if on {
                            self.chosen.insert(row.id.clone());
                        } else {
                            self.chosen.remove(&row.id);
                        }
                    }
                }
            }
            ui.horizontal(|ui| {
                let label = ui.label("Path");
                ui.add(egui::TextEdit::singleline(&mut self.path).id_salt("export-path"))
                    .labelled_by(label.id);
                if let Some(on) = shown.browse
                    && browse::button(ui, on)
                {
                    action = Action::Browse;
                }
            });
        });
        if shown.running {
            // Said, not animated: the app draws a frame only when something changed.
            ui.label("Exporting…");
        }
        if let Some(said) = shown.said {
            ui.colored_label(ui.visuals().error_fg_color, said);
        }
        ui.horizontal(|ui| {
            let request = self.request(bodies);
            if ui
                .add_enabled(idle && request.is_some(), egui::Button::new("Export"))
                .clicked()
                && let Some(request) = request
            {
                action = Action::Export(request);
            }
            if ui.button("Cancel").clicked() {
                action = Action::Cancel;
            }
        });
        action
    }
}

/// The path `typed`, of the document at `document`: as typed if it's absolute, or else in
/// the document's folder, as SolidWorks puts a bare name in its dialog's folder. Only the
/// path's words: nothing on disk is looked at.
pub fn beside(document: &str, typed: &str) -> PathBuf {
    let typed = Path::new(typed);
    // Joined onto an absolute path, the folder is replaced by it.
    match Path::new(document).parent() {
        Some(folder) => folder.join(typed),
        None => typed.to_path_buf(),
    }
}

/// The format `path`'s extension names, in any case: STEP (`.step`, `.stp`) is `true`, 3MF
/// (`.3mf`) `false`; none if it names neither.
fn format_of(path: &str) -> Option<bool> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "step" | "stp" => Some(true),
        "3mf" => Some(false),
        _ => None,
    }
}

/// Whether a tolerance as typed shows red: it doesn't read as a length.
fn unreadable(tolerance: &str) -> bool {
    Length::parse(tolerance.trim()).is_err()
}

/// `path` with its extension `extension` in place of the one it has (if any).
fn with_extension(path: &str, extension: &str) -> String {
    Path::new(path)
        .with_extension(extension)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> ExportForm {
        ExportForm::new(SessionId::parse("s1").unwrap(), "/work/bracket.silico")
    }

    fn row(id: &str, name: &str) -> Row {
        Row {
            id: FeatureId::new(id).unwrap(),
            name: name.to_owned(),
            health: silico_core::timeline::Health::Healthy,
            marked_by: None,
        }
    }

    #[test]
    fn a_new_dialog_asks_a_3mf_of_every_body_at_the_cores_default_beside_the_document() {
        let form = form();
        assert_eq!(form.path, "/work/bracket.3mf");
        assert_eq!(form.tolerance, "0.1");
        assert_eq!(
            form.request(&[]),
            Some(Export {
                format: ExportFormat::ThreeMf {
                    tolerance: Length::from_nm(100_000)
                },
                path: PathBuf::from("/work/bracket.3mf"),
                replace: false,
                features: None,
            })
        );
    }

    #[test]
    fn a_tolerance_that_doesnt_read_as_a_length_shows_red() {
        for typed in ["0.1", " 0.25 ", "1e-5", "0"] {
            assert!(!unreadable(typed), "{typed:?}");
        }
        for typed in ["x", "", "-0", "1e400", "0.1mm"] {
            assert!(unreadable(typed), "{typed:?}");
        }
    }

    #[test]
    fn the_paths_extension_follows_the_format_only_where_it_was_the_others() {
        let mut form = form();
        form.choose(true);
        assert_eq!(form.path, "/work/bracket.step");
        form.choose(false);
        assert_eq!(form.path, "/work/bracket.3mf");
        // .stp is STEP's too.
        form.path = "/work/part.stp".to_owned();
        form.step = true;
        form.choose(false);
        assert_eq!(form.path, "/work/part.3mf");
        // A name of the person's own is kept.
        form.path = "/work/part.bin".to_owned();
        form.choose(true);
        assert_eq!(form.path, "/work/part.bin");
        // Upper case is the format's too.
        form.path = "/work/part.STEP".to_owned();
        form.step = true;
        form.choose(false);
        assert_eq!(form.path, "/work/part.3mf");
        form.path = "/work/part.3MF".to_owned();
        form.choose(true);
        assert_eq!(form.path, "/work/part.step");
        form.step = true;
        // Choosing the format it has changes nothing.
        form.path = "/work/part.3mf".to_owned();
        form.choose(true);
        assert_eq!(form.path, "/work/part.3mf");
    }

    #[test]
    fn a_path_not_absolute_is_in_the_documents_folder() {
        assert_eq!(
            beside("/work/bracket.silico", "out.step"),
            PathBuf::from("/work/out.step")
        );
        assert_eq!(
            beside("/work/bracket.silico", "sub/out.step"),
            PathBuf::from("/work/sub/out.step")
        );
        assert_eq!(
            beside("/work/bracket.silico", "/elsewhere/out.step"),
            PathBuf::from("/elsewhere/out.step")
        );
        // A document opened by a bare name: its folder is the one it was opened in.
        assert_eq!(
            beside("bracket.silico", "out.step"),
            PathBuf::from("out.step")
        );
        let mut form = form();
        // As typed, spaces and all: only an empty path (spaces aside) sends nothing.
        form.path = " out.3mf".to_owned();
        assert_eq!(
            form.request(&[]).unwrap().path,
            PathBuf::from("/work/ out.3mf")
        );
    }

    #[test]
    fn a_request_needs_a_tolerance_that_reads_a_path_and_a_body() {
        let bodies = [row("b0000001", "Plate"), row("b0000002", "Washer")];
        let mut form = form();
        // What doesn't read as a length: not a number, past an i64 of nanometres, finer
        // than a nanometre, zero written negative, a float's words.
        for typed in ["x", "", "1e400", "0.0000001", "-0", "NaN", "inf", "0.1mm"] {
            form.tolerance = typed.to_owned();
            assert_eq!(form.request(&bodies), None, "{typed:?}");
        }
        // STEP takes none: what's typed doesn't matter.
        form.step = true;
        assert!(form.request(&bodies).is_some());
        form.step = false;
        form.tolerance = " 0.25 ".to_owned();
        assert!(form.request(&bodies).is_some());
        form.path = "  ".to_owned();
        assert_eq!(form.request(&bodies), None);
        form.path = "/work/out.3mf".to_owned();
        // Some bodies: in the timeline's order, whatever order they were chosen in.
        form.all = false;
        assert_eq!(form.request(&bodies), None);
        form.chosen.insert(FeatureId::new("b0000002").unwrap());
        form.chosen.insert(FeatureId::new("b0000001").unwrap());
        assert_eq!(
            form.request(&bodies).unwrap().features,
            Some(vec![
                FeatureId::new("b0000001").unwrap(),
                FeatureId::new("b0000002").unwrap()
            ])
        );
        // A body chosen that the model no longer has isn't sent.
        form.chosen.clear();
        form.chosen.insert(FeatureId::new("b0000009").unwrap());
        assert_eq!(form.request(&bodies), None);
    }
}
