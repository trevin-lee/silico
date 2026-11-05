//! A feature's commands from the timeline and the browser (docs/design/m5.0-app.md, section
//! 8, *The timeline*: "Right-click also gives Rename, Suppress or Unsuppress, and Find in
//! Browser"; the approved prototype's `contextmenu` handler in js/interact.js): the right-click
//! menu each draws for a feature, its items in the prototype's order and words, each off
//! with its reason where it can't apply; and what the status bar says a suppression or a
//! deletion did (the prototype's toast).
//!
//! Every item goes through the core's commands (invariant 3): Edit feature opens the
//! feature's dialog as a double-click does, Suppress and Unsuppress are
//! `suppress_feature`, and Delete is `remove_feature`, each an edit in the tab's
//! transaction, so one undo takes it back. Delete asks nothing first, as the prototype's
//! `del`: SolidWorks' Confirm Delete lists a feature's absorbed and child features, to
//! delete with it or keep, but the core never deletes a child with its parent: it rewires
//! what consumed the feature to its base, says so, and refuses, naming both, where nothing
//! can stand in, so there's nothing to confirm and the undo is there.
//!
//! Left out: the prototype's "Roll back to here". SEAM: the core has no rollback (the
//! rollback bar is out of M5, DESIGN.md's Proposal 5); the item goes between Find in… and
//! Delete once it has one.

use eframe::egui::{self, RichText, Ui};
use silico_core::command::{Removed, Suppressed};
use silico_core::document::FeatureId;
use silico_core::document::chain::Rewired;
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
    /// Suppress (true) or Unsuppress (false).
    Suppress(FeatureId, bool),
    /// Find in browser: the feature selected, its row scrolled to.
    FindInBrowser(FeatureId),
    /// Find in timeline: the feature selected, its item scrolled to.
    FindInTimeline(FeatureId),
    Delete(FeatureId),
}

/// One of the menu's items, in the prototype's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Edit,
    Suppress,
    Find,
    Delete,
}

/// The items, in order (the prototype's: Edit, Suppress, Find in…, a separator, Delete).
pub const ITEMS: [Item; 4] = [Item::Edit, Item::Suppress, Item::Find, Item::Delete];

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
/// sketch, else "Edit feature"; "Unsuppress" for a suppressed feature; "Find in browser"
/// from the timeline, "Find in timeline" from the browser).
pub fn label(item: Item, entry: &Entry, place: Place) -> &'static str {
    match item {
        Item::Edit if entry.kind == "sketch" => "Edit Sketch",
        Item::Edit => "Edit feature",
        Item::Suppress if entry.suppressed => "Unsuppress",
        Item::Suppress => "Suppress",
        Item::Find => match place {
            Place::Timeline => "Find in browser",
            Place::Browser => "Find in timeline",
        },
        Item::Delete => "Delete",
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
        Item::Suppress | Item::Delete => unchangeable(tab),
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
                Item::Suppress | Item::Find | Item::Delete => None,
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

/// One item (`.menu .mi`, Delete's `.danger` red), on or off: off, its reason its hover
/// text and its description for a screen reader. Whether it was clicked.
fn item(ui: &mut Ui, label: &str, keys: Option<&str>, off: Option<&str>, danger: bool) -> bool {
    let text = if danger {
        RichText::new(label).color(theme::ERROR_TEXT)
    } else {
        RichText::new(label)
    };
    let mut button = egui::Button::new(text);
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
/// the name, Edit feature, Suppress or Unsuppress, Find in…, a separator, Delete): what
/// was clicked, the menu closed.
pub fn menu(ui: &mut Ui, tab: &Tab, entry: &Entry, place: Place) -> Option<Act> {
    head(ui, &entry.name);
    let mut asked = None;
    for shown in items(tab, entry, place) {
        if shown.item == Item::Delete {
            ui.separator();
        }
        let clicked = item(
            ui,
            &shown.label,
            shown.keys.as_deref(),
            shown.off.as_deref(),
            shown.item == Item::Delete,
        );
        if clicked {
            let id = entry.id.clone();
            asked = Some(match shown.item {
                Item::Edit => Act::Edit(id),
                Item::Suppress => Act::Suppress(id, !entry.suppressed),
                Item::Find => match place {
                    Place::Timeline => Act::FindInBrowser(id),
                    Place::Browser => Act::FindInTimeline(id),
                },
                Item::Delete => Act::Delete(id),
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
        ("Suppress", None),
        ("Find in timeline", None),
        ("Delete", None),
    ] {
        let danger = label == "Delete";
        if danger {
            ui.separator();
        }
        item(ui, label, keys, Some(&off), danger);
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
        Act::Suppress(id, suppressed) => {
            tab.suppress_feature(sessions, id, suppressed);
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
        Act::Delete(id) => {
            tab.remove_feature(sessions, id);
            true
        }
    }
}

/// `names` as a list in words: "A", "A and B", "A, B, and C".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// What the status bar says a suppression did, each feature by `name`: the feature, then
/// what went with it (its dependents, suppressed; what it needs, unsuppressed). Nothing
/// for one that changed nothing.
pub fn suppressed_words(done: &Suppressed, name: impl Fn(&FeatureId) -> String) -> Option<String> {
    let (word, changed, dependents) = if done.suppressed.is_empty() {
        ("Unsuppressed", &done.unsuppressed, false)
    } else {
        ("Suppressed", &done.suppressed, true)
    };
    if changed.is_empty() {
        return None;
    }
    let others: Vec<String> = changed
        .iter()
        .filter(|id| **id != done.feature)
        .map(&name)
        .collect();
    let feature = name(&done.feature);
    let with = match (dependents, others.len()) {
        (_, 0) => return Some(format!("{word} {feature}")),
        (true, 1) => "which depends on it",
        (true, _) => "which depend on it",
        (false, _) => "which it needs",
    };
    Some(format!("{word} {feature}, and {}, {with}", listed(&others)))
}

/// What the status bar says a deletion did: the feature deleted, then each feature rewired
/// in its place, by `name`.
pub fn removed_words(removed: &Removed, name: impl Fn(&FeatureId) -> String) -> String {
    let mut words = format!("Deleted {}", removed.feature.name);
    for rewired in &removed.rewired {
        words.push_str(" · ");
        words.push_str(&rewired_words(rewired, &name));
    }
    words
}

/// What `rewired` changed, in words.
fn rewired_words(rewired: &Rewired, name: impl Fn(&FeatureId) -> String) -> String {
    let feature = name(&rewired.feature);
    match (rewired.field, &rewired.to) {
        ("base", Some(to)) => format!("{feature} now builds on {}", name(to)),
        ("base", None) => format!("{feature} now begins a body"),
        (field, Some(to)) => format!("{feature}'s {field} is now {}", name(to)),
        (field, None) => format!("{feature} has no {field} now"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fid(id: &str) -> FeatureId {
        FeatureId::new(id).unwrap()
    }

    fn named(id: &FeatureId) -> String {
        match id.as_str() {
            "a0000001" => "Block".to_owned(),
            "b0000001" => "Pin".to_owned(),
            "c0000001" => "Hole".to_owned(),
            "d0000001" => "Bore".to_owned(),
            other => other.to_owned(),
        }
    }

    fn suppressed(feature: &str, suppressed: &[&str], unsuppressed: &[&str]) -> Suppressed {
        Suppressed {
            feature: fid(feature),
            suppressed: suppressed.iter().map(|id| fid(id)).collect(),
            unsuppressed: unsuppressed.iter().map(|id| fid(id)).collect(),
            changed: !(suppressed.is_empty() && unsuppressed.is_empty()),
            commit: None,
        }
    }

    #[test]
    fn a_suppression_says_the_feature_then_what_went_with_it() {
        let said = |done: &Suppressed| suppressed_words(done, named);
        assert_eq!(
            said(&suppressed("b0000001", &["b0000001"], &[])).as_deref(),
            Some("Suppressed Pin")
        );
        assert_eq!(
            said(&suppressed("b0000001", &["b0000001", "c0000001"], &[])).as_deref(),
            Some("Suppressed Pin, and Hole, which depends on it")
        );
        assert_eq!(
            said(&suppressed(
                "a0000001",
                &["a0000001", "c0000001", "d0000001"],
                &[]
            ))
            .as_deref(),
            Some("Suppressed Block, and Hole and Bore, which depend on it")
        );
        assert_eq!(
            said(&suppressed("c0000001", &[], &["b0000001", "c0000001"])).as_deref(),
            Some("Unsuppressed Hole, and Pin, which it needs")
        );
        assert_eq!(
            said(&suppressed("c0000001", &[], &["c0000001"])).as_deref(),
            Some("Unsuppressed Hole")
        );
        // Changing nothing says nothing.
        assert_eq!(said(&suppressed("c0000001", &[], &[])), None);
    }

    #[test]
    fn a_list_in_words_has_its_and_and_its_commas() {
        let names = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(listed(&names(&[])), "");
        assert_eq!(listed(&names(&["A"])), "A");
        assert_eq!(listed(&names(&["A", "B"])), "A and B");
        assert_eq!(listed(&names(&["A", "B", "C"])), "A, B, and C");
    }

    #[test]
    fn a_rewiring_says_what_each_feature_builds_on_now() {
        let rewired = |field: &'static str, to: Option<&str>| Rewired {
            feature: fid("d0000001"),
            field,
            from: Some(fid("c0000001")),
            to: to.map(fid),
        };
        assert_eq!(
            rewired_words(&rewired("base", Some("a0000001")), named),
            "Bore now builds on Block"
        );
        assert_eq!(
            rewired_words(&rewired("base", None), named),
            "Bore now begins a body"
        );
        assert_eq!(
            rewired_words(&rewired("tool", Some("b0000001")), named),
            "Bore's tool is now Pin"
        );
        assert_eq!(
            rewired_words(&rewired("tool", None), named),
            "Bore has no tool now"
        );
    }
}
