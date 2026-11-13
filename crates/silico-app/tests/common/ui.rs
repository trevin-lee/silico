//! The app driven as a person drives it, on either kernel ([`AppKernel`]): each pane and
//! control found by its role and label in the accessibility tree and clicked or typed into,
//! keys pressed, and the canvas given the pointer's own events; and reads of what the window
//! and the tab show, which act on nothing. Every frame is stepped on a thread marked so that
//! a wait on a command there is counted.
//!
//! The app's tests take their UI helpers from here, one copy each: tests/app.rs, the
//! hostile-input tests, and the slice. How long a wait may take follows the kernel the app
//! runs on: [`WAIT`] on the fake kernel, [`SLICE_WAIT`] on the real workers. Two pairs do
//! one job two ways, and are kept apart because the events each sends are what its tests
//! have checked the app against:
//! - [`click_at`] gives the press and the release in one frame, as the harness's own clicks
//!   do; [`click_as_a_person`] waits out egui's double-click delay first, so that no click
//!   is read as another's second, and gives the press and the release a frame each;
//! - [`retype`] takes a field's characters away a Backspace a frame and reads `text` back at
//!   once; [`type_over`] takes them in one frame (Home, and a Delete each) and waits for the
//!   field to read `text`.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::accesskit::Role;
use eframe::egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use silico_app::App;
use silico_core::fake::{FakeKernel, never_waits};
use silico_core::kernel::Clients;
use silico_core::pool::KernelPool;

/// The app in a harness.
pub type AppUi<P> = Harness<'static, App<P>>;

/// Long enough for anything the fake kernel does on a loaded machine.
pub const WAIT: Duration = Duration::from_secs(60);

/// Long enough for anything a real worker does on a loaded machine, a first start included:
/// longer than [`WAIT`], since here every answer is a real kernel's, sketch worker's, or STEP
/// worker's, and every frame is drawn on lavapipe.
pub const SLICE_WAIT: Duration = Duration::from_secs(120);

/// A kernel the app is driven on here, and how long [`step_until`] waits on it.
pub trait AppKernel: Clients + 'static {
    /// The longest a wait for the app's answers may take.
    const WAIT: Duration;
}

/// The fake kernel, which app.rs and the hostile-input tests drive the app on.
impl AppKernel for Arc<FakeKernel> {
    const WAIT: Duration = WAIT;
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
    h.hover_at(Pos2::new(640.0, 400.0));
    step(h);
    step(h);
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

/// Command+`key` pressed (Z undoes, Y redoes) once Undo, or Redo, is on in the first tab, and
/// its answer settled.
pub fn keyed<P: AppKernel>(h: &mut AppUi<P>, key: Key) {
    if key == Key::Z {
        step_until(h, "undoable", |h| h.state().tabs()[0].undoable());
    } else {
        step_until(h, "redoable", |h| h.state().tabs()[0].redoable());
    }
    h.key_press_modifiers(Modifiers::COMMAND, key);
    step(h);
    settled(h);
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

// --- The canvas ----------------------------------------------------------------------------

/// The viewport's frame, as it was last drawn.
pub fn frame<P: AppKernel>(h: &AppUi<P>) -> Option<&silico_app::viewport::ViewFrame> {
    h.state().tabs()[0].view.frame()
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
pub fn click_at<P: AppKernel>(h: &mut AppUi<P>, at: Pos2) {
    h.hover_at(at);
    for pressed in [true, false] {
        button_event(h, at, PointerButton::Primary, pressed, Modifiers::NONE);
    }
    step(h);
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
