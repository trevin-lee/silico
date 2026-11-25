//! The window's panes, docked with `egui_tiles` as Fusion lays them out (docs/design/m5.0-
//! app.md, section 8; docs/ux/fusion.md, *The window*): the browser at the canvas's left,
//! the canvas (the viewport, `crate::viewport`), and the property panel at its right; the
//! timeline across the bottom is a panel of its own, a fixed 40 high (`crate::timeline`).
//! Each pane is labelled for the accessibility tree (`egui_tiles` labels none), which is
//! how the app's tests find it (decision 7).

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Sense, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType};
use egui_tiles::{Tile, TileId, Tiles, Tree, UiResponse};
use silico_core::command::elements::Entry as ElementEntry;
use silico_core::command::{SessionId, Sessions};
use silico_core::disk::Disk;
use silico_core::document::FeatureId;
use silico_core::document::{Length, Measure, Units};
use silico_core::kernel::Clients;
use silico_core::timeline::{Entry, Health};
use silico_protocol::ElementKind;

use crate::dialog::Action;
use crate::icons::{self, Icon, IconSize};
use crate::measures::Answers;
use crate::model_measures::Answers as ModelAnswers;
use crate::properties;
use crate::selection::{Picked, SelectedAs};
use crate::shortcuts::Command;
use crate::tab::{Hideable, Tab};
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
}

impl Pane {
    /// Its name, as the accessibility tree labels it.
    pub fn label(self) -> &'static str {
        match self {
            Pane::Browser => "Browser",
            Pane::Viewport => "Viewport",
            Pane::Properties => "Properties",
        }
    }
}

/// Fusion's layout: the browser, the canvas, and the property panel side by side (the
/// timeline, a panel of its own, is under them: `crate::timeline`), sized each frame by
/// [`fit`].
pub fn layout() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let browser = tiles.insert_pane(Pane::Browser);
    let viewport = tiles.insert_pane(Pane::Viewport);
    let properties = tiles.insert_pane(Pane::Properties);
    let root = tiles.insert_horizontal_tile(vec![browser, viewport, properties]);
    Tree::new("silico-window", root, tiles)
}

/// The tree's panes sized as `laid` lays out the columns (`crate::columns::fit`: the side
/// panes as dragged, the canvas the rest), each side pane's border, the tiles' 1-point gap
/// (`Behavior::gap_width`), inside its column, so its pane's own rect is the column less
/// that point. Columns wider than the window (its panes at their leasts) run past its right
/// edge, where the property panel is cut off.
///
/// `egui_tiles` shares its width out in proportion to each child's share, so each share
/// is the pane's width itself: shares that sum to the width shared give each its own. A
/// collapsed browser's column is its strip (`crate::columns::fit_strip`), laid out so too.
pub(crate) fn fit(tree: &mut Tree<Pane>, laid: &crate::columns::Laid) {
    use crate::columns::BORDER;
    tree.set_width(laid.browser + laid.canvas + laid.properties);
    let pane = |tree: &Tree<Pane>, pane| tree.tiles.find_pane(&pane);
    let (Some(root), Some(browser), Some(viewport), Some(properties)) = (
        tree.root(),
        pane(tree, Pane::Browser),
        pane(tree, Pane::Viewport),
        pane(tree, Pane::Properties),
    ) else {
        return;
    };
    if let Some(Tile::Container(egui_tiles::Container::Linear(linear))) = tree.tiles.get_mut(root) {
        // The two gaps are the side panes' borders.
        linear.shares.set_share(browser, laid.browser - BORDER);
        linear.shares.set_share(viewport, laid.canvas);
        linear
            .shares
            .set_share(properties, laid.properties - BORDER);
    }
}

/// What the panes draw from and change: the tab shown, and the sessions its commands go to.
pub struct Panes<'a, P: Clients + 'static> {
    pub tab: Option<&'a mut Tab>,
    pub sessions: &'a Arc<Sessions<P>>,
    /// The property panel's name field, while it's being typed in: the feature, and the
    /// text.
    pub name: &'a mut Option<NameField>,
    /// Whether the frame after is to be asked for (`App::keys`' meaning): a command started,
    /// which it polls, or a key taken, whose change it draws.
    pub next_frame: &'a mut bool,
    /// What the viewport's renderer holds, and how its frames reach it.
    pub drawing: &'a mut crate::viewport::Drawing,
    /// Enter or Esc pressed this frame, if it's an open dialog's (`App::keys`).
    pub dialog_key: Option<egui::Key>,
    /// What each kind of dialog was last OK'd with: the selection view's Fillet opens its
    /// dialog with them, as the toolbar's does.
    pub remembered: &'a crate::dialog::Remembered,
    /// egui's fade of a disabled `Ui`, as the style has it: what's off of its own in a pane
    /// fades by, behind a modal too (`crate::modal::unfaded`).
    pub fade: f32,
    /// Whether the browser is collapsed to its strip (`#main.browser-collapsed`).
    pub browser_collapsed: bool,
    /// Whether the browser's chevron was clicked this frame, to collapse it or show it
    /// (interact.js's `toggleBrowser`).
    pub browser_toggled: &'a mut bool,
}

impl<P: Clients + 'static> egui_tiles::Behavior<Pane> for Panes<'_, P> {
    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: TileId, pane: &mut Pane) -> UiResponse {
        ui.visuals_mut().disabled_alpha = self.fade;
        let label = pane.label();
        let rect = ui.max_rect();
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::Pane);
            node.set_label(label);
            node.set_bounds(bounds(rect));
        });
        if *pane == Pane::Viewport {
            // The canvas reaches the pane's edges.
            crate::viewport::show(
                ui,
                self.tab.as_deref_mut(),
                self.sessions,
                self.drawing,
                self.remembered,
            );
            return UiResponse::None;
        }
        if *pane == Pane::Browser {
            // Its rows' fills reach the pane's edges; each part pads itself.
            self.browser(ui);
            return UiResponse::None;
        }
        // Its sections' rules and fills reach the pane's edges; each part pads itself.
        self.properties(ui);
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.label().into()
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        crate::columns::BORDER
    }

    fn is_container_resizable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        // The side panes' borders are dragged by `crate::columns`' handles, not by the
        // tiles' splitters (its module says why): the tiles draw each border, no more.
        false
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: false,
            ..Default::default()
        }
    }
}

/// Any place on the screen, `rect` (a pane's, a floating panel's, a tab's name), as the
/// accessibility tree gives it: in points, as egui's widgets give theirs.
pub(crate) fn bounds(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// What a pane with no timeline yet says: that it's building, or that its rebuild was
/// stopped (the status bar's Rebuild, or Ctrl+B, starts it again).
pub(crate) fn building(tab: &Tab) -> &'static str {
    if tab.rebuild_stopped() {
        "Rebuild stopped"
    } else {
        "Building…"
    }
}

impl<P: Clients + 'static> Panes<'_, P> {
    /// The browser (section 8, *The browser*; DESIGN.md, *egui mapping*): its title with
    /// its chevron, Collapse browser; then the document's row, and under it Origin, Bodies,
    /// and Sketches, by type as Fusion groups them, each under a header with its caret; each
    /// row with its eye (the tab's visibility, never saved), its icon, its name, its meta at
    /// its right, and its mark, from the rows the tab worked out as its timeline was read
    /// ([`crate::tab::browser`]). Collapsed, its strip and the one chevron there, Show
    /// browser ([`Self::strip`]).
    fn browser(&mut self, ui: &mut Ui) {
        if self.browser_collapsed {
            self.strip(ui);
            return;
        }
        // `.panel-body` straight under `.panel-title`, as each row under the last.
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        // app.js's `renderBrowser`: `aria-label="Collapse browser" data-tip="Collapse"`, the
        // chevron as it's drawn, unturned.
        let collapse = properties::PanelButton {
            icon: Icon::Chevron,
            label: "Collapse browser",
            tip: "Collapse".to_owned(),
        };
        if properties::title_with(ui, None, "Browser", Some(collapse)) {
            *self.browser_toggled = true;
        }
        let Some(tab) = self.tab.as_deref_mut() else {
            padded(ui, |ui| {
                ui.weak("No document open");
            });
            return;
        };
        // Shown: a Find in browser's row is scrolled to below, as it is.
        tab.reveal_browser = false;
        let mut asked = Asked::default();
        let browsers = |renaming: &crate::feature_menu::Renaming| {
            renaming.place == crate::feature_menu::Place::Browser
        };
        let shown = tab.selection().scroll_target().cloned().or_else(|| {
            tab.renaming
                .as_ref()
                .filter(|renaming| browsers(renaming))
                .map(|renaming| renaming.feature.clone())
        });
        if let Some(shown) = shown {
            // A body picked in the canvas, or a row found from the timeline or renamed:
            // its row shown, its group opened (section 9, *Browser and canvas*: the browser
            // expands and scrolls to it).
            let group = if tab.browser.sketches().iter().any(|row| row.id == shown) {
                "Sketches"
            } else {
                "Bodies"
            };
            for key in ["root", group] {
                Openness::load(ui.ctx(), key).opened(ui.ctx());
            }
        }
        // The rename field's, taken while the rows are drawn, as they read the tab.
        let mut renaming = tab.renaming.take();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                tree(ui, tab, &mut asked, &mut renaming);
                ui.add_space(theme::BROWSER_PADDING_BOTTOM);
            });
        // Scrolled to once, whether its row was there or not.
        tab.selection_mut().scrolled();
        // A field whose row is gone (its feature deleted, its body consumed) goes with it.
        if renaming.as_ref().is_some_and(browsers) && !asked.renaming {
            renaming = None;
        }
        if asked.renamed.is_some() {
            renaming = None;
        }
        if tab.renaming.is_none() {
            tab.renaming = renaming;
        }
        if let Some((feature, typed)) = asked.renamed {
            *self.next_frame |= tab.finish_rename(self.sessions, feature, &typed);
        }
        for what in asked.toggled {
            tab.toggle_shown(what);
        }
        if let Some(shown) = asked.planes_shown {
            tab.show_origin_planes(shown);
        }
        if let Some(plane) = asked.plane_picked {
            tab.pick_origin_plane(self.sessions, plane);
            *self.next_frame = true;
        }
        if tab.view.plane_hovered != asked.plane_hovered {
            tab.view.plane_hovered = asked.plane_hovered;
            // The canvas, drawn before the browser or after it, draws it from the next frame,
            // asked for at once.
            ui.ctx().request_repaint();
        }
        if let Some((id, how)) = asked.selected {
            row_clicked(tab, id, how, Click::Left);
        }
        if let Some(id) = asked.deselected {
            if tab.pick_body(&id) {
                // A dialog's box takes the row, as a plain click's does.
                tab.selected_in = crate::feature_menu::Place::Browser;
            } else if !tab.in_dialog() {
                tab.selection_mut().deselect_feature(&id);
            }
        }
        if let Some((id, how)) = asked.menu_selected {
            // Selected as its menu opens, by both panes' rule (`Tab::menu_selects`).
            row_clicked(tab, id, how, Click::Menu);
        }
        if let Some(act) = asked.act {
            *self.next_frame |= crate::feature_menu::act(tab, self.sessions, act);
        }
        if let Some(id) = asked.edit {
            // Edit Sketch for a sketch, a feature's dialog for any other.
            tab.edit_sketch_or_feature(self.sessions, &id);
            *self.next_frame = true;
        }
        if asked.root {
            // The document's row (§5: "a click on the tree's top node lists them all"):
            // What's Wrong if anything is, else the document's own properties, as the
            // prototype's `rootRow`.
            if tab.anything_wrong() {
                tab.whats_wrong = true;
            } else {
                tab.selection_mut().clear();
            }
        }
    }

    /// The browser collapsed to its strip (`#main.browser-collapsed`; app.js's
    /// `renderBrowser`): its one button, Show browser (`.ab-btn`, `margin: 2px`), its chevron
    /// as the title's; nothing of the tree, which shows again with the browser. A body
    /// picked in the canvas meanwhile isn't scrolled to later: its row is "scrolled to once,
    /// whether its row was there or not". Find in browser shows it again, its row scrolled
    /// to once it's shown, as SolidWorks and Fusion reveal their tree. A name being typed
    /// over a row goes, sending nothing, as it goes with a row that's gone.
    fn strip(&mut self, ui: &mut Ui) {
        let side = theme::APP_BAR_BUTTON_SIZE;
        let place = egui::Rect::from_min_size(
            ui.max_rect().min + Vec2::splat(theme::BROWSER_STRIP_BUTTON_MARGIN),
            Vec2::splat(side),
        );
        let show = properties::PanelButton {
            icon: Icon::Chevron,
            label: "Show browser",
            tip: "Show browser".to_owned(),
        };
        let shown = ui
            .scope_builder(UiBuilder::new().max_rect(place), |ui| {
                properties::panel_button(ui, side, &show)
            })
            .inner;
        if shown.clicked() {
            *self.browser_toggled = true;
        }
        let Some(tab) = self.tab.as_deref_mut() else {
            return;
        };
        if tab
            .renaming
            .as_ref()
            .is_some_and(|renaming| renaming.place == crate::feature_menu::Place::Browser)
        {
            tab.renaming = None;
        }
        if tab.reveal_browser {
            // Shown from the next frame, which scrolls to the row.
            tab.reveal_browser = false;
            *self.browser_toggled = true;
        } else {
            tab.selection_mut().scrolled();
        }
    }

    /// The property panel (DESIGN.md, *Layout*: one right panel, four jobs): the feature
    /// dialog, if one is open; else, while a sketch is open, sketch mode's own
    /// ([`sketch_mode`]); else the selection's faces, edges, and vertices, if it has
    /// any, with their measures (`selectionProps`, [`selected_elements`]), and what its link
    /// and buttons ask done; else the selected feature's properties; else the document's,
    /// never an empty state.
    fn properties(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let Some(tab) = self.tab.as_deref_mut() else {
            properties::title(ui, None, "Properties", None);
            properties::hint(
                ui,
                &[(
                    "Properties of what you select appear here. Feature dialogs dock here too.",
                    false,
                )],
            );
            return;
        };
        if tab.dialog.is_some() {
            self.dialog(ui);
            return;
        }
        if self.dialog_key == Some(egui::Key::Escape) && tab.in_dialog() {
            // A dialog still being read (Edit Feature): Esc cancels it.
            tab.close_dialog(self.sessions, false);
        }
        if tab.sketch.is_some() {
            // Sketch mode's own (app.js's `renderRight`: `S.sketch` before the selection).
            properties::title(ui, None, "Properties", None);
            let asked = egui::ScrollArea::vertical()
                .id_salt("properties")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    sketch_mode(ui, tab)
                })
                .inner;
            if let Some(asked) = asked {
                *self.next_frame |= tab.card_asked(self.sessions, asked);
            }
            return;
        }
        // The selection's views close with Clear selection (`selectionProps`' `clearSel`).
        let selecting = tab.selection().elements().next().is_some();
        let clear = selecting.then_some((CLEAR_SELECTION, Command::ClearSelection));
        if properties::title(ui, None, "Properties", clear) {
            tab.selection_mut().clear();
        }
        let entry: Option<Entry> = tab.selection().feature().and_then(|selected| {
            tab.timeline
                .as_ref()
                .and_then(|timeline| timeline.features.as_ref())
                .and_then(|features| features.iter().find(|entry| &entry.id == selected))
                .cloned()
        });
        let sessions = self.sessions;
        let name = &mut *self.name;
        let next_frame = &mut *self.next_frame;
        let elements = tab.selection().elements().next().is_some();
        if elements {
            // A pick made this frame (the canvas is drawn before this pane) is measured from
            // this frame, not the next.
            tab.measure(sessions);
        } else if entry.is_none() {
            // The document's view: its model's measures read, if they're not yet.
            let now = self.drawing.now();
            tab.measure_model(sessions, now);
            // A read refused is asked again when its delay is up: a frame then.
            if let Some(wait) = tab.model_measures().wake_in(now) {
                ui.ctx().request_repaint_after(wait);
            }
        }
        let mut viewed = None;
        let out = egui::ScrollArea::vertical()
            .id_salt("properties")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                match entry {
                    _ if elements => {
                        viewed = selected_elements(ui, tab);
                        properties::CardOut::default()
                    }
                    Some(entry) => feature(ui, tab, &entry, sessions, name, next_frame),
                    None => document(ui, tab),
                }
            })
            .inner;
        tab.hover_candidate(out.hovered);
        if let Some(asked) = out.asked {
            *self.next_frame |= tab.card_asked(sessions, asked);
        }
        match viewed {
            None => {}
            Some(Viewed::Select(id)) => tab.selection_mut().select_feature(id),
            Some(Viewed::Edit(id)) => {
                // Never a sketch's: an element's name names the feature that made it in a
                // solid, which a sketch never is.
                tab.edit_feature(sessions, &id);
                *self.next_frame = true;
            }
            Some(Viewed::Fillet) => {
                tab.open_dialog(sessions, crate::dialog::Kind::Fillet, self.remembered);
                *self.next_frame = true;
            }
            Some(Viewed::SketchOn(face)) => {
                tab.create_sketch_on_face(sessions, &face.to_string());
                *self.next_frame = true;
            }
        }
    }

    /// The open dialog, docked in the property panel (open item 27): drawn, what's changed
    /// in it sent, and its OK or Cancel taken, or the key pressed: Enter is OK, if OK is on;
    /// Esc goes to the first of these there is (section 3): an arrow's press held in the
    /// canvas, which it ends, a drag where it began; a dialog's preview rebuilding, which it
    /// stops; the dialog left open by either; or the dialog, which it cancels
    /// ([`Tab::dialog_escape`]).
    fn dialog(&mut self, ui: &mut Ui) {
        let sessions = self.sessions;
        let key = self.dialog_key;
        let Some(tab) = self.tab.as_deref_mut() else {
            return;
        };
        let refused: Vec<&'static str> = tab
            .dialog
            .iter()
            .flat_map(|dialog| dialog.inputs.iter().map(|input| input.part.field()))
            .filter(|field| tab.field_refused(field))
            .collect();
        let said: Vec<_> = tab
            .dialog_said()
            .into_iter()
            .map(|said| {
                let name = said
                    .feature()
                    .and_then(|id| tab.entry(id))
                    .map(|entry| entry.name.clone());
                let repair = tab.repair(&said);
                (said, name, repair)
            })
            .collect();
        let preview = preview_of(tab);
        let Some(mut dialog) = tab.dialog.take() else {
            return;
        };
        let enabled = !tab.closing();
        let title = dialog.kind.title();
        // A feature's name as the timeline lists it: a sweep's sketch, a boolean's body (one
        // its own feature consumes too, which the browser doesn't list).
        let entries = tab
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.features.as_deref())
            .unwrap_or_default();
        let named = |id: &FeatureId| {
            entries
                .iter()
                .find(|entry| &entry.id == id)
                .map(|entry| entry.name.clone())
        };
        let refused = |field: &str| refused.contains(&field);
        let mut cards = properties::Cards {
            said,
            out: properties::CardOut::default(),
        };
        let mut action = ui
            .scope_builder(UiBuilder::new().id_salt(("dialog", dialog.serial)), |ui| {
                ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                    node.set_role(egui::accesskit::Role::Dialog);
                    node.set_label(title);
                });
                dialog.ui(
                    ui,
                    &mut cards,
                    preview,
                    &named,
                    &refused,
                    crate::dialog::Allowed {
                        enabled,
                        ok: &|dialog: &crate::dialog::Dialog| tab.ok_for(dialog),
                    },
                )
            })
            .inner;
        tab.dialog = Some(dialog);
        tab.hover_candidate(cards.out.hovered);
        if let Some(asked) = cards.out.asked {
            *self.next_frame |= tab.card_asked(sessions, asked);
        }
        *self.next_frame |= tab.dialog_changed(sessions);
        // The button's click, asked of the inputs before this frame's edits were sent, taken
        // as Enter is: only if OK still holds after them.
        if action == Action::Ok && !tab.can_ok() {
            action = Action::None;
        }
        match key {
            Some(egui::Key::Enter) if tab.can_ok() => action = Action::Ok,
            Some(egui::Key::Escape) if tab.dialog_escape(sessions) => action = Action::Cancel,
            _ => {}
        }
        match action {
            Action::None => {}
            Action::Ok => tab.close_dialog(sessions, true),
            Action::Cancel => tab.close_dialog(sessions, false),
        }
        *self.next_frame |= action != Action::None;
    }
}

/// Sketch mode's property panel (sketch.js's `renderProps`): its head, the sketch's icon,
/// its name as the sketch bar gives it, and `Sketch · ` its ID, or `new` until its add
/// answers; the SKETCH section's rows, once it's read ([`sketch_mode_rows`]); the card for
/// a sketch over defined, or one whose solve was refused ([`sketch::problem_card`]); and
/// the prototype's two hints, on dragging and on Finish and Cancel. The head's lines are
/// labels in the accessibility tree, as the selection's are.
///
/// [`sketch_mode_rows`]: crate::parameters::sketch_mode_rows
/// [`sketch::problem_card`]: crate::sketch::problem_card
fn sketch_mode(ui: &mut Ui, tab: &Tab) -> Option<properties::CardAct> {
    let mode = tab.sketch.as_ref()?;
    let name = tab.sketch_name();
    let id = mode
        .sketch
        .as_ref()
        .map_or_else(|| "new".to_owned(), |id| id.as_str().to_owned());
    let mut job = properties::body("Sketch · ", theme::PROPERTY_HEAD_SUB);
    job.sections.iter_mut().for_each(|section| {
        section.format.font_id.size = theme::HEADER_SIZE;
    });
    // The ID in the monospace type (`.mono`); `new` in the sub-line's own.
    let id_format = match mode.sketch {
        Some(_) => properties::format(theme::MONOSPACE_SIZE, theme::PROPERTY_HEAD_SUB, true),
        None => properties::format(theme::HEADER_SIZE, theme::PROPERTY_HEAD_SUB, false),
    };
    job.append(&id, 0.0, id_format);
    let sub = format!("Sketch · {id}");
    let lines = properties::head(ui, Icon::Sketch, &name, job);
    for (line, said) in lines.into_iter().zip([name.as_str(), sub.as_str()]) {
        let response = ui.interact(line, ui.id().with(("sketch-head", said)), Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, said));
    }
    let read = mode.drawable();
    if let Some(read) = read {
        properties::section(ui, "Sketch");
        for row in crate::parameters::sketch_mode_rows(read) {
            // No row of a sketch's is a link.
            let _ = draw_row(ui, row);
        }
    }
    let mut asked = None;
    if let Some(said) = read.and_then(|read| crate::sketch::problem_card(read, &name)) {
        let offer = properties::Offer {
            edit: None,
            show: false,
        };
        let driven = tab.sketch_make_driven_offer();
        asked = ui
            .push_id("sketch-card", |ui| {
                properties::driven_card(ui, &said, Some(&name), offer, driven.as_ref())
            })
            .inner;
    }
    properties::hint(
        ui,
        &[(
            "Drag a blue point or curve to move it: what's fully defined stays put. Double-click a dimension to change it.",
            false,
        )],
    );
    let keys = Command::FinishSketch.keys_text(ui.ctx());
    properties::hint(
        ui,
        &[
            ("Finish Sketch", true),
            (&format!(" {keys} keeps your changes and rebuilds. "), false),
            ("Cancel", true),
            (" drops them.", false),
        ],
    );
    asked
}

/// A row of PARAMETERS, or of sketch mode's SKETCH, drawn: words in their colour, an ID in
/// the monospace type, or a link to a feature with words after it. The feature its link
/// asked selected, if it was clicked.
fn draw_row(ui: &mut Ui, row: crate::parameters::Row) -> Option<FeatureId> {
    use crate::parameters::Shown;
    match row.value {
        Shown::Text(words, colour) => {
            properties::text_row(ui, row.key, properties::body(&words, colour));
            None
        }
        Shown::Id(id) => {
            properties::text_row(
                ui,
                row.key,
                egui::text::LayoutJob::single_section(
                    id,
                    properties::format(theme::MONOSPACE_SIZE, theme::PROPERTY_VALUE, true),
                ),
            );
            None
        }
        Shown::Link {
            feature,
            name,
            after,
        } => properties::link_row_then(ui, row.key, &name, after.as_deref()).then_some(feature),
    }
}

/// What `tab`'s open dialog's preview line says (`renderDialog`'s PREVIEW, state 3e): none
/// before its feature is there (a new one's add unanswered); else its rebuild running (Esc
/// stops it), stopped, or done, with the errors the timeline counts, if any, or the core's
/// time for its rebuild.
fn preview_of(tab: &Tab) -> Option<properties::Preview> {
    use silico_core::command::RebuildState;
    tab.dialog.as_ref()?.feature.as_ref()?;
    let rebuild = &tab.status.as_ref()?.rebuild;
    if tab.preview_rebuilding() {
        let progress = rebuild
            .progress
            .as_ref()
            .map(|progress| (progress.done, progress.steps));
        return Some(properties::Preview::Running(progress));
    }
    if tab.rebuild_stopped() {
        return Some(properties::Preview::Stopped);
    }
    (rebuild.state == RebuildState::Done && rebuild.revision == tab.revision).then(|| {
        match tab.errors() {
            0 => properties::Preview::UpToDate {
                revision: rebuild.revision.number(),
                ms: tab.rebuilt_ms(),
            },
            errors => properties::Preview::Errors(errors),
        }
    })
}

/// The selected feature's properties (`featureProps`): its head (its type's icon, its name,
/// its type and ID), the document's problems and its own (its error card, with Edit, as
/// `featureProps` offers it, `noShow`), its name, editable, which is an edit of the
/// document; its PARAMETERS ([`parameters`]); its health and ID, with the ID's copy button
/// (`prop('ID', …, { copy })`); its Edit feature (Edit Sketch for a sketch) and Suppress
/// (Unsuppress for one suppressed), each its feature menu's item, done as the menu does it,
/// off with the menu's reason; and, if another program changed it, what changed (the info
/// Changed on disk card, with Mark as seen). What an error card's button asked, if one
/// was.
fn feature<P: Clients + 'static>(
    ui: &mut Ui,
    tab: &mut Tab,
    entry: &Entry,
    sessions: &Arc<Sessions<P>>,
    name: &mut Option<NameField>,
    next_frame: &mut bool,
) -> properties::CardOut {
    let mut sub = properties::body(
        &format!("{} · ", crate::timeline::kind_words(entry.kind)),
        theme::PROPERTY_HEAD_SUB,
    );
    sub.sections.iter_mut().for_each(|section| {
        section.format.font_id.size = theme::HEADER_SIZE;
    });
    sub.append(
        entry.id.as_str(),
        0.0,
        properties::format(theme::MONOSPACE_SIZE, theme::PROPERTY_HEAD_SUB, true),
    );
    properties::head(ui, Icon::of_kind(entry.kind), &entry.name, sub);
    let mut asked = problems(ui, tab);
    // Coloured by the problem's own severity, as the prototype's `errCard` is, not by
    // the feature's health in the timeline.
    if let Some(said) = tab.said.get(entry.id.as_str()).cloned() {
        let offer = properties::Offer {
            edit: Some(tab.can_edit(&entry.id)),
            show: false,
        };
        let repair = tab.repair(&said);
        asked = asked.or(ui
            .push_id("feature-card", |ui| {
                properties::repair_card(ui, &said, Some(&entry.name), offer, repair.as_ref())
            })
            .inner);
    }
    // Changed by another program (`d.outside`): what changed, until it's marked seen.
    if let Some(text) = tab.outside.get(&entry.id).cloned()
        && properties::outside_card(ui, &text)
    {
        tab.outside.remove(&entry.id);
    }
    // The text typed, kept for this document's feature alone (two documents may hold
    // features of one ID), and only while it's ahead of the document: the document's
    // value again once that moves other than by this field's own edits (an undo, a
    // redo, a revert, a reload). A refusal keeps what was typed, the refusal said.
    let mut field = match name.take() {
        Some(field) if field.keeps(tab.id, &entry.id, &entry.name, tab.resets, tab.applying()) => {
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
    properties::row(ui, "Name", |ui, label| {
        // An ID of its own, the document's and the feature's, so its cursor and focus
        // stay put however the panel above it changes.
        // Off while the tab closes: an edit then would be dropped (#298's review).
        let edit = egui::TextEdit::singleline(&mut text);
        let salt = ("name", tab.id.to_string(), entry.id.as_str());
        let edit = theme::field(ui, edit, salt, theme::FieldKind::Dialog, false);
        let at = properties::field_rect(ui, ui.available_width());
        ui.add_enabled_ui(!tab.closing(), |ui| ui.put(at, edit))
            .inner
            .labelled_by(label);
    });
    if text != typed {
        // Each change an edit, coalesced in the tab (section 3); an empty name too, for
        // the core to refuse and the tab to say why.
        tab.rename(sessions, entry.id.clone(), &text);
        *next_frame = true;
        field.typed.push(text.clone());
    }
    field.text = text;
    *name = Some(field);
    if let Some(id) = parameters(ui, tab, entry, sessions) {
        tab.selection_mut().select_feature(id);
        *next_frame = true;
    }
    properties::section(ui, "Health");
    let (state, colour) = match entry.health {
        Health::Healthy => ("Healthy", theme::SUCCESS),
        Health::Warning => ("Warning", theme::WARNING),
        Health::Error => ("Error", theme::ERROR_TEXT),
        Health::Suppressed => ("Suppressed", theme::PROPERTY_VALUE),
        Health::Unknown => ("Not built", theme::TEXT_SUBDUED),
    };
    properties::text_row(ui, "State", properties::body(state, colour));
    properties::copy_row(ui, "ID", entry.id.as_str());
    // `featureProps`' buttons: Edit (`btn-secondary`, the pencil) and Suppress
    // (`btn-ghost`), the feature menu's own items, words, and reasons, done through its
    // `act`, so each does what its menu item does.
    use crate::feature_menu::{self, Item};
    let place = tab.selected_in;
    let items = [Item::Edit, Item::Suppress];
    let offs = items.map(|item| feature_menu::off(tab, entry, item, place));
    let buttons = [
        crate::modal::Button::new(
            feature_menu::label(Item::Edit, entry, place),
            crate::modal::Kind::Secondary,
        )
        .icon(Icon::Edit)
        .enabled(offs[0].is_none()),
        crate::modal::Button::new(
            feature_menu::label(Item::Suppress, entry, place),
            crate::modal::Kind::Ghost,
        )
        .enabled(offs[1].is_none()),
    ];
    let reasons = offs.each_ref().map(Option::as_deref);
    if let Some(item) =
        properties::buttons_off(ui, &buttons, &reasons).and_then(|at| items.get(at).copied())
    {
        let act = feature_menu::act_of(item, entry, place);
        *next_frame |= feature_menu::act(tab, sessions, act);
    }
    properties::hint(
        ui,
        &[(
            "Hovering a feature in the timeline lights up the faces it made.",
            false,
        )],
    );
    asked
}

/// The selected feature's PARAMETERS (`featureProps`' `section('Parameters')` and its
/// rows, [`crate::parameters`]): a sketch's from its read, any other's from its text at the
/// tab's revision, read here off the UI thread, the rows as last read shown meanwhile; no
/// section while there are none. The feature a Profile link asked selected, if one was
/// clicked.
fn parameters<P: Clients + 'static>(
    ui: &mut Ui,
    tab: &mut Tab,
    entry: &Entry,
    sessions: &Arc<Sessions<P>>,
) -> Option<FeatureId> {
    use crate::parameters::{Context, feature_rows, has_parameters, sketch_rows};
    if !has_parameters(entry.kind) {
        return None;
    }
    let rows = if entry.kind == "sketch" {
        sketch_rows(tab.sketch_reads.held(&entry.id).map(|held| &*held.read))
    } else {
        let (session, revision) = (tab.id, tab.revision);
        tab.parameters.poll(sessions, &session, &entry.id, revision);
        let tab = &*tab;
        let name = |id: &FeatureId| tab.entry(id.as_str()).map(|entry| entry.name.clone());
        let sketch = |id: &FeatureId| tab.sketch_reads.held(id).map(|held| &*held.read);
        let context = Context {
            name: &name,
            sketch: &sketch,
        };
        tab.parameters
            .node(&entry.id)
            .map(|node| feature_rows(entry.kind, node, &context))
            .unwrap_or_default()
    };
    // No section with no rows (a suppressed sketch, never read); but while a feature's text
    // is read, the room its kind's rows last took, so HEALTH and the buttons under it stay
    // where they'll be once it answers.
    if rows.is_empty() {
        let reading = entry.kind != "sketch" && tab.parameters.node(&entry.id).is_none();
        if let Some(kept) = tab
            .parameters
            .last_shown(entry.kind)
            .filter(|kept| reading && *kept > 0)
        {
            properties::section(ui, "Parameters");
            ui.allocate_exact_size(
                Vec2::new(
                    ui.available_width(),
                    kept as f32 * theme::PROPERTY_ROW_HEIGHT,
                ),
                Sense::hover(),
            );
        }
        return None;
    }
    tab.parameters.showed(entry.kind, rows.len());
    properties::section(ui, "Parameters");
    let mut asked = None;
    for row in rows {
        asked = draw_row(ui, row).or(asked);
    }
    asked
}

/// What the selection view asked (`selectionProps`' link and buttons): a feature selected
/// (Made by), its dialog opened (Edit), Fillet started with the selection, or a sketch
/// created on the planar face a store names so (Sketch on this face).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Viewed {
    Select(FeatureId),
    Edit(FeatureId),
    Fillet,
    SketchOn(Arc<silico_core::naming::Name>),
}

/// The selection's faces, edges, and vertices (`selectionProps`, M5.7): one element's view
/// ([`one_selected`]), or several's ([`several_selected`]), from the core's answers for the
/// selection as it is now ([`crate::measures`]), never another's. What it asked, if
/// anything.
fn selected_elements(ui: &mut Ui, tab: &Tab) -> Option<Viewed> {
    let elements: Vec<&Picked> = tab.selection().elements().collect();
    let none = Answers::default();
    let answers = crate::measures::Of::now(tab.selection())
        .and_then(|of| tab.measures().answers(of))
        .unwrap_or(&none);
    match elements.as_slice() {
        [one] => one_selected(ui, tab, one, answers),
        several => {
            several_selected(ui, several, answers);
            None
        }
    }
}

/// The selection views' close button's label and tooltip (`selectionProps`' `clearSel`).
const CLEAR_SELECTION: &str = "Clear selection";

/// What the property panel says of a measure not answered yet: words, never a dash, as the
/// panel says the file on disk isn't compared yet.
const NOT_MEASURED: &str = "Not measured yet";

/// What it says of one element's measure while its feature doesn't build: not measured,
/// for as long as that lasts, never "not measured yet" (its Stability card says why).
const NOT_BUILT: &str = "Not measured: its feature didn't build";

/// The planar face's button that creates a sketch on it (`selectionProps`' `sketchOnFace`).
pub(crate) const SKETCH_ON_FACE: &str = "Sketch on this face";

/// One element selected (`selectionProps`' first view): its head (its kind's icon, its
/// number in its body's solid, and what it is, on which body); MEASURES, from its entry
/// as `elements` lists it ([`measure_rows`]); NAME: its name as an agent writes it, with a
/// copy button, the feature that made it, a link that selects it, and its stability, as
/// [`stability`] says it; then Sketch on this face for a planar face (Create Sketch on it,
/// [`Tab::create_sketch_on_face`]), Fillet for an edge (the toolbar's, with the selection),
/// and Edit for its maker (Edit Feature's), and the hint.
fn one_selected(ui: &mut Ui, tab: &Tab, picked: &Picked, answers: &Answers) -> Option<Viewed> {
    use silico_core::command::elements::ElementType;
    let found = picked.found();
    let kind = found.map(silico_core::selection::Selected::kind);
    let entry = answers
        .element
        .as_ref()
        .and_then(|entry| entry.as_ref().ok());
    let icon = kind.map_or(Icon::SelFace, Icon::of_element);
    let title = found.map_or_else(|| "Element".to_owned(), |found| found.element.to_string());
    let what = element_words(kind, entry);
    let sub = match found {
        Some(found) => format!("{what} · {}", body_name(tab, &found.feature)),
        None => what,
    };
    element_head(ui, icon, &title, &sub);
    properties::section(ui, "Measures");
    measure_rows(ui, picked, kind, answers.element.as_ref());
    properties::section(ui, "Name");
    properties::copy_row(ui, "Stable name", &picked.name);
    let maker = found.map(|found| found.name.feature().clone());
    let mut viewed = None;
    if let Some(maker) = &maker
        && properties::link_row(ui, "Made by", &tab.name_of(maker))
    {
        viewed = Some(Viewed::Select(maker.clone()));
    }
    stability(ui, picked);
    let edit = maker
        .as_ref()
        .map(|maker| format!("Edit {}", tab.name_of(maker)));
    let mut buttons = Vec::new();
    let mut acts = Vec::new();
    // A face `elements` says is a plane (`selectionProps`' `sketchOnFace`), by the name a
    // store writes for it (`Selected::stored`), as every store takes a selection's: off
    // while none of its names is stable (its Stability card says why), or while sketch
    // mode can't open.
    if kind == Some(ElementKind::Face)
        && entry.is_some_and(|entry| entry.r#type == ElementType::Plane)
    {
        let stored = found.and_then(|found| found.stored.as_ref().ok());
        buttons.push(
            crate::modal::Button::new(SKETCH_ON_FACE, crate::modal::Kind::Secondary)
                .icon(Icon::Sketch)
                .enabled(stored.is_some() && tab.can_sketch()),
        );
        acts.push(stored.map(|face| Viewed::SketchOn(Arc::clone(face))));
    }
    if kind == Some(ElementKind::Edge) {
        buttons.push(
            crate::modal::Button::new("Fillet", crate::modal::Kind::Secondary)
                .icon(Icon::Fillet)
                .enabled(tab.can_open(crate::dialog::Kind::Fillet)),
        );
        acts.push(Some(Viewed::Fillet));
    }
    if let (Some(maker), Some(words)) = (&maker, &edit) {
        buttons.push(
            crate::modal::Button::new(words, crate::modal::Kind::Ghost)
                .icon(Icon::Edit)
                .enabled(tab.can_edit(maker)),
        );
        acts.push(Some(Viewed::Edit(maker.clone())));
    }
    if let Some(at) = properties::buttons(ui, &buttons) {
        viewed = acts.get(at).cloned().flatten();
    }
    let modifier = crate::start::modifier_caps(ui.ctx(), egui::Modifiers::COMMAND)
        .into_iter()
        .next()
        .unwrap_or_default();
    properties::hint(
        ui,
        &[
            (
                &format!("{modifier}-click adds or removes. Right-click › "),
                false,
            ),
            ("Select Other", true),
            (" reaches what's behind.", false),
        ],
    );
    viewed
}

/// Several elements selected (`selectionProps`' second view): its head (how many, and of
/// each kind how many); for two, MEASURE, from `distance` and `angle` ([`pair_rows`]);
/// SELECTION, a row for each, its number in its body's solid and its name as an agent
/// writes it; and Copy names, which copies their names as a JSON array, as an agent writes
/// a fillet's `edges`.
fn several_selected(ui: &mut Ui, elements: &[&Picked], answers: &Answers) {
    let title = format!("{} selected", elements.len());
    element_head(ui, Icon::SelFace, &title, &kinds_words(elements));
    if let [a, b] = elements {
        properties::section(ui, "Measure");
        pair_rows(ui, [a, b], answers);
    }
    properties::section(ui, "Selection");
    for (place, picked) in elements.iter().enumerate() {
        // Each element's row under an ID of its own: two may have one number.
        let key = picked
            .found()
            .map_or_else(|| "Element".to_owned(), |found| found.element.to_string());
        ui.push_id(("selected", place), |ui| {
            properties::text_row(
                ui,
                &key,
                egui::text::LayoutJob::single_section(
                    picked.name.clone(),
                    properties::format(theme::MONOSPACE_SIZE, theme::TEXT_SUBDUED, true),
                ),
            );
        });
    }
    let copy = crate::modal::Button::new("Copy names", crate::modal::Kind::Ghost).icon(Icon::Copy);
    if properties::buttons(ui, &[copy]).is_some()
        && let Some(names) = names_json(elements)
    {
        ui.ctx().copy_text(names);
    }
}

/// The elements' names, in the order picked, as a JSON array on one line, as the command
/// API writes strings (a fillet's `edges`); none if it's too long to write.
fn names_json(elements: &[&Picked]) -> Option<String> {
    use silico_core::command::wire::{self, Json, Layout};
    let names = Json::array(elements.iter().map(|picked| Json::str(&picked.name)));
    wire::write(&names, Layout::OneLine).ok()
}

/// A selection's head ([`properties::head`]): `icon`, `title` over `sub` in the head's
/// smaller type; each line labelled, as no other widget says them.
fn element_head(ui: &mut Ui, icon: Icon, title: &str, sub: &str) {
    let mut job = properties::body(sub, theme::PROPERTY_HEAD_SUB);
    job.sections.iter_mut().for_each(|section| {
        section.format.font_id.size = theme::HEADER_SIZE;
    });
    let lines = properties::head(ui, icon, title, job);
    for (line, said) in lines.into_iter().zip([title, sub]) {
        let response = ui.interact(line, ui.id().with(("head", said)), Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, said));
    }
}

/// How many of each kind `elements` are, faces first, then edges and vertices ("2 faces, 1
/// edge"), and how many aren't answered yet, with no kind known.
fn kinds_words(elements: &[&Picked]) -> String {
    let kind_of = |picked: &&Picked| picked.found().map(silico_core::selection::Selected::kind);
    let count = |kind: Option<ElementKind>| {
        elements
            .iter()
            .filter(|picked| kind_of(picked) == kind)
            .count()
    };
    let mut words: Vec<String> = [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex]
        .into_iter()
        .filter_map(|kind| match count(Some(kind)) {
            0 => None,
            1 => Some(format!("1 {}", kind.word())),
            n => Some(format!("{n} {}", kind.plural())),
        })
        .collect();
    match count(None) {
        0 => {}
        1 => words.push("1 element".to_owned()),
        n => words.push(format!("{n} elements")),
    }
    words.join(", ")
}

/// What an element is, as the head's sub-line says it (`selectionProps`: "Planar face",
/// "Line edge", "Vertex"): its entry's type, once it's read; its kind alone till then. A
/// circle edge whose ends are one point is a circle, any other an arc.
fn element_words(kind: Option<ElementKind>, entry: Option<&ElementEntry>) -> String {
    use silico_core::command::elements::ElementType as T;
    let face = |r#type: Option<T>| match r#type {
        Some(T::Plane) => "Planar face",
        Some(T::Cylinder) => "Cylindrical face",
        Some(T::Cone) => "Conical face",
        Some(T::Sphere) => "Spherical face",
        Some(T::Torus) => "Toroidal face",
        Some(T::Bezier) => "Bézier face",
        Some(T::Bspline) => "B-spline face",
        Some(T::Revolution) => "Revolved face",
        Some(T::Extrusion) => "Extruded face",
        Some(T::Offset) => "Offset face",
        _ => "Face",
    };
    let edge = |entry: Option<&ElementEntry>| match entry.map(|entry| entry.r#type) {
        Some(T::Line) => "Line edge",
        Some(T::Circle) if entry.is_some_and(|entry| entry.start == entry.end) => "Circle edge",
        Some(T::Circle) => "Arc edge",
        Some(T::Ellipse) => "Ellipse edge",
        Some(T::Hyperbola) => "Hyperbola edge",
        Some(T::Parabola) => "Parabola edge",
        Some(T::Bezier) => "Bézier edge",
        Some(T::Bspline) => "B-spline edge",
        Some(T::Offset) => "Offset edge",
        _ => "Edge",
    };
    match kind {
        Some(ElementKind::Face) => face(entry.map(|entry| entry.r#type)),
        Some(ElementKind::Edge) => edge(entry),
        Some(ElementKind::Vertex) => "Vertex",
        _ => "Element",
    }
    .to_owned()
}

/// One element's MEASURES (`selectionProps`), from its `entry` as `elements` lists it: a
/// face's area, and a planar face's normal or a cylinder's radius and axis; an edge's
/// length, and a circle's or arc's radius; a vertex's position. Not read yet: the rows its
/// kind always has, each [`NOT_MEASURED`], or [`NOT_BUILT`] while `picked` waits for a
/// feature that didn't build; refused, the core's refusal, as a card.
fn measure_rows(
    ui: &mut Ui,
    picked: &Picked,
    kind: Option<ElementKind>,
    entry: Option<&Result<ElementEntry, silico_core::command::Problems>>,
) {
    use silico_core::command::elements::ElementType as T;
    let entry = match entry {
        Some(Ok(entry)) => entry,
        Some(Err(problems)) => {
            if let Some(error) = problems.errors().first() {
                let said = crate::tab::Said::of(error);
                properties::card(ui, &said, None, properties::Offer::default());
            }
            return;
        }
        None => {
            let key = match kind {
                Some(ElementKind::Face) => "Area",
                Some(ElementKind::Edge) => "Length",
                Some(ElementKind::Vertex) => "Position",
                _ => return,
            };
            let said = match &picked.state {
                crate::selection::State::Waiting { why: Some(_), .. } => NOT_BUILT,
                _ => NOT_MEASURED,
            };
            properties::text_row(ui, key, properties::body(said, theme::TEXT_SUBDUED));
            return;
        }
    };
    let value = |text: &str| properties::body(text, theme::PROPERTY_VALUE);
    if let Some(area) = entry.area {
        let area = format!("{} mm²", properties::fixed(area.millionths(), MILLIONTHS));
        properties::text_row(ui, "Area", value(&area));
    }
    if let Some(normal) = entry.normal.filter(|_| entry.r#type == T::Plane) {
        properties::text_row(ui, "Normal", value(&properties::direction(normal)));
    }
    if let Some(length) = entry.length {
        properties::text_row(ui, "Length", value(&mm(length)));
    }
    if matches!(entry.r#type, T::Cylinder | T::Circle) {
        if let Some(radius) = entry.radius {
            properties::text_row(ui, "Radius", value(&mm(radius)));
        }
        if let Some(axis) = entry.axis.as_ref().filter(|_| entry.r#type == T::Cylinder) {
            properties::text_row(ui, "Axis", value(&properties::direction(axis.direction)));
        }
    }
    if let Some(position) = entry.position {
        let [x, y, z] =
            position.map(|length| properties::trimmed(length.nm().into(), properties::NM_PER_MM));
        properties::text_row(ui, "Position", value(&format!("({x}, {y}, {z}) mm")));
    }
}

/// Two elements' MEASURE (`selectionProps`, as SolidWorks' Measure reports any two
/// entities: docs/ux/solidworks.md, §7): from `distance`, the least distance, and the
/// normal, centre, and centre-to-element distances where the core gives them; from
/// `angle`, the angle, "(parallel)" at exactly 0 or 180, or no row where it refuses the
/// pair. Not read yet: Distance, and Angle unless one is a vertex (which no angle takes),
/// each [`NOT_MEASURED`]; the distance refused, the core's refusal, as a card. One waiting
/// for a feature that didn't build: why, as a card, in place of the rows.
fn pair_rows(ui: &mut Ui, pair: [&Picked; 2], answers: &Answers) {
    use crate::selection::State;
    // One waits for a feature that didn't build: nothing's measured till it builds, so why
    // is said, as one element's Stability says it, never a measure "not measured yet" for
    // as long as it fails.
    let mut whys: Vec<&str> = pair
        .iter()
        .filter_map(|picked| match &picked.state {
            State::Waiting { why: Some(why), .. } => Some(why.as_str()),
            _ => None,
        })
        .collect();
    whys.dedup();
    if !whys.is_empty() {
        for (at, why) in whys.into_iter().enumerate() {
            let said = crate::tab::Said {
                warning: true,
                message: why.to_owned(),
                ..crate::tab::Said::default()
            };
            ui.push_id(("waiting", at), |ui| {
                properties::card(ui, &said, None, properties::Offer::default())
            });
        }
        return;
    }
    let value = |text: &str| properties::body(text, theme::PROPERTY_VALUE);
    let waiting = || properties::body(NOT_MEASURED, theme::TEXT_SUBDUED);
    match &answers.distance {
        None => properties::text_row(ui, "Distance", waiting()),
        Some(Ok(distance)) => {
            properties::text_row(ui, "Distance", value(&mm(distance.distance)));
            for (key, length) in [
                ("Normal distance", distance.normal_distance),
                ("Center distance", distance.center_distance),
                ("Center to element", distance.center_to_element),
            ] {
                if let Some(length) = length {
                    properties::text_row(ui, key, value(&mm(length)));
                }
            }
        }
        Some(Err(problems)) => {
            if let Some(error) = problems.errors().first() {
                let said = crate::tab::Said::of(error);
                properties::card(ui, &said, None, properties::Offer::default());
            }
        }
    }
    let vertex = pair.iter().any(|picked| {
        picked.found().map(silico_core::selection::Selected::kind) == Some(ElementKind::Vertex)
    });
    match &answers.angle {
        None if !vertex => properties::text_row(ui, "Angle", waiting()),
        Some(Ok(angle)) => properties::text_row(ui, "Angle", value(&angle_words(angle.angle))),
        // Refused: the pair has no angle, and nothing is said of it.
        None | Some(Err(_)) => {}
    }
}

/// An angle as the selection view writes it: "0° (parallel)" and "180° (parallel)" at
/// exactly those, as the prototype writes parallel faces' (the core's two faces facing
/// apart read 0, facing alike 180); else to two decimals, as `fmtF`.
fn angle_words(angle: silico_core::document::Angle) -> String {
    match angle.ndeg() {
        0 => "0° (parallel)".to_owned(),
        NDEG_HALF_TURN => "180° (parallel)".to_owned(),
        ndeg => format!("{}°", properties::fixed(ndeg.into(), NDEG_PER_DEGREE)),
    }
}

/// `length` as the selection view writes it: in mm, to two decimals (`fmtF`).
fn mm(length: Length) -> String {
    format!(
        "{} mm",
        properties::fixed(length.nm().into(), properties::NM_PER_MM)
    )
}

/// An area's millionths in a mm², and an angle's nanodegrees in a degree and in half a turn:
/// the steps the core gives each in (a length's, [`properties::NM_PER_MM`]).
const MILLIONTHS: i128 = 1_000_000;
const NDEG_PER_DEGREE: i128 = 1_000_000_000;
const NDEG_HALF_TURN: i64 = 180_000_000_000;

/// The body `id`'s name, as the browser's row or the timeline gives it; its ID if neither
/// lists it.
fn body_name(tab: &Tab, id: &FeatureId) -> String {
    tab.browser
        .bodies()
        .iter()
        .find(|row| &row.id == id)
        .map(|row| row.name.clone())
        .unwrap_or_else(|| tab.name_of(id))
}

/// An element's stability (#385's row, `selectionProps`' Stable): Stable, if a store takes
/// its name; or, Unstable, the refusal a store gives it, with its suggestion (what a
/// fillet's edges would say); or, Waiting, why it isn't answered (its feature didn't
/// build, or its body isn't named yet).
fn stability(ui: &mut Ui, picked: &Picked) {
    use crate::selection::State;
    let (state, colour, said) = match &picked.state {
        State::Found(selected) => match &selected.stored {
            Ok(_) => ("Stable", theme::SUCCESS, None),
            Err(refusal) => (
                "Unstable",
                theme::WARNING,
                Some((refusal.message().to_owned(), refusal.suggestion())),
            ),
        },
        State::Waiting { why, .. } => (
            "Waiting",
            theme::TEXT_SUBDUED,
            Some((
                why.clone().unwrap_or_else(|| {
                    "Not answered yet: its body's elements aren't named yet".to_owned()
                }),
                None,
            )),
        ),
    };
    properties::text_row(ui, "Stability", properties::body(state, colour));
    if let Some((message, suggestion)) = said {
        let said = crate::tab::Said {
            warning: true,
            message,
            suggestion: suggestion.map(str::to_owned),
            ..crate::tab::Said::default()
        };
        properties::card(ui, &said, None, properties::Offer::default());
    }
}

/// The document's problems, as the gate found them: a card each (`docProps`' `errCard(e)`,
/// with Edit and Show in timeline for a feature it names). What a card's button asked, if
/// one was.
fn problems(ui: &mut Ui, tab: &Tab) -> properties::CardOut {
    properties::document_cards(ui, tab, &tab.document_problems, "document-problem")
}

/// The document's properties, with nothing selected (`docProps`): its head (its file's name
/// over its path, or an untitled part's name over its having no file yet), its problems,
/// its revision, units, features, save state, and file on disk, and its model.
///
/// The model's section counts its bodies and gives its measures ([`model_rows`]).
fn document(ui: &mut Ui, tab: &Tab) -> properties::CardOut {
    properties::head(
        ui,
        Icon::Doc,
        tab.name(),
        egui::text::LayoutJob::single_section(
            tab.path
                .clone()
                .unwrap_or_else(|| crate::bar::NOT_SAVED_YET.to_owned()),
            properties::format(theme::MONOSPACE_SIZE, theme::PROPERTY_HEAD_SUB, true),
        ),
    );
    let asked = problems(ui, tab);
    properties::section(ui, "Document");
    let mut revision = properties::body(&tab.revision.number().to_string(), theme::PROPERTY_VALUE);
    if let Some(model) = tab.published.filter(|model| *model != tab.revision) {
        revision.append(
            &format!(" · model {}", model.number()),
            0.0,
            properties::format(theme::BODY_SIZE, theme::TEXT_SUBDUED, false),
        );
    }
    properties::text_row(ui, "Revision", revision);
    properties::text_row(
        ui,
        "Units",
        properties::body(&units(), theme::PROPERTY_VALUE),
    );
    let features = tab
        .timeline
        .as_ref()
        .and_then(|timeline| timeline.features.as_ref())
        .map(Vec::len);
    if let Some(features) = features {
        properties::text_row(
            ui,
            "Features",
            properties::body(&features.to_string(), theme::PROPERTY_VALUE),
        );
    }
    let saved = if tab.unsaved() {
        properties::body("Unsaved edits", theme::TEXT_STRONG)
    } else {
        properties::body("Yes", theme::PROPERTY_VALUE)
    };
    properties::text_row(ui, "Saved", saved);
    let disk = tab.status.as_ref().and_then(|status| status.disk);
    let (words, colour) = match disk {
        None => ("Not compared yet", theme::TEXT_SUBDUED),
        Some(Disk::Unchanged) => ("Unchanged", theme::PROPERTY_VALUE),
        Some(Disk::Changed) => ("Changed by another program", theme::WARNING),
        Some(Disk::Gone) => ("Gone", theme::ERROR_TEXT),
        Some(Disk::Unreadable) => ("Unreadable", theme::ERROR_TEXT),
    };
    properties::text_row(ui, "File on disk", properties::body(words, colour));
    properties::section(ui, "Model");
    let bodies = tab
        .timeline
        .as_ref()
        .filter(|timeline| timeline.model.is_some())
        .map(|timeline| timeline.bodies.len());
    match (bodies, features) {
        (Some(bodies), _) => {
            properties::text_row(
                ui,
                "Bodies",
                properties::body(&bodies.to_string(), theme::PROPERTY_VALUE),
            );
            if bodies > 0 {
                model_rows(ui, tab);
            }
        }
        (None, Some(0)) => properties::hint(
            ui,
            &[
                ("An empty part. ", false),
                ("CREATE", true),
                (" in the toolbar adds its first feature.", false),
            ],
        ),
        (None, _) => properties::hint(ui, &[("Nothing is built yet.", false)]),
    }
    properties::hint(
        ui,
        &[("Double-click a feature in the timeline to edit it.", false)],
    );
    asked
}

/// The model's measures (`docProps`): Volume and Area, its bodies' added, as
/// `mass_properties` gives each (to two decimals, `fmtF`); Size, the box around its bodies'
/// tight boxes, as the viewport's `meshes` read gives each (each side to at most two
/// decimals, `fmt`), once that read is of the same model and the same bodies (left out if
/// one has no box: the kernel couldn't measure it); and Faces, as many as `query` matches.
/// Each not answered yet [`NOT_MEASURED`]; a read refused leaves its rows out, and its
/// refusal is said as a card.
fn model_rows(ui: &mut Ui, tab: &Tab) {
    let of = tab.published_model();
    let answers = of.and_then(|of| tab.model_measures().answers(of));
    let value = |text: String| properties::body(&text, theme::PROPERTY_VALUE);
    let waiting = || properties::body(NOT_MEASURED, theme::TEXT_SUBDUED);
    let measure = |measure: Option<Measure>, unit: &str| {
        measure.map_or_else(waiting, |measure| {
            let written = properties::fixed(measure.millionths(), MILLIONTHS);
            value(format!("{written} {unit}"))
        })
    };
    if !answers.is_some_and(ModelAnswers::bodies_refused) {
        properties::text_row(
            ui,
            "Volume",
            measure(answers.and_then(ModelAnswers::volume), "mm³"),
        );
        properties::text_row(
            ui,
            "Area",
            measure(answers.and_then(ModelAnswers::area), "mm²"),
        );
    }
    // The view's bodies, if they're of this model and are the bodies measured.
    let measured = answers.and_then(ModelAnswers::measured_features);
    let view = tab.view.bodies();
    let same = measured.is_some_and(|measured| {
        tab.view.bodies_of() == of
            && view.len() == measured.len()
            && view.values().map(|body| &body.feature).eq(measured)
    });
    // Of this model's bodies: their box, or, for a body the kernel couldn't measure, none
    // ever (its row left out, never said to be coming).
    let boxes = same.then(|| {
        view.values()
            .map(|body| body.bounds)
            .collect::<Option<Vec<_>>>()
    });
    match boxes.map(|boxes| boxes.and_then(crate::model_measures::size)) {
        Some(None) => {}
        Some(Some(sides)) => {
            let [x, y, z] =
                sides.map(|side| properties::trimmed(side.nm().into(), properties::NM_PER_MM));
            properties::text_row(ui, "Size", value(format!("{x} × {y} × {z} mm")));
        }
        None => properties::text_row(ui, "Size", waiting()),
    }
    if !answers.is_some_and(ModelAnswers::faces_refused) {
        let faces = answers.and_then(ModelAnswers::face_count);
        properties::text_row(
            ui,
            "Faces",
            faces.map_or_else(waiting, |faces| value(faces.to_string())),
        );
    }
    if let Some(error) = answers
        .and_then(ModelAnswers::refused)
        .and_then(|problems| problems.errors().first())
    {
        let said = crate::tab::Said::of(error);
        properties::card(ui, &said, None, properties::Offer::default());
    }
}

/// The document's units, as its header holds them: `mm · 1 nm resolution`.
fn units() -> String {
    let resolution = Units::RESOLUTION;
    let digits = resolution
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(resolution.len());
    let (number, unit) = resolution.split_at(digits);
    format!("{} · {number} {unit} resolution", Units::LENGTH)
}

/// What the browser's rows asked for this frame, done once they're drawn: eyes clicked, a
/// row selected (by a click, or by its menu's opening), a sketch's Edit Sketch, the
/// document's row clicked, what a row's menu asked, and a rename field's name sent.
#[derive(Default)]
struct Asked {
    toggled: Vec<Hideable>,
    /// The Origin folder's eye clicked: the planes all shown (true) or all hidden.
    planes_shown: Option<bool>,
    /// The origin plane whose row the pointer is on, if one is.
    plane_hovered: Option<silico_core::document::OriginPlane>,
    /// The origin plane whose row was clicked while Create Sketch's pick is on: picked.
    plane_picked: Option<silico_core::document::OriginPlane>,
    /// A row clicked: its feature, and how its row selects it.
    selected: Option<(FeatureId, SelectedAs)>,
    /// A selected row Ctrl-clicked (Cmd on macOS): its feature taken out of the selection.
    deselected: Option<FeatureId>,
    /// A row right-clicked, selected as its menu opens: its feature, and how its row selects
    /// it.
    menu_selected: Option<(FeatureId, SelectedAs)>,
    edit: Option<FeatureId>,
    root: bool,
    act: Option<crate::feature_menu::Act>,
    renamed: Option<(FeatureId, String)>,
    /// Whether the rename field was drawn.
    renaming: bool,
}

/// The origin's rows, each by its name with its icon (Fusion's Origin folder: its point,
/// its axes, and its planes).
const ORIGIN: [(&str, Icon); 7] = [
    ("Origin", Icon::Origin),
    ("X", Icon::Axis),
    ("Y", Icon::Axis),
    ("Z", Icon::Axis),
    ("XY", Icon::Plane),
    ("XZ", Icon::Plane),
    ("YZ", Icon::Plane),
];

/// A sketch row's prefix, `(–)`, `(+)`, or `(?)` (DESIGN.md, *Errors*: SolidWorks'
/// definition prefixes), from each sketch's state as its read gives it
/// ([`SketchReads::state`](crate::sketch::SketchReads::state): sketch mode's, for the sketch
/// open there).
fn sketch_prefix(tab: &Tab, id: &FeatureId) -> Option<&'static str> {
    tab.sketch_reads.prefix(id)
}

/// A sketch row's meta, its plane (app.js's `renderBrowser`, `meta: f.plane`): an origin
/// plane by its name, as the prototype's `XY`, `XZ`, and `YZ`; a planar face as `Face`, the
/// prototype's sketch on a face before it names one (sketch.js's `plane: opts.plane ||
/// 'Face'`), since the name it stores (`k7f2qx3m.box.Face6`) is no word for a row's right.
pub(crate) fn plane_meta(plane: &silico_core::document::Plane) -> &'static str {
    use silico_core::document::{OriginPlane, Plane};
    match plane {
        Plane::Origin(OriginPlane::Xy) => "XY",
        Plane::Origin(OriginPlane::Xz) => "XZ",
        Plane::Origin(OriginPlane::Yz) => "YZ",
        Plane::Face(_) => "Face",
    }
}

/// Whether a sketch's row asks for Edit Sketch by a double-click (its context menu's is the
/// feature's menu's).
fn sketch_edit_asked(tab: &Tab, row: &egui::Response) -> bool {
    crate::sketch::edit_asked(Some(row), tab.can_sketch())
}

/// A prefix's colour (`.row .prefix`, `.under`, `.over`).
fn prefix_colour(prefix: &str) -> Color32 {
    match prefix {
        "(–)" => theme::SKETCH_UNDER_DEFINED,
        "(+)" => theme::SKETCH_OVER_DEFINED,
        _ => theme::BROWSER_PREFIX,
    }
}

/// How a browser row was clicked: a left click, or a right click opening its menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Click {
    Left,
    Menu,
}

/// The browser's row `id` clicked, or right-clicked as its menu opens ([`Click::Menu`]):
/// with a left click, a dialog's box that takes a body takes it, or says why it doesn't
/// (section 9: the selection is then the bodies the boxes hold), and a sketch's row fills a
/// box that takes a sketch, once (never a menu's, which a click alone makes); then the row's
/// feature selected alone `how` its row selects it, its body whole for a body's row (the
/// prototype's `bodyRow`, `maker: '*'`), itself for a sketch's, whatever was selected
/// before. Either way the browser is where the selection was made (F2's field opens over the
/// row).
fn row_clicked(tab: &mut Tab, id: FeatureId, how: SelectedAs, click: Click) {
    tab.selected_in = crate::feature_menu::Place::Browser;
    if click == Click::Left {
        if tab.pick_body(&id) {
            return;
        }
        if tab.selection().feature() != Some(&id) {
            tab.pick(&id);
        }
    }
    tab.selection_mut().select_as(id, how);
}

/// The browser's tree, for `tab`: what its rows ask is put in `asked`, and a feature's name
/// typed in `renaming`'s field, over its row.
fn tree(
    ui: &mut Ui,
    tab: &Tab,
    asked: &mut Asked,
    renaming: &mut Option<crate::feature_menu::Renaming>,
) {
    let ctx = ui.ctx().clone();
    let name = tab.name();
    let root_name = name.strip_suffix(".silico").unwrap_or(name);
    let failed = tab.timeline.as_ref().is_some_and(|timeline| {
        timeline.problems > 0
            || timeline
                .features
                .as_ref()
                .is_some_and(|features| features.iter().any(|entry| entry.health == Health::Error))
    });
    let root_text = tab
        .document_problems
        .first()
        .map(|said| said.message.as_str());
    let mut root = Openness::load(&ctx, "root");
    let answer = row(
        ui,
        &Line {
            id: egui::Id::new("browser-root"),
            depth: 0,
            caret: Some(root.open),
            eye: None,
            not_drawn: None,
            eye_named: None,
            fades: true,
            icon: Icon::Doc,
            prefix: None,
            name: root_name,
            selected: None,
            meta: tab.unsaved().then_some("unsaved"),
            mark: failed.then_some((Health::Error, root_text)),
            failed: false,
            outside: None,
            clicks: true,
        },
    );
    asked.root |= answer.row.clicked();
    root.take(&ctx, &answer);
    if !root.open {
        return;
    }
    // The folder's eye is the planes', as the prototype's: open while any shows, and a
    // click shows all three if none shows, else hides them all.
    let any_plane = crate::planes::ORIGIN_PLANES
        .iter()
        .any(|(_, name)| tab.shown(&Hideable::Origin(name)));
    let (origin_open, folder_eye) = header(ui, "Origin", None, Some((any_plane, "origin planes")));
    if folder_eye {
        asked.planes_shown = Some(!any_plane);
    }
    if origin_open {
        group(ui, "Origin", |ui| {
            for (name, icon) in ORIGIN {
                let what = Hideable::Origin(name);
                let shown = tab.shown(&what);
                let answer = row(
                    ui,
                    &Line {
                        id: egui::Id::new(("browser-origin", name)),
                        depth: 2,
                        caret: None,
                        eye: Some(shown),
                        not_drawn: None,
                        eye_named: None,
                        fades: false,
                        icon,
                        prefix: None,
                        name,
                        selected: None,
                        meta: None,
                        mark: None,
                        failed: false,
                        outside: None,
                        // Its menu's right-click: every item off, for it isn't a feature.
                        clicks: true,
                    },
                );
                if answer.eye.as_ref().is_some_and(egui::Response::clicked) {
                    asked.toggled.push(what);
                }
                // A plane's row under the pointer lights its plane in the canvas, and, while
                // Create Sketch's pick is on, one with the keys' focus too; clicked then (or
                // Enter on it), it's picked (`crate::plane_pick`), as Fusion's browser and
                // SolidWorks' tree give a plane a sketch is asked for.
                if let Some(plane) = crate::planes::of_row(name) {
                    let picking = tab.plane_pick.is_some();
                    if answer.row.hovered()
                        || answer.eye.as_ref().is_some_and(egui::Response::hovered)
                        || (picking && answer.row.has_focus())
                    {
                        asked.plane_hovered = Some(plane);
                    }
                    if answer.row.clicked() && picking {
                        asked.plane_picked = Some(plane);
                    }
                }
                answer
                    .row
                    .context_menu(|ui| crate::feature_menu::origin_menu(ui, name));
            }
        });
    }
    let Some(timeline) = &tab.timeline else {
        padded(ui, |ui| {
            ui.weak(building(tab));
        });
        return;
    };
    let any_features = timeline
        .features
        .as_ref()
        .is_some_and(|features| !features.is_empty());
    let no_body = match tab.document_problems.first() {
        Some(said) if any_features => format!("No body: {}", said.message),
        None if any_features => "No body: nothing builds one yet".to_owned(),
        _ => "None yet. A pad makes one.".to_owned(),
    };
    // Each group with how its rows select their features (a body's whole, a sketch as
    // itself) and whether they're sketches, together so they can't drift apart.
    struct Group<'a> {
        name: &'static str,
        rows: &'a [crate::tab::Row],
        icon: Icon,
        empty: String,
        how: SelectedAs,
        sketches: bool,
    }
    let groups = [
        Group {
            name: "Bodies",
            rows: tab.browser.bodies(),
            icon: Icon::Body,
            empty: no_body,
            how: SelectedAs::Body,
            sketches: false,
        },
        Group {
            name: "Sketches",
            rows: tab.browser.sketches(),
            icon: Icon::Sketch,
            empty: "None yet. Create Sketch starts one.".to_owned(),
            how: SelectedAs::Itself,
            sketches: true,
        },
    ];
    for Group {
        name: group_name,
        rows,
        icon,
        empty,
        how,
        sketches,
    } in groups
    {
        let count = rows.is_empty().then_some("0");
        if !header(ui, group_name, count, None).0 {
            continue;
        }
        group(ui, group_name, |ui| {
            if rows.is_empty() {
                empty_words(ui, &empty);
            }
            for each in rows {
                let text = each
                    .marked_by
                    .as_ref()
                    .and_then(|by| tab.said.get(by.as_str()))
                    .map(|said| said.message.as_str());
                let what = Hideable::Feature(each.id.clone());
                let marked = matches!(each.health, Health::Error | Health::Warning);
                let not_drawn = if sketches { tab.not_drawn(each) } else { None };
                let answer = row(
                    ui,
                    &Line {
                        id: egui::Id::new(("browser-feature", each.id.as_str())),
                        depth: 2,
                        caret: None,
                        eye: Some(tab.shown(&what)),
                        not_drawn: not_drawn.as_deref(),
                        eye_named: None,
                        fades: true,
                        icon,
                        prefix: if sketches {
                            sketch_prefix(tab, &each.id)
                        } else {
                            None
                        },
                        name: &each.name,
                        selected: Some(tab.selection().marks_body(&each.id)),
                        // app.js's `renderBrowser`: a body's the last healthy feature of
                        // its chain, a sketch's its plane.
                        meta: if sketches {
                            tab.sketch_plane(&each.id).map(plane_meta)
                        } else {
                            each.last_healthy.as_deref()
                        },
                        mark: marked.then_some((each.health, text)),
                        // A sketch's own failure reddens its name (`.row.err`); a body's
                        // mark is its chain's.
                        failed: sketches && each.health == Health::Error,
                        // Changed by another program (`d.outside`), its info-blue dot where it
                        // has no health mark.
                        outside: tab.outside.get(&each.id).map(String::as_str),
                        clicks: false,
                    },
                );
                if answer.eye.as_ref().is_some_and(egui::Response::clicked) {
                    asked.toggled.push(what);
                } else if answer.row.clicked() {
                    if tab.selection().has_feature(&each.id)
                        && ui.input(|input| input.modifiers.command)
                    {
                        asked.deselected = Some(each.id.clone());
                    } else {
                        asked.selected = Some((each.id.clone(), how));
                    }
                }
                if let Some(entry) = tab.entry(each.id.as_str()) {
                    answer.row.context_menu(|ui| {
                        // Selected as it's opened, as the prototype's `contextmenu` selects it,
                        // by `Tab::menu_selects`.
                        if tab.menu_selects(&each.id, how) {
                            asked.menu_selected = Some((each.id.clone(), how));
                        }
                        let place = crate::feature_menu::Place::Browser;
                        if let Some(act) = crate::feature_menu::menu(ui, tab, entry, place) {
                            asked.act = Some(act);
                        }
                    });
                    if let Some(field) = renaming.as_mut().filter(|renaming| {
                        renaming.place == crate::feature_menu::Place::Browser
                            && renaming.feature == each.id
                    }) {
                        asked.renaming = true;
                        let at = egui::Rect::from_center_size(
                            answer.name.center(),
                            Vec2::new(answer.name.width(), theme::FIELD_HEIGHT),
                        );
                        if let crate::feature_menu::Field::Done(name) =
                            crate::feature_menu::field(ui, field, at, &entry.name)
                        {
                            asked.renamed = Some((each.id.clone(), name));
                        }
                    }
                }
                if tab.selection().scroll_target() == Some(&each.id) {
                    answer.row.scroll_to_me(None);
                }
                if sketches && sketch_edit_asked(tab, &answer.row) {
                    asked.edit = Some(each.id.clone());
                }
            }
        });
    }
}

/// A header's or the document's row: open or not (egui's memory keeps it, by `key`).
struct Openness {
    state: egui::collapsing_header::CollapsingState,
    open: bool,
}

impl Openness {
    /// The open state of the row `key`; open at first.
    fn load(ctx: &egui::Context, key: &str) -> Self {
        let id = egui::Id::new(("browser-open", key));
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(ctx, id, true);
        let open = state.is_open();
        Openness { state, open }
    }

    /// Opened, and kept so.
    fn opened(mut self, ctx: &egui::Context) {
        if !self.open {
            self.open = true;
            self.state.set_open(true);
            self.state.store(ctx);
        }
    }

    /// Its caret's state taken from the row: opened or closed, and kept.
    fn take(&mut self, ctx: &egui::Context, answer: &RowAnswer) {
        if let Some(open) = answer.open
            && open != self.open
        {
            self.open = open;
            self.state.set_open(open);
            self.state.store(ctx);
        }
    }
}

/// A group's header row (a folder, with its caret, and `meta`, and an eye if `eye` gives
/// whether it shows and what its label names): whether it's open, and whether its eye was
/// clicked.
fn header(ui: &mut Ui, name: &str, meta: Option<&str>, eye: Option<(bool, &str)>) -> (bool, bool) {
    let ctx = ui.ctx().clone();
    let mut openness = Openness::load(&ctx, name);
    let answer = row(
        ui,
        &Line {
            id: egui::Id::new(("browser-group", name)),
            depth: 1,
            caret: Some(openness.open),
            eye: eye.map(|(shown, _)| shown),
            not_drawn: None,
            eye_named: eye.map(|(_, named)| named),
            fades: false,
            icon: Icon::Folder,
            prefix: None,
            name,
            selected: None,
            meta,
            mark: None,
            failed: false,
            outside: None,
            clicks: false,
        },
    );
    openness.take(&ctx, &answer);
    let eye_clicked = answer.eye.as_ref().is_some_and(egui::Response::clicked);
    (openness.open, eye_clicked)
}

/// A group of rows, labelled for the accessibility tree (the browser's Origin, Bodies, and
/// Sketches, by type, as Fusion's browser groups them).
fn group(ui: &mut Ui, name: &str, rows: impl FnOnce(&mut Ui)) {
    ui.scope_builder(UiBuilder::new().id_salt(("group", name)), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::Group);
            node.set_label(name);
        });
        rows(ui);
    });
}

/// `add`, at the pane's padding from its left.
fn padded(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(theme::VIEW_PADDING, 0))
        .show(ui, add);
}

/// An empty group's words (`.tree-empty`), each line as high as the body's
/// ([`theme::BODY_LINE_HEIGHT`]: `.tree-empty` sets its size, not its line's height).
fn empty_words(ui: &mut Ui, words: &str) {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: theme::BROWSER_EMPTY_PADDING_LEFT,
            right: theme::VIEW_PADDING,
            top: theme::BROWSER_EMPTY_PADDING_TOP,
            bottom: theme::BROWSER_EMPTY_PADDING_BOTTOM,
        })
        .show(ui, |ui| {
            ui.label(
                RichText::new(words)
                    .size(theme::HEADER_SIZE)
                    .line_height(Some(theme::BODY_LINE_HEIGHT))
                    .color(theme::BROWSER_EMPTY),
            );
        });
}

/// A browser row (`.row`; app.js's `row`): what it shows.
struct Line<'a> {
    /// Its interaction's ID: unique in the tree.
    id: egui::Id,
    /// Its depth in the tree, which indents it.
    depth: u8,
    /// Its caret, if it has one: whether it's open.
    caret: Option<bool>,
    /// Its eye, if it has one: whether it shows.
    eye: Option<bool>,
    /// Why what its eye shows is drawn nowhere, if it is (a sketch suppressed, or on no
    /// plane): its eye's tooltip says it under its word.
    not_drawn: Option<&'a str>,
    /// What its eye's label names after its word (Hide or Show), if not its name: the
    /// Origin folder's eye is the planes' ("origin planes"), apart from the origin point's
    /// row, also named Origin.
    eye_named: Option<&'a str>,
    /// Whether its eye closed fades it: a body's and a sketch's do (the prototype's
    /// `hidden-obj`); the origin's rows' don't, as the prototype's planes' and folder's.
    fades: bool,
    icon: Icon,
    /// A sketch's definition prefix, before its name.
    prefix: Option<&'static str>,
    name: &'a str,
    /// Whether it's selectable, and selected; a label if not.
    selected: Option<bool>,
    /// Its words at its right (`.r-meta`).
    meta: Option<&'a str>,
    /// Its mark (`.r-err`, `.r-warn`): its health and the text it's marked with.
    mark: Option<(Health, Option<&'a str>)>,
    /// Whether its name is the error's red (`.row.err`).
    failed: bool,
    /// What another program changed in it, if it did (`d.outside`): an info-blue dot at its
    /// right where it has no mark (`.state-dot`).
    outside: Option<&'a str>,
    /// Whether it takes a click though it isn't selectable (the document's row, whose click
    /// opens What's Wrong).
    clicks: bool,
}

/// What a row answered: the row's response, its caret's state, its eye's response, and
/// where its name is (between its icon, or prefix, and its meta or mark).
struct RowAnswer {
    row: egui::Response,
    /// Whether its caret is open, its click this frame taken.
    open: Option<bool>,
    eye: Option<egui::Response>,
    name: egui::Rect,
}

/// The colours of a row's name and its icon (`.row`, `:hover`, `.selected`, `.err`,
/// `.hidden-obj`): hovered or selected, failed, and hidden.
fn row_colours(hovered: bool, selected: bool, failed: bool, hidden: bool) -> (Color32, Color32) {
    let (name, icon) = match (selected, failed, hovered) {
        (true, _, _) => (theme::ON_SELECTION, theme::ON_SELECTION),
        (false, true, _) => (theme::ERROR_TEXT, theme::ROW_ICON),
        (false, false, true) => (theme::TEXT_ROW_HOVER, theme::ROW_ICON),
        (false, false, false) => (theme::TEXT_ROW, theme::ROW_ICON),
    };
    if hidden {
        (faded(name), faded(icon))
    } else {
        (name, icon)
    }
}

/// `colour` at the hidden rows' opacity, as CSS's `opacity` takes it: its alpha scaled and
/// rounded, its colour kept.
fn faded(colour: Color32) -> Color32 {
    let [r, g, b, a] = colour.to_srgba_unmultiplied();
    // An alpha times a fraction under one: within u8's range.
    let alpha = (f32::from(a) * theme::BROWSER_HIDDEN_OPACITY).round() as u8;
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// The fill behind a row: selected, hovered, or none (`.row.selected`, `.row:hover`).
fn row_fill(hovered: bool, selected: bool) -> Option<Color32> {
    if selected {
        Some(theme::BROWSER_ROW_SELECTED_BG)
    } else if hovered {
        Some(theme::BROWSER_ROW_HOVER_BG)
    } else {
        None
    }
}

/// A browser row, [`theme::LIST_ITEM_HEIGHT`] high across the pane: its caret, its eye, its
/// icon, its prefix, its name, its meta at its right, and its mark. The row is one widget
/// for the accessibility tree, by its name (selectable, with its state, if `selected` is
/// given); its caret and its eye are buttons of their own on top of it.
fn row(ui: &mut Ui, line: &Line<'_>) -> RowAnswer {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme::LIST_ITEM_HEIGHT),
        Sense::hover(),
    );
    let id = line.id;
    let sense = if line.selected.is_some() || line.clicks {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, id, sense);
    let selected = line.selected.unwrap_or(false);
    response.widget_info(|| match line.selected {
        Some(selected) => {
            WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, line.name)
        }
        None => WidgetInfo::labeled(WidgetType::Label, true, line.name),
    });
    let centre = rect.center().y;
    let gap = theme::BROWSER_ROW_GAP;
    let place = |x: f32, side: f32| {
        egui::Rect::from_min_size(egui::pos2(x, centre - side / 2.0), Vec2::splat(side))
    };
    let mut x =
        rect.left() + theme::BROWSER_ROW_PADDING_LEFT + f32::from(line.depth) * theme::TREE_INDENT;
    let caret_rect = place(x, theme::BROWSER_CARET_SIDE);
    x += theme::BROWSER_CARET_SIDE + gap;
    let eye_rect = place(x, theme::BROWSER_EYE_SIDE);
    x += theme::BROWSER_EYE_SIDE + gap;
    // The caret and the eye, on top of the row, each drawn as its click leaves it, so
    // nothing shows a frame late.
    let caret = line.caret.map(|open| {
        let caret = ui.interact(caret_rect, id.with("caret"), Sense::click());
        let open = if caret.clicked() { !open } else { open };
        (caret, open)
    });
    let eye = line.eye.map(|shown| {
        let eye = ui.interact(eye_rect, id.with("eye"), Sense::click());
        let shown = if eye.clicked() { !shown } else { shown };
        (eye, shown)
    });
    let hovered = response.contains_pointer();
    if let Some(fill) = row_fill(hovered, selected) {
        ui.painter().rect_filled(rect, 0.0, fill);
    }
    theme::focus_ring(ui, &response, rect, 0, egui::StrokeKind::Inside);
    // The caret: a chevron turned down while open (`.caret.open`).
    if let Some((caret, open)) = &caret {
        let word = if *open { "Collapse" } else { "Expand" };
        caret.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, true, format!("{word} {}", line.name))
        });
        let chevron = egui::Rect::from_center_size(
            caret_rect.center(),
            Vec2::splat(theme::BROWSER_CARET_ICON),
        );
        let turn = if *open {
            std::f32::consts::FRAC_PI_2
        } else {
            0.0
        };
        Icon::Chevron
            .image(IconSize::Small)
            .fit_to_exact_size(chevron.size())
            .tint(theme::BROWSER_CARET)
            .rotate(turn, Vec2::splat(0.5))
            .paint_at(ui, chevron);
        theme::focus_ring(
            ui,
            caret,
            caret_rect,
            theme::BROWSER_EYE_CORNER_RADIUS,
            egui::StrokeKind::Inside,
        );
    }
    // The eye: open while the row shows, struck through while it's hidden.
    let eye = eye.map(|(eye, shown)| {
        let word = if shown { "Hide" } else { "Show" };
        let named = line.eye_named.unwrap_or(line.name);
        eye.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, true, format!("{word} {named}"))
        });
        let colour = if eye.hovered() {
            ui.painter().rect_filled(
                eye_rect,
                theme::BROWSER_EYE_CORNER_RADIUS,
                theme::BROWSER_EYE_HOVER_BG,
            );
            theme::BROWSER_EYE_HOVER
        } else if shown {
            theme::BROWSER_EYE
        } else {
            theme::BROWSER_EYE_OFF
        };
        let glyph = if shown { Icon::Eye } else { Icon::EyeOff };
        let at = egui::Rect::from_center_size(eye_rect.center(), Vec2::splat(icons::ICON_SIZE));
        glyph.image(IconSize::Small).tint(colour).paint_at(ui, at);
        theme::focus_ring(
            ui,
            &eye,
            eye_rect,
            theme::BROWSER_EYE_CORNER_RADIUS,
            egui::StrokeKind::Inside,
        );
        (
            crate::bar::tooltip(
                eye,
                word,
                crate::shortcuts::Command::Hide.shown(ui.ctx()),
                line.not_drawn,
            ),
            shown,
        )
    });
    let hidden = line.fades && eye.as_ref().is_some_and(|(_, shown)| !shown);
    let (name_colour, icon_colour) = row_colours(hovered, selected, line.failed, hidden);
    // The icon.
    line.icon
        .image(IconSize::Small)
        .tint(icon_colour)
        .paint_at(ui, place(x, icons::ICON_SIZE));
    x += icons::ICON_SIZE + gap;
    // The prefix, in the monospace font.
    if let Some(prefix) = line.prefix {
        // Its state's colour on a selected row too: styles.css has no `.row.selected
        // .prefix`.
        let colour = prefix_colour(prefix);
        let galley = ui.painter().layout_no_wrap(
            prefix.to_owned(),
            egui::FontId::monospace(theme::MONOSPACE_SIZE),
            colour,
        );
        let width = galley.size().x;
        ui.painter().galley(
            egui::pos2(x, centre - galley.size().y / 2.0),
            galley,
            colour,
        );
        x += width + gap;
    }
    // From the right: the mark, then the meta.
    let mut right = rect.right() - theme::BROWSER_ROW_PADDING_RIGHT;
    if let Some((health, text)) = line.mark {
        let at = place(right - icons::ICON_SIZE, icons::ICON_SIZE);
        health_mark(ui, id.with("mark"), at, health, text, MarkLook::Glyph);
        right -= icons::ICON_SIZE + gap;
    } else if let Some(text) = line.outside {
        let at = place(right - theme::STATE_DOT_SIZE, theme::STATE_DOT_SIZE);
        outside_mark(ui, id.with("outside"), at, text, None);
        right -= theme::STATE_DOT_SIZE + gap;
    }
    if let Some(meta) = line.meta {
        let colour = if selected {
            theme::ON_SELECTION
        } else {
            theme::BROWSER_META
        };
        // At most half the room between the name's start and the mark, cut short with an
        // ellipsis past it, so a long feature's name in it never hides the row's own name,
        // nor runs out of the row's left (the prototype's `.r-meta` doesn't shrink).
        let mut job = egui::text::LayoutJob::simple_singleline(
            meta.to_owned(),
            egui::FontId::proportional(theme::HEADER_SIZE),
            colour,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(((right - x) / 2.0).max(0.0));
        let galley = ui.painter().layout_job(job);
        let width = galley.size().x;
        ui.painter().galley(
            egui::pos2(right - width, centre - galley.size().y / 2.0),
            galley,
            colour,
        );
        right -= width + gap;
    }
    // The name, cut short with an ellipsis where it won't fit (`text-overflow: ellipsis`).
    let mut job = egui::text::LayoutJob::simple_singleline(
        line.name.to_owned(),
        egui::FontId::proportional(theme::BODY_SIZE),
        name_colour,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((right - x).max(0.0));
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(x, centre - galley.size().y / 2.0),
        galley,
        name_colour,
    );
    RowAnswer {
        row: response,
        open: caret.map(|(_, open)| open),
        eye: eye.map(|(eye, _)| eye),
        name: egui::Rect::from_x_y_ranges(x..=right.max(x), rect.y_range()),
    }
}

/// The mark of a feature another program changed (`d.outside`), the browser's and the
/// timeline's alike: an info-blue dot filling `rect` (`.state-dot`, or the timeline's
/// `.badge.outside`, in a ring of `ring`'s colour 1.5 outside it), labelled with what
/// changed (`changed on disk: radius 5 → 3`) for the accessibility tree, and with the
/// prototype's words as its hover text.
pub(crate) fn outside_mark(
    ui: &mut Ui,
    id: egui::Id,
    rect: egui::Rect,
    text: &str,
    ring: Option<Color32>,
) {
    let radius = rect.width().min(rect.height()) / 2.0;
    let painter = ui.painter();
    if let Some(ring) = ring {
        painter.circle_filled(
            rect.center(),
            radius + theme::TIMELINE_BADGE_RING_WIDTH,
            ring,
        );
    }
    painter.circle_filled(rect.center(), radius, theme::CHANGED_OUTSIDE);
    let response = ui.interact(rect, id, Sense::hover());
    let said = format!("changed on disk: {text}");
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &said));
    response.on_hover_text("Changed on disk since you opened it");
}

/// How a health mark is drawn: the browser's, the error's or the warning's icon at a row's
/// right (`.r-err`, `.r-warn`); or the timeline's, a dot at an item's corner (`.badge.err`,
/// `.badge.warn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkLook {
    Glyph,
    Badge,
}

/// A feature's health mark (section 9, *Error marks*), the browser's and the timeline's
/// alike: drawn at `rect` as `look` has it, in the error's colours or the warning's, with
/// its text (`error: …`, or the word alone until the text is read) for the accessibility
/// tree and as its hover text; none for any other health.
pub(crate) fn health_mark(
    ui: &mut Ui,
    id: egui::Id,
    rect: egui::Rect,
    health: Health,
    text: Option<&str>,
    look: MarkLook,
) {
    let (word, glyph, colour, fill) = match health {
        Health::Error => (
            "error",
            Icon::Error,
            theme::ERROR_TEXT,
            theme::TIMELINE_BADGE_ERROR,
        ),
        Health::Warning => (
            "warning",
            Icon::Warn,
            theme::WARNING,
            theme::TIMELINE_BADGE_WARNING,
        ),
        Health::Healthy | Health::Suppressed | Health::Unknown => return,
    };
    let said = match text {
        Some(text) => format!("{word}: {text}"),
        None => word.to_owned(),
    };
    match look {
        MarkLook::Glyph => glyph.image(IconSize::Small).tint(colour).paint_at(ui, rect),
        MarkLook::Badge => {
            // Its ring outside it (`box-shadow: 0 0 0 1.5px`), then the dot.
            let radius = rect.width().min(rect.height()) / 2.0;
            let painter = ui.painter();
            painter.circle_filled(
                rect.center(),
                radius + theme::TIMELINE_BADGE_RING_WIDTH,
                theme::TIMELINE_BADGE_RING,
            );
            painter.circle_filled(rect.center(), radius, fill);
        }
    }
    let response = ui.interact(rect, id, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &said));
    response.on_hover_text(said);
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
        let failed = crate::tab::browser(
            &timeline(&entries, &["k7f2qx3m", "a91cw0de"]),
            &[],
            &crate::tab::Browser::default(),
        );
        assert_eq!(
            rows(failed.bodies()),
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
            rows(
                crate::tab::browser(
                    &timeline(entries, &["m3tq8v0z"]),
                    &[],
                    &crate::tab::Browser::default(),
                )
                .bodies(),
            )
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
    fn a_rows_name_and_icon_are_the_prototypes_in_each_state() {
        let rgb = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Color32::from_rgb(r, g, b)
        };
        // styles.css: `.row` `--g700`, its `.r-icon` `--g500`; `:hover` `--g800`;
        // `.selected` both `--blue-text`; `.err .r-name` `--red`, and `--blue-text` when
        // selected too.
        let cases = [
            ((false, false, false), (0xbfbfbf, 0x827f7f)),
            ((true, false, false), (0xe0dfdf, 0x827f7f)),
            ((false, true, false), (0xf0f2ff, 0xf0f2ff)),
            ((true, true, false), (0xf0f2ff, 0xf0f2ff)),
            ((false, false, true), (0xff3865, 0x827f7f)),
            ((true, false, true), (0xff3865, 0x827f7f)),
            ((false, true, true), (0xf0f2ff, 0xf0f2ff)),
        ];
        for ((hovered, selected, failed), (name, icon)) in cases {
            assert_eq!(
                row_colours(hovered, selected, failed, false),
                (rgb(name), rgb(icon)),
                "{hovered} {selected} {failed}"
            );
        }
        // `.hidden-obj .r-name, .hidden-obj .r-icon { opacity: .45 }`: each at 45% of 255,
        // 115, over what's behind it.
        let at = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Color32::from_rgba_unmultiplied(r, g, b, 115)
        };
        assert_eq!(
            row_colours(false, false, false, true),
            (at(0xbfbfbf), at(0x827f7f))
        );
        assert_eq!(
            row_colours(false, true, false, true),
            (at(0xf0f2ff), at(0xf0f2ff))
        );
    }

    #[test]
    fn a_rows_fill_is_blue_selected_gray_hovered_and_none_at_rest() {
        // `.row.selected` `--blue`, after `.row:hover` `--g250` in styles.css, so a selected
        // row hovered stays blue.
        let blue = Some(Color32::from_rgb(0x18, 0x6a, 0xdd));
        assert_eq!(row_fill(false, true), blue);
        assert_eq!(row_fill(true, true), blue);
        assert_eq!(
            row_fill(true, false),
            Some(Color32::from_rgb(0x27, 0x26, 0x26))
        );
        assert_eq!(row_fill(false, false), None);
    }

    #[test]
    fn a_sketchs_prefix_is_coloured_by_its_state() {
        // `.prefix.under` `--sk-under` (`#7a95ff`), `.over` `--sk-over` (`#ff3865`), and
        // otherwise `--g550`.
        assert_eq!(prefix_colour("(–)"), Color32::from_rgb(0x7a, 0x95, 0xff));
        assert_eq!(prefix_colour("(+)"), Color32::from_rgb(0xff, 0x38, 0x65));
        assert_eq!(prefix_colour("(?)"), Color32::from_rgb(0x93, 0x90, 0x90));
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
            let node = crate::tab::name_value(name).unwrap();
            assert_eq!(node.value.as_str(), Some(name), "{name:?}");
        }
    }

    // ---- The browser as drawn, against the prototype ----
    //
    // Each value below is typed from the approved prototype's stylesheet
    // (docs/design/m5-ui/styles.css: `:root`'s tokens, `#browser`, `.panel-title`,
    // `.panel-body`, `.row` and its parts, `.tree-empty`) and js/app.js's `row` and
    // `renderBrowser`, never from theme.rs, and compared with what the pane painted: each
    // rectangle, icon, and text from egui's shapes, and each widget's bounds from the
    // accessibility tree.

    use eframe::egui::epaint::{RectShape, TextShape};
    use eframe::egui::text::TextFormat;
    use eframe::egui::{CornerRadius, FontFamily, FontId, Pos2, Rect, Shape, TextureId};
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use silico_core::command::Problems;
    use silico_core::fake::FakeKernel;

    use crate::tab::{Browser, Row, Said};

    /// The prototype's tokens and geometry.
    mod css {
        use eframe::egui::Color32;

        // `:root`'s greys and colours.
        pub const G250: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
        pub const G350: Color32 = Color32::from_rgb(0x52, 0x51, 0x51);
        pub const G400: Color32 = Color32::from_rgb(0x64, 0x63, 0x63);
        pub const G500: Color32 = Color32::from_rgb(0x82, 0x7f, 0x7f);
        pub const G550: Color32 = Color32::from_rgb(0x93, 0x90, 0x90);
        pub const G700: Color32 = Color32::from_rgb(0xbf, 0xbf, 0xbf);
        pub const G800: Color32 = Color32::from_rgb(0xe0, 0xdf, 0xdf);
        pub const G1000: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
        /// `--blue`, `--blue-text`, and `--blue-soft` (`--sk-under`).
        pub const BLUE: Color32 = Color32::from_rgb(0x18, 0x6a, 0xdd);
        pub const BLUE_TEXT: Color32 = Color32::from_rgb(0xf0, 0xf2, 0xff);
        pub const BLUE_SOFT: Color32 = Color32::from_rgb(0x7a, 0x95, 0xff);
        /// `--red` (`.r-err`, `.row.err .r-name`) and `--orange` (`.r-warn`).
        pub const RED: Color32 = Color32::from_rgb(0xff, 0x38, 0x65);
        pub const ORANGE: Color32 = Color32::from_rgb(0xff, 0x7a, 0x0c);

        /// `--fs`, `--lh` (the body's `font: var(--fs)/var(--lh)`, inherited), `--fs-small`,
        /// `--icon`, and `.row .prefix`'s `font-size: 11px`.
        pub const FS: f32 = 12.0;
        pub const LH: f32 = 16.0;
        pub const FS_SMALL: f32 = 11.0;
        pub const ICON: f32 = 14.0;
        pub const PREFIX: f32 = 11.0;
        /// `--browser-w`, and `#browser`'s `border-right: 1px` within it (`border-box`).
        pub const BROWSER_W: f32 = 248.0;
        pub const BORDER: f32 = 1.0;
        /// `.panel-title`: `height: var(--title-h)`, `padding-left: var(--pad)`,
        /// `letter-spacing: .5px`.
        pub const TITLE_H: f32 = 24.0;
        pub const PAD: f32 = 12.0;
        pub const CAPITALS_SPACING: f32 = 0.5;
        /// `.row`: `height: var(--row-h)`, `gap: 4px`, `padding-right: 6px`; app.js's `row`:
        /// `padding-left: 6 + depth * 18` (`--indent`).
        pub const ROW_H: f32 = 24.0;
        pub const ROW_GAP: f32 = 4.0;
        pub const ROW_PADDING_RIGHT: f32 = 6.0;
        pub const ROW_PADDING_LEFT: f32 = 6.0;
        pub const INDENT: f32 = 18.0;
        /// `.row .caret`: 14 square; its `svg` 10 square, `rotate(90deg)` while `.open`.
        pub const CARET: f32 = 14.0;
        pub const CARET_SVG: f32 = 10.0;
        /// `.row .eye`: 18 square, `border-radius: 3px`.
        pub const EYE: f32 = 18.0;
        pub const EYE_RADIUS: u8 = 3;
        /// `.row.hidden-obj .r-name, .row.hidden-obj .r-icon { opacity: .45 }`: an alpha of
        /// 255 × .45 = 114.75, 115.
        pub const HIDDEN_ALPHA: u8 = 115;
        /// `.tree-empty`: `padding: 2px 12px 6px calc(var(--pad) + 36px)`.
        pub const EMPTY_TOP: f32 = 2.0;
        pub const EMPTY_RIGHT: f32 = 12.0;
        pub const EMPTY_BOTTOM: f32 = 6.0;
        pub const EMPTY_LEFT: f32 = 12.0 + 36.0;
    }

    /// The pane's top left in the harness's window (away from the origin, so a sum with its
    /// corner never reads as a product), its width (`#browser`'s, less its border), and its
    /// height.
    const PANE: Pos2 = Pos2::new(40.0, 30.0);
    const W: f32 = css::BROWSER_W - css::BORDER;
    const HEIGHT: f32 = 640.0;

    type Fake = Arc<silico_core::fake::FakeKernel>;

    /// What the browser is drawn from.
    struct Shown {
        tab: Option<Tab>,
        sessions: Arc<Sessions<Fake>>,
        name: Option<NameField>,
        next_frame: bool,
        drawing: crate::viewport::Drawing,
        remembered: crate::dialog::Remembered,
        toggled: bool,
    }

    fn fid(id: &str) -> FeatureId {
        FeatureId::new(id).unwrap()
    }

    /// A browser row: `id`, named `name`, marked with `health` by itself if it's marked.
    fn listed(id: &str, name: &str, health: Health) -> Row {
        Row {
            id: fid(id),
            name: name.to_owned(),
            health,
            marked_by: matches!(health, Health::Error | Health::Warning).then(|| fid(id)),
            consumers: Default::default(),
            suppressed: false,
            last_healthy: None,
        }
    }

    /// The tab of `/models/Part.silico`, the gate taking it, its timeline `features` with
    /// `problems`, and the browser's rows `bodies` and `sketches`.
    fn part(features: &[Entry], problems: u64, bodies: Vec<Row>, sketches: Vec<Row>) -> Tab {
        let none = Problems::from(silico_core::Error::new(
            silico_core::ErrorCode::Internal,
            "unused",
        ));
        let mut tab = Tab::opened(
            SessionId::parse("s1").unwrap(),
            "/models/Part.silico".to_owned(),
            &none,
        );
        tab.document_problems.clear();
        let mut answer = timeline(features, &[]);
        answer.problems = problems;
        tab.timeline = Some(answer);
        tab.browser = Browser::new(bodies, sketches);
        tab
    }

    /// The bodies Block, Cut (failed: "The cut removes nothing"), and Shell (warned), and
    /// the sketches Profile and Bad (failed).
    fn modelled() -> Tab {
        let features = [
            entry("b0000001", "box", &[], Health::Healthy),
            entry("c0000001", "cut", &["b0000001"], Health::Error),
            entry("s0000001", "box", &[], Health::Warning),
            entry("p0000001", "sketch", &[], Health::Healthy),
            entry("b0000002", "sketch", &[], Health::Error),
        ];
        let mut tab = part(
            &features,
            0,
            vec![
                listed("b0000001", "Block", Health::Healthy),
                listed("c0000001", "Cut", Health::Error),
                listed("s0000001", "Shell", Health::Warning),
            ],
            vec![
                listed("p0000001", "Profile", Health::Healthy),
                listed("b0000002", "Bad", Health::Error),
            ],
        );
        tab.said.insert(
            "c0000001".to_owned(),
            Said {
                message: "The cut removes nothing".to_owned(),
                ..Said::default()
            },
        );
        tab
    }

    /// The browser pane, at [`PANE`], drawn from `tab` in the app's theme.
    fn drawn(tab: Tab) -> Harness<'static, Shown> {
        drawn_at(tab, W)
    }

    /// `tab`'s properties drawn in a pane `width` wide.
    fn drawn_at(tab: Tab, width: f32) -> Harness<'static, Shown> {
        let shown = Shown {
            tab: Some(tab),
            sessions: Arc::new(Sessions::with_cache_opener(
                Arc::new(FakeKernel::new(1).unwrap()),
                |dir| silico_core::cache::Cache::unsealed(dir),
            )),
            name: None,
            next_frame: false,
            drawing: crate::viewport::Drawing::default(),
            remembered: crate::dialog::Remembered::default(),
            toggled: false,
        };
        let mut h = Harness::builder()
            .with_size(Vec2::new(PANE.x + width + 40.0, PANE.y + HEIGHT + 40.0))
            .build_ui_state(
                move |ui, shown: &mut Shown| {
                    let pane = Rect::from_min_size(PANE, Vec2::new(width, HEIGHT));
                    ui.scope_builder(UiBuilder::new().max_rect(pane), |ui| {
                        Panes {
                            tab: shown.tab.as_mut(),
                            sessions: &shown.sessions,
                            name: &mut shown.name,
                            next_frame: &mut shown.next_frame,
                            drawing: &mut shown.drawing,
                            dialog_key: None,
                            remembered: &shown.remembered,
                            fade: ui.visuals().disabled_alpha,
                            browser_collapsed: false,
                            browser_toggled: &mut shown.toggled,
                        }
                        .browser(ui);
                    });
                },
                shown,
            );
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    fn tab_of<'a>(h: &'a mut Harness<'_, Shown>) -> &'a mut Tab {
        h.state_mut().tab.as_mut().unwrap()
    }

    /// Every shape the last frame painted, nested ones flattened.
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

    fn texts<S>(h: &Harness<'_, S>) -> Vec<TextShape> {
        painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Text(shown) => Some(shown),
                _ => None,
            })
            .collect()
    }

    /// The one text `text` painted within `within`.
    #[track_caller]
    fn text_in<S>(h: &Harness<'_, S>, text: &str, within: Rect) -> TextShape {
        let mut found: Vec<TextShape> = texts(h)
            .into_iter()
            .filter(|shown| shown.galley.text() == text && within.contains(shown.pos))
            .collect();
        assert_eq!(found.len(), 1, "{text:?} painted {} times", found.len());
        found.remove(0)
    }

    /// A text's colour as painted: its override, else its first section's, else the shape's
    /// own for a placeholder.
    fn colour(shown: &TextShape) -> Color32 {
        let format = shown.galley.job.sections[0].format.color;
        shown
            .override_text_color
            .unwrap_or(if format == Color32::PLACEHOLDER {
                shown.fallback_color
            } else {
                format
            })
    }

    fn format(shown: &TextShape) -> &TextFormat {
        &shown.galley.job.sections[0].format
    }

    /// The tint of the icon painted at `rect` (a rectangle filled with its texture), to half
    /// a point: egui puts an image on the pixel grid.
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

    /// The icon painted turned over `rect` (a mesh with its texture): its tint, and where its
    /// image's top left corner went.
    fn turned_at<S>(h: &Harness<'_, S>, rect: Rect) -> Option<(Color32, Pos2)> {
        painted(h).into_iter().find_map(|shape| match shape {
            Shape::Mesh(mesh) if mesh.texture_id != TextureId::default() => {
                let corners: Vec<Pos2> = mesh.vertices.iter().map(|vertex| vertex.pos).collect();
                let bounds = Rect::from_points(&corners);
                let corner = mesh
                    .vertices
                    .iter()
                    .find(|vertex| vertex.uv == Pos2::ZERO)?;
                near_rect(bounds, rect).then_some((corner.color, corner.pos))
            }
            _ => None,
        })
    }

    /// The fills painted over the whole of `rect` (none but transparent ones), and their
    /// corners.
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

    /// Within egui's rounding of a widget's place, a 32nd of a point.
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

    /// Whether a widget labelled `label` stands over `rect`.
    fn labelled_at<S>(h: &Harness<'_, S>, label: &str, rect: Rect) -> bool {
        // A group's node, labelled as a row may be, has no bounds.
        h.query_all_by_label(label).any(|node| {
            node.accesskit_node().bounding_box().is_some() && near_rect(node.rect(), rect)
        })
    }

    fn has<S>(h: &Harness<'_, S>, label: &str) -> bool {
        h.query_by_label(label).is_some()
    }

    /// A row's parts as `.row` lays them out, `top` below the pane's and `depth` deep: its
    /// box; its caret's box and chevron; its eye's box and glyph; its icon; where its name
    /// starts with no prefix; and where its padding at the right starts.
    struct Parts {
        row: Rect,
        caret: Rect,
        chevron: Rect,
        eye: Rect,
        glyph: Rect,
        icon: Rect,
        name: f32,
        end: f32,
    }

    fn parts(top: f32, depth: u8) -> Parts {
        let row = Rect::from_min_size(PANE + Vec2::new(0.0, top), Vec2::new(W, css::ROW_H));
        let middle = row.center().y;
        let square = |left: f32, side: f32| {
            Rect::from_min_size(Pos2::new(left, middle - side / 2.0), Vec2::splat(side))
        };
        let caret = square(
            row.left() + css::ROW_PADDING_LEFT + f32::from(depth) * css::INDENT,
            css::CARET,
        );
        let eye = square(caret.right() + css::ROW_GAP, css::EYE);
        let icon = square(eye.right() + css::ROW_GAP, css::ICON);
        Parts {
            row,
            caret,
            chevron: Rect::from_center_size(caret.center(), Vec2::splat(css::CARET_SVG)),
            eye,
            glyph: Rect::from_center_size(eye.center(), Vec2::splat(css::ICON)),
            icon,
            name: icon.right() + css::ROW_GAP,
            end: row.right() - css::ROW_PADDING_RIGHT,
        }
    }

    /// The mark's place at a row's right (`.r-err`, `.r-warn`: an icon, 14 square).
    fn mark_of(parts: &Parts) -> Rect {
        Rect::from_min_size(
            Pos2::new(
                parts.end - css::ICON,
                parts.row.center().y - css::ICON / 2.0,
            ),
            Vec2::splat(css::ICON),
        )
    }

    /// The name `name` on the row `parts`, from `left`, its line's middle the row's,
    /// `--fs`, in `ink`, cut short at `right`.
    #[track_caller]
    fn named<S>(
        h: &Harness<'_, S>,
        name: &str,
        parts: &Parts,
        left: f32,
        ink: Color32,
        right: f32,
    ) {
        let shown = text_in(h, name, parts.row);
        let size = shown.galley.size();
        at(
            shown.pos,
            Pos2::new(left, parts.row.center().y - size.y / 2.0),
            name,
        );
        assert_eq!(
            format(&shown).font_id,
            FontId::proportional(css::FS),
            "{name}"
        );
        assert_eq!(colour(&shown), ink, "{name}'s ink");
        // egui rounds a wrap's width to the point as it lays the text out.
        assert!(
            (shown.galley.job.wrap.max_width - (right - left)).abs() <= 0.5,
            "{name} cut at {}, not {}",
            shown.galley.job.wrap.max_width,
            right - left
        );
    }

    /// The meta `meta` on the row `parts`, ending at `right`: `--fs-small`, in `ink`.
    #[track_caller]
    fn meta_at<S>(h: &Harness<'_, S>, meta: &str, parts: &Parts, right: f32, ink: Color32) -> Rect {
        let shown = text_in(h, meta, parts.row);
        let size = shown.galley.size();
        at(
            shown.pos,
            Pos2::new(right - size.x, parts.row.center().y - size.y / 2.0),
            meta,
        );
        assert_eq!(format(&shown).font_id, FontId::proportional(css::FS_SMALL));
        assert_eq!(colour(&shown), ink, "{meta}'s ink");
        Rect::from_min_size(shown.pos, size)
    }

    /// The caret on `parts`' row, labelled `label`: a button over its box, its chevron 10
    /// square at its centre in Gray.700, turned a quarter clockwise while `open`.
    #[track_caller]
    fn caret_drawn<S>(h: &Harness<'_, S>, parts: &Parts, label: &str, open: bool) {
        sized(widget(h, Role::Button, label), parts.caret, label);
        let corner = if open {
            Pos2::new(parts.chevron.right(), parts.chevron.top())
        } else {
            parts.chevron.min
        };
        let drawn = turned_at(h, parts.chevron);
        assert!(
            drawn.is_some_and(|(tint, at)| tint == css::G700 && near_pos(at, corner)),
            "{label}'s chevron: {drawn:?}, not Gray.700 with its corner at {corner:?}"
        );
    }

    #[test]
    fn the_title_is_drawn_as_panel_title_over_the_panel_body() {
        let h = drawn(part(&[], 0, Vec::new(), Vec::new()));
        // `.panel-title`: `--pad` in, its line's middle the 24-high row's, 11 points in
        // capitals .5 apart, white.
        let bar = Rect::from_min_size(PANE, Vec2::new(W, css::TITLE_H));
        let title = text_in(&h, "BROWSER", bar);
        let size = title.galley.size();
        at(
            title.pos,
            Pos2::new(PANE.x + css::PAD, bar.center().y - size.y / 2.0),
            "the title",
        );
        assert_eq!(format(&title).font_id, FontId::proportional(css::FS_SMALL));
        assert_eq!(format(&title).extra_letter_spacing, css::CAPITALS_SPACING);
        assert_eq!(colour(&title), css::G1000);
        // `.panel-body` straight under it: the document's row its first, at 24.
        sized(
            widget(&h, Role::Label, "Part"),
            parts(css::TITLE_H, 0).row,
            "the document's row",
        );
    }

    #[test]
    fn a_click_on_the_documents_row_opens_whats_wrong_if_anything_is() {
        // The prototype's `rootRow`: the cut failed and the shell warned, so What's Wrong.
        let mut h = drawn(modelled());
        assert!(!tab_of(&mut h).whats_wrong);
        h.get_by_label("Part").click();
        h.run_steps(2);
        assert!(tab_of(&mut h).whats_wrong);
        // A warning alone is wrong enough.
        let features = [
            entry("b0000001", "box", &[], Health::Healthy),
            entry("s0000001", "box", &[], Health::Warning),
        ];
        let mut h = drawn(part(&features, 0, Vec::new(), Vec::new()));
        h.get_by_label("Part").click();
        h.run_steps(2);
        assert!(tab_of(&mut h).whats_wrong);
        // Nothing wrong: the document's own properties, the selection let go.
        let features = [entry("b0000001", "box", &[], Health::Healthy)];
        let mut tab = part(&features, 0, Vec::new(), Vec::new());
        tab.selection_mut().select_feature(fid("b0000001"));
        let mut h = drawn(tab);
        h.get_by_label("Part").click();
        h.run_steps(2);
        let tab = tab_of(&mut h);
        assert!(!tab.whats_wrong);
        assert!(tab.selection().is_empty());
        // Its caret still only opens and closes it.
        h.get_by_label("Collapse Part").click();
        h.run_steps(2);
        assert!(h.query_by_label("Expand Part").is_some());
        assert!(!tab_of(&mut h).whats_wrong);
    }

    #[test]
    fn the_documents_row_is_drawn_as_the_prototypes_root_row() {
        let features = [
            entry("b0000001", "box", &[], Health::Healthy),
            entry("s0000001", "box", &[], Health::Warning),
        ];
        let mut h = drawn(part(&features, 0, Vec::new(), Vec::new()));
        let root = parts(css::TITLE_H, 0);
        // Depth 0: the caret, open; no eye; the document's icon; its name, the file's
        // without `.silico`, to the padding; nothing at its right, the gate taking it, no
        // feature failed (a warning marks nothing), and saved.
        sized(widget(&h, Role::Label, "Part"), root.row, "the row");
        caret_drawn(&h, &root, "Collapse Part", true);
        assert_eq!(icon_at(&h, root.glyph), None, "its eye");
        assert!(!has(&h, "Hide Part") && !has(&h, "Show Part"));
        assert_eq!(icon_at(&h, root.icon), Some(css::G500), "its icon");
        named(&h, "Part", &root, root.name, css::G700, root.end);
        assert_eq!(icon_at(&h, mark_of(&root)), None, "its mark");
        assert!(!has(&h, "error"));
        // Unsaved: `unsaved` at its right, 11 points, Gray.550; its name cut 4 before it.
        let tab = tab_of(&mut h);
        tab.transaction = true;
        tab.revision = tab.revision.next().unwrap();
        h.run_steps(2);
        let meta = meta_at(&h, "unsaved", &root, root.end, css::G550);
        named(
            &h,
            "Part",
            &root,
            root.name,
            css::G700,
            meta.left() - css::ROW_GAP,
        );
        // The gate's problems mark it: the error's icon in `--red` at its right, said with
        // the document's first problem; `unsaved` 4 before the mark, the name 4 before that.
        let tab = tab_of(&mut h);
        tab.timeline.as_mut().unwrap().problems = 1;
        tab.document_problems.push(Said {
            message: "Two features share the ID block".to_owned(),
            ..Said::default()
        });
        h.run_steps(2);
        let mark = mark_of(&root);
        assert_eq!(icon_at(&h, mark), Some(css::RED), "its mark");
        sized(
            widget(&h, Role::Label, "error: Two features share the ID block"),
            mark,
            "its mark",
        );
        let meta = meta_at(&h, "unsaved", &root, mark.left() - css::ROW_GAP, css::G550);
        named(
            &h,
            "Part",
            &root,
            root.name,
            css::G700,
            meta.left() - css::ROW_GAP,
        );
        // A feature failed marks it too, the gate taking the document.
        let tab = tab_of(&mut h);
        tab.document_problems.clear();
        tab.timeline = Some(timeline(
            &[
                entry("b0000001", "box", &[], Health::Error),
                entry("s0000001", "box", &[], Health::Warning),
            ],
            &[],
        ));
        h.run_steps(2);
        assert_eq!(icon_at(&h, mark), Some(css::RED), "a failed feature's mark");
        sized(widget(&h, Role::Label, "error"), mark, "its mark");
        // Folded: its caret turned back, nothing under it.
        h.get_by_label("Collapse Part").click();
        h.run_steps(2);
        caret_drawn(&h, &root, "Expand Part", false);
        assert!(!has(&h, "Origin"));
    }

    #[test]
    fn the_group_headers_are_drawn_as_the_prototypes_folder_rows_with_their_carets() {
        let features = [entry("b0000001", "box", &[], Health::Healthy)];
        let mut h = drawn(part(
            &features,
            0,
            vec![listed("b0000001", "Block", Health::Healthy)],
            Vec::new(),
        ));
        // Depth 1, each 24 below the last row above it: Origin under the document, its
        // seven rows under it; Bodies, its one row; Sketches, with none, `0` at its right.
        let down = |n: f32| css::TITLE_H + n * css::ROW_H;
        for (name, top, meta) in [
            ("Origin", down(1.0), None),
            ("Bodies", down(9.0), None),
            ("Sketches", down(11.0), Some("0")),
        ] {
            let header = parts(top, 1);
            assert!(labelled_at(&h, name, header.row), "{name}'s row");
            caret_drawn(&h, &header, &format!("Collapse {name}"), true);
            if name == "Origin" {
                // The planes' eye (the prototype's folder row's), closed while none shows,
                // as they open: Gray.400, the eye off's, the folder not faded.
                assert!(labelled_at(&h, "Show origin planes", header.eye));
                assert_eq!(icon_at(&h, header.glyph), Some(css::G400), "{name}'s eye");
            } else {
                assert_eq!(icon_at(&h, header.glyph), None, "{name}'s eye");
            }
            assert_eq!(icon_at(&h, header.icon), Some(css::G500), "{name}'s icon");
            let right = match meta {
                Some(meta) => {
                    meta_at(&h, meta, &header, header.end, css::G550).left() - css::ROW_GAP
                }
                None => header.end,
            };
            named(&h, name, &header, header.name, css::G700, right);
        }
        assert!(!has(&h, "Hide Bodies") && !has(&h, "Hide Sketches"));
        // Origin folded: its caret turned back, Bodies straight under it.
        h.get_by_label("Collapse Origin").click();
        h.run_steps(2);
        caret_drawn(&h, &parts(down(1.0), 1), "Expand Origin", false);
        sized(
            widget(&h, Role::Label, "Bodies"),
            parts(down(2.0), 1).row,
            "Bodies",
        );
        assert!(!has(&h, "Hide XY"));
    }

    /// A row expected: its name, its top below the pane's, its name's ink, and its mark's
    /// tint and what it says, if it has one.
    type Expected<'a> = (&'a str, f32, Color32, Option<(Color32, &'a str)>);

    #[test]
    fn a_row_another_program_changed_has_its_info_dot_where_it_has_no_health_mark() {
        // The prototype's row (`o.mark === 'outside'`): a `.state-dot` (8 square, round) in
        // `--outside` (the info tone, #adbbff; Trevin's change of 2026-11-09, from purple) at
        // the row's right, the name cut 4 (`.row`'s gap) before it; a row with a health mark
        // shows that mark alone.
        const OUTSIDE: Color32 = Color32::from_rgb(0xad, 0xbb, 0xff);
        const DOT: f32 = 8.0;
        let mut tab = modelled();
        // Named long enough to be cut short by the dot.
        let long = "Block with a name long enough to run past the browser's edge";
        let mut bodies = tab.browser.bodies().to_vec();
        bodies[0].name = long.to_owned();
        tab.browser = crate::tab::Browser::new(bodies, tab.browser.sketches().to_vec());
        for id in ["b0000001", "c0000001"] {
            tab.outside.insert(fid(id), "height 10 → 11".to_owned());
        }
        let h = drawn(tab);
        let down = |n: f32| css::TITLE_H + n * css::ROW_H;
        let block = parts(down(10.0), 2);
        let dot = Rect::from_min_size(
            Pos2::new(block.end - DOT, block.row.center().y - DOT / 2.0),
            Vec2::splat(DOT),
        );
        let circles: Vec<(Pos2, f32, Color32)> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Circle(circle) if block.row.contains(circle.center) => {
                    Some((circle.center, circle.radius, circle.fill))
                }
                _ => None,
            })
            .collect();
        assert!(
            matches!(circles[..], [(centre, radius, OUTSIDE)]
                if near_pos(centre, dot.center()) && near(radius, DOT / 2.0)),
            "the block's dot: {circles:?}, not {DOT} across at {:?}",
            dot.center()
        );
        assert!(labelled_at(&h, "changed on disk: height 10 → 11", dot));
        named(
            &h,
            long,
            &block,
            block.name,
            css::G700,
            dot.left() - css::ROW_GAP,
        );
        // The failed cut: its error's mark, no dot.
        let cut = parts(down(11.0), 2);
        assert_eq!(icon_at(&h, mark_of(&cut)), Some(css::RED));
        assert!(
            !painted(&h).iter().any(
                |shape| matches!(shape, Shape::Circle(circle) if cut.row.contains(circle.center))
            ),
            "the cut's dot"
        );
    }

    #[test]
    fn the_rows_are_drawn_as_the_prototypes_rows_with_their_eyes_icons_names_and_marks() {
        let h = drawn(modelled());
        let down = |n: f32| css::TITLE_H + n * css::ROW_H;
        // Depth 2, no caret: the eye, open, Gray.550 (an origin plane's closed, Gray.400: the
        // planes open hidden, as the prototype's, its row not faded); the icon, Gray.500; the
        // name, Gray.700, a failed sketch's `--red`, cut 4 before its mark, if it has one:
        // `--red` for an error, `--orange` for a warning, said with what's said of its
        // feature.
        let origin = ["Origin", "X", "Y", "Z", "XY", "XZ", "YZ"];
        let mut rows: Vec<Expected<'_>> = origin
            .iter()
            .enumerate()
            .map(|(n, name)| (*name, down(2.0 + n as f32), css::G700, None))
            .collect();
        rows.extend([
            ("Block", down(10.0), css::G700, None),
            (
                "Cut",
                down(11.0),
                css::G700,
                Some((css::RED, "error: The cut removes nothing")),
            ),
            (
                "Shell",
                down(12.0),
                css::G700,
                Some((css::ORANGE, "warning")),
            ),
            ("Profile", down(14.0), css::G700, None),
            ("Bad", down(15.0), css::RED, Some((css::RED, "error"))),
        ]);
        for (name, top, ink, mark) in rows {
            let laid = parts(top, 2);
            assert!(
                labelled_at(&h, name, laid.row),
                "{name}'s row at {:?}",
                laid.row
            );
            assert_eq!(turned_at(&h, laid.chevron), None, "{name}'s caret");
            let plane = ["XY", "XZ", "YZ"].contains(&name);
            let (word, glyph) = if plane {
                ("Show", css::G400)
            } else {
                ("Hide", css::G550)
            };
            let eye = format!("{word} {name}");
            assert!(labelled_at(&h, &eye, laid.eye), "{eye} at {:?}", laid.eye);
            assert_eq!(icon_at(&h, laid.glyph), Some(glyph), "{name}'s eye");
            assert_eq!(icon_at(&h, laid.icon), Some(css::G500), "{name}'s icon");
            let right = match mark {
                Some((tint, said)) => {
                    let at = mark_of(&laid);
                    assert_eq!(icon_at(&h, at), Some(tint), "{name}'s mark");
                    assert!(labelled_at(&h, said, at), "{said} at {at:?}");
                    at.left() - css::ROW_GAP
                }
                None => {
                    assert_eq!(icon_at(&h, mark_of(&laid)), None, "{name}'s mark");
                    laid.end
                }
            };
            named(&h, name, &laid, laid.name, ink, right);
        }
        // No sketch has a prefix yet (#351), and no row asked for Edit Sketch.
        assert!(
            texts(&h)
                .iter()
                .all(|shown| format(shown).font_id.family != FontFamily::Monospace),
            "a prefix"
        );
        assert!(!h.state().next_frame, "an edit asked");

        // A sketch's prefix (`.row .prefix`), as a row with one draws it: 4 after the icon,
        // 11 points of the monospace font, its state's colour on a selected row too; the
        // name 4 after it; the meta `--blue-text` on a selected row.
        let mut h = Harness::builder()
            .with_size(Vec2::new(PANE.x + W + 40.0, PANE.y + 100.0))
            .build_ui(|ui| {
                let pane = Rect::from_min_size(PANE, Vec2::new(W, css::ROW_H));
                ui.scope_builder(UiBuilder::new().max_rect(pane), |ui| {
                    row(
                        ui,
                        &Line {
                            id: egui::Id::new("prefixed"),
                            depth: 2,
                            caret: None,
                            eye: Some(true),
                            not_drawn: None,
                            eye_named: None,
                            fades: true,
                            icon: Icon::Sketch,
                            prefix: Some("(–)"),
                            name: "Profile",
                            selected: Some(true),
                            meta: Some("XY"),
                            mark: Some((Health::Warning, None)),
                            failed: false,
                            outside: None,
                            clicks: false,
                        },
                    );
                });
            });
        theme::apply(&h.ctx);
        h.run_steps(4);
        let laid = parts(0.0, 2);
        assert_eq!(icon_at(&h, laid.icon), Some(css::BLUE_TEXT), "the icon");
        let prefix = text_in(&h, "(–)", laid.row);
        let size = prefix.galley.size();
        at(
            prefix.pos,
            Pos2::new(laid.name, laid.row.center().y - size.y / 2.0),
            "the prefix",
        );
        assert_eq!(format(&prefix).font_id, FontId::monospace(css::PREFIX));
        assert_eq!(colour(&prefix), css::BLUE_SOFT);
        let mark = mark_of(&laid);
        assert_eq!(icon_at(&h, mark), Some(css::ORANGE), "the mark");
        let meta = meta_at(&h, "XY", &laid, mark.left() - css::ROW_GAP, css::BLUE_TEXT);
        named(
            &h,
            "Profile",
            &laid,
            laid.name + size.x + css::ROW_GAP,
            css::BLUE_TEXT,
            meta.left() - css::ROW_GAP,
        );
    }

    /// At the browser's least column, 200 (pane_widths.rs), less its border, 199: a sketch's
    /// row two levels in, with its prefix and a health mark, shows a default name with two
    /// digits, `Sketch12`, whole (by hand from the parts' widths: 100 before the prefix, the
    /// prefix's 19.8 and 4, then the name, and 4, the mark's 14, and 6 after it, which leave
    /// the name 51.2, and egui fits it by its last glyph's ink, 51.0, not its advance, 52.2);
    /// and a body's, with its mark, the longest default name, `Intersect`. A pane a point
    /// narrower cuts the sketch's name short, so the test can tell.
    #[test]
    fn at_the_least_browser_a_deep_rows_parts_and_a_default_name_fit() {
        let drawn_at = |width: f32| {
            let mut h = Harness::builder()
                .with_size(Vec2::new(PANE.x + width + 40.0, PANE.y + 100.0))
                .build_ui(move |ui| {
                    let rows = [
                        (Icon::Sketch, Some("(–)"), "Sketch12", Health::Error),
                        (Icon::Body, None, "Intersect", Health::Warning),
                    ];
                    for (at, (icon, prefix, name, health)) in rows.into_iter().enumerate() {
                        let top = PANE.y + at as f32 * css::ROW_H;
                        let pane = Rect::from_min_size(
                            Pos2::new(PANE.x, top),
                            Vec2::new(width, css::ROW_H),
                        );
                        ui.scope_builder(UiBuilder::new().max_rect(pane), |ui| {
                            row(
                                ui,
                                &Line {
                                    id: egui::Id::new(("least", at)),
                                    depth: 2,
                                    caret: None,
                                    eye: Some(true),
                                    not_drawn: None,
                                    eye_named: None,
                                    fades: true,
                                    icon,
                                    prefix,
                                    name,
                                    selected: Some(false),
                                    meta: None,
                                    mark: Some((health, None)),
                                    failed: false,
                                    outside: None,
                                    clicks: false,
                                },
                            );
                        });
                    }
                });
            theme::apply(&h.ctx);
            h.run_steps(4);
            h
        };
        let elided = |h: &Harness<'_>, name: &str| {
            let shown: Vec<bool> = texts(h)
                .iter()
                .filter(|shown| shown.galley.text() == name)
                .map(|shown| shown.galley.elided)
                .collect();
            assert_eq!(shown.len(), 1, "{name} painted {} times", shown.len());
            shown[0]
        };
        let h = drawn_at(crate::pane_widths::BROWSER_MIN_W - css::BORDER);
        assert!(!elided(&h, "Sketch12"), "Sketch12 cut short");
        assert!(!elided(&h, "Intersect"), "Intersect cut short");
        let h = drawn_at(198.0);
        assert!(elided(&h, "Sketch12"), "Sketch12 whole at 198");
    }

    #[test]
    fn a_row_hovered_or_selected_is_filled_as_row_hover_and_row_selected_and_an_eye_as_eye_hover() {
        let mut h = drawn(modelled());
        let down = |n: f32| css::TITLE_H + n * css::ROW_H;
        let block = parts(down(10.0), 2);
        let cut = parts(down(11.0), 2);
        // At rest: no fill.
        h.remove_cursor();
        h.run_steps(2);
        assert!(fills_at(&h, block.row).is_empty());
        // Hovered (`.row:hover`): Gray.250 across it, square; its name Gray.800, its icon
        // Gray.500 still.
        h.get_by_label("Block").hover();
        h.run_steps(2);
        assert_eq!(fills_at(&h, block.row), [(css::G250, CornerRadius::ZERO)]);
        assert_eq!(colour(&text_in(&h, "Block", block.row)), css::G800);
        assert_eq!(icon_at(&h, block.icon), Some(css::G500));
        // Its eye hovered (`.eye:hover`): Gray.350 behind it, 3 round, its glyph white.
        h.get_by_label("Hide Block").hover();
        h.run_steps(2);
        assert_eq!(
            fills_at(&h, block.eye),
            [(css::G350, CornerRadius::same(css::EYE_RADIUS))]
        );
        assert_eq!(icon_at(&h, block.glyph), Some(css::G1000));
        // Selected (`.row.selected`): `--blue` across it, its name and icon `--blue-text`;
        // hovered, the same.
        tab_of(&mut h)
            .selection_mut()
            .select_feature(fid("c0000001"));
        h.remove_cursor();
        h.run_steps(2);
        assert_eq!(fills_at(&h, cut.row), [(css::BLUE, CornerRadius::ZERO)]);
        assert_eq!(colour(&text_in(&h, "Cut", cut.row)), css::BLUE_TEXT);
        assert_eq!(icon_at(&h, cut.icon), Some(css::BLUE_TEXT));
        assert!(fills_at(&h, block.row).is_empty());
        h.get_by_label("Cut").hover();
        h.run_steps(2);
        assert_eq!(fills_at(&h, cut.row), [(css::BLUE, CornerRadius::ZERO)]);
        assert_eq!(colour(&text_in(&h, "Cut", cut.row)), css::BLUE_TEXT);
    }

    #[test]
    fn a_hidden_rows_name_and_icon_are_dimmed_as_hidden_obj_from_the_frame_its_eye_is_clicked() {
        let mut tab = modelled();
        tab.overrides.insert(
            crate::tab::Hideable::Feature(fid("b0000001")),
            crate::tab::Override {
                shown: false,
                under: Default::default(),
            },
        );
        let mut h = drawn(tab);
        h.remove_cursor();
        h.run_steps(2);
        let down = |n: f32| css::TITLE_H + n * css::ROW_H;
        let dim = |colour: Color32| {
            let [r, g, b, _] = colour.to_array();
            Color32::from_rgba_unmultiplied(r, g, b, css::HIDDEN_ALPHA)
        };
        // Hidden (`.hidden-obj`, `.eye.off`): its eye closed, Gray.400; its name and icon at
        // .45; nothing else of the row's dimmed.
        let block = parts(down(10.0), 2);
        assert!(has(&h, "Show Block") && !has(&h, "Hide Block"));
        assert_eq!(icon_at(&h, block.glyph), Some(css::G400));
        assert_eq!(icon_at(&h, block.icon), Some(dim(css::G500)));
        named(&h, "Block", &block, block.name, dim(css::G700), block.end);
        // Its eye clicked: dimmed in the frame of the click, hovered as it is.
        let shell = parts(down(12.0), 2);
        h.get_by_label("Hide Shell").click();
        h.step();
        assert!(has(&h, "Show Shell") && !has(&h, "Hide Shell"));
        assert_eq!(icon_at(&h, shell.icon), Some(dim(css::G500)));
        assert_eq!(
            colour(&text_in(&h, "Shell", shell.row)),
            dim(css::G800),
            "the name hovered and hidden"
        );
        h.run_steps(2);
        assert!(!tab_of(&mut h).shown(&crate::tab::Hideable::Feature(fid("s0000001"))));
        assert!(has(&h, "Show Shell"));
    }

    #[test]
    fn an_empty_groups_words_are_drawn_as_tree_empty_saying_what_makes_its_first_row() {
        let features = [entry("p0000001", "sketch", &[], Health::Healthy)];
        let problem = || Said {
            message: "The pad's sketch is open".to_owned(),
            ..Said::default()
        };
        let down = |n: f32| css::TITLE_H + n * css::ROW_H;
        let none_yet = "None yet. A pad makes one.";
        for (features, problem, bodies) in [
            (&[][..], None, none_yet),
            (&[][..], Some(problem()), none_yet),
            (&features[..], None, "No body: nothing builds one yet"),
            (
                &features[..],
                Some(problem()),
                "No body: The pad's sketch is open",
            ),
        ] {
            let mut tab = part(features, 0, Vec::new(), Vec::new());
            tab.document_problems.extend(problem);
            let h = drawn(tab);
            // Under Bodies' header: 2 down and `--pad` + 36 in, 11 points, Gray.550, wrapped
            // 12 before the pane's edge, on a 16-high line; Sketches' header 6 below it.
            let header = parts(down(9.0), 1).row;
            let mut top = header.bottom();
            for (words, next) in [
                (bodies, "Sketches"),
                ("None yet. Create Sketch starts one.", ""),
            ] {
                let area = Rect::from_min_size(Pos2::new(PANE.x, top), Vec2::new(W, css::ROW_H));
                let shown = text_in(&h, words, area);
                at(
                    shown.pos,
                    Pos2::new(PANE.x + css::EMPTY_LEFT, top + css::EMPTY_TOP),
                    words,
                );
                assert_eq!(format(&shown).font_id, FontId::proportional(css::FS_SMALL));
                assert_eq!(colour(&shown), css::G550, "{words}");
                let wrap = shown.galley.job.wrap.max_width;
                let width = W - css::EMPTY_LEFT - css::EMPTY_RIGHT;
                assert!(near(wrap, width), "{words} wrapped at {wrap}, not {width}");
                let lines = shown.galley.rows.len() as f32;
                let high = shown.galley.size().y;
                assert!(
                    near(high, lines * css::LH),
                    "{words}: {lines} lines {high} high, not 16 each"
                );
                top += css::EMPTY_TOP + lines * css::LH + css::EMPTY_BOTTOM;
                if !next.is_empty() {
                    let at = parts(top - PANE.y, 1).row;
                    sized(widget(&h, Role::Label, next), at, next);
                    top = at.bottom();
                }
            }
        }
    }

    #[test]
    fn a_row_found_opens_its_own_group_and_what_a_rename_or_a_menu_sends_is_polled() {
        use eframe::egui::Key;
        let mut h = drawn(modelled());
        // Both groups folded: no row of either shown.
        for group in ["Bodies", "Sketches"] {
            h.get_by_label(&format!("Collapse {group}")).click();
            h.run_steps(2);
        }
        assert!(!has(&h, "Hide Block") && !has(&h, "Hide Profile"));
        // Section 9, *Browser and canvas*: the browser expands and scrolls to a row found
        // (Find in browser): a body's in Bodies, which opens, the sketches left folded.
        tab_of(&mut h)
            .selection_mut()
            .find_in_browser(fid("b0000001"));
        h.run_steps(3);
        assert!(has(&h, "Hide Block"), "the block's row");
        assert!(has(&h, "Expand Sketches") && !has(&h, "Hide Profile"));
        // A sketch's in Sketches.
        tab_of(&mut h)
            .selection_mut()
            .find_in_browser(fid("p0000001"));
        h.run_steps(3);
        assert!(has(&h, "Hide Profile"), "the sketch's row");

        // F2 on the block: its row's field; a name typed, then Enter, sent, and the next
        // frame asked for, to poll it.
        assert!(!h.state().next_frame);
        tab_of(&mut h).start_rename(fid("b0000001"), crate::feature_menu::Place::Browser);
        h.run_steps(3);
        assert!(h.get_by_role(Role::TextInput).is_focused());
        h.get_by_role(Role::TextInput).type_text("Plate");
        h.run_steps(2);
        assert!(!h.state().next_frame, "sent as it's typed");
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert!(h.state().next_frame, "the name sent, never polled");
        assert!(tab_of(&mut h).renaming.is_none());

        // The menu's Suppress on the shell's row: sent, and polled the same.
        let mut h = drawn(modelled());
        h.get_by_label("Shell").click_secondary();
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "Suppress").click();
        h.run_steps(2);
        assert!(h.state().next_frame, "the suppression sent, never polled");
    }
}

#[cfg(test)]
mod property_panel_drawn {
    // ---- The property panel as drawn, against the prototype ----
    //
    // Each value below is typed from the approved prototype's stylesheet
    // (docs/design/m5-ui/styles.css: `:root`'s tokens, `#right`, `.panel-title`, `.prop-head`,
    // `.section`, `.prop`, `.hint-block`, `.err-card` and its parts, `.field`, `.dlg-footer`,
    // `.btn`, `.btn-primary`, `.btn-secondary`) and js/app.js's `docProps`, `featureProps`, and
    // `renderDialog`, never from theme.rs, and compared with what the panel painted: each
    // rectangle, line, icon, and text from egui's shapes, and each widget's bounds from the
    // accessibility tree.

    use eframe::egui::epaint::{RectShape, TextShape};
    use eframe::egui::{CornerRadius, FontFamily, FontId, Pos2, Rect, Shape, Stroke};
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use silico_core::command::{Problems, Revision, TimelineAnswer};
    use silico_core::fake::FakeKernel;

    use super::*;
    use crate::dialog::{Action, Dialog, Kind};
    use crate::tab::Said;

    /// The prototype's tokens and geometry.
    mod css {
        use eframe::egui::Color32;

        // `:root`'s greys and colours.
        pub const G100: Color32 = Color32::from_rgb(0x0d, 0x0d, 0x0d);
        pub const G150: Color32 = Color32::from_rgb(0x17, 0x17, 0x17);
        pub const G200: Color32 = Color32::from_rgb(0x21, 0x21, 0x21);
        pub const G250: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
        pub const G300: Color32 = Color32::from_rgb(0x2c, 0x2b, 0x2b);
        pub const G550: Color32 = Color32::from_rgb(0x93, 0x90, 0x90);
        pub const G750: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);
        pub const G800: Color32 = Color32::from_rgb(0xe0, 0xdf, 0xdf);
        pub const G1000: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
        /// `--red` (`.err-text`).
        pub const RED: Color32 = Color32::from_rgb(0xff, 0x38, 0x65);
        /// `--alert-red-bg: rgba(236, 0, 79, 0.18)`: an alpha of 255 × .18 = 45.9, 46.
        pub const ALERT_RED_BG: Color32 = Color32::from_rgba_unmultiplied_const(236, 0, 79, 46);
        /// `.err-card { border: 1px solid #5c001a }`; `.ec-head { color: #ffd1d6 }`;
        /// `.ec-fix::before { color: #ff99a6 }`.
        pub const CARD_BORDER: Color32 = Color32::from_rgb(0x5c, 0x00, 0x1a);
        pub const CARD_HEAD: Color32 = Color32::from_rgb(0xff, 0xd1, 0xd6);
        pub const CARD_ACCENT: Color32 = Color32::from_rgb(0xff, 0x99, 0xa6);

        /// `--fs`, `--lh`, `--fs-small`, `.mono`'s size, `--icon`, `--r-sm`, `--r`.
        pub const FS: f32 = 12.0;
        pub const LH: f32 = 16.0;
        pub const FS_SMALL: f32 = 11.0;
        pub const MONO: f32 = 11.0;
        pub const ICON: f32 = 14.0;
        pub const R_SM: u8 = 4;
        pub const R: u8 = 6;
        /// `--right-w`, and `#right`'s `border-left: 1px` within it (`border-box`).
        pub const RIGHT_W: f32 = 296.0;
        pub const BORDER: f32 = 1.0;
        /// `.panel-title`: `height: var(--title-h)`, `padding-left: var(--pad)`,
        /// `letter-spacing: .5px`.
        pub const TITLE_H: f32 = 24.0;
        pub const PAD: f32 = 12.0;
        pub const CAPITALS_SPACING: f32 = 0.5;
        /// `.prop-head`: `padding: 6px var(--pad) 8px; gap: 8px`; `.ph-icon`: 28 square;
        /// `.ph-title`: `font-size: 13px`.
        pub const HEAD_TOP: f32 = 6.0;
        pub const HEAD_BOTTOM: f32 = 8.0;
        pub const HEAD_GAP: f32 = 8.0;
        pub const PH_ICON: f32 = 28.0;
        pub const PH_TITLE: f32 = 13.0;
        /// `.section`: `padding: 6px var(--pad) 2px; font-size: 10.5px; gap: 6px`; its
        /// `::after` 1 high, Gray.200.
        pub const SECTION_TOP: f32 = 6.0;
        pub const SECTION_BOTTOM: f32 = 2.0;
        pub const SECTION_FS: f32 = 10.5;
        pub const SECTION_GAP: f32 = 6.0;
        /// `.prop`: `min-height: var(--row-h); grid-template-columns: 96px 1fr; gap: 8px`.
        pub const ROW_H: f32 = 24.0;
        pub const KEY_W: f32 = 96.0;
        pub const ROW_GAP: f32 = 8.0;
        /// `.hint-block`: `margin: 8px var(--pad); padding: 8px 10px`.
        pub const HINT_MARGIN: f32 = 8.0;
        pub const HINT_PADDING_Y: f32 = 8.0;
        pub const HINT_PADDING_X: f32 = 10.0;
        /// `.err-card`: `margin: 8px var(--pad)`; `.ec-head`: `padding: 8px 10px 4px; gap:
        /// 8px`; `.ec-body`: `padding: 0 10px 8px 32px`; `.ec-fix`: `gap: 6px;
        /// margin-bottom: 8px`.
        pub const CARD_MARGIN: f32 = 8.0;
        pub const EC_HEAD_TOP: f32 = 8.0;
        pub const EC_HEAD_SIDE: f32 = 10.0;
        pub const EC_HEAD_BOTTOM: f32 = 4.0;
        pub const EC_HEAD_GAP: f32 = 8.0;
        pub const EC_BODY_LEFT: f32 = 32.0;
        pub const EC_BODY_BOTTOM: f32 = 8.0;
        pub const EC_FIX_GAP: f32 = 6.0;
        pub const EC_FIX_MARGIN: f32 = 8.0;
        /// `.ec-body { color: #ffe2e5 }`; `.ec-body p { margin: 0 0 6px }`.
        pub const EC_BODY_TEXT: Color32 = Color32::from_rgb(0xff, 0xe2, 0xe5);
        pub const EC_P_MARGIN: f32 = 6.0;
        /// `.ec-code`: `font-size: 10px` (`--mono`), `background: rgba(0,0,0,.3)` (255 × .3 =
        /// 76.5, 77), `padding: 0 4px; border-radius: 3px`; on the head's line (`--lh`).
        pub const EC_CODE_FS: f32 = 10.0;
        pub const EC_CODE_BG: Color32 = Color32::from_rgba_unmultiplied_const(0, 0, 0, 77);
        pub const EC_CODE_PADDING: f32 = 4.0;
        pub const EC_CODE_RADIUS: u8 = 3;
        /// `.ec-kv`: `gap: 2px 8px; font-size: var(--fs-small); margin-bottom: 6px`.
        pub const EC_KV_ROW_GAP: f32 = 2.0;
        pub const EC_KV_COLUMN_GAP: f32 = 8.0;
        pub const EC_KV_MARGIN: f32 = 6.0;
        /// `.mono { opacity: .7 }` after the feature's name: an alpha of 255 × .7 = 178.5,
        /// 179.
        pub const EC_ID_ALPHA: u8 = 179;
        /// `.el-chip`: `font-size: 10.5px` (`--mono`), `padding: 0 5px; border-radius: 3px;
        /// background: rgba(0,0,0,.35)` (255 × .35 = 89.25, 89), white.
        pub const EL_CHIP_FS: f32 = 10.5;
        pub const EL_CHIP_PADDING: f32 = 5.0;
        pub const EL_CHIP_RADIUS: u8 = 3;
        pub const EL_CHIP_BG: Color32 = Color32::from_rgba_unmultiplied_const(0, 0, 0, 89);
        /// The candidates' heading (`font-size: 10.5px; margin-bottom: 4px; letter-spacing:
        /// .5px`, in capitals); `.cand`: `padding: 5px 6px; border-radius: var(--r-sm);
        /// background: rgba(0,0,0,.25)` (255 × .25 = 63.75, 64); `margin-bottom: 4px`.
        pub const CANDS_FS: f32 = 10.5;
        pub const CANDS_MARGIN: f32 = 4.0;
        pub const CAND_PADDING_X: f32 = 6.0;
        pub const CAND_PADDING_Y: f32 = 5.0;
        pub const CAND_BG: Color32 = Color32::from_rgba_unmultiplied_const(0, 0, 0, 64);
        pub const CAND_MARGIN: f32 = 4.0;
        pub const CAND_TEXT: Color32 = Color32::from_rgb(0xff, 0xd1, 0xd6);
        /// `.ec-actions { gap: 6px }`; `.btn { height: 22px }`.
        pub const EC_ACTIONS_GAP: f32 = 6.0;
        pub const BTN_H: f32 = 22.0;
        /// `selectionProps`' buttons: `padding: 10px var(--pad); gap: 6px`.
        pub const BUTTONS_PADDING: f32 = 10.0;
        pub const BUTTONS_GAP: f32 = 6.0;
        /// `--green` (`.ok-text`), `--info` (`.link`).
        pub const GREEN: Color32 = Color32::from_rgb(0x00, 0xda, 0x7e);
        pub const INFO: Color32 = Color32::from_rgb(0xad, 0xbb, 0xff);
        /// `.field`: `height: 22px`, Gray.200, corners `--r-sm`.
        pub const FIELD_H: f32 = 22.0;
        /// `.dlg-footer`: `padding: 8px var(--pad); gap: 6px; border-top: 1px`; its `.btn`
        /// 26 high.
        pub const FOOTER_PADDING: f32 = 8.0;
        pub const FOOTER_GAP: f32 = 6.0;
        pub const FOOTER_BTN_H: f32 = 26.0;
        /// `.field.select`: `padding: 0 6px`, its `svg` 10 square; `.field input`: `padding:
        /// 0 6px`; `.field .unit`: `padding: 0 6px 0 0`.
        pub const SELECT_PADDING: f32 = 6.0;
        pub const SELECT_ARROW: f32 = 10.0;
        pub const INPUT_PADDING: f32 = 6.0;
        pub const UNIT_PADDING: f32 = 6.0;
        /// `.dlg-status`: `padding: 6px 8px; gap: 6px`.
        pub const STATUS_PADDING_X: f32 = 8.0;
        pub const STATUS_PADDING_Y: f32 = 6.0;
        pub const STATUS_GAP: f32 = 6.0;
        /// `.panel-title`: `padding-right: 6px; gap: 6px`; `.panel-title .ab-btn`: 20
        /// square.
        pub const TITLE_PADDING_RIGHT: f32 = 6.0;
        pub const TITLE_BTN: f32 = 20.0;
        pub const TITLE_GAP: f32 = 6.0;
    }

    /// The pane's top left in the harness's window, its width (`#right`'s, less its
    /// border), and its height.
    const PANE: Pos2 = Pos2::new(40.0, 30.0);
    const W: f32 = css::RIGHT_W - css::BORDER;
    const HEIGHT: f32 = 640.0;

    type Fake = Arc<FakeKernel>;

    struct Shown {
        tab: Option<Tab>,
        sessions: Arc<Sessions<Fake>>,
        name: Option<NameField>,
        next_frame: bool,
        drawing: crate::viewport::Drawing,
        remembered: crate::dialog::Remembered,
        toggled: bool,
    }

    fn fid(id: &str) -> FeatureId {
        FeatureId::new(id).unwrap()
    }

    fn entry(id: &str, kind: &'static str, name: &str, health: Health) -> Entry {
        Entry {
            id: fid(id),
            kind,
            name: name.to_owned(),
            suppressed: false,
            inputs: Vec::new(),
            health,
            problems: u64::from(health == Health::Error),
            warnings: 0,
        }
    }

    /// The tab of `/models/Part.silico` at its first revision, its model built: Block and
    /// Hole (failed: "The cut removes nothing"), Block the one body.
    fn part() -> Tab {
        let none = Problems::from(silico_core::Error::new(
            silico_core::ErrorCode::Internal,
            "unused",
        ));
        let mut tab = Tab::opened(
            SessionId::parse("s1").unwrap(),
            "/models/Part.silico".to_owned(),
            &none,
        );
        tab.document_problems.clear();
        tab.timeline = Some(TimelineAnswer {
            revision: Revision::OPENED,
            model: Some(Revision::OPENED),
            problems: 0,
            features: Some(vec![
                entry("b0000001", "box", "Block", Health::Healthy),
                entry("c0000001", "cut", "Hole", Health::Error),
            ]),
            bodies: vec![fid("b0000001")],
        });
        tab.said.insert(
            "c0000001".to_owned(),
            Said {
                message: "The cut removes nothing".to_owned(),
                suggestion: Some("Move the pin".to_owned()),
                ..Said::default()
            },
        );
        tab
    }

    fn drawn(tab: Tab) -> Harness<'static, Shown> {
        drawn_at(tab, W)
    }

    /// `tab`'s properties drawn in a pane `width` wide.
    fn drawn_at(tab: Tab, width: f32) -> Harness<'static, Shown> {
        let shown = Shown {
            tab: Some(tab),
            sessions: Arc::new(Sessions::with_cache_opener(
                Arc::new(FakeKernel::new(1).unwrap()),
                |dir| silico_core::cache::Cache::unsealed(dir),
            )),
            name: None,
            next_frame: false,
            drawing: crate::viewport::Drawing::default(),
            remembered: crate::dialog::Remembered::default(),
            toggled: false,
        };
        let mut h = Harness::builder()
            .with_size(Vec2::new(PANE.x + width + 40.0, PANE.y + HEIGHT + 40.0))
            .build_ui_state(
                move |ui, shown: &mut Shown| {
                    let pane = Rect::from_min_size(PANE, Vec2::new(width, HEIGHT));
                    ui.scope_builder(UiBuilder::new().max_rect(pane), |ui| {
                        Panes {
                            tab: shown.tab.as_mut(),
                            sessions: &shown.sessions,
                            name: &mut shown.name,
                            next_frame: &mut shown.next_frame,
                            drawing: &mut shown.drawing,
                            dialog_key: None,
                            remembered: &shown.remembered,
                            fade: ui.visuals().disabled_alpha,
                            browser_collapsed: false,
                            browser_toggled: &mut shown.toggled,
                        }
                        .properties(ui);
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

    /// The one text `text` painted within `within`.
    #[track_caller]
    fn text_in<S>(h: &Harness<'_, S>, text: &str, within: Rect) -> TextShape {
        let mut found: Vec<TextShape> = painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Text(shown) if shown.galley.text() == text && within.contains(shown.pos) => {
                    Some(shown)
                }
                _ => None,
            })
            .collect();
        assert_eq!(found.len(), 1, "{text:?} painted {} times", found.len());
        found.remove(0)
    }

    /// A text's colour as painted: its override, else its section's.
    fn colour(shown: &TextShape, section: usize) -> Color32 {
        let format = shown.galley.job.sections[section].format.color;
        shown.override_text_color.unwrap_or(format)
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

    /// The untextured rectangles painted at `rect`, filled: their fill, corners, and stroke.
    fn fills_at<S>(h: &Harness<'_, S>, rect: Rect) -> Vec<(Color32, CornerRadius, Stroke)> {
        rects(h)
            .into_iter()
            .filter(|shape| {
                shape.brush.is_none()
                    && shape.fill != Color32::TRANSPARENT
                    && near_rect(shape.rect, rect)
            })
            .map(|shape| (shape.fill, shape.corner_radius, shape.stroke))
            .collect()
    }

    fn hlines<S>(h: &Harness<'_, S>, y: f32) -> Vec<([Pos2; 2], Stroke)> {
        painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::LineSegment { points, stroke }
                    if near(points[0].y, y) && near(points[1].y, y) =>
                {
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

    /// `words` drawn from `left`, centred on `middle`, `size` points of `family`, in `ink`
    /// (its first section's): its right edge.
    #[track_caller]
    fn said_at<S>(
        h: &Harness<'_, S>,
        words: &str,
        left: f32,
        middle: f32,
        font: FontId,
        ink: Color32,
    ) -> f32 {
        // Of the texts `words` painted (a value may be painted on more than one row), the
        // one on this line.
        let all: Vec<TextShape> = painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Text(shown) if shown.galley.text() == words => Some(shown),
                _ => None,
            })
            .collect();
        let mut found: Vec<&TextShape> = all
            .iter()
            .filter(|shown| (shown.pos.y + shown.galley.size().y / 2.0 - middle).abs() < 4.0)
            .collect();
        assert_eq!(
            found.len(),
            1,
            "{words:?} on the line at {middle}: {:?}",
            all.iter().map(|shown| shown.pos).collect::<Vec<_>>()
        );
        let shown = found.remove(0);
        let size = shown.galley.size();
        at(shown.pos, Pos2::new(left, middle - size.y / 2.0), words);
        assert_eq!(shown.galley.job.sections[0].format.font_id, font, "{words}");
        assert_eq!(colour(shown, 0), ink, "{words}'s ink");
        left + size.x
    }

    fn body(size: f32) -> FontId {
        FontId::new(size, FontFamily::Proportional)
    }

    fn mono() -> FontId {
        FontId::new(css::MONO, FontFamily::Monospace)
    }

    /// `.panel-title` and `.prop-head` from the pane's top: the head's icon in its box, its
    /// title and the line under it; the head's bottom.
    #[track_caller]
    fn title_and_head<S>(h: &Harness<'_, S>, title: &str, sub: &str, sub_font: FontId) -> f32 {
        let shown = text(h, "PROPERTIES");
        at(
            shown.pos,
            Pos2::new(
                PANE.x + css::PAD,
                PANE.y + css::TITLE_H / 2.0 - shown.galley.size().y / 2.0,
            ),
            "the title",
        );
        let format = &shown.galley.job.sections[0].format;
        assert_eq!(format.font_id, body(css::FS_SMALL));
        assert_eq!(format.extra_letter_spacing, css::CAPITALS_SPACING);
        assert_eq!(colour(&shown, 0), css::G1000);
        let head = PANE.y + css::TITLE_H;
        // Two 16 point lines, the 28 point box centred on them.
        let words = 2.0 * css::LH;
        let boxed = Rect::from_min_size(
            Pos2::new(
                PANE.x + css::PAD,
                head + css::HEAD_TOP + (words - css::PH_ICON) / 2.0,
            ),
            Vec2::splat(css::PH_ICON),
        );
        assert_eq!(
            fills_at(h, boxed),
            [(css::G200, CornerRadius::same(css::R_SM), Stroke::NONE)]
        );
        let icon = Rect::from_center_size(boxed.center(), Vec2::splat(css::ICON));
        assert_eq!(icon_at(h, icon), Some(css::G800), "the head's icon");
        let left = boxed.right() + css::HEAD_GAP;
        let top = head + css::HEAD_TOP;
        // The title, the head's (a feature's name is its Name field's too).
        let shown = text_in(
            h,
            title,
            Rect::from_x_y_ranges(left..=PANE.x + W, top..=top + css::LH),
        );
        at(
            shown.pos,
            Pos2::new(left, top + css::LH / 2.0 - shown.galley.size().y / 2.0),
            title,
        );
        assert_eq!(
            shown.galley.job.sections[0].format.font_id,
            body(css::PH_TITLE)
        );
        assert_eq!(colour(&shown, 0), css::G1000);
        said_at(h, sub, left, top + 1.5 * css::LH, sub_font, css::G550);
        top + words + css::HEAD_BOTTOM
    }

    /// `.section` at `top`: its words in capitals, the rule from 6 after them to the
    /// padding; its bottom.
    #[track_caller]
    fn section_at<S>(h: &Harness<'_, S>, words: &str, top: f32) -> f32 {
        let line = top + css::SECTION_TOP + css::LH / 2.0;
        let end = said_at(
            h,
            words,
            PANE.x + css::PAD,
            line,
            body(css::SECTION_FS),
            css::G550,
        );
        assert_eq!(
            text(h, words).galley.job.sections[0]
                .format
                .extra_letter_spacing,
            css::CAPITALS_SPACING
        );
        assert_eq!(
            hlines(h, line),
            [(
                [
                    Pos2::new(end + css::SECTION_GAP, line),
                    Pos2::new(PANE.x + W - css::PAD, line)
                ],
                Stroke::new(1.0, css::G200)
            )],
            "{words}'s rule"
        );
        top + css::SECTION_TOP + css::LH + css::SECTION_BOTTOM
    }

    /// `.prop` at `top`: `key` in its 96 point column in Gray.550, `value` 8 after it in
    /// `ink` and `font`; its bottom.
    #[track_caller]
    fn row_at<S>(
        h: &Harness<'_, S>,
        key: &str,
        value: &str,
        top: f32,
        font: FontId,
        ink: Color32,
    ) -> f32 {
        let middle = top + css::ROW_H / 2.0;
        said_at(h, key, PANE.x + css::PAD, middle, body(css::FS), css::G550);
        said_at(
            h,
            value,
            PANE.x + css::PAD + css::KEY_W + css::ROW_GAP,
            middle,
            font,
            ink,
        );
        top + css::ROW_H
    }

    /// `.hint-block` at `top` (its margin's top): its box, and `words` in it; its bottom.
    #[track_caller]
    fn hint_at<S>(h: &Harness<'_, S>, words: &str, top: f32) -> f32 {
        let shown = text(h, words);
        let height = shown.galley.size().y + 2.0 * css::HINT_PADDING_Y;
        let boxed = Rect::from_min_max(
            Pos2::new(PANE.x + css::PAD, top + css::HINT_MARGIN),
            Pos2::new(PANE.x + W - css::PAD, top + css::HINT_MARGIN + height),
        );
        assert_eq!(
            fills_at(h, boxed),
            [(css::G150, CornerRadius::same(css::R), Stroke::NONE)],
            "{words}'s box"
        );
        at(
            shown.pos,
            boxed.min + Vec2::new(css::HINT_PADDING_X, css::HINT_PADDING_Y),
            words,
        );
        assert_eq!(
            shown.galley.job.sections[0].format.font_id,
            body(css::FS_SMALL)
        );
        assert_eq!(colour(&shown, 0), css::G550);
        assert!(near(shown.galley.rows[0].height(), css::LH), "its line");
        boxed.bottom() + css::HINT_MARGIN
    }

    /// How many times `text` is painted.
    fn painted_times<S>(h: &Harness<'_, S>, text: &str) -> usize {
        painted(h)
            .into_iter()
            .filter(|shape| matches!(shape, Shape::Text(shown) if shown.galley.text() == text))
            .count()
    }

    /// [`part`]'s document view drawn in `h` checked down to its Bodies row, as
    /// [`the_documents_properties_are_drawn_as_doc_props`] checks it: the bottom of that row.
    #[track_caller]
    fn part_down_to_bodies<S>(h: &Harness<'_, S>) -> f32 {
        let mut y = title_and_head(h, "Part.silico", "/models/Part.silico", mono());
        y = section_at(h, "DOCUMENT", y);
        let value = body(css::FS);
        for (key, said) in [
            ("Revision", "1"),
            ("Units", "mm · 1 nm resolution"),
            ("Features", "2"),
            ("Saved", "Yes"),
        ] {
            y = row_at(h, key, said, y, value.clone(), css::G800);
        }
        y = row_at(
            h,
            "File on disk",
            "Not compared yet",
            y,
            value.clone(),
            css::G550,
        );
        y = section_at(h, "MODEL", y);
        row_at(h, "Bodies", "1", y, value, css::G800)
    }

    #[test]
    fn the_documents_properties_are_drawn_as_doc_props() {
        let h = drawn(part());
        let mut y = title_and_head(&h, "Part.silico", "/models/Part.silico", mono());
        y = section_at(&h, "DOCUMENT", y);
        let value = body(css::FS);
        y = row_at(&h, "Revision", "1", y, value.clone(), css::G800);
        y = row_at(
            &h,
            "Units",
            "mm · 1 nm resolution",
            y,
            value.clone(),
            css::G800,
        );
        y = row_at(&h, "Features", "2", y, value.clone(), css::G800);
        y = row_at(&h, "Saved", "Yes", y, value.clone(), css::G800);
        y = row_at(
            &h,
            "File on disk",
            "Not compared yet",
            y,
            value.clone(),
            css::G550,
        );
        y = section_at(&h, "MODEL", y);
        y = row_at(&h, "Bodies", "1", y, value.clone(), css::G800);
        // Nothing published, so nothing measured: each measure's row says so, subdued.
        for key in ["Volume", "Area", "Size", "Faces"] {
            y = row_at(&h, key, NOT_MEASURED, y, value.clone(), css::G550);
        }
        hint_at(&h, "Double-click a feature in the timeline to edit it.", y);
    }

    /// [`part`], its model published (the first seen), and measured as `answers` say.
    fn measured_part(answers: crate::model_measures::Answers) -> Tab {
        let mut tab = part();
        tab.published = Some(Revision::OPENED);
        tab.published_models = 1;
        tab.model_measures_mut()
            .answered_as((1, Revision::OPENED), answers);
        tab
    }

    /// The block's mass properties, as `mass_properties` gives them: `volume` and `area` in
    /// millionths.
    fn block_measured(volume: i128, area: i128) -> silico_core::command::measure::Bodies {
        use silico_core::document::{Length, Measure};
        silico_core::command::measure::Bodies {
            bodies: vec![silico_core::command::measure::MassProperties {
                feature: fid("b0000001"),
                volume: Measure::from_millionths(volume),
                area: Measure::from_millionths(area),
                center_of_mass: [Length::from_nm(0); 3],
            }],
        }
    }

    #[test]
    fn the_models_measures_are_drawn_as_doc_props_writes_them() {
        // `fmtF(g.volume, 2) + ' mm³'` and `fmtF(g.area, 2) + ' mm²'` (`toLocaleString`
        // en-US: grouped by threes, two decimals, half away from zero), and `g.faces.length`
        // as a plain integer. By hand: 1234567.891 mm³ → 1,234,567.89; 2800.005 mm² →
        // 2,800.01.
        let faces = silico_core::command::elements::Queried {
            entries: Vec::new(),
            matched: 1234,
            tied: None,
        };
        let h = drawn(measured_part(crate::model_measures::Answers {
            bodies: Some(Ok(block_measured(1_234_567_891_000, 2_800_005_000))),
            faces: Some(Ok(faces)),
        }));
        let value = body(css::FS);
        let mut y = part_down_to_bodies(&h);
        y = row_at(
            &h,
            "Volume",
            "1,234,567.89 mm³",
            y,
            value.clone(),
            css::G800,
        );
        y = row_at(&h, "Area", "2,800.01 mm²", y, value.clone(), css::G800);
        // The viewport's bodies aren't of this model: its box not read yet.
        y = row_at(&h, "Size", NOT_MEASURED, y, value.clone(), css::G550);
        y = row_at(&h, "Faces", "1234", y, value, css::G800);
        hint_at(&h, "Double-click a feature in the timeline to edit it.", y);
    }

    /// A view of one body, the block, boxed from (0, 0, 0) to (40, 20, 10), read of no model.
    fn viewing_the_block() -> crate::viewport::View {
        let body = crate::viewport::ViewBody {
            feature: fid("b0000001"),
            key: silico_core::recompute::Key::parse(&"01".repeat(32)).unwrap(),
            level: 1.0,
            deflection: 1.0,
            bounds: Some([[0.0; 3], [40.0, 20.0, 10.0]]),
            serves: [0.0, 1.0],
            held: false,
            mesh: Arc::new(silico_protocol::Mesh {
                positions: Vec::new(),
                normals: Vec::new(),
                triangles: Vec::new(),
                face_triangles: Vec::new(),
                valid: None,
                edge_points: Vec::new(),
                edge_point_counts: Vec::new(),
                vertex_points: Vec::new(),
            }),
            faces: Vec::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
        };
        crate::viewport::View::holding(std::collections::BTreeMap::from([(1, body)]))
    }

    #[test]
    fn the_size_is_the_views_box_only_once_the_view_is_of_the_model_measured() {
        let measured = || crate::model_measures::Answers {
            bodies: Some(Ok(block_measured(8_000_000_000, 2_800_000_000))),
            faces: None,
        };
        // The view's block, read of no model: its box isn't this model's.
        let mut tab = measured_part(measured());
        tab.view = viewing_the_block();
        let h = drawn(tab);
        assert_eq!(
            painted_times(&h, "40 × 20 × 10 mm"),
            0,
            "another model's box"
        );
        assert_eq!(painted_times(&h, NOT_MEASURED), 2, "Size and Faces waiting");
        // Read of another model published at the same revision: still not this one's.
        let mut tab = measured_part(measured());
        tab.view = viewing_the_block();
        tab.view.read_as((2, Revision::OPENED));
        let h = drawn(tab);
        assert_eq!(
            painted_times(&h, "40 × 20 × 10 mm"),
            0,
            "another model's box"
        );
        // Read of this model: its box.
        let mut tab = measured_part(measured());
        tab.view = viewing_the_block();
        tab.view.read_as((1, Revision::OPENED));
        let h = drawn(tab);
        let value = body(css::FS);
        let mut y = part_down_to_bodies(&h);
        y = row_at(&h, "Volume", "8,000.00 mm³", y, value.clone(), css::G800);
        y = row_at(&h, "Area", "2,800.00 mm²", y, value.clone(), css::G800);
        y = row_at(&h, "Size", "40 × 20 × 10 mm", y, value.clone(), css::G800);
        row_at(&h, "Faces", NOT_MEASURED, y, value, css::G550);
        // Of this model, but the measures name another body: not the same bodies.
        let mut answers = measured();
        if let Some(Ok(bodies)) = answers.bodies.as_mut() {
            bodies.bodies[0].feature = fid("c0000001");
        }
        let mut tab = measured_part(answers);
        tab.view = viewing_the_block();
        tab.view.read_as((1, Revision::OPENED));
        let h = drawn(tab);
        assert_eq!(
            painted_times(&h, "40 × 20 × 10 mm"),
            0,
            "another body's box"
        );
    }

    #[test]
    fn a_model_of_no_body_has_no_measures() {
        let mut tab = measured_part(crate::model_measures::Answers {
            bodies: Some(Ok(silico_core::command::measure::Bodies {
                bodies: Vec::new(),
            })),
            faces: None,
        });
        if let Some(timeline) = tab.timeline.as_mut() {
            timeline.bodies.clear();
        }
        let h = drawn(tab);
        let value = body(css::FS);
        let mut y = title_and_head(&h, "Part.silico", "/models/Part.silico", mono());
        y = section_at(&h, "DOCUMENT", y);
        for _ in 0..5 {
            y += css::ROW_H;
        }
        y = section_at(&h, "MODEL", y);
        y = row_at(&h, "Bodies", "0", y, value, css::G800);
        for key in ["Volume", "Area", "Size", "Faces"] {
            assert_eq!(painted_times(&h, key), 0, "{key}");
        }
        hint_at(&h, "Double-click a feature in the timeline to edit it.", y);
    }

    #[test]
    fn a_refused_model_measure_leaves_its_rows_out_and_says_why() {
        let refusal = Problems::from(
            silico_core::Error::new(
                silico_core::ErrorCode::KernelFailed,
                "b0000001's solid couldn't be described, so its elements aren't known",
            )
            .with_suggestion("check the document again"),
        );
        let h = drawn(measured_part(crate::model_measures::Answers {
            bodies: Some(Ok(block_measured(8_000_000_000, 2_800_000_000))),
            faces: Some(Err(refusal.clone())),
        }));
        assert_eq!(painted_times(&h, "Faces"), 0, "Faces left out");
        assert_eq!(painted_times(&h, "8,000.00 mm³"), 1);
        assert_eq!(
            painted_times(
                &h,
                "b0000001's solid couldn't be described, so its elements aren't known"
            ),
            1,
            "its refusal said"
        );
        // The volume's read refused too: Volume and Area out, one card, the first refusal
        // (the mass properties').
        let bodies_refusal = Problems::from(silico_core::Error::new(
            silico_core::ErrorCode::KernelFailed,
            "b0000001's solid couldn't be measured",
        ));
        let h = drawn(measured_part(crate::model_measures::Answers {
            bodies: Some(Err(bodies_refusal)),
            faces: Some(Err(refusal)),
        }));
        assert_eq!(
            painted_times(&h, "b0000001's solid couldn't be measured"),
            1,
            "the first refusal said"
        );
        for key in ["Volume", "Area", "Faces"] {
            assert_eq!(painted_times(&h, key), 0, "{key} left out");
        }
        assert_eq!(painted_times(&h, "Size"), 1);
        assert_eq!(
            painted_times(
                &h,
                "b0000001's solid couldn't be described, so its elements aren't known"
            ),
            0,
            "the second refusal not said"
        );
    }

    #[test]
    fn a_features_properties_are_drawn_as_feature_props_with_its_error_card_and_name_field() {
        let mut tab = part();
        tab.selection_mut().select_feature(fid("c0000001"));
        // The cut's text as a read gives it: its target the block, its tool one the timeline
        // doesn't list, named by its ID.
        let cut = r#"{"id": "c0000001", "type": "cut", "v": 1, "name": "Hole", "base": "b0000001", "tool": "t0000001"}"#;
        tab.parameters.hold(
            fid("c0000001"),
            silico_core::document::json::read(cut.as_bytes()).unwrap(),
        );
        let h = drawn(tab);
        let mut y = title_and_head(&h, "Hole", "Subtract · c0000001", body(css::FS_SMALL));
        // The line under the title: the type, then the ID in monospace.
        let sub = text(&h, "Subtract · c0000001");
        assert_eq!(sub.galley.job.sections[1].format.font_id, mono());
        assert_eq!(colour(&sub, 1), css::G550);
        // `.err-card`: its fill, border, and corners, `--pad` in, 8 below the head.
        let message = text(&h, "The cut removes nothing");
        let card_top = y + css::CARD_MARGIN;
        let inner = Pos2::new(PANE.x + css::PAD + css::BORDER, card_top + css::BORDER);
        let icon = Rect::from_min_size(
            inner + Vec2::new(css::EC_HEAD_SIDE, css::EC_HEAD_TOP),
            Vec2::splat(css::ICON),
        );
        assert_eq!(icon_at(&h, icon), Some(css::CARD_HEAD), "the card's icon");
        // `.ec-head .t`: white, at the top of the head's line (`align-items: flex-start`).
        at(
            message.pos,
            Pos2::new(icon.right() + css::EC_HEAD_GAP, icon.top()),
            "the message",
        );
        assert_eq!(message.galley.job.sections[0].format.font_id, body(css::FS));
        assert_eq!(colour(&message, 0), css::G1000);
        // `.ec-body`, 32 in: the arrow, and the suggestion 6 after it, white.
        let fix = icon.top() + message.galley.size().y + css::EC_HEAD_BOTTOM;
        let arrow = text(&h, "→");
        at(arrow.pos, Pos2::new(inner.x + css::EC_BODY_LEFT, fix), "→");
        assert_eq!(colour(&arrow, 0), css::CARD_ACCENT);
        let suggestion = text(&h, "Move the pin");
        at(
            suggestion.pos,
            Pos2::new(arrow.pos.x + arrow.galley.size().x + css::EC_FIX_GAP, fix),
            "the suggestion",
        );
        assert_eq!(colour(&suggestion, 0), css::G1000);
        let card_bottom = fix
            + suggestion.galley.size().y
            + css::EC_FIX_MARGIN
            + css::EC_BODY_BOTTOM
            + css::BORDER;
        let card = Rect::from_min_max(
            Pos2::new(PANE.x + css::PAD, card_top),
            Pos2::new(PANE.x + W - css::PAD, card_bottom),
        );
        assert_eq!(
            fills_at(&h, card),
            [(
                css::ALERT_RED_BG,
                CornerRadius::same(css::R),
                Stroke::new(css::BORDER, css::CARD_BORDER)
            )]
        );
        y = card_bottom + css::CARD_MARGIN;
        // The Name row: its field (`.field`) across the value's column, 22 high, centred.
        let field = Rect::from_min_max(
            Pos2::new(
                PANE.x + css::PAD + css::KEY_W + css::ROW_GAP,
                y + (css::ROW_H - css::FIELD_H) / 2.0,
            ),
            Pos2::new(PANE.x + W - css::PAD, y + (css::ROW_H + css::FIELD_H) / 2.0),
        );
        let fills = fills_at(&h, field);
        assert_eq!(fills.len(), 1, "the name's field");
        assert_eq!(
            (fills[0].0, fills[0].1),
            (css::G200, CornerRadius::same(css::R_SM))
        );
        let named = h.get_by_role_and_label(Role::TextInput, "Name").rect();
        assert!(near_rect(named, field), "{named:?}, not {field:?}");
        said_at(
            &h,
            "Name",
            PANE.x + css::PAD,
            y + css::ROW_H / 2.0,
            body(css::FS),
            css::G550,
        );
        // PARAMETERS after the Name row (`section('Parameters')`), its rows the feature's text
        // as read, each a `.prop` in the value's own type and Gray.800; then HEALTH.
        y = section_at(&h, "PARAMETERS", y + css::ROW_H);
        y = row_at(&h, "Target Body", "Block", y, body(css::FS), css::G800);
        // Its tool, which the timeline doesn't list, by its ID, in the monospace type.
        y = row_at(&h, "Tool Body", "t0000001", y, mono(), css::G800);
        y = section_at(&h, "HEALTH", y);
        y = row_at(&h, "State", "Error", y, body(css::FS), css::RED);
        y = row_at(&h, "ID", "c0000001", y, mono(), css::G800);
        // `featureProps`' buttons, 10 under the rows: Edit feature (`.btn-secondary`,
        // Gray.300) first, then Suppress (`.btn-ghost`, no fill), 6 after it.
        let edit = h.get_by_role_and_label(Role::Button, "Edit feature").rect();
        at(
            edit.min,
            Pos2::new(PANE.x + css::PAD, y + css::BUTTONS_PADDING),
            "Edit feature",
        );
        assert!(near(edit.height(), css::BTN_H));
        assert_eq!(
            fills_at(&h, edit),
            [(css::G300, CornerRadius::same(css::R_SM), Stroke::NONE)]
        );
        let suppress = h.get_by_role_and_label(Role::Button, "Suppress").rect();
        at(
            suppress.min,
            Pos2::new(edit.right() + css::BUTTONS_GAP, edit.top()),
            "Suppress",
        );
        assert!(near(suppress.height(), css::BTN_H));
        assert!(fills_at(&h, suppress).is_empty(), "a ghost has no fill");
        hint_at(
            &h,
            "Hovering a feature in the timeline lights up the faces it made.",
            edit.bottom() + css::BUTTONS_PADDING,
        );
    }

    #[test]
    fn a_dialogs_inputs_and_footer_are_drawn_as_render_dialog() {
        let dialog = std::cell::RefCell::new(Dialog::new(Kind::Box, None, 1));
        let mut h = Harness::builder()
            .with_size(Vec2::new(PANE.x + W + 40.0, PANE.y + HEIGHT + 40.0))
            .build_ui(move |ui| {
                let pane = Rect::from_min_size(PANE, Vec2::new(W, HEIGHT));
                ui.scope_builder(UiBuilder::new().max_rect(pane), |ui| {
                    let mut cards = properties::Cards::default();
                    dialog.borrow_mut().ui(
                        ui,
                        &mut cards,
                        None,
                        &|_| None,
                        &|_| false,
                        crate::dialog::Allowed::ALL,
                    );
                });
            });
        theme::apply(&h.ctx);
        h.run_steps(4);
        // Its title: the kind in capitals after its icon; INPUTS under it; its first row.
        assert!(h.query_by_label("BOX").is_some());
        let y = section_at(&h, "INPUTS", PANE.y + css::TITLE_H);
        let field = h.get_by_role_and_label(Role::TextInput, "X").rect();
        assert!(near(
            field.left(),
            PANE.x + css::PAD + css::KEY_W + css::ROW_GAP
        ));
        assert!(near(field.center().y, y + css::ROW_H / 2.0));
        assert!(near(field.height(), css::FIELD_H));
        // `.dlg-footer` at the bottom: its 1 px Gray.250 border, on the panel's Gray.100.
        let bottom = PANE.y + HEIGHT;
        let footer_top = bottom - (2.0 * css::FOOTER_PADDING + css::FOOTER_BTN_H + css::BORDER);
        assert_eq!(
            hlines(&h, footer_top + css::BORDER / 2.0),
            [(
                [
                    Pos2::new(PANE.x, footer_top + css::BORDER / 2.0),
                    Pos2::new(PANE.x + W, footer_top + css::BORDER / 2.0)
                ],
                Stroke::new(css::BORDER, css::G250)
            )]
        );
        let footer =
            Rect::from_min_max(Pos2::new(PANE.x, footer_top), Pos2::new(PANE.x + W, bottom));
        assert_eq!(
            fills_at(&h, footer),
            [(css::G100, CornerRadius::ZERO, Stroke::NONE)]
        );
        // Cancel (`.btn-secondary`) then OK (`.btn-primary`), sharing the width, 6 apart.
        let width = (W - 2.0 * css::PAD - css::FOOTER_GAP) / 2.0;
        let cancel = Rect::from_min_size(
            Pos2::new(
                PANE.x + css::PAD,
                footer_top + css::BORDER + css::FOOTER_PADDING,
            ),
            Vec2::new(width, css::FOOTER_BTN_H),
        );
        let ok = cancel.translate(Vec2::new(width + css::FOOTER_GAP, 0.0));
        for (label, rect, fill, ink) in [
            ("Cancel", cancel, css::G300, css::G750),
            ("OK", ok, css::G800, css::G250),
        ] {
            let shown = h.get_by_role_and_label(Role::Button, label).rect();
            assert!(near_rect(shown, rect), "{label}: {shown:?}, not {rect:?}");
            assert_eq!(
                fills_at(&h, rect),
                [(fill, CornerRadius::same(css::R_SM), Stroke::NONE)],
                "{label}'s fill"
            );
            assert_eq!(colour(&text(&h, label), 0), ink, "{label}'s ink");
            assert_eq!(
                text(&h, label).galley.job.sections[0].format.font_id,
                body(css::FS)
            );
        }
    }

    /// `dialog` drawn at the pane, in the app's theme; with `preview` its preview's line.
    fn dialog_drawn(
        dialog: Dialog,
        preview: Option<properties::Preview>,
    ) -> Harness<'static, (Dialog, Action)> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(PANE.x + W + 40.0, PANE.y + HEIGHT + 40.0))
            .build_ui_state(
                move |ui, (dialog, action): &mut (Dialog, Action)| {
                    let pane = Rect::from_min_size(PANE, Vec2::new(W, HEIGHT));
                    ui.scope_builder(UiBuilder::new().max_rect(pane), |ui| {
                        let mut cards = properties::Cards::default();
                        let done = dialog.ui(
                            ui,
                            &mut cards,
                            preview,
                            &|_| None,
                            &|_| false,
                            crate::dialog::Allowed::ALL,
                        );
                        if done != Action::None {
                            *action = done;
                        }
                    });
                },
                (dialog, Action::None),
            );
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    /// The words each of `kind`'s choices offers.
    fn choices(dialog: &Dialog) -> Vec<(&'static str, Vec<String>, usize)> {
        dialog
            .inputs
            .iter()
            .filter(|input| dialog.shown(input))
            .filter_map(|input| match &input.value {
                crate::dialog::Value::Choice { options, chosen } => Some((
                    input.label,
                    options.iter().map(|(_, shown)| shown.clone()).collect(),
                    *chosen,
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn each_dialogs_choices_draw_as_a_select_however_few_and_short_their_words() {
        // `renderDialog`'s Extent, `<button class="field select">` with its down icon, never
        // `.seg`: each kind's choices, and a revolve's about one of its sketch's lines,
        // offered as itself. A revolve's two forms and two axes among them, which side by
        // side would fit the value column at 295 (Inter Medium at 12: One side 50.6,
        // Symmetric 61.6, each well under half the column's 167).
        let mut about_a_line = Dialog::new(Kind::Revolve, None, 1);
        for input in &mut about_a_line.inputs {
            if let crate::dialog::Value::Choice { options, .. } = &mut input.value
                && input.label == "Axis"
            {
                options.push(("nc000000".to_owned(), "nc000000".to_owned()));
            }
        }
        let dialogs = Kind::ALL
            .into_iter()
            .map(|kind| Dialog::new(kind, None, 1))
            .chain([about_a_line]);
        let left = PANE.x + css::PAD + css::KEY_W + css::ROW_GAP;
        let mut selects = Vec::new();
        for dialog in dialogs {
            let kind = dialog.kind.title();
            let offered = choices(&dialog);
            let h = dialog_drawn(dialog, None);
            for (key, options, chosen) in offered {
                let what = format!("{kind}'s {key}");
                let node = h
                    .query_by_role_and_label(Role::ComboBox, key)
                    .unwrap_or_else(|| panic!("{what}: no drop-down"));
                // Across the value column, a field high, showing what's chosen.
                let select = node.rect();
                assert!(
                    near(select.left(), left) && near(select.right(), PANE.x + W - css::PAD),
                    "{what}: {select:?}"
                );
                assert!(near(select.height(), css::FIELD_H), "{what}: {select:?}");
                assert_eq!(
                    node.accesskit_node().value().as_deref(),
                    Some(options[chosen].as_str()),
                    "{what}"
                );
                text_in(&h, &options[chosen], select);
                // None of its words is a button of its own, and none but the chosen one's
                // is painted.
                for (place, option) in options.iter().enumerate() {
                    assert!(h.query_by_label(option).is_none(), "{what}'s {option}");
                    let painted = painted(&h)
                        .into_iter()
                        .filter(|shape| {
                            matches!(shape, Shape::Text(shown) if shown.galley.text() == option)
                        })
                        .count();
                    let want = usize::from(place == chosen);
                    assert_eq!(painted, want, "{what}'s {option} painted {painted} times");
                }
                selects.push(format!("{kind} {key}"));
            }
        }
        assert_eq!(
            selects,
            [
                "Pad Extent",
                "Pocket Extent",
                "Revolve Axis",
                "Revolve Extent",
                "Groove Axis",
                "Groove Extent",
                "Revolve Axis",
                "Revolve Extent",
            ],
            "the choices drawn as selects"
        );
    }

    #[test]
    fn a_select_is_drawn_as_field_select_and_its_menu_picks() {
        let h = dialog_drawn(Dialog::new(Kind::Pocket, None, 1), None);
        let select = h.get_by_role_and_label(Role::ComboBox, "Extent").rect();
        // `.field.select`: across the value column, a field's 22, Gray.200, corners 4.
        let left = PANE.x + css::PAD + css::KEY_W + css::ROW_GAP;
        assert!(near(select.left(), left) && near(select.right(), PANE.x + W - css::PAD));
        assert!(near(select.height(), css::FIELD_H));
        assert_eq!(
            fills_at(&h, select),
            [(css::G200, CornerRadius::same(css::R_SM), Stroke::NONE)]
        );
        // Its words padded 6 in (inside its 1 px border), Gray.800; its arrow 10 square,
        // as far in from the right, Gray.550.
        said_at(
            &h,
            "One side",
            select.left() + css::BORDER + css::SELECT_PADDING,
            select.center().y,
            body(css::FS),
            css::G800,
        );
        let arrow = Rect::from_min_size(
            Pos2::new(
                select.right() - css::BORDER - css::SELECT_PADDING - css::SELECT_ARROW,
                select.center().y - css::SELECT_ARROW / 2.0,
            ),
            Vec2::splat(css::SELECT_ARROW),
        );
        assert_eq!(icon_at(&h, arrow), Some(css::G550), "the arrow");
        // A click opens its menu; a choice in it is chosen.
        let mut h = h;
        h.get_by_role_and_label(Role::ComboBox, "Extent").click();
        h.run_steps(2);
        h.get_by_label("Two sides").click();
        h.run_steps(2);
        let extent = h
            .state()
            .0
            .inputs
            .iter()
            .find(|input| input.label == "Extent")
            .map(|input| input.value.clone());
        assert!(
            matches!(extent, Some(crate::dialog::Value::Choice { chosen: 2, .. })),
            "{extent:?}"
        );
        assert_eq!(
            h.get_by_role_and_label(Role::ComboBox, "Extent")
                .accesskit_node()
                .value()
                .as_deref(),
            Some("Two sides")
        );
    }

    #[test]
    fn a_fields_unit_is_drawn_inside_it_as_field_unit() {
        let h = dialog_drawn(Dialog::new(Kind::Box, None, 1), None);
        let field = h.get_by_role_and_label(Role::TextInput, "Length").rect();
        // The field across the value column, its unit inside it, 6 and its border from its
        // right, Gray.550.
        assert!(near(field.right(), PANE.x + W - css::PAD), "{field:?}");
        let units: Vec<TextShape> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Text(shown) if shown.galley.text() == "mm" && field.contains(shown.pos) => {
                    Some(shown)
                }
                _ => None,
            })
            .collect();
        assert_eq!(units.len(), 1, "Length's unit");
        let unit = &units[0];
        let size = unit.galley.size();
        at(
            unit.pos,
            Pos2::new(
                field.right() - css::BORDER - css::UNIT_PADDING - size.x,
                field.center().y - size.y / 2.0,
            ),
            "the unit",
        );
        assert_eq!(colour(unit, 0), css::G550);
        // The value's text ends clear of it: the field's text area stops 6 (the input's
        // padding) short of the unit.
        let value = painted(&h)
            .into_iter()
            .find_map(|shape| match shape {
                Shape::Text(shown) if shown.galley.text() == "10" && field.contains(shown.pos) => {
                    Some(shown)
                }
                _ => None,
            })
            .unwrap();
        assert!(near(
            value.pos.x,
            field.left() + css::BORDER + css::INPUT_PADDING
        ));
        assert!(
            value.galley.job.wrap.max_width.is_infinite()
                || value.pos.x + value.galley.job.wrap.max_width
                    <= unit.pos.x - css::INPUT_PADDING + 0.5
        );
    }

    #[test]
    fn the_preview_is_drawn_as_dlg_status_in_each_state() {
        let dialog = || Dialog::new(Kind::Box, None, 1);
        let line = |h: &Harness<'_, (Dialog, Action)>, words: &str, ink: Color32| {
            let shown = text(h, words);
            assert_eq!(colour(&shown, 0), ink, "{words}");
            assert_eq!(
                shown.galley.job.sections[0].format.font_id,
                body(css::FS_SMALL)
            );
            // `.dlg-status`: `--pad` in, its padding 8 at the left, after a 14 point mark
            // and the 6 gap; in its Gray.150 box with corners 4.
            let boxed = rects(h).into_iter().find(|shape| {
                shape.fill == css::G150
                    && near(shape.rect.left(), PANE.x + css::PAD)
                    && near(shape.rect.right(), PANE.x + W - css::PAD)
            });
            let boxed = boxed.unwrap_or_else(|| panic!("{words}'s box"));
            assert_eq!(boxed.corner_radius, CornerRadius::same(css::R_SM));
            boxed.rect
        };
        // 3e: errors, red; OK keeps the feature with them.
        let h = dialog_drawn(dialog(), Some(properties::Preview::Errors(2)));
        assert!(h.query_by_label("PREVIEW").is_some());
        let boxed = line(
            &h,
            "The preview has 2 errors. OK keeps the feature with them.",
            css::RED,
        );
        // Its words on 16 point lines, wrapped; the icon centred on them (`align-items:
        // center`); the box their height and its padding.
        let shown = text(
            &h,
            "The preview has 2 errors. OK keeps the feature with them.",
        );
        let words = Rect::from_min_size(shown.pos, shown.galley.size());
        assert!(near(shown.galley.rows[0].height(), css::LH));
        assert!(near(words.top(), boxed.top() + css::STATUS_PADDING_Y));
        assert!(near(words.bottom(), boxed.bottom() - css::STATUS_PADDING_Y));
        let icon = Rect::from_min_size(
            Pos2::new(
                boxed.left() + css::STATUS_PADDING_X,
                words.center().y - css::ICON / 2.0,
            ),
            Vec2::splat(css::ICON),
        );
        assert_eq!(icon_at(&h, icon), Some(css::RED), "the error icon");
        assert!(near(shown.pos.x, icon.right() + css::STATUS_GAP));
        // Running: Gray.750, and Esc stops it, Gray.550, at its right.
        let h = dialog_drawn(dialog(), Some(properties::Preview::Running(Some((1, 3)))));
        let boxed = line(&h, "Rebuilding the preview · 1 of 3", css::G750);
        let esc = text(&h, "Esc stops it");
        assert_eq!(colour(&esc, 0), css::G550);
        assert!(near(
            esc.pos.x + esc.galley.size().x,
            boxed.right() - css::STATUS_PADDING_X
        ));
        // Up to date, and stopped.
        let h = dialog_drawn(
            dialog(),
            Some(properties::Preview::UpToDate {
                revision: 7,
                ms: Some(84),
            }),
        );
        line(&h, "Preview up to date · revision 7 · 84 ms", css::G550);
        let h = dialog_drawn(
            dialog(),
            Some(properties::Preview::UpToDate {
                revision: 7,
                ms: None,
            }),
        );
        line(&h, "Preview up to date · revision 7", css::G550);
        let h = dialog_drawn(dialog(), Some(properties::Preview::Stopped));
        let stopped = painted(&h).into_iter().any(|shape| {
            matches!(shape, Shape::Text(shown)
                if shown.galley.text().starts_with("Preview stopped. Rebuild ")
                    && shown.galley.text().ends_with(" resumes it."))
        });
        assert!(stopped);
        // None: no PREVIEW.
        let h = dialog_drawn(dialog(), None);
        assert!(h.query_by_label("PREVIEW").is_none());
    }

    #[test]
    fn the_dialogs_close_button_is_the_titles_ab_btn_and_cancels() {
        let mut h = dialog_drawn(Dialog::new(Kind::Box, None, 1), None);
        let close = h
            .get_by_role_and_label(Role::Button, crate::dialog::CLOSE)
            .rect();
        // `.panel-title .ab-btn`: 20 square, its right 6 in, centred on the 24 row.
        let want = Rect::from_min_size(
            Pos2::new(
                PANE.x + W - css::TITLE_PADDING_RIGHT - css::TITLE_BTN,
                PANE.y + (css::TITLE_H - css::TITLE_BTN) / 2.0,
            ),
            Vec2::splat(css::TITLE_BTN),
        );
        assert!(near_rect(close, want), "{close:?}, not {want:?}");
        let cross = Rect::from_center_size(want.center(), Vec2::splat(css::ICON));
        assert_eq!(icon_at(&h, cross), Some(css::G750), "its ×");
        // Before it, the kind's icon at the padding and its title 6 after (`.panel-title {
        // gap: 6px }`), white capitals.
        let middle = PANE.y + css::TITLE_H / 2.0;
        let icon = Rect::from_min_size(
            Pos2::new(PANE.x + css::PAD, middle - css::ICON / 2.0),
            Vec2::splat(css::ICON),
        );
        assert_eq!(icon_at(&h, icon), Some(css::G1000), "the kind's icon");
        said_at(
            &h,
            "BOX",
            icon.right() + css::TITLE_GAP,
            middle,
            body(css::FS_SMALL),
            css::G1000,
        );
        assert_eq!(h.state().1, Action::None);
        h.get_by_role_and_label(Role::Button, crate::dialog::CLOSE)
            .click();
        h.run_steps(2);
        assert_eq!(h.state().1, Action::Cancel);
    }

    // ---- The error card (`errCard`) ----

    /// A broken reference's error, by hand: Hole's, naming two of the five edges at fault,
    /// its field, its cause, what to do, and two names in the broken ones' place.
    fn broken() -> silico_core::Error {
        silico_core::Error::new(
            silico_core::ErrorCode::ReferenceBroken,
            "An edge the fillet names is gone",
        )
        .with_feature("c0000001")
        .with_elements(["Edge3", "Edge5"])
        .with_elements_total(5)
        .with_parameter("edges")
        .with_cause("Edge3 and Edge5 aren't in its base")
        .with_suggestion("Pick the edges again")
        .with_candidates(["Edge2", "Edge4"])
    }

    /// [`broken`] as the command API writes it, typed by hand from the error contract
    /// (`tests/contract/error-full.json`'s fields, in its order), on one line as MCP sends it.
    const BROKEN_JSON: &str = r#"{"code": "reference_broken", "message": "An edge the fillet names is gone", "feature": "c0000001", "elements": ["Edge3", "Edge5"], "elements_total": 5, "parameter": "edges", "cause": "Edge3 and Edge5 aren't in its base", "suggestion": "Pick the edges again", "candidates": ["Edge2", "Edge4"], "ranking": [], "advice": null}"#;

    /// [`part`] with Hole selected, its card [`broken`]'s.
    fn broken_hole() -> Tab {
        hole_said(&broken())
    }

    /// [`part`] with Hole selected, its card `error`'s.
    fn hole_said(error: &silico_core::Error) -> Tab {
        let mut tab = part();
        tab.said.insert("c0000001".to_owned(), Said::of(error));
        tab.selection_mut().select_feature(fid("c0000001"));
        tab
    }

    /// [`broken`], its candidates ranked as the core ranks them (#417), by hand, as the
    /// core's sharing rule gives them: the broken name made from Face1 and Face4, Edge2 from
    /// Face1 alone and Edge4 from Face4 alone, each of its feature, by name.
    fn ranked() -> silico_core::Error {
        let names = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        let sharing = |shared: &str, unshared: &str| silico_core::Rank {
            shared: names(&[shared]),
            unshared: names(&[unshared]),
            same_feature: true,
            nearest: None,
        };
        broken().with_ranked([
            ("Edge2", sharing("Face1", "Face4")),
            ("Edge4", sharing("Face4", "Face1")),
        ])
    }

    /// [`ranked`]'s whys, as the card words them (by hand from `worded`'s doc).
    const EDGE2_WHY: &str = "Made from Face1, as the broken name is, but not from Face4";
    const EDGE4_WHY: &str = "Made from Face4, as the broken name is, but not from Face1";

    #[test]
    fn each_candidate_says_why_the_core_ranked_it_under_its_name() {
        // `.cand .why`: under each name, on the body's 16 px line, 10.5 in the accent, the
        // core's ranking in words, the first's after "Closest · " (the heading's "closest
        // first"); each row's description in the accessibility tree its why alone, its label
        // as before.
        let h = drawn(hole_said(&ranked()));
        assert!(h.query_by_label("CANDIDATES, CLOSEST FIRST").is_some());
        let first = format!("{}{EDGE2_WHY}", crate::properties::CLOSEST);
        let mut lines = Vec::new();
        for (name, words) in [("Edge2", first.as_str()), ("Edge4", EDGE4_WHY)] {
            let shown = text(&h, name);
            let why = text(&h, words);
            at(
                why.pos,
                Pos2::new(shown.pos.x, shown.pos.y + css::LH),
                words,
            );
            assert_eq!(
                why.galley.job.sections[0].format.font_id,
                body(css::CANDS_FS)
            );
            assert_eq!(colour(&why, 0), css::CARD_ACCENT);
            lines.push(why.galley.rows.len());
        }
        let described = |label: &str| h.get_by_label(label).accesskit_node().description();
        assert_eq!(described("Closest · Edge2").as_deref(), Some(EDGE2_WHY));
        assert_eq!(described("Edge4").as_deref(), Some(EDGE4_WHY));
        // Each row as high as its name and its why's lines: the buttons under them lower,
        // by the lines the whys add, than under [`broken`]'s, whose rows have no whys.
        let unranked = drawn(broken_hole());
        let edit = |h: &Harness<'static, Shown>| {
            h.get_by_role_and_label(Role::Button, "Edit Hole")
                .rect()
                .top()
        };
        let added = (lines[0] + lines[1]) as f32 * css::LH;
        assert!(
            near(edit(&h) - edit(&unranked), added),
            "{} lower, not {added}",
            edit(&h) - edit(&unranked)
        );
    }

    #[test]
    fn a_why_too_long_for_its_row_wraps_whole_and_the_row_grows() {
        // A pane 180 wide: the first's why wraps to three lines or more, inside its row's
        // padding, every character drawn (never cut, as the candidate's own name is); the
        // row as high as its name and every line of its why.
        let width = 180.0;
        let h = drawn_at(hole_said(&ranked()), width);
        let words = format!("{}{EDGE2_WHY}", crate::properties::CLOSEST);
        let why = text(&h, &words);
        let lines = why.galley.rows.len();
        assert!(lines >= 3, "{lines} lines");
        assert!(!why.galley.elided);
        let whole: String = words.chars().filter(|chr| !chr.is_whitespace()).collect();
        assert_eq!(crate::modal::tests::drawn_characters(&why), whole);
        // The row's inside: the pane less the panel's padding, the card's border and its
        // body's padding, and the row's own.
        let inside =
            PANE.x + width - css::PAD - css::BORDER - css::EC_HEAD_SIDE - css::CAND_PADDING_X;
        assert!(
            why.pos.x + why.galley.size().x <= inside + 0.5,
            "{} past {inside}",
            why.pos.x + why.galley.size().x
        );
        let shown = text(&h, "Edge2");
        let next = text(&h, "Edge4");
        let row_bottom = shown.pos.y + (1 + lines) as f32 * css::LH + css::CAND_PADDING_Y;
        assert!(
            near(
                next.pos.y,
                row_bottom + css::CAND_MARGIN + css::CAND_PADDING_Y
            ),
            "Edge4 at {}, not under Edge2's {lines} lines of why",
            next.pos.y
        );
    }

    #[test]
    fn candidates_the_core_gives_no_ranking_say_nothing_of_closeness() {
        // [`broken`]'s candidates follow no ranking (`with_candidates`): another rule, which
        // its suggestion states, so the heading says CANDIDATES alone, no row says it's
        // closest or why, and none is described.
        let h = drawn(broken_hole());
        assert!(h.query_by_label("CANDIDATES").is_some());
        assert!(h.query_by_label("CANDIDATES, CLOSEST FIRST").is_none());
        assert!(h.query_by_label("Closest · Edge2").is_none());
        for name in ["Edge2", "Edge4"] {
            assert_eq!(h.get_by_label(name).accesskit_node().description(), None);
        }
        assert!(
            painted(&h).into_iter().all(|shape| match shape {
                Shape::Text(shown) => !shown.galley.text().contains("Closest"),
                _ => true,
            }),
            "no Closest drawn"
        );
    }

    /// The text the frame just stepped copied, if it copied any.
    fn copied<S>(h: &Harness<'_, S>) -> Option<String> {
        h.output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
    }

    #[test]
    fn an_error_card_says_its_code_feature_elements_parameter_and_candidates() {
        let h = drawn(broken_hole());
        // `errCard`, its parts in its order: the title, the feature's name, and the code;
        // the message (`.ec-body p`); the cause; Feature (the name, then the ID), Elements
        // (two of five), Parameter; the suggestion; the candidates under their heading.
        let code = h.get_by_label("reference_broken").rect().top();
        let titles: Vec<f32> = h
            .query_all_by_label("Hole")
            .map(|node| node.rect().top())
            .filter(|top| near(*top, code))
            .collect();
        assert_eq!(titles.len(), 1, "the title on the code's line: {titles:?}");
        let lines = [
            "reference_broken",
            "An edge the fillet names is gone",
            "Edge3 and Edge5 aren't in its base",
            "Feature: Hole c0000001",
            "Elements: Edge3, Edge5 (the first 2 of 5)",
            "Parameter: edges",
            "Pick the edges again",
            "CANDIDATES",
            "Edge2",
            "Edge4",
        ];
        let tops: Vec<f32> = lines
            .iter()
            .map(|line| h.get_by_label(line).rect().top())
            .collect();
        // Each under the one before.
        for pair in tops.windows(2) {
            assert!(pair[0] < pair[1], "{lines:?} out of order: {tops:?}");
        }
        // `featureProps` gives `noShow`: Edit, on (a cut's dialog is Subtract's), and Copy
        // as JSON.
        let edit = h.get_by_role_and_label(Role::Button, "Edit Hole");
        assert!(!edit.accesskit_node().is_disabled());
        assert!(
            h.query_by_role_and_label(Role::Button, "Show in timeline")
                .is_none()
        );
        let copy = h.get_by_role_and_label(Role::Button, "Copy as JSON");
        assert!(!copy.accesskit_node().is_disabled());
        // Every element named: no count.
        let mut tab = part();
        let all = silico_core::Error::new(silico_core::ErrorCode::KernelFailed, "Fillet failed")
            .with_feature("c0000001")
            .with_elements(["Edge3"]);
        tab.said.insert("c0000001".to_owned(), Said::of(&all));
        tab.selection_mut().select_feature(fid("c0000001"));
        let h = drawn(tab);
        assert!(h.query_by_label("Elements: Edge3").is_some());
        assert!(h.query_by_label("kernel_failed").is_some());
        assert!(h.query_by_label_contains("Parameter").is_none());
        assert!(h.query_by_label_contains("CANDIDATES").is_none());
    }

    #[test]
    fn copy_as_json_puts_the_cores_error_on_the_clipboard_as_an_agent_gets_it() {
        let mut h = drawn(broken_hole());
        h.get_by_role_and_label(Role::Button, "Copy as JSON")
            .click();
        h.step();
        assert_eq!(copied(&h).as_deref(), Some(BROKEN_JSON));
        // A warning's, as an error's, its code a warning's.
        let warning = silico_core::Warning::new(
            silico_core::WarningCode::FingerprintMissing,
            "Hole stores no fingerprint",
        )
        .with_feature("c0000001");
        let said = Said::of_warning(&warning);
        assert_eq!(
            said.json().as_deref(),
            Some(
                r#"{"code": "fingerprint_missing", "message": "Hole stores no fingerprint", "feature": "c0000001", "elements": [], "elements_total": 0, "parameter": null, "cause": null, "suggestion": null, "candidates": [], "ranking": [], "advice": null}"#
            )
        );
        // A refusal said in words alone has none to copy.
        let words = Said {
            message: "refused".to_owned(),
            ..Said::default()
        };
        assert_eq!(words.json(), None);
    }

    #[test]
    fn an_error_cards_edit_opens_its_features_dialog_as_edit_feature_does() {
        let mut tab = part();
        let failed =
            silico_core::Error::new(silico_core::ErrorCode::KernelFailed, "The block failed")
                .with_feature("b0000001");
        tab.said.insert("b0000001".to_owned(), Said::of(&failed));
        tab.selection_mut().select_feature(fid("b0000001"));
        let mut h = drawn(tab);
        assert!(!h.state().tab.as_ref().unwrap().in_dialog());
        let edit = h.get_by_role_and_label(Role::Button, "Edit Block");
        assert!(!edit.accesskit_node().is_disabled());
        edit.click();
        h.step();
        // Edit Feature's read of the box, under way: the dialog opens once it answers.
        let shown = h.state();
        assert!(shown.tab.as_ref().unwrap().in_dialog());
        assert!(shown.next_frame);
    }

    #[test]
    fn a_document_problems_show_in_timeline_selects_its_feature_for_the_timeline_to_scroll_to() {
        // `docProps` gives every card Edit and Show in timeline: the gate's problem naming
        // Hole, with nothing selected, the timeline counting it as the gate's.
        let mut tab = part();
        let gate = silico_core::Error::new(
            silico_core::ErrorCode::InvalidInput,
            "Hole's tool names no feature",
        )
        .with_feature("c0000001")
        .with_parameter("tool");
        tab.document_problems.push(Said::of(&gate));
        if let Some(timeline) = tab.timeline.as_mut() {
            timeline.problems = 1;
        }
        let mut h = drawn(tab);
        assert!(
            h.query_by_label("Revision").is_some(),
            "the document's view"
        );
        // Off: nothing of a document the gate refused changes but its text, though the
        // cut's dialog is Subtract's.
        assert!(
            h.get_by_role_and_label(Role::Button, "Edit Hole")
                .accesskit_node()
                .is_disabled()
        );
        h.get_by_role_and_label(Role::Button, "Show in timeline")
            .click();
        h.step();
        let tab = h.state().tab.as_ref().unwrap();
        assert_eq!(tab.selection().feature(), Some(&fid("c0000001")));
        assert_eq!(tab.timeline_scroll(), Some(&fid("c0000001")));
        assert!(!h.state().next_frame, "a selection sends nothing");
    }

    /// The pane's width in the app's 1280 × 800 window (its fixture's screenshots): the
    /// tiles' shares, not yet `--right-w`.
    const APP_W: f32 = 248.0;

    #[test]
    fn an_error_cards_message_wraps_across_the_body_and_its_title_beside_the_code() {
        // The core's message, long and with long words, under a title on the code's line.
        let message = "edges.1, \"k7f2qx3m.box.Edge5\", names no element of m3tq8v0z's \
                       solid: it isn't one of the elements that solid is made from";
        for width in [W, APP_W] {
            let mut tab = part();
            let error = silico_core::Error::new(silico_core::ErrorCode::ReferenceBroken, message)
                .with_feature("c0000001");
            tab.said.insert("c0000001".to_owned(), Said::of(&error));
            tab.selection_mut().select_feature(fid("c0000001"));
            let h = drawn_at(tab, width);
            // The card's inside: the pane less `--pad` and the border at each side.
            let inner_left = PANE.x + css::PAD + css::BORDER;
            let inner_right = PANE.x + width - css::PAD - css::BORDER;
            // `.ec-body p`: wrapped at the body's whole width, `padding: 0 10px 0 32px`.
            let body = inner_right - inner_left - css::EC_BODY_LEFT - css::EC_HEAD_SIDE;
            let said = text(&h, message);
            assert!(
                (said.galley.job.wrap.max_width - body).abs() <= 1.0,
                "at {width}: the message wraps at {}, not {body}",
                said.galley.job.wrap.max_width
            );
            assert!(said.galley.rows.len() > 1, "at {width}: it wraps");
            assert!(said.galley.size().x <= body + 0.5, "at {width}");
            assert!(
                near(said.pos.x, inner_left + css::EC_BODY_LEFT),
                "at {width}"
            );
            // `.ec-code`: its right edge at the head's 10 px right padding, on the title's
            // line; the title, `flex: 1`, wraps at what the icon, both gaps, and the chip
            // leave.
            let code = text(&h, "reference_broken");
            let chip_width = code.galley.size().x + 2.0 * css::EC_CODE_PADDING;
            let line = code.pos.y + code.galley.size().y / 2.0 - css::LH / 2.0;
            let title = text_in(
                &h,
                "Hole",
                Rect::from_min_max(
                    Pos2::new(inner_left, line - 0.5),
                    Pos2::new(inner_right, line + 0.5),
                ),
            );
            let chip = Rect::from_min_max(
                Pos2::new(inner_right - css::EC_HEAD_SIDE - chip_width, line),
                Pos2::new(inner_right - css::EC_HEAD_SIDE, line + css::LH),
            );
            assert_eq!(
                fills_at(&h, chip),
                [(
                    css::EC_CODE_BG,
                    CornerRadius::same(css::EC_CODE_RADIUS),
                    Stroke::NONE
                )],
                "at {width}: the chip"
            );
            let row = inner_right
                - inner_left
                - 2.0 * css::EC_HEAD_SIDE
                - css::ICON
                - 2.0 * css::EC_HEAD_GAP
                - chip_width;
            assert!(
                (title.galley.job.wrap.max_width - row).abs() <= 1.0,
                "at {width}: the title wraps at {}, not {row}",
                title.galley.job.wrap.max_width
            );
            assert!(
                near(
                    title.pos.x,
                    inner_left + css::EC_HEAD_SIDE + css::ICON + css::EC_HEAD_GAP
                ),
                "at {width}"
            );
        }
    }

    #[test]
    fn an_error_card_is_drawn_as_err_card_with_its_code_rows_candidates_and_buttons() {
        let h = drawn(broken_hole());
        let y = title_and_head(&h, "Hole", "Subtract · c0000001", body(css::FS_SMALL));
        let card_top = y + css::CARD_MARGIN;
        let inner = Pos2::new(PANE.x + css::PAD + css::BORDER, card_top + css::BORDER);
        let inner_right = PANE.x + W - css::PAD - css::BORDER;
        // `.ec-head`: the title, the feature's name, from the icon's right, white, and the
        // code chip flush with the head's right padding, on the head's first line.
        let head_top = inner.y + css::EC_HEAD_TOP;
        let title = text_in(
            &h,
            "Hole",
            Rect::from_min_max(
                Pos2::new(inner.x, head_top),
                Pos2::new(inner_right, head_top + css::LH),
            ),
        );
        at(
            title.pos,
            Pos2::new(
                inner.x + css::EC_HEAD_SIDE + css::ICON + css::EC_HEAD_GAP,
                head_top,
            ),
            "the title",
        );
        assert_eq!(title.galley.job.sections[0].format.font_id, body(css::FS));
        assert_eq!(colour(&title, 0), css::G1000);
        let code = text(&h, "reference_broken");
        assert_eq!(
            code.galley.job.sections[0].format.font_id,
            FontId::new(css::EC_CODE_FS, FontFamily::Monospace)
        );
        assert_eq!(colour(&code, 0), css::CARD_ACCENT);
        let chip = Rect::from_min_max(
            Pos2::new(
                inner_right - css::EC_HEAD_SIDE - code.galley.size().x - 2.0 * css::EC_CODE_PADDING,
                head_top,
            ),
            Pos2::new(inner_right - css::EC_HEAD_SIDE, head_top + css::LH),
        );
        assert_eq!(
            fills_at(&h, chip),
            [(
                css::EC_CODE_BG,
                CornerRadius::same(css::EC_CODE_RADIUS),
                Stroke::NONE
            )]
        );
        at(
            code.pos,
            Pos2::new(
                chip.left() + css::EC_CODE_PADDING,
                chip.center().y - code.galley.size().y / 2.0,
            ),
            "the code",
        );
        // The title wraps short of the chip, `gap: 8px` before it.
        assert!(title.pos.x + title.galley.size().x <= chip.left() - css::EC_HEAD_GAP + 0.5);
        // `.ec-body p`: the message, 32 in, in the body's colour, then the cause 6 under it,
        // 6 above the rows.
        let body_left = inner.x + css::EC_BODY_LEFT;
        let body_top = head_top + title.galley.size().y + css::EC_HEAD_BOTTOM;
        let message = text(&h, "An edge the fillet names is gone");
        at(message.pos, Pos2::new(body_left, body_top), "the message");
        assert_eq!(message.galley.job.sections[0].format.font_id, body(css::FS));
        assert_eq!(colour(&message, 0), css::EC_BODY_TEXT);
        let cause_top = body_top + message.galley.size().y + css::EC_P_MARGIN;
        let cause = text(&h, "Edge3 and Edge5 aren't in its base");
        at(cause.pos, Pos2::new(body_left, cause_top), "the cause");
        assert_eq!(cause.galley.job.sections[0].format.font_id, body(css::FS));
        assert_eq!(colour(&cause, 0), css::EC_BODY_TEXT);
        // `.ec-kv`: each key in the accent at `--fs-small`, the values 8 after the widest.
        let rows_top = cause_top + cause.galley.size().y + css::EC_P_MARGIN;
        let keys: Vec<TextShape> = ["Feature", "Elements", "Parameter"]
            .iter()
            .map(|key| text(&h, key))
            .collect();
        let column = keys
            .iter()
            .map(|key| key.galley.size().x)
            .fold(0.0, f32::max);
        let values = body_left + column + css::EC_KV_COLUMN_GAP;
        for key in &keys {
            assert_eq!(
                key.galley.job.sections[0].format.font_id,
                body(css::FS_SMALL)
            );
            assert_eq!(colour(key, 0), css::CARD_ACCENT, "{}", key.galley.text());
            assert!(near(key.pos.x, body_left), "{}", key.galley.text());
        }
        at(keys[0].pos, Pos2::new(body_left, rows_top), "Feature");
        // Feature: the name in the body's colour, then the ID in `.mono` at 70%.
        let feature = text(&h, "Hole c0000001");
        at(feature.pos, Pos2::new(values, rows_top), "the feature");
        assert_eq!(
            feature.galley.job.sections[0].format.font_id,
            body(css::FS_SMALL)
        );
        assert_eq!(colour(&feature, 0), css::EC_BODY_TEXT);
        assert_eq!(feature.galley.job.sections[1].format.font_id, mono());
        assert_eq!(colour(&feature, 1).a(), css::EC_ID_ALPHA);
        // Elements, 2 under Feature: each a chip on the row's line.
        let elements_top = rows_top + css::LH + css::EC_KV_ROW_GAP;
        at(keys[1].pos, Pos2::new(body_left, elements_top), "Elements");
        let edge3 = text(&h, "Edge3");
        assert_eq!(
            edge3.galley.job.sections[0].format.font_id,
            FontId::new(css::EL_CHIP_FS, FontFamily::Monospace)
        );
        assert_eq!(colour(&edge3, 0), css::G1000);
        let chip_height = edge3.galley.size().y;
        let chip = Rect::from_min_size(
            Pos2::new(values, elements_top + (css::LH - chip_height) / 2.0),
            Vec2::new(
                edge3.galley.size().x + 2.0 * css::EL_CHIP_PADDING,
                chip_height,
            ),
        );
        assert_eq!(
            fills_at(&h, chip),
            [(
                css::EL_CHIP_BG,
                CornerRadius::same(css::EL_CHIP_RADIUS),
                Stroke::NONE
            )]
        );
        at(
            edge3.pos,
            Pos2::new(chip.left() + css::EL_CHIP_PADDING, chip.top()),
            "Edge3",
        );
        let edge5 = text(&h, "Edge5");
        assert!(near(edge5.pos.y, edge3.pos.y) && edge5.pos.x > chip.right());
        // Parameter, 2 under the Elements' last line.
        let more = text(&h, "the first 2 of 5");
        let elements_bottom = more.pos.y.max(elements_top) + css::LH;
        let parameter_top = elements_bottom + css::EC_KV_ROW_GAP;
        at(
            keys[2].pos,
            Pos2::new(body_left, parameter_top),
            "Parameter",
        );
        let parameter = text(&h, "edges");
        at(parameter.pos, Pos2::new(values, parameter_top), "edges");
        // `.ec-fix`, 6 under the rows; the candidates' heading 8 under it.
        let fix = parameter_top + css::LH + css::EC_KV_MARGIN;
        let arrow = text(&h, "→");
        at(arrow.pos, Pos2::new(body_left, fix), "→");
        // Unranked candidates (`with_candidates`): CANDIDATES alone.
        let heading = text(&h, "CANDIDATES");
        let heading_top = fix + css::LH + css::EC_FIX_MARGIN;
        at(
            heading.pos,
            Pos2::new(body_left, heading_top),
            "the heading",
        );
        assert_eq!(
            heading.galley.job.sections[0].format.font_id,
            body(css::CANDS_FS)
        );
        assert_eq!(
            heading.galley.job.sections[0].format.extra_letter_spacing,
            css::CAPITALS_SPACING
        );
        assert_eq!(colour(&heading, 0), css::CARD_ACCENT);
        // `.cand`s: across the body, its line and its padding high, 4 apart, each its name
        // alone (unranked, no why: a ranked row's why is placed by
        // `each_candidate_says_why_the_core_ranked_it_under_its_name`).
        let body_right = inner_right - css::EC_HEAD_SIDE;
        let mut top = heading_top + css::LH + css::CANDS_MARGIN;
        for (name, lines) in [("Edge2", 1.0), ("Edge4", 1.0)] {
            let row = Rect::from_min_max(
                Pos2::new(body_left, top),
                Pos2::new(
                    body_right,
                    top + 2.0 * css::CAND_PADDING_Y + lines * css::LH,
                ),
            );
            assert_eq!(
                fills_at(&h, row),
                [(css::CAND_BG, CornerRadius::same(css::R_SM), Stroke::NONE)],
                "{name}'s row"
            );
            let shown = text(&h, name);
            at(
                shown.pos,
                Pos2::new(body_left + css::CAND_PADDING_X, top + css::CAND_PADDING_Y),
                name,
            );
            assert_eq!(shown.galley.job.sections[0].format.font_id, mono());
            assert_eq!(colour(&shown, 0), css::CAND_TEXT);
            top = row.bottom() + css::CAND_MARGIN;
        }
        // `.ec-actions`: Edit Hole (`.btn-secondary`, Gray.300, on, its words Gray.750),
        // then Copy as JSON 6 after it (`.btn-ghost`: no fill, Gray.750), each 22 high.
        let edit = h.get_by_role_and_label(Role::Button, "Edit Hole").rect();
        assert!(
            near(edit.left(), body_left) && near(edit.top(), top),
            "{edit:?}"
        );
        assert!(near(edit.height(), css::BTN_H));
        assert_eq!(
            fills_at(&h, edit),
            [(css::G300, CornerRadius::same(css::R_SM), Stroke::NONE)]
        );
        let copy = h.get_by_role_and_label(Role::Button, "Copy as JSON").rect();
        assert!(
            near(copy.left(), edit.right() + css::EC_ACTIONS_GAP) && near(copy.top(), top),
            "{copy:?}"
        );
        assert!(near(copy.height(), css::BTN_H));
        assert!(fills_at(&h, copy).is_empty(), "a ghost has no fill");
        assert_eq!(colour(&text(&h, "Copy as JSON"), 0), css::G750);
        // The card's frame, to 8 under the buttons (`.ec-body`'s padding) and its border.
        let card = Rect::from_min_max(
            Pos2::new(PANE.x + css::PAD, card_top),
            Pos2::new(
                PANE.x + W - css::PAD,
                top + css::BTN_H + css::EC_BODY_BOTTOM + css::BORDER,
            ),
        );
        assert_eq!(
            fills_at(&h, card),
            [(
                css::ALERT_RED_BG,
                CornerRadius::same(css::R),
                Stroke::new(css::BORDER, css::CARD_BORDER)
            )]
        );
    }

    // ---- The selection view (`selectionProps`) ----

    /// The fake's 2 by 1 by 1 block, `aaaaaaaa`, named Block and the timeline's one body,
    /// its model published (the selection's tests' `box_model`), `names` selected in it in
    /// order, and the measures of that selection answered `answers`, as if read.
    fn selecting(names: &[&str], answers: Answers) -> Tab {
        let mut tab = part();
        tab.timeline = Some(TimelineAnswer {
            revision: Revision::OPENED,
            model: Some(Revision::OPENED),
            problems: 0,
            features: Some(vec![entry("aaaaaaaa", "box", "Block", Health::Healthy)]),
            bodies: vec![fid("aaaaaaaa")],
        });
        tab.browser = crate::tab::Browser::new(
            vec![crate::tab::Row {
                id: fid("aaaaaaaa"),
                name: "Block".to_owned(),
                health: Health::Healthy,
                marked_by: None,
                consumers: Default::default(),
                suppressed: false,
                last_healthy: None,
            }],
            tab.browser.sketches().to_vec(),
        );
        let model = crate::selection::tests::box_model();
        tab.selection_mut()
            .published(Some((model, Revision::OPENED)), 1);
        for name in names {
            tab.selection_mut().pick(
                &format!("aaaaaaaa.box.{name}"),
                crate::selection::How::Toggle,
            );
        }
        assert_eq!(tab.selection().elements().count(), names.len());
        let of = crate::measures::Of::now(tab.selection()).unwrap();
        let measured = crate::measures::Measured::of(tab.selection());
        tab.measures_mut().answered_as(of, measured, answers);
        tab
    }

    /// The block's element `name`'s entry, as `elements` lists it from the fake's
    /// description (every face of type other, every edge too).
    fn element(name: &str) -> ElementEntry {
        let model = crate::selection::tests::box_model();
        silico_core::command::elements::entries(&model, &fid("aaaaaaaa"))
            .unwrap()
            .iter()
            .find(|entry| entry.name.to_string() == format!("aaaaaaaa.box.{name}"))
            .unwrap()
            .clone()
    }

    #[test]
    fn one_planar_face_is_drawn_as_selection_props_its_measures_name_sketch_on_it_edit_and_hint() {
        use silico_core::command::elements::ElementType;
        // Face5, by hand the block's bottom: 2 by 1, so 2 mm², facing −z.
        let mut face = element("Face5");
        face.r#type = ElementType::Plane;
        face.area = silico_core::document::Measure::rounded(2.0);
        face.normal = Some([0.0, 0.0, -1.0]);
        let answers = Answers {
            element: Some(Ok(face)),
            ..Answers::default()
        };
        let mut h = drawn(selecting(&["Face5"], answers));
        h.ctx.set_os(egui::os::OperatingSystem::Nix);
        h.run_steps(2);
        let mut y = title_and_head(&h, "Face5", "Planar face · Block", body(css::FS_SMALL));
        y = section_at(&h, "MEASURES", y);
        y = row_at(&h, "Area", "2.00 mm²", y, body(css::FS), css::G800);
        y = row_at(&h, "Normal", "(0, 0, -1)", y, body(css::FS), css::G800);
        y = section_at(&h, "NAME", y);
        // `.prop .v .copy`: the copy icon at the value's right (`margin-left: auto`),
        // Gray.550, centred on the row; the name in `.mono`, the value's Gray.800.
        let copy = Rect::from_min_size(
            Pos2::new(
                PANE.x + W - css::PAD - css::ICON,
                y + (css::ROW_H - css::ICON) / 2.0,
            ),
            Vec2::splat(css::ICON),
        );
        y = row_at(
            &h,
            "Stable name",
            "aaaaaaaa.box.Face5",
            y,
            mono(),
            css::G800,
        );
        assert_eq!(icon_at(&h, copy), Some(css::G550), "the copy icon");
        assert!(near_rect(
            h.get_by_role_and_label(Role::Button, "Copy").rect(),
            copy
        ));
        // Made by: a link (`.link`, `--info`); Stable, `.ok-text`.
        y = row_at(&h, "Made by", "Block", y, body(css::FS), css::INFO);
        assert!(h.query_by_role_and_label(Role::Link, "Block").is_some());
        y = row_at(&h, "Stability", "Stable", y, body(css::FS), css::GREEN);
        // A planar face's buttons, 10 under the rows (app.js's `selectionProps`): Sketch on
        // this face (`.btn-secondary`, Gray.300) first, then Edit Block (`.btn-ghost`, no
        // fill), 6 after it.
        let sketch = h
            .get_by_role_and_label(Role::Button, "Sketch on this face")
            .rect();
        at(
            sketch.min,
            Pos2::new(PANE.x + css::PAD, y + css::BUTTONS_PADDING),
            "Sketch on this face",
        );
        assert!(near(sketch.height(), css::BTN_H));
        assert_eq!(
            fills_at(&h, sketch),
            [(css::G300, CornerRadius::same(css::R_SM), Stroke::NONE)]
        );
        let edit = h.get_by_role_and_label(Role::Button, "Edit Block").rect();
        at(
            edit.min,
            Pos2::new(sketch.right() + css::BUTTONS_GAP, sketch.top()),
            "Edit Block",
        );
        assert!(near(edit.height(), css::BTN_H));
        assert!(fills_at(&h, edit).is_empty(), "a ghost has no fill");
        assert!(
            h.query_by_role_and_label(Role::Button, "Fillet").is_none(),
            "a face has no Fillet"
        );
        // The hint, 10 under them, Select Other emphasised; the modifier the platform's.
        let words = "Ctrl-click adds or removes. Right-click › Select Other reaches what's behind.";
        hint_at(&h, words, edit.bottom() + css::BUTTONS_PADDING);
        assert_eq!(colour(&text(&h, words), 1), css::G800);
        h.ctx.set_os(egui::os::OperatingSystem::Mac);
        h.run_steps(2);
        assert!(
            h.query_by_label(
                "⌘-click adds or removes. Right-click › Select Other reaches what's behind."
            )
            .is_some()
        );
        // The copy button copies the name as it's written.
        h.get_by_role_and_label(Role::Button, "Copy").click();
        h.step();
        assert_eq!(copied(&h).as_deref(), Some("aaaaaaaa.box.Face5"));
        // Sketch on this face on while sketch mode can open, and off while it can't
        // (`Tab::can_sketch`): here, the document has a problem the gate found.
        let sketch_off = |h: &Harness<'static, Shown>| {
            h.get_by_role_and_label(Role::Button, "Sketch on this face")
                .accesskit_node()
                .is_disabled()
        };
        assert!(!sketch_off(&h));
        let tab = h.state_mut().tab.as_mut().unwrap();
        tab.timeline.as_mut().unwrap().problems = 1;
        assert!(!tab.can_sketch());
        h.run_steps(2);
        assert!(sketch_off(&h));
    }

    #[test]
    fn a_face_not_measured_yet_offers_no_sketch_on_it_till_elements_says_its_a_plane() {
        // Whether a face is a plane is `elements`' to say: unanswered, its Area says so, and
        // there's no Sketch on this face, Edit Block alone.
        let h = drawn(selecting(&["Face5"], Answers::default()));
        assert!(h.query_by_label("Not measured yet").is_some());
        assert!(
            h.query_by_role_and_label(Role::Button, "Sketch on this face")
                .is_none()
        );
        assert!(
            h.query_by_role_and_label(Role::Button, "Edit Block")
                .is_some()
        );
    }

    #[test]
    fn an_edge_not_measured_yet_says_so_and_offers_fillet_before_edit() {
        // No answer yet: its kind's row, Length, says so in `.subdued`, never a dash.
        let h = drawn(selecting(&["Edge3"], Answers::default()));
        let mut y = title_and_head(&h, "Edge3", "Edge · Block", body(css::FS_SMALL));
        y = section_at(&h, "MEASURES", y);
        y = row_at(
            &h,
            "Length",
            "Not measured yet",
            y,
            body(css::FS),
            css::G550,
        );
        y = section_at(&h, "NAME", y);
        y = row_at(
            &h,
            "Stable name",
            "aaaaaaaa.box.Edge3",
            y,
            mono(),
            css::G800,
        );
        y = row_at(&h, "Made by", "Block", y, body(css::FS), css::INFO);
        y = row_at(&h, "Stability", "Stable", y, body(css::FS), css::GREEN);
        // Fillet (`.btn-secondary`, Gray.300) first, then Edit Block, 6 after it.
        let fillet = h.get_by_role_and_label(Role::Button, "Fillet").rect();
        at(
            fillet.min,
            Pos2::new(PANE.x + css::PAD, y + css::BUTTONS_PADDING),
            "Fillet",
        );
        assert_eq!(
            fills_at(&h, fillet),
            [(css::G300, CornerRadius::same(css::R_SM), Stroke::NONE)]
        );
        let edit = h.get_by_role_and_label(Role::Button, "Edit Block").rect();
        at(
            edit.min,
            Pos2::new(fillet.right() + css::BUTTONS_GAP, fillet.top()),
            "Edit Block",
        );
    }

    #[test]
    fn two_faces_are_drawn_as_selection_props_with_measure_over_selection_and_copy_names() {
        use silico_core::command::measure::{AngleAnswer, DistanceAnswer};
        // Face5 and Face6, by hand the block's bottom and top: 1 apart each way, facing apart.
        let mm = Length::from_nm(1_000_000);
        let answers = Answers {
            element: None,
            distance: Some(Ok(DistanceAnswer {
                distance: mm,
                from: [Length::from_nm(0); 3],
                to: [Length::from_nm(0), Length::from_nm(0), mm],
                center_distance: None,
                normal_distance: Some(mm),
                center_to_element: None,
            })),
            angle: Some(Ok(AngleAnswer {
                angle: silico_core::document::Angle::rounded(0.0).unwrap(),
                vertex: None,
            })),
        };
        let mut h = drawn(selecting(&["Face5", "Face6"], answers));
        let mut y = title_and_head(&h, "2 selected", "2 faces", body(css::FS_SMALL));
        y = section_at(&h, "MEASURE", y);
        y = row_at(&h, "Distance", "1.00 mm", y, body(css::FS), css::G800);
        y = row_at(
            &h,
            "Normal distance",
            "1.00 mm",
            y,
            body(css::FS),
            css::G800,
        );
        y = row_at(&h, "Angle", "0° (parallel)", y, body(css::FS), css::G800);
        y = section_at(&h, "SELECTION", y);
        // Each by its number, its name in `.mono.subdued`.
        y = row_at(&h, "Face5", "aaaaaaaa.box.Face5", y, mono(), css::G550);
        y = row_at(&h, "Face6", "aaaaaaaa.box.Face6", y, mono(), css::G550);
        let copy = h.get_by_role_and_label(Role::Button, "Copy names").rect();
        at(
            copy.min,
            Pos2::new(PANE.x + css::PAD, y + css::BUTTONS_PADDING),
            "Copy names",
        );
        assert!(fills_at(&h, copy).is_empty(), "a ghost has no fill");
        assert!(h.query_by_label("MEASURES").is_none());
        h.get_by_role_and_label(Role::Button, "Copy names").click();
        h.step();
        assert_eq!(
            copied(&h).as_deref(),
            Some(r#"["aaaaaaaa.box.Face5", "aaaaaaaa.box.Face6"]"#)
        );
    }

    #[test]
    fn an_elements_words_are_its_types_and_an_angle_is_parallel_at_exactly_0_or_180() {
        use silico_core::command::elements::ElementType;
        use silico_core::document::Angle;
        let face = Some(ElementKind::Face);
        let edge = Some(ElementKind::Edge);
        let mut entry = element("Edge3");
        let at = |x: i64| [Length::from_nm(x), Length::from_nm(0), Length::from_nm(0)];
        // A circle's edge whose ends are one point is a circle; another, an arc.
        entry.r#type = ElementType::Circle;
        entry.start = Some(at(1));
        entry.end = Some(at(1));
        assert_eq!(element_words(edge, Some(&entry)), "Circle edge");
        entry.end = Some(at(2));
        assert_eq!(element_words(edge, Some(&entry)), "Arc edge");
        for (r#type, words) in [
            (ElementType::Line, "Line edge"),
            (ElementType::Bspline, "B-spline edge"),
            (ElementType::Other, "Edge"),
        ] {
            entry.r#type = r#type;
            assert_eq!(element_words(edge, Some(&entry)), words);
        }
        for (r#type, words) in [
            (ElementType::Plane, "Planar face"),
            (ElementType::Cylinder, "Cylindrical face"),
            (ElementType::Cone, "Conical face"),
            (ElementType::Other, "Face"),
        ] {
            entry.r#type = r#type;
            assert_eq!(element_words(face, Some(&entry)), words);
        }
        // Not read yet: its kind alone.
        assert_eq!(element_words(face, None), "Face");
        assert_eq!(element_words(edge, None), "Edge");
        assert_eq!(element_words(Some(ElementKind::Vertex), None), "Vertex");
        assert_eq!(element_words(None, None), "Element");
        // Parallel at exactly 0 or 180 at the nanodegree, as the core reads it; else fmtF's.
        for (degrees, words) in [
            (0.0, "0° (parallel)"),
            (180.0, "180° (parallel)"),
            (90.0, "90.00°"),
            (1e-9, "0.00°"),
            (179.999_999_999, "180.00°"),
            (30.004_999_999, "30.00°"),
            (30.005, "30.01°"),
        ] {
            assert_eq!(
                angle_words(Angle::rounded(degrees).unwrap()),
                words,
                "{degrees}"
            );
        }
    }

    #[test]
    fn every_element_type_has_its_words_for_each_kind_of_element_it_can_be() {
        use silico_core::command::elements::ElementType as T;
        // By hand: the prototype's words where it has them (app.js's `selectionProps`:
        // "Planar face", "Cylindrical face", "Line edge", "Arc edge", "Vertex"); for each
        // other type the core gives (`ElementType`, its geometry as OCCT names it), a face its
        // adjective before "face", an edge its curve's noun before "edge", as those are; the
        // kind alone for a type of no name (`Other`). The words past the prototype's are #414's
        // approved ones, pinned.
        let (face, edge, vertex) = (ElementKind::Face, ElementKind::Edge, ElementKind::Vertex);
        let table = [
            (face, T::Plane, "Planar face"),
            (face, T::Cylinder, "Cylindrical face"),
            (face, T::Cone, "Conical face"),
            (face, T::Sphere, "Spherical face"),
            (face, T::Torus, "Toroidal face"),
            (face, T::Bezier, "Bézier face"),
            (face, T::Bspline, "B-spline face"),
            (face, T::Revolution, "Revolved face"),
            (face, T::Extrusion, "Extruded face"),
            (face, T::Offset, "Offset face"),
            (face, T::Other, "Face"),
            (edge, T::Line, "Line edge"),
            // Its ends apart: an arc (its ends one point, a circle: below).
            (edge, T::Circle, "Arc edge"),
            (edge, T::Ellipse, "Ellipse edge"),
            (edge, T::Hyperbola, "Hyperbola edge"),
            (edge, T::Parabola, "Parabola edge"),
            (edge, T::Bezier, "Bézier edge"),
            (edge, T::Bspline, "B-spline edge"),
            (edge, T::Offset, "Offset edge"),
            (edge, T::Other, "Edge"),
            (vertex, T::Point, "Vertex"),
        ];
        // Every type with each kind of element it can be has its row, and only those.
        let pairs: Vec<(ElementKind, T)> = table.iter().map(|&(kind, t, _)| (kind, t)).collect();
        for t in T::ALL {
            for kind in [face, edge, vertex] {
                assert_eq!(
                    pairs.contains(&(kind, t)),
                    t.is_of(kind),
                    "a {kind:?} of type {t:?}"
                );
            }
        }
        let mut entry = element("Edge3");
        let at = |x: i64| [Length::from_nm(x), Length::from_nm(0), Length::from_nm(0)];
        entry.start = Some(at(1));
        entry.end = Some(at(2));
        for (kind, t, words) in table {
            entry.r#type = t;
            assert_eq!(
                element_words(Some(kind), Some(&entry)),
                words,
                "a {kind:?} of type {t:?}"
            );
        }
        entry.r#type = T::Circle;
        entry.end = Some(at(1));
        assert_eq!(element_words(Some(edge), Some(&entry)), "Circle edge");
    }

    #[test]
    fn a_vertex_not_measured_yet_says_its_position_isnt_and_offers_edit_alone() {
        // `selectionProps`' vertex (app.js): "Vertex" on its body, and its one measure,
        // Position, which says it isn't measured till `elements` answers, in `.subdued`,
        // never a dash; no Sketch on this face, no Fillet: Edit Block alone, 10 under the rows.
        let h = drawn(selecting(&["Vertex1"], Answers::default()));
        let mut y = title_and_head(&h, "Vertex1", "Vertex · Block", body(css::FS_SMALL));
        y = section_at(&h, "MEASURES", y);
        y = row_at(&h, "Position", NOT_MEASURED, y, body(css::FS), css::G550);
        y = section_at(&h, "NAME", y);
        y = row_at(
            &h,
            "Stable name",
            "aaaaaaaa.box.Vertex1",
            y,
            mono(),
            css::G800,
        );
        y = row_at(&h, "Made by", "Block", y, body(css::FS), css::INFO);
        y = row_at(&h, "Stability", "Stable", y, body(css::FS), css::GREEN);
        let edit = h.get_by_role_and_label(Role::Button, "Edit Block").rect();
        at(
            edit.min,
            Pos2::new(PANE.x + css::PAD, y + css::BUTTONS_PADDING),
            "Edit Block",
        );
        for button in [SKETCH_ON_FACE, "Fillet"] {
            assert!(
                h.query_by_role_and_label(Role::Button, button).is_none(),
                "a vertex has no {button}"
            );
        }
    }

    #[test]
    fn a_pair_not_measured_yet_says_so_of_distance_and_angle_and_with_a_vertex_of_distance_alone() {
        // Two faces, unanswered: Distance and Angle, each not measured yet in `.subdued`,
        // then SELECTION.
        let h = drawn(selecting(&["Face5", "Face6"], Answers::default()));
        let mut y = title_and_head(&h, "2 selected", "2 faces", body(css::FS_SMALL));
        y = section_at(&h, "MEASURE", y);
        y = row_at(&h, "Distance", NOT_MEASURED, y, body(css::FS), css::G550);
        y = row_at(&h, "Angle", NOT_MEASURED, y, body(css::FS), css::G550);
        section_at(&h, "SELECTION", y);
        // A face and a vertex, unanswered: no angle takes a vertex (`angle`'s design), so
        // Distance alone, then SELECTION.
        let h = drawn(selecting(&["Face5", "Vertex1"], Answers::default()));
        let mut y = title_and_head(&h, "2 selected", "1 face, 1 vertex", body(css::FS_SMALL));
        y = section_at(&h, "MEASURE", y);
        y = row_at(&h, "Distance", NOT_MEASURED, y, body(css::FS), css::G550);
        section_at(&h, "SELECTION", y);
        assert!(h.query_by_label("Angle").is_none());
    }

    #[test]
    fn an_elements_body_is_named_by_its_own_row_among_several_bodies() {
        // Two bodies, the element's listed second, named Body1 in the browser (its feature
        // Block in the timeline): its head names its own body's row, Body1, never the
        // other's, Plate, nor its feature's name.
        let mut tab = selecting(&["Face5"], Answers::default());
        let mut bodies = tab.browser.bodies().to_vec();
        bodies[0].name = "Body1".to_owned();
        bodies.insert(
            0,
            crate::tab::Row {
                id: fid("b0000002"),
                name: "Plate".to_owned(),
                health: Health::Healthy,
                marked_by: None,
                consumers: Default::default(),
                suppressed: false,
                last_healthy: None,
            },
        );
        tab.browser = crate::tab::Browser::new(bodies, tab.browser.sketches().to_vec());
        let h = drawn(tab);
        title_and_head(&h, "Face5", "Face · Body1", body(css::FS_SMALL));
        assert!(h.query_by_label_contains("Plate").is_none());
    }

    #[test]
    fn a_refused_measure_is_the_cores_card_in_place_of_its_rows() {
        // A distance the kernel couldn't find, in the core's words (`distance`'s design):
        // its card, with its code, and no Distance row; the angle, answered, still shown.
        let refused = |code, message: &str| Problems::from(silico_core::Error::new(code, message));
        let distance = "the kernel couldn't find the least distance between these elements";
        let answers = Answers {
            element: None,
            distance: Some(Err(refused(silico_core::ErrorCode::KernelFailed, distance))),
            angle: Some(Ok(silico_core::command::measure::AngleAnswer {
                angle: silico_core::document::Angle::rounded(90.0).unwrap(),
                vertex: None,
            })),
        };
        let h = drawn(selecting(&["Face5", "Face6"], answers));
        assert!(h.query_by_label(distance).is_some());
        assert!(h.query_by_label("kernel_failed").is_some());
        assert!(h.query_by_label("Distance").is_none());
        assert!(h.query_by_label(NOT_MEASURED).is_none());
        assert!(h.query_by_label("90.00°").is_some());
        // An element's listing refused: its card in place of MEASURES' rows.
        let listing = "the document has no feature aaaaaaaa";
        let answers = Answers {
            element: Some(Err(refused(silico_core::ErrorCode::InvalidInput, listing))),
            ..Answers::default()
        };
        let h = drawn(selecting(&["Face5"], answers));
        assert!(h.query_by_label(listing).is_some());
        assert!(h.query_by_label("invalid_input").is_some());
        assert!(h.query_by_label("Area").is_none());
        assert!(h.query_by_label(NOT_MEASURED).is_none());
    }

    #[test]
    fn each_selection_view_closes_with_clear_selection_in_its_title_and_a_feature_has_none() {
        for names in [&["Face5"][..], &["Face5", "Face6"][..]] {
            let mut h = drawn(selecting(names, Answers::default()));
            // `.panel-title .ab-btn`, at the title's right, past its 6 of padding.
            let close = h
                .get_by_role_and_label(Role::Button, "Clear selection")
                .rect();
            let right = PANE.x + W - css::TITLE_PADDING_RIGHT;
            at(
                close.min,
                Pos2::new(
                    right - css::TITLE_BTN,
                    PANE.y + (css::TITLE_H - css::TITLE_BTN) / 2.0,
                ),
                "Clear selection",
            );
            h.get_by_role_and_label(Role::Button, "Clear selection")
                .click();
            h.run_steps(2);
            let tab = h.state().tab.as_ref().unwrap();
            assert!(tab.selection().is_empty(), "{names:?}");
            // The document's properties now, which have no close button.
            assert!(
                h.query_by_role_and_label(Role::Button, "Clear selection")
                    .is_none()
            );
        }
        let mut tab = part();
        tab.selection_mut().select_feature(fid("b0000001"));
        let h = drawn(tab);
        assert!(
            h.query_by_role_and_label(Role::Button, "Clear selection")
                .is_none()
        );
    }

    #[test]
    fn a_features_id_row_copies_its_id() {
        let mut tab = part();
        tab.selection_mut().select_feature(fid("b0000001"));
        let mut h = drawn(tab);
        h.get_by_role_and_label(Role::Button, "Copy").click();
        h.step();
        assert_eq!(copied(&h).as_deref(), Some("b0000001"));
    }

    #[test]
    fn the_panels_suppress_asks_the_next_frame_to_poll_what_it_sent() {
        // The menu's Suppress asks the next frame, to poll the `suppress_feature` it sent
        // (the browser's test above); the panel's, the same item through the same `act`,
        // asks it too, from a frame that asked nothing before it.
        let mut tab = part();
        tab.selection_mut().select_feature(fid("b0000001"));
        let mut h = drawn(tab);
        assert!(!h.state().next_frame, "nothing sent yet");
        h.get_by_role_and_label(Role::Button, "Suppress").click();
        h.run_steps(2);
        assert!(h.state().next_frame, "the suppression sent, never polled");
    }

    #[test]
    fn the_sketch_cards_make_it_driven_asks_the_next_frame_to_poll_what_it_sent() {
        // Sketch mode over defined by the right side's second length (ka000002), read at a
        // published revision: the card's Make it driven, from a frame that asked nothing,
        // asks the next one, to poll the `set_field` it queued.
        use crate::sketch::tests::{length, read, rectangle};
        use silico_core::sketch_states::{ConstraintState, MemberState, SketchState};
        let mut tab = part();
        let mut mode = crate::sketch::SketchMode::new(1, Some(fid("s0000001")));
        mode.read = Some(crate::sketch::Read {
            model: Some(Revision::OPENED),
            result: Ok(read(
                rectangle(vec![
                    length("ka000001", "na000002", "20", true),
                    length("ka000002", "na000002", "20", true),
                ]),
                SketchState::OverDefined,
                MemberState::OverDefined,
                &[("ka000002", ConstraintState::Redundant)],
            )),
        });
        tab.sketch = Some(mode);
        let mut h = drawn(tab);
        assert!(!h.state().next_frame, "nothing sent yet");
        h.get_by_role_and_label(Role::Button, "Make it driven")
            .click();
        h.run_steps(2);
        assert!(h.state().next_frame, "the driven edit queued, never polled");
    }

    /// `part()` with a sketch, Profile, listed before its features, selected, its read not
    /// held (as a suppressed sketch's, never read), after a sketch's PARAMETERS last showed
    /// four rows.
    fn unread_sketch() -> Tab {
        let mut tab = part();
        if let Some(features) = tab
            .timeline
            .as_mut()
            .and_then(|timeline| timeline.features.as_mut())
        {
            features.insert(0, entry("s0000001", "sketch", "Profile", Health::Healthy));
        }
        tab.parameters.showed("sketch", 4);
        tab.selection_mut().select_feature(fid("s0000001"));
        tab
    }

    #[test]
    fn a_sketch_never_read_has_no_parameters_however_many_rows_a_sketch_last_showed() {
        // A sketch's rows are from its read, never from a text read off the UI thread: with
        // none held there's nothing to wait for, so no PARAMETERS, not an empty one kept the
        // height of the last sketch's four rows. HEALTH straight under the Name row.
        let h = drawn(unread_sketch());
        assert!(h.query_by_label("PARAMETERS").is_none(), "an empty header");
        // The Name row's top, its field centred in it (`.field`, 22 high in a 24 row).
        let name = h.get_by_role_and_label(Role::TextInput, "Name").rect();
        let row = name.top() - (css::ROW_H - css::FIELD_H) / 2.0;
        section_at(&h, "HEALTH", row + css::ROW_H);
    }

    #[test]
    fn a_feature_read_while_its_kind_last_showed_no_rows_keeps_no_room() {
        // The cut's text not read yet, and a cut last shown with no rows: no room to keep,
        // so no PARAMETERS header over nothing.
        let mut tab = part();
        tab.parameters.showed("cut", 0);
        tab.selection_mut().select_feature(fid("c0000001"));
        let h = drawn(tab);
        assert!(h.query_by_label("PARAMETERS").is_none(), "an empty header");
        assert!(h.query_by_label("HEALTH").is_some());
    }
}
