//! The window's panes, docked with `egui_tiles` as Fusion lays them out (docs/design/m5.0-
//! app.md, section 8; docs/ux/fusion.md, *The window*): the browser at the canvas's left,
//! the canvas (the viewport, `crate::viewport`), and the property panel at its right; the
//! timeline across the bottom is a panel of its own, a fixed 40 high (`crate::timeline`).
//! Each pane is labelled for the accessibility tree (`egui_tiles` labels none), which is
//! how the app's tests find it (decision 7).

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Sense, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType};
use egui_tiles::{Tile, TileId, Tiles, Tree, UiResponse};
use silico_core::command::{SessionId, Sessions};
use silico_core::disk::Disk;
use silico_core::document::FeatureId;
use silico_core::document::Units;
use silico_core::kernel::Clients;
use silico_core::timeline::{Entry, Health};

use crate::dialog::Action;
use crate::icons::{self, Icon, IconSize};
use crate::properties;
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
/// timeline, a panel of its own, is under them: `crate::timeline`), sized for each frame's
/// width by [`fit`].
pub fn layout() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let browser = tiles.insert_pane(Pane::Browser);
    let viewport = tiles.insert_pane(Pane::Viewport);
    let properties = tiles.insert_pane(Pane::Properties);
    let root = tiles.insert_horizontal_tile(vec![browser, viewport, properties]);
    Tree::new("silico-window", root, tiles)
}

/// The side panes' widths, as the prototype's `#main` lays them out (styles.css: `grid-
/// template-columns: var(--browser-w) 1fr var(--right-w)`, `--browser-w: 248px`, `--right-
/// w: 296px`), each with its 1px border inside it (`#browser`'s `border-right`, `#right`'s
/// `border-left`, `box-sizing: border-box`): the border is the tiles' gap, 1 wide
/// (`Behavior::gap_width`), so each pane's own rect is the column less that 1.
pub(crate) const BROWSER_W: f32 = 248.0;
pub(crate) const RIGHT_W: f32 = 296.0;

/// A side pane's border (`#browser`'s `border-right`, `#right`'s `border-left`): the tiles'
/// gap ([`Panes`]' `gap_width`), taken from its column's width in [`fit`].
const BORDER: f32 = 1.0;

/// The tree's panes sized for a width of `width`: the browser `--browser-w` wide and the
/// property panel `--right-w`, each with its border, and the canvas (`1fr`) the rest. A
/// window narrower than the two side panes does as the prototype's grid does: the canvas
/// shrinks to nothing (`#canvas-wrap`'s `min-width: 0`), and the panes keep their widths,
/// the property panel running past the window's right edge, where it's cut off.
///
/// `egui_tiles` shares its width out in proportion to each child's share, so each share
/// is the pane's width itself: shares that sum to the width shared give each its own.
/// The panes aren't resizable ([`Panes`]' `is_container_resizable`), as the prototype's
/// aren't.
// SEAM: the browser collapsed to a 28px strip (`#main.browser-collapsed`, interact.js's
// `toggleBrowser`; design section 8), once its strip and chevron are built.
pub fn fit(tree: &mut Tree<Pane>, width: f32) {
    let sides = BROWSER_W + RIGHT_W;
    let width = width.max(sides);
    tree.set_width(width);
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
        linear.shares.set_share(browser, BROWSER_W - BORDER);
        linear.shares.set_share(viewport, width - sides);
        linear.shares.set_share(properties, RIGHT_W - BORDER);
    }
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
    /// What the viewport's renderer holds, and how its frames reach it.
    pub drawing: &'a mut crate::viewport::Drawing,
    /// Enter or Esc pressed this frame, if it's an open dialog's (`App::keys`).
    pub dialog_key: Option<egui::Key>,
}

impl<P: Clients + 'static> egui_tiles::Behavior<Pane> for Panes<'_, P> {
    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: TileId, pane: &mut Pane) -> UiResponse {
        let label = pane.label();
        let rect = ui.max_rect();
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::Pane);
            node.set_label(label);
            node.set_bounds(bounds(rect));
        });
        if *pane == Pane::Viewport {
            // The canvas reaches the pane's edges.
            crate::viewport::show(ui, self.tab.as_deref_mut(), self.sessions, self.drawing);
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
        BORDER
    }

    fn is_container_resizable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        // The prototype's columns are fixed (`fit`): no splitter between them.
        false
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: false,
            ..Default::default()
        }
    }
}

/// A pane's place, `rect`, as the accessibility tree gives it (in points, as egui's
/// widgets give theirs).
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
    /// The browser (section 8, *The browser*; DESIGN.md, *egui mapping*): the document's
    /// row, and under it Origin, Bodies, and Sketches, by type as Fusion groups them, each
    /// under a header with its caret; each row with its eye (the tab's visibility, never
    /// saved), its icon, its name, its meta at its right, and its mark, from the rows the
    /// tab worked out as its timeline was read ([`crate::tab::browser`]).
    fn browser(&mut self, ui: &mut Ui) {
        // `.panel-body` straight under `.panel-title`, as each row under the last.
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        properties::title(ui, None, "Browser", None);
        let Some(tab) = self.tab.as_deref_mut() else {
            padded(ui, |ui| {
                ui.weak("No document open");
            });
            return;
        };
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
            let group = if tab.browser.sketches.iter().any(|row| row.id == shown) {
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
            *self.started |= tab.finish_rename(self.sessions, feature, &typed);
        }
        for what in asked.toggled {
            tab.toggle_shown(what);
        }
        if let Some(id) = asked.selected
            && tab.selection().feature() != Some(&id)
        {
            // A sketch chosen here fills an open dialog's selection box (section 9).
            tab.pick(&id);
            tab.selected_in = crate::feature_menu::Place::Browser;
            tab.selection_mut().select_feature(id);
        }
        if let Some(id) = asked.menu_selected
            && tab.selection().feature() != Some(&id)
        {
            // Selected as its menu opens, as the timeline's right-click selects: never an
            // open dialog's pick, which a click alone makes.
            tab.selected_in = crate::feature_menu::Place::Browser;
            tab.selection_mut().select_feature(id);
        }
        if let Some(act) = asked.act {
            *self.started |= crate::feature_menu::act(tab, self.sessions, act);
        }
        if let Some(id) = asked.edit {
            // SEAM(#351): `tab.edit_sketch_or_feature(self.sessions, &id)`, Edit Sketch for a
            // sketch; until then a sketch has no dialog, and this opens none.
            tab.edit_feature(self.sessions, &id);
            *self.started = true;
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

    /// The property panel (DESIGN.md, *Layout*: one right panel, four jobs): the feature
    /// dialog, if one is open; else the selection's faces and edges, if it has any
    /// (`selectionProps`, [`selected_elements`]); else the selected feature's properties;
    /// else the document's, never an empty state.
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
        properties::title(ui, None, "Properties", None);
        let entry: Option<Entry> = tab.selection().feature().and_then(|selected| {
            tab.timeline
                .as_ref()
                .and_then(|timeline| timeline.features.as_ref())
                .and_then(|features| features.iter().find(|entry| &entry.id == selected))
                .cloned()
        });
        let sessions = self.sessions;
        let name = &mut *self.name;
        let started = &mut *self.started;
        let elements = tab.selection().elements().next().is_some();
        let asked = egui::ScrollArea::vertical()
            .id_salt("properties")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                match entry {
                    _ if elements => {
                        selected_elements(ui, tab);
                        None
                    }
                    Some(entry) => feature(ui, tab, &entry, sessions, name, started),
                    None => document(ui, tab),
                }
            })
            .inner;
        if let Some(asked) = asked {
            *self.started |= tab.card_asked(sessions, asked);
        }
    }

    /// The open dialog, docked in the property panel (open item 27): drawn, what's changed
    /// in it sent, and its OK or Cancel taken, or the key pressed: Enter is OK, if OK is on;
    /// Esc goes to the first of these there is (section 3), a dialog's preview rebuilding,
    /// which it stops, the dialog left open, or the dialog, which it cancels
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
        let said: Vec<(crate::tab::Said, Option<String>)> = tab
            .dialog_said()
            .into_iter()
            .map(|said| {
                let name = said
                    .feature()
                    .and_then(|id| tab.entry(id))
                    .map(|entry| entry.name.clone());
                (said, name)
            })
            .collect();
        let preview = preview_of(tab);
        let Some(mut dialog) = tab.dialog.take() else {
            return;
        };
        let enabled = !tab.closing();
        let title = dialog.kind.title();
        let rows = &tab.browser.sketches;
        let named = |id: &FeatureId| {
            rows.iter()
                .find(|row| &row.id == id)
                .map(|row| row.name.clone())
        };
        let refused = |field: &str| refused.contains(&field);
        let mut action = ui
            .scope_builder(UiBuilder::new().id_salt(("dialog", dialog.serial)), |ui| {
                ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                    node.set_role(egui::accesskit::Role::Dialog);
                    node.set_label(title);
                });
                dialog.ui(ui, &said, preview, &named, &refused, enabled)
            })
            .inner;
        tab.dialog = Some(dialog);
        *self.started |= tab.dialog_changed(sessions);
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
        *self.started |= action != Action::None;
    }
}

/// What `tab`'s open dialog's preview line says (`renderDialog`'s PREVIEW, state 3e): none
/// before its feature is there (a new one's add unanswered); else its rebuild running (Esc
/// stops it), stopped, or done, with the errors the timeline counts, if any.
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
            0 => properties::Preview::UpToDate(rebuild.revision.number()),
            errors => properties::Preview::Errors(errors),
        }
    })
}

/// The selected feature's properties (`featureProps`): its head (its type's icon, its name,
/// its type and ID), the document's problems and its own (its error card, with Edit, as
/// `featureProps` offers it, `noShow`), its name, editable, which is an edit of the
/// document, and its health and ID; and, if another program changed it, what changed (the
/// purple Changed on disk card, with Mark as seen). What an error card's button asked, if
/// one was.
///
/// SEAM: its parameters (`featureProps`' PARAMETERS: a pad's profile, extent, and
/// direction), which need its fields read, as its dialog reads them; and its Edit feature
/// and Suppress buttons (the timeline's double-click and menu edit it already).
/// SEAM: the ID row's copy button (`.prop .v .copy`), once the app has a clipboard command
/// of its own.
fn feature<P: Clients + 'static>(
    ui: &mut Ui,
    tab: &mut Tab,
    entry: &Entry,
    sessions: &Arc<Sessions<P>>,
    name: &mut Option<NameField>,
    started: &mut bool,
) -> Option<properties::CardAct> {
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
        asked = asked.or(ui
            .push_id("feature-card", |ui| {
                properties::card(ui, &said, Some(&entry.name), offer)
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
        *started = true;
        field.typed.push(text.clone());
    }
    field.text = text;
    *name = Some(field);
    properties::section(ui, "Health");
    let (state, colour) = match entry.health {
        Health::Healthy => ("Healthy", theme::SUCCESS),
        Health::Warning => ("Warning", theme::WARNING),
        Health::Error => ("Error", theme::ERROR_TEXT),
        Health::Suppressed => ("Suppressed", theme::PROPERTY_VALUE),
        Health::Unknown => ("Not built", theme::TEXT_SUBDUED),
    };
    properties::text_row(ui, "State", properties::body(state, colour));
    properties::text_row(
        ui,
        "ID",
        egui::text::LayoutJob::single_section(
            entry.id.as_str().to_owned(),
            properties::format(theme::MONOSPACE_SIZE, theme::PROPERTY_VALUE, true),
        ),
    );
    properties::hint(
        ui,
        &[(
            "Hovering a feature in the timeline lights up the faces it made.",
            false,
        )],
    );
    asked
}

/// The selection's faces and edges (`selectionProps`, M5.7), each under its kind: its name
/// as an agent writes it, and whether a store takes it, Stable; or, Unstable, the refusal
/// a store gives it, with its suggestion (what a fillet's edges would say); or, Waiting,
/// why it isn't answered (its feature didn't build, or its body isn't named yet).
///
/// SEAM: the prototype's look for it (`selectionProps`: each element's measures, its
/// surface type, and its feature), which needs an `elements` read of each; and a vertex's
/// row, once vertices are picked.
fn selected_elements(ui: &mut Ui, tab: &Tab) {
    let elements: Vec<_> = tab.selection().elements().collect();
    let sub = properties::body("Selection", theme::PROPERTY_HEAD_SUB);
    let title = match elements.len() {
        1 => "1 element selected".to_owned(),
        n => format!("{n} elements selected"),
    };
    properties::head(ui, Icon::SelFace, &title, sub);
    // The head is painted, not labelled: the count said again where it reads.
    properties::text_row(
        ui,
        "Selected",
        properties::body(&title, theme::PROPERTY_VALUE),
    );
    for (place, picked) in elements.into_iter().enumerate() {
        // Each element's rows under an ID of its own: they're keyed alike.
        ui.push_id(("selected", place), |ui| selected_element(ui, picked));
    }
}

/// A selected element's rows ([`selected_elements`]).
fn selected_element(ui: &mut Ui, picked: &crate::selection::Picked) {
    use crate::selection::State;
    let kind = match picked.found().map(silico_core::selection::Selected::kind) {
        Some(silico_protocol::ElementKind::Face) => "Face",
        Some(silico_protocol::ElementKind::Edge) => "Edge",
        Some(silico_protocol::ElementKind::Vertex) => "Vertex",
        _ => "Element",
    };
    properties::section(ui, kind);
    properties::text_row(
        ui,
        "Name",
        egui::text::LayoutJob::single_section(
            picked.name.clone(),
            properties::format(theme::MONOSPACE_SIZE, theme::PROPERTY_VALUE, true),
        ),
    );
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
fn problems(ui: &mut Ui, tab: &Tab) -> Option<properties::CardAct> {
    properties::document_cards(ui, tab, &tab.document_problems, "document-problem")
}

/// The document's properties, with nothing selected (`docProps`): its head (its file's name
/// over its path, or an untitled part's name over its having no file yet), its problems,
/// its revision, units, features, save state, and file on disk, and its model.
///
/// SEAM: the model's measures (`docProps`' Volume, Area, Size, and Faces), which need a
/// `mass_properties` read of its bodies the app doesn't make yet; its bodies are counted.
fn document(ui: &mut Ui, tab: &Tab) -> Option<properties::CardAct> {
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
        (Some(bodies), _) => properties::text_row(
            ui,
            "Bodies",
            properties::body(&bodies.to_string(), theme::PROPERTY_VALUE),
        ),
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
    selected: Option<FeatureId>,
    /// A row right-clicked, selected as its menu opens.
    menu_selected: Option<FeatureId>,
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
/// definition prefixes). SEAM(#351): `tab.prefixes.prefix(id)`; none until it lands.
fn sketch_prefix(_tab: &Tab, _id: &FeatureId) -> Option<&'static str> {
    None
}

/// Whether a sketch's row asks for Edit Sketch. SEAM(#351):
/// `crate::sketch::edit_asked(Some(row), tab.can_sketch())`; none until it lands.
fn sketch_edit_asked(_tab: &Tab, _row: &egui::Response) -> bool {
    false
}

/// A prefix's colour (`.row .prefix`, `.under`, `.over`).
fn prefix_colour(prefix: &str) -> Color32 {
    match prefix {
        "(–)" => theme::SKETCH_UNDER_DEFINED,
        "(+)" => theme::SKETCH_OVER_DEFINED,
        _ => theme::BROWSER_PREFIX,
    }
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
    let origin_open = header(ui, "Origin", None);
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
        _ => "None yet. A pad or a primitive makes one.".to_owned(),
    };
    let groups = [
        ("Bodies", &tab.browser.bodies, Icon::Body, no_body),
        (
            "Sketches",
            &tab.browser.sketches,
            Icon::Sketch,
            "None yet. Create Sketch starts one.".to_owned(),
        ),
    ];
    for (group_name, rows, icon, empty) in groups {
        let count = rows.is_empty().then_some("0");
        if !header(ui, group_name, count) {
            continue;
        }
        let sketches = group_name == "Sketches";
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
                let answer = row(
                    ui,
                    &Line {
                        id: egui::Id::new(("browser-feature", each.id.as_str())),
                        depth: 2,
                        caret: None,
                        eye: Some(tab.shown(&what)),
                        icon,
                        prefix: if sketches {
                            sketch_prefix(tab, &each.id)
                        } else {
                            None
                        },
                        name: &each.name,
                        selected: Some(tab.selection().marks_body(&each.id)),
                        meta: None,
                        mark: marked.then_some((each.health, text)),
                        // A sketch's own failure reddens its name (`.row.err`); a body's
                        // mark is its chain's.
                        failed: sketches && each.health == Health::Error,
                        // Changed by another program (`d.outside`), its purple dot where it
                        // has no health mark.
                        outside: tab.outside.get(&each.id).map(String::as_str),
                        clicks: false,
                    },
                );
                if answer.eye.as_ref().is_some_and(egui::Response::clicked) {
                    asked.toggled.push(what);
                } else if answer.row.clicked() {
                    asked.selected = Some(each.id.clone());
                }
                if let Some(entry) = tab.entry(each.id.as_str()) {
                    answer.row.context_menu(|ui| {
                        // Selected as it's opened, as the prototype's `contextmenu` selects it.
                        if tab.selection().feature() != Some(&each.id) {
                            asked.menu_selected = Some(each.id.clone());
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

/// A group's header row (a folder, with its caret, and `meta`): whether it's open.
fn header(ui: &mut Ui, name: &str, meta: Option<&str>) -> bool {
    let ctx = ui.ctx().clone();
    let mut openness = Openness::load(&ctx, name);
    let answer = row(
        ui,
        &Line {
            id: egui::Id::new(("browser-group", name)),
            depth: 1,
            caret: Some(openness.open),
            eye: None,
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
    openness.open
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
    /// What another program changed in it, if it did (`d.outside`): a purple dot at its
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
        eye.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, true, format!("{word} {}", line.name))
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
                None,
            ),
            shown,
        )
    });
    let hidden = eye.as_ref().is_some_and(|(_, shown)| !shown);
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
        let galley = ui.painter().layout_no_wrap(
            meta.to_owned(),
            egui::FontId::proportional(theme::HEADER_SIZE),
            colour,
        );
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
/// timeline's alike: a purple dot filling `rect` (`.state-dot`, or the timeline's
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
        started: bool,
        drawing: crate::viewport::Drawing,
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
        tab.browser = Browser { bodies, sketches };
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
            started: false,
            drawing: crate::viewport::Drawing::default(),
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
                            started: &mut shown.started,
                            drawing: &mut shown.drawing,
                            dialog_key: None,
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
            assert_eq!(icon_at(&h, header.glyph), None, "{name}'s eye");
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
    fn a_row_another_program_changed_has_its_purple_dot_where_it_has_no_health_mark() {
        // The prototype's row (`o.mark === 'outside'`): a `.state-dot` (8 square, round) in
        // `--purple` (#b780ff) at the row's right, the name cut 4 (`.row`'s gap) before it;
        // a row with a health mark shows that mark alone.
        const PURPLE: Color32 = Color32::from_rgb(0xb7, 0x80, 0xff);
        const DOT: f32 = 8.0;
        let mut tab = modelled();
        // Named long enough to be cut short by the dot.
        let long = "Block with a name long enough to run past the browser's edge";
        tab.browser.bodies[0].name = long.to_owned();
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
            matches!(circles[..], [(centre, radius, PURPLE)]
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
        // Depth 2, no caret: the eye, open, Gray.550; the icon, Gray.500; the name, Gray.700,
        // a failed sketch's `--red`, cut 4 before its mark, if it has one: `--red` for an
        // error, `--orange` for a warning, said with what's said of its feature.
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
            let eye = format!("Hide {name}");
            assert!(labelled_at(&h, &eye, laid.eye), "{eye} at {:?}", laid.eye);
            assert_eq!(icon_at(&h, laid.glyph), Some(css::G550), "{name}'s eye");
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
        assert!(!h.state().started, "an edit asked");

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
        tab.hidden
            .insert(crate::tab::Hideable::Feature(fid("b0000001")));
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
        let none_yet = "None yet. A pad or a primitive makes one.";
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
        started: bool,
        drawing: crate::viewport::Drawing,
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
            started: false,
            drawing: crate::viewport::Drawing::default(),
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
                            started: &mut shown.started,
                            drawing: &mut shown.drawing,
                            dialog_key: None,
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
        y = row_at(&h, "Bodies", "1", y, value, css::G800);
        hint_at(&h, "Double-click a feature in the timeline to edit it.", y);
    }

    #[test]
    fn a_features_properties_are_drawn_as_feature_props_with_its_error_card_and_name_field() {
        let mut tab = part();
        tab.selection_mut().select_feature(fid("c0000001"));
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
        y = section_at(&h, "HEALTH", y + css::ROW_H);
        y = row_at(&h, "State", "Error", y, body(css::FS), css::RED);
        y = row_at(&h, "ID", "c0000001", y, mono(), css::G800);
        hint_at(
            &h,
            "Hovering a feature in the timeline lights up the faces it made.",
            y,
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
                    dialog
                        .borrow_mut()
                        .ui(ui, &[], None, &|_| None, &|_| false, true);
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
                        let done = dialog.ui(ui, &[], preview, &|_| None, &|_| false, true);
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
    fn each_dialogs_choices_draw_as_seg_where_their_words_fit_and_as_a_select_where_not() {
        // A revolve about one of its sketch's lines, offered as itself, besides each kind's.
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
        let mut selects = Vec::new();
        for dialog in dialogs {
            let kind = dialog.kind.title();
            let offered = choices(&dialog);
            let h = dialog_drawn(dialog, None);
            for (key, options, chosen) in offered {
                if let Some(select) = h.query_by_role_and_label(Role::ComboBox, key) {
                    // A select: what's chosen shows; none of the words is laid out as `.seg`.
                    assert_eq!(
                        select.accesskit_node().value().as_deref(),
                        Some(options[chosen].as_str()),
                        "{kind}'s {key}"
                    );
                    for option in &options {
                        assert!(h.query_by_label(option).is_none(), "{kind}'s {option}");
                    }
                    selects.push(format!("{kind} {key}"));
                    continue;
                }
                // `.seg`: no two choices overlap, and each one's words lie inside it.
                let rects: Vec<Rect> = options
                    .iter()
                    .map(|option| h.get_by_role_and_label(Role::Button, option).rect())
                    .collect();
                for (at, rect) in rects.iter().enumerate() {
                    for other in &rects[at + 1..] {
                        assert!(
                            rect.intersect(*other).area() <= 0.0,
                            "{kind}'s {key}: {rect:?} and {other:?}"
                        );
                    }
                    let shown = text(&h, &options[at]);
                    let words = Rect::from_min_size(shown.pos, shown.galley.size());
                    assert!(
                        rect.contains_rect(words),
                        "{kind}'s {:?}: {words:?} outside {rect:?}",
                        options[at]
                    );
                }
            }
        }
        // At 295 (the reviewer's measures, Inter Medium at 12: One side 50.6, Symmetric 61.6,
        // Two sides 58.5, Through all 64.6, in a value column of 167): the pocket's four and
        // the pad's three don't fit; a revolve's two forms and two axes do; its three axes,
        // one a line's ID, don't.
        assert_eq!(
            selects,
            ["Pad Extent", "Pocket Extent", "Revolve Axis"],
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
        let h = dialog_drawn(dialog(), Some(properties::Preview::UpToDate(7)));
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
    const BROKEN_JSON: &str = r#"{"code": "reference_broken", "message": "An edge the fillet names is gone", "feature": "c0000001", "elements": ["Edge3", "Edge5"], "elements_total": 5, "parameter": "edges", "cause": "Edge3 and Edge5 aren't in its base", "suggestion": "Pick the edges again", "candidates": ["Edge2", "Edge4"], "advice": null}"#;

    /// [`part`] with Hole selected, its card [`broken`]'s.
    fn broken_hole() -> Tab {
        let mut tab = part();
        tab.said.insert("c0000001".to_owned(), Said::of(&broken()));
        tab.selection_mut().select_feature(fid("c0000001"));
        tab
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
            "CANDIDATES, CLOSEST FIRST",
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
        // `featureProps` gives `noShow`: Edit, off (a cut has no dialog), and Copy as JSON.
        let edit = h.get_by_role_and_label(Role::Button, "Edit Hole");
        assert!(edit.accesskit_node().is_disabled());
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
                r#"{"code": "fingerprint_missing", "message": "Hole stores no fingerprint", "feature": "c0000001", "elements": [], "elements_total": 0, "parameter": null, "cause": null, "suggestion": null, "candidates": [], "advice": null}"#
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
        assert!(shown.started);
    }

    #[test]
    fn a_document_problems_show_in_timeline_selects_its_feature_for_the_timeline_to_scroll_to() {
        // `docProps` gives every card Edit and Show in timeline: the gate's problem naming
        // Hole, with nothing selected.
        let mut tab = part();
        let gate = silico_core::Error::new(
            silico_core::ErrorCode::InvalidInput,
            "Hole's tool names no feature",
        )
        .with_feature("c0000001")
        .with_parameter("tool");
        tab.document_problems.push(Said::of(&gate));
        let mut h = drawn(tab);
        assert!(
            h.query_by_label("Revision").is_some(),
            "the document's view"
        );
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
        assert!(!h.state().started, "a selection sends nothing");
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
        let heading = text(&h, "CANDIDATES, CLOSEST FIRST");
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
        // `.cand`s: across the body, a line and its padding high, 4 apart.
        let body_right = inner_right - css::EC_HEAD_SIDE;
        let mut top = heading_top + css::LH + css::CANDS_MARGIN;
        for name in ["Edge2", "Edge4"] {
            let row = Rect::from_min_max(
                Pos2::new(body_left, top),
                Pos2::new(body_right, top + 2.0 * css::CAND_PADDING_Y + css::LH),
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
        // `.ec-actions`: Edit Hole (`.btn-secondary`, Gray.300, off: its words Gray.750),
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
}
