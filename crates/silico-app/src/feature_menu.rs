//! A feature's commands from the timeline and the browser (docs/design/m5.0-app.md, section
//! 8, *The timeline*: "Right-click also gives Rename, Suppress or Unsuppress, and Find in
//! Browser"; the approved prototype's `contextmenu` handler in js/interact.js): the right-click
//! menu each draws for a feature, its items in the prototype's order and words, each off
//! with its reason where it can't apply; Rename's field, typed in the item's or the row's
//! own place (the prototype's `renameSelected`); and what the status bar says a suppression
//! or a deletion did (the prototype's toast).
//!
//! Every item goes through the core's commands (invariant 3): Edit feature opens the
//! feature's dialog as a double-click does, Rename is `set_field` of `name`, Suppress and
//! Unsuppress are `suppress_feature`, and Delete is `remove_feature`, each an edit in the
//! tab's transaction, so one undo takes it back. Delete asks nothing first, as the
//! prototype's `del`: SolidWorks' Confirm Delete lists a feature's absorbed and child
//! features, to delete with it or keep, but the core never deletes a child with its
//! parent: it rewires what consumed the feature to its base, says so, and refuses, naming
//! both, where nothing can stand in, so there's nothing to confirm and the undo is there.
//!
//! Left out: the prototype's "Roll back to here". SEAM: the core has no rollback (the
//! rollback bar is out of M5, DESIGN.md's Proposal 5); the item goes between Find in… and
//! Delete once it has one.

use eframe::egui::{
    self, Color32, FontId, Pos2, Rect, Sense, Ui, Vec2, WidgetInfo, WidgetType, vec2,
};
use silico_core::command::{Removed, Suppressed};
use silico_core::document::FeatureId;
use silico_core::document::chain::Rewired;
use silico_core::timeline::{Entry, Health};

use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::shortcuts::Command;
use crate::tab::Tab;
use crate::theme;

/// Where a feature is shown: its item in the timeline, or its row in the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Timeline,
    Browser,
}

/// A feature's name being typed in its own field (F2, or Rename in a menu): the feature,
/// where its field is, and the text so far.
#[derive(Debug, Clone, PartialEq)]
pub struct Renaming {
    pub feature: FeatureId,
    pub place: Place,
    pub text: String,
    /// Whether the field has had the focus: until it has, the focus is asked for it; once
    /// it has, its losing it sends the name.
    focused: bool,
    /// Whether it's typed over whole still: until the person first types, deletes, or moves
    /// in it.
    whole: bool,
    /// Where the field was last drawn, clipped to what shows of it: a press of the pointer
    /// outside it finishes the name before anything else the press does ([`clicked_away`]).
    rect: Option<Rect>,
}

impl Renaming {
    /// The feature `feature`'s name, `text`, to be typed over at `place`.
    pub fn new(feature: FeatureId, place: Place, text: String) -> Self {
        Renaming {
            feature,
            place,
            text,
            focused: false,
            whole: true,
            rect: None,
        }
    }

    /// Whether its field has had the focus: from then on, the name typed in it is sent
    /// when it's left (Enter, a click away, or a command's shortcut).
    pub fn focused(&self) -> bool {
        self.focused
    }
}

/// Whether this frame's input presses the pointer away from `renaming`'s field, which has
/// had the focus: a click away, which finishes the name (SolidWorks keeps a name typed in
/// its tree when the focus leaves it). The window asks this before any of its controls is
/// drawn, so the name is sent before whatever the click does: a dialog the click opens
/// starts after it (its Cancel keeps the name), and a save the click asks saves it.
pub fn clicked_away(ctx: &egui::Context, renaming: &Renaming) -> bool {
    let Some(rect) = renaming.rect.filter(|_| renaming.focused) else {
        return false;
    };
    ctx.input(|input| {
        input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::PointerButton { pos, pressed: true, .. } if !rect.contains(*pos)
            )
        })
    })
}

/// What a menu asked, done once its pane is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Edit feature: the feature's dialog, as a double-click opens it.
    Edit(FeatureId),
    /// Rename: the feature's own field, where the menu was.
    Rename(FeatureId, Place),
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
    Rename,
    Suppress,
    Find,
    Delete,
}

/// The items, in order (the prototype's: Edit, Rename, Suppress, Find in…, a separator,
/// Delete).
pub const ITEMS: [Item; 5] = [
    Item::Edit,
    Item::Rename,
    Item::Suppress,
    Item::Find,
    Item::Delete,
];

/// An item as the menu shows it: its words, the gesture in dim words after them, its keys'
/// caps, and why it's off, if it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub item: Item,
    pub label: String,
    /// Edit feature's "double-click" (the prototype's `sub`).
    pub sub: Option<&'static str>,
    /// Its keys' caps, as the platform names them: none for an item with no keys.
    pub caps: Vec<String>,
    pub off: Option<String>,
}

/// Edit feature's gesture, in dim words after it.
const DOUBLE_CLICK: &str = "double-click";

/// `item`'s icon (the prototype's: Edit's pencil and Suppress's struck circle), if it has
/// one; the others keep its place.
fn icon(item: Item) -> Option<Icon> {
    match item {
        Item::Edit => Some(Icon::Edit),
        Item::Suppress => Some(Icon::Suppress),
        Item::Rename | Item::Find | Item::Delete => None,
    }
}

/// `item`'s gesture, and its keys' caps on `ctx`'s platform.
fn keys(item: Item, ctx: &egui::Context) -> (Option<&'static str>, Vec<String>) {
    match item {
        Item::Edit => (Some(DOUBLE_CLICK), Vec::new()),
        Item::Rename => (None, Command::Rename.caps(ctx)),
        Item::Delete => (None, Command::Delete.caps(ctx)),
        Item::Suppress | Item::Find => (None, Vec::new()),
    }
}

/// The words of `item` for `entry`, shown at `place` (the prototype's: "Edit Sketch" for a
/// sketch, else "Edit feature"; "Unsuppress" for a suppressed feature; "Find in browser"
/// from the timeline, "Find in timeline" from the browser).
pub fn label(item: Item, entry: &Entry, place: Place) -> &'static str {
    match item {
        Item::Edit if entry.kind == "sketch" => "Edit Sketch",
        Item::Edit => "Edit feature",
        Item::Rename => Command::Rename.label(),
        Item::Suppress if entry.suppressed => "Unsuppress",
        Item::Suppress => Command::Suppress.label(),
        Item::Find => match place {
            Place::Timeline => Command::FindInBrowser.label(),
            Place::Browser => Command::FindInTimeline.label(),
        },
        Item::Delete => Command::Delete.label(),
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
        Item::Rename | Item::Suppress | Item::Delete => unchangeable(tab),
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
pub fn items(tab: &Tab, entry: &Entry, place: Place, ctx: &egui::Context) -> Vec<Shown> {
    ITEMS
        .iter()
        .map(|&item| {
            let (sub, caps) = keys(item, ctx);
            Shown {
                item,
                label: label(item, entry, place).to_owned(),
                sub,
                caps,
                off: off(tab, entry, item, place),
            }
        })
        .collect()
}

/// What a menu's lines take: the widest of its head's and its items', and the least
/// `.menu` gives (`min-width: 200px`, less its border and its padding).
fn width(ui: &Ui, head: &egui::Galley, shown: &[(Shown, Lines)]) -> f32 {
    let least =
        theme::MENU_MIN_WIDTH - 2.0 * (f32::from(theme::MENU_PADDING) + theme::BORDER_WIDTH);
    shown
        .iter()
        .map(|(shown, lines)| {
            let sub = lines.sub.as_ref().map_or(0.0, |sub| {
                theme::MENU_ITEM_GAP + theme::MENU_ITEM_SUB_MARGIN + sub.size().x
            });
            // `.sc`, at the right, is there with no keys too: its gap and its padding. Its
            // caps sit in one `.keys` span, 2 apart (`.keys { gap: 2px }`).
            2.0 * theme::MENU_ITEM_PADDING
                + ICON_SIZE
                + theme::MENU_ITEM_GAP
                + lines.label.size().x
                + sub
                + theme::MENU_ITEM_GAP
                + theme::MENU_KEYS_PADDING
                + crate::start::key_caps_width_spaced(ui, &shown.caps, theme::KEY_CAP_GAP)
        })
        .fold(
            least.max(head.size().x + 2.0 * theme::MENU_ITEM_PADDING),
            f32::max,
        )
}

/// An item's words laid out: its label's and its dim gesture's.
struct Lines {
    label: std::sync::Arc<egui::Galley>,
    sub: Option<std::sync::Arc<egui::Galley>>,
}

fn lines(ui: &Ui, shown: &Shown) -> Lines {
    let painter = ui.painter();
    Lines {
        label: painter.layout_no_wrap(
            shown.label.clone(),
            FontId::proportional(theme::BODY_SIZE),
            theme::MENU_ITEM_TEXT,
        ),
        sub: shown.sub.map(|sub| {
            painter.layout_no_wrap(
                sub.to_owned(),
                FontId::proportional(theme::MENU_ITEM_SUB_SIZE),
                theme::MENU_ITEM_SUB_TEXT,
            )
        }),
    }
}

/// The menu's head (`.menu .mh`): the feature's name, in capitals, laid out.
fn head_galley(ui: &Ui, name: &str) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &name.to_uppercase(),
        0.0,
        egui::TextFormat {
            font_id: FontId::proportional(theme::TOOLBAR_LABEL_SIZE),
            color: theme::TOOLBAR_LABEL_TEXT,
            extra_letter_spacing: theme::CAPITALS_LETTER_SPACING,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

/// The head, `width` wide: 6 above its line and 2 under it, 8 in from the left.
fn head(ui: &mut Ui, galley: std::sync::Arc<egui::Galley>, width: f32) {
    let height = theme::MENU_HEAD_TOP + theme::MENU_LINE_HEIGHT + theme::MENU_HEAD_BOTTOM;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let text = galley.text().to_owned();
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &text));
    let line = rect.top() + theme::MENU_HEAD_TOP;
    let at = Pos2::new(
        rect.left() + theme::MENU_ITEM_PADDING,
        line + (theme::MENU_LINE_HEIGHT - galley.size().y) / 2.0,
    );
    ui.painter().galley(at, galley, theme::TOOLBAR_LABEL_TEXT);
}

/// The separator before Delete (`.menu .msep`), `width` wide.
fn separator(ui: &mut Ui, width: f32) {
    let height = theme::BORDER_WIDTH + 2.0 * theme::MENU_SEPARATOR_MARGIN;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let line = Rect::from_min_size(
        Pos2::new(rect.left(), rect.top() + theme::MENU_SEPARATOR_MARGIN),
        vec2(width, theme::BORDER_WIDTH),
    );
    ui.painter().rect_filled(line, 0.0, theme::MENU_SEPARATOR);
}

/// What a screen reader is told an item is: its words, then its gesture or its keys.
fn said(ctx: &egui::Context, shown: &Shown) -> String {
    match (shown.sub, shown.caps.is_empty()) {
        (Some(sub), _) => format!("{} {sub}", shown.label),
        (None, false) => format!(
            "{} {}",
            shown.label,
            crate::shortcuts::joined(ctx, &shown.caps)
        ),
        (None, true) => shown.label.clone(),
    }
}

/// One item (`.menu .mi`), `width` wide, on or off: its icon or its spacer, its words, its
/// gesture dim after them, and its caps at its right. Hovered (or focused, the prototype's
/// `.kfocus`) it's filled blue; off, its words are Gray.400 and its reason is its hover text
/// and its description for a screen reader; Delete's words are red (`.danger`), off too.
/// Whether it was clicked.
fn item(ui: &mut Ui, shown: &Shown, lines: Lines, width: f32) -> bool {
    let said = said(ui.ctx(), shown);
    // Painted at the menu's opacity, not faded further when off: `.menu .mi:disabled` is
    // Gray.400 itself.
    let opacity = ui.opacity();
    let response = ui.add_enabled(shown.off.is_none(), |ui: &mut Ui| {
        ui.set_opacity(opacity);
        let (rect, response) =
            ui.allocate_exact_size(vec2(width, theme::MENU_ITEM_HEIGHT), Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &said));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let lit = enabled && (response.hovered() || response.has_focus());
        // `.menu .mi:hover:not(:disabled)` outweighs `.danger` (a class and two
        // pseudo-classes to two classes), so a hovered Delete is lit as the others are;
        // `.danger` comes after `.menu .mi:disabled`, of the same weight, so an off Delete
        // is red still.
        let ink = if lit {
            theme::MENU_ITEM_HOVER_TEXT
        } else if shown.item == Item::Delete {
            theme::ERROR_TEXT
        } else if !enabled {
            theme::TEXT_DISABLED
        } else {
            theme::MENU_ITEM_TEXT
        };
        if lit {
            ui.painter()
                .rect_filled(rect, theme::SMALL_CORNER_RADIUS, theme::MENU_ITEM_HOVER_BG);
        }
        let middle = rect.center().y;
        let icon_rect = Rect::from_min_size(
            Pos2::new(
                rect.left() + theme::MENU_ITEM_PADDING,
                middle - ICON_SIZE / 2.0,
            ),
            Vec2::splat(ICON_SIZE),
        );
        if let Some(icon) = icon(shown.item) {
            icon.image(IconSize::Small)
                .tint(ink)
                .paint_at(ui, icon_rect);
        }
        let label_at = Pos2::new(
            icon_rect.right() + theme::MENU_ITEM_GAP,
            middle - lines.label.size().y / 2.0,
        );
        let label_right = label_at.x + lines.label.size().x;
        ui.painter()
            .galley_with_override_text_color(label_at, lines.label, ink);
        if let Some(sub) = lines.sub {
            let at = Pos2::new(
                label_right + theme::MENU_ITEM_GAP + theme::MENU_ITEM_SUB_MARGIN,
                middle - sub.size().y / 2.0,
            );
            ui.painter().galley(at, sub, theme::MENU_ITEM_SUB_TEXT);
        }
        if !shown.caps.is_empty() {
            // `.menu .mi:hover .kbd`: no fill, a faint white outline, the item's words'
            // colour; else `.kbd`'s own, off or on. 2 apart, as `.keys` sets them.
            let colours = if lit {
                (
                    Color32::TRANSPARENT,
                    theme::MENU_KEY_CAP_HOVER_STROKE,
                    theme::MENU_ITEM_HOVER_TEXT,
                )
            } else {
                (
                    theme::KEY_CAP_BG,
                    theme::KEY_CAP_STROKE,
                    theme::KEY_CAP_TEXT,
                )
            };
            crate::start::key_caps_coloured(
                ui,
                Pos2::new(rect.right() - theme::MENU_ITEM_PADDING, middle),
                &shown.caps,
                colours,
                theme::KEY_CAP_GAP,
            );
        }
        response
    });
    if let Some(off) = &shown.off {
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_description(off.to_owned());
        });
        return response.on_disabled_hover_text(off).clicked();
    }
    response.clicked()
}

/// A menu headed `name`, of `shown`'s items, in order, Delete after a separator: which was
/// clicked, the menu closed.
fn draw(ui: &mut Ui, name: &str, shown: Vec<Shown>) -> Option<Item> {
    // `.menu`'s lines stacked, none apart.
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let head_text = head_galley(ui, name);
    let laid: Vec<(Shown, Lines)> = shown
        .into_iter()
        .map(|shown| {
            let lines = lines(ui, &shown);
            (shown, lines)
        })
        .collect();
    let width = width(ui, &head_text, &laid);
    head(ui, head_text, width);
    let mut clicked = None;
    for (shown, lines) in laid {
        if shown.item == Item::Delete {
            separator(ui, width);
        }
        if item(ui, &shown, lines, width) {
            clicked = Some(shown.item);
            ui.close();
        }
    }
    clicked
}

/// The right-click menu of the feature `entry`, at `place` (the prototype's, in its order:
/// the name, Edit feature, Rename, Suppress or Unsuppress, Find in…, a separator, Delete):
/// what was clicked, the menu closed.
pub fn menu(ui: &mut Ui, tab: &Tab, entry: &Entry, place: Place) -> Option<Act> {
    let shown = items(tab, entry, place, ui.ctx());
    let id = entry.id.clone();
    Some(match draw(ui, &entry.name, shown)? {
        Item::Edit => Act::Edit(id),
        Item::Rename => Act::Rename(id, place),
        Item::Suppress => Act::Suppress(id, !entry.suppressed),
        Item::Find => match place {
            Place::Timeline => Act::FindInBrowser(id),
            Place::Browser => Act::FindInTimeline(id),
        },
        Item::Delete => Act::Delete(id),
    })
}

/// The menu of the origin's row `name` (its point, an axis, or a plane): the browser's
/// items, each off, for none is a feature.
pub fn origin_menu(ui: &mut Ui, name: &str) {
    let off = format!("{name} is the origin's, not a feature");
    let ctx = ui.ctx().clone();
    let shown = ITEMS
        .iter()
        .map(|&item| {
            let (sub, caps) = keys(item, &ctx);
            Shown {
                item,
                label: match item {
                    Item::Edit => "Edit feature",
                    Item::Rename => Command::Rename.label(),
                    Item::Suppress => Command::Suppress.label(),
                    Item::Find => Command::FindInTimeline.label(),
                    Item::Delete => Command::Delete.label(),
                }
                .to_owned(),
                sub,
                caps,
                off: Some(off.clone()),
            }
        })
        .collect();
    draw(ui, name, shown);
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
        Act::Rename(id, place) => {
            tab.start_rename(id, place);
            false
        }
        Act::Suppress(id, suppressed) => {
            tab.suppress_feature(sessions, id, suppressed);
            true
        }
        Act::FindInBrowser(id) => {
            tab.selected_in = Place::Browser;
            tab.selection_mut().find_in_browser(id);
            false
        }
        Act::FindInTimeline(id) => {
            tab.selected_in = Place::Timeline;
            tab.show_in_timeline(id);
            false
        }
        Act::Delete(id) => {
            tab.remove_feature(sessions, id);
            true
        }
    }
}

/// What a rename field's frame came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    /// Still being typed in.
    Typing,
    /// Enter, or a click away (SolidWorks keeps a name typed in its tree when the focus
    /// leaves it): the name typed, to send ([`finished`]).
    Done(String),
}

/// The rename field of `renaming`, of the feature named `name`, in `rect`: given the focus
/// as it's first drawn, its name typed over whole until the person first types, deletes,
/// or moves in it. Esc drops it before it's drawn (`App::keys`), so it never sends.
pub fn field(ui: &mut Ui, renaming: &mut Renaming, rect: egui::Rect, name: &str) -> Field {
    let salt = ("rename", renaming.feature.as_str());
    let id = ui.make_persistent_id(salt);
    let kind = match renaming.place {
        Place::Timeline => theme::FieldKind::Modify,
        Place::Browser => theme::FieldKind::Dialog,
    };
    let focused = ui.memory(|memory| memory.has_focus(id));
    if !focused && !renaming.focused {
        // The focus asked for it until it has it (a new layer's first frame lays it out
        // unseen).
        ui.memory_mut(|memory| memory.request_focus(id));
    }
    if renaming.whole && focused {
        // Typed over whole, as a name selected whole is (the prototype's `prompt`): until
        // the person first types, deletes, or moves in it, what's typed (or pasted, or
        // composed in an input method) replaces the name, and Del or ⌫ clears it. Its text
        // is shown selected too, as egui's DragValue selects its own.
        let first = ui.input(|input| first_edit(&input.events));
        match first {
            Some(replaces) => {
                if replaces {
                    // The selection stored for it covers the name only from the frame
                    // after the field first has the focus, and is kept only where its text
                    // is laid out in glyphs: the name is let go here, so it's replaced
                    // whole either way.
                    renaming.text.clear();
                }
                renaming.whole = false;
            }
            None => {
                let mut state =
                    egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
                let end = renaming.text.chars().count();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(end),
                    )));
                state.store(ui.ctx(), id);
            }
        }
    }
    let edit = egui::TextEdit::singleline(&mut renaming.text);
    let edit = theme::field(ui, edit, salt, kind, false);
    let response = ui.put(rect, edit);
    // Where it shows, clipped to its pane (the browser's scrolled rows): a field scrolled
    // out of view is clipped to nothing, so any press is away from it.
    renaming.rect = Some(response.interact_rect);
    let label = format!("Rename {name}");
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(label);
    });
    if response.has_focus() {
        renaming.focused = true;
    } else if renaming.focused {
        return Field::Done(renaming.text.clone());
    }
    Field::Typing
}

/// Whether `events` hold the person's first edit of a name typed over whole: `Some(true)`
/// for one that replaces it (a letter typed, a paste, an input method's text composed or
/// committed, or Del or ⌫), `Some(false)` for one that keeps it (a key that moves the
/// cursor, a click in it), or none yet. A letter's key comes before its text, and its text
/// decides.
fn first_edit(events: &[egui::Event]) -> Option<bool> {
    events.iter().find_map(|event| match event {
        egui::Event::Text(_) | egui::Event::Paste(_) => Some(true),
        egui::Event::Ime(egui::ImeEvent::Preedit { text, .. } | egui::ImeEvent::Commit(text))
            if !text.is_empty() =>
        {
            Some(true)
        }
        egui::Event::Key {
            key: egui::Key::Backspace | egui::Key::Delete,
            pressed: true,
            modifiers,
            ..
        } if modifiers.is_none() => Some(true),
        egui::Event::Key {
            key:
                egui::Key::ArrowLeft
                | egui::Key::ArrowRight
                | egui::Key::ArrowUp
                | egui::Key::ArrowDown
                | egui::Key::Home
                | egui::Key::End
                | egui::Key::PageUp
                | egui::Key::PageDown,
            pressed: true,
            ..
        } => Some(false),
        egui::Event::PointerButton { pressed: true, .. } => Some(false),
        _ => None,
    })
}

/// The name typed, finished (Enter, or a click away), as it's sent: trimmed, and none if
/// that leaves nothing, which drops it as Esc does (the prototype's `renameSelected`, its
/// `n.trim()`).
pub fn finished(typed: &str) -> Option<&str> {
    Some(typed.trim()).filter(|name| !name.is_empty())
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

    mod drawn {
        use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
        use egui::accesskit::Role;
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        use super::super::{Item, Shown, draw};
        use crate::modal::tests::{
            cap_border, colour, css, fills_at, font, icon_at, kbd_border, near, rects, rgba,
            same_colour, sized, text_shape, widget,
        };
        use crate::theme;

        /// The prototype's `.menu` (styles.css): `min-width: 200px; padding: 4px`, its
        /// border `1px`, all inside its width (`box-sizing: border-box`).
        const MENU_MIN_W: f32 = 200.0;
        const MENU_PAD: f32 = 4.0;
        /// `.menu .mi`: `height: 24px; gap: 8px; padding: 0 8px`; Gray.750, hovered
        /// `--blue` and `--blue-text`, off Gray.400, `.danger` `--red`.
        const MI_H: f32 = 24.0;
        const MI_PAD: f32 = 8.0;
        const MI_GAP: f32 = 8.0;
        const BLUE: Color32 = Color32::from_rgb(0x18, 0x6a, 0xdd);
        const BLUE_TEXT: Color32 = Color32::from_rgb(0xf0, 0xf2, 0xff);
        /// The `subdued` span: `margin-left: 8px; font-size: 11px`, Gray.550.
        const SUB_MARGIN: f32 = 8.0;
        const SUB_FS: f32 = 11.0;
        /// `.menu .mi .sc`: `padding-left: 24px`, at the item's right.
        const SC_PAD: f32 = 24.0;
        /// `.kbd`: 16 high, `border-radius: 3px`, its border 1 and 2 at its bottom;
        /// `.menu .mi:hover .kbd`: `border-color: rgba(255,255,255,.35)`, no fill.
        const KBD_H: f32 = 16.0;
        const KBD_R: u8 = 3;
        /// `.keys`: `gap: 2px` between one shortcut's caps.
        const KEYS_GAP: f32 = 2.0;
        const KBD_HOVER_BORDER: [u8; 4] = [255, 255, 255, 89];
        /// `.menu .mh`: `padding: 6px 8px 2px; font-size: 10px`, Gray.550, on the body's 16
        /// line; `.menu .msep`: `height: 1px; margin: 4px 0`, Gray.250.
        const MH_TOP: f32 = 6.0;
        const MH_BOTTOM: f32 = 2.0;
        const MH_FS: f32 = 10.0;
        const MSEP_MARGIN: f32 = 4.0;

        fn shown(item: Item, label: &str, sub: Option<&'static str>, caps: &[&str]) -> Shown {
            Shown {
                item,
                label: label.to_owned(),
                sub,
                caps: caps.iter().map(|cap| (*cap).to_owned()).collect(),
                off: None,
            }
        }

        /// The menu of a feature named Block, as the timeline shows it, Suppress off.
        fn block() -> Harness<'static> {
            let mut h = Harness::builder()
                .with_size(Vec2::new(600.0, 400.0))
                .build_ui(|ui| {
                    let mut items = vec![
                        shown(Item::Edit, "Edit feature", Some("double-click"), &[]),
                        shown(Item::Rename, "Rename", None, &["F2"]),
                        shown(Item::Suppress, "Suppress", None, &[]),
                        shown(Item::Find, "Find in browser", None, &[]),
                        shown(Item::Delete, "Delete", None, &["Del"]),
                    ];
                    items[2].off = Some("Close the open dialog first".to_owned());
                    draw(ui, "Block", items);
                });
            theme::apply(&h.ctx);
            h.run_steps(4);
            h
        }

        #[test]
        fn the_menu_is_drawn_as_the_prototypes_menu_css_lays_it_out() {
            let mut h = block();
            let items: Vec<Rect> = [
                "Edit feature double-click",
                "Rename F2",
                "Suppress",
                "Find in browser",
                "Delete Del",
            ]
            .into_iter()
            .map(|label| widget(&h, Role::Button, label))
            .collect();
            // Each 24 high, all one width, the widest's (`white-space: nowrap`, in a menu
            // `.menu`'s least wide less its border and its padding at least): here Edit
            // feature's, its words, its gesture, and `.sc`'s padding with no caps in it.
            let advance = |text: &str| text_shape(&h, text).galley.size().x;
            let width = (MI_PAD
                + css::ICON
                + MI_GAP
                + advance("Edit feature")
                + MI_GAP
                + SUB_MARGIN
                + advance("double-click")
                + MI_GAP
                + SC_PAD
                + MI_PAD)
                .max(MENU_MIN_W - 2.0 * (MENU_PAD + css::BORDER));
            assert!(width > MENU_MIN_W - 2.0 * (MENU_PAD + css::BORDER));
            for rect in &items {
                assert!(near(rect.height(), MI_H), "{rect:?}");
                assert!(near(rect.width(), width), "{rect:?}, not {width} wide");
                assert!(near(rect.left(), items[0].left()));
            }
            for pair in items[..4].windows(2) {
                assert!(near(pair[1].top(), pair[0].bottom()), "{pair:?}");
            }
            assert!(near(
                items[4].top(),
                items[3].bottom() + 2.0 * MSEP_MARGIN + css::BORDER
            ));
            let line = Rect::from_min_size(
                Pos2::new(items[3].left(), items[3].bottom() + MSEP_MARGIN),
                Vec2::new(width, css::BORDER),
            );
            assert_eq!(
                fills_at(&h, line),
                [(css::G250, egui::CornerRadius::ZERO)],
                "the separator"
            );
            // The head: the name in capitals, 10, Gray.550, 8 in and on a 16 line 6 down,
            // 2 above the first item.
            let head = text_shape(&h, "BLOCK");
            font(&head, MH_FS, "the head");
            same_colour(colour(&head), css::G550, "the head");
            assert!(near(head.pos.x, items[0].left() + MI_PAD), "{:?}", head.pos);
            let head_line = items[0].top() - MH_BOTTOM - css::LH;
            assert!(near(
                head.pos.y + head.galley.size().y / 2.0,
                head_line + css::LH / 2.0
            ));
            let head_rect = widget(&h, Role::Label, "BLOCK");
            assert!(near(head_rect.top(), head_line - MH_TOP), "{head_rect:?}");
            assert!(near(head_rect.bottom(), items[0].top()), "{head_rect:?}");
            assert!(near(head_rect.width(), width), "{head_rect:?}");
            for (at, rect) in items.iter().enumerate() {
                // The icon (or its 14 wide spacer) 8 in, the words 8 after it, in the
                // item's middle; Edit's and Suppress's icons drawn, the others' none.
                let icon = Rect::from_min_size(
                    Pos2::new(rect.left() + MI_PAD, rect.center().y - css::ICON / 2.0),
                    Vec2::splat(css::ICON),
                );
                let drawn = icon_at(&h, icon);
                assert_eq!(drawn.is_some(), at == 0 || at == 2, "item {at}'s icon");
                let words = [
                    "Edit feature",
                    "Rename",
                    "Suppress",
                    "Find in browser",
                    "Delete",
                ][at];
                let label = text_shape(&h, words);
                font(&label, css::FS, words);
                assert!(
                    near(label.pos.x, icon.right() + MI_GAP),
                    "{words}: {:?}",
                    label.pos
                );
                assert!(near(
                    label.pos.y + label.galley.size().y / 2.0,
                    rect.center().y
                ));
                // Gray.750; off, Gray.400; Delete red. The icon in the words' colour.
                let ink = match at {
                    2 => css::G400,
                    4 => css::RED,
                    _ => css::G750,
                };
                same_colour(colour(&label), ink, words);
                if let Some(tint) = drawn {
                    same_colour(tint, ink, &format!("{words}'s icon"));
                }
            }
            // "double-click": 11, Gray.550, 8 after the words and 8 more.
            let edit = text_shape(&h, "Edit feature");
            let sub = text_shape(&h, "double-click");
            font(&sub, SUB_FS, "double-click");
            same_colour(colour(&sub), css::G550, "double-click");
            assert!(near(
                sub.pos.x,
                edit.pos.x + edit.galley.size().x + MI_GAP + SUB_MARGIN
            ));
            // The keys as caps, 16 high, at the item's right less its padding, unstroked,
            // their border `.kbd`'s, Gray.350.
            for (rect, key) in [(items[1], "F2"), (items[4], "Del")] {
                let cap = rects(&h)
                    .into_iter()
                    .find(|shape| {
                        shape.corner_radius == egui::CornerRadius::same(KBD_R)
                            && rect.contains_rect(shape.rect)
                            && near(shape.rect.height(), KBD_H)
                    })
                    .unwrap_or_else(|| panic!("{key}'s cap"));
                assert!(cap.stroke.is_empty(), "{key}'s cap: {:?}", cap.stroke);
                assert!(near(cap.rect.right(), rect.right() - MI_PAD), "{key}");
                assert!(near(cap.rect.center().y, rect.center().y), "{key}");
                same_colour(cap.fill, css::G250, &format!("{key}'s cap"));
                kbd_border(
                    &cap_border(&h, cap.rect),
                    cap.rect,
                    css::G350,
                    &format!("{key}'s cap"),
                );
                same_colour(colour(&text_shape(&h, key)), css::G800, key);
            }
            // Hovered: filled blue at `--r-sm`, its words, icon, and caps `--blue-text`, the
            // caps unfilled with a faint white outline.
            h.get_by_role_and_label(Role::Button, "Rename F2").hover();
            h.run_steps(2);
            assert_eq!(
                fills_at(&h, items[1]),
                [(BLUE, egui::CornerRadius::same(css::R_SM))]
            );
            same_colour(colour(&text_shape(&h, "Rename")), BLUE_TEXT, "hovered");
            same_colour(colour(&text_shape(&h, "F2")), BLUE_TEXT, "hovered F2");
            let cap = rects(&h)
                .into_iter()
                .find(|shape| {
                    shape.corner_radius == egui::CornerRadius::same(KBD_R)
                        && items[1].contains_rect(shape.rect)
                        && near(shape.rect.height(), KBD_H)
                })
                .expect("F2's cap");
            assert_eq!(cap.fill, Color32::TRANSPARENT);
            assert!(cap.stroke.is_empty(), "the hovered cap: {:?}", cap.stroke);
            kbd_border(
                &cap_border(&h, cap.rect),
                cap.rect,
                rgba(KBD_HOVER_BORDER),
                "the hovered cap",
            );
            // Delete hovered is lit as the others are (`:hover` outweighs `.danger`); the
            // off Suppress, hovered, isn't.
            h.get_by_role_and_label(Role::Button, "Delete Del").hover();
            h.run_steps(2);
            same_colour(
                colour(&text_shape(&h, "Delete")),
                BLUE_TEXT,
                "hovered Delete",
            );
            h.get_by_role_and_label(Role::Button, "Suppress").hover();
            h.run_steps(2);
            assert!(fills_at(&h, items[2]).is_empty());
            same_colour(
                colour(&text_shape(&h, "Suppress")),
                css::G400,
                "hovered off",
            );
        }

        #[test]
        fn a_menu_of_short_items_is_as_wide_as_the_menus_least_and_an_off_delete_is_red() {
            let mut h = Harness::builder()
                .with_size(Vec2::new(600.0, 400.0))
                .build_ui(|ui| {
                    let mut delete = shown(Item::Delete, "Delete", None, &["Del"]);
                    delete.off = Some("B is the origin's, not a feature".to_owned());
                    draw(
                        ui,
                        "B",
                        vec![shown(Item::Rename, "Rename", None, &["F2"]), delete],
                    );
                });
            theme::apply(&h.ctx);
            h.run_steps(4);
            let rename = widget(&h, Role::Button, "Rename F2");
            assert!(near(
                rename.width(),
                MENU_MIN_W - 2.0 * (MENU_PAD + css::BORDER)
            ));
            // `.danger`'s red, not `.menu .mi:disabled`'s Gray.400: of the same weight,
            // `.danger` comes after it in styles.css.
            same_colour(colour(&text_shape(&h, "Delete")), css::RED, "an off Delete");
        }

        #[test]
        fn an_items_caps_are_2_apart_as_keys_sets_them() {
            let mut h = Harness::builder()
                .with_size(Vec2::new(600.0, 400.0))
                .build_ui(|ui| {
                    draw(
                        ui,
                        "Block",
                        vec![shown(Item::Rename, "Rename", None, &["Shift", "F2"])],
                    );
                });
            theme::apply(&h.ctx);
            h.run_steps(4);
            let item = widget(&h, Role::Button, "Rename Shift+F2");
            let mut caps: Vec<Rect> = rects(&h)
                .into_iter()
                .filter(|shape| {
                    shape.corner_radius == egui::CornerRadius::same(KBD_R)
                        && item.contains_rect(shape.rect)
                        && near(shape.rect.height(), KBD_H)
                })
                .map(|shape| {
                    assert!(shape.stroke.is_empty(), "{shape:?}");
                    shape.rect
                })
                .collect();
            caps.sort_by(|a, b| a.left().total_cmp(&b.left()));
            assert_eq!(caps.len(), 2, "{caps:?}");
            // `.keys { gap: 2px }`, the last cap at the item's right less its padding.
            assert!(near(caps[1].left() - caps[0].right(), KEYS_GAP), "{caps:?}");
            assert!(near(caps[1].right(), item.right() - MI_PAD), "{caps:?}");
        }

        #[test]
        fn a_wide_item_widens_every_item_and_keeps_its_caps_24_from_its_words() {
            let name = "Find in browser, a long way along the track";
            let mut h = Harness::builder()
                .with_size(Vec2::new(800.0, 400.0))
                .build_ui(|ui| {
                    draw(
                        ui,
                        "Block",
                        vec![
                            shown(Item::Rename, "Rename", None, &["F2"]),
                            shown(Item::Find, name, None, &[]),
                            shown(Item::Delete, name, None, &["Del"]),
                        ],
                    );
                });
            theme::apply(&h.ctx);
            h.run_steps(4);
            let painted: Vec<_> = crate::modal::tests::painted(&h)
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Text(text) if text.galley.text() == name => Some(text),
                    _ => None,
                })
                .collect();
            let words = painted[0].galley.size().x;
            let cap = text_shape(&h, "Del").galley.size().x;
            // `.kbd`: its words, 4 of padding and 1 of border at each side, 16 at least.
            let cap = (cap + 2.0 * (4.0 + css::BORDER)).max(16.0);
            let want = MI_PAD + css::ICON + MI_GAP + words + MI_GAP + SC_PAD + cap + MI_PAD;
            assert!(want > MENU_MIN_W);
            let rename = widget(&h, Role::Button, "Rename F2");
            let delete = widget(&h, Role::Button, &format!("{name} Del"));
            sized(
                rename,
                Rect::from_min_size(rename.min, Vec2::new(want, MI_H)),
                "Rename, as wide as the widest",
            );
            assert!(near(delete.width(), want));
        }
    }

    #[test]
    fn a_letters_key_leaves_its_text_to_decide_and_only_a_key_that_moves_keeps_the_name() {
        use egui::{Event, Key, Modifiers};
        let key = |key| Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        // A real keystroke: the letter's key, then its text, which replaces the name.
        assert_eq!(
            first_edit(&[key(Key::D), Event::Text("D".to_owned())]),
            Some(true)
        );
        assert_eq!(first_edit(&[key(Key::D)]), None);
        assert_eq!(first_edit(&[key(Key::Enter)]), None);
        for moves in [
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::ArrowUp,
            Key::ArrowDown,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
        ] {
            assert_eq!(
                first_edit(&[key(moves), Event::Text("D".to_owned())]),
                Some(false),
                "{moves:?}"
            );
        }
        for deletes in [Key::Backspace, Key::Delete] {
            assert_eq!(first_edit(&[key(deletes)]), Some(true), "{deletes:?}");
        }
    }

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
