//! A feature's commands from the timeline and the browser (docs/design/m5.0-app.md, section
//! 8, *The timeline*: "Right-click also gives Rename, Suppress or Unsuppress, and Find in
//! Browser"; the approved prototype's `contextmenu` handler in js/interact.js): the right-click
//! menu each draws for a feature, its items in the prototype's order and words, each off
//! with its reason where it can't apply.
//!
//! Every item goes through the core's commands (invariant 3): Edit feature opens the
//! feature's dialog as a double-click does.
//!
//! Left out: the prototype's "Roll back to here". SEAM: the core has no rollback (the
//! rollback bar is out of M5, DESIGN.md's Proposal 5); the item goes after Find in… once
//! it has one.

use eframe::egui::{self, RichText, Ui};
use silico_core::document::FeatureId;
use silico_core::timeline::{Entry, Health};

use crate::tab::Tab;
use crate::theme;

/// Where a feature is shown: its item in the timeline, or its row in the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Timeline,
    Browser,
}

/// What a menu asked, done once its pane is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Edit feature: the feature's dialog, as a double-click opens it.
    Edit(FeatureId),
    /// Find in browser: the feature selected, its row scrolled to.
    FindInBrowser(FeatureId),
    /// Find in timeline: the feature selected, its item scrolled to.
    FindInTimeline(FeatureId),
}

/// One of the menu's items, in the prototype's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Edit,
    Find,
}

/// The items, in order (the prototype's: Edit, then Find in…).
pub const ITEMS: [Item; 2] = [Item::Edit, Item::Find];

/// An item as the menu shows it: its words, the keys or gesture beside them, and why it's
/// off, if it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub item: Item,
    pub label: String,
    pub keys: Option<String>,
    pub off: Option<String>,
}

/// The words of `item` for `entry`, shown at `place` (the prototype's: "Edit Sketch" for a
/// sketch, else "Edit feature"; "Find in browser" from the timeline, "Find in timeline"
/// from the browser).
pub fn label(item: Item, entry: &Entry, place: Place) -> &'static str {
    match item {
        Item::Edit if entry.kind == "sketch" => "Edit Sketch",
        Item::Edit => "Edit feature",
        Item::Find => match place {
            Place::Timeline => "Find in browser",
            Place::Browser => "Find in timeline",
        },
    }
}

/// Why the feature `entry` can't be changed in `tab` now, if it can't: the tab closing, a
/// dialog open (its Cancel undoes to where it opened), or a document the gate refused,
/// which takes no edit but of its text.
fn unchangeable(tab: &Tab) -> Option<String> {
    if tab.closing() {
        return Some("The document is closing".to_owned());
    }
    if tab.in_dialog() {
        return Some("Close the open dialog first".to_owned());
    }
    if tab
        .timeline
        .as_ref()
        .is_some_and(|timeline| timeline.problems > 0)
    {
        return Some(
            "The document has problems of its own: no feature changes until they're mended"
                .to_owned(),
        );
    }
    None
}

/// Why `item` is off for the feature `entry` of `tab`, at `place`, if it is.
pub fn off(tab: &Tab, entry: &Entry, item: Item, place: Place) -> Option<String> {
    match item {
        Item::Edit => {
            if entry.kind == "sketch" {
                // SEAM(#351): sketch mode's Edit Sketch.
                return Some("Sketch mode isn't built yet".to_owned());
            }
            let Some(kind) = crate::dialog::Kind::of(entry.kind) else {
                return Some(format!(
                    "{} has no dialog yet",
                    crate::timeline::kind_words(entry.kind)
                ));
            };
            if tab.can_open(kind) {
                None
            } else {
                Some(unchangeable(tab).unwrap_or_else(|| "Its dialog can't open now".to_owned()))
            }
        }
        Item::Find => match place {
            Place::Browser => None,
            Place::Timeline if tab.in_browser(&entry.id) => None,
            Place::Timeline => Some(no_row(tab, entry)),
        },
    }
}

/// Why the browser has no row for `entry`: it lists bodies and sketches, a body by the
/// feature that ends its chain; a feature that failed there built none.
fn no_row(tab: &Tab, entry: &Entry) -> String {
    let consumed = tab
        .timeline
        .as_ref()
        .and_then(|timeline| timeline.features.as_ref())
        .is_some_and(|entries| entries.iter().any(|other| other.inputs.contains(&entry.id)));
    if consumed {
        "It's inside a body: the browser lists bodies and sketches".to_owned()
    } else if entry.health == Health::Error {
        "It built no body, so the browser has no row for it".to_owned()
    } else {
        "The browser has no row for it".to_owned()
    }
}

/// The menu's items for `entry` in `tab`, at `place`, in order.
pub fn items(tab: &Tab, entry: &Entry, place: Place) -> Vec<Shown> {
    ITEMS
        .iter()
        .map(|&item| Shown {
            item,
            label: label(item, entry, place).to_owned(),
            keys: match item {
                Item::Edit => Some("double-click".to_owned()),
                Item::Find => None,
            },
            off: off(tab, entry, item, place),
        })
        .collect()
}

/// The menu's head (`.menu .mh`): the feature's name, in capitals.
fn head(ui: &mut Ui, name: &str) {
    ui.label(
        RichText::new(name.to_uppercase())
            .size(theme::TOOLBAR_LABEL_SIZE)
            .color(theme::TOOLBAR_LABEL_TEXT)
            .extra_letter_spacing(theme::CAPITALS_LETTER_SPACING),
    );
}

/// One item (`.menu .mi`), on or off: off, its reason its hover text and its description
/// for a screen reader. Whether it was clicked.
fn item(ui: &mut Ui, label: &str, keys: Option<&str>, off: Option<&str>) -> bool {
    let mut button = egui::Button::new(label);
    if let Some(keys) = keys {
        button = button.shortcut_text(keys);
    }
    let response = ui.add_enabled(off.is_none(), button);
    if let Some(off) = off {
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_description(off.to_owned());
        });
        return response.on_disabled_hover_text(off).clicked();
    }
    response.clicked()
}

/// The right-click menu of the feature `entry`, at `place` (the prototype's, in its order:
/// the name, Edit feature, Find in…): what was clicked, the menu closed.
pub fn menu(ui: &mut Ui, tab: &Tab, entry: &Entry, place: Place) -> Option<Act> {
    head(ui, &entry.name);
    let mut asked = None;
    for shown in items(tab, entry, place) {
        let clicked = item(
            ui,
            &shown.label,
            shown.keys.as_deref(),
            shown.off.as_deref(),
        );
        if clicked {
            let id = entry.id.clone();
            asked = Some(match shown.item {
                Item::Edit => Act::Edit(id),
                Item::Find => match place {
                    Place::Timeline => Act::FindInBrowser(id),
                    Place::Browser => Act::FindInTimeline(id),
                },
            });
            ui.close();
        }
    }
    asked
}

/// The menu of the origin's row `name` (its point, an axis, or a plane): the browser's
/// items, each off, for none is a feature.
pub fn origin_menu(ui: &mut Ui, name: &str) {
    head(ui, name);
    let off = format!("{name} is the origin's, not a feature");
    for (label, keys) in [
        ("Edit feature", Some("double-click")),
        ("Find in timeline", None),
    ] {
        item(ui, label, keys, Some(&off));
    }
}

/// What a menu asked, done to `tab` through `sessions`: whether a command was started.
pub fn act<P: silico_core::kernel::Clients + 'static>(
    tab: &mut Tab,
    sessions: &std::sync::Arc<silico_core::command::Sessions<P>>,
    act: Act,
) -> bool {
    match act {
        Act::Edit(id) => {
            // SEAM(#351): `tab.edit_sketch_or_feature(sessions, &id)`, Edit Sketch for a
            // sketch; until then a sketch's item is off.
            tab.edit_feature(sessions, &id);
            true
        }
        Act::FindInBrowser(id) => {
            tab.selection_mut().find_in_browser(id);
            false
        }
        Act::FindInTimeline(id) => {
            tab.show_in_timeline(id);
            false
        }
    }
}
