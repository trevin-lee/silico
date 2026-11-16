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
use crate::modal::{self, Button, Kind};
use crate::properties;
use crate::tab::Row;
use crate::theme::{self, FieldKind};

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
    /// The path as typed: one that isn't absolute is in the document's folder (and refused
    /// for an untitled part, which has none: [`refusal`](Self::refusal)).
    pub path: String,
    /// The document's path, whose folder a relative path is in: none for an untitled part.
    document: Option<String>,
}

/// What the export dialog (and the typed Save As) says of a path that isn't absolute, typed
/// for an untitled part: there's no folder for it to be in.
pub const NO_FOLDER: &str = "Give a full path: an untitled part has no folder";

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
        let mut form = Self::of(session, Some(document.to_owned()));
        form.path = with_extension(document, form.extension());
        form
    }

    /// The dialog of an untitled part shown as `name`, of session `session`: as
    /// [`new`](Self::new)'s, but its path in `documents` (the platform's Documents folder),
    /// named by the part, or empty if there's no such folder; a path typed must be absolute.
    pub fn untitled(session: SessionId, name: &str, documents: Option<&Path>) -> Self {
        let mut form = Self::of(session, None);
        form.path = documents.map_or_else(String::new, |folder| {
            folder
                .join(format!("{name}.{}", form.extension()))
                .to_string_lossy()
                .into_owned()
        });
        form
    }

    fn of(session: SessionId, document: Option<String>) -> Self {
        ExportForm {
            session,
            step: false,
            tolerance: DEFAULT_TOLERANCE.to_string(),
            all: true,
            chosen: BTreeSet::new(),
            path: String::new(),
            document,
        }
    }

    /// The document saved as the file at `document` while the dialog was open (an untitled
    /// part's first save): a relative path is in its folder from then on. The path typed is
    /// left as it is.
    pub fn saved_as(&mut self, document: &str) {
        self.document = Some(document.to_owned());
    }

    /// What's said of the path typed, if it can't be taken: one that isn't absolute, for an
    /// untitled part ([`NO_FOLDER`]).
    pub fn refusal(&self) -> Option<&'static str> {
        let typed = self.path.trim();
        (!typed.is_empty() && beside(self.document.as_deref(), &self.path).is_none())
            .then_some(NO_FOLDER)
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
    /// none while its tolerance doesn't read, its path is empty (or, for an untitled part,
    /// isn't absolute), or no body is chosen.
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
        let path = beside(self.document.as_deref(), &self.path)?;
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
            path,
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

    /// The dialog's body and foot drawn into `ui`, a modal's (`crate::modal`), of `bodies`,
    /// showing `shown` of the tab's export.
    pub fn ui(&mut self, ui: &mut Ui, bodies: &[Row], shown: &Shown<'_>) -> Action {
        let mut action = Action::None;
        let idle = !shown.running && shown.replacing.is_none();
        if let Some(path) = shown.replacing {
            modal::body(ui, |ui| {
                modal::text(
                    ui,
                    &format!("{} already exists. Replace it?", path.display()),
                    theme::MODAL_TEXT,
                );
            });
            let ([], [cancel, replace]) = modal::foot(
                ui,
                [],
                [
                    Button::new("Cancel", Kind::Secondary),
                    Button::new("Replace", Kind::Danger),
                ],
            );
            if replace {
                action = Action::Replace;
            }
            if cancel {
                action = Action::Cancel;
            }
            return action;
        }
        modal::body(ui, |ui| {
            ui.add_enabled_ui(idle, |ui| {
                modal::label(ui, "Format");
                let formats = ["3MF".to_owned(), "STEP".to_owned()];
                if let Some(place) =
                    properties::segmented(ui, &formats, usize::from(self.step), idle)
                {
                    self.choose(place == 1);
                }
                if !self.step {
                    let label = modal::label(ui, "Tolerance");
                    let red = unreadable(&self.tolerance);
                    let unit_width = properties::unit_width(ui, "mm");
                    let edit = theme::field_with_room(
                        ui,
                        egui::TextEdit::singleline(&mut self.tolerance),
                        "export-tolerance",
                        FieldKind::Modify,
                        red,
                        unit_width + theme::FIELD_UNIT_PADDING,
                    );
                    let (field, _) = modal::field(ui, edit, label.id, None);
                    properties::unit(ui, field.rect, "mm");
                }
                ui.add_space(theme::MODAL_LABEL_MARGIN_TOP);
                ui.spacing_mut().item_spacing.y = theme::MODAL_LABEL_MARGIN_BOTTOM;
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
                ui.spacing_mut().item_spacing.y = 0.0;
                let label = modal::label(ui, "Path");
                let edit = theme::field(
                    ui,
                    egui::TextEdit::singleline(&mut self.path),
                    "export-path",
                    FieldKind::Modify,
                    false,
                );
                let (_, button) =
                    modal::field(ui, edit, label.id, shown.browse.map(browse::button));
                if button.is_some_and(browse::clicked) {
                    action = Action::Browse;
                }
            });
            if shown.running {
                // Said, not animated: the app draws a frame only when something changed.
                ui.add_space(theme::MODAL_LABEL_MARGIN_TOP);
                modal::text(ui, "Exporting…", theme::MODAL_TEXT);
            }
            if let Some(refusal) = self.refusal() {
                ui.add_space(theme::MODAL_LABEL_MARGIN_TOP);
                modal::text(ui, refusal, theme::ERROR_TEXT);
            }
            if let Some(said) = shown.said {
                ui.add_space(theme::MODAL_LABEL_MARGIN_TOP);
                modal::text(ui, said, theme::ERROR_TEXT);
            }
        });
        let request = self.request(bodies);
        let enter = crate::shortcuts::Command::Ok.keys_text(ui.ctx());
        let ([], [cancel, export]) = modal::foot(
            ui,
            [],
            [
                Button::new("Cancel", Kind::Secondary),
                Button::new("Export", Kind::Primary)
                    .cap(&enter)
                    .enabled(idle && request.is_some()),
            ],
        );
        if export && let Some(request) = request {
            action = Action::Export(request);
        }
        if cancel {
            action = Action::Cancel;
        }
        action
    }
}

/// The path `typed`, of the document at `document`: as typed if it's absolute, or else in
/// the document's folder, as SolidWorks puts a bare name in its dialog's folder. Only the
/// path's words: nothing on disk is looked at. None for a path not absolute typed for an
/// untitled part (`document` none), which has no folder for it to be in.
pub fn beside(document: Option<&str>, typed: &str) -> Option<PathBuf> {
    let typed = Path::new(typed);
    let Some(document) = document else {
        return typed.is_absolute().then(|| typed.to_path_buf());
    };
    // Joined onto an absolute path, the folder is replaced by it.
    Some(match Path::new(document).parent() {
        Some(folder) => folder.join(typed),
        None => typed.to_path_buf(),
    })
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
            consumers: Default::default(),
            suppressed: false,
            last_healthy: None,
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
        let doc = Some("/work/bracket.silico");
        assert_eq!(
            beside(doc, "out.step"),
            Some(PathBuf::from("/work/out.step"))
        );
        assert_eq!(
            beside(doc, "sub/out.step"),
            Some(PathBuf::from("/work/sub/out.step"))
        );
        assert_eq!(
            beside(doc, "/elsewhere/out.step"),
            Some(PathBuf::from("/elsewhere/out.step"))
        );
        // A document opened by a bare name: its folder is the one it was opened in.
        assert_eq!(
            beside(Some("bracket.silico"), "out.step"),
            Some(PathBuf::from("out.step"))
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
    fn an_untitled_parts_export_starts_in_the_documents_folder_or_with_no_path() {
        let id = SessionId::parse("s1").unwrap();
        let form = ExportForm::untitled(id, "Untitled 2", Some(Path::new("/home/me/Documents")));
        assert_eq!(form.path, "/home/me/Documents/Untitled 2.3mf");
        assert_eq!(form.refusal(), None);
        assert_eq!(
            form.request(&[]).unwrap().path,
            PathBuf::from("/home/me/Documents/Untitled 2.3mf")
        );
        // No Documents folder: no path, so nothing to send, and nothing refused yet.
        let form = ExportForm::untitled(id, "Untitled", None);
        assert_eq!(form.path, "");
        assert_eq!(form.request(&[]), None);
        assert_eq!(form.refusal(), None);
    }

    #[test]
    fn an_untitled_parts_export_refuses_a_path_that_isnt_absolute() {
        let id = SessionId::parse("s1").unwrap();
        let mut form = ExportForm::untitled(id, "Untitled", None);
        for typed in [
            "out.3mf",
            "sub/out.3mf",
            "./out.3mf",
            "../out.3mf",
            " /abs.3mf",
        ] {
            form.path = typed.to_owned();
            assert_eq!(form.request(&[]), None, "{typed:?}");
            assert_eq!(
                form.refusal(),
                Some("Give a full path: an untitled part has no folder"),
                "{typed:?}"
            );
        }
        // Spaces alone: empty, sending nothing, but not refused for its folder.
        form.path = "   ".to_owned();
        assert_eq!(form.refusal(), None);
        form.path = "/elsewhere/out.3mf".to_owned();
        assert_eq!(form.refusal(), None);
        assert_eq!(
            form.request(&[]).unwrap().path,
            PathBuf::from("/elsewhere/out.3mf")
        );
        // A file's document takes the same relative path, in its folder.
        let mut form = form_of("/work/bracket.silico");
        form.path = "out.3mf".to_owned();
        assert_eq!(form.refusal(), None);
        assert_eq!(
            form.request(&[]).unwrap().path,
            PathBuf::from("/work/out.3mf")
        );
    }

    #[test]
    fn an_untitled_parts_form_takes_its_folder_once_its_first_save_gives_it_one() {
        let mut form = ExportForm::untitled(SessionId::parse("s1").unwrap(), "Untitled", None);
        form.path = "out.3mf".to_owned();
        assert!(form.refusal().is_some());
        form.saved_as("/work/part.silico");
        assert_eq!(form.path, "out.3mf");
        assert_eq!(form.refusal(), None);
        assert_eq!(
            form.request(&[]).unwrap().path,
            PathBuf::from("/work/out.3mf")
        );
    }

    fn form_of(document: &str) -> ExportForm {
        ExportForm::new(SessionId::parse("s1").unwrap(), document)
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

    #[test]
    fn the_tolerances_text_stops_short_of_its_unit_as_the_prototypes_field() {
        use eframe::egui::{Shape, Vec2};
        use egui::accesskit::Role;
        use egui_kittest::Harness;

        use crate::modal::tests::{near, text_shape, widget};

        // `.field`: its 1 px border; `.field input { padding: 0 6px }`, then `.field .unit
        // { padding: 0 6px 0 0 }` at its right: the text stops 6 before the unit, which
        // stops 6 before the border.
        const BORDER: f32 = 1.0;
        const INPUT_PADDING: f32 = 6.0;
        const UNIT_PADDING: f32 = 6.0;
        // Longer than the field, so it's clipped where its room ends.
        const LONG: &str = "0.100000000000000000000000000000000000000000000000000000000000000000000000000000000000";
        let mut h = Harness::builder()
            .with_size(Vec2::new(800.0, 600.0))
            .build_ui_state(
                |ui, form: &mut ExportForm| {
                    let shown = Shown {
                        running: false,
                        replacing: None,
                        said: None,
                        browse: None,
                    };
                    modal::show(ui.ctx(), egui::Id::new("export"), |ui| {
                        form.ui(ui, &[], &shown);
                    });
                },
                ExportForm {
                    tolerance: LONG.to_owned(),
                    ..form()
                },
            );
        theme::apply(&h.ctx);
        h.run_steps(4);
        let field = widget(&h, Role::TextInput, "Tolerance");
        assert!(text_shape(&h, LONG).galley.size().x > field.width());
        let unit = text_shape(&h, "mm");
        let unit_width = unit.galley.size().x;
        assert!(near(
            unit.pos.x,
            field.right() - BORDER - UNIT_PADDING - unit_width
        ));
        let text_right = field.right() - BORDER - UNIT_PADDING - unit_width - INPUT_PADDING;
        // Where the typed text is clipped: egui's text area, 1 wider for its cursor, whose
        // margins are whole points (the unit's room rounded up).
        let clipped: Vec<f32> = h
            .output()
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(shown) if shown.galley.text() == LONG => {
                    Some(clipped.clip_rect.right() - 1.0)
                }
                _ => None,
            })
            .collect();
        assert_eq!(clipped.len(), 1, "the tolerance's text");
        assert!(
            clipped[0] <= text_right && clipped[0] > text_right - 1.0,
            "the text's room ends at {}, not {text_right}",
            clipped[0]
        );
    }
}
