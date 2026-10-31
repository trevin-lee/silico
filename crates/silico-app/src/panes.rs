//! The window's panes, docked with `egui_tiles` as Fusion lays them out (docs/design/m5.0-
//! app.md, section 8; docs/ux/fusion.md, *The window*): the browser at the canvas's left,
//! the canvas (the viewport's place, empty until M5.6), the property panel at its right,
//! and the timeline across the bottom. Each pane is labelled for the accessibility tree
//! (`egui_tiles` labels none), which is how the app's tests find it (decision 7).

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Sense, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType};
use egui_tiles::{Tile, TileId, Tiles, Tree, UiResponse};
use silico_core::command::{SessionId, Sessions};
use silico_core::document::FeatureId;
use silico_core::document::json::Node;
use silico_core::kernel::Clients;
use silico_core::timeline::{Entry, Health};

use crate::tab::{Edit, Tab};
use crate::theme;

/// The property panel's Name field, while it's typed in: the document's feature it's of,
/// its text, the name the document held when it was last drawn, every name it has typed,
/// and the tab's count of resets then ([`crate::tab::Tab::resets`]).
pub struct NameField {
    session: SessionId,
    feature: FeatureId,
    text: String,
    base: String,
    typed: Vec<String>,
    resets: u64,
}

impl NameField {
    /// Whether its text still stands for the feature `feature` of session `session`, whose
    /// name the document holds as `name`: the same feature, no reset since, and the name
    /// unchanged or one this field typed, or the tab still `applying` what was asked (a
    /// name the document passes through on the way isn't the one it comes to).
    fn keeps(
        &self,
        session: SessionId,
        feature: &FeatureId,
        name: &str,
        resets: u64,
        applying: bool,
    ) -> bool {
        self.session == session
            && &self.feature == feature
            && self.resets == resets
            && (applying || self.base == name || self.typed.iter().any(|typed| typed == name))
    }
}

/// A pane of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Browser,
    Viewport,
    Properties,
    Timeline,
}

impl Pane {
    /// Its name, as the accessibility tree labels it.
    pub fn label(self) -> &'static str {
        match self {
            Pane::Browser => "Browser",
            Pane::Viewport => "Viewport",
            Pane::Properties => "Properties",
            Pane::Timeline => "Timeline",
        }
    }
}

/// Fusion's layout: the browser, the canvas, and the property panel side by side, over the
/// timeline across the window's bottom.
pub fn layout() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let browser = tiles.insert_pane(Pane::Browser);
    let viewport = tiles.insert_pane(Pane::Viewport);
    let properties = tiles.insert_pane(Pane::Properties);
    let timeline = tiles.insert_pane(Pane::Timeline);
    let top = tiles.insert_horizontal_tile(vec![browser, viewport, properties]);
    if let Some(Tile::Container(egui_tiles::Container::Linear(linear))) = tiles.get_mut(top) {
        linear.shares.set_share(browser, 1.0);
        linear.shares.set_share(viewport, 3.0);
        linear.shares.set_share(properties, 1.0);
    }
    let root = tiles.insert_vertical_tile(vec![top, timeline]);
    if let Some(Tile::Container(egui_tiles::Container::Linear(linear))) = tiles.get_mut(root) {
        linear.shares.set_share(top, 5.0);
        linear.shares.set_share(timeline, 1.0);
    }
    Tree::new("silico-window", root, tiles)
}

/// What the panes draw from and change: the tab shown, and the sessions its commands go to.
pub struct Panes<'a, P: Clients + 'static> {
    pub tab: Option<&'a mut Tab>,
    pub sessions: &'a Arc<Sessions<P>>,
    /// The property panel's name field, while it's being typed in: the feature, and the
    /// text.
    pub name: &'a mut Option<NameField>,
    /// Whether a command was started: the frame after polls it.
    pub started: &'a mut bool,
}

impl<P: Clients + 'static> egui_tiles::Behavior<Pane> for Panes<'_, P> {
    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: TileId, pane: &mut Pane) -> UiResponse {
        let label = pane.label();
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::Pane);
            node.set_label(label);
        });
        egui::Frame::new()
            .inner_margin(egui::Margin::same(theme::VIEW_PADDING / 2))
            .show(ui, |ui| match pane {
                Pane::Browser => self.browser(ui),
                Pane::Viewport => viewport(ui),
                Pane::Properties => self.properties(ui),
                Pane::Timeline => self.timeline(ui),
            });
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.label().into()
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        1.0
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: false,
            ..Default::default()
        }
    }
}

/// The canvas's place: the viewport comes with M5.6.
fn viewport(ui: &mut Ui) {
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    ui.allocate_rect(rect, Sense::hover());
}

/// What a pane with no timeline yet says: that it's building, or that its rebuild was
/// stopped (the status bar's Rebuild, or Ctrl+B, starts it again).
fn building(tab: &Tab) -> &'static str {
    if tab.rebuild_stopped() {
        "Rebuild stopped"
    } else {
        "Building…"
    }
}

/// A group of rows, labelled for the accessibility tree (the browser's Origin, Bodies, and
/// Sketches, by type, as Fusion's browser groups them).
fn group(ui: &mut Ui, name: &str, rows: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(RichText::new(name).strong())
        .id_salt(name)
        .default_open(true)
        .show(ui, |ui| {
            ui.scope_builder(UiBuilder::new().id_salt(("group", name)), |ui| {
                ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                    node.set_role(egui::accesskit::Role::Group);
                    node.set_label(name);
                });
                rows(ui);
            });
        });
}

/// A feature's health mark (section 9, *Error marks*): an error's or a warning's icon of
/// the app's own, in `theme`'s colour, with its text for the accessibility tree and as its
/// hover text; none for a healthy, suppressed, or unknown feature.
fn mark(ui: &mut Ui, health: Health, text: Option<&str>) {
    let (word, colour) = match health {
        Health::Error => ("error", ui.visuals().error_fg_color),
        Health::Warning => ("warning", ui.visuals().warn_fg_color),
        Health::Healthy | Health::Suppressed | Health::Unknown => return,
    };
    let said = match text {
        Some(text) => format!("{word}: {text}"),
        None => word.to_owned(),
    };
    let side = theme::LIST_ITEM_HEIGHT - 8.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    let painter = ui.painter();
    match health {
        // An error: a filled circle with a bar; a warning: a filled triangle with a bar.
        Health::Error => {
            painter.circle_filled(rect.center(), side / 2.0, colour);
        }
        _ => {
            let points = vec![
                egui::pos2(rect.center().x, rect.top()),
                egui::pos2(rect.right(), rect.bottom()),
                egui::pos2(rect.left(), rect.bottom()),
            ];
            painter.add(egui::Shape::convex_polygon(
                points,
                colour,
                egui::Stroke::NONE,
            ));
        }
    }
    let bar = egui::Rect::from_center_size(rect.center(), Vec2::new(2.0, side * 0.45));
    painter.rect_filled(bar, 0.0, Color32::WHITE);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &said));
    response.on_hover_text(said);
}

impl<P: Clients + 'static> Panes<'_, P> {
    /// The browser (section 8, *The browser*): Origin, Bodies, and Sketches, each row with a
    /// visibility eye, its name, and its mark, from the rows the tab worked out as its
    /// timeline was read ([`crate::tab::browser`]).
    fn browser(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("BROWSER").small().strong());
        let Some(tab) = self.tab.as_deref_mut() else {
            ui.weak("No document open");
            return;
        };
        group(ui, "Origin", |ui| {
            for name in ["Origin", "X", "Y", "Z", "XY", "XZ", "YZ"] {
                row(ui, &mut tab.hidden, name, None, Health::Healthy, None);
            }
        });
        if tab.timeline.is_none() {
            ui.weak(building(tab));
            return;
        }
        let mut selected = tab.selected.clone();
        let crate::tab::Tab {
            browser,
            hidden,
            said,
            ..
        } = &mut *tab;
        for (name, rows) in [("Bodies", &browser.bodies), ("Sketches", &browser.sketches)] {
            group(ui, name, |ui| {
                for each in rows {
                    let text = each
                        .marked_by
                        .as_ref()
                        .and_then(|by| said.get(by.as_str()))
                        .map(|said| said.message.as_str());
                    if row(
                        ui,
                        hidden,
                        &each.name,
                        Some(selected.as_ref() == Some(&each.id)),
                        each.health,
                        text,
                    ) {
                        selected = Some(each.id.clone());
                    }
                }
            });
        }
        if selected != tab.selected
            && let Some(id) = &selected
        {
            // A sketch chosen here fills an open dialog's selection box (section 9).
            tab.pick(id);
        }
        tab.selected = selected;
    }

    /// The timeline (section 8, *The timeline*): the features in order, left to right, each
    /// with its health's mark and its text; suppressed and unknown ones dimmed. A double-click
    /// on one, or Edit Feature in its context menu, opens its dialog (section 9), if a
    /// dialog edits its type.
    fn timeline(&mut self, ui: &mut Ui) {
        let Some(tab) = self.tab.as_deref_mut() else {
            return;
        };
        let Some(timeline) = &tab.timeline else {
            ui.weak(building(tab));
            return;
        };
        let Some(entries) = &timeline.features else {
            ui.weak("The document doesn't read as a document: its problems are in Properties");
            return;
        };
        let mut selected = tab.selected.clone();
        let mut edit = None;
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for entry in entries {
                    let text = match entry.health {
                        Health::Suppressed | Health::Unknown => RichText::new(&entry.name).weak(),
                        _ => RichText::new(&entry.name),
                    };
                    let chosen = selected.as_ref() == Some(&entry.id);
                    let response = ui.selectable_label(chosen, text);
                    if response.clicked() {
                        selected = Some(entry.id.clone());
                    }
                    let editable = crate::dialog::Kind::of(entry.kind).is_some();
                    if response.double_clicked() {
                        edit = Some(entry.id.clone());
                    }
                    response.context_menu(|ui| {
                        if ui
                            .add_enabled(editable, egui::Button::new("Edit Feature"))
                            .clicked()
                        {
                            edit = Some(entry.id.clone());
                            ui.close();
                        }
                    });
                    let said = tab.said.get(entry.id.as_str());
                    mark(ui, entry.health, said.map(|said| said.message.as_str()));
                    ui.separator();
                }
            });
        });
        tab.selected = selected;
        if let Some(id) = edit {
            tab.edit_feature(self.sessions, &id);
            *self.started = true;
        }
    }

    /// The property panel: the feature selected (its name, editable, which is an edit of
    /// the document; its type; its mark's text and suggestion), and the document's problems.
    fn properties(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("PROPERTIES").small().strong());
        let Some(tab) = self.tab.as_deref_mut() else {
            return;
        };
        if !tab.document_problems.is_empty() {
            ui.label(RichText::new("Problems").strong());
            for said in &tab.document_problems {
                ui.colored_label(ui.visuals().error_fg_color, &said.message);
                if let Some(suggestion) = &said.suggestion {
                    ui.weak(suggestion);
                }
            }
            ui.separator();
        }
        let entry: Option<Entry> = tab.selected.as_ref().and_then(|selected| {
            tab.timeline
                .as_ref()
                .and_then(|timeline| timeline.features.as_ref())
                .and_then(|features| features.iter().find(|entry| &entry.id == selected))
                .cloned()
        });
        let Some(entry) = entry else {
            ui.weak("Select a feature on the timeline or in the browser");
            return;
        };
        // The text typed, kept for this document's feature alone (two documents may hold
        // features of one ID), and only while it's ahead of the document: the document's
        // value again once that moves other than by this field's own edits (an undo, a
        // redo, a revert, a reload). A refusal keeps what was typed, the refusal said.
        let mut field = match self.name.take() {
            Some(field)
                if field.keeps(tab.id, &entry.id, &entry.name, tab.resets, tab.applying()) =>
            {
                field
            }
            _ => NameField {
                session: tab.id,
                feature: entry.id.clone(),
                text: entry.name.clone(),
                base: entry.name.clone(),
                typed: Vec::new(),
                resets: tab.resets,
            },
        };
        field.base = entry.name.clone();
        let typed = field.text.clone();
        let mut text = typed.clone();
        ui.horizontal(|ui| {
            let label = ui.label("Name");
            // An ID of its own, the document's and the feature's, so its cursor and focus
            // stay put however the panel above it changes.
            // Off while the tab closes: an edit then would be dropped (#298's review).
            ui.add_enabled(
                !tab.closing(),
                egui::TextEdit::singleline(&mut text).id_salt((
                    "name",
                    tab.id.to_string(),
                    entry.id.as_str(),
                )),
            )
            .labelled_by(label.id);
        });
        if text != typed {
            // Each change an edit, coalesced in the tab (section 3); an empty name too, for
            // the core to refuse and the tab to say why.
            if let Ok(value) = name_value(&text) {
                tab.edit(
                    self.sessions,
                    Edit {
                        feature: entry.id.clone(),
                        field: "name".to_owned(),
                        value,
                    },
                );
                *self.started = true;
            }
        }
        if text != typed {
            field.typed.push(text.clone());
        }
        field.text = text;
        *self.name = Some(field);
        ui.label(format!("Type: {}", entry.kind));
        if let Some(said) = tab.said.get(entry.id.as_str()) {
            let colour = match entry.health {
                Health::Error => ui.visuals().error_fg_color,
                _ => ui.visuals().warn_fg_color,
            };
            ui.colored_label(colour, &said.message);
            if let Some(suggestion) = &said.suggestion {
                ui.weak(suggestion);
            }
        }
    }
}

/// A feature's name as the document reads a string (`set_field`'s value).
fn name_value(name: &str) -> Result<Node, silico_core::document::json::SyntaxError> {
    silico_core::document::json::read(crate::dialog::quoted(name).as_bytes())
}

/// A browser row: its eye (the tab's visibility, never saved), its name (selectable if
/// `selected` is given; whether it was clicked is returned), and its mark.
fn row(
    ui: &mut Ui,
    hidden: &mut std::collections::BTreeSet<String>,
    name: &str,
    selected: Option<bool>,
    health: Health,
    said: Option<&str>,
) -> bool {
    ui.horizontal(|ui| {
        let shown = !hidden.contains(name);
        let toggle = if shown { "Hide" } else { "Show" };
        // The eye, drawn: a filled dot while the row shows, a ring while it's hidden.
        let side = theme::LIST_ITEM_HEIGHT - 10.0;
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
        let colour = ui.visuals().widgets.inactive.fg_stroke.color;
        if shown {
            ui.painter()
                .circle_filled(rect.center(), side / 3.0, colour);
        } else {
            ui.painter()
                .circle_stroke(rect.center(), side / 3.0, egui::Stroke::new(1.0, colour));
        }
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, true, format!("{toggle} {name}"))
        });
        let response = response.on_hover_text(format!("{toggle} {name}"));
        if response.clicked() {
            if shown {
                hidden.insert(name.to_owned());
            } else {
                hidden.remove(name);
            }
        }
        let clicked = match selected {
            Some(selected) => ui.selectable_label(selected, name).clicked(),
            None => {
                ui.label(name);
                false
            }
        };
        mark(ui, health, said);
        clicked
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, kind: &'static str, inputs: &[&str], health: Health) -> Entry {
        Entry {
            id: FeatureId::new(id).unwrap(),
            kind,
            name: id.to_owned(),
            suppressed: false,
            inputs: inputs.iter().map(|i| FeatureId::new(i).unwrap()).collect(),
            health,
            problems: u64::from(health == Health::Error),
            warnings: u64::from(health == Health::Warning),
        }
    }

    fn timeline(entries: &[Entry], bodies: &[&str]) -> silico_core::command::TimelineAnswer {
        silico_core::command::TimelineAnswer {
            revision: silico_core::command::Revision::OPENED,
            model: Some(silico_core::command::Revision::OPENED),
            problems: 0,
            features: Some(entries.to_vec()),
            bodies: bodies.iter().map(|b| FeatureId::new(b).unwrap()).collect(),
        }
    }

    fn rows(browser: &[crate::tab::Row]) -> Vec<(String, Health, Option<String>)> {
        browser
            .iter()
            .map(|row| {
                (
                    row.id.as_str().to_owned(),
                    row.health,
                    row.marked_by.as_ref().map(|by| by.as_str().to_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn the_browsers_bodies_are_the_models_each_marked_by_the_first_worst_of_its_chain() {
        // The M2.1 example with its cut failed: the model's bodies, the block and the pin,
        // whose consumer failed, and nothing more (the cut built none); healthy.
        let entries = [
            entry("k7f2qx3m", "box", &[], Health::Healthy),
            entry("a91cw0de", "cylinder", &[], Health::Healthy),
            entry("m3tq8v0z", "cut", &["k7f2qx3m", "a91cw0de"], Health::Error),
        ];
        let failed = crate::tab::browser(&timeline(&entries, &["k7f2qx3m", "a91cw0de"]));
        assert_eq!(
            rows(&failed.bodies),
            [
                ("k7f2qx3m".to_owned(), Health::Healthy, None),
                ("a91cw0de".to_owned(), Health::Healthy, None)
            ]
        );
        // Built, the cut is the one body, unmarked; a warning upstream marks it, an error
        // before it marks it so instead.
        let mut built = entries.clone();
        built[2].health = Health::Healthy;
        let body = |entries: &[Entry]| {
            rows(&crate::tab::browser(&timeline(entries, &["m3tq8v0z"])).bodies)
        };
        assert_eq!(
            body(&built),
            [("m3tq8v0z".to_owned(), Health::Healthy, None)]
        );
        built[0].health = Health::Warning;
        assert_eq!(
            body(&built),
            [(
                "m3tq8v0z".to_owned(),
                Health::Warning,
                Some("k7f2qx3m".to_owned())
            )]
        );
        built[1].health = Health::Error;
        assert_eq!(
            body(&built),
            [(
                "m3tq8v0z".to_owned(),
                Health::Error,
                Some("a91cw0de".to_owned())
            )]
        );
    }

    #[test]
    fn a_name_reads_back_as_the_string_it_is() {
        for name in [
            "Block",
            "a \"quoted\" name",
            "back\\slash",
            "tab\there",
            "Ω",
        ] {
            let node = name_value(name).unwrap();
            assert_eq!(node.value.as_str(), Some(name), "{name:?}");
        }
    }
}
