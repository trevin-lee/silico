//! The app driven as a person drives it, on either kernel ([`AppKernel`]): each pane and
//! control found by its role and label in the accessibility tree and clicked or typed into,
//! keys pressed, and the canvas given the pointer's own events; and reads of what the window
//! and the tab show, which act on nothing. Every frame is stepped on a thread marked so that
//! a wait on a command there is counted. Last, the stand-ins more than one file uses: the
//! native file dialog ([`FakeBrowser`]), the fake kernel's meshes counted, a file's bytes
//! read plainly, and a clock moved by hand.
//!
//! tests/app.rs, the hostile-input tests, and the slice take from here the helpers that more
//! than one of them had a copy of, the ones those call, and the twins below. How long a wait
//! may take follows the kernel the app runs on: [`FAKE_WAIT`] on the fake kernel,
//! [`SLICE_WAIT`] on the real workers.
//!
//! Three clicks and two ways of typing over a field do one job differently, and are kept
//! apart because the events each sends are what its tests have checked the app against:
//! - [`click_in_one_frame`] gives the press and the release in one frame, as the harness's
//!   own clicks do; [`click_over_two_frames`] gives the press, the keys it's given held and
//!   all, in the pointer's first frame there and the release in the next;
//!   [`click_as_a_person`] waits out egui's double-click delay first, so that no click is
//!   read as another's second, and gives the move, the press, and the release a frame each;
//! - [`retype`] takes a field's characters away a Backspace a frame and reads `text` back at
//!   once; [`type_over`] takes them in one frame (Home, and a Delete each) and waits for the
//!   field to read `text`. Typing after what a field holds is [`type_in`]'s alone.
//!
//! Each fixture on the fake kernel starts from [`stand_in_sessions`], and the documents more
//! than one file opens are here ([`EXAMPLE`], [`SKETCHED_EXAMPLE`], [`TWO_BOXES`],
//! [`A_BOX`]). What stays in each file is its own, not a copy:
//! - each file's `fixture` and `Fixture`, since each layers on that base what its tests need
//!   (app.rs's write STEP through the fake kernel, and compare the disk through a test's
//!   reader when one is given; adversarial_undo_names.rs's compare it through
//!   [`plain_read`] with no interval, and write no STEP; the untitled and viewport tests' are
//!   the base alone, the viewport's watching the disk when asked) and has its own methods
//!   for opening the app;
//! - app.rs's and adversarial_viewport.rs's renderer stand-ins (`Told`, `FakeRenderer`,
//!   `Recorder`), which record different things and answer picks differently: app.rs's give
//!   the pick a test sets, or hold, drop, or never give it; the viewport's give the
//!   background's every time, and count frames drawn with no tab;
//! - adversarial_untitled.rs's `add_sketch` finishes its sketch without waiting for the tab's
//!   asterisk, where app.rs's (`finish_sketch_unsaved`) waits for it;
//! - helpers that share a name but not a job: app.rs's `renamed` (the name typed and its edit
//!   landed) and adversarial_undo_names.rs's (the feature selected, the name typed, and
//!   everything settled); app.rs's and slice.rs's `fully_defined` and `seen`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::accesskit::Role;
use eframe::egui::{
    Color32, CursorIcon, Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Shape,
    ViewportCommand,
};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use silico_app::App;
use silico_app::browse::{Answer, Ask, Browse};
use silico_core::cache::Cache;
use silico_core::command::Sessions;
use silico_core::disk::{Clock, Found};
use silico_core::document::FeatureId;
use silico_core::document::json::{self, Plain};
use silico_core::fake::{FakeKernel, FakeSketches, never_waits};
use silico_core::kernel::{Clients, Op};
use silico_core::pool::KernelPool;
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};
use silico_protocol::{Camera, Projection};
use tempfile::TempDir;

/// The app in a harness.
pub type AppUi<P> = Harness<'static, App<P>>;

/// Long enough for anything the fake kernel does on a loaded machine.
pub const FAKE_WAIT: Duration = Duration::from_secs(60);

/// Long enough for anything a real worker does on a loaded machine, a first start included:
/// longer than [`FAKE_WAIT`], since in the slice every answer is a real kernel's, sketch
/// worker's, or STEP worker's, and every frame is drawn on lavapipe.
pub const SLICE_WAIT: Duration = Duration::from_secs(120);

/// A kernel the app is driven on here, and how long [`step_until`] waits on it.
pub trait AppKernel: Clients + 'static {
    /// The longest a wait for the app's answers may take.
    const WAIT: Duration;
}

/// The fake kernel, which app.rs and the hostile-input tests drive the app on.
impl AppKernel for Arc<FakeKernel> {
    const WAIT: Duration = FAKE_WAIT;
}

/// The real kernel's pool, which the slice drives the app on.
impl AppKernel for KernelPool {
    const WAIT: Duration = SLICE_WAIT;
}

/// One frame, on the marked thread.
pub fn step<P: AppKernel>(h: &mut AppUi<P>) {
    let _marked = never_waits::marked();
    h.step();
}

/// Frames stepped until `done` holds, within the kernel's [`AppKernel::WAIT`].
#[track_caller]
pub fn step_until<P: AppKernel>(h: &mut AppUi<P>, what: &str, done: impl Fn(&AppUi<P>) -> bool) {
    let deadline = Instant::now() + P::WAIT;
    loop {
        step(h);
        if done(h) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "never {what}; the window shows: {:?}; its tabs' messages: {:?}",
            everything(h),
            h.state()
                .tabs()
                .iter()
                .map(|tab| tab.message.clone())
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Every label and value the window holds, in the tree's order: what a failure says.
pub fn everything<P: AppKernel>(h: &AppUi<P>) -> Vec<String> {
    h.root()
        .query_all(egui_kittest::kittest::by().recursive(true))
        .filter_map(|node| {
            let node = node.accesskit_node();
            node.label().or_else(|| node.value())
        })
        .collect()
}

/// Whether the last frame told the window `command` (to close, or to cancel a close): in any
/// harness, the app's or another window's.
pub fn window_told<S>(h: &Harness<'_, S>, command: ViewportCommand) -> bool {
    h.output()
        .viewport_output
        .get(&eframe::egui::ViewportId::ROOT)
        .is_some_and(|viewport| viewport.commands.contains(&command))
}

/// Whether the window holds a node labelled `label`, anywhere.
pub fn anywhere<P: AppKernel>(h: &AppUi<P>, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

/// Stepped until nothing of the tab's is under way and its last answer's revision is read:
/// its edits, its steps, a dialog's add, and the timeline read at that revision
/// (`applying`), which answers a frame or more after them.
pub fn settled<P: AppKernel>(h: &mut AppUi<P>) {
    step_until(h, "everything answered", |h| {
        let tab = &h.state().tabs()[0];
        tab.edits.idle() && !tab.applying() && tab.dialog.as_ref().is_none_or(|d| !d.adding)
    });
}

// --- The app bar and the status bar --------------------------------------------------------

/// The pointer away from the app bar, over the viewport, a frame drawn.
pub fn pointer_away<P: AppKernel>(h: &mut AppUi<P>) {
    h.hover_at(Pos2::new(640.0, 400.0));
    step(h);
    step(h);
}

/// The title the app bar's `button` ("Undo" or "Redo") shows in its tooltip, the pointer
/// brought over it from over the canvas, where no tooltip shows, and where its label is: the
/// word alone, or the word, a colon, and the step it would take (the prototype's `data-tip`,
/// `'Undo' + (canUndo ? ': ' + label : '')`).
pub fn tooltip<P: AppKernel>(h: &mut AppUi<P>, button: &str) -> (String, Rect) {
    let named = format!("{button}: ");
    let title = |h: &AppUi<P>| {
        h.query_all_by_role(Role::Label).find_map(|node| {
            let rect = node.rect();
            let node = node.accesskit_node();
            let said = node.label().or_else(|| node.value())?;
            (said == button || said.starts_with(&named)).then_some((said, rect))
        })
    };
    pointer_away(h);
    assert_eq!(
        title(h).map(|(said, _)| said),
        None,
        "{button}'s tooltip shown with the pointer away"
    );
    h.get_by_role_and_label(Role::Button, button).hover();
    step_until(h, "its tooltip", |h| title(h).is_some());
    // Drawn from the frame after it's laid out (egui sizes a new popup unseen).
    step(h);
    title(h).unwrap()
}

/// [`tooltip`]'s title alone.
pub fn tooltip_title<P: AppKernel>(h: &mut AppUi<P>, button: &str) -> String {
    tooltip(h, button).0
}

/// Whether a label `words` is in the status bar: on the line of its Shortcuts button.
pub fn in_status_bar<P: AppKernel>(h: &AppUi<P>, words: &str) -> bool {
    let bar = h.get_by_role_and_label(Role::Button, "Shortcuts").rect();
    h.query_all_by_label(words)
        .any(|node| bar.y_range().contains(node.rect().center().y))
}

/// Command+Z pressed once Undo is on in the first tab, and its answer settled.
pub fn undo_by_key<P: AppKernel>(h: &mut AppUi<P>) {
    step_until(h, "undoable", |h| h.state().tabs()[0].undoable());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    step(h);
    settled(h);
}

/// Command+Y pressed once Redo is on in the first tab, and its answer settled.
pub fn redo_by_key<P: AppKernel>(h: &mut AppUi<P>) {
    step_until(h, "redoable", |h| h.state().tabs()[0].redoable());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Y);
    step(h);
    settled(h);
}

/// The window asked to close, as its close button asks.
pub fn close_window<P: AppKernel>(h: &mut AppUi<P>) {
    let root = eframe::egui::ViewportId::ROOT;
    h.input_mut()
        .viewports
        .get_mut(&root)
        .unwrap()
        .events
        .push(eframe::egui::ViewportEvent::Close);
    step(h);
    h.input_mut()
        .viewports
        .get_mut(&root)
        .unwrap()
        .events
        .clear();
}

/// The delay the last frame asked for the next one at.
pub fn repaint_delay<P: AppKernel>(h: &AppUi<P>) -> Option<Duration> {
    h.output()
        .viewport_output
        .get(&eframe::egui::ViewportId::ROOT)
        .map(|viewport| viewport.repaint_delay)
}

// --- Files ---------------------------------------------------------------------------------

/// Command+N pressed, and the new part's tab waited for.
pub fn new_part<P: AppKernel>(h: &mut AppUi<P>) {
    let before = h.state().tabs().len();
    h.key_press_modifiers(Modifiers::COMMAND, Key::N);
    step_until(h, "the untitled part's tab", |h| {
        h.state().tabs().len() == before + 1
    });
}

/// File > Save As… of the tab named `name`, `path` typed into its dialog, and Save.
pub fn save_as_of<P: AppKernel>(h: &mut AppUi<P>, name: &str, path: &Path) {
    let title = format!("Save {name} as");
    h.get_by_label("File").click();
    step(h);
    step(h);
    h.get_by_role_and_label(Role::Button, "Save As…").click();
    step_until(h, "the Save As dialog", |h| {
        h.query_by_label_contains(&title).is_some()
    });
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text(path.to_str().unwrap());
    step(h);
    // The dialog's, after the app bar's.
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    step_until(h, "the Save As dialog gone", |h| {
        h.query_by_label_contains(&title).is_none()
    });
}

/// The tab named `name`.
pub fn tab_named<'a, P: AppKernel>(h: &'a AppUi<P>, name: &str) -> &'a silico_app::tab::Tab {
    h.state()
        .tabs()
        .iter()
        .find(|tab| tab.name() == name)
        .unwrap_or_else(|| panic!("no tab {name}"))
}

/// The question's button `label`, the last of that label (after the app bar's Save).
pub fn answer_question<P: AppKernel>(h: &mut AppUi<P>, label: &str) {
    h.query_all_by_role_and_label(Role::Button, label)
        .last()
        .unwrap()
        .click_accesskit();
    step(h);
}

// --- The export dialog ---------------------------------------------------------------------

/// File > Export…, its dialog open, titled `title` ("Export bracket.silico").
pub fn open_export<P: AppKernel>(h: &mut AppUi<P>, title: &str) {
    h.get_by_label("File").click();
    step(h);
    step(h);
    h.get_by_role_and_label(Role::Button, "Export…").click();
    step_until(h, "the export dialog", |h| anywhere(h, title));
}

/// Where a text field is found: anywhere in the window, in the feature dialog, or in the
/// node of `role` labelled `label` (the Modify box, say).
#[derive(Debug, Clone, Copy)]
pub enum In<'s> {
    Window,
    Dialog,
    Node(Role, &'s str),
}

/// The text field labelled `label`, where `within` says.
pub fn field<'a, P: AppKernel>(
    h: &'a AppUi<P>,
    within: In<'a>,
    label: &'a str,
) -> egui_kittest::Node<'a> {
    match within {
        In::Window => h.get_by_role_and_label(Role::TextInput, label),
        In::Dialog => dialog(h).get_by_role_and_label(Role::TextInput, label),
        In::Node(role, name) => h
            .get_by_role_and_label(role, name)
            .get_by_role_and_label(Role::TextInput, label),
    }
}

/// The text field labelled `label` (where `within` says) typed over with `text`, as a person
/// does it: focused if it isn't, then Home and a Delete for each character it holds, read
/// from the field itself, in one frame, and `text` typed in; stepped until it reads `text`.
pub fn type_over<P: AppKernel>(h: &mut AppUi<P>, within: In<'_>, label: &str, text: &str) {
    if !field(h, within, label).is_focused() {
        field(h, within, label).focus();
        step(h);
    }
    let held = field(h, within, label)
        .value()
        .unwrap_or_default()
        .chars()
        .count();
    let key = |key| Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    h.input_mut().events.push(key(Key::Home));
    for _ in 0..held {
        h.input_mut().events.push(key(Key::Delete));
    }
    step(h);
    field(h, within, label).type_text(text);
    step(h);
    step_until(h, "the field typed over", |h| {
        field(h, within, label).value().as_deref() == Some(text)
    });
}

/// The text field labelled `label` (where `within` says) emptied a Backspace a frame (its
/// cursor at its end once focused, a Backspace for each character it holds, read from the
/// field itself), then `text` typed at once, and the field read as `text` the next frame.
pub fn retype<P: AppKernel>(h: &mut AppUi<P>, within: In<'_>, label: &str, text: &str) {
    field(h, within, label).focus();
    step(h);
    let held = field(h, within, label).value().unwrap().chars().count();
    for _ in 0..held {
        h.key_press(Key::Backspace);
        step(h);
    }
    field(h, within, label).type_text(text);
    step(h);
    assert_eq!(field(h, within, label).value().as_deref(), Some(text));
}

/// The button `label` clicked, through the accessibility tree (a modal's layout moves as
/// its content changes, so a click at its last place can miss); drawn again as what it did
/// leaves it.
pub fn press<P: AppKernel>(h: &mut AppUi<P>, label: &str) {
    h.get_by_role_and_label(Role::Button, label)
        .click_accesskit();
    step(h);
    step(h);
}

// --- Feature dialogs -----------------------------------------------------------------------

/// The open dialog.
pub fn dialog<P: AppKernel>(h: &AppUi<P>) -> egui_kittest::Node<'_> {
    h.get_by_role(Role::Dialog)
}

/// Whether a dialog is open.
pub fn dialog_open<P: AppKernel>(h: &AppUi<P>) -> bool {
    h.query_by_role(Role::Dialog).is_some()
}

/// The dialog of `kind` opened from the toolbar's CREATE row (a sweep: Pad, Pocket).
pub fn create<P: AppKernel>(h: &mut AppUi<P>, kind: &str) {
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_label(kind)
        .click();
    step_until(h, "the dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, kind).is_some()
    });
}

/// MODIFY's `kind` clicked in its row, and its dialog open.
pub fn modify<P: AppKernel>(h: &mut AppUi<P>, kind: &str) {
    h.get_by_role_and_label(Role::Toolbar, "MODIFY")
        .get_by_label(kind)
        .click();
    step_until(h, "the dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, kind).is_some()
    });
}

/// Whether the dialog's choice `key`, a drop-down, shows `option` chosen.
pub fn chosen<P: AppKernel>(h: &AppUi<P>, key: &str, option: &str) -> bool {
    dialog(h)
        .get_by_role_and_label(Role::ComboBox, key)
        .accesskit_node()
        .value()
        .as_deref()
        == Some(option)
}

/// `option` chosen in the dialog's choice `key`, from its drop-down's menu.
pub fn choose<P: AppKernel>(h: &mut AppUi<P>, key: &str, option: &str) {
    dialog(h).get_by_role_and_label(Role::ComboBox, key).click();
    step_until(h, "the drop-down's menu", |h| {
        h.query_all_by_role_and_label(Role::Button, option).count() == 1
    });
    // egui lays a new menu out unseen in its first frame, and places it in its next.
    step(h);
    h.get_by_role_and_label(Role::Button, option).click();
    step(h);
    step(h);
    assert!(chosen(h, key, option), "{option} not chosen in {key}");
}

/// The dialog's checkbox labelled `label` clicked (dialog.rs's `Value::Toggle`, named as its
/// row's key).
pub fn tick<P: AppKernel>(h: &mut AppUi<P>, label: &str) {
    dialog(h)
        .get_by_role_and_label(Role::CheckBox, label)
        .click();
    step(h);
}

/// Whether the dialog's OK is on.
pub fn ok_enabled<P: AppKernel>(h: &AppUi<P>) -> bool {
    !dialog(h)
        .get_by_role_and_label(Role::Button, "OK")
        .accesskit_node()
        .is_disabled()
}

/// `pane`'s node labelled `label` double-clicked: both clicks in one frame, as the
/// harness's own clicks each take frames of their own, a quarter second apart, which egui
/// counts as two single clicks.
pub fn double_click<P: AppKernel>(h: &mut AppUi<P>, pane: &str, label: &str) {
    let pos = h
        .get_by_role_and_label(Role::Pane, pane)
        .get_by_label(label)
        .rect()
        .center();
    h.input_mut().events.push(Event::PointerMoved(pos));
    step(h);
    for pressed in [true, false, true, false] {
        h.input_mut().events.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        });
    }
    step(h);
}

/// The dialog's feature's add answered.
pub fn added<P: AppKernel>(h: &mut AppUi<P>) {
    step_until(h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
}

/// The names the open dialog's box of names holds.
pub fn boxed<P: AppKernel>(h: &AppUi<P>) -> Vec<String> {
    h.state().tabs()[0]
        .dialog
        .as_ref()
        .and_then(|dialog| dialog.names())
        .map(|names| names.names.clone())
        .unwrap_or_default()
}

// --- The timeline and the browser ----------------------------------------------------------

/// The timeline's item `name`, hovered at its centre, and two frames drawn: the timeline
/// draws before the canvas, so the frame after the pointer's move draws what it links (the
/// faces its feature made, tinted).
pub fn hover_item<P: AppKernel>(h: &mut AppUi<P>, name: &str) {
    let at = h
        .get_by_role_and_label(Role::Pane, "Timeline")
        .get_by_label(name)
        .rect()
        .center();
    h.hover_at(at);
    step(h);
    step(h);
}

/// Every face of body `number`, by its number in the view's table, in order.
pub fn every_face<P: AppKernel>(
    h: &AppUi<P>,
    number: u32,
) -> Vec<(u32, silico_app::renderer::Element)> {
    let count = view(h).bodies()[&number].faces.len();
    (1..=u32::try_from(count).unwrap())
        .map(|n| (number, silico_app::renderer::Element::Face(n)))
        .collect()
}

/// The faces of body `number` that feature `feature` made, as each face's name's text says
/// (it starts with the feature's ID and a dot), in order: read from the text, not as the
/// viewport reads a name.
pub fn faces_made_by<P: AppKernel>(
    h: &AppUi<P>,
    number: u32,
    feature: &str,
) -> Vec<(u32, silico_app::renderer::Element)> {
    let prefix = format!("{feature}.");
    view(h).bodies()[&number]
        .faces
        .iter()
        .zip(1u32..)
        .filter(|(face, _)| face.name.to_string().starts_with(&prefix))
        .map(|(_, n)| (number, silico_app::renderer::Element::Face(n)))
        .collect()
}

/// The body whose feature is `id`, by its number in the view's table.
pub fn body_of<P: AppKernel>(h: &AppUi<P>, id: &str) -> u32 {
    view(h)
        .bodies()
        .iter()
        .find(|(_, body)| body.feature.as_str() == id)
        .map(|(number, _)| *number)
        .unwrap()
}

/// The feature `name` selected on the timeline, its properties shown.
pub fn select<P: AppKernel>(h: &mut AppUi<P>, name: &str) {
    h.get_by_role_and_label(Role::Pane, "Timeline")
        .get_by_label(name)
        .click();
    step_until(h, "its properties", |h| {
        h.get_by_role_and_label(Role::Pane, "Properties")
            .query_by_role(Role::TextInput)
            .and_then(|field| field.value())
            .is_some_and(|value| value.ends_with(name))
    });
}

/// The property panel's Name field.
pub fn name_field<P: AppKernel>(h: &AppUi<P>) -> egui_kittest::Node<'_> {
    h.get_by_role_and_label(Role::Pane, "Properties")
        .get_by_role(Role::TextInput)
}

/// Typed into the text field labelled `label` (where `within` says), one keystroke a frame:
/// focused, the field's cursor is at its end, so what's typed comes after what it holds.
pub fn type_in<P: AppKernel>(h: &mut AppUi<P>, within: In<'_>, label: &str, text: &str) {
    field(h, within, label).focus();
    step(h);
    for c in text.chars() {
        field(h, within, label).type_text(&c.to_string());
        step(h);
    }
}

/// Typed into the property panel's Name field, one keystroke a frame, after the name
/// ([`type_in`]).
pub fn type_name<P: AppKernel>(h: &mut AppUi<P>, text: &str) {
    type_in(h, In::Node(Role::Pane, "Properties"), "Name", text);
}

/// The rename field of the feature `name`, if it's open.
pub fn rename_field<'a, P: AppKernel>(
    h: &'a AppUi<P>,
    name: &str,
) -> Option<egui_kittest::Node<'a>> {
    let label = format!("Rename {name}");
    h.query_all_by_role(Role::TextInput)
        .find(|node| node.accesskit_node().label().as_deref() == Some(label.as_str()))
}

/// The feature `name`'s rename field opened by F2, the feature selected on the timeline,
/// and `typed` typed over its name.
pub fn f2_typed<P: AppKernel>(h: &mut AppUi<P>, name: &str, typed: &str) {
    select(h, name);
    h.key_press(Key::F2);
    step_until(h, "the rename field", |h| {
        rename_field(h, name).is_some_and(|field| field.is_focused())
    });
    step(h);
    rename_field(h, name).unwrap().type_text(typed);
    step(h);
    assert_eq!(
        rename_field(h, name).unwrap().value().as_deref(),
        Some(typed),
        "typed over whole"
    );
}

// --- Sketch mode ---------------------------------------------------------------------------

/// Sketch mode's state, in the first tab.
pub fn sketch_mode<P: AppKernel>(h: &AppUi<P>) -> Option<&silico_app::sketch::SketchMode> {
    h.state().tabs()[0].sketch.as_ref()
}

/// Create Sketch on the plane labelled `plane` in the part `part`'s tab, once Create Sketch
/// is on: sketch mode open, its sketch added and read.
pub fn begin_sketch_on<P: AppKernel>(h: &mut AppUi<P>, part: &str, plane: &str) {
    step_until(h, "Create Sketch on", |h| {
        !h.get_by_role_and_label(Role::Toolbar, "CREATE")
            .get_by_label("Create Sketch")
            .accesskit_node()
            .is_disabled()
    });
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_label("Create Sketch")
        .click();
    step_until(h, "its planes", |h| {
        h.query_by_role_and_label(Role::Button, plane).is_some()
    });
    step(h);
    h.get_by_role_and_label(Role::Button, plane).click();
    step_until(h, "its sketch added and read", |h| {
        tab_named(h, part)
            .sketch
            .as_ref()
            .is_some_and(|mode| mode.sketch.is_some() && mode.drawable().is_some())
    });
}

/// Create Sketch from CREATE's menu, on the plane labelled `plane` ("XY plane"): sketch
/// mode open, and its sketch's add answered.
pub fn create_sketch<P: AppKernel>(h: &mut AppUi<P>, plane: &str) {
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_label("Create Sketch")
        .click();
    step(h);
    step(h);
    h.get_by_role_and_label(Role::Button, plane).click();
    step_until(h, "its sketch added", |h| {
        sketch_mode(h).is_some_and(|mode| mode.sketch.is_some())
    });
}

/// Stepped until sketch mode's newest read holds a sketch.
pub fn sketch_read<P: AppKernel>(h: &mut AppUi<P>) {
    step_until(h, "the sketch read", |h| {
        sketch_mode(h).is_some_and(|mode| mode.drawable().is_some())
    });
}

/// The sketch bar's Finish Sketch (`true`) or Cancel clicked, and sketch mode closed: once
/// the sketch is read and the bar laid out with its state (it's centred, so it moves as
/// that appears).
pub fn leave_sketch<P: AppKernel>(h: &mut AppUi<P>, finish: bool) {
    leave_sketch_unsettled(h, finish);
    settled(h);
}

/// [`leave_sketch`] but for what's left to answer after sketch mode closes.
pub fn leave_sketch_unsettled<P: AppKernel>(h: &mut AppUi<P>, finish: bool) {
    let word = if finish { "Finish Sketch" } else { "Cancel" };
    sketch_read(h);
    let place = |h: &AppUi<P>| {
        h.get_by_role_and_label(Role::Toolbar, "Sketch bar")
            .get_by_label(word)
            .accesskit_node()
            .bounding_box()
    };
    let last = std::cell::Cell::new(place(h));
    step_until(h, "the bar laid out", |h| {
        let now = place(h);
        now == last.replace(now)
    });
    h.get_by_role_and_label(Role::Toolbar, "Sketch bar")
        .get_by_label(word)
        .click();
    step_until(h, "sketch mode closed", |h| sketch_mode(h).is_none());
}

/// The over-define prompt, if it's shown.
pub fn prompt<P: AppKernel>(h: &AppUi<P>) -> Option<egui_kittest::Node<'_>> {
    h.query_by_role_and_label(Role::Dialog, "Dimension over-defines the sketch")
}

/// The Modify box's field, if a box is open.
pub fn modify_field<P: AppKernel>(h: &AppUi<P>) -> Option<egui_kittest::Node<'_>> {
    h.query_by_role_and_label(Role::Dialog, "Modify dimension")?
        .query_by_label("Value in millimetres")
}

// --- The canvas ----------------------------------------------------------------------------

/// The first tab's view.
pub fn view<P: AppKernel>(h: &AppUi<P>) -> &silico_app::viewport::View {
    &h.state().tabs()[0].view
}

/// The first tab's view, to change.
pub fn view_mut<P: AppKernel>(h: &mut AppUi<P>) -> &mut silico_app::viewport::View {
    &mut h.state_mut().tabs_mut()[0].view
}

/// The viewport's place in the window, in points, as it was last frame.
pub fn the_viewport<P: AppKernel>(h: &AppUi<P>) -> Rect {
    view(h).frame().unwrap().rect
}

/// The viewport's frame, as it was last drawn.
pub fn frame<P: AppKernel>(h: &AppUi<P>) -> Option<&silico_app::viewport::ViewFrame> {
    h.state().tabs()[0].view.frame()
}

/// The axes' colours, the grid's, the triad's, and the ViewCube's stubs' (DESIGN.md's
/// `axis_color_x/y/z`: `#ff6981`, `#00b266`, `#7a95ff`), and the triad's dot's (viewport.js's
/// `<circle r="2" fill="#cfcfcf"/>`).
pub const AXIS_X: Color32 = Color32::from_rgb(0xff, 0x69, 0x81);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x00, 0xb2, 0x66);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x7a, 0x95, 0xff);
pub const TRIAD_DOT: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);

/// The triad's origin over the canvas `rect`: 44 in from its left, 44 up from its bottom
/// (viewport.js's `ox = 44, oy = h - 44`).
pub fn triad_origin(rect: Rect) -> Pos2 {
    Pos2::new(rect.left() + 44.0, rect.bottom() - 44.0)
}

/// The way `camera` looks from its target to its eye, made a unit.
pub fn view_direction(camera: &silico_protocol::Camera) -> [f64; 3] {
    let way: [f64; 3] = std::array::from_fn(|k| camera.eye[k] - camera.target[k]);
    let length = (way[0] * way[0] + way[1] * way[1] + way[2] * way[2]).sqrt();
    way.map(|part| part / length)
}

/// Whether `camera` looks from `way`, a unit vector from its target to its eye, each part
/// within 1e-9.
pub fn looks_from(camera: &silico_protocol::Camera, way: [f64; 3]) -> bool {
    let at = view_direction(camera);
    (0..3).all(|k| (at[k] - way[k]).abs() < 1e-9)
}

/// The view label's node in the viewport, as a screen reader finds it: the viewport's one
/// label that names the projection.
pub fn view_label_node<P: AppKernel>(h: &AppUi<P>) -> egui_kittest::Node<'_> {
    let mut labels: Vec<_> = h
        .get_by_role_and_label(Role::Pane, "Viewport")
        .query_all_by_role(Role::Label)
        .filter(|node| {
            node.accesskit_node().value().is_some_and(|words| {
                words.starts_with("Orthographic") || words.starts_with("Perspective")
            })
        })
        .collect();
    assert_eq!(labels.len(), 1, "not one view label");
    labels.remove(0)
}

/// The view label's words, as a screen reader has them.
pub fn view_label<P: AppKernel>(h: &AppUi<P>) -> String {
    view_label_node(h)
        .accesskit_node()
        .value()
        .unwrap_or_default()
}

/// A button's press or release at `at`, `modifiers` held.
pub fn button_event<P: AppKernel>(
    h: &AppUi<P>,
    at: Pos2,
    button: PointerButton,
    pressed: bool,
    modifiers: Modifiers,
) {
    h.event(Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers,
    });
}

/// The pointer moved to `at`; a frame.
pub fn move_to<P: AppKernel>(h: &mut AppUi<P>, at: Pos2) {
    h.hover_at(at);
    step(h);
}

/// Frames stepped until a click would be one of its own, not a double-click's second: as
/// long as a person leaves between two clicks, egui's double-click delay since the last.
pub fn pause<P: AppKernel>(h: &mut AppUi<P>) {
    let delay = h.ctx.options(|o| o.input_options.max_double_click_delay);
    while f64::from(h.ctx.input(|i| i.pointer.time_since_last_click())) <= delay {
        step(h);
    }
}

/// A left click at `at`, Ctrl (Cmd) held if `ctrl`, after a [`pause`]: the pointer moved
/// there a frame, then its press one frame and its release the next.
pub fn click_as_a_person<P: AppKernel>(h: &mut AppUi<P>, at: Pos2, ctrl: bool) {
    pause(h);
    let held = if ctrl {
        Modifiers::COMMAND
    } else {
        Modifiers::NONE
    };
    h.event(Event::ModifiersChanged(held));
    move_to(h, at);
    button_event(h, at, PointerButton::Primary, true, held);
    step(h);
    button_event(h, at, PointerButton::Primary, false, held);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    step(h);
}

/// A left click at `at` as the harness gives one: the pointer there, and the press and the
/// release in one frame, then a frame more.
pub fn click_in_one_frame<P: AppKernel>(h: &mut AppUi<P>, at: Pos2) {
    h.hover_at(at);
    for pressed in [true, false] {
        button_event(h, at, PointerButton::Primary, pressed, Modifiers::NONE);
    }
    step(h);
    step(h);
}

/// A left click at `at`, `held` held through it: the pointer there and the press in one
/// frame, the release in the next.
pub fn click_over_two_frames<P: AppKernel>(h: &mut AppUi<P>, at: Pos2, held: Modifiers) {
    h.event(Event::ModifiersChanged(held));
    h.hover_at(at);
    button_event(h, at, PointerButton::Primary, true, held);
    step(h);
    button_event(h, at, PointerButton::Primary, false, held);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    step(h);
}

/// A wheel of `delta` in `unit`s at `at`.
pub fn wheel<P: AppKernel>(h: &mut AppUi<P>, at: Pos2, unit: MouseWheelUnit, delta: f32) {
    h.hover_at(at);
    h.event(Event::MouseWheel {
        unit,
        delta: eframe::egui::vec2(0.0, delta),
        phase: eframe::egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    step(h);
}

/// `button` pressed at `from`, moved to `to` in `steps` even steps, a frame each, then let
/// go there.
pub fn drag<P: AppKernel>(
    h: &mut AppUi<P>,
    button: PointerButton,
    from: Pos2,
    to: Pos2,
    steps: u16,
) {
    h.hover_at(from);
    button_event(h, from, button, true, Modifiers::NONE);
    step(h);
    for i in 1..=steps {
        h.hover_at(from + (to - from) * f32::from(i) / f32::from(steps));
        step(h);
    }
    button_event(h, to, button, false, Modifiers::NONE);
    step(h);
}

/// The selection's names, in the tab.
pub fn selected_names<P: AppKernel>(h: &AppUi<P>) -> Vec<String> {
    h.state().tabs()[0]
        .selection()
        .names()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// Frames stepped until the first tab's read has answered and nothing more is read: its
/// bodies held, of the published revision, no read out, and none wanted over three more
/// frames.
pub fn meshes_settled<P: AppKernel>(h: &mut AppUi<P>) {
    meshes_settled_in(h, 0);
}

/// [`meshes_settled`] for the tab at `tab`.
pub fn meshes_settled_in<P: AppKernel>(h: &mut AppUi<P>, tab: usize) {
    step_until(h, "the meshes read", |h| {
        h.state().tabs().get(tab).is_some_and(|tab| {
            !tab.view.reading() && tab.published.is_some() && tab.view.model() == tab.published
        })
    });
    for _ in 0..3 {
        step(h);
    }
    step_until(h, "the meshes read", |h| {
        !h.state().tabs()[tab].view.reading()
    });
}

// --- A feature dialog's arrows in the canvas ----------------------------------------------

/// A 40 × 20 plate sketched on XY from the origin, its profile's centroid (20, 10), and its
/// pad, 10 up: the pad's end over the centroid at [`PLATE_END`], the arrows' place.
pub const PLATE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Plate", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 40, "y": 0, "construction": false}, {"id": "pa000003", "x": 40, "y": 20, "construction": false}, {"id": "pa000004", "x": 0, "y": 20, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []},
    {"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}}
  ]
}
"#;

/// The plate's pad's end: its profile's centroid, (20, 10), 10 up.
pub const PLATE_END: [f64; 3] = [20.0, 10.0, 10.0];

/// The arrows' blue (interact.js's `#4676ff`), and the white of the one dragged.
pub const ARROW_BLUE: Color32 = Color32::from_rgb(0x46, 0x76, 0xff);
pub const ARROW_WHITE: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);

/// The plate's sketch.
pub fn plate_sketch() -> FeatureId {
    FeatureId::new("s0000001").unwrap()
}

/// The pad's dialog opened as Edit Feature opens it, a double-click on its timeline item:
/// its length loaded into it, and its sketch's read held, which its arrows stand on.
pub fn edit_plates_pad<P: AppKernel>(h: &mut AppUi<P>) {
    step_until(h, "the pad in the timeline", |h| {
        h.query_by_role_and_label(Role::Pane, "Timeline")
            .is_some_and(|timeline| timeline.query_by_label("Pad").is_some())
    });
    double_click(h, "Timeline", "Pad");
    step_until(h, "its dialog, loaded, its sketch read", |h| {
        h.query_by_role_and_label(Role::Dialog, "Pad").is_some()
            && field(h, In::Dialog, "Length").value().as_deref() == Some("10")
            && h.state().tabs()[0]
                .sketch_reads
                .held(&plate_sketch())
                .is_some()
    });
}

/// The pad's dialog opened on the plate (open in `h`) as [`edit_plates_pad`] opens it, its
/// answers settled, and the view from the front onto the pad's end at 4 points a millimetre
/// ([`from_the_front`]): the image's centre, where the end is.
pub fn plates_pad_from_the_front<P: AppKernel>(h: &mut AppUi<P>) -> Pos2 {
    edit_plates_pad(h);
    settled(h);
    from_the_front(h, PLATE_END, 4.0);
    image_centre(h)
}

/// `points` above `at` on the screen (below it for a negative number).
pub fn up(at: Pos2, points: f32) -> Pos2 {
    at - eframe::egui::vec2(0.0, points)
}

/// The extent of a one-sided pad of `length`, `reversed` or not, as JSON reads it.
pub fn pad_extent(length: &str, reversed: bool) -> Plain {
    Plain::of(
        &json::read(
            format!(r#"{{"type": "one_side", "length": {length}, "reversed": {reversed}}}"#)
                .as_bytes(),
        )
        .unwrap(),
    )
}

/// The first tab's document, as its session holds it now (`text`).
pub fn text_of<P: AppKernel>(sessions: &Sessions<P>, h: &AppUi<P>) -> String {
    let id = h.state().tabs()[0].id;
    sessions.text(&id, None).wait().result.unwrap().text
}

/// The fields of the feature named `name` in `text`, but its `id`, `v`, and `fingerprint`,
/// each as JSON writes it: none if no feature has that name.
pub fn fields_of(text: &str, name: &str) -> Option<Vec<(String, Plain)>> {
    let root = json::read(text.as_bytes()).unwrap();
    let Plain::Object(members) = Plain::of(&root) else {
        panic!("not an object");
    };
    let (_, Plain::Array(features)) = members.into_iter().find(|(key, _)| key == "features")?
    else {
        panic!("no features");
    };
    features.into_iter().find_map(|feature| {
        let Plain::Object(fields) = feature else {
            return None;
        };
        fields
            .iter()
            .any(|(key, value)| key == "name" && *value == Plain::String(name.to_owned()))
            .then(|| sorted_fields(fields))
    })
}

/// `fields`, but `id`, `v`, and `fingerprint`, sorted by key.
pub fn sorted_fields(fields: Vec<(String, Plain)>) -> Vec<(String, Plain)> {
    let mut fields: Vec<(String, Plain)> = fields
        .into_iter()
        .filter(|(key, _)| !["id", "v", "fingerprint"].contains(&key.as_str()))
        .collect();
    fields.sort_by(|a, b| a.0.cmp(&b.0));
    fields
}

/// The field `field` of the feature named `name` in the first tab's document, as its session
/// holds it now ([`fields_of`]): none if no feature has that name, or it has no such field.
pub fn written<P: AppKernel>(
    sessions: &Sessions<P>,
    h: &AppUi<P>,
    name: &str,
    field: &str,
) -> Option<Plain> {
    fields_of(&text_of(sessions, h), name)?
        .into_iter()
        .find(|(key, _)| key == field)
        .map(|(_, value)| value)
}

/// The extent of the feature named Pad in the first tab's document, as its session holds it
/// now ([`written`]).
pub fn pads_extent<P: AppKernel>(sessions: &Sessions<P>, h: &AppUi<P>) -> Plain {
    written(sessions, h, "Pad", "extent").unwrap()
}

/// The text `tab`'s open dialog's Length input holds, as the tab has it (its field's,
/// drawn or not): none with no dialog, or one with no Length.
pub fn dialog_length(tab: &silico_app::tab::Tab) -> Option<String> {
    tab.dialog.as_ref().and_then(|dialog| {
        dialog.inputs.iter().find_map(|input| match &input.value {
            silico_app::dialog::Value::Length(text) if input.label == "Length" => {
                Some(text.clone())
            }
            _ => None,
        })
    })
}

/// Whether the open dialog's Reversed is ticked.
pub fn reversed_ticked<P: AppKernel>(h: &AppUi<P>) -> bool {
    dialog(h)
        .get_by_role_and_label(Role::CheckBox, "Reversed")
        .accesskit_node()
        .toggled()
        == Some(eframe::egui::accesskit::Toggled::True)
}

/// The view from the front onto `target`, Z up, orthographic at `per_mm` points a
/// millimetre, and a frame drawn through it: X right and Z up the screen, `target` at the
/// image's centre ([`image_centre`]).
pub fn from_the_front<P: AppKernel>(h: &mut AppUi<P>, target: [f64; 3], per_mm: f64) {
    let at = frame(h).unwrap();
    let rows = f64::from(at.size_px[1]);
    let ppp = f64::from(at.pixels_per_point);
    view_mut(h).set_camera(Camera {
        projection: Projection::Orthographic {
            view_height: rows / ppp / per_mm,
        },
        eye: [target[0], target[1] - 500.0, target[2]],
        target,
        up: [0.0, 0.0, 1.0],
    });
    step(h);
    step(h);
}

/// Where the image's centre is on the screen, in points: the viewport's top left, and half
/// its pixels each way at its pixels a point, as the pane places a pixel. What the view's
/// target lands on.
pub fn image_centre<P: AppKernel>(h: &AppUi<P>) -> Pos2 {
    let at = frame(h).unwrap();
    let ppp = at.pixels_per_point;
    at.rect.min
        + eframe::egui::vec2(
            at.size_px[0] as f32 / 2.0 / ppp,
            at.size_px[1] as f32 / 2.0 / ppp,
        )
}

/// Every shape the last frame painted, nested ones flattened.
pub fn shapes_painted<P: AppKernel>(h: &AppUi<P>) -> Vec<Shape> {
    fn flat(shape: &Shape, into: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| flat(s, into)),
            other => into.push(other.clone()),
        }
    }
    let mut shapes = Vec::new();
    for clipped in &h.output().shapes {
        flat(&clipped.shape, &mut shapes);
    }
    shapes
}

/// The distance arrows' dots painted last frame (a disc 3.5 in radius, in the arrows' blue or
/// white): each one's centre and colour.
pub fn arrow_dots<P: AppKernel>(h: &AppUi<P>) -> Vec<(Pos2, Color32)> {
    shapes_painted(h)
        .into_iter()
        .filter_map(|shape| match shape {
            Shape::Circle(dot)
                if dot.radius == 3.5 && [ARROW_BLUE, ARROW_WHITE].contains(&dot.fill) =>
            {
                Some((dot.center, dot.fill))
            }
            _ => None,
        })
        .collect()
}

/// The arrows' shafts painted last frame (a segment 2.5 wide in the arrows' blue or white):
/// each one's ends and colour.
pub fn arrow_shafts<P: AppKernel>(h: &AppUi<P>) -> Vec<([Pos2; 2], Color32)> {
    shapes_painted(h)
        .into_iter()
        .filter_map(|shape| match shape {
            Shape::LineSegment { points, stroke }
                if stroke.width == 2.5 && [ARROW_BLUE, ARROW_WHITE].contains(&stroke.color) =>
            {
                Some((points, stroke.color))
            }
            _ => None,
        })
        .collect()
}

/// Whether `got` is `expected`, to a thousandth of a point (the renderer's projection is
/// f32's).
pub fn near_point(got: Pos2, expected: Pos2) -> bool {
    (got - expected).length() < 1e-3
}

/// The cursor the last frame asked for.
pub fn cursor<P: AppKernel>(h: &AppUi<P>) -> CursorIcon {
    h.output().platform_output.cursor_icon
}

/// The left button pressed at `at`, the pointer there: a frame.
pub fn left_down_at<P: AppKernel>(h: &mut AppUi<P>, at: Pos2) {
    h.hover_at(at);
    button_event(h, at, PointerButton::Primary, true, Modifiers::NONE);
    step(h);
}

/// The left button let go at `at`, the pointer there: a frame.
pub fn left_up_at<P: AppKernel>(h: &mut AppUi<P>, at: Pos2) {
    h.hover_at(at);
    button_event(h, at, PointerButton::Primary, false, Modifiers::NONE);
    step(h);
}

// --- The stand-ins -------------------------------------------------------------------------

/// [`FakeBrowser`]'s state: what each dialog was asked, the answer the test gave (`None`
/// until it does), the waker of the last poll, how many polls there were, and whether a
/// dialog asked for opens none.
#[derive(Default)]
pub struct Dialogs {
    asked: Vec<Ask>,
    answer: Option<Option<PathBuf>>,
    waker: Option<std::task::Waker>,
    polls: u64,
    refusing: bool,
}

/// The platform's file dialog, stood in for through the same `Browse` trait (the platform's
/// own needs a window server and a person): it records what each dialog was asked and
/// answers when the test says, from a thread of its own, as the platform's completion does;
/// or, told to, opens none, as when the platform's won't.
#[derive(Clone, Default)]
pub struct FakeBrowser(Arc<Mutex<Dialogs>>);

impl Browse for FakeBrowser {
    fn browse(&mut self, ask: &Ask) -> Option<Answer> {
        let mut dialogs = self.0.lock().unwrap();
        dialogs.asked.push(ask.clone());
        if dialogs.refusing {
            return None;
        }
        dialogs.answer = None;
        Some(Box::pin(FakeAnswer(Arc::clone(&self.0))))
    }
}

/// A dialog [`FakeBrowser`] opened, answered when the test says.
pub struct FakeAnswer(Arc<Mutex<Dialogs>>);

impl std::future::Future for FakeAnswer {
    type Output = Option<PathBuf>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<PathBuf>> {
        let mut dialogs = self.0.lock().unwrap();
        dialogs.polls += 1;
        match dialogs.answer.take() {
            Some(answer) => std::task::Poll::Ready(answer),
            None => {
                dialogs.waker = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

impl FakeBrowser {
    /// The dialog open answers `answer` (`None`: cancelled), its waker woken on a thread of
    /// its own.
    pub fn answer(&self, answer: Option<&Path>) {
        let waker = {
            let mut dialogs = self.0.lock().unwrap();
            dialogs.answer = Some(answer.map(Path::to_path_buf));
            dialogs.waker.take()
        };
        std::thread::spawn(move || {
            if let Some(waker) = waker {
                waker.wake();
            }
        })
        .join()
        .unwrap();
    }

    /// What each dialog was asked, in order.
    pub fn asked(&self) -> Vec<Ask> {
        self.0.lock().unwrap().asked.clone()
    }

    /// Each dialog asked for from now on opens none (`refusing`), or opens.
    pub fn refusing(&self, refusing: bool) {
        self.0.lock().unwrap().refusing = refusing;
    }

    /// How many times the dialogs opened have been polled.
    pub fn polls(&self) -> u64 {
        self.0.lock().unwrap().polls
    }
}

/// How many meshes the fake kernel has made.
pub fn tessellated(kernel: &FakeKernel) -> usize {
    kernel
        .calls()
        .iter()
        .filter(|call| matches!(call.op, Op::Tessellate { .. }))
        .count()
}

/// The file at `path` read plainly, as a disk watch's reader for a test that compares with no
/// interval: its bytes, gone if nothing is there, unreadable on any other error. None of the
/// session's own checks (`silico_core::disk`'s reader: the workspace, a regular file, and
/// `MAX_INPUT_LEN`, past which it's unreadable).
pub fn plain_read(path: &Path) -> Found {
    match std::fs::read(path) {
        Ok(bytes) => Found::Bytes(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Found::Gone,
        Err(_) => Found::Unreadable,
    }
}

/// A clock that says what the test sets, from now: frozen until [`advance`]d.
pub fn hand_clock() -> (Arc<Mutex<Instant>>, Clock) {
    let time = Arc::new(Mutex::new(Instant::now()));
    let read = Arc::clone(&time);
    (time, Arc::new(move || *read.lock().unwrap()))
}

/// [`hand_clock`]'s time moved on `by`.
pub fn advance(time: &Mutex<Instant>, by: Duration) {
    *time.lock().unwrap() += by;
}

// --- Lavapipe ------------------------------------------------------------------------------

/// `test` run where lavapipe is this process's Vulkan driver: in a run of this test alone
/// with lavapipe's variables, if this process hasn't them (a loader's path can't be set
/// once the process runs), as silico-render's own frame tests run. With no lavapipe built
/// in (none in the environment), here, where its check of the adapter fails the test: it
/// never runs on another.
pub fn on_lavapipe(name: &str, test: impl FnOnce()) {
    if let Some(vars) = silico_render::lavapipe::environment()
        && vars
            .iter()
            .any(|(k, v)| std::env::var(k).map_or(true, |have| have != *v))
    {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([name, "--exact", "--nocapture"])
            .envs(vars)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{said}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(said.contains("1 passed"), "{name} didn't run: {said}");
        // What the run said of itself, passed on.
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &out.stderr);
        return;
    }
    test();
}

/// eframe's render state as the app gets it, on lavapipe: egui_kittest's wgpu setup (which
/// takes a CPU adapter first), its device made by egui_wgpu as eframe makes the app's.
pub fn lavapipe_render_state() -> eframe::egui_wgpu::RenderState {
    let mut setup = egui_kittest::wgpu::default_wgpu_setup();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(new) = &mut setup {
        new.instance_descriptor.flags |=
            eframe::wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER;
    }
    let state = egui_kittest::wgpu::create_render_state(
        setup,
        eframe::egui_wgpu::RendererOptions::PREDICTABLE,
    );
    // Lavapipe: a CPU adapter on Vulkan, not GL's llvmpipe (egui_kittest's wgpu has gles).
    let info = state.adapter.get_info();
    assert_eq!(
        (info.device_type, info.backend),
        (eframe::wgpu::DeviceType::Cpu, eframe::wgpu::Backend::Vulkan),
        "not lavapipe: {info:?}"
    );
    state
}

// --- Fixtures ------------------------------------------------------------------------------

/// Sessions on the fake kernel `kernel` and the sketch solver `sketches`, the rebuild cache
/// sealed with a secret kept in `dir`: every fake-kernel fixture's base, on which each file
/// layers its own builder steps (STEP written through the fake, the disk compared).
pub fn stand_in_sessions(
    dir: &TempDir,
    kernel: &Arc<FakeKernel>,
    sketches: &FakeSketches,
) -> Sessions<Arc<FakeKernel>> {
    let secret = dir.path().join("secret");
    let solver = sketches.clone();
    Sessions::with_stand_ins(
        Arc::clone(kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    )
}

/// The kernel refusing to mesh while the flag given back is set.
pub fn failing_kernel(kernel: &FakeKernel) -> Arc<AtomicBool> {
    let failing = Arc::new(AtomicBool::new(false));
    let refusing = Arc::clone(&failing);
    kernel.refuse(move |op| {
        (matches!(op, Op::Tessellate { .. }) && refusing.load(Ordering::SeqCst))
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the worker crashed meshing"))
    });
    failing
}

// --- Documents ---------------------------------------------------------------------------

/// The M2.1 document's example (docs/design/m2.1-document-format.md): a block, a pin, and
/// the hole the pin cuts in it.
pub const EXAMPLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
    {"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12},
    {"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de", "note": "Clearance for an M5 bolt"}
  ]
}
"#;

/// The M2.1 example with a triangle sketched on XY, not yet swept: a part to make a feature
/// in sketch-first, as SolidWorks makes solids (Trevin's decision of 2026-11-09: the app
/// makes no primitive). The triangle, (30, 0), (50, 0), (30, 10), lies half on the block's
/// floor and half past its end (x = 40), so its pad, 10 up from z = 0, is one solid with the
/// hole's body and changes it. The hole, the tip, is a Pad's base.
pub const SKETCHED_EXAMPLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
    {"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12},
    {"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de", "note": "Clearance for an M5 bolt"},
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Triangle", "plane": "xy", "points": [{"id": "pa000000", "x": 30, "y": 0, "construction": false}, {"id": "pb000000", "x": 50, "y": 0, "construction": false}, {"id": "pc000000", "x": 30, "y": 10, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}, {"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}, {"id": "nc000000", "from": "pc000000", "to": "pa000000", "construction": false}], "circles": [], "arcs": [], "constraints": []}
  ]
}
"#;

/// Two boxes, each a body: a 50 mm cube at the origin, and a 1 m cube 2 m along x. The
/// fake meshes a box at its floor, its box's diagonal × 1e-4 (`mesh_floor`): the small
/// one's 0.00866 mm, under every level these tests view it at; the large one's
/// 1000√3 × 1e-4 = 0.173205 mm, over all of them, so it's meshed at its floor, which
/// serves every half pixel under 2 · 0.125 (its floor's level, 2⁻³).
pub const TWO_BOXES: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Small", "position": [0, 0, 0], "length": 50, "width": 50, "height": 50},
    {"id": "a91cw0de", "type": "box", "v": 1, "name": "Large", "position": [2000, 0, 0], "length": 1000, "width": 1000, "height": 1000}
  ]
}
"#;

/// A block alone: its box is its body.
pub const A_BOX: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;
