//! The app's one command registry (docs/design/m5-ui/DESIGN.md, *Keyboard*: "the sheet is
//! generated from the same command registry as menus, tooltips and search, so a shortcut is
//! never documented in one place and different in another"; the prototype's `CMD`, in
//! js/app.js): each command the keyboard or the pointer reaches, with its name, its keys,
//! its group, and what it does. The shortcut sheet (`crate::sheet`) lists it, the tooltips
//! and the buttons' key caps name their keys from it, and the app's key handling
//! (`App::keys`) takes its keys from it, so none can differ from another.
//!
//! Orbit, Pan, and Zoom (`Chord::Pointer`) are descriptive only: the viewport takes those
//! gestures itself (`crate::viewport`'s navigation, its Shift and middle button and wheel
//! written there), and nothing here matches them; the sheet lists them as the prototype
//! does.
//!
//! Sketch mode's group, `Sketch` (*while a sketch is open*), comes between Dialogs and Help
//! as the prototype orders them: L line, R rectangle, C circle, A arc, P point, T trim,
//! D Smart Dimension, X construction, and Ctrl+Enter Finish Sketch, each its SKETCH tab's
//! button's (`crate::toolbar`); the app takes them only while a sketch is open.
//!
//! Only what the app does today is here. Left out, each a seam:
//! - SEAM: hold Ctrl for no relations (the prototype's *Sketch without relations*), a held
//!   modifier the sketch tools read from the pointer's events, no key's press.
//! - SEAM: Ctrl+8 Look At, once the app can turn the view square to a face or a sketch.
//! - SEAM(M5.7): Select Other (right-click), picking's part 3.
//! - F5, SolidWorks' key for its filter toolbar, which the app hasn't: the app takes it
//!   itself and says where the filters are (`App::keys`), as the prototype's F5 is no
//!   command of its `CMD` and isn't on its sheet.
//! - SEAM: F2 Rename, Delete, and S Search commands, once they're built.

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, Ui};

use crate::start;

/// A command the registry holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Command {
    New,
    Open,
    Save,
    SaveAs,
    Close,
    Undo,
    Redo,
    Rebuild,
    RebuildAll,
    Stop,
    Fit,
    Home,
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Isometric,
    Orbit,
    Pan,
    Zoom,
    FilterFace,
    FilterEdge,
    FilterVertex,
    ClearSelection,
    Hide,
    Ok,
    Cancel,
    Line,
    Rectangle,
    Circle,
    Arc,
    Point,
    Trim,
    Dimension,
    Construction,
    FinishSketch,
    Shortcuts,
}

/// The sheet's groups, in the order its columns list them (the prototype's `order`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    File,
    Edit,
    Rebuild,
    View,
    Selection,
    Dialogs,
    Sketch,
    Help,
}

/// Every group, in the sheet's order.
pub const GROUPS: [Group; 8] = [
    Group::File,
    Group::Edit,
    Group::Rebuild,
    Group::View,
    Group::Selection,
    Group::Dialogs,
    Group::Sketch,
    Group::Help,
];

impl Group {
    /// Its name, as the sheet heads its column (in capitals there).
    pub fn name(self) -> &'static str {
        match self {
            Group::File => "File",
            Group::Edit => "Edit",
            Group::Rebuild => "Rebuild",
            Group::View => "View",
            Group::Selection => "Selection",
            Group::Dialogs => "Dialogs",
            Group::Sketch => "Sketch",
            Group::Help => "Help",
        }
    }

    /// What the sheet says beside its name, if anything (the prototype's `notes`).
    pub fn note(self) -> Option<&'static str> {
        match self {
            Group::Selection => Some("outside sketches"),
            Group::Sketch => Some("while a sketch is open"),
            Group::View => Some("Fusion’s mouse"),
            _ => None,
        }
    }
}

/// One way to reach a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    /// A key and its modifiers, matched as egui's `consume_shortcut` matches them: an extra
    /// Shift or Alt aside, and the system's repeat of a key held down too.
    Key(KeyboardShortcut),
    /// A key with exactly its modifiers (so Shift+H and Alt+H stay free for other
    /// commands), freshly pressed, never the system's repeat of one held down.
    Exact(KeyboardShortcut),
    /// The pointer's gesture, with its modifiers held: descriptive only, for the sheet. The
    /// viewport takes the gesture itself; [`take`] never matches it.
    Pointer(Modifiers, &'static str),
}

/// A command as the registry holds it.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub command: Command,
    /// Its name, as its button, its menu item, and the sheet say it.
    pub label: &'static str,
    pub group: Group,
    /// Every chord that reaches it, the first the one shown (but [`Entry::elsewhere`]).
    pub keys: &'static [Chord],
    /// Off macOS, the chord shown in place of the first, if another is (Redo's Ctrl+Y,
    /// where Shift+Command+Z is macOS's).
    pub elsewhere: Option<Chord>,
    /// What it does, in a line, if more than its name says.
    pub description: Option<&'static str>,
}

const fn key(modifiers: Modifiers, key: Key) -> Chord {
    Chord::Key(KeyboardShortcut::new(modifiers, key))
}

const fn command(key: Key) -> Chord {
    Chord::Key(KeyboardShortcut::new(Modifiers::COMMAND, key))
}

const fn bare(key: Key) -> Chord {
    Chord::Key(KeyboardShortcut::new(Modifiers::NONE, key))
}

const SHIFT_COMMAND: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);

/// Esc: the nearest thing first (DESIGN.md, *Keyboard*, §3), which several commands share.
const ESCAPE: Chord = bare(Key::Escape);

const fn entry(
    command: Command,
    label: &'static str,
    group: Group,
    keys: &'static [Chord],
    description: Option<&'static str>,
) -> Entry {
    Entry {
        command,
        label,
        group,
        keys,
        elsewhere: None,
        description,
    }
}

const NEW: Entry = entry(
    Command::New,
    "New part",
    Group::File,
    &[command(Key::N)],
    Some("Opens an untitled part"),
);
const OPEN: Entry = entry(
    Command::Open,
    "Open…",
    Group::File,
    &[command(Key::O)],
    None,
);
const SAVE: Entry = entry(
    Command::Save,
    "Save",
    Group::File,
    &[command(Key::S)],
    Some("Writes your edits to the file"),
);
const SAVE_AS: Entry = entry(
    Command::SaveAs,
    "Save As…",
    Group::File,
    &[key(SHIFT_COMMAND, Key::S)],
    None,
);
const CLOSE: Entry = entry(
    Command::Close,
    "Close",
    Group::File,
    &[command(Key::W)],
    None,
);
const UNDO: Entry = entry(Command::Undo, "Undo", Group::Edit, &[command(Key::Z)], None);
/// Shift+Command+Z first: egui would match Command+Z with its Shift aside, so it's taken
/// before Undo's, as Redo, never an Undo.
const REDO: Entry = Entry {
    elsewhere: Some(command(Key::Y)),
    ..entry(
        Command::Redo,
        "Redo",
        Group::Edit,
        &[key(SHIFT_COMMAND, Key::Z), command(Key::Y)],
        None,
    )
};
const REBUILD: Entry = entry(
    Command::Rebuild,
    "Rebuild",
    Group::Rebuild,
    &[command(Key::B)],
    Some("Resumes a stopped rebuild"),
);
const REBUILD_ALL: Entry = entry(
    Command::RebuildAll,
    "Rebuild everything",
    Group::Rebuild,
    &[key(SHIFT_COMMAND, Key::B)],
    Some("Every feature, ignoring the cache"),
);
const STOP: Entry = entry(
    Command::Stop,
    "Stop rebuild",
    Group::Rebuild,
    &[ESCAPE],
    None,
);
const FIT: Entry = entry(
    Command::Fit,
    "Fit",
    Group::View,
    &[bare(Key::F6)],
    Some("The whole model in view"),
);
const HOME: Entry = entry(
    Command::Home,
    "Home view",
    Group::View,
    &[bare(Key::Home)],
    None,
);
const FRONT: Entry = entry(
    Command::Front,
    "Front",
    Group::View,
    &[command(Key::Num1)],
    None,
);
const BACK: Entry = entry(
    Command::Back,
    "Back",
    Group::View,
    &[command(Key::Num2)],
    None,
);
const LEFT: Entry = entry(
    Command::Left,
    "Left",
    Group::View,
    &[command(Key::Num3)],
    None,
);
const RIGHT: Entry = entry(
    Command::Right,
    "Right",
    Group::View,
    &[command(Key::Num4)],
    None,
);
const TOP: Entry = entry(
    Command::Top,
    "Top",
    Group::View,
    &[command(Key::Num5)],
    None,
);
const BOTTOM: Entry = entry(
    Command::Bottom,
    "Bottom",
    Group::View,
    &[command(Key::Num6)],
    None,
);
const ISOMETRIC: Entry = entry(
    Command::Isometric,
    "Isometric",
    Group::View,
    &[command(Key::Num7)],
    None,
);
const ORBIT: Entry = entry(
    Command::Orbit,
    "Orbit",
    Group::View,
    &[Chord::Pointer(Modifiers::SHIFT, "Middle-drag")],
    None,
);
const PAN: Entry = entry(
    Command::Pan,
    "Pan",
    Group::View,
    &[Chord::Pointer(Modifiers::NONE, "Middle-drag")],
    None,
);
const ZOOM: Entry = entry(
    Command::Zoom,
    "Zoom to the pointer",
    Group::View,
    &[Chord::Pointer(Modifiers::NONE, "Wheel")],
    None,
);
const CLEAR_SELECTION: Entry = entry(
    Command::ClearSelection,
    "Clear selection",
    Group::Selection,
    &[ESCAPE],
    None,
);
/// The selection filters (the prototype's `filterFace`, `filterEdge`, `filterVertex`;
/// SolidWorks' X, E, and V, docs/ux/solidworks.md §1), each toggling its kind in the set
/// (M5.0, open item 24), as SELECT's buttons do: exactly the key, freshly pressed, so a key
/// held down doesn't flicker the filter, and Shift or Alt with it stay free. Outside
/// sketches only (the group's note): sketch mode's X is Construction.
const FILTER_FACE: Entry = entry(
    Command::FilterFace,
    "Faces",
    Group::Selection,
    &[Chord::Exact(KeyboardShortcut::new(Modifiers::NONE, Key::X))],
    Some("Toggles whether faces can be selected"),
);
const FILTER_EDGE: Entry = entry(
    Command::FilterEdge,
    "Edges",
    Group::Selection,
    &[Chord::Exact(KeyboardShortcut::new(Modifiers::NONE, Key::E))],
    Some("Toggles whether edges can be selected"),
);
/// Vertices aren't picked yet (M5.7's part 3): the filter is kept, and matters once they
/// are.
const FILTER_VERTEX: Entry = entry(
    Command::FilterVertex,
    "Vertices",
    Group::Selection,
    &[Chord::Exact(KeyboardShortcut::new(Modifiers::NONE, Key::V))],
    Some("Toggles whether vertices can be selected"),
);
const HIDE: Entry = entry(
    Command::Hide,
    "Hide or show",
    Group::Selection,
    &[Chord::Exact(KeyboardShortcut::new(Modifiers::NONE, Key::H))],
    Some("The selected body, sketch, or plane"),
);
const OK: Entry = entry(Command::Ok, "OK", Group::Dialogs, &[bare(Key::Enter)], None);
const CANCEL: Entry = entry(Command::Cancel, "Cancel", Group::Dialogs, &[ESCAPE], None);
/// A sketch tool's letter: only the letter, freshly pressed (Shift or Alt with it is another
/// key's, and a letter held down chooses once).
const fn letter(key: Key) -> Chord {
    Chord::Exact(KeyboardShortcut::new(Modifiers::NONE, key))
}

const LINE: Entry = entry(
    Command::Line,
    "Line",
    Group::Sketch,
    &[letter(Key::L)],
    Some("Click, click: a chain of lines. Double-click ends it"),
);
const RECTANGLE: Entry = entry(
    Command::Rectangle,
    "Rectangle",
    Group::Sketch,
    &[letter(Key::R)],
    None,
);
const CIRCLE: Entry = entry(
    Command::Circle,
    "Circle",
    Group::Sketch,
    &[letter(Key::C)],
    None,
);
const ARC: Entry = entry(Command::Arc, "Arc", Group::Sketch, &[letter(Key::A)], None);
const POINT: Entry = entry(
    Command::Point,
    "Point",
    Group::Sketch,
    &[letter(Key::P)],
    None,
);
const TRIM: Entry = entry(
    Command::Trim,
    "Trim",
    Group::Sketch,
    &[letter(Key::T)],
    None,
);
const DIMENSION: Entry = entry(
    Command::Dimension,
    "Smart Dimension",
    Group::Sketch,
    &[letter(Key::D)],
    Some("Pick a line, a circle, or two entities, then type the value"),
);
/// The palette's Construction. SEAM: the prototype's X toggles the selected curves between
/// profile and construction, once a selection's curves can be set so.
const CONSTRUCTION: Entry = entry(
    Command::Construction,
    "Construction",
    Group::Sketch,
    &[letter(Key::X)],
    Some("Draws the tools' curves as construction, or as profile again"),
);
const FINISH_SKETCH: Entry = entry(
    Command::FinishSketch,
    "Finish Sketch",
    Group::Sketch,
    &[command(Key::Enter)],
    Some("Keeps your changes and rebuilds"),
);
const SHORTCUTS: Entry = entry(
    Command::Shortcuts,
    "Keyboard shortcuts",
    Group::Help,
    &[bare(Key::Questionmark)],
    None,
);

/// Every command, in the order the sheet lists each group's.
pub const REGISTRY: [Entry; 39] = [
    NEW,
    OPEN,
    SAVE,
    SAVE_AS,
    CLOSE,
    UNDO,
    REDO,
    REBUILD,
    REBUILD_ALL,
    STOP,
    FIT,
    HOME,
    FRONT,
    BACK,
    LEFT,
    RIGHT,
    TOP,
    BOTTOM,
    ISOMETRIC,
    ORBIT,
    PAN,
    ZOOM,
    FILTER_FACE,
    FILTER_EDGE,
    FILTER_VERTEX,
    CLEAR_SELECTION,
    HIDE,
    OK,
    CANCEL,
    LINE,
    RECTANGLE,
    CIRCLE,
    ARC,
    POINT,
    TRIM,
    DIMENSION,
    CONSTRUCTION,
    FINISH_SKETCH,
    SHORTCUTS,
];

impl Command {
    /// Its entry in the registry.
    pub fn entry(self) -> &'static Entry {
        match self {
            Command::New => &NEW,
            Command::Open => &OPEN,
            Command::Save => &SAVE,
            Command::SaveAs => &SAVE_AS,
            Command::Close => &CLOSE,
            Command::Undo => &UNDO,
            Command::Redo => &REDO,
            Command::Rebuild => &REBUILD,
            Command::RebuildAll => &REBUILD_ALL,
            Command::Stop => &STOP,
            Command::Fit => &FIT,
            Command::Home => &HOME,
            Command::Front => &FRONT,
            Command::Back => &BACK,
            Command::Left => &LEFT,
            Command::Right => &RIGHT,
            Command::Top => &TOP,
            Command::Bottom => &BOTTOM,
            Command::Isometric => &ISOMETRIC,
            Command::Orbit => &ORBIT,
            Command::Pan => &PAN,
            Command::Zoom => &ZOOM,
            Command::ClearSelection => &CLEAR_SELECTION,
            Command::FilterFace => &FILTER_FACE,
            Command::FilterEdge => &FILTER_EDGE,
            Command::FilterVertex => &FILTER_VERTEX,
            Command::Hide => &HIDE,
            Command::Ok => &OK,
            Command::Cancel => &CANCEL,
            Command::Line => &LINE,
            Command::Rectangle => &RECTANGLE,
            Command::Circle => &CIRCLE,
            Command::Arc => &ARC,
            Command::Point => &POINT,
            Command::Trim => &TRIM,
            Command::Dimension => &DIMENSION,
            Command::Construction => &CONSTRUCTION,
            Command::FinishSketch => &FINISH_SKETCH,
            Command::Shortcuts => &SHORTCUTS,
        }
    }

    /// Its name.
    pub fn label(self) -> &'static str {
        self.entry().label
    }

    /// The chord shown for it on `ctx`'s platform, if it has one.
    pub fn shown(self, ctx: &egui::Context) -> Option<Chord> {
        let entry = self.entry();
        match entry.elsewhere {
            Some(other) if ctx.os() != egui::os::OperatingSystem::Mac => Some(other),
            _ => entry.keys.first().copied(),
        }
    }

    /// The caps of its shown chord, as the platform names them: none if it has none.
    pub fn caps(self, ctx: &egui::Context) -> Vec<String> {
        self.shown(ctx)
            .map(|chord| caps(ctx, chord))
            .unwrap_or_default()
    }

    /// Its shown chord's caps as one text (`Ctrl+B`, `⌘B` on macOS), as a menu item gives
    /// it.
    pub fn keys_text(self, ctx: &egui::Context) -> String {
        joined(ctx, &self.caps(ctx))
    }
}

/// `caps` as one text: run together on macOS (`⌘⇧B`), and joined by `+` elsewhere
/// (`Ctrl+Shift+B`).
pub fn joined(ctx: &egui::Context, caps: &[String]) -> String {
    if ctx.os() == egui::os::OperatingSystem::Mac {
        caps.concat()
    } else {
        caps.join("+")
    }
}

/// What a key's cap says where the prototype names it otherwise than egui (its `KEYNAME`):
/// Esc, and ↵ for Enter.
fn key_name(key: Key) -> &'static str {
    match key {
        Key::Escape => "Esc",
        Key::Enter => "↵",
        key => key.symbol_or_name(),
    }
}

/// A cap for each of `chord`'s modifiers, Command (Ctrl off macOS) first, then Shift, then
/// its key's or its gesture's, as the platform names them.
pub fn caps(ctx: &egui::Context, chord: Chord) -> Vec<String> {
    let (modifiers, last) = match chord {
        Chord::Key(shortcut) | Chord::Exact(shortcut) => {
            (shortcut.modifiers, key_name(shortcut.logical_key))
        }
        Chord::Pointer(modifiers, gesture) => (modifiers, gesture),
    };
    let mut caps = start::modifier_caps(ctx, modifiers);
    caps.push(last.to_owned());
    caps
}

/// Whether `command` was pressed this frame by any of its chords: each taken, so no widget
/// after reads it again.
pub fn take(ui: &mut Ui, command: Command) -> bool {
    let mut taken = false;
    for chord in command.entry().keys {
        taken |= take_chord(ui, *chord);
    }
    taken
}

/// Whether `chord` was pressed this frame, taking it if so.
fn take_chord(ui: &mut Ui, chord: Chord) -> bool {
    match chord {
        Chord::Key(shortcut) => ui.input_mut(|input| input.consume_shortcut(&shortcut)),
        Chord::Exact(shortcut) => take_exact(ui, shortcut),
        Chord::Pointer(..) => false,
    }
}

/// Whether `shortcut` was pressed this frame with exactly its modifiers (egui's own match
/// ignores Shift and Alt), and freshly, never the system's repeat of a key held down:
/// taking it if so.
fn take_exact(ui: &mut Ui, shortcut: KeyboardShortcut) -> bool {
    ui.input_mut(|input| {
        let mut pressed = false;
        input.events.retain(|event| {
            let fresh = matches!(
                event,
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if *key == shortcut.logical_key && *modifiers == shortcut.modifiers
            );
            pressed |= fresh;
            !fresh
        });
        pressed
    })
}

/// Whether the filter `query` finds `entry`, its case aside: in its name, its description,
/// or its shown chord's caps, run together or joined by `+` (`ctrl+b`, `⌘b`, `f6`). An
/// empty query (spaces aside) finds every entry.
pub fn finds(ctx: &egui::Context, entry: &Entry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let caps = entry.command.caps(ctx);
    [
        entry.label.to_lowercase(),
        entry.description.unwrap_or_default().to_lowercase(),
        caps.concat().to_lowercase(),
        caps.join("+").to_lowercase(),
    ]
    .iter()
    .any(|text| text.contains(&query))
}

#[cfg(test)]
mod tests {
    use eframe::egui::os::OperatingSystem;

    use super::*;

    fn on(os: OperatingSystem) -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_os(os);
        ctx
    }

    #[test]
    fn every_command_is_in_the_registry_once_under_its_own_entry() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in &REGISTRY {
            assert!(seen.insert(entry.command), "{:?} twice", entry.command);
            assert_eq!(entry.command.entry().command, entry.command);
            assert_eq!(entry.command.entry().label, entry.label);
        }
        // Each group's entries stand together, in the groups' order.
        let groups: Vec<Group> = REGISTRY.iter().map(|entry| entry.group).collect();
        assert!(
            groups.windows(2).all(|pair| pair[0] <= pair[1]),
            "{groups:?}"
        );
    }

    #[test]
    fn no_two_commands_share_a_key_but_escs_nearest_thing_first() {
        // DESIGN.md, *Keyboard*: Esc stops a dialog's preview, then cancels the dialog, then
        // clears the selection, then stops the rebuild: one key, its order the app's.
        let mut keys: Vec<(KeyboardShortcut, Command)> = Vec::new();
        for entry in &REGISTRY {
            for chord in entry.keys {
                if let Chord::Key(shortcut) | Chord::Exact(shortcut) = chord {
                    keys.push((*shortcut, entry.command));
                }
            }
        }
        for (at, (shortcut, command)) in keys.iter().enumerate() {
            for (other, by) in &keys[at + 1..] {
                if shortcut == other {
                    assert_eq!(shortcut.logical_key, Key::Escape, "{command:?} and {by:?}");
                }
            }
        }
        let escape: Vec<Command> = keys
            .iter()
            .filter(|(shortcut, _)| shortcut.logical_key == Key::Escape)
            .map(|(_, command)| *command)
            .collect();
        assert_eq!(
            escape,
            [Command::Stop, Command::ClearSelection, Command::Cancel]
        );
    }

    #[test]
    fn the_standard_views_are_solidworks_numbers_and_f6_fits() {
        // docs/design/m5-ui/DESIGN.md, *Keyboard* ("⌘1–⌘7 standard views (SolidWorks'
        // numbering)"), and js/app.js's `CMD`: Ctrl+1 to Ctrl+7 front, back, left, right,
        // top, bottom, isometric.
        for (command, digit) in [
            (Command::Front, Key::Num1),
            (Command::Back, Key::Num2),
            (Command::Left, Key::Num3),
            (Command::Right, Key::Num4),
            (Command::Top, Key::Num5),
            (Command::Bottom, Key::Num6),
            (Command::Isometric, Key::Num7),
        ] {
            assert_eq!(
                command.entry().keys,
                [Chord::Key(KeyboardShortcut::new(Modifiers::COMMAND, digit))]
            );
        }
        let ctx = on(OperatingSystem::Nix);
        assert_eq!(Command::Fit.caps(&ctx), ["F6"]);
        assert_eq!(Command::Home.caps(&ctx), ["Home"]);
        assert_eq!(Command::RebuildAll.caps(&ctx), ["Ctrl", "Shift", "B"]);
        assert_eq!(Command::Hide.caps(&ctx), ["H"]);
        assert_eq!(Command::Shortcuts.caps(&ctx), ["?"]);
    }

    #[test]
    fn the_sketch_group_is_the_prototypes_letters_and_ctrl_enter_between_dialogs_and_help() {
        // js/app.js's `CMD`: sLine L, sRect R, sCircle C, sArc A, sPoint P, sTrim T, sDim D,
        // sConstruction X, finishSketch Mod Enter, all `group: 'Sketch'`; its sheet's `order`
        // puts Sketch after Dialogs, before Help, noted "while a sketch is open".
        let sketch: Vec<(Command, &[Chord])> = REGISTRY
            .iter()
            .filter(|entry| entry.group == Group::Sketch)
            .map(|entry| (entry.command, entry.keys))
            .collect();
        let exact = |key| Chord::Exact(KeyboardShortcut::new(Modifiers::NONE, key));
        assert_eq!(
            sketch,
            [
                (Command::Line, &[exact(Key::L)][..]),
                (Command::Rectangle, &[exact(Key::R)]),
                (Command::Circle, &[exact(Key::C)]),
                (Command::Arc, &[exact(Key::A)]),
                (Command::Point, &[exact(Key::P)]),
                (Command::Trim, &[exact(Key::T)]),
                (Command::Dimension, &[exact(Key::D)]),
                (Command::Construction, &[exact(Key::X)]),
                (
                    Command::FinishSketch,
                    &[Chord::Key(KeyboardShortcut::new(
                        Modifiers::COMMAND,
                        Key::Enter
                    ))]
                ),
            ]
        );
        let at = |group| GROUPS.iter().position(|each| *each == group);
        assert_eq!(at(Group::Sketch), Some(6));
        assert_eq!(at(Group::Dialogs), Some(5));
        assert_eq!(at(Group::Help), Some(7));
        assert_eq!(Group::Sketch.name(), "Sketch");
        assert_eq!(Group::Sketch.note(), Some("while a sketch is open"));
        let mac = on(OperatingSystem::Mac);
        let nix = on(OperatingSystem::Nix);
        assert_eq!(Command::FinishSketch.caps(&mac), ["⌘", "↵"]);
        assert_eq!(Command::FinishSketch.keys_text(&nix), "Ctrl+↵");
        assert_eq!(Command::Dimension.caps(&nix), ["D"]);
        assert_eq!(Command::Dimension.label(), "Smart Dimension");
    }

    #[test]
    fn caps_are_the_prototypes_keynames_on_each_platform() {
        // app.js's KEYNAME: Mod ⌘ on macOS and Ctrl elsewhere, Shift ⇧ or Shift, Enter ↵,
        // Esc; Redo's Shift+Mod+Z on macOS, Mod+Y elsewhere.
        let mac = on(OperatingSystem::Mac);
        let nix = on(OperatingSystem::Nix);
        let windows = on(OperatingSystem::Windows);
        assert_eq!(Command::SaveAs.caps(&mac), ["⌘", "⇧", "S"]);
        assert_eq!(Command::SaveAs.caps(&nix), ["Ctrl", "Shift", "S"]);
        assert_eq!(Command::Redo.caps(&mac), ["⌘", "⇧", "Z"]);
        assert_eq!(Command::Redo.caps(&nix), ["Ctrl", "Y"]);
        assert_eq!(Command::Redo.caps(&windows), ["Ctrl", "Y"]);
        assert_eq!(Command::Ok.caps(&nix), ["↵"]);
        assert_eq!(Command::Cancel.caps(&mac), ["Esc"]);
        assert_eq!(Command::Orbit.caps(&mac), ["⇧", "Middle-drag"]);
        assert_eq!(Command::Orbit.caps(&nix), ["Shift", "Middle-drag"]);
        assert_eq!(Command::Zoom.caps(&nix), ["Wheel"]);
        assert_eq!(Command::Front.keys_text(&mac), "⌘1");
        assert_eq!(Command::Front.keys_text(&nix), "Ctrl+1");
        assert_eq!(Command::Redo.shown(&nix), Some(command(Key::Y)));
        assert_eq!(Command::Redo.shown(&mac), Some(key(SHIFT_COMMAND, Key::Z)));
    }

    #[test]
    fn the_filter_finds_by_name_description_or_key_its_case_aside() {
        let nix = on(OperatingSystem::Nix);
        let mac = on(OperatingSystem::Mac);
        let found = |ctx: &egui::Context, query: &str| -> Vec<Command> {
            REGISTRY
                .iter()
                .filter(|entry| finds(ctx, entry, query))
                .map(|entry| entry.command)
                .collect()
        };
        assert_eq!(found(&nix, "").len(), REGISTRY.len());
        assert_eq!(found(&nix, "  ").len(), REGISTRY.len());
        assert_eq!(found(&nix, "SAVE"), [Command::Save, Command::SaveAs]);
        assert_eq!(found(&nix, "f6"), [Command::Fit]);
        assert_eq!(found(&nix, "ctrl+b"), [Command::Rebuild]);
        assert_eq!(found(&nix, "shift+b"), [Command::RebuildAll]);
        // Command first, then Shift, as every cap is ordered (the prototype's `Mod Shift`).
        assert_eq!(found(&mac, "⌘b"), [Command::Rebuild]);
        assert_eq!(found(&mac, "⌘⇧b"), [Command::RebuildAll]);
        assert_eq!(found(&nix, "cache"), [Command::RebuildAll]);
        assert_eq!(found(&nix, "middle-drag"), [Command::Orbit, Command::Pan]);
        assert!(found(&nix, "no such thing").is_empty());
    }
}
