//! The window's panes, docked with `egui_tiles` as Fusion lays them out (docs/design/m5.0-
//! app.md, section 8; docs/ux/fusion.md, *The window*): the browser at the canvas's left,
//! the canvas (the viewport's place, empty until M5.6), the property panel at its right,
//! and the timeline across the bottom. Each pane is labelled for the accessibility tree
//! (`egui_tiles` labels none), which is how the app's tests find it (decision 7).

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Sense, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType};
use egui_tiles::{Tile, TileId, Tiles, Tree, UiResponse};
use silico_core::command::Sessions;
use silico_core::document::FeatureId;
use silico_core::document::json::Node;
use silico_core::kernel::Clients;
use silico_core::timeline::{Entry, Health};

use crate::tab::{Edit, Tab};
use crate::theme;

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
    pub name: &'a mut Option<(FeatureId, String)>,
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
    /// visibility eye, its name, and its mark. A body is a chain's last feature, marked by the
    /// worst of its chain; one the model couldn't build (its last feature failed) is listed
    /// too, marked, since a failed feature's mark is on its browser entry (section 9).
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
        let Some(timeline) = tab.timeline.clone() else {
            ui.weak("Building…");
            return;
        };
        let entries = timeline.features.clone().unwrap_or_default();
        let bodies = browser_bodies(&entries, &timeline.bodies);
        let mut selected = tab.selected.clone();
        group(ui, "Bodies", |ui| {
            for &(body, worst) in &bodies {
                let said = worst.and_then(|at| tab.said.get(entries[at].id.as_str()));
                let health = worst.map_or(Health::Healthy, |at| entries[at].health);
                let id = &entries[body].id;
                let name = entries[body].name.clone();
                if row(
                    ui,
                    &mut tab.hidden,
                    &name,
                    Some(selected.as_ref() == Some(id)),
                    health,
                    said.map(|said| said.message.as_str()),
                ) {
                    selected = Some(id.clone());
                }
            }
        });
        group(ui, "Sketches", |ui| {
            for entry in entries.iter().filter(|entry| entry.kind == "sketch") {
                let said = tab.said.get(entry.id.as_str());
                if row(
                    ui,
                    &mut tab.hidden,
                    &entry.name,
                    Some(selected.as_ref() == Some(&entry.id)),
                    entry.health,
                    said.map(|said| said.message.as_str()),
                ) {
                    selected = Some(entry.id.clone());
                }
            }
        });
        tab.selected = selected;
    }

    /// The timeline (section 8, *The timeline*): the features in order, left to right, each
    /// with its health's mark and its text; suppressed and unknown ones dimmed.
    fn timeline(&mut self, ui: &mut Ui) {
        let Some(tab) = self.tab.as_deref_mut() else {
            return;
        };
        let Some(timeline) = &tab.timeline else {
            ui.weak("Building…");
            return;
        };
        let Some(entries) = &timeline.features else {
            ui.weak("The document doesn't read as a document: its problems are in Properties");
            return;
        };
        let mut selected = tab.selected.clone();
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for entry in entries {
                    let text = match entry.health {
                        Health::Suppressed | Health::Unknown => RichText::new(&entry.name).weak(),
                        _ => RichText::new(&entry.name),
                    };
                    let chosen = selected.as_ref() == Some(&entry.id);
                    if ui.selectable_label(chosen, text).clicked() {
                        selected = Some(entry.id.clone());
                    }
                    let said = tab.said.get(entry.id.as_str());
                    mark(ui, entry.health, said.map(|said| said.message.as_str()));
                    ui.separator();
                }
            });
        });
        tab.selected = selected;
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
        let typed = match self.name.take() {
            Some((id, text)) if id == entry.id => text,
            _ => entry.name.clone(),
        };
        let mut text = typed.clone();
        ui.horizontal(|ui| {
            let label = ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut text))
                .labelled_by(label.id);
        });
        if text != typed && !text.is_empty() {
            // Each change an edit, coalesced in the tab (section 3).
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
        *self.name = Some((entry.id.clone(), text));
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
    let mut quoted = String::with_capacity(name.len() + 2);
    quoted.push('"');
    for c in name.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            c if u32::from(c) < 0x20 => quoted.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    silico_core::document::json::read(quoted.as_bytes())
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

/// The bodies the browser lists, in timeline order, each a chain's last feature, its place
/// among `entries`, with the place of the worst-marked feature of its chain (the first in
/// error, else the first with a warning): the model's bodies, and each chain's last feature
/// the model couldn't build, which a failed feature leaves.
fn browser_bodies(entries: &[Entry], bodies: &[FeatureId]) -> Vec<(usize, Option<usize>)> {
    let place = |id: &FeatureId| entries.iter().position(|entry| &entry.id == id);
    let consumed: std::collections::BTreeSet<&FeatureId> = entries
        .iter()
        .filter(|entry| entry.kind != "sketch")
        .flat_map(|entry| entry.inputs.iter())
        .collect();
    let mut listed: Vec<usize> = bodies.iter().filter_map(place).collect();
    for (at, entry) in entries.iter().enumerate() {
        if entry.kind != "sketch"
            && entry.health == Health::Error
            && !consumed.contains(&entry.id)
            && !listed.contains(&at)
        {
            listed.push(at);
        }
    }
    listed.sort_unstable();
    listed
        .into_iter()
        .map(|at| {
            let chain = chain(entries, at);
            let worst = chain
                .iter()
                .copied()
                .find(|&i| entries[i].health == Health::Error)
                .or_else(|| {
                    chain
                        .iter()
                        .copied()
                        .find(|&i| entries[i].health == Health::Warning)
                });
            (at, worst)
        })
        .collect()
}

/// The places of the features `entries[at]` is built from (its inputs, and theirs), with
/// its own, in timeline order.
fn chain(entries: &[Entry], at: usize) -> Vec<usize> {
    let mut found = std::collections::BTreeSet::new();
    let mut to_visit = vec![at];
    while let Some(next) = to_visit.pop() {
        if !found.insert(next) {
            continue;
        }
        for input in &entries[next].inputs {
            if let Some(place) = entries.iter().position(|entry| &entry.id == input) {
                to_visit.push(place);
            }
        }
    }
    found.into_iter().collect()
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

    #[test]
    fn a_failed_last_feature_is_listed_and_a_chain_is_marked_by_its_first_error() {
        // The M2.1 example with its cut failed: the model's bodies are the block and the
        // pin, whose consumer failed; the cut, which built nothing, is listed too, marked.
        let entries = [
            entry("k7f2qx3m", "box", &[], Health::Healthy),
            entry("a91cw0de", "cylinder", &[], Health::Healthy),
            entry("m3tq8v0z", "cut", &["k7f2qx3m", "a91cw0de"], Health::Error),
        ];
        let bodies = [
            FeatureId::new("k7f2qx3m").unwrap(),
            FeatureId::new("a91cw0de").unwrap(),
        ];
        let listed = browser_bodies(&entries, &bodies);
        assert_eq!(listed, [(0, None), (1, None), (2, Some(2))]);
        // Built, the cut is the one body, unmarked.
        let mut built = entries.clone();
        built[2].health = Health::Healthy;
        let cut = [FeatureId::new("m3tq8v0z").unwrap()];
        assert_eq!(browser_bodies(&built, &cut), [(2, None)]);
        // A warning upstream marks the body, an error before it marks it so instead.
        built[0].health = Health::Warning;
        assert_eq!(browser_bodies(&built, &cut), [(2, Some(0))]);
        built[1].health = Health::Error;
        assert_eq!(browser_bodies(&built, &cut), [(2, Some(1))]);
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
